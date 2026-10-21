//! Lightning: random strikes in thunderstorms (and, rarely, in heavy rain). A strike is a few
//! quick pulses (a dim leader, then two or three return strokes) that light the scene for a
//! moment, a jagged bolt far off under the clouds, and thunder a little later (sound travels at
//! 343 m/s: a strike 2 km away is heard six seconds after it is seen).
//!
//! How it lights the world without fighting `sky.rs`, which rewrites the sun, ambient and
//! exposure every frame: its own directional light ("Lightning", hidden between strikes so it
//! costs nothing) shining down from the strike, and a boost of the ambient fill added after the
//! sky systems have run (`GlobalAmbientLight`); the volumetric clouds take their fill from that
//! ambient, so the overcast lights up with it. Both scale with the camera's exposure, so a strike
//! reads the same by day and blinding at night. `Lightning::flash` is public for anything else
//! that wants to flicker with it.
//!
//! Strikes come from a seeded random sequence (`SEED`), so automated runs are repeatable;
//! `weather_set {"lightning_now": true}` strikes at once, in front of the camera.
//! Thunder: `assets/audio/ambience/thunder_*.mp3`, synthesised for Skyfold by
//! `tools/audio/gen_thunder.py` (no recordings).

use bevy::asset::RenderAssetUsages;
use bevy::audio::{PlaybackSettings, Volume};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use super::RainState;
use crate::camera::MainCamera;
use crate::noise::smoothstep;
use crate::sky::{Lighting, SkyState};

/// The strike sequence's seed (repeatable runs).
const SEED: u64 = 0x5EED_B017;
/// Speed of sound (m/s).
const SOUND: f32 = 343.0;
/// Peak illuminance of a strike, in units of the illuminance that exposes as mid-grey.
const FLASH_KEY: f32 = 13.0;
/// Peak ambient boost, in the same units.
const FLASH_FILL: f32 = 3.5;
/// Thunder takes (`tools/audio/gen_thunder.py`): far rumbles, then near cracks.
const THUNDER_FAR: [&str; 3] = ["thunder_far_0", "thunder_far_1", "thunder_far_2"];
const THUNDER_NEAR: [&str; 2] = ["thunder_near_0", "thunder_near_1"];

pub(super) fn register(app: &mut App) {
    app.add_plugins(MaterialPlugin::<BoltMaterial>::default())
        .insert_resource(Lightning::new(SEED))
        .add_systems(Startup, spawn)
        .add_systems(
            Update,
            (strike, light)
                .chain()
                .in_set(LightningSystems)
                .after(super::track)
                .after(crate::sky::SkySystems),
        );
}

/// The strike and its light (after the sky systems: it adds to their light).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct LightningSystems;

/// The storm's lightning now.
#[derive(Resource)]
pub struct Lightning {
    /// Brightness of the flash this frame: 0 dark … ≈1 a near strike at its peak.
    pub flash: f32,
    /// Strikes so far (MCP / tests).
    pub strikes: u32,
    /// Strike at once (MCP `lightning_now`), in front of the camera.
    pub now: bool,
    rng: fastrand::Rng,
    /// seconds until the next random strike
    wait: f32,
    strike: Option<Strike>,
    /// thunder on its way: (seconds left, distance m, strength)
    thunder: Vec<(f32, f32, f32)>,
}

struct Strike {
    age: f32,
    /// (start s, peak) of each pulse
    pulses: Vec<(f32, f32)>,
    /// direction towards the strike from the camera (for the light)
    dir: Vec3,
    /// 0..1: nearer strikes light the scene more
    strength: f32,
}

impl Strike {
    fn flash(&self) -> f32 {
        self.pulses
            .iter()
            .map(|&(t0, a)| a * pulse(self.age - t0))
            .sum::<f32>()
            .min(1.5)
    }
    fn done(&self) -> bool {
        self.pulses.last().is_none_or(|p| self.age > p.0 + 0.6)
    }
}

/// One stroke's brightness `t` seconds after it starts: a near-instant rise, a fast decay with a
/// faint afterglow.
fn pulse(t: f32) -> f32 {
    if t < 0.0 {
        0.0
    } else if t < 0.015 {
        t / 0.015
    } else {
        let x = t - 0.015;
        0.85 * (-x / 0.055).exp() + 0.15 * (-x / 0.25).exp()
    }
}

impl Lightning {
    fn new(seed: u64) -> Self {
        Self {
            flash: 0.0,
            strikes: 0,
            now: false,
            rng: fastrand::Rng::with_seed(seed),
            wait: 4.0,
            strike: None,
            thunder: vec![],
        }
    }

    /// Strikes a minute for this weather (`RainSettings::lightning` × the storm).
    pub fn rate(sky: &SkyState, frequency: f32) -> f32 {
        let w = &sky.weather;
        // a thunderstorm (rain ≈ 1 under a heavy, dark sky) …
        let storm = smoothstep(0.85, 0.97, w.rain) * smoothstep(0.66, 0.76, w.dim);
        // … or now and then in heavy rain
        let heavy = smoothstep(0.6, 0.9, w.rain);
        frequency.max(0.0) * (5.0 * storm + 0.25 * heavy)
    }
}

/// The flash light and the bolt.
#[derive(Component)]
struct FlashLight;
#[derive(Component)]
struct Bolt;
#[derive(Resource)]
struct BoltHandle(Handle<BoltMaterial>);

#[derive(Clone, Debug, ShaderType, Default, PartialEq)]
pub struct BoltParams {
    /// rgb: the core's colour × the flash now
    pub color: Vec4,
    /// `volumetrics::FroxelFog::params` (w 0: off)
    pub fog: Vec4,
}

/// The bolt (`bolt.wesl`): additive ribbons that dim themselves by the volumetric light in front
/// of them (drawn after the view was fogged), and skip Bevy's distance fog (a strike is brighter
/// than anything in the storm's haze).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct BoltMaterial {
    #[uniform(0)]
    pub params: BoltParams,
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    pub fog_volume: Option<Handle<Image>>,
}

impl Material for BoltMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/bolt.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    /// (the ribbons face the camera of the moment they struck: either side may be seen)
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

fn spawn(mut commands: Commands, mut mats: ResMut<Assets<BoltMaterial>>) {
    commands.spawn((
        Name::new("Lightning"),
        FlashLight,
        DirectionalLight {
            illuminance: 0.0,
            color: Color::srgb(0.82, 0.88, 1.0),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 1.0, 0.0).looking_at(Vec3::ZERO, Vec3::Z),
        Visibility::Hidden,
    ));
    let m = mats.add(BoltMaterial {
        params: BoltParams::default(),
        fog_volume: None,
    });
    commands.insert_resource(BoltHandle(m));
}

#[allow(clippy::too_many_arguments)]
fn strike(
    mut commands: Commands,
    mut lt: ResMut<Lightning>,
    sky: Res<SkyState>,
    lighting: Res<Lighting>,
    rain: Res<RainState>,
    time: Res<Time>,
    terrain: Res<crate::terrain::Terrain>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    bolts: Query<Entity, With<Bolt>>,
    bolt_mat: Option<Res<BoltHandle>>,
    mut meshes: ResMut<Assets<Mesh>>,
    server: Res<AssetServer>,
    audio: Res<crate::audio::AudioSettings>,
) {
    let dt = time.delta_secs().min(0.1);
    let Ok(cam) = cam.single() else { return };
    let eye = cam.translation();
    let l = &mut *lt;

    // thunder arriving
    let mut arrived = vec![];
    l.thunder.retain_mut(|t| {
        t.0 -= dt;
        if t.0 <= 0.0 {
            arrived.push((t.1, t.2));
        }
        t.0 > 0.0
    });
    for (dist, strength) in arrived {
        if !lighting.rain.thunder {
            continue;
        }
        let near = dist < 900.0;
        let takes: &[&str] = if near { &THUNDER_NEAR } else { &THUNDER_FAR };
        let stem = takes[l.rng.usize(..takes.len())];
        let rel = format!("audio/ambience/{stem}.mp3");
        let vol = if audio.muted {
            0.0
        } else {
            audio.master * audio.ambience
        } * (0.35 + 0.65 * strength)
            * (1.0 - smoothstep(1500.0, 6000.0, dist) * 0.7);
        if vol > 0.002 {
            commands.spawn((
                AudioPlayer::<AudioSource>(server.load(rel)),
                PlaybackSettings::DESPAWN
                    .with_volume(Volume::Linear(vol * 1.6))
                    .with_speed(0.9 + l.rng.f32() * 0.2),
            ));
        }
    }

    // the strike in progress
    if let Some(s) = &mut l.strike {
        s.age += dt;
        if s.done() {
            l.strike = None;
            for e in &bolts {
                commands.entity(e).despawn();
            }
        }
    }

    // a new one: forced, or at random (an exponential wait at the storm's rate)
    let rate = Lightning::rate(&sky, lighting.rain.lightning) * rain.rain.max(0.3);
    let forced = std::mem::take(&mut l.now);
    if rate > 0.0 && l.strike.is_none() {
        l.wait -= dt;
    }
    let due = rate > 0.0 && l.wait <= 0.0 && l.strike.is_none();
    if !forced && !due {
        return;
    }
    // the next wait: exponential at `rate` per minute
    l.wait = if rate > 0.0 {
        (-(1.0 - l.rng.f32()).max(1e-4).ln() * 60.0 / rate).max(1.5)
    } else {
        10.0
    };
    // where: forced strikes in front of the camera (so they are seen), the others anywhere
    // round the viewer, though more often ahead
    let fwd = cam.forward().as_vec3();
    let yaw_fwd = fwd.z.atan2(fwd.x);
    let ahead = forced || l.rng.f32() < 0.5;
    let yaw = if ahead {
        yaw_fwd + (l.rng.f32() - 0.5) * 0.6
    } else {
        l.rng.f32() * std::f32::consts::TAU
    };
    // mostly far off; now and then close
    let dist = if forced {
        1300.0 + l.rng.f32() * 500.0
    } else if l.rng.f32() < 0.2 {
        450.0 + l.rng.f32() * 600.0
    } else {
        1100.0 + l.rng.f32() * 2400.0
    };
    let ground_xz = eye.xz() + Vec2::new(yaw.cos(), yaw.sin()) * dist;
    let ground_y = if terrain.in_bounds(ground_xz.x, ground_xz.y) {
        terrain
            .height_at(ground_xz.x, ground_xz.y)
            .max(terrain.sea_level)
    } else {
        terrain.sea_level
    };
    let ground = Vec3::new(ground_xz.x, ground_y, ground_xz.y);
    let strength = 1.0 - smoothstep(400.0, 3600.0, dist) * 0.7;
    // a leader flicker, then 2–4 return strokes
    let mut pulses = vec![(0.0, 0.18 + l.rng.f32() * 0.12)];
    let mut t = 0.05 + l.rng.f32() * 0.05;
    let strokes = 2 + l.rng.usize(..3);
    for k in 0..strokes {
        pulses.push((t, (1.0 - k as f32 * 0.18) * (0.75 + l.rng.f32() * 0.25)));
        t += 0.06 + l.rng.f32() * 0.14;
    }
    let top = Vec3::new(
        ground.x,
        crate::clouds::CLOUD_BASE.max(ground.y + 600.0),
        ground.z,
    );
    let dir = (top.lerp(ground, 0.3) - eye).normalize_or(Vec3::Y);
    l.strike = Some(Strike {
        age: 0.0,
        pulses,
        dir,
        strength,
    });
    l.strikes += 1;
    l.thunder.push((dist / SOUND, dist, strength));
    // the bolt
    for e in &bolts {
        commands.entity(e).despawn();
    }
    if let Some(m) = bolt_mat {
        let mesh = bolt_mesh(&mut l.rng, top, ground, eye);
        commands.spawn((
            Name::new("Lightning bolt"),
            Bolt,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(m.0.clone()),
            Transform::IDENTITY,
            NoFrustumCulling,
            NotShadowCaster,
            NotShadowReceiver,
        ));
    }
}

/// Lights the scene with the flash (after the sky systems set this frame's light).
#[allow(clippy::type_complexity)]
fn light(
    mut lt: ResMut<Lightning>,
    mut ambient: ResMut<GlobalAmbientLight>,
    cam: Query<&bevy::camera::Exposure, With<MainCamera>>,
    mut q: Query<(&mut DirectionalLight, &mut Transform, &mut Visibility), With<FlashLight>>,
    bolt_mat: Option<Res<BoltHandle>>,
    mut mats: ResMut<Assets<BoltMaterial>>,
    indoors: Option<Res<crate::motion::indoors::Indoors>>,
    fog: Option<Res<crate::volumetrics::FroxelFog>>,
) {
    let (flash, dir, strength) = lt
        .strike
        .as_ref()
        .map(|s| (s.flash(), s.dir, s.strength))
        .unwrap_or((0.0, Vec3::Y, 0.0));
    lt.flash = flash * strength;
    let Ok((mut l, mut t, mut vis)) = q.single_mut() else {
        return;
    };
    let on = lt.flash > 0.002;
    let want = if on {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if *vis != want {
        *vis = want;
    }
    if let Some(m) = &bolt_mat {
        let c = if on {
            Vec4::new(6.0, 6.6, 9.0, 1.0) * (flash.min(1.2) * (0.5 + 0.5 * strength))
        } else {
            Vec4::ZERO
        };
        let (fog_volume, fog) = super::froxel(fog.as_deref());
        let want = BoltParams { color: c, fog };
        // (the fog's reach moves with the editor camera's zoom: only a real change counts)
        let stale = |mm: &BoltMaterial| {
            mm.params.color != want.color
                || mm.fog_volume != fog_volume
                || (mm.params.fog - fog).abs().max_element() > 0.01 * fog.y.abs().max(0.1)
        };
        if mats.get(&m.0).is_some_and(stale)
            && let Some(mut mm) = mats.get_mut(&m.0)
        {
            mm.params = want;
            mm.fog_volume = fog_volume;
        }
    }
    if !on {
        return;
    }
    // (illuminance that exposes as mid-grey, from the camera's exposure: ≈ 2^ev100 lux)
    let ev = cam.single().map(|e| e.ev100).unwrap_or(13.0);
    let grey = 2f32.powf(ev);
    // the light comes down from high in the strike's direction (sky-wide, so steep)
    let from = Vec3::new(dir.x * 0.6, 1.0, dir.z * 0.6).normalize();
    *t = Transform::IDENTITY.looking_to(
        -from,
        if from.y.abs() > 0.98 {
            Vec3::Z
        } else {
            Vec3::Y
        },
    );
    // indoors only what comes through the windows and doors
    let inside = indoors.map(|i| i.amount.max(i.cave)).unwrap_or(0.0);
    let k = lt.flash * (1.0 - 0.8 * inside);
    l.illuminance = grey * FLASH_KEY * k;
    ambient.brightness += grey * FLASH_FILL * k;
    let flash_col = Color::srgb(0.8, 0.86, 1.0);
    let share = (grey * FLASH_FILL * k / ambient.brightness.max(1e-3)).min(1.0);
    ambient.color = ambient.color.mix(&flash_col, share);
}

/// A jagged bolt from `top` to `ground` (a main channel by midpoint displacement, a few forks),
/// as ribbons facing `eye`: a bright core and a wide soft glow (colour falls to black at the
/// ribbon's edges; the material is additive).
fn bolt_mesh(rng: &mut fastrand::Rng, top: Vec3, ground: Vec3, eye: Vec3) -> Mesh {
    let mut pos: Vec<[f32; 3]> = vec![];
    let mut col: Vec<[f32; 4]> = vec![];
    let mut idx: Vec<u32> = vec![];
    let jag = |rng: &mut fastrand::Rng, a: Vec3, b: Vec3, depth: u32, rough: f32| -> Vec<Vec3> {
        let mut pts = vec![a, b];
        for _ in 0..depth {
            let mut next = Vec::with_capacity(pts.len() * 2);
            for w in pts.windows(2) {
                let len = w[0].distance(w[1]);
                let m = (w[0] + w[1]) * 0.5
                    + Vec3::new(rng.f32() - 0.5, (rng.f32() - 0.5) * 0.3, rng.f32() - 0.5)
                        * len
                        * rough;
                next.push(w[0]);
                next.push(m);
            }
            next.push(*pts.last().unwrap());
            pts = next;
        }
        pts
    };
    let mut ribbon = |pts: &[Vec3], width: f32, glow: f32, bright: f32| {
        for (half, c) in [(width * 0.5, bright), (width * glow, bright * 0.12)] {
            let base = pos.len() as u32;
            for (i, p) in pts.iter().enumerate() {
                let d = if i + 1 < pts.len() {
                    pts[i + 1] - *p
                } else {
                    *p - pts[i - 1]
                };
                let side = d.cross(eye - *p).normalize_or(Vec3::X) * half;
                // thins towards its lower end a little
                let f = 1.0 - 0.4 * i as f32 / pts.len() as f32;
                for (q, k) in [(*p - side * f, 0.0), (*p, c), (*p + side * f, 0.0)] {
                    pos.push(q.to_array());
                    col.push([k, k, k, 1.0]);
                }
            }
            for i in 0..pts.len() as u32 - 1 {
                let a = base + i * 3;
                let b = a + 3;
                idx.extend_from_slice(&[
                    a,
                    b,
                    a + 1,
                    a + 1,
                    b,
                    b + 1,
                    a + 1,
                    b + 1,
                    a + 2,
                    a + 2,
                    b + 1,
                    b + 2,
                ]);
            }
        }
    };
    let main = jag(rng, top, ground, 7, 0.32);
    let len = top.distance(ground);
    ribbon(&main, (len * 0.004).clamp(2.0, 6.0), 7.0, 1.0);
    for _ in 0..2 + rng.usize(..3) {
        let at = 8 + rng.usize(..main.len() / 2);
        let from = main[at];
        let away = Vec3::new(rng.f32() - 0.5, 0.0, rng.f32() - 0.5).normalize_or(Vec3::X);
        let to = from
            + (away * (0.25 + rng.f32() * 0.3) - Vec3::Y * (0.25 + rng.f32() * 0.35))
                * (from.y - ground.y).max(50.0);
        let fork = jag(rng, from, to, 5, 0.38);
        ribbon(&fork, (len * 0.002).clamp(1.0, 3.0), 5.0, 0.45);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storms_strike_and_clear_skies_do_not() {
        let mut sky = SkyState::default();
        assert_eq!(Lightning::rate(&sky, 1.0), 0.0);
        sky.weather = crate::world::Weather::Storm.params();
        assert!(Lightning::rate(&sky, 1.0) > 3.0);
        assert_eq!(Lightning::rate(&sky, 0.0), 0.0);
        sky.weather = crate::world::Weather::Rain.params();
        let r = Lightning::rate(&sky, 1.0);
        assert!(r > 0.0 && r < 0.5, "{r}");
    }

    #[test]
    fn a_strike_flashes_then_fades() {
        let s = Strike {
            age: 0.0,
            pulses: vec![(0.0, 0.2), (0.08, 1.0)],
            dir: Vec3::Y,
            strength: 1.0,
        };
        let at = |t: f32| {
            Strike {
                age: t,
                pulses: s.pulses.clone(),
                dir: s.dir,
                strength: 1.0,
            }
            .flash()
        };
        assert!(at(0.095) > 0.8);
        assert!(at(0.5) < 0.1);
        assert!(Strike { age: 1.0, ..s }.done());
    }

    #[test]
    fn bolt_mesh_is_well_formed() {
        let mut rng = fastrand::Rng::with_seed(1);
        let m = bolt_mesh(
            &mut rng,
            Vec3::new(0.0, 1500.0, 2000.0),
            Vec3::new(0.0, 10.0, 2000.0),
            Vec3::ZERO,
        );
        let n = m.count_vertices() as u32;
        let Some(Indices::U32(i)) = m.indices() else {
            panic!()
        };
        assert!(n > 100 && i.iter().all(|&k| k < n) && i.len() % 3 == 0);
    }
}
