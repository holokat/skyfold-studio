// Ore deposits (src/gameplay/deposits.rs): a photoscanned rock with an ore in it. Everything is
// worked out in the rock's own space (metres, shifted per deposit), so the ore stays put on the
// rock and no two deposits look alike. On top of the scan's albedo, normal map and roughness:
//
// - Vein: a few metal veins crossing the rock (warped sheets that pinch and swell, with veinlets
//   branching off), a halo around them (patina, rust, tarnish), domed nuggets along them and fine
//   grains in the halo;
// - Seam: black layers dipping through the rock (coal);
// - Plain: granite;
// - Glass: patches of black volcanic glass with conchoidal ripples;
// - Crystal: faceted crystal patches, glowing from within;
// - Runes: veins with angular glyphs carved along them, glowing.
//
// The veins, nuggets, seams, glass and granite are filled with the ore's photoscanned surface
// (`fill`: native copper, gold, raw metal, coal…), the halo band and the stains with a second one
// (`halo_fill`: verdigris, rust, quartz…): CC0 scans in texture arrays, mapped triplanar in the
// rock's space at their real size and recoloured towards the ore's colours as far as the ore asks.
// Their height decides where a mask's edge falls, so crusts and metal break up along the scan's own
// shapes. From afar, where their detail is below a pixel, their mean colour stands in (no samples);
// ores without fills draw flat colours.
//
// Effects: SHIFT (the colour turns between two with the view angle), STARS (bright specks),
// EMBERS (glowing cracks), RIM (glow on the veins' edges), QUARTZ (the veins are white quartz
// with the metal in specks), STAIN (the ore's weathering colour stains patches of the rock).
// Shards (crystal clusters growing out of the rock) glow towards their tips. Glow is in cd/m²
// under the camera's exposure: faint by day, glowing at dusk. Fine detail fades out once it gets
// smaller than a pixel. Icons (thumbnails) pack the veins closer.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_functions,
};
import super::noise::{value_noise3, hash3f, hashf};

// A photoscanned surface filling part of the deposit.
struct Fill {
    // rgb the scan's mean colour (linear), w its texture-array layer (-1: a flat colour)
    mean: vec4<f32>,
    // x mean roughness, y mean metalness, z how far it's recoloured to the ore, w metres per repeat
    pbr: vec4<f32>,
}

struct OreLook {
    // rgb ore colour (linear), w metallic
    ore: vec4<f32>,
    // rgb halo / second colour, w the ore's roughness
    halo: vec4<f32>,
    // rgb glow × cd/m², w how much of the scan's own colour the host keeps
    glow: vec4<f32>,
    // rgb host tint, w vein coverage
    host: vec4<f32>,
    // x nuggets, y style, z effect bits, w 1 = crystal shard, 0.25 = an icon (more ore on it)
    shape: vec4<f32>,
    // the veins and nuggets (seams, glass, granite)
    fill: Fill,
    // the halo band and stains (QUARTZ: the veins)
    halo_fill: Fill,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> ore: OreLook;
// the photoscans, a layer each: rgb albedo + a height, xy tangent normal, x roughness + y metalness
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var scan_albedo: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var scan_normal: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var scan_pbr: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var scan_sampler: sampler;

const SHIFT: u32 = 1u;
const STARS: u32 = 2u;
const EMBERS: u32 = 4u;
const RIM: u32 = 8u;
const QUARTZ: u32 = 16u;
const STAIN: u32 = 32u;

fn ridge(p: vec3<f32>, seed: u32) -> f32 {
    return 1.0 - abs(2.0 * value_noise3(p, seed) - 1.0);
}

// The normal tilted by a relief `h` (m) drawn on the surface (bump mapping from screen-space
// derivatives: needs uniform control flow).
fn bump(n: vec3<f32>, wp: vec3<f32>, h: f32) -> vec3<f32> {
    let dpx = dpdx(wp);
    let dpy = dpdy(wp);
    let r1 = cross(dpy, n);
    let r2 = cross(n, dpx);
    let det = dot(dpx, r1);
    let grad = sign(det) * (dpdx(h) * r1 + dpdy(h) * r2);
    return normalize(abs(det) * n - grad);
}

// Distance (m) to the nearest of parallel sheets `period` metres apart, along a coordinate.
fn sheet_dist(x: f32, period: f32) -> f32 {
    return abs(fract(x / period) - 0.5) * period;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The nearest point of a jittered 3D grid: xyz = p - that point (cell units), w = its random id.
fn cell3(p: vec3<f32>, seed: u32) -> vec4<f32> {
    let i = floor(p);
    let f = p - i;
    var best = 9.0;
    var out = vec4<f32>(0.0);
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let cx = i32(i.x) + x;
                let cy = i32(i.y) + y;
                let cz = i32(i.z) + z;
                let o = vec3<f32>(hash3f(cx, cy, cz, seed), hash3f(cx, cy, cz, seed + 1u), hash3f(cx, cy, cz, seed + 2u));
                let d = f - (vec3<f32>(f32(x), f32(y), f32(z)) + 0.15 + o * 0.7);
                let dd = dot(d, d);
                if dd < best {
                    best = dd;
                    out = vec4<f32>(d, hash3f(cx, cy, cz, seed + 3u));
                }
            }
        }
    }
    return out;
}

fn seg_dist(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let t = clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-6), 0.0, 1.0);
    return length(p - (a + ab * t));
}

// Angular rune glyphs on a grid of `size`-metre cells: 0..1 coverage of the strokes.
fn runes(uv: vec2<f32>, size: f32, width: f32) -> f32 {
    let g = uv / size;
    let c = floor(g);
    let f = fract(g);
    let ci = vec2<i32>(i32(c.x), i32(c.y));
    if hashf(ci.x, ci.y, 811u) > 0.62 {
        return 0.0;
    }
    // strokes between points of a 3 × 3 lattice inside the cell
    var d = 9.0;
    let k = u32(hashf(ci.x, ci.y, 812u) * 65535.0);
    for (var s = 0u; s < 4u; s++) {
        let hs = (k >> (s * 4u)) & 15u;
        let a = vec2<f32>(f32(hs % 3u), f32((hs / 3u) % 3u));
        let hb = ((k * 2654435761u) >> (s * 5u + 3u)) & 15u;
        let b = vec2<f32>(f32(hb % 3u), f32((hb / 3u + 1u) % 3u));
        if s == 3u && (k & 1u) == 0u {
            break;
        }
        d = min(d, seg_dist(f, vec2<f32>(0.22) + a * 0.28, vec2<f32>(0.22) + b * 0.28));
    }
    let w = width / size;
    return 1.0 - smoothstep(w * 0.55, w, d);
}

// Where on the rock (its space, metres, and the change per pixel), its geometric normal there,
// the triplanar weights and how much of the photoscans' detail still shows (1 near … 0 far).
struct Frame {
    p: vec3<f32>,
    dx: vec3<f32>,
    dy: vec3<f32>,
    ln: vec3<f32>,
    w: vec3<f32>,
    near: f32,
}

// A photoscanned surface at one spot (its normal in the rock's space, height 0..1 around 0.5).
struct Surface {
    albedo: vec3<f32>,
    n: vec3<f32>,
    rough: f32,
    metal: f32,
    h: f32,
}

struct Taps {
    a: vec4<f32>,
    n: vec3<f32>,
    r: vec2<f32>,
}

// One projection of a scan (explicit gradients: these run under per-pixel branches).
fn taps(uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>, layer: i32) -> Taps {
    var t: Taps;
    t.a = textureSampleGrad(scan_albedo, scan_sampler, uv, layer, gx, gy);
    let xy = textureSampleGrad(scan_normal, scan_sampler, uv, layer, gx, gy).xy * 2.0 - 1.0;
    t.n = vec3<f32>(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
    t.r = textureSampleGrad(scan_pbr, scan_sampler, uv, layer, gx, gy).xy;
    return t;
}

// A fill at this spot: its scan mapped triplanar (shifted by `shift` so two fills never line
// up), recoloured towards the `want`ed colour and roughness as far as the fill asks; its mean from
// afar. Each projection's texture v runs down the rock, so the scans' up stays up on the sides;
// the normals whiteout-blend onto the geometric normal.
fn surface(f: Fill, fr: Frame, shift: vec3<f32>, want: vec3<f32>, want_rough: f32) -> Surface {
    let tint = mix(vec3<f32>(1.0), want / max(f.mean.rgb, vec3<f32>(1e-3)), f.pbr.z);
    let rough_shift = (want_rough - f.pbr.x) * f.pbr.z;
    var s: Surface;
    s.albedo = f.mean.rgb * tint;
    s.n = fr.ln;
    s.rough = f.pbr.x + rough_shift;
    s.metal = f.pbr.y;
    s.h = 0.5;
    if f.mean.w < 0.0 || fr.near <= 0.0 {
        return s;
    }
    let layer = i32(f.mean.w + 0.5);
    let k = 1.0 / f.pbr.w;
    let p = (fr.p + shift) * k;
    let dx = fr.dx * k;
    let dy = fr.dy * k;
    var a = vec4<f32>(0.0);
    var r = vec2<f32>(0.0);
    var n = vec3<f32>(0.0);
    if fr.w.x > 0.0 {
        let t = taps(vec2<f32>(p.z, -p.y), vec2<f32>(dx.z, -dx.y), vec2<f32>(dy.z, -dy.y), layer);
        a += t.a * fr.w.x;
        r += t.r * fr.w.x;
        n += vec3<f32>(t.n.xy + fr.ln.zy, abs(t.n.z) * fr.ln.x).zyx * fr.w.x;
    }
    if fr.w.y > 0.0 {
        let t = taps(vec2<f32>(p.x, -p.z), vec2<f32>(dx.x, -dx.z), vec2<f32>(dy.x, -dy.z), layer);
        a += t.a * fr.w.y;
        r += t.r * fr.w.y;
        n += vec3<f32>(t.n.xy + fr.ln.xz, abs(t.n.z) * fr.ln.y).xzy * fr.w.y;
    }
    if fr.w.z > 0.0 {
        let t = taps(vec2<f32>(p.x, -p.y), vec2<f32>(dx.x, -dx.y), vec2<f32>(dy.x, -dy.y), layer);
        a += t.a * fr.w.z;
        r += t.r * fr.w.z;
        n += vec3<f32>(t.n.xy + fr.ln.xy, abs(t.n.z) * fr.ln.z) * fr.w.z;
    }
    s.albedo = mix(s.albedo, min(a.rgb * tint, vec3<f32>(1.0)), fr.near);
    s.n = normalize(mix(fr.ln, normalize(n), fr.near));
    s.rough = mix(s.rough, r.x + rough_shift, fr.near);
    s.metal = mix(s.metal, r.y, fr.near);
    s.h = mix(0.5, a.w, fr.near);
    return s;
}

// How much of a mask a fill covers when its height decides the edge: the crust, metal or quartz
// takes the spots it rises out of first, so the edge follows the scan's own shapes.
fn edge(mask: f32, h: f32, sharp: f32) -> f32 {
    return clamp((mask + (h - 0.5) * 1.2 - 0.5) * sharp + 0.5, 0.0, 1.0);
}

fn has(f: Fill) -> bool {
    return f.mean.w >= 0.0;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr = pbr_input_from_standard_material(in, is_front);
@if(VERTEX_OUTPUT_INSTANCE_INDEX) {
    let m = mesh_functions::get_world_from_local(in.instance_index);
    let rot = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
    let s = sqrt(max(dot(m[0].xyz, m[0].xyz), 1e-6));
    // the rock's own space in metres, shifted per deposit
    let lp = transpose(rot) * (in.world_position.xyz - m[3].xyz) / s;
    let p = lp + fract(m[3].xyz * vec3<f32>(0.0131, 0.0173, 0.0157)) * 61.0;
    // metres per pixel here: detail smaller than that fades out
    let px = max(length(fwidth(lp)), 1e-5);
    let style = u32(ore.shape.y + 0.5);
    let fx = u32(ore.shape.z + 0.5);
    let V = pbr.V;

    if ore.shape.w > 0.5 {
        // ---- a crystal shard: dark rock at its foot, the crystal glowing towards the tips
        var h = 0.6;
        var facet = 0.5;
@if(VERTEX_UVS_A) {
        h = in.uv.y;
        facet = in.uv.x;
}
        let body = smoothstep(0.02, 0.2, h);
        var c = ore.ore.rgb;
        if (fx & SHIFT) != 0u {
            c = mix(ore.ore.rgb, ore.halo.rgb, smoothstep(0.3, 0.7, fract(facet * 3.7)));
        }
        let fres = pow(1.0 - clamp(dot(pbr.N, V), 0.0, 1.0), 3.0);
        // growth striations across the crystal and cloudy inclusions inside it
        let stria = 0.5 + 0.5 * sin(lp.y * 140.0 + value_noise3(lp * 9.0, 121u) * 6.0);
        let cloud = value_noise3(lp * 5.0 + vec3<f32>(facet * 13.0), 123u);
        // deep colour with bright edges (glass), or crystallised metal for the metallic ores
        let metal_k = ore.ore.w;
        let glass = c * (0.07 + 0.14 * facet + 0.12 * cloud * cloud) + c * fres * 0.45 + vec3<f32>(0.03) * smoothstep(0.75, 0.95, cloud);
        let deep = mix(glass, c * (0.5 + 0.35 * facet) * (0.85 + 0.15 * stria), metal_k);
        pbr.material.base_color = vec4<f32>(mix(vec3<f32>(0.025, 0.022, 0.024), deep, body), 1.0);
        pbr.material.perceptual_roughness = mix(0.7, mix(0.035 + 0.06 * stria, ore.halo.w, metal_k), body);
        pbr.material.metallic = metal_k * body;
        pbr.material.reflectance = vec3<f32>(0.6);
        let inner = (0.08 + 0.9 * pow(h, 1.8)) * (0.5 + 0.9 * facet) * body;
        pbr.material.emissive = vec4<f32>(ore.glow.rgb * inner * (1.0 + 0.8 * fres), 1.0);
    } else {
        let scan = pbr.material.base_color.rgb;
        let host = mix(vec3<f32>(luma(scan)), scan, ore.glow.w) * ore.host.rgb;
        // the scan's light and dark, so what is painted on keeps the rock's grain (photoscanned
        // fills have their own: they keep a little of it, as shading)
        let grain = clamp(0.55 + 2.2 * luma(scan), 0.55, 1.35);
        let shade = mix(1.0, grain, 0.3);
        let cov = ore.host.w;
        var col = host;
        var rough = pbr.material.perceptual_roughness;
        var metal = 0.0;
        var refl = pbr.material.reflectance;
        var glow = vec3<f32>(0.0);
        var n = pbr.N;
        // relief (m) added to the scan's: raised veins, nuggets, crystals
        var hgt = 0.0;
        // cavities of the photoscanned fills (pits, cracks): less sky and sun reach into them
        var occl = 1.0;
        let fine_fade = 1.0 - smoothstep(0.004, 0.012, px);
        // a sheet lying along the surface would cover a whole face (below); the fills' projections
        let geo_n = normalize(transpose(rot) * normalize(in.world_normal));
        var tw = pow(abs(geo_n), vec3<f32>(4.0));
        tw = max(tw / (tw.x + tw.y + tw.z) - 0.05, vec3<f32>(0.0));
        let fr = Frame(p, dpdx(p), dpdy(p), geo_n, tw / (tw.x + tw.y + tw.z), 1.0 - smoothstep(0.01, 0.025, px));

        // ---- where the ore runs: a few sheets crossing the rock (each deposit its own way),
        // warped and wiggling, pinching and swelling, ragged at the edges, with veinlets
        let rnd = fract(m[3].xyz * vec3<f32>(0.0131, 0.0173, 0.0157));
        let a1 = normalize(vec3<f32>(rnd.x - 0.5, 0.45 + rnd.y * 0.6, rnd.z - 0.5));
        let a2 = normalize(vec3<f32>(rnd.z - 0.5, rnd.x - 0.7, 0.6 - rnd.y));
        let a3 = normalize(cross(a1, a2) + vec3<f32>(0.3, 0.2, -0.4));
        let warp = vec3<f32>(value_noise3(p * 0.8, 21u), value_noise3(p * 0.8 + 17.3, 22u), value_noise3(p * 0.8 + 41.7, 23u)) - 0.5;
        let wiggle = vec3<f32>(value_noise3(p * 3.3, 24u), value_noise3(p * 3.3 + 7.1, 25u), value_noise3(p * 3.3 + 3.9, 26u)) - 0.5;
        let pw = lp + warp * 0.9 + wiggle * 0.07;
        let swell = 0.4 + 1.2 * value_noise3(p * 1.6, 33u);
        let quartz = (fx & QUARTZ) != 0u;
        // (quartz veins run a few centimetres to a decimetre wide)
        let wv = (0.006 + 0.026 * cov) * swell * select(1.0, 1.8, quartz);
        // (icons pack the veins closer, so a small picture shows the ore)
        let icon = ore.shape.w > 0.1;
        let per = select(1.0, 0.4, icon);
        let flat1 = smoothstep(0.75, 0.93, abs(dot(geo_n, a1)));
        let flat2 = smoothstep(0.75, 0.93, abs(dot(geo_n, a2)));
        let flat3 = smoothstep(0.75, 0.93, abs(dot(geo_n, a3)));
        let d1 = sheet_dist(dot(pw, a1) + rnd.y * 3.0, 1.25 * per) + flat1;
        let d2 = sheet_dist(dot(pw, a2) + rnd.z * 2.0, 1.7 * per) + flat2;
        let d3 = sheet_dist(dot(pw, a3) + rnd.x * 5.0, 0.8 * per) + flat3;
        // the second set only on richer ores, the third as thin veinlets; ragged edges
        let rag = (value_noise3(p * 26.0, 35u) - 0.5) * wv * 0.9 * fine_fade;
        let dv = min(d1, min(d2 + (0.7 - cov) * 0.25, d3 + wv * 0.6)) + rag;
        let r = ridge(p * 3.1 + warp * 2.0, 41u);
        let veinlet = smoothstep(0.965, 0.99, r) * (1.0 - smoothstep(wv * 3.0, wv * 8.0, dv)) * fine_fade;
        let vein = max(1.0 - smoothstep(wv * 0.55, wv, dv), veinlet * 0.9);
        // (a softer vein for the fills, whose height then decides the ragged edge)
        let vein_soft = max(1.0 - smoothstep(wv * 0.2, wv * 1.4, dv), veinlet * 0.9);
        let halo = 1.0 - smoothstep(wv, wv * 3.5, dv);
        let tone = value_noise3(p * 7.0, 51u);
        let lode = value_noise3(p * 0.42, 11u);
        // weathered staining over part of the rock (patina, rust…): the ore reads from afar
        let stain = smoothstep(0.5, 0.78, lode) * (0.5 + 0.5 * value_noise3(p * 2.3, 13u));

        // the ore's colour (turning with the view angle for SHIFT)
        var oc = ore.ore.rgb;
        var gc = ore.glow.rgb;
        if (fx & SHIFT) != 0u {
            let f = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 1.4);
            let k = clamp(f * 1.6 + (value_noise3(p * 1.3, 53u) - 0.5) * 1.2, 0.0, 1.0);
            oc = mix(ore.ore.rgb, ore.halo.rgb, k);
            gc = mix(ore.glow.rgb, ore.halo.rgb * max(max(ore.glow.r, ore.glow.g), ore.glow.b), k);
        }
        // the fills' metalness: the ore's own over the vein scan's mean (a rock scan stays
        // stone); a halo scan recoloured into another mineral loses its bare metal
        let fill_metal = ore.ore.w / max(ore.fill.pbr.y, 0.05);
        let halo_metal = 1.0 - ore.halo_fill.pbr.z;

        if style == 1u {
            // ---- seams: black layers dipping through the rock (coal), shale between them
            let hh = dot(pw, a1) * 2.2 + (value_noise3(p * 1.1, 61u) - 0.5) * 1.1;
            let band = abs(fract(hh) - 0.5) * 2.0;
            let seam = smoothstep(0.66, 0.46, band) * smoothstep(0.2, 0.35, lode + cov * 0.4);
            let shale = smoothstep(0.9, 0.66, band) * (1.0 - seam);
            col = mix(col, ore.halo.rgb * (0.6 + 0.8 * luma(host)), shale * 0.6);
            if has(ore.fill) {
                var sv = 0.0;
                if seam > 0.0 {
                    let c = surface(ore.fill, fr, vec3<f32>(0.0), oc, ore.halo.w);
                    sv = edge(seam, c.h, 3.0);
                    col = mix(col, c.albedo * shade, sv);
                    rough = mix(rough, c.rough, sv);
                    n = normalize(mix(n, normalize(rot * c.n), sv * 0.9));
                }
                refl = mix(refl, vec3<f32>(0.35), sv);
                // the soft coal weathers back between harder layers
                hgt -= sv * 0.01;
            } else {
                let gloss = smoothstep(0.55, 0.75, value_noise3(p * 5.0, 63u));
                col = mix(col, oc * (0.8 + 0.5 * tone) * grain, seam);
                rough = mix(rough, mix(ore.halo.w + 0.2, 0.28, gloss), seam);
                refl = mix(refl, vec3<f32>(0.5), seam);
                hgt -= seam * 0.01;
            }
        } else if style == 2u {
            // ---- granite: fresh, crisp grain over most of the rock, the weathered scan in patches
            let tint = mix(vec3<f32>(luma(host)), host, 0.45) * 1.08;
            if has(ore.fill) {
                let fresh = 0.5 + 0.4 * smoothstep(0.35, 0.65, lode);
                let c = surface(ore.fill, fr, vec3<f32>(0.0), oc, ore.halo.w);
                col = mix(tint, c.albedo * mix(1.0, grain, 0.6), fresh);
                rough = mix(rough, c.rough, fresh);
                n = normalize(mix(n, normalize(rot * c.n), fresh * 0.6));
            } else {
                let fade = 1.0 - smoothstep(0.006, 0.02, px);
                let g1 = value_noise3(p * 34.0, 71u);
                let g2 = value_noise3(p * 27.0 + 3.1, 72u);
                let g3 = value_noise3(p * 41.0 + 7.7, 73u);
                col = tint;
                col = mix(col, tint * ore.halo.rgb * 2.2, smoothstep(0.66, 0.74, g2) * 0.55 * fade);
                col = mix(col, vec3<f32>(0.75) * max(luma(tint), 0.3) * 1.4, smoothstep(0.7, 0.78, g3) * 0.6 * fade);
                col = mix(col, tint * 0.22, smoothstep(0.74, 0.8, g1) * 0.85 * fade);
                rough = 0.72;
            }
        } else if style == 3u {
            // ---- volcanic glass in patches with conchoidal ripples
            let gn = value_noise3(p * 0.75 + warp * 0.8, 81u);
            let gp = smoothstep(0.42, 0.48, gn);
            let c = cell3(p * 2.2, 83u);
            let d = length(c.xyz);
            let ripple = sin(d * 38.0 + c.w * 6.0) * (1.0 - smoothstep(0.003, 0.01, px));
            let dir = normalize(rot * c.xyz + vec3<f32>(1e-5));
            var glass = oc + vec3<f32>(0.004, 0.003, 0.006) * c.w;
            var glass_rough = ore.halo.w;
            if has(ore.fill) && gp > 0.0 {
                // the glass's faint flow banding
                let g = surface(ore.fill, fr, vec3<f32>(0.0), oc, ore.halo.w);
                glass = g.albedo;
                glass_rough = g.rough;
                n = normalize(mix(n, normalize(rot * g.n), gp * 0.5));
            }
            n = normalize(mix(n, normalize(n + dir * (0.35 + 0.18 * ripple)), gp));
            col = mix(col * 0.55, glass, gp);
            rough = mix(rough, glass_rough + 0.03 * abs(ripple), gp);
            refl = mix(refl, vec3<f32>(0.75), gp);
            // a thin weathered rind where glass meets rock
            col = mix(col, ore.halo.rgb, (smoothstep(0.38, 0.42, gn) - gp) * 0.6);
        } else if style == 4u {
            // ---- crystal veins swelling into crystal-lined lenses, glowing from within
            let pk = cell3(p * 2.6, 93u);
            let lens = (1.0 - smoothstep(0.1, 0.45, length(pk.xyz))) * step(pk.w, 0.6);
            let wc = wv * (1.0 + 3.5 * lens);
            let cp = 1.0 - smoothstep(wc * 0.5, wc * 0.9, dv);
            let pocket = cp * lens;
            let c = cell3(p * 16.0, 91u);
            let dir = normalize(vec3<f32>(c.w - 0.5, fract(c.w * 7.31) - 0.5, fract(c.w * 13.7) - 0.5));
            n = normalize(mix(n, normalize(n + rot * dir * 1.2), pocket * fine_fade));
            let edge_k = smoothstep(0.06, 0.25, length(c.xyz));
            let fres = pow(1.0 - clamp(dot(n, V), 0.0, 1.0), 3.0);
            col = mix(col, ore.halo.rgb * (0.4 + 0.8 * luma(host)), clamp(halo - cp, 0.0, 1.0) * 0.5);
            col = mix(col, oc * (0.22 + 0.22 * fract(c.w * 3.3)) * (0.55 + 0.45 * edge_k) + oc * fres * 0.4, cp);
            rough = mix(rough, ore.halo.w, cp);
            refl = mix(refl, vec3<f32>(0.8), cp);
            glow += gc * cp * (0.2 + 0.5 * fract(c.w * 5.1)) * edge_k;
            hgt += pocket * 0.012 * (0.5 + 0.5 * fract(c.w * 2.9));
        } else {
            // ---- metal veins (and runes): the halo, the veins, nuggets and grains along them
            let near_vein = 1.0 - smoothstep(wv * 1.5, wv * 4.0, dv);
            let band = clamp(halo - vein, 0.0, 1.0);
            let stained_k = select(0.0, stain, (fx & STAIN) != 0u);
            // (a shifting ore's halo is its second colour, not a stain)
            let shift = (fx & SHIFT) != 0u;
            if has(ore.halo_fill) && !shift {
                // the halo scan: a crust over the band and the stains (patina, rust…), white
                // quartz in the veins of QUARTZ ores. The crust is thick along some stretches of
                // the vein, thin or gone along others, and reaches out further in places.
                let reach = 1.6 + 3.2 * value_noise3(p * 1.9, 37u);
                let thick = 0.2 + 0.9 * value_noise3(p * 3.7 + 9.1, 38u);
                let crust_k = clamp((1.0 - smoothstep(wv, wv * reach, dv)) * thick - vein, 0.0, 1.0);
                var crust = 0.0;
                var qv = 0.0;
                if crust_k + stained_k + select(0.0, vein, quartz) > 0.0 {
                    let c = surface(ore.halo_fill, fr, vec3<f32>(17.3, 5.1, 29.7), ore.halo.rgb, ore.halo_fill.pbr.x);
                    // crusts gather in centimetre-sized patches (the scan's height and a little noise)
                    let ch = mix(c.h, value_noise3(p * 14.0, 39u), 0.4);
                    crust = max(edge(crust_k * 0.85, ch, 3.0) * select(0.9, 0.25, quartz), edge(stained_k * 0.7, ch, 3.0) * select(0.6, 0.85, icon));
                    if quartz {
                        qv = edge(vein_soft, c.h, 2.5);
                    }
                    let k = max(crust, qv);
                    col = mix(col, c.albedo * shade, k);
                    rough = mix(rough, c.rough, k);
                    metal = mix(metal, c.metal * halo_metal, k);
                    n = normalize(mix(n, normalize(rot * c.n), max(crust * 0.7, qv)));
                }
                // crusts and quartz stand a little proud of the rock
                hgt += crust * 0.0015 + qv * 0.003;
            } else {
                let stained = select(ore.halo.rgb * (0.45 + 0.9 * luma(host)), host * 0.6, shift);
                col = mix(col, stained, stained_k * select(0.3, 0.5, icon));
                col = mix(col, stained, band * select(0.45, 0.12, quartz));
                if quartz {
                    col = mix(col, ore.halo.rgb * (0.7 + 0.25 * tone) * grain, vein);
                    rough = mix(rough, 0.35, vein);
                }
            }
            // the ore's own scan: in the veins (not QUARTZ's), nuggets and grains
            var c: Surface;
            c.albedo = oc * (0.6 + 0.45 * tone) * grain;
            c.n = geo_n;
            c.rough = ore.halo.w + 0.18 * tone;
            c.metal = ore.ore.w;
            c.h = 0.5;
            let scanned = has(ore.fill);
            if scanned && max(select(vein, 0.0, quartz), near_vein) > 0.0 {
                c = surface(ore.fill, fr, vec3<f32>(0.0), oc, ore.halo.w);
                c.albedo *= shade;
                c.metal = clamp(c.metal * fill_metal, 0.0, 1.0);
            }
            if !quartz {
                let v = select(vein, edge(vein_soft, c.h, 2.5), scanned);
                col = mix(col, c.albedo, v);
                metal = mix(metal, c.metal, v);
                rough = mix(rough, c.rough, v);
                if scanned {
                    n = normalize(mix(n, normalize(rot * c.n), v * 0.9));
                    occl = mix(occl, clamp(0.3 + 1.4 * c.h, 0.45, 1.0), v);
                    hgt += v * (c.h - 0.5) * 0.006;
                }
            }
            // harder than the rock around them: the veins stand a little proud
            hgt += vein * 0.004;
            // (nuggets and grains are too small to see beyond a few tens of metres: skipped there)
            if ore.shape.x > 0.0 && px < 0.03 {
                // (warped, so nuggets come out as irregular blebs and flakes, not discs)
                let nw = vec3<f32>(value_noise3(p * 22.0, 108u), value_noise3(p * 22.0 + 5.3, 109u), value_noise3(p * 22.0 + 11.9, 110u)) - 0.5;
                let cn = cell3(p * 9.0 + nw * 0.7 * fine_fade, 101u);
                // lumpy, every nugget its own size
                let lump = 1.0 + 0.8 * (value_noise3(p * 35.0, 105u) - 0.5) * fine_fade;
                let d = length(cn.xyz) * lump;
                let rad = 0.12 + 0.2 * fract(cn.w * 9.1) * fract(cn.w * 3.7);
                let present = step(cn.w, ore.shape.x * 0.75) * near_vein * (1.0 - smoothstep(0.012, 0.03, px));
                // (a scanned metal's nuggets are hackly: its height breaks up their outline)
                let disc = 1.0 - smoothstep(rad * 0.7, rad, d);
                let nug = select(disc, edge(1.0 - smoothstep(rad * 0.4, rad * 1.15, d), c.h, 2.5), scanned) * present;
                hgt += present * sqrt(max(1.0 - (d / rad) * (d / rad), 0.0)) * rad * 0.06;
                // fine grains scattered in the halo
                let gcell = cell3(p * 34.0, 103u);
                let grn = step(gcell.w, ore.shape.x * 0.35) * (1.0 - smoothstep(0.14, 0.2, length(gcell.xyz) * lump)) * halo * (1.0 - smoothstep(0.003, 0.008, px));
                let ore_bit = max(nug, grn);
                if scanned {
                    col = mix(col, c.albedo * 1.1, ore_bit);
                    metal = mix(metal, c.metal, ore_bit);
                    rough = mix(rough, c.rough * 0.8, ore_bit);
                } else {
                    col = mix(col, oc * (0.85 + 0.3 * tone), ore_bit);
                    metal = mix(metal, ore.ore.w, ore_bit);
                    rough = mix(rough, ore.halo.w * 0.7, ore_bit);
                }
                glow += gc * nug * 0.5;
            }
            if style == 5u {
                // runes along the veins: project on the face the surface looks along
                let ln = abs(transpose(rot) * n);
                var uv = p.xz;
                if ln.x > ln.y && ln.x > ln.z {
                    uv = p.zy;
                } else if ln.z > ln.y {
                    uv = p.xy;
                }
                let g = runes(uv, 0.2, max(0.012, px * 1.5)) * smoothstep(0.15, 0.6, halo);
                col = mix(col, oc * 0.35, g);
                rough = mix(rough, 0.35, g);
                glow += gc * (g * 1.2 + vein * 0.2);
                hgt -= g * 0.004;
            } else if (fx & RIM) != 0u {
                glow += gc * clamp(halo - vein * 0.8, 0.0, 1.0) * (0.35 + 0.65 * tone);
            } else if (fx & EMBERS) != 0u {
                let crack = smoothstep(0.965, 0.99, ridge(p * 6.0 + warp * 3.0, 97u)) * smoothstep(0.2, 0.7, halo);
                // still hot inside: dull orange by day, glowing at night
                col = mix(col, gc / max(max(gc.r, gc.g), max(gc.b, 1e-4)) * 0.22, crack);
                rough = mix(rough, 0.8, crack);
                metal = mix(metal, 0.0, crack);
                glow += gc * (crack * (0.7 + 0.6 * tone) + vein * 0.06);
                hgt -= crack * 0.005;
            } else if (fx & STARS) == 0u {
                glow += gc * vein * 0.6;
            }
            if (fx & STARS) != 0u && px < 0.05 {
                // star specks (the only glow): in and around the veins, a few over the whole rock
                let cs = cell3(p * 18.0, 107u);
                let rad = max(0.07 + 0.1 * fract(cs.w * 7.3), px * 18.0 * 0.9);
                let star = step(cs.w, 0.45) * (1.0 - smoothstep(rad * 0.4, rad, length(cs.xyz))) * clamp(smoothstep(0.0, 0.4, halo) + 0.25 * stain, 0.0, 1.0);
                col = mix(col, vec3<f32>(0.9), star);
                glow += ore.glow.rgb * star * (0.6 + 0.8 * fract(cs.w * 11.0));
            }
        }
        n = bump(n, in.world_position.xyz, hgt);
        pbr.material.base_color = vec4<f32>(col, 1.0);
        pbr.material.perceptual_roughness = clamp(rough, 0.03, 1.0);
        pbr.material.metallic = clamp(metal, 0.0, 1.0);
        pbr.material.reflectance = refl;
        pbr.material.emissive = vec4<f32>(glow, 1.0);
        pbr.N = n;
        pbr.diffuse_occlusion *= occl;
        pbr.specular_occlusion *= occl;
    }
}
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr);
    out.color = main_pass_post_lighting_processing(pbr, out.color);
    return out;
}
