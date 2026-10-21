//! The ground under the roads: the terrain heights and paint before any road touched them, kept
//! per 64² tile of vertices (shared between undo snapshots, copied only when a tile changes).
//! Every edit rebuilds the affected region from this base: each road (in rank order) pulls the
//! ground towards its profile with smooth shoulders and paints its surface and worn verges.
//! Vertices no road covers any more get their original height and paint back. Sculpting or
//! painting on a road after it was built is kept: the difference to what the roads last wrote is
//! added to the base.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::plan::RoadPlan;
use crate::history::History;
use crate::noise::smoothstep as ss;
use crate::terrain::layers::LAYER_COUNT;
use crate::terrain::{GridRect, Terrain};

pub const TILE: usize = 64;

#[derive(Clone, Debug)]
pub struct GroundTile {
    /// Height before any road (NaN: no road covers this vertex).
    pub base_h: Vec<f32>,
    pub base_s: Vec<[u8; LAYER_COUNT]>,
    /// What the roads last wrote (NaN: unknown, i.e. the terrain as loaded).
    pub app_h: Vec<f32>,
    pub app_s: Vec<[u8; LAYER_COUNT]>,
}

impl GroundTile {
    fn new() -> Self {
        let n = TILE * TILE;
        GroundTile {
            base_h: vec![f32::NAN; n],
            base_s: vec![[0; LAYER_COUNT]; n],
            app_h: vec![f32::NAN; n],
            app_s: vec![[0; LAYER_COUNT]; n],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct GroundStore {
    /// Terrain vertices per side the tiles belong to (a different terrain discards them).
    pub n: usize,
    pub tiles: HashMap<(u32, u32), Arc<GroundTile>>,
    /// Bumped on every change (road plans are cached against it).
    pub revision: u64,
}

impl GroundStore {
    fn key(i: usize, j: usize) -> ((u32, u32), usize) {
        (
            ((i / TILE) as u32, (j / TILE) as u32),
            (j % TILE) * TILE + i % TILE,
        )
    }
    /// Base (pre-road) height of a vertex, if a road covers it.
    pub fn base(&self, i: usize, j: usize) -> Option<(f32, [u8; LAYER_COUNT])> {
        let (k, l) = Self::key(i, j);
        let t = self.tiles.get(&k)?;
        let h = t.base_h[l];
        if h.is_nan() {
            None
        } else {
            Some((h, t.base_s[l]))
        }
    }
    /// Drops everything if the terrain was replaced by one of another size.
    pub fn validate(&mut self, t: &Terrain) {
        if self.n != t.n() {
            if !self.tiles.is_empty() {
                warn!("road ground data is for another terrain size; dropped");
            }
            self.tiles.clear();
            self.n = t.n();
        }
    }
    pub fn covered_count(&self) -> usize {
        self.tiles
            .values()
            .map(|t| t.base_h.iter().filter(|h| !h.is_nan()).count())
            .sum()
    }
}

/// The terrain as it was before the roads (heights), for profiles and routing.
pub struct Base<'a> {
    pub t: &'a Terrain,
    pub g: &'a GroundStore,
}

impl Base<'_> {
    pub fn h(&self, i: usize, j: usize) -> f32 {
        if self.g.n == self.t.n()
            && let Some((h, _)) = self.g.base(i, j)
        {
            return h;
        }
        self.t.h(i, j)
    }
    /// Bilinear base height at a world position (clamped to the terrain).
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let t = self.t;
        let g = t.world_to_grid(x, z);
        let max = t.res as f32;
        let (gx, gz) = (g.x.clamp(0.0, max), g.y.clamp(0.0, max));
        let i0 = (gx.floor() as usize).min(t.res - 1);
        let j0 = (gz.floor() as usize).min(t.res - 1);
        let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
        let a = self.h(i0, j0) + (self.h(i0 + 1, j0) - self.h(i0, j0)) * fx;
        let b = self.h(i0, j0 + 1) + (self.h(i0 + 1, j0 + 1) - self.h(i0, j0 + 1)) * fx;
        a + (b - a) * fz
    }
}

/// Blends a splat vertex toward one layer (as the paint brush does).
pub(crate) fn paint_toward(s: &mut [u8; LAYER_COUNT], layer: usize, k: f32) {
    if k <= 0.003 {
        return;
    }
    for (l, v) in s.iter_mut().enumerate() {
        let target = if l == layer { 255.0 } else { 0.0 };
        let cur = *v as f32;
        *v = (cur + (target - cur) * k).round().clamp(0.0, 255.0) as u8;
    }
    let sum: u32 = s.iter().map(|v| *v as u32).sum();
    if sum > 255 {
        let scale = 255.0 / sum as f32;
        for v in s.iter_mut() {
            *v = (*v as f32 * scale).floor() as u8;
        }
    }
}

/// Distance and arc position of the nearest centreline point, for every vertex within the
/// plan's reach inside `rect`.
pub fn influence(plan: &RoadPlan, t: &Terrain, rect: GridRect) -> HashMap<usize, (f32, f32)> {
    let mut out: HashMap<usize, (f32, f32)> = HashMap::new();
    let r = plan.reach;
    let n = plan.samples.len();
    if n == 0 {
        return out;
    }
    let mut visit = |a: Vec2, b: Vec2, sa: f32, sb: f32| {
        let lo = t.world_to_grid(a.x.min(b.x) - r, a.y.min(b.y) - r);
        let hi = t.world_to_grid(a.x.max(b.x) + r, a.y.max(b.y) + r);
        let x0 = (lo.x.ceil().max(rect.x0 as f32)) as usize;
        let z0 = (lo.y.ceil().max(rect.z0 as f32)) as usize;
        let x1 = hi.x.floor().min(rect.x1 as f32);
        let z1 = hi.y.floor().min(rect.z1 as f32);
        if x1 < x0 as f32 || z1 < z0 as f32 {
            return;
        }
        let ab = b - a;
        let l2 = ab.length_squared().max(1e-9);
        for j in z0..=z1 as usize {
            for i in x0..=x1 as usize {
                let p = t.grid_to_world(i, j);
                let u = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
                let d = p.distance(a + ab * u);
                if d > r {
                    continue;
                }
                let k = t.idx(i, j);
                let s = sa + (sb - sa) * u;
                match out.get_mut(&k) {
                    Some(e) if e.0 <= d => {}
                    Some(e) => *e = (d, s),
                    None => {
                        out.insert(k, (d, s));
                    }
                }
            }
        }
    };
    if n == 1 {
        let p = plan.samples[0].p;
        visit(p, p, 0.0, 0.0);
    }
    for w in plan.samples.windows(2) {
        visit(w[0].p, w[1].p, w[0].s, w[1].s);
    }
    out
}

/// Rebuilds heights and paint inside `rect` from the base ground and the given plans (every road
/// whose reach touches the rect, sorted by rank, lowest first). Records undo for the tiles it
/// changes. Returns the rect that actually changed.
pub fn rebuild(
    t: &mut Terrain,
    hist: &mut History,
    store: &mut GroundStore,
    plans: &[&RoadPlan],
    rect: GridRect,
) -> Option<GridRect> {
    // Authored sheet routes are suspended decks. They must never claim terrain vertices during a
    // road rebuild: otherwise removing or updating the deck could restore/paint land it never
    // modified. Ordinary roads remain fully eligible, including those that overlap a deck.
    let plans: Vec<&RoadPlan> = plans
        .iter()
        .copied()
        .filter(|plan| !plan.authored)
        .collect();
    store.validate(t);
    let n = t.n();
    // 1. keep edits made on top of the roads since they were last built
    for tz in rect.z0 / TILE..=rect.z1 / TILE {
        for tx in rect.x0 / TILE..=rect.x1 / TILE {
            let Some(tile) = store.tiles.get_mut(&(tx as u32, tz as u32)) else {
                continue;
            };
            let tile = Arc::make_mut(tile);
            for l in 0..TILE * TILE {
                if tile.base_h[l].is_nan() {
                    continue;
                }
                let (i, j) = (tx * TILE + l % TILE, tz * TILE + l / TILE);
                if i >= n || j >= n {
                    continue;
                }
                let k = t.idx(i, j);
                let (cur_h, cur_s) = (t.heights[k], t.splat[k]);
                if !tile.app_h[l].is_nan() {
                    let dh = cur_h - tile.app_h[l];
                    if dh.abs() > 0.01 {
                        tile.base_h[l] += dh;
                    }
                    if cur_s != tile.app_s[l] {
                        tile.base_s[l] = cur_s;
                    }
                }
            }
        }
    }

    // 2. what every road wants for each vertex it reaches
    let near: Vec<HashMap<usize, (f32, f32)>> =
        plans.iter().map(|p| influence(p, t, rect)).collect();
    let mut touched: Vec<usize> = near.iter().flat_map(|m| m.keys().copied()).collect();
    touched.sort_unstable();
    touched.dedup();
    let mut writes: Vec<(usize, f32, [u8; LAYER_COUNT], bool)> = Vec::with_capacity(touched.len());
    for &k in &touched {
        let (i, j) = (k % n, k / n);
        let (base_h, base_s) = store.base(i, j).unwrap_or((t.heights[k], t.splat[k]));
        let p = t.grid_to_world(i, j);
        let mut h = base_h;
        let mut s = base_s;
        let mut any = false;
        for (pi, plan) in plans.iter().enumerate() {
            let Some(&(d, sa)) = near[pi].get(&k) else {
                continue;
            };
            let st = plan.kind.style();
            let bed = plan.bed_at(sa);
            // ragged band edges
            let nz = crate::noise::fbm(
                p.x / 6.5,
                p.y / 6.5,
                3,
                2.0,
                0.5,
                plan.id.wrapping_mul(97) + 5,
            );
            if plan.options.bed && bed > 0.0 {
                let target = plan.height_at(sa);
                let dh = target - h;
                let core = plan.hw + st.edge * 0.5 + 0.35 * t.cell;
                // embankments / cuttings about 1:1.6 (fill) and 1:1.25 (cut), at least a cell wide
                let bank = (dh.abs() * if dh > 0.0 { 1.6 } else { 1.25 })
                    .clamp((t.cell * 1.25).max(2.5), (plan.reach - core).max(2.5));
                let w = (1.0 - ss(core, core + bank, d)) * bed;
                if w > 0.0 {
                    h += dh * w;
                    any = true;
                }
            }
            if bed > 0.0 {
                // paint: a worn verge, then the layer under the surface a little past the edge,
                // so grass (whose density map has the terrain's resolution) doesn't grow through
                let rc = (plan.hw + st.under_reach).max(0.35 * t.cell) + nz * 0.3;
                let rs = rc + st.shoulder_w * (1.0 + nz * 0.8);
                let verge = (1.0 - ss(rs - 1.2, rs + 1.0, d)) * st.shoulder_paint * bed;
                let under = (1.0 - ss(rc - 0.5, rc + 0.5, d)) * st.under_paint * bed;
                if verge > 0.003 || under > 0.003 {
                    paint_toward(&mut s, st.shoulder as usize, verge);
                    paint_toward(&mut s, st.under as usize, under);
                    any = true;
                }
            }
        }
        writes.push((k, h, s, any));
    }

    // 3. vertices no road covers any more get their ground back
    let mut freed: Vec<(usize, f32, [u8; LAYER_COUNT])> = vec![];
    for tz in rect.z0 / TILE..=rect.z1 / TILE {
        for tx in rect.x0 / TILE..=rect.x1 / TILE {
            let Some(tile) = store.tiles.get(&(tx as u32, tz as u32)) else {
                continue;
            };
            for l in 0..TILE * TILE {
                if tile.base_h[l].is_nan() {
                    continue;
                }
                let (i, j) = (tx * TILE + l % TILE, tz * TILE + l / TILE);
                if i < rect.x0 || i > rect.x1 || j < rect.z0 || j > rect.z1 || i >= n || j >= n {
                    continue;
                }
                let k = t.idx(i, j);
                let covered = touched.binary_search(&k).is_ok_and(|w| writes[w].3);
                if !covered {
                    freed.push((k, tile.base_h[l], tile.base_s[l]));
                }
            }
        }
    }

    // 4. write: snapshot the changed tiles for undo, then apply
    let mut changed: Option<GridRect> = None;
    let mut snap = std::collections::HashSet::new();
    let mut apply =
        |t: &mut Terrain, hist: &mut History, k: usize, h: f32, s: [u8; LAYER_COUNT]| {
            if (t.heights[k] - h).abs() < 1e-4 && t.splat[k] == s {
                return;
            }
            let (i, j) = (k % n, k / n);
            if snap.insert((i / crate::history::TILE, j / crate::history::TILE)) {
                let (x0, z0) = (
                    (i / crate::history::TILE) * crate::history::TILE,
                    (j / crate::history::TILE) * crate::history::TILE,
                );
                hist.touch_terrain(
                    t,
                    GridRect {
                        x0,
                        z0,
                        x1: x0,
                        z1: z0,
                    },
                );
            }
            t.heights[k] = h;
            t.splat[k] = s;
            let r = GridRect {
                x0: i,
                z0: j,
                x1: i,
                z1: j,
            };
            changed = Some(changed.map_or(r, |c| c.union(r)));
        };
    for &(k, h, s, any) in &writes {
        if !any {
            continue;
        }
        let (key, l) = GroundStore::key(k % n, k / n);
        let tile = Arc::make_mut(
            store
                .tiles
                .entry(key)
                .or_insert_with(|| Arc::new(GroundTile::new())),
        );
        if tile.base_h[l].is_nan() {
            tile.base_h[l] = t.heights[k];
            tile.base_s[l] = t.splat[k];
        }
        tile.app_h[l] = h;
        tile.app_s[l] = s;
        apply(t, hist, k, h, s);
    }
    for &(k, h, s) in &freed {
        apply(t, hist, k, h, s);
        let (key, l) = GroundStore::key(k % n, k / n);
        if let Some(tile) = store.tiles.get_mut(&key) {
            let tile = Arc::make_mut(tile);
            tile.base_h[l] = f32::NAN;
            tile.app_h[l] = f32::NAN;
        }
    }
    store
        .tiles
        .retain(|_, tile| tile.base_h.iter().any(|h| !h.is_nan()));
    store.revision += 1;
    if let Some(r) = changed {
        t.mark_heights_dirty(r);
        t.mark_splat_dirty(r);
    }
    changed
}

// ---------------------------------------------------------------- saving

/// Saved as one base64 blob per tile: (u16 vertex, i32 height mm, 12 paint weights) per vertex.
#[derive(Serialize, Deserialize)]
struct StoreDoc {
    n: usize,
    tiles: Vec<TileDoc>,
}

#[derive(Serialize, Deserialize)]
struct TileDoc {
    x: u32,
    z: u32,
    data: String,
}

impl Serialize for GroundStore {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let mut keys: Vec<_> = self.tiles.keys().copied().collect();
        keys.sort();
        let tiles = keys
            .into_iter()
            .map(|(x, z)| {
                let t = &self.tiles[&(x, z)];
                let mut bytes = vec![];
                for l in 0..TILE * TILE {
                    if t.base_h[l].is_nan() {
                        continue;
                    }
                    bytes.extend_from_slice(&(l as u16).to_le_bytes());
                    bytes.extend_from_slice(&((t.base_h[l] * 1000.0).round() as i32).to_le_bytes());
                    bytes.extend_from_slice(&t.base_s[l]);
                }
                TileDoc {
                    x,
                    z,
                    data: base64_encode(&bytes),
                }
            })
            .collect();
        StoreDoc { n: self.n, tiles }.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for GroundStore {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let doc = StoreDoc::deserialize(de)?;
        let mut tiles = HashMap::new();
        const REC: usize = 2 + 4 + LAYER_COUNT;
        for td in doc.tiles {
            let Some(bytes) = base64_decode(&td.data) else {
                continue;
            };
            let mut t = GroundTile::new();
            for r in bytes.chunks_exact(REC) {
                let l = u16::from_le_bytes([r[0], r[1]]) as usize;
                if l >= TILE * TILE {
                    continue;
                }
                t.base_h[l] = i32::from_le_bytes([r[2], r[3], r[4], r[5]]) as f32 * 0.001;
                t.base_s[l].copy_from_slice(&r[6..6 + LAYER_COUNT]);
            }
            tiles.insert((td.x, td.z), Arc::new(t));
        }
        Ok(GroundStore {
            n: doc.n,
            tiles,
            revision: 0,
        })
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let v = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[(v >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub(crate) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let vals: Vec<u8> = s
        .bytes()
        .filter(|c| *c != b'=')
        .map(|c| B64.iter().position(|x| *x == c).map(|p| p as u8))
        .collect::<Option<_>>()?;
    for c in vals.chunks(4) {
        let mut v = 0u32;
        for (k, x) in c.iter().enumerate() {
            v |= (*x as u32) << (18 - 6 * k);
        }
        for k in 0..c.len().saturating_sub(1) {
            out.push((v >> (16 - 8 * k)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for n in 0..40 {
            let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&b)).unwrap(), b);
        }
    }
}
