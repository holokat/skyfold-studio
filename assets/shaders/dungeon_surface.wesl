// A dungeon's walls, floors, pillars and vaults (`crate::dungeons::render`): the standard
// material (with parallax from the style's depth map), weathered by the map's weathering:
// - grime gathers at the foot of the walls, in the corners and in streaks down them;
// - moss grows in patches where it's damp and sheltered;
// - near water the stone darkens and shines;
// - puddles lie on the floors.
// Per vertex: corner shading (colour rgb, already in the base colour), how mossy the spot may get
// (colour alpha), the floor height under it (uv_b.x) and how wet it is (uv_b.y).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};
import super::noise::fbm2;

struct SurfaceParams {
    // x moss, y grime, z wet, w puddles (0 none … ~1.5 heavy)
    amounts: vec4<f32>,
    // the moss's tint (linear), w: 1 when the moss texture has its own coverage (alpha)
    moss: vec4<f32>,
    // what grime multiplies the colour by (linear), w: metres per moss repeat
    grime: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> surf: SurfaceParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var moss_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var moss_samp: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let wp = in.world_position.xyz;
    var floor_y = wp.y - 10.0;
    var wet_v = 0.0;
    var mossy = 0.4;
    var ao = 1.0;
@if(VERTEX_UVS_B) {
    floor_y = in.uv_b.x;
    wet_v = in.uv_b.y;
}
@if(VERTEX_COLORS) {
    mossy = in.color.a;
    ao = in.color.r;
}
    let n = normalize(in.world_normal);
    let h = max(wp.y - floor_y, 0.0);
    var base = pbr_input.material.base_color.rgb;
    var rough = pbr_input.material.perceptual_roughness;
    let wall = abs(n.y) < 0.4;

    // grime: up the foot of the walls, into the corners, in streaks down from above (a floor is
    // all "at the foot": it gets its grime from the corners, by the walls)
    let foot = (1.0 - smoothstep(0.0, 1.4, h)) * select(0.0, 1.0, wall);
    let streak = smoothstep(0.58, 0.86, fbm2(vec2<f32>((wp.x + wp.z) * 1.3, wp.y * 0.07), 3u, 5u)) * select(0.0, 1.0, wall);
    let g = clamp((foot * 0.75 + (1.0 - ao) * 0.9 + streak * 0.35) * surf.amounts.y, 0.0, 1.0);
    base = mix(base, base * surf.grime.rgb, g);

    // moss: patches where it's damp and sheltered (the foot of walls, corners, by water)
    let up = abs(n.y) > 0.6;
    let muv = select(vec2<f32>(wp.x + wp.z, -wp.y), wp.xz, up) / max(surf.grime.w, 0.1);
    let mt = textureSample(moss_tex, moss_samp, muv);
    let patch_n = fbm2(wp.xz * 0.21 + vec2<f32>(wp.y * 0.13, -wp.y * 0.09), 4u, 9u);
    let want = (mossy * 0.9 + foot * 0.35 + wet_v * 0.4) * surf.amounts.x;
    var m = smoothstep(0.64 - want * 0.35, 0.8 - want * 0.35, patch_n) * clamp(want * 1.5, 0.0, 1.0);
    m = m * select(1.0, mt.a, surf.moss.w > 0.5);
    m = clamp(m, 0.0, 1.0);
    base = mix(base, mt.rgb * surf.moss.rgb, m);
    rough = mix(rough, 0.92, m);

    // wet: darker and glossier near water, streaks down damp walls; puddles on the floors
    let w = clamp((wet_v + streak * 0.25) * surf.amounts.z, 0.0, 1.0);
    let puddle_n = fbm2(wp.xz * 0.15, 3u, 13u);
    let puddle = smoothstep(0.6, 0.67, puddle_n + w * 0.1) * select(0.0, 1.0, n.y > 0.85) * clamp(surf.amounts.w, 0.0, 1.0);
    let wet = max(w * 0.7, puddle);
    base = base * mix(1.0, 0.5, wet);
    rough = mix(rough, 0.05, wet);

    pbr_input.material.base_color = vec4<f32>(base, pbr_input.material.base_color.a);
    pbr_input.material.perceptual_roughness = rough;
    if puddle > 0.0 {
        // standing water is flat
        pbr_input.N = normalize(mix(pbr_input.N, n, puddle));
    }
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
