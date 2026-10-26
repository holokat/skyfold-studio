//! The image library: every picture lives in its own folder under the user's assets,
//! `images/<id>/` with the image (`image.png` / `image.jpg`) and `meta.json` (prompt, model, seed,
//! notes, tags, favourite, where it came from). Generated and imported pictures are the same kind
//! of entry; nothing else keeps a list, so a folder copied in by hand shows up on the next load.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Image files the library takes in.
pub const EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ImageMeta {
    pub id: String,
    pub name: String,
    /// the image's file name inside its folder
    pub file: String,
    /// "generated" or "imported"
    pub source: String,
    pub prompt: String,
    /// backend id (`codex`, `cloudflare`, `bfl`), empty for imports
    pub backend: String,
    pub model: String,
    pub seed: Option<u64>,
    /// "1:1", "16:9", …
    pub aspect: String,
    pub width: u32,
    pub height: u32,
    /// unix seconds
    pub created: u64,
    pub notes: String,
    pub tags: Vec<String>,
    pub favourite: bool,
    /// the image this one was varied or edited from
    pub parent: Option<String>,
    /// library images handed to the model as references
    pub references: Vec<String>,
    /// "AI-generated · cloudflare/@cf/…", "Imported"
    pub provenance: String,
    /// an import's original file name
    pub original: String,
}

impl ImageMeta {
    pub fn generated(&self) -> bool {
        self.source == "generated"
    }
    /// Lower-case text the search box matches against.
    fn haystack(&self) -> String {
        format!(
            "{} {} {} {} {} {}",
            self.name,
            self.prompt,
            self.notes,
            self.tags.join(" "),
            self.model,
            self.original
        )
        .to_lowercase()
    }
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty() || {
            let h = self.haystack();
            q.split_whitespace().all(|w| h.contains(w))
        }
    }
}

/// `images/` under the user's assets.
pub fn root() -> PathBuf {
    crate::paths::user_assets_dir().join("images")
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A new id: time-ordered, then random ("img_19a2b3c4d5e_3f2a").
pub fn new_id() -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("img_{ms:x}_{:04x}", fastrand::u16(..))
}

/// Ids are folder names: only what `new_id` makes (and nothing that walks out of `images/`).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Tags: trimmed, lower-case, no duplicates, no empties.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in tags {
        let t = t.trim().to_lowercase();
        if !t.is_empty() && t.len() <= 40 && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// "2026-10-25" for unix seconds (UTC).
pub fn date(secs: u64) -> String {
    // days since 1970-01-01 → civil date (Howard Hinnant's algorithm)
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// A name from a prompt: its first few words.
pub fn name_from_prompt(prompt: &str) -> String {
    let words: Vec<&str> = prompt.split_whitespace().take(6).collect();
    let mut n = words.join(" ");
    n = n.trim_end_matches([',', '.', ';', ':']).to_string();
    if n.chars().count() > 48 {
        n = n.chars().take(48).collect::<String>() + "…";
    }
    if n.is_empty() { "Untitled".into() } else { n }
}

/// The image kind of a file by its first bytes ("png", "jpg", "webp").
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

#[derive(Default)]
pub struct Library {
    /// newest first
    pub entries: Vec<ImageMeta>,
    pub loaded: bool,
}

impl Library {
    pub fn ensure(&mut self) {
        if !self.loaded {
            self.reload();
        }
    }

    pub fn reload(&mut self) {
        self.loaded = true;
        self.entries = read_dir(&root());
    }

    pub fn get(&self, id: &str) -> Option<&ImageMeta> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn dir(id: &str) -> PathBuf {
        root().join(id)
    }

    /// The image file of `id`.
    pub fn file(&self, id: &str) -> Option<PathBuf> {
        self.get(id).map(|e| Self::dir(&e.id).join(&e.file))
    }

    /// Every tag in use, with how many images have it (most used first).
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = vec![];
        for e in &self.entries {
            for t in &e.tags {
                match v.iter_mut().find(|(n, _)| n == t) {
                    Some((_, c)) => *c += 1,
                    None => v.push((t.clone(), 1)),
                }
            }
        }
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    /// Adds (or replaces) an entry that's already on disk.
    pub fn insert(&mut self, meta: ImageMeta) {
        self.entries.retain(|e| e.id != meta.id);
        self.entries.insert(0, meta);
        self.entries.sort_by(|a, b| b.created.cmp(&a.created));
    }

    /// Changes an entry and writes its meta.json.
    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut ImageMeta)) -> Result<ImageMeta, String> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("no image {id}"))?;
        f(e);
        e.tags = clean_tags(&e.tags);
        write_meta(e)?;
        Ok(e.clone())
    }

    /// Copies image files into the library.
    pub fn import(&mut self, paths: &[PathBuf]) -> Result<Vec<ImageMeta>, String> {
        let mut out = vec![];
        for p in paths {
            let m = import_file(p)?;
            self.insert(m.clone());
            out.push(m);
        }
        Ok(out)
    }

    /// A copy of an image (its own folder, " copy" on its name, the original as parent).
    pub fn duplicate(&mut self, id: &str) -> Result<ImageMeta, String> {
        let src = self.get(id).cloned().ok_or_else(|| format!("no image {id}"))?;
        let mut m = src.clone();
        m.id = new_id();
        m.name = format!("{} copy", src.name);
        m.created = now_secs();
        m.favourite = false;
        m.parent = Some(src.id.clone());
        let dir = Self::dir(&m.id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::copy(Self::dir(&src.id).join(&src.file), dir.join(&m.file))
            .map_err(|e| e.to_string())?;
        write_meta(&m)?;
        self.insert(m.clone());
        Ok(m)
    }

    /// Removes an image: its folder goes to the Trash (recoverable), else is deleted.
    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        if !valid_id(id) || self.get(id).is_none() {
            return Err(format!("no image {id}"));
        }
        let dir = Self::dir(id);
        if dir.exists() && !trash(&dir) {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        self.entries.retain(|e| e.id != id);
        Ok(())
    }

    /// Shows the image in Finder.
    pub fn reveal(&self, id: &str) {
        if let Some(f) = self.file(id) {
            let _ = std::process::Command::new("/usr/bin/open").arg("-R").arg(f).spawn();
        }
    }
}

/// Moves a folder into the user's Trash (`~/.Trash`, recoverable from there); false when that
/// isn't possible (another volume).
fn trash(dir: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else { return false };
    let bin = PathBuf::from(home).join(".Trash");
    let Some(name) = dir.file_name().map(|n| n.to_string_lossy().to_string()) else { return false };
    let mut dst = bin.join(format!("Skyfold image {name}"));
    let mut n = 2;
    while dst.exists() {
        dst = bin.join(format!("Skyfold image {name} {n}"));
        n += 1;
    }
    bin.is_dir() && std::fs::rename(dir, &dst).is_ok()
}

fn read_dir(root: &Path) -> Vec<ImageMeta> {
    let mut v: Vec<ImageMeta> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|d| {
            let dir = d.path();
            let id = dir.file_name()?.to_str()?.to_string();
            if !valid_id(&id) {
                return None;
            }
            let text = std::fs::read_to_string(dir.join("meta.json")).ok()?;
            let mut m: ImageMeta = serde_json::from_str(&text).ok()?;
            m.id = id;
            dir.join(&m.file).is_file().then_some(m)
        })
        .collect();
    v.sort_by(|a, b| b.created.cmp(&a.created));
    v
}

pub fn write_meta(m: &ImageMeta) -> Result<(), String> {
    if !valid_id(&m.id) {
        return Err(format!("bad image id {}", m.id));
    }
    let dir = Library::dir(&m.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("meta.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(m).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join("meta.json")).map_err(|e| e.to_string())
}

/// The pixel size of an image file (header only).
pub fn dimensions(path: &Path) -> (u32, u32) {
    image::image_dimensions(path).unwrap_or((0, 0))
}

fn import_file(p: &Path) -> Result<ImageMeta, String> {
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let ext = sniff(&bytes).ok_or_else(|| format!("{} isn't a PNG, JPEG or WebP image", p.display()))?;
    let original = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = p
        .file_stem()
        .map(|n| n.to_string_lossy().replace(['_', '-'], " "))
        .unwrap_or_else(|| "Imported".into());
    let mut m = ImageMeta {
        id: new_id(),
        name,
        file: format!("image.{ext}"),
        source: "imported".into(),
        created: now_secs(),
        provenance: "Imported".into(),
        original,
        ..Default::default()
    };
    let dir = Library::dir(&m.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dst = dir.join(&m.file);
    std::fs::write(&dst, &bytes).map_err(|e| e.to_string())?;
    (m.width, m.height) = dimensions(&dst);
    m.aspect = aspect_label(m.width, m.height);
    write_meta(&m)?;
    Ok(m)
}

/// Stores a generated picture as a new library entry (its folder, file and meta.json).
pub fn store_generated(bytes: &[u8], mut m: ImageMeta) -> Result<ImageMeta, String> {
    let ext = sniff(bytes).ok_or("the model didn't return a PNG or JPEG image")?;
    m.id = new_id();
    m.file = format!("image.{ext}");
    m.source = "generated".into();
    if m.created == 0 {
        m.created = now_secs();
    }
    let dir = Library::dir(&m.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dst = dir.join(&m.file);
    std::fs::write(&dst, bytes).map_err(|e| e.to_string())?;
    (m.width, m.height) = dimensions(&dst);
    write_meta(&m)?;
    Ok(m)
}

/// "16:9" for a size (the nearest of the usual ratios; "W×H" otherwise).
pub fn aspect_label(w: u32, h: u32) -> String {
    if w == 0 || h == 0 {
        return String::new();
    }
    let r = w as f32 / h as f32;
    for (label, v) in [
        ("1:1", 1.0),
        ("4:3", 4.0 / 3.0),
        ("3:4", 0.75),
        ("16:9", 16.0 / 9.0),
        ("9:16", 9.0 / 16.0),
        ("3:2", 1.5),
        ("2:3", 2.0 / 3.0),
    ] {
        if (r / v - 1.0).abs() < 0.03 {
            return label.into();
        }
    }
    format!("{w}×{h}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_names() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_792_886_400), "2026-10-25");
        assert_eq!(name_from_prompt("  a misty pine forest at dawn, volumetric light, 35mm "), "a misty pine forest at dawn");
        assert_eq!(name_from_prompt(""), "Untitled");
        assert_eq!(clean_tags(&[" Forest".into(), "forest".into(), "".into(), "Moss".into()]), vec!["forest", "moss"]);
        assert!(valid_id(&new_id()));
        assert!(!valid_id("../x"));
        assert_eq!(aspect_label(1920, 1080), "16:9");
        assert_eq!(aspect_label(1254, 1254), "1:1");
    }

    #[test]
    fn sniffs_and_searches() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0, 0]), Some("png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(sniff(b"{\"x\":1}"), None);
        let m = ImageMeta {
            name: "Mossy rocks".into(),
            prompt: "granite boulders".into(),
            notes: "use for the river bank".into(),
            tags: vec!["rock".into()],
            ..Default::default()
        };
        assert!(m.matches("river rock"));
        assert!(m.matches(""));
        assert!(!m.matches("snow"));
    }
}
