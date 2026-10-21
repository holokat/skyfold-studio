//! The ground water carved: rivers remember the heights their bed replaced (as drawn lakes keep
//! a `lakes::GroundPatch`), so removing or re-routing a river, or removing a drawn lake, gives
//! the land back. Edits made on top of the carve since (sculpting along the banks…) are kept:
//! the restored height is the remembered ground plus whatever changed after the carve.
//! Everything goes through `History`, so undo puts the bed back.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::{River, WaterBodies, WaterSel, lakes};
use crate::history::History;
use crate::objects::ObjectStore;
use crate::terrain::{GridRect, Terrain};

/// A carve that differs from the recomputed one by less than this kept nothing of later edits.
const KEEP_EDITS: f32 = 0.02;

/// The ground under a river before its bed was carved: only the vertices the carve changed
/// (millimetres), on a grid of `n` × `n` vertices.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RiverGround {
    pub n: usize,
    pub idx: Vec<u32>,
    pub mm: Vec<i32>,
}

impl RiverGround {
    pub fn record(n: usize, changed: &[(usize, f32)]) -> Self {
        Self {
            n,
            idx: changed.iter().map(|c| c.0 as u32).collect(),
            mm: changed
                .iter()
                .map(|c| (c.1 * 1000.0).round() as i32)
                .collect(),
        }
    }
}

/// What carving a river's channel makes of the ground height `cur` at `p` (`None`: unchanged).
/// `pts` is the centre line and `surf` the water surface along it.
pub fn channel_height(
    p: Vec2,
    cur: f32,
    pts: &[Vec2],
    surf: &[f32],
    hw: f32,
    depth: f32,
) -> Option<f32> {
    // nearest segment
    let mut best = (f32::MAX, 0.0f32);
    for k in 0..pts.len().saturating_sub(1) {
        let (a, b) = (pts[k], pts[k + 1]);
        let ab = b - a;
        let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        let d = p.distance(a + ab * u);
        if d < best.0 {
            best = (d, surf[k] + (surf[k + 1] - surf[k]) * u);
        }
    }
    let (d, s) = best;
    if d < hw {
        let x = d / hw;
        let bed = s - depth * (1.0 - x * x);
        (cur > bed).then_some(bed)
    } else if d < hw * 2.2 {
        // banks: blend from just above the water line back to the natural ground
        let x = (d - hw) / (hw * 1.2);
        let bank = s + 0.35 + x * x * 4.0;
        let x2 = crate::noise::smoothstep(0.0, 1.0, x);
        let target = bank * (1.0 - x2) + cur * x2;
        if cur > bank && x < 1.0 {
            Some(cur + (bank.max(target) - cur) * (1.0 - x2) * 0.8)
        } else if cur < s + 0.3 {
            // low ground next to the river: a natural levee keeps the water in
            Some(cur + (s + 0.3 - cur) * (1.0 - x2))
        } else {
            None
        }
    } else {
        None
    }
}

/// Puts back the ground under a river's bed. False if the river has no remembered ground
/// (drawn before rivers kept it) or the grid changed since.
pub fn restore_river(t: &mut Terrain, h: &mut History, r: &River) -> bool {
    let Some(g) = &r.ground else { return false };
    let n = t.n();
    if g.n != n || g.idx.len() != g.mm.len() || g.idx.is_empty() {
        return false;
    }
    let pts: Vec<Vec2> = r.points.iter().map(|p| Vec2::new(p[0], p[2])).collect();
    let surf: Vec<f32> = r.points.iter().map(|p| p[1]).collect();
    let (mut lo, mut hi) = ((usize::MAX, usize::MAX), (0, 0));
    for &k in &g.idx {
        let (i, j) = (k as usize % n, k as usize / n);
        lo = (lo.0.min(i), lo.1.min(j));
        hi = (hi.0.max(i), hi.1.max(j));
    }
    if hi.1 >= n {
        return false;
    }
    let rect = GridRect {
        x0: lo.0,
        z0: lo.1,
        x1: hi.0,
        z1: hi.1,
    };
    h.touch_terrain(t, rect);
    for (&k, &mm) in g.idx.iter().zip(&g.mm) {
        let k = k as usize;
        let base = mm as f32 * 0.001;
        let p = t.grid_to_world(k % n, k / n);
        let carved = channel_height(p, base, &pts, &surf, r.width * 0.5, r.depth).unwrap_or(base);
        let later = t.heights[k] - carved;
        t.heights[k] = if later.abs() > KEEP_EDITS {
            base + later
        } else {
            base
        };
    }
    t.mark_heights_dirty(rect);
    true
}

/// What removing bodies of water did.
#[derive(Default)]
pub struct Removed {
    pub done: Vec<u32>,
    pub missing: Vec<u32>,
    /// bodies whose carved ground came back
    pub restored: usize,
    /// rivers drawn before rivers remembered their ground (their bed stays)
    pub kept_beds: usize,
}

/// Removes rivers, lakes, waterfalls and fish schools by id, giving back the ground rivers and
/// drawn lakes carved (and removing a drawn lake's shore plants). Call inside an undo step.
pub fn remove_bodies(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    water: &mut WaterBodies,
    ids: &[u32],
) -> Removed {
    let mut out = Removed::default();
    h.touch_water(water);
    for &id in ids {
        if let Some(i) = water.rivers.iter().position(|r| r.id == id) {
            let r = water.rivers.remove(i);
            if restore_river(t, h, &r) {
                out.restored += 1;
            } else {
                out.kept_beds += 1;
            }
        } else if let Some(i) = water.lakes.iter().position(|l| l.id == id) {
            let l = water.lakes.remove(i);
            if lakes::uncarve(t, h, store, &l) {
                out.restored += 1;
            }
        } else if water.waterfalls.iter().any(|f| f.id == id) {
            water.waterfalls.retain(|f| f.id != id);
        } else if water.fish.iter().any(|f| f.id == id) {
            water.fish.retain(|f| f.id != id);
        } else {
            out.missing.push(id);
            continue;
        }
        out.done.push(id);
    }
    if water.selected.is_some_and(|s| matches!(s, WaterSel::River(i) | WaterSel::Lake(i) | WaterSel::Waterfall(i) | WaterSel::Fish(i) if out.done.contains(&i))) {
        water.selected = None;
    }
    water.dirty = true;
    out
}

/// What stops a basin lake from holding water at some level.
pub struct Spill {
    /// The lowest point of the basin's rim (where water would run out).
    pub rim: Vec2,
    pub rim_height: f32,
    /// Where it would run: "the sea", "off the edge of the world", "a basin too large to fill".
    pub towards: &'static str,
}

/// Where water rising from `seed` first escapes its basin: a priority flood that always grows
/// over the lowest pass, so the highest point passed when it escapes is the rim.
pub fn spill_point(t: &Terrain, seed: Vec2, max_cells: usize) -> Option<Spill> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    #[derive(PartialEq, PartialOrd)]
    struct Level(f32);
    impl Eq for Level {}
    impl Ord for Level {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&o.0)
        }
    }
    let n = t.n();
    let g = t.world_to_grid(seed.x, seed.y);
    let (si, sj) = (g.x.round() as usize, g.y.round() as usize);
    if si >= n || sj >= n {
        return None;
    }
    let mut seen = vec![false; n * n];
    let mut heap = BinaryHeap::new();
    let s = sj * n + si;
    seen[s] = true;
    heap.push(Reverse((Level(t.heights[s]), s)));
    let (mut level, mut rim, mut count) = (f32::MIN, s, 0usize);
    while let Some(Reverse((Level(k), idx))) = heap.pop() {
        if k > level {
            (level, rim) = (k, idx);
        }
        let (i, j) = (idx % n, idx / n);
        let towards = if t.heights[idx] < t.sea_level - 0.5 {
            Some("the sea")
        } else if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
            Some("off the edge of the world")
        } else if count > max_cells {
            Some("a basin too large to fill")
        } else {
            None
        };
        if let Some(towards) = towards {
            return Some(Spill {
                rim: t.grid_to_world(rim % n, rim / n),
                rim_height: level,
                towards,
            });
        }
        count += 1;
        for (di, dj) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (ni, nj) = ((i as i32 + di) as usize, (j as i32 + dj) as usize);
            let nk = nj * n + ni;
            if !seen[nk] {
                seen[nk] = true;
                heap.push(Reverse((Level(k.max(t.heights[nk])), nk)));
            }
        }
    }
    None
}

/// `flood`, with a failure explained: where the water would spill over the rim, how far, and
/// the deepest fill that stays in the basin.
pub fn flood_explained(t: &Terrain, seed: Vec2, level: f32) -> Result<Vec<usize>, String> {
    let n = t.n();
    match super::flood(t, seed, level) {
        Ok(cells) => Ok(cells),
        Err(e) => {
            let Some(s) = spill_point(t, seed, (n * n / 6).max(20_000)) else {
                return Err(e.to_string());
            };
            let ground = t.height_at(seed.x, seed.y);
            let max = ((s.rim_height - ground) * 10.0).floor() / 10.0;
            let fill = level - ground;
            if s.towards == "a basin too large to fill" {
                return Err(format!(
                    "At {fill:.1} m the lake would grow too large (it spreads past [{:.0}, {:.0}] at {:.1} m). Max fill_depth here = {:.1} m.",
                    s.rim.x,
                    s.rim.y,
                    s.rim_height,
                    max.max(0.0)
                ));
            }
            if max < 0.5 {
                return Err(format!(
                    "That point isn't in a basin that holds water: it drains over [{:.0}, {:.0}] ({:.1} m) towards {} (ground here {ground:.1} m).",
                    s.rim.x, s.rim.y, s.rim_height, s.towards
                ));
            }
            Err(format!(
                "At {fill:.1} m the water (level {level:.1} m) spills over the rim at [{:.0}, {:.0}] ({:.1} m) by {:.1} m, towards {}. Max fill_depth here = {max:.1} m.",
                s.rim.x,
                s.rim.y,
                s.rim_height,
                level - s.rim_height,
                s.towards
            ))
        }
    }
}
