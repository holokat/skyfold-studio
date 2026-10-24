//! The thumbnail disk cache stores `<subject>-<hash>.png` under the user's data folder. The hash
//! covers the subject,
//! its source (model file size and date, or the generated mesh's shape) and the studio's version,
//! so a changed model or a new studio look renders again and anything else loads in a few ms.
//! Files hold the premultiplied RGBA exactly as rendered; reading, decoding and writing happen on
//! background threads.

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::Subject;
use crate::objects::Library;

/// Bump when the studio (lights, framing, size) changes: every thumbnail renders again.
const VERSION: u64 = 1;

pub fn dir() -> PathBuf {
    crate::paths::cache_dir("thumbs")
}

/// FNV-1a.
fn hash(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        for b in *p {
            h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn file_stamp(path: &Path) -> Vec<u8> {
    let Ok(m) = std::fs::metadata(path) else {
        return vec![];
    };
    let t = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    [m.len().to_le_bytes(), t.to_le_bytes()].concat()
}

fn mesh_stamp(meshes: &Assets<Mesh>, h: &Handle<Mesh>) -> Vec<u8> {
    let Some(m) = meshes.get(h) else {
        return vec![];
    };
    // (meshes whose data went to the GPU only keep their bounds)
    let n = m
        .try_attribute(Mesh::ATTRIBUTE_POSITION)
        .map_or(0, |a| a.len());
    let mut out = (n as u64).to_le_bytes().to_vec();
    if let Some((lo, hi)) = super::studio::mesh_bounds(m) {
        for v in [lo.x, lo.y, lo.z, hi.x, hi.y, hi.z] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// What the thumbnail of `s` depends on. `None` while that can't be known yet (meshes loading).
pub fn stamp(
    s: &Subject,
    lib: Option<&Library>,
    meshes: &Assets<Mesh>,
    size: UVec2,
) -> Option<u64> {
    let assets = PathBuf::from(crate::assets_path());
    let source = match s {
        Subject::Asset(id) => {
            let e = lib?.get(id)?;
            if let Some(t) = &e.tree {
                let v = t.variants.last()?;
                let mut b = mesh_stamp(meshes, &v.lods.first()?.0);
                b.extend_from_slice(
                    &[
                        v.width.to_le_bytes(),
                        v.height.to_le_bytes(),
                        v.min_y.to_le_bytes(),
                    ]
                    .concat(),
                );
                b
            } else if let Some((m, _)) = e.meshes.first() {
                mesh_stamp(meshes, m)
            } else {
                file_stamp(&crate::asset_file(e.import_path.as_deref()?))
            }
        }
        Subject::Fish(f) => {
            let m = f.info().model;
            [
                file_stamp(&assets.join(format!("models/fish/{m}.glb"))),
                file_stamp(&assets.join(format!("models/fish/{m}.png"))),
            ]
            .concat()
        }
        // the ore's look, the shader, the photoscanned surfaces and the rock scan it is drawn on
        // the scene's file, and the models it's made of
        Subject::Scene(id) => {
            let mut b = crate::scenes::file::stamp(id);
            if !b.is_empty() {
                let scene = crate::scenes::file::read(id)?;
                for (asset, _) in scene.assets() {
                    if let Some(e) = lib?.get(&asset) {
                        match (&e.import_path, e.meshes.first()) {
                            (Some(p), _) => b.extend(file_stamp(&crate::asset_file(p))),
                            (None, Some((m, _))) => b.extend(mesh_stamp(meshes, m)),
                            _ => {}
                        }
                    }
                }
            }
            b
        }
        Subject::Ore(o) => {
            let rock = lib?
                .get(&crate::objects::builtin_id(
                    crate::procgen::meshes::AssetKind::Boulder,
                ))?
                .tree
                .as_ref()?
                .variants
                .first()?;
            let mut b = format!(
                "{:?} {}",
                o.look(),
                crate::gameplay::deposits::PREVIEW_VERSION
            )
            .into_bytes();
            b.extend(file_stamp(&assets.join("shaders/ore.wesl")));
            for f in crate::gameplay::deposits::scans::files() {
                b.extend(file_stamp(&f));
            }
            b.extend(mesh_stamp(meshes, &rock.lods.first()?.0));
            b
        }
    };
    if source.is_empty() {
        return None;
    }
    Some(hash(&[
        s.key().as_bytes(),
        &source,
        &VERSION.to_le_bytes(),
        &size.x.to_le_bytes(),
        &size.y.to_le_bytes(),
    ]))
}

/// The picture a preview library ships for `s` (`crate::objects::PREVIEWS`: a paid pack's model
/// this app has no files for) and its key. It is the studio's own thumbnail, made when the app
/// was packaged, so it is loaded like a cached one and never shot (or cached) here.
pub fn shipped(s: &Subject, lib: Option<&Library>) -> Option<(PathBuf, u64)> {
    let Subject::Asset(id) = s else { return None };
    let file = crate::asset_file(&crate::objects::preview_thumb(lib?.get(id)?)?);
    let source = file_stamp(&file);
    (!source.is_empty()).then(|| {
        let stamp = hash(&[
            s.key().as_bytes(),
            b"shipped",
            &source,
            &VERSION.to_le_bytes(),
        ]);
        (file, stamp)
    })
}

/// The file name prefix of a subject (its key with anything unsafe replaced).
fn prefix(s: &Subject) -> String {
    s.key()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        + "-"
}

pub fn path(s: &Subject, stamp: u64) -> PathBuf {
    dir().join(format!("{}{stamp:016x}.png", prefix(s)))
}

/// Reads a cached thumbnail (background thread).
pub fn load(path: &Path, size: UVec2) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.into_rgba8();
    (img.dimensions() == (size.x, size.y)).then(|| img.into_raw())
}

/// Writes a thumbnail and removes the subject's outdated ones (background thread).
pub fn save(path: PathBuf, size: UVec2, rgba: Vec<u8>) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name
        .rsplit_once('-')
        .map(|(p, _)| format!("{p}-"))
        .unwrap_or_default();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for f in rd.flatten() {
            let n = f.file_name().to_string_lossy().into_owned();
            // same subject (prefix + 16 hex digits), another version
            if n != name && n.len() == stem.len() + 20 && n.starts_with(&stem) {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
    if let Some(img) = image::RgbaImage::from_raw(size.x, size.y, rgba)
        && let Err(e) = img.save(&path)
    {
        warn!("thumbnail cache {}: {e}", path.display());
    }
}

/// A scene's cover picture (`thumb.png` in its folder), kept with its transparent background.
pub fn save_cover(path: PathBuf, size: UVec2, rgba: Vec<u8>) {
    if let Some(img) = image::RgbaImage::from_raw(size.x, size.y, rgba)
        && let Err(e) = img.save(&path)
    {
        warn!("scene cover {}: {e}", path.display());
    }
}

/// A cached thumbnail as a PNG on the panel's dark background (for MCP).
pub fn flattened_png(path: &Path, size: UVec2) -> Result<Vec<u8>, String> {
    let rgba = load(path, size).ok_or_else(|| format!("{}: unreadable", path.display()))?;
    let bg = [24.0f32, 30.0, 40.0];
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|p| {
            (0..3).map(move |c| {
                (p[c] as f32 + bg[c] * (1.0 - p[3] as f32 / 255.0))
                    .round()
                    .min(255.0) as u8
            })
        })
        .collect();
    let mut out = std::io::Cursor::new(vec![]);
    image::RgbImage::from_raw(size.x, size.y, rgb)
        .ok_or("bad size")?
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// A thumbnail texture with its mip chain (box filtered: the data is premultiplied, so a plain
/// average is right), for crisp tiles at any UI scale.
pub fn image(size: UVec2, rgba: Vec<u8>) -> Image {
    let mut data = rgba.clone();
    let mut level = rgba;
    let (mut w, mut h) = (size.x as usize, size.y as usize);
    let mut levels = 1;
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| {
                        level[(yy.min(h - 1) * w + xx.min(w - 1)) * 4 + c] as u32
                    };
                    next[(y * nw + x) * 4 + c] = ((at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1)
                        + 2)
                        / 4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        level = next;
        (w, h) = (nw, nh);
        levels += 1;
    }
    let mut img = Image::new_uninit(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = levels;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A preview set as the app ships it (`package_macos.py --previews-only DIR`):
    /// `SKYFOLD_PREVIEW_SET=DIR cargo test a_shipped_preview_set -- --ignored`
    #[test]
    #[ignore]
    fn a_shipped_preview_set_has_every_picture_and_only_the_samples() {
        let dir = std::path::PathBuf::from(std::env::var("SKYFOLD_PREVIEW_SET").unwrap());
        let models = dir.join("models");
        let libs = crate::objects::model_libraries_in(&models, &dir.join("no-packs"));
        let hp = libs
            .iter()
            .find(|l| l.prefix == "hp")
            .expect("the preview library");
        assert!(hp.preview && hp.pack_id == "skyfold_studio");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(hp.dir.join("models.json")).unwrap())
                .unwrap();
        let samples: Vec<String> = doc["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| format!("hp:{}", s.as_str().unwrap()))
            .collect();
        let entries = crate::objects::model_library(hp);
        assert!(entries.len() >= 500, "{} models", entries.len());
        let (mut with_files, mut pictures) = (0, 0);
        for e in &entries {
            let thumb = dir.join(crate::objects::preview_thumb(e).expect("a preview entry"));
            assert!(
                load(&thumb, UVec2::splat(192)).is_some(),
                "{}: its picture",
                e.id
            );
            pictures += 1;
            let model = dir.join(e.import_path.as_ref().unwrap());
            assert_eq!(model.is_file(), samples.contains(&e.id), "{}: files", e.id);
            with_files += model.is_file() as usize;
        }
        assert_eq!(with_files, 11, "the hero and 10 free samples");
        eprintln!("{pictures} pictures, {with_files} models with files");
    }

    #[test]
    fn saving_replaces_older_versions() {
        let s = Subject::Fish(crate::fish::FishSpecies::Koi);
        let tmp = std::env::temp_dir().join(format!("wb_thumbs_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let size = UVec2::new(4, 2);
        let a = tmp.join(format!("{}{:016x}.png", prefix(&s), 1));
        let b = tmp.join(format!("{}{:016x}.png", prefix(&s), 2));
        let other = tmp.join(format!(
            "{}{:016x}.png",
            prefix(&Subject::Asset("x".into())),
            1
        ));
        save(a.clone(), size, vec![255; 32]);
        save(other.clone(), size, vec![9; 32]);
        save(b.clone(), size, vec![128; 32]);
        assert!(!a.exists() && b.exists() && other.exists());
        assert_eq!(load(&b, size).unwrap()[0], 128);
        assert!(load(&b, UVec2::new(8, 8)).is_none());
        let img = image(size, vec![200; 32]);
        assert_eq!(img.texture_descriptor.mip_level_count, 3);
        assert_eq!(img.data.as_ref().unwrap().len(), (8 + 2 + 1) * 4);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
