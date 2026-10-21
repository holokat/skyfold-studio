//! The engine-independent half of the exporters: a snapshot of the world taken on the main
//! thread, and the building blocks every target writes from it on a background thread — the
//! resampled 16-bit heightmap with its exact height scale, splat weights, the world's chosen
//! photoscan layer textures, chunked terrain meshes, models as GLB (ez-tree trees, photoscans,
//! Poly Haven and food models; one file per asset, variants and LODs as named nodes), instance
//! lists, grass density, water and road meshes, gameplay data, lighting and credits.
//!
//! Coordinates everywhere are the editor's: metres, Y up, +X east, +Z south, right-handed (the
//! glTF convention); world (0, 0) is the map centre; heightmap pixel (0, 0) is the north-west
//! corner, columns run east and rows run south.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use serde_json::{Value, json};

use super::glb::{self, Glb, Material, Prim, Wrap};
use super::{ExportOptions, Progress};
use crate::gameplay::Gameplay;
use crate::look::LookParams;
use crate::objects::{Category, Placed};
use crate::procgen::meshes::AssetKind;
use crate::roads::{RoadKind, Roads};
use crate::sky::Lighting;
use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer, Surface};
use crate::terrain::textures::{
    LAYER_FILES, LAYER_TILE, LibraryMaterial, TerrainLayerChoices, lin_to_srgb, srgb_to_lin_lut,
};
use crate::water::{WaterBodies, WaterStyle};
use crate::world::Zone;

// ---------------------------------------------------------------------------------------------
// job context

/// Where a running export writes, how far it is, and whether it was cancelled.
pub struct Ctx {
    pub root: PathBuf,
    pub opts: ExportOptions,
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Arc<AtomicBool>,
    files: Mutex<Vec<(String, u64)>>,
    warnings: Mutex<Vec<String>>,
    /// the shared texture folder GLBs written with `write_glb` refer to (`textures`)
    pub shared: super::textures::Shared,
}

impl Ctx {
    pub fn new(
        opts: ExportOptions,
        progress: Arc<Mutex<Progress>>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        Self {
            root: opts.dir.clone(),
            opts,
            progress,
            cancel,
            files: Mutex::new(vec![]),
            warnings: Mutex::new(vec![]),
            shared: Default::default(),
        }
    }

    /// Writes a GLB, its images moved to the shared texture folder when the target enabled one.
    pub fn write_glb(&self, rel: &str, bytes: Vec<u8>) -> Result<(), String> {
        let b = self.shared.externalize(self, bytes, rel)?;
        self.write(rel, &b)
    }

    /// Reports a new stage (and stops here if the export was cancelled).
    pub fn stage(&self, what: &str, fraction: f32) -> Result<(), String> {
        self.check()?;
        if let Ok(mut p) = self.progress.lock() {
            p.stage = what.to_string();
            p.fraction = fraction.clamp(0.0, 1.0);
        }
        info!("export: {what}");
        Ok(())
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(super::CANCELLED.into())
        } else {
            Ok(())
        }
    }

    /// Writes a file below the export root (folders are created).
    pub fn write(&self, rel: &str, bytes: &[u8]) -> Result<(), String> {
        self.check()?;
        let p = self.root.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        if let Ok(mut f) = self.files.lock() {
            f.retain(|(r, _)| r != rel);
            f.push((rel.to_string(), bytes.len() as u64));
        }
        Ok(())
    }

    pub fn write_json(&self, rel: &str, v: &Value) -> Result<(), String> {
        self.write(
            rel,
            serde_json::to_string_pretty(v)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )
    }

    pub fn warn(&self, w: impl Into<String>) {
        let w = w.into();
        warn!("export: {w}");
        if let Ok(mut l) = self.warnings.lock() {
            l.push(w);
        }
    }

    pub fn files(&self) -> Vec<(String, u64)> {
        self.files.lock().map(|f| f.clone()).unwrap_or_default()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().map(|w| w.clone()).unwrap_or_default()
    }
}

/// Runs `f` for `0..n` on all cores, results in order.
pub fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map_or(4, |c| c.get())
        .clamp(1, 12);
    let chunk = n.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let f = &f;
        let handles: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|a| s.spawn(move || (a..(a + chunk).min(n)).map(f).collect::<Vec<T>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// A file / node name safe for every engine: letters, digits and `_`.
pub fn safe_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    match out.chars().next() {
        None => "world".into(),
        Some(c) if c.is_ascii_digit() => format!("_{out}"),
        _ => out,
    }
}

// ---------------------------------------------------------------------------------------------
// snapshot

/// Material settings of a library entry as the editor renders it.
#[derive(Clone, Copy, Debug)]
pub struct MatInfo {
    pub roughness: f32,
    /// alpha-tested cut-out
    pub mask: bool,
    /// both faces drawn
    pub two_sided: bool,
    /// back faces flip their normals (glTF `doubleSided`); leaves and photo cards keep them
    pub flip_back: bool,
}

impl MatInfo {
    const OPAQUE: MatInfo = MatInfo {
        roughness: 0.9,
        mask: false,
        two_sided: false,
        flip_back: false,
    };
    const CUTOUT: MatInfo = MatInfo {
        roughness: 0.75,
        mask: true,
        two_sided: true,
        flip_back: false,
    };

    /// glTF `doubleSided` for this material (see `Arrays::sides`).
    fn gltf_double_sided(self) -> bool {
        self.two_sided && self.flip_back
    }
}

/// LOD meshes (ez-tree trees, photoscans): distances and per-variant size from the library.
pub struct TreeInfo {
    pub ranges: [f32; 4],
    /// (height, width, lowest point) per variant
    pub variants: Vec<[f32; 3]>,
    pub bark: MatInfo,
    pub leaves: MatInfo,
}

/// A library asset the world uses.
pub struct AssetInfo {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub kind: Option<AssetKind>,
    pub import_path: Option<String>,
    pub radius: f32,
    pub walkable: bool,
    pub surface: Surface,
    pub tree: Option<TreeInfo>,
}

/// Everything the exporters read, copied off the main thread.
pub struct Snapshot {
    pub name: String,
    pub seed: u32,
    pub terrain: Terrain,
    pub layer_choices: TerrainLayerChoices,
    pub library: Vec<LibraryMaterial>,
    pub layer_surface: [Surface; LAYER_COUNT],
    pub assets: Vec<AssetInfo>,
    pub objects: Vec<Placed>,
    pub water: WaterBodies,
    pub ocean: WaterStyle,
    pub lighting: Lighting,
    pub look: LookParams,
    /// direction x, z, speed (m/s), gustiness
    pub wind: [f32; 4],
    pub draw_distance: f32,
    pub detail: f32,
    pub gameplay: Gameplay,
    pub roads: Roads,
    pub zones: Vec<Zone>,
    /// per-vertex grass map (density, dryness, tint, height) at the terrain's resolution
    pub grass: Option<Vec<[u8; 4]>>,
    pub grass_amount: f32,
    /// editor camera eye and focus
    pub camera: (Vec3, Vec3),
    /// procedural nature as density maps + rules (its instances are baked into `objects`)
    pub nature: Option<crate::nature::export::NatureExport>,
    /// caves, mines, sinkholes, arches (their recipes: the rock is rebuilt for the export)
    pub caves: crate::caves::Caves,
    /// terrain cells (i, j) the caves open: no terrain triangles there
    pub cave_holes: std::collections::HashSet<IVec2>,
    /// Skyfold Studio's navmesh files (`nav::capture`; empty when not asked for)
    pub navmesh: Vec<(String, Vec<u8>)>,
    /// the world's dungeons, their maps read from their folders (`dungeons::export::gather`;
    /// an error while a dungeon's map is open)
    pub dungeons: Result<Vec<crate::dungeons::export::DungeonExport>, String>,
    /// the placed objects' fires (`crate::flames`: fire anchors, painted flames, campfires)
    pub fires: Vec<crate::flames::Fire>,
}

pub fn snapshot(world: &World) -> Snapshot {
    let t = world.resource::<Terrain>();
    let mut terrain = Terrain::new(t.res, t.cell, t.sea_level);
    terrain.heights = t.heights.clone();
    terrain.splat = t.splat.clone();
    terrain.zones = t.zones.clone();
    terrain.biomes = t.biomes.clone();
    terrain.style = t.style.clone();
    // placed objects plus every procedural tree, rock, plant and piece of debris (baked)
    let nature_ctx = crate::nature::world_ctx(world);
    let mut objects = crate::nature::all_placed(
        world.resource::<crate::objects::ObjectStore>(),
        nature_ctx.as_ref(),
        &[0, 1, 2, 3],
    );
    // ore deposits are scanned rocks (their ore is in the gameplay data)
    objects.extend(crate::gameplay::deposits::export_objects(world));
    objects.sort_by_key(|o| o.id);
    let nature = nature_ctx.as_ref().map(crate::nature::export::export);
    // (the dungeons' objects use the world's models: they're written once, for both)
    let dungeons = crate::dungeons::export::gather(world);
    let mut used: Vec<&str> = objects.iter().map(|o| o.asset.as_str()).collect();
    if let Ok(list) = &dungeons {
        used.extend(
            list.iter()
                .flat_map(|d| d.objects.iter().map(|o| o.asset.as_str())),
        );
    }
    used.sort_unstable();
    used.dedup();
    let mats = world.get_resource::<Assets<crate::objects::FoliageMaterial>>();
    let mat_info = |h: &Handle<crate::objects::FoliageMaterial>| {
        mats.and_then(|m| m.get(h)).map(|m| MatInfo {
            roughness: m.base.perceptual_roughness,
            mask: matches!(m.base.alpha_mode, AlphaMode::Mask(_)),
            two_sided: m.base.cull_mode.is_none(),
            flip_back: m.base.double_sided,
        })
    };
    let assets = match world.get_resource::<crate::objects::Library>() {
        Some(lib) => used
            .iter()
            .filter_map(|id| lib.get(id))
            .map(|e| AssetInfo {
                id: e.id.clone(),
                name: e.name.clone(),
                category: e.category,
                kind: e.kind,
                import_path: e.import_path.clone(),
                radius: e.radius,
                walkable: e.walkable,
                surface: e.surface,
                tree: e.tree.as_ref().map(|t| TreeInfo {
                    ranges: t.ranges,
                    variants: t
                        .variants
                        .iter()
                        .map(|v| [v.height, v.width, v.min_y])
                        .collect(),
                    bark: mat_info(&t.bark).unwrap_or(MatInfo::OPAQUE),
                    leaves: mat_info(&t.leaves).unwrap_or(MatInfo::CUTOUT),
                }),
            })
            .collect(),
        None => vec![],
    };
    let lighting = world.resource::<Lighting>().clone();
    let look = lighting.look.effective();
    let wind = world.resource::<crate::objects::Wind>();
    let graphics = world.resource::<crate::streaming::Graphics>();
    let orbit = world.resource::<crate::camera::OrbitCamera>();
    let meta = world.resource::<crate::world::WorldMeta>();
    let fires = crate::flames::export_fires(world, &objects, None);
    Snapshot {
        name: meta.name.clone(),
        seed: meta.seed,
        terrain,
        layer_choices: world.resource::<TerrainLayerChoices>().clone(),
        library: world
            .resource::<crate::terrain::textures::TerrainLibrary>()
            .materials
            .clone(),
        layer_surface: world.resource::<crate::editor::Editor>().layer_surface,
        assets,
        objects,
        water: world.resource::<WaterBodies>().clone(),
        ocean: world.resource::<WaterStyle>().clone(),
        look,
        lighting,
        wind: [wind.dir.x, wind.dir.y, wind.speed, wind.gustiness],
        draw_distance: graphics.draw_distance,
        detail: graphics.detail,
        gameplay: world.resource::<Gameplay>().clone(),
        roads: world.resource::<Roads>().clone(),
        zones: world.resource::<crate::world::Zones>().list.clone(),
        grass: world
            .get_resource::<crate::grass::GrassField>()
            .and_then(|g| g.density_map())
            .filter(|(_, n)| *n == t.n())
            .map(|(d, _)| d.to_vec()),
        grass_amount: world.resource::<crate::objects::NatureAmounts>().grass,
        camera: (orbit.eye(), orbit.focus),
        nature,
        caves: world
            .get_resource::<crate::caves::Caves>()
            .cloned()
            .unwrap_or_default(),
        cave_holes: world
            .get_resource::<crate::caves::render::CaveWorld>()
            .map(|c| c.holes().clone())
            .unwrap_or_default(),
        navmesh: vec![],
        dungeons,
        fires,
    }
}

impl Snapshot {
    /// Objects of one category group, as chosen in the options.
    pub fn included(&self, a: &AssetInfo, o: &ExportOptions) -> bool {
        match a.category {
            Category::Trees => o.trees,
            Category::Plants => o.plants,
            Category::Rocks
            | Category::Debris
            | Category::Structures
            | Category::Props
            | Category::Imported => o.props,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// image helpers

pub fn png_rgb(img: &image::RgbImage) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgb8(img.clone()))
}

pub fn png_rgba(img: &image::RgbaImage) -> Vec<u8> {
    encode(image::DynamicImage::ImageRgba8(img.clone()))
}

pub fn png_gray8(w: u32, h: u32, px: Vec<u8>) -> Vec<u8> {
    image::GrayImage::from_raw(w, h, px)
        .map(|i| encode(image::DynamicImage::ImageLuma8(i)))
        .unwrap_or_default()
}

pub fn png_gray16(w: u32, h: u32, px: Vec<u16>) -> Vec<u8> {
    image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(w, h, px)
        .map(|i| encode(image::DynamicImage::ImageLuma16(i)))
        .unwrap_or_default()
}

fn encode(img: image::DynamicImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(vec![]);
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

pub fn jpeg(img: &image::RgbImage, quality: u8) -> Vec<u8> {
    let mut out = vec![];
    let _ = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(img);
    out
}

/// Scales a square-ish texture down so no side exceeds `max` (never up).
pub fn fit<P: image::Pixel + 'static>(
    img: image::ImageBuffer<P, Vec<P::Subpixel>>,
    max: u32,
) -> image::ImageBuffer<P, Vec<P::Subpixel>>
where
    P::Subpixel: 'static,
{
    if img.width().max(img.height()) <= max {
        return img;
    }
    let k = max as f32 / img.width().max(img.height()) as f32;
    let (w, h) = (
        ((img.width() as f32 * k).round() as u32).max(1),
        ((img.height() as f32 * k).round() as u32).max(1),
    );
    image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle)
}

/// Multiplies an sRGB image by a linear colour (tints above 1 brighten).
fn tint(img: &mut image::RgbaImage, t: [f32; 3]) {
    if t.iter().all(|v| (v - 1.0).abs() < 1e-3) {
        return;
    }
    let lut = srgb_to_lin_lut();
    for p in img.pixels_mut() {
        for c in 0..3 {
            p.0[c] = lin_to_srgb(lut[p.0[c] as usize] * t[c]);
        }
    }
}

/// Tangent-space XY (0..255, OpenGL convention) to an RGB normal map with Z rebuilt.
fn xy_to_normal(x: u8, y: u8) -> [u8; 3] {
    let (nx, ny) = (x as f32 / 255.0 * 2.0 - 1.0, y as f32 / 255.0 * 2.0 - 1.0);
    let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
    [x, y, ((nz * 0.5 + 0.5) * 255.0).round() as u8]
}

// ---------------------------------------------------------------------------------------------
// terrain

/// The terrain resampled for one target.
pub struct TerrainOut {
    /// vertices per side
    pub n: usize,
    pub size: f32,
    /// metres between vertices
    pub cell: f32,
    pub half: f32,
    pub lo: f32,
    pub hi: f32,
    pub sea: f32,
    /// n × n heights (m), rows north → south, columns west → east
    pub heights: Vec<f32>,
    /// layers with any weight, in layer order
    pub used: Vec<usize>,
    /// per used layer: n × n weights 0..255, summing to 255 at every vertex
    pub weights: Vec<Vec<u8>>,
    /// (n − 1)² cells, rows north → south: true where a cave opens the ground (no terrain there)
    pub holes: Vec<bool>,
}

impl TerrainOut {
    /// Is cell (i, j) open (a cave mouth)?
    pub fn hole(&self, i: usize, j: usize) -> bool {
        !self.holes.is_empty() && self.holes[j * (self.n - 1) + i]
    }
}

impl TerrainOut {
    /// 16-bit heights: `height = lo + value / 65535 * (hi - lo)`.
    pub fn heights16(&self) -> Vec<u16> {
        let range = (self.hi - self.lo).max(1e-3);
        self.heights
            .iter()
            .map(|h| {
                (((h - self.lo) / range) * 65535.0)
                    .round()
                    .clamp(0.0, 65535.0) as u16
            })
            .collect()
    }

    /// Smooth normal at a vertex.
    pub fn normal(&self, i: usize, j: usize) -> [f32; 3] {
        let n = self.n;
        let h = |i: usize, j: usize| self.heights[j * n + i];
        let (l, r) = (i.saturating_sub(1), (i + 1).min(n - 1));
        let (d, u) = (j.saturating_sub(1), (j + 1).min(n - 1));
        let dx = (h(r, j) - h(l, j)) / (self.cell * (r - l).max(1) as f32);
        let dz = (h(i, u) - h(i, d)) / (self.cell * (u - d).max(1) as f32);
        Vec3::new(-dx, 1.0, -dz).normalize().to_array()
    }

    pub fn meta(&self) -> Value {
        json!({
            "vertices_per_side": self.n,
            "size_m": self.size,
            "metres_per_vertex": self.cell,
            "origin_xz": [-self.half, -self.half],
            "height_min_m": self.lo,
            "height_max_m": self.hi,
            "height_decode": "height_m = height_min_m + value / 65535 * (height_max_m - height_min_m)",
            "sea_level_m": self.sea,
        })
    }
}

/// Heights and final ground-material weights (automatic style + paint) at `n` vertices per side.
pub fn terrain(s: &Snapshot, n: usize) -> TerrainOut {
    let t = &s.terrain;
    let nn = t.n();
    let size = t.size();
    let half = t.half();
    let cell = size / (n - 1) as f32;
    let native: Vec<Vec<[f32; LAYER_COUNT]>> =
        par_map(nn, |j| (0..nn).map(|i| t.final_weights(i, j)).collect());
    let max_w = native
        .iter()
        .flatten()
        .fold([0.0f32; LAYER_COUNT], |mut m, w| {
            for k in 0..LAYER_COUNT {
                m[k] = m[k].max(w[k]);
            }
            m
        });
    let used: Vec<usize> = (0..LAYER_COUNT)
        .filter(|&k| max_w[k] > 1.5 / 255.0)
        .collect();
    let used = if used.is_empty() { vec![0] } else { used };
    let rows: Vec<(Vec<f32>, Vec<Vec<u8>>)> = par_map(n, |j| {
        let mut hs = Vec::with_capacity(n);
        let mut ws = vec![Vec::with_capacity(n); used.len()];
        for i in 0..n {
            let (x, z) = (-half + i as f32 * cell, -half + j as f32 * cell);
            hs.push(t.height_at(x, z));
            // bilinear weights from the native grid
            let g = t.world_to_grid(x, z);
            let (gx, gz) = (
                g.x.clamp(0.0, (nn - 1) as f32),
                g.y.clamp(0.0, (nn - 1) as f32),
            );
            let (i0, j0) = (
                (gx.floor() as usize).min(nn - 2),
                (gz.floor() as usize).min(nn - 2),
            );
            let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
            let mut w = [0.0f32; LAYER_COUNT];
            for (di, dj, k) in [
                (0, 0, (1.0 - fx) * (1.0 - fz)),
                (1, 0, fx * (1.0 - fz)),
                (0, 1, (1.0 - fx) * fz),
                (1, 1, fx * fz),
            ] {
                let src = &native[j0 + dj][i0 + di];
                for l in 0..LAYER_COUNT {
                    w[l] += src[l] * k;
                }
            }
            for (k, q) in quantize(&used.iter().map(|&l| w[l]).collect::<Vec<_>>())
                .into_iter()
                .enumerate()
            {
                ws[k].push(q);
            }
        }
        (hs, ws)
    });
    let mut heights = Vec::with_capacity(n * n);
    let mut weights = vec![Vec::with_capacity(n * n); used.len()];
    for (hs, ws) in rows {
        heights.extend(hs);
        for (k, w) in ws.into_iter().enumerate() {
            weights[k].extend(w);
        }
    }
    let (lo, hi) = heights
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
    // cave mouths: a cell is open where its middle falls in a cell the caves open
    let holes = if s.cave_holes.is_empty() {
        vec![]
    } else {
        let c = n - 1;
        (0..c * c)
            .map(|k| {
                let (i, j) = (k % c, k / c);
                let (x, z) = (
                    -half + (i as f32 + 0.5) * cell,
                    -half + (j as f32 + 0.5) * cell,
                );
                let g = t.world_to_grid(x, z);
                s.cave_holes
                    .contains(&IVec2::new(g.x.floor() as i32, g.y.floor() as i32))
            })
            .collect()
    };
    TerrainOut {
        n,
        size,
        cell,
        half,
        lo,
        hi: hi.max(lo + 0.01),
        sea: t.sea_level,
        heights,
        used,
        weights,
        holes,
    }
}

/// Weights to bytes summing to exactly 255 (largest remainders get the leftover).
fn quantize(w: &[f32]) -> Vec<u8> {
    let sum: f32 = w.iter().map(|v| v.max(0.0)).sum();
    if sum <= 1e-6 {
        let mut out = vec![0u8; w.len()];
        out[0] = 255;
        return out;
    }
    let scaled: Vec<f32> = w.iter().map(|v| v.max(0.0) / sum * 255.0).collect();
    let mut out: Vec<u8> = scaled.iter().map(|v| v.floor() as u8).collect();
    let mut left = 255 - out.iter().map(|&v| v as u32).sum::<u32>() as i32;
    let mut order: Vec<usize> = (0..w.len()).collect();
    order.sort_by(|&a, &b| {
        (scaled[b] - scaled[b].floor()).total_cmp(&(scaled[a] - scaled[a].floor()))
    });
    for k in order.into_iter().cycle() {
        if left <= 0 {
            break;
        }
        out[k] = out[k].saturating_add(1);
        left -= 1;
    }
    out
}

/// One ground layer's textures, engine-ready: albedo (sRGB), an OpenGL-convention normal map and
/// a mask map in Unity HDRP / URP layout (R metallic, G AO, B height, A smoothness).
pub struct LayerTex {
    pub index: usize,
    /// file stem, e.g. `dry_grass`
    pub key: &'static str,
    pub name: &'static str,
    /// terrain library id, or `builtin`
    pub material: String,
    pub tile_m: f32,
    pub surface: Surface,
    pub albedo: image::RgbImage,
    pub normal: image::RgbImage,
    pub mask: image::RgbaImage,
}

/// The chosen material of each layer (library photoscan or the built-in one) at `size`.
pub fn layer_textures(s: &Snapshot, layers: &[usize], size: u32) -> Result<Vec<LayerTex>, String> {
    let out: Vec<Result<LayerTex, String>> =
        par_map(layers.len(), |k| layer_texture(s, layers[k], size));
    out.into_iter().collect()
}

pub fn layer_texture(s: &Snapshot, i: usize, size: u32) -> Result<LayerTex, String> {
    layer_texture_with(s, s.layer_choices.get(i), i, size)
}

/// Layer `i` wearing library material `choice` ("" = the built-in one).
fn layer_texture_with(s: &Snapshot, choice: &str, i: usize, size: u32) -> Result<LayerTex, String> {
    let lib_dir = crate::terrain::textures::library_dir();
    let chosen = s
        .library
        .iter()
        .find(|m| !choice.is_empty() && m.id == choice);
    // (a hand-painted world exports its painted layers: `art_style`)
    let art = crate::art_style::current();
    let from_lib = chosen.and_then(|m| {
        let d = lib_dir.join(&m.id);
        let [albedo, normal] =
            crate::art_style::textures([d.join("albedo.png"), d.join("normal.png")], art);
        Some((
            image::open(albedo).ok()?.to_rgba8(),
            image::open(normal).ok()?.to_rgba8(),
            m.id.clone(),
            m.tile_m.max(0.3),
        ))
    });
    let (a, d, material, tile_m) = match from_lib {
        Some(x) => x,
        None => {
            let dir = crate::terrain::textures::texture_dir();
            let name = LAYER_FILES[i];
            let [albedo, normal] = crate::art_style::textures(
                [
                    dir.join(format!("{name}_albedo.png")),
                    dir.join(format!("{name}_normal.png")),
                ],
                art,
            );
            (
                image::open(albedo)
                    .map_err(|e| format!("{name}_albedo.png: {e}"))?
                    .to_rgba8(),
                image::open(normal)
                    .map_err(|e| format!("{name}_normal.png: {e}"))?
                    .to_rgba8(),
                "builtin".to_string(),
                LAYER_TILE[i],
            )
        }
    };
    let (a, d) = (fit(a, size), fit(d, size));
    let d = if d.dimensions() == a.dimensions() {
        d
    } else {
        image::imageops::resize(
            &d,
            a.width(),
            a.height(),
            image::imageops::FilterType::Triangle,
        )
    };
    let (w, h) = a.dimensions();
    let mut albedo = image::RgbImage::new(w, h);
    let mut normal = image::RgbImage::new(w, h);
    let mut mask = image::RgbaImage::new(w, h);
    for (x, y, p) in a.enumerate_pixels() {
        let q = d.get_pixel(x, y).0;
        albedo.put_pixel(x, y, image::Rgb([p.0[0], p.0[1], p.0[2]]));
        // the editor stores green flipped (its shader maps image rows to +Z): flip back to OpenGL
        normal.put_pixel(x, y, image::Rgb(xy_to_normal(q[0], 255 - q[1])));
        mask.put_pixel(x, y, image::Rgba([0, q[3], p.0[3], 255 - q[2]]));
    }
    Ok(LayerTex {
        index: i,
        key: LAYER_FILES[i],
        name: Layer::from_index(i).name(),
        material,
        tile_m,
        surface: s.layer_surface[i],
        albedo,
        normal,
        mask,
    })
}

/// The terrain as a grid of mesh chunks (one GLB, a node `chunk_XX_ZZ` per chunk, world-space
/// vertices, UV 0..1 over the whole terrain for the splat maps).
pub fn terrain_glb(t: &TerrainOut, chunks: usize) -> Vec<u8> {
    let c = (t.n - 1) / chunks;
    let mut g = Glb::new();
    let mat = g.material(&Material {
        name: "terrain".into(),
        base_color: [0.45, 0.47, 0.38, 1.0],
        roughness: 0.9,
        ..Material::new("terrain")
    });
    let mut nodes = vec![];
    for cz in 0..chunks {
        for cx in 0..chunks {
            let (mut pos, mut nrm, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
            for j in cz * c..=(cz + 1) * c {
                for i in cx * c..=(cx + 1) * c {
                    pos.push([
                        -t.half + i as f32 * t.cell,
                        t.heights[j * t.n + i],
                        -t.half + j as f32 * t.cell,
                    ]);
                    nrm.push(t.normal(i, j));
                    uv.push([i as f32 / (t.n - 1) as f32, j as f32 / (t.n - 1) as f32]);
                }
            }
            let row = (c + 1) as u32;
            for j in 0..c as u32 {
                for i in 0..c as u32 {
                    // (a cave mouth: the cave meshes draw the ground there)
                    if t.hole(cx * c + i as usize, cz * c + j as usize) {
                        continue;
                    }
                    let (a, b, cc, d) = (
                        j * row + i,
                        j * row + i + 1,
                        (j + 1) * row + i,
                        (j + 1) * row + i + 1,
                    );
                    idx.extend_from_slice(&[a, cc, b, b, cc, d]);
                }
            }
            let name = format!("chunk_{cx:02}_{cz:02}");
            if let Some(m) = g.mesh(
                &name,
                &[Prim {
                    positions: &pos,
                    normals: Some(&nrm),
                    uvs: Some(&uv),
                    indices: &idx,
                    material: Some(mat),
                    ..Default::default()
                }],
            ) {
                nodes.push(g.node(&name, Some(m), vec![]));
            }
        }
    }
    let root = g.node("Terrain", None, nodes);
    g.finish(&[root])
}

/// Grass map (density × the world's grass amount, dryness, tint, height) at `n` vertices per side.
pub fn grass(s: &Snapshot, n: usize) -> Option<Vec<[u8; 4]>> {
    let src = s.grass.as_ref()?;
    let nn = s.terrain.n();
    let k = (nn - 1) as f32 / (n - 1) as f32;
    let amount = s.grass_amount.clamp(0.0, 2.0);
    Some(
        par_map(n, |j| {
            (0..n)
                .map(|i| {
                    let (gx, gz) = (i as f32 * k, j as f32 * k);
                    let (i0, j0) = (
                        (gx.floor() as usize).min(nn - 2),
                        (gz.floor() as usize).min(nn - 2),
                    );
                    let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
                    let mut v = [0.0f32; 4];
                    for (di, dj, w) in [
                        (0, 0, (1.0 - fx) * (1.0 - fz)),
                        (1, 0, fx * (1.0 - fz)),
                        (0, 1, (1.0 - fx) * fz),
                        (1, 1, fx * fz),
                    ] {
                        let p = src[(j0 + dj) * nn + i0 + di];
                        for c in 0..4 {
                            v[c] += p[c] as f32 * w;
                        }
                    }
                    v[0] = (v[0] * amount).min(255.0);
                    v.map(|c| c.round() as u8)
                })
                .collect::<Vec<_>>()
        })
        .into_iter()
        .flatten()
        .collect(),
    )
}

// ---------------------------------------------------------------------------------------------
// models

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Collider {
    None,
    /// a trunk capsule per variant (radius, height in `VariantOut`)
    Capsule,
    /// convex hull of the last level of detail
    Convex,
    /// the full triangle mesh (buildings, bridges)
    Mesh,
}

impl Collider {
    pub fn name(self) -> &'static str {
        match self {
            Collider::None => "none",
            Collider::Capsule => "capsule",
            Collider::Convex => "convex",
            Collider::Mesh => "mesh",
        }
    }
}

pub struct VariantOut {
    /// node holding the LOD nodes ("" for a packed model: the whole scene)
    pub node: String,
    /// LOD node names, finest first
    pub lods: Vec<String>,
    pub height: f32,
    pub width: f32,
    /// trunk capsule radius (trees)
    pub trunk: f32,
    /// collision proxy node (`<node>_COL`, a root of its own; "" = none, or the render meshes)
    pub collision: String,
}

/// One exported model file.
pub struct ModelOut {
    /// index into `Snapshot::assets`
    pub asset: usize,
    pub safe: String,
    /// path relative to the models folder
    pub file: String,
    /// generated by Skyfold Studio (variant + LOD nodes) rather than packed from a source glTF
    pub generated: bool,
    pub variants: Vec<VariantOut>,
    /// where each LOD stops being drawn (m); each LOD starts where the previous one ends
    pub lod_end: Vec<f32>,
    pub shadows: bool,
    pub collider: Collider,
    pub triangles: usize,
    pub bytes: usize,
}

impl ModelOut {
    pub fn json(&self, a: &AssetInfo, dir: &str, instances: usize) -> Value {
        json!({
            "id": a.id,
            "name": a.name,
            "safe_name": self.safe,
            "category": a.category.name(),
            "file": format!("{dir}/{}", self.file),
            "generated": self.generated,
            "variants": self.variants.iter().map(|v| json!({ "node": v.node, "lods": v.lods, "height_m": v.height, "width_m": v.width, "trunk_radius_m": v.trunk, "collision_node": v.collision })).collect::<Vec<_>>(),
            "lod_end_m": self.lod_end,
            "cast_shadows": self.shadows,
            "collider": self.collider.name(),
            "walkable": a.walkable,
            "footstep_surface": a.surface.sound_stem(),
            "physics": super::collision::physics_json(a.surface),
            "triangles": self.triangles,
            "bytes": self.bytes,
            "instances": instances,
        })
    }
}

/// Plain arrays of a Bevy mesh.
pub struct Arrays {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub tan: Vec<[f32; 4]>,
    pub uv: Vec<[f32; 2]>,
    pub idx: Vec<u32>,
}

pub fn arrays(m: &Mesh) -> Arrays {
    let f3 = |id| match m.attribute(id) {
        Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
        _ => vec![],
    };
    let tan = match m.attribute(Mesh::ATTRIBUTE_TANGENT) {
        Some(VertexAttributeValues::Float32x4(v)) => v.clone(),
        _ => vec![],
    };
    let uv = match m.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(v)) => v.clone(),
        _ => vec![],
    };
    let idx = m
        .indices()
        .map(|i| i.iter().map(|x| x as u32).collect())
        .unwrap_or_default();
    Arrays {
        pos: f3(Mesh::ATTRIBUTE_POSITION),
        nrm: f3(Mesh::ATTRIBUTE_NORMAL),
        tan,
        uv,
        idx,
    }
}

impl Arrays {
    /// Leaves and photo cards are drawn from both sides with their normals unflipped (lit like
    /// the editor, not dark underneath). glTF can't say that, so their triangles are doubled with
    /// the reverse winding and the material stays single-sided.
    fn sides(mut self, mi: MatInfo) -> Self {
        if mi.two_sided && !mi.flip_back {
            let back: Vec<u32> = self
                .idx
                .chunks_exact(3)
                .flat_map(|t| [t[0], t[2], t[1]])
                .collect();
            self.idx.extend(back);
        }
        self
    }

    fn prim(&self, material: usize) -> Prim<'_> {
        Prim {
            positions: &self.pos,
            normals: Some(&self.nrm),
            tangents: (!self.tan.is_empty()).then_some(&self.tan[..]),
            uvs: (!self.uv.is_empty()).then_some(&self.uv[..]),
            colors: None,
            indices: &self.idx,
            material: Some(material),
        }
    }
}

fn foliage_dir() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("textures/foliage")
}

/// Per-LOD end distances from the library's LOD ranges (LODs that would never show are dropped:
/// shrubs end where their last LOD starts).
fn lod_ends(ranges: [f32; 4], lods: usize, s: &Snapshot, cap: f32) -> Vec<f32> {
    let detail = s.detail.clamp(0.5, 2.0);
    let mut e: Vec<f32> = (0..lods)
        .map(|k| {
            if k + 1 < lods {
                ranges[k.min(2)]
            } else {
                ranges[3]
            }
        })
        .map(|d| (d * detail).min(cap))
        .collect();
    while e.len() >= 2 && e[e.len() - 1] <= e[e.len() - 2] {
        e.pop();
    }
    e
}

/// The image as a GLB texture: JPEG when opaque, PNG when its alpha matters.
fn embed(g: &mut Glb, img: &image::RgbaImage, alpha: bool, name: &str, wrap: Wrap) -> usize {
    let i = if alpha {
        g.image(&png_rgba(img), "image/png", name)
    } else {
        g.image(
            &jpeg(&image::DynamicImage::ImageRgba8(img.clone()).to_rgb8(), 90),
            "image/jpeg",
            name,
        )
    };
    g.texture(i, wrap)
}

fn load_rgba(path: PathBuf) -> Result<image::RgbaImage, String> {
    image::open(&path)
        .map(|i| i.to_rgba8())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Trunk capsule radius from the bark vertices 0.3–1.8 m up: the inner quarter (low conifer
/// branches start there too), at most a slender trunk's share of the height.
fn trunk_radius(bark: &Arrays, height: f32) -> f32 {
    let mut r: Vec<f32> = bark
        .pos
        .iter()
        .filter(|p| p[1] > 0.3 && p[1] < 1.8)
        .map(|p| p[0].hypot(p[2]))
        .collect();
    if r.is_empty() {
        return 0.2;
    }
    r.sort_by(f32::total_cmp);
    r[r.len() / 4].min(height * 0.035).clamp(0.06, 1.5)
}

fn tree_glb(
    a: &AssetInfo,
    safe: &str,
    sp: &crate::trees::SpeciesExport,
    tex: u32,
) -> Result<(Vec<u8>, Vec<VariantOut>), String> {
    let info = a.tree.as_ref();
    let (bark_mi, leaf_mi) = info
        .map(|t| (t.bark, t.leaves))
        .unwrap_or((MatInfo::OPAQUE, MatInfo::CUTOUT));
    let dir = foliage_dir();
    let art = crate::art_style::current();
    let mut g = Glb::new();
    // Some botanically distinct procedural trees generate their leaf card at runtime rather
    // than shipping a misleading scanned PNG. Export the exact renderer pixels for those cards;
    // existing photographed and painted foliage keeps the established disk-texture path.
    let leaf_source = crate::trees::generated_leaf_rgba(sp.leaf, art).map_or_else(
        || {
            load_rgba(crate::art_style::texture(
                &dir.join(format!("{}.png", sp.leaf)),
                art,
            ))
        },
        Ok,
    )?;
    let mut leaf = fit(leaf_source, tex);
    tint(&mut leaf, sp.leaf_tint);
    let leaf_tex = embed(&mut g, &leaf, true, &format!("{safe}_leaf"), Wrap::Clamp);
    let leaf_mat = g.material(&Material {
        base_tex: Some(leaf_tex),
        roughness: leaf_mi.roughness,
        mask: Some(0.5),
        double_sided: leaf_mi.gltf_double_sided(),
        ..Material::new(&format!("{safe}_leaves"))
    });
    let mut bark_img = None;
    let bark_mat = if sp.card {
        leaf_mat
    } else {
        let [albedo_path, normal_path] = crate::art_style::textures(
            [
                dir.join(format!("{}_albedo.png", sp.bark)),
                dir.join(format!("{}_normal.png", sp.bark)),
            ],
            art,
        );
        let mut albedo = fit(load_rgba(albedo_path)?, tex);
        tint(&mut albedo, sp.bark_tint);
        let normal = fit(load_rgba(normal_path)?, tex);
        let at = embed(
            &mut g,
            &albedo,
            false,
            &format!("{safe}_bark"),
            Wrap::Repeat,
        );
        let nt = embed(
            &mut g,
            &normal,
            false,
            &format!("{safe}_bark_normal"),
            Wrap::Repeat,
        );
        bark_img = Some(albedo);
        g.material(&Material {
            base_tex: Some(at),
            normal_tex: Some(nt),
            roughness: bark_mi.roughness,
            double_sided: bark_mi.gltf_double_sided(),
            ..Material::new(&format!("{safe}_bark"))
        })
    };
    let mut roots = vec![];
    let mut variants = vec![];
    for (vi, lods) in sp.variants.iter().enumerate() {
        let vname = format!("{safe}_v{vi}");
        let mut children = vec![];
        let mut names = vec![];
        let [height, width, min_y] = info
            .and_then(|t| t.variants.get(vi).copied())
            .unwrap_or([5.0, 3.0, 0.0]);
        let mut trunk = 0.2;
        for (li, (bark, leaves)) in lods.iter().enumerate() {
            let b = arrays(bark).sides(if sp.card { leaf_mi } else { bark_mi });
            if li == 0 && !sp.card {
                trunk = trunk_radius(&b, height);
            }
            let l = leaves.as_ref().map(|l| arrays(l).sides(leaf_mi));
            let mut prims = vec![b.prim(bark_mat)];
            if let Some(l) = &l {
                prims.push(l.prim(leaf_mat));
            }
            let name = format!("{vname}_LOD{li}");
            if let Some(m) = g.mesh(&name, &prims) {
                children.push(g.node(&name, Some(m), vec![]));
                names.push(name);
            }
        }
        // trees end in a crossed-quad picture of themselves (`impostor`) out to the draw distance
        if a.category == Category::Trees
            && !sp.card
            && let Some((bark, leaves)) = lods.first()
            && let Some(imp) = super::impostor::build(
                bark,
                leaves.as_ref(),
                bark_img.as_ref(),
                &leaf,
                height,
                width,
                min_y,
            )
        {
            let t = embed(
                &mut g,
                &imp.image,
                true,
                &format!("{vname}_impostor"),
                Wrap::Clamp,
            );
            let mat = g.material(&Material {
                base_tex: Some(t),
                roughness: 0.95,
                mask: Some(0.5),
                double_sided: true,
                ..Material::new(&format!("{vname}_impostor"))
            });
            let name = format!("{vname}_LOD{}", names.len());
            if let Some(m) = g.mesh(
                &name,
                &[Prim {
                    positions: &imp.pos,
                    normals: Some(&imp.nrm),
                    uvs: Some(&imp.uv),
                    indices: &imp.idx,
                    material: Some(mat),
                    ..Default::default()
                }],
            ) {
                children.push(g.node(&name, Some(m), vec![]));
                names.push(name);
            }
        }
        roots.push(g.node(&vname, None, children));
        variants.push(VariantOut {
            node: vname,
            lods: names,
            height,
            width,
            trunk,
            collision: String::new(),
        });
    }
    Ok((g.finish(&roots), variants))
}

fn prop_glb(
    info: Option<&TreeInfo>,
    safe: &str,
    p: &crate::photoscan::PropExport,
    tex: u32,
) -> Result<(Vec<u8>, Vec<VariantOut>), String> {
    let mi = info.map(|t| t.bark).unwrap_or(MatInfo::OPAQUE);
    let mut g = Glb::new();
    let wrap = if p.tiling { Wrap::Repeat } else { Wrap::Clamp };
    let (w, h, data) = &p.albedo;
    let albedo = fit(
        image::RgbaImage::from_raw(*w, *h, data.clone()).ok_or("bad albedo")?,
        tex,
    );
    let at = embed(&mut g, &albedo, mi.mask, &format!("{safe}_albedo"), wrap);
    let nt = match &p.normal {
        Some((w, h, rg)) => {
            let rgba: Vec<u8> = rg
                .chunks_exact(2)
                .flat_map(|c| {
                    let [r, g, b] = xy_to_normal(c[0], c[1]);
                    [r, g, b, 255]
                })
                .collect();
            let normal = fit(
                image::RgbaImage::from_raw(*w, *h, rgba).ok_or("bad normal")?,
                tex,
            );
            Some(embed(
                &mut g,
                &normal,
                false,
                &format!("{safe}_normal"),
                wrap,
            ))
        }
        None => None,
    };
    let mat = g.material(&Material {
        base_tex: Some(at),
        normal_tex: nt,
        roughness: mi.roughness,
        mask: mi.mask.then_some(0.5),
        double_sided: mi.gltf_double_sided(),
        ..Material::new(safe)
    });
    let mut roots = vec![];
    let mut variants = vec![];
    for (vi, lods) in p.variants.iter().enumerate() {
        let vname = format!("{safe}_v{vi}");
        let mut children = vec![];
        let mut names = vec![];
        for (li, m) in lods.iter().enumerate() {
            let arr = arrays(m).sides(mi);
            let name = format!("{vname}_LOD{li}");
            if let Some(mesh) = g.mesh(&name, &[arr.prim(mat)]) {
                children.push(g.node(&name, Some(mesh), vec![]));
                names.push(name);
            }
        }
        let [height, width, _] = info
            .and_then(|t| t.variants.get(vi).copied())
            .unwrap_or([1.0, 1.0, 0.0]);
        roots.push(g.node(&vname, None, children));
        variants.push(VariantOut {
            node: vname,
            lods: names,
            height,
            width,
            trunk: 0.0,
            collision: String::new(),
        });
    }
    Ok((g.finish(&roots), variants))
}

/// Writes one GLB per used, included asset into `dir`. Procedural placeholder kinds are skipped.
pub fn models(
    s: &Snapshot,
    ctx: &Ctx,
    dir: &str,
    f0: f32,
    f1: f32,
) -> Result<Vec<ModelOut>, String> {
    let todo: Vec<usize> = (0..s.assets.len())
        .filter(|&i| s.included(&s.assets[i], &ctx.opts))
        .collect();
    let tex = ctx.opts.texture_size;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Mutex<Vec<ModelOut>> = Mutex::new(vec![]);
    let first_err: Mutex<Option<String>> = Mutex::new(None);
    // a few assets at a time: trees hold a lot of geometry while they are written
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&ai) = todo.get(k) else { break };
                    if ctx.check().is_err() {
                        break;
                    }
                    match model(s, ai, tex, ctx) {
                        Ok(Some(m)) => {
                            if let Err(e) =
                                ctx.write_glb(&format!("{dir}/{}", m.out.file), m.bytes_vec)
                            {
                                first_err.lock().unwrap().get_or_insert(e);
                                break;
                            }
                            results.lock().unwrap().push(m.out);
                        }
                        Ok(None) => {}
                        Err(e) => ctx.warn(format!("{}: {e}", s.assets[ai].name)),
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = ctx.stage(
                        &format!("Models {d}/{}", todo.len()),
                        f0 + (f1 - f0) * d as f32 / todo.len().max(1) as f32,
                    );
                }
            });
        }
    });
    if let Some(e) = first_err.into_inner().unwrap_or(None) {
        return Err(e);
    }
    ctx.check()?;
    let mut out = results.into_inner().unwrap_or_default();
    out.sort_by_key(|m| m.asset);
    Ok(out)
}

pub(super) struct Built {
    pub(super) out: ModelOut,
    pub(super) bytes_vec: Vec<u8>,
}

pub(super) fn model(s: &Snapshot, ai: usize, tex: u32, ctx: &Ctx) -> Result<Option<Built>, String> {
    let a = &s.assets[ai];
    let safe = safe_name(&a.id);
    let draw = s.draw_distance.max(200.0);
    // trees are drawn as meshes all the way to the draw distance (engines have no shared
    // impostor system; Unity's terrain billboards don't apply to trees with LOD groups): their
    // far LOD is light (about a seventh of LOD 0)
    let tree_cap = draw.max(600.0);
    let (bytes, variants, generated, lod_end) = if let Some(k) = a.kind {
        if let Some(sp) = crate::trees::export_species(&[k]).into_iter().next() {
            let (b, v) = tree_glb(a, &safe, &sp, tex)?;
            let lods = v.first().map_or(1, |v| v.lods.len());
            let cap = if a.category == Category::Trees {
                tree_cap
            } else {
                draw
            };
            let ranges = a
                .tree
                .as_ref()
                .map_or([40.0, 110.0, 320.0, 1500.0], |t| t.ranges);
            let ends = if lods == sp.variants.first().map_or(0, |l| l.len()) + 1 {
                // the far mesh for a stretch past LOD 2, then the impostor to the draw distance
                let d = s.detail.clamp(0.5, 2.0);
                let far = (ranges[2] * 2.5 * d)
                    .clamp(ranges[2] * d + 50.0, (cap * 0.6).max(ranges[2] * d + 50.0));
                vec![ranges[0] * d, ranges[1] * d, ranges[2] * d, far, cap]
            } else {
                lod_ends(ranges, lods, s, cap)
            };
            (b, v, true, ends)
        } else if let Some(p) = crate::flora::export_props(&[k], crate::art_style::current())
            .into_iter()
            .next()
        {
            let (b, v) = prop_glb(a.tree.as_ref(), &safe, &p, tex)?;
            let lods = v.first().map_or(1, |v| v.lods.len());
            let ranges = a
                .tree
                .as_ref()
                .map_or([20.0, 48.0, 78.0, 78.0], |t| t.ranges);
            (b, v, true, lod_ends(ranges, lods, s, draw))
        } else if let Some(p) = crate::photoscan::export_props(&[k]).into_iter().next() {
            let (b, v) = prop_glb(a.tree.as_ref(), &safe, &p, tex)?;
            let lods = v.first().map_or(1, |v| v.lods.len());
            let ranges = a
                .tree
                .as_ref()
                .map_or([30.0, 120.0, 1000.0, 1000.0], |t| t.ranges);
            (b, v, true, lod_ends(ranges, lods, s, draw))
        } else {
            ctx.warn(format!(
                "{} is a procedural placeholder model: not exported (its instances are listed)",
                a.name
            ));
            return Ok(None);
        }
    } else if let Some(p) = &a.import_path {
        let path = crate::asset_file(p);
        let b = glb::pack(&path, tex)?;
        let end = if a.category == Category::Props {
            90.0 * s.detail
        } else {
            draw
        };
        // levels of detail and a variant node like the generated models (`lods::packed`)
        match super::lods::packed(&b, &safe, end, ctx.opts.target == super::Target::Unreal) {
            Some(p) => (p.bytes, vec![p.variant], false, p.lod_end),
            None => (
                b,
                vec![VariantOut {
                    node: String::new(),
                    lods: vec![],
                    height: a.radius * 2.0,
                    width: a.radius * 2.0,
                    trunk: 0.0,
                    collision: String::new(),
                }],
                false,
                vec![end],
            ),
        }
    } else {
        return Ok(None);
    };
    let collider = match a.category {
        Category::Trees => Collider::Capsule,
        Category::Plants => Collider::None,
        Category::Rocks | Category::Debris | Category::Props => Collider::Convex,
        Category::Structures | Category::Imported => super::collision::kind(
            a,
            variants
                .first()
                .map_or(a.radius * 2.0, |v| v.height.max(v.width)),
        ),
    };
    let collider = if ctx.opts.collision {
        collider
    } else {
        Collider::None
    };
    // simplified collision proxies (`collision::add_proxies`)
    let mut variants = variants;
    let bytes = super::collision::add_proxies(bytes, &mut variants, collider);
    let out = ModelOut {
        asset: ai,
        file: format!("{safe}.glb"),
        safe,
        generated,
        variants,
        lod_end,
        shadows: a.category != Category::Plants,
        collider,
        triangles: glb::triangles(&bytes),
        bytes: bytes.len(),
    };
    Ok(Some(Built {
        out,
        bytes_vec: bytes,
    }))
}

/// The photoscanned grass tufts (tall grass cards) as a model of their own, for engines that
/// scatter grass from the grass map (Unity terrain details). `None` if the scan is missing.
pub fn grass_tufts(tex: u32) -> Option<(Vec<u8>, Vec<VariantOut>)> {
    let p = crate::photoscan::export_props(&[AssetKind::TallGrass])
        .into_iter()
        .next()?;
    let info = TreeInfo {
        ranges: [75.0; 4],
        variants: p.variants.iter().map(|_| [0.8, 0.8, 0.0]).collect(),
        bark: MatInfo {
            roughness: 0.8,
            ..MatInfo::CUTOUT
        },
        leaves: MatInfo::CUTOUT,
    };
    prop_glb(Some(&info), "grass_tufts", &p, tex.min(1024)).ok()
}

/// Instances per exported model (same order as `models`).
pub struct Inst {
    pub variant: u32,
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    pub scale: f32,
    /// Non-uniform objects: per-axis factors on top of `scale` (model axes).
    pub stretch: Option<[f32; 3]>,
}

pub fn instances(s: &Snapshot, models: &[ModelOut]) -> Vec<Vec<Inst>> {
    let by_id: HashMap<&str, usize> = models
        .iter()
        .enumerate()
        .map(|(k, m)| (s.assets[m.asset].id.as_str(), k))
        .collect();
    let mut out: Vec<Vec<Inst>> = (0..models.len()).map(|_| vec![]).collect();
    for o in &s.objects {
        if let Some(&k) = by_id.get(o.asset.as_str()) {
            let nv = models[k].variants.len().max(1) as u32;
            out[k].push(Inst {
                variant: o.variant.min(nv - 1),
                pos: o.pos,
                rot: o.rot,
                scale: o.scale,
                stretch: o.stretch,
            });
        }
    }
    out
}

/// One instance as `variant, x, y, z, qx, qy, qz, qw, scale`, rounded to millimetres and five
/// decimals (keeps the JSON compact).
pub fn inst_row(i: &Inst, pos: [f32; 3], rot: [f32; 4]) -> [f64; 9] {
    let r = |v: f32, k: f64| (v as f64 * k).round() / k;
    [
        i.variant as f64,
        r(pos[0], 1e3),
        r(pos[1], 1e3),
        r(pos[2], 1e3),
        r(rot[0], 1e5),
        r(rot[1], 1e5),
        r(rot[2], 1e5),
        r(rot[3], 1e5),
        r(i.scale, 1e4),
    ]
}

/// Non-uniformly scaled instances of a list as `row, sx, sy, sz` (row: the instance's index;
/// the factors multiply its uniform scale along the model's own axes). Empty for most models.
pub fn stretch_rows(list: &[Inst]) -> Vec<f64> {
    let r = |v: f32| (v as f64 * 1e4).round() / 1e4;
    list.iter()
        .enumerate()
        .filter_map(|(k, i)| i.stretch.map(|s| [k as f64, r(s[0]), r(s[1]), r(s[2])]))
        .flatten()
        .collect()
}

/// `instances.json`: per model a flat array of `variant, x, y, z, qx, qy, qz, qw, scale`, plus
/// `stretch` rows for the few non-uniformly scaled instances.
pub fn instances_json(s: &Snapshot, models: &[ModelOut], inst: &[Vec<Inst>], dir: &str) -> Value {
    json!({
        "format": "variant, x, y, z, qx, qy, qz, qw, scale per instance (world metres, rotation quaternion, uniform scale); optional `stretch`: row, sx, sy, sz for instances scaled per axis (factors on top of scale, model axes)",
        "models": models.iter().zip(inst).map(|(m, list)| {
            let mut v = m.json(&s.assets[m.asset], dir, list.len());
            v["data"] = json!(list.iter().flat_map(|i| inst_row(i, i.pos, i.rot)).collect::<Vec<f64>>());
            let st = stretch_rows(list);
            if !st.is_empty() {
                v["stretch"] = json!(st);
            }
            v
        }).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------------
// water

pub struct BodyOut {
    pub kind: &'static str,
    pub name: String,
    pub node: String,
    pub style: WaterStyle,
}

/// Lakes, rivers and waterfalls as one GLB (a node per body) and their styles.
pub fn water(s: &Snapshot) -> (Option<Vec<u8>>, Vec<BodyOut>) {
    let bodies = crate::water::export_bodies(&s.water, &s.terrain);
    if bodies.is_empty() {
        return (None, vec![]);
    }
    let mut g = Glb::new();
    let mut nodes = vec![];
    let mut out = vec![];
    for (k, (kind, name, mesh, style)) in bodies.into_iter().enumerate() {
        let arr = arrays(&mesh);
        let c = style.shallow;
        let mat = g.material(&Material {
            base_color: [c[0], c[1], c[2], 1.0],
            roughness: style.roughness.clamp(0.02, 1.0),
            ..Material::new(&format!("water_{k}"))
        });
        let node = format!("{kind}_{k:02}_{}", safe_name(&name));
        let nrm = if arr.nrm.len() == arr.pos.len() {
            arr.nrm.clone()
        } else {
            vec![[0.0, 1.0, 0.0]; arr.pos.len()]
        };
        if let Some(m) = g.mesh(
            &node,
            &[Prim {
                positions: &arr.pos,
                normals: Some(&nrm),
                uvs: (!arr.uv.is_empty()).then_some(&arr.uv[..]),
                indices: &arr.idx,
                material: Some(mat),
                ..Default::default()
            }],
        ) {
            nodes.push(g.node(&node, Some(m), vec![]));
            out.push(BodyOut {
                kind,
                name,
                node,
                style,
            });
        }
    }
    if nodes.is_empty() {
        return (None, vec![]);
    }
    let root = g.node("Water", None, nodes);
    (Some(g.finish(&[root])), out)
}

pub fn style_json(st: &WaterStyle) -> Value {
    let mut v = serde_json::to_value(st).unwrap_or(Value::Null);
    let (absorb, scatter) = crate::water::water_optics(st);
    v["absorption_per_m"] = json!(absorb.to_array());
    v["scattering"] = json!(scatter.to_array());
    v
}

/// A tileable ripple normal map (OpenGL convention) for the engines' water materials.
pub fn water_normal_png(size: u32) -> Vec<u8> {
    let mut rng = crate::noise::Rng::new(9137);
    // integer wave vectors keep it seamless
    let waves: Vec<(f32, f32, f32, f32)> = (0..40)
        .map(|_| {
            let r = 3.0 + rng.f32() * 26.0;
            let a = rng.f32() * std::f32::consts::TAU;
            let (kx, kz) = ((r * a.cos()).round(), (r * a.sin()).round());
            let k = kx.hypot(kz).max(1.0);
            (kx, kz, rng.f32() * std::f32::consts::TAU, 1.0 / k.powf(1.6))
        })
        .collect();
    let tau = std::f32::consts::TAU;
    let mut img = image::RgbImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let (mut dx, mut dz) = (0.0, 0.0);
            for &(kx, kz, ph, amp) in &waves {
                let c = (tau * (kx * u + kz * v) + ph).cos() * amp * tau;
                dx += c * kx;
                dz += c * kz;
            }
            let n = Vec3::new(-dx * 0.012, 1.0, -dz * 0.012).normalize();
            // tangent space: x along u, y up the image (OpenGL), z out of the surface
            img.put_pixel(
                x,
                y,
                image::Rgb([
                    ((n.x * 0.5 + 0.5) * 255.0) as u8,
                    ((-n.z * 0.5 + 0.5) * 255.0) as u8,
                    ((n.y * 0.5 + 0.5) * 255.0) as u8,
                ]),
            );
        }
    }
    png_rgb(&img)
}

// ---------------------------------------------------------------------------------------------
// roads

pub struct RoadOut {
    pub id: u32,
    pub name: String,
    pub kind: RoadKind,
    pub width: f32,
    pub length: f32,
    /// sampled on the road surface, including elevated authored decks
    pub centreline: Vec<[f32; 3]>,
    pub node: String,
}

/// The library material (falling back to a built-in layer) each road surface wears.
fn road_material(kind: RoadKind) -> (&'static str, usize) {
    match kind {
        RoadKind::Trail => ("muddy_cart_track", Layer::Dirt as usize),
        RoadKind::Footpath => ("gravel_path", Layer::Gravel as usize),
        RoadKind::DirtRoad => ("muddy_cart_track", Layer::Dirt as usize),
        RoadKind::Gravel => ("gravel_path", Layer::Gravel as usize),
        RoadKind::Cobblestone => ("rural_cobbles", Layer::Cobblestone as usize),
        RoadKind::PavedStone => ("slate_flagstones", Layer::Cobblestone as usize),
    }
}

/// Road ribbons draped on the terrain (one GLB, a node per road, textured per surface kind).
pub fn roads(s: &Snapshot, tex: u32) -> (Option<Vec<u8>>, Vec<RoadOut>) {
    let t = &s.terrain;
    let list: Vec<&crate::roads::Road> = s
        .roads
        .roads
        .iter()
        .filter(|r| r.points.len() >= 2)
        .collect();
    if list.is_empty() {
        return (None, vec![]);
    }
    let mut g = Glb::new();
    let mut mats: HashMap<RoadKind, (usize, f32)> = HashMap::new();
    let mut nodes = vec![];
    let mut out = vec![];
    for r in list {
        let (mat, tile) = *mats.entry(r.kind).or_insert_with(|| {
            // a road surface is a terrain library material (or the layer's own when missing)
            let (lib_id, layer) = road_material(r.kind);
            let choice = if s.library.iter().any(|m| m.id == lib_id) {
                lib_id
            } else {
                s.layer_choices.get(layer)
            };
            match layer_texture_with(s, choice, layer, tex) {
                Ok(lt) => {
                    let a = image::DynamicImage::ImageRgb8(lt.albedo).to_rgba8();
                    let n = image::DynamicImage::ImageRgb8(lt.normal).to_rgba8();
                    let at = embed(
                        &mut g,
                        &a,
                        false,
                        &format!("road_{:?}", r.kind),
                        Wrap::Repeat,
                    );
                    let nt = embed(
                        &mut g,
                        &n,
                        false,
                        &format!("road_{:?}_normal", r.kind),
                        Wrap::Repeat,
                    );
                    (
                        g.material(&Material {
                            base_tex: Some(at),
                            normal_tex: Some(nt),
                            roughness: 0.85,
                            ..Material::new(&format!("road_{:?}", r.kind))
                        }),
                        lt.tile_m,
                    )
                }
                Err(_) => (
                    g.material(&Material::new(&format!("road_{:?}", r.kind))),
                    2.0,
                ),
            }
        });
        // Sheet bridges carry an explicit deck profile. Keep that same water-cleared profile
        // in exported ribbons and navigation centrelines instead of snapping it to the bed.
        let authored = r.has_valid_authored_route();
        let mut authored_ranges = Vec::new();
        let line = if authored {
            let base = crate::roads::ground::Base {
                t,
                g: &s.roads.ground,
            };
            let plan = crate::roads::plan::plan_road(r, &base, &s.water, &[]);
            authored_ranges = plan.authored_deck_ranges();
            plan.samples
                .iter()
                .zip(&plan.h)
                .map(|(sample, y)| Vec3::new(sample.p.x, *y, sample.p.y))
                .collect()
        } else {
            r.centreline(2.0)
        };
        let across = 4usize;
        let (mut pos, mut nrm, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
        let mut dist = 0.0;
        let mut centre = vec![];
        for (k, p) in line.iter().enumerate() {
            if k > 0 {
                dist += p.xz().distance(line[k - 1].xz());
            }
            let a = line[k.saturating_sub(1)];
            let b = line[(k + 1).min(line.len() - 1)];
            let tan = (b - a).xz().normalize_or(Vec2::X);
            let side = Vec2::new(-tan.y, tan.x);
            for c in 0..=across {
                let o = (c as f32 / across as f32 - 0.5) * r.width;
                let q = p.xz() + side * o;
                let (y, normal) = if authored {
                    let across = Vec3::new(side.x, 0.0, side.y);
                    (p.y, across.cross(b - a).normalize_or(Vec3::Y))
                } else {
                    (t.height_at(q.x, q.y), t.normal_at(q.x, q.y))
                };
                pos.push([q.x, y + 0.06, q.y]);
                nrm.push(normal.to_array());
                uv.push([(o + r.width * 0.5) / tile, dist / tile]);
            }
            centre.push([p.x, if authored { p.y } else { t.height_at(p.x, p.z) }, p.z]);
        }
        let row = (across + 1) as u32;
        for k in 0..line.len() as u32 - 1 {
            if authored
                && !authored_ranges
                    .iter()
                    .any(|&(a, b)| (k as usize) >= a && (k as usize) < b)
            {
                continue;
            }
            for c in 0..across as u32 {
                let (a, b, cc, d) = (
                    k * row + c,
                    k * row + c + 1,
                    (k + 1) * row + c,
                    (k + 1) * row + c + 1,
                );
                idx.extend_from_slice(&[a, b, cc, b, d, cc]);
            }
        }
        let node = format!("road_{}_{}", r.id, safe_name(&r.name));
        if let Some(m) = g.mesh(
            &node,
            &[Prim {
                positions: &pos,
                normals: Some(&nrm),
                uvs: Some(&uv),
                indices: &idx,
                material: Some(mat),
                ..Default::default()
            }],
        ) {
            nodes.push(g.node(&node, Some(m), vec![]));
            out.push(RoadOut {
                id: r.id,
                name: r.name.clone(),
                kind: r.kind,
                width: r.width,
                length: dist,
                centreline: centre,
                node,
            });
        }
    }
    if nodes.is_empty() {
        return (None, vec![]);
    }
    let root = g.node("Roads", None, nodes);
    (Some(g.finish(&[root])), out)
}

pub fn roads_json(r: &[RoadOut]) -> Value {
    json!(r.iter().map(|r| json!({
        "id": r.id, "name": r.name, "kind": format!("{:?}", r.kind), "kind_name": r.kind.name(), "width_m": r.width,
        "length_m": r.length, "travel_factor": r.kind.travel_factor(), "node": r.node, "centreline": r.centreline,
    })).collect::<Vec<_>>())
}

// ---------------------------------------------------------------------------------------------
// gameplay

/// One gameplay item as an engine marker.
pub struct Marker {
    /// player_start, poi, npc, spawn_region, resource, fishing
    pub kind: &'static str,
    pub id: u32,
    pub name: String,
    /// POI kind / NPC role / resource kind / species
    pub sub: String,
    pub pos: [f32; 3],
    /// facing (radians, 0 = +X, counter-clockwise seen from above)
    pub yaw: f32,
    pub radius: f32,
    pub notes: String,
    /// spawn regions: outline in world xz
    pub polygon: Vec<[f32; 2]>,
}

pub fn markers(s: &Snapshot) -> Vec<Marker> {
    let t = &s.terrain;
    let g = &s.gameplay;
    let ground = |x: f32, z: f32| t.height_at(x, z);
    let mut out = vec![];
    if let Some(p) = g.player_start {
        out.push(Marker {
            kind: "player_start",
            id: 0,
            name: "Player Start".into(),
            sub: String::new(),
            pos: p.pos,
            yaw: p.yaw,
            radius: 1.0,
            notes: String::new(),
            polygon: vec![],
        });
    }
    for p in &g.pois {
        out.push(Marker {
            kind: "poi",
            id: p.id,
            name: p.name.clone(),
            sub: p.kind.name().into(),
            pos: p.pos,
            yaw: 0.0,
            radius: p.radius,
            notes: p.notes.clone(),
            polygon: vec![],
        });
    }
    for n in &g.npcs {
        out.push(Marker {
            kind: "npc",
            id: n.id,
            name: n.name.clone(),
            sub: n.role.name().into(),
            pos: n.pos,
            yaw: n.yaw,
            radius: 0.5,
            notes: n.notes.clone(),
            polygon: vec![],
        });
    }
    for r in &g.spawns {
        let c = r.centroid().unwrap_or_default();
        out.push(Marker {
            kind: "spawn_region",
            id: r.id,
            name: r.name.clone(),
            sub: format!("{:?}", r.behaviour),
            pos: [c.x, ground(c.x, c.y), c.y],
            yaw: 0.0,
            radius: (r.area() / std::f32::consts::PI).sqrt(),
            notes: r.notes.clone(),
            polygon: r.polygon.clone(),
        });
    }
    for n in &g.nodes {
        let notes = n.ore().map_or_else(
            || format!("tier {}", n.tier),
            |o| format!("tier {} · ore {} ({})", o.tier(), o.id(), o.rarity().name()),
        );
        out.push(Marker {
            kind: "resource",
            id: n.id,
            name: n.variant.clone(),
            sub: n.kind.name().into(),
            pos: n.pos,
            yaw: 0.0,
            radius: 1.0,
            notes,
            polygon: vec![],
        });
    }
    for f in &s.water.fish {
        out.push(Marker {
            kind: "fishing",
            id: f.id,
            name: format!("{} school", f.species.name()),
            sub: f.species.name().into(),
            pos: f.center,
            yaw: 0.0,
            radius: f.radius,
            notes: f.species.habitat().into(),
            polygon: vec![],
        });
    }
    out
}

/// `gameplay.json`: the documented server format (`docs/server-data.md`, the same file the
/// Gameplay tab's "Export server data" writes): every item with its zone and level range.
pub fn gameplay_json(s: &Snapshot) -> Value {
    let zones = crate::world::Zones {
        list: s.zones.clone(),
        selected: None,
    };
    let ground = crate::gameplay::edit::Ground {
        terrain: &s.terrain,
        zones: &zones,
        water: &s.water,
        nature: None,
    };
    crate::gameplay::export::server_data(&s.gameplay, &ground, &s.name)
}

/// Every gameplay item as an engine marker (`markers.json` of the generic package).
pub fn markers_json(s: &Snapshot) -> Value {
    json!({
        "note": "player start, POIs, NPCs, spawn regions (with outline), resource nodes and fishing spots as plain markers; gameplay.json has the full data",
        "markers": markers(s).iter().map(|m| json!({ "kind": m.kind, "id": m.id, "name": m.name, "sub": m.sub, "position": m.pos, "yaw": m.yaw, "radius_m": m.radius, "notes": m.notes, "polygon": m.polygon })).collect::<Vec<_>>(),
    })
}

// ---------------------------------------------------------------------------------------------
// lighting

/// The world's look at its time of day, in engine-neutral terms.
pub struct LightOut {
    pub hour: f32,
    /// unit vector towards the sun
    pub sun_dir: Vec3,
    pub elevation_deg: f32,
    /// compass bearing of the sun (0 north = -Z, 90 east = +X)
    pub azimuth_deg: f32,
    /// linear RGB, brightest channel 1
    pub sun_color: [f32; 3],
    /// 1 = the editor's default sun
    pub sun_strength: f32,
    pub night: f32,
    pub fog: bool,
    /// exponential-squared density (per m), as the editor's distance haze
    pub fog_density_exp2: f32,
    /// linear RGB
    pub fog_color: [f32; 3],
    /// Whether the world's outdoor haze effects are enabled.
    pub haze_enabled: bool,
    /// Effective master haze amount (zero when haze is disabled).
    pub haze: f32,
    pub clouds: f32,
    /// stops (+ brighter)
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub bloom: f32,
    pub vignette: f32,
}

impl LightOut {
    /// Exponential (not squared) density matching the haze at `d` metres.
    pub fn fog_density_exp(&self, d: f32) -> f32 {
        self.fog_density_exp2 * self.fog_density_exp2 * d
    }

    pub fn json(&self, s: &Snapshot) -> Value {
        json!({
            "hour": self.hour,
            "sun_direction": self.sun_dir.to_array(),
            "sun_elevation_deg": self.elevation_deg,
            "sun_azimuth_deg": self.azimuth_deg,
            "sun_color_linear": self.sun_color,
            "sun_strength": self.sun_strength,
            "sun_illuminance_lux": 110_000.0 * self.sun_strength,
            "night": self.night,
            "fog": { "enabled": self.fog, "density_exp2_per_m": self.fog_density_exp2, "color_linear": self.fog_color },
            "haze_enabled": self.haze_enabled,
            "haze": self.haze,
            "cloud_coverage": self.clouds,
            "grade": { "exposure_stops": self.exposure, "contrast": self.contrast, "saturation": self.saturation, "bloom": self.bloom, "vignette": self.vignette },
            "editor_lighting": &s.lighting,
            "look": &s.look,
            "wind": { "direction_xz": [s.wind[0], s.wind[1]], "speed_mps": s.wind[2], "gustiness": s.wind[3] },
        })
    }
}

pub fn lighting(s: &Snapshot) -> LightOut {
    let l = &s.lighting;
    let lk = &s.look;
    let sun = crate::sky::sun_direction(l.hour, l.sun_tilt, l.sun_azimuth);
    let elevation = sun.y.clamp(-1.0, 1.0).asin();
    let night = 1.0 - crate::noise::smoothstep(-0.2, 0.03, sun.y);
    // atmospheric reddening along the air mass (Kasten–Young), white at the zenith
    let e_deg = elevation.to_degrees().max(-2.0);
    let air = 1.0
        / ((e_deg.to_radians().sin()).max(0.0)
            + 0.50572 * (e_deg + 6.07995).max(0.1).powf(-1.6364));
    let tau = [0.03f32, 0.075, 0.17];
    let atm = tau.map(|t| (-(air - 1.0).max(0.0) * t).exp());
    let sc = lk.sun_color();
    let raw = [atm[0] * sc.x, atm[1] * sc.y, atm[2] * sc.z];
    let m = raw.iter().cloned().fold(1e-4, f32::max);
    let above = crate::noise::smoothstep(-0.06, 0.02, sun.y);
    let density = if l.haze_enabled {
        (0.000_05 + l.haze * 0.000_12) * lk.haze
    } else {
        0.0
    };
    let sunset = (1.0 - (sun.y * 4.0).abs()).clamp(0.0, 1.0) * (1.0 - night);
    let fog = Vec3::new(0.63, 0.70, 0.79)
        .lerp(Vec3::new(0.78, 0.55, 0.42), sunset)
        .lerp(Vec3::new(0.035, 0.05, 0.09), night);
    let az = sun.x.atan2(-sun.z).to_degrees().rem_euclid(360.0);
    LightOut {
        hour: l.hour,
        sun_dir: sun,
        elevation_deg: elevation.to_degrees(),
        azimuth_deg: az,
        sun_color: raw.map(|c| c / m),
        sun_strength: l.sun_strength * lk.sun * above,
        night,
        fog: l.haze_enabled && l.haze * lk.haze > 0.02,
        fog_density_exp2: density,
        fog_color: fog.to_array(),
        haze_enabled: l.haze_enabled,
        haze: if l.haze_enabled { l.haze } else { 0.0 },
        clouds: l.cloud_coverage,
        exposure: lk.exposure + l.exposure_comp,
        contrast: lk.contrast,
        saturation: lk.saturation,
        bloom: lk.bloom,
        vignette: lk.vignette,
    }
}

// ---------------------------------------------------------------------------------------------
// maps, manifest, credits

/// Footstep surfaces, zones and biomes at the terrain's own resolution (the generic package).
pub fn generic_maps(s: &Snapshot, ctx: &Ctx, dir: &str) -> Result<Value, String> {
    let t = &s.terrain;
    let n = t.n();
    let rows: Vec<Vec<u8>> = par_map(n, |j| {
        (0..n)
            .map(|i| {
                let w = t.final_weights(i, j);
                let best = (0..LAYER_COUNT)
                    .max_by(|&a, &b| w[a].total_cmp(&w[b]))
                    .unwrap_or(0);
                let k = t.idx(i, j);
                let p = t.grid_to_world(i, j);
                let wl = s
                    .water
                    .water_level(Vec3::new(p.x, t.heights[k], p.y))
                    .unwrap_or(f32::NEG_INFINITY)
                    .max(t.sea_level);
                let surf = if wl - t.heights[k] > 0.04 {
                    Surface::Water
                } else {
                    s.layer_surface[best]
                };
                surf.index() as u8
            })
            .collect()
    });
    let surf: Vec<u8> = rows.into_iter().flatten().collect();
    ctx.write(
        &format!("{dir}/footstep_surfaces.png"),
        &png_gray8(n as u32, n as u32, surf),
    )?;
    ctx.write(
        &format!("{dir}/zones.png"),
        &png_gray8(n as u32, n as u32, t.zones.clone()),
    )?;
    ctx.write(
        &format!("{dir}/biomes.png"),
        &png_gray8(n as u32, n as u32, t.biomes.clone()),
    )?;
    Ok(json!({
        "resolution": n,
        "footstep_surfaces": { "map": format!("{dir}/footstep_surfaces.png"), "legend": Surface::ALL.iter().map(|s| json!({ "index": s.index(), "name": s.name(), "sound": s.sound_stem() })).collect::<Vec<_>>() },
        "zones": { "map": format!("{dir}/zones.png"), "list": s.zones, "lighting": zone_lighting(&s.zones) },
        "biomes": {
            "map": format!("{dir}/biomes.png"),
            "legend": crate::biomes::BiomePreset::ALL.iter().map(|b| {
                let d = b.def();
                let light = (d.mood != crate::world::Mood::Natural).then(|| d.mood.light());
                json!({ "id": b.id(), "name": d.name, "weather": d.weather.name(), "ambience": d.soundscape.name(), "temperature_c": d.temperature, "wind_mps": d.wind, "lighting": light })
            }).collect::<Vec<_>>(),
        },
    }))
}

/// Each zone's own lighting & style as the game applies it while the player is inside (biomes'
/// `lighting` applies where no zone sets one): `light` stacks on the world's look (brightness
/// in stops; contrast, saturation, sun and shadow fill multiply; fog adds `fog_density` × fog), `style`
/// replaces the world's look by its strength. Crossfade over `fade_seconds` at the border.
fn zone_lighting(zones: &[Zone]) -> Value {
    let list: Vec<Value> = zones
        .iter()
        .filter_map(|z| {
            let light = z.own_light()?;
            let style = z
                .own_style()
                .map(|(p, w)| json!({ "name": p.name(), "strength": w, "params": p.params() }));
            Some(json!({ "zone": z.id, "mood": z.mood.name(), "light": light, "style": style }))
        })
        .collect();
    // fog_density: the exponential-squared density per metre that `fog` = 1 adds
    json!({ "fade_seconds": crate::look::ZONE_FADE, "fog_density": crate::sky::ZONE_FOG_DENSITY, "zones": list })
}

/// Sums the export's files (count, bytes) and lists the biggest.
pub fn file_summary(ctx: &Ctx) -> Value {
    let mut files = ctx.files();
    files.sort_by(|a, b| b.1.cmp(&a.1));
    let total: u64 = files.iter().map(|f| f.1).sum();
    let mut by_folder: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (p, b) in &files {
        let top = p.split('/').next().unwrap_or("").to_string();
        let top = if top.contains('.') {
            ".".to_string()
        } else {
            top
        };
        let e = by_folder.entry(top).or_default();
        e.0 += 1;
        e.1 += b;
    }
    json!({
        "count": files.len(),
        "bytes": total,
        "megabytes": (total as f64 / 1e6 * 10.0).round() / 10.0,
        "by_folder": by_folder.iter().map(|(k, (n, b))| json!({ "folder": k, "files": n, "megabytes": (*b as f64 / 1e6 * 10.0).round() / 10.0 })).collect::<Vec<_>>(),
        "largest": files.iter().take(8).map(|(p, b)| json!({ "file": p, "megabytes": (*b as f64 / 1e6 * 100.0).round() / 100.0 })).collect::<Vec<_>>(),
    })
}

/// The licences and authors of every bundled asset (all CC0) in one file.
pub fn credits(s: &Snapshot, layers: &[LayerTex]) -> String {
    let assets = PathBuf::from(crate::assets_path());
    let mut out = format!(
        "# Credits\n\nExported from Skyfold Studio: \"{}\". All bundled textures and models are CC0 1.0 (public domain); \
        attribution is appreciated but not required. Details per source follow.\n\n",
        s.name
    );
    let used: Vec<&LibraryMaterial> = layers
        .iter()
        .filter_map(|l| s.library.iter().find(|m| m.id == l.material))
        .collect();
    if !used.is_empty() {
        out.push_str("## Terrain layer materials\n\n");
        let raw =
            std::fs::read_to_string(crate::terrain::textures::library_dir().join("library.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        for m in used {
            let entry = raw.as_ref().and_then(|r| {
                r["materials"]
                    .as_array()?
                    .iter()
                    .find(|e| e["id"] == m.id.as_str())
                    .cloned()
            });
            let field = |k: &str| {
                entry
                    .as_ref()
                    .and_then(|e| e[k].as_str().map(str::to_string))
                    .unwrap_or_default()
            };
            out.push_str(&format!(
                "- {} — {} {} ({}), {}\n",
                m.name,
                field("source"),
                field("asset"),
                field("url"),
                field("license")
            ));
        }
        out.push('\n');
    }
    for (title, rel) in [
        ("Textures (terrain, foliage, bark)", "textures/CREDITS.md"),
        (
            "Photoscanned rocks, wood and plants",
            "models/photoscan/CREDITS.md",
        ),
        ("Poly Haven models", "models/polyhaven/CREDITS.md"),
        ("Food and tableware models", "models/food/CREDITS.md"),
    ] {
        if let Ok(t) = std::fs::read_to_string(assets.join(rel)) {
            out.push_str(&format!("## {title}\n\n"));
            // demote the source file's headings below ours
            for line in t.lines() {
                if line.starts_with('#') {
                    out.push_str("##");
                }
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
    }
    out
}

/// Counts for summaries.
pub fn counts(s: &Snapshot, models: &[ModelOut], inst: &[Vec<Inst>]) -> Value {
    let mut by_cat: BTreeMap<&str, usize> = BTreeMap::new();
    for (m, list) in models.iter().zip(inst) {
        *by_cat.entry(s.assets[m.asset].category.name()).or_default() += list.len();
    }
    json!({
        "models": models.len(),
        "instances": inst.iter().map(|l| l.len()).sum::<usize>(),
        "instances_by_category": by_cat,
        "model_triangles": models.iter().map(|m| m.triangles).sum::<usize>(),
        "gameplay_markers": markers(s).len(),
        "roads": s.roads.roads.len(),
        "lakes": s.water.lakes.len(),
        "rivers": s.water.rivers.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_weights_sum_to_255() {
        for w in [
            vec![0.2, 0.3, 0.5],
            vec![1.0],
            vec![0.0, 0.0],
            vec![0.333, 0.333, 0.334, 0.0001],
        ] {
            let q = quantize(&w);
            assert_eq!(q.iter().map(|&v| v as u32).sum::<u32>(), 255, "{w:?}");
        }
    }

    /// Exported trees get a far LOD well below LOD 2 (they're drawn as meshes to the horizon;
    /// an oak's LODs are about 14k / 6.7k / 3.8k / 2k triangles).
    #[test]
    fn trees_have_a_light_far_lod() {
        let sp = crate::trees::export_species(&[AssetKind::Oak])
            .pop()
            .unwrap();
        for lods in &sp.variants {
            assert_eq!(lods.len(), 4);
            let tris = |(b, l): &(Mesh, Option<Mesh>)| {
                arrays(b).idx.len() / 3 + l.as_ref().map_or(0, |l| arrays(l).idx.len() / 3)
            };
            let (lod2, far) = (tris(&lods[2]), tris(&lods[3]));
            assert!(far * 10 < lod2 * 6, "far {far} vs LOD2 {lod2}");
        }
    }

    /// Generated blossom and fan cards have no disk PNG.  Tree export must embed their exact
    /// runtime pixels just as it embeds photographed leaves, so every new tree remains a
    /// self-contained GLB.
    #[test]
    fn diverse_tree_exports_embed_leaf_cards() {
        for kind in [
            AssetKind::SakuraPink,
            AssetKind::SakuraWhite,
            AssetKind::Ginkgo,
            AssetKind::Sequoia,
            AssetKind::Bamboo,
        ] {
            let sp = crate::trees::export_species(&[kind])
                .pop()
                .expect("registered diverse species");
            let a = AssetInfo {
                id: format!("tree_{kind:?}"),
                name: format!("{kind:?}"),
                category: Category::Trees,
                kind: Some(kind),
                import_path: None,
                radius: 2.0,
                walkable: false,
                surface: Surface::Leaves,
                // Deliberately use the exporter fallback materials: this test is independent
                // of a live Bevy asset library and exercises the disk/generated texture split.
                tree: None,
            };
            let (bytes, variants) = tree_glb(&a, "diverse_tree", &sp, 128)
                .expect("diverse tree GLB exports with embedded foliage");
            assert_eq!(&bytes[..4], b"glTF");
            assert!(!variants.is_empty());
            let doc = super::glb::glb_json(&bytes).expect("valid GLB JSON");
            assert!(doc["images"].as_array().is_some_and(|v| !v.is_empty()));
            assert!(doc["materials"].as_array().is_some_and(|materials| {
                materials.iter().any(|m| {
                    m["name"].as_str().is_some_and(|n| n.ends_with("_leaves"))
                        && m["alphaMode"].as_str() == Some("MASK")
                })
            }));
        }
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("ph:brass_pot_02"), "ph_brass_pot_02");
        assert_eq!(safe_name("3 ships"), "_3_ships");
        assert_eq!(safe_name("--"), "world");
    }
}

// ---------------------------------------------------------------------------------------------
// caves

/// The caves for the engines: their rock as meshes, the terrain cells to open, what they are.
pub struct CavesOut {
    /// one node per cave (`cave_<id>_<name>`): its walls and floors (two textured parts, UVs
    /// projected along each vertex's main axis, AO in the vertex colours), mine timbers, pools
    pub glb: Option<Vec<u8>>,
    /// the terrain cells (i, j) whose triangles are left out (cave mouths, sinkholes)
    pub holes: Vec<[u32; 2]>,
    /// the same as a mask image (res × res cells, white = open)
    pub holes_png: Option<Vec<u8>>,
    pub json: Value,
}

/// (`suffix` goes on every cave node's name: Godot's `-col` makes import colliders.
/// `optimize`: each cave's rock simplified to within a few centimetres, `caves::simplify`.)
pub fn caves(s: &Snapshot, tex: u32, suffix: &str, optimize: bool) -> CavesOut {
    use crate::caves::{mesher, render, sdf};
    let empty = CavesOut {
        glb: None,
        holes: vec![],
        holes_png: None,
        json: json!([]),
    };
    if s.caves.is_empty() {
        return empty;
    }
    let vol = sdf::compile(&s.caves, &s.terrain);
    let fills: Vec<f32> = s.caves.list.iter().map(|c| c.darkness.fill()).collect();
    let built = mesher::build(&vol, &HashMap::new(), &fills);
    let mut g = Glb::new();
    // textures of the themes in use (wall rock and floor), shared between caves
    let mut mats: HashMap<String, (usize, f32)> = HashMap::new();
    let mut material = |g: &mut Glb, id: &str, rough: f32| -> (usize, f32) {
        if let Some(m) = mats.get(id) {
            return *m;
        }
        let m = match layer_texture_with(s, id, 6, tex) {
            Ok(lt) => {
                let a = image::DynamicImage::ImageRgb8(lt.albedo).to_rgba8();
                let n = image::DynamicImage::ImageRgb8(lt.normal).to_rgba8();
                let at = embed(g, &a, false, &format!("cave_{id}"), Wrap::Repeat);
                let nt = embed(g, &n, false, &format!("cave_{id}_normal"), Wrap::Repeat);
                (
                    g.material(&Material {
                        base_tex: Some(at),
                        normal_tex: Some(nt),
                        roughness: rough,
                        ..Material::new(&format!("cave_{id}"))
                    }),
                    lt.tile_m * 1.3,
                )
            }
            Err(_) => (
                g.material(&Material {
                    base_color: [0.32, 0.3, 0.28, 1.0],
                    roughness: rough,
                    ..Material::new(&format!("cave_{id}"))
                }),
                3.0,
            ),
        };
        mats.insert(id.to_string(), m);
        m
    };
    let wood = {
        let dir = std::path::PathBuf::from(crate::assets_path())
            .join("textures/materials/wood_rough_sawn_dark");
        let a = load_rgba(dir.join("albedo.png")).ok().map(|a| fit(a, tex));
        let n = load_rgba(dir.join("normal.png")).ok().map(|n| fit(n, tex));
        match (a, n) {
            (Some(a), Some(n)) => {
                let at = embed(&mut g, &a, false, "cave_timber", Wrap::Repeat);
                let nt = embed(&mut g, &n, false, "cave_timber_normal", Wrap::Repeat);
                g.material(&Material {
                    base_tex: Some(at),
                    normal_tex: Some(nt),
                    roughness: 0.85,
                    ..Material::new("cave_timber")
                })
            }
            _ => g.material(&Material {
                base_color: [0.25, 0.18, 0.12, 1.0],
                ..Material::new("cave_timber")
            }),
        }
    };
    let water = g.material_json(json!({ "name": "cave_water", "alphaMode": "BLEND", "pbrMetallicRoughness": { "baseColorFactor": [0.02, 0.035, 0.04, 0.85], "metallicFactor": 0.0, "roughnessFactor": 0.05 } }));
    // cobwebs: the web drawn the same way as in the editor, blended, seen from both sides
    let mut web_mat = None;
    let mut web = |g: &mut Glb| -> usize {
        *web_mat.get_or_insert_with(|| {
            let size = 512u32;
            let img = image::RgbaImage::from_raw(size, size, crate::caves::dress::web_texture(size, 0x5eb)).unwrap_or_else(|| image::RgbaImage::new(1, 1));
            let t = embed(g, &img, true, "cave_cobweb", Wrap::Clamp);
            g.material_json(json!({ "name": "cave_cobweb", "alphaMode": "BLEND", "doubleSided": true, "pbrMetallicRoughness": { "baseColorTexture": { "index": t }, "baseColorFactor": [0.62, 0.62, 0.6, 0.85], "metallicFactor": 0.0, "roughnessFactor": 0.6 } }))
        })
    };
    // glowing crystals: one emissive material per glow colour
    let mut glow_mats: HashMap<[u8; 3], usize> = HashMap::new();
    let mut glow = |g: &mut Glb, c: [u8; 3]| -> usize {
        *glow_mats.entry(c).or_insert_with(|| {
            let f = c.map(|v| v as f32 / 255.0);
            g.material_json(json!({ "name": format!("cave_crystal_{:02x}{:02x}{:02x}", c[0], c[1], c[2]), "emissiveFactor": f, "pbrMetallicRoughness": { "baseColorFactor": [f[0] * 0.15 + 0.02, f[1] * 0.15 + 0.02, f[2] * 0.15 + 0.02, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.12 } }))
        })
    };
    let mut nodes = vec![];
    let mut list = vec![];
    for (i, cave) in s.caves.list.iter().enumerate() {
        let look = render::look(cave.theme());
        let (wall_mat, wall_tile) = material(&mut g, look.wall[0], 0.85);
        let (floor_mat, floor_tile) = material(&mut g, look.floor, 0.9);
        // this cave's chunks welded into one mesh (and simplified), then walls and floors split
        // by the triangles' slope
        let (rock_pos, rock_nrm, rock_ao, rock_idx) = {
            let mut pos: Vec<Vec3> = vec![];
            let mut nrm: Vec<[f32; 3]> = vec![];
            // the mesher's [daylight, AO, wetness, skin]: the AO becomes a grey vertex colour
            let mut ao: Vec<[f32; 4]> = vec![];
            let mut idx: Vec<u32> = vec![];
            for m in built.meshes.iter().filter(|m| m.owner as usize == i) {
                let base = pos.len() as u32;
                pos.extend(m.positions.iter().map(|p| Vec3::from_array(*p)));
                nrm.extend_from_slice(&m.normals);
                ao.extend((0..m.positions.len()).map(|k| {
                    m.colors
                        .get(k)
                        .map_or([1.0; 4], |c| [c[1], c[1], c[1], 1.0])
                }));
                idx.extend(m.indices.iter().map(|k| k + base));
            }
            let (keep, idx) = crate::caves::simplify::weld(&pos, &idx);
            let pos: Vec<Vec3> = keep.iter().map(|k| pos[*k as usize]).collect();
            let nrm: Vec<[f32; 3]> = keep.iter().map(|k| nrm[*k as usize]).collect();
            let ao: Vec<[f32; 4]> = keep.iter().map(|k| ao[*k as usize]).collect();
            let idx = if optimize {
                crate::caves::simplify::simplify(
                    &pos,
                    &idx,
                    crate::caves::simplify::EXPORT_TOLERANCE,
                    &[],
                )
            } else {
                idx
            };
            (pos, nrm, ao, idx)
        };
        #[allow(clippy::type_complexity)]
        let mut parts: [(
            Vec<[f32; 3]>,
            Vec<[f32; 3]>,
            Vec<[f32; 2]>,
            Vec<[f32; 4]>,
            Vec<u32>,
        ); 2] = Default::default();
        {
            // (vertices shared within each part)
            let mut remap: [HashMap<u32, u32>; 2] = Default::default();
            for tri in rock_idx.chunks(3) {
                let p: Vec<Vec3> = tri.iter().map(|&k| rock_pos[k as usize]).collect();
                let fnrm = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                let part = if fnrm.y > 0.6 { 1 } else { 0 };
                let tile = if part == 1 { floor_tile } else { wall_tile };
                let (pos, nrm, uv, col, idx) = &mut parts[part];
                for &k in tri {
                    let v = *remap[part].entry(k).or_insert_with(|| {
                        let q = rock_pos[k as usize].to_array();
                        let n = rock_nrm[k as usize];
                        // projected along the vertex's main axis (no UVs in the rock itself)
                        let a = Vec3::from_array(n).abs();
                        let u = if a.y >= a.x && a.y >= a.z {
                            [q[0] / tile, q[2] / tile]
                        } else if a.x >= a.z {
                            [q[2] / tile, q[1] / tile]
                        } else {
                            [q[0] / tile, q[1] / tile]
                        };
                        pos.push(q);
                        nrm.push(n);
                        uv.push(u);
                        col.push(rock_ao[k as usize]);
                        pos.len() as u32 - 1
                    });
                    idx.push(v);
                }
            }
        }
        let mut prims = vec![];
        let mut soft = vec![];
        let [(wp, wn, wu, wc, wi), (fp, fnn, fu, fc, fi)] = &parts;
        if !wi.is_empty() {
            prims.push(Prim {
                positions: wp,
                normals: Some(wn),
                uvs: Some(wu),
                colors: Some(wc),
                indices: wi,
                material: Some(wall_mat),
                ..Default::default()
            });
        }
        if !fi.is_empty() {
            prims.push(Prim {
                positions: fp,
                normals: Some(fnn),
                uvs: Some(fu),
                colors: Some(fc),
                indices: fi,
                material: Some(floor_mat),
                ..Default::default()
            });
        }
        let dress = crate::caves::dress::dress(cave, &vol, &built.light);
        let (tm, wm) = (&dress.timbers, &dress.water);
        let tm_uv: Vec<[f32; 2]> = tm.uvs.iter().map(|u| [u[0] / 1.5, u[1] / 1.5]).collect();
        if !tm.indices.is_empty() {
            prims.push(Prim {
                positions: &tm.positions,
                normals: Some(&tm.normals),
                uvs: Some(&tm_uv),
                indices: &tm.indices,
                material: Some(wood),
                ..Default::default()
            });
        }
        let wm_uv: Vec<[f32; 2]> = wm.uvs.iter().map(|u| [u[0] / 4.0, u[1] / 4.0]).collect();
        if !wm.indices.is_empty() {
            soft.push(Prim {
                positions: &wm.positions,
                normals: Some(&wm.normals),
                uvs: Some(&wm_uv),
                indices: &wm.indices,
                material: Some(water),
                ..Default::default()
            });
        }
        // the crystals, split by glow colour (each part its own vertices)
        let mut by_glow: HashMap<[u8; 3], (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u32>)> =
            HashMap::new();
        let cm = &dress.crystals;
        for tri in cm.indices.chunks(3) {
            let c = cm.colors[tri[0] as usize];
            let key = [c[0], c[1], c[2]]
                .map(|v| ((v * 4.0).round() / 4.0 * 255.0).clamp(0.0, 255.0) as u8);
            let part = by_glow.entry(key).or_default();
            for &k in tri {
                part.0.push(cm.positions[k as usize]);
                part.1.push(cm.normals[k as usize]);
                part.2.push(cm.uvs[k as usize]);
                part.3.push(part.3.len() as u32);
            }
        }
        let mut keys: Vec<[u8; 3]> = by_glow.keys().copied().collect();
        keys.sort();
        let glow_ids: Vec<usize> = keys.iter().map(|k| glow(&mut g, *k)).collect();
        for (k, id) in keys.iter().zip(&glow_ids) {
            let (pos, nrm, uv, idx) = &by_glow[k];
            prims.push(Prim {
                positions: pos,
                normals: Some(nrm),
                uvs: Some(uv),
                indices: idx,
                material: Some(*id),
                ..Default::default()
            });
        }
        let web_id = (!dress.webs.indices.is_empty()).then(|| web(&mut g));
        if let Some(id) = web_id {
            let wb = &dress.webs;
            soft.push(Prim {
                positions: &wb.positions,
                normals: Some(&wb.normals),
                uvs: Some(&wb.uvs),
                indices: &wb.indices,
                material: Some(id),
                ..Default::default()
            });
        }
        let node = format!("cave_{}_{}{suffix}", cave.id, safe_name(&cave.name));
        // (water and webs on a node of their own: no collider is made for them)
        let fx = format!("cave_{}_{}_water_webs", cave.id, safe_name(&cave.name));
        let fx_node = (!soft.is_empty())
            .then(|| g.mesh(&fx, &soft))
            .flatten()
            .map(|m| g.node(&fx, Some(m), vec![]));
        if !prims.is_empty()
            && let Some(mesh) = g.mesh(&node, &prims)
        {
            nodes.push(g.node(&node, Some(mesh), vec![]));
        }
        nodes.extend(fx_node);
        let (lo, hi) = cave.bounds();
        let r1 = |v: f32| (v * 100.0).round() / 100.0;
        list.push(json!({
            "id": cave.id,
            "name": cave.name,
            "kind": cave.kind.id(),
            "theme": cave.theme().id(),
            "node": node,
            "entrance": cave.entrance,
            "heading_deg": crate::mcp_tools::dir_bearing(crate::caves::layout::dir(cave.heading)),
            "bounds": { "min": [r1(lo.x), r1(lo.y), r1(lo.z)], "max": [r1(hi.x), r1(hi.y), r1(hi.z)] },
            "levels": cave.levels,
            "chambers": cave.chambers.iter().map(|c| json!({ "id": c.id, "floor": c.pos, "size": [c.size[0] * 2.0, c.size[1], c.size[2] * 2.0], "level": c.level })).collect::<Vec<_>>(),
            "darkness": cave.darkness.id(),
            "size": cave.size.id(),
            "levels_go": if cave.up { "up" } else { "down" },
            "great_halls": cave.chambers.iter().filter(|c| c.hall).map(|c| c.id).collect::<Vec<_>>(),
            "waterfalls": crate::caves::dress::fall_sites(cave).iter().map(|f| json!({ "lip": [r1(f.lip.x), r1(f.lip.y), r1(f.lip.z)], "lands": [r1(f.land.x), r1(f.level), r1(f.land.z)], "width": r1(f.width) })).collect::<Vec<_>>(),
            "crystal_lights": dress.lights.iter().map(|l| json!({ "position": [r1(l.pos.x), r1(l.pos.y), r1(l.pos.z)], "color_linear": l.color, "lumens": l.lumens.round(), "range": r1(l.range) })).collect::<Vec<_>>(),
            "dungeon": cave.dungeon.as_ref().map(|d| json!({
                "name": d.name,
                "door": d.door,
                "door_heading_deg": crate::mcp_tools::dir_bearing(crate::caves::layout::dir(d.door_yaw)),
                "instancing": d.instancing.id(),
                "note": "a portal into the dungeon's own map (dungeons/dungeons.json: its scene / level, trigger and where players come back out)",
            })),
        }));
    }
    let mut holes: Vec<[u32; 2]> = built
        .holes
        .iter()
        .filter(|c| c.x >= 0 && c.y >= 0)
        .map(|c| [c.x as u32, c.y as u32])
        .collect();
    holes.sort();
    let res = s.terrain.res as u32;
    let holes_png = (!holes.is_empty()).then(|| {
        let mut px = vec![0u8; (res * res) as usize];
        for h in &holes {
            if h[0] < res && h[1] < res {
                px[(h[1] * res + h[0]) as usize] = 255;
            }
        }
        png_gray8(res, res, px)
    });
    let glb = (!nodes.is_empty()).then(|| {
        let root = g.node("Caves", None, nodes);
        g.finish(&[root])
    });
    CavesOut {
        glb,
        holes,
        holes_png,
        json: json!(list),
    }
}
