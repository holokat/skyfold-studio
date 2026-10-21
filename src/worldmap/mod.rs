//! The world map and minimap (Map tab): the world drawn as a map a game can ship and a studio
//! can show off, in four styles — Parchment (inked fantasy map), Satellite (a tiled top-down
//! orthographic render of the real 3D world), Atlas (hypsometric topographic map) and Minimal
//! (flat colours for in-game minimaps) — lettered with the bundled OFL fonts, with POI icons,
//! roads, zone borders, compass, scale and title. Generation runs on a background thread (the
//! satellite tiles are rendered by the GPU a few frames each first) and the result exports as
//! PNGs, a minimap tile pyramid, a fog-of-war mask and `map.json`.
//!
//! - `snapshot`: the copy of the world a generation works from;
//! - `render`: derived fields, the per-pixel base of each style and the pipeline;
//! - `features`, `labels`, `decor`: vector layers, icons and lettering, map furniture;
//! - `satellite`: the GPU capture; `export`: files; `minimap`: the HUD widget; `mcp`: agent tools.
//!
//! Test hooks: `WB_MAP=<style>[,size]` generates after the demo world is built (`WB_MAP_OUT=<dir>`
//! exports it there and quits), `WB_MAP_DEMO=1` adds sample POIs, roads, a player start and water
//! names (gameplay and roads may be empty in a fresh world); `WB_MAP_VIEW=1` opens the viewer;
//! `WB_MAP_STATS=1` logs what the generator read (trees, rivers, forest cover);
//! `WB_MAP_SHOT=x0,z0,x1,z1,width[,height]` takes a bare satellite capture of that world rectangle
//! into `WB_MAP_OUT` (and quits once written).

pub mod canvas;
mod decor;
pub mod export;
mod features;
pub mod field;
mod labels;
pub(crate) mod mcp;
pub mod minimap;
pub mod render;
mod satellite;
pub mod snapshot;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use bevy::prelude::*;
use bevy_egui::egui;
use serde::{Deserialize, Serialize};

pub use render::{MapResult, Progress};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MapStyle {
    #[default]
    Parchment,
    Satellite,
    Atlas,
    Minimal,
}

impl MapStyle {
    pub const ALL: [MapStyle; 4] = [
        MapStyle::Parchment,
        MapStyle::Satellite,
        MapStyle::Atlas,
        MapStyle::Minimal,
    ];
    pub fn name(self) -> &'static str {
        match self {
            MapStyle::Parchment => "Parchment",
            MapStyle::Satellite => "Satellite",
            MapStyle::Atlas => "Atlas",
            MapStyle::Minimal => "Minimal",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            MapStyle::Parchment => "parchment",
            MapStyle::Satellite => "satellite",
            MapStyle::Atlas => "atlas",
            MapStyle::Minimal => "minimal",
        }
    }
    pub fn from_id(s: &str) -> Option<MapStyle> {
        Self::ALL.into_iter().find(|m| {
            m.id().eq_ignore_ascii_case(s.trim()) || m.name().eq_ignore_ascii_case(s.trim())
        })
    }
    pub fn blurb(self) -> &'static str {
        match self {
            MapStyle::Parchment => {
                "Hand-inked fantasy map on aged paper: hatched peaks, tree symbols, rippled coasts."
            }
            MapStyle::Satellite => {
                "A true top-down render of your 3D world, tile by tile. No clouds or fog."
            }
            MapStyle::Atlas => {
                "Topographic: elevation colours, contour lines, hillshade, cased roads."
            }
            MapStyle::Minimal => "Flat, calm colours for in-game minimaps and HUDs.",
        }
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            MapStyle::Parchment => i::SCROLL,
            MapStyle::Satellite => i::GLOBE_HEMISPHERE_EAST,
            MapStyle::Atlas => i::MOUNTAINS,
            MapStyle::Minimal => i::SQUARES_FOUR,
        }
    }
}

/// Editor-only things (markers, handles) to leave out of satellite renders: add this component
/// and the capture hides the entity while it renders.
#[derive(Component, Default)]
pub struct HideFromMap;

/// Which layers a map shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MapLayers {
    pub labels: bool,
    pub zones: bool,
    pub roads: bool,
    pub pois: bool,
    pub grid: bool,
    /// Compass and scale bar.
    pub compass: bool,
}

impl Default for MapLayers {
    fn default() -> Self {
        Self {
            labels: true,
            zones: true,
            roads: true,
            pois: true,
            grid: false,
            compass: true,
        }
    }
}

/// The Map tab's choices (saved with the world).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MapSettings {
    pub style: MapStyle,
    /// Output side in px: 2048, 4096 or 8192.
    pub size: u32,
    pub layers: MapLayers,
    pub show_title: bool,
    /// Empty: the world's name.
    pub title: String,
    /// Lettered across the open sea (empty: none).
    pub sea_name: String,
}

impl Default for MapSettings {
    fn default() -> Self {
        Self {
            style: MapStyle::Parchment,
            size: 4096,
            layers: MapLayers::default(),
            show_title: true,
            title: String::new(),
            sea_name: String::new(),
        }
    }
}

pub const SIZES: [u32; 3] = [2048, 4096, 8192];

impl MapSettings {
    /// A power-of-two size the tiles and pyramid can split evenly (a hand-edited world file may
    /// hold anything).
    fn sanitized(&self) -> MapSettings {
        let mut s = self.clone();
        if !(512..=8192).contains(&s.size) || !s.size.is_power_of_two() {
            s.size = 4096;
        }
        s
    }
}

/// Pictures of a finished map, prepared off the main thread for the preview and the HUD.
pub struct PreviewImages {
    /// ≤ 2048 px, the whole map.
    pub overview: egui::ColorImage,
    /// ≤ 1024 px (the viewer zoomed out: no mipmaps, so a smaller copy keeps text smooth).
    pub fit: egui::ColorImage,
    /// 512 px.
    pub thumb: egui::ColorImage,
    /// ≤ 2048 px, without lettering (the minimap).
    pub mini: egui::ColorImage,
}

/// Uploaded egui textures for the current map.
pub struct MapTextures {
    pub overview: egui::TextureHandle,
    pub fit: egui::TextureHandle,
    pub thumb: egui::TextureHandle,
    pub mini: egui::TextureHandle,
    /// Full-resolution 2048 px tiles for close zoom in the viewer, made on demand.
    pub tiles: Vec<Option<egui::TextureHandle>>,
}

struct Job {
    progress: Arc<Progress>,
    handle: std::thread::JoinHandle<Result<(MapResult, PreviewImages), String>>,
    started: Instant,
    /// A map for the walk HUD only (not shown in the Map tab).
    minimap_only: bool,
    /// Share of the whole generation already done before this job (the satellite tiles).
    done_before: f32,
}

/// The large zoomable preview over the viewport.
#[derive(Clone, Debug)]
pub struct Viewer {
    pub open: bool,
    /// Screen px per map px.
    pub zoom: f32,
    /// Map px at the viewer's centre.
    pub centre: Vec2,
    pub fit: bool,
}

impl Default for Viewer {
    fn default() -> Self {
        Self {
            open: false,
            zoom: 1.0,
            centre: Vec2::ZERO,
            fit: true,
        }
    }
}

#[derive(Resource, Default)]
pub struct WorldMap {
    pub settings: MapSettings,
    job: Option<Job>,
    capture: Option<satellite::Capture>,
    pub result: Option<Arc<MapResult>>,
    images: Option<PreviewImages>,
    pub textures: Option<MapTextures>,
    /// A minimal map made automatically for the walk-mode minimap when none was generated.
    minimap: Option<AutoMinimap>,
    /// The world the job running now was started from.
    job_stamp: Stamp,
    pub viewer: Viewer,
    /// Last message for the panel (errors, export results).
    pub status: Option<String>,
    /// Export here when the generation running now finishes.
    pub export_after: Option<PathBuf>,
    /// The UI asked to export into a folder (generating first if needed).
    pub export_request: Option<PathBuf>,
    export_job: Option<(
        PathBuf,
        std::thread::JoinHandle<Result<export::Exported, String>>,
    )>,
    pub last_export: Option<PathBuf>,
    /// The UI asked for a generation (started by the map system next frame).
    pub generate_requested: bool,
    /// The world changed since the map was made.
    stamp: Stamp,
    pub stale: bool,
    /// Where an agent's last `map_generate` / `map_status` saved the full-size PNG (per map).
    mcp_saved: Option<(usize, PathBuf)>,
    /// A bare satellite capture being written, and the last one finished.
    shot_job: Option<std::thread::JoinHandle<Result<satellite::ShotResult, String>>>,
    pub shot: Option<satellite::ShotResult>,
    /// The last thing started was a bare capture (what `map_status` reports).
    pub(crate) last_was_shot: bool,
}

impl WorldMap {
    pub fn busy(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !j.minimap_only)
            || self.capture.is_some()
            || self.shot_job.is_some()
    }
    pub fn exporting(&self) -> bool {
        self.export_job.is_some()
    }
    /// The satellite capture is borrowing the main camera.
    pub fn capturing(&self) -> bool {
        self.capture.is_some()
    }
    /// Progress (0..1) and stage of the generation running now.
    pub fn progress(&self) -> Option<(f32, String)> {
        if let Some(c) = &self.capture {
            return Some(c.progress());
        }
        let j = self.job.as_ref().filter(|j| !j.minimap_only)?;
        let (f, s) = j.progress.get();
        Some((
            j.done_before + f * (1.0 - j.done_before),
            format!("{s} · {:.0} s", j.started.elapsed().as_secs_f32()),
        ))
    }
    pub fn cancel(&mut self) {
        if let Some(j) = &self.job {
            j.progress.cancel();
        }
        if let Some(c) = &mut self.capture {
            c.cancel();
        }
        self.export_after = None;
    }
    /// What the world file should store while a satellite capture has the lighting and detail
    /// temporarily changed (an autosave mid-capture keeps the crafter's own settings).
    pub fn lighting_to_save(&self, current: &crate::sky::Lighting) -> crate::sky::Lighting {
        self.capture
            .as_ref()
            .map(|c| c.saved_lighting().clone())
            .unwrap_or_else(|| current.clone())
    }
    pub fn graphics_to_save(
        &self,
        current: &crate::streaming::Graphics,
    ) -> crate::streaming::Graphics {
        self.capture
            .as_ref()
            .map(|c| c.saved_graphics().clone())
            .unwrap_or_else(|| current.clone())
    }

    /// Uploads the preview pictures of a new map (call from the UI with its egui context).
    pub fn ensure_textures(&mut self, ctx: &egui::Context) {
        if let Some(img) = self.images.take() {
            let opts = egui::TextureOptions::LINEAR;
            let tiles = self
                .result
                .as_ref()
                .map(|r| ((r.size / 2048).max(1) as usize).pow(2))
                .unwrap_or(1);
            self.textures = Some(MapTextures {
                overview: ctx.load_texture("world_map_overview", img.overview, opts),
                fit: ctx.load_texture("world_map_fit", img.fit, opts),
                thumb: ctx.load_texture("world_map_thumb", img.thumb, opts),
                mini: ctx.load_texture("world_map_mini", img.mini, opts),
                tiles: vec![None; tiles],
            });
        }
        if let Some((_, img, tex, _)) = &mut self.minimap
            && let Some(img) = img.take()
        {
            *tex = Some(ctx.load_texture("world_minimap_auto", img, egui::TextureOptions::LINEAR));
        }
    }

    /// Full-resolution tile `i` (row-major, 2048 px) of the current map, uploaded on first use.
    pub fn tile_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureId> {
        let r = self.result.clone()?;
        let tex = self.textures.as_mut()?;
        if tex.tiles.len() <= 1 {
            return Some(tex.overview.id());
        }
        if let Some(Some(t)) = tex.tiles.get(i) {
            return Some(t.id());
        }
        let per = (r.size / 2048) as usize;
        let (tx, ty) = (i % per, i / per);
        let mut px = Vec::with_capacity(2048 * 2048 * 4);
        for row in 0..2048 {
            let start = ((ty * 2048 + row) * r.size as usize + tx * 2048) * 4;
            px.extend_from_slice(&r.rgba[start..start + 2048 * 4]);
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([2048, 2048], &px);
        let h = ctx.load_texture(
            format!("world_map_tile_{i}"),
            img,
            egui::TextureOptions::LINEAR,
        );
        let id = h.id();
        if let Some(slot) = tex.tiles.get_mut(i) {
            *slot = Some(h);
        }
        Some(id)
    }

    /// The map the walk-mode minimap draws from: the generated map, else an automatic one.
    pub fn minimap_source(&self) -> Option<minimap::Source> {
        if let (Some(r), Some(t)) = (&self.result, &self.textures) {
            return Some(minimap::Source {
                texture: t.mini.id(),
                world_min: r.world_min,
                world_size: r.world_size,
            });
        }
        let (r, _, tex, _) = self.minimap.as_ref()?;
        Some(minimap::Source {
            texture: tex.as_ref()?.id(),
            world_min: r.world_min,
            world_size: r.world_size,
        })
    }
}

impl WorldMap {
    fn spawn_export(&mut self, dir: PathBuf, r: Arc<MapResult>) {
        let d = dir.clone();
        let h = std::thread::Builder::new()
            .name("world-map-export".into())
            .spawn(move || export::export(&d, &r))
            .expect("spawn export thread");
        self.status = Some(format!("Exporting to {}…", dir.display()));
        self.export_job = Some((dir, h));
    }
}

/// Revisions of what a map shows (terrain, zones, objects, water).
type Stamp = (u64, u64, u64, u64);

/// The automatic walk-mode minimap: the map, its picture until uploaded, the texture, and the
/// world it was made from.
type AutoMinimap = (
    Arc<MapResult>,
    Option<egui::ColorImage>,
    Option<egui::TextureHandle>,
    Stamp,
);

pub struct WorldMapPlugin;

impl Plugin for WorldMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldMap>()
            .add_systems(Update, drive.after(crate::camera::CameraSystems))
            .add_systems(Update, debug_hooks.after(crate::tools::ToolSystems::Commit));
        mcp::register(app);
    }
}

fn world_stamp(world: &World) -> Stamp {
    let t = world.resource::<crate::terrain::Terrain>();
    let w = world.resource::<crate::water::WaterBodies>();
    // (procedural nature: its paint revision and the biome map count as object changes)
    let nature = world
        .get_resource::<crate::nature::NatureField>()
        .map_or(0, |f| f.revision)
        + t.biome_revision;
    (
        t.height_revision,
        t.zone_revision,
        world.resource::<crate::objects::ObjectStore>().revision + nature,
        (w.rivers.len() + w.lakes.len() * 1000) as u64,
    )
}

/// Starts a bare top-down capture of a world rectangle (see `satellite::Shot`).
pub fn start_shot(world: &mut World, shot: satellite::Shot) -> Result<(), String> {
    if world.resource::<WorldMap>().busy() {
        return Err("a map is being generated: poll map_status".into());
    }
    if world.resource::<crate::walk::WalkState>().active {
        return Err("leave walk mode first".into());
    }
    let [x0, z0, x1, z1] = shot.rect;
    if !(x1 > x0 && z1 > z0)
        || !(16..=8192).contains(&shot.width)
        || !(16..=8192).contains(&shot.height)
    {
        return Err(
            "rect must be [min_x, min_z, max_x, max_z] with max > min, and the size 16–8192 px"
                .into(),
        );
    }
    let mut map = world.resource_mut::<WorldMap>();
    map.last_was_shot = true;
    map.shot = None;
    map.status = None;
    satellite::begin_shot(world, shot);
    Ok(())
}

/// Starts generating with the current settings (a satellite map renders its tiles first).
pub fn start(world: &mut World) {
    let settings = world.resource::<WorldMap>().settings.sanitized();
    if world.resource::<WorldMap>().busy() {
        return;
    }
    if world.resource::<crate::walk::WalkState>().active {
        world.resource_mut::<WorldMap>().status =
            Some("Leave walk mode to generate the map.".into());
        return;
    }
    world.resource_mut::<WorldMap>().last_was_shot = false;
    if settings.style == MapStyle::Satellite {
        satellite::begin(world, settings);
    } else {
        spawn_job(world, settings, None, false);
    }
}

fn spawn_job(
    world: &mut World,
    settings: MapSettings,
    satellite: Option<Vec<u8>>,
    minimap_only: bool,
) {
    let input = snapshot::take(world);
    let stamp = world_stamp(world);
    let done_before = if satellite.is_some() { 0.7 } else { 0.0 };
    let progress = Arc::new(Progress::default());
    let p = progress.clone();
    let handle = std::thread::Builder::new()
        .name("world-map".into())
        .spawn(move || {
            let r = render::render(&input, &settings, satellite.as_deref(), &p)?;
            let images = preview_images(&r);
            Ok((r, images))
        })
        .expect("spawn map thread");
    let mut map = world.resource_mut::<WorldMap>();
    if !minimap_only {
        map.stamp = stamp;
        map.status = None;
    }
    map.job_stamp = stamp;
    map.job = Some(Job {
        progress,
        handle,
        started: Instant::now(),
        minimap_only,
        done_before,
    });
}

fn preview_images(r: &MapResult) -> PreviewImages {
    let (ov, os) = export::downscaled(r, 2048);
    let mut fit = ov.clone();
    let mut fs = os;
    while fs > 1024 {
        fit = export::halve(&fit, fs);
        fs /= 2;
    }
    let mut thumb = fit.clone();
    let mut ts = fs;
    while ts > 512 {
        thumb = export::halve(&thumb, ts);
        ts /= 2;
    }
    let mut mini = r.clean.clone();
    let mut ms = r.size;
    while ms > 2048 {
        mini = export::halve(&mini, ms);
        ms /= 2;
    }
    PreviewImages {
        overview: egui::ColorImage::from_rgba_unmultiplied([os as usize; 2], &ov),
        fit: egui::ColorImage::from_rgba_unmultiplied([fs as usize; 2], &fit),
        thumb: egui::ColorImage::from_rgba_unmultiplied([ts as usize; 2], &thumb),
        mini: egui::ColorImage::from_rgba_unmultiplied([ms as usize; 2], &mini),
    }
}

/// Generates synchronously (agents, the export hook). Satellite maps can't: they need frames.
pub fn generate_now(world: &mut World, settings: &MapSettings) -> Result<Arc<MapResult>, String> {
    if settings.style == MapStyle::Satellite {
        return Err("satellite maps render over several frames: start one with map_generate and poll map_status".into());
    }
    let settings = &settings.sanitized();
    let input = snapshot::take(world);
    let stamp = world_stamp(world);
    let r = render::render(&input, settings, None, &Progress::default())?;
    let images = preview_images(&r);
    let r = Arc::new(r);
    let mut map = world.resource_mut::<WorldMap>();
    map.result = Some(r.clone());
    map.images = Some(images);
    map.textures = None;
    map.stamp = stamp;
    map.stale = false;
    Ok(r)
}

/// The generic export's map (`<export>/map/`): the current map if it is up to date, else one
/// generated now with the saved settings (parchment when they ask for satellite, which needs
/// frames to render). A failure is logged and leaves the rest of the export alone.
pub fn export_hook(dir: &std::path::Path, data: &crate::io::WorldData) {
    if let Err(e) = export_for_world(dir, data) {
        warn!("world map export: {e}");
    }
}

fn export_for_world(dir: &std::path::Path, data: &crate::io::WorldData) -> Result<(), String> {
    let map = &*data.map;
    if let Some(r) = &map.result
        && !map.stale
    {
        return export::export(dir, r).map(|_| ());
    }
    let mut settings = map.settings.sanitized();
    if settings.style == MapStyle::Satellite {
        settings.style = MapStyle::Parchment;
    }
    let input = snapshot::from_parts(snapshot::Parts {
        terrain: &data.terrain,
        water: &data.water,
        zones: &data.zones,
        store: &data.store,
        lib: data.lib.as_deref(),
        gameplay: &data.gameplay,
        roads: &data.roads,
        name: &data.meta.name,
        nature: crate::nature::data_ctx(data),
    });
    let r = render::render(&input, &settings, None, &Progress::default())?;
    export::export(dir, &r).map(|_| ())
}

/// Polls the generation, drives the satellite capture, keeps the minimap ready.
fn drive(world: &mut World) {
    if world.resource::<WorldMap>().generate_requested {
        world.resource_mut::<WorldMap>().generate_requested = false;
        start(world);
    }
    satellite::step(world);
    // a finished generation
    let done = world
        .resource::<WorldMap>()
        .job
        .as_ref()
        .is_some_and(|j| j.handle.is_finished());
    if done {
        let job = world.resource_mut::<WorldMap>().job.take().unwrap();
        let elapsed = job.started.elapsed();
        match job.handle.join() {
            Ok(Ok((r, images))) => {
                info!(
                    "MAPLOG generated {} {}px in {:?} ({:?})",
                    r.settings.style.id(),
                    r.size,
                    elapsed,
                    r.timings
                );
                let r = Arc::new(r);
                let mut map = world.resource_mut::<WorldMap>();
                if job.minimap_only {
                    let stamp = map.job_stamp;
                    map.minimap = Some((r, Some(images.mini), None, stamp));
                } else {
                    map.result = Some(r.clone());
                    map.images = Some(images);
                    map.textures = None;
                    map.stale = false;
                    map.status = Some(format!(
                        "{} map, {} px, made in {:.1} s",
                        r.settings.style.name(),
                        r.size,
                        elapsed.as_secs_f32()
                    ));
                    if let Some(dir) = map.export_after.take() {
                        map.spawn_export(dir, r);
                    }
                }
            }
            Ok(Err(e)) => {
                if e != "cancelled" {
                    warn!("world map: {e}");
                }
                world.resource_mut::<WorldMap>().status = Some(if e == "cancelled" {
                    "Cancelled.".into()
                } else {
                    format!("Map failed: {e}")
                });
            }
            Err(_) => {
                world.resource_mut::<WorldMap>().status = Some("Map generation crashed.".into())
            }
        }
    }
    // a bare capture written
    let written = world
        .resource::<WorldMap>()
        .shot_job
        .as_ref()
        .is_some_and(|j| j.is_finished());
    if written {
        let mut map = world.resource_mut::<WorldMap>();
        let h = map.shot_job.take().unwrap();
        match h.join() {
            Ok(Ok(r)) => {
                info!(
                    "MAPLOG capture {}×{} px saved to {} in {} ms",
                    r.shot.width,
                    r.shot.height,
                    r.shot.path.display(),
                    r.millis
                );
                map.status = Some(format!(
                    "Satellite capture saved to {}",
                    r.shot.path.display()
                ));
                map.shot = Some(r);
            }
            Ok(Err(e)) => map.status = Some(format!("Capture failed: {e}")),
            Err(_) => map.status = Some("Capture crashed.".into()),
        }
    }
    // exports
    let request = world.resource_mut::<WorldMap>().export_request.take();
    if let Some(dir) = request {
        let map = world.resource::<WorldMap>();
        match (
            &map.result,
            map.stale
                || map
                    .result
                    .as_ref()
                    .is_some_and(|r| r.settings != map.settings),
        ) {
            (Some(r), false) => {
                let r = r.clone();
                world.resource_mut::<WorldMap>().spawn_export(dir, r);
            }
            _ if !map.busy() => {
                world.resource_mut::<WorldMap>().export_after = Some(dir);
                start(world);
            }
            _ => world.resource_mut::<WorldMap>().export_after = Some(dir),
        }
    }
    let finished = world
        .resource::<WorldMap>()
        .export_job
        .as_ref()
        .is_some_and(|j| j.1.is_finished());
    if finished {
        let mut map = world.resource_mut::<WorldMap>();
        let (dir, h) = map.export_job.take().unwrap();
        match h.join() {
            Ok(Ok(e)) => {
                info!(
                    "MAPLOG exported {} files ({} bytes) to {}",
                    e.files,
                    e.bytes,
                    dir.display()
                );
                map.status = Some(format!(
                    "Exported {} files ({:.1} MB) to {}",
                    e.files,
                    e.bytes as f64 / 1e6,
                    dir.display()
                ));
                map.last_export = Some(dir);
            }
            Ok(Err(e)) => map.status = Some(format!("Export failed: {e}")),
            Err(_) => map.status = Some("Export crashed.".into()),
        }
    }
    // stale marker
    let stamp = world_stamp(world);
    let mut map = world.resource_mut::<WorldMap>();
    if map.result.is_some() && !map.busy() && stamp != map.stamp {
        map.stale = true;
    }
    // walking without a map: make a quick minimal one for the minimap
    let walking = world.resource::<crate::walk::WalkState>().active;
    let map = world.resource::<WorldMap>();
    let outdated = map.minimap.as_ref().is_none_or(|m| m.3 != stamp);
    // (an underground dungeon draws its own: `dungeons::survey::hud`)
    let underground = world
        .get_resource::<crate::dungeons::DungeonMap>()
        .is_some_and(|d| d.interior());
    if walking
        && !underground
        && map.result.is_none()
        && outdated
        && map.job.is_none()
        && map.capture.is_none()
    {
        let settings = MapSettings {
            style: MapStyle::Minimal,
            size: 2048,
            layers: MapLayers {
                labels: false,
                pois: false,
                grid: false,
                compass: false,
                ..default()
            },
            show_title: false,
            ..default()
        };
        spawn_job(world, settings, None, true);
    }
}

/// `WB_MAP`, `WB_MAP_OUT`, `WB_MAP_DEMO`, `WB_MAP_VIEW` (see the module docs). `WB_MAP` takes
/// several styles joined by `+` (each exported into `WB_MAP_OUT/<style>`), `WB_MAP_LAYERS` a
/// `+`-joined layer list, `WB_MAP_TITLE` / `WB_MAP_SEA` the title and sea name.
fn debug_hooks(
    world: &mut World,
    mut frame: Local<u32>,
    mut queue: Local<Option<Vec<MapStyle>>>,
    mut logged: Local<Option<String>>,
) {
    *frame += 1;
    let f = *frame;
    if f == 330 && std::env::var("WB_MAP_DEMO").is_ok() {
        demo_content(world);
    }
    if let Ok(spec) = std::env::var("WB_MAP_SHOT") {
        let v: Vec<f32> = spec
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        let out = PathBuf::from(std::env::var("WB_MAP_OUT").unwrap_or_else(|_| ".".into()));
        if f == 340 && v.len() >= 5 {
            let rect = [v[0], v[1], v[2], v[3]];
            let w = v[4] as u32;
            let h = v
                .get(5)
                .map(|h| *h as u32)
                .unwrap_or(((w as f32) * (v[3] - v[1]) / (v[2] - v[0])).round() as u32);
            let shot = satellite::Shot {
                rect,
                width: w,
                height: h,
                path: out.join("satellite_capture.png"),
            };
            info!("MAPLOG shot {}", shot.transform());
            if let Err(e) = start_shot(world, shot) {
                warn!("MAPLOG shot failed: {e}");
            }
        }
        let map = world.resource::<WorldMap>();
        if f > 340 && !map.busy() && std::env::var("WB_MAP_STAY").is_err() {
            world.write_message(AppExit::Success);
        }
        return;
    }
    let Ok(spec) = std::env::var("WB_MAP") else {
        return;
    };
    if f < 340 {
        return;
    }
    let (styles, size) = spec.split_once(',').unwrap_or((spec.as_str(), "2048"));
    let queue =
        queue.get_or_insert_with(|| styles.split('+').filter_map(MapStyle::from_id).collect());
    let size = size.trim().parse().unwrap_or(2048);
    let out = std::env::var("WB_MAP_OUT").ok().map(PathBuf::from);
    let map = world.resource::<WorldMap>();
    let idle = !map.busy()
        && map.export_after.is_none()
        && !map.exporting()
        && map.export_request.is_none();
    if !idle {
        return;
    }
    if let Some(s) = &map.status
        && logged.as_ref() != Some(s)
    {
        info!("MAPLOG {s}");
        *logged = Some(s.clone());
    }
    if queue.is_empty() {
        if out.is_some() && std::env::var("WB_MAP_STAY").is_err() {
            world.write_message(AppExit::Success);
        }
        return;
    }
    let style = queue.remove(0);
    let mut map = world.resource_mut::<WorldMap>();
    map.settings.style = style;
    map.settings.size = size;
    if let Ok(v) = std::env::var("WB_MAP_LAYERS") {
        let on = |k: &str| v.split('+').any(|x| x == k);
        map.settings.layers = MapLayers {
            labels: on("labels"),
            zones: on("zones"),
            roads: on("roads"),
            pois: on("pois"),
            grid: on("grid"),
            compass: on("compass"),
        };
    }
    if let Ok(t) = std::env::var("WB_MAP_TITLE") {
        map.settings.title = t;
    }
    map.settings.sea_name =
        std::env::var("WB_MAP_SEA").unwrap_or_else(|_| "The Sundering Sea".into());
    map.export_after = out.map(|d| d.join(style.id()));
    if std::env::var("WB_MAP_VIEW").is_ok() {
        map.viewer.open = true;
    }
    map.status = None;
    info!("MAPLOG start {} {}", style.id(), size);
    start(world);
}

/// Sample gameplay content for testing maps of the demo world: a player start, towns, a dungeon
/// and other places, roads between them, and names for the demo's river (gameplay and roads are
/// built by other tools; a fresh world has none).
fn demo_content(world: &mut World) {
    use crate::gameplay::{Gameplay, Poi, PoiKind};
    use crate::roads::{Road, RoadKind, Roads};
    let (half, sea) = {
        let t = world.resource::<crate::terrain::Terrain>();
        (t.half(), t.sea_level)
    };
    let ground = |world: &World, p: Vec2| {
        world
            .resource::<crate::terrain::Terrain>()
            .height_at(p.x, p.y)
    };
    // find dry spots near the wanted places
    let near_land = |world: &World, p: Vec2| -> Vec2 {
        let mut best = p;
        'search: for r in 0..40 {
            for k in 0..16 {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                let q = p + Vec2::new(a.cos(), a.sin()) * r as f32 * 12.0;
                let h = ground(world, q);
                if h > sea + 3.0
                    && world
                        .resource::<crate::water::WaterBodies>()
                        .water_level(Vec3::new(q.x, h, q.y))
                        .is_none()
                {
                    best = q;
                    break 'search;
                }
            }
        }
        best
    };
    let spots: [(&str, PoiKind, f32, f32); 12] = [
        ("Emberfall", PoiKind::Town, -0.2, 0.22),
        ("Highgarde", PoiKind::Town, 0.18, -0.12),
        ("Saltmere", PoiKind::Town, 0.42, 0.24),
        ("The Hollow Crypt", PoiKind::Dungeon, 0.02, -0.42),
        ("Warden's Rest", PoiKind::Camp, -0.38, 0.05),
        ("Starfall Crater", PoiKind::Landmark, -0.06, 0.34),
        ("Old Lighthouse", PoiKind::Vista, 0.55, -0.05),
        ("Gryphon Roost", PoiKind::FlightPoint, 0.05, 0.05),
        ("Shrine of Tides", PoiKind::Shrine, 0.3, 0.42),
        ("Barrow Field", PoiKind::Graveyard, -0.28, -0.18),
        ("Rift of Ash", PoiKind::Portal, 0.32, -0.34),
        ("The Drowned Hoard", PoiKind::Treasure, -0.45, 0.32),
    ];
    let mut pois = Vec::new();
    for (name, kind, x, z) in spots {
        let p = near_land(world, Vec2::new(x, z) * half);
        pois.push((name, kind, p));
    }
    {
        let positions: Vec<(String, PoiKind, Vec3)> = pois
            .iter()
            .map(|(n, k, p)| (n.to_string(), *k, Vec3::new(p.x, ground(world, *p), p.y)))
            .collect();
        let mut gp = world.resource_mut::<Gameplay>();
        for (name, kind, p) in positions {
            let id = gp.alloc_id();
            gp.pois.push(Poi {
                id,
                kind,
                level: None,
                name,
                pos: p.to_array(),
                radius: 40.0,
                notes: String::new(),
                tags: vec![],
            });
        }
        if let Some(first) = gp.pois.first() {
            let p = first.pos;
            gp.player_start = Some(crate::gameplay::PlayerStart {
                pos: [p[0] + 30.0, p[1], p[2] + 25.0],
                yaw: 0.0,
            });
        }
        gp.dirty = true;
    }
    // roads: a highway between the towns, dirt roads and trails to the rest
    let link = |world: &World, a: Vec2, b: Vec2, bend: f32| -> Vec<[f32; 3]> {
        let n = 7;
        (0..=n)
            .map(|k| {
                let t = k as f32 / n as f32;
                let side =
                    (b - a).perp().normalize_or_zero() * (t * std::f32::consts::PI).sin() * bend;
                let wob = (b - a).perp().normalize_or_zero()
                    * (t * 9.0).sin()
                    * 18.0
                    * (t * std::f32::consts::PI).sin();
                let q = near_land(world, a.lerp(b, t) + side + wob);
                [q.x, ground(world, q), q.y]
            })
            .collect()
    };
    let p = |name: &str| {
        pois.iter()
            .find(|x| x.0 == name)
            .map(|x| x.2)
            .unwrap_or(Vec2::ZERO)
    };
    let roads: Vec<(&str, RoadKind, Vec<[f32; 3]>)> = vec![
        (
            "King's Road",
            RoadKind::PavedStone,
            link(world, p("Emberfall"), p("Highgarde"), 120.0),
        ),
        (
            "Coast Road",
            RoadKind::Cobblestone,
            link(world, p("Highgarde"), p("Saltmere"), -90.0),
        ),
        (
            "Miller's Lane",
            RoadKind::DirtRoad,
            link(world, p("Emberfall"), p("Warden's Rest"), 60.0),
        ),
        (
            "Crypt Path",
            RoadKind::Trail,
            link(world, p("Highgarde"), p("The Hollow Crypt"), 80.0),
        ),
        (
            "Crater Walk",
            RoadKind::Footpath,
            link(world, p("Emberfall"), p("Starfall Crater"), -40.0),
        ),
        (
            "Lighthouse Track",
            RoadKind::Gravel,
            link(world, p("Saltmere"), p("Old Lighthouse"), 50.0),
        ),
    ];
    {
        let mut rd = world.resource_mut::<Roads>();
        for (name, kind, points) in roads {
            let id = rd.alloc_id();
            rd.roads.push(Road {
                authored: None,
                options: Default::default(),
                cleared: vec![],
                prop_ids: vec![],
                id,
                name: name.into(),
                kind,
                points,
                width: kind.default_width(),
            });
        }
        rd.dirty = true;
    }
    let mut water = world.resource_mut::<crate::water::WaterBodies>();
    if let Some(r) = water.rivers.first_mut() {
        r.name = "River Lune".into();
    }
    if let Some(l) = water.lakes.first_mut() {
        l.name = "Mirrormere".into();
    }
    info!("MAPLOG demo content: {} places, 6 roads", pois.len());
}
