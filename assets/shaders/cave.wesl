// Cave rock (`crate::caves::render`): photoscanned rock, floor and accent textures from the
// terrain library projected on three axes, chosen per vertex by the cave's theme.
//
// Vertex inputs: colour = (daylight reaching it 0..1, ambient occlusion, dampness, outside
// ground 0..1), uv = (theme slot, darkness fill 0..1).
//
// - Walls mix two rocks by a slow 3D noise; floors (and ledges) get the floor texture height-
//   blended over the rock; the outside ground round a mouth uses the terrain's own rock / dirt /
//   gravel layers so it meets the terrain without a seam.
// - Accents by theme: moss where daylight reaches (mossy, limestone), a frost coat (ice), sand
//   drifts (sandstone), glowing embers deep in lava tubes, sparkling veins (crystal), ore
//   streaks (mine).
// - Damp rock darkens and turns glossy, with drip streaks down the walls and wet floors.
// - Light: sky light only as far as daylight reaches (`colour.r`); deeper in, the cave's fill
//   (dark / dim / readable) is a glow in display units, so it reads the same at any exposure.
// - X-ray: everything above the cut-away height is not drawn (look down into the cave).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};
import super::noise::{value_noise2, value_noise3, fbm2, fbm3, cloud_density};

struct Theme {
    // layer in the cave texture array of: wall A, wall B, floor, accent
    tex: vec4<f32>,
    // repeat size (m) of each
    tile: vec4<f32>,
    // rgb: tint, a: how damp it gets
    tint: vec4<f32>,
    // x: accent kind (0 moss, 1 frost, 2 sand, 3 embers, 4 crystal, 5 ore), y: accent amount,
    // z: gloss, w: rock contrast
    fx: vec4<f32>,
}

struct CaveParams {
    // xyz: direction towards the sun, w: cloud shadow strength
    sun: vec4<f32>,
    // x: cloud coverage, y: cloud scale, zw: cloud offset
    clouds: vec4<f32>,
    // x: normal strength, y: texture softening (mip bias, log2), z: rain wetness, w: the
    // terrain's anti-tiling strength
    look: vec4<f32>,
    // x: cut-away height (x-ray), y: cut-away on, z: terrain textures ready, w: cave textures ready
    misc: vec4<f32>,
    // repeat size (m) of the terrain's rock, dirt, gravel and grass layers
    ground_tile: vec4<f32>,
    // rgb: fill light colour, a: time (s)
    fill: vec4<f32>,
    themes: array<Theme, 7>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> cave: CaveParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var cave_albedo: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var cave_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var cave_detail: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var ground_albedo: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var ground_detail: texture_2d_array<f32>;

// The exposure of an eye adjusted to a cave by day (ev100 ≈ 14.6 − 5 = 9.6, `sky::CAVE_EV`):
// the fill is drawn as bright as `fill` says at this exposure, darker at brighter exposures.
const CAVE_EXPOSURE: f32 = 1.0 / (1.2 * 776.0);

struct Tex {
    a: vec4<f32>,
    d: vec4<f32>,
}

struct Surf {
    albedo: vec3<f32>,
    height: f32,
    rough: f32,
    ao: f32,
    pert: vec3<f32>,
}

fn samp(k: i32, uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>, ground: bool) -> Tex {
    var o: Tex;
    let b = exp2(cave.look.y);
    if ground {
        o.a = textureSampleGrad(ground_albedo, cave_sampler, uv, k, gx * b, gy * b);
        o.d = textureSampleGrad(ground_detail, cave_sampler, uv, k, gx * b, gy * b);
    } else {
        o.a = textureSampleGrad(cave_albedo, cave_sampler, uv, k, gx * b, gy * b);
        o.d = textureSampleGrad(cave_detail, cave_sampler, uv, k, gx * b, gy * b);
    }
    return o;
}

// Three-axis projection, blended by the normal.
fn tri(k: i32, wp: vec3<f32>, bw: vec3<f32>, tile: f32, dpx: vec3<f32>, dpy: vec3<f32>, ground: bool) -> Surf {
    let s = 1.0 / tile;
    let tx = samp(k, wp.zy * s, dpx.zy * s, dpy.zy * s, ground);
    let ty = samp(k, wp.xz * s, dpx.xz * s, dpy.xz * s, ground);
    let tz = samp(k, wp.xy * s, dpx.xy * s, dpy.xy * s, ground);
    let a = tx.a * bw.x + ty.a * bw.y + tz.a * bw.z;
    let nx = tx.d.xy * 2.0 - 1.0;
    let ny = ty.d.xy * 2.0 - 1.0;
    let nz = tz.d.xy * 2.0 - 1.0;
    var o: Surf;
    o.albedo = a.rgb;
    o.height = a.a;
    o.rough = tx.d.z * bw.x + ty.d.z * bw.y + tz.d.z * bw.z;
    o.ao = tx.d.w * bw.x + ty.d.w * bw.y + tz.d.w * bw.z;
    o.pert = vec3<f32>(0.0, nx.y, nx.x) * bw.x + vec3<f32>(ny.x, 0.0, ny.y) * bw.y + vec3<f32>(nz.x, nz.y, 0.0) * bw.z;
    return o;
}

fn rot2(v: vec2<f32>, c: f32, s: f32) -> vec2<f32> {
    return vec2<f32>(v.x * c - v.y * s, v.x * s + v.y * c);
}

// A terrain layer seen from above exactly as the terrain shader draws it (`sample_top` there):
// the scan plus a rotated, larger copy blended in by noise and with distance, so the ground
// round a cave mouth matches the terrain next to it texel for texel.
fn ground_top(k: i32, wp: vec3<f32>, tile: f32, dpx: vec3<f32>, dpy: vec3<f32>, dist: f32) -> Tex {
    let anti = smoothstep(0.3, 0.7, value_noise2(wp.xz * 0.045, 43u));
    let far = smoothstep(40.0, 260.0, dist);
    var t1 = samp(k, wp.xz / tile, dpx.xz / tile, dpy.xz / tile, true);
    t1.d = vec4<f32>(t1.d.xy * 2.0 - 1.0, t1.d.zw);
    let amt = mix(anti * cave.look.w, 0.8, far);
    if amt <= 0.01 {
        return t1;
    }
    let c = 0.8090;
    let sn = 0.5878;
    let sc = tile * mix(1.618, 7.0, far);
    var t2 = samp(k, rot2(wp.xz, c, sn) / sc + vec2<f32>(0.37, 0.71), rot2(dpx.xz, c, sn) / sc, rot2(dpy.xz, c, sn) / sc, true);
    let n2 = rot2(t2.d.xy * 2.0 - 1.0, c, -sn);
    t2.d = vec4<f32>(n2, t2.d.zw);
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

fn ground_layer(k: i32, wp: vec3<f32>, n: vec3<f32>, tile: f32, dpx: vec3<f32>, dpy: vec3<f32>, dist: f32) -> Surf {
    let ty = ground_top(k, wp, tile, dpx, dpy, dist);
    var o: Surf;
    if k == 6 {
        // rock: on three axes (as the terrain's rock layers)
        var bw = pow(abs(n), vec3<f32>(4.0));
        bw /= (bw.x + bw.y + bw.z);
        let s = 1.0 / tile;
        let tx = samp(k, wp.zy * s, dpx.zy * s, dpy.zy * s, true);
        let tz = samp(k, wp.xy * s, dpx.xy * s, dpy.xy * s, true);
        let nx = tx.d.xy * 2.0 - 1.0;
        let nz = tz.d.xy * 2.0 - 1.0;
        let a = ty.a * bw.y + tx.a * bw.x + tz.a * bw.z;
        o.albedo = a.rgb;
        o.height = a.a;
        o.rough = ty.d.z * bw.y + tx.d.z * bw.x + tz.d.z * bw.z;
        o.ao = ty.d.w * bw.y + tx.d.w * bw.x + tz.d.w * bw.z;
        o.pert = vec3<f32>(ty.d.x, 0.0, ty.d.y) * bw.y + vec3<f32>(0.0, nx.y, nx.x) * bw.x + vec3<f32>(nz.x, nz.y, 0.0) * bw.z;
        return o;
    }
    o.albedo = ty.a.rgb;
    o.height = ty.a.a;
    o.rough = ty.d.z;
    o.ao = ty.d.w;
    o.pert = vec3<f32>(ty.d.x, 0.0, ty.d.y);
    return o;
}

// Without textures (still loading / missing): plain stone.
fn plain(wp: vec3<f32>, tint: vec3<f32>) -> Surf {
    var o: Surf;
    let n = fbm3(wp * 0.6, 3u, 7u);
    o.albedo = mix(vec3<f32>(0.16, 0.15, 0.14), vec3<f32>(0.34, 0.32, 0.3), n) * tint;
    o.height = n;
    o.rough = 0.85;
    o.ao = 1.0;
    o.pert = vec3<f32>(0.0);
    return o;
}

fn mix_surf(a: Surf, b: Surf, t: f32) -> Surf {
    var o: Surf;
    o.albedo = mix(a.albedo, b.albedo, t);
    o.height = mix(a.height, b.height, t);
    o.rough = mix(a.rough, b.rough, t);
    o.ao = mix(a.ao, b.ao, t);
    o.pert = mix(a.pert, b.pert, t);
    return o;
}

// Height blend: `b` laid over `a` with weight `w` — b's high spots (stones, ridges) show first.
fn over(a: Surf, b: Surf, w: f32) -> Surf {
    if w <= 0.001 {
        return a;
    }
    if w >= 0.999 {
        return b;
    }
    let ha = a.height + 1.0 - w;
    let hb = b.height + w;
    let top = max(ha, hb) - 0.22;
    let wa = max(ha - top, 0.0);
    let wb = max(hb - top, 0.0);
    return mix_surf(a, b, wb / max(wa + wb, 1e-4));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let wp = in.world_position.xyz;
    // x-ray: look down into the cave
    if cave.misc.y > 0.5 && wp.y > cave.misc.x {
        discard;
    }
    // x-ray: rock seen from inside (cut open) is a flat dark section
    if !is_front {
        var sec: FragmentOutput;
        let g = 0.045 + 0.015 * value_noise2(wp.xz * 0.7, 67u);
        sec.color = vec4<f32>(g * 1.05, g, g * 0.92, 1.0);
        return sec;
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var col = vec4<f32>(1.0, 1.0, 0.3, 0.0);
    var uv = vec2<f32>(0.0, 0.35);
@if(VERTEX_COLORS)
    col = in.color;
@if(VERTEX_UVS_A)
    uv = in.uv;
    let day = clamp(col.r, 0.0, 1.0);
    let vao = clamp(col.g, 0.0, 1.0);
    let damp = clamp(col.b, 0.0, 1.0);
    let skin = clamp(col.a, 0.0, 1.0);
    let th = cave.themes[min(u32(uv.x + 0.5), 6u)];
    let n_geo = normalize(in.world_normal);
    var bw = pow(abs(n_geo), vec3<f32>(4.0));
    bw /= (bw.x + bw.y + bw.z);
    let dpx = dpdx(wp);
    let dpy = dpdy(wp);
    let dist = length(view.world_position - wp);
    let up = n_geo.y;
    let tex_ok = cave.misc.w > 0.5;

    // ---- rock: two scans mixed by a slow 3D noise (beds of different stone)
    let mixn = smoothstep(0.35, 0.65, fbm3(wp * 0.045, 2u, 13u) + (value_noise3(wp * 0.21, 17u) - 0.5) * 0.25);
    var rock: Surf;
    if tex_ok {
        let ra = tri(i32(th.tex.x + 0.5), wp, bw, th.tile.x, dpx, dpy, false);
        let rb = tri(i32(th.tex.y + 0.5), wp * 1.07 + vec3<f32>(3.1, 0.0, 7.7), bw, th.tile.y, dpx, dpy, false);
        rock = over(ra, rb, mixn);
    } else {
        rock = plain(wp, th.tint.rgb);
    }
    // the rock's own large-scale tone (beds, stains), stronger with the theme's contrast
    let tone = fbm3(wp * vec3<f32>(0.05, 0.16, 0.05), 3u, 21u);
    rock.albedo *= th.tint.rgb * mix(1.0, 0.72 + 0.56 * tone, th.fx.w);

    // ---- floors and ledges: the floor texture over the rock where it faces up (inside; out in
    // the open the tops take the hillside's own ground, below)
    let outside = smoothstep(0.82, 0.97, day);
    let floorw = smoothstep(0.42, 0.78, up + (rock.height - 0.5) * 0.25) * (1.0 - outside);
    var s = rock;
    if floorw > 0.001 {
        var fl: Surf;
        if tex_ok {
            fl = tri(i32(th.tex.z + 0.5), wp, vec3<f32>(bw.x * 0.3, bw.y + 0.0001, bw.z * 0.3) / (bw.x * 0.3 + bw.y + 0.0001 + bw.z * 0.3), th.tile.z, dpx, dpy, false);
        } else {
            fl = plain(wp + 11.0, th.tint.rgb * 0.9);
        }
        s = over(rock, fl, floorw);
    }

    // ---- accents
    let kind = u32(th.fx.x + 0.5);
    let amt = th.fx.y;
    var emissive = vec3<f32>(0.0);
    let an = value_noise3(wp * 0.35, 29u) * 0.6 + value_noise3(wp * 1.3, 31u) * 0.4;
    if amt > 0.0 && tex_ok {
        let acc = tri(i32(th.tex.w + 0.5), wp, bw, th.tile.w, dpx, dpy, false);
        var w = 0.0;
        if kind == 0u {
            // moss: in the twilight zone just inside (some light, damp, out of the sun), on
            // floors and walls (not ceilings), patchy
            let dd = day + (an - 0.5) * 0.4;
            w = smoothstep(0.05, 0.35, dd) * (1.0 - smoothstep(0.6, 0.9, dd)) * smoothstep(-0.35, 0.3, up) * amt;
        } else if kind == 1u {
            // frost: a coat over the inside, thickest on upward faces and in the twilight near
            // the mouth; out in the sun only patches cling to the rock
            w = clamp(0.5 + up * 0.3 + day * 0.25 + (an - 0.5) * 0.7 - outside * 0.45, 0.0, 1.0) * amt;
        } else if kind == 2u {
            // sand drifts on floors and ledges
            w = smoothstep(0.55, 0.85, up + (an - 0.5) * 0.4) * amt;
        } else if kind == 5u {
            // ore-stained patches and streaks
            w = smoothstep(0.6, 0.8, an) * amt;
        }
        s = over(s, acc, w);
        if kind == 1u {
            s.rough = mix(s.rough, 0.12, w * 0.8);
        }
    }
    if kind == 3u {
        // embers: a few hair-thin cracks still glowing, low on the walls deep in the tube (the
        // crack follows a fine noise's zero line, kept only where a slow noise allows it)
        let n1 = value_noise3(wp * vec3<f32>(1.7, 0.9, 1.7), 37u) - 0.5;
        let w1 = max(fwidth(n1), 1e-4);
        let line = 1.0 - smoothstep(0.0, w1 * 1.5, abs(n1));
        let spot = smoothstep(0.68, 0.82, value_noise3(wp * 0.11, 39u));
        let crack = line * spot * smoothstep(0.75, 0.2, up) * (1.0 - smoothstep(0.02, 0.2, day));
        let flick = 0.7 + 0.3 * value_noise2(vec2<f32>(globals.time * 0.8, wp.x * 0.3 + wp.z * 0.3), 41u);
        emissive += vec3<f32>(1.0, 0.3, 0.05) * crack * amt * flick * 0.5;
        s.albedo *= 1.0 - crack * 0.5;
    }
    // (crystal caves: the crystals themselves are objects that glow and light the chambers)

    // ---- rock tops out in the open: grass and soil like the hillside round them
    let top = outside * smoothstep(0.62, 0.9, up + (s.height - 0.5) * 0.3) * (1.0 - skin);
    if top > 0.001 && cave.misc.z > 0.5 {
        let gbw = vec3<f32>(bw.x * 0.2, bw.y + 0.0001, bw.z * 0.2) / (bw.x * 0.2 + bw.y + 0.0001 + bw.z * 0.2);
        let gg = tri(0, wp, gbw, cave.ground_tile.w, dpx, dpy, true);
        let gd = tri(3, wp, gbw, cave.ground_tile.y, dpx, dpy, true);
        s = over(s, over(gd, gg, smoothstep(0.35, 0.65, value_noise2(wp.xz * 0.15, 61u))), top);
    }

    // ---- the ground round a mouth: the terrain's own layers (rock on slopes, dirt and gravel on
    // the flat), so it meets the terrain without a seam
    if skin > 0.001 {
        var g: Surf;
        if cave.misc.z > 0.5 {
            // (as painted round the mouth: rock where the slope is steeper than ~33°, dirt on the
            // flat; the terrain's own layers, repeats, anti-tiling and broad tone)
            let gr = ground_layer(6, wp, n_geo, cave.ground_tile.x, dpx, dpy, dist);
            let gd = ground_layer(3, wp, n_geo, cave.ground_tile.y, dpx, dpy, dist);
            g = over(gr, gd, smoothstep(0.78, 0.9, up));
            g.albedo *= 0.9 + 0.2 * fbm2(wp.xz * 0.0018, 3u, 91u);
        } else {
            g = plain(wp, vec3<f32>(0.9, 0.85, 0.75));
        }
        s = mix_surf(s, g, skin);
    }

    // ---- damp: darker, glossier, drip streaks down the walls, wet floors
    var wet = clamp(damp * th.tint.a + cave.look.z * day * 0.8, 0.0, 1.0) * (1.0 - skin * 0.7);
    let wall = 1.0 - abs(up);
    let streak = smoothstep(0.55, 0.85, value_noise2(vec2<f32>((wp.x + wp.z) * 2.3, wp.y * 0.12), 59u)) * wall;
    wet = clamp(wet + streak * th.tint.a * 0.6 * (1.0 - day * 0.5), 0.0, 1.0);
    s.albedo *= mix(1.0, 0.62, wet);
    s.rough = mix(s.rough, mix(0.22, 0.08, th.fx.z), wet * 0.85);

    // ---- normal
    var n = n_geo;
    if tex_ok {
        n = normalize(n_geo + s.pert * cave.look.x);
    }

    // ---- light: sky light only as far as daylight reaches in; crevices darker
    let ao = vao * mix(1.0, s.ao, 0.8);
    let amb = smoothstep(0.0, 0.85, day);
    pbr_input.material.base_color = vec4<f32>(s.albedo, 1.0);
    pbr_input.material.perceptual_roughness = clamp(s.rough, 0.04, 1.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.diffuse_occlusion = min(pbr_input.diffuse_occlusion, vec3<f32>(ao)) * amb; // keeps the screen-space AO
    pbr_input.specular_occlusion = ao * mix(0.05, 1.0, amb);
    pbr_input.world_normal = n_geo;
    pbr_input.N = n;
    // the cave's fill (dark / dim / readable): light that follows the exposure — from out in
    // the sun a cave mouth is black, and once inside the eye adjusts and the chamber reads
    // (`CAVE_EV` opens the exposure; this is how bright it looks at that adjusted exposure);
    // a soft top light keeps the form readable (floors a little brighter than ceilings)
    let fillk = uv.y * (1.0 - amb * 0.85) * mix(0.3, 1.0, ao) * (0.72 + 0.28 * up);
    let adjusted = view.exposure / CAVE_EXPOSURE;
    emissive += s.albedo * cave.fill.rgb * fillk * 0.62 * min(adjusted, 4.0);
    // x-ray (editing from above): lit from overhead so every chamber reads
    if cave.misc.y > 0.5 {
        emissive += s.albedo * (0.16 + 0.22 * max(up, 0.0)) * mix(0.5, 1.0, ao);
    }
    pbr_input.material.emissive = vec4<f32>(emissive, 0.0);

    // moving cloud shadows (as on the terrain)
    let sun_dir = normalize(cave.sun.xyz);
    let to_cloud = (2000.0 - wp.y) / max(sun_dir.y, 0.05);
    let cloud_p = wp.xz + sun_dir.xz * to_cloud;
    let cloud = smoothstep(0.25, 0.7, cloud_density(cloud_p, cave.clouds.x, cave.clouds.zw));
    pbr_input.directional_shadow_factor = 1.0 - cloud * cave.sun.w;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
