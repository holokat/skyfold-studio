//! What a sheet realm's build places with the editor's own tools, a step at a time (like the
//! temple compound's): low rails down both sides of every stair, the models that fill markers
//! (where an installed model fits the key's name and size), the exit portal or the return gate,
//! and the gameplay — the dungeon's creatures, a boss and a boss arena per ring, a camp in the
//! other zones, every zone named as a place, the sightline's vista. Everything is ordinary
//! content.

use bevy::math::Vec2;
use serde_json::json;

use super::blueprint::{Blueprint, EndKind, Role};
use super::land::{self, Site, w};
use super::markers::{self, Marker};
use crate::dungeons::compound::Theme;
use crate::dungeons::compound::furnish::{Step, call, obj, place, r2, stair_rails};

/// Fills the markers that an installed model fits (when `auto` is on), returning their
/// `objects_place` items; the markers are marked filled.
pub fn auto_fill(ms: &mut [Marker], auto: bool) -> Vec<serde_json::Value> {
    if !auto {
        return vec![];
    }
    let mut items = vec![];
    // (one model per key letter, so the copies match)
    let mut chosen: std::collections::HashMap<String, Option<(String, f32)>> = Default::default();
    for m in ms.iter_mut() {
        let pick = chosen
            .entry(m.letter.clone())
            .or_insert_with(|| markers::suggest(m, 1).into_iter().next())
            .clone();
        if let Some((asset, scale)) = pick {
            items.push(markers::place_item(m, &asset, scale));
            m.filled = Some(asset);
        }
    }
    items
}

/// Every step that builds the realm, in order.
pub fn steps(
    bp: &Blueprint,
    site: &Site,
    theme: Theme,
    fill: Vec<serde_json::Value>,
    levels: [u8; 2],
) -> Vec<Step> {
    let kit = theme.kit();
    let mut out = vec![Step::Wait(12)];
    // --- the stairs' rails
    let mut rails = vec![];
    for (head, foot, width) in land::stair_ends(site) {
        if head.distance(foot) > 2.0 {
            rails.extend(stair_rails(head, foot, width.min(12.0), kit));
        }
    }
    out.extend(place("The stairs' rails", rails));
    // --- the models that fit the key (in batches, so a frame stays short)
    for (i, chunk) in fill.chunks(40).enumerate() {
        out.extend(place(format!("Dressing ({})", i + 1), chunk.to_vec()));
    }
    // --- the way the dungeon ends
    let finale = bp.finale().map(|z| z.num);
    if let Some(at) = bp
        .ending
        .at
        .map(w)
        .or_else(|| finale.and_then(|n| site.zone(n)).map(|z| z.c))
    {
        match bp.ending.kind {
            EndKind::Exit => {
                out.extend(place(
                    "The exit portal",
                    vec![obj(("portal_ring", 1.0), at, 0.0, 1.0)],
                ));
                out.push(call("Exit portal", "gameplay_add_poi", json!({ "kind": "Portal", "position": [r2(at.x), r2(at.y)], "name": "Exit portal", "radius": 6.0, "notes": "Opens once the final boss has fallen: the way out of the dungeon." })));
            }
            EndKind::Return => {
                out.extend(place(
                    "The return gate",
                    vec![obj(kit.outer_gate, at, 0.0, 1.0)],
                ));
                out.push(call("Return gate", "gameplay_add_poi", json!({ "kind": "Landmark", "position": [r2(at.x), r2(at.y)], "name": "Return gate", "radius": 8.0, "notes": "One-way: opens from the inside, back toward the start." })));
            }
            EndKind::Shortcut => {
                out.push(call("Shortcut", "gameplay_add_poi", json!({ "kind": "Landmark", "position": [r2(at.x), r2(at.y)], "name": "Shortcut", "radius": 6.0, "notes": "One-way shortcut back toward the start." })));
            }
        }
    }
    out.extend(gameplay(bp, site, theme, levels));
    out
}

fn gameplay(bp: &Blueprint, site: &Site, theme: Theme, levels: [u8; 2]) -> Vec<Step> {
    let kit = theme.kit();
    let mut out = vec![];
    let [lo, hi] = [levels[0].max(1), levels[1].max(levels[0]).max(1)];
    for (name, family) in kit.trash {
        out.push(call(
            format!("Creature: {name}"),
            "gameplay_add_creature",
            json!({ "name": name, "family": family, "level": [lo, hi], "aggression": "Hostile" }),
        ));
    }
    let bosses: Vec<_> = bp.zones.iter().filter(|z| z.boss.is_some()).collect();
    let nb = bosses.len().max(1);
    for (i, z) in bosses.iter().enumerate() {
        let zw = site.zone(z.num).unwrap();
        let (c, r) = zw.ring.unwrap_or((zw.c, 12.0));
        let level = (lo as f32 + (hi - lo) as f32 * (i + 1) as f32 / nb as f32).round() as u8;
        let boss = boss_name(&z.name, z.role == Role::Final, kit, i);
        out.push(call(format!("Boss: {boss}"), "gameplay_add_creature", json!({ "name": boss, "family": kit.boss_family, "level": [level, level], "aggression": "Hostile", "elite": true, "notes": format!("The boss of {}", z.name) })));
        out.push(call(format!("Boss arena: {}", z.name), "gameplay_add_poi", json!({ "kind": "BossArena", "position": [r2(c.x), r2(c.y)], "name": z.name, "radius": r2(r + 4.0), "level": [level.saturating_sub(1).max(1), level], "notes": format!("{} (zone {:02}).", z.boss.as_ref().map(|b| b.label.as_str()).unwrap_or("Boss"), z.num) })));
        out.push(call(format!("Boss spawn: {boss}"), "gameplay_add_spawn_region", json!({ "center": [r2(c.x), r2(c.y)], "radius": 9.0, "creatures": [boss], "name": boss, "level": [level, level], "behaviour": "Static", "pack": [1, 1], "max_alive": 1, "respawn_secs": 0 })));
    }
    // a camp in every zone without a boss (not the Arrival)
    for (i, z) in bp
        .zones
        .iter()
        .filter(|z| z.boss.is_none() && z.role != Role::Arrival)
        .enumerate()
    {
        let zw = site.zone(z.num).unwrap();
        let a = kit.trash[i % kit.trash.len()].0;
        let b = kit.trash[(i + 1) % kit.trash.len()].0;
        let r = (zw.h.x.min(zw.h.y) * 0.6).clamp(9.0, 22.0);
        out.push(call(format!("Camp: {}", z.name), "gameplay_add_spawn_region", json!({ "center": [r2(zw.c.x), r2(zw.c.y)], "radius": r2(r), "creatures": [a, b], "weights": [2, 1], "name": z.name, "level": [lo, hi], "behaviour": "Static", "pack": [2, 4], "max_alive": 5, "respawn_secs": 600 })));
    }
    for z in &bp.zones {
        if z.boss.is_some() {
            continue;
        }
        let zw = site.zone(z.num).unwrap();
        let kind = if z.role == Role::Side {
            "Treasure"
        } else {
            "Landmark"
        };
        out.push(call(format!("Place: {}", z.name), "gameplay_add_poi", json!({ "kind": kind, "position": [r2(zw.c.x), r2(zw.c.y)], "name": z.name, "radius": r2(zw.h.x.max(zw.h.y)), "notes": format!("Zone {:02} of the sheet, {:+.0} m.", z.num, z.elevation) })));
    }
    if let Some([eye, _]) = bp.sightline {
        let p = w(eye);
        out.push(call("Vista", "gameplay_add_poi", json!({ "kind": "Vista", "position": [r2(p.x), r2(p.y)], "name": "Overlook", "radius": 8.0, "notes": "The finale's landmark is in view from here." })));
    }
    out
}

/// A boss's name from its zone's name and the kit's titles ("Warden of the Sentinel Court").
fn boss_name(
    zone: &str,
    last: bool,
    kit: &crate::dungeons::compound::theme::Kit,
    i: usize,
) -> String {
    let z = zone
        .trim()
        .trim_start_matches("The ")
        .trim_start_matches("the ");
    if last {
        return format!("The Lord of {z}");
    }
    let titles = [
        "Warden", "Keeper", "Guardian", "Master", "Matron", "Overseer",
    ];
    let _ = kit;
    format!("The {} of {z}", titles[i % titles.len()])
}

/// The markers' spots sorted for the build log (for tests).
pub fn spots(ms: &[Marker]) -> Vec<Vec2> {
    ms.iter().map(|m| Vec2::new(m.at[0], m.at[1])).collect()
}
