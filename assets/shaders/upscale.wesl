// Upscales the 3D view, rendered at a reduced resolution (`render_scale.rs`), to the window under
// the panels: Catmull-Rom bicubic from 9 bilinear taps (sharper than bilinear, no blocky look),
// clamped to the 2×2 texels the pixel lies between so edges don't ring. It filters
// display-encoded values (the source is bound through a plain, not sRGB, view), like FSR's
// upscaler: in linear light bright edges would grow dark halos. The output is linear.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

fn tap(p: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src, samp, p, 0.0).rgb;
}

fn texel(i: vec2<i32>, hi: vec2<i32>) -> vec3<f32> {
    return textureLoad(src, clamp(i, vec2<i32>(0), hi), 0).rgb;
}

// the sRGB transfer curve, display → linear
fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let p = in.uv * size;
    // the texel centre up-left of the pixel and the weights of the 4×4 texels round it
    let t1 = floor(p - 0.5) + 0.5;
    let f = p - t1;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    // the middle two texels in one bilinear tap
    let w12 = w1 + w2;
    let a = (t1 - 1.0) / size;
    let b = (t1 + w2 / w12) / size;
    let c = (t1 + 2.0) / size;
    var col = (tap(vec2<f32>(a.x, a.y)) * w0.x + tap(vec2<f32>(b.x, a.y)) * w12.x + tap(vec2<f32>(c.x, a.y)) * w3.x) * w0.y;
    col += (tap(vec2<f32>(a.x, b.y)) * w0.x + tap(vec2<f32>(b.x, b.y)) * w12.x + tap(vec2<f32>(c.x, b.y)) * w3.x) * w12.y;
    col += (tap(vec2<f32>(a.x, c.y)) * w0.x + tap(vec2<f32>(b.x, c.y)) * w12.x + tap(vec2<f32>(c.x, c.y)) * w3.x) * w3.y;
    // no overshoot past the nearest texels (ringing round high-contrast edges)
    let i = vec2<i32>(floor(t1));
    let hi = vec2<i32>(size) - 1;
    let n0 = texel(i, hi);
    let n1 = texel(i + vec2<i32>(1, 0), hi);
    let n2 = texel(i + vec2<i32>(0, 1), hi);
    let n3 = texel(i + vec2<i32>(1, 1), hi);
    col = clamp(col, min(min(n0, n1), min(n2, n3)), max(max(n0, n1), max(n2, n3)));
    return vec4<f32>(to_linear(col), 1.0);
}
