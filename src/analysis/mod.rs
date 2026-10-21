//! World analysis for MMO designers (the Analyze tab): walkability, travel time, content gaps,
//! level flow, reachability, the World Doctor and navmesh export.
//!
//! Everything runs on a ~2 m walkability grid (`grid`). A snapshot of the world is taken on the
//! main thread and analysed on a worker thread (`job`); results show on the ground through the
//! shared `TerrainOverlay` while the tab is open, with markers and hover read-outs (`view`).
//! Results re-compute by themselves when the world changes (only the parts whose inputs changed).
//! Agents drive it through MCP tools (`mcp`); `navmesh::export` / `export_navmesh` serve the
//! exporters.

pub mod doctor;
pub mod flow;
pub mod grid;
mod hooks;
pub mod job;
pub(crate) mod mcp;
pub mod navmesh;
pub mod paint;
pub mod travel;
mod view;

use std::collections::HashSet;
use std::sync::Arc;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Tab};
use crate::gameplay::Gameplay;
use job::{Fps, Inputs, Origin, Parts, Progress};
pub use view::severity_colour;

/// What a walker does on slopes steeper than the walk limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SteepRule {
    /// Classic MMO: slide down, can't walk up.
    #[default]
    SlideDown,
    /// Climb slowly (a third of the speed).
    Climb,
    /// Nobody goes there.
    Block,
}

/// Where travel time is measured from.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub enum TravelSource {
    #[default]
    PlayerStart,
    Poi(u32),
    /// A clicked point (world xz).
    Point([f32; 2]),
}

/// The movement rules and thresholds of a world (saved with it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisSettings {
    /// Grid cell size (m).
    pub cell: f32,
    /// Steepest walkable slope (degrees).
    pub max_walk_slope: f32,
    /// Slopes above this are cliffs nobody passes (degrees).
    pub cliff_slope: f32,
    pub steep: SteepRule,
    /// Deeper water is swum (m).
    pub swim_depth: f32,
    pub allow_swim: bool,
    /// Trees, rocks and buildings block movement.
    pub objects_block: bool,
    /// m/s on flat open ground.
    pub run_speed: f32,
    pub swim_speed: f32,
    /// Travel on a mount where the zone allows mounts.
    pub mounted: bool,
    pub mount_speed: f32,
    pub source: TravelSource,
    /// Seconds from the nearest content after which ground counts as a dead zone.
    pub gap_threshold: f32,
    pub gap_fish: bool,
    pub gap_roads: bool,
    /// Flag neighbouring zones whose average levels differ by more than this.
    pub level_jump: u32,
    /// Objects per 128 m chunk before the doctor calls it a hotspot.
    pub chunk_budget: u32,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            cell: 2.0,
            max_walk_slope: 40.0,
            cliff_slope: 60.0,
            steep: SteepRule::SlideDown,
            swim_depth: 1.2,
            allow_swim: true,
            objects_block: true,
            // the editor's walk mode jogs at 6.5 m/s
            run_speed: 6.5,
            swim_speed: 2.5,
            mounted: false,
            mount_speed: 11.0,
            source: TravelSource::PlayerStart,
            gap_threshold: 45.0,
            gap_fish: true,
            gap_roads: false,
            level_jump: 5,
            chunk_budget: 2500,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum View {
    #[default]
    Walkability,
    Travel,
    Gaps,
    Flow,
    Reach,
    Doctor,
}

impl View {
    pub const ALL: [View; 6] = [
        View::Walkability,
        View::Travel,
        View::Gaps,
        View::Flow,
        View::Reach,
        View::Doctor,
    ];
    pub fn name(self) -> &'static str {
        match self {
            View::Walkability => "Walkability",
            View::Travel => "Travel Time",
            View::Gaps => "Content Gaps",
            View::Flow => "Level Flow",
            View::Reach => "Reachability",
            View::Doctor => "World Doctor",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            View::Walkability => "Walkable",
            View::Travel => "Travel",
            View::Gaps => "Gaps",
            View::Flow => "Flow",
            View::Reach => "Reach",
            View::Doctor => "Doctor",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            View::Walkability => "walk",
            View::Travel => "travel",
            View::Gaps => "gaps",
            View::Flow => "flow",
            View::Reach => "reach",
            View::Doctor => "doctor",
        }
    }
}

/// A job in flight on the worker thread.
pub struct Running {
    handle: std::thread::JoinHandle<job::JobOut>,
    pub progress: Arc<Progress>,
    pub todo: [bool; 6],
    fps: Fps,
}

#[derive(Resource)]
pub struct Analysis {
    pub settings: AnalysisSettings,
    pub view: View,
    pub opacity: f32,
    pub parts: Parts,
    /// Fingerprints of the world as last checked.
    pub fps: Fps,
    /// The doctor runs with the analysis (after "Run checks").
    pub want_doctor: bool,
    /// Issues fixed since the report was made (hidden until the re-run).
    pub fixed: HashSet<u32>,
    /// Where reachability starts without a player start (camera focus when first needed).
    pub fallback_origin: Option<Vec2>,
    /// A list row the cursor is over: ringed on the map.
    pub highlight: Option<Vec3>,
    pub timings: Vec<(&'static str, f32)>,
    /// Set by the UI when settings change (saved with the world, re-analysed).
    pub settings_changed: bool,
    pub running: Option<Running>,
    /// A navmesh export writing files in the background (the target folder, the work).
    pub export: Option<(
        std::path::PathBuf,
        std::thread::JoinHandle<Result<navmesh::NavStats, String>>,
    )>,
    next_check: f32,
    pending: Option<(f32, Fps)>,
    overlay: Option<Handle<Image>>,
    painted: Option<(View, [Option<u64>; 4], u64)>,
}

impl Default for Analysis {
    fn default() -> Self {
        Self {
            settings: AnalysisSettings::default(),
            view: View::Walkability,
            opacity: 1.0,
            parts: Parts::default(),
            fps: Fps::default(),
            want_doctor: false,
            fixed: HashSet::new(),
            fallback_origin: None,
            highlight: None,
            timings: Vec::new(),
            settings_changed: false,
            running: None,
            export: None,
            next_check: 0.0,
            pending: None,
            overlay: None,
            painted: None,
        }
    }
}

impl Analysis {
    /// A world was loaded or created: take its settings, forget old results.
    pub fn load_settings(&mut self, s: AnalysisSettings) {
        if let Some(r) = &self.running {
            r.progress
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        *self = Analysis {
            settings: s,
            view: self.view,
            opacity: self.opacity,
            overlay: self.overlay.take(),
            running: self.running.take(),
            ..default()
        };
    }
    /// Overall progress (0..1) and stage of the running job.
    pub fn progress(&self) -> Option<(f32, &'static str)> {
        self.running.as_ref().map(|r| r.progress.overall(&r.todo))
    }
    /// (reachability origin, travel source).
    pub fn origins(&self, gp: &Gameplay) -> (Option<Origin>, Option<Origin>) {
        let origin = gp
            .player_start
            .map(|s| Origin {
                pos: Vec2::new(s.pos[0], s.pos[2]),
                label: "the player start".into(),
            })
            .or_else(|| {
                self.fallback_origin.map(|p| Origin {
                    pos: p,
                    label: "the chosen start point".into(),
                })
            });
        let source = match self.settings.source {
            TravelSource::PlayerStart => origin.clone(),
            TravelSource::Poi(id) => gp
                .pois
                .iter()
                .find(|p| p.id == id)
                .map(|p| Origin {
                    pos: Vec2::new(p.pos[0], p.pos[2]),
                    label: p.name.clone(),
                })
                .or(origin.clone()),
            TravelSource::Point(p) => Some(Origin {
                pos: Vec2::from_array(p),
                label: "the clicked point".into(),
            }),
        };
        (origin, source)
    }
    pub fn grid(&self) -> Option<&grid::WalkGrid> {
        self.parts.grid.as_ref().map(|g| &*g.1)
    }
    pub fn report(&self) -> Option<&doctor::Report> {
        self.parts.doctor.as_ref().map(|d| &*d.1)
    }
}

pub struct AnalysisPlugin;

impl Plugin for AnalysisPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Analysis>()
            .add_systems(
                Update,
                (
                    pick_point.after(crate::tools::ToolSystems::Input),
                    drive,
                    overlay,
                )
                    .chain(),
            )
            .add_systems(bevy_egui::EguiPrimaryContextPass, view::viewport);
        mcp::register(app);
        hooks::register(app);
    }
}

/// World resources the analysis reads (shared by the scheduler and the MCP tools).
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldRefs<'w> {
    pub terrain: Res<'w, crate::terrain::Terrain>,
    pub water: Res<'w, crate::water::WaterBodies>,
    pub store: Res<'w, crate::objects::ObjectStore>,
    pub lib: Option<Res<'w, crate::objects::Library>>,
    pub roads: Res<'w, crate::roads::Roads>,
    pub gameplay: Res<'w, Gameplay>,
    pub zones: Res<'w, crate::world::Zones>,
    pub nature: crate::nature::NatureParams<'w>,
}

impl WorldRefs<'_> {
    pub fn inputs(&self) -> Inputs<'_> {
        Inputs {
            terrain: &self.terrain,
            water: &self.water,
            store: &self.store,
            lib: self.lib.as_deref(),
            roads: &self.roads,
            gameplay: &self.gameplay,
            zones: &self.zones,
            nature: self.nature.ctx(),
        }
    }
}

/// Polls the worker and starts a new job when the world (or a setting) changed, once things
/// have been still for a moment. Only while the Analyze tab is open.
fn drive(
    mut a: ResMut<Analysis>,
    mut ed: ResMut<Editor>,
    w: WorldRefs,
    orbit: Res<crate::camera::OrbitCamera>,
    stroke: Res<crate::tools::Stroke>,
    time: Res<Time<Real>>,
    mut meta: ResMut<crate::world::WorldMeta>,
) {
    if a.running.as_ref().is_some_and(|r| r.handle.is_finished()) {
        let r = a.running.take().unwrap();
        let cancelled = r.progress.cancel.load(std::sync::atomic::Ordering::Relaxed);
        if let Ok(out) = r.handle.join() {
            if cancelled {
                // keep a finished grid, drop the rest
                if out.parts.grid.as_ref().is_some_and(|g| g.0 == a.fps.grid) {
                    a.parts.grid = out.parts.grid;
                }
            } else {
                if out.parts.doctor.as_ref().map(|d| d.0) != a.parts.doctor.as_ref().map(|d| d.0) {
                    a.fixed.clear();
                }
                a.parts = out.parts;
                let t: Vec<String> = out
                    .timings
                    .iter()
                    .map(|(n, ms)| format!("{n} {ms:.0} ms"))
                    .collect();
                info!(
                    "ANALYSIS done: {} (grid {}²)",
                    t.join(", "),
                    a.grid().map(|g| g.n).unwrap_or(0)
                );
                a.timings = out.timings;
            }
        }
    }
    if a.export.as_ref().is_some_and(|e| e.1.is_finished()) {
        let (dir, h) = a.export.take().unwrap();
        match h.join() {
            Ok(Ok(st)) => ed.toast(format!(
                "Navmesh: {} polygons in {} connected part{} → {}",
                st.polygons,
                st.components,
                if st.components == 1 { "" } else { "s" },
                dir.display()
            )),
            Ok(Err(e)) => ed.toast(format!("Navmesh export failed: {e}")),
            Err(_) => ed.toast("Navmesh export failed"),
        }
    }
    if a.settings_changed {
        a.settings_changed = false;
        meta.dirty = true;
        a.next_check = 0.0;
    }
    if ed.tab != Tab::Analyze {
        return;
    }
    let now = time.elapsed_secs();
    if now < a.next_check {
        return;
    }
    a.next_check = now + 0.25;
    if w.gameplay.player_start.is_none() && a.fallback_origin.is_none() {
        a.fallback_origin = Some(orbit.focus.xz());
    }
    let (origin, source) = a.origins(&w.gameplay);
    let inputs = w.inputs();
    let fps = job::fingerprints(&inputs, &a.settings, &origin, &source);
    a.fps = fps;
    let stale = a.parts.stale(&fps, a.want_doctor);
    if !stale.iter().any(|s| *s) {
        a.pending = None;
        return;
    }
    if let Some(r) = &a.running {
        // inputs moved on while it works: stop it early
        if r.fps != fps {
            r.progress
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        return;
    }
    if stroke.active {
        return;
    }
    // wait until the inputs have been still for a moment (slider drags, brush strokes)
    let first = a.parts.grid.is_none();
    match a.pending {
        Some((t, p)) if p == fps => {
            if now - t < 0.35 && !first {
                return;
            }
        }
        _ => {
            a.pending = Some((now, fps));
            if !first {
                return;
            }
        }
    }
    a.pending = None;
    let t = std::time::Instant::now();
    let snap = inputs.snapshot(&a.settings, origin, source, stale[5]);
    let snap_ms = t.elapsed().as_secs_f32() * 1000.0;
    let prev = a.parts.clone();
    let progress = Arc::new(Progress::default());
    let p2 = progress.clone();
    let doctor = a.want_doctor;
    let handle = std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || job::compute(&snap, &prev, fps, doctor, &p2));
    match handle {
        Ok(handle) => {
            info!(
                "ANALYSIS started (snapshot {snap_ms:.1} ms on the main thread), stale {stale:?}"
            );
            a.running = Some(Running {
                handle,
                progress,
                todo: stale,
                fps,
            });
        }
        Err(e) => error!("ANALYSIS couldn't start a worker thread: {e}"),
    }
}

/// Clicking the ground sets the travel start (Travel view, "click" source) or the reachability
/// start (Reach view, worlds without a player start).
fn pick_point(
    stroke: Res<crate::tools::Stroke>,
    ed: Res<Editor>,
    mut a: ResMut<Analysis>,
    gameplay: Res<Gameplay>,
) {
    if ed.tab != Tab::Analyze || !stroke.just_started {
        return;
    }
    let Some(hit) = stroke.hit else { return };
    match a.view {
        View::Travel if matches!(a.settings.source, TravelSource::Point(_)) => {
            a.settings.source = TravelSource::Point([hit.x, hit.z]);
            a.settings_changed = true;
        }
        View::Reach if gameplay.player_start.is_none() => {
            a.fallback_origin = Some(hit.xz());
            a.next_check = 0.0;
        }
        _ => {}
    }
}

/// Paints the current view into the overlay image and shows it on the terrain while the tab is
/// open.
fn overlay(
    mut a: ResMut<Analysis>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    mut ov: ResMut<crate::terrain::overlay::TerrainOverlay>,
    mut images: ResMut<Assets<Image>>,
    zones: Res<crate::world::Zones>,
) {
    if ed.tab != Tab::Analyze || walk.active {
        if ov.is_shown_by("analyze") {
            ov.clear("analyze");
        }
        return;
    }
    let p = &a.parts;
    let g = p.grid.as_ref().map(|x| x.0);
    let key_parts = match a.view {
        View::Walkability | View::Doctor => [g, None, None, None],
        View::Travel => [g, p.travel.as_ref().map(|x| x.0), None, None],
        View::Gaps => [
            g,
            p.gapfield.as_ref().map(|x| x.0),
            p.gaps.as_ref().map(|x| x.0),
            p.reach.as_ref().map(|x| x.0),
        ],
        View::Flow => [g, p.flow.as_ref().map(|x| x.0), None, None],
        View::Reach => [g, p.reach.as_ref().map(|x| x.0), None, None],
    };
    let mut colours = [[0u8; 3]; 256];
    let mut zh = 0u64;
    for z in &zones.list {
        let c = [
            (z.color[0] * 255.0) as u8,
            (z.color[1] * 255.0) as u8,
            (z.color[2] * 255.0) as u8,
        ];
        colours[z.id as usize] = c;
        zh = zh
            .wrapping_mul(31)
            .wrapping_add(u64::from_le_bytes([c[0], c[1], c[2], z.id, 0, 0, 0, 0]));
    }
    let key = (a.view, key_parts, if a.view == View::Flow { zh } else { 0 });
    let t = std::time::Instant::now();
    if a.painted != Some(key)
        && let Some((n, data)) = paint::paint(a.view, &a.parts, &colours)
    {
        debug!(
            "ANALYSIS painted {:?} ({n}²) in {:.1} ms",
            a.view,
            t.elapsed().as_secs_f32() * 1000.0
        );
        let same_size = a
            .overlay
            .as_ref()
            .and_then(|h| images.get(h))
            .is_some_and(|img| img.width() as usize == n);
        if same_size && let Some(mut img) = a.overlay.as_ref().and_then(|h| images.get_mut(h)) {
            img.data = Some(data);
        } else {
            let mut img = crate::terrain::overlay::new_overlay_image(n as u32, n as u32);
            img.data = Some(data);
            a.overlay = Some(images.add(img));
        }
        a.painted = Some(key);
    }
    match (&a.overlay, a.painted.is_some_and(|k| k.0 == a.view)) {
        (Some(h), true) => {
            if !ov.is_shown_by("analyze")
                || ov.image.as_ref() != Some(h)
                || (ov.opacity - a.opacity).abs() > 1e-3
            {
                ov.show("analyze", h.clone(), a.opacity);
            }
        }
        _ => {
            if ov.is_shown_by("analyze") {
                ov.clear("analyze");
            }
        }
    }
}

/// Builds the walkability grid now (blocking) and writes the navmesh files into `dir`: for the
/// exporters. Returns the stats.
pub fn export_navmesh(
    world: &mut World,
    dir: &std::path::Path,
) -> Result<navmesh::NavStats, String> {
    mcp::ensure(world, false)?;
    let a = world.resource::<Analysis>();
    let g = a
        .parts
        .grid
        .as_ref()
        .ok_or("no walkability grid")?
        .1
        .clone();
    let speed = a.settings.run_speed;
    navmesh::export(&g, speed, dir)
}
