//! Layered canyonlands: raised plateaus cut by branching canyon networks, terraced walls with
//! strata ledges, open basins with isolated mesas and buttes, and canyon floors at a few distinct
//! levels, so lakes sit at different heights and water falls from tier to tier.
//!
//! The landform is a pure height function (`canyon_height`, metres above the lowest canyon floor)
//! sampled over the whole world (`generate_canyonlands`) or a disc of it blended into the ground
//! round it (`generate_canyonlands_region`). Neither records undo: callers snapshot first with
//! `History::touch_terrain` (see `canyon_rect`), as the Sculpt tool and the MCP tool do.

use bevy::prelude::*;

use super::brush::falloff_weight;
use super::strata::{StrataParams, strata_height};
use super::{GridRect, Terrain};
use crate::noise::{Rng, fbm, smoothstep};

/// The canyonlands' look. Defaults suit a 4 km world; see `CanyonParams::alien` for the Sci-Fi look.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CanyonParams {
    /// Height of the high plateau above the lowest canyon floor (m).
    pub relief: f32,
    /// Absolute height of the lowest canyon floor (m). None: 6 m above sea for the whole world,
    /// the ground's mean height for a region.
    pub floor: Option<f32>,
    /// Typical distance between main canyons (m).
    pub scale: f32,
    /// × how wide the canyons are (0.4 narrow slots … 2 broad valleys).
    pub canyon_width: f32,
    /// 0..1 side canyons branching off the main ones (hanging valleys, gullies).
    pub branching: f32,
    /// 0..1 open basins where the plateau has worn away, leaving isolated mesas and buttes.
    pub mesas: f32,
    /// Distinct canyon-floor levels (1..5): the floors step down from tier to tier.
    pub tiers: u32,
    /// 0..1 the plateau itself steps (two or three plateau levels).
    pub plateau_steps: f32,
    /// Ledges on the walls.
    pub strata: StrataParams,
    /// 0..1 shallow lake basins dug into the canyon floors (on every tier).
    pub lakes: f32,
}

impl Default for CanyonParams {
    fn default() -> Self {
        Self {
            relief: 180.0,
            floor: None,
            scale: 1100.0,
            canyon_width: 1.0,
            branching: 0.6,
            mesas: 0.45,
            tiers: 3,
            plateau_steps: 0.5,
            strata: StrataParams {
                step: 16.0,
                sharpness: 0.75,
                irregularity: 0.55,
            },
            lakes: 0.0,
        }
    }
}

impl CanyonParams {
    /// A vast alien canyonland (the Sci-Fi template): deep, wide, many tiers, crisp strata.
    pub fn alien() -> Self {
        Self {
            relief: 260.0,
            floor: None,
            scale: 1300.0,
            canyon_width: 1.25,
            branching: 0.75,
            mesas: 0.6,
            tiers: 4,
            plateau_steps: 0.7,
            strata: StrataParams {
                step: 20.0,
                sharpness: 0.85,
                irregularity: 0.6,
            },
            lakes: 0.6,
        }
    }

    pub fn clamped(self) -> Self {
        Self {
            relief: self.relief.clamp(10.0, 1200.0),
            floor: self.floor,
            scale: self.scale.clamp(150.0, 20_000.0),
            canyon_width: self.canyon_width.clamp(0.2, 3.0),
            branching: self.branching.clamp(0.0, 1.0),
            mesas: self.mesas.clamp(0.0, 1.0),
            tiers: self.tiers.clamp(1, 5),
            plateau_steps: self.plateau_steps.clamp(0.0, 1.0),
            strata: self.strata.clamped(),
            lakes: self.lakes.clamp(0.0, 1.0),
        }
    }
}

/// What a generation made.
#[derive(Clone, Debug, Default)]
pub struct CanyonReport {
    /// Absolute height of the lowest canyon floor (m).
    pub floor: f32,
    /// Absolute height of the highest plateau (m).
    pub top: f32,
    /// Absolute heights of the canyon-floor tiers, lowest first.
    pub tier_heights: Vec<f32>,
    /// Lake basins dug into the floors: centre, radius (m) and the floor height round them (m).
    /// Fill one with water a few metres deep (`water_add_lake` `point` + `fill_depth`).
    pub lake_sites: Vec<LakeSite>,
}

#[derive(Clone, Copy, Debug)]
pub struct LakeSite {
    pub center: Vec2,
    pub radius: f32,
    /// Ground height round the basin (m): water up to ~1 m below this stays inside.
    pub rim: f32,
    /// Which floor tier (0 = lowest).
    pub tier: u32,
}

/// One sample of the landform.
#[derive(Clone, Copy, Debug)]
pub struct CanyonSample {
    /// Metres above the lowest canyon floor.
    pub height: f32,
    /// 0..1 how deep in a main canyon (1 = on its floor).
    pub main: f32,
    /// The floor tier here (0 = lowest).
    pub tier: u32,
    /// Floor height of this spot's tier above the lowest floor (m).
    pub tier_floor: f32,
}

/// Steps of `x` (0..1) into `n` levels with short steep ramps between them.
#[inline]
fn tiers_of(x: f32, n: u32) -> (u32, f32) {
    if n <= 1 {
        return (0, 0.0);
    }
    let v = (x.clamp(0.0, 0.999) * n as f32).max(0.0);
    let k = v.floor();
    let level = k + smoothstep(0.94, 1.0, v - k);
    (k as u32, level / (n - 1) as f32)
}

/// The canyonland at world position `p` (m) (placing props by canyon / tier, tests).
#[allow(dead_code)]
pub fn canyon_sample(p: Vec2, cp: &CanyonParams, seed: u32) -> CanyonSample {
    sample_open(p, cp, seed, 0.0)
}

/// `canyon_sample` with the plateau worn open by `open` (0..1): towards 1 the land becomes a
/// basin plain on the lowest floor with isolated buttes (the world's edge).
fn sample_open(p: Vec2, cp: &CanyonParams, seed: u32, open: f32) -> CanyonSample {
    let cp = cp.clamped();
    let q = p / cp.scale;
    // domain warp: canyons meander and branch at odd angles
    let wq = q + Vec2::new(
        fbm(q.x * 0.8 + 3.1, q.y * 0.8 + 7.7, 3, 2.0, 0.5, seed + 1),
        fbm(q.x * 0.8 - 5.2, q.y * 0.8 + 1.3, 3, 2.0, 0.5, seed + 2),
    ) * 0.45;
    let w = 0.06 * cp.canyon_width;
    // walls are notched by alcoves and spurs
    let notch = 1.0 + fbm(q.x * 9.0, q.y * 9.0, 3, 2.0, 0.5, seed + 11) * 0.35;
    // main canyons: the zero lines of a smooth noise form a connected, meandering network. Two
    // shelves: the inner canyon down to the floor and a broad bench part-way up its walls
    let m = fbm(wq.x, wq.y, 3, 2.0, 0.45, seed + 3).abs();
    let mj = m * notch;
    let main = 1.0 - smoothstep(w * 0.55, w * 1.05, mj);
    let bench = 1.0 - smoothstep(w * 1.55, w * 2.1, mj);
    let canyon = 0.6 * main + 0.4 * bench;
    // how close to a main canyon (side canyons deepen towards it: hanging valleys)
    let near = 1.0 - smoothstep(w * 2.0, w * 7.0, m);
    // side canyons: finer zero lines, near the main canyons; deepest where they meet one,
    // shallow towards their heads, their floors hanging above the main floor
    let tq = wq * 1.9 + Vec2::new(17.3, -9.1);
    let t = fbm(tq.x, tq.y, 3, 2.0, 0.5, seed + 4).abs() * notch;
    let wt = w * 0.8;
    let side = (1.0 - smoothstep(wt * 0.5, wt * 1.05, t))
        * cp.branching
        * (0.25 + 0.4 * near)
        * smoothstep(0.0, 0.35, near + 0.25 * cp.branching - 0.1);
    // open basins: the plateau worn away, leaving isolated mesas and buttes
    let e = smoothstep(
        0.1,
        0.15,
        fbm(q.x * 0.55 + 41.0, q.y * 0.55 - 13.0, 3, 2.0, 0.5, seed + 6) * notch
            + (cp.mesas - 0.5) * 0.3,
    ) * smoothstep(0.0, 0.15, cp.mesas);
    let bq = wq * 3.0 + Vec2::new(5.5, 22.0);
    let b = fbm(bq.x, bq.y, 3, 2.0, 0.45, seed + 7) * notch;
    let butte = smoothstep(0.1, 0.14, b);
    // deeper in a basin the buttes stand lower (worn further down)
    let e = e.max(open);
    let basin = e * (1.0 - butte * (1.0 - 0.4 * e));
    let d = canyon.max(side).max(basin);

    // canyon floors step down across the land in tiers
    let tf = fbm(q.x * 0.32 - 7.0, q.y * 0.32 + 19.0, 2, 2.0, 0.5, seed + 8) * 1.6 + 0.5;
    let (tier, lv) = tiers_of(tf, cp.tiers);
    let tier_floor = lv * cp.relief * 0.42 * (1.0 - smoothstep(0.1, 0.3, open));
    // the plateau steps too
    let pf = fbm(q.x * 0.45 + 2.0, q.y * 0.45 - 3.0, 2, 2.0, 0.5, seed + 9) * 1.6 + 0.5;
    let (_, pl) = tiers_of(pf, 3);
    let top = cp.relief * (1.0 - 0.22 * cp.plateau_steps * (1.0 - pl))
        + fbm(q.x * 1.5, q.y * 1.5, 2, 2.0, 0.5, seed + 10) * cp.strata.step * 0.15;
    let mut h = tier_floor + (top - tier_floor) * (1.0 - d);
    // ledges on the walls; the floors stay on their tier
    h = strata_height(h, p, &cp.strata, seed ^ 0x5717).max(tier_floor);
    // an inner gorge where the river runs, below the strata
    let gorge = 1.0 - smoothstep(0.0, w * 0.12, m);
    h -= gorge * (2.0 + cp.strata.step * 0.15);
    CanyonSample {
        height: h,
        main,
        tier,
        tier_floor,
    }
}

/// The canyonland's height at `p` above its lowest floor (m).
#[allow(dead_code)]
pub fn canyon_height(p: Vec2, cp: &CanyonParams, seed: u32) -> f32 {
    canyon_sample(p, cp, seed).height
}

/// The grid rect a region generation changes (snapshot it for undo first).
pub fn canyon_rect(t: &Terrain, center: Vec2, radius: f32) -> Option<GridRect> {
    t.rect_for_circle(center, radius)
}

/// Samples `rect` on all cores: one `CanyonSample` per vertex, row-major.
fn sample_rect(
    t: &Terrain,
    rect: GridRect,
    cp: &CanyonParams,
    seed: u32,
    edge: &(impl Fn(Vec2) -> (f32, f32) + Sync),
) -> Vec<CanyonSample> {
    let rows: Vec<usize> = (rect.z0..=rect.z1).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = rows.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = rows
            .chunks(chunk)
            .map(|js| {
                s.spawn(move || {
                    let mut out = Vec::with_capacity(js.len() * rect.width());
                    for &j in js {
                        for i in rect.x0..=rect.x1 {
                            let p = t.grid_to_world(i, j);
                            out.push(sample_open(p, cp, seed, edge(p).0));
                        }
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    })
}

/// Turn the whole world into canyonlands. Heights only (paint, biomes, water untouched).
pub fn generate_canyonlands(t: &mut Terrain, params: &CanyonParams, seed: u32) -> CanyonReport {
    let n = t.n();
    let rect = GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    };
    let floor = params.floor.unwrap_or(t.sea_level + 6.0);
    // towards the world's border the canyonlands give way to a low plain that meets the sea,
    // so no sheer wall stands at the edge of the world
    let half = t.half();
    let edge = move |p: Vec2| {
        let raw = p.x.abs().max(p.y.abs()) / half;
        let r = raw + fbm(p.x / 800.0, p.y / 800.0, 4, 2.0, 0.5, seed ^ 0xed6e) * 0.3;
        let plain = smoothstep(0.85, 0.875, r);
        // (the very border always lies under the sea: nothing stands at the world's edge)
        let shore = smoothstep(0.95, 1.0, r).max(smoothstep(0.96, 0.995, raw));
        (plain, shore)
    };
    apply(t, rect, params, seed, floor, |_| 1.0, edge)
}

/// Canyonlands in a disc of `radius` m round `center`, blending into the ground round it over
/// the outer ~30% (a ragged edge). The lowest floor is `params.floor` or the ground's mean height
/// there. Canyons are spaced at most 1.6 × `radius` apart, so even a small disc holds some.
/// None when the disc misses the world.
pub fn generate_canyonlands_region(
    t: &mut Terrain,
    params: &CanyonParams,
    seed: u32,
    center: Vec2,
    radius: f32,
) -> Option<CanyonReport> {
    let rect = canyon_rect(t, center, radius)?;
    let params = &CanyonParams {
        scale: params.scale.min(radius.max(100.0) * 1.6),
        ..*params
    };
    let floor = params.floor.unwrap_or_else(|| {
        let (mut sum, mut cnt) = (0.0, 0.0);
        for j in (rect.z0..=rect.z1).step_by(4) {
            for i in (rect.x0..=rect.x1).step_by(4) {
                if t.grid_to_world(i, j).distance(center) <= radius {
                    sum += t.h(i, j);
                    cnt += 1.0;
                }
            }
        }
        if cnt > 0.0 {
            (sum / cnt).max(t.sea_level + 3.0)
        } else {
            t.sea_level + 6.0
        }
    });
    let r = radius.max(1.0);
    Some(apply(
        t,
        rect,
        params,
        seed,
        floor,
        move |p| {
            let edge = fbm(p.x / (r * 0.35), p.y / (r * 0.35), 3, 2.0, 0.5, seed ^ 0xe09e) * 0.18;
            falloff_weight((p.distance(center) / r + edge).max(0.0), 0.35)
        },
        |_| (0.0, 0.0),
    ))
}

fn apply(
    t: &mut Terrain,
    rect: GridRect,
    params: &CanyonParams,
    seed: u32,
    floor: f32,
    weight: impl Fn(Vec2) -> f32,
    // (how far the land gives way to a plain, how far it dips under the sea) at a point
    edge: impl Fn(Vec2) -> (f32, f32) + Sync,
) -> CanyonReport {
    let cp = params.clamped();
    let sea = t.sea_level;
    let samples = sample_rect(t, rect, &cp, seed, &edge);
    let w = rect.width();
    let mut top = f32::MIN;
    for (k, s) in samples.iter().enumerate() {
        let (i, j) = (rect.x0 + k % w, rect.z0 + k / w);
        let p = t.grid_to_world(i, j);
        let wt = weight(p);
        if wt <= 0.0 {
            continue;
        }
        let idx = t.idx(i, j);
        // (the plain at the world's edge dips to the shore)
        // the plain at the world's edge dips under the sea
        let shore = edge(p).1;
        let new = floor + s.height + (sea - 10.0 - floor - s.height) * shore;
        t.heights[idx] += (new - t.heights[idx]) * wt;
        top = top.max(new);
    }
    let tier_heights = (0..cp.tiers)
        .map(|k| {
            let lv = k as f32 / (cp.tiers.max(2) - 1) as f32;
            floor + lv * cp.relief * 0.42
        })
        .collect();
    // one soft pass keeps the cliff risers from stair-stepping along the grid
    soften(t, rect, 0.5, &weight);
    let lake_sites = if cp.lakes > 0.0 {
        dig_lakes(t, rect, &samples, &cp, seed, &weight, &edge)
    } else {
        vec![]
    };
    t.mark_heights_dirty(rect);
    CanyonReport {
        floor,
        top,
        tier_heights,
        lake_sites,
    }
}

/// One 3×3 blur pass over `rect` (amount 0..1, × the region weight).
fn soften(t: &mut Terrain, rect: GridRect, amount: f32, weight: &impl Fn(Vec2) -> f32) {
    let n = t.n();
    let src = t.heights.clone();
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let mut sum = 0.0;
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    let x = (i as i32 + dx).clamp(0, n as i32 - 1) as usize;
                    let z = (j as i32 + dz).clamp(0, n as i32 - 1) as usize;
                    sum += src[z * n + x];
                }
            }
            let k = j * n + i;
            let w = weight(t.grid_to_world(i, j));
            if w > 0.0 {
                t.heights[k] += (sum / 9.0 - src[k]) * amount * w;
            }
        }
    }
}

/// Shallow round basins on the main canyons' floors, a few per tier, well inside each tier (not
/// on the steps between them). Returns where they are.
fn dig_lakes(
    t: &mut Terrain,
    rect: GridRect,
    samples: &[CanyonSample],
    cp: &CanyonParams,
    seed: u32,
    weight: &impl Fn(Vec2) -> f32,
    edge: &impl Fn(Vec2) -> (f32, f32),
) -> Vec<LakeSite> {
    let w = rect.width();
    let cell = t.cell;
    let radius = (cp.scale * 0.06 * cp.canyon_width).clamp(25.0, 160.0);
    let spacing = radius * 5.0;
    let want = ((rect.width() * rect.height()) as f32 * cell * cell / (spacing * spacing)
        * cp.lakes
        * 0.35)
        .ceil() as usize;
    let mut rng = Rng::new(seed as u64 ^ 0x1a4e);
    let mut sites: Vec<LakeSite> = vec![];
    let tries = want * 60 + 50;
    for _ in 0..tries {
        if sites.len() >= want {
            break;
        }
        let i = rect.x0 + rng.below(w as u32) as usize;
        let j = rect.z0 + rng.below(rect.height() as u32) as usize;
        let s = samples[(j - rect.z0) * w + (i - rect.x0)];
        let c = t.grid_to_world(i, j);
        if s.main < 0.98 || weight(c) < 0.99 || edge(c).0 > 0.0 || sites.iter().any(|o| o.center.distance(c) < spacing) {
            continue;
        }
        // the whole basin must sit on this tier's flat floor (no tier step under it)
        let ok = (0..12).all(|k| {
            let q = c + Vec2::from_angle(k as f32 * std::f32::consts::TAU / 12.0) * radius * 1.3;
            if !t.in_bounds(q.x, q.y) {
                return false;
            }
            let g = t.world_to_grid(q.x, q.y);
            let (gi, gj) = (g.x.round() as usize, g.y.round() as usize);
            if gi < rect.x0 || gi > rect.x1 || gj < rect.z0 || gj > rect.z1 {
                return false;
            }
            let o = samples[(gj - rect.z0) * w + (gi - rect.x0)];
            o.tier == s.tier && (o.tier_floor - s.tier_floor).abs() < 0.5
        });
        if !ok {
            continue;
        }
        let rim = t.height_at(c.x, c.y) + 0.5;
        let depth = 3.0 + cp.strata.step * 0.15;
        if let Some(r) = t.rect_for_circle(c, radius * 1.2) {
            for jj in r.z0..=r.z1 {
                for ii in r.x0..=r.x1 {
                    let p = t.grid_to_world(ii, jj);
                    let edge = fbm(p.x / (radius * 0.6), p.y / (radius * 0.6), 3, 2.0, 0.5, seed ^ 0x1a)
                        * 0.45;
                    let dn = (p.distance(c) / radius + edge).max(0.0);
                    if dn >= 1.0 {
                        continue;
                    }
                    let bowl = rim - 0.5 - depth * (1.0 - dn * dn);
                    let k = t.idx(ii, jj);
                    t.heights[k] = t.heights[k].min(bowl);
                }
            }
        }
        sites.push(LakeSite {
            center: c,
            radius,
            rim,
            tier: s.tier,
        });
    }
    sites
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plateaus_canyons_and_tiers() {
        let mut t = Terrain::new(256, 8.0, 0.0);
        let cp = CanyonParams::alien();
        let rep = generate_canyonlands(&mut t, &cp, 5);
        let floor = rep.floor;
        let relief = cp.relief;
        let n = t.heights.len() as f32;
        let low = t.heights.iter().filter(|h| **h < floor + relief * 0.15).count() as f32 / n;
        let high = t.heights.iter().filter(|h| **h > floor + relief * 0.7).count() as f32 / n;
        assert!(low > 0.05, "canyon floors: {low}");
        assert!(high > 0.15, "plateaus: {high}");
        assert_eq!(rep.tier_heights.len(), 4);
        assert!(t.heights.iter().all(|h| h.is_finite()));
        // tiers: floors at more than one level
        let mut levels = std::collections::BTreeSet::new();
        for j in (0..t.n()).step_by(3) {
            for i in (0..t.n()).step_by(3) {
                let s = canyon_sample(t.grid_to_world(i, j), &cp, 5);
                if s.main > 0.99 {
                    levels.insert(s.tier);
                }
            }
        }
        assert!(levels.len() >= 2, "floor tiers: {levels:?}");
    }

    #[test]
    fn region_blends_into_the_ground() {
        let mut t = Terrain::new(128, 4.0, 0.0);
        t.heights.iter_mut().for_each(|h| *h = 10.0);
        let rep =
            generate_canyonlands_region(&mut t, &CanyonParams::default(), 3, Vec2::ZERO, 150.0)
                .unwrap();
        assert!((rep.floor - 10.0).abs() < 1e-3);
        // outside the disc nothing moved
        assert_eq!(t.height_at(240.0, 240.0), 10.0);
        assert!(t.heights.iter().any(|h| *h > 60.0));
    }
}
