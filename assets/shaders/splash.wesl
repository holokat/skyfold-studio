// Rain splashes (rain/splash.rs). One static mesh of quads; the vertex shader animates every
// splash: each cycle it lands at a new random spot in a box wrapped round the camera, standing on
// the ground height window (terrain raised to the tops of the objects there), as a small upright
// quad turned to the camera. The fragment draws the crown: a few droplets flung up and out on
// arcs and the thin sheet at its foot, fading as they fall back.

import bevy_pbr::render::{
    forward_io::{Vertex, VertexOutput, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
};
import super::volumetrics_fog::froxel_fog;

struct SplashParams {
    // x: rain amount, y: brightness, z: box half size (m), w: lightning flash
    amount: vec4<f32>,
    // xy: world xz of the height window's corner, z: metres per texel, w: texels per side
    window: vec4<f32>,
    // `volumetrics::FroxelFog::params` (w 0: no volumetric light)
    fog: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> splash: SplashParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var height_tex: texture_2d<f32>;
// the volumetric light (drawn after the view was fogged, each splash fogs itself)
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var fog_tex: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var fog_smp: sampler;

fn hash(n: f32) -> f32 {
    return fract(sin(n) * 43758.5453);
}

// (a splash's life: this share of its cycle; the rest of the cycle it is hidden)
const LIFE: f32 = 0.38;
// size of the quad: half width and height (m)
const HALF_W: f32 = 0.12;
const TALL: f32 = 0.15;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
@if(VERTEX_UVS_A) {
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let id = v.uv.x;
    let corner = u32(v.uv.y + 0.5);
    let t = globals.time;
    let period = 0.55 + hash(id * 1.31) * 0.45;
    let ph = t / period + hash(id * 2.77);
    let cyc = floor(ph);
    let s = fract(ph) / LIFE;
    let seed = id * 13.17 + cyc * 7.31;
    // as many as the rain is heavy; each lives the first part of its cycle
    let alive = s < 1.0 && hash(seed * 0.93) < splash.amount.x;
    let half = splash.amount.z;
    let box_size = half * 2.0;
    let cam = view.world_position;
    var xz = vec2<f32>(hash(seed * 1.7), hash(seed * 2.9)) * box_size;
    xz = xz - box_size * floor((xz - (cam.xz - vec2<f32>(half))) / box_size);
    // the ground under it (the height window round the camera)
    let n = i32(splash.window.w);
    let g = vec2<i32>(floor((xz - splash.window.xy) / splash.window.z));
    let inside = all(g >= vec2<i32>(0)) && all(g < vec2<i32>(n));
    let y = textureLoad(height_tex, clamp(g, vec2<i32>(0), vec2<i32>(n - 1)), 0).r;
    let p = vec3<f32>(xz.x, y, xz.y);
    let d = length(p - cam);
    if alive && inside && abs(y - cam.y) < 40.0 && d > 0.4 {
        // upright, turned to the camera about the vertical
        var to_cam = cam - p;
        to_cam.y = 0.0;
        let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), normalize(to_cam + vec3<f32>(1e-4, 0.0, 0.0))));
        let size = 0.8 + 0.4 * hash(seed * 3.3);
        let cx = select(-1.0, 1.0, corner == 1u || corner == 2u);
        let cy = select(0.0, 1.0, corner >= 2u);
        // (a hair below the surface so the sheet at its foot sits in it)
        let wp = p + right * cx * HALF_W * size + vec3<f32>(0.0, cy * TALL * size - 0.004, 0.0);
        out.world_position = vec4<f32>(wp, 1.0);
        out.position = position_world_to_clip(wp);
        out.uv = vec2<f32>(cx, cy);
        // fade with distance (the box edge) and in the last of its life
        let fade = 1.0 - smoothstep(half * 0.6, half, d);
        out.world_normal = vec3<f32>(s, fade, hash(seed * 5.1));
    }
}
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let s = in.world_normal.x;
    let fade = in.world_normal.y;
    let seed = in.world_normal.z;
    var a = 0.0;
@if(VERTEX_UVS_A) {
    // in metres across the quad: x ±HALF_W, y up from the ground
    let q = vec2<f32>(in.uv.x * HALF_W, in.uv.y * TALL);
    // droplets flung up and out on arcs (back down by the end of the life)
    for (var k = 0; k < 6; k++) {
        let fk = f32(k);
        let dx = (hash(seed * 17.0 + fk * 3.1) * 2.0 - 1.0) * 0.095;
        let up = 0.05 + hash(seed * 23.0 + fk * 5.7) * 0.07;
        let r = 0.019 * (1.0 - 0.4 * s);
        let c = vec2<f32>(dx * s, r + up * 4.0 * s * (1.0 - s));
        a = max(a, 1.0 - smoothstep(r * 0.4, r, length(q - c)));
    }
    // the crown's thin sheet at its foot early on: a low rim opening outwards
    let rim_x = 0.015 + 0.07 * s;
    let rim = (1.0 - smoothstep(0.004, 0.014, abs(abs(q.x) - rim_x))) * (1.0 - smoothstep(0.0, 0.06 * (1.0 - s) + 0.001, q.y));
    a = max(a, rim * 0.8 * (1.0 - smoothstep(0.25, 0.65, s)));
}
    let life = 1.0 - smoothstep(0.6, 1.0, s);
    let alpha = clamp(a * fade * life * 0.85, 0.0, 1.0);
    if alpha < 0.01 {
        discard;
    }
    let col = vec3<f32>(0.85, 0.9, 0.97) * (splash.amount.y * 1.3 + splash.amount.w * 1.5);
    let f = froxel_fog(fog_tex, fog_smp, splash.fog, in.position, in.world_position.xyz);
    return vec4<f32>(col * alpha * f.a + f.rgb * alpha, alpha);
}
