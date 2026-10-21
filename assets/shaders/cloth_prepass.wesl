// Depth / shadow prepass of the cloth material: the same wind displacement as the main pass
// (cloth_sway.wesl), so a banner's shadow waves with it.

import bevy_pbr::{
    prepass::io::{Vertex, VertexOutput, decompress_vertex},
    render::{mesh_functions, view_transformations::position_world_to_clip},
};
import bevy_render::globals::Globals;
import super::cloth_sway::cloth_sway;

struct WindParams {
    wind: vec4<f32>,
    gust: vec4<f32>,
}

@group(0) @binding(1) var<uniform> globals: Globals;
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> air: WindParams;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
@if(VERTEX_COLORS) {
    wp = vec4<f32>(cloth_sway(wp.xyz, world_from_local[3].xyz, v.color.a, globals.time, air.wind, air.gust.xy), 1.0);
    out.color = vec4<f32>(v.color.rgb, 1.0);
}
    out.world_position = wp;
    out.position = position_world_to_clip(wp.xyz);
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, v.tangent, vertex_in.instance_index);
@if(MOTION_VECTOR_PREPASS) {
    // where the vertex was last frame (its previous transform, the wind a frame ago): motion
    // vectors for temporal anti-aliasing
    let prev_from_local = mesh_functions::get_previous_world_from_local(vertex_in.instance_index);
    var pp = mesh_functions::mesh_position_local_to_world(prev_from_local, vec4<f32>(v.position, 1.0));
@if(VERTEX_COLORS)
    pp = vec4<f32>(cloth_sway(pp.xyz, prev_from_local[3].xyz, v.color.a, globals.time - globals.delta_time, air.wind, air.gust.xy), 1.0);
    out.previous_world_position = pp;
}
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);
    return out;
}
