//! Fitting objects to the ground: which objects level the ground under themselves, at what
//! height, and the system that keeps every world fitted — for any edit, from any tool (the
//! viewport, the inspector, MCP, scripts) and for worlds made before this existed (fitted once
//! when they load, as an undoable step).
//!
//! A building is seated at the median of the natural ground under its footprint, so cuts and
//! fills balance: on a slope it cuts into the hill uphill and stands on a fill downhill
//! (`ground::Foundations::rebuild` shapes both). Moving, turning or resizing it gives the old
//! spot its ground back and fits the new one, in the same undo step as the edit. Grass never
//! grows under a fitted footprint (`GrassMask`, read by `grass.rs`) and procedural nature keeps
//! clear of every structure's footprint (`nature::place::Masks`).

use std::collections::HashSet;
use std::sync::Arc;

use bevy::prelude::*;

use super::footprint::{Footprint, ModelBounds};
use super::ground::{Foundations, Pad};
use crate::history::History;
use crate::objects::{Category, Library, LibraryEntry, ObjectStore, Placed};
use crate::terrain::{GridRect, Terrain};
use crate::water::WaterBodies;

/// History key of the foundations' undo snapshots.
pub const UNDO_KEY: &str = "foundations";
/// A fitted object's origin sits this far below its levelled ground, so its base never shows a
/// hairline gap.
pub const SINK: f32 = 0.03;

/// Everything a fit reads and changes.
pub struct FitWorld<'a> {
    pub terrain: &'a mut Terrain,
    pub history: &'a mut History,
    pub store: &'a mut ObjectStore,
    pub lib: &'a Library,
    pub water: &'a WaterBodies,
    pub found: &'a mut Foundations,
    pub bounds: &'a ModelBounds,
}

/// Whether this kind of object levels the ground under itself unless told otherwise: built
/// structures with a real floor area (walk-in buildings, fort pieces, tents, big tables) — not
/// ships, boats or bridges, and not props, nature or imported models (those can opt in).
pub fn fits_by_default(e: &LibraryEntry, bounds: (Vec3, Vec3)) -> bool {
    if e.category != Category::Structures {
        return false;
    }
    let has = |w: &str| {
        e.id.as_bytes()
            .windows(w.len())
            .any(|x| x.eq_ignore_ascii_case(w.as_bytes()))
    };
    if ["ship", "boat", "pinnace", "bridge"].into_iter().any(has) {
        return false;
    }
    let size = bounds.1 - bounds.0;
    size.x.min(size.z) >= 1.5 && size.x * size.z >= 4.0
}

/// Whether object `p` should stand on levelled ground: its own choice, else its kind's.
pub fn wants(found: &Foundations, lib: &Library, bounds: &ModelBounds, p: &Placed) -> bool {
    lib.get(&p.asset).is_some_and(|e| {
        found
            .overrides
            .get(&p.id)
            .copied()
            .unwrap_or_else(|| fits_by_default(e, bounds.of(e)))
    })
}

/// The footprint object `p` is fitted with, if it should be (and can be: objects tilted to the
/// slope and pier pieces standing in water are left alone). Also whether it is stone-built.
pub fn wanted(w: &FitWorld, p: &Placed) -> Option<(Footprint, bool)> {
    if !wants(w.found, w.lib, w.bounds, p) {
        return None;
    }
    let e = w.lib.get(&p.asset)?;
    if (p.rotation() * Vec3::Y).y < 0.995 {
        return None;
    }
    let fp = Footprint::of(p, w.bounds.of(e));
    if !w.terrain.in_bounds(fp.center.x, fp.center.y) {
        return None;
    }
    // (well under the ground in a cave it stands on the cave's floor: the terrain is the ground
    // over it, as for objects riding the ground in `objects::follow_ground`)
    if p.pos[1] < w.found.base_at(w.terrain, fp.center.x, fp.center.y) - 2.0
        && w.found.in_caves(p.translation())
    {
        return None;
    }
    if e.floats() {
        let wet = fp.corners().into_iter().chain([fp.center]).any(|q| {
            let g = w.found.base_at(w.terrain, q.x, q.y);
            w.water
                .surface_at(Vec3::new(q.x, g, q.y), w.terrain.sea_level)
                > g + 0.05
        });
        if wet {
            return None;
        }
    }
    Some((fp, e.surface == crate::terrain::layers::Surface::Stone))
}

/// The floor height for a footprint: the median of the natural ground under it, so the cut into
/// the slope and the fill under the low side balance.
pub fn seat_level(found: &Foundations, t: &Terrain, fp: &Footprint) -> f32 {
    let nx = ((fp.half.x * 2.0).ceil() as usize).clamp(2, 24);
    let nz = ((fp.half.y * 2.0).ceil() as usize).clamp(2, 24);
    let mut hs = Vec::with_capacity((nx + 1) * (nz + 1));
    for b in 0..=nz {
        for a in 0..=nx {
            let l = Vec2::new(
                (a as f32 / nx as f32 * 2.0 - 1.0) * fp.half.x,
                (b as f32 / nz as f32 * 2.0 - 1.0) * fp.half.y,
            );
            let q = fp.center + fp.ax * l.x + fp.az() * l.y;
            hs.push(found.base_at(t, q.x, q.y));
        }
    }
    hs.sort_by(f32::total_cmp);
    hs[hs.len() / 2]
}

/// The deepest cut and highest fill (m) a pad makes under its footprint.
pub fn cut_fill(found: &Foundations, t: &Terrain, pad: &Pad) -> (f32, f32) {
    let fp = pad.footprint();
    let (mut cut, mut fill) = (0.0f32, 0.0f32);
    for c in fp.corners().into_iter().chain([fp.center]) {
        let d = found.base_at(t, c.x, c.y) - pad.level;
        cut = cut.max(d);
        fill = fill.max(-d);
    }
    (cut, fill)
}

/// What a fit would change: objects whose pad is missing or stale get a new one (`Some`),
/// objects that no longer want one (moved away, deleted, opted out) give it up (`None`).
/// `only`: just these objects.
pub fn plan(w: &mut FitWorld, only: Option<&[u64]>) -> Vec<(u64, Option<Pad>)> {
    w.found.validate(w.terrain);
    let ids: Vec<u64> = match only {
        Some(ids) => ids.to_vec(),
        None => {
            let mut v: Vec<u64> = w
                .store
                .items
                .keys()
                .copied()
                .chain(w.found.pads.keys().copied())
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        }
    };
    let mut todo = vec![];
    for id in ids {
        let want = w
            .store
            .items
            .get(&id)
            .and_then(|p| wanted(w, p).map(|(fp, stone)| (p, fp, stone)));
        match (want, w.found.pads.get(&id)) {
            (Some((p, _, _)), Some(pad)) if !pad.stale(p) => {}
            (Some((p, fp, stone)), _) => {
                let level = seat_level(w.found, w.terrain, &fp);
                todo.push((id, Some(Pad::new(p, fp, level, stone))));
            }
            (None, Some(_)) => todo.push((id, None)),
            (None, None) => {}
        }
    }
    todo
}

/// Fits the objects whose pad is missing or stale and gives the ground back where an object no
/// longer wants one (`plan`), inside the caller's history step (objects, terrain tiles and the
/// foundations' snapshot). Returns the objects whose pad changed.
pub fn sync(w: &mut FitWorld, only: Option<&[u64]>) -> Vec<u64> {
    let todo = plan(w, only);
    apply(w, todo)
}

/// Carries out a `plan`.
pub fn apply(w: &mut FitWorld, todo: Vec<(u64, Option<Pad>)>) -> Vec<u64> {
    if todo.is_empty() {
        return vec![];
    }
    let before = w.found.clone();
    let mut rect: Option<GridRect> = None;
    let mut grow = |r: Option<GridRect>| {
        if let Some(r) = r {
            rect = Some(rect.map_or(r, |c| c.union(r)));
        }
    };
    let fitted: Vec<u64> = todo.iter().map(|(id, _)| *id).collect();
    let fitting: HashSet<u64> = w
        .found
        .pads
        .keys()
        .copied()
        .chain(fitted.iter().copied())
        .collect();
    for (id, new) in todo {
        let old = w.found.pads.get(&id).cloned();
        grow(old.as_ref().and_then(|o| o.region(w.terrain)));
        let Some(pad) = new else {
            w.found.pads.remove(&id);
            continue;
        };
        grow(pad.region(w.terrain));
        // seat it on the pad (touched even when it stays put, so it doesn't ride the ground change)
        if let Some(p) = w.store.items.get(&id).cloned() {
            w.history.touch_object(w.store, id);
            let dy = pad.level - SINK - p.pos[1];
            if dy.abs() > 1e-4 {
                let mut np = p.clone();
                np.pos[1] += dy;
                w.store.update(np);
                // fitted where it stands: what stands inside it (furniture, a chandelier) keeps
                // its place on the floor
                if old.as_ref().is_none_or(|o| o.at == [p.pos[0], p.pos[2]]) {
                    let fp = pad.footprint();
                    let inside: Vec<u64> =
                        w.store
                            .query(fp.center, fp.half.length() + 0.5)
                            .into_iter()
                            .filter(|o| !fitting.contains(o))
                            .filter(|o| {
                                w.store.items.get(o).is_some_and(|q| {
                                    fp.contains(Vec2::new(q.pos[0], q.pos[2]), -0.05)
                                })
                            })
                            .collect();
                    for o in inside {
                        w.history.touch_object(w.store, o);
                        if let Some(mut q) = w.store.items.get(&o).cloned() {
                            q.pos[1] += dy;
                            w.store.update(q);
                        }
                    }
                }
            }
        }
        w.found.pads.insert(id, pad);
    }
    if let Some(r) = rect {
        w.found.rebuild(w.terrain, w.history, r);
    }
    w.found.dirty = false;
    w.history.push_ext(
        "Fit to the ground",
        UNDO_KEY,
        Arc::new(before),
        Arc::new(w.found.clone()),
    );
    fitted
}

/// Fits objects `ids` now, first recording each one's choice in `choice` (`Some(true)`: level the
/// ground under it whatever it is, `Some(false)`: never, `None`: as its kind does). Inside the
/// caller's history step. Returns the objects whose pad changed.
pub fn fit_objects(w: &mut FitWorld, ids: &[u64], choice: &[(u64, Option<bool>)]) -> Vec<u64> {
    if !choice.is_empty() {
        let before = w.found.clone();
        for &(id, on) in choice {
            match on {
                Some(v) => w.found.overrides.insert(id, v),
                None => w.found.overrides.remove(&id),
            };
        }
        w.history.push_ext(
            "Level the ground",
            UNDO_KEY,
            Arc::new(before),
            Arc::new(w.found.clone()),
        );
    }
    sync(w, Some(ids))
}

/// An explicit height change of a fitted object (the lift handle, the height field, MCP): its
/// pad follows to the new floor (a fill under it, a cut above) instead of leaving a gap or
/// burying it. Inside the caller's step; false if it isn't fitted or didn't change height.
pub fn follow_lift(w: &mut FitWorld, id: u64) -> bool {
    let Some(p) = w.store.items.get(&id).cloned() else {
        return false;
    };
    let Some(pad) = w.found.pads.get(&id) else {
        return false;
    };
    if pad.stale(&p) || (p.pos[1] + SINK - pad.level).abs() < 0.01 {
        return false;
    }
    let Some((fp, stone)) = wanted(w, &p) else {
        return false;
    };
    let pad = Pad::new(&p, fp, p.pos[1] + SINK, stone);
    apply(w, vec![(id, Some(pad))]);
    true
}

/// The land was regenerated under the objects: forget the remembered ground and the pads (inside
/// the caller's step; the objects are fitted again to the new land).
pub fn forget(found: &mut Foundations, history: &mut History) {
    if found.pads.is_empty() {
        return;
    }
    let before = found.clone();
    found.forget();
    history.push_ext(
        "Generate",
        UNDO_KEY,
        Arc::new(before),
        Arc::new(found.clone()),
    );
}

/// Undo / redo: the foundations go back with the objects and terrain of the step.
pub(super) fn restore_undo(history: &mut History, found: &mut Foundations) {
    if !history.ext_apply.iter().any(|(k, _)| *k == UNDO_KEY) {
        return;
    }
    let mut keep = vec![];
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        if key != UNDO_KEY {
            keep.push((key, snap));
        } else if let Some(f) = snap.downcast_ref::<Foundations>() {
            *found = f.clone();
        }
    }
    history.ext_apply = keep;
}

#[derive(Default)]
pub(super) struct AutoState {
    rev: Option<u64>,
    generation: u64,
    serial: u64,
    /// every object's place (position, turn, size, asset) when last looked at
    seen: std::collections::HashMap<u64, u64>,
}

/// What an object's fit depends on (not its height: lifting is deliberate).
fn place_key(p: &Placed) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.asset.hash(&mut h);
    for v in [p.pos[0], p.pos[2]]
        .into_iter()
        .chain(p.rot)
        .chain(p.scale3().to_array())
    {
        v.to_bits().hash(&mut h);
    }
    h.finish()
}

/// Keeps every world fitted: after any edit (once the mouse is released), objects that were
/// placed, moved, turned or resized are fitted, deleted ones give their ground back, and a loaded
/// world's buildings are fitted once. The fit joins the undo step of the edit that caused it.
/// Only what changed is looked at, so undoing a fit leaves the ground as it was.
#[allow(clippy::too_many_arguments)]
pub(super) fn auto_fit(
    mut terrain: ResMut<Terrain>,
    mut history: ResMut<History>,
    mut store: ResMut<ObjectStore>,
    lib: Option<Res<Library>>,
    water: Res<WaterBodies>,
    mut found: ResMut<Foundations>,
    bounds: Res<ModelBounds>,
    mut meta: ResMut<crate::world::WorldMeta>,
    stroke: Res<crate::tools::Stroke>,
    buttons: Res<ButtonInput<MouseButton>>,
    handles: Res<super::Handles>,
    mut st: Local<AutoState>,
    caves: Res<crate::caves::Caves>,
    dungeon: Option<Res<crate::dungeons::DungeonMap>>,
) {
    restore_undo(&mut history, &mut found);
    // (the caves' bounds: objects in them are left on their floors; in a dungeon's underground
    // map everything is underground)
    let interior = dungeon.as_ref().is_some_and(|d| d.interior());
    if caves.is_changed() || found.caves.len() != caves.list.len() + interior as usize {
        let mut b: Vec<(Vec3, Vec3)> = caves.list.iter().map(|c| c.bounds()).collect();
        if interior {
            b.push((
                Vec3::new(-1.0e6, -1.0e6, -1.0e6),
                Vec3::new(1.0e6, crate::dungeons::render::ROOF - 2.0, 1.0e6),
            ));
        }
        if b != found.caves {
            found.bypass_change_detection().caves = b;
        }
    }
    let Some(lib) = lib else { return };
    // (fitted when the drag ends, not on every frame of it)
    if stroke.active || buttons.pressed(MouseButton::Left) || handles.drag.is_some() {
        return;
    }
    if st.rev == Some(store.revision) && st.generation == terrain.generation && !found.dirty {
        return;
    }
    // what changed: new, moved, turned or resized objects, and deleted ones that had a pad (an
    // override changed: everything)
    if st.generation != terrain.generation {
        st.seen.clear();
    }
    let mut changed: Vec<u64> = vec![];
    for (id, p) in &store.items {
        let k = place_key(p);
        if st.seen.insert(*id, k) != Some(k) {
            changed.push(*id);
        }
    }
    if st.seen.len() > store.items.len() {
        st.seen.retain(|id, _| store.items.contains_key(id));
    }
    changed.extend(found.pads.keys().filter(|id| !store.items.contains_key(id)));
    changed.sort_unstable();
    let only = (!found.dirty).then_some(changed);
    // (looking changes nothing: the terrain isn't flagged as changed for it)
    let mut w = FitWorld {
        terrain: terrain.bypass_change_detection(),
        history: &mut history,
        store: &mut store,
        lib: &lib,
        water: &water,
        found: &mut found,
        bounds: &bounds,
    };
    let todo = plan(&mut w, only.as_deref());
    w.found.dirty = false;
    st.rev = Some(w.store.revision);
    st.generation = w.terrain.generation;
    if todo.is_empty() {
        return;
    }
    // the edit that caused this, if it was just committed: the fit joins its step
    let serial = history.serial();
    let fresh = serial != st.serial && history.undo_serial() == Some(serial);
    let recent = history.recent_objects.clone();
    let label = history.undo_label().unwrap_or_default().to_string();
    let open = history.is_recording();
    if !open {
        history.begin("Fit to the ground");
    }
    let t0 = std::time::Instant::now();
    let fitted = apply(
        &mut FitWorld {
            terrain: &mut terrain,
            history: &mut history,
            store: &mut store,
            lib: &lib,
            water: &water,
            found: &mut found,
            bounds: &bounds,
        },
        todo,
    );
    if !open {
        if fitted.len() > 1 {
            history.set_label("Fit buildings to the ground");
        }
        history.commit(&store);
        let merge = fresh && !fitted.is_empty() && fitted.iter().all(|id| recent.contains(id));
        if merge && history.serial() == serial + 1 {
            history.squash_after(serial - 1, label);
        }
    }
    meta.dirty = true;
    info!(
        "FIT {} object(s) fitted to the ground in {:.1} ms: {fitted:?}",
        fitted.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    st.rev = Some(store.revision);
    st.serial = history.serial();
}

/// Terrain vertices where grass never grows: every vertex of the cells under a fitted object's
/// footprint (the grass map is per vertex and filtered between them, so the whole floor stays
/// bare whatever the grid size). Read by `grass.rs`.
#[derive(Resource, Default)]
pub struct GrassMask {
    pub n: usize,
    verts: HashSet<u32>,
}

impl GrassMask {
    /// Clears the grass density (channel 0) of masked vertices inside `rect` of a map with `n`
    /// vertices per side.
    pub fn clear(&self, data: &mut [[u8; 4]], n: usize, rect: GridRect) {
        if self.n != n {
            return;
        }
        for &v in &self.verts {
            let (i, j) = (v as usize % n, v as usize / n);
            if i >= rect.x0 && i <= rect.x1 && j >= rect.z0 && j <= rect.z1 {
                data[v as usize][0] = 0;
            }
        }
    }
}

/// The vertices of every terrain cell `fp` (grown by `margin`) touches.
pub fn cell_vertices(t: &Terrain, fp: &Footprint, margin: f32, out: &mut HashSet<u32>) {
    let (lo, hi) = fp.aabb(margin);
    let g0 = t.world_to_grid(lo.x, lo.y).floor().max(Vec2::ZERO);
    let g1 = t
        .world_to_grid(hi.x, hi.y)
        .ceil()
        .min(Vec2::splat(t.res as f32));
    let n = t.n();
    for cj in g0.y as usize..(g1.y as usize).max(g0.y as usize) {
        for ci in g0.x as usize..(g1.x as usize).max(g0.x as usize) {
            let a = t.grid_to_world(ci, cj);
            if fp.overlaps_rect(a, a + Vec2::splat(t.cell), margin) {
                for (i, j) in [(ci, cj), (ci + 1, cj), (ci, cj + 1), (ci + 1, cj + 1)] {
                    if i < n && j < n {
                        out.insert((j * n + i) as u32);
                    }
                }
            }
        }
    }
}

/// Keeps the grass mask in step with the fitted objects (and walk-in buildings left unfitted):
/// the grass map is recomputed where it changed.
#[allow(clippy::too_many_arguments)]
pub(super) fn update_grass_mask(
    mut mask: ResMut<GrassMask>,
    mut terrain: ResMut<Terrain>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    found: Res<Foundations>,
    bounds: Res<ModelBounds>,
    stroke: Res<crate::tools::Stroke>,
    fields: Res<crate::fields::Fields>,
    mut seen: Local<Option<(u64, u64, usize, u64)>>,
) {
    let Some(lib) = lib else { return };
    let key = (
        store.revision,
        terrain.generation,
        found.pads.len(),
        fields.revision,
    );
    if stroke.active || *seen == Some(key) {
        return;
    }
    *seen = Some(key);
    let t = &*terrain;
    let mut verts = HashSet::new();
    for pad in found.pads.values() {
        cell_vertices(t, &pad.footprint(), 0.25, &mut verts);
    }
    for p in store.items.values() {
        if found.pads.contains_key(&p.id) {
            continue;
        }
        if let Some(e) = lib.get(&p.asset)
            && e.walk_in()
        {
            cell_vertices(t, &Footprint::of(p, bounds.of(e)), 0.25, &mut verts);
        }
    }
    // and none in farm fields: the crop grows there (crate::fields)
    for f in &fields.list {
        let poly = f.poly();
        let (lo, hi) = crate::fields::bounds(&poly);
        let (a, b) = (t.world_to_grid(lo.x, lo.y), t.world_to_grid(hi.x, hi.y));
        let n = t.n();
        for j in (a.y.floor().max(0.0) as usize)..=(b.y.ceil() as usize).min(n - 1) {
            for i in (a.x.floor().max(0.0) as usize)..=(b.x.ceil() as usize).min(n - 1) {
                if crate::setpieces::poly_sdf(&poly, t.grid_to_world(i, j)) < 1.0 {
                    verts.insert((j * n + i) as u32);
                }
            }
        }
    }
    let n = t.n();
    if mask.n == n && mask.verts == verts {
        return;
    }
    // the region whose grass changes
    let changed: Vec<u32> = if mask.n == n {
        mask.verts.symmetric_difference(&verts).copied().collect()
    } else {
        verts.iter().copied().collect()
    };
    let mut rect: Option<GridRect> = None;
    for v in changed {
        let (i, j) = (v as usize % n, v as usize / n);
        let r = GridRect {
            x0: i,
            z0: j,
            x1: i,
            z1: j,
        };
        rect = Some(rect.map_or(r, |c| c.union(r)));
    }
    info!(
        "FIT grass kept off {} terrain vertices under buildings and fields",
        verts.len()
    );
    mask.n = n;
    mask.verts = verts;
    if let Some(r) = rect {
        terrain.dirty_grass = Some(terrain.dirty_grass.map_or(r, |d| d.union(r)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;

    fn slope_terrain() -> Terrain {
        // 64 cells of 4 m; ground rising 0.25 m per metre along x (a 14° slope) plus a bump
        let mut t = Terrain::new(64, 4.0, -50.0);
        for j in 0..t.n() {
            for i in 0..t.n() {
                let p = t.grid_to_world(i, j);
                let k = t.idx(i, j);
                t.heights[k] = 10.0 + p.x * 0.25 + (p.y * 0.3).sin() * 0.4;
            }
        }
        t
    }

    fn building(id: u64, x: f32, z: f32, yaw: f32) -> Placed {
        Placed {
            group: 0,
            id,
            asset: "sb:test_hall".into(),
            variant: 0,
            pos: [x, 0.0, z],
            rot: Quat::from_rotation_y(yaw).to_array(),
            scale: 1.0,
            stretch: None,
            locked: false,
            glow: None,
        }
    }

    fn library() -> Library {
        let mut lib = Library::empty_for_tests();
        lib.add(LibraryEntry {
            id: "sb:test_hall".into(),
            name: "Test hall".into(),
            category: Category::Structures,
            group: "buildings".into(),
            kind: None,
            import_path: None,
            surface: crate::terrain::layers::Surface::Wood,
            radius: 5.5,
            align: false,
            walkable: true,
            whole_scene: false,
            biomes: vec![],
            sway: false,
            default_scale: 1.0,
            meshes: vec![],
            scene: None,
            height: 6.0,
            tree: None,
            textured: false,
            pack: crate::objects::PHOTOREAL.into(),
        });
        lib
    }

    fn bounds() -> ModelBounds {
        let mut b = ModelBounds::default();
        b.0.insert(
            "sb:test_hall".into(),
            (Vec3::new(-3.0, -0.04, -5.0), Vec3::new(3.0, 6.0, 5.0)),
        );
        b
    }

    struct World {
        t: Terrain,
        h: History,
        store: ObjectStore,
        lib: Library,
        water: WaterBodies,
        found: Foundations,
        bounds: ModelBounds,
    }

    impl World {
        fn new() -> Self {
            World {
                t: slope_terrain(),
                h: History::default(),
                store: ObjectStore::default(),
                lib: library(),
                water: WaterBodies::default(),
                found: Foundations::default(),
                bounds: bounds(),
            }
        }
        /// One edit step: `f` changes the objects, then the fit runs in the same step.
        fn step(&mut self, f: impl FnOnce(&mut ObjectStore, &mut History)) -> Vec<u64> {
            self.h.begin("edit");
            f(&mut self.store, &mut self.h);
            let out = sync(
                &mut FitWorld {
                    terrain: &mut self.t,
                    history: &mut self.h,
                    store: &mut self.store,
                    lib: &self.lib,
                    water: &self.water,
                    found: &mut self.found,
                    bounds: &self.bounds,
                },
                None,
            );
            self.h.commit(&self.store);
            out
        }
        fn undo(&mut self) {
            let mut gp = crate::gameplay::Gameplay::default();
            let mut roads = crate::roads::Roads::default();
            let mut zones = crate::world::Zones::default();
            self.h.step(
                false,
                &mut self.t,
                &mut self.store,
                &mut self.water,
                &mut zones,
                &mut gp,
                &mut roads,
            );
            restore_undo(&mut self.h, &mut self.found);
        }
    }

    #[test]
    fn fit_levels_the_footprint_and_seats_the_object() {
        let mut w = World::new();
        let natural = w.t.heights.clone();
        let fitted = w.step(|s, h| {
            h.touch_object(s, 1);
            s.insert(building(1, 3.0, -2.0, 0.4));
        });
        assert_eq!(fitted, vec![1]);
        let pad = w.found.pads[&1].clone();
        let p = &w.store.items[&1];
        assert!((p.pos[1] - (pad.level - SINK)).abs() < 1e-4);
        // the ground under the whole footprint is the pad level: no gap, nothing poking through
        let fp = pad.footprint();
        for k in 0..400 {
            let l = Vec2::new(
                (k % 20) as f32 / 19.0 * 2.0 - 1.0,
                (k / 20) as f32 / 19.0 * 2.0 - 1.0,
            ) * fp.half;
            let q = fp.center + fp.ax * l.x + fp.az() * l.y;
            assert!(
                (w.t.height_at(q.x, q.y) - pad.level).abs() < 0.01,
                "ground {} vs pad {}",
                w.t.height_at(q.x, q.y),
                pad.level
            );
        }
        // the median: some of the footprint was cut, some filled
        let (cut, fill) = cut_fill(&w.found, &w.t, &pad);
        assert!(cut > 0.3 && fill > 0.3, "cut {cut} fill {fill}");
        // far away, nothing changed
        let far = w.t.idx(2, 2);
        assert_eq!(w.t.heights[far], natural[far]);
    }

    #[test]
    fn moving_restores_the_old_ground_and_undo_is_exact() {
        let mut w = World::new();
        let natural = w.t.heights.clone();
        let natural_paint = w.t.splat.clone();
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.insert(building(1, -20.0, 10.0, 0.0));
        });
        let after_place = w.t.heights.clone();
        let old_rect = w.found.pads[&1].region(&w.t).unwrap();
        // move it 40 m away
        w.step(|s, h| {
            h.touch_object(s, 1);
            let mut p = s.items[&1].clone();
            p.pos[0] = 25.0;
            s.update(p);
        });
        let new_fp = w.found.pads[&1].footprint();
        for j in old_rect.z0..=old_rect.z1 {
            for i in old_rect.x0..=old_rect.x1 {
                let q = w.t.grid_to_world(i, j);
                if new_fp.sdist(q) < 20.0 {
                    continue;
                }
                let k = w.t.idx(i, j);
                assert!(
                    (w.t.heights[k] - natural[k]).abs() < 1e-3,
                    "old spot not restored at {i},{j}"
                );
                assert_eq!(w.t.splat[k], natural_paint[k]);
            }
        }
        // undo the move: exactly as after placing
        w.undo();
        assert_eq!(w.t.heights, after_place);
        assert!((w.store.items[&1].pos[0] + 20.0).abs() < 1e-5);
        assert!(!w.found.pads[&1].stale(&w.store.items[&1]));
        // deleting it gives all the ground back
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.remove(1);
        });
        assert!(w.found.pads.is_empty());
        for (a, b) in w.t.heights.iter().zip(&natural) {
            assert!((a - b).abs() < 1e-3);
        }
        assert_eq!(w.found.covered_count(), 0);
    }

    #[test]
    fn edits_on_a_pad_survive_a_refit() {
        let mut w = World::new();
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.insert(building(1, 0.0, 0.0, 0.0));
        });
        // sculpt a mound 6 m from the building, on its bank
        let fp = w.found.pads[&1].footprint();
        let g =
            w.t.world_to_grid(fp.center.x + fp.half.x + 6.0, fp.center.y);
        let k = w.t.idx(g.x.round() as usize, g.y.round() as usize);
        let pre = w.t.heights[k];
        w.t.heights[k] += 2.0;
        // turn the building a little in place: the sculpted vertex keeps its +2 m
        w.step(|s, h| {
            h.touch_object(s, 1);
            let mut p = s.items[&1].clone();
            p.rot = Quat::from_rotation_y(0.05).to_array();
            s.update(p);
        });
        assert!(w.t.heights[k] > pre + 0.5, "{} vs {}", w.t.heights[k], pre);
        // and deleting the building leaves the mound on the natural slope
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.remove(1);
        });
        let natural = slope_terrain().heights[k];
        assert!((w.t.heights[k] - (natural + 2.0)).abs() < 1e-3);
    }

    #[test]
    fn fitting_is_deterministic() {
        let run = || {
            let mut w = World::new();
            w.step(|s, h| {
                for (id, x, z, yaw) in [
                    (1, 0.0, 0.0, 0.3),
                    (2, 8.0, 4.0, 1.2),
                    (3, -30.0, 20.0, 2.0),
                ] {
                    h.touch_object(s, id);
                    s.insert(building(id, x, z, yaw));
                }
            });
            (
                w.t.heights,
                w.t.splat,
                w.store
                    .items
                    .values()
                    .map(|p| (p.id, p.pos[1].to_bits()))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            )
        };
        let (a, b) = (run(), run());
        assert!(a.0 == b.0 && a.1 == b.1 && a.2 == b.2);
    }

    #[test]
    fn lifting_raises_the_pad_with_it() {
        let mut w = World::new();
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.insert(building(1, 0.0, 0.0, 0.0));
        });
        let level = w.found.pads[&1].level;
        w.h.begin("lift");
        w.h.touch_object(&w.store, 1);
        let mut p = w.store.items[&1].clone();
        p.pos[1] += 1.5;
        w.store.update(p);
        assert!(follow_lift(
            &mut FitWorld {
                terrain: &mut w.t,
                history: &mut w.h,
                store: &mut w.store,
                lib: &w.lib,
                water: &w.water,
                found: &mut w.found,
                bounds: &w.bounds
            },
            1
        ));
        w.h.commit(&w.store);
        let pad = &w.found.pads[&1];
        assert!((pad.level - level - 1.5).abs() < 1e-4);
        let c = pad.footprint().center;
        assert!((w.t.height_at(c.x, c.y) - pad.level).abs() < 0.01);
        // a refit check finds nothing to do (the lift isn't undone by the automatic fit)
        let mut fw = FitWorld {
            terrain: &mut w.t,
            history: &mut w.h,
            store: &mut w.store,
            lib: &w.lib,
            water: &w.water,
            found: &mut w.found,
            bounds: &w.bounds,
        };
        assert!(plan(&mut fw, None).is_empty());
    }

    #[test]
    fn saved_foundations_load_back() {
        let mut w = World::new();
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.insert(building(1, 0.0, 0.0, 0.0));
        });
        w.found.overrides.insert(7, false);
        let json = serde_json::to_string(&w.found).unwrap();
        let back: Foundations = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pads, w.found.pads);
        assert_eq!(back.overrides, w.found.overrides);
        assert_eq!(back.covered_count(), w.found.covered_count());
        let (i, j) = (32, 32);
        assert!((back.base(i, j).unwrap_or(0.0) - w.found.base(i, j).unwrap_or(0.0)).abs() < 1e-3);
    }

    #[test]
    fn opting_out_gives_the_ground_back() {
        let mut w = World::new();
        let natural = w.t.heights.clone();
        w.step(|s, h| {
            h.touch_object(s, 1);
            s.insert(building(1, 0.0, 0.0, 0.0));
        });
        w.h.begin("opt out");
        fit_objects(
            &mut FitWorld {
                terrain: &mut w.t,
                history: &mut w.h,
                store: &mut w.store,
                lib: &w.lib,
                water: &w.water,
                found: &mut w.found,
                bounds: &w.bounds,
            },
            &[1],
            &[(1, Some(false))],
        );
        w.h.commit(&w.store);
        assert!(w.found.pads.is_empty());
        for (a, b) in w.t.heights.iter().zip(&natural) {
            assert!((a - b).abs() < 1e-3);
        }
        // undo brings the choice and the pad back together
        w.undo();
        assert!(w.found.pads.contains_key(&1) && !w.found.overrides.contains_key(&1));
    }
}
