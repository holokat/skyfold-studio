//! Small, fast, deterministic noise functions used by terrain generation and brushes.
//! The WGSL versions in `assets/shaders/noise.wesl` mirror `hash2` / `value_noise2` so the
//! CPU-side material classification (footstep surfaces) agrees with what is rendered.

#[inline]
pub fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
pub fn hash2(ix: i32, iy: i32, seed: u32) -> u32 {
    hash_u32(
        (ix as u32).wrapping_mul(0x8da6_b343)
            ^ (iy as u32).wrapping_mul(0xd816_3841)
            ^ seed.wrapping_mul(0xcb1a_b31f),
    )
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn grad(h: u32, x: f32, y: f32) -> f32 {
    // 8 gradient directions
    match h & 7 {
        0 => x + y,
        1 => x - y,
        2 => -x + y,
        3 => -x - y,
        4 => x * 1.4142,
        5 => -x * 1.4142,
        6 => y * 1.4142,
        _ => -y * 1.4142,
    }
}

/// Gradient (Perlin-style) noise in roughly [-1, 1].
pub fn perlin2(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let ix = x0 as i32;
    let iy = y0 as i32;
    let fx = x - x0;
    let fy = y - y0;
    let u = fade(fx);
    let v = fade(fy);
    let n00 = grad(hash2(ix, iy, seed), fx, fy);
    let n10 = grad(hash2(ix + 1, iy, seed), fx - 1.0, fy);
    let n01 = grad(hash2(ix, iy + 1, seed), fx, fy - 1.0);
    let n11 = grad(hash2(ix + 1, iy + 1, seed), fx - 1.0, fy - 1.0);
    let a = n00 + (n10 - n00) * u;
    let b = n01 + (n11 - n01) * u;
    (a + (b - a) * v) * 0.7071
}

/// Value noise in [0, 1] (smooth). Mirrors `value_noise2` in the shader.
pub fn value2(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = x.floor();
    let y0 = y.floor();
    let ix = x0 as i32;
    let iy = y0 as i32;
    let fx = x - x0;
    let fy = y - y0;
    let u = fx * fx * (3.0 - 2.0 * fx);
    let v = fy * fy * (3.0 - 2.0 * fy);
    let h = |a, b| (hash2(a, b, seed) & 0xffff) as f32 / 65535.0;
    let a = h(ix, iy);
    let b = h(ix + 1, iy);
    let c = h(ix, iy + 1);
    let d = h(ix + 1, iy + 1);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}

/// Fractal Brownian motion of gradient noise, roughly [-1, 1].
pub fn fbm(x: f32, y: f32, octaves: u32, lacunarity: f32, gain: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        // rotate each octave a little to hide grid alignment
        let (s, c) = (0.5f32 * o as f32).sin_cos();
        let rx = x * c - y * s;
        let ry = x * s + y * c;
        sum += amp * perlin2(rx * freq, ry * freq, seed.wrapping_add(o * 1013));
        norm += amp;
        amp *= gain;
        freq *= lacunarity;
    }
    sum / norm
}

/// Ridged multifractal in [0, 1]; sharp crests suited to mountain ranges.
pub fn ridged(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut prev = 1.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        let (s, c) = (0.7f32 * o as f32).sin_cos();
        let rx = x * c - y * s;
        let ry = x * s + y * c;
        let n = 1.0 - perlin2(rx * freq, ry * freq, seed.wrapping_add(o * 7919)).abs();
        let n = n * n;
        sum += n * amp * prev;
        norm += amp;
        prev = n.clamp(0.0, 1.0);
        amp *= 0.5;
        freq *= 2.0;
    }
    (sum / norm).clamp(0.0, 1.0)
}

/// Deterministic small RNG (PCG32) for scattering and generation.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng(seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407));
        r.next_u32();
        r
    }
    pub fn next_u32(&mut self) -> u32 {
        let old = self.0;
        self.0 = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.next_u32() % n }
    }
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
