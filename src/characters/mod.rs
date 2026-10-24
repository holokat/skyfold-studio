//! Characters (Labs, behind a testing flag): import a model (glTF / GLB, FBX, OBJ or a .zip
//! of one), rig it automatically with the standard humanoid skeleton (`autorig`: body markers,
//! skeleton fit, geodesic voxel skin weights; all local, on the CPU), preview any clip of the
//! animation library on it, place animated characters in the world and export them.
//!
//! Every character lives in `<user assets>/characters/<id>/`: `meta.json`, `source/` (the files
//! as imported), `prepared.glb` (the model stood up, metres, facing +Z; with its own rig when it
//! came with a humanoid one) and `rig_v<N>.glb` (rigged: mesh, skeleton, skin; no clips). The
//! two mannequins of the CC0 Universal Animation Library are built in.
//!
//! Turn it on with `WB_LABS=characters`, the ⌘K palette ("Labs: Characters") or the
//! `characters_labs` MCP tool.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use autorig::markers::{Markers, Normalized};
use autorig::{Model, Skeleton};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub mod anims;
pub mod creatures;
pub mod export;
pub(crate) mod mcp;
pub mod moves;
pub mod npc;
pub mod player;
pub mod puppet;
pub mod stage;
pub mod view;
mod view_char;
mod view_codex;
pub mod world;

pub use anims::AnimLibrary;
pub use world::WorldCharacters;

const LABS_KEY: &str = "labs_characters";

/// Whether the Characters lab is on (env `WB_LABS` naming it, or the stored preference).
pub fn labs_enabled() -> bool {
    static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| {
        std::env::var("WB_LABS")
            .map(|v| v.split(',').any(|s| s.trim() == "characters" || s.trim() == "all"))
            .unwrap_or(false)
    });
    env || LABS.load(std::sync::atomic::Ordering::Relaxed)
}

static LABS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_labs(on: bool) {
    LABS.store(on, std::sync::atomic::Ordering::Relaxed);
    crate::prefs::set(LABS_KEY, serde_json::Value::Bool(on));
}

/// Where user characters live (`<id>/`), and their own animations (`animations/`).
pub fn user_dir() -> PathBuf {
    crate::paths::user_assets_dir().join(USER_REL)
}

/// `user_dir` relative to the user asset root (kept apart from the bundled `characters/`).
const USER_REL: &str = "characters/mine";

/// How a character got its skeleton.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RigKind {
    /// not rigged yet
    #[default]
    None,
    /// fitted and skinned here
    Auto,
    /// the file's own humanoid rig
    Imported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharacterMeta {
    pub id: String,
    pub name: String,
    /// the imported file's name (in `source/`)
    pub source: String,
    #[serde(default)]
    pub rig: RigKind,
    /// the markers the auto-rig used (or guessed, before rigging)
    #[serde(default)]
    pub markers: Option<Markers>,
    /// bumped by every re-rig (the rigged file's name)
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub height: f32,
    #[serde(default)]
    pub triangles: usize,
    #[serde(default)]
    pub images: usize,
    /// quarter turns applied to face +Z when imported
    #[serde(default)]
    pub turns: i32,
    /// included in world exports (with `export_anims`)
    #[serde(default = "yes")]
    pub export: bool,
    /// the clips exported with it (animation library keys)
    #[serde(default = "anims::default_set")]
    pub export_anims: Vec<String>,
    /// what it plays when placed
    #[serde(default)]
    pub idle: Option<String>,
    /// non-fatal notes from import / rigging
    #[serde(default)]
    pub notes: Vec<String>,
    /// its own clip per move (`moves::Move` id → animation key); the rest are the library's
    #[serde(default)]
    pub moves: std::collections::BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}

/// One character of the library.
#[derive(Clone, Debug)]
pub struct Entry {
    pub meta: CharacterMeta,
    /// bundled (read-only): `assets/characters/builtin/<file>`
    pub builtin: Option<&'static str>,
}

pub const BUILTIN: [(&str, &str, &str); 2] = [
    ("builtin_mannequin_m", "Mannequin", "mannequin_m.glb"),
    ("builtin_mannequin_f", "Mannequin (female)", "mannequin_f.glb"),
];

impl Entry {
    pub fn dir(&self) -> PathBuf {
        user_dir().join(&self.meta.id)
    }
    pub fn rigged(&self) -> bool {
        self.builtin.is_some() || self.meta.rig != RigKind::None
    }
    /// The rigged model's AssetServer path.
    pub fn asset_path(&self) -> Option<String> {
        if let Some(f) = self.builtin {
            return Some(format!("characters/builtin/{f}"));
        }
        self.rigged().then(|| {
            crate::paths::user_asset_path(&format!(
                "{USER_REL}/{}/{}",
                self.meta.id,
                self.rig_file_name()
            ))
        })
    }
    /// What the stage shows: the rigged model, or (before rigging) the prepared one.
    pub fn display_path(&self) -> String {
        self.asset_path().unwrap_or_else(|| {
            crate::paths::user_asset_path(&format!("{USER_REL}/{}/prepared.glb", self.meta.id))
        })
    }
    fn rig_file_name(&self) -> String {
        match self.meta.rig {
            RigKind::Imported => "prepared.glb".into(),
            _ => format!("rig_v{}.glb", self.meta.version),
        }
    }
    /// The rigged model on disk.
    pub fn rig_file(&self) -> Option<PathBuf> {
        if let Some(f) = self.builtin {
            return Some(crate::asset_file(&format!("characters/builtin/{f}")));
        }
        self.rigged().then(|| self.dir().join(self.rig_file_name()))
    }
    pub fn prepared_file(&self) -> PathBuf {
        match self.builtin {
            Some(_) => self.rig_file().unwrap(),
            None => self.dir().join("prepared.glb"),
        }
    }
    /// A key that changes whenever the rig does (caches of the skeleton, spawned scenes).
    pub fn rig_key(&self) -> String {
        format!("{}#{}", self.meta.id, self.meta.version)
    }
}

/// Every character, the work in progress and the loaded rigs.
#[derive(Resource, Default)]
pub struct Characters {
    pub entries: Vec<Entry>,
    pub jobs: Vec<Job>,
    /// rig key → its skeleton (loaded off the main thread)
    pub rigs: std::collections::HashMap<String, RigSlot>,
    /// a prepared model kept for marker editing: (character id, model)
    pub prepared: Option<(String, Arc<Model>)>,
    pub loading_prepared: Option<(String, Arc<Mutex<Option<Result<Model, String>>>>)>,
    /// bumped whenever `entries` changes
    pub revision: u64,
    loaded: bool,
}

pub enum RigSlot {
    Loading(Arc<Mutex<Option<Result<Skeleton, String>>>>),
    Ready(Arc<Skeleton>),
    Failed(#[allow(dead_code)] String),
}

/// Background work on one character.
pub struct Job {
    pub id: String,
    pub what: &'static str,
    pub status: Arc<Mutex<String>>,
    pub result: Arc<Mutex<Option<Result<CharacterMeta, String>>>>,
}

impl Characters {
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.meta.id == id)
    }
    pub fn busy(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }
    /// The skeleton of a rigged character, once loaded (starts loading it).
    pub fn skeleton(&mut self, e: &Entry) -> Option<Arc<Skeleton>> {
        let key = e.rig_key();
        match self.rigs.get(&key) {
            Some(RigSlot::Ready(s)) => return Some(s.clone()),
            Some(RigSlot::Failed(_)) => return None,
            Some(RigSlot::Loading(slot)) => {
                let done = slot.lock().unwrap().take();
                match done {
                    Some(Ok(s)) => {
                        let s = Arc::new(s);
                        self.rigs.insert(key, RigSlot::Ready(s.clone()));
                        return Some(s);
                    }
                    Some(Err(err)) => {
                        warn!("character rig {key}: {err}");
                        self.rigs.insert(key, RigSlot::Failed(err));
                    }
                    None => {}
                }
                return None;
            }
            None => {}
        }
        let Some(file) = e.rig_file() else {
            return None;
        };
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        std::thread::spawn(move || {
            let r = autorig::import::load(&file).and_then(|m| {
                m.skeleton
                    .ok_or_else(|| "the rigged file has no skeleton".to_string())
            });
            *out.lock().unwrap() = Some(r);
        });
        self.rigs.insert(key, RigSlot::Loading(slot));
        None
    }
    /// The prepared (unrigged) model of `id` for marker editing, once loaded.
    pub fn prepared_model(&mut self, id: &str) -> Option<Arc<Model>> {
        if let Some((pid, m)) = &self.prepared
            && pid == id
        {
            return Some(m.clone());
        }
        if let Some((lid, slot)) = &self.loading_prepared
            && lid == id
        {
            let done = slot.lock().unwrap().take();
            match done {
                Some(Ok(m)) => {
                    let m = Arc::new(m);
                    self.prepared = Some((id.to_string(), m.clone()));
                    self.loading_prepared = None;
                    return Some(m);
                }
                Some(Err(e)) => {
                    warn!("prepared model {id}: {e}");
                    self.loading_prepared = None;
                }
                None => {}
            }
            return None;
        }
        let file = self.get(id)?.prepared_file();
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(autorig::import::load(&file));
        });
        self.loading_prepared = Some((id.to_string(), slot));
        None
    }

    fn reload(&mut self) {
        let mut entries: Vec<Entry> = BUILTIN
            .iter()
            .map(|(id, name, file)| Entry {
                meta: CharacterMeta {
                    id: id.to_string(),
                    name: name.to_string(),
                    source: file.to_string(),
                    rig: RigKind::Imported,
                    markers: None,
                    version: 0,
                    height: 1.83,
                    triangles: 0,
                    images: 0,
                    turns: 0,
                    export: false,
                    export_anims: anims::default_set(),
                    idle: None,
                    notes: vec![],
                    moves: Default::default(),
                },
                builtin: Some(file),
            })
            .collect();
        // (a built-in's export choice and clips are kept in the prefs)
        if let Some(v) = crate::prefs::get("characters_builtin")
            && let Ok(saved) = serde_json::from_value::<Vec<(String, bool, Vec<String>)>>(v)
        {
            for (id, export, anims) in saved {
                if let Some(e) = entries.iter_mut().find(|e| e.meta.id == id) {
                    e.meta.export = export;
                    e.meta.export_anims = anims;
                }
            }
        }
        let mut user: Vec<Entry> = std::fs::read_dir(user_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|d| {
                let s = std::fs::read_to_string(d.path().join("meta.json")).ok()?;
                let meta: CharacterMeta = serde_json::from_str(&s).ok()?;
                Some(Entry {
                    meta,
                    builtin: None,
                })
            })
            .filter(|e| e.meta.id != "animations")
            .collect();
        user.sort_by(|a, b| a.meta.name.to_lowercase().cmp(&b.meta.name.to_lowercase()));
        user.extend(entries);
        self.entries = user;
        self.revision += 1;
        self.loaded = true;
    }

    /// Stores a character's meta (built-ins: their export choices) and reloads the list.
    pub fn save_meta(&mut self, meta: &CharacterMeta) {
        if let Some(e) = self.entries.iter().find(|e| e.meta.id == meta.id)
            && e.builtin.is_some()
        {
            let mut saved: Vec<(String, bool, Vec<String>)> = self
                .entries
                .iter()
                .filter(|e| e.builtin.is_some())
                .map(|e| (e.meta.id.clone(), e.meta.export, e.meta.export_anims.clone()))
                .collect();
            for s in &mut saved {
                if s.0 == meta.id {
                    s.1 = meta.export;
                    s.2 = meta.export_anims.clone();
                }
            }
            crate::prefs::set("characters_builtin", serde_json::to_value(saved).unwrap());
        } else if let Err(e) = write_meta(meta) {
            warn!("saving character {}: {e}", meta.id);
        }
        self.reload();
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let e = self.get(id).ok_or("no such character")?;
        if e.builtin.is_some() {
            return Err("the built-in mannequins can't be deleted".into());
        }
        let dir = e.dir();
        // (moved aside, not erased: the user's files stay recoverable)
        let trash = user_dir().join(".deleted");
        let _ = std::fs::create_dir_all(&trash);
        std::fs::rename(&dir, trash.join(format!("{id}-{}", now_secs())))
            .map_err(|e| e.to_string())?;
        self.reload();
        Ok(())
    }
}

fn write_meta(meta: &CharacterMeta) -> Result<(), String> {
    let dir = user_dir().join(&meta.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("meta.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(meta).unwrap()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join("meta.json")).map_err(|e| e.to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A folder-safe id from a name.
fn new_id(name: &str) -> String {
    let base: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(32)
        .collect();
    let base = if base.is_empty() || base == "animations" {
        "character".into()
    } else {
        base
    };
    let mut id = base.clone();
    let mut n = 2;
    while user_dir().join(&id).exists() {
        id = format!("{base}_{n}");
        n += 1;
    }
    id
}

// ------------------------------------------------------------------ import & rig jobs

/// What an import should do once the model is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnImport {
    /// keep a humanoid rig the file has, else rig automatically
    Auto,
    /// always fit and skin our own rig
    Rerig,
    /// stop after preparing (markers guessed, not rigged)
    Prepare,
}

impl Characters {
    /// Imports a model file (or a .zip holding one) as a new character, in the background.
    pub fn import(&mut self, path: &Path, on: OnImport) -> String {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().replace(['_', '-'], " "))
            .unwrap_or_else(|| "Character".into());
        let id = new_id(&name);
        let status = Arc::new(Mutex::new("Reading the model…".to_string()));
        let result = Arc::new(Mutex::new(None));
        let (st, out, path, id2) = (status.clone(), result.clone(), path.to_path_buf(), id.clone());
        std::thread::spawn(move || {
            let r = import_job(&id2, &name, &path, on, &st);
            if r.is_err() {
                let _ = std::fs::remove_dir_all(user_dir().join(&id2));
            }
            *out.lock().unwrap() = Some(r);
        });
        self.jobs.push(Job {
            id: id.clone(),
            what: "Importing",
            status,
            result,
        });
        id
    }

    /// Rigs (or re-rigs) a character with `markers` (guessed when `None`), in the background.
    pub fn rig(&mut self, id: &str, markers: Option<Markers>) -> Result<(), String> {
        let e = self.get(id).ok_or("no such character")?.clone();
        if e.builtin.is_some() {
            return Err("the built-in mannequins are rigged already".into());
        }
        if self.busy(id).is_some() {
            return Err("this character is busy".into());
        }
        let status = Arc::new(Mutex::new("Fitting the skeleton…".to_string()));
        let result = Arc::new(Mutex::new(None));
        let (st, out) = (status.clone(), result.clone());
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(rig_job(e.meta, markers, &st));
        });
        self.jobs.push(Job {
            id: id.to_string(),
            what: "Rigging",
            status,
            result,
        });
        Ok(())
    }

    /// Collects finished jobs. Returns the ids that finished (and how).
    fn poll(&mut self) -> Vec<(String, Result<(), String>)> {
        let mut done = vec![];
        self.jobs.retain(|j| {
            let r = j.result.lock().unwrap().take();
            match r {
                Some(Ok(_)) => {
                    done.push((j.id.clone(), Ok(())));
                    false
                }
                Some(Err(e)) => {
                    done.push((j.id.clone(), Err(e)));
                    false
                }
                None => true,
            }
        });
        if !done.is_empty() {
            self.prepared = None;
            self.reload();
        }
        done
    }
}

fn set(status: &Mutex<String>, s: &str) {
    *status.lock().unwrap() = s.to_string();
}

/// The model file in a picked path: itself, or the best one inside a .zip.
fn model_file(path: &Path, work: &Path) -> Result<PathBuf, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext != "zip" {
        return Ok(path.to_path_buf());
    }
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let ok = std::process::Command::new("/usr/bin/unzip")
        .args(["-q", "-o"])
        .arg(path)
        .arg("-d")
        .arg(work)
        .status()
        .map_err(|e| format!("unzip: {e}"))?;
    if !ok.success() {
        return Err("couldn't unpack the .zip".into());
    }
    // prefer FBX (the usual character format), then glTF, then OBJ; the biggest of a kind
    let mut found: Vec<(u8, u64, PathBuf)> = vec![];
    let mut stack = vec![work.to_path_buf()];
    while let Some(d) = stack.pop() {
        for f in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = f.path();
            if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
                || p.components().any(|c| c.as_os_str() == "__MACOSX")
            {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rank = match p
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .as_deref()
            {
                Some("fbx") => 0,
                Some("glb") => 1,
                Some("gltf") => 2,
                Some("obj") => 3,
                _ => continue,
            };
            let size = f.metadata().map(|m| m.len()).unwrap_or(0);
            found.push((rank, u64::MAX - size, p));
        }
    }
    found.sort();
    found
        .into_iter()
        .next()
        .map(|f| f.2)
        .ok_or_else(|| "the .zip has no .fbx, .glb, .gltf or .obj model".into())
}

/// Copies the file (and, for .gltf / .obj, everything beside it) into `source/`.
fn keep_source(file: &Path, into: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| e.to_string())?;
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dest = into.join(file.file_name().ok_or("bad file name")?);
    if ext == "gltf" || ext == "obj" {
        // (external buffers, textures and the .mtl: the whole folder, flat files only)
        if let Some(dir) = file.parent() {
            for f in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let p = f.path();
                if p.is_file() && f.metadata().map(|m| m.len()).unwrap_or(0) < 256 << 20 {
                    let _ = std::fs::copy(&p, into.join(f.file_name()));
                }
            }
        }
        if let Some(tex) = file.parent().map(|d| d.join("textures"))
            && tex.is_dir()
        {
            let _ = std::fs::create_dir_all(into.join("textures"));
            for f in std::fs::read_dir(&tex).into_iter().flatten().flatten() {
                let _ = std::fs::copy(f.path(), into.join("textures").join(f.file_name()));
            }
        }
    } else {
        std::fs::copy(file, &dest).map_err(|e| e.to_string())?;
    }
    Ok(dest)
}

fn import_job(
    id: &str,
    name: &str,
    path: &Path,
    on: OnImport,
    status: &Mutex<String>,
) -> Result<CharacterMeta, String> {
    let dir = user_dir().join(id);
    let work = dir.join("unzipped");
    let file = model_file(path, &work)?;
    let src = keep_source(&file, &dir.join("source"))?;
    let _ = std::fs::remove_dir_all(&work);
    set(status, "Reading the model…");
    let mut model = autorig::import::load(&src)?;
    if model.meshes.is_empty() || model.vertex_count() == 0 {
        return Err("the file has no meshes".into());
    }
    let n: Normalized = autorig::markers::normalize(&mut model, true);
    let mut notes = vec![];
    if n.quarter_turns != 0 {
        notes.push(format!(
            "Turned {}° to face forward",
            (n.quarter_turns * 90).rem_euclid(360)
        ));
    }
    if !(0.3..=6.0).contains(&n.height) {
        notes.push(format!("It is {:.2} m tall: check its size", n.height));
    }
    // a humanoid rig of its own?
    let own = model
        .skeleton
        .as_ref()
        .map(|sk| autorig::humanoid::is_humanoid(&autorig::humanoid::map_bones(sk)))
        .unwrap_or(false);
    let skinned = model.meshes.iter().any(|m| m.skin.is_some());
    let keep_own = own && skinned && on == OnImport::Auto;
    if model.skeleton.is_some() && !keep_own {
        if !own {
            notes.push("Its own rig isn't a humanoid one: rigged automatically".into());
        }
        model.skeleton = None;
        model.clips.clear();
        for m in &mut model.meshes {
            m.skin = None;
        }
    }
    set(status, "Saving…");
    let prepared = autorig::glb::write(&model, &[]);
    std::fs::write(dir.join("prepared.glb"), prepared).map_err(|e| e.to_string())?;
    let mut meta = CharacterMeta {
        id: id.to_string(),
        name: name.to_string(),
        source: src
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
        rig: if keep_own {
            RigKind::Imported
        } else {
            RigKind::None
        },
        markers: None,
        version: 0,
        height: n.height,
        triangles: model.triangle_count(),
        images: model.images.len(),
        turns: n.quarter_turns,
        export: true,
        export_anims: anims::default_set(),
        idle: None,
        notes,
        moves: Default::default(),
    };
    if keep_own {
        meta.notes.push("Uses the rig it came with".into());
        write_meta(&meta)?;
        return Ok(meta);
    }
    set(status, "Finding the body…");
    meta.markers = Some(autorig::markers::guess(&model));
    write_meta(&meta)?;
    if on == OnImport::Prepare {
        return Ok(meta);
    }
    rig_model(meta, model, None, status)
}

fn rig_job(
    meta: CharacterMeta,
    markers: Option<Markers>,
    status: &Mutex<String>,
) -> Result<CharacterMeta, String> {
    set(status, "Reading the model…");
    let mut model = autorig::import::load(&user_dir().join(&meta.id).join("prepared.glb"))?;
    model.skeleton = None;
    model.clips.clear();
    for m in &mut model.meshes {
        m.skin = None;
    }
    rig_model(meta, model, markers, status)
}

fn rig_model(
    mut meta: CharacterMeta,
    mut model: Model,
    markers: Option<Markers>,
    status: &Mutex<String>,
) -> Result<CharacterMeta, String> {
    set(status, "Fitting the skeleton and skinning…");
    let m = markers.or(meta.markers);
    let (used, report) = autorig::rig::auto_rig(&mut model, m, Default::default());
    info!(
        "rigged {}: {} voxels solid, {} vertices unreached, {} ms",
        meta.id, report.solid, report.unreached, report.millis
    );
    meta.notes.retain(|n| !n.starts_with("Uses the rig"));
    if report.unreached > 0 {
        meta.notes.push(format!(
            "{} vertices sit apart from the body (weighted by distance)",
            report.unreached
        ));
    }
    set(status, "Saving…");
    let old = user_dir()
        .join(&meta.id)
        .join(format!("rig_v{}.glb", meta.version));
    meta.version += 1;
    meta.markers = Some(used);
    meta.rig = RigKind::Auto;
    let bytes = autorig::glb::write(&model, &[]);
    let dir = user_dir().join(&meta.id);
    std::fs::write(dir.join(format!("rig_v{}.glb", meta.version)), bytes)
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(old);
    write_meta(&meta)?;
    Ok(meta)
}

// ------------------------------------------------------------------ plugin

/// Toasts and selection after jobs finish.
#[derive(Message, Clone, Debug)]
pub struct CharacterEvent {
    pub id: String,
    pub result: Result<(), String>,
}

pub struct CharactersPlugin;

impl Plugin for CharactersPlugin {
    fn build(&self, app: &mut App) {
        if crate::prefs::get_bool(LABS_KEY).unwrap_or(false) {
            LABS.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        app.init_resource::<Characters>()
            .init_resource::<AnimLibrary>()
            .init_resource::<WorldCharacters>()
            .init_resource::<view::CharacterView>()
            .add_message::<CharacterEvent>()
            .add_systems(Update, poll_jobs)
            .add_plugins((
                puppet::PuppetPlugin,
                stage::StagePlugin,
                view::ViewPlugin,
                world::WorldPlugin,
                npc::NpcPlugin,
                player::PlayerPlugin,
                creatures::CreaturesPlugin,
            ));
        mcp::register(app);
    }
}

fn poll_jobs(mut chars: ResMut<Characters>, mut events: MessageWriter<CharacterEvent>) {
    if !chars.loaded {
        chars.reload();
    }
    if chars.jobs.is_empty() {
        return;
    }
    for (id, result) in chars.poll() {
        if let Err(e) = &result {
            warn!("character {id}: {e}");
        }
        events.write(CharacterEvent { id, result });
    }
}
