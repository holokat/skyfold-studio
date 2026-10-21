// Vertex shader of the fish material (fish/model.rs): the swim wave of fish_swim.wesl, then the
// standard PBR fragment.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
};
import super::fish_swim::{fish_swim, FishClock};

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> fish_clock: FishClock;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> swim: vec4<f32>;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let tag = mesh_functions::get_tag(vertex_in.instance_index);
    var n = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_NORMALS)
    n = v.normal;
    let s = fish_swim(v.position, n, swim, tag, fish_clock.t.x, globals.time);
    let wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(s.position, 1.0));
    out.world_position = wp;
    out.position = position_world_to_clip(wp.xyz);
@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(s.normal, vertex_in.instance_index);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);
    return out;
}
