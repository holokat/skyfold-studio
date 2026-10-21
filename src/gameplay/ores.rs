//! Ores: the catalogue of 25 mineable ores in five tiers (common → legendary), each with its
//! typical use, where it forms (biomes, height band, cliffs) and how it shows in the rock (`Look`,
//! drawn by `deposits`), plus ore deposit areas.
//!
//! A deposit is a resource node carrying an `Ore` (`ResourceNode::ore`); nodes saved before ores
//! existed get one from their name or tier (`ResourceNode::ore()`). An `OreArea` is an outline on
//! the ground with a small ore table: its deposits are scattered inside it on suitable ground (not
//! in water, on roads, paving or next to buildings, not on steep cliffs unless the ore forms in
//! them) from a jittered grid anchored to the world, so the same area and seed always give the
//! same deposits, and editing it moves only the deposits near the change.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::edit::{Ground, point_in_polygon, polygon_area, unique_name};
use super::{Gameplay, ResourceKind, ResourceNode, SpawnRegion};
use crate::biomes::BiomePreset;
use crate::noise::{Rng, hash2};
use crate::objects::{Category, Library, ObjectStore};
use crate::roads::Roads;
use crate::terrain::layers::Layer;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ore {
    Copper,
    Tin,
    Iron,
    Coal,
    Stone,
    Silver,
    Gold,
    Mithril,
    Cobalt,
    Nickel,
    Adamantite,
    Thorium,
    Platinum,
    Titanium,
    Orichalcum,
    Obsidian,
    DarkIron,
    Elementium,
    Truesilver,
    Runite,
    Eternium,
    Arcanite,
    Starmetal,
    Voidstone,
    Dragonsteel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub const ALL: [Rarity; 5] = [
        Rarity::Common,
        Rarity::Uncommon,
        Rarity::Rare,
        Rarity::Epic,
        Rarity::Legendary,
    ];
    pub fn of_tier(tier: u8) -> Rarity {
        Self::ALL[(tier.clamp(1, 5) - 1) as usize]
    }
    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "Common",
            Rarity::Uncommon => "Uncommon",
            Rarity::Rare => "Rare",
            Rarity::Epic => "Epic",
            Rarity::Legendary => "Legendary",
        }
    }
    /// The usual MMO item-quality colour (sRGB).
    pub fn color(self) -> [u8; 3] {
        match self {
            Rarity::Common => [214, 218, 224],
            Rarity::Uncommon => [64, 214, 72],
            Rarity::Rare => [52, 140, 255],
            Rarity::Epic => [182, 92, 255],
            Rarity::Legendary => [255, 146, 32],
        }
    }
}

/// How the ore shows in the host rock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// metal veins with nuggets, a coloured halo (patina, rust, quartz) around them
    Vein = 0,
    /// matte dark seams in layers (coal)
    Seam = 1,
    /// plain granite: no ore, salt-and-pepper grain (building stone)
    Plain = 2,
    /// patches of black volcanic glass with conchoidal ripples
    Glass = 3,
    /// faceted crystal patches
    Crystal = 4,
    /// veins with glowing rune glyphs carved along them
    Runes = 5,
}

/// Extra effects (`Look::fx` bits).
pub mod fx {
    /// the ore's colour shifts between `ore` and `halo` with the view angle (elementium)
    pub const SHIFT: u8 = 1;
    /// tiny bright star specks in the veins (starmetal)
    pub const STARS: u8 = 2;
    /// glowing ember cracks inside the veins (dark iron, dragonsteel)
    pub const EMBERS: u8 = 4;
    /// the glow sits on the veins' edges (voidstone)
    pub const RIM: u8 = 8;
    /// the veins are white quartz (`halo`) with the metal in nuggets and specks (gold)
    pub const QUARTZ: u8 = 16;
    /// the ore weathers: its `halo` colour (patina, rust) stains patches of the rock
    pub const STAIN: u8 = 32;
}

/// Photoscanned surfaces (CC0 scans, `assets/textures/ores`) that fill parts of a deposit instead
/// of a flat colour: drawn in the rock's own space at their real-world scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surf {
    /// a flat colour
    None,
    /// native copper: dark, oxidised copper metal
    Copper,
    /// copper under a blue-green verdigris patina
    Verdigris,
    /// a coarse iron-oxide crust (gossan)
    Rust,
    /// rusted iron with bare metal showing
    RustyIron,
    /// black, finely fractured rock (coal)
    Coal,
    /// white, iron-stained fractured quartz
    Quartz,
    /// impure, rough native gold
    Gold,
    /// bright raw metal with dark tarnish patches
    Silver,
    /// grey, pitted raw metal
    RawMetal,
    /// salt-and-pepper granite
    Granite,
    /// black glass with faint flow banding
    Obsidian,
}

impl Surf {
    /// The scans, in their texture-array order.
    pub const ALL: [Surf; 11] = [
        Surf::Copper,
        Surf::Verdigris,
        Surf::Rust,
        Surf::RustyIron,
        Surf::Coal,
        Surf::Quartz,
        Surf::Gold,
        Surf::Silver,
        Surf::RawMetal,
        Surf::Granite,
        Surf::Obsidian,
    ];
    /// The file name stem (`<id>_albedo.ktx2`…).
    pub fn id(self) -> &'static str {
        match self {
            Surf::None => "",
            Surf::Copper => "copper",
            Surf::Verdigris => "verdigris",
            Surf::Rust => "rust",
            Surf::RustyIron => "rusty_iron",
            Surf::Coal => "coal",
            Surf::Quartz => "quartz",
            Surf::Gold => "gold",
            Surf::Silver => "silver",
            Surf::RawMetal => "raw_metal",
            Surf::Granite => "granite",
            Surf::Obsidian => "obsidian",
        }
    }
    /// Its layer in the texture arrays.
    pub fn layer(self) -> Option<usize> {
        Self::ALL.iter().position(|s| *s == self)
    }
    /// Metres of rock one repeat of the scan covers (crusts and grains at their real size).
    pub fn tile(self) -> f32 {
        match self {
            Surf::Copper | Surf::Gold => 0.35,
            Surf::Verdigris | Surf::Silver | Surf::RawMetal => 0.45,
            Surf::Quartz => 0.7,
            Surf::Rust | Surf::Granite => 0.8,
            Surf::RustyIron => 0.9,
            Surf::Coal => 1.1,
            Surf::Obsidian | Surf::None => 1.2,
        }
    }
}

/// A photoscanned surface filling part of a deposit, recoloured `recolor` of the way to the ore's
/// colour (0 as scanned … 1 the ore's colour and roughness, keeping the scan's light and dark).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub surf: Surf,
    pub recolor: f32,
}

const FLAT: Fill = Fill {
    surf: Surf::None,
    recolor: 0.0,
};

/// What a deposit of the ore looks like (colours sRGB).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub style: Style,
    pub ore: [u8; 3],
    /// the band around the veins (patina, rust, quartz, tarnish) or the second colour
    pub halo: [u8; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub glow: [u8; 3],
    /// glow brightness (cd/m², under the camera's exposure: faint by day, glowing at dusk)
    pub glow_nits: f32,
    /// how much of the rock the veins cover (0..1)
    pub veins: f32,
    /// nuggets along the veins (0..1)
    pub nuggets: f32,
    /// tint of the host rock (multiplies the scan)
    pub host: [u8; 3],
    /// how much of the scan's own colour the host rock keeps (0 grey … 1 as scanned)
    pub host_sat: f32,
    pub fx: u8,
    /// crystal shards grow out of the rock
    pub shards: bool,
    /// what fills the veins and nuggets (coal seams, obsidian glass, the granite), recoloured
    /// towards `ore`
    pub fill: Fill,
    /// what fills the halo band and the stains (QUARTZ: the veins), recoloured towards `halo`
    pub halo_fill: Fill,
}

impl Look {
    const fn fill(mut self, surf: Surf, recolor: f32) -> Self {
        self.fill = Fill { surf, recolor };
        self
    }
    const fn halo_fill(mut self, surf: Surf, recolor: f32) -> Self {
        self.halo_fill = Fill { surf, recolor };
        self
    }
}

/// Where an ore forms.
#[derive(Clone, Copy, Debug)]
pub struct Habitat {
    pub biomes: &'static [BiomePreset],
    /// favoured height band, as a fraction of the way from sea level to the highest peak
    pub alt: (f32, f32),
    /// forms in cliffs and steep faces
    pub cliffs: bool,
    /// for hints: "hills and mountains"
    pub about: &'static str,
}

pub struct OreDef {
    pub id: &'static str,
    pub name: &'static str,
    /// The ore's colour in the editor (swatches, area outlines, labels), sRGB.
    pub color: [u8; 3],
    pub tier: u8,
    pub uses: &'static str,
    pub habitat: Habitat,
    pub look: Look,
}

#[allow(clippy::too_many_arguments)]
const fn look(
    style: Style,
    ore: [u8; 3],
    halo: [u8; 3],
    metallic: f32,
    roughness: f32,
    glow: [u8; 3],
    glow_nits: f32,
    veins: f32,
    nuggets: f32,
    host: ([u8; 3], f32),
    fx: u8,
    shards: bool,
) -> Look {
    Look {
        style,
        ore,
        halo,
        metallic,
        roughness,
        glow,
        glow_nits,
        veins,
        nuggets,
        host: host.0,
        host_sat: host.1,
        fx,
        shards,
        fill: FLAT,
        halo_fill: FLAT,
    }
}

const fn hab(
    biomes: &'static [BiomePreset],
    alt: (f32, f32),
    cliffs: bool,
    about: &'static str,
) -> Habitat {
    Habitat {
        biomes,
        alt,
        cliffs,
        about,
    }
}

use BiomePreset as B;
use Style::*;
const NO: [u8; 3] = [0, 0, 0];

/// The catalogue, in `Ore` order.
pub const DEFS: [OreDef; 25] = [
    // ---- tier 1, common
    OreDef {
        id: "copper",
        name: "Copper",
        color: [214, 122, 70],
        tier: 1,
        uses: "Beginner weapons, tools",
        habitat: hab(
            &[B::Meadow, B::TemperateForest, B::AutumnWoods, B::Desert],
            (0.02, 0.5),
            false,
            "low hills",
        ),
        look: look(
            Vein,
            [200, 112, 62],
            [70, 156, 122],
            0.9,
            0.36,
            NO,
            0.0,
            0.55,
            0.45,
            ([236, 226, 214], 0.55),
            fx::STAIN,
            false,
        )
        .fill(Surf::Copper, 0.0)
        .halo_fill(Surf::Verdigris, 0.0),
    },
    OreDef {
        id: "tin",
        name: "Tin",
        color: [192, 198, 204],
        tier: 1,
        uses: "Bronze alloy",
        habitat: hab(
            &[B::Meadow, B::TemperateForest, B::Taiga, B::TropicalCoast],
            (0.0, 0.4),
            false,
            "low hills and stream banks",
        ),
        look: look(
            Vein,
            [184, 186, 182],
            [118, 92, 74],
            0.75,
            0.52,
            NO,
            0.0,
            0.5,
            0.35,
            ([226, 226, 226], 0.45),
            fx::STAIN,
            false,
        )
        .fill(Surf::RawMetal, 1.0)
        .halo_fill(Surf::Rust, 0.8),
    },
    OreDef {
        id: "iron",
        name: "Iron",
        color: [184, 96, 62],
        tier: 1,
        uses: "Weapons, armor",
        habitat: hab(
            &[B::TemperateForest, B::Taiga, B::AutumnWoods, B::Alpine],
            (0.15, 0.8),
            true,
            "hills and mountains",
        ),
        look: look(
            Vein,
            [72, 46, 40],
            [168, 84, 40],
            0.25,
            0.78,
            NO,
            0.0,
            0.62,
            0.3,
            ([232, 216, 204], 0.8),
            fx::STAIN,
            false,
        )
        .fill(Surf::RustyIron, 0.7)
        .halo_fill(Surf::Rust, 0.0),
    },
    OreDef {
        id: "coal",
        name: "Coal",
        color: [128, 128, 132],
        tier: 1,
        uses: "Steel smelting",
        habitat: hab(
            &[B::Taiga, B::TemperateForest, B::Swamp, B::AutumnWoods],
            (0.05, 0.6),
            true,
            "hillsides and old swamps",
        ),
        look: look(
            Seam,
            [16, 15, 15],
            [52, 48, 44],
            0.0,
            0.42,
            NO,
            0.0,
            0.66,
            0.0,
            ([196, 190, 184], 0.35),
            0,
            false,
        )
        .fill(Surf::Coal, 0.75),
    },
    OreDef {
        id: "stone",
        name: "Stone",
        color: [204, 198, 188],
        tier: 1,
        uses: "Construction, masonry",
        habitat: hab(&[], (0.0, 1.0), true, "rocky ground everywhere"),
        look: look(
            Plain,
            [138, 134, 128],
            [190, 128, 112],
            0.0,
            0.78,
            NO,
            0.0,
            0.0,
            0.0,
            ([250, 245, 238], 0.3),
            0,
            false,
        )
        .fill(Surf::Granite, 0.6),
    },
    // ---- tier 2, uncommon
    OreDef {
        id: "silver",
        name: "Silver",
        color: [218, 222, 230],
        tier: 2,
        uses: "Jewelry, magical equipment",
        habitat: hab(
            &[B::Taiga, B::Alpine, B::Tundra, B::TemperateForest],
            (0.3, 0.9),
            true,
            "mountain slopes",
        ),
        look: look(
            Vein,
            [200, 202, 204],
            [64, 66, 72],
            1.0,
            0.46,
            NO,
            0.0,
            0.5,
            0.5,
            ([226, 226, 230], 0.3),
            0,
            false,
        )
        .fill(Surf::Silver, 0.5)
        .halo_fill(Surf::Silver, 1.0),
    },
    OreDef {
        id: "gold",
        name: "Gold",
        color: [255, 200, 70],
        tier: 2,
        uses: "Jewelry, enchanting",
        habitat: hab(
            &[B::Desert, B::Jungle, B::TropicalCoast, B::Meadow],
            (0.0, 0.6),
            false,
            "deserts, jungles and river valleys",
        ),
        look: look(
            Vein,
            [255, 186, 52],
            [236, 206, 132],
            1.0,
            0.24,
            NO,
            0.0,
            0.7,
            1.0,
            ([214, 204, 190], 0.6),
            fx::QUARTZ,
            false,
        )
        .fill(Surf::Gold, 0.0)
        .halo_fill(Surf::Quartz, 0.55),
    },
    OreDef {
        id: "mithril",
        name: "Mithril",
        color: [160, 210, 255],
        tier: 2,
        uses: "Lightweight magical armor",
        habitat: hab(
            &[B::Alpine, B::Tundra],
            (0.55, 1.0),
            true,
            "high alpine peaks",
        ),
        look: look(
            Vein,
            [198, 222, 242],
            [150, 192, 228],
            1.0,
            0.14,
            [150, 210, 255],
            160.0,
            0.55,
            0.4,
            ([214, 220, 232], 0.25),
            0,
            false,
        )
        .fill(Surf::Silver, 0.85),
    },
    OreDef {
        id: "cobalt",
        name: "Cobalt",
        color: [76, 116, 244],
        tier: 2,
        uses: "Advanced weapons, blue alloys",
        habitat: hab(
            &[B::Taiga, B::Tundra, B::Alpine],
            (0.3, 0.8),
            true,
            "cold mountains",
        ),
        look: look(
            Vein,
            [36, 70, 184],
            [92, 110, 172],
            0.75,
            0.32,
            NO,
            0.0,
            0.55,
            0.35,
            ([210, 212, 222], 0.35),
            0,
            false,
        )
        .fill(Surf::RawMetal, 1.0),
    },
    OreDef {
        id: "nickel",
        name: "Nickel",
        color: [172, 192, 142],
        tier: 2,
        uses: "Alloy crafting",
        habitat: hab(
            &[B::Tundra, B::Taiga, B::Volcanic],
            (0.2, 0.7),
            false,
            "tundra and old volcanic rock",
        ),
        look: look(
            Vein,
            [174, 184, 150],
            [108, 152, 94],
            0.85,
            0.44,
            NO,
            0.0,
            0.5,
            0.4,
            ([220, 222, 210], 0.45),
            fx::STAIN,
            false,
        )
        .fill(Surf::RawMetal, 1.0)
        .halo_fill(Surf::Verdigris, 0.7),
    },
    // ---- tier 3, rare
    OreDef {
        id: "adamantite",
        name: "Adamantite",
        color: [90, 200, 140],
        tier: 3,
        uses: "High-strength armor",
        habitat: hab(
            &[B::Alpine, B::Volcanic, B::HauntedWoods],
            (0.5, 1.0),
            true,
            "high cliffs",
        ),
        look: look(
            Vein,
            [34, 66, 48],
            [18, 30, 24],
            1.0,
            0.2,
            [60, 210, 120],
            90.0,
            0.6,
            0.45,
            ([150, 156, 150], 0.3),
            0,
            false,
        )
        .fill(Surf::RawMetal, 1.0),
    },
    OreDef {
        id: "thorium",
        name: "Thorium",
        color: [192, 232, 92],
        tier: 3,
        uses: "Advanced equipment",
        habitat: hab(
            &[B::Desert, B::Volcanic, B::Swamp],
            (0.1, 0.7),
            false,
            "deserts, swamps and volcanic land",
        ),
        look: look(
            Vein,
            [196, 232, 84],
            [96, 116, 44],
            0.5,
            0.38,
            [190, 255, 90],
            620.0,
            0.62,
            0.4,
            ([160, 160, 150], 0.5),
            fx::STAIN,
            false,
        )
        .fill(Surf::RawMetal, 1.0)
        .halo_fill(Surf::Rust, 1.0),
    },
    OreDef {
        id: "platinum",
        name: "Platinum",
        color: [232, 234, 238],
        tier: 3,
        uses: "Premium jewelry, enchanting",
        habitat: hab(
            &[B::Alpine, B::Tundra, B::Desert],
            (0.4, 0.9),
            false,
            "high, dry country",
        ),
        look: look(
            Vein,
            [228, 228, 224],
            [188, 188, 184],
            1.0,
            0.42,
            NO,
            0.0,
            0.45,
            0.6,
            ([216, 216, 216], 0.3),
            0,
            false,
        )
        .fill(Surf::Silver, 0.8),
    },
    OreDef {
        id: "titanium",
        name: "Titanium",
        color: [142, 162, 194],
        tier: 3,
        uses: "High-tier weapons",
        habitat: hab(
            &[B::Alpine, B::Taiga, B::Tundra],
            (0.4, 1.0),
            true,
            "mountain cliffs",
        ),
        look: look(
            Vein,
            [118, 134, 158],
            [160, 170, 186],
            1.0,
            0.45,
            NO,
            0.0,
            0.55,
            0.35,
            ([206, 208, 216], 0.3),
            0,
            false,
        )
        .fill(Surf::RawMetal, 1.0),
    },
    OreDef {
        id: "orichalcum",
        name: "Orichalcum",
        color: [242, 132, 62],
        tier: 3,
        uses: "Magical weapons, artifacts",
        habitat: hab(
            &[B::Jungle, B::TropicalCoast, B::Desert],
            (0.2, 0.8),
            false,
            "jungle ruins and deserts",
        ),
        look: look(
            Vein,
            [234, 122, 60],
            [204, 152, 92],
            1.0,
            0.22,
            [255, 140, 60],
            110.0,
            0.5,
            0.55,
            ([226, 210, 194], 0.75),
            fx::STAIN,
            false,
        )
        .fill(Surf::Gold, 0.7)
        .halo_fill(Surf::Verdigris, 0.6),
    },
    // ---- tier 4, epic
    OreDef {
        id: "obsidian",
        name: "Obsidian",
        color: [156, 146, 176],
        tier: 4,
        uses: "Dark weapons, ritual equipment",
        habitat: hab(&[B::Volcanic], (0.0, 1.0), true, "volcanic land"),
        look: look(
            Glass,
            [8, 8, 10],
            [44, 40, 44],
            0.0,
            0.04,
            NO,
            0.0,
            0.75,
            0.0,
            ([118, 112, 112], 0.3),
            0,
            false,
        )
        .fill(Surf::Obsidian, 0.0),
    },
    OreDef {
        id: "dark_iron",
        name: "Dark Iron",
        color: [224, 84, 42],
        tier: 4,
        uses: "Fire-resistant armor",
        habitat: hab(
            &[B::Volcanic, B::HauntedWoods],
            (0.2, 0.9),
            true,
            "volcanic and blighted land",
        ),
        look: look(
            Vein,
            [104, 56, 48],
            [92, 32, 20],
            0.85,
            0.34,
            [255, 84, 22],
            900.0,
            0.6,
            0.3,
            ([122, 106, 100], 0.5),
            fx::EMBERS,
            false,
        )
        .fill(Surf::RustyIron, 1.0)
        .halo_fill(Surf::Rust, 1.0),
    },
    OreDef {
        id: "elementium",
        name: "Elementium",
        color: [92, 152, 255],
        tier: 4,
        uses: "Elemental equipment",
        habitat: hab(
            &[B::Volcanic, B::Alpine, B::Swamp],
            (0.4, 1.0),
            true,
            "where the elements clash: volcanoes, peaks, marshes",
        ),
        look: look(
            Vein,
            [62, 124, 255],
            [255, 132, 40],
            0.85,
            0.2,
            [130, 176, 255],
            450.0,
            0.55,
            0.4,
            ([190, 190, 200], 0.4),
            fx::SHIFT,
            true,
        )
        .fill(Surf::RawMetal, 1.0),
    },
    OreDef {
        id: "truesilver",
        name: "Truesilver",
        color: [212, 232, 255],
        tier: 4,
        uses: "Enchanted silver equipment",
        habitat: hab(
            &[B::Alpine, B::Tundra],
            (0.6, 1.0),
            true,
            "high alpine peaks",
        ),
        look: look(
            Vein,
            [234, 244, 255],
            [172, 202, 255],
            1.0,
            0.08,
            [200, 226, 255],
            260.0,
            0.55,
            0.5,
            ([226, 228, 236], 0.25),
            0,
            false,
        )
        .fill(Surf::Silver, 0.9),
    },
    OreDef {
        id: "runite",
        name: "Runite",
        color: [44, 222, 252],
        tier: 4,
        uses: "Rune-infused equipment",
        habitat: hab(
            &[B::HauntedWoods, B::Alpine, B::Tundra],
            (0.3, 1.0),
            true,
            "old, haunted and frozen places",
        ),
        look: look(
            Runes,
            [44, 176, 198],
            [22, 58, 68],
            0.6,
            0.3,
            [40, 230, 255],
            800.0,
            0.5,
            0.2,
            ([150, 160, 166], 0.3),
            0,
            false,
        )
        .fill(Surf::RawMetal, 1.0),
    },
    // ---- tier 5, legendary
    OreDef {
        id: "eternium",
        name: "Eternium",
        color: [255, 230, 160],
        tier: 5,
        uses: "Legendary crafting",
        habitat: hab(
            &[B::Alpine, B::Desert],
            (0.75, 1.0),
            true,
            "the highest peaks",
        ),
        look: look(
            Vein,
            [255, 232, 168],
            [255, 244, 214],
            1.0,
            0.12,
            [255, 222, 150],
            650.0,
            0.55,
            0.6,
            ([230, 224, 214], 0.5),
            0,
            true,
        )
        .fill(Surf::Gold, 1.0),
    },
    OreDef {
        id: "arcanite",
        name: "Arcanite",
        color: [232, 72, 222],
        tier: 5,
        uses: "Arcane weapons",
        habitat: hab(
            &[B::HauntedWoods, B::Swamp, B::Jungle],
            (0.2, 0.9),
            false,
            "places thick with magic",
        ),
        look: look(
            Crystal,
            [214, 44, 196],
            [92, 22, 92],
            0.0,
            0.07,
            [255, 60, 220],
            700.0,
            0.6,
            0.0,
            ([170, 160, 172], 0.4),
            0,
            true,
        ),
    },
    OreDef {
        id: "starmetal",
        name: "Starmetal",
        color: [112, 132, 255],
        tier: 5,
        uses: "Celestial equipment",
        habitat: hab(
            &[B::Desert, B::Tundra, B::Alpine],
            (0.5, 1.0),
            false,
            "open high plains under clear skies",
        ),
        look: look(
            Vein,
            [44, 76, 196],
            [18, 26, 78],
            0.85,
            0.22,
            [205, 222, 255],
            1500.0,
            0.7,
            0.3,
            ([176, 176, 190], 0.35),
            fx::STARS,
            false,
        )
        .fill(Surf::RawMetal, 1.0),
    },
    OreDef {
        id: "voidstone",
        name: "Voidstone",
        color: [164, 72, 255],
        tier: 5,
        uses: "Shadow equipment",
        habitat: hab(
            &[B::HauntedWoods, B::Swamp],
            (0.0, 0.8),
            false,
            "haunted and dark places",
        ),
        look: look(
            Vein,
            [30, 12, 42],
            [14, 5, 20],
            0.3,
            0.14,
            [150, 52, 255],
            650.0,
            0.65,
            0.0,
            ([112, 102, 118], 0.35),
            fx::RIM,
            true,
        )
        .fill(Surf::Obsidian, 1.0),
    },
    OreDef {
        id: "dragonsteel",
        name: "Dragonsteel",
        color: [232, 44, 52],
        tier: 5,
        uses: "Dragon-themed weapons",
        habitat: hab(&[B::Volcanic], (0.6, 1.0), true, "volcanic peaks"),
        look: look(
            Vein,
            [152, 22, 28],
            [62, 10, 10],
            1.0,
            0.34,
            [255, 92, 20],
            1100.0,
            0.6,
            0.4,
            ([142, 112, 106], 0.55),
            fx::EMBERS,
            false,
        )
        .fill(Surf::RustyIron, 1.0)
        .halo_fill(Surf::RustyIron, 1.0),
    },
];

impl Ore {
    pub const ALL: [Ore; 25] = [
        Ore::Copper,
        Ore::Tin,
        Ore::Iron,
        Ore::Coal,
        Ore::Stone,
        Ore::Silver,
        Ore::Gold,
        Ore::Mithril,
        Ore::Cobalt,
        Ore::Nickel,
        Ore::Adamantite,
        Ore::Thorium,
        Ore::Platinum,
        Ore::Titanium,
        Ore::Orichalcum,
        Ore::Obsidian,
        Ore::DarkIron,
        Ore::Elementium,
        Ore::Truesilver,
        Ore::Runite,
        Ore::Eternium,
        Ore::Arcanite,
        Ore::Starmetal,
        Ore::Voidstone,
        Ore::Dragonsteel,
    ];
    pub fn def(self) -> &'static OreDef {
        &DEFS[self as usize]
    }
    pub fn id(self) -> &'static str {
        self.def().id
    }
    pub fn name(self) -> &'static str {
        self.def().name
    }
    pub fn tier(self) -> u8 {
        self.def().tier
    }
    pub fn color(self) -> [u8; 3] {
        self.def().color
    }
    pub fn rarity(self) -> Rarity {
        Rarity::of_tier(self.tier())
    }
    pub fn uses(self) -> &'static str {
        self.def().uses
    }
    pub fn look(self) -> &'static Look {
        &self.def().look
    }
    pub fn habitat(self) -> &'static Habitat {
        &self.def().habitat
    }
    /// The ores of one tier, in catalogue order.
    pub fn of_tier(tier: u8) -> impl Iterator<Item = Ore> {
        Self::ALL.into_iter().filter(move |o| o.tier() == tier)
    }
    /// Quarry stone is gathered as stone, everything else is mined as ore.
    pub fn kind(self) -> ResourceKind {
        if self == Ore::Stone {
            ResourceKind::Stone
        } else {
            ResourceKind::Ore
        }
    }
    /// A deposit's default name.
    pub fn deposit_name(self) -> String {
        match self {
            Ore::Stone => "Granite Outcrop".into(),
            Ore::Coal => "Coal Seam".into(),
            Ore::Obsidian => "Obsidian Outcrop".into(),
            o if o.look().style == Style::Crystal => format!("{} Cluster", o.name()),
            o => format!("{} Vein", o.name()),
        }
    }
    /// Seconds until a mined deposit comes back (rarer ores take longer).
    pub fn default_respawn(self) -> f32 {
        [300.0, 450.0, 600.0, 900.0, 1800.0][(self.tier().clamp(1, 5) - 1) as usize]
    }
    /// The ore usually found with this one (mixed into new deposit areas at a lower weight).
    pub fn companion(self) -> Option<Ore> {
        Some(match self {
            Ore::Copper => Ore::Tin,
            Ore::Tin => Ore::Copper,
            Ore::Iron => Ore::Coal,
            Ore::Coal => Ore::Iron,
            Ore::Silver => Ore::Gold,
            Ore::Gold => Ore::Silver,
            Ore::Mithril => Ore::Silver,
            Ore::Cobalt => Ore::Nickel,
            Ore::Nickel => Ore::Cobalt,
            Ore::Thorium => Ore::Titanium,
            Ore::Platinum => Ore::Gold,
            Ore::Titanium => Ore::Nickel,
            Ore::Orichalcum => Ore::Gold,
            Ore::Obsidian => Ore::DarkIron,
            Ore::DarkIron => Ore::Obsidian,
            Ore::Truesilver => Ore::Platinum,
            Ore::Dragonsteel => Ore::Obsidian,
            _ => return None,
        })
    }
    /// By id or name, any case and spacing ("dark iron", "DarkIron", "dark_iron").
    pub fn parse(s: &str) -> Option<Ore> {
        let n: String = s
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        Self::ALL.into_iter().find(|o| {
            let id: String = o
                .id()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            id == n || format!("{o:?}").to_lowercase() == n
        })
    }
    /// The ore a name mentions ("Copper Vein", "Adamant Vein", "rich dark iron"), longest name first.
    pub fn in_name(name: &str) -> Option<Ore> {
        let l = name.to_lowercase();
        let mut all = Self::ALL;
        all.sort_by_key(|o| std::cmp::Reverse(o.name().len()));
        all.into_iter()
            .find(|o| l.contains(&o.name().to_lowercase()))
            .or_else(|| l.contains("adamant").then_some(Ore::Adamantite))
    }
    /// The ore of a mining node saved before ores existed: from its name, else its tier (the old
    /// tiers ran 1–8 with copper, tin, iron, silver, gold, mithril, starmetal, adamant).
    pub fn from_legacy(variant: &str, tier: u8) -> Ore {
        Self::in_name(variant).unwrap_or(
            [
                Ore::Copper,
                Ore::Tin,
                Ore::Iron,
                Ore::Silver,
                Ore::Gold,
                Ore::Mithril,
                Ore::Starmetal,
                Ore::Adamantite,
            ][(tier.clamp(1, 8) - 1) as usize],
        )
    }
    /// How well a spot suits the ore (0..1): its biomes, height band (`alt`: 0 at sea level, 1 on
    /// the highest peak) and slope. Unpainted land (no biome) suits common ores well, rare ones
    /// less; the rarer the ore, the more it keeps to its own biomes.
    pub fn affinity(self, biome: Option<BiomePreset>, alt: f32, slope_deg: f32) -> f32 {
        let h = self.habitat();
        let t = self.tier() as f32;
        let b = match biome {
            _ if h.biomes.is_empty() => 1.0,
            Some(b) if h.biomes.contains(&b) => 1.0,
            None => 0.75 - 0.1 * t,
            Some(_) => (0.32 - 0.07 * t).max(0.0),
        };
        let below = ((h.alt.0 - alt) / 0.15).max(0.0);
        let above = ((alt - h.alt.1) / 0.15).max(0.0);
        let a = (1.0 - (below + above)).max(0.08);
        let s = if h.cliffs {
            0.7 + 0.3 * crate::noise::smoothstep(15.0, 40.0, slope_deg)
        } else {
            1.0 - 0.6 * crate::noise::smoothstep(28.0, 45.0, slope_deg)
        };
        (b * a * s).clamp(0.0, 1.0)
    }
}

/// The highest point of the terrain (m).
pub fn top(t: &crate::terrain::Terrain) -> f32 {
    t.heights.iter().copied().fold(t.sea_level + 1.0, f32::max)
}

/// How high a point lies: 0 at sea level, 1 on the highest peak (`top`: `ores::top`).
pub fn altitude_with(t: &crate::terrain::Terrain, p: Vec2, top: f32) -> f32 {
    ((t.height_at(p.x, p.y) - t.sea_level) / (top - t.sea_level).max(1.0)).clamp(0.0, 1.0)
}

pub fn altitude(t: &crate::terrain::Terrain, p: Vec2) -> f32 {
    altitude_with(t, p, top(t))
}

/// The ore tier a zone's level range calls for (1 for levels 1–10, … 5 from level 41).
pub fn tier_for_levels(l: [u8; 2]) -> u8 {
    super::edit::tier_for_levels(l).min(5)
}

/// Picks the ore for a mining node scattered at a spot: ores of the zone's tier first (lower
/// tiers stay around, the next tier up is rare), weighted by how well each suits the ground.
/// `roll` in 0..1 makes it deterministic.
pub fn pick_for_spot(
    zone_tier: u8,
    biome: Option<BiomePreset>,
    alt: f32,
    slope_deg: f32,
    roll: f32,
) -> Ore {
    if let Some(o) = pick_with(
        &OreSettings::default(),
        Some(zone_tier),
        biome,
        alt,
        slope_deg,
        roll,
    ) {
        return o;
    }
    pick_for_spot_old(zone_tier, biome, alt, slope_deg, roll)
}

fn pick_for_spot_old(
    zone_tier: u8,
    biome: Option<BiomePreset>,
    alt: f32,
    slope_deg: f32,
    roll: f32,
) -> Ore {
    let z = zone_tier.clamp(1, 5) as i32;
    let w: Vec<(Ore, f32)> = Ore::ALL
        .into_iter()
        .filter(|o| o.kind() == ResourceKind::Ore)
        .map(|o| {
            let d = o.tier() as i32 - z;
            let tier_w = match d {
                0 => 1.0,
                -1 => 0.4,
                1 => 0.07,
                d if d < 0 => 0.12,
                _ => 0.0,
            } * if o.tier() == 5 { 0.6 } else { 1.0 };
            (o, tier_w * (0.05 + o.affinity(biome, alt, slope_deg)))
        })
        .filter(|(_, w)| *w > 0.0)
        .collect();
    let total: f32 = w.iter().map(|x| x.1).sum();
    let mut r = roll.clamp(0.0, 0.9999) * total;
    for (o, wt) in &w {
        if r < *wt {
            return *o;
        }
        r -= wt;
    }
    w.last().map(|x| x.0).unwrap_or(Ore::Copper)
}

// ------------------------------------------------------------------ the world's ore mix

/// How much ore a scatter makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Abundance {
    Sparse,
    #[default]
    Normal,
    Rich,
}

impl Abundance {
    pub const ALL: [Abundance; 3] = [Abundance::Sparse, Abundance::Normal, Abundance::Rich];
    pub fn name(self) -> &'static str {
        match self {
            Abundance::Sparse => "Sparse",
            Abundance::Normal => "Normal",
            Abundance::Rich => "Rich",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Abundance::Sparse => "sparse",
            Abundance::Normal => "normal",
            Abundance::Rich => "rich",
        }
    }
    /// × the scatter's ore density
    pub fn factor(self) -> f32 {
        match self {
            Abundance::Sparse => 0.45,
            Abundance::Normal => 1.0,
            Abundance::Rich => 2.2,
        }
    }
}

/// Share of the deposits per tier (%), common … legendary.
pub const DEFAULT_MIX: [f32; 5] = [60.0, 25.0, 10.0, 4.0, 1.0];

fn yes() -> bool {
    true
}
fn default_mix() -> [f32; 5] {
    DEFAULT_MIX
}

/// How ore fills the world: "Scatter ore", and new worlds when `on_generate`. Saved with the
/// world (a new world keeps the previous world's choice).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OreSettings {
    /// fill the world with ore when a world is generated
    #[serde(default = "yes")]
    pub on_generate: bool,
    #[serde(default)]
    pub abundance: Abundance,
    /// only on mountainous ground (high, steep or alpine)
    #[serde(default)]
    pub mountains_only: bool,
    /// share of the deposits outside zones per tier (%); inside a zone its level range decides
    #[serde(default = "default_mix")]
    pub tier_mix: [f32; 5],
    /// ores that never appear
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off: Vec<Ore>,
    /// how often an ore appears within its tier (× 1 when not listed)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weights: Vec<(Ore, f32)>,
}

impl Default for OreSettings {
    fn default() -> Self {
        Self {
            on_generate: true,
            abundance: Abundance::Normal,
            mountains_only: false,
            tier_mix: DEFAULT_MIX,
            off: vec![],
            weights: vec![],
        }
    }
}

impl OreSettings {
    pub fn weight(&self, o: Ore) -> f32 {
        if self.off.contains(&o) {
            return 0.0;
        }
        self.weights
            .iter()
            .find(|w| w.0 == o)
            .map(|w| w.1.max(0.0))
            .unwrap_or(1.0)
    }
    pub fn set_weight(&mut self, o: Ore, w: f32) {
        self.weights.retain(|x| x.0 != o);
        if (w - 1.0).abs() > 1e-3 {
            self.weights.push((o, w.max(0.0)));
        }
    }
    pub fn enabled(&self, o: Ore) -> bool {
        !self.off.contains(&o)
    }
    pub fn set_enabled(&mut self, o: Ore, on: bool) {
        self.off.retain(|x| *x != o);
        if !on {
            self.off.push(o);
        }
    }
    /// The tier shares as fractions (summing to 1).
    pub fn mix(&self) -> [f32; 5] {
        let total: f32 = self.tier_mix.iter().map(|v| v.max(0.0)).sum();
        if total <= 0.0 {
            return [0.2; 5];
        }
        self.tier_mix.map(|v| v.max(0.0) / total)
    }
}

/// Mountainous ground for "mountains only": high up, steep and fairly high, or alpine.
pub fn mountainous(biome: Option<BiomePreset>, alt: f32, slope_deg: f32) -> bool {
    alt >= 0.45
        || (alt >= 0.22 && slope_deg >= 18.0)
        || (matches!(biome, Some(BiomePreset::Alpine)) && alt >= 0.12)
}

/// Picks the ore for a scattered deposit. Inside a zone (`zone_tier`) its level range leads:
/// ores of that tier first, lower tiers stay around, the next tier up is rare. In the wilderness
/// the tier comes from the world's mix, rarer tiers leaning towards high, steep and harsh ground.
/// Within a tier, ores that suit the ground (iron and coal in hills, mithril on peaks…) are more
/// likely, times the designer's weights. `roll` in 0..1 makes it deterministic; `None` when every
/// ore that could go here is switched off.
pub fn pick_with(
    st: &OreSettings,
    zone_tier: Option<u8>,
    biome: Option<BiomePreset>,
    alt: f32,
    slope_deg: f32,
    roll: f32,
) -> Option<Ore> {
    let tier_w: [f32; 5] = match zone_tier {
        Some(z) => {
            let z = z.clamp(1, 5) as i32;
            std::array::from_fn(|i| {
                let d = i as i32 + 1 - z;
                (match d {
                    0 => 1.0,
                    -1 => 0.4,
                    1 => 0.07,
                    d if d < 0 => 0.12,
                    _ => 0.0,
                }) * if i == 4 { 0.6 } else { 1.0 }
            })
        }
        None => {
            let harsh = (alt * 1.3
                + if slope_deg > 25.0 { 0.25 } else { 0.0 }
                + if matches!(
                    biome,
                    Some(
                        BiomePreset::Volcanic
                            | BiomePreset::Alpine
                            | BiomePreset::Tundra
                            | BiomePreset::HauntedWoods
                    )
                ) {
                    0.3
                } else {
                    0.0
                })
            .min(1.6);
            let mix = st.mix();
            std::array::from_fn(|i| {
                let k = i as f32 / 4.0;
                mix[i] * (1.0 + (0.3 + harsh - 1.0) * k)
            })
        }
    };
    let mut w: Vec<(Ore, f32)> = vec![];
    for t in 1..=5u8 {
        if tier_w[t as usize - 1] <= 0.0 {
            continue;
        }
        let row: Vec<(Ore, f32)> = Ore::of_tier(t)
            .filter(|o| o.kind() == ResourceKind::Ore)
            .map(|o| (o, (0.05 + o.affinity(biome, alt, slope_deg)) * st.weight(o)))
            .filter(|x| x.1 > 0.0)
            .collect();
        let sum: f32 = row.iter().map(|x| x.1).sum();
        if sum <= 0.0 {
            continue;
        }
        w.extend(
            row.into_iter()
                .map(|(o, x)| (o, tier_w[t as usize - 1] * x / sum)),
        );
    }
    let total: f32 = w.iter().map(|x| x.1).sum();
    if total <= 0.0 {
        return None;
    }
    let mut r = roll.clamp(0.0, 0.9999) * total;
    for (o, wt) in &w {
        if r < *wt {
            return Some(*o);
        }
        r -= wt;
    }
    w.last().map(|x| x.0)
}

// ------------------------------------------------------------------ deposit areas

/// One row of an area's ore table.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OreEntry {
    pub ore: Ore,
    pub weight: f32,
}

/// An outline on the ground where ore deposits are scattered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OreArea {
    pub id: u32,
    pub name: String,
    /// Outline in world xz (closed; at least 3 points).
    pub polygon: Vec<[f32; 2]>,
    /// Which ores, how often (weights).
    pub table: Vec<OreEntry>,
    /// Deposits per hectare (10 000 m²) of suitable ground.
    pub density: f32,
    pub respawn_secs: f32,
    pub seed: u32,
    #[serde(default)]
    pub notes: String,
}

/// Deposits never stand closer than this (m).
pub const MIN_SPACING: f32 = 3.5;

impl OreArea {
    pub fn centroid(&self) -> Option<Vec2> {
        (!self.polygon.is_empty()).then(|| {
            self.polygon
                .iter()
                .fold(Vec2::ZERO, |a, p| a + Vec2::new(p[0], p[1]))
                / self.polygon.len() as f32
        })
    }
    pub fn area(&self) -> f32 {
        polygon_area(&self.polygon)
    }
    pub fn contains(&self, p: Vec2) -> bool {
        point_in_polygon(&self.polygon, p)
    }
    pub fn translate(&mut self, d: Vec2) {
        for p in &mut self.polygon {
            p[0] += d.x;
            p[1] += d.y;
        }
    }
    /// Share of each table row (weights normalised).
    pub fn chances(&self) -> Vec<f32> {
        let total: f32 = self.table.iter().map(|e| e.weight.max(0.0)).sum();
        self.table
            .iter()
            .map(|e| {
                if total > 0.0 {
                    e.weight.max(0.0) / total
                } else {
                    0.0
                }
            })
            .collect()
    }
    /// The ore with the most weight.
    pub fn main_ore(&self) -> Option<Ore> {
        self.table
            .iter()
            .filter(|e| e.weight > 0.0)
            .max_by(|a, b| a.weight.total_cmp(&b.weight))
            .map(|e| e.ore)
    }
    /// "Iron 75%, Coal 25%".
    pub fn mix_label(&self) -> String {
        let ch = self.chances();
        let parts: Vec<String> = self
            .table
            .iter()
            .zip(ch)
            .filter(|(_, c)| *c > 0.0)
            .map(|(e, c)| format!("{} {:.0}%", e.ore.name(), c * 100.0))
            .collect();
        if parts.is_empty() {
            "no ores".into()
        } else {
            parts.join(", ")
        }
    }
}

/// A new area's default density (deposits per hectare): rarer ores sit further apart.
pub fn default_density(ore: Ore) -> f32 {
    [60.0, 48.0, 36.0, 28.0, 20.0][(ore.tier().clamp(1, 5) - 1) as usize]
}

/// The default ore table for an area of `ore`: it, plus its usual companion at a quarter of the
/// weight.
pub fn default_table(ore: Ore) -> Vec<OreEntry> {
    let mut t = vec![OreEntry { ore, weight: 1.0 }];
    if let Some(c) = ore.companion() {
        t.push(OreEntry {
            ore: c,
            weight: 0.33,
        });
    }
    t
}

/// What the deposit scatter checks the ground against.
pub struct Site<'a> {
    pub g: &'a Ground<'a>,
    pub store: Option<&'a ObjectStore>,
    pub lib: Option<&'a Library>,
    pub roads: Option<&'a Roads>,
}

/// Road centrelines near a rectangle, as (segment start, end, half width + margin).
fn road_segments(roads: Option<&Roads>, lo: Vec2, hi: Vec2) -> Vec<(Vec2, Vec2, f32)> {
    let mut out = vec![];
    for r in roads.map(|r| r.roads.as_slice()).unwrap_or_default() {
        let reach = r.width * 0.5 + 2.5;
        let pts: Vec<Vec2> = r.centreline(4.0).iter().map(|p| p.xz()).collect();
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a.min(b).x - reach > hi.x
                || a.max(b).x + reach < lo.x
                || a.min(b).y - reach > hi.y
                || a.max(b).y + reach < lo.y
            {
                continue;
            }
            out.push((a, b, reach));
        }
    }
    out
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

impl Site<'_> {
    /// Can a deposit of `ore` stand at `p`? (`roads`: the segments near the area)
    fn fits(&self, p: Vec2, ore: Ore, roads: &[(Vec2, Vec2, f32)]) -> bool {
        let t = self.g.terrain;
        if self.g.water_depth(p) > 0.05 || t.height_at(p.x, p.y) < t.sea_level + 0.3 {
            return false;
        }
        let max_slope = if ore.habitat().cliffs { 55.0 } else { 38.0 };
        if self.g.slope_deg(p) > max_slope {
            return false;
        }
        // paving
        let gi = t.world_to_grid(p.x, p.y).round();
        let (i, j) = (
            (gi.x.max(0.0) as usize).min(t.res),
            (gi.y.max(0.0) as usize).min(t.res),
        );
        if t.final_weights(i, j)[Layer::Cobblestone as usize] > 0.35 {
            return false;
        }
        if roads
            .iter()
            .any(|(a, b, reach)| seg_dist(p, *a, *b) < *reach)
        {
            return false;
        }
        // buildings, props, trees and rocks already there
        if let (Some(store), Some(lib)) = (self.store, self.lib) {
            for id in store.query(p, 14.0) {
                let Some(o) = store.items.get(&id) else {
                    continue;
                };
                let Some(e) = lib.get(&o.asset) else { continue };
                if matches!(e.category, Category::Plants | Category::Debris) {
                    continue;
                }
                let r = if e.category == Category::Trees {
                    0.8
                } else {
                    e.radius * o.scale
                };
                if Vec2::new(o.pos[0], o.pos[2]).distance(p) < r + 1.6 {
                    return false;
                }
            }
        }
        true
    }
}

/// Where an area's deposits go, and of which ore: one candidate per cell of a world-anchored grid
/// (a few tries per cell on unsuitable ground), deterministic per area seed.
pub fn plan_area(a: &OreArea, site: &Site) -> Vec<(Vec2, Ore)> {
    let total: f32 = a.table.iter().map(|e| e.weight.max(0.0)).sum();
    if a.polygon.len() < 3 || total <= 0.0 || a.density <= 0.0 {
        return vec![];
    }
    let (lo, hi) = SpawnRegion::bounds_of(&a.polygon);
    let cell = (1.0e4 / a.density.max(0.01))
        .sqrt()
        .clamp(MIN_SPACING, 400.0);
    let roads = road_segments(site.roads, lo, hi);
    let mut out: Vec<(Vec2, Ore)> = vec![];
    let mut grid: HashMap<(i32, i32), Vec<Vec2>> = HashMap::new();
    let key = |p: Vec2| {
        (
            (p.x / MIN_SPACING).floor() as i32,
            (p.y / MIN_SPACING).floor() as i32,
        )
    };
    let (i0, i1) = ((lo.x / cell).floor() as i32, (hi.x / cell).floor() as i32);
    let (j0, j1) = ((lo.y / cell).floor() as i32, (hi.y / cell).floor() as i32);
    for j in j0..=j1 {
        for i in i0..=i1 {
            let mut rng = Rng::new(
                hash2(i, j, a.seed.wrapping_mul(0x9E37_79B9) ^ 0x0de9_051d) as u64
                    | ((a.seed as u64) << 32),
            );
            // the ore of this cell
            let mut r = rng.f32() * total;
            let mut ore = a.table[0].ore;
            for e in &a.table {
                if r < e.weight.max(0.0) {
                    ore = e.ore;
                    break;
                }
                r -= e.weight.max(0.0);
            }
            for _ in 0..3 {
                let p = Vec2::new(
                    (i as f32 + rng.range(0.12, 0.88)) * cell,
                    (j as f32 + rng.range(0.12, 0.88)) * cell,
                );
                if !a.contains(p) || !site.fits(p, ore, &roads) {
                    continue;
                }
                let (kx, kz) = key(p);
                let crowded = (-1..=1).any(|dz| {
                    (-1..=1).any(|dx| {
                        grid.get(&(kx + dx, kz + dz))
                            .is_some_and(|v| v.iter().any(|q| q.distance(p) < MIN_SPACING))
                    })
                });
                if crowded {
                    continue;
                }
                grid.entry((kx, kz)).or_default().push(p);
                out.push((p, ore));
                break;
            }
        }
    }
    out
}

/// Re-scatters an area's deposits (after any edit of it). Deposits that stay where they were
/// keep their ids. Returns how many it now has.
pub fn rescatter(gp: &mut Gameplay, id: u32, site: &Site) -> usize {
    let Some(a) = gp.ore_areas.iter().find(|a| a.id == id).cloned() else {
        return 0;
    };
    let old: HashMap<(u32, u32), u32> = gp
        .nodes
        .iter()
        .filter(|n| n.area == Some(id))
        .map(|n| ((n.pos[0].to_bits(), n.pos[2].to_bits()), n.id))
        .collect();
    gp.nodes.retain(|n| n.area != Some(id));
    let planned = plan_area(&a, site);
    let n = planned.len();
    for (p, ore) in planned {
        let id_n = match old.get(&(p.x.to_bits(), p.y.to_bits())) {
            Some(k) => *k,
            None => gp.alloc_id(),
        };
        gp.nodes.push(deposit_node(
            id_n,
            ore,
            site.g.at(p),
            a.respawn_secs,
            Some(id),
        ));
    }
    if gp.selected.is_some_and(|s| !gp.exists(s)) {
        gp.selected = None;
    }
    gp.dirty = true;
    n
}

/// A deposit as a resource node.
pub fn deposit_node(
    id: u32,
    ore: Ore,
    at: Vec3,
    respawn_secs: f32,
    area: Option<u32>,
) -> ResourceNode {
    ResourceNode {
        id,
        kind: ore.kind(),
        variant: ore.deposit_name(),
        tier: ore.tier(),
        pos: at.to_array(),
        respawn_secs,
        auto: false,
        ore: Some(ore),
        area,
    }
}

/// Places one deposit of `ore` at `p` by hand.
pub fn add_deposit(gp: &mut Gameplay, g: &Ground, ore: Ore, p: Vec2) -> u32 {
    let id = gp.alloc_id();
    gp.nodes
        .push(deposit_node(id, ore, g.at(p), ore.default_respawn(), None));
    gp.dirty = true;
    id
}

/// Adds a deposit area (at least 3 corners, ~100 m²) and scatters its deposits.
pub fn add_area(
    gp: &mut Gameplay,
    site: &Site,
    polygon: Vec<[f32; 2]>,
    table: Vec<OreEntry>,
    name: Option<String>,
) -> Result<u32, String> {
    if polygon.len() < 3 {
        return Err("A deposit area needs at least 3 corners".into());
    }
    if polygon_area(&polygon) < 100.0 {
        return Err("The area is too small: draw at least ~10 × 10 m".into());
    }
    let main = table
        .iter()
        .filter(|e| e.weight > 0.0)
        .max_by(|a, b| a.weight.total_cmp(&b.weight))
        .map(|e| e.ore)
        .ok_or("The ore table is empty")?;
    let id = gp.alloc_id();
    let base = format!("{} Deposits", main.name());
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| unique_name(gp.ore_areas.iter().map(|a| a.name.as_str()), &base));
    gp.ore_areas.push(OreArea {
        id,
        name,
        polygon,
        table,
        density: default_density(main),
        respawn_secs: main.default_respawn(),
        seed: id,
        notes: String::new(),
    });
    rescatter(gp, id, site);
    Ok(id)
}

/// Removes an area and its deposits. Returns its name.
pub fn remove_area(gp: &mut Gameplay, id: u32) -> Option<String> {
    let i = gp.ore_areas.iter().position(|a| a.id == id)?;
    gp.nodes.retain(|n| n.area != Some(id));
    Some(gp.ore_areas.remove(i).name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ore sits at its index, with a unique id and name, a tier matching the user's table
    /// and sensible look values.
    #[test]
    fn table_integrity() {
        assert_eq!(DEFS.len(), 25);
        let mut ids = std::collections::HashSet::new();
        for (i, d) in DEFS.iter().enumerate() {
            let o = Ore::ALL[i];
            assert_eq!(o as usize, i);
            assert_eq!(
                format!("{o:?}").to_lowercase(),
                d.name.replace(' ', "").to_lowercase(),
                "{} out of order",
                d.name
            );
            assert!(ids.insert(d.id), "duplicate id {}", d.id);
            assert_eq!(
                serde_json::to_value(o).unwrap(),
                serde_json::json!(d.id),
                "serde id of {}",
                d.name
            );
            assert_eq!(Ore::parse(d.id), Some(o));
            assert_eq!(Ore::parse(d.name), Some(o));
            assert!(!d.uses.is_empty() && !d.habitat.about.is_empty());
            let l = &d.look;
            assert!(
                (0.0..=1.0).contains(&l.metallic)
                    && (0.02..=1.0).contains(&l.roughness)
                    && (0.0..=1.0).contains(&l.veins)
                    && (0.0..=1.0).contains(&l.nuggets)
            );
            assert!(
                d.habitat.alt.0 < d.habitat.alt.1 && (0.0..=1.0).contains(&d.habitat.alt.0),
                "{}: habitat band",
                d.name
            );
            // magical ores glow, plain metals don't; the glowing tiers carry the glow colour
            assert_eq!(
                l.glow_nits > 0.0,
                l.glow != NO,
                "{}: glow colour vs strength",
                d.name
            );
        }
        for t in 1..=5 {
            assert_eq!(Ore::of_tier(t).count(), 5, "tier {t}");
        }
        let names: Vec<&str> = Ore::of_tier(4).map(|o| o.name()).collect();
        assert_eq!(
            names,
            [
                "Obsidian",
                "Dark Iron",
                "Elementium",
                "Truesilver",
                "Runite"
            ]
        );
        assert_eq!(Ore::Dragonsteel.rarity(), Rarity::Legendary);
        assert_eq!(Ore::Coal.uses(), "Steel smelting");
        assert_eq!(Ore::Stone.kind(), ResourceKind::Stone);
        assert_eq!(Ore::parse("dark iron"), Some(Ore::DarkIron));
        // companions are real neighbours in the table: never more than a tier apart
        for o in Ore::ALL {
            if let Some(c) = o.companion() {
                assert!(c != o && o.tier().abs_diff(c.tier()) <= 1, "{o:?} → {c:?}");
            }
        }
    }

    /// Ores keep to their habitats: mithril high in the alps, obsidian and dragonsteel in
    /// volcanic land, voidstone in haunted woods; a zone's tier picks that tier's ores.
    #[test]
    fn habitats() {
        assert!(Ore::Mithril.affinity(Some(B::Alpine), 0.85, 30.0) > 0.8);
        assert!(Ore::Mithril.affinity(Some(B::Meadow), 0.1, 5.0) < 0.1);
        assert!(
            Ore::Obsidian.affinity(Some(B::Volcanic), 0.5, 20.0)
                > Ore::Obsidian.affinity(Some(B::Meadow), 0.5, 20.0) * 5.0
        );
        assert!(Ore::Voidstone.affinity(Some(B::HauntedWoods), 0.3, 10.0) > 0.9);
        let mut counts: HashMap<Ore, usize> = HashMap::new();
        for k in 0..1000 {
            *counts
                .entry(pick_for_spot(
                    5,
                    Some(B::Volcanic),
                    0.85,
                    30.0,
                    k as f32 / 1000.0,
                ))
                .or_default() += 1;
        }
        let top = counts
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(o, _)| *o)
            .unwrap();
        assert_eq!(top, Ore::Dragonsteel, "{counts:?}");
        for k in 0..100 {
            let o = pick_for_spot(1, Some(B::Meadow), 0.2, 10.0, k as f32 / 100.0);
            assert!(
                o.tier() <= 2 && o != Ore::Stone,
                "{o:?} in a starter meadow"
            );
        }
    }

    /// Mining nodes saved before ores existed get the ore their name or tier suggests.
    #[test]
    fn legacy_nodes_map_to_ores() {
        let json = r#"{"id":7,"kind":"Ore","variant":"Adamant Vein","tier":8,"pos":[1.0,2.0,3.0],"respawn_secs":600.0,"auto":true}"#;
        let n: ResourceNode = serde_json::from_str(json).unwrap();
        assert_eq!(n.ore, None);
        assert_eq!(n.area, None);
        assert_eq!(n.ore(), Some(Ore::Adamantite));
        assert_eq!(Ore::from_legacy("Copper Vein", 1), Ore::Copper);
        assert_eq!(Ore::from_legacy("Truesilver Lode", 3), Ore::Truesilver);
        assert_eq!(Ore::from_legacy("Strange Rock", 4), Ore::Silver);
        let herb = ResourceNode {
            kind: ResourceKind::Herb,
            variant: "Silverleaf".into(),
            ..n.clone()
        };
        assert_eq!(herb.ore(), None, "only mining nodes are ores");
        // old worlds without areas load, and new fields stay out of old-style nodes' JSON
        let gp: Gameplay = serde_json::from_str(r#"{"nodes":[{"id":1,"kind":"Herb","variant":"Sage","tier":1,"pos":[0,0,0],"respawn_secs":1}]}"#).unwrap();
        assert!(gp.ore_areas.is_empty());
        let v = serde_json::to_value(&gp.nodes[0]).unwrap();
        assert!(v.get("ore").is_none() && v.get("area").is_none());
    }

    fn flat_world() -> (
        crate::terrain::Terrain,
        crate::world::Zones,
        crate::water::WaterBodies,
    ) {
        let mut t = crate::terrain::Terrain::new(128, 4.0, 0.0);
        for h in t.heights.iter_mut() {
            *h = 20.0;
        }
        (
            t,
            crate::world::Zones::default(),
            crate::water::WaterBodies::default(),
        )
    }

    /// The same area and seed always give the same deposits, all inside the outline and apart;
    /// another seed gives others; moving one corner keeps the deposits away from it.
    #[test]
    fn deterministic_scatter_inside_outline() {
        let (t, z, w) = flat_world();
        let g = Ground {
            terrain: &t,
            zones: &z,
            water: &w,
            nature: None,
        };
        let site = Site {
            g: &g,
            store: None,
            lib: None,
            roads: None,
        };
        let poly = vec![
            [-60.0, -40.0],
            [70.0, -50.0],
            [80.0, 45.0],
            [-10.0, 70.0],
            [-70.0, 20.0],
        ];
        let a = OreArea {
            id: 1,
            name: "t".into(),
            polygon: poly.clone(),
            table: default_table(Ore::Iron),
            density: 30.0,
            respawn_secs: 300.0,
            seed: 4,
            notes: String::new(),
        };
        let first = plan_area(&a, &site);
        let again = plan_area(&a, &site);
        assert_eq!(first, again);
        let ha = a.area() / 1.0e4;
        assert!(
            first.len() as f32 > ha * 30.0 * 0.6 && (first.len() as f32) < ha * 30.0 * 1.3,
            "{} deposits on {ha:.2} ha",
            first.len()
        );
        for (p, _) in &first {
            assert!(point_in_polygon(&poly, *p));
        }
        for (i, (p, _)) in first.iter().enumerate() {
            for (q, _) in &first[i + 1..] {
                assert!(p.distance(*q) >= MIN_SPACING - 1e-3);
            }
        }
        let iron = first.iter().filter(|(_, o)| *o == Ore::Iron).count();
        let coal = first.iter().filter(|(_, o)| *o == Ore::Coal).count();
        assert!(iron > coal * 2 && coal > 0, "iron {iron}, coal {coal}");
        let other = plan_area(
            &OreArea {
                seed: 5,
                ..a.clone()
            },
            &site,
        );
        assert_ne!(first, other);
        // drag one corner: deposits far from it stay put
        let mut moved = a.clone();
        moved.polygon[2] = [95.0, 60.0];
        let after = plan_area(&moved, &site);
        let far: Vec<_> = first
            .iter()
            .filter(|(p, _)| p.distance(Vec2::new(-60.0, 0.0)) < 30.0)
            .collect();
        assert!(!far.is_empty() && far.iter().all(|d| after.contains(d)));
    }

    /// No deposits in water or on roads; the area's records follow edits and removal.
    #[test]
    fn scatter_avoids_water_and_roads_and_rescatters() {
        let (mut t, z, w) = flat_world();
        // a channel below sea level along x = 0
        for j in 0..=t.res {
            for i in 0..=t.res {
                let x = i as f32 * t.cell - t.half();
                if x.abs() < 10.0 {
                    t.heights[j * (t.res + 1) + i] = -3.0;
                }
            }
        }
        let mut roads = Roads::default();
        roads.roads.push(crate::roads::Road::new(
            1,
            crate::roads::RoadKind::DirtRoad,
            vec![[-100.0, 20.0, 30.0], [100.0, 20.0, 30.0]],
            6.0,
            default(),
        ));
        let g = Ground {
            terrain: &t,
            zones: &z,
            water: &w,
            nature: None,
        };
        let site = Site {
            g: &g,
            store: None,
            lib: None,
            roads: Some(&roads),
        };
        let mut gp = Gameplay::default();
        let poly = vec![[-80.0, -60.0], [80.0, -60.0], [80.0, 60.0], [-80.0, 60.0]];
        let id = add_area(&mut gp, &site, poly, default_table(Ore::Gold), None).unwrap();
        let deps: Vec<&ResourceNode> = gp.nodes.iter().filter(|n| n.area == Some(id)).collect();
        assert!(deps.len() > 10);
        for n in &deps {
            assert!(n.pos[0].abs() >= 10.0 - 1e-3, "in the water at {:?}", n.pos);
            assert!(
                (n.pos[2] - 30.0).abs() > 3.0 + 2.5 - 0.1,
                "on the road at {:?}",
                n.pos
            );
            assert!(matches!(n.ore, Some(Ore::Gold | Ore::Silver)));
        }
        let before: Vec<(u32, [f32; 3])> = deps.iter().map(|n| (n.id, n.pos)).collect();
        // one corner moved: the deposits far from it keep their places and ids
        gp.ore_areas[0].polygon[2] = [95.0, 75.0];
        rescatter(&mut gp, id, &site);
        let far: Vec<&(u32, [f32; 3])> = before
            .iter()
            .filter(|(_, p)| Vec2::new(p[0], p[2]).distance(Vec2::new(-80.0, -60.0)) < 45.0)
            .collect();
        assert!(
            !far.is_empty()
                && far
                    .iter()
                    .all(|(i, p)| gp.nodes.iter().any(|n| n.id == *i && n.pos == *p))
        );
        // denser: more deposits
        gp.ore_areas[0].density *= 2.0;
        assert!(rescatter(&mut gp, id, &site) > before.len());
        assert_eq!(remove_area(&mut gp, id).as_deref(), Some("Gold Deposits"));
        assert!(gp.nodes.is_empty() && gp.ore_areas.is_empty());
    }
}
