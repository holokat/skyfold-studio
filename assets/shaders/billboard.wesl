// Distant tree billboards, merged per 128 m chunk (streaming.rs). Each vertex carries its tree's
// root (UV1 = x, z; colour.r = y). Trees still drawn by their own mesh LODs are collapsed; in the
// hand-over band and at the draw distance each tree cross-fades with the same dither levels Bevy
// uses between LODs, so the seam with the last mesh LOD is invisible. Standard PBR fragment.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::view,
};

// x..y: fade in after the trees' last mesh LOD, z..w: fade out at the draw distance (m)
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
    // Bevy's encoding: −16 (hidden) … 0 fading in, 0 visible, 0 … 16 (hidden) fading out
    var level = 0;
    if d < ranges.y {
        level = -16 + clamp(i32(round((d - ranges.x) / max(ranges.y - ranges.x, 1.0) * 16.0)), 0, 16);
    } else if d > ranges.z {
        level = clamp(i32(round((d - ranges.z) / max(ranges.w - ranges.z, 1.0) * 16.0)), 0, 16);
    }
    let hidden = d < ranges.x || d > ranges.w;

    out.world_position = wp;
    // collapsed trees: every vertex of their quads lands on one point outside the clip volume
    out.position = select(position_world_to_clip(wp.xyz), vec4<f32>(0.0, 0.0, -1.0, 1.0), hidden);
@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(1.0);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = level;
    return out;
}
