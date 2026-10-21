// Sun shafts (sun_shafts.rs): `march` runs at half resolution and walks from each pixel towards
// the sun's place on screen, gathering the bright sky round the sun wherever nothing stands in
// front of it (reverse-Z depth 0 = sky); `composite` adds the result to the HDR view.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct SunShafts {
    // xy: the sun on screen (uv), z: strength, w: dither frame (0: still)
    sun: vec4<f32>,
    // x: share of the way to the sun marched, y: sky brightness that starts to shine,
    // z: glow radius round the sun (uv), w: decay per step
    params: vec4<f32>,
    // rgb: tint, w: view aspect
    tint: vec4<f32>,
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var<uniform> shafts: SunShafts;

const STEPS: i32 = 40;
const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// The light one sample adds: open sky only, the part of its brightness above the threshold
// (the sun, its halo and sunlit cloud edges, not a plain blue sky), gathered round the sun.
fn light_at(uv: vec2<f32>, dims: vec2<f32>) -> vec3<f32> {
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return vec3<f32>(0.0);
    }
    let p = min(vec2<i32>(uv * dims), vec2<i32>(dims) - vec2<i32>(1));
    if textureLoad(depth_tex, p, 0) > 0.0 {
        return vec3<f32>(0.0);
    }
    let c = textureSampleLevel(src_tex, src_samp, uv, 0.0).rgb;
    let lum = dot(c, LUMA);
    let bright = max(lum - shafts.params.y, 0.0) / max(lum, 1.0e-4);
    let d = (uv - shafts.sun.xy) * vec2<f32>(shafts.tint.w, 1.0);
    let glow = exp(-dot(d, d) / (shafts.params.z * shafts.params.z));
    return min(c * bright, vec3<f32>(6.0)) * glow;
}

@fragment
fn march(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(depth_tex));
    let step = (shafts.sun.xy - in.uv) * shafts.params.x / f32(STEPS);
    // interleaved gradient noise starts each pixel's march at a different point, trading banding
    // for fine noise that the upscale (and temporal AA, which gets a new pattern each frame) smooths
    let px = in.position.xy + vec2<f32>(5.588238) * shafts.sun.w;
    let ign = fract(52.9829189 * fract(dot(px, vec2<f32>(0.06711056, 0.00583715))));
    var p = in.uv + step * ign;
    var sum = vec3<f32>(0.0);
    var w = 1.0;
    for (var i = 0; i < STEPS; i++) {
        sum += light_at(p, dims) * w;
        w *= shafts.params.w;
        p += step;
    }
    return vec4<f32>(sum / f32(STEPS), 1.0);
}

// (its bind group has the rays' texture in `src_tex` and no depth)
@fragment
fn composite(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let r = textureSampleLevel(src_tex, src_samp, in.uv, 0.0).rgb;
    return vec4<f32>(r * shafts.tint.rgb * shafts.sun.z, 0.0);
}
