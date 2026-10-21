//! Saving, loading, exporting and importing, plus world-wide actions requested by the UI.
//!
//! A saved world is a folder `<name>.world/` containing `world.json` (everything except the
//! big grids and the objects), `objects.bin` (the placed objects: `bigworld::encode_objects`;
//! worlds saved before it keep them in world.json and still load) and raw little-endian grids:
//! `heights.f32`, `splat.u8` (12 weights per vertex), `zones.u8` and `biomes.u8`. Autosaves
//! write in the background (`save`).

mod save;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use bevy::ecs::system::{NonSendMarker, SystemParam};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::camera::OrbitCamera;
use crate::editor::Editor;
use crate::history::History;
use crate::objects::{Category, Library, LibraryEntry, ObjectStore, Placed, Wind};
use crate::sky::Lighting;
use crate::terrain::layers::{LAYER_COUNT, Layer, Surface};
use crate::terrain::{GridRect, StyleRules, Terrain, worldgen};
use crate::ui::UiAction;
use crate::water::{Lake, River, WaterBodies, WaterStyle};
use crate::world::{WorldMeta, Zone, Zones};

#[derive(Serialize, Deserialize, Clone)]
pub struct ImportedAsset {
    pub id: String,
    pub name: String,
    pub path: String,
    pub surface: Surface,
    pub walkable: bool,
    pub radius: f32,
}

#[derive(Serialize, Deserialize)]
struct CameraDoc {
    focus: [f32; 3],
    yaw: f32,
    pitch: f32,
    distance: f32,
}

#[derive(Serialize, Deserialize)]
struct WorldDoc {
    version: u32,
    name: String,
    seed: u32,
    res: usize,
    cell: f32,
    sea_level: f32,
    style: StyleRules,
    lighting: Lighting,
    water_style: WaterStyle,
    wind: Vec<f32>,
    /// draw distance and detail (also read by the game export)
    #[serde(default)]
    graphics: crate::streaming::Graphics,
    #[serde(default)]
    nature: crate::objects::NatureAmounts,
    /// library material per building slot
    #[serde(default)]
    building_materials: crate::surfaces::SurfaceChoices,
    /// terrain library material per paint layer
    #[serde(default)]
    terrain_layers: crate::terrain::textures::TerrainLayerChoices,
    /// realistic ("photoreal") or hand-painted ("handpainted") surfaces and assets (worlds saved
    /// before it: realistic; a dungeon's map: its dungeon's art)
    #[serde(default)]
    art_style: Option<crate::art_style::ArtStyle>,
    zones: Vec<Zone>,
    rivers: Vec<River>,
    lakes: Vec<Lake>,
    #[serde(default)]
    fish: Vec<crate::fish::FishSchool>,
    #[serde(default)]
    waterfalls: Vec<crate::water::Waterfall>,
    /// player start, POIs, NPCs, spawn regions, creature library, resource nodes
    #[serde(default)]
    gameplay: crate::gameplay::Gameplay,
    #[serde(default)]
    roads: crate::roads::Roads,
    /// farm fields (crop, outline, rows)
    #[serde(default)]
    fields: crate::fields::Fields,
    /// buildings fitted to the ground: their pads and the ground under them before
    #[serde(default)]
    foundations: crate::placement::Foundations,
    /// movement rules and thresholds of the Analyze tab
    #[serde(default)]
    analysis: crate::analysis::AnalysisSettings,
    /// camera paths and recent playtest sessions
    #[serde(default)]
    motion: crate::motion::MotionData,
    /// caves, mines, sinkholes, passages, caverns, arches and dug strokes (their recipes)
    #[serde(default)]
    caves: crate::caves::Caves,
    /// the world's dungeons and their ways in (their maps are worlds in `dungeons/`)
    #[serde(default)]
    dungeons: crate::dungeons::Dungeons,
    /// this world is a dungeon's map: its plan, style, lights and mood
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dungeon_map: Option<crate::dungeons::MapDoc>,
    /// Map tab settings (style, resolution, layers, title)
    #[serde(default)]
    map: crate::worldmap::MapSettings,
    /// reference picture (copied into the world folder), placement, analysis edits
    #[serde(default)]
    reference: crate::refmap::RefDoc,
    /// Retained verbatim for worlds written by removed Effects Studio features. The editor no
    /// longer interprets these fields, but keeps them when an existing world is re-saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reference_effects: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effects_studio: Option<serde_json::Value>,
    objects: Vec<Placed>,
    /// The objects are in this compact binary table (`bigworld::encode_objects`; older worlds
    /// keep them in `objects`).
    #[serde(default)]
    objects_file: Option<String>,
    /// procedural nature: density maps and hand removals (`nature.bin`); version 0 = an old world
    /// whose nature objects are converted on load
    #[serde(default)]
    procedural: crate::nature::NatureDoc,
    imports: Vec<ImportedAsset>,
    layer_surface: Vec<Surface>,
    camera: CameraDoc,
}

/// Compatibility-only metadata from the removed Effects Studio features. Keeping this opaque
/// avoids silently discarding user-authored data when an older world is opened and saved.
#[derive(Resource, Default)]
pub struct LegacyEffectsMetadata {
    reference_effects: Option<serde_json::Value>,
    effects_studio: Option<serde_json::Value>,
}

#[derive(SystemParam)]
pub struct WorldData<'w> {
    pub terrain: ResMut<'w, Terrain>,
    pub zones: ResMut<'w, Zones>,
    pub store: ResMut<'w, ObjectStore>,
    pub water: ResMut<'w, WaterBodies>,
    pub water_style: ResMut<'w, WaterStyle>,
    pub lighting: ResMut<'w, Lighting>,
    pub wind: ResMut<'w, Wind>,
    pub graphics: ResMut<'w, crate::streaming::Graphics>,
    pub amounts: ResMut<'w, crate::objects::NatureAmounts>,
    pub appearance: ResMut<'w, crate::nature::NatureAppearance>,
    pub surface_choices: ResMut<'w, crate::surfaces::SurfaceChoices>,
    pub layer_choices: ResMut<'w, crate::terrain::textures::TerrainLayerChoices>,
    pub meta: ResMut<'w, WorldMeta>,
    pub history: ResMut<'w, History>,
    pub ed: ResMut<'w, Editor>,
    pub orbit: ResMut<'w, OrbitCamera>,
    pub lib: Option<ResMut<'w, Library>>,
    pub gameplay: ResMut<'w, crate::gameplay::Gameplay>,
    pub roads: ResMut<'w, crate::roads::Roads>,
    pub fields: ResMut<'w, crate::fields::Fields>,
    /// fitted objects' pads and the ground under them; model bounds
    pub fit: crate::placement::FitParams<'w>,
    pub analysis: ResMut<'w, crate::analysis::Analysis>,
    pub motion: ResMut<'w, crate::motion::MotionData>,
    pub caves: ResMut<'w, crate::caves::Caves>,
    pub dungeons: ResMut<'w, crate::dungeons::Dungeons>,
    pub dungeon_map: ResMut<'w, crate::dungeons::DungeonMap>,
    pub map: ResMut<'w, crate::worldmap::WorldMap>,
    pub reference: ResMut<'w, crate::refmap::RefMap>,
    pub legacy_effects: ResMut<'w, LegacyEffectsMetadata>,
    pub nature: ResMut<'w, crate::nature::NatureField>,
    pub nature_pending: ResMut<'w, crate::nature::migrate::Pending>,
    pub nature_rules: Option<Res<'w, crate::nature::Rules>>,
    pub nature_masks: Res<'w, crate::nature::Masks>,
}

pub struct IoPlugin;

impl Plugin for IoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Autosave>()
            .init_resource::<LegacyEffectsMetadata>()
            .add_systems(
                Update,
                (handle_actions, autoload)
                    .chain()
                    .after(crate::tools::ToolSystems::Commit),
            )
            .add_systems(Last, (track_revision, autosave).chain());
        // a GPU error quits the app (Bevy's default); keep a recovery copy of unsaved work first
        app.insert_resource(bevy::render::error_handler::RenderErrorHandler(
            |error, main_world, _| {
                error!(
                    "Quitting after a {:?} render error; saving a recovery copy first",
                    error.ty
                );
                let _ = main_world.run_system_cached(save_recovery);
                main_world.write_message(AppExit::error());
                bevy::render::error_handler::RenderErrorPolicy::StopRendering
            },
        ));
    }
}

/// Autosave: every couple of minutes with unsaved changes (never while walking or mid-stroke) and
/// when the window closes. A saved world saves in place; an untitled one goes to a single
/// recovery slot. Only files whose content changed are rewritten, each atomically (temp + rename),
/// so nothing accumulates on disk and a crash mid-write can't corrupt the world.
#[derive(Resource)]
pub struct Autosave {
    pub enabled: bool,
    /// Seconds between autosaves.
    pub every: f32,
    /// Real time of the last save (manual or automatic) and whether it was automatic.
    pub last: Option<(f32, bool)>,
}

impl Default for Autosave {
    fn default() -> Self {
        let enabled = crate::prefs::get_bool("autosave").unwrap_or(true);
        Self {
            enabled,
            every: 120.0,
            last: None,
        }
    }
}

impl Autosave {
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        crate::prefs::set("autosave", serde_json::json!(on));
    }
}

/// The recovery slot for untitled worlds (and for any world when the app must quit on an
/// error). Saving the world normally removes it.
/// (`WB_RECOVERY_DIR=<dir>` redirects it and turns autosave on in automated runs, every 5 s.)
pub(crate) fn recovery_path() -> PathBuf {
    match std::env::var("WB_RECOVERY_DIR") {
        Ok(d) => PathBuf::from(d).join("_recovery.world"),
        Err(_) => worlds_dir().join("_recovery.world"),
    }
}

fn automated_run() -> bool {
    (std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok())
        && std::env::var("WB_RECOVERY_DIR").is_err()
}

/// Saves unsaved work without asking: in place for a saved world, else to the recovery slot.
/// The main thread only takes a snapshot; the files are written on a background thread unless
/// `wait` (closing the window, render errors).
fn autosave_now(data: &mut WorldData, why: &'static str, wait: bool) {
    // never two saves at once
    finish_save(data, save::wait());
    let in_place = data.meta.path.clone().filter(|p| *p != recovery_path());
    let dir = in_place.clone().unwrap_or_else(recovery_path);
    let snap = save::Snapshot::take(&dir, data);
    if wait {
        let (snapshot_ms, t0) = (snap.ms, std::time::Instant::now());
        let result = snap.write();
        let write_ms = t0.elapsed().as_secs_f32() * 1000.0;
        finish_save(
            data,
            Some(save::Done {
                dir,
                in_place: in_place.is_some(),
                revision: data.meta.revision,
                why,
                snapshot_ms,
                write_ms,
                result,
            }),
        );
    } else {
        save::start(snap, in_place.is_some(), data.meta.revision, why);
    }
}

/// A finished autosave: the world is clean if it was saved in place and nothing changed since
/// its snapshot.
fn finish_save(data: &mut WorldData, done: Option<save::Done>) {
    let Some(d) = done else { return };
    match d.result {
        Ok(()) => {
            info!(
                "autosave ({}) to {}: snapshot {:.1} ms on the main thread, written in {:.0} ms",
                d.why,
                d.dir.display(),
                d.snapshot_ms,
                d.write_ms
            );
            if d.in_place && data.meta.path.as_deref() == Some(d.dir.as_path()) {
                // the world on disk is current: no recovery copy needed
                let _ = fs::remove_dir_all(recovery_path());
                if data.meta.revision == d.revision {
                    data.meta.dirty = false;
                }
            }
        }
        Err(e) => warn!("autosave failed: {e}"),
    }
}

/// Counts the frames in which the world was edited (`WorldMeta::revision`), so a background
/// save knows whether edits happened after its snapshot.
fn track_revision(mut meta: ResMut<WorldMeta>) {
    if meta.is_changed() && meta.dirty {
        meta.bypass_change_detection().revision += 1;
    }
}

pub(crate) fn save_recovery(mut data: WorldData) {
    if !automated_run() && data.meta.dirty {
        autosave_now(&mut data, "render error", true);
    }
}

fn autosave(
    mut data: WorldData,
    mut auto: ResMut<Autosave>,
    time: Res<Time<Real>>,
    walk: Res<crate::walk::WalkState>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut closing: MessageReader<bevy::window::WindowCloseRequested>,
    mut exits: MessageReader<AppExit>,
    mut last: Local<f32>,
    mut told: Local<bool>,
) {
    finish_save(&mut data, save::finished());
    if automated_run() || data.lib.is_none() {
        return;
    }
    let now = time.elapsed_secs();
    if !*told {
        *told = true;
        *last = now;
        if let Ok(m) = fs::metadata(recovery_path().join("world.json")).and_then(|m| m.modified()) {
            let mins = m.elapsed().map(|d| d.as_secs() / 60).unwrap_or(0);
            data.ed.toast(format!(
                "Unsaved work from {mins} min ago was kept: Open → worlds/_recovery.world"
            ));
        }
    }
    // closing the window: keep unsaved work
    let quitting = closing.read().count() > 0 || exits.read().count() > 0;
    if quitting {
        // a background save finishes before the editor quits
        finish_save(&mut data, save::wait());
        if data.meta.dirty && auto.enabled {
            autosave_now(&mut data, "closing", true);
        }
        return;
    }
    let every = if std::env::var("WB_RECOVERY_DIR").is_ok() {
        5.0
    } else {
        auto.every
    };
    if !auto.enabled
        || !data.meta.dirty
        || now - *last < every
        || walk.active
        || mouse.pressed(MouseButton::Left)
        || data.map.capturing()
    {
        return;
    }
    *last = now;
    autosave_now(&mut data, "timer", false);
    auto.last = Some((now, true));
}

pub(crate) fn worlds_dir() -> PathBuf {
    let d = crate::paths::worlds_dir();
    let _ = fs::create_dir_all(&d);
    d
}

pub fn assets_dir() -> PathBuf {
    crate::paths::user_assets_dir()
}

/// The world to reopen next time. Automated runs (screenshots, tests) never change it.
fn remember_last(path: &Path) {
    if std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok() {
        return;
    }
    let _ = fs::write(
        worlds_dir().join(".last"),
        path.to_string_lossy().as_bytes(),
    );
}

/// Reopen the most recently saved world on startup (unless WB_FRESH is set).
fn autoload(mut done: Local<bool>, mut data: WorldData) {
    if *done {
        return;
    }
    if data.lib.is_none() {
        return; // wait for the asset library
    }
    *done = true;
    if std::env::var("WB_FRESH").is_ok() {
        return;
    }
    let Ok(last) = fs::read_to_string(worlds_dir().join(".last")) else {
        // first run: a living starting world (biomes, ground, forests, plants); no zones — those
        // are the crafter's to add
        {
            let seed = data.meta.seed;
            data.history.begin("Generate biomes");
            crate::biomegen::generate(
                &mut data.terrain,
                &mut data.zones,
                &mut data.history,
                &mut data.water,
                seed,
                crate::biomegen::Climate::Varied,
            );
            data.history.commit(&data.store);
            data.history.clear();
        }
        data.ed.toast("Welcome! A starting world is ready: walk it (Walk the World), reshape it, or generate a new one in Land (1). Press H for help");
        return;
    };
    let path = PathBuf::from(last.trim());
    if path.exists() {
        match load_world(&path, &mut data) {
            Ok(()) => data.ed.toast(format!("Opened {}", data.meta.name)),
            Err(e) => warn!("could not reopen {path:?}: {e}"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_actions(
    _main_thread: NonSendMarker,
    mut frames: Local<u32>,
    mut actions: MessageReader<UiAction>,
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<crate::camera::Pointer>,
    mut data: WorldData,
) {
    let mut list: Vec<UiAction> = actions.read().cloned().collect();
    let cmd = keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
        || keys.pressed(KeyCode::ControlLeft);
    if cmd && !pointer.keyboard_captured {
        if keys.just_pressed(KeyCode::KeyS) {
            list.push(if keys.pressed(KeyCode::ShiftLeft) {
                UiAction::SaveAs
            } else {
                UiAction::Save
            });
        }
        if keys.just_pressed(KeyCode::KeyO) {
            list.push(UiAction::Open);
        }
        if keys.just_pressed(KeyCode::KeyE) {
            list.push(UiAction::Export);
        }
    }
    // `WB_NEW_FROM_PICTURE=<png>` (tests): File › New › From a picture, once the editor is up
    *frames += 1;
    if *frames == 150
        && let Ok(p) = std::env::var("WB_NEW_FROM_PICTURE")
    {
        list.push(UiAction::NewWorldFromPicture {
            path: p.into(),
            size_m: 4096.0,
            art_style: crate::art_style::ArtStyle::Realistic,
            appearance: default(),
        });
    }
    // file dialogs answer on a later frame (crate::dialogs)
    if let Some(p) = crate::dialogs::take_file("world_save") {
        list.push(UiAction::SaveTo(
            if p.extension().is_some_and(|e| e == "world") {
                p
            } else {
                p.with_extension("world")
            },
        ));
    }
    if let Some(p) = crate::dialogs::take_file("world_open") {
        list.push(UiAction::OpenPath(p));
    }
    if let Some(files) = crate::dialogs::take_files("import_models") {
        list.push(UiAction::ImportFiles {
            files,
            from: String::new(),
        });
    }
    for a in list {
        match a {
            UiAction::NewWorldFromPicture {
                path,
                size_m,
                art_style,
                appearance,
            } => {
                // the picture laid over the world, then the reference builder makes a new world of
                // that size from it (refmap::ui::run_build, a few frames later); the trace panel
                // stays open to fine-tune and rebuild
                let size = data.terrain.size();
                match data.reference.load(&path, size) {
                    Ok(()) => {
                        data.reference.doc.build.world_size = size_m;
                        data.reference.new_world_art_style = Some(art_style);
                        data.reference.new_world_appearance = Some(appearance);
                        data.reference.request = Some(crate::refmap::RefRequest::Build);
                        data.ed.tab = crate::editor::Tab::Land;
                        data.ed.land_tool = crate::editor::LandTool::Reference;
                        data.ed.toast(format!(
                            "Building a {:.0} km world from {}…",
                            size_m / 1024.0,
                            path.file_name()
                                .map(|f| f.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ));
                    }
                    Err(e) => data.ed.toast(format!("Couldn't read that picture: {e}")),
                }
            }
            UiAction::NewWorld {
                preset,
                res,
                cell,
                seed,
                biomes,
                art_style,
                appearance,
            } => {
                new_world_styled(&mut data, preset, res, cell, seed, art_style);
                *data.appearance = appearance.sanitized();
                let mut extra = String::new();
                if let Some(climate) = biomes {
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
                    extra = format!(
                        " with {} biome regions and their forests, plants and rocks",
                        g.regions
                    );
                }
                data.ed
                    .toast(format!("Created a new {} world{extra}", preset.name()));
                data.gameplay.ore_fill_pending = true;
            }
            UiAction::Regenerate {
                preset,
                seed,
                biomes,
            } => {
                let is_nature = |lib: &crate::objects::Library, o: &crate::objects::Placed| {
                    lib.get(&o.asset)
                        .is_some_and(|e| Category::NATURE.contains(&e.category))
                };
                let t = &mut data.terrain;
                let all = full_rect(t);
                data.history.begin(format!("Generate {}", preset.name()));
                data.history.touch_terrain(t, all);
                t.heights = worldgen::build_world(preset, t.res, t.cell, t.sea_level, seed);
                t.mark_all_dirty();
                data.meta.seed = seed;

                // The old forests, rivers and lakes belong to the old landscape: clear them.
                // Structures stay and follow the new ground (objects::follow_ground).
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
                water.fish.retain(|f| {
                    terrain.height_at(f.center[0], f.center[2]) < terrain.sea_level - 1.0
                });
                water.selected = None;
                water.dirty = true;

                // biomes: biome map, ground and water for the new land (procedural nature follows
                // the biomes; a populated world's nature grows on the new land by itself)
                crate::nature::reset_paint(&mut data.nature, &mut data.history, "Generate");
                let mut made = 0;
                if let Some(climate) = biomes {
                    made = crate::biomegen::generate(
                        &mut data.terrain,
                        &mut data.zones,
                        &mut data.history,
                        &mut data.water,
                        seed,
                        climate,
                    )
                    .regions;
                }
                crate::roads::build::forget_ground(&mut data.roads, &mut data.history);
                crate::placement::fit::forget(&mut data.fit.found, &mut data.history);
                data.history.commit(&data.store);
                data.meta.dirty = true;
                let extra = if made > 0 {
                    format!(" with {made} biome regions and their nature")
                } else {
                    String::new()
                };
                data.ed.toast(format!(
                    "Generated a new {}{extra} (undo to go back)",
                    preset.name()
                ));
                data.gameplay.ore_fill_pending = true;
            }
            UiAction::ErodeWorld => {} // bigworld erodes in the background (progress, cancel, undo)
            UiAction::SmoothWorld => {
                let t = &mut data.terrain;
                let all = full_rect(t);
                data.history.begin("Soften world");
                data.history.touch_terrain(t, all);
                let res = t.res;
                worldgen::smooth_all(&mut t.heights, res, 0.6);
                t.mark_all_dirty();
                data.history.commit(&data.store);
                data.meta.dirty = true;
            }
            UiAction::ClearPaint => {
                let t = &mut data.terrain;
                let all = full_rect(t);
                data.history.begin("Clear paint");
                data.history.touch_terrain(t, all);
                t.splat.iter_mut().for_each(|s| *s = [0; LAYER_COUNT]);
                t.mark_splat_dirty(all);
                data.history.commit(&data.store);
                data.ed
                    .toast("All paint cleared: ground follows the automatic style again");
            }
            UiAction::Save => match data.meta.path.clone() {
                Some(p) => save_and_report(&p, &mut data),
                None => save_dialog(&data.meta.name),
            },
            UiAction::SaveAs => save_dialog(&data.meta.name),
            UiAction::SaveTo(p) => save_and_report(&p, &mut data),
            UiAction::Open => crate::dialogs::pick_folder(
                "world_open",
                rfd::AsyncFileDialog::new()
                    .set_title("Open world folder (.world)")
                    .set_directory(worlds_dir()),
            ),
            UiAction::OpenPath(p) => match load_world(&p, &mut data) {
                Ok(()) if p == recovery_path() => {
                    // restored work still needs a real home: Save asks where
                    data.meta.path = None;
                    data.meta.dirty = true;
                    data.ed.toast("Recovered unsaved work: Save to keep it");
                }
                Ok(()) => data.ed.toast(format!("Opened {}", data.meta.name)),
                Err(e) => data.ed.toast(format!("Couldn't open world: {e}")),
            },
            // the export dialog (engine targets, options, progress) lives in crate::export
            UiAction::Export => crate::export::request_dialog(),
            // File › Import: its dialog answers as ImportFiles
            UiAction::ImportModel => crate::dialogs::pick_files(
                "import_models",
                rfd::AsyncFileDialog::new()
                    .set_title("Import 3D models")
                    .add_filter("glTF models", &["glb", "gltf"]),
            ),
            UiAction::ImportFiles { files, from } => {
                // (`from`: a marketplace listing's title; empty for File › Import)
                let from = Some(from).filter(|f| !f.is_empty());
                {
                    let mut n = 0;
                    for f in files {
                        match import_model(&f) {
                            Ok(imp) => {
                                // the asset rules' cheap checks, in the background: a very heavy
                                // model gets a friendly one-time card (never a block)
                                crate::asset_rules::check_import(
                                    crate::asset_file(&imp.path),
                                    imp.name.clone(),
                                );
                                if let Some(lib) = data.lib.as_mut() {
                                    lib.add(imported_entry(&imp));
                                }
                                n += 1;
                            }
                            Err(e) => data.ed.toast(format!("Import failed: {e}")),
                        }
                    }
                    if n > 0 {
                        match &from {
                            Some(from) => data.ed.toast(format!(
                                "{from} is in your library — find it under Objects › Imported"
                            )),
                            None => data.ed.toast(format!(
                                "Imported {n} model(s) — find them under Objects › Imported"
                            )),
                        }
                        data.ed.tab = crate::editor::Tab::Objects;
                        data.meta.dirty = true;
                    }
                }
            }
            UiAction::PopulateWorld => {
                // procedural: the whole world grows nature fitted to the land, at once
                crate::nature::populate(&mut data.nature, &mut data.amounts, &mut data.history);
                data.meta.dirty = true;
                data.ed.toast("Forests, meadows and rocks now grow across the world, fitted to the land and its biomes — paint, thin or erase to refine");
            }
            UiAction::ClearNature => {
                let Some(lib) = data.lib.as_deref() else {
                    continue;
                };
                // placed trees, rocks and plants go; procedural nature is dialled to nothing
                data.history.begin("Clear nature");
                let ids: Vec<u64> = data
                    .store
                    .items
                    .iter()
                    .filter(|(_, o)| {
                        lib.get(&o.asset)
                            .is_some_and(|e| Category::NATURE.contains(&e.category))
                    })
                    .map(|(id, _)| *id)
                    .collect();
                for id in &ids {
                    data.history.touch_object(&data.store, *id);
                    data.store.remove(*id);
                }
                data.history.commit(&data.store);
                crate::nature::clear(&mut data.nature, &mut data.amounts, &mut data.history);
                data.meta.dirty = true;
                data.ed.toast("Nature cleared (the Nature dials are at 0%: raise them, or Auto-populate, to bring it back)");
            }
            UiAction::ToggleWalk => {}
        }
    }
}

fn full_rect(t: &Terrain) -> GridRect {
    let n = t.n();
    GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    }
}

pub(crate) fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim().replace(' ', "_");
    if s.is_empty() { "world".into() } else { s }
}

/// Asks where to save (the answer comes back as `UiAction::SaveTo`).
fn save_dialog(name: &str) {
    crate::dialogs::save_file(
        "world_save",
        rfd::AsyncFileDialog::new()
            .set_title("Save world")
            .set_directory(worlds_dir())
            .set_file_name(format!("{}.world", sanitize(name))),
    );
}

pub(crate) fn save_and_report(path: &Path, data: &mut WorldData) {
    finish_save(data, save::wait());
    // (a world saved somewhere new takes its dungeons' maps along)
    let from = data.meta.path.clone().unwrap_or_else(recovery_path);
    crate::dungeons::session::carry_maps(&from, path);
    match save_world(path, data) {
        Ok(()) => {
            data.meta.path = Some(path.to_path_buf());
            data.meta.dirty = false;
            if let Some(stem) = path.file_stem() {
                data.meta.name = stem.to_string_lossy().replace('_', " ");
            }
            remember_last(&path);
            // saved for real: the recovery copy is stale now
            if path != recovery_path() {
                let _ = fs::remove_dir_all(recovery_path());
            }
            data.ed.toast(format!("Saved {}", path.display()));
        }
        Err(e) => data.ed.toast(format!("Save failed: {e}")),
    }
}

pub fn new_world(
    data: &mut WorldData,
    preset: worldgen::WorldPreset,
    res: usize,
    cell: f32,
    seed: u32,
) {
    new_world_from(
        data,
        worldgen::build_world(preset, res, cell, 0.0, seed),
        res,
        cell,
        seed,
    );
}

/// Creates a fresh world with its selected art style, without recording the discarded world in
/// undo history.
pub fn new_world_styled(
    data: &mut WorldData,
    preset: worldgen::WorldPreset,
    res: usize,
    cell: f32,
    seed: u32,
    art_style: crate::art_style::ArtStyle,
) {
    new_world(data, preset, res, cell, seed);
    crate::art_style::initialize_new_world(&mut data.meta, &mut data.lighting, art_style);
}

/// Like [`new_world_from`], preserving an explicitly selected style for a new world.
pub fn new_world_from_styled(
    data: &mut WorldData,
    heights: Vec<f32>,
    res: usize,
    cell: f32,
    seed: u32,
    art_style: crate::art_style::ArtStyle,
) {
    new_world_from(data, heights, res, cell, seed);
    crate::art_style::initialize_new_world(&mut data.meta, &mut data.lighting, art_style);
}

/// A new world on the given heights (sea level 0); everything else starts empty.
pub fn new_world_from(data: &mut WorldData, heights: Vec<f32>, res: usize, cell: f32, seed: u32) {
    let generation = data.terrain.generation + 1;
    let mut t = Terrain::new(res, cell, 0.0);
    t.heights = heights;
    t.generation = generation;
    t.mark_all_dirty();
    *data.terrain = t;
    data.store.clear();
    *data.nature = default();
    *data.appearance = default();
    data.nature_pending.0 = None;
    *data.water = WaterBodies {
        dirty: true,
        ..default()
    };
    // (the ore mix is a designer's choice: a new world keeps it, and gets ore when generated)
    let ore_settings = data.gameplay.ore_settings.clone();
    *data.gameplay = crate::gameplay::Gameplay {
        dirty: true,
        ore_settings,
        ..default()
    };
    *data.roads = crate::roads::Roads {
        dirty: true,
        ..default()
    };
    *data.fields = crate::fields::Fields {
        revision: data.fields.revision + 1,
        ..default()
    };
    *data.fit.found = default();
    data.analysis.load_settings(default());
    *data.motion = default();
    *data.caves = default();
    *data.dungeons = default();
    data.dungeon_map.0 = None;
    crate::nature::forget_snapshot();
    data.reference.clear();
    *data.legacy_effects = default();
    data.zones.list.clear();
    data.zones.selected = None;
    data.history.clear();
    data.meta.name = "Untitled World".into();
    data.meta.path = None;
    data.meta.seed = seed;
    data.meta.art_style = Default::default();
    data.meta.dirty = true;
    let size = res as f32 * cell;
    data.orbit
        .set_view(Vec3::new(0.0, 0.0, size * 0.08), 0.35, 0.62, size * 0.8);
    data.orbit.snap();
}

fn write_raw<T: bytemuck::Pod>(path: &Path, v: &[T]) -> std::io::Result<()> {
    write_if_changed(path, bytemuck::cast_slice(v))
}

/// Content hashes of the files this session wrote: unchanged files are not rewritten.
static WRITTEN: std::sync::Mutex<Option<HashMap<PathBuf, u64>>> = std::sync::Mutex::new(None);

/// Fast 64-bit content hash (word-wise multiply–xorshift), ~10 GB/s.
fn content_hash(b: &[u8]) -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15 ^ b.len() as u64;
    let mut chunks = b.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().unwrap());
        h = (h ^ w).wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        h ^= h >> 29;
    }
    for &x in chunks.remainder() {
        h = (h ^ x as u64).wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    }
    h ^ (h >> 32)
}

/// Writes `bytes` to `path` unless this session already wrote exactly that; atomically
/// (a temp file renamed over the old one).
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let h = content_hash(bytes);
    let mut cache = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    let map = cache.get_or_insert_with(HashMap::new);
    if map.get(path) == Some(&h) && path.exists() {
        return Ok(());
    }
    let tmp = path.with_extension("tmp-write");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    map.insert(path.to_path_buf(), h);
    Ok(())
}

/// Saves now (on this thread), through the same snapshot as the background autosave.
pub fn save_world(dir: &Path, data: &WorldData) -> Result<(), String> {
    // (never alongside a background save)
    let _ = save::wait();
    let snap = save::Snapshot::take(dir, data);
    let (ms, t0) = (snap.ms, std::time::Instant::now());
    snap.write()?;
    info!(
        "saved {} (snapshot {ms:.1} ms, written in {:.0} ms)",
        dir.display(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    Ok(())
}

pub fn load_world(dir: &Path, data: &mut WorldData) -> Result<(), String> {
    finish_save(data, save::wait());
    let t0 = std::time::Instant::now();
    let json = fs::read_to_string(dir.join("world.json")).map_err(|e| e.to_string())?;
    let doc: WorldDoc = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    let n = doc.res + 1;
    let hb = fs::read(dir.join("heights.f32")).map_err(|e| e.to_string())?;
    if hb.len() != n * n * 4 {
        return Err("heights.f32 has the wrong size".into());
    }
    let mut t = Terrain::new(doc.res, doc.cell, doc.sea_level);
    t.heights = hb
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    if let Ok(sb) = fs::read(dir.join("splat.u8"))
        && sb.len() == n * n * LAYER_COUNT
    {
        t.splat = sb
            .chunks_exact(LAYER_COUNT)
            .map(|c| c.try_into().unwrap())
            .collect();
    }
    if let Ok(zb) = fs::read(dir.join("zones.u8"))
        && zb.len() == n * n
    {
        t.zones = zb;
    }
    if let Ok(bb) = fs::read(dir.join("biomes.u8"))
        && bb.len() == n * n
    {
        t.biomes = bb;
    }
    t.style = doc.style;
    t.generation = data.terrain.generation + 1;
    t.mark_all_dirty();
    *data.terrain = t;

    if let Some(lib) = data.lib.as_mut() {
        // (worlds saved before only the user's imports were listed also list the bundled
        // models: those keep their own library entries — category, group, walk-in rooms)
        let own: Vec<&ImportedAsset> = doc
            .imports
            .iter()
            .filter(|imp| {
                lib.get(&imp.id)
                    .is_none_or(|e| e.category == Category::Imported)
            })
            .collect();
        for imp in own {
            lib.add(imported_entry(imp));
        }
    }
    data.store.clear();
    let objects = match &doc.objects_file {
        Some(f) => {
            crate::bigworld::decode_objects(&fs::read(dir.join(f)).map_err(|e| e.to_string())?)?
        }
        None => doc.objects,
    };
    // procedural nature; an old world's tree, rock, plant and debris objects become density
    // (`nature::migrate`, a few frames later; lake shore plants stay objects)
    *data.nature = crate::nature::load(dir, &doc.procedural)?;
    *data.appearance = doc.procedural.appearance.sanitized();
    let objects = if doc.procedural.version == 0 {
        let keep: std::collections::HashSet<u64> = doc
            .lakes
            .iter()
            .flat_map(|l| l.plant_ids.iter().copied())
            .collect();
        let (objects, nature) = crate::nature::migrate::split(objects, data.lib.as_deref(), &keep);
        data.nature_pending.0 = (!nature.is_empty()).then_some(nature);
        objects
    } else {
        data.nature_pending.0 = None;
        objects
    };
    let count = objects.len();
    for o in objects {
        data.store.insert(o);
    }
    info!(
        "loaded {} ({count} objects) in {:.0} ms",
        dir.display(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    let next = doc
        .rivers
        .iter()
        .map(|r| r.id)
        .chain(doc.lakes.iter().map(|l| l.id))
        .chain(doc.fish.iter().map(|f| f.id))
        .chain(doc.waterfalls.iter().map(|f| f.id))
        .max()
        .unwrap_or(0);
    *data.water = WaterBodies {
        rivers: doc.rivers,
        lakes: doc.lakes,
        fish: doc.fish,
        waterfalls: doc.waterfalls,
        next_id: next,
        dirty: true,
        ..default()
    };
    *data.water_style = doc.water_style;
    *data.gameplay = crate::gameplay::Gameplay {
        dirty: true,
        ..doc.gameplay
    };
    *data.roads = crate::roads::Roads {
        dirty: true,
        ..doc.roads
    };
    *data.fields = crate::fields::Fields {
        revision: data.fields.revision + 1,
        ..doc.fields
    };
    *data.fit.found = doc.foundations;
    data.analysis.load_settings(doc.analysis);
    *data.motion = doc.motion;
    *data.caves = doc.caves;
    *data.dungeons = doc.dungeons;
    data.dungeon_map.0 = doc.dungeon_map;
    // Sheet maps saved before raised bridge decks existed are repaired once, from their own
    // preserved blueprint and build floor.  This touches neither terrain, water nor objects.
    let bridges_reconciled = crate::dungeons::sheets::bridges::reconcile_loaded(
        dir,
        &mut data.roads,
        &mut data.dungeon_map.0,
    );
    crate::nature::forget_snapshot();
    data.map.settings = doc.map;
    crate::refmap::load_doc(&mut data.reference, doc.reference, dir);
    *data.legacy_effects = LegacyEffectsMetadata {
        reference_effects: doc.reference_effects,
        effects_studio: doc.effects_studio,
    };
    *data.lighting = doc.lighting;
    data.wind.dir = Vec2::new(doc.wind[0], doc.wind[1]);
    // worlds saved before wind had units stored a 0..0.8 "sway" value
    let w = doc.wind.get(2).copied().unwrap_or(2.5);
    data.wind.speed = if w <= 0.8 { w / 0.18 * 2.5 } else { w };
    data.wind.gustiness = doc.wind.get(3).copied().unwrap_or(0.5);
    *data.graphics = doc.graphics;
    *data.surface_choices = doc.building_materials;
    *data.layer_choices = doc.terrain_layers;
    // the loaded plants were grown with the saved amount
    *data.amounts = crate::objects::NatureAmounts {
        applied_plants: doc.nature.plants,
        ..doc.nature
    };
    data.zones.list = doc.zones;
    data.zones.selected = None;
    for (i, s) in doc.layer_surface.iter().enumerate().take(LAYER_COUNT) {
        data.ed.layer_surface[i] = *s;
    }
    data.history.clear();
    data.meta.name = doc.name;
    data.meta.seed = doc.seed;
    data.meta.art_style = doc.art_style.unwrap_or_else(|| match &data.dungeon_map.0 {
        Some(m) if m.art == crate::dungeons::styles::Art::HandPainted => {
            crate::art_style::ArtStyle::HandPainted
        }
        _ => crate::art_style::ArtStyle::Realistic,
    });
    data.meta.path = Some(dir.to_path_buf());
    data.meta.dirty = bridges_reconciled;
    let c = doc.camera;
    data.orbit
        .set_view(Vec3::from_array(c.focus), c.yaw, c.pitch, c.distance);
    data.orbit.snap();
    remember_last(dir);
    Ok(())
}

pub fn imported_entry(imp: &ImportedAsset) -> LibraryEntry {
    LibraryEntry {
        textured: false,
        pack: crate::objects::PHOTOREAL.into(),
        id: imp.id.clone(),
        name: imp.name.clone(),
        category: Category::Imported,
        group: String::new(),
        kind: None,
        import_path: Some(crate::paths::user_asset_path(&imp.path)),
        surface: imp.surface,
        radius: imp.radius,
        align: false,
        walkable: imp.walkable,
        whole_scene: false,
        biomes: crate::world::Biome::ALL.to_vec(),
        sway: false,
        default_scale: 1.0,
        meshes: vec![],
        scene: None,
        height: imp.radius * 2.0,
        tree: None,
    }
}

/// Copies a glTF/GLB (and any files a .gltf references) into `assets/imported/`.
fn import_model(src: &Path) -> Result<ImportedAsset, String> {
    let name = src
        .file_name()
        .ok_or("no file name")?
        .to_string_lossy()
        .to_string();
    let dst_dir = assets_dir().join("imported");
    fs::create_dir_all(&dst_dir).map_err(|e| e.to_string())?;
    fs::copy(src, dst_dir.join(&name)).map_err(|e| e.to_string())?;
    if src
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf"))
    {
        // copy external buffers / images next to it
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(src).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let base = src.parent().unwrap_or(Path::new("."));
        for key in ["buffers", "images"] {
            if let Some(arr) = json.get(key).and_then(|v| v.as_array()) {
                for item in arr {
                    if let Some(uri) = item.get("uri").and_then(|u| u.as_str())
                        && !uri.starts_with("data:")
                    {
                        let from = base.join(uri);
                        let to = dst_dir.join(uri);
                        if let Some(parent) = to.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        let _ = fs::copy(from, to);
                    }
                }
            }
        }
    }
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(name.clone());
    let pretty: String = stem.replace(['_', '-'], " ");
    Ok(ImportedAsset {
        id: format!("imported/{name}"),
        name: pretty.clone(),
        path: crate::paths::user_asset_path(&format!("imported/{name}")),
        surface: Surface::guess_from_name(&pretty),
        walkable: true,
        radius: 4.0,
    })
}

/// Engine-agnostic export package.
pub fn export_world(dir: &Path, data: &WorldData) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let t = &data.terrain;
    let n = t.n() as u32;
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for h in &t.heights {
        lo = lo.min(*h);
        hi = hi.max(*h);
    }
    let range = (hi - lo).max(1e-3);
    // heightmap: 16-bit PNG + raw r16 + raw f32
    let h16: Vec<u16> = t
        .heights
        .iter()
        .map(|h| (((h - lo) / range) * 65535.0).round() as u16)
        .collect();
    let img = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(n, n, h16.clone())
        .ok_or("heightmap buffer")?;
    img.save(dir.join("heightmap.png"))
        .map_err(|e| e.to_string())?;
    fs::write(
        dir.join("heightmap.r16"),
        bytemuck::cast_slice::<u16, u8>(&h16),
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        dir.join("heightmap.f32"),
        bytemuck::cast_slice::<f32, u8>(&t.heights),
    )
    .map_err(|e| e.to_string())?;

    // final material weights (automatic rules + paint) and footstep surfaces
    let mut mats = [
        vec![0u8; (n * n * 4) as usize],
        vec![0u8; (n * n * 4) as usize],
        vec![0u8; (n * n * 4) as usize],
    ];
    let mut surf = vec![0u8; (n * n) as usize];
    let mut surf_rgb = vec![0u8; (n * n * 3) as usize];
    for j in 0..t.n() {
        for i in 0..t.n() {
            let k = t.idx(i, j);
            let w = t.final_weights(i, j);
            let mut best = 0;
            for l in 0..LAYER_COUNT {
                mats[l / 4][k * 4 + l % 4] = (w[l] * 255.0).round() as u8;
                if w[l] > w[best] {
                    best = l;
                }
            }
            let mut s = data.ed.layer_surface[best];
            let ground = t.heights[k];
            let p = t.grid_to_world(i, j);
            let wl = data
                .water
                .water_level(Vec3::new(p.x, ground, p.y))
                .unwrap_or(f32::NEG_INFINITY)
                .max(t.sea_level);
            if wl - ground > 0.04 {
                s = Surface::Water;
            }
            surf[k] = s.index() as u8;
            let c = s.overlay_color();
            surf_rgb[k * 3..k * 3 + 3].copy_from_slice(&c);
        }
    }
    for (i, m) in mats.iter().enumerate() {
        image::RgbaImage::from_raw(n, n, m.clone())
            .ok_or("splat")?
            .save(dir.join(format!("materials_{i}.png")))
            .map_err(|e| e.to_string())?;
    }
    image::GrayImage::from_raw(n, n, surf)
        .ok_or("surf")?
        .save(dir.join("footstep_surfaces.png"))
        .map_err(|e| e.to_string())?;
    image::RgbImage::from_raw(n, n, surf_rgb)
        .ok_or("surf rgb")?
        .save(dir.join("footstep_surfaces_preview.png"))
        .map_err(|e| e.to_string())?;

    // zones
    image::GrayImage::from_raw(n, n, t.zones.clone())
        .ok_or("zones")?
        .save(dir.join("zones.png"))
        .map_err(|e| e.to_string())?;
    let mut zrgb = vec![0u8; (n * n * 3) as usize];
    for (k, z) in t.zones.iter().enumerate() {
        if let Some(zone) = data.zones.by_id(*z) {
            let c = zone.color;
            zrgb[k * 3] = (c[0] * 255.0) as u8;
            zrgb[k * 3 + 1] = (c[1] * 255.0) as u8;
            zrgb[k * 3 + 2] = (c[2] * 255.0) as u8;
        }
    }
    image::RgbImage::from_raw(n, n, zrgb)
        .ok_or("zone rgb")?
        .save(dir.join("zones_preview.png"))
        .map_err(|e| e.to_string())?;
    // biomes
    image::GrayImage::from_raw(n, n, t.biomes.clone())
        .ok_or("biomes")?
        .save(dir.join("biomes.png"))
        .map_err(|e| e.to_string())?;

    // metadata
    let lib = data.lib.as_ref();
    // placed objects plus every procedural tree, rock, plant and piece of debris (baked; procedural
    // ids are strings "n<hex>": they don't fit a JSON number)
    let all = crate::nature::all_placed(
        &data.store,
        crate::nature::data_ctx(data).as_ref(),
        &[0, 1, 2, 3],
    );
    let objects: Vec<serde_json::Value> = all
        .iter()
        .map(|o| {
            let e = lib.and_then(|l| l.get(&o.asset));
            let id = if crate::nature::place::is_procedural(o.id) {
                serde_json::json!(format!("n{:x}", o.id))
            } else {
                serde_json::json!(o.id)
            };
            serde_json::json!({
                "id": id,
                "asset": o.asset,
                "name": e.map(|e| e.name.clone()),
                "category": e.map(|e| format!("{:?}", e.category)),
                "variant": o.variant,
                "position": o.pos,
                "rotation_xyzw": o.rot,
                "scale": o.scale,
                "scale_xyz": o.scale3().to_array(),
                "footstep_surface": e.map(|e| e.surface.sound_stem()),
                "walkable": e.map(|e| e.walkable),
                "model": e.and_then(|e| e.import_path.clone()),
            })
        })
        .collect();
    let surfaces: Vec<serde_json::Value> = Surface::ALL
        .iter()
        .map(|s| serde_json::json!({"index": s.index(), "name": s.name(), "sound": s.sound_stem(), "preview_rgb": s.overlay_color()}))
        .collect();
    let layers: Vec<serde_json::Value> = (0..LAYER_COUNT)
        .map(|i| {
            let l = Layer::from_index(i);
            let channel = ["r", "g", "b", "a"][i % 4];
            serde_json::json!({"index": i, "name": l.name(), "texture": format!("materials_{}.png", i / 4), "channel": channel, "footstep_surface": data.ed.layer_surface[i].sound_stem()})
        })
        .collect();
    let meta = serde_json::json!({
        "format": "world-builder-export/1",
        "name": data.meta.name,
        "size_m": t.size(),
        "resolution": n,
        "meters_per_pixel": t.cell,
        "origin": "world (0,0) is the map centre; +X east, +Z south, Y up; pixel (0,0) is the north-west corner",
        "height_min_m": lo,
        "height_max_m": hi,
        "sea_level_m": t.sea_level,
        "heightmap": {"png16": "heightmap.png", "raw_r16": "heightmap.r16", "raw_f32": "heightmap.f32", "decode": "height = min + value/65535 * (max - min)"},
        "material_layers": layers,
        "footstep_surfaces": {"map": "footstep_surfaces.png", "legend": surfaces},
        "zones": {"map": "zones.png", "list": data.zones.list},
        "biomes": {
            "map": "biomes.png",
            "legend": crate::biomes::BiomePreset::ALL.iter().map(|b| {
                let d = b.def();
                serde_json::json!({
                    "id": b.id(), "name": d.name, "weather": d.weather.name(), "ambience": d.soundscape.name(),
                    "temperature_c": d.temperature, "wind_mps": d.wind,
                })
            }).collect::<Vec<_>>(),
        },
        "ocean": &*data.water_style,
        "rivers": data.water.rivers,
        "lakes": data.water.lakes,
        "waterfalls": data.water.waterfalls,
        "fishing_spots": data.water.fish.iter().map(|f| serde_json::json!({
            "id": f.id, "species": f.species.name(), "species_id": f.species.id(), "habitat": f.species.habitat(), "rarity": f.species.info().rarity,
            "position": f.center, "radius_m": f.radius, "fish_count": f.count,
        })).collect::<Vec<_>>(),
        "lighting": &*data.lighting,
        "art_style": data.meta.art_style.id(),
        "nature_appearance": data.appearance.sanitized(),
        "wind": {"direction_xz": [data.wind.dir.x, data.wind.dir.y], "speed_mps": data.wind.speed, "gustiness": data.wind.gustiness},
        "graphics": {
            "draw_distance_m": data.graphics.draw_distance,
            "detail_distance_scale": data.graphics.detail,
            "note": "suggested defaults; expose both as player settings (lower on weak GPUs)",
        },
        "objects": objects,
    });
    fs::write(
        dir.join("world.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    crate::worldmap::export_hook(&dir.join("map"), data);
    crate::gameplay::write_server_data(
        dir,
        &data.gameplay,
        &data.terrain,
        &data.zones,
        &data.water,
        &data.meta.name,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compatibility_doc() -> WorldDoc {
        WorldDoc {
            version: 1,
            name: "Compatibility world".into(),
            seed: 42,
            res: 64,
            cell: 8.0,
            sea_level: 3.0,
            style: default(),
            lighting: default(),
            water_style: default(),
            wind: vec![1.0, 0.0, 2.5, 0.5],
            graphics: default(),
            nature: default(),
            building_materials: default(),
            terrain_layers: default(),
            art_style: None,
            zones: vec![],
            rivers: vec![],
            lakes: vec![],
            fish: vec![],
            waterfalls: vec![],
            gameplay: default(),
            roads: default(),
            fields: default(),
            foundations: default(),
            analysis: default(),
            motion: default(),
            caves: default(),
            dungeons: default(),
            dungeon_map: None,
            map: default(),
            reference: default(),
            reference_effects: None,
            effects_studio: None,
            objects: vec![],
            objects_file: None,
            procedural: default(),
            imports: vec![],
            layer_surface: vec![],
            camera: CameraDoc {
                focus: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
                distance: 1.0,
            },
        }
    }

    #[test]
    fn world_document_accepts_absent_legacy_effect_fields() {
        let encoded = serde_json::to_value(compatibility_doc()).unwrap();
        assert!(encoded.get("reference_effects").is_none());
        assert!(encoded.get("effects_studio").is_none());

        let loaded: WorldDoc = serde_json::from_value(encoded).unwrap();
        assert!(loaded.reference_effects.is_none());
        assert!(loaded.effects_studio.is_none());
        assert_eq!(loaded.name, "Compatibility world");
        assert_eq!(loaded.seed, 42);
    }

    #[test]
    fn world_document_round_trips_opaque_legacy_effect_metadata() {
        let mut document = compatibility_doc();
        document.reference_effects = Some(serde_json::json!({
            "forests": [{"recipe": "preserve without interpreting"}],
            "custom_reference_data": [1, true, null]
        }));
        document.effects_studio = Some(serde_json::json!({
            "assets": [{"opaque_package": {"version": 7}}],
            "placements": ["legacy-placement"]
        }));

        let encoded = serde_json::to_value(document).unwrap();
        let loaded: WorldDoc = serde_json::from_value(encoded.clone()).unwrap();
        let round_tripped = serde_json::to_value(loaded).unwrap();

        assert_eq!(round_tripped["reference_effects"], encoded["reference_effects"]);
        assert_eq!(round_tripped["effects_studio"], encoded["effects_studio"]);
        assert_eq!(round_tripped["name"], "Compatibility world");
        assert_eq!(round_tripped["seed"], 42);
        assert_eq!(round_tripped["wind"], encoded["wind"]);
        assert_eq!(round_tripped["water_style"], encoded["water_style"]);
    }
}
