//! Render plumbing for the art-style post pass: one fullscreen pass after tonemapping (before
//! SMAA) that reads the tonemapped colour and the main-pass depth buffer. Modelled on Bevy's
//! `FullscreenMaterial`, plus the depth binding (sky detection, distance haze, outlines) and the
//! look's colour grade as a 3D LUT (`GradeLut`, baked by `look::bake_grade_lut` from
//! `crate::grade`).

use std::collections::HashMap;

use bevy::anti_alias::smaa::smaa;
use bevy::core_pipeline::prepass::ViewPrepassTextures;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_3d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, TextureDimension,
    TextureFormat, TextureSampleType, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{FallbackImage, GpuImage};
use bevy::render::view::{ExtractedView, ViewDepthStencilTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

/// Uniforms of the style pass. Present on the camera only while a look needs the pass.
#[derive(Component, Clone, Copy, Default, Debug, ShaderType, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct LookPost {
    /// x: painterly strength, y: painterly radius (px), z: outline strength, w: film grain
    pub paint: Vec4,
    /// rgb: sky tint (linear), w: amount
    pub sky: Vec4,
    /// rgb: haze colour (linear display), w: amount
    pub haze: Vec4,
    /// x: near plane, y: time, w: haze distance (m)
    pub misc: Vec4,
    /// xyz: camera right, w: tan(fov/2) * aspect
    pub cam_right: Vec4,
    /// xyz: camera up, w: tan(fov/2)
    pub cam_up: Vec4,
    /// xyz: camera forward, w: window scale factor
    pub cam_fwd: Vec4,
    /// x: distance (m) where the colour haze starts, y: contrast (display space; film grain)
    pub extra: Vec4,
    /// x: distant forest softness (0 disables), y/z: fade start/end (m), w: pixels per point
    pub forest: Vec4,
}

/// The look's colour grade as a 3D LUT over display colour (gamma 2.2), `grade::RUNTIME_SIZE`³.
/// One image asset for the app's life; `look::bake_grade_lut` replaces its contents.
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct GradeLut(pub Handle<Image>);

impl FromWorld for GradeLut {
    fn from_world(world: &mut World) -> Self {
        let id = crate::grade::Lut3::identity(crate::grade::RUNTIME_SIZE);
        GradeLut(world.resource_mut::<Assets<Image>>().add(lut_image(&id)))
    }
}

/// A LUT as a 3D texture (x = red, y = green, z = blue), RGBA16F so strong grades don't band.
pub fn lut_image(lut: &crate::grade::Lut3) -> Image {
    let n = lut.size as u32;
    Image::new(
        Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: n,
        },
        TextureDimension::D3,
        lut.to_rgba16f(),
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    )
}

pub struct LookPostPlugin;

impl Plugin for LookPostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GradeLut>().add_plugins((
            ExtractComponentPlugin::<LookPost>::default(),
            UniformComponentPlugin::<LookPost>::default(),
            ExtractResourcePlugin::<GradeLut>::default(),
        ));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(
                Core3d,
                look_post_pass
                    .in_set(Core3dSystems::PostProcess)
                    .after(tonemapping)
                    .before(smaa),
            );
    }
}

#[derive(Resource)]
struct LookPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    shader: Handle<Shader>,
    vertex: VertexState,
    ids: HashMap<TextureFormat, CachedRenderPipelineId>,
}

#[derive(Component)]
struct LookPipelineId(CachedRenderPipelineId);

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "look_post_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
                uniform_buffer::<LookPost>(true),
                texture_2d(TextureSampleType::Float { filterable: false }),
                texture_depth_2d(),
                texture_3d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(LookPipeline {
        layout,
        sampler,
        // Separate asset path per bind group layout: an older running app hot-reloads its own.
        shader: assets.load("shaders/look_post_grade.wesl"),
        vertex: fullscreen.to_vertex_state(),
        ids: HashMap::new(),
    });
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipe: ResMut<LookPipeline>,
    views: Query<(Entity, &ExtractedView), With<LookPost>>,
) {
    for (e, view) in &views {
        let format = view.target_format;
        let id = match pipe.ids.get(&format) {
            Some(id) => *id,
            None => {
                let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("look_post_pipeline".into()),
                    layout: vec![pipe.layout.clone()],
                    vertex: pipe.vertex.clone(),
                    fragment: Some(FragmentState {
                        shader: pipe.shader.clone(),
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                        ..default()
                    }),
                    ..default()
                });
                pipe.ids.insert(format, id);
                id
            }
        };
        commands.entity(e).insert(LookPipelineId(id));
    }
}

fn look_post_pass(
    // With<LookPost>: the index and pipeline id outlive a removed LookPost (stale pass otherwise)
    view: ViewQuery<
        (
            &ViewTarget,
            &ViewDepthStencilTexture,
            &DynamicUniformIndex<LookPost>,
            &LookPipelineId,
            Option<&ViewPrepassTextures>,
        ),
        With<LookPost>,
    >,
    pipe: Res<LookPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<LookPost>>,
    fallback: Res<FallbackImage>,
    grade: Option<Res<GradeLut>>,
    images: Res<RenderAssets<GpuImage>>,
    mut ctx: RenderContext,
) {
    let (target, depth, index, pid, prepass) = view.into_inner();
    let Some(pipeline) = cache.get_render_pipeline(pid.0) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
        return;
    };
    // (checked before taking the post-process write: skipping after it would show a stale frame)
    let Some(lut) = grade.as_ref().and_then(|g| images.get(&g.0)) else {
        return;
    };
    let post = target.post_process_write();
    // White alpha (1) is untagged, so the fallback cannot activate forest softening.
    // Keep the rest of the style pass working while prepass textures are unavailable.
    let normal_view = prepass
        .and_then(|p| p.normal_view())
        .unwrap_or(&fallback.d2.texture_view);
    let prepass_depth = prepass
        .and_then(|p| p.depth_only_view())
        .unwrap_or(depth_view);
    let bind_group = ctx.render_device().create_bind_group(
        "look_post_bind_group",
        &cache.get_bind_group_layout(&pipe.layout),
        &BindGroupEntries::sequential((
            post.source,
            &pipe.sampler,
            depth_view,
            binding.clone(),
            normal_view,
            prepass_depth,
            &lut.texture_view,
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("look_post_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post.destination,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[index.index()]);
    pass.draw(0..3, 0..1);
}
