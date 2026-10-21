//! Sun shafts (crepuscular rays): light scattered in the air, streaming from the sun past whatever
//! blocks it (trees, buildings, a ridge, the dark side of a cloud). A screen-space radial march
//! towards the sun's place on screen (Mitchell, GPU Gems 3 ch. 13): the bright sky round the sun
//! is the light, anything in the depth buffer in front of it casts a gap.
//!
//! Runs on the HDR image after the main pass (clouds and water drawn) and before temporal
//! anti-aliasing, bloom and tonemapping, so the rays glow like any other bright light and TAA
//! smooths the dithered march:
//! 1. a half-resolution march writes the rays into their own texture;
//! 2. a full-resolution pass adds them to the view (additive blend).
//!
//! Strength comes from the look (`LookParams::sunbeams`); it fades out when the sun is behind the
//! camera or below the horizon, at night and under water, and the passes are skipped at zero.

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    Extent3d, FilterMode, FragmentState, LoadOp, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, StoreOp, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ViewDepthStencilTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::window::PrimaryWindow;

use crate::camera::MainCamera;

/// The rays' own texture (half the view's resolution per axis).
const SHAFT_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// Brightness the look's `sunbeams` = 1 adds (tuned against the Natural look's exposure).
const STRENGTH: f32 = 0.55;
/// Their share while the volumetric light (`volumetrics`) draws the shafts.
const VOLUMETRIC_SHARE: f32 = 0.35;

/// Uniforms of the sun-shaft passes: on the main camera while the rays show.
#[derive(Component, Clone, Copy, Default, Debug, ShaderType, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct SunShafts {
    /// xy: the sun on screen (uv, may lie outside 0..1), z: strength, w: dither frame (0: still)
    pub sun: Vec4,
    /// x: share of the way to the sun marched, y: sky brightness that starts to shine,
    /// z: glow radius round the sun (uv), w: decay per step
    pub params: Vec4,
    /// rgb: tint, w: view aspect (width / height)
    pub tint: Vec4,
}

pub struct SunShaftsPlugin;

impl Plugin for SunShaftsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<SunShafts>::default(),
            UniformComponentPlugin::<SunShafts>::default(),
        ))
        .add_systems(
            PostUpdate,
            update_shafts.after(bevy::transform::TransformSystems::Propagate),
        );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                (prepare_pipelines, prepare_textures).in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Core3d,
                sun_shafts_pass
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::EarlyPostProcess),
            );
    }
}

/// Where the sun is on screen and how strong its rays are this frame.
#[allow(clippy::type_complexity)]
fn update_shafts(
    mut commands: Commands,
    look: Res<crate::look::LookState>,
    sky: Res<crate::sky::SkyState>,
    lighting: Res<crate::sky::Lighting>,
    volumetrics: Res<crate::volumetrics::VolumetricsActive>,
    frame: Res<FrameCount>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<
        (
            Entity,
            &GlobalTransform,
            &Projection,
            &Camera,
            Option<&mut SunShafts>,
            Has<crate::motion::underwater::UnderwaterView>,
        ),
        With<MainCamera>,
    >,
) {
    for (e, gt, proj, cam, shafts, underwater) in &mut cams {
        let Projection::Perspective(pp) = proj else {
            continue;
        };
        let aspect = cam
            .logical_viewport_size()
            .map(|v| v.x / v.y.max(1.0))
            .or(window
                .single()
                .ok()
                .map(|w| w.width() / w.height().max(1.0)))
            .unwrap_or(1.6);
        let sun = sky.sun_dir.normalize_or_zero();
        // the sun in view space (the camera looks down -z)
        let v = gt.affine().inverse().transform_vector3(sun);
        let facing = crate::noise::smoothstep(0.0, 0.35, -v.z);
        let up = crate::noise::smoothstep(-0.03, 0.04, sun.y);
        // the volumetric light draws the real shafts (from its shadow maps, wherever the sun is);
        // these keep a little of the radiant glare round a sun peeking past the skyline
        let share = if volumetrics.0 { VOLUMETRIC_SHARE } else { 1.0 };
        let strength = look.params.sunbeams
            * STRENGTH
            * share
            * facing
            * up
            * (1.0 - sky.night)
            * look.haze.strength
            * if !underwater { 1.0 } else { 0.0 };
        if strength < 1e-3 {
            if shafts.is_some() {
                commands.entity(e).remove::<SunShafts>();
            }
            continue;
        }
        let th = (pp.fov * 0.5).tan();
        let ndc = Vec2::new(v.x / (-v.z * th * aspect), v.y / (-v.z * th));
        let uv = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
            .clamp(Vec2::splat(-1.5), Vec2::splat(2.5));
        // a still dither pattern unless temporal anti-aliasing averages it out over frames
        let dither = if lighting.anti_aliasing == crate::camera::AntiAliasing::Taa {
            (frame.0 % 64) as f32 + 1.0
        } else {
            0.0
        };
        let u = SunShafts {
            sun: Vec4::new(uv.x, uv.y, strength, dither),
            params: Vec4::new(0.85, 1.2, 0.32, 0.965),
            tint: Vec4::new(1.0, 0.97, 0.9, aspect),
        };
        match shafts {
            Some(mut cur) => *cur = u,
            None => {
                commands.entity(e).insert(u);
            }
        }
    }
}

#[derive(Resource)]
struct ShaftsPipeline {
    march_layout: BindGroupLayoutDescriptor,
    composite_layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    march: Option<CachedRenderPipelineId>,
    /// Additive composite, per view target format.
    composite: Vec<(TextureFormat, CachedRenderPipelineId)>,
    shader: Handle<Shader>,
    vertex: VertexState,
}

#[derive(Component)]
struct ShaftsViewPipeline(CachedRenderPipelineId);

#[derive(Component)]
struct ShaftsTexture(CachedTexture);

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let march_layout = BindGroupLayoutDescriptor::new(
        "sun_shafts_march_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
                uniform_buffer::<SunShafts>(true),
            ),
        ),
    );
    let composite_layout = BindGroupLayoutDescriptor::new(
        "sun_shafts_composite_layout",
        // the march's slots without the depth (one shader module)
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (0, texture_2d(TextureSampleType::Float { filterable: true })),
                (1, sampler(SamplerBindingType::Filtering)),
                (3, uniform_buffer::<SunShafts>(true)),
            ),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(ShaftsPipeline {
        march_layout,
        composite_layout,
        sampler,
        march: None,
        composite: Vec::new(),
        shader: assets.load("shaders/sun_shafts.wesl"),
        vertex: fullscreen.to_vertex_state(),
    });
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipe: ResMut<ShaftsPipeline>,
    views: Query<(Entity, &ViewTarget), With<SunShafts>>,
) {
    if pipe.march.is_none() && !views.is_empty() {
        pipe.march = Some(cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("sun_shafts_march".into()),
            layout: vec![pipe.march_layout.clone()],
            vertex: pipe.vertex.clone(),
            fragment: Some(FragmentState {
                shader: pipe.shader.clone(),
                entry_point: Some("march".into()),
                targets: vec![Some(ColorTargetState {
                    format: SHAFT_FORMAT,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }));
    }
    for (e, target) in &views {
        let format = target.main_texture_format();
        let id = match pipe.composite.iter().find(|(f, _)| *f == format) {
            Some((_, id)) => *id,
            None => {
                // adds the rays' light, leaves alpha alone
                let add = BlendComponent {
                    src_factor: BlendFactor::One,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                };
                let keep = BlendComponent {
                    src_factor: BlendFactor::Zero,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                };
                let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("sun_shafts_composite".into()),
                    layout: vec![pipe.composite_layout.clone()],
                    vertex: pipe.vertex.clone(),
                    fragment: Some(FragmentState {
                        shader: pipe.shader.clone(),
                        entry_point: Some("composite".into()),
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: Some(BlendState {
                                color: add,
                                alpha: keep,
                            }),
                            write_mask: ColorWrites::ALL,
                        })],
                        ..default()
                    }),
                    ..default()
                });
                pipe.composite.push((format, id));
                id
            }
        };
        commands.entity(e).insert(ShaftsViewPipeline(id));
    }
}

fn prepare_textures(
    mut commands: Commands,
    mut textures: ResMut<TextureCache>,
    device: Res<RenderDevice>,
    views: Query<(Entity, &ExtractedCamera), With<SunShafts>>,
) {
    for (e, cam) in &views {
        let Some(size) = cam.physical_viewport_size else {
            continue;
        };
        let half = (size / 2).max(UVec2::ONE);
        let tex = textures.get(
            &device,
            TextureDescriptor {
                label: Some("sun_shafts_texture"),
                size: Extent3d {
                    width: half.x,
                    height: half.y,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: SHAFT_FORMAT,
                usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        commands.entity(e).insert(ShaftsTexture(tex));
    }
}

fn sun_shafts_pass(
    // With<SunShafts>: the index, texture and pipeline id outlive a removed SunShafts
    view: ViewQuery<
        (
            &ViewTarget,
            &ViewDepthStencilTexture,
            &DynamicUniformIndex<SunShafts>,
            &ShaftsViewPipeline,
            &ShaftsTexture,
        ),
        With<SunShafts>,
    >,
    pipe: Res<ShaftsPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<SunShafts>>,
    mut ctx: RenderContext,
) {
    let (target, depth, index, pid, tex) = view.into_inner();
    let (Some(march), Some(composite)) = (
        pipe.march.and_then(|id| cache.get_render_pipeline(id)),
        cache.get_render_pipeline(pid.0),
    ) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
        return;
    };
    let device = ctx.render_device().clone();

    let march_group = device.create_bind_group(
        "sun_shafts_march_bind_group",
        &cache.get_bind_group_layout(&pipe.march_layout),
        &BindGroupEntries::sequential((
            target.main_texture_view(),
            &pipe.sampler,
            depth_view,
            binding.clone(),
        )),
    );
    {
        let mut pass = ctx
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("sun_shafts_march"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &tex.0.default_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations::default(),
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_pipeline(march);
        pass.set_bind_group(0, &march_group, &[index.index()]);
        pass.draw(0..3, 0..1);
    }

    let composite_group = device.create_bind_group(
        "sun_shafts_composite_bind_group",
        &cache.get_bind_group_layout(&pipe.composite_layout),
        &BindGroupEntries::with_indices((
            (0, &tex.0.default_view),
            (1, &pipe.sampler),
            (3, binding.clone()),
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("sun_shafts_composite"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target.main_texture_view(),
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
    pass.set_pipeline(composite);
    pass.set_bind_group(0, &composite_group, &[index.index()]);
    pass.draw(0..3, 0..1);
}
