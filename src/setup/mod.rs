//! Guided setup: a new world from a short, visual questionnaire. File › New world offers a blank
//! world or the guided setup; its steps (`wizard`) fill in a `plan::WorldPlan` — the MMOs the
//! user loves, the kind of game, the land, towns, adventure, gathering — the AI (`ai`) reads the
//! user's own words into it and makes up names, and `build` turns the plan into a playable world
//! through the editor's own tools: land and climates, rivers, nature, towns and roads, zones with
//! level ranges, dungeons, caves, monster camps, creatures, gathering, the look; then checks it
//! (reachability, level flow, content gaps) and flies the camera through it.
//!
//! MCP: `world_setup` (a plan → a world), `world_setup_status`, `world_setup_name_ideas`.

pub mod ai;
pub mod build;
pub mod plan;
pub mod wizard;

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
pub use plan::WorldPlan;

/// The guided setup's state (the wizard's and the build's).
#[derive(Resource, Default)]
pub struct Setup {
    /// the wizard is open
    pub open: bool,
    /// the wizard's page
    pub step: usize,
    pub plan: WorldPlan,
    /// the AI reading the user's description
    pub ai: AiState,
    /// a build running or finished
    pub build: Option<BuildState>,
    pub requests: Vec<SetupRequest>,
}

/// What the wizard (or MCP) asks for; done with world access (`handle_requests`).
pub enum SetupRequest {
    /// read `plan.description` (and the picks so far) with the AI: fills in the plan and names
    ReadDescription,
    /// build a new world from the plan
    Build(WorldPlan),
    /// stop the build (what's made so far stays)
    Cancel,
    /// fly the camera through the finished world
    Flythrough,
    /// walk into the finished world where players start
    Walk,
    /// show a place the build made ("start", "capital" or its name)
    GoTo(String),
}

#[derive(Default, Clone)]
pub struct AiState {
    pub running: bool,
    /// what's happening ("Reading your description…")
    pub status: String,
    /// what it understood, in plain words (shown under the description)
    pub understood: String,
    pub error: Option<String>,
}

/// A build's progress, for the wizard's last page and `world_setup_status`.
#[derive(Clone, Debug)]
pub struct BuildState {
    pub plan: WorldPlan,
    /// the stages and how far each got (0..1)
    pub stages: Vec<(String, f32)>,
    pub current: usize,
    /// one line per thing made
    pub log: Vec<String>,
    pub done: bool,
    pub cancelled: bool,
    pub failed: Option<String>,
    /// what the checks found, in plain words
    pub report: Vec<String>,
    /// seconds since it started
    pub elapsed: f32,
}

impl BuildState {
    pub fn progress(&self) -> f32 {
        if self.stages.is_empty() {
            return 0.0;
        }
        self.stages.iter().map(|s| s.1).sum::<f32>() / self.stages.len() as f32
    }
}

pub struct SetupPlugin;

impl Plugin for SetupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Setup>()
            .init_resource::<build::BuildRun>()
            .add_systems(Update, (handle_requests, build::tick, ai::poll).chain())
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                wizard::wizard_ui.run_if(crate::mcp::ui_visible),
            );
        register(app);
    }
}

fn handle_requests(world: &mut World) {
    let reqs = std::mem::take(&mut world.resource_mut::<Setup>().requests);
    for r in reqs {
        match r {
            SetupRequest::ReadDescription => ai::start(world),
            SetupRequest::Build(plan) => build::start(world, plan),
            SetupRequest::Cancel => build::cancel(world),
            SetupRequest::Flythrough => build::flythrough(world),
            SetupRequest::Walk => build::walk(world),
            SetupRequest::GoTo(p) => {
                if let Err(e) = build::goto(world, &p, [None; 3]) {
                    warn!("guided setup: {e}");
                }
            }
        }
    }
}

fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "world_setup",
        description: "Guided setup (File › New world › Guided setup) in one call: build a NEW playable world from a plan — replaces the open world (save it first). \
The plan's fields (all optional; defaults in brackets): name, description (free words), inspirations (wow, daoc, bdo, elder_scrolls, gw2, lotro, ffxiv, runescape: they suggest art, look, game and levels), art (hand_painted | photoreal), look (a look_set preset), \
game (themepark | sandbox | realm_war | survival | story), levels [1, 60], land (continent | twin_islands | archipelago | peninsula | inland_sea | fjord_coast | mountain_island), size (small 2 km | medium 4 km | large 8 km), climate (varied | temperate | tropical | arid | cold), rivers, \
realms (1–3), capital, towns, villages, roads, farms, zones (named zones with level ranges), dungeons, raids, outdoor_dungeons (how many of them are open-air, built from the design-sheet library: the raids first, then every other party dungeon) [3], unique_dungeons (paint each open-air one a unique sheet of its own with the user's Codex/ChatGPT afterwards) [false], dungeon_layouts (dungeons_catalog ids; empty: by the land), dungeon_rooms (normal | large | extra_large), dungeon_irregularity (0..1), caves, encounters (monster camps), creatures (spawn regions), \
gathering {herbs, wood, stone, ore, fish, density}, hour, seed, names {world, capital, towns[], zones[], dungeons[], realms[]}. \
It builds over a few minutes: land, climates, water, nature, towns and roads, zones, dungeons, caves, camps, creatures, gathering, the look; then checks reachability, level flow and dead zones, and saves it as worlds/<name>.world (with its plan in plan.json). Poll world_setup_status. \
Refuses if the open world has unsaved changes unless `discard_unsaved` is true.",
        input_schema: || json!({ "type": "object", "properties": { "plan": { "type": "object", "description": "the world plan (see the description)" }, "discard_unsaved": { "type": "boolean", "default": false } } }),
        run: |w, a| {
            if w.resource::<Setup>().build.as_ref().is_some_and(|b| !b.done) {
                return Err("a guided setup is already building (world_setup_status)".into());
            }
            if w.resource::<crate::dungeons::DungeonMap>().0.is_some() {
                return Err("a dungeon is open: go back to its world first (dungeons_back)".into());
            }
            if w.resource::<crate::world::WorldMeta>().dirty && !a.get("discard_unsaved").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err("The open world has unsaved changes: save it (world_save) first, or pass discard_unsaved: true".into());
            }
            let mut plan: WorldPlan = match a.get("plan") {
                Some(p) => serde_json::from_value(p.clone()).map_err(|e| format!("plan: {e}"))?,
                None => WorldPlan::default(),
            };
            if plan.look == WorldPlan::default().look && !plan.inspirations.is_empty() {
                plan.apply_inspirations();
            }
            let plan = plan.normalized();
            let lines = plan.summary();
            let mut s = w.resource_mut::<Setup>();
            s.plan = plan.clone();
            s.requests.push(SetupRequest::Build(plan.clone()));
            Ok(vec![McpContent::Json(json!({ "building": plan.name, "plan": plan, "summary": lines, "note": "poll world_setup_status" }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "world_setup_name_ideas",
        description: "Made-up world names (Asterath, Valoria, Thalmere…), what the guided setup's dice button rolls: none is a world already in worlds/. Pass one as the plan's `name` to world_setup. `count` (1–20, default 5).",
        input_schema: || json!({ "type": "object", "properties": { "count": { "type": "integer", "minimum": 1, "maximum": 20, "default": 5 } } }),
        run: |_, a| {
            let n = a.get("count").and_then(|v| v.as_u64()).unwrap_or(5).clamp(1, 20) as usize;
            let dir = crate::io::worlds_dir();
            let mut names: Vec<String> = vec![];
            for _ in 0..n {
                let name = plan::random_world_name(|x| names.iter().any(|m| m.eq_ignore_ascii_case(x)) || dir.join(format!("{x}.world")).exists());
                names.push(name);
            }
            Ok(vec![McpContent::Json(json!({ "names": names }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "world_setup_goto",
        description: "Show a place the guided setup made: the camera flies to it. `place`: \"start\" (where players start), \"capital\" (or the first town) or a place's name from world_setup_status. Optional view: `distance` (m), `pitch_deg` (how far it looks down), `turn_deg` (the camera's side of the place).",
        input_schema: || json!({ "type": "object", "properties": { "place": { "type": "string" }, "distance": { "type": "number" }, "pitch_deg": { "type": "number" }, "turn_deg": { "type": "number" } }, "required": ["place"] }),
        run: |w, a| {
            let place = a.get("place").and_then(|p| p.as_str()).ok_or("missing `place`")?.to_string();
            let num = |k: &str| a.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
            let name = build::goto(w, &place, [num("distance"), num("pitch_deg"), num("turn_deg")])?;
            Ok(vec![McpContent::Text(format!("Showing {name}"))])
        },
    });
    app.mcp_tool(McpTool {
        name: "world_setup_status",
        description: "How the guided setup's build is going: its stages and progress, what it has made, and when done what the checks found.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, _| {
            let s = w.resource::<Setup>();
            let v = match &s.build {
                None => json!({ "building": false }),
                Some(b) => json!({
                    "world": b.plan.name,
                    "progress": (b.progress() * 100.0).round(),
                    "stage": b.stages.get(b.current).map(|x| x.0.clone()),
                    "stages": b.stages.iter().map(|(n, p)| json!({ "stage": n, "done": (*p * 100.0).round() })).collect::<Vec<Value>>(),
                    "done": b.done,
                    "cancelled": b.cancelled,
                    "failed": b.failed,
                    "log": b.log.iter().rev().take(40).rev().cloned().collect::<Vec<_>>(),
                    "report": b.report,
                    "seconds": b.elapsed.round(),
                    "start": w.resource::<build::BuildRun>().start.map(|(p, yaw)| json!({ "position": p, "yaw_deg": yaw })),
                    "places": w.resource::<build::BuildRun>().places.iter().map(|(n, k, p)| json!({ "name": n, "kind": k, "position": p })).collect::<Vec<_>>(),
                }),
            };
            Ok(vec![McpContent::Json(v)])
        },
    });
}
