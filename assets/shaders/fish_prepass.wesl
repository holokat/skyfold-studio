// Depth prepass of the fish material: the same swim wave as the main pass (fish_swim.wesl), so
// the depth buffer matches the fish as drawn.

import bevy_pbr::{
    prepass::io::{Vertex, VertexOutput, decompress_vertex},
    render::{mesh_functions, view_transformations::position_world_to_clip},
};
import bevy_render::globals::Globals;
import super::fish_swim::{fish_swim, FishClock};

@group(0) @binding(1) var<uniform> globals: Globals;
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> fish_clock: FishClock;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var<uniform> swim: vec4<f32>;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let tag = mesh_functions::get_tag(vertex_in.instance_index);
    var n = vec3<f32>(0.0, 1.0, 0.0);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    n = v.normal;
    let s = fish_swim(v.position, n, swim, tag, fish_clock.t.x, globals.time);
    let wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(s.position, 1.0));
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
    out.world_normal = mesh_functions::mesh_normal_local_to_world(s.normal, vertex_in.instance_index);
// last frame's transform with this frame's swim pose (the school's travel, for temporal
// anti-aliasing; the tail beat is small)
@if(MOTION_VECTOR_PREPASS)
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(mesh_functions::get_previous_world_from_local(vertex_in.instance_index), vec4<f32>(s.position, 1.0));
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex_in.instance_index, world_from_local[3]);
    return out;
}
