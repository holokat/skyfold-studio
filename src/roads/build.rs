//! Road edits shared by the tools, the inspector and MCP: create, update, rebuild and remove a
//! road. Each one is a single undo step: the roads (with the ground store), the terrain tiles the
//! bed and paint touched, the vegetation it cleared or put back and the lamp posts it placed.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::ground::{self, Base};
use super::plan::{self, RoadPlan};
use super::{Road, Roads};
use crate::history::History;
use crate::objects::{Category, Library, ObjectStore, Placed};
use crate::terrain::{GridRect, Terrain};
use crate::water::WaterBodies;

/// Photoscanned lamp post placed along paved and cobbled roads.
pub const LAMP: &str = "ph:street_lamp_01";

pub enum Edit {
    Create(Road),
    /// Replace a road's definition (points, kind, width, options, name).
    Update(Road),
    /// Re-carve, re-clear and re-place props with the current settings.
    Rebuild(u32),
    Remove(u32),
}

/// Everything a road edit touches.
#[derive(SystemParam)]
pub struct RoadWorld<'w> {
    pub terrain: ResMut<'w, Terrain>,
    pub history: ResMut<'w, History>,
    pub store: ResMut<'w, ObjectStore>,
    pub lib: Option<Res<'w, Library>>,
    pub water: Res<'w, WaterBodies>,
    pub roads: ResMut<'w, Roads>,
    pub meta: ResMut<'w, crate::world::WorldMeta>,
}

impl RoadWorld<'_> {
    /// Applies an edit as one undo step (merged into a stroke that is being recorded). Returns
    /// the road id.
    pub fn apply(&mut self, edit: Edit, label: &str) -> Result<u32, String> {
        let own = !self.history.is_recording();
        if own {
            self.history.begin(label);
        } else {
            self.history.set_label(label);
        }
        let r = self.apply_inner(edit);
        if own {
            self.history.commit(&self.store);
        }
        self.meta.dirty = true;
        r
    }

    fn apply_inner(&mut self, edit: Edit) -> Result<u32, String> {
        let RoadWorld {
            terrain,
            history,
            store,
            lib,
            water,
            roads,
            ..
        } = self;
        let t: &mut Terrain = terrain;
        let roads: &mut Roads = roads;
        let lib = lib.as_deref();
        roads.ground.validate(t);
        // what changes
        let (old, new): (Option<Road>, Option<Road>) = match edit {
            Edit::Create(mut r) => {
                if r.points.len() < 2 {
                    return Err("a road needs at least two points".into());
                }
                r.id = roads.alloc_id();
                if r.name.is_empty() {
                    r.name = format!("{} {}", r.kind.name(), r.id);
                }
                (None, Some(r))
            }
            Edit::Update(r) => {
                let old = roads
                    .get(r.id)
                    .cloned()
                    .ok_or(format!("no road {}", r.id))?;
                if r.points.len() < 2 {
                    return Err("a road needs at least two points".into());
                }
                (Some(old), Some(r))
            }
            Edit::Rebuild(id) => {
                let old = roads.get(id).cloned().ok_or(format!("no road {id}"))?;
                (Some(old.clone()), Some(old))
            }
            Edit::Remove(id) => (
                Some(roads.get(id).cloned().ok_or(format!("no road {id}"))?),
                None,
            ),
        };
        history.touch_roads(roads);
        let id = new.as_ref().or(old.as_ref()).map(|r| r.id).unwrap_or(0);

        // the region to rebuild: where the road was and where it is now
        let base = Base {
            t,
            g: &roads.ground,
        };
        let mut region: Option<(Vec2, Vec2)> = None;
        for r in old.iter().chain(new.iter()) {
            let p = plan::plan_road(r, &base, water, &[]);
            region = Some(match region {
                Some((a, b)) => (a.min(p.lo), b.max(p.hi)),
                None => (p.lo, p.hi),
            });
        }
        let Some((lo, hi)) = region else {
            return Ok(id);
        };
        let rect = grid_rect(
            t,
            lo - Vec2::splat(t.cell * 2.0),
            hi + Vec2::splat(t.cell * 2.0),
        );

        // update the road list (the edited road keeps its vegetation and props for now)
        let mut cleared = old.as_ref().map(|o| o.cleared.clone()).unwrap_or_default();
        let old_props = old.as_ref().map(|o| o.prop_ids.clone()).unwrap_or_default();
        let at = roads.roads.iter().position(|r| r.id == id);
        match (new.clone(), at) {
            (Some(mut r), at) => {
                r.cleared.clear();
                r.prop_ids.clear();
                match at {
                    Some(i) => roads.roads[i] = r,
                    None => roads.roads.push(r),
                }
            }
            (None, Some(i)) => {
                roads.roads.remove(i);
            }
            (None, None) => {}
        }

        // lamp posts come out before the ground changes (they are placed again on the new bed)
        for pid in old_props {
            if store.items.contains_key(&pid) {
                history.touch_object(store, pid);
                store.remove(pid);
            }
        }

        // bed and paint
        let plans = plan::plan_all(
            roads,
            &Base {
                t,
                g: &roads.ground,
            },
            water,
        );
        if let Some(rect) = rect {
            let inside: Vec<&RoadPlan> = plans.iter().filter(|p| overlaps(t, p, rect)).collect();
            ground::rebuild(t, history, &mut roads.ground, &inside, rect);
        }
        let plans = plan::plan_all(
            roads,
            &Base {
                t,
                g: &roads.ground,
            },
            water,
        );

        // vegetation: put back what no road covers any more, clear what the road now crosses
        let covers = |p: &Placed, plans: &[RoadPlan], roads: &Roads| -> Option<u32> {
            let q = Vec2::new(p.pos[0], p.pos[2]);
            plans
                .iter()
                .find(|pl| {
                    roads.get(pl.id).is_some_and(|r| r.options.clear) && clear_hit(pl, lib, p, q)
                })
                .map(|pl| pl.id)
        };
        let mut kept: Vec<(u32, Placed)> = vec![];
        for p in cleared.drain(..) {
            match covers(&p, &plans, roads) {
                Some(owner) => kept.push((owner, p)),
                None => {
                    if !store.items.contains_key(&p.id) {
                        history.touch_object(store, p.id);
                        store.insert(p);
                    }
                }
            }
        }
        for (owner, p) in kept {
            if let Some(r) = roads.roads.iter_mut().find(|r| r.id == owner) {
                r.cleared.push(p);
            }
        }
        if let Some(r) = new.as_ref()
            && r.options.clear
            && let Some(pl) = plans.iter().find(|p| p.id == r.id)
        {
            let c = (pl.lo + pl.hi) * 0.5;
            let rad = (pl.hi - pl.lo).length() * 0.5;
            let hits: Vec<u64> = store
                .query(c, rad)
                .into_iter()
                .filter(|oid| {
                    store.items.get(oid).is_some_and(|o| {
                        let entry = lib.and_then(|l| l.get(&o.asset));
                        entry.is_some_and(|e| Category::NATURE.contains(&e.category))
                            && clear_hit(pl, lib, o, Vec2::new(o.pos[0], o.pos[2]))
                    })
                })
                .collect();
            let mut removed = vec![];
            for oid in hits {
                history.touch_object(store, oid);
                if let Some(p) = store.remove(oid) {
                    removed.push(p);
                }
            }
            if let Some(rr) = roads.roads.iter_mut().find(|x| x.id == r.id) {
                rr.cleared.extend(removed);
            }
        }

        // lamp posts
        if let Some(r) = new.as_ref()
            && r.options.props
            && r.kind.style().lamps > 0.0
            && let Some(pl) = plans.iter().find(|p| p.id == r.id)
            && lib.is_some_and(|l| l.get(LAMP).is_some())
            && crate::market::usable(LAMP)
        {
            let ids = place_lamps(t, water, history, store, pl, &plans);
            if let Some(rr) = roads.roads.iter_mut().find(|x| x.id == r.id) {
                rr.prop_ids = ids;
            }
        }
        roads.dirty = true;
        roads.selected = new.map(|r| r.id);
        Ok(id)
    }
}

fn grid_rect(t: &Terrain, lo: Vec2, hi: Vec2) -> Option<GridRect> {
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

fn overlaps(t: &Terrain, p: &RoadPlan, r: GridRect) -> bool {
    let a = t.grid_to_world(r.x0, r.z0);
    let b = t.grid_to_world(r.x1, r.z1);
    !(p.hi.x < a.x || p.hi.y < a.y || p.lo.x > b.x || p.lo.y > b.y)
}

/// Is a plant / tree / rock in the road's way (on land, within the width plus a margin)?
fn clear_hit(pl: &RoadPlan, lib: Option<&Library>, o: &Placed, q: Vec2) -> bool {
    let (cat, radius) = lib
        .and_then(|l| l.get(&o.asset))
        .map(|e| (e.category, e.radius * o.scale))
        .unwrap_or((Category::Plants, 0.5));
    let margin = match cat {
        Category::Trees => 1.4 + radius * 0.35,
        Category::Rocks => 0.5 + radius,
        _ => 0.4 + radius * 0.6,
    };
    pl.nearest(q, pl.hw + margin)
        .is_some_and(|(_, s)| pl.bed_at(s) > 0.5)
}

/// Lamp posts at even spacing, alternating sides, skipping bridges, junctions, water and steep
/// verges. Returns the placed object ids.
fn place_lamps(
    t: &Terrain,
    water: &WaterBodies,
    history: &mut History,
    store: &mut ObjectStore,
    pl: &RoadPlan,
    plans: &[RoadPlan],
) -> Vec<u64> {
    let spacing = pl.kind.style().lamps;
    let len = pl.length();
    let mut ids = vec![];
    let mut s = spacing * 0.5;
    let mut side = 1.0f32;
    while s < len - 3.0 {
        let here = s;
        s += spacing;
        let (i, _) = pl.locate(here);
        if pl.span_of(i).is_some() || pl.span_of(i + 1).is_some() || pl.bed_at(here) < 1.0 {
            continue;
        }
        let p = pl.point_at(here);
        let dir = (pl.point_at(here + 0.5) - pl.point_at(here - 0.5)).normalize_or(Vec2::X);
        let q = p + dir.perp() * side * (pl.hw + 0.7);
        side = -side;
        if !t.in_bounds(q.x, q.y) {
            continue;
        }
        // keep clear of other roads (junctions) and of the water
        if plans
            .iter()
            .any(|o| o.id != pl.id && o.nearest(q, o.hw + 2.5).is_some())
        {
            continue;
        }
        let g = super::mesh::surface_height(t, q.x, q.y);
        if plan::water_surface(water, t.sea_level, q, g) > g - 0.1 || t.normal_at(q.x, q.y).y < 0.85
        {
            continue;
        }
        // the lantern arm reaches over the road
        let yaw = (-dir.y).atan2(dir.x)
            + if side > 0.0 {
                std::f32::consts::PI
            } else {
                0.0
            };
        let id = store.new_id();
        history.touch_object(store, id);
        store.insert(Placed {
            group: 0,
            id,
            asset: LAMP.to_string(),
            variant: 0,
            stretch: None,
            pos: [q.x, g - 0.04, q.y],
            rot: Quat::from_rotation_y(yaw).to_array(),
            scale: 1.0,
            locked: false,
            glow: None,
        });
        ids.push(id);
    }
    ids
}

/// The landscape under the roads was replaced (Generate): their old ground and cleared plants
/// belong to the old one. The roads stay and are rebuilt on the new ground once its water is in
/// place (`regrade`).
pub fn forget_ground(roads: &mut Roads, history: &mut History) {
    if roads.roads.is_empty() {
        return;
    }
    history.touch_roads(roads);
    roads.ground = Default::default();
    for r in &mut roads.roads {
        r.cleared.clear();
    }
    roads.regrade = true;
}

/// Rebuilds every road after `forget_ground` (one undo step), when the new water is ready.
pub fn regrade(mut rw: RoadWorld) {
    if !rw.roads.regrade || rw.water.dirty {
        return;
    }
    rw.roads.regrade = false;
    let ids: Vec<u32> = rw.roads.roads.iter().map(|r| r.id).collect();
    rw.history.begin("Rebuild roads");
    for id in ids {
        if let Err(e) = rw.apply(Edit::Rebuild(id), "Rebuild roads") {
            warn!("ROADS rebuild failed: {e}");
        }
    }
    let RoadWorld { history, store, .. } = &mut rw;
    history.commit(store);
}
