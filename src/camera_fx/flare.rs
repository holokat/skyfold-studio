//! Sun glare and lens flare: what a bright sun does to the eye and the lens when the camera looks
//! towards it. Light scattered in the eye (or the glass) spreads round the sun as a soft veiling
//! glare, wide and faint enough to lift the shadows of the whole frame a little; the aperture's
//! blades add a faint starburst; reflections between the lens elements throw ghosts (images of the
//! hexagonal aperture, a ring or two) along the line from the sun through the centre of the view.
//!
//! Two passes on the HDR image after temporal anti-aliasing (the ghosts move with the view, not
//! the world, so they mustn't be reprojected) and before bloom and tonemapping, so the glare blooms
//! and is tone mapped like any other light:
//! 1. `probe` (one pixel): how much of the sun is in view (open sky in the depth buffer round its
//!    place on screen: leaves, a roof edge, a ridge hide it) and how much it outshines the sky a
//!    few degrees from it (a clear sun by hundreds of times, a sun behind cloud hardly at all),
//!    with the colour it is drawn in (white at noon, orange low down);
//! 2. `flare`: the glare, starburst and ghosts, scaled by the probe, added over the view.
//!
//! Strength comes from the look (`LookParams::glare`, `LookParams::flare`); it fades as the sun
//! leaves the frame, near and below the horizon, at night, under water and in a roofed dungeon,
//! and the passes are skipped at zero.

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::render_resource::binding_types::{texture_2d, texture_depth_2d, uniform_buffer};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    Extent3d, FragmentState, LoadOp, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, ShaderStages, ShaderType, StoreOp,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ViewDepthStencilTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use crate::camera::MainCamera;

/// The probe's three pixels, kept from frame to frame and blended with the last (a flicker
/// through leaves is smoothed): (0, 0) rgb the sun's colour as drawn; (1, 0) r how much of the sun
/// is in view, g the glare's scale (the sky's brightness a few degrees from the sun, as exposed:
/// glare and ghosts are the sun's light, so they follow the exposure like the sky round it);
/// (2, 0) r how clear the sun is (it outshines the sky round it). Out of frame the sun's colour and
/// clearness can't be measured (the taps clamp to the edge): they ease towards `LensFlare::prior`,
/// while what is in view and the scale are read from the edge nearest the sun.
const PROBE_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
/// The look's `glare` / `flare` = 1 (× the sky's brightness by the sun).
const GLARE: f32 = 1.0;
const FLARE: f32 = 1.0;
/// The sun's angular radius (rad; Bevy's `SunDisk::EARTH`).
const SUN_RADIUS: f32 = 0.004_65;

/// Uniforms of the flare passes: on the main camera while the sun can flare.
#[derive(Component, Clone, Copy, Default, Debug, ShaderType, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct LensFlare {
    /// xy: the sun on screen (uv, may lie off screen), z: glare strength, w: ghost strength
    pub sun: Vec4,
    /// rgb: the look's sun colour, w: view aspect (width / height)
    pub tint: Vec4,
    /// x: starburst turn (rad), y: angular scale (tan of half the field of view over tan 25°: the
    /// glare's reach shrinks on screen as the view widens), z: probe radius (screen heights), w: how
    /// far the probe's disc is in frame (1: the sun's colour and clearness are measured, 0: `prior`)
    pub params: Vec4,
    /// Out of frame: rgb the sun's colour (warmer low down), w how clear it likely is (the cloud
    /// cover and the weather)
    pub prior: Vec4,
}

/// The flare passes in the render schedule (the meter runs after them).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LensFlarePass;

pub struct LensFlarePlugin;

impl Plugin for LensFlarePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<LensFlare>::default(),
            UniformComponentPlugin::<LensFlare>::default(),
        ))
        .add_systems(
            PostUpdate,
            update_flare.after(bevy::transform::TransformSystems::Propagate),
        );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                prepare_pipelines.in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Core3d,
                lens_flare_pass
                    .in_set(LensFlarePass)
                    .before(bevy::post_process::bloom::bloom)
                    .in_set(Core3dSystems::PostProcess),
            );
    }
}

/// Where the sun is on screen and how strongly it flares this frame.
#[allow(clippy::type_complexity)]
fn update_flare(
    mut commands: Commands,
    look: Res<crate::look::LookState>,
    sky: Res<crate::sky::SkyState>,
    interior: Option<Res<crate::dungeons::render::InteriorLook>>,
    mut cams: Query<
        (
            Entity,
            &GlobalTransform,
            &Projection,
            &Camera,
            Option<&mut LensFlare>,
            Has<crate::motion::underwater::UnderwaterView>,
        ),
        With<MainCamera>,
    >,
) {
    let lk = &look.params;
    // a dungeon's roofed map has no sun
    let dark = interior.is_some_and(|i| i.active && !i.sky);
    for (e, gt, proj, cam, flare, underwater) in &mut cams {
        let Projection::Perspective(pp) = proj else {
            continue;
        };
        let aspect = cam
            .logical_viewport_size()
            .map(|v| v.x / v.y.max(1.0))
            .unwrap_or(1.6);
        let sun = sky.sun_dir.normalize_or_zero();
        let v = gt
            .affine()
            .inverse()
            .transform_vector3(sun)
            .normalize_or_zero();
        let th = (pp.fov * 0.5).tan();
        let mut base = crate::noise::smoothstep(0.05, 0.3, -v.z)
            * crate::noise::smoothstep(-0.015, 0.05, sun.y)
            * (1.0 - sky.night);
        if underwater || dark {
            base = 0.0;
        }
        // (the probe's disc: three times the sun's, in screen heights)
        let probe_r = 3.0 * SUN_RADIUS / (2.0 * th);
        let (mut glare, mut ghosts, mut uv, mut inside) = (0.0, 0.0, Vec2::ZERO, 0.0);
        if base > 1e-3 {
            let ndc = Vec2::new(v.x / (-v.z * th * aspect), v.y / (-v.z * th));
            uv = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5)
                .clamp(Vec2::splat(-2.0), Vec2::splat(3.0));
            // how far outside the frame the sun is (screen heights): the glare reaches in from a
            // sun just out of frame, the ghosts go soon after it leaves
            let off =
                ((uv.x - 0.5).abs() - 0.5).max(0.0) * aspect + ((uv.y - 0.5).abs() - 0.5).max(0.0);
            glare = lk.glare * GLARE * base * (1.0 - crate::noise::smoothstep(0.0, 0.45, off));
            ghosts = lk.flare * FLARE * base * (1.0 - crate::noise::smoothstep(0.0, 0.12, off));
            // the probe's disc reaching past the frame's edge reads clamped pixels
            let edge = ((uv.x - 0.5).abs() - 0.5) * aspect;
            let edge = edge.max((uv.y - 0.5).abs() - 0.5) + probe_r;
            inside = 1.0 - crate::noise::smoothstep(-probe_r, 0.0, edge);
        }
        if glare.max(ghosts) < 1e-3 {
            if flare.is_some() {
                commands.entity(e).remove::<LensFlare>();
            }
            continue;
        }
        let sc = lk.sun_color();
        let u = LensFlare {
            sun: Vec4::new(uv.x, uv.y, glare, ghosts),
            tint: Vec4::new(sc.x, sc.y, sc.z, aspect),
            // (the starburst turns a little as the sun crosses the frame, like a real aperture's
            // spikes catching differently)
            params: Vec4::new(
                0.21 + (uv.x - 0.5) * 0.25,
                th / 25f32.to_radians().tan(),
                probe_r,
                inside,
            ),
            prior: prior(&sky),
        };
        match flare {
            Some(mut cur) => *cur = u,
            None => {
                commands.entity(e).insert(u);
            }
        }
    }
}

/// The sun's colour and how clear it likely is, for when it is out of frame: warmer low down; behind
/// cloud with heavy cover, dimmed by a storm's gloom.
fn prior(sky: &crate::sky::SkyState) -> Vec4 {
    let low = 1.0 - crate::noise::smoothstep(0.02, 0.35, sky.sun_dir.y);
    let tint = Vec3::ONE.lerp(Vec3::new(1.0, 0.72, 0.45), low);
    let clear = (1.0 - 0.8 * crate::noise::smoothstep(0.35, 0.9, sky.weather.clouds))
        * (1.0 - sky.weather.dim).clamp(0.0, 1.0);
    tint.extend(clear)
}

#[derive(Resource)]
struct FlarePipeline {
    probe_layout: BindGroupLayoutDescriptor,
    flare_layout: BindGroupLayoutDescriptor,
    probe: Option<CachedRenderPipelineId>,
    /// Additive flare, per view target format.
    flare: Vec<(TextureFormat, CachedRenderPipelineId)>,
    shader: Handle<Shader>,
    vertex: VertexState,
}

#[derive(Component)]
struct FlareViewPipeline(CachedRenderPipelineId);

/// The probe's three pixels (one main camera), kept from frame to frame.
#[derive(Resource)]
struct ProbeTexture(TextureView);

fn init_pipeline(
    mut commands: Commands,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
    device: Res<RenderDevice>,
) {
    let probe = device.create_texture(&TextureDescriptor {
        label: Some("lens_flare_probe"),
        size: Extent3d {
            width: 3,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: PROBE_FORMAT,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    commands.insert_resource(ProbeTexture(
        probe.create_view(&TextureViewDescriptor::default()),
    ));
    let probe_layout = BindGroupLayoutDescriptor::new(
        "lens_flare_probe_layout",
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (
                    0,
                    texture_2d(TextureSampleType::Float { filterable: false }),
                ),
                (1, texture_depth_2d()),
                (3, uniform_buffer::<LensFlare>(true)),
            ),
        ),
    );
    let flare_layout = BindGroupLayoutDescriptor::new(
        "lens_flare_layout",
        // the probe's slots without the view (one shader module)
        &BindGroupLayoutEntries::with_indices(
            ShaderStages::FRAGMENT,
            (
                (
                    2,
                    texture_2d(TextureSampleType::Float { filterable: false }),
                ),
                (3, uniform_buffer::<LensFlare>(true)),
            ),
        ),
    );
    commands.insert_resource(FlarePipeline {
        probe_layout,
        flare_layout,
        probe: None,
        flare: Vec::new(),
        shader: assets.load("shaders/lens_flare.wesl"),
        vertex: fullscreen.to_vertex_state(),
    });
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipe: ResMut<FlarePipeline>,
    views: Query<(Entity, &ViewTarget), With<LensFlare>>,
) {
    if pipe.probe.is_none() && !views.is_empty() {
        pipe.probe = Some(cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some("lens_flare_probe".into()),
            layout: vec![pipe.probe_layout.clone()],
            vertex: pipe.vertex.clone(),
            fragment: Some(FragmentState {
                shader: pipe.shader.clone(),
                entry_point: Some("probe".into()),
                // new · (1 − a) + last · a (the alpha is only the weight)
                targets: vec![Some(ColorTargetState {
                    format: PROBE_FORMAT,
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::OneMinusSrcAlpha,
                            dst_factor: BlendFactor::SrcAlpha,
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::Zero,
                            dst_factor: BlendFactor::One,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            ..default()
        }));
    }
    for (e, target) in &views {
        let format = target.main_texture_format();
        let id = match pipe.flare.iter().find(|(f, _)| *f == format) {
            Some((_, id)) => *id,
            None => {
                // adds the flare's light, leaves alpha alone
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
                    label: Some("lens_flare".into()),
                    layout: vec![pipe.flare_layout.clone()],
                    vertex: pipe.vertex.clone(),
                    fragment: Some(FragmentState {
                        shader: pipe.shader.clone(),
                        entry_point: Some("flare".into()),
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
                pipe.flare.push((format, id));
                id
            }
        };
        commands.entity(e).insert(FlareViewPipeline(id));
    }
}

fn lens_flare_pass(
    // With<LensFlare>: the index, texture and pipeline id outlive a removed LensFlare
    view: ViewQuery<
        (
            &ViewTarget,
            &ViewDepthStencilTexture,
            &DynamicUniformIndex<LensFlare>,
            &FlareViewPipeline,
        ),
        With<LensFlare>,
    >,
    pipe: Res<FlarePipeline>,
    tex: Res<ProbeTexture>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<LensFlare>>,
    mut ctx: RenderContext,
) {
    let (target, depth, index, pid) = view.into_inner();
    let (Some(probe), Some(flare)) = (
        pipe.probe.and_then(|id| cache.get_render_pipeline(id)),
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

    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "lens_flare");
    let probe_group = device.create_bind_group(
        "lens_flare_probe_bind_group",
        &cache.get_bind_group_layout(&pipe.probe_layout),
        &BindGroupEntries::with_indices((
            (0, target.main_texture_view()),
            (1, depth_view),
            (3, binding.clone()),
        )),
    );
    {
        let mut pass = ctx
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("lens_flare_probe"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &tex.0,
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
        pass.set_pipeline(probe);
        pass.set_bind_group(0, &probe_group, &[index.index()]);
        pass.draw(0..3, 0..1);
    }

    let flare_group = device.create_bind_group(
        "lens_flare_bind_group",
        &cache.get_bind_group_layout(&pipe.flare_layout),
        &BindGroupEntries::with_indices(((2, &tex.0), (3, binding.clone()))),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("lens_flare"),
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
    pass.set_pipeline(flare);
    pass.set_bind_group(0, &flare_group, &[index.index()]);
    pass.draw(0..3, 0..1);
    drop(pass);
    span.end(ctx.command_encoder());
}
