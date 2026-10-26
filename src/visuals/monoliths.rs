//! Monoliths (`AssetKind::Monolith`): tall stones set up on purpose, or grown that way.
//!
//! * A slab (variant 0): a tall, smooth-dressed block of dark stone, its edges worn and chipped,
//!   ringed by a few fine carved grooves with a seam running up its faces.
//! * An obelisk (1): a tapering square shaft under a pyramid cap.
//! * A standing stone (2): a rough, leaning menhir, broad and lumpy, rounded by weather.
//! * Basalt columns (3): a cluster of hexagonal columns of cooled lava, jointed across, their tops
//!   tilted and broken at different heights.
//!
//! The shapes are procedural and wear the photoscanned rock texture (`photoscan::DRESSED`), its
//! tiles sized to the object (`rock_material`). They stand on the ground, sunk into it.

use bevy::prelude::*;

use super::floating::weld;
use super::formations::{fbm3, finish, hash3};
use crate::noise::Rng;
use crate::procgen::meshes::MeshData;

/// Variants: slab, obelisk, standing stone, basalt columns.
pub const VARIANTS: u32 = 4;

/// How deep a monolith reaches into the ground (m), so it stands on a slope without a gap.
const SINK: f32 = 0.9;

/// The mesh of a monolith variant at level of detail `lod` (0 near, 1 far).
pub fn monolith(variant: u32, lod: u32) -> MeshData {
    let fine = lod == 0;
    let (pos, idx) = match variant % VARIANTS {
        0 => slab(fine),
        1 => obelisk(fine),
        2 => standing_stone(fine),
        _ => basalt(variant, fine),
    };
    let (pos, idx) = weld(pos, idx);
    finish(pos, idx, [0.3, 0.3, 0.32])
}

/// Positions along one axis of a box face, from -h to h: `res` apart, closer over the rounded
/// edges (bevel `b`), with rows exactly at each of `keep` (carved grooves).
fn coords(h: f32, b: f32, res: f32, fine: bool, keep: &[f32]) -> Vec<f32> {
    let mut v = vec![-h, h];
    if fine {
        for f in [0.12, 0.45, 1.0] {
            v.push(-h + b * f);
            v.push(h - b * f);
        }
    } else {
        v.push(-h + b);
        v.push(h - b);
    }
    let inner = h - b;
    let n = ((2.0 * inner) / res).ceil().max(1.0) as usize;
    for i in 1..n {
        v.push(-inner + 2.0 * inner * i as f32 / n as f32);
    }
    v.extend(keep.iter().filter(|k| k.abs() < inner));
    v.sort_by(f32::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    v
}

/// A box of half size `half` with edges rounded to radius `b`, its six faces gridded by `coords`
/// (rows at `keep_y` heights, measured from the box's centre), each point pushed by `shape`
/// (the point on the rounded box and its outward direction → the final point).
fn rounded_box(
    half: Vec3,
    b: f32,
    res: f32,
    fine: bool,
    keep_y: &[f32],
    shape: &dyn Fn(Vec3, Vec3) -> Vec3,
) -> (Vec<Vec3>, Vec<u32>) {
    let axes = [
        coords(half.x, b, res, fine, &[]),
        coords(half.y, b, res, fine, keep_y),
        coords(half.z, b, res, fine, &[]),
    ];
    let axis_of = |v: Vec3| {
        if v.x != 0.0 {
            0
        } else if v.y != 0.0 {
            1
        } else {
            2
        }
    };
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let (mut pos, mut idx) = (vec![], vec![]);
    for (n, u, v) in faces {
        let (au, av, an) = (axis_of(u), axis_of(v), axis_of(n));
        let su = |x: f32| x * u[au];
        let sv = |x: f32| x * v[av];
        let start = pos.len() as u32;
        let (cu, cv) = (&axes[au], &axes[av]);
        for &y in cv {
            for &x in cu {
                let mut p = Vec3::ZERO;
                p[an] = n[an] * half[an];
                p[au] = su(x);
                p[av] = sv(y);
                let inner = p.clamp(-(half - Vec3::splat(b)), half - Vec3::splat(b));
                let d = (p - inner).normalize_or(n);
                pos.push(shape(inner + d * b, d));
            }
        }
        let w = cu.len() as u32;
        for j in 0..cv.len() as u32 - 1 {
            for i in 0..w - 1 {
                let p00 = start + j * w + i;
                let (p10, p01, p11) = (p00 + 1, p00 + w, p00 + w + 1);
                idx.extend_from_slice(&[p00, p10, p11, p00, p11, p01]);
            }
        }
    }
    (pos, idx)
}

/// How near `p` is to an edge of the box `half` (0 on a face, 1 on an edge, over `reach` m).
fn edginess(p: Vec3, half: Vec3, reach: f32) -> f32 {
    let t = (p.abs() - half + Vec3::splat(reach)).max(Vec3::ZERO) / reach;
    let t = t.min(Vec3::ONE);
    (t.x + t.y + t.z - t.max_element()).min(1.0)
}

/// Worn, chipped edges and a weathered skin, `depth` m deep at the edges.
fn weathered(p: Vec3, half: Vec3, depth: f32, seed: u32, fine: bool) -> f32 {
    let edge = edginess(p, half, 0.35);
    let chips = (fbm3(p * 1.7, 3, seed) * 2.0 + 0.25).max(0.0);
    let mut d = 0.012 * fbm3(p * 2.2, 3, seed ^ 3) - depth * edge * chips;
    if fine {
        d += 0.004 * fbm3(p * 9.0, 2, seed ^ 5);
    }
    d
}

/// The tall slab with carved grooves and a seam up its broad faces.
fn slab(fine: bool) -> (Vec<Vec3>, Vec<u32>) {
    let height = 10.5;
    let half = Vec3::new(2.0, (height + SINK) * 0.5, 0.55);
    let lift = (height - SINK) * 0.5;
    // groove heights above the ground, as box coordinates (with the rows either side of each)
    let grooves: Vec<f32> = [1.4, 3.5, 7.9, 9.3].iter().map(|g| g - lift).collect();
    let keep: Vec<f32> = if fine {
        grooves
            .iter()
            .flat_map(|g| [g - 0.07, g - 0.03, *g, g + 0.03, g + 0.07])
            .collect()
    } else {
        vec![]
    };
    let shape = move |p: Vec3, n: Vec3| -> Vec3 {
        let mut d = weathered(p, half, 0.07, 0x5_1ab, fine);
        if fine {
            // the grooves round the stone, and a seam up the middle of its broad faces
            let face = n.x.abs().max(n.z.abs()) > 0.9;
            for g in &grooves {
                d -= 0.03 * (1.0 - (p.y - g).abs() / 0.07).max(0.0) * face as u8 as f32;
            }
            let seam = n.z.abs() > 0.9 && p.y > grooves[0] && p.y < grooves[3];
            d -= 0.025 * (1.0 - p.x.abs() / 0.07).max(0.0) * seam as u8 as f32;
        }
        p + n * d + Vec3::Y * lift
    };
    rounded_box(
        half,
        0.08,
        if fine { 0.24 } else { 1.4 },
        fine,
        &keep,
        &shape,
    )
}

/// A square shaft narrowing upwards to a pyramid cap.
fn obelisk(fine: bool) -> (Vec<Vec3>, Vec<u32>) {
    let height = 13.0;
    let cap = 1.2;
    let half = Vec3::new(0.85, (height + SINK) * 0.5, 0.85);
    let lift = (height - SINK) * 0.5;
    let shape = move |p: Vec3, n: Vec3| -> Vec3 {
        let mut q = p + n * weathered(p, half, 0.05, 0x0be1, fine);
        let y = q.y + lift;
        let t = (y / (height - cap)).clamp(0.0, 1.0);
        let mut k = 1.0 - 0.3 * t;
        if y > height - cap {
            // the pyramidion: the shaft's square drawn in to a point
            k *= ((height - y) / cap).max(0.0);
            q.y = y.min(height) - lift;
        }
        Vec3::new(q.x * k, q.y + lift, q.z * k)
    };
    // (rows at the cap's foot, so the turn from shaft to pyramid is crisp)
    let foot = height - cap - lift;
    rounded_box(
        half,
        0.06,
        if fine { 0.22 } else { 1.6 },
        fine,
        &[foot - 0.04, foot],
        &shape,
    )
}

/// A rough, leaning menhir.
fn standing_stone(fine: bool) -> (Vec<Vec3>, Vec<u32>) {
    let height = 6.5;
    let half = Vec3::new(1.15, (height + SINK) * 0.5, 0.6);
    let lift = (height - SINK) * 0.5;
    let seed = 0x3e41;
    let shape = move |p: Vec3, n: Vec3| -> Vec3 {
        let mut d = 0.18 * fbm3(p * 0.45, 4, seed) + 0.05 * fbm3(p * 1.8, 3, seed ^ 3);
        if fine {
            d += 0.012 * fbm3(p * 7.0, 2, seed ^ 5);
        }
        let q = p + n * d;
        let y = q.y + lift;
        let t = (y / height).clamp(0.0, 1.0);
        let k = 1.0 - 0.28 * t * t;
        Vec3::new(q.x * k + 0.12 * height * t * t, y, q.z * k)
    };
    rounded_box(half, 0.45, if fine { 0.18 } else { 1.0 }, fine, &[], &shape)
}

/// A cluster of jointed hexagonal basalt columns.
fn basalt(variant: u32, fine: bool) -> (Vec<Vec3>, Vec<u32>) {
    let mut rng = Rng::new(0xba5a_u64 + variant as u64 * 7_919);
    let seed = rng.next_u32();
    let apothem = 0.45;
    let circ = apothem / (std::f32::consts::PI / 6.0).cos();
    let gap = 0.035;
    let tallest = rng.range(7.0, 9.5);
    // centres on a hexagonal lattice: the middle, its ring of six, some of the next ring
    let step = 2.0 * apothem + gap;
    let dirs: Vec<Vec2> = (0..6)
        .map(|k| {
            let a = std::f32::consts::FRAC_PI_6 + k as f32 * std::f32::consts::FRAC_PI_3;
            Vec2::new(a.cos(), a.sin())
        })
        .collect();
    let mut cols: Vec<(Vec2, u32)> = vec![(Vec2::ZERO, 0)];
    for d in &dirs {
        cols.push((*d * step, 1));
    }
    for k in 0..6 {
        if rng.f32() < 0.55 {
            cols.push((dirs[k] * step * 2.0, 2));
        }
        if rng.f32() < 0.35 {
            cols.push(((dirs[k] + dirs[(k + 1) % 6]) * step, 2));
        }
    }
    let (mut pos, mut idx) = (vec![], vec![]);
    for (c, ring) in cols {
        let h = tallest * (1.0 - 0.2 * ring as f32) * rng.range(0.7, 1.12);
        let h = h.max(1.2);
        let tilt = Vec2::new(rng.range(-0.12, 0.12), rng.range(-0.12, 0.12));
        let chip = rng.below(6) as usize;
        let chip_d = rng.range(0.1, 0.45);
        // the joints across the column, and the rows of its faces
        let mut rows = vec![-SINK, h];
        let mut joints = vec![];
        let mut y = rng.range(0.6, 1.4);
        while y < h - 0.5 {
            joints.push(y);
            y += rng.range(0.9, 1.7);
        }
        if fine {
            let n = ((h + SINK) / 0.45).ceil() as usize;
            rows.extend((1..n).map(|i| -SINK + (h + SINK) * i as f32 / n as f32));
            for j in &joints {
                rows.extend_from_slice(&[j - 0.06, *j, j + 0.06]);
            }
        }
        rows.sort_by(f32::total_cmp);
        rows.dedup_by(|a, b| (*a - *b).abs() < 0.02);
        // the top of a corner at (x, z): tilted, one corner broken off
        let top_at = |k: usize, p: Vec2| h + tilt.dot(p - c) - if k == chip { chip_d } else { 0.0 };
        let corner = |k: usize, y: f32| -> Vec3 {
            let a = k as f32 * std::f32::consts::FRAC_PI_3;
            let mut r = circ;
            if fine {
                for j in &joints {
                    r *= 1.0 - 0.04 * (1.0 - (y - j).abs() / 0.06).max(0.0);
                }
            }
            let p = c + Vec2::new(a.cos(), a.sin()) * r;
            let w = 0.025 * fbm3(Vec3::new(p.x, y * 0.7, p.y) * 2.0, 2, seed);
            let q = c + (p - c) * (1.0 + w / r);
            Vec3::new(q.x, y.min(top_at(k % 6, q)), q.y)
        };
        for k in 0..6 {
            let start = pos.len() as u32;
            for &y in &rows {
                pos.push(corner(k, y));
                pos.push(corner((k + 1) % 6, y));
            }
            for r in 0..rows.len() as u32 - 1 {
                let (a, b) = (start + r * 2, start + r * 2 + 1);
                let (c2, d) = (a + 2, b + 2);
                idx.extend_from_slice(&[a, b, d, a, d, c2]);
            }
        }
        // the top: a low, uneven fan
        let mid = pos.len() as u32;
        let rise = 0.04 * (1.0 + hash3(c.x as i32 * 7, c.y as i32 * 7, 0, seed));
        pos.push(Vec3::new(c.x, top_at(6, c) + rise, c.y));
        for k in 0..6 {
            pos.push(corner(k, h + 1.0));
        }
        for k in 0..6u32 {
            idx.extend_from_slice(&[mid, mid + 1 + (k + 1) % 6, mid + 1 + k]);
        }
    }
    (pos, idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant builds a sound mesh standing on (and sunk into) the ground, its far level
    /// much lighter.
    #[test]
    fn monoliths_build() {
        for v in 0..VARIANTS {
            let near = monolith(v, 0);
            let far = monolith(v, 1);
            for (m, what) in [(&near, "near"), (&far, "far")] {
                assert!(
                    !m.indices.is_empty() && m.indices.len() % 3 == 0,
                    "{v} {what}: no triangles"
                );
                assert!(
                    m.indices.iter().all(|i| (*i as usize) < m.positions.len()),
                    "{v} {what}: index out of range"
                );
                assert!(
                    m.positions.iter().all(|p| p.iter().all(|x| x.is_finite())),
                    "{v} {what}: non-finite"
                );
            }
            let (mn, mx) = near.bounds();
            assert!(mn[1] < -0.5, "{v}: not sunk into the ground");
            assert!(mx[1] > 4.0 && mx[1] < 16.0, "{v}: {} m tall", mx[1]);
            assert!(
                far.triangle_count() * 4 < near.triangle_count() && near.triangle_count() < 12_000,
                "{v}: {} / {} triangles",
                near.triangle_count(),
                far.triangle_count()
            );
        }
    }
}
