// Instanced 2D sprite shader for fnfrs-render.
// Expands a unit quad [0..1] per instance, rotates around `origin`, handles
// Sparrow rotated subtextures (90 deg CW restoration), flip X/Y, optional
// pixel-art snapping, RGB<->HSV color shifting, and vertex color tinting.

struct CameraUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(1) @binding(0)
var sprite_tex: texture_2d<f32>;

@group(1) @binding(1)
var sprite_sampler: sampler;

const FLAG_ROTATED_90_CW: u32 = 1u;
const FLAG_FLIP_X: u32 = 2u;
const FLAG_FLIP_Y: u32 = 4u;
const FLAG_PIXEL_SNAP: u32 = 8u;
const FLAG_ROTATED_90_CCW: u32 = 16u;

struct SpriteInstanceInput {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_rect: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) hsv_shift: vec3<f32>,
    @location(5) rotation: f32,
    @location(6) origin: vec2<f32>,
    @location(7) flags: u32,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) hsv_shift: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    inst: SpriteInstanceInput,
) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );

    let corner = corners[vertex_index % 6u];

    // Rotate local position around `inst.origin` (relative to unrotated top-left).
    let local_pos = corner * inst.size - inst.origin;
    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let rotated_pos = vec2<f32>(
        local_pos.x * c - local_pos.y * s,
        local_pos.x * s + local_pos.y * c,
    );
    var world_pos = inst.pos + inst.origin + rotated_pos;

    if (inst.flags & FLAG_PIXEL_SNAP) != 0u {
        world_pos = round(world_pos);
    }

    // Apply flips in upright sprite space before unrotating packed atlas UVs.
    let flip_x = (inst.flags & FLAG_FLIP_X) != 0u;
    let flip_y = (inst.flags & FLAG_FLIP_Y) != 0u;
    let flipped = vec2<f32>(
        select(corner.x, 1.0 - corner.x, flip_x),
        select(corner.y, 1.0 - corner.y, flip_y),
    );

    var local_uv = flipped;
    if (inst.flags & FLAG_ROTATED_90_CW) != 0u {
        // Sparrow packs rotated subtextures 90 deg CCW in the atlas; rotate +90 deg CW to restore.
        local_uv = vec2<f32>(flipped.y, 1.0 - flipped.x);
    } else if (inst.flags & FLAG_ROTATED_90_CCW) != 0u {
        // Adobe Animate spritemaps pack rotated subtextures 90 deg CW; rotate -90 deg CCW to restore.
        local_uv = vec2<f32>(1.0 - flipped.y, flipped.x);
    }

    let uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, local_uv);

    var out: VertexOutput;
    out.clip_pos = camera.view_proj * vec4<f32>(world_pos, 0.0, 1.0);
    out.uv = uv;
    out.color = inst.color;
    out.hsv_shift = inst.hsv_shift;
    return out;
}

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let k = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4<f32>(c.bg, k.wz), vec4<f32>(c.gb, k.xy), step(c.b, c.g));
    let q = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + k.xyz) * 6.0 - k.www);
    return c.z * mix(k.xxx, clamp(p - k.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(sprite_tex, sprite_sampler, in.uv);
    var rgb = tex_color.rgb;

    if any(in.hsv_shift != vec3<f32>(0.0)) {
        var hsv = rgb_to_hsv(rgb);
        hsv.x = fract(hsv.x + in.hsv_shift.x);
        hsv.y = clamp(hsv.y * (1.0 + in.hsv_shift.y), 0.0, 1.0);
        hsv.z = clamp(hsv.z * (1.0 + in.hsv_shift.z), 0.0, 1.0);
        rgb = hsv_to_rgb(hsv);
    }

    return vec4<f32>(rgb * in.color.rgb, tex_color.a * in.color.a);
}
