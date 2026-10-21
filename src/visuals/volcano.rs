//! Volcanoes come alive on their own: any lake or river of lava (the Molten behaviour, e.g. the
//! Lava preset) glows on its surroundings and smokes.
//!
//! * Glow: the lava's own emissive surface (with bloom) and the ground around it, which the
//!   terrain shader lights orange from the shore map (most of all at night).
//! * Plumes: a lava lake breathes a tall column of ash and steam that rises, spreads and bends
//!   downwind; lava rivers trail thin wisps of heat smoke; where lava runs into water it bursts
//!   into white steam. Each plume is one mesh of camera-facing puffs animated in `plume.wesl` and
//!   lit through Bevy PBR, so it takes the scene's sun, sky, fog and night.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::noise::Rng;
use crate::terrain::Terrain;
use crate::water::{WaterBehaviour, WaterBodies};

pub type PlumeMaterial = ExtendedMaterial<StandardMaterial, PlumeExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct PlumeParams {
    /// xy: wind direction (xz), z: wind speed (m/s), w: rise time of a puff (s)
    pub wind: Vec4,
    /// x: column height (m), y: source radius (m), z: spread with height (×), w: puff opacity
    pub shape: Vec4,
    /// rgb: colour at the source, a: colour high up (grey level)
    pub color: Vec4,
    /// rgb: glow of the lava below on the low puffs (display units), w: night (0..1)
    pub glow: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct PlumeExt {
    #[uniform(100)]
    pub params: PlumeParams,
    #[texture(101)]
    #[sampler(102)]
    pub noise: Handle<Image>,
}

impl MaterialExtension for PlumeExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/plume.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/plume.wesl".into()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PlumeKind {
    /// a volcano's column of ash and steam over a lava lake
    Ash,
    /// heat smoke trailing off a lava flow
    Wisp,
    /// lava running into water
    Steam,
}

struct Emitter {
    kind: PlumeKind,
    pos: Vec3,
    /// size of the source (m)
    radius: f32,
}

#[derive(Component)]
pub struct Plume {
    material: Handle<PlumeMaterial>,
}

/// What was built last (the water's lava bodies), so plumes rebuild only when it changes.
#[derive(Resource, Default)]
pub struct VolcanoState {
    key: u64,
}

fn lava_key(water: &WaterBodies) -> u64 {
    let mut k = 1469598103934665603u64;
    let mut mix = |v: f32| k = (k ^ v.to_bits() as u64).wrapping_mul(1099511628211);
    for l in water
        .lakes
        .iter()
        .filter(|l| l.style.behaviour == WaterBehaviour::Molten)
    {
        mix(l.id as f32);
        mix(l.level);
        mix(l.seed[0]);
        mix(l.outline.len() as f32);
    }
    for r in water
        .rivers
        .iter()
        .filter(|r| r.style.behaviour == WaterBehaviour::Molten)
    {
        mix(r.id as f32);
        mix(r.points.len() as f32);
        mix(r.width);
        if let Some(p) = r.points.last() {
            mix(p[0]);
            mix(p[2]);
        }
    }
    k
}

fn emitters(t: &Terrain, water: &WaterBodies) -> Vec<Emitter> {
    let mut out = vec![];
    for l in water
        .lakes
        .iter()
        .filter(|l| l.style.behaviour == WaterBehaviour::Molten)
    {
        let (centre, radius) = if l.outline.len() >= 3 {
            let pts: Vec<Vec2> = l.outline.iter().map(|p| Vec2::from_array(*p)).collect();
            let c = pts.iter().copied().sum::<Vec2>() / pts.len() as f32;
            let r = pts.iter().map(|p| p.distance(c)).sum::<f32>() / pts.len() as f32;
            (c, r)
        } else {
            (Vec2::from_array(l.seed), 20.0)
        };
        out.push(Emitter {
            kind: PlumeKind::Ash,
            pos: Vec3::new(centre.x, l.level, centre.y),
            radius: radius.clamp(6.0, 120.0),
        });
    }
    for r in water
        .rivers
        .iter()
        .filter(|r| r.style.behaviour == WaterBehaviour::Molten)
    {
        let mut along = 0.0;
        let mut next_wisp = 25.0;
        for w in r.points.windows(2) {
            let (a, b) = (Vec3::from_array(w[0]), Vec3::from_array(w[1]));
            let seg = a.distance(b);
            while next_wisp <= along + seg {
                let p = a.lerp(b, ((next_wisp - along) / seg.max(1e-3)).clamp(0.0, 1.0));
                out.push(Emitter {
                    kind: PlumeKind::Wisp,
                    pos: p,
                    radius: r.width * 0.35,
                });
                next_wisp += 70.0;
            }
            along += seg;
        }
        // lava running into the sea or a lake: steam
        if let Some(end) = r.points.last() {
            let e = Vec3::from_array(*end);
            let other = water
                .lakes
                .iter()
                .filter(|l| l.style.behaviour != WaterBehaviour::Molten)
                .any(|l| {
                    (l.level - e.y).abs() < 2.0
                        && water
                            .water_level(Vec3::new(e.x, t.height_at(e.x, e.z), e.z))
                            .is_some_and(|v| (v - l.level).abs() < 0.05)
                });
            if e.y <= t.sea_level + 1.5 || other {
                out.push(Emitter {
                    kind: PlumeKind::Steam,
                    pos: e,
                    radius: r.width * 0.6,
                });
            }
        }
    }
    out
}

/// Puffs of one plume: camera-facing quads expanded in the vertex shader. Vertex colour: (phase
/// 0..1, seed, size (m), opacity); position: where the puff leaves the source.
fn plume_mesh(e: &Emitter, seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let (count, size) = match e.kind {
        PlumeKind::Ash => (220, (e.radius * 0.8).clamp(12.0, 50.0)),
        PlumeKind::Wisp => (16, (e.radius * 0.9).clamp(2.5, 7.0)),
        PlumeKind::Steam => (40, (e.radius * 1.2).clamp(4.0, 16.0)),
    };
    let mut pos = vec![];
    let mut uv = vec![];
    let mut col = vec![];
    let mut nor = vec![];
    let mut idx = vec![];
    for k in 0..count {
        let a = rng.f32() * std::f32::consts::TAU;
        let r = e.radius * 0.55 * rng.f32().sqrt();
        let c = e.pos + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
        // evenly spread phases (jittered), so the column is never empty or clumped
        let phase = (k as f32 + rng.f32() * 0.8) / count as f32;
        let s = size * rng.range(0.7, 1.3);
        let base = pos.len() as u32;
        let sd = rng.f32();
        let op = rng.range(0.7, 1.0);
        for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            pos.push(c.to_array());
            uv.push(corner);
            nor.push([0.0, 1.0, 0.0]);
            col.push([phase, sd, s, op]);
        }
        idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
    }
    // two unused corner vertices: the mesh's bounds cover the whole column as it rises and drifts
    // (GPU culling goes by them; the puffs themselves are expanded in the shader)
    let (h, r) = match e.kind {
        PlumeKind::Ash => ((e.radius * 16.0).clamp(220.0, 700.0), 500.0),
        PlumeKind::Wisp => (22.0, 40.0),
        PlumeKind::Steam => (45.0, 70.0),
    };
    for c in [
        e.pos - Vec3::new(r + h * 0.5, 2.0, r + h * 0.5),
        e.pos + Vec3::new(r + h * 0.5, h * 1.2, r + h * 0.5),
    ] {
        pos.push(c.to_array());
        uv.push([0.0, 0.0]);
        nor.push([0.0, 1.0, 0.0]);
        col.push([0.0, 0.5, 0.0, 0.0]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

fn plume_params(
    kind: PlumeKind,
    radius: f32,
    wind: &crate::objects::Wind,
    night: f32,
) -> PlumeParams {
    let dir = wind.dir.normalize_or(Vec2::X);
    let speed = wind.speed.clamp(0.5, 12.0);
    let (rise, shape, color, glow) = match kind {
        // tall: ~16 × the crater's size, up to 650 m; slow, heavy, grey-brown ash over white steam
        PlumeKind::Ash => (
            110.0,
            Vec4::new((radius * 16.0).clamp(220.0, 700.0), radius * 0.6, 0.3, 0.7),
            Vec4::new(0.23, 0.22, 0.215, 0.6),
            Vec3::new(1.0, 0.3, 0.06) * 0.4,
        ),
        PlumeKind::Wisp => (
            14.0,
            Vec4::new(22.0, radius, 0.35, 0.12),
            Vec4::new(0.55, 0.53, 0.5, 0.72),
            Vec3::ZERO,
        ),
        PlumeKind::Steam => (
            16.0,
            Vec4::new(45.0, radius, 0.45, 0.5),
            Vec4::new(0.86, 0.88, 0.9, 0.95),
            Vec3::ZERO,
        ),
    };
    PlumeParams {
        wind: Vec4::new(dir.x, dir.y, speed, rise),
        shape,
        color,
        glow: glow.extend(night),
    }
}

/// Rebuilds the plumes when the lava bodies change.
#[allow(clippy::too_many_arguments)]
pub fn rebuild_volcanoes(
    mut commands: Commands,
    mut state: ResMut<VolcanoState>,
    water: Res<WaterBodies>,
    terrain: Res<Terrain>,
    wind: Res<crate::objects::Wind>,
    sky: Res<crate::sky::SkyState>,
    tex: Option<Res<crate::ocean::OceanTextures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<PlumeMaterial>>,
    old: Query<Entity, With<Plume>>,
) {
    let Some(tex) = tex else { return };
    if !water.is_changed() && state.key != 0 {
        return;
    }
    let key = lava_key(&water) | 1;
    if key == state.key {
        return;
    }
    state.key = key;
    for e in &old {
        commands.entity(e).despawn();
    }
    let ems = emitters(&terrain, &water);
    if !ems.is_empty() {
        info!("volcanoes: {} plumes", ems.len());
    }
    for (k, e) in ems.iter().enumerate() {
        let material = mats.add(PlumeMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE,
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                reflectance: 0.0,
                double_sided: true,
                cull_mode: None,
                ..default()
            },
            extension: PlumeExt {
                params: plume_params(e.kind, e.radius, &wind, sky.night),
                noise: tex.noise.clone(),
            },
        });
        commands.spawn((
            Name::new(format!("{:?} plume", e.kind)),
            Plume {
                material: material.clone(),
            },
            Mesh3d(meshes.add(plume_mesh(e, k as u64 * 7919 + 17))),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            NotShadowCaster,
            // (the puffs are expanded in the vertex shader: the mesh bounds are only their sources)
            bevy::camera::visibility::NoFrustumCulling,
        ));
    }
}

/// Wind and night for the plumes.
pub fn animate_volcanoes(
    wind: Res<crate::objects::Wind>,
    sky: Res<crate::sky::SkyState>,
    plumes: Query<&Plume>,
    mut mats: ResMut<Assets<PlumeMaterial>>,
    mut last: Local<Vec4>,
) {
    let w = Vec4::new(wind.dir.x, wind.dir.y, wind.speed, sky.night);
    if (w - *last).abs().max_element() > 0.01 {
        *last = w;
        for p in &plumes {
            if let Some(mut m) = mats.get_mut(&p.material) {
                let rise = m.extension.params.wind.w;
                let dir = wind.dir.normalize_or(Vec2::X);
                m.extension.params.wind =
                    Vec4::new(dir.x, dir.y, wind.speed.clamp(0.5, 12.0), rise);
                m.extension.params.glow.w = sky.night;
            }
        }
    }
}
