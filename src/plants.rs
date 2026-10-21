//! Procedural plants that ez-tree can't grow: coconut palms (curved ringed trunk + drooping
//! photographic fronds), saguaro cacti (ribbed trunk with arms), and photo-card plants (flowers,
//! grass tufts, cattails, lily pads: crossed cut-out cards). They produce ez-tree style buffers
//! (positions, normals, uvs with v = 0 at the base, indices, sway weights) so `trees.rs` treats
//! them exactly like ez-tree trees: same materials, LODs, wind and impostors.

use bevy::math::{Quat, Vec2, Vec3};
use ez_tree::Buffers;

use crate::noise::Rng;

/// (bark/trunk, leaves) buffers per level of detail.
pub type Lods = Vec<(Buffers, Option<Buffers>)>;

fn push(b: &mut Buffers, p: Vec3, n: Vec3, uv: [f32; 2], sway: f32) -> u32 {
    b.positions.push(p.to_array());
    b.normals.push(n.normalize_or(Vec3::Y).to_array());
    b.uvs.push(uv);
    b.sway.push(sway);
    (b.positions.len() - 1) as u32
}

/// Recomputes smooth normals from the triangles (for shapes with analytic surface detail).
fn smooth_normals(b: &mut Buffers) {
    let mut acc = vec![Vec3::ZERO; b.positions.len()];
    for t in b.indices.chunks_exact(3) {
        let (a, c, d) = (
            Vec3::from(b.positions[t[0] as usize]),
            Vec3::from(b.positions[t[1] as usize]),
            Vec3::from(b.positions[t[2] as usize]),
        );
        let n = (c - a).cross(d - a);
        for &i in t {
            acc[i as usize] += n;
        }
    }
    for (i, n) in acc.into_iter().enumerate() {
        if n.length_squared() > 1e-12 {
            b.normals[i] = n.normalize().to_array();
        }
    }
}

/// A tube along `path` (with per-ring radius), u wrapping `wraps` times around, v in metres /
/// `v_scale`. Returns nothing; appends to `b`. `ribs`: (count, depth) radial modulation.
#[allow(clippy::too_many_arguments)]
fn tube(
    b: &mut Buffers,
    path: &[Vec3],
    radius: &[f32],
    sides: usize,
    wraps: f32,
    v_scale: f32,
    sway: &[f32],
    ribs: (f32, f32),
    cap: bool,
) {
    let base = b.positions.len() as u32;
    let mut arc = 0.0;
    let mut prev_n = Vec3::X;
    for (k, p) in path.iter().enumerate() {
        if k > 0 {
            arc += path[k].distance(path[k - 1]);
        }
        let tan = if k + 1 < path.len() {
            path[k + 1] - *p
        } else {
            *p - path[k - 1]
        }
        .normalize_or(Vec3::Y);
        // parallel-transport frame (no twisting)
        let mut nrm = (prev_n - tan * prev_n.dot(tan)).normalize_or(tan.any_orthonormal_vector());
        if nrm.length_squared() < 0.5 {
            nrm = tan.any_orthonormal_vector();
        }
        prev_n = nrm;
        let bin = tan.cross(nrm);
        for j in 0..=sides {
            let a = j as f32 / sides as f32 * std::f32::consts::TAU;
            let rib = 1.0 + ribs.1 * (a * ribs.0).cos();
            let dir = nrm * a.cos() + bin * a.sin();
            push(
                b,
                *p + dir * radius[k] * rib,
                dir,
                [j as f32 / sides as f32 * wraps, arc / v_scale],
                sway[k],
            );
        }
    }
    let ring = sides as u32 + 1;
    for k in 0..path.len() as u32 - 1 {
        for j in 0..sides as u32 {
            let a = base + k * ring + j;
            let c = a + ring;
            b.indices.extend_from_slice(&[a, c, a + 1, a + 1, c, c + 1]);
        }
    }
    if cap {
        // close the top with a small dome point
        let last = *path.last().unwrap();
        let tan = (last - path[path.len() - 2]).normalize_or(Vec3::Y);
        let tip = push(
            b,
            last + tan * radius[radius.len() - 1] * 0.6,
            tan,
            [0.5, arc / v_scale + 0.05],
            *sway.last().unwrap(),
        );
        let start = base + (path.len() as u32 - 1) * ring;
        for j in 0..sides as u32 {
            b.indices
                .extend_from_slice(&[start + j, tip, start + j + 1]);
        }
    }
}

/// Coconut palm, `height` metres.
pub fn palm(seed: u64, height: f32) -> Lods {
    let mut lods = vec![];
    for lod in 0..3 {
        let mut rng = Rng::new(seed);
        let (rings, sides, fronds, segs) = [(14, 10, 15, 7), (8, 7, 11, 5), (4, 5, 8, 3)][lod];
        let lean_az = rng.f32() * std::f32::consts::TAU;
        let lean = height * rng.range(0.08, 0.2);
        let ld = Vec3::new(lean_az.cos(), 0.0, lean_az.sin());
        let mut path: Vec<Vec3> = (0..=rings)
            .map(|k| {
                let t = k as f32 / rings as f32;
                ld * (lean * t.powf(1.7)) + Vec3::Y * (height * t)
            })
            .collect();
        let mut radius: Vec<f32> = (0..=rings)
            .map(|k| {
                let t = k as f32 / rings as f32;
                (0.25 + (0.15 - 0.25) * t) * (1.0 + 0.45 * (1.0 - t).powi(10))
            })
            .collect();
        let mut sway: Vec<f32> = (0..=rings)
            .map(|k| (k as f32 / rings as f32).powf(1.6) * 0.35)
            .collect();
        // the trunk carries on half a metre underground: its open end never shows on a slope
        path.insert(0, Vec3::Y * -0.5);
        radius.insert(0, radius[0]);
        sway.insert(0, 0.0);
        let mut trunk = Buffers::default();
        // one wrap of the 1.86 m bark scan (BarkTex::Palm) around the ~1.3 m trunk, aspect kept
        tube(
            &mut trunk,
            &path,
            &radius,
            sides,
            1.0,
            1.3,
            &sway,
            (0.0, 0.0),
            true,
        );

        // crown of fronds
        let crown = *path.last().unwrap();
        let up = (crown - path[path.len() - 2]).normalize();
        let mut leaves = Buffers::default();
        let golden = 2.399_96;
        let az0 = rng.f32() * std::f32::consts::TAU;
        let scale = height / 11.0;
        for f in 0..fronds {
            let az =
                az0 + f as f32 * golden * (15.0 / fronds as f32).sqrt() + rng.range(-0.15, 0.15);
            let elev = rng.range(-0.15, 0.75);
            let len = rng.range(3.4, 4.8) * scale.max(0.6);
            let droop = rng.range(0.10, 0.2) / len;
            let width = len * 0.62;
            let fold = rng.range(0.14, 0.24);
            let hdir = Vec3::new(az.cos(), 0.0, az.sin());
            let base = leaves.positions.len() as u32;
            let mut prev = crown;
            for s in 0..=segs {
                let t = s as f32 / segs as f32;
                let along = len * t;
                let p = crown
                    + hdir * (along * elev.cos())
                    + up * (along * elev.sin() - droop * along * along)
                    + up * 0.05;
                let next_t = ((s + 1) as f32 / segs as f32).min(1.0);
                let pn = crown
                    + hdir * (len * next_t * elev.cos())
                    + up * (len * next_t * elev.sin() - droop * (len * next_t).powi(2));
                let tan = if s < segs { pn - p } else { p - prev }.normalize_or(hdir);
                prev = p;
                let side = tan.cross(Vec3::Y).normalize_or(Vec3::X);
                let lup = side.cross(tan).normalize_or(Vec3::Y);
                // V-shaped cross-section: the leaflets rise from the midrib
                let half = width * 0.5 * (0.35 + 0.65 * (t * 3.0).min(1.0));
                let nrm = (lup + (p - crown).normalize_or(Vec3::Y) * 0.35).normalize();
                let sw = 0.35 + 0.65 * t;
                push(
                    &mut leaves,
                    p - side * half + lup * half * fold,
                    nrm,
                    [0.0, t],
                    sw,
                );
                push(&mut leaves, p, nrm, [0.5, t], sw);
                push(
                    &mut leaves,
                    p + side * half + lup * half * fold,
                    nrm,
                    [1.0, t],
                    sw,
                );
            }
            for s in 0..segs as u32 {
                let a = base + s * 3;
                let c = a + 3;
                leaves.indices.extend_from_slice(&[
                    a,
                    c,
                    a + 1,
                    a + 1,
                    c,
                    c + 1,
                    a + 1,
                    c + 1,
                    a + 2,
                    a + 2,
                    c + 1,
                    c + 2,
                ]);
            }
        }
        lods.push((trunk, Some(leaves)));
    }
    lods
}

/// Saguaro cactus, `height` metres.
pub fn cactus(seed: u64, height: f32) -> Lods {
    let mut lods = vec![];
    for lod in 0..3 {
        let mut rng = Rng::new(seed);
        let (rings, sides) = [(16, 40), (9, 24), (5, 14)][lod];
        let r0 = rng.range(0.22, 0.3) * (height / 4.5).sqrt();
        let ribs = 20.0;
        let mut b = Buffers::default();
        let mut path: Vec<Vec3> = (0..=rings)
            .map(|k| Vec3::Y * (height * k as f32 / rings as f32 - 0.1))
            .collect();
        let mut radius: Vec<f32> = (0..=rings)
            .map(|k| {
                let t = k as f32 / rings as f32;
                r0 * (1.0 - 0.12 * t)
                    * if t > 0.9 {
                        (1.0 - ((t - 0.9) / 0.1).powi(2)).max(0.05).sqrt()
                    } else {
                        1.0
                    }
            })
            .collect();
        // (it carries on underground: its open end never shows on a slope)
        path.insert(0, Vec3::Y * -0.6);
        radius.insert(0, radius[0]);
        let zero = vec![0.0; path.len()];
        tube(
            &mut b,
            &path,
            &radius,
            sides,
            ribs / 8.0,
            0.35,
            &zero,
            (ribs, 0.07),
            true,
        );
        // arms: out, then up
        let arms = (rng.f32() * 3.2) as usize;
        let mut az = rng.f32() * std::f32::consts::TAU;
        for _ in 0..arms {
            let h0 = height * rng.range(0.32, 0.6);
            let out = rng.range(0.45, 0.75);
            let rise = rng.range(0.9, 1.9).min(height - h0 + 0.3);
            let dir = Vec3::new(az.cos(), 0.0, az.sin());
            let ar = r0 * 0.68;
            let n = [10, 6, 4][lod];
            let apath: Vec<Vec3> = (0..=n)
                .map(|k| {
                    let t = k as f32 / n as f32;
                    // quarter-circle elbow then straight up
                    let ang = (t * 1.6).min(1.0) * std::f32::consts::FRAC_PI_2;
                    let horiz = out * ang.sin();
                    let up = out * (1.0 - ang.cos()) + (t - 0.625).max(0.0) / 0.375 * rise;
                    Vec3::Y * h0 + dir * (r0 * 0.8 + horiz) + Vec3::Y * up
                })
                .collect();
            let arad: Vec<f32> = (0..=n)
                .map(|k| ar * if k == n { 0.85 } else { 1.0 })
                .collect();
            let az0 = vec![0.0; n + 1];
            tube(
                &mut b,
                &apath,
                &arad,
                (sides * 2 / 3).max(10),
                ribs / 12.0,
                0.35,
                &az0,
                (ribs * 0.75, 0.07),
                true,
            );
            az += rng.range(1.6, 3.4);
        }
        smooth_normals(&mut b);
        lods.push((b, None));
    }
    lods
}

/// Crossed photo cards (flowers, grass tufts, cattails): `planes` vertical cards per clump,
/// `clumps` clumps within `spread` m. uv v = 0 at the bottom of the card.
pub fn cards(seed: u64, height: f32, aspect: f32, planes: u32, clumps: u32, spread: f32) -> Lods {
    let mut rng = Rng::new(seed);
    let mut b = Buffers::default();
    for c in 0..clumps {
        let off = if c == 0 {
            Vec2::ZERO
        } else {
            Vec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0)) * spread
        };
        let h = height * rng.range(0.75, 1.15);
        let w = h * aspect;
        let rot0 = rng.f32() * std::f32::consts::PI;
        let lean = Quat::from_rotation_x(rng.range(-0.08, 0.08))
            * Quat::from_rotation_z(rng.range(-0.08, 0.08));
        for p in 0..planes {
            let a = rot0 + p as f32 * std::f32::consts::PI / planes as f32;
            let across = Vec3::new(a.cos(), 0.0, a.sin());
            let o = Vec3::new(off.x, -0.04, off.y);
            let base = b.positions.len() as u32;
            for (s, t) in [(0.0f32, 0.0f32), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                let pos = o + lean * (across * ((s - 0.5) * w) + Vec3::Y * (t * h));
                // up-biased normals so cards shade like the ground around them
                let n = (Vec3::Y + across.cross(Vec3::Y) * 0.25).normalize();
                push(&mut b, pos, n, [s, t], t * t);
            }
            b.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    vec![(b, None)]
}

/// Floating lily pads: one horizontal card `size` m across, slightly above the origin.
pub fn lily_pads(size: f32) -> Lods {
    let mut b = Buffers::default();
    let h = size * 0.5;
    for (x, z, s, t) in [
        (-h, -h, 0.0, 0.0),
        (h, -h, 1.0, 0.0),
        (h, h, 1.0, 1.0),
        (-h, h, 0.0, 1.0),
    ] {
        push(&mut b, Vec3::new(x, 0.02, z), Vec3::Y, [s, t], 0.08);
    }
    b.indices.extend_from_slice(&[0, 2, 1, 0, 3, 2]);
    vec![(b, None)]
}
