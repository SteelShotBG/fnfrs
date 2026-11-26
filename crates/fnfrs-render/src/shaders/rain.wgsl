// Fullscreen RuntimeRainShader post-processing pass for Weekend 1 / Erect rain.

struct RainUniforms {
    time: f32,
    scale: f32,
    intensity: f32,
    puddle_y: f32,
    rain_color: vec4<f32>,
    screen_res: vec2<f32>,
    cam_pos: vec2<f32>,
    cam_zoom: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0)
var<uniform> u: RainUniforms;

@group(0) @binding(1)
var scene_tex: texture_2d<f32>;

@group(0) @binding(2)
var scene_sampler: sampler;

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VertexOut {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    let p = pos[vid % 3u];
    var out: VertexOut;
    out.clip_pos = vec4<f32>(p, 0.0, 1.0);
    // Flip Y for top-left UV origin.
    out.uv = vec2<f32>((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return out;
}

fn glsl_mod2(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return a - b * floor(a / b);
}

fn glsl_modf(a: f32, b: f32) -> f32 {
    return a - b * floor(a / b);
}

fn rand2(a: vec2<f32>) -> f32 {
    let m = glsl_mod2(a, vec2<f32>(1000.0));
    return fract(sin(dot(m, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn rain_dist(p_in: vec2<f32>, scale: f32, intensity: f32) -> f32 {
    var p = p_in * 0.1;
    p.x += p.y * 0.1;
    p.y -= u.time * 500.0 / scale;
    p.y *= 0.03;
    let ix = floor(p.x);
    p.y += glsl_modf(ix, 2.0) * 0.5 + (rand2(vec2<f32>(ix, ix)) - 0.5) * 0.3;
    let iy = floor(p.y);
    let idx = vec2<f32>(ix, iy);
    p -= idx;
    p.x += (rand2(idx.yx) * 2.0 - 1.0) * 0.35;
    let a = abs(p - 0.5);
    let res = max(a.x * 0.8, a.y * 0.5) - 0.1;
    let empty = rand2(idx) < mix(1.0, 0.1, intensity);
    return select(res, 1.0, empty);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let zoom = max(u.cam_zoom, 0.05);
    let centered = (in.uv - 0.5) * (u.screen_res / zoom);
    let orig_wpos = u.cam_pos + centered;
    var wpos = orig_wpos;

    let intensity = u.intensity;
    var add = vec3<f32>(0.0);
    var rain_sum = 0.0;

    var scales = array<f32, 4>(1.0, 1.8, 2.6, 4.8);
    let rain_scale = max(u.scale, 0.1);
    for (var i = 0; i < 4; i = i + 1) {
        let s = scales[i];
        let r = rain_dist(wpos * s / rain_scale + 500.0 * f32(i), s, intensity);
        if r < 0.0 {
            let v = (1.0 - exp(r * 5.0)) / s * 2.0;
            wpos.x += v * 10.0 * rain_scale;
            wpos.y -= v * 2.0 * rain_scale;
            add += vec3<f32>(0.1, 0.15, 0.2) * v;
            rain_sum += (1.0 - rain_sum) * 0.75;
        }
    }

    let sample_uv = clamp(in.uv + (wpos - orig_wpos) * zoom / u.screen_res, vec2<f32>(0.0), vec2<f32>(1.0));
    let sampled = textureSample(scene_tex, scene_sampler, sample_uv);
    var color = sampled.rgb + add;
    color = mix(color, u.rain_color.rgb, 0.1 * rain_sum);
    return vec4<f32>(color, sampled.a);
}
