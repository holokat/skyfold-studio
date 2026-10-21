//! Living world effects from the particle-physics port (`crates/particle_fluids`):
//! * campfires release smoke: heated air rises, bends in the wind and carries smoke tracers,
//!   ray-marched as a lit volume; at night the fire also glows and flickers;
//! * banners hang from their poles as cloth that waves in the world's wind;
//! * tarp shelters are cloth pinned to four posts; when the zone's weather rains, drops land on
//!   the fabric, darken it as it soaks and run off the low edge.
//!
//! Effects follow the placed objects automatically. Every one keeps its last shape, but only the
//! few nearest the camera simulate, each stepping on a worker thread.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use particle_fluids::glam::Vec3 as PVec3;
use particle_fluids::{
    Aabb, ClothHandle, ClothOptions, ClothPins, ClothSurface, Container, LiquidFill, LiquidHandle,
    LiquidOptions, ParticleSizing, SimConfig, Simulation, SmokeOptions, SmokeVolume,
    SmokeVolumeOptions,
};

use crate::camera::MainCamera;
use crate::objects::{Library, ObjectStore, Placed};
use crate::procgen::meshes::{AssetKind, BANNER_ARM, BANNER_POLE_H, TARP_HALF, TARP_POSTS};
use crate::sky::{Moon, SkyState, Sun};

/// Effects simulate only within these distances, on screen, and while the world animates (see
/// wind::Motion); paused, they keep their last state. Beyond, smoke keeps its last volume.
const SMOKE_RANGE: f32 = 160.0;
const CLOTH_RANGE: f32 = 60.0;
/// Seconds an effect simulates even while motion is paused, so a fresh plume isn't empty.
const WARM_UP: f32 = 4.0;
const MAX_SMOKE: usize = 2;
const MAX_CLOTH: usize = 6;
/// Smoke container (m, around the fire): narrow and tall, like a real plume.
const SMOKE_HALF: f32 = 0.7;
const SMOKE_H: f32 = 4.4;

fn v3(v: Vec3) -> PVec3 {
    PVec3::new(v.x, v.y, v.z)
}
fn b3(v: PVec3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FxKind {
    Smoke,
    Banner,
    Tarp,
}

/// The photoscanned stone fire pit and the hand-painted campfire smoke and glow like the campfire
/// (and burn: `crate::flames`).
pub fn is_campfire_id(id: &str) -> bool {
    matches!(id, "ph:stone_fire_pit" | "hp:campfire")
}

/// Does this library model burn as a campfire (smoke, a glow; a flame from `crate::flames`)?
pub fn is_campfire(kind: Option<AssetKind>, id: &str) -> bool {
    kind == Some(AssetKind::Campfire) || is_campfire_id(id)
}

fn fx_kind(k: AssetKind) -> Option<FxKind> {
    match k {
        AssetKind::Campfire => Some(FxKind::Smoke),
        AssetKind::Banner => Some(FxKind::Banner),
        AssetKind::TarpShelter => Some(FxKind::Tarp),
        _ => None,
    }
}

/// What a finished step hands back for rendering.
enum RenderData {
    Smoke(SmokeVolume),
    Cloth {
        surface: ClothSurface,
        drops: Vec<PVec3>,
    },
}

struct StepOut {
    sim: Box<Simulation>,
    render: RenderData,
    rain_carry: f32,
}

/// Inputs for one step, captured on the main thread.
#[derive(Clone, Copy)]
struct StepIn {
    /// one step of the whole `dt` instead of fixed 1/60 s steps (half-rate smoke)
    single: bool,
    dt: f32,
    wind: Vec3,
    rain: f32,
    light: Vec3,
    seed: u32,
}

struct Effect {
    kind: FxKind,
    placed: Placed,
    sim: Option<Box<Simulation>>,
    task: Option<Task<StepOut>>,
    cloth: Option<ClothHandle>,
    rain: Option<LiquidHandle>,
    rain_carry: f32,
    root: Entity,
    mesh: Option<Handle<Mesh>>,
    drops: Option<Handle<Mesh>>,
    smoke_mat: Option<Handle<SmokeMaterial>>,
    volume: Option<Handle<Image>>,
    light: Option<Entity>,
    smoke_box: Option<Entity>,
    pending_dt: f32,
    /// simulated seconds so far
    sim_time: f32,
}

#[derive(Resource, Default)]
struct Effects {
    list: HashMap<u64, Effect>,
    revision: Option<u64>,
    candidates: Vec<(u64, FxKind)>,
    mats: Option<FxMaterials>,
    frame: u32,
}

struct FxMaterials {
    banners: Vec<Handle<StandardMaterial>>,
    canvas: Handle<StandardMaterial>,
    water: Handle<StandardMaterial>,
    smoke_box: Handle<Mesh>,
    smoke_box_inside: Handle<Mesh>,
}

#[derive(Clone, Debug, ShaderType, Default)]
pub struct SmokeParams {
    /// xyz: volume min (world), w: steps
    pub min: Vec4,
    /// xyz: volume max (world), w: opacity
    pub max: Vec4,
    /// rgb: radiance of fully lit smoke (before exposure)
    pub color: Vec4,
    /// rgb: radiance of smoke in its own shadow
    pub shadow: Vec4,
    /// rgb: scattered firelight at one metre (before exposure), w unused
    pub fire: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SmokeMaterial {
    #[uniform(0)]
    pub params: SmokeParams,
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    pub volume: Handle<Image>,
}

impl Material for SmokeMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/smoke.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SmokeMaterial>::default())
            .init_resource::<Effects>()
            .add_systems(
                Update,
                (manage_effects, step_effects)
                    .chain()
                    .after(crate::sky::SkySystems),
            );
    }
}

fn setup_materials(
    meshes: &mut Assets<Mesh>,
    mats: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> FxMaterials {
    // photoscanned CC0 fabrics from the material library
    let dir = std::path::PathBuf::from(crate::assets_path()).join("textures/materials");
    // (in the world's art style when they're first needed: painted fabric in a hand-painted world)
    let art = crate::art_style::current();
    let tex = |images: &mut Assets<Image>, name: &str, srgb: bool| {
        crate::trees::load_texture(
            &crate::art_style::texture(&dir.join(name), art),
            srgb,
            false,
            true,
        )
        .map(|i| images.add(i))
    };
    let linen = tex(images, "fabric_linen/albedo.png", true);
    let linen_n = tex(images, "fabric_linen/normal.png", false);
    let canvas = tex(images, "fabric_canvas_tent/albedo.png", true);
    let canvas_n = tex(images, "fabric_canvas_tent/normal.png", false);
    let fabric =
        |c: Color, t: &Option<Handle<Image>>, n: &Option<Handle<Image>>| StandardMaterial {
            base_color: c,
            base_color_texture: t.clone(),
            normal_map_texture: n.clone(),
            perceptual_roughness: 0.88,
            reflectance: 0.2,
            double_sided: true,
            cull_mode: None,
            diffuse_transmission: 0.25,
            ..default()
        };
    // heraldic banner colours
    let banners = [
        Color::srgb(0.62, 0.08, 0.1),
        Color::srgb(0.1, 0.2, 0.55),
        Color::srgb(0.12, 0.38, 0.16),
        Color::srgb(0.78, 0.6, 0.15),
        Color::srgb(0.32, 0.12, 0.42),
    ]
    .iter()
    .map(|c| mats.add(fabric(*c, &linen, &linen_n)))
    .collect();
    let canvas = mats.add(fabric(Color::srgb(0.86, 0.8, 0.68), &canvas, &canvas_n));
    let water = mats.add(StandardMaterial {
        base_color: Color::srgba(0.75, 0.85, 0.95, 0.55),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.05,
        reflectance: 0.5,
        ..default()
    });
    let cube = Cuboid::new(1.0, 1.0, 1.0).mesh().build();
    let mut inside = cube.clone();
    // inverted box (seen from inside): flip the winding
    if let Some(Indices::U32(idx)) = inside.indices_mut() {
        for t in idx.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    } else if let Some(Indices::U16(idx)) = inside.indices_mut() {
        for t in idx.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
    FxMaterials {
        banners,
        canvas,
        water,
        smoke_box: meshes.add(cube),
        smoke_box_inside: meshes.add(inside),
    }
}

fn new_sim(
    kind: FxKind,
    wind: Vec3,
    rot: Quat,
) -> Option<(Box<Simulation>, Option<ClothHandle>, Option<LiquidHandle>)> {
    let local_wind = rot.inverse() * wind;
    match kind {
        FxKind::Smoke => {
            let bounds = Aabb::new(
                PVec3::new(-SMOKE_HALF, -0.05, -SMOKE_HALF),
                PVec3::new(SMOKE_HALF, SMOKE_H, SMOKE_HALF),
            );
            let mut cfg = SimConfig::new(
                Some(Container::closed(bounds)),
                ParticleSizing::Budget(2000),
            );
            cfg.substeps = Some(2);
            cfg.threads = Some(2);
            let mut sim = Simulation::new(cfg);
            sim.add_smoke(SmokeOptions {
                source: Some(PVec3::new(0.0, 0.3, 0.0)),
                radius: 0.24,
                rate: 1600.0,
                lifetime: 4.5,
                heat: 6.0,
                cooling: 0.3,
                // the plume rises in still air; wind bends it at render time (a narrow box can't
                // hold a drifting plume: it would blow out sideways before it rises)
                wind: PVec3::ZERO,
                wind_tunnel: false,
                ..default()
            })
            .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
            .ok()?;
            sim.start()
                .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
                .ok()?;
            Some((Box::new(sim), None, None))
        }
        FxKind::Banner => {
            let w = BANNER_ARM.1 - BANNER_ARM.0 - 0.12;
            let h = 2.1;
            let top = BANNER_POLE_H - 0.25 - 0.04;
            let bounds = Aabb::new(
                PVec3::new(-3.0, -0.5, -3.0),
                PVec3::new(4.0, BANNER_POLE_H + 1.0, 3.0),
            );
            let mut cfg =
                SimConfig::new(Some(Container::open(bounds)), ParticleSizing::Radius(0.045));
            cfg.substeps = Some(10);
            cfg.threads = Some(1);
            let mut sim = Simulation::new(cfg);
            let cloth = sim
                .add_cloth(ClothOptions {
                    width: w,
                    height: h,
                    position: PVec3::new((BANNER_ARM.0 + BANNER_ARM.1) * 0.5, top - h * 0.5, 0.0),
                    pins: ClothPins::Top,
                    softness: 0.55,
                    weight: 0.22,
                    wind: v3(local_wind),
                    ..default()
                })
                .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
                .ok()?;
            sim.start()
                .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
                .ok()?;
            Some((Box::new(sim), Some(cloth), None))
        }
        FxKind::Tarp => {
            let run = TARP_HALF * 2.0;
            let drop = TARP_POSTS.0 - TARP_POSTS.1;
            let tilt = (drop / run).atan();
            let slant = (run * run + drop * drop).sqrt();
            let bounds = Aabb::new(
                PVec3::new(-3.2, -0.3, -3.2),
                PVec3::new(3.2, TARP_POSTS.0 + 1.5, 3.2),
            );
            let mut cfg =
                SimConfig::new(Some(Container::open(bounds)), ParticleSizing::Radius(0.045));
            cfg.substeps = Some(6);
            cfg.threads = Some(2);
            cfg.reserve = 600;
            let mut sim = Simulation::new(cfg);
            let cloth = sim
                .add_cloth(ClothOptions {
                    width: run,
                    height: slant,
                    position: PVec3::new(0.0, (TARP_POSTS.0 + TARP_POSTS.1) * 0.5, 0.0),
                    // lay flat (top edge towards −z), tilted so the back edge is lower
                    rotation: particle_fluids::glam::Quat::from_rotation_x(
                        -std::f32::consts::FRAC_PI_2 - tilt,
                    ),
                    pins: ClothPins::Corners,
                    softness: 0.25,
                    weight: 0.45,
                    wind: v3(local_wind * 0.5),
                    ..default()
                })
                .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
                .ok()?;
            let rain = sim
                .add_liquid(LiquidOptions {
                    fill: LiquidFill::None,
                    kill_height: Some(0.0),
                    ..default()
                })
                .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
                .ok()?;
            sim.start()
                .map_err(|e| warn!("effect setup ({kind:?}): {e:?}"))
                .ok()?;
            Some((Box::new(sim), Some(cloth), Some(rain)))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn manage_effects(
    mut commands: Commands,
    mut fx: ResMut<Effects>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    wind: Res<crate::wind::WindState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut smoke_mats: ResMut<Assets<SmokeMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(lib) = lib else { return };
    if fx.mats.is_none() {
        fx.mats = Some(setup_materials(&mut meshes, &mut mats, &mut images));
    }
    // effect objects change only when the store does
    if fx.revision != Some(store.revision) {
        fx.revision = Some(store.revision);
        fx.candidates = store
            .items
            .values()
            .filter_map(|p| {
                lib.get(&p.asset)
                    .and_then(|e| {
                        e.kind
                            .and_then(fx_kind)
                            .or_else(|| is_campfire_id(&e.id).then_some(FxKind::Smoke))
                    })
                    .map(|k| (p.id, k))
            })
            .collect();
        // drop effects whose object is gone or moved
        let gone: Vec<u64> = fx
            .list
            .iter()
            .filter(|(id, e)| {
                store.items.get(id).is_none_or(|p| {
                    p.pos != e.placed.pos
                        || p.rot != e.placed.rot
                        || p.scale != e.placed.scale
                        || p.stretch != e.placed.stretch
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(e) = fx.list.remove(&id) {
                commands.entity(e.root).despawn();
            }
        }
        // create the missing ones (each keeps its shape even while it isn't simulating)
        let wind_v = wind_velocity(&wind, 0.0);
        let cands = fx.candidates.clone();
        for (id, kind) in cands {
            if fx.list.contains_key(&id) {
                continue;
            }
            let Some(p) = store.items.get(&id).cloned() else {
                continue;
            };
            let rot = Quat::from_array(p.rot);
            let Some((sim, cloth, rain)) = new_sim(kind, wind_v, rot) else {
                continue;
            };
            let tf = Transform {
                translation: Vec3::from_array(p.pos),
                rotation: if kind == FxKind::Smoke {
                    Quat::IDENTITY
                } else {
                    rot
                },
                scale: Vec3::ONE,
            };
            let root = commands
                .spawn((Name::new("Effect"), tf, Visibility::default()))
                .id();
            let m = fx.mats.as_ref().unwrap();
            let mut e = Effect {
                kind,
                placed: p.clone(),
                sim: Some(sim),
                task: None,
                cloth,
                rain,
                rain_carry: 0.0,
                root,
                mesh: None,
                drops: None,
                smoke_mat: None,
                volume: None,
                light: None,
                smoke_box: None,
                pending_dt: 0.0,
                sim_time: 0.0,
            };
            match kind {
                FxKind::Smoke => {
                    let vol = images.add(volume_image([2, 2, 2], &[0.0; 8], &[0.0; 8]));
                    let mat = smoke_mats.add(SmokeMaterial {
                        params: SmokeParams::default(),
                        volume: vol.clone(),
                    });
                    let child = commands
                        .spawn((
                            Mesh3d(m.smoke_box.clone()),
                            MeshMaterial3d(mat.clone()),
                            Transform::from_xyz(0.0, SMOKE_H * 0.5, 0.0).with_scale(Vec3::new(
                                SMOKE_HALF * 2.0,
                                SMOKE_H,
                                SMOKE_HALF * 2.0,
                            )),
                            NotShadowCaster,
                            bevy::light::NotShadowReceiver,
                        ))
                        .id();
                    let light = commands
                        .spawn((
                            PointLight {
                                color: Color::srgb(1.0, 0.55, 0.22),
                                intensity: 0.0,
                                range: 14.0,
                                shadow_maps_enabled: false,
                                ..default()
                            },
                            Transform::from_xyz(0.0, 0.5, 0.0),
                        ))
                        .id();
                    commands.entity(root).add_children(&[child, light]);
                    e.smoke_mat = Some(mat);
                    e.volume = Some(vol);
                    e.light = Some(light);
                    e.smoke_box = Some(child);
                }
                FxKind::Banner | FxKind::Tarp => {
                    let mut surface = ClothSurface::new(2, true);
                    if let (Some(sim), Some(c)) = (&e.sim, e.cloth) {
                        sim.fill_cloth_surface(c, &mut surface);
                    }
                    let mesh = meshes.add(cloth_mesh(&surface));
                    let mat = if kind == FxKind::Banner {
                        m.banners[(id as usize) % m.banners.len()].clone()
                    } else {
                        m.canvas.clone()
                    };
                    let child = commands
                        .spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat)))
                        .id();
                    commands.entity(root).add_child(child);
                    e.mesh = Some(mesh);
                    if kind == FxKind::Tarp {
                        let drops = meshes.add(drops_mesh(&[]));
                        let d = commands
                            .spawn((
                                Mesh3d(drops.clone()),
                                MeshMaterial3d(m.water.clone()),
                                NotShadowCaster,
                            ))
                            .id();
                        commands.entity(root).add_child(d);
                        e.drops = Some(drops);
                    }
                }
            }
            if std::env::var("WB_FX_LOG").is_ok() {
                info!("effect {kind:?} for object {id} at {:?}", e.placed.pos);
            }
            fx.list.insert(id, e);
        }
    }
}

/// World wind in m/s from the editor's wind setting, with slow gusts.
fn wind_velocity(w: &crate::wind::WindState, t: f32) -> Vec3 {
    w.velocity(t)
}

/// Sky illumination belongs to the plume's location, not the camera. In particular a cave
/// camera opens its exposure by several stops; unoccluded daylight here turns smoke white.
fn smoke_sky_light(key: Vec3, ambient: Vec3, daylight: f32) -> (Vec3, Vec3) {
    let daylight = daylight.clamp(0.0, 1.0);
    (
        (key * 0.48 + ambient * 0.7) * daylight,
        (ambient * 0.4 + key * 0.04) * daylight,
    )
}

fn campfire_lumens(t: f32, id: u64, night: f32) -> f32 {
    let flicker =
        0.8 + 0.12 * (t * 9.1 + id as f32).sin() + 0.08 * (t * 23.7 + id as f32 * 0.7).sin();
    60_000.0 * flicker * (0.15 + 0.85 * night)
}

#[allow(clippy::too_many_arguments)]
fn step_effects(
    mut fx: ResMut<Effects>,
    wind: Res<crate::wind::WindState>,
    environment: (Res<SkyState>, Res<crate::caves::render::CaveWorld>),
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    sun: Query<&DirectionalLight, (With<Sun>, Without<Moon>)>,
    moon: Query<&DirectionalLight, (With<Moon>, Without<Sun>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut smoke_mats: ResMut<Assets<SmokeMaterial>>,
    mut lights: Query<&mut PointLight>,
    mut mesh_q: Query<&mut Mesh3d>,
    children: Query<&Children>,
    mut box_tf: Query<&mut Transform, Without<PointLight>>,
    motion: Res<crate::wind::Motion>,
    frustum: Query<&bevy::camera::primitives::Frustum, With<MainCamera>>,
) {
    let (sky, caves) = environment;
    let Ok(cam) = cam.single() else { return };
    if std::env::var("WB_PERF").is_ok_and(|p| p.contains("noeffects")) {
        return;
    }
    let frustum = frustum.single().ok();
    let cam_p = cam.translation();
    let dt = time.delta_secs().min(0.05);
    let t = time.elapsed_secs();
    fx.frame = fx.frame.wrapping_add(1);
    let frame = fx.frame;
    let wind_v = wind_velocity(&wind, t);
    let rain = sky.weather.rain;
    // light the smoke with whatever lights the scene
    let (sun_lux, sun_col) = sun
        .single()
        .map(|l| (l.illuminance, l.color.to_linear()))
        .unwrap_or((0.0, LinearRgba::WHITE));
    let (moon_lux, moon_col) = moon
        .single()
        .map(|l| (l.illuminance, l.color.to_linear()))
        .unwrap_or((0.0, LinearRgba::WHITE));
    let light_dir = if sky.sun_dir.y > -0.05 {
        sky.sun_dir
    } else {
        sky.moon_dir
    };
    let key =
        |c: LinearRgba, lux: f32| Vec3::new(c.red, c.green, c.blue) * lux / std::f32::consts::PI;
    let key_rad = key(sun_col, sun_lux) + key(moon_col, moon_lux);
    let ambient = key_rad * 0.35 + Vec3::new(0.02, 0.025, 0.035) * 30.0 * (1.0 - sky.night * 0.7);

    // nearest effects simulate
    // nearest on-screen effects simulate (while the world animates, or to warm up)
    let on_screen = |p: Vec3| {
        frustum.is_none_or(|f| {
            f.intersects_sphere(
                &bevy::camera::primitives::Sphere {
                    center: (p + Vec3::Y * 2.0).into(),
                    radius: 6.0,
                },
                false,
            )
        })
    };
    let mut by_dist: Vec<(u64, FxKind, f32, bool)> = fx
        .list
        .iter()
        .map(|(id, e)| {
            let p = Vec3::from_array(e.placed.pos);
            (
                *id,
                e.kind,
                p.distance(cam_p),
                on_screen(p) && (motion.live || e.sim_time < WARM_UP),
            )
        })
        .collect();
    by_dist.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut active = std::collections::HashSet::new();
    let (mut ns, mut nc) = (0, 0);
    for (id, kind, d, wanted) in &by_dist {
        let ok = *wanted
            && match kind {
                FxKind::Smoke => *d < SMOKE_RANGE && ns < MAX_SMOKE,
                _ => *d < CLOTH_RANGE && nc < MAX_CLOTH,
            };
        if ok {
            active.insert(*id);
            if *kind == FxKind::Smoke {
                ns += 1;
            } else {
                nc += 1;
            }
        }
    }

    let pool = AsyncComputeTaskPool::get();
    let boxes = fx_box_meshes(&fx);
    let ids: Vec<u64> = fx.list.keys().copied().collect();
    for id in ids {
        let Some(e) = fx.list.get_mut(&id) else {
            continue;
        };
        // collect a finished step
        if let Some(task) = e.task.as_mut()
            && let Some(out) = check_ready(task)
        {
            e.task = None;
            e.sim = Some(out.sim);
            e.rain_carry = out.rain_carry;
            match out.render {
                RenderData::Smoke(vol) => {
                    if std::env::var("WB_FX_LOG").is_ok() && frame % 60 == 0 {
                        let total: f32 = vol.density.iter().sum();
                        let peak = vol.density.iter().copied().fold(0.0f32, f32::max);
                        let [nx, ny, _] = vol.resolution;
                        let my: f32 = vol
                            .density
                            .iter()
                            .enumerate()
                            .map(|(i, d)| d * ((i as u32 / nx) % ny) as f32)
                            .sum::<f32>()
                            / total.max(1e-6)
                            / ny as f32;
                        info!(
                            "smoke {id}: density sum {total:.1}, peak {peak:.2}, centroid height {:.0}%",
                            my * 100.0
                        );
                    }
                    if let (Some(h), Some(mh)) = (&e.volume, &e.smoke_mat) {
                        if let Some(mut img) = images.get_mut(h) {
                            *img = volume_image(vol.resolution, &vol.density, &vol.light);
                        }
                        let origin = Vec3::from_array(e.placed.pos);
                        if let Some(mut m) = smoke_mats.get_mut(mh) {
                            m.params.min = (origin + b3(vol.min)).extend(48.0);
                            m.params.max = (origin + b3(vol.max)).extend(1.0);
                        }
                    }
                }
                RenderData::Cloth { surface, drops } => {
                    if std::env::var("WB_FX_LOG").is_ok() && frame % 60 == 0 {
                        info!(
                            "cloth {id}: {} verts, {} drops",
                            surface.positions.len(),
                            drops.len()
                        );
                    }
                    if let Some(h) = &e.mesh
                        && let Some(mut m) = meshes.get_mut(h)
                    {
                        *m = cloth_mesh(&surface);
                    }
                    if let Some(h) = &e.drops
                        && let Some(mut m) = meshes.get_mut(h)
                    {
                        *m = drops_mesh(&drops);
                    }
                }
            }
        }
        // smoke colour / fire light follow the lighting every frame; the wind leans the plume
        let fire_lumens = campfire_lumens(t, id, sky.night.max(sky.weather.dim * 0.5));
        let bend = Vec2::new(wind_v.x, wind_v.z) * 0.45;
        let bend = bend.clamp_length_max(4.0);
        if let Some(mh) = &e.smoke_mat
            && let Some(mut m) = smoke_mats.get_mut(mh)
        {
            // Sample above the hearth (which can be seated in the floor), in the same daylight
            // field as the cave rock. This also preserves the gradual light at cave mouths.
            let source = Vec3::from_array(e.placed.pos) + Vec3::Y * 0.8;
            let daylight = caves.light.as_ref().map_or(1.0, |l| l.at(source));
            let (lit, shadow) = smoke_sky_light(key_rad, ambient, daylight);
            m.params.color = lit.extend(bend.x);
            m.params.shadow = shadow.extend(bend.y);
            // A small scattered share of the hearth's real light, attenuated by distance in
            // the volume shader. The upper smoke stays smoke rather than becoming a flame.
            let fire = Color::srgb(1.0, 0.45, 0.12).to_linear();
            m.params.fire = (Vec3::new(fire.red, fire.green, fire.blue)
                * (fire_lumens * 0.06 / (4.0 * std::f32::consts::PI.powi(2))))
            .extend(0.0);
            if let Some(b) = e.smoke_box
                && let Ok(mut tf) = box_tf.get_mut(b)
            {
                let (lo, hi) = (
                    Vec3::new(-SMOKE_HALF, 0.0, -SMOKE_HALF),
                    Vec3::new(SMOKE_HALF, SMOKE_H, SMOKE_HALF),
                );
                let rlo = lo + Vec3::new(bend.x.min(0.0), 0.0, bend.y.min(0.0));
                let rhi = hi + Vec3::new(bend.x.max(0.0), 0.0, bend.y.max(0.0));
                tf.translation = (rlo + rhi) * 0.5;
                tf.scale = rhi - rlo;
            }
            let visible = active.contains(&id)
                || Vec3::from_array(e.placed.pos).distance(cam_p) < SMOKE_RANGE;
            m.params.max.w = if visible { 1.0 } else { 0.0 };
        }
        if let Some(l) = e.light
            && let Ok(mut pl) = lights.get_mut(l)
        {
            pl.intensity = fire_lumens;
        }
        // camera inside the smoke box: draw its inside faces
        if e.kind == FxKind::Smoke
            && let Ok(kids) = children.get(e.root)
            && let Some(m) = boxes.clone()
        {
            let o = Vec3::from_array(e.placed.pos);
            let inside = (cam_p.x - o.x).abs() < SMOKE_HALF + 4.3
                && (cam_p.z - o.z).abs() < SMOKE_HALF + 4.3
                && cam_p.y > o.y - 0.3
                && cam_p.y < o.y + SMOKE_H + 0.3;
            let want = if inside { m.1.clone() } else { m.0.clone() };
            for k in kids.iter() {
                if let Ok(mut mesh) = mesh_q.get_mut(k)
                    && mesh.0 != want
                    && (mesh.0 == m.0 || mesh.0 == m.1)
                {
                    mesh.0 = want.clone();
                }
            }
        }
        let Some(e) = fx.list.get_mut(&id) else {
            continue;
        };
        e.pending_dt += dt;
        // smoke beyond arm's length simulates every other frame, one double-length step
        let far_smoke =
            e.kind == FxKind::Smoke && Vec3::from_array(e.placed.pos).distance(cam_p) > 12.0;
        if far_smoke && frame % 2 == 1 && e.task.is_none() {
            continue;
        }
        if !active.contains(&id) || e.task.is_some() {
            if !active.contains(&id) {
                e.pending_dt = 0.0;
            }
            continue;
        }
        let Some(mut sim) = e.sim.take() else {
            continue;
        };
        let input = StepIn {
            single: far_smoke,
            dt: if far_smoke {
                e.pending_dt.min(0.05)
            } else {
                e.pending_dt.min(0.1)
            },
            wind: Quat::from_array(e.placed.rot).inverse() * wind_v,
            rain,
            light: light_dir,
            seed: (id as u32) ^ frame.wrapping_mul(2_654_435_761),
        };
        e.sim_time += input.dt;
        e.pending_dt = 0.0;
        let (kind, cloth, rain_h, carry) = (e.kind, e.cloth, e.rain, e.rain_carry);
        e.task = Some(pool.spawn(async move {
            run_step(&mut sim, kind, cloth, rain_h, carry, input)
                .map(|(render, rain_carry)| StepOut {
                    sim,
                    render,
                    rain_carry,
                })
                .unwrap()
        }));
    }
}

fn fx_box_meshes(fx: &Effects) -> Option<(Handle<Mesh>, Handle<Mesh>)> {
    fx.mats
        .as_ref()
        .map(|m| (m.smoke_box.clone(), m.smoke_box_inside.clone()))
}

/// One step on a worker thread: apply wind / rain, simulate, extract render data.
fn run_step(
    sim: &mut Simulation,
    kind: FxKind,
    cloth: Option<ClothHandle>,
    rain: Option<LiquidHandle>,
    mut carry: f32,
    i: StepIn,
) -> Option<(RenderData, f32)> {
    match kind {
        FxKind::Smoke => {
            let _ = if i.single {
                sim.advance(i.dt)
            } else {
                sim.step(i.dt).map(|_| ())
            };
            let mut vol = SmokeVolume::default();
            let opts = SmokeVolumeOptions {
                resolution: Some([18, 46, 18]),
                light_direction: v3(i.light.normalize_or(Vec3::Y)),
                density: 2.6,
                ..default()
            };
            sim.build_smoke_volume(&opts, &mut vol);
            Some((RenderData::Smoke(vol), 0.0))
        }
        FxKind::Banner | FxKind::Tarp => {
            let c = cloth?;
            let gust = if kind == FxKind::Tarp { 0.5 } else { 1.0 };
            sim.set_cloth_wind(c, v3(i.wind * gust));
            let mut drops = vec![];
            if let Some(r) = rain {
                // rain over the tarp (plus a margin), heavier with the weather
                carry += i.rain * 90.0 * i.dt;
                let mut h = i.seed;
                let mut rnd = || {
                    h ^= h << 13;
                    h ^= h >> 17;
                    h ^= h << 5;
                    (h & 0xffff) as f32 / 65535.0
                };
                while carry >= 1.0 {
                    carry -= 1.0;
                    let p = PVec3::new(
                        (rnd() * 2.0 - 1.0) * (TARP_HALF + 0.2),
                        TARP_POSTS.0 + 1.0,
                        (rnd() * 2.0 - 1.0) * (TARP_HALF + 0.2),
                    );
                    let _ =
                        sim.emit_liquid(r, p, PVec3::new(i.wind.x * 0.15, -5.5, i.wind.z * 0.15));
                }
            }
            let _ = sim.step(i.dt);
            let mut surface = ClothSurface::new(2, true);
            sim.fill_cloth_surface(c, &mut surface);
            if let Some(r) = rain {
                sim.liquid_particles(r, &mut drops, None);
            }
            Some((RenderData::Cloth { surface, drops }, carry))
        }
    }
}

fn cloth_mesh(s: &ClothSurface) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        s.positions
            .iter()
            .map(|p| [p.x, p.y, p.z])
            .collect::<Vec<_>>(),
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_NORMAL,
        s.normals
            .iter()
            .map(|p| [p.x, p.y, p.z])
            .collect::<Vec<_>>(),
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        s.uvs
            .iter()
            .map(|u| [u[0] * 2.0, u[1] * 2.0])
            .collect::<Vec<_>>(),
    );
    // wet fabric darkens
    let wet: Vec<[f32; 4]> = (0..s.positions.len())
        .map(|i| {
            let w = s.wetness.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            let k = 1.0 - 0.5 * w;
            [k, k, k * 1.02, 1.0]
        })
        .collect();
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, wet);
    if !s.uvs.is_empty() {
        m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, cloth_tangents(s));
    }
    m.insert_indices(Indices::U32(s.indices.clone()));
    m
}

/// Per-vertex tangents from the triangles' UV gradients, orthogonalised against the normals.
/// The cloth is a smooth, regularly mapped sheet, so this matches mikktspace closely at a tiny
/// fraction of its cost (mikktspace on every cloth update took ~15 ms a frame).
fn cloth_tangents(s: &ClothSurface) -> Vec<[f32; 4]> {
    let n = s.positions.len();
    let mut t = vec![Vec3::ZERO; n];
    let mut b = vec![Vec3::ZERO; n];
    let p = |i: u32| {
        Vec3::new(
            s.positions[i as usize].x,
            s.positions[i as usize].y,
            s.positions[i as usize].z,
        )
    };
    let uv = |i: u32| Vec2::from_array(s.uvs[i as usize]);
    for tri in s.indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0], tri[1], tri[2]);
        let (e1, e2) = (p(i1) - p(i0), p(i2) - p(i0));
        let (d1, d2) = (uv(i1) - uv(i0), uv(i2) - uv(i0));
        let det = d1.x * d2.y - d2.x * d1.y;
        if det.abs() < 1e-12 {
            continue;
        }
        let r = 1.0 / det;
        let tt = (e1 * d2.y - e2 * d1.y) * r;
        let bb = (e2 * d1.x - e1 * d2.x) * r;
        for i in [i0, i1, i2] {
            t[i as usize] += tt;
            b[i as usize] += bb;
        }
    }
    (0..n)
        .map(|i| {
            let nn =
                Vec3::new(s.normals[i].x, s.normals[i].y, s.normals[i].z).normalize_or(Vec3::Y);
            let tg = (t[i] - nn * nn.dot(t[i])).normalize_or(nn.any_orthonormal_vector());
            let w = if nn.cross(tg).dot(b[i]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            [tg.x, tg.y, tg.z, w]
        })
        .collect()
}

/// Rain drops as tiny elongated diamonds.
fn drops_mesh(drops: &[PVec3]) -> Mesh {
    let mut pos = Vec::with_capacity(drops.len() * 6);
    let mut nor = Vec::with_capacity(drops.len() * 6);
    let mut idx: Vec<u32> = Vec::with_capacity(drops.len() * 24);
    let (r, h) = (0.022, 0.05);
    for d in drops {
        let base = pos.len() as u32;
        let c = [d.x, d.y, d.z];
        for (o, n) in [
            ([0.0, h, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, -h, 0.0], [0.0, -1.0, 0.0]),
            ([r, 0.0, 0.0], [1.0, 0.0, 0.0]),
            ([-r, 0.0, 0.0], [-1.0, 0.0, 0.0]),
            ([0.0, 0.0, r], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, -r], [0.0, 0.0, -1.0]),
        ] {
            pos.push([c[0] + o[0], c[1] + o[1], c[2] + o[2]]);
            nor.push(n);
        }
        for (a, b) in [(2, 4), (4, 3), (3, 5), (5, 2)] {
            idx.extend_from_slice(&[base, base + a, base + b, base + 1, base + b, base + a]);
        }
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Smoke volume as an RG16F 3D texture: R extinction (1/m), G light (0..1).
fn volume_image(res: [u32; 3], density: &[f32], light: &[f32]) -> Image {
    let n = (res[0] * res[1] * res[2]) as usize;
    let mut data = Vec::with_capacity(n * 4);
    for i in 0..n {
        let d = half::f16::from_f32(density.get(i).copied().unwrap_or(0.0));
        let l = half::f16::from_f32(light.get(i).copied().unwrap_or(1.0));
        data.extend_from_slice(&d.to_le_bytes());
        data.extend_from_slice(&l.to_le_bytes());
    }
    let mut img = Image::new(
        Extent3d {
            width: res[0],
            height: res[1],
            depth_or_array_layers: res[2],
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rg16Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_w: bevy::image::ImageAddressMode::ClampToEdge,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        ..default()
    });
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_cave_smoke_does_not_receive_outdoor_sunlight() {
        let sun = Vec3::new(30_000.0, 25_000.0, 20_000.0);
        let (lit, shadow) = smoke_sky_light(sun, sun * 0.35, 0.0);
        assert_eq!(lit, Vec3::ZERO);
        assert_eq!(shadow, Vec3::ZERO);
    }

    #[test]
    fn cave_mouth_smoke_follows_local_daylight_without_changing_outdoor_light() {
        let key = Vec3::new(30_000.0, 25_000.0, 20_000.0);
        let ambient = key * 0.35;
        let (outside, outside_shadow) = smoke_sky_light(key, ambient, 1.0);
        assert_eq!(outside, key * 0.48 + ambient * 0.7);
        assert_eq!(outside_shadow, ambient * 0.4 + key * 0.04);
        let (mouth, mouth_shadow) = smoke_sky_light(key, ambient, 0.25);
        assert_eq!(mouth, outside * 0.25);
        assert_eq!(mouth_shadow, outside_shadow * 0.25);
    }
}
