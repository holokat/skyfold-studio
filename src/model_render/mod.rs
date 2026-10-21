//! Rendering help for library and imported glTF models: mip chains and GPU block compression for
//! their textures (`mips`, `compress`),
//! automatic distance LODs (`lod`, built with `simplify`), wind-blown cloth (`cloth`), moving
//! parts (`spin`: windmill sails, water wheels; `swing`: lanterns rocking on their hooks, boats
//! bobbing), hinged doors that open as you walk up (`door`) and chest lids the explorer opens with
//! the interact key (`lid`).

mod cloth;
pub(crate) mod compress;
pub mod door;
pub mod lid;
mod lod;
pub(crate) mod mips;
pub(crate) mod simplify;
pub mod spin;
pub mod swing;

use bevy::prelude::*;

pub struct ModelRenderPlugin;

impl Plugin for ModelRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<cloth::ClothMaterial>::default())
            .init_resource::<cloth::ClothMaterials>()
            .add_observer(cloth::on_scene_ready)
            .init_resource::<mips::MipJobs>()
            .init_resource::<lod::ModelLods>()
            .add_observer(lod::on_scene_ready)
            .add_observer(spin::on_scene_ready)
            .add_observer(door::on_scene_ready)
            .add_observer(swing::on_scene_ready)
            .add_systems(
                Update,
                (
                    mips::queue,
                    mips::finish,
                    lod::update,
                    lod::log,
                    spin::turn,
                    door::swing,
                    swing::rock,
                ),
            );
        lid::register(app);
    }
}
