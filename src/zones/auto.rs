//! LLM-assisted zone layout.  The agent sees a bounded north-up terrain context and returns a
//! typed proposal; it never receives editor tools and cannot mutate a document itself.

mod partition;

use std::path::PathBuf;
use std::sync::{
    Mutex,
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use image::{ImageBuffer, Rgb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::gameplay::Gameplay;
use crate::history::History;
use crate::mcp_tools::{McpContent, McpTool};
use crate::prompt::agent::{self, AgentEvent, AgentHandle, Backend};
use crate::prompt::{self, PromptSession};
use crate::terrain::{GridRect, Terrain};
use crate::world::{Soundscape, Weather, Zone, ZoneType, Zones};

const MAX_SIDE: usize = 128;
const MAP_SIDE: u32 = 1024;
const MAX_REPLY: usize = 256 * 1024;
const MAX_ZONES: usize = 24;
const START_RADIUS_METRES: f32 = 180.0;

const INSTRUCTIONS: &str = "You design gameplay zones for a world builder. Answer with one JSON object only, no prose or markdown. It must have `zones`, an array of 1..24 objects. Each object has `name` (1..80 chars), optional `description` (max 320 chars), `level_min`, `level_max`, optional `zone_type` one of open_world|sanctuary|contested|city|dungeon|raid, optional `settings`: allow_mounts, allow_flying, rested_area, limit_placement, visible booleans; weather clear|cloudy|rain|storm|snow|fog|sandstorm; ambience none|forest|meadow|mountain|coast|marsh|desert|cave|frost|town|ruins|volcanic|eerie|sunken|infernal; haze {mode inherit|on|off, amount 0..1}; forest_diversity {enabled, layout natural_groves|mixed}; temperature -40..55; weather_intensity (.1..1); ambience_volume/cloud_cover (0..1); wind_speed (0..25); and `region_ids`, a nonempty array of candidate IDs shown on the map. Assign every candidate exactly once across the zones. A zone's candidate IDs must form one connected group. Keep the player start in a level 1-5 zone and all zones within 180m of it at level 10 or below. Levels are integral, ordered, and must not exceed the cap. Ensure a painted zone reaches the cap. Candidate geometry is fixed: make names, setting choices, and progression fit the actual coastlines, waterways, ridges, valleys, and biome regions that candidates capture. Do not invent coordinates or request geometry changes.";

/// Requests are queued by the Zones panel so the worker is the only owner of UI state.
pub enum AutoZoneRequest {
    Start,
    Cancel,
    Undo,
}

/// The same queue used by the panel. `status` is read-only; mutations are deferred to Update so
/// an MCP call cannot interleave a running agent or a brush stroke.
pub fn mcp_tool() -> McpTool {
    McpTool {
        name: "zones_auto",
        description: "Ask the connected language model to fill only unzoned dry land with a safe level progression. action is start, cancel, undo, or status.",
        input_schema: || json!({"type":"object","properties":{"action":{"type":"string","enum":["start","cancel","undo","status"]}},"required":["action"]}),
        run: |world, args| {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .ok_or("`action` is required")?;
            if action == "status" {
                return Ok(vec![McpContent::Json(AutoZones::status(world))]);
            }
            let request = match action {
                "start" => AutoZoneRequest::Start,
                "cancel" => AutoZoneRequest::Cancel,
                "undo" => AutoZoneRequest::Undo,
                _ => return Err("`action` must be start, cancel, undo, or status".into()),
            };
            world.resource_mut::<AutoZones>().requests.push(request);
            Ok(vec![McpContent::Json(AutoZones::status(world))])
        },
    }
}

#[derive(Resource)]
pub struct AutoZones {
    pub requests: Vec<AutoZoneRequest>,
    pub status: String,
    pub error: Option<String>,
    lifecycle: Lifecycle,
    applied: Option<(u64, u64)>, // (history serial, terrain generation)
    artifact: Option<PathBuf>,
    applied_count: Option<usize>,
    started_at: Option<Instant>,
}

impl Default for AutoZones {
    fn default() -> Self {
        Self {
            requests: vec![],
            status: String::new(),
            error: None,
            lifecycle: Lifecycle::Idle,
            applied: None,
            artifact: None,
            applied_count: None,
            started_at: None,
        }
    }
}

impl AutoZones {
    pub fn busy(&self) -> bool {
        !matches!(self.lifecycle, Lifecycle::Idle)
    }

    /// A stable, human-facing stage for the editor. `status` may contain a backend update, so it
    /// remains available to MCP while the UI uses this lifecycle-derived label instead.
    pub fn stage(&self) -> Option<&'static str> {
        match &self.lifecycle {
            Lifecycle::Idle => None,
            Lifecycle::Preparing { .. } => Some("Preparing terrain"),
            Lifecycle::Running { repair: false, .. } => Some("Creating zones"),
            Lifecycle::Running { repair: true, .. } => Some("Refining zone plan"),
            Lifecycle::Validating { .. } => Some("Checking zone plan"),
        }
    }

    /// Elapsed wall time is intentionally monotonic and only shown while work is active.
    pub fn elapsed(&self) -> Option<Duration> {
        if self.busy() {
            self.started_at.map(|started_at| started_at.elapsed())
        } else {
            None
        }
    }

    pub fn can_undo(&self, history: &History, generation: u64) -> bool {
        self.applied.is_some_and(|(serial, made_on)| {
            !self.busy()
                && !history.is_recording()
                && !history.in_group()
                && made_on == generation
                && history.undo_serial() == Some(serial)
        })
    }

    pub fn start(&mut self, world: &mut World) -> Result<(), String> {
        if self.busy() {
            return Err("Auto zones is already running".into());
        }
        let prompt = world.resource::<PromptSession>();
        let gameplay = world.resource::<Gameplay>();
        if let Some(e) = prerequisite_error(prompt, gameplay) {
            return Err(e);
        }
        if world.resource::<History>().is_recording() || world.resource::<History>().in_group() {
            return Err("Finish the current edit before auto zoning".into());
        }
        if prompt.running()
            || world
                .get_resource::<crate::setup::Setup>()
                .is_some_and(|s| {
                    s.ai.running
                        || s.build
                            .as_ref()
                            .is_some_and(|b| !b.done && !b.cancelled && b.failed.is_none())
                })
        {
            return Err("Finish the active AI or setup run before auto zoning".into());
        }
        let snapshot = Snapshot::capture(world)?;
        if !snapshot
            .dry
            .iter()
            .enumerate()
            .any(|(i, dry)| *dry && snapshot.grid[i] == 0)
        {
            return Err("There is no unzoned dry land to fill".into());
        }
        if (1..=63).all(|id| snapshot.zones.iter().any(|zone| zone.id == id)) {
            return Err("All 63 zone IDs are already in use".into());
        }
        let (tx, rx) = mpsc::channel();
        let mut map_snapshot = snapshot;
        std::thread::Builder::new()
            .name("auto-zones-map".into())
            .spawn(move || {
                let _ = tx.send(map_snapshot.build_partition().and_then(|_| {
                    render_context_map(&map_snapshot).map(|image| (map_snapshot, image))
                }));
            })
            .map_err(|e| e.to_string())?;
        self.error = None;
        self.artifact = None;
        self.applied_count = None;
        self.started_at = Some(Instant::now());
        self.status = "Preparing terrain context map…".into();
        self.lifecycle = Lifecycle::Preparing {
            map: Mutex::new(rx),
        };
        Ok(())
    }

    pub fn cancel(&mut self, _world: &mut World) {
        if let Lifecycle::Running { handle, .. } = &self.lifecycle {
            handle.stop();
        }
        self.lifecycle = Lifecycle::Idle;
        self.started_at = None;
        self.status = "Auto zones cancelled".into();
        self.error = None;
    }

    pub fn undo(&mut self, world: &mut World) -> Result<(), String> {
        let generation = world.resource::<Terrain>().generation;
        if !self.can_undo(world.resource::<History>(), generation) {
            return Err("Auto zones can only undo the latest unchanged auto-zone step".into());
        }
        let (serial, made_on) = self.applied.ok_or("There is no auto-zone step to undo")?;
        debug_assert_eq!(generation, made_on);
        debug_assert_eq!(world.resource::<History>().undo_serial(), Some(serial));
        let mut history = world
            .remove_resource::<History>()
            .ok_or("History is unavailable")?;
        let mut terrain = world
            .remove_resource::<Terrain>()
            .ok_or("Terrain is unavailable")?;
        let mut zones = world
            .remove_resource::<Zones>()
            .ok_or("Zones are unavailable")?;
        let mut gameplay = world
            .remove_resource::<Gameplay>()
            .ok_or("Gameplay is unavailable")?;
        let mut store = world
            .remove_resource::<crate::objects::ObjectStore>()
            .ok_or("Objects are unavailable")?;
        let mut water = world
            .remove_resource::<crate::water::WaterBodies>()
            .ok_or("Water is unavailable")?;
        let mut roads = world
            .remove_resource::<crate::roads::Roads>()
            .ok_or("Roads are unavailable")?;
        history.step(
            false,
            &mut terrain,
            &mut store,
            &mut water,
            &mut zones,
            &mut gameplay,
            &mut roads,
        );
        world.insert_resource(history);
        world.insert_resource(terrain);
        world.insert_resource(zones);
        world.insert_resource(gameplay);
        world.insert_resource(store);
        world.insert_resource(water);
        world.insert_resource(roads);
        world.resource_mut::<crate::world::WorldMeta>().dirty = true;
        self.applied = None;
        self.status = "Auto zones undone".into();
        Ok(())
    }

    pub fn status(world: &World) -> Value {
        let a = world.resource::<AutoZones>();
        json!({"busy": a.busy(), "stage": a.stage(), "elapsed_seconds": a.elapsed().map(|elapsed| elapsed.as_secs_f32()), "status": a.status, "error": a.error, "can_undo": a.can_undo(world.resource::<History>(), world.resource::<Terrain>().generation), "artifact": a.artifact.as_ref().map(|p| p.display().to_string()), "applied_zones": a.applied_count})
    }
}

/// The prompt connection and saved gameplay inputs required before a request can start.
pub fn prerequisite_error(session: &PromptSession, gameplay: &Gameplay) -> Option<String> {
    if prompt::ai_ready(session).is_none() {
        return Some("Connect a language model first".into());
    }
    if gameplay.level_cap.is_none_or(|n| !(1..=200).contains(&n)) {
        return Some("Save a level cap from 1 to 200 first".into());
    }
    if gameplay.player_start.is_none() {
        return Some("Place a player start first".into());
    }
    None
}

enum Lifecycle {
    Idle,
    Preparing {
        map: Mutex<Receiver<Result<(Snapshot, PathBuf), String>>>,
    },
    Running {
        snapshot: Snapshot,
        handle: AgentHandle,
        endpoint: crate::mcp::PrivateEndpoint,
        repair: bool,
    },
    Validating {
        snapshot: Snapshot,
        result: Mutex<Receiver<Result<Proposal, String>>>,
        repair: bool,
        answer: String,
    },
}

pub fn drive(world: &mut World) {
    world.resource_scope(|world, mut auto: Mut<AutoZones>| {
        let was_busy = auto.busy();
        let requests = std::mem::take(&mut auto.requests);
        for request in requests {
            match request {
                AutoZoneRequest::Start => {
                    if let Err(e) = auto.start(world) {
                        auto.error = Some(e);
                    }
                }
                AutoZoneRequest::Cancel => auto.cancel(world),
                AutoZoneRequest::Undo => {
                    if let Err(e) = auto.undo(world) {
                        auto.error = Some(e);
                    }
                }
            }
        }
        auto.poll(world);
        if was_busy && !auto.busy() {
            let message = if auto.error.is_some() {
                "Could not create zones. See the Zones panel for details.".to_owned()
            } else {
                auto.status.clone()
            };
            if !message.is_empty() {
                world.resource_mut::<crate::editor::Editor>().toast(message);
            }
        }
    });
}

impl AutoZones {
    fn poll(&mut self, world: &mut World) {
        let state = std::mem::replace(&mut self.lifecycle, Lifecycle::Idle);
        self.lifecycle = match state {
            Lifecycle::Idle => Lifecycle::Idle,
            Lifecycle::Preparing { map } => {
                let received = { map.lock().unwrap_or_else(|e| e.into_inner()).try_recv() };
                match received {
                    Ok(Ok((snapshot, image))) => {
                        self.artifact = Some(image.clone());
                        match begin_agent(world, &snapshot, image, None) {
                            Ok((handle, endpoint)) => {
                                self.status = "Designing zones…".into();
                                Lifecycle::Running {
                                    snapshot,
                                    handle,
                                    endpoint,
                                    repair: false,
                                }
                            }
                            Err(e) => {
                                self.error = Some(e);
                                self.status.clear();
                                self.started_at = None;
                                Lifecycle::Idle
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        self.error = Some(e);
                        self.status.clear();
                        self.started_at = None;
                        Lifecycle::Idle
                    }
                    Err(TryRecvError::Empty) => Lifecycle::Preparing { map },
                    Err(TryRecvError::Disconnected) => {
                        self.error = Some("Terrain context worker stopped".into());
                        self.started_at = None;
                        Lifecycle::Idle
                    }
                }
            }
            Lifecycle::Running {
                snapshot,
                handle,
                endpoint,
                repair,
            } => {
                // There are deliberately no tools. Drain attempted tool calls so a model cannot
                // hold a request open waiting for an unavailable mutation.
                while endpoint.take_job().is_some() {}
                let mut end = None;
                if let Ok(rx) = handle.rx.lock() {
                    while let Ok(event) = rx.try_recv() {
                        match event {
                            AgentEvent::Status(s) => self.status = s,
                            AgentEvent::Finished { summary, .. } => end = Some(Ok(summary)),
                            AgentEvent::Failed(e) => end = Some(Err(e)),
                            AgentEvent::Message(_) => {}
                        }
                    }
                }
                match end {
                    None => Lifecycle::Running {
                        snapshot,
                        handle,
                        endpoint,
                        repair,
                    },
                    Some(Err(e)) => {
                        self.error = Some(e);
                        self.status.clear();
                        self.started_at = None;
                        Lifecycle::Idle
                    }
                    Some(Ok(answer)) if answer.len() > MAX_REPLY => {
                        self.error = Some("The auto-zone answer was too large".into());
                        self.status.clear();
                        self.started_at = None;
                        Lifecycle::Idle
                    }
                    Some(Ok(answer)) => {
                        let (tx, rx) = mpsc::channel();
                        let copy = snapshot.clone();
                        let validation_answer = answer.clone();
                        let _ = std::thread::Builder::new()
                            .name("auto-zones-validate".into())
                            .spawn(move || {
                                let _ = tx.send(validate(&copy, &validation_answer));
                            });
                        self.status = "Validating zone plan…".into();
                        Lifecycle::Validating {
                            snapshot,
                            result: Mutex::new(rx),
                            repair,
                            answer,
                        }
                    }
                }
            }
            Lifecycle::Validating {
                snapshot,
                result,
                repair,
                answer,
            } => {
                let received = { result.lock().unwrap_or_else(|e| e.into_inner()).try_recv() };
                match received {
                    Ok(Ok(plan)) => {
                        let added = plan.list.len().saturating_sub(snapshot.zones.len());
                        match apply(world, &snapshot, plan) {
                            Ok(serial) => {
                                self.applied = Some((serial, snapshot.generation));
                                self.applied_count = Some(added);
                                self.status = format!(
                                    "Created {added} zone{}",
                                    if added == 1 { "" } else { "s" }
                                );
                                self.error = None;
                                self.started_at = None;
                                Lifecycle::Idle
                            }
                            Err(e) => {
                                self.error = Some(e);
                                self.status.clear();
                                self.started_at = None;
                                Lifecycle::Idle
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        if !repair {
                            if let Some(image) = self.artifact.clone() {
                                let mut cutoff = answer.len().min(32 * 1024);
                                while cutoff > 0 && !answer.is_char_boundary(cutoff) {
                                    cutoff -= 1;
                                }
                                let prior = &answer[..cutoff];
                                let correction = format!("{e}\nPrevious JSON to correct:\n{prior}");
                                match begin_agent(world, &snapshot, image, Some(&correction)) {
                                    Ok((handle, endpoint)) => {
                                        self.status = "Repairing zone plan…".into();
                                        Lifecycle::Running {
                                            snapshot,
                                            handle,
                                            endpoint,
                                            repair: true,
                                        }
                                    }
                                    Err(start_error) => {
                                        self.error = Some(start_error);
                                        self.status.clear();
                                        self.started_at = None;
                                        Lifecycle::Idle
                                    }
                                }
                            } else {
                                self.error = Some(e);
                                self.status.clear();
                                self.started_at = None;
                                Lifecycle::Idle
                            }
                        } else {
                            self.error = Some(e);
                            self.status.clear();
                            self.started_at = None;
                            Lifecycle::Idle
                        }
                    }
                    Err(TryRecvError::Empty) => Lifecycle::Validating {
                        snapshot,
                        result,
                        repair,
                        answer,
                    },
                    Err(TryRecvError::Disconnected) => {
                        self.error = Some("Zone validation worker stopped".into());
                        self.started_at = None;
                        Lifecycle::Idle
                    }
                }
            }
        };
    }
}

#[derive(Clone)]
struct Snapshot {
    generation: u64,
    height_revision: u64,
    zone_revision: u64,
    biome_revision: u64,
    n: usize,
    cell: f32,
    sea: f32,
    cap: u32,
    start: [f32; 2],
    zones: Vec<Zone>,
    grid: Vec<u8>,
    dry: Vec<bool>,
    water_fingerprint: u64,
    sample: Vec<Sample>,
    existing: Vec<ExistingZoneContext>,
    partition: Option<partition::Partition>,
    biomes: Vec<BiomeContext>,
    height_range: [f32; 2],
    pois: Vec<[f32; 2]>,
    road_count: usize,
}
#[derive(Clone)]
struct Sample {
    h: f32,
    biome: u8,
    zone: u8,
    dry: bool,
}
#[derive(Clone, Serialize)]
struct ExistingZoneContext {
    id: u8,
    name: String,
    levels: [u32; 2],
    map_rgb: [u8; 3],
    painted_cells: usize,
    xz_bounds: Option<[f32; 4]>,
}

#[derive(Clone, Serialize)]
struct BiomeContext {
    id: u8,
    sampled_cells: usize,
    map_rgb: [u8; 3],
}

impl Snapshot {
    fn capture(world: &World) -> Result<Self, String> {
        let terrain = world.resource::<Terrain>();
        let gameplay = world.resource::<Gameplay>();
        let zones = world.resource::<Zones>();
        let water = world.resource::<crate::water::WaterBodies>();
        let roads = world.resource::<crate::roads::Roads>();
        if water.dirty {
            return Err("Wait for water to finish updating before auto zoning".into());
        }
        let start = gameplay.player_start.ok_or("Place a player start first")?;
        if !start.pos.into_iter().all(f32::is_finite)
            || !terrain.in_bounds(start.pos[0], start.pos[2])
            || terrain.height_at(start.pos[0], start.pos[2]) <= terrain.sea_level
        {
            return Err("Player start must be placed on dry terrain".into());
        }
        let cap = gameplay.level_cap.ok_or("Save a level cap first")?;
        let n = terrain.n();
        let mut dry = vec![false; n * n];
        for j in 0..n {
            for i in 0..n {
                let k = terrain.idx(i, j);
                let level = water.level_map.get(k).copied().unwrap_or(f32::NEG_INFINITY);
                let wet = level.is_finite();
                dry[k] = terrain.heights[k] > terrain.sea_level && !wet;
            }
        }
        let sg = terrain.world_to_grid(start.pos[0], start.pos[2]);
        let sk = terrain.idx(
            sg.x.round().clamp(0.0, (n - 1) as f32) as usize,
            sg.y.round().clamp(0.0, (n - 1) as f32) as usize,
        );
        if !dry[sk] {
            return Err(
                "Player start must be placed on dry terrain, outside lakes and rivers".into(),
            );
        }
        if terrain.zones[sk] != 0 {
            let zone = zones
                .list
                .iter()
                .find(|zone| zone.id == terrain.zones[sk])
                .ok_or("The player start is inside an undefined existing zone")?;
            if zone.level_min != 1 || zone.level_max > cap.min(5) {
                return Err(format!(
                    "Adjust existing starter zone '{}' to level 1-{} before auto zoning",
                    zone.name,
                    cap.min(5)
                ));
            }
        }
        let r2 = START_RADIUS_METRES * START_RADIUS_METRES;
        for j in 0..n {
            for i in 0..n {
                let id = terrain.zones[terrain.idx(i, j)];
                if id == 0 {
                    continue;
                }
                let p = terrain.grid_to_world(i, j);
                if (p.x - start.pos[0]).powi(2) + (p.y - start.pos[2]).powi(2) <= r2 {
                    let zone = zones
                        .list
                        .iter()
                        .find(|zone| zone.id == id)
                        .ok_or("The zone map contains an undefined existing zone ID")?;
                    if zone.level_max > cap.min(10) {
                        return Err(format!(
                            "Adjust existing zone '{}' near the player start to level {} or lower before auto zoning",
                            zone.name,
                            cap.min(10)
                        ));
                    }
                }
            }
        }
        let side = MAX_SIDE.min(n);
        let water_fingerprint = water_fingerprint(water);
        let mut sample = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let i = x * (n - 1) / (side - 1).max(1);
                let j = y * (n - 1) / (side - 1).max(1);
                let k = terrain.idx(i, j);
                sample.push(Sample {
                    h: terrain.heights[k],
                    biome: terrain.biomes[k],
                    zone: terrain.zones[k],
                    dry: dry[k],
                });
            }
        }
        let mut biome_counts = std::collections::BTreeMap::<u8, usize>::new();
        for biome in &terrain.biomes {
            *biome_counts.entry(*biome).or_default() += 1;
        }
        let height_range = terrain
            .heights
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |range, height| {
                [range[0].min(*height), range[1].max(*height)]
            });
        Ok(Self {
            generation: terrain.generation,
            height_revision: terrain.height_revision,
            zone_revision: terrain.zone_revision,
            biome_revision: terrain.biome_revision,
            n,
            cell: terrain.cell,
            sea: terrain.sea_level,
            cap,
            start: [start.pos[0], start.pos[2]],
            zones: zones.list.clone(),
            grid: terrain.zones.clone(),
            dry,
            water_fingerprint,
            sample,
            existing: zones
                .list
                .iter()
                .map(|zone| {
                    let mut count = 0usize;
                    let mut bounds: Option<[f32; 4]> = None;
                    for (k, id) in terrain.zones.iter().enumerate() {
                        if *id == zone.id {
                            let p = terrain.grid_to_world(k % n, k / n);
                            count += 1;
                            bounds = Some(match bounds {
                                Some(b) => {
                                    [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]
                                }
                                None => [p.x, p.y, p.x, p.y],
                            });
                        }
                    }
                    ExistingZoneContext {
                        id: zone.id,
                        name: zone.name.clone(),
                        levels: [zone.level_min, zone.level_max],
                        map_rgb: zone_color(zone.id),
                        painted_cells: count,
                        xz_bounds: bounds,
                    }
                })
                .collect(),
            partition: None,
            biomes: biome_counts
                .into_iter()
                .map(|(id, sampled_cells)| BiomeContext {
                    id,
                    sampled_cells,
                    map_rgb: biome_color(id, 180),
                })
                .collect(),
            height_range,
            pois: gameplay
                .pois
                .iter()
                .take(32)
                .map(|p| [p.pos[0], p.pos[2]])
                .collect(),
            road_count: roads.roads.len(),
        })
    }
    fn half(&self) -> f32 {
        (self.n - 1) as f32 * self.cell * 0.5
    }
    fn build_partition(&mut self) -> Result<(), String> {
        let sample_cell = (self.n - 1) as f32 * self.cell / (self.sample_side() - 1).max(1) as f32;
        self.partition = Some(partition::Partition::build(
            self.sample_side(),
            self.sample.iter().map(|s| s.dry && s.zone == 0).collect(),
            self.sample.iter().map(|s| s.h).collect(),
            self.sample.iter().map(|s| s.biome).collect(),
            self.start,
            self.half(),
            sample_cell,
        )?);
        Ok(())
    }
    fn sample_side(&self) -> usize {
        (self.sample.len() as f64).sqrt() as usize
    }
}

fn zone_color(id: u8) -> [u8; 3] {
    let h = id.wrapping_mul(47);
    [96 + h % 120, 72 + h % 100, 52 + h % 80]
}

fn biome_color(id: u8, elevation: u8) -> [u8; 3] {
    let brightness = elevation.saturating_sub(45);
    match id % 5 {
        1 => [brightness / 2, brightness, brightness / 2],
        2 => [brightness, brightness.saturating_mul(3) / 4, brightness / 3],
        3 => [
            brightness.saturating_mul(3) / 4,
            brightness.saturating_mul(3) / 4,
            brightness / 2,
        ],
        4 => [brightness / 2, brightness.saturating_mul(3) / 4, brightness],
        _ => [brightness.saturating_mul(2) / 3, brightness, brightness / 2],
    }
}

fn render_context_map(snapshot: &Snapshot) -> Result<PathBuf, String> {
    let partition = snapshot
        .partition
        .as_ref()
        .ok_or("Terrain candidates are not ready")?;
    let side = (snapshot.sample.len() as f64).sqrt() as usize;
    let mut img = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(MAP_SIDE, MAP_SIDE);
    let (lo, hi) = snapshot
        .sample
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |a, s| {
            (a.0.min(s.h), a.1.max(s.h))
        });
    for y in 0..MAP_SIDE {
        for x in 0..MAP_SIDE {
            let sx = (x as usize * side / MAP_SIDE as usize).min(side - 1);
            let sy = (y as usize * side / MAP_SIDE as usize).min(side - 1);
            let s = &snapshot.sample[sy * side + sx];
            let v = ((s.h - lo) / (hi - lo).max(0.001) * 150.0 + 55.0) as u8;
            let c = if !s.dry {
                [35, 94, 144]
            } else if s.zone > 0 {
                zone_color(s.zone)
            } else if partition.labels[sy * side + sx] != 0 {
                partition
                    .regions
                    .iter()
                    .find(|r| r.id == partition.labels[sy * side + sx])
                    .map_or([100, 100, 100], |r| r.map_rgb)
            } else {
                biome_color(s.biome, v)
            };
            img.put_pixel(x, y, Rgb(c));
        }
    }
    let gx = ((snapshot.start[0] + snapshot.half()) / ((snapshot.n - 1) as f32 * snapshot.cell)
        * MAP_SIDE as f32) as i32;
    let gy = ((snapshot.start[1] + snapshot.half()) / ((snapshot.n - 1) as f32 * snapshot.cell)
        * MAP_SIDE as f32) as i32;
    for y in gy - 5..=gy + 5 {
        for x in gx - 5..=gx + 5 {
            if x >= 0 && y >= 0 && x < MAP_SIDE as i32 && y < MAP_SIDE as i32 {
                img.put_pixel(x as u32, y as u32, Rgb([255, 255, 255]));
            }
        }
    }
    // The writable data root: the packaged app's cwd is `/`, a read-only volume.
    let dir = crate::paths::cache_dir("auto-zones").join(format!(
        "{}-{}",
        std::process::id(),
        fastrand::u32(..)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("terrain-context-north-up.png");
    for region in &partition.regions {
        let gx = ((region.anchor[0] + snapshot.half()) / ((snapshot.n - 1) as f32 * snapshot.cell)
            * MAP_SIDE as f32) as i32;
        let gy = ((region.anchor[1] + snapshot.half()) / ((snapshot.n - 1) as f32 * snapshot.cell)
            * MAP_SIDE as f32) as i32;
        draw_number(&mut img, gx, gy, region.id);
    }
    img.save(&path).map_err(|e| e.to_string())?;
    let metadata = serde_json::json!({
        "north_up": true,
        "world_xz_bounds": [-snapshot.half(), snapshot.half()],
        "candidate_regions": partition.regions,
    });
    std::fs::write(
        dir.join("terrain-candidates.json"),
        serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

fn draw_number(img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, x: i32, y: i32, value: u8) {
    const DIGITS: [[u8; 15]; 10] = [
        [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1],
        [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1],
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
        [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
        [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1],
        [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1],
        [1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0],
        [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1],
        [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1],
    ];
    let text = value.to_string();
    let left = x - (text.len() as i32 * 8) / 2;
    for (n, ch) in text.bytes().enumerate() {
        let glyph = DIGITS[(ch - b'0') as usize];
        for gy in 0..5 {
            for gx in 0..3 {
                if glyph[gy * 3 + gx] == 1 {
                    for py in -1..=1 {
                        for px in -1..=1 {
                            let xx = left + n as i32 * 8 + gx as i32 * 3 + px;
                            let yy = y - 7 + gy as i32 * 3 + py;
                            if xx >= 0 && yy >= 0 && xx < MAP_SIDE as i32 && yy < MAP_SIDE as i32 {
                                img.put_pixel(xx as u32, yy as u32, Rgb([255, 255, 255]));
                            }
                        }
                    }
                }
            }
        }
    }
}

fn water_fingerprint(water: &crate::water::WaterBodies) -> u64 {
    water
        .level_map
        .iter()
        .fold(0xcbf2_9ce4_8422u64, |h, level| {
            (h ^ level.to_bits() as u64).wrapping_mul(0x100_0000_01b3)
        })
}

fn begin_agent(
    world: &World,
    s: &Snapshot,
    image: PathBuf,
    correction: Option<&str>,
) -> Result<(AgentHandle, crate::mcp::PrivateEndpoint), String> {
    let partition = s
        .partition
        .as_ref()
        .ok_or("Terrain candidates are not ready")?;
    let p = world.resource::<PromptSession>();
    let backend = p.backend;
    let st = p.status(backend).cloned();
    let api = p.api_conn();
    if backend != Backend::Api
        && agent::stub_path().is_none()
        && !st.as_ref().is_some_and(|v| v.ready)
    {
        return Err("Selected language model is not connected".into());
    }
    if backend == Backend::Api && (!api.ready() || p.model().is_empty()) {
        return Err("Add an API key and pick a model first".into());
    }
    let endpoint = crate::mcp::PrivateEndpoint::open(&[])?;
    let defs = serde_json::to_value(&s.existing).unwrap_or_default();
    let max_zones = MAX_ZONES.min(
        (1..=63)
            .filter(|id| !s.zones.iter().any(|zone| zone.id == *id))
            .count(),
    );
    let adjacent_jump = (s.cap.saturating_add(9) / 10).max(5);
    let mut prompt = format!(
        "North-up candidate terrain map attached. It is a north-up terrain/world context map, not a 3D render; top is north (-Z). World x/z bounds [{}, {}]. Player start is [{}, {}]. Level cap is {}. You may return at most {} zones because that is the number of free IDs. Terrain elevation ranges from {:.1} to {:.1}; within each dry biome tint, brighter pixels are higher. Blue is water. Biome legend is {}: each map_rgb is its representative dry-terrain tint, and height changes its brightness. Existing zones must never be edited or painted over. Existing-zone legend is {}: each entry has its exact distinct map_rgb, painted_cells, and xz_bounds [min_x,min_z,max_x,max_z]. Candidate regions are the colored areas with visible white numeric labels. Candidate metadata is {}: map_rgb and anchor locate each fixed region; adjacent lists legal connected groups; start_distance/contains_start/touches_start_guard support a safe progression. Assign every candidate ID exactly once with connected groups. Existing road count: {}. POI x/z summaries: {}. Keep the 180m radius around start <=10. Neighboring level ranges should overlap or follow naturally: any gap between the higher level_min and the lower level_max must be at most {}. Keep level ranges meaningful; use wider midgame ranges when the geography branches rather than forcing a linear chain. Return only region ID groups, never polygons or coordinates.",
        -s.half(),
        s.half(),
        s.start[0],
        s.start[1],
        s.cap,
        max_zones,
        s.height_range[0],
        s.height_range[1],
        serde_json::to_string(&s.biomes).unwrap_or_default(),
        serde_json::to_string(&defs).unwrap_or_default(),
        serde_json::to_string(&partition.regions).unwrap_or_default(),
        s.road_count,
        serde_json::to_string(&s.pois).unwrap_or_default(),
        adjacent_jump
    );
    if let Some(error) = correction {
        prompt.push_str(" Your first response was rejected without changing the world: ");
        prompt.push_str(error);
        prompt.push_str(
            " Return one corrected JSON response that uses the same fixed candidate IDs.",
        );
    }
    let workdir = image
        .parent()
        .unwrap_or_else(|| std::path::Path::new("work"))
        .to_path_buf();
    let spec = agent::RunSpec {
        backend,
        model: p.model(),
        effort: p.effort(),
        features: st.as_ref().map(|x| x.features.clone()).unwrap_or_default(),
        exe: st.as_ref().and_then(|x| x.path.clone()),
        instructions: INSTRUCTIONS.into(),
        prompt,
        port: endpoint.port,
        api: Some(api),
        messages: vec![],
        resume: None,
        timeout: Duration::from_secs(180),
        workdir,
        images: vec![image],
    };
    Ok((agent::start(spec), endpoint))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    zones: Vec<ReplyZone>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyZone {
    name: String,
    #[serde(default)]
    description: String,
    level_min: u32,
    level_max: u32,
    #[serde(default)]
    zone_type: Option<String>,
    #[serde(default)]
    settings: ZoneSettings,
    region_ids: Vec<u8>,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoneSettings {
    allow_mounts: Option<bool>,
    allow_flying: Option<bool>,
    rested_area: Option<bool>,
    limit_placement: Option<bool>,
    visible: Option<bool>,
    temperature: Option<f32>,
    weather_intensity: Option<f32>,
    ambience_volume: Option<f32>,
    wind_speed: Option<f32>,
    cloud_cover: Option<f32>,
    weather: Option<String>,
    ambience: Option<String>,
    haze: Option<HazeSetting>,
    forest_diversity: Option<ForestSetting>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HazeSetting {
    mode: String,
    #[serde(default)]
    amount: Option<f32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForestSetting {
    enabled: bool,
    #[serde(default)]
    layout: Option<String>,
}
struct Proposal {
    list: Vec<Zone>,
    grid: Vec<u8>,
}

fn validate(s: &Snapshot, text: &str) -> Result<Proposal, String> {
    let partition = s
        .partition
        .as_ref()
        .ok_or("Terrain candidates are not ready")?;
    let a = text
        .find('{')
        .ok_or("The model did not return a zone plan")?;
    let b = text
        .rfind('}')
        .ok_or("The model did not return a zone plan")?;
    let answer: Reply = serde_json::from_str(&text[a..=b])
        .map_err(|e| format!("Auto-zone response was not valid JSON: {e}"))?;
    if answer.zones.is_empty() || answer.zones.len() > MAX_ZONES {
        return Err("Auto-zone response must contain 1 to 24 zones".into());
    }
    let mut free: Vec<u8> = (1..=63)
        .filter(|id| !s.zones.iter().any(|z| z.id == *id))
        .collect();
    if answer.zones.len() > free.len() {
        return Err("There are not enough free zone IDs".into());
    }
    if s.grid
        .iter()
        .any(|id| *id != 0 && !s.zones.iter().any(|z| z.id == *id))
    {
        return Err("The existing zone map contains an undefined zone ID".into());
    }
    let mut grid = s.grid.clone();
    let mut list = s.zones.clone();
    let mut assignments = std::collections::BTreeMap::<u8, u8>::new();
    for z in answer.zones {
        if z.name.trim().is_empty() || z.name.len() > 80 || z.description.len() > 320 {
            return Err("Zone names or descriptions were too long or empty".into());
        }
        if z.level_min < 1 || z.level_min > z.level_max || z.level_max > s.cap {
            return Err("Zone levels must be integral, ordered, and within the saved cap".into());
        }
        if z.region_ids.is_empty() || z.region_ids.len() > partition.regions.len() {
            return Err("Each zone must contain one or more candidate region IDs".into());
        }
        let ids: std::collections::BTreeSet<_> = z.region_ids.iter().copied().collect();
        if ids.len() != z.region_ids.len()
            || ids
                .iter()
                .any(|id| !partition.regions.iter().any(|r| r.id == *id))
            || !partition.connected(&z.region_ids)
        {
            return Err("A zone used unknown, duplicate, or disconnected candidate IDs".into());
        }
        let id = free.remove(0);
        for region in ids {
            if assignments.insert(region, id).is_some() {
                return Err("Each candidate region must be assigned exactly once".into());
            }
        }
        let mut zone = Zone::new(id, z.name.trim().into());
        zone.description = z.description;
        zone.level_min = z.level_min;
        zone.level_max = z.level_max;
        zone.zone_type = match z.zone_type.as_deref().unwrap_or("open_world") {
            "open_world" => ZoneType::OpenWorld,
            "sanctuary" => ZoneType::Sanctuary,
            "contested" => ZoneType::Contested,
            "city" => ZoneType::City,
            "dungeon" => ZoneType::Dungeon,
            "raid" => ZoneType::Raid,
            _ => return Err("A zone used an unknown zone_type".into()),
        };
        if !z
            .settings
            .temperature
            .is_none_or(|v| v.is_finite() && (-40.0..=55.0).contains(&v))
            || !z
                .settings
                .weather_intensity
                .is_none_or(|v| v.is_finite() && (0.1..=1.0).contains(&v))
            || !z
                .settings
                .ambience_volume
                .is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v))
            || !z
                .settings
                .wind_speed
                .is_none_or(|v| v.is_finite() && (0.0..=25.0).contains(&v))
            || !z
                .settings
                .cloud_cover
                .is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v))
        {
            return Err("A zone setting was outside its allowed range".into());
        }
        if let Some(v) = z.settings.allow_mounts {
            zone.allow_mounts = v;
        }
        if let Some(v) = z.settings.allow_flying {
            zone.allow_flying = v;
        }
        if let Some(v) = z.settings.rested_area {
            zone.rested_area = v;
        }
        if let Some(v) = z.settings.limit_placement {
            zone.limit_placement = v;
        }
        if let Some(v) = z.settings.visible {
            zone.visible = v;
        }
        if let Some(v) = z.settings.temperature {
            zone.temperature = v;
        }
        if let Some(v) = z.settings.weather_intensity {
            zone.weather_intensity = v;
        }
        if let Some(v) = z.settings.ambience_volume {
            zone.audio_volume = v;
        }
        if let Some(v) = z.settings.wind_speed {
            zone.wind_enabled = true;
            zone.wind_speed = v;
        }
        if let Some(v) = z.settings.cloud_cover {
            zone.clouds_enabled = true;
            zone.cloud_cover = v;
        }
        if let Some(v) = z.settings.weather.as_deref() {
            zone.weather = match v {
                "clear" => Weather::Clear,
                "cloudy" => Weather::Cloudy,
                "rain" => Weather::Rain,
                "storm" => Weather::Storm,
                "snow" => Weather::Snow,
                "fog" => Weather::Fog,
                "sandstorm" => Weather::Sandstorm,
                _ => return Err("A zone used an unknown weather setting".into()),
            };
            zone.weather_enabled = true;
        }
        if let Some(v) = z.settings.ambience.as_deref() {
            zone.soundscape = match v {
                "none" => Soundscape::None,
                "forest" => Soundscape::Forest,
                "meadow" => Soundscape::Meadow,
                "mountain" => Soundscape::Mountain,
                "coast" => Soundscape::Coast,
                "marsh" => Soundscape::Marsh,
                "desert" => Soundscape::Desert,
                "cave" => Soundscape::Cave,
                "frost" => Soundscape::Frost,
                "town" => Soundscape::Town,
                "ruins" => Soundscape::Ruins,
                "volcanic" => Soundscape::Volcanic,
                "eerie" => Soundscape::Eerie,
                "sunken" => Soundscape::Sunken,
                "infernal" => Soundscape::Infernal,
                _ => return Err("A zone used an unknown ambience setting".into()),
            };
            zone.audio_enabled = true;
        }
        if let Some(h) = z.settings.haze {
            zone.haze.mode = match h.mode.as_str() {
                "inherit" => crate::haze::HazeMode::Inherit,
                "on" => crate::haze::HazeMode::On,
                "off" => crate::haze::HazeMode::Off,
                _ => return Err("A zone used an unknown haze mode".into()),
            };
            if let Some(v) = h.amount {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err("Haze amount was outside its allowed range".into());
                }
                zone.haze.amount = v;
            }
        }
        if let Some(f) = z.settings.forest_diversity {
            zone.forest = Some(crate::nature::ForestDiversity {
                enabled: f.enabled,
                layout: match f.layout.as_deref().unwrap_or("natural_groves") {
                    "natural_groves" => crate::nature::ForestLayout::NaturalGroves,
                    "mixed" => crate::nature::ForestLayout::Mixed,
                    _ => return Err("A zone used an unknown forest layout".into()),
                },
            });
        }
        list.push(zone);
    }
    if assignments.len() != partition.regions.len() {
        return Err("Every candidate region must be assigned exactly once".into());
    }
    for j in 0..s.n {
        for i in 0..s.n {
            let k = j * s.n + i;
            if s.dry[k] && s.grid[k] == 0 {
                let candidate = partition.label_at(i, j, s.n);
                grid[k] = *assignments
                    .get(&candidate)
                    .ok_or("A terrain cell had no candidate region")?;
            }
        }
    }
    if s.dry
        .iter()
        .enumerate()
        .any(|(k, d)| *d && s.grid[k] == 0 && grid[k] == 0)
    {
        return Err("The proposal did not cover all unzoned dry land".into());
    }
    validate_progression(s, &list, &grid)?;
    Ok(Proposal { list, grid })
}

fn validate_progression(s: &Snapshot, list: &[Zone], grid: &[u8]) -> Result<(), String> {
    let lookup = |id| list.iter().find(|z| z.id == id);
    let sx = ((s.start[0] + s.half()) / s.cell)
        .round()
        .clamp(0.0, (s.n - 1) as f32) as usize;
    let sy = ((s.start[1] + s.half()) / s.cell)
        .round()
        .clamp(0.0, (s.n - 1) as f32) as usize;
    let starter =
        lookup(grid[sy * s.n + sx]).ok_or("The player start is not inside a proposed zone")?;
    if starter.level_min != 1 || starter.level_max > s.cap.min(5) {
        return Err("The player start must be in a level 1-5 zone".into());
    }
    let r2 = START_RADIUS_METRES * START_RADIUS_METRES;
    for j in 0..s.n {
        for i in 0..s.n {
            let x = i as f32 * s.cell - s.half();
            let z = j as f32 * s.cell - s.half();
            if (x - s.start[0]).powi(2) + (z - s.start[1]).powi(2) <= r2 && grid[j * s.n + i] != 0 {
                let q = lookup(grid[j * s.n + i]).unwrap();
                if q.level_max > s.cap.min(10) {
                    return Err("A high-level zone reaches too close to the player start".into());
                }
            }
            if i + 1 < s.n {
                adjacent(
                    s.cap,
                    lookup(grid[j * s.n + i]),
                    lookup(grid[j * s.n + i + 1]),
                )?;
            }
            if j + 1 < s.n {
                adjacent(
                    s.cap,
                    lookup(grid[j * s.n + i]),
                    lookup(grid[(j + 1) * s.n + i]),
                )?;
            }
        }
    }
    if !grid
        .iter()
        .any(|id| lookup(*id).is_some_and(|z| z.level_max == s.cap))
    {
        return Err("The progression does not reach the saved level cap".into());
    }
    Ok(())
}
fn adjacent(cap: u32, a: Option<&Zone>, b: Option<&Zone>) -> Result<(), String> {
    if let (Some(a), Some(b)) = (a, b) {
        let allowed = (cap.saturating_add(9) / 10).max(5);
        let gap = a
            .level_min
            .saturating_sub(b.level_max)
            .max(b.level_min.saturating_sub(a.level_max));
        if a.id != b.id && gap > allowed {
            return Err(format!(
                "Adjacent zones '{}' ({}-{}) and '{}' ({}-{}) have a gap larger than {} levels",
                a.name, a.level_min, a.level_max, b.name, b.level_min, b.level_max, allowed
            ));
        }
    }
    Ok(())
}
fn apply(world: &mut World, s: &Snapshot, plan: Proposal) -> Result<u64, String> {
    let t = world.resource::<Terrain>();
    let zones = world.resource::<Zones>();
    let gp = world.resource::<Gameplay>();
    if world.resource::<crate::water::WaterBodies>().dirty
        || t.generation != s.generation
        || t.height_revision != s.height_revision
        || t.zone_revision != s.zone_revision
        || t.biome_revision != s.biome_revision
        || t.sea_level != s.sea
        || t.zones != s.grid
        || zones.list != s.zones
        || gp.level_cap != Some(s.cap)
        || gp.player_start.map(|p| [p.pos[0], p.pos[2]]) != Some(s.start)
        || water_fingerprint(world.resource::<crate::water::WaterBodies>()) != s.water_fingerprint
    {
        return Err(
            "The world changed while auto zones was planning; no changes were applied".into(),
        );
    }
    if world.resource::<History>().is_recording() || world.resource::<History>().in_group() {
        return Err("Another edit started while auto zones was planning".into());
    }
    let mut terrain = world.remove_resource::<Terrain>().unwrap();
    let mut zones = world.remove_resource::<Zones>().unwrap();
    let before_grid = std::mem::replace(&mut terrain.zones, plan.grid);
    let before_list = std::mem::replace(&mut zones.list, plan.list);
    let before_selected = zones.selected;
    let n = terrain.n();
    terrain.mark_zone_dirty(GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    });
    world.insert_resource(terrain);
    world.insert_resource(zones);
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    let serial = world.resource_mut::<History>().record_zone_layout(
        "Auto zones",
        before_list,
        before_grid,
        before_selected,
    );
    Ok(serial)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_snapshot(cap: u32) -> Snapshot {
        let mut starter = Zone::new(1, "Existing starter".into());
        starter.level_min = 1;
        starter.level_max = 5;
        let mut grid = vec![0; 25];
        grid[12] = 1;
        let partition = partition::Partition::build(
            5,
            vec![true; 25],
            vec![0.0; 25],
            vec![0; 25],
            [0.0, 0.0],
            2.0,
            1.0,
        )
        .unwrap();
        Snapshot {
            generation: 1,
            height_revision: 1,
            zone_revision: 1,
            biome_revision: 1,
            n: 5,
            cell: 1.0,
            sea: 0.0,
            cap,
            start: [0.0, 0.0],
            zones: vec![starter],
            grid,
            dry: vec![true; 25],
            water_fingerprint: 0,
            sample: vec![],
            existing: vec![],
            partition: Some(partition),
            biomes: vec![],
            height_range: [0.0, 0.0],
            pois: vec![],
            road_count: 0,
        }
    }

    fn full_world_reply(snapshot: &Snapshot, max: u32) -> String {
        let ids: Vec<_> = snapshot
            .partition
            .as_ref()
            .unwrap()
            .regions
            .iter()
            .map(|r| r.id)
            .collect();
        serde_json::json!({"zones":[{"name":"New lands","level_min":1,"level_max":max,"region_ids":ids}]}).to_string()
    }

    #[test]
    fn rejects_unknown_llm_fields_instead_of_silently_ignoring_them() {
        let text = r#"{"zones":[{"name":"Start","level_min":1,"level_max":5,"region_ids":[1],"unsafe":true}]}"#;
        assert!(serde_json::from_str::<Reply>(text).is_err());
    }

    #[test]
    fn rejects_duplicate_or_disconnected_candidate_groups() {
        let snapshot = small_snapshot(10);
        let duplicate =
            r#"{"zones":[{"name":"New","level_min":1,"level_max":10,"region_ids":[1,1,2]}]}"#;
        assert!(validate(&snapshot, duplicate).is_err());
    }

    #[test]
    fn adjacent_ranges_may_overlap_but_not_skip_a_large_band() {
        let mut low = Zone::new(1, "Low".into());
        low.level_min = 1;
        low.level_max = 10;
        let mut next = Zone::new(2, "Next".into());
        next.level_min = 9;
        next.level_max = 29;
        assert!(adjacent(60, Some(&low), Some(&next)).is_ok());
        next.level_min = 11;
        next.level_max = 35;
        assert!(adjacent(60, Some(&low), Some(&next)).is_ok());
        next.level_min = 40;
        next.level_max = 60;
        assert!(adjacent(60, Some(&low), Some(&next)).is_err());
    }

    #[test]
    fn auto_undo_requires_its_exact_latest_history_step_and_no_open_edit() {
        let mut history = History::default();
        let serial = history.record_zone_layout("Auto zones", vec![], vec![], None);
        let mut auto = AutoZones::default();
        auto.applied = Some((serial, 7));
        assert!(auto.can_undo(&history, 7));
        assert!(!auto.can_undo(&history, 8));
        history.begin("Other edit");
        assert!(!auto.can_undo(&history, 7));
    }

    #[test]
    fn validated_plan_preserves_existing_zone_definition_and_painted_cell() {
        let snapshot = small_snapshot(10);
        let before = snapshot.zones[0].clone();
        let plan = validate(&snapshot, &full_world_reply(&snapshot, 10)).unwrap();
        assert_eq!(plan.list[0], before);
        assert_eq!(plan.grid[12], 1);
        assert!(
            plan.grid
                .iter()
                .enumerate()
                .all(|(i, id)| i == 12 || *id == 2)
        );
    }

    #[test]
    fn unpainted_cap_definition_does_not_satisfy_progression() {
        let mut snapshot = small_snapshot(10);
        let mut unpainted = Zone::new(2, "Unused cap".into());
        unpainted.level_min = 10;
        unpainted.level_max = 10;
        snapshot.zones.push(unpainted);
        assert!(validate(&snapshot, &full_world_reply(&snapshot, 5)).is_err());
    }

    #[test]
    fn wide_or_high_start_radius_plan_is_rejected() {
        let snapshot = small_snapshot(20);
        assert!(validate(&snapshot, &full_world_reply(&snapshot, 20)).is_err());
    }
}
