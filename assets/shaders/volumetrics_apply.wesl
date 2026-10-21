// Volumetric light, step 3 of 3 (`volumetrics.rs`): fog the view. Each pixel looks up the
// integrated froxel volume at its own depth (trilinear, so the coarse grid never steps across a
// silhouette) and is blended as `light + colour × transmittance`.
//
// `apply` runs on the HDR view after the opaque, transmissive and cloud-tracing passes. The two
// `cloud_*` entries fog the resolved clouds the same way before the composite dome lays them
// over the (already fogged) sky: premultiplied, a cloud of opacity a becomes c·T + S·a.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import super::volumetrics_types::{Volumetrics, depth_slice};

@group(0) @binding(0) var<uniform> vol: Volumetrics;
@group(0) @binding(1) var fog_tex: texture_3d<f32>;
@group(0) @binding(2) var fog_smp: sampler;
@group(0) @binding(3) var depth_tex: texture_depth_2d;

// The light added and the transmittance between the eye and view depth d along this pixel.
fn fog_at(uv: vec2<f32>, d: f32) -> vec4<f32> {
    let w = depth_slice(vol, d);
    if w <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let gz = vol.grid.z;
    let wc = min(w, 1.0);
    // texel k holds the sum up to the far edge of cell k, (k + 1) / gz
    let tc = max(wc - 0.5 / gz, 0.5 / gz);
    let s = textureSampleLevel(fog_tex, fog_smp, vec3<f32>(uv, tc), 0.0);
    // in the first cell it fades in from nothing at the near plane
    return mix(vec4<f32>(0.0, 0.0, 0.0, 1.0), s, clamp(wc * gz, 0.0, 1.0));
}

// Blended One / SrcAlpha: dst × transmittance + light.
@fragment
fn apply(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(depth_tex));
    let px = clamp(vec2<i32>(in.position.xy), vec2<i32>(0), dims - vec2<i32>(1));
    let z = textureLoad(depth_tex, px, 0);
    // reverse-Z with an infinite far plane: view depth = near / z (the sky, z = 0, is beyond the grid)
    let d = select(1.0e9, vol.mist.w / z, z > 0.0);
    return fog_at(in.uv, d);
}

// Blended Zero / Src (colour only): the cloud image × the transmittance to the far end of the grid.
@fragment
fn cloud_mul(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let t = fog_at(in.uv, 1.0e9).a;
    return vec4<f32>(t, t, t, 1.0);
}

// Blended DstAlpha / One (colour only): + the grid's light × the cloud's opacity.
@fragment
fn cloud_add(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(fog_at(in.uv, 1.0e9).rgb, 0.0);
}
