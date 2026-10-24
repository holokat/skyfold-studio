//! Gameplay edits shared by the viewport tools, the inspector and the MCP tools: adding, moving,
//! duplicating and removing items with automatic names, levels and NPC → POI links, the starter
//! bestiary, and the validation hints the inspector shows.
//!
//! These functions only change the `Gameplay` data; callers wrap them in an undo step
//! (`History::begin` → `touch_gameplay` → `commit`).

use std::collections::HashSet;

use bevy::prelude::*;

use super::*;
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::{Zone, ZoneType, Zones};

/// NPCs placed this close to a point of interest belong to it.
pub const NPC_ATTACH_M: f32 = 60.0;

/// What the gameplay layer needs to know about the world under an item.
pub struct Ground<'a> {
    pub terrain: &'a Terrain,
    pub zones: &'a Zones,
    pub water: &'a WaterBodies,
    /// procedural trees and rocks (`crate::nature`), where the caller has them
    pub nature: Option<crate::nature::Ctx<'a>>,
}

impl Ground<'_> {
    pub fn at(&self, p: Vec2) -> Vec3 {
        Vec3::new(p.x, self.terrain.height_at(p.x, p.y), p.y)
    }
    pub fn zone(&self, p: Vec2) -> Option<&Zone> {
        self.zones.by_id(self.terrain.zone_at(p.x, p.y))
    }
    pub fn zone_levels(&self, p: Vec2) -> Option<[u8; 2]> {
        self.zone(p).map(levels_of)
    }
    /// Water above the ground (m; 0 on dry land).
    pub fn water_depth(&self, p: Vec2) -> f32 {
        let g = self.terrain.height_at(p.x, p.y);
        let wl = self
            .water
            .water_level(Vec3::new(p.x, g, p.y))
            .unwrap_or(f32::NEG_INFINITY)
            .max(self.terrain.sea_level);
        (wl - g).max(0.0)
    }
    /// The visible surface (ground or water) under a point.
    pub fn surface(&self, p: Vec2) -> f32 {
        self.terrain.height_at(p.x, p.y) + self.water_depth(p)
    }
    pub fn slope_deg(&self, p: Vec2) -> f32 {
        self.terrain
            .normal_at(p.x, p.y)
            .y
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }
    /// Points inside a polygon on a grid (at most ~`max` of them).
    pub fn samples(poly: &[[f32; 2]], max: usize) -> Vec<Vec2> {
        let r = SpawnRegion::bounds_of(poly);
        let area = (r.1 - r.0).x.max(1.0) * (r.1 - r.0).y.max(1.0);
        let step = (area / max as f32).sqrt().max(2.0);
        let mut out = vec![];
        let mut z = r.0.y + step * 0.5;
        while z < r.1.y {
            let mut x = r.0.x + step * 0.5;
            while x < r.1.x {
                let p = Vec2::new(x, z);
                if point_in_polygon(poly, p) {
                    out.push(p);
                }
                x += step;
            }
            z += step;
        }
        out
    }
    /// The zone most of a region lies in, and the share of it under water.
    pub fn region_info(&self, poly: &[[f32; 2]]) -> RegionInfo {
        let pts = Self::samples(poly, 400);
        let mut counts: Vec<(u8, usize)> = vec![];
        let mut wet = 0;
        for p in &pts {
            let z = self.terrain.zone_at(p.x, p.y);
            match counts.iter_mut().find(|c| c.0 == z) {
                Some(c) => c.1 += 1,
                None => counts.push((z, 1)),
            }
            if self.water_depth(*p) > 0.4 {
                wet += 1;
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let zone = counts
            .iter()
            .find(|c| c.0 > 0 && c.1 * 4 >= pts.len().max(1))
            .map(|c| c.0)
            .filter(|z| self.zones.by_id(*z).is_some());
        RegionInfo {
            zone,
            zones: counts.iter().filter(|c| c.0 > 0).map(|c| c.0).collect(),
            water: wet as f32 / pts.len().max(1) as f32,
        }
    }
}

pub struct RegionInfo {
    /// Zone covering at least a quarter of the region (the largest such).
    pub zone: Option<u8>,
    /// Every zone it touches.
    pub zones: Vec<u8>,
    /// Share under water (0..1).
    pub water: f32,
}

pub fn levels_of(z: &Zone) -> [u8; 2] {
    [
        z.level_min.clamp(1, 255) as u8,
        z.level_max.clamp(1, 255) as u8,
    ]
}

pub fn level_label(l: [u8; 2]) -> String {
    if l[0] == l[1] {
        format!("Lv {}", l[0])
    } else {
        format!("Lv {}–{}", l[0], l[1])
    }
}

/// Gathering tier for a level range: 1 for levels 1–10, 2 for 11–20, …
pub fn tier_for_levels(l: [u8; 2]) -> u8 {
    let avg = (l[0] as f32 + l[1] as f32) * 0.5;
    (1.0 + ((avg - 1.0).max(0.0) / 10.0).floor()).clamp(1.0, 8.0) as u8
}

/// `base`, or `base 2`, `base 3`… whichever is free.
pub fn unique_name<'a>(taken: impl Iterator<Item = &'a str>, base: &str) -> String {
    let taken: HashSet<&str> = taken.collect();
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|c| !taken.contains(c.as_str()))
        .unwrap_or_default()
}

/// A name without its trailing counter ("Camp 3" → "Camp").
fn base_name(name: &str) -> &str {
    match name.rsplit_once(' ') {
        Some((head, n)) if n.parse::<u32>().is_ok() && !head.is_empty() => head,
        _ => name,
    }
}

/// Area of a polygon in m² (shoelace).
pub fn polygon_area(poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a * 0.5).abs()
}

pub fn point_in_polygon(poly: &[[f32; 2]], p: Vec2) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p.y) != (b[1] > p.y) && p.x < (b[0] - a[0]) * (p.y - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl PoiKind {
    /// Discovery radius a new POI of this kind gets (m).
    pub fn default_radius(self) -> f32 {
        match self {
            PoiKind::Town => 120.0,
            PoiKind::QuestHub => 45.0,
            PoiKind::Dungeon => 30.0,
            PoiKind::Landmark => 60.0,
            PoiKind::Camp => 35.0,
            PoiKind::Vendor => 15.0,
            PoiKind::FlightPoint => 20.0,
            PoiKind::Graveyard => 25.0,
            PoiKind::Portal => 20.0,
            PoiKind::BossArena => 50.0,
            PoiKind::Shrine => 20.0,
            PoiKind::Vista => 40.0,
            PoiKind::Treasure => 10.0,
            PoiKind::Custom => 30.0,
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            PoiKind::Town => "A settlement: inn, vendors, trainers, safe haven",
            PoiKind::QuestHub => "Where players pick up and turn in quests",
            PoiKind::Dungeon => "Entrance to an instanced dungeon",
            PoiKind::Landmark => "A place worth finding: ruins, a giant tree, a crashed ship",
            PoiKind::Camp => "Outpost or enemy camp",
            PoiKind::Vendor => "A lone merchant",
            PoiKind::FlightPoint => "Travel network node (griffons, wyverns, airships)",
            PoiKind::Graveyard => "Where dead players respawn",
            PoiKind::Portal => "Teleport to another place or zone",
            PoiKind::BossArena => "A world boss or elite encounter",
            PoiKind::Shrine => "Buff, blessing or lore stone",
            PoiKind::Vista => "A view point (achievement, screenshot spot)",
            PoiKind::Treasure => "A hidden chest or rare spawn",
            PoiKind::Custom => "Anything else",
        }
    }
    /// Short tile caption.
    pub fn short(self) -> &'static str {
        match self {
            PoiKind::Dungeon => "Dungeon",
            PoiKind::FlightPoint => "Flight",
            PoiKind::Graveyard => "Graveyard",
            PoiKind::BossArena => "Boss",
            other => other.name(),
        }
    }
}

impl NpcRole {
    pub fn hint(self) -> &'static str {
        match self {
            NpcRole::QuestGiver => "Gives and takes quests",
            NpcRole::Vendor => "Buys and sells goods",
            NpcRole::Trainer => "Teaches skills and professions",
            NpcRole::Guard => "Protects a town or road",
            NpcRole::Innkeeper => "Rest and hearthstone point",
            NpcRole::FlightMaster => "Runs the flight point",
            NpcRole::Banker => "Bank and storage",
            NpcRole::Ambient => "Villagers, workers: life in the world",
            NpcRole::Custom => "Anything else",
        }
    }
}

impl ResourceKind {
    pub fn default_respawn(self) -> f32 {
        match self {
            ResourceKind::Ore => 600.0,
            ResourceKind::Herb => 300.0,
            ResourceKind::Wood => 450.0,
            ResourceKind::Stone => 450.0,
            ResourceKind::Crystal => 1200.0,
            ResourceKind::Custom => 600.0,
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            ResourceKind::Ore => "Mining veins: rocky slopes, mountains, cliffs",
            ResourceKind::Herb => "Herbalism plants: grassland and forest floors",
            ResourceKind::Wood => "Lumber: grown trees",
            ResourceKind::Stone => "Quarry stone: boulders and rocky ground",
            ResourceKind::Crystal => "Rare crystals: high peaks and volcanic rock",
            ResourceKind::Custom => "Anything else",
        }
    }
}

/// The variant a node gets when none is given: by kind, tier and ground (biome).
pub fn default_variant(
    kind: ResourceKind,
    tier: u8,
    biome: Option<crate::biomes::BiomePreset>,
) -> String {
    use crate::biomes::BiomePreset as B;
    let t = (tier.clamp(1, 8) - 1) as usize;
    match kind {
        ResourceKind::Ore => [
            "Copper Vein",
            "Tin Vein",
            "Iron Vein",
            "Silver Vein",
            "Gold Vein",
            "Mithril Vein",
            "Starmetal Vein",
            "Adamant Vein",
        ][t]
            .into(),
        ResourceKind::Herb => match biome {
            Some(B::Swamp) => "Marsh Lily".into(),
            Some(B::Desert) => "Desert Bloom".into(),
            Some(B::Alpine | B::Tundra) => "Frostleaf".into(),
            Some(B::Jungle | B::TropicalCoast) => "Jungle Orchid".into(),
            Some(B::HauntedWoods) => "Gravemoss".into(),
            Some(B::Volcanic) => "Firebloom".into(),
            _ => [
                "Meadowbloom",
                "Silverleaf",
                "Wild Sage",
                "Sunpetal",
                "Moonblossom",
                "Dreamroot",
                "Starbloom",
                "Ghostcap",
            ][t]
                .into(),
        },
        ResourceKind::Wood => wood_variant("Tree", tier),
        ResourceKind::Stone => match biome {
            Some(B::Volcanic) => "Obsidian Outcrop".into(),
            _ => [
                "Rough Stone",
                "Coarse Stone",
                "Granite Outcrop",
                "Heavy Stone",
                "Solid Stone",
                "Dense Stone",
                "Marble Outcrop",
                "Runestone",
            ][t]
                .into(),
        },
        ResourceKind::Crystal => [
            "Quartz Cluster",
            "Amethyst Cluster",
            "Sunstone Cluster",
            "Moonstone Cluster",
            "Starcrystal",
            "Void Crystal",
            "Prismatic Crystal",
            "Dragonglass",
        ][t]
            .into(),
        ResourceKind::Custom => "Resource".into(),
    }
}

/// "Oak" at tier 3 → "Old Oak"; dead trees give "Deadwood".
pub fn wood_variant(species: &str, tier: u8) -> String {
    if species.to_lowercase().contains("dead") {
        return "Deadwood".into();
    }
    let prefix = [
        "",
        "Sturdy",
        "Old",
        "Ancient",
        "Elder",
        "Ironbark",
        "Heartwood",
        "Primeval",
    ][(tier.clamp(1, 8) - 1) as usize];
    format!("{prefix} {species}").trim().to_string()
}

impl SpawnRegion {
    pub fn bounds_of(poly: &[[f32; 2]]) -> (Vec2, Vec2) {
        let mut lo = Vec2::splat(f32::MAX);
        let mut hi = Vec2::splat(f32::MIN);
        for p in poly {
            lo = lo.min(Vec2::new(p[0], p[1]));
            hi = hi.max(Vec2::new(p[0], p[1]));
        }
        (lo, hi)
    }
    /// Creatures alive at once: density × area, capped by `max_alive`.
    pub fn expected_alive(&self) -> u32 {
        if self.entries.is_empty() {
            return 0;
        }
        let n = (self.density * self.area() / 1.0e6).round().max(1.0) as u32;
        if self.max_alive > 0 {
            n.min(self.max_alive)
        } else {
            n
        }
    }
    pub fn translate(&mut self, d: Vec2) {
        for p in &mut self.polygon {
            p[0] += d.x;
            p[1] += d.y;
        }
    }
    /// Encounter chance of each entry (weights normalised).
    pub fn chances(&self) -> Vec<f32> {
        let total: f32 = self.entries.iter().map(|e| e.weight.max(0.0)).sum();
        self.entries
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
    /// The most hostile creature of the table.
    pub fn aggression(&self, gp: &Gameplay) -> Option<Aggression> {
        let rank = |a: Aggression| match a {
            Aggression::Passive => 0,
            Aggression::Neutral => 1,
            Aggression::Hostile => 2,
        };
        self.entries
            .iter()
            .filter_map(|e| gp.creature(e.creature))
            .map(|c| c.aggression)
            .max_by_key(|a| rank(*a))
    }
}

impl Aggression {
    pub const ALL: [Aggression; 3] = [
        Aggression::Passive,
        Aggression::Neutral,
        Aggression::Hostile,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Aggression::Passive => "Passive",
            Aggression::Neutral => "Neutral",
            Aggression::Hostile => "Hostile",
        }
    }
}

impl SpawnBehaviour {
    pub const ALL: [SpawnBehaviour; 4] = [
        SpawnBehaviour::Roam,
        SpawnBehaviour::Patrol,
        SpawnBehaviour::Static,
        SpawnBehaviour::Ambush,
    ];
    pub fn name(self) -> &'static str {
        match self {
            SpawnBehaviour::Roam => "Roam",
            SpawnBehaviour::Patrol => "Patrol",
            SpawnBehaviour::Static => "Static",
            SpawnBehaviour::Ambush => "Ambush",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            SpawnBehaviour::Roam => "Wander around inside the region",
            SpawnBehaviour::Patrol => "Walk routes through the region",
            SpawnBehaviour::Static => "Stand or sit where they spawn",
            SpawnBehaviour::Ambush => "Hide until a player comes close",
        }
    }
}

// ------------------------------------------------------------------ adding

pub fn set_player_start(gp: &mut Gameplay, g: &Ground, p: Vec2, yaw: f32) {
    gp.player_start = Some(PlayerStart {
        pos: g.at(p).to_array(),
        yaw,
    });
    gp.dirty = true;
}

/// Adds a point of interest; NPCs within 60 m that belong nowhere yet join it.
pub fn add_poi(gp: &mut Gameplay, g: &Ground, kind: PoiKind, p: Vec2, name: Option<String>) -> u32 {
    let id = gp.alloc_id();
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| unique_name(gp.pois.iter().map(|p| p.name.as_str()), kind.name()));
    gp.pois.push(Poi {
        id,
        kind,
        name,
        pos: g.at(p).to_array(),
        radius: kind.default_radius(),
        notes: String::new(),
        tags: vec![],
        level: None,
    });
    for n in gp
        .npcs
        .iter_mut()
        .filter(|n| n.poi.is_none() && Vec2::new(n.pos[0], n.pos[2]).distance(p) <= NPC_ATTACH_M)
    {
        n.poi = Some(id);
    }
    gp.dirty = true;
    id
}

/// The closest POI within `NPC_ATTACH_M`.
pub fn nearest_poi(gp: &Gameplay, p: Vec2) -> Option<u32> {
    gp.pois
        .iter()
        .map(|q| (q.id, Vec2::new(q.pos[0], q.pos[2]).distance(p)))
        .filter(|(_, d)| *d <= NPC_ATTACH_M)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// Adds an NPC; `poi: None` attaches it to the nearest POI within 60 m.
pub fn add_npc(
    gp: &mut Gameplay,
    g: &Ground,
    role: NpcRole,
    p: Vec2,
    yaw: f32,
    name: Option<String>,
    poi: Option<Option<u32>>,
) -> u32 {
    let poi = poi.unwrap_or_else(|| nearest_poi(gp, p));
    let id = gp.alloc_id();
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| unique_name(gp.npcs.iter().map(|p| p.name.as_str()), role.name()));
    gp.npcs.push(Npc {
        id,
        name,
        role,
        pos: g.at(p).to_array(),
        yaw,
        poi,
        notes: String::new(),
        life: Default::default(),
    });
    gp.dirty = true;
    id
}

/// Default level range of a region: the zone's, else its creatures', else 1–5.
fn region_levels(gp: &Gameplay, g: &Ground, poly: &[[f32; 2]], entries: &[SpawnEntry]) -> [u8; 2] {
    if let Some(z) = g.region_info(poly).zone.and_then(|z| g.zones.by_id(z)) {
        return levels_of(z);
    }
    let lv: Vec<[u8; 2]> = entries
        .iter()
        .filter_map(|e| gp.creature(e.creature))
        .map(|c| c.level)
        .collect();
    if lv.is_empty() {
        [1, 5]
    } else {
        [
            lv.iter().map(|l| l[0]).min().unwrap_or(1),
            lv.iter().map(|l| l[1]).max().unwrap_or(5),
        ]
    }
}

/// Adds a spawn region (at least 3 points, a few hundred m²). Level from the zone below.
pub fn add_spawn(
    gp: &mut Gameplay,
    g: &Ground,
    polygon: Vec<[f32; 2]>,
    creatures: &[u32],
    name: Option<String>,
) -> Result<u32, String> {
    if polygon.len() < 3 {
        return Err("A spawn region needs at least 3 corners".into());
    }
    if polygon_area(&polygon) < 150.0 {
        return Err("The region is too small: draw at least ~15 × 15 m".into());
    }
    let entries: Vec<SpawnEntry> = creatures
        .iter()
        .filter(|c| gp.creature(**c).is_some())
        .map(|c| SpawnEntry {
            creature: *c,
            weight: 1.0,
        })
        .collect();
    let probe = SpawnRegion {
        id: 0,
        name: String::new(),
        polygon,
        entries,
        density: 300.0,
        max_alive: 0,
        respawn_secs: 300.0,
        level: [1, 1],
        pack: [1, 2],
        behaviour: SpawnBehaviour::Roam,
        notes: String::new(),
    };
    let level = region_levels(gp, g, &probe.polygon, &probe.entries);
    let base = match probe.entries.first().and_then(|e| gp.creature(e.creature)) {
        Some(c) => format!("{} Grounds", c.name),
        None => "Spawn Region".to_string(),
    };
    let id = gp.alloc_id();
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| unique_name(gp.spawns.iter().map(|s| s.name.as_str()), &base));
    gp.spawns.push(SpawnRegion {
        id,
        name,
        level,
        ..probe
    });
    gp.dirty = true;
    Ok(id)
}

/// Adds a resource node. `tier: None` follows the zone's level range (tier 1 outside zones),
/// `variant: None` picks one from the kind, tier and biome. A mining node gets the ore its name
/// mentions, else one that suits the tier and the ground (`ores::pick_for_spot`).
pub fn add_node(
    gp: &mut Gameplay,
    g: &Ground,
    kind: ResourceKind,
    p: Vec2,
    variant: Option<String>,
    tier: Option<u8>,
) -> u32 {
    let tier = tier
        .filter(|t| *t > 0)
        .unwrap_or_else(|| g.zone_levels(p).map(tier_for_levels).unwrap_or(1))
        .clamp(1, 8);
    let biome = crate::biomes::BiomePreset::from_id(g.terrain.biome_at(p.x, p.y));
    let variant = variant.filter(|v| !v.trim().is_empty());
    let ore = (kind == ResourceKind::Ore).then(|| {
        variant
            .as_deref()
            .and_then(ores::Ore::in_name)
            .unwrap_or_else(|| {
                let roll = (crate::noise::hash2(p.x.round() as i32, p.y.round() as i32, 0x0e5)
                    & 0xffff) as f32
                    / 65536.0;
                ores::pick_for_spot(
                    tier.min(5),
                    biome,
                    ores::altitude(g.terrain, p),
                    g.slope_deg(p),
                    roll,
                )
            })
    });
    let variant = variant.unwrap_or_else(|| {
        ore.map(|o| o.deposit_name())
            .unwrap_or_else(|| default_variant(kind, tier, biome))
    });
    let id = gp.alloc_id();
    let (tier, respawn_secs) = match ore {
        Some(o) => (o.tier(), o.default_respawn()),
        None => (tier, kind.default_respawn()),
    };
    gp.nodes.push(ResourceNode {
        id,
        kind,
        variant,
        tier,
        pos: g.at(p).to_array(),
        respawn_secs,
        auto: false,
        ore,
        area: None,
    });
    gp.dirty = true;
    id
}

pub fn add_creature(
    gp: &mut Gameplay,
    name: &str,
    family: &str,
    level: [u8; 2],
    aggression: Aggression,
    elite: bool,
) -> u32 {
    let id = gp.alloc_id();
    let name = unique_name(
        gp.creatures.iter().map(|c| c.name.as_str()),
        if name.trim().is_empty() {
            "New Creature"
        } else {
            name.trim()
        },
    );
    let level = [level[0].max(1), level[1].max(level[0].max(1))];
    gp.creatures.push(CreatureType {
        id,
        name,
        family: family.trim().to_string(),
        level,
        aggression,
        elite,
        notes: String::new(),
        character: None,
    });
    id
}

/// A starter bestiary for a fantasy MMO (only names not in the library yet are added).
pub fn add_starter_creatures(gp: &mut Gameplay) -> usize {
    use Aggression::*;
    let list: [(&str, &str, [u8; 2], Aggression, bool); 14] = [
        ("Meadow Hare", "critter", [1, 2], Passive, false),
        ("Young Boar", "boar", [1, 4], Neutral, false),
        ("Shore Crab", "crab", [2, 5], Neutral, false),
        ("Timber Wolf", "wolf", [3, 6], Hostile, false),
        ("Forest Spider", "spider", [4, 7], Hostile, false),
        ("Bandit Scout", "bandit", [5, 9], Hostile, false),
        ("Brown Bear", "bear", [8, 12], Neutral, false),
        ("Cave Goblin", "goblin", [9, 13], Hostile, false),
        ("Cliff Harpy", "harpy", [11, 15], Hostile, false),
        ("Frost Wolf", "wolf", [13, 17], Hostile, false),
        ("Bog Lurker", "swamp beast", [16, 20], Hostile, false),
        ("Mountain Troll", "troll", [18, 22], Hostile, true),
        ("Ancient Treant", "treant", [20, 25], Neutral, true),
        ("Ash Elemental", "elemental", [25, 30], Hostile, false),
    ];
    let mut n = 0;
    for (name, family, level, aggression, elite) in list {
        if gp.creatures.iter().any(|c| c.name == name) {
            continue;
        }
        add_creature(gp, name, family, level, aggression, elite);
        n += 1;
    }
    n
}

// ------------------------------------------------------------------ changing

/// Moves a point item to `p` (a spawn region's centroid goes to `p`).
pub fn move_to(gp: &mut Gameplay, g: &Ground, sel: GameplaySel, p: Vec2) {
    let y = g.terrain.height_at(p.x, p.y);
    let set = |pos: &mut [f32; 3]| *pos = [p.x, y, p.y];
    match sel {
        GameplaySel::PlayerStart => {
            if let Some(s) = &mut gp.player_start {
                set(&mut s.pos);
            }
        }
        GameplaySel::Poi(id) => {
            if let Some(q) = gp.pois.iter_mut().find(|q| q.id == id) {
                set(&mut q.pos);
            }
        }
        GameplaySel::Npc(id) => {
            if let Some(q) = gp.npcs.iter_mut().find(|q| q.id == id) {
                set(&mut q.pos);
            }
        }
        GameplaySel::Node(id) => {
            if let Some(q) = gp.nodes.iter_mut().find(|q| q.id == id) {
                set(&mut q.pos);
            }
        }
        GameplaySel::Spawn(id) => {
            if let Some(s) = gp.spawns.iter_mut().find(|s| s.id == id)
                && let Some(c) = s.centroid()
            {
                s.translate(p - c);
            }
        }
        // an ore area carries its deposits along
        GameplaySel::OreArea(id) => {
            if let Some(a) = gp.ore_areas.iter_mut().find(|a| a.id == id)
                && let Some(c) = a.centroid()
            {
                let d = p - c;
                a.translate(d);
                for n in gp.nodes.iter_mut().filter(|n| n.area == Some(id)) {
                    let q = Vec2::new(n.pos[0], n.pos[2]) + d;
                    n.pos = g.at(q).to_array();
                }
            }
        }
    }
    gp.dirty = true;
}

/// Copies an item next to the original (Ctrl/⌘ D). Returns the copy.
pub fn duplicate(gp: &mut Gameplay, g: &Ground, sel: GameplaySel) -> Option<GameplaySel> {
    let off = Vec2::new(10.0, 10.0);
    let shifted = |pos: [f32; 3]| {
        let p = Vec2::new(pos[0], pos[2]) + off;
        g.at(p).to_array()
    };
    match sel {
        GameplaySel::PlayerStart => None,
        GameplaySel::Poi(id) => {
            let mut q = gp.pois.iter().find(|q| q.id == id)?.clone();
            q.id = gp.alloc_id();
            q.name = unique_name(gp.pois.iter().map(|p| p.name.as_str()), base_name(&q.name));
            q.pos = shifted(q.pos);
            gp.pois.push(q);
            gp.pois.last().map(|q| GameplaySel::Poi(q.id))
        }
        GameplaySel::Npc(id) => {
            let mut q = gp.npcs.iter().find(|q| q.id == id)?.clone();
            q.id = gp.alloc_id();
            q.name = unique_name(gp.npcs.iter().map(|p| p.name.as_str()), base_name(&q.name));
            q.pos = shifted(q.pos);
            gp.npcs.push(q);
            gp.npcs.last().map(|q| GameplaySel::Npc(q.id))
        }
        GameplaySel::Node(id) => {
            let mut q = gp.nodes.iter().find(|q| q.id == id)?.clone();
            q.id = gp.alloc_id();
            q.pos = shifted(q.pos);
            q.auto = false;
            q.area = None;
            gp.nodes.push(q);
            gp.nodes.last().map(|q| GameplaySel::Node(q.id))
        }
        GameplaySel::Spawn(id) => {
            let mut s = gp.spawns.iter().find(|s| s.id == id)?.clone();
            let (lo, hi) = SpawnRegion::bounds_of(&s.polygon);
            s.id = gp.alloc_id();
            s.name = unique_name(
                gp.spawns.iter().map(|p| p.name.as_str()),
                base_name(&s.name),
            );
            s.translate(Vec2::new((hi.x - lo.x) * 0.6 + 10.0, 0.0));
            gp.spawns.push(s);
            gp.spawns.last().map(|s| GameplaySel::Spawn(s.id))
        }
        GameplaySel::OreArea(id) => {
            let mut a = gp.ore_areas.iter().find(|a| a.id == id)?.clone();
            let (lo, hi) = SpawnRegion::bounds_of(&a.polygon);
            let d = Vec2::new((hi.x - lo.x) * 0.6 + 10.0, 0.0);
            let new = gp.alloc_id();
            let copies: Vec<ResourceNode> = gp
                .nodes
                .iter()
                .filter(|n| n.area == Some(id))
                .cloned()
                .collect();
            for mut n in copies {
                n.id = gp.alloc_id();
                n.pos = g.at(Vec2::new(n.pos[0], n.pos[2]) + d).to_array();
                n.area = Some(new);
                gp.nodes.push(n);
            }
            a.id = new;
            a.name = unique_name(
                gp.ore_areas.iter().map(|p| p.name.as_str()),
                base_name(&a.name),
            );
            a.translate(d);
            gp.ore_areas.push(a);
            Some(GameplaySel::OreArea(new))
        }
    }
    .inspect(|_| gp.dirty = true)
}

/// Removes the item or creature with this id (POIs unlink their NPCs, creatures leave the
/// encounter tables). Returns what was removed.
pub fn remove(gp: &mut Gameplay, id: u32) -> Option<String> {
    let what = if let Some(i) = gp.pois.iter().position(|q| q.id == id) {
        for n in &mut gp.npcs {
            if n.poi == Some(id) {
                n.poi = None;
            }
        }
        format!("POI {}", gp.pois.remove(i).name)
    } else if let Some(i) = gp.npcs.iter().position(|q| q.id == id) {
        format!("NPC {}", gp.npcs.remove(i).name)
    } else if let Some(i) = gp.spawns.iter().position(|q| q.id == id) {
        format!("spawn region {}", gp.spawns.remove(i).name)
    } else if let Some(i) = gp.nodes.iter().position(|q| q.id == id) {
        format!("{} node", gp.nodes.remove(i).variant)
    } else if let Some(name) = ores::remove_area(gp, id) {
        format!("deposit area {name}")
    } else {
        let i = gp.creatures.iter().position(|q| q.id == id)?;
        for s in &mut gp.spawns {
            s.entries.retain(|e| e.creature != id);
        }
        format!("creature {}", gp.creatures.remove(i).name)
    };
    if gp.selected.is_some_and(|s| !gp.exists(s)) {
        gp.selected = None;
    }
    gp.dirty = true;
    Some(what)
}

pub fn remove_sel(gp: &mut Gameplay, sel: GameplaySel) -> Option<String> {
    match sel {
        GameplaySel::PlayerStart => {
            gp.player_start.take()?;
            gp.selected = None;
            gp.dirty = true;
            Some("player start".into())
        }
        GameplaySel::Poi(id)
        | GameplaySel::Npc(id)
        | GameplaySel::Spawn(id)
        | GameplaySel::Node(id)
        | GameplaySel::OreArea(id) => remove(gp, id),
    }
}

/// Short label of a selection for undo steps and messages.
pub fn sel_label(gp: &Gameplay, sel: GameplaySel) -> String {
    match sel {
        GameplaySel::PlayerStart => "player start".into(),
        GameplaySel::Poi(id) => gp
            .pois
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.name.clone())
            .unwrap_or_default(),
        GameplaySel::Npc(id) => gp
            .npcs
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.name.clone())
            .unwrap_or_default(),
        GameplaySel::Spawn(id) => gp
            .spawns
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.name.clone())
            .unwrap_or_default(),
        GameplaySel::Node(id) => gp
            .nodes
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.variant.clone())
            .unwrap_or_default(),
        GameplaySel::OreArea(id) => gp
            .ore_areas
            .iter()
            .find(|q| q.id == id)
            .map(|q| q.name.clone())
            .unwrap_or_default(),
    }
}

/// Reduces a hand-drawn loop to its corners (Douglas–Peucker, `tol` metres).
pub fn simplify(points: &[Vec2], tol: f32) -> Vec<Vec2> {
    fn rec(p: &[Vec2], tol: f32, out: &mut Vec<Vec2>) {
        let (a, b) = (p[0], p[p.len() - 1]);
        let ab = b - a;
        let len = ab.length().max(1e-4);
        let (mut best, mut idx) = (0.0, 0);
        for (i, q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
            let d = (ab.perp_dot(*q - a) / len).abs();
            if d > best {
                best = d;
                idx = i;
            }
        }
        if best > tol && idx > 0 {
            rec(&p[..=idx], tol, out);
            rec(&p[idx..], tol, out);
        } else {
            out.push(a);
        }
    }
    if points.len() < 4 {
        return points.to_vec();
    }
    let mut out = vec![];
    rec(points, tol, &mut out);
    out
}

// ------------------------------------------------------------------ validation

pub struct Issue {
    /// A real problem (else a note).
    pub warn: bool,
    pub text: String,
}

fn warn(text: String) -> Issue {
    Issue { warn: true, text }
}
fn note(text: String) -> Issue {
    Issue { warn: false, text }
}

fn level_fits(l: [u8; 2], zone: [u8; 2]) -> bool {
    l[0] >= zone[0] && l[1] <= zone[1]
}

/// Hints for one item: placement problems and mismatches with the zone below.
pub fn issues(gp: &Gameplay, g: &Ground, sel: GameplaySel) -> Vec<Issue> {
    let mut out = vec![];
    let xz = |p: [f32; 3]| Vec2::new(p[0], p[2]);
    let wet = |p: Vec2| g.water_depth(p);
    match sel {
        GameplaySel::PlayerStart => {
            let Some(s) = gp.player_start else { return out };
            let p = xz(s.pos);
            if wet(p) > 0.5 {
                out.push(warn(format!(
                    "The player start is under {:.1} m of water.",
                    wet(p)
                )));
            }
            if g.slope_deg(p) > 35.0 {
                out.push(warn("The player start is on a steep slope.".into()));
            }
            for r in &gp.spawns {
                if r.contains(p) && r.aggression(gp) == Some(Aggression::Hostile) {
                    out.push(warn(format!(
                        "Hostile creatures spawn right at the start ({}).",
                        r.name
                    )));
                }
            }
            if g.zone(p).is_none() {
                out.push(note(
                    "Not inside a zone: new players start in the wilderness.".into(),
                ));
            }
        }
        GameplaySel::Poi(id) => {
            let Some(q) = gp.pois.iter().find(|q| q.id == id) else {
                return out;
            };
            let p = xz(q.pos);
            if wet(p) > 0.5 {
                out.push(warn(format!("Under {:.1} m of water.", wet(p))));
            }
            match g.zone(p) {
                Some(z) => {
                    if let Some(l) = q.level
                        && !level_fits(l, levels_of(z))
                    {
                        out.push(warn(format!(
                            "{} is outside {}'s {}.",
                            level_label(l),
                            z.name,
                            level_label(levels_of(z))
                        )));
                    }
                }
                None => out.push(note("Not inside a zone.".into())),
            }
            if matches!(q.kind, PoiKind::Town | PoiKind::QuestHub)
                && !gp.npcs.iter().any(|n| n.poi == Some(id))
            {
                out.push(note("No NPCs yet: place some with the NPC tool.".into()));
            }
        }
        GameplaySel::Npc(id) => {
            let Some(n) = gp.npcs.iter().find(|q| q.id == id) else {
                return out;
            };
            let p = xz(n.pos);
            if wet(p) > 0.5 {
                out.push(warn(format!("Under {:.1} m of water.", wet(p))));
            }
            match n.poi.and_then(|pid| gp.pois.iter().find(|q| q.id == pid)) {
                Some(q) => {
                    let d = xz(q.pos).distance(p);
                    if d > (q.radius * 1.5).max(NPC_ATTACH_M) {
                        out.push(warn(format!(
                            "{d:.0} m from its point of interest {}.",
                            q.name
                        )));
                    }
                }
                None if n.poi.is_some() => {
                    out.push(warn("Its point of interest no longer exists.".into()))
                }
                None => out.push(note("Not linked to a point of interest.".into())),
            }
            for r in &gp.spawns {
                if r.contains(p)
                    && r.aggression(gp) == Some(Aggression::Hostile)
                    && n.role != NpcRole::Guard
                {
                    out.push(warn(format!("Stands inside hostile spawns ({}).", r.name)));
                    break;
                }
            }
        }
        GameplaySel::Spawn(id) => {
            let Some(r) = gp.spawns.iter().find(|q| q.id == id) else {
                return out;
            };
            if r.entries.is_empty() {
                out.push(warn(
                    "No creatures yet: add some to the encounter table.".into(),
                ));
            }
            let info = g.region_info(&r.polygon);
            if info.water > 0.25 {
                out.push(warn(format!(
                    "{:.0}% of the region is under water.",
                    info.water * 100.0
                )));
            }
            match info.zone.and_then(|z| g.zones.by_id(z)) {
                Some(z) => {
                    if !level_fits(r.level, levels_of(z)) {
                        out.push(warn(format!(
                            "{} is outside {}'s {}.",
                            level_label(r.level),
                            z.name,
                            level_label(levels_of(z))
                        )));
                    }
                    if matches!(z.zone_type, ZoneType::Sanctuary | ZoneType::City)
                        && r.aggression(gp) == Some(Aggression::Hostile)
                    {
                        out.push(warn(format!(
                            "Hostile creatures in {} ({}).",
                            z.name,
                            z.zone_type.name()
                        )));
                    }
                }
                None => out.push(note("Not inside a zone.".into())),
            }
            if info.zones.len() > 1 {
                out.push(note(format!("Spans {} zones.", info.zones.len())));
            }
            for e in &r.entries {
                if let Some(c) = gp.creature(e.creature)
                    && (c.level[1] < r.level[0] || c.level[0] > r.level[1])
                {
                    out.push(warn(format!(
                        "{} ({}) doesn't fit the region's {}.",
                        c.name,
                        level_label(c.level),
                        level_label(r.level)
                    )));
                }
            }
            if let Some(p) = gp.player_start
                && r.contains(xz(p.pos))
                && r.aggression(gp) == Some(Aggression::Hostile)
            {
                out.push(warn("Hostile creatures spawn on the player start.".into()));
            }
            let alive = r.expected_alive();
            if alive > 150 {
                out.push(note(format!(
                    "~{alive} creatures alive at once: heavy for the server."
                )));
            }
            if r.area() < 900.0 {
                out.push(note("A very small region: creatures will crowd.".into()));
            }
        }
        GameplaySel::Node(id) => {
            let Some(n) = gp.nodes.iter().find(|q| q.id == id) else {
                return out;
            };
            let p = xz(n.pos);
            if wet(p) > 0.5 {
                out.push(warn(format!("Under {:.1} m of water.", wet(p))));
            }
            if g.slope_deg(p) > 50.0 {
                out.push(warn("On a cliff: players may not reach it.".into()));
            }
            if let Some(z) = g.zone(p) {
                match n.ore() {
                    Some(o) => {
                        let t = ores::tier_for_levels(levels_of(z));
                        if o.tier().abs_diff(t) >= 2 {
                            out.push(note(format!(
                                "{} ({}) is unusual in {} (tier {t} ores fit {}).",
                                o.name(),
                                o.rarity().name(),
                                z.name,
                                level_label(levels_of(z))
                            )));
                        }
                    }
                    None => {
                        let t = tier_for_levels(levels_of(z));
                        if n.tier.abs_diff(t) >= 2 {
                            out.push(note(format!(
                                "Tier {} is unusual in {} (tier {t} fits {}).",
                                n.tier,
                                z.name,
                                level_label(levels_of(z))
                            )));
                        }
                    }
                }
            }
        }
        GameplaySel::OreArea(id) => {
            let Some(a) = gp.ore_areas.iter().find(|q| q.id == id) else {
                return out;
            };
            if a.table.iter().all(|e| e.weight <= 0.0) {
                out.push(warn("No ores yet: add some to the ore table.".into()));
            }
            let n = gp.nodes.iter().filter(|n| n.area == Some(id)).count();
            if n == 0 && !a.table.is_empty() {
                out.push(warn("No deposit fits: the ground inside is under water, too steep, paved or built over.".into()));
            }
            let info = g.region_info(&a.polygon);
            if info.water > 0.25 {
                out.push(note(format!(
                    "{:.0}% of the area is under water (no deposits there).",
                    info.water * 100.0
                )));
            }
            if let Some(z) = info.zone.and_then(|z| g.zones.by_id(z)) {
                let t = ores::tier_for_levels(levels_of(z));
                for e in a.table.iter().filter(|e| e.weight > 0.0) {
                    if e.ore.tier().abs_diff(t) >= 2 {
                        out.push(note(format!(
                            "{} ({}) is unusual in {} (tier {t} ores fit {}).",
                            e.ore.name(),
                            e.ore.rarity().name(),
                            z.name,
                            level_label(levels_of(z))
                        )));
                    }
                }
            }
            if n > 600 {
                out.push(note(format!("{n} deposits: a lot for one area.")));
            }
        }
    }
    out
}

/// Every warning in the world (notes skipped), item by item.
pub fn all_warnings(gp: &Gameplay, g: &Ground) -> Vec<(GameplaySel, String)> {
    let mut sels: Vec<GameplaySel> = vec![];
    if gp.player_start.is_some() {
        sels.push(GameplaySel::PlayerStart);
    }
    sels.extend(gp.pois.iter().map(|q| GameplaySel::Poi(q.id)));
    sels.extend(gp.npcs.iter().map(|q| GameplaySel::Npc(q.id)));
    sels.extend(gp.spawns.iter().map(|q| GameplaySel::Spawn(q.id)));
    sels.extend(gp.nodes.iter().map(|q| GameplaySel::Node(q.id)));
    sels.extend(gp.ore_areas.iter().map(|q| GameplaySel::OreArea(q.id)));
    let mut out = vec![];
    for s in sels {
        for i in issues(gp, g, s) {
            if i.warn {
                out.push((s, i.text));
            }
        }
    }
    out
}
