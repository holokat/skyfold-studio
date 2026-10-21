// Water: a port of Emberfall's three.js sea surface (src/sea/scene/ocean/surface.js) on Bevy PBR,
// shaded like tidewater's sea (a custom radiance model composed in the shader).
//
// Vertex (ocean): the camera-following polar grid lifted by the three FFT cascades (choppy
// displacement), each read at the mip its vertex spacing can hold; in shallow water the long
// swell dies first while the short chop survives (per-cascade attenuation). Lakes and rivers rise
// and fall with a tidal behaviour, and a whirlpool pulls a lake's surface down in the middle.
// Fragment:
//   normal     the cascades' true slopes summed; their mips' slope variance widens the roughness
//              with distance (LEAN) so the sun's glitter spreads into a path instead of aliasing;
//              near the eye the finest cascade is re-sampled at smaller rotated tiles (capillaries).
//              Rivers read the cascades in their own frame (across, along the course) moving
//              downstream, so their ripples follow every bend without shearing; a whirlpool's
//              swirl is a two-phase flow map
//   sea detail world-space gusts (cat's paws), glassy slicks and windrows that drift with the wind
//              and break up the open sea's tiling
//   reflection Bevy PBR: the atmosphere's environment map + GGX sun specular with shadows; the
//              foam is its only diffuse part. Surface roughness blurs it; screen-space reflection
//              (none / short / full march with blur by quality). Liquid metal tints it with the
//              water colour, iridescence adds a thin-film rainbow
//   water body light scattered towards the eye along the refracted view ray through the terrain
//              heightfield (per-channel extinction from the water type), the low sun shining
//              through the crests, bubbles under foam; transparency blends it with an opaque,
//              infinitely deep body (paint-like liquids)
//   glow       inner glow veins (arcane waters) and bioluminescence: crests, foam, bubbles and
//              sparkles that light up in the dark
//   behaviour  still mirror, ripples, flows, rapids (white water torn along the current),
//              whirlpool (swirl and spiral foam), storm (big waves, caps, rain), tidal, bubbling
//              (bubbles rising and bursting in rings)
//   output     premultiplied: reflection, glint and in-scattered light are added as they are; only
//              the scene behind is dimmed by what the water column lets through
//   fog        Bevy's distance fog on the water's own light; the scene it shows was fogged already
//              (`fog_surface`)
//   foam       whitecaps from each cascade's Jacobian, built where a crest folds and fading over
//              seconds (CPU sim), a fresh white edge on the sharpest crests, gale streaks,
//              windrows, shore break and river rapids; lace up close, averaged coverage far away
//              (by pixel footprint)
//   surf       each wave breaks over the shallows and rolls in as a bore of white water whose foam
//              spreads into lace and dissolves; it reaches the waterline just as its swash starts
//              up the sand (shore.wesl keeps the beach's clock in step)
//   rivers     the current shows: drifting foam flecks and streaks, white water where it runs
//              shallow or fast and along the banks of bends
//   lava       (Molten behaviour) a dark crust of plates drifting with the flow over glowing,
//              slowly opening cracks; hotter and brighter in the middle of the flow

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    mesh_view_bindings as vb,
    mesh_view_types::{FOG_MODE_LINEAR, FOG_MODE_EXPONENTIAL, FOG_MODE_EXPONENTIAL_SQUARED, FOG_MODE_ATMOSPHERIC},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_functions as pf,
    pbr_types::{PbrInput, STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT},
    pbr_lighting::F_AB,
    mesh_functions,
    view_transformations::{position_world_to_clip, position_world_to_view, direction_world_to_view},
};
import super::noise::{cloud_density, cloud_cover, CLOUD_TILE, hashf, value_noise2, voronoi2};
import super::shore::{surf_cycle, surf_size};

struct WaterParams {
    shallow: vec4<f32>,
    deep: vec4<f32>,
    scatter: vec4<f32>,
    grid: vec4<f32>,     // half size, cell, verts per side, sea level
    params: vec4<f32>,   // kind (0 ocean, 1 lake, 2 river), clarity, wave scale, foam amount
    wind: vec4<f32>,     // dir xy, rain, river flow speed
    look: vec4<f32>,     // foam, whitecaps, streaks, chop
    look2: vec4<f32>,    // hs, rain, wind, choppy
    sun: vec4<f32>,      // direction to the key light, key strength
    sun_col: vec4<f32>,  // key light colour (display units)
    sky: vec4<f32>,      // sky irradiance (display units), glitter
    casc: array<vec4<f32>, 3>, // (0, 0, 1/L, L/N)
    extinct: vec4<f32>,  // rgb: extinction per metre, w: clarity
    inscat: vec4<f32>,   // rgb: in-scatter albedo (deep water colour)
    detail: vec4<f32>,   // xy: gust field drift (m), z: wind speed (m/s), w: gust amount
    clouds: vec4<f32>,   // x: cloud coverage, y: cloud shadow strength, zw: cloud offset
    style: vec4<f32>,    // x: roughness, y: shore foam, z: wave speed, w: reflection quality
    optics: vec4<f32>,   // x: transparency, y: metallic, z: iridescence, w: behaviour
    glow: vec4<f32>,     // rgb: bioluminescence, w: darkness
    emis: vec4<f32>,     // rgb: inner glow
    flowv: vec4<f32>,    // xy: flow direction (lakes, sea), z: flow speed (m/s), w: tide (m)
    body: vec4<f32>,     // xy: body centre, z: radius (rivers: width), w: storm
    surface: vec4<f32>,  // x: undisturbed surface height for planar matching
    reflection_clip: mat4x4<f32>, // last rendered planar reflection's clip-from-world
    reflection: vec4<f32>, // valid, plane height, capture-to-current exposure scale, unused
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> water: WaterParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var height_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var disp0: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var disp1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var disp2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var norm0: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var norm1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var norm2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var noise_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(110) var foam_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(111) var scene_depth: texture_depth_2d;
@group(constants::MATERIAL_BIND_GROUP) @binding(112) var caustic_tex: texture_2d<f32>;
// the volumetric clouds' baked cover field over one CLOUD_TILE (clouds.rs: `CloudCoverMap`);
// builds without it (CLOUD_COVER_MAP unset) evaluate the cover per pixel
@if(CLOUD_COVER_MAP) {
@group(constants::MATERIAL_BIND_GROUP) @binding(113) var cloud_cover_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(114) var cloud_sampler: sampler;
}
@group(constants::MATERIAL_BIND_GROUP) @binding(115) var planar_reflection_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(116) var planar_reflection_sampler: sampler;

const IOR: f32 = 1.333;
// behaviours (WaterBehaviour::ALL order)
const B_STILL: u32 = 0u;
const B_RAPIDS: u32 = 5u;
const B_WHIRLPOOL: u32 = 6u;
const B_BUBBLING: u32 = 9u;
const B_MOLTEN: u32 = 10u;

// smoothstep that also accepts falling edges (a > b)
fn ss(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn rot2(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(v.x * c - v.y * s, v.x * s + v.y * c);
}

fn terrain_height(xz: vec2<f32>) -> f32 {
    let half = water.grid.x;
    let cell = water.grid.y;
    let n = i32(water.grid.z);
    if n < 2 {
        return -1000.0;
    }
    let g = clamp((xz + vec2<f32>(half)) / cell, vec2<f32>(0.0), vec2<f32>(f32(n - 1)));
    let i = vec2<i32>(floor(g));
    let f = g - floor(g);
    let i1 = min(i + vec2<i32>(1), vec2<i32>(n - 1));
    let a = textureLoad(height_tex, i, 0).r;
    let b = textureLoad(height_tex, vec2<i32>(i1.x, i.y), 0).r;
    let c = textureLoad(height_tex, vec2<i32>(i.x, i1.y), 0).r;
    let d = textureLoad(height_tex, i1, 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Long waves feel the bottom first: the 800 m swell is gone in a few metres of water, the 151 m
// wind sea fades later, the 27.7 m chop survives into the shallows (tidewater WaterSurface.js).
fn cascade_atten(depth: f32) -> vec3<f32> {
    let d0 = vec3<f32>(40.0, 12.1, 2.2);
    let floor_amt = vec3<f32>(0.0, 0.05, 0.25);
    let a = vec3<f32>(ss(0.0, d0.x, depth), ss(0.0, d0.y, depth), ss(0.0, d0.z, depth));
    return mix(floor_amt * ss(0.0, 0.6, depth), vec3<f32>(1.0), a);
}

// Height of the open sea right above / below the camera (the same lift as the vertex stage), so
// the surface knows whether it is seen from below.
fn sea_surface_at(xz: vec2<f32>) -> f32 {
    let att = cascade_atten(water.grid.w - terrain_height(xz));
    let q = xz - water.flowv.xy * (water.flowv.z * globals.time);
    var h = textureSampleLevel(disp0, samp, q * water.casc[0].z, 0.0).y * att.x;
    h += textureSampleLevel(disp1, samp, q * water.casc[1].z, 0.0).y * att.y;
    h += textureSampleLevel(disp2, samp, q * water.casc[2].z, 0.0).y * att.z;
    return water.grid.w + h;
}

// The cover map is the same tiled field the visible cloud renderer and terrain shadows use. A
// water reflection intersects that actual field at the cloud base instead of laying a screen-space
// pattern over the water, so a clear part of the sky remains clear in the reflection.
fn cloud_cover_at(xz: vec2<f32>) -> f32 {
    if water.clouds.x <= 0.001 {
        return 0.0;
    }
    @if(CLOUD_COVER_MAP) {
        if textureDimensions(cloud_cover_tex).x > 1u {
            let n = textureSampleLevel(cloud_cover_tex, cloud_sampler, (xz + water.clouds.zw) / CLOUD_TILE, 0.0).r;
            return cloud_cover(n, water.clouds.x);
        }
    }
    return cloud_density(xz, water.clouds.x, water.clouds.zw);
}

// The surface seen from under the water (motion/underwater.rs fogs the path to it): inside
// Snell's window the world above, squeezed into a 97° cone and rippling with the waves, with the
// sun's disc; beyond the critical angle total internal reflection mirrors the water's own depths;
// foam blocks the light from below. Also returns the opaque scene it shows through the window and
// that scene's share of the colour (for `fog_surface`).
struct Under {
    col: vec3<f32>,
    scene: vec3<f32>,
    seen: f32,
}

fn underside(wp: vec3<f32>, V: vec3<f32>, N: vec3<f32>, cover: f32) -> Under {
    let Nd = -N;
    let I = -V;
    // into the air (zero beyond the critical angle)
    let Tr = refract(I, Nd, IOR);
    let L = normalize(water.sun.xyz);
    let sun = water.sun_col.rgb * water.sun.w;
    let sky_irr = water.sky.rgb;
    let alb = water.inscat.rgb;
    // what the water's own depths send back (reflected below the surface)
    let Ls = normalize(vec3<f32>(L.x / IOR, max(sqrt(max(1.0 - (1.0 - L.y * L.y) / (IOR * IOR), 0.0)), 0.1), L.z / IOR));
    let sun_in = sun * (0.98 - 0.98 * pow(1.0 - max(L.y, 0.0), 5.0)) * ss(0.0, 0.12, L.y) * max(L.y, 0.0) / max(Ls.y, 0.1);
    let depths = sun_in * alb * 1.4 + sky_irr * alb * 2.1;
    var u: Under;
    u.col = depths;
    u.scene = vec3<f32>(0.0);
    u.seen = 0.0;
    if dot(Tr, Tr) > 0.0 {
        // Fresnel from the water side (Schlick on the air-side angle)
        let cos_t = clamp(dot(Tr, N), 0.0, 1.0);
        let F = 0.02 + 0.98 * pow(1.0 - cos_t, 5.0);
        // the world above where the bent ray lands on screen, else the sky
        var above = sky_irr * 1.4 + vec3<f32>(0.03, 0.04, 0.05) * water.sun.w;
        let c = position_world_to_clip(wp + Tr * 80.0);
        let suv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
        if c.w > 0.0 && all(suv > vec2<f32>(0.002)) && all(suv < vec2<f32>(0.998)) {
            u.scene = scene_color(suv);
            above = mix(above, u.scene * 1.5, 0.8);
            u.seen = 1.5 * 0.8;
        }
        // the sun: a hot disc and a soft halo through the moving surface
        let sd = max(dot(Tr, L), 0.0);
        above += sun * (pow(sd, 1400.0) * 40.0 + pow(sd, 60.0) * 0.6 + pow(sd, 6.0) * 0.08);
        // the window's rim grows dim and greenish as it nears the critical angle
        let win = (1.0 - F) * ss(0.0, 0.25, cos_t);
        u.col = mix(depths, above, win);
        u.seen *= win;
    }
    // foam and bubbles from below: grey, light-blocking blotches
    let light = sky_irr * 0.85 + sun * max(L.y, 0.0) * 0.35;
    u.col = mix(u.col, light * vec3<f32>(0.32, 0.36, 0.38), cover * 0.85);
    u.seen *= 1.0 - cover * 0.85;
    return u;
}

// ---- the three cascades at one sample position
struct Casc {
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
    d0: vec4<f32>,
    d1: vec4<f32>,
    d2: vec4<f32>,
}

fn casc_at(sp: vec2<f32>) -> Casc {
    var c: Casc;
    let uv0 = sp * water.casc[0].z;
    let uv1 = sp * water.casc[1].z;
    let uv2 = sp * water.casc[2].z;
    c.n0 = textureSample(norm0, samp, uv0);
    c.n1 = textureSample(norm1, samp, uv1);
    c.n2 = textureSample(norm2, samp, uv2);
    c.d0 = textureSample(disp0, samp, uv0);
    c.d1 = textureSample(disp1, samp, uv1);
    c.d2 = textureSample(disp2, samp, uv2);
    return c;
}

fn casc_mix(a: Casc, b: Casc, t: f32) -> Casc {
    var c: Casc;
    c.n0 = mix(a.n0, b.n0, t);
    c.n1 = mix(a.n1, b.n1, t);
    c.n2 = mix(a.n2, b.n2, t);
    c.d0 = mix(a.d0, b.d0, t);
    c.d1 = mix(a.d1, b.d1, t);
    c.d2 = mix(a.d2, b.d2, t);
    return c;
}

// Near-field capillaries (see the fragment): (slope.xy, variance). Explicit LOD: runs in branches.
fn near_detail(sp: vec2<f32>, fp: f32, near_a: f32, near_b: f32) -> vec3<f32> {
    let ka = 4.1 / 27.7;
    let c1 = textureSampleLevel(norm2, samp, rot2(sp, 0.63) * ka, max(log2(fp * ka * 256.0), 0.0));
    var slope = rot2(c1.xy, -0.63) * (0.3 * near_a);
    let vari = max(c1.z - dot(c1.xy, c1.xy), 0.0) * 0.09 * near_a * near_a;
    if near_b > 0.002 {
        let kb = 12.0;
        let kc = 28.5;
        let c2 = textureSampleLevel(norm2, samp, rot2(sp, 2.14) * (kb / 27.7), max(log2(fp * kb / 27.7 * 256.0), 0.0));
        let c3 = textureSampleLevel(norm2, samp, rot2(sp, 4.02) * (kc / 27.7), max(log2(fp * kc / 27.7 * 256.0), 0.0));
        slope += (rot2(c2.xy, -2.14) * 0.35 + rot2(c3.xy, -4.02) * 0.5) * near_b;
    }
    return vec3<f32>(slope, vari);
}

// One layer of rising bubbles: per cell a bubble breaks the surface (a foam dot) and its ring
// spreads and fades. Returns (ring slope xy, foam).
fn bubble_layer(p: vec2<f32>, t: f32, cell: f32, seed: u32) -> vec3<f32> {
    let g = floor(p / cell);
    let f = p / cell - g;
    let ix = i32(g.x);
    let iy = i32(g.y);
    let h1 = hashf(ix, iy, seed);
    let h2 = hashf(ix, iy, seed + 1u);
    let h3 = hashf(ix, iy, seed + 2u);
    let c = vec2<f32>(0.3 + 0.4 * h1, 0.3 + 0.4 * h2);
    let life = fract(t * (0.22 + 0.4 * h3) + h1 * 7.0);
    let live = step(0.4, h3);
    let rel = f - c;
    let dl = length(rel);
    let d = dl * cell;
    let rad = life * cell * 0.3;
    let w = 0.03 + 0.06 * life * cell;
    let x = (d - rad) / w;
    let ring = exp(-x * x) * (1.0 - life) * live;
    let pop = ss(0.1 * cell, 0.0, d) * ss(0.15, 0.0, life) * live;
    let dir = rel / max(dl, 1e-3);
    return vec3<f32>(dir * ring * 0.45, ring * 0.3 + pop);
}

// ---- lava (Molten): plates of dark cooling crust drifting with the flow over molten rock that
// glows through the cracks between them. `q` is the surface position in the flow's frame
// (moving with the current), `heat` 0..1 (hotter in the middle of a flow). Returns the crust's
// albedo (rgb) and roughness (a) in `crust`, the glow in display units in `glow.rgb` and the
// crust's relief slope in `slope`.
struct Lava {
    crust: vec4<f32>,
    glow: vec3<f32>,
    slope: vec2<f32>,
}

fn lava(q: vec2<f32>, t: f32, heat: f32, fp: f32) -> Lava {
    var o: Lava;
    // two sizes of plates; the cracks between them open and close slowly
    let warp = vec2<f32>(value_noise2(q * 0.11 + vec2<f32>(t * 0.013, 0.0), 301u), value_noise2(q * 0.11 + vec2<f32>(5.2, -t * 0.011), 307u)) - 0.5;
    let p = q + warp * 3.5;
    let v1 = voronoi2(p * 0.22, 311u);
    let v2 = voronoi2(p * 0.7 + vec2<f32>(3.7, 1.9), 313u);
    let open = 0.5 + 0.5 * sin(t * 0.35 + v1.z * 6.2832);
    let w1 = mix(0.012, 0.055, heat) * (0.5 + 0.8 * open);
    let w2 = mix(0.006, 0.03, heat);
    // (cracks antialiased by footprint: far away they average into a dull red)
    let aa = fp * 0.6;
    let crack1 = 1.0 - smoothstep(w1, w1 + 0.035 + aa * 0.22, v1.y - v1.x);
    let crack2 = (1.0 - smoothstep(w2, w2 + 0.03 + aa * 0.7, v2.y - v2.x)) * 0.55;
    let crack = max(crack1, crack2);
    // molten rock: deep red as it cools, orange, and yellow only where it is hottest
    let hot = clamp(heat * 0.55 + open * 0.2 + (value_noise2(q * 0.05 + vec2<f32>(0.0, t * 0.02), 317u) - 0.5) * 0.5, 0.0, 1.0);
    let molten = mix(vec3<f32>(0.7, 0.07, 0.01), mix(vec3<f32>(1.3, 0.38, 0.05), vec3<f32>(2.0, 1.1, 0.3), smoothstep(0.75, 1.0, hot)), smoothstep(0.2, 0.75, hot));
    // the crust: near-black basalt, a dull red glow through thin crust where it is hot
    let shade = 0.035 + 0.05 * v1.z + 0.03 * value_noise2(q * 1.7, 319u);
    let thin = smoothstep(0.5, 1.0, heat) * (1.0 - smoothstep(0.0, 0.25, v1.y - v1.x));
    o.crust = vec4<f32>(vec3<f32>(shade, shade * 0.93, shade * 0.88), mix(0.75, 0.55, heat));
    // (far away the cracks average out: a footprint-wide glow keeps the flow reading hot)
    let far = smoothstep(0.2, 2.5, fp);
    let avg = mix(0.04, 0.2, heat);
    let lit = mix(crack, avg, far);
    o.glow = molten * (lit * 1.3 + thin * 0.12) + vec3<f32>(0.25, 0.04, 0.0) * heat * 0.2;
    // tilted plates with ropy wrinkles across the flow
    let rope = sin(p.y * 2.3 + v2.x * 3.0) * 0.5 * (1.0 - far);
    o.slope = vec2<f32>(warp.x * 0.25, warp.y * 0.25 + rope * 0.08) * (1.0 - crack);
    return o;
}

// ---- what the opaque scene holds (depth copy, colour copy)

// view z (negative ahead) of the opaque scene at a screen uv; the sky is infinitely far
fn scene_view_z(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(scene_depth));
    let d = textureLoad(scene_depth, vec2<i32>(clamp(uv, vec2<f32>(0.0), vec2<f32>(0.9999)) * size), 0);
    // (reverse-z perspective: depth = near / distance)
    return -view.clip_from_view[3][2] / max(d, 1e-9);
}

fn view_to_uv(p: vec3<f32>) -> vec2<f32> {
    let c = view.clip_from_view * vec4<f32>(p, 1.0);
    let n = c.xy / max(c.w, 1e-4);
    return vec2<f32>(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5);
}

fn scene_color(uv: vec2<f32>) -> vec3<f32> {
    var col = vec3<f32>(0.0);
    @if(VIEW_TRANSMISSION_TEXTURE) {
        col = textureSampleLevel(vb::view_transmission_texture, vb::view_transmission_sampler, uv, 0.0).rgb;
    }
    return col;
}

// The capture camera is mirrored and clipped at the selected water plane. Projecting the present
// surface position into that camera gives the exact flat-mirror lookup; the long-wave slope adds
// only a small distortion so a calm shoreline remains coherent. Rough open water uses a compact
// five-tap footprint instead of dropping the coherent off-screen sky and shore entirely.
fn planar_reflection(wp: vec3<f32>, reflection_slope: vec2<f32>, dist: f32, rough: f32, reflected_up: f32) -> vec4<f32> {
    // One capture can only describe its chosen flat sea or lake. Rivers use SSR/environment;
    // a different lake height also falls back instead of sampling a geometrically wrong mirror.
    if water.reflection.x < 0.5 || water.style.w < 0.5 || water.params.x > 1.1 || abs(water.surface.x - water.reflection.y) > 0.025 {
        return vec4<f32>(0.0);
    }
    let c = water.reflection_clip * vec4<f32>(wp, 1.0);
    if c.w <= 0.0 {
        return vec4<f32>(0.0);
    }
    var uv = vec2<f32>(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
    let wave = reflection_slope * mix(0.012, 0.002, ss(35.0, 500.0, dist));
    uv += vec2<f32>(wave.x, -wave.y);
    // Match the sampling margin to the roughness blur so clamp-to-edge never pulls a bright
    // shoreline or cloud band over the texture boundary.
    let blur = ss(0.20, 0.62, rough);
    let texel = 1.0 / vec2<f32>(textureDimensions(planar_reflection_tex, 0));
    let radius = (1.0 + 11.0 * blur) * texel;
    let edge = ss(0.0, radius.x, uv.x) * ss(0.0, radius.y, uv.y)
        * ss(1.0, 1.0 - radius.x, uv.x) * ss(1.0, 1.0 - radius.y, uv.y);
    if edge <= 0.0 {
        return vec4<f32>(0.0);
    }
    var col = textureSampleLevel(planar_reflection_tex, planar_reflection_sampler, uv, 0.0).rgb;
    if blur > 0.01 {
        // A cross kernel keeps the reflected cloud mass and horizon coherent on a choppy sea
        // without pasting a sharp image over its wave-normal variation. The planar target is
        // already half-resolution, so this is deliberately a small bounded extra footprint.
        let dx = vec2<f32>(radius.x, 0.0);
        let dy = vec2<f32>(0.0, radius.y);
        col = col * 0.50
            + textureSampleLevel(planar_reflection_tex, planar_reflection_sampler, uv + dx, 0.0).rgb * 0.125
            + textureSampleLevel(planar_reflection_tex, planar_reflection_sampler, uv - dx, 0.0).rgb * 0.125
            + textureSampleLevel(planar_reflection_tex, planar_reflection_sampler, uv + dy, 0.0).rgb * 0.125
            + textureSampleLevel(planar_reflection_tex, planar_reflection_sampler, uv - dy, 0.0).rgb * 0.125;
    }
    // At a high sea-eye the mirrored atmosphere eye lies below the planet's ground reference.
    // Its oblique horizon can contain a thin unstable colour row. The environment already covers
    // that nearly horizontal direction, so hand it back only as the virtual eye moves well below
    // ground; low shore cameras and elevated lakes keep their captured horizon.
    let mirror_eye_y = 2.0 * water.surface.x - view.world_position.y;
    let below_ground = max(-mirror_eye_y, 0.0);
    let horizon_fallback = ss(2.0, 10.0, below_ground) * ss(0.10, 0.02, reflected_up);
    return vec4<f32>(col * water.reflection.z, edge * (1.0 - horizon_fallback));
}

// ---- distance fog. What this surface shows of the opaque scene (the floor through the water, the
// shore at the waterline, the hills in its mirror, the world above through Snell's window) was
// fogged in the opaque pass over about the same path: fogged again here it would leave shallow,
// clear or mirror-calm water hazier than the land beside it. So only the water's own light takes
// the fog. Bevy's fog is affine, apply_fog(c) = c · keep + inscatter; with `kept` that already
// fogged light in `col` and `kept_w` (per channel) its share of the pixel:
//   own · keep + inscatter · (1 − kept_w) + kept,  own = col − kept
// (a share above 1, a boosted scene, carries its own fog: no more in-scatter is taken away).

// how much of a surface's light Bevy's distance fog lets through over `d` metres (fog.wesl; the fog
// colour's alpha is its strength)
fn fog_keep(d: f32) -> vec3<f32> {
    var keep = vec3<f32>(1.0);
    @if(DISTANCE_FOG) {
        let f = vb::fog;
        var a = vec3<f32>(0.0);
        if f.mode == FOG_MODE_LINEAR {
            a = vec3<f32>(1.0 - clamp((f.be.y - d) / (f.be.y - f.be.x), 0.0, 1.0));
        } else if f.mode == FOG_MODE_EXPONENTIAL {
            a = vec3<f32>(1.0 - exp(-d * f.be.x));
        } else if f.mode == FOG_MODE_EXPONENTIAL_SQUARED {
            let x = d * f.be.x;
            a = vec3<f32>(1.0 - exp(-x * x));
        } else if f.mode == FOG_MODE_ATMOSPHERIC {
            a = 1.0 - exp(-d * f.be);
        }
        keep = 1.0 - a * f.base_color.a;
    }
    return keep;
}

fn fog_surface(col: vec3<f32>, kept: vec3<f32>, kept_w: vec3<f32>, wp: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    var out = col;
    @if(DISTANCE_FOG) {
        let own = col - kept;
        let fogged = pf::apply_fog(vb::fog, vec4<f32>(own, 1.0), wp, view.world_position, frag).rgb;
        let inscatter = fogged - own * fog_keep(distance(wp, view.world_position));
        out = fogged + kept - inscatter * min(kept_w, vec3<f32>(1.0));
    }
    return out;
}

// The water's final colour: fogged here, then Bevy's post-lighting steps without its own fog
fn finish(pbr_in: PbrInput, col: vec3<f32>, kept: vec3<f32>, kept_w: vec3<f32>) -> vec4<f32> {
    var p = pbr_in;
    p.material.flags &= ~STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    let c = fog_surface(col, kept, kept_w, p.world_position.xyz, p.frag_coord.xy);
    return main_pass_post_lighting_processing(p, vec4<f32>(c, 1.0));
}

// Screen-space reflection (after tidewater WaterMaterial.js `_waterSSR`, with a longer reach for
// the island's hills): march the reflected ray through the opaque depth copy in view space
// (geometric steps out to ~1.5 km, then a bisection), so calm water mirrors the shore, trees and
// hills. Returns (colour, weight): the weight fades at screen
// edges, for rays heading back toward the camera, at the end of the search range and for hits
// under the water (those rays run into the next wave instead). `blur` (surface roughness) spreads
// the lookup over a few taps.
fn water_ssr(pos_v: vec3<f32>, rv: vec3<f32>, y0: f32, ry: f32, steps: i32, refine: i32, blur: f32) -> vec4<f32> {
    var hit = false;
    // steps grow with distance: far away the first ones would all land in the same pixel
    let step_scale = max(-pos_v.z / 60.0, 1.0);
    var t = 0.15 * step_scale;
    var dt = 0.25 * step_scale * select(1.0, 1.7, steps < 14);
    let growth = select(1.7, 2.1, steps < 14);
    var prev_t = 0.0;
    for (var i = 0; i < 16; i++) {
        if i >= steps {
            break;
        }
        prev_t = t;
        if prev_t >= 1500.0 || (ry <= 0.0 && y0 + ry * prev_t < y0 - 0.2) {
            break;
        }
        t += dt;
        dt *= growth;
        let p = pos_v + rv * t;
        let uv = view_to_uv(p);
        if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || p.z > -0.1 {
            break;
        }
        let sz = scene_view_z(uv);
        // behind the visible surface, within a thickness covering the last step
        if p.z < sz && sz - p.z < max(dt * 1.3, max(t * 0.08, 0.3)) {
            hit = true;
            break;
        }
    }
    var col = vec3<f32>(0.0);
    var w = 0.0;
    if hit {
        var a = prev_t;
        var b = t;
        for (var k = 0; k < 5; k++) {
            if k >= refine {
                break;
            }
            let m = (a + b) * 0.5;
            let p = pos_v + rv * m;
            let behind = p.z < scene_view_z(view_to_uv(p));
            b = select(b, m, behind);
            a = select(m, a, behind);
        }
        let hit_v = pos_v + rv * b;
        let uv = view_to_uv(hit_v);
        // the refined ray must really touch the surface there (not pass far behind a thin object)
        let gap = abs(scene_view_z(uv) - hit_v.z);
        let touch = ss(max(b * 0.04, 0.4), max(b * 0.02, 0.2), gap);
        let hit_w = view.world_from_view * vec4<f32>(hit_v, 1.0);
        col = scene_color(uv);
        if blur > 0.0005 {
            // a rough surface spreads the reflection: the cone grows with the reflected distance
            let r = clamp(blur * b / max(-hit_v.z, 1.0) * 0.35, 0.0, 0.035);
            col = col * 0.2
                + (scene_color(uv + vec2<f32>(r, r * 0.4)) + scene_color(uv - vec2<f32>(r, r * 0.4))
                + scene_color(uv + vec2<f32>(-r * 0.4, r * 1.6)) + scene_color(uv + vec2<f32>(r * 0.4, -r * 1.6))) * 0.2;
        }
        let edge = ss(0.0, 0.12, uv.x) * ss(1.0, 0.88, uv.x) * ss(0.0, 0.06, uv.y) * ss(1.0, 0.94, uv.y);
        let facing = ss(0.5, 0.1, rv.z);
        w = edge * facing * touch * ss(1500.0, 900.0, b) * ss(y0 - 0.15, y0 + 0.35, hit_w.y);
    }
    return vec4<f32>(col, w);
}

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0)).xyz;
    let kind = u32(water.params.x + 0.5);
    let bh = u32(water.optics.w + 0.5);
    if kind == 0u {
        var sp = 1.0;
@if(VERTEX_UVS_A) {
        sp = max(v.uv.x, 0.05);
}
        // waves calm down over the shelf so they never climb the beach
        let att = cascade_atten(water.grid.w - terrain_height(wp.xz));
        let K0 = water.casc[0];
        let K1 = water.casc[1];
        let K2 = water.casc[2];
        // (each cascade fades out where the grid is too coarse to carry it)
        let f0 = 1.0 - ss(800.0 / 14.0, 800.0 / 6.0, sp);
        let f1 = 1.0 - ss(151.0 / 14.0, 151.0 / 6.0, sp);
        let f2 = 1.0 - ss(27.7 / 14.0, 27.7 / 6.0, sp);
        // (a sea current drifts the whole wave field)
        let q = wp.xz - water.flowv.xy * (water.flowv.z * globals.time);
        var d = textureSampleLevel(disp0, samp, q * K0.z, max(log2(sp / K0.w), 0.0)).xyz * (f0 * att.x);
        d += textureSampleLevel(disp1, samp, q * K1.z, max(log2(sp / K1.w), 0.0)).xyz * (f1 * att.y);
        d += textureSampleLevel(disp2, samp, q * K2.z, max(log2(sp / K2.w), 0.0)).xyz * (f2 * att.z);
        wp += d;
    } else {
        // tides lift lakes and rivers (the sea's grid is moved as a whole)
        wp.y += water.flowv.w;
        if bh == B_WHIRLPOOL && kind == 1u {
            let r = distance(wp.xz, water.body.xy);
            let rad = water.body.z;
            let k = r / max(rad * 0.22, 1.0);
            wp.y -= min(rad * 0.08, 2.5) * (0.4 + water.params.z) * exp(-k * k);
        }
    }
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_COLORS)
    out.color = v.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

// Non-repeating world-space modulation of the short wind waves (tidewater SeaDetail.js), from the
// tiling value-noise texture (R 8, G 16, B 12, A 32 cells per tile):
//   gusts    cat's paws ~150 m and ~50 m across drifting downwind: rougher, darker water
//   slicks   long glassy bands in light wind, torn apart by gusts
//   windrows thin lanes along the wind in fresh wind, smooth with a trace of foam
// Returns (rough, gust, slick, streak).
fn sea_detail(xz: vec2<f32>, wd: vec2<f32>, t: f32) -> vec4<f32> {
    let p = xz - water.detail.xy;
    let u = water.detail.z;
    let g1 = textureSample(noise_tex, samp, p / 1240.0).r;
    let g2 = textureSample(noise_tex, samp, p / 736.0 + vec2<f32>(t * 0.0009, 0.37)).g;
    let gust = clamp((g1 * 0.62 + g2 * 0.38 - 0.5) * 2.6 * water.detail.w + 0.5, 0.0, 1.0);
    let along = dot(p, wd);
    let across = dot(p, vec2<f32>(-wd.y, wd.x)) + (g2 - 0.5) * 40.0 + (g1 - 0.5) * 90.0;
    // (a few irregular glassy bands, ~30 m wide and hundreds of metres long)
    let sl = textureSample(noise_tex, samp, vec2<f32>(along / 3300.0, across / 420.0)).b;
    let slick = ss(0.6, 0.7, sl) * (1.0 - gust * 0.8) * ss(13.0, 4.0, u);
    // (thin lanes 20-60 m apart that come and go along their length)
    let st = textureSample(noise_tex, samp, vec2<f32>(along / 960.0, across / 72.0) + vec2<f32>(0.13, 0.71)).b;
    let brk = textureSample(noise_tex, samp, vec2<f32>(along / 280.0, across / 80.0) + vec2<f32>(0.51, 0.29)).r;
    let streak = ss(0.66, 0.74, st) * ss(0.45, 0.62, brk) * ss(6.0, 12.0, u);
    let rough = mix(0.5, 1.5, gust) * (1.0 - slick * 0.8) * (1.0 - streak * 0.3);
    return vec4<f32>(rough, gust, slick, streak);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let wp = in.world_position.xyz;
    let kind = u32(water.params.x + 0.5);
    let ocean = kind == 0u;
    let river = kind == 2u;
    let bh = u32(water.optics.w + 0.5);
    let t = globals.time;
    let sea = water.grid.w;

    let to_eye = view.world_position - wp;
    var V = normalize(to_eye);
    let dist = length(to_eye.xz);
    // this pixel's footprint on the water (m): drives every filtering / detail fade, so they hold
    // for any field of view and resolution
    let fp = max(length(fwidth(wp.xz)), 1e-4);

    var flow = normalize(water.wind.xy + vec2<f32>(1e-4));
    var steep = 0.0;
    // drawn lakes fade out over their last metre: the shoreline is the smooth outline
    var edge = 1.0;
@if(VERTEX_COLORS) {
    if river {
        flow = normalize(in.color.xy * 2.0 - 1.0 + vec2<f32>(1e-4));
        steep = in.color.z;
    }
    edge = ss(0.0, 1.0, in.color.w);
}
    var ruv = vec2<f32>(0.5, 0.0);
@if(VERTEX_UVS_A) {
    ruv = in.uv;
}
    let fspeed = water.flowv.z;

    // ---- where the waves are read: q (and the frame that turns its slopes back into the world)
    // rivers: across / along their course, moving downstream (no shear at bends)
    // lakes and the sea: world space drifting with the flow; ripples drift faster with wave speed
    var fx = vec2<f32>(1.0, 0.0);
    var fz = vec2<f32>(0.0, 1.0);
    var sp = wp.xz - water.flowv.xy * (fspeed * t);
    if !ocean {
        sp += flow * ((water.style.z - 1.0) * 1.2 * t);
    }
    if river {
        let w = max(water.body.z, 1.0);
        fx = vec2<f32>(-flow.y, flow.x);
        fz = flow;
        sp = vec2<f32>((ruv.x - 0.5) * w * 1.25, ruv.y * w - t * max(fspeed, 0.15) - t * (water.style.z - 1.0) * 1.2);
    }
    // whirlpool: a swirl around the body's centre, faster near the core (two-phase flow map)
    var whirl = 0.0;
    var wr = 1e4;
    var wrel = vec2<f32>(0.0);
    var sp_b = sp;
    var w_b = 0.0;
    if bh == B_WHIRLPOOL && !ocean {
        wrel = wp.xz - water.body.xy;
        wr = length(wrel);
        let rad = water.body.z;
        let dirr = wrel / max(wr, 1e-3);
        let tang = vec2<f32>(-dirr.y, dirr.x);
        whirl = ss(rad, rad * 0.15, wr);
        let core = rad * 0.1 + 1.0;
        let spd = (0.6 + max(fspeed, 1.0)) * (1.0 + 2.5 * core / (wr + core));
        let vel = (tang - dirr * 0.35) * spd * whirl;
        let period = 5.0;
        let ph0 = fract(t / period);
        let ph1 = fract(t / period + 0.5);
        // (each phase is invisible when it resets: A at ph0 = 0, B half a period later)
        w_b = abs(2.0 * ph0 - 1.0);
        sp_b = sp - vel * (ph1 * period) + vec2<f32>(17.3, 41.9);
        sp = sp - vel * (ph0 * period);
    }

    let ground = terrain_height(wp.xz);
    let depth = max(wp.y - ground, 0.0);

    // ---- lava: its own surface, lit through Bevy PBR, glowing on top
    if bh == B_MOLTEN {
        // rivers: hotter in the middle of the flow and nearer the source; a lake mostly crusted over
        var heat = clamp(depth / 4.0, 0.0, 1.0) * 0.3 + 0.2;
        var q = sp;
        if river {
            heat = (1.0 - abs(ruv.x - 0.5) * 1.7) * 0.75 + 0.25 * (1.0 - smoothstep(0.0, 60.0, ruv.y));
            q = vec2<f32>(sp.x, sp.y * 0.6);
        } else {
            // a lake churns slowly: the crust drifts in a gentle swirl of currents
            q = wp.xz + (vec2<f32>(value_noise2(wp.xz * 0.02 + t * 0.004, 331u), value_noise2(wp.xz * 0.02 - t * 0.004, 337u)) - 0.5) * 12.0;
        }
        let lv = lava(q, t, clamp(heat, 0.0, 1.0), fp);
        let Nl = normalize(vec3<f32>(-lv.slope.x, 1.0, -lv.slope.y));
        pbr_input.material.base_color = vec4<f32>(lv.crust.rgb, 1.0);
        pbr_input.material.perceptual_roughness = lv.crust.a;
        pbr_input.material.metallic = 0.0;
        pbr_input.material.reflectance = vec3<f32>(0.3);
        pbr_input.material.emissive = vec4<f32>(lv.glow * (0.55 + 0.45 * water.emis.a + water.emis.r * 0.3), 0.0);
        pbr_input.N = Nl;
        pbr_input.world_normal = vec3<f32>(0.0, 1.0, 0.0);
        var lout: FragmentOutput;
        lout.color = apply_pbr_lighting(pbr_input);
        // the banks: the flow thins to nothing at its edge
        let bank = scene_color((in.position.xy - view.viewport.xy) / view.viewport.zw);
        let k = ss(0.0, 0.08, depth) * edge;
        lout.color = finish(pbr_input, mix(bank, lout.color.rgb, k), bank * (1.0 - k), vec3<f32>(1.0 - k));
        return lout;
    }
    // per-cascade amplitude in shallow water (the same weights as the vertex stage); lakes and
    // rivers carry no swell and calm down a little over their shallows
    let att = select(vec3<f32>(mix(0.35, 1.0, ss(0.3, 4.0, depth))), cascade_atten(sea - ground), ocean);
    let scale = select(water.params.z, 1.0, ocean);
    let det = select(vec4<f32>(1.0, 0.5, 0.0, 0.0), sea_detail(wp.xz, flow, t), !river);
    let rough_d = det.x;
    let storm = water.body.w;

    // ---- the cascades: slopes, slope variance, foam, the Jacobian
    var c = casc_at(sp);
    if bh == B_WHIRLPOOL && !ocean {
        c = casc_mix(c, casc_at(sp_b), w_b);
    }
    let n0 = c.n0;
    let n1 = c.n1;
    let n2 = c.n2;
    let d0 = c.d0;
    let d1 = c.d1;
    let d2 = c.d2;
    // mean slope variance of the finest cascade (its 1x1 mip): the unresolved capillary roughness
    let n2_mean = textureSampleLevel(norm2, samp, vec2<f32>(0.5), 8.0).z;
    // distance fades hide the tiling; what a faded cascade loses goes into the roughness (LEAN)
    let k0 = select(0.0, 1.0, ocean);
    let k1 = (1.0 - ss(900.0, 3000.0, dist)) * select(mix(0.55, 1.0, storm), 1.0, ocean);
    let k2 = 1.0 - ss(140.0, 520.0, dist);
    // amplitude per cascade: shallow-water attenuation, gusts and slicks on the short waves;
    // rapids churn the short waves
    let churn = select(1.0, 1.5, bh == B_RAPIDS);
    let w0 = att.x * k0 * scale;
    let w1 = att.y * mix(1.0, rough_d, 0.4) * scale;
    let w2 = att.z * rough_d * scale * churn;
    var slope = n0.xy * (k0 * w0) + n1.xy * (k1 * w1) + n2.xy * (k2 * w2);
    // The mirror follows the resolved swell and the middle cascade. The short cascade keeps a
    // restrained share so close water still moves, while capillaries, rain and bubbles stay in
    // the lighting/refraction normal rather than shredding a whole reflected cloud or shoreline.
    var reflection_slope = n0.xy * (k0 * w0) + n1.xy * (k1 * w1) + n2.xy * (k2 * w2 * 0.22);
    var vari = max(n0.z - dot(n0.xy, n0.xy) * k0 * k0, 0.0) * w0 * w0
        + max(n1.z - dot(n1.xy, n1.xy) * k1 * k1, 0.0) * w1 * w1
        + max(n2.z - dot(n2.xy, n2.xy) * k2 * k2, 0.0) * w2 * w2;
    let j_all = 1.0 + (d0.w - 1.0) * k0 * att.x + (d1.w - 1.0) * att.y + (d2.w - 1.0) * att.z;

    // near-field detail: within a few metres a pixel covers less than the finest cascade's texel
    // (10.8 cm), its bilinear facets show in the glint and the surface turns glassy. Re-sample that
    // cascade at smaller rotated tiles (they never line up with it): 6.8 m for the short chop it
    // can't resolve, then 2.3 m and 0.97 m for capillaries (tidewater's scales).
    let near_a = (1.0 - ss(0.04, 0.14, fp)) * rough_d * att.z * scale;
    let near_b = (1.0 - ss(0.012, 0.045, fp)) * rough_d * att.z * scale;
    vari += n2_mean * 0.12 * w2 * w2 * (1.0 - near_b);
    if near_a > 0.002 {
        var nd = near_detail(sp, fp, near_a, near_b);
        if bh == B_WHIRLPOOL && !ocean {
            nd = mix(nd, near_detail(sp_b, fp, near_a, near_b), w_b);
        }
        slope += nd.xy;
        vari += nd.z;
    }
    // the river frame's slopes back into world axes
    slope = fx * slope.x + fz * slope.y;
    reflection_slope = fx * reflection_slope.x + fz * reflection_slope.y;

    // rain pits (a storm rains on its own water)
    let rain = max(water.wind.z, storm * 0.85);
    let near_k = 1.0 - ss(0.05, 0.4, fp);
    let r1 = textureSample(noise_tex, samp, wp.xz / 1.7 + vec2<f32>(t * 0.9, -t * 0.7)).rg - 0.5;
    let r2 = textureSample(noise_tex, samp, wp.xz / 0.6 - vec2<f32>(t * 1.3, t * 0.5)).ba - 0.5;
    slope += (r1 * 0.6 + r2 * 0.4) * 0.5 * rain * near_k;
    vari += 0.02 * rain * near_k;

    // bubbling: rings spreading from bubbles bursting at the surface
    var bub_f = 0.0;
    if bh == B_BUBBLING {
        let bfade = 1.0 - ss(0.08, 0.5, fp);
        if bfade > 0.001 {
            let bp = sp;
            let b1 = bubble_layer(bp, t, 1.5, 11u);
            let b2 = bubble_layer(rot2(bp, 1.3) + vec2<f32>(3.7, 1.9), t * 1.3, 0.7, 23u);
            slope += (b1.xy + b2.xy * 0.7) * bfade;
            bub_f = clamp(b1.z + b2.z * 0.8, 0.0, 1.0) * bfade * ss(0.05, 0.3, depth);
        }
        // far away: a speckle of foam instead of individual rings
        bub_f = max(bub_f, (1.0 - bfade) * 0.12);
    }

    // ---- foam
    let shelf = att.y;
    let foam_k = water.look.x * water.params.w * shelf;
    let caps_k = water.look.y * water.params.w * shelf;
    let streak_k = water.look.z;
    let hs = water.look2.x;
    let h_n = clamp((wp.y - sea + hs * 0.3) / max(hs * 0.85, 0.35), 0.0, 1.2);
    let on_crest = max(ss(0.985, 0.88, d0.w), ss(0.45, 0.95, h_n));
    let gust_f = mix(0.5, 1.5, det.y);
    var aged = (n0.w * foam_k * 0.6 + (n1.w * (0.35 * foam_k + 0.5 * caps_k) + n2.w * caps_k * 0.5) * mix(0.15, 1.0, on_crest)) * k0;
    let patchy = textureSample(noise_tex, samp, sp * water.casc[1].z * 3.0).r * 0.6 + textureSample(noise_tex, samp, sp * water.casc[2].z * 2.0 + 0.5).g * 0.4;
    aged *= ss(0.3, 0.62, patchy + aged * 0.12) * 1.3 * gust_f;
    var fresh = ss(0.38, -0.05, j_all) * foam_k * mix(0.3, 1.0, on_crest) * k0 * gust_f;
    // a storm on a lake or river: whitecaps where the short waves fold
    fresh = max(fresh, ss(0.6, 0.05, 1.0 + (j_all - 1.0) * scale) * storm * (1.0 - k0) * water.params.w);
    // gale streaks: old foam drawn out along the wind
    let wd = flow;
    let wpp = vec2<f32>(-wd.y, wd.x);
    let along = dot(wp.xz, wd) - t * (0.3 + water.look2.z * 0.9);
    let across = dot(wp.xz, wpp);
    let sm = textureSample(noise_tex, samp, vec2<f32>(across / 11.0, along / 190.0)).g * 0.6 + textureSample(noise_tex, samp, vec2<f32>(across / 31.0, along / 470.0) + 0.3).r * 0.5;
    let foamy = clamp(n0.w * 2.5 + n1.w * 1.5 + 0.25, 0.0, 1.0);
    // (broken up along their length: long lines that come and go, not ruled scratches)
    let s_brk = ss(0.4, 0.7, textureSample(noise_tex, samp, vec2<f32>(across / 60.0, along / 90.0) + 0.61).b);
    let streaks = (ss(0.62, 0.86, sm) * streak_k * 0.45 * s_brk * foamy * (1.0 - ss(250.0, 1400.0, dist)) + det.w * 0.12 * foam_k) * k0;
    // white water: the surf on beaches, lapping lake shores, the current on rivers
    // (domain-warped: thresholded value noise alone shows its square cells; rivers read it in
    // their moving frame, the rest drifts slowly)
    let np = select(wp.xz + flow * t * 0.04, sp, river);
    let fwarp = textureSample(noise_tex, samp, np / 3.7).gb - 0.5;
    let foam_noise = textureSample(noise_tex, samp, rot2(np, 0.7) / 9.0 + fwarp * 0.12).r;
    let lace_n = textureSample(noise_tex, samp, rot2(np, 2.1) / 2.3 + fwarp * 0.3 + vec2<f32>(0.41, 0.17)).g;
    var white = 0.0;
    if ocean {
        // the surf: the next wave breaks where the water is about its own height deep and rolls in
        // as a bore of white water over the last third of the cycle, reaching the waterline as
        // its swash starts up the sand; the foam it leaves spreads into lace and dissolves
        let hs = water.look2.x;
        let cyc = surf_cycle(wp.xz, t, hs);
        let c = fract(cyc.x);
        let next = surf_size(floor(cyc.x) + 1.0, wp.xz);
        let brk = clamp(hs * 1.1 * next + 0.3, 0.3, 3.0);
        // (the bore takes the whole wave period to cross the surf zone, ~3 m/s: one is always on
        // its way in)
        let s_left = 1.0 - c;
        let s_here = 0.9 * depth / brk;
        let bore = (1.0 - ss(0.0, 0.06, abs(s_left - s_here))) * ss(brk * 1.15, brk * 0.75, depth);
        // (behind the bore: seconds of foam; after it hit the shore, a fading lace in the shallows)
        let behind = select(0.0, exp(-(s_here - s_left) * 5.0), s_left < s_here) * ss(brk * 1.1, brk * 0.5, depth);
        let after = exp(-c * 3.5) * ss(brk * 0.9, 0.0, depth) * cyc.y;
        let lace = ss(0.35, 0.75, foam_noise * 0.55 + lace_n * 0.45);
        // the backwash leaves a thin, broken bead of foam at the waterline
        let bead = (1.0 - ss(0.02, 0.14, depth)) * ss(0.25, 0.4, c) * ss(0.55, 0.7, lace_n) * 0.6;
        white = (bore * (0.55 + 0.45 * lace) + max(behind, after) * lace * 0.85 + bead) * water.style.y * ss(0.0, 0.08, hs);
    } else if river {
        // the current: foam flecks drifting downstream, white water where it runs shallow or
        // steep, along the banks where it bends
        let spd = clamp(fspeed / 2.5, 0.0, 1.5);
        let flecks = ss(0.72, 0.86, lace_n * 0.6 + foam_noise * 0.4) * (0.25 + 0.5 * spd);
        let shallow = (1.0 - ss(0.15, 0.9, depth)) * ss(0.4, 0.7, foam_noise) * spd;
        let bend = clamp(length(vec2<f32>(length(dpdx(flow)), length(dpdy(flow)))) / max(fp, 1e-3) * 25.0, 0.0, 1.0);
        let banks = (1.0 - ss(0.0, 0.6, depth)) * (0.2 + 0.8 * bend) * spd * ss(0.45, 0.7, lace_n);
        // a steep reach: white water torn into streaks along the current, the water showing
        // between them (more of it white the steeper it runs)
        let st = ss(0.12, 0.55, steep);
        let rs = textureSample(noise_tex, samp, vec2<f32>(sp.x / 2.3, sp.y / 8.5) + 0.27).r * 0.6
            + textureSample(noise_tex, samp, vec2<f32>(sp.x / 0.9, sp.y / 3.1) + 0.61).g * 0.4;
        let torn = ss(0.62 - 0.2 * st, 0.8 - 0.12 * st, rs * 0.7 + foam_noise * 0.35) * (0.5 + 0.45 * st) * st;
        white = max(max(flecks * water.params.w, shallow), max(banks, torn));
        white *= ss(0.0, 0.05, water.params.w + water.style.y);
    } else {
        // a lake laps its shore: a faint, broken line of bubbles right at the water's edge
        white = (1.0 - ss(0.02, 0.18, depth)) * ss(0.6, 0.8, lace_n) * 0.45 * water.style.y;
    }
    if bh == B_RAPIDS {
        // white water torn into streaks along the current
        let rs = textureSample(noise_tex, samp, vec2<f32>(sp.x / 2.3, sp.y / 8.5) + 0.27).r * 0.6
            + textureSample(noise_tex, samp, vec2<f32>(sp.x / 0.9, sp.y / 3.1) + 0.61).g * 0.4;
        white = max(white, ss(0.42, 0.72, rs * 0.75 + foam_noise * 0.35) * (0.55 + 0.45 * water.params.w));
    }
    if bh == B_WHIRLPOOL && !ocean {
        // spiral arms of foam winding into the core
        let th = atan2(wrel.y, wrel.x);
        let arms = sin(th * 3.0 + log(wr + 1.0) * 5.0 + t * 1.8) * 0.5 + 0.5;
        let core = ss(water.body.z * 0.12 + 1.2, 0.0, wr);
        white = max(white, (ss(0.6, 0.95, arms + (foam_noise - 0.5) * 0.5) * 0.7 + core * 0.6) * whirl * water.params.w);
    }
    white = max(white, bub_f * 0.9);
    // foam: an irregular bubbly mat (the foam texture) thresholded by the local coverage, so foam
    // grows, tears into lace and dissolves; the averaged coverage once a pixel spans the pattern
    let a = clamp(aged + streaks, 0.0, 1.0);
    let aw = clamp(white, 0.0, 1.0);
    let cov = clamp(max(a, aw) + fresh * 0.9, 0.0, 1.0);
    // (a river's foam is drawn out along its current)
    let fuv = select(sp * 0.09, vec2<f32>(sp.x * 0.09, sp.y * 0.03), river);
    let p1 = textureSample(foam_tex, samp, fuv);
    let p2 = textureSample(foam_tex, samp, rot2(fuv, 0.6435) * 2.37 + vec2<f32>(0.31, 0.77));
    var pattern = p1.x * 0.62 + p2.x * 0.38;
    var p1y = p1.y;
    if bh == B_WHIRLPOOL && !ocean {
        let fuv_b = sp_b * 0.09;
        let q1 = textureSample(foam_tex, samp, fuv_b);
        let q2 = textureSample(foam_tex, samp, rot2(fuv_b, 0.6435) * 2.37 + vec2<f32>(0.31, 0.77));
        pattern = mix(pattern, q1.x * 0.62 + q2.x * 0.38, w_b);
        p1y = mix(p1y, q1.y, w_b);
    }
    let thresh = 1.05 - cov * 1.1;
    // (a river's churned foam has softer edges than the sea's lace)
    let soft = 0.06 + fp * 0.1 + select(0.0, 0.07, river);
    let lace = ss(thresh - soft, thresh + soft, pattern) * (p1y * 0.25 + 0.8);
    let cover = clamp(mix(lace, cov * 0.85, ss(0.15, 1.2, fp)), 0.0, 0.97) * edge;

    // ---- seen from below: the camera is under this surface (the sea's waves are only looked up
    // when the camera is anywhere near sea level)
    var cam_surface = wp.y;
    if ocean {
        cam_surface = -1e9;
        if view.world_position.y < water.grid.w + 25.0 {
            cam_surface = sea_surface_at(view.world_position.xz);
        }
    }
    if view.world_position.y < cam_surface - 0.03 {
        let u = underside(wp, V, normalize(vec3<f32>(-slope.x, 1.0, -slope.y)), cover);
        var under: FragmentOutput;
        under.color = finish(pbr_input, u.col, u.scene * u.seen, vec3<f32>(u.seen));
        return under;
    }
    // (a swimmer's eye in a trough sees nearby crests from below their mean plane: they are still
    // water seen from the air, at a grazing angle)
    V = normalize(vec3<f32>(V.x, max(V.y, 0.02), V.z));

    // ---- normal and roughness
    var N = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));
    // Pixel-sized capillaries still bend refraction and make the sun glitter, but reflections use
    // their filtered surface normal. Far water averages even the middle chop into long coherent
    // bands instead of a field of independently tilted environment-map samples.
    let reflect_fade = mix(1.0, 0.42, ss(90.0, 700.0, dist));
    let Nr = normalize(vec3<f32>(-reflection_slope.x * reflect_fade, 1.0, -reflection_slope.y * reflect_fade));
    let ndv0 = dot(N, V);
    if ndv0 < 0.03 {
        N = normalize(N + V * (0.03 - ndv0));
    }
    let ndv = clamp(dot(Nr, V), 1e-3, 1.0);
    // the style's surface roughness frosts the mirror
    let r_min = water.style.x * 0.6;
    let a2 = clamp(max(vari * 0.9 + 0.0008 + 0.004 * ss(300.0, 6000.0, dist) + rain * 0.006, r_min * r_min * r_min * r_min), 0.0005, 0.6);
    let perceptual = sqrt(sqrt(a2));

    // ---- the water column along the refracted view ray (Snell), through the terrain heightfield
    // (two refinements, tidewater WaterMaterial.js): at grazing angles the straight screen ray
    // would overestimate the water path ~10x
    let Tr = refract(-V, N, 1.0 / IOR);
    let Tv = normalize(vec3<f32>(Tr.x, min(Tr.y, -0.08), Tr.z));
    let tdown = max(-Tv.y, 0.04);
    let L0 = depth / tdown;
    var path = L0;
    if L0 < 100.0 {
        let L1 = max(wp.y - terrain_height(wp.xz + Tv.xz * min(L0, 200.0)), 0.0) / tdown;
        path = max(wp.y - terrain_height(wp.xz + Tv.xz * min(0.5 * (L0 + L1), 200.0)), 0.0) / tdown;
    }
    path = min(path, 400.0);

    // ---- what lies behind: the opaque scene where the refracted ray ends (seabed, rocks, fish).
    // A lookup is only used if it lies behind this surface point (not a post or hull in front of
    // it); else the pixel straight behind. Objects in front of the seabed shorten the path.
    let vp = view.viewport;
    let suv = (in.position.xy - vp.xy) / vp.zw;
    let dsize = vec2<f32>(textureDimensions(scene_depth));
    let surf_z = -position_world_to_view(wp).z;
    let p_end = wp + Tv * min(path, 60.0);
    let c_end = position_world_to_clip(p_end);
    let ruv2 = vec2<f32>(c_end.x / c_end.w * 0.5 + 0.5, 0.5 - c_end.y / c_end.w * 0.5);
    var uv = suv;
    var d_s = textureLoad(scene_depth, vec2<i32>(clamp(suv * dsize, vec2<f32>(0.0), dsize - 1.0)), 0);
    if all(ruv2 > vec2<f32>(0.002)) && all(ruv2 < vec2<f32>(0.998)) {
        let d_r = textureLoad(scene_depth, vec2<i32>(ruv2 * dsize), 0);
        // (reverse-z: view distance = near / depth; 0 is the sky, infinitely far)
        if d_r < 1e-7 || view.clip_from_view[3][2] / d_r > surf_z + 0.05 {
            uv = ruv2;
            d_s = d_r;
        }
    }
    var path_v = path;
    var dq = depth;
    var q = wp + Tv * path;
    var sky_behind = d_s < 1e-7;
    if !sky_behind {
        let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d_s, 1.0);
        let qh = view.world_from_clip * ndc;
        q = qh.xyz / qh.w;
        path_v = clamp(min(path, distance(q, wp)), 0.0, 400.0);
        dq = max(wp.y - q.y, 0.0);
    } else {
        path_v = 400.0;
    }
    let scene = scene_color(uv);

    let sig_t = water.extinct.rgb;
    let trans = select(exp(-sig_t * path_v), vec3<f32>(0.0), sky_behind);

    // light scattered towards the eye inside the water (single scattering of the refracted sun and
    // the sky, analytic along the path): saturates to the deep colour, sooner for murky water
    let L = normalize(water.sun.xyz);
    // moving cloud shadows (as on the terrain): they dim the glint and the light in the water.
    // (The baked map is the cover field `noise.wesl::cloud_density` thresholds, 32 m a texel: far
    // finer than the shadow's own softness, and a single lookup instead of the noise's 13 hashed
    // cells per pixel.)
    var cloud_sh = 1.0;
    if water.clouds.y > 0.001 {
        let cloud_p = wp.xz + L.xz * ((1800.0 - wp.y) / max(L.y, 0.05));
        let cloud = cloud_cover_at(cloud_p);
        cloud_sh = 1.0 - cloud * water.clouds.y;
    }
    let sun = water.sun_col.rgb * water.sun.w * cloud_sh;
    // The downwelling fill follows the local part of the same cloud field. This is deliberately
    // gentler than the direct-light shadow: the sky dome remains a broad source of light.
    let sky_irr = water.sky.rgb * (1.0 - cloud_cover_at(wp.xz) * 0.28);
    let Ls = normalize(vec3<f32>(L.x / IOR, max(sqrt(max(1.0 - (1.0 - L.y * L.y) / (IOR * IOR), 0.0)), 0.1), L.z / IOR));
    let mu_s = max(Ls.y, 0.1);
    let mu_v = max(-Tv.y, 0.15);
    let k_sun = 1.0 + mu_v / mu_s;
    let k_amb = 1.0 + mu_v / 0.75;
    let phase = 1.0 + 0.6 * ss(-0.6, 0.2, dot(Tv, Ls));
    // sun entering the water: Fresnel-transmitted, spread over the refracted beam
    let sun_in = sun * (0.98 - 0.98 * pow(1.0 - max(L.y, 0.0), 5.0)) * ss(0.0, 0.12, L.y) * max(L.y, 0.0) / mu_s;
    let alb = water.inscat.rgb;
    let in_sun = sun_in * alb * (2.7 * phase) * (1.0 - exp(-sig_t * k_sun * path_v)) / k_sun;
    let in_amb = sky_irr * alb * 3.7 * (1.0 - exp(-sig_t * k_amb * path_v)) / k_amb;
    // the light that lit what we see down there came through the water too: the sun along the
    // refracted beam, the sky from all around (UnderwaterLighting.js)
    let sun_w = dot(sun_in, vec3<f32>(0.3, 0.5, 0.2)) / max(dot(sun_in + sky_irr * 1.5, vec3<f32>(0.3, 0.5, 0.2)), 1e-4);
    // caustics: the waves focus the sunlight into a moving net on whatever lies down there. Looked
    // up where the refracted sun ray through that point entered the water, between the 1.5 m and
    // 4 m layers (the map is read at half its tile: half-size waves), blurrier with depth (and by
    // footprint, so far floors don't crawl)
    let entry = q.xz + Ls.xz * (dq / Ls.y) - select(wp.xz - sp, vec2<f32>(0.0), river);
    let c_lod = clamp(max(dq * 0.15, log2(fp * 256.0 / 13.85 + 1e-4)), 0.0, 5.0);
    let cs = textureSampleLevel(caustic_tex, samp, rot2(entry, 0.37) / 13.85, c_lod);
    let caus = mix(cs.r, cs.g, ss(1.5, 4.0, dq));
    let c_amt = ss(0.05, 0.6, dq) * exp(-0.05 * dq) * att.z * mix(1.0, rough_d, 0.5) * min(scale, 1.5) * (1.0 - cover);
    let lit = mix(exp(-sig_t * dq * 1.2) * 0.9, exp(-sig_t * dq / mu_s) * max(1.0 + (caus - 1.0) * c_amt, 0.0), sun_w);

    // the low sun shining through thin crests; bubbles under foam
    let scatter = water.scatter.rgb;
    let lh = normalize(vec3<f32>(L.x, min(L.y, 0.35), L.z));
    let through = pow(clamp(dot(lh, -V), 0.0, 1.0), 4.0) * pow(clamp(0.5 - 0.5 * dot(L, N), 0.0, 1.0), 2.0);
    let big = 0.35 + 0.65 * ss(0.5, 4.0, hs);
    let sss = scatter * sun * (through * 3.2 * h_n * h_n * big + max(dot(N, L), 0.0) * 0.06 * h_n) * (1.0 - ss(400.0, 3000.0, dist) * 0.7) * select(0.4, 1.0, ocean) * ss(0.2, 1.5, depth);
    let light = sky_irr * 0.85 + sun * max(L.y, 0.0) * 0.35;
    let bubbles = (scatter * 1.2 + water.deep.rgb * 0.5 + vec3<f32>(0.01)) * light * 0.22 * clamp(aged * 0.5 + fresh * 0.6 + bub_f * 0.5, 0.0, 1.0);

    // ---- reflection and glint through Bevy PBR (sky env map + sun GGX with shadows); the foam is
    // its only diffuse part; liquid metal reflects in its own colour
    let metal = water.optics.y * (1.0 - cover);
    let rough = mix(perceptual, 0.92, cover);
    // (murky water's foam is stained by it: beige on a muddy river, white on clear water)
    let murk = 1.0 - ss(0.6, 3.0, water.params.y);
    let foam_rgb = mix(vec3<f32>(0.8), min(water.shallow.rgb * 2.2, vec3<f32>(0.8)), 0.6 * murk);
    pbr_input.material.base_color = vec4<f32>(mix(foam_rgb * cover, water.shallow.rgb, metal), 1.0);
    pbr_input.material.perceptual_roughness = rough;
    pbr_input.material.metallic = metal;
    pbr_input.material.reflectance = vec3<f32>(0.35);
    pbr_input.material.specular_transmission = 0.0;
    // the uniform ambient fill (night readability) lights the foam but is not mirrored by the
    // open water as a bright sheet at grazing angles: the sea reflects the actual (dark) sky
    pbr_input.diffuse_occlusion = vec3<f32>(mix(0.12, 1.0, ss(0.0, 0.3, cover)));
    // Bevy uses one normal for its environment and direct lobes. Feed it the reflection normal;
    // the detailed `N` above still controls refraction, water depth and the explicit glint.
    pbr_input.N = Nr;
    pbr_input.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    pbr_input.directional_shadow_factor = cloud_sh;

    // energy the surface reflects (the env-map lobe's directional albedo, F0 = 0.02, or the metal's
    // colour); the rest enters the water
    let fab = F_AB(rough, ndv);
    let fres_e = clamp(0.0196 * fab.x + fab.y, 0.0, 1.0);
    let fres_rgb = mix(vec3<f32>(fres_e), clamp(water.shallow.rgb * fab.x + fab.y, vec3<f32>(0.0), vec3<f32>(1.0)), metal);
    let R = reflect(-V, Nr);
    let detail_R = reflect(-V, N);

    // thin-film iridescence: a rainbow that shifts with the viewing angle and the film's thickness
    var film = vec3<f32>(1.0);
    let irid = water.optics.z;
    if irid > 0.001 {
        let th = textureSample(noise_tex, samp, sp / 19.0 + vec2<f32>(t * 0.004, t * 0.003)).r;
        let ph = (th * 1.8 + (1.0 - ndv) * 1.6 + (slope.x - slope.y) * 0.6) * 6.2832;
        let rainbow = 0.5 + 0.5 * cos(vec3<f32>(ph, ph - 2.094, ph - 4.189));
        film = mix(vec3<f32>(1.0), rainbow * 1.9 + 0.1, irid);
    }

    // the shore, trees and hills mirrored from the screen where the reflected ray finds them (it
    // replaces that much of the sky's environment-map reflection); only rays near the horizon
    // can hit anything, and looking down the reflection is too faint to matter
    // (rough or distant water blurs the mirror into a smear that one ray per pixel can't resolve:
    // it would sparkle, so it fades to the environment map and the march is skipped)
    // (the march costs per step: where the mirror's weight is faint a coarser march finds the
    // same shore, and where it could add at most 2 % of what it mirrors it is skipped)
    var ssr = vec4<f32>(0.0);
    let quality = u32(water.style.w + 0.5);
    let ssr_fade = ss(0.42, 0.2, rough) * ss(600.0, 250.0, length(to_eye));
    let ssr_w = max(fres_e, metal) * ssr_fade * (1.0 - cover);
    if quality > 0u && R.y < 0.45 && max(fres_e, metal) > 0.04 && cover < 0.9 && ssr_w > 0.02 {
        let rv = direction_world_to_view(R);
        if rv.z < 0.5 {
            let full = quality > 1u && ssr_w > 0.12;
            let steps = select(10, 16, full);
            let refine = select(3, 5, full);
            let blur = select(0.0, water.style.x, quality > 1u);
            ssr = water_ssr(position_world_to_view(wp), rv, wp.y, R.y, steps, refine, blur);
            ssr.w *= ssr_fade;
        }
    }
    // A clipped, offscreen view supplies coherent terrain and architecture beyond the current
    // screen. Screen-space reflection remains authoritative where it has a local hit.
    let planar = planar_reflection(wp, reflection_slope, dist, rough, R.y);
    // The bounded blur above carries the capture across normal variance. Reserve the fallback
    // only for truly frosted/stormy water, where even the filtered reflection stops reading.
    let planar_w = planar.w * ss(0.72, 0.18, rough) * (1.0 - ssr.w) * (1.0 - cover);
    pbr_input.specular_occlusion = 1.0 - min(ssr.w + planar_w, 1.0);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    // liquid metal: its colour saturates what it mirrors (a grey sky in gold reads as gold)
    let m_tint = mix(vec3<f32>(1.0), water.shallow.rgb / max(dot(water.shallow.rgb, vec3<f32>(0.3, 0.5, 0.2)), 0.05), metal * 0.5);
    // (the mirrored scene, and below the scene seen through the water: already fogged, `kept` out
    // of the fog with their share `kept_w`)
    var kept_w = fres_rgb * (ssr.w * (1.0 - cover) + planar_w) * film * m_tint;
    var kept = (ssr.rgb * ssr.w * (1.0 - cover) + planar.rgb * planar_w) * fres_rgb * film * m_tint;
    out.color = vec4<f32>(out.color.rgb * film * m_tint + kept, out.color.a);
    let into = (1.0 - fres_e) * (1.0 - cover) * (1.0 - metal);
    let tw = textureSample(noise_tex, samp, wp.xz * 0.7 + vec2<f32>(t * 0.37, -t * 0.29)).a;
    let sparkle = pow(max(dot(detail_R, L), 0.0), 1600.0) * ss(0.6, 0.85, tw) * 6.0 * (1.0 - ss(0.02, 0.12, fp));
    // transparency: blend the water column with an opaque, infinitely deep body
    let opaque_body = sun_in * alb * (2.7 * phase) / k_sun + sky_irr * alb * 3.7 / k_amb;
    let T = water.optics.x;
    let column = (scene * lit * trans + in_sun + in_amb) * T + opaque_body * (1.0 - T);
    let film_tint = mix(vec3<f32>(1.0), 0.7 + 0.3 * film, irid * 0.5);
    var water_col = (column + sss + bubbles) * film_tint;
    let seen = lit * trans * T * film_tint * into;
    kept += scene * seen;
    kept_w += seen;
    // inner glow: slow veins of light drifting through the water
    if dot(water.emis.rgb, vec3<f32>(1.0)) > 0.0005 {
        let e1 = textureSample(noise_tex, samp, sp / 31.0 + vec2<f32>(t * 0.006, 0.0)).g;
        let e2 = textureSample(noise_tex, samp, rot2(sp, 1.1) / 11.0 - vec2<f32>(0.0, t * 0.013)).b;
        let ridge = 1.0 - abs(2.0 * (e1 * 0.6 + e2 * 0.4) - 1.0);
        let veins = pow(ridge, 6.0);
        let pulse = 0.85 + 0.15 * sin(t * 0.8 + e1 * 9.0);
        water_col += water.emis.rgb * (0.35 + 1.5 * veins) * pulse * ss(0.0, 0.8, depth);
    }
    var col = out.color.rgb + water_col * into + sun * sparkle * water.sky.w * (1.0 - cover);
    // bioluminescence: breaking crests, foam, bubbles and stray sparkles light up in the dark
    let glow_on = dot(water.glow.rgb, vec3<f32>(1.0)) * water.glow.w;
    if glow_on > 0.0005 {
        let g1 = textureSample(noise_tex, samp, wp.xz * 0.53 + vec2<f32>(t * 0.11, -t * 0.07)).a;
        let g2 = textureSample(noise_tex, samp, wp.xz * 1.9 - vec2<f32>(t * 0.21, t * 0.13)).r;
        // (the plankton light up where the water is disturbed: breaking crests, fresh foam, the
        // shore break, bubbles and rain, not the open swell)
        let agit = clamp(fresh * 1.8 + cover * 0.9 + aw * 0.9 + bub_f * 2.0 + ss(0.95, 1.15, h_n) * 0.25 * k0 + rain * 0.25 + whirl * 0.3, 0.0, 1.0);
        let g3 = textureSample(noise_tex, samp, wp.xz * 4.7 + vec2<f32>(-t * 0.31, t * 0.23)).g;
        let spark = ss(0.8, 0.93, g1 * 0.4 + g2 * 0.3 + g3 * 0.3) * (1.0 - ss(0.05, 0.6, fp));
        col += water.glow.rgb * water.glow.w * (agit * (0.35 + 1.1 * g2 * g3) + spark * (0.4 + 2.5 * agit) + 0.004) * edge;
    }
    // a whirlpool's eye is dark
    if bh == B_WHIRLPOOL && !ocean {
        let eye_dark = 1.0 - 0.45 * ss(water.body.z * 0.2 + 2.0, 0.0, wr) * whirl;
        col *= eye_dark;
        kept *= eye_dark;
        kept_w *= eye_dark;
    }
    // the waterline: the film thins to nothing on the sand (and at a drawn lake's outline)
    let shore = ss(0.0, 0.05, depth) * edge;
    col = mix(scene, col, shore);
    kept = mix(scene, kept, shore);
    kept_w = mix(vec3<f32>(1.0), kept_w, shore);

    out.color = finish(pbr_input, col, kept, kept_w);
    return out;
}
