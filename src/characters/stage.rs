//! The Character view's stage: the selected character on a floor, lit on its own render layer
//! far below the world, shot by a camera into a texture the view shows. It exists only while
//! the view asks for it each frame (`Stage::want`); otherwise it's taken down.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::light::{Cascades, NotShadowCaster};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};

use super::Characters;
use super::puppet::{self, Puppet};

pub const LAYER: usize = 11;
/// Character portraits (the library grid) are shot on this layer, the stage's lights reach it.
pub const THUMB_LAYER: usize = 12;
const THUMB_ORIGIN: Vec3 = Vec3::new(60.0, -24_000.0, -6_000.0);
const THUMB_SIZE: u32 = 256;
/// Where the stage stands (world space), away from the world and the thumbnail studio.
pub const ORIGIN: Vec3 = Vec3::new(0.0, -24_000.0, -6_000.0);

#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub character: String,
    /// the picture's size in pixels
    pub size: UVec2,
}

#[derive(Clone, Copy, Debug)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    /// distance as a multiple of the character's height
    pub zoom: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            yaw: 0.45,
            pitch: 0.12,
            zoom: 1.0,
        }
    }
}

#[derive(Resource)]
pub struct Stage {
    /// set by the view every frame it shows the stage
    pub want: Option<Want>,
    pub orbit: Orbit,
    /// straight-on front view (marker editing)
    pub front: bool,
    // playback, applied to the puppet
    pub anim: Option<String>,
    pub playing: bool,
    pub speed: f32,
    pub in_place: bool,
    pub seek: Option<f32>,
    /// read back for the view
    pub time: f32,
    pub ready: bool,
    pub egui: Option<egui::TextureId>,
    /// model space → clip space of the stage camera (for marker overlays)
    pub clip_from_model: Mat4,
    pub height: f32,
    shown: Option<(String, String)>,
    puppet: Option<Entity>,
    camera: Option<Entity>,
    rig: Vec<Entity>,
    target: Option<(Handle<Image>, UVec2)>,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            want: None,
            orbit: Orbit::default(),
            front: false,
            anim: Some(super::anims::IDLE.into()),
            playing: true,
            speed: 1.0,
            in_place: true,
            seek: None,
            time: 0.0,
            ready: false,
            egui: None,
            clip_from_model: Mat4::IDENTITY,
            height: 1.8,
            shown: None,
            puppet: None,
            camera: None,
            rig: vec![],
            target: None,
        }
    }
}

impl Stage {
    /// A model-space point → (0..1, 0..1) across the picture, if in front of the camera.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let c = self.clip_from_model * p.extend(1.0);
        if c.w <= 1e-5 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(Vec2::new(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5))
    }
    /// The model-space point at (0..1, 0..1) on the picture, on the plane z = `z`.
    pub fn unproject(&self, uv: Vec2, z: f32) -> Option<Vec3> {
        let inv = self.clip_from_model.inverse();
        let ndc = Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
        let a = inv * Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
        let b = inv * Vec4::new(ndc.x, ndc.y, 0.1, 1.0);
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        let d = b - a;
        if d.z.abs() < 1e-6 {
            return None;
        }
        let t = (z - a.z) / d.z;
        Some(a + d * t)
    }
}

#[derive(Component)]
struct StageCamera;

pub struct StagePlugin;

impl Plugin for StagePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Stage>()
            .init_resource::<Thumbs>()
            .add_systems(Update, (update, thumbs, skip_cascades).chain());
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut stage: ResMut<Stage>,
    chars: Res<Characters>,
    server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut textures: ResMut<EguiUserTextures>,
    mut puppets: Query<&mut Puppet>,
    mut cams: Query<(&mut Transform, &mut Projection, &Camera), With<StageCamera>>,
    mut commands: Commands,
) {
    let st = &mut *stage;
    let want = st.want.take();
    let Some(want) = want else {
        if st.camera.is_some() || st.puppet.is_some() {
            teardown(st, &mut commands, &mut textures);
        }
        return;
    };
    // the picture
    let size = want.size.max(UVec2::splat(64)).min(UVec2::splat(4096));
    let resize = st
        .target
        .as_ref()
        .is_none_or(|(_, s)| s.as_ivec2().distance_squared(size.as_ivec2()) > 64);
    if resize {
        if let Some((old, _)) = st.target.take() {
            textures.remove_image(&old);
        }
        let img = Image::new_target_texture(
            size.x,
            size.y,
            TextureFormat::Rgba8Unorm,
            Some(TextureFormat::Rgba8UnormSrgb),
        );
        let h = images.add(img);
        st.egui = Some(textures.add_image(EguiTextureHandle::Strong(h.clone())));
        if let Some(c) = st.camera {
            commands.entity(c).insert(RenderTarget::Image(h.clone().into()));
        }
        st.target = Some((h, size));
    }
    let target = st.target.as_ref().unwrap().0.clone();
    if st.camera.is_none() {
        st.camera = Some(
            commands
                .spawn((
                    StageCamera,
                    Camera3d::default(),
                    Camera {
                        order: -80,
                        clear_color: ClearColorConfig::Custom(Color::srgb(0.085, 0.09, 0.1)),
                        ..default()
                    },
                    RenderTarget::Image(target.into()),
                    Projection::Perspective(PerspectiveProjection {
                        fov: 30f32.to_radians(),
                        near: 0.05,
                        far: 200.0,
                        ..default()
                    }),
                    Transform::from_translation(ORIGIN + Vec3::new(0.0, 1.0, 5.0)),
                    bevy::camera::Exposure { ev100: 12.0 },
                    AmbientLight {
                        color: Color::srgb(0.85, 0.9, 1.0),
                        brightness: 260.0,
                        affects_lightmapped_meshes: false,
                    },
                    Msaa::Sample4,
                    RenderLayers::layer(LAYER),
                ))
                .id(),
        );
        spawn_rig(st, &mut commands, &mut images, &mut meshes, &mut materials);
    }
    // the character
    let entry = chars.get(&want.character);
    let key = entry.map(|e| (e.meta.id.clone(), e.rig_key()));
    if key != st.shown {
        if let Some(p) = st.puppet.take() {
            commands.entity(p).despawn();
        }
        st.ready = false;
        st.time = 0.0;
        st.shown = key.clone();
        if let Some(e) = entry {
            st.height = e.meta.height.max(0.2);
            st.puppet = puppet::spawn(
                &mut commands,
                &server,
                e,
                Transform::from_translation(ORIGIN),
                st.anim.clone(),
                Some(LAYER),
            );
        }
    }
    if let Some(p) = st.puppet
        && let Ok(mut pp) = puppets.get_mut(p)
    {
        if pp.anim != st.anim {
            pp.anim = st.anim.clone();
            pp.time = 0.0;
        }
        pp.playing = st.playing && !st.front;
        pp.speed = st.speed;
        pp.in_place = st.in_place;
        if st.front {
            // marker editing: the model as it was modelled
            pp.anim = None;
        }
        if let Some(t) = st.seek.take() {
            pp.time = t;
        }
        st.time = pp.time;
        st.ready = pp.ready();
    }
    // the camera
    if let Some(c) = st.camera
        && let Ok((mut tf, mut proj, cam)) = cams.get_mut(c)
    {
        let h = st.height;
        let aspect = size.x as f32 / size.y.max(1) as f32;
        let (yaw, pitch, fov) = if st.front {
            (0.0, 0.0, 22f32.to_radians())
        } else {
            (st.orbit.yaw, st.orbit.pitch, 30f32.to_radians())
        };
        // fit the height (and, on a narrow picture, the T-pose's arm span)
        let half = (h * 0.62).max(h * 0.55 / aspect.max(0.1));
        let dist = half / (fov * 0.5).tan() * if st.front { 1.0 } else { st.orbit.zoom };
        let focus = ORIGIN + Vec3::Y * h * 0.5;
        let dir = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos());
        *tf = Transform::from_translation(focus + dir * dist).looking_at(focus, Vec3::Y);
        if let Projection::Perspective(p) = &mut *proj {
            p.fov = fov;
            p.aspect_ratio = aspect;
            p.far = dist * 4.0 + 20.0;
        }
        let view = tf.to_matrix().inverse();
        let clip = cam.clip_from_view();
        st.clip_from_model = clip * view * Mat4::from_translation(ORIGIN);
    }
}

fn teardown(st: &mut Stage, commands: &mut Commands, textures: &mut EguiUserTextures) {
    for e in st
        .puppet
        .take()
        .into_iter()
        .chain(st.camera.take())
        .chain(st.rig.drain(..))
    {
        commands.entity(e).despawn();
    }
    if let Some((img, _)) = st.target.take() {
        textures.remove_image(&img);
    }
    st.egui = None;
    st.shown = None;
    st.ready = false;
}

/// Key, fill and rim lights and the floor.
fn spawn_rig(
    st: &mut Stage,
    commands: &mut Commands,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let layer = RenderLayers::layer(LAYER);
    let lit = RenderLayers::from_layers(&[LAYER, THUMB_LAYER]);
    for (from, lux, color, shadows) in [
        (Vec3::new(-1.6, 3.2, 2.4), 42_000.0, Color::srgb(1.0, 0.97, 0.92), true),
        (Vec3::new(2.4, 1.0, 2.0), 11_000.0, Color::srgb(0.85, 0.92, 1.0), false),
        (Vec3::new(0.6, 2.4, -3.0), 30_000.0, Color::WHITE, false),
    ] {
        st.rig.push(
            commands
                .spawn((
                    DirectionalLight {
                        illuminance: lux,
                        color,
                        shadow_maps_enabled: shadows,
                        ..default()
                    },
                    Transform::from_translation(ORIGIN + from).looking_at(ORIGIN, Vec3::Y),
                    lit.clone(),
                ))
                .id(),
        );
    }
    let grid = images.add(grid_texture());
    let floor = materials.add(StandardMaterial {
        base_color_texture: Some(grid),
        perceptual_roughness: 0.95,
        reflectance: 0.2,
        ..default()
    });
    st.rig.push(
        commands
            .spawn((
                Mesh3d(meshes.add(Circle::new(5.0).mesh().resolution(96).build())),
                MeshMaterial3d(floor),
                Transform::from_translation(ORIGIN)
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                NotShadowCaster,
                layer,
            ))
            .id(),
    );
}

/// A dark floor with half-metre lines, fading out towards the rim (circle UVs: 0..1 across).
fn grid_texture() -> Image {
    const N: usize = 1024;
    let mut px = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32 * 10.0 - 5.0, y as f32 / N as f32 * 10.0 - 5.0);
            let r = (u * u + v * v).sqrt();
            let line = |a: f32| {
                let f = (a * 2.0).fract().abs();
                f.min(1.0 - f) < 0.012
            };
            let fade = (1.0 - (r / 5.0)).clamp(0.0, 1.0).powf(0.6);
            let base = 0.16 * fade + 0.07;
            let l = if line(u) || line(v) { 0.09 * fade } else { 0.0 };
            let c = ((base + l) * 255.0) as u8;
            let i = (y * N + x) * 4;
            px[i..i + 4].copy_from_slice(&[c, c, (c as f32 * 1.04).min(255.0) as u8, 255]);
        }
    }
    Image::new(
        Extent3d {
            width: N as u32,
            height: N as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// The world's sun doesn't light the stage: don't fit its cascades to the stage camera.
fn skip_cascades(
    cams: Query<Entity, With<StageCamera>>,
    mut lights: Query<(&mut Cascades, Option<&RenderLayers>)>,
) {
    for (mut c, layers) in &mut lights {
        if layers.is_some_and(|l| l.intersects(&RenderLayers::layer(LAYER))) {
            continue;
        }
        if cams.iter().any(|e| c.cascades.contains_key(&e)) {
            for e in &cams {
                c.cascades.remove(&e);
            }
        }
    }
}

// ------------------------------------------------------------------ portraits

/// Portraits of the characters the library grid shows: one at a time, shot while the stage is
/// up (its lights), kept as GPU pictures for the session.
#[derive(Resource, Default)]
pub struct Thumbs {
    /// rig key → picture
    pub ready: std::collections::HashMap<String, egui::TextureId>,
    /// the characters the grid shows this frame (set by the view)
    pub wanted: Vec<String>,
    images: Vec<Handle<Image>>,
    shot: Option<Shot>,
}

struct Shot {
    key: String,
    puppet: Entity,
    camera: Entity,
    image: Handle<Image>,
    frames: u32,
    waited: u32,
}

impl Thumbs {
    pub fn get(&self, key: &str) -> Option<egui::TextureId> {
        self.ready.get(key).copied()
    }
}

#[allow(clippy::too_many_arguments)]
fn thumbs(
    mut t: ResMut<Thumbs>,
    stage: Res<Stage>,
    chars: Res<Characters>,
    server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut textures: ResMut<EguiUserTextures>,
    puppets: Query<&Puppet>,
    mut commands: Commands,
) {
    let t = &mut *t;
    let wanted = std::mem::take(&mut t.wanted);
    // (only while the stage is up: its lights light the portraits)
    if stage.camera.is_none() {
        if let Some(s) = t.shot.take() {
            commands.entity(s.puppet).despawn();
            commands.entity(s.camera).despawn();
        }
        return;
    }
    if let Some(s) = &mut t.shot {
        s.waited += 1;
        let ready = puppets.get(s.puppet).is_ok_and(|p| p.ready());
        if ready {
            s.frames += 1;
        }
        // a few frames posed (and its textures swapped in), or give up after ~20 s
        if s.frames > 8 || s.waited > 1200 {
            let s = t.shot.take().unwrap();
            commands.entity(s.puppet).despawn();
            commands.entity(s.camera).despawn();
            let id = textures.add_image(EguiTextureHandle::Strong(s.image.clone()));
            t.ready.insert(s.key, id);
            t.images.push(s.image);
        }
        return;
    }
    let Some(e) = wanted
        .iter()
        .filter_map(|id| chars.get(id))
        .find(|e| !t.ready.contains_key(&e.rig_key()))
    else {
        return;
    };
    let h = e.meta.height.max(0.2);
    // head and shoulders: from a little under the chin to the top of the head
    let chin = e.meta.markers.map(|m| m.chin.y).unwrap_or(h * 0.86);
    let bottom = chin - (h - chin) * 0.6;
    let focus = (h + bottom) * 0.5;
    let half = (h - bottom) * 0.55;
    let fov = 24f32.to_radians();
    let dist = half / (fov * 0.5).tan();
    let dir = Vec3::new(0.42f32.sin(), 0.1, 0.42f32.cos()).normalize();
    let at = THUMB_ORIGIN + Vec3::Y * focus;
    let image = images.add(Image::new_target_texture(
        THUMB_SIZE,
        THUMB_SIZE,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    ));
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera {
                order: -81,
                clear_color: ClearColorConfig::Custom(Color::srgb(0.13, 0.14, 0.16)),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            Projection::Perspective(PerspectiveProjection {
                fov,
                near: 0.05,
                far: dist * 4.0 + 10.0,
                ..default()
            }),
            Transform::from_translation(at + dir * dist).looking_at(at, Vec3::Y),
            bevy::camera::Exposure { ev100: 12.0 },
            AmbientLight {
                color: Color::srgb(0.85, 0.9, 1.0),
                brightness: 320.0,
                affects_lightmapped_meshes: false,
            },
            Msaa::Sample4,
            RenderLayers::layer(THUMB_LAYER),
            StageCamera,
        ))
        .id();
    let Some(puppet) = puppet::spawn(
        &mut commands,
        &server,
        e,
        Transform::from_translation(THUMB_ORIGIN),
        e.rigged().then(|| super::moves::anim_for(&e.meta.moves, super::moves::Move::Idle)),
        Some(THUMB_LAYER),
    ) else {
        commands.entity(camera).despawn();
        return;
    };
    t.shot = Some(Shot {
        key: e.rig_key(),
        puppet,
        camera,
        image,
        frames: 0,
        waited: 0,
    });
}
