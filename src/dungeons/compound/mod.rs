//! Temple compounds: outdoor raid-style dungeons laid out by algorithm, in a realm of their own.
//!
//! The recipe (the great outdoor temple cities of MMO raids): an enclosed basin, cliffs on three
//! sides and a perimeter wall along the open fourth; the entrance plaza at one end, behind its
//! gate, stairs and curved wall, where the way out stands; the final court at the far end,
//! reached by a straight ceremonial axis (a gate, a round plaza, a throne terrace); a central
//! jungle hub with winding trails out to the boss courts, each a raised platform of its own
//! shape (a cross-shaped yard, a pillared hall, a stepped pyramid, a round shrine, an octagonal
//! arena) with its stairs toward its trail and a floor of its own colour; lakes in the low
//! ground between them and a temple on an island reached by a bridge; broken walls, pillars and
//! tree clumps scattered between, trash camps along the trails, a boss arena per court with
//! levels rising, and the final boss at the far end. No prompt needed: the theme, the number of
//! courts, the order, the size and the seed make it, never the same twice.
//!
//! - `theme`: the five looks and what each is built from; `layout`: where everything goes;
//!   `shape`: the land shaped for it and its lakes, paint and nature; `furnish`: the editor's own
//!   tools that build the rest, a step at a time; `picker`: the create flow's options and the
//!   live preview of the layout.
//!
//! A compound is made when its realm is first opened (`session::create_map` calls `begin` once
//! the land is made and `end` once the biomes have grown); the build then runs over the next
//! frames (`drive`) and the map is saved when it's done. `dungeons_compound` reports it.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod furnish;
pub mod layout;
pub mod picker;
pub mod shape;
pub mod theme;

pub use theme::{Order, Theme};

use super::model::DungeonDef;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};
use furnish::Step;
use layout::Plan;

/// The realm id of a temple compound (`presets::realms`).
pub const LAYOUT: &str = "temple_compound";

/// What a compound is made from (kept with its dungeon until its realm is first opened).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Options {
    #[serde(default)]
    pub theme: Theme,
    /// boss courts besides the final one (3–6)
    #[serde(default = "four")]
    pub courts: u8,
    #[serde(default)]
    pub order: Order,
}

fn four() -> u8 {
    4
}

impl Default for Options {
    fn default() -> Self {
        Options {
            theme: Theme::Jungle,
            courts: 4,
            order: Order::Hub,
        }
    }
}

impl Options {
    pub fn clamped(self) -> Self {
        Options {
            courts: self.courts.clamp(3, 6),
            ..self
        }
    }
    /// From MCP: `{theme, courts, order}` (any may be left out).
    pub fn from_json(v: &Value) -> Result<Options, String> {
        let mut o = Options::default();
        if let Some(t) = v.get("theme").and_then(|x| x.as_str()) {
            o.theme = Theme::from_id(t).ok_or_else(|| {
                format!(
                    "unknown compound theme `{t}`: {}",
                    Theme::ALL.map(|t| t.id()).join(", ")
                )
            })?;
        }
        if let Some(n) = v.get("courts").and_then(|x| x.as_u64()) {
            o.courts = n.clamp(3, 6) as u8;
        }
        if let Some(s) = v.get("order").and_then(|x| x.as_str()) {
            o.order = Order::from_id(s)
                .ok_or_else(|| format!("unknown compound order `{s}`: hub or linear"))?;
        }
        Ok(o)
    }
    pub fn json(&self) -> Value {
        json!({ "theme": self.theme.id(), "courts": self.courts, "order": self.order.id() })
    }
}

/// The JSON schema of a compound's options (dungeons_create, the preview).
pub fn options_schema() -> Value {
    json!({ "type": "object", "description": "a temple compound (an outdoor raid-style dungeon laid out for you): `theme`, how many boss `courts` besides the final one (3–6, default 4), `order` (hub: trails from a central hub to every court, the final court gated; linear: one court after another). Implies kind realm, layout temple_compound.", "properties": {
        "theme": { "type": "string", "enum": Theme::ALL.map(|t| t.id()), "description": "jungle temple city (default), desert necropolis, frozen citadel, ember forge-temple, twilight elven ruins" },
        "courts": { "type": "integer", "minimum": 3, "maximum": 6 },
        "order": { "type": "string", "enum": Order::ALL.map(|o| o.id()) },
    }})
}

/// The realm preset a compound's map takes its sky and music from (None: not a compound).
pub fn realm_preset(def: &DungeonDef) -> Option<&'static super::presets::RealmPreset> {
    let o = def.compound?;
    Some(theme_preset(o.theme))
}

/// A theme's realm: its realm's own (its music, its land's colours) under a clearer sky, so the
/// compound reads from its walls (the jungle's mist and the twilight's haze thinned).
fn theme_preset(t: Theme) -> &'static super::presets::RealmPreset {
    use super::presets::RealmPreset;
    use crate::biomegen::Climate;
    use crate::biomes::BiomePreset as B;
    use crate::look::LookPreset as L;
    use crate::terrain::worldgen::WorldPreset as W;
    use egui_phosphor::regular as icon;
    static P: [RealmPreset; 5] = [
        RealmPreset {
            id: "jungle_realm",
            name: "Jungle temple city",
            blurb: "",
            world: W::Island,
            climate: Climate::Tropical,
            look: L::EpicSunbeams,
            hour: 10.5,
            only: Some(B::Jungle),
            icon: icon::TREE_PALM,
        },
        RealmPreset {
            id: "desert_realm",
            name: "Desert necropolis",
            blurb: "",
            world: W::Island,
            climate: Climate::Arid,
            look: L::GoldenHour,
            hour: 16.0,
            only: Some(B::Desert),
            icon: icon::SUN_HORIZON,
        },
        RealmPreset {
            id: "frozen_realm",
            name: "Frozen citadel",
            blurb: "",
            world: W::Island,
            climate: Climate::Cold,
            look: L::Nordic,
            hour: 11.5,
            only: Some(B::Tundra),
            icon: icon::SNOWFLAKE,
        },
        RealmPreset {
            id: "ember_realm",
            name: "Ember forge-temple",
            blurb: "",
            world: W::Island,
            climate: Climate::Arid,
            look: L::Hellfire,
            hour: 16.6,
            only: Some(B::Volcanic),
            icon: icon::FIRE,
        },
        RealmPreset {
            id: "twilight_realm",
            name: "Twilight elven ruins",
            blurb: "",
            world: W::Island,
            climate: Climate::Temperate,
            look: L::Lavender,
            hour: 16.6,
            only: Some(B::AutumnWoods),
            icon: icon::MOON_STARS,
        },
    ];
    &P[Theme::ALL.iter().position(|x| *x == t).unwrap_or(0)]
}

/// (climate, the one biome) a compound's biomes grow from.
pub fn biomes(
    def: &DungeonDef,
) -> Option<(crate::biomegen::Climate, Option<crate::biomes::BiomePreset>)> {
    let o = def.compound?;
    let (c, b, _, _) = o.theme.sky();
    Some((c, b))
}

/// The plan of a compound whose map is being made, waiting for its build to start.
static PENDING: Mutex<Option<(u32, Plan)>> = Mutex::new(None);

/// A new compound realm's land has just been made: lay the compound out on it and shape the land
/// for it (before the biomes). None when it isn't a compound or there is no floor for one.
pub fn begin(d: &mut WorldData, def: &DungeonDef, seed: u32) -> Option<Plan> {
    let o = def.compound?.clamped();
    let t0 = Instant::now();
    let ask = layout::Ask {
        theme: o.theme,
        courts: o.courts as usize,
        order: o.order,
        seed,
        levels: def.levels,
    };
    let plan = {
        let g = layout::Grid {
            n: d.terrain.n(),
            cell: d.terrain.cell,
            h: &d.terrain.heights,
        };
        layout::lay_out(&g, &ask)
    };
    let plan = match plan {
        Ok(p) => p,
        Err(e) => {
            warn!("temple compound: {e}");
            return None;
        }
    };
    let (n, cell) = (d.terrain.n(), d.terrain.cell);
    shape::carve(&mut d.terrain.heights, n, cell, &plan);
    d.terrain.mark_all_dirty();
    d.terrain.height_revision += 1;
    info!(
        "temple compound: {} courts ({} dropped), {} lakes, {} trails, laid out in {:.0} ms",
        plan.courts.len(),
        plan.dropped,
        plan.lakes.len(),
        plan.trails.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    Some(plan)
}

/// The biomes have grown: the lakes, the island, the paint, the nature and where players arrive;
/// then the build is queued to run over the next frames.
pub fn end(d: &mut WorldData, def: &DungeonDef, plan: Option<Plan>) -> bool {
    let Some(mut plan) = plan else { return false };
    shape::finish(d, &mut plan);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some((def.id, plan));
    true
}

// ------------------------------------------------------------------ the build over frames

/// The compound being built in the open map, and the last one's summary.
#[derive(Resource, Default)]
pub struct CompoundRun {
    queue: VecDeque<Step>,
    wait: u32,
    map: Option<u32>,
    pub plan: Option<Plan>,
    pub done: usize,
    pub total: usize,
    pub log: Vec<String>,
    pub skipped: Vec<String>,
    started: Option<Instant>,
    /// objects, roads, gameplay items made
    made: [usize; 3],
    /// the finished build's summary (also saved as `compound.json` in the map's folder)
    pub summary: Option<Value>,
    pub finished: bool,
}

impl CompoundRun {
    pub fn running(&self) -> bool {
        self.map.is_some() && !self.finished
    }
    /// The dungeon whose compound this is.
    pub fn map_id(&self) -> Option<u32> {
        self.map
    }
}

/// Whether a compound is being built.
pub fn running(world: &World) -> bool {
    world
        .get_resource::<CompoundRun>()
        .is_some_and(|r| r.running())
        || PENDING.lock().map(|p| p.is_some()).unwrap_or(false)
}

fn start(world: &mut World, map: u32, plan: Plan) {
    let steps = furnish::steps(&plan);
    let mut run = CompoundRun {
        total: steps.len(),
        queue: steps.into(),
        map: Some(map),
        started: Some(Instant::now()),
        ..Default::default()
    };
    run.log.push(format!(
        "Building the compound: {} courts, the final court, {} lakes, {} trails",
        plan.courts.len(),
        plan.lakes.len(),
        plan.trails.len()
    ));
    run.plan = Some(plan);
    *world.resource_mut::<CompoundRun>() = run;
}

/// Runs the build's next steps (about 15 ms a frame).
pub fn drive(world: &mut World) {
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((map, plan)) = pending {
        start(world, map, plan);
    }
    if !world.resource::<CompoundRun>().running() {
        return;
    }
    // (the map was closed: the rest is dropped)
    let open = world
        .resource::<super::DungeonMap>()
        .0
        .as_ref()
        .map(|m| m.id);
    if open != world.resource::<CompoundRun>().map {
        let mut r = world.resource_mut::<CompoundRun>();
        r.queue.clear();
        r.finished = true;
        r.log.push("Stopped: its map was closed".into());
        return;
    }
    let mut run = std::mem::take(&mut *world.resource_mut::<CompoundRun>());
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
                let count = match tool {
                    "objects_place" => args
                        .get("items")
                        .and_then(|v| v.as_array())
                        .map_or(0, |a| a.len()),
                    _ => 1,
                };
                match crate::mcp::call_tool(world, tool, args) {
                    Ok(_) => {
                        let slot = match tool {
                            "objects_place" => 0,
                            "roads_create" => 1,
                            _ => 2,
                        };
                        run.made[slot] += count;
                        run.log.push(label);
                    }
                    Err(e) => {
                        let e = e.lines().next().unwrap_or_default().to_string();
                        warn!("temple compound skipped {label}: {e}");
                        run.skipped.push(format!("{label}: {e}"));
                    }
                }
                // (a heavy step gets a frame to itself)
                if t0.elapsed() > Duration::from_millis(6) {
                    break;
                }
            }
        }
    }
    if run.queue.is_empty() && run.wait == 0 {
        finish(world, &mut run);
    }
    *world.resource_mut::<CompoundRun>() = run;
}

/// The build is done: the summary, the map saved with it, its history starting fresh.
fn finish(world: &mut World, run: &mut CompoundRun) {
    run.finished = true;
    let secs = run.started.map_or(0.0, |s| s.elapsed().as_secs_f32());
    let Some(plan) = run.plan.as_ref() else {
        return;
    };
    let mut s = plan.summary();
    s["built"] = json!({ "objects": run.made[0], "roads": run.made[1], "gameplay": run.made[2], "skipped": run.skipped, "seconds": (secs * 10.0).round() / 10.0 });
    run.log.push(format!(
        "The compound is ready ({} objects, {} roads, {:.0} s)",
        run.made[0], run.made[1], secs
    ));
    info!(
        "temple compound built: {} objects, {} roads, {} gameplay items, {} skipped, {:.1} s",
        run.made[0],
        run.made[1],
        run.made[2],
        run.skipped.len(),
        secs
    );
    let saved = world
        .run_system_cached_with(save_map, s.clone())
        .unwrap_or_else(|e| Err(e.to_string()));
    if let Err(e) = saved {
        warn!("temple compound: couldn't save its map: {e}");
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
        dir.join("compound.json"),
        serde_json::to_string_pretty(&summary).unwrap_or_default(),
    );
    Ok(())
}

// ------------------------------------------------------------------ MCP

fn status_json(world: &World) -> Value {
    let r = world.resource::<CompoundRun>();
    let open = world
        .resource::<super::DungeonMap>()
        .0
        .as_ref()
        .map(|m| m.id);
    let saved = || {
        let dir = world.resource::<crate::world::WorldMeta>().path.clone()?;
        serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join("compound.json")).ok()?)
            .ok()
    };
    if r.running() {
        return json!({ "building": true, "step": r.done, "steps": r.total, "log": r.log.iter().rev().take(6).rev().collect::<Vec<_>>(), "plan": r.plan.as_ref().map(|p| p.summary()) });
    }
    match (&r.summary, r.map == open && open.is_some()) {
        (Some(s), true) => json!({ "building": false, "summary": s }),
        _ => match open.and_then(|_| saved()) {
            Some(s) => json!({ "building": false, "summary": s }),
            None => {
                json!({ "building": false, "summary": Value::Null, "note": "the open map isn't a temple compound (dungeons_create with `compound`, then dungeons_open)" })
            }
        },
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<CompoundRun>();
    app.add_systems(Update, drive.after(crate::tools::ToolSystems::Commit));
    app.mcp_tool(McpTool {
        name: "dungeons_compound",
        description: "The temple compound of the open realm: while it is being built (it builds over a few seconds after dungeons_open first makes the map) its progress; once done, what was made — the entrance plaza and the perimeter wall, the hub, the processional axis (gate, round plaza, final court), every boss court (name, shape, boss, level, position, platform height, which way its stairs face, the island it stands on), the lakes, the trails and bridges, the camps, and how many objects, roads and gameplay items were placed. `wait` true waits for the build to finish (up to `seconds`, default 60).",
        input_schema: || json!({ "type": "object", "properties": { "wait": { "type": "boolean" }, "seconds": { "type": "number" } } }),
        run: |w, a| {
            let wait: bool = opt_arg(&a, "wait")?.unwrap_or(false);
            if !wait || !running(w) {
                return Ok(vec![McpContent::Json(status_json(w))]);
            }
            let secs: f32 = opt_arg(&a, "seconds")?.unwrap_or(60.0);
            let start = w.resource::<Time<Real>>().elapsed_secs();
            Ok(vec![McpContent::deferred(move |w| {
                let now = w.resource::<Time<Real>>().elapsed_secs();
                (!running(w) || now - start > secs).then(|| Ok(vec![McpContent::Json(status_json(w))]))
            })])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_compound_preview",
        description: "Lay a temple compound out without making it: a preview map (PNG) of the land (`realm_size`, `seed`: the very land dungeons_create makes from them, so the layout is the one it gets) with the compound drawn on it — the entrance plaza and its wall, the hub, the trails, the lakes and the island, every court's platform, the processional axis and the final court — and the layout as JSON (names, shapes, positions, levels). `theme`, `courts` (3–6) and `order` as in dungeons_create's `compound`. Try seeds, then pass the one you like to dungeons_create. `save_path` also writes the PNG.",
        input_schema: || {
            let mut s = options_schema();
            s["properties"]["realm_size"] = json!({ "type": "string", "enum": super::realm_shapes::RealmSize::ALL.map(|s| s.id()) });
            s["properties"]["seed"] = json!({ "type": "integer", "minimum": 0 });
            s["properties"]["px"] = json!({ "type": "integer", "minimum": 128, "maximum": 1024 });
            s["properties"]["save_path"] = json!({ "type": "string" });
            s
        },
        run: |_, a| {
            let o = Options::from_json(&a)?.clamped();
            let size = match opt_arg::<String>(&a, "realm_size")? {
                Some(s) => super::realm_shapes::RealmSize::from_id(&s).ok_or_else(|| format!("unknown realm_size `{s}`"))?,
                None => super::realm_shapes::RealmSize::Small,
            };
            let seed: u32 = opt_arg(&a, "seed")?.unwrap_or(1);
            let px = opt_arg::<u32>(&a, "px")?.unwrap_or(512).clamp(128, 1024) as usize;
            let (rgba, plan) = picker::render_preview(o, size, seed, px);
            let plan = plan?;
            let mut png = Vec::new();
            {
                use image::ImageEncoder;
                image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba, px as u32, px as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
            }
            if let Some(p) = opt_arg::<String>(&a, "save_path")?.filter(|p| !p.trim().is_empty()) {
                std::fs::write(p.trim(), &png).map_err(|e| format!("couldn't write {p}: {e}"))?;
            }
            Ok(vec![McpContent::Png(png), McpContent::Json(json!({ "options": o.json(), "realm_size": size.id(), "seed": seed, "layout": plan.summary(), "note": "the layout dungeons_create makes from these (the island's height and the objects come when it is built)" }))])
        },
    });
}

#[cfg(test)]
mod tests {
    use super::layout::{Ask, CourtKind, Floor, Grid, lay_out};
    use super::*;
    use crate::terrain::realm_land::{self, Detail, RealmShape, RealmSize};

    fn plan_for(
        theme: Theme,
        courts: usize,
        order: Order,
        size: RealmSize,
        seed: u32,
        detail: Detail,
    ) -> (realm_land::Land, Result<Plan, String>) {
        let land = realm_land::generate(RealmShape::EnclosedValley, size, seed, detail);
        let g = Grid {
            n: land.n(),
            cell: land.cell,
            h: &land.heights,
        };
        let p = lay_out(
            &g,
            &Ask {
                theme,
                courts,
                order,
                seed,
                levels: [10, 15],
            },
        );
        (land, p)
    }

    /// Every court on the floor, apart from the others, off the axis; the axis straight and on
    /// the floor; the island court in its lake; trails from where they start to where they go.
    fn check(land: &realm_land::Land, p: &Plan, asked: usize) {
        let g = Grid {
            n: land.n(),
            cell: land.cell,
            h: &land.heights,
        };
        let floor = Floor::find(&g).expect("a floor");
        assert!(
            p.courts.len() + p.dropped == asked,
            "courts {} + dropped {} != {asked}",
            p.courts.len(),
            p.dropped
        );
        assert!(
            p.courts.iter().any(|c| c.kind == CourtKind::SteppedPyramid) || p.courts.len() < 3,
            "a stepped pyramid"
        );
        for (i, a) in p.courts.iter().enumerate() {
            assert!(
                floor.room(a.c) >= a.reach() * 0.9,
                "{} sits on the floor ({} m of room, reach {})",
                a.name,
                floor.room(a.c),
                a.reach()
            );
            assert!(
                p.axis.dist(a.c) > a.reach() + p.axis.hw,
                "{} keeps off the axis",
                a.name
            );
            for b in &p.courts[i + 1..] {
                assert!(
                    a.c.distance(b.c) > a.reach() + b.reach() + 15.0,
                    "{} and {} are spaced",
                    a.name,
                    b.name
                );
            }
            if let Some(l) = a.island {
                assert!(
                    crate::dungeons::geom::poly_sdf(&p.lakes[l].outline, a.c) > a.hx,
                    "{} stands in its lake",
                    a.name
                );
            }
        }
        // the axis runs on the floor all the way, straight from the gate to the final stairs
        let n = (p.axis.len() / 4.0) as usize;
        for i in 0..=n {
            let q = p.axis.gate.lerp(p.axis.end, i as f32 / n.max(1) as f32);
            assert!(
                floor.room(q) > p.axis.hw * 0.8,
                "the axis stays on the floor at {q:?}"
            );
        }
        assert!(p.axis.len() > 60.0, "the axis is grand: {} m", p.axis.len());
        // the lakes keep off the courts (but the island's)
        for (li, l) in p.lakes.iter().enumerate() {
            for (ci, c) in p.courts.iter().enumerate() {
                if l.island == Some(ci) {
                    continue;
                }
                assert!(
                    crate::dungeons::geom::poly_sdf(&l.outline, c.c) < -c.reach(),
                    "lake {li} keeps off {}",
                    c.name
                );
            }
        }
        // a trail reaches every court
        for (ci, c) in p.courts.iter().enumerate() {
            let t = p
                .trails
                .iter()
                .filter(|t| t.to == Some(ci))
                .last()
                .expect("a trail to every court");
            assert!(
                t.pts.last().unwrap().distance(c.stair_foot) < 3.0,
                "the trail to {} ends at its stairs",
                c.name
            );
        }
        assert!(p.entrance.wall.len() >= 2, "a wall along the open edge");
    }

    #[test]
    fn compounds_lay_out_on_valley_land() {
        let mut lakes = 0;
        let mut islands = 0;
        for seed in 1..=14u32 {
            let theme = Theme::ALL[seed as usize % 5];
            let courts = 3 + (seed as usize % 4);
            let order = if seed % 4 == 0 {
                Order::Linear
            } else {
                Order::Hub
            };
            let (land, p) = plan_for(theme, courts, order, RealmSize::Small, seed, Detail::Full);
            let p = p.unwrap_or_else(|e| panic!("seed {seed}: {e}"));
            check(&land, &p, courts);
            assert!(
                p.dropped <= 1,
                "seed {seed}: {} of {courts} courts didn't fit",
                p.dropped
            );
            lakes += p.lakes.len();
            islands += p.lakes.iter().filter(|l| l.island.is_some()).count();
        }
        assert!(
            lakes >= 14,
            "lakes in the low ground ({lakes} in 14 compounds)"
        );
        assert!(islands >= 8, "island temples ({islands} in 14 compounds)");
    }

    #[test]
    fn compounds_lay_out_on_the_real_land_of_every_size() {
        for (size, courts) in [
            (RealmSize::Cozy, 3),
            (RealmSize::Small, 5),
            (RealmSize::Medium, 6),
        ] {
            let (land, p) = plan_for(Theme::Jungle, courts, Order::Hub, size, 7, Detail::Full);
            let p = p.unwrap_or_else(|e| panic!("{size:?}: {e}"));
            check(&land, &p, courts);
        }
    }

    #[test]
    fn a_seed_makes_the_same_compound_and_another_a_different_one() {
        let a = plan_for(
            Theme::Jungle,
            4,
            Order::Hub,
            RealmSize::Small,
            11,
            Detail::Preview(200),
        )
        .1
        .unwrap()
        .summary();
        let b = plan_for(
            Theme::Jungle,
            4,
            Order::Hub,
            RealmSize::Small,
            11,
            Detail::Preview(200),
        )
        .1
        .unwrap()
        .summary();
        let c = plan_for(
            Theme::Jungle,
            4,
            Order::Hub,
            RealmSize::Small,
            12,
            Detail::Preview(200),
        )
        .1
        .unwrap()
        .summary();
        assert_eq!(a, b);
        assert_ne!(a["courts"], c["courts"]);
    }

    #[test]
    fn every_asset_a_compound_uses_is_in_the_hand_painted_pack() {
        let path = crate::asset_file("models/stylepacks/handpainted/models.json");
        if !path.is_file() {
            return; // public source deliberately excludes this optional paid pack
        }
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let ids: std::collections::HashSet<String> = doc["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect();
        for t in Theme::ALL {
            for a in theme::kit_assets(t.kit()) {
                assert!(ids.contains(a), "{}: hp:{a} is not in the pack", t.id());
            }
            // and every object the build places
            let (_, p) = plan_for(t, 5, Order::Hub, RealmSize::Small, 3, Detail::Preview(200));
            let p = p.unwrap();
            let mut placed = 0;
            for s in furnish::steps(&p) {
                if let Step::Call {
                    tool: "objects_place",
                    args,
                    ..
                } = s
                {
                    for it in args["items"].as_array().unwrap() {
                        let a = it["asset"].as_str().unwrap();
                        assert!(ids.contains(a.trim_start_matches("hp:")), "{a}");
                        placed += 1;
                    }
                }
            }
            assert!(placed > 150, "{}: {placed} objects", t.id());
        }
    }

    #[test]
    fn words_pick_the_compound() {
        use crate::dungeons::describe::{compound_options, read};
        for s in [
            "a Zul'Aman-like troll temple city",
            "a jungle ziggurat compound",
            "an outdoor troll city with five courts",
            "a temple city in the desert",
        ] {
            assert_eq!(read(s, None).layout, LAYOUT, "{s}");
        }
        for s in ["a troll cave", "a sunken temple", "a desert realm of dunes"] {
            assert_ne!(read(s, None).layout, LAYOUT, "{s}");
        }
        assert_eq!(
            compound_options("a temple city in the desert").theme,
            Theme::Desert
        );
        assert_eq!(
            compound_options("an outdoor troll city with five courts").courts,
            5
        );
    }

    /// `WB_COMPOUND_PREVIEWS=<dir>`: writes preview maps of a few seeds and themes there (and
    /// the real land of seed 1 shaped for its compound).
    #[test]
    fn preview_maps() {
        let Ok(dir) = std::env::var("WB_COMPOUND_PREVIEWS") else {
            return;
        };
        let _ = std::fs::create_dir_all(&dir);
        {
            let (mut land, p) = plan_for(
                Theme::Jungle,
                4,
                Order::Hub,
                RealmSize::Small,
                1,
                Detail::Full,
            );
            let p = p.unwrap();
            let n = land.n();
            let img = realm_land::render(&land, 1024, realm_land::Palette::Jungle);
            image::save_buffer(
                format!("{dir}/full_jungle_1_raw.png"),
                &img,
                1024,
                1024,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
            shape::carve(&mut land.heights, n, land.cell, &p);
            let img = realm_land::render(&land, 1024, realm_land::Palette::Jungle);
            image::save_buffer(
                format!("{dir}/full_jungle_1.png"),
                &img,
                1024,
                1024,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        }
        for (i, (theme, seed, size)) in [
            (Theme::Jungle, 1, RealmSize::Small),
            (Theme::Jungle, 2, RealmSize::Small),
            (Theme::Jungle, 3, RealmSize::Small),
            (Theme::Desert, 4, RealmSize::Small),
            (Theme::Frozen, 5, RealmSize::Medium),
            (Theme::Twilight, 6, RealmSize::Small),
        ]
        .iter()
        .enumerate()
        {
            let o = Options {
                theme: *theme,
                courts: 4 + (i % 2) as u8,
                order: Order::Hub,
            };
            let (img, plan) = picker::render_preview(o, *size, *seed, 512);
            let plan = plan.unwrap();
            image::save_buffer(
                format!("{dir}/preview_{}_{seed}.png", theme.id()),
                &img,
                512,
                512,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
            std::fs::write(
                format!("{dir}/preview_{}_{seed}.json", theme.id()),
                serde_json::to_string_pretty(&plan.summary()).unwrap(),
            )
            .unwrap();
        }
    }
}

#[cfg(test)]
mod summary_dump {
    use super::*;
    use crate::terrain::realm_land::{self, Detail, RealmShape, RealmSize};

    /// `WB_COMPOUND_SUMMARY=theme,courts,order,size,seed,out.json`: the layout a compound realm
    /// made from those gets (the same land `dungeons_open` makes), for scripted screenshots.
    #[test]
    fn dump_summary() {
        let Ok(spec) = std::env::var("WB_COMPOUND_SUMMARY") else {
            return;
        };
        for one in spec.split(';') {
            let v: Vec<&str> = one.split(',').map(str::trim).collect();
            let theme = Theme::from_id(v[0]).unwrap();
            let size = RealmSize::from_id(v[3]).unwrap();
            let seed: u32 = v[4].parse().unwrap();
            let land = realm_land::generate(RealmShape::EnclosedValley, size, seed, Detail::Full);
            let g = layout::Grid {
                n: land.n(),
                cell: land.cell,
                h: &land.heights,
            };
            let p = layout::lay_out(
                &g,
                &layout::Ask {
                    theme,
                    courts: v[1].parse().unwrap(),
                    order: Order::from_id(v[2]).unwrap(),
                    seed,
                    levels: [10, 15],
                },
            )
            .unwrap();
            std::fs::write(v[5], serde_json::to_string_pretty(&p.summary()).unwrap()).unwrap();
        }
    }
}
