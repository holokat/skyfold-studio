// Sky bodies (`celestial`): planets, moons, distant stars and black holes on an inward dome drawn
// between the stars and the clouds. Each body is ray-traced inside its own patch of sky: a
// sphere at unit distance whose radius gives its angular size (so a looming giant keeps its true
// perspective), with a 3D procedural surface on it, lit by the world's sun (phases and a soft
// terminator with crater shadows), a scattering rim, a banded ring that shadows the body and is
// shadowed by it. Stars glow with a corona; a black hole bends light round its shadow and shows
// its Doppler-bright accretion disc above and below it.
//
// Output is premultiplied: the light is added over the sky (the day sky's own light lies in
// front of the bodies, so a planet by day is a pale ghost and its dark side is sky blue); alpha
// only hides the stars behind them at night.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};
import super::noise::{hash3f, fbm2, fbm3, value_noise3};

struct Body {
    // xyz: direction towards the centre, w: angular radius (rad)
    dir: vec4<f32>,
    // x: kind, y: seed, z: brightness, w: atmosphere
    info: vec4<f32>,
    // rgb: colour, w: ring opacity
    color: vec4<f32>,
    // rgb: second colour, w: ring inner radius (body radii)
    color2: vec4<f32>,
    // rgb: atmosphere colour, w: ring outer radius
    atmo: vec4<f32>,
    // xyz: spin axis, w: spin (rad)
    axis: vec4<f32>,
    // xyz: ring plane normal
    ring: vec4<f32>,
    ring_color: vec4<f32>,
}

struct Celestial {
    bodies: array<Body, 8>,
    // xyz: direction towards the sun, w: sunlight (lux)
    sun: vec4<f32>,
    // x: count, y: time, z: night, w: visibility
    params: vec4<f32>,
    // x: haze, y: star visibility
    air: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> cel: Celestial;

const PI: f32 = 3.14159265;
const MOON: i32 = 0;
const TERRAN: i32 = 1;
const GAS: i32 = 2;
const ICE: i32 = 3;
const LAVA: i32 = 4;
const DESERT: i32 = 5;
const STAR: i32 = 6;
const HOLE: i32 = 7;
// A bright emitter's light in display units never grows past this many times its daylight
// brightness when the exposure opens at night (else a star or a black hole's disc would burn
// the whole night sky white).
const EMIT_NIGHT_GAIN: f32 = 5.0;
// Sunlit bodies likewise: by night a giant lit like a full moon would fill the sky with white,
// so they brighten only this far past their daylight look (a very bright moonlit disc).
const LIT_NIGHT_GAIN: f32 = 2.2;
const DAY_EXPOSURE: f32 = 3.4e-5;

struct Shade {
    // premultiplied light (cd/m², before exposure)
    rgb: vec3<f32>,
    // how much it hides what lies behind (stars)
    cover: f32,
    // the part that glows by itself (exposure-limited separately)
    glow: vec3<f32>,
    // light already in display units (the bent stars, drawn like the night sky's)
    disp: vec3<f32>,
}

// ---------------------------------------------------------------------------- noise helpers

fn hash31(p: vec3<i32>, seed: u32) -> vec3<f32> {
    return vec3<f32>(hash3f(p.x, p.y, p.z, seed), hash3f(p.x, p.y, p.z, seed + 1u), hash3f(p.x, p.y, p.z, seed + 2u));
}

// Craters from a 3D cell grid: x = depth profile (bowl, raised rim, flat beyond), y = rim mask.
fn craters(p: vec3<f32>, seed: u32, density: f32) -> vec2<f32> {
    let c = floor(p);
    var h = 0.0;
    var rim = 0.0;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let cell = vec3<i32>(c) + vec3<i32>(x, y, z);
                let r = hash31(cell, seed);
                if r.x > density {
                    continue;
                }
                let centre = vec3<f32>(cell) + 0.2 + 0.6 * hash31(cell, seed + 7u);
                let size = 0.12 + 0.38 * r.y * r.y;
                let d = length(p - centre) / size;
                if d < 1.6 {
                    // bowl below the plain, a raised rim, ejecta fading out
                    let bowl = (d * d - 1.0) * 0.5;
                    let rimh = 0.35 * exp(-pow((d - 1.0) * 4.0, 2.0));
                    let prof = select(rimh * exp(-(d - 1.0) * 3.0), bowl + rimh, d < 1.0);
                    h += prof * size;
                    rim = max(rim, rimh / 0.35 * step(0.85, d));
                }
            }
        }
    }
    return vec2<f32>(h, rim);
}

// Distance to the nearest cell edge (cracks): small near the edges.
fn cracks(p: vec3<f32>, seed: u32) -> f32 {
    let c = floor(p);
    var d1 = 9.0;
    var d2 = 9.0;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let cell = vec3<i32>(c) + vec3<i32>(x, y, z);
                let o = vec3<f32>(cell) + hash31(cell, seed);
                let d = length(p - o);
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                } else if d < d2 {
                    d2 = d;
                }
            }
        }
    }
    return d2 - d1;
}

// ---------------------------------------------------------------------------- surfaces

struct Surface {
    albedo: vec3<f32>,
    // surface height (for the normal), in body radii
    height: f32,
    // self-lit light (cd/m²)
    emit: vec3<f32>,
    // 0 rough rock (moon-like, flat-lit), 1 smooth/cloudy (Lambert)
    smooth_: f32,
    // ocean (specular glint)
    wet: f32,
}

fn surface(kind: i32, q: vec3<f32>, b: Body, t: f32) -> Surface {
    var s: Surface;
    s.emit = vec3<f32>(0.0);
    s.wet = 0.0;
    s.smooth_ = 0.0;
    s.height = 0.0;
    let seed = u32(b.info.y);
    let c1 = b.color.rgb;
    let c2 = b.color2.rgb;
    let sp = q * 1.0 + vec3<f32>(f32(seed) * 1.37, f32(seed) * 0.71, f32(seed) * 2.13);
    if kind == MOON || kind == DESERT {
        // maria / dark plains and bright highlands, two scales of craters, fine regolith
        let maria = smoothstep(0.08, 0.22, fbm3(sp * 1.6, 5u, seed + 3u));
        let big = craters(sp * 3.0, seed + 11u, 0.55);
        let small = craters(sp * 11.0, seed + 23u, 0.7);
        let tiny = craters(sp * 34.0, seed + 31u, 0.8);
        let grain = fbm3(sp * 60.0, 3u, seed + 5u);
        s.height = big.x * 0.05 + small.x * 0.016 + tiny.x * 0.005 + fbm3(sp * 8.0, 4u, seed) * 0.012;
        var a = mix(c1, c2, maria) * (0.86 + 0.28 * grain);
        a = a * (1.0 + 0.35 * big.y + 0.2 * small.y);
        if kind == DESERT {
            // wind-streaked dunes, dark canyons, polar frost
            let lat = q.y;
            let streak = fbm2(vec2<f32>(sp.x * 3.0 + sp.z * 2.0, lat * 18.0), 4u, seed + 9u);
            a = a * (0.85 + 0.3 * streak);
            let canyon = 1.0 - smoothstep(0.0, 0.05, cracks(sp * 2.2, seed + 41u));
            a = mix(a, c2 * 0.55, canyon * 0.7);
            s.height = s.height - canyon * 0.02;
            let cap = smoothstep(0.82, 0.9, abs(lat) + fbm3(sp * 4.0, 3u, seed) * 0.15);
            a = mix(a, vec3<f32>(0.92, 0.9, 0.88), cap);
        }
        s.albedo = a;
    } else if kind == TERRAN {
        let land_n = fbm3(sp * 1.4, 6u, seed + 3u);
        let land = smoothstep(0.02, 0.05, land_n);
        let lat = abs(q.y);
        let dry = smoothstep(0.1, 0.35, fbm3(sp * 2.5, 4u, seed + 8u)) * (1.0 - smoothstep(0.2, 0.55, lat));
        var ground = mix(c2, vec3<f32>(0.6, 0.5, 0.36), dry);
        ground = ground * (0.8 + 0.4 * fbm3(sp * 12.0, 3u, seed + 4u));
        let ocean = c1 * (0.75 + 0.25 * smoothstep(-0.25, 0.0, land_n));
        var a = mix(ocean, ground, land);
        let ice = smoothstep(0.78, 0.86, lat + fbm3(sp * 3.0, 3u, seed) * 0.12);
        a = mix(a, vec3<f32>(0.9, 0.92, 0.95), ice);
        // weather: swirled cloud bands drifting slowly
        let w = sp * 2.2 + vec3<f32>(t * 0.002, 0.0, 0.0);
        let warp = vec3<f32>(fbm3(w * 1.3, 3u, seed + 50u), 0.0, fbm3(w * 1.3 + 5.0, 3u, seed + 51u));
        let cloud = smoothstep(0.02, 0.22, fbm3(w + warp * 1.5, 6u, seed + 52u) + 0.06 * cos(q.y * 9.0));
        a = mix(a, vec3<f32>(0.95), cloud * 0.85);
        s.albedo = a;
        s.wet = (1.0 - land) * (1.0 - cloud) * (1.0 - ice);
        s.smooth_ = 1.0;
        s.height = land * 0.004;
    } else if kind == GAS {
        // latitude bands bent by turbulence, a great oval storm, fine streaks
        let lat = q.y;
        let turb = fbm3(sp * vec3<f32>(2.0, 6.0, 2.0) + vec3<f32>(t * 0.001, 0.0, 0.0), 5u, seed + 3u);
        let band = sin(lat * 22.0 + turb * 3.5 + fbm3(sp * 0.7, 2u, seed + 4u) * 4.0);
        let fine = fbm2(vec2<f32>(atan2(q.z, q.x) * 6.0 + turb * 4.0, lat * 80.0), 4u, seed + 6u);
        var a = mix(c1, c2, 0.5 + 0.5 * band);
        a = a * (0.85 + 0.3 * fine);
        let so = normalize(vec3<f32>(0.9, -0.28, 0.35));
        let sd = length((q - so) * vec3<f32>(0.55, 1.4, 0.55));
        let storm = 1.0 - smoothstep(0.1, 0.17, sd + turb * 0.04);
        a = mix(a, c2 * vec3<f32>(1.15, 0.75, 0.6), storm * 0.8);
        s.albedo = a;
        s.smooth_ = 1.0;
    } else if kind == ICE {
        let cr = cracks(sp * 3.0, seed + 3u);
        let fine = cracks(sp * 11.0, seed + 5u);
        let line = (1.0 - smoothstep(0.0, 0.04, cr)) * 0.8 + (1.0 - smoothstep(0.0, 0.03, fine)) * 0.35;
        var a = mix(c1, c2, smoothstep(0.0, 0.3, fbm3(sp * 2.0, 5u, seed + 7u)));
        a = mix(a, c2 * 0.55 + vec3<f32>(0.1, 0.05, 0.0), line);
        s.albedo = a;
        s.height = -line * 0.004 + fbm3(sp * 9.0, 3u, seed) * 0.004;
        s.smooth_ = 0.6;
    } else if kind == LAVA {
        let cr = cracks(sp * 2.6, seed + 3u);
        let fine = cracks(sp * 9.0, seed + 5u);
        let flow = (1.0 - smoothstep(0.0, 0.06, cr)) + (1.0 - smoothstep(0.0, 0.035, fine)) * 0.5;
        let crust = fbm3(sp * 14.0, 4u, seed + 9u);
        s.albedo = c1 * (0.7 + 0.6 * crust);
        let pulse = 0.85 + 0.15 * sin(t * 0.6 + crust * 12.0);
        s.emit = c2 * c2 * flow * 2600.0 * pulse * b.info.z;
        s.height = fbm3(sp * 6.0, 4u, seed) * 0.02 - flow * 0.006;
    }
    return s;
}

// Shadow cast by the ring on a point of the body (1 = no shadow).
fn ring_shadow(p: vec3<f32>, b: Body, c: vec3<f32>, rad: f32, l: vec3<f32>) -> f32 {
    let op = b.color.w;
    if op <= 0.0 {
        return 1.0;
    }
    let rn = b.ring.xyz;
    let den = dot(l, rn);
    if abs(den) < 1.0e-4 {
        return 1.0;
    }
    let tt = dot(c - p, rn) / den;
    if tt <= 0.0 {
        return 1.0;
    }
    let rho = length(p + l * tt - c) / rad;
    return 1.0 - ring_density(rho, b) * op * 0.85;
}

fn ring_density(rho: f32, b: Body) -> f32 {
    let r0 = b.color2.w;
    let r1 = b.atmo.w;
    if rho < r0 || rho > r1 {
        return 0.0;
    }
    let u = (rho - r0) / (r1 - r0);
    let seed = u32(b.info.y);
    // ringlets and gaps: several frequencies, a dark division two thirds out
    let bands = fbm2(vec2<f32>(u * 40.0, 0.5), 4u, seed + 60u) * 0.9 + fbm2(vec2<f32>(u * 190.0, 1.5), 2u, seed + 61u) * 0.5;
    var d = smoothstep(0.0, 0.5, bands + 0.25);
    d = d * (1.0 - 0.85 * exp(-pow((u - 0.64) * 30.0, 2.0)));
    d = d * smoothstep(0.0, 0.04, u) * smoothstep(1.0, 0.9, u);
    return clamp(d, 0.0, 1.0);
}

// A planet, a moon: ray-traced sphere, surface, light, rim, ring.
fn planet(dir: vec3<f32>, b: Body, kind: i32, px: f32, sun: vec3<f32>, e_sun: f32, t: f32) -> Shade {
    var o: Shade;
    o.rgb = vec3<f32>(0.0);
    o.cover = 0.0;
    o.glow = vec3<f32>(0.0);
    o.disp = vec3<f32>(0.0);
    let c = normalize(b.dir.xyz);
    let ar = b.dir.w;
    let rad = sin(ar);
    let bb = dot(dir, c);
    let disc_h = bb * bb - 1.0 + rad * rad;
    let ang = acos(clamp(bb, -1.0, 1.0));
    // antialiased disc edge in screen pixels
    let cover = 1.0 - smoothstep(ar - px, ar + px, ang);

    // spin frame for the surface
    let ax = normalize(b.axis.xyz);
    let side = normalize(cross(ax, c));
    let fwd = cross(side, ax);
    let cs = cos(b.axis.w);
    let sn = sin(b.axis.w);

    var t_body = 1.0e9;
    var body = vec3<f32>(0.0);
    if cover > 0.0 {
        let tt = bb - sqrt(max(disc_h, 0.0));
        t_body = tt;
        let p = dir * tt;
        var n = normalize(p - c);
        // body-space coordinates (y along the spin axis, turned by the spin)
        let lx = dot(n, side);
        let ly = dot(n, ax);
        let lz = dot(n, fwd);
        let q = vec3<f32>(lx * cs - lz * sn, ly, lx * sn + lz * cs);
        let s = surface(kind, q, b, t);
        // bump: the surface height's slope along two tangents (relief at the terminator)
        if s.height != 0.0 {
            let e = 0.004;
            let ta = normalize(cross(n, ax + vec3<f32>(1.0e-3, 0.0, 0.0)));
            let tb = cross(n, ta);
            let qa = q + vec3<f32>(dot(ta, side) * cs - dot(ta, fwd) * sn, dot(ta, ax), dot(ta, side) * sn + dot(ta, fwd) * cs) * e;
            let qb = q + vec3<f32>(dot(tb, side) * cs - dot(tb, fwd) * sn, dot(tb, ax), dot(tb, side) * sn + dot(tb, fwd) * cs) * e;
            let ha = surface(kind, normalize(qa), b, t).height;
            let hb = surface(kind, normalize(qb), b, t).height;
            n = normalize(n - (ta * (ha - s.height) + tb * (hb - s.height)) / e * 2.5);
        }
        let mu0 = dot(n, sun);
        let mu = max(dot(n, -dir), 0.0);
        // rough regolith is lit flat to the limb (Lommel–Seeliger); clouds and gas are Lambert
        let ls = 2.0 * max(mu0, 0.0) / (max(mu0, 0.0) + mu + 1.0e-3) * 0.5;
        let lam = max(mu0, 0.0);
        var lit = mix(ls, lam, s.smooth_);
        // a soft terminator (the sun's own size and the scattered light)
        lit = lit * smoothstep(-0.04, 0.06, dot(normalize(p - c), sun));
        lit = lit * ring_shadow(p, b, c, rad, sun);
        var col = s.albedo * b.info.z * lit * e_sun / PI;
        // ocean glint
        if s.wet > 0.0 {
            let h = normalize(sun - dir);
            col += vec3<f32>(1.0, 0.95, 0.85) * s.wet * pow(max(dot(n, h), 0.0), 120.0) * lit * e_sun * 0.05;
        }
        // limb darkening of thick atmospheres
        if s.smooth_ > 0.5 {
            col = col * (0.55 + 0.45 * pow(mu, 0.35));
        }
        // faint light from the world below on the night side, so the shape still reads
        col += s.albedo * 0.0008 * e_sun / PI * (1.0 - lit) * 0.3;
        body = col;
        o.glow = s.emit * cover;
    }

    // scattering rim: the atmosphere shell lit by the sun, brightest towards the sunlit limb and
    // when the sun is behind the body (forward scattering)
    let atm = b.info.w;
    var rim = vec3<f32>(0.0);
    if atm > 0.0 {
        let h = ar * (0.035 + 0.05 * atm);
        let edge = (ang - ar) / h;
        let outer = exp(-max(edge, 0.0) * 2.5) * step(-0.0, edge);
        let inner = exp(-max(-edge, 0.0) * 6.0) * step(edge, 0.0);
        let shell = select(inner * 0.6, outer, edge > 0.0);
        let toward = normalize(dir - c * bb);
        let lit_side = smoothstep(-0.35, 0.6, dot(toward, sun) + 0.25);
        let forward = pow(max(dot(dir, sun), 0.0), 6.0) * 3.0;
        rim = b.atmo.rgb * shell * atm * (lit_side + forward) * e_sun / PI * 0.35;
    }

    // the ring: where the view ray crosses its plane
    var ring = vec4<f32>(0.0);
    var t_ring = 1.0e9;
    if b.color.w > 0.0 {
        let rn = normalize(b.ring.xyz);
        let den = dot(dir, rn);
        if abs(den) > 1.0e-5 {
            let tr = dot(c, rn) / den;
            if tr > 0.0 {
                let pr = dir * tr;
                let rho = length(pr - c) / rad;
                let d = ring_density(rho, b) * b.color.w;
                if d > 0.0 {
                    t_ring = tr;
                    // the body's shadow on the ring
                    let oc = pr - c;
                    let sb = dot(oc, sun);
                    let sc = dot(oc, oc) - rad * rad;
                    let shadowed = select(1.0, 0.04, sb < 0.0 && sb * sb - sc > 0.0);
                    // lit from either face; brighter seen against the sun (forward scatter)
                    let lr = 0.35 + 0.65 * abs(dot(sun, rn));
                    let fwd_s = 1.0 + 2.0 * pow(max(dot(dir, sun), 0.0), 8.0);
                    let rc = b.ring_color.rgb * lr * fwd_s * shadowed * e_sun / PI * 0.8 * b.info.z;
                    // thin at grazing angles: fade by screen-space size of a ringlet
                    let graze = smoothstep(0.0, 0.02, abs(den));
                    ring = vec4<f32>(rc * d * graze, d * graze);
                }
            }
        }
    }

    // composite back to front: ring behind, body, ring in front
    var rgb = vec3<f32>(0.0);
    var a = 0.0;
    if t_ring > t_body {
        rgb = ring.rgb * (1.0 - cover);
        a = ring.a * (1.0 - cover);
    }
    rgb = rgb + body * cover;
    a = a + cover * (1.0 - a);
    rgb = rgb + rim;
    if t_ring <= t_body {
        rgb = ring.rgb + rgb * (1.0 - ring.a);
        o.glow = o.glow * (1.0 - ring.a);
        a = ring.a + a * (1.0 - ring.a);
    }
    o.rgb = rgb;
    o.cover = a;
    return o;
}

// Black-body colour (normalised) for a temperature in kelvin, 1000..15000.
fn blackbody(k: f32) -> vec3<f32> {
    let t = k / 100.0;
    var r = 1.0;
    var g = 0.0;
    var bl = 1.0;
    if t <= 66.0 {
        g = clamp(0.39 * log(t) - 0.634, 0.0, 1.0);
        bl = select(clamp(0.543 * log(t - 10.0) - 1.186, 0.0, 1.0), 0.0, t <= 19.0);
    } else {
        r = clamp(1.29 * pow(t - 60.0, -0.1332), 0.0, 1.0);
        g = clamp(1.13 * pow(t - 60.0, -0.0755), 0.0, 1.0);
    }
    return vec3<f32>(r, g, bl);
}

fn star(dir: vec3<f32>, b: Body, px: f32, t: f32) -> Shade {
    var o: Shade;
    o.rgb = vec3<f32>(0.0);
    o.disp = vec3<f32>(0.0);
    let c = normalize(b.dir.xyz);
    let ar = b.dir.w;
    let ang = acos(clamp(dot(dir, c), -1.0, 1.0));
    let disc = 1.0 - smoothstep(ar - px, ar + px, ang);
    let r = clamp(ang / ar, 0.0, 1.0);
    let mu = sqrt(max(1.0 - r * r, 0.0));
    // limb darkening and granulation
    let gran = fbm3(normalize(dir - c) * 400.0 / max(ar * 60.0, 0.3) + vec3<f32>(t * 0.05), 3u, u32(b.info.y));
    let surf = b.color.rgb * (0.4 + 0.6 * mu) * (0.92 + 0.16 * gran);
    let beyond = max(ang - ar, 0.0) / ar;
    // corona: a tight bright halo, a wide soft glow, faint rays
    let rays = 0.6 + 0.4 * fbm2(vec2<f32>(atan2(dot(dir, cross(c, vec3<f32>(0.0, 1.0, 0.0))), dot(dir, vec3<f32>(0.0, 1.0, 0.0))) * 6.0, t * 0.03), 3u, 9u);
    let corona = b.color2.rgb * (exp(-beyond * 2.2) * 0.35 + exp(-beyond * 0.35) * 0.05 * rays);
    let k = 2.4e5 * b.info.z;
    o.glow = (surf * disc * 1.0 + corona * (1.0 - disc)) * k;
    o.cover = disc;
    return o;
}

// A star field for the lensed sky round a black hole (the same look as the night sky's).
fn lensed_stars(d: vec3<f32>, px: f32) -> vec3<f32> {
    var c = vec3<f32>(0.0);
    for (var i = 0u; i < 2u; i++) {
        let scale = select(260.0, 120.0, i == 1u);
        let p = d * scale;
        let cell = floor(p);
        let ci = vec3<i32>(cell);
        let h = hash3f(ci.x, ci.y, ci.z, 11u + i * 12u);
        if h > 0.86 {
            let sp = vec3<f32>(hash3f(ci.x, ci.y, ci.z, 12u + i * 12u), hash3f(ci.x, ci.y, ci.z, 13u + i * 12u), hash3f(ci.x, ci.y, ci.z, 14u + i * 12u)) * 0.7 + 0.15;
            let dd = length(p - cell - sp);
            let size = max(0.07, px * scale * 1.5);
            let core = smoothstep(size, 0.0, dd);
            c += vec3<f32>(0.85, 0.9, 1.0) * core * core * (0.5 + 1.5 * h);
        }
    }
    return c;
}

// A black hole: bend each ray round a point mass (r_s = 1) and gather the thin accretion disc it
// crosses (above, below and the lensed far side); rays that escape show the bent star field.
fn black_hole(dir: vec3<f32>, b: Body, px: f32, t: f32, stars_vis: f32) -> Shade {
    var o: Shade;
    o.rgb = vec3<f32>(0.0);
    o.glow = vec3<f32>(0.0);
    o.cover = 0.0;
    o.disp = vec3<f32>(0.0);
    let c = normalize(b.dir.xyz);
    let ar = b.dir.w;
    // the shadow's angular radius is 2.6 r_s / distance
    let dist = 2.598 / sin(ar);
    var pos = -c * dist;
    var vel = dir;
    let hv = cross(pos, vel);
    let h2 = dot(hv, hv);
    let dn = normalize(b.ring.xyz);
    let r_in = 2.6;
    let r_out = 3.0 * b.atmo.w + 4.0;
    var col = vec3<f32>(0.0);
    var alpha = 0.0;
    var captured = false;
    var prev = dot(pos, dn);
    for (var i = 0; i < 160; i++) {
        let r = length(pos);
        let dt = clamp(0.08 * r, 0.03, 2.0);
        let acc = -1.5 * h2 * pos / pow(r, 5.0);
        vel = vel + acc * dt;
        pos = pos + vel * dt;
        let side = dot(pos, dn);
        if sign(side) != sign(prev) {
            // crossed the disc plane: interpolate the crossing
            let f = prev / (prev - side);
            let q = pos - vel * dt * (1.0 - f);
            let rho = length(q);
            if rho > r_in && rho < r_out && alpha < 0.99 {
                let u = (rho - r_in) / (r_out - r_in);
                let ang = atan2(dot(q, cross(dn, c)), dot(q, normalize(c - dn * dot(c, dn))));
                let omega = 0.6 / pow(rho, 1.5);
                // streaks wound round the hole, sheared by its faster inner orbits
                let wind = ang + log(rho) * 2.0 - t * omega * 4.0;
                let swirl = fbm2(vec2<f32>(log(rho) * 6.0, wind * 2.0), 3u, u32(b.info.y) + 3u);
                let dens = smoothstep(0.0, 0.06, u) * (1.0 - smoothstep(0.45, 1.0, u)) * (0.6 + 0.6 * swirl);
                // orbital speed (fraction of c) and its Doppler beaming towards the viewer
                let beta = clamp(sqrt(0.5 / max(rho - 1.0, 0.5)), 0.0, 0.7);
                let tang = normalize(cross(dn, q));
                let gamma = 1.0 / sqrt(1.0 - beta * beta);
                let dop = 1.0 / (gamma * (1.0 + beta * dot(tang, normalize(vel))));
                let grav = sqrt(max(1.0 - 1.0 / rho, 0.05));
                let g = dop * grav;
                let temp = mix(9000.0, 2200.0, pow(u, 0.6)) * g;
                let tint = mix(b.color.rgb, b.color2.rgb, smoothstep(0.0, 0.8, u));
                let em = blackbody(temp) * tint * pow(g, 3.0) * pow(r_in / rho, 2.2) * 2.2;
                let a = clamp(dens * 0.9, 0.0, 1.0);
                col += (1.0 - alpha) * em * a;
                alpha += (1.0 - alpha) * a;
            }
        }
        prev = side;
        if r < 1.02 {
            captured = true;
            break;
        }
        if r > dist * 1.6 && dot(pos, vel) > 0.0 {
            break;
        }
    }
    // the disc is art-directed rather than physical: as bright on screen by day as by night, so a
    // black hole filling the sky reads under a blue sky too (a physical one would burn it white
    // at night or vanish by day)
    o.disp = col * 1.4 * b.info.z;
    // the bent stars round the shadow (fading out to the night sky's own unbent ones)
    let ang = acos(clamp(dot(dir, c), -1.0, 1.0));
    let lens = 1.0 - smoothstep(ar * 3.0, ar * 5.0, ang);
    let shadow = select(0.0, 1.0, captured);
    var bg = vec3<f32>(0.0);
    if !captured {
        bg = lensed_stars(normalize(vel), px) * stars_vis * lens;
    }
    o.disp = o.disp + bg * (1.0 - alpha);
    o.cover = max(shadow, alpha);
    return o;
}

// The world's air between the viewer and the sky: dimmer and redder towards the horizon.
fn extinction(d: vec3<f32>) -> vec3<f32> {
    let el = degrees(asin(clamp(d.y, -1.0, 1.0)));
    let airmass = 1.0 / (max(d.y, 0.0) + 0.50572 * pow(max(el + 6.07995, 0.1), -1.6364));
    let tau = vec3<f32>(0.05, 0.1, 0.22) * (1.0 + cel.air.x);
    return exp(-tau * min(airmass, 40.0)) * smoothstep(-0.035, 0.01, d.y);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = normalize(in.world_position.xyz - view.world_position);
    // one pixel in radians (derivatives before any branch)
    let px = max(length(fwidth(dir)), 1.0e-6);
    let t = cel.params.y;
    let sun = normalize(cel.sun.xyz);
    let e_sun = cel.sun.w;
    var lit = vec3<f32>(0.0);
    var glow = vec3<f32>(0.0);
    var disp = vec3<f32>(0.0);
    var cover = 0.0;
    let n = i32(cel.params.x);
    for (var i = 0; i < 8; i++) {
        if i >= n {
            break;
        }
        let b = cel.bodies[i];
        let kind = i32(b.info.x + 0.5);
        let c = normalize(b.dir.xyz);
        let ar = b.dir.w;
        // the patch of sky this body can touch
        var reach = ar * 1.25;
        if b.color.w > 0.0 {
            // the ring's outer edge is a sphere of radius outer × R round the centre (the body at
            // unit distance): its near side comes much closer to the eye than the body, so it
            // spans asin(outer × R) of the sky, all of it when the ring would reach the viewer
            let ro = b.atmo.w * sin(ar);
            reach = max(reach, select(asin(min(ro, 1.0)) * 1.02 + 0.01, 3.2, ro >= 0.98));
        }
        if kind == STAR {
            reach = ar * 40.0;
        }
        if kind == HOLE {
            // past this the rays barely bend: the night sky's own stars carry on
            reach = ar * 5.0;
        }
        if dot(dir, c) < cos(min(reach, 3.1)) {
            continue;
        }
        var s: Shade;
        if kind == STAR {
            s = star(dir, b, px, t);
        } else if kind == HOLE {
            s = black_hole(dir, b, px, t, cel.air.y);
        } else {
            s = planet(dir, b, kind, px, sun, e_sun, t);
        }
        // later bodies in the list are in front
        lit = s.rgb + lit * (1.0 - s.cover);
        glow = s.glow + glow * (1.0 - s.cover);
        disp = s.disp + disp * (1.0 - s.cover);
        cover = s.cover + cover * (1.0 - s.cover);
    }
    let air = extinction(dir) * cel.params.w;
    let emit_exposure = min(view.exposure, DAY_EXPOSURE * EMIT_NIGHT_GAIN);
    let lit_exposure = min(view.exposure, DAY_EXPOSURE * LIT_NIGHT_GAIN);
    let out = (lit * lit_exposure + glow * emit_exposure + disp) * air;
    // hide the stars behind a body, but not the night sky's own glow (the air in front of it):
    // half cover is a dark disc against the stars without punching a hole in the twilight
    let a = cover * 0.55 * cel.params.z * cel.params.w * smoothstep(-0.035, 0.01, dir.y);
    if max(max(out.r, out.g), max(out.b, a)) <= 0.0 {
        discard;
    }
    return vec4<f32>(out, a);
}
