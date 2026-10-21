//! A dungeon's living details: the flames of its fires — every torch, brazier, hanging fire and
//! candle cluster burns with real simulated fire (AetherFX's flames as flipbooks, `crate::flames`;
//! a green torch burns green) — the haze its hazy lights glow in (`Mood::haze`,
//! `LightDef::haze`: a soft halo round a fire, a beam of light under a shaft or a lamp;
//! `shaders/dungeon_haze.wesl`) and the dust or embers its mood fills the air with
//! (`Mood::motes`), drifting round the explorer.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use super::lights::{self, LightDef, LightKind};
use super::model::DungeonMap;
use crate::camera::MainCamera;
use crate::flames::{self, FireKind, FlipbookMaterial};
use crate::visuals::motes::{MoteExt, MoteMaterial, MoteParams};

#[derive(Component)]
pub struct Flame {
    light: u32,
    kind: FireKind,
    base: Vec3,
    height: f32,
    spread: f32,
    flicker: f32,
}

pub type HazeMaterial = ExtendedMaterial<StandardMaterial, HazeExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct HazeParams {
    /// colour (linear), w: strength
    pub color: Vec4,
    /// x: 0 halo / 1 beam, y: dust, z: seed
    pub look: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct HazeExt {
    #[uniform(100)]
    pub params: HazeParams,
}

impl MaterialExtension for HazeExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/dungeon_haze.wesl".into()
    }
}

/// A light's haze: a halo (facing the eye, breathing with the light's flicker) or a beam.
#[derive(Component)]
pub struct Haze {
    light: u32,
    at: Vec3,
    size: f32,
    flicker: f32,
    beam: bool,
}

/// What a haze was made from (rebuilt when it changes).
#[derive(Clone, Copy, PartialEq)]
struct HazeSig {
    at: Vec3,
    dir: Vec3,
    size: f32,
    len: f32,
    r0: f32,
    key: [u8; 5],
}

#[derive(Component)]
pub struct DungeonMotes;

/// How many flames burn at once (the nearest).
const FLAMES: usize = 140;
/// How many halos and beams glow at once (the nearest).
const HALOS: usize = 48;
const BEAMS: usize = 24;

/// What a flame was made from (remade when its light moves or changes colour).
type FlameSig = (Vec3, f32, FireKind, [u8; 3], u8);

#[derive(Resource, Default)]
pub struct Fx {
    flames: HashMap<(u32, u8), (Entity, FlameSig)>,
    /// the sheets' meshes by (seed, `flames::spread_key`)
    meshes: HashMap<(u8, u8), Handle<Mesh>>,
    mats: HashMap<(FireKind, [u8; 3]), Handle<FlipbookMaterial>>,
    motes: Option<(Entity, Handle<MoteMaterial>)>,
    eval: f32,
    haze: HashMap<u32, (Entity, HazeSig)>,
    halo_mesh: Option<Handle<Mesh>>,
    haze_mats: HashMap<[u8; 5], Handle<HazeMaterial>>,
    haze_eval: f32,
}

/// The flames a light has: which fire, where its foot is, how tall it burns.
pub fn flames_of(l: &LightDef) -> Vec<(FireKind, Vec3, f32)> {
    let p = l.position();
    match l.kind {
        LightKind::Torch => vec![(FireKind::Torch, p - Vec3::Y * 0.3, 0.5)],
        LightKind::Brazier => vec![(FireKind::Brazier, p - Vec3::Y * 0.45, 1.0)],
        LightKind::HangingBrazier => vec![(FireKind::Brazier, p - Vec3::Y * 0.3, 0.75)],
        LightKind::Candles => [
            Vec3::new(0.0, -0.18, 0.0),
            Vec3::new(0.14, -0.24, 0.06),
            Vec3::new(-0.12, -0.22, -0.05),
        ]
        .iter()
        .map(|o| (FireKind::Candle, p + *o, 0.075))
        .collect(),
        LightKind::Lantern | LightKind::Chandelier => {
            vec![(FireKind::Candle, p - Vec3::Y * 0.12, 0.1)]
        }
        _ => vec![],
    }
}

/// How wide (m, its radius) the bed of fire a light burns on is: a brazier's bowl spreads its
/// flame at the foot (`flames::spread`); a torch's or a candle's keeps its own width.
pub fn bed_of(kind: LightKind) -> f32 {
    match kind {
        LightKind::Brazier => 0.3,
        LightKind::HangingBrazier => 0.25,
        _ => 0.1,
    }
}

/// How bright a fire burns (display units: it glows the same at any exposure).
pub fn strength(kind: FireKind) -> f32 {
    match kind {
        FireKind::Candle => 2.6,
        _ => 2.2,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn flames(
    mut commands: Commands,
    mut fx: ResMut<Fx>,
    map: Res<DungeonMap>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(&Flame, &mut Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<FlipbookMaterial>>,
    server: Res<AssetServer>,
    burning: Res<flames::AnchorFires>,
    time: Res<Time>,
    real: Res<Time<Real>>,
) {
    let Some(m) = map.0.as_ref() else {
        for (_, (e, _)) in fx.flames.drain() {
            commands.entity(e).despawn();
        }
        return;
    };
    let eye = cam.single().map(|c| c.translation()).unwrap_or(Vec3::ZERO);
    // which flames burn (every quarter second, or when the lights changed)
    let now = real.elapsed_secs();
    if map.is_changed() || now - fx.eval > 0.25 {
        fx.eval = now;
        let mut want: Vec<(f32, u32, u8, FireKind, Vec3, f32, [f32; 3], f32, f32)> = Vec::new();
        // (a light whose fixture burns already, its painted flames or fire anchors turned to real
        // fire where they are: `crate::flames`, draws no flame of its own)
        for l in m
            .lights
            .iter()
            .filter(|l| !l.fixture.is_some_and(|o| burning.objects.contains(&o)))
        {
            let color = m.mood.tinted(l.color);
            for (k, (kind, base, height)) in flames_of(l).into_iter().enumerate() {
                let spread = flames::spread(kind, height, bed_of(l.kind));
                want.push((
                    base.distance_squared(eye),
                    l.id,
                    k as u8,
                    kind,
                    base,
                    height,
                    color,
                    l.flicker,
                    spread,
                ));
            }
        }
        want.sort_by(|a, b| a.0.total_cmp(&b.0));
        want.truncate(FLAMES);
        let keep: std::collections::HashSet<(u32, u8)> = want.iter().map(|w| (w.1, w.2)).collect();
        let stale: Vec<(u32, u8)> = fx
            .flames
            .keys()
            .filter(|k| !keep.contains(k))
            .copied()
            .collect();
        for k in stale {
            if let Some((e, _)) = fx.flames.remove(&k) {
                commands.entity(e).despawn();
            }
        }
        for (_, light, k, kind, base, height, color, flicker, spread) in want {
            // a material per fire and colour (quantised)
            let key = [
                (color[0] * 15.0) as u8,
                (color[1] * 15.0) as u8,
                (color[2] * 15.0) as u8,
            ];
            let sig = (base, height, kind, key, flames::spread_key(spread));
            match fx.flames.get(&(light, k)) {
                Some((_, old)) if *old == sig => continue,
                Some((e, _)) => {
                    commands.entity(*e).despawn();
                }
                None => {}
            }
            let mat = fx
                .mats
                .entry((kind, key))
                .or_insert_with(|| mats.add(flames::material(kind, color, strength(kind), &server)))
                .clone();
            let seed = ((light as usize * 3 + k as usize) % 8) as u8;
            let mesh = fx
                .meshes
                .entry((seed, flames::spread_key(spread)))
                .or_insert_with(|| meshes.add(flames::quad(seed as f32 / 8.0 + 0.03, spread)))
                .clone();
            let e = commands
                .spawn((
                    Name::new("Dungeon flame"),
                    Flame {
                        light,
                        kind,
                        base,
                        height,
                        spread,
                        flicker,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(mat),
                    flames::facing(kind, base, height, spread, eye),
                    NotShadowCaster,
                    NotShadowReceiver,
                ))
                .id();
            fx.flames.insert((light, k), (e, sig));
        }
    }
    // each frame: face the eye (turning about the upright), swelling a little with the light's
    // flicker (the flipbook does the burning)
    let t = time.elapsed_secs();
    for (f, mut tf) in &mut q {
        let k = lights::flicker(t * 1.3, f.light, f.flicker.max(0.25));
        *tf = flames::facing(f.kind, f.base, f.height * (0.94 + 0.06 * k), f.spread, eye);
    }
}

/// A sheet one metre square about its middle, facing +z (a halo).
fn halo_sheet() -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0f32],
        ],
    );
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0f32]; 4]);
    m.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0f32]],
    );
    m.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    m
}

/// An open cone down local −y: radius `r0` at the light, `r1` where it lands `len` below; uv.y
/// runs from the light (0) to the end (1).
fn beam_cone(r0: f32, r1: f32, len: f32) -> Mesh {
    const N: u32 = 28;
    let (mut pos, mut nrm, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..=N {
        let a = i as f32 / N as f32 * std::f32::consts::TAU;
        let (sn, cs) = a.sin_cos();
        let n = [cs, 0.0, sn];
        for (y, r, v) in [(0.0, r0, 0.0), (-len, r1, 1.0)] {
            pos.push([cs * r, y, sn * r]);
            nrm.push(n);
            uv.push([i as f32 / N as f32, v]);
        }
    }
    for i in 0..N {
        let (a, b, c, d) = (i * 2, i * 2 + 1, i * 2 + 2, i * 2 + 3);
        idx.extend_from_slice(&[a, b, d, a, d, c]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U32(idx));
    m
}

/// The haze the mood hangs in the air (`Mood::haze`) lit by the hazy lights (`LightDef::haze`):
/// a soft halo round each hazy fire or lamp, a beam of light under each hazy shaft or spot (down
/// to where it lands). The nearest few; none above the editing view's cut.
#[allow(clippy::too_many_arguments)]
pub(super) fn haze(
    mut commands: Commands,
    mut fx: ResMut<Fx>,
    map: Res<DungeonMap>,
    caves: Res<crate::caves::render::CaveWorld>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(&Haze, &mut Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<HazeMaterial>>,
    time: Res<Time>,
    real: Res<Time<Real>>,
    walk: Res<crate::walk::WalkState>,
    graphics: Res<crate::streaming::Graphics>,
) {
    let mut amount = map
        .0
        .as_ref()
        .filter(|m| m.kind == super::model::MapKind::Interior)
        .map(|m| m.mood.haze)
        .unwrap_or(0.0);
    // (walking at normal detail and up the haze is real volumetric fog: `render::volumetric`)
    if walk.active && graphics.detail >= 1.0 {
        amount = 0.0;
    }
    let (Some(m), Some(space)) = (
        map.0.as_ref().filter(|_| amount > 0.01),
        caves.dungeon.as_ref(),
    ) else {
        for (_, (e, _)) in fx.haze.drain() {
            commands.entity(e).despawn();
        }
        return;
    };
    let eye = cam.single().map(|c| c.translation()).unwrap_or(Vec3::ZERO);
    let now = real.elapsed_secs();
    if map.is_changed() || caves.is_changed() || now - fx.haze_eval > 0.25 {
        fx.haze_eval = now;
        let cut = space.cut.unwrap_or(f32::INFINITY);
        // the whole space, uncut, for where a beam lands
        let whole = super::space::Space {
            cut: None,
            ..space.clone()
        };
        let dust = (m.mood.motes * 1.4).clamp(0.3, 1.0);
        let mut halos: Vec<(f32, &LightDef)> = Vec::new();
        let mut beams: Vec<(f32, &LightDef)> = Vec::new();
        for l in m
            .lights
            .iter()
            .filter(|l| l.haze && l.position().y < cut + 0.5)
        {
            let d = l.position().distance_squared(eye);
            if l.cone > 0.0 {
                beams.push((d, l))
            } else {
                halos.push((d, l))
            }
        }
        halos.sort_by(|a, b| a.0.total_cmp(&b.0));
        beams.sort_by(|a, b| a.0.total_cmp(&b.0));
        halos.truncate(HALOS);
        beams.truncate(BEAMS);
        let mut keep = std::collections::HashSet::new();
        for (beam, l) in halos
            .iter()
            .map(|x| (false, x.1))
            .chain(beams.iter().map(|x| (true, x.1)))
        {
            keep.insert(l.id);
            let p = l.position();
            let c = Color::srgb_from_array(m.mood.tinted(l.color)).to_linear();
            let c = Vec3::new(c.red, c.green, c.blue).normalize_or(Vec3::ONE.normalize());
            // brighter lights glow harder (gently)
            let base = if beam { 3_500_000.0 } else { 650_000.0 };
            let k = (m.all_lights.apply(l).0 / base).powf(0.3).clamp(0.4, 1.8);
            let strength = amount * k * if beam { 0.42 } else { 0.75 };
            let key = [
                (c.x * 15.0) as u8,
                (c.y * 15.0) as u8,
                (c.z * 15.0) as u8,
                (strength * 40.0).min(255.0) as u8,
                beam as u8,
            ];
            let (dir, size, len, r0) = if beam {
                let dir = l.direction();
                let len = whole
                    .raycast(p + dir * 0.3, dir, l.range * 1.5)
                    .map(|(_, t)| t + 0.3)
                    .unwrap_or(l.range)
                    .clamp(1.5, l.range * 1.5);
                let r0 = if l.kind == LightKind::Shaft { 0.9 } else { 0.1 };
                (dir, 0.0, len, r0)
            } else {
                // round the light, but no deeper than it hangs over the floor (the sheet would cut it)
                let over = space
                    .floor_under(p, 30.0)
                    .map(|f| p.y - f)
                    .unwrap_or(2.0)
                    .max(0.4);
                (
                    Vec3::NEG_Y,
                    (l.range * 0.22).clamp(1.0, 5.0).min(over * 2.2),
                    0.0,
                    0.0,
                )
            };
            let sig = HazeSig {
                at: p,
                dir,
                size,
                len,
                r0,
                key,
            };
            match fx.haze.get(&l.id) {
                Some((_, old)) if *old == sig => continue,
                Some((e, _)) => {
                    commands.entity(*e).despawn();
                }
                None => {}
            }
            let mat = fx
                .haze_mats
                .entry(key)
                .or_insert_with(|| {
                    mats.add(ExtendedMaterial {
                        base: StandardMaterial {
                            alpha_mode: AlphaMode::Add,
                            unlit: true,
                            cull_mode: None,
                            ..default()
                        },
                        extension: HazeExt {
                            params: HazeParams {
                                color: c.extend(strength),
                                look: Vec4::new(beam as u8 as f32, dust, (l.id % 97) as f32, 0.0),
                            },
                        },
                    })
                })
                .clone();
            let (mesh, tf) = if beam {
                let r1 = r0 + len * (l.cone.max(4.0) * 0.5).to_radians().tan() * 0.85;
                (
                    meshes.add(beam_cone(r0, r1, len)),
                    Transform::from_translation(p)
                        .with_rotation(Quat::from_rotation_arc(Vec3::NEG_Y, dir)),
                )
            } else {
                (
                    fx.halo_mesh
                        .get_or_insert_with(|| meshes.add(halo_sheet()))
                        .clone(),
                    Transform::from_translation(p).with_scale(Vec3::splat(size)),
                )
            };
            let e = commands
                .spawn((
                    Name::new(if beam {
                        "Dungeon light beam"
                    } else {
                        "Dungeon halo"
                    }),
                    Haze {
                        light: l.id,
                        at: p,
                        size,
                        flicker: l.flicker,
                        beam,
                    },
                    Mesh3d(mesh),
                    MeshMaterial3d(mat),
                    tf,
                    NotShadowCaster,
                    NotShadowReceiver,
                ))
                .id();
            fx.haze.insert(l.id, (e, sig));
        }
        let stale: Vec<u32> = fx
            .haze
            .keys()
            .filter(|k| !keep.contains(k))
            .copied()
            .collect();
        for k in stale {
            if let Some((e, _)) = fx.haze.remove(&k) {
                commands.entity(e).despawn();
            }
        }
    }
    // each frame: halos face the eye and breathe with their light
    let t = time.elapsed_secs();
    for (h, mut tf) in &mut q {
        if h.beam {
            continue;
        }
        let k = lights::flicker(t * 1.3, h.light, h.flicker);
        tf.translation = h.at;
        tf.rotation = Quat::from_rotation_arc(Vec3::Z, (eye - h.at).normalize_or(Vec3::Z));
        tf.scale = Vec3::splat(h.size * (0.92 + 0.08 * k));
    }
}

/// Dust (or embers in hot moods) drifting in the air round the explorer, as the mood says.
#[allow(clippy::too_many_arguments)]
pub(super) fn motes(
    mut commands: Commands,
    mut fx: ResMut<Fx>,
    map: Res<DungeonMap>,
    walk: Res<crate::walk::WalkState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<MoteMaterial>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut tf: Query<&mut Transform, With<DungeonMotes>>,
) {
    let want = map.0.as_ref().filter(|m| {
        m.kind == super::model::MapKind::Interior && walk.active && m.mood.motes > 0.01
    });
    let Some(m) = want else {
        if let Some((e, _)) = fx.motes.take() {
            commands.entity(e).despawn();
        }
        return;
    };
    if fx.motes.is_none() {
        let mat = mats.add(MoteMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE,
                alpha_mode: AlphaMode::Add,
                unlit: true,
                cull_mode: None,
                ..default()
            },
            extension: MoteExt {
                params: MoteParams {
                    color: Vec4::ZERO,
                    shape: Vec4::new(30.0, 0.05, 0.1, 1.0),
                },
            },
        });
        let e = commands
            .spawn((
                Name::new("Dungeon motes"),
                DungeonMotes,
                Mesh3d(meshes.add(crate::visuals::motes::field_mesh())),
                MeshMaterial3d(mat.clone()),
                Transform::IDENTITY,
                NotShadowCaster,
                NotShadowReceiver,
                bevy::camera::visibility::NoFrustumCulling,
            ))
            .id();
        fx.motes = Some((e, mat));
    }
    let Some((_, h)) = fx.motes.clone() else {
        return;
    };
    // hot moods: embers rising; the rest: pale dust hanging in the air
    let hot = matches!(m.mood.theme.as_str(), "forge" | "infernal");
    let (color, size, rise) = if hot {
        (Vec3::new(1.0, 0.45, 0.12) * 1.6, 0.045, 0.6)
    } else {
        (Vec3::new(0.75, 0.68, 0.58) * 0.35, 0.035, 0.03)
    };
    if let Some(mut mm) = mats.get_mut(&h) {
        let p = MoteParams {
            color: color.extend(m.mood.motes.clamp(0.0, 1.0)),
            shape: Vec4::new(26.0, size, rise, if hot { 2.0 } else { 0.4 }),
        };
        if mm.extension.params.color != p.color || mm.extension.params.shape != p.shape {
            mm.extension.params = p;
        }
    }
    if let (Ok(c), Ok(mut t)) = (cam.single(), tf.single_mut()) {
        t.translation = c.translation();
    }
}
