//! What a town is made of: its sizes, the two looks (hand-painted, photoreal), the building
//! types with the words people use for them and the models that stand for each, the default
//! building mix for a size, and the catalog of models this account can place (`Catalog`).

use std::collections::{BTreeMap, HashMap, HashSet};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::noise::Rng;

/// How big a settlement is. Every size is laid out the same way, like a hand-made MMO town: a
/// road through it, a green (hamlets, villages) or a cobbled square (towns, cities) beside the
/// road with a landmark, the key buildings framing it, homes spaced well apart along a few lanes,
/// farmsteads and a windmill or watchtower on the outskirts. Bigger sizes have more of each, not
/// a denser grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TownSize {
    Hamlet,
    #[default]
    Village,
    Town,
    City,
}

impl TownSize {
    pub const ALL: [TownSize; 4] = [
        TownSize::Hamlet,
        TownSize::Village,
        TownSize::Town,
        TownSize::City,
    ];

    pub fn id(self) -> &'static str {
        match self {
            TownSize::Hamlet => "hamlet",
            TownSize::Village => "village",
            TownSize::Town => "town",
            TownSize::City => "city",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TownSize::Hamlet => "Hamlet",
            TownSize::Village => "Village",
            TownSize::Town => "Town",
            TownSize::City => "City",
        }
    }

    pub fn parse(s: &str) -> Option<TownSize> {
        let s = s.trim().to_lowercase();
        Self::ALL.into_iter().find(|k| k.id() == s)
    }

    /// A cobbled square with a town hall rather than a village green.
    pub fn is_town(self) -> bool {
        matches!(self, TownSize::Town | TownSize::City)
    }

    /// Half the length and depth (m) of the area a click gives it: room for its few buildings
    /// to stand well apart, with grass, trees and gardens between them.
    pub fn extent(self) -> Vec2 {
        match self {
            TownSize::Hamlet => Vec2::new(45.0, 35.0),
            TownSize::Village => Vec2::new(80.0, 60.0),
            TownSize::Town => Vec2::new(115.0, 90.0),
            TownSize::City => Vec2::new(170.0, 135.0),
        }
    }

    /// The area (m²) of the ellipse a click gives.
    pub fn default_area(self) -> f32 {
        let e = self.extent();
        std::f32::consts::PI * e.x * e.y
    }

    /// The size an area drawn for a village (or a town) amounts to (between two sizes' own
    /// areas: their geometric mean).
    pub fn for_area(town: bool, area: f32) -> TownSize {
        let between = |a: TownSize, b: TownSize| (a.default_area() * b.default_area()).sqrt();
        match (town, area) {
            (false, a) if a < between(TownSize::Hamlet, TownSize::Village) => TownSize::Hamlet,
            (false, _) => TownSize::Village,
            (true, a) if a < between(TownSize::Town, TownSize::City) => TownSize::Town,
            (true, _) => TownSize::City,
        }
    }

    /// How many buildings an area of this kind holds: few, like a hand-made MMO town — about 4
    /// in a hamlet, 8 in a village, 14 in a town and 28 in a city of their own extent. A bigger
    /// area gets somewhat more (the spacing stays).
    pub fn buildings_for(self, area: f32) -> usize {
        let (n, lo, hi) = match self {
            TownSize::Hamlet => (4.0, 3.0, 6.0),
            TownSize::Village => (8.0, 5.0, 16.0),
            TownSize::Town => (14.0, 9.0, 36.0),
            TownSize::City => (28.0, 18.0, 72.0),
        };
        (n * (area / self.default_area()).max(0.05).powf(0.6))
            .round()
            .clamp(lo, hi) as usize
    }

    pub fn hint(self) -> &'static str {
        match self {
            TownSize::Hamlet => "A farmstead and a couple of cottages round a well by the road",
            TownSize::Village => {
                "A green with a well, the inn and chapel facing it, a smithy where the road comes in, homes with gardens along a lane, a farm and a windmill"
            }
            TownSize::Town => {
                "A cobbled square with a statue and market stalls, framed by the town hall, inn, chapel and store; homes spaced along lanes, farms and a watchtower on the outskirts"
            }
            TownSize::City => {
                "A grand square with a rotunda and stalls, the hall, inn, chapel, bank and shops round it, homes along several lanes, farmsteads, a windmill and towers outside"
            }
        }
    }
}

/// The look of a town: which model library it is built from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// hand-painted if this account can use that pack, else photoreal
    #[default]
    Auto,
    Handpainted,
    Photoreal,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Auto, Style::Handpainted, Style::Photoreal];

    pub fn id(self) -> &'static str {
        match self {
            Style::Auto => "auto",
            Style::Handpainted => "handpainted",
            Style::Photoreal => "photoreal",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::Auto => "Auto",
            Style::Handpainted => "Hand-painted",
            Style::Photoreal => "Photoreal",
        }
    }

    pub fn parse(s: &str) -> Option<Style> {
        let k: String = s
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        match k.as_str() {
            "auto" | "" | "any" => Some(Style::Auto),
            "handpainted" | "hp" | "painted" | "stylized" | "stylised" | "fantasy" => {
                Some(Style::Handpainted)
            }
            "photoreal" | "photo" | "realistic" | "real" | "photoscan" | "sb" | "skyborn" => {
                Some(Style::Photoreal)
            }
            _ => None,
        }
    }

    /// A concrete style for this catalog: Auto is hand-painted when its inn is usable (the
    /// pack, not just its free samples), else photoreal when that is, else hand-painted samples.
    pub fn resolve(self, cat: &Catalog) -> Style {
        match self {
            Style::Auto if cat.has("hp:inn") => Style::Handpainted,
            Style::Auto if cat.has("sb:two_storey_inn") => Style::Photoreal,
            Style::Auto => Style::Handpainted,
            s => s,
        }
    }
}

/// What a building is for: its dressing follows (shops get barrels and crates, village homes
/// a garden fence, farms a haystack).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Home,
    Shop,
    Civic,
    Farm,
    Guard,
}

/// A kind of building: the words for it, its models per style and how central it stands.
pub struct BuildingType {
    pub id: &'static str,
    pub name: &'static str,
    /// what people call it (singular, lower case)
    pub words: &'static [&'static str],
    /// hand-painted models (`hp:`) and photoreal ones (`sb:`)
    pub hp: &'static [&'static str],
    pub sb: &'static [&'static str],
    /// 1: on the square, 0: at the very edge
    pub centre: f32,
    pub role: Role,
}

const fn bt(
    id: &'static str,
    name: &'static str,
    words: &'static [&'static str],
    hp: &'static [&'static str],
    sb: &'static [&'static str],
    centre: f32,
    role: Role,
) -> BuildingType {
    BuildingType {
        id,
        name,
        words,
        hp,
        sb,
        centre,
        role,
    }
}

/// Every building type a town can have. `house` is the generic home: it becomes a cottage or a
/// townhouse when the mix is drawn up.
pub const TYPES: &[BuildingType] = &[
    bt(
        "house",
        "House",
        &["house", "home", "dwelling", "residence", "homestead house"],
        &["house_small", "house_large"],
        &["caretaker_hut"],
        0.4,
        Role::Home,
    ),
    bt(
        "cottage",
        "Cottage",
        &["cottage", "small house", "hut", "shack", "cabin", "hovel"],
        &["house_small"],
        &["caretaker_hut"],
        0.3,
        Role::Home,
    ),
    bt(
        "townhouse",
        "Townhouse",
        &[
            "townhouse",
            "large house",
            "big house",
            "manor",
            "mansion",
            "rowhouse",
        ],
        &["house_large"],
        &["caretaker_hut"],
        0.55,
        Role::Home,
    ),
    bt(
        "town_hall",
        "Town hall",
        &[
            "town hall",
            "townhall",
            "hall",
            "guildhall",
            "guild hall",
            "city hall",
            "council",
            "mayor",
        ],
        &["town_hall"],
        &[],
        1.0,
        Role::Civic,
    ),
    bt(
        "chapel",
        "Chapel",
        &[
            "chapel",
            "church",
            "temple",
            "cathedral",
            "abbey",
            "monastery",
            "priory",
            "kirk",
        ],
        &["chapel"],
        &[],
        0.97,
        Role::Civic,
    ),
    bt(
        "inn",
        "Inn",
        &[
            "inn", "tavern", "pub", "alehouse", "hostel", "lodging", "hostelry", "saloon",
        ],
        &["inn"],
        &["two_storey_inn"],
        0.95,
        Role::Shop,
    ),
    bt(
        "bank",
        "Bank",
        &["bank", "treasury", "moneylender", "counting house", "vault"],
        &["bank"],
        &[],
        0.9,
        Role::Civic,
    ),
    bt(
        "general_store",
        "General store",
        &[
            "general store",
            "store",
            "shop",
            "merchant",
            "trader",
            "trading post",
            "emporium",
            "grocer",
            "grocery",
        ],
        &["general_store"],
        &["storehouse"],
        0.85,
        Role::Shop,
    ),
    bt(
        "jeweler",
        "Jeweller",
        &[
            "jeweler",
            "jeweller",
            "jewelry",
            "jewellery",
            "goldsmith",
            "gem",
        ],
        &["jeweler_shop"],
        &[],
        0.8,
        Role::Shop,
    ),
    bt(
        "library",
        "Scribe's library",
        &[
            "library",
            "scribe",
            "scriptorium",
            "bookshop",
            "bookstore",
            "archive",
            "book",
        ],
        &["scribe_library"],
        &[],
        0.78,
        Role::Civic,
    ),
    bt(
        "tailor",
        "Tailor",
        &[
            "tailor",
            "clothier",
            "seamstress",
            "dressmaker",
            "clothes",
            "clothing",
        ],
        &["tailor_shop"],
        &[],
        0.75,
        Role::Shop,
    ),
    bt(
        "alchemist",
        "Alchemist",
        &[
            "alchemist",
            "apothecary",
            "potion",
            "potion shop",
            "magic shop",
            "wizard",
        ],
        &["alchemist_shop"],
        &[],
        0.72,
        Role::Shop,
    ),
    bt(
        "bakery",
        "Bakery",
        &["bakery", "baker", "bread", "patisserie"],
        &["bakery"],
        &[],
        0.7,
        Role::Shop,
    ),
    bt(
        "butcher",
        "Butcher",
        &["butcher", "butchery", "meat", "slaughterhouse"],
        &["butcher_shop"],
        &[],
        0.68,
        Role::Shop,
    ),
    bt(
        "chandler",
        "Chandler",
        &["chandler", "candle maker", "candlemaker", "candle"],
        &["chandler_shop"],
        &[],
        0.62,
        Role::Shop,
    ),
    bt(
        "cobbler",
        "Cobbler",
        &["cobbler", "shoemaker", "shoe", "cordwainer", "boot"],
        &["cobbler_shop"],
        &[],
        0.6,
        Role::Shop,
    ),
    bt(
        "blacksmith",
        "Blacksmith",
        &[
            "blacksmith",
            "smithy",
            "smith",
            "forge",
            "armorer",
            "armourer",
            "ironmonger",
        ],
        &["blacksmith"],
        &["blacksmith"],
        0.5,
        Role::Shop,
    ),
    bt(
        "brewery",
        "Brewery",
        &[
            "brewery",
            "brewer",
            "brewhouse",
            "beer",
            "ale",
            "meadery",
            "distillery",
        ],
        &["brewery"],
        &[],
        0.45,
        Role::Shop,
    ),
    bt(
        "weaver",
        "Weaver",
        &["weaver", "weaving", "loom", "textile"],
        &["weaver_house"],
        &[],
        0.42,
        Role::Shop,
    ),
    bt(
        "herbalist",
        "Herbalist",
        &[
            "herbalist",
            "herb",
            "healer",
            "physician",
            "doctor",
            "medicine",
        ],
        &["herbalist_hut"],
        &["herbalist_shop"],
        0.4,
        Role::Shop,
    ),
    bt(
        "carpenter",
        "Carpenter",
        &[
            "carpenter",
            "joiner",
            "woodworker",
            "workshop",
            "furniture maker",
        ],
        &["carpenter_workshop"],
        &[],
        0.35,
        Role::Shop,
    ),
    bt(
        "cooper",
        "Cooper",
        &["cooper", "barrel maker", "barrelmaker"],
        &["cooper_workshop"],
        &[],
        0.33,
        Role::Shop,
    ),
    bt(
        "bowyer",
        "Bowyer & fletcher",
        &["bowyer", "fletcher", "archery", "bow", "arrow", "bowmaker"],
        &["bowyer_fletcher"],
        &[],
        0.3,
        Role::Shop,
    ),
    bt(
        "armory",
        "Armoury",
        &["armory", "armoury", "arsenal", "weapon"],
        &["armory"],
        &[],
        0.3,
        Role::Guard,
    ),
    bt(
        "stable",
        "Stable",
        &["stable", "horse", "livery", "stud"],
        &["stable"],
        &["stable"],
        0.25,
        Role::Farm,
    ),
    bt(
        "barracks",
        "Barracks",
        &[
            "barracks",
            "garrison",
            "guardhouse",
            "guard house",
            "soldier",
            "militia",
            "guard post",
        ],
        &["barracks"],
        &["watchhouse"],
        0.2,
        Role::Guard,
    ),
    bt(
        "mason",
        "Mason's yard",
        &["mason", "stonemason", "masonry", "stonecutter"],
        &["mason_yard"],
        &[],
        0.2,
        Role::Shop,
    ),
    bt(
        "warehouse",
        "Storehouse",
        &["warehouse", "storehouse", "store house", "storage", "depot"],
        &["granary"],
        &["storehouse", "shore_store"],
        0.2,
        Role::Farm,
    ),
    bt(
        "fisherman",
        "Fisherman's hut",
        &[
            "fisherman",
            "fishing hut",
            "fish",
            "fishmonger",
            "fisher",
            "fishery",
        ],
        &["fisherman_hut"],
        &["shore_store"],
        0.2,
        Role::Shop,
    ),
    bt(
        "potter",
        "Potter",
        &["potter", "pottery", "kiln", "ceramic"],
        &["pottery_kiln"],
        &[],
        0.18,
        Role::Shop,
    ),
    bt(
        "tannery",
        "Tannery",
        &["tannery", "tanner", "leather", "leatherworker", "hide"],
        &["tannery"],
        &[],
        0.12,
        Role::Shop,
    ),
    bt(
        "hunter",
        "Hunter's lodge",
        &[
            "hunter",
            "hunting lodge",
            "hunter lodge",
            "lodge",
            "trapper",
            "ranger",
        ],
        &["hunter_lodge"],
        &[],
        0.12,
        Role::Home,
    ),
    bt(
        "sawmill",
        "Sawmill",
        &[
            "sawmill",
            "lumber mill",
            "lumberyard",
            "lumber",
            "timber yard",
        ],
        &["sawmill"],
        &[],
        0.1,
        Role::Shop,
    ),
    bt(
        "farmhouse",
        "Farmhouse",
        &["farmhouse", "farm", "farmstead", "homestead", "croft"],
        &["farmhouse"],
        &[],
        0.1,
        Role::Farm,
    ),
    bt(
        "watchtower",
        "Watchtower",
        &[
            "watchtower",
            "tower",
            "guard tower",
            "lookout",
            "watch tower",
        ],
        &["watchtower"],
        &["watchhouse"],
        0.08,
        Role::Guard,
    ),
    bt(
        "granary",
        "Granary",
        &["granary", "grain store", "silo", "corn store"],
        &["granary"],
        &["storehouse"],
        0.06,
        Role::Farm,
    ),
    bt(
        "barn",
        "Barn",
        &["barn", "cowshed", "hayloft", "byre"],
        &["barn"],
        &["stable"],
        0.04,
        Role::Farm,
    ),
    bt(
        "windmill",
        "Windmill",
        &["windmill", "mill", "flour mill"],
        &["windmill"],
        &[],
        0.0,
        Role::Farm,
    ),
];

/// Things a request may ask for that aren't buildings along the streets but go on the square:
/// market stalls and the well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    Stall,
    Well,
}

const EXTRA_WORDS: &[(&str, Extra)] = &[
    ("market stall", Extra::Stall),
    ("market", Extra::Stall),
    ("stall", Extra::Stall),
    ("marketplace", Extra::Stall),
    ("bazaar", Extra::Stall),
    ("well", Extra::Well),
    ("fountain", Extra::Well),
];

pub fn type_index(id: &str) -> Option<usize> {
    TYPES.iter().position(|t| t.id == id)
}

fn norm(s: &str) -> String {
    let s: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let words: Vec<String> = s
        .split_whitespace()
        .filter(|w| !["the", "a", "an", "of", "some", "few", "new"].contains(w))
        .map(|w| {
            // plurals: smithies → smithy, churches → church, shops → shop
            if let Some(b) = w.strip_suffix("ies").filter(|b| b.len() >= 3) {
                format!("{b}y")
            } else if w.len() > 4
                && (w.ends_with("ches") || w.ends_with("shes") || w.ends_with("sses"))
            {
                w[..w.len() - 2].to_string()
            } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && w != "barracks" {
                w[..w.len() - 1].to_string()
            } else {
                w.to_string()
            }
        })
        .collect();
    words.join(" ")
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1)
                .min(cur[j - 1] + 1)
                .min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// What a word in a request means.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wanted {
    Building(usize),
    Extra(Extra),
}

/// A building type (or a square extra) for a word people use — "smithy", "tavern", "houses",
/// "candle maker" — the nearest one when it's misspelt, None when nothing is close.
pub fn parse_type(word: &str) -> Option<Wanted> {
    let w = norm(word);
    if w.is_empty() {
        return None;
    }
    let w = w.replace(' ', "_");
    let w = w.as_str();
    let flat = |s: &str| norm(s).replace(' ', "_");
    // exact: an id or a word
    if let Some(i) = TYPES
        .iter()
        .position(|t| t.id == w || t.words.iter().any(|x| flat(x) == w))
    {
        return Some(Wanted::Building(i));
    }
    if let Some((_, e)) = EXTRA_WORDS.iter().find(|(x, _)| flat(x) == w) {
        return Some(Wanted::Extra(*e));
    }
    // a word inside a longer phrase ("old stone chapel", "blacksmith shop")
    let mut best: Option<(usize, Wanted)> = None;
    for (i, t) in TYPES.iter().enumerate() {
        for x in t.words.iter().copied().chain([t.id]) {
            let x = flat(x);
            if x.len() >= 4
                && (w.contains(x.as_str()) || (w.len() >= 4 && x.contains(w)))
                && best.is_none_or(|b| x.len() > b.0)
            {
                best = Some((x.len(), Wanted::Building(i)));
            }
        }
    }
    for (x, e) in EXTRA_WORDS {
        let x = flat(x);
        if x.len() >= 4 && w.contains(x.as_str()) && best.is_none_or(|b| x.len() > b.0) {
            best = Some((x.len(), Wanted::Extra(*e)));
        }
    }
    if let Some((_, b)) = best {
        return Some(b);
    }
    // a misspelling: the closest word, at most two letters off (one for short words)
    let mut close: Option<(usize, Wanted)> = None;
    for (i, t) in TYPES.iter().enumerate() {
        for x in t.words.iter().copied().chain([t.id]) {
            let d = edit_distance(w, &flat(x));
            if close.is_none_or(|c| d < c.0) {
                close = Some((d, Wanted::Building(i)));
            }
        }
    }
    close
        .filter(|(d, _)| *d <= if w.len() >= 6 { 2 } else { 1 })
        .map(|c| c.1)
}

// ------------------------------------------------------------------ the catalog

/// The models this account can place, with their bounds (model frame, metres), and the ones it
/// can't (locked packs), for the notes.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub assets: HashMap<String, (Vec3, Vec3)>,
    pub locked: HashSet<String>,
}

impl Catalog {
    pub fn has(&self, id: &str) -> bool {
        self.assets.contains_key(id)
    }

    pub fn bounds(&self, id: &str) -> Option<(Vec3, Vec3)> {
        self.assets.get(id).copied()
    }

    /// From the bundled libraries' `models.json` (tests): every model, usable as `usable` says.
    #[cfg(test)]
    pub fn bundled(usable: impl Fn(&str) -> bool) -> Catalog {
        let dir = std::path::PathBuf::from(crate::assets_path()).join("models");
        let mut c = Catalog::default();
        for lib in crate::objects::model_libraries(&dir) {
            let Some(doc) = std::fs::read_to_string(dir.join(&lib.folder).join("models.json"))
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            else {
                continue;
            };
            for m in doc["models"].as_array().into_iter().flatten() {
                let (Some(id), Some(a), Some(b)) = (
                    m["id"].as_str(),
                    m["aabb_min"].as_array(),
                    m["aabb_max"].as_array(),
                ) else {
                    continue;
                };
                let v = |a: &Vec<serde_json::Value>| {
                    Vec3::new(
                        a[0].as_f64().unwrap_or(0.0) as f32,
                        a[1].as_f64().unwrap_or(0.0) as f32,
                        a[2].as_f64().unwrap_or(0.0) as f32,
                    )
                };
                let id = format!("{}:{id}", lib.prefix);
                if usable(&id) {
                    c.assets.insert(id, (v(a), v(b)));
                } else {
                    c.locked.insert(id);
                }
            }
        }
        c
    }
}

/// The models a type has in a style.
pub fn models_of(t: &BuildingType, style: Style) -> &'static [&'static str] {
    match style {
        Style::Photoreal => t.sb,
        _ => t.hp,
    }
}

fn prefix(style: Style) -> &'static str {
    match style {
        Style::Photoreal => "sb",
        _ => "hp",
    }
}

/// One building to place: its type, its model and how near the centre it wants to stand.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub ty: usize,
    pub asset: String,
    pub key: f32,
}

/// The buildings to place (most central first), the square's extras and notes about what
/// couldn't be had.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mix {
    pub wants: Vec<Want>,
    /// market stalls asked for (None: the square's own few)
    pub stalls: Option<usize>,
    pub notes: Vec<String>,
}

/// The default mix of `n` buildings for a size, as (type id, count): the first `n` of the size's
/// own order of importance, so a small settlement keeps its inn, smithy and homes and a bigger
/// one adds a chapel, a store, a farmstead, a windmill or watchtower, more trades and more homes.
pub fn default_counts(size: TownSize, n: usize, rng: &mut Rng) -> Vec<(&'static str, usize)> {
    // trades in a per-seed order (taken one by one where the order says "trade")
    let mut trades: Vec<&'static str> = match size {
        TownSize::Hamlet => vec!["herbalist", "hunter", "weaver", "carpenter"],
        TownSize::Village => vec![
            "bakery",
            "weaver",
            "herbalist",
            "cobbler",
            "carpenter",
            "cooper",
            "butcher",
            "tailor",
            "hunter",
        ],
        _ => vec![
            "bakery",
            "butcher",
            "tailor",
            "cobbler",
            "alchemist",
            "jeweler",
            "library",
            "chandler",
            "brewery",
            "weaver",
            "herbalist",
            "carpenter",
            "cooper",
            "bowyer",
            "potter",
            "mason",
            "tannery",
        ],
    };
    for i in (1..trades.len()).rev() {
        trades.swap(i, rng.below(i as u32 + 1) as usize);
    }
    let store = if rng.f32() < 0.65 {
        "general_store"
    } else {
        "bakery"
    };
    let mill = if rng.f32() < 0.7 {
        "windmill"
    } else {
        "watchtower"
    };
    // the order of importance ("trade": the next trade; past its end, homes and the odd farm)
    let order: &[&'static str] = match size {
        TownSize::Hamlet => &[
            "farmhouse",
            "cottage",
            "barn",
            "cottage",
            "trade",
            "cottage",
        ],
        TownSize::Village => &[
            "inn",
            "blacksmith",
            "house",
            "farmhouse",
            "house",
            "chapel",
            store,
            mill,
            "barn",
            "house",
            "trade",
            "house",
            "farmhouse",
            "house",
            "trade",
            "granary",
        ],
        TownSize::Town => &[
            "town_hall",
            "inn",
            "chapel",
            "general_store",
            "blacksmith",
            "house",
            "house",
            "trade",
            "house",
            "farmhouse",
            "trade",
            "house",
            "watchtower",
            "barn",
            "house",
            "stable",
            "trade",
            "house",
            "windmill",
            "house",
            "bank",
            "trade",
            "house",
            "farmhouse",
        ],
        TownSize::City => &[
            "town_hall",
            "inn",
            "chapel",
            "general_store",
            "blacksmith",
            "bank",
            "house",
            "trade",
            "house",
            "trade",
            "house",
            "house",
            "farmhouse",
            "trade",
            "watchtower",
            "house",
            "barracks",
            "barn",
            "trade",
            "house",
            "stable",
            "windmill",
            "house",
            "trade",
            "house",
            "farmhouse",
            "house",
            "trade",
            "granary",
            "house",
            "trade",
            "house",
            "armory",
            "house",
            "barn",
            "trade",
            "house",
        ],
    };
    let home = if size == TownSize::Hamlet {
        "cottage"
    } else {
        "house"
    };
    let mut next_trade = 0;
    let mut out: Vec<(&'static str, usize)> = vec![];
    for i in 0..n {
        let id = match order.get(i).copied() {
            Some("trade") => {
                next_trade += 1;
                trades.get(next_trade - 1).copied().unwrap_or(home)
            }
            Some(id) => id,
            // past the order: mostly homes, a farmstead every dozen
            None => match (i - order.len()) % 12 {
                5 => "farmhouse",
                6 => "barn",
                9 => trades
                    .get(next_trade)
                    .copied()
                    .inspect(|_| next_trade += 1)
                    .unwrap_or(home),
                _ => home,
            },
        };
        match out.iter_mut().find(|o| o.0 == id) {
            Some(o) => o.1 += 1,
            None => out.push((id, 1)),
        }
    }
    out
}

/// Draws up the buildings to place from counts per type for a style: types the style has no
/// model for (or whose models are locked) are left out with a note, homes pick a cottage or a
/// townhouse (more townhouses in towns), and each gets its place in the order (central ones
/// first, a little shuffled). `asked`: the counts came from a request (its notes name what was
/// skipped; a default mix quietly fills the gaps with homes).
pub fn draw_up(
    counts: &[(usize, usize)],
    size: TownSize,
    style: Style,
    cat: &Catalog,
    asked: bool,
    rng: &mut Rng,
) -> Mix {
    let mut mix = Mix::default();
    let mut missing = 0;
    let townish = size.is_town();
    let mut skipped: BTreeMap<&str, (usize, bool)> = BTreeMap::new();
    let usable_ids = |t: &BuildingType| -> Vec<String> {
        models_of(t, style)
            .iter()
            .map(|m| format!("{}:{m}", prefix(style)))
            .filter(|id| cat.has(id))
            .collect()
    };
    let mut counts: Vec<(usize, usize)> = counts.to_vec();
    if !asked {
        // a default mix: what the style lacks becomes a building of the same kind it has (a
        // shop for a shop), or a home
        // (at most three of any one stand-in: a street of the same shop reads as a mistake)
        let mut more: Vec<(usize, usize)> = vec![];
        let mut used: BTreeMap<usize, usize> = counts.iter().copied().collect();
        for c in &mut counts {
            let t = &TYPES[c.0];
            if !usable_ids(t).is_empty() {
                continue;
            }
            skipped.entry(t.name).or_insert((0, false));
            for _ in 0..c.1 {
                let alike: Vec<usize> = (0..TYPES.len())
                    .filter(|i| {
                        TYPES[*i].role == t.role
                            && t.role != Role::Civic
                            && !matches!(
                                TYPES[*i].id,
                                "house" | "cottage" | "townhouse" | "inn" | "blacksmith"
                            )
                    })
                    .filter(|i| {
                        used.get(i).copied().unwrap_or(0) < 3 && !usable_ids(&TYPES[*i]).is_empty()
                    })
                    .collect();
                if alike.is_empty() {
                    break;
                }
                let pick = alike[rng.below(alike.len() as u32) as usize];
                *used.entry(pick).or_default() += 1;
                more.push((pick, 1));
                c.1 -= 1;
            }
        }
        counts.extend(more);
    }
    for &(ti, k) in &counts {
        let t = &TYPES[ti];
        if k == 0 {
            continue;
        }
        let ids: Vec<String> = models_of(t, style)
            .iter()
            .map(|m| format!("{}:{m}", prefix(style)))
            .collect();
        let usable: Vec<&String> = ids.iter().filter(|id| cat.has(id)).collect();
        if usable.is_empty() {
            let locked = ids.iter().any(|id| cat.locked.contains(id));
            let e = skipped.entry(t.name).or_insert((0, locked));
            e.0 += k;
            missing += k;
            continue;
        }
        for _ in 0..k {
            let asset = if t.id == "house" && usable.len() > 1 {
                // townhouses toward the middle of a town, cottages in villages
                let large = rng.f32() < if townish { 0.55 } else { 0.28 };
                usable
                    .iter()
                    .find(|a| a.ends_with(if large { "house_large" } else { "house_small" }))
                    .copied()
                    .unwrap_or(usable[0])
                    .clone()
            } else {
                usable[rng.below(usable.len() as u32) as usize].clone()
            };
            let centre = match asset.as_str() {
                "hp:house_large" => 0.55,
                "hp:house_small" => 0.3,
                _ => t.centre,
            };
            // the square's own buildings keep their place; the rest mix a little
            let jitter = if centre >= 0.94 {
                0.0
            } else {
                rng.range(-0.16, 0.16)
            };
            mix.wants.push(Want {
                ty: ti,
                asset,
                key: centre + jitter,
            });
        }
    }
    let style_name = if style == Style::Photoreal {
        "photoreal"
    } else {
        "hand-painted"
    };
    for (name, (k, locked)) in &skipped {
        if !asked {
            continue;
        }
        mix.notes.push(if *locked {
            format!("{k} × {name}: its models are in an asset pack this account hasn't unlocked (Objects › Asset packs) — skipped")
        } else {
            format!("{k} × {name}: the {style_name} style has no model for it — skipped")
        });
    }
    if !asked && missing > 0 {
        // homes stand in for the rest
        let home = type_index("house").unwrap_or(0);
        let more = draw_up(&[(home, missing)], size, style, cat, false, rng);
        mix.wants.extend(more.wants);
    }
    if !asked && !skipped.is_empty() {
        let names: Vec<&str> = skipped.keys().copied().collect();
        mix.notes.push(format!("No {style_name} model (or a locked one) for: {} — similar buildings or homes took their place", names.join(", ")));
    }
    mix.wants.sort_by(|a, b| b.key.total_cmp(&a.key));
    mix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_map_to_types() {
        let id = |w: &str| match parse_type(w) {
            Some(Wanted::Building(i)) => TYPES[i].id,
            Some(Wanted::Extra(Extra::Stall)) => "stall",
            Some(Wanted::Extra(Extra::Well)) => "well",
            None => "-",
        };
        for (w, want) in [
            ("house", "house"),
            ("Houses", "house"),
            ("cottages", "cottage"),
            ("tavern", "inn"),
            ("smithy", "blacksmith"),
            ("smithies", "blacksmith"),
            ("Blacksmith", "blacksmith"),
            ("church", "chapel"),
            ("temple", "chapel"),
            ("market", "stall"),
            ("market stalls", "stall"),
            ("bakery", "bakery"),
            ("butcher", "butcher"),
            ("tailor", "tailor"),
            ("candle maker", "chandler"),
            ("stables", "stable"),
            ("barracks", "barracks"),
            ("blacksmth", "blacksmith"),
            ("old stone chapel", "chapel"),
            ("well", "well"),
            ("town_hall", "town_hall"),
            ("spaceport", "-"),
        ] {
            assert_eq!(id(w), want, "{w}");
        }
    }

    #[test]
    fn every_type_has_bundled_models() {
        let cat = Catalog::bundled(|_| true);
        let handpainted_installed =
            crate::asset_file("models/stylepacks/handpainted/models.json").is_file();
        for t in TYPES {
            if handpainted_installed {
                assert!(!t.hp.is_empty(), "{}", t.id);
                for m in t.hp {
                    assert!(cat.has(&format!("hp:{m}")), "hp:{m}");
                }
            }
            for m in t.sb {
                assert!(cat.has(&format!("sb:{m}")), "sb:{m}");
            }
        }
    }

    #[test]
    fn mixes_respect_the_catalog() {
        if !crate::asset_file("models/stylepacks/handpainted/models.json").is_file() {
            return; // its sample-lock scenarios require the optional paid-pack manifest
        }
        let all = Catalog::bundled(|_| true);
        // only the free samples of the hand-painted pack
        let samples = Catalog::bundled(|id| {
            !id.starts_with("hp:")
                || [
                    "hp:house_small",
                    "hp:market_stall",
                    "hp:well",
                    "hp:lamp_post",
                    "hp:bench",
                    "hp:barrel",
                    "hp:crate",
                    "hp:signpost",
                    "hp:fence",
                ]
                .contains(&id)
        });
        assert_eq!(Style::Auto.resolve(&all), Style::Handpainted);
        assert_eq!(Style::Auto.resolve(&samples), Style::Photoreal);
        let mut rng = Rng::new(3);
        let counts: Vec<(usize, usize)> = default_counts(TownSize::Town, 40, &mut rng)
            .into_iter()
            .map(|(id, k)| (type_index(id).unwrap(), k))
            .collect();
        assert_eq!(counts.iter().map(|c| c.1).sum::<usize>(), 40);
        let m = draw_up(
            &counts,
            TownSize::Town,
            Style::Handpainted,
            &samples,
            false,
            &mut rng,
        );
        assert_eq!(m.wants.len(), 40, "homes fill in for the locked models");
        assert!(m.wants.iter().all(|w| w.asset == "hp:house_small"));
        let asked = [
            (type_index("inn").unwrap(), 1),
            (type_index("house").unwrap(), 5),
        ];
        let m = draw_up(
            &asked,
            TownSize::Village,
            Style::Handpainted,
            &samples,
            true,
            &mut rng,
        );
        assert_eq!(m.wants.len(), 5);
        assert!(
            m.notes
                .iter()
                .any(|n| n.contains("Inn") && n.contains("unlocked")),
            "{:?}",
            m.notes
        );
        let m = draw_up(
            &asked,
            TownSize::Village,
            Style::Photoreal,
            &all,
            true,
            &mut rng,
        );
        assert_eq!(m.wants.len(), 6);
        assert!(m.wants.iter().all(|w| w.asset.starts_with("sb:")));
        // central ones first
        assert_eq!(m.wants[0].asset, "sb:two_storey_inn");
    }

    #[test]
    fn photoreal_mix_respects_the_catalog_without_paid_pack() {
        let all = Catalog::bundled(|_| true);
        let asked = [
            (type_index("inn").unwrap(), 1),
            (type_index("house").unwrap(), 5),
        ];
        let mut rng = Rng::new(3);
        let m = draw_up(
            &asked,
            TownSize::Village,
            Style::Photoreal,
            &all,
            true,
            &mut rng,
        );
        assert_eq!(m.wants.len(), 6);
        assert!(m.wants.iter().all(|w| w.asset.starts_with("sb:")));
        // central ones first
        assert_eq!(m.wants[0].asset, "sb:two_storey_inn");
    }
}
