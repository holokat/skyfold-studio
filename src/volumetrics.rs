//! Volumetric light: the air itself, lit. A froxel fog (Wronski, "Volumetric Fog", SIGGRAPH 2014;
//! Hillaire, Frostbite 2015) — the view frustum cut into a grid of cells, thin near the eye and
//! long in the distance — so that sunlight streams in shafts between trees and buildings and
//! through windows into dark rooms, haze glows round the sun, mist lies in the hollows of the
//! ground, and lamps and fires hang soft halos in it.
//!
//! Three passes on the main camera after the opaque, transmissive and cloud-tracing passes and
//! before the transparent pass (`volumetrics*.wesl`):
//! 1. **inject** (compute): one jittered sample per cell of the medium (haze, weather fog, ground
//!    mist, drifting noise) and of the light scattered towards the eye — sun and moon through
//!    their shadow cascades, sky light from Bevy's atmosphere LUTs, the ambient fill, the nearest
//!    lamps — blended with the last frame's reprojected result;
//! 2. **integrate** (compute): each column accumulated away from the eye;
//! 3. **apply**: every pixel looks the volume up at its depth (`colour × T + light`), and the
//!    resolved clouds get the same treatment before they are laid over the sky.
//!
//! It replaces the weather's and the zones' share of Bevy's distance fog (`sky.rs` keeps only
//! the clear-air haze there, for what lies beyond the grid) and most of the screen-space sun
//! shafts. Quality (Lighting › Volumetric light) sets the grid; the look's `air`, `mist` and
//! `sunbeams` shape it. Off in dungeons (they have their own fog), under water and in caves.

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
use bevy::core_pipeline::schedule::Core3d;
use bevy::diagnostic::FrameCount;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::resources::{
    AtmosphereSampler, AtmosphereTextures, AtmosphereTransform, AtmosphereTransforms,
    AtmosphereTransformsOffset, GpuAtmosphere,
};
use bevy::pbr::{
    GpuAtmosphereSettings, GpuLights, LightMeta, ShadowSamplers, ViewLightsUniformOffset,
    ViewShadowBindings, main_transmissive_pass_3d,
};
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{
    DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_2d_array, texture_3d, texture_depth_2d, texture_storage_3d,
    uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, CachedComputePipelineId, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, ComputePassDescriptor, ComputePipelineDescriptor, Extent3d,
    FilterMode, FragmentState, LoadOp, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, ShaderType, StorageTextureAccess, StoreOp, Texture, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor, UniformBuffer,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::view::{
    ExtractedView, ViewDepthStencilTexture, ViewTarget, ViewUniform, ViewUniformOffset,
    ViewUniforms,
};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use serde::{Deserialize, Serialize};

use crate::camera::MainCamera;

/// Lamps and fires that light the haze (the nearest).
pub const MAX_LIGHTS: usize = 24;
/// The froxels' format: rgb light, a extinction (inject) or transmittance (integrate).
const FROXEL_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// Side of the tiling noise volume (texels).
const NOISE_RES: usize = 32;
/// Lamps' and fires' scattering gain by day (× 3 at night).
const LAMP_GAIN: f32 = 2.0;
/// Dust in the air of a room (1/m, × the look's `air`): what a sunbeam through a window lights.
const INDOOR_DUST: f32 = 0.0018;
/// How much of the last frame each cell keeps (the jitter resolves over ~8 frames).
const HISTORY: f32 = 0.88;

/// Lighting › Volumetric light: the froxel grid (cost grows with its cells, not the window's
/// pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VolumetricQuality {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl VolumetricQuality {
    pub const ALL: [VolumetricQuality; 4] = [
        VolumetricQuality::Off,
        VolumetricQuality::Low,
        VolumetricQuality::Medium,
        VolumetricQuality::High,
    ];
    pub fn name(self) -> &'static str {
        match self {
            VolumetricQuality::Off => "Off",
            VolumetricQuality::Low => "Low",
            VolumetricQuality::Medium => "Medium",
            VolumetricQuality::High => "High",
        }
    }
    /// Rows and depth slices of the grid (columns follow the view's aspect).
    fn rows_slices(self) -> (u32, u32) {
        match self {
            VolumetricQuality::Off => (0, 0),
            VolumetricQuality::Low => (64, 48),
            VolumetricQuality::Medium => (90, 64),
            VolumetricQuality::High => (120, 96),
        }
    }
}

/// Whether the volumetric light runs this frame (`sky.rs` and `sun_shafts.rs` hand it their
/// share of the fog and the rays when it does).
#[derive(Resource, Default, Clone, Copy)]
pub struct VolumetricsActive(pub bool);

#[derive(Clone, Copy, Default, Debug, ShaderType)]
struct FogLight {
    /// xyz: position (m), w: range (m)
    position: Vec4,
    /// rgb: luminous intensity (cd)
    color: Vec4,
}

/// Uniforms of the volumetric passes (`volumetrics_types.wesl`). The view matrices are filled in
/// the render world from the extracted view.
#[derive(Clone, Copy, Default, Debug, ShaderType)]
struct VolumetricsGpu {
    world_from_view: Mat4,
    view_from_world: Mat4,
    prev_view_from_world: Mat4,
    /// xyz: camera, w: frame index
    camera: Vec4,
    /// xyz: grid size, w: history weight (0: none this frame)
    grid: Vec4,
    /// x: near, y: far (view depth, m), z: ln(far / near), w: depth jitter this frame
    depth: Vec4,
    /// xy: tan of the half field of view, zw: last frame's
    proj: Vec4,
    /// x: haze extinction at its base (1/m), y: its scale height (m), z: base height (m), w: weather fog extinction (1/m)
    haze: Vec4,
    /// x: weather fog scale height (m), y: ground mist extinction (1/m), z: mist height (m), w: camera near plane (m)
    mist: Vec4,
    /// xyz: noise drift (m), w: noise strength
    wind: Vec4,
    /// x: noise frequency (1/m), y: phase anisotropy, z: sun gain, w: lamp gain
    shape: Vec4,
    /// rgb: scattering albedo, w: sky light gain
    tint: Vec4,
    /// rgb: ambient fill radiance, w: lamp count
    ambient: Vec4,
    /// x: terrain half size, y: cell, z: texels per side, w: 1 with a terrain
    terrain: Vec4,
    /// x: ground haze extinction (1/m), y: its height over the terrain (m), z: sea level (m)
    layers: Vec4,
    lights: [FogLight; MAX_LIGHTS],
}

/// This frame's settings, extracted to the render world.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
struct VolumetricFrame {
    enabled: bool,
    grid: UVec3,
    gpu: VolumetricsGpu,
    noise: Handle<Image>,
    /// the integrated volume (`FroxelFog::image`)
    integrated: Handle<Image>,
    height: Option<Handle<Image>>,
    /// drop the history (a camera cut, a new range)
    reset: bool,
}

/// The integrated volume for transparent things drawn after it (smoke, flames, particles, rain):
/// a material binds `image` (with a linear clamp sampler) and `params`, and fogs its colour at
/// its own depth with `volumetrics_fog::froxel_fog` (`volumetrics_fog.wesl`). `params.w` is 0
/// while the volumetric light is off (no fog then).
#[derive(Resource, Clone, Default)]
pub struct FroxelFog {
    pub image: Handle<Image>,
    /// x: near (m), y: ln(far / near), z: depth slices, w: 1 while it runs
    pub params: Vec4,
    grid: UVec3,
}

/// A 3D image the integrate pass writes (the GPU side only).
fn froxel_image(grid: UVec3) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: grid.x,
            height: grid.y,
            depth_or_array_layers: grid.z,
        },
        TextureDimension::D3,
        FROXEL_FORMAT,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage = TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

/// Marks the camera the passes run on.
#[derive(Component, Clone, Copy, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct VolumetricView;

pub struct VolumetricsPlugin;

impl Plugin for VolumetricsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<VolumetricView>::default(),
            ExtractResourcePlugin::<VolumetricFrame>::default(),
        ))
        .init_resource::<VolumetricFrame>()
        .init_resource::<VolumetricsActive>()
        .init_resource::<FroxelFog>()
        .add_systems(Startup, setup_noise)
        .add_systems(
            PostUpdate,
            update_volumetrics.after(bevy::transform::TransformSystems::Propagate),
        );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<VolumetricState>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Render,
                prepare_volumetrics.in_set(RenderSystems::PrepareBindGroups),
            )
            .add_systems(
                Core3d,
                volumetrics_pass
                    .after(main_transmissive_pass_3d)
                    .after(crate::clouds::CloudTrace)
                    .before(main_transparent_pass_3d),
            );
    }
}

// ------------------------------------------------------------------------------------------------
// Main world

/// A tiling volume of smooth noise (two octaves of value noise) that breaks fog into banks and
/// wisps as it drifts.
fn noise_volume() -> Image {
    let n = NOISE_RES;
    let hash = |x: usize, y: usize, z: usize, period: usize, seed: u32| -> f32 {
        let (x, y, z) = (
            (x % period) as u32,
            (y % period) as u32,
            (z % period) as u32,
        );
        let mut h = x.wrapping_mul(0x8da6_b343)
            ^ y.wrapping_mul(0xd816_3841)
            ^ z.wrapping_mul(0xcb1a_b31f)
            ^ seed.wrapping_mul(0x9e37_79b9);
        h ^= h >> 16;
        h = h.wrapping_mul(0x7feb_352d);
        h ^= h >> 15;
        h = h.wrapping_mul(0x846c_a68b);
        h ^= h >> 16;
        (h & 0xffff) as f32 / 65535.0
    };
    let octave = |x: usize, y: usize, z: usize, cells: usize, seed: u32| -> f32 {
        let step = n as f32 / cells as f32;
        let (fx, fy, fz) = (x as f32 / step, y as f32 / step, z as f32 / step);
        let (ix, iy, iz) = (
            fx.floor() as usize,
            fy.floor() as usize,
            fz.floor() as usize,
        );
        let s = |t: f32| t * t * (3.0 - 2.0 * t);
        let (tx, ty, tz) = (s(fx.fract()), s(fy.fract()), s(fz.fract()));
        let c = |dx: usize, dy: usize, dz: usize| hash(ix + dx, iy + dy, iz + dz, cells, seed);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let x00 = lerp(c(0, 0, 0), c(1, 0, 0), tx);
        let x10 = lerp(c(0, 1, 0), c(1, 1, 0), tx);
        let x01 = lerp(c(0, 0, 1), c(1, 0, 1), tx);
        let x11 = lerp(c(0, 1, 1), c(1, 1, 1), tx);
        lerp(lerp(x00, x10, ty), lerp(x01, x11, ty), tz)
    };
    let mut data = Vec::with_capacity(n * n * n);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let v = octave(x, y, z, 8, 1) * 0.62 + octave(x, y, z, 16, 2) * 0.38;
                // stretch the contrast a little (value noise bunches round the middle)
                data.push((((v - 0.5) * 1.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: n as u32,
        },
        TextureDimension::D3,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

fn setup_noise(
    mut images: ResMut<Assets<Image>>,
    mut frame: ResMut<VolumetricFrame>,
    mut commands: Commands,
    cams: Query<Entity, With<MainCamera>>,
) {
    frame.noise = images.add(noise_volume());
    for e in &cams {
        commands.entity(e).insert(VolumetricView);
    }
}

/// The medium's settings for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Medium {
    /// Boundary-layer haze extinction at the base height (1/m) and its scale height (m).
    pub haze: f32,
    pub haze_height: f32,
    /// Weather / zone fog extinction (1/m) and its scale height (m).
    pub fog: f32,
    pub fog_height: f32,
    /// Street haze over the ground (1/m), where sunbeams show, and its height (m).
    pub ground_haze: f32,
    pub ground_haze_height: f32,
    /// Ground mist over the terrain (1/m) and how high it lies (m).
    pub mist: f32,
    pub mist_height: f32,
    /// Henyey–Greenstein anisotropy of the sunlit air.
    pub anisotropy: f32,
    /// How much the noise breaks it up (0..1).
    pub noise: f32,
    /// Scattering colour (albedo).
    pub albedo: Vec3,
}

/// What fills the air: the world's haze, the look, the weather and the viewer's zone. `indoor`
/// (0..1) thins what lies on the ground (no mist on a floor).
pub fn medium(
    _lighting: &crate::sky::Lighting,
    look: &crate::look::LookState,
    w: &crate::world::WeatherParams,
    indoor: f32,
) -> Medium {
    let hz = look.haze;
    if hz.strength <= 1e-4 {
        return Medium {
            haze: 0.0,
            haze_height: 320.0,
            fog: 0.0,
            fog_height: 70.0,
            ground_haze: 0.0,
            ground_haze_height: 35.0,
            mist: 0.0,
            mist_height: 5.0,
            anisotropy: 0.72,
            noise: 0.0,
            albedo: Vec3::ONE,
        };
    }
    let lk = &look.params;
    let zl = &look.zone;
    // clear air: thin haze, thicker where the look wants sunbeams to show
    let haze = (0.000_05 * hz.strength + hz.effective_amount() * 0.000_45)
        * lk.haze.max(0.0)
        * lk.air.max(0.0)
        * (1.0 + 1.2 * lk.sunbeams.max(0.0))
        + (w.rain * 0.000_5 + w.snow * 0.000_9 + w.dust * 0.004) * hz.strength;
    let fog = (w.fog * 0.007 + zl.fog * 0.005) * lk.haze.max(0.0) * hz.strength;
    let mist =
        ((lk.mist.max(0.0) + 0.35 * w.fog + 0.12 * w.wet) * 0.012 * (1.0 - indoor) * hz.strength)
            .max(0.0);
    // a sandstorm's dust is tan; a zone's fog takes its tint
    let dust = Vec3::new(0.95, 0.78, 0.55);
    let albedo = Vec3::splat(0.92).lerp(dust, w.dust.clamp(0.0, 1.0)) * Vec3::from(zl.fog_tint);
    // the air in the streets and under the trees: dust, pollen, smoke from the chimneys, what a
    // beam of sunlight shows up in (thin in the Natural look, thick in Epic sunbeams)
    let ground_haze = 0.0015
        * lk.air.max(0.0)
        * (0.4 + lk.sunbeams.max(0.0))
        * lk.haze.max(0.0).min(2.0)
        * hz.strength;
    Medium {
        haze,
        ground_haze,
        ground_haze_height: 35.0,
        haze_height: 320.0 - 150.0 * w.dust.clamp(0.0, 1.0),
        fog,
        fog_height: 70.0 + 50.0 * w.fog.clamp(0.0, 1.0),
        mist,
        mist_height: 5.0 + 9.0 * w.fog.clamp(0.0, 1.0),
        anisotropy: (0.72 + 0.1 * lk.sunbeams).clamp(0.2, 0.85),
        noise: (0.35 + 0.4 * w.fog.max(lk.mist).clamp(0.0, 1.0)).min(0.8),
        albedo,
    }
}

/// Whether the volumetric light runs (Off, a dungeon's own fog, under water and deep in a cave
/// turn it off).
pub fn wanted(
    lighting: &crate::sky::Lighting,
    haze: crate::haze::HazeState,
    dungeon: bool,
    underwater: bool,
    cave: f32,
) -> bool {
    haze.strength > 1e-4
        && lighting.volumetrics != VolumetricQuality::Off
        && !dungeon
        && !underwater
        && cave < 0.35
        && std::env::var("WB_PERF").map_or(true, |p| !p.contains("novol"))
}

#[derive(bevy::ecs::system::SystemParam)]
struct Scene<'w> {
    lighting: Res<'w, crate::sky::Lighting>,
    sky: Res<'w, crate::sky::SkyState>,
    look: Res<'w, crate::look::LookState>,
    orbit: Res<'w, crate::camera::OrbitCamera>,
    ambient: Res<'w, GlobalAmbientLight>,
    wind: Option<Res<'w, crate::wind::WindState>>,
    terrain: Option<Res<'w, crate::terrain::Terrain>>,
    terrain_gpu: Option<Res<'w, crate::terrain::material::TerrainGpu>>,
    indoors: Option<Res<'w, crate::motion::indoors::Indoors>>,
    interior: Option<Res<'w, crate::dungeons::render::InteriorLook>>,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_volumetrics(
    mut commands: Commands,
    scene: Scene,
    time: Res<Time>,
    frames: Res<FrameCount>,
    mut frame: ResMut<VolumetricFrame>,
    mut active: ResMut<VolumetricsActive>,
    (mut fog, mut images): (ResMut<FroxelFog>, ResMut<Assets<Image>>),
    mut state: Local<(Vec3, Option<f32>, f32, Option<Vec3>)>,
    cams: Query<
        (
            Entity,
            &Camera,
            &GlobalTransform,
            Has<VolumetricView>,
            Has<crate::motion::underwater::UnderwaterView>,
        ),
        With<MainCamera>,
    >,
    points: Query<(
        &PointLight,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&Name>,
    )>,
) {
    let Ok((cam_e, cam, cam_t, has_view, underwater)) = cams.single() else {
        return;
    };
    if !has_view {
        commands.entity(cam_e).insert(VolumetricView);
    }
    let l = &*scene.lighting;
    let dungeon = scene.interior.as_ref().is_some_and(|i| i.active);
    let indoor = scene.indoors.as_ref().map(|i| i.amount).unwrap_or(0.0);
    let cave = scene.indoors.as_ref().map(|i| i.cave).unwrap_or(0.0);
    let on = wanted(l, scene.look.haze, dungeon, underwater, cave);
    active.0 = on;
    frame.enabled = on;
    fog.params.w = on as u8 as f32;
    if !on {
        state.1 = None;
        return;
    }
    let (drift, ground, last_far, last_eye) = &mut *state;
    let dt = time.delta_secs();
    let eye = cam_t.translation();

    // the grid: rows and slices by quality, columns by the view's aspect
    let size = cam
        .physical_viewport_size()
        .unwrap_or(UVec2::new(16, 9))
        .max(UVec2::ONE);
    let (rows, slices) = l.volumetrics.rows_slices();
    let cols = ((rows as f32 * size.x as f32 / size.y as f32).round() as u32).clamp(16, 320);
    frame.grid = UVec3::new(cols, rows, slices);
    if fog.grid != frame.grid {
        fog.image = images.add(froxel_image(frame.grid));
        fog.grid = frame.grid;
    }
    frame.integrated = fog.image.clone();

    // how far it reaches: a few hundred metres round a walker; past the focus when overviewing
    let walking = !scene.orbit.enabled;
    let far = if walking {
        match l.volumetrics {
            VolumetricQuality::Low => 500.0,
            VolumetricQuality::High => 1200.0,
            _ => 800.0,
        }
    } else {
        (scene.orbit.distance * 3.0).clamp(900.0, 14_000.0)
    };
    let near = 0.6f32;
    fog.params = Vec4::new(near, (far / near).ln(), slices as f32, 1.0);
    // a new range or a jump of the camera: the history no longer lines up
    let cut = last_eye.is_some_and(|p| {
        p.distance(eye)
            > (if walking {
                6.0
            } else {
                scene.orbit.distance * 0.2
            })
            .max(6.0)
    });
    frame.reset = cut || (far / last_far.max(1.0) - 1.0).abs() > 0.03;
    *last_far = far;
    *last_eye = Some(eye);

    // the haze's base follows the ground under the viewer, slowly (walking downhill the haze
    // layer settles with you instead of jumping)
    let g = scene
        .terrain
        .as_ref()
        .map(|t| t.height_at(scene.sky.viewer_pos.x, scene.sky.viewer_pos.z))
        .unwrap_or(0.0);
    let k = 1.0 - (-dt / 6.0).exp();
    let base = ground.map_or(g, |b| b + (g - b) * k);
    *ground = Some(base);

    let w = scene.sky.weather;
    let m = medium(l, &scene.look, &w, indoor);
    // (the ground haze and mist lie on the sea, not on the seabed)
    let sea = scene.terrain.as_ref().map(|t| t.sea_level).unwrap_or(0.0);
    let over_ground = eye.y
        - scene
            .terrain
            .as_ref()
            .map(|t| t.height_at(eye.x, eye.z).max(sea))
            .unwrap_or(0.0);
    let low_fade = 1.0 - crate::noise::smoothstep(150.0, 900.0, over_ground);
    // the noise drifts with the wind (and rises a little: warm air, breath of the ground)
    if let Some(wind) = scene.wind.as_ref() {
        *drift -= Vec3::new(wind.dir.x, 0.0, wind.dir.y) * wind.speed.max(0.3) * dt;
    }
    drift.y -= 0.08 * dt;
    *drift = (*drift + Vec3::splat(1.0e5)).rem_euclid(Vec3::splat(2.0e5)) - Vec3::splat(1.0e5);

    // lamps and fires near the eye
    let reach = far.min(250.0);
    let mut near_lights: Vec<(f32, FogLight)> = points
        .iter()
        // (not the editor's work light: an aid for editing at night, not a lamp in the world)
        .filter(|(p, _, v, name)| {
            v.get()
                && p.intensity > 1.0
                && !name.is_some_and(|n| n.as_str().starts_with("Work light"))
        })
        .filter_map(|(p, t, _, _)| {
            let pos = t.translation();
            let d = pos.distance(eye) - p.range;
            (d < reach).then(|| {
                let c = p.color.to_linear();
                let cd = p.intensity / (4.0 * std::f32::consts::PI);
                (
                    d,
                    FogLight {
                        position: pos.extend(p.range.max(0.5)),
                        color: Vec4::new(c.red * cd, c.green * cd, c.blue * cd, 0.0),
                    },
                )
            })
        })
        .collect();
    near_lights.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut lights = [FogLight::default(); MAX_LIGHTS];
    for (slot, (_, fl)) in lights.iter_mut().zip(near_lights.iter()) {
        *slot = *fl;
    }

    let terrain = match (scene.terrain.as_ref(), scene.terrain_gpu.as_ref()) {
        (Some(t), Some(gpu)) => {
            frame.height = Some(gpu.height.clone());
            Vec4::new(t.half(), t.cell, t.n() as f32, 1.0)
        }
        _ => {
            frame.height = None;
            Vec4::ZERO
        }
    };
    let lk = &scene.look.params;
    let amb = scene.ambient.color.to_linear();
    let fill = Vec3::new(amb.red, amb.green, amb.blue) * scene.ambient.brightness * 0.15;
    let night = scene.sky.night;
    frame.gpu = VolumetricsGpu {
        camera: eye.extend(frames.0 as f32),
        grid: Vec4::new(cols as f32, rows as f32, slices as f32, HISTORY),
        depth: Vec4::new(
            near,
            far,
            (far / near).ln(),
            (frames.0 as f32 * 0.618_034).fract(),
        ),
        haze: Vec4::new(m.haze, m.haze_height, base - 10.0, m.fog),
        mist: Vec4::new(m.fog_height, m.mist * low_fade, m.mist_height, 0.3),
        wind: drift.extend(m.noise),
        // shafts: the look's sunbeams push the sun's share (gloomy weather keeps it off)
        // lamps: physically their halos are a percent of what they light, lost in the dark; a
        // night scene wants them to read (the grade every film gives a street lamp in the mist)
        shape: Vec4::new(
            1.0 / 170.0,
            m.anisotropy,
            (1.0 + 0.6 * lk.sunbeams) * (1.0 - 0.6 * w.dim),
            LAMP_GAIN * (1.0 + 2.0 * night),
        ),
        tint: m.albedo.extend(1.0 - 0.85 * indoor),
        ambient: fill.extend(near_lights.len().min(MAX_LIGHTS) as f32),
        terrain,
        // the layers over the ground are a few metres to tens of metres thick: seen from high
        // above, the grid's cells are far deeper than that and they would come out blotchy
        // indoors: dust in the room's air instead of the street's haze (a beam from a window)
        layers: Vec4::new(
            m.ground_haze * (1.0 - indoor) * low_fade + INDOOR_DUST * indoor * lk.air.max(0.0),
            m.ground_haze_height,
            sea,
            0.0,
        ),
        lights,
        ..default()
    };
}

// ------------------------------------------------------------------------------------------------
// Render world

#[derive(Resource)]
struct VolumetricPipelines {
    atmo_layout: BindGroupLayoutDescriptor,
    inject_layout: BindGroupLayoutDescriptor,
    integrate_layout: BindGroupLayoutDescriptor,
    apply_layout: BindGroupLayoutDescriptor,
    inject: CachedComputePipelineId,
    integrate: CachedComputePipelineId,
    apply: CachedRenderPipelineId,
    cloud_mul: CachedRenderPipelineId,
    cloud_add: CachedRenderPipelineId,
    clamp: Sampler,
    repeat: Sampler,
}

fn init_pipelines(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    cache: Res<PipelineCache>,
    fullscreen: Res<FullscreenShader>,
) {
    let filterable = TextureSampleType::Float { filterable: true };
    // the subset of Bevy's atmosphere bindings (same indices) the inject pass uses, the lights too
    let atmo_layout = BindGroupLayoutDescriptor::new(
        "volumetrics_atmosphere_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::COMPUTE,
            (
                (0, uniform_buffer::<GpuAtmosphere>(true)),
                (1, uniform_buffer::<GpuAtmosphereSettings>(true)),
                (2, uniform_buffer::<AtmosphereTransform>(true)),
                (3, uniform_buffer::<ViewUniform>(true)),
                (4, uniform_buffer::<GpuLights>(true)),
                (8, texture_2d(filterable)),
                (10, texture_2d(filterable)),
                (11, texture_3d(filterable)),
                (12, sampler(SamplerBindingType::Filtering)),
            ),
        ),
    );
    let inject_layout = BindGroupLayoutDescriptor::new(
        "volumetrics_inject_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<VolumetricsGpu>(false),
                texture_2d_array(TextureSampleType::Depth),
                sampler(SamplerBindingType::Comparison),
                texture_3d(filterable),
                sampler(SamplerBindingType::Filtering),
                texture_3d(filterable),
                sampler(SamplerBindingType::Filtering),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_storage_3d(FROXEL_FORMAT, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let integrate_layout = BindGroupLayoutDescriptor::new(
        "volumetrics_integrate_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<VolumetricsGpu>(false),
                texture_3d(TextureSampleType::Float { filterable: false }),
                texture_storage_3d(FROXEL_FORMAT, StorageTextureAccess::WriteOnly),
            ),
        ),
    );
    let apply_layout = BindGroupLayoutDescriptor::new(
        "volumetrics_apply_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                uniform_buffer::<VolumetricsGpu>(false),
                texture_3d(filterable),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
            ),
        ),
    );
    let inject = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("volumetrics_inject".into()),
        layout: vec![atmo_layout.clone(), inject_layout.clone()],
        shader: assets.load("shaders/volumetrics.wesl"),
        entry_point: Some("inject".into()),
        ..default()
    });
    let integrate = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("volumetrics_integrate".into()),
        layout: vec![integrate_layout.clone()],
        shader: assets.load("shaders/volumetrics_integrate.wesl"),
        entry_point: Some("integrate".into()),
        ..default()
    });
    let apply_shader: Handle<Shader> = assets.load("shaders/volumetrics_apply.wesl");
    let target = |entry: &'static str, color: BlendComponent| {
        // the alpha channel is left alone (the view's and the clouds' coverage)
        let keep = BlendComponent {
            src_factor: BlendFactor::Zero,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        };
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(format!("volumetrics_{entry}").into()),
            layout: vec![apply_layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: apply_shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format: TextureFormat::Rgba16Float,
                    blend: Some(BlendState { color, alpha: keep }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        })
    };
    let blend = |src_factor, dst_factor| BlendComponent {
        src_factor,
        dst_factor,
        operation: BlendOperation::Add,
    };
    // dst × T + light
    let apply = target("apply", blend(BlendFactor::One, BlendFactor::SrcAlpha));
    // cloud × T, then + light × the cloud's opacity
    let cloud_mul = target("cloud_mul", blend(BlendFactor::Zero, BlendFactor::Src));
    let cloud_add = target("cloud_add", blend(BlendFactor::DstAlpha, BlendFactor::One));
    let clamp = device.create_sampler(&SamplerDescriptor {
        label: Some("volumetrics_clamp"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    let repeat = device.create_sampler(&SamplerDescriptor {
        label: Some("volumetrics_repeat"),
        address_mode_u: bevy::render::render_resource::AddressMode::Repeat,
        address_mode_v: bevy::render::render_resource::AddressMode::Repeat,
        address_mode_w: bevy::render::render_resource::AddressMode::Repeat,
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(VolumetricPipelines {
        atmo_layout,
        inject_layout,
        integrate_layout,
        apply_layout,
        inject,
        integrate,
        apply,
        cloud_mul,
        cloud_add,
        clamp,
        repeat,
    });
}

struct FroxelTextures {
    grid: UVec3,
    /// the inject pass's output, ping-ponged: one is this frame's, the other the history
    scatter: [(Texture, TextureView); 2],
}

#[derive(Resource, Default)]
struct VolumetricState {
    textures: Option<FroxelTextures>,
    uniform: UniformBuffer<VolumetricsGpu>,
    prev: Option<(Mat4, Vec2)>,
    /// which scatter texture this frame writes
    flip: bool,
    /// the pass ran last frame with these textures
    history: bool,
}

#[derive(Component)]
struct VolumetricBindGroups {
    atmo: BindGroup,
    inject: BindGroup,
    integrate: BindGroup,
    apply: BindGroup,
    grid: UVec3,
}

fn froxel_texture(
    device: &RenderDevice,
    label: &'static str,
    grid: UVec3,
) -> (Texture, TextureView) {
    let tex = device.create_texture(&TextureDescriptor {
        label: Some(label),
        size: Extent3d {
            width: grid.x,
            height: grid.y,
            depth_or_array_layers: grid.z,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D3,
        format: FROXEL_FORMAT,
        usage: TextureUsages::STORAGE_BINDING | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&TextureViewDescriptor::default());
    (tex, view)
}

#[derive(bevy::ecs::system::SystemParam)]
struct Bindings<'w> {
    view_uniforms: Res<'w, ViewUniforms>,
    atmo_uniforms: Res<'w, bevy::render::extract_component::ComponentUniforms<GpuAtmosphere>>,
    settings_uniforms:
        Res<'w, bevy::render::extract_component::ComponentUniforms<GpuAtmosphereSettings>>,
    atmo_transforms: Res<'w, AtmosphereTransforms>,
    atmo_sampler: Res<'w, AtmosphereSampler>,
    light_meta: Res<'w, LightMeta>,
    shadow_samplers: Res<'w, ShadowSamplers>,
    fallback: Res<'w, FallbackImage>,
    images: Res<'w, RenderAssets<GpuImage>>,
}

#[allow(clippy::type_complexity)]
fn prepare_volumetrics(
    mut commands: Commands,
    (device, queue): (Res<RenderDevice>, Res<RenderQueue>),
    pipes: Option<Res<VolumetricPipelines>>,
    cache: Res<PipelineCache>,
    frame: Option<Res<VolumetricFrame>>,
    b: Bindings,
    mut state: ResMut<VolumetricState>,
    views: Query<
        (
            Entity,
            &ExtractedView,
            Option<&AtmosphereTextures>,
            Option<&ViewShadowBindings>,
            &ViewDepthStencilTexture,
        ),
        With<VolumetricView>,
    >,
) {
    let (Some(pipes), Some(frame)) = (pipes, frame) else {
        return;
    };
    let Some((entity, view, atmo_tex, shadows, depth)) = views.iter().next() else {
        state.history = false;
        return;
    };
    let ready = cache.get_compute_pipeline(pipes.inject).is_some()
        && cache.get_compute_pipeline(pipes.integrate).is_some()
        && cache.get_render_pipeline(pipes.apply).is_some();
    let (Some(atmo_tex), Some(shadows)) = (atmo_tex, shadows) else {
        commands.entity(entity).remove::<VolumetricBindGroups>();
        state.history = false;
        return;
    };
    let noise = b.images.get(&frame.noise);
    // (the integrated volume's image is made the same frame as the grid it fits)
    let integrated = b.images.get(&frame.integrated).filter(|g| {
        g.texture.size().depth_or_array_layers == frame.grid.z
            && g.texture.size().width == frame.grid.x
    });
    if !frame.enabled
        || !ready
        || noise.is_none()
        || integrated.is_none()
        || frame.grid.min_element() == 0
    {
        commands.entity(entity).remove::<VolumetricBindGroups>();
        state.history = false;
        return;
    }
    let (
        Some(view_binding),
        Some(atmo_binding),
        Some(settings_binding),
        Some(transforms_binding),
        Some(lights_binding),
    ) = (
        b.view_uniforms.uniforms.binding(),
        b.atmo_uniforms.uniforms().binding(),
        b.settings_uniforms.uniforms().binding(),
        b.atmo_transforms.uniforms().binding(),
        b.light_meta.view_gpu_lights.binding(),
    )
    else {
        return;
    };
    let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
        return;
    };

    let grid = frame.grid;
    if state.textures.as_ref().is_none_or(|t| t.grid != grid) {
        state.textures = Some(FroxelTextures {
            grid,
            scatter: [
                froxel_texture(&device, "volumetrics_scatter_a", grid),
                froxel_texture(&device, "volumetrics_scatter_b", grid),
            ],
        });
        state.history = false;
    }

    // the view as drawn this frame, and last frame's for the history
    let world_from_view = view.world_from_view.to_matrix();
    let view_from_world = world_from_view.inverse();
    let tan = Vec2::new(
        1.0 / view.clip_from_view.x_axis.x,
        1.0 / view.clip_from_view.y_axis.y,
    );
    let mut gpu = frame.gpu;
    gpu.world_from_view = world_from_view;
    gpu.view_from_world = view_from_world;
    let (prev_view, prev_tan) = state.prev.unwrap_or((view_from_world, tan));
    gpu.prev_view_from_world = prev_view;
    gpu.proj = Vec4::new(tan.x, tan.y, prev_tan.x, prev_tan.y);
    gpu.camera = world_from_view.w_axis.truncate().extend(gpu.camera.w);
    // reverse-Z with an infinite far plane: clip z = near / view depth
    gpu.mist.w = view.clip_from_view.w_axis.z.max(1e-3);
    if !state.history || frame.reset {
        gpu.grid.w = 0.0;
    }
    state.prev = Some((view_from_world, tan));
    state.uniform.set(gpu);
    state.uniform.write_buffer(&device, &queue);
    state.flip = !state.flip;
    state.history = true;

    let Some(uniform) = state.uniform.binding() else {
        return;
    };
    let tex = state.textures.as_ref().unwrap();
    let (cur, prev) = if state.flip {
        (&tex.scatter[0], &tex.scatter[1])
    } else {
        (&tex.scatter[1], &tex.scatter[0])
    };
    let height = frame
        .height
        .as_ref()
        .and_then(|h| b.images.get(h))
        .map(|g| &g.texture_view)
        .unwrap_or(&b.fallback.d2.texture_view);
    let atmo = device.create_bind_group(
        "volumetrics_atmosphere",
        &cache.get_bind_group_layout(&pipes.atmo_layout),
        &BindGroupEntries::with_indices((
            (0, atmo_binding),
            (1, settings_binding),
            (2, transforms_binding),
            (3, view_binding),
            (4, lights_binding),
            (8, &atmo_tex.transmittance_lut.default_view),
            (10, &atmo_tex.sky_view_lut.default_view),
            (11, &atmo_tex.aerial_view_lut.default_view),
            (12, &**b.atmo_sampler),
        )),
    );
    let inject = device.create_bind_group(
        "volumetrics_inject",
        &cache.get_bind_group_layout(&pipes.inject_layout),
        &BindGroupEntries::sequential((
            uniform.clone(),
            &shadows.directional_light_depth_texture_view,
            &b.shadow_samplers.directional_light_comparison_sampler,
            &prev.1,
            &pipes.clamp,
            &noise.unwrap().texture_view,
            &pipes.repeat,
            height,
            &cur.1,
        )),
    );
    let integrate = device.create_bind_group(
        "volumetrics_integrate",
        &cache.get_bind_group_layout(&pipes.integrate_layout),
        &BindGroupEntries::sequential((uniform.clone(), &cur.1, &integrated.unwrap().texture_view)),
    );
    let apply = device.create_bind_group(
        "volumetrics_apply",
        &cache.get_bind_group_layout(&pipes.apply_layout),
        &BindGroupEntries::sequential((
            uniform,
            &integrated.unwrap().texture_view,
            &pipes.clamp,
            depth_view,
        )),
    );
    commands.entity(entity).insert(VolumetricBindGroups {
        atmo,
        inject,
        integrate,
        apply,
        grid,
    });
}

#[allow(clippy::type_complexity)]
fn volumetrics_pass(
    view: ViewQuery<(
        &VolumetricBindGroups,
        &ViewTarget,
        &DynamicUniformIndex<GpuAtmosphere>,
        &DynamicUniformIndex<GpuAtmosphereSettings>,
        &AtmosphereTransformsOffset,
        &ViewUniformOffset,
        &ViewLightsUniformOffset,
        Option<&crate::clouds::CloudView>,
    )>,
    pipes: Res<VolumetricPipelines>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    mut ctx: RenderContext,
) {
    let (
        groups,
        target,
        atmo_index,
        settings_index,
        transforms_offset,
        view_offset,
        lights_offset,
        clouds,
    ) = view.into_inner();
    let (Some(inject), Some(integrate), Some(apply)) = (
        cache.get_compute_pipeline(pipes.inject),
        cache.get_compute_pipeline(pipes.integrate),
        cache.get_render_pipeline(pipes.apply),
    ) else {
        return;
    };
    let grid = groups.grid;
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let encoder = ctx.command_encoder();
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("volumetrics_light"),
            timestamp_writes: None,
        });
        let span = diagnostics.pass_span(&mut pass, "volumetrics_light");
        pass.set_pipeline(inject);
        pass.set_bind_group(
            0,
            &groups.atmo,
            &[
                atmo_index.index(),
                settings_index.index(),
                transforms_offset.index(),
                view_offset.offset,
                lights_offset.offset,
            ],
        );
        pass.set_bind_group(1, &groups.inject, &[]);
        pass.dispatch_workgroups(grid.x.div_ceil(4), grid.y.div_ceil(4), grid.z.div_ceil(4));
        pass.set_pipeline(integrate);
        pass.set_bind_group(0, &groups.integrate, &[]);
        pass.dispatch_workgroups(grid.x.div_ceil(8), grid.y.div_ceil(8), 1);
        span.end(&mut pass);
    }
    let fullscreen = |encoder: &mut bevy::render::render_resource::CommandEncoder,
                      view: &TextureView,
                      pipeline: &bevy::render::render_resource::RenderPipeline,
                      label: &'static str| {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &groups.apply, &[]);
        pass.draw(0..3, 0..1);
    };
    fullscreen(
        encoder,
        target.main_texture_view(),
        apply,
        "volumetrics_apply",
    );
    // the clouds were traced without the fog: fog them before they are laid over the sky
    if let Some(cv) = clouds
        && let Some(img) = images.get(&cv.target)
        && let (Some(mul), Some(add)) = (
            cache.get_render_pipeline(pipes.cloud_mul),
            cache.get_render_pipeline(pipes.cloud_add),
        )
    {
        fullscreen(encoder, &img.texture_view, mul, "volumetrics_cloud_mul");
        fullscreen(encoder, &img.texture_view, add, "volumetrics_cloud_add");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worlds_saved_before_it_load_with_the_volumetric_light_on() {
        let l: crate::sky::Lighting = serde_json::from_str("{\"hour\": 9.0}").unwrap();
        assert_eq!(l.volumetrics, VolumetricQuality::Medium);
        assert!(l.haze_enabled);
        let p: crate::look::LookParams = serde_json::from_str("{\"sunbeams\": 0.7}").unwrap();
        assert_eq!((p.air, p.mist), (1.0, 0.0));
    }

    #[test]
    fn weather_fog_and_mist_thicken_the_air_and_a_room_has_no_mist() {
        let l = crate::sky::Lighting::default();
        let look = crate::look::LookState {
            haze: crate::haze::resolve(true, l.haze, None),
            ..default()
        };
        let clear = medium(&l, &look, &crate::world::Weather::Clear.params(), 0.0);
        let fog = medium(&l, &look, &crate::world::Weather::Fog.params(), 0.0);
        assert!(clear.haze > 0.0 && clear.fog == 0.0);
        assert!(fog.fog > 0.003 && fog.mist > clear.mist && fog.mist_height > clear.mist_height);
        let inside = medium(&l, &look, &crate::world::Weather::Fog.params(), 1.0);
        assert_eq!(inside.mist, 0.0);
    }

    #[test]
    fn it_stays_off_where_it_does_not_belong() {
        let mut l = crate::sky::Lighting::default();
        let haze = crate::haze::resolve(true, l.haze, None);
        assert!(wanted(&l, haze, false, false, 0.0));
        assert!(
            !wanted(&l, haze, true, false, 0.0),
            "dungeons have their own fog"
        );
        assert!(!wanted(&l, haze, false, true, 0.0), "under water");
        assert!(!wanted(&l, haze, false, false, 0.8), "deep in a cave");
        l.volumetrics = VolumetricQuality::Off;
        assert!(!wanted(&l, haze, false, false, 0.0));
    }

    #[test]
    fn disabling_haze_removes_outdoor_medium_without_losing_its_values() {
        let mut l = crate::sky::Lighting::default();
        l.haze = 0.7;
        let look = crate::look::LookState {
            haze: crate::haze::resolve(true, l.haze, None),
            ..default()
        };
        let weather = crate::world::Weather::Fog.params();
        let on = medium(&l, &look, &weather, 0.0);
        let mut look = look;
        look.haze = crate::haze::resolve(false, l.haze, None);
        let off = medium(&l, &look, &weather, 0.0);
        assert!(on.haze > 0.0 && on.fog > 0.0 && on.mist > 0.0 && on.ground_haze > 0.0);
        assert_eq!(
            (off.haze, off.fog, off.mist, off.ground_haze),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert!(!wanted(&l, look.haze, false, false, 0.0));
        look.haze = crate::haze::resolve(true, l.haze, None);
        assert_eq!(l.haze, 0.7);
        assert_eq!(medium(&l, &look, &weather, 0.0), on);
    }
}
