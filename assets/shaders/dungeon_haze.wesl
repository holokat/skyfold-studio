// A dungeon's haze (`crate::dungeons::fx`): light hanging in dusty air — a soft glow round a fire
// or a beam falling from a light shaft or a lamp — drawn additively, with dust drifting through
// it. In display units, like the flames. It fades softly where it meets the walls and the floor
// (the depth prepass) and thins right in front of the eye.
//
// look.x 0, a halo: a sheet facing the eye, uv 0..1 across.
// look.x 1, a beam: an open cone, uv.x round it, uv.y from the light (0) to where it lands (1).

import bevy_pbr::render::{
    mesh_view_bindings::{view, globals},
    forward_io::{VertexOutput, FragmentOutput},
    view_transformations::depth_ndc_to_view_z,
};
import super::noise::fbm2;

struct HazeParams {
    // colour (linear rgb), w: strength
    color: vec4<f32>,
    // x: 0 halo / 1 beam, y: dust (0..1), z: seed
    look: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> haze: HazeParams;

// How far (m, along the view axis) the opaque scene lies behind this fragment.
fn behind(frag: vec4<f32>) -> f32 {
    var gap = 1e4;
@if(DEPTH_PREPASS && !MULTISAMPLED) {
    let d = textureLoad(bevy_pbr::render::mesh_view_bindings::depth_prepass_texture, vec2<i32>(frag.xy), 0);
    if d > 0.0 {
        gap = depth_ndc_to_view_z(frag.z) - depth_ndc_to_view_z(d);
    }
}
    return gap;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var uv = vec2<f32>(0.5);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    let t = globals.time;
    let wp = in.world_position.xyz;
    let seed = u32(haze.look.z);
    // dust drifting slowly through the light
    let dust = fbm2(vec2<f32>(wp.x * 0.8 + wp.y * 0.5 + t * 0.06, wp.z * 0.8 - wp.y * 0.35 - t * 0.045), 3u, seed);
    let grain = mix(1.0, 0.45 + 1.1 * dust, clamp(haze.look.y, 0.0, 1.0));
    var a = 0.0;
    if haze.look.x < 0.5 {
        let r = length(uv - vec2<f32>(0.5)) * 2.0;
        let g = clamp(1.0 - r, 0.0, 1.0);
        a = g * g * g;
    } else {
        let v = normalize(view.world_position - wp);
        let facing = abs(dot(normalize(in.world_normal), v));
        let along = clamp(uv.y, 0.0, 1.0);
        a = pow(facing, 2.2) * smoothstep(0.0, 0.1, along) * pow(1.0 - along * 0.85, 1.6);
    }
    // soft where it meets the rock, thin in front of the eye
    let soft = smoothstep(0.0, 0.9, behind(in.position));
    let near = smoothstep(0.5, 3.0, length(view.world_position - wp));
    var out: FragmentOutput;
    out.color = vec4<f32>(haze.color.rgb * haze.color.w * a * grain * soft * near, 0.0);
    return out;
}
