// Vegetation wind (trees, bushes, plants). Vertex colour alpha is the sway weight: 0 at the
// trunk base, rising through the branch hierarchy to 1 at the twig tips (ez-tree's hierarchy
// weights; leaves carry the weight of the twig they grow on).
// Motion, scaled by the wind speed (m/s) and a travelling gust field shared with the grass:
//  1. the whole plant bends downwind, growing with height² and rocking with the gusts;
//  2. branches sway on top of it, weighted by the hierarchy. The sway phase only changes over
//     metres, so a branch and every leaf on it move together as one piece — no per-leaf motion.
// Costs a couple of noise taps and a handful of sines per vertex (foliage_sway.wesl, shared with
// the prepass). The fragment is standard PBR.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
};
import super::foliage_sway::foliage_sway;

struct FoliageParams {
    // xy: wind direction (world xz), z: wind speed (m/s), w: gustiness 0..1
    wind: vec4<f32>,
    // xy: gust field offset (m)
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
