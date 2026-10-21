//! Drifting motes: soft points of light floating around the explorer in eerie and magical places
//! (a Haunted Woods biome, a zone whose mood is Eerie, the glow of nearby crystals), brightest at
//! night, and fireflies over swamps on warm nights. A field of glowing specks wraps around the
//! camera (`motes.wesl` keeps them in a box that travels with it), so they cost one small mesh.
//! Only while walking: the editor overview stays clean. No fog is added: the zone or biome's own
//! mood decides that.

use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::biomes::BiomePreset;
use crate::noise::Rng;
use crate::objects::ObjectStore;
use crate::world::{Mood, Zones};

/// Motes in the field and the size of the box they wrap around the camera in (m).
const COUNT: usize = 700;
const BOX: f32 = 46.0;

pub type MoteMaterial = ExtendedMaterial<StandardMaterial, MoteExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct MoteParams {
    /// rgb: colour (display units), a: amount (0 hides)
    pub color: Vec4,
    /// x: box size (m), y: mote size (m), z: rise speed (m/s), w: twinkle rate
    pub shape: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct MoteExt {
    #[uniform(100)]
    pub params: MoteParams,
}

impl MaterialExtension for MoteExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/motes.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/motes.wesl".into()
    }
}

#[derive(Component)]
pub struct MoteField(Handle<MoteMaterial>);

/// Positions in the unit cube (wrapped around the camera in the shader); vertex colour: (seed,
/// phase, size, 1); uv: corner.
pub fn field_mesh() -> Mesh {
    let mut rng = Rng::new(0x307e5);
    let (mut pos, mut uv, mut col, mut nor, mut idx) = (vec![], vec![], vec![], vec![], vec![]);
    for _ in 0..COUNT {
        let p = [rng.f32(), rng.f32(), rng.f32()];
        let c = [rng.f32(), rng.f32(), rng.range(0.6, 1.4), 1.0];
        let base = pos.len() as u32;
        for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            pos.push(p);
            uv.push(corner);
            col.push(c);
            nor.push([0.0, 1.0, 0.0]);
        }
        idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
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

pub fn spawn_motes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<MoteMaterial>>,
) {
    let material = mats.add(MoteMaterial {
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
                shape: Vec4::new(BOX, 0.07, 0.15, 1.0),
            },
        },
    });
    commands.spawn((
        Name::new("Motes"),
        MoteField(material.clone()),
        Mesh3d(meshes.add(field_mesh())),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        NotShadowCaster,
        NotShadowReceiver,
        // (wrapped around the camera in the vertex shader; the entity rides along with the camera
        // so GPU culling, which goes by the mesh's own bounds, always keeps it)
        bevy::camera::visibility::NoFrustumCulling,
    ));
}

/// What the place around the explorer asks for: (colour, amount, size, rise).
fn wanted(
    terrain: &crate::terrain::Terrain,
    zones: &Zones,
    store: &ObjectStore,
    sky: &crate::sky::SkyState,
    p: Vec3,
) -> (Vec3, f32, f32, f32) {
    let night = sky.night;
    let eerie_zone = zones
        .by_id(sky.viewer_zone)
        .is_some_and(|z| z.mood_enabled && z.mood == Mood::Eerie);
    let biome = BiomePreset::from_id(terrain.biome_at(p.x, p.z));
    // crystals within ~40 m
    let crystals = store
        .query(p.xz(), 40.0)
        .into_iter()
        .filter(|id| store.items.get(id).is_some_and(|o| o.asset == "Crystals"))
        .count();
    if crystals > 0 {
        return (
            Vec3::new(0.55, 0.45, 1.0),
            (0.45 + 0.1 * crystals as f32).min(1.0) * (0.35 + 0.65 * night),
            0.1,
            0.12,
        );
    }
    if eerie_zone || biome == Some(BiomePreset::HauntedWoods) {
        return (Vec3::new(0.45, 1.0, 0.7), 0.3 + 0.7 * night, 0.11, 0.08);
    }
    if biome == Some(BiomePreset::Swamp) || biome == Some(BiomePreset::Jungle) {
        // fireflies on warm nights
        return (
            Vec3::new(0.85, 1.0, 0.35),
            crate::noise::smoothstep(0.4, 0.8, night) * 0.8,
            0.08,
            0.05,
        );
    }
    (Vec3::ZERO, 0.0, 0.07, 0.1)
}

#[allow(clippy::too_many_arguments)]
pub fn update_motes(
    walk: Res<crate::walk::WalkState>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut field_tf: Query<&mut Transform, With<MoteField>>,
    terrain: Res<crate::terrain::Terrain>,
    zones: Res<Zones>,
    store: Res<ObjectStore>,
    sky: Res<crate::sky::SkyState>,
    time: Res<Time>,
    field: Query<&MoteField>,
    mut mats: ResMut<Assets<MoteMaterial>>,
    mut cur: Local<(Vec3, f32)>,
    mut timer: Local<f32>,
    mut target: Local<(Vec3, f32, f32, f32)>,
) {
    let Ok(f) = field.single() else { return };
    if let (Ok(c), Ok(mut tf)) = (cam.single(), field_tf.single_mut()) {
        tf.translation = c.translation();
    }
    *timer -= time.delta_secs();
    if *timer <= 0.0 {
        *timer = 0.5;
        *target = if walk.active {
            wanted(&terrain, &zones, &store, &sky, walk.position)
        } else {
            (cur.0, 0.0, 0.07, 0.1)
        };
    }
    // ease in and out over a couple of seconds
    let k = (time.delta_secs() * 0.6).min(1.0);
    let amount = cur.1 + (target.1 - cur.1) * k;
    let color = if target.1 > 0.0 {
        cur.0.lerp(target.0, (k * 2.0).min(1.0))
    } else {
        cur.0
    };
    if (amount - cur.1).abs() < 1e-4 && amount < 1e-3 && cur.1 < 1e-3 {
        return;
    }
    *cur = (color, amount);
    if std::env::var("WB_VSEQ").is_ok() && (time.elapsed_secs() % 5.0) < time.delta_secs() {
        info!("motes: amount {amount:.2}, colour {color:?}");
    }
    if let Some(mut m) = mats.get_mut(&f.0) {
        m.extension.params.color = (color * 3.5).extend(amount);
        m.extension.params.shape = Vec4::new(BOX, target.2, target.3, 1.0);
    }
}
