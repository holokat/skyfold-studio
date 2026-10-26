//! A saved scene on disk: `scenes/<id>/scene.json` (the pieces, laid out round the scene's
//! middle), `thumb.png` (rendered by the thumbnail studio, `Subject::Scene`) and `models/` (the
//! user's own imported models it uses, so it works in any world and on any computer). Models from
//! the bundled libraries and asset packs are named by id, not copied: a scene shared or sold
//! needs its packs (`requirements`).
//!
//! A scene file to share is the folder zipped (`<name>.zip`, Export or the marketplace); opening
//! one unpacks it into the library (`import_zip`).

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::objects::{Library, ObjectStore, Placed};
use crate::terrain::Terrain;

pub const FORMAT: &str = "skyfold-scene";

/// Where the library of saved scenes lives (`WB_SCENES_DIR`; automated runs use a scratch
/// folder so tests never touch the user's scenes).
pub fn dir() -> PathBuf {
    if let Ok(d) = std::env::var("WB_SCENES_DIR") {
        return PathBuf::from(d);
    }
    if std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok() || cfg!(test) {
        return std::env::temp_dir().join(format!("skyfold-test-scenes-{}", std::process::id()));
    }
    crate::paths::scenes_dir()
}

pub fn folder(id: &str) -> PathBuf {
    dir().join(id)
}

/// One piece of a scene: an asset, where it stands from the scene's middle (x, height above the
/// ground, z), its turn and size.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Piece {
    pub asset: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub variant: u32,
    /// [dx, height above the ground, dz] (m)
    pub at: [f32; 3],
    pub rot: [f32; 4],
    pub scale: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch: Option<[f32; 3]>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// A user model shipped inside the scene (`models/<file>`), and the asset id it had.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Bundled {
    pub asset: String,
    pub file: String,
    /// files a .gltf references (buffers, images), next to it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SceneFile {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub created: u64,
    /// the loop it was captured with, round its middle [x, z]
    #[serde(default)]
    pub outline: Vec<[f32; 2]>,
    pub pieces: Vec<Piece>,
    #[serde(default)]
    pub models: Vec<Bundled>,
}

impl SceneFile {
    /// Width and depth (m) of the pieces' spread (or the outline's).
    pub fn size(&self) -> Vec2 {
        let pts = self
            .outline
            .iter()
            .map(|p| Vec2::from_array(*p))
            .chain(self.pieces.iter().map(|p| Vec2::new(p.at[0], p.at[2])));
        let (lo, hi) = pts.fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(lo, hi), p| (lo.min(p), hi.max(p)),
        );
        if lo.x > hi.x { Vec2::ZERO } else { hi - lo }
    }

    /// Distinct assets with how many pieces use each, most first.
    pub fn assets(&self) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = vec![];
        for p in &self.pieces {
            match v.iter_mut().find(|a| a.0 == p.asset) {
                Some(a) => a.1 += 1,
                None => v.push((p.asset.clone(), 1)),
            }
        }
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
}

/// Seconds since 1970.
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A file-safe id from a name, unique in the library.
pub fn new_id(name: &str) -> String {
    let mut slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').chars().take(40).collect::<String>();
    let slug = if slug.is_empty() {
        "scene".to_string()
    } else {
        slug
    };
    let mut id = slug.clone();
    let mut k = 2;
    while folder(&id).exists() {
        id = format!("{slug}-{k}");
        k += 1;
    }
    id
}

/// The objects whose middle lies inside `outline` (world x, z).
pub fn inside(store: &ObjectStore, outline: &[Vec2]) -> Vec<u64> {
    let mut ids: Vec<u64> = store
        .items
        .values()
        .filter(|p| crate::setpieces::poly_sdf(outline, Vec2::new(p.pos[0], p.pos[2])) < 0.0)
        .map(|p| p.id)
        .collect();
    ids.sort_unstable();
    ids
}

/// The middle of a set of objects (x, z): their footprints' bounding box centre.
fn middle(objs: &[&Placed]) -> Vec2 {
    let (lo, hi) = objs.iter().map(|p| Vec2::new(p.pos[0], p.pos[2])).fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    );
    (lo + hi) * 0.5
}

/// A scene made of objects `ids` (not yet saved): each piece relative to their middle, its height
/// kept above the ground under it. `outline` (world) defaults to a box round the pieces.
pub fn capture(
    store: &ObjectStore,
    terrain: &Terrain,
    ids: &[u64],
    outline: Option<&[Vec2]>,
    name: &str,
    description: &str,
) -> Result<SceneFile, String> {
    let objs: Vec<&Placed> = ids.iter().filter_map(|id| store.items.get(id)).collect();
    if objs.is_empty() {
        return Err("there are no placed objects in it".into());
    }
    let mid = middle(&objs);
    let pieces = objs
        .iter()
        .map(|p| Piece {
            asset: p.asset.clone(),
            variant: p.variant,
            at: [
                p.pos[0] - mid.x,
                p.pos[1] - terrain.height_at(p.pos[0], p.pos[2]),
                p.pos[2] - mid.y,
            ],
            rot: p.rot,
            scale: p.scale,
            stretch: p.stretch,
        })
        .collect();
    let outline: Vec<[f32; 2]> = match outline {
        Some(o) => o.iter().map(|p| (*p - mid).to_array()).collect(),
        None => vec![],
    };
    let name = if name.trim().is_empty() {
        "Untitled scene".to_string()
    } else {
        name.trim().to_string()
    };
    Ok(SceneFile {
        format: FORMAT.into(),
        version: 1,
        id: String::new(),
        name,
        description: description.trim().to_string(),
        created: now(),
        outline,
        pieces,
        models: vec![],
    })
}

/// Saves `scene` as a new scene in the library (its user models copied in); returns its id.
pub fn save_new(mut scene: SceneFile, lib: &Library) -> Result<String, String> {
    scene.id = new_id(&scene.name);
    let dir = folder(&scene.id);
    std::fs::create_dir_all(dir.join("models")).map_err(|e| e.to_string())?;
    // the user's imported models travel with it
    let mut models = vec![];
    for (asset, _) in scene.assets() {
        let Some(e) = lib
            .get(&asset)
            .filter(|e| e.category == crate::objects::Category::Imported)
        else {
            continue;
        };
        let Some(rel) = &e.import_path else { continue };
        let src = crate::asset_file(rel);
        let file = src
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .ok_or("an imported model without a file name")?;
        std::fs::copy(&src, dir.join("models").join(&file))
            .map_err(|e| format!("couldn't copy {file}: {e}"))?;
        let extra = gltf_extras(&src);
        for x in &extra {
            let to = dir.join("models").join(x);
            if let Some(p) = to.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::copy(src.parent().unwrap_or(Path::new(".")).join(x), to);
        }
        models.push(Bundled { asset, file, extra });
    }
    scene.models = models;
    write(&scene)?;
    Ok(scene.id)
}

/// The buffers and images a .gltf names beside it (none for a .glb).
fn gltf_extras(src: &Path) -> Vec<String> {
    if !src
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gltf"))
    {
        return vec![];
    }
    let Ok(text) = std::fs::read_to_string(src) else {
        return vec![];
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    ["buffers", "images"]
        .iter()
        .filter_map(|k| json.get(*k).and_then(|v| v.as_array()))
        .flatten()
        .filter_map(|v| v.get("uri").and_then(|u| u.as_str()))
        .filter(|u| !u.starts_with("data:") && !u.contains(".."))
        .map(str::to_string)
        .collect()
}

pub fn write(scene: &SceneFile) -> Result<(), String> {
    let dir = folder(&scene.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("scene.json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(scene).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join("scene.json")).map_err(|e| e.to_string())
}

pub fn read(id: &str) -> Option<SceneFile> {
    read_file(&folder(id).join("scene.json"))
}

fn read_file(path: &Path) -> Option<SceneFile> {
    let s: SceneFile = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (s.format == FORMAT).then_some(s)
}

/// Every saved scene, newest first.
pub fn list() -> Vec<SceneFile> {
    let mut v: Vec<SceneFile> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| read_file(&e.path().join("scene.json")))
        .collect();
    v.sort_by(|a, b| b.created.cmp(&a.created).then(a.name.cmp(&b.name)));
    v
}

/// The scene file's date and size, for the thumbnail cache key.
pub fn stamp(id: &str) -> Vec<u8> {
    let Ok(m) = std::fs::metadata(folder(id).join("scene.json")) else {
        return vec![];
    };
    let t = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    [t.to_le_bytes().as_slice(), &m.len().to_le_bytes()].concat()
}

pub fn delete(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
        return Err("not a scene".into());
    }
    let d = folder(id);
    if !d.join("scene.json").is_file() {
        return Err(format!("no scene `{id}`"));
    }
    std::fs::remove_dir_all(d).map_err(|e| e.to_string())
}

/// Makes sure a scene's own models are in this project (`assets/imported/`) and returns the
/// asset id each bundled model has here (a different file of the same name gets a new name).
pub fn install_models(
    scene: &SceneFile,
) -> Result<Vec<(String, crate::io::ImportedAsset)>, String> {
    let src_dir = folder(&scene.id).join("models");
    let dst_dir = crate::io::assets_dir().join("imported");
    std::fs::create_dir_all(&dst_dir).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for b in &scene.models {
        let src = src_dir.join(&b.file);
        let bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", b.file))?;
        // the same name: reuse it if it's the same model, else a new name
        let (stem, ext) = match b.file.rsplit_once('.') {
            Some((s, e)) => (s.to_string(), format!(".{e}")),
            None => (b.file.clone(), String::new()),
        };
        let mut name = b.file.clone();
        let mut k = 2;
        while let Ok(have) = std::fs::read(dst_dir.join(&name)) {
            if have == bytes {
                break;
            }
            name = format!("{stem}-{k}{ext}");
            k += 1;
        }
        let dst = dst_dir.join(&name);
        if !dst.exists() {
            std::fs::write(&dst, &bytes).map_err(|e| e.to_string())?;
            for x in &b.extra {
                let to = dst_dir.join(x);
                if let Some(p) = to.parent() {
                    let _ = std::fs::create_dir_all(p);
                }
                if !to.exists() {
                    let _ = std::fs::copy(src_dir.join(x), to);
                }
            }
        }
        let pretty = stem.replace(['_', '-'], " ");
        let imp = crate::io::ImportedAsset {
            id: format!("imported/{name}"),
            name: pretty.clone(),
            path: format!("imported/{name}"),
            surface: crate::terrain::layers::Surface::guess_from_name(&pretty),
            walkable: true,
            radius: 4.0,
        };
        out.push((b.asset.clone(), imp));
    }
    Ok(out)
}

/// Zips a saved scene to `to` (a `.zip`).
pub fn export_zip(id: &str, to: &Path) -> Result<u64, String> {
    let src = folder(id);
    if !src.join("scene.json").is_file() {
        return Err(format!("no scene `{id}`"));
    }
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(to);
    let ok = std::process::Command::new("/usr/bin/zip")
        .current_dir(&src)
        .arg("-qr")
        .arg(to)
        .args(["scene.json", "models"])
        .arg("thumb.png")
        .status()
        .map(|s| s.code() == Some(0) || s.code() == Some(12))
        .unwrap_or(false);
    // (zip skips a missing thumb.png with a warning)
    if !ok || !to.is_file() {
        // without the thumbnail
        let ok = std::process::Command::new("/usr/bin/zip")
            .current_dir(&src)
            .arg("-qr")
            .arg(to)
            .args(["scene.json", "models"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err("couldn't write the zip".into());
        }
    }
    Ok(std::fs::metadata(to).map(|m| m.len()).unwrap_or(0))
}

/// A scene folder unpacked from a download or a zip (its `scene.json` at the top, or one level
/// down) added to the library under a new id. Returns the id.
pub fn import_folder(unpacked: &Path) -> Result<String, String> {
    let root = std::iter::once(unpacked.to_path_buf())
        .chain(
            std::fs::read_dir(unpacked)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir()),
        )
        .find(|d| d.join("scene.json").is_file())
        .ok_or("it isn't a Skyfold scene (no scene.json)")?;
    let mut scene =
        read_file(&root.join("scene.json")).ok_or("its scene.json isn't a Skyfold scene")?;
    scene.id = new_id(&scene.name);
    let dst = folder(&scene.id);
    std::fs::create_dir_all(dst.join("models")).map_err(|e| e.to_string())?;
    for b in &scene.models {
        for f in std::iter::once(&b.file).chain(&b.extra) {
            if f.contains("..") || f.starts_with('/') {
                return Err("the scene names a model outside itself".into());
            }
            let to = dst.join("models").join(f);
            if let Some(p) = to.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            std::fs::copy(root.join("models").join(f), &to).map_err(|e| format!("{f}: {e}"))?;
        }
    }
    let _ = std::fs::copy(root.join("thumb.png"), dst.join("thumb.png"));
    write(&scene)?;
    Ok(scene.id)
}

/// Opens a scene `.zip` into the library.
pub fn import_zip(zip: &Path) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!("skyfold-scene-{}", fastrand::u64(..)));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let ok = std::process::Command::new("/usr/bin/unzip")
        .args(["-q", "-o"])
        .arg(zip)
        .arg("-d")
        .arg(&tmp)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let r = if ok {
        import_folder(&tmp)
    } else {
        Err("couldn't open the zip".into())
    };
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_keeps_its_layout_and_travels_as_a_zip() {
        let t = Terrain::new(64, 4.0, -10.0);
        let mut store = ObjectStore::default();
        for (i, (x, z, lift)) in [
            (10.0, 10.0, 0.0),
            (14.0, 10.0, 1.2),
            (12.0, 16.0, 0.0),
            (90.0, 90.0, 0.0),
        ]
        .into_iter()
        .enumerate()
        {
            let y = t.height_at(x, z) + lift;
            store.insert(Placed {
                id: i as u64 + 1,
                asset: "hp:well".into(),
                variant: 0,
                pos: [x, y, z],
                rot: Quat::IDENTITY.to_array(),
                scale: 1.0,
                stretch: None,
                group: 0,
                locked: false,
                glow: None,
            });
        }
        let loop_ = [
            Vec2::new(5.0, 5.0),
            Vec2::new(20.0, 5.0),
            Vec2::new(20.0, 20.0),
            Vec2::new(5.0, 20.0),
        ];
        let ids = inside(&store, &loop_);
        assert_eq!(ids, vec![1, 2, 3], "the far one stays out");
        let s = capture(&store, &t, &ids, Some(&loop_), "  Moonlit Shrine ", "").unwrap();
        // round the middle of the pieces (12, 13), heights kept above the ground
        assert_eq!(s.pieces[0].at, [-2.0, 0.0, -3.0]);
        assert!((s.pieces[1].at[1] - 1.2).abs() < 1e-4);
        assert_eq!(
            (s.name.as_str(), s.size()),
            ("Moonlit Shrine", Vec2::new(15.0, 15.0))
        );
        let lib = Library::empty_for_tests();
        let id = save_new(s.clone(), &lib).unwrap();
        assert_eq!(id, "moonlit-shrine");
        assert_eq!(new_id("Moonlit Shrine"), "moonlit-shrine-2");
        let back = read(&id).unwrap();
        assert_eq!(back.pieces, s.pieces);
        assert!(list().iter().any(|x| x.id == id));
        // shared as a zip, opened again as a copy
        let zip = dir().join("out/shrine.zip");
        assert!(export_zip(&id, &zip).unwrap() > 0);
        let copy = import_zip(&zip).unwrap();
        assert_eq!(copy, "moonlit-shrine-2");
        assert_eq!(read(&copy).unwrap().pieces, s.pieces);
        delete(&copy).unwrap();
        assert!(read(&copy).is_none());
        assert!(delete("../x").is_err());
        let _ = std::fs::remove_dir_all(dir());
    }
}
