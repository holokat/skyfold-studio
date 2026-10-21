//! High-level terrain features: realistic landform generators (volcano, mountain range, ridge,
//! crater, mesas and buttes, canyon, desert dunes, rolling hills, plateau, valley, sea cliffs,
//! lagoon, swamp channels, lake with an island). Each shapes the heightfield deterministically
//! from a seed, blends into the ground around it, records undo history like a brush, and leaves
//! slopes, snow and beaches to the automatic ground style (plus a little painting of its own).
//! The selection's edit mask (`mask`) applies afterwards like for any edit.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

use super::selection::MaskField;
use crate::history::History;
use crate::noise::{fbm, perlin2, ridged, smoothstep};
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::terrain::{GridRect, Terrain};

// ------------------------------------------------------------------ shared helpers

/// Rewrites the heights in `rect` with `f(world xz, old height)` (rows in parallel).
pub fn shape_heights(
    t: &mut Terrain,
    h: &mut History,
    rect: GridRect,
    f: impl Fn(Vec2, f32) -> f32 + Sync,
) {
    h.touch_terrain(t, rect);
    let rows: Vec<usize> = (rect.z0..=rect.z1).collect();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 12);
    let chunk = rows.len().div_ceil(threads).max(1);
    let out: Vec<(usize, Vec<f32>)> = {
        let tt = &*t;
        let f = &f;
        std::thread::scope(|s| {
            let hs: Vec<_> = rows
                .chunks(chunk)
                .map(|js| {
                    s.spawn(move || {
                        js.iter()
                            .map(|&j| {
                                let row = (rect.x0..=rect.x1)
                                    .map(|i| f(tt.grid_to_world(i, j), tt.h(i, j)))
                                    .collect();
                                (j, row)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            hs.into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };
    for (j, row) in out {
        let a = t.idx(rect.x0, j);
        t.heights[a..a + row.len()].copy_from_slice(&row);
    }
    t.mark_heights_dirty(rect);
}

/// Blends ground paint towards `layer` by `f(world xz, height, slope°) -> weight` (0..1).
pub fn paint_where(
    t: &mut Terrain,
    h: &mut History,
    rect: GridRect,
    layer: Layer,
    f: impl Fn(Vec2, f32, f32) -> f32,
) {
    h.touch_terrain(t, rect);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let p = t.grid_to_world(i, j);
            let slope = t
                .normal_at_grid(i, j)
                .y
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            let w = f(p, t.h(i, j), slope).clamp(0.0, 1.0);
            if w <= 0.003 {
                continue;
            }
            let k = t.idx(i, j);
            let s = &mut t.splat[k];
            for (l, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
                let target = if l == layer as usize { 255.0 } else { 0.0 };
                *v = (*v as f32 + (target - *v as f32) * w)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            let sum: u32 = s.iter().map(|v| *v as u32).sum();
            if sum > 255 {
                let k = 255.0 / sum as f32;
                s.iter_mut()
                    .for_each(|v| *v = (*v as f32 * k).floor() as u8);
            }
        }
    }
    t.mark_splat_dirty(rect);
}

/// Smooth maximum (blends over `k` m instead of a crease).
fn smax(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (a - b) / k).clamp(0.0, 1.0);
    b + (a - b) * h + k * h * (1.0 - h)
}

fn rect_around(t: &Terrain, lo: Vec2, hi: Vec2) -> Option<GridRect> {
    t.rect_for_circle((lo + hi) * 0.5, ((hi - lo) * 0.5).max_element())
}

/// Distance from `p` to a polyline, metres along it at the closest point, and the side (+1 left).
pub fn polyline_dist(p: Vec2, path: &[Vec2]) -> (f32, f32, f32) {
    if path.len() == 1 {
        return (p.distance(path[0]), 0.0, 1.0);
    }
    let (mut best, mut along, mut side, mut acc) = (f32::MAX, 0.0, 1.0, 0.0);
    for w in path.windows(2) {
        let ab = w[1] - w[0];
        let len = ab.length().max(1e-4);
        let t = ((p - w[0]).dot(ab) / (len * len)).clamp(0.0, 1.0);
        let d = p.distance(w[0] + ab * t);
        if d < best {
            best = d;
            along = acc + t * len;
            side = if ab.perp_dot(p - w[0]) >= 0.0 {
                1.0
            } else {
                -1.0
            };
        }
        acc += len;
    }
    (best, along, side)
}

pub fn path_length(path: &[Vec2]) -> f32 {
    path.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Points along a polyline every `step` m.
pub fn resample(path: &[Vec2], step: f32) -> Vec<Vec2> {
    let mut out = vec![path[0]];
    for w in path.windows(2) {
        let n = (w[0].distance(w[1]) / step.max(0.5)).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(w[0].lerp(w[1], k as f32 / n as f32));
        }
    }
    out
}

/// Low ground under a path that only ever descends (a canyon or valley floor), sampled every
/// `step` m from the higher end. Returns (samples, floor heights) with the path oriented downhill.
fn descending_floor(t: &Terrain, path: &[Vec2], step: f32, depth: f32) -> (Vec<Vec2>, Vec<f32>) {
    let mut pts = resample(path, step);
    if t.height_at(pts[0].x, pts[0].y) < t.height_at(pts[pts.len() - 1].x, pts[pts.len() - 1].y) {
        pts.reverse();
    }
    let mut floor = Vec::with_capacity(pts.len());
    let mut m = f32::MAX;
    for p in &pts {
        // a little look-around so the floor follows the valley, not a ridge it crosses
        let h = (0..6)
            .map(|k| t.height_at(p.x + (k as f32).cos() * step, p.y + (k as f32).sin() * step))
            .fold(t.height_at(p.x, p.y), f32::min);
        m = m.min(h);
        floor.push(m - depth);
    }
    // smooth while staying monotonic
    for _ in 0..4 {
        for i in 1..floor.len().saturating_sub(1) {
            floor[i] = ((floor[i - 1] + floor[i] + floor[i + 1]) / 3.0)
                .min(floor[i - 1])
                .max(floor[i + 1]);
        }
    }
    (pts, floor)
}

fn floor_at(floor: &[f32], step: f32, along: f32) -> f32 {
    let x = (along / step).max(0.0);
    let i = (x.floor() as usize).min(floor.len() - 1);
    let j = (i + 1).min(floor.len() - 1);
    floor[i] + (floor[j] - floor[i]) * x.fract()
}

/// Local rain erosion around a feature, kept within `keep(p)` (0..1) of it. Droplets roam, so a
/// margin around `rect` is recorded and restored too.
fn erode_near(
    t: &mut Terrain,
    h: &mut History,
    rect: GridRect,
    seed: u32,
    strength: f32,
    keep: impl Fn(Vec2) -> f32,
) {
    let rect = rect.expand(96, t.n());
    h.touch_terrain(t, rect);
    let w = rect.width();
    let before: Vec<f32> = (rect.z0..=rect.z1)
        .flat_map(|j| (rect.x0..=rect.x1).map(move |i| (i, j)))
        .map(|(i, j)| t.h(i, j))
        .collect();
    let (cx, cz) = (
        (rect.x0 + rect.x1) as f32 * 0.5,
        (rect.z0 + rect.z1) as f32 * 0.5,
    );
    let r = ((rect.width().max(rect.height()) as f32) * 0.5 - 96.0).max(8.0);
    let droplets = ((r * r * 3.0) * 0.22 * strength) as usize;
    let (res, cell) = (t.res, t.cell);
    crate::terrain::worldgen::erode(
        &mut t.heights,
        res,
        cell,
        droplets.min(400_000),
        seed as u64 * 7919 + 1,
        Some((cx, cz, r)),
        1.0,
    );
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let k = keep(t.grid_to_world(i, j)).clamp(0.0, 1.0);
            let b = before[(j - rect.z0) * w + (i - rect.x0)];
            let idx = t.idx(i, j);
            t.heights[idx] = b + (t.heights[idx] - b) * k;
        }
    }
    t.mark_heights_dirty(rect);
}

// ------------------------------------------------------------------ volcano

pub struct Volcano {
    pub center: Vec2,
    /// Foot radius (m).
    pub radius: f32,
    /// Summit height above the foot (m).
    pub height: f32,
    pub crater_radius: f32,
    pub crater_depth: f32,
    pub seed: u32,
}

/// Lowest ground around a circle (the foot of a cone).
fn foot_level(t: &Terrain, c: Vec2, r: f32) -> f32 {
    (0..48)
        .map(|k| {
            let a = k as f32 / 48.0 * TAU;
            t.height_at(c.x + a.cos() * r, c.y + a.sin() * r)
        })
        .fold(f32::MAX, f32::min)
        .max(t.sea_level - 8.0)
}

/// A stratovolcano: concave flanks, a crater with a raised lip and steep inner walls, radial
/// gullies and lava-flow lobes. Returns (foot level, crater floor height).
pub fn volcano(t: &mut Terrain, h: &mut History, v: &Volcano) -> Option<(f32, f32)> {
    let (c, r, hh) = (v.center, v.radius, v.height);
    let base = foot_level(t, c, r * 0.98);
    let s_rim = (v.crater_radius / r).clamp(0.05, 0.4);
    let depth = v.crater_depth.min(hh * 0.8);
    let seed = v.seed;
    let rect = t.rect_for_circle(c, r * 1.05)?;
    shape_heights(t, h, rect, |p, old| {
        let d = p - c;
        let s0 = d.length() / r;
        if s0 >= 1.05 {
            return old;
        }
        let ang = d.y.atan2(d.x);
        let (ca, sa) = (ang.cos(), ang.sin());
        // lava-flow lobes push the foot out unevenly
        let lobe = fbm(ca * 1.4, sa * 1.4, 3, 2.0, 0.5, seed + 1) * 0.16
            + fbm(p.x / (r * 0.22), p.y / (r * 0.22), 3, 2.0, 0.5, seed + 2) * 0.05;
        let s = (s0 * (1.0 - lobe * smoothstep(0.35, 0.95, s0))).max(0.0);
        let mut y = if s >= s_rim {
            let x = ((s - s_rim) / (1.0 - s_rim)).clamp(0.0, 1.0);
            hh * (1.0 - x).powf(1.45)
        } else {
            // crater: flat floor, steep inner walls up to the rim
            let q = s / s_rim;
            hh - depth * (1.0 - smoothstep(0.5, 1.0, q))
        };
        // raised lip around the rim
        y += hh * 0.035 * (-((s - s_rim) / 0.045).powi(2)).exp();
        // radial gullies: deep near the upper flanks, fading at the foot
        let flank = smoothstep(s_rim + 0.03, s_rim + 0.22, s) * (1.0 - smoothstep(0.72, 1.0, s));
        let g1 = perlin2(ca * 4.5 + s * 1.1, sa * 4.5 + s * 1.1, seed + 3);
        let g2 = perlin2(ca * 11.0 + s * 2.3, sa * 11.0 - s * 2.3, seed + 4);
        let v1 = (1.0 - (g1.abs() * 3.0).min(1.0)).powi(2);
        let v2 = (1.0 - (g2.abs() * 3.5).min(1.0)).powi(2);
        y -= flank * hh * (0.075 * v1 + 0.03 * v2) * (0.55 + 0.45 * (1.0 - s));
        // rough lava surface, stronger lower down
        y += fbm(p.x / 60.0, p.y / 60.0, 4, 2.0, 0.5, seed + 5) * hh * 0.012
            + fbm(p.x / (r * 0.18), p.y / (r * 0.18), 3, 2.0, 0.5, seed + 6)
                * hh
                * 0.035
                * smoothstep(0.25, 0.9, s);
        let target = base + y.max(0.0);
        // the cone owns its upper part; near the foot it rises out of the ground around it
        let own = 1.0 - smoothstep(0.62, 0.9, s0);
        let rise = smax(old, target, hh * 0.04);
        let out = rise + (target - rise) * own;
        let edge = 1.0 - smoothstep(0.95, 1.05, s0);
        old + (out - old) * edge
    });
    Some((base, base + hh - depth))
}

/// Volcanic ground: black rock on the upper cone and in tongues of old lava flowing down the
/// flanks, grey ash on the slopes thinning out raggedly over a dusty apron.
pub fn paint_volcano(t: &mut Terrain, h: &mut History, v: &Volcano) {
    let (c, r, seed) = (v.center, v.radius, v.seed);
    let Some(rect) = t.rect_for_circle(c, r * 1.25) else {
        return;
    };
    let polar = move |p: Vec2| {
        let d = p - c;
        let a = d.y.atan2(d.x);
        (d.length() / r, a.cos(), a.sin())
    };
    // dusty apron
    paint_where(t, h, rect, Layer::Dirt, |p, _, _| {
        let (s, _, _) = polar(p);
        let n = fbm(p.x / 90.0, p.y / 90.0, 4, 2.0, 0.5, seed + 20);
        (1.0 - smoothstep(0.85, 1.2, s + n * 0.25))
            * smoothstep(0.55, 0.8, s)
            * (0.45 + 0.4 * (n * 0.5 + 0.5))
    });
    // ash
    paint_where(t, h, rect, Layer::Gravel, |p, _, _| {
        let (s, _, _) = polar(p);
        let n = fbm(p.x / 55.0, p.y / 55.0, 4, 2.0, 0.5, seed + 21);
        (1.0 - smoothstep(0.62, 1.0, s + n * 0.22)) * (0.7 + 0.3 * (n * 0.5 + 0.5))
    });
    // black rock: the summit, steep crags and lava tongues down the flanks
    paint_where(t, h, rect, Layer::DarkRock, |p, _, slope| {
        let (s, ca, sa) = polar(p);
        let n = fbm(p.x / 40.0, p.y / 40.0, 4, 2.0, 0.5, seed + 22);
        let summit = 1.0 - smoothstep(0.3, 0.52, s + n * 0.12);
        let tongue = smoothstep(
            0.1,
            0.32,
            fbm(
                ca * 2.4 + s * 0.4,
                sa * 2.4 - s * 0.3,
                4,
                2.0,
                0.55,
                seed + 23,
            ) + n * 0.15,
        );
        let reach = 1.0
            - smoothstep(
                0.7,
                0.98,
                s + fbm(ca * 6.0, sa * 6.0, 3, 2.0, 0.5, seed + 24) * 0.15,
            );
        let crag = smoothstep(26.0, 38.0, slope) * (1.0 - smoothstep(0.9, 1.05, s));
        summit.max(tongue * reach).max(crag * 0.8)
    });
}

// ------------------------------------------------------------------ mountains

/// A mountain range (or a sharper `ridge`) along a polyline: a broad massif with a varying crest,
/// ridged peaks and spurs, then local erosion. Heights add to the ground.
pub fn mountain_range(
    t: &mut Terrain,
    h: &mut History,
    path: &[Vec2],
    width: f32,
    height: f32,
    sharp: bool,
    seed: u32,
    erode: bool,
) -> Option<f32> {
    let hw = width * 0.5;
    let (lo, hi) = super::selection::bounds(path);
    let rect = rect_around(t, lo - Vec2::splat(hw * 1.4), hi + Vec2::splat(hw * 1.4))?;
    let len = path_length(path).max(1.0);
    let fq = 1.0 / (width * if sharp { 0.32 } else { 0.42 });
    let mut top = f32::MIN;
    shape_heights(t, h, rect, |p, old| {
        let (d, u, _) = polyline_dist(p, path);
        let warp = fbm(
            p.x / (width * 0.9),
            p.y / (width * 0.9),
            3,
            2.0,
            0.5,
            seed + 31,
        ) * hw
            * 0.3;
        let x = (d + warp) / hw;
        if x >= 1.0 {
            return old;
        }
        let ends =
            (smoothstep(0.0, width * 0.7, u) * smoothstep(len, len - width * 0.7, u)).max(0.25);
        // several summits along the crest
        let bumps = fbm(u / (width * 0.75), 0.5, 3, 2.0, 0.5, seed + 32) * 0.5 + 0.5;
        let crest = height * (0.6 + 0.5 * bumps) * ends;
        let body = smoothstep(1.0, 0.0, x);
        let q = Vec2::new(
            p.x + fbm(p.x * fq * 0.5, p.y * fq * 0.5, 3, 2.0, 0.5, seed + 34) / fq * 0.35,
            p.y + fbm(p.x * fq * 0.5 + 7.7, p.y * fq * 0.5, 3, 2.0, 0.5, seed + 35) / fq * 0.35,
        );
        let rn = ridged(q.x * fq, q.y * fq, 6, seed + 33);
        let base = body.powf(if sharp { 1.0 } else { 1.35 });
        let ridges = rn.powf(1.8) * body.powf(0.6);
        let y = crest * (0.62 * base + 0.6 * ridges * (0.45 + 0.55 * base));
        old + y
    });
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            top = top.max(t.h(i, j));
        }
    }
    if erode {
        erode_near(t, h, rect, seed, 1.0, |p| {
            1.0 - smoothstep(hw * 0.9, hw * 1.3, polyline_dist(p, path).0)
        });
    }
    // scree on the mid slopes; snow from a little under the snow line on what isn't too steep
    let (sea, snow) = (t.sea_level, t.style.snow_line);
    paint_where(t, h, rect, Layer::Gravel, |p, _, slope| {
        let near = 1.0 - smoothstep(hw * 0.7, hw * 1.0, polyline_dist(p, path).0);
        near * smoothstep(22.0, 30.0, slope)
            * (1.0 - smoothstep(40.0, 48.0, slope))
            * smoothstep(
                -0.1,
                0.4,
                fbm(p.x / 50.0, p.y / 50.0, 3, 2.0, 0.5, seed + 36),
            )
            * 0.55
    });
    paint_where(t, h, rect, Layer::Snow, |p, ht, slope| {
        let line = sea + snow * 0.8 + fbm(p.x / 90.0, p.y / 90.0, 4, 2.0, 0.5, seed + 37) * 25.0;
        smoothstep(line - 8.0, line + 12.0, ht) * (1.0 - smoothstep(38.0, 50.0, slope))
    });
    Some(top)
}

// ------------------------------------------------------------------ crater

/// An impact crater: a bowl with a raised rim and a ring of ejecta.
pub fn crater(
    t: &mut Terrain,
    h: &mut History,
    c: Vec2,
    r: f32,
    depth: f32,
    seed: u32,
) -> Option<()> {
    let rect = t.rect_for_circle(c, r * 2.0)?;
    let base = foot_level(t, c, r * 1.4).max(t.sea_level + 1.5 + depth);
    let rim = depth * 0.3;
    shape_heights(t, h, rect, |p, old| {
        let d = p - c;
        let ang = d.y.atan2(d.x);
        let wob = 1.0 + fbm(ang.cos() * 2.0, ang.sin() * 2.0, 4, 2.0, 0.55, seed + 41) * 0.22;
        let s = d.length() / (r * wob);
        if s >= 2.0 {
            return old;
        }
        let y = if s < 1.0 {
            base - depth * (1.0 - s * s).max(0.0).powf(0.8) + rim * s.powi(6)
        } else {
            let lip = base + rim * (-((s - 1.0) / 0.12).powi(2)).exp();
            let ejecta = rim
                * 0.35
                * (1.0 - smoothstep(1.0, 2.0, s))
                * (0.5 + 0.5 * fbm(p.x / (r * 0.15), p.y / (r * 0.15), 3, 2.0, 0.5, seed + 42));
            old.max(lip) + ejecta
        };
        if s < 1.0 {
            y
        } else {
            old + (y - old) * (1.0 - smoothstep(1.6, 2.0, s))
        }
    });
    Some(())
}

// ------------------------------------------------------------------ mesas and buttes

/// A field of flat-topped mesas (or narrow buttes) with sheer, banded cliffs and talus aprons.
pub fn mesas(
    t: &mut Terrain,
    h: &mut History,
    area: &MaskField,
    count: usize,
    height: f32,
    buttes: bool,
    seed: u32,
) -> usize {
    let (lo, hi) = super::selection::bounds(&area.poly);
    let mut rng = crate::noise::Rng::new(seed as u64 * 31 + 7);
    let (rmin, rmax) = if buttes { (24.0, 60.0) } else { (55.0, 150.0) };
    let mut tops: Vec<(Vec2, f32, f32)> = vec![];
    for _ in 0..count * 40 {
        if tops.len() >= count {
            break;
        }
        let p = Vec2::new(rng.range(lo.x, hi.x), rng.range(lo.y, hi.y));
        let r = rng
            .range(rmin, rmax)
            .min(((hi - lo).min_element() * 0.3).max(rmin));
        if area.at(p) < 0.9
            || tops
                .iter()
                .any(|(q, rq, _)| q.distance(p) < (r + rq) * 1.15)
        {
            continue;
        }
        let ht = height * rng.range(0.7, 1.25) * if buttes { 1.3 } else { 1.0 };
        tops.push((p, r, ht));
    }
    let Some(rect) = rect_around(t, lo, hi) else {
        return 0;
    };
    let floor = foot_level(t, (lo + hi) * 0.5, (hi - lo).max_element() * 0.5);
    let bands = 4.0;
    shape_heights(t, h, rect, |p, old| {
        let mut best = old;
        for (k, (c, r, ht)) in tops.iter().enumerate() {
            let d = p - *c;
            let ang = d.y.atan2(d.x);
            let wob = 1.0
                + fbm(
                    ang.cos() * 1.6 + k as f32,
                    ang.sin() * 1.6,
                    4,
                    2.0,
                    0.5,
                    seed + 51,
                ) * 0.28;
            let s = d.length() / (r * wob);
            if s >= 1.9 {
                continue;
            }
            let base = old.min(floor + 2.0).max(old - 6.0);
            let top =
                base.max(floor) + ht + fbm(p.x / 40.0, p.y / 40.0, 3, 2.0, 0.5, seed + 52) * 1.5;
            let y = if s < 0.94 {
                top
            } else if s < 1.0 {
                // sheer cliff with ledges (rock bands)
                let q = 1.0 - (s - 0.94) / 0.06;
                let b = (q * bands).floor();
                let f = (q * bands).fract();
                let stepped = (b + smoothstep(0.0, 0.55, f)) / bands;
                base + (top - base) * (0.32 + 0.68 * stepped)
            } else {
                // talus apron
                let q = 1.0 - ((s - 1.0) / 0.9).clamp(0.0, 1.0);
                base + (top - base) * 0.32 * q.powf(1.8)
            };
            best = best.max(y);
        }
        best
    });
    tops.len()
}

// ------------------------------------------------------------------ canyon / valley

/// A canyon along a path: a meandering slot with banded, stepped walls cut down to a floor that
/// only ever descends (so a river can run in it). Where the ground beside it is lower than the
/// rim, shoulders rise to hold the walls up. Returns the floor's centre line (for a river).
pub fn canyon(
    t: &mut Terrain,
    h: &mut History,
    path: &[Vec2],
    depth: f32,
    width: f32,
    floor_width: f32,
    seed: u32,
) -> Option<Vec<Vec3>> {
    let step = (t.cell * 2.0).max(6.0);
    let hw = width * 0.5;
    // a sinuous course
    let base = resample(path, step);
    let len = path_length(path).max(1.0);
    let n = base.len();
    let course: Vec<Vec2> = (0..n)
        .map(|i| {
            let a = base[i.saturating_sub(1)];
            let b = base[(i + 1).min(n - 1)];
            let side = (b - a).perp().normalize_or_zero();
            let u = i as f32 * step;
            let taper = smoothstep(0.0, width, u) * smoothstep(len, len - width, u);
            base[i]
                + side * fbm(u / (width * 2.4), 0.37, 3, 2.0, 0.5, seed + 60) * width * 0.9 * taper
        })
        .collect();
    let (pts, mut floor) = descending_floor(t, &course, step, depth);
    let sea = t.sea_level;
    let total = floor.len() as f32 * step;
    for (i, f) in floor.iter_mut().enumerate() {
        // shallower towards the ends (the canyon opens out) and never below the sea
        let u = i as f32 * step;
        let open =
            0.4 + 0.6 * smoothstep(0.0, width * 1.2, u) * smoothstep(total, total - width * 1.2, u);
        *f = (*f + depth * (1.0 - open)).max(sea + 1.5);
    }
    let (lo, hi) = super::selection::bounds(&pts);
    let rect = rect_around(t, lo - Vec2::splat(hw * 1.9), hi + Vec2::splat(hw * 1.9))?;
    let xf = (floor_width / width).clamp(0.05, 0.8);
    let bands = 3.0;
    shape_heights(t, h, rect, |p, old| {
        let jitter = Vec2::new(
            fbm(p.x / 70.0, p.y / 70.0, 3, 2.0, 0.5, seed + 61),
            fbm(p.x / 70.0 + 3.1, p.y / 70.0, 3, 2.0, 0.5, seed + 62),
        ) * width
            * 0.08;
        let (d, u, _) = polyline_dist(p + jitter, &pts);
        // ragged rim with side gullies
        let gully = ridged(p.x / 55.0, p.y / 55.0, 3, seed + 63);
        let wob = 1.0
            + fbm(p.x / 110.0, p.y / 110.0, 3, 2.0, 0.5, seed + 64) * 0.22
            + gully.powi(3) * 0.25;
        let x = d / (hw * wob);
        if x >= 1.8 {
            return old;
        }
        let fl = floor_at(&floor, step, u);
        let rim = fl + depth * (1.0 + fbm(p.x / 160.0, p.y / 160.0, 3, 2.0, 0.5, seed + 65) * 0.1);
        let top = old.max(rim);
        if x < xf {
            return fl + fbm(p.x / 25.0, p.y / 25.0, 2, 2.0, 0.5, seed + 66) * 0.5;
        }
        if x < 1.0 {
            let w = ((x - xf) / (1.0 - xf)
                + fbm(p.x / 45.0, p.y / 45.0, 3, 2.0, 0.5, seed + 67) * 0.06)
                .clamp(0.0, 1.0);
            let b = (w * bands).floor();
            let f = (w * bands).fract();
            let stepped = (b + smoothstep(0.08, 0.8, f)) / bands;
            let prof = (w * 0.35 + stepped * 0.65).powf(0.8);
            return fl + (top - fl) * prof;
        }
        if old >= rim {
            old
        } else {
            rim + (old - rim) * smoothstep(1.0, 1.8, x)
        }
    });
    // banded walls: brown and grey rock strata
    let band_h = (depth / 5.0).max(4.0);
    paint_where(t, h, rect, Layer::Dirt, |p, ht, slope| {
        let d = polyline_dist(p, &pts).0;
        if d > hw * 1.3 || slope < 24.0 {
            return 0.0;
        }
        let k = (ht / band_h + fbm(p.x / 60.0, p.y / 60.0, 2, 2.0, 0.5, seed + 68) * 0.6).floor()
            as i32;
        if k.rem_euclid(2) == 0 { 0.8 } else { 0.0 }
    });
    let centre: Vec<Vec3> = pts
        .iter()
        .filter(|p| t.in_bounds(p.x, p.y))
        .map(|p| Vec3::new(p.x, t.height_at(p.x, p.y), p.y))
        .collect();
    Some(centre)
}

/// A valley along a path: U-shaped (glacial) or V-shaped (river-cut), floor descending.
pub fn valley(
    t: &mut Terrain,
    h: &mut History,
    path: &[Vec2],
    depth: f32,
    width: f32,
    u_shape: bool,
    seed: u32,
) -> Option<()> {
    let step = (t.cell * 2.0).max(6.0);
    let (pts, mut floor) = descending_floor(t, path, step, depth);
    let sea = t.sea_level;
    floor.iter_mut().for_each(|f| *f = f.max(sea + 1.5));
    let hw = width * 0.5;
    let (lo, hi) = super::selection::bounds(&pts);
    let rect = rect_around(t, lo - Vec2::splat(hw * 1.3), hi + Vec2::splat(hw * 1.3))?;
    shape_heights(t, h, rect, |p, old| {
        let warp = Vec2::new(
            fbm(p.x / 400.0, p.y / 400.0, 3, 2.0, 0.5, seed + 71),
            fbm(p.x / 400.0, p.y / 400.0 + 5.3, 3, 2.0, 0.5, seed + 72),
        ) * width
            * 0.25;
        let (d, u, _) = polyline_dist(p + warp, &pts);
        let x = d / hw;
        if x >= 1.0 {
            return old;
        }
        let fl = floor_at(&floor, step, u);
        if old <= fl {
            return old;
        }
        let shape = if u_shape { x.powf(2.2) } else { x.powf(1.1) };
        let y = fl + (old - fl) * shape;
        old + (y.min(old) - old) * (1.0 - smoothstep(0.8, 1.0, x))
    });
    Some(())
}

// ------------------------------------------------------------------ dunes, hills, plateau

/// Desert dunes over an area: the relief is flattened into a sand sea, then asymmetric
/// transverse dunes (gentle windward side, steep slip face) with sinuous crests and smaller
/// cross-dunes are added. `wind_deg` is the compass heading the wind blows towards.
pub fn dunes(
    t: &mut Terrain,
    h: &mut History,
    area: &MaskField,
    height: f32,
    wavelength: f32,
    wind_deg: f32,
    seed: u32,
) {
    let mean = mean_height(t, area).max(t.sea_level + 2.0);
    // compass heading (0 = north = −z, clockwise) → a direction in x, z
    let heading = |deg: f32| Vec2::new(deg.to_radians().sin(), -deg.to_radians().cos());
    let dir = heading(wind_deg);
    let perp = dir.perp();
    let dir2 = heading(wind_deg + 52.0);
    let l = wavelength.max(20.0);
    let dune = |u: f32| -> f32 {
        let f = u.rem_euclid(1.0);
        if f < 0.74 {
            smoothstep(0.0, 0.74, f).powf(1.15)
        } else {
            1.0 - smoothstep(0.74, 1.0, f)
        }
    };
    shape_heights(t, h, area.rect, |p, old| {
        let w = area.at(p);
        if w <= 0.0 {
            return old;
        }
        let base = old
            + (mean + fbm(p.x / (l * 9.0), p.y / (l * 9.0), 3, 2.0, 0.5, seed + 81) * height * 1.2
                - old)
                * 0.55;
        let warp = fbm(p.x / (l * 2.6), p.y / (l * 2.6), 3, 2.0, 0.5, seed + 82) * 0.55
            + fbm(p.x / (l * 0.8), p.y / (l * 0.8), 2, 2.0, 0.5, seed + 83) * 0.12;
        let u = p.dot(dir) / l + warp + 0.2 * (p.dot(perp) / (l * 1.9)).sin();
        let amp =
            height * (0.55 + 0.45 * fbm(p.x / (l * 5.0), p.y / (l * 5.0), 3, 2.0, 0.5, seed + 84));
        let u2 = p.dot(dir2) / (l * 0.45) + warp * 1.3;
        let y = base
            + amp * dune(u)
            + amp * 0.22 * dune(u2)
            + fbm(p.x / 9.0, p.y / 9.0, 2, 2.0, 0.5, seed + 85) * 0.15;
        old + (y - old) * w
    });
}

/// Rolling hills over an area (added to the ground).
pub fn hills(
    t: &mut Terrain,
    h: &mut History,
    area: &MaskField,
    height: f32,
    wavelength: f32,
    seed: u32,
) {
    let l = wavelength.max(30.0);
    shape_heights(t, h, area.rect, |p, old| {
        let w = area.at(p);
        if w <= 0.0 {
            return old;
        }
        let n = fbm(p.x / l, p.y / l, 5, 2.0, 0.45, seed + 91) * 0.5 + 0.5;
        old + height * n.powf(1.3) * w
    });
}

/// A plateau: the area rises to a flat-ish top `height` m above its median ground. Its edge is a
/// lobed escarpment: rock bands over a talus apron (the area field's feather sets the width).
pub fn plateau(t: &mut Terrain, h: &mut History, area: &MaskField, height: f32, seed: u32) -> f32 {
    let top = median_height(t, area) + height;
    shape_heights(t, h, area.rect, |p, old| {
        let w = area.at(p);
        if w <= 0.0 {
            return old;
        }
        // cliff in the upper part of the edge, scree below
        let q = (w + fbm(p.x / 30.0, p.y / 30.0, 3, 2.0, 0.5, seed + 102) * 0.18).clamp(0.0, 1.0);
        let talus = smoothstep(0.0, 0.45, q).powf(1.6) * 0.35;
        let cliff = smoothstep(0.32, 0.8, q);
        let b = (cliff * 3.0).floor();
        let f = (cliff * 3.0).fract();
        let banded = ((b + smoothstep(0.0, 0.6, f)) / 3.0).min(1.0);
        let k = talus + (1.0 - 0.35) * banded;
        let y = top + fbm(p.x / 140.0, p.y / 140.0, 4, 2.0, 0.5, seed + 101) * height * 0.1;
        old.max(old + (y - old) * k)
    });
    top
}

fn heights_in(t: &Terrain, area: &MaskField) -> Vec<f32> {
    let r = area.rect;
    let mut v = vec![];
    for j in (r.z0..=r.z1).step_by(2) {
        for i in (r.x0..=r.x1).step_by(2) {
            if area.at_grid(i, j) > 0.5 {
                v.push(t.h(i, j));
            }
        }
    }
    if v.is_empty() {
        v.push(t.h((r.x0 + r.x1) / 2, (r.z0 + r.z1) / 2));
    }
    v
}

fn mean_height(t: &Terrain, area: &MaskField) -> f32 {
    let v = heights_in(t, area);
    v.iter().sum::<f32>() / v.len() as f32
}

fn median_height(t: &Terrain, area: &MaskField) -> f32 {
    percentile(heights_in(t, area), 0.5)
}

fn percentile(mut v: Vec<f32>, q: f32) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f32 * q).round() as usize]
}

// ------------------------------------------------------------------ coast

/// Sea cliffs along a coastline path: the land side rises to a clifftop, the face drops sheer
/// (jagged, with rock bands) into the sea. The sea side is found from the ground on each side.
pub fn sea_cliffs(
    t: &mut Terrain,
    h: &mut History,
    path: &[Vec2],
    height: f32,
    width: f32,
    seed: u32,
) -> Option<()> {
    let sea = t.sea_level;
    // which side is the sea: the lower one, on average
    let mut votes = 0.0;
    for w in path.windows(2) {
        let m = (w[0] + w[1]) * 0.5;
        let n = (w[1] - w[0]).perp().normalize_or_zero();
        votes += t.height_at(m.x + n.x * width * 0.5, m.y + n.y * width * 0.5)
            - t.height_at(m.x - n.x * width * 0.5, m.y - n.y * width * 0.5);
    }
    let land_side = if votes >= 0.0 { 1.0 } else { -1.0 };
    let (lo, hi) = super::selection::bounds(path);
    let rect = rect_around(
        t,
        lo - Vec2::splat(width * 1.3),
        hi + Vec2::splat(width * 1.3),
    )?;
    let len = path_length(path);
    shape_heights(t, h, rect, |p, old| {
        let (d, u, side) = polyline_dist(p, path);
        let jag = fbm(u / 35.0, 0.3, 4, 2.0, 0.55, seed + 111) * 14.0
            + fbm(p.x / 12.0, p.y / 12.0, 2, 2.0, 0.5, seed + 112) * 3.0;
        let s = d * side * land_side + jag; // + land, − sea
        if s > width * 1.2 || s < -width * 0.5 {
            return old;
        }
        let ends = smoothstep(0.0, width * 0.6, u) * smoothstep(len, len - width * 0.6, u);
        let top = sea + height * (0.85 + 0.3 * fbm(u / 160.0, 1.7, 3, 2.0, 0.5, seed + 113)) * ends;
        let y = if s >= 0.0 {
            // clifftop sloping gently inland into the existing ground
            let inland = smoothstep(width * 0.55, width * 1.2, s);
            let b = ((top - sea) / 6.0).max(1.0);
            let lip = top + (old - top) * inland;
            let face = smoothstep(0.0, 5.0, s);
            let banded = sea - 3.0
                + (lip - sea + 3.0)
                    * ((face * b).floor() + smoothstep(0.0, 0.6, (face * b).fract()))
                    / b;
            old.max(banded)
        } else {
            // the foot: a rocky shelf just under the waves
            let q = (-s / (width * 0.5)).clamp(0.0, 1.0);
            let shelf =
                sea - 2.5 - 6.0 * q + fbm(p.x / 20.0, p.y / 20.0, 2, 2.0, 0.5, seed + 114) * 1.2;
            old.min(shelf + (old - shelf).max(0.0) * q)
        };
        old + (y - old) * ends
    });
    Some(())
}

/// A coastal lagoon: a shallow basin (a few metres under sea level) stretched along the coast,
/// enclosed on the sea side by a sandy barrier with `inlets` gaps, with a few sandy islets.
/// `seaward` points from the centre towards the open sea.
#[allow(clippy::too_many_arguments)]
pub fn lagoon(
    t: &mut Terrain,
    h: &mut History,
    c: Vec2,
    r: f32,
    depth: f32,
    barrier: f32,
    inlets: u32,
    seaward: Vec2,
    seed: u32,
) -> Option<()> {
    let sea = t.sea_level;
    let rect = t.rect_for_circle(c, r * 1.6 + barrier * 3.0)?;
    let out_dir = seaward.normalize_or(Vec2::Y);
    let along = out_dir.perp();
    let sea_ang = out_dir.y.atan2(out_dir.x);
    let gaps: Vec<f32> = (0..inlets)
        .map(|k| {
            sea_ang
                + (k as f32 - (inlets as f32 - 1.0) * 0.5) * 0.9
                + fbm(k as f32 * 1.7, 0.5, 2, 2.0, 0.5, seed + 121) * 0.4
        })
        .collect();
    shape_heights(t, h, rect, |p, old| {
        let d = p - c;
        // elliptical: 1.5× longer along the coast
        let e = Vec2::new(d.dot(along) / 1.5, d.dot(out_dir));
        let ang = d.y.atan2(d.x);
        let wob = 1.0 + fbm(ang.cos() * 1.6, ang.sin() * 1.6, 4, 2.0, 0.55, seed + 122) * 0.3;
        let rr = r * wob;
        let s = e.length();
        if s > rr + barrier * 2.8 {
            return old;
        }
        let gap = gaps
            .iter()
            .map(|g| ((ang - g + PI).rem_euclid(TAU) - PI).abs())
            .fold(PI, f32::min);
        let gap_w = barrier * 1.4 / rr;
        let in_gap = 1.0 - smoothstep(gap_w * 0.5, gap_w * 1.1, gap);
        let islet = smoothstep(
            0.5,
            0.6,
            fbm(p.x / 60.0, p.y / 60.0, 3, 2.0, 0.5, seed + 125),
        ) * (1.0 - smoothstep(0.45, 0.7, s / rr));
        let basin = sea - depth * (1.0 - (s / rr).powi(4)).max(0.0).powf(0.6) - 0.3
            + fbm(p.x / 140.0, p.y / 140.0, 3, 2.0, 0.5, seed + 123) * 0.35;
        let basin = basin + (sea + 0.5 - basin) * islet;
        let bar_top = sea + 1.7 + fbm(p.x / 40.0, p.y / 40.0, 3, 2.0, 0.5, seed + 124) * 0.8;
        let x = (s - rr) / barrier; // 0 at the inner shore … 1 at the outer
        let bar = bar_top * (1.0 - smoothstep(0.8, 2.5, x)) + (sea - 3.5) * smoothstep(0.8, 2.5, x);
        let bar = bar + (sea - 2.2 - bar) * in_gap;
        let inner = smoothstep(-0.25, 0.1, x);
        let y = basin + (old.max(bar) - basin) * inner;
        y + (old - y) * smoothstep(1.5, 2.8, x)
    });
    Some(())
}

// ------------------------------------------------------------------ swamp

/// Swamp ground over an area: the land settles to a low, wet level with hummocks, cut by a
/// meandering network of shallow channels and ponds. Returns the water level and points inside
/// the channels (to fill with water).
pub fn swamp(
    t: &mut Terrain,
    h: &mut History,
    area: &MaskField,
    level: Option<f32>,
    seed: u32,
) -> (f32, Vec<Vec2>) {
    let sea = t.sea_level;
    let lowish = percentile(heights_in(t, area), 0.25);
    // (a bank all round, inside the area's soft edge, holds the water in)
    let lvl = level.unwrap_or(lowish.max(sea + 2.0)).max(sea + 1.0);
    let floor = sea - 0.3;
    let warp = |p: Vec2| {
        p + Vec2::new(
            fbm(p.x / 260.0, p.y / 260.0, 3, 2.0, 0.5, seed + 131),
            fbm(p.x / 260.0, p.y / 260.0 + 3.3, 3, 2.0, 0.5, seed + 132),
        ) * 120.0
    };
    let channels = move |p: Vec2| -> f32 {
        let q = warp(p);
        let a = 1.0
            - smoothstep(
                0.035,
                0.09,
                fbm(q.x / 210.0, q.y / 210.0, 3, 2.0, 0.5, seed + 133).abs(),
            );
        let b = 1.0
            - smoothstep(
                0.02,
                0.05,
                fbm(q.x / 90.0, q.y / 90.0, 3, 2.0, 0.5, seed + 134).abs(),
            );
        let pond = smoothstep(
            0.2,
            0.36,
            fbm(p.x / 90.0, p.y / 90.0, 3, 2.0, 0.5, seed + 135),
        );
        a.max(b * 0.75).max(pond)
    };
    let step = t.cell * 2.0;
    shape_heights(t, h, area.rect, |p, old| {
        let w = area.at(p);
        if w <= 0.0 {
            return old;
        }
        let hummock = lvl
            + 0.45
            + fbm(p.x / 35.0, p.y / 35.0, 3, 2.0, 0.5, seed + 136) * 0.8
            + fbm(p.x / 9.0, p.y / 9.0, 2, 2.0, 0.5, seed + 137) * 0.2;
        let mut y = (hummock - channels(p) * 1.9).max(floor);
        // a low, unbroken bank holds the water in: around the edge of the part the swamp fully
        // owns (no soft edge blends it there), and everything outside that part
        let rim = [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y]
            .iter()
            .any(|o| area.at(p + *o * step) < 0.999);
        if w < 0.999 || rim {
            y = y.max(lvl + 0.4);
        }
        old + (y - old) * w
    });
    let mut seeds = vec![];
    let r = area.rect;
    let stride = ((12.0 / t.cell).ceil() as usize).max(1);
    for j in (r.z0..=r.z1).step_by(stride) {
        for i in (r.x0..=r.x1).step_by(stride) {
            if area.at_grid(i, j) >= 0.999 && t.h(i, j) < lvl - 0.7 {
                seeds.push(t.grid_to_world(i, j));
            }
        }
    }
    (lvl, seeds)
}

// ------------------------------------------------------------------ lake island

/// Raises an island inside a lake whose surface is at `level`: a sandy beach ring rising to a
/// rounded grassy top.
pub fn island(
    t: &mut Terrain,
    h: &mut History,
    c: Vec2,
    r: f32,
    level: f32,
    height: f32,
    seed: u32,
) -> Option<()> {
    let rect = t.rect_for_circle(c, r * 1.4)?;
    shape_heights(t, h, rect, |p, old| {
        let d = p - c;
        let ang = d.y.atan2(d.x);
        let wob = 1.0
            + fbm(ang.cos() * 1.4, ang.sin() * 1.4, 4, 2.0, 0.6, seed + 141) * 0.7
            + fbm(ang.cos() * 4.0, ang.sin() * 4.0, 2, 2.0, 0.5, seed + 143) * 0.15;
        let s = d.length() / (r * wob);
        if s >= 1.35 {
            return old;
        }
        let dome = level
            + 0.6
            + height
                * (1.0 - s * s).max(0.0).powf(0.8)
                * (0.7
                    + 0.6
                        * (fbm(p.x / (r * 0.6), p.y / (r * 0.6), 3, 2.0, 0.5, seed + 144) * 0.5
                            + 0.5))
            + fbm(p.x / 25.0, p.y / 25.0, 3, 2.0, 0.5, seed + 142) * 0.6;
        let shore = level - 2.5 * smoothstep(1.0, 1.35, s);
        let y = if s < 1.0 {
            dome
        } else {
            shore + (level + 0.6 - shore) * (1.0 - smoothstep(1.0, 1.15, s))
        };
        old.max(y.min(old.max(y)))
    });
    Some(())
}
