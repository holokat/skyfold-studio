// Drifting motes (visuals/motes.rs): a field of glowing specks wrapped in a box that travels with
// the camera. Each mote (position in the unit cube, vertex colour = seed, phase, size, 1, uv =
// corner) wanders on slow loops and rises, twinkles on its own rhythm, fades towards the box's
// edges so the wrap never shows, and is drawn additively as a soft point of light.

import bevy_pbr::render::{
    mesh_view_bindings::{view, globals},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
};

struct MoteParams {
    color: vec4<f32>, // rgb colour (display units), a: amount
    shape: vec4<f32>, // x: box size, y: mote size, z: rise speed, w: twinkle rate
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> motes: MoteParams;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    var c = vec4<f32>(0.5, 0.5, 1.0, 1.0);
@if(VERTEX_COLORS) {
    c = v.color;
}
    var corner = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    corner = v.uv;
}
    let t = globals.time;
    let b = motes.shape.x;
    let cam = view.world_position;
    // slow loops and a gentle rise, then wrapped into the box around the camera
    let s = c.x * 6.2832;
    let wander = vec3<f32>(sin(t * 0.21 + s * 3.0) + 0.5 * sin(t * 0.53 + s * 7.0), sin(t * 0.17 + s * 5.0) * 0.6, cos(t * 0.19 + s * 2.0) + 0.5 * cos(t * 0.47 + s * 11.0)) * 1.6;
    let local = v.position * b + wander + vec3<f32>(0.0, t * motes.shape.z, 0.0);
    let rel = (fract((local - cam) / b) - 0.5) * b;
    let centre = cam + rel;
    // fade at the box's edges and very close to the eye
    let r = length(rel) / (b * 0.5);
    let fade = (1.0 - smoothstep(0.6, 1.0, r)) * smoothstep(0.03, 0.12, r);
    let twinkle = 0.35 + 0.65 * pow(0.5 + 0.5 * sin(t * (0.8 + 1.7 * c.y) * motes.shape.w + s * 13.0), 3.0);
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let size = motes.shape.y * c.z;
    let wp = centre + (right * corner.x + up * corner.y) * size;
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_UVS_A)
    out.uv = corner;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(c.x, c.y, fade * twinkle, 1.0);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var vc = vec4<f32>(0.5, 0.5, 1.0, 1.0);
@if(VERTEX_COLORS) {
    vc = in.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    let r2 = dot(uv, uv);
    // a bright core in a soft halo
    let glow = exp(-r2 * 9.0) + 0.25 * exp(-r2 * 2.5);
    let a = glow * vc.z * motes.color.a * (1.0 - smoothstep(0.8, 1.0, r2));
    var out: FragmentOutput;
    out.color = vec4<f32>(motes.color.rgb * a, a);
    return out;
}
