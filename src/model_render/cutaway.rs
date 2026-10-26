//! The locked game cameras' cutaway (isometric and top-down, `crate::characters::play_camera`):
//! while one looks on, the placed models round the player draw with `CutawayMaterial`, the same
//! textures and PBR as their own material with two cuts (`shaders/cutaway_cut.wesl`). Under a roof
//! the building is cut away above the player's head (roof and upper floors), and whatever stands
//! in front of the player opens a see-through circle round it. Shadows keep every piece. The
//! free cameras (first person, over the shoulder, classic MMO) never cut anything; leaving the
//! locked view gives every model its own material back.
//!
//! The cut's parameters live in one shared storage buffer (`CUTAWAY`), so following the player
//! never touches the materials themselves.

use std::collections::HashMap;

use bevy::asset::{RenderAssetUsages, uuid_handle};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use crate::objects::PlacedObject;

pub type CutawayMaterial = ExtendedMaterial<StandardMaterial, CutawayExt>;

/// The cut's parameters (`CutawayParams` in `shaders/cutaway_cut.wesl`).
pub const CUTAWAY: Handle<ShaderBuffer> = uuid_handle!("fc21d24a-7296-445e-b01e-3a0b354ae77c");

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CutawayExt {
    #[storage(100, read_only)]
    pub params: Handle<ShaderBuffer>,
}

impl MaterialExtension for CutawayExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/cutaway.wesl".into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        "shaders/cutaway_prepass.wesl".into()
    }
}

/// What the game camera wants cut away this frame (`play_camera` sets it).
#[derive(Resource, Default, Clone, PartialEq, Debug)]
pub struct Cutaway {
    pub on: bool,
    /// the player's chest
    pub player: Vec3,
    pub feet: f32,
    pub camera: Vec3,
    /// the see-through circle's radius, a part of the view's height
    pub circle: f32,
    /// the building (or bridge) over the player: its plan bounds (min x, min z, max x, max z)
    /// and the height it's cut away above
    pub roof: Option<(Vec4, f32)>,
}

impl Cutaway {
    /// Models whose origin is within this of the player (m) are drawn ready to be cut.
    const REACH: f32 = 32.0;

    fn pack(&self) -> [f32; 16] {
        let (b, cut) = self.roof.unwrap_or((Vec4::new(1.0, 1.0, -1.0, -1.0), 1.0e9));
        [
            self.player.x,
            self.player.y,
            self.player.z,
            if self.on { 1.0 } else { 0.0 },
            self.camera.x,
            self.camera.y,
            self.camera.z,
            self.circle,
            b.x,
            b.y,
            b.z,
            b.w,
            cut,
            0.35,
            self.feet,
            0.0,
        ]
    }
}

/// A mesh drawn with the cutaway material: its own material, to give back.
#[derive(Component)]
pub struct CutawayOf(Handle<StandardMaterial>);

#[derive(Resource, Default)]
pub struct CutawayMaterials {
    by_source: HashMap<AssetId<StandardMaterial>, Handle<CutawayMaterial>>,
    /// frames until the models round the player are looked over again
    rescan: u32,
    last: Option<[f32; 16]>,
}

pub fn register(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CutawayMaterial>::default())
        .init_resource::<Cutaway>()
        .init_resource::<CutawayMaterials>()
        .add_systems(PreStartup, init_buffer);
}

fn init_buffer(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let _ = buffers.insert(
        &CUTAWAY,
        ShaderBuffer::new(
            Cutaway::default().pack().to_vec(),
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        ),
    );
}

/// Follows the player with the cut, and gives the models round it the cutaway material (and,
/// out of reach or out of the locked view, their own back).
#[allow(clippy::too_many_arguments)]
pub fn apply(
    mut commands: Commands,
    cut: Res<Cutaway>,
    mut cache: ResMut<CutawayMaterials>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    roots: Query<(Entity, &GlobalTransform), With<PlacedObject>>,
    children: Query<&Children>,
    plain: Query<&MeshMaterial3d<StandardMaterial>, Without<CutawayOf>>,
    swapped: Query<(Entity, &CutawayOf)>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut cut_mats: ResMut<Assets<CutawayMaterial>>,
) {
    let data = cut.pack();
    if cache.last != Some(data) {
        cache.last = Some(data);
        let _ = buffers.insert(
            &CUTAWAY,
            ShaderBuffer::new(
                data.to_vec(),
                RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
            ),
        );
    }
    if !cut.on {
        for (e, of) in &swapped {
            commands
                .entity(e)
                .remove::<(MeshMaterial3d<CutawayMaterial>, CutawayOf)>()
                .insert(MeshMaterial3d(of.0.clone()));
        }
        cache.rescan = 0;
        return;
    }
    // (looked over a few times a second: models stream in as the player walks)
    if cache.rescan > 0 {
        cache.rescan -= 1;
        return;
    }
    cache.rescan = 15;
    let at = cut.player.xz();
    let mut keep = std::collections::HashSet::new();
    for (root, tf) in &roots {
        if tf.translation().xz().distance(at) > Cutaway::REACH {
            continue;
        }
        for e in children.iter_descendants(root) {
            keep.insert(e);
            let Ok(m) = plain.get(e) else { continue };
            let Some(h) = cache.by_source.get(&m.0.id()).cloned().or_else(|| {
                let base = std_mats.get(&m.0)?.clone();
                let h = cut_mats.add(CutawayMaterial {
                    base,
                    extension: CutawayExt { params: CUTAWAY },
                });
                cache.by_source.insert(m.0.id(), h.clone());
                Some(h)
            }) else {
                continue;
            };
            commands
                .entity(e)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert((MeshMaterial3d(h), CutawayOf(m.0.clone())));
        }
    }
    for (e, of) in &swapped {
        if !keep.contains(&e) {
            commands
                .entity(e)
                .remove::<(MeshMaterial3d<CutawayMaterial>, CutawayOf)>()
                .insert(MeshMaterial3d(of.0.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_like_the_shader_reads_it() {
        let c = Cutaway {
            on: true,
            player: Vec3::new(1.0, 2.0, 3.0),
            feet: 1.0,
            camera: Vec3::new(4.0, 5.0, 6.0),
            circle: 0.13,
            roof: Some((Vec4::new(-1.0, -2.0, 7.0, 8.0), 3.4)),
        };
        let p = c.pack();
        assert_eq!(&p[0..4], &[1.0, 2.0, 3.0, 1.0]);
        assert_eq!(&p[4..8], &[4.0, 5.0, 6.0, 0.13]);
        assert_eq!(&p[8..12], &[-1.0, -2.0, 7.0, 8.0]);
        assert_eq!(&p[12..15], &[3.4, 0.35, 1.0]);
        // off and roofless: no box, no cut
        let p = Cutaway::default().pack();
        assert_eq!(p[3], 0.0);
        assert!(p[8] > p[10] && p[12] > 1.0e8);
    }
}
