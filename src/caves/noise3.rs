//! 3D gradient noise for rock: Perlin-style gradients on a hashed lattice with a quintic fade,
//! fractal sums, ridged sums and a domain warp. Deterministic and allocation free, so cave
//! volumes evaluate the same on every thread and every run.

use bevy::math::Vec3;

#[inline]
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

/// One of Perlin's 12 edge gradients (plus 4 repeats) dotted with the offset.
#[inline]
fn grad(h: u32, x: f32, y: f32, z: f32) -> f32 {
    match h & 15 {
        0 | 12 => x + y,
        1 | 13 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 | 14 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Gradient noise, roughly −1..1 (rarely beyond ±0.9).
pub fn gnoise(p: Vec3, seed: u32) -> f32 {
    let (fx, fy, fz) = (p.x.floor(), p.y.floor(), p.z.floor());
    let (ix, iy, iz) = (fx as i32, fy as i32, fz as i32);
    let (x, y, z) = (p.x - fx, p.y - fy, p.z - fz);
    let (u, v, w) = (fade(x), fade(y), fade(z));
    let g = |dx: i32, dy: i32, dz: i32| {
        grad(
            hash3(ix + dx, iy + dy, iz + dz, seed),
            x - dx as f32,
            y - dy as f32,
            z - dz as f32,
        )
    };
    let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w) * 0.97
}

/// Fractal sum, normalised to about −1..1.
pub fn fbm(p: Vec3, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 1.0, 0.0, p);
    for o in 0..octaves {
        sum += gnoise(q, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= 0.5;
        // (a rotation between octaves hides the lattice)
        q = Vec3::new(q.x * 1.6 + q.z * 1.2, q.y * 2.0, -q.x * 1.2 + q.z * 1.6) + Vec3::splat(17.3);
    }
    sum / norm
}

/// Ridged fractal sum: sharp crests (broken rock edges), 0..1.
pub fn ridged(p: Vec3, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 1.0, 0.0, p);
    for o in 0..octaves {
        let n = 1.0 - gnoise(q, seed.wrapping_add(o * 131)).abs();
        sum += n * n * amp;
        norm += amp;
        amp *= 0.5;
        q = Vec3::new(q.x * 1.7 - q.z * 1.1, q.y * 2.1, q.x * 1.1 + q.z * 1.7) + Vec3::splat(5.9);
    }
    sum / norm
}

/// A low-frequency offset field (domain warp), each axis −1..1.
pub fn warp(p: Vec3, seed: u32) -> Vec3 {
    Vec3::new(
        gnoise(p, seed),
        gnoise(p + Vec3::new(31.7, 11.3, 5.1), seed ^ 0x5bd1),
        gnoise(p + Vec3::new(-7.9, 23.1, 41.3), seed ^ 0x2c1b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_bounded_and_deterministic() {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..4000 {
            let p = Vec3::new(
                i as f32 * 0.137,
                (i * 7 % 113) as f32 * 0.21,
                (i * 13 % 97) as f32 * 0.33,
            );
            let n = gnoise(p, 9);
            assert_eq!(n, gnoise(p, 9));
            lo = lo.min(n);
            hi = hi.max(n);
            let f = fbm(p, 4, 3);
            assert!((-1.2..=1.2).contains(&f));
            let r = ridged(p, 3, 3);
            assert!((0.0..=1.0001).contains(&r));
        }
        assert!(lo < -0.4 && hi > 0.4, "{lo} {hi}");
        assert!(lo > -1.1 && hi < 1.1);
    }

    #[test]
    fn noise_is_continuous() {
        for i in 0..500 {
            let p = Vec3::new(i as f32 * 0.31, 1.7, -(i as f32) * 0.11);
            let a = gnoise(p, 1);
            let b = gnoise(p + Vec3::splat(1e-3), 1);
            assert!((a - b).abs() < 0.02);
        }
    }
}
