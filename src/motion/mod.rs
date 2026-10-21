//! Exploring the world like a game: every way of moving (walk, run, sprint, crouch, jump, swim,
//! dive, ride, fly — `movement`), solid objects (`collide`), the world under water
//! (`underwater`), Playtest sessions for designers (`playtest`: spawn at the player start, compass
//! and minimap, discoveries, recorded routes) and cinematic camera paths with video capture
//! (`flythrough`). Walk mode itself lives in `walk.rs`; this module owns what it uses.
//!
//! Saved with the world (`MotionData`): camera paths and the last ten playtest sessions. Edits to
//! them are undoable (`History::push_ext`). Agents drive it all through MCP (`mcp`).

mod buoyancy;
pub mod collide;
pub mod flythrough;
mod hooks;
mod hud;
pub mod indoors;
pub(crate) mod mcp;
pub mod movement;
pub mod pick;
pub mod playtest;
mod sfx;
pub mod underwater;
pub mod waves;

use std::sync::Arc;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::history::History;

/// Camera paths and playtest sessions, saved with the world.
#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MotionData {
    #[serde(default)]
    pub paths: Vec<flythrough::CameraPath>,
    /// The most recent playtest sessions (oldest first, at most `playtest::KEEP`).
    #[serde(default)]
    pub sessions: Vec<playtest::Session>,
    #[serde(default)]
    pub next_id: u32,
}

impl MotionData {
    pub fn alloc_id(&mut self) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

const UNDO_KEY: &str = "motion";

/// Make an undoable change to the motion data (`f` returns false when nothing changed).
pub fn edit(
    data: &mut MotionData,
    history: &mut History,
    label: &str,
    f: impl FnOnce(&mut MotionData) -> bool,
) {
    let before = data.clone();
    if f(data) && *data != before {
        history.push_ext(label, UNDO_KEY, Arc::new(before), Arc::new(data.clone()));
    }
}

/// Record a change already made to the motion data (a drag that just ended) as one undo step.
pub fn commit(data: &MotionData, history: &mut History, label: &str, before: MotionData) {
    if *data != before {
        history.push_ext(label, UNDO_KEY, Arc::new(before), Arc::new(data.clone()));
    }
}

/// Requests from the editor UI (the top bar's Walk menu, the motion window).
#[derive(Message, Clone, Debug)]
pub enum MotionAction {
    /// Start a playtest session at the player start.
    Playtest,
    /// Open the motion window (camera paths, playtest sessions).
    OpenPanel(PanelTab),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelTab {
    #[default]
    Paths,
    Playtests,
}

pub struct MotionPlugin;

impl Plugin for MotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<collide::Colliders>()
            .init_resource::<indoors::Indoors>()
            .add_systems(
                Update,
                indoors::update_indoors.before(crate::sky::SkySystems),
            )
            .add_systems(
                PostUpdate,
                buoyancy::ride_waves.before(bevy::transform::TransformSystems::Propagate),
            )
            .init_resource::<collide::ColliderMeshes>()
            .init_resource::<MotionData>()
            .add_message::<MotionAction>()
            .add_systems(Startup, collide::collect_library_meshes)
            .add_systems(Update, (collide::load_gltf_meshes, apply_undo))
            .add_plugins((
                underwater::UnderwaterPlugin,
                playtest::PlaytestPlugin,
                flythrough::FlythroughPlugin,
                hud::HudPlugin,
                sfx::SfxPlugin,
                hooks::HooksPlugin,
            ));
        mcp::register(app);
    }
}

/// Undo / redo of motion edits: restore the snapshot the history hands back.
fn apply_undo(mut history: ResMut<History>, mut data: ResMut<MotionData>) {
    if history.ext_apply.is_empty() {
        return;
    }
    let mut keep = vec![];
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        if key != UNDO_KEY {
            keep.push((key, snap));
            continue;
        }
        if let Some(d) = snap.downcast_ref::<MotionData>() {
            *data = d.clone();
        }
    }
    history.ext_apply = keep;
}
