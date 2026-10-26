//! Bought asset packs on this computer: installing a pack's download and reading it back.
//!
//! A first-party pack (the Skyfold Studio Collection) is sold as a skyfoldstudio.com listing
//! whose files are a descriptor, `skyfold-pack.json`, and its folders (`roots`: the model library
//! `models/stylepacks/handpainted` and the painted surfaces `textures/handpainted`) split into ZIP
//! parts (`part-01.zip`…, each under the website's 32 MiB file limit;
//! `tools/release/pack_listing.py` makes them). The website serves a bought listing as one ZIP of
//! those files. (Parts are named `part-NN.skyfoldpart`, ZIPs inside: the website's uploader
//! unpacks anything called `.zip` in the browser.) `install` checks it is the pack that was bought, unpacks every part into a
//! staging folder, checks every file lands inside one of the pack's folders, then swaps each
//! folder in whole.
//!
//! Installs live in `crate::paths::packs_dir()`, laid out like `assets/`, with the user's data
//! (Application Support in the shipped app): an app update or reinstall never touches them. The
//! default asset source reads the bundle first and these installs second (`OverlayReader`),
//! `crate::paths::asset_file` does the same for plain file reads, and
//! `crate::objects::model_libraries` lists installed libraries, so a bought pack's assets use the
//! very same `models/stylepacks/<style>/…` paths as a bundled one: worlds made with them open
//! unchanged everywhere.
//!
//! Ownership is recorded separately, the moment the website says an order is paid
//! (`backend::record_owned`), and never removed; files can always be downloaded again from the
//! account's purchases (`site`).

use std::path::{Path, PathBuf};
use std::process::Command;

use bevy::asset::io::file::FileAssetReader;
use bevy::asset::io::{AssetReader, AssetReaderError, ErasedAssetReader, PathStream, Reader};
use serde::Deserialize;

/// The descriptor at the top of a pack's download.
pub const DESCRIPTOR: &str = "skyfold-pack.json";

/// `skyfold-pack.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct Descriptor {
    pub format: u32,
    /// the catalog pack id, e.g. "skyfold_studio"
    pub pack: String,
    /// its folders, laid out like `assets/`: "models/stylepacks/handpainted" (the model library,
    /// required) and e.g. "textures/handpainted" (its painted surfaces)
    pub roots: Vec<String>,
    #[serde(default)]
    pub version: String,
}

/// A folder a pack may install: a style's model library `models/stylepacks/<style>` or its
/// painted surfaces `textures/<style>`.
fn valid_root(root: &str) -> bool {
    let style = root
        .strip_prefix("models/stylepacks/")
        .or_else(|| root.strip_prefix("textures/"));
    style.is_some_and(|style| {
        !style.is_empty()
            && style
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    })
}

/// The pack's model library (its first `models/stylepacks/…` folder).
fn library_root(d: &Descriptor) -> Option<&str> {
    d.roots
        .iter()
        .map(String::as_str)
        .find(|r| r.starts_with("models/stylepacks/"))
}

/// An archive entry name that stays inside the folder it's unpacked into.
fn safe_entry(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && name.split('/').all(|p| p != "..")
}

/// The most a download may unpack to (a ZIP bomb stops here, before anything is written).
const MAX_UNPACKED: u64 = 4 << 30;

/// Unpacks `zip` into `into`, refusing unsafe names, symbolic links and oversized archives
/// before writing anything (a link made by one part could carry the next part's files out of
/// the pack's folder).
pub(crate) fn unzip(zip: &Path, into: &Path) -> Result<(), String> {
    let zipinfo = |flag: &str| {
        Command::new("/usr/bin/unzip")
            .args(["-Z", flag])
            .arg(zip)
            .output()
            .map_err(|e| format!("unzip: {e}"))
            .and_then(|o| {
                o.status
                    .success()
                    .then(|| String::from_utf8_lossy(&o.stdout).into_owned())
                    .ok_or_else(|| "the download isn't a readable ZIP".to_string())
            })
    };
    let names = zipinfo("-1")?;
    if let Some(bad) = names.lines().find(|n| !safe_entry(n)) {
        return Err(format!("the download has an unsafe file path ({bad})"));
    }
    // long listing: one line per entry starting with its Unix mode ("lrwx…" = a link)
    let long = zipinfo("-l")?;
    if long.lines().any(|l| l.starts_with('l')) {
        return Err("the download has a symbolic link".into());
    }
    let total: u64 = zipinfo("-t")?
        .split(" bytes uncompressed")
        .next()
        .and_then(|s| s.rsplit(' ').next())
        .and_then(|n| n.parse().ok())
        .ok_or("the download isn't a readable ZIP")?;
    if total > MAX_UNPACKED {
        return Err("the download unpacks to more than 4 GB".into());
    }
    let ok = Command::new("/usr/bin/unzip")
        .args(["-q", "-o"])
        .arg(zip)
        .arg("-d")
        .arg(into)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err("couldn't unpack the download".into())
    }
}

/// Every file under `dir` (relative, '/'-separated); refuses symlinks.
pub(crate) fn files_under(dir: &Path) -> Result<Vec<String>, String> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| e.to_string())?.flatten() {
            let t = e.file_type().map_err(|e| e.to_string())?;
            let p = e.path();
            if t.is_symlink() {
                return Err("the download has a symbolic link".into());
            } else if t.is_dir() {
                stack.push(p);
            } else if let Ok(rel) = p.strip_prefix(dir) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Ok(out)
}

/// The descriptor and the folder holding it (the top of the download, or one folder down: the
/// website keeps the uploaded folder's name).
fn find_descriptor(outer: &Path) -> Result<(Descriptor, PathBuf), String> {
    let mut dirs = vec![outer.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(outer) {
        dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for d in dirs {
        let f = d.join(DESCRIPTOR);
        if f.is_file() {
            let desc: Descriptor = serde_json::from_str(
                &std::fs::read_to_string(&f).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("unreadable {DESCRIPTOR}: {e}"))?;
            return Ok((desc, d));
        }
    }
    Err(format!("the download has no {DESCRIPTOR}: it isn't an asset pack"))
}

/// Installs a pack's download (the website's ZIP) under `packs` as the library of `pack`, the
/// pack that was bought. Returns its descriptor. On any problem nothing installed changes.
pub fn install_into(bytes: &[u8], pack: &str, packs: &Path) -> Result<Descriptor, String> {
    let work = packs.join(".incoming").join(pack);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let r = (|| {
        let zip = work.join("download.zip");
        std::fs::write(&zip, bytes).map_err(|e| e.to_string())?;
        let outer = work.join("download");
        unzip(&zip, &outer)?;
        let _ = std::fs::remove_file(&zip);
        let (desc, base) = find_descriptor(&outer)?;
        if desc.format != 1 {
            return Err(format!(
                "this pack needs a newer Skyfold Studio (format {})",
                desc.format
            ));
        }
        if desc.pack != pack {
            return Err(format!(
                "the download is the `{}` pack, not the one bought (`{pack}`)",
                desc.pack
            ));
        }
        if let Some(bad) = desc.roots.iter().find(|r| !valid_root(r)) {
            return Err(format!("the pack's folder `{bad}` isn't allowed"));
        }
        let lib_root = library_root(&desc)
            .ok_or("the pack has no model library")?
            .to_string();
        let mut parts: Vec<PathBuf> = std::fs::read_dir(&base)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| {
                    x.eq_ignore_ascii_case("skyfoldpart") || x.eq_ignore_ascii_case("zip")
                }) && p
                    .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("part-"))
            })
            .collect();
        parts.sort();
        if parts.is_empty() {
            return Err("the download has no pack parts".into());
        }
        let staging = work.join("staging");
        for p in &parts {
            unzip(p, &staging)?;
            // (and nothing a part left behind may turn into a link the next part writes through)
            files_under(&staging)?;
        }
        // everything inside the pack's folders, and its library readable
        let prefixes: Vec<String> = desc.roots.iter().map(|r| format!("{r}/")).collect();
        if let Some(bad) = files_under(&staging)?
            .into_iter()
            .find(|f| !prefixes.iter().any(|p| f.starts_with(p)))
        {
            return Err(format!("the pack has a file outside its folder ({bad})"));
        }
        let lib = staging.join(&lib_root);
        let models: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(lib.join("models.json"))
                .map_err(|_| "the pack has no models.json")?,
        )
        .map_err(|e| format!("the pack's models.json: {e}"))?;
        if models["models"].as_array().is_none_or(|m| m.is_empty()) {
            return Err("the pack's models.json lists no models".into());
        }
        // swap each folder in whole: the old copy moves aside first, and comes back if the
        // move fails
        for (i, root) in desc.roots.iter().enumerate() {
            let new = staging.join(root);
            if !new.is_dir() {
                continue;
            }
            let target = packs.join(root);
            std::fs::create_dir_all(target.parent().unwrap_or(packs))
                .map_err(|e| e.to_string())?;
            let old = work.join(format!("old-{i}"));
            let had = target.exists();
            if had {
                std::fs::rename(&target, &old).map_err(|e| e.to_string())?;
            }
            if let Err(e) = std::fs::rename(&new, &target) {
                if had {
                    let _ = std::fs::rename(&old, &target);
                }
                return Err(e.to_string());
            }
        }
        Ok(desc)
    })();
    let _ = std::fs::remove_dir_all(&work);
    r
}

/// Installs a bought pack in this computer's packs folder and notes it in the prefs.
pub fn install(bytes: &[u8], pack: &str) -> Result<Descriptor, String> {
    let d = install_into(bytes, pack, &crate::paths::packs_dir())?;
    let mut all = crate::prefs::get("market_installed").unwrap_or(serde_json::json!({}));
    if !all.is_object() {
        all = serde_json::json!({});
    }
    all[pack] = serde_json::json!({ "roots": d.roots, "version": d.version, "at": super::backend::now() });
    crate::prefs::set("market_installed", all);
    Ok(d)
}

/// The default asset source: the app's bundled assets, then bought packs (`packs_dir`) for
/// whatever the bundle lacks.
pub struct OverlayReader {
    bundled: Box<dyn ErasedAssetReader>,
    packs: Box<dyn ErasedAssetReader>,
}

impl OverlayReader {
    pub fn new(bundled: &Path, packs: &Path) -> Self {
        OverlayReader {
            bundled: Box::new(FileAssetReader::new(bundled)),
            packs: Box::new(FileAssetReader::new(packs)),
        }
    }
}

impl AssetReader for OverlayReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<Box<dyn Reader + 'a>, AssetReaderError> {
        match self.bundled.read(path).await {
            Err(AssetReaderError::NotFound(_)) => self.packs.read(path).await,
            r => r,
        }
    }
    async fn read_meta<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<dyn Reader + 'a>, AssetReaderError> {
        match self.bundled.read_meta(path).await {
            Err(AssetReaderError::NotFound(_)) => self.packs.read_meta(path).await,
            r => r,
        }
    }
    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        match self.bundled.read_directory(path).await {
            Err(AssetReaderError::NotFound(_)) => self.packs.read_directory(path).await,
            r => r,
        }
    }
    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        match self.bundled.is_directory(path).await {
            Ok(true) => Ok(true),
            _ => Ok(self.packs.is_directory(path).await.unwrap_or(false)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("skyfold-install-{name}-{}", fastrand::u64(..)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn zip_dir(dir: &Path, out: &Path) {
        let ok = Command::new("/usr/bin/zip")
            .args(["-q", "-r"])
            .arg(out)
            .arg(".")
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok);
    }

    /// A download like the website's: the uploaded folder holding the descriptor and parts.
    fn download(root: &Path, pack: &str, lib_root: &str, extra: Option<(&str, &str)>) -> Vec<u8> {
        download_with(root, pack, &[lib_root, "textures/handpainted"], extra)
    }

    fn download_with(
        root: &Path,
        pack: &str,
        roots: &[&str],
        extra: Option<(&str, &str)>,
    ) -> Vec<u8> {
        let lib_root = roots[0];
        let src = root.join("src");
        let lib = src.join(lib_root);
        std::fs::create_dir_all(lib.join("well")).unwrap();
        std::fs::write(
            lib.join("models.json"),
            r#"{"models":[{"id":"well","name":"Well","category":"props","size_m":[1,1,1],"gltf":"well/well.gltf"}]}"#,
        )
        .unwrap();
        std::fs::write(lib.join("well/well.gltf"), "{}").unwrap();
        if roots.contains(&"textures/handpainted") {
            let t = src.join("textures/handpainted/terrain");
            std::fs::create_dir_all(&t).unwrap();
            std::fs::write(t.join("grass_albedo.png"), "png").unwrap();
        }
        if let Some((path, body)) = extra {
            let f = src.join(path);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        let upload = root.join("upload/skyfold_studio_collection");
        std::fs::create_dir_all(&upload).unwrap();
        zip_dir(&src, &upload.join("part-01.zip"));
        std::fs::rename(upload.join("part-01.zip"), upload.join("part-01.skyfoldpart")).unwrap();
        std::fs::write(
            upload.join(DESCRIPTOR),
            format!(
                r#"{{"format":1,"pack":"{pack}","roots":{},"version":"1"}}"#,
                serde_json::json!(roots)
            ),
        )
        .unwrap();
        let out = root.join("download.zip");
        zip_dir(&root.join("upload"), &out);
        std::fs::read(out).unwrap()
    }

    #[test]
    fn installs_a_bought_pack_whole_and_replaces_an_older_copy() {
        let root = tmp("ok");
        let packs = root.join("packs");
        let lib = packs.join("models/stylepacks/handpainted");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("stale.txt"), "old").unwrap();
        let bytes = download(&root, "skyfold_studio", "models/stylepacks/handpainted", None);
        let d = install_into(&bytes, "skyfold_studio", &packs).expect("installs");
        assert_eq!(d.roots, ["models/stylepacks/handpainted", "textures/handpainted"]);
        assert!(lib.join("models.json").is_file() && lib.join("well/well.gltf").is_file());
        assert!(packs.join("textures/handpainted/terrain/grass_albedo.png").is_file());
        assert!(!lib.join("stale.txt").exists(), "the old copy is replaced whole");
        assert!(!packs.join(".incoming/skyfold_studio").exists());
        // the library lists it under its usual prefix
        let libs = crate::objects::model_libraries_in(&root.join("none"), &packs.join("models"));
        let hp = libs.iter().find(|l| l.prefix == "hp").expect("installed library listed");
        assert_eq!(hp.dir, lib);
        assert_eq!(crate::objects::model_library(hp)[0].id, "hp:well");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_the_wrong_pack_and_files_outside_its_folder() {
        let root = tmp("bad");
        let packs = root.join("packs");
        let other = download(&root, "someone_else", "models/stylepacks/handpainted", None);
        assert!(
            install_into(&other, "skyfold_studio", &packs)
                .unwrap_err()
                .contains("not the one bought")
        );
        let root2 = tmp("bad2");
        let sneaky = download(
            &root2,
            "skyfold_studio",
            "models/stylepacks/handpainted",
            Some(("shaders/evil.wesl", "x")),
        );
        assert!(
            install_into(&sneaky, "skyfold_studio", &packs)
                .unwrap_err()
                .contains("outside its folder")
        );
        let root3 = tmp("bad3");
        let wrong_root = download(&root3, "skyfold_studio", "models/../shaders", None);
        assert!(install_into(&wrong_root, "skyfold_studio", &packs).is_err());
        assert!(!packs.join("models").exists(), "nothing installed");
        assert!(!install_into(b"not a zip", "skyfold_studio", &packs).is_ok());
        for r in [root, root2, root3] {
            let _ = std::fs::remove_dir_all(r);
        }
    }

    /// The real listing upload, zipped the way the website serves it:
    /// `SKYFOLD_PACK_DOWNLOAD=<zip> cargo test installs_the_real_pack -- --ignored`
    #[test]
    #[ignore]
    fn installs_the_real_pack() {
        let zip = std::env::var("SKYFOLD_PACK_DOWNLOAD").expect("SKYFOLD_PACK_DOWNLOAD");
        let root = tmp("real");
        let packs = root.join("packs");
        let d = install_into(&std::fs::read(zip).unwrap(), "skyfold_studio", &packs).unwrap();
        let libs = crate::objects::model_libraries_in(&root.join("none"), &packs.join("models"));
        let hp = libs.iter().find(|l| l.prefix == "hp").expect("library listed");
        let n = crate::objects::model_library(hp).len();
        let textures = files_under(&packs.join("textures/handpainted")).unwrap().len();
        eprintln!("installed {:?}: {n} models, {textures} painted texture files", d.roots);
        assert!(n >= 500 && textures >= 700);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_link_in_one_part_cant_carry_the_next_part_outside_the_pack() {
        let root = tmp("link");
        let packs = root.join("packs");
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let lib = "models/stylepacks/handpainted";
        // part 1: a library plus `lib/l -> <outside>`; part 2: `lib/l/evil.txt`
        let p1 = root.join("p1");
        std::fs::create_dir_all(p1.join(lib)).unwrap();
        std::fs::write(p1.join(lib).join("models.json"), r#"{"models":[]}"#).unwrap();
        std::os::unix::fs::symlink(&outside, p1.join(lib).join("l")).unwrap();
        let p2 = root.join("p2");
        std::fs::create_dir_all(p2.join(lib).join("l")).unwrap();
        std::fs::write(p2.join(lib).join("l/evil.txt"), "x").unwrap();
        let upload = root.join("upload/pack");
        std::fs::create_dir_all(&upload).unwrap();
        for (dir, part) in [(&p1, "part-01.zip"), (&p2, "part-02.zip")] {
            let ok = Command::new("/usr/bin/zip")
                .args(["-q", "-r", "-y"]) // -y: store the link as a link
                .arg(upload.join(part))
                .arg(".")
                .current_dir(dir)
                .status()
                .unwrap()
                .success();
            assert!(ok);
        }
        std::fs::write(
            upload.join(DESCRIPTOR),
            format!(r#"{{"format":1,"pack":"skyfold_studio","roots":["{lib}"],"version":"1"}}"#),
        )
        .unwrap();
        zip_dir(&root.join("upload"), &root.join("download.zip"));
        let err = install_into(&std::fs::read(root.join("download.zip")).unwrap(), "skyfold_studio", &packs).unwrap_err();
        assert!(err.contains("symbolic link"), "{err}");
        assert!(!outside.join("evil.txt").exists(), "nothing written outside the pack");
        assert!(!packs.join(lib).exists(), "nothing installed");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn entry_names_and_roots_are_checked() {
        assert!(safe_entry("models/stylepacks/handpainted/a.gltf"));
        for bad in ["../x", "a/../../x", "/etc/passwd", "a\\b", "C:x", ""] {
            assert!(!safe_entry(bad), "{bad}");
        }
        assert!(valid_root("models/stylepacks/handpainted") && valid_root("textures/handpainted"));
        for bad in [
            "models/stylepacks/",
            "models/stylepacks/../x",
            "models/polyhaven",
            "shaders",
            "textures/../shaders",
            "textures/",
            "models/stylepacks/Hand",
        ] {
            assert!(!valid_root(bad), "{bad}");
        }
    }
}
