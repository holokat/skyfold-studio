//! Image to world ("Reference map", Land tab › Trace a Picture): load a map picture (concept art,
//! a sketch, a real map), lay it over the world like tracing paper, let the editor read it
//! (sea, lakes and rivers; land regions with a suggested biome and height role; legends and
//! lettering ignored), adjust the regions, and build a playable first draft in one step: the
//! coastline, a plausible heightfield, biomes, lakes, rivers and (optionally) zones.
//!
//! - `analyse`: reading the picture (pure image processing on a worker thread).
//! - `relief`: the draft's heights from the analysis (pure).
//! - `build`: applying a draft to the world as one undo step.
//! - `paint`: the terrain overlay (picture or analysis) and the MCP preview image.
//! - `ui`: the Land tab panel, region list and viewport tools; `mcp`: the agent tools.
//!
//! The picture, its placement, the analysis settings, ignore areas and region edits are saved
//! with the world (`WorldDoc::reference`; the picture is copied into the world folder). Edits are
//! stored by picture position so they survive re-analysis. Placement, ignore areas and region
//! edits are undoable.
//!
//! Test hooks: `WB_REF=<picture>` loads a picture once the demo world is built (fitted, Land tab
//! › Trace a Picture open, analysis view); `WB_REF_VIEW=picture` shows the picture instead;
//! `WB_REF_TOOL=place|ignore|split` picks the viewport tool (drive it with `WB_STROKE`).

pub mod analyse;
mod build;
pub(crate) mod mcp;
mod paint;
pub mod relief;
pub mod ui;

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, LandTool, Tab};
use crate::history::History;
use crate::terrain::Terrain;
pub use relief::{Placement, ReliefParams};

/// Owner name on the shared terrain overlay.
const OVERLAY: &str = "reference";
/// Undo key for reference edits.
const UNDO_KEY: &str = "reference";

/// What the terrain overlay shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RefView {
    #[default]
    Picture,
    /// The analysis: regions, water, rivers, ignored areas.
    Analysis,
}

/// How a draft is built (saved with the world).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildSettings {
    /// 0: the open world; else a new world of this size (m).
    pub world_size: f32,
    /// Sea kept clear around the picture when fitting (share of the world size per side).
    pub margin: f32,
    pub relief: ReliefParams,
    /// Also create a zone per region (zones are the crafter's own: off unless asked).
    pub zones: bool,
    /// Grow the biomes' vegetation.
    pub vegetation: bool,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            world_size: 0.0,
            margin: 0.03,
            relief: ReliefParams::default(),
            zones: false,
            vegetation: true,
        }
    }
}

/// The reference picture and everything done with it (saved with the world).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RefDoc {
    /// The picture: a file name inside the world folder once the world is saved, else a path.
    pub image: String,
    /// Where it was loaded from.
    pub source: String,
    pub placement: Placement,
    pub opacity: f32,
    pub visible: bool,
    pub view: RefView,
    pub settings: analyse::Settings,
    /// Ignore areas (polygons in picture uv, 0..1 from the top-left).
    pub ignore: Vec<Vec<[f32; 2]>>,
    /// Region edits, replayed after every analysis.
    pub ops: Vec<analyse::Op>,
    pub build: BuildSettings,
}

impl Default for RefDoc {
    fn default() -> Self {
        Self {
            image: String::new(),
            source: String::new(),
            placement: Placement::default(),
            opacity: 0.75,
            visible: true,
            view: RefView::Picture,
            settings: analyse::Settings::default(),
            ignore: vec![],
            ops: vec![],
            build: BuildSettings::default(),
        }
    }
}

/// A decoded picture.
pub struct Picture {
    pub w: usize,
    pub h: usize,
    /// RGB8, row-major from the top.
    pub rgb: Vec<u8>,
    /// At most 2048 px on its longer side, for the overlay.
    pub small: image::RgbImage,
    pub path: PathBuf,
}

impl Picture {
    /// Height / width.
    pub fn aspect(&self) -> f32 {
        self.h as f32 / self.w as f32
    }
}

/// Viewport tools of the reference map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RefTool {
    /// Drag to move; corner handles scale, the knob rotates. A click selects a region.
    #[default]
    Place,
    /// Lasso areas of the picture that aren't map (⌥-click one to remove it).
    Ignore,
    /// Draw a line across a region to cut it in two.
    Split,
}

/// What an undo step of the reference map restores.
#[derive(Clone)]
pub struct Snapshot {
    doc: RefDoc,
    picture: Option<Arc<Picture>>,
}

/// Something the panel asked for that runs in its own system (file dialog, build).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefRequest {
    Load,
    Remove,
    Build,
}

/// An analysis on a worker thread: its inputs' key and the result slot.
struct Job {
    key: u64,
    base_key: u64,
    slot: Arc<Mutex<Option<(Arc<analyse::Base>, Arc<analyse::Analysis>)>>>,
}

/// The reference map's state.
#[derive(Resource, Default)]
pub struct RefMap {
    pub doc: RefDoc,
    pub picture: Option<Arc<Picture>>,
    pub base: Option<Arc<analyse::Base>>,
    pub analysis: Option<Arc<analyse::Analysis>>,
    /// Inputs the base / the analysis were made from.
    base_key: u64,
    an_key: u64,
    job: Option<Job>,
    /// Bumped whenever the analysis changes (overlay, labels).
    pub revision: u64,
    /// Region index selected in the list.
    pub selected: Option<usize>,
    pub tool: RefTool,
    /// Something to tell the person (load errors…).
    pub status: Option<String>,
    overlay: Option<Handle<Image>>,
    overlay_key: u64,
    /// The picture over open water (the terrain overlay lies on the seabed, under the water).
    plane: Option<(Entity, Handle<StandardMaterial>, Handle<Image>)>,
    plane_key: u64,
    /// The doc before a drag started (one undo step per drag).
    pub drag_before: Option<RefDoc>,
    pub request: Option<RefRequest>,
    /// File › New › From a picture's style draft, consumed only when it creates a new world.
    pub new_world_art_style: Option<crate::art_style::ArtStyle>,
    /// Staged by File › New from picture and applied only after its replacement world exists.
    pub new_world_appearance: Option<crate::nature::NatureAppearance>,
}

impl RefMap {
    pub fn has_picture(&self) -> bool {
        self.picture.is_some()
    }
    pub fn aspect(&self) -> f32 {
        self.picture.as_ref().map(|p| p.aspect()).unwrap_or(1.0)
    }
    /// Forget the picture and everything read from it.
    pub fn clear(&mut self) {
        *self = RefMap {
            overlay: self.overlay.take(),
            plane: self.plane.take(),
            ..default()
        };
    }
    fn base_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.picture.as_ref().map(|p| p.path.clone()).hash(&mut h);
        serde_json::to_string(&self.doc.settings)
            .unwrap_or_default()
            .hash(&mut h);
        serde_json::to_string(&self.doc.ignore)
            .unwrap_or_default()
            .hash(&mut h);
        h.finish()
    }
    fn an_key(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.base_key().hash(&mut h);
        serde_json::to_string(&self.doc.ops)
            .unwrap_or_default()
            .hash(&mut h);
        h.finish()
    }
    /// Whether the analysis matches the current picture, settings, ignore areas and edits.
    pub fn analysis_current(&self) -> bool {
        self.analysis.is_some() && self.an_key == self.an_key()
    }
    pub fn analysing(&self) -> bool {
        self.job.is_some()
    }

    /// Loads a picture (PNG / JPG) and fits it to a world of `world_size` m.
    pub fn load(&mut self, path: &Path, world_size: f32) -> Result<(), String> {
        let pic = decode(path)?;
        let aspect = pic.aspect();
        let keep_build = self.doc.build.clone();
        self.clear();
        self.doc.image = path.to_string_lossy().into_owned();
        self.doc.source = self.doc.image.clone();
        self.doc.placement = Placement::fit(world_size, aspect, keep_build.margin);
        self.doc.build = keep_build;
        self.picture = Some(Arc::new(pic));
        Ok(())
    }

    /// Runs the analysis now (blocking) if it isn't current.
    pub fn analyse_now(&mut self) -> Result<Arc<analyse::Analysis>, String> {
        let Some(pic) = self.picture.clone() else {
            return Err("no reference picture: load one first (reference_load)".into());
        };
        if self.analysis_current()
            && let Some(a) = &self.analysis
        {
            return Ok(a.clone());
        }
        // a worker already on it: wait for its result
        if let Some(job) = self.job.take() {
            if job.key == self.an_key() {
                loop {
                    if let Some((base, an)) =
                        job.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
                    {
                        self.set_result(job.base_key, job.key, base, an.clone());
                        return Ok(an);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        let bk = self.base_key();
        let base = match &self.base {
            Some(b) if self.base_key == bk => b.clone(),
            _ => Arc::new(analyse::analyse(
                &pic.rgb,
                pic.w,
                pic.h,
                &self.doc.settings,
                &self.doc.ignore,
            )),
        };
        let an = Arc::new(analyse::finish(&base, &self.doc.ops));
        self.set_result(bk, self.an_key(), base, an.clone());
        Ok(an)
    }

    fn set_result(
        &mut self,
        base_key: u64,
        an_key: u64,
        base: Arc<analyse::Base>,
        an: Arc<analyse::Analysis>,
    ) {
        let n = an.regions.len();
        self.base = Some(base);
        self.analysis = Some(an);
        self.base_key = base_key;
        self.an_key = an_key;
        self.revision += 1;
        if self.selected.is_some_and(|s| s >= n) {
            self.selected = None;
        }
    }

    /// Starts a background analysis when the inputs changed (and none is running for them).
    fn kick(&mut self) {
        let Some(pic) = self.picture.clone() else {
            return;
        };
        let key = self.an_key();
        if self.an_key == key && self.analysis.is_some() {
            return;
        }
        // one at a time: a running job's result is picked up first, then the latest inputs run
        if self.job.is_some() {
            return;
        }
        let bk = self.base_key();
        let reuse = self.base.clone().filter(|_| self.base_key == bk);
        let (settings, ignore, ops) = (
            self.doc.settings.clone(),
            self.doc.ignore.clone(),
            self.doc.ops.clone(),
        );
        let slot: Arc<Mutex<Option<(Arc<analyse::Base>, Arc<analyse::Analysis>)>>> = Arc::default();
        let out = slot.clone();
        std::thread::spawn(move || {
            let t0 = std::time::Instant::now();
            let base = reuse.unwrap_or_else(|| {
                Arc::new(analyse::analyse(&pic.rgb, pic.w, pic.h, &settings, &ignore))
            });
            let an = Arc::new(analyse::finish(&base, &ops));
            info!(
                "REFMAP analysed {}×{} picture: {} regions, {} lakes, {} rivers in {:.0} ms (reading {:.0} ms)",
                pic.w,
                pic.h,
                an.regions.len(),
                an.lakes.len(),
                an.rivers.len(),
                t0.elapsed().as_secs_f32() * 1000.0,
                base.millis
            );
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some((base, an));
        });
        self.job = Some(Job {
            key,
            base_key: bk,
            slot,
        });
    }

    /// The state an undo step restores.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            doc: self.doc.clone(),
            picture: self.picture.clone(),
        }
    }

    /// Records the change from `before` to now as one undo step.
    pub fn record(&self, history: &mut History, label: &str, before: Snapshot) {
        history.push_ext(label, UNDO_KEY, Arc::new(before), Arc::new(self.snapshot()));
    }

    /// Makes an undoable change to the doc (`f` returns false when nothing changed).
    pub fn edit(
        &mut self,
        history: &mut History,
        label: &str,
        f: impl FnOnce(&mut RefDoc) -> bool,
    ) -> bool {
        let before = self.snapshot();
        if f(&mut self.doc) && self.doc != before.doc {
            self.record(history, label, before);
            true
        } else {
            false
        }
    }

    /// Records a change already made to the doc (a drag that ended) as one undo step.
    pub fn commit_drag(&mut self, history: &mut History, label: &str) {
        if let Some(before) = self.drag_before.take()
            && before != self.doc
        {
            self.record(
                history,
                label,
                Snapshot {
                    doc: before,
                    picture: self.picture.clone(),
                },
            );
        }
    }

    /// A region edit by picture position, recorded as one undo step.
    pub fn push_op(&mut self, history: &mut History, label: &str, op: analyse::Op) {
        self.edit(history, label, |d| {
            d.ops.push(op);
            true
        });
    }

    /// The vegetation density read from the picture, placed on the world.
    pub fn density(&self) -> Option<DensityMap> {
        let base = self.base.as_ref()?;
        Some(DensityMap {
            w: base.w,
            h: base.h,
            values: base.density.clone(),
            placement: self.doc.placement,
            aspect: self.aspect(),
        })
    }
}

/// Vegetation density read from the reference picture, as a raster on the picture's grid with
/// its placement on the world (for the procedural nature system's density maps).
#[derive(Clone)]
pub struct DensityMap {
    pub w: usize,
    pub h: usize,
    /// 0 (bare) … 1 (dense canopy), row-major from the picture's top-left.
    pub values: Vec<f32>,
    pub placement: Placement,
    /// Picture height / width.
    pub aspect: f32,
}

impl DensityMap {
    /// Density at a world position (0 outside the picture), bilinear.
    pub fn at(&self, x: f32, z: f32) -> f32 {
        let uv = self.placement.to_uv([x, z], self.aspect);
        if !(0.0..1.0).contains(&uv[0]) || !(0.0..1.0).contains(&uv[1]) {
            return 0.0;
        }
        let fx = (uv[0] * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (uv[1] * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let v = |x: usize, y: usize| self.values[y * self.w + x];
        let a = v(x0, y0) + (v(x1, y0) - v(x0, y0)) * tx;
        let b = v(x0, y1) + (v(x1, y1) - v(x0, y1)) * tx;
        a + (b - a) * ty
    }
}

/// The vegetation density from the open world's reference picture, if it has one.
pub fn image_density(world: &World) -> Option<DensityMap> {
    world.get_resource::<RefMap>()?.density()
}

/// `TemperateForest` → `temperate_forest` (the MCP tools' biome ids).
pub fn biome_id(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn decode(path: &Path) -> Result<Picture, String> {
    let img = image::open(path)
        .map_err(|e| format!("couldn't read {}: {e}", path.display()))?
        .to_rgb8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 16 || h < 16 {
        return Err("the picture is too small".into());
    }
    let k = (2048.0 / w.max(h) as f32).min(1.0);
    let small = if k < 1.0 {
        image::imageops::resize(
            &img,
            ((w as f32 * k) as u32).max(1),
            ((h as f32 * k) as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img.clone()
    };
    Ok(Picture {
        w,
        h,
        rgb: img.into_raw(),
        small,
        path: path.to_path_buf(),
    })
}

// ------------------------------------------------------------------ saving

/// The doc to save into the world folder `dir`: the picture is copied in (once) so the world
/// travels with it.
pub(crate) fn doc_to_save(r: &RefMap, dir: &Path) -> RefDoc {
    let mut doc = r.doc.clone();
    let Some(pic) = &r.picture else {
        return RefDoc {
            image: String::new(),
            ..doc
        };
    };
    let ext = pic
        .path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let name = format!("reference.{ext}");
    let dest = dir.join(&name);
    let same = std::fs::canonicalize(&pic.path).ok() == std::fs::canonicalize(&dest).ok();
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    if !same && size(&dest) != size(&pic.path) {
        // (a new world's folder doesn't exist yet when its first save is snapshotted)
        let _ = std::fs::create_dir_all(dir);
        if let Err(e) = std::fs::copy(&pic.path, &dest) {
            warn!("couldn't copy the reference picture into the world: {e}");
            return doc;
        }
    }
    doc.image = name;
    doc
}

/// Restores the reference saved with a world loaded from `dir`.
pub(crate) fn load_doc(r: &mut RefMap, doc: RefDoc, dir: &Path) {
    r.clear();
    if doc.image.is_empty() {
        return;
    }
    let p = PathBuf::from(&doc.image);
    let path = if p.is_absolute() { p } else { dir.join(p) };
    match decode(&path) {
        Ok(pic) => {
            r.picture = Some(Arc::new(pic));
            r.doc = doc;
        }
        Err(e) => {
            warn!("reference picture: {e}");
            r.status = Some(format!("The reference picture is missing: {e}"));
        }
    }
}

// ------------------------------------------------------------------ plugin

pub struct RefMapPlugin;

impl Plugin for RefMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RefMap>()
            .add_systems(
                Update,
                (
                    apply_undo,
                    drive,
                    test_hooks.after(crate::debug::demo_content),
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    overlay.before(crate::terrain::TerrainSystems::Gpu),
                    sea_plane,
                ),
            );
        ui::build(app);
        mcp::register(app);
    }
}

/// Whether the reference map is in use on screen (its panel is open).
fn panel_open(ed: &Editor) -> bool {
    ed.tab == Tab::Land && ed.land_tool == LandTool::Reference
}

/// Keeps the analysis current while the panel is open (on a worker thread) and picks up results.
fn drive(mut r: ResMut<RefMap>, ed: Res<Editor>) {
    if let Some(job) = &r.job {
        let done = job.slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((base, an)) = done {
            let (bk, key) = (job.base_key, job.key);
            r.job = None;
            r.set_result(bk, key, base, an);
        }
    }
    if r.has_picture() && (panel_open(&ed) || r.doc.view == RefView::Analysis && r.doc.visible) {
        r.kick();
    }
}

/// Undo / redo of reference edits: restore the doc the history hands back.
fn apply_undo(mut history: ResMut<History>, mut r: ResMut<RefMap>) {
    if !history.ext_apply.iter().any(|(k, _)| *k == UNDO_KEY) {
        return;
    }
    let mut keep = vec![];
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        if key != UNDO_KEY {
            keep.push((key, snap));
            continue;
        }
        if let Some(s) = snap.downcast_ref::<Snapshot>() {
            let same = match (&r.picture, &s.picture) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            };
            if !same {
                // another picture: forget what was read from this one
                let doc = s.doc.clone();
                r.clear();
                r.picture = s.picture.clone();
                r.doc = doc;
            } else {
                r.doc = s.doc.clone();
            }
        }
    }
    history.ext_apply = keep;
}

/// Shows the picture (or the analysis) on the terrain while it's relevant: the shaping tabs.
#[allow(clippy::too_many_arguments)]
fn overlay(
    mut r: ResMut<RefMap>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<Terrain>,
    mut ov: ResMut<crate::terrain::overlay::TerrainOverlay>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time<Real>>,
    mut last_paint: Local<f32>,
) {
    let tab_ok = matches!(
        ed.tab,
        Tab::Land | Tab::Sculpt | Tab::Paint | Tab::Water | Tab::Zones
    );
    let show = r.has_picture()
        && r.doc.visible
        && tab_ok
        && !walk.active
        && !walk.cinematic
        && (r.doc.view == RefView::Picture || r.analysis.is_some());
    if !show {
        if ov.is_shown_by(OVERLAY) {
            ov.clear(OVERLAY);
        }
        return;
    }
    let res = ((terrain.size() / 4.0) as u32).clamp(512, 2048);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    res.hash(&mut h);
    terrain.size().to_bits().hash(&mut h);
    r.doc.view.hash(&mut h);
    for v in [
        r.doc.placement.center[0],
        r.doc.placement.center[1],
        r.doc.placement.width,
        r.doc.placement.rotation,
    ] {
        v.to_bits().hash(&mut h);
    }
    if r.doc.view == RefView::Analysis {
        r.revision.hash(&mut h);
        r.selected.hash(&mut h);
        serde_json::to_string(&r.doc.ignore)
            .unwrap_or_default()
            .hash(&mut h);
    }
    let key = h.finish();
    // repaint at most ~12 times a second while dragging
    let now = time.elapsed_secs();
    if key != r.overlay_key && (now - *last_paint > 0.08 || r.overlay.is_none()) {
        *last_paint = now;
        let data = match r.doc.view {
            RefView::Picture => r
                .picture
                .as_ref()
                .map(|p| paint::picture_overlay(p, &r.doc.placement, terrain.size(), res)),
            RefView::Analysis => match (&r.analysis, &r.base, &r.picture) {
                (Some(a), Some(b), Some(p)) => Some(paint::analysis_overlay(
                    a,
                    &b.ignored,
                    &r.doc.placement,
                    p.aspect(),
                    r.selected,
                    terrain.size(),
                    res,
                )),
                _ => None,
            },
        };
        if let Some(data) = data {
            let reuse = r
                .overlay
                .as_ref()
                .and_then(|h| images.get(h))
                .is_some_and(|img| img.width() == res);
            if reuse && let Some(mut img) = r.overlay.as_ref().and_then(|h| images.get_mut(h)) {
                img.data = Some(data);
            } else {
                let mut img = crate::terrain::overlay::new_overlay_image(res, res);
                img.data = Some(data);
                r.overlay = Some(images.add(img));
            }
            r.overlay_key = key;
        }
    }
    if let Some(img) = r.overlay.clone()
        && (!ov.is_shown_by(OVERLAY)
            || ov.image.as_ref() != Some(&img)
            || (ov.opacity - r.doc.opacity).abs() > 1e-3)
    {
        ov.show(OVERLAY, img, r.doc.opacity.max(0.02));
    }
}

/// The picture (or analysis) as an unlit sheet just above the sea, so it shows over open water
/// too; land hides it and shows the terrain overlay instead.
#[allow(clippy::too_many_arguments)]
fn sea_plane(
    mut commands: Commands,
    mut r: ResMut<RefMap>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<Terrain>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&mut Transform, &mut Visibility)>,
) {
    let tab_ok = matches!(
        ed.tab,
        Tab::Land | Tab::Sculpt | Tab::Paint | Tab::Water | Tab::Zones
    );
    let show = r.has_picture()
        && r.doc.visible
        && tab_ok
        && !walk.active
        && !walk.cinematic
        && (r.doc.view == RefView::Picture || r.analysis.is_some());
    if !show {
        if let Some((e, _, _)) = &r.plane
            && let Ok((_, mut v)) = q.get_mut(*e)
        {
            *v = Visibility::Hidden;
        }
        return;
    }
    // the texture: the picture, or the analysis coloured like the terrain overlay
    let key = match r.doc.view {
        RefView::Picture => 1,
        RefView::Analysis => {
            2 + r.revision * 4 + r.selected.map(|s| s as u64 + 1).unwrap_or(0) * 1_000_003
        }
    };
    if r.plane.is_none() || key != r.plane_key {
        let img = match r.doc.view {
            RefView::Picture => r.picture.as_ref().map(|p| paint::picture_image(p)),
            RefView::Analysis => match (&r.analysis, &r.base) {
                (Some(a), Some(b)) => Some(paint::analysis_image(a, &b.ignored, r.selected)),
                _ => None,
            },
        };
        let Some(img) = img else { return };
        match &r.plane {
            Some((_, _, tex)) => {
                if let Some(mut t) = images.get_mut(tex) {
                    *t = img;
                }
            }
            None => {
                let tex = images.add(img);
                let mat = mats.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 1.0, 1.0, r.doc.opacity),
                    base_color_texture: Some(tex.clone()),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    ..default()
                });
                let e = commands
                    .spawn((
                        Mesh3d(meshes.add(unit_sheet())),
                        MeshMaterial3d(mat.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                        Name::new("Reference picture (sea)"),
                    ))
                    .id();
                r.plane = Some((e, mat, tex));
            }
        }
        r.plane_key = key;
    }
    let Some((e, mat, _)) = r.plane.clone() else {
        return;
    };
    if mats
        .get(&mat)
        .is_some_and(|m| (m.base_color.alpha() - r.doc.opacity).abs() > 1e-3)
        && let Some(mut m) = mats.get_mut(&mat)
    {
        m.base_color.set_alpha(r.doc.opacity);
    }
    if let Ok((mut t, mut v)) = q.get_mut(e) {
        let p = r.doc.placement;
        *t = Transform::from_xyz(p.center[0], terrain.sea_level + 0.6, p.center[1])
            .with_rotation(Quat::from_rotation_y(-p.rotation.to_radians()))
            .with_scale(Vec3::new(p.width, 1.0, p.width * r.aspect()));
        *v = Visibility::Visible;
    }
}

/// A 1 × 1 sheet in the xz plane, uv (0, 0) at its −x / −z (north-west) corner.
fn unit_sheet() -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, 0.0, -0.5],
            [0.5, 0.0, -0.5],
            [0.5, 0.0, 0.5],
            [-0.5, 0.0, 0.5],
        ],
    );
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 4]);
    m.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    );
    m.insert_indices(Indices::U32(vec![0, 3, 2, 0, 2, 1]));
    m
}

/// `WB_REF=<picture>`: load it once the demo world exists, open the panel (see the module docs).
fn test_hooks(
    mut done: Local<bool>,
    mut r: ResMut<RefMap>,
    mut ed: ResMut<Editor>,
    terrain: Res<Terrain>,
    frames: Res<bevy::diagnostic::FrameCount>,
) {
    if *done || frames.0 < 200 {
        return;
    }
    *done = true;
    let Ok(path) = std::env::var("WB_REF") else {
        return;
    };
    match r.load(Path::new(&path), terrain.size()) {
        Ok(()) => {
            r.doc.view = if std::env::var("WB_REF_VIEW").is_ok_and(|v| v == "picture") {
                RefView::Picture
            } else {
                RefView::Analysis
            };
            ed.tab = Tab::Land;
            ed.land_tool = LandTool::Reference;
            r.tool = match std::env::var("WB_REF_TOOL").as_deref() {
                Ok("ignore") => RefTool::Ignore,
                Ok("split") => RefTool::Split,
                _ => RefTool::Place,
            };
            info!("REFMAP loaded {path}");
        }
        Err(e) => warn!("REFMAP {e}"),
    }
}
