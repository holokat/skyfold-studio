//! One-click dungeon layouts: each lays out a whole plan (levels, rooms, halls, corridors,
//! caverns, stairs, pits, pools, daises, bridges, room roles) in its character, in a style and a
//! mood that suit it. Deterministic for a seed; Shuffle re-rolls the seed.

use bevy::math::{Vec2, Vec3};

use super::plan::*;
use super::styles;
use crate::noise::Rng;

/// How big a layout is.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    Small,
    #[default]
    Medium,
    Large,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Small, Size::Medium, Size::Large];
    pub fn name(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    fn rooms(self) -> usize {
        match self {
            Size::Small => 5,
            Size::Medium => 9,
            Size::Large => 15,
        }
    }
    fn k(self) -> f32 {
        match self {
            Size::Small => 0.8,
            Size::Medium => 1.0,
            Size::Large => 1.25,
        }
    }
}

/// How big the rooms are, on top of the layout's own (Small / Medium / Large is how many rooms).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RoomSize {
    #[default]
    Normal,
    Large,
    ExtraLarge,
}

impl RoomSize {
    pub const ALL: [RoomSize; 3] = [RoomSize::Normal, RoomSize::Large, RoomSize::ExtraLarge];
    pub fn name(self) -> &'static str {
        match self {
            RoomSize::Normal => "Normal",
            RoomSize::Large => "Large",
            RoomSize::ExtraLarge => "Extra large",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            RoomSize::Normal => "normal",
            RoomSize::Large => "large",
            RoomSize::ExtraLarge => "extra_large",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// How much wider everything gets.
    pub fn scale(self) -> f32 {
        match self {
            RoomSize::Normal => 1.0,
            RoomSize::Large => 1.35,
            RoomSize::ExtraLarge => 1.7,
        }
    }
}

/// Grows a laid-out plan `k` times across (where every room is, and how big) and √k times in
/// height (rooms, raised floors, depths and the levels' spacing, so the levels still stack).
/// Corridors, stairs and bridges get longer by `k` but only √k wider.
pub fn grow(plan: &mut Plan, k: f32) {
    if (k - 1.0).abs() < 1e-3 {
        return;
    }
    let kh = k.sqrt();
    let s = |p: [f32; 2]| [p[0] * k, p[1] * k];
    for r in &mut plan.rooms {
        let passage = matches!(
            r.kind,
            RoomKind::Corridor | RoomKind::Stairs | RoomKind::Bridge
        );
        match &mut r.shape {
            Shape::Rect { c, half, .. } => {
                *c = s(*c);
                if passage {
                    // the long side is the way along
                    let long = if half[0] >= half[1] { 0 } else { 1 };
                    half[long] *= k;
                    half[1 - long] *= kh;
                } else {
                    *half = s(*half);
                }
            }
            Shape::Ngon { c, r, .. } | Shape::Round { c, r } => {
                *c = s(*c);
                *r *= if passage { kh } else { k };
            }
            Shape::Poly { pts } => {
                for p in pts.iter_mut() {
                    *p = s(*p);
                }
            }
            Shape::Path { pts, width } => {
                for p in pts.iter_mut() {
                    *p = s(*p);
                }
                *width *= if passage { kh } else { k };
            }
        }
        r.height *= kh;
        r.floor *= kh;
        r.depth *= kh;
    }
    for l in &mut plan.levels {
        l.y *= kh;
    }
}

/// Irregular rooms, `strength` 0..1 (0: none, a little: barely uneven, 1: deep alcoves and many
/// jogs): most rooms and halls get uneven walls, each its own way (pillared halls stay regular:
/// their rows of pillars follow the walls).
pub fn roughen(plan: &mut Plan, seed: u32, strength: f32) {
    if strength < 0.01 {
        return;
    }
    let mut rng = Rng::new(seed as u64 ^ 0x0dd5_4a9e);
    for r in &mut plan.rooms {
        let (pick, s, k) = (rng.f32(), rng.next_u32(), rng.range(0.7, 1.15));
        if pick < 0.8 && r.pillars == Pillars::None && r.can_be_irregular() {
            r.set_irregular(s, (strength * k).clamp(0.05, 1.0));
        }
    }
}

/// A one-click layout.
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// the style and mood it is built in unless you pick others
    pub style: &'static str,
    pub mood: &'static str,
    /// an underground map, or a realm with a sky
    pub realm: bool,
    pub icon: &'static str,
    build: fn(&mut Plan, &mut Rng, Size),
}

impl Preset {
    /// The style it is built in for an art (photoreal: the photoscanned counterpart, when there
    /// is one).
    pub fn style_for(&self, art: super::styles::Art) -> &'static str {
        match art {
            super::styles::Art::HandPainted => self.style,
            super::styles::Art::Photoreal => match self.style {
                "keep_stone" => "castle_keep",
                "cathedral_hp" => "gothic_cathedral",
                "sewer_hp" => "sewer_brick",
                "cave_rock" => "natural_cave",
                "clockwork_hp" => "clockwork_works",
                "nordic_hp" => "nordic_keep",
                "overgrown_hp" => "overgrown_ruins",
                _ => "castle_keep",
            },
        }
    }
    /// What its open-ceilinged rooms show overhead.
    pub fn sky(&self) -> super::model::SkyMode {
        use super::model::SkyMode;
        match self.id {
            "sun_temple" => SkyMode::Day,
            "ruined_fortress" => SkyMode::Night,
            _ => SkyMode::None,
        }
    }
    /// A name short enough for a tile.
    pub fn short(&self) -> &'static str {
        match self.id {
            "prison_keep" => "Prison",
            "sun_temple" => "Temple",
            "cathedral" => "Cathedral",
            "dwarven_halls" => "Dwarf Halls",
            "underground_city" => "Undercity",
            "great_caverns" => "Caverns",
            "goblin_cave" => "Goblin Cave",
            "spider_den" => "Spider Den",
            "sewers" => "Sewers",
            "mine" => "Mine",
            "clockwork" => "Clockwork",
            "frost_halls" => "Frost Halls",
            "infernal" => "Infernal",
            "ruined_fortress" => "Fortress",
            _ => self.name,
        }
    }
}

pub fn all() -> &'static [Preset] {
    use egui_phosphor::regular as icon;
    &[
        Preset {
            id: "crypt",
            name: "Crypt",
            blurb: "Vaulted burial chambers off a central passage, a tomb lord at the end",
            style: "crypt_stone",
            mood: "candlelit",
            realm: false,
            icon: icon::SKULL,
            build: crypt,
        },
        Preset {
            id: "catacombs",
            name: "Catacombs",
            blurb: "A maze of narrow bone-lined tunnels, ossuary chambers, two levels",
            style: "crypt_stone",
            mood: "catacombs",
            realm: false,
            icon: icon::GRID_FOUR,
            build: catacombs,
        },
        Preset {
            id: "prison_keep",
            name: "Prison Keep",
            blurb: "Cell blocks, guard rooms, an interrogation pit and a warden's hall",
            style: "keep_stone",
            mood: "prison",
            realm: false,
            icon: icon::LOCK,
            build: prison,
        },
        Preset {
            id: "sun_temple",
            name: "Sun Temple",
            blurb: "An axial jungle temple: stairs, a pillared hall, a courtyard open to the sky and a stepped altar",
            style: "sun_temple",
            mood: "golden",
            realm: false,
            icon: icon::SUN,
            build: temple,
        },
        Preset {
            id: "cathedral",
            name: "Gothic Cathedral",
            blurb: "A galleried nave, transept and apse under pointed vaults, a crypt below",
            style: "cathedral_hp",
            mood: "moonlit",
            realm: false,
            icon: icon::CASTLE_TURRET,
            build: cathedral,
        },
        Preset {
            id: "dwarven_halls",
            name: "Dwarven Forge Halls",
            blurb: "A galleried great hall, forges round lava, a deep chasm with a bridge",
            style: "dwarven_forge",
            mood: "forge",
            realm: false,
            icon: icon::FIRE,
            build: dwarven,
        },
        Preset {
            id: "underground_city",
            name: "Underground City",
            blurb: "A vast cavern with plazas, a chasm crossed by bridges, halls cut into the rock",
            style: "dwarven_forge",
            mood: "city",
            realm: false,
            icon: icon::BUILDINGS,
            build: city,
        },
        Preset {
            id: "great_caverns",
            name: "Great Caverns",
            blurb: "Huge natural caverns joined by winding tunnels, underground pools",
            style: "cave_rock",
            mood: "cave",
            realm: false,
            icon: icon::MOUNTAINS,
            build: caverns,
        },
        Preset {
            id: "goblin_cave",
            name: "Goblin Cave",
            blurb: "A warren of rough caves and tunnels, a chieftain's den",
            style: "cave_rock",
            mood: "torchlit",
            realm: false,
            icon: icon::FLAME,
            build: goblin,
        },
        Preset {
            id: "spider_den",
            name: "Spider Den",
            blurb: "Webbed caverns and round chambers round deep shafts, three levels",
            style: "spider_den",
            mood: "spider",
            realm: false,
            icon: icon::BUG,
            build: spider,
        },
        Preset {
            id: "sewers",
            name: "Old Sewers",
            blurb: "Brick tunnels with water channels, junction rooms and a cistern",
            style: "sewer_hp",
            mood: "damp",
            realm: false,
            icon: icon::DROP,
            build: sewers,
        },
        Preset {
            id: "mine",
            name: "Abandoned Mine",
            blurb: "Winding drifts, ore chambers and a shaft down to the deep workings",
            style: "cave_rock",
            mood: "mine",
            realm: false,
            icon: icon::HAMMER,
            build: mine,
        },
        Preset {
            id: "clockwork",
            name: "Clockwork Works",
            blurb: "Machine halls, catwalks over pits, a reactor core",
            style: "clockwork_hp",
            mood: "industrial",
            realm: false,
            icon: icon::GEAR,
            build: works,
        },
        Preset {
            id: "frost_halls",
            name: "Frozen Halls",
            blurb: "Ice-glazed halls and frozen caverns round a frost throne",
            style: "frost_halls",
            mood: "frost",
            realm: false,
            icon: icon::SNOWFLAKE,
            build: frost,
        },
        Preset {
            id: "infernal",
            name: "Infernal Halls",
            blurb: "Scorched halls over rivers of lava, a demon's arena",
            style: "infernal_halls",
            mood: "infernal",
            realm: false,
            icon: icon::FIRE_SIMPLE,
            build: infernal,
        },
        Preset {
            id: "ruined_fortress",
            name: "Ruined Fortress",
            blurb: "A moonlit courtyard open to the sky, a galleried great hall, a chasm crossed by a bridge, windows and alcoves",
            style: "keep_stone",
            mood: "moonlit",
            realm: false,
            icon: icon::CASTLE_TURRET,
            build: fortress,
        },
        Preset {
            id: "empty",
            name: "Empty",
            blurb: "One room to start from: draw the rest yourself",
            style: "crypt_stone",
            mood: "torchlit",
            realm: false,
            icon: icon::SQUARE,
            build: empty,
        },
    ]
}

/// A realm: a dungeon that is a world of its own under a sky (another dimension), generated like a
/// new world and built with the world's tools.
pub struct RealmPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    pub world: crate::terrain::worldgen::WorldPreset,
    pub climate: crate::biomegen::Climate,
    pub look: crate::look::LookPreset,
    pub hour: f32,
    /// every bit of land this one biome (None: the climate's mix)
    pub only: Option<crate::biomes::BiomePreset>,
    pub icon: &'static str,
}

pub fn realms() -> &'static [RealmPreset] {
    use crate::biomegen::Climate;
    use crate::biomes::BiomePreset as B;
    use crate::look::LookPreset as L;
    use crate::terrain::worldgen::WorldPreset as W;
    use egui_phosphor::regular as icon;
    &[
        RealmPreset {
            id: "desert_realm",
            name: "Desert Realm",
            blurb: "Endless dunes and mesas under a blazing sky",
            world: W::Continent,
            climate: Climate::Arid,
            look: L::GoldenHour,
            hour: 16.5,
            only: Some(B::Desert),
            icon: icon::SUN_HORIZON,
        },
        RealmPreset {
            id: "frozen_realm",
            name: "Frozen Realm",
            blurb: "An island of ice and snow in a cold sea",
            world: W::Island,
            climate: Climate::Cold,
            look: L::Nordic,
            hour: 11.0,
            only: Some(B::Tundra),
            icon: icon::SNOWFLAKE,
        },
        RealmPreset {
            id: "jungle_realm",
            name: "Jungle Realm",
            blurb: "Steaming jungle islands and lagoons",
            world: W::Archipelago,
            climate: Climate::Tropical,
            look: L::Enchanted,
            hour: 9.5,
            only: None,
            icon: icon::TREE_PALM,
        },
        RealmPreset {
            id: "ember_realm",
            name: "Ember Realm",
            blurb: "A burning land under a red sky",
            world: W::Island,
            climate: Climate::Arid,
            look: L::Hellfire,
            hour: 18.6,
            only: Some(B::Volcanic),
            icon: icon::FIRE,
        },
        RealmPreset {
            id: "twilight_realm",
            name: "Twilight Realm",
            blurb: "Rolling dreamlands in an endless violet dusk",
            world: W::Plains,
            climate: Climate::Temperate,
            look: L::Lavender,
            hour: 19.2,
            only: None,
            icon: icon::MOON_STARS,
        },
        RealmPreset {
            id: "wild_realm",
            name: "Wild Realm",
            blurb: "A whole untamed continent of every biome",
            world: W::Continent,
            climate: Climate::Varied,
            look: L::Natural,
            hour: 14.0,
            only: None,
            icon: icon::GLOBE_HEMISPHERE_EAST,
        },
        // (its theme gives it its own sky and biomes: `compound::Theme::realm`)
        RealmPreset {
            id: super::compound::LAYOUT,
            name: "Temple Compound",
            blurb: "An outdoor raid-style temple city laid out for you: a walled valley, a hub, raised boss courts, lakes and an island temple, a processional way to the final court",
            world: W::Island,
            climate: Climate::Tropical,
            look: L::Enchanted,
            hour: 9.5,
            only: Some(B::Jungle),
            icon: icon::BANK,
        },
    ]
}

pub fn realm(id: &str) -> Option<&'static RealmPreset> {
    realms()
        .iter()
        .find(|p| p.id == id || p.name.eq_ignore_ascii_case(id))
}

pub fn get(id: &str) -> Option<&'static Preset> {
    all()
        .iter()
        .find(|p| p.id == id || p.name.eq_ignore_ascii_case(id))
}

/// Lays out a fresh plan: the preset's rooms (grown to `rooms`, most of them made uneven by
/// `irregular` 0..1, on `style`'s ceilings), from `seed`.
pub fn generate(
    p: &Preset,
    size: Size,
    rooms: RoomSize,
    irregular: f32,
    seed: u32,
    style: &str,
) -> Plan {
    let mut plan = Plan {
        seed,
        ..Plan::default()
    };
    let mut rng = Rng::new(seed as u64 ^ 0xd06e_0000);
    (p.build)(&mut plan, &mut rng, size);
    grow(&mut plan, rooms.scale());
    roughen(&mut plan, seed, irregular);
    let st = styles::get(style);
    for r in &mut plan.rooms {
        if r.kind != RoomKind::Cavern && r.ceiling != Ceiling::Open && r.ceiling != Ceiling::Rock {
            r.ceiling = st.ceiling_for(r.kind);
        }
    }
    plan.prune_levels();
    plan
}

/// Re-rolls a plan: unlocked rooms are laid out afresh; locked ones stay where they are.
pub fn shuffle(
    old: &Plan,
    p: &Preset,
    size: Size,
    rooms: RoomSize,
    irregular: f32,
    seed: u32,
    style: &str,
) -> Plan {
    let fresh = generate(p, size, rooms, irregular, seed, style);
    let locked: Vec<Room> = old.rooms.iter().filter(|r| r.locked).cloned().collect();
    if locked.is_empty() {
        return fresh;
    }
    let mut out = Plan {
        levels: old.levels.clone(),
        rooms: vec![],
        next_id: 0,
        seed,
    };
    for r in locked {
        out.rooms.push(r);
    }
    out.next_id = out.rooms.iter().map(|r| r.id).max().unwrap_or(0);
    for mut r in fresh.rooms {
        // fresh rooms that would sit on a locked one are dropped
        let c = r.shape.centre();
        if out
            .rooms
            .iter()
            .any(|l| l.level == r.level && l.shape.sdf(c) > -2.0)
        {
            continue;
        }
        r.id = 0;
        out.add(r);
    }
    for l in fresh.levels {
        if !out.levels.iter().any(|x| x.id == l.id) {
            out.levels.push(l);
        }
    }
    out.levels.sort_by_key(|l| -l.id);
    out
}

// ------------------------------------------------------------------ building blocks

fn add(plan: &mut Plan, kind: RoomKind, level: i32, shape: Shape) -> u32 {
    plan.add(Room::new(0, kind, level, shape))
}

fn set(plan: &mut Plan, id: u32, f: impl FnOnce(&mut Room)) {
    if let Some(r) = plan.room_mut(id) {
        f(r);
    }
}

fn rect(c: Vec2, w: f32, d: f32) -> Shape {
    Shape::rect(c, Vec2::new(w * 0.5, d * 0.5))
}

/// A corridor from `a` to `b` with one bend (horizontal first, or vertical first).
fn corridor(plan: &mut Plan, level: i32, a: Vec2, b: Vec2, width: f32, x_first: bool) -> u32 {
    let mid = if x_first {
        Vec2::new(b.x, a.y)
    } else {
        Vec2::new(a.x, b.y)
    };
    let pts: Vec<Vec2> = if (a.x - b.x).abs() < 0.5 || (a.y - b.y).abs() < 0.5 {
        vec![a, b]
    } else {
        vec![a, mid, b]
    };
    add(plan, RoomKind::Corridor, level, Shape::path(&pts, width))
}

/// A stairway from `a` (on `from`) to `b` (on `to`).
fn stairs(plan: &mut Plan, from: i32, to: i32, a: Vec2, b: Vec2, width: f32) -> u32 {
    plan.ensure_level(to);
    let id = add(plan, RoomKind::Stairs, from, Shape::path(&[a, b], width));
    set(plan, id, |r| r.to_level = Some(to));
    id
}

/// A hall two storeys high on `level`: a gallery round it at the floor of the level above
/// (railings and an arcade of posts where it overlooks the hall), and a stair up along one long
/// side to it. Rooms of the level above that reach its walls open onto the gallery. At least
/// 18 × 24 m (the stair needs the run). Under a curved `ceiling` the hall rises until its vault
/// springs above the gallery's headroom. Returns the hall.
pub fn gallery_hall(plan: &mut Plan, level: i32, c: Vec2, w: f32, d: f32, ceiling: Ceiling) -> u32 {
    let (w, d) = (w.max(18.0), d.max(24.0));
    let up = level + 1;
    plan.ensure_level(up);
    let rise = plan.level_y(up) - plan.level_y(level);
    let inr = w.min(d) * 0.5;
    let curve = match ceiling {
        Ceiling::Pointed => inr * 1.35,
        Ceiling::Stepped => inr * 0.8,
        Ceiling::Vault => inr,
        _ => 0.0,
    };
    let height = (rise + 3.8 + curve).max(rise + 8.0);
    let hall = add(plan, RoomKind::Hall, level, rect(c, w, d));
    set(plan, hall, |r| {
        r.height = height;
        r.pillars = Pillars::None;
        r.ceiling = ceiling;
        r.name = "Galleried hall".into();
    });
    let g = (w.min(d) * 0.15).clamp(2.6, 3.6);
    let (hx, hz) = (w * 0.5, d * 0.5);
    // the gallery: the two ends full width, the long sides between them
    for (cc, sw, sd) in [
        (Vec2::new(c.x, c.y - hz + g * 0.5), w, g),
        (Vec2::new(c.x, c.y + hz - g * 0.5), w, g),
        (Vec2::new(c.x - hx + g * 0.5, c.y), g, d - 2.0 * g + 0.2),
        (Vec2::new(c.x + hx - g * 0.5, c.y), g, d - 2.0 * g + 0.2),
    ] {
        let b = add(plan, RoomKind::Balcony, up, rect(cc, sw, sd));
        set(plan, b, |r| r.name = "Gallery".into());
    }
    // the stair: up along the inner edge of one long side, landing on the far end's gallery
    let sw = 2.6;
    let x = c.x + hx - g - sw * 0.5 - 0.5;
    let s = stairs(
        plan,
        level,
        up,
        Vec2::new(x, c.y + hz - g - 0.8),
        Vec2::new(x, c.y - hz + g - 0.2),
        sw,
    );
    set(plan, s, |r| {
        r.ceiling = Ceiling::Open;
        r.name = "Gallery stair".into();
    });
    hall
}

/// A window (`true`) or an alcove cut into the wall of the room on `level` nearest to `p` (within
/// 3 m): a small arched opening as a room of its own. A window goes through to the room beyond
/// the wall when one is within 4 m (else it's a blind window); an alcove is a recess into the
/// rock. None when no wall is near.
pub fn wall_opening(plan: &Plan, level: i32, p: Vec2, window: bool) -> Option<Room> {
    // the nearest wall: a point on a room's outline, along it and out of it
    let mut best: Option<(f32, &Room, Vec2, Vec2, Vec2)> = None;
    for r in plan.rooms.iter().filter(|r| {
        r.level == level && r.kind.is_space() && !r.kind.is_pit() && r.kind != RoomKind::Pool
    }) {
        let pts = r.shape.outline();
        let n = pts.len();
        for k in 0..n {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let q = a + ab * t;
            let dist = q.distance(p);
            if dist < 3.0 && best.as_ref().is_none_or(|x| dist < x.0) {
                let along = ab.normalize_or_zero();
                let mut out = along.perp();
                if r.shape.sdf(q + out * 0.1) > 0.0 {
                    out = -out;
                }
                best = Some((dist, r, q, along, out));
            }
        }
    }
    let (_, r, q, along, out) = best?;
    let yaw = along.y.atan2(along.x);
    let base = plan.level_y(level) + r.floor;
    let (shape, floor, height, name) = if window {
        let (sill, head) = (base + 1.0, base + 2.9);
        // the room beyond: its air spans the window's height
        let beyond = (1..=40).map(|k| k as f32 * 0.1).find(|s| {
            let x = q + out * *s;
            plan.rooms.iter().any(|o| {
                o.id != r.id
                    && o.kind.is_space()
                    && o.shape.sdf(x) > 0.05
                    && plan.level_y(o.level) + o.floor <= sill + 0.4
                    && plan.level_y(o.level) + o.floor + o.height >= head
            })
        });
        let depth = beyond.map(|s| s + 0.35).unwrap_or(0.9);
        let c = q + out * (depth * 0.5 - 0.25);
        (
            Shape::Rect {
                c: c.to_array(),
                half: [0.7, depth * 0.5 + 0.25],
                yaw,
            },
            r.floor + 1.0,
            1.9,
            "Window",
        )
    } else {
        let c = q + out * 0.5;
        (
            Shape::Rect {
                c: c.to_array(),
                half: [0.95, 0.6],
                yaw,
            },
            r.floor + 0.45,
            2.4,
            "Alcove",
        )
    };
    let mut o = Room::new(0, RoomKind::Room, level, shape);
    o.floor = floor;
    o.height = height;
    o.ceiling = Ceiling::Vault;
    o.style = r.style.clone();
    o.name = name.into();
    Some(o)
}

/// The wall of a room on `level` nearest `p` (within `reach` m): the room, the wall's point and
/// its outward direction.
pub fn nearest_wall(plan: &Plan, level: i32, p: Vec2, reach: f32) -> Option<(u32, Vec2, Vec2)> {
    let mut best: Option<(f32, u32, Vec2, Vec2)> = None;
    for r in plan.rooms.iter().filter(|r| {
        r.level == level && r.kind.is_space() && !r.kind.is_pit() && r.kind != RoomKind::Pool
    }) {
        let pts = r.shape.outline();
        let n = pts.len();
        for k in 0..n {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let q = a + ab * t;
            let dist = q.distance(p);
            if dist < reach && best.as_ref().is_none_or(|x| dist < x.0) {
                let mut out = ab.normalize_or_zero().perp();
                if r.shape.sdf(q + out * 0.1) > 0.0 {
                    out = -out;
                }
                best = Some((dist, r.id, q, out));
            }
        }
    }
    best.map(|(_, id, q, out)| (id, q, out))
}

/// The opening a door on a plain wall needs (room `id`'s wall at `at`, facing `out`): a short
/// passage through to the room behind it when one is within 6 m on its floor, else a shallow
/// niche (a door set in the wall). Returns the new room and where the door goes.
pub fn door_opening(plan: &Plan, id: u32, at: Vec2, out: Vec2) -> Option<(Room, Vec3)> {
    let r = plan.room(id)?;
    let base = plan.level_y(r.level) + r.floor;
    let along = out.perp();
    let yaw = along.y.atan2(along.x);
    let beyond = (2..=60).map(|k| k as f32 * 0.1).find(|s| {
        let x = at + out * *s;
        plan.rooms.iter().any(|o| {
            o.id != id
                && o.level == r.level
                && o.kind.is_space()
                && !o.kind.is_pit()
                && o.shape.sdf(x) > 0.4
                && (plan.level_y(o.level) + o.floor - base).abs() < 0.8
        })
    });
    let (shape, kind, name) = match beyond {
        Some(s) => (
            Shape::path(&[at - out * 0.8, at + out * (s + 0.8)], 1.9),
            RoomKind::Corridor,
            "Doorway",
        ),
        None => (
            Shape::Rect {
                c: (at + out * 0.25).to_array(),
                half: [0.95, 0.45],
                yaw,
            },
            RoomKind::Room,
            "Door niche",
        ),
    };
    let mut o = Room::new(0, kind, r.level, shape);
    o.floor = r.floor;
    o.height = 2.7f32.min(r.height - 0.3).max(2.2);
    o.ceiling = Ceiling::Vault;
    o.style = r.style.clone();
    o.name = name.into();
    Some((o, Vec3::new(at.x, base, at.y)))
}

/// Rooms on a grid joined by corridors along a random spanning tree (plus a loop or two): the
/// layout of keeps, crypts, tombs, temples' side wings. Returns the room ids in visiting order
/// (the entrance first, the farthest last) and their centres.
#[allow(clippy::too_many_arguments)]
fn branching(
    plan: &mut Plan,
    rng: &mut Rng,
    level: i32,
    origin: Vec2,
    n: usize,
    cell: f32,
    room: (f32, f32),
    corridor_w: f32,
    kinds: &dyn Fn(usize, &mut Rng) -> RoomKind,
) -> Vec<(u32, Vec2)> {
    use std::collections::HashMap;
    let mut cells: HashMap<(i32, i32), usize> = HashMap::new();
    let mut order: Vec<(i32, i32)> = vec![(0, 0)];
    let mut parent: Vec<Option<usize>> = vec![None];
    cells.insert((0, 0), 0);
    let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let mut guard = 0;
    while order.len() < n && guard < 4000 {
        guard += 1;
        // grow from a random room, favouring the newest (long branches)
        let from = if rng.f32() < 0.55 {
            order.len() - 1
        } else {
            rng.below(order.len() as u32) as usize
        };
        let (cx, cz) = order[from];
        let (dx, dz) = dirs[rng.below(4) as usize];
        let next = (cx + dx, cz + dz);
        // keep away from the entrance's south (it's where you come in)
        if next.1 < 0 || cells.contains_key(&next) {
            continue;
        }
        cells.insert(next, order.len());
        order.push(next);
        parent.push(Some(from));
    }
    let mut out = Vec::with_capacity(order.len());
    for (i, (gx, gz)) in order.iter().enumerate() {
        let jitter = Vec2::new(rng.range(-0.12, 0.12), rng.range(-0.12, 0.12)) * cell;
        let c = origin + Vec2::new(*gx as f32, *gz as f32) * cell + jitter;
        let kind = kinds(i, rng);
        let (w, d) = (rng.range(room.0, room.1), rng.range(room.0, room.1));
        let shape = match kind {
            RoomKind::Cavern => Shape::Round {
                c: c.to_array(),
                r: (w + d) * 0.25,
            },
            _ if i > 0 && rng.f32() < 0.18 => Shape::Ngon {
                c: c.to_array(),
                r: w * 0.55,
                sides: 8,
                yaw: 0.0,
            },
            _ => rect(c, w, d),
        };
        let id = add(plan, kind, level, shape);
        out.push((id, c));
    }
    // corridors along the tree, then a couple of loops between neighbours
    let mut joined = std::collections::HashSet::new();
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            corridor(
                plan,
                level,
                out[*p].1,
                out[i].1,
                corridor_w,
                rng.f32() < 0.5,
            );
            joined.insert((i.min(*p), i.max(*p)));
        }
    }
    let loops = (order.len() / 5).max(1);
    let mut made = 0;
    for _ in 0..40 {
        if made >= loops {
            break;
        }
        let i = rng.below(order.len() as u32) as usize;
        let (gx, gz) = order[i];
        let (dx, dz) = dirs[rng.below(4) as usize];
        if let Some(&j) = cells.get(&(gx + dx, gz + dz))
            && !joined.contains(&(i.min(j), i.max(j)))
        {
            corridor(plan, level, out[i].1, out[j].1, corridor_w, true);
            joined.insert((i.min(j), i.max(j)));
            made += 1;
        }
    }
    // farthest from the entrance last (by the tree)
    let mut depth = vec![0usize; order.len()];
    for i in 1..order.len() {
        depth[i] = depth[parent[i].unwrap_or(0)] + 1;
    }
    let mut idx: Vec<usize> = (0..order.len()).collect();
    idx.sort_by_key(|i| depth[*i]);
    idx.into_iter().map(|i| out[i]).collect()
}

/// Makes the room `id` a boss room: bigger, a dais at its far end, braziers (by role).
fn make_boss(plan: &mut Plan, id: u32, scale: f32, toward: Vec2) {
    let Some(r) = plan.room(id).cloned() else {
        return;
    };
    let c = r.shape.centre();
    let big = (14.0 * scale).max(12.0);
    set(plan, id, |r| {
        r.kind = RoomKind::Hall;
        r.role = Role::Boss;
        r.shape = Shape::Ngon {
            c: c.to_array(),
            r: big,
            sides: 8,
            yaw: 0.0,
        };
        r.height = 13.0;
        r.pillars = Pillars::Ring;
    });
    let dir = (c - toward).normalize_or(Vec2::Y);
    let dais = add(
        plan,
        RoomKind::Dais,
        r.level,
        Shape::Round {
            c: (c + dir * big * 0.45).to_array(),
            r: big * 0.28,
        },
    );
    set(plan, dais, |d| d.height = 1.2);
}

fn entrance(plan: &mut Plan, id: u32) {
    set(plan, id, |r| {
        r.role = Role::Entrance;
        r.name = "Entrance".into();
    });
}

// ------------------------------------------------------------------ layouts

fn empty(plan: &mut Plan, _: &mut Rng, _: Size) {
    let id = add(plan, RoomKind::Room, 0, rect(Vec2::ZERO, 16.0, 16.0));
    entrance(plan, id);
}

fn crypt(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms();
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        26.0 * k,
        (10.0, 16.0),
        3.6,
        &|i, rng| {
            if i > 0 && rng.f32() < 0.25 {
                RoomKind::Hall
            } else {
                RoomKind::Room
            }
        },
    );
    entrance(plan, rooms[0].0);
    let roles = [
        Role::Crypt,
        Role::Crypt,
        Role::Shrine,
        Role::Treasure,
        Role::Crypt,
        Role::Ritual,
    ];
    for (i, (id, _)) in rooms.iter().enumerate().skip(1) {
        set(plan, *id, |r| r.role = roles[i % roles.len()]);
    }
    let (last, _) = *rooms.last().unwrap();
    make_boss(plan, last, k, rooms[0].1);
}

fn catacombs(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = (size.rooms() as f32 * 1.4) as i32;
    let step = 14.0;
    let side = ((n as f32).sqrt().ceil() as i32 + 1).max(3);
    // a grid of narrow tunnels with burial niches and small ossuaries where they cross
    for i in 0..=side {
        let x = i as f32 * step;
        if rng.f32() < 0.85 {
            add(
                plan,
                RoomKind::Corridor,
                0,
                Shape::path(&[Vec2::new(x, 0.0), Vec2::new(x, side as f32 * step)], 2.8),
            );
        }
        let z = i as f32 * step;
        if rng.f32() < 0.85 {
            add(
                plan,
                RoomKind::Corridor,
                0,
                Shape::path(&[Vec2::new(0.0, z), Vec2::new(side as f32 * step, z)], 2.8),
            );
        }
    }
    for i in 0..=side {
        for j in 0..=side {
            let c = Vec2::new(i as f32, j as f32) * step;
            if rng.f32() < 0.22 {
                let id = add(
                    plan,
                    RoomKind::Room,
                    0,
                    Shape::Ngon {
                        c: c.to_array(),
                        r: 4.5 * k,
                        sides: 8,
                        yaw: 0.0,
                    },
                );
                set(plan, id, |r| r.role = Role::Crypt);
            }
        }
    }
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(side as f32 * step * 0.5, -10.0), 12.0, 10.0),
    );
    entrance(plan, ent);
    corridor(
        plan,
        0,
        Vec2::new(side as f32 * step * 0.5, -6.0),
        Vec2::new(side as f32 * step * 0.5, 0.0),
        3.0,
        true,
    );
    // the lower level: an ossuary hall
    let far = Vec2::new(side as f32 * step, side as f32 * step);
    stairs(plan, 0, -1, far, far + Vec2::new(0.0, 26.0), 3.2);
    let hall = add(
        plan,
        RoomKind::Hall,
        -1,
        rect(far + Vec2::new(0.0, 40.0), 26.0 * k, 22.0 * k),
    );
    set(plan, hall, |r| {
        r.role = Role::Boss;
        r.pillars = Pillars::Grid;
        r.height = 9.0;
    });
}

fn prison(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let blocks = match size {
        Size::Small => 1,
        Size::Medium => 2,
        Size::Large => 3,
    };
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -8.0), 14.0, 10.0),
    );
    entrance(plan, ent);
    let mut z = 0.0;
    for b in 0..blocks {
        // a long cell block: a corridor with cells both sides
        let len = 44.0 * k;
        add(
            plan,
            RoomKind::Corridor,
            0,
            Shape::path(&[Vec2::new(0.0, z), Vec2::new(0.0, z + len)], 4.0),
        );
        let cells = (len / 7.0) as i32;
        for c in 0..cells {
            for s in [-1.0f32, 1.0] {
                let cz = z + 4.0 + c as f32 * 7.0;
                let id = add(
                    plan,
                    RoomKind::Room,
                    0,
                    rect(Vec2::new(s * 6.2, cz), 6.0, 5.6),
                );
                set(plan, id, |r| {
                    r.role = Role::Prison;
                    r.height = 4.2;
                });
            }
        }
        z += len;
        // a guard room
        let g = add(
            plan,
            RoomKind::Room,
            0,
            rect(Vec2::new(0.0, z + 8.0), 16.0, 14.0),
        );
        set(plan, g, |r| {
            r.role = if b % 2 == 0 {
                Role::Barracks
            } else {
                Role::Storage
            }
        });
        z += 16.0;
    }
    // the interrogation pit off to the side, and the warden's hall at the end
    let side = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(24.0, z - 8.0), 16.0, 16.0),
    );
    set(plan, side, |r| r.role = Role::Ritual);
    add(
        plan,
        RoomKind::Pit,
        0,
        Shape::Round {
            c: [24.0, z - 8.0],
            r: 3.5,
        },
    );
    corridor(
        plan,
        0,
        Vec2::new(0.0, z - 8.0),
        Vec2::new(18.0, z - 8.0),
        3.4,
        true,
    );
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, z - 2.0), Vec2::new(0.0, z + 10.0)], 4.0),
    );
    let hall = add(
        plan,
        RoomKind::Hall,
        0,
        rect(Vec2::new(0.0, z + 26.0), 30.0 * k, 26.0 * k),
    );
    set(plan, hall, |r| {
        r.role = Role::Boss;
        r.pillars = Pillars::Rows;
    });
    let dais = add(
        plan,
        RoomKind::Dais,
        0,
        rect(Vec2::new(0.0, z + 34.0 * k), 10.0, 5.0),
    );
    set(plan, dais, |r| r.height = 0.9);
    let _ = rng;
}

fn temple(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    // an axial temple: a stair down from the entrance, a long pillared hall, a courtyard, the altar
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, 0.0), 16.0, 12.0),
    );
    entrance(plan, ent);
    stairs(plan, 0, -1, Vec2::new(0.0, 5.0), Vec2::new(0.0, 32.0), 6.0);
    let hall = add(
        plan,
        RoomKind::Hall,
        -1,
        rect(Vec2::new(0.0, 56.0 * k), 22.0, 48.0 * k),
    );
    set(plan, hall, |r| {
        r.pillars = Pillars::Rows;
        r.height = 14.0;
        r.name = "Hall of Kings".into();
    });
    let court_c = Vec2::new(0.0, (56.0 + 40.0) * k);
    let court = add(plan, RoomKind::Hall, -1, rect(court_c, 34.0 * k, 26.0 * k));
    set(plan, court, |r| {
        r.ceiling = Ceiling::Open;
        r.height = 18.0;
        r.role = Role::Shrine;
    });
    add(plan, RoomKind::Pool, -1, rect(court_c, 12.0, 8.0));
    // side chambers either side of the hall, symmetric
    let n = (size.rooms() / 3).max(1);
    for i in 0..n {
        let z = 40.0 * k + i as f32 * (40.0 * k / n as f32);
        for s in [-1.0f32, 1.0] {
            let c = Vec2::new(s * 26.0, z);
            let id = add(plan, RoomKind::Room, -1, rect(c, 12.0, 12.0));
            set(plan, id, |r| {
                r.role = [Role::Treasure, Role::Shrine, Role::Library][(i + (s > 0.0) as usize) % 3]
            });
            corridor(plan, -1, Vec2::new(s * 10.0, z), c, 3.6, true);
        }
    }
    // the altar: a great stepped dais under a tall roof
    let altar_c = court_c + Vec2::new(0.0, 36.0 * k);
    corridor(plan, -1, court_c, altar_c, 6.0, true);
    let altar = add(
        plan,
        RoomKind::Hall,
        -1,
        Shape::Ngon {
            c: altar_c.to_array(),
            r: 18.0 * k,
            sides: 8,
            yaw: 0.0,
        },
    );
    set(plan, altar, |r| {
        r.role = Role::Boss;
        r.height = 16.0;
        r.pillars = Pillars::Ring;
        r.name = "Altar of the Sun".into();
    });
    let dais = add(
        plan,
        RoomKind::Dais,
        -1,
        rect(altar_c + Vec2::new(0.0, 4.0), 12.0, 10.0),
    );
    set(plan, dais, |r| r.height = 2.1);
    let _ = rng;
}

/// A ruined fortress: a gatehouse into a courtyard open to the sky, a galleried great hall with
/// chambers off its gallery, towers with arched windows and alcoves, a chasm split through the
/// rock behind it crossed by a bridge to the keep.
fn fortress(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let gate = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -8.0), 12.0, 10.0),
    );
    entrance(plan, gate);
    set(plan, gate, |r| r.name = "Gatehouse".into());
    // the courtyard: open to the sky, its walls high
    let court_c = Vec2::new(0.0, 22.0);
    let court = add(plan, RoomKind::Hall, 0, rect(court_c, 40.0 * k, 34.0));
    set(plan, court, |r| {
        r.ceiling = Ceiling::Open;
        r.height = 16.0;
        r.pillars = Pillars::None;
        r.name = "Courtyard".into();
    });
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, -3.0), Vec2::new(0.0, 6.0)], 5.0),
    );
    let well = add(
        plan,
        RoomKind::Pool,
        0,
        Shape::Round {
            c: court_c.to_array(),
            r: 2.6,
        },
    );
    set(plan, well, |r| r.floor = -2.4);
    // the great hall beyond the courtyard: two storeys and a gallery
    let hall_c = Vec2::new(0.0, 22.0 + 17.0 + 4.0 + 18.0 * k);
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(
            &[
                Vec2::new(0.0, 22.0 + 15.0),
                Vec2::new(0.0, hall_c.y - 12.0 * k),
            ],
            6.0,
        ),
    );
    let hall = gallery_hall(plan, 0, hall_c, 26.0 * k, 34.0 * k, Ceiling::Beams);
    set(plan, hall, |r| {
        r.name = "Great Hall".into();
        r.role = Role::Throne;
    });
    for s in [-1.0f32, 1.0] {
        let id = add(
            plan,
            RoomKind::Room,
            1,
            rect(Vec2::new(s * (13.0 * k + 5.0), hall_c.y), 10.0, 12.0),
        );
        set(plan, id, |r| {
            r.height = 5.0;
            r.role = if s < 0.0 {
                Role::Library
            } else {
                Role::Barracks
            };
            r.name = "Gallery chamber".into();
        });
    }
    // towers at the courtyard's corners: windows over the courtyard, alcoves inside
    for s in [-1.0f32, 1.0] {
        let tc = Vec2::new(s * (20.0 * k + 9.0), court_c.y - 10.0);
        let tower = add(
            plan,
            RoomKind::Room,
            0,
            Shape::Ngon {
                c: tc.to_array(),
                r: 7.0,
                sides: 8,
                yaw: 0.0,
            },
        );
        set(plan, tower, |r| {
            r.height = 9.0;
            r.role = Role::Storage;
            r.name = "Tower".into();
        });
        corridor(plan, 0, tc, Vec2::new(s * 19.0 * k, tc.y), 3.6, true);
        if let Some(w) = wall_opening(plan, 0, Vec2::new(s * 20.0 * k, court_c.y + 6.0), true) {
            plan.add(w);
        }
        if let Some(a) = wall_opening(plan, 0, tc + Vec2::new(0.0, 7.2), false) {
            plan.add(a);
        }
    }
    // behind the hall the rock splits: a chasm, a bridge over it to the keep
    let chasm_z = hall_c.y + 17.0 * k + 14.0;
    let chasm = add(
        plan,
        RoomKind::Chasm,
        0,
        Shape::path(
            &[
                Vec2::new(-34.0 * k, chasm_z - 3.0),
                Vec2::new(-8.0, chasm_z + 2.0),
                Vec2::new(12.0, chasm_z - 2.0),
                Vec2::new(34.0 * k, chasm_z + 3.0),
            ],
            9.0,
        ),
    );
    set(plan, chasm, |r| {
        r.depth = 45.0;
        r.height = 12.0;
        r.name = "The Rift".into();
    });
    let ledge = add(
        plan,
        RoomKind::Cavern,
        0,
        Shape::path(
            &[
                Vec2::new(-34.0 * k, chasm_z - 3.0),
                Vec2::new(-8.0, chasm_z + 2.0),
                Vec2::new(12.0, chasm_z - 2.0),
                Vec2::new(34.0 * k, chasm_z + 3.0),
            ],
            22.0,
        ),
    );
    set(plan, ledge, |r| {
        r.height = 14.0;
        r.ceiling = Ceiling::Rock;
        r.name = "Rift cave".into();
    });
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(
            &[
                Vec2::new(0.0, hall_c.y + 16.0 * k),
                Vec2::new(0.0, chasm_z - 8.0),
            ],
            5.0,
        ),
    );
    add(
        plan,
        RoomKind::Bridge,
        0,
        Shape::path(
            &[Vec2::new(0.0, chasm_z - 8.0), Vec2::new(0.0, chasm_z + 8.0)],
            3.4,
        ),
    );
    let keep_c = Vec2::new(0.0, chasm_z + 26.0);
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(
            &[
                Vec2::new(0.0, chasm_z + 7.0),
                Vec2::new(0.0, keep_c.y - 8.0),
            ],
            5.0,
        ),
    );
    let keep = add(
        plan,
        RoomKind::Hall,
        0,
        Shape::Ngon {
            c: keep_c.to_array(),
            r: 13.0 * k,
            sides: 8,
            yaw: 0.0,
        },
    );
    set(plan, keep, |r| {
        r.role = Role::Boss;
        r.height = 14.0;
        r.pillars = Pillars::Ring;
        r.name = "The Keep".into();
    });
    let _ = rng;
}

fn cathedral(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -6.0), 14.0, 10.0),
    );
    entrance(plan, ent);
    // the nave, the crossing and transepts, the apse
    let nave_len = 60.0 * k;
    // the nave: two storeys, a gallery round it (the triforium) with chambers off it
    let nave = gallery_hall(
        plan,
        0,
        Vec2::new(0.0, nave_len * 0.5),
        22.0,
        nave_len,
        Ceiling::Pointed,
    );
    set(plan, nave, |r| r.name = "Nave".into());
    for s in [-1.0f32, 1.0] {
        for z in [nave_len * 0.3, nave_len * 0.7] {
            let id = add(
                plan,
                RoomKind::Room,
                1,
                rect(Vec2::new(s * 15.0, z), 9.0, 8.0),
            );
            set(plan, id, |r| {
                r.height = 4.5;
                r.role = Role::Library;
                r.name = "Triforium chamber".into();
            });
        }
    }
    let cross_z = nave_len + 12.0;
    let tr = add(
        plan,
        RoomKind::Hall,
        0,
        rect(Vec2::new(0.0, cross_z), 56.0 * k, 20.0),
    );
    set(plan, tr, |r| {
        r.height = 22.0;
        r.name = "Transept".into();
    });
    let apse = add(
        plan,
        RoomKind::Hall,
        0,
        Shape::Round {
            c: [0.0, cross_z + 16.0],
            r: 13.0,
        },
    );
    set(plan, apse, |r| {
        r.height = 20.0;
        r.role = Role::Boss;
        r.name = "Apse".into();
    });
    let dais = add(
        plan,
        RoomKind::Dais,
        0,
        Shape::Round {
            c: [0.0, cross_z + 18.0],
            r: 6.0,
        },
    );
    set(plan, dais, |r| r.height = 1.2);
    // side aisles with chapels
    for s in [-1.0f32, 1.0] {
        let n = (size.rooms() / 3).max(1);
        for i in 0..n {
            let z = 10.0 + i as f32 * (nave_len - 20.0) / n as f32;
            let c = Vec2::new(s * 18.0, z);
            let id = add(plan, RoomKind::Room, 0, rect(c, 10.0, 9.0));
            set(plan, id, |r| r.role = Role::Shrine);
        }
    }
    // the crypt below, by a stair from a transept's end
    stairs(
        plan,
        0,
        -1,
        Vec2::new(24.0 * k, cross_z),
        Vec2::new(24.0 * k, cross_z - 30.0),
        3.4,
    );
    let crypt = add(
        plan,
        RoomKind::Hall,
        -1,
        rect(Vec2::new(24.0 * k, cross_z - 48.0), 24.0, 22.0),
    );
    set(plan, crypt, |r| {
        r.pillars = Pillars::Grid;
        r.height = 7.5;
        r.role = Role::Crypt;
        r.name = "Crypt".into();
    });
    let _ = rng;
}

fn dwarven(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -10.0), 18.0, 12.0),
    );
    entrance(plan, ent);
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, -4.0), Vec2::new(0.0, 8.0)], 7.0),
    );
    let hall = gallery_hall(
        plan,
        0,
        Vec2::new(0.0, 36.0 * k),
        44.0 * k,
        52.0 * k,
        Ceiling::Stepped,
    );
    set(plan, hall, |r| {
        r.name = "Great Hall".into();
        r.role = Role::Throne;
    });
    // forges either side, round lava pools
    for s in [-1.0f32, 1.0] {
        let c = Vec2::new(s * 46.0 * k, 36.0 * k);
        let f = add(plan, RoomKind::Hall, 0, rect(c, 26.0, 30.0));
        set(plan, f, |r| {
            r.role = Role::Forge;
            r.height = 12.0;
        });
        let pool = add(
            plan,
            RoomKind::Pool,
            0,
            Shape::Round {
                c: c.to_array(),
                r: 4.5,
            },
        );
        set(plan, pool, |r| r.liquid = Liquid::Lava);
        corridor(plan, 0, Vec2::new(s * 20.0 * k, 36.0 * k), c, 6.0, true);
    }
    // the great shaft: a round room with a pit, a bridge over it, stairs to the deep halls
    let shaft_c = Vec2::new(0.0, 36.0 * k + 26.0 * k + 26.0);
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, 36.0 * k + 24.0 * k), shaft_c], 7.0),
    );
    let shaft = add(
        plan,
        RoomKind::Hall,
        0,
        Shape::Round {
            c: shaft_c.to_array(),
            r: 20.0,
        },
    );
    set(plan, shaft, |r| {
        r.height = 16.0;
        r.name = "The Deep Shaft".into();
    });
    let pit = add(
        plan,
        RoomKind::Chasm,
        0,
        Shape::Round {
            c: shaft_c.to_array(),
            r: 12.0,
        },
    );
    set(plan, pit, |r| {
        r.depth = LEVEL_STEP * 3.0;
        r.liquid = Liquid::Lava;
    });
    add(
        plan,
        RoomKind::Bridge,
        0,
        Shape::path(
            &[
                shaft_c - Vec2::new(0.0, 16.0),
                shaft_c + Vec2::new(0.0, 16.0),
            ],
            4.0,
        ),
    );
    // down to the deep forge
    stairs(
        plan,
        0,
        -1,
        shaft_c + Vec2::new(0.0, 19.0),
        shaft_c + Vec2::new(0.0, 46.0),
        5.0,
    );
    let deep = add(
        plan,
        RoomKind::Hall,
        -1,
        Shape::Ngon {
            c: (shaft_c + Vec2::new(0.0, 66.0)).to_array(),
            r: 22.0 * k,
            sides: 8,
            yaw: 0.0,
        },
    );
    set(plan, deep, |r| {
        r.role = Role::Boss;
        r.height = 15.0;
        r.pillars = Pillars::Ring;
        r.name = "The Molten Core".into();
    });
    let lava = add(
        plan,
        RoomKind::Pool,
        -1,
        Shape::Round {
            c: (shaft_c + Vec2::new(0.0, 66.0)).to_array(),
            r: 6.0,
        },
    );
    set(plan, lava, |r| r.liquid = Liquid::Lava);
    if size != Size::Small {
        // side workshops off the great hall
        let rooms = branching(
            plan,
            rng,
            0,
            Vec2::new(0.0, -40.0),
            size.rooms() / 2,
            24.0,
            (10.0, 14.0),
            4.0,
            &|_, _| RoomKind::Room,
        );
        for (id, _) in rooms {
            set(plan, id, |r| r.role = Role::Storage);
        }
        corridor(
            plan,
            0,
            Vec2::new(0.0, -40.0),
            Vec2::new(0.0, -14.0),
            4.0,
            true,
        );
    }
}

fn city(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    // one vast cavern, its floor cut by a chasm; plazas (daises) and halls cut into its walls
    let r = 70.0 * k;
    let cav = add(plan, RoomKind::Cavern, 0, Shape::Round { c: [0.0, 0.0], r });
    set(plan, cav, |c| {
        c.height = 46.0;
        c.name = "The Undercity".into();
    });
    let chasm = add(
        plan,
        RoomKind::Chasm,
        0,
        Shape::path(
            &[
                Vec2::new(-r * 0.9, 6.0),
                Vec2::new(-r * 0.2, -4.0),
                Vec2::new(r * 0.3, 8.0),
                Vec2::new(r * 0.95, 2.0),
            ],
            12.0,
        ),
    );
    set(plan, chasm, |c| {
        c.depth = 40.0;
        c.liquid = Liquid::Water;
    });
    for x in [-r * 0.5, r * 0.15, r * 0.6] {
        add(
            plan,
            RoomKind::Bridge,
            0,
            Shape::path(&[Vec2::new(x, -16.0), Vec2::new(x, 22.0)], 5.0),
        );
    }
    for (i, (x, z)) in [
        (-0.45, -0.45),
        (0.4, -0.5),
        (-0.35, 0.5),
        (0.45, 0.45),
        (0.0, 0.62),
    ]
    .iter()
    .enumerate()
    {
        let c = Vec2::new(x * r, z * r);
        let d = add(plan, RoomKind::Dais, 0, rect(c, 20.0, 16.0));
        set(plan, d, |r| {
            r.height = 0.6 + (i % 2) as f32 * 0.6;
            r.name = [
                "Market",
                "Temple Steps",
                "Guild Plaza",
                "Lords' Terrace",
                "Throne Plaza",
            ][i]
                .into();
        });
    }
    // halls cut into the cavern wall
    let n = size.rooms().max(4);
    for i in 0..n {
        let a = std::f32::consts::TAU * (i as f32 + 0.5) / n as f32 + rng.range(-0.1, 0.1);
        let dir = Vec2::from_angle(a);
        let c = dir * (r + 18.0);
        let id = add(
            plan,
            RoomKind::Hall,
            0,
            Shape::rect(c, Vec2::new(10.0, 9.0)),
        );
        set(plan, id, |h| {
            h.role = [
                Role::Barracks,
                Role::Library,
                Role::Treasure,
                Role::Forge,
                Role::Shrine,
                Role::Storage,
            ][i % 6];
            h.height = 10.0;
            h.shape = Shape::Rect {
                c: c.to_array(),
                half: [10.0, 9.0],
                yaw: -a,
            };
        });
        add(
            plan,
            RoomKind::Corridor,
            0,
            Shape::path(&[dir * (r - 6.0), dir * (r + 10.0)], 5.0),
        );
    }
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -r - 16.0), 18.0, 12.0),
    );
    entrance(plan, ent);
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, -r - 10.0), Vec2::new(0.0, -r + 8.0)], 6.0),
    );
    // the palace above the throne plaza
    let palace = add(
        plan,
        RoomKind::Hall,
        0,
        rect(Vec2::new(0.0, r + 26.0), 30.0, 22.0),
    );
    set(plan, palace, |h| {
        h.role = Role::Boss;
        h.pillars = Pillars::Rows;
        h.height = 14.0;
    });
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, r * 0.62), Vec2::new(0.0, r + 16.0)], 6.0),
    );
}

fn caverns(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms();
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        46.0 * k,
        (22.0, 38.0),
        6.0,
        &|_, _| RoomKind::Cavern,
    );
    // tunnels between them are rough too: widen corridors into cavern passages
    for r in &mut plan.rooms {
        if r.kind == RoomKind::Corridor {
            r.kind = RoomKind::Cavern;
            r.height = 7.0;
            r.ceiling = Ceiling::Rock;
        }
    }
    entrance(plan, rooms[0].0);
    for (i, (id, c)) in rooms.iter().enumerate() {
        set(plan, *id, |r| r.height = 12.0 + (i % 3) as f32 * 6.0);
        if i % 3 == 2 {
            let pool = add(
                plan,
                RoomKind::Pool,
                0,
                Shape::Round {
                    c: (*c + Vec2::new(3.0, 2.0)).to_array(),
                    r: 5.0,
                },
            );
            set(plan, pool, |r| r.liquid = Liquid::Water);
        }
    }
    let (last, _) = *rooms.last().unwrap();
    set(plan, last, |r| {
        r.role = Role::Lair;
        r.height = 24.0;
    });
}

fn goblin(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms() + 2;
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        30.0 * k,
        (14.0, 22.0),
        4.0,
        &|_, _| RoomKind::Cavern,
    );
    for r in &mut plan.rooms {
        if r.kind == RoomKind::Corridor {
            r.kind = RoomKind::Cavern;
            r.height = 5.5;
            r.ceiling = Ceiling::Rock;
        }
    }
    entrance(plan, rooms[0].0);
    let roles = [
        Role::Barracks,
        Role::Storage,
        Role::Lair,
        Role::Forge,
        Role::Prison,
    ];
    for (i, (id, _)) in rooms.iter().enumerate().skip(1) {
        set(plan, *id, |r| {
            r.role = roles[i % roles.len()];
            r.height = 9.0;
        });
    }
    let (last, _) = *rooms.last().unwrap();
    set(plan, last, |r| {
        r.role = Role::Throne;
        r.height = 14.0;
        r.name = "Chieftain's Den".into();
    });
}

fn spider(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    // three levels of round chambers round shafts, joined by caverns
    for level in 0..3 {
        let lv = -level;
        let c = Vec2::new(level as f32 * 18.0, level as f32 * 40.0);
        let hub = add(
            plan,
            RoomKind::Hall,
            lv,
            Shape::Round {
                c: c.to_array(),
                r: 22.0 * k,
            },
        );
        set(plan, hub, |r| {
            r.height = 12.0;
            r.pillars = Pillars::Ring;
        });
        if level < 2 {
            let pit = add(
                plan,
                RoomKind::Pit,
                lv,
                Shape::Round {
                    c: (c + Vec2::new(6.0, 4.0)).to_array(),
                    r: 7.0,
                },
            );
            set(plan, pit, |r| r.depth = LEVEL_STEP);
        }
        let rooms = branching(
            plan,
            rng,
            lv,
            c + Vec2::new(40.0, 0.0),
            size.rooms() / 2 + 1,
            30.0,
            (14.0, 22.0),
            4.5,
            &|i, _| {
                if i % 2 == 0 {
                    RoomKind::Cavern
                } else {
                    RoomKind::Room
                }
            },
        );
        corridor(plan, lv, c, c + Vec2::new(40.0, 0.0), 5.0, true);
        for (id, _) in &rooms {
            set(plan, *id, |r| r.role = Role::Lair);
        }
        if level < 2 {
            stairs(
                plan,
                lv,
                lv - 1,
                c + Vec2::new(-14.0, 14.0),
                c + Vec2::new(4.0, 40.0),
                4.0,
            );
        }
    }
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -32.0 * k), 14.0, 10.0),
    );
    entrance(plan, ent);
    corridor(
        plan,
        0,
        Vec2::new(0.0, -28.0 * k),
        Vec2::new(0.0, -18.0 * k),
        4.0,
        true,
    );
    let boss = plan
        .rooms
        .iter()
        .filter(|r| r.level == -2 && r.kind == RoomKind::Hall)
        .map(|r| r.id)
        .next();
    if let Some(b) = boss {
        set(plan, b, |r| {
            r.role = Role::Boss;
            r.name = "The Broodmother's Web".into();
        });
    }
}

fn sewers(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms();
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        34.0 * k,
        (12.0, 16.0),
        6.0,
        &|_, _| RoomKind::Room,
    );
    // a water channel down the middle of every tunnel
    let tunnels: Vec<Room> = plan
        .rooms
        .iter()
        .filter(|r| r.kind == RoomKind::Corridor)
        .cloned()
        .collect();
    for t in tunnels {
        if let Shape::Path { pts, .. } = &t.shape {
            let id = add(
                plan,
                RoomKind::Pool,
                0,
                Shape::Path {
                    pts: pts.clone(),
                    width: 2.2,
                },
            );
            set(plan, id, |r| {
                r.floor = -1.0;
                r.liquid = Liquid::Water;
            });
        }
    }
    entrance(plan, rooms[0].0);
    let (last, c) = *rooms.last().unwrap();
    set(plan, last, |r| {
        r.role = Role::Boss;
        r.kind = RoomKind::Hall;
        r.shape = Shape::Round {
            c: c.to_array(),
            r: 18.0 * k,
        };
        r.height = 14.0;
        r.pillars = Pillars::Ring;
        r.name = "The Cistern".into();
    });
    let pool = add(
        plan,
        RoomKind::Pool,
        0,
        Shape::Round {
            c: c.to_array(),
            r: 9.0 * k,
        },
    );
    set(plan, pool, |r| r.liquid = Liquid::Slime);
}

fn mine(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms();
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        28.0 * k,
        (10.0, 16.0),
        4.2,
        &|i, rng| {
            if rng.f32() < 0.5 || i == 0 {
                RoomKind::Cavern
            } else {
                RoomKind::Room
            }
        },
    );
    for r in &mut plan.rooms {
        if r.kind == RoomKind::Corridor {
            r.height = 4.2;
            r.ceiling = Ceiling::Beams;
        }
    }
    entrance(plan, rooms[0].0);
    for (id, _) in rooms.iter().skip(1) {
        set(plan, *id, |r| r.role = Role::Storage);
    }
    // the shaft to the deep workings
    let (last, c) = *rooms.last().unwrap();
    let pit = add(plan, RoomKind::Pit, 0, Shape::rect(c, Vec2::new(3.0, 3.0)));
    set(plan, pit, |r| r.depth = LEVEL_STEP);
    stairs(
        plan,
        0,
        -1,
        c + Vec2::new(6.0, 0.0),
        c + Vec2::new(6.0, 30.0),
        3.6,
    );
    let deep = add(
        plan,
        RoomKind::Cavern,
        -1,
        Shape::Round {
            c: (c + Vec2::new(6.0, 48.0)).to_array(),
            r: 20.0 * k,
        },
    );
    set(plan, deep, |r| {
        r.role = Role::Boss;
        r.height = 16.0;
        r.name = "The Deep Workings".into();
    });
    let _ = last;
}

fn works(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let ent = add(
        plan,
        RoomKind::Room,
        0,
        rect(Vec2::new(0.0, -10.0), 16.0, 12.0),
    );
    entrance(plan, ent);
    let mut z = 0.0;
    let halls = (size.rooms() / 3).max(2);
    for i in 0..halls {
        add(
            plan,
            RoomKind::Corridor,
            0,
            Shape::path(&[Vec2::new(0.0, z - 4.0), Vec2::new(0.0, z + 8.0)], 6.0),
        );
        let c = Vec2::new(0.0, z + 28.0 * k);
        let h = add(plan, RoomKind::Hall, 0, rect(c, 40.0 * k, 36.0 * k));
        set(plan, h, |r| {
            r.pillars = Pillars::Grid;
            r.height = 14.0;
            r.role = [Role::Forge, Role::Storage, Role::Arena][i % 3];
        });
        if i % 2 == 1 {
            // a pit crossed by a catwalk
            let pit = add(plan, RoomKind::Pit, 0, rect(c, 22.0 * k, 12.0));
            set(plan, pit, |r| {
                r.depth = 20.0;
                r.liquid = Liquid::Slime;
            });
            add(
                plan,
                RoomKind::Bridge,
                0,
                Shape::path(&[c - Vec2::new(0.0, 10.0), c + Vec2::new(0.0, 10.0)], 3.2),
            );
        }
        z += 56.0 * k;
    }
    add(
        plan,
        RoomKind::Corridor,
        0,
        Shape::path(&[Vec2::new(0.0, z - 4.0), Vec2::new(0.0, z + 8.0)], 6.0),
    );
    let core = add(
        plan,
        RoomKind::Hall,
        0,
        Shape::Round {
            c: [0.0, z + 26.0],
            r: 22.0 * k,
        },
    );
    set(plan, core, |r| {
        r.role = Role::Boss;
        r.height = 18.0;
        r.pillars = Pillars::Ring;
        r.name = "Reactor Core".into();
    });
    let pool = add(
        plan,
        RoomKind::Pool,
        0,
        Shape::Round {
            c: [0.0, z + 26.0],
            r: 6.0,
        },
    );
    set(plan, pool, |r| r.liquid = Liquid::Arcane);
    let _ = rng;
}

fn frost(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms();
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        32.0 * k,
        (14.0, 20.0),
        4.5,
        &|i, rng| {
            if i > 0 && rng.f32() < 0.35 {
                RoomKind::Cavern
            } else {
                RoomKind::Hall
            }
        },
    );
    entrance(plan, rooms[0].0);
    for (i, (id, _)) in rooms.iter().enumerate().skip(1) {
        set(plan, *id, |r| {
            if r.kind == RoomKind::Hall {
                r.pillars = if i % 2 == 0 {
                    Pillars::Rows
                } else {
                    Pillars::None
                };
                r.height = 11.0;
            }
        });
    }
    let (last, _) = *rooms.last().unwrap();
    make_boss(plan, last, k, rooms[0].1);
    set(plan, last, |r| r.name = "Frost Throne".into());
}

fn infernal(plan: &mut Plan, rng: &mut Rng, size: Size) {
    let k = size.k();
    let n = size.rooms();
    let rooms = branching(
        plan,
        rng,
        0,
        Vec2::ZERO,
        n,
        34.0 * k,
        (14.0, 22.0),
        5.0,
        &|_, _| RoomKind::Hall,
    );
    entrance(plan, rooms[0].0);
    for (i, (id, c)) in rooms.iter().enumerate().skip(1) {
        set(plan, *id, |r| r.height = 12.0);
        if i % 2 == 0 {
            let pit = add(
                plan,
                RoomKind::Pit,
                0,
                Shape::Round {
                    c: c.to_array(),
                    r: 5.0,
                },
            );
            set(plan, pit, |r| {
                r.depth = 12.0;
                r.liquid = Liquid::Lava;
            });
            add(
                plan,
                RoomKind::Bridge,
                0,
                Shape::path(&[*c - Vec2::new(7.0, 0.0), *c + Vec2::new(7.0, 0.0)], 3.0),
            );
        }
    }
    let (last, _) = *rooms.last().unwrap();
    make_boss(plan, last, k * 1.2, rooms[0].1);
    set(plan, last, |r| {
        r.role = Role::Arena;
        r.name = "The Burning Arena".into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_makes_a_connected_walkable_plan() {
        for p in all() {
            for size in Size::ALL {
                let plan = generate(p, size, RoomSize::Normal, 0.0, 7, p.style);
                assert!(!plan.rooms.is_empty(), "{}", p.id);
                let entrance = plan
                    .rooms
                    .iter()
                    .find(|r| r.role == Role::Entrance)
                    .unwrap_or_else(|| panic!("{} has an entrance", p.id));
                let c = Compiled::new(&plan, &|_| (0.7, false));
                let e = entrance.shape.centre();
                let y = plan.level_y(entrance.level);
                assert!(
                    c.floor_under(bevy::math::Vec3::new(e.x, y + 1.0, e.y), 3.0)
                        .is_some(),
                    "{} {:?}: standing in the entrance",
                    p.id,
                    size
                );
                // every level referenced exists
                for r in &plan.rooms {
                    assert!(
                        plan.levels.iter().any(|l| l.id == r.level),
                        "{}: level {}",
                        p.id,
                        r.level
                    );
                }
            }
        }
    }

    #[test]
    fn every_preset_builds_its_architecture_quickly() {
        for p in all() {
            let plan = generate(p, Size::Large, RoomSize::Normal, 0.0, 11, p.style);
            let c = Compiled::new(&plan, &|_| (0.7, false));
            let t0 = std::time::Instant::now();
            let built =
                crate::dungeons::mesh::build(&c, &|_| crate::dungeons::styles::index(p.style));
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            let tris = built.triangles();
            println!(
                "{:>18}: {:>4} rooms, {:>7} triangles, {:>6.0} ms, {} doorways",
                p.id,
                plan.rooms.len(),
                tris,
                ms,
                built.doorways.len()
            );
            assert!(tris > 500, "{}: {tris} triangles", p.id);
            assert!(tris < 3_000_000, "{}: {tris} triangles", p.id);
            for m in built.parts.values() {
                assert!(
                    m.pos.iter().all(|p| p.iter().all(|v| v.is_finite())),
                    "{}: finite",
                    p.id
                );
                assert!(m.idx.iter().all(|i| (*i as usize) < m.pos.len()));
            }
        }
    }

    #[test]
    fn shuffle_keeps_locked_rooms() {
        let p = get("crypt").unwrap();
        let mut plan = generate(p, Size::Medium, RoomSize::Normal, 0.0, 3, p.style);
        let keep = plan.rooms[2].clone();
        plan.rooms[2].locked = true;
        let next = shuffle(&plan, p, Size::Medium, RoomSize::Normal, 0.0, 4, p.style);
        assert!(next.rooms.iter().any(|r| r.shape == keep.shape && r.locked));
    }
}
