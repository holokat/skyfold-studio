// Terrain material: procedural layered shading on top of Bevy PBR lighting.
//
// Weights per layer = automatic style rules (slope / altitude / beaches / snow line), overridden
// by hand-painted splat weights. Each layer is shaded procedurally in world space (no tiling),
// blended by height for natural transitions, and bump-mapped with screen-space derivatives.
// Ground-level realism on top (visuals module): painted borders read through a wandering warp
// and interleaved by noise (ecotones), snow that sheds off steep faces and patches out at its
// edge, and the shore map's nearest water: wet sand, swash and surf on beaches, lapped lake
// shores, damp gravelly river banks and scorched, glowing ground at the rim of lava.
//
// Cost (this is most of the main pass): only the six strongest layers of a pixel are shaded;
// texture reads the height blend can't give any weight are skipped (the anti-tiling copy where
// the first copy stands high, triplanar sides on gentle slopes, the fine copy beyond 300 m where
// the macro copy has taken over), noises only feed what they change, and cloud shadows read the
// clouds' baked cover map. `terrain_reference.wesl` is the shading before this (A/B: `WB_TERRAIN_REF=1`).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};
import super::noise::{value_noise2, value_noise3, fbm2, fbm3, voronoi2, srgb, cloud_density, cloud_cover, CLOUD_TILE};
import super::shore::{surf_cycle, swash_front, run_up};
import super::rain::{puddle_amount, rain_ripples};
import super::canopy_shade::canopy_sky;

struct TerrainParams {
    grid: vec4<f32>,
    style: vec4<f32>,
    style2: vec4<f32>,
    brush: vec4<f32>,
    brush_color: vec4<f32>,
    overlay: vec4<f32>,
    sun: vec4<f32>,
    clouds: vec4<f32>,
    tex: vec4<f32>,
    layer_tile: array<vec4<f32>, 3>,
    water: vec4<f32>,
    zone_colors: array<vec4<f32>, 64>,
    layer_overlay: array<vec4<f32>, 12>,
    // x: generic overlay image opacity, y: rain puddle fill, z: rain falling (ripples), w: unused
    overlay_image: vec4<f32>,
    // x: sea wave height (significant, m), y: shore foam amount, zw: wind direction (xz)
    visuals: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> terrain: TerrainParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var normal_map: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var linear_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var splat0: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var splat1: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var splat2: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var height_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(107) var zone_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(108) var albedo_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(109) var array_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(110) var detail_array: texture_2d_array<f32>;
// generic overlay image over the whole terrain (analysis heatmaps, spawn regions…)
@group(constants::MATERIAL_BIND_GROUP) @binding(111) var overlay_tex: texture_2d<f32>;
// nearest water per spot (visuals/shore.rs): level, distance to its edge (m), kind, 1
@group(constants::MATERIAL_BIND_GROUP) @binding(112) var shore_tex: texture_2d<f32>;
// the volumetric clouds' baked cover field over one CLOUD_TILE (clouds.rs: `CloudCoverMap`);
// builds without it (CLOUD_COVER_MAP unset) evaluate the cover per pixel
@if(CLOUD_COVER_MAP) {
@group(constants::MATERIAL_BIND_GROUP) @binding(113) var cloud_cover_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(114) var cloud_sampler: sampler;
}

const LAYERS: u32 = 12u;
// layers shaded per pixel (the strongest: four left a visible difference where paths, grass,
// gullies and interleaved borders meet; six match the full twelve)
const TOP: u32 = 6u;

struct LayerSample {
    albedo: vec3<f32>,
    rough: f32,
    height: f32,
    // world-space normal perturbation (textured path) and ambient occlusion
    pert: vec3<f32>,
    ao: f32,
}

fn tile_of(k: u32) -> f32 {
    return terrain.layer_tile[k / 4u][k % 4u];
}

struct Tex {
    a: vec4<f32>,
    d: vec4<f32>,
}

fn sample_arrays(k: u32, uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>) -> Tex {
    var o: Tex;
    // art-style texture softening: widen the footprint = sample blurrier mips (hand-painted look)
    let b = exp2(terrain.tex.w);
    o.a = textureSampleGrad(albedo_array, array_sampler, uv, i32(k), gx * b, gy * b);
    o.d = textureSampleGrad(detail_array, array_sampler, uv, i32(k), gx * b, gy * b);
    return o;
}

fn rot2(v: vec2<f32>, c: f32, s: f32) -> vec2<f32> {
    return vec2<f32>(v.x * c - v.y * s, v.x * s + v.y * c);
}

// Top-down projection plus a second, rotated copy. Up close the second copy is only slightly
// larger and blended by noise (hides repetition); with distance it grows to a 7x macro scale and
// takes over, so far terrain keeps visible structure instead of averaging to a flat colour.
// A copy the blend below can't give any weight isn't read: the second one where the first
// stands at least 0.2 + 2 amt high (heights are 0..1, so the second can't rise 0.2 above it),
// the first one (`fine` false) beyond 300 m, where it is a far-off mip near the layer's mean
// height that the macro copy all but always outgrows.
fn sample_top(k: u32, wp: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, anti: f32, far: f32, fine: bool) -> Tex {
    let tile = tile_of(k);
    let amt = mix(anti * terrain.tex.y, 0.8, far);
    var t1: Tex;
    if fine {
        t1 = sample_arrays(k, wp.xz / tile, dpx.xz / tile, dpy.xz / tile);
        t1.d = vec4<f32>(t1.d.xy * 2.0 - 1.0, t1.d.zw);
        if amt <= 0.01 || t1.a.a >= 0.2 + 2.0 * amt {
            return t1;
        }
    }
    let c = 0.8090;
    let s = 0.5878;
    let sc = tile * mix(1.618, 7.0, far);
    var t2 = sample_arrays(k, rot2(wp.xz, c, s) / sc + vec2<f32>(0.37, 0.71), rot2(dpx.xz, c, s) / sc, rot2(dpy.xz, c, s) / sc);
    let n2 = rot2(t2.d.xy * 2.0 - 1.0, c, -s);
    t2.d = vec4<f32>(n2, t2.d.zw);
    if !fine {
        return t2;
    }
    // height-aware blend: wherever one copy's pebbles / blades stand higher, that copy wins, so
    // photo textures keep their contrast instead of averaging two copies into a blur
    let h1 = t1.a.a + 1.0 - amt;
    let h2 = t2.a.a + amt;
    let top = max(h1, h2) - 0.2;
    let w1 = max(h1 - top, 0.0);
    let w2 = max(h2 - top, 0.0);
    let f = w2 / max(w1 + w2, 1e-4);
    var o: Tex;
    o.a = mix(t1.a, t2.a, f);
    o.d = mix(t1.d, t2.d, f);
    return o;
}

fn tex_layer(k: u32, wp: vec3<f32>, n: vec3<f32>, dpx: vec3<f32>, dpy: vec3<f32>, anti: f32, far: f32, fine: bool) -> LayerSample {
    var o: LayerSample;
    // rock layers are projected on three axes so cliffs don't stretch; a projection with under
    // 2 % of the weight is dropped (the others take its share): gentle slopes read only the top
    // one, cliffs facing along x or z one side
    if k == 6u || k == 7u {
        let tile = tile_of(k);
        var bw = pow(abs(n), vec3<f32>(4.0));
        bw /= (bw.x + bw.y + bw.z);
        bw = max(bw - 0.02, vec3<f32>(0.0));
        bw /= (bw.x + bw.y + bw.z);
        var a = vec4<f32>(0.0);
        var rough = 0.0;
        var ao = 0.0;
        var pert = vec3<f32>(0.0);
        if bw.y > 0.0 {
            let ty = sample_top(k, wp, dpx, dpy, anti, far, fine);
            a += ty.a * bw.y;
            rough += ty.d.z * bw.y;
            ao += ty.d.w * bw.y;
            pert += vec3<f32>(ty.d.x, 0.0, ty.d.y) * bw.y;
        }
        if bw.x > 0.0 {
            let tx = sample_arrays(k, wp.zy / tile, dpx.zy / tile, dpy.zy / tile);
            let nx = tx.d.xy * 2.0 - 1.0;
            a += tx.a * bw.x;
            rough += tx.d.z * bw.x;
            ao += tx.d.w * bw.x;
            pert += vec3<f32>(0.0, nx.y, nx.x) * bw.x;
        }
        if bw.z > 0.0 {
            let tz = sample_arrays(k, wp.xy / tile, dpx.xy / tile, dpy.xy / tile);
            let nz = tz.d.xy * 2.0 - 1.0;
            a += tz.a * bw.z;
            rough += tz.d.z * bw.z;
            ao += tz.d.w * bw.z;
            pert += vec3<f32>(nz.x, nz.y, 0.0) * bw.z;
        }
        // layered strata on cliff faces: beds of slightly different tone stacked up the wall,
        // wandering a little in height (canyonlands; `overlay_image.w` is 0 in ordinary worlds)
        let sb = terrain.overlay_image.w;
        if k == 6u && sb > 0.0 {
            let side = 1.0 - bw.y;
            let y = wp.y + (value_noise2(wp.xz * 0.012, 61u) - 0.5) * 9.0;
            let bed = smoothstep(0.25, 0.75, value_noise2(vec2<f32>(y * 0.28, 7.5), 67u));
            let thin = value_noise2(vec2<f32>(y * 1.7, 3.5), 71u);
            let tint = mix(vec3<f32>(0.68, 0.63, 0.61), vec3<f32>(1.22, 1.04, 0.84), bed) * mix(0.86, 1.1, thin);
            a = vec4<f32>(a.rgb * mix(vec3<f32>(1.0), tint, side * sb), a.a);
        }
        o.albedo = a.rgb;
        o.height = a.a;
        o.rough = rough;
        o.ao = ao;
        o.pert = pert;
        return o;
    }
    let t = sample_top(k, wp, dpx, dpy, anti, far, fine);
    o.albedo = t.a.rgb;
    o.height = t.a.a;
    o.rough = t.d.z;
    o.ao = t.d.w;
    o.pert = vec3<f32>(t.d.x, 0.0, t.d.y);
    return o;
}

// Mirror of `terrain::auto_weights` (Rust). Keep in sync!
fn dry_noise(xz: vec2<f32>) -> f32 {
    return value_noise2(xz * 0.0031, 41u) * 0.55 + value_noise2(xz * 0.0113, 43u) * 0.3 + value_noise2(xz * 0.041, 47u) * 0.15 - 0.5;
}

fn auto_weights(h: f32, ny: f32, macro_n: f32, mid_n: f32, dry_n: f32) -> array<f32, 12> {
    let sea = terrain.grid.w;
    let snow_line = terrain.style.x - terrain.style2.w;
    let snow_blend = terrain.style.y;
    let rs = terrain.style.z;
    let beach_h = terrain.style.w;
    let dryness = terrain.style2.x;

    let slope = 1.0 - ny;
    let alt = h - sea;
    let dry = clamp(smoothstep(0.2, 0.55, dry_n * 1.6 + dryness - 0.25 + alt / 900.0), 0.0, 1.0);
    var grass = 1.0 - dry;
    var dry_grass = dry;
    let dirt = smoothstep(rs * 0.45, rs * 0.85, slope + mid_n * 0.06);
    let rock = smoothstep(rs * 0.8, rs * 1.15, slope + mid_n * 0.05);
    let beach = 1.0 - smoothstep(beach_h - 1.0, beach_h + 1.5, alt + mid_n * 2.5);
    let under = 1.0 - smoothstep(-6.0, -1.5, alt);
    let snow = smoothstep(snow_line - snow_blend, snow_line + snow_blend, alt + macro_n * 60.0 + mid_n * 25.0)
        * (1.0 - smoothstep(0.62, 0.85, slope));

    var dirt_w = dirt * (1.0 - rock);
    grass *= 1.0 - dirt;
    dry_grass *= 1.0 - dirt;
    var rock_w = rock;
    grass *= 1.0 - rock;
    dry_grass *= 1.0 - rock;
    var sand = beach * (1.0 - rock * 0.7);
    grass *= 1.0 - sand;
    dry_grass *= 1.0 - sand;
    dirt_w *= 1.0 - sand;
    rock_w *= 1.0 - sand;
    let gravel = under * smoothstep(0.0, 0.3, mid_n + 0.1) * 0.6;
    sand = max(sand, under) * (1.0 - gravel);

    let keep = 1.0 - snow;
    var w = array<f32, 12>(
        grass * keep, dry_grass * keep, 0.0, dirt_w * keep, 0.0, gravel * keep,
        rock_w * keep, 0.0, sand * keep, snow, 0.0, 0.0
    );
    var s = 0.0;
    for (var k = 0u; k < LAYERS; k++) { s += w[k]; }
    if s > 1e-4 {
        for (var k = 0u; k < LAYERS; k++) { w[k] /= s; }
    } else {
        w[0] = 1.0;
    }
    return w;
}

// Where rain gathers (normal map alpha: flow map x wet-gully strength): dry grass greens up along
// swales, the banks of the wettest lines turn to dark damp soil (moss is added in the shading),
// and only the narrow, gentle bed of a real stream gets gravel; applied to the final (painted)
// weights. Mirrors `terrain::flow_weights`.
fn flow_weights(w_in: array<f32, 12>, wet: f32, ny: f32, mid_n: f32) -> array<f32, 12> {
    var w = w_in;
    if wet <= 0.01 {
        return w;
    }
    let green = smoothstep(0.25, 0.5, wet) * 0.85;
    let moved = w[1] * green;
    w[1] -= moved;
    w[0] += moved;
    let bed = smoothstep(0.86, 0.97, wet + mid_n * 0.08) * smoothstep(0.88, 0.97, ny);
    // (damp soil on the gentler banks: on steep ones the top-down soil would smear, they only darken)
    let bank = smoothstep(0.5, 0.8, wet + mid_n * 0.15) * (1.0 - bed) * smoothstep(0.78, 0.9, ny);
    let soil = bank * 0.45;
    let stones = bed * 0.75;
    let veg = w[0] + w[1] + w[2];
    let k = 1.0 - soil - stones;
    w[0] *= k;
    w[1] *= k;
    w[2] *= k;
    w[4] += veg * soil;
    w[5] += veg * stones;
    return w;
}

// Ecotones: wherever several grounds share a spot (a biome border, a feathered paint stroke, the
// slope rules' own transitions) each takes its own wandering share at three scales (~45, ~12 and
// ~3 m), so the height blend breaks the transition into tongues, islands and interleaved patches
// instead of a smooth fade along a line. The more evenly a spot is contested the more the shares
// wander; a ground that has the spot (almost) to itself is untouched, and a faint one never takes
// over.
fn interleave(w_in: array<f32, 12>, xz: vec2<f32>) -> array<f32, 12> {
    var w = w_in;
    // (the noises only where some ground is contested)
    var contested_any = false;
    for (var k = 0u; k < LAYERS; k++) {
        contested_any = contested_any || (w[k] > 0.004 && w[k] < 0.996);
    }
    var n = vec3<f32>(0.0);
    if contested_any {
        n = vec3<f32>(value_noise2(xz * 0.022, 151u), value_noise2(xz * 0.083, 157u), value_noise2(xz * 0.33, 163u)) - 0.5;
    }
    var s = 0.0;
    for (var k = 0u; k < LAYERS; k++) {
        if w[k] > 0.004 && w[k] < 0.996 {
            // (each layer its own mix of the three noises: two grounds meeting rarely agree)
            let h = vec3<f32>(f32(k & 1u), f32((k >> 1u) & 1u), f32(((k * 7u) >> 2u) & 1u)) * 2.0 - 1.0;
            let contested = clamp(w[k] * (1.0 - w[k]) * 4.0, 0.0, 1.0);
            w[k] *= exp2(dot(n, h * vec3<f32>(1.0, 0.8, 0.45)) * 3.2 * contested * contested);
        }
        s += w[k];
    }
    for (var k = 0u; k < LAYERS; k++) {
        w[k] /= max(s, 1e-4);
    }
    return w;
}

// Snow lies naturally: it collects in hollows and on ledges, slides off faces steeper than
// ~40-55° where the rock shows through (irregular ribs first), thins into patches at its lower
// edge and is combed into streaks along the wind. Painted snow (`painted`: its paint weight) is
// meant where it's painted: it clings to far steeper faces (up to ~75-85°, a few ribs showing).
// What it uncovers is the ground under it, or bare rock and scree where nothing else is there.
fn shape_snow(w_in: array<f32, 12>, wp: vec3<f32>, uv: vec2<f32>, ny: f32, cell: f32, nverts: f32, painted: f32) -> array<f32, 12> {
    var w = w_in;
    let s0 = w[9];
    if s0 < 0.003 {
        return w;
    }
    // curvature from the normal map ~3 cells around: hollows (+) fill up, ridges (-) blow bare
    let e = 1.5 / nverts;
    let nxp = textureSampleLevel(normal_map, linear_sampler, uv + vec2<f32>(e, 0.0), 0.0).x;
    let nxm = textureSampleLevel(normal_map, linear_sampler, uv - vec2<f32>(e, 0.0), 0.0).x;
    let nzp = textureSampleLevel(normal_map, linear_sampler, uv + vec2<f32>(0.0, e), 0.0).z;
    let nzm = textureSampleLevel(normal_map, linear_sampler, uv - vec2<f32>(0.0, e), 0.0).z;
    let div = 2.0 * ((nxp - nxm) + (nzp - nzm)) / (3.0 * cell);
    let hollow = clamp(-div * 9.0, -1.0, 1.0);
    // faces steeper than ~55° shed it, along irregular ribs; high above the snow line wind-packed
    // snow and ice cling to steeper faces (~65°)
    let rib = value_noise2(wp.xz * 0.25 + wp.y * 0.11, 167u) * 0.6 + value_noise2(wp.xz * 0.06 - wp.y * 0.03, 169u) * 0.4;
    let high = 0.16 * smoothstep(0.0, 150.0, wp.y - terrain.grid.w - terrain.style.x);
    let cling = max(high, 0.3 * smoothstep(0.05, 0.5, painted));
    let shed = smoothstep(0.36 - cling, 0.6 - cling, ny + (rib - 0.5) * 0.24);
    // patches and wind streaks (long along the wind, narrow across)
    var wd = terrain.visuals.zw;
    wd = select(vec2<f32>(0.8, 0.6), normalize(wd), dot(wd, wd) > 1e-6);
    let along = dot(wp.xz, wd);
    let across = dot(wp.xz, vec2<f32>(-wd.y, wd.x));
    let streak = value_noise2(vec2<f32>(along * 0.012, across * 0.16), 181u) * 0.65 + value_noise2(vec2<f32>(along * 0.04, across * 0.55), 191u) * 0.35;
    let patches = value_noise2(wp.xz * 0.035, 171u) * 0.55 + value_noise2(wp.xz * 0.13, 173u) * 0.3 + value_noise2(wp.xz * 0.55, 179u) * 0.15;
    let depth = (s0 + hollow * 0.3) * shed;
    // (the patches and streaks only break up the snow's edge; a deep snowfield stays whole)
    let edge = 1.0 - smoothstep(0.7, 1.0, depth);
    let cover = smoothstep(0.42, 0.58, depth + ((patches - 0.5) * 0.8 + (streak - 0.5) * 0.35) * mix(0.2, 1.0, edge)) * smoothstep(0.0, 0.08, depth);
    // the rest goes to the other grounds, or to bare rock / scree under a painted snowfield
    var rest = 0.0;
    for (var k = 0u; k < LAYERS; k++) {
        if k != 9u {
            rest += w[k];
        }
    }
    let rock = 1.0 - smoothstep(0.6, 0.85, ny);
    let alone = 1.0 - smoothstep(0.0, 0.08, rest);
    let bare = alone * (1.0 - cover);
    let scale = select(0.0, (1.0 - cover) / max(rest, 1e-4), rest > 1e-4) * (1.0 - alone);
    for (var k = 0u; k < LAYERS; k++) {
        w[k] *= scale;
    }
    w[9] = cover;
    w[6] += bare * mix(0.45, 1.0, rock);
    w[5] += bare * (1.0 - rock) * 0.55;
    return w;
}

// The shore map at `xz`: (level of the nearest water, distance to its edge (m), kind, 1); kind 0
// far from any water (or before the map is built). Level and distance are bilinear, the kind is
// the nearest texel's.
fn shore_at(xz: vec2<f32>) -> vec4<f32> {
    let dims = textureDimensions(shore_tex);
    if dims.x < 2u {
        return vec4<f32>(terrain.grid.w, 1e4, 0.0, 0.0);
    }
    let m = f32(dims.x);
    let g = clamp((xz + terrain.grid.x) * ((m - 1.0) / (2.0 * terrain.grid.x)), vec2<f32>(0.0), vec2<f32>(m - 1.001));
    let i = vec2<i32>(floor(g));
    let f = g - floor(g);
    let a = textureLoad(shore_tex, i, 0);
    let b = textureLoad(shore_tex, i + vec2<i32>(1, 0), 0);
    let c = textureLoad(shore_tex, i + vec2<i32>(0, 1), 0);
    let d = textureLoad(shore_tex, i + vec2<i32>(1, 1), 0);
    let v = mix(mix(a.xy, b.xy, f.x), mix(c.xy, d.xy, f.x), f.y);
    let near = select(select(a, b, f.x > 0.5), select(c, d, f.x > 0.5), f.y > 0.5);
    // (different waters meet: no blending of their levels)
    let same = a.z == b.z && a.z == c.z && a.z == d.z && max(max(abs(a.x - b.x), abs(a.x - c.x)), abs(a.x - d.x)) < 3.0;
    return select(vec4<f32>(near.xy, near.z, 1.0), vec4<f32>(v, near.z, 1.0), same);
}

fn layer_sample(k: u32, wp: vec3<f32>, fine_n: f32, micro_n: f32, mid_n: f32, micro_fade: f32, fine_fade: f32) -> LayerSample {
    var o: LayerSample;
    o.pert = vec3<f32>(0.0);
    o.ao = 1.0;
    let speck = (micro_n - 0.5) * micro_fade;
    switch k {
        case 0u: { // grass
            let tint = mix(srgb(vec3(0.24, 0.36, 0.10)), srgb(vec3(0.40, 0.48, 0.16)), clamp(fine_n * 0.7 + mid_n * 0.6 + 0.15, 0.0, 1.0));
            o.albedo = tint * (0.9 + speck * 0.45);
            o.rough = 0.88;
            o.height = 0.45 + 0.25 * fine_n + 0.3 * speck;
        }
        case 1u: { // dry grass
            let tint = mix(srgb(vec3(0.47, 0.43, 0.22)), srgb(vec3(0.62, 0.56, 0.31)), fine_n);
            o.albedo = tint * (0.9 + speck * 0.5);
            o.rough = 0.92;
            o.height = 0.45 + 0.25 * fine_n + 0.3 * speck;
        }
        case 2u: { // forest floor: needles, leaves, twigs
            let base = mix(srgb(vec3(0.17, 0.13, 0.08)), srgb(vec3(0.30, 0.24, 0.12)), fine_n);
            let leaf = step(0.72, micro_n) * micro_fade;
            let moss = smoothstep(0.55, 0.8, value_noise2(wp.xz * 0.12, 5u));
            o.albedo = mix(mix(base, srgb(vec3(0.22, 0.30, 0.10)), moss * 0.6), srgb(vec3(0.46, 0.26, 0.10)), leaf * 0.7);
            o.rough = 0.92;
            o.height = 0.5 + 0.2 * fine_n + leaf * 0.2;
        }
        case 3u: { // dirt with small pebbles
            let v = voronoi2(wp.xz * 2.2, 3u);
            let pebble = (1.0 - smoothstep(0.05, 0.22, v.x)) * step(0.6, v.z) * micro_fade;
            o.albedo = mix(srgb(vec3(0.33, 0.24, 0.16)), srgb(vec3(0.46, 0.35, 0.23)), fine_n) * (0.92 + speck * 0.3);
            o.albedo = mix(o.albedo, srgb(vec3(0.50, 0.47, 0.42)), pebble * 0.8);
            o.rough = 0.95;
            o.height = 0.35 + 0.2 * fine_n + pebble * 0.35;
        }
        case 4u: { // mud
            o.albedo = mix(srgb(vec3(0.17, 0.13, 0.09)), srgb(vec3(0.27, 0.20, 0.14)), fine_n);
            let puddle = smoothstep(0.6, 0.7, value_noise2(wp.xz * 0.35, 9u));
            o.albedo *= 1.0 - puddle * 0.35;
            o.rough = mix(0.45, 0.08, puddle);
            o.height = 0.25 + 0.15 * fine_n - puddle * 0.2;
        }
        case 5u: { // gravel
            let v = voronoi2(wp.xz * 5.0, 11u);
            let stone = 1.0 - smoothstep(0.25, 0.45, v.x);
            let shade = mix(0.36, 0.60, v.z);
            o.albedo = mix(srgb(vec3(0.20, 0.19, 0.17)), srgb(vec3(shade, shade * 0.97, shade * 0.92)), mix(0.6, stone, micro_fade));
            o.rough = 0.85;
            o.height = 0.4 + stone * 0.4 * micro_fade + fine_n * 0.2;
        }
        case 6u: { // rock: 3D noise + horizontal strata, no stretching on cliffs
            let n3 = fbm3(wp * 0.18, 3u, 21u);
            let strata = sin(wp.y * 1.1 + value_noise2(wp.xz * 0.02, 23u) * 9.0) * 0.5 + 0.5;
            let crack = smoothstep(0.08, 0.0, abs(value_noise3(wp * vec3(0.6, 1.8, 0.6), 27u) - 0.5)) * fine_fade;
            var c = mix(srgb(vec3(0.33, 0.31, 0.28)), srgb(vec3(0.52, 0.49, 0.45)), n3);
            c = mix(c, c * vec3(1.08, 1.02, 0.94), strata * 0.5);
            o.albedo = c * (1.0 - crack * 0.45) * (0.93 + speck * 0.25);
            o.rough = 0.78;
            o.height = 0.55 + 0.35 * n3 + strata * 0.1 - crack * 0.2;
        }
        case 7u: { // volcanic rock
            let n3 = fbm3(wp * 0.25, 3u, 41u);
            let v = voronoi2(wp.xz * 0.7, 43u);
            o.albedo = mix(srgb(vec3(0.07, 0.065, 0.065)), srgb(vec3(0.20, 0.18, 0.17)), n3) * (0.85 + 0.3 * v.z);
            o.rough = 0.65;
            o.height = 0.5 + 0.4 * n3;
        }
        case 8u: { // sand with wind ripples
            let dir = normalize(vec2<f32>(0.8, 0.6));
            let rip = sin(dot(wp.xz, dir) * 2.6 + mid_n * 14.0 + value_noise2(wp.xz * 0.4, 51u) * 3.0) * 0.5 + 0.5;
            o.albedo = mix(srgb(vec3(0.74, 0.64, 0.45)), srgb(vec3(0.85, 0.76, 0.57)), fine_n) * (0.96 + speck * 0.15 + rip * 0.05 * fine_fade);
            o.rough = 0.93;
            o.height = 0.35 + rip * 0.15 * fine_fade + fine_n * 0.1;
        }
        case 9u: { // snow
            o.albedo = srgb(vec3(0.90, 0.92, 0.96)) * (0.97 + fine_n * 0.05);
            o.rough = 0.55;
            o.height = 0.6 + 0.15 * fine_n + speck * 0.05;
        }
        case 10u: { // cobblestone
            let v = voronoi2(wp.xz * 1.5, 61u);
            let stone = smoothstep(0.03, 0.14, v.y - v.x);
            let shade = mix(0.34, 0.55, v.z);
            o.albedo = mix(srgb(vec3(0.16, 0.14, 0.12)), srgb(vec3(shade, shade * 0.96, shade * 0.9)) * (0.9 + speck * 0.3), stone);
            o.rough = mix(0.95, 0.7, stone);
            o.height = 0.3 + stone * 0.5;
        }
        default: { // ice
            let crack = smoothstep(0.03, 0.0, abs(value_noise2(wp.xz * 0.9, 71u) - 0.5)) * fine_fade;
            o.albedo = mix(srgb(vec3(0.55, 0.72, 0.84)), srgb(vec3(0.80, 0.90, 0.96)), fine_n) * (1.0 - crack * 0.3);
            o.rough = 0.12 + crack * 0.4;
            o.height = 0.3 - crack * 0.1;
        }
    }
    return o;
}

// Ground at the water's edge (from the shore map): damp dark soil on river banks with a band of
// wet gravel at the waterline and pebbles on the bed; around lava the ground is burnt to dark
// volcanic rock.
fn bank_weights(w_in: array<f32, 12>, sh: vec4<f32>, above: f32, ny: f32, xz: vec2<f32>) -> array<f32, 12> {
    var w = w_in;
    let kind = u32(sh.z + 0.5);
    if !((kind == 3u && sh.y < 18.0) || (kind == 4u && sh.y < 14.0)) {
        return w;
    }
    let n = value_noise2(xz * 0.3, 271u) - 0.5;
    var amt = 0.0;
    var t = array<f32, 12>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    if kind == 3u && sh.y < 18.0 {
        let bank = (1.0 - smoothstep(0.4, 1.8, above + n * 0.8)) * (1.0 - smoothstep(6.0, 18.0, sh.y + n * 6.0));
        let edge = 1.0 - smoothstep(0.15, 0.55, abs(above - 0.05) + n * 0.25);
        let gentle = smoothstep(0.72, 0.9, ny);
        // (a cut bank shows wet soil; where the river runs down a mountainside, wet rock)
        let cliff = 1.0 - smoothstep(0.45, 0.65, ny);
        let stones = select(mix(0.2, 0.85, edge) * gentle, 0.9, above < -0.1);
        let soil = (1.0 - stones) * (1.0 - cliff);
        t[5] = stones;
        t[6] = (1.0 - stones) * cliff;
        t[4] = soil * 0.75;
        t[3] = soil * 0.25;
        amt = bank * 0.85;
    } else if kind == 4u && sh.y < 14.0 {
        t[7] = 1.0;
        amt = 1.0 - smoothstep(3.0, 12.0, sh.y + n * 6.0);
    }
    if amt > 0.001 {
        for (var k = 0u; k < LAYERS; k++) {
            w[k] = w[k] * (1.0 - amt) + t[k] * amt;
        }
    }
    return w;
}

fn zone_id_at(g: vec2<f32>) -> u32 {
    let n = i32(terrain.grid.z);
    let c = clamp(vec2<i32>(round(g)), vec2<i32>(0), vec2<i32>(n - 1));
    return u32(textureLoad(zone_tex, c, 0).r * 255.0 + 0.5);
}

// Surface-gradient bump mapping from a scalar height field (Mikkelsen 2010).
fn perturb_normal(n: vec3<f32>, p: vec3<f32>, h: f32) -> vec3<f32> {
    let dpx = dpdx(p);
    let dpy = dpdy(p);
    let dhx = dpdx(h);
    let dhy = dpdy(h);
    let r1 = cross(dpy, n);
    let r2 = cross(n, dpx);
    let det = dot(dpx, r1);
    let grad = sign(det) * (dhx * r1 + dhy * r2);
    return normalize(abs(det) * n - grad);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let wp = in.world_position.xyz;
    let half = terrain.grid.x;
    let cell = terrain.grid.y;
    let nverts = terrain.grid.z;
    let sea = terrain.grid.w;

    let g = (wp.xz + vec2<f32>(half)) / cell;
    let uv = (g + 0.5) / nverts;
    let nmap = textureSample(normal_map, linear_sampler, uv);
    let n_geo = normalize(nmap.xyz * 2.0 - 1.0);
    let flow = nmap.a;
    // the paint is read through a small wandering warp (about ±1.5 m), so painted borders never
    // follow the 4 m vertex grid or a ruled line (paths keep their course)
    // (its fine octave, ±0.4 m, moves the paint by under a pixel from ~300 m on and fades out there)
    let dist = length(view.world_position - wp);
    var wq = (vec2<f32>(value_noise2(wp.xz * 0.21, 101u), value_noise2(wp.xz * 0.21 + vec2<f32>(17.3, 5.9), 103u)) - 0.5) * 2.2;
    let wq_fine = 1.0 - smoothstep(300.0, 450.0, dist);
    if wq_fine > 0.0 {
        wq += (vec2<f32>(value_noise2(wp.xz * 0.83, 107u), value_noise2(wp.xz * 0.83 + vec2<f32>(3.1, 11.7), 109u)) - 0.5) * (0.8 * wq_fine);
    }
    let suv = uv + wq / (cell * nverts);
    let s0 = textureSample(splat0, linear_sampler, suv);
    let s1 = textureSample(splat1, linear_sampler, suv);
    let s2 = textureSample(splat2, linear_sampler, suv);

    let fine_fade = 1.0 - smoothstep(60.0, 320.0, dist);
    let micro_fade = 1.0 - smoothstep(10.0, 70.0, dist);
    let use_tex = terrain.tex.x > 0.5;

    let macro_n = value_noise2(wp.xz * 0.004, 17u) - 0.5;
    let mid_n = value_noise2(wp.xz * 0.05, 29u) - 0.5;
    // (the procedural grounds' finer noises: the textured look doesn't use them)
    var fine_n = 0.5;
    var micro_n = 0.5;
    if !use_tex {
        fine_n = value_noise2(wp.xz * 0.45, 31u);
        if micro_fade > 0.0 {
            micro_n = mix(0.5, value_noise2(wp.xz * 2.7, 37u), micro_fade);
        }
    }

    // ---- layer weights: auto rules overridden by paint
    var w = auto_weights(wp.y, n_geo.y, macro_n, mid_n, dry_noise(wp.xz));
    let paint = array<f32, 12>(s0.r, s0.g, s0.b, s0.a, s1.r, s1.g, s1.b, s1.a, s2.r, s2.g, s2.b, s2.a);
    var ps = 0.0;
    for (var k = 0u; k < LAYERS; k++) { ps += paint[k]; }
    ps = min(ps, 1.0);
    // painted ground wins over the automatic snow line (mirrored in `Terrain::final_weights`)
    w[9] *= 1.0 - smoothstep(0.0, 0.6, ps);
    for (var k = 0u; k < LAYERS; k++) { w[k] = w[k] * (1.0 - ps) + paint[k]; }
    w = flow_weights(w, flow, n_geo.y, mid_n);
    // the nearest water's banks, snow lying naturally, interleaved transitions
    let sh = shore_at(wp.xz);
    let above = wp.y - sh.x;
    let shore_kind = u32(sh.z + 0.5);
    w = bank_weights(w, sh, above, n_geo.y, wp.xz);
    w = shape_snow(w, wp, uv, n_geo.y, cell, nverts, paint[9]);
    w = interleave(w, wp.xz);

    // ---- the strongest layers (`TOP`; a pixel rarely has more above 0.004): shaded, then
    // height-blended. Where more are present the weakest go, and the kept ones give up the weight
    // of the strongest one dropped, so a layer enters or leaves at zero weight (no popping).
    var top_k: array<u32, TOP + 1u>;
    var top_w: array<f32, TOP + 1u>;
    for (var k = 0u; k < LAYERS; k++) {
        var wk = w[k];
        var kk = k;
        if wk > 0.004 {
            for (var i = 0u; i < TOP + 1u; i++) {
                if wk > top_w[i] {
                    let tw = top_w[i];
                    let tk = top_k[i];
                    top_w[i] = wk;
                    top_k[i] = kk;
                    wk = tw;
                    kk = tk;
                }
            }
        }
    }
    let cut = top_w[TOP];
    let dpx = dpdx(wp);
    let dpy = dpdy(wp);
    let far = smoothstep(40.0, 260.0, dist);
    // (the anti-tiling noise doesn't matter once the macro copy has taken over)
    var anti = 0.0;
    if far < 1.0 {
        anti = smoothstep(0.3, 0.7, value_noise2(wp.xz * 0.045, 43u));
    }
    let fine = dist < 300.0;
    var albedos: array<vec3<f32>, TOP>;
    var roughs: array<f32, TOP>;
    var heights: array<f32, TOP>;
    var perts: array<vec3<f32>, TOP>;
    var aos: array<f32, TOP>;
    var lw: array<f32, TOP>;
    var max_h = -10.0;
    for (var i = 0u; i < TOP; i++) {
        lw[i] = top_w[i] - cut;
        if lw[i] > 0.0 {
            let k = top_k[i];
            var s: LayerSample;
            if use_tex {
                s = tex_layer(k, wp, n_geo, dpx, dpy, anti, far, fine);
            } else {
                s = layer_sample(k, wp, fine_n, micro_n, mid_n, micro_fade, fine_fade);
            }
            albedos[i] = s.albedo;
            roughs[i] = s.rough;
            perts[i] = s.pert;
            aos[i] = s.ao;
            heights[i] = s.height + lw[i];
            max_h = max(max_h, heights[i]);
        }
    }
    let blend_depth = 0.25;
    var albedo = vec3<f32>(0.0);
    var rough = 0.0;
    var bump_h = 0.0;
    var pert = vec3<f32>(0.0);
    var ao = 0.0;
    var wsum = 0.0;
    let dominant = top_k[0];
    for (var i = 0u; i < TOP; i++) {
        if lw[i] > 0.0 {
            let hw = max(heights[i] - max_h + blend_depth, 0.0) * lw[i];
            albedo += albedos[i] * hw;
            rough += roughs[i] * hw;
            bump_h += (heights[i] - lw[i]) * hw;
            pert += perts[i] * hw;
            ao += aos[i] * hw;
            wsum += hw;
        }
    }
    albedo /= max(wsum, 1e-4);
    rough /= max(wsum, 1e-4);
    bump_h /= max(wsum, 1e-4);
    pert /= max(wsum, 1e-4);
    ao /= max(wsum, 1e-4);

    // snowfields are combed by the wind: faint long drifts and sastrugi, never a flat white sheet
    if w[9] > 0.05 {
        var wdir = terrain.visuals.zw;
        wdir = select(vec2<f32>(0.8, 0.6), normalize(wdir), dot(wdir, wdir) > 1e-6);
        let al = dot(wp.xz, wdir);
        let ac = dot(wp.xz, vec2<f32>(-wdir.y, wdir.x));
        let drift = value_noise2(vec2<f32>(al * 0.02, ac * 0.2), 193u) * 0.6 + value_noise2(vec2<f32>(al * 0.09, ac * 0.9), 197u) * 0.4;
        albedo *= mix(1.0, 0.9 + 0.14 * drift, w[9]);
    }
    // large-scale tonal variation to break up uniform areas
    let macro_tone = fbm2(wp.xz * 0.0018, 3u, 91u);
    albedo *= select(0.86 + 0.28 * macro_tone, 0.9 + 0.2 * macro_tone, use_tex);

    // ---- wetness and the shore: rain, the surf on sea beaches, lapped lake shores, damp river
    // banks, lava rims
    var wet = terrain.style2.y * (1.0 - w[9]);
    // (a thin sheet of water running over the ground, foam on it, the glow of molten rock)
    var film = 0.0;
    var foam = 0.0;
    var glow = vec3<f32>(0.0);
    let t = globals.time;
    var edge_n = 0.0;
    if shore_kind != 0u {
        edge_n = value_noise2(wp.xz * 0.6, 241u) - 0.5;
    }
    if shore_kind == 1u && sh.y < 120.0 && above < 3.0 {
        // the swash runs up the beach behind each breaking wave and drains back; the sand it
        // reaches is dark and glossy, drying towards an uneven tide mark
        let hs = terrain.visuals.x;
        let cyc = surf_cycle(wp.xz, t, hs);
        let c = fract(cyc.x);
        let reach = run_up(hs) * cyc.y;
        let front = swash_front(c, reach) + edge_n * 0.05 + (value_noise2(wp.xz * 2.1, 243u) - 0.5) * 0.025;
        film = smoothstep(-0.004, 0.03, front - above);
        // a lacy line of foam on the rushing front, bubbles stranded as it drains
        let lace = value_noise2(wp.xz * 3.1 + vec2<f32>(c * 2.0, 0.0), 247u) * 0.6 + value_noise2(wp.xz * 9.0, 251u) * 0.4;
        let lead = 1.0 - smoothstep(0.0, 0.035 + 0.04 * cyc.y, abs(front - 0.012 - above));
        let rush = 1.0 - smoothstep(0.18, 0.6, c);
        let stranded = smoothstep(0.0, 0.03, above - front) * (1.0 - smoothstep(0.0, 0.12, above - front)) * smoothstep(0.24, 0.32, c) * (1.0 - smoothstep(0.5, 0.95, c));
        foam = max(lead * smoothstep(0.32, 0.62, lace) * mix(0.45, 1.0, rush), stranded * smoothstep(0.6, 0.78, lace) * 0.8);
        foam *= clamp(terrain.visuals.y, 0.0, 1.0) * smoothstep(0.0, 0.05, reach);
        let top = run_up(hs) * (0.95 + 0.35 * value_noise2(wp.xz * 0.045, 253u)) + 0.06;
        wet = max(wet, 1.0 - smoothstep(top * 0.4, top, above + edge_n * 0.06));
    } else if shore_kind == 2u && sh.y < 30.0 && above < 1.0 && above > -0.2 {
        // small waves lap a few centimetres up the shore over a narrow damp band
        let lap = sin(t * 1.3 + dot(wp.xz, vec2<f32>(0.21, 0.17)) + edge_n * 5.0) * 0.5 + 0.5;
        // (only right at the water: a flat shore a hand's breadth above the level stays dry)
        let near = 1.0 - smoothstep(2.0, 7.0, sh.y);
        film = smoothstep(-0.004, 0.012, 0.015 + 0.045 * lap - above) * smoothstep(-0.2, -0.05, above) * near;
        wet = max(wet, (1.0 - smoothstep(0.06, 0.3, above + edge_n * 0.06)) * (1.0 - smoothstep(4.0, 12.0, sh.y)));
    } else if shore_kind == 3u && sh.y < 30.0 && above < 2.0 && above > -0.2 {
        wet = max(wet, (1.0 - smoothstep(0.05, 0.75, above + edge_n * 0.3)) * 0.85 * (1.0 - smoothstep(6.0, 16.0, sh.y)));
        film = smoothstep(-0.004, 0.01, 0.02 - above) * smoothstep(-0.2, -0.05, above) * (1.0 - smoothstep(2.0, 7.0, sh.y));
    } else if shore_kind == 4u && sh.y < 70.0 && above < 25.0 {
        // charred ground, red-hot at the rim, flickering; the glow of the molten rock lights the
        // ground around it (a few tens of metres, most visible once the sun is down)
        let rim = (1.0 - smoothstep(0.0, 2.4 + edge_n * 1.6, sh.y)) * (1.0 - smoothstep(0.1, 1.4, above));
        let flick = 0.7 + 0.3 * value_noise2(vec2<f32>(t * 1.9, 0.0) + wp.xz * 0.45, 257u);
        albedo *= mix(1.0, 0.4, 1.0 - smoothstep(0.0, 10.0, sh.y));
        let dark = 1.0 - smoothstep(-0.08, 0.25, terrain.sun.y);
        let reach = exp(-sh.y / 6.0) * (1.0 - smoothstep(3.0, 18.0, above));
        glow = vec3<f32>(1.0, 0.3, 0.05) * rim * rim * flick * 1.6 + albedo * vec3<f32>(1.0, 0.34, 0.08) * reach * mix(0.15, 1.6, dark) * (0.85 + 0.15 * flick);
    }
    wet = clamp(wet, 0.0, 1.0);
    albedo *= mix(1.0, 0.6, wet);
    rough = mix(rough, 0.3, wet * 0.85);
    // under a sheet of water: darker still, tinted by it, a mirror; foam on top
    albedo = mix(albedo, albedo * 0.55 + terrain.water.rgb * 0.03, film);
    rough = mix(rough, 0.035, film);
    albedo = mix(albedo, vec3<f32>(0.8, 0.83, 0.84), foam);
    rough = mix(rough, 0.7, foam);
    // damp ground where rain gathers: darker, with a mossy green on the flats
    let damp = smoothstep(0.4, 0.75, flow) * (1.0 - w[9]) * (1.0 - wet);
    albedo *= mix(1.0, 0.8, damp);
    albedo = mix(albedo, albedo * vec3<f32>(0.78, 0.9, 0.6), damp * smoothstep(0.8, 0.95, n_geo.y) * 0.7);
    rough = mix(rough, rough * 0.85, damp);

    // ---- rain puddles (rain.wesl; `overlay_image` y: puddle fill, z: rain falling): standing
    // water in the flat, low spots of ground that holds it (earth, mud, gravel, cobbles, flat
    // rock; hardly grass or sand), filling as the rain goes on: dark, a mirror (the sky and the
    // lamps in it), rings from the drops while it rains. (A uniform branch: dry worlds skip it.)
    var puddle = 0.0;
    var ripple = vec2<f32>(0.0);
    if terrain.overlay_image.y > 0.0 && dist < 420.0 {
        let holds = w[0] * 0.12 + w[1] * 0.1 + w[2] * 0.35 + w[3] * 0.9 + w[4] + w[5] * 0.7 + (w[6] + w[7]) * 0.55 + w[8] * 0.15 + w[10];
        // (on a gentle slope only the deepest gaps hold water; none past about 20°)
        let flat = smoothstep(0.93, 0.975, n_geo.y) * (1.0 - w[9]) * smoothstep(0.1, 0.3, wp.y - sea) * (1.0 - smoothstep(300.0, 420.0, dist));
        let low = (0.5 - bump_h) * 1.5 + smoothstep(0.3, 0.8, flow) + (min(holds, 1.0) - 1.0) * 4.0 - (1.0 - n_geo.y) * 25.0;
        puddle = puddle_amount(wp.xz, terrain.overlay_image.y, low, flat);
        albedo = mix(albedo, albedo * 0.45, puddle);
        rough = mix(rough, 0.02, puddle);
        if puddle > 0.0 && terrain.overlay_image.z > 0.0 && dist < 45.0 {
            ripple = rain_ripples(wp.xz, t, terrain.overlay_image.z) * (1.0 - smoothstep(25.0, 45.0, dist));
        }
    }

    // ---- under water: no tint here. The water surface drawn on top shades what it sees through
    // (absorption and scattering along the light's way down and the view's way up, caustics), so
    // the bed keeps its own colour, only wet
    let depth = sea - wp.y;

    // ---- normal: geometry normal from the full-res map + procedural bump
    let bump_strength = 0.35 * (0.3 + 0.7 * fine_fade);
    var n_bumped = perturb_normal(n_geo, wp, bump_h * select(bump_strength, 0.0, use_tex));
    if use_tex {
        n_bumped = normalize(n_geo + pert * terrain.tex.z);
        pbr_input.diffuse_occlusion = vec3<f32>(ao);
    }
    // a sheet of water smooths the ground's relief
    n_bumped = normalize(mix(n_bumped, n_geo, film * 0.85));
    // and so does a puddle (mirror-flat but for the drops' rings)
    if puddle > 0.0 {
        n_bumped = normalize(mix(n_bumped, n_geo, puddle) - vec3<f32>(ripple.x, 0.0, ripple.y) * (0.12 * puddle));
    }

    pbr_input.material.base_color = vec4<f32>(albedo, 1.0);
    pbr_input.material.perceptual_roughness = clamp(rough, 0.05, 1.0);
    pbr_input.material.metallic = 0.0;
    // (display units: the rim of lava glows the same by day and by night)
    pbr_input.material.emissive = vec4<f32>(glow, 0.0);
    pbr_input.world_normal = n_geo;
    pbr_input.N = n_bumped;

    // ---- moving cloud shadows (only dim direct sunlight): the volumetric clouds' cover map,
    // projected along the sun to the lower part of the cloud layer; the clouds only fill the
    // inner part of their cover (see clouds.wesl), so the shadow does too. (The baked map is the
    // cover field `noise.wesl::cloud_density` thresholds, 32 m a texel: far finer than the
    // shadow's own softness.)
    var cloud = 0.0;
    if terrain.sun.w > 0.0 {
        let sun_dir = normalize(terrain.sun.xyz);
        let cloud_alt = 2000.0;
        let to_cloud = (cloud_alt - wp.y) / max(sun_dir.y, 0.05);
        let cloud_p = wp.xz + sun_dir.xz * to_cloud;
        @if(CLOUD_COVER_MAP) {
            // (none before the map is bound)
            if textureDimensions(cloud_cover_tex).x > 1u {
                let cn = textureSampleLevel(cloud_cover_tex, cloud_sampler, (cloud_p + terrain.clouds.zw) / CLOUD_TILE, 0.0).r;
                cloud = smoothstep(0.25, 0.7, cloud_cover(cn, terrain.clouds.x));
            }
        } @else {
            cloud = smoothstep(0.25, 0.7, cloud_density(cloud_p, terrain.clouds.x, terrain.clouds.zw));
        }
    }
    pbr_input.directional_shadow_factor = 1.0 - cloud * terrain.sun.w;
    // ---- the sky light the tree crowns above hold back (forest floors)
    let canopy = canopy_sky(wp);
    pbr_input.diffuse_occlusion *= canopy;
    pbr_input.specular_occlusion *= canopy;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);

    // ---- overlays (unlit, drawn on top of fog)
    let mode = u32(terrain.overlay.x + 0.5);
    let px = max(length(fwidth(wp.xz)), 0.001);
    if mode == 2u {
        // footstep surface map
        var sc = terrain.layer_overlay[dominant].rgb;
        if depth > 0.15 {
            sc = vec3<f32>(0.03, 0.2, 1.0);
        }
        out = blend_overlay(out, sc, 0.72);
    }
    if mode == 1u || terrain.overlay.z > 0.5 {
        let id = zone_id_at(g);
        let lw = max(cell * 0.6, px * 1.6) / cell;
        let a = zone_id_at(g + vec2<f32>(lw, 0.0));
        let b = zone_id_at(g - vec2<f32>(lw, 0.0));
        let c = zone_id_at(g + vec2<f32>(0.0, lw));
        let d = zone_id_at(g - vec2<f32>(0.0, lw));
        let border = id != a || id != b || id != c || id != d;
        let zmax = max(max(max(id, a), max(b, c)), d);
        if mode == 1u && id > 0u {
            var fill = terrain.overlay.y;
            if f32(id) == terrain.overlay.w {
                fill = fill + 0.08 + 0.05 * sin(globals.time * 3.0);
            }
            out = blend_overlay(out, terrain.zone_colors[min(id, 63u)].rgb, fill);
        }
        if border && zmax > 0u {
            let col = terrain.zone_colors[min(zmax, 63u)].rgb;
            // dashed glow line
            let dash = step(0.35, fract((wp.x + wp.z) / max(px * 18.0, 2.0)));
            out = blend_overlay(out, col * 1.35 + 0.08, 0.92 * mix(0.55, 1.0, dash));
        }
    }

    // tool overlay image (heatmaps, regions)
    if terrain.overlay_image.x > 0.0 {
        let ouv = (wp.xz + terrain.grid.x) / (2.0 * terrain.grid.x);
        let oc = textureSampleLevel(overlay_tex, linear_sampler, ouv, 0.0);
        out = blend_overlay(out, oc.rgb, oc.a * terrain.overlay_image.x);
    }

    // brush cursor
    if terrain.brush_color.a > 0.0 {
        let d = distance(wp.xz, terrain.brush.xy);
        let r = terrain.brush.z;
        let ring = 1.0 - smoothstep(px * 1.0, px * 2.6, abs(d - r));
        let inner_r = r * (1.0 - terrain.brush.w);
        let ang = atan2(wp.z - terrain.brush.y, wp.x - terrain.brush.x);
        let dashes = step(0.5, fract(ang * 10.0 / 3.14159 + globals.time * 0.3));
        let inner = (1.0 - smoothstep(px * 0.8, px * 2.0, abs(d - inner_r))) * dashes * step(0.02, terrain.brush.w);
        let fill = (1.0 - smoothstep(inner_r, r, d)) * step(d, r) * 0.10;
        let center = 1.0 - smoothstep(px * 1.5, px * 3.0, d);
        let a = clamp(ring * 0.95 + inner * 0.6 + fill + center, 0.0, 1.0) * terrain.brush_color.a;
        out = blend_overlay(out, terrain.brush_color.rgb, a);
    }
    return out;
}

fn blend_overlay(o: FragmentOutput, col: vec3<f32>, a: f32) -> FragmentOutput {
    var r = o;
    r.color = vec4<f32>(mix(o.color.rgb, col, clamp(a, 0.0, 1.0)), o.color.a);
    return r;
}
