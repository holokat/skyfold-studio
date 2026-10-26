//! The world's side of characters: which one is the player (`player`, saved in `world.json`
//! `characters`), and adding one as an NPC: click the ground and a Gameplay NPC with that
//! character stands there (edited in Gameplay › NPCs, `crate::characters::npc`). Worlds where
//! characters were placed before NPCs existed get those placements as NPCs.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::view::CharacterView;
use super::{Characters, labs_enabled};
use crate::camera::{MainCamera, Pointer};
use crate::gameplay::{Gameplay, GameplaySel, NpcRole};

/// A character placed before NPCs existed (only read, then made an NPC).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlacedCharacter {
    pub id: u64,
    pub character: String,
    pub pos: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub anim: Option<String>,
    #[serde(default)]
    pub speed: f32,
    #[serde(default)]
    pub scale: f32,
}

#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorldCharacters {
    /// the character Walk mode plays as
    #[serde(default)]
    pub player: Option<String>,
    /// placements from before NPCs (made NPCs on load)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub list: Vec<PlacedCharacter>,
    /// the game camera while walking (`super::play_camera`)
    #[serde(default)]
    pub camera: super::play_camera::GameCamera,
}

impl WorldCharacters {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.player.is_none() && self.camera == Default::default()
    }
}

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (migrate, place).chain());
    }
}

fn ground<'a>(
    terrain: &'a crate::terrain::Terrain,
    zones: &'a crate::world::Zones,
    water: &'a crate::water::WaterBodies,
) -> crate::gameplay::edit::Ground<'a> {
    crate::gameplay::edit::Ground {
        terrain,
        zones,
        water,
        nature: None,
    }
}

/// Old placements become NPCs.
fn migrate(
    mut world: ResMut<WorldCharacters>,
    mut gp: ResMut<Gameplay>,
    chars: Res<Characters>,
    terrain: Res<crate::terrain::Terrain>,
    zones: Res<crate::world::Zones>,
    water: Res<crate::water::WaterBodies>,
) {
    if world.list.is_empty() {
        return;
    }
    let g = ground(&terrain, &zones, &water);
    for c in std::mem::take(&mut world.list) {
        let name = chars.get(&c.character).map(|e| e.meta.name.clone());
        // (character yaw: 0 faces +z; gameplay facing: 0 = +x, counter-clockwise)
        let yaw = c.yaw - std::f32::consts::FRAC_PI_2;
        let id = crate::gameplay::edit::add_npc(
            &mut gp,
            &g,
            NpcRole::Ambient,
            Vec2::new(c.pos[0], c.pos[2]),
            yaw,
            name,
            None,
        );
        if let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) {
            n.life.character = Some(c.character.clone());
            if let Some(a) = c.anim {
                n.life.moves.insert(super::moves::Move::Idle.id().into(), a);
            }
        }
    }
}

/// While adding an NPC: a left click on the ground stands it there, facing the camera, and opens
/// it in Gameplay › NPCs; Esc or a right click stops.
#[allow(clippy::too_many_arguments)]
fn place(
    mut view: ResMut<CharacterView>,
    mut gp: ResMut<Gameplay>,
    mut history: ResMut<crate::history::History>,
    store: Res<crate::objects::ObjectStore>,
    chars: Res<Characters>,
    terrain: Res<crate::terrain::Terrain>,
    zones: Res<crate::world::Zones>,
    water: Res<crate::water::WaterBodies>,
    pointer: Res<Pointer>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
) {
    let Some(placing) = view.placing.clone() else {
        return;
    };
    if !labs_enabled()
        || keys.just_pressed(KeyCode::Escape)
        || mouse.just_pressed(MouseButton::Right)
    {
        view.placing = None;
        return;
    }
    if pointer.over_ui || !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(hit) = pointer.hit else {
        return;
    };
    // facing the camera (gameplay facing: 0 = +x, counter-clockwise from above)
    let yaw = cam
        .single()
        .map(|c| {
            let d = c.translation() - hit;
            (-d.z).atan2(d.x)
        })
        .unwrap_or(0.0);
    let name = chars.get(&placing.character).map(|e| e.meta.name.clone());
    history.begin("Add NPC");
    history.touch_gameplay(&gp);
    let g = ground(&terrain, &zones, &water);
    let id = crate::gameplay::edit::add_npc(
        &mut gp,
        &g,
        NpcRole::Ambient,
        hit.xz(),
        yaw,
        name,
        None,
    );
    if let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) {
        n.life.character = Some(placing.character.clone());
    }
    history.commit(&store);
    gp.selected = Some(GameplaySel::Npc(id));
    view.placing = None;
    view.section = super::view::Section::Npcs;
    view.sel = Some(super::view::Sel::Npc(id));
    view.npc_tab = super::view::NpcTab::Behavior;
}
