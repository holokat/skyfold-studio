//! Runtime locations for bundled resources and user-authored data.
//!
//! Development executables find the checkout from their `target/` location. A packaged macOS
//! app reads immutable resources from `Contents/Resources/assets` and writes to Application
//! Support, so it can run from a read-only disk image.

use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Skyfold Studio";
pub const USER_ASSET_SOURCE: &str = "user";

fn macos_bundle_assets(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents")
        .then(|| contents.join("Resources/assets"))
        .filter(|assets| assets.is_dir())
}

fn adjacent_assets(exe: &Path) -> Option<PathBuf> {
    exe.parent()
        .map(|dir| dir.join("assets"))
        .filter(|assets| assets.is_dir())
}

fn checkout_root_from(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|dir| dir.join("Cargo.toml").is_file() && dir.join("assets").is_dir())
        .map(Path::to_path_buf)
}

fn checkout_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(checkout_root_from)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .as_deref()
                .and_then(checkout_root_from)
        })
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// The source checkout when one can be discovered from a development executable.
#[cfg(test)]
pub fn project_dir() -> PathBuf {
    checkout_root()
}

/// The immutable asset root: the app bundle when packaged, or the checkout while developing.
pub fn bundled_assets_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(|exe| macos_bundle_assets(exe).or_else(|| adjacent_assets(exe)))
        .unwrap_or_else(|| checkout_root().join("assets"))
}

fn is_macos_bundle() -> bool {
    std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(macos_bundle_assets)
        .is_some()
}

fn user_data_dir_from_bundle(packaged: bool, home: Option<&Path>, checkout: &Path) -> PathBuf {
    if packaged {
        return home
            .map(|home| home.join("Library/Application Support").join(APP_NAME))
            .unwrap_or_else(|| std::env::temp_dir().join(APP_NAME));
    }
    checkout.to_path_buf()
}

/// The writable root. Dev builds keep using the checkout, preserving existing local worlds.
pub fn user_data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    user_data_dir_from_bundle(is_macos_bundle(), home.as_deref(), &checkout_root())
}

pub fn worlds_dir() -> PathBuf {
    user_data_dir().join("worlds")
}

pub fn scenes_dir() -> PathBuf {
    user_data_dir().join("scenes")
}

pub fn exports_dir() -> PathBuf {
    user_data_dir().join("exports")
}

/// Rebuildable derived data such as thumbnail, texture, and mesh caches.
pub fn cache_dir(name: &str) -> PathBuf {
    user_data_dir().join("cache").join(name)
}

/// Writable imported assets. The default AssetServer source remains the bundled root.
pub fn user_assets_dir() -> PathBuf {
    if is_macos_bundle() {
        user_data_dir().join("assets")
    } else {
        bundled_assets_dir()
    }
}

/// Bought asset packs (`crate::market::install`), laid out like `assets/` (a pack's library in
/// `models/stylepacks/<style>`). They live with the user's data, outside the app, so updating or
/// reinstalling Skyfold Studio never removes them; anything the bundle lacks is read from here
/// (the default asset source and `asset_file`).
pub fn packs_dir() -> PathBuf {
    user_data_dir().join("packs")
}

/// Make an AssetServer path that resolves through the writable user-asset source.
pub fn user_asset_path(path: &str) -> String {
    if path.starts_with(&format!("{USER_ASSET_SOURCE}://")) {
        path.to_string()
    } else {
        format!("{USER_ASSET_SOURCE}://{path}")
    }
}

fn asset_file_from(path: &str, bundled: &Path, user_assets: &Path, packs: &Path) -> PathBuf {
    let path = path.split('#').next().unwrap_or(path);
    if let Some(relative) = path.strip_prefix(&format!("{USER_ASSET_SOURCE}://")) {
        return user_assets.join(relative);
    }
    let file = bundled.join(path);
    if !file.exists() {
        // a bought pack's file (the shipped app doesn't bundle paid packs)
        let bought = packs.join(path);
        if bought.exists() {
            return bought;
        }
    }
    file
}

/// Resolve an AssetServer path to the filesystem path needed by exporters and validators.
pub fn asset_file(path: &str) -> PathBuf {
    asset_file_from(path, &bundled_assets_dir(), &user_assets_dir(), &packs_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_assets_in_a_macos_bundle() {
        let root = std::env::temp_dir().join(format!("skyfold-paths-{}", fastrand::u64(..)));
        let assets = root.join("Skyfold Studio.app/Contents/Resources/assets");
        std::fs::create_dir_all(&assets).unwrap();
        let exe = root.join("Skyfold Studio.app/Contents/MacOS/Skyfold Studio");
        assert_eq!(macos_bundle_assets(&exe), Some(assets));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn user_asset_paths_are_idempotent() {
        assert_eq!(
            user_asset_path("imported/cabin.gltf"),
            "user://imported/cabin.gltf"
        );
        assert_eq!(
            user_asset_path("user://imported/cabin.gltf"),
            "user://imported/cabin.gltf"
        );
    }

    #[test]
    fn packaged_data_and_user_assets_stay_outside_the_bundle() {
        let home = Path::new("/Users/example");
        let checkout = Path::new("/source/skyfold");
        let data = user_data_dir_from_bundle(true, Some(home), checkout);
        assert_eq!(
            data,
            home.join("Library/Application Support/Skyfold Studio")
        );
        assert_eq!(
            user_data_dir_from_bundle(false, Some(home), checkout),
            checkout
        );

        let bundled = Path::new("/Applications/Skyfold Studio.app/Contents/Resources/assets");
        let user_assets = data.join("assets");
        let packs = data.join("packs");
        assert_eq!(
            asset_file_from("models/inn.gltf", bundled, &user_assets, &packs),
            bundled.join("models/inn.gltf")
        );
        assert_eq!(
            asset_file_from(
                "user://imported/cabin.gltf#Scene0",
                bundled,
                &user_assets,
                &packs
            ),
            user_assets.join("imported/cabin.gltf")
        );
    }

    #[test]
    fn bought_pack_files_fill_in_for_the_bundle() {
        let root = std::env::temp_dir().join(format!("skyfold-packs-{}", fastrand::u64(..)));
        let (bundled, user, packs) = (root.join("app"), root.join("user"), root.join("packs"));
        std::fs::create_dir_all(bundled.join("models/stylepacks/handpainted/inn")).unwrap();
        std::fs::create_dir_all(packs.join("models/stylepacks/handpainted/well")).unwrap();
        std::fs::write(bundled.join("models/stylepacks/handpainted/inn/inn.gltf"), "{}").unwrap();
        std::fs::write(packs.join("models/stylepacks/handpainted/inn/inn.gltf"), "{}").ok();
        std::fs::write(packs.join("models/stylepacks/handpainted/well/well.gltf"), "{}").unwrap();
        let f = |p: &str| asset_file_from(p, &bundled, &user, &packs);
        // the bundle wins; the pack fills in what it lacks; neither has it: the bundle's path
        assert_eq!(
            f("models/stylepacks/handpainted/inn/inn.gltf"),
            bundled.join("models/stylepacks/handpainted/inn/inn.gltf")
        );
        assert_eq!(
            f("models/stylepacks/handpainted/well/well.gltf#Scene0"),
            packs.join("models/stylepacks/handpainted/well/well.gltf")
        );
        assert_eq!(f("models/none.gltf"), bundled.join("models/none.gltf"));
        let _ = std::fs::remove_dir_all(root);
    }
}
