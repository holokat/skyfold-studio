//! Dungeons: maps of their own behind ways in from the world.
//!
//! In the world (the Dungeons tab): create a dungeon in one click and pick its way in — a liquid
//! portal standing anywhere, an object of the world (a cellar door), a plain spot, or the end of a
//! cave — then open its builder. The dungeon is a world of its own in the world's folder; the
//! editor saves the world and switches to it (`session`), and back.
//!
//! In a dungeon's map (the Dungeon tabs): one-click layouts (`presets`), a top-down plan to draw
//! rooms, halls, corridors, caverns, stairs, pits, pools, daises and bridges (`plan`, `planview`),
//! styles that build them (`styles`, `mesh`), light sources placed by hand or by Auto-light
//! (`lights`), the mood (`mood`), and dressing from the object library (`dress`). A realm (an
//! open-sky dungeon) is built with the world's own tools.
//!
//! - `model`: what is saved, in the world and in the dungeon's map.
//! - `plan`: the rooms and the space they make (`Compiled`); `space`: that space for the walker,
//!   the camera and the cursor (through `caves::physics`).
//! - `render`: meshes, materials, the cut-away, the underground look; `mcp`: the `dungeons_*`
//!   tools.

#![allow(dead_code)]

pub mod bounce;
pub mod compound;
pub mod decals;
pub mod describe;
pub mod dress;
pub mod edit;
pub mod export;
pub mod fx;
pub mod geom;
pub mod lights;
mod mcp;
pub(crate) use mcp::selection_json;
pub mod mesh;
pub mod model;
pub mod mood;
pub mod pieces;
pub mod plan;
pub mod portal;
pub mod presets;
pub mod realm_shapes;
pub mod render;
pub mod session;
pub mod sheets;
pub mod sound;
pub mod space;
pub mod styles;
pub mod survey;
pub mod ui;
pub mod viewport;

use std::sync::atomic::{AtomicU8, Ordering};

use bevy::prelude::*;

pub use model::*;

/// What is open: the world (0), an underground dungeon map (1), a realm (2). Read by the tool
/// rail (which groups and tabs show) without needing the resources.
static MODE: AtomicU8 = AtomicU8::new(0);

pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}

/// An underground dungeon's map is open.
pub fn interior() -> bool {
    mode() == 1
}

/// Any dungeon's map is open (underground or a realm).
pub fn in_dungeon() -> bool {
    mode() != 0
}

fn sync_mode(map: Res<DungeonMap>, mut ed: ResMut<crate::editor::Editor>) {
    if !map.is_changed() {
        return;
    }
    let m = match map.0.as_ref().map(|m| m.kind) {
        None => 0,
        Some(MapKind::Interior) => 1,
        Some(MapKind::Realm) => 2,
    };
    let before = MODE.swap(m, Ordering::Relaxed);
    // a tab that isn't there any more: the dungeon's
    if before != m && !crate::editor::TabGroup::visible().contains(&ed.tab.group()) {
        ed.tab = crate::editor::Tab::Dungeons;
    }
}

pub struct DungeonsPlugin;

impl Plugin for DungeonsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Dungeons>()
            .init_resource::<DungeonMap>()
            .init_resource::<ui::DungeonTool>()
            .init_resource::<render::DungeonView>()
            .init_resource::<render::DungeonWorld>()
            .init_resource::<render::InteriorLook>()
            .init_resource::<session::Session>()
            .init_resource::<portal::Portals>()
            .init_resource::<sound::DungeonSound>()
            .init_resource::<fx::Fx>()
            .init_resource::<decals::Decals>()
            .init_resource::<bounce::Bounce>()
            .init_resource::<pieces::PieceState>()
            .add_systems(bevy_egui::EguiPrimaryContextPass, pieces::hint)
            .init_gizmo_group::<viewport::DungeonGizmos>()
            .add_plugins(MaterialPlugin::<portal::PortalMaterial>::default())
            .add_plugins(MaterialPlugin::<fx::HazeMaterial>::default())
            .add_plugins(MaterialPlugin::<render::SurfaceMaterial>::default())
            .add_systems(Startup, viewport::setup_gizmos)
            .add_systems(
                Update,
                (
                    viewport::world_pick,
                    viewport::builder_input,
                    viewport::fixtures_follow,
                    portal::way_out_follows,
                )
                    .chain()
                    .in_set(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                (
                    viewport::walk_look,
                    viewport::walk_prompt,
                    viewport::pending_door,
                )
                    .chain()
                    .after(crate::camera::CameraSystems),
            )
            // Describe it: dropped pictures, the way in's spot, the test hook, and the run
            .add_systems(
                Update,
                (
                    describe::dropped_images,
                    describe::pick_spot,
                    describe::test_hook,
                )
                    .in_set(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                describe::drive.after(crate::tools::ToolSystems::Commit),
            )
            .add_systems(
                Update,
                (
                    viewport::apply_requests,
                    edit::apply_undo,
                    viewport::commit_live,
                    sync_mode,
                    session::follow,
                    session::sync_entrances,
                    session::link_caves,
                    (portal::crossing, pieces::interact),
                    render::track,
                    session::setup_fresh,
                    render::start,
                    render::apply,
                    (render::mips, render::weathering, bounce::update),
                    render::view,
                    render::interior_env,
                    (render::sync_lights, render::volumetric).chain(),
                    portal::sync,
                    sound::ambience,
                    (fx::flames, fx::motes, fx::haze, decals::sync, pieces::sync),
                    viewport::draw,
                )
                    .chain()
                    .after(crate::tools::ToolSystems::Commit),
            );
        mcp::register(app);
        describe::register(app);
        realm_shapes::register(app);
        compound::register(app);
        sheets::register(app);
    }
}
