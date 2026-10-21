//! The user's own art styles: the look as fine-tuned in the Style tab (colour grade, light,
//! haze, lens, ambient occlusion, sun rays), saved under a name to apply to any world later or
//! share as a file. A saved style is a `.skystyle` file (JSON); the library is one such file per
//! style in `worlds/styles/`, so sharing one is copying its file and importing it is adding one.
//!
//! Applying a style copies its values into the world's look (`Look::custom`), so a world keeps
//! its look even where the style file is missing; `Look::saved` only names it in the UI.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::look::{Look, LookParams, LookPreset};

/// File extension of a saved (or shared) style.
pub const EXT: &str = "skystyle";
const FORMAT: &str = "skyfold-style";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedStyle {
    /// Always "skyfold-style" (what an imported file is checked for).
    pub format: String,
    pub version: u32,
    pub name: String,
    /// The built-in style it was made from (its colour swatch in the list).
    pub based_on: LookPreset,
    pub params: LookParams,
}

impl SavedStyle {
    /// The look as it shows now (its strength folded in), under `name`.
    pub fn from_look(name: &str, look: &Look) -> SavedStyle {
        SavedStyle {
            format: FORMAT.into(),
            version: 1,
            name: clean_name(name),
            based_on: look.preset,
            params: look.effective(),
        }
    }

    pub fn apply(&self, look: &mut Look) {
        look.preset = self.based_on;
        look.custom = Some(self.params);
        look.strength = 1.0;
        look.saved = Some(self.name.clone());
    }
}

/// The library folder.
pub fn dir() -> PathBuf {
    crate::io::worlds_dir().join("styles")
}

/// The library, read once and kept in step with every change made here.
static LIBRARY: LazyLock<Mutex<Option<Vec<SavedStyle>>>> = LazyLock::new(Default::default);

fn clean_name(name: &str) -> String {
    let n: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(48)
        .collect();
    if n.is_empty() { "My style".into() } else { n }
}

/// A file name for a style name (letters, digits, spaces, - and _ kept).
pub fn file_name(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}.{EXT}", stem.trim())
}

fn read(path: &Path) -> Result<SavedStyle, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let s: SavedStyle = serde_json::from_str(&text)
        .map_err(|_| format!("{} is not a Skyfold style", path.display()))?;
    if s.format != FORMAT {
        return Err(format!("{} is not a Skyfold style", path.display()));
    }
    Ok(s)
}

fn load() -> Vec<SavedStyle> {
    let mut out: Vec<SavedStyle> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == EXT))
        .filter_map(|p| read(&p).ok())
        .collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

/// The saved styles, by name.
pub fn list() -> Vec<SavedStyle> {
    let mut lib = LIBRARY.lock().unwrap();
    lib.get_or_insert_with(load).clone()
}

pub fn find(name: &str) -> Option<SavedStyle> {
    let n = name.trim().to_lowercase();
    list().into_iter().find(|s| s.name.to_lowercase() == n)
}

/// Saves (or replaces) a style in the library; returns its file.
pub fn save(style: &SavedStyle) -> Result<PathBuf, String> {
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("can't create {}: {e}", d.display()))?;
    // a style of the same name (any case) is replaced
    if let Some(old) = find(&style.name) {
        let _ = std::fs::remove_file(d.join(file_name(&old.name)));
    }
    let path = d.join(file_name(&style.name));
    write(style, &path)?;
    *LIBRARY.lock().unwrap() = None;
    Ok(path)
}

fn write(style: &SavedStyle, path: &Path) -> Result<(), String> {
    let text = serde_json::to_string_pretty(style).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("can't write {}: {e}", path.display()))
}

pub fn delete(name: &str) -> Result<(), String> {
    let s = find(name).ok_or_else(|| format!("no saved style named `{name}`"))?;
    std::fs::remove_file(dir().join(file_name(&s.name))).map_err(|e| e.to_string())?;
    *LIBRARY.lock().unwrap() = None;
    Ok(())
}

/// Writes a saved style to `path` to share it (`.skystyle` added when missing).
pub fn export(name: &str, path: &Path) -> Result<PathBuf, String> {
    let s = find(name).ok_or_else(|| format!("no saved style named `{name}`"))?;
    let mut p = path.to_path_buf();
    if p.extension().is_none_or(|x| x != EXT) {
        p.set_extension(EXT);
    }
    write(&s, &p)?;
    Ok(p)
}

/// Adds a shared style file to the library (a name already taken gets a number).
pub fn import(path: &Path) -> Result<SavedStyle, String> {
    let mut s = read(path)?;
    s.name = clean_name(&s.name);
    if find(&s.name).is_some() {
        let base = s.name.clone();
        s.name = (2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| find(n).is_none())
            .unwrap_or(base);
    }
    save(&s)?;
    Ok(s)
}

/// A free "My style N" name for a new style.
pub fn next_name() -> String {
    (1..)
        .map(|i| format!("My style {i}"))
        .find(|n| find(n).is_none())
        .unwrap_or_else(|| "My style".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_file_round_trips_and_applies() {
        let mut look = Look::default();
        look.select(LookPreset::GoldenHour);
        let s = SavedStyle::from_look("  Dusk / test ", &look);
        assert_eq!(s.name, "Dusk / test");
        assert_eq!(file_name(&s.name), "Dusk _ test.skystyle");
        let back: SavedStyle = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let mut other = Look::default();
        back.apply(&mut other);
        assert!(other.base().approx_eq(&look.effective()));
        assert_eq!(other.saved.as_deref(), Some("Dusk / test"));
        // picking a built-in style leaves the saved one
        other.select(LookPreset::Natural);
        assert_eq!(other.saved, None);
    }
}
