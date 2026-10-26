//! Rock that keeps its grain at any size (`shaders/rock_sized.wesl`).
//!
//! A placed object's scale stretches its texture with it: fine for a boulder at 0.6–1.6×, not for
//! a floating island scaled from a 2 m pebble to 60 m, or a 15 m rock spire grown to a 300 m
//! tower. Two fixes, one material type:
//!
//! * Tiled shapes (floating rocks, monoliths: procedural, wearing the tiling rock scan): always
//!   drawn with their uv multiplied by the object's size.
//! * Scanned formations (rock spires, hoodoos, sea stacks) placed at `GIANT`× and more: their own
//!   stretched atlas, with the tiling rock scan laid over it in world space as fine detail. Below
//!   that size they keep their usual material, so worlds built at today's sizes look the same.

use std::collections::HashMap;

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

use crate::objects::{FoliageMaterial, Library, builtin_id};
use crate::procgen::meshes::AssetKind;

pub type RockMaterial = ExtendedMaterial<StandardMaterial, RockExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct RockExt {
    /// x: 1 = uv scale with the object's size, y: detail strength (0 none), z: detail tile (m)
    #[uniform(100)]
    pub params: Vec4,
    #[texture(101)]
    #[sampler(102)]
    pub detail_albedo: Handle<Image>,
    #[texture(103)]
    pub detail_normal: Handle<Image>,
}

impl MaterialExtension for RockExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/rock_sized.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/rock_sized.wesl".into()
    }
}

/// From this size up a scanned formation wears the world-space detail.
pub const GIANT: f32 = 2.5;

/// The kinds whose texture is a tiling scan on a procedural shape, sized with the object.
pub fn tiled(kind: AssetKind) -> bool {
    matches!(kind, AssetKind::FloatingRock | AssetKind::Monolith)
}

/// The kinds that grow to giant sizes in alien worlds and get the detail there.
fn grows_giant(kind: AssetKind) -> bool {
    matches!(
        kind,
        AssetKind::RockSpire | AssetKind::Hoodoo | AssetKind::SeaStack
    )
}

/// The rock materials by kind: tiled shapes' always, scanned formations' for giant sizes.
#[derive(Resource, Default)]
pub struct RockMaterials {
    by_kind: HashMap<AssetKind, Handle<RockMaterial>>,
}

impl RockMaterials {
    /// The material a placed object of `kind` at size `scale` wears instead of its library one.
    pub fn for_object(&self, kind: Option<AssetKind>, scale: f32) -> Option<Handle<RockMaterial>> {
        let k = kind?;
        if tiled(k) || (grows_giant(k) && scale >= GIANT) {
            self.by_kind.get(&k).cloned()
        } else {
            None
        }
    }
}

/// Builds the rock materials from the library's (once it's built): the tiled shapes and the cliff
/// rock share the tiling rock scan (`photoscan::DRESSED`), which is also the formations' detail.
pub fn build_rock_materials(
    mut commands: Commands,
    lib: Option<Res<Library>>,
    foliage: Res<Assets<FoliageMaterial>>,
    mut rocks: ResMut<Assets<RockMaterial>>,
) {
    let mut out = RockMaterials::default();
    let base_of = |k: AssetKind| -> Option<StandardMaterial> {
        let e = lib.as_ref()?.get(&builtin_id(k))?;
        Some(foliage.get(&e.tree.as_ref()?.bark)?.base.clone())
    };
    let Some(tile) = base_of(AssetKind::CliffRock) else {
        warn!(
            "rock materials: no tiling rock scan (CliffRock), floating rocks keep stretched tiles"
        );
        commands.insert_resource(out);
        return;
    };
    let (Some(albedo), Some(normal)) = (
        tile.base_color_texture.clone(),
        tile.normal_map_texture.clone(),
    ) else {
        commands.insert_resource(out);
        return;
    };
    let ext = |params: Vec4| RockExt {
        params,
        detail_albedo: albedo.clone(),
        detail_normal: normal.clone(),
    };
    for k in [AssetKind::FloatingRock, AssetKind::Monolith] {
        let Some(mut base) = base_of(k) else { continue };
        if k == AssetKind::Monolith {
            // dark, close-grained dressed stone
            base.base_color = Color::srgb(0.66, 0.65, 0.68);
            base.perceptual_roughness = 0.68;
        } else {
            base.base_color = Color::srgb(0.9, 0.86, 0.82);
        }
        out.by_kind.insert(
            k,
            rocks.add(RockMaterial {
                base,
                extension: ext(Vec4::new(1.0, 0.0, 1.0, 0.0)),
            }),
        );
    }
    for k in [AssetKind::RockSpire, AssetKind::Hoodoo, AssetKind::SeaStack] {
        let Some(base) = base_of(k) else { continue };
        out.by_kind.insert(
            k,
            rocks.add(RockMaterial {
                base,
                extension: ext(Vec4::new(0.0, 0.7, 5.5, 0.0)),
            }),
        );
    }
    info!("rock materials: {} kinds sized", out.by_kind.len());
    commands.insert_resource(out);
}
