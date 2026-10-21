//! Outdoor dungeons from design sheets: a top-down level-design sheet (painted by an image model
//! from a brief, `sheet_prompt.md`) is read into a `Blueprint` (`read`, `read_prompt.md`), and
//! the blueprint is built into a realm: the land shaped to its zones, paths, stairs, water and
//! chasms (`land`), the floors painted, and a marker standing wherever the sheet's object key
//! puts something (`markers`), filled from the installed packs where a model fits. The sheet
//! stays with the dungeon (`<map>/sheet/`) as its design reference.
//!
//! - `blueprint`: the format; `read`: sheet → blueprint (the AI's reading, measured and
//!   repaired); `library`: the bundled sheets and the world's own; `family`: a setting's sky,
//!   biome and kit; `land`: the shaping and paint; `markers`; `furnish`: the build's steps;
//!   `generate`: a new sheet painted and read for a world (the user's AI); `mcp`: the tools.
//!
//! A dungeon made from a sheet is a realm whose `DungeonDef::sheet` names it. Its land is made
//! when it is first opened (`session::create_map` calls `begin` once the land is made and `end`
//! once the biomes have grown, as for a temple compound); the build then runs over the next
//! frames (`drive`) and the map is saved when it's done.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde_json::{Value, json};

pub mod blueprint;
pub mod bridges;
pub mod family;
pub mod furnish;
pub mod generate;
pub mod land;
pub mod library;
pub mod markers;
pub(crate) mod mcp;
pub mod read;
pub mod ui;

pub use blueprint::Blueprint;
pub use library::SheetRef;

use super::compound::furnish::Step;
use super::model::DungeonDef;
use crate::io::WorldData;

/// The realm layout id of a dungeon made from a sheet.
pub const LAYOUT: &str = "design_sheet";
/// At most this many markers in one realm.
const MAX_MARKERS: usize = 220;

/// The realm preset a sheet realm takes its sky, biomes and music from (None: not a sheet).
pub fn realm_preset(def: &DungeonDef) -> Option<&'static super::presets::RealmPreset> {
    let r = def.sheet.as_ref()?;
    let fam = if r.family.is_empty() {
        library::entry(&r.id)
            .map(|e| e.family.as_str())
            .unwrap_or("temperate")
    } else {
        r.family.as_str()
    };
    Some(family::preset(if family::FAMILIES.contains(&fam) {
        fam
    } else {
        "temperate"
    }))
}

/// (climate, the one biome) a sheet realm's biomes grow from.
pub fn biomes(
    def: &DungeonDef,
) -> Option<(crate::biomegen::Climate, Option<crate::biomes::BiomePreset>)> {
    realm_preset(def).map(|p| (p.climate, p.only))
}

/// A sheet realm being made: its blueprint laid on the land.
pub struct Plan {
    pub bp: Blueprint,
    pub site: land::Site,
}

struct Pending {
    map: u32,
    steps: Vec<Step>,
    markers: Vec<markers::Marker>,
    hour: f32,
    summary: Value,
    bridges: bridges::State,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
/// The build's progress for the panel: (done, total, last step, finished).
static PROGRESS: Mutex<Option<(usize, usize, String, bool)>> = Mutex::new(None);

/// The open sheet realm's build progress (None: no build this session).
pub fn progress() -> Option<(usize, usize, String, bool)> {
    PROGRESS.lock().ok().and_then(|p| p.clone())
}

/// Bridge source ids prepared during `end`, before `session::create_map` creates its `MapDoc`.
/// The frame builder still owns its copy in `Pending` and writes it again when it starts.
pub fn take_pending_state() -> bridges::State {
    PENDING
        .lock()
        .ok()
        .and_then(|p| p.as_ref().map(|p| p.bridges.clone()))
        .unwrap_or_default()
}

/// A new sheet realm's land has just been made: lay the blueprint on it and shape the land (before
/// the biomes). The sheet's picture and blueprint are copied into the map's folder. None when it
/// isn't a sheet realm or its sheet can't be built (the realm is then made as an ordinary one).
pub fn begin(d: &mut WorldData, def: &DungeonDef, seed: u32, dir: &Path) -> Option<Plan> {
    let r = def.sheet.as_ref()?;
    let world = dir.parent().and_then(|p| p.parent());
    let (bp, pic) = match library::load(r, world) {
        Ok(x) => x,
        Err(e) => {
            warn!("dungeon sheet: {e}");
            return None;
        }
    };
    let bad = bp.problems();
    if !bad.is_empty() {
        warn!(
            "dungeon sheet `{}` can't be built: {}",
            r.id,
            bad.join("; ")
        );
        return None;
    }
    // (the design reference kept with the map)
    let keep = dir.join("sheet");
    let _ = std::fs::create_dir_all(&keep);
    if let Some(p) = &pic {
        let _ = std::fs::copy(p, keep.join(p.file_name().unwrap_or_default()));
    }
    let _ = std::fs::write(
        keep.join("blueprint.json"),
        serde_json::to_string_pretty(&bp).unwrap_or_default(),
    );
    let t0 = Instant::now();
    let (n, cell) = (d.terrain.n(), d.terrain.cell);
    let half = Vec2::new(bp.size[0], bp.size[1]) * 0.5;
    // (an island map's sea is the realm's own ocean)
    let sea = bp
        .water
        .iter()
        .filter(|wt| land::is_sea(&bp, wt))
        .map(|wt| wt.level)
        .reduce(f32::min);
    let y0 = match sea {
        Some(level) => land::floor_for_sea(level, d.terrain.sea_level),
        None => land::floor_level(&d.terrain.heights, n, cell, half, d.terrain.sea_level),
    };
    let mut site = land::Site::new(&bp, y0, seed);
    // (a sea's shape is read from the sheet's picture)
    if site.sea.is_some() {
        if let (Some(pf), Some(img)) = (
            bp.picture.as_ref(),
            pic.as_ref().and_then(|p| image::open(p).ok()),
        ) {
            site.water_mask = land::WaterMask::from_picture(&img.to_rgba8(), pf, bp.size);
        }
    }
    land::carve(&mut d.terrain.heights, n, cell, &site);
    d.terrain.mark_all_dirty();
    d.terrain.height_revision += 1;
    info!(
        "dungeon sheet `{}`: {} zones, {} links, land shaped in {:.0} ms",
        bp.id,
        site.zones.len(),
        site.links.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    Some(Plan { bp, site })
}

/// The biomes have grown: the water, the paint, the nature, where players arrive, the markers;
/// then the build is queued to run over the next frames.
pub fn end(d: &mut WorldData, def: &DungeonDef, plan: Option<Plan>) -> bool {
    let Some(Plan { bp, site }) = plan else {
        return false;
    };
    let fam = if family::FAMILIES.contains(&bp.family.as_str()) {
        bp.family.clone()
    } else {
        "temperate".into()
    };
    let theme = family::theme(&fam);
    for (layer, id) in theme.layer_choices() {
        d.layer_choices.set(*layer as usize, id);
    }
    // the water: lakes and rivers at the sheet's levels (a sea is the realm's own ocean)
    let shape = crate::water::lakes::shape_by_id(family::lake(&fam));
    for (i, (outline, level)) in site.water.iter().enumerate() {
        if bp.water.get(i).is_some_and(|wt| land::is_sea(&bp, wt)) {
            continue;
        }
        let res = {
            let WorldData {
                terrain,
                history,
                store,
                water,
                lib,
                ..
            } = &mut *d;
            crate::water::create_drawn_lake_at_level(
                terrain,
                history,
                store,
                lib.as_deref(),
                water,
                outline,
                shape,
                1.0,
                0.8,
                Some(*level),
            )
        };
        match res {
            Ok(id) => {
                let name = bp.water.get(i).map(|w| w.name.clone()).unwrap_or_default();
                if let Some(l) = d.water.lakes.iter_mut().find(|l| l.id == id) {
                    if !name.is_empty() {
                        l.name = name;
                    }
                }
                d.water.dirty = true;
            }
            Err(e) => warn!("dungeon sheet water: {e}"),
        }
    }
    land::paint(&mut d.terrain, &site, family::paint(&fam));
    // The final water bodies are in place now.  These are authored roads with raised decks,
    // deliberately installed without the road builder so the sheet terrain and paint stay put.
    let bridges = bridges::install_new(&mut d.roads, &bp, &site);
    // the nature: off the walkable ground, thick away from the route
    let half = d.terrain.half();
    d.nature
        .set_texels(Vec2::splat(-half), Vec2::splat(half), |p| {
            let (clear, thick) = land::clearing(&site, p);
            let keep = 1.0 - clear;
            let trees = keep * (0.9 + 2.6 * thick);
            Some((
                [
                    trees,
                    keep * (1.0 + 0.4 * thick),
                    keep * (1.0 + 0.8 * thick),
                    keep,
                ],
                0,
            ))
        });
    // players arrive at the sheet's start, facing into the dungeon
    let s = land::w(bp.start);
    let f = land::w(bp.start_dir).normalize_or(Vec2::NEG_Y);
    let y = d.terrain.height_at(s.x, s.y);
    d.gameplay.player_start = Some(crate::gameplay::PlayerStart {
        pos: [s.x, y, s.y],
        yaw: (-f.y).atan2(f.x),
    });
    d.gameplay.dirty = true;
    // the view: the whole map from the south, a little high
    d.orbit.set_view(
        Vec3::new(0.0, site.y0, 0.0),
        std::f32::consts::PI,
        0.85,
        (bp.size[0].max(bp.size[1]) * 1.15).max(260.0),
    );
    d.orbit.snap();
    // the markers (and the models that fill them), the build's steps
    let mut ms = markers::expand(&bp, &site, MAX_MARKERS);
    let fill = furnish::auto_fill(&mut ms, def.art == super::styles::Art::HandPainted);
    let steps = furnish::steps(&bp, &site, theme, fill, def.levels);
    let summary = json!({ "sheet": bp.summary(), "markers": ms.len(), "filled": ms.iter().filter(|m| m.filled.is_some()).count(), "floor": site.y0 });
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending {
        map: def.id,
        steps,
        markers: ms,
        hour: family::hour(&bp.art.light, &fam),
        summary,
        bridges,
    });
    true
}

// ------------------------------------------------------------------ the build over frames

/// The sheet realm being built in the open map, and the last one's summary.
#[derive(Resource, Default)]
pub struct SheetRun {
    queue: VecDeque<Step>,
    wait: u32,
    map: Option<u32>,
    pub done: usize,
    pub total: usize,
    pub log: Vec<String>,
    pub skipped: Vec<String>,
    started: Option<Instant>,
    made: usize,
    /// the sheet's hour of day (set again when the build ends)
    hour: Option<f32>,
    pub summary: Option<Value>,
    pub finished: bool,
}

impl SheetRun {
    pub fn running(&self) -> bool {
        self.map.is_some() && !self.finished
    }
}

pub fn running(world: &World) -> bool {
    world
        .get_resource::<SheetRun>()
        .is_some_and(|r| r.running())
        || PENDING.lock().map(|p| p.is_some()).unwrap_or(false)
}

fn start(world: &mut World, p: Pending) {
    if let Some(doc) = world.resource_mut::<super::DungeonMap>().0.as_mut() {
        doc.markers = p.markers;
        doc.sheet_bridges = p.bridges;
    }
    world.resource_mut::<crate::sky::Lighting>().hour = p.hour;
    let mut run = SheetRun {
        total: p.steps.len(),
        queue: p.steps.into(),
        map: Some(p.map),
        started: Some(Instant::now()),
        summary: Some(p.summary),
        hour: Some(p.hour),
        ..Default::default()
    };
    run.log.push("Building the dungeon from its sheet".into());
    *world.resource_mut::<SheetRun>() = run;
}

/// Runs the build's next steps (about 15 ms a frame).
pub fn drive(world: &mut World) {
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(p) = pending {
        start(world, p);
    }
    if !world.resource::<SheetRun>().running() {
        return;
    }
    let open = world
        .resource::<super::DungeonMap>()
        .0
        .as_ref()
        .map(|m| m.id);
    if open != world.resource::<SheetRun>().map {
        let mut r = world.resource_mut::<SheetRun>();
        r.queue.clear();
        r.finished = true;
        r.log.push("Stopped: its map was closed".into());
        return;
    }
    let mut run = std::mem::take(&mut *world.resource_mut::<SheetRun>());
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(15) {
        if run.wait > 0 {
            run.wait -= 1;
            break;
        }
        let Some(step) = run.queue.pop_front() else {
            break;
        };
        run.done += 1;
        match step {
            Step::Wait(n) => run.wait = n,
            Step::Call { label, tool, args } => {
                let count = if tool == "objects_place" {
                    args.get("items")
                        .and_then(|v| v.as_array())
                        .map_or(0, |a| a.len())
                } else {
                    0
                };
                match crate::mcp::call_tool(world, tool, args) {
                    Ok(_) => {
                        run.made += count;
                        run.log.push(label);
                    }
                    Err(e) => {
                        let e = e.lines().next().unwrap_or_default().to_string();
                        warn!("dungeon sheet build skipped {label}: {e}");
                        run.skipped.push(format!("{label}: {e}"));
                    }
                }
                if t0.elapsed() > Duration::from_millis(6) {
                    break;
                }
            }
        }
    }
    if run.queue.is_empty() && run.wait == 0 {
        finish(world, &mut run);
    }
    if let Ok(mut p) = PROGRESS.lock() {
        *p = Some((
            run.done,
            run.total,
            run.log.last().cloned().unwrap_or_default(),
            run.finished,
        ));
    }
    *world.resource_mut::<SheetRun>() = run;
}

fn finish(world: &mut World, run: &mut SheetRun) {
    run.finished = true;
    if let Some(h) = run.hour {
        world.resource_mut::<crate::sky::Lighting>().hour = h;
    }
    let secs = run.started.map_or(0.0, |s| s.elapsed().as_secs_f32());
    let mut s = run.summary.clone().unwrap_or_else(|| json!({}));
    s["built"] = json!({ "objects": run.made, "skipped": run.skipped, "seconds": (secs * 10.0).round() / 10.0 });
    run.log.push(format!(
        "The dungeon is ready ({} objects, {:.0} s)",
        run.made, secs
    ));
    info!(
        "dungeon sheet built: {} objects, {} skipped, {:.1} s",
        run.made,
        run.skipped.len(),
        secs
    );
    if let Err(e) = world
        .run_system_cached_with(save_map, s.clone())
        .unwrap_or_else(|e| Err(e.to_string()))
    {
        warn!("dungeon sheet: couldn't save its map: {e}");
    }
    run.summary = Some(s);
}

fn save_map(In(summary): In<Value>, mut d: WorldData) -> Result<(), String> {
    d.history.clear();
    let Some(dir) = d.meta.path.clone() else {
        return Ok(());
    };
    if d.dungeon_map.0.is_none() {
        return Ok(());
    }
    crate::io::save_world(&dir, &d)?;
    d.meta.dirty = false;
    let _ = std::fs::write(
        dir.join("sheet").join("build.json"),
        serde_json::to_string_pretty(&summary).unwrap_or_default(),
    );
    Ok(())
}

/// The open map's sheet folder (its picture and blueprint), when it was made from a sheet.
pub fn open_sheet_dir(world: &World) -> Option<std::path::PathBuf> {
    world.resource::<super::DungeonMap>().0.as_ref()?;
    let dir = world
        .resource::<crate::world::WorldMeta>()
        .path
        .clone()?
        .join("sheet");
    dir.join("blueprint.json").exists().then_some(dir)
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<SheetRun>()
        .init_resource::<ui::SheetUi>();
    app.add_systems(
        Update,
        (drive, ui::run_requests).after(crate::tools::ToolSystems::Commit),
    );
    markers::register(app);
    generate::register(app);
    mcp::register(app);
}
