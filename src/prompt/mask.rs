//! The edit mask: while a selection masks edits, changes to the world stay inside it, fading out
//! over a feathered edge. It hooks in where every edit already passes, the undo history:
//!
//! - after each step is committed (an MCP tool call, a brush stroke, a button), terrain heights,
//!   ground paint and biomes outside blend back to the state the step recorded, by the mask
//!   weight; objects and water bodies outside are restored (objects in the feather thin out);
//!   painted nature density outside fades back by the mask weight too (the Nature brush itself
//!   fades out over the feather while it paints);
//! - while a brush stroke is open, the same happens every frame, so a brush never visibly reaches
//!   outside the selection.
//!
//! So the UI brushes, the MCP tools and the area prompt's agent all honour it without code of
//! their own. Zones are not masked (they are the user's gameplay regions).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use super::selection::{MaskField, Selection};
use crate::history::{History, TILE, TileSnap};
use crate::noise::value2;
use crate::objects::{ObjectStore, Placed};
use crate::terrain::layers::LAYER_COUNT;
use crate::terrain::{GridRect, Terrain};
use crate::water::WaterBodies;

#[derive(Resource, Default)]
pub struct MaskState {
    /// History serial up to which committed steps are settled.
    settled: u64,
    /// The open step's tiles as the last per-frame pass left them.
    live: HashMap<(usize, usize), TileRef>,
}

struct TileRef {
    heights: Vec<f32>,
    splat: Vec<[u8; LAYER_COUNT]>,
    biomes: Vec<u8>,
}

impl TileRef {
    fn of(s: &TileSnap) -> Self {
        Self {
            heights: s.heights.clone(),
            splat: s.splat.clone(),
            biomes: s.biomes.clone(),
        }
    }
}

/// Settles the steps committed since the last call (the MCP executor calls this after every
/// tool; a system does after the UI's commits).
pub fn settle_new_steps(world: &mut World) {
    let _ = world.run_system_cached(settle_committed);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn settle_committed(
    mut state: ResMut<MaskState>,
    mut sel: ResMut<Selection>,
    mut terrain: ResMut<Terrain>,
    mut history: ResMut<History>,
    mut store: ResMut<ObjectStore>,
    mut water: ResMut<WaterBodies>,
    mut nature: ResMut<crate::nature::NatureField>,
) {
    let serial = history.serial();
    if serial == state.settled {
        return;
    }
    let Some(field) = sel.mask(&terrain) else {
        state.settled = serial;
        state.live.clear();
        return;
    };
    for s in history.serials_after(state.settled) {
        let mut reverted = HashSet::new();
        if let Some((tiles, objects, water_before)) = history.step_before(s) {
            for snap in tiles {
                match state.live.get(&(snap.tx, snap.tz)) {
                    Some(r) => settle_tile(
                        &mut terrain,
                        field,
                        snap.tx,
                        snap.tz,
                        &r.heights,
                        &r.splat,
                        &r.biomes,
                    ),
                    None => settle_tile(
                        &mut terrain,
                        field,
                        snap.tx,
                        snap.tz,
                        &snap.heights,
                        &snap.splat,
                        &snap.biomes,
                    ),
                };
            }
            for (id, before) in objects {
                if settle_object(field, &mut store, id, before) {
                    reverted.insert(id);
                }
            }
            if let Some(wb) = water_before
                && settle_water(field, wb, &mut water)
            {
                water.dirty = true;
            }
        }
        if !reverted.is_empty() {
            history.refresh_objects(s, &store, &reverted);
        }
        // procedural nature (density paint): faded back outside by the mask weight (painted
        // ecosystems on the biomes' organic line)
        let key = crate::nature::brush::UNDO_KEY;
        if let Some((before, _)) = history.step_ext(s, key)
            && let Some(before) = before.downcast_ref::<crate::nature::field::NatureUndo>()
            && let Some(after) = nature.settle_outside(before, |p| field.at(p), keep_biome)
        {
            history.refresh_ext(s, key, std::sync::Arc::new(after));
        }
    }
    state.live.clear();
    state.settled = serial;
}

/// Masks the open step (a brush stroke) every frame.
pub(crate) fn settle_open_step(
    mut state: ResMut<MaskState>,
    mut sel: ResMut<Selection>,
    mut terrain: ResMut<Terrain>,
    history: Res<History>,
    mut store: ResMut<ObjectStore>,
) {
    let Some((tiles, objects)) = history.open_before() else {
        return;
    };
    let Some(field) = sel.mask(&terrain) else {
        return;
    };
    for snap in tiles {
        let r = state
            .live
            .entry((snap.tx, snap.tz))
            .or_insert_with(|| TileRef::of(snap));
        if settle_tile(
            &mut terrain,
            field,
            snap.tx,
            snap.tz,
            &r.heights,
            &r.splat,
            &r.biomes,
        ) {
            capture(&terrain, snap.tx, snap.tz, r);
        }
    }
    for (id, before) in objects {
        settle_object(field, &mut store, id, before);
    }
}

fn tile_rect(t: &Terrain, tx: usize, tz: usize) -> GridRect {
    let n = t.n();
    let (x0, z0) = (tx * TILE, tz * TILE);
    GridRect {
        x0,
        z0,
        x1: (x0 + TILE).min(n) - 1,
        z1: (z0 + TILE).min(n) - 1,
    }
}

/// The reference becomes the tile as it is now.
fn capture(t: &Terrain, tx: usize, tz: usize, r: &mut TileRef) {
    let rect = tile_rect(t, tx, tz);
    let w = rect.width();
    for j in rect.z0..=rect.z1 {
        let a = t.idx(rect.x0, j);
        let o = (j - rect.z0) * w;
        r.heights[o..o + w].copy_from_slice(&t.heights[a..a + w]);
        r.splat[o..o + w].copy_from_slice(&t.splat[a..a + w]);
        r.biomes[o..o + w].copy_from_slice(&t.biomes[a..a + w]);
    }
}

/// Blends one tile back towards `rh`/`rs`/`rb` (its reference state) by the mask weight.
/// Returns whether it changed anything.
#[allow(clippy::too_many_arguments)]
fn settle_tile(
    t: &mut Terrain,
    f: &MaskField,
    tx: usize,
    tz: usize,
    rh: &[f32],
    rs: &[[u8; LAYER_COUNT]],
    rb: &[u8],
) -> bool {
    let rect = tile_rect(t, tx, tz);
    let w = rect.width();
    let uniform = f.uniform_in(rect);
    if uniform == Some(true) {
        return false;
    }
    let (mut dh, mut ds, mut db) = (false, false, false);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let k = t.idx(i, j);
            let r = (j - rect.z0) * w + (i - rect.x0);
            let m = if uniform == Some(false) {
                0.0
            } else {
                f.at_grid(i, j)
            };
            if m >= 1.0 {
                continue;
            }
            let cur = t.heights[k];
            if cur != rh[r] {
                t.heights[k] = rh[r] + (cur - rh[r]) * m;
                dh = true;
            }
            if t.splat[k] != rs[r] {
                t.splat[k] = blend_splat(&rs[r], &t.splat[k], m);
                ds = true;
            }
            if t.biomes[k] != rb[r] && !keep_biome(t.grid_to_world(i, j), m) {
                t.biomes[k] = rb[r];
                db = true;
            }
        }
    }
    if dh {
        t.mark_heights_dirty(rect);
    }
    if ds {
        t.mark_splat_dirty(rect);
    }
    if db {
        t.mark_biome_dirty(rect);
    }
    dh || ds || db
}

/// `from` + (`to` − `from`) × m per layer, total kept ≤ 255.
fn blend_splat(from: &[u8; LAYER_COUNT], to: &[u8; LAYER_COUNT], m: f32) -> [u8; LAYER_COUNT] {
    let mut out = [0u8; LAYER_COUNT];
    let mut sum = 0u32;
    for l in 0..LAYER_COUNT {
        let v = from[l] as f32 + (to[l] as f32 - from[l] as f32) * m;
        out[l] = v.round().clamp(0.0, 255.0) as u8;
        sum += out[l] as u32;
    }
    if sum > 255 {
        let k = 255.0 / sum as f32;
        for v in &mut out {
            *v = (*v as f32 * k).floor() as u8;
        }
    }
    out
}

/// Biomes are ids, not blendable: an organic, noisy line through the middle of the feather.
fn keep_biome(p: Vec2, m: f32) -> bool {
    m > 0.3 + 0.4 * value2(p.x / 14.0, p.y / 14.0, 4711)
}

/// Objects thin out across the feather (a stable per-object threshold).
fn keep_object(f: &MaskField, id: u64, p: Vec3) -> bool {
    let m = f.at(p.xz());
    let h = (crate::noise::hash_u32((id as u32) ^ ((id >> 32) as u32).wrapping_mul(0x9E37_79B9))
        & 0xffff) as f32
        / 65535.0;
    m > 0.0 && m >= 0.02 + h * 0.96
}

/// Restores an object the step changed outside the mask. Returns whether it did.
fn settle_object(f: &MaskField, store: &mut ObjectStore, id: u64, before: Option<&Placed>) -> bool {
    let after = store.items.get(&id);
    let same = match (before, after) {
        (Some(b), Some(a)) => {
            b.pos == a.pos
                && b.rot == a.rot
                && b.scale == a.scale
                && b.asset == a.asset
                && b.variant == a.variant
        }
        (None, None) => true,
        _ => false,
    };
    if same {
        return false;
    }
    let ok = before.is_none_or(|b| keep_object(f, id, b.translation()))
        && after.is_none_or(|a| keep_object(f, id, a.translation()));
    if ok {
        return false;
    }
    let before = before.cloned();
    store.remove(id);
    if let Some(b) = before {
        store.insert(b);
    }
    true
}

/// Share of `pts` inside the mask.
fn inside_share(f: &MaskField, pts: impl Iterator<Item = Vec2>) -> f32 {
    let (mut n, mut inside) = (0, 0);
    for p in pts {
        n += 1;
        inside += (f.at(p) > 0.0) as usize;
    }
    if n == 0 {
        0.0
    } else {
        inside as f32 / n as f32
    }
}

fn same<T: serde::Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// Water bodies: new ones outside are dropped (rivers are cut to the part inside), changed or
/// removed ones outside are restored. Returns whether anything changed.
fn settle_water(f: &MaskField, before: &WaterBodies, water: &mut WaterBodies) -> bool {
    let mut changed = false;
    // rivers
    let river_pts = |r: &crate::water::River| {
        r.points
            .iter()
            .map(|p| Vec2::new(p[0], p[2]))
            .collect::<Vec<_>>()
    };
    let mut rivers = vec![];
    for r in std::mem::take(&mut water.rivers) {
        match before.rivers.iter().find(|b| b.id == r.id) {
            Some(b) if same(b, &r) => rivers.push(r),
            Some(b) => {
                let outside = inside_share(f, river_pts(b).into_iter()) < 0.5;
                changed |= outside;
                rivers.push(if outside { b.clone() } else { r });
            }
            None => {
                // the longest run of points inside
                let pts = river_pts(&r);
                let (mut best, mut cur) = ((0, 0), (0, 0));
                for (k, p) in pts.iter().enumerate() {
                    if f.at(*p) > 0.05 {
                        if cur.1 == 0 {
                            cur.0 = k;
                        }
                        cur.1 += 1;
                        if cur.1 > best.1 {
                            best = cur;
                        }
                    } else {
                        cur.1 = 0;
                    }
                }
                if best.1 == pts.len() {
                    rivers.push(r);
                } else {
                    changed = true;
                    if best.1 >= 4 {
                        let mut r = r;
                        r.points = r.points[best.0..best.0 + best.1].to_vec();
                        rivers.push(r);
                    }
                }
            }
        }
    }
    for b in &before.rivers {
        if !rivers.iter().any(|r| r.id == b.id) && inside_share(f, river_pts(b).into_iter()) < 0.5 {
            rivers.push(b.clone());
            changed = true;
        }
    }
    water.rivers = rivers;
    // lakes
    let lake_pts = |l: &crate::water::Lake| -> Vec<Vec2> {
        if l.outline.is_empty() {
            vec![Vec2::from_array(l.seed)]
        } else {
            l.outline.iter().map(|p| Vec2::from_array(*p)).collect()
        }
    };
    let mut lakes = vec![];
    for l in std::mem::take(&mut water.lakes) {
        match before.lakes.iter().find(|b| b.id == l.id) {
            Some(b) if same(b, &l) => lakes.push(l),
            Some(b) => {
                let outside = f.at(Vec2::from_array(b.seed)) <= 0.0
                    && inside_share(f, lake_pts(b).into_iter()) < 0.5;
                changed |= outside;
                lakes.push(if outside { b.clone() } else { l });
            }
            None => {
                let keep = f.at(Vec2::from_array(l.seed)) > 0.2
                    && inside_share(f, lake_pts(&l).into_iter()) >= 0.5;
                changed |= !keep;
                if keep {
                    lakes.push(l);
                }
            }
        }
    }
    for b in &before.lakes {
        if !lakes.iter().any(|l| l.id == b.id) && f.at(Vec2::from_array(b.seed)) <= 0.0 {
            lakes.push(b.clone());
            changed = true;
        }
    }
    water.lakes = lakes;
    // waterfalls and fish: by position
    let fall_in = |w: &crate::water::Waterfall| {
        w.path
            .first()
            .is_some_and(|p| f.at(Vec2::new(p[0], p[2])) > 0.0)
    };
    let n0 = water.waterfalls.len();
    water
        .waterfalls
        .retain(|w| before.waterfalls.iter().any(|b| b.id == w.id) || fall_in(w));
    for b in &before.waterfalls {
        if !water.waterfalls.iter().any(|w| w.id == b.id) && !fall_in(b) {
            water.waterfalls.push(b.clone());
        }
    }
    let fish_in = |s: &crate::fish::FishSchool| f.at(Vec2::new(s.center[0], s.center[2])) > 0.0;
    let f0 = water.fish.len();
    water
        .fish
        .retain(|s| before.fish.iter().any(|b| b.id == s.id) || fish_in(s));
    for b in &before.fish {
        if !water.fish.iter().any(|s| s.id == b.id) && !fish_in(b) {
            water.fish.push(b.clone());
        }
    }
    changed || n0 != water.waterfalls.len() || f0 != water.fish.len()
}
