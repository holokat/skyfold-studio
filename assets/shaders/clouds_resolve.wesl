// Temporal reconstruction of the clouds at history resolution. Each frame traced one pixel of
// every 2x2 block (rotating), so a pixel gets a fresh sample every fourth frame; in between it
// keeps its reprojected history (moved with the cloud depth and the wind drift), clipped to the
// range of the current samples around it (no ghosts where clouds appear, move or where the
// terrain uncovers sky). Without a history the traced samples are filtered spatially.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import super::clouds_types::Clouds;

@group(0) @binding(0) var<uniform> cl: Clouds;
@group(0) @binding(1) var trace_tex: texture_2d<f32>;
@group(0) @binding(2) var depth_tex: texture_2d<f32>;
@group(0) @binding(3) var history_tex: texture_2d<f32>;
@group(0) @binding(4) var smp: sampler;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.position.xy);
    let tsize = vec2<i32>(cl.size.xy);
    let ti = min(p / 2, tsize - 1);
    let traced = all(p == ti * 2 + vec2<i32>(cl.grid.zw));
    let uv = (vec2<f32>(p) + 0.5) / cl.grid.xy;
    // the traced samples are offset by this frame's position in the block
    let tuv = (uv * cl.grid.xy - cl.grid.zw + 0.5) / (2.0 * cl.size.xy);
    let spatial = max(textureSampleLevel(trace_tex, smp, tuv, 0.0), vec4<f32>(0.0));
    if cl.motion.z < 0.5 {
        return spatial;
    }

    // range of the current samples around this pixel
    var m1 = vec4<f32>(0.0);
    var m2 = vec4<f32>(0.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let s = textureLoad(trace_tex, clamp(ti + vec2<i32>(x, y), vec2<i32>(0), tsize - 1), 0);
            m1 += s;
            m2 += s * s;
        }
    }
    m1 /= 9.0;
    let sigma = sqrt(max(m2 / 9.0 - m1 * m1, vec4<f32>(0.0)));
    // a generous box: the samples are jittered, a tight clip would keep their noise
    let lo = m1 - sigma * 2.25;
    let hi = m1 + sigma * 2.25;

    // where this pixel's cloud was last frame
    let t = textureLoad(depth_tex, ti, 0).r;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let wn = cl.world_from_clip * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(wn.xyz / wn.w - cl.camera.xyz);
    let wp = cl.camera.xyz + dir * t + vec3<f32>(cl.motion.x, 0.0, cl.motion.y);
    let pc = cl.prev_clip_from_world * vec4<f32>(wp, 1.0);
    let puv = vec2<f32>(pc.x / pc.w * 0.5 + 0.5, 0.5 - pc.y / pc.w * 0.5);
    if pc.w <= 0.0 || any(puv < vec2<f32>(0.0)) || any(puv > vec2<f32>(1.0)) {
        return spatial;
    }
    var hist = textureSampleLevel(history_tex, smp, puv, 0.0);
    if any(hist != hist) {
        return spatial;
    }
    hist = clamp(hist, lo, hi);
    if traced {
        let cur = textureLoad(trace_tex, ti, 0);
        hist = mix(hist, cur, cl.quality.z);
    }
    return max(hist, vec4<f32>(0.0));
}
