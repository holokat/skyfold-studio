// Air life (visuals/air.rs): the specks sunlit air is full of — fine dust, pollen and seed fluff,
// falling leaves and petals, volcanic ash and embers — in boxes that travel with the camera. Each
// speck (position in the unit cube; vertex colour = seed, phase, size, rank; normal = kind, rate)
// rides the wind (the travelled drift and fall per kind come from the CPU, wrapped to its box, so
// changing winds never jump), wanders, flutters or tumbles on its own rhythm, is pushed along by
// the shared gust field (`wind.wesl`) and fades towards its box's edges so the wrap never shows.
// The rank decides which of a kind's specks show (the share asked for), so amounts ease in.
//
// Lighting is the point: each speck reads the sun's (and moon's) shadow map, so it only shines
// inside sunbeams, and scatters strongly forwards (Henyey-Greenstein): looking towards the sun the
// air sparkles, with the sun behind you it almost vanishes. Leaves and petals are thin two-sided
// plates: lit on the sun side, glowing through when backlit. Drawn premultiplied: dust, pollen and
// embers add light (alpha 0), leaves, petals and ash flakes cover what is behind them.

import bevy_pbr::render::{
    mesh_view_bindings::{view, lights},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    view_transformations::{position_world_to_clip, position_world_to_view},
};
import super::wind::gust_field;
import super::sun_scatter::{sun_visibility, hg};
import super::volumetrics_fog::froxel_fog;

struct AirParams {
    kinds: array<vec4<f32>, 5>,   // per kind: x share shown, y size (m), z box width, w box height
    offsets: array<vec4<f32>, 5>, // per kind: xyz travelled (wind drift, fall), wrapped to the box
    tints: array<vec4<f32>, 5>,   // per kind: rgb colour (linear albedo / glow), w: its extra
    sky: vec4<f32>,               // rgb: sky fill (lux, before exposure), w: brightness
    wind: vec4<f32>,              // xy: direction (xz), z: speed (m/s), w: clock (s)
    gust: vec4<f32>,              // xy: travelled gust offset, z: gustiness, w: layer fade
    fog: vec4<f32>,               // `volumetrics::FroxelFog::params` (w 0: no volumetric light)
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> air: AirParams;
// the volumetric light's integrated volume: specks in the fog fade into it like everything else
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var fog_tex: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var fog_smp: sampler;

const TAU: f32 = 6.2831853;
const PI: f32 = 3.14159265;

const DUST: u32 = 0u;
const POLLEN: u32 = 1u;
const LEAF: u32 = 2u;
const PETAL: u32 = 3u;
const ASH: u32 = 4u;

fn rotate(v: vec3<f32>, axis: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return v * c + cross(axis, v) * s + axis * dot(axis, v) * (1.0 - c);
}

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
    var nrm = vec3<f32>(0.0, 0.5, 0.0);
@if(VERTEX_NORMALS) {
    nrm = v.normal;
}
    let k = min(u32(nrm.x + 0.5), 4u);
    let kp = air.kinds[k];
    // (the share asked for picks which specks show: a soft edge on the rank eases them in)
    let shown = clamp((kp.x - c.w) * 25.0, 0.0, 1.0) * air.gust.w;
    if shown <= 0.0 {
        // a collapsed quad draws nothing
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let seed = c.x;
    let s = seed * TAU;
    let rate = nrm.y;
    let t = air.wind.w;
    let cam = view.world_position;
    let bx = vec3<f32>(kp.z, kp.w, kp.z);
    var wander = vec3<f32>(0.0);
    if k == DUST {
        // slow Brownian loops, hardly carried
        wander = vec3<f32>(sin(t * 0.13 + s * 3.0) + 0.6 * sin(t * 0.31 + s * 7.0), 0.5 * sin(t * 0.11 + s * 5.0) + 0.3 * sin(t * 0.27 + s * 11.0), cos(t * 0.12 + s * 2.0) + 0.6 * cos(t * 0.29 + s * 13.0)) * 0.7;
    } else if k == POLLEN {
        // floating: wide lazy loops and a slow bob
        wander = vec3<f32>(sin(t * 0.09 + s * 3.0) + 0.5 * sin(t * 0.23 + s * 9.0), 0.8 * sin(t * 0.15 + s * 5.0), cos(t * 0.08 + s * 2.0) + 0.5 * cos(t * 0.21 + s * 7.0)) * 1.4;
    } else {
        // falling: swinging side to side like a pendulum, rising a little at each end of a swing
        let amp = select(select(0.45, 0.6, k == PETAL), 0.3, k == ASH);
        let ph = t * (1.3 + 1.5 * rate) + s * 5.0;
        let side = vec3<f32>(cos(s * 3.0), 0.0, sin(s * 3.0));
        wander = side * amp * sin(ph) + vec3<f32>(0.0, amp * 0.35 * cos(2.0 * ph), 0.0) + vec3<f32>(sin(t * 0.07 + s * 9.0), 0.0, cos(t * 0.06 + s * 4.0)) * 1.2;
    }
    let local = v.position * bx + air.offsets[k].xyz + wander;
    // (around the eye; more of the box above it than below, where the ground is)
    let rel = (fract((local - cam) / bx) - vec3<f32>(0.5, 0.3, 0.5)) * bx;
    var centre = cam + rel;
    // the gust field sweeps everything along in gusts and lets it hang in lulls
    var push = array<f32, 5>(0.4, 1.0, 1.1, 1.0, 0.7);
    let wdir = vec3<f32>(air.wind.x, 0.0, air.wind.y);
    let g = gust_field(centre.xz - air.gust.xy) - 0.5;
    centre = centre + wdir * g * air.wind.z * push[k] * (0.4 + air.gust.z);
    // fade at the box's edges, and right at the eye
    let rh = length(rel.xz) / (kp.z * 0.5);
    let ry = rel.y / kp.w;
    // (pollen and dust right at the eye would be big blurry balls: they stay a metre or so off)
    // (and flakes right at the eye would cover much of the view in blended fragments)
    let near = select(vec2<f32>(0.9, 1.8), select(vec2<f32>(0.6, 1.6), vec2<f32>(1.2, 3.0), k == POLLEN), k <= POLLEN);
    let fade = (1.0 - smoothstep(0.62, 0.98, rh)) * smoothstep(-0.3, -0.2, ry) * (1.0 - smoothstep(0.55, 0.7, ry)) * smoothstep(near.x, near.y, length(rel));
    // never smaller than a few pixels across (a far speck dims instead): temporal anti-aliasing
    // keeps only what fills its 3 × 3 neighbourhood and all but erases a moving speck smaller
    // than that (these draw no motion vectors), so dust and pollen are soft discs of ~5 px at least
    let d = max(length(centre - cam), 0.05);
    let px = 2.0 * d / (view.clip_from_view[1][1] * max(view.viewport.w, 1.0));
    let want = kp.y * c.z;
    // and never more than a few dozen pixels (overdraw: a speck that close is a blur anyway)
    let size = clamp(want, px * select(1.7, 2.6, k <= POLLEN), px * select(40.0, select(6.0, 14.0, k == POLLEN), k <= POLLEN));
    let energy = min((want / size) * (want / size), 1.5);
    var ax = normalize(view.world_from_view[0].xyz);
    var ay = normalize(view.world_from_view[1].xyz);
    var n = normalize(cam - centre);
    if k >= LEAF {
        // a flat flake tumbling about its own axis
        let axis = normalize(vec3<f32>(sin(s * 3.0), 0.7 * cos(s * 7.0), cos(s * 3.0)));
        let spin = select(1.0, -1.0, fract(seed * 7.0) > 0.5);
        let a = t * (0.5 + 2.8 * rate) * spin + s * 4.0;
        ax = rotate(vec3<f32>(1.0, 0.0, 0.0), axis, a);
        ay = rotate(vec3<f32>(0.0, 0.0, 1.0), axis, a);
        n = normalize(cross(ax, ay));
    }
    let wp = centre + (ax * corner.x + ay * corner.y) * size;
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = n;
@if(VERTEX_UVS_A)
    out.uv = corner;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(f32(k), seed, fade * shown, energy);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

// A pointed oval leaf on a short stalk (uv: −1..1, stalk at −y); 0 outside.
// (`aa`: the edge's softness, from the uv's footprint: derivatives are taken up front, where the
// control flow is still uniform)
fn leaf_shape(uv: vec2<f32>, aa: f32) -> f32 {
    let y = uv.y * 1.1 + 0.1;
    let w = 0.55 * pow(max(1.0 - y * y, 0.0), 0.7) * (1.0 - 0.3 * y);
    let e = abs(uv.x) - w;
    var m = 1.0 - smoothstep(-aa, aa, e);
    // the stalk
    m = max(m, (1.0 - smoothstep(0.03, 0.06, abs(uv.x))) * step(-1.0, uv.y) * (1.0 - step(-0.75, uv.y)));
    return m;
}

// A rounded petal narrowing to its base, a little notch at its tip.
fn petal_shape(uv: vec2<f32>, aa: f32) -> f32 {
    let y = uv.y;
    let w = 0.72 * sqrt(max(1.0 - y * y, 0.0)) * (0.45 + 0.55 * (y * 0.5 + 0.5));
    let e = abs(uv.x) - w;
    let notch = smoothstep(0.1, 0.16, length(uv - vec2<f32>(0.0, 1.02)));
    return (1.0 - smoothstep(-aa, aa, e)) * notch;
}

// An irregular flake of ash.
fn flake_shape(uv: vec2<f32>, seed: f32, aa: f32) -> f32 {
    let a = atan2(uv.y, uv.x);
    let edge = 0.72 + 0.18 * sin(a * 3.0 + seed * 20.0) + 0.1 * sin(a * 5.0 + seed * 31.0);
    let r = length(uv);
    return 1.0 - smoothstep(edge - aa, edge + aa, r);
}

// Leaf colours (linear albedo): summer greens with a few dry ones; autumn's oranges, reds, golds
// and browns for the share `autumn`.
fn leaf_colour(seed: f32, autumn: f32) -> vec3<f32> {
    let h = fract(seed * 17.31);
    var summer = mix(vec3<f32>(0.10, 0.20, 0.035), vec3<f32>(0.30, 0.36, 0.07), h);
    if h > 0.8 {
        summer = vec3<f32>(0.36, 0.26, 0.09);
    }
    var fall = vec3<f32>(0.62, 0.21, 0.035);
    if h < 0.28 {
        fall = vec3<f32>(0.52, 0.07, 0.025);
    } else if h < 0.55 {
        fall = vec3<f32>(0.75, 0.46, 0.05);
    } else if h > 0.86 {
        fall = vec3<f32>(0.28, 0.15, 0.06);
    }
    return mix(summer, fall, step(fract(seed * 5.71), autumn));
}

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var vc = vec4<f32>(0.0, 0.5, 1.0, 1.0);
@if(VERTEX_COLORS) {
    vc = in.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    var out: FragmentOutput;
    let aa = max(fwidth(uv.x), fwidth(uv.y)) + 0.01;
    let k = min(u32(vc.x + 0.5), 4u);
    let seed = vc.y;
    let r2 = dot(uv, uv);
    if r2 > 1.0 && k <= POLLEN {
        discard;
    }
    let wp = in.world_position.xyz;
    let V = normalize(wp - view.world_position);
    // the flake's two sides: the one facing the eye
    var N = normalize(in.world_normal);
    if dot(N, V) > 0.0 {
        N = -N;
    }
    let tint = air.tints[k];
    // coverage of the speck's shape
    var cover = 0.0;
    // (a speck grown to its few pixels is a flat disc, so all of it shows)
    let flat = clamp(1.0 - vc.w, 0.0, 1.0);
    if k == DUST {
        cover = mix(exp(-r2 * 7.0), 1.0 - smoothstep(0.45, 1.0, r2), flat);
    } else if k == POLLEN {
        // a bright seed in a soft tuft
        cover = mix(exp(-r2 * 14.0) + 0.3 * exp(-r2 * 3.0), 1.0 - smoothstep(0.45, 1.0, r2), flat);
    } else if k == LEAF {
        cover = leaf_shape(uv, aa);
    } else if k == PETAL {
        cover = petal_shape(uv, aa);
    } else {
        cover = flake_shape(uv, seed, aa);
    }
    let a = cover * vc.z;
    if a < 0.002 {
        discard;
    }
    // the sun (and the moon): shadowed, scattered forwards, on either side of a flake
    let view_z = position_world_to_view(wp).z;
    let g = select(0.55, 0.68, k <= POLLEN);
    var lit = vec3<f32>(0.0);
    var fwd = vec3<f32>(0.0);
    var front = vec3<f32>(0.0);
    var back = vec3<f32>(0.0);
    let n_lights = lights.n_directional_lights;
    for (var i: u32 = 0u; i < n_lights; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = light.direction_to_light;
        let col = light.color.rgb * view.exposure * sun_visibility(i, wp, L, view_z);
        lit += col;
        fwd += col * hg(g, dot(V, L));
        let nl = dot(N, L);
        front += col * max(nl, 0.0);
        back += col * max(-nl, 0.0);
    }
    let amb = (air.sky.rgb + lights.ambient_color.rgb) * view.exposure;
    let bright = air.sky.w;
    var rgb = vec3<f32>(0.0);
    var alpha = 0.0;
    if k == DUST {
        // only seen by the light it scatters: a speck in the beams, nothing in the shade, and now
        // and then a glint as a turning flake catches the sun
        let glint = 0.6 + 2.4 * pow(0.5 + 0.5 * sin(air.wind.w * (1.5 + 4.0 * fract(seed * 3.7)) + seed * 40.0), 12.0);
        rgb = tint.rgb * (fwd * 1.6 + lit * 0.07 + amb * 0.01) * glint * vc.w * bright;
    } else if k == POLLEN {
        rgb = tint.rgb * (fwd * 0.7 + lit * 0.05 + amb * 0.03) * vc.w * bright;
        alpha = 0.15 * cover * vc.z;
    } else if k == ASH && fract(seed * 13.7) < tint.w {
        // an ember: glowing on its own, flickering, dimming as it cools
        let fl = 0.55 + 0.45 * sin(air.wind.w * (6.0 + 9.0 * fract(seed * 3.1)) + seed * 40.0);
        rgb = vec3<f32>(1.0, 0.32, 0.06) * exp(-r2 * 3.0) * fl * 3.0 * vc.w;
    } else {
        var albedo = tint.rgb;
        var through = 0.55;
        if k == LEAF {
            albedo = leaf_colour(seed, tint.w);
            // the midrib
            albedo *= 1.0 - 0.3 * (1.0 - smoothstep(0.0, 0.05, abs(uv.x)));
        } else if k == PETAL {
            // pinks and whites (tint.w: the share of white ones), paler towards the tip
            let white = step(fract(seed * 9.13), tint.w);
            albedo = mix(tint.rgb, vec3<f32>(0.86, 0.84, 0.8), white) * mix(0.85, 1.1, uv.y * 0.5 + 0.5);
            through = 0.6;
        } else {
            albedo = tint.rgb * mix(0.7, 1.15, fract(seed * 5.3));
            through = 0.1;
        }
        // lit side, light through it when backlit (brighter still looking into the sun), sky fill
        rgb = albedo * (front / PI + amb) + albedo * through * (back / PI + fwd * 0.5);
        rgb *= bright;
        alpha = a;
    }
    // the haze or fog between the eye and the speck (premultiplied: it dims the speck, and a
    // covering flake shows the fog's own light in front of it; glows only dim)
    let f = froxel_fog(fog_tex, fog_smp, air.fog, in.position, wp);
    out.color = vec4<f32>(rgb * a * f.a + f.rgb * alpha, alpha);
    return out;
}
