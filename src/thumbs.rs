//! Rendered pictures of every library asset, fish species and ore deposit: thumbnails for the
//! asset grids and a live turntable preview of the asset (or fish) being placed.
//!
//! Thumbnails are shot off the critical path: each subject is staged on its own render layer far
//! below the world with a small camera (`studio.rs`: soft studio lights, a transparent background,
//! framing fitted to its bounds), a few at a time, and read back once two shots agree (so slow
//! pipelines and textures never end up in a picture). Each one is cached on disk (`cache.rs`) and
//! loaded only once its tile scrolls into view. Tiles on screen are shot first (three at a time);
//! the rest of the library, and models imported later, follow in the background one at a time.

use std::collections::HashMap;

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::system::SystemParam;
use bevy::light::{EnvironmentMapLight, NotShadowCaster};
use bevy::prelude::*;
use bevy::render::gpu_readback::{ReadbackComplete, ReadbackOnce};
use bevy::tasks::{IoTaskPool, Task, block_on, futures_lite::future};
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};
use serde_json::json;

use crate::fish::{FishMaterial, FishModels, FishSpecies};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::objects::Library;

mod cache;
mod preview;
mod studio;
pub mod ui;

/// Something to picture.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Subject {
    /// a library asset by id (procedural, ez-tree, photoscan or glTF model)
    Asset(String),
    Fish(FishSpecies),
    /// a deposit of an ore (`gameplay::deposits`)
    Ore(crate::gameplay::Ore),
    /// a saved scene (`crate::scenes`) by id: its pieces together
    Scene(String),
}

impl Subject {
    pub fn key(&self) -> String {
        match self {
            Subject::Asset(id) => format!("asset:{id}"),
            Subject::Fish(f) => format!("fish:{}", f.id()),
            Subject::Ore(o) => format!("ore:{}", o.id()),
            Subject::Scene(id) => format!("scene:{id}"),
        }
    }
    /// Thumbnail size (px): fish are long, so theirs are wide.
    pub fn size(&self) -> UVec2 {
        // twice a tile's size on a high-DPI screen (rows of 192 RGBA texels need no padding to
        // be read back)
        match self {
            Subject::Fish(_) => UVec2::new(192, 96),
            Subject::Asset(_) | Subject::Ore(_) => UVec2::splat(192),
            // scenes are wide (and their cover picture on the marketplace)
            Subject::Scene(_) => UVec2::new(320, 192),
        }
    }
    fn fish(&self) -> Option<FishSpecies> {
        match self {
            Subject::Fish(f) => Some(*f),
            Subject::Asset(_) | Subject::Ore(_) | Subject::Scene(_) => None,
        }
    }
}

enum State {
    /// its cache key isn't known yet (its meshes are still loading)
    Queued,
    /// cached on disk: loaded once it's on screen
    OnDisk(u64),
    Loading(u64, Task<Option<Vec<u8>>>),
    /// not cached: to be shot
    Render(u64),
    Shooting,
    Done(u64, Handle<Image>),
    Failed(u64),
}

impl State {
    fn stamp(&self) -> Option<u64> {
        match self {
            State::OnDisk(s)
            | State::Loading(s, _)
            | State::Render(s)
            | State::Done(s, _)
            | State::Failed(s) => Some(*s),
            State::Queued | State::Shooting => None,
        }
    }
}

struct Thumb {
    state: State,
    egui: Option<egui::TextureId>,
    /// on screen this frame: loaded and shot first
    wanted: bool,
}

/// Thumbnails and the turntable, for the UI (`UiState::previews`).
#[derive(Resource, Default)]
pub struct Previews {
    thumbs: HashMap<Subject, Thumb>,
    /// background order (library, then fish)
    order: Vec<Subject>,
    jobs: Vec<Job>,
    readbacks: HashMap<u64, Vec<u8>>,
    next_job: u64,
    /// pictures replaced by newer ones, to drop from egui
    stale: Vec<Handle<Image>>,
    /// asked for over MCP: treated as on screen until done
    pinned: Vec<Subject>,
    turntable: preview::Turntable,
}

impl Previews {
    /// The subject's thumbnail (`None` while it's being made). Call it for tiles on screen only:
    /// they are loaded and shot first.
    pub fn thumb(&mut self, s: &Subject) -> Option<egui::TextureId> {
        let t = self.thumbs.entry(s.clone()).or_insert_with(|| Thumb {
            state: State::Queued,
            egui: None,
            wanted: false,
        });
        t.wanted = true;
        t.egui
    }
    /// Whether the subject's thumbnail is still to come (a spinner beats an empty tile).
    pub fn pending(&self, s: &Subject) -> bool {
        self.thumbs
            .get(s)
            .is_none_or(|t| !matches!(t.state, State::Done(..) | State::Failed(_)))
    }
    /// The live turntable picture of `s` (`None` until its first frames are in). It turns only
    /// while asked for every frame.
    pub fn turntable(&mut self, s: &Subject) -> Option<egui::TextureId> {
        self.turntable.ask(s)
    }
}

/// One subject being shot.
struct Job {
    id: u64,
    subject: Subject,
    stamp: u64,
    root: Entity,
    scenes: Vec<Handle<WorldAsset>>,
    skin: Option<Handle<Image>>,
    camera: Option<Entity>,
    target: Handle<Image>,
    origin: Vec3,
    frames: u32,
    /// frame of the next readback, and the previous shot's (coverage, brightness)
    next_shot: u32,
    reading: bool,
    last: Option<(f32, f32)>,
    shots: u32,
}

/// Shots at once for tiles on screen (one at a time in the background): each is a camera, and
/// every camera costs a visibility pass over the world.
const MAX_JOBS: usize = 3;
/// Disk reads at once.
const MAX_LOADS: usize = 6;
/// Cache keys worked out per frame for subjects not on screen (a key reads the mesh bounds).
const STAMPS_PER_FRAME: usize = 12;

pub struct ThumbsPlugin;

impl Plugin for ThumbsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Previews>()
            .init_resource::<studio::Studio>()
            .add_systems(
                Update,
                (
                    queue_library,
                    check_cache,
                    shoot,
                    preview::turntable,
                    publish,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                studio::skip_studio_cascades
                    .after(bevy::light::SimulationLightSystems::UpdateDirectionalLightCascades)
                    .before(bevy::light::SimulationLightSystems::UpdateLightFrusta),
            );
        mcp(app);
    }
}

/// The thumbnails' MCP tool.
pub fn mcp(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "objects_preview",
        description: "What a library asset, fish species or ore deposit looks like: its studio picture (the thumbnail the Objects tab, the Fish tool and the Ore tool show, 192 px, PNG on a dark background), rendered first if needed. Give `asset` (an objects_library id; assets of asset packs this account hasn't got are refused), `fish` (a species id from water_presets) or `ore` (an id from gameplay_list_ores).",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "asset": { "type": "string", "description": "library asset id" },
                    "fish": { "type": "string", "description": "fish species id or name" },
                    "ore": { "type": "string", "description": "ore id or name" }
                }
            })
        },
        run: preview_tool,
    });
}

/// `objects_preview`: waits for the subject's thumbnail and returns it as a PNG.
fn preview_tool(world: &mut World, args: serde_json::Value) -> Result<Vec<McpContent>, String> {
    let ore = args.get("ore").and_then(|v| v.as_str());
    let s =
        match (
            args.get("asset").and_then(|v| v.as_str()),
            args.get("fish").and_then(|v| v.as_str()),
        ) {
            _ if ore.is_some() => {
                let o = ore.unwrap_or_default();
                Subject::Ore(
                    crate::gameplay::Ore::parse(o)
                        .ok_or_else(|| format!("unknown ore `{o}` (see gameplay_list_ores)"))?,
                )
            }
            (Some(id), _) => {
                if world
                    .get_resource::<Library>()
                    .is_none_or(|l| l.get(id).is_none())
                {
                    return Err(format!(
                        "no asset `{id}` in the library (see objects_library)"
                    ));
                }
                // (assets of packs this account hasn't got aren't shown to agents)
                crate::market::check(id)?;
                Subject::Asset(id.to_string())
            }
            (None, Some(f)) => Subject::Fish(
                FishSpecies::parse(f)
                    .ok_or_else(|| format!("unknown fish species `{f}` (see water_presets)"))?,
            ),
            _ => return Err(
                "give `asset` (an objects_library id), `fish` (a species id) or `ore` (an ore id)"
                    .into(),
            ),
        };
    let mut p = world.resource_mut::<Previews>();
    p.thumb(&s);
    p.pinned.push(s.clone());
    let mut frames = 0;
    Ok(vec![McpContent::deferred(move |w| {
        frames += 1;
        let state = w
            .resource::<Previews>()
            .thumbs
            .get(&s)
            .map(|t| (t.state.stamp(), matches!(t.state, State::Failed(_))));
        match state {
            Some((_, true)) => Some(Err(format!("{} couldn't be pictured", s.key()))),
            // the file is written in the background just after the shot
            Some((Some(stamp), false)) if cache::path(&s, stamp).is_file() => Some(
                cache::flattened_png(&cache::path(&s, stamp), s.size())
                    .map(|png| vec![McpContent::Png(png)]),
            ),
            _ if frames > 1800 => Some(Err(format!("{}: still rendering, try again", s.key()))),
            _ => None,
        }
    })])
}

/// Everything thumbnails and previews need to stage a subject.
#[derive(SystemParam)]
struct Stage<'w, 's> {
    commands: Commands<'w, 's>,
    lib: Option<Res<'w, Library>>,
    server: Res<'w, AssetServer>,
    meshes: Res<'w, Assets<Mesh>>,
    images: ResMut<'w, Assets<Image>>,
    fish: ResMut<'w, FishModels>,
    fish_materials: ResMut<'w, Assets<FishMaterial>>,
    ores: Option<Res<'w, crate::gameplay::deposits::OreAssets>>,
    studio: ResMut<'w, studio::Studio>,
    envs: Query<'w, 's, &'static EnvironmentMapLight>,
    children: Query<'w, 's, &'static Children>,
    placed: Query<'w, 's, (&'static GlobalTransform, Option<&'static Mesh3d>)>,
}

/// A staged subject: its root entity, plus what must load before it can be shot.
struct Staged {
    root: Entity,
    scenes: Vec<Handle<WorldAsset>>,
    skin: Option<Handle<Image>>,
}

/// One library model under `parent` at `at` (on `layers`); returns its glTF scene to wait for.
/// `None`: it has nothing to show.
fn spawn_model(
    commands: &mut Commands,
    server: &AssetServer,
    lib: &Library,
    e: &crate::objects::LibraryEntry,
    parent: Entity,
    at: Transform,
    layers: &RenderLayers,
) -> Option<Option<Handle<WorldAsset>>> {
    // the largest tree variant, as it grows in the world
    let tree = e
        .tree
        .as_ref()
        .and_then(|t| Some((t, t.variants.last()?.lods.first()?.clone())));
    if (e.tree.is_some() && tree.is_none())
        || (e.tree.is_none() && e.meshes.is_empty() && e.import_path.is_none())
    {
        return None;
    }
    let mut node = commands.spawn((at, Visibility::Inherited, layers.clone(), ChildOf(parent)));
    let mut scene = None;
    if let Some((t, (bark, leaves))) = tree {
        node.with_children(|c| {
            c.spawn((
                Mesh3d(bark),
                MeshMaterial3d(t.bark.clone()),
                layers.clone(),
                NotShadowCaster,
            ));
            if let Some(l) = leaves {
                c.spawn((
                    Mesh3d(l),
                    MeshMaterial3d(t.leaves.clone()),
                    layers.clone(),
                    NotShadowCaster,
                ));
            }
        });
    } else if let Some((m, _)) = e.meshes.first() {
        let m = Mesh3d(m.clone());
        node.with_children(|c| {
            if e.sway {
                c.spawn((
                    m,
                    MeshMaterial3d(lib.foliage.clone()),
                    layers.clone(),
                    NotShadowCaster,
                ));
            } else if e.textured {
                c.spawn((
                    m,
                    MeshMaterial3d(lib.surface.clone()),
                    layers.clone(),
                    NotShadowCaster,
                ));
            } else {
                c.spawn((
                    m,
                    MeshMaterial3d(lib.solid.clone()),
                    layers.clone(),
                    NotShadowCaster,
                ));
            }
        });
    } else if let Some(path) = &e.import_path {
        let h = e
            .scene
            .clone()
            .unwrap_or_else(|| server.load(GltfAssetLabel::Scene(0).from_asset(path.clone())));
        node.with_child(WorldAssetRoot(h.clone()));
        scene = Some(h);
    }
    Some(scene)
}

impl Stage<'_, '_> {
    /// Stages `s` at `origin` on `layer`, hidden until it's ready (`ready`). `live`: fish swim.
    fn spawn(&mut self, s: &Subject, origin: Vec3, layer: usize, live: bool) -> Option<Staged> {
        let layers = RenderLayers::layer(layer);
        let at = Transform::from_translation(origin);
        match s {
            Subject::Fish(f) => {
                let model =
                    self.fish
                        .model(*f, &self.server, &mut self.fish_materials, &mut self.images);
                let root = crate::fish::spawn_fish(
                    &mut self.commands,
                    &model,
                    at,
                    crate::fish::swim_tag(0.0, 0.0, if live { 1.0 } else { 0.0 }, live),
                    Some(&layers),
                );
                self.commands.entity(root).insert(Visibility::Hidden);
                Some(Staged {
                    root,
                    scenes: vec![],
                    skin: Some(model.skin),
                })
            }
            Subject::Ore(o) => {
                let root = crate::gameplay::deposits::spawn_preview(
                    &mut self.commands,
                    self.ores.as_deref()?,
                    *o,
                    origin,
                    &layers,
                )?;
                self.commands.entity(root).insert(Visibility::Hidden);
                Some(Staged {
                    root,
                    scenes: vec![],
                    skin: None,
                })
            }
            Subject::Asset(id) => {
                let lib = self.lib.as_deref()?;
                let e = lib.get(id)?;
                let root = self
                    .commands
                    .spawn((at, Visibility::Hidden, layers.clone()))
                    .id();
                match spawn_model(
                    &mut self.commands,
                    &self.server,
                    lib,
                    e,
                    root,
                    Transform::IDENTITY,
                    &layers,
                ) {
                    Some(scene) => Some(Staged {
                        root,
                        scenes: scene.into_iter().collect(),
                        skin: None,
                    }),
                    None => {
                        self.commands.entity(root).despawn();
                        None
                    }
                }
            }
            Subject::Scene(id) => {
                let lib = self.lib.as_deref()?;
                let scene = crate::scenes::file::read(id)?;
                let root = self
                    .commands
                    .spawn((at, Visibility::Hidden, layers.clone()))
                    .id();
                let mut scenes = vec![];
                let mut any = false;
                // (a scene's own models are pictured once it has been placed in this project)
                for pc in scene.pieces.iter().take(1500) {
                    let Some(e) = lib.get(&pc.asset) else {
                        continue;
                    };
                    let s3 = Vec3::from_array(pc.stretch.unwrap_or([1.0; 3])) * pc.scale;
                    let tf = Transform {
                        translation: Vec3::from_array(pc.at),
                        rotation: Quat::from_array(pc.rot).normalize(),
                        scale: s3,
                    };
                    if let Some(h) =
                        spawn_model(&mut self.commands, &self.server, lib, e, root, tf, &layers)
                    {
                        any = true;
                        if let Some(h) = h
                            && !scenes.contains(&h)
                        {
                            scenes.push(h);
                        }
                    }
                }
                if !any {
                    self.commands.entity(root).despawn();
                    return None;
                }
                Some(Staged {
                    root,
                    scenes,
                    skin: None,
                })
            }
        }
    }

    /// The subject's model failed to load.
    fn broken(&self, st: &Staged) -> bool {
        st.scenes
            .iter()
            .any(|h| self.server.load_state(h).is_failed())
    }

    /// Once everything has loaded and spawned: puts the subject's parts on `layer`, shows it and
    /// returns the corners of its parts' bounds relative to the root. `None` while still loading.
    fn ready(&mut self, st: &Staged, layer: usize) -> Option<Vec<Vec3>> {
        if st
            .scenes
            .iter()
            .any(|h| !self.server.is_loaded_with_dependencies(h))
            || st.skin.as_ref().is_some_and(|h| !self.images.contains(h))
        {
            return None;
        }
        let (root_tf, _) = self.placed.get(st.root).ok()?;
        let inv = root_tf.affine().inverse();
        let mut pts = vec![];
        let mut parts = vec![];
        for e in self.children.iter_descendants(st.root) {
            let Ok((tf, mesh)) = self.placed.get(e) else {
                continue;
            };
            parts.push(e);
            let Some(m) = mesh else { continue };
            let (lo, hi) = studio::mesh_bounds(self.meshes.get(&m.0)?)?;
            let to_root = inv * tf.affine();
            pts.extend(
                studio::corners(lo, hi)
                    .iter()
                    .map(|p| to_root.transform_point3(*p)),
            );
        }
        if pts.is_empty() {
            return None;
        }
        let layers = RenderLayers::layer(layer);
        for e in parts {
            self.commands
                .entity(e)
                .insert((layers.clone(), NotShadowCaster));
        }
        self.commands.entity(st.root).insert(Visibility::Inherited);
        Some(pts)
    }
}

/// Lines the whole library up (and the fish after it); a model that changed on disk (re-imported
/// under the same name) is pictured again.
fn queue_library(mut p: ResMut<Previews>, lib: Option<Res<Library>>, meshes: Res<Assets<Mesh>>) {
    let Some(lib) = lib else { return };
    if !lib.is_changed() {
        return;
    }
    let p = &mut *p;
    p.order = lib
        .entries
        .iter()
        .map(|e| Subject::Asset(e.id.clone()))
        .chain(FishSpecies::ALL.map(Subject::Fish))
        .collect();
    for s in &p.order {
        let t = p.thumbs.entry(s.clone()).or_insert_with(|| Thumb {
            state: State::Queued,
            egui: None,
            wanted: false,
        });
        // (only imported models can change while the editor runs; their key is the file's date)
        if matches!(s, Subject::Asset(id) if lib.get(id).is_some_and(|e| e.category == crate::objects::Category::Imported))
            && let Some(old) = t.state.stamp()
            && cache::stamp(s, Some(&lib), &meshes, s.size()).is_some_and(|new| new != old)
        {
            if let State::Done(_, h) = std::mem::replace(&mut t.state, State::Queued) {
                p.stale.push(h);
            }
            t.egui = None;
        }
    }
}

/// Works out cache keys, finds cached pictures, and loads them once they're on screen.
fn check_cache(
    mut p: ResMut<Previews>,
    lib: Option<Res<Library>>,
    meshes: Res<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    let p = &mut *p;
    let mut loading = 0;
    for (s, t) in p.thumbs.iter_mut() {
        if let State::Loading(stamp, task) = &mut t.state {
            match block_on(future::poll_once(task)) {
                Some(Some(rgba)) => {
                    t.state = State::Done(*stamp, images.add(cache::image(s.size(), rgba)))
                }
                Some(None) => t.state = State::Render(*stamp),
                None => loading += 1,
            }
        }
    }
    // keys: everything on screen, and a few more of the rest
    let wanted: Vec<Subject> = p
        .thumbs
        .iter()
        .filter(|(_, t)| t.wanted && matches!(t.state, State::Queued))
        .map(|(s, _)| s.clone())
        .collect();
    let rest: Vec<Subject> = p
        .order
        .iter()
        .filter(|s| {
            p.thumbs
                .get(*s)
                .is_some_and(|t| matches!(t.state, State::Queued))
        })
        .take(STAMPS_PER_FRAME)
        .cloned()
        .collect();
    for s in wanted.into_iter().chain(rest) {
        let Some(stamp) = cache::stamp(&s, lib.as_deref(), &meshes, s.size()) else {
            continue;
        };
        if let Some(t) = p.thumbs.get_mut(&s) {
            t.state = if cache::path(&s, stamp).is_file() {
                State::OnDisk(stamp)
            } else {
                State::Render(stamp)
            };
        }
    }
    for (s, t) in p.thumbs.iter_mut() {
        if loading >= MAX_LOADS {
            break;
        }
        if let (true, State::OnDisk(stamp)) = (t.wanted, &t.state) {
            let (stamp, path, size) = (*stamp, cache::path(s, *stamp), s.size());
            t.state = State::Loading(
                stamp,
                IoTaskPool::get().spawn(async move { cache::load(&path, size) }),
            );
            loading += 1;
        }
    }
}

/// Runs the shoots: stages subjects, places cameras once they're ready, reads back until two
/// shots agree, then keeps (and caches) the picture.
fn shoot(mut p: ResMut<Previews>, mut stage: Stage) {
    let p = &mut *p;
    let busy = !p.jobs.is_empty()
        || p.turntable.active()
        || p.thumbs
            .values()
            .any(|t| matches!(t.state, State::Render(_)));
    studio::rig(
        &mut stage.commands,
        &mut stage.studio,
        &mut stage.images,
        &stage.envs,
        busy,
    );
    // shots wait for the studio's environment so every thumbnail is lit alike
    if stage.studio.env_light.is_none() {
        return;
    }
    // new shoots: tiles on screen first, then the library in order, one at a time
    let render = |s: &Subject| {
        p.thumbs.get(s).and_then(|t| {
            if let State::Render(st) = t.state {
                Some(st)
            } else {
                None
            }
        })
    };
    let mut todo: Vec<(Subject, u64, usize)> = p
        .thumbs
        .iter()
        .filter(|(_, t)| t.wanted)
        .filter_map(|(s, _)| Some((s.clone(), render(s)?, MAX_JOBS)))
        .collect();
    todo.extend(
        p.order
            .iter()
            .filter_map(|s| Some((s.clone(), render(s)?, 1))),
    );
    for (s, stamp, limit) in todo {
        if p.jobs.len() >= limit
            || !p
                .thumbs
                .get(&s)
                .is_some_and(|t| matches!(t.state, State::Render(_)))
        {
            continue;
        }
        p.next_job += 1;
        let id = p.next_job;
        // far below the world, each shoot in its own spot
        let origin = Vec3::new((id % 64) as f32 * 400.0 - 12_800.0, -20_000.0, 0.0);
        let staged = stage.spawn(&s, origin, studio::THUMB_LAYER, false);
        if let Some(t) = p.thumbs.get_mut(&s) {
            t.state = if staged.is_some() {
                State::Shooting
            } else {
                State::Failed(stamp)
            };
        }
        let Some(staged) = staged else { continue };
        let target = studio::target(&mut stage.images, s.size());
        p.jobs.push(Job {
            id,
            subject: s,
            stamp,
            root: staged.root,
            scenes: staged.scenes,
            skin: staged.skin,
            camera: None,
            target,
            origin,
            frames: 0,
            next_shot: 0,
            reading: false,
            last: None,
            shots: 0,
        });
    }
    let mut finished = vec![];
    for job in p.jobs.iter_mut() {
        job.frames += 1;
        if job.camera.is_none() {
            let staged = Staged {
                root: job.root,
                scenes: job.scenes.clone(),
                skin: job.skin.clone(),
            };
            // parts get their final transforms a frame after they spawn
            if job.frames >= 3
                && let Some(pts) = stage.ready(&staged, studio::THUMB_LAYER)
            {
                let size = job.subject.size();
                let world: Vec<Vec3> = pts.iter().map(|q| *q + job.origin).collect();
                let (at, look, near, far) = studio::frame(
                    &world,
                    studio::view_dir(job.subject.fish()),
                    size.x as f32 / size.y as f32,
                    0.07,
                );
                job.camera = Some(
                    stage
                        .commands
                        .spawn(studio::camera(
                            &stage.studio,
                            job.target.clone(),
                            -100 - (job.id % 64) as isize,
                            at,
                            look,
                            near,
                            far,
                            studio::THUMB_LAYER,
                        ))
                        .id(),
                );
                job.next_shot = job.frames + 8;
            } else if job.frames > 900 || stage.broken(&staged) {
                warn!("thumbnail of {}: its model didn't load", job.subject.key());
                finished.push((job.id, None));
            }
            continue;
        }
        if !job.reading && job.frames >= job.next_shot {
            let id = job.id;
            stage
                .commands
                .spawn(ReadbackOnce::texture(job.target.clone()))
                .observe(move |ev: On<ReadbackComplete>, mut p: ResMut<Previews>| {
                    p.readbacks.insert(id, ev.data.clone());
                });
            job.reading = true;
            continue;
        }
        if job.reading
            && let Some(data) = p.readbacks.remove(&job.id)
        {
            job.reading = false;
            job.shots += 1;
            let n = (data.len() / 4).max(1) as f32;
            let cover = data.chunks_exact(4).filter(|c| c[3] > 16).count() as f32 / n;
            let light = data
                .chunks_exact(4)
                .map(|c| c[0] as f32 + c[1] as f32 + c[2] as f32)
                .sum::<f32>()
                / n;
            let steady = job.last.is_some_and(|(c, l)| {
                (c - cover).abs() < 0.003 && (l - light).abs() <= 0.01 * l.max(1.0)
            });
            if cover > 0.002 && (steady || job.shots >= 8) {
                finished.push((job.id, Some(data)));
            } else if job.shots >= 12 {
                warn!("thumbnail of {}: nothing in the picture", job.subject.key());
                finished.push((job.id, None));
            } else {
                job.last = Some((cover, light));
                job.next_shot = job.frames + 6;
            }
        }
    }
    for (id, data) in finished {
        let Some(i) = p.jobs.iter().position(|j| j.id == id) else {
            continue;
        };
        let job = p.jobs.swap_remove(i);
        stage.commands.entity(job.root).despawn();
        if let Some(c) = job.camera {
            stage.commands.entity(c).despawn();
        }
        let size = job.subject.size();
        let state = match data {
            Some(rgba) => {
                let path = cache::path(&job.subject, job.stamp);
                // a scene keeps its picture too (its cover when shared or listed)
                if let Subject::Scene(id) = &job.subject {
                    let (cover, copy) = (
                        crate::scenes::file::folder(id).join("thumb.png"),
                        rgba.clone(),
                    );
                    IoTaskPool::get()
                        .spawn(async move { cache::save_cover(cover, size, copy) })
                        .detach();
                }
                let copy = rgba.clone();
                IoTaskPool::get()
                    .spawn(async move { cache::save(path, size, copy) })
                    .detach();
                State::Done(job.stamp, stage.images.add(cache::image(size, rgba)))
            }
            None => State::Failed(job.stamp),
        };
        if let Some(t) = p.thumbs.get_mut(&job.subject) {
            t.state = state;
        }
    }
}

/// Hands finished thumbnails to egui (and takes replaced ones back).
fn publish(mut p: ResMut<Previews>, mut textures: ResMut<EguiUserTextures>) {
    let p = &mut *p;
    for h in p.stale.drain(..) {
        textures.remove_image(&h);
    }
    for (s, t) in p.thumbs.iter_mut() {
        if let (None, State::Done(_, h)) = (t.egui, &t.state) {
            t.egui = Some(textures.add_image(EguiTextureHandle::Strong(h.clone())));
        }
        t.wanted = p.pinned.contains(s) && !matches!(t.state, State::Done(..) | State::Failed(_));
    }
    let thumbs = &p.thumbs;
    p.pinned.retain(|s| thumbs.get(s).is_some_and(|t| t.wanted));
    p.turntable.publish(&mut textures);
}
