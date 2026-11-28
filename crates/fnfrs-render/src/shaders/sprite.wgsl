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
const FLAG_AFFINE: u32 = 32u;
const FLAG_ADJUST_COLOR: u32 = 64u;
const FLAG_WIGGLE: u32 = 128u;
const FLAG_RAIN: u32 = 256u;

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
    @location(2) hsv_shift: vec3<f32>,
    @location(3) hsbc: vec4<f32>,
    @location(4) @interpolate(flat) flags: u32,
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
        // Exact 2x2 affine quad: `inst.size` = X-edge (m.a*w, m.b*w), `inst.origin` = Y-edge (m.c*h, m.d*h).
        world_pos = inst.pos + corner.x * inst.size + corner.y * inst.origin;
    } else {
        // Rotate local position around `inst.origin` (relative to unrotated top-left).
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
    out.hsbc = inst.hsbc;
    out.flags = inst.flags;
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

fn apply_adjust_color(color: vec3<f32>, hsbc: vec4<f32>) -> vec3<f32> {
    // Matches Adobe Animate/Flash AdjustColorShader.hx (`hsbc = [hue_rad, sat_mult, bright_add, contrast_mult]`).
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

fn rain_rand2(a: vec2<f32>) -> f32 {
    let m = a - vec2<f32>(1000.0) * floor(a / vec2<f32>(1000.0));
    return fract(sin(dot(m, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn sprite_rain_dist(p_in: vec2<f32>, time: f32, scale: f32, intensity: f32) -> f32 {
    var p = p_in * 0.1;
    p.x += p.y * 0.1;
    p.y -= time * 500.0 / scale;
    p.y *= 0.03;
    let ix = floor(p.x);
    let mod_ix = ix - 2.0 * floor(ix * 0.5);
    p.y += mod_ix * 0.5 + (rain_rand2(vec2<f32>(ix, ix)) - 0.5) * 0.3;
    let iy = floor(p.y);
    let idx = vec2<f32>(ix, iy);
    p -= idx;
    p.x += (rain_rand2(idx.yx) * 2.0 - 1.0) * 0.35;
    let a = abs(p - 0.5);
    let res = max(a.x * 0.8, a.y * 0.5) - 0.1;
    let empty = rain_rand2(idx) < mix(1.0, 0.1, intensity);
    return select(res, 1.0, empty);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var sample_uv = in.uv;
    let tex_sz = vec2<f32>(textureDimensions(sprite_tex, 0));
    if (in.flags & FLAG_WIGGLE) != 0u {
        // Flixel WiggleEffect DREAMY mode matching wiggle.frag exactly.
        // hsbc = [time, freq, speed, amp].
        let w = 1.0 / tex_sz.y;  // pixel height in UV
        let h = 1.0 / tex_sz.x;  // pixel width  in UV

        // Snap X to pixel grid
        sample_uv.x = floor(sample_uv.x / h) * h;

        // X → Y distortion (full freq/speed/amp)
        let offset_x = sin(sample_uv.x * in.hsbc.y + in.hsbc.x * in.hsbc.z) * in.hsbc.w;
        sample_uv.y = sample_uv.y + floor(offset_x / w) * w;
        sample_uv.y = floor(sample_uv.y / w) * w;

        // Y → X distortion (half freq/speed/amp)
        let offset_y = sin(sample_uv.y * (in.hsbc.y * 0.5) + in.hsbc.x * (in.hsbc.z * 0.5)) * (in.hsbc.w * 0.5);
        sample_uv.x = clamp(sample_uv.x + floor(offset_y / h) * h, 0.0, 1.0);
        sample_uv.y = clamp(sample_uv.y, 0.0, 1.0);
    }

    var rain_add = vec3<f32>(0.0);
    var rain_sum = 0.0;
    if (in.flags & FLAG_RAIN) != 0u {
        // Sprite-mode RuntimeRainShader (`hsbc = [time, scale, intensity, 0.0]`).
        let r_time = in.hsbc.x;
        let r_scale = max(in.hsbc.y, 0.1);
        let r_intensity = in.hsbc.z;
        let orig_wpos = sample_uv * tex_sz;
        var wpos = orig_wpos;
        var scales = array<f32, 4>(1.0, 1.8, 2.6, 4.8);
        for (var i = 0; i < 4; i = i + 1) {
            let s = scales[i];
            let r = sprite_rain_dist(wpos * s / r_scale + 500.0 * f32(i), r_time, s, r_intensity);
            if r < 0.0 {
                let v = (1.0 - exp(r * 5.0)) / s * 2.0;
                wpos.x += v * 10.0 * r_scale;
                wpos.y -= v * 2.0 * r_scale;
                rain_add += vec3<f32>(0.1, 0.15, 0.2) * v;
                rain_sum += (1.0 - rain_sum) * 0.75;
            }
        }
        sample_uv = clamp(wpos / tex_sz, vec2<f32>(0.0), vec2<f32>(1.0));
    }

    let tex_color = textureSample(sprite_tex, sprite_sampler, sample_uv);
    var rgb = tex_color.rgb;
    if (in.flags & FLAG_RAIN) != 0u {
        rgb = rgb + rain_add;
        rgb = mix(rgb, vec3<f32>(0.659, 0.678, 0.710), 0.1 * rain_sum);
    }

    if (in.flags & FLAG_ADJUST_COLOR) != 0u {
        rgb = apply_adjust_color(rgb, in.hsbc);
    }

    if any(in.hsv_shift != vec3<f32>(0.0)) {
        var hsv = rgb_to_hsv(rgb);
        hsv.x = fract(hsv.x + in.hsv_shift.x);
        hsv.y = clamp(hsv.y * (1.0 + in.hsv_shift.y), 0.0, 1.0);
        hsv.z = clamp(hsv.z * (1.0 + in.hsv_shift.z), 0.0, 1.0);
        rgb = hsv_to_rgb(hsv);
    }

    return vec4<f32>(rgb * in.color.rgb, tex_color.a * in.color.a);
}
