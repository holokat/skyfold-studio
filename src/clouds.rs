//! Volumetric clouds.
//!
//! - A cumulus layer (base at 1500 m, top by cloud type) is ray marched at half resolution right
//!   after the opaque scene and the water are drawn, so the march stops at the terrain and clouds
//!   between the camera and the ground show over it. Density follows the Nubis model: a baked
//!   2D cover map (the same function as the terrain's cloud shadows, `noise.wesl::cloud_density`),
//!   a height profile by cloud type, a Perlin–Worley base shape and Worley detail erosion. Light:
//!   a short march toward the sun (or the moon), multiple-scattering octaves with a two-lobe
//!   Henyey–Greenstein phase, sky ambient from Bevy's atmosphere LUTs, sunset colours from the
//!   transmittance LUT (with the earth's shadow) and aerial perspective from the aerial-view LUT.
//! - A high cirrus veil (one textured shell at 8.5 km).
//! - The result is accumulated over frames (reprojected with the cloud depth and the wind, clipped
//!   to the current neighbourhood) and blended over the scene by a transparent material drawn just
//!   after the star dome, so clouds hide stars and the moon but rain and smoke stay in front.
//!
//! Inputs: `SkyState::weather.clouds` (cover after zone weather), `SkyState::cloud_offset` (wind
//! drift, wrapped to the field's period), and the `Lighting` cloud fields.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
use bevy::core_pipeline::schedule::Core3d;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::resources::{
    AtmosphereSampler, AtmosphereTextures, AtmosphereTransform, AtmosphereTransforms,
    AtmosphereTransformsOffset, GpuAtmosphere,
};
use bevy::pbr::{
    GpuAtmosphereSettings, MaterialPipeline, MaterialPipelineKey, main_transmissive_pass_3d,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_3d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    AsBindGroup, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, CompareFunction, Extent3d, FilterMode,
    FragmentState, LoadOp, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, ShaderType, SpecializedMeshPipelineError, StoreOp, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor, UniformBuffer,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{
    ExtractedView, ViewDepthStencilTexture, ViewUniform, ViewUniformOffset, ViewUniforms,
};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use crate::camera::MainCamera;
use crate::look::LookState;
use crate::noise::{hash_u32, hash2, smoothstep};
use crate::sky::{Lighting, Moon, SkyState, Sun};

/// Period (m) of the whole cloud field: every noise used by the clouds and their shadows tiles
/// with it, so the wind offset wraps around it without a seam.
pub const CLOUD_TILE: f32 = 32768.0;
/// Altitude (m) of the cumulus base.
pub const CLOUD_BASE: f32 = 1500.0;
/// The planar capture's oblique clip needs its private composite dome to reach its cloud horizon.
/// Keep this within the camera's 80 km far plane and aligned with `clouds.wesl`'s march distance.
const REFLECTION_CLOUD_DOME_RADIUS: f32 = 65_000.0;
/// Bevy's earth atmosphere inner radius (the clouds sit on its shell).
const EARTH_RADIUS: f32 = 6_360_000.0;
/// Altitude (m) of the cirrus veil.
const CIRRUS_ALT: f32 = 8500.0;

const BASE_RES: u32 = 128;
const DETAIL_RES: u32 = 32;
const COVER_RES: u32 = 1024;
const CIRRUS_RES: u32 = 1024;

/// The cloud trace in the render schedule (`volumetrics` fogs its result after it).
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CloudTrace;

/// Main-world cloud setup has completed for this frame. Reflection setup follows the water
/// camera, so keeping the ordering explicit avoids a capture starting before it has a cloud view.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CloudSystems;

pub struct CloudsPlugin;

impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<CloudCompositeMaterial>::default(),
            ExtractResourcePlugin::<CloudFrame>::default(),
            ExtractComponentPlugin::<CloudView>::default(),
        ))
        .add_systems(Startup, setup_clouds)
        .configure_sets(PostUpdate, CloudSystems)
        .add_systems(
            PostUpdate,
            (update_clouds, update_reflection_clouds)
                .chain()
                .in_set(CloudSystems)
                .after(crate::sky::SkySystems)
                .after(crate::water_render::WaterRenderSystems)
                .before(bevy::transform::TransformSystems::Propagate),
        );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<CloudState>()
            .add_systems(RenderStartup, init_cloud_pipelines)
            .add_systems(
                Render,
                prepare_clouds.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(
                Core3d,
                clouds_pass
                    .in_set(CloudTrace)
                    .after(main_transmissive_pass_3d)
                    .before(main_transparent_pass_3d),
            );
    }
}

// ------------------------------------------------------------------------------------------------
// Settings

/// Cost / quality of the cloud trace (`Lighting::cloud_quality`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CloudQuality {
    /// A third of the resolution, short marches: for slow GPUs.
    Low,
    /// Half resolution.
    #[default]
    Medium,
    /// Half resolution, longer marches and light samples.
    High,
}

impl CloudQuality {
    pub const ALL: [CloudQuality; 3] =
        [CloudQuality::Low, CloudQuality::Medium, CloudQuality::High];
    pub fn name(self) -> &'static str {
        match self {
            CloudQuality::Low => "Low",
            CloudQuality::Medium => "Medium",
            CloudQuality::High => "High",
        }
    }
    /// The clouds are rebuilt at 1 / scale of the view resolution (per axis); each frame traces one
    /// pixel of every 2x2 block of that.
    fn history_scale(self) -> u32 {
        match self {
            CloudQuality::Low => 3,
            CloudQuality::Medium | CloudQuality::High => 2,
        }
    }
    /// (coarse steps across the layer, light samples, weight of a new frame in the history)
    pub(crate) fn march(self) -> (f32, f32, f32) {
        match self {
            CloudQuality::Low => (32.0, 3.0, 0.12),
            CloudQuality::Medium => (48.0, 4.0, 0.15),
            CloudQuality::High => (64.0, 5.0, 0.15),
        }
    }
}

/// A look for the sky, picked by [`randomize_clouds`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudPreset {
    Clear,
    FairWeather,
    Scattered,
    Broken,
    Overcast,
    CirrusOnly,
    Towering,
}

impl CloudPreset {
    pub const ALL: [CloudPreset; 7] = [
        CloudPreset::Clear,
        CloudPreset::FairWeather,
        CloudPreset::Scattered,
        CloudPreset::Broken,
        CloudPreset::Overcast,
        CloudPreset::CirrusOnly,
        CloudPreset::Towering,
    ];

    /// Ranges for (coverage, type, density, cirrus).
    fn ranges(self) -> [(f32, f32); 4] {
        match self {
            // the everyday skies keep their cumulus low and thin; Towering is the tall one
            CloudPreset::Clear => [(0.02, 0.1), (0.35, 0.55), (0.6, 1.0), (0.0, 0.35)],
            CloudPreset::FairWeather => [(0.18, 0.32), (0.35, 0.5), (0.7, 0.95), (0.0, 0.5)],
            CloudPreset::Scattered => [(0.35, 0.5), (0.4, 0.6), (0.8, 1.1), (0.1, 0.6)],
            CloudPreset::Broken => [(0.58, 0.75), (0.45, 0.75), (1.0, 1.4), (0.0, 0.4)],
            CloudPreset::Overcast => [(0.88, 1.0), (0.2, 0.45), (1.1, 1.6), (0.0, 0.2)],
            CloudPreset::CirrusOnly => [(0.0, 0.06), (0.6, 0.8), (0.8, 1.0), (0.6, 1.0)],
            CloudPreset::Towering => [(0.3, 0.45), (0.9, 1.0), (1.2, 1.6), (0.0, 0.4)],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CloudPreset::Clear => "Clear",
            CloudPreset::FairWeather => "Fair",
            CloudPreset::Scattered => "Scattered",
            CloudPreset::Broken => "Broken",
            CloudPreset::Overcast => "Overcast",
            CloudPreset::CirrusOnly => "Cirrus",
            CloudPreset::Towering => "Towering",
        }
    }

    /// Picks the preset and its values (all from `seed`).
    pub fn apply(self, lighting: &mut Lighting, seed: u64) {
        let mut rng = fastrand::Rng::with_seed(seed ^ 0x5eed_c10d);
        let r = self.ranges();
        let mut pick = |(a, b): (f32, f32)| a + (b - a) * rng.f32();
        lighting.cloud_coverage = pick(r[0]);
        lighting.cloud_type = pick(r[1]);
        lighting.cloud_density = pick(r[2]);
        lighting.cirrus = pick(r[3]);
        lighting.cloud_seed = (seed as u32) ^ (seed >> 32) as u32;
    }
}

/// Picks a pleasant random sky (fair-weather cumulus, scattered, broken, overcast, cirrus only, …)
/// and writes it into the lighting's cloud fields (cover, type, density, cirrus and the pattern
/// seed). Fair skies are the most likely.
pub fn randomize_clouds(lighting: &mut Lighting, seed: u64) -> CloudPreset {
    const WEIGHTS: [u32; 7] = [8, 26, 22, 14, 8, 10, 12];
    let mut rng = fastrand::Rng::with_seed(seed);
    let total: u32 = WEIGHTS.iter().sum();
    let mut k = rng.u32(0..total);
    let mut preset = CloudPreset::FairWeather;
    for (p, w) in CloudPreset::ALL.into_iter().zip(WEIGHTS) {
        if k < w {
            preset = p;
            break;
        }
        k -= w;
    }
    preset.apply(lighting, rng.u64(..));
    preset
}

/// Where the seed puts the field (the pattern is periodic, so any offset in one tile is a new sky).
pub fn seed_offset(seed: u32) -> Vec2 {
    if seed == 0 {
        return Vec2::ZERO;
    }
    let h = hash_u32(seed.wrapping_mul(0x9e37_79b9) ^ 0x68e3_1da4);
    Vec2::new((h & 0xffff) as f32, (h >> 16) as f32) / 65536.0 * CLOUD_TILE
}

/// Wraps a field offset into one period (seamless: the field tiles).
pub fn wrap_offset(o: Vec2) -> Vec2 {
    Vec2::new(o.x.rem_euclid(CLOUD_TILE), o.y.rem_euclid(CLOUD_TILE))
}

// ------------------------------------------------------------------------------------------------
// Cloud cover (mirror of `noise.wesl`: cloud_noise / cloud_cover)

fn cover_hash(ix: i32, iy: i32, period: i32, seed: u32) -> u32 {
    hash2(ix.rem_euclid(period), iy.rem_euclid(period), seed)
}

fn cover_gdot(h: u32, dx: f32, dy: f32) -> f32 {
    let gx = (h & 0xffff) as f32 * (1.0 / 32767.5) - 1.0;
    let gy = (h >> 16) as f32 * (1.0 / 32767.5) - 1.0;
    gx * dx + gy * dy
}

fn cover_gnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v) = (fade(fx), fade(fy));
    let a = cover_gdot(cover_hash(ix, iy, period, seed), fx, fy);
    let b = cover_gdot(cover_hash(ix + 1, iy, period, seed), fx - 1.0, fy);
    let c = cover_gdot(cover_hash(ix, iy + 1, period, seed), fx, fy - 1.0);
    let d = cover_gdot(cover_hash(ix + 1, iy + 1, period, seed), fx - 1.0, fy - 1.0);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}

fn cover_blobs(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let mut b = 0.0f32;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let h = cover_hash(ix + ox, iy + oy, period, seed);
            let px = (h & 0xff) as f32 * (0.8 / 255.0) + 0.1;
            let py = ((h >> 8) & 0xff) as f32 * (0.8 / 255.0) + 0.1;
            let r = 0.45 + 0.6 * ((h >> 16) & 0xff) as f32 / 255.0;
            let s = 0.55 + 0.45 * (h >> 24) as f32 / 255.0;
            let dx = ox as f32 + px - fx;
            let dy = oy as f32 + py - fy;
            let v = (1.0 - (dx * dx + dy * dy).sqrt() / r).clamp(0.0, 1.0);
            b = b.max(v * (2.0 - v) * s);
        }
    }
    b
}

/// Cloudiness (0..1, evenly distributed) of the field at `q` in tile units.
pub fn cloud_noise(qx: f32, qy: f32) -> f32 {
    let region = 0.5
        + 0.5
            * (0.65 * cover_gnoise(qx * 4.0, qy * 4.0, 4, 11)
                + 0.35 * cover_gnoise(qx * 8.0, qy * 8.0, 8, 12));
    let wx = cover_gnoise(qx * 16.0 + 3.1, qy * 16.0, 16, 21) * 0.02;
    let wy = cover_gnoise(qx * 16.0, qy * 16.0 + 7.7, 16, 22) * 0.02;
    let cells = cover_blobs((qx + wx) * 16.0, (qy + wy) * 16.0, 16, 13);
    let n = 0.55 * cells + 0.45 * region;
    0.5 + 0.5 * ((n - 0.527) * 5.73).tanh()
}

// ------------------------------------------------------------------------------------------------
// Noise volumes (generated once at startup)

fn hash3(ix: i32, iy: i32, iz: i32, seed: u32) -> u32 {
    hash_u32(hash2(ix, iy, seed) ^ (iz as u32).wrapping_mul(0x9e37_79b9))
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Periodic 3D gradient noise, about -1..1.
fn perlin3(p: Vec3, period: i32, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (ix, iy, iz) = (i.x as i32, i.y as i32, i.z as i32);
    let g = |dx: i32, dy: i32, dz: i32| {
        let h = hash3(
            (ix + dx).rem_euclid(period),
            (iy + dy).rem_euclid(period),
            (iz + dz).rem_euclid(period),
            seed,
        );
        let gx = (h & 0x3ff) as f32 / 511.5 - 1.0;
        let gy = ((h >> 10) & 0x3ff) as f32 / 511.5 - 1.0;
        let gz = ((h >> 20) & 0x3ff) as f32 / 511.5 - 1.0;
        gx * (f.x - dx as f32) + gy * (f.y - dy as f32) + gz * (f.z - dz as f32)
    };
    let (u, v, w) = (fade(f.x), fade(f.y), fade(f.z));
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = l(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = l(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = l(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = l(g(0, 1, 1), g(1, 1, 1), u);
    l(l(x00, x10, v), l(x01, x11, v), w)
}

/// Periodic 3D Worley noise: distance (cells) to the nearest feature point.
fn worley3(p: Vec3, period: i32, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (ix, iy, iz) = (i.x as i32, i.y as i32, i.z as i32);
    let mut d = 9.0f32;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let h = hash3(
                    (ix + dx).rem_euclid(period),
                    (iy + dy).rem_euclid(period),
                    (iz + dz).rem_euclid(period),
                    seed,
                );
                let o = Vec3::new(
                    (h & 0x3ff) as f32,
                    ((h >> 10) & 0x3ff) as f32,
                    ((h >> 20) & 0x3ff) as f32,
                ) / 1023.0;
                let r = Vec3::new(dx as f32, dy as f32, dz as f32) + o - f;
                d = d.min(r.length_squared());
            }
        }
    }
    d.sqrt()
}

/// Evaluates `f` over a `res`³ volume (texel centres in 0..1) on all cores.
fn volume(res: u32, f: impl Fn(Vec3) -> [f32; 4] + Sync) -> Vec<[f32; 4]> {
    let n = res as usize;
    let mut out = vec![[0.0f32; 4]; n * n * n];
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let slices_per = n.div_ceil(threads);
    std::thread::scope(|s| {
        for (k, chunk) in out.chunks_mut(slices_per * n * n).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, v) in chunk.iter_mut().enumerate() {
                    let idx = k * slices_per * n * n + j;
                    let (x, y, z) = (idx % n, (idx / n) % n, idx / (n * n));
                    *v = f((Vec3::new(x as f32, y as f32, z as f32) + 0.5) / res as f32);
                }
            });
        }
    });
    out
}

/// Stretches each channel to 0..1 (0.3 % / 99.7 % percentiles) and quantises to bytes.
fn normalize_rgba8(data: &[[f32; 4]]) -> Vec<u8> {
    let mut lo = [0.0f32; 4];
    let mut hi = [1.0f32; 4];
    for c in 0..4 {
        let mut s: Vec<f32> = data.iter().step_by(7).map(|v| v[c]).collect();
        s.sort_by(f32::total_cmp);
        if s.is_empty() {
            continue;
        }
        lo[c] = s[s.len() * 3 / 1000];
        hi[c] = s[(s.len() * 997 / 1000).min(s.len() - 1)];
    }
    let mut out = Vec::with_capacity(data.len() * 4);
    for v in data {
        for c in 0..4 {
            let t = ((v[c] - lo[c]) / (hi[c] - lo[c]).max(1e-6)).clamp(0.0, 1.0);
            out.push((t * 255.0 + 0.5) as u8);
        }
    }
    out
}

fn repeat_sampler(mipmaps: bool) -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: if mipmaps {
            ImageFilterMode::Linear
        } else {
            ImageFilterMode::Nearest
        },
        ..default()
    })
}

/// Base shape (128³, one tile = 4096 m): r = Perlin–Worley, gba = Worley at 2×, 4×, 8× frequency.
pub(crate) fn base_volume() -> Image {
    let data = volume(BASE_RES, |p| {
        let pf = 0.55 * perlin3(p * 4.0, 4, 1)
            + 0.3 * perlin3(p * 8.0, 8, 2)
            + 0.15 * perlin3(p * 16.0, 16, 3);
        let pf = (pf * 1.3 + 0.5).clamp(0.0, 1.0);
        let w4 = 1.0 - worley3(p * 4.0, 4, 4);
        let w8 = 1.0 - worley3(p * 8.0, 8, 5);
        let w16 = 1.0 - worley3(p * 16.0, 16, 6);
        let w32 = 1.0 - worley3(p * 32.0, 32, 7);
        let wf = w4 * 0.625 + w8 * 0.25 + w16 * 0.125;
        // Perlin–Worley: the billows of the Worley fbm, broken up by the Perlin noise
        let pw = (pf - (wf - 1.0)) / (2.0 - wf);
        [pw, w8, w16, w32]
    });
    let mut img = Image::new(
        Extent3d {
            width: BASE_RES,
            height: BASE_RES,
            depth_or_array_layers: BASE_RES,
        },
        TextureDimension::D3,
        normalize_rgba8(&data),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = repeat_sampler(false);
    img
}

/// Detail erosion (32³, one tile = 512 m): rgb = Worley at 2, 4, 8 cells per tile.
pub(crate) fn detail_volume() -> Image {
    let data = volume(DETAIL_RES, |p| {
        [
            1.0 - worley3(p * 2.0, 2, 8),
            1.0 - worley3(p * 4.0, 4, 9),
            1.0 - worley3(p * 8.0, 8, 10),
            1.0,
        ]
    });
    let mut img = Image::new(
        Extent3d {
            width: DETAIL_RES,
            height: DETAIL_RES,
            depth_or_array_layers: DETAIL_RES,
        },
        TextureDimension::D3,
        normalize_rgba8(&data),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = repeat_sampler(false);
    img
}

/// The baked cover field (`cover_map`), for other shaders that need the clouds' cover (the
/// terrain's cloud shadows sample it instead of evaluating `noise.wesl::cloud_noise` per pixel).
#[derive(Resource, Clone)]
pub struct CloudCoverMap(pub Handle<Image>);

/// The cover field over one tile (`cloud_noise`, baked; the coverage threshold is applied live).
fn cover_map() -> Image {
    let n = COVER_RES as usize;
    let mut data = vec![0u8; n * n * 2];
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let rows_per = n.div_ceil(threads);
    std::thread::scope(|s| {
        for (k, chunk) in data.chunks_mut(rows_per * n * 2).enumerate() {
            s.spawn(move || {
                for (j, px) in chunk.chunks_mut(2).enumerate() {
                    let idx = k * rows_per * n + j;
                    let (x, y) = (idx % n, idx / n);
                    let v = cloud_noise((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
                    px.copy_from_slice(&half::f16::from_f32(v).to_le_bytes());
                }
            });
        }
    });
    let mut img = Image::new(
        Extent3d {
            width: COVER_RES,
            height: COVER_RES,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = repeat_sampler(false);
    img
}

/// Cirrus (one tile, mipmapped): r = fibres along x, g = patches, b = fibres along (3, 1).
pub(crate) fn cirrus_map() -> Image {
    let n = CIRRUS_RES as usize;
    let fibres = |u: f32, v: f32, seed: u32| {
        // stretched gradient noise (8:1), three octaves: long thin streaks
        let mut s = 0.0;
        let mut amp = 0.55;
        for o in 0..3 {
            let k = 1 << o;
            s += amp
                * cover_gnoise(
                    u * (6 * k) as f32,
                    v * (48 * k) as f32,
                    6 * k,
                    seed + o as u32,
                )
                .abs();
            amp *= 0.55;
        }
        s
    };
    let mut px = vec![[0.0f32; 4]; n * n];
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .clamp(1, 16);
    let rows_per = n.div_ceil(threads);
    std::thread::scope(|s| {
        for (k, chunk) in px.chunks_mut(rows_per * n).enumerate() {
            s.spawn(move || {
                for (j, v) in chunk.iter_mut().enumerate() {
                    let idx = k * rows_per * n + j;
                    let (x, y) = ((idx % n) as f32 + 0.5, (idx / n) as f32 + 0.5);
                    let (qx, qy) = (x / n as f32, y / n as f32);
                    // gentle curves in the fibres
                    let wx = cover_gnoise(qx * 4.0, qy * 4.0, 4, 41) * 0.05;
                    let wy = cover_gnoise(qx * 4.0 + 5.2, qy * 4.0, 4, 42) * 0.05;
                    let (ux, uy) = (qx + wx, qy + wy);
                    let f1 = fibres(ux, uy, 51);
                    // along (3, 1): rotated lattice coordinates stay periodic
                    let f2 = fibres(ux * 3.0 + uy, uy * 3.0 - ux, 61);
                    let patch = 0.5
                        + 0.5
                            * (0.6 * cover_gnoise(qx * 2.0, qy * 2.0, 2, 71)
                                + 0.3 * cover_gnoise(qx * 4.0, qy * 4.0, 4, 72)
                                + 0.1 * cover_gnoise(qx * 8.0, qy * 8.0, 8, 73));
                    *v = [f1, patch, f2, 1.0];
                }
            });
        }
    });
    let base = normalize_rgba8(&px);
    // box-filtered mip chain
    let mut levels = vec![base];
    let mut size = n;
    while size > 1 {
        let prev = levels.last().unwrap();
        let half = size / 2;
        let mut next = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let s: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                        .iter()
                        .map(|(dx, dy)| prev[((y * 2 + dy) * size + x * 2 + dx) * 4 + c] as u32)
                        .sum();
                    next[(y * half + x) * 4 + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        levels.push(next);
        size = half;
    }
    let mips = levels.len() as u32;
    let mut img = Image::new(
        Extent3d {
            width: CIRRUS_RES,
            height: CIRRUS_RES,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        levels[0].clone(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(levels.concat());
    img.texture_descriptor.mip_level_count = mips;
    img.sampler = repeat_sampler(true);
    img
}

// ------------------------------------------------------------------------------------------------
// Main world

/// Uniforms of the cloud passes (matrices are filled in by the render world).
#[derive(Clone, Copy, Default, Debug, ShaderType)]
pub struct CloudGpu {
    pub world_from_clip: Mat4,
    pub prev_clip_from_world: Mat4,
    /// xyz: camera position, w: frame index
    pub camera: Vec4,
    /// xy: trace size (px), zw: view size (px)
    pub size: Vec4,
    /// xy: history size (px), zw: this frame's traced pixel in each 2x2 block
    pub grid: Vec4,
    /// x: coverage, y: extinction of the densest cloud (1/m), z: cloud type, w: cirrus amount
    pub cover: Vec4,
    /// xy: cumulus field offset (m), zw: cirrus offset (m)
    pub offset: Vec4,
    /// xy: change of the cumulus offset since the last frame, z: history valid, w: time (s)
    pub motion: Vec4,
    /// xyz: direction towards the key light (sun, or the moon at night), w: 1 for the moon
    pub key: Vec4,
    /// rgb: key illuminance above the atmosphere (lux), w: unused
    pub key_color: Vec4,
    /// xyz: direction towards the second light, w: unused
    pub fill: Vec4,
    /// rgb: second light's illuminance (lux)
    pub fill_color: Vec4,
    /// x: base altitude, y: top altitude, z: cirrus altitude, w: near fade distance (m)
    pub layer: Vec4,
    /// x: ambient scale, y: storm darkening, z: night (0..1), w: unused
    pub shade: Vec4,
    /// rgb: the scene's artificial ambient fill (radiance, keeps night clouds readable like the
    /// terrain), w: unused
    pub fill_ambient: Vec4,
    /// x: coarse steps, y: light samples, z: weight of a new sample, w: history scale
    pub quality: Vec4,
}

/// Per-frame cloud state handed to the render world.
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
struct CloudFrame {
    enabled: bool,
    /// The history is 1 / scale of the view resolution.
    scale: u32,
    gpu: CloudGpu,
    base: Handle<Image>,
    detail: Handle<Image>,
    cover: Handle<Image>,
    cirrus: Handle<Image>,
}

/// On each cloud camera: the image the accumulated clouds are resolved into (history size).
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
pub(crate) struct CloudView {
    /// (`volumetrics` fogs it before the composite lays it over the sky)
    pub(crate) target: Handle<Image>,
    size: UVec2,
    scale: u32,
    reduced: bool,
}

/// One traced pixel per 2x2 block per frame, in this order.
const INTERLEAVE: [(u32, u32); 4] = [(0, 0), (1, 1), (1, 0), (0, 1)];

/// Marker of the composite dome.
#[derive(Component)]
struct CloudComposite;

/// A second dome visible only to the planar-reflection camera. It samples that camera's own
/// reduced cloud history; the main cloud dome and its history remain untouched.
#[derive(Component)]
struct ReflectionCloudComposite;

/// Blends the resolved clouds (premultiplied) over the scene, after the star dome.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct CloudCompositeMaterial {
    #[texture(0)]
    #[sampler(1)]
    image: Handle<Image>,
    /// x: cloud base altitude, y: nearest distance a cloud can be from the camera (near fade),
    /// z: earth radius. Per pixel this gives a conservative cloud depth that is depth-tested
    /// against the scene, so low-resolution clouds never smear over nearer trees or terrain.
    #[uniform(2)]
    params: Vec4,
}

impl Material for CloudCompositeMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/clouds_composite.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    /// Right after the star dome (−1e9) and before every other transparent surface.
    fn depth_bias(&self) -> f32 {
        -1.0e8
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    /// Covers the whole view; each fragment writes the nearest depth its clouds can have and is
    /// tested against the scene (the trace samples at low resolution, taking the farthest depth
    /// of its block, so without the test far clouds bled over foliage and silhouettes).
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_compare = Some(CompareFunction::GreaterEqual);
            ds.depth_write_enabled = Some(false);
        }
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

fn target_image(size: UVec2) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

fn setup_clouds(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<CloudCompositeMaterial>>,
    mut lighting: ResMut<Lighting>,
) {
    let t0 = std::time::Instant::now();
    let base = images.add(base_volume());
    let detail = images.add(detail_volume());
    let cover = images.add(cover_map());
    let cirrus = images.add(cirrus_map());
    info!("generated cloud noise in {:?}", t0.elapsed());
    let target = images.add(target_image(UVec2::ONE));
    let reflection_target = images.add(target_image(UVec2::ONE));
    commands.insert_resource(CloudCoverMap(cover.clone()));
    commands.insert_resource(CloudFrame {
        enabled: false,
        scale: 2,
        gpu: CloudGpu::default(),
        base,
        detail,
        cover,
        cirrus,
    });
    // Any mesh around the main camera covers the view (depth test off); a small sphere keeps it cheap.
    commands.spawn((
        Name::new("Clouds"),
        CloudComposite,
        Mesh3d(meshes.add(Sphere::new(10.0).mesh().uv(16, 8))),
        MeshMaterial3d(mats.add(CloudCompositeMaterial {
            image: target,
            params: Vec4::new(CLOUD_BASE, 270.0, EARTH_RADIUS, 0.0),
        })),
        Transform::IDENTITY,
        // The planar capture owns a separate reduced cloud history and composite layer.
        // Keep this full-resolution main-view dome out of that capture.
        RenderLayers::layer(crate::water_render::CLOUD_RENDER_LAYER),
        NotShadowCaster,
        NotShadowReceiver,
        NoFrustumCulling,
    ));
    commands.spawn((
        Name::new("Water reflection clouds"),
        ReflectionCloudComposite,
        // The mirror eye can sit below its oblique water clip plane. A normal 10 m sky dome
        // would then be entirely clipped before this material can composite its cloud history;
        // This is below the camera's 80 km far plane and reaches the traced cloud horizon.
        Mesh3d(meshes.add(Sphere::new(REFLECTION_CLOUD_DOME_RADIUS).mesh().uv(16, 8))),
        MeshMaterial3d(mats.add(CloudCompositeMaterial {
            image: reflection_target,
            params: Vec4::new(CLOUD_BASE, 270.0, EARTH_RADIUS, 0.0),
        })),
        Transform::IDENTITY,
        RenderLayers::layer(crate::water_render::REFLECTION_CLOUD_RENDER_LAYER),
        NotShadowCaster,
        NotShadowReceiver,
        NoFrustumCulling,
    ));
    // `WB_CLOUD_QUALITY=low|medium|high`
    if let Ok(q) = std::env::var("WB_CLOUD_QUALITY")
        && let Some(q) = CloudQuality::ALL
            .into_iter()
            .find(|x| format!("{x:?}").eq_ignore_ascii_case(&q))
    {
        lighting.cloud_quality = q;
    }
    // `WB_CLOUDS=cover=0.5,type=0.8,density=1,cirrus=0.4,speed=1`: set the cloud fields
    if let Ok(spec) = std::env::var("WB_CLOUDS") {
        for kv in spec.split(',') {
            let Some((k, v)) = kv.split_once('=') else {
                continue;
            };
            let Ok(v) = v.trim().parse::<f32>() else {
                continue;
            };
            match k.trim() {
                "cover" => lighting.cloud_coverage = v,
                "type" => lighting.cloud_type = v,
                "density" => lighting.cloud_density = v,
                "cirrus" => lighting.cirrus = v,
                "speed" => lighting.cloud_speed = v,
                _ => {}
            }
        }
    }
    // `WB_CLOUD_SEED=<n>`: start with a random sky (testing the randomiser)
    if let Ok(s) = std::env::var("WB_CLOUD_SEED")
        && let Ok(seed) = s.parse::<u64>()
    {
        let preset = randomize_clouds(&mut lighting, seed);
        info!(
            "clouds: {preset:?} (cover {:.2}, type {:.2}, density {:.2}, cirrus {:.2})",
            lighting.cloud_coverage, lighting.cloud_type, lighting.cloud_density, lighting.cirrus
        );
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_clouds(
    mut commands: Commands,
    lighting: Res<Lighting>,
    sky: Res<SkyState>,
    look: Res<LookState>,
    time: Res<Time>,
    mut frame: ResMut<CloudFrame>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<CloudCompositeMaterial>>,
    cams: Query<(Entity, &Camera, &Transform, Option<&CloudView>), With<MainCamera>>,
    sun: Query<&DirectionalLight, (With<Sun>, Without<Moon>)>,
    moon: Query<&DirectionalLight, (With<Moon>, Without<Sun>)>,
    mut composite: Query<
        (
            &mut Transform,
            &MeshMaterial3d<CloudCompositeMaterial>,
            &Visibility,
        ),
        (
            With<CloudComposite>,
            Without<ReflectionCloudComposite>,
            Without<MainCamera>,
            Without<crate::water_render::WaterReflectionCamera>,
        ),
    >,
    ambient: Res<GlobalAmbientLight>,
    orbit: Res<crate::camera::OrbitCamera>,
    mut state: Local<(Option<Vec2>, Vec2, u32)>,
    mut look_now: Local<Option<Vec3>>,
) {
    let Ok((cam_e, cam, cam_t, view)) = cams.single() else {
        return;
    };
    let Ok((mut dome_t, dome_mat, vis)) = composite.single_mut() else {
        return;
    };
    dome_t.translation = cam_t.translation;

    // resolve target at the trace resolution
    let full = cam
        .physical_viewport_size()
        .unwrap_or(UVec2::ONE)
        .max(UVec2::ONE);
    let scale = lighting.cloud_quality.history_scale();
    let size = (full + scale - 1) / scale;
    let trace = (size + 1) / 2;
    if view.is_none_or(|v| v.size != size) {
        let target = images.add(target_image(size));
        if let Some(mut m) = mats.get_mut(&dome_mat.0) {
            m.image = target.clone();
        }
        commands.entity(cam_e).insert(CloudView {
            target,
            size,
            scale,
            reduced: false,
        });
    }

    // wind drift since the last frame (the offset wraps around the tile)
    let (prev_offset, cirrus_drift, frame_idx) = &mut *state;
    let offset = sky.cloud_offset;
    let mut delta = prev_offset.map(|p| offset - p).unwrap_or(Vec2::ZERO);
    delta = (delta + CLOUD_TILE * 0.5).rem_euclid(Vec2::splat(CLOUD_TILE)) - CLOUD_TILE * 0.5;
    *prev_offset = Some(offset);
    // the upper wind is faster and veered
    *cirrus_drift = wrap_offset(
        *cirrus_drift + Vec2::new(delta.x * 1.4 - delta.y * 0.5, delta.y * 1.4 + delta.x * 0.5),
    );
    *frame_idx = frame_idx.wrapping_add(1);

    let w = sky.weather;
    let coverage = w.clouds.clamp(0.0, 1.0);
    // type, density and cirrus follow the settings smoothly (a new random sky blends in)
    let target = Vec3::new(
        lighting.cloud_type.clamp(0.0, 1.0),
        lighting.cloud_density.clamp(0.05, 4.0),
        lighting.cirrus.clamp(0.0, 1.0),
    );
    let k = 1.0 - (-time.delta_secs() * 2.5).exp();
    let cur = look_now.map_or(target, |c| c.lerp(target, k));
    *look_now = Some(cur);
    // overcast decks spread out flat; rain makes them dense and dark
    let ty = cur.x + (0.4 - cur.x) * smoothstep(0.8, 1.0, coverage) * 0.7;
    let density = cur.y * (1.0 + 0.8 * w.rain + 0.4 * w.dim);
    let top = CLOUD_BASE + 400.0 + 2600.0 * ty * ty;

    // lights: the sun's illuminance before the horizon fade (the atmosphere and the earth's shadow
    // are applied per cloud, which stays lit a while after sunset), and the moon
    let sun_dir = sky.sun_dir;
    let moon_dir = sky.moon_dir;
    let (sun_e, sun_c) = sun
        .single()
        .map(|l| {
            let above = smoothstep(-0.06, 0.02, sun_dir.y).max(0.0001);
            (l.illuminance / above, l.color.to_linear())
        })
        .unwrap_or((
            bevy::light::light_consts::lux::RAW_SUNLIGHT * look.params.sun,
            LinearRgba::WHITE,
        ));
    let (moon_e, moon_c) = moon
        .single()
        .map(|l| (l.illuminance, l.color.to_linear()))
        .unwrap_or((0.0, LinearRgba::WHITE));
    let sun_col = Vec3::new(sun_c.red, sun_c.green, sun_c.blue) * sun_e;
    let moon_col = Vec3::new(moon_c.red, moon_c.green, moon_c.blue) * moon_e;
    let sun_key = sun_dir.y > -0.04;
    // gloomy weather: a deck above keeps most direct light off the clouds below it
    let gloom = 1.0 - 0.6 * sky.weather.dim;
    let (sun_col, moon_col) = (sun_col * gloom, moon_col * gloom);
    let (key, key_col, fill, fill_col) = if sun_key {
        (sun_dir, sun_col, moon_dir, moon_col)
    } else {
        (moon_dir, moon_col, sun_dir, sun_col)
    };

    // clouds right around the camera fade out. When the editor's orbit camera is up among or
    // above the clouds, everything most of the way to its focus does, so clouds never hide the
    // map being edited (their shadows still show where they are)
    let high = smoothstep(CLOUD_BASE - 700.0, CLOUD_BASE, cam_t.translation.y);
    let near_fade = if orbit.enabled {
        450.0 + ((orbit.distance * 0.85).max(450.0) - 450.0) * high
    } else {
        450.0
    };

    // the composite's depth test: no cloud is nearer than 0.6 × the near fade (see the trace)
    let params = Vec4::new(CLOUD_BASE, near_fade * 0.6, EARTH_RADIUS, 0.0);
    if mats
        .get(&dome_mat.0)
        .is_some_and(|m| (m.params - params).abs().max_element() > 1.0)
        && let Some(mut m) = mats.get_mut(&dome_mat.0)
    {
        m.params = params;
    }
    frame.enabled = *vis != Visibility::Hidden;
    frame.scale = scale;
    let (steps, light_steps, blend) = lighting.cloud_quality.march();
    frame.gpu = CloudGpu {
        world_from_clip: Mat4::IDENTITY,
        prev_clip_from_world: Mat4::IDENTITY,
        camera: cam_t.translation.extend(*frame_idx as f32),
        size: Vec4::new(trace.x as f32, trace.y as f32, full.x as f32, full.y as f32),
        grid: {
            let o = INTERLEAVE[(*frame_idx % 4) as usize];
            Vec4::new(size.x as f32, size.y as f32, o.0 as f32, o.1 as f32)
        },
        cover: Vec4::new(coverage, 0.06 * density, ty, cur.z),
        offset: Vec4::new(offset.x, offset.y, cirrus_drift.x, cirrus_drift.y),
        motion: Vec4::new(delta.x, delta.y, 1.0, time.elapsed_secs_wrapped()),
        key: key.extend(if sun_key { 0.0 } else { 1.0 }),
        key_color: key_col.extend(0.0),
        fill: fill.extend(0.0),
        fill_color: fill_col.extend(0.0),
        layer: Vec4::new(CLOUD_BASE, top, CIRRUS_ALT, near_fade),
        shade: Vec4::new(1.0 - 0.55 * w.dim, w.dim, sky.night, 0.0),
        quality: Vec4::new(steps, light_steps, blend, scale as f32),
        fill_ambient: {
            // mostly grey: the fill's blue tint would paint thin cloud flat navy
            let c = ambient.color.to_linear();
            let c = Vec3::new(c.red, c.green, c.blue);
            let grey = Vec3::splat(c.dot(Vec3::new(0.2126, 0.7152, 0.0722)));
            (grey.lerp(c, 0.35) * ambient.brightness * 0.09).extend(0.0)
        },
    };
}

/// Gives the planar-reflection camera its own reduced temporal cloud target and composite dome.
/// The field, weather and light inputs are still produced by `update_clouds`; only the
/// view-dependent history and trace resolution differ.
fn update_reflection_clouds(
    mut commands: Commands,
    reflection: Res<crate::water_render::WaterReflection>,
    main: Query<(&Transform, &Visibility), With<MainCamera>>,
    capture: Query<
        (Entity, &Transform, Option<&CloudView>),
        With<crate::water_render::WaterReflectionCamera>,
    >,
    orbit: Res<crate::camera::OrbitCamera>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<CloudCompositeMaterial>>,
    mut dome: Query<
        (
            &mut Transform,
            &MeshMaterial3d<CloudCompositeMaterial>,
            &mut Visibility,
        ),
        (
            With<ReflectionCloudComposite>,
            Without<CloudComposite>,
            Without<MainCamera>,
            Without<crate::water_render::WaterReflectionCamera>,
        ),
    >,
) {
    let Ok((main_t, main_visibility)) = main.single() else {
        return;
    };
    let Ok((capture_e, capture_t, capture_view)) = capture.single() else {
        return;
    };
    let Ok((mut dome_t, dome_mat, mut dome_visibility)) = dome.single_mut() else {
        return;
    };

    // The planar target is half-resolution; this third-resolution history makes the extra cloud
    // trace one ninth of the main history's pixel count at the same window size.
    let scale = 3;
    let size = (reflection.size + scale - 1) / scale;
    if capture_view.is_none_or(|v| v.size != size || v.scale != scale || !v.reduced) {
        let target = images.add(target_image(size));
        if let Some(mut mat) = mats.get_mut(&dome_mat.0) {
            mat.image = target.clone();
        }
        commands.entity(capture_e).insert(CloudView {
            target,
            size,
            scale,
            reduced: true,
        });
    }

    dome_t.translation = capture_t.translation;
    *dome_visibility = *main_visibility;
    let high = smoothstep(CLOUD_BASE - 700.0, CLOUD_BASE, main_t.translation.y);
    let near_fade = if orbit.enabled {
        450.0 + ((orbit.distance * 0.85).max(450.0) - 450.0) * high
    } else {
        450.0
    };
    let params = Vec4::new(CLOUD_BASE, near_fade * 0.6, EARTH_RADIUS, 0.0);
    if mats
        .get(&dome_mat.0)
        .is_some_and(|mat| (mat.params - params).abs().max_element() > 1.0)
        && let Some(mut mat) = mats.get_mut(&dome_mat.0)
    {
        mat.params = params;
    }
}

// ------------------------------------------------------------------------------------------------
// Render world

#[derive(Resource)]
struct CloudPipelines {
    atmo_layout: BindGroupLayoutDescriptor,
    trace_layout: BindGroupLayoutDescriptor,
    resolve_layout: BindGroupLayoutDescriptor,
    trace: CachedRenderPipelineId,
    resolve: CachedRenderPipelineId,
    repeat: Sampler,
    clamp: Sampler,
}

fn init_cloud_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
) {
    let filterable = TextureSampleType::Float { filterable: true };
    // the subset of Bevy's atmosphere bindings (same indices) the trace uses
    let atmo_layout = BindGroupLayoutDescriptor::new(
        "clouds_atmosphere_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (0, uniform_buffer::<GpuAtmosphere>(true)),
                (1, uniform_buffer::<GpuAtmosphereSettings>(true)),
                (2, uniform_buffer::<AtmosphereTransform>(true)),
                (3, uniform_buffer::<ViewUniform>(true)),
                (8, texture_2d(filterable)),
                (10, texture_2d(filterable)),
                (11, texture_3d(filterable)),
                (12, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    );
    let trace_layout = BindGroupLayoutDescriptor::new(
        "clouds_trace_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<CloudGpu>(false),
                texture_3d(filterable),
                texture_3d(filterable),
                texture_2d(filterable),
                texture_2d(filterable),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
            ),
        ),
    );
    let resolve_layout = BindGroupLayoutDescriptor::new(
        "clouds_resolve_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<CloudGpu>(false),
                texture_2d(filterable),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_2d(filterable),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let trace_shader = assets.load("shaders/clouds.wesl");
    let resolve_shader = assets.load("shaders/clouds_resolve.wesl");
    let target = |format| {
        Some(ColorTargetState {
            format,
            blend: None,
            write_mask: ColorWrites::ALL,
        })
    };
    let trace = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("clouds_trace".into()),
        layout: vec![atmo_layout.clone(), trace_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: trace_shader,
            targets: vec![
                target(TextureFormat::Rgba16Float),
                target(TextureFormat::R32Float),
            ],
            ..default()
        }),
        ..default()
    });
    let resolve = cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some("clouds_resolve".into()),
        layout: vec![resolve_layout.clone()],
        vertex: fullscreen.to_vertex_state(),
        fragment: Some(FragmentState {
            shader: resolve_shader,
            targets: vec![target(TextureFormat::Rgba16Float)],
            ..default()
        }),
        ..default()
    });
    let repeat = device.create_sampler(&SamplerDescriptor {
        label: Some("clouds_repeat"),
        address_mode_u: bevy::render::render_resource::AddressMode::Repeat,
        address_mode_v: bevy::render::render_resource::AddressMode::Repeat,
        address_mode_w: bevy::render::render_resource::AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: bevy::render::render_resource::MipmapFilterMode::Linear,
        ..default()
    });
    let clamp = device.create_sampler(&SamplerDescriptor {
        label: Some("clouds_clamp"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(CloudPipelines {
        atmo_layout,
        trace_layout,
        resolve_layout,
        trace,
        resolve,
        repeat,
        clamp,
    });
}

struct ViewTextures {
    size: UVec2,
    trace: (Texture, TextureView),
    depth: (Texture, TextureView),
    history: (Texture, TextureView),
}

/// Render-world history and scratch textures for one cloud camera.
#[derive(Resource, Default)]
struct CloudViewState {
    textures: Option<ViewTextures>,
    uniform: UniformBuffer<CloudGpu>,
    prev_clip_from_world: Option<Mat4>,
    /// Last cloud-field offset this view actually rendered. Reflection captures may be skipped,
    /// so their temporal reprojection needs the accumulated wind rather than one frame's drift.
    prev_cloud_offset: Option<Vec2>,
    /// The 2×2 reconstruction phase advances only when this particular view renders.
    frame_index: Option<u32>,
    /// The pass ran last frame with the current textures (the history holds clouds).
    history: bool,
}

/// Each CloudView gets its own temporal history. The reflection camera shares the same volume
/// assets and lighting frame, but must never reproject through the main camera's history.
#[derive(Resource, Default)]
struct CloudState {
    views: HashMap<Entity, CloudViewState>,
}

#[derive(Component)]
struct CloudBindGroups {
    atmo: BindGroup,
    trace: BindGroup,
    resolve: BindGroup,
}

fn make_texture(
    device: &RenderDevice,
    label: &'static str,
    size: UVec2,
    format: TextureFormat,
    usage: TextureUsages,
) -> (Texture, TextureView) {
    let tex = device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let view = tex.create_view(&TextureViewDescriptor::default());
    (tex, view)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn prepare_clouds(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipes: Option<Res<CloudPipelines>>,
    cache: Res<PipelineCache>,
    frame: Option<Res<CloudFrame>>,
    images: Res<RenderAssets<GpuImage>>,
    view_uniforms: Res<ViewUniforms>,
    atmo_uniforms: Res<ComponentUniforms<GpuAtmosphere>>,
    settings_uniforms: Res<ComponentUniforms<GpuAtmosphereSettings>>,
    atmo_transforms: Res<AtmosphereTransforms>,
    atmo_sampler: Res<AtmosphereSampler>,
    mut state: ResMut<CloudState>,
    views: Query<(
        Entity,
        &ExtractedView,
        &CloudView,
        &AtmosphereTextures,
        &ViewDepthStencilTexture,
    )>,
) {
    let (Some(pipes), Some(frame)) = (pipes, frame) else {
        return;
    };
    let ready = frame.enabled
        && cache.get_render_pipeline(pipes.trace).is_some()
        && cache.get_render_pipeline(pipes.resolve).is_some();
    let (Some(view_binding), Some(atmo_binding), Some(settings_binding), Some(transforms_binding)) = (
        view_uniforms.uniforms.binding(),
        atmo_uniforms.uniforms().binding(),
        settings_uniforms.uniforms().binding(),
        atmo_transforms.uniforms().binding(),
    ) else {
        return;
    };
    let (Some(base), Some(detail), Some(cover), Some(cirrus)) = (
        images.get(&frame.base),
        images.get(&frame.detail),
        images.get(&frame.cover),
        images.get(&frame.cirrus),
    ) else {
        return;
    };

    for (entity, view, cv, atmo_tex, depth) in &views {
        let size = cv.size;
        let trace_size = (size + 1) / 2;
        let full = UVec2::new(view.viewport.z, view.viewport.w);
        // Each view owns a target at its requested history scale. This is also what lets the
        // reflection camera use a reduced cloud trace without changing the main camera's quality.
        if !ready || (full + cv.scale - 1) / cv.scale != size {
            state.views.remove(&entity);
            commands.entity(entity).remove::<CloudBindGroups>();
            continue;
        }
        let Some(target) = images.get(&cv.target) else {
            state.views.remove(&entity);
            commands.entity(entity).remove::<CloudBindGroups>();
            continue;
        };
        if target.texture.size().width != size.x || target.texture.size().height != size.y {
            state.views.remove(&entity);
            commands.entity(entity).remove::<CloudBindGroups>();
            continue;
        }
        let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
            state.views.remove(&entity);
            commands.entity(entity).remove::<CloudBindGroups>();
            continue;
        };

        let view_state = state.views.entry(entity).or_default();
        if view_state.textures.as_ref().is_none_or(|t| t.size != size) {
            let rt = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
            view_state.textures = Some(ViewTextures {
                size,
                trace: make_texture(
                    &device,
                    "clouds_trace",
                    trace_size,
                    TextureFormat::Rgba16Float,
                    rt,
                ),
                depth: make_texture(
                    &device,
                    "clouds_trace_depth",
                    trace_size,
                    TextureFormat::R32Float,
                    rt,
                ),
                history: make_texture(
                    &device,
                    "clouds_history",
                    size,
                    TextureFormat::Rgba16Float,
                    TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                ),
            });
            view_state.history = false;
        }

        let clip_from_world = view
            .clip_from_world
            .unwrap_or_else(|| view.clip_from_view * view.world_from_view.to_matrix().inverse());
        let mut gpu = frame.gpu;
        gpu.size = Vec4::new(
            trace_size.x as f32,
            trace_size.y as f32,
            full.x as f32,
            full.y as f32,
        );
        let frame_index = *view_state
            .frame_index
            .get_or_insert(frame.gpu.camera.w as u32);
        let interleave = INTERLEAVE[(frame_index % INTERLEAVE.len() as u32) as usize];
        gpu.grid = Vec4::new(
            size.x as f32,
            size.y as f32,
            interleave.0 as f32,
            interleave.1 as f32,
        );
        // The mirror already renders at half the main resolution. Use the Low trace budget for
        // its additional history, while retaining the same cloud field, weather and light state.
        if cv.reduced {
            let (steps, light_steps, blend) = CloudQuality::Low.march();
            gpu.quality = Vec4::new(steps, light_steps, blend, cv.scale as f32);
        } else {
            gpu.quality.w = cv.scale as f32;
        }
        gpu.world_from_clip = clip_from_world.inverse();
        gpu.prev_clip_from_world = view_state.prev_clip_from_world.unwrap_or(clip_from_world);
        gpu.camera = view
            .world_from_view
            .translation()
            .extend(frame_index as f32);
        let current_offset = frame.gpu.offset.xy();
        if let Some(previous_offset) = view_state.prev_cloud_offset {
            let drift = (current_offset - previous_offset + Vec2::splat(CLOUD_TILE * 0.5))
                .rem_euclid(Vec2::splat(CLOUD_TILE))
                - Vec2::splat(CLOUD_TILE * 0.5);
            gpu.motion.x = drift.x;
            gpu.motion.y = drift.y;
        }
        view_state.prev_cloud_offset = Some(current_offset);
        if !view_state.history {
            gpu.motion.z = 0.0;
        }
        view_state.prev_clip_from_world = Some(clip_from_world);
        view_state.frame_index = Some(frame_index.wrapping_add(1));
        view_state.uniform.set(gpu);
        view_state.uniform.write_buffer(&device, &queue);
        view_state.history = true;

        let Some(uniform) = view_state.uniform.binding() else {
            continue;
        };
        let tex = view_state.textures.as_ref().unwrap();
        let atmo = device.create_bind_group(
            "clouds_atmosphere",
            &cache.get_bind_group_layout(&pipes.atmo_layout),
            &BindGroupEntries::with_indices((
                (0, atmo_binding.clone()),
                (1, settings_binding.clone()),
                (2, transforms_binding.clone()),
                (3, view_binding.clone()),
                (8, &atmo_tex.transmittance_lut.default_view),
                (10, &atmo_tex.sky_view_lut.default_view),
                (11, &atmo_tex.aerial_view_lut.default_view),
                (12, &**atmo_sampler),
            )),
        );
        let trace = device.create_bind_group(
            "clouds_trace",
            &cache.get_bind_group_layout(&pipes.trace_layout),
            &BindGroupEntries::sequential((
                uniform.clone(),
                &base.texture_view,
                &detail.texture_view,
                &cover.texture_view,
                &cirrus.texture_view,
                &pipes.repeat,
                depth_view,
            )),
        );
        let resolve = device.create_bind_group(
            "clouds_resolve",
            &cache.get_bind_group_layout(&pipes.resolve_layout),
            &BindGroupEntries::sequential((
                uniform,
                &tex.trace.1,
                &tex.depth.1,
                &tex.history.1,
                &pipes.clamp,
            )),
        );
        commands.entity(entity).insert(CloudBindGroups {
            atmo,
            trace,
            resolve,
        });
    }
}

#[allow(clippy::type_complexity)]
fn clouds_pass(
    view: ViewQuery<(
        Entity,
        &CloudView,
        &CloudBindGroups,
        &DynamicUniformIndex<GpuAtmosphere>,
        &DynamicUniformIndex<GpuAtmosphereSettings>,
        &AtmosphereTransformsOffset,
        &ViewUniformOffset,
    )>,
    pipes: Res<CloudPipelines>,
    cache: Res<PipelineCache>,
    state: Res<CloudState>,
    images: Res<RenderAssets<GpuImage>>,
    mut ctx: RenderContext,
) {
    let (entity, cv, groups, atmo_index, settings_index, transforms_offset, view_offset) =
        view.into_inner();
    let (Some(trace_pipe), Some(resolve_pipe)) = (
        cache.get_render_pipeline(pipes.trace),
        cache.get_render_pipeline(pipes.resolve),
    ) else {
        return;
    };
    let Some(view_state) = state.views.get(&entity) else {
        return;
    };
    let Some(tex) = view_state.textures.as_ref() else {
        return;
    };
    let Some(target) = images.get(&cv.target) else {
        return;
    };
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = ctx.command_encoder();
    let clear = |view| {
        Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations {
                load: LoadOp::Clear(Default::default()),
                store: StoreOp::Store,
            },
        })
    };
    // profiling: `WB_CLOUD_REPEAT=k` traces k times (separate passes, so tile-based GPUs can't
    // skip any); the slope of the frame time over k is the trace's GPU cost
    let repeat = std::env::var("WB_CLOUD_REPEAT")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    for _ in 0..repeat {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("clouds_trace"),
            color_attachments: &[clear(&tex.trace.1), clear(&tex.depth.1)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let span = diagnostics.pass_span(&mut pass, "clouds_trace");
        pass.set_pipeline(trace_pipe);
        pass.set_bind_group(
            0,
            &groups.atmo,
            &[
                atmo_index.index(),
                settings_index.index(),
                transforms_offset.index(),
                view_offset.offset,
            ],
        );
        pass.set_bind_group(1, &groups.trace, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
    }
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("clouds_resolve"),
            color_attachments: &[clear(&target.texture_view)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let span = diagnostics.pass_span(&mut pass, "clouds_resolve");
        pass.set_pipeline(resolve_pipe);
        pass.set_bind_group(0, &groups.resolve, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
    }
    encoder.copy_texture_to_texture(
        target.texture.as_image_copy(),
        tex.history.0.as_image_copy(),
        tex.history.0.size(),
    );
}
