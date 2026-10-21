//! Rain splashes: little crowns of droplets jumping off the ground round the camera while it
//! rains (`splash.wesl`). One static mesh of quads animated entirely in the vertex shader, like
//! the rain itself (`weather.rs`): each quad is a splash that lands at a new random spot every
//! cycle, in a box wrapped round the camera, as many as the rain is heavy.
//!
//! The ground they land on is a small height window round the camera (`WINDOW`² texels of
//! `TEXEL` m, an `R32Float` texture), refilled on the CPU when the camera has moved a couple of
//! metres (or every `REFRESH` s): the terrain (or the sea), raised to the top of whatever object
//! stands there (`Colliders::tops`: roofs, walls, crates, a market stall's awning), so splashes
//! land on cobbled squares and roofs and none fall under cover. High above the ground (flying
//! the editor camera) there are none.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;

use super::RainState;
use crate::camera::MainCamera;
use crate::noise::smoothstep;
use crate::sky::SkyState;
use crate::terrain::Terrain;

/// Splash quads (at most this many alive per cycle, in heavy rain).
const SPLASHES: u32 = 1800;
/// Half size of the box round the camera the splashes land in (m).
const BOX_HALF: f32 = 13.0;
/// Height window: texels per side and metres per texel (32 m: the box with a margin).
const WINDOW: usize = 64;
const TEXEL: f32 = 0.5;
/// Refill the window when the camera moved this far (m) from its centre, or this often (s).
const MOVE: f32 = 2.0;
const REFRESH: f32 = 0.75;

#[derive(Clone, Debug, ShaderType, Default)]
pub struct SplashParams {
    /// x: rain amount (0..1), y: brightness, z: box half size (m), w: lightning flash
    pub amount: Vec4,
    /// xy: world xz of the height window's first texel corner, z: metres per texel, w: texels per side
    pub window: Vec4,
    /// `volumetrics::FroxelFog::params` (w 0: off)
    pub fog: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SplashMaterial {
    #[uniform(0)]
    pub params: SplashParams,
    #[texture(1, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
    /// The volumetric light (None while it is off): drawn after the view was fogged, the
    /// splashes fog themselves.
    #[texture(2, dimension = "3d")]
    #[sampler(3)]
    pub fog_volume: Option<Handle<Image>>,
}

impl Material for SplashMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/splash.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/splash.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    /// Sort after the other see-through surfaces: roads are alpha-blended ribbons a hair above
    /// the ground (and this mesh's centre is the world origin), so drawn before them the splashes
    /// on a road would be painted over.
    fn depth_bias(&self) -> f32 {
        1.0e6
    }
}

#[derive(Component)]
struct Splashes;

/// The height window's state: its corner and when it was filled.
#[derive(Default)]
struct HeightWindow {
    origin: Option<Vec2>,
    age: f32,
}

pub(super) fn register(app: &mut App) {
    app.add_plugins(MaterialPlugin::<SplashMaterial>::default())
        .add_systems(Startup, spawn)
        .add_systems(
            Update,
            update
                .after(super::lightning::LightningSystems)
                .after(crate::camera::CameraSystems),
        );
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<SplashMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let mut pos = Vec::with_capacity((SPLASHES * 4) as usize);
    let mut uv = Vec::with_capacity(pos.capacity());
    let mut idx = Vec::with_capacity((SPLASHES * 6) as usize);
    for i in 0..SPLASHES {
        for c in 0..4u32 {
            pos.push([0.0f32, 0.0, 0.0]);
            uv.push([i as f32, c as f32]);
        }
        let b = i * 4;
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U32(idx));
    let height = images.add(Image::new_fill(
        Extent3d {
            width: WINDOW as u32,
            height: WINDOW as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &(-1.0e4f32).to_le_bytes(),
        TextureFormat::R32Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    ));
    commands.spawn((
        Name::new("Rain splashes"),
        Splashes,
        Mesh3d(meshes.add(m)),
        MeshMaterial3d(mats.add(SplashMaterial {
            params: SplashParams::default(),
            height,
            fog_volume: None,
        })),
        Transform::IDENTITY,
        Visibility::Hidden,
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

#[allow(clippy::too_many_arguments)]
fn update(
    sky: Res<SkyState>,
    rain: Res<RainState>,
    lightning: Res<super::lightning::Lightning>,
    terrain: Res<Terrain>,
    colliders: Option<Res<crate::motion::collide::Colliders>>,
    store: Res<crate::objects::ObjectStore>,
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(&mut Visibility, &MeshMaterial3d<SplashMaterial>), With<Splashes>>,
    mut mats: ResMut<Assets<SplashMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut win: Local<HeightWindow>,
    fog: Option<Res<crate::volumetrics::FroxelFog>>,
) {
    let (Ok(cam), Ok((mut vis, mat))) = (cam.single(), q.single_mut()) else {
        return;
    };
    let p = cam.translation();
    let ground = if terrain.in_bounds(p.x, p.z) {
        terrain.height_at(p.x, p.z).max(terrain.sea_level)
    } else {
        terrain.sea_level
    };
    // only near the ground (from high up they would be noise, and far too small to see)
    let near = 1.0 - smoothstep(12.0, 30.0, p.y - ground);
    let amount = super::quantize(rain.rain * near);
    let show = amount > 0.02;
    let want = if show {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if *vis != want {
        *vis = want;
    }
    if !show {
        win.origin = None;
        return;
    }
    let Some(m) = mats.get(&mat.0) else { return };
    let height = m.height.clone();

    // the height window: refill when the camera has moved on, or now and then (objects stream
    // in, colliders finish building)
    win.age += time.delta_secs();
    let half = WINDOW as f32 * TEXEL * 0.5;
    let snapped = ((p.xz() / TEXEL).floor() * TEXEL) - Vec2::splat(half);
    let stale = win.origin.is_none_or(|o| o.distance(snapped) >= MOVE) || win.age >= REFRESH;
    let mut refilled = false;
    if stale && let Some(mut img) = images.get_mut(&height) {
        let mut h = vec![f32::MIN; WINDOW * WINDOW];
        for j in 0..WINDOW {
            for i in 0..WINDOW {
                let q = snapped + (Vec2::new(i as f32, j as f32) + 0.5) * TEXEL;
                h[j * WINDOW + i] = if terrain.in_bounds(q.x, q.y) {
                    terrain.height_at(q.x, q.y).max(terrain.sea_level)
                } else {
                    terrain.sea_level
                };
            }
        }
        if let Some(c) = &colliders {
            let centre = snapped + Vec2::splat(half);
            c.tops(&store, centre, half, |lo, hi, top| {
                let i0 = ((lo.x - snapped.x) / TEXEL).floor().max(0.0) as usize;
                let j0 = ((lo.y - snapped.y) / TEXEL).floor().max(0.0) as usize;
                let i1 = (((hi.x - snapped.x) / TEXEL).ceil() as usize).min(WINDOW);
                let j1 = (((hi.y - snapped.y) / TEXEL).ceil() as usize).min(WINDOW);
                for j in j0..j1 {
                    for i in i0..i1 {
                        let k = j * WINDOW + i;
                        h[k] = h[k].max(top);
                    }
                }
            });
        }
        img.data = Some(h.iter().flat_map(|v| v.to_le_bytes()).collect());
        win.origin = Some(snapped);
        win.age = 0.0;
        refilled = true;
    }
    let Some(origin) = win.origin else { return };
    let brightness = super::quantize((1.0 - sky.night * 0.85) * (1.0 - sky.weather.dim * 0.35));
    let (fog_volume, fog) = super::froxel(fog.as_deref());
    let params = SplashParams {
        amount: Vec4::new(amount, brightness, BOX_HALF, lightning.flash),
        window: Vec4::new(origin.x, origin.y, TEXEL, WINDOW as f32),
        fog,
    };
    // (a refilled height window needs the material's bind group rebuilt as well; the fog's reach
    // moves with the editor camera's zoom: only a real change counts)
    let changed = mats.get(&mat.0).is_some_and(|m| {
        m.params.amount != params.amount
            || m.params.window != params.window
            || m.fog_volume != fog_volume
            || (m.params.fog - fog).abs().max_element() > 0.01 * fog.y.abs().max(0.1)
    });
    if (changed || refilled)
        && let Some(mut m) = mats.get_mut(&mat.0)
    {
        m.params = params;
        m.fog_volume = fog_volume;
    }
}
