// Rock at any size (visuals::rock_material): standard PBR, two ways to keep the photoscanned
// rock's grain at its real size however far a placed rock is scaled up.
//  - Tiled shapes (floating rocks, monoliths): their uv are metres of a tiling scan at size 1,
//    so they're multiplied by the object's size — a 60 m island shows the same 2.7 m tiles as a
//    3 m boulder, and the tiles move with the rock as it drifts.
//  - Scanned formations scaled up (rock spires 100+ m tall): their own atlas stays, stretched,
//    and a tiling rock scan laid over it in world space (triplanar) brings back the fine grain:
//    its brightness relative to its own mean shades the colour, its normals roughen the surface.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
};
@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;

struct RockParams {
    // x: 1 = uv scale with the object's size, y: detail strength (0 none), z: detail tile (m)
    p: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> rock: RockParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var detail_albedo: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var detail_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var detail_normal: texture_2d<f32>;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
    out.world_position = wp;
    out.position = position_world_to_clip(wp.xyz);
@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, v.tangent, vertex_in.instance_index);
@if(VERTEX_UVS_A) {
    // the object's size: the mean length of its axes
    let size = (length(world_from_local[0].xyz) + length(world_from_local[1].xyz)
        + length(world_from_local[2].xyz)) / 3.0;
    out.uv = v.uv * select(1.0, size, rock.p.x > 0.5);
}
@if(VERTEX_COLORS)
    out.color = v.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_in.instance_index, world_from_local[3]);
    return out;
}

// tangent-space normal (RG = XY) of the detail scan at `uv`
fn detail_n(uv: vec2<f32>) -> vec2<f32> {
    return textureSample(detail_normal, detail_sampler, uv).xy * 2.0 - 1.0;
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
    let k = rock.p.y;
    if k > 0.0 {
        let q = in.world_position.xyz / rock.p.z;
        let n = normalize(in.world_normal);
        var w = pow(abs(n), vec3<f32>(4.0));
        w = w / (w.x + w.y + w.z);
        let ax = textureSample(detail_albedo, detail_sampler, q.zy).rgb;
        let ay = textureSample(detail_albedo, detail_sampler, q.xz).rgb;
        let az = textureSample(detail_albedo, detail_sampler, q.xy).rgb;
        let a = ax * w.x + ay * w.y + az * w.z;
        // (the 1×1 mip is the scan's mean colour)
        let mean = textureSampleLevel(detail_albedo, detail_sampler, vec2<f32>(0.5), 16.0).rgb;
        let lum = vec3<f32>(0.2126, 0.7152, 0.0722);
        let shade = clamp(dot(a, lum) / max(dot(mean, lum), 0.02), 0.35, 2.0);
        pbr_input.material.base_color = vec4<f32>(
            pbr_input.material.base_color.rgb * mix(1.0, shade, k),
            pbr_input.material.base_color.a,
        );
        // each projection's normal tilts the surface along that projection's own axes
        let nx = detail_n(q.zy);
        let ny = detail_n(q.xz);
        let nz = detail_n(q.xy);
        let tilt = vec3<f32>(0.0, nx.y, nx.x) * w.x + vec3<f32>(ny.x, 0.0, ny.y) * w.y
            + vec3<f32>(nz.x, nz.y, 0.0) * w.z;
        pbr_input.N = normalize(pbr_input.N + tilt * k * 0.8);
    }
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
