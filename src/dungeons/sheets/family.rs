//! A sheet's setting family (temperate, arid, cold, tropical, volcanic, twilight): the realm's
//! climate, biome, look and hour, the materials its floors are paved with, and the temple kit
//! (`compound::theme`) its stair rails, portal and creatures come from.

use crate::biomegen::Climate;
use crate::biomes::BiomePreset as B;
use crate::dungeons::compound::Theme;
use crate::dungeons::presets::RealmPreset;
use crate::look::LookPreset as L;
use crate::terrain::layers::Layer;
use crate::terrain::worldgen::WorldPreset as W;
use egui_phosphor::regular as icon;

pub const FAMILIES: [&str; 6] = [
    "temperate",
    "arid",
    "cold",
    "tropical",
    "volcanic",
    "twilight",
];

static PRESETS: [RealmPreset; 6] = [
    RealmPreset {
        id: "sheet_temperate",
        name: "Temperate",
        blurb: "",
        world: W::Island,
        climate: Climate::Temperate,
        look: L::Natural,
        hour: 15.5,
        only: Some(B::AutumnWoods),
        icon: icon::TREE,
    },
    RealmPreset {
        id: "sheet_arid",
        name: "Arid",
        blurb: "",
        world: W::Island,
        climate: Climate::Arid,
        look: L::GoldenHour,
        hour: 16.0,
        only: Some(B::Desert),
        icon: icon::SUN_HORIZON,
    },
    RealmPreset {
        id: "sheet_cold",
        name: "Cold",
        blurb: "",
        world: W::Island,
        climate: Climate::Cold,
        look: L::Nordic,
        hour: 11.5,
        only: Some(B::Tundra),
        icon: icon::SNOWFLAKE,
    },
    RealmPreset {
        id: "sheet_tropical",
        name: "Tropical",
        blurb: "",
        world: W::Island,
        climate: Climate::Tropical,
        look: L::EpicSunbeams,
        hour: 10.5,
        only: Some(B::Jungle),
        icon: icon::TREE_PALM,
    },
    RealmPreset {
        id: "sheet_volcanic",
        name: "Volcanic",
        blurb: "",
        world: W::Island,
        climate: Climate::Arid,
        look: L::GoldenHour,
        hour: 16.6,
        only: Some(B::Volcanic),
        icon: icon::FIRE,
    },
    RealmPreset {
        id: "sheet_twilight",
        name: "Twilight",
        blurb: "",
        world: W::Island,
        climate: Climate::Temperate,
        look: L::Lavender,
        hour: 17.5,
        only: Some(B::AutumnWoods),
        icon: icon::MOON_STARS,
    },
];

fn index(family: &str) -> usize {
    FAMILIES.iter().position(|f| *f == family).unwrap_or(0)
}

/// The realm a family's sheets are made in (its sky, biome and music).
pub fn preset(family: &str) -> &'static RealmPreset {
    &PRESETS[index(family)]
}

/// The temple kit its rails, portal and creatures come from.
pub fn theme(family: &str) -> Theme {
    match family {
        "arid" => Theme::Desert,
        "cold" => Theme::Frozen,
        "tropical" => Theme::Jungle,
        "volcanic" => Theme::Ember,
        _ => Theme::Twilight,
    }
}

/// The layers its floors, rings and stairs, and paths are painted with.
pub fn paint(family: &str) -> super::land::Paint {
    let path = match family {
        "arid" => Layer::Sand,
        "cold" => Layer::Gravel,
        "volcanic" => Layer::Gravel,
        _ => Layer::Dirt,
    };
    super::land::Paint {
        floor: Layer::Cobblestone,
        accent: Layer::Ice,
        path,
    }
}

/// The lake its water is made as.
pub fn lake(family: &str) -> &'static str {
    match family {
        "arid" => "oasis",
        "cold" => "frozen_lake",
        "tropical" => "tropical_lagoon",
        "volcanic" => "volcanic_lake",
        "twilight" => "mirror_lake",
        _ => "forest_lake",
    }
}

/// The hour of day the sheet's light describes ("late afternoon, sun from the north-west…").
pub fn hour(light: &str, family: &str) -> f32 {
    let l = light.to_ascii_lowercase();
    let table: [(&str, f32); 12] = [
        ("eclipse", 13.0),
        ("night", 22.5),
        ("midnight", 23.5),
        ("dawn", 6.6),
        ("sunrise", 6.8),
        ("early morning", 7.8),
        ("late morning", 10.5),
        ("late afternoon", 16.6),
        ("dusk", 18.4),
        ("twilight", 18.6),
        ("sunset", 18.2),
        ("golden", 17.0),
    ];
    for (w, h) in table {
        if l.contains(w) {
            return h;
        }
    }
    for (w, h) in [
        ("morning", 9.5),
        ("noon", 12.5),
        ("midday", 12.5),
        ("afternoon", 15.0),
        ("evening", 18.0),
    ] {
        if l.contains(w) {
            return h;
        }
    }
    preset(family).hour
}
