//! The World Doctor: checks a world for the mistakes that slip into big maps (floating rocks,
//! drowned trees, NPCs under water, spawn camps on cliffs, unreachable quest givers, zones with
//! nothing to do, roads too steep to ride, object pile-ups that tank the frame rate) and offers a
//! safe one-click fix where one exists. Fixes go through the undo history.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use bevy::prelude::*;

use super::grid::{Class, Footprint, WalkGrid};
use super::job::{Reach, Snapshot};
use crate::history::History;
use crate::objects::{Category, Library, ObjectStore};
use crate::procgen::meshes::AssetKind;
use crate::terrain::Terrain;
use crate::world::Zones;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub const ALL: [Severity; 3] = [Severity::Error, Severity::Warning, Severity::Info];
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "Errors",
            Severity::Warning => "Warnings",
            Severity::Info => "Notes",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Check {
    Floating,
    Buried,
    Drowned,
    Overlap,
    SpawnTerrain,
    Underwater,
    Unreachable,
    Trap,
    Pocket,
    EmptyZone,
    Unnamed,
    OutsideZone,
    SteepRoad,
    Hotspot,
}

impl Check {
    pub const ALL: [Check; 14] = [
        Check::Underwater,
        Check::Unreachable,
        Check::SpawnTerrain,
        Check::Trap,
        Check::Floating,
        Check::Buried,
        Check::Drowned,
        Check::Overlap,
        Check::SteepRoad,
        Check::Hotspot,
        Check::EmptyZone,
        Check::Unnamed,
        Check::OutsideZone,
        Check::Pocket,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Check::Floating => "Floating objects",
            Check::Buried => "Buried objects",
            Check::Drowned => "Trees & plants under water",
            Check::Overlap => "Overlapping objects",
            Check::SpawnTerrain => "Spawns on steep ground or water",
            Check::Underwater => "POIs & NPCs under water",
            Check::Unreachable => "Unreachable gameplay",
            Check::Trap => "Player traps",
            Check::Pocket => "Unreachable areas",
            Check::EmptyZone => "Zones without content",
            Check::Unnamed => "Unnamed zones & places",
            Check::OutsideZone => "Gameplay outside any zone",
            Check::SteepRoad => "Steep roads",
            Check::Hotspot => "Performance hotspots",
        }
    }
    /// Stable id for MCP (`doctor_fix {"check": …}`).
    pub fn id(self) -> &'static str {
        match self {
            Check::Floating => "floating",
            Check::Buried => "buried",
            Check::Drowned => "drowned",
            Check::Overlap => "overlap",
            Check::SpawnTerrain => "spawn_terrain",
            Check::Underwater => "underwater",
            Check::Unreachable => "unreachable",
            Check::Trap => "trap",
            Check::Pocket => "pocket",
            Check::EmptyZone => "empty_zone",
            Check::Unnamed => "unnamed",
            Check::OutsideZone => "outside_zone",
            Check::SteepRoad => "steep_road",
            Check::Hotspot => "hotspot",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Check::Floating => "Hovering above the ground. Fix snaps them back down.",
            Check::Buried => {
                "Sunk far deeper into the ground than placement leaves them. Fix snaps them back up."
            }
            Check::Drowned => "Land plants standing in water deeper than 30 cm. Fix removes them.",
            Check::Overlap => "Large objects pushed into each other. Fix removes the smaller one.",
            Check::SpawnTerrain => "Creatures would spawn on slopes too steep to walk or in water.",
            Check::Underwater => "Players won't find them, and NPCs can't stand there.",
            Check::Unreachable => "Players can't walk to these from the player start.",
            Check::Trap => "Players can drop in here but can't walk back out.",
            Check::Pocket => {
                "Walkable ground nobody can reach on foot (often fine for mountain tops)."
            }
            Check::EmptyZone => "Zones with no POIs, NPCs, spawns or resource nodes inside.",
            Check::Unnamed => "Players see these names in the UI. Fix names zones for you.",
            Check::OutsideZone => {
                "Outside every zone: no level range, weather or ambience rules apply."
            }
            Check::SteepRoad => "Steeper than 15%: hard to ride and looks wrong for a road.",
            Check::Hotspot => {
                "More objects in one 128 m chunk than the budget. Fix thins out small plants and debris."
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Fix {
    /// Put objects back on the ground.
    Snap(Vec<u64>),
    Remove(Vec<u64>),
    NameZone(u8, String),
}

impl Fix {
    pub fn label(&self) -> &'static str {
        match self {
            Fix::Snap(_) => "Snap to ground",
            Fix::Remove(ids) if ids.len() > 1 => "Remove extras",
            Fix::Remove(_) => "Remove",
            Fix::NameZone(..) => "Name it",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub id: u32,
    pub check: Check,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub pos: Vec3,
    pub fix: Option<Fix>,
}

#[derive(Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub ms: f32,
    pub objects: usize,
}

/// How deep the scatter brush sinks an object (bases on slopes go a little deeper).
pub fn expected_sink(category: Option<Category>, scale: f32, radius: f32, slope_deg: f32) -> f32 {
    match category {
        Some(Category::Trees | Category::Rocks | Category::Plants | Category::Debris) => {
            0.15 * scale + slope_deg.to_radians().tan().min(1.5) * radius * scale * 0.15
        }
        _ => 0.05,
    }
}

fn thousands(v: usize) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A name for an unnamed zone from the ground it covers, unique among `taken`.
fn zone_name(snap: &Snapshot, g: &WalkGrid, id: u8, taken: &[String]) -> String {
    let (mut cells, mut water, mut high, mut steep) = (0usize, 0usize, 0usize, 0usize);
    for k in 0..g.len() {
        if g.zone[k] != id {
            continue;
        }
        cells += 1;
        match g.class[k] {
            Class::Swim | Class::OpenSea | Class::Shallow => water += 1,
            Class::Steep | Class::Cliff => steep += 1,
            _ => {}
        }
        if g.surface[k] - snap.field.sea > 140.0 {
            high += 1;
        }
    }
    let c = cells.max(1) as f32;
    let names: &[&str] = if water as f32 / c > 0.2 {
        &[
            "Saltwind Shore",
            "Gullwing Coast",
            "Driftwater Bay",
            "Tidemark Strand",
            "Pearlfoam Cove",
            "Kelpshadow Reach",
        ]
    } else if high as f32 / c > 0.4 || steep as f32 / c > 0.3 {
        &[
            "Greyspire Heights",
            "Windscar Peaks",
            "Frostcrag Pass",
            "Eaglerest Ridge",
            "Stonehelm Highlands",
            "Cloudbreak Summit",
        ]
    } else {
        &[
            "Amberfield Vale",
            "Mossbrook Woods",
            "Hollowmere",
            "Thistledown Meadows",
            "Oakenshade Glen",
            "Willowmarch Fields",
        ]
    };
    let start = id as usize % names.len();
    for k in 0..names.len() {
        let n = names[(start + k) % names.len()];
        if !taken.iter().any(|t| t == n) {
            return n.to_string();
        }
    }
    format!("{} {}", names[start], id)
}

fn is_unnamed_zone(name: &str) -> bool {
    let t = name.trim();
    t.is_empty()
        || t.eq_ignore_ascii_case("new zone")
        || t.strip_prefix("Zone ")
            .is_some_and(|r| r.chars().all(|c| c.is_ascii_digit()))
}

fn is_default_name(name: &str, kind_name: &str) -> bool {
    let t = name.trim();
    t.is_empty()
        || t.eq_ignore_ascii_case(kind_name)
        || t.to_ascii_lowercase().starts_with("new ")
        || t.trim_end_matches(|c: char| c.is_ascii_digit() || c == ' ')
            .eq_ignore_ascii_case(kind_name)
}

/// Runs every check. `progress` gets ‰.
pub fn run(
    snap: &Snapshot,
    g: &WalkGrid,
    reach: &Reach,
    budget: u32,
    progress: &AtomicU32,
) -> Report {
    let t0 = std::time::Instant::now();
    let mut issues: Vec<Issue> = Vec::new();
    let f = &snap.field;
    let ground = |p: Vec2| Vec3::new(p.x, f.height_at(p.x, p.y), p.y);
    let mut push = |check: Check,
                    severity: Severity,
                    title: String,
                    detail: String,
                    pos: Vec3,
                    fix: Option<Fix>| {
        issues.push(Issue {
            id: 0,
            check,
            severity,
            title,
            detail,
            pos,
            fix,
        });
    };
    let gp = &snap.gameplay;

    // ---------------------------------------------------------------- objects
    for o in &snap.objects {
        let p2 = o.pos.xz();
        let gnd = f.height_at(p2.x, p2.y);
        let depth = f.water_surface(p2.x, p2.y) - gnd;
        let water_plant = matches!(o.kind, Some(AssetKind::Reeds | AssetKind::LilyPads));
        if matches!(o.category, Some(Category::Trees | Category::Plants))
            && !water_plant
            && depth > 0.3
        {
            push(
                Check::Drowned,
                Severity::Warning,
                format!("{} in {:.1} m of water", o.name, depth),
                "A land plant growing under water.".into(),
                o.pos,
                Some(Fix::Remove(vec![o.id])),
            );
            continue;
        }
        if o.category == Some(Category::Props)
            || o.walkable
            || matches!(o.foot, Footprint::Deck { .. })
        {
            continue;
        }
        // floating on water on purpose (lily pads, driftwood)
        if depth > 0.2 && (o.pos.y - (gnd + depth)).abs() < 0.6 {
            continue;
        }
        let above = o.pos.y - gnd;
        if above > 0.4 {
            push(
                Check::Floating,
                Severity::Warning,
                format!("{} floats {:.1} m up", o.name, above),
                "Hovering above the ground.".into(),
                o.pos,
                Some(Fix::Snap(vec![o.id])),
            );
            continue;
        }
        let slope = f.slope_deg(p2.x, p2.y, f.cell);
        let sink = expected_sink(o.category, o.scale, o.radius, slope);
        let rock = o.category == Some(Category::Rocks);
        let allow = if rock {
            (0.5 * o.height * o.scale).max(1.0)
        } else {
            (0.3 * o.height * o.scale).max(0.8)
        };
        let deep = gnd - o.pos.y - sink;
        if deep > allow {
            push(
                Check::Buried,
                Severity::Warning,
                format!("{} buried {:.1} m deep", o.name, gnd - o.pos.y),
                format!("Placement sinks it about {sink:.1} m; it's {deep:.1} m deeper than that."),
                Vec3::new(p2.x, gnd, p2.y),
                Some(Fix::Snap(vec![o.id])),
            );
        }
    }
    progress.store(250, Ordering::Relaxed);

    // overlaps between large objects (spatial hash, 16 m cells)
    let large: Vec<(usize, f32, bool)> = snap
        .objects
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            // logs and stumps lie around trees and rocks on purpose
            if o.category == Some(Category::Debris) {
                return None;
            }
            let tree = o.category == Some(Category::Trees);
            let r = match o.foot {
                Footprint::Disc { radius, .. } => radius,
                Footprint::Rect { half, .. } => half.length() * 0.8,
                _ => return None,
            };
            let big = tree || r >= 1.0;
            big.then_some((i, r, tree))
        })
        .collect();
    let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (li, (i, _, _)) in large.iter().enumerate() {
        let p = snap.objects[*i].pos;
        cells
            .entry(((p.x / 16.0).floor() as i32, (p.z / 16.0).floor() as i32))
            .or_default()
            .push(li);
    }
    for (li, &(i, ra, ta)) in large.iter().enumerate() {
        let a = &snap.objects[i];
        let c = (
            (a.pos.x / 16.0).floor() as i32,
            (a.pos.z / 16.0).floor() as i32,
        );
        for dz in -1..=1 {
            for dx in -1..=1 {
                let Some(list) = cells.get(&(c.0 + dx, c.1 + dz)) else {
                    continue;
                };
                for &lj in list {
                    if lj <= li {
                        continue;
                    }
                    let (j, rb, tb) = large[lj];
                    let b = &snap.objects[j];
                    let d = a.pos.xz().distance(b.pos.xz());
                    let limit = match (ta, tb) {
                        // trunks touching
                        (true, true) => ra + rb,
                        // a trunk well inside a rock or building
                        (true, false) => rb * 0.6,
                        (false, true) => ra * 0.6,
                        // pushed more than halfway into each other
                        (false, false) => (ra + rb) * 0.5,
                    };
                    if d < limit {
                        let (small, big) = if ra * a.scale <= rb * b.scale {
                            (a, b)
                        } else {
                            (b, a)
                        };
                        push(
                            Check::Overlap,
                            Severity::Warning,
                            format!("{} inside {}", small.name, big.name),
                            format!("Centres {d:.1} m apart."),
                            (a.pos + b.pos) * 0.5,
                            Some(Fix::Remove(vec![small.id])),
                        );
                    }
                }
            }
        }
    }
    progress.store(400, Ordering::Relaxed);

    // performance hotspots per 128 m chunk
    let mut chunks: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, o) in snap.objects.iter().enumerate() {
        chunks
            .entry(crate::objects::chunk_of(o.pos.x, o.pos.z))
            .or_default()
            .push(i);
    }
    let budget = budget.max(1) as usize;
    let mut hot: Vec<_> = chunks.iter().filter(|(_, v)| v.len() > budget).collect();
    hot.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (c, list) in hot {
        let mut by_cat: HashMap<&'static str, usize> = HashMap::new();
        for &i in list {
            *by_cat
                .entry(
                    snap.objects[i]
                        .category
                        .map(|c| c.name())
                        .unwrap_or("Other"),
                )
                .or_default() += 1;
        }
        let mut parts: Vec<_> = by_cat.into_iter().collect();
        parts.sort_by(|a, b| b.1.cmp(&a.1));
        let detail = parts
            .iter()
            .map(|(n, c)| format!("{} {}", thousands(*c), n.to_lowercase()))
            .collect::<Vec<_>>()
            .join(", ");
        // thin the small stuff first, deterministically
        let mut small: Vec<&super::job::ObjInfo> = list
            .iter()
            .map(|&i| &snap.objects[i])
            .filter(|o| {
                matches!(o.category, Some(Category::Plants | Category::Debris))
                    || o.kind == Some(AssetKind::Pebbles)
            })
            .collect();
        small.sort_by_key(|o| o.id.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let extra = list.len() - budget;
        let remove: Vec<u64> = small.iter().take(extra).map(|o| o.id).collect();
        let centre = Vec2::new(
            (c.0 as f32 + 0.5) * crate::objects::CHUNK,
            (c.1 as f32 + 0.5) * crate::objects::CHUNK,
        );
        let sev = if list.len() > budget * 2 {
            Severity::Error
        } else {
            Severity::Warning
        };
        push(
            Check::Hotspot,
            sev,
            format!(
                "{} objects in one chunk (budget {})",
                thousands(list.len()),
                thousands(budget)
            ),
            detail,
            ground(centre),
            (!remove.is_empty()).then_some(Fix::Remove(remove)),
        );
    }
    progress.store(500, Ordering::Relaxed);

    // ---------------------------------------------------------------- gameplay
    let water_depth = |p: [f32; 3]| f.water_surface(p[0], p[2]) - f.height_at(p[0], p[2]);
    for poi in &gp.pois {
        let d = water_depth(poi.pos);
        if d > 0.6 {
            push(
                Check::Underwater,
                Severity::Warning,
                format!("POI '{}' is {:.1} m under water", poi.name, d),
                "Players won't see or reach it on foot.".into(),
                Vec3::from_array(poi.pos),
                None,
            );
        }
    }
    for npc in &gp.npcs {
        let d = water_depth(npc.pos);
        if d > 0.6 {
            push(
                Check::Underwater,
                Severity::Error,
                format!("NPC '{}' stands in {:.1} m of water", npc.name, d),
                "NPCs need dry ground to stand on.".into(),
                Vec3::from_array(npc.pos),
                None,
            );
        }
    }
    for it in &reach.items {
        let sev = if matches!(it.kind, "POI" | "NPC") {
            Severity::Error
        } else {
            Severity::Warning
        };
        push(
            Check::Unreachable,
            sev,
            format!("{} '{}' can't be reached", it.kind, it.name),
            it.why.to_string(),
            it.pos,
            None,
        );
    }
    for sp in &gp.spawns {
        if sp.polygon.len() < 3 {
            continue;
        }
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in &sp.polygon {
            lo = lo.min(Vec2::from_array(*p));
            hi = hi.max(Vec2::from_array(*p));
        }
        let step = (((hi - lo).max_element() / g.cell) / 160.0).ceil().max(1.0) as usize;
        let (mut all, mut steep, mut wet, mut blocked) = (0usize, 0usize, 0usize, 0usize);
        let Some(k0) = g.cell_at(lo.max(g.origin)) else {
            continue;
        };
        let k1 = g
            .cell_at(hi.min(g.origin + Vec2::splat(g.n as f32 * g.cell - 0.01)))
            .unwrap_or(k0);
        let (i0, j0, i1, j1) = (k0 % g.n, k0 / g.n, k1 % g.n, k1 / g.n);
        for j in (j0..=j1).step_by(step) {
            for i in (i0..=i1).step_by(step) {
                let k = g.idx(i, j);
                if !sp.contains(g.center(k)) {
                    continue;
                }
                all += 1;
                match g.class[k] {
                    Class::Steep | Class::Cliff => steep += 1,
                    Class::Swim | Class::OpenSea => wet += 1,
                    Class::Obstacle | Class::Outside => blocked += 1,
                    _ => {}
                }
            }
        }
        if all == 0 {
            continue;
        }
        let bad = (steep + wet) as f32 / all as f32;
        if bad > 0.35 {
            let what = match (steep > 0, wet > 0) {
                (true, true) => "steep or under water",
                (true, false) => "too steep to walk",
                _ => "under water",
            };
            let sev = if bad > 0.7 {
                Severity::Error
            } else {
                Severity::Warning
            };
            let detail = format!(
                "{:.0}% steep, {:.0}% water, {:.0}% blocked by objects.",
                steep as f32 * 100.0 / all as f32,
                wet as f32 * 100.0 / all as f32,
                blocked as f32 * 100.0 / all as f32
            );
            let c = sp.centroid().unwrap_or(lo);
            push(
                Check::SpawnTerrain,
                sev,
                format!("Spawn '{}': {:.0}% {what}", sp.name, bad * 100.0),
                detail,
                ground(c),
                None,
            );
        }
    }
    for t in &reach.traps {
        if t.area_km2 * 1.0e6 >= 200.0 {
            push(
                Check::Trap,
                Severity::Warning,
                format!(
                    "Pit of {:.0} m² players can't climb out of",
                    t.area_km2 * 1.0e6
                ),
                "Reachable by sliding or dropping in, but every way out is too steep.".into(),
                t.pos,
                None,
            );
        }
    }
    for pk in &reach.pockets {
        if pk.area_km2 >= 0.005 {
            push(
                Check::Pocket,
                Severity::Info,
                format!("{:.3} km² of walkable ground out of reach", pk.area_km2),
                format!("Not reachable on foot from {}.", reach.origin_label),
                pk.pos,
                None,
            );
        }
    }
    progress.store(700, Ordering::Relaxed);

    // ---------------------------------------------------------------- zones and names
    // cells and centroid per zone
    let mut zone_cells: HashMap<u8, (usize, Vec2)> = HashMap::new();
    for (k, z) in g.zone.iter().enumerate() {
        if *z > 0 {
            let e = zone_cells.entry(*z).or_default();
            e.0 += 1;
            e.1 += g.center(k);
        }
    }
    let content: Vec<Vec2> = gp.content_points();
    let taken: Vec<String> = snap.zones.iter().map(|z| z.name.clone()).collect();
    for z in &snap.zones {
        let (painted, sum) = zone_cells.get(&z.id).copied().unwrap_or_default();
        let anchor = ground(sum / painted.max(1) as f32);
        if painted == 0 {
            push(
                Check::EmptyZone,
                Severity::Info,
                format!("Zone '{}' isn't painted on the map", z.name),
                "It has rules but covers no ground yet.".into(),
                anchor,
                None,
            );
        } else if !content.iter().any(|c| f.zone_at(c.x, c.y) == z.id) {
            let km2 = painted as f64 * g.cell_area_km2();
            push(
                Check::EmptyZone,
                Severity::Warning,
                format!("Zone '{}' has nothing to do", z.name),
                format!("{km2:.2} km² with no POIs, NPCs, spawns or resource nodes."),
                anchor,
                None,
            );
        }
        if is_unnamed_zone(&z.name) {
            let name = zone_name(snap, g, z.id, &taken);
            push(
                Check::Unnamed,
                Severity::Info,
                format!("Zone {} has no real name", z.id),
                format!("Suggested: {name}"),
                anchor,
                Some(Fix::NameZone(z.id, name)),
            );
        }
    }
    for poi in &gp.pois {
        if is_default_name(&poi.name, poi.kind.name()) {
            push(
                Check::Unnamed,
                Severity::Info,
                format!("{} has no name", poi.kind.name()),
                "Players see POI names on the map and in quests.".into(),
                Vec3::from_array(poi.pos),
                None,
            );
        }
    }
    for npc in &gp.npcs {
        if is_default_name(&npc.name, npc.role.name()) {
            push(
                Check::Unnamed,
                Severity::Info,
                format!("{} NPC has no name", npc.role.name()),
                "Name it in the Gameplay tab.".into(),
                Vec3::from_array(npc.pos),
                None,
            );
        }
    }
    for sp in &gp.spawns {
        if is_default_name(&sp.name, "Spawn Region") {
            let c = sp.centroid().unwrap_or_default();
            push(
                Check::Unnamed,
                Severity::Info,
                "Spawn region has no name".into(),
                "Names make encounter tables readable.".into(),
                ground(c),
                None,
            );
        }
    }
    if !snap.zones.is_empty() {
        let mut outside = |kind: &str, name: &str, p: Vec3| {
            if f.zone_at(p.x, p.z) == 0 {
                push(
                    Check::OutsideZone,
                    Severity::Info,
                    format!("{kind} '{name}' is outside every zone"),
                    "No level range, weather or ambience applies there.".into(),
                    p,
                    None,
                );
            }
        };
        for p in &gp.pois {
            outside("POI", &p.name, Vec3::from_array(p.pos));
        }
        for p in &gp.npcs {
            outside("NPC", &p.name, Vec3::from_array(p.pos));
        }
        for p in &gp.nodes {
            outside("Node", &p.variant, Vec3::from_array(p.pos));
        }
        for sp in &gp.spawns {
            if let Some(c) = sp.centroid() {
                outside("Spawn", &sp.name, ground(c));
            }
        }
    }
    progress.store(850, Ordering::Relaxed);

    // ---------------------------------------------------------------- roads
    for r in &snap.roads {
        let pts: Vec<Vec3> = r.pts.iter().map(|p| ground(p.xz())).collect();
        if pts.len() < 2 {
            continue;
        }
        // grade over ~10 m windows
        let mut dist = vec![0.0f32; pts.len()];
        for k in 1..pts.len() {
            dist[k] = dist[k - 1] + pts[k].xz().distance(pts[k - 1].xz());
        }
        let mut grades = vec![0.0f32; pts.len()];
        let mut e = 0;
        for s in 0..pts.len() {
            while e + 1 < pts.len() && dist[e] - dist[s] < 10.0 {
                e += 1;
            }
            let d = dist[e] - dist[s];
            if d > 4.0 {
                grades[s] = (pts[e].y - pts[s].y).abs() / d;
            }
        }
        let mut s = 0;
        while s < pts.len() {
            if grades[s] <= 0.15 {
                s += 1;
                continue;
            }
            let mut e = s;
            let mut peak = s;
            while e + 1 < pts.len() && grades[e + 1] > 0.15 {
                e += 1;
                if grades[e] > grades[peak] {
                    peak = e;
                }
            }
            let len = (dist[e] - dist[s]).max(10.0);
            let g_ = grades[peak];
            let sev = if g_ > 0.3 {
                Severity::Error
            } else {
                Severity::Warning
            };
            let name = if r.name.trim().is_empty() {
                r.kind.name().to_string()
            } else {
                r.name.clone()
            };
            push(
                Check::SteepRoad,
                sev,
                format!("{name} climbs {:.0}% over {:.0} m", g_ * 100.0, len),
                "Re-route it along the slope or add switchbacks.".into(),
                pts[peak],
                None,
            );
            s = e + 1;
        }
    }

    issues.sort_by(|a, b| {
        a.severity.cmp(&b.severity).then(
            Check::ALL
                .iter()
                .position(|c| *c == a.check)
                .cmp(&Check::ALL.iter().position(|c| *c == b.check)),
        )
    });
    for (i, it) in issues.iter_mut().enumerate() {
        it.id = i as u32 + 1;
    }
    progress.store(1000, Ordering::Relaxed);
    Report {
        issues,
        ms: t0.elapsed().as_secs_f32() * 1000.0,
        objects: snap.objects.len(),
    }
}

/// Applies fixes as one undo step. Returns a toast message.
pub fn apply(
    fixes: &[&Fix],
    terrain: &Terrain,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    zones: &mut Zones,
    history: &mut History,
) -> Option<String> {
    let (mut snapped, mut removed, mut named) = (0, 0, 0);
    history.begin(if fixes.len() == 1 {
        format!("Doctor: {}", fixes[0].label().to_lowercase())
    } else {
        format!("Doctor: {} fixes", fixes.len())
    });
    for f in fixes {
        match f {
            Fix::Snap(ids) => {
                for id in ids {
                    let Some(p) = store.items.get(id).cloned() else {
                        continue;
                    };
                    let e = lib.and_then(|l| l.get(&p.asset));
                    let slope = terrain
                        .normal_at(p.pos[0], p.pos[2])
                        .y
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees();
                    let sink = expected_sink(
                        e.map(|e| e.category),
                        p.scale,
                        e.map(|e| e.radius).unwrap_or(1.0),
                        slope,
                    );
                    let mut np = p.clone();
                    np.pos[1] = terrain.height_at(p.pos[0], p.pos[2]) - sink;
                    history.touch_object(store, *id);
                    store.update(np);
                    snapped += 1;
                }
            }
            Fix::Remove(ids) => {
                for id in ids {
                    if store.items.contains_key(id) {
                        history.touch_object(store, *id);
                        store.remove(*id);
                        removed += 1;
                    }
                }
            }
            Fix::NameZone(id, name) => {
                if let Some(i) = zones.list.iter().position(|z| z.id == *id) {
                    history.touch_zones(zones);
                    zones.list[i].name = name.clone();
                    named += 1;
                }
            }
        }
    }
    history.commit(store);
    let mut parts = vec![];
    if snapped > 0 {
        parts.push(format!(
            "snapped {snapped} object{} to the ground",
            if snapped == 1 { "" } else { "s" }
        ));
    }
    if removed > 0 {
        parts.push(format!(
            "removed {removed} object{}",
            if removed == 1 { "" } else { "s" }
        ));
    }
    if named > 0 {
        parts.push(format!(
            "named {named} zone{}",
            if named == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        return None;
    }
    let mut msg = parts.join(", ");
    msg.replace_range(..1, &msg[..1].to_uppercase());
    Some(format!("{msg} (undo with ⌘Z)"))
}
