//! Photoreal terrain material textures: CC0 photoscans from ambientCG / Poly Haven (see
//! `assets/textures/CREDITS.md`) processed by `tools/process_photoscan.py`; ice has no CC0 photo
//! and uses ambientCG's procedural Ice003 set. Loaded on a background thread into
//! two mip-mapped 2D texture arrays: albedo (RGB sRGB + height in A) and detail (normal XY,
//! roughness, AO). 1024² per layer.
//!
//! Where the GPU samples BC formats the arrays are BC7 (`model_render::compress`: ~34 MB for both
//! instead of ~134 MB as RGBA8, and a quarter of the memory traffic for the most-sampled textures
//! of the frame). Every layer's compressed chains are cached on disk (`assets/.texcache/`, keyed by
//! its source files' paths, sizes and dates), so a later start reads them instead of decoding the
//! PNGs. A layer that isn't cached yet (first start, a new library material) shows uncompressed
//! at once and is compressed in the background, then the arrays are swapped. `WB_NOBC=1` keeps
//! them uncompressed (A/B tests).

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::{
    CompressedImageFormatSupport, CompressedImageFormats, ImageAddressMode, ImageFilterMode,
    ImageSampler, ImageSamplerDescriptor,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::layers::{LAYER_COUNT, Layer};
use super::material::{TerrainGpu, TerrainMaterial};
use crate::model_render::compress::{self, Codec, Effort};

/// Albedo: sRGB colour + height in alpha.
const ALBEDO_CODEC: Codec = Codec::Bc7 { alpha: true };
/// Detail: normal XY, roughness, AO (four uncorrelated data channels: the slower, fuller BC7
/// search keeps the normals ~4 dB closer).
const DETAIL_CODEC: Codec = Codec::Bc7 { alpha: true };
const DETAIL_EFFORT: Effort = Effort::Max;

pub const LAYER_FILES: [&str; LAYER_COUNT] = [
    "grass",
    "dry_grass",
    "forest_floor",
    "dirt",
    "mud",
    "gravel",
    "rock",
    "volcanic_rock",
    "sand",
    "snow",
    "cobblestone",
    "ice",
];

/// World-space size (m) of one texture repeat per layer: the scans' real size (dry grass 2 m,
/// mud 2.35 m, sand 3.5 m, cobbles 1.5 m, the rest judged from pebbles / leaves / blades), up to
/// ~25% larger to soften the repeat. Rock faces are scale-free and stay large (the 1.8 m granite
/// scan at 4.5 m) so big slopes don't show the repeat.
pub const LAYER_TILE: [f32; LAYER_COUNT] =
    [2.0, 2.2, 2.4, 1.6, 2.4, 1.6, 4.5, 5.0, 3.5, 3.5, 1.6, 5.0];

/// One material of the bundled CC0 terrain library (`assets/textures/terrain_library/`).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct LibraryMaterial {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub surface: String,
    #[serde(default = "default_tile")]
    pub tile_m: f32,
}

fn default_tile() -> f32 {
    2.0
}

#[derive(Resource, Default)]
pub struct TerrainLibrary {
    pub materials: Vec<LibraryMaterial>,
}

/// How much of the world each slot covers (fractions of the land, painted or automatic).
pub fn layer_usage(t: &crate::terrain::Terrain) -> [f32; LAYER_COUNT] {
    let mut sum = [0.0f32; LAYER_COUNT];
    let n = t.n();
    let step = (n / 256).max(1);
    let mut count: f32 = 0.0;
    for j in (0..n).step_by(step) {
        for i in (0..n).step_by(step) {
            let w = t.final_weights(i, j);
            for k in 0..LAYER_COUNT {
                sum[k] += w[k];
            }
            count += 1.0;
        }
    }
    sum.map(|v| v / count.max(1.0))
}

/// Slots the automatic style is least likely to need, first: a free one of these takes a new
/// material.
pub const FREE_ORDER: [Layer; LAYER_COUNT] = [
    Layer::Ice,
    Layer::Cobblestone,
    Layer::DarkRock,
    Layer::Mud,
    Layer::DryGrass,
    Layer::ForestFloor,
    Layer::Gravel,
    Layer::Dirt,
    Layer::Snow,
    Layer::Sand,
    Layer::Rock,
    Layer::Grass,
];

/// The slot a library material paints with: the one holding it already, else one the world
/// doesn't use at all (`FREE_ORDER`); None when all twelve are in use.
pub fn slot_for(choices: &TerrainLayerChoices, t: &crate::terrain::Terrain, id: &str) -> Option<usize> {
    if let Some(i) = (0..LAYER_COUNT).find(|i| choices.get(*i) == id) {
        return Some(i);
    }
    let usage = layer_usage(t);
    FREE_ORDER.iter().map(|l| *l as usize).find(|i| usage[*i] < 1e-4)
}

/// Which library material fills each paint layer ("" = the built-in one). Saved with the world.
#[derive(Resource, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TerrainLayerChoices {
    pub ids: Vec<String>,
}

/// Lush green lawn under the grass by default: the built-in grass layer (soil with plants)
/// reads brown from afar.
pub const LUSH_GRASS: &str = "lush_green_lawn";

impl Default for TerrainLayerChoices {
    fn default() -> Self {
        let mut ids = vec![String::new(); LAYER_COUNT];
        ids[0] = LUSH_GRASS.to_string();
        Self { ids }
    }
}

impl TerrainLayerChoices {
    pub fn get(&self, layer: usize) -> &str {
        self.ids.get(layer).map(|s| s.as_str()).unwrap_or("")
    }
    pub fn set(&mut self, layer: usize, id: &str) {
        if self.ids.len() < LAYER_COUNT {
            self.ids.resize(LAYER_COUNT, String::new());
        }
        self.ids[layer] = id.to_string();
    }
}

pub fn library_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::assets_path()).join("textures/terrain_library")
}

/// One file of a library material ("albedo.png"): the app's, or a downloaded content pack's
/// (`crate::content`; the app keeps every material's swatch).
pub fn library_file(id: &str, name: &str) -> std::path::PathBuf {
    crate::paths::asset_file(&format!("textures/terrain_library/{id}/{name}"))
}

/// Whether a library material's files are on this computer (false for a content pack's
/// material that hasn't been downloaded yet).
pub fn library_available(id: &str) -> bool {
    crate::content::available(&format!("textures/terrain_library/{id}"))
}

fn read_library() -> Vec<LibraryMaterial> {
    #[derive(serde::Deserialize)]
    struct Doc {
        materials: Vec<LibraryMaterial>,
    }
    std::fs::read_to_string(library_dir().join("library.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Doc>(&s).ok())
        .map(|d| d.materials)
        .unwrap_or_default()
}

#[derive(Resource, Default)]
pub struct TerrainTextures {
    pub albedo: Option<Handle<Image>>,
    pub detail: Option<Handle<Image>>,
    /// metres per repeat for each layer (built-in or library)
    pub tiles: [f32; LAYER_COUNT],
    /// bumped whenever new arrays are loaded (UI swatches refresh)
    pub revision: u32,
    /// the choices and surface style (`art_style::surfaces`) the current arrays were built from,
    /// and the content packs installed then (`content::revision`)
    applied: (TerrainLayerChoices, crate::art_style::ArtStyle, u32),
    /// Swatch previews for the UI (RGBA8, 128²), if available.
    pub swatches: Vec<Option<(u32, u32, Vec<u8>)>>,
    pub status: TextureStatus,
    /// bumped per load; a background compression of older arrays is dropped
    load_gen: u32,
}

#[derive(Default, Clone, PartialEq, Debug)]
pub enum TextureStatus {
    #[default]
    Loading,
    Ready,
    Missing(String),
}

#[derive(Resource)]
struct LoadTask(Task<Result<Loaded, String>>);

struct Loaded {
    albedo: Image,
    detail: Image,
    swatches: Vec<Option<(u32, u32, Vec<u8>)>>,
    tiles: [f32; LAYER_COUNT],
    /// the arrays are uncompressed but the GPU samples BC: compress them in the background
    compress: Option<CompressJob>,
}

/// The layers to block-compress (the rest are cached already).
struct CompressJob {
    size: u32,
    mips: u32,
    /// cache keys (albedo, detail) per layer
    keys: Vec<(u64, u64)>,
    /// (layer, albedo chain, detail chain) of the layers missing from the cache
    missing: Vec<(usize, Vec<u8>, Vec<u8>)>,
}

/// Background compression of the arrays now in use; carries their `load_gen`.
#[derive(Resource)]
struct CompressTask(Task<Result<(Image, Image), String>>, u32);

pub struct TerrainTexturesPlugin;

impl Plugin for TerrainTexturesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainTextures>()
            .init_resource::<TerrainLayerChoices>()
            .insert_resource(TerrainLibrary {
                materials: read_library(),
            })
            .add_systems(Startup, start_loading)
            .add_systems(Update, reload_on_choice)
            .add_systems(
                PostUpdate,
                (finish_loading, finish_compressing, bind_textures)
                    .chain()
                    .after(super::TerrainSystems::Gpu),
            );
    }
}

pub fn texture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::assets_path()).join("textures/terrain")
}

/// Whether the arrays are block-compressed: the GPU samples BC formats and `WB_NOBC` isn't set.
fn use_bc(formats: Option<&CompressedImageFormatSupport>) -> bool {
    std::env::var("WB_NOBC").is_err()
        && formats.is_some_and(|f| f.0.contains(CompressedImageFormats::BC))
}

fn start_loading(
    mut commands: Commands,
    library: Res<TerrainLibrary>,
    mut tex: ResMut<TerrainTextures>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    meta: Res<crate::world::WorldMeta>,
) {
    tex.tiles = LAYER_TILE;
    if std::env::var("WB_NOTEX").is_ok() {
        return;
    }
    let art = crate::art_style::surfaces(meta.art_style);
    tex.applied = (
        TerrainLayerChoices::default(),
        art,
        crate::content::revision(),
    );
    spawn_load(
        &mut commands,
        &mut tex,
        TerrainLayerChoices::default(),
        library.materials.clone(),
        use_bc(formats.as_deref()),
        art,
    );
}

fn spawn_load(
    commands: &mut Commands,
    tex: &mut TerrainTextures,
    choices: TerrainLayerChoices,
    library: Vec<LibraryMaterial>,
    bc: bool,
    art: crate::art_style::ArtStyle,
) {
    let dir = texture_dir();
    tex.load_gen += 1;
    let task = AsyncComputeTaskPool::get()
        .spawn(async move { load_all(&dir, &choices, &library, bc, art) });
    commands.insert_resource(LoadTask(task));
}

/// A layer got another library material, or the world another art style: rebuild the arrays in
/// the background (the material keeps its bindings; `bind_textures` swaps the arrays in).
fn reload_on_choice(
    mut commands: Commands,
    choices: Res<TerrainLayerChoices>,
    library: Res<TerrainLibrary>,
    mut tex: ResMut<TerrainTextures>,
    task: Option<Res<LoadTask>>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    meta: Res<crate::world::WorldMeta>,
) {
    let art = crate::art_style::surfaces(meta.art_style);
    let rev = crate::content::revision();
    if task.is_some()
        || tex.status != TextureStatus::Ready
        || (*choices == tex.applied.0 && art == tex.applied.1 && rev == tex.applied.2)
    {
        return;
    }
    tex.applied = (choices.clone(), art, rev);
    spawn_load(
        &mut commands,
        &mut tex,
        choices.clone(),
        library.materials.clone(),
        use_bc(formats.as_deref()),
        art,
    );
}

fn finish_loading(
    mut commands: Commands,
    task: Option<ResMut<LoadTask>>,
    mut tex: ResMut<TerrainTextures>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut task) = task else { return };
    let Some(result) = block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<LoadTask>();
    match result {
        Ok(l) => {
            tex.albedo = Some(images.add(l.albedo));
            tex.detail = Some(images.add(l.detail));
            tex.swatches = l.swatches;
            tex.tiles = l.tiles;
            tex.revision += 1;
            tex.status = TextureStatus::Ready;
            info!("terrain textures ready");
            if let Some(job) = l.compress {
                commands.insert_resource(CompressTask(
                    AsyncComputeTaskPool::get().spawn(async move { compress_arrays(job) }),
                    tex.load_gen,
                ));
            }
        }
        Err(e) => {
            warn!("terrain textures unavailable, using procedural shading: {e}");
            tex.status = TextureStatus::Missing(e);
        }
    }
}

/// Swaps the block-compressed arrays in once they are built (if no newer load replaced them).
fn finish_compressing(
    mut commands: Commands,
    task: Option<ResMut<CompressTask>>,
    mut tex: ResMut<TerrainTextures>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut task) = task else { return };
    let Some(result) = block_on(future::poll_once(&mut task.0)) else {
        return;
    };
    commands.remove_resource::<CompressTask>();
    match result {
        Ok((albedo, detail)) if task.1 == tex.load_gen => {
            tex.albedo = Some(images.add(albedo));
            tex.detail = Some(images.add(detail));
        }
        Ok(_) => {}
        Err(e) => warn!("terrain textures stay uncompressed: {e}"),
    }
}

/// Puts the arrays and the clouds' cover map (cloud shadows) into the terrain material (also
/// after a new world recreates it).
fn bind_textures(
    tex: Res<TerrainTextures>,
    gpu: Option<Res<TerrainGpu>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    cover: Option<Res<crate::clouds::CloudCoverMap>>,
) {
    let Some(gpu) = gpu else { return };
    if let Some(cover) = cover
        && materials
            .get(&gpu.material)
            .is_some_and(|m| m.extension.cloud_cover != cover.0)
        && let Some(mut m) = materials.get_mut(&gpu.material)
    {
        m.extension.cloud_cover = cover.0.clone();
    }
    let (Some(albedo), Some(detail)) = (&tex.albedo, &tex.detail) else {
        return;
    };
    let Some(m) = materials.get_mut_untracked(&gpu.material) else {
        return;
    };
    if m.extension.albedo_array != *albedo || m.extension.detail_array != *detail {
        let (albedo, detail) = (albedo.clone(), detail.clone());
        let mut m = materials.get_mut(&gpu.material).unwrap();
        m.extension.albedo_array = albedo;
        m.extension.detail_array = detail;
        m.extension.params.tex.x = 1.0;
    }
}

/// 1×1 placeholder arrays so the material always has valid bindings.
pub fn placeholder_array(srgb: bool) -> Image {
    let data: Vec<u8> = (0..LAYER_COUNT)
        .flat_map(|_| [128u8, 128, 128, 128])
        .collect();
    array_image(
        1,
        1,
        data,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
    )
}

fn array_image(size: u32, mips: u32, data: Vec<u8>, format: TextureFormat) -> Image {
    // `Image::new` asserts the data is exactly one mip level, so fill in the full chain ourselves
    let mut img = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: LAYER_COUNT as u32,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = mips;
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    img
}

pub(crate) fn srgb_to_lin_lut() -> [f32; 256] {
    std::array::from_fn(|i| {
        let c = i as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

pub(crate) fn lin_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// Box-filter mip chain. `srgb_rgb` averages RGB in linear light.
pub(crate) fn build_mips(base: &[u8], size: u32, srgb_rgb: bool, lut: &[f32; 256]) -> Vec<u8> {
    let mut out = base.to_vec();
    let mut prev = base.to_vec();
    let mut s = size;
    while s > 1 {
        let n = s / 2;
        let mut next = vec![0u8; (n * n * 4) as usize];
        for y in 0..n {
            for x in 0..n {
                for c in 0..4 {
                    let px = |xx: u32, yy: u32| prev[((yy * s + xx) * 4 + c) as usize];
                    let v = [
                        px(2 * x, 2 * y),
                        px(2 * x + 1, 2 * y),
                        px(2 * x, 2 * y + 1),
                        px(2 * x + 1, 2 * y + 1),
                    ];
                    next[((y * n + x) * 4 + c) as usize] = if srgb_rgb && c < 3 {
                        lin_to_srgb(v.iter().map(|&b| lut[b as usize]).sum::<f32>() * 0.25)
                    } else {
                        ((v.iter().map(|&b| b as u32).sum::<u32>() + 2) / 4) as u8
                    };
                }
            }
        }
        out.extend_from_slice(&next);
        prev = next;
        s = n;
    }
    out
}

/// The files of one layer: albedo, detail ("normal"), swatch and the library material's repeat.
struct LayerFiles {
    albedo: PathBuf,
    normal: PathBuf,
    swatch: PathBuf,
    tile: Option<f32>,
}

/// The library material the world chose for layer `i` (if its files are there), else the
/// built-in layer; in a hand-painted world their painted versions where there are some
/// (`art_style::textures`).
fn layer_files(
    dir: &Path,
    i: usize,
    choices: &TerrainLayerChoices,
    library: &[LibraryMaterial],
    art: crate::art_style::ArtStyle,
) -> LayerFiles {
    if let Some(m) = library
        .iter()
        .find(|m| !choices.get(i).is_empty() && m.id == choices.get(i))
    {
        let f = |name: &str| library_file(&m.id, name);
        if f("albedo.png").is_file() && f("normal.png").is_file() {
            let [albedo, normal, swatch] = crate::art_style::textures(
                [f("albedo.png"), f("normal.png"), f("swatch.png")],
                art,
            );
            return LayerFiles {
                albedo,
                normal,
                swatch,
                tile: Some(m.tile_m.max(0.3)),
            };
        }
    }
    built_in_files(dir, i, art)
}

fn built_in_files(dir: &Path, i: usize, art: crate::art_style::ArtStyle) -> LayerFiles {
    let name = LAYER_FILES[i];
    let [albedo, normal, swatch] = crate::art_style::textures(
        [
            dir.join(format!("{name}_albedo.png")),
            dir.join(format!("{name}_normal.png")),
            dir.join(format!("{name}_swatch.png")),
        ],
        art,
    );
    LayerFiles {
        albedo,
        normal,
        swatch,
        tile: None,
    }
}

/// Cache keys (albedo, detail) of a layer's compressed chains: its files' paths, sizes and dates.
fn layer_keys(f: &LayerFiles, size: u32) -> (u64, u64) {
    let stamp = |p: &Path| {
        let m = std::fs::metadata(p).ok();
        let t = m
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        compress::content_hash(
            format!("terrain|{}|{}|{t}", p.display(), m.map_or(0, |m| m.len())).as_bytes(),
        )
    };
    (
        compress::cache_key(
            stamp(&f.albedo),
            size,
            size,
            ALBEDO_CODEC,
            true,
            Effort::High,
        ),
        compress::cache_key(
            stamp(&f.normal),
            size,
            size,
            DETAIL_CODEC,
            false,
            DETAIL_EFFORT,
        ),
    )
}

fn read_swatch(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    image::open(path).ok().map(|i| {
        let i = i.to_rgba8();
        (i.width(), i.height(), i.into_raw())
    })
}

fn load_all(
    dir: &Path,
    choices: &TerrainLayerChoices,
    library: &[LibraryMaterial],
    bc: bool,
    art: crate::art_style::ArtStyle,
) -> Result<Loaded, String> {
    let t0 = std::time::Instant::now();
    let mut files: Vec<LayerFiles> = (0..LAYER_COUNT)
        .map(|i| layer_files(dir, i, choices, library, art))
        .collect();
    let mut tiles = LAYER_TILE;
    // the arrays take the largest layer's size (painted library materials are smaller than the
    // built-in layers; smaller ones are scaled up)
    let largest = files
        .iter()
        .filter_map(|f| image::image_dimensions(&f.albedo).ok())
        .map(|d| d.0)
        .max();
    // every layer already compressed: read the cached chains, no PNG to decode
    if bc
        && let Some(size) = largest
        && size.is_power_of_two()
    {
        let mips = size.ilog2() + 1;
        let (mut albedo, mut detail) = (Vec::new(), Vec::new());
        let complete = files.iter().all(|f| {
            let (ka, kd) = layer_keys(f, size);
            match (
                compress::read_cache(ka, size, size, mips),
                compress::read_cache(kd, size, size, mips),
            ) {
                (Some(a), Some(d)) => {
                    albedo.extend(a);
                    detail.extend(d);
                    true
                }
                _ => false,
            }
        });
        if complete {
            for (i, f) in files.iter().enumerate() {
                if let Some(t) = f.tile {
                    tiles[i] = t;
                }
            }
            info!(
                "loaded terrain textures ({}, {size}², {mips} mips, block-compressed, from the cache) in {:?}",
                art.name(),
                t0.elapsed()
            );
            return Ok(Loaded {
                albedo: array_image(size, mips, albedo, ALBEDO_CODEC.format(true)),
                detail: array_image(size, mips, detail, DETAIL_CODEC.format(false)),
                swatches: files.iter().map(|f| read_swatch(&f.swatch)).collect(),
                tiles,
                compress: None,
            });
        }
    }
    let lut = srgb_to_lin_lut();
    let mut size = 0u32;
    let mut albedo = Vec::new();
    let mut detail = Vec::new();
    let mut swatches = Vec::new();
    let mut chains = Vec::new();
    for i in 0..LAYER_COUNT {
        let name = LAYER_FILES[i];
        // a library material whose files don't decode falls back to the built-in layer
        let decode = |f: &LayerFiles| {
            Some((
                image::open(&f.albedo).ok()?.to_rgba8(),
                image::open(&f.normal).ok()?.to_rgba8(),
            ))
        };
        let (a, d) = match files[i].tile.and_then(|_| decode(&files[i])) {
            Some(ad) => ad,
            None => {
                files[i] = built_in_files(dir, i, art);
                (
                    image::open(&files[i].albedo)
                        .map_err(|e| format!("{name}_albedo.png: {e}"))?
                        .to_rgba8(),
                    image::open(&files[i].normal)
                        .map_err(|e| format!("{name}_normal.png: {e}"))?
                        .to_rgba8(),
                )
            }
        };
        if let Some(t) = files[i].tile {
            tiles[i] = t;
        }
        if size == 0 {
            size = largest.unwrap_or(a.width());
        }
        let fit = |img: image::RgbaImage| {
            if img.width() == size && img.height() == size {
                img
            } else {
                image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle)
            }
        };
        let (a, d) = (fit(a), fit(d));
        if !size.is_power_of_two() {
            return Err(format!("{name}: textures must be power-of-two squares"));
        }
        let (ca, cd) = (
            build_mips(a.as_raw(), size, true, &lut),
            build_mips(d.as_raw(), size, false, &lut),
        );
        albedo.extend_from_slice(&ca);
        detail.extend_from_slice(&cd);
        chains.push((ca, cd));
        swatches.push(read_swatch(&files[i].swatch));
    }
    let mips = size.ilog2() + 1;
    info!(
        "loaded terrain textures ({}, {size}², {mips} mips) in {:?}",
        art.name(),
        t0.elapsed()
    );
    // the layers not compressed yet go to the background job (size 4+: whole BC blocks)
    let compress = (bc && size >= 4).then(|| {
        let keys: Vec<(u64, u64)> = files.iter().map(|f| layer_keys(f, size)).collect();
        let missing = chains
            .into_iter()
            .enumerate()
            .filter(|(i, _)| {
                compress::read_cache(keys[*i].0, size, size, mips).is_none()
                    || compress::read_cache(keys[*i].1, size, size, mips).is_none()
            })
            .map(|(i, (a, d))| (i, a, d))
            .collect();
        CompressJob {
            size,
            mips,
            keys,
            missing,
        }
    });
    Ok(Loaded {
        albedo: array_image(size, mips, albedo, TextureFormat::Rgba8UnormSrgb),
        detail: array_image(size, mips, detail, TextureFormat::Rgba8Unorm),
        swatches,
        tiles,
        compress,
    })
}

/// Block-compresses the missing layers (on about half the cores), caches them and builds the
/// compressed arrays from the cache.
fn compress_arrays(job: CompressJob) -> Result<(Image, Image), String> {
    use rayon::prelude::*;
    let t0 = std::time::Instant::now();
    let CompressJob {
        size,
        mips,
        keys,
        missing,
    } = job;
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .div_ceil(2)
        .max(2);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| e.to_string())?;
    let top = (size * size * 4) as usize;
    let psnr: Vec<(f64, f64)> = pool.install(|| {
        missing
            .par_iter()
            .map(|(i, a, d)| {
                let ba = compress::encode_chain(a, size, size, mips, ALBEDO_CODEC, Effort::High);
                let bd = compress::encode_chain(d, size, size, mips, DETAIL_CODEC, DETAIL_EFFORT);
                let lb = compress::level_bytes(size, size);
                let pa = compress::psnr(
                    &a[..top],
                    &compress::decode_level(&ba[..lb], size, size, ALBEDO_CODEC),
                    &[0, 1, 2, 3],
                );
                let pd = compress::psnr(
                    &d[..top],
                    &compress::decode_level(&bd[..lb], size, size, DETAIL_CODEC),
                    &[0, 1, 2, 3],
                );
                compress::write_cache(keys[*i].0, size, size, mips, &ba);
                compress::write_cache(keys[*i].1, size, size, mips, &bd);
                (pa, pd)
            })
            .collect()
    });
    let (mut albedo, mut detail) = (Vec::new(), Vec::new());
    for (ka, kd) in &keys {
        albedo.extend(
            compress::read_cache(*ka, size, size, mips)
                .ok_or("a compressed layer is missing from the cache")?,
        );
        detail.extend(
            compress::read_cache(*kd, size, size, mips)
                .ok_or("a compressed layer is missing from the cache")?,
        );
    }
    let min = |f: fn(&(f64, f64)) -> f64| psnr.iter().map(f).fold(f64::INFINITY, f64::min);
    info!(
        "terrain textures block-compressed ({} layers) in {:?}: PSNR min albedo {:.1} dB, detail {:.1} dB",
        missing.len(),
        t0.elapsed(),
        min(|p| p.0),
        min(|p| p.1)
    );
    Ok((
        array_image(size, mips, albedo, ALBEDO_CODEC.format(true)),
        array_image(size, mips, detail, DETAIL_CODEC.format(false)),
    ))
}
