// Prepass vertex path for distant tree billboards. It mirrors billboard.wesl's range fade and
// collapsed vertices, with a static previous world position for temporal motion vectors.

import bevy_pbr::{
    prepass::io::{Vertex, VertexOutput, decompress_vertex},
    render::{
        mesh_functions,
        mesh_view_bindings::view,
        view_transformations::position_world_to_clip,
    },
};

// x..y: fade in after the trees' last mesh LOD, z..w: fade out at the draw distance (m).
@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> ranges: vec4<f32>;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
    var root = wp.xyz;
@if(VERTEX_UVS_B) {
    root = vec3<f32>(v.uv_b.x, root.y, v.uv_b.y);
}
@if(VERTEX_COLORS) {
    root.y = v.color.r;
}
    let d = length(view.lod_view_world_position - root);
    // Bevy's encoding: −16 (hidden) … 0 fading in, 0 visible, 0 … 16 (hidden) fading out.
    var level = 0;
    if d < ranges.y {
        level = -16 + clamp(i32(round((d - ranges.x) / max(ranges.y - ranges.x, 1.0) * 16.0)), 0, 16);
    } else if d > ranges.z {
        level = clamp(i32(round((d - ranges.z) / max(ranges.w - ranges.z, 1.0) * 16.0)), 0, 16);
    }
    let hidden = d < ranges.x || d > ranges.w;

    out.world_position = wp;
    out.position = select(position_world_to_clip(wp.xyz), vec4<f32>(0.0, 0.0, -1.0, 1.0), hidden);
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, v.tangent, vertex_in.instance_index);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(1.0);
@if(MOTION_VECTOR_PREPASS)
    // Billboards are static geometry; use last frame's world transform for temporal motion.
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_previous_world_from_local(vertex_in.instance_index),
        vec4<f32>(v.position, 1.0),
    );
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = level;
    return out;
}
