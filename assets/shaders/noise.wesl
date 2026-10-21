// Shared procedural noise. `value_noise2` mirrors `noise::value2` on the CPU exactly so that
// rule-based terrain classification (footstep surfaces) agrees with what is rendered.

fn hash_u32(x_in: u32) -> u32 {
    var x = x_in;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return x;
}

fn hash2i(ix: i32, iy: i32, seed: u32) -> u32 {
    return hash_u32((bitcast<u32>(ix) * 0x8da6b343u) ^ (bitcast<u32>(iy) * 0xd8163841u) ^ (seed * 0xcb1ab31fu));
}

fn hashf(ix: i32, iy: i32, seed: u32) -> f32 {
    return f32(hash2i(ix, iy, seed) & 0xffffu) / 65535.0;
}

fn hash3f(ix: i32, iy: i32, iz: i32, seed: u32) -> f32 {
    return f32(hash_u32(hash2i(ix, iy, seed) ^ (bitcast<u32>(iz) * 0x9e3779b9u)) & 0xffffu) / 65535.0;
}

fn value_noise2(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let ix = i32(i.x);
    let iy = i32(i.y);
    let a = hashf(ix, iy, seed);
    let b = hashf(ix + 1, iy, seed);
    let c = hashf(ix, iy + 1, seed);
    let d = hashf(ix + 1, iy + 1, seed);
    let ab = a + (b - a) * u.x;
    let cd = c + (d - c) * u.x;
    return ab + (cd - ab) * u.y;
}

fn value_noise3(p: vec3<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let ix = i32(i.x);
    let iy = i32(i.y);
    let iz = i32(i.z);
    let a = mix(hash3f(ix, iy, iz, seed), hash3f(ix + 1, iy, iz, seed), u.x);
    let b = mix(hash3f(ix, iy + 1, iz, seed), hash3f(ix + 1, iy + 1, iz, seed), u.x);
    let c = mix(hash3f(ix, iy, iz + 1, seed), hash3f(ix + 1, iy, iz + 1, seed), u.x);
    let d = mix(hash3f(ix, iy + 1, iz + 1, seed), hash3f(ix + 1, iy + 1, iz + 1, seed), u.x);
    return mix(mix(a, b, u.y), mix(c, d, u.y), u.z);
}

// Fractal value noise in [0, 1].
fn fbm2(p_in: vec2<f32>, octaves: u32, seed: u32) -> f32 {
    var p = p_in;
    var sum = 0.0;
    var amp = 0.5;
    var norm = 0.0;
    for (var o = 0u; o < octaves; o++) {
        sum += amp * value_noise2(p, seed + o * 101u);
        norm += amp;
        amp *= 0.5;
        // rotate + scale to hide the lattice
        p = vec2<f32>(p.x * 1.6 - p.y * 1.2, p.x * 1.2 + p.y * 1.6);
    }
    return sum / norm;
}

fn fbm3(p_in: vec3<f32>, octaves: u32, seed: u32) -> f32 {
    var p = p_in;
    var sum = 0.0;
    var amp = 0.5;
    var norm = 0.0;
    for (var o = 0u; o < octaves; o++) {
        sum += amp * value_noise3(p, seed + o * 131u);
        norm += amp;
        amp *= 0.5;
        p = p * 2.03 + vec3<f32>(1.7, 9.2, 3.1);
    }
    return sum / norm;
}

// Cellular noise: x = distance to nearest feature, y = distance to second nearest,
// z = random id of the nearest cell.
fn voronoi2(p: vec2<f32>, seed: u32) -> vec3<f32> {
    let i = floor(p);
    let f = p - i;
    var d1 = 8.0;
    var d2 = 8.0;
    var id = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let cx = i32(i.x) + x;
            let cy = i32(i.y) + y;
            let o = vec2<f32>(hashf(cx, cy, seed), hashf(cx, cy, seed + 7u)) * 0.85 + 0.075;
            let r = vec2<f32>(f32(x), f32(y)) + o - f;
            let d = dot(r, r);
            if d < d1 {
                d2 = d1;
                d1 = d;
                id = hashf(cx, cy, seed + 13u);
            } else if d < d2 {
                d2 = d;
            }
        }
    }
    return vec3<f32>(sqrt(d1), sqrt(d2), id);
}

fn srgb(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

// ---- Cloud cover: where the cumulus field holds cloud (0..1), shared by the volumetric clouds
// (`clouds.wesl` reads a texture baked from the same function by `clouds.rs`) and by the cloud
// shadows on the terrain and the water, so the shadows lie under the clouds. Every octave tiles
// with CLOUD_TILE: the wind offset wraps around it without a seam. Mirrors `clouds::cloud_noise`.

const CLOUD_TILE: f32 = 32768.0;

fn cloud_hash(ix: i32, iy: i32, period: i32, seed: u32) -> u32 {
    return hash2i(((ix % period) + period) % period, ((iy % period) + period) % period, seed);
}

fn cloud_gdot(h: u32, d: vec2<f32>) -> f32 {
    let g = vec2<f32>(f32(h & 0xffffu), f32(h >> 16u)) * (1.0 / 32767.5) - 1.0;
    return dot(g, d);
}

// Periodic gradient noise (period in cells), about -1..1.
fn cloud_gnoise(p: vec2<f32>, period: i32, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let ix = i32(i.x);
    let iy = i32(i.y);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let a = cloud_gdot(cloud_hash(ix, iy, period, seed), f);
    let b = cloud_gdot(cloud_hash(ix + 1, iy, period, seed), f - vec2<f32>(1.0, 0.0));
    let c = cloud_gdot(cloud_hash(ix, iy + 1, period, seed), f - vec2<f32>(0.0, 1.0));
    let d = cloud_gdot(cloud_hash(ix + 1, iy + 1, period, seed), f - vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Cumulus cells: one round blob per cell with its own size and strength (0..1).
fn cloud_blobs(p: vec2<f32>, period: i32, seed: u32) -> f32 {
    let i = floor(p);
    let f = p - i;
    let ix = i32(i.x);
    let iy = i32(i.y);
    var b = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let h = cloud_hash(ix + x, iy + y, period, seed);
            let o = vec2<f32>(f32(h & 0xffu), f32((h >> 8u) & 0xffu)) * (0.8 / 255.0) + 0.1;
            let r = 0.45 + 0.6 * f32((h >> 16u) & 0xffu) / 255.0;
            let s = 0.55 + 0.45 * f32(h >> 24u) / 255.0;
            let v = clamp(1.0 - length(vec2<f32>(f32(x), f32(y)) + o - f) / r, 0.0, 1.0);
            b = max(b, v * (2.0 - v) * s);
        }
    }
    return b;
}

// Cloudiness of the field at q (tile units): clusters of cells of 0.5 - 2 km under a regional
// variation of ~8 km, remapped to an even distribution so `coverage` is about the covered fraction.
fn cloud_noise(q: vec2<f32>) -> f32 {
    let region = 0.5 + 0.5 * (0.65 * cloud_gnoise(q * 4.0, 4, 11u) + 0.35 * cloud_gnoise(q * 8.0, 8, 12u));
    let warp = vec2<f32>(cloud_gnoise(q * 16.0 + vec2<f32>(3.1, 0.0), 16, 21u), cloud_gnoise(q * 16.0 + vec2<f32>(0.0, 7.7), 16, 22u)) * 0.02;
    let cells = cloud_blobs((q + warp) * 16.0, 16, 13u);
    let n = 0.55 * cells + 0.45 * region;
    return 0.5 + 0.5 * tanh((n - CLOUD_NOISE_MEAN) * CLOUD_NOISE_GAIN);
}

const CLOUD_NOISE_MEAN: f32 = 0.527;
const CLOUD_NOISE_GAIN: f32 = 5.73;

// Cover (0..1) for a cloudiness value and the coverage setting: rises from a cell's edge to its
// core (the clouds dome over it); from 75 % the gaps close into an overcast deck.
fn cloud_cover(n: f32, coverage: f32) -> f32 {
    let c = clamp(coverage, 0.0, 1.0);
    let thr = 1.0 - c;
    return max(smoothstep(thr, thr + 0.4, n), clamp((c - 0.75) * 4.0, 0.0, 1.0));
}

// Cloud cover at a world position: `offset` is the wind drift (`SkyState::cloud_offset`).
fn cloud_density(xz: vec2<f32>, coverage: f32, offset: vec2<f32>) -> f32 {
    return cloud_cover(cloud_noise((xz + offset) / CLOUD_TILE), coverage);
}
