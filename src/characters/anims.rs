//! The animation library: the clips every character can play. Bundled: the Quaternius Universal
//! Animation Library 1 and 2 (CC0, on the standard skeleton itself). The user's own clips
//! (Mixamo downloads, packs they bought: FBX or glTF, any humanoid rig) are imported into
//! `<user assets>/characters/animations/` and stay on their machine.
//!
//! Clips are keyed `<source>/<clip name>`; each source keeps its own skeleton, and
//! `crate::characters::puppet` retargets from it onto each character.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use autorig::{Clip, Skeleton};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Idle,
    Move,
    Combat,
    Actions,
    Social,
    Creature,
    Mine,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Idle,
        Category::Move,
        Category::Combat,
        Category::Actions,
        Category::Social,
        Category::Creature,
        Category::Mine,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Category::Idle => "Idle",
            Category::Move => "Move",
            Category::Combat => "Combat",
            Category::Actions => "Actions",
            Category::Social => "Social",
            Category::Creature => "Creature",
            Category::Mine => "Mine",
        }
    }
}

pub struct Source {
    pub id: String,
    pub name: String,
    /// "CC0 · Quaternius" / "Yours (stays on this Mac)"
    pub license: String,
    pub skeleton: Arc<Skeleton>,
    pub clips: Vec<Arc<Clip>>,
    pub user: bool,
}

#[derive(Clone, Debug)]
pub struct Anim {
    /// "<source>/<clip>"
    pub key: String,
    pub label: String,
    pub category: Category,
    pub looped: bool,
    pub source: usize,
    pub clip: usize,
    pub seconds: f32,
}

#[derive(Resource, Default)]
pub struct AnimLibrary {
    pub sources: Vec<Source>,
    pub anims: Vec<Anim>,
    loading: Option<Arc<Mutex<Option<Vec<Source>>>>>,
    pub loaded: bool,
    pub revision: u64,
}

/// The clips a new character exports with.
pub fn default_set() -> Vec<String> {
    [
        "Idle_Loop",
        "Walk_Loop",
        "Jog_Fwd_Loop",
        "Sprint_Loop",
        "Jump_Start",
        "Jump_Loop",
        "Jump_Land",
        "Roll",
        "Punch_Jab",
        "Punch_Cross",
        "Sword_Idle",
        "Sword_Attack",
        "Hit_Chest",
        "Death01",
    ]
    .iter()
    .map(|c| format!("ual1/{c}"))
    .chain(["ual2/Sword_Regular_Combo", "ual2/Sword_Block", "ual2/Idle_FoldArms_Loop"].map(String::from))
    .collect()
}

/// The animation to play when nothing was chosen.
pub const IDLE: &str = "ual1/Idle_Loop";

pub fn user_dir() -> PathBuf {
    super::user_dir().join("animations")
}

impl AnimLibrary {
    /// Starts loading (once); true when ready.
    pub fn ensure(&mut self) -> bool {
        if self.loaded {
            return true;
        }
        if let Some(slot) = &self.loading {
            let done = slot.lock().unwrap().take();
            if let Some(sources) = done {
                self.loading = None;
                self.set_sources(sources);
                return true;
            }
            return false;
        }
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(load_all());
        });
        self.loading = Some(slot);
        false
    }

    fn set_sources(&mut self, sources: Vec<Source>) {
        self.sources = sources;
        self.anims = vec![];
        for (si, s) in self.sources.iter().enumerate() {
            for (ci, c) in s.clips.iter().enumerate() {
                if c.name == "A_TPose" {
                    continue;
                }
                let (label, category, looped) = describe(&c.name, s.user);
                self.anims.push(Anim {
                    key: format!("{}/{}", s.id, c.name),
                    label,
                    category,
                    looped,
                    source: si,
                    clip: ci,
                    seconds: c.duration(),
                });
            }
        }
        self.anims.sort_by(|a, b| {
            (a.category as u8, a.label.to_lowercase()).cmp(&(b.category as u8, b.label.to_lowercase()))
        });
        self.loaded = true;
        self.revision += 1;
    }

    pub fn find(&self, key: &str) -> Option<&Anim> {
        self.anims.iter().find(|a| a.key == key)
    }

    pub fn clip(&self, a: &Anim) -> (&Source, &Arc<Clip>) {
        let s = &self.sources[a.source];
        (s, &s.clips[a.clip])
    }

    /// Imports the user's own animation file(s): each becomes a source of its own. Runs on
    /// the calling thread (call it off the main one).
    pub fn import_files(paths: &[PathBuf]) -> Result<Vec<String>, String> {
        let dir = user_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut added = vec![];
        for p in paths {
            let m = autorig::import::load(p)?;
            let Some(sk) = &m.skeleton else {
                return Err(format!("{}: no skeleton in the file", p.display()));
            };
            if m.clips.is_empty() {
                return Err(format!("{}: no animation in the file", p.display()));
            }
            let map = autorig::humanoid::map_bones(sk);
            if !autorig::humanoid::is_humanoid(&map) {
                return Err(format!(
                    "{}: not a humanoid rig (missing {})",
                    p.display(),
                    autorig::humanoid::missing(&map).join(", ")
                ));
            }
            // kept as a GLB of the skeleton and its clips (no mesh)
            let only = autorig::Model {
                skeleton: m.skeleton.clone(),
                ..Default::default()
            };
            let stem = p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "clips".into());
            // ("Unreal Take", "Take 001", "mixamo.com": a one-clip file is named after itself)
            let mut clips = m.clips.clone();
            let generic = |n: &str| {
                let n = n.to_lowercase();
                n.contains("take") || n == "mixamo.com" || n.is_empty() || n.starts_with("anim")
            };
            if clips.len() == 1 {
                clips[0].name = stem.clone();
            } else {
                for (i, c) in clips.iter_mut().enumerate() {
                    if generic(&c.name) {
                        c.name = format!("{stem} {}", i + 1);
                    }
                }
            }
            let file = dir.join(format!("{}.glb", safe(&stem)));
            std::fs::write(&file, autorig::glb::write(&only, &clips)).map_err(|e| e.to_string())?;
            added.extend(clips.iter().map(|c| format!("mine_{}/{}", safe(&stem), c.name)));
        }
        Ok(added)
    }

    /// Reloads everything (after an import).
    pub fn reload(&mut self) {
        self.loaded = false;
        self.loading = None;
    }
}

fn safe(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

pub(crate) fn load_all() -> Vec<Source> {
    let mut out = vec![];
    for (id, name) in [("ual1", "Universal Animation Library"), ("ual2", "Universal Animation Library 2")] {
        let file = crate::asset_file(&format!("characters/animations/{id}.glb"));
        match autorig::import::load(&file) {
            Ok(m) => out.push(source(id, name, "CC0 · Quaternius", m, false)),
            Err(e) => warn!("animation library {}: {e}", file.display()),
        }
    }
    let mut mine: Vec<PathBuf> = std::fs::read_dir(user_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|f| f.path())
        .filter(|p| p.extension().is_some_and(|e| e == "glb"))
        .collect();
    mine.sort();
    for p in mine {
        let stem = p.file_stem().unwrap().to_string_lossy().into_owned();
        match autorig::import::load(&p) {
            Ok(m) => out.push(source(
                &format!("mine_{stem}"),
                &stem.replace('_', " "),
                "Yours (stays on this Mac)",
                m,
                true,
            )),
            Err(e) => warn!("your animations {}: {e}", p.display()),
        }
    }
    out
}

fn source(id: &str, name: &str, license: &str, m: autorig::Model, user: bool) -> Source {
    Source {
        id: id.to_string(),
        name: name.to_string(),
        license: license.to_string(),
        skeleton: Arc::new(m.skeleton.unwrap_or_default()),
        clips: m.clips.into_iter().map(Arc::new).collect(),
        user,
    }
}

/// A clip's label, category and whether it loops, from its name.
fn describe(name: &str, user: bool) -> (String, Category, bool) {
    let looped = name.ends_with("_Loop") || name.contains("Idle") || name.ends_with("Loop");
    let label = name
        .trim_start_matches("Anim_")
        .trim_end_matches("_RM")
        .trim_end_matches("_Loop")
        .replace('_', " ")
        .replace("Fwd", "forward")
        .replace("RM", "")
        .trim()
        .to_string();
    let n = name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    let category = if user {
        Category::Mine
    } else if has(&["zombie"]) {
        Category::Creature
    } else if has(&[
        "sword", "punch", "melee", "shield", "pistol", "spell", "hit_", "death", "throw", "knockback",
    ]) {
        Category::Combat
    } else if has(&[
        "walk", "jog", "sprint", "run", "crouch_fwd", "swim", "slide", "climb", "jump", "roll",
    ]) {
        Category::Move
    } else if has(&["dance", "talk", "yes", "_no", "foldarms", "phone", "rail_call"]) {
        Category::Social
    } else if has(&["idle", "crouch_idle", "sitting_idle", "lay"]) {
        Category::Idle
    } else {
        Category::Actions
    };
    (label, category, looped)
}
