//! The analysis job: a snapshot of the world taken on the main thread (terrain and water copies,
//! object footprints, roads, gameplay, zones), and the computation that runs on a worker thread.
//! Every result part carries the fingerprint of the inputs it was made from, so a re-run only
//! recomputes what changed (moving the travel start doesn't rebuild the walk grid).

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use bevy::prelude::*;

use super::grid::{self, Field, Footprint, RoadLine, WalkGrid};
use super::{AnalysisSettings, doctor, flow, travel};
use crate::gameplay::Gameplay;
use crate::objects::{Category, Library, ObjectStore};
use crate::procgen::meshes::AssetKind;
use crate::roads::Roads;
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::Zones;

pub struct ZoneInfo {
    pub id: u8,
    pub name: String,
    pub level_min: u32,
    pub level_max: u32,
    pub allow_mounts: bool,
}

/// What the doctor needs to know about a placed object.
pub struct ObjInfo {
    pub id: u64,
    pub name: Arc<str>,
    pub category: Option<Category>,
    pub kind: Option<AssetKind>,
    pub pos: Vec3,
    pub scale: f32,
    pub radius: f32,
    pub height: f32,
    pub foot: Footprint,
    pub walkable: bool,
}

/// A named place the analysis measures from.
#[derive(Clone, Debug)]
pub struct Origin {
    pub pos: Vec2,
    pub label: String,
}

pub struct Snapshot {
    pub field: Field,
    pub footprints: Vec<Footprint>,
    /// Filled only when the doctor runs.
    pub objects: Vec<ObjInfo>,
    pub roads: Vec<RoadLine>,
    pub gameplay: Gameplay,
    pub fish: Vec<Vec2>,
    pub zones: Vec<ZoneInfo>,
    pub settings: AnalysisSettings,
    /// Reachability flood fills start here (the player start, else a fallback).
    pub origin: Option<Origin>,
    /// Travel time is measured from here.
    pub source: Option<Origin>,
}

/// Borrowed world state the snapshot and fingerprints are made from.
pub struct Inputs<'a> {
    pub terrain: &'a Terrain,
    pub water: &'a WaterBodies,
    pub store: &'a ObjectStore,
    pub lib: Option<&'a Library>,
    pub roads: &'a Roads,
    pub gameplay: &'a Gameplay,
    pub zones: &'a Zones,
    /// procedural trees, rocks and logs (`crate::nature`): footprints like placed ones
    pub nature: Option<crate::nature::Ctx<'a>>,
}

impl Inputs<'_> {
    pub fn snapshot(
        &self,
        settings: &AnalysisSettings,
        origin: Option<Origin>,
        source: Option<Origin>,
        doctor: bool,
    ) -> Snapshot {
        let lib = self.lib;
        let mut footprints = Vec::new();
        let mut objects = Vec::new();
        // display names shared per asset
        let mut names: std::collections::HashMap<&str, Arc<str>> = std::collections::HashMap::new();
        for p in self.store.items.values() {
            let e = lib.and_then(|l| l.get(&p.asset));
            let foot = grid::object_footprint(p, e);
            if foot != Footprint::None {
                footprints.push(foot);
            }
            if doctor {
                let kind = grid::asset_kind(p, e);
                let name = names
                    .entry(p.asset.as_str())
                    .or_insert_with(|| {
                        e.map(|e| e.name.clone())
                            .or_else(|| kind.map(|k| k.name().to_string()))
                            .unwrap_or_else(|| p.asset.clone())
                            .into()
                    })
                    .clone();
                objects.push(ObjInfo {
                    id: p.id,
                    name,
                    category: e
                        .map(|e| e.category)
                        .or(kind.map(crate::objects::kind_category)),
                    kind,
                    pos: p.translation(),
                    scale: p.scale,
                    radius: e.map(|e| e.radius).unwrap_or(1.0),
                    height: e.map(|e| e.height).unwrap_or(1.0),
                    foot,
                    walkable: e.is_some_and(|e| e.walkable),
                });
            }
        }
        if let (Some(ctx), Some(lib)) = (&self.nature, lib) {
            for i in crate::nature::place::place_world(ctx, &[0, 1, 3]) {
                let foot =
                    grid::object_footprint(&i.placed(), Some(&lib.entries[i.entry as usize]));
                if foot != Footprint::None {
                    footprints.push(foot);
                }
            }
        }
        // a stable order (the store is a hash map) keeps issue lists steady between runs
        objects.sort_by_key(|o| o.id);
        let roads = self
            .roads
            .roads
            .iter()
            .map(|r| RoadLine {
                name: r.name.clone(),
                kind: r.kind,
                width: r.width,
                pts: r.centreline(2.0),
            })
            .collect();
        Snapshot {
            field: Field::from_world(self.terrain, self.water),
            footprints,
            objects,
            roads,
            gameplay: self.gameplay.clone(),
            fish: self
                .water
                .fish
                .iter()
                .map(|f| Vec2::new(f.center[0], f.center[2]))
                .collect(),
            zones: zone_infos(self.zones),
            settings: settings.clone(),
            origin,
            source,
        }
    }
}

pub fn zone_infos(z: &Zones) -> Vec<ZoneInfo> {
    z.list
        .iter()
        .map(|z| ZoneInfo {
            id: z.id,
            name: z.name.clone(),
            level_min: z.level_min,
            level_max: z.level_max,
            allow_mounts: z.allow_mounts,
        })
        .collect()
}

// ---------------------------------------------------------------- fingerprints

fn h<T: Hash>(v: T) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut s);
    s.finish()
}

fn hf(s: &mut impl Hasher, v: f32) {
    v.to_bits().hash(s);
}

/// Fingerprints of the inputs of every result part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fps {
    pub grid: u64,
    pub reach: u64,
    pub travel: u64,
    pub gapfield: u64,
    pub gaps: u64,
    pub flow: u64,
    pub doctor: u64,
}

pub fn fingerprints(
    i: &Inputs,
    st: &AnalysisSettings,
    origin: &Option<Origin>,
    source: &Option<Origin>,
) -> Fps {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    let t = i.terrain;
    (
        t.generation,
        t.height_revision,
        t.zone_revision,
        t.res,
        i.store.revision,
        t.biome_revision,
    )
        .hash(&mut s);
    if let Some(n) = &i.nature {
        (n.field.revision, n.amounts.map(f32::to_bits), n.seed).hash(&mut s);
    }
    hf(&mut s, t.sea_level);
    hf(&mut s, t.cell);
    i.water.level_map.len().hash(&mut s);
    for r in &i.water.rivers {
        (r.id, r.points.len()).hash(&mut s);
        hf(&mut s, r.width);
        hf(&mut s, r.points.first().map(|p| p[1]).unwrap_or(0.0));
    }
    for l in &i.water.lakes {
        l.id.hash(&mut s);
        hf(&mut s, l.level);
        l.outline.len().hash(&mut s);
    }
    for r in &i.roads.roads {
        (r.id, r.kind, r.points.len()).hash(&mut s);
        hf(&mut s, r.width);
        for p in &r.points {
            hf(&mut s, p[0]);
            hf(&mut s, p[2]);
        }
    }
    for z in &i.zones.list {
        (z.id, z.allow_mounts).hash(&mut s);
    }
    for v in [
        st.cell,
        st.max_walk_slope,
        st.cliff_slope,
        st.swim_depth,
        st.run_speed,
        st.swim_speed,
        st.mount_speed,
    ] {
        hf(&mut s, v);
    }
    (st.steep, st.allow_swim, st.objects_block, st.mounted).hash(&mut s);
    let grid = s.finish();
    let pos = |o: &Option<Origin>| {
        o.as_ref().map(|o| {
            (
                (o.pos.x * 2.0).round() as i64,
                (o.pos.y * 2.0).round() as i64,
            )
        })
    };
    let reach = h((grid, pos(origin)));
    let travel = h((grid, pos(source)));
    let mut c = std::collections::hash_map::DefaultHasher::new();
    grid.hash(&mut c);
    for p in content_sources(
        i.gameplay,
        &i.water
            .fish
            .iter()
            .map(|f| Vec2::new(f.center[0], f.center[2]))
            .collect::<Vec<_>>(),
        &[],
        st,
    ) {
        hf(&mut c, p.0.x);
        hf(&mut c, p.0.y);
    }
    (st.gap_fish, st.gap_roads).hash(&mut c);
    let gapfield = c.finish();
    let gaps = h((gapfield, reach, st.gap_threshold.to_bits()));
    let mut z = std::collections::hash_map::DefaultHasher::new();
    (grid, st.level_jump).hash(&mut z);
    for zone in &i.zones.list {
        (zone.id, zone.level_min, zone.level_max, &zone.name).hash(&mut z);
    }
    let flow = z.finish();
    let mut d = std::collections::hash_map::DefaultHasher::new();
    (grid, reach, flow, st.chunk_budget).hash(&mut d);
    if let Ok(js) = serde_json::to_string(i.gameplay) {
        js.hash(&mut d);
    }
    for r in &i.roads.roads {
        r.name.hash(&mut d);
    }
    let doctor = d.finish();
    Fps {
        grid,
        reach,
        travel,
        gapfield,
        gaps,
        flow,
        doctor,
    }
}

/// Content points the gap analysis measures from: gameplay content, fish schools and (optionally)
/// road centrelines.
pub fn content_sources(
    gp: &Gameplay,
    fish: &[Vec2],
    roads: &[RoadLine],
    st: &AnalysisSettings,
) -> Vec<(Vec2, &'static str)> {
    let mut v: Vec<(Vec2, &'static str)> = Vec::new();
    v.extend(
        gp.pois
            .iter()
            .map(|p| (Vec2::new(p.pos[0], p.pos[2]), "POI")),
    );
    v.extend(
        gp.npcs
            .iter()
            .map(|p| (Vec2::new(p.pos[0], p.pos[2]), "NPC")),
    );
    v.extend(
        gp.nodes
            .iter()
            .map(|p| (Vec2::new(p.pos[0], p.pos[2]), "Node")),
    );
    v.extend(
        gp.spawns
            .iter()
            .filter_map(|s| s.centroid())
            .map(|c| (c, "Spawn")),
    );
    if st.gap_fish {
        v.extend(fish.iter().map(|f| (*f, "Fish")));
    }
    if st.gap_roads {
        for r in roads {
            v.extend(r.pts.iter().step_by(4).map(|p| (p.xz(), "Road")));
        }
    }
    v
}

// ---------------------------------------------------------------- result parts

pub struct Pocket {
    pub pos: Vec3,
    pub area_km2: f32,
}

/// A gameplay item players can't get to.
pub struct ItemReach {
    pub kind: &'static str,
    pub id: u32,
    pub name: String,
    pub pos: Vec3,
    pub why: &'static str,
}

pub struct Reach {
    /// Per cell: bit 1 = reachable from the origin, bit 2 = can walk back to it.
    pub flags: Vec<u8>,
    pub origin: Option<Vec3>,
    pub origin_label: String,
    pub land_km2: f32,
    pub reachable_km2: f32,
    pub unreachable_km2: f32,
    pub trap_km2: f32,
    /// Unreachable walkable areas, largest first.
    pub pockets: Vec<Pocket>,
    /// Areas you can get into but not back out of, largest first.
    pub traps: Vec<Pocket>,
    pub items: Vec<ItemReach>,
}

impl Reach {
    pub fn reachable(&self, k: usize) -> bool {
        self.origin.is_none() || self.flags[k] & 1 != 0
    }
}

pub struct Travel {
    pub times: Vec<f32>,
    pub source: Option<Vec3>,
    pub label: String,
    pub median: f32,
    pub p90: f32,
    pub max: f32,
    pub farthest: Option<Vec3>,
    /// Share of walkable ground reachable, and within 1 / 2 / 5 minutes.
    pub reachable_pct: f32,
    pub within: [f32; 3],
}

pub struct GapField {
    pub times: Vec<f32>,
    pub sources: Vec<(Vec2, &'static str)>,
}

pub struct GapRegion {
    /// The worst spot (longest way to content).
    pub pos: Vec3,
    pub area_km2: f32,
    pub max_s: f32,
}

pub struct Gaps {
    pub threshold: f32,
    pub walk_km2: f32,
    pub dead_km2: f32,
    pub dead_pct: f32,
    pub regions: Vec<GapRegion>,
    pub sources: usize,
}

/// The latest result of every part, each with the fingerprint it was made from.
#[derive(Default, Clone)]
pub struct Parts {
    pub grid: Option<(u64, Arc<WalkGrid>)>,
    pub reach: Option<(u64, Arc<Reach>)>,
    pub travel: Option<(u64, Arc<Travel>)>,
    pub gapfield: Option<(u64, Arc<GapField>)>,
    pub gaps: Option<(u64, Arc<Gaps>)>,
    pub flow: Option<(u64, Arc<flow::FlowResult>)>,
    pub doctor: Option<(u64, Arc<doctor::Report>)>,
}

fn fresh<T>(p: &Option<(u64, Arc<T>)>, fp: u64) -> Option<Arc<T>> {
    p.as_ref().filter(|(f, _)| *f == fp).map(|(_, a)| a.clone())
}

impl Parts {
    /// Which parts are out of date (grid, reach, travel, gapfield + gaps, flow, doctor).
    pub fn stale(&self, fps: &Fps, doctor: bool) -> [bool; 6] {
        [
            fresh(&self.grid, fps.grid).is_none(),
            fresh(&self.reach, fps.reach).is_none(),
            fresh(&self.travel, fps.travel).is_none(),
            fresh(&self.gapfield, fps.gapfield).is_none() || fresh(&self.gaps, fps.gaps).is_none(),
            fresh(&self.flow, fps.flow).is_none(),
            doctor && fresh(&self.doctor, fps.doctor).is_none(),
        ]
    }
}

/// Stage names and weights (of the progress bar).
pub const STAGES: [(&str, f32); 6] = [
    ("Walk grid", 0.2),
    ("Reachability", 0.1),
    ("Travel time", 0.3),
    ("Content gaps", 0.3),
    ("Level flow", 0.05),
    ("World doctor", 0.05),
];

#[derive(Default)]
pub struct Progress {
    /// ‰ per stage.
    pub stage: [AtomicU32; 6],
    pub cancel: AtomicBool,
}

impl Progress {
    /// Overall 0..1 over the stages being computed, and the first unfinished stage.
    pub fn overall(&self, todo: &[bool; 6]) -> (f32, &'static str) {
        let (mut sum, mut w) = (0.0, 0.0);
        let mut label = "Finishing";
        for (k, (name, weight)) in STAGES.iter().enumerate() {
            if !todo[k] {
                continue;
            }
            let p = self.stage[k].load(Ordering::Relaxed).min(1000) as f32 / 1000.0;
            sum += p * weight;
            w += weight;
            if p < 1.0 && label == "Finishing" {
                label = name;
            }
        }
        (if w > 0.0 { sum / w } else { 1.0 }, label)
    }
}

pub struct JobOut {
    pub parts: Parts,
    pub timings: Vec<(&'static str, f32)>,
}

fn ms(t: std::time::Instant) -> f32 {
    t.elapsed().as_secs_f32() * 1000.0
}

/// Computes the stale parts (reusing fresh ones from `prev`). Runs on a worker thread, or on the
/// main thread for MCP calls.
pub fn compute(
    snap: &Snapshot,
    prev: &Parts,
    fps: Fps,
    doctor_wanted: bool,
    prog: &Progress,
) -> JobOut {
    let st = &snap.settings;
    let mut timings = Vec::new();
    let t_all = std::time::Instant::now();
    let mut out = Parts::default();

    let grid = match fresh(&prev.grid, fps.grid) {
        Some(g) => g,
        None => {
            let t = std::time::Instant::now();
            let g = Arc::new(grid::build(
                &snap.field,
                &snap.footprints,
                &snap.roads,
                &snap.zones,
                st,
                &prog.stage[0],
            ));
            timings.push(("grid", ms(t)));
            g
        }
    };
    prog.stage[0].store(1000, Ordering::Relaxed);
    out.grid = Some((fps.grid, grid.clone()));
    let g = &*grid;
    let snap_cell = |p: Vec2| g.nearest_passable(p, 8.0);

    let sources = content_sources(&snap.gameplay, &snap.fish, &snap.roads, st);
    let (mut reach, mut trav, mut gapf, mut flow_r) = (
        fresh(&prev.reach, fps.reach),
        fresh(&prev.travel, fps.travel),
        fresh(&prev.gapfield, fps.gapfield),
        fresh(&prev.flow, fps.flow),
    );
    let mut t_parts = Vec::new();
    std::thread::scope(|sc| {
        let hr = reach.is_none().then(|| {
            sc.spawn(|| {
                let t = std::time::Instant::now();
                let r = compute_reach(snap, g, &prog.stage[1]);
                (r, ms(t))
            })
        });
        let ht = trav.is_none().then(|| {
            sc.spawn(|| {
                let t = std::time::Instant::now();
                let src: Vec<usize> = snap
                    .source
                    .as_ref()
                    .and_then(|s| snap_cell(s.pos))
                    .into_iter()
                    .collect();
                let times = travel::travel_times(g, &src, &prog.stage[2], &prog.cancel);
                let r = travel_stats(snap, g, times, src.first().copied());
                (r, ms(t))
            })
        });
        let hg = gapf.is_none().then(|| {
            sc.spawn(|| {
                let t = std::time::Instant::now();
                let cells: Vec<usize> = sources.iter().filter_map(|(p, _)| snap_cell(*p)).collect();
                let times = travel::travel_times(g, &cells, &prog.stage[3], &prog.cancel);
                (
                    GapField {
                        times,
                        sources: sources.clone(),
                    },
                    ms(t),
                )
            })
        });
        let hf = flow_r.is_none().then(|| {
            sc.spawn(|| {
                let t = std::time::Instant::now();
                let r = flow::compute(g, &snap.zones, st.level_jump);
                prog.stage[4].store(1000, Ordering::Relaxed);
                (r, ms(t))
            })
        });
        if let Some(hr) = hr {
            let (r, t) = hr.join().unwrap();
            reach = Some(Arc::new(r));
            t_parts.push(("reach", t));
        }
        if let Some(ht) = ht {
            let (r, t) = ht.join().unwrap();
            trav = Some(Arc::new(r));
            t_parts.push(("travel", t));
        }
        if let Some(hg) = hg {
            let (r, t) = hg.join().unwrap();
            gapf = Some(Arc::new(r));
            t_parts.push(("gaps", t));
        }
        if let Some(hf) = hf {
            let (r, t) = hf.join().unwrap();
            flow_r = Some(Arc::new(r));
            t_parts.push(("flow", t));
        }
    });
    timings.extend(t_parts);
    for k in 1..5 {
        prog.stage[k].store(1000, Ordering::Relaxed);
    }
    let reach = reach.unwrap();
    let gapf = gapf.unwrap();
    let gaps = match fresh(&prev.gaps, fps.gaps) {
        Some(g) => g,
        None => {
            let t = std::time::Instant::now();
            let r = Arc::new(gap_regions(
                g,
                &reach,
                &gapf,
                st.gap_threshold,
                st.run_speed,
            ));
            timings.push(("gap regions", ms(t)));
            r
        }
    };
    out.reach = Some((fps.reach, reach.clone()));
    out.travel = trav.map(|t| (fps.travel, t));
    out.gapfield = Some((fps.gapfield, gapf));
    out.gaps = Some((fps.gaps, gaps));
    out.flow = flow_r.map(|f| (fps.flow, f));
    out.doctor = prev.doctor.clone();
    if doctor_wanted && fresh(&prev.doctor, fps.doctor).is_none() {
        let t = std::time::Instant::now();
        let r = doctor::run(snap, g, &reach, st.chunk_budget, &prog.stage[5]);
        timings.push(("doctor", ms(t)));
        out.doctor = Some((fps.doctor, Arc::new(r)));
    }
    prog.stage[5].store(1000, Ordering::Relaxed);
    timings.push(("total", ms(t_all)));
    JobOut {
        parts: out,
        timings,
    }
}

fn compute_reach(snap: &Snapshot, g: &WalkGrid, progress: &AtomicU32) -> Reach {
    let mut flags = vec![0u8; g.len()];
    let start = snap
        .origin
        .as_ref()
        .and_then(|o| g.nearest_passable(o.pos, 8.0));
    if let Some(s) = start {
        travel::flood(g, s, true, &mut flags, 1);
        progress.store(450, Ordering::Relaxed);
        travel::flood(g, s, false, &mut flags, 2);
    }
    progress.store(800, Ordering::Relaxed);
    let km2 = g.cell_area_km2() as f32;
    let land = |k: usize| g.class[k].land();
    let land_cells = (0..g.len()).filter(|k| land(*k)).count();
    let has = start.is_some();
    let (pockets, unreach_cells) = if has {
        travel::regions(g.n, |k| land(k) && flags[k] & 1 == 0, 25)
    } else {
        (Vec::new(), 0)
    };
    let (traps, trap_cells) = if has {
        travel::regions(g.n, |k| land(k) && flags[k] == 1, 25)
    } else {
        (Vec::new(), 0)
    };
    let to_pockets = |v: Vec<travel::Region>| {
        v.into_iter()
            .take(40)
            .map(|r| Pocket {
                pos: g.world(r.center),
                area_km2: r.cells as f32 * km2,
            })
            .collect()
    };
    let mut items = Vec::new();
    if has {
        let mut check = |kind: &'static str, id: u32, name: &str, p: Vec2| {
            let pos = Vec3::new(p.x, snap.field.height_at(p.x, p.y), p.y);
            match g.nearest_passable(p, 6.0) {
                None => items.push(ItemReach {
                    kind,
                    id,
                    name: name.to_string(),
                    pos,
                    why: "It stands inside a cliff, deep water or an object.",
                }),
                Some(k) if flags[k] & 1 == 0 => items.push(ItemReach {
                    kind,
                    id,
                    name: name.to_string(),
                    pos,
                    why: "No walkable route leads there from the player start.",
                }),
                _ => {}
            }
        };
        let gp = &snap.gameplay;
        for p in &gp.pois {
            check("POI", p.id, &p.name, Vec2::new(p.pos[0], p.pos[2]));
        }
        for p in &gp.npcs {
            check("NPC", p.id, &p.name, Vec2::new(p.pos[0], p.pos[2]));
        }
        for p in &gp.nodes {
            check("Node", p.id, &p.variant, Vec2::new(p.pos[0], p.pos[2]));
        }
        for s in &gp.spawns {
            // a spawn region is reachable if any part of it is
            let Some(c) = s.centroid() else { continue };
            let ok = s
                .polygon
                .iter()
                .map(|p| Vec2::from_array(*p))
                .chain(std::iter::once(c))
                .any(|p| {
                    g.nearest_passable(p, 6.0)
                        .is_some_and(|k| flags[k] & 1 != 0)
                });
            let inside = ok || {
                let (lo, hi) = s.polygon.iter().fold(
                    (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
                    |(lo, hi), p| (lo.min(Vec2::from_array(*p)), hi.max(Vec2::from_array(*p))),
                );
                let step = ((hi - lo).max_element() / 24.0).max(g.cell);
                let mut found = false;
                let mut z = lo.y;
                while z <= hi.y && !found {
                    let mut x = lo.x;
                    while x <= hi.x && !found {
                        let p = Vec2::new(x, z);
                        found = s.contains(p) && g.cell_at(p).is_some_and(|k| flags[k] & 1 != 0);
                        x += step;
                    }
                    z += step;
                }
                found
            };
            if !inside {
                items.push(ItemReach {
                    kind: "Spawn",
                    id: s.id,
                    name: s.name.clone(),
                    pos: Vec3::new(c.x, snap.field.height_at(c.x, c.y), c.y),
                    why: "No part of the region can be reached on foot.",
                });
            }
        }
    }
    progress.store(1000, Ordering::Relaxed);
    let reachable = (0..g.len())
        .filter(|k| land(*k) && flags[*k] & 1 != 0)
        .count();
    Reach {
        origin: start.map(|s| g.world(s)),
        origin_label: snap
            .origin
            .as_ref()
            .map(|o| o.label.clone())
            .unwrap_or_else(|| "nowhere".into()),
        land_km2: land_cells as f32 * km2,
        reachable_km2: if has {
            reachable as f32 * km2
        } else {
            land_cells as f32 * km2
        },
        unreachable_km2: unreach_cells as f32 * km2,
        trap_km2: trap_cells as f32 * km2,
        pockets: to_pockets(pockets),
        traps: to_pockets(traps),
        items,
        flags,
    }
}

fn travel_stats(snap: &Snapshot, g: &WalkGrid, times: Vec<f32>, src: Option<usize>) -> Travel {
    let land = |k: usize| g.class[k].land();
    let land_cells = (0..g.len()).filter(|k| land(*k)).count().max(1);
    let mut reach = 0usize;
    let mut within = [0usize; 3];
    let mut far = (0.0f32, None);
    for k in 0..g.len() {
        let t = times[k];
        if !land(k) || !t.is_finite() {
            continue;
        }
        reach += 1;
        for (w, lim) in within.iter_mut().zip([60.0, 120.0, 300.0]) {
            if t <= lim {
                *w += 1;
            }
        }
        if t > far.0 {
            far = (t, Some(k));
        }
    }
    Travel {
        median: travel::percentile(&times, land, 0.5).unwrap_or(0.0),
        p90: travel::percentile(&times, land, 0.9).unwrap_or(0.0),
        max: far.0,
        farthest: far.1.map(|k| g.world(k)),
        reachable_pct: reach as f32 * 100.0 / land_cells as f32,
        within: within.map(|w| w as f32 * 100.0 / land_cells as f32),
        source: src.map(|s| g.world(s)),
        label: snap
            .source
            .as_ref()
            .map(|s| s.label.clone())
            .unwrap_or_default(),
        times,
    }
}

/// Dead zones, split into gaps around their worst spots: the farthest point from any content is
/// the first gap's heart, then the farthest point at least a content spacing away from it, and so
/// on; every dead cell belongs to the nearest heart. Each gap is a place that wants new content.
fn gap_regions(g: &WalkGrid, reach: &Reach, f: &GapField, threshold: f32, run_speed: f32) -> Gaps {
    let km2 = g.cell_area_km2() as f32;
    // the walkable ground players can actually get to
    let walk = |k: usize| g.class[k].land() && reach.reachable(k);
    let dead: Vec<u32> = (0..g.len())
        .filter(|k| walk(*k) && f.times[*k] > threshold)
        .map(|k| k as u32)
        .collect();
    let walk_cells = (0..g.len()).filter(|k| walk(*k)).count();
    let time = |k: u32| {
        if f.times[k as usize].is_finite() {
            f.times[k as usize]
        } else {
            1.0e9
        }
    };
    // what one piece of content covers: the threshold walked, a little more
    let spacing = (threshold * run_speed * 1.5).max(150.0);
    let mut hearts: Vec<(u32, Vec2)> = Vec::new();
    let mut taken = vec![false; dead.len()];
    while hearts.len() < 12 {
        // one pass: drop cells near the newest heart, find the farthest of the rest
        let last = hearts.last().map(|h| h.1);
        let mut best: Option<(f32, u32)> = None;
        for (i, &k) in dead.iter().enumerate() {
            if taken[i] {
                continue;
            }
            if last.is_some_and(|h| g.center(k as usize).distance_squared(h) <= spacing * spacing) {
                taken[i] = true;
                continue;
            }
            let t = time(k);
            if best.is_none_or(|b| t > b.0) {
                best = Some((t, k));
            }
        }
        let Some((_, b)) = best else { break };
        hearts.push((b, g.center(b as usize)));
    }
    let mut area = vec![0usize; hearts.len()];
    for &k in &dead {
        let c = g.center(k as usize);
        if let Some((i, _)) = hearts.iter().enumerate().min_by(|a, b| {
            a.1.1
                .distance_squared(c)
                .total_cmp(&b.1.1.distance_squared(c))
        }) {
            area[i] += 1;
        }
    }
    let mut regions: Vec<GapRegion> = hearts
        .iter()
        .zip(&area)
        .filter(|(_, a)| **a >= 400)
        .map(|((k, _), a)| {
            let t = f.times[*k as usize];
            GapRegion {
                pos: g.world(*k as usize),
                area_km2: *a as f32 * km2,
                max_s: t,
            }
        })
        .collect();
    regions.sort_by(|a, b| b.area_km2.total_cmp(&a.area_km2));
    Gaps {
        threshold,
        walk_km2: walk_cells as f32 * km2,
        dead_km2: dead.len() as f32 * km2,
        dead_pct: if walk_cells > 0 {
            dead.len() as f32 * 100.0 / walk_cells as f32
        } else {
            0.0
        },
        regions,
        sources: f.sources.len(),
    }
}
