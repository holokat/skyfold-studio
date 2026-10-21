//! Render resolution: the 3D view can draw fewer pixels than the window has and be upscaled to
//! it, while the editor's panels stay sharp. The editor is bound by pixel shading (a 5K display
//! shades ~13 MP a frame; at 75% per axis it shades 56% of them), so this is the biggest single
//! frame-rate setting. Lighting tab › Performance › Render resolution, MCP `lighting_set`
//! `render_resolution`; saved per machine in the prefs (a Retina laptop and a desktop want
//! different values), not with the world.
//!
//! How: a second camera, "View output", owns the window (or `WB_OFFSCREEN`'s image) and the egui
//! context. At 100% the main camera draws into the same intermediate texture as it (its output
//! `Skip`ped: no extra pass at all) and the output camera adds the panels and presents. Below
//! 100% the main camera draws into an image of the reduced size whose scale factor makes its
//! *logical* size equal the window's, so every cursor ray (`viewport_to_world`) and every
//! projected handle and label (`world_to_viewport`) keeps working in window points unchanged;
//! the output camera upscales that image (`upscale.wesl`) under the panels. Every pass of the
//! main camera (water, clouds, the art-style pass, fog, SMAA) simply runs at the smaller size.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{
    CameraMainTextureUsages, CameraOutputMode, CameraUpdateSystems, ClearColorConfig, Hdr,
    ImageRenderTarget, RenderTarget,
};
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::upscaling::upscaling;
use bevy::ecs::schedule::{ScheduleBuildSettings, ScheduleLabel};
use bevy::prelude::*;
use bevy::render::camera::CameraRenderGraph;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_2d};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendState,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, Extent3d, FilterMode, FragmentState,
    LoadOp, Operations, PipelineCache, RenderPassColorAttachment, RenderPassDescriptor,
    RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    StoreOp, TextureDimension, TextureFormat, TextureId, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::window::{PrimaryWindow, WindowRef};
use bevy_egui::{EguiGlobalSettings, PrimaryEguiContext};

use crate::camera::MainCamera;

/// The prefs key.
const PREF: &str = "render_resolution";
/// The output camera's only render layer, which nothing is on: it draws no world (and doesn't
/// mark the whole world visible to itself), only the upscaled view and the panels.
const OUTPUT_LAYER: usize = 27;

/// The Render resolution setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderResolution {
    /// 75% on high-density (Retina, 5K) displays, where the extra pixels cost the most and show
    /// the least; 100% elsewhere.
    #[default]
    Auto,
    Full,
    P85,
    P75,
    P67,
    P50,
}

impl RenderResolution {
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Full,
        Self::P85,
        Self::P75,
        Self::P67,
        Self::P50,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Full => "100",
            Self::P85 => "85",
            Self::P75 => "75",
            Self::P67 => "67",
            Self::P50 => "50",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Full => "100%",
            Self::P85 => "85%",
            Self::P75 => "75%",
            Self::P67 => "67%",
            Self::P50 => "50%",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_end_matches('%').to_ascii_lowercase();
        Self::ALL.into_iter().find(|r| r.id() == s)
    }

    /// The fraction of the output's pixels the 3D view draws, per axis, on a display with this
    /// scale factor (pixels per point).
    pub fn scale(self, scale_factor: f32) -> f32 {
        match self {
            Self::Auto => {
                if scale_factor >= 1.75 {
                    0.75
                } else {
                    1.0
                }
            }
            Self::Full => 1.0,
            Self::P85 => 0.85,
            Self::P75 => 0.75,
            Self::P67 => 0.67,
            Self::P50 => 0.5,
        }
    }
}

/// The render resolution: the setting and what it gives on this display.
#[derive(Resource)]
pub struct RenderScale {
    pub setting: RenderResolution,
    /// The scale in use, per axis (1: every output pixel is rendered).
    pub active: f32,
    /// The 3D view's size in pixels.
    pub pixels: UVec2,
    /// A forced scale (benchmarks: `WB_BENCH` `scale=0.75`).
    pub force: Option<f32>,
    /// The setting last written to the prefs.
    saved: RenderResolution,
}

impl RenderScale {
    fn load() -> Self {
        let setting = crate::prefs::get(PREF)
            .and_then(|v| v.as_str().and_then(RenderResolution::parse))
            .unwrap_or_default();
        Self {
            setting,
            active: 1.0,
            pixels: UVec2::ZERO,
            force: None,
            saved: setting,
        }
    }
}

/// Marks the camera that presents the 3D view and the panels.
#[derive(Component)]
pub struct ViewOutput;

/// `WB_OFFSCREEN`: the image everything is presented into instead of the window (`debug.rs`).
#[derive(Resource, Clone)]
pub struct OutputImage(pub Handle<Image>);

/// On the output camera: the reduced-size image of the 3D view it upscales (None at 100%).
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct UpscaleSource(pub Option<Handle<Image>>);

/// The output camera's render schedule: upscale (below 100%), the panels, present.
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ViewOutputGraph;

pub struct RenderScalePlugin;

impl Plugin for RenderScalePlugin {
    fn build(&self, app: &mut App) {
        // the panels' egui context lives on the output camera (it must not follow the 3D view
        // into a smaller image)
        match app.world_mut().get_resource_mut::<EguiGlobalSettings>() {
            Some(mut s) => s.auto_create_primary_context = false,
            None => {
                app.insert_resource(EguiGlobalSettings {
                    auto_create_primary_context: false,
                    ..default()
                });
            }
        }
        app.insert_resource(RenderScale::load())
            .add_plugins(ExtractComponentPlugin::<UpscaleSource>::default())
            .add_systems(Startup, spawn_output)
            .add_systems(PostUpdate, apply.before(CameraUpdateSystems))
            .add_systems(Last, save_pref);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        let mut graph = Schedule::new(ViewOutputGraph);
        graph.set_build_settings(ScheduleBuildSettings {
            auto_insert_apply_deferred: false,
            ..default()
        });
        render_app
            .add_schedule(graph)
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(
                ViewOutputGraph,
                (
                    upscale_pass,
                    bevy_egui::render::prepare_egui_pass,
                    bevy_egui::render::egui_pass,
                    upscaling,
                )
                    .chain(),
            );
    }
}

fn spawn_output(mut commands: Commands) {
    commands.spawn((
        Name::new("View output"),
        ViewOutput,
        Camera {
            // after every other camera; keeps what the main camera drew (100%: their shared texture)
            order: 1000,
            clear_color: ClearColorConfig::None,
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::REPLACE),
                clear_color: ClearColorConfig::Default,
            },
            ..default()
        },
        CameraRenderGraph::new(ViewOutputGraph),
        // (Bevy only sizes cameras that have a projection; it draws nothing with it)
        Projection::Orthographic(OrthographicProjection::default_2d()),
        Msaa::Off,
        Hdr,
        RenderLayers::layer(OUTPUT_LAYER),
        UpscaleSource(None),
        PrimaryEguiContext,
    ));
}

/// What the cameras were last set up for.
#[derive(Default)]
struct Applied {
    /// the output: None the window, else `WB_OFFSCREEN`'s image (outer None: not set up yet)
    output: Option<Option<AssetId<Image>>>,
    /// the reduced 3D view's image and size (below 100%)
    scene: Option<(Handle<Image>, UVec2)>,
    image_scale: f32,
    direct: Option<bool>,
}

/// The 3D view's image at a reduced size: drawn through an sRGB view (the camera's output),
/// sampled through a plain one (`upscale_pass` filters display-encoded values).
fn scene_image(size: UVec2) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT;
    img.texture_descriptor.view_formats = &[TextureFormat::Rgba8UnormSrgb];
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        format: Some(TextureFormat::Rgba8UnormSrgb),
        ..default()
    });
    img
}

/// Points the main and output cameras at the right targets for the current scale (only when
/// something changed: a new target re-sizes every render texture of the view).
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn apply(
    mut commands: Commands,
    mut rs: ResMut<RenderScale>,
    mut images: ResMut<Assets<Image>>,
    out_image: Option<Res<OutputImage>>,
    window: Query<&Window, With<PrimaryWindow>>,
    fly: Option<Res<crate::motion::flythrough::Flythrough>>,
    mut main: Query<
        (
            Entity,
            &mut Camera,
            &CameraMainTextureUsages,
            &mut Projection,
        ),
        (With<MainCamera>, Without<ViewOutput>),
    >,
    mut output: Query<
        (
            Entity,
            &mut CameraMainTextureUsages,
            &mut UpscaleSource,
            &mut Projection,
        ),
        (With<ViewOutput>, Without<MainCamera>),
    >,
    mut done: Local<Applied>,
) {
    let (
        Ok((main_e, mut main_cam, usages, mut main_proj)),
        Ok((out_e, mut out_usages, mut src, mut out_proj)),
    ) = (main.single_mut(), output.single_mut())
    else {
        return;
    };
    // the output: WB_OFFSCREEN's image (1 pixel per point; set up once it exists) or the window
    if out_image.is_none() && std::env::var("WB_OFFSCREEN").is_ok() {
        return;
    }
    let (out_id, phys, sf) = match &out_image {
        Some(o) => match images.get(&o.0) {
            Some(i) => (Some(o.0.id()), i.size(), 1.0),
            None => return,
        },
        None => match window.single() {
            Ok(w) => (None, w.physical_size(), w.scale_factor()),
            Err(_) => return,
        },
    };
    if phys.x == 0 || phys.y == 0 {
        return;
    }
    // a flythrough capture records full-resolution frames (it isn't real time)
    let capture = fly.is_some_and(|f| f.playing.as_ref().is_some_and(|p| p.capture.is_some()));
    let scale = if capture {
        1.0
    } else {
        rs.force.unwrap_or_else(|| rs.setting.scale(sf))
    };
    let scale = if scale > 0.995 {
        1.0
    } else {
        scale.clamp(0.25, 1.0)
    };
    let direct = scale >= 1.0;
    let size = if direct {
        phys
    } else {
        (phys.as_vec2() * scale).round().as_uvec2().max(UVec2::ONE)
    };
    let out_target = || match &out_image {
        Some(o) => RenderTarget::Image(o.0.clone().into()),
        None => RenderTarget::Window(WindowRef::Primary),
    };
    // (Bevy re-sizes a camera for a new target only when its projection changed, or its window
    // or image did: without the nudge it kept the old size, and the cameras sharing a texture
    // disagreed about it)
    let new_output = done.output != Some(out_id);
    if new_output {
        commands.entity(out_e).insert(out_target());
        out_proj.set_changed();
    }
    if direct {
        if new_output || done.direct != Some(true) {
            commands.entity(main_e).insert(out_target());
            main_proj.set_changed();
            commands.entity(out_e).insert(Hdr);
            src.0 = None;
            done.scene = None;
        }
        // the main camera leaves its picture in the texture it shares with the output camera
        // (same target, format and usages), which presents it under the panels
        if !matches!(main_cam.output_mode, CameraOutputMode::Skip) {
            main_cam.output_mode = CameraOutputMode::Skip;
        }
        if out_usages.0 != usages.0 {
            out_usages.0 = usages.0;
        }
    } else {
        let resized = done.scene.as_ref().is_none_or(|(_, s)| *s != size);
        if resized {
            done.scene = Some((images.add(scene_image(size)), size));
        }
        let Some((h, _)) = done.scene.clone() else {
            return;
        };
        // (logical size = the output's: cursor rays and projected handles work in window points)
        let image_scale = sf * size.x as f32 / phys.x as f32;
        if resized || new_output || done.direct != Some(false) || done.image_scale != image_scale {
            commands
                .entity(main_e)
                .insert(RenderTarget::Image(ImageRenderTarget {
                    handle: h.clone(),
                    scale_factor: image_scale,
                }));
            main_proj.set_changed();
            commands.entity(out_e).remove::<Hdr>();
            src.0 = Some(h);
            done.image_scale = image_scale;
        }
        if matches!(main_cam.output_mode, CameraOutputMode::Skip) {
            main_cam.output_mode = CameraOutputMode::default();
        }
    }
    done.output = Some(out_id);
    done.direct = Some(direct);
    if rs.active != scale {
        rs.active = scale;
    }
    if rs.pixels != size {
        rs.pixels = size;
    }
}

/// Remembers a changed setting (automated runs never write the prefs: `prefs::set`).
fn save_pref(mut rs: ResMut<RenderScale>) {
    if rs.setting != rs.saved {
        rs.saved = rs.setting;
        crate::prefs::set(PREF, serde_json::json!(rs.setting.id()));
    }
}

// ---------------------------------------------------------------- render world

#[derive(Resource)]
struct UpscalePipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    shader: Handle<Shader>,
    vertex: VertexState,
    ids: HashMap<TextureFormat, CachedRenderPipelineId>,
}

#[derive(Component)]
struct UpscalePipelineId(CachedRenderPipelineId);

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "upscale_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(UpscalePipeline {
        layout,
        sampler,
        shader: assets.load("shaders/upscale.wesl"),
        vertex: fullscreen.to_vertex_state(),
        ids: HashMap::new(),
    });
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipe: ResMut<UpscalePipeline>,
    views: Query<(Entity, &ExtractedView), With<UpscaleSource>>,
) {
    for (e, view) in &views {
        let format = view.target_format;
        let id = match pipe.ids.get(&format) {
            Some(id) => *id,
            None => {
                let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("upscale_pipeline".into()),
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
        commands.entity(e).insert(UpscalePipelineId(id));
    }
}

/// Draws the reduced 3D view over the whole output (below 100%; the panels follow).
fn upscale_pass(
    view: ViewQuery<(&ViewTarget, &UpscaleSource, &UpscalePipelineId)>,
    pipe: Res<UpscalePipeline>,
    cache: Res<PipelineCache>,
    images: Res<RenderAssets<GpuImage>>,
    mut raw: Local<Option<(TextureId, TextureView)>>,
    mut ctx: RenderContext,
) {
    let (target, src, pid) = view.into_inner();
    let Some(handle) = &src.0 else { return };
    let (Some(gpu), Some(pipeline)) = (images.get(handle), cache.get_render_pipeline(pid.0)) else {
        return;
    };
    let id = gpu.texture.id();
    if raw.as_ref().is_none_or(|(t, _)| *t != id) {
        *raw = Some((
            id,
            gpu.texture.create_view(&TextureViewDescriptor {
                format: Some(TextureFormat::Rgba8Unorm),
                ..default()
            }),
        ));
    }
    let Some((_, plain)) = raw.as_ref() else {
        return;
    };
    let bind_group = ctx.render_device().create_bind_group(
        "upscale_bind_group",
        &cache.get_bind_group_layout(&pipe.layout),
        &BindGroupEntries::sequential((plain, &pipe.sampler)),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("upscale_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: target.main_texture_view(),
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Default::default()),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_renders_fewer_pixels_only_on_dense_displays() {
        assert_eq!(RenderResolution::Auto.scale(1.0), 1.0);
        assert_eq!(RenderResolution::Auto.scale(2.0), 0.75);
        assert_eq!(RenderResolution::P50.scale(2.0), 0.5);
        for r in RenderResolution::ALL {
            assert_eq!(RenderResolution::parse(r.id()), Some(r));
            assert_eq!(RenderResolution::parse(r.label()), Some(r));
        }
        assert_eq!(RenderResolution::parse("75%"), Some(RenderResolution::P75));
        assert_eq!(RenderResolution::parse("60"), None);
    }
}
