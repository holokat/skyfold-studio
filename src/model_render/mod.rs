//! Rendering help for library and imported glTF models: mip chains and GPU block compression for
//! their textures (`mips`, `compress`),
//! automatic distance LODs (`lod`, built with `simplify`), wind-blown cloth (`cloth`), moving
//! parts (`spin`: windmill sails, water wheels; `swing`: lanterns rocking on their hooks, boats
//! bobbing), hinged doors that open as you walk up (`door`) and chest lids the explorer opens with
//! the interact key (`lid`) and lift cars that carry the explorer between floors (`lift`); the
//! locked game cameras see into buildings through `cutaway`; `clips` plays a model's own named glTF
//! clips (weapons' fire / draw / ignite, previews over MCP `model_clip`). A
//! model's `collision` node (the lean walk collider it ships, read by `motion::collide`) is never
//! drawn.

pub mod clips;
mod cloth;
pub(crate) mod compress;
pub mod cutaway;
pub mod door;
pub mod lid;
pub mod lift;
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
            .init_resource::<lift::LiftFloors>()
            .init_resource::<lift::LiftButtonFocus>()
            .add_systems(Startup, lift::load_sounds)
            .add_observer(lift::on_scene_ready)
            .add_observer(swing::on_scene_ready)
            .add_observer(hide_collision)
            .add_systems(
                Update,
                (
                    mips::queue,
                    mips::finish,
                    lod::update,
                    lod::log,
                    spin::turn,
                    door::swing,
                    lift::run.before(crate::walk::WalkSystems),
                    lift::sounds.after(lift::run),
                    lift::displays.after(lift::run),
                    (lift::aim_buttons, lift::press_buttons)
                        .chain()
                        .after(lid::press),
                    swing::rock,
                ),
            );
        app.add_systems(bevy_egui::EguiPrimaryContextPass, lift::button_hint);
        lid::register(app);
        clips::register(app);
        cutaway::register(app);
    }
}

/// A model's scene is in: hide its `collision` node (geometry for the walk collider only).
fn hide_collision(
    ev: On<bevy::world_serialization::WorldInstanceReady>,
    children: Query<&Children>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    for c in children.iter_descendants(ev.event().entity) {
        if names.get(c).is_ok_and(|n| n.as_str() == "collision") {
            commands.entity(c).insert(Visibility::Hidden);
        }
    }
}
