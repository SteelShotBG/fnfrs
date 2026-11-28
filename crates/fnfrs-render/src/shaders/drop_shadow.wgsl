// Per-sprite drop-shadow / rim-light shader matching DropShadowShader.hx
// Replicates Adobe Animate/Flash drop shadow with threshold-based rim lighting,
// integrated AdjustColor, difference-based AA, and optional alternate mask support.

struct CameraUniform {
    view_proj: mat4x4<f32>,
};

struct DropShadowUniform {
    drop_color: vec4<f32>, // rgb = dropColor, a = strength
    params: vec4<f32>,     // x = angle_rad, y = distance, z = threshold, w = mask_threshold
    hsbc: vec4<f32>,       // x = hue_rad, y = sat_mult, z = bright_add, w = contrast_mult
    flags: vec4<u32>,      // x = use_mask (0 or 1)
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(1) @binding(0)
var sprite_tex: texture_2d<f32>;

@group(1) @binding(1)
var sprite_sampler: sampler;

@group(2) @binding(0)
var<uniform> ds: DropShadowUniform;

@group(3) @binding(0)
var alt_mask_tex: texture_2d<f32>;

@group(3) @binding(1)
var alt_mask_sampler: sampler;

const FLAG_ROTATED_90_CW: u32 = 1u;
const FLAG_FLIP_X: u32 = 2u;
const FLAG_FLIP_Y: u32 = 4u;
const FLAG_PIXEL_SNAP: u32 = 8u;
const FLAG_ROTATED_90_CCW: u32 = 16u;
const FLAG_AFFINE: u32 = 32u;

struct SpriteInstanceInput {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_rect: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) hsv_shift: vec3<f32>,
    @location(5) rotation: f32,
    @location(6) origin: vec2<f32>,
    @location(7) flags: u32,
    @location(8) hsbc: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv_rect: vec4<f32>,
    @location(3) @interpolate(flat) flags: u32,
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

    var world_pos: vec2<f32>;
    if (inst.flags & FLAG_AFFINE) != 0u {
        world_pos = inst.pos + corner.x * inst.size + corner.y * inst.origin;
    } else {
        let local_pos = corner * inst.size - inst.origin;
        let c = cos(inst.rotation);
        let s = sin(inst.rotation);
        let rotated_pos = vec2<f32>(
            local_pos.x * c - local_pos.y * s,
            local_pos.x * s + local_pos.y * c,
        );
        world_pos = inst.pos + inst.origin + rotated_pos;
    }

    if (inst.flags & FLAG_PIXEL_SNAP) != 0u {
        world_pos = round(world_pos);
    }

    let flip_x = (inst.flags & FLAG_FLIP_X) != 0u;
    let flip_y = (inst.flags & FLAG_FLIP_Y) != 0u;
    let flipped = vec2<f32>(
        select(corner.x, 1.0 - corner.x, flip_x),
        select(corner.y, 1.0 - corner.y, flip_y),
    );

    var local_uv = flipped;
    if (inst.flags & FLAG_ROTATED_90_CW) != 0u {
        local_uv = vec2<f32>(flipped.y, 1.0 - flipped.x);
    } else if (inst.flags & FLAG_ROTATED_90_CCW) != 0u {
        local_uv = vec2<f32>(1.0 - flipped.y, flipped.x);
    }

    let uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, local_uv);

    var out: VertexOutput;
    out.clip_pos = camera.view_proj * vec4<f32>(world_pos, 0.0, 1.0);
    out.uv = uv;
    out.color = inst.color;
    out.uv_rect = inst.uv_rect;
    out.flags = inst.flags;
    return out;
}

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn get_luma_rgb(c: vec3<f32>) -> f32 {
    return dot(c, LUMA);
}

fn apply_adjust_color(color: vec3<f32>, hsbc: vec4<f32>) -> vec3<f32> {
    let bh_in = color + hsbc.z;
    let c = cos(hsbc.x);
    let s = sin(hsbc.x);
    let wr = 0.299;
    let wg = 0.587;
    let wb = 0.114;
    let h0 = vec3<f32>(wr + (1.0 - wr) * c - wr * s, wg - wg * c - wg * s, wb - wb * c + (1.0 - wb) * s);
    let h1 = vec3<f32>(wr - wr * c + 0.143 * s, wg + (1.0 - wg) * c + 0.140 * s, wb - wb * c - 0.283 * s);
    let h2 = vec3<f32>(wr - wr * c - (1.0 - wr) * s, wg - wg * c + wg * s, wb + (1.0 - wb) * c + wb * s);
    let bh = vec3<f32>(dot(bh_in, h0), dot(bh_in, h1), dot(bh_in, h2));

    let contrasted = (bh - 0.25) * hsbc.w + 0.25;

    let lr = 0.2126;
    let lg = 0.7152;
    let lb = 0.0722;
    let sat = hsbc.y;
    let inv = 1.0 - sat;
    let s0 = vec3<f32>(lr * inv + sat, lg * inv, lb * inv);
    let s1 = vec3<f32>(lr * inv, lg * inv + sat, lb * inv);
    let s2 = vec3<f32>(lr * inv, lg * inv, lb * inv + sat);
    return vec3<f32>(dot(contrasted, s0), dot(contrasted, s1), dot(contrasted, s2));
}

fn lwidth_manual(center: f32, uv: vec2<f32>, px: vec2<f32>) -> f32 {
    let p2x1 = textureSample(sprite_tex, sprite_sampler, uv + vec2<f32>(1.0, 0.0) * px).rgb;
    let p1x2 = textureSample(sprite_tex, sprite_sampler, uv + vec2<f32>(0.0, 1.0) * px).rgb;
    let p2x2 = textureSample(sprite_tex, sprite_sampler, uv + vec2<f32>(1.0, 1.0) * px).rgb;

    let right = get_luma_rgb(p2x1);
    let down = get_luma_rgb(p1x2);
    let diagonal = get_luma_rgb(p2x2);

    let dx = abs(right - center);
    let dy = abs(down - center);
    let dd = abs(diagonal - center);

    return ((dx + dy + dd * 0.7) / (1.0 + 1.0 + 0.7)) * 2.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(sprite_tex, sprite_sampler, in.uv);
    let mask_sample = textureSample(alt_mask_tex, alt_mask_sampler, in.uv);

    let tex_sz = vec2<f32>(textureDimensions(sprite_tex, 0));
    let ratio = 1.0 / tex_sz;
    let px = max(ratio, fwidth(in.uv));

    let color3_light = get_luma_rgb(tex_color.rgb);
    let delta = lwidth_manual(color3_light, in.uv, px);

    let use_alt = ds.flags.x != 0u && mask_sample.b > 0.0;
    let threshold = select(ds.params.z, ds.params.w, use_alt);
    let intensity = smoothstep(threshold - delta, threshold + delta, color3_light);

    var angle_offset = 0.0;
    if (in.flags & FLAG_ROTATED_90_CW) != 0u {
        angle_offset = -1.5707963;
    } else if (in.flags & FLAG_ROTATED_90_CCW) != 0u {
        angle_offset = 1.5707963;
    }
    let eff_angle = ds.params.x + angle_offset;
    let ang_cos = cos(eff_angle);
    let ang_sin = sin(eff_angle);
    let dist = ds.params.y;
    let str = ds.drop_color.a;

    let checked = vec2<f32>(
        in.uv.x + (dist * ang_cos * ratio.x),
        in.uv.y - (dist * ang_sin * ratio.y),
    );
    let in_bounds = checked.x > in.uv_rect.x &&
        checked.y > in.uv_rect.y &&
        checked.x < in.uv_rect.z &&
        checked.y < in.uv_rect.w;
    let sampled_shadow = textureSample(
        sprite_tex,
        sprite_sampler,
        clamp(checked, in.uv_rect.xy, in.uv_rect.zw),
    ).a;
    let shadow_alpha = select(0.0, sampled_shadow, in_bounds);

    let rim = (1.0 - (shadow_alpha * str)) * intensity;

    let color3_no_effect = select(tex_color.rgb, tex_color.rgb / tex_color.a, tex_color.a > 0.0);
    var color3 = apply_adjust_color(color3_no_effect, ds.hsbc);
    color3 = color3 + ds.drop_color.rgb * rim;

    return vec4<f32>(color3 * tex_color.a, tex_color.a) * in.color;
}
