//! The Export dialog (top bar ▸ Export, ⌘E): engine tiles, folder, terrain / texture size, what to
//! include (with counts), a size estimate, and progress with cancel while the export runs on a
//! background thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, FontId, RichText, vec2};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_phosphor::regular as icon;
use serde_json::Value;

use super::{ExportOptions, Progress, Target, core};
use crate::objects::Category;
use crate::ui::theme::*;
use crate::ui::widgets::*;

static REQUEST: AtomicBool = AtomicBool::new(false);

/// Opens the export dialog (the top bar's Export button and ⌘E, via `io`).
pub fn request_dialog() {
    REQUEST.store(true, Ordering::Relaxed);
}

struct Job {
    progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<Value, String>>>,
    target: Target,
    dir: PathBuf,
}

/// What the world holds, for the option counts and the size estimate.
#[derive(Default)]
struct Stats {
    trees: usize,
    props: usize,
    plants: usize,
    /// distinct models per group (generated species / photoscans, packed files and their size)
    tree_models: usize,
    scan_models: usize,
    packed_models: usize,
    packed_bytes: u64,
    lakes: usize,
    rivers: usize,
    falls: usize,
    markers: usize,
    roads: usize,
    layers: usize,
}

#[derive(Resource)]
struct ExportUi {
    open: bool,
    target: Target,
    parent: PathBuf,
    terrain: usize,
    texture: u32,
    trees: bool,
    props: bool,
    plants: bool,
    water: bool,
    gameplay: bool,
    roads: bool,
    collision: bool,
    terrain_meshes: bool,
    optimize_caves: bool,
    start: bool,
    job: Option<Job>,
    /// last finished export: (target, folder, summary or error)
    result: Option<(Target, PathBuf, Result<Value, String>)>,
    stats: Stats,
}

impl Default for ExportUi {
    fn default() -> Self {
        Self {
            open: false,
            target: Target::Godot,
            parent: super::exports_dir(),
            terrain: 0,
            texture: 1024,
            trees: true,
            props: true,
            plants: true,
            water: true,
            gameplay: true,
            roads: true,
            collision: true,
            terrain_meshes: true,
            optimize_caves: true,
            start: false,
            job: None,
            result: None,
            stats: Stats::default(),
        }
    }
}

impl ExportUi {
    fn folder(&self, world_name: &str) -> PathBuf {
        self.parent.join(format!(
            "{}-{}",
            core::safe_name(world_name),
            self.target.folder()
        ))
    }

    fn options(&self, world_name: &str) -> ExportOptions {
        ExportOptions {
            target: self.target,
            dir: self.folder(world_name),
            terrain: self.terrain,
            texture_size: self.texture,
            trees: self.trees,
            props: self.props,
            plants: self.plants,
            water: self.water,
            gameplay: self.gameplay,
            roads: self.roads,
            collision: self.collision,
            terrain_meshes: self.terrain_meshes,
            optimize_caves: self.optimize_caves,
            shared_textures: true,
            navmesh: true,
        }
    }

    /// Rough output size (bytes), calibrated on exports of the demo world (4 km, 1K textures:
    /// generic 196 MB, Godot 199 MB, Unity 178 MB, Unreal 136 MB).
    fn estimate(&self) -> u64 {
        let st = &self.stats;
        let n = self.target.terrain_vertices(self.terrain) as u64;
        let px = n as f64 * n as f64;
        let tex = (self.texture as f64 / 1024.0).powi(2);
        let unreal = self.target == Target::Unreal;
        // heightmap PNG + RAW, weights
        let mut b = px * 3.5 + px * st.layers as f64 * 0.3;
        // layer textures (albedo, normal and, except Unreal, a mask map)
        b += st.layers as f64 * tex * if unreal { 6.0e6 } else { 8.6e6 };
        if self.target == Target::Godot || (self.target == Target::Generic && self.terrain_meshes) {
            b += px * 45.0;
        }
        if self.target == Target::Generic {
            b += px * 4.5;
        }
        if self.trees || self.plants {
            b += st.tree_models as f64 * (tex * 2.8e6 + 4.0e6);
        }
        if self.props || self.plants {
            b += st.scan_models as f64 * (tex * 0.9e6 + 0.8e6);
        }
        if self.props {
            b += st.packed_bytes as f64 * tex.min(1.0) * 0.9;
        }
        if self.plants && self.target == Target::Unity {
            b += 2.0e7 * (px / 1.05e6).min(1.0);
        }
        let inst = (if self.trees { st.trees } else { 0 }
            + if self.props { st.props } else { 0 }
            + if self.plants { st.plants } else { 0 }) as f64;
        b += inst * 150.0;
        b as u64
    }
}

pub struct ExportUiPlugin;

impl Plugin for ExportUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExportUi>()
            .add_systems(
                Update,
                (dialog_hook, open_on_request, start_job, poll_job).chain(),
            )
            .add_systems(EguiPrimaryContextPass, dialog);
    }
}

/// Test hook: `WB_EXPORT_DIALOG=1` opens the dialog at frame 200 (screenshots); `=<folder>` also
/// presses Export (into that folder) at frame 260.
fn dialog_hook(mut ui: ResMut<ExportUi>, mut frame: Local<u32>) {
    let Ok(v) = std::env::var("WB_EXPORT_DIALOG") else {
        return;
    };
    *frame += 1;
    if *frame == 200 {
        request_dialog();
    }
    if *frame == 260 && v != "1" {
        ui.parent = PathBuf::from(v);
        ui.start = true;
    }
}

/// Opens the dialog on request and keeps its counts current while it is open (they're cheap: a
/// pass over the objects and a coarse sample of the ground layers).
#[allow(clippy::too_many_arguments)]
fn open_on_request(
    mut ui: ResMut<ExportUi>,
    terrain: Res<crate::terrain::Terrain>,
    store: Res<crate::objects::ObjectStore>,
    lib: Option<Res<crate::objects::Library>>,
    water: Res<crate::water::WaterBodies>,
    gameplay: Res<crate::gameplay::Gameplay>,
    roads: Res<crate::roads::Roads>,
    mut seen: Local<Option<(u64, u64, usize, usize, bool)>>,
) {
    let opening = REQUEST.swap(false, Ordering::Relaxed);
    if opening {
        ui.open = true;
        if ui.terrain == 0 {
            ui.terrain = terrain.res.next_power_of_two().clamp(512, 16384);
        }
    }
    let world = (
        store.revision,
        terrain.height_revision,
        gameplay.pois.len() + gameplay.npcs.len() + gameplay.spawns.len() + gameplay.nodes.len(),
        roads.roads.len(),
        gameplay.player_start.is_some(),
    );
    if !ui.open || (!opening && *seen == Some(world)) {
        return;
    }
    *seen = Some(world);
    let mut st = Stats {
        lakes: water.lakes.len(),
        rivers: water.rivers.len(),
        falls: water.waterfalls.len(),
        roads: roads.roads.len(),
        ..default()
    };
    st.markers = gameplay.pois.len()
        + gameplay.npcs.len()
        + gameplay.spawns.len()
        + gameplay.nodes.len()
        + gameplay.player_start.is_some() as usize
        + water.fish.len();
    let mut used = std::collections::HashSet::new();
    if let Some(lib) = &lib {
        for o in store.items.values() {
            let Some(e) = lib.get(&o.asset) else { continue };
            match e.category {
                Category::Trees => st.trees += 1,
                Category::Plants => st.plants += 1,
                _ => st.props += 1,
            }
            if used.insert(e.id.clone()) {
                if e.tree.is_some() {
                    if e.category == Category::Trees
                        || e.kind.is_some_and(|k| {
                            matches!(
                                k,
                                crate::procgen::meshes::AssetKind::Bush
                                    | crate::procgen::meshes::AssetKind::DesertShrub
                            )
                        })
                    {
                        st.tree_models += 1;
                    } else {
                        st.scan_models += 1;
                    }
                } else if let Some(p) = &e.import_path {
                    st.packed_models += 1;
                    let dir = crate::asset_file(p);
                    st.packed_bytes +=
                        dir.parent()
                            .and_then(|d| std::fs::read_dir(d).ok())
                            .map_or(0, |it| {
                                it.flatten()
                                    .map(|f| {
                                        if f.path().is_dir() {
                                            std::fs::read_dir(f.path()).map_or(0, |i| {
                                                i.flatten()
                                                    .filter_map(|g| g.metadata().ok())
                                                    .map(|m| m.len())
                                                    .sum()
                                            })
                                        } else {
                                            f.metadata().map_or(0, |m| m.len())
                                        }
                                    })
                                    .sum()
                            });
                }
            }
        }
    }
    // layers in use, sampled coarsely
    let n = terrain.n();
    let mut max_w = [0.0f32; crate::terrain::layers::LAYER_COUNT];
    for j in (0..n).step_by(8) {
        for i in (0..n).step_by(8) {
            for (m, w) in max_w.iter_mut().zip(terrain.final_weights(i, j)) {
                *m = m.max(w);
            }
        }
    }
    st.layers = max_w.iter().filter(|w| **w > 1.5 / 255.0).count().max(1);
    ui.stats = st;
}

/// Pressing Export: snapshot the world now (main thread), write it on a background thread.
fn start_job(world: &mut World) {
    if !world.resource::<ExportUi>().start {
        return;
    }
    let name = world.resource::<crate::world::WorldMeta>().name.clone();
    let opts = {
        let mut ui = world.resource_mut::<ExportUi>();
        ui.start = false;
        if ui.job.is_some() {
            return;
        }
        ui.options(&name)
    };
    // (the navmesh needs the walkability grid: built here if the Analyze tab has no fresh one)
    let navmesh = if opts.navmesh {
        super::nav::capture(world)
    } else {
        vec![]
    };
    let mut snap = core::snapshot(world);
    snap.navmesh = navmesh;
    let progress = Arc::new(Mutex::new(Progress {
        stage: "Starting".into(),
        fraction: 0.0,
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let (target, dir) = (opts.target, opts.dir.clone());
    let ctx = core::Ctx::new(opts, progress.clone(), cancel.clone());
    let handle = std::thread::Builder::new()
        .name("export".into())
        .spawn(move || super::run(&snap, &ctx))
        .ok();
    let mut ui = world.resource_mut::<ExportUi>();
    ui.result = None;
    ui.job = Some(Job {
        progress,
        cancel,
        handle,
        target,
        dir,
    });
}

fn poll_job(mut ui: ResMut<ExportUi>, mut ed: ResMut<crate::editor::Editor>) {
    let finished = ui
        .job
        .as_ref()
        .is_some_and(|j| j.handle.as_ref().is_none_or(|h| h.is_finished()));
    if !finished {
        return;
    }
    let Some(mut job) = ui.job.take() else { return };
    let result = match job.handle.take().map(|h| h.join()) {
        Some(Ok(r)) => r,
        Some(Err(_)) => Err("the export crashed".into()),
        None => Err("couldn't start the export thread".into()),
    };
    match &result {
        Ok(v) => ed.toast(format!(
            "Exported to {} ({:.0} MB)",
            job.dir.display(),
            v["files"]["megabytes"].as_f64().unwrap_or(0.0)
        )),
        Err(e) if e == super::CANCELLED => ed.toast("Export cancelled"),
        Err(e) => ed.toast(format!("Export failed: {e}")),
    }
    ui.result = Some((job.target, job.dir, result));
}

fn target_icon(t: Target) -> &'static str {
    match t {
        Target::Generic => icon::FILES,
        Target::Godot => icon::ROBOT,
        Target::Unity => icon::CUBE,
        Target::Unreal => icon::LIGHTNING,
    }
}

fn blurb(t: Target) -> &'static str {
    match t {
        Target::Generic => {
            "Heightmap, splat maps, layer textures, GLB models, instance lists, water, roads, gameplay and lighting as plain files with a manifest — for any engine or tool."
        }
        Target::Godot => {
            "A complete Godot 4.3+ project: open project.godot and press F5 to fly or walk around (terrain shader, collision, vegetation, water, sky and markers)."
        }
        Target::Unity => {
            "A WorldBuilder folder to drop into Assets with an importer (Tools ▸ Skyfold Studio ▸ Import World) that builds Terrain, trees, grass, props, water, sky and markers. Models need glTFast."
        }
        Target::Unreal => {
            "Landscape heightmap and weightmaps with exact scale values, GLB models and import_world.py for Unreal 5.4+ (Python plugin): meshes, instances, sky, sun, fog and markers."
        }
    }
}

fn next_steps(t: Target) -> &'static str {
    match t {
        Target::Generic => "Open README.md for the formats; manifest.json lists every file.",
        Target::Godot => {
            "In Godot 4.3+: Import ▸ pick project.godot, open it and press F5 (WASD fly, right mouse to look, F to walk)."
        }
        Target::Unity => {
            "Copy the WorldBuilder folder into your project's Assets, add the glTFast package, then Tools ▸ Skyfold Studio ▸ Import World."
        }
        Target::Unreal => {
            "In Unreal 5.4+: enable Python Editor Script Plugin, then Tools ▸ Execute Python Script ▸ import_world.py. README.md has the Landscape steps."
        }
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn megabytes(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1e9)
    } else {
        format!("{:.0} MB", (b as f64 / 1e6).max(1.0))
    }
}

fn dialog(
    _main: bevy::ecs::system::NonSendMarker,
    mut contexts: EguiContexts,
    mut ui: ResMut<ExportUi>,
    meta: Res<crate::world::WorldMeta>,
    terrain: Res<crate::terrain::Terrain>,
) -> Result {
    // Change… answers on a later frame (crate::dialogs)
    if let Some(d) = crate::dialogs::take_file("export_folder") {
        ui.parent = d;
    }
    if !ui.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?.clone();
    let mut open = true;
    let ui = &mut *ui;
    egui::Window::new(RichText::new(format!("{}  Export World", icon::EXPORT)).font(FontId::new(17.0, semibold())))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .fixed_size(vec2(470.0, 0.0))
        .open(&mut open)
        .show(&ctx, |eg| {
            let busy = ui.job.is_some();
            eg.add_enabled_ui(!busy, |eg| {
                eg.horizontal(|eg| {
                    for t in Target::ALL {
                        if tool_tile(eg, target_icon(t), t.name(), ui.target == t, vec2(108.0, 66.0)).clicked() {
                            ui.target = t;
                            ui.result = None;
                        }
                    }
                });
                eg.add_space(4.0);
                dim(eg, blurb(ui.target));
                separator(eg);

                section(eg, "Folder");
                eg.horizontal(|eg| {
                    let shown = ui.folder(&meta.name).display().to_string();
                    let short = if shown.len() > 52 { format!("…{}", &shown[shown.len() - 51..]) } else { shown.clone() };
                    eg.label(RichText::new(short).color(TEXT)).on_hover_text(shown);
                    eg.with_layout(egui::Layout::right_to_left(egui::Align::Center), |eg| {
                        if eg.button(format!("{} Change…", icon::FOLDER_OPEN)).clicked() {
                            let _ = std::fs::create_dir_all(&ui.parent);
                            crate::dialogs::pick_folder("export_folder", rfd::AsyncFileDialog::new().set_title("Export into folder").set_directory(&ui.parent));
                        }
                    });
                });
                dim(eg, "Skyfold Studio's own files in that folder are replaced; nothing else is touched.");

                section(eg, "Terrain resolution");
                eg.push_id("export_terrain", |eg| segmented(eg, &mut ui.terrain, &[(512, "512"), (1024, "1K"), (2048, "2K"), (4096, "4K"), (8192, "8K"), (16384, "16K")]));
                let n = ui.target.terrain_vertices(ui.terrain);
                let (tiles, tn) = super::tiles::split(ui.target, n);
                let tiled = if tiles > 1 { format!(" in {tiles} × {tiles} tiles of {tn}") } else { String::new() };
                dim(eg, &format!("{n} × {n} heightmap{tiled} · {:.2} m between samples (the world has {} × {})", terrain.size() / (n - 1) as f32, terrain.n(), terrain.n()));

                section(eg, "Texture size");
                eg.push_id("export_texture", |eg| segmented(eg, &mut ui.texture, &[(512, "512"), (1024, "1024"), (2048, "2048")]));

                section(eg, "Include");
                let st = &ui.stats;
                let rows = [
                    (format!("Trees · {}", st.trees), 0),
                    (format!("Rocks, debris & props · {}", st.props), 1),
                    (format!("Plants & grass · {}", st.plants), 2),
                    (format!("Water · {}, {}{}", plural(st.lakes, "lake"), plural(st.rivers, "river"), if st.falls > 0 { format!(", {}", plural(st.falls, "fall")) } else { String::new() }), 3),
                    (format!("Gameplay markers · {}", st.markers), 4),
                    (format!("Roads · {}", st.roads), 5),
                    ("Collision for trees, rocks & buildings".to_string(), 6),
                ];
                egui::Grid::new("export_include").num_columns(2).spacing(vec2(16.0, 2.0)).show(eg, |eg| {
                    for (k, (label, field)) in rows.into_iter().enumerate() {
                        let v = match field {
                            0 => &mut ui.trees,
                            1 => &mut ui.props,
                            2 => &mut ui.plants,
                            3 => &mut ui.water,
                            4 => &mut ui.gameplay,
                            5 => &mut ui.roads,
                            _ => &mut ui.collision,
                        };
                        toggle(eg, v, label);
                        if k % 2 == 1 {
                            eg.end_row();
                        }
                    }
                    if ui.target == Target::Generic {
                        toggle(eg, &mut ui.terrain_meshes, "Terrain meshes (GLB chunks)");
                    }
                    toggle(eg, &mut ui.optimize_caves, "Optimize caves")
                        .on_hover_text("Simplify the caves' rock for the engine: far fewer triangles, within a few centimetres of the shape (the editor keeps every triangle)");
                    eg.end_row();
                });
                separator(eg);
                let models = ui.stats.tree_models + ui.stats.scan_models + ui.stats.packed_models;
                eg.label(RichText::new(format!("About {} · {models} models", megabytes(ui.estimate()))).color(TEXT));
            });
            eg.add_space(6.0);
            if let Some(job) = &ui.job {
                let p = job.progress.lock().map(|p| p.clone()).unwrap_or_default();
                eg.add(egui::ProgressBar::new(p.fraction).text(p.stage).animate(true).fill(Color32::from_white_alpha(80)));
                eg.add_space(4.0);
                if wide_button(eg, &format!("{} Cancel", icon::X)).clicked() {
                    job.cancel.store(true, Ordering::Relaxed);
                }
                ctx.request_repaint();
            } else if primary_button(eg, &format!("{}  Export to {}", icon::EXPORT, ui.target.name())).clicked() {
                ui.start = true;
            }
            if let Some((t, dir, res)) = &ui.result {
                eg.add_space(6.0);
                match res {
                    Ok(v) => {
                        let files = &v["files"];
                        eg.label(RichText::new(format!(
                            "{} Exported {} files ({:.0} MB) in {:.0} s",
                            icon::CHECK_CIRCLE,
                            files["count"].as_u64().unwrap_or(0),
                            files["megabytes"].as_f64().unwrap_or(0.0),
                            v["seconds"].as_f64().unwrap_or(0.0)
                        )).color(Color32::from_rgb(120, 210, 140)));
                        dim(eg, next_steps(*t));
                        let warnings = v["warnings"].as_array().map_or(0, |w| w.len());
                        if warnings > 0 {
                            dim(eg, &format!("{warnings} note(s) in the log (e.g. placeholder models that were skipped)."));
                        }
                        if wide_button(eg, &format!("{} Show in Finder", icon::FOLDER_OPEN)).clicked() {
                            let _ = std::process::Command::new("open").arg(dir).spawn();
                        }
                    }
                    Err(e) => {
                        eg.label(RichText::new(format!("{} {e}", icon::WARNING)).color(Color32::from_rgb(235, 120, 100)));
                    }
                }
            }
        });
    if !open {
        ui.open = false;
    }
    Ok(())
}
