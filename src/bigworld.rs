//! Big, realistic landmasses: erosion of the whole world (or an area / polygon, and the Sculpt
//! tab's Erode brush), the continent / coastline generator, worlds from 2 to 16 km, the
//! background flow map behind the ground's wet gullies, and their MCP tools.
//!
//! Whole-world jobs run on a worker thread behind a progress bar with Cancel and land as one
//! undo step (heights only, so undoing a 16 km erosion costs 64 MB, not 300). The flow map is
//! derived data: it is recomputed in the background whenever the heights settle after an edit.
//!
//! Test hooks (timings are logged with the tag `BIGW`):
//! - `WB_ERODE=<preset>[,strength]` erodes the whole world at frame 300 (or `WB_ERODE_FRAME`);
//! - `WB_CONTINENT=<preset>[,size_km[,cell m[,seed]]]` generates a new world with biomes at
//!   startup (`WB_CONTINENT_BARE=1`: bare land); once it is ready, `WB_BW_WALK=<frames>` shows the
//!   overview that long, then walks from gentle land near the centre, and `WB_BW_IOTEST=<dir>` saves it, loads it back and compares;
//! - `WB_BW_SHEET=<dir>[,km]` generates every continent preset in turn (with biomes, through the
//!   MCP code path), screenshots each from above into `<dir>/<preset>.png`, then quits;
//! - `WB_BW_BRUSH=<preset>` selects the Erode brush (drive it with `WB_STROKE`);
//! - `WB_BW_DIALOG=<open frame>,<close frame>` shows the New World dialog in between;
//! - `WB_BW_WET=<frame>:<strength>,…` sets the style's wet gullies at those frames;
//! - `WB_BW_UNDO=<frame>` undoes then and logs the heights checksum (jobs log it when they start);
//! - `WB_MEM=1` logs the process memory every 5 s.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use serde_json::{Value, json};

use crate::biomegen::Climate;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};
use crate::objects::Category;
use crate::terrain::continent::{self, ContinentParams, ContinentPreset};
use crate::terrain::erosion::{self, ErosionParams, ErosionPreset, Progress};
use crate::terrain::{GridRect, Terrain};
use crate::tools::Stroke;
use crate::ui::UiAction;

/// World sizes offered for new worlds: (km, terrain cell sizes in metres).
pub const SIZES: [(u32, &[f32]); 4] = [
    (2, &[2.0, 4.0]),
    (4, &[2.0, 4.0, 8.0]),
    (8, &[2.0, 4.0, 8.0]),
    (16, &[4.0, 8.0]),
];

/// Memory the editor uses with a world of `res` cells of `cell` m (GB, measured on an M2 Max):
/// the app itself, ~52 bytes per terrain vertex (heights, paint, maps and their GPU textures)
/// and, with biomes, ~3 MB of trees, plants and rocks per km².
pub fn memory_gb(res: usize, cell: f32, biomes: bool) -> f32 {
    let n = (res + 1) as f32;
    let km2 = (res as f32 * cell / 1000.0).powi(2);
    1.65 + n * n * 52e-9 + if biomes { km2 * 0.003 } else { 0.0 }
}

/// How long making (and eroding) a world of this many cells per side takes.
pub fn perf_hint(res: usize) -> &'static str {
    match res {
        0..=1024 => "Ready in a few seconds",
        1025..=2048 => "Ready in about 15 s; whole-world erosion about 10 s",
        _ => {
            "Ready in about 30 s; whole-world erosion about 20 s. Walk mode streams it like any other world"
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewShape {
    Continent(ContinentPreset),
    /// An empty ocean or flat plains (the classic blank canvases).
    Blank(crate::terrain::worldgen::WorldPreset),
}

/// Requests from the UI.
pub enum Request {
    ErodeWorld,
    /// A new landmass from `BigWorld::continent`: a new world of (cells, cell size) or in place.
    Generate {
        new_world: Option<(usize, f32)>,
        biomes: Option<Climate>,
        art_style: Option<crate::art_style::ArtStyle>,
        appearance: Option<crate::nature::NatureAppearance>,
    },
}

/// Settings and running jobs (UI state; not saved with the world).
#[derive(Resource)]
pub struct BigWorld {
    pub erosion_preset: ErosionPreset,
    /// Strength of the erosion buttons and brush (×).
    pub erosion_strength: f32,
    pub continent: ContinentParams,
    /// New world: shape, size (km) and cell size (m).
    pub new_shape: NewShape,
    pub size_km: u32,
    pub cell: f32,
    pub requests: Vec<Request>,
    job: Option<Job>,
    /// A generated world waiting for its biomes (they run a frame later, under a message).
    biomes_next: Option<(u32, Climate, Instant)>,
    flow: FlowState,
    /// A generated world is complete (test hooks wait for it).
    world_ready: bool,
}

impl Default for BigWorld {
    fn default() -> Self {
        Self {
            erosion_preset: ErosionPreset::HeavyRain,
            erosion_strength: 1.0,
            continent: ContinentPreset::Continent.params(7),
            new_shape: NewShape::Continent(ContinentPreset::Continent),
            size_km: 8,
            cell: 4.0,
            requests: vec![],
            job: None,
            biomes_next: None,
            flow: FlowState::default(),
            world_ready: false,
        }
    }
}

impl BigWorld {
    /// Erosion settings of the selected preset at the chosen strength.
    pub fn erosion_params(&self) -> ErosionParams {
        let mut p = self.erosion_preset.params();
        p.strength = self.erosion_strength;
        p
    }
    /// Cells per side of the chosen new world size.
    pub fn new_res(&self) -> usize {
        (self.size_km as f32 * 1024.0 / self.cell).round() as usize
    }
    /// A world job is running (its label and progress).
    pub fn busy(&self) -> Option<(String, f32)> {
        if let Some(j) = &self.job {
            return Some((j.label.clone(), j.progress.get()));
        }
        self.biomes_next
            .as_ref()
            .map(|_| ("Growing biomes, forests and plants".to_string(), 1.0))
    }
}

struct Job {
    label: String,
    progress: Arc<Progress>,
    started: Instant,
    generation: u64,
    kind: JobKind,
    handle: JoinHandle<Option<(Vec<f32>, Vec<u8>)>>,
}

enum JobKind {
    Erode {
        name: &'static str,
    },
    Generate {
        name: String,
        seed: u32,
        new_world: Option<(usize, f32)>,
        biomes: Option<Climate>,
        art_style: Option<crate::art_style::ArtStyle>,
        appearance: Option<crate::nature::NatureAppearance>,
    },
}

#[derive(Default)]
struct FlowState {
    job: Option<((u64, u64), JoinHandle<Vec<u8>>)>,
    /// (height revision, generation) the flow map was made from
    done: Option<(u64, u64)>,
    seen: (u64, u64),
    changed_at: f32,
    wet: f32,
}

impl FlowState {
    /// The terrain's flow map is current (a job delivered it with its heights).
    fn mark_current(&mut self, t: &Terrain) {
        self.done = Some((t.height_revision, t.generation));
    }
}

pub struct BigWorldPlugin;

impl Plugin for BigWorldPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BigWorld>()
            .add_systems(
                Update,
                (start_jobs, finish_jobs, update_flow, test_hooks, io_hook)
                    .chain()
                    .after(crate::tools::ToolSystems::Commit),
            )
            .add_systems(EguiPrimaryContextPass, job_overlay)
            .add_systems(Update, preset_sheet);
        app.mcp_tool(McpTool {
            name: "terrain_erode",
            description: "Erode the terrain realistically (stream-power rain erosion with sediment transport, glaciers, wind, thermal scree). \
                Use a preset (light_rain, heavy_rain, ancient_mountains, glacial, desert_wind) and/or explicit params; optionally limit it to \
                an `area` {center: [x, z], radius} or a `polygon` [[x, z], ...] in world metres (soft edges). One undo step. Returns timing and how much \
                ground moved.",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "preset": {"type": "string", "enum": ErosionPreset::ALL.map(|p| p.id())},
                        "strength": {"type": "number", "description": "× preset strength (0.1..3, default 1)"},
                        "params": {"type": "object", "description": "Overrides of the preset's values", "properties": erosion_params_schema()},
                        "area": {"type": "object", "description": "a circle", "properties": {"center": crate::mcp_tools::position_schema("centre"), "radius": {"type": "number"}}, "required": ["radius"], "x-aliases": crate::mcp_tools::xz_aliases()},
                        "polygon": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}}
                    }
                })
            },
            run: mcp_erode,
        });
        app.mcp_tool(McpTool {
            name: "world_generate_continent",
            description: "Generate a landmass: continent, archipelago, twin_islands, peninsula, ring_atoll, fjord_coast, mountain_island or \
                inland_sea, with natural coastlines, mountain ranges and erosion, plus climate-driven biomes and nature. Either replaces the \
                current terrain in place (undoable; keeps its size) or creates a new world of size_km (2, 4, 8, 16) and cell (2, 4, 8 m).",
            input_schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "preset": {"type": "string", "enum": ContinentPreset::ALL.map(|p| p.id())},
                        "seed": {"type": "integer"},
                        "land": {"type": "number", "description": "share of the map that is land, 0.05..0.9"},
                        "roughness": {"type": "number", "description": "coast raggedness 0..1"},
                        "ranges": {"type": "integer", "description": "mountain ranges 0..8"},
                        "range_dir": crate::mcp_tools::bearing_schema("which way the mountain ranges run (90: east-west)"),
                        "relief": {"type": "number", "description": "mountain height ×, 0.2..2"},
                        "erosion": {"type": ["string", "null"], "description": "erosion preset id, or null for none"},
                        "new_world": {"type": "boolean", "description": "create a new world (default: replace the current terrain)"},
                        "size_km": {"type": "integer", "enum": [2, 4, 8, 16]},
                        "cell": {"type": "number", "enum": [2, 4, 8]},
                        "biomes": {"type": "boolean", "description": "grow biomes, ground and nature (default true)"},
                        "climate": {"type": "string", "enum": Climate::ALL.map(|c| c.name().to_lowercase())}
                    },
                    "required": ["preset"]
                })
            },
            run: mcp_generate,
        });
    }
}

/// Sum of all heights (test logs: undo restores it exactly).
fn checksum(t: &Terrain) -> f64 {
    t.heights.iter().map(|h| *h as f64).sum()
}

fn full(t: &Terrain) -> GridRect {
    GridRect {
        x0: 0,
        z0: 0,
        x1: t.n() - 1,
        z1: t.n() - 1,
    }
}

/// Starts queued requests (and erosion asked for through `UiAction::ErodeWorld`).
fn start_jobs(
    mut bw: ResMut<BigWorld>,
    mut actions: MessageReader<UiAction>,
    terrain: Res<Terrain>,
    mut ed: ResMut<crate::editor::Editor>,
    stroke: Res<Stroke>,
) {
    for a in actions.read() {
        if matches!(a, UiAction::ErodeWorld) {
            bw.requests.push(Request::ErodeWorld);
        }
    }
    if bw.requests.is_empty() || stroke.active {
        return;
    }
    if bw.job.is_some() || bw.biomes_next.is_some() {
        bw.requests.clear();
        ed.toast("Still working on the last world job…");
        return;
    }
    let req = bw.requests.remove(0);
    bw.requests.clear();
    let progress = Arc::new(Progress::default());
    let pr = progress.clone();
    let (label, kind, handle) = match req {
        Request::ErodeWorld => {
            let p = bw.erosion_params();
            let mut heights = terrain.heights.clone();
            let (n, cell, sea) = (terrain.n(), terrain.cell, terrain.sea_level);
            let label = format!("Eroding the world: {}", bw.erosion_preset.name());
            let h = std::thread::spawn(move || {
                let all = GridRect {
                    x0: 0,
                    z0: 0,
                    x1: n - 1,
                    z1: n - 1,
                };
                if !erosion::erode(
                    &mut heights,
                    n,
                    cell,
                    sea,
                    all,
                    None,
                    &p,
                    pr.stage(0.0, 0.95),
                ) {
                    return None;
                }
                let flow = erosion::flow_map(&heights, n, cell, sea);
                Some((heights, flow))
            });
            (
                label,
                JobKind::Erode {
                    name: bw.erosion_preset.name(),
                },
                h,
            )
        }
        Request::Generate {
            new_world,
            biomes,
            art_style,
            appearance,
        } => {
            let p = bw.continent.clone();
            let (res, cell, sea) = match new_world {
                Some((r, c)) => (r, c, 0.0),
                None => (terrain.res, terrain.cell, terrain.sea_level),
            };
            let name = p.preset.name().to_string();
            let label = match new_world {
                Some((r, c)) => format!("Creating a {:.0} km {name}", r as f32 * c / 1000.0),
                None => format!("Generating a {name}"),
            };
            let seed = p.seed;
            let h = std::thread::spawn(move || {
                let heights = continent::generate(&p, res, cell, sea, pr.stage(0.0, 0.95))?;
                let flow = erosion::flow_map(&heights, res + 1, cell, sea);
                Some((heights, flow))
            });
            (
                label,
                JobKind::Generate {
                    name,
                    seed,
                    new_world,
                    biomes,
                    art_style,
                    appearance,
                },
                h,
            )
        }
    };
    info!(
        "BIGW start: {label} (heights checksum {:.1})",
        checksum(&terrain)
    );
    bw.job = Some(Job {
        label,
        progress,
        started: Instant::now(),
        generation: terrain.generation,
        kind,
        handle,
    });
}

/// Applies finished jobs (one undo step each) and grows biomes for generated worlds.
fn finish_jobs(mut bw: ResMut<BigWorld>, mut data: WorldData) {
    if let Some((seed, climate, t0)) = bw.biomes_next.take() {
        grow_biomes(&mut data, seed, climate, t0);
        bw.world_ready = true;
        data.gameplay.ore_fill_pending = true;
        return;
    }
    if bw.job.as_ref().is_none_or(|j| !j.handle.is_finished()) {
        return;
    }
    let job = bw.job.take().unwrap();
    let secs = job.started.elapsed().as_secs_f32();
    let Ok(Some((heights, flow))) = job.handle.join() else {
        info!("BIGW cancelled: {}", job.label);
        data.ed.toast("Cancelled: the terrain is unchanged");
        return;
    };
    if job.generation != data.terrain.generation
        && !matches!(
            job.kind,
            JobKind::Generate {
                new_world: Some(_),
                ..
            }
        )
    {
        data.ed
            .toast("The world changed while working: result dropped");
        return;
    }
    info!("BIGW done: {} in {secs:.2} s", job.label);
    match job.kind {
        JobKind::Erode { name } => {
            let label = format!("Erode world ({name})");
            let moved = apply_heights(&mut data, &label, heights, flow, None);
            bw.flow.mark_current(&data.terrain);
            data.ed.toast(format!("Eroded the world in {secs:.1} s ({:.1} million m³ of rock and soil moved). Undo to go back", moved / 1.0e6));
        }
        JobKind::Generate {
            name,
            seed,
            new_world,
            biomes,
            art_style,
            appearance,
        } => {
            if let Some((res, cell)) = new_world {
                crate::io::new_world_from_styled(
                    &mut data,
                    heights,
                    res,
                    cell,
                    seed,
                    art_style.unwrap_or_default(),
                );
                if let Some(appearance) = appearance {
                    *data.appearance = appearance.sanitized();
                }
                fit_snow_line(&mut data.terrain);
                data.terrain.flow = flow;
                bw.flow.mark_current(&data.terrain);
                data.ed.toast(format!(
                    "Created a new {:.0} km {name} in {secs:.1} s",
                    res as f32 * cell / 1000.0
                ));
                match biomes {
                    Some(c) => bw.biomes_next = Some((seed, c, Instant::now())),
                    None => {
                        bw.world_ready = true;
                        data.gameplay.ore_fill_pending = true;
                    }
                }
            } else {
                let made = regenerate_in_place(&mut data, &name, heights, flow, seed, biomes);
                data.gameplay.ore_fill_pending = true;
                bw.flow.mark_current(&data.terrain);
                data.ed.toast(format!("Generated a {name} in {secs:.1} s with {made} biome regions and their nature (undo to go back)"));
            }
        }
    }
}

/// A new world's snow caps only its highest summits (a quarter of the relief).
fn fit_snow_line(t: &mut Terrain) {
    let peak = t.heights.iter().copied().fold(f32::MIN, f32::max) - t.sea_level;
    t.style.snow_line = (peak * 0.75).max(t.style.snow_line);
}

/// Biomes, ground and nature for a freshly created world (no undo: it's a new world).
fn grow_biomes(data: &mut WorldData, seed: u32, climate: Climate, t0: Instant) {
    data.history.begin("Generate biomes");
    let g = crate::biomegen::generate(
        &mut data.terrain,
        &mut data.zones,
        &mut data.history,
        &mut data.water,
        seed,
        climate,
    );
    data.history.commit(&data.store);
    data.history.clear();
    info!(
        "BIGW biomes: {} regions in {:.2} s",
        g.regions,
        t0.elapsed().as_secs_f32()
    );
    data.ed.toast(format!(
        "New world ready with {} biome regions and their forests, plants and rocks",
        g.regions
    ));
}

/// Heights from a job become one undo step (heights only). Returns the volume moved (m³).
fn apply_heights(
    data: &mut WorldData,
    label: &str,
    heights: Vec<f32>,
    flow: Vec<u8>,
    rect: Option<GridRect>,
) -> f64 {
    let t = &mut *data.terrain;
    let rect = rect.unwrap_or(full(t));
    data.history.begin(label);
    data.history.touch_heights(t, rect);
    let area = (t.cell * t.cell) as f64;
    let moved: f64 = t
        .heights
        .iter()
        .zip(&heights)
        .map(|(a, b)| (a - b).abs() as f64)
        .sum::<f64>()
        * area
        * 0.5;
    t.heights = heights;
    t.mark_heights_dirty(rect);
    set_flow(t, flow);
    data.history.commit(&data.store);
    data.meta.dirty = true;
    moved
}

/// Installs a new flow map, refreshing the GPU and the grass where it changed.
fn set_flow(t: &mut Terrain, flow: Vec<u8>) {
    if flow.len() != t.heights.len() {
        return;
    }
    let n = t.n();
    let (mut x0, mut z0, mut x1, mut z1) = (usize::MAX, usize::MAX, 0, 0);
    for (k, (a, b)) in t.flow.iter().zip(&flow).enumerate() {
        if a.abs_diff(*b) > 6 {
            let (i, j) = (k % n, k / n);
            x0 = x0.min(i);
            x1 = x1.max(i);
            z0 = z0.min(j);
            z1 = z1.max(j);
        }
    }
    t.flow = flow;
    if x0 <= x1 && z0 <= z1 {
        let r = GridRect { x0, z0, x1, z1 }.expand(2, n);
        t.dirty_flow = Some(t.dirty_flow.map_or(r, |d| d.union(r)));
        t.dirty_grass = Some(t.dirty_grass.map_or(r, |d| d.union(r)));
    }
}

/// A new landmass on the current world, like the Land tab's Generate: the old forests, rivers and
/// lakes go with the old land; biomes (or fresh nature) grow on the new one. One undo step.
fn regenerate_in_place(
    data: &mut WorldData,
    name: &str,
    heights: Vec<f32>,
    flow: Vec<u8>,
    seed: u32,
    biomes: Option<Climate>,
) -> usize {
    let is_nature = |lib: &crate::objects::Library, o: &crate::objects::Placed| {
        lib.get(&o.asset)
            .is_some_and(|e| Category::NATURE.contains(&e.category))
    };
    let t = &mut *data.terrain;
    let all = full(t);
    data.history.begin(format!("Generate {name}"));
    data.history.touch_terrain(t, all);
    t.heights = heights;
    t.flow = flow;
    t.mark_all_dirty();
    data.meta.seed = seed;
    if let Some(lib) = data.lib.as_deref() {
        let ids: Vec<u64> = data
            .store
            .items
            .values()
            .filter(|o| is_nature(lib, o))
            .map(|o| o.id)
            .collect();
        for id in ids {
            data.history.touch_object(&data.store, id);
            data.store.remove(id);
        }
    }
    data.history.touch_water(&data.water);
    data.water.rivers.clear();
    data.water.lakes.clear();
    data.water.waterfalls.clear();
    let (terrain, water) = (&data.terrain, &mut data.water);
    water
        .fish
        .retain(|f| terrain.height_at(f.center[0], f.center[2]) < terrain.sea_level - 1.0);
    water.selected = None;
    water.dirty = true;
    // procedural nature follows the new biomes (or, in a populated world, the new land)
    crate::nature::reset_paint(&mut data.nature, &mut data.history, "Generate");
    let made = biomes
        .map(|climate| {
            crate::biomegen::generate(
                &mut data.terrain,
                &mut data.zones,
                &mut data.history,
                &mut data.water,
                seed,
                climate,
            )
            .regions
        })
        .unwrap_or(0);
    data.history.commit(&data.store);
    data.meta.dirty = true;
    made
}

/// Keeps `Terrain::flow` in step with the heights: recomputed in the background once edits settle.
fn update_flow(
    mut bw: ResMut<BigWorld>,
    mut terrain: ResMut<Terrain>,
    stroke: Res<Stroke>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed_secs();
    let busy = bw.job.is_some();
    let fs = &mut bw.flow;
    if (terrain.style.wet_gullies - fs.wet).abs() > 1e-4 {
        fs.wet = terrain.style.wet_gullies;
        terrain.dirty_flow = Some(full(&terrain));
    }
    let key = (terrain.height_revision, terrain.generation);
    if fs.seen != key {
        fs.seen = key;
        fs.changed_at = now;
    }
    if let Some((made_from, h)) = fs.job.take() {
        if !h.is_finished() {
            fs.job = Some((made_from, h));
        } else if let Ok(flow) = h.join()
            && made_from.1 == terrain.generation
        {
            set_flow(&mut terrain, flow);
            fs.done = Some(made_from);
        }
        return;
    }
    if fs.done == Some(key) || stroke.active || busy || now - fs.changed_at < 0.5 {
        return;
    }
    let (heights, n, cell, sea) = (
        terrain.heights.clone(),
        terrain.n(),
        terrain.cell,
        terrain.sea_level,
    );
    fs.job = Some((
        key,
        std::thread::spawn(move || erosion::flow_map(&heights, n, cell, sea)),
    ));
}

/// The progress of a world job: a modal card with a bar and Cancel.
fn job_overlay(mut contexts: EguiContexts, bw: Res<BigWorld>) {
    let Some((label, p)) = bw.busy() else { return };
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let elapsed = bw
        .job
        .as_ref()
        .map(|j| j.started.elapsed().as_secs_f32())
        .unwrap_or(0.0);
    egui::Modal::new(egui::Id::new("bigworld_job"))
        .backdrop_color(Color32::from_black_alpha(90))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                RichText::new(&label)
                    .font(FontId::proportional(16.0))
                    .strong(),
            );
            ui.add_space(8.0);
            let left = if p > 0.03 {
                (elapsed / p * (1.0 - p)).max(0.0)
            } else {
                0.0
            };
            let text = if bw.job.is_some() {
                format!("{:.0}%  ·  {:.0} s left", p * 100.0, left.ceil())
            } else {
                "Almost done…".into()
            };
            ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(360.0)
                    .text(text)
                    .corner_radius(CornerRadius::same(6))
                    .fill(egui::Color32::from_white_alpha(80)),
            );
            ui.add_space(8.0);
            if let Some(j) = &bw.job {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Cancel leaves the terrain as it was.")
                            .color(Color32::GRAY)
                            .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            j.progress.cancel();
                        }
                    });
                });
            }
        });
}

/// Test hooks (see the module docs).
#[allow(clippy::too_many_arguments)]
fn test_hooks(
    mut frame: Local<u32>,
    mut bw: ResMut<BigWorld>,
    time: Res<Time<Real>>,
    mut next_mem: Local<f32>,
    mut walk_at: Local<Option<u32>>,
    mut terrain: ResMut<Terrain>,
    mut orbit: ResMut<crate::camera::OrbitCamera>,
    mut actions: MessageWriter<UiAction>,
    mut ed: ResMut<crate::editor::Editor>,
    mut undo: MessageWriter<crate::tools::EditorAction>,
) {
    *frame += 1;
    // WB_BW_UNDO=<frame>: undo then, and log the heights checksum (compare with the job's start)
    if let Some(f) = std::env::var("WB_BW_UNDO")
        .ok()
        .and_then(|f| f.parse::<u32>().ok())
    {
        if *frame == f {
            info!(
                "BIGW before undo: heights checksum {:.1}",
                checksum(&terrain)
            );
            undo.write(crate::tools::EditorAction::Undo);
        }
        if *frame == f + 3 {
            info!(
                "BIGW after undo: heights checksum {:.1}",
                checksum(&terrain)
            );
        }
    }
    // WB_BW_WET=<frame>:<strength>,…: set the style's wet gullies at those frames
    if let Ok(spec) = std::env::var("WB_BW_WET") {
        for (f, v) in spec.split(',').filter_map(|kv| kv.split_once(':')) {
            if f.parse::<u32>().ok() == Some(*frame)
                && let Ok(v) = v.parse::<f32>()
            {
                terrain.style.wet_gullies = v;
            }
        }
    }
    // WB_BW_DIALOG=<open frame>,<close frame>: show the New World dialog in between
    if let Ok(spec) = std::env::var("WB_BW_DIALOG") {
        let f: Vec<u32> = spec.split(',').filter_map(|v| v.parse().ok()).collect();
        if f.first() == Some(&*frame) {
            ed.open_new_world();
        }
        if f.get(1) == Some(&*frame) {
            ed.show_new_world = false;
        }
    }
    // WB_BW_BRUSH=<preset>: the Sculpt tab's Erode brush with that preset (drive it with WB_STROKE)
    if *frame == 2
        && let Ok(p) = std::env::var("WB_BW_BRUSH")
    {
        ed.tab = crate::editor::Tab::Sculpt;
        ed.sculpt_tool = crate::editor::SculptTool::Erode;
        bw.erosion_preset = ErosionPreset::from_id(&p).unwrap_or_default();
    }
    // WB_BW_WALK=<frames>: once the generated world is ready, walk from gentle land near the centre
    if std::env::var("WB_BW_WALK").is_ok() && bw.world_ready && walk_at.is_none() {
        info!("BIGW world ready");
        // (the overview of the new world stays up for the given number of frames first)
        let delay: u32 = std::env::var("WB_BW_WALK")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        *walk_at = Some(*frame + delay.max(10));
    }
    if walk_at.is_some_and(|f| f == *frame) {
        let t = &*terrain;
        let mut spot = Vec2::ZERO;
        'search: for r in (0..(t.half() as i32)).step_by(40) {
            for k in 0..16 {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                let p = Vec2::new(a.cos(), a.sin()) * r as f32;
                let h = t.height_at(p.x, p.y) - t.sea_level;
                if h > 4.0 && h < t.style.snow_line * 0.7 && t.normal_at(p.x, p.y).y > 0.9 {
                    spot = p;
                    break 'search;
                }
            }
        }
        info!("BIGW walk test from {spot:?}");
        orbit.set_view(Vec3::new(spot.x, 0.0, spot.y), 0.6, 0.3, 40.0);
        orbit.snap();
    }
    if walk_at.is_some_and(|f| f + 5 == *frame) {
        actions.write(UiAction::ToggleWalk);
    }
    if std::env::var("WB_MEM").is_ok() && time.elapsed_secs() > *next_mem {
        *next_mem = time.elapsed_secs() + 5.0;
        let pid = std::process::id().to_string();
        if let Ok(o) = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid])
            .output()
        {
            let kb: f32 = String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0.0);
            info!("BIGW memory {:.0} MB", kb / 1024.0);
        }
    }
    if *frame
        == std::env::var("WB_ERODE_FRAME")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or(300)
        && let Ok(spec) = std::env::var("WB_ERODE")
    {
        let mut it = spec.split(',');
        if let Some(p) = it.next().and_then(ErosionPreset::from_id) {
            bw.erosion_preset = p;
        }
        if let Some(s) = it.next().and_then(|s| s.parse().ok()) {
            bw.erosion_strength = s;
        }
        bw.requests.push(Request::ErodeWorld);
    }
    if *frame == 5
        && let Ok(spec) = std::env::var("WB_CONTINENT")
    {
        let v: Vec<&str> = spec.split(',').collect();
        let preset = v
            .first()
            .and_then(|s| ContinentPreset::from_id(s))
            .unwrap_or_default();
        let km: u32 = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(4);
        let cell: f32 = v.get(2).and_then(|s| s.parse().ok()).unwrap_or(4.0);
        let seed: u32 = v.get(3).and_then(|s| s.parse().ok()).unwrap_or(7);
        bw.continent = preset.params(seed);
        let res = (km as f32 * 1024.0 / cell).round() as usize;
        let biomes = std::env::var("WB_CONTINENT_BARE")
            .is_err()
            .then_some(Climate::Varied);
        bw.requests.push(Request::Generate {
            new_world: Some((res, cell)),
            biomes,
            art_style: None,
            appearance: None,
        });
    }
}

/// `WB_BW_SHEET` (see the module docs).
fn preset_sheet(world: &mut World) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static FRAME: AtomicU32 = AtomicU32::new(0);
    let Ok(spec) = std::env::var("WB_BW_SHEET") else {
        return;
    };
    let (dir, km) = spec
        .split_once(',')
        .map_or((spec.as_str(), 4), |(d, k)| (d, k.parse().unwrap_or(4)));
    let f = FRAME.fetch_add(1, Ordering::Relaxed);
    if f < 60 || world.get_resource::<crate::objects::Library>().is_none() {
        return;
    }
    let (k, phase) = (((f - 60) / 240) as usize, (f - 60) % 240);
    let Some(preset) = ContinentPreset::ALL.get(k).copied() else {
        if phase == 0 {
            world.write_message(AppExit::Success);
        }
        return;
    };
    if phase == 0 {
        let args = GenArgs {
            params: preset.params(7),
            new_world: Some(((km as f32 * 1024.0 / 4.0) as usize, 4.0)),
            biomes: Some(Climate::Varied),
        };
        match world
            .run_system_cached_with(generate_now, args)
            .map_err(|e| e.to_string())
            .and_then(|r| r)
        {
            Ok(v) => info!("BIGW sheet {} -> {v}", preset.id()),
            Err(e) => warn!("BIGW sheet {} failed: {e}", preset.id()),
        }
    }
    if phase == 200 {
        let path = format!("{dir}/{}.png", preset.id());
        world
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
}

/// `WB_BW_IOTEST=<dir>`: save the generated world, load it back, compare (see the module docs).
fn io_hook(bw: Res<BigWorld>, mut data: WorldData, mut wait: Local<u32>) {
    let Ok(dir) = std::env::var("WB_BW_IOTEST") else {
        return;
    };
    if !bw.world_ready || *wait > 30 {
        return;
    }
    *wait += 1;
    if *wait != 30 {
        return;
    }
    let path = std::path::PathBuf::from(dir).join("big.world");
    let before = (
        data.store.count(),
        data.terrain.heights.iter().map(|h| *h as f64).sum::<f64>(),
        data.terrain.res,
    );
    let t0 = Instant::now();
    if let Err(e) = crate::io::save_world(&path, &data) {
        return error!("BIGW io save FAIL {e}");
    }
    let save = t0.elapsed().as_secs_f32();
    let bytes: u64 = std::fs::read_dir(&path)
        .map(|d| {
            d.filter_map(|e| e.ok()?.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    let t1 = Instant::now();
    if let Err(e) = crate::io::load_world(&path, &mut data) {
        return error!("BIGW io load FAIL {e}");
    }
    let load = t1.elapsed().as_secs_f32();
    let after = (
        data.store.count(),
        data.terrain.heights.iter().map(|h| *h as f64).sum::<f64>(),
        data.terrain.res,
    );
    let ok = before.0 == after.0 && (before.1 - after.1).abs() < 1e-3 && before.2 == after.2;
    info!(
        "BIGW io {} save {save:.2} s load {load:.2} s, {:.0} MB on disk, {} objects",
        if ok { "PASS" } else { "FAIL" },
        bytes as f64 / 1e6,
        after.0
    );
}

// ------------------------------------------------------------------ objects table

/// The first version: 48-byte rows without per-axis scale (still read).
const OBJECTS_MAGIC_V1: &[u8; 4] = b"WBO1";
const OBJECTS_MAGIC: &[u8; 4] = b"WBO2";
/// Bytes per object row: id, asset index, variant, position, rotation, scale, stretch.
const ROW: usize = 60;
/// Flag: the payload is deflated (`miniz_oxide`).
const DEFLATED: u32 = 1;
/// Flag: the rows are stored byte-plane by byte-plane (byte k of every row, then k + 1…), which
/// lines up similar bytes (ids, assets, nearby coordinates) for the compressor.
const SHUFFLED: u32 = 2;

/// Every world keeps its objects in `objects.bin`, a compact little-endian table: magic `WBO2`,
/// flags (u32), payload length (u64), then the payload (deflated): the asset names (count,
/// then length + UTF-8 each), the object count and one 60-byte row per object (id u64, asset
/// index u32, variant u32, position 3 × f32, rotation quaternion 4 × f32, scale f32, per-axis
/// stretch 3 × f32 with 0 = uniform), byte-shuffled and sorted by id, so an unchanged world
/// packs to the same bytes (autosave rewrites only changed files).
///
/// This raw form (rows in the given order, not compressed) is quick to build on the main thread
/// for a background save: [`pack_objects`] does the rest.
pub fn encode_objects(items: &[&crate::objects::Placed]) -> Vec<u8> {
    use rayon::prelude::*;
    const CHUNK: usize = 16_384;
    // rows with chunk-local asset indices, in parallel; then one shared name table
    let mut rows = vec![0u8; items.len() * ROW];
    let local: Vec<Vec<&str>> = rows
        .par_chunks_mut(CHUNK * ROW)
        .zip(items.par_chunks(CHUNK))
        .map(|(out, c)| {
            let mut names: Vec<&str> = vec![];
            let mut index: FxMap<&str, u32> = FxMap::default();
            for (r, o) in out.chunks_exact_mut(ROW).zip(c) {
                let a = *index.entry(o.asset.as_str()).or_insert_with(|| {
                    names.push(o.asset.as_str());
                    names.len() as u32 - 1
                });
                r[0..8].copy_from_slice(&o.id.to_le_bytes());
                r[8..12].copy_from_slice(&a.to_le_bytes());
                r[12..16].copy_from_slice(&o.variant.to_le_bytes());
                let st = o.stretch.unwrap_or([0.0; 3]);
                for (k, v) in o
                    .pos
                    .iter()
                    .chain(&o.rot)
                    .chain(std::iter::once(&o.scale))
                    .chain(&st)
                    .enumerate()
                {
                    r[16 + k * 4..20 + k * 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            names
        })
        .collect();
    // (sorted, so the table doesn't depend on the order the objects came in)
    let mut names: Vec<&str> = local.iter().flatten().copied().collect();
    names.sort_unstable();
    names.dedup();
    let index: FxMap<&str, u32> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, i as u32))
        .collect();
    let remap: Vec<Vec<u32>> = local
        .iter()
        .map(|l| l.iter().map(|n| index[n]).collect())
        .collect();
    rows.par_chunks_mut(CHUNK * ROW)
        .zip(remap.par_iter())
        .for_each(|(out, m)| {
            for r in out.chunks_exact_mut(ROW) {
                let l = u32::from_le_bytes(r[8..12].try_into().unwrap()) as usize;
                r[8..12].copy_from_slice(&m[l].to_le_bytes());
            }
        });
    let mut head = vec![];
    head.extend_from_slice(&(names.len() as u32).to_le_bytes());
    for n in &names {
        head.extend_from_slice(&(n.len() as u32).to_le_bytes());
        head.extend_from_slice(n.as_bytes());
    }
    head.extend_from_slice(&(items.len() as u32).to_le_bytes());
    let mut out = Vec::with_capacity(16 + head.len() + rows.len());
    out.extend_from_slice(OBJECTS_MAGIC);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((head.len() + rows.len()) as u64).to_le_bytes());
    out.extend_from_slice(&head);
    out.extend_from_slice(&rows);
    out
}

/// A fast hasher for the asset names (FxHash-style; the names are short and trusted).
#[derive(Default)]
struct Fx(u64);

impl std::hash::Hasher for Fx {
    fn write(&mut self, b: &[u8]) {
        for c in b.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.0 =
                (self.0.rotate_left(5) ^ u64::from_le_bytes(w)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

type FxMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<Fx>>;

/// The table as written to disk: rows sorted by id and byte-shuffled, the payload deflated
/// (a fast level). For background threads: about a second for 750 000 objects.
pub fn pack_objects(raw: &[u8]) -> Vec<u8> {
    if raw.len() < 16 || &raw[0..4] != OBJECTS_MAGIC {
        return raw.to_vec();
    }
    let payload = &raw[16..];
    let names = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    let mut at = 4;
    for _ in 0..names {
        at += 4 + u32::from_le_bytes(payload[at..at + 4].try_into().unwrap()) as usize;
    }
    at += 4;
    let (head, rows) = payload.split_at(at);
    let n = rows.len() / ROW;
    let id = |i: usize| u64::from_le_bytes(rows[i * ROW..i * ROW + 8].try_into().unwrap());
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_unstable_by_key(|i| id(*i as usize));
    let mut plain = Vec::with_capacity(payload.len());
    plain.extend_from_slice(head);
    plain.resize(payload.len(), 0);
    let planes = &mut plain[at..];
    for (dst, &src) in order.iter().enumerate() {
        let r = &rows[src as usize * ROW..src as usize * ROW + ROW];
        for k in 0..ROW {
            planes[k * n + dst] = r[k];
        }
    }
    let packed = miniz_oxide::deflate::compress_to_vec(&plain, 2);
    let mut out = OBJECTS_MAGIC.to_vec();
    out.extend_from_slice(&(DEFLATED | SHUFFLED).to_le_bytes());
    out.extend_from_slice(&(plain.len() as u64).to_le_bytes());
    out.extend_from_slice(&packed);
    out
}

/// Reads `objects.bin` (both versions; packed or raw).
pub fn decode_objects(b: &[u8]) -> Result<Vec<crate::objects::Placed>, String> {
    let bad = || "objects.bin is damaged".to_string();
    let (row, payload): (usize, std::borrow::Cow<[u8]>) = match b.get(0..4) {
        Some(m) if m == OBJECTS_MAGIC_V1 => (48, std::borrow::Cow::Borrowed(&b[4..])),
        Some(m) if m == OBJECTS_MAGIC => {
            let flags = u32::from_le_bytes(b.get(4..8).ok_or_else(bad)?.try_into().unwrap());
            let len =
                u64::from_le_bytes(b.get(8..16).ok_or_else(bad)?.try_into().unwrap()) as usize;
            let mut p = if flags & DEFLATED != 0 {
                miniz_oxide::inflate::decompress_to_vec_with_limit(&b[16..], len)
                    .map_err(|_| bad())?
            } else {
                b[16..].to_vec()
            };
            if p.len() != len {
                return Err(bad());
            }
            if flags & SHUFFLED != 0 {
                unshuffle(&mut p).ok_or_else(bad)?;
            }
            (ROW, std::borrow::Cow::Owned(p))
        }
        _ => return Err(bad()),
    };
    let b = &payload[..];
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let s = b.get(at..at + n).ok_or_else(bad)?;
        at += n;
        Ok(s)
    };
    let u32_at = |s: &[u8]| u32::from_le_bytes(s.try_into().unwrap());
    let names = u32_at(take(4)?);
    let mut assets = Vec::with_capacity(names as usize);
    for _ in 0..names {
        let len = u32_at(take(4)?) as usize;
        assets.push(String::from_utf8(take(len)?.to_vec()).map_err(|_| bad())?);
    }
    let count = u32_at(take(4)?) as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let r = take(row)?;
        let f = |k: usize| f32::from_le_bytes(r[16 + k * 4..20 + k * 4].try_into().unwrap());
        let stretch = (row >= ROW && f(8) > 0.0).then(|| [f(8), f(9), f(10)]);
        out.push(crate::objects::Placed {
            group: 0,
            id: u64::from_le_bytes(r[0..8].try_into().unwrap()),
            asset: assets
                .get(u32_at(&r[8..12]) as usize)
                .ok_or_else(bad)?
                .clone(),
            variant: u32_at(&r[12..16]),
            pos: [f(0), f(1), f(2)],
            rot: [f(3), f(4), f(5), f(6)],
            scale: f(7),
            stretch,
            locked: false,
        });
    }
    Ok(out)
}

/// Undoes the byte shuffle of a payload's rows (in place).
fn unshuffle(p: &mut [u8]) -> Option<()> {
    let names = u32::from_le_bytes(p.get(0..4)?.try_into().ok()?) as usize;
    let mut at = 4;
    for _ in 0..names {
        at += 4 + u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?) as usize;
    }
    let n = u32::from_le_bytes(p.get(at..at + 4)?.try_into().ok()?) as usize;
    at += 4;
    let planes = p.get(at..at + n * ROW)?.to_vec();
    let rows = &mut p[at..at + n * ROW];
    for k in 0..ROW {
        for i in 0..n {
            rows[i * ROW + k] = planes[k * n + i];
        }
    }
    Some(())
}

// ------------------------------------------------------------------ MCP

/// Where erosion applies: the whole world, a circle or a polygon (world metres).
enum Area {
    All,
    Circle(Vec2, f32),
    Polygon(Vec<Vec2>),
}

/// One property per erosion parameter (from the parameters themselves).
fn erosion_params_schema() -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    if let Ok(Value::Object(o)) = serde_json::to_value(ErosionPreset::default().params()) {
        for (k, v) in o {
            let p = match k.as_str() {
                "wind_dir" => crate::mcp_tools::bearing_schema("where the wind blows sand towards"),
                "talus_angle" => {
                    json!({"type": "number", "description": "steepest scree slope (degrees)"})
                }
                "years" => json!({"type": "integer", "description": "rounds of erosion"}),
                _ if v.is_u64() || v.is_i64() => json!({"type": "integer"}),
                _ => json!({"type": "number"}),
            };
            m.insert(k, p);
        }
    }
    m
}

fn mcp_erode(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let preset = match opt_arg::<String>(&args, "preset")? {
        Some(s) => ErosionPreset::from_id(&s).ok_or_else(|| format!("unknown preset `{s}`"))?,
        None => world.resource::<BigWorld>().erosion_preset,
    };
    let mut p = preset.params();
    if let Some(o) = args.get("params").filter(|v| v.is_object()) {
        let mut v = serde_json::to_value(&p).map_err(|e| e.to_string())?;
        for (k, val) in o.as_object().unwrap() {
            if v.get(k).is_none() {
                return Err(format!("unknown erosion param `{k}`"));
            }
            // wind_dir: a compass bearing (internally 0 = east, 90 = south)
            v[k] = if k == "wind_dir" {
                json!(val.as_f64().ok_or("params.wind_dir: a number")? - 90.0)
            } else {
                val.clone()
            };
        }
        p = serde_json::from_value(v).map_err(|e| format!("params: {e}"))?;
    }
    if let Some(s) = opt_arg::<f32>(&args, "strength")? {
        p.strength = s.clamp(0.0, 4.0);
    }
    let area = if let Some(a) = args.get("area").filter(|v| v.is_object()) {
        let (c, _) = crate::mcp_tools::position_arg(a, "center")?
            .ok_or("area: give `center` [x, z] and `radius`")?;
        Area::Circle(c, arg::<f32>(a, "radius")?.max(10.0))
    } else if let Some(poly) = opt_arg::<Vec<[f32; 2]>>(&args, "polygon")? {
        if poly.len() < 3 {
            return Err("polygon needs at least 3 points".into());
        }
        Area::Polygon(poly.into_iter().map(Vec2::from_array).collect())
    } else {
        Area::All
    };
    let out = world
        .run_system_cached_with(erode_now, (preset, p, area))
        .map_err(|e| e.to_string())??;
    Ok(vec![McpContent::Json(out)])
}

fn erode_now(
    In((preset, p, area)): In<(ErosionPreset, ErosionParams, Area)>,
    mut data: WorldData,
    mut bw: ResMut<BigWorld>,
) -> Result<Value, String> {
    let t0 = Instant::now();
    let t = &*data.terrain;
    let (n, cell, sea) = (t.n(), t.cell, t.sea_level);
    let feather = 40.0f32;
    let (rect, mask) = match &area {
        Area::All => (full(t), None),
        Area::Circle(c, r) => {
            let rect = t
                .rect_for_circle(*c, r * 1.15 + cell * 4.0)
                .ok_or("the area is outside the world")?;
            let mask = mask_for(t, rect, |p| {
                1.0 - crate::noise::smoothstep(r - feather.min(r * 0.5), *r, p.distance(*c))
            });
            (rect, Some(mask))
        }
        Area::Polygon(poly) => {
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in poly {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            let rect = t
                .rect_for_circle(
                    (lo + hi) * 0.5,
                    (hi - lo).length() * 0.5 + feather + cell * 4.0,
                )
                .ok_or("the polygon is outside the world")?;
            let mask = mask_for(t, rect, |p| {
                if inside(p, poly) {
                    crate::noise::smoothstep(0.0, feather, dist_to_edges(p, poly))
                } else {
                    0.0
                }
            });
            (rect, Some(mask))
        }
    };
    let mut heights = data.terrain.heights.clone();
    let progress = Progress::default();
    erosion::erode(
        &mut heights,
        n,
        cell,
        sea,
        rect,
        mask.as_deref(),
        &p,
        progress.stage(0.0, 1.0),
    );
    let flow = erosion::flow_map(&heights, n, cell, sea);
    let label = format!("Erode ({})", preset.name());
    let moved = apply_heights(&mut data, &label, heights, flow, Some(rect));
    bw.flow.mark_current(&data.terrain);
    let secs = t0.elapsed().as_secs_f32();
    info!("BIGW mcp erode {} in {secs:.2} s", preset.id());
    Ok(json!({
        "preset": preset.id(),
        "seconds": secs,
        "moved_m3": moved.round(),
        "area_m2": (rect.width() * rect.height()) as f32 * cell * cell,
        "undo": label,
    }))
}

fn mask_for(t: &Terrain, r: GridRect, f: impl Fn(Vec2) -> f32) -> Vec<f32> {
    let mut m = Vec::with_capacity(r.width() * r.height());
    for j in r.z0..=r.z1 {
        for i in r.x0..=r.x1 {
            m.push(f(t.grid_to_world(i, j)).clamp(0.0, 1.0));
        }
    }
    m
}

fn inside(p: Vec2, poly: &[Vec2]) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
        j = i;
    }
    c
}

fn dist_to_edges(p: Vec2, poly: &[Vec2]) -> f32 {
    (0..poly.len())
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            p.distance(a + ab * t)
        })
        .fold(f32::MAX, f32::min)
}

struct GenArgs {
    params: ContinentParams,
    new_world: Option<(usize, f32)>,
    biomes: Option<Climate>,
}

fn mcp_generate(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let preset_s: String = arg(&args, "preset")?;
    let preset = ContinentPreset::from_id(&preset_s)
        .ok_or_else(|| format!("unknown preset `{preset_s}`"))?;
    let seed = opt_arg::<u32>(&args, "seed")?.unwrap_or_else(|| fastrand::u32(0..999_999));
    let mut p = preset.params(seed);
    if let Some(v) = opt_arg::<f32>(&args, "land")? {
        p.land = v.clamp(0.05, 0.9);
    }
    if let Some(v) = opt_arg::<f32>(&args, "roughness")? {
        p.roughness = v.clamp(0.0, 1.0);
    }
    if let Some(v) = opt_arg::<u32>(&args, "ranges")? {
        p.ranges = v.min(8);
    }
    if let Some(v) = opt_arg::<f32>(&args, "range_dir")? {
        // (internally 0 = along +x)
        p.range_dir = Some(v - 90.0);
    }
    if let Some(v) = opt_arg::<f32>(&args, "relief")? {
        p.relief = v.clamp(0.1, 2.5);
    }
    if let Some(e) = args.get("erosion") {
        p.erosion = match e {
            Value::Null => None,
            Value::String(s) if s == "none" => None,
            Value::String(s) => Some(
                ErosionPreset::from_id(s).ok_or_else(|| format!("unknown erosion preset `{s}`"))?,
            ),
            _ => return Err("erosion: a preset id or null".into()),
        };
    }
    let new_world =
        if opt_arg::<bool>(&args, "new_world")?.unwrap_or(false) || args.get("size_km").is_some() {
            let km = opt_arg::<u32>(&args, "size_km")?.unwrap_or(4);
            let cell = opt_arg::<f32>(&args, "cell")?.unwrap_or(4.0);
            let Some((_, cells)) = SIZES.iter().find(|s| s.0 == km) else {
                return Err("size_km must be 2, 4, 8 or 16".into());
            };
            if !cells.contains(&cell) {
                return Err(format!("a {km} km world takes cell sizes {cells:?} m"));
            }
            Some(((km as f32 * 1024.0 / cell).round() as usize, cell))
        } else {
            None
        };
    let climate = match opt_arg::<String>(&args, "climate")? {
        Some(c) => Climate::ALL
            .into_iter()
            .find(|x| x.name().eq_ignore_ascii_case(&c))
            .ok_or_else(|| format!("unknown climate `{c}`"))?,
        None => Climate::Varied,
    };
    let biomes = opt_arg::<bool>(&args, "biomes")?
        .unwrap_or(true)
        .then_some(climate);
    world.resource_mut::<BigWorld>().continent = p.clone();
    let out = world
        .run_system_cached_with(
            generate_now,
            GenArgs {
                params: p,
                new_world,
                biomes,
            },
        )
        .map_err(|e| e.to_string())??;
    Ok(vec![McpContent::Json(out)])
}

fn generate_now(
    In(a): In<GenArgs>,
    mut data: WorldData,
    mut bw: ResMut<BigWorld>,
) -> Result<Value, String> {
    let t0 = Instant::now();
    let (res, cell, sea) = match a.new_world {
        Some((r, c)) => (r, c, 0.0),
        None => (data.terrain.res, data.terrain.cell, data.terrain.sea_level),
    };
    let progress = Progress::default();
    let heights = continent::generate(&a.params, res, cell, sea, progress.stage(0.0, 1.0))
        .ok_or("cancelled")?;
    let flow = erosion::flow_map(&heights, res + 1, cell, sea);
    let gen_secs = t0.elapsed().as_secs_f32();
    let name = a.params.preset.name();
    let mut regions = 0;
    if a.new_world.is_some() {
        crate::io::new_world_from(&mut data, heights, res, cell, a.params.seed);
        fit_snow_line(&mut data.terrain);
        data.terrain.flow = flow;
        if let Some(c) = a.biomes {
            data.history.begin("Generate biomes");
            regions = crate::biomegen::generate(
                &mut data.terrain,
                &mut data.zones,
                &mut data.history,
                &mut data.water,
                a.params.seed,
                c,
            )
            .regions;
            data.history.commit(&data.store);
            data.history.clear();
        }
    } else {
        regions = regenerate_in_place(&mut data, name, heights, flow, a.params.seed, a.biomes);
    }
    bw.flow.mark_current(&data.terrain);
    let t = &*data.terrain;
    let land =
        t.heights.iter().filter(|h| **h > t.sea_level).count() as f32 / t.heights.len() as f32;
    let peak = t.heights.iter().copied().fold(f32::MIN, f32::max) - t.sea_level;
    let secs = t0.elapsed().as_secs_f32();
    info!(
        "BIGW mcp generate {} {}x{} in {secs:.2} s (terrain {gen_secs:.2} s)",
        a.params.preset.id(),
        res,
        res
    );
    data.ed.toast(format!("Generated a {name}"));
    Ok(json!({
        "preset": a.params.preset.id(),
        "seed": a.params.seed,
        "size_m": t.size(),
        "cell_m": t.cell,
        "land_share": land,
        "highest_peak_m": peak,
        "biome_regions": regions,
        "seconds": secs,
        "terrain_seconds": gen_secs,
    }))
}
