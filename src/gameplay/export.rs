//! Server data: the gameplay layer as `gameplay.json` for a game server (format documented in
//! `docs/server-data.md`). Every item carries the zone it lies in and resolved level ranges, so
//! a server can load it without the editor's terrain or zone maps. Heights are re-read from the
//! current terrain. Mining nodes carry their ore; `ore_deposits` lists them all with the ore's
//! tier and rarity, `deposit_areas` the areas they were scattered in, `ores` the catalogue.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde_json::{Value, json};

use super::edit::{Ground, level_label, levels_of};
use super::{Gameplay, Ore};
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::Zones;

pub const FORMAT: &str = "world-builder-gameplay";
pub const VERSION: u32 = 1;

/// Centimetre precision (as f64, so the JSON has no float noise).
fn round(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn pos(g: &Ground, p: Vec2) -> Value {
    let q = g.at(p);
    json!([round(q.x), round(q.y), round(q.z)])
}

fn zone_ref(g: &Ground, id: Option<u8>) -> Value {
    match id.and_then(|z| g.zones.by_id(z)) {
        Some(z) => {
            json!({"id": z.id, "name": z.name, "level": levels_of(z), "type": format!("{:?}", z.zone_type)})
        }
        None => Value::Null,
    }
}

fn zone_at(g: &Ground, p: Vec2) -> Value {
    zone_ref(g, Some(g.terrain.zone_at(p.x, p.y)))
}

/// The whole gameplay layer as server data.
pub fn server_data(gp: &Gameplay, g: &Ground, world_name: &str) -> Value {
    let xz = |p: [f32; 3]| Vec2::new(p[0], p[2]);
    let player_start = gp.player_start.map(|s| {
        json!({
            "position": pos(g, xz(s.pos)),
            "yaw_rad": round(s.yaw),
            "facing": [round(s.yaw.cos()), 0.0, round(-s.yaw.sin())],
            "zone": zone_at(g, xz(s.pos)),
        })
    });
    let pois: Vec<Value> = gp
        .pois
        .iter()
        .map(|q| {
            let p = xz(q.pos);
            let zone_level = g.zone_levels(p);
            json!({
                "id": q.id,
                "kind": format!("{:?}", q.kind),
                "kind_name": q.kind.name(),
                "name": q.name,
                "position": pos(g, p),
                "discovery_radius_m": round(q.radius),
                "level": q.level.or(zone_level),
                "level_source": if q.level.is_some() { "custom" } else if zone_level.is_some() { "zone" } else { "none" },
                "zone": zone_at(g, p),
                "npcs": gp.npcs.iter().filter(|n| n.poi == Some(q.id)).map(|n| n.id).collect::<Vec<_>>(),
                "notes": q.notes,
                "tags": q.tags,
            })
        })
        .collect();
    let npcs: Vec<Value> = gp
        .npcs
        .iter()
        .map(|n| {
            let p = xz(n.pos);
            let poi = n.poi.and_then(|id| gp.pois.iter().find(|q| q.id == id));
            json!({
                "id": n.id,
                "name": n.name,
                "role": format!("{:?}", n.role),
                "position": pos(g, p),
                "yaw_rad": round(n.yaw),
                "poi": poi.map(|q| q.id),
                "poi_name": poi.map(|q| q.name.clone()),
                "zone": zone_at(g, p),
                "notes": n.notes,
            })
        })
        .collect();
    let creatures: Vec<Value> = gp
        .creatures
        .iter()
        .map(|c| {
            json!({
                "id": c.id, "name": c.name, "family": c.family, "level": c.level,
                "aggression": format!("{:?}", c.aggression), "elite": c.elite, "notes": c.notes,
            })
        })
        .collect();
    let spawns: Vec<Value> = gp
        .spawns
        .iter()
        .map(|r| {
            let info = g.region_info(&r.polygon);
            let c = r.centroid().unwrap_or_default();
            let chances = r.chances();
            json!({
                "id": r.id,
                "name": r.name,
                "polygon_xz": r.polygon.iter().map(|p| [round(p[0]), round(p[1])]).collect::<Vec<_>>(),
                "centroid": pos(g, c),
                "area_m2": round(r.area()),
                "zone": zone_ref(g, info.zone),
                "zones": info.zones,
                "level": r.level,
                "density_per_km2": round(r.density),
                "expected_alive": r.expected_alive(),
                "max_alive": r.max_alive,
                "respawn_secs": round(r.respawn_secs),
                "pack_size": r.pack,
                "behaviour": format!("{:?}", r.behaviour),
                "aggression": r.aggression(gp).map(|a| format!("{a:?}")),
                "encounter_table": r.entries.iter().zip(chances).map(|(e, ch)| {
                    let cr = gp.creature(e.creature);
                    json!({
                        "creature": e.creature,
                        "creature_name": cr.map(|c| c.name.clone()),
                        "weight": round(e.weight),
                        "chance": round(ch),
                    })
                }).collect::<Vec<_>>(),
                "notes": r.notes,
            })
        })
        .collect();
    let nodes: Vec<Value> = gp
        .nodes
        .iter()
        .map(|n| {
            let p = xz(n.pos);
            let ore = n.ore();
            json!({
                "id": n.id,
                "kind": format!("{:?}", n.kind),
                "variant": n.variant,
                "tier": ore.map(|o| o.tier()).unwrap_or(n.tier),
                "ore": ore.map(|o| o.id()),
                "rarity": ore.map(|o| o.rarity().name()),
                "area": n.area,
                "position": pos(g, p),
                "respawn_secs": round(n.respawn_secs),
                "zone": zone_at(g, p),
                "auto_placed": n.auto,
            })
        })
        .collect();
    let deposits: Vec<Value> = gp
        .nodes
        .iter()
        .filter_map(|n| {
            let o = n.ore()?;
            let p = xz(n.pos);
            Some(json!({
                "id": n.id,
                "ore": o.id(),
                "ore_name": o.name(),
                "name": n.variant,
                "tier": o.tier(),
                "rarity": o.rarity().name(),
                "position": pos(g, p),
                "respawn_secs": round(n.respawn_secs),
                "area": n.area,
                "zone": zone_at(g, p),
                "auto_placed": n.auto,
            }))
        })
        .collect();
    let areas: Vec<Value> = gp
        .ore_areas
        .iter()
        .map(|a| {
            let info = g.region_info(&a.polygon);
            let ids: Vec<u32> = gp.nodes.iter().filter(|n| n.area == Some(a.id)).map(|n| n.id).collect();
            json!({
                "id": a.id,
                "name": a.name,
                "polygon_xz": a.polygon.iter().map(|p| [round(p[0]), round(p[1])]).collect::<Vec<_>>(),
                "centroid": pos(g, a.centroid().unwrap_or_default()),
                "area_m2": round(a.area()),
                "zone": zone_ref(g, info.zone),
                "density_per_ha": round(a.density),
                "respawn_secs": round(a.respawn_secs),
                "seed": a.seed,
                "ore_table": a.table.iter().zip(a.chances()).map(|(e, ch)| json!({
                    "ore": e.ore.id(), "ore_name": e.ore.name(), "tier": e.ore.tier(), "rarity": e.ore.rarity().name(),
                    "weight": round(e.weight), "chance": round(ch),
                })).collect::<Vec<_>>(),
                "deposits": ids.len(),
                "deposit_ids": ids,
                "notes": a.notes,
            })
        })
        .collect();
    let catalogue: Vec<Value> = Ore::ALL
        .iter()
        .map(|o| json!({"id": o.id(), "name": o.name(), "tier": o.tier(), "rarity": o.rarity().name(), "typical_use": o.uses(), "found_in": o.habitat().about, "gathered_as": format!("{:?}", o.kind())}))
        .collect();
    let fishing: Vec<Value> = g
        .water
        .fish
        .iter()
        .map(|f| {
            let p = Vec2::new(f.center[0], f.center[2]);
            json!({
                "id": f.id,
                "species": f.species.name(),
                "habitat": f.species.habitat(),
                "position": [round(f.center[0]), round(f.center[1]), round(f.center[2])],
                "radius_m": round(f.radius),
                "fish_count": f.count,
                "zone": zone_at(g, p),
            })
        })
        .collect();
    let zones: Vec<Value> = g
        .zones
        .list
        .iter()
        .map(|z| json!({"id": z.id, "name": z.name, "level": levels_of(z), "level_label": level_label(levels_of(z)), "type": format!("{:?}", z.zone_type)}))
        .collect();
    json!({
        "format": FORMAT,
        "version": VERSION,
        "world": {
            "name": world_name,
            "size_m": g.terrain.size(),
            "sea_level_m": g.terrain.sea_level,
            "level_cap": gp.level_cap,
            "origin": "world (0,0) is the map centre; +X east, +Z south, Y up (metres)",
            "yaw": "radians, 0 = +X (east), counter-clockwise seen from above (π/2 = north, −Z)",
        },
        "counts": {
            "pois": pois.len(), "npcs": npcs.len(), "creatures": creatures.len(), "spawn_regions": spawns.len(),
            "resource_nodes": nodes.len(), "fishing_spots": fishing.len(),
            "ore_deposits": deposits.len(), "deposit_areas": areas.len(),
        },
        "zones": zones,
        "player_start": player_start,
        "pois": pois,
        "npcs": npcs,
        "creatures": creatures,
        "spawn_regions": spawns,
        "resource_nodes": nodes,
        "ore_deposits": deposits,
        "deposit_areas": areas,
        "ores": catalogue,
        "fishing_spots": fishing,
    })
}

/// Writes `gameplay.json` into `dir` (created if needed). Returns the file path.
pub fn write_server_data(
    dir: &Path,
    gp: &Gameplay,
    terrain: &Terrain,
    zones: &Zones,
    water: &WaterBodies,
    world_name: &str,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let g = Ground {
        terrain,
        zones,
        water,
        nature: None,
    };
    let data = server_data(gp, &g, world_name);
    let path = dir.join("gameplay.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}
