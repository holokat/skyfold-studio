//! Under water: when the camera is below the surface of the sea, a lake or a river, one
//! fullscreen pass (`shaders/underwater.wesl`, in HDR before bloom) turns the scene into a view
//! through water of that body's style (its clarity and colours, `water::water_optics`):
//!
//! - the light that reached the floor and objects fell through the water above them: dimmed per
//!   colour with depth and focused into the moving caustic net of the waves overhead;
//! - along every view ray light is absorbed and scattered: distance fades to the water's deep
//!   colour, and sun shafts slant down through it (the same caustic pattern, blurred, projected
//!   along the refracted sun), brightest looking towards the sun;
//! - drifting motes of marine snow catch the light at every depth.
//!
//! The surface seen from below (Snell's window, total internal reflection, the sun's disc
//! through the waves) is drawn by the water shader itself. Exhaled bubbles rise from the diver
//! and burst at the surface; the ambience ducks (audio.rs reads `WalkState::submerged`).

use std::collections::HashMap;

use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::{Core3dSystems, FullscreenShader};
use bevy::light::NotShadowCaster;
use bevy::post_process::bloom::bloom;
use bevy::prelude::*;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin,
    UniformComponentPlugin,
};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{
    sampler, texture_2d, texture_depth_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, FilterMode, FragmentState, Operations, PipelineCache,
    RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, TextureFormat,
    TextureSampleType, VertexState,
};
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{ExtractedView, ViewDepthStencilTexture, ViewTarget};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use super::movement::InWater;
use crate::camera::MainCamera;
use crate::terrain::Terrain;
use crate::walk::WalkState;
use crate::water::{WaterBodies, WaterSel, WaterStyle};

/// Uniforms of the underwater pass: on the main camera only while it is under water.
#[derive(Component, Clone, Copy, Default, Debug, ShaderType, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct UnderwaterView {
    pub clip_to_world: Mat4,
    /// xyz: camera position, w: time (s)
    pub cam: Vec4,
    /// xyz: camera forward, w: near plane (m)
    pub fwd: Vec4,
    /// x: surface level above the camera, y: caustic strength, z: share of sunlight in the light
    /// (vs sky), w: how far the shafts reach (m)
    pub level: Vec4,
    /// rgb: extinction per metre, w: clarity (m)
    pub extinct: Vec4,
    /// rgb: in-scatter albedo (the deep colour)
    pub inscat: Vec4,
    /// xyz: direction to the key light, w: key strength
    pub sun: Vec4,
    /// rgb: key light (display units)
    pub sun_col: Vec4,
    /// rgb: sky irradiance (display units)
    pub sky: Vec4,
}

/// The caustics the finest wave cascade focuses (shared with the water shader).
#[derive(Resource, Clone, ExtractResource)]
#[extract_app(RenderApp)]
pub struct UnderwaterCaustics(pub Handle<Image>);

/// A bubble exhaled by the diver, rising to burst at the surface.
#[derive(Component)]
pub struct Bubble {
    vel: Vec3,
    phase: f32,
    age: f32,
}

#[derive(Resource)]
struct BubbleAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

pub struct UnderwaterPlugin;

impl Plugin for UnderwaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            ExtractComponentPlugin::<UnderwaterView>::default(),
            UniformComponentPlugin::<UnderwaterView>::default(),
            ExtractResourcePlugin::<UnderwaterCaustics>::default(),
        ))
        .add_systems(Startup, setup)
        .add_systems(
            PostUpdate,
            update_view.after(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(Update, bubbles.after(crate::walk::WalkSystems));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(
                Core3d,
                underwater_pass
                    .in_set(Core3dSystems::PostProcess)
                    .before(bloom),
            );
    }
}

fn setup(
    mut commands: Commands,
    tex: Res<crate::ocean::OceanTextures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(UnderwaterCaustics(tex.caustics.clone()));
    // bubbles look silvery under water (light reflects off them totally): small mirror spheres
    commands.insert_resource(BubbleAssets {
        mesh: meshes.add(
            Sphere::new(1.0)
                .mesh()
                .ico(2)
                .unwrap_or_else(|_| Sphere::new(1.0).mesh().uv(12, 8)),
        ),
        material: mats.add(StandardMaterial {
            base_color: Color::srgb(0.88, 0.95, 1.0),
            metallic: 0.85,
            perceptual_roughness: 0.12,
            reflectance: 0.9,
            emissive: LinearRgba::rgb(0.02, 0.03, 0.035),
            ..default()
        }),
    });
}

/// The style of the water body around a point.
pub fn body_style<'a>(
    terrain: &Terrain,
    water: &'a WaterBodies,
    ocean: &'a WaterStyle,
    xz: Vec2,
) -> &'a WaterStyle {
    match water.body_at(terrain, xz) {
        Some(WaterSel::Lake(id)) => water
            .lakes
            .iter()
            .find(|l| l.id == id)
            .map(|l| &l.style)
            .unwrap_or(ocean),
        Some(WaterSel::River(id)) => water
            .rivers
            .iter()
            .find(|r| r.id == id)
            .map(|r| &r.style)
            .unwrap_or(ocean),
        Some(WaterSel::Waterfall(id)) => water
            .waterfalls
            .iter()
            .find(|f| f.id == id)
            .map(|f| &f.style)
            .unwrap_or(ocean),
        _ => ocean,
    }
}

#[allow(clippy::too_many_arguments)]
fn update_view(
    mut commands: Commands,
    cam: Query<
        (
            Entity,
            &Camera,
            &GlobalTransform,
            &Projection,
            Has<UnderwaterView>,
        ),
        With<MainCamera>,
    >,
    terrain: Res<Terrain>,
    water: Res<WaterBodies>,
    ocean: Res<WaterStyle>,
    waves: Res<crate::ocean::OceanWaves>,
    time: Res<Time>,
    mats: Option<Res<crate::water::WaterMaterials>>,
    materials: Res<Assets<crate::water::WaterMaterial>>,
    mut walk: ResMut<WalkState>,
    caves: Res<crate::caves::render::CaveWorld>,
) {
    let Ok((e, camera, gt, proj, has)) = cam.single() else {
        return;
    };
    let eye = gt.translation();
    let now = time.elapsed_secs_wrapped();
    let surface = super::waves::surface_at(&terrain, &water, &ocean, &waves, eye.xz(), &time);
    // (in a cave under a river or lake the rock keeps the water out)
    let under = surface.is_some_and(|l| eye.y < l - 0.03) && !caves.in_space(eye);
    let target = if under { 1.0 } else { 0.0 };
    if walk.submerged != target {
        walk.submerged = target;
    }
    let light = mats.and_then(|m| materials.get(&m.ocean).map(|m| m.extension.params.clone()));
    let (Some(level), Some(light), true) = (surface, light, under) else {
        if has {
            commands.entity(e).remove::<UnderwaterView>();
        }
        return;
    };
    let style = body_style(&terrain, &water, &ocean, eye.xz());
    let (ext, deep) = crate::water::water_optics(style);
    // over a shallow, bright floor the water glows with the light the floor sends back up: the
    // shallow colour takes over from the deep one
    let shallow = Color::srgb(style.shallow[0], style.shallow[1], style.shallow[2]).to_linear();
    let shallow = Vec3::new(shallow.red, shallow.green, shallow.blue);
    let floor_depth = (level - terrain.height_at(eye.x, eye.z)).max(0.0);
    let ins = deep.lerp(shallow * 0.45, 0.6 * (-floor_depth / 7.0).exp());
    let near = match proj {
        Projection::Perspective(p) => p.near,
        _ => 0.3,
    };
    let clip_to_world = gt.to_matrix() * camera.clip_from_view().inverse();
    // share of the sun in the light down there (the sky fills the rest)
    let sun_lum =
        light.sun_col.truncate().dot(Vec3::new(0.3, 0.5, 0.2)) * light.sun.w * light.sun.y.max(0.0);
    let sky_lum = light.sky.truncate().dot(Vec3::new(0.3, 0.5, 0.2)) * 1.5;
    let sun_w = sun_lum / (sun_lum + sky_lum).max(1e-4);
    // calm or murky water barely focuses light; clear water with chop draws a bright net
    let caustic = (0.4 + 0.6 * crate::noise::smoothstep(0.5, 6.0, style.clarity))
        * (0.5 + 0.5 * style.waves.min(1.5));
    commands.entity(e).insert(UnderwaterView {
        clip_to_world,
        cam: eye.extend(now),
        fwd: gt.forward().as_vec3().extend(near),
        level: Vec4::new(
            level,
            caustic,
            sun_w,
            (style.clarity * 6.0).clamp(12.0, 70.0),
        ),
        extinct: ext.extend(style.clarity),
        inscat: ins.extend(0.0),
        sun: light.sun,
        sun_col: light.sun_col,
        sky: light.sky,
    });
}

/// Breath bubbles: a puff every few seconds while diving, a burst on going under.
#[allow(clippy::too_many_arguments)]
fn bubbles(
    mut commands: Commands,
    assets: Option<Res<BubbleAssets>>,
    walk: Res<WalkState>,
    time: Res<Time>,
    terrain: Res<Terrain>,
    water: Res<WaterBodies>,
    ocean: Res<WaterStyle>,
    waves: Res<crate::ocean::OceanWaves>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(Entity, &mut Bubble, &mut Transform)>,
    mut next: Local<f32>,
    mut levels: Local<HashMap<(i32, i32), f32>>,
) {
    let Some(assets) = assets else { return };
    let dt = time.delta_secs().min(0.05);
    levels.clear();
    let mut level_at = |p: Vec3| {
        *levels
            .entry(((p.x * 0.5) as i32, (p.z * 0.5) as i32))
            .or_insert_with(|| {
                super::waves::surface_at(&terrain, &water, &ocean, &waves, p.xz(), &time)
                    .unwrap_or(f32::MIN)
            })
    };
    for (e, mut b, mut t) in &mut q {
        b.age += dt;
        // bigger bubbles rise faster and wobble; they swell a little nearing the surface
        let s = t.scale.x;
        b.vel.y += (0.25 + s * 90.0 - b.vel.y) * (1.0 - (-dt * 3.0).exp());
        let wob = Vec3::new(
            (b.age * 7.0 + b.phase).sin(),
            0.0,
            (b.age * 6.1 + b.phase * 1.7).cos(),
        ) * (0.05 + s * 8.0);
        t.translation += (b.vel + wob) * dt;
        t.scale = Vec3::splat(s * (1.0 + 0.02 * dt));
        if t.translation.y > level_at(t.translation) - 0.01 || b.age > 14.0 || !walk.active {
            commands.entity(e).despawn();
        }
    }
    if !walk.active || walk.water != InWater::Diving {
        *next = 0.6;
        return;
    }
    let Ok(cam) = cam.single() else { return };
    let dived = walk.last.as_ref().is_some_and(|(o, _)| o.dived);
    *next -= dt;
    let burst = if dived {
        28
    } else if *next <= 0.0 {
        *next = 3.2 + fastrand::f32() * 1.6;
        5 + fastrand::usize(..6)
    } else {
        0
    };
    let mouth = cam.translation() + cam.forward().as_vec3() * 0.35 - Vec3::Y * 0.12;
    for _ in 0..burst {
        let spread = if dived { 0.9 } else { 0.06 };
        let off = Vec3::new(
            fastrand::f32() - 0.5,
            fastrand::f32() - 0.5,
            fastrand::f32() - 0.5,
        ) * spread;
        let r = 0.002 + fastrand::f32().powi(2) * 0.007;
        commands.spawn((
            Name::new("Bubble"),
            Bubble {
                vel: Vec3::new(0.0, 0.2 + fastrand::f32() * 0.3, 0.0),
                phase: fastrand::f32() * 6.3,
                age: fastrand::f32() * 0.3,
            },
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
            Transform::from_translation(mouth + off).with_scale(Vec3::splat(r)),
            NotShadowCaster,
        ));
    }
}

// ---------------------------------------------------------------- render world

#[derive(Resource)]
struct UnderwaterPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    shader: Handle<Shader>,
    vertex: VertexState,
    ids: HashMap<TextureFormat, CachedRenderPipelineId>,
}

#[derive(Component)]
struct UnderwaterPipelineId(CachedRenderPipelineId);

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    assets: Res<AssetServer>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "underwater_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_depth_2d(),
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer::<UnderwaterView>(true),
            ),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    commands.insert_resource(UnderwaterPipeline {
        layout,
        sampler,
        shader: assets.load("shaders/underwater.wesl"),
        vertex: fullscreen.to_vertex_state(),
        ids: HashMap::new(),
    });
}

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipe: ResMut<UnderwaterPipeline>,
    views: Query<(Entity, &ExtractedView), With<UnderwaterView>>,
) {
    for (e, view) in &views {
        let format = view.target_format;
        let id = match pipe.ids.get(&format) {
            Some(id) => *id,
            None => {
                let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("underwater_pipeline".into()),
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
        commands.entity(e).insert(UnderwaterPipelineId(id));
    }
}

#[allow(clippy::too_many_arguments)]
fn underwater_pass(
    // (the uniform index and pipeline id outlive the view's `UnderwaterView`: require it too, or
    // the pass would keep running on stale uniforms after surfacing)
    view: ViewQuery<
        (
            &ViewTarget,
            &ViewDepthStencilTexture,
            &DynamicUniformIndex<UnderwaterView>,
            &UnderwaterPipelineId,
        ),
        With<UnderwaterView>,
    >,
    pipe: Res<UnderwaterPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<UnderwaterView>>,
    caustics: Option<Res<UnderwaterCaustics>>,
    images: Res<RenderAssets<GpuImage>>,
    mut ctx: RenderContext,
) {
    let (target, depth, index, pid) = view.into_inner();
    let Some(pipeline) = cache.get_render_pipeline(pid.0) else {
        return;
    };
    let Some(binding) = uniforms.uniforms().binding() else {
        return;
    };
    let Some(depth_view) = depth.attachment.depth_stencil_views().depth_only_view() else {
        return;
    };
    let Some(caus) = caustics.and_then(|c| images.get(&c.0)) else {
        return;
    };
    let post = target.post_process_write();
    let bind_group = ctx.render_device().create_bind_group(
        "underwater_bind_group",
        &cache.get_bind_group_layout(&pipe.layout),
        &BindGroupEntries::sequential((
            post.source,
            &pipe.sampler,
            depth_view,
            &caus.texture_view,
            &caus.sampler,
            binding.clone(),
        )),
    );
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("underwater_pass"),
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
