//! The photoscanned ore surfaces (`ores::Surf`: copper, verdigris, rust, coal, quartz, gold…):
//! CC0 scans that `tools/process_ore_surfaces.py` turns into `assets/textures/ores/<surf>_albedo.ktx2`
//! (BC3: colour, height in alpha), `_normal.ktx2` (BC5) and `_pbr.ktx2` (BC5: roughness,
//! metalness), every mip included. They are stacked here into three texture arrays, a layer per
//! surface in `Surf::ALL` order, and each layer's mean colour, roughness and metalness is read
//! from its smallest mip: what the shader shows from afar, and what it recolours from.

use std::path::PathBuf;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};

use crate::gameplay::ores::Surf;
use crate::photoscan::{decode_bc, ktx2_image};

/// A surface's averages (colour linear).
#[derive(Clone, Copy, Debug)]
pub struct LayerStats {
    pub albedo: Vec3,
    pub rough: f32,
    pub metal: f32,
}

/// The surfaces as texture arrays (layer = `Surf::layer`).
pub struct Scans {
    pub albedo: Handle<Image>,
    pub normal: Handle<Image>,
    pub pbr: Handle<Image>,
    pub stats: Vec<LayerStats>,
}

fn dir() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("textures/ores")
}

/// The folder the surfaces load from in an art style: the painted set in a hand-painted world
/// when it is complete (the arrays need every layer alike), else the photoscans.
fn dir_for(art: crate::art_style::ArtStyle) -> PathBuf {
    let real = dir();
    if crate::art_style::surfaces(art).painted() {
        let twins: Vec<Option<PathBuf>> = files()
            .iter()
            .map(|f| crate::art_style::painted_twin(f).filter(|t| t.is_file()))
            .collect();
        if twins.iter().all(Option::is_some)
            && let Some(painted) = crate::art_style::painted_twin(&real)
        {
            return painted;
        }
    }
    real
}

/// The files of every surface: albedo, normal, pbr (thumbnails re-render when one changes).
pub fn files() -> Vec<PathBuf> {
    let d = dir();
    Surf::ALL
        .iter()
        .flat_map(|s| ["albedo", "normal", "pbr"].map(|m| d.join(format!("{}_{m}.ktx2", s.id()))))
        .collect()
}

/// The last texel of an image (its 1 × 1 mip), decoded: RGBA8 or RG8.
fn last_texel(img: &Image, bc: bool) -> Vec<u8> {
    let data = img.data.as_deref().unwrap_or_default();
    let format = img.texture_descriptor.format;
    if !bc {
        let n = if format == TextureFormat::Rg8Unorm {
            2
        } else {
            4
        };
        return data[data.len().saturating_sub(n)..].to_vec();
    }
    let block = if matches!(
        format,
        TextureFormat::Bc1RgbaUnorm | TextureFormat::Bc1RgbaUnormSrgb
    ) {
        8
    } else {
        16
    };
    let mut out = vec![];
    decode_bc(
        &data[data.len().saturating_sub(block)..],
        1,
        1,
        format,
        &mut out,
    );
    out
}

/// One map of every surface as a texture array (all layers alike: size, format, mips).
fn stack(layers: Vec<Image>) -> Result<Image, String> {
    let first = layers.first().ok_or("no surfaces")?;
    let (desc, sampler) = (first.texture_descriptor.clone(), first.sampler.clone());
    if layers.iter().any(|l| {
        l.texture_descriptor.size != desc.size
            || l.texture_descriptor.format != desc.format
            || l.texture_descriptor.mip_level_count != desc.mip_level_count
    }) {
        return Err("the surfaces differ in size, format or mips".into());
    }
    let n = layers.len() as u32;
    let mut img = Image::new_uninit(
        Extent3d {
            depth_or_array_layers: n,
            ..desc.size
        },
        TextureDimension::D2,
        desc.format,
        RenderAssetUsages::RENDER_WORLD,
    );
    // layer-major: each layer with all its mips
    img.data = Some(
        layers
            .into_iter()
            .flat_map(|l| l.data.unwrap_or_default())
            .collect(),
    );
    img.texture_descriptor.mip_level_count = desc.mip_level_count;
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    img.sampler = sampler;
    Ok(img)
}

/// Reads every surface (in parallel) and stacks them. `bc`: the GPU samples BC formats (else
/// they're decoded to RGBA8 / RG8). (The photoscans; tests.)
#[cfg(test)]
pub fn load(images: &mut Assets<Image>, bc: bool) -> Result<Scans, String> {
    load_style(images, bc, crate::art_style::ArtStyle::Realistic)
}

/// `load` in an art style (the painted surfaces in a hand-painted world).
pub fn load_style(
    images: &mut Assets<Image>,
    bc: bool,
    art: crate::art_style::ArtStyle,
) -> Result<Scans, String> {
    let t0 = std::time::Instant::now();
    let d = dir_for(art);
    let read = |s: Surf, m: &str| ktx2_image(&d.join(format!("{}_{m}.ktx2", s.id())), bc, true);
    let maps: Vec<Result<[Image; 3], String>> = std::thread::scope(|sc| {
        let hs: Vec<_> = Surf::ALL
            .iter()
            .map(|s| {
                sc.spawn(move || {
                    Ok::<_, String>([read(*s, "albedo")?, read(*s, "normal")?, read(*s, "pbr")?])
                })
            })
            .collect();
        hs.into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err("reader panicked".into())))
            .collect()
    });
    let maps = maps.into_iter().collect::<Result<Vec<_>, _>>()?;
    let stats = maps
        .iter()
        .map(|[a, _, p]| {
            let (c, rm) = (last_texel(a, bc), last_texel(p, bc));
            let lin = Color::srgb_u8(c[0], c[1], c[2]).to_linear();
            LayerStats {
                albedo: Vec3::new(lin.red, lin.green, lin.blue),
                rough: rm[0] as f32 / 255.0,
                metal: rm[1] as f32 / 255.0,
            }
        })
        .collect();
    let (mut albedo, mut normal, mut pbr) = (vec![], vec![], vec![]);
    for [a, n, p] in maps {
        albedo.push(a);
        normal.push(n);
        pbr.push(p);
    }
    let size = albedo[0].texture_descriptor.size;
    let scans = Scans {
        albedo: images.add(stack(albedo)?),
        normal: images.add(stack(normal)?),
        pbr: images.add(stack(pbr)?),
        stats,
    };
    info!(
        "ore surfaces: {} {}, {}² each, in {:?}",
        Surf::ALL.len(),
        if d == dir() {
            "photoscans"
        } else {
            "painted surfaces"
        },
        size.width,
        t0.elapsed()
    );
    Ok(scans)
}
