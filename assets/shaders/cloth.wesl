// Wind-blown cloth on library models (banners, flags, ragged hems): any glTF material named
// `*_sway` renders with this vertex shader (model_render::cloth). Vertex colour alpha is the sway
// weight (0 pinned .. 1 free); the displacement is cloth_sway.wesl, shared with the prepass. The
// fragment is standard PBR.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
};
import super::cloth_sway::cloth_sway;

struct WindParams {
    // xy: wind direction (world xz), z: wind speed (m/s), w: gustiness 0..1
    wind: vec4<f32>,
    // xy: gust field offset (m)
    gust: vec4<f32>,
}

// the foliage wind buffer (objects::FOLIAGE_WIND), shared with every plant and tree
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
