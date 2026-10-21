//! The fish species: 23 from the user's Emberfall game (models + paintings in
//! `assets/models/fish/`) and four made for Skyfold Studio (salmon, cod, tuna, reef fish). Saved
//! worlds store the variant name, so variants are only ever added.

use serde::{Deserialize, Serialize};

/// Fresh water, the sea, or both (estuaries, sea-run fish).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Water {
    Fresh,
    Sea,
    Both,
}

impl Water {
    pub fn suits(self, fresh: bool) -> bool {
        match self {
            Water::Fresh => fresh,
            Water::Sea => !fresh,
            Water::Both => true,
        }
    }
}

/// How the body moves when it swims (the vertex shader's swim wave, `shaders/fish_swim.wesl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// a tail-driven wave, strongest at the tail
    Fish,
    /// a flatfish lying on its side: the wave runs up and down
    Flat,
    /// a ray: the wings flap, the whip tail trails
    Ray,
    /// an eel: the whole body undulates
    Eel,
}

impl Plan {
    pub fn code(self) -> f32 {
        match self {
            Plan::Fish => 0.0,
            Plan::Flat => 1.0,
            Plan::Ray => 2.0,
            Plan::Eel => 3.0,
        }
    }
}

pub struct Info {
    /// model and painting file stem in `assets/models/fish/`
    pub model: &'static str,
    pub name: &'static str,
    /// the real fish it looks like
    pub look: &'static str,
    pub habitat: &'static str,
    pub water: Water,
    /// common · uncommon · rare · prize · giant (Emberfall's tiers; exported with fishing spots)
    pub rarity: &'static str,
    pub plan: Plan,
    /// body length in the world (m; wingspan for the ray), a little larger than life so schools read
    pub length: f32,
    /// cruising speed (m/s)
    pub speed: f32,
    /// where it swims: 0 just under the surface … 1 along the bottom
    pub depth: f32,
    /// the name in Emberfall, for the ones that come from it
    pub emberfall: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FishSpecies {
    #[default]
    Trout,
    Salmon,
    Bass,
    Pike,
    Koi,
    ReefFish,
    Cod,
    Tuna,
    Sprat,
    Herring,
    Mullet,
    Perch,
    Bream,
    Wrasse,
    Mackerel,
    Snapper,
    Flounder,
    Conger,
    Grouper,
    Emberjack,
    Lancefin,
    Gloamwing,
    Blackwater,
    Sovereign,
    Drowner,
    Ironscale,
    RibbonKing,
}

#[allow(clippy::too_many_arguments)]
const fn info(
    model: &'static str,
    name: &'static str,
    look: &'static str,
    habitat: &'static str,
    water: Water,
    rarity: &'static str,
    plan: Plan,
    (length, speed, depth): (f32, f32, f32),
    emberfall: Option<&'static str>,
) -> Info {
    Info {
        model,
        name,
        look,
        habitat,
        water,
        rarity,
        plan,
        length,
        speed,
        depth,
        emberfall,
    }
}

impl FishSpecies {
    pub const ALL: [FishSpecies; 27] = [
        FishSpecies::Trout,
        FishSpecies::Salmon,
        FishSpecies::Bass,
        FishSpecies::Pike,
        FishSpecies::Koi,
        FishSpecies::Perch,
        FishSpecies::ReefFish,
        FishSpecies::Cod,
        FishSpecies::Tuna,
        FishSpecies::Sprat,
        FishSpecies::Herring,
        FishSpecies::Mullet,
        FishSpecies::Bream,
        FishSpecies::Wrasse,
        FishSpecies::Mackerel,
        FishSpecies::Snapper,
        FishSpecies::Flounder,
        FishSpecies::Conger,
        FishSpecies::Grouper,
        FishSpecies::Emberjack,
        FishSpecies::Lancefin,
        FishSpecies::Gloamwing,
        FishSpecies::Blackwater,
        FishSpecies::Sovereign,
        FishSpecies::Drowner,
        FishSpecies::Ironscale,
        FishSpecies::RibbonKing,
    ];

    pub fn info(self) -> &'static Info {
        use FishSpecies::*;
        use Plan::*;
        use Water::*;
        const T: [Info; 27] = [
            info(
                "trout",
                "Sea Trout",
                "sea-run brown trout",
                "Rivers, lakes & coasts",
                Both,
                "uncommon",
                Fish,
                (0.6, 1.0, 0.4),
                Some("Silver Trout"),
            ),
            info(
                "salmon",
                "Salmon",
                "Atlantic salmon",
                "Rivers & cold coasts",
                Both,
                "uncommon",
                Fish,
                (0.9, 1.3, 0.4),
                None,
            ),
            info(
                "kelpbass",
                "Bass",
                "largemouth bass",
                "Lakes, ponds & weed beds",
                Fresh,
                "uncommon",
                Fish,
                (0.6, 0.8, 0.5),
                Some("Kelpwood Bass"),
            ),
            info(
                "pike",
                "Pike",
                "northern pike",
                "Rivers & lakes",
                Fresh,
                "rare",
                Fish,
                (1.0, 0.9, 0.5),
                Some("Moonscale Pike"),
            ),
            info(
                "koi",
                "Koi",
                "koi carp",
                "Ponds & slow rivers",
                Fresh,
                "prize",
                Fish,
                (0.8, 0.5, 0.3),
                Some("Ember Koi"),
            ),
            info(
                "perch",
                "Perch",
                "European perch",
                "Rivers & lakes",
                Fresh,
                "common",
                Fish,
                (0.35, 0.7, 0.5),
                Some("River Perch"),
            ),
            info(
                "reeffish",
                "Reef Fish",
                "queen angelfish",
                "Tropical reefs",
                Sea,
                "common",
                Fish,
                (0.35, 0.6, 0.6),
                None,
            ),
            info(
                "cod",
                "Cod",
                "Atlantic cod",
                "Cold seas, near the bottom",
                Sea,
                "common",
                Fish,
                (0.9, 0.8, 0.8),
                None,
            ),
            info(
                "tuna",
                "Tuna",
                "Atlantic bluefin tuna",
                "Open sea",
                Sea,
                "rare",
                Fish,
                (1.8, 2.4, 0.4),
                None,
            ),
            info(
                "sprat",
                "Sprat",
                "sprat / anchovy",
                "Coastal shallows",
                Sea,
                "common",
                Fish,
                (0.2, 0.9, 0.15),
                Some("Glass Sprat"),
            ),
            info(
                "herring",
                "Herring",
                "Atlantic herring",
                "Coastal shallows",
                Sea,
                "common",
                Fish,
                (0.35, 0.9, 0.3),
                Some("Harbour Herring"),
            ),
            info(
                "mullet",
                "Mullet",
                "grey mullet",
                "Harbours & estuaries",
                Both,
                "common",
                Fish,
                (0.45, 0.7, 0.15),
                Some("Wharf Mullet"),
            ),
            info(
                "bream",
                "Sea Bream",
                "gilthead sea bream",
                "Weed beds & shallows",
                Sea,
                "common",
                Fish,
                (0.4, 0.6, 0.7),
                Some("Copper Bream"),
            ),
            info(
                "wrasse",
                "Wrasse",
                "ballan wrasse",
                "Rocky weed beds",
                Sea,
                "common",
                Fish,
                (0.32, 0.5, 0.85),
                Some("Greenback Wrasse"),
            ),
            info(
                "mackerel",
                "Mackerel",
                "Atlantic mackerel",
                "Channels & open sea",
                Sea,
                "uncommon",
                Fish,
                (0.45, 1.3, 0.3),
                Some("Barred Mackerel"),
            ),
            info(
                "snapper",
                "Snapper",
                "red snapper",
                "Reefs & wrecks",
                Sea,
                "uncommon",
                Fish,
                (0.65, 0.6, 0.75),
                Some("Wreck Snapper"),
            ),
            info(
                "flounder",
                "Flounder",
                "European flounder",
                "Sandy sea floor",
                Sea,
                "uncommon",
                Flat,
                (0.5, 0.4, 1.0),
                Some("Tide Flounder"),
            ),
            info(
                "conger",
                "Conger Eel",
                "conger eel",
                "Wrecks & channels",
                Sea,
                "rare",
                Eel,
                (1.4, 0.45, 0.95),
                Some("Wharf Conger"),
            ),
            info(
                "grouper",
                "Grouper",
                "dusky grouper",
                "Reefs & wrecks",
                Sea,
                "rare",
                Fish,
                (1.0, 0.45, 0.85),
                Some("Stormback Grouper"),
            ),
            info(
                "emberjack",
                "Ember Jack",
                "greater amberjack",
                "Reefs & open sea",
                Sea,
                "rare",
                Fish,
                (1.1, 1.6, 0.5),
                Some("Ember Jack"),
            ),
            info(
                "lancefin",
                "Lancefin",
                "sailfish",
                "Open sea",
                Sea,
                "rare",
                Fish,
                (1.8, 2.0, 0.25),
                Some("Lancefin"),
            ),
            info(
                "gloamwing",
                "Gloamwing",
                "great eagle ray",
                "Reefs & open sea",
                Sea,
                "prize",
                Ray,
                (2.2, 0.8, 0.7),
                Some("The Gloamwing"),
            ),
            info(
                "blackwater",
                "Blackwater Eel",
                "giant moray",
                "Wrecks",
                Sea,
                "prize",
                Eel,
                (2.6, 0.35, 1.0),
                Some("The Blackwater Eel"),
            ),
            info(
                "sovereign",
                "Sovereign",
                "giant tarpon",
                "River mouths & open sea",
                Both,
                "prize",
                Fish,
                (2.2, 1.2, 0.4),
                Some("The Harbour Sovereign"),
            ),
            info(
                "drowner",
                "Old Drowner",
                "giant sea bass",
                "Deep wrecks",
                Sea,
                "giant",
                Fish,
                (4.7, 0.35, 1.0),
                Some("The Old Drowner"),
            ),
            info(
                "ironscale",
                "Ironscale",
                "armoured sturgeon",
                "Deep rivers",
                Both,
                "giant",
                Fish,
                (4.2, 0.6, 1.0),
                Some("Grandfather Ironscale"),
            ),
            info(
                "ribbonking",
                "Ribbon King",
                "giant oarfish",
                "Deep open sea",
                Sea,
                "giant",
                Eel,
                (5.0, 0.5, 0.6),
                Some("The Ribbon King"),
            ),
        ];
        let i = match self {
            Trout => 0,
            Salmon => 1,
            Bass => 2,
            Pike => 3,
            Koi => 4,
            Perch => 5,
            ReefFish => 6,
            Cod => 7,
            Tuna => 8,
            Sprat => 9,
            Herring => 10,
            Mullet => 11,
            Bream => 12,
            Wrasse => 13,
            Mackerel => 14,
            Snapper => 15,
            Flounder => 16,
            Conger => 17,
            Grouper => 18,
            Emberjack => 19,
            Lancefin => 20,
            Gloamwing => 21,
            Blackwater => 22,
            Sovereign => 23,
            Drowner => 24,
            Ironscale => 25,
            RibbonKing => 26,
        };
        &T[i]
    }

    pub fn name(self) -> &'static str {
        self.info().name
    }

    pub fn habitat(self) -> &'static str {
        self.info().habitat
    }

    /// Stable snake_case id (MCP, export): the variant name.
    pub fn id(self) -> String {
        let mut out = String::new();
        for (i, c) in format!("{self:?}").chars().enumerate() {
            if c.is_uppercase() && i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        }
        out
    }

    /// Hover text: what it is and where it lives.
    pub fn about(self) -> String {
        let i = self.info();
        let from = i
            .emberfall
            .map(|n| format!("\nEmberfall: {n}"))
            .unwrap_or_default();
        format!(
            "{} — {}\n{} · {}{from}",
            i.name, i.look, i.habitat, i.rarity
        )
    }

    /// Finds a species by id, name, model file or Emberfall name (case, spaces and underscores ignored).
    pub fn parse(s: &str) -> Option<FishSpecies> {
        let norm = |s: &str| {
            s.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect::<String>()
        };
        let n = norm(s.trim_start_matches("The ").trim_start_matches("the "));
        FishSpecies::ALL.into_iter().find(|f| {
            let i = f.info();
            norm(&format!("{f:?}")) == n
                || norm(i.name) == n
                || norm(i.model) == n
                || i.emberfall
                    .is_some_and(|e| norm(e.trim_start_matches("The ")) == n)
        })
    }

    /// The species a new school gets when none is picked: by the water it is released in.
    pub fn default_for(fresh: bool) -> FishSpecies {
        if fresh {
            FishSpecies::Trout
        } else {
            FishSpecies::Mackerel
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_species_has_its_files_and_parses() {
        let dir = crate::asset_file("models/fish");
        let mut models = std::collections::HashSet::new();
        for f in FishSpecies::ALL {
            let i = f.info();
            assert!(models.insert(i.model), "{f:?} shares a model");
            for ext in ["glb", "png"] {
                assert!(
                    dir.join(format!("{}.{ext}", i.model)).is_file(),
                    "{f:?}: missing {}.{ext}",
                    i.model
                );
            }
            assert_eq!(FishSpecies::parse(&f.id()), Some(f));
            assert_eq!(FishSpecies::parse(i.name), Some(f));
            assert_eq!(FishSpecies::parse(i.model), Some(f));
            if let Some(e) = i.emberfall {
                assert_eq!(FishSpecies::parse(e), Some(f), "{e}");
            }
        }
    }

    #[test]
    fn old_saves_load() {
        for (s, f) in [
            ("\"Trout\"", FishSpecies::Trout),
            ("\"Salmon\"", FishSpecies::Salmon),
            ("\"ReefFish\"", FishSpecies::ReefFish),
            ("\"Tuna\"", FishSpecies::Tuna),
        ] {
            assert_eq!(serde_json::from_str::<FishSpecies>(s).unwrap(), f);
        }
    }
}
