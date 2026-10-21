//! Dungeons › Create a dungeon › Describe it: a whole dungeon from a few words (and pictures).
//!
//! The user writes what they want ("a flooded dwarven mine with a collapsed forge") or picks one
//! of the ideas, may add reference images, and presses Generate. The words are read for a layout
//! (or a realm), style, mood, size, room size, irregularity, a name, and the portal's colour and
//! frame (`read`); the dungeon is created with a portal where the camera looks (or at a spot
//! picked on the ground), its map is opened, laid out, lit and dressed like any new dungeon — and
//! then, when an AI is connected (Prompt an area's backends), the AI shapes the whole of it from
//! the description and the images with the dungeon tools (`prompt::start_dungeon_shape`): rooms,
//! halls, lights and their colours, dressing, music, the way out. Its progress shows here and in
//! the prompt panel (Stop; then Keep / Undo / Refine). Without an AI the words alone make it.
//!
//! MCP: `dungeons_create_from_prompt` (returns at once with a run id) and
//! `dungeons_create_from_prompt_status`. Test hooks: `WB_DUNGEON_DESCRIBE="<text>"` (or
//! `idea:<id>`) opens the page with that text (once `WB_DUNGEON_DESCRIBE_WAIT=<file>` exists, if
//! given), `WB_DUNGEON_DESCRIBE_IMAGES=<a.png,b.jpg>` attaches pictures, and
//! `WB_DUNGEON_DESCRIBE_GO=1` (or `=<file>`: once it exists) presses Generate.

use std::path::PathBuf;
use std::time::Instant;

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;
use serde_json::{Value, json};

use super::edit::{NewDungeon, NewEntrance};
use super::model::*;
use super::plan::{Liquid, RoomKind};
use super::presets::{self, RoomSize, Size};
use super::ui::{DungeonTool, Request};
use super::{mood, styles};
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// Reference images a description may carry.
const MAX_IMAGES: usize = 4;
const GREEN: Color32 = Color32::from_rgb(74, 210, 120);
const AMBER: Color32 = Color32::from_rgb(232, 180, 90);

// ------------------------------------------------------------------ ideas

/// A ready-made description (a chip on the page) with the settings that suit it best.
pub struct Idea {
    pub id: &'static str,
    pub chip: &'static str,
    pub text: &'static str,
    name: &'static str,
    layout: &'static str,
    style: &'static str,
    mood: &'static str,
    size: Size,
    room_size: RoomSize,
    irregularity: f32,
    color: PortalColor,
    frame: &'static str,
    liquid: Option<Liquid>,
    age: f32,
    light: f32,
    wet: f32,
    moss: f32,
}

pub const IDEAS: [Idea; 8] = [
    Idea {
        id: "flooded_mine",
        chip: "Flooded dwarven mine",
        text: "A flooded dwarven mine with a collapsed forge: black water filling the lower drifts, broken ore carts on rusted rails, a great forge hall half caved in, and lanterns still burning by the shaft.",
        name: "The Drowned Delve",
        layout: "mine",
        style: "dwarven_forge",
        mood: "sunken",
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.35,
        color: PortalColor::Tide,
        frame: "hp:portal_dwarven",
        liquid: Some(Liquid::Water),
        age: 1.4,
        light: 0.9,
        wet: 2.0,
        moss: 1.0,
    },
    Idea {
        id: "vampire_crypt",
        chip: "Vampire crypt",
        text: "A vampire lord's crypt beneath a cathedral: a moonlit nave above, a narrow stair down into candlelit burial vaults, and the lord's coffin on a dais in a blood-red chamber at the very bottom.",
        name: "The Sanguine Undercroft",
        layout: "cathedral",
        style: "cathedral_hp",
        mood: "moonlit",
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.0,
        color: PortalColor::Blood,
        frame: "hp:portal_gate",
        liquid: Some(Liquid::Blood),
        age: 1.2,
        light: 0.8,
        wet: 1.0,
        moss: 1.0,
    },
    Idea {
        id: "goblin_warren",
        chip: "Goblin warren",
        text: "A goblin warren inside a hollow hill: cramped, twisting tunnels, smoky torchlit dens full of junk, a feasting cave round a firepit, and the chieftain's throne of scrap at the heart of it.",
        name: "The Gristle Hollow",
        layout: "goblin_cave",
        style: "cave_rock",
        mood: "torchlit",
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.6,
        color: PortalColor::Fel,
        frame: "hp:portal_ruin",
        liquid: None,
        age: 1.2,
        light: 1.0,
        wet: 1.0,
        moss: 1.2,
    },
    Idea {
        id: "ice_dragon",
        chip: "Ice dragon's lair",
        text: "A frozen temple around an ice dragon's lair: frost-glazed halls lined with pillars of ice, cold blue light through frozen walls, and a vast cavern where the dragon sleeps on its hoard.",
        name: "The Rimefang Sanctum",
        layout: "frost_halls",
        style: "frost_halls",
        mood: "frost",
        size: Size::Large,
        room_size: RoomSize::Large,
        irregularity: 0.2,
        color: PortalColor::Frost,
        frame: "hp:portal_spires",
        liquid: None,
        age: 0.6,
        light: 1.0,
        wet: 1.0,
        moss: 0.0,
    },
    Idea {
        id: "smugglers_cave",
        chip: "Smugglers' sea cave",
        text: "A sunken sea-cave smugglers' den: tide pools and a hidden cove, crates and barrels stacked in lantern-lit side caves, and a secret tunnel up to the cellar of a ruined lighthouse.",
        name: "Saltlight Grotto",
        layout: "great_caverns",
        style: "cave_rock",
        mood: "sunken",
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.5,
        color: PortalColor::Tide,
        frame: "hp:portal_ruin",
        liquid: Some(Liquid::Water),
        age: 1.0,
        light: 0.9,
        wet: 2.2,
        moss: 1.4,
    },
    Idea {
        id: "clockwork_foundry",
        chip: "Clockwork foundry",
        text: "A clockwork foundry: roaring machine halls, catwalks over glowing furnace pits, brass gears turning in the walls, and a reactor core guarded by a giant automaton.",
        name: "The Brasswork Foundry",
        layout: "clockwork",
        style: "clockwork_hp",
        mood: "industrial",
        size: Size::Medium,
        room_size: RoomSize::Large,
        irregularity: 0.0,
        color: PortalColor::Fire,
        frame: "hp:portal_dwarven",
        liquid: Some(Liquid::Lava),
        age: 0.8,
        light: 1.2,
        wet: 1.0,
        moss: 0.5,
    },
    Idea {
        id: "spider_nest",
        chip: "Spider queen's nest",
        text: "A spider queen's nest: webbed caverns round deep shafts, cocooned victims hanging in the dark, egg chambers glowing violet, and the queen's lair at the very bottom.",
        name: "The Silken Deep",
        layout: "spider_den",
        style: "spider_den",
        mood: "spider",
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.5,
        color: PortalColor::Void,
        frame: "hp:portal_ring",
        liquid: Some(Liquid::Slime),
        age: 1.0,
        light: 0.7,
        wet: 1.2,
        moss: 1.0,
    },
    Idea {
        id: "haunted_prison",
        chip: "Haunted prison",
        text: "A haunted prison keep: cell blocks thick with pale fog, chains rattling in empty cells, a flooded interrogation pit, and the warden's hall where his ghost still walks.",
        name: "Blackchain Gaol",
        layout: "prison_keep",
        style: "keep_stone",
        mood: "haunted",
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.15,
        color: PortalColor::Void,
        frame: "hp:portal_ruin",
        liquid: Some(Liquid::Water),
        age: 1.6,
        light: 0.7,
        wet: 1.5,
        moss: 1.0,
    },
];

pub fn idea(id: &str) -> Option<usize> {
    IDEAS.iter().position(|i| i.id == id)
}

impl Idea {
    fn picks(&self) -> Picks {
        Picks {
            kind: MapKind::Interior,
            layout: self.layout.into(),
            style: self.style.into(),
            mood: self.mood.into(),
            size: self.size,
            room_size: self.room_size,
            irregularity: self.irregularity,
            name: self.name.into(),
            color: self.color,
            frame: self.frame.into(),
            instancing: Instancing::Party,
            light: self.light,
            age: self.age,
            wet: self.wet,
            moss: self.moss,
            liquid: self.liquid,
            words: vec![],
            idea: Some(self.id),
            realm_shape: String::new(),
        }
    }
}

// ------------------------------------------------------------------ reading the words

/// What a description gives: everything a new dungeon is made with, and a few touches after.
#[derive(Clone, Debug, PartialEq)]
pub struct Picks {
    pub kind: MapKind,
    /// a layout (`presets::all`) or a realm (`presets::realms`)
    pub layout: String,
    /// a style id ("" for a realm)
    pub style: String,
    /// a mood preset id ("" for a realm)
    pub mood: String,
    pub size: Size,
    pub room_size: RoomSize,
    pub irregularity: f32,
    pub name: String,
    pub color: PortalColor,
    /// the portal's frame ("" = the first the library has)
    pub frame: String,
    pub instancing: Instancing,
    /// Auto-light's density (1 = normal)
    pub light: f32,
    /// Age the walls (0: not aged)
    pub age: f32,
    /// weathering multipliers over the style's own
    pub wet: f32,
    pub moss: f32,
    /// what fills its pools (and its pits that hold a liquid)
    pub liquid: Option<Liquid>,
    /// the words it went by
    pub words: Vec<String>,
    /// a ready-made idea's settings
    pub idea: Option<&'static str>,
    /// a realm's land shape from the words (`terrain::realm_land::RealmShape` ids; "" = the
    /// realm's own)
    pub realm_shape: String,
}

/// The land a realm's words describe (ids of `terrain::realm_land::RealmShape`).
const REALM_SHAPE_WORDS: &[(&str, Table)] = &[
    (
        "twin_isles",
        &[("twin", 3), ("two islands", 3), ("strait", 2)],
    ),
    (
        "island_chain",
        &[
            ("archipelago", 3),
            ("chain", 2),
            ("islands", 2),
            ("atoll", 2),
        ],
    ),
    (
        "crescent_bay",
        &[
            ("bay$", 3),
            ("crescent", 3),
            ("cove", 2),
            ("harbour", 2),
            ("harbor", 2),
        ],
    ),
    ("long_isle", &[("long island", 3), ("long isle", 3)]),
    ("round_isle", &[("island", 2), ("isle", 2)]),
    (
        "coastal_shelf",
        &[("coast", 3), ("shore", 2), ("cliffs by the sea", 3)],
    ),
    (
        "enclosed_valley",
        &[("valley", 3), ("vale", 2), ("walled in", 2), ("hidden", 1)],
    ),
    (
        "highland_plateau",
        &[
            ("plateau", 3),
            ("highland", 3),
            ("table land", 3),
            ("tableland", 3),
            ("mesa", 2),
        ],
    ),
    ("lakeland", &[("lake", 3), ("lakes", 3), ("loch", 3)]),
    ("canyon", &[("canyon", 3), ("gorge", 3), ("ravine", 2)]),
    (
        "glade",
        &[
            ("glade", 3),
            ("clearing", 3),
            ("grove", 2),
            ("cozy", 1),
            ("cosy", 1),
        ],
    ),
];

/// (stem, weight): a word starting with the stem; `stem$` the word itself or its plural; a stem
/// with a space is a phrase.
type Table = &'static [(&'static str, u32)];

const LAYOUT_WORDS: &[(&str, Table)] = &[
    (
        "crypt",
        &[
            ("crypt", 3),
            ("tomb", 3),
            ("mausoleum", 3),
            ("barrow", 3),
            ("sepulch", 3),
            ("grave", 2),
            ("burial", 2),
            ("sarcophag", 2),
            ("coffin", 2),
            ("vampir", 2),
            ("lich", 2),
            ("undead", 1),
            ("necroman", 1),
            ("zombie", 1),
            ("mumm", 1),
            ("dead$", 1),
        ],
    ),
    (
        "catacombs",
        &[
            ("catacomb", 3),
            ("ossuar", 3),
            ("labyrinth", 3),
            ("maze", 3),
            ("bone", 2),
            ("skull", 2),
            ("skeleton", 1),
            ("tunnels", 1),
        ],
    ),
    (
        "prison_keep",
        &[
            ("prison", 3),
            ("jail", 3),
            ("gaol", 3),
            ("stockade", 3),
            ("oubliette", 3),
            ("cell$", 2),
            ("torture", 2),
            ("warden", 2),
            ("interrogat", 2),
            ("prisoner", 2),
            ("chain", 1),
            ("captive", 1),
            ("execution", 1),
        ],
    ),
    (
        "sun_temple",
        &[
            ("temple", 3),
            ("ziggurat", 3),
            ("pyramid", 2),
            ("jungle", 2),
            ("sun$", 2),
            ("aztec", 2),
            ("mayan", 2),
            ("idol", 2),
            ("serpent", 1),
            ("altar", 1),
            ("sacrific", 1),
            ("shrine", 1),
        ],
    ),
    (
        "cathedral",
        &[
            ("cathedral", 3),
            ("church", 3),
            ("chapel", 3),
            ("abbey", 3),
            ("monaster", 3),
            ("basilica", 3),
            ("scriptorium", 3),
            ("nave", 2),
            ("gothic", 2),
            ("library", 2),
            ("archive", 2),
            ("cult", 1),
            ("priest", 1),
            ("bell", 1),
            ("holy", 1),
            ("unholy", 1),
            ("organ", 1),
        ],
    ),
    (
        "dwarven_halls",
        &[
            ("dwarf", 2),
            ("dwarv", 2),
            ("forge", 2),
            ("anvil", 2),
            ("smith", 1),
            ("hold$", 1),
            ("throne", 1),
            ("king", 1),
        ],
    ),
    (
        "underground_city",
        &[
            ("undercity", 3),
            ("city", 3),
            ("metropol", 3),
            ("town", 2),
            ("market", 2),
            ("bazaar", 2),
            ("kingdom", 2),
            ("drow", 2),
            ("dark elf", 2),
            ("street", 2),
            ("district", 2),
            ("plaza", 2),
        ],
    ),
    (
        "great_caverns",
        &[
            ("cavern", 3),
            ("cave", 3),
            ("grotto", 3),
            ("chasm", 2),
            ("abyss", 2),
            ("stalact", 2),
            ("stalagm", 2),
            ("underground lake", 2),
            ("lake", 1),
            ("crystal", 1),
            ("mushroom", 1),
            ("fung", 1),
            ("subterran", 1),
            ("lair", 1),
            ("dragon", 1),
            ("cove", 2),
        ],
    ),
    (
        "goblin_cave",
        &[
            ("goblin", 3),
            ("kobold", 3),
            ("warren", 3),
            ("burrow", 3),
            ("hollow hill", 3),
            ("orc", 2),
            ("troll", 2),
            ("ogre", 2),
            ("gnoll", 2),
            ("ratmen", 2),
            ("ratfolk", 2),
            ("bandit", 2),
            ("hideout", 2),
            ("chieftain", 2),
            ("den$", 1),
            ("junk", 1),
        ],
    ),
    (
        "spider_den",
        &[
            ("spider", 3),
            ("arachn", 3),
            ("web", 2),
            ("brood", 2),
            ("nest$", 2),
            ("hive", 2),
            ("insect", 2),
            ("cocoon", 2),
            ("egg", 1),
            ("silk", 1),
            ("queen", 1),
        ],
    ),
    (
        "sewers",
        &[
            ("sewer", 3),
            ("cistern", 3),
            ("aqueduct", 3),
            ("drain", 2),
            ("gutter", 2),
            ("canal", 2),
            ("underbelly", 2),
            ("rat$", 1),
            ("plague", 1),
            ("thieves", 1),
            ("guild", 1),
        ],
    ),
    (
        "mine",
        &[
            ("mine$", 3),
            ("mines", 3),
            ("mining", 3),
            ("quarry", 3),
            ("shaft", 2),
            ("ore$", 2),
            ("miner", 2),
            ("excavat", 2),
            ("cart", 1),
            ("rail", 1),
            ("gold", 1),
            ("gem", 1),
            ("dig$", 1),
            ("lantern", 1),
        ],
    ),
    (
        "clockwork",
        &[
            ("clockwork", 3),
            ("foundry", 3),
            ("factory", 3),
            ("workshop", 3),
            ("gear", 2),
            ("cog", 2),
            ("machine", 2),
            ("mechan", 2),
            ("steam", 2),
            ("automat", 2),
            ("engine", 2),
            ("reactor", 2),
            ("gnom", 2),
            ("brass", 2),
            ("piston", 2),
            ("golem", 1),
            ("furnace", 1),
        ],
    ),
    (
        "frost_halls",
        &[
            ("frost", 3),
            ("frozen", 3),
            ("ice$", 3),
            ("icy", 3),
            ("glacier", 3),
            ("snow", 2),
            ("winter", 2),
            ("rime", 2),
            ("yeti", 2),
            ("blizzard", 2),
            ("cold$", 1),
            ("tundra", 1),
        ],
    ),
    (
        "infernal",
        &[
            ("infernal", 3),
            ("hell", 3),
            ("inferno", 3),
            ("demon", 3),
            ("devil", 3),
            ("brimstone", 3),
            ("lava", 2),
            ("magma", 2),
            ("molten", 2),
            ("volcan", 2),
            ("abyssal", 2),
            ("fire$", 1),
            ("burning", 1),
            ("ash$", 1),
        ],
    ),
    (
        "ruined_fortress",
        &[
            ("fortress", 3),
            ("castle", 3),
            ("keep$", 3),
            ("citadel", 3),
            ("stronghold", 3),
            ("fort$", 3),
            ("bastion", 3),
            ("ruin", 2),
            ("courtyard", 2),
            ("siege", 2),
            ("battlement", 2),
            ("rampart", 2),
            ("knight", 1),
            ("barracks", 1),
        ],
    ),
];

/// Words that say the dungeon is an outdoor temple compound (`compound`): a score of 3 or more.
const COMPOUND_SAID: Table = &[
    ("zul aman", 4),
    ("zulaman", 4),
    ("temple city", 4),
    ("temple cities", 4),
    ("compound$", 4),
    ("ziggurat", 3),
    ("troll", 2),
    ("courts", 1),
    ("temple", 1),
    ("jungle", 1),
    ("pyramid", 1),
    ("raid", 1),
    ("outdoor", 1),
    ("city", 1),
];

/// A temple compound's options from the words: its theme (the realm words), how many courts, and
/// whether they come one after another.
pub fn compound_options(text: &str) -> super::compound::Options {
    use super::compound::{Options, Order, Theme};
    let t = Text::new(text);
    let theme = match t.best(REALM_WORDS).map(|x| x.0) {
        Some("desert_realm") => Theme::Desert,
        Some("frozen_realm") => Theme::Frozen,
        Some("ember_realm") => Theme::Ember,
        Some("twilight_realm") => Theme::Twilight,
        _ => Theme::Jungle,
    };
    let nums = [
        ("three", 3u8),
        ("3", 3),
        ("four", 4),
        ("4", 4),
        ("five", 5),
        ("5", 5),
        ("six", 6),
        ("6", 6),
    ];
    let courts = t
        .words
        .windows(2)
        .find_map(|w| {
            nums.iter()
                .find(|(n, _)| *n == w[0])
                .filter(|_| w[1].starts_with("court") || w[1].starts_with("boss"))
                .map(|(_, k)| *k)
        })
        .unwrap_or(4);
    let order =
        if t.score(&[
            ("linear", 3),
            ("one after another", 3),
            ("in order", 3),
            ("in sequence", 3),
        ])
        .0 > 0
        {
            Order::Linear
        } else {
            Order::Hub
        };
    Options {
        theme,
        courts,
        order,
    }
}

/// Words that say the dungeon is a realm (a world of its own under a sky).
const REALM_SAID: Table = &[
    ("realm$", 1),
    ("open sky", 1),
    ("under the sky", 1),
    ("floating island", 1),
    ("sky island", 1),
    ("pocket world", 1),
    ("pocket dimension", 1),
    ("dreamland", 1),
    ("world of its own", 1),
];

const REALM_WORDS: &[(&str, Table)] = &[
    (
        "desert_realm",
        &[
            ("desert", 3),
            ("sand", 2),
            ("dune", 3),
            ("arid", 2),
            ("oasis", 2),
            ("mesa", 2),
        ],
    ),
    (
        "frozen_realm",
        &[
            ("frozen", 3),
            ("ice$", 3),
            ("icy", 3),
            ("snow", 3),
            ("frost", 3),
            ("glacier", 3),
            ("tundra", 3),
            ("cold$", 2),
        ],
    ),
    (
        "jungle_realm",
        &[
            ("jungle", 3),
            ("tropic", 3),
            ("island", 1),
            ("lagoon", 2),
            ("palm", 2),
            ("steam", 1),
            ("rainforest", 3),
        ],
    ),
    (
        "ember_realm",
        &[
            ("ember", 3),
            ("fire", 2),
            ("lava", 3),
            ("burning", 2),
            ("volcan", 3),
            ("ash$", 2),
            ("hell", 3),
            ("demon", 2),
            ("infernal", 3),
        ],
    ),
    (
        "twilight_realm",
        &[
            ("twilight", 3),
            ("dusk", 3),
            ("dream", 3),
            ("violet", 2),
            ("fae", 3),
            ("fairy", 3),
            ("moon", 2),
            ("mist", 2),
            ("ethereal", 3),
        ],
    ),
    (
        "wild_realm",
        &[
            ("wild", 2),
            ("untamed", 3),
            ("continent", 3),
            ("forest", 1),
            ("mountain", 1),
        ],
    ),
];

const STYLE_WORDS: &[(&str, Table)] = &[
    (
        "cathedral_hp",
        &[("gothic", 2), ("cathedral", 2), ("marble", 2)],
    ),
    ("sewer_hp", &[("brick", 2), ("sewer", 2)]),
    (
        "keep_stone",
        &[("castle", 2), ("keep$", 2), ("fortress", 2), ("prison", 2)],
    ),
    (
        "clockwork_hp",
        &[
            ("clockwork", 3),
            ("brass", 2),
            ("copper", 2),
            ("steam", 2),
            ("machine", 2),
            ("gear", 2),
        ],
    ),
    (
        "nordic_hp",
        &[
            ("nordic", 3),
            ("viking", 3),
            ("norse", 3),
            ("longhouse", 3),
            ("jarl", 2),
            ("timber", 2),
            ("wooden", 2),
            ("mead", 2),
        ],
    ),
    (
        "overgrown_hp",
        &[
            ("overgrown", 3),
            ("vine", 2),
            ("roots", 2),
            ("moss", 2),
            ("druid", 2),
            ("ivy", 2),
            ("forest", 1),
            ("jungle", 1),
        ],
    ),
    (
        "dwarven_forge",
        &[("dwarf", 3), ("dwarv", 3), ("forge", 2), ("anvil", 2)],
    ),
    (
        "frost_halls",
        &[
            ("ice$", 3),
            ("icy", 3),
            ("frozen", 3),
            ("frost", 3),
            ("glacier", 3),
            ("snow", 2),
        ],
    ),
    (
        "sandstone_tomb",
        &[
            ("sandstone", 3),
            ("desert", 3),
            ("pharaoh", 3),
            ("egypt", 3),
            ("sand$", 2),
            ("mumm", 2),
            ("pyramid", 2),
            ("scarab", 2),
            ("dune", 2),
        ],
    ),
    ("spider_den", &[("spider", 3), ("web", 2)]),
    (
        "infernal_halls",
        &[
            ("infernal", 3),
            ("hell", 3),
            ("demon", 3),
            ("lava", 2),
            ("magma", 2),
            ("brimstone", 3),
        ],
    ),
    (
        "sun_temple",
        &[("aztec", 2), ("mayan", 2), ("sun$", 2), ("golden", 1)],
    ),
];

const MOOD_WORDS: &[(&str, Table)] = &[
    ("torchlit", &[("torch", 2), ("smoky", 1)]),
    ("candlelit", &[("candle", 3)]),
    ("prison", &[("prison", 2), ("chain", 1)]),
    ("moonlit", &[("moon", 3), ("night", 2), ("starlight", 2)]),
    (
        "golden",
        &[
            ("golden", 2),
            ("holy", 2),
            ("sacred", 2),
            ("divine", 2),
            ("radiant", 2),
            ("celestial", 2),
        ],
    ),
    ("forge", &[("forge", 2), ("ember", 2), ("smith", 2)]),
    (
        "damp",
        &[
            ("damp", 3),
            ("wet$", 2),
            ("drip", 2),
            ("mold", 2),
            ("mould", 2),
            ("slime", 2),
            ("rot", 1),
        ],
    ),
    (
        "frost",
        &[
            ("frozen", 2),
            ("ice$", 2),
            ("icy", 2),
            ("frost", 2),
            ("snow", 2),
        ],
    ),
    (
        "overgrown",
        &[
            ("overgrown", 3),
            ("moss", 2),
            ("vine", 2),
            ("roots", 2),
            ("druid", 2),
        ],
    ),
    (
        "industrial",
        &[
            ("steam", 2),
            ("machine", 2),
            ("clockwork", 2),
            ("factory", 2),
            ("foundry", 2),
        ],
    ),
    (
        "void",
        &[
            ("void", 3),
            ("eldritch", 3),
            ("cosmic", 2),
            ("aberra", 2),
            ("tentacle", 2),
            ("madness", 2),
            ("otherworld", 2),
            ("alien", 2),
            ("warp", 2),
        ],
    ),
    (
        "infernal",
        &[("demon", 2), ("hell", 2), ("lava", 2), ("infernal", 2)],
    ),
    ("mine", &[("lantern", 2)]),
    (
        "tomb",
        &[("desert", 2), ("pharaoh", 2), ("mumm", 2), ("sand$", 1)],
    ),
    (
        "arcane",
        &[
            ("arcane", 3),
            ("wizard", 3),
            ("mage", 3),
            ("sorcer", 3),
            ("magic", 2),
            ("rune", 2),
            ("crystal", 2),
            ("enchant", 2),
            ("spell", 2),
            ("astral", 2),
        ],
    ),
    (
        "sunken",
        &[
            ("sunken", 3),
            ("flooded", 3),
            ("drowned", 3),
            ("underwater", 3),
            ("submerged", 3),
            ("sea$", 2),
            ("tide", 2),
            ("coral", 2),
            ("ocean", 2),
            ("shipwreck", 2),
            ("kelp", 2),
        ],
    ),
    (
        "haunted",
        &[
            ("haunted", 3),
            ("ghost", 3),
            ("phantom", 3),
            ("spectral", 3),
            ("spirit", 2),
            ("wraith", 2),
            ("banshee", 2),
            ("cursed", 2),
            ("fog", 2),
            ("mist", 2),
            ("whisper", 2),
            ("eerie", 2),
        ],
    ),
];

/// When the words name no kind of place: one of these (by the words, so the same words make
/// the same dungeon).
const FALLBACK: [&str; 5] = [
    "crypt",
    "ruined_fortress",
    "great_caverns",
    "catacombs",
    "cathedral",
];

/// A description's words, lower case, split on anything but letters and digits.
struct Text {
    words: Vec<String>,
    joined: String,
}

impl Text {
    fn new(s: &str) -> Self {
        let words: Vec<String> = s
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        let joined = format!(" {} ", words.join(" "));
        Text { words, joined }
    }

    fn matches(stem: &str, word: &str) -> bool {
        match stem.strip_suffix('$') {
            Some(x) => {
                word == x
                    || word
                        .strip_suffix('s')
                        .is_some_and(|w| w == x || w.strip_suffix('e') == Some(x))
            }
            None => word.starts_with(stem),
        }
    }

    /// The table's score (each word counts once, at its best stem; phrases on top), where its
    /// first match is, and the words matched.
    fn score(&self, t: &[(&str, u32)]) -> (u32, usize, Vec<String>) {
        let (mut total, mut first, mut hit) = (0, usize::MAX, vec![]);
        for (i, w) in self.words.iter().enumerate() {
            if let Some(k) = t
                .iter()
                .filter(|(st, _)| !st.contains(' ') && Self::matches(st, w))
                .map(|(_, k)| *k)
                .max()
            {
                total += k;
                first = first.min(i);
                hit.push(w.clone());
            }
        }
        for (st, k) in t.iter().filter(|(st, _)| st.contains(' ')) {
            if let Some(at) = self.joined.find(&format!(" {st}")) {
                total += k;
                first = first.min(self.joined[..at].matches(' ').count());
                hit.push(st.to_string());
            }
        }
        (total, first, hit)
    }

    fn any(&self, t: &[(&str, u32)]) -> bool {
        self.score(t).0 > 0
    }

    /// The best entry of a table (score, then the earliest in the words).
    fn best<'a>(&self, tables: &'a [(&'a str, Table)]) -> Option<(&'a str, u32, Vec<String>)> {
        tables
            .iter()
            .map(|(id, t)| {
                let (s, f, h) = self.score(t);
                (*id, s, f, h)
            })
            .filter(|x| x.1 > 0)
            .max_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)))
            .map(|(id, s, _, h)| (id, s, h))
    }
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Reads a description into a dungeon (no AI: what the words say, and sensible picks for the
/// rest). `kind` forces underground or a realm.
pub fn read(text: &str, kind: Option<MapKind>) -> Picks {
    let t = Text::new(text);
    let seed = hash(&t.joined);
    let mut used: Vec<String> = vec![];
    // (a temple city, a compound, a ziggurat: an outdoor temple compound)
    let compound = kind != Some(MapKind::Interior) && t.score(COMPOUND_SAID).0 >= 3;
    let realm = compound || kind.map_or_else(|| t.any(REALM_SAID), |k| k == MapKind::Realm);
    let mut p = Picks {
        kind: if realm {
            MapKind::Realm
        } else {
            MapKind::Interior
        },
        layout: String::new(),
        style: String::new(),
        mood: String::new(),
        size: Size::Medium,
        room_size: RoomSize::Normal,
        irregularity: 0.0,
        name: String::new(),
        color: PortalColor::Arcane,
        frame: String::new(),
        instancing: Instancing::Party,
        light: 1.0,
        age: 0.0,
        wet: 1.0,
        moss: 1.0,
        liquid: None,
        words: vec![],
        idea: None,
        realm_shape: String::new(),
    };
    if compound {
        p.layout = super::compound::LAYOUT.into();
        used.extend(t.score(COMPOUND_SAID).2);
    } else if realm {
        let (id, _, hit) = t.best(REALM_WORDS).unwrap_or(("wild_realm", 0, vec![]));
        p.layout = id.into();
        used.extend(hit);
        if let Some((shape, _, hit)) = t.best(REALM_SHAPE_WORDS) {
            p.realm_shape = shape.into();
            used.extend(hit);
        }
    } else {
        // the kind of place: its words, and the first place named (the subject: "a flooded
        // dwarven MINE with a collapsed forge") counts extra
        let head = t.words.iter().find_map(|w| {
            LAYOUT_WORDS
                .iter()
                .find(|(_, tab)| {
                    tab.iter()
                        .any(|(st, k)| *k == 3 && !st.contains(' ') && Text::matches(st, w))
                })
                .map(|(id, _)| *id)
        });
        let best = LAYOUT_WORDS
            .iter()
            .map(|(id, tab)| {
                let (s, f, h) = t.score(tab);
                (*id, s + if head == Some(*id) { 2 } else { 0 }, f, h)
            })
            .filter(|x| x.1 > 0)
            .max_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)));
        match best {
            Some((id, _, _, hit)) => {
                p.layout = id.into();
                used.extend(hit);
            }
            None => p.layout = FALLBACK[(seed % FALLBACK.len() as u64) as usize].into(),
        }
        let preset = presets::get(&p.layout).unwrap_or(&presets::all()[0]);
        p.style = match t.best(STYLE_WORDS) {
            Some((id, s, hit)) if s >= 2 => {
                used.extend(hit);
                id.into()
            }
            _ => preset.style.into(),
        };
        p.mood = match t.best(MOOD_WORDS) {
            Some((id, s, hit)) if s >= 2 => {
                used.extend(hit);
                id.into()
            }
            _ => preset.mood.into(),
        };
    }
    // how big
    let small = t.score(&[
        ("small", 3),
        ("tiny", 3),
        ("little", 2),
        ("short", 2),
        ("compact", 2),
        ("quick", 2),
        ("simple", 1),
        ("few$", 1),
    ]);
    let large = t.score(&[
        ("large", 3),
        ("huge", 3),
        ("vast", 3),
        ("sprawling", 3),
        ("enormous", 3),
        ("massive", 3),
        ("immense", 3),
        ("epic", 2),
        ("endless", 2),
        ("raid", 2),
        ("mega", 3),
        ("deep$", 1),
        ("many", 1),
        ("grand", 1),
    ]);
    p.size = if large.0 > small.0 {
        Size::Large
    } else if small.0 > large.0 {
        Size::Small
    } else {
        Size::Medium
    };
    used.extend(small.2.into_iter().chain(large.2));
    let roomy = t.score(&[
        ("cavernous", 2),
        ("towering", 2),
        ("vast", 1),
        ("huge", 1),
        ("grand", 1),
        ("giant", 1),
        ("soaring", 2),
        ("lofty", 2),
    ]);
    let huge = t.score(&[
        ("colossal", 2),
        ("titanic", 2),
        ("gargantuan", 2),
        ("cyclopean", 2),
    ]);
    let tight = t.score(&[
        ("cramped", 2),
        ("narrow", 2),
        ("tight", 2),
        ("claustrophob", 2),
        ("low$", 1),
    ]);
    p.room_size = if huge.0 >= 2 {
        RoomSize::ExtraLarge
    } else if roomy.0 >= 2 && roomy.0 > tight.0 {
        RoomSize::Large
    } else {
        RoomSize::Normal
    };
    // uneven walls: natural, ruined and dug places
    let natural = matches!(
        p.layout.as_str(),
        "great_caverns" | "goblin_cave" | "spider_den" | "mine"
    );
    let mut irr: f32 = if natural {
        0.4
    } else if p.layout == "ruined_fortress" {
        0.2
    } else {
        0.0
    };
    for (stem, add) in [
        ("collapsed", 0.3),
        ("crumbl", 0.3),
        ("ruin", 0.25),
        ("eroded", 0.25),
        ("organic", 0.25),
        ("natural", 0.2),
        ("rough", 0.2),
        ("twist", 0.2),
        ("winding", 0.15),
        ("cracked", 0.15),
        ("broken", 0.15),
        ("caved", 0.25),
        ("hewn", 0.1),
    ] {
        if t.any(&[(stem, 1)]) {
            irr += add;
        }
    }
    if t.any(&[
        ("symmetric", 1),
        ("ordered", 1),
        ("pristine", 1),
        ("perfect", 1),
        ("geometric", 1),
    ]) {
        irr = 0.0;
    }
    p.irregularity = (irr.min(0.9) * 20.0).round() / 20.0;
    if t.any(&[("raid", 1)]) {
        p.instancing = Instancing::Raid;
    } else if t.any(&[("shared", 1), ("public", 1)]) {
        p.instancing = Instancing::Shared;
    }
    // light, age, damp, moss, liquids
    let dark = t.score(&[
        ("dark$", 2),
        ("darkness", 2),
        ("pitch", 2),
        ("gloom", 2),
        ("shadow", 1),
        ("dim$", 2),
        ("unlit", 2),
        ("lightless", 2),
    ]);
    let bright = t.score(&[
        ("bright", 2),
        ("well lit", 2),
        ("radiant", 2),
        ("glowing", 1),
        ("brilliant", 2),
        ("sunlit", 2),
    ]);
    p.light = if dark.0 > bright.0 {
        0.65
    } else if bright.0 > dark.0 {
        1.4
    } else {
        1.0
    };
    let old = t.score(&[
        ("old$", 1),
        ("ancient", 2),
        ("abandoned", 2),
        ("ruin", 2),
        ("crumbl", 2),
        ("forgotten", 2),
        ("decay", 2),
        ("collapsed", 2),
        ("derelict", 2),
        ("forsaken", 2),
        ("haunted", 1),
        ("lost$", 1),
        ("aged", 1),
        ("rotting", 2),
    ]);
    p.age = if t.any(&[
        ("pristine", 1),
        ("gleaming", 1),
        ("polished", 1),
        ("brand", 1),
    ]) {
        0.0
    } else if old.0 >= 2 {
        1.4
    } else if old.0 == 1 {
        1.0
    } else {
        0.0
    };
    let wet = t.score(&[
        ("flood", 2),
        ("sunken", 2),
        ("drowned", 2),
        ("damp", 2),
        ("wet$", 2),
        ("water", 1),
        ("drip", 1),
        ("submerged", 2),
        ("sea$", 1),
        ("tide", 1),
        ("sewer", 1),
        ("swamp", 2),
        ("marsh", 2),
    ]);
    if wet.0 >= 2 {
        p.wet = 2.0;
    }
    if t.any(&[
        ("overgrown", 1),
        ("moss", 1),
        ("vine", 1),
        ("roots", 1),
        ("jungle", 1),
        ("ivy", 1),
    ]) {
        p.moss = 2.0;
    }
    p.liquid = if t.any(&[("lava", 1), ("magma", 1), ("molten", 1)]) {
        Some(Liquid::Lava)
    } else if t.any(&[("blood", 1), ("vampir", 1), ("sanguine", 1)]) {
        Some(Liquid::Blood)
    } else if t.any(&[
        ("slime", 1),
        ("toxic", 1),
        ("poison", 1),
        ("acid", 1),
        ("ooze", 1),
        ("plague", 1),
    ]) {
        Some(Liquid::Slime)
    } else if t.any(&[("arcane", 1), ("mana", 1), ("ley$", 1)]) {
        Some(Liquid::Arcane)
    } else if wet.0 >= 2 || t.any(&[("pool", 1), ("lake", 1), ("river", 1)]) {
        Some(Liquid::Water)
    } else {
        None
    };
    // the portal: its colour and its frame
    p.color = if t.any(&[("blood", 1), ("crimson", 1), ("vampir", 1), ("scarlet", 1)]) {
        PortalColor::Blood
    } else if t.any(&[
        ("plague", 1),
        ("poison", 1),
        ("toxic", 1),
        ("fel$", 1),
        ("slime", 1),
        ("goblin", 1),
    ]) {
        PortalColor::Fel
    } else {
        match p.mood.as_str() {
            "frost" => PortalColor::Frost,
            "infernal" | "forge" | "industrial" => PortalColor::Fire,
            "void" | "spider" | "haunted" => PortalColor::Void,
            "sunken" | "damp" => PortalColor::Tide,
            "golden" => PortalColor::Holy,
            "overgrown" => PortalColor::Fel,
            _ => match p.layout.as_str() {
                "frozen_realm" => PortalColor::Frost,
                "ember_realm" => PortalColor::Fire,
                "twilight_realm" => PortalColor::Void,
                "desert_realm" => PortalColor::Holy,
                "jungle_realm" => PortalColor::Tide,
                _ => PortalColor::Arcane,
            },
        }
    };
    p.frame = match (p.layout.as_str(), p.style.as_str()) {
        (_, "dwarven_forge") | ("mine" | "clockwork" | "dwarven_halls", _) => "hp:portal_dwarven",
        ("sun_temple" | "cathedral", _) => "hp:portal_gate",
        ("ruined_fortress" | "goblin_cave" | "catacombs" | "prison_keep" | "sewers", _) => {
            "hp:portal_ruin"
        }
        ("frost_halls" | "frozen_realm", _) => "hp:portal_spires",
        ("spider_den" | "infernal" | "twilight_realm" | "ember_realm", _) => "hp:portal_ring",
        _ if p.mood == "arcane" => "hp:portal_spires",
        _ => "",
    }
    .into();
    p.name = name_for(&t, &p, seed);
    used.dedup();
    let mut words = vec![];
    for w in used {
        if !words.contains(&w) {
            words.push(w);
        }
    }
    p.words = words;
    p
}

/// A name from the words: "The Drowned Mine", "The Crimson Crypt".
fn name_for(t: &Text, p: &Picks, seed: u64) -> String {
    const ADJ: &[(&[&str], &[&str])] = &[
        (
            &["flood", "drowned", "sunken", "underwater", "submerged"],
            &["Drowned", "Sunken"],
        ),
        (
            &["frozen", "ice", "icy", "frost", "glacier", "snow"],
            &["Frozen", "Rimefrost", "Frostbound"],
        ),
        (
            &["vampir", "blood", "crimson", "sanguine"],
            &["Crimson", "Sanguine"],
        ),
        (
            &[
                "burning", "fire", "lava", "magma", "infernal", "hell", "demon", "molten", "volcan",
            ],
            &["Burning", "Ashen", "Molten"],
        ),
        (
            &["haunted", "ghost", "spectral", "phantom", "cursed"],
            &["Haunted", "Whispering"],
        ),
        (&["spider", "web"], &["Webbed", "Silken"]),
        (
            &["clockwork", "gear", "machine", "steam", "brass"],
            &["Clockwork", "Brass"],
        ),
        (&["goblin", "kobold"], &["Goblin", "Gristle"]),
        (
            &["collapsed", "ruin", "crumbl", "broken", "shattered"],
            &["Broken", "Fallen", "Shattered"],
        ),
        (
            &["forgotten", "ancient", "lost", "old"],
            &["Forgotten", "Ancient", "Lost"],
        ),
        (&["void", "eldritch", "starless"], &["Starless", "Hollow"]),
        (&["golden", "gold", "sun", "holy"], &["Golden", "Sunlit"]),
        (
            &["overgrown", "moss", "vine", "root", "jungle"],
            &["Overgrown", "Mossbound"],
        ),
        (
            &["dark", "shadow", "black", "gloom"],
            &["Shadowed", "Black"],
        ),
        (&["dwarf", "dwarv"], &["Deep", "Dwarven"]),
    ];
    const NOUN: &[(&str, &str)] = &[
        ("mines", "Mines"),
        ("mine", "Mine"),
        ("crypt", "Crypt"),
        ("tomb", "Tomb"),
        ("warren", "Warren"),
        ("burrow", "Burrow"),
        ("den", "Den"),
        ("lair", "Lair"),
        ("foundry", "Foundry"),
        ("forge", "Forge"),
        ("temple", "Temple"),
        ("cathedral", "Cathedral"),
        ("chapel", "Chapel"),
        ("abbey", "Abbey"),
        ("catacomb", "Catacombs"),
        ("sewer", "Sewers"),
        ("cistern", "Cistern"),
        ("prison", "Prison"),
        ("gaol", "Gaol"),
        ("jail", "Gaol"),
        ("keep", "Keep"),
        ("fortress", "Fortress"),
        ("castle", "Castle"),
        ("citadel", "Citadel"),
        ("halls", "Halls"),
        ("sanctum", "Sanctum"),
        ("vault", "Vault"),
        ("nest", "Nest"),
        ("hive", "Hive"),
        ("cavern", "Caverns"),
        ("cave", "Caves"),
        ("grotto", "Grotto"),
        ("cove", "Cove"),
        ("labyrinth", "Labyrinth"),
        ("maze", "Maze"),
        ("library", "Library"),
        ("workshop", "Works"),
        ("factory", "Works"),
        ("city", "Undercity"),
        ("barrow", "Barrow"),
        ("mausoleum", "Mausoleum"),
        ("abyss", "Abyss"),
        ("depths", "Depths"),
        ("ruins", "Ruins"),
        ("shrine", "Shrine"),
        ("hold", "Hold"),
        ("realm", "Realm"),
    ];
    let pick = |opts: &[&str], k: u64| opts[(k % opts.len() as u64) as usize].to_string();
    let adj = t
        .words
        .iter()
        .find_map(|w| {
            ADJ.iter()
                .find(|(stems, _)| {
                    stems
                        .iter()
                        .any(|s| w.starts_with(s) && (s.len() > 3 || w.len() <= s.len() + 1))
                })
                .map(|(_, a)| pick(a, seed))
        })
        .unwrap_or_else(|| {
            pick(
                &[
                    "Forgotten",
                    "Shadowed",
                    "Hollow",
                    "Silent",
                    "Ashen",
                    "Whispering",
                    "Gloom",
                    "Iron",
                ],
                seed >> 7,
            )
        });
    let noun = t
        .words
        .iter()
        .find_map(|w| {
            NOUN.iter()
                .find(|(stem, _)| {
                    w == stem
                        || w.strip_suffix('s') == Some(stem)
                        || (stem.len() > 4 && w.starts_with(stem))
                })
                .map(|(_, n)| n.to_string())
        })
        .unwrap_or_else(|| {
            let auto = super::edit::auto_name(&p.layout, 0);
            auto.rsplit(' ').next().unwrap_or("Depths").to_string()
        });
    if noun.eq_ignore_ascii_case(&adj) {
        return format!("The {noun}");
    }
    format!("The {adj} {noun}")
}

impl Picks {
    /// "Abandoned Mine · Dwarven Forge · Sunken ruins · medium"
    pub fn summary(&self) -> String {
        if self.kind == MapKind::Realm {
            let r = presets::realm(&self.layout)
                .map(|r| r.name)
                .unwrap_or("Realm");
            let shape = if self.realm_shape.is_empty() {
                String::new()
            } else {
                format!(" · {}", self.realm_shape.replace('_', " "))
            };
            return format!("{r}{shape} · {}", self.size.name().to_lowercase());
        }
        let layout = presets::get(&self.layout)
            .map(|p| p.name)
            .unwrap_or("Dungeon");
        let mut s = format!(
            "{layout} · {} · {} · {}",
            styles::get(&self.style).name,
            mood::name(&self.mood),
            self.size.name().to_lowercase()
        );
        if self.room_size != RoomSize::Normal {
            s += &format!(", {} rooms", self.room_size.name().to_lowercase());
        }
        if self.irregularity > 0.01 {
            s += if self.irregularity < 0.4 {
                ", a little uneven"
            } else {
                ", uneven"
            };
        }
        s
    }

    /// What it was made from, for the AI.
    pub fn made_text(&self) -> String {
        if self.kind == MapKind::Realm {
            let land = if self.realm_shape.is_empty() {
                String::new()
            } else {
                format!(" (its land: {})", self.realm_shape.replace('_', " "))
            };
            return format!(
                "It was made as the realm \"{}\"{land} from the user's words.",
                presets::realm(&self.layout)
                    .map(|r| r.name)
                    .unwrap_or("Realm")
            );
        }
        let p = presets::get(&self.layout);
        let mut s = format!(
            "It was made from the user's words: the \"{}\" layout ({}), the {} style, the {} mood, {} size, {} rooms, {:.0}% irregular.",
            p.map(|p| p.name).unwrap_or("?"),
            p.map(|p| p.blurb).unwrap_or(""),
            styles::get(&self.style).name,
            mood::name(&self.mood),
            self.size.id(),
            self.room_size.name().to_lowercase(),
            self.irregularity * 100.0
        );
        if let Some(l) = self.liquid {
            s += &format!(" Its pools hold {}.", l.name().to_lowercase());
        }
        s += &format!(
            " Its way in, in the world, is a {} portal.",
            self.color.name().to_lowercase()
        );
        if !self.words.is_empty() {
            s += &format!(" Words it went by: {}.", self.words.join(", "));
        }
        s
    }

    pub fn json(&self) -> Value {
        json!({
            "kind": self.kind.id(),
            "layout": self.layout,
            "style": self.style,
            "mood": self.mood,
            "size": self.size.id(),
            "room_size": self.room_size.id(),
            "irregularity": self.irregularity,
            "name": self.name,
            "portal_color": self.color.id(),
            "portal_frame": self.frame,
            "instancing": self.instancing.id(),
            "light_density": self.light,
            "aged": self.age,
            "wet": self.wet,
            "moss": self.moss,
            "liquid": self.liquid.map(|l| l.id()),
            "words": self.words,
            "idea": self.idea,
            "realm_shape": if self.realm_shape.is_empty() { Value::Null } else { json!(self.realm_shape) },
            "summary": self.summary(),
        })
    }

    fn apply(&mut self, o: &Overrides) {
        if !o.name.trim().is_empty() {
            self.name = o.name.trim().to_string();
        }
        if let Some(l) = &o.layout {
            self.layout = l.clone();
            if self.kind == MapKind::Interior
                && let Some(p) = presets::get(l)
            {
                if o.style.is_none() {
                    self.style = p.style.into();
                }
                if o.mood.is_none() {
                    self.mood = p.mood.into();
                }
            }
        }
        if let Some(s) = &o.style {
            self.style = s.clone();
        }
        if let Some(m) = &o.mood {
            self.mood = m.clone();
        }
        if let Some(s) = o.size {
            self.size = s;
        }
        if let Some(r) = o.room_size {
            self.room_size = r;
        }
        if let Some(i) = o.irregularity {
            self.irregularity = i.clamp(0.0, 1.0);
        }
        if let Some(i) = o.instancing {
            self.instancing = i;
        }
        if let Some(c) = o.color {
            self.color = c;
        }
    }
}

/// What the user (More options) or an MCP call set by hand: it wins over the words.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub name: String,
    pub kind: Option<MapKind>,
    pub layout: Option<String>,
    pub style: Option<String>,
    pub mood: Option<String>,
    pub size: Option<Size>,
    pub room_size: Option<RoomSize>,
    pub irregularity: Option<f32>,
    pub instancing: Option<Instancing>,
    pub color: Option<PortalColor>,
}

// ------------------------------------------------------------------ the page's state and the run

/// What a Generate asks for.
pub struct Spec {
    pub text: String,
    /// an idea chip whose words are in the box (its settings are used while they're unchanged)
    pub idea: Option<usize>,
    pub images: Vec<PathBuf>,
    pub opts: Overrides,
    /// let the connected AI shape it
    pub ai: bool,
    /// the way in (default: a portal where the camera looks, or the spot picked)
    pub way_in: Option<NewEntrance>,
    pub spot: Option<(Vec3, f32)>,
    /// from the panel: an unsaved world is saved first (Save As); from MCP that's an error
    pub interactive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// the world is being saved (its dungeons live in its folder)
    Save,
    Open,
    /// laid out, lit, dressed and the words' touches (aged walls, damp, liquids)
    Build,
    /// the AI shapes it
    Shape,
    Done,
    Stopped,
    Failed,
}

impl Stage {
    pub fn active(self) -> bool {
        matches!(
            self,
            Stage::Save | Stage::Open | Stage::Build | Stage::Shape
        )
    }
    fn id(self) -> &'static str {
        match self {
            Stage::Save => "saving_world",
            Stage::Open => "opening",
            Stage::Build => "building",
            Stage::Shape => "shaping",
            Stage::Done => "done",
            Stage::Stopped => "stopped",
            Stage::Failed => "failed",
        }
    }
}

/// A dungeon being made from a description (or the last one made).
pub struct GenRun {
    pub id: u32,
    pub text: String,
    pub images: Vec<PathBuf>,
    pub picks: Picks,
    pub dungeon: u32,
    pub name: String,
    pub stage: Stage,
    /// the AI that shapes it (None: the words alone)
    pub ai: Option<String>,
    /// why there's no AI step
    pub ai_note: String,
    pub status: String,
    pub log: Vec<String>,
    started: Instant,
    pub elapsed: f32,
    stage_at: Instant,
    frames: u32,
    asked: bool,
    touched: bool,
    pub calls: u32,
    pub summary: String,
    pub error: Option<String>,
    /// the card shows it (Close hides a finished run; MCP still reports it)
    pub shown: bool,
    /// the world had to be saved first
    pub saved_first: bool,
}

impl GenRun {
    fn to(&mut self, stage: Stage, status: impl Into<String>) {
        let status = status.into();
        self.log.push(format!(
            "{:>4.0}s  {status}",
            self.started.elapsed().as_secs_f32()
        ));
        self.stage = stage;
        self.status = status;
        self.stage_at = Instant::now();
        self.frames = 0;
        self.asked = false;
    }

    pub fn json(&self) -> Value {
        json!({
            "run": self.id,
            "stage": self.stage.id(),
            "done": !self.stage.active(),
            "dungeon": { "id": self.dungeon, "name": self.name },
            "description": self.text,
            "images": self.images,
            "picks": self.picks.json(),
            "ai": self.ai,
            "ai_note": if self.ai.is_none() { Some(&self.ai_note) } else { None },
            "status": self.status,
            "ai_steps": self.calls,
            "seconds": self.elapsed.round(),
            "summary": self.summary,
            "error": self.error,
            "log": self.log,
        })
    }
}

/// Describe it: the page's inputs and the run (kept in `DungeonTool`).
pub struct Describe {
    /// the create flow shows Describe it (else the step-by-step choices)
    pub page: bool,
    pub text: String,
    pub idea: Option<usize>,
    pub images: Vec<PathBuf>,
    pub opts: Overrides,
    pub ai: bool,
    /// the way in's spot, picked on the ground (else where the camera looks)
    pub spot: Option<(Vec3, f32)>,
    pub picking: bool,
    pub run: Option<GenRun>,
    pub requests: Vec<DescribeRequest>,
    next_run: u32,
    /// what the words give, for the page (the text, idea and options it was read with)
    preview: Option<(String, Option<usize>, String, Picks)>,
}

impl Default for Describe {
    fn default() -> Self {
        Self {
            page: true,
            text: String::new(),
            idea: None,
            images: vec![],
            opts: Overrides::default(),
            ai: true,
            spot: None,
            picking: false,
            run: None,
            requests: vec![],
            next_run: 1,
            preview: None,
        }
    }
}

impl Describe {
    pub fn busy(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.stage.active())
    }
    /// The idea whose words are still in the box.
    fn live_idea(&self) -> Option<usize> {
        self.idea.filter(|i| IDEAS[*i].text == self.text.trim())
    }
}

pub enum DescribeRequest {
    Generate,
    Stop,
    Close,
    SaveAgain,
}

impl DungeonTool {
    /// Describe it is on screen: a picture dropped on the window is a reference for it.
    pub fn describe_takes_images(&self) -> bool {
        self.creating && self.describe.page && self.pick.is_none()
    }
}

/// The picks for a request: an idea's own (while its words are unchanged), else the words', then
/// what was set by hand.
pub fn picks_for(text: &str, idea: Option<usize>, o: &Overrides) -> Result<Picks, String> {
    let text = text.trim();
    let mut p = match idea {
        Some(i)
            if (text.is_empty() || text == IDEAS[i].text)
                && o.kind.is_none_or(|k| k == MapKind::Interior) =>
        {
            IDEAS[i].picks()
        }
        _ => read(text, o.kind),
    };
    p.apply(o);
    // (checked here, so the run never starts with something it can't make)
    match p.kind {
        MapKind::Interior => {
            if presets::get(&p.layout).is_none() {
                return Err(format!(
                    "unknown layout `{}` (dungeons_catalog layouts)",
                    p.layout
                ));
            }
            if styles::all().iter().all(|s| s.id != p.style) {
                return Err(format!(
                    "unknown style `{}` (dungeons_catalog styles)",
                    p.style
                ));
            }
            if styles::get(&p.style).art() != styles::Art::HandPainted {
                return Err(format!(
                    "`{}` is a photoreal style (coming soon): pick a hand-painted one",
                    p.style
                ));
            }
            if mood::preset(&p.mood).is_none() {
                return Err(format!(
                    "unknown mood `{}` (dungeons_catalog moods)",
                    p.mood
                ));
            }
        }
        MapKind::Realm => {
            if presets::realm(&p.layout).is_none() {
                p.layout = read(text, Some(MapKind::Realm)).layout;
            }
            p.style.clear();
            p.mood.clear();
            p.liquid = None;
            if p.realm_shape.is_empty() {
                p.realm_shape = read(text, Some(MapKind::Realm)).realm_shape;
            }
        }
    }
    if p.name.trim().is_empty() {
        p.name = "The Forgotten Depths".into();
    }
    Ok(p)
}

fn saved(world: &World) -> bool {
    world
        .resource::<crate::world::WorldMeta>()
        .path
        .as_deref()
        .is_some_and(|p| p != crate::io::recovery_path().as_path())
}

fn create_sys(In(n): In<NewDungeon>, mut d: WorldData) -> Result<u32, String> {
    let id = super::edit::create(&mut d, &n)?;
    d.dungeons.selected = Some(id);
    Ok(id)
}

fn way_in_arg(In(v): In<Value>, d: WorldData) -> Result<NewEntrance, String> {
    super::mcp::entrance_arg(&v, &d)
}

/// Where the portal goes when none was given: the spot picked, else the middle of the selected
/// area, else where the camera looks — on dry land near it, facing the camera.
fn default_way_in(world: &World, p: &Picks, spot: Option<(Vec3, f32)>) -> NewEntrance {
    let (at, yaw) = spot.unwrap_or_else(|| {
        let orbit = world.resource::<crate::camera::OrbitCamera>();
        let t = world.resource::<crate::terrain::Terrain>();
        let sel = world.resource::<crate::prompt::Selection>();
        let mut c = match sel.polygon() {
            Some(poly) if !poly.is_empty() => {
                poly.iter().copied().sum::<Vec2>() / poly.len() as f32
            }
            _ => Vec2::new(orbit.focus.x, orbit.focus.z),
        };
        let h = t.half() - 8.0;
        c = c.clamp(Vec2::splat(-h), Vec2::splat(h));
        if t.height_at(c.x, c.y) < t.sea_level + 0.5 {
            // (the nearest dry ground, within half a kilometre)
            'search: for ring in 1..60 {
                let r = ring as f32 * 8.0;
                for k in 0..(ring * 6) {
                    let q =
                        c + Vec2::from_angle(std::f32::consts::TAU * k as f32 / (ring * 6) as f32)
                            * r;
                    if q.abs().max_element() < h && t.height_at(q.x, q.y) > t.sea_level + 1.0 {
                        c = q;
                        break 'search;
                    }
                }
            }
        }
        let at = Vec3::new(c.x, t.height_at(c.x, c.y), c.y);
        let to = orbit.eye() - at;
        (at, to.x.atan2(to.z))
    });
    let lib = world.get_resource::<crate::objects::Library>();
    let frame = (!p.frame.is_empty() && lib.is_some_and(|l| l.get(&p.frame).is_some()))
        .then(|| p.frame.clone());
    NewEntrance {
        kind: EntranceKind::Portal,
        at,
        yaw,
        color: p.color,
        frame,
        ..default()
    }
}

/// Starts making a dungeon from a description: creates it in the world now (with its way in);
/// the rest follows frame by frame (`step`). Returns the run's id.
pub fn start(world: &mut World, spec: Spec) -> Result<u32, String> {
    if world.resource::<DungeonMap>().0.is_some() {
        return Err(
            "A dungeon's map is open: dungeons are made in the world (go back to it first)".into(),
        );
    }
    if world.resource::<DungeonTool>().describe.busy() {
        return Err("A dungeon is already being made from a description (dungeons_create_from_prompt_status)".into());
    }
    let text = match spec.idea {
        Some(i) if spec.text.trim().is_empty() => IDEAS[i].text.to_string(),
        _ => spec.text.trim().to_string(),
    };
    if text.is_empty() {
        return Err("Describe the dungeon first (or pick an idea)".into());
    }
    if !spec.interactive && !saved(world) {
        return Err(
            "Save the world first (world_save): its dungeons are kept in its folder".into(),
        );
    }
    if spec.images.len() > MAX_IMAGES {
        return Err(format!("At most {MAX_IMAGES} reference images"));
    }
    for p in &spec.images {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if !crate::prompt::IMAGE_EXTS.contains(&ext.as_str()) {
            return Err(format!("{}: reference images are PNG or JPEG", p.display()));
        }
        if !p.is_file() {
            return Err(format!("{}: no such file", p.display()));
        }
    }
    let picks = picks_for(&text, spec.idea, &spec.opts)?;
    // the AI that shapes it
    let (ai, ai_note) = {
        let s = world.resource::<crate::prompt::PromptSession>();
        match crate::prompt::ai_ready(s) {
            _ if !spec.ai => (None, "Made from the words alone (the AI was not asked).".to_string()),
            Some(_) if s.running() => (None, "Another AI edit is running, so the words alone made it.".to_string()),
            Some(name) => (Some(name.to_string()), String::new()),
            None => (None, "No AI is connected, so it was made from the words it recognised. Connect Codex, Claude Code or an API key in Prompt an area (the sparkle button at the top) for a dungeon that follows your description closely.".to_string()),
        }
    };
    let way_in = spec
        .way_in
        .unwrap_or_else(|| default_way_in(world, &picks, spec.spot));
    let n = NewDungeon {
        name: picks.name.clone(),
        kind: picks.kind,
        layout: picks.layout.clone(),
        size: picks.size,
        room_size: picks.room_size,
        irregularity: picks.irregularity,
        art: styles::Art::HandPainted,
        style: picks.style.clone(),
        mood: picks.mood.clone(),
        instancing: picks.instancing,
        way_in: Some(way_in),
        // a realm's land: the shape its words name (else the realm's own); its size and seed:
        // the defaults
        shape: (picks.kind == MapKind::Realm)
            .then(|| super::realm_shapes::RealmShape::from_id(&picks.realm_shape))
            .flatten(),
        // a temple compound: its theme, courts and order from the words
        compound: (picks.layout == super::compound::LAYOUT).then(|| compound_options(&text)),
        ..Default::default()
    };
    let id = world
        .run_system_cached_with(create_sys, n)
        .map_err(|e| e.to_string())??;
    let name = world
        .resource::<Dungeons>()
        .get(id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| picks.name.clone());
    let save_first = !saved(world);
    let now = Instant::now();
    let mut tool = world.resource_mut::<DungeonTool>();
    let d = &mut tool.describe;
    let run_id = d.next_run;
    d.next_run += 1;
    d.spot = None;
    d.picking = false;
    let mut run = GenRun {
        id: run_id,
        text: text.clone(),
        images: spec.images,
        picks: picks.clone(),
        dungeon: id,
        name: name.clone(),
        stage: Stage::Open,
        ai,
        ai_note,
        status: String::new(),
        log: vec![],
        started: now,
        elapsed: 0.0,
        stage_at: now,
        frames: 0,
        asked: false,
        touched: false,
        calls: 0,
        summary: String::new(),
        error: None,
        shown: true,
        saved_first: save_first,
    };
    run.log
        .push(format!("   0s  Made {name}: {}", picks.summary()));
    if save_first {
        run.to(
            Stage::Save,
            "Save the world first: its dungeons are kept in its folder",
        );
    } else {
        run.to(Stage::Open, "Opening its map…");
    }
    d.run = Some(run);
    // the builder's own choices follow (Layout › Lay out, Shuffle)
    tool.creating = false;
    if picks.kind == MapKind::Interior {
        tool.layout = picks.layout.clone();
        tool.size = picks.size;
        tool.room_size = picks.room_size;
        tool.irregular = picks.irregularity;
    }
    info!(
        "describe: run {run_id} made dungeon {id} \"{name}\" from \"{}\" ({})",
        text.chars().take(80).collect::<String>(),
        picks.summary()
    );
    Ok(run_id)
}

/// The rectangle round the whole dungeon (all levels), for the AI's selection.
fn dungeon_outline(m: &MapDoc) -> Option<Vec<Vec2>> {
    let (lo, hi) = m.plan.bounds2()?;
    let (lo, hi) = (lo - Vec2::splat(4.0), hi + Vec2::splat(4.0));
    Some(vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)])
}

fn select(world: &mut World, poly: &[Vec2]) -> Result<(), String> {
    world.resource_scope(|world, mut sel: Mut<crate::prompt::Selection>| {
        sel.set(world.resource::<crate::terrain::Terrain>(), poly)
    })
}

/// One frame of the run: what each stage waits for and does next.
fn step(world: &mut World) {
    let Some(stage) = world
        .resource::<DungeonTool>()
        .describe
        .run
        .as_ref()
        .map(|r| r.stage)
    else {
        return;
    };
    if !stage.active() {
        // (a finished run's card goes when its dungeon is left)
        let in_world = world.resource::<DungeonMap>().0.is_none();
        let hide = in_world
            && world
                .resource::<DungeonTool>()
                .describe
                .run
                .as_ref()
                .is_some_and(|r| r.shown && r.stage == Stage::Done);
        if hide && let Some(r) = world.resource_mut::<DungeonTool>().describe.run.as_mut() {
            r.shown = false;
        }
        return;
    }
    let open = world
        .resource::<DungeonMap>()
        .0
        .as_ref()
        .map(|m| (m.id, m.kind));
    {
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        r.frames += 1;
        r.elapsed = r.started.elapsed().as_secs_f32();
    }
    let since = world
        .resource::<DungeonTool>()
        .describe
        .run
        .as_ref()
        .map_or(0.0, |r| r.stage_at.elapsed().as_secs_f32());
    match stage {
        Stage::Save => {
            let ok = saved(world);
            let mut t = world.resource_mut::<DungeonTool>();
            let r = t.describe.run.as_mut().expect("run");
            if ok {
                r.to(Stage::Open, "Opening its map…");
            } else if !r.asked {
                r.asked = true;
                world.write_message(crate::ui::UiAction::SaveAs);
            }
        }
        Stage::Open => {
            let mut t = world.resource_mut::<DungeonTool>();
            let id = t.describe.run.as_ref().expect("run").dungeon;
            match open {
                Some((m, _)) if m == id => {
                    let r = t.describe.run.as_mut().expect("run");
                    r.to(
                        Stage::Build,
                        if r.picks.kind == MapKind::Realm {
                            "Making the realm's land…"
                        } else {
                            "Laying it out, lighting and dressing it…"
                        },
                    );
                }
                Some(_) => {
                    let r = t.describe.run.as_mut().expect("run");
                    r.error = Some("Another dungeon's map was opened".into());
                    r.to(Stage::Failed, "Stopped: another dungeon's map was opened");
                }
                None => {
                    let asked = t.describe.run.as_ref().expect("run").asked;
                    if !asked {
                        t.requests.push(Request::Open(id));
                        t.describe.run.as_mut().expect("run").asked = true;
                    } else if since > 20.0 {
                        let r = t.describe.run.as_mut().expect("run");
                        r.error = Some(
                            "Its map didn't open (see the message at the bottom of the window)"
                                .into(),
                        );
                        r.to(Stage::Failed, "Couldn't open its map");
                    }
                }
            }
        }
        Stage::Build => build_step(world, open.map(|o| o.1), since),
        Stage::Shape => shape_step(world),
        _ => {}
    }
}

fn build_step(world: &mut World, kind: Option<MapKind>, since: f32) {
    let Some(kind) = kind else {
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        r.error = Some("Its map was closed".into());
        r.to(Stage::Stopped, "Stopped: its map was closed");
        return;
    };
    let (frames, touched, ai) = {
        let r = world
            .resource::<DungeonTool>()
            .describe
            .run
            .as_ref()
            .expect("run");
        (r.frames, r.touched, r.ai.is_some())
    };
    let ready = match kind {
        // (a temple compound: once its build is done)
        MapKind::Realm => frames >= 90 && !super::compound::running(world),
        MapKind::Interior => {
            let fresh = world.resource::<super::session::Session>().fresh;
            let dw = world.resource::<super::render::DungeonWorld>();
            let idle = !fresh
                && world.resource::<DungeonTool>().requests.is_empty()
                && dw.compiled.is_some()
                && !dw.busy();
            (idle && frames >= 10) || since > 120.0
        }
    };
    if !ready {
        return;
    }
    if kind == MapKind::Interior && !touched {
        // the words' touches: light density, aged walls, damp and moss, what the pools hold
        let mut t = world.resource_mut::<DungeonTool>();
        let p = t.describe.run.as_ref().expect("run").picks.clone();
        if (p.light - 1.0).abs() > 0.05 {
            t.requests.push(Request::AutoLight { density: p.light });
        }
        if p.age > 0.0 {
            t.requests.push(Request::AgeWalls { density: p.age });
        }
        if (p.wet - 1.0).abs() > 0.01 || (p.moss - 1.0).abs() > 0.01 || p.liquid.is_some() {
            let (wet, moss, liquid) = (p.wet, p.moss, p.liquid);
            t.requests.push(Request::Map(
                "Weathering and pools".into(),
                Box::new(move |m| {
                    m.weathering.wet *= wet;
                    m.weathering.puddles *= wet;
                    m.weathering.moss *= moss;
                    if let Some(l) = liquid {
                        for r in m.plan.rooms.iter_mut().filter(|r| {
                            r.kind == RoomKind::Pool
                                || (r.kind == RoomKind::Pit && r.liquid != Liquid::None)
                        }) {
                            r.liquid = l;
                        }
                    }
                }),
            ));
        }
        let r = t.describe.run.as_mut().expect("run");
        r.touched = true;
        r.frames = 0;
        return;
    }
    let name = world
        .resource::<DungeonMap>()
        .0
        .as_ref()
        .map(|m| m.name.clone())
        .unwrap_or_default();
    if ai {
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        let who = r.ai.clone().unwrap_or_default();
        r.name = name;
        r.to(
            Stage::Shape,
            format!("{who} is shaping it from your description…"),
        );
    } else {
        finish(world, None);
    }
}

fn shape_step(world: &mut World) {
    let (asked, frames, kind) = {
        let r = world
            .resource::<DungeonTool>()
            .describe
            .run
            .as_ref()
            .expect("run");
        (r.asked, r.frames, r.picks.kind)
    };
    if !asked {
        world
            .resource_mut::<DungeonTool>()
            .describe
            .run
            .as_mut()
            .expect("run")
            .asked = true;
        let (text, images, made) = {
            let r = world
                .resource::<DungeonTool>()
                .describe
                .run
                .as_ref()
                .expect("run");
            (r.text.clone(), r.images.clone(), r.picks.made_text())
        };
        let started = match kind {
            MapKind::Interior => {
                let poly = world
                    .resource::<DungeonMap>()
                    .0
                    .as_ref()
                    .and_then(dungeon_outline);
                match poly {
                    Some(poly) => select(world, &poly).and_then(|_| {
                        crate::prompt::start_dungeon_shape(world, &text, images, made)
                    }),
                    None => Err("the dungeon has no rooms".into()),
                }
            }
            MapKind::Realm => {
                let h = world.resource::<crate::terrain::Terrain>().half() * 0.995;
                let poly = [
                    Vec2::new(-h, -h),
                    Vec2::new(h, -h),
                    Vec2::new(h, h),
                    Vec2::new(-h, h),
                ];
                let compound = {
                    let r = world.resource::<super::compound::CompoundRun>();
                    let open = world.resource::<DungeonMap>().0.as_ref().map(|m| m.id);
                    r.summary.clone().filter(|_| r.map_id() == open)
                };
                let context = match compound {
                    // a temple compound: its layout stays, the AI names it and adds the touches
                    Some(s) => format!(
                        "Context: this realm (an open-sky dungeon: a world of its own behind a portal) was just made for the request above as a temple compound, laid out by the generator: {}. The selection is the whole realm. Keep the compound's layout — its courts, stairs, trails, lakes, the processional way and the buildings on them stay where they are. Rename the courts, bosses and places after the description (gameplay_update on the boss arenas, creatures and landmarks) and add what the description asks for round them: statues, banners, ruins, plants, light — never on the courts' floors or across the trails.",
                        serde_json::to_string(&json!({ "courts": s["courts"], "final": s["final"], "hub": s["hub"], "lakes": s["lakes"] })).unwrap_or_default()
                    ),
                    None => format!(
                        "Context: this realm (an open-sky dungeon: a world of its own behind a portal) was just made for the request above, the user's description of it. {} The selection is the whole realm: shape all of it to match the description — its landforms, water, ground, vegetation, and any ruins or buildings it names. Players arrive at the player start near the middle.",
                        made
                    ),
                };
                select(world, &poly)
                    .and_then(|_| crate::prompt::start_area_edit(world, &text, images, context))
            }
        };
        if let Err(e) = started {
            warn!("describe: the AI couldn't start: {e}");
            let mut t = world.resource_mut::<DungeonTool>();
            let r = t.describe.run.as_mut().expect("run");
            r.ai_note = format!("The AI couldn't start ({e}), so the words alone made it.");
            r.ai = None;
            finish(world, Some(format!("The AI couldn't start: {e}")));
        }
        return;
    }
    let (running, calls, last, summary, error) = {
        let s = world.resource::<crate::prompt::PromptSession>();
        (
            s.running(),
            s.calls,
            s.feed
                .iter()
                .rev()
                .find(|f| f.tool.is_some())
                .or(s.feed.last())
                .map(|f| f.text.clone()),
            s.summary.clone(),
            s.error.clone(),
        )
    };
    if running {
        // (its map was left meanwhile: the AI has nothing to shape)
        let id = world
            .resource::<DungeonTool>()
            .describe
            .run
            .as_ref()
            .map(|r| r.dungeon);
        if world.resource::<DungeonMap>().0.as_ref().map(|m| m.id) != id {
            world
                .resource_mut::<crate::prompt::PromptSession>()
                .requests
                .push(crate::prompt::Request::Stop);
            return;
        }
        // (the selection keeps round the whole dungeon as it grows)
        if frames % 30 == 0
            && kind == MapKind::Interior
            && let Some(poly) = world
                .resource::<DungeonMap>()
                .0
                .as_ref()
                .and_then(dungeon_outline)
        {
            let inside = world
                .resource::<crate::prompt::Selection>()
                .polygon()
                .map(crate::prompt::selection::bounds)
                .is_some_and(|(lo, hi)| {
                    poly.iter().all(|p| {
                        p.x >= lo.x - 0.5
                            && p.y >= lo.y - 0.5
                            && p.x <= hi.x + 0.5
                            && p.y <= hi.y + 0.5
                    })
                });
            if !inside {
                let _ = select(world, &poly);
            }
        }
        let name = world
            .resource::<DungeonMap>()
            .0
            .as_ref()
            .map(|m| m.name.clone());
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        r.calls = calls;
        if let Some(l) = last {
            r.status = l;
        }
        // (the AI may rename it)
        if let Some(n) = name.filter(|n| *n != r.name) {
            r.name = n;
        }
        return;
    }
    {
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        r.calls = calls;
        r.summary = summary;
    }
    let stopped = error.as_deref() == Some("Stopped");
    if stopped {
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        r.to(
            Stage::Stopped,
            format!("Stopped after {calls} AI steps: what it built stays (⌘Z undoes it)"),
        );
        reframe(world);
        return;
    }
    finish(world, error);
}

/// Ends a run: its name as it is now, the camera over the whole dungeon, a toast.
fn finish(world: &mut World, error: Option<String>) {
    let name = world
        .resource::<DungeonMap>()
        .0
        .as_ref()
        .map(|m| m.name.clone());
    let msg = {
        let mut t = world.resource_mut::<DungeonTool>();
        let r = t.describe.run.as_mut().expect("run");
        if let Some(n) = name {
            r.name = n;
        }
        r.error = error;
        if r.ai.is_some() && r.error.is_none() {
            if r.summary.is_empty() {
                r.summary = format!(
                    "{} shaped it from your description.",
                    r.ai.clone().unwrap_or_default()
                );
            }
        } else if r.summary.is_empty() {
            r.summary = match r.picks.idea.and_then(idea) {
                Some(i) => format!(
                    "Made from the idea \u{201c}{}\u{201d}: {}.",
                    IDEAS[i].chip,
                    r.picks.summary()
                ),
                None => format!("Made from your words: {}.", r.picks.summary()),
            };
        }
        let status = format!("{} is ready", r.name);
        r.to(Stage::Done, status.clone());
        status
    };
    reframe(world);
    world.resource_mut::<crate::editor::Editor>().toast(format!(
        "{msg}: walk in, or refine any part with Prompt an area (G)"
    ));
}

/// The camera over the whole dungeon, looking down into it.
fn reframe(world: &mut World) {
    let Some((mid, y, size)) = world
        .resource::<DungeonMap>()
        .0
        .as_ref()
        .filter(|m| m.kind == MapKind::Interior)
        .and_then(|m| {
            let (lo, hi) = m.plan.bounds2()?;
            let y = m
                .plan
                .levels
                .iter()
                .find(|l| l.id == 0)
                .map_or(0.0, |l| l.y);
            Some(((lo + hi) * 0.5, y, (hi - lo).max_element().max(40.0)))
        })
    else {
        return;
    };
    world.resource_mut::<super::render::DungeonView>().level = 0;
    let mut o = world.resource_mut::<crate::camera::OrbitCamera>();
    let yaw = o.yaw;
    o.set_view(Vec3::new(mid.x, y, mid.y), yaw, 1.05, size * 1.05);
}

fn stop(world: &mut World) {
    let Some((stage, asked)) = world
        .resource::<DungeonTool>()
        .describe
        .run
        .as_ref()
        .map(|r| (r.stage, r.asked))
    else {
        return;
    };
    match stage {
        Stage::Shape if asked => world
            .resource_mut::<crate::prompt::PromptSession>()
            .requests
            .push(crate::prompt::Request::Stop),
        s if s.active() => {
            let mut t = world.resource_mut::<DungeonTool>();
            let r = t.describe.run.as_mut().expect("run");
            r.to(
                Stage::Stopped,
                "Stopped: the dungeon stays as it was made so far",
            );
        }
        _ => {}
    }
}

/// The page's requests, then the run's next step (with the world).
pub(super) fn drive(world: &mut World) {
    let reqs = if world.resource::<DungeonTool>().describe.requests.is_empty() {
        vec![]
    } else {
        std::mem::take(&mut world.resource_mut::<DungeonTool>().describe.requests)
    };
    for r in reqs {
        match r {
            DescribeRequest::Generate => {
                let spec = {
                    let t = world.resource::<DungeonTool>();
                    let d = &t.describe;
                    Spec {
                        text: d.text.clone(),
                        idea: d.live_idea(),
                        images: d.images.clone(),
                        opts: d.opts.clone(),
                        ai: d.ai,
                        way_in: None,
                        spot: d.spot,
                        interactive: true,
                    }
                };
                match start(world, spec) {
                    Ok(_) => {
                        let mut t = world.resource_mut::<DungeonTool>();
                        t.describe.images.clear();
                    }
                    Err(e) => world.resource_mut::<crate::editor::Editor>().toast(e),
                }
            }
            DescribeRequest::Stop => stop(world),
            DescribeRequest::Close => {
                if let Some(r) = world
                    .resource_mut::<DungeonTool>()
                    .describe
                    .run
                    .as_mut()
                    .filter(|r| !r.stage.active())
                {
                    r.shown = false;
                }
            }
            DescribeRequest::SaveAgain => {
                world.write_message(crate::ui::UiAction::SaveAs);
            }
        }
    }
    step(world);
}

/// Pictures dropped on the window while Describe it is open.
pub(super) fn dropped_images(
    mut drops: MessageReader<bevy::window::FileDragAndDrop>,
    mut tool: ResMut<DungeonTool>,
    mut ed: ResMut<crate::editor::Editor>,
    map: Res<DungeonMap>,
) {
    let on =
        tool.describe_takes_images() && ed.tab == crate::editor::Tab::Dungeons && map.0.is_none();
    for d in drops.read() {
        let bevy::window::FileDragAndDrop::DroppedFile { path_buf, .. } = d else {
            continue;
        };
        if !on {
            continue;
        }
        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if !crate::prompt::IMAGE_EXTS.contains(&ext.as_str()) {
            ed.toast("Reference images are PNG or JPEG pictures");
            continue;
        }
        let imgs = &mut tool.describe.images;
        if imgs.len() >= MAX_IMAGES {
            ed.toast(format!("Up to {MAX_IMAGES} reference images"));
        } else if !imgs.contains(path_buf) {
            imgs.push(path_buf.clone());
            ed.toast("Reference image added to the dungeon's description");
        }
    }
}

/// Picking the way in's spot on the ground (and marking it while the page is open).
pub(super) fn pick_spot(
    mut tool: ResMut<DungeonTool>,
    pointer: Res<crate::camera::Pointer>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    map: Res<DungeonMap>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    time: Res<Time<Real>>,
    mut gizmos: Gizmos<super::viewport::DungeonGizmos>,
) {
    if map.0.is_some() || !tool.creating {
        if tool.describe.picking {
            tool.describe.picking = false;
        }
        return;
    }
    if let Some((at, yaw)) = tool.describe.spot.filter(|_| tool.describe.page) {
        let pulse = 1.6 + (time.elapsed_secs() * 3.0).sin() * 0.25;
        let c = Color::srgb(0.35, 0.65, 1.0);
        gizmos.circle(
            Isometry3d::new(
                at + Vec3::Y * 0.15,
                Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            ),
            pulse,
            c,
        );
        gizmos.arrow(
            at + Vec3::Y * 0.2,
            at + Vec3::Y * 0.2 + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * 3.0,
            c,
        );
    }
    if !tool.describe.picking {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) && !pointer.keyboard_captured {
        tool.describe.picking = false;
        return;
    }
    if !buttons.just_pressed(MouseButton::Left) || pointer.over_ui || keys.pressed(KeyCode::Space) {
        return;
    }
    let Some(hit) = pointer.hit else { return };
    let eye = cam
        .single()
        .map(|c| c.translation())
        .unwrap_or(hit + Vec3::Z);
    let to = eye - hit;
    tool.describe.spot = Some((hit, to.x.atan2(to.z)));
    tool.describe.picking = false;
}

/// `WB_DUNGEON_DESCRIBE` / `_IMAGES` / `_WAIT` / `_GO` (see the module doc).
pub(super) fn test_hook(
    mut state: Local<(u32, bool, bool)>,
    mut tool: ResMut<DungeonTool>,
    mut ed: ResMut<crate::editor::Editor>,
    map: Res<DungeonMap>,
) {
    let Ok(text) = std::env::var("WB_DUNGEON_DESCRIBE") else {
        return;
    };
    // (`_WAIT=<file>` / `_GO=<file>`: once that file exists; `_GO=1`: at once)
    let file_says = |k: &str| {
        std::env::var(k)
            .ok()
            .is_none_or(|f| f == "1" || std::path::Path::new(&f).exists())
    };
    state.0 += 1;
    if state.1 {
        if !state.2
            && std::env::var("WB_DUNGEON_DESCRIBE_GO").is_ok()
            && file_says("WB_DUNGEON_DESCRIBE_GO")
        {
            state.2 = true;
            tool.describe.requests.push(DescribeRequest::Generate);
        }
        return;
    }
    if state.0 < 240 || map.0.is_some() || !file_says("WB_DUNGEON_DESCRIBE_WAIT") {
        return;
    }
    state.1 = true;
    ed.tab = crate::editor::Tab::Dungeons;
    let t = &mut *tool;
    t.creating = true;
    t.pick = None;
    t.describe.page = true;
    match text.strip_prefix("idea:").and_then(idea) {
        Some(i) => {
            t.describe.idea = Some(i);
            t.describe.text = IDEAS[i].text.to_string();
        }
        None => t.describe.text = text,
    }
    if let Ok(imgs) = std::env::var("WB_DUNGEON_DESCRIBE_IMAGES") {
        t.describe.images = imgs
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .take(MAX_IMAGES)
            .collect();
    }
}

// ------------------------------------------------------------------ the page

/// Describe it: the text, the ideas, reference images, More options, Generate.
pub fn page(ui: &mut egui::Ui, s: &mut UiState) {
    let ai = crate::prompt::ai_ready(&s.prompt.session).map(str::to_string);
    let busy_ai = s.prompt.session.running();
    let t = &mut *s.dungeon_tool;
    let d = &mut t.describe;
    section(ui, "Describe your dungeon");
    dim(
        ui,
        "Say what kind of place it is, what's in it and how it feels. Or start from an idea below.",
    );
    ui.add_space(4.0);
    let r = ui.add(
        egui::TextEdit::multiline(&mut d.text)
            .hint_text("e.g. A flooded dwarven mine with a collapsed forge, lanterns still burning by the shaft…")
            .font(FontId::proportional(14.0))
            .desired_rows(5)
            .desired_width(f32::INFINITY),
    );
    let go_key =
        r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.add_space(6.0);
    ui.label(RichText::new("Ideas").small().color(TEXT_DIM));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (i, idea) in IDEAS.iter().enumerate() {
            let sel = d.idea == Some(i) && d.text.trim() == idea.text;
            let b = egui::Button::new(RichText::new(idea.chip).size(12.5).color(if sel {
                Color32::WHITE
            } else {
                TEXT
            }))
            .fill(if sel { ACCENT_SOFT } else { CARD_BG })
            .stroke(Stroke::new(1.0, if sel { ACCENT } else { BORDER }))
            .corner_radius(CornerRadius::same(13))
            .min_size(vec2(0.0, 26.0));
            if ui.add(b).on_hover_text(idea.text).clicked() {
                d.idea = Some(i);
                d.text = idea.text.to_string();
            }
        }
    });
    ui.add_space(6.0);
    images_row(ui, d);
    // what the words give
    let text = d.text.trim().to_string();
    let live = d.live_idea();
    if !text.is_empty() {
        let key = format!("{:?}", d.opts);
        let stale = d
            .preview
            .as_ref()
            .is_none_or(|(t0, i0, k0, _)| *t0 != text || *i0 != live || *k0 != key);
        if stale {
            let p = picks_for(&text, live, &d.opts).unwrap_or_else(|_| read(&text, None));
            d.preview = Some((text.clone(), live, key, p));
        }
        if let Some((_, _, _, p)) = &d.preview {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(CARD_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(9))
                .inner_margin(egui::Margin::same(9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let ic = if p.kind == MapKind::Realm {
                        icon::PLANET
                    } else {
                        presets::get(&p.layout).map_or(icon::CASTLE_TURRET, |x| x.icon)
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ic).size(18.0).color(ACCENT));
                        ui.label(
                            RichText::new(&p.name)
                                .font(FontId::new(13.5, semibold()))
                                .color(Color32::WHITE),
                        );
                    });
                    ui.add(egui::Label::new(RichText::new(p.summary()).small().color(TEXT)).wrap());
                    let from = if live.is_some() {
                        "A ready-made idea, tuned for it.".to_string()
                    } else if p.words.is_empty() {
                        "No words it knows yet: a classic dungeon.".to_string()
                    } else {
                        format!(
                            "From: {}",
                            p.words
                                .iter()
                                .take(8)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    ui.label(RichText::new(from).small().color(TEXT_DIM));
                });
        }
    }
    ui.add_space(4.0);
    more_options(ui, d);
    separator(ui);
    // the AI and the way in, in plain words
    let (dot, col, line) = match (&ai, d.ai) {
        (Some(name), true) if !busy_ai => (icon::CHECK_CIRCLE, GREEN, format!("{name} builds it from your description and pictures: rooms, light, dressing and music. It takes a few minutes; you can watch, and stop it any time.")),
        (Some(_), true) => (icon::WARNING_CIRCLE, AMBER, "Another AI edit is running: this one is made from your words alone (or wait for it to finish).".to_string()),
        (Some(_), false) => (icon::INFO, TEXT_DIM, "Made from your words alone (Let the AI shape it is off in More options).".to_string()),
        (None, _) => (icon::INFO, AMBER, "No AI connected: it's made from the words it recognises. Connect Codex, Claude Code or an API key in Prompt an area (the sparkle button at the top) and the AI follows your description closely.".to_string()),
    };
    ui.horizontal_top(|ui| {
        ui.label(RichText::new(dot).color(col));
        ui.add(egui::Label::new(RichText::new(line).small().color(TEXT)).wrap());
    });
    ui.horizontal_top(|ui| {
        ui.label(RichText::new(icon::SPIRAL).color(TEXT_DIM));
        let where_ = if d.picking {
            "Click the ground where its portal goes (Esc cancels)".to_string()
        } else if d.spot.is_some() {
            "Its portal goes at the spot you picked.".to_string()
        } else {
            "Its portal goes where the camera looks (move the view first, or pick a spot in More options).".to_string()
        };
        ui.add(egui::Label::new(RichText::new(where_).small().color(if d.picking { ACCENT } else { TEXT_DIM })).wrap());
    });
    ui.add_space(6.0);
    let can = !text.is_empty() && !d.busy();
    let w = ui.available_width();
    let go = ui
        .add_enabled(
            can,
            egui::Button::new(
                RichText::new(format!("{}  Generate", icon::SPARKLE))
                    .size(15.5)
                    .color(ON_ACCENT),
            )
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(w, 40.0)),
        )
        .on_hover_text("Make the dungeon (⌘↩)")
        .on_disabled_hover_text(if d.busy() {
            "A dungeon is being made: wait for it or stop it"
        } else {
            "Describe it first, or pick an idea"
        });
    if (go.clicked() || go_key) && can {
        d.requests.push(DescribeRequest::Generate);
    }
    ui.add_space(2.0);
    if ui.button(format!("{} Cancel", icon::X)).clicked() {
        t.creating = false;
        t.describe.picking = false;
    }
}

fn images_row(ui: &mut egui::Ui, d: &mut Describe) {
    let ctx = ui.ctx().clone();
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, p) in d.images.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(58.0, 58.0), egui::Sense::click());
            match crate::prompt::image_thumb(&ctx, p) {
                Some(tex) => {
                    egui::Image::new((tex, r.size())).corner_radius(CornerRadius::same(6)).paint_at(ui, r);
                }
                None => {
                    ui.painter().rect_filled(r, CornerRadius::same(6), CARD_BG);
                }
            }
            let x = r.right_top() + vec2(-9.0, 9.0);
            ui.painter().circle_filled(x, 8.0, Color32::from_black_alpha(190));
            ui.painter().text(x, egui::Align2::CENTER_CENTER, icon::X, FontId::proportional(10.0), Color32::WHITE);
            if resp.on_hover_text(format!("{}\nClick to remove", p.display())).clicked() {
                remove = Some(i);
            }
        }
        if d.images.len() < MAX_IMAGES {
            let label = if d.images.is_empty() { format!("{}  Add a reference image", icon::IMAGE) } else { format!("{}  Add", icon::IMAGE) };
            if ui
                .add(egui::Button::new(RichText::new(label).color(TEXT)).corner_radius(CornerRadius::same(8)).min_size(vec2(0.0, 28.0)))
                .on_hover_text("A floor plan to follow, or pictures of the place (its rooms, stone, light and colours). You can also drop a picture on the window.")
                .clicked()
            {
                crate::dialogs::pick_files("dungeon_describe_reference", rfd::AsyncFileDialog::new().add_filter("Images", &crate::prompt::IMAGE_EXTS).set_title("Reference image"));
            }
        }
    });
    if d.images.is_empty() {
        dim(
            ui,
            "Optional: a floor plan or pictures of the place. Drop one on the window, too.",
        );
    }
    if let Some(files) = crate::dialogs::take_files("dungeon_describe_reference") {
        for f in files {
            if !d.images.contains(&f) && d.images.len() < MAX_IMAGES {
                d.images.push(f);
            }
        }
    }
    if let Some(i) = remove {
        d.images.remove(i);
    }
}

/// The expert choices, folded away: each "Auto" follows the words.
fn more_options(ui: &mut egui::Ui, d: &mut Describe) {
    egui::CollapsingHeader::new(RichText::new("More options").color(TEXT)).id_salt("describe_more").show(ui, |ui| {
        let o = &mut d.opts;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Name").color(TEXT));
            ui.add(egui::TextEdit::singleline(&mut o.name).hint_text("made from your words").desired_width(ui.available_width()));
        });
        ui.add_space(2.0);
        ui.label(RichText::new("Kind").small().color(TEXT_DIM));
        segmented(ui, &mut o.kind, &[(None, "Auto"), (Some(MapKind::Interior), "Underground"), (Some(MapKind::Realm), "Realm")]);
        ui.label(RichText::new("Size").small().color(TEXT_DIM));
        segmented(ui, &mut o.size, &[(None, "Auto"), (Some(Size::Small), "Small"), (Some(Size::Medium), "Medium"), (Some(Size::Large), "Large")]);
        ui.label(RichText::new("Rooms").small().color(TEXT_DIM));
        segmented(ui, &mut o.room_size, &[(None, "Auto"), (Some(RoomSize::Normal), "Normal"), (Some(RoomSize::Large), "Large"), (Some(RoomSize::ExtraLarge), "Extra large")]);
        let mut auto = o.irregularity.is_none();
        ui.horizontal(|ui| {
            if toggle(ui, &mut auto, RichText::new("Uneven walls from the words").color(TEXT)).changed() {
                o.irregularity = if auto { None } else { Some(0.3) };
            }
        });
        if let Some(v) = o.irregularity.as_mut() {
            slider_row(ui, "Irregular rooms", v, 0.0..=1.0, |x| if x < 0.01 { "Off".into() } else { format!("{:.0}%", x * 100.0) });
        }
        ui.label(RichText::new("How the game runs it").small().color(TEXT_DIM));
        segmented(ui, &mut o.instancing, &[(None, "Auto"), (Some(Instancing::Shared), "Shared"), (Some(Instancing::Party), "Party"), (Some(Instancing::Raid), "Raid")]);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Portal").small().color(TEXT_DIM));
            if ui.selectable_label(o.color.is_none(), "Auto").clicked() {
                o.color = None;
            }
            for pc in PortalColor::ALL {
                let (core, rim) = pc.colors();
                let col = Color32::from_rgb(((core[0] * 0.5 + rim[0] * 0.5) * 255.0) as u8, ((core[1] * 0.5 + rim[1] * 0.5) * 255.0) as u8, ((core[2] * 0.5 + rim[2] * 0.5) * 255.0) as u8);
                let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
                ui.painter().circle_filled(r.center(), 8.0, col);
                if o.color == Some(pc) {
                    ui.painter().circle_stroke(r.center(), 10.0, Stroke::new(2.0, Color32::WHITE));
                }
                if resp.on_hover_text(pc.name()).clicked() {
                    o.color = Some(pc);
                }
            }
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if ui.button(format!("{} {}", icon::MAP_PIN, if d.spot.is_some() { "Pick again" } else { "Pick the spot on the ground" })).on_hover_text("Click the ground where the dungeon's portal stands (else it goes where the camera looks)").clicked() {
                d.picking = true;
            }
            if d.spot.is_some() && ui.button(format!("{} Where the camera looks", icon::X)).clicked() {
                d.spot = None;
            }
        });
        toggle(ui, &mut d.ai, RichText::new("Let the AI shape it (when one is connected)").color(TEXT)).on_hover_text("Off: the dungeon is made from the words alone, in seconds");
    });
}

/// The run's card at the top of the Dungeons tab (in the world and in its map).
pub fn progress_card(ui: &mut egui::Ui, s: &mut UiState) {
    let t = &mut *s.dungeon_tool;
    let Some(r) = t.describe.run.as_ref().filter(|r| r.shown) else {
        return;
    };
    let active = r.stage.active();
    let mut req = None;
    egui::Frame::new()
        .fill(SOFT)
        .stroke(Stroke::new(1.0, if r.stage == Stage::Failed { AMBER } else { LINE_STRONG }))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if active {
                    ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
                } else {
                    let (ic, col) = match r.stage {
                        Stage::Done => (icon::CHECK_CIRCLE, GREEN),
                        _ => (icon::WARNING_CIRCLE, AMBER),
                    };
                    ui.label(RichText::new(ic).size(17.0).color(col));
                }
                ui.label(RichText::new(&r.name).font(FontId::new(14.5, semibold())).color(Color32::WHITE));
            });
            ui.add_space(2.0);
            // the steps
            let realm = r.picks.kind == MapKind::Realm;
            let made = match r.picks.idea.and_then(idea) {
                Some(i) => format!("Made from the idea \u{201c}{}\u{201d}: {}", IDEAS[i].chip, r.picks.summary()),
                None => format!("Made from your words: {}", r.picks.summary()),
            };
            let mut steps: Vec<(String, Stage)> = vec![(made, Stage::Open)];
            if r.saved_first {
                steps.push(("The world saved (its dungeons live in its folder)".into(), Stage::Save));
            }
            steps.push(("Its map opened".into(), Stage::Open));
            steps.push((if realm { "The realm's land made".into() } else { "Laid out, lit and dressed".into() }, Stage::Build));
            if let Some(ai) = &r.ai {
                steps.push((format!("{ai} shapes it from your description"), Stage::Shape));
            }
            let order = |st: Stage| match st {
                Stage::Save => 0,
                Stage::Open => 1,
                Stage::Build => 2,
                Stage::Shape => 3,
                _ => 9,
            };
            let now = order(r.stage);
            for (i, (label, st)) in steps.iter().enumerate() {
                let n = if i == 0 { -1 } else { order(*st) };
                let (ic, col) = if !active && r.stage == Stage::Done || n < now {
                    (icon::CHECK, GREEN)
                } else if n == now && active {
                    (icon::CIRCLE_NOTCH, ACCENT)
                } else {
                    (icon::CIRCLE, TEXT_DIM)
                };
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(ic).color(col));
                    ui.add(egui::Label::new(RichText::new(label).small().color(if n <= now || !active { TEXT } else { TEXT_DIM })).wrap());
                });
            }
            ui.add_space(3.0);
            if active {
                ui.add(egui::Label::new(RichText::new(&r.status).italics().small().color(TEXT_DIM)).wrap());
                let steps = if r.stage == Stage::Shape && r.calls > 0 { format!("  ·  {} AI step{}", r.calls, if r.calls == 1 { "" } else { "s" }) } else { String::new() };
                ui.label(RichText::new(format!("{}:{:02}{steps}", (r.elapsed / 60.0) as u32, (r.elapsed % 60.0) as u32)).small().color(TEXT_DIM));
                ui.horizontal(|ui| {
                    if r.stage == Stage::Save && ui.button(format!("{} Save the world…", icon::FLOPPY_DISK)).clicked() {
                        req = Some(DescribeRequest::SaveAgain);
                    }
                    if ui.add(egui::Button::new(RichText::new(format!("{}  Stop", icon::STOP)).color(ON_ACCENT)).fill(DANGER).corner_radius(CornerRadius::same(8))).on_hover_text("Stop: what's made so far stays").clicked() {
                        req = Some(DescribeRequest::Stop);
                    }
                });
            } else {
                if let Some(e) = &r.error {
                    ui.add(egui::Label::new(RichText::new(format!("{}  {e}", icon::WARNING_CIRCLE)).small().color(AMBER)).wrap());
                }
                if r.stage == Stage::Done && r.ai.is_some() && !r.summary.is_empty() {
                    // (what the AI made, in its words)
                    ui.add(egui::Label::new(RichText::new(&r.summary).color(TEXT)).wrap());
                } else if r.stage != Stage::Done {
                    ui.add(egui::Label::new(RichText::new(&r.status).small().color(TEXT)).wrap());
                }
                if r.ai.is_none() && !r.ai_note.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&r.ai_note).small().color(TEXT_DIM)).wrap());
                }
                if r.stage == Stage::Done {
                    ui.add(egui::Label::new(RichText::new(format!("{}  Change any part: press Prompt an area (G), draw round it and say what to change.", icon::SPARKLE)).small().color(TEXT_DIM)).wrap());
                }
                ui.horizontal(|ui| {
                    if r.stage == Stage::Done && s.dungeon_map.0.as_ref().is_some_and(|m| m.id == r.dungeon) && ui.button(format!("{} Walk in", icon::PERSON_SIMPLE_WALK)).clicked() {
                        t.requests.push(Request::WalkIn);
                    }
                    if ui.button(format!("{} Close", icon::X)).clicked() {
                        req = Some(DescribeRequest::Close);
                    }
                });
            }
        });
    if let Some(q) = req {
        t.describe.requests.push(q);
    }
    ui.add_space(8.0);
    if active {
        ui.ctx().request_repaint();
    }
}

// ------------------------------------------------------------------ MCP

fn parse<T>(a: &Value, key: &str, from: fn(&str) -> Option<T>) -> Result<Option<T>, String> {
    match a.get(key).and_then(|v| v.as_str()) {
        None => Ok(None),
        Some(s) => from(s)
            .map(Some)
            .ok_or_else(|| format!("unknown {key} `{s}`")),
    }
}

fn mcp_create(world: &mut World, a: Value) -> Result<Vec<McpContent>, String> {
    let text: String = opt_arg(&a, "prompt")?.unwrap_or_default();
    let idea_i = match opt_arg::<String>(&a, "idea")? {
        Some(id) => Some(idea(&id).ok_or_else(|| {
            format!(
                "unknown idea `{id}` ({})",
                IDEAS.iter().map(|i| i.id).collect::<Vec<_>>().join(", ")
            )
        })?),
        None => None,
    };
    if text.trim().is_empty() && idea_i.is_none() {
        return Err("give a `prompt` (the dungeon in plain words) or an `idea`".into());
    }
    let images: Vec<PathBuf> = opt_arg::<Vec<String>>(&a, "images")?
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if images.iter().any(|p| !p.is_absolute()) {
        return Err("`images` are absolute paths".into());
    }
    let opts = Overrides {
        name: opt_arg(&a, "name")?.unwrap_or_default(),
        kind: parse(&a, "kind", MapKind::from_id)?,
        layout: opt_arg(&a, "layout")?,
        style: opt_arg(&a, "style")?,
        mood: opt_arg(&a, "mood")?,
        size: parse(&a, "size", Size::from_id)?,
        room_size: parse(&a, "room_size", RoomSize::from_id)?,
        irregularity: opt_arg(&a, "irregularity")?,
        instancing: parse(&a, "instancing", |s| {
            Instancing::ALL.into_iter().find(|i| i.id() == s)
        })?,
        color: parse(&a, "color", PortalColor::from_id)?,
    };
    if opt_arg::<bool>(&a, "dry_run")?.unwrap_or(false) {
        let p = picks_for(
            if text.trim().is_empty() {
                idea_i.map_or("", |i| IDEAS[i].text)
            } else {
                &text
            },
            idea_i,
            &opts,
        )?;
        return Ok(vec![McpContent::Json(
            json!({ "picks": p.json(), "for_the_ai": p.made_text(), "note": "dry run: nothing was made" }),
        )]);
    }
    let way_in = match a.get("way_in") {
        Some(v) if v.is_object() => Some(
            world
                .run_system_cached_with(way_in_arg, v.clone())
                .map_err(|e| e.to_string())??,
        ),
        _ => None,
    };
    let spec = Spec {
        text,
        idea: idea_i,
        images,
        opts,
        ai: opt_arg::<bool>(&a, "ai")?.unwrap_or(true),
        way_in,
        spot: None,
        interactive: false,
    };
    let id = start(world, spec)?;
    let run = world
        .resource::<DungeonTool>()
        .describe
        .run
        .as_ref()
        .map(GenRun::json)
        .unwrap_or(Value::Null);
    Ok(vec![McpContent::Json(json!({
        "run": id,
        "started": run,
        "note": "It is made in the background: its map opens, is laid out, lit and dressed, then (when an AI is connected) the AI shapes it from the description. Poll dungeons_create_from_prompt_status until done.",
    }))])
}

pub(super) fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "dungeons_create_from_prompt",
        description: "Create a whole dungeon from a description (and optional reference images), like Dungeons › Create a dungeon › Describe it. The words pick the layout or realm, style, mood, size, room size, irregularity, a name and the portal (`dry_run` shows what they give); any of those can be set instead. \
It is created in the world with a portal where the camera looks (or `way_in`, as in dungeons_create), its map is opened (the world must be saved: world_save), laid out, lit and dressed; then, when an AI is connected (Prompt an area's backend: Codex, Claude Code or an API key) and `ai` isn't false, the AI shapes the whole dungeon from the description with the dungeon tools (rooms, lights and colours, dressing, music, the way out) — that takes minutes. \
Returns at once with a run id; poll dungeons_create_from_prompt_status. When done the dungeon's map is open in the builder.",
        input_schema: || {
            let mut way_in = serde_json::Map::new();
            for (k, v) in super::mcp::entrance_schema() {
                way_in.insert(k.into(), v);
            }
            json!({ "type": "object", "properties": {
                "prompt": { "type": "string", "description": "the dungeon in plain words: what kind of place, what's in it, the light and mood" },
                "idea": { "type": "string", "enum": IDEAS.iter().map(|i| i.id).collect::<Vec<_>>(), "description": "a ready-made description with settings tuned for it (used when `prompt` is empty or the same words)" },
                "images": { "type": "array", "items": { "type": "string" }, "maxItems": MAX_IMAGES, "description": "absolute paths of reference images (PNG or JPEG): a floor plan to follow, or pictures of the place" },
                "name": { "type": "string" },
                "kind": { "type": "string", "enum": MapKind::ALL.map(|k| k.id()), "description": "underground (interior) or a realm; default: from the words" },
                "layout": { "type": "string", "description": "a layout id (interior) or realm id (dungeons_catalog); default: from the words" },
                "style": { "type": "string", "description": "a hand-painted style id; default: from the words" },
                "mood": { "type": "string", "description": "a mood preset id; default: from the words" },
                "size": { "type": "string", "enum": Size::ALL.map(|s| s.id()) },
                "room_size": { "type": "string", "enum": RoomSize::ALL.map(|s| s.id()) },
                "irregularity": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                "instancing": { "type": "string", "enum": Instancing::ALL.map(|i| i.id()) },
                "color": { "type": "string", "enum": PortalColor::ALL.map(|c| c.id()), "description": "the portal's colour" },
                "way_in": { "type": "object", "properties": way_in, "description": "its way in (as dungeons_create's); default: a portal where the camera looks, facing the camera" },
                "ai": { "type": "boolean", "default": true, "description": "let the connected AI shape it from the description (false: the words alone, in seconds)" },
                "dry_run": { "type": "boolean", "default": false, "description": "only say what the words give; nothing is made" }
            } })
        },
        run: mcp_create,
    });
    app.mcp_tool(McpTool {
        name: "dungeons_create_from_prompt_status",
        description: "How the dungeon being made from a description is going (or the last one made): its stage (saving_world, opening, building, shaping, done, stopped, failed), what the words gave, the AI's latest step and step count, the AI's summary when done, and a log. `stop` true stops it (what's made so far stays).",
        input_schema: || json!({ "type": "object", "properties": { "run": { "type": "integer" }, "stop": { "type": "boolean", "default": false } } }),
        run: |w, a| {
            let want: Option<u32> = opt_arg(&a, "run")?;
            let have = w.resource::<DungeonTool>().describe.run.as_ref().map(|r| r.id);
            if have.is_none() {
                return Ok(vec![McpContent::Json(json!({ "running": false, "note": "no dungeon has been made from a description yet" }))]);
            }
            if want.is_some_and(|x| Some(x) != have) {
                return Err(format!("run {} is not the latest (that is {})", want.unwrap_or(0), have.unwrap_or(0)));
            }
            if opt_arg::<bool>(&a, "stop")?.unwrap_or(false) {
                stop(w);
            }
            let v = w.resource::<DungeonTool>().describe.run.as_ref().map(GenRun::json).unwrap_or(Value::Null);
            Ok(vec![McpContent::Json(v)])
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_pick_the_place() {
        let p = read("A flooded dwarven mine with a collapsed forge", None);
        assert_eq!(p.layout, "mine");
        assert_eq!(p.style, "dwarven_forge");
        assert_eq!(p.mood, "sunken");
        assert_eq!(p.liquid, Some(Liquid::Water));
        assert!(p.irregularity > 0.3);
        assert!(
            p.name == "The Drowned Mine" || p.name == "The Sunken Mine",
            "{}",
            p.name
        );
        let p = read("a frozen temple around an ice dragon's lair", None);
        assert_eq!(p.layout, "frost_halls");
        assert_eq!(p.color, PortalColor::Frost);
        let p = read("A goblin warren inside a hollow hill", None);
        assert_eq!(p.layout, "goblin_cave");
        assert!(p.name.ends_with("Warren"));
        let p = read("a huge clockwork foundry", None);
        assert_eq!((p.layout.as_str(), p.size), ("clockwork", Size::Large));
        let p = read("a burning realm of lava and ash", None);
        assert_eq!((p.kind, p.layout.as_str()), (MapKind::Realm, "ember_realm"));
        // nothing it knows: still a dungeon, the same one for the same words
        let a = read("something nice", None);
        assert_eq!(a, read("something nice", None));
        assert!(presets::get(&a.layout).is_some());
    }

    #[test]
    fn ideas_and_overrides() {
        for (i, idea) in IDEAS.iter().enumerate() {
            let p = picks_for(idea.text, Some(i), &Overrides::default()).expect(idea.id);
            assert_eq!(p.name, idea.name);
            assert!(
                presets::get(&p.layout).is_some()
                    && styles::all().iter().any(|s| s.id == p.style)
                    && mood::preset(&p.mood).is_some(),
                "{}",
                idea.id
            );
        }
        // changed words: read again
        let p = picks_for(
            "a small spider nest",
            Some(6),
            &Overrides {
                size: Some(Size::Large),
                ..default()
            },
        )
        .unwrap();
        assert_eq!(
            (p.layout.as_str(), p.size, p.idea),
            ("spider_den", Size::Large, None)
        );
        assert!(
            picks_for(
                "a crypt",
                None,
                &Overrides {
                    style: Some("nope".into()),
                    ..default()
                }
            )
            .is_err()
        );
    }
}
