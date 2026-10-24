//! Keeps the terrain's GPU textures in sync with the CPU data.
//!
//! Full textures are created when a world is created or loaded. While painting, only the dirty
//! sub-rectangle is streamed to the GPU through `RenderQueue::write_texture`, so editing a 4k
//! world stays cheap. The normal and splat maps carry mip chains (`super::mips`); a change
//! uploads the texels of every level over it.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
    TextureDimension, TextureFormat,
};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

use super::layers::LAYER_COUNT;
use super::material::{TerrainExt, TerrainGpu, TerrainMaterial, TerrainParams};
use super::mips::Pyramid;
use super::{GridRect, Terrain};

#[derive(Clone)]
pub struct TexWrite {
    pub image: AssetId<Image>,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub bytes_per_texel: u32,
    pub mip: u32,
    /// byte offset of this write's data inside `data` (several mips can share one buffer)
    pub offset: usize,
    pub data: Arc<Vec<u8>>,
}

/// Texture writes produced this frame in the main world.
#[derive(Resource, Default)]
pub struct PendingTexWrites(pub Vec<TexWrite>);

/// Render-world queue (writes wait here until their GPU image exists).
#[derive(Resource, Default)]
struct RenderTexWrites(Vec<TexWrite>);

pub struct TerrainGpuPlugin;

impl Plugin for TerrainGpuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingTexWrites>()
            .init_resource::<TerrainMips>()
            .add_systems(First, |mut p: ResMut<PendingTexWrites>| p.0.clear())
            .add_systems(
                PostUpdate,
                sync_terrain_gpu.in_set(super::TerrainSystems::Gpu),
            );
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<RenderTexWrites>()
                .add_systems(ExtractSchedule, extract_writes)
                .add_systems(Render, apply_writes.in_set(RenderSystems::PrepareResources));
        }
    }
}

fn extract_writes(mut dst: ResMut<RenderTexWrites>, src: Extract<Res<PendingTexWrites>>) {
    dst.0.extend(src.0.iter().cloned());
}

fn apply_writes(
    mut writes: ResMut<RenderTexWrites>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
) {
    writes.0.retain(|w| {
        let Some(gpu) = images.get(w.image) else {
            return true;
        };
        let (mw, mh) = (
            (gpu.texture_descriptor.size.width >> w.mip).max(1),
            (gpu.texture_descriptor.size.height >> w.mip).max(1),
        );
        if mw < w.x + w.w || mh < w.y + w.h || w.mip >= gpu.texture_descriptor.mip_level_count {
            return false; // stale write for a texture of another size
        }
        let len = (w.w * w.h * w.bytes_per_texel) as usize;
        if w.offset + len > w.data.len() {
            return false;
        }
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &gpu.texture,
                mip_level: w.mip,
                origin: Origin3d {
                    x: w.x,
                    y: w.y,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            &w.data[w.offset..w.offset + len],
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w.w * w.bytes_per_texel),
                rows_per_image: Some(w.h),
            },
            Extent3d {
                width: w.w,
                height: w.h,
                depth_or_array_layers: 1,
            },
        );
        false
    });
}

/// Mip chains of the normal and splat maps (`super::mips`), for the current terrain generation.
#[derive(Resource, Default)]
pub struct TerrainMips {
    normal: Pyramid,
    splat: [Pyramid; 3],
}

/// A filtered map with its full mip chain (anisotropic: distant paint borders stay crisp across
/// the view instead of blurring along it).
fn make_mipped_image(n: usize, top: Vec<u8>, pyramid: &Pyramid) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.data = Some(pyramid.chain(top));
    img.texture_descriptor.mip_level_count = pyramid.levels.len() as u32 + 1;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    img
}

fn make_image(n: usize, format: TextureFormat, data: Vec<u8>, linear: bool) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.sampler = if linear {
        ImageSampler::Descriptor(ImageSamplerDescriptor::linear())
    } else {
        ImageSampler::Descriptor(ImageSamplerDescriptor::nearest())
    };
    img
}

pub fn encode_normal(n: Vec3) -> [u8; 4] {
    [
        ((n.x * 0.5 + 0.5) * 255.0).round() as u8,
        ((n.y * 0.5 + 0.5) * 255.0).round() as u8,
        ((n.z * 0.5 + 0.5) * 255.0).round() as u8,
        255,
    ]
}

fn height_bytes(t: &Terrain, r: GridRect) -> Vec<u8> {
    let mut out = Vec::with_capacity(r.width() * r.height() * 4);
    for j in r.z0..=r.z1 {
        for i in r.x0..=r.x1 {
            out.extend_from_slice(&t.h(i, j).to_le_bytes());
        }
    }
    out
}

/// Normals, with the flow map (× the style's wet-gully strength) in alpha.
fn normal_bytes(t: &Terrain, r: GridRect) -> Vec<u8> {
    let mut out = Vec::with_capacity(r.width() * r.height() * 4);
    for j in r.z0..=r.z1 {
        for i in r.x0..=r.x1 {
            let mut e = encode_normal(t.normal_at_grid(i, j));
            let f = t.flow.get(t.idx(i, j)).copied().unwrap_or(0);
            // (dry biomes keep no damp gullies: `Terrain::gully_wet`)
            let wet = if f == 0 {
                0.0
            } else {
                t.gully_wet(i, j).clamp(0.0, 1.0)
            };
            e[3] = (f as f32 * wet).round() as u8;
            out.extend_from_slice(&e);
        }
    }
    out
}

fn splat_bytes(t: &Terrain, r: GridRect, tex: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(r.width() * r.height() * 4);
    for j in r.z0..=r.z1 {
        for i in r.x0..=r.x1 {
            let s = &t.splat[t.idx(i, j)];
            out.extend_from_slice(&s[tex * 4..tex * 4 + 4]);
        }
    }
    out
}

/// The overlay map: zones, or the biome map while the biome brush is active.
fn zone_bytes(t: &Terrain, r: GridRect) -> Vec<u8> {
    let src = if t.show_biomes { &t.biomes } else { &t.zones };
    let mut out = Vec::with_capacity(r.width() * r.height());
    for j in r.z0..=r.z1 {
        for i in r.x0..=r.x1 {
            out.push(src[t.idx(i, j)]);
        }
    }
    out
}

/// Creates/updates GPU textures for the terrain.
pub fn sync_terrain_gpu(
    mut commands: Commands,
    mut terrain: ResMut<Terrain>,
    gpu: Option<ResMut<TerrainGpu>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut pending: ResMut<PendingTexWrites>,
    mut mips: ResMut<TerrainMips>,
) {
    let n = terrain.n();
    let all = GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    };
    let needs_full = gpu
        .as_ref()
        .map(|g| g.generation != Some(terrain.generation))
        .unwrap_or(true);
    if needs_full {
        let height = images.add(make_image(
            n,
            TextureFormat::R32Float,
            height_bytes(&terrain, all),
            false,
        ));
        // (`WB_NOTERRAINMIPS=1`: the top levels only, A/B tests)
        let chain = |top: &[u8]| {
            if std::env::var("WB_NOTERRAINMIPS").is_ok() {
                Pyramid { n, levels: vec![] }
            } else {
                Pyramid::build(top, n)
            }
        };
        let top = normal_bytes(&terrain, all);
        mips.normal = chain(&top);
        let normal = images.add(make_mipped_image(n, top, &mips.normal));
        let splat: [Handle<Image>; 3] = std::array::from_fn(|k| {
            let top = splat_bytes(&terrain, all, k);
            mips.splat[k] = chain(&top);
            images.add(make_mipped_image(n, top, &mips.splat[k]))
        });
        let zones = images.add(make_image(
            n,
            TextureFormat::R8Unorm,
            zone_bytes(&terrain, all),
            false,
        ));
        // (baked by `bake` before the terrain draws with them)
        let rules = super::bake::rule_maps(n, &mut images);
        let albedo_array = images.add(super::textures::placeholder_array(true));
        let detail_array = images.add(super::textures::placeholder_array(false));
        let ext = TerrainExt {
            albedo_array,
            detail_array,
            params: TerrainParams::default(),
            normal_map: normal.clone(),
            splat0: splat[0].clone(),
            splat1: splat[1].clone(),
            splat2: splat[2].clone(),
            height: height.clone(),
            zones: zones.clone(),
            overlay: Handle::default(),
            shore: Handle::default(),
            cloud_cover: Handle::default(),
            rule0: rules[0].clone(),
            rule1: rules[1].clone(),
            canopy: crate::nature::shade::CANOPY_SHADE,
            reference: false,
            per_pixel_rules: false,
        };
        match gpu {
            Some(mut g) => {
                if let Some(mut m) = materials.get_mut(&g.material) {
                    // keep uniforms and the loaded material textures
                    let params = m.extension.params.clone();
                    let (a, d, o, s) = (
                        m.extension.albedo_array.clone(),
                        m.extension.detail_array.clone(),
                        m.extension.overlay.clone(),
                        m.extension.shore.clone(),
                    );
                    let (cloud_cover, reference, per_pixel_rules) = (
                        m.extension.cloud_cover.clone(),
                        m.extension.reference,
                        m.extension.per_pixel_rules,
                    );
                    m.extension = TerrainExt {
                        params,
                        albedo_array: a,
                        detail_array: d,
                        overlay: o,
                        shore: s,
                        cloud_cover,
                        reference,
                        per_pixel_rules,
                        ..ext
                    };
                }
                g.height = height;
                g.normal = normal;
                g.splat = splat;
                g.zones = zones;
                g.rules = rules;
                g.generation = Some(terrain.generation);
            }
            None => {
                let material = materials.add(TerrainMaterial {
                    base: StandardMaterial {
                        base_color: Color::WHITE,
                        perceptual_roughness: 0.92,
                        reflectance: 0.3,
                        ..default()
                    },
                    extension: ext,
                });
                commands.insert_resource(TerrainGpu {
                    material,
                    height,
                    normal,
                    splat,
                    zones,
                    rules,
                    generation: Some(terrain.generation),
                });
            }
        }
        terrain.dirty_height = None;
        terrain.dirty_splat = None;
        terrain.dirty_zone = None;
        terrain.dirty_flow = None;
        return;
    }
    let gpu = gpu.unwrap();

    // (each change also refreshes the mip texels over it)
    let mut normal_dirty: Option<GridRect> = terrain.dirty_flow.take();
    if let Some(r) = terrain.dirty_height.take() {
        pending
            .0
            .push(write_for(&gpu.height, r, 4, height_bytes(&terrain, r)));
        let rn = r.expand(1, n);
        normal_dirty = Some(normal_dirty.map_or(rn, |d| d.union(rn)));
    }
    if let Some(r) = normal_dirty {
        pending
            .0
            .push(write_for(&gpu.normal, r, 4, normal_bytes(&terrain, r)));
        mips.normal.update(
            r,
            |s| normal_bytes(&terrain, s),
            gpu.normal.id(),
            &mut pending.0,
        );
    }
    if let Some(r) = terrain.dirty_splat.take() {
        for k in 0..LAYER_COUNT / 4 {
            pending
                .0
                .push(write_for(&gpu.splat[k], r, 4, splat_bytes(&terrain, r, k)));
            mips.splat[k].update(
                r,
                |s| splat_bytes(&terrain, s, k),
                gpu.splat[k].id(),
                &mut pending.0,
            );
        }
    }
    if let Some(r) = terrain.dirty_zone.take() {
        pending
            .0
            .push(write_for(&gpu.zones, r, 1, zone_bytes(&terrain, r)));
    }
}

fn write_for(image: &Handle<Image>, r: GridRect, bpt: u32, data: Vec<u8>) -> TexWrite {
    TexWrite {
        image: image.id(),
        x: r.x0 as u32,
        y: r.z0 as u32,
        w: r.width() as u32,
        h: r.height() as u32,
        bytes_per_texel: bpt,
        mip: 0,
        offset: 0,
        data: Arc::new(data),
    }
}
