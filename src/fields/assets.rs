//! Crop models built in Blender (`tools/crops`): `assets/models/crops/<crop>.lods.bin` (plant
//! variants × levels of detail, the photoscan format) with `<crop>_albedo.ktx2` (colour + cut-out
//! alpha) and `<crop>_normal.ktx2` (tangent-space normal map). A crop that has them is drawn from
//! them (`plants::patch`); the others keep their procedural plants. Files are checked every two
//! seconds and a changed crop is read again, so a model being worked on shows up in the running
//! editor.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::*;

use super::Crop;
use crate::photoscan::{RawMesh, read_lods_raw};

/// A crop's Blender-built plants.
pub struct CropModel {
    /// variants, each with its levels of detail (near first)
    pub variants: Vec<Vec<RawMesh>>,
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub roughness: f32,
}

/// The shared crop texture atlas (`tools/crops/paint.py`, `tools/crops/ATLAS_SPEC.md`): every
/// procedural plant part maps into one of its named regions.
pub struct Atlas {
    pub regions: super::plants::Regions,
    /// each region's average colour (sRGB 0–1) and opaque share, for the far canopy
    pub means: super::plants::Regions,
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
}

#[derive(Resource, Default)]
pub struct CropAssets {
    pub models: HashMap<Crop, Arc<CropModel>>,
    pub atlas: Option<Arc<Atlas>>,
    atlas_stamp: Option<Option<SystemTime>>,
    /// bumped whenever a model is (re)loaded or dropped
    pub revision: u64,
    stamps: HashMap<Crop, Option<SystemTime>>,
    next_check: f32,
}

impl CropAssets {
    pub fn get(&self, c: Crop) -> Option<&Arc<CropModel>> {
        self.models.get(&c)
    }
}

pub fn dir() -> PathBuf {
    PathBuf::from(crate::assets_path())
        .join("models")
        .join("crops")
}

fn stamp(c: Crop) -> Option<SystemTime> {
    let d = dir();
    // the newest of the three files
    [
        format!("{}.lods.bin", c.id()),
        format!("{}_albedo.ktx2", c.id()),
        format!("{}_normal.ktx2", c.id()),
    ]
    .iter()
    .map(|f| std::fs::metadata(d.join(f)).and_then(|m| m.modified()).ok())
    .collect::<Option<Vec<_>>>()
    .and_then(|v| v.into_iter().max())
}

fn load(c: Crop, bc: bool) -> Result<(Vec<Vec<RawMesh>>, Image, Image, f32), String> {
    let d = dir();
    let raw = read_lods_raw(&d.join(format!("{}.lods.bin", c.id())))?;
    let albedo =
        crate::photoscan::ktx2_image(&d.join(format!("{}_albedo.ktx2", c.id())), bc, false)?;
    let normal =
        crate::photoscan::ktx2_image(&d.join(format!("{}_normal.ktx2", c.id())), bc, false)?;
    Ok((
        raw.variants.into_iter().map(|v| v.lods).collect(),
        albedo,
        normal,
        raw.roughness,
    ))
}

/// Loads crop models at startup and whenever their files change.
pub(super) fn watch(
    mut a: ResMut<CropAssets>,
    mut images: ResMut<Assets<Image>>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed_secs();
    if now < a.next_check {
        return;
    }
    a.next_check = now + 2.0;
    let bc = formats
        .as_deref()
        .is_some_and(|f| f.0.contains(CompressedImageFormats::BC));
    // the shared atlas
    let d = dir();
    let files = ["atlas.json", "atlas_albedo.ktx2", "atlas_normal.ktx2"];
    let st = files
        .iter()
        .map(|f| std::fs::metadata(d.join(f)).and_then(|m| m.modified()).ok())
        .collect::<Option<Vec<_>>>()
        .and_then(|v| v.into_iter().max());
    if a.atlas_stamp != Some(st) {
        a.atlas_stamp = Some(st);
        let loaded = st.map(|_| -> Result<_, String> {
            #[derive(serde::Deserialize)]
            struct Json {
                regions: super::plants::Regions,
                #[serde(default)]
                means: super::plants::Regions,
            }
            let j: Json = serde_json::from_slice(
                &std::fs::read(d.join("atlas.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("atlas.json: {e}"))?;
            let albedo = crate::photoscan::ktx2_image(&d.join("atlas_albedo.ktx2"), bc, false)?;
            let normal = crate::photoscan::ktx2_image(&d.join("atlas_normal.ktx2"), bc, false)?;
            Ok((j.regions, j.means, albedo, normal))
        });
        match loaded {
            Some(Ok((regions, means, albedo, normal))) => {
                info!("CROPS atlas: {} regions loaded", regions.len());
                a.atlas = Some(Arc::new(Atlas {
                    regions,
                    means,
                    albedo: images.add(albedo),
                    normal: images.add(normal),
                }));
                a.revision += 1;
            }
            Some(Err(e)) => warn!("CROPS atlas: {e}"),
            None => {
                if a.atlas.take().is_some() {
                    a.revision += 1;
                }
            }
        }
    }
    for c in Crop::ALL {
        let s = stamp(c);
        if a.stamps.get(&c) == Some(&s) {
            continue;
        }
        a.stamps.insert(c, s);
        match s.map(|_| load(c, bc)) {
            Some(Ok((variants, albedo, normal, roughness))) => {
                let n: usize = variants
                    .iter()
                    .map(|v| v.first().map_or(0, |m| m.pos.len()))
                    .sum();
                info!(
                    "CROPS {}: {} variants ({n} vertices at full detail) loaded",
                    c.id(),
                    variants.len()
                );
                a.models.insert(
                    c,
                    Arc::new(CropModel {
                        variants,
                        albedo: images.add(albedo),
                        normal: images.add(normal),
                        roughness,
                    }),
                );
                a.revision += 1;
            }
            Some(Err(e)) => warn!("CROPS {}: {e}", c.id()),
            None => {
                if a.models.remove(&c).is_some() {
                    a.revision += 1;
                }
            }
        }
    }
}
