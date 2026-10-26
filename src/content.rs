//! Free content packs that download the first time they're used, to keep the app download small:
//! the city & sci-fi surfaces (terrain grounds and object materials) and the hand-painted
//! surfaces (the painted twin of every texture, `art_style`).
//!
//! `assets/content/packs.json` (shipped with the app, written by `tools/release/content_packs.py`)
//! lists each pack: its folders (laid out like `assets/`), its ZIP's file name, size and SHA-256,
//! and where to download it from. `tools/release/package_macos.py` leaves those folders out of
//! the app; a development checkout has them all, so everything is installed there.
//!
//! A pack downloads in the background (system `curl`, like `market::backend`), is checked against
//! its SHA-256, unpacked with the Marketplace's safe unzip (`market::install::unzip`: no links, no
//! paths out of the folder, no ZIP bombs), and its folders are moved into
//! `crate::paths::packs_dir()`, where bought packs live too: the asset source and
//! `paths::asset_file` read whatever the bundle lacks from there, and an app update never removes
//! it. When one lands, `revision` goes up and the terrain and object-material arrays are read
//! again (`terrain::textures`, `surfaces`), so the new surfaces show at once.
//!
//! `WB_CONTENT_URL` replaces the download address (tests with a local server).
//!
//! MCP: `content_packs` (what is installed, sizes, progress) and `content_download`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex};

use bevy::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool};

pub const CITY: &str = "city_surfaces";
pub const HANDPAINTED: &str = "handpainted_surfaces";

#[derive(Clone, Debug, Deserialize)]
pub struct Pack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub blurb: String,
    #[serde(default)]
    pub version: String,
    /// the ZIP's name under the manifest's `base_url`
    #[serde(default)]
    pub file: String,
    /// the ZIP's size (what the user downloads)
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub sha256: String,
    /// its folders, relative to `assets/`
    pub folders: Vec<String>,
    /// files in those folders the app keeps anyway (each material's small swatch, so pickers
    /// show it before the download)
    #[serde(default)]
    pub keep: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Manifest {
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    packs: Vec<Pack>,
}

static MANIFEST: LazyLock<Manifest> = LazyLock::new(|| {
    let path = PathBuf::from(crate::assets_path()).join("content/packs.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
});

static REVISION: AtomicU32 = AtomicU32::new(0);

/// Goes up each time a pack is installed (the texture arrays compare it to rebuild).
pub fn revision() -> u32 {
    REVISION.load(Ordering::Relaxed)
}

pub fn packs() -> &'static [Pack] {
    &MANIFEST.packs
}

pub fn pack(id: &str) -> Option<&'static Pack> {
    packs().iter().find(|p| p.id == id)
}

/// Whether a pack's folder is on this computer: downloaded, or in the app with more than the
/// files it keeps anyway (a development checkout). Remembered until the next install.
fn folder_present(p: &Pack, folder: &str) -> bool {
    static SEEN: LazyLock<Mutex<(u32, HashMap<String, bool>)>> = LazyLock::new(Default::default);
    let rev = revision();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.0 != rev {
        *seen = (rev, HashMap::new());
    }
    if let Some(v) = seen.1.get(folder) {
        return *v;
    }
    let bundled = PathBuf::from(crate::assets_path()).join(folder);
    let v = crate::paths::packs_dir().join(folder).is_dir()
        || std::fs::read_dir(&bundled).is_ok_and(|d| {
            d.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                !p.keep.contains(&name) && !name.starts_with('.')
            })
        });
    seen.1.insert(folder.to_string(), v);
    v
}

/// Whether every folder of the pack is on this computer (in the app or downloaded).
pub fn installed(id: &str) -> bool {
    pack(id).is_none_or(|p| p.folders.iter().all(|f| folder_present(p, f)))
}

/// The pack an assets-relative path ("textures/terrain_library/city_x/albedo.png") belongs to.
pub fn pack_of(rel: &str) -> Option<&'static Pack> {
    packs().iter().find(|p| {
        p.folders
            .iter()
            .any(|f| rel == f || rel.starts_with(&format!("{f}/")))
    })
}

/// Whether the files under an assets-relative path are here; false only inside a pack's folder
/// that hasn't been downloaded yet.
pub fn available(rel: &str) -> bool {
    pack_of(rel).is_none_or(|p| {
        p.folders
            .iter()
            .filter(|f| rel == f.as_str() || rel.starts_with(&format!("{f}/")))
            .all(|f| folder_present(p, f))
    })
}

/// "470 MB"
pub fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", (bytes as f64 / (1u64 << 20) as f64).round().max(1.0))
    }
}

/// Where a download stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// bytes so far, of the total
    Downloading(u64, u64),
    Installing,
    Failed(String),
}

static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(Default::default);

pub fn job(id: &str) -> Option<Job> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
}

fn set_job(id: &str, j: Option<Job>) {
    let mut jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    match j {
        Some(j) => {
            jobs.insert(id.to_string(), j);
        }
        None => {
            jobs.remove(id);
        }
    }
}

/// Packs that finished since the last frame (a toast each).
static DONE: LazyLock<Mutex<Vec<String>>> = LazyLock::new(Default::default);

/// One line for the UI: "Downloading… 34 %", "Installing…", the error, or "".
pub fn status_text(id: &str) -> String {
    match job(id) {
        Some(Job::Downloading(done, total)) if total > 0 => format!(
            "Downloading… {} % of {}",
            (done * 100 / total).min(99),
            size_text(total)
        ),
        Some(Job::Downloading(..)) => "Downloading…".into(),
        Some(Job::Installing) => "Installing…".into(),
        Some(Job::Failed(e)) => format!("Download failed: {e}"),
        None => String::new(),
    }
}

pub fn busy(id: &str) -> bool {
    matches!(job(id), Some(Job::Downloading(..) | Job::Installing))
}

fn url_of(p: &Pack) -> String {
    let base = std::env::var("WB_CONTENT_URL").unwrap_or_else(|_| MANIFEST.base_url.clone());
    format!("{}/{}", base.trim_end_matches('/'), p.file)
}

/// Starts downloading a pack in the background (nothing happens when it is installed or already
/// on its way). Returns what happens, for a toast or an MCP reply.
pub fn download(id: &str) -> Result<String, String> {
    let p = pack(id).ok_or_else(|| format!("no content pack `{id}`"))?;
    if installed(id) {
        return Ok(format!("{} is already installed.", p.name));
    }
    if busy(id) {
        return Ok(format!("{}: {}", p.name, status_text(id)));
    }
    if p.file.is_empty() {
        return Err(format!("{} has no download yet", p.name));
    }
    set_job(id, Some(Job::Downloading(0, p.bytes)));
    let reply = format!(
        "Downloading {} ({}): it shows as soon as it is installed.",
        p.name,
        size_text(p.bytes)
    );
    let p = p.clone();
    std::thread::spawn(move || {
        let r = fetch_and_install(&p, &crate::paths::packs_dir());
        match r {
            Ok(()) => {
                REVISION.fetch_add(1, Ordering::Relaxed);
                set_job(&p.id, None);
                DONE.lock().unwrap_or_else(|e| e.into_inner()).push(p.id.clone());
            }
            Err(e) => set_job(&p.id, Some(Job::Failed(e))),
        }
    });
    Ok(reply)
}

fn curl() -> &'static str {
    if Path::new("/usr/bin/curl").is_file() {
        "/usr/bin/curl"
    } else {
        "curl"
    }
}

fn fetch_and_install(p: &Pack, packs: &Path) -> Result<(), String> {
    let work = packs.join(".incoming").join(&p.id);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let r = (|| {
        let zip = work.join("download.zip");
        // the address goes through stdin (`-K -`), like market::backend
        let mut child: Child = Command::new(curl())
            .args(["-sS", "-f", "-L", "--retry", "2", "-K", "-", "-o"])
            .arg(&zip)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let _ = writeln!(stdin, "url = \"{}\"", url_of(p).replace('"', "%22"));
        }
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    let mut err = String::new();
                    if let Some(mut s) = child.stderr.take() {
                        use std::io::Read;
                        let _ = s.read_to_string(&mut err);
                    }
                    return Err(match err.trim() {
                        "" => "the download didn't finish (are you online?)".to_string(),
                        e => format!("the download didn't finish ({e})"),
                    });
                }
                break;
            }
            let done = std::fs::metadata(&zip).map(|m| m.len()).unwrap_or(0);
            set_job(&p.id, Some(Job::Downloading(done, p.bytes)));
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        set_job(&p.id, Some(Job::Installing));
        if !p.sha256.is_empty() {
            let sum = sha256(&zip)?;
            if !sum.eq_ignore_ascii_case(&p.sha256) {
                return Err("the download is damaged (its checksum doesn't match); try again".into());
            }
        }
        install_zip(&zip, &work.join("staging"), p, packs)
    })();
    let _ = std::fs::remove_dir_all(&work);
    r
}

fn sha256(file: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(file)
        .output()
        .map_err(|e| format!("shasum: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .filter(|s| s.len() == 64)
        .ok_or_else(|| "couldn't check the download".to_string())
}

/// Unpacks a pack's ZIP into `staging`, checks every file is inside one of its folders, then
/// moves each folder into `packs` (a folder that is there already is replaced whole).
fn install_zip(zip: &Path, staging: &Path, p: &Pack, packs: &Path) -> Result<(), String> {
    crate::market::install::unzip(zip, staging)?;
    let prefixes: Vec<String> = p.folders.iter().map(|f| format!("{f}/")).collect();
    if let Some(bad) = crate::market::install::files_under(staging)?
        .into_iter()
        .find(|f| !prefixes.iter().any(|pre| f.starts_with(pre)))
    {
        return Err(format!("the download has a file outside the pack ({bad})"));
    }
    for f in &p.folders {
        let new = staging.join(f);
        if !new.is_dir() {
            return Err(format!("the download is missing {f}"));
        }
        let target = packs.join(f);
        std::fs::create_dir_all(target.parent().unwrap_or(packs)).map_err(|e| e.to_string())?;
        if target.exists() {
            std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&new, &target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub struct ContentPlugin;

impl Plugin for ContentPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (announce, fetch_chosen));
        app.mcp_tool(McpTool {
            name: "content_packs",
            description: "The free content packs that download the first time they're used, to keep the app small: city_surfaces (City & sci-fi grounds and object materials) and handpainted_surfaces (the painted textures a Hand-painted world uses). Each one's name, download size, whether it is installed, and the download's progress. Picking one of their surfaces, switching a world to Hand-painted, or content_download fetches them.",
            input_schema: || json!({ "type": "object", "properties": {} }),
            run: |_w, _a| {
                let list: Vec<Value> = packs()
                    .iter()
                    .map(|p| json!({
                        "id": p.id, "name": p.name, "about": p.blurb, "version": p.version,
                        "download": size_text(p.bytes),
                        "installed": installed(&p.id), "status": status_text(&p.id),
                    }))
                    .collect();
                Ok(vec![McpContent::Json(json!({ "packs": list }))])
            },
        });
        app.mcp_tool(McpTool {
            name: "content_download",
            description: "Downloads one of the free content packs (content_packs) in the background; content_packs shows its progress. Its surfaces show as soon as it is installed.",
            input_schema: || json!({ "type": "object", "properties": {
                "pack": { "type": "string", "enum": [CITY, HANDPAINTED] }
            }, "required": ["pack"] }),
            run: |_w, a| {
                let id = a.get("pack").and_then(Value::as_str).ok_or("`pack` is required")?;
                Ok(vec![McpContent::Text(download(id)?)])
            },
        });
    }
}

/// A ground or building material from a pack this computer doesn't have yet (picked in the UI or
/// over MCP, or in a world that was opened) downloads its pack; until it lands the layer shows
/// its built-in texture. A failed download waits for the user to try again.
fn fetch_chosen(
    ground: Res<crate::terrain::textures::TerrainLayerChoices>,
    parts: Res<crate::surfaces::SurfaceChoices>,
) {
    if !ground.is_changed() && !parts.is_changed() {
        return;
    }
    let ground = ground
        .ids
        .iter()
        .map(|id| format!("textures/terrain_library/{id}"));
    let parts = crate::surfaces::Slot::ALL
        .iter()
        .map(|s| format!("textures/materials/{}", parts.get(*s)));
    for rel in ground.chain(parts) {
        if rel.ends_with('/') {
            continue;
        }
        if let Some(p) = pack_of(&rel)
            && job(&p.id).is_none()
            && !available(&rel)
        {
            let _ = download(&p.id);
        }
    }
}

/// A toast when a pack has landed (and the editor's arrays rebuild through `revision`).
fn announce(mut ed: ResMut<crate::editor::Editor>, mut seen: Local<Vec<(String, String)>>) {
    let done = std::mem::take(&mut *DONE.lock().unwrap_or_else(|e| e.into_inner()));
    for id in done {
        if let Some(p) = pack(&id) {
            ed.toast(format!("{} installed", p.name));
        }
    }
    // a failure is shown once
    for p in packs() {
        if let Some(Job::Failed(e)) = job(&p.id) {
            let entry = (p.id.clone(), e.clone());
            if !seen.contains(&entry) {
                ed.toast(format!("{}: download failed ({e})", p.name));
                seen.push(entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_with(folders: &[&str]) -> Pack {
        Pack {
            id: "test".into(),
            name: "Test".into(),
            blurb: String::new(),
            version: "1".into(),
            file: "test.zip".into(),
            bytes: 0,
            sha256: String::new(),
            folders: folders.iter().map(|s| s.to_string()).collect(),
            keep: vec![],
        }
    }

    fn zip_of(dir: &Path, zip: &Path) {
        let ok = Command::new("/usr/bin/zip")
            .args(["-q", "-r"])
            .arg(zip)
            .arg(".")
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    #[test]
    fn a_pack_installs_into_the_packs_folder_and_refuses_strays() {
        let root = std::env::temp_dir().join(format!("skyfold-content-{}", fastrand::u64(..)));
        let src = root.join("src");
        let f = src.join("textures/terrain_library/city_a");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("albedo.png"), b"png").unwrap();
        let zip = root.join("pack.zip");
        zip_of(&src, &zip);
        let packs = root.join("packs");
        let p = pack_with(&["textures/terrain_library/city_a"]);
        install_zip(&zip, &root.join("staging"), &p, &packs).unwrap();
        assert!(packs.join("textures/terrain_library/city_a/albedo.png").is_file());
        // installing again replaces the folder
        install_zip(&zip, &root.join("staging2"), &p, &packs).unwrap();
        // a file outside the pack's folders is refused, and nothing is installed
        std::fs::write(src.join("textures/evil.png"), b"x").unwrap();
        let zip2 = root.join("pack2.zip");
        zip_of(&src, &zip2);
        let other = root.join("packs2");
        assert!(install_zip(&zip2, &root.join("staging3"), &p, &other).is_err());
        assert!(!other.join("textures/terrain_library/city_a").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_text(470 << 20), "470 MB");
        assert_eq!(size_text(3 << 29), "1.5 GB");
    }
}
