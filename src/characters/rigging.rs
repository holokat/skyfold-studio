//! Rig behaviors on characters ([`rigkit`]): what runs on top of the clips — feet planted on the
//! ground, the head turning to look, style layers (posture, arm swing, breathing) and moving
//! parts (capes, skirts, hair, tails, pouches swinging under gravity and wind, kept off the body).
//!
//! A character's settings live in `rig.json` next to its rigged file (built-ins: in
//! `characters/mine/_builtin/<id>/`). Adding a moving part or fitting a garment changes the
//! model itself (new bones, new weights, new meshes), so those run as background jobs that write
//! the next rig version, like re-rigging does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use autorig::Model;
use bevy::prelude::*;
use rigkit::author::{self, Build, Suggestion};
use rigkit::spec::{PartKind, Preset};
use rigkit::{Host, Rig, RigSpec};

use super::{CharacterMeta, Characters, Entry, Job, RigKind, user_dir, write_meta};

/// Where a character's rig settings live.
pub fn spec_file(e: &Entry) -> PathBuf {
    match e.builtin {
        Some(_) => user_dir().join("_builtin").join(&e.meta.id).join("rig.json"),
        None => e.dir().join("rig.json"),
    }
}

pub fn read_spec(file: &std::path::Path) -> RigSpec {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_spec(file: &std::path::Path, spec: &RigSpec) -> Result<(), String> {
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let s = serde_json::to_string_pretty(spec).map_err(|e| e.to_string())?;
    std::fs::write(file, s).map_err(|e| e.to_string())
}

/// Drops a character's moving parts from its settings (its skeleton was rebuilt). Returns
/// whether it had any.
pub fn forget_parts(id: &str) -> bool {
    let file = user_dir().join(id).join("rig.json");
    let mut spec = read_spec(&file);
    if spec.parts.is_empty() {
        return false;
    }
    spec.parts.clear();
    spec.colliders.clear();
    let _ = write_spec(&file, &spec);
    true
}

/// Every character's rig settings (loaded when first asked for) and the loose parts found on
/// their models.
#[derive(Resource, Default)]
pub struct RigSpecs {
    /// character id → (the rig key it was read for, settings)
    specs: HashMap<String, (String, RigSpec)>,
    /// bumped by every change (puppets pick up new settings)
    pub revision: u64,
    /// rig key → what was found on it
    found: HashMap<String, Found>,
    /// character id → how its last job ended (read by the MCP tools that wait for one)
    pub outcome: HashMap<String, Result<(), String>>,
}

pub enum Found {
    Looking(Arc<Mutex<Option<Result<FoundParts, String>>>>),
    Ready(Arc<FoundParts>),
    Failed(String),
}

/// What a model offers for moving parts.
#[derive(Debug, Default)]
pub struct FoundParts {
    /// loose pieces that look like they could move, largest first
    pub suggestions: Vec<Suggestion>,
    /// every mesh's name (any can be made to move whole)
    pub meshes: Vec<String>,
}

impl RigSpecs {
    /// A character's settings.
    pub fn get(&mut self, e: &Entry) -> &RigSpec {
        let key = e.rig_key();
        let stale = self.specs.get(&e.meta.id).is_none_or(|(k, _)| *k != key);
        if stale {
            self.specs
                .insert(e.meta.id.clone(), (key, read_spec(&spec_file(e))));
            self.revision += 1;
        }
        &self.specs[&e.meta.id].1
    }
    /// Changes a character's settings (saved at once).
    pub fn set(&mut self, e: &Entry, spec: RigSpec) {
        if let Err(err) = write_spec(&spec_file(e), &spec) {
            warn!("rig settings {}: {err}", e.meta.id);
        }
        self.specs.insert(e.meta.id.clone(), (e.rig_key(), spec));
        self.revision += 1;
    }
    /// The loose parts found on a rigged character (starts looking).
    pub fn suggestions(&mut self, e: &Entry) -> Result<Option<Arc<FoundParts>>, String> {
        let key = e.rig_key();
        match self.found.get(&key) {
            Some(Found::Ready(s)) => return Ok(Some(s.clone())),
            Some(Found::Failed(err)) => return Err(err.clone()),
            Some(Found::Looking(slot)) => {
                let done = slot.lock().unwrap().take();
                match done {
                    Some(Ok(s)) => {
                        let s = Arc::new(s);
                        self.found.insert(key, Found::Ready(s.clone()));
                        return Ok(Some(s));
                    }
                    Some(Err(err)) => {
                        self.found.insert(key, Found::Failed(err.clone()));
                        return Err(err);
                    }
                    None => return Ok(None),
                }
            }
            None => {}
        }
        let Some(file) = e.rig_file() else {
            return Err("rig it first".into());
        };
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        std::thread::spawn(move || {
            let r = autorig::import::load(&file).map(|m| FoundParts {
                suggestions: author::suggest(&m),
                meshes: m.meshes.iter().map(|x| x.name.clone()).collect(),
            });
            *out.lock().unwrap() = Some(r);
        });
        self.found.insert(key, Found::Looking(slot));
        Ok(None)
    }
}

/// What a moving part is made from.
#[derive(Clone, Debug)]
pub enum Source {
    /// a found loose part (index into the suggestions)
    Suggestion(usize),
    /// a whole mesh of the model, by name
    Mesh(String),
}

/// The settings a new part starts with.
#[derive(Clone, Debug, Default)]
pub struct NewPart {
    pub name: Option<String>,
    pub kind: Option<PartKind>,
    pub preset: Option<Preset>,
    pub closed: Option<bool>,
    pub rows: Option<usize>,
    pub columns: Option<usize>,
}

/// Starts a background job on character `id`: `work` gets its rigged model and settings, changes
/// them and they are saved as its next rig version.
fn model_job(
    chars: &mut Characters,
    specs: &mut RigSpecs,
    id: &str,
    what: &'static str,
    work: impl FnOnce(&mut Model, &mut RigSpec, &Mutex<String>) -> Result<String, String> + Send + 'static,
) -> Result<(), String> {
    let e = chars.get(id).ok_or("no such character")?.clone();
    if e.builtin.is_some() {
        return Err("the built-in mannequins can't be changed: make a copy first".into());
    }
    if !e.rigged() {
        return Err("rig it first".into());
    }
    if chars.busy(id).is_some() {
        return Err("this character is busy".into());
    }
    let file = e.rig_file().ok_or("rig it first")?;
    let spec = specs.get(&e).clone();
    specs.outcome.remove(id);
    let status = Arc::new(Mutex::new("Reading the model…".to_string()));
    let result = Arc::new(Mutex::new(None));
    let (st, out) = (status.clone(), result.clone());
    let meta = e.meta.clone();
    std::thread::spawn(move || {
        let r = (|| -> Result<CharacterMeta, String> {
            let mut model = autorig::import::load(&file)?;
            let mut spec = spec;
            let note = work(&mut model, &mut spec, &st)?;
            super::set(&st, "Saving…");
            save_version(meta, &model, &spec, &note)
        })();
        *out.lock().unwrap() = Some(r);
    });
    chars.jobs.push(Job {
        id: id.to_string(),
        what,
        status,
        result,
    });
    Ok(())
}

/// Writes `model` as the character's next rig version, with its settings.
fn save_version(mut meta: CharacterMeta, model: &Model, spec: &RigSpec, note: &str) -> Result<CharacterMeta, String> {
    let dir = user_dir().join(&meta.id);
    let old = (meta.version > 0).then(|| dir.join(format!("rig_v{}.glb", meta.version)));
    meta.version += 1;
    if meta.rig == RigKind::None {
        meta.rig = RigKind::Auto;
    }
    std::fs::write(
        dir.join(format!("rig_v{}.glb", meta.version)),
        autorig::glb::write(model, &[]),
    )
    .map_err(|e| e.to_string())?;
    write_spec(&dir.join("rig.json"), spec)?;
    if let Some(old) = old {
        let _ = std::fs::remove_file(old);
    }
    meta.triangles = model.triangle_count();
    meta.images = model.images.len();
    if !note.is_empty() {
        meta.notes.retain(|n| !n.starts_with("Re-rigging removed"));
    }
    write_meta(&meta)?;
    Ok(meta)
}

/// Adds a moving part to character `id` in the background.
pub fn add_part(chars: &mut Characters, specs: &mut RigSpecs, id: &str, src: Source, opts: NewPart) -> Result<(), String> {
    let e = chars.get(id).ok_or("no such character")?.clone();
    let suggestion = match &src {
        Source::Suggestion(i) => {
            let list = specs.suggestions(&e)?.ok_or("still looking for its loose parts: try again in a moment")?;
            Some(list.suggestions.get(*i).cloned().ok_or("no such suggestion")?)
        }
        Source::Mesh(_) => None,
    };
    model_job(chars, specs, id, "Adding a moving part", move |model, spec, st| {
        super::set(st, "Laying bones through it…");
        let (region, base) = match (&src, suggestion) {
            (_, Some(s)) => {
                let b = Build {
                    name: s.name.clone(),
                    kind: Some(s.kind),
                    closed: Some(s.closed),
                    preset: Some(s.preset),
                    ..Default::default()
                };
                (s.region, b)
            }
            (Source::Mesh(name), None) => {
                let mi = model
                    .meshes
                    .iter()
                    .position(|m| m.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| format!("no mesh called {name}"))?;
                let b = Build {
                    name: name.clone(),
                    ..Default::default()
                };
                (author::mesh_region(model, mi), b)
            }
            _ => unreachable!(),
        };
        let b = Build {
            name: opts.name.unwrap_or(base.name),
            kind: opts.kind.or(base.kind),
            closed: opts.closed.or(base.closed),
            preset: opts.preset.or(base.preset),
            rows: opts.rows,
            columns: opts.columns,
        };
        let pid = author::add_part(model, spec, &region, &b)?;
        let p = spec.part(&pid).unwrap();
        info!("{}: added {} ({} bones)", p.name, p.kind.label(), p.bone_count());
        Ok(String::new())
    })
}

/// Takes a moving part out of character `id` in the background.
pub fn remove_part(chars: &mut Characters, specs: &mut RigSpecs, id: &str, part: &str) -> Result<(), String> {
    let e = chars.get(id).ok_or("no such character")?.clone();
    let spec = specs.get(&e);
    if spec.part(part).is_none() {
        return Err("no such part".into());
    }
    let part = part.to_string();
    model_job(chars, specs, id, "Removing a moving part", move |model, spec, _| {
        // (a part whose bones a re-rig already dropped just leaves the list)
        if author::remove_part(model, spec, &part).is_err() {
            spec.parts.retain(|p| p.id != part);
        }
        Ok(String::new())
    })
}

/// Fits a garment file onto character `id` in the background (its weights from the skin under
/// it), so it moves with the body; its loose parts can then be made to move.
pub fn fit_garment(chars: &mut Characters, specs: &mut RigSpecs, id: &str, path: PathBuf) -> Result<(), String> {
    let e = chars.get(id).ok_or("no such character")?.clone();
    let source = e.dir().join("source").join(&e.meta.source);
    model_job(chars, specs, id, "Fitting clothing", move |model, _spec, st| {
        super::set(st, "Reading the garment…");
        let work = std::env::temp_dir().join(format!("skyfold_garment_{}", std::process::id()));
        let file = super::model_file(&path, &work)?;
        let garment = autorig::import::load(&file);
        let _ = std::fs::remove_dir_all(&work);
        let mut g = garment?;
        if g.meshes.is_empty() {
            return Err("the file has no meshes".into());
        }
        // made in the character's own scene: the same move the character got on import
        if let Ok(mut src) = autorig::import::load(&source) {
            let n = autorig::markers::normalize(&mut src, true);
            g.transform(n.transform);
        }
        g.skeleton = None;
        g.clips.clear();
        for m in &mut g.meshes {
            m.skin = None;
            if m.name.is_empty() {
                m.name = "garment".into();
            }
        }
        super::set(st, "Fitting it to the body…");
        let r = rigkit::transfer::fit_garment(model, g, 0.004)?;
        info!(
            "garment: {} vertices, {} matched, {} filled in, {} lifted{}",
            r.vertices,
            r.matched,
            r.inpainted,
            r.lifted,
            if r.aligned { ", moved onto the body" } else { "" }
        );
        Ok("fitted".into())
    })
}

/// Copies a built-in mannequin into the library so it can be changed. Returns the copy's id.
pub fn copy_builtin(chars: &mut Characters, id: &str) -> Result<String, String> {
    let e = chars.get(id).ok_or("no such character")?.clone();
    let file = e.rig_file().ok_or("not rigged")?;
    if e.builtin.is_none() {
        return Err("only the built-in mannequins need copying".into());
    }
    let name = format!("{} copy", e.meta.name);
    let nid = super::new_id(&name);
    let dir = user_dir().join(&nid);
    std::fs::create_dir_all(dir.join("source")).map_err(|e| e.to_string())?;
    std::fs::copy(&file, dir.join("prepared.glb")).map_err(|e| e.to_string())?;
    std::fs::copy(&file, dir.join("source").join("mannequin.glb")).map_err(|e| e.to_string())?;
    let mut meta = e.meta.clone();
    meta.id = nid.clone();
    meta.name = name;
    meta.source = "mannequin.glb".into();
    meta.rig = RigKind::Imported;
    meta.version = 0;
    meta.export = true;
    meta.notes = vec!["A copy of a built-in mannequin".into()];
    write_meta(&meta)?;
    chars.reload_now();
    Ok(nid)
}

// ------------------------------------------------------------------ run time

/// A puppet's running rig.
pub struct RigRuntime {
    pub rig: Rig,
    key: String,
    revision: u64,
}

impl std::fmt::Debug for RigRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RigRuntime").field("key", &self.key).finish()
    }
}

/// The world a puppet's rig sees.
pub struct WorldHost<'a> {
    pub terrain: Option<&'a crate::terrain::Terrain>,
    pub look: Option<Vec3>,
    pub wind: Vec3,
}

impl Host for WorldHost<'_> {
    fn ground(&self, x: f32, z: f32) -> Option<(f32, Vec3)> {
        let t = self.terrain?;
        let h = t.height_at(x, z);
        let e = 0.25;
        let dx = t.height_at(x + e, z) - t.height_at(x - e, z);
        let dz = t.height_at(x, z + e) - t.height_at(x, z - e);
        Some((h, Vec3::new(-dx, 2.0 * e, -dz).normalize_or(Vec3::Y)))
    }
    fn look_target(&self) -> Option<Vec3> {
        self.look
    }
    fn wind(&self, _p: Vec3) -> Vec3 {
        self.wind
    }
}

impl RigRuntime {
    /// The running rig for `e`, rebuilt when its rig or settings changed.
    pub fn refresh(slot: &mut Option<Self>, e: &Entry, sk: &autorig::Skeleton, specs: &mut RigSpecs) {
        let key = e.rig_key();
        let rev = specs.revision;
        match slot {
            Some(r) if r.key == key && r.revision == rev => {}
            Some(r) if r.key == key => {
                let s = specs.get(e).clone();
                r.rig.set_spec(&s, sk);
                r.revision = specs.revision;
            }
            _ => {
                let s = specs.get(e).clone();
                *slot = Some(RigRuntime {
                    rig: Rig::new(&s, sk),
                    key,
                    revision: specs.revision,
                });
            }
        }
    }
}

/// Where world characters look: the walker's head while walking.
#[derive(Resource, Default)]
pub struct Gaze {
    pub world: Option<Vec3>,
    pub stage: Option<Vec3>,
}

fn collect_outcomes(mut events: MessageReader<super::CharacterEvent>, mut specs: ResMut<RigSpecs>) {
    for e in events.read() {
        specs.outcome.insert(e.id.clone(), e.result.clone());
    }
}

fn update_gaze(walk: Res<crate::walk::WalkState>, stage: Res<super::stage::Stage>, mut gaze: ResMut<Gaze>) {
    gaze.world = walk.active.then(|| walk.position + Vec3::Y * 1.6);
    gaze.stage = stage.eye;
}

pub struct RiggingPlugin;

impl Plugin for RiggingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RigSpecs>()
            .init_resource::<Gaze>()
            .add_systems(Update, (update_gaze, collect_outcomes));
    }
}
