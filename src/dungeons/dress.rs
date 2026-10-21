//! Dress: furnishes every room by what it is for (its role) from the object library — coffins,
//! candles and statues in crypts, cages and bones in prisons, bookcases and lecterns in
//! libraries, anvils and weapon racks in forges, a throne on a boss's dais, chests in treasure
//! rooms, bones in lairs, barrels and crates where things are kept — set against the walls
//! (facing in) or in the middle, never in doorways, pillars or each other. Hand-painted styles
//! take hand-painted pieces, photoreal ones photoscanned pieces where there are any. The objects
//! are ordinary objects (move, delete or add more by hand); Dress again replaces only its own.

use std::sync::Arc;

use bevy::prelude::*;

use super::plan::{CRoom, Compiled, Role, RoomKind};
use super::render::DungeonWorld;
use crate::io::WorldData;
use crate::noise::Rng;

/// Where a piece goes.
#[derive(Clone, Copy, PartialEq)]
enum Spot {
    /// against a wall, facing in
    Wall,
    /// in the middle (or on the dais)
    Centre,
    /// anywhere on the floor
    Floor,
}

/// (asset, where, how many per 100 m² of floor (wall pieces: per 10 m of wall), at least)
type Piece = (&'static str, Spot, f32, u32);

fn pieces(role: Role, hp: bool) -> Vec<Piece> {
    use Spot::*;
    let mut v: Vec<Piece> = match role {
        Role::Crypt => vec![
            ("hp:coffin", Floor, 1.2, 2),
            ("hp:candles", Wall, 1.2, 1),
            ("hp:skull_pile", Wall, 0.8, 0),
            ("hp:winged_statue", Wall, 0.25, 0),
            ("hp:gravestone_skull", Wall, 0.4, 0),
        ],
        Role::Prison => vec![
            ("hp:orc_iron_cage", Wall, 0.6, 1),
            ("hp:crossed_bones", Floor, 0.8, 0),
            ("hp:bone_pile", Wall, 0.5, 0),
        ],
        Role::Shrine => vec![
            ("hp:skullbanner_oath_altar", Centre, 0.0, 1),
            ("hp:candles", Wall, 1.0, 2),
            ("hp:winged_statue", Wall, 0.3, 0),
        ],
        Role::Library => vec![
            (
                if hp {
                    "hp:dragonbone_bookcase"
                } else {
                    "ph:wooden_bookshelf_worn"
                },
                Wall,
                2.2,
                2,
            ),
            ("hp:dragonbone_reading_table", Floor, 0.6, 1),
            ("hp:dragonbone_lectern", Centre, 0.0, 1),
            ("hp:dragonbone_scroll_urn", Wall, 0.4, 0),
        ],
        Role::Forge => vec![
            ("hp:anvil", Floor, 0.6, 1),
            ("hp:weapon_rack", Wall, 0.8, 1),
            ("hp:barrel", Wall, 0.6, 0),
            ("hp:ashhammer_crate", Wall, 0.5, 0),
        ],
        Role::Barracks => vec![
            ("hp:weapon_rack", Wall, 0.8, 1),
            ("hp:table", Floor, 0.5, 1),
            ("hp:warband_supply_chest", Wall, 0.4, 0),
            ("hp:barrel", Wall, 0.5, 0),
        ],
        Role::Throne | Role::Boss => vec![
            ("hp:warchief_throne", Centre, 0.0, 1),
            ("hp:orc_war_banner", Wall, 0.6, 2),
            ("hp:skull_pile", Wall, 0.3, 0),
        ],
        Role::Treasure => vec![
            (
                if hp {
                    "hp:bonehook_chest"
                } else {
                    "ph:treasure_chest"
                },
                Wall,
                1.0,
                2,
            ),
            ("hp:turtleshell_bronze_chest", Centre, 0.0, 1),
            (
                if hp { "hp:barrel" } else { "ph:wine_barrel_01" },
                Wall,
                0.4,
                0,
            ),
        ],
        Role::Storage => vec![
            (
                if hp { "hp:barrel" } else { "ph:wine_barrel_01" },
                Wall,
                1.6,
                2,
            ),
            (
                if hp { "hp:crate" } else { "ph:wooden_crate_02" },
                Wall,
                1.6,
                2,
            ),
            ("hp:crate_open", Floor, 0.5, 0),
        ],
        Role::Lair => vec![
            ("hp:bone_pile", Floor, 1.2, 2),
            ("hp:bone_pile_skull", Floor, 0.8, 1),
            ("hp:beast_skulls", Wall, 0.4, 0),
            ("hp:giant_ribcage", Centre, 0.0, 0),
        ],
        Role::Ritual => vec![
            ("hp:skullbanner_ritual_rug", Centre, 0.0, 1),
            ("hp:candles", Wall, 1.0, 2),
            ("hp:skullbanner_skull_shrine", Wall, 0.05, 0),
        ],
        Role::Arena => vec![
            ("hp:weapon_rack", Wall, 0.5, 2),
            ("hp:orc_war_banner", Wall, 0.4, 2),
        ],
        Role::Entrance => vec![
            (
                if hp { "hp:barrel" } else { "ph:wine_barrel_01" },
                Wall,
                0.4,
                0,
            ),
            (
                if hp { "hp:crate" } else { "ph:wooden_crate_01" },
                Wall,
                0.3,
                0,
            ),
        ],
        Role::None => vec![
            (
                if hp { "hp:barrel" } else { "ph:wine_barrel_01" },
                Wall,
                0.3,
                0,
            ),
            ("hp:bone_pile", Wall, 0.2, 0),
            (
                if hp { "hp:crate" } else { "ph:wooden_crate_02" },
                Wall,
                0.3,
                0,
            ),
        ],
    };
    v.retain(|p| p.2 > 0.0 || p.3 > 0 || p.1 == Spot::Centre);
    v
}

/// A placed piece: (asset, position, heading).
struct Put {
    asset: String,
    at: Vec3,
    yaw: f32,
}

fn plan_room(
    c: &Compiled,
    r: &CRoom,
    role: Role,
    hp: bool,
    lib: &crate::objects::Library,
    rng: &mut Rng,
    taken: &mut Vec<(Vec2, f32)>,
) -> Vec<Put> {
    let mut out = Vec::new();
    let area = std::f32::consts::PI * r.inr * r.inr * 1.4;
    let perim = r
        .shape
        .outline()
        .windows(2)
        .map(|w| w[0].distance(w[1]))
        .sum::<f32>()
        .max(4.0);
    let free = |p: Vec2, rad: f32, taken: &[(Vec2, f32)]| {
        taken.iter().all(|(q, qr)| q.distance(p) > rad + qr + 0.3)
    };
    let floor_at = |p: Vec2| c.floor_under(Vec3::new(p.x, r.floor_at(p) + 1.4, p.y), 2.5);
    let room_air = |p: Vec2, rad: f32, h: f32| -> Option<f32> {
        let f = floor_at(p)?;
        // the whole footprint stands in this room's air, on its floor
        for k in 0..8 {
            let q = p + Vec2::from_angle(k as f32 * std::f32::consts::FRAC_PI_4) * rad;
            if !c.air(Vec3::new(q.x, f + 0.3, q.y))
                || !c.air(Vec3::new(q.x, f + h.min(2.5), q.y))
                || (floor_at(q)? - f).abs() > 0.25
            {
                return None;
            }
        }
        Some(f)
    };
    for (asset, spot, rate, min) in pieces(role, hp) {
        let Some(e) = lib.get(asset) else { continue };
        let rad = e.radius.max(0.3);
        let want = match spot {
            Spot::Centre => min.max(1) as usize,
            Spot::Wall => ((perim / 10.0 * rate).round() as u32).max(min) as usize,
            Spot::Floor => ((area / 100.0 * rate).round() as u32).max(min) as usize,
        }
        .min(24);
        let mut placed = 0;
        let mut tries = 0;
        while placed < want && tries < want * 30 + 30 {
            tries += 1;
            let (p, yaw) = match spot {
                Spot::Centre => {
                    // the dais's top, else the middle (a bit off for the next one)
                    let c0 = r.shape.centre()
                        + if placed == 0 {
                            Vec2::ZERO
                        } else {
                            Vec2::new(rng.range(-3.0, 3.0), rng.range(-3.0, 3.0))
                        };
                    let (ax, hl, _) = r.shape.axes();
                    let p = if role == Role::Boss || role == Role::Throne {
                        c0 + ax * hl * 0.45
                    } else {
                        c0
                    };
                    let to = r.shape.centre() - p;
                    (
                        p,
                        if to.length() > 0.1 {
                            to.x.atan2(to.y)
                        } else {
                            std::f32::consts::PI
                        },
                    )
                }
                Spot::Wall => {
                    let lp = super::geom::resample_loop(&r.shape.outline(), 0.5);
                    let k = rng.below(lp.len() as u32) as usize;
                    let (a, b) = (lp[k], lp[(k + 1) % lp.len()]);
                    let m = (a + b) * 0.5;
                    let n = (b - a).perp().normalize_or_zero();
                    let inward = if r.inside(m + n * 0.2) > r.inside(m - n * 0.2) {
                        n
                    } else {
                        -n
                    };
                    let p = m + inward * (rad + 0.25);
                    // a whole wall behind it (not a doorway)
                    let f = r.floor_at(p);
                    if c.air(Vec3::new(
                        m.x - inward.x * 0.3,
                        f + 1.0,
                        m.y - inward.y * 0.3,
                    )) {
                        continue;
                    }
                    (p, inward.x.atan2(inward.y))
                }
                Spot::Floor => {
                    let (lo, hi) = (r.lo, r.hi);
                    let p = Vec2::new(rng.range(lo.x, hi.x), rng.range(lo.y, hi.y));
                    if r.inside(p) < rad + 1.2 {
                        continue;
                    }
                    (p, rng.range(0.0, std::f32::consts::TAU))
                }
            };
            if !free(p, rad, taken) {
                continue;
            }
            let Some(f) = room_air(p, rad * 0.8, e.height) else {
                continue;
            };
            taken.push((p, rad));
            out.push(Put {
                asset: e.id.clone(),
                at: Vec3::new(p.x, f, p.y),
                yaw,
            });
            placed += 1;
        }
    }
    out
}

/// Dresses every room (replacing what Dress put there before); one undo step.
pub fn dress(d: &mut WorldData, dw: &DungeonWorld) -> Result<Option<String>, String> {
    dress_rooms(d, dw, None)
}

/// Dress, or just room `only` ("furnish this room" while walking): its earlier Dress pieces go,
/// new ones by what it's for come in; the rest of the dungeon stays as it is.
pub fn dress_rooms(
    d: &mut WorldData,
    dw: &DungeonWorld,
    only: Option<u32>,
) -> Result<Option<String>, String> {
    let Some(m) = d.dungeon_map.0.clone() else {
        return Err("No dungeon's map is open".into());
    };
    let Some(c) = dw.compiled.clone() else {
        return Err("Lay the dungeon out first".into());
    };
    let Some(lib) = d.lib.as_deref() else {
        return Err("The object library isn't ready".into());
    };
    let mut rng = Rng::new(m.plan.seed as u64 ^ 0xd2e5);
    // keep away from the light fixtures and from where players arrive
    let fixtures: std::collections::HashSet<u64> =
        m.lights.iter().filter_map(|l| l.fixture).collect();
    let mut taken: Vec<(Vec2, f32)> = m
        .lights
        .iter()
        .map(|l| (Vec2::new(l.pos[0], l.pos[2]), 0.6))
        .collect();
    if let Some(ps) = &d.gameplay.player_start {
        taken.push((Vec2::new(ps.pos[0], ps.pos[2]), 3.5));
    }
    let mut puts = Vec::new();
    for r in &c.rooms {
        if !r.kind.is_space()
            || r.kind.is_pit()
            || matches!(
                r.kind,
                RoomKind::Corridor | RoomKind::Stairs | RoomKind::Pool
            )
            || only.is_some_and(|o| o != r.id)
        {
            continue;
        }
        let style = super::styles::get(r.style.as_deref().unwrap_or(&m.style));
        let role = m.plan.room(r.id).map(|x| x.role).unwrap_or(Role::None);
        puts.extend(plan_room(
            &c,
            r,
            role,
            style.pack == "handpainted",
            lib,
            &mut rng,
            &mut taken,
        ));
    }
    let before = Arc::new(m.clone());
    d.history.begin_group(&d.store);
    let mut out = m.clone();
    // Dress's own pieces from before go (not the light fixtures, not anything placed by hand;
    // with `only`, just those in that room)
    let in_room = |id: &u64| match (only, d.store.items.get(id)) {
        (None, _) => true,
        (Some(o), Some(p)) => c.room(o).is_some_and(|r| {
            r.inside(Vec2::new(p.pos[0], p.pos[2])) > -0.3
                && (p.pos[1] - r.base).abs() < r.height.max(3.0)
        }),
        (Some(_), None) => false,
    };
    let gone: Vec<u64> = m
        .props
        .iter()
        .filter(|p| !fixtures.contains(p) && in_room(p))
        .copied()
        .collect();
    for id in &gone {
        if d.store.items.contains_key(id) {
            d.history.touch_object(&d.store, *id);
            d.store.remove(*id);
        }
    }
    out.props.retain(|p| !gone.contains(p));
    let n = puts.len();
    for p in puts {
        let Some(e) = d.lib.as_deref().and_then(|l| l.get(&p.asset)) else {
            continue;
        };
        let oid = d.store.new_id();
        d.history.touch_object(&d.store, oid);
        let mut o = crate::objects::Placed {
            id: oid,
            asset: e.id.clone(),
            variant: 0,
            pos: p.at.to_array(),
            rot: Quat::from_rotation_y(p.yaw).to_array(),
            scale: 1.0,
            stretch: None,
            group: 0,
            locked: false,
        };
        o.set_scale3(Vec3::splat(e.default_scale));
        d.store.insert(o);
        out.props.push(oid);
    }
    d.dungeon_map.0 = Some(out.clone());
    let label = if only.is_some() {
        "Furnish room"
    } else {
        "Dress rooms"
    };
    d.history
        .push_ext(label, super::edit::UNDO_MAP, before, Arc::new(out));
    d.history.end_group(&d.store, label);
    d.meta.dirty = true;
    Ok(Some(if only.is_some() {
        format!("{n} pieces set in the room")
    } else {
        format!(
            "{n} pieces set in the rooms by what each is for (ordinary objects: move or delete any)"
        )
    }))
}
