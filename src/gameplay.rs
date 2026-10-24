//! The gameplay layer: what players interact with rather than what they look at — the player
//! start, points of interest, NPCs, creature spawn regions (with a shared creature library for
//! their encounter tables) and resource nodes. Saved with the world, shown as editor markers,
//! read by the pacing analysis, the world map and the playtest, and exported as server data.
//!
//! Positions are world metres (`[x, y, z]`, y = ground height when placed); regions are
//! polygons in world xz. Ids are unique across all gameplay items (`next_id`).
//!
//! The Gameplay tab (`ui/panels/gameplay_tab.rs`) edits it with the viewport tools in
//! `viewport`, through the shared operations in `edit` (also used by the MCP tools in `mcp`);
//! `markers` draws the editor pins and spawn-region overlay, `scatter` places resource nodes
//! automatically and `export` writes the server data (`gameplay.json`). Mining nodes carry one of
//! the 25 ores of `ores` (which also holds the ore deposit areas); `deposits` shows each as a
//! photoscanned rock with the ore in it.

pub mod deposits;
pub mod edit;
pub mod export;
pub mod markers;
pub(crate) mod mcp;
pub mod ores;
pub mod scatter;
mod viewport;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use export::write_server_data;
pub use ores::{Ore, OreArea};
pub use viewport::{finish_area, finish_region};

#[derive(Resource, Clone, Debug, Default, Serialize, Deserialize)]
pub struct Gameplay {
    #[serde(default)]
    pub player_start: Option<PlayerStart>,
    /// The world's intended progression ceiling. Older worlds leave this unset until a designer
    /// chooses it explicitly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_cap: Option<u32>,
    #[serde(default)]
    pub pois: Vec<Poi>,
    #[serde(default)]
    pub npcs: Vec<Npc>,
    #[serde(default)]
    pub spawns: Vec<SpawnRegion>,
    #[serde(default)]
    pub nodes: Vec<ResourceNode>,
    /// Creature library the spawn regions draw from.
    #[serde(default)]
    pub creatures: Vec<CreatureType>,
    /// Ore deposit areas (their deposits are in `nodes`, with `area` set).
    #[serde(default)]
    pub ore_areas: Vec<OreArea>,
    /// How ore fills the world (Scatter ore, and new worlds).
    #[serde(default)]
    pub ore_settings: ores::OreSettings,
    /// A world was just generated: fill it with ore (`ore_settings.on_generate`) next frame.
    #[serde(skip)]
    pub ore_fill_pending: bool,
    #[serde(default)]
    pub next_id: u32,
    /// Markers / overlays need rebuilding.
    #[serde(skip)]
    pub dirty: bool,
    #[serde(skip)]
    pub selected: Option<GameplaySel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameplaySel {
    PlayerStart,
    Poi(u32),
    Npc(u32),
    Spawn(u32),
    Node(u32),
    OreArea(u32),
}

impl Gameplay {
    /// What an id refers to (ids are unique across all items, creatures included).
    pub fn find(&self, id: u32) -> Option<GameplaySel> {
        if self.pois.iter().any(|p| p.id == id) {
            Some(GameplaySel::Poi(id))
        } else if self.npcs.iter().any(|p| p.id == id) {
            Some(GameplaySel::Npc(id))
        } else if self.spawns.iter().any(|p| p.id == id) {
            Some(GameplaySel::Spawn(id))
        } else if self.nodes.iter().any(|p| p.id == id) {
            Some(GameplaySel::Node(id))
        } else if self.ore_areas.iter().any(|a| a.id == id) {
            Some(GameplaySel::OreArea(id))
        } else {
            None
        }
    }

    /// Whether the selection still exists.
    pub fn exists(&self, sel: GameplaySel) -> bool {
        match sel {
            GameplaySel::PlayerStart => self.player_start.is_some(),
            GameplaySel::Poi(id)
            | GameplaySel::Npc(id)
            | GameplaySel::Spawn(id)
            | GameplaySel::Node(id)
            | GameplaySel::OreArea(id) => self.find(id) == Some(sel),
        }
    }

    /// Ground anchor of an item (a spawn region's centroid).
    pub fn anchor(&self, sel: GameplaySel) -> Option<Vec3> {
        let v = |p: [f32; 3]| Vec3::from_array(p);
        match sel {
            GameplaySel::PlayerStart => self.player_start.map(|p| v(p.pos)),
            GameplaySel::Poi(id) => self.pois.iter().find(|p| p.id == id).map(|p| v(p.pos)),
            GameplaySel::Npc(id) => self.npcs.iter().find(|p| p.id == id).map(|p| v(p.pos)),
            GameplaySel::Node(id) => self.nodes.iter().find(|p| p.id == id).map(|p| v(p.pos)),
            GameplaySel::Spawn(id) => self
                .spawns
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.centroid())
                .map(|c| Vec3::new(c.x, 0.0, c.y)),
            GameplaySel::OreArea(id) => self
                .ore_areas
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.centroid())
                .map(|c| Vec3::new(c.x, 0.0, c.y)),
        }
    }

    pub fn creature(&self, id: u32) -> Option<&CreatureType> {
        self.creatures.iter().find(|c| c.id == id)
    }

    pub fn alloc_id(&mut self) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Every place a player can find something to do (POIs, NPCs, spawn regions, resource
    /// nodes), as world xz points: the pacing analysis measures the gaps between them.
    pub fn content_points(&self) -> Vec<Vec2> {
        let mut v: Vec<Vec2> = Vec::new();
        v.extend(self.pois.iter().map(|p| Vec2::new(p.pos[0], p.pos[2])));
        v.extend(self.npcs.iter().map(|p| Vec2::new(p.pos[0], p.pos[2])));
        v.extend(self.nodes.iter().map(|p| Vec2::new(p.pos[0], p.pos[2])));
        v.extend(self.spawns.iter().filter_map(|s| s.centroid()));
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerStart {
    pub pos: [f32; 3],
    /// Facing (radians, 0 = +x, counter-clockwise seen from above).
    pub yaw: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PoiKind {
    Town,
    QuestHub,
    Dungeon,
    Landmark,
    Camp,
    Vendor,
    FlightPoint,
    Graveyard,
    Portal,
    BossArena,
    Shrine,
    Vista,
    Treasure,
    Custom,
}

impl PoiKind {
    pub const ALL: [PoiKind; 14] = [
        PoiKind::Town,
        PoiKind::QuestHub,
        PoiKind::Dungeon,
        PoiKind::Landmark,
        PoiKind::Camp,
        PoiKind::Vendor,
        PoiKind::FlightPoint,
        PoiKind::Graveyard,
        PoiKind::Portal,
        PoiKind::BossArena,
        PoiKind::Shrine,
        PoiKind::Vista,
        PoiKind::Treasure,
        PoiKind::Custom,
    ];
    pub fn name(self) -> &'static str {
        match self {
            PoiKind::Town => "Town",
            PoiKind::QuestHub => "Quest Hub",
            PoiKind::Dungeon => "Dungeon Entrance",
            PoiKind::Landmark => "Landmark",
            PoiKind::Camp => "Camp",
            PoiKind::Vendor => "Vendor",
            PoiKind::FlightPoint => "Flight Point",
            PoiKind::Graveyard => "Graveyard / Respawn",
            PoiKind::Portal => "Portal",
            PoiKind::BossArena => "Boss Arena",
            PoiKind::Shrine => "Shrine",
            PoiKind::Vista => "Vista",
            PoiKind::Treasure => "Treasure",
            PoiKind::Custom => "Custom",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Poi {
    pub id: u32,
    pub kind: PoiKind,
    pub name: String,
    pub pos: [f32; 3],
    /// Discovery radius (m): entering it counts as finding the place.
    pub radius: f32,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Recommended level range (None: the zone's).
    #[serde(default)]
    pub level: Option<[u8; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NpcRole {
    QuestGiver,
    Vendor,
    Trainer,
    Guard,
    Innkeeper,
    FlightMaster,
    Banker,
    Ambient,
    Custom,
}

impl NpcRole {
    pub const ALL: [NpcRole; 9] = [
        NpcRole::QuestGiver,
        NpcRole::Vendor,
        NpcRole::Trainer,
        NpcRole::Guard,
        NpcRole::Innkeeper,
        NpcRole::FlightMaster,
        NpcRole::Banker,
        NpcRole::Ambient,
        NpcRole::Custom,
    ];
    pub fn name(self) -> &'static str {
        match self {
            NpcRole::QuestGiver => "Quest Giver",
            NpcRole::Vendor => "Vendor",
            NpcRole::Trainer => "Trainer",
            NpcRole::Guard => "Guard",
            NpcRole::Innkeeper => "Innkeeper",
            NpcRole::FlightMaster => "Flight Master",
            NpcRole::Banker => "Banker",
            NpcRole::Ambient => "Ambient",
            NpcRole::Custom => "Custom",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Npc {
    pub id: u32,
    pub name: String,
    pub role: NpcRole,
    pub pos: [f32; 3],
    pub yaw: f32,
    /// The POI this NPC belongs to, if any.
    #[serde(default)]
    pub poi: Option<u32>,
    #[serde(default)]
    pub notes: String,
    /// Its character, behavior, attitude, dialogue and moves (Labs: `crate::characters::npc`).
    #[serde(default, skip_serializing_if = "crate::characters::npc::NpcLife::is_plain")]
    pub life: crate::characters::npc::NpcLife,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Aggression {
    Passive,
    Neutral,
    Hostile,
}

/// An entry of the creature library (what a spawn region's encounter table refers to).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreatureType {
    pub id: u32,
    pub name: String,
    /// Family / model group, e.g. "wolf", "bandit", "spider".
    #[serde(default)]
    pub family: String,
    pub level: [u8; 2],
    pub aggression: Aggression,
    #[serde(default)]
    pub elite: bool,
    #[serde(default)]
    pub notes: String,
    /// The library character it looks like (Labs: `crate::characters`; `None`: a marker only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpawnBehaviour {
    Roam,
    Patrol,
    Static,
    Ambush,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpawnEntry {
    /// `CreatureType::id`
    pub creature: u32,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpawnRegion {
    pub id: u32,
    pub name: String,
    /// Outline in world xz (closed; at least 3 points).
    pub polygon: Vec<[f32; 2]>,
    /// Encounter table.
    pub entries: Vec<SpawnEntry>,
    /// Creatures alive at once per km² of the region.
    pub density: f32,
    /// Hard cap on creatures alive at once (0 = from density only).
    #[serde(default)]
    pub max_alive: u32,
    pub respawn_secs: f32,
    pub level: [u8; 2],
    /// Creatures per group.
    pub pack: [u8; 2],
    pub behaviour: SpawnBehaviour,
    #[serde(default)]
    pub notes: String,
}

impl SpawnRegion {
    pub fn centroid(&self) -> Option<Vec2> {
        if self.polygon.is_empty() {
            return None;
        }
        let s = self
            .polygon
            .iter()
            .fold(Vec2::ZERO, |a, p| a + Vec2::new(p[0], p[1]));
        Some(s / self.polygon.len() as f32)
    }

    /// Area in m² (shoelace).
    pub fn area(&self) -> f32 {
        let n = self.polygon.len();
        if n < 3 {
            return 0.0;
        }
        let mut a = 0.0;
        for i in 0..n {
            let p = self.polygon[i];
            let q = self.polygon[(i + 1) % n];
            a += p[0] * q[1] - q[0] * p[1];
        }
        (a * 0.5).abs()
    }

    pub fn contains(&self, p: Vec2) -> bool {
        let n = self.polygon.len();
        let mut inside = false;
        let mut j = n.wrapping_sub(1);
        for i in 0..n {
            let (a, b) = (self.polygon[i], self.polygon[j]);
            if (a[1] > p.y) != (b[1] > p.y)
                && p.x < (b[0] - a[0]) * (p.y - a[1]) / (b[1] - a[1]) + a[0]
            {
                inside = !inside;
            }
            j = i;
        }
        inside
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    Ore,
    Herb,
    Wood,
    Stone,
    Crystal,
    Custom,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 6] = [
        ResourceKind::Ore,
        ResourceKind::Herb,
        ResourceKind::Wood,
        ResourceKind::Stone,
        ResourceKind::Crystal,
        ResourceKind::Custom,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ResourceKind::Ore => "Ore",
            ResourceKind::Herb => "Herb",
            ResourceKind::Wood => "Wood",
            ResourceKind::Stone => "Stone",
            ResourceKind::Crystal => "Crystal",
            ResourceKind::Custom => "Custom",
        }
    }
}

/// A gatherable node (fishing spots are the water tab's fish schools).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResourceNode {
    pub id: u32,
    pub kind: ResourceKind,
    /// What exactly, e.g. "Copper Vein", "Silverleaf", "Old Oak".
    pub variant: String,
    /// Gathering tier (1 = starter).
    pub tier: u8,
    pub pos: [f32; 3],
    pub respawn_secs: f32,
    /// Placed by "Scatter resources" (a new scatter replaces these, hand-placed nodes stay).
    #[serde(default)]
    pub auto: bool,
    /// What a mining node yields (see `ore()` for nodes saved before ores existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ore: Option<Ore>,
    /// The ore deposit area that scattered it (re-scattered with the area).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<u32>,
}

impl ResourceNode {
    /// The ore of a mining or quarry node: its own, or for mining nodes saved before ores existed,
    /// the one its name or tier suggests. `None` for herbs, wood and plain stone nodes.
    pub fn ore(&self) -> Option<Ore> {
        self.ore.or_else(|| {
            (self.kind == ResourceKind::Ore).then(|| Ore::from_legacy(&self.variant, self.tier))
        })
    }
}

/// The Gameplay tab's tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GameplayTool {
    #[default]
    Select,
    PlayerStart,
    Poi,
    Npc,
    Spawn,
    Node,
    /// ore deposits: one per click, or a drawn area
    Ore,
    Scatter,
    Remove,
}

/// How the Ore tool places deposits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OreMode {
    /// one deposit per click
    #[default]
    Single,
    /// outline an area that fills itself with deposits
    Area,
}

/// Which markers show, and where.
#[derive(Clone, Debug)]
pub struct MarkerView {
    pub pois: bool,
    pub npcs: bool,
    pub spawns: bool,
    pub nodes: bool,
    pub labels: bool,
    /// Also outside the Gameplay tab (pins only; spawn regions show on the ground in the tab).
    pub all_tabs: bool,
    /// In walk mode.
    pub walk: bool,
}

impl Default for MarkerView {
    fn default() -> Self {
        Self {
            pois: true,
            npcs: true,
            spawns: true,
            nodes: true,
            labels: true,
            all_tabs: false,
            walk: false,
        }
    }
}

/// Editor-side state of the Gameplay tab (not saved): tool settings, the spawn region being
/// drawn, marker visibility.
#[derive(Resource)]
pub struct GameplayEditor {
    pub tool: GameplayTool,
    pub poi_kind: PoiKind,
    pub npc_role: NpcRole,
    pub node_kind: ResourceKind,
    /// Variant for hand-placed nodes (empty: picked from the kind, tier and ground).
    pub node_variant: String,
    /// Tier for hand-placed nodes (0: from the zone's level range).
    pub node_tier: u8,
    /// Creatures new spawn regions start with.
    pub spawn_creatures: Vec<u32>,
    /// Outline of the spawn region (or ore deposit area) being drawn (ground points).
    pub draft: Vec<Vec3>,
    /// The ore the Ore tool places, and how.
    pub ore: Ore,
    pub ore_mode: OreMode,
    pub scatter: scatter::ScatterParams,
    pub view: MarkerView,
    /// The creature open in the library editor.
    pub creature: Option<u32>,
    /// An inspector edit is being recorded (one undo step until the widget is let go).
    pub editing: bool,
    /// The inspector was drawn since the viewport tools last ran (else a pending edit is
    /// committed: the tab was left mid-edit).
    pub inspector_drawn: bool,
    /// Placement warnings of the whole layer (refreshed twice a second while the tab is open).
    pub warnings: Vec<(GameplaySel, String)>,
    pub warnings_at: f32,
}

impl Default for GameplayEditor {
    fn default() -> Self {
        Self {
            tool: GameplayTool::Select,
            poi_kind: PoiKind::QuestHub,
            npc_role: NpcRole::QuestGiver,
            node_kind: ResourceKind::Ore,
            node_variant: String::new(),
            node_tier: 0,
            spawn_creatures: vec![],
            draft: vec![],
            ore: Ore::Copper,
            ore_mode: OreMode::Single,
            scatter: default(),
            view: default(),
            creature: None,
            editing: false,
            inspector_drawn: false,
            warnings: vec![],
            warnings_at: f32::MIN,
        }
    }
}

pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Gameplay>()
            .init_resource::<GameplayEditor>();
        viewport::build(app);
        markers::build(app);
        deposits::build(app);
        mcp::register(app);
        app.add_systems(Update, mcp::auto_fill_ore);
    }
}

#[cfg(test)]
mod tests {
    use super::Gameplay;

    #[test]
    fn legacy_gameplay_leaves_level_cap_unset() {
        let gameplay: Gameplay = serde_json::from_str("{}").expect("legacy gameplay loads");
        assert_eq!(gameplay.level_cap, None);
        let saved = serde_json::to_value(gameplay).expect("gameplay saves");
        assert!(saved.get("level_cap").is_none());
    }

    #[test]
    fn gameplay_persists_explicit_level_cap() {
        let gameplay = Gameplay {
            level_cap: Some(80),
            ..Default::default()
        };
        let saved = serde_json::to_value(&gameplay).expect("gameplay saves");
        assert_eq!(saved["level_cap"], 80);
        let loaded: Gameplay = serde_json::from_value(saved).expect("gameplay reloads");
        assert_eq!(loaded.level_cap, Some(80));
    }
}
