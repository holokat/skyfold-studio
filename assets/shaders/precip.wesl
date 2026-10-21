// Rain / snow particles. One static mesh of quads; the vertex shader animates every particle
// inside a box that follows the camera (wrapping), so there is no per-frame CPU work.

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
};
import super::volumetrics_fog::froxel_fog;

struct PrecipParams {
    // x: rain amount, y: snow amount, z: box half size, w: wind strength
    amount: vec4<f32>,
    // xy: wind direction, z: brightness, w: unused
    wind: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> precip: PrecipParams;
// the volumetric light (`volumetrics::FroxelFog`; drawn after the view was fogged, the streaks fog
// themselves): bound only in builds that set FROXEL_FOG, so an older build still runs this shader
@if(FROXEL_FOG) {
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var fog_tex: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var fog_smp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<uniform> fog_params: vec4<f32>;
}

fn hash(n: f32) -> f32 {
    return fract(sin(n) * 43758.5453);
}

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    // uv.x = particle id, uv.y = corner index (0..3)
@if(VERTEX_UVS_A) {
    let id = v.uv.x;
    let corner = u32(v.uv.y + 0.5);
    let snow = precip.amount.y > precip.amount.x;
    let density = max(precip.amount.x, precip.amount.y);
    let half = precip.amount.z;
    let t = globals.time;
    // hide particles beyond the current density
    let visible = select(0.0, 1.0, hash(id * 7.13) < density);
    let speed = select(9.0 + hash(id) * 4.0, 1.1 + hash(id * 3.1) * 0.6, snow);
    let box_size = half * 2.0;
    let cam = view.world_position;
    var p = vec3<f32>(hash(id * 1.7), hash(id * 2.3), hash(id * 3.9)) * box_size;
    p.y -= t * speed;
    let drift = precip.wind.xy * precip.amount.w * t * select(1.0, 0.6, snow);
    p.x += drift.x;
    p.z += drift.y;
    if snow {
        p.x += sin(t * 0.9 + id) * 0.6;
        p.z += cos(t * 0.7 + id * 1.3) * 0.6;
    }
    // wrap into the box around the camera
    p = p - box_size * floor((p - (cam - vec3<f32>(half))) / box_size);
    // camera-facing quad; rain is a thin streak stretched along the fall direction
    let to_cam = normalize(cam - p);
    let fall = normalize(vec3<f32>(precip.wind.x * precip.amount.w * 0.08, -1.0, precip.wind.y * precip.amount.w * 0.08));
    var right = normalize(cross(fall, to_cam));
    var up = fall;
    var w = select(0.012, 0.045, snow);
    var h = select(0.55, 0.045, snow);
    if snow {
        right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_cam));
        up = normalize(cross(to_cam, right));
    }
    let cx = select(-1.0, 1.0, corner == 1u || corner == 2u);
    let cy = select(-1.0, 1.0, corner >= 2u);
    let wp = p + right * cx * w * visible + up * cy * h * visible;
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.uv = vec2<f32>(cx, cy);
    // fade with distance from the camera so the box edges don't show
    let fade = 1.0 - smoothstep(half * 0.55, half, length(p - cam));
    out.world_normal = vec3<f32>(fade * visible, select(0.0, 1.0, snow), 0.0);
}
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let fade = in.world_normal.x;
    let snow = in.world_normal.y > 0.5;
    var a = 0.0;
@if(VERTEX_UVS_A) {
    let r = length(in.uv);
    a = select(1.0 - abs(in.uv.x), 1.0 - smoothstep(0.5, 1.0, r), snow);
}
    let col = select(vec3<f32>(0.62, 0.68, 0.76), vec3<f32>(0.95, 0.97, 1.0), snow) * precip.wind.z;
    let alpha = clamp(a * fade * select(0.28, 0.85, snow), 0.0, 1.0);
    if alpha < 0.01 {
        discard;
    }
    var rgb = col * alpha;
@if(FROXEL_FOG) {
    let f = froxel_fog(fog_tex, fog_smp, fog_params, in.position, in.world_position.xyz);
    rgb = rgb * f.a + f.rgb * alpha;
}
    return vec4<f32>(rgb, alpha);
}
