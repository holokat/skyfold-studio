//! A new design sheet for one dungeon, painted and read by the user's own AI (the Codex CLI that
//! comes with the ChatGPT app, signed in with ChatGPT): Codex writes an original brief for the
//! world's setting and a layout type the world hasn't used, paints the sheet from the same
//! template the library was made with (`sheet_prompt.md`, its `$imagegen` skill), checks it and
//! retries; then a second, read-only run reads the sheet (`read_prompt.md`) and `read` measures it
//! into a blueprint. A finished sheet lands in the user's own sheet folder
//! (`library::user_dir`); if it was made for a dungeon whose map isn't made yet, that dungeon is
//! switched to it (so a world made during onboarding gets its library sheets at once and its own
//! unique ones a few minutes later).
//!
//! Jobs run on worker threads, two at a time; `SheetJobs` holds their progress for the panel and
//! `dungeons_sheet_jobs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::blueprint::Kind;
use super::library::{self, SheetRef};

pub const SHEET_PROMPT: &str = include_str!("sheet_prompt.md");
pub const LAYOUT_TYPES: &str = include_str!("layout_types.json");

/// How each layout type ends (kept in step with tools/dungeon_sheets/make_jobs.py).
fn ending_text(kind: &str) -> &'static str {
    match kind {
        "shortcut" => {
            "A one-way shortcut — a steep stair, rope slide or lift, labelled \"Shortcut\" — drops from the final arena straight back toward the Arrival area, drawn as a grey dotted arrow."
        }
        "return" => {
            "A one-way return path leads from the final arena back to near the Arrival area through a \"Return gate\", drawn as a grey dotted arrow."
        }
        _ => {
            "After the final boss an Exit portal opens in the final arena: draw it as a glowing ring labelled \"Exit portal\". No return path."
        }
    }
}

fn post_text(family: &str) -> &'static str {
    match family {
        "arid" => {
            "Warm bloom, strong contrast, heat haze at the horizon. No depth of field or motion blur."
        }
        "cold" => {
            "Clean cool contrast, soft bloom on snow, light blue shadows. No depth of field or motion blur."
        }
        "tropical" => {
            "Soft humid bloom, saturated greens, gentle haze. No depth of field or motion blur."
        }
        "volcanic" => {
            "Strong bloom on lava, deep shadows, warm haze. No depth of field or motion blur."
        }
        "twilight" => {
            "Soft bloom on glows, violet shadows, light fog. No depth of field or motion blur."
        }
        _ => "Gentle bloom, warm highlights, cool shadows. No depth of field or motion blur.",
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct LayoutType {
    pub id: String,
    pub name: String,
    pub ending: String,
    pub structure: String,
}

pub fn layout_types() -> Vec<LayoutType> {
    serde_json::from_str(LAYOUT_TYPES).unwrap_or_default()
}

/// What to make a sheet for.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub family: String,
    pub kind: Kind,
    /// a layout type id (None: one the world hasn't used)
    pub layout_type: Option<String>,
    pub world: String,
    /// dungeon names already in the world (the brief avoids them)
    pub avoid: Vec<String>,
    /// layout types the world already has (a new one is preferred)
    pub used_types: Vec<String>,
    /// the dungeon to switch to the sheet once it is ready
    pub dungeon: Option<u32>,
    pub seed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Queued,
    Painting,
    Reading,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    pub id: String,
    pub family: String,
    pub kind: Kind,
    pub layout_type: String,
    pub dungeon: Option<u32>,
    pub stage: Stage,
    pub note: String,
    #[serde(skip)]
    pub started: Option<Instant>,
    /// the finished sheet's name
    pub name: String,
    /// switched its dungeon to the new sheet
    pub applied: bool,
}

#[derive(Resource, Default)]
pub struct SheetJobs {
    pub jobs: Vec<Job>,
    queue: Vec<(Request, String, LayoutType)>,
    updates: Arc<Mutex<Vec<(String, Stage, String, String)>>>,
    /// Codex's path (None: not found — sheets can't be painted here)
    pub codex: Option<PathBuf>,
    looked: bool,
}

impl SheetJobs {
    pub fn running(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| matches!(j.stage, Stage::Painting | Stage::Reading))
            .count()
    }
    pub fn codex(&mut self) -> Option<PathBuf> {
        if !self.looked {
            self.looked = true;
            self.codex = crate::prompt::agent::find_codex();
        }
        self.codex.clone()
    }
}

/// Queues a sheet; returns its job id.
pub fn queue(jobs: &mut SheetJobs, r: Request) -> Result<String, String> {
    if jobs.codex().is_none() {
        return Err("painting a new sheet needs the Codex CLI (the ChatGPT app, signed in with ChatGPT); the library's sheets work without it".into());
    }
    let types = layout_types();
    let ty = match &r.layout_type {
        Some(t) => types.iter().find(|x| x.id == *t).cloned().ok_or_else(|| {
            format!(
                "unknown layout type `{t}`: {}",
                types
                    .iter()
                    .map(|t| t.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?,
        None => {
            // (a type the world doesn't have yet; the loop rarely)
            let fresh: Vec<&LayoutType> = types
                .iter()
                .filter(|t| !r.used_types.contains(&t.id) && t.id != "loop")
                .collect();
            let pool: Vec<&LayoutType> = if fresh.is_empty() {
                types.iter().collect()
            } else {
                fresh
            };
            pool[(r.seed as usize) % pool.len()].clone()
        }
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = format!("u{stamp}_{}_{}", r.family, ty.id);
    jobs.jobs.push(Job {
        id: id.clone(),
        family: r.family.clone(),
        kind: r.kind,
        layout_type: ty.id.clone(),
        dungeon: r.dungeon,
        stage: Stage::Queued,
        note: String::new(),
        started: None,
        name: String::new(),
        applied: false,
    });
    jobs.queue.push((r, id.clone(), ty));
    Ok(id)
}

/// The template with everything but the brief's own words filled in.
fn template_for(r: &Request, ty: &LayoutType) -> String {
    let (w, zones, bosses) = match r.kind {
        Kind::Raid => ([340, 360, 380, 400][r.seed as usize % 4], 7 + r.seed % 2, 4),
        Kind::Party => (
            [260, 280, 300, 320][r.seed as usize % 4],
            5 + r.seed % 2,
            2 + r.seed % 2,
        ),
    };
    let depth = ((w as f32 * 0.69 / 10.0).round() * 10.0) as u32;
    let mut s = SHEET_PROMPT.to_string();
    for (k, v) in [
        ("TYPE_NAME", ty.name.clone()),
        ("TYPE_STRUCTURE", ty.structure.clone()),
        ("ENDING", ending_text(&ty.ending).to_string()),
        ("WIDTH", w.to_string()),
        ("DEPTH", depth.to_string()),
        ("ZONE_COUNT", zones.to_string()),
        ("BOSS_COUNT", bosses.to_string()),
        ("POST", post_text(&r.family).to_string()),
        ("STYLE", "Hand-painted fantasy".to_string()),
    ] {
        s = s.replace(&format!("{{{{{k}}}}}"), &v);
    }
    s
}

const CODEX_BRIEF: &str = r#"# Design one dungeon sheet

You are making a top-down level-design sheet for an open-air dungeon in a fantasy world builder.

1. Read `request.json`. Write `brief.json`, an object with exactly these string fields:
   "name", "archetype", "setting", "layout_hint", "features", "materials", "light", "vfx".
   It is an ORIGINAL dungeon for the world named in the request, in its setting family, sized as
   its kind says, and its layout must follow the request's layout type (its structure is given).
   - "layout_hint" names the edge or corner the route starts from, which way it runs and where the
     finale is, consistent with the layout type; vary it — not always clockwise, not always from the
     south-west.
   - "features": 1–3 hero landmarks; "materials": 4–6 separated by " • "; "light": time of day and
     the sun's direction (e.g. "late afternoon, sun from the north-west. Warm key, cool skylight.");
     "vfx": 3–4 effects.
   - The name must differ from every name in "avoid". No real-world religious symbols, brands or
     franchise names.
2. Write `prompt.txt`: the text of `TEMPLATE.md` with its remaining slots {{NAME}}, {{ARCHETYPE}},
   {{SETTING}}, {{LAYOUT_HINT}}, {{FEATURES}}, {{MATERIALS}}, {{LIGHT}} and {{VFX}} replaced by
   brief.json's values. Change nothing else; no {{...}} may remain.
3. Use the $imagegen skill to make `sheet.png` (landscape 1536 × 1024) from the full text of
   prompt.txt, verbatim — do not shorten or rewrite it.
4. Check the image for what matters: it reads as a map seen from above (a slight lean showing
   some cliff and wall faces is fine; a strong perspective view is not); the layout follows the
   layout type (not a ring of zones round a central lake unless the type is a loop); every zone
   has its red numbered disc and captions are legible. Small flaws (a missing prop tag, a stair
   label that disagrees with the elevations) are fine: they are repaired later. Regenerate only a
   clear failure, at most once, and keep the better one as `sheet.png`.
5. Write `done.json`: {"ok": true or false, "notes": "what passed or failed"}.

Only generate images; never edit an image with code.
"#;

fn run(cmd: &mut Command, log: &Path, timeout: Duration) -> Result<(), String> {
    let out = std::fs::File::create(log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("couldn't start Codex: {e}"))?;
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return Ok(()),
            Ok(Some(s)) => return Err(format!("Codex stopped ({s})")),
            Ok(None) if t0.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Codex took too long".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(500)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// One job, start to end, on a worker thread.
fn work(
    codex: PathBuf,
    r: Request,
    id: String,
    ty: LayoutType,
    up: Arc<Mutex<Vec<(String, Stage, String, String)>>>,
) {
    let say = |stage: Stage, note: &str, name: &str| {
        up.lock().unwrap_or_else(|e| e.into_inner()).push((
            id.clone(),
            stage,
            note.to_string(),
            name.to_string(),
        ))
    };
    let dir = library::user_dir().join(&id);
    let result = (|| -> Result<String, String> {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let req = json!({
            "world": r.world, "family": r.family, "kind": r.kind, "avoid": r.avoid,
            "layout_type": { "id": ty.id, "name": ty.name, "structure": ty.structure },
        });
        std::fs::write(
            dir.join("request.json"),
            serde_json::to_string_pretty(&req).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(dir.join("TEMPLATE.md"), template_for(&r, &ty))
            .map_err(|e| e.to_string())?;
        std::fs::write(dir.join("BRIEF.md"), CODEX_BRIEF).map_err(|e| e.to_string())?;
        say(
            Stage::Painting,
            "Codex is writing the brief and painting the sheet",
            "",
        );
        run(
            Command::new(&codex).args(["exec", "--skip-git-repo-check", "-C"]).arg(&dir).arg("-o").arg(dir.join("reply.txt")).arg("Use the $imagegen skill for the image. Read BRIEF.md and carry it out completely."),
            &dir.join("paint.log"),
            Duration::from_secs(40 * 60),
        )?;
        let sheet = dir.join("sheet.png");
        if !sheet.exists() {
            return Err("Codex didn't make sheet.png".into());
        }
        let brief: Value = std::fs::read_to_string(dir.join("brief.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(json!({}));
        say(
            Stage::Reading,
            "Reading the sheet",
            brief["name"].as_str().unwrap_or_default(),
        );
        let mut b = brief.clone();
        b["id"] = json!(id);
        b["family"] = json!(r.family);
        b["kind"] = json!(r.kind);
        b["type"] = json!(ty.id);
        read_folder(&codex, &dir, &b)
    })();
    match result {
        Ok(name) => say(Stage::Ready, "Ready", &name),
        Err(e) => say(Stage::Failed, &e, ""),
    }
}

/// Reads the sheet in a job folder (`sheet.png`) with Codex (read-only, the image attached) and
/// measures it into `blueprint.json`; returns the dungeon's name.
pub fn read_folder(codex: &Path, dir: &Path, brief: &Value) -> Result<String, String> {
    let sheet = dir.join("sheet.png");
    // (the prompt goes before `-i`: that flag takes any number of files)
    run(
        Command::new(codex)
            .args(["exec", "--skip-git-repo-check", "-s", "read-only", "-C"])
            .arg(dir)
            .arg("-o")
            .arg(dir.join("reading.txt"))
            .arg(super::read::READ_PROMPT)
            .arg("-i")
            .arg(&sheet),
        &dir.join("read.log"),
        Duration::from_secs(15 * 60),
    )?;
    let text =
        std::fs::read_to_string(dir.join("reading.txt")).map_err(|e| format!("no reading: {e}"))?;
    let reading = super::read::parse(&text)?;
    let img = image::open(&sheet).map_err(|e| e.to_string())?.to_rgba8();
    let (bp, _notes) = super::read::normalize(&reading, Some(&img), Some(brief))?;
    let bad = bp.problems();
    std::fs::write(
        dir.join("blueprint.json"),
        serde_json::to_string_pretty(&bp).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    if !bad.is_empty() {
        return Err(format!("the sheet can't be built: {}", bad.join("; ")));
    }
    Ok(bp.name.clone())
}

/// Starts queued jobs, takes the workers' news, and switches dungeons to their finished sheets.
fn drive(
    mut jobs: ResMut<SheetJobs>,
    mut dungeons: ResMut<crate::dungeons::Dungeons>,
    mut meta: ResMut<crate::world::WorldMeta>,
) {
    let news: Vec<_> = std::mem::take(&mut *jobs.updates.lock().unwrap_or_else(|e| e.into_inner()));
    for (id, stage, note, name) in news {
        if let Some(j) = jobs.jobs.iter_mut().find(|j| j.id == id) {
            j.stage = stage;
            j.note = note;
            if !name.is_empty() {
                j.name = name;
            }
            if stage == Stage::Failed {
                warn!("dungeon sheet {id}: {}", j.note);
            }
        }
    }
    // a finished sheet replaces its dungeon's library sheet while that map isn't made yet
    let world = meta.path.clone();
    for j in jobs
        .jobs
        .iter_mut()
        .filter(|j| j.stage == Stage::Ready && !j.applied)
    {
        j.applied = true;
        let Some(did) = j.dungeon else { continue };
        let made = |folder: &str| {
            world
                .as_ref()
                .is_some_and(|w| w.join("dungeons").join(folder).join("world.json").exists())
        };
        let Some(def) = dungeons.list.iter_mut().find(|d| d.id == did) else {
            continue;
        };
        if made(&def.folder) || def.sheet.is_none() {
            j.note =
                "Ready (its dungeon was already made from another sheet; it stays in your sheets)"
                    .into();
            continue;
        }
        def.sheet = Some(SheetRef {
            id: j.id.clone(),
            custom: true,
            family: j.family.clone(),
        });
        meta.dirty = true;
        j.note = format!("Ready: {} now uses it", def.name);
        info!("dungeon {} switched to its own sheet {}", def.name, j.id);
    }
    // start what's queued (two at a time)
    while jobs.running() < 2 && !jobs.queue.is_empty() {
        let (r, id, ty) = jobs.queue.remove(0);
        let Some(codex) = jobs.codex() else { break };
        if let Some(j) = jobs.jobs.iter_mut().find(|j| j.id == id) {
            j.stage = Stage::Painting;
            j.started = Some(Instant::now());
        }
        let up = jobs.updates.clone();
        std::thread::Builder::new()
            .name(format!("sheet-{id}"))
            .spawn(move || work(codex, r, id, ty, up))
            .ok();
    }
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<SheetJobs>().add_systems(Update, drive);
}

/// A user sheet folder's picture path (for tests and the panel).
pub fn sheet_png(id: &str) -> PathBuf {
    library::user_dir().join(id).join("sheet.png")
}

/// `WB_SHEET_GENERATE=<family>,<party|raid>[,<layout type>]`: paints and reads one sheet with Codex
/// right here (no app), printing where it landed and its blueprint's problems — the runtime path
/// a world's "unique dungeons" take. It uses one image of the user's ChatGPT allowance.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    fn generate_one_sheet() {
        let Ok(spec) = std::env::var("WB_SHEET_GENERATE") else {
            return;
        };
        let v: Vec<&str> = spec.split(',').map(str::trim).collect();
        let codex = crate::prompt::agent::find_codex().expect("the Codex CLI");
        let r = Request {
            family: v[0].into(),
            kind: if v.get(1) == Some(&"raid") {
                Kind::Raid
            } else {
                Kind::Party
            },
            layout_type: v.get(2).map(|s| s.to_string()),
            world: "Test World".into(),
            avoid: vec![],
            used_types: vec![],
            dungeon: None,
            seed: 3,
        };
        let types = layout_types();
        let ty = types
            .iter()
            .find(|t| Some(t.id.as_str()) == r.layout_type.as_deref())
            .cloned()
            .unwrap_or_else(|| types[3].clone());
        let id = format!("test_{}_{}", r.family, ty.id);
        let up = Arc::new(Mutex::new(vec![]));
        work(codex, r, id.clone(), ty, up.clone());
        for (_, stage, note, name) in up.lock().unwrap().iter() {
            println!("{stage:?}: {note} {name}");
        }
        println!("sheet folder: {}", library::user_dir().join(&id).display());
    }
}

/// `WB_SHEET_READ_DIR=<job folder>`: reads an already painted sheet (its `sheet.png`, `brief.json`,
/// `request.json`) with Codex into its blueprint, printing the result.
#[cfg(test)]
mod live_read {
    use super::*;

    #[test]
    fn read_one_folder() {
        let Ok(dir) = std::env::var("WB_SHEET_READ_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let codex = crate::prompt::agent::find_codex().expect("the Codex CLI");
        let mut b: Value = std::fs::read_to_string(dir.join("brief.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(json!({}));
        let req: Value = std::fs::read_to_string(dir.join("request.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(json!({}));
        b["id"] = json!(dir.file_name().unwrap().to_string_lossy());
        b["family"] = req["family"].clone();
        b["kind"] = req["kind"].clone();
        b["type"] = req["layout_type"]["id"].clone();
        match read_folder(&codex, &dir, &b) {
            Ok(name) => println!("ready: {name}"),
            Err(e) => println!("failed: {e}"),
        }
        if let Ok(t) = std::fs::read_to_string(dir.join("blueprint.json")) {
            let bp: super::super::Blueprint = serde_json::from_str(&t).unwrap();
            println!("{}", serde_json::to_string(&bp.summary()).unwrap());
            println!("problems: {:?}", bp.problems());
        }
    }
}
