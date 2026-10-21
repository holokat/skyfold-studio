//! The slow noise fields of the terrain's ground rules, baked. Which grounds the style puts where
//! (slope rock, beaches, the snow line, dry patches) is worked out per pixel, and most of that
//! work was noise: about ten value noises per pixel per frame (the macro and mid noise and the
//! three octaves of dry patches the rules shift their bands by, the large-scale tone, the
//! anti-tiling noise, the coarsest ecotone octave). They only depend on where a spot is, so they
//! are baked once per world, on the GPU from the same WGSL (`terrain_bake.wesl`), into two RGBA8
//! maps with a texel per terrain vertex, and the shading (`RULE_NOISE_MAPS`) reads them (bilinear,
//! always the top level: at a distance the per-pixel noise wasn't filtered either).
//!
//! The rules themselves stay per pixel (`terrain_rules.wesl`): the slope, the height, the paint
//! and the shore are exact there, so brush strokes, the style sliders and the weather's snow line
//! show at once, and borders don't move. (Baking the final per-layer weights instead was tried:
//! bilinear weights between 4 m texels moved the sharp slope-rock and dirt borders by up to a
//! texel.) The noises the maps can't hold (the paint's ±1.5 m warp, the 12 m and 3 m ecotone
//! octaves) stay per pixel too.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{texture_storage_2d, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    CommandEncoderDescriptor, ComputePassDescriptor, ComputePipelineDescriptor, Extent3d,
    PipelineCache, ShaderStages, ShaderType, StorageTextureAccess, TextureDimension, TextureFormat,
    TextureUsages, UniformBuffer,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

use super::Terrain;
use super::material::TerrainGpu;

/// The noise maps' format (see `terrain_bake.wesl` for the channels).
const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// Creates the (unbaked) noise maps of an `n`-vertex terrain: a texel per vertex, no mips.
pub fn rule_maps(n: usize, images: &mut Assets<Image>) -> [Handle<Image>; 2] {
    std::array::from_fn(|_| {
        let mut img = Image::new_uninit(
            Extent3d {
                width: n as u32,
                height: n as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            FORMAT,
            RenderAssetUsages::RENDER_WORLD,
        );
        img.texture_descriptor.usage =
            TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING;
        images.add(img)
    })
}

/// Where the noise fields lie (`BakeParams` in `terrain_bake.wesl`).
#[derive(Clone, Copy, Default, PartialEq, Debug, ShaderType)]
struct BakeParams {
    grid: Vec4,
    rect: Vec4,
}

/// The bake to run this frame (main world): the maps of a new world.
#[derive(Resource, Default)]
pub struct RuleBake {
    /// Terrain generation and layout of the last request.
    last: Option<(u64, Vec4)>,
    request: Option<BakeRequest>,
}

#[derive(Clone)]
struct BakeRequest {
    generation: u64,
    maps: [AssetId<Image>; 2],
    params: BakeParams,
}

pub struct TerrainBakePlugin;

impl Plugin for TerrainBakePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RuleBake>()
            .add_systems(First, |mut b: ResMut<RuleBake>| b.request = None)
            .add_systems(PostUpdate, request_bake.after(super::TerrainSystems::Gpu));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<RenderBake>()
                .add_systems(RenderStartup, init_pipeline)
                .add_systems(ExtractSchedule, extract_request)
                .add_systems(Render, run_bake.in_set(RenderSystems::PrepareBindGroups));
        }
    }
}

/// Asks for the maps of a new world (or of a resized one) to be baked.
fn request_bake(mut bake: ResMut<RuleBake>, terrain: Res<Terrain>, gpu: Option<Res<TerrainGpu>>) {
    let Some(gpu) = gpu else { return };
    let Some(generation) = gpu.generation else {
        return;
    };
    let n = terrain.n() as f32;
    let grid = Vec4::new(terrain.half(), terrain.cell, n, 0.0);
    if bake.last == Some((generation, grid)) {
        return;
    }
    bake.last = Some((generation, grid));
    bake.request = Some(BakeRequest {
        generation,
        maps: std::array::from_fn(|k| gpu.rules[k].id()),
        params: BakeParams {
            grid,
            rect: Vec4::new(0.0, 0.0, n - 1.0, n - 1.0),
        },
    });
}

/// The bake waiting for its maps and the pipeline (render world).
#[derive(Resource, Default)]
struct RenderBake {
    pending: Option<BakeRequest>,
    uniform: UniformBuffer<BakeParams>,
    /// The last bake, and whether the pipeline was ready: an edited bake shader (hot reload)
    /// recompiles the pipeline, and once it's back the maps are baked again with it.
    last: Option<BakeRequest>,
    ready: bool,
}

fn extract_request(mut dst: ResMut<RenderBake>, src: Extract<Res<RuleBake>>) {
    // (a new world's request replaces one still waiting: those maps are gone)
    if let Some(r) = &src.request {
        dst.pending = Some(r.clone());
    }
}

#[derive(Resource)]
struct BakePipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

fn init_pipeline(mut commands: Commands, assets: Res<AssetServer>, cache: Res<PipelineCache>) {
    let storage = || texture_storage_2d(FORMAT, StorageTextureAccess::WriteOnly);
    let layout = BindGroupLayoutDescriptor::new(
        "terrain_rule_bake",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (uniform_buffer::<BakeParams>(false), storage(), storage()),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("terrain_rule_bake".into()),
        layout: vec![layout.clone()],
        shader: assets.load("shaders/terrain_bake.wesl"),
        entry_point: Some("bake".into()),
        ..default()
    });
    commands.insert_resource(BakePipeline { layout, pipeline });
}

/// Runs the waiting bake (one compute dispatch) before the frame draws.
fn run_bake(
    mut bake: ResMut<RenderBake>,
    pipe: Option<Res<BakePipeline>>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    let bake = &mut *bake;
    let Some(pipe) = pipe else { return };
    let pipeline = cache.get_compute_pipeline(pipe.pipeline);
    if pipeline.is_some() && !bake.ready && bake.pending.is_none() {
        bake.pending = bake.last.clone();
    }
    bake.ready = pipeline.is_some();
    let (Some(pipeline), Some(req)) = (pipeline, bake.pending.as_ref()) else {
        return;
    };
    let (Some(m0), Some(m1)) = (images.get(req.maps[0]), images.get(req.maps[1])) else {
        return;
    };
    let req = bake.pending.take().unwrap();
    bake.uniform.set(req.params);
    bake.uniform.write_buffer(&device, &queue);
    let Some(uniform) = bake.uniform.binding() else {
        return;
    };
    let group = device.create_bind_group(
        "terrain_rule_bake",
        &cache.get_bind_group_layout(&pipe.layout),
        &BindGroupEntries::sequential((uniform, &m0.texture_view, &m1.texture_view)),
    );
    let (w, h) = (req.params.rect.z as u32 + 1, req.params.rect.w as u32 + 1);
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("terrain_rule_bake"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("terrain_rule_bake"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
    }
    queue.submit([encoder.finish()]);
    debug!(
        "terrain rule noise baked: {w}×{h} (generation {})",
        req.generation
    );
    bake.last = Some(req);
}
