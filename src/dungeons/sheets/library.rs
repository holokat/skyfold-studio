//! The sheets a dungeon can be made from: the bundled library (`assets/dungeon_sheets/`, one
//! folder per sheet with `sheet.jpg` and `blueprint.json`, listed in `index.json`) and the
//! world's own custom sheets (`<world>/dungeon_sheets/<id>/`, made by `generate` or imported).
//! Onboarding picks from here (`pick`): a sheet of the dungeon's setting and size that the world
//! hasn't used yet.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::blueprint::{Blueprint, Kind};

/// A dungeon's sheet: a library id, or a custom sheet in the world's folder.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SheetRef {
    pub id: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom: bool,
    /// its setting family (temperate, arid…): the realm's sky and biome
    #[serde(default)]
    pub family: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub family: String,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub layout_type: String,
    pub size: [f32; 2],
    pub zones: u8,
    #[serde(default)]
    pub bosses: u8,
}

pub fn dir() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("dungeon_sheets")
}

/// The bundled library (empty when it isn't installed).
pub fn index() -> &'static Vec<Entry> {
    static I: OnceLock<Vec<Entry>> = OnceLock::new();
    I.get_or_init(|| {
        std::fs::read_to_string(dir().join("index.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    })
}

pub fn entry(id: &str) -> Option<&'static Entry> {
    index().iter().find(|e| e.id == id)
}

/// The user's own sheets (painted for their worlds by `generate`, or imported), shared by all
/// their worlds.
pub fn user_dir() -> PathBuf {
    crate::io::worlds_dir().join("dungeon_sheets")
}

/// The folder a world keeps sheets imported into it.
pub fn custom_dir(world: &Path) -> PathBuf {
    world.join("dungeon_sheets")
}

/// Where a sheet's files are: the library's, the user's own, or the world's.
pub fn folder(r: &SheetRef, world: Option<&Path>) -> Option<PathBuf> {
    if !r.custom {
        return Some(dir().join(&r.id));
    }
    let mine = user_dir().join(&r.id);
    if mine.join("blueprint.json").exists() {
        return Some(mine);
    }
    world
        .map(|w| custom_dir(w).join(&r.id))
        .filter(|f| f.join("blueprint.json").exists())
        .or(Some(mine))
}

/// The user's own sheets that are ready (id, blueprint name, family).
pub fn user_sheets() -> Vec<(String, String, String)> {
    let mut v = vec![];
    let Ok(rd) = std::fs::read_dir(user_dir()) else {
        return v;
    };
    for e in rd.flatten() {
        let p = e.path().join("blueprint.json");
        let Some(bp) = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| serde_json::from_str::<Blueprint>(&t).ok())
        else {
            continue;
        };
        if bp.problems().is_empty() {
            v.push((
                e.file_name().to_string_lossy().to_string(),
                bp.name.clone(),
                bp.family.clone(),
            ));
        }
    }
    v.sort();
    v
}

/// A sheet's blueprint and its picture (when there is one).
pub fn load(r: &SheetRef, world: Option<&Path>) -> Result<(Blueprint, Option<PathBuf>), String> {
    let f = folder(r, world)
        .ok_or_else(|| format!("the custom sheet `{}` needs a saved world", r.id))?;
    let text = std::fs::read_to_string(f.join("blueprint.json"))
        .map_err(|e| format!("sheet `{}`: no blueprint ({e})", r.id))?;
    let bp: Blueprint = serde_json::from_str(&text)
        .map_err(|e| format!("sheet `{}`: bad blueprint ({e})", r.id))?;
    Ok((bp, picture(&f)))
}

/// The sheet's picture in a sheet folder.
pub fn picture(f: &Path) -> Option<PathBuf> {
    ["sheet.jpg", "sheet.png", "sheet.jpeg"]
        .iter()
        .map(|n| f.join(n))
        .find(|p| p.exists())
}

/// A library sheet for a dungeon: its family and kind, not in `used`; else any kind of its
/// family, else its kind in any family. Deterministic for a seed.
pub fn pick(family: &str, kind: Kind, used: &[String], seed: u32) -> Option<&'static Entry> {
    let fresh = |e: &&Entry| !used.contains(&e.id);
    let tiers: [Box<dyn Fn(&&Entry) -> bool>; 3] = [
        Box::new(|e| e.family == family && e.kind == kind),
        Box::new(|e| e.family == family),
        Box::new(|e| e.kind == kind),
    ];
    for t in tiers.iter() {
        let c: Vec<&Entry> = index().iter().filter(|e| t(e) && fresh(e)).collect();
        if !c.is_empty() {
            return Some(c[(seed as usize) % c.len()]);
        }
    }
    index().first()
}

/// A file-name slug of a name ("The Sundered Aqueduct" → "sundered_aqueduct").
pub fn slug(name: &str) -> String {
    let s: String = name
        .to_ascii_lowercase()
        .trim_start_matches("the ")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    s.split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// The setting family for a world climate and biome name ("arid", "cold"…).
pub fn family_for(climate: &str, biome: &str) -> &'static str {
    let b = biome.to_ascii_lowercase();
    if b.contains("volcan") || b.contains("ash") || b.contains("lava") {
        return "volcanic";
    }
    if b.contains("haunt") || b.contains("twilight") || b.contains("fae") {
        return "twilight";
    }
    if b.contains("desert") || b.contains("savanna") || b.contains("badland") || b.contains("mesa")
    {
        return "arid";
    }
    if b.contains("tundra")
        || b.contains("taiga")
        || b.contains("alpine")
        || b.contains("snow")
        || b.contains("glacier")
    {
        return "cold";
    }
    if b.contains("jungle") || b.contains("tropic") || b.contains("swamp") || b.contains("mangrove")
    {
        return "tropical";
    }
    match climate.to_ascii_lowercase().as_str() {
        "arid" => "arid",
        "cold" => "cold",
        "tropical" => "tropical",
        _ => "temperate",
    }
}

/// `WB_SHEET_LIBRARY=<sheets dir>,<manifest.json>[,<out dir>]`: builds the bundled library from
/// generated sheets. For every brief in the manifest (tools/dungeon_sheets/make_jobs.py) with a
/// sheet `<id>.png` and a reading `<id>.read.json` in the sheets dir (searched recursively), the
/// reading is measured into a blueprint; the ones that can be built are written to
/// `<out>/<id>/{sheet.jpg, blueprint.json}` (default `assets/dungeon_sheets`) with `index.json`.
#[cfg(test)]
mod build_library {
    use super::*;
    use std::collections::HashMap;

    fn find(dir: &Path, name: &str, out: &mut HashMap<String, PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                find(&p, name, out);
            } else if let Some(f) = p.file_name().and_then(|f| f.to_str()) {
                if f.ends_with(name) && !f.starts_with("work-") {
                    out.insert(f.trim_end_matches(name).to_string(), p.clone());
                }
            }
        }
    }

    #[test]
    fn build_library() {
        let Ok(spec) = std::env::var("WB_SHEET_LIBRARY") else {
            return;
        };
        let v: Vec<&str> = spec.split(',').map(str::trim).collect();
        let src = Path::new(v[0]);
        let manifest: Vec<serde_json::Value> =
            serde_json::from_str(&std::fs::read_to_string(v[1]).unwrap()).unwrap();
        let out = v.get(2).map(PathBuf::from).unwrap_or_else(dir);
        let mut pngs = HashMap::new();
        let mut reads = HashMap::new();
        find(src, ".png", &mut pngs);
        find(src, ".read.json", &mut reads);
        // (entries already in the library that this run doesn't rebuild are kept)
        let ids: Vec<&str> = manifest.iter().filter_map(|b| b["id"].as_str()).collect();
        let mut index: Vec<Entry> = std::fs::read_to_string(out.join("index.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Entry>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !ids.contains(&e.id.as_str()))
            .collect();
        for b in &manifest {
            let id = b["id"].as_str().unwrap();
            let (Some(png), Some(rd)) = (pngs.get(id), reads.get(id)) else {
                println!("{id}: no sheet or no reading");
                continue;
            };
            let reading = match super::super::read::parse(&std::fs::read_to_string(rd).unwrap()) {
                Ok(r) => r,
                Err(e) => {
                    println!("{id}: bad reading: {e}");
                    continue;
                }
            };
            let img = image::open(png).unwrap();
            let (bp, notes) =
                match super::super::read::normalize(&reading, Some(&img.to_rgba8()), Some(b)) {
                    Ok(x) => x,
                    Err(e) => {
                        println!("{id}: {e}");
                        continue;
                    }
                };
            let bad = bp.problems();
            if !bad.is_empty() {
                println!("{id}: can't be built: {}", bad.join("; "));
                continue;
            }
            let d = out.join(id);
            std::fs::create_dir_all(&d).unwrap();
            let rgb = img.to_rgb8();
            let mut f =
                std::io::BufWriter::new(std::fs::File::create(d.join("sheet.jpg")).unwrap());
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut f, 84)
                .encode_image(&rgb)
                .unwrap();
            std::fs::write(
                d.join("blueprint.json"),
                serde_json::to_string_pretty(&bp).unwrap(),
            )
            .unwrap();
            println!(
                "{id}: ok ({} zones, {} links, {} notes)",
                bp.zones.len(),
                bp.links.len(),
                notes.len()
            );
            index.push(Entry {
                id: id.into(),
                name: bp.name.clone(),
                family: bp.family.clone(),
                kind: bp.kind,
                layout_type: bp.layout_type.clone(),
                size: bp.size,
                zones: bp.zones.len() as u8,
                bosses: bp.zones.iter().filter(|z| z.boss.is_some()).count() as u8,
            });
        }
        index.sort_by(|a, b| a.id.cmp(&b.id));
        std::fs::write(
            out.join("index.json"),
            serde_json::to_string_pretty(&index).unwrap(),
        )
        .unwrap();
        println!("library: {} sheets in {}", index.len(), out.display());
    }
}
