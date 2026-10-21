//! Caves, mines, sinkholes, passages, hidden caverns and rock arches, carved into (or added onto)
//! the terrain. Terrain › Caves: click a hillside and a cave is there.
//!
//! - `model`: the saved recipe of each cave (kind, size, levels, theme, features, seed) and its
//!   laid-out chambers and tunnels; freehand dig / fill strokes.
//! - `layout`: lays a cave out from its entrance and recipe (deterministic; Shuffle keeps locked
//!   chambers; "+" grows a branch).
//! - `sdf`: the rock as a signed distance field: terrain ∪ outcrops − carved space ∪ formations.
//! - `mesher`: surface nets in chunks, daylight flooded in through the carved space, baked into
//!   the vertices; only chunks whose inputs changed are rebuilt.
//! - `render`: the rock material (`shaders/cave.wesl`) and its textures, the chunk entities,
//!   background rebuilds, the terrain's holes (`terrain::holes`).
//! - `edit`: every change (UI and MCP), one undo step each; `mcp`: the `caves_*` tools.
//! - `ui`: Terrain › Caves (tools, the chosen cave's card, viewport preview, "+" handles).
//! - `physics`: floors, roofs, walls and rays in the caves for walk mode, the camera, picking.

#![allow(dead_code)]

pub mod dress;
pub mod edit;
pub mod layout;
mod mcp;
pub mod mesher;
pub mod model;
pub mod noise3;
pub mod physics;
pub mod render;
pub mod sdf;
pub mod simplify;
pub mod ui;

use bevy::prelude::*;

pub use model::*;

pub struct CavesPlugin;

impl Plugin for CavesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Caves>()
            .init_resource::<render::CaveWorld>()
            .init_resource::<ui::CaveTool>()
            .init_gizmo_group::<ui::CaveGizmos>()
            .add_plugins(MaterialPlugin::<render::CaveMaterial>::default())
            .add_plugins(MaterialPlugin::<render::CaveFxMaterial>::default())
            .add_systems(Startup, (render::init, ui::setup_gizmos))
            .add_systems(
                Update,
                (ui::pick, ui::viewport)
                    .chain()
                    .in_set(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                (
                    ui::apply_requests,
                    edit::apply_undo,
                    render::track,
                    render::start,
                    render::apply,
                    render::sync_xray,
                    render::sync_lights,
                    render::textures,
                    render::params,
                    ui::draw,
                )
                    .chain()
                    .after(crate::tools::ToolSystems::Commit),
            )
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                ui::cursor_label.run_if(crate::mcp::ui_visible),
            );
        mcp::register(app);
    }
}
