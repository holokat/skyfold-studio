//! The camera's light meter: one compute workgroup a frame reads a 64 × 64 grid of the HDR view
//! (each point the mean of a 2 × 2 block of pixels) into a centre-weighted histogram of scene
//! luminance and writes its trimmed mean (the darkest and brightest tenth left out, so the sun's
//! disc and a black doorway don't swing it), plus the distance to what is at the centre of the
//! view (the median of a 5 × 5 patch of the depth buffer). About 16 000 texel reads: nothing next
//! to Bevy's full-resolution histogram (`AutoExposure`, one read per pixel of a 13 MP view).
//!
//! The frame's exposure is taken out on the GPU (a pixel's log2 brightness + its ev100 + log2 1.2
//! is the log2 of its scene luminance), so the reading doesn't depend on the exposure it is used
//! to set: there is no feedback loop, and the few frames the readback lags behind don't matter.
//!
//! Runs after the main pass, temporal anti-aliasing and the lens flare (looking into the sun's
//! glare reads brighter, as it does to the eye), before bloom. Read back every frame
//! (`Readback`) into `MeterReading`.

use std::num::NonZero;

use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::Core3dSystems;
use bevy::core_pipeline::schedule::Core3d;
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    storage_buffer_sized, texture_2d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedComputePipelineId,
    ComputePassDescriptor, ComputePipelineDescriptor, PipelineCache, ShaderStages, ShaderType,
    TextureSampleType,
};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::storage::{GpuShaderBuffer, ShaderBuffer};
use bevy::render::view::{ViewDepthStencilTexture, ViewTarget};
use bevy::render::{RenderApp, RenderStartup};

use crate::camera::MainCamera;

/// Floats the meter writes: scene log2 luminance, centre distance (median), centre distance
/// (nearest), histogram weight (0: nothing measured yet), four spare.
const OUT_LEN: usize = 8;
/// `ln 1.2 / ln 2`: Bevy's exposure is `1 / (1.2 · 2^ev100)`.
pub const LOG2_1_2: f32 = 0.263_034_4;

/// The meter's uniforms, on the main camera.
#[derive(Component, Clone, Copy, Default, Debug, ShaderType, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct CameraMeter {
    /// x: added to a pixel's log2 brightness gives the log2 of its scene luminance (the frame's
    /// ev100 + log2 1.2), y: near plane (m), z: view aspect (width / height), w: unused
    pub params: Vec4,
}

/// The latest reading (a few frames old).
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct MeterReading {
    /// log2 of the view's average scene luminance (cd/m²; the brightest and darkest tenth left
    /// out, the centre of the view counting about four times as much as the corners)
    pub scene_log_lum: Option<f32>,
    /// Distance along the view (m) to what is at the centre: the median of a 5 × 5 patch round
    /// it (open sky reads as 100 km).
    pub centre_distance: Option<f32>,
    /// The nearest of those points (m).
    pub centre_nearest: Option<f32>,
    /// Readings so far.
    pub count: u64,
}

#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
struct MeterBuffer(Handle<ShaderBuffer>);

pub struct MeterPlugin;

impl Plugin for MeterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MeterReading>()
            .add_plugins((
                ExtractComponentPlugin::<CameraMeter>::default(),
                UniformComponentPlugin::<CameraMeter>::default(),
                ExtractResourcePlugin::<MeterBuffer>::default(),
            ))
            .add_systems(Startup, setup)
            .add_systems(
                PostUpdate,
                update_meter.after(bevy::transform::TransformSystems::Propagate),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Core3d,
                meter_pass
                    .after(super::flare::LensFlarePass)
                    .before(bevy::post_process::bloom::bloom)
                    .in_set(Core3dSystems::PostProcess),
            );
    }
}

/// The GPU buffer the meter writes and the readback that copies it to `MeterReading` each frame.
fn setup(mut commands: Commands, mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let mut buf = ShaderBuffer::with_size((OUT_LEN * 4) as u64, RenderAssetUsages::RENDER_WORLD);
    buf.label = "camera_meter".into();
    let handle = buffers.add(buf);
    commands.insert_resource(MeterBuffer(handle.clone()));
    commands
        .spawn((Name::new("Camera meter readback"), Readback::buffer(handle)))
        .observe(read);
}

fn read(ev: On<ReadbackComplete>, mut out: ResMut<MeterReading>) {
    let v: Vec<f32> = ev
        .data
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    if v.len() < 4 || v[3] <= 0.0 || !v[0].is_finite() {
        return;
    }
    out.scene_log_lum = Some(v[0]);
    out.centre_distance = Some(v[1]).filter(|d| d.is_finite() && *d > 0.0);
    out.centre_nearest = Some(v[2]).filter(|d| d.is_finite() && *d > 0.0);
    out.count += 1;
}

/// The frame's exposure and projection for the meter (after the sky has set the exposure).
fn update_meter(
    mut commands: Commands,
    mut cams: Query<
        (
            Entity,
            &bevy::camera::Exposure,
            &Projection,
            &Camera,
            Option<&mut CameraMeter>,
        ),
        With<MainCamera>,
    >,
) {
    for (e, exp, proj, cam, meter) in &mut cams {
        let near = match proj {
            Projection::Perspective(p) => p.near,
            _ => 0.1,
        };
        let aspect = cam
            .logical_viewport_size()
            .map(|v| v.x / v.y.max(1.0))
            .unwrap_or(1.6);
        let u = CameraMeter {
            params: Vec4::new(exp.ev100 + LOG2_1_2, near, aspect, 0.0),
        };
        match meter {
            Some(mut m) => *m = u,
            None => {
                commands.entity(e).insert(u);
            }
        }
    }
}

#[derive(Resource)]
struct MeterPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
}

fn init_pipeline(mut commands: Commands, cache: Res<PipelineCache>, assets: Res<AssetServer>) {
    let layout = BindGroupLayoutDescriptor::new(
        "camera_meter_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_depth_2d(),
                uniform_buffer::<CameraMeter>(true),
                storage_buffer_sized(false, NonZero::<u64>::new((OUT_LEN * 4) as u64)),
            ),
        ),
    );
    let pipeline = cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("camera_meter".into()),
        layout: vec![layout.clone()],
        shader: assets.load("shaders/camera_meter.wesl"),
        entry_point: Some("meter".into()),
        ..default()
    });
    commands.insert_resource(MeterPipeline { layout, pipeline });
}

fn meter_pass(
    view: ViewQuery<
        (
            &ViewTarget,
            &ViewDepthStencilTexture,
            &DynamicUniformIndex<CameraMeter>,
        ),
        With<CameraMeter>,
    >,
    pipe: Res<MeterPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<CameraMeter>>,
    buffer: Option<Res<MeterBuffer>>,
    buffers: Res<RenderAssets<GpuShaderBuffer>>,
    mut ctx: RenderContext,
) {
    let (target, depth, index) = view.into_inner();
    let Some(pipeline) = cache.get_compute_pipeline(pipe.pipeline) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
        return;
    };
    let Some(out) = buffer.and_then(|b| buffers.get(&b.0)) else {
        return;
    };
    let group = ctx.render_device().create_bind_group(
        "camera_meter_bind_group",
        &cache.get_bind_group_layout(&pipe.layout),
        &BindGroupEntries::sequential((
            target.main_texture_view(),
            depth_view,
            binding,
            out.buffer.as_entire_buffer_binding(),
        )),
    );
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "camera_meter");
    let mut pass = ctx
        .command_encoder()
        .begin_compute_pass(&ComputePassDescriptor {
            label: Some("camera_meter"),
            timestamp_writes: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &group, &[index.index()]);
    pass.dispatch_workgroups(1, 1, 1);
    drop(pass);
    span.end(ctx.command_encoder());
}
