//! What a dungeon is, as saved.
//!
//! In the world (`Dungeons`, saved in the world's `world.json`): each dungeon's name, how the game
//! runs it, and its ways in — a liquid portal standing anywhere, an object of the world used as
//! the way in (a cellar door, a well), a plain spot on the ground, or the portal at the end of a
//! cave. The dungeon's own map is a world of its own in the world's folder
//! (`<world>.world/dungeons/<folder>/`).
//!
//! In that map (`DungeonMap`, saved in the map's `world.json`): which dungeon it is, the plan of
//! its rooms (`plan`), its style, its lights and its mood.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use crate::caves::Instancing;

use super::lights::LightDef;
use super::mood::Mood;
use super::plan::Plan;

/// What sort of place a dungeon's map is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MapKind {
    /// Rooms, halls, corridors and caverns underground: no sky, lit by its own lights.
    #[default]
    Interior,
    /// A world of its own with a sky (another dimension, a desert realm): built with the world's
    /// tools.
    Realm,
}

impl MapKind {
    pub const ALL: [MapKind; 2] = [MapKind::Interior, MapKind::Realm];
    pub fn name(self) -> &'static str {
        match self {
            MapKind::Interior => "Underground",
            MapKind::Realm => "Realm (open sky)",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            MapKind::Interior => "interior",
            MapKind::Realm => "realm",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| {
            k.id() == s
                || k.name().eq_ignore_ascii_case(s)
                || (s == "underground" && *k == MapKind::Interior)
        })
    }
}

/// How a dungeon is entered from the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EntranceKind {
    /// A swirling liquid portal in a stone arch, standing wherever it is put (an object: move,
    /// turn and resize it like any other).
    #[default]
    Portal,
    /// An object of the world is the way in (a cellar door, a well, a statue): using it in the
    /// game enters the dungeon. Nothing is added.
    Object,
    /// A spot on the ground (a trapdoor you built, the bottom of a pit): the game decides how it's
    /// used. Nothing is added.
    Point,
    /// The portal at the end of a cave (Terrain › Caves).
    Cave,
}

impl EntranceKind {
    pub const ALL: [EntranceKind; 4] = [
        EntranceKind::Portal,
        EntranceKind::Object,
        EntranceKind::Point,
        EntranceKind::Cave,
    ];
    pub fn name(self) -> &'static str {
        match self {
            EntranceKind::Portal => "Portal",
            EntranceKind::Object => "An object",
            EntranceKind::Point => "A spot",
            EntranceKind::Cave => "End of a cave",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            EntranceKind::Portal => "portal",
            EntranceKind::Object => "object",
            EntranceKind::Point => "point",
            EntranceKind::Cave => "cave",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    pub fn hint(self) -> &'static str {
        match self {
            EntranceKind::Portal => {
                "Click the ground: a liquid portal in a stone arch stands there (move and turn it like any object)"
            }
            EntranceKind::Object => {
                "Click an object of the world (a cellar door, a well, a statue): using it enters the dungeon"
            }
            EntranceKind::Point => {
                "Click a spot on the ground: an invisible way in (the bottom of a stair, a trapdoor you built)"
            }
            EntranceKind::Cave => "Click a cave: a portal opens at the end of its deepest chamber",
        }
    }
}

/// The colour of a portal's liquid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PortalColor {
    /// pale blue-white (arcane)
    #[default]
    Arcane,
    /// deep violet (void, shadow)
    Void,
    /// sickly green (fel, plague)
    Fel,
    /// orange and red (fire, the underworld)
    Fire,
    /// gold and white (holy, celestial)
    Holy,
    /// icy cyan (frost)
    Frost,
    /// blood red
    Blood,
    /// sea green (tides, drowned places)
    Tide,
}

impl PortalColor {
    pub const ALL: [PortalColor; 8] = [
        PortalColor::Arcane,
        PortalColor::Void,
        PortalColor::Fel,
        PortalColor::Fire,
        PortalColor::Holy,
        PortalColor::Frost,
        PortalColor::Blood,
        PortalColor::Tide,
    ];
    pub fn name(self) -> &'static str {
        match self {
            PortalColor::Arcane => "Arcane",
            PortalColor::Void => "Void",
            PortalColor::Fel => "Fel",
            PortalColor::Fire => "Fire",
            PortalColor::Holy => "Holy",
            PortalColor::Frost => "Frost",
            PortalColor::Blood => "Blood",
            PortalColor::Tide => "Tide",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            PortalColor::Arcane => "arcane",
            PortalColor::Void => "void",
            PortalColor::Fel => "fel",
            PortalColor::Fire => "fire",
            PortalColor::Holy => "holy",
            PortalColor::Frost => "frost",
            PortalColor::Blood => "blood",
            PortalColor::Tide => "tide",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// (core, rim) colours, linear rgb: the bright middle and the glowing edge.
    pub fn colors(self) -> ([f32; 3], [f32; 3]) {
        match self {
            PortalColor::Arcane => ([0.62, 0.78, 1.0], [0.16, 0.32, 1.0]),
            PortalColor::Void => ([0.55, 0.3, 0.95], [0.25, 0.05, 0.6]),
            PortalColor::Fel => ([0.55, 1.0, 0.35], [0.12, 0.6, 0.05]),
            PortalColor::Fire => ([1.0, 0.62, 0.22], [0.9, 0.18, 0.02]),
            PortalColor::Holy => ([1.0, 0.93, 0.68], [1.0, 0.7, 0.2]),
            PortalColor::Frost => ([0.7, 0.98, 1.0], [0.15, 0.65, 0.95]),
            PortalColor::Blood => ([1.0, 0.3, 0.28], [0.55, 0.02, 0.04]),
            PortalColor::Tide => ([0.45, 1.0, 0.85], [0.03, 0.45, 0.4]),
        }
    }
}

/// One way into a dungeon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entrance {
    pub id: u32,
    pub kind: EntranceKind,
    /// where it is in the world and which way it faces (radians, 0 = +z). A portal or an object
    /// way in follows its object; a cave's follows the cave's portal.
    pub pos: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    /// the portal object (Portal) or the object used as the way in (Object)
    #[serde(default)]
    pub object: Option<u64>,
    /// the cave whose end it is (Cave)
    #[serde(default)]
    pub cave: Option<u32>,
    #[serde(default)]
    pub color: PortalColor,
    /// what players see on it ("Cellar door")
    #[serde(default)]
    pub label: String,
}

/// A dungeon of this world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DungeonDef {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub kind: MapKind,
    #[serde(default)]
    pub instancing: Instancing,
    /// recommended player levels
    #[serde(default = "default_levels")]
    pub levels: [u8; 2],
    /// players it's tuned for (5, 10, 20, 40…)
    #[serde(default = "five")]
    pub players: u8,
    #[serde(default)]
    pub entrances: Vec<Entrance>,
    /// its map: `<world>.world/dungeons/<folder>/` (a world folder of its own)
    pub folder: String,
    /// the one-click layout it was made from (`presets::Preset::id`)
    #[serde(default)]
    pub layout: String,
    /// the style it was made in (`styles::Style::id`)
    #[serde(default)]
    pub style: String,
    /// how big the layout was made
    #[serde(default)]
    pub size: super::presets::Size,
    /// how big its rooms were made
    #[serde(default)]
    pub room_size: super::presets::RoomSize,
    /// how uneven its rooms were made, 0..1 (`presets::roughen`)
    #[serde(default)]
    pub irregularity: f32,
    /// its art: hand-painted or photoreal
    #[serde(default)]
    pub art: super::styles::Art,
    /// the mood it was made in (`mood::PRESETS`); empty: the layout's
    #[serde(default)]
    pub mood: String,
    #[serde(default)]
    pub notes: String,
    /// a realm's land (`realm_shapes`): its shape (None: a realm made before shapes, the classic
    /// island), its size and the seed it's rolled from (None: from its id). Made into land when
    /// the realm is first opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<super::realm_shapes::RealmShape>,
    #[serde(default)]
    pub realm_size: super::realm_shapes::RealmSize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    /// a temple compound laid out in the realm's land when it is first opened (`compound`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compound: Option<super::compound::Options>,
    /// a design sheet the realm's land and dressing are built from when it is first opened
    /// (`sheets`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<super::sheets::SheetRef>,
}

fn default_levels() -> [u8; 2] {
    [10, 15]
}

fn five() -> u8 {
    5
}

/// The dungeons of the open world, and their ways in.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Resource)]
pub struct Dungeons {
    pub list: Vec<DungeonDef>,
    #[serde(default)]
    pub next_id: u32,
    #[serde(default)]
    pub next_entrance: u32,
    /// the dungeon picked in the Dungeons tab
    #[serde(skip)]
    pub selected: Option<u32>,
}

impl Dungeons {
    pub fn alloc_id(&mut self) -> u32 {
        self.next_id = self
            .next_id
            .max(self.list.iter().map(|d| d.id).max().unwrap_or(0))
            + 1;
        self.next_id
    }
    pub fn alloc_entrance(&mut self) -> u32 {
        let top = self
            .list
            .iter()
            .flat_map(|d| d.entrances.iter().map(|e| e.id))
            .max()
            .unwrap_or(0);
        self.next_entrance = self.next_entrance.max(top) + 1;
        self.next_entrance
    }
    pub fn get(&self, id: u32) -> Option<&DungeonDef> {
        self.list.iter().find(|d| d.id == id)
    }
    pub fn get_mut(&mut self, id: u32) -> Option<&mut DungeonDef> {
        self.list.iter_mut().find(|d| d.id == id)
    }
    /// The dungeon and entrance an object is the way in of (a portal, or a linked object).
    pub fn by_object(&self, object: u64) -> Option<(u32, u32)> {
        self.list.iter().find_map(|d| {
            d.entrances
                .iter()
                .find(|e| e.object == Some(object))
                .map(|e| (d.id, e.id))
        })
    }
    /// The dungeon whose way in is the end of cave `cave`.
    pub fn by_cave(&self, cave: u32) -> Option<(u32, u32)> {
        self.list.iter().find_map(|d| {
            d.entrances
                .iter()
                .find(|e| e.kind == EntranceKind::Cave && e.cave == Some(cave))
                .map(|e| (d.id, e.id))
        })
    }
    /// A folder name for a new dungeon's map (unique among this world's dungeons).
    pub fn folder_for(&self, id: u32, name: &str) -> String {
        let slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let slug = slug.trim_matches('_').chars().take(32).collect::<String>();
        let slug = if slug.is_empty() {
            "dungeon".to_string()
        } else {
            slug
        };
        format!("{id:03}_{slug}")
    }
}

/// This world is the map of a dungeon (saved in its `world.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapDoc {
    /// the dungeon's id in its world (`DungeonDef::id`)
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub kind: MapKind,
    /// the rooms, halls, corridors, caverns, stairs and pits (interior maps)
    #[serde(default)]
    pub plan: Plan,
    /// its art (the styles, fixtures and dressing it takes)
    #[serde(default)]
    pub art: super::styles::Art,
    /// the style everything is built in unless a room says otherwise (`styles::Style::id`)
    #[serde(default = "default_style")]
    pub style: String,
    /// the light sources (placed by hand or by Auto-light)
    #[serde(default)]
    pub lights: Vec<LightDef>,
    /// Lights › All lights: brightness, reach and flicker over every light at once
    #[serde(default)]
    pub all_lights: super::lights::AllLights,
    #[serde(default)]
    pub next_light: u32,
    /// light, fog and sound of the place
    #[serde(default)]
    pub mood: Mood,
    /// its own music or ambience track
    #[serde(default)]
    pub music: super::sound::Music,
    /// the objects the builder put in (dressing, doorways, portals): replaced when it builds
    /// again. Objects placed by hand are never touched.
    #[serde(default)]
    pub props: Vec<u64>,
    /// the way back out: its portal's frame (an object of the map, standing where `way_out` says)
    #[serde(default)]
    pub exit: Option<u64>,
    /// the way back out's look and where it stands
    #[serde(default)]
    pub way_out: WayOut,
    /// what open-ceilinged rooms show overhead: darkness, or a sky (courtyards, ruined halls,
    /// a city under an open sky)
    #[serde(default)]
    pub sky: SkyMode,
    /// how weathered it is, on top of its style's own (multipliers)
    #[serde(default)]
    pub weathering: Weathering,
    /// cracks, stains, drips, blood, scorch marks, runes on its walls and floors
    #[serde(default)]
    pub decals: Vec<super::decals::Decal>,
    #[serde(default)]
    pub next_decal: u32,
    /// doors, gates, bars, secret doors, levers, plates, traps and the game's markers
    #[serde(default)]
    pub pieces: Vec<super::pieces::Piece>,
    #[serde(default)]
    pub next_piece: u32,
    /// a sheet realm's markers: where its design sheet puts things (`sheets::markers`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<super::sheets::markers::Marker>,
    /// Sheet-bridge migration state.  The ids identify the authored roads that came from the
    /// sheet; once recorded, opening a map never recreates a bridge a player has removed.
    #[serde(default)]
    pub sheet_bridges: super::sheets::bridges::State,
}

/// The way back out of a dungeon's map: a portal like the way in players came through (its frame
/// and colour, unless changed here), standing behind where players arrive unless put somewhere.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WayOut {
    /// its frame (`edit::FRAMES`; "" the liquid alone); None: the way in's
    #[serde(default)]
    pub frame: Option<String>,
    /// its colour; None: the way in's
    #[serde(default)]
    pub color: Option<PortalColor>,
    /// where its foot stands; None: behind where players arrive (facing them)
    #[serde(default)]
    pub at: Option<[f32; 3]>,
    /// which way it faces where it was put (radians, 0 = +z; players walk out against it)
    #[serde(default)]
    pub yaw: f32,
    /// the way in's frame and colour (the one players came through, else the dungeon's first
    /// portal), kept each time the map is opened from its world
    #[serde(default)]
    pub way_in: Option<(String, PortalColor)>,
}

/// Weathering on top of a style's own: moss, grime, wet, puddles (1 = the style's; 0 none).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weathering {
    pub moss: f32,
    pub grime: f32,
    pub wet: f32,
    pub puddles: f32,
    /// broken stones piled at the foot of the walls and in corners
    #[serde(default = "one")]
    pub rubble: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for Weathering {
    fn default() -> Self {
        Weathering {
            moss: 1.0,
            grime: 1.0,
            wet: 1.0,
            puddles: 1.0,
            rubble: 1.0,
        }
    }
}

/// The sky over a dungeon's open-ceilinged rooms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkyMode {
    /// darkness overhead (vast halls, chasms)
    #[default]
    None,
    Day,
    Dusk,
    Night,
}

impl SkyMode {
    pub const ALL: [SkyMode; 4] = [SkyMode::None, SkyMode::Day, SkyMode::Dusk, SkyMode::Night];
    pub fn id(self) -> &'static str {
        match self {
            SkyMode::None => "none",
            SkyMode::Day => "day",
            SkyMode::Dusk => "dusk",
            SkyMode::Night => "night",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            SkyMode::None => "Darkness",
            SkyMode::Day => "Day",
            SkyMode::Dusk => "Dusk",
            SkyMode::Night => "Night",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// The hour it shows, the sun's (or moon's) light for an eye opened to the dungeon, and the
    /// exposure an eye adapts to under it.
    pub fn hour(self) -> f32 {
        match self {
            SkyMode::None | SkyMode::Day => 13.5,
            SkyMode::Dusk => 18.7,
            SkyMode::Night => 23.0,
        }
    }
    /// (night and dusk stay close to the inside's exposure: torches still glow, the moon is faint)
    pub fn ev100(self) -> f32 {
        match self {
            SkyMode::None => 9.6,
            SkyMode::Day => 13.6,
            SkyMode::Dusk => 10.6,
            SkyMode::Night => 9.2,
        }
    }
}

fn default_style() -> String {
    "crypt_stone".into()
}

impl MapDoc {
    pub fn new(id: u32, name: &str, kind: MapKind) -> Self {
        MapDoc {
            id,
            name: name.to_string(),
            kind,
            art: super::styles::Art::HandPainted,
            plan: Plan::default(),
            style: default_style(),
            lights: vec![],
            all_lights: Default::default(),
            next_light: 0,
            mood: Mood::default(),
            music: Default::default(),
            props: vec![],
            exit: None,
            way_out: WayOut::default(),
            sky: SkyMode::None,
            weathering: Weathering::default(),
            decals: vec![],
            next_decal: 0,
            pieces: vec![],
            next_piece: 0,
            markers: vec![],
            sheet_bridges: Default::default(),
        }
    }
    pub fn alloc_piece(&mut self) -> u32 {
        self.next_piece = self
            .next_piece
            .max(self.pieces.iter().map(|d| d.id + 1).max().unwrap_or(0));
        self.next_piece += 1;
        self.next_piece - 1
    }
    pub fn alloc_decal(&mut self) -> u32 {
        self.next_decal = self
            .next_decal
            .max(self.decals.iter().map(|d| d.id + 1).max().unwrap_or(0));
        self.next_decal += 1;
        self.next_decal - 1
    }
    pub fn alloc_light(&mut self) -> u32 {
        self.next_light = self
            .next_light
            .max(self.lights.iter().map(|l| l.id).max().unwrap_or(0))
            + 1;
        self.next_light
    }
}

/// The open world is a dungeon's map (`Some`), or the open world (`None`).
#[derive(Clone, Debug, Default, PartialEq, Resource)]
pub struct DungeonMap(pub Option<MapDoc>);

impl DungeonMap {
    pub fn interior(&self) -> bool {
        self.0.as_ref().is_some_and(|m| m.kind == MapKind::Interior)
    }
}
