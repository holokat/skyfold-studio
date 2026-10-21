//! The world plan: everything the guided setup asks (or the AI reads from the user's words),
//! which `build` turns into a playable world. Kept with the world (`plan.json` in its folder).

use serde::{Deserialize, Serialize};

/// The MMOs the user can pick as inspiration (their names are only shown as words; the cards'
/// pictures, `assets/onboarding/<id>.jpg`, are our own). Each suggests an art style, a look, the kind of game and its
/// level range.
pub struct Inspiration {
    pub id: &'static str,
    pub name: &'static str,
    /// what it brings to the plan, in a few words
    pub blurb: &'static str,
    pub art: Art,
    pub look: &'static str,
    pub game: GameKind,
    pub levels: [u32; 2],
}

pub const INSPIRATIONS: [Inspiration; 8] = [
    Inspiration {
        id: "wow",
        name: "World of Warcraft",
        blurb: "Bold hand-painted colour, big themed zones, capital cities, dungeons and raids",
        art: Art::HandPainted,
        look: "epic_sunbeams",
        game: GameKind::Themepark,
        levels: [1, 60],
    },
    Inspiration {
        id: "daoc",
        name: "Dark Age of Camelot",
        blurb: "Three realms at war over frontier keeps, misty northern lands",
        art: Art::HandPainted,
        look: "nordic",
        game: GameKind::RealmWar,
        levels: [1, 50],
    },
    Inspiration {
        id: "bdo",
        name: "Black Desert",
        blurb: "Photoreal landscapes, cinematic light, gathering and trade everywhere",
        art: Art::Photoreal,
        look: "cinematic",
        game: GameKind::Sandbox,
        levels: [1, 60],
    },
    Inspiration {
        id: "elder_scrolls",
        name: "The Elder Scrolls",
        blurb: "Realistic wilds, snowy mountains, ruins and caves to explore",
        art: Art::Photoreal,
        look: "nordic",
        game: GameKind::Story,
        levels: [1, 50],
    },
    Inspiration {
        id: "gw2",
        name: "Guild Wars 2",
        blurb: "Painterly vistas, open events, lots to discover in every zone",
        art: Art::HandPainted,
        look: "golden_hour",
        game: GameKind::Themepark,
        levels: [1, 80],
    },
    Inspiration {
        id: "lotro",
        name: "Lord of the Rings Online",
        blurb: "Gentle green countryside, old roads, a long journey into darker lands",
        art: Art::Photoreal,
        look: "golden_vintage",
        game: GameKind::Story,
        levels: [1, 50],
    },
    Inspiration {
        id: "ffxiv",
        name: "Final Fantasy XIV",
        blurb: "Bright, magical places, grand cities and story-driven dungeons",
        art: Art::HandPainted,
        look: "enchanted",
        game: GameKind::Story,
        levels: [1, 90],
    },
    Inspiration {
        id: "runescape",
        name: "RuneScape",
        blurb: "A friendly medieval world built round skills: mining, woodcutting, fishing",
        art: Art::HandPainted,
        look: "natural",
        game: GameKind::Sandbox,
        levels: [1, 99],
    },
];

pub fn inspiration(id: &str) -> Option<&'static Inspiration> {
    INSPIRATIONS.iter().find(|i| i.id == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Art {
    #[default]
    HandPainted,
    Photoreal,
}

impl Art {
    pub const ALL: [Art; 2] = [Art::HandPainted, Art::Photoreal];
    pub fn name(self) -> &'static str {
        match self {
            Art::HandPainted => "Hand-painted",
            Art::Photoreal => "Realistic",
        }
    }
    /// The id the town and dungeon tools take.
    pub fn id(self) -> &'static str {
        match self {
            Art::HandPainted => "handpainted",
            Art::Photoreal => "photoreal",
        }
    }
}

/// What kind of MMO it is: how many dungeons and raids, frontier zones, how much gathering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameKind {
    /// quest hubs, zones by level, dungeons and raids
    #[default]
    Themepark,
    /// open world, gathering and crafting, player-made goals
    Sandbox,
    /// realms at war over a contested frontier
    RealmWar,
    /// survive, gather, build; dangerous wilds
    Survival,
    /// a journey through the land, story dungeons
    Story,
}

impl GameKind {
    pub const ALL: [GameKind; 5] = [
        GameKind::Themepark,
        GameKind::Sandbox,
        GameKind::RealmWar,
        GameKind::Survival,
        GameKind::Story,
    ];
    pub fn name(self) -> &'static str {
        match self {
            GameKind::Themepark => "Themepark adventure",
            GameKind::Sandbox => "Open sandbox",
            GameKind::RealmWar => "Realm vs realm",
            GameKind::Survival => "Survival & crafting",
            GameKind::Story => "Story journey",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            GameKind::Themepark => "Zones by level, quest hubs in every town, dungeons and raids",
            GameKind::Sandbox => "An open world full of resources; players make their own goals",
            GameKind::RealmWar => "Realms with their own homelands, fighting over a frontier",
            GameKind::Survival => "Wild, dangerous land; gather, craft and build to survive",
            GameKind::Story => "A journey across the land from a quiet start to a dark end",
        }
    }
}

/// The shape of the land.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Land {
    #[default]
    Continent,
    /// two landmasses with a sea between them
    TwinIslands,
    Archipelago,
    Peninsula,
    /// a ring of land round an inland sea
    InlandSea,
    /// a land of fjords and steep coasts
    FjordCoast,
    /// one island with a mountain in the middle
    MountainIsland,
}

impl Land {
    pub const ALL: [Land; 7] = [
        Land::Continent,
        Land::TwinIslands,
        Land::Archipelago,
        Land::Peninsula,
        Land::InlandSea,
        Land::FjordCoast,
        Land::MountainIsland,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Land::Continent => "One continent",
            Land::TwinIslands => "Two continents",
            Land::Archipelago => "Many islands",
            Land::Peninsula => "A peninsula",
            Land::InlandSea => "Round an inland sea",
            Land::FjordCoast => "Fjords",
            Land::MountainIsland => "A mountain island",
        }
    }
    /// The `world_generate_continent` preset.
    pub fn preset(self) -> &'static str {
        match self {
            Land::Continent => "continent",
            Land::TwinIslands => "twin_islands",
            Land::Archipelago => "archipelago",
            Land::Peninsula => "peninsula",
            Land::InlandSea => "inland_sea",
            Land::FjordCoast => "fjord_coast",
            Land::MountainIsland => "mountain_island",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl WorldSize {
    pub const ALL: [WorldSize; 3] = [WorldSize::Small, WorldSize::Medium, WorldSize::Large];
    pub fn km(self) -> u32 {
        match self {
            WorldSize::Small => 2,
            WorldSize::Medium => 4,
            WorldSize::Large => 8,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            WorldSize::Small => "Small · 2 km",
            WorldSize::Medium => "Medium · 4 km",
            WorldSize::Large => "Large · 8 km",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Climate {
    /// from snowy north to a warm south
    #[default]
    Varied,
    Temperate,
    Tropical,
    Arid,
    Cold,
}

impl Climate {
    pub const ALL: [Climate; 5] = [
        Climate::Varied,
        Climate::Temperate,
        Climate::Tropical,
        Climate::Arid,
        Climate::Cold,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Climate::Varied => "Every climate",
            Climate::Temperate => "Green and temperate",
            Climate::Tropical => "Tropical",
            Climate::Arid => "Deserts and canyons",
            Climate::Cold => "Cold and snowy",
        }
    }
    /// The climate id the world generator takes.
    pub fn id(self) -> &'static str {
        match self {
            Climate::Varied => "varied",
            Climate::Temperate => "temperate",
            Climate::Tropical => "tropical",
            Climate::Arid => "arid",
            Climate::Cold => "cold",
        }
    }
}

/// What can be gathered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gathering {
    pub herbs: bool,
    pub wood: bool,
    pub stone: bool,
    pub ore: bool,
    /// fishing spots
    pub fish: bool,
    /// 0.5 sparse … 1 normal … 2 plenty
    pub density: f32,
}

impl Default for Gathering {
    fn default() -> Self {
        Self {
            herbs: true,
            wood: true,
            stone: true,
            ore: true,
            fish: true,
            density: 1.0,
        }
    }
}

/// Names the AI made up (or the user typed); empty ones are made up from the land.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Names {
    pub world: String,
    pub capital: String,
    pub towns: Vec<String>,
    pub zones: Vec<String>,
    pub dungeons: Vec<String>,
    pub realms: Vec<String>,
}

/// Everything the guided setup decides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldPlan {
    /// the world's name (its folder: worlds/<name>.world)
    pub name: String,
    /// the user's own words about their world (the AI reads them)
    pub description: String,
    /// MMOs they love (`INSPIRATIONS` ids)
    pub inspirations: Vec<String>,
    pub art: Art,
    /// the look preset (`look_set` preset id)
    pub look: String,
    pub game: GameKind,
    /// lowest and highest character level
    pub levels: [u32; 2],
    pub land: Land,
    pub size: WorldSize,
    pub climate: Climate,
    pub rivers: bool,
    /// realms / factions with homelands of their own (1 = none)
    pub realms: u32,
    pub capital: bool,
    pub towns: u32,
    pub villages: u32,
    /// roads between the settlements
    pub roads: bool,
    /// farm fields round villages
    pub farms: bool,
    /// zones with names and level ranges (the user's choice: zones are never made unasked)
    pub zones: bool,
    /// underground dungeons (party) and raids (big, raid-sized)
    pub dungeons: u32,
    pub raids: u32,
    /// how many of them are open-air dungeons built from design sheets (`dungeons::sheets`):
    /// the raids first, then every other party dungeon
    pub outdoor_dungeons: u32,
    /// paint each open-air dungeon a unique sheet of its own with the user's AI (Codex) after the
    /// world is made (it starts from a library sheet meanwhile)
    pub unique_dungeons: bool,
    /// layout ids the dungeons are made from (empty: chosen by the land each sits in)
    pub dungeon_layouts: Vec<String>,
    /// how big dungeon rooms are, and how uneven (0 regular … 1 very)
    pub dungeon_rooms: String,
    pub dungeon_irregularity: f32,
    pub caves: u32,
    /// ready-made encounter sites (monster camps…) out in the wilds
    pub encounters: bool,
    /// creature spawn regions (with a starter bestiary)
    pub creatures: bool,
    pub gathering: Gathering,
    /// time of day the world opens at
    pub hour: f32,
    pub seed: u32,
    pub names: Names,
}

impl Default for WorldPlan {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            inspirations: Vec::new(),
            art: Art::HandPainted,
            look: "natural".into(),
            game: GameKind::Themepark,
            levels: [1, 60],
            land: Land::Continent,
            size: WorldSize::Medium,
            climate: Climate::Varied,
            rivers: true,
            realms: 1,
            capital: true,
            towns: 3,
            villages: 4,
            roads: true,
            farms: true,
            zones: true,
            dungeons: 4,
            raids: 1,
            outdoor_dungeons: 3,
            unique_dungeons: false,
            dungeon_layouts: Vec::new(),
            dungeon_rooms: "normal".into(),
            dungeon_irregularity: 0.3,
            caves: 3,
            encounters: true,
            creatures: true,
            gathering: Gathering::default(),
            hour: 16.5,
            seed: 0,
            names: Names::default(),
        }
    }
}

impl WorldPlan {
    /// Picking inspirations sets the art, look, kind of game and level range from the first
    /// one (the user can change each afterwards).
    pub fn apply_inspirations(&mut self) {
        let Some(i) = self.inspirations.first().and_then(|id| inspiration(id)) else {
            return;
        };
        self.art = i.art;
        self.look = i.look.into();
        self.game = i.game;
        self.levels = i.levels;
        match i.game {
            GameKind::RealmWar => self.realms = 3,
            GameKind::Sandbox | GameKind::Survival => self.gathering.density = 1.5,
            _ => {}
        }
    }

    /// Sensible counts for the world's size (a larger world holds more).
    pub fn fit_to_size(&mut self) {
        let (t, v, d, c) = match self.size {
            WorldSize::Small => (1, 2, 2, 1),
            WorldSize::Medium => (3, 4, 4, 3),
            WorldSize::Large => (6, 8, 8, 6),
        };
        self.towns = t;
        self.villages = v;
        self.dungeons = d;
        self.caves = c;
        self.raids = if self.size == WorldSize::Small { 0 } else { 1 };
    }

    /// The plan as the build sees it: counts kept within what fits, a seed chosen.
    pub fn normalized(&self) -> WorldPlan {
        let mut p = self.clone();
        let cap = match p.size {
            WorldSize::Small => 1,
            WorldSize::Medium => 2,
            WorldSize::Large => 4,
        };
        p.realms = p.realms.clamp(1, 3);
        p.towns = p.towns.min(4 * cap);
        p.villages = p.villages.min(6 * cap);
        p.dungeons = p.dungeons.min(6 * cap);
        p.raids = p.raids.min(cap);
        p.caves = p.caves.min(5 * cap);
        p.levels = [p.levels[0].max(1), p.levels[1].max(p.levels[0] + 4)];
        p.dungeon_irregularity = p.dungeon_irregularity.clamp(0.0, 1.0);
        if p.seed == 0 {
            p.seed = fastrand::u32(1..u32::MAX);
        }
        if p.name.trim().is_empty() {
            p.name = if p.names.world.trim().is_empty() {
                "New World".into()
            } else {
                p.names.world.clone()
            };
        }
        p
    }

    /// One line per decision, for the review page and the build's log.
    pub fn summary(&self) -> Vec<String> {
        let mut v = vec![
            format!(
                "{} · {} · {}",
                self.size.name(),
                self.land.name(),
                self.climate.name()
            ),
            format!(
                "{} · {} art · levels {}–{}",
                self.game.name(),
                self.art.name(),
                self.levels[0],
                self.levels[1]
            ),
        ];
        let mut places = vec![];
        if self.capital {
            places.push("a capital city".to_string());
        }
        if self.towns > 0 {
            places.push(format!("{} towns", self.towns));
        }
        if self.villages > 0 {
            places.push(format!("{} villages", self.villages));
        }
        if !places.is_empty() {
            v.push(format!(
                "{}{}",
                places.join(", "),
                if self.roads { ", joined by roads" } else { "" }
            ));
        }
        if self.realms > 1 {
            v.push(format!(
                "{} realms with homelands of their own",
                self.realms
            ));
        }
        let mut adv = vec![];
        if self.dungeons > 0 {
            adv.push(format!("{} dungeons", self.dungeons));
        }
        if self.raids > 0 {
            adv.push(format!(
                "{} raid{}",
                self.raids,
                if self.raids == 1 { "" } else { "s" }
            ));
        }
        if self.caves > 0 {
            adv.push(format!("{} caves", self.caves));
        }
        if self.encounters {
            adv.push("monster camps".into());
        }
        if !adv.is_empty() {
            v.push(adv.join(", "));
        }
        if self.zones {
            v.push("Zones with names and level ranges".into());
        }
        let g = &self.gathering;
        let kinds: Vec<&str> = [
            (g.herbs, "herbs"),
            (g.wood, "wood"),
            (g.stone, "stone"),
            (g.ore, "ore"),
            (g.fish, "fish"),
        ]
        .into_iter()
        .filter(|x| x.0)
        .map(|x| x.1)
        .collect();
        if !kinds.is_empty() {
            v.push(format!("Gathering: {}", kinds.join(", ")));
        }
        if self.creatures {
            v.push("Creatures in every zone".into());
        }
        v
    }
}

/// A made-up world name (Asterath, Valoria, Thalmere…) for the setup's dice button: a stem and an
/// ending that join without clashing letters. `taken` names to skip (the current one, worlds on disk).
pub fn random_world_name(taken: impl Fn(&str) -> bool) -> String {
    const STEM: &[&str] = &[
        "Aster", "Eld", "Val", "Myr", "Thal", "Kor", "Sol", "Aer", "Dra", "Ith", "Ost", "Zeph",
        "Bel", "Cal", "Lor", "Mor", "Tir", "Vel", "Ny", "Ar", "Eri", "Gal", "Lum", "Nor", "Oro",
        "Pyr", "Quel", "Rha", "Syl", "Tor", "Ul", "Xan", "Yr", "Zan", "Aza", "Bryn", "Cyr", "Dun",
        "Fen", "Hal", "Iss", "Mal", "Nim", "Sar", "Tal", "Vor", "Wyn", "Ever", "Ash", "Elan", "Ky",
        "Sel", "Aur", "Om", "Kael", "Ser",
    ];
    const END: &[&str] = &[
        "ath", "oria", "endor", "ara", "orn", "aris", "ion", "eth", "anor", "ia", "undra", "ys",
        "ador", "essa", "oth", "ania", "ereth", "indel", "os", "avar", "emar", "une", "ith", "ova",
        "aron", "iel", "ast", "mere", "gard", "dell", "wyn", "thas", "dor", "mir", "rath", "ven",
        "loth", "deth",
    ];
    let vowel = |c: char| "aeiou".contains(c);
    let mut last = String::new();
    for _ in 0..200 {
        let (s, e) = (
            STEM[fastrand::usize(..STEM.len())],
            END[fastrand::usize(..END.len())],
        );
        let (sl, e0) = (
            s.chars().last().unwrap().to_ascii_lowercase(),
            e.chars().next().unwrap(),
        );
        let tail = s
            .chars()
            .rev()
            .take_while(|c| !vowel(c.to_ascii_lowercase()))
            .count();
        // no "Draara" (two vowels), "Ostdor" (three consonants) or "Valloth" (a doubled letter)
        if (vowel(sl) && vowel(e0)) || (tail >= 2 && !vowel(e0)) || sl == e0 {
            continue;
        }
        last = format!("{s}{e}");
        if (5..=10).contains(&last.len()) && !taken(&last) {
            return last;
        }
    }
    last
}
