//! Editor camera: orbit / pan / zoom-to-cursor (mouse + trackpad), WASD fly, and the
//! cursor → terrain pointer used by every brush.

use bevy::anti_alias::smaa::Smaa;
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::camera::Exposure;
use bevy::camera::Hdr;
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::gestures::PinchGesture;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::light::atmosphere::ScatteringMedium;
use bevy::light::{Atmosphere, ShadowFilteringMethod};
use bevy::pbr::{
    AtmosphereSettings, DistanceFog, FogFalloff, ScreenSpaceAmbientOcclusion,
    ScreenSpaceAmbientOcclusionQualityLevel,
};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_egui::input::EguiWantsInput;
use serde::{Deserialize, Serialize};

use crate::terrain::Terrain;

#[derive(Component)]
pub struct MainCamera;

#[derive(Resource, Clone)]
pub struct OrbitCamera {
    pub focus: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    target_focus: Vec3,
    target_yaw: f32,
    target_pitch: f32,
    target_distance: f32,
    /// Disabled while walking.
    pub enabled: bool,
    /// The focus is on a cave's floor: where it last stood there (moves keep it in the cave's
    /// space; `set_view` and F jump anywhere).
    cave: Option<Vec3>,
    /// In a cave the camera turns round a point this high over the floor (eased in and out), so
    /// it can look up at the roof and along the walls.
    lift: f32,
    /// How far the camera could be from that point before rock is in the way (last frame).
    arm: Option<f32>,
    /// `snap` was called: the lift and the arm jump too (next frame)
    snapped: bool,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        let focus = Vec3::new(0.0, 0.0, 300.0);
        Self {
            focus,
            yaw: 0.35,
            pitch: 0.62,
            distance: 3300.0,
            target_focus: focus,
            target_yaw: 0.35,
            target_pitch: 0.62,
            target_distance: 3300.0,
            enabled: true,
            cave: None,
            lift: 0.0,
            arm: None,
            snapped: false,
        }
    }
}

impl OrbitCamera {
    pub fn set_view(&mut self, focus: Vec3, yaw: f32, pitch: f32, distance: f32) {
        self.target_focus = focus;
        self.target_yaw = yaw;
        self.target_pitch = pitch;
        self.target_distance = distance;
        self.cave = None;
    }
    pub fn snap(&mut self) {
        self.focus = self.target_focus;
        self.yaw = self.target_yaw;
        self.pitch = self.target_pitch;
        self.distance = self.target_distance;
        self.snapped = true;
    }
    /// The focus is on a cave's floor (the camera keeps inside the cave's space).
    pub fn in_cave(&self) -> bool {
        self.cave.is_some()
    }
    /// The point the camera turns round and looks at.
    pub fn pivot(&self) -> Vec3 {
        self.focus + Vec3::Y * self.lift
    }
    fn offset(yaw: f32, pitch: f32, distance: f32) -> Vec3 {
        Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        ) * distance
    }
    /// Where the eye sits relative to the focus for these angles.
    pub fn offset_of(yaw: f32, pitch: f32, distance: f32) -> Vec3 {
        Self::offset(yaw, pitch, distance)
    }
    /// Where the camera is (in a cave: in front of the rock behind it).
    pub fn eye(&self) -> Vec3 {
        self.pivot()
            + Self::offset(
                self.yaw,
                self.pitch,
                self.distance.min(self.arm.unwrap_or(f32::MAX)),
            )
    }
}

/// How high over a cave's floor the camera turns (about eye height).
pub const CAVE_LIFT: f32 = 1.6;
/// The lowest pitch: just over the ground outdoors; in a cave the camera can look up from low down.
const MIN_PITCH: f32 = 0.03;
const MIN_PITCH_CAVE: f32 = -1.0;

/// Where the mouse cursor hits the terrain (or a cave's rock) this frame.
#[derive(Resource, Default, Clone)]
pub struct Pointer {
    pub screen: Option<Vec2>,
    pub ray: Option<Ray3d>,
    pub hit: Option<Vec3>,
    /// The hit is on a cave's rock (floor, wall or roof): the rock's normal there.
    pub cave_normal: Option<Vec3>,
    /// True when the cursor is over an egui panel (tools must not react).
    pub over_ui: bool,
    pub keyboard_captured: bool,
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::render_scale::RenderScalePlugin)
            .init_resource::<OrbitCamera>()
            .init_resource::<Pointer>()
            .init_resource::<WorkLight>()
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (prepass_switch, contact_shadows))
            .add_systems(
                PostUpdate,
                image_quality.before(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(
                PreUpdate,
                update_pointer.after(bevy_egui::EguiPreUpdateSet::ProcessInput),
            )
            .add_systems(
                Update,
                (orbit_input, apply_orbit, work_light)
                    .chain()
                    .in_set(CameraSystems),
            );
    }
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraSystems;

fn spawn_camera(
    mut commands: Commands,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    orbit: Res<OrbitCamera>,
) {
    let medium = mediums.add(ScatteringMedium::earth(256, 256));
    commands.spawn((Name::new("Atmosphere"), Atmosphere::earth(medium)));

    let camera = commands
        .spawn((
            Name::new("Main Camera"),
            MainCamera,
            // the art-style pass reads the main depth buffer (sky mask, distance haze, outlines);
            // the water copies it before it is drawn (what lies behind the surface)
            Camera3d {
                depth_texture_usages:
                    (bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT
                        | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING
                        | bevy::render::render_resource::TextureUsages::COPY_SRC)
                        .into(),
                ..default()
            },
            Hdr,
            Projection::Perspective(PerspectiveProjection {
                fov: 50f32.to_radians(),
                near: 0.3,
                far: 80_000.0,
                ..default()
            }),
            Transform::from_translation(orbit.eye()).looking_at(orbit.focus, Vec3::Y),
            AtmosphereSettings {
                aerial_view_lut_max_distance: 4.0e4,
                ..default()
            },
            // ambient / reflection lighting from the sky: a small cubemap is plenty for diffuse IBL
            // a little above physical so shaded valleys stay readable (artistic choice)
            bevy::light::AtmosphereEnvironmentMapLight {
                intensity: 1.7,
                size: UVec2::splat(128),
                ..default()
            },
            Exposure { ev100: 13.0 },
            Tonemapping::AgX,
            Bloom::NATURAL,
            DistanceFog {
                color: Color::srgba(0.7, 0.75, 0.82, 0.0),
                falloff: FogFalloff::ExponentialSquared { density: 0.0 },
                ..default()
            },
            Msaa::Off,
            Smaa::default(),
            (
                ShadowFilteringMethod::Gaussian,
                // point and spot lights pick their shadow detail by the distance to this view (not
                // to the camera that presents it: `render_scale`)
                bevy::camera::ShadowLodOrigin,
            ),
        ))
        .id();
    // The water and cloud dome live on private layers so the planar water camera can exclude
    // both, while the editor view still receives the complete scene.
    commands.entity(camera).insert(RenderLayers::from_layers(&[
        0,
        crate::water_render::WATER_RENDER_LAYER,
        crate::water_render::CLOUD_RENDER_LAYER,
    ]));
}

/// The main camera draws depth first (`DepthPrepass`): alpha-tested leaves of a forest then run
/// the full lighting once per pixel instead of once per overlapping leaf (about twice the frame
/// rate in dense woods). Materials that move vertices have matching prepass shaders or stay out
/// of it. `WB_NOPREPASS=1` turns it off (comparisons).
fn prepass_switch(mut commands: Commands, cams: Query<Entity, Added<MainCamera>>) {
    if std::env::var("WB_NOPREPASS").is_err() {
        for e in &cams {
            commands
                .entity(e)
                .insert(bevy::core_pipeline::prepass::DepthPrepass);
        }
    }
}

/// Lighting › "Contact shadows (fine detail)": Bevy's screen-space contact shadows are a camera
/// component (the sun's `contact_shadows_enabled` alone does nothing). They march the depth
/// buffer for every lit pixel, so they cost frame rate in proportion to the window's pixels.
fn contact_shadows(
    mut commands: Commands,
    lighting: Res<crate::sky::Lighting>,
    cams: Query<(Entity, Has<bevy::pbr::ContactShadows>), With<MainCamera>>,
) {
    for (e, has) in &cams {
        if lighting.contact_shadows && !has {
            commands
                .entity(e)
                .insert(bevy::pbr::ContactShadows::default());
        } else if !lighting.contact_shadows && has {
            commands.entity(e).remove::<bevy::pbr::ContactShadows>();
        }
    }
}

/// Lighting › Shadows › Ambient occlusion: screen-space ambient occlusion (Bevy's GTAO) darkens
/// creases and the ground where things stand on it, so objects sit in the world instead of
/// floating on it. It costs frame rate in proportion to the window's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AmbientOcclusion {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl AmbientOcclusion {
    pub const ALL: [AmbientOcclusion; 4] = [
        AmbientOcclusion::Off,
        AmbientOcclusion::Low,
        AmbientOcclusion::Medium,
        AmbientOcclusion::High,
    ];
    pub fn name(self) -> &'static str {
        match self {
            AmbientOcclusion::Off => "Off",
            AmbientOcclusion::Low => "Low",
            AmbientOcclusion::Medium => "Medium",
            AmbientOcclusion::High => "High",
        }
    }
    fn level(self) -> Option<ScreenSpaceAmbientOcclusionQualityLevel> {
        match self {
            AmbientOcclusion::Off => None,
            AmbientOcclusion::Low => Some(ScreenSpaceAmbientOcclusionQualityLevel::Low),
            AmbientOcclusion::Medium => Some(ScreenSpaceAmbientOcclusionQualityLevel::Medium),
            AmbientOcclusion::High => Some(ScreenSpaceAmbientOcclusionQualityLevel::High),
        }
    }
}

/// Lighting › Performance › Anti-aliasing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AntiAliasing {
    /// SMAA: smooths edges within one frame; crisp, but leaves, grass and thin lines shimmer as
    /// the camera moves.
    Smaa,
    /// Temporal: blends jittered frames along each pixel's motion, so foliage and thin lines hold
    /// still and the ambient occlusion's noise resolves; a touch softer (the look's sharpening
    /// makes up for it).
    #[default]
    Taa,
}

impl AntiAliasing {
    pub const ALL: [AntiAliasing; 2] = [AntiAliasing::Smaa, AntiAliasing::Taa];
    pub fn name(self) -> &'static str {
        match self {
            AntiAliasing::Smaa => "Sharp (SMAA)",
            AntiAliasing::Taa => "Smooth (TAA)",
        }
    }
}

/// Sample distance of the ambient occlusion at the style's standard strength (m): wide enough to shade the ground round a house
/// or a rock, not so wide that it rings everything in dark halos.
const AO_RADIUS: f32 = 1.5;
/// How thick things are assumed to be behind what the depth buffer sees (m).
const AO_THICKNESS: f32 = 0.5;

/// Applies Lighting › Ambient occlusion and Anti-aliasing to the main camera. AO needs the normal
/// prepass; TAA needs the motion-vector prepass (materials that move their vertices write their
/// last frame's positions there: foliage, cloth, fish, grass) and replaces SMAA. A camera cut (a
/// jump rather than a glide: world load, teleport, a preset view) drops the TAA history, which
/// would otherwise ghost the old view over the new one for a few frames.
fn image_quality(
    mut commands: Commands,
    lighting: Res<crate::sky::Lighting>,
    look: Res<crate::look::LookState>,
    orbit: Res<OrbitCamera>,
    mut cams: Query<
        (
            Entity,
            &Transform,
            Option<&mut ScreenSpaceAmbientOcclusion>,
            Option<&mut TemporalAntiAliasing>,
            Has<bevy::core_pipeline::prepass::NormalPrepass>,
            Has<bevy::core_pipeline::prepass::DepthPrepass>,
        ),
        With<MainCamera>,
    >,
    mut last: Local<Option<Transform>>,
) {
    // the style's strength scales how far the shade reaches (0: none)
    let amount = look.params.occlusion.clamp(0.0, 3.0);
    let level = lighting.ambient_occlusion.level().filter(|_| amount > 0.02);
    let (radius, thickness) = (AO_RADIUS * amount, AO_THICKNESS * amount.max(0.5));
    for (e, tf, ao, taa, normals, depth) in &mut cams {
        // Both AO and the optional forest mask share this attachment. Turning off either
        // consumer must not remove the other one's normals.
        let needs_normals = level.is_some()
            || (lighting.forest_contours.enabled && lighting.forest_contours.strength > 0.001);
        if lighting.forest_contours.enabled && lighting.forest_contours.strength > 0.001 && !depth {
            commands
                .entity(e)
                .insert(bevy::core_pipeline::prepass::DepthPrepass);
        }
        if needs_normals && !normals {
            commands
                .entity(e)
                .insert(bevy::core_pipeline::prepass::NormalPrepass);
        } else if !needs_normals && normals {
            commands
                .entity(e)
                .remove::<bevy::core_pipeline::prepass::NormalPrepass>();
        }
        match (level, ao) {
            (Some(q), Some(mut ao)) => {
                if ao.quality_level != q
                    || (ao.radius - radius).abs() > 1e-3
                    || (ao.constant_object_thickness - thickness).abs() > 1e-3
                {
                    ao.quality_level = q;
                    ao.radius = radius;
                    ao.constant_object_thickness = thickness;
                }
            }
            (Some(q), None) => {
                commands.entity(e).insert(ScreenSpaceAmbientOcclusion {
                    quality_level: q,
                    radius,
                    constant_object_thickness: thickness,
                });
            }
            (None, Some(_)) => {
                commands.entity(e).remove::<ScreenSpaceAmbientOcclusion>();
            }
            (None, None) => {}
        }
        match (lighting.anti_aliasing, taa) {
            (AntiAliasing::Taa, Some(mut taa)) => {
                let cut = last.is_some_and(|l| {
                    l.translation.distance(tf.translation) > (orbit.distance * 0.25).max(4.0)
                        || l.rotation.angle_between(tf.rotation) > 0.35
                });
                if cut {
                    taa.reset = true;
                }
            }
            (AntiAliasing::Taa, None) => {
                commands
                    .entity(e)
                    .remove::<Smaa>()
                    .insert(TemporalAntiAliasing::default());
            }
            (AntiAliasing::Smaa, Some(_)) => {
                commands
                    .entity(e)
                    .remove::<(
                        TemporalAntiAliasing,
                        bevy::render::camera::TemporalJitter,
                        bevy::render::camera::MipBias,
                        bevy::core_pipeline::prepass::MotionVectorPrepass,
                    )>()
                    .insert(Smaa::default());
            }
            (AntiAliasing::Smaa, None) => {}
        }
        *last = Some(*tf);
    }
}

fn update_pointer(
    mut pointer: ResMut<Pointer>,
    window: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    terrain: Option<Res<Terrain>>,
    egui: Option<Res<EguiWantsInput>>,
    caves: Option<Res<crate::caves::render::CaveWorld>>,
    holes: Option<Res<crate::terrain::holes::TerrainHoles>>,
) {
    pointer.over_ui = egui
        .as_ref()
        .map(|e| e.is_pointer_over_area() || e.is_using_pointer())
        .unwrap_or(false);
    pointer.keyboard_captured = egui
        .as_ref()
        .map(|e| e.wants_keyboard_input())
        .unwrap_or(false);
    pointer.screen = None;
    pointer.ray = None;
    pointer.hit = None;
    pointer.cave_normal = None;
    let (Ok(window), Ok((cam, cam_t))) = (window.single(), camera.single()) else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    pointer.screen = Some(cursor);
    let Ok(ray) = cam.viewport_to_world(cam_t, cursor) else {
        return;
    };
    pointer.ray = Some(ray);
    let Some(t) = terrain else { return };
    if let Some((p, n)) = pick(&t, caves.as_deref(), holes.as_deref(), ray) {
        pointer.hit = Some(p);
        pointer.cave_normal = n;
    }
}

/// What a ray from the view meets, the way the cursor does: the terrain, and the caves' rock and
/// the ground in them (`CaveWorld::pick`). (the point, and the rock's normal when it is cave rock)
pub fn pick(
    t: &Terrain,
    caves: Option<&crate::caves::render::CaveWorld>,
    holes: Option<&crate::terrain::holes::TerrainHoles>,
    ray: Ray3d,
) -> Option<(Vec3, Option<Vec3>)> {
    match (caves, holes) {
        (Some(c), Some(h)) if c.volume.is_some() || c.dungeon.is_some() => {
            c.pick(t, h, ray.origin, *ray.direction, 60_000.0)
        }
        _ => t
            .raycast(ray.origin, *ray.direction, 60_000.0)
            .map(|p| (p, None)),
    }
}

#[allow(clippy::too_many_arguments)]
fn orbit_input(
    mut orbit: ResMut<OrbitCamera>,
    pointer: Res<Pointer>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut pinch: MessageReader<PinchGesture>,
    time: Res<Time>,
    mut drag: Local<Option<MouseButton>>,
    terrain: Option<Res<Terrain>>,
    caves: Option<Res<crate::caves::render::CaveWorld>>,
) {
    if !orbit.enabled {
        pinch.clear();
        return;
    }
    let dt = time.delta_secs();
    let in_cave = orbit.in_cave();
    let low = if in_cave { MIN_PITCH_CAVE } else { MIN_PITCH };
    // (in a cave: how far the camera really is, in front of the rock)
    let near = orbit
        .target_distance
        .min(orbit.arm.filter(|_| in_cave).unwrap_or(f32::MAX));
    // A drag that started in the viewport keeps working even if the cursor crosses a panel.
    for b in [MouseButton::Right, MouseButton::Middle, MouseButton::Left] {
        if buttons.just_pressed(b) && !pointer.over_ui && drag.is_none() {
            let space_pan = b == MouseButton::Left && keys.pressed(KeyCode::Space);
            if b != MouseButton::Left || space_pan {
                *drag = Some(b);
            }
        }
    }
    if let Some(b) = *drag
        && !buttons.pressed(b)
    {
        *drag = None;
    }

    let d = motion.delta;
    match *drag {
        Some(MouseButton::Right) => {
            orbit.target_yaw -= d.x * 0.0055;
            orbit.target_pitch = (orbit.target_pitch + d.y * 0.0045).clamp(low, 1.52);
        }
        Some(MouseButton::Middle) | Some(MouseButton::Left) => {
            let k = near * 0.0016;
            let (s, c) = orbit.yaw.sin_cos();
            let right = Vec3::new(c, 0.0, -s);
            let fwd = Vec3::new(-s, 0.0, -c);
            orbit.target_focus += (-right * d.x + fwd * d.y) * k;
        }
        _ => {}
    }

    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    // zoom (shift+scroll is reserved for brush size)
    if !pointer.over_ui && !shift {
        let (mut zoom, yaw) = match scroll.unit {
            MouseScrollUnit::Line => (scroll.delta.y * 0.14, 0.0),
            MouseScrollUnit::Pixel => (scroll.delta.y * 0.0045, scroll.delta.x * 0.004),
        };
        for p in pinch.read() {
            zoom += p.0 * 2.2;
        }
        if alt || ctrl {
            // trackpad orbit
            orbit.target_pitch = (orbit.target_pitch - zoom * 0.6).clamp(low, 1.52);
            orbit.target_yaw += yaw;
        } else if zoom != 0.0 || yaw != 0.0 {
            // (in a cave from where the rock holds the camera, so zooming back in starts at once)
            let old = if in_cave && zoom < 0.0 {
                near
            } else {
                orbit.target_distance
            };
            // big worlds can be seen whole
            let far = terrain
                .as_ref()
                .map_or(14_000.0, |t| (t.size() * 1.3).max(14_000.0));
            let new = (old * (-zoom).exp()).clamp(if in_cave { 1.5 } else { 4.0 }, far);
            orbit.target_distance = new;
            orbit.target_yaw += yaw;
            // zoom towards the point under the cursor
            if let Some(hit) = pointer.hit
                && zoom > 0.0
            {
                let f = 1.0 - new / old;
                let tf = orbit.target_focus;
                orbit.target_focus = tf + (hit - tf) * f;
            }
        }
    } else {
        pinch.clear();
    }

    if !pointer.keyboard_captured {
        let (s, c) = orbit.yaw.sin_cos();
        let right = Vec3::new(c, 0.0, -s);
        let fwd = Vec3::new(-s, 0.0, -c);
        let mut mv = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
            mv += fwd;
        }
        if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
            mv -= fwd;
        }
        if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
            mv -= right;
        }
        if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
            mv += right;
        }
        if ctrl || keys.pressed(KeyCode::SuperLeft) {
            mv = Vec3::ZERO; // ctrl/cmd + S etc. are shortcuts
        }
        // (in a cave at a walking pace: chambers are small)
        let base = if in_cave {
            near.clamp(4.0, 30.0)
        } else {
            orbit.target_distance.max(30.0)
        };
        let speed = base * if shift { 2.4 } else { 0.8 };
        orbit.target_focus += mv * speed * dt;
        if keys.pressed(KeyCode::KeyQ) {
            orbit.target_yaw += 1.4 * dt;
        }
        if keys.pressed(KeyCode::KeyE) {
            orbit.target_yaw -= 1.4 * dt;
        }
        // (⌘/Ctrl+F is find)
        if keys.just_pressed(KeyCode::KeyF)
            && !ctrl
            && !keys.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight])
            && let Some(hit) = pointer.hit
        {
            // a cave's wall or roof: the floor in front of it
            let floor = pointer.cave_normal.zip(caves.as_ref()).and_then(|(n, c)| {
                let q = hit + n * 1.0;
                c.floor_under(q, 40.0).map(|y| Vec3::new(q.x, y, q.z))
            });
            orbit.target_focus = floor.unwrap_or(hit);
            orbit.target_distance =
                orbit
                    .target_distance
                    .min(if floor.is_some() { 10.0 } else { 400.0 });
            if floor.is_none() {
                orbit.target_pitch = orbit.target_pitch.max(MIN_PITCH);
            }
            orbit.cave = None;
        }
    }
}

fn apply_orbit(
    mut orbit: ResMut<OrbitCamera>,
    terrain: Option<Res<Terrain>>,
    time: Res<Time>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
    caves: Option<Res<crate::caves::render::CaveWorld>>,
) {
    if !orbit.enabled {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let k = 1.0 - (-dt * 14.0).exp();
    // a dungeon's underground map: the focus rides the floor of the level being edited (the
    // hidden ground far overhead is no ground) and the view looks in through the cut-away
    if let Some(d) = caves
        .as_deref()
        .and_then(|c| c.dungeon.as_ref())
        .filter(|d| d.interior)
    {
        let tf = orbit.target_focus;
        if let Some(f) = d.floor_under(Vec3::new(tf.x, tf.y + 2.0, tf.z), 8.0) {
            orbit.target_focus.y = f;
        }
        orbit.cave = None;
        orbit.arm = None;
        let o = orbit.clone();
        orbit.focus = o.focus.lerp(o.target_focus, k);
        orbit.yaw = o.yaw + (o.target_yaw - o.yaw) * k;
        orbit.pitch = o.pitch + (o.target_pitch - o.pitch) * k;
        orbit.distance = o.distance + (o.target_distance - o.distance) * k;
        orbit.lift = 0.0;
        orbit.snapped = false;
        let pivot = orbit.pivot();
        let eye = pivot + OrbitCamera::offset(orbit.yaw, orbit.pitch, 1.0) * orbit.distance;
        if let Ok(mut tf) = cam.single_mut() {
            *tf = Transform::from_translation(eye).looking_at(pivot, Vec3::Y);
        }
        return;
    }
    let caves = caves.as_deref().filter(|c| c.volume.is_some());
    let was_in_cave = orbit.cave.is_some();
    // keep the focus on the ground (or water surface) and inside the world
    if let Some(t) = &terrain {
        let lim = t.half() * 1.3;
        orbit.target_focus.x = orbit.target_focus.x.clamp(-lim, lim);
        orbit.target_focus.z = orbit.target_focus.z.clamp(-lim, lim);
        let ground = t
            .height_at(orbit.target_focus.x, orbit.target_focus.z)
            .max(t.sea_level);
        // (a focus in a cave stays on the cave's floor)
        let tf = orbit.target_focus;
        let floor_at = |p: Vec3, y: f32, reach: f32| {
            caves.and_then(|c| c.floor_under(Vec3::new(p.x, y + 1.2, p.z), reach))
        };
        let at = match orbit.cave {
            // (off a ledge: down to the floor below)
            Some(last) => match floor_at(tf, last.y, 30.0) {
                Some(f) => Some(Vec3::new(tf.x, f, tf.z)),
                // out of a mouth onto ground about as high as the floor: out of the cave
                None if t.height_at(tf.x, tf.z) < last.y + 2.0 => None,
                // into the rock: slid along the wall, else stopped at it
                None => keep_in_cave(last, tf, &|p, y| floor_at(p, y, 30.0)),
            },
            None => floor_at(tf, tf.y, 6.0).map(|f| Vec3::new(tf.x, f, tf.z)),
        };
        orbit.cave = at;
        orbit.target_focus = at.unwrap_or(Vec3::new(tf.x, ground, tf.z));
    }
    let in_cave = orbit.cave.is_some();
    if was_in_cave && !in_cave {
        // (out of a cave: no more looking up from under the floor)
        orbit.target_pitch = orbit.target_pitch.max(MIN_PITCH);
    }
    let o = orbit.clone();
    orbit.focus = o.focus.lerp(o.target_focus, k);
    orbit.yaw = o.yaw + (o.target_yaw - o.yaw) * k;
    orbit.pitch = o.pitch + (o.target_pitch - o.pitch) * k;
    orbit.distance = o.distance + (o.target_distance - o.distance) * k;
    // in a cave the camera turns round a point over the floor
    let lift = if in_cave { CAVE_LIFT } else { 0.0 };
    orbit.lift = if o.snapped {
        lift
    } else {
        o.lift + (lift - o.lift) * (1.0 - (-dt * 6.0).exp())
    };

    // in a cave the camera stays in front of the rock behind it (walls, the roof), never in it
    let pivot = orbit.pivot();
    let dir = OrbitCamera::offset(orbit.yaw, orbit.pitch, 1.0);
    let mut dist = orbit.distance;
    match caves.filter(|_| in_cave) {
        Some(c) => {
            let free = c
                .raycast(pivot, dir, dist + 1.0)
                .map_or(dist + 1.0, |(_, t)| t);
            let mut room = (free - 0.45).max(0.25);
            for _ in 0..8 {
                if room <= 0.45 || c.field(pivot + dir * room).is_none_or(|d| d >= 0.3) {
                    break;
                }
                room -= 0.2;
            }
            // (pulled in at once, let out gently)
            let arm = match o.arm {
                Some(a) if !o.snapped && room > a => a + (room - a) * (1.0 - (-dt * 5.0).exp()),
                _ => room,
            };
            orbit.arm = Some(arm);
            dist = dist.min(arm);
        }
        None => orbit.arm = None,
    }
    orbit.snapped = false;

    let mut eye = pivot + dir * dist;
    // (in a cave its rock and the ground round it hold the camera: never up onto the hillside)
    let held = caves.filter(|_| in_cave).and_then(|c| c.field(eye));
    if held.is_some_and(|d| d <= 0.0) {
        eye = pivot + dir * 0.25;
    }
    if let Some(t) = &terrain
        && held.is_none()
        && !caves.is_some_and(|c| c.in_space(eye))
    {
        let ground = t.height_at(eye.x, eye.z).max(t.sea_level);
        if eye.y < ground + 2.0 {
            eye.y = ground + 2.0;
        }
    }
    if let Ok(mut tf) = cam.single_mut() {
        *tf = Transform::from_translation(eye).looking_at(pivot, Vec3::Y);
    }
}

/// The editor's work light (B, the cave bar's Light button, MCP `camera_work_light`): while
/// editing, a lamp over where the camera looks and a softer one behind the camera, so dark caves
/// and night scenes can be seen to decorate. Off in walk mode (the real darkness shows there);
/// never saved or exported.
///
/// Neither casts shadows, so in a cave both sit in the rock — the lamp a few metres over the roof
/// (light falls on floors and on what stands there), the other behind the camera's wall — where
/// nothing comes close enough to them to burn white.
#[derive(Resource, Default)]
pub struct WorkLight {
    pub on: bool,
    lamps: Option<[Entity; 2]>,
}

/// Lux at the focus from the lamp over it and from the one behind the camera (a cave's adjusted
/// exposure reads about 1500 lux as mid grey).
const WORK_LUX: f32 = 1300.0;
const FILL_LUX: f32 = 600.0;

#[allow(clippy::too_many_arguments)]
fn work_light(
    mut commands: Commands,
    mut wl: ResMut<WorkLight>,
    orbit: Res<OrbitCamera>,
    walk: Res<crate::walk::WalkState>,
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<Pointer>,
    mut ed: ResMut<crate::editor::Editor>,
    terrain: Option<Res<Terrain>>,
    caves: Option<Res<crate::caves::render::CaveWorld>>,
    mut lamps: Query<(&mut Transform, &mut PointLight)>,
) {
    let cmd = keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
    ]);
    if keys.just_pressed(KeyCode::KeyB) && !cmd && !pointer.keyboard_captured && !walk.active {
        wl.on = !wl.on;
        ed.toast(if wl.on {
            "Work light on (B): only while editing, Walk shows the real darkness"
        } else {
            "Work light off"
        });
    }
    if !wl.on || walk.active || walk.cinematic || !orbit.enabled {
        if let Some(l) = wl.lamps.take() {
            for e in l {
                commands.entity(e).despawn();
            }
        }
        return;
    }
    let [over, behind] = *wl.lamps.get_or_insert_with(|| {
        let mut lamp = |name: &'static str| {
            commands
                .spawn((
                    Name::new(name),
                    PointLight {
                        color: Color::srgb(1.0, 0.95, 0.88),
                        radius: 0.4,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::default(),
                ))
                .id()
        };
        [lamp("Work light"), lamp("Work light (camera)")]
    });
    let pivot = orbit.pivot();
    let eye = orbit.eye();
    let d = eye.distance(pivot).max(2.0);
    let cave = caves.as_ref().filter(|_| orbit.in_cave());
    // over the focus: in a cave a few metres into the rock over its roof (kept under the ground
    // outside), else well over the ground
    let floor = orbit.focus.y;
    let surface = terrain
        .as_ref()
        .map_or(f32::MAX, |t| t.height_at(pivot.x, pivot.z) - 0.5);
    let y = match cave.map(|c| c.roof_over(pivot, 60.0)) {
        Some(Some(roof)) => (roof + 3.0).min(surface).max(roof - 0.5),
        Some(None) => floor + 8.0,
        None => floor + 6.0 + d * 0.25,
    };
    let h = y - floor;
    // (the area it lights grows with the view)
    let r = d.max(8.0);
    // behind the camera, back as far again as the focus is in front of it
    let back = (eye - pivot).normalize_or(Vec3::Y);
    let k = d.max(4.0);
    let fill_at = eye + back * k + Vec3::Y * 1.0;
    let set = |p: Vec3, cd: f32, range: f32, l: &mut (Mut<Transform>, Mut<PointLight>)| {
        l.0.translation = p;
        l.1.intensity = cd * 4.0 * std::f32::consts::PI;
        l.1.range = range;
    };
    if let Ok(mut l) = lamps.get_mut(over) {
        set(
            Vec3::new(pivot.x, y, pivot.z),
            WORK_LUX * (h * h + 0.25 * r * r),
            (h * h + r * r).sqrt() * 2.0 + 10.0,
            &mut l,
        );
    }
    if let Ok(mut l) = lamps.get_mut(behind) {
        let to_focus = fill_at.distance(pivot);
        set(
            fill_at,
            FILL_LUX * to_focus * to_focus,
            to_focus * 2.0 + 10.0,
            &mut l,
        );
    }
}

/// A move of the focus from `last` (on a cave's floor) to `to` ran into the rock: where it ends
/// up instead, still on the floor — whichever goes furthest of sliding along the wall (just the
/// move's x or z) and going as far as the floor reaches towards `to`. None if even `last` is no
/// longer in the cave (it was rebuilt): the focus leaves it.
fn keep_in_cave(
    last: Vec3,
    to: Vec3,
    floor_at: &impl Fn(Vec3, f32) -> Option<f32>,
) -> Option<Vec3> {
    let on = |p: Vec3| floor_at(p, last.y).map(|f| Vec3::new(p.x, f, p.z));
    let (mut a, mut b) = (0.0f32, 1.0f32);
    for _ in 0..8 {
        let m = 0.5 * (a + b);
        if on(last.lerp(to, m)).is_some() {
            a = m;
        } else {
            b = m;
        }
    }
    [
        last.lerp(to, a),
        Vec3::new(to.x, last.y, last.z),
        Vec3::new(last.x, last.y, to.z),
    ]
    .into_iter()
    .filter_map(on)
    .max_by(|p, q| {
        p.xz()
            .distance_squared(last.xz())
            .total_cmp(&q.xz().distance_squared(last.xz()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room 10 m across with its floor at 0, a step down to −1 past x = 3.
    fn room(p: Vec3, _y: f32) -> Option<f32> {
        (p.x.abs() < 5.0 && p.z.abs() < 5.0).then_some(if p.x > 3.0 { -1.0 } else { 0.0 })
    }

    #[test]
    fn focus_slides_along_cave_walls() {
        // diagonally into the +x wall: slides along it
        let p = keep_in_cave(Vec3::new(4.9, -1.0, 0.0), Vec3::new(5.4, -1.0, 0.3), &room).unwrap();
        assert!(p.x < 5.0 && (p.z - 0.3).abs() < 1e-4 && p.y == -1.0, "{p}");
        // head on: up to the wall
        let p = keep_in_cave(Vec3::new(4.0, -1.0, 0.0), Vec3::new(6.0, -1.0, 0.0), &room).unwrap();
        assert!(p.x > 4.9 && p.x < 5.0 && p.z == 0.0, "{p}");
        // a long jump (zooming at a far wall) stops at the wall, taking the floor's height along
        let p = keep_in_cave(Vec3::new(-2.0, 0.0, 0.0), Vec3::new(9.0, 0.0, 1.0), &room).unwrap();
        assert!(p.x > 4.9 && p.x < 5.0 && p.y == -1.0, "{p}");
        // the cave was rebuilt and the focus's spot is rock now: it leaves the cave
        assert!(keep_in_cave(Vec3::ZERO, Vec3::X, &|_, _| None).is_none());
    }
}
