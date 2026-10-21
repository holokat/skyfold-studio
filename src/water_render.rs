//! Render plumbing for the water: what lies behind the surface.
//!
//! - The main camera gets `ScreenSpaceTransmission`: Bevy copies the opaque scene's colour right
//!   before the transmissive phase, where all water is drawn (opaque, composing everything itself:
//!   the seabed seen along the refracted ray, absorbed and scattered by the water column).
//! - `copy_scene_depth` copies the opaque depth buffer at the same point into an image the water
//!   material binds, so a refracted lookup can tell whether it landed on something behind the
//!   surface (not a pier post or hull in front of it) and how far the light travelled in water.
//! - With no water drawn in the view (the ocean hidden, e.g. inside a dungeon, and every lake and
//!   river frustum-culled) the transmissive phase is empty: Bevy then skips its colour copy, and the
//!   depth copy is skipped too.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraProjection, ClearColorConfig, Exposure, Hdr, RenderTarget};
use bevy::core_pipeline::core_3d::main_opaque_pass_3d;
use bevy::core_pipeline::schedule::Core3d;
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{
    AtmosphereSettings, DistanceFog, ScreenSpaceTransmission, ScreenSpaceTransmissionQuality,
    Transmissive3d, main_transmissive_pass_3d,
};
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::ViewSortedRenderPhases;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::{ExtractedView, ViewDepthStencilTexture};

use crate::camera::MainCamera;

/// Water is absent from the capture camera, preventing a planar-reflection feedback loop.
/// The layer is deliberately separate from thumbnail and output-camera layers.
pub const WATER_RENDER_LAYER: usize = 24;
/// The main camera's cloud dome; reflection capture uses its separate layer and history.
pub const CLOUD_RENDER_LAYER: usize = 25;
/// The reflection camera's own cloud dome. It is deliberately invisible to the main view.
pub const REFLECTION_CLOUD_RENDER_LAYER: usize = 26;

#[derive(Component)]
pub(crate) struct WaterReflectionCamera;

const REFLECTION_TARGET_FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const REFLECTION_MSAA: Msaa = Msaa::Off;
const REFLECTION_SHADOW_FILTER: bevy::light::ShadowFilteringMethod =
    bevy::light::ShadowFilteringMethod::Gaussian;
const REFLECTION_INVERT_CULLING: bool = true;

/// The capture is created only when water first enters the view. Its material
/// variants must nevertheless be prepared before a walker makes it visible.
/// Keep this beside the camera configuration, using the same fixed settings.
/// Fog, atmosphere and the environment map are copied from the main camera;
/// its prepasses, TAA, SSAO, transmission and tonemapping are not copied.
pub(crate) fn reflection_pipeline_key(
    main_key: bevy::pbr::MeshPipelineKey,
) -> bevy::pbr::MeshPipelineKey {
    use bevy::light::ShadowFilteringMethod;
    use bevy::pbr::MeshPipelineKey as Key;
    let mut key = Key::from_target_format(REFLECTION_TARGET_FORMAT)
        | Key::from_msaa_samples(REFLECTION_MSAA.samples())
        | Key::VIEW_PROJECTION_PERSPECTIVE
        | main_key.intersection(Key::DISTANCE_FOG | Key::ATMOSPHERE | Key::ENVIRONMENT_MAP);
    if REFLECTION_INVERT_CULLING {
        key |= Key::INVERT_CULLING;
    }
    key |= match REFLECTION_SHADOW_FILTER {
        ShadowFilteringMethod::Hardware2x2 => Key::SHADOW_FILTER_METHOD_HARDWARE_2X2,
        ShadowFilteringMethod::Gaussian => Key::SHADOW_FILTER_METHOD_GAUSSIAN,
        ShadowFilteringMethod::Temporal => Key::SHADOW_FILTER_METHOD_TEMPORAL,
    };
    key
}

/// The most recently rendered planar view. `clip_from_world` and `exposure_ev100` always describe
/// `image`; they are deliberately left unchanged on skipped capture frames.
#[derive(Resource, Clone)]
pub struct WaterReflection {
    pub image: Handle<Image>,
    pub clip_from_world: Mat4,
    pub plane_y: f32,
    pub exposure_ev100: f32,
    pub valid: bool,
    pub(crate) size: UVec2,
    camera: Option<Entity>,
    frame: u64,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaterRenderSystems;

impl Default for WaterReflection {
    fn default() -> Self {
        Self {
            image: Handle::default(),
            clip_from_world: Mat4::IDENTITY,
            plane_y: 0.0,
            exposure_ev100: 13.0,
            valid: false,
            size: UVec2::ZERO,
            camera: None,
            frame: 0,
        }
    }
}

/// The copy of the opaque depth buffer the water reads (resized with the camera's target).
#[derive(Resource, Clone)]
pub struct SceneDepth {
    pub image: Handle<Image>,
    size: UVec2,
}

/// On the main camera: where `copy_scene_depth` puts its depth copy.
#[derive(Component, Clone, ExtractComponent)]
#[extract_app(RenderApp)]
pub struct WaterDepthTarget(pub Handle<Image>);

pub struct WaterRenderPlugin;

impl Plugin for WaterRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractComponentPlugin::<WaterDepthTarget>::default())
            .configure_sets(PostUpdate, WaterRenderSystems)
            .add_systems(PreStartup, (init_scene_depth, init_water_reflection))
            .add_systems(
                PostUpdate,
                (follow_camera, update_water_reflection)
                    .chain()
                    .in_set(WaterRenderSystems)
                    .after(bevy::camera::CameraUpdateSystems)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app.add_systems(
            Core3d,
            copy_scene_depth
                .after(main_opaque_pass_3d)
                .before(main_transmissive_pass_3d),
        );
    }
}

fn depth_image(size: UVec2) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Depth32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
    img
}

fn init_scene_depth(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = images.add(depth_image(UVec2::ONE));
    commands.insert_resource(SceneDepth {
        image,
        size: UVec2::ONE,
    });
}

fn reflection_image(size: UVec2) -> Image {
    let mut image =
        Image::new_target_texture(size.x.max(1), size.y.max(1), REFLECTION_TARGET_FORMAT, None);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn init_water_reflection(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = images.add(reflection_image(UVec2::ONE));
    commands.insert_resource(WaterReflection {
        image,
        size: UVec2::ONE,
        ..default()
    });
}

type MainCameraState<'a> = (
    Entity,
    &'a Camera,
    Has<ScreenSpaceTransmission>,
    Has<WaterDepthTarget>,
);

/// Keeps the main camera transmissive (the water samples the scene colour behind it) and the
/// depth copy at the camera's resolution.
fn follow_camera(
    mut commands: Commands,
    mut depth: ResMut<SceneDepth>,
    mut images: ResMut<Assets<Image>>,
    cams: Query<MainCameraState, With<MainCamera>>,
) {
    let Ok((e, cam, transmissive, targeted)) = cams.single() else {
        return;
    };
    if !transmissive {
        // one step: all water is drawn after a single copy (the blur quality is unused: the water
        // samples the copy itself)
        commands.entity(e).insert(ScreenSpaceTransmission {
            steps: 1,
            quality: ScreenSpaceTransmissionQuality::Low,
        });
    }
    let Some(size) = cam.physical_target_size() else {
        return;
    };
    if size != depth.size {
        depth.image = images.add(depth_image(size));
        depth.size = size;
        commands
            .entity(e)
            .insert(WaterDepthTarget(depth.image.clone()));
    } else if !targeted {
        commands
            .entity(e)
            .insert(WaterDepthTarget(depth.image.clone()));
    }
}

/// A few lower-frustum rays choose the water which dominates the view. Rivers and falls
/// deliberately do not select the shared planar target: a single horizontal projection cannot
/// represent their slope.
fn reflection_rays(camera: &Transform, projection: &PerspectiveProjection) -> [Vec3; 4] {
    let tan_half_fov = (projection.fov * 0.5).tan();
    [(0.0, 0.0), (0.0, -0.62), (-0.52, -0.55), (0.52, -0.55)].map(|(x, y)| {
        (camera.rotation
            * Vec3::new(
                x * projection.aspect_ratio * tan_half_fov,
                y * tan_half_fov,
                -1.0,
            ))
        .normalize()
    })
}

fn reflection_plane(
    camera: &Transform,
    projection: &PerspectiveProjection,
    terrain: &crate::terrain::Terrain,
    water: &crate::water::WaterBodies,
    ocean: &crate::water::WaterStyle,
    now: f32,
    preferred_plane: Option<f32>,
) -> Option<(f32, crate::water::ReflectionQuality)> {
    let sea = terrain.sea_level + crate::water::tide_offset(ocean, now);
    let mut candidates = Vec::with_capacity(water.lakes.len() + 1);
    candidates.push((sea, crate::water::WaterSel::Ocean, ocean.reflections));
    for lake in &water.lakes {
        candidates.push((
            lake.level,
            crate::water::WaterSel::Lake(lake.id),
            lake.style.reflections,
        ));
    }
    let mut hits = Vec::with_capacity(4);
    for ray in reflection_rays(camera, projection) {
        if ray.y >= -0.001 {
            continue;
        }
        let mut nearest = None;
        for &(level, body, quality) in &candidates {
            let distance = (level - camera.translation.y) / ray.y;
            if distance <= 0.0 {
                continue;
            }
            let hit = camera.translation + ray * distance;
            if water.body_at(terrain, hit.xz()) == Some(body)
                && nearest.is_none_or(|(_, _, _, best)| distance < best)
            {
                nearest = Some((body, level, quality, distance));
            }
        }
        if let Some(hit) = nearest {
            hits.push(hit);
        }
    }
    if let Some(plane) = preferred_plane
        && let Some((_, level, quality, _)) = hits
            .iter()
            .find(|(_, level, _, _)| (*level - plane).abs() < 0.01)
    {
        return Some((*level, *quality));
    }
    let mut best: Option<(usize, f32, f32, crate::water::ReflectionQuality)> = None;
    for &(body, level, quality, distance) in &hits {
        let count = hits.iter().filter(|(other, ..)| *other == body).count();
        let candidate = (count, -distance, level, quality);
        if best.is_none_or(|(best_count, best_distance, ..)| {
            count > best_count || (count == best_count && -distance > best_distance)
        }) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, level, quality)| (level, quality))
}

fn capture_scale(quality: crate::water::ReflectionQuality) -> (f32, u64) {
    match quality {
        crate::water::ReflectionQuality::Low => (0.25, 4),
        crate::water::ReflectionQuality::Medium => (0.5, 2),
        crate::water::ReflectionQuality::High => (0.5, 1),
    }
}

fn reflection_size(main: UVec2, scale: f32) -> UVec2 {
    let mut size = UVec2::new(
        (main.x as f32 * scale).round().max(1.0) as u32,
        (main.y as f32 * scale).round().max(1.0) as u32,
    );
    // A half-resolution reflection is enough to keep coherent shorelines; this cap avoids a
    // second multi-megapixel world render on high-density displays.
    if size.x > 960 {
        size = UVec2::new(960, (size.y as u64 * 960 / size.x as u64).max(1) as u32);
    }
    size
}

fn mirrored_camera(
    main_transform: &Transform,
    main_projection: &PerspectiveProjection,
    plane_y: f32,
) -> (Transform, PerspectiveProjection, Mat4) {
    let reflect = Mat4::from_translation(Vec3::Y * plane_y)
        * Mat4::from_mat3a(bevy::math::reflection_matrix(Vec3::Y))
        * Mat4::from_translation(Vec3::NEG_Y * plane_y);
    let transform = Transform::from_matrix(reflect * main_transform.to_matrix());
    // Bevy's mirror example uses the original view basis for this vector. The negative plane
    // normal retains the half-space above the water and the signed distance moves it to this plane.
    let view_from_world = main_transform.compute_affine().matrix3.inverse();
    let near_normal = (view_from_world * Vec3::NEG_Y).normalize();
    let projection = PerspectiveProjection {
        near_clip_plane: near_normal.extend(plane_y - main_transform.translation.y),
        ..main_projection.clone()
    };
    let clip_from_world = projection.get_clip_from_view() * transform.to_matrix().inverse();
    (transform, projection, clip_from_world)
}

/// Updates and, at the selected cadence, enables a single clipped mirror camera. The stored
/// matrix is committed only when that camera is enabled, so skipped frames keep sampling the
/// texture from the matching old viewpoint.
#[allow(clippy::too_many_arguments)]
fn update_water_reflection(
    mut commands: Commands,
    mut reflection: ResMut<WaterReflection>,
    mut images: ResMut<Assets<Image>>,
    terrain: Res<crate::terrain::Terrain>,
    water: Res<crate::water::WaterBodies>,
    ocean: Res<crate::water::WaterStyle>,
    time: Res<Time>,
    main: Query<
        (
            &Camera,
            &Transform,
            &Projection,
            &Exposure,
            Option<&AtmosphereSettings>,
            Option<&DistanceFog>,
            Option<&bevy::light::EnvironmentMapLight>,
        ),
        (With<MainCamera>, Without<WaterReflectionCamera>),
    >,
    mut capture: Query<
        (
            &mut Camera,
            &mut Transform,
            &mut Projection,
            &mut Exposure,
            &mut RenderTarget,
        ),
        (With<WaterReflectionCamera>, Without<MainCamera>),
    >,
) {
    let Ok((
        main_camera,
        main_transform,
        Projection::Perspective(main_projection),
        main_exposure,
        atmosphere,
        fog,
        environment,
    )) = main.single()
    else {
        return;
    };
    reflection.frame = reflection.frame.wrapping_add(1);
    let Some((plane_y, quality)) = reflection_plane(
        main_transform,
        main_projection,
        &terrain,
        &water,
        &ocean,
        time.elapsed_secs(),
        reflection.valid.then_some(reflection.plane_y),
    ) else {
        reflection.valid = false;
        if let Some(entity) = reflection.camera
            && let Ok((mut camera, ..)) = capture.get_mut(entity)
        {
            camera.is_active = false;
        }
        return;
    };
    // Low has always meant environment-only reflection (and disables the screen-space march).
    // Keep it a true inexpensive fallback for comparison and slower GPUs.
    if quality == crate::water::ReflectionQuality::Low {
        reflection.valid = false;
        if let Some(entity) = reflection.camera
            && let Ok((mut camera, ..)) = capture.get_mut(entity)
        {
            camera.is_active = false;
        }
        return;
    }
    let (scale, cadence) = capture_scale(quality);
    let Some(main_size) = main_camera.physical_target_size() else {
        return;
    };
    let size = reflection_size(main_size, scale);
    let plane_changed = !reflection.valid || (reflection.plane_y - plane_y).abs() > 0.01;
    let resized = size != reflection.size;
    if resized {
        reflection.image = images.add(reflection_image(size));
        reflection.size = size;
        reflection.valid = false;
    }
    let render_now = plane_changed || resized || reflection.frame % cadence == 0;

    let entity = match reflection.camera {
        Some(entity) => entity,
        None => {
            let mut entity = commands.spawn((
                Name::new("Water planar reflection"),
                WaterReflectionCamera,
                Camera3d::default(),
                Camera {
                    order: -1,
                    is_active: false,
                    clear_color: ClearColorConfig::Default,
                    invert_culling: REFLECTION_INVERT_CULLING,
                    ..default()
                },
                RenderTarget::Image(reflection.image.clone().into()),
                Hdr,
                Tonemapping::None,
                DebandDither::Disabled,
                REFLECTION_MSAA,
                REFLECTION_SHADOW_FILTER,
                RenderLayers::from_layers(&[0, REFLECTION_CLOUD_RENDER_LAYER]),
            ));
            if let Some(atmosphere) = atmosphere {
                entity.insert(atmosphere.clone());
            }
            if let Some(fog) = fog {
                entity.insert(fog.clone());
            }
            // `AtmosphereEnvironmentMapLight` generated this on the main camera already. Sharing
            // the filtered handles lights captured PBR meshes identically without another cube
            // generation job.
            if let Some(environment) = environment {
                entity.insert(environment.clone());
            }
            let id = entity.id();
            reflection.camera = Some(id);
            id
        }
    };

    // A newly spawned capture camera will start on the next frame. Its first sample stays invalid
    // until this same setup runs with a live camera entity.
    let Ok((
        mut capture_camera,
        mut capture_transform,
        mut capture_projection,
        mut capture_exposure,
        mut capture_target,
    )) = capture.get_mut(entity)
    else {
        reflection.valid = false;
        return;
    };
    if !render_now {
        capture_camera.is_active = false;
        return;
    }

    // These are view components, not global scene state. Keep the capture in lockstep with the
    // main camera whenever it renders so changing weather or the generated IBL intensity cannot
    // leave a newly captured reflection lit with stale fog or ambient light.
    let mut capture_entity = commands.entity(entity);
    if let Some(atmosphere) = atmosphere {
        capture_entity.insert(atmosphere.clone());
    }
    if let Some(fog) = fog {
        capture_entity.insert(fog.clone());
    }
    if let Some(environment) = environment {
        capture_entity.insert(environment.clone());
    }

    let (transform, projection, clip_from_world) =
        mirrored_camera(main_transform, main_projection, plane_y);

    *capture_target = RenderTarget::Image(reflection.image.clone().into());
    capture_camera.is_active = true;
    *capture_transform = transform;
    *capture_projection = Projection::Perspective(projection);
    *capture_exposure = *main_exposure;
    reflection.clip_from_world = clip_from_world;
    reflection.plane_y = plane_y;
    reflection.exposure_ev100 = main_exposure.ev100;
    reflection.valid = true;
}

fn copy_scene_depth(
    view: ViewQuery<(&ViewDepthStencilTexture, &WaterDepthTarget, &ExtractedView)>,
    images: Res<RenderAssets<GpuImage>>,
    transmissive: Res<ViewSortedRenderPhases<Transmissive3d>>,
    mut ctx: RenderContext,
) {
    let (depth, target, extracted) = view.into_inner();
    // (no water drawn in this view: nothing reads the copy)
    if transmissive
        .get(&extracted.retained_view_entity)
        .is_none_or(|p| p.items.is_empty())
    {
        return;
    }
    let Some(dst) = images.get(&target.0) else {
        return;
    };
    let src = depth.texture();
    // (a stale copy target of another size waits for the resized one)
    if src.size() != dst.texture.size()
        || src.format() != dst.texture.format()
        || src.sample_count() != 1
    {
        return;
    }
    ctx.command_encoder().copy_texture_to_texture(
        src.as_image_copy(),
        dst.texture.as_image_copy(),
        src.size(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_reflection_target_preserves_aspect_and_caps_width() {
        assert_eq!(
            reflection_size(UVec2::new(1920, 1080), 0.5),
            UVec2::new(960, 540)
        );
        assert_eq!(
            reflection_size(UVec2::new(3840, 2160), 0.5),
            UVec2::new(960, 540)
        );
        assert_eq!(
            reflection_size(UVec2::new(1280, 720), 0.25),
            UVec2::new(320, 180)
        );
    }

    #[test]
    fn mirror_camera_reflects_eye_and_clips_at_water_plane() {
        let main =
            Transform::from_xyz(8.0, 14.0, -3.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y);
        let projection = PerspectiveProjection {
            near: 0.3,
            far: 80_000.0,
            ..default()
        };
        let (mirrored, clipped, clip_from_world) = mirrored_camera(&main, &projection, 2.0);
        assert!((mirrored.translation.y + 10.0).abs() < 1e-4);
        assert!((clipped.near_clip_plane.w + 12.0).abs() < 1e-4);
        assert!((clipped.near_clip_plane.xyz().length() - 1.0).abs() < 1e-4);
        let above = clip_from_world * Vec3::new(0.0, 2.1, 0.0).extend(1.0);
        let below = clip_from_world * Vec3::new(0.0, 1.9, 0.0).extend(1.0);
        assert!(
            above.z >= 0.0 && above.z <= above.w,
            "objects above water must survive the oblique clip"
        );
        assert!(
            below.z < 0.0 || below.z > below.w,
            "objects below water must be clipped"
        );
    }

    #[test]
    fn lower_frustum_rays_find_water_below_a_level_camera() {
        let camera = Transform::IDENTITY;
        let projection = PerspectiveProjection {
            fov: 60f32.to_radians(),
            aspect_ratio: 16.0 / 9.0,
            ..default()
        };
        let rays = reflection_rays(&camera, &projection);
        assert!(rays[0].y.abs() < 1e-5, "the centre ray is level");
        assert!(
            rays[1..].iter().all(|ray| ray.y < -0.2),
            "lower-screen rays must hit water below a level horizon"
        );
    }
}
