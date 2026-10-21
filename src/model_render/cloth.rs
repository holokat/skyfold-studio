//! Wind-blown cloth on library and imported glTF models: every mesh whose glTF material is named
//! `*_sway` (the style packs' banners, flags and ragged hems, `tools/stylepack` `canvas_sway`) is
//! drawn with `ClothMaterial` once its model's scene has spawned: the same textures and PBR as the
//! glTF material, with a vertex shader (`shaders/cloth.wesl`) that blows the cloth in the world's
//! wind and gusts (the foliage wind buffer). Vertex colour alpha is the sway weight: 0 where the
//! cloth is pinned, 1 at its free edge. Collision and the cloth's LODs are unaffected (it keeps one
//! level).

use std::collections::HashMap;

use bevy::gltf::GltfMaterialName;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bevy::world_serialization::WorldInstanceReady;

use crate::objects::FOLIAGE_WIND;

pub type ClothMaterial = ExtendedMaterial<StandardMaterial, ClothExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct ClothExt {
    #[storage(100, read_only)]
    pub wind: Handle<ShaderBuffer>,
}

impl Default for ClothExt {
    fn default() -> Self {
        Self { wind: FOLIAGE_WIND }
    }
}

impl MaterialExtension for ClothExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/cloth.wesl".into()
    }
    /// Shadows and depth wave with the cloth.
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/cloth_prepass.wesl".into()
    }
}

/// Is this glTF material wind-blown cloth?
pub fn is_cloth(material_name: &str) -> bool {
    material_name.ends_with("_sway")
}

/// One cloth material per glTF material (shared by every placed copy of the model).
#[derive(Resource, Default)]
pub struct ClothMaterials(HashMap<AssetId<StandardMaterial>, Handle<ClothMaterial>>);

/// A model's scene is in: its cloth meshes switch to the wind-blown material.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    cloth: Query<(&GltfMaterialName, &MeshMaterial3d<StandardMaterial>)>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut cloth_mats: ResMut<Assets<ClothMaterial>>,
    mut cache: ResMut<ClothMaterials>,
    mut commands: Commands,
    wet: Res<crate::rain::models::WetMaterials>,
) {
    let root = ev.event().entity;
    for c in children.iter_descendants(root) {
        let Ok((name, mat)) = cloth.get(c) else {
            continue;
        };
        if !is_cloth(name) {
            continue;
        }
        let Some(base) = std_mats.get(&mat.0) else {
            continue;
        };
        // (its dry look: a model loaded in the rain has its materials wetted, `rain::models`)
        let h = cache
            .0
            .entry(mat.0.id())
            .or_insert_with(|| {
                cloth_mats.add(ClothMaterial {
                    base: wet.dry_copy(mat.0.id(), base),
                    extension: ClothExt::default(),
                })
            })
            .clone();
        commands
            .entity(c)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert(MeshMaterial3d(h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sway_materials_are_cloth() {
        assert!(is_cloth("canvas_sway"));
        assert!(!is_cloth("canvas"));
        assert!(!is_cloth("sway_canvas"));
    }
}
