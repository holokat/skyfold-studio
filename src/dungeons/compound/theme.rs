//! A temple compound's themes: what each looks like (biome, sky, water, ground materials), what
//! it is built from (the hand-painted pack's floors, temples, walls, pillars, braziers, ruins and
//! camp props for each role), who lives there and what its places are called.

use serde::{Deserialize, Serialize};

use crate::biomegen::Climate;
use crate::biomes::BiomePreset;
use crate::look::LookPreset;
use crate::noise::Rng;
use crate::terrain::layers::Layer;

/// The look of a temple compound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// a jungle temple city of courts, pyramids and lagoons (the troll-city feel)
    #[default]
    Jungle,
    /// sandstone tombs and obelisks in a walled desert valley
    Desert,
    /// a snowbound citadel of keeps and towers round a frozen lake
    Frozen,
    /// a forge-temple of furnaces and braziers in a burning valley
    Ember,
    /// moonlit elven ruins: shrines, spires and standing stones at dusk
    Twilight,
}

impl Theme {
    pub const ALL: [Theme; 5] = [
        Theme::Jungle,
        Theme::Desert,
        Theme::Frozen,
        Theme::Ember,
        Theme::Twilight,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Theme::Jungle => "jungle",
            Theme::Desert => "desert",
            Theme::Frozen => "frozen",
            Theme::Ember => "ember",
            Theme::Twilight => "twilight",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Theme::Jungle => "Jungle temple city",
            Theme::Desert => "Desert necropolis",
            Theme::Frozen => "Frozen citadel",
            Theme::Ember => "Ember forge-temple",
            Theme::Twilight => "Twilight elven ruins",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Theme::Jungle => "Jungle",
            Theme::Desert => "Desert",
            Theme::Frozen => "Frozen",
            Theme::Ember => "Ember",
            Theme::Twilight => "Twilight",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Theme::Jungle => {
                "Raised courts and a stepped pyramid in steaming jungle, lagoons between them, a temple on an island"
            }
            Theme::Desert => {
                "Sandstone tombs, obelisks and giant bones in a walled desert valley, an oasis between them"
            }
            Theme::Frozen => "A snowbound citadel: keeps, towers and courts round a frozen lake",
            Theme::Ember => {
                "Forges, furnaces and fire-lit courts in a burning valley under a red sky"
            }
            Theme::Twilight => {
                "Moonlit elven ruins: shrines, spires and standing stones in an endless dusk"
            }
        }
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as icon;
        match self {
            Theme::Jungle => icon::TREE_PALM,
            Theme::Desert => icon::SUN_HORIZON,
            Theme::Frozen => icon::SNOWFLAKE,
            Theme::Ember => icon::FIRE,
            Theme::Twilight => icon::MOON_STARS,
        }
    }
    /// By id or name, or a plain word ("troll", "necropolis", "ice"…).
    pub fn from_id(s: &str) -> Option<Theme> {
        let k = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        if let Some(t) = Self::ALL.into_iter().find(|t| {
            t.id() == k
                || t.name().to_ascii_lowercase().replace([' ', '-'], "_") == k
                || t.short().to_ascii_lowercase() == k
        }) {
            return Some(t);
        }
        Some(match k.as_str() {
            "temple_city" | "troll" | "tropical" | "rainforest" => Theme::Jungle,
            "necropolis" | "sand" | "tomb" | "tombs" => Theme::Desert,
            "ice" | "snow" | "citadel" | "frost" | "cold" => Theme::Frozen,
            "fire" | "forge" | "lava" | "volcanic" | "forge_temple" => Theme::Ember,
            "elven" | "elf" | "dusk" | "moon" | "ruins" => Theme::Twilight,
            _ => return None,
        })
    }
    /// The realm preset whose biomes, sky and music it takes (`presets::realms`).
    pub fn realm(self) -> &'static str {
        match self {
            Theme::Jungle => "jungle_realm",
            Theme::Desert => "desert_realm",
            Theme::Frozen => "frozen_realm",
            Theme::Ember => "ember_realm",
            Theme::Twilight => "twilight_realm",
        }
    }
    /// (climate, the one biome everywhere, look, hour)
    pub fn sky(self) -> (Climate, Option<BiomePreset>, LookPreset, f32) {
        match self {
            Theme::Jungle => (
                Climate::Tropical,
                Some(BiomePreset::Jungle),
                LookPreset::EpicSunbeams,
                10.5,
            ),
            Theme::Desert => (
                Climate::Arid,
                Some(BiomePreset::Desert),
                LookPreset::GoldenHour,
                16.0,
            ),
            Theme::Frozen => (
                Climate::Cold,
                Some(BiomePreset::Tundra),
                LookPreset::Nordic,
                11.5,
            ),
            Theme::Ember => (
                Climate::Arid,
                Some(BiomePreset::Volcanic),
                LookPreset::Hellfire,
                16.6,
            ),
            Theme::Twilight => (
                Climate::Temperate,
                Some(BiomePreset::AutumnWoods),
                LookPreset::Lavender,
                16.6,
            ),
        }
    }
    /// The water of its lakes (`water::lakes::SHAPES`).
    pub fn lake(self) -> &'static str {
        match self {
            Theme::Jungle => "tropical_lagoon",
            Theme::Desert => "oasis",
            Theme::Frozen => "frozen_lake",
            Theme::Ember => "volcanic_lake",
            Theme::Twilight => "mirror_lake",
        }
    }
    /// The colour of its portals.
    pub fn portal(self) -> super::super::model::PortalColor {
        use super::super::model::PortalColor as P;
        match self {
            Theme::Jungle => P::Tide,
            Theme::Desert => P::Holy,
            Theme::Frozen => P::Frost,
            Theme::Ember => P::Fire,
            Theme::Twilight => P::Void,
        }
    }
    /// The ground materials its paving layers take (layer, terrain-library material): the
    /// courts' floors, the plaza and the processional way.
    pub fn layer_choices(self) -> &'static [(Layer, &'static str)] {
        match self {
            Theme::Jungle => &[
                (Layer::Cobblestone, "mossy_cobbles"),
                (Layer::Ice, "grassy_flagstones"),
            ],
            Theme::Desert => &[
                (Layer::Cobblestone, "cracked_sandstone"),
                (Layer::Ice, "flagstone_path"),
            ],
            Theme::Frozen => &[(Layer::Cobblestone, "slate_flagstones")],
            Theme::Ember => &[
                (Layer::Cobblestone, "slate_flagstones"),
                (Layer::Ice, "red_granite"),
            ],
            Theme::Twilight => &[
                (Layer::Cobblestone, "flagstone_path"),
                (Layer::Ice, "veined_marble_rock"),
            ],
        }
    }
    /// Each court's own floor colour, in turn (the ground round its floor and up its stairs).
    pub fn court_floors(self) -> [Layer; 6] {
        match self {
            Theme::Frozen => [
                Layer::Cobblestone,
                Layer::Gravel,
                Layer::Rock,
                Layer::Ice,
                Layer::DarkRock,
                Layer::Snow,
            ],
            Theme::Desert => [
                Layer::Cobblestone,
                Layer::Ice,
                Layer::Gravel,
                Layer::Rock,
                Layer::DarkRock,
                Layer::Dirt,
            ],
            _ => [
                Layer::Cobblestone,
                Layer::Ice,
                Layer::Gravel,
                Layer::Rock,
                Layer::DarkRock,
                Layer::Dirt,
            ],
        }
    }
    /// The roads its trails are made as: (the way in, the trails out to the courts): flagstones or
    /// dirt by theme.
    pub fn trail_kinds(self) -> (&'static str, &'static str) {
        match self {
            Theme::Jungle => ("cobblestone", "dirt_road"),
            Theme::Desert => ("paved", "paved"),
            Theme::Frozen => ("cobblestone", "cobblestone"),
            Theme::Ember => ("gravel", "dirt_road"),
            Theme::Twilight => ("paved", "cobblestone"),
        }
    }
    /// The paving of the plaza, the axis and the final terrace.
    pub fn paving(self) -> Layer {
        Layer::Cobblestone
    }
    /// What the compound is built from.
    pub fn kit(self) -> &'static Kit {
        match self {
            Theme::Jungle => &JUNGLE,
            Theme::Desert => &DESERT,
            Theme::Frozen => &FROZEN,
            Theme::Ember => &EMBER,
            Theme::Twilight => &TWILIGHT,
        }
    }
}

/// How the courts are visited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// trails from a central hub out to every court, in any order; the gate to the final court
    /// opens once they have all fallen
    #[default]
    Hub,
    /// one court after another along a single winding trail, the final court last
    Linear,
}

impl Order {
    pub const ALL: [Order; 2] = [Order::Hub, Order::Linear];
    pub fn id(self) -> &'static str {
        match self {
            Order::Hub => "hub",
            Order::Linear => "linear",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Order::Hub => "Hub, final gated",
            Order::Linear => "Linear",
        }
    }
    pub fn from_id(s: &str) -> Option<Order> {
        let k = s.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|o| o.id() == k || o.name().eq_ignore_ascii_case(&k))
            .or(match k.as_str() {
                "gated" | "any" | "hub_and_spoke" | "spokes" => Some(Order::Hub),
                "chain" | "sequence" | "in_order" | "line" => Some(Order::Linear),
                _ => None,
            })
    }
}

/// A placed asset of the hand-painted pack: id (without the `hp:` prefix) and scale.
pub type A = (&'static str, f32);

/// What a theme's compound is built from (hand-painted pack ids).
pub struct Kit {
    /// court floors, by the court's shape
    pub cross_floor: &'static str,
    pub hall_floor: &'static str,
    pub round_floor: &'static str,
    pub oct_floor: &'static str,
    pub pyramid_floor: &'static str,
    pub final_floor: &'static str,
    /// the stepped pyramid court's temple
    pub pyramid: A,
    /// the island's temple
    pub island_temple: A,
    /// at the back of a cross yard, an octagon and a round shrine
    pub cross_back: A,
    pub oct_back: A,
    pub round_back: A,
    /// pillars (the pillared hall's rows, the axis, rings round plazas)
    pub pillar: A,
    pub brazier: A,
    /// the tall braziers at a court's stair head
    pub tall_brazier: A,
    pub statue: A,
    /// a trophy by a court's rim (a skull, a war drum)
    pub trophy: A,
    /// the final court: its throne and what stands behind it
    pub throne: A,
    pub final_back: A,
    /// the gate on the axis
    pub gate: A,
    /// the hub's monument, the round plaza's
    pub hub_center: A,
    pub plaza_center: A,
    /// the perimeter wall along the open edge, its towers and the gate behind the way out
    pub wall: A,
    pub tower: A,
    pub outer_gate: A,
    /// the gateway at the head of the entrance plaza's stairs
    pub entrance_arch: A,
    /// a court's corner pavilions (on its forecourt) and towers (at its back corners)
    pub pavilion: A,
    pub corner_tower: A,
    /// scattered between the courts
    pub ruins: &'static [(A, u32)],
    /// a trash camp (the first is the big piece)
    pub camp: &'static [A],
    /// trash creatures: (name, family)
    pub trash: &'static [(&'static str, &'static str)],
    /// the bosses' family
    pub boss_family: &'static str,
    /// what the courts are named for, and the patterns
    pub totems: &'static [&'static str],
    pub court_names: &'static [&'static str],
    pub boss_titles: &'static [&'static str],
    pub final_name: &'static str,
    pub final_title: &'static str,
    pub hub_name: &'static str,
}

const JUNGLE: Kit = Kit {
    cross_floor: "warchief_courtyard",
    hall_floor: "rivetjaw_courtyard",
    round_floor: "blooddrum_courtyard",
    oct_floor: "thorniron_courtyard",
    pyramid_floor: "bonehook_courtyard",
    final_floor: "skullbanner_courtyard",
    pyramid: ("step_temple", 1.0),
    island_temple: ("sun_temple", 1.0),
    cross_back: ("forest_shrine", 1.0),
    oct_back: ("skullbanner_skull_shrine", 1.0),
    round_back: ("dolmen", 1.0),
    pillar: ("stone_pillar", 1.9),
    brazier: ("orc_brazier", 1.2),
    tall_brazier: ("blooddrum_pillar_brazier", 1.0),
    statue: ("winged_statue", 1.4),
    trophy: ("giant_skull", 1.0),
    throne: ("warchief_throne", 1.0),
    final_back: ("step_temple", 1.15),
    gate: ("trilithon_gate", 1.6),
    hub_center: ("stone_circle", 1.0),
    plaza_center: ("rune_obelisk", 0.8),
    wall: ("castle_wall", 1.0),
    tower: ("wall_tower", 1.0),
    outer_gate: ("gatehouse", 1.0),
    entrance_arch: ("spore_mossy_stone_arch", 1.8),
    pavilion: ("skullbanner_ritual_pavilion", 1.0),
    corner_tower: ("ruined_tower", 0.8),
    ruins: &[
        (("stone_wall", 1.3), 6),
        (("stone_pillar", 1.7), 5),
        (("stonefist_rubble_pile", 1.4), 3),
        (("spore_mossy_stone_arch", 1.3), 2),
        (("ruined_tower", 0.9), 1),
        (("wayside_shrine", 1.2), 1),
        (("dolmen", 0.8), 1),
    ],
    camp: &[
        ("redhide_supply_leanto", 1.0),
        ("campfire", 1.0),
        ("bonehook_skull_post", 1.0),
        ("redhide_tusk_stake", 1.2),
        ("redhide_tusk_stake", 1.2),
        ("skullbanner_war_drum", 1.0),
        ("redhide_crate", 1.0),
        ("redhide_pelt", 1.0),
    ],
    trash: &[
        ("Jungle Troll Spearman", "troll"),
        ("Troll Hexcaller", "troll"),
        ("Temple Panther", "cat"),
        ("Bloodscale Raptor", "raptor"),
    ],
    boss_family: "troll",
    totems: &[
        "Bear",
        "Eagle",
        "Lynx",
        "Dragonhawk",
        "Serpent",
        "Panther",
        "Crocolisk",
        "Bat",
        "Raptor",
    ],
    court_names: &[
        "Court of the {T}",
        "{T} Terrace",
        "Temple of the {T}",
        "{T} Pyramid",
        "Shrine of the {T}",
        "{T} Yard",
    ],
    boss_titles: &[
        "the {T} Avatar",
        "Speaker of the {T}",
        "{T}-Blessed",
        "Keeper of the {T}",
    ],
    final_name: "Throne of the Chieftain",
    final_title: "the Warlord",
    hub_name: "The Heart of the City",
};

const DESERT: Kit = Kit {
    cross_floor: "rivetjaw_courtyard",
    hall_floor: "redhide_courtyard",
    round_floor: "dragonbone_foundation",
    oct_floor: "thorniron_courtyard",
    pyramid_floor: "cindermaw_courtyard",
    final_floor: "skullbanner_courtyard",
    pyramid: ("step_temple", 1.0),
    island_temple: ("sun_temple", 1.0),
    cross_back: ("crypt", 1.3),
    oct_back: ("dragonbone_vertebra_monument", 0.9),
    round_back: ("rune_obelisk", 1.0),
    pillar: ("hourglass_stone_marker", 1.3),
    brazier: ("skullbanner_fire_bowl", 1.4),
    tall_brazier: ("dragonbone_lantern_pedestal", 1.0),
    statue: ("winged_statue", 1.4),
    trophy: ("giant_ribcage", 0.9),
    throne: ("warchief_throne", 1.0),
    final_back: ("step_temple", 1.15),
    gate: ("mountain_gate", 1.0),
    hub_center: ("dragonbone_vertebra_monument", 1.0),
    plaza_center: ("rune_obelisk", 0.8),
    wall: ("castle_wall", 1.0),
    tower: ("wall_tower", 1.0),
    outer_gate: ("gatehouse", 1.0),
    entrance_arch: ("trilithon_gate", 1.2),
    pavilion: ("hourglass_study_pavilion", 0.8),
    corner_tower: ("rune_obelisk", 0.75),
    ruins: &[
        (("stone_wall", 1.3), 5),
        (("gravestone_obelisk", 1.6), 3),
        (("gravestone_broken", 1.6), 3),
        (("giant_vertebrae", 1.0), 2),
        (("crypt", 1.0), 2),
        (("dragonbone_rubble_pile", 1.2), 3),
        (("giant_skull", 0.9), 1),
    ],
    camp: &[
        ("war_tent", 0.8),
        ("campfire", 1.0),
        ("bone_pile_skull", 1.0),
        ("redhide_tusk_stake", 1.2),
        ("crate", 1.0),
        ("barrel", 1.0),
        ("sacks", 1.0),
    ],
    trash: &[
        ("Sand Mummy", "undead"),
        ("Tomb Scarab", "beetle"),
        ("Dune Ghoul", "undead"),
        ("Necropolis Acolyte", "cultist"),
    ],
    boss_family: "undead",
    totems: &[
        "Scarab", "Jackal", "Vulture", "Asp", "Sphinx", "Ibis", "Sandwyrm",
    ],
    court_names: &[
        "Tomb of the {T}",
        "{T} Necropolis",
        "Court of the {T}",
        "{T} Pyramid",
        "Sanctum of the {T}",
        "{T} Vault",
    ],
    boss_titles: &[
        "the {T} King",
        "Embalmer of the {T}",
        "Tomb Warden of the {T}",
        "the {T} Priest",
    ],
    final_name: "Terrace of the Sun King",
    final_title: "the Sun King",
    hub_name: "The Bone Crossroads",
};

const FROZEN: Kit = Kit {
    cross_floor: "stonefist_courtyard",
    hall_floor: "ironfang_courtyard",
    round_floor: "thorniron_courtyard",
    oct_floor: "bonehook_courtyard",
    pyramid_floor: "siegebreaker_courtyard",
    final_floor: "stonefist_courtyard",
    pyramid: ("defense_tower", 1.2),
    island_temple: ("moon_shrine", 1.5),
    cross_back: ("beacon_tower", 1.0),
    oct_back: ("ruined_tower", 1.0),
    round_back: ("rune_obelisk", 1.0),
    pillar: ("stone_pillar", 1.9),
    brazier: ("brazier", 1.3),
    tall_brazier: ("ironfang_brazier_pedestal", 1.3),
    statue: ("winged_statue", 1.4),
    trophy: ("giant_skull", 1.0),
    throne: ("warchief_throne", 1.0),
    final_back: ("castle_keep", 1.0),
    gate: ("gatehouse", 1.0),
    hub_center: ("rune_obelisk", 1.0),
    plaza_center: ("winged_statue", 2.0),
    wall: ("castle_wall", 1.0),
    tower: ("wall_tower", 1.0),
    outer_gate: ("gatehouse", 1.0),
    entrance_arch: ("trilithon_gate", 1.2),
    pavilion: ("moon_shrine", 1.0),
    corner_tower: ("wall_tower", 0.75),
    ruins: &[
        (("stone_wall", 1.3), 6),
        (("stone_pillar", 1.7), 4),
        (("ruined_tower", 0.9), 2),
        (("stonefist_rubble_pile", 1.4), 3),
        (("gravestone_tall", 1.6), 2),
    ],
    camp: &[
        ("war_tent", 0.8),
        ("campfire", 1.0),
        ("crate", 1.0),
        ("barrel", 1.0),
        ("weapon_rack", 1.0),
        ("firewood", 1.0),
    ],
    trash: &[
        ("Frost Wight", "undead"),
        ("Ice Troll", "troll"),
        ("Rime Wolf", "wolf"),
        ("Citadel Guard", "undead"),
    ],
    boss_family: "undead",
    totems: &["Wolf", "Bear", "Owl", "Wyrm", "Mammoth", "Stag", "Raven"],
    court_names: &[
        "{T} Bastion",
        "Hall of the {T}",
        "{T} Spire",
        "Keep of the {T}",
        "{T} Court",
        "{T} Watch",
    ],
    boss_titles: &[
        "the {T} Lord",
        "Frostborn of the {T}",
        "Warden of the {T}",
        "the {T} Knight",
    ],
    final_name: "The Frost Throne",
    final_title: "the Frost King",
    hub_name: "The Frozen Square",
};

const EMBER: Kit = Kit {
    cross_floor: "ashhammer_courtyard",
    hall_floor: "cindermaw_courtyard",
    round_floor: "blooddrum_courtyard",
    oct_floor: "ironfang_courtyard",
    pyramid_floor: "redhide_courtyard",
    final_floor: "skullbanner_courtyard",
    pyramid: ("step_temple", 1.0),
    island_temple: ("dark_spire", 0.8),
    cross_back: ("ashhammer_forge", 1.0),
    oct_back: ("ashhammer_furnace", 1.0),
    round_back: ("blooddrum_war_drum", 1.0),
    pillar: ("blooddrum_pillar_brazier", 1.0),
    brazier: ("orc_brazier", 1.2),
    tall_brazier: ("blooddrum_pillar_brazier", 1.2),
    statue: ("winged_statue", 1.4),
    trophy: ("cindermaw_cauldron", 1.0),
    throne: ("warchief_throne", 1.0),
    final_back: ("dark_spire", 1.0),
    gate: ("mountain_gate", 1.0),
    hub_center: ("ashhammer_furnace", 1.0),
    plaza_center: ("rune_obelisk", 0.8),
    wall: ("castle_wall", 1.0),
    tower: ("wall_tower", 1.0),
    outer_gate: ("gatehouse", 1.0),
    entrance_arch: ("trilithon_gate", 1.2),
    pavilion: ("ashhammer_bench_shelter", 1.0),
    corner_tower: ("ballista_tower", 0.85),
    ruins: &[
        (("stone_wall", 1.3), 5),
        (("stone_pillar", 1.7), 3),
        (("stonefist_rubble_pile", 1.4), 4),
        (("ashhammer_stakes", 1.4), 2),
        (("ruined_tower", 0.9), 1),
        (("orc_brazier", 1.2), 2),
    ],
    camp: &[
        ("redhide_supply_leanto", 1.0),
        ("redhide_fire_pit", 1.0),
        ("ashhammer_anvil", 1.0),
        ("ashhammer_coal_pan", 1.0),
        ("redhide_tusk_stake", 1.2),
        ("ashhammer_crate", 1.0),
    ],
    trash: &[
        ("Ember Imp", "demon"),
        ("Forge Golem", "golem"),
        ("Magma Hound", "hound"),
        ("Cinder Cultist", "cultist"),
    ],
    boss_family: "demon",
    totems: &[
        "Forge", "Anvil", "Cinder", "Magma", "Ash", "Brand", "Bellows",
    ],
    court_names: &[
        "The {T} Court",
        "Pit of the {T}",
        "{T} Altar",
        "{T} Pyramid",
        "Ring of the {T}",
        "{T} Works",
    ],
    boss_titles: &[
        "the {T} Lord",
        "Keeper of the {T}",
        "the {T}-Born",
        "Master of the {T}",
    ],
    final_name: "Heart of the Forge",
    final_title: "the Flame Tyrant",
    hub_name: "The Ash Market",
};

const TWILIGHT: Kit = Kit {
    cross_floor: "mushroom_crossroads_ground",
    hall_floor: "bonehook_courtyard",
    round_floor: "thorniron_courtyard",
    oct_floor: "rivetjaw_courtyard",
    pyramid_floor: "stonefist_courtyard",
    final_floor: "skullbanner_courtyard",
    pyramid: ("hourglass_observatory_tower", 0.85),
    island_temple: ("moon_shrine", 1.5),
    cross_back: ("forest_shrine", 1.0),
    oct_back: ("turtleshell_shrine_pavilion", 1.0),
    round_back: ("stone_circle", 0.8),
    pillar: ("stone_pillar", 1.9),
    brazier: ("mushroom_standing_lantern", 1.6),
    tall_brazier: ("turtleshell_pearl_lantern", 1.0),
    statue: ("winged_statue", 1.4),
    trophy: ("dolmen", 0.9),
    throne: ("warchief_throne", 1.0),
    final_back: ("arcane_spire", 1.0),
    gate: ("trilithon_gate", 1.6),
    hub_center: ("stone_circle", 1.0),
    plaza_center: ("rune_obelisk", 0.8),
    wall: ("castle_wall", 1.0),
    tower: ("wall_tower", 1.0),
    outer_gate: ("gatehouse", 1.0),
    entrance_arch: ("spore_mossy_stone_arch", 1.8),
    pavilion: ("turtleshell_shrine_pavilion", 1.0),
    corner_tower: ("mage_tower", 0.55),
    ruins: &[
        (("stone_wall", 1.3), 5),
        (("stone_pillar", 1.7), 4),
        (("spore_mossy_stone_arch", 1.3), 3),
        (("trilithon_gate", 0.9), 1),
        (("dolmen", 0.8), 1),
        (("wayside_shrine", 1.2), 2),
        (("stonefist_rubble_pile", 1.4), 2),
    ],
    camp: &[
        ("skullbanner_ritual_pavilion", 1.0),
        ("campfire", 1.0),
        ("mushroom_lantern_post", 1.0),
        ("crate", 1.0),
        ("mage_supply_crate", 1.0),
    ],
    trash: &[
        ("Shade Elf Revenant", "elf"),
        ("Moonlit Wisp", "spirit"),
        ("Thornback Treant", "treant"),
        ("Dusk Stalker", "cat"),
    ],
    boss_family: "elf",
    totems: &["Moon", "Star", "Dusk", "Thorn", "Willow", "Owl", "Mist"],
    court_names: &[
        "{T} Sanctum",
        "Court of the {T}",
        "{T} Spire",
        "{T} Garden",
        "Shrine of the {T}",
        "{T} Ring",
    ],
    boss_titles: &[
        "the {T} Queen",
        "Warden of the {T}",
        "the {T}-Sworn",
        "Keeper of the {T}",
    ],
    final_name: "The Moonlit Throne",
    final_title: "the Last Prince",
    hub_name: "The Moonwell Glade",
};

/// Every asset id a theme's kit uses (tests check them against the pack's catalog).
pub fn kit_assets(k: &Kit) -> Vec<&'static str> {
    let mut v = vec![
        k.cross_floor,
        k.hall_floor,
        k.round_floor,
        k.oct_floor,
        k.pyramid_floor,
        k.final_floor,
    ];
    for a in [
        k.pyramid,
        k.island_temple,
        k.cross_back,
        k.oct_back,
        k.round_back,
        k.pillar,
        k.brazier,
        k.tall_brazier,
        k.statue,
        k.trophy,
        k.throne,
        k.final_back,
        k.gate,
        k.hub_center,
        k.plaza_center,
        k.wall,
        k.tower,
        k.outer_gate,
        k.entrance_arch,
        k.pavilion,
        k.corner_tower,
    ] {
        v.push(a.0);
    }
    v.extend(k.ruins.iter().map(|(a, _)| a.0));
    v.extend(k.camp.iter().map(|a| a.0));
    v
}

/// A boss's name: two or three syllables (the same seed, the same name).
pub fn boss_name(rng: &mut Rng) -> String {
    const A: [&str; 16] = [
        "Ka", "Ro", "Ma", "Vo", "Te", "Ul", "Go", "Na", "Za", "Ash", "Ke", "Mo", "Ra", "Ti", "Hu",
        "Sa",
    ];
    const B: [&str; 14] = [
        "lak", "vek", "tor", "ruk", "zan", "mir", "gal", "dun", "rok", "sha", "lo", "ken", "jar",
        "tum",
    ];
    const C: [&str; 8] = ["", "", "", "a", "i", "o", "ash", "un"];
    let s = format!(
        "{}{}{}",
        A[rng.below(A.len() as u32) as usize],
        B[rng.below(B.len() as u32) as usize],
        C[rng.below(C.len() as u32) as usize]
    );
    s
}

/// `pattern` with `{T}` the totem.
pub fn fill(pattern: &str, totem: &str) -> String {
    pattern.replace("{T}", totem)
}
