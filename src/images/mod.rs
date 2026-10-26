//! Images: generate pictures inside the builder and keep them in a library with notes and tags
//! (concept art, references, mood boards). One big window of its own, like the Characters lab
//! (`view.rs`): the library grid, an inspector for the picked image, and the Generate box.
//!
//! - `library.rs`: the pictures on disk (`<user assets>/images/<id>/`, image + meta.json).
//! - `backends/`: the image models (Codex imagegen, Cloudflare Workers AI, Black Forest Labs),
//!   any of which the user can pick; keys live in the macOS Keychain (`keychain.rs`).
//! - `jobs.rs`: generation in the background, landing in the library.
//! - `mcp.rs`: every feature as `images_*` MCP tools.

pub mod backends;
pub mod jobs;
pub mod keychain;
pub mod library;
pub mod mcp;
pub mod view;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use backends::{BackendKind, Model};
use library::Library;

const PREFS_KEY: &str = "images";

/// The user's image-model choices (prefs `images`; keys are in the Keychain, never here).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// "backend:model"
    pub model: String,
    pub cloudflare_account: String,
    pub bfl_base: String,
    /// model ids the user added, "backend:model"
    pub custom: Vec<String>,
    pub count: u32,
    pub aspect: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            model: "codex:imagegen".into(),
            cloudflare_account: String::new(),
            bfl_base: backends::bfl::BASES[0].0.into(),
            custom: vec![],
            count: 1,
            aspect: "1:1".into(),
        }
    }
}

impl Settings {
    fn load() -> Self {
        crate::prefs::get(PREFS_KEY)
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) {
        crate::prefs::set(PREFS_KEY, serde_json::to_value(self).unwrap_or_default());
    }
}

/// Whether a backend can paint now, and why not.
#[derive(Clone, Debug, Default)]
pub struct BackendStatus {
    pub ready: bool,
    pub note: String,
    /// a key is stored in the Keychain
    pub has_key: bool,
}

/// What to generate.
#[derive(Clone, Debug, Default)]
pub struct GenerateArgs {
    pub prompt: String,
    /// "backend:model" (None: the default model)
    pub model: Option<String>,
    pub count: u32,
    pub aspect: String,
    pub seed: Option<u64>,
    pub refs: Vec<String>,
    pub ref_files: Vec<std::path::PathBuf>,
    pub parent: Option<String>,
}

#[derive(Resource)]
pub struct Images {
    pub lib: Library,
    pub settings: Settings,
    pub jobs: jobs::Jobs,
    pub status: HashMap<BackendKind, BackendStatus>,
    /// a status check running on a thread
    checking: Option<Arc<Mutex<Option<HashMap<BackendKind, BackendStatus>>>>>,
    checked: bool,
}

impl Default for Images {
    fn default() -> Self {
        Images {
            lib: Library::default(),
            settings: Settings::load(),
            jobs: jobs::Jobs::default(),
            status: HashMap::new(),
            checking: None,
            checked: false,
        }
    }
}

impl Images {
    /// Loads the library and checks the backends the first time they're needed.
    pub fn ensure(&mut self) {
        self.lib.ensure();
        if !self.checked {
            self.checked = true;
            self.check_backends();
        }
    }

    /// Finds out (on a thread) which backends are ready: Codex installed and signed in, keys
    /// stored, the Cloudflare account set.
    pub fn check_backends(&mut self) {
        if self.checking.is_some() {
            return;
        }
        let slot = Arc::new(Mutex::new(None));
        self.checking = Some(slot.clone());
        let account_ok = backends::cloudflare::valid_account(&self.settings.cloudflare_account);
        std::thread::spawn(move || {
            let mut m = HashMap::new();
            let (ready, note) = backends::codex::status();
            m.insert(BackendKind::Codex, BackendStatus { ready, note, has_key: false });
            let cf_key = keychain::has(BackendKind::Cloudflare.key_account());
            m.insert(
                BackendKind::Cloudflare,
                BackendStatus {
                    ready: cf_key && account_ok,
                    has_key: cf_key,
                    note: match (account_ok, cf_key) {
                        (true, true) => "Ready".into(),
                        (false, _) => "Add your account id and an API token".into(),
                        (true, false) => "Add an API token (Workers AI: Read and Edit)".into(),
                    },
                },
            );
            let bfl_key = keychain::has(BackendKind::Bfl.key_account());
            m.insert(
                BackendKind::Bfl,
                BackendStatus {
                    ready: bfl_key,
                    has_key: bfl_key,
                    note: if bfl_key { "Ready".into() } else { "Add your API key".into() },
                },
            );
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(m);
        });
    }

    pub fn checking(&self) -> bool {
        self.checking.is_some()
    }

    fn poll_check(&mut self) {
        let done = self
            .checking
            .as_ref()
            .and_then(|s| s.lock().unwrap_or_else(|e| e.into_inner()).take());
        if let Some(m) = done {
            self.status = m;
            self.checking = None;
        }
    }

    pub fn ready(&self, b: BackendKind) -> bool {
        self.status.get(&b).is_some_and(|s| s.ready)
    }

    /// Every model on offer: each backend's built-ins, then the ones the user added.
    pub fn models(&self) -> Vec<Model> {
        let mut v: Vec<Model> = vec![];
        for b in BackendKind::ALL {
            v.extend(b.models().iter().map(|s| Model::from_spec(b, s)));
            for key in &self.settings.custom {
                if let Some((kb, id)) = backends::parse_key(key)
                    && kb == b
                    && !v.iter().any(|m| m.backend == b && m.id == id)
                {
                    v.push(Model::custom(b, id));
                }
            }
        }
        v
    }

    /// The model for "backend:model" (a built-in, or any valid id).
    pub fn model(&self, key: &str) -> Option<Model> {
        let (b, id) = backends::parse_key(key)?;
        if !backends::valid_model_id(id) {
            return None;
        }
        Some(
            self.models()
                .into_iter()
                .find(|m| m.backend == b && m.id == id)
                .unwrap_or_else(|| Model::custom(b, id)),
        )
    }

    /// The default model (the first ready one if the chosen one is gone).
    pub fn current_model(&self) -> Model {
        self.model(&self.settings.model).unwrap_or_else(|| {
            Model::from_spec(BackendKind::Codex, &backends::codex::MODELS[0])
        })
    }

    /// Stores a key the user typed (Keychain) and re-checks the backends.
    pub fn set_key(&mut self, b: BackendKind, key: &str) -> Result<(), String> {
        if b.key_account().is_empty() {
            return Err("this backend needs no key".into());
        }
        keychain::set(b.key_account(), key)?;
        self.check_backends();
        Ok(())
    }

    /// Starts generating (the UI's Generate and `images_generate`); returns the job id.
    /// `refs`: library ids of reference pictures; `ref_files`: other picture files.
    pub fn generate(&mut self, g: GenerateArgs) -> Result<u64, String> {
        let prompt = g.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("Describe the picture first".into());
        }
        let model = match &g.model {
            Some(k) => self.model(k).ok_or_else(|| format!("unknown model {k} (use backend:model, see images_backends)"))?,
            None => self.current_model(),
        };
        if let Some(st) = self.status.get(&model.backend)
            && !st.ready
        {
            return Err(format!("{}: {}", model.backend.name(), st.note));
        }
        let mut ref_ids = vec![];
        let mut references = vec![];
        for id in &g.refs {
            let f = self.lib.file(id).ok_or_else(|| format!("no image {id}"))?;
            ref_ids.push(id.clone());
            references.push(f);
        }
        for p in &g.ref_files {
            if !p.is_file() {
                return Err(format!("no file {}", p.display()));
            }
            references.push(p.clone());
        }
        if !references.is_empty() && model.max_refs == 0 {
            return Err(format!("{} takes no reference pictures; pick a FLUX.2 model or ChatGPT images", model.name));
        }
        references.truncate(model.max_refs);
        ref_ids.truncate(model.max_refs);
        let aspect = match model.size {
            backends::SizeStyle::Square => "1:1".to_string(),
            _ if backends::ASPECTS.contains(&g.aspect.as_str()) => g.aspect.clone(),
            _ => "1:1".to_string(),
        };
        let creds = backends::Creds {
            cloudflare_account: self.settings.cloudflare_account.clone(),
            key: None,
            bfl_base: self.settings.bfl_base.clone(),
        };
        Ok(self.jobs.start(jobs::Start {
            req: backends::GenRequest {
                model,
                prompt,
                count: g.count.clamp(1, 4),
                aspect,
                seed: g.seed,
                references,
            },
            parent: g.parent,
            ref_ids,
            creds,
        }))
    }

    pub fn forget_key(&mut self, b: BackendKind) {
        if !b.key_account().is_empty() {
            keychain::delete(b.key_account());
            self.check_backends();
        }
    }
}

pub struct ImagesPlugin;

impl Plugin for ImagesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Images>()
            .init_resource::<view::ImageView>()
            .add_systems(Update, (drive, view::dropped_files))
            .add_plugins(view::ViewPlugin);
        mcp::register(app);
    }
}

/// Takes the workers' news (new pictures go into the library) and finished backend checks.
fn drive(mut images: ResMut<Images>, mut ed: ResMut<crate::editor::Editor>) {
    images.poll_check();
    let Images { jobs, lib, .. } = &mut *images;
    for msg in jobs.poll(lib) {
        ed.toast(msg);
    }
}
