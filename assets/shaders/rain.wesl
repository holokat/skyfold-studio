// Rain on the ground (rain/mod.rs), shared by the terrain and the roads: where puddles stand and
// the rings raindrops make on them.
//
// `puddle_amount`: standing water fills the low spots first. A slow two-octave noise lays out the
// puddles' shapes (as a z-score: about ±1 over most of the ground); `low` adds how low the spot
// is in the same units (the gaps between cobbles and stones in the surface's height map, cart
// ruts, the lines where rain gathers; strongly negative where the ground doesn't hold water) and
// `fill` (0..1, rising while it rains) moves the waterline up through both: about a sixth of the
// flat ground at 0.6, nearly half at 1. Only on the flat.
//
// `rain_ripples`: drop rings on standing water. The ground is cut into small cells; in each,
// one drop lands per cycle at a new random spot and its ring runs outwards and fades. Returns the
// water's slope (d height / dx, dz), to tilt the mirror-flat normal. Nine cells are looked at per
// pixel (rings cross cell borders): callers only ask for it in puddles near the camera.

import super::noise::{value_noise2, hashf};

fn puddle_amount(xz: vec2<f32>, fill: f32, low: f32, flat: f32) -> f32 {
    if fill <= 0.0 || flat <= 0.0 {
        return 0.0;
    }
    let shape = value_noise2(xz * 0.21, 401u) * 0.65 + value_noise2(xz * 0.73, 409u) * 0.35;
    let z = (shape - 0.5) * 9.0 + low;
    let th = 2.2 - 2.0 * fill;
    return smoothstep(th, th + 0.35, z) * flat;
}

fn rain_ripples(xz: vec2<f32>, t: f32, rain: f32) -> vec2<f32> {
    // cells of about 0.3 m
    let q = xz * 3.3;
    let base = floor(q);
    var g = vec2<f32>(0.0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = base + vec2<f32>(f32(i), f32(j));
            let ci = vec2<i32>(c);
            let period = 0.6 + hashf(ci.x, ci.y, 421u) * 0.5;
            let ph = t / period + hashf(ci.x, ci.y, 431u);
            let cyc = i32(floor(ph));
            let age = fract(ph);
            // fewer drops in a light rain
            if hashf(ci.x + cyc * 7, ci.y, 433u) > rain {
                continue;
            }
            let at = c + vec2<f32>(hashf(ci.x, ci.y + cyc * 13, 439u), hashf(ci.x + cyc * 3, ci.y, 443u));
            let d = q - at;
            let r = length(d);
            let x = r - age * 1.25;
            // a ring travelling out: a short wave packet, fading as it spreads
            let ring = sin(x * 25.0) * exp(-x * x * 30.0) * (1.0 - age) * (1.0 - age);
            g += d / max(r, 1e-3) * ring;
        }
    }
    return g;
}
