//! Mip chains for the terrain's per-vertex maps (normal + flow, the three splat maps). Sampled at
//! full resolution only, distant terrain read one texel out of many per pixel: shimmering paint
//! borders and slope colours, and a texture cache thrashed by every far pixel. The chains are
//! built when a world's textures are created and kept on the CPU (a third of the top level), so a
//! brush stroke only recomputes and uploads the few texels of each level over the stroke.
//!
//! The maps have `n = 2^k + 1` texels a side (one per vertex), so a level isn't exactly half the
//! one above: each texel averages the area of the level above it covers (2–3 texels a side,
//! fractional weights at the ends), the same footprint the GPU assumes when it picks a level, so
//! the levels line up with the top one (no drift or bleeding towards the far edge).

use std::sync::Arc;

use rayon::prelude::*;

use super::GridRect;
use super::gpu::TexWrite;
use bevy::prelude::*;

/// Texels a side of level `l` of an `n`-texel map.
pub fn level_size(n: usize, l: usize) -> usize {
    (n >> l).max(1)
}

/// Levels of a full chain down to 1×1.
pub fn level_count(n: usize) -> usize {
    n.max(1).ilog2() as usize + 1
}

/// The texels of a level of `a` texels that texel `i` of the level below (`b` texels) covers,
/// with their weights (sum 1).
fn taps(i: usize, a: usize, b: usize) -> ([(usize, f32); 3], usize) {
    let r = a as f64 / b as f64;
    let (x0, x1) = (i as f64 * r, (i + 1) as f64 * r);
    let mut out = [(0usize, 0f32); 3];
    let mut n = 0;
    let mut j = x0.floor() as usize;
    while (j as f64) < x1 && j < a && n < 3 {
        let w = ((x1.min(j as f64 + 1.0) - x0.max(j as f64)) / r) as f32;
        if w > 1e-6 {
            out[n] = (j, w);
            n += 1;
        }
        j += 1;
    }
    (out, n)
}

/// The range of texels of the level below (`b` texels) whose footprint touches texels `x0..=x1`
/// of a level of `a` texels.
fn affected(x0: usize, x1: usize, a: usize, b: usize) -> (usize, usize) {
    let r = a as f64 / b as f64;
    let i0 = (x0 as f64 / r).floor() as usize;
    let i1 = (((x1 + 1) as f64 / r).ceil() as usize).saturating_sub(1);
    (i0.min(b - 1), i1.min(b - 1))
}

/// The texels of the level above that texels `i0..=i1` of the level below read.
fn sources(i0: usize, i1: usize, a: usize, b: usize) -> (usize, usize) {
    let r = a as f64 / b as f64;
    (
        (i0 as f64 * r).floor() as usize,
        (((i1 + 1) as f64 * r).ceil() as usize).min(a) - 1,
    )
}

/// Downsamples `rect` (inclusive, level-below coordinates) from `src`, which holds the region
/// `sx0..=sx1 × sz0..=sz1` of the level above (`a` texels a side; RGBA8 rows).
fn downsample(
    src: &[u8],
    (sx0, sz0, sw): (usize, usize, usize),
    a: usize,
    b: usize,
    r: GridRect,
) -> Vec<u8> {
    let w = r.width();
    let mut out = vec![0u8; w * r.height() * 4];
    let xs: Vec<_> = (r.x0..=r.x1).map(|i| taps(i, a, b)).collect();
    out.par_chunks_mut(w * 4).enumerate().for_each(|(row, o)| {
        let (tz, nz) = taps(r.z0 + row, a, b);
        for (px, (tx, nx)) in o.chunks_exact_mut(4).zip(&xs) {
            let nx = *nx;
            let mut acc = [0f32; 4];
            for &(zj, wz) in &tz[..nz] {
                for &(xj, wx) in &tx[..nx] {
                    let s = &src[((zj - sz0) * sw + (xj - sx0)) * 4..][..4];
                    for k in 0..4 {
                        acc[k] += s[k] as f32 * wz * wx;
                    }
                }
            }
            for k in 0..4 {
                px[k] = (acc[k] + 0.5).min(255.0) as u8;
            }
        }
    });
    out
}

/// Levels 1… of one RGBA8 map (level 0 lives in the terrain data).
#[derive(Default)]
pub struct Pyramid {
    pub n: usize,
    /// level l + 1: `level_size(n, l + 1)`² texels
    pub levels: Vec<Vec<u8>>,
}

impl Pyramid {
    /// Builds the whole chain from the full top level.
    pub fn build(top: &[u8], n: usize) -> Self {
        let mut levels: Vec<Vec<u8>> = Vec::new();
        for l in 1..level_count(n) {
            let (a, b) = (level_size(n, l - 1), level_size(n, l));
            let src = if l == 1 { top } else { &levels[l - 2] };
            let next = downsample(
                src,
                (0, 0, a),
                a,
                b,
                GridRect {
                    x0: 0,
                    z0: 0,
                    x1: b - 1,
                    z1: b - 1,
                },
            );
            levels.push(next);
        }
        Self { n, levels }
    }

    /// Top level followed by every level (the image's data).
    pub fn chain(&self, top: Vec<u8>) -> Vec<u8> {
        let mut d = top;
        for l in &self.levels {
            d.extend_from_slice(l);
        }
        d
    }

    /// The top level changed over `rect`: recomputes the texels of every level over it and
    /// queues their uploads. `top` gives the top level's texels over a rectangle.
    pub fn update(
        &mut self,
        rect: GridRect,
        top: impl Fn(GridRect) -> Vec<u8>,
        image: AssetId<Image>,
        out: &mut Vec<TexWrite>,
    ) {
        let n = self.n;
        let mut r = rect;
        for l in 1..=self.levels.len() {
            let (a, b) = (level_size(n, l - 1), level_size(n, l));
            let (x0, x1) = affected(r.x0, r.x1, a, b);
            let (z0, z1) = affected(r.z0, r.z1, a, b);
            let rl = GridRect { x0, z0, x1, z1 };
            let (sx0, sx1) = sources(x0, x1, a, b);
            let (sz0, sz1) = sources(z0, z1, a, b);
            let sr = GridRect {
                x0: sx0,
                z0: sz0,
                x1: sx1,
                z1: sz1,
            };
            let src = if l == 1 {
                top(sr)
            } else {
                let lv = &self.levels[l - 2];
                (sz0..=sz1)
                    .flat_map(|z| lv[(z * a + sx0) * 4..(z * a + sx1 + 1) * 4].iter().copied())
                    .collect()
            };
            let data = downsample(&src, (sx0, sz0, sr.width()), a, b, rl);
            let lv = &mut self.levels[l - 1];
            for (row, z) in (z0..=z1).enumerate() {
                lv[(z * b + x0) * 4..(z * b + x1 + 1) * 4]
                    .copy_from_slice(&data[row * rl.width() * 4..(row + 1) * rl.width() * 4]);
            }
            out.push(TexWrite {
                image,
                x: x0 as u32,
                y: z0 as u32,
                w: rl.width() as u32,
                h: rl.height() as u32,
                bytes_per_texel: 4,
                mip: l as u32,
                offset: 0,
                data: Arc::new(data),
            });
            r = rl;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(n: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        (0..n * n)
            .flat_map(|i| [f(i % n, i / n), 0, 255, 128])
            .collect()
    }

    #[test]
    fn odd_sizes_weigh_the_covered_area() {
        // 2^k + 1: each lower texel covers a bit more than two upper ones
        let (t, k) = taps(0, 5, 2);
        assert_eq!(k, 3);
        assert!((t[..k].iter().map(|t| t.1).sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((t[0].1 - 0.4).abs() < 1e-5 && (t[2].1 - 0.2).abs() < 1e-5);
        assert_eq!(level_count(4097), 13);
        assert_eq!(level_size(4097, 12), 1);
    }

    #[test]
    fn region_updates_match_a_full_rebuild() {
        let n = 33;
        let before = map(n, |x, z| ((x * 7 + z * 3) % 200) as u8);
        let mut p = Pyramid::build(&before, n);
        // paint a patch, then update only its region
        let after = map(n, |x, z| {
            if (10..14).contains(&x) && (5..9).contains(&z) {
                250
            } else {
                ((x * 7 + z * 3) % 200) as u8
            }
        });
        let rect = GridRect {
            x0: 10,
            z0: 5,
            x1: 13,
            z1: 8,
        };
        let top = |r: GridRect| {
            (r.z0..=r.z1)
                .flat_map(|z| after[(z * n + r.x0) * 4..(z * n + r.x1 + 1) * 4].to_vec())
                .collect::<Vec<u8>>()
        };
        let mut writes = vec![];
        p.update(rect, top, AssetId::default(), &mut writes);
        let full = Pyramid::build(&after, n);
        assert_eq!(p.levels, full.levels);
        assert_eq!(writes.len(), level_count(n) - 1);
        assert!(
            writes
                .iter()
                .all(|w| w.data.len() == (w.w * w.h * 4) as usize)
        );
    }
}
