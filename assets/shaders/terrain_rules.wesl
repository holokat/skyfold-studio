// The terrain's ground rules: which grounds the style puts where (slope rock, beaches, the snow
// line, dry patches) and where rain gathers. Shared by the terrain shading (`terrain.wesl`, per
// pixel on builds without the baked rule maps) and the bake that evaluates them once per map
// texel whenever the land or the style changes (`terrain_bake.wesl`, `terrain::bake`), so the two
// can't drift apart. `terrain::auto_weights` / `flow_weights` mirror them on the CPU.

const RULE_LAYERS: u32 = 12u;

// Irregular dry patches (-0.5..0.5). Mirrors `terrain::dry_noise`.
fn dry_noise(xz: vec2<f32>) -> f32 {
    return vnoise2(xz * 0.0031, 41u) * 0.55 + vnoise2(xz * 0.0113, 43u) * 0.3 + vnoise2(xz * 0.041, 47u) * 0.15 - 0.5;
}

// The style's grounds at a spot of height `h` (m) and up-normal `ny`. `style`: snow line, snow
// blend, rock slope, beach height (the material's `style`); `snow_boost`: how far weather lowers
// the snow line (m). Mirror of `terrain::auto_weights` (Rust). Keep in sync!
fn auto_weights(h: f32, ny: f32, macro_n: f32, mid_n: f32, dry_n: f32, sea: f32, style: vec4<f32>, dryness: f32, snow_boost: f32) -> array<f32, 12> {
    let snow_line = style.x - snow_boost;
    let snow_blend = style.y;
    let rs = style.z;
    let beach_h = style.w;

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
    for (var k = 0u; k < RULE_LAYERS; k++) { s += w[k]; }
    if s > 1e-4 {
        for (var k = 0u; k < RULE_LAYERS; k++) { w[k] /= s; }
    } else {
        w[0] = 1.0;
    }
    return w;
}

// Where rain gathers (normal map alpha: flow map x wet-gully strength): dry grass greens up along
// swales, the banks of the wettest lines turn to dark damp soil (moss is added in the shading),
// and only the narrow, gentle bed of a real stream gets gravel; applied to the final (painted)
// weights. Linear in the weights (the bake applies it to the style's grounds, the shading to the
// paint). Mirrors `terrain::flow_weights`.
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

// `noise::value_noise2`, bit for bit, for fewer integer multiplies (the four lattice corners share
// their column and row products): the noises the shading still evaluates per pixel.
fn vnoise2(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let x0 = bitcast<u32>(i32(i.x)) * 0x8da6b343u;
    let y0 = bitcast<u32>(i32(i.y)) * 0xd8163841u;
    let s = seed * 0xcb1ab31fu;
    let x1 = x0 + 0x8da6b343u;
    let y1 = y0 + 0xd8163841u;
    let a = lattice(x0 ^ y0 ^ s);
    let b = lattice(x1 ^ y0 ^ s);
    let c = lattice(x0 ^ y1 ^ s);
    let d = lattice(x1 ^ y1 ^ s);
    let ab = a + (b - a) * u.x;
    let cd = c + (d - c) * u.x;
    return ab + (cd - ab) * u.y;
}

// `noise::hash_u32` of a lattice corner, as `noise::hashf` returns it.
fn lattice(h: u32) -> f32 {
    var x = h;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return f32(x & 0xffffu) / 65535.0;
}
