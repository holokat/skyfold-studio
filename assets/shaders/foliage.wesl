// Vegetation wind (trees, bushes, plants). Vertex colour alpha is the sway weight: 0 at the
// trunk base, rising through the branch hierarchy to 1 at the twig tips (ez-tree's hierarchy
// weights; leaves carry the weight of the twig they grow on).
// Motion, scaled by the wind speed (m/s) and a travelling gust field shared with the grass:
//  1. the whole plant bends downwind, growing with height² and rocking with the gusts;
//  2. branches sway on top of it, weighted by the hierarchy. The sway phase only changes over
//     metres, so a branch and every leaf on it move together as one piece — no per-leaf motion.
// Costs a couple of noise taps and a handful of sines per vertex (foliage_sway.wesl, shared with
// the prepass). The fragment is standard PBR, with the sky light the crowns around hold back
// (canopy_shade.wesl): lower and inner leaves, trunks and plants under the trees darken.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
};
@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;
import super::foliage_sway::foliage_sway;
import super::canopy_shade::canopy_sky;
import super::alien_flora::{alien_tint, alien_surface};

struct FoliageParams {
    // xy: wind direction (world xz), z: wind speed (m/s), w: gustiness 0..1
    wind: vec4<f32>,
    // xy: gust field offset (m), zw: alien flora colour (`alien_flora.wesl`, 0 = off)
    gust: vec4<f32>,
}

// one storage buffer shared by every foliage material (see objects::FOLIAGE_WIND)
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> foliage: FoliageParams;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
@if(VERTEX_COLORS) {
    wp = vec4<f32>(foliage_sway(wp.xyz, world_from_local[3].xyz, v.color.a, globals.time, foliage.wind, foliage.gust.xy), 1.0);
    out.color = vec4<f32>(v.color.rgb, 1.0);
}

    out.world_position = wp;
    out.position = position_world_to_clip(wp.xyz);
@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, v.tangent, vertex_in.instance_index);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_in.instance_index, world_from_local[3]);
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
@if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    );
    let own = pbr_input.material.base_color.rgb;
    pbr_input.material.base_color = vec4<f32>(alien_tint(own, foliage.gust.zw), pbr_input.material.base_color.a);
    // the flora mode's surface: a soft glow, a glassy frost, dull char or stone
    let alien = alien_surface(own, foliage.gust.zw);
    if alien.w >= 0.0 {
        pbr_input.material.perceptual_roughness = mix(pbr_input.material.perceptual_roughness, alien.w, 0.8);
    }
    if alien.r + alien.g + alien.b > 0.0 {
        pbr_input.material.emissive = vec4<f32>(pbr_input.material.emissive.rgb + alien.rgb, 0.0);
    }
    let sky = canopy_sky(in.world_position.xyz);
    pbr_input.diffuse_occlusion *= sky;
    pbr_input.specular_occlusion *= sky;
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
