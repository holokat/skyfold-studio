//! Placing and shaping objects once they're in the world:
//!
//! - `handles`: freehand handles on the selected object in the viewport — move, turn, resize,
//!   lift — in both Objects tools (Place and Select & Move).
//! - `fit`: buildings and other structures fit the terrain wherever they go: the ground under
//!   them is levelled to their floor with natural banks around (cut uphill, fill downhill), a
//!   packed-earth apron is painted, grass and procedural nature keep off their footprint. It
//!   happens for every edit from every tool and for older worlds when they load; moving or
//!   deleting a building gives its old spot its ground back. One undo step with the edit.
//! - `ground`: the remembered ground under the pads (saved with the world).
//! - `footprint`: model bounds and the rectangle an object covers.
//!
//! MCP: `objects_place` / `objects_transform` fit structures as they go (`fit_ground: false`
//! opts out) and `objects_fit_ground` levels or releases the ground under chosen objects
//! (`mcp/tools/objects.rs`).

pub mod fit;
pub mod footprint;
pub mod ground;
pub mod group;
pub mod handles;
pub mod lock;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

pub use fit::{FitWorld, GrassMask};
pub use footprint::{Footprint, ModelBounds};
pub use ground::Foundations;
pub use handles::Handles;

/// The foundations and model bounds, for systems that edit objects (`io::WorldData`, the UI).
#[derive(SystemParam)]
pub struct FitParams<'w> {
    pub found: ResMut<'w, Foundations>,
    pub bounds: Res<'w, ModelBounds>,
}

pub struct PlacementPlugin;

impl Plugin for PlacementPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Foundations>()
            .init_resource::<ModelBounds>()
            .init_resource::<GrassMask>()
            .init_resource::<Handles>()
            .init_resource::<lock::ObjectMenu>()
            .add_systems(
                Update,
                lock::menu_input.after(crate::tools::ToolSystems::Input),
            )
            .add_systems(Startup, footprint::collect)
            .add_systems(
                Update,
                handles::input
                    .after(crate::tools::ToolSystems::Input)
                    .before(crate::tools::ToolSystems::Apply),
            )
            .add_systems(
                Update,
                (fit::auto_fit, fit::update_grass_mask)
                    .chain()
                    .after(crate::tools::ToolSystems::Commit),
            )
            .add_systems(
                Update,
                handles::test_hook
                    .after(crate::debug::demo_content)
                    .before(fit::auto_fit),
            )
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                (handles::draw, lock::draw),
            );
    }
}
