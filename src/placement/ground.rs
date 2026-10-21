//! The ground under fitted objects: the terrain heights and paint before any foundation touched
//! them, kept per 64² tile of vertices (shared between undo snapshots, copied only when a tile
//! changes) — the bookkeeping roads use for their bed (`roads::ground`). Every fit rebuilds the
//! affected region from this base: each pad levels every terrain cell its footprint touches to
//! the object's floor, blends a bank back into the slope (cut into the hill uphill, a fill
//! downhill) and paints a packed-earth or gravel apron. Vertices no pad covers any more get their
//! original height and paint back; sculpting or painting on a pad after it was fitted is kept
//! (the difference to what the pads last wrote is added to the base).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::footprint::Footprint;
use crate::history::History;
use crate::noise::smoothstep as ss;
use crate::objects::Placed;
use crate::roads::ground::paint_toward;
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::terrain::{GridRect, Terrain};

pub const TILE: usize = 64;
/// Widest bank (m) a pad blends back into the natural slope over.
pub const MAX_BANK: f32 = 14.0;
/// Ground around the footprint (m) levelled with it.
const MARGIN: f32 = 0.3;

#[derive(Clone, Debug)]
struct Tile {
    /// Height before any pad (NaN: no pad covers this vertex).
    base_h: Vec<f32>,
    base_s: Vec<[u8; LAYER_COUNT]>,
    /// What the pads last wrote (NaN: unknown).
    app_h: Vec<f32>,
    app_s: Vec<[u8; LAYER_COUNT]>,
}

impl Tile {
    fn new() -> Self {
        let n = TILE * TILE;
        Tile {
            base_h: vec![f32::NAN; n],
            base_s: vec![[0; LAYER_COUNT]; n],
            app_h: vec![f32::NAN; n],
            app_s: vec![[0; LAYER_COUNT]; n],
        }
    }
}

/// One fitted object: the footprint it was fitted with and where it stood then.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    pub asset: String,
    /// footprint: centre x, z, the model's x axis (x, z), half sizes along its x and z
    pub fp: [f32; 6],
    /// height of the levelled ground (m)
    pub level: f32,
    /// the object's position (x, z), rotation and size when it was fitted
    pub at: [f32; 2],
    pub rot: [f32; 4],
    pub scale: [f32; 3],
    /// stone-built (forts, stone halls): a gravel apron instead of packed earth
    #[serde(default)]
    pub stone: bool,
}

impl Pad {
    pub fn new(p: &Placed, fp: Footprint, level: f32, stone: bool) -> Pad {
        Pad {
            asset: p.asset.clone(),
            fp: [
                fp.center.x,
                fp.center.y,
                fp.ax.x,
                fp.ax.y,
                fp.half.x,
                fp.half.y,
            ],
            level,
            at: [p.pos[0], p.pos[2]],
            rot: p.rot,
            scale: p.scale3().to_array(),
            stone,
        }
    }

    pub fn footprint(&self) -> Footprint {
        Footprint {
            center: Vec2::new(self.fp[0], self.fp[1]),
            ax: Vec2::new(self.fp[2], self.fp[3]),
            half: Vec2::new(self.fp[4], self.fp[5]),
        }
    }

    /// Whether the object moved, turned, was resized or swapped since it was fitted (lifting or
    /// lowering it doesn't count: that's a deliberate height).
    pub fn stale(&self, p: &Placed) -> bool {
        p.asset != self.asset
            || (p.pos[0] - self.at[0]).abs() > 0.005
            || (p.pos[2] - self.at[1]).abs() > 0.005
            || p.rotation().dot(Quat::from_array(self.rot)).abs() < 0.999_999
            || (p.scale3() - Vec3::from_array(self.scale))
                .abs()
                .max_element()
                > 1e-4
    }

    /// The grid region the pad can change (its banks included).
    pub fn region(&self, t: &Terrain) -> Option<GridRect> {
        let (lo, hi) = self.footprint().aabb(t.cell * 1.5 + MAX_BANK);
        grid_rect(t, lo, hi)
    }
}

/// Fitted objects and the ground under them (saved with the world).
#[derive(Resource, Clone, Debug, Default)]
pub struct Foundations {
    /// Terrain vertices per side the ground tiles belong to (a different terrain drops them).
    pub n: usize,
    tiles: HashMap<(u32, u32), Arc<Tile>>,
    /// Fitted objects by id.
    pub pads: BTreeMap<u64, Pad>,
    /// Per object: `true` levels the ground under it even when it wouldn't by default, `false`
    /// never does.
    pub overrides: BTreeMap<u64, bool>,
    /// An override changed outside a fit: check the objects again (not saved).
    pub dirty: bool,
    /// The caves' bounds (`caves::Caves`, kept by `fit::auto_fit`; not saved): what stands well
    /// under the ground in one stands on its floor and is never fitted.
    pub caves: Vec<(Vec3, Vec3)>,
}

pub fn grid_rect(t: &Terrain, lo: Vec2, hi: Vec2) -> Option<GridRect> {
    let g0 = t.world_to_grid(lo.x, lo.y);
    let g1 = t.world_to_grid(hi.x, hi.y);
    let max = t.res as f32;
    if g1.x < 0.0 || g1.y < 0.0 || g0.x > max || g0.y > max {
        return None;
    }
    Some(GridRect {
        x0: g0.x.floor().clamp(0.0, max) as usize,
        z0: g0.y.floor().clamp(0.0, max) as usize,
        x1: g1.x.ceil().clamp(0.0, max) as usize,
        z1: g1.y.ceil().clamp(0.0, max) as usize,
    })
}

impl Foundations {
    /// `p` is in one of the caves' bounds.
    pub fn in_caves(&self, p: Vec3) -> bool {
        self.caves
            .iter()
            .any(|(lo, hi)| p.cmpge(*lo).all() && p.cmple(*hi).all())
    }

    fn key(i: usize, j: usize) -> ((u32, u32), usize) {
        (
            ((i / TILE) as u32, (j / TILE) as u32),
            (j % TILE) * TILE + i % TILE,
        )
    }

    /// Height before any pad, where a pad covers the vertex.
    pub fn base(&self, i: usize, j: usize) -> Option<f32> {
        let (k, l) = Self::key(i, j);
        let h = self.tiles.get(&k)?.base_h[l];
        (!h.is_nan()).then_some(h)
    }

    /// The natural ground (before any pad) at a world position, bilinear like `Terrain::height_at`.
    pub fn base_at(&self, t: &Terrain, x: f32, z: f32) -> f32 {
        let h = |i: usize, j: usize| {
            if self.n == t.n() {
                self.base(i, j).unwrap_or_else(|| t.h(i, j))
            } else {
                t.h(i, j)
            }
        };
        let g = t.world_to_grid(x, z);
        let max = t.res as f32;
        let (gx, gz) = (g.x.clamp(0.0, max), g.y.clamp(0.0, max));
        let i0 = (gx.floor() as usize).min(t.res - 1);
        let j0 = (gz.floor() as usize).min(t.res - 1);
        let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
        let a = h(i0, j0) + (h(i0 + 1, j0) - h(i0, j0)) * fx;
        let b = h(i0, j0 + 1) + (h(i0 + 1, j0 + 1) - h(i0, j0 + 1)) * fx;
        a + (b - a) * fz
    }

    /// Drops the ground and pads if the terrain was replaced by one of another size.
    pub fn validate(&mut self, t: &Terrain) {
        if self.n != t.n() {
            self.tiles.clear();
            self.pads.clear();
            self.n = t.n();
        }
    }

    /// Forgets the remembered ground and every pad (the land was regenerated under them: the
    /// objects are fitted again to the new land).
    pub fn forget(&mut self) {
        self.tiles.clear();
        self.pads.clear();
    }

    #[cfg(test)]
    pub fn covered_count(&self) -> usize {
        self.tiles
            .values()
            .map(|t| t.base_h.iter().filter(|h| !h.is_nan()).count())
            .sum()
    }

    /// Rebuilds heights and paint inside `rect` from the remembered ground and every pad reaching
    /// into it (cores win over banks; among overlapping pads the newest object wins). Records
    /// undo for the tiles it changes. Returns the rect that actually changed.
    pub fn rebuild(
        &mut self,
        t: &mut Terrain,
        hist: &mut History,
        rect: GridRect,
    ) -> Option<GridRect> {
        self.validate(t);
        let n = t.n();
        // 1. keep edits made on the pads since they were last fitted
        for tz in rect.z0 / TILE..=rect.z1 / TILE {
            for tx in rect.x0 / TILE..=rect.x1 / TILE {
                let Some(tile) = self.tiles.get_mut(&(tx as u32, tz as u32)) else {
                    continue;
                };
                let tile = Arc::make_mut(tile);
                for l in 0..TILE * TILE {
                    if tile.base_h[l].is_nan() || tile.app_h[l].is_nan() {
                        continue;
                    }
                    let (i, j) = (tx * TILE + l % TILE, tz * TILE + l / TILE);
                    if i >= n || j >= n {
                        continue;
                    }
                    let k = t.idx(i, j);
                    let dh = t.heights[k] - tile.app_h[l];
                    if dh.abs() > 0.01 {
                        tile.base_h[l] += dh;
                    }
                    if t.splat[k] != tile.app_s[l] {
                        tile.base_s[l] = t.splat[k];
                    }
                }
            }
        }

        // 2. what the pads want for every vertex in the rect
        let (wlo, whi) = (
            t.grid_to_world(rect.x0, rect.z0),
            t.grid_to_world(rect.x1, rect.z1),
        );
        let reach = t.cell * 1.5 + MAX_BANK;
        let pads: Vec<(Footprint, &Pad)> = self
            .pads
            .values()
            .map(|p| (p.footprint(), p))
            .filter(|(f, _)| {
                let (lo, hi) = f.aabb(reach);
                lo.x <= whi.x && lo.y <= whi.y && hi.x >= wlo.x && hi.y >= wlo.y
            })
            .collect();
        let mut writes: Vec<(usize, f32, [u8; LAYER_COUNT], bool)> =
            Vec::with_capacity(rect.width() * rect.height());
        for j in rect.z0..=rect.z1 {
            for i in rect.x0..=rect.x1 {
                let k = t.idx(i, j);
                let (k0, l) = Self::key(i, j);
                let base = self
                    .tiles
                    .get(&k0)
                    .filter(|tile| !tile.base_h[l].is_nan())
                    .map(|tile| (tile.base_h[l], tile.base_s[l]));
                let (bh, bs) = base.unwrap_or((t.heights[k], t.splat[k]));
                let (h, s, any) = shape(t, &pads, i, j, bh, bs);
                writes.push((k, h, s, any));
            }
        }

        // 3. write: snapshot the changed tiles for undo, remember the base of newly covered
        // vertices, give uncovered ones their ground back
        let mut changed: Option<GridRect> = None;
        let mut snap = std::collections::HashSet::new();
        for (k, h, s, any) in writes {
            let (i, j) = (k % n, k / n);
            let (key, l) = Self::key(i, j);
            let (h, s) = if any {
                let tile = Arc::make_mut(
                    self.tiles
                        .entry(key)
                        .or_insert_with(|| Arc::new(Tile::new())),
                );
                if tile.base_h[l].is_nan() {
                    tile.base_h[l] = t.heights[k];
                    tile.base_s[l] = t.splat[k];
                }
                tile.app_h[l] = h;
                tile.app_s[l] = s;
                (h, s)
            } else {
                let Some(tile) = self.tiles.get_mut(&key) else {
                    continue;
                };
                if tile.base_h[l].is_nan() {
                    continue;
                }
                let tile = Arc::make_mut(tile);
                let back = (tile.base_h[l], tile.base_s[l]);
                tile.base_h[l] = f32::NAN;
                tile.app_h[l] = f32::NAN;
                back
            };
            if (t.heights[k] - h).abs() < 1e-4 && t.splat[k] == s {
                continue;
            }
            let (ti, tj) = (i / crate::history::TILE, j / crate::history::TILE);
            if snap.insert((ti, tj)) {
                let (x0, z0) = (ti * crate::history::TILE, tj * crate::history::TILE);
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
        }
        self.tiles
            .retain(|_, tile| tile.base_h.iter().any(|h| !h.is_nan()));
        if let Some(r) = changed {
            t.mark_heights_dirty(r);
            t.mark_splat_dirty(r);
        }
        changed
    }
}

/// The height and paint the pads give vertex (i, j) whose natural ground is `bh` / `bs`, and
/// whether any pad reaches it.
fn shape(
    t: &Terrain,
    pads: &[(Footprint, &Pad)],
    i: usize,
    j: usize,
    bh: f32,
    bs: [u8; LAYER_COUNT],
) -> (f32, [u8; LAYER_COUNT], bool) {
    let c = t.cell;
    let p = t.grid_to_world(i, j);
    let mut h = bh;
    let mut any = false;
    // a vertex of any terrain cell the footprint touches is levelled: the ground under the
    // object is then flat whatever the grid
    let core = |f: &Footprint| f.overlaps_rect(p - Vec2::splat(c), p + Vec2::splat(c), MARGIN);
    // banks: from the levelled cells back into the slope, wider where the cut or fill is higher
    for (f, pad) in pads {
        if core(f) {
            continue;
        }
        let d = f.sdist(p);
        let dh = pad.level - h;
        let bank = (dh.abs() * if dh > 0.0 { 1.7 } else { 1.3 }).clamp(c * 1.25, MAX_BANK);
        let w = 1.0 - ss(c * 0.5, c * 0.5 + bank, d);
        if w > 0.002 {
            h += dh * w;
            any = true;
        }
    }
    let mut apron: Option<(f32, Layer)> = None;
    for (f, pad) in pads {
        let layer = if pad.stone {
            Layer::Gravel
        } else {
            Layer::Dirt
        };
        if core(f) {
            h = pad.level;
            any = true;
        }
        // a ragged band of trodden earth along the walls, fading out within a few metres
        let nz = crate::noise::fbm(p.x / 5.0, p.y / 5.0, 3, 2.0, 0.5, 4517);
        let a = 0.7 * (1.0 - ss(-0.5, (2.2 + nz * 1.4).max(0.6) + c * 0.25, f.sdist(p)));
        if apron.is_none_or(|(b, _)| a > b) {
            apron = Some((a, layer));
        }
    }
    if !any {
        return (bh, bs, false);
    }
    let mut s = bs;
    if let Some((a, layer)) = apron {
        paint_toward(&mut s, layer as usize, a);
        // the faces of deep cuts are bare rock (fills grass over)
        paint_toward(
            &mut s,
            Layer::Rock as usize,
            0.85 * ss(1.6, 3.5, bh - h) * (1.0 - a),
        );
    }
    (h, s, true)
}

// ---------------------------------------------------------------- saving

/// Saved as one base64 blob per tile: (u16 vertex, i32 base mm, base paint, i32 written mm,
/// written paint) per covered vertex.
#[derive(Serialize, Deserialize)]
struct Doc {
    n: usize,
    tiles: Vec<TileDoc>,
    pads: Vec<(u64, Pad)>,
    #[serde(default)]
    overrides: Vec<(u64, bool)>,
}

#[derive(Serialize, Deserialize)]
struct TileDoc {
    x: u32,
    z: u32,
    data: String,
}

const REC: usize = 2 + 4 + LAYER_COUNT + 4 + LAYER_COUNT;

impl Serialize for Foundations {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        let mut keys: Vec<_> = self.tiles.keys().copied().collect();
        keys.sort();
        let mm = |v: f32| {
            if v.is_nan() {
                i32::MIN
            } else {
                (v * 1000.0).round() as i32
            }
        };
        let tiles = keys
            .into_iter()
            .map(|(x, z)| {
                let t = &self.tiles[&(x, z)];
                let mut bytes = Vec::with_capacity(REC * 64);
                for l in 0..TILE * TILE {
                    if t.base_h[l].is_nan() {
                        continue;
                    }
                    bytes.extend_from_slice(&(l as u16).to_le_bytes());
                    bytes.extend_from_slice(&mm(t.base_h[l]).to_le_bytes());
                    bytes.extend_from_slice(&t.base_s[l]);
                    bytes.extend_from_slice(&mm(t.app_h[l]).to_le_bytes());
                    bytes.extend_from_slice(&t.app_s[l]);
                }
                TileDoc {
                    x,
                    z,
                    data: crate::roads::ground::base64_encode(&bytes),
                }
            })
            .collect();
        Doc {
            n: self.n,
            tiles,
            pads: self.pads.iter().map(|(k, v)| (*k, v.clone())).collect(),
            overrides: self.overrides.iter().map(|(k, v)| (*k, *v)).collect(),
        }
        .serialize(ser)
    }
}

impl<'de> Deserialize<'de> for Foundations {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let doc = Doc::deserialize(de)?;
        let mut tiles = HashMap::new();
        let h = |b: &[u8]| {
            let v = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            if v == i32::MIN {
                f32::NAN
            } else {
                v as f32 * 0.001
            }
        };
        for td in doc.tiles {
            let Some(bytes) = crate::roads::ground::base64_decode(&td.data) else {
                continue;
            };
            let mut t = Tile::new();
            for r in bytes.chunks_exact(REC) {
                let l = u16::from_le_bytes([r[0], r[1]]) as usize;
                if l >= TILE * TILE {
                    continue;
                }
                let mut o = 2;
                t.base_h[l] = h(&r[o..]);
                o += 4;
                t.base_s[l].copy_from_slice(&r[o..o + LAYER_COUNT]);
                o += LAYER_COUNT;
                t.app_h[l] = h(&r[o..]);
                o += 4;
                t.app_s[l].copy_from_slice(&r[o..o + LAYER_COUNT]);
            }
            tiles.insert((td.x, td.z), Arc::new(t));
        }
        Ok(Foundations {
            n: doc.n,
            tiles,
            pads: doc.pads.into_iter().collect(),
            overrides: doc.overrides.into_iter().collect(),
            dirty: false,
            caves: vec![],
        })
    }
}
