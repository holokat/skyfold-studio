//! Undo / redo. A brush stroke (mouse down → up) becomes one undo step that can include
//! terrain tiles (heights, paint, zones), placed objects, water bodies, zone definitions and the
//! gameplay layer.
//! Every change is stored as a swap, so undo and redo are the same operation.
//! A group (`begin_group` … `end_group`, used by MCP batches) records several edits as one step:
//! commits inside it only extend the open step.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::gameplay::Gameplay;
use crate::objects::{ObjectStore, Placed};
use crate::terrain::layers::LAYER_COUNT;
use crate::terrain::{GridRect, Terrain};
use crate::water::WaterBodies;
use crate::world::{Zone, Zones};

pub const TILE: usize = 64;

/// A terrain tile (`TILE` × `TILE` vertices from `tx * TILE`, `tz * TILE`; clipped at the edge) as
/// it was before a step changed it.
pub struct TileSnap {
    pub tx: usize,
    pub tz: usize,
    pub heights: Vec<f32>,
    pub splat: Vec<[u8; LAYER_COUNT]>,
    pub zones: Vec<u8>,
    pub biomes: Vec<u8>,
}

enum Change {
    Terrain(Vec<TileSnap>),
    /// The zone definitions and their painted grid.  Auto-zoning changes neither terrain
    /// heights, splat nor biomes, so retaining this compact swap avoids a whole-world terrain
    /// snapshot.
    ZoneLayout {
        list: Vec<Zone>,
        grid: Vec<u8>,
        selected: Option<usize>,
    },
    /// id, state before, state after
    Objects(Vec<(u64, Option<Placed>, Option<Placed>)>),
    Water(Box<WaterBodies>),
    Zones(Vec<Zone>),
    Gameplay(Box<Gameplay>),
    Roads(Box<crate::roads::Roads>),
    /// Feature data outside the core types (camera paths, …): whole snapshots before and after,
    /// handed to their owner through `History::ext_apply`.
    Ext(&'static str, ExtSnapshot, ExtSnapshot),
}

/// A feature's state snapshot (the owner downcasts it to its own type).
pub type ExtSnapshot = std::sync::Arc<dyn std::any::Any + Send + Sync>;

struct Entry {
    label: String,
    changes: Vec<Change>,
    /// Counts up with every commit (see `History::serial`).
    serial: u64,
}

impl Entry {
    /// Memory held by the terrain snapshots of this step.
    fn bytes(&self) -> usize {
        self.changes
            .iter()
            .map(|c| match c {
                Change::Terrain(tiles) => tiles
                    .iter()
                    .map(|s| {
                        s.heights.len() * 4
                            + s.splat.len() * LAYER_COUNT
                            + s.zones.len()
                            + s.biomes.len()
                    })
                    .sum(),
                Change::ZoneLayout { list, grid, .. } => {
                    grid.len()
                        + list
                            .iter()
                            .map(|z| z.name.len() + z.description.len() + 160)
                            .sum::<usize>()
                }
                _ => 0,
            })
            .sum()
    }
}

/// Copy of one tile (`all`: every layer, else heights only).
fn snap_tile(t: &Terrain, tx: usize, tz: usize, n: usize, all: bool) -> TileSnap {
    let (x0, z0) = (tx * TILE, tz * TILE);
    let (x1, z1) = ((x0 + TILE).min(n), (z0 + TILE).min(n));
    let mut s = TileSnap {
        tx,
        tz,
        heights: vec![],
        splat: vec![],
        zones: vec![],
        biomes: vec![],
    };
    for j in z0..z1 {
        let a = t.idx(x0, j);
        let b = t.idx(x1 - 1, j) + 1;
        s.heights.extend_from_slice(&t.heights[a..b]);
        if all {
            s.splat.extend_from_slice(&t.splat[a..b]);
            s.zones.extend_from_slice(&t.zones[a..b]);
            s.biomes.extend_from_slice(&t.biomes[a..b]);
        }
    }
    s
}

#[derive(Default)]
struct Pending {
    label: String,
    tiles: HashMap<(usize, usize), TileSnap>,
    objects: HashMap<u64, Option<Placed>>,
    water: Option<WaterBodies>,
    zones: Option<Vec<Zone>>,
    gameplay: Option<Gameplay>,
    roads: Option<crate::roads::Roads>,
    /// feature snapshots pushed while a group is open: (key, first before, last after)
    ext: Vec<(&'static str, ExtSnapshot, ExtSnapshot)>,
}

#[derive(Resource, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    pending: Option<Pending>,
    /// Objects placed explicitly by the latest commit / undo / redo. Objects that merely follow
    /// the ground after a terrain change skip these, since their positions are already final.
    pub recent_objects: HashSet<u64>,
    /// Snapshots an undo / redo restored for features outside the core types: (key, state). Each
    /// owner takes its own key's entries and restores them.
    pub ext_apply: Vec<(&'static str, ExtSnapshot)>,
    /// Serial of the newest committed step (0: none yet).
    serial: u64,
    /// A group is open: commits keep the step open until `end_group`.
    group: bool,
    /// Edits begun inside the open group.
    group_edits: u32,
}

const MAX_STEPS: usize = 60;
/// Undo memory budget: the oldest steps go first beyond it (whole-world steps of a 16 km world
/// are hundreds of MB each).
const MAX_BYTES: usize = 2 << 30;

impl History {
    pub fn begin(&mut self, label: impl Into<String>) {
        match &mut self.pending {
            None => {
                self.pending = Some(Pending {
                    label: label.into(),
                    ..default()
                })
            }
            Some(p) if self.group => {
                self.group_edits += 1;
                if p.label.is_empty() {
                    p.label = label.into();
                }
            }
            Some(_) => {}
        }
    }
    pub fn is_recording(&self) -> bool {
        self.pending.is_some()
    }
    /// Renames the step being recorded (e.g. a click that turned out to place a marker).
    pub fn set_label(&mut self, label: impl Into<String>) {
        if let Some(p) = &mut self.pending {
            p.label = label.into();
        }
    }

    /// Snapshot terrain tiles intersecting `rect` before they are modified.
    pub fn touch_terrain(&mut self, t: &Terrain, rect: GridRect) {
        let Some(p) = &mut self.pending else { return };
        let n = t.n();
        for tz in rect.z0 / TILE..=rect.z1 / TILE {
            for tx in rect.x0 / TILE..=rect.x1 / TILE {
                if let Some(s) = p.tiles.get(&(tx, tz)) {
                    if s.splat.is_empty() && !s.heights.is_empty() {
                        // a heights-only snapshot: take the rest now, before it changes
                        let full = snap_tile(t, tx, tz, n, true);
                        let s = p.tiles.get_mut(&(tx, tz)).unwrap();
                        (s.splat, s.zones, s.biomes) = (full.splat, full.zones, full.biomes);
                    }
                    continue;
                }
                p.tiles.insert((tx, tz), snap_tile(t, tx, tz, n, true));
            }
        }
    }
    /// Snapshot only the heights of the tiles intersecting `rect` (erosion and other height-only
    /// changes of big areas: a quarter of the memory).
    pub fn touch_heights(&mut self, t: &Terrain, rect: GridRect) {
        let Some(p) = &mut self.pending else { return };
        let n = t.n();
        for tz in rect.z0 / TILE..=rect.z1 / TILE {
            for tx in rect.x0 / TILE..=rect.x1 / TILE {
                p.tiles
                    .entry((tx, tz))
                    .or_insert_with(|| snap_tile(t, tx, tz, n, false));
            }
        }
    }
    pub fn touch_object(&mut self, store: &ObjectStore, id: u64) {
        if let Some(p) = &mut self.pending {
            p.objects
                .entry(id)
                .or_insert_with(|| store.items.get(&id).cloned());
        }
    }
    pub fn touch_water(&mut self, w: &WaterBodies) {
        if let Some(p) = &mut self.pending
            && p.water.is_none()
        {
            p.water = Some(w.clone());
        }
    }
    pub fn touch_roads(&mut self, r: &crate::roads::Roads) {
        if let Some(p) = &mut self.pending
            && p.roads.is_none()
        {
            p.roads = Some(r.clone());
        }
    }
    pub fn touch_zones(&mut self, z: &Zones) {
        if let Some(p) = &mut self.pending
            && p.zones.is_none()
        {
            p.zones = Some(z.list.clone());
        }
    }

    /// Records a complete zone-definition/grid edit as a compact swap. Call this after the
    /// caller has replaced the world with its new zone layout; `before_*` are moved in rather
    /// than cloned and become the undo side of the swap.
    pub fn record_zone_layout(
        &mut self,
        label: impl Into<String>,
        list: Vec<Zone>,
        grid: Vec<u8>,
        selected: Option<usize>,
    ) -> u64 {
        debug_assert!(!self.is_recording() && !self.in_group());
        self.push_entry(Entry {
            label: label.into(),
            changes: vec![Change::ZoneLayout {
                list,
                grid,
                selected,
            }],
            serial: 0,
        });
        self.serial
    }

    pub fn touch_gameplay(&mut self, g: &Gameplay) {
        if let Some(p) = &mut self.pending
            && p.gameplay.is_none()
        {
            p.gameplay = Some(g.clone());
        }
    }

    /// Opens a group: every edit until `end_group` becomes one undo step.
    pub fn begin_group(&mut self, store: &ObjectStore) {
        self.commit_step(store);
        self.group = true;
        self.group_edits = 0;
        self.pending = Some(Pending::default());
    }

    pub fn in_group(&self) -> bool {
        self.group
    }

    /// Closes the group and commits its step (labelled `label`, or the single edit's own label
    /// when only one edit happened). Returns whether a step was recorded.
    pub fn end_group(&mut self, store: &ObjectStore, label: impl Into<String>) -> bool {
        self.group = false;
        if let Some(p) = &mut self.pending
            && (self.group_edits > 1 || p.label.is_empty())
        {
            p.label = label.into();
        }
        self.commit_step(store)
    }

    /// Drops the newest redo step (after undoing a failed group, which must not be redone).
    pub fn drop_redo(&mut self) {
        self.redo.pop();
    }

    pub fn commit(&mut self, store: &ObjectStore) {
        if self.group {
            return;
        }
        self.commit_step(store);
    }

    fn commit_step(&mut self, store: &ObjectStore) -> bool {
        let Some(p) = self.pending.take() else {
            return false;
        };
        let mut changes = vec![];
        if !p.tiles.is_empty() {
            changes.push(Change::Terrain(p.tiles.into_values().collect()));
        }
        self.recent_objects = p.objects.keys().copied().collect();
        if !p.objects.is_empty() {
            let objs: Vec<_> = p
                .objects
                .into_iter()
                .map(|(id, before)| (id, before, store.items.get(&id).cloned()))
                .filter(|(_, b, a)| b.is_some() || a.is_some())
                .collect();
            if !objs.is_empty() {
                changes.push(Change::Objects(objs));
            }
        }
        if let Some(w) = p.water {
            changes.push(Change::Water(Box::new(w)));
        }
        if let Some(z) = p.zones {
            changes.push(Change::Zones(z));
        }
        if let Some(g) = p.gameplay {
            changes.push(Change::Gameplay(Box::new(g)));
        }
        if let Some(r) = p.roads {
            changes.push(Change::Roads(Box::new(r)));
        }
        for (key, before, after) in p.ext {
            changes.push(Change::Ext(key, before, after));
        }
        if changes.is_empty() {
            return false;
        }
        self.push_entry(Entry {
            label: p.label,
            changes,
            serial: 0,
        });
        true
    }

    /// A finished step onto the undo stack (the oldest go beyond the step and memory budgets).
    fn push_entry(&mut self, mut e: Entry) {
        self.serial += 1;
        e.serial = self.serial;
        self.undo.push(e);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
        let mut total: usize = self.undo.iter().map(Entry::bytes).sum();
        while total > MAX_BYTES && self.undo.len() > 1 {
            total -= self.undo.remove(0).bytes();
        }
    }

    /// Record one finished change of a feature's own data (a complete undo step; inside a group
    /// or an open step, part of that step).
    pub fn push_ext(
        &mut self,
        label: impl Into<String>,
        key: &'static str,
        before: ExtSnapshot,
        after: ExtSnapshot,
    ) {
        if let Some(p) = &mut self.pending {
            match p.ext.iter_mut().find(|e| e.0 == key) {
                Some(e) => e.2 = after,
                None => p.ext.push((key, before, after)),
            }
            if self.group {
                self.group_edits += 1;
            }
            if p.label.is_empty() {
                p.label = label.into();
            }
            return;
        }
        self.push_entry(Entry {
            label: label.into(),
            changes: vec![Change::Ext(key, before, after)],
            serial: 0,
        });
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.group = false;
        self.recent_objects.clear();
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|e| e.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.label.as_str())
    }

    // ---- for code that post-processes steps (the selection's edit mask, `prompt::mask`) and
    // groups them (an area prompt's whole agent run is one undo step)

    /// Serial of the newest committed step: every commit counts up from 0.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// Serial of the step `undo` would revert.
    pub fn undo_serial(&self) -> Option<u64> {
        self.undo.last().map(|e| e.serial)
    }

    /// Serials of the undoable steps committed after `serial`, oldest first.
    pub fn serials_after(&self, serial: u64) -> Vec<u64> {
        self.undo
            .iter()
            .filter(|e| e.serial > serial)
            .map(|e| e.serial)
            .collect()
    }

    /// What a committed step recorded: its terrain tiles and objects as they were before it, and the
    /// water bodies before it (if it changed water).
    pub fn step_before(
        &self,
        serial: u64,
    ) -> Option<(
        Vec<&TileSnap>,
        Vec<(u64, Option<&Placed>)>,
        Option<&WaterBodies>,
    )> {
        let e = self.undo.iter().rev().find(|e| e.serial == serial)?;
        let (mut tiles, mut objects, mut water) = (vec![], vec![], None);
        for c in &e.changes {
            match c {
                Change::Terrain(t) => tiles.extend(t.iter()),
                Change::Objects(list) => {
                    objects.extend(list.iter().map(|(id, before, _)| (*id, before.as_ref())))
                }
                Change::Water(w) => water = Some(&**w),
                _ => {}
            }
        }
        Some((tiles, objects, water))
    }

    /// The step being recorded: terrain tiles and objects as they were before it.
    pub fn open_before(&self) -> Option<(Vec<&TileSnap>, Vec<(u64, Option<&Placed>)>)> {
        let p = self.pending.as_ref()?;
        Some((
            p.tiles.values().collect(),
            p.objects.iter().map(|(id, b)| (*id, b.as_ref())).collect(),
        ))
    }

    /// Re-reads the after-state of objects `ids` of step `serial` from the store (after a
    /// post-process changed them), so redo restores what is in the world now.
    pub fn refresh_objects(&mut self, serial: u64, store: &ObjectStore, ids: &HashSet<u64>) {
        let Some(e) = self.undo.iter_mut().rev().find(|e| e.serial == serial) else {
            return;
        };
        for c in &mut e.changes {
            if let Change::Objects(list) = c {
                for (id, _, after) in list.iter_mut() {
                    if ids.contains(id) {
                        *after = store.items.get(id).cloned();
                    }
                }
            }
        }
    }

    /// A committed step's feature snapshots under `key`: (before, after).
    pub fn step_ext(&self, serial: u64, key: &str) -> Option<(ExtSnapshot, ExtSnapshot)> {
        let e = self.undo.iter().rev().find(|e| e.serial == serial)?;
        e.changes.iter().find_map(|c| match c {
            Change::Ext(k, b, a) if *k == key => Some((b.clone(), a.clone())),
            _ => None,
        })
    }

    /// Replaces the after-state under `key` of step `serial` (after a post-process changed it),
    /// so redo restores what is in the world now.
    pub fn refresh_ext(&mut self, serial: u64, key: &str, after: ExtSnapshot) {
        let Some(e) = self.undo.iter_mut().rev().find(|e| e.serial == serial) else {
            return;
        };
        for c in &mut e.changes {
            if let Change::Ext(k, _, a) = c
                && *k == key
            {
                *a = after.clone();
            }
        }
    }

    /// Merges the undoable steps committed after `serial` into one step named `label` (it undoes
    /// them all at once). Returns the merged step's serial.
    pub fn squash_after(&mut self, serial: u64, label: impl Into<String>) -> Option<u64> {
        let first = self.undo.iter().position(|e| e.serial > serial)?;
        let merged: Vec<Entry> = self.undo.drain(first..).collect();
        let newest = merged.last().map(|e| e.serial).unwrap_or(serial);
        // terrain tiles, water, zones and gameplay keep their oldest "before"; objects their
        // oldest before and newest after
        let mut tiles: HashMap<(usize, usize), TileSnap> = HashMap::new();
        let mut objects: Vec<(u64, Option<Placed>, Option<Placed>)> = vec![];
        let mut obj_index: HashMap<u64, usize> = HashMap::new();
        let (mut water, mut zones, mut gameplay, mut roads, mut zone_layout) =
            (None, None, None, None, None);
        // feature snapshots: per key, the oldest before and the newest after
        let mut ext: Vec<(&'static str, ExtSnapshot, ExtSnapshot)> = vec![];
        for e in merged {
            for c in e.changes {
                match c {
                    Change::Terrain(list) => {
                        for t in list {
                            tiles.entry((t.tx, t.tz)).or_insert(t);
                        }
                    }
                    Change::ZoneLayout {
                        list,
                        grid,
                        selected,
                    } => {
                        if zone_layout.is_none() {
                            zone_layout = Some((list, grid, selected));
                        }
                    }
                    Change::Objects(list) => {
                        for (id, before, after) in list {
                            match obj_index.get(&id) {
                                Some(&i) => objects[i].2 = after,
                                None => {
                                    obj_index.insert(id, objects.len());
                                    objects.push((id, before, after));
                                }
                            }
                        }
                    }
                    Change::Water(w) => water = water.or(Some(w)),
                    Change::Zones(z) => zones = zones.or(Some(z)),
                    Change::Gameplay(g) => gameplay = gameplay.or(Some(g)),
                    Change::Roads(r) => roads = roads.or(Some(r)),
                    Change::Ext(key, before, after) => match ext.iter_mut().find(|x| x.0 == key) {
                        Some(x) => x.2 = after,
                        None => ext.push((key, before, after)),
                    },
                }
            }
        }
        let mut changes = vec![];
        if !tiles.is_empty() {
            changes.push(Change::Terrain(tiles.into_values().collect()));
        }
        if let Some((list, grid, selected)) = zone_layout {
            changes.push(Change::ZoneLayout {
                list,
                grid,
                selected,
            });
        }
        objects.retain(|(_, b, a)| b.is_some() || a.is_some());
        if !objects.is_empty() {
            changes.push(Change::Objects(objects));
        }
        changes.extend(water.map(Change::Water));
        changes.extend(zones.map(Change::Zones));
        changes.extend(gameplay.map(Change::Gameplay));
        changes.extend(roads.map(Change::Roads));
        changes.extend(ext.into_iter().map(|(k, b, a)| Change::Ext(k, b, a)));
        if !changes.is_empty() {
            self.undo.push(Entry {
                label: label.into(),
                changes,
                serial: newest,
            });
        }
        Some(newest)
    }

    /// Undo (`redo == false`) or redo the last step. Returns the step label.
    pub fn step(
        &mut self,
        redo: bool,
        terrain: &mut Terrain,
        store: &mut ObjectStore,
        water: &mut WaterBodies,
        zones: &mut Zones,
        gameplay: &mut Gameplay,
        roads: &mut crate::roads::Roads,
    ) -> Option<String> {
        self.pending = None;
        let mut entry = if redo {
            self.redo.pop()?
        } else {
            self.undo.pop()?
        };
        self.recent_objects.clear();
        for c in &mut entry.changes {
            match c {
                Change::Terrain(tiles) => {
                    let n = terrain.n();
                    for s in tiles.iter_mut() {
                        let (x0, z0) = (s.tx * TILE, s.tz * TILE);
                        let (x1, z1) = ((x0 + TILE).min(n), (z0 + TILE).min(n));
                        let w = x1 - x0;
                        for (row, j) in (z0..z1).enumerate() {
                            let a = terrain.idx(x0, j);
                            let r = row * w..row * w + w;
                            terrain.heights[a..a + w].swap_with_slice(&mut s.heights[r.clone()]);
                            if s.splat.is_empty() {
                                continue; // heights-only snapshot
                            }
                            terrain.splat[a..a + w].swap_with_slice(&mut s.splat[r.clone()]);
                            terrain.zones[a..a + w].swap_with_slice(&mut s.zones[r.clone()]);
                            terrain.biomes[a..a + w].swap_with_slice(&mut s.biomes[r]);
                        }
                        let rect = GridRect {
                            x0,
                            z0,
                            x1: x1 - 1,
                            z1: z1 - 1,
                        };
                        terrain.mark_heights_dirty(rect);
                        terrain.mark_splat_dirty(rect);
                        terrain.mark_zone_dirty(rect);
                        terrain.mark_biome_dirty(rect);
                    }
                }
                Change::ZoneLayout {
                    list,
                    grid,
                    selected,
                } => {
                    // A compact zone-only history entry must not touch any other terrain
                    // layer: its redo side is produced by this very swap.
                    std::mem::swap(&mut zones.list, list);
                    std::mem::swap(&mut terrain.zones, grid);
                    std::mem::swap(&mut zones.selected, selected);
                    let n = terrain.n();
                    terrain.mark_zone_dirty(GridRect {
                        x0: 0,
                        z0: 0,
                        x1: n - 1,
                        z1: n - 1,
                    });
                }
                Change::Roads(r) => {
                    std::mem::swap(roads, r.as_mut());
                    roads.dirty = true;
                }
                Change::Objects(list) => {
                    self.recent_objects
                        .extend(list.iter().map(|(id, _, _)| *id));
                    for (id, before, after) in list.iter() {
                        let target = if redo { after } else { before };
                        store.remove(*id);
                        if let Some(p) = target {
                            store.insert(p.clone());
                        }
                    }
                }
                Change::Water(w) => {
                    std::mem::swap(water, w.as_mut());
                    water.dirty = true;
                }
                Change::Zones(z) => {
                    std::mem::swap(&mut zones.list, z);
                    if zones.selected.is_some_and(|i| i >= zones.list.len()) {
                        zones.selected = None;
                    }
                }
                Change::Ext(key, before, after) => {
                    self.ext_apply
                        .push((key, if redo { after.clone() } else { before.clone() }));
                }
                Change::Gameplay(g) => {
                    // the selection stays (the inspector drops it if the item is gone)
                    let sel = gameplay.selected;
                    std::mem::swap(gameplay, g.as_mut());
                    gameplay.selected = sel;
                    gameplay.dirty = true;
                }
            }
        }
        let label = entry.label.clone();
        if redo {
            self.undo.push(entry);
        } else {
            self.redo.push(entry);
        }
        Some(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_layout_swap_undo_redo_preserves_other_terrain_layers() {
        let mut terrain = Terrain::new(2, 1.0, 0.0);
        terrain.heights.fill(7.0);
        terrain.splat.fill([1; LAYER_COUNT]);
        terrain.biomes.fill(3);
        let heights = terrain.heights.clone();
        let splat = terrain.splat.clone();
        let biomes = terrain.biomes.clone();
        let mut zones = Zones {
            list: vec![Zone::new(1, "Kept".into())],
            selected: Some(0),
        };
        terrain.zones.fill(1);
        let before_grid = terrain.zones.clone();
        let before_list = zones.list.clone();
        terrain.zones.fill(2);
        zones.list.push(Zone::new(2, "Generated".into()));
        zones.selected = Some(1);
        let mut history = History::default();
        history.record_zone_layout("Auto zones", before_list, before_grid, Some(0));
        let mut store = ObjectStore::default();
        let mut water = WaterBodies::default();
        let mut gameplay = Gameplay::default();
        let mut roads = crate::roads::Roads::default();
        history.step(
            false,
            &mut terrain,
            &mut store,
            &mut water,
            &mut zones,
            &mut gameplay,
            &mut roads,
        );
        assert_eq!(terrain.zones, vec![1; 9]);
        assert_eq!(zones.list.len(), 1);
        assert_eq!(zones.selected, Some(0));
        assert_eq!(terrain.heights, heights);
        assert_eq!(terrain.splat, splat);
        assert_eq!(terrain.biomes, biomes);
        history.step(
            true,
            &mut terrain,
            &mut store,
            &mut water,
            &mut zones,
            &mut gameplay,
            &mut roads,
        );
        assert_eq!(terrain.zones, vec![2; 9]);
        assert_eq!(zones.list.len(), 2);
        assert_eq!(zones.selected, Some(1));
    }
}
