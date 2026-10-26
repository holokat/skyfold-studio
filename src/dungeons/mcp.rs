//! MCP tools for dungeons: the same edits as the Dungeons tab and the builder (`edit`), one undo
//! step each. In the world: create dungeons and their ways in, open one. In a dungeon's map: lay
//! it out, add and change rooms and levels, style, mood, lights, dressing, the view; then go back.

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use super::edit::{self, NewDungeon, NewEntrance};
use super::lights::{LightDef, LightKind};
use super::model::*;
use super::plan::{Ceiling, Liquid, Pillars, Role, Room, RoomKind, Shape};
use super::presets::{self, Size};
use super::render::{DungeonView, DungeonWorld};
use super::session::Session;
use super::{mood, styles};
use crate::io::WorldData;
use crate::mcp_tools::{
    McpAppExt, McpContent, McpTool, arg, bearing_dir, bearing_schema, dir_bearing, opt_arg,
    position_arg, position_schema,
};

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn obj(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut m = Map::new();
    for (k, v) in props {
        m.insert(k.into(), v);
    }
    json!({ "type": "object", "properties": m, "required": required })
}

fn heading_of(bearing: f32) -> f32 {
    let v = bearing_dir(bearing);
    v.x.atan2(v.y)
}

fn bearing_of(yaw: f32) -> f32 {
    dir_bearing(Vec2::new(yaw.sin(), yaw.cos()))
}

fn enum_arg<T: Copy>(
    args: &Value,
    key: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    match args.get(key).and_then(|v| v.as_str()) {
        None => Ok(None),
        Some(s) => parse(s)
            .map(Some)
            .ok_or_else(|| format!("unknown {key} `{s}`")),
    }
}

pub fn dungeon_json(d: &DungeonDef) -> Value {
    let mut v = json!({
        "id": d.id,
        "name": d.name,
        "kind": d.kind.id(),
        "layout": d.layout,
        "style": d.style,
        "size": d.size.id(),
        "room_size": d.room_size.id(),
        "irregularity": d.irregularity,
        "art": d.art.id(),
        "instancing": d.instancing.id(),
        "levels": d.levels,
        "players": d.players,
        "map_folder": format!("dungeons/{}", d.folder),
        "ways_in": d.entrances.iter().map(|e| json!({
            "id": e.id,
            "kind": e.kind.id(),
            "position": [r1(e.pos[0]), r1(e.pos[1]), r1(e.pos[2])],
            "facing_deg": r1(bearing_of(e.yaw)),
            "object": e.object,
            "cave": e.cave,
            "color": e.color.id(),
            "label": e.label,
        })).collect::<Vec<_>>(),
    });
    // a realm's land (made when it is first opened)
    if d.kind == MapKind::Realm {
        v["shape"] = json!(d.shape.map(|s| s.id()).unwrap_or("classic"));
        v["realm_size"] = json!(d.realm_size.id());
        v["seed"] = json!(d.seed);
        if let Some(c) = &d.compound {
            v["compound"] = c.json();
        }
    }
    v
}

fn room_json(plan: &super::plan::Plan, r: &Room) -> Value {
    let (lo, hi) = r.shape.bounds();
    json!({
        "id": r.id,
        "name": r.label(),
        "kind": r.kind.id(),
        "level": r.level,
        "floor_y": r1(plan.level_y(r.level) + r.floor),
        "shape": r.shape,
        "bounds": [[r1(lo.x), r1(lo.y)], [r1(hi.x), r1(hi.y)]],
        "height": r1(r.height),
        "floor": r1(r.floor),
        "ceiling": r.ceiling.id(),
        "pillars": r.pillars.id(),
        "role": r.role.id(),
        "style": r.style,
        "locked": r.locked,
        "to_level": r.to_level,
        "depth": r1(r.depth),
        "liquid": r.liquid.id(),
    })
}

/// Prompt an area in a dungeon (`prompt::tools::selection_info`): what the selected outline
/// covers on the open dungeon — the rooms touching it (all levels, the edited one first), the
/// lights, pieces and decals inside, the style, mood and levels.
pub(crate) fn selection_json(m: &MapDoc, poly: &[Vec2], level: i32) -> Value {
    let inside = |p: Vec2| crate::biomes::point_in_polygon(p, poly);
    let (lo, hi) = poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(a, b), p| (a.min(*p), b.max(*p)),
    );
    // a room touches the selection when a point of its outline or its middle is inside, or a
    // corner of the selection is inside it
    let touches = |r: &Room| {
        let o = r.shape.outline();
        inside(r.shape.centre())
            || o.iter().any(|p| inside(*p))
            || poly.iter().any(|p| r.shape.sdf(*p) > 0.0)
    };
    let mut rooms: Vec<&Room> = m.plan.rooms.iter().filter(|r| touches(r)).collect();
    rooms.sort_by_key(|r| (r.level != level, r.level, r.id));
    let p3 = |p: [f32; 3]| inside(Vec2::new(p[0], p[2]));
    json!({
        "dungeon": m.name,
        "kind": "underground dungeon",
        "editing_level": level,
        "levels": m.plan.levels.iter().map(|l| json!({ "id": l.id, "y": r1(l.y), "name": l.name })).collect::<Vec<_>>(),
        "outline": poly.iter().map(|p| [r1(p.x), r1(p.y)]).collect::<Vec<_>>(),
        "bounds": [[r1(lo.x), r1(lo.y)], [r1(hi.x), r1(hi.y)]],
        "size_m": [r1(hi.x - lo.x), r1(hi.y - lo.y)],
        "style": m.style,
        "art": format!("{:?}", m.art),
        "mood": m.mood.preset,
        "rooms": rooms.iter().map(|r| room_json(&m.plan, r)).collect::<Vec<_>>(),
        "rooms_elsewhere": m.plan.rooms.len() - rooms.len(),
        "lights": m.lights.iter().filter(|l| p3(l.pos)).map(light_json).collect::<Vec<_>>(),
        "pieces": m.pieces.iter().filter(|p| p3(p.pos)).map(piece_json).collect::<Vec<_>>(),
        "decals": m.decals.iter().filter(|x| p3(x.pos)).count(),
        "note": "coordinates in metres: x east, z south, y up; a room's floor_y is absolute. Rooms join where they overlap. Change only these rooms (and add new ones inside the outline)",
    })
}

fn light_json(l: &LightDef) -> Value {
    json!({
        "id": l.id, "kind": l.kind.id(), "position": [r1(l.pos[0]), r1(l.pos[1]), r1(l.pos[2])],
        "color": l.color, "lumens": l.lumens.round(), "range": r1(l.range), "cone_deg": r1(l.cone), "flicker": l.flicker,
        "heading_deg": r1(bearing_of(l.dir[0])), "tilt_deg": r1(l.dir[1].to_degrees()), "shadows": l.shadows, "haze": l.haze, "auto": l.auto, "fixture": l.fixture, "room": l.room,
    })
}

fn state_json(d: &WorldData, dw: &DungeonWorld, view: &DungeonView) -> Value {
    match d.dungeon_map.0.as_ref() {
        None => {
            json!({ "open": "world", "world": d.meta.name, "dungeons": d.dungeons.list.iter().map(dungeon_json).collect::<Vec<_>>() })
        }
        Some(m) => {
            let p = &m.plan;
            json!({
                "open": "dungeon",
                "id": m.id,
                "name": m.name,
                "kind": m.kind.id(),
                "art": m.art.id(),
                "style": m.style,
                "mood": m.mood.preset,
                "levels": p.levels.iter().map(|l| json!({ "id": l.id, "y": l.y, "name": l.name, "rooms": p.rooms.iter().filter(|r| r.level == l.id).count() })).collect::<Vec<_>>(),
                "rooms": p.rooms.len(),
                "rooms_by_kind": RoomKind::ALL.iter().map(|k| (k.id().to_string(), json!(p.rooms.iter().filter(|r| r.kind == *k).count()))).filter(|(_, n)| n.as_u64() != Some(0)).collect::<Map<_, _>>(),
                "bounds": p.bounds2().map(|(a, b)| json!([[r1(a.x), r1(a.y)], [r1(b.x), r1(b.y)]])),
                "lights": m.lights.len(),
                "dressing_objects": m.props.len(),
                "player_start": d.gameplay.player_start.map(|s| s.pos),
                "way_out": way_out_json(m, d),
                "built": { "triangles": dw.triangles, "busy": dw.busy(), "doorways": dw.doorways.len() },
                "view": { "level": view.level, "cut": view.cut },
            })
        }
    }
}

fn way_out_at(m: &MapDoc, d: &WorldData) -> Option<(Vec3, f32)> {
    super::portal::exit_at(m, &d.store, d.gameplay.player_start.as_ref())
}

fn way_out_json(m: &MapDoc, d: &WorldData) -> Value {
    let f = super::portal::exit_frame(m);
    let at = way_out_at(m, d);
    json!({
        "frame": if f.is_empty() { Value::Null } else { json!(f) },
        "frame_name": edit::frame(&f).map_or("Liquid only", |x| x.name),
        "color": super::portal::exit_color(m).id(),
        "frame_is_way_ins": m.way_out.frame.is_none(),
        "color_is_way_ins": m.way_out.color.is_none(),
        "way_in": m.way_out.way_in.as_ref().map(|(f, c)| json!({ "frame": f, "color": c.id() })),
        "position": at.map(|(p, _)| [r1(p.x), r1(p.y), r1(p.z)]),
        "facing_deg": at.map(|(_, y)| r1(bearing_of(y))),
        "put_by_hand": m.way_out.at.is_some(),
        "object": m.exit,
    })
}

pub(super) fn entrance_arg(args: &Value, d: &WorldData) -> Result<NewEntrance, String> {
    let kind = enum_arg(args, "kind", EntranceKind::from_id)?.unwrap_or(EntranceKind::Portal);
    let pos = position_arg(args, "position")?;
    let object: Option<u64> = opt_arg(args, "object")?;
    let cave: Option<u32> = opt_arg(args, "cave")?;
    let at = match (pos, kind) {
        (Some((xz, y)), _) => Vec3::new(
            xz.x,
            y.unwrap_or_else(|| d.terrain.height_at(xz.x, xz.y)),
            xz.y,
        ),
        (None, EntranceKind::Object | EntranceKind::Cave) => Vec3::ZERO,
        (None, _) => return Err("give `position` [x, z] for a portal or a spot".into()),
    };
    let yaw = match opt_arg::<f32>(args, "facing_deg")? {
        Some(b) => heading_of(b),
        None => {
            // facing the camera
            let to = d.orbit.eye() - at;
            to.x.atan2(to.z)
        }
    };
    let color = enum_arg(args, "color", PortalColor::from_id)?.unwrap_or_default();
    let frame: Option<String> = opt_arg(args, "frame")?;
    let prop: Option<String> = opt_arg(args, "asset")?;
    for a in frame.iter().chain(prop.iter()).filter(|a| !a.is_empty()) {
        crate::market::check(a)?;
    }
    if let Some(a) = &prop {
        if kind != EntranceKind::Object {
            return Err(
                "`asset` is for kind object (a ready-made way in put down at `position`)".into(),
            );
        }
        if !edit::ENTRANCE_PROPS.contains(&a.as_str()) {
            return Err(format!(
                "`asset` is one of {}",
                edit::ENTRANCE_PROPS.join(", ")
            ));
        }
        if pos.is_none() {
            return Err("give `position` [x, z] for where the way in is put down".into());
        }
    }
    let label: Option<String> = opt_arg(args, "label")?;
    Ok(NewEntrance {
        kind,
        at,
        yaw,
        object,
        cave,
        color,
        frame,
        prop,
        label: label.unwrap_or_default(),
    })
}

pub(super) fn entrance_schema() -> Vec<(&'static str, Value)> {
    vec![
        (
            "kind",
            json!({ "type": "string", "enum": EntranceKind::ALL.map(|k| k.id()), "description": "portal (a liquid portal in a stone frame standing at `position`; an ordinary object afterwards), object (an existing object `object` is the way in — a cellar door, a well — or a ready-made one, `asset`, put down at `position`), point (an invisible spot at `position`), cave (the end of cave `cave`: a portal fills its deepest chamber's doorway)" }),
        ),
        (
            "position",
            position_schema("where the portal or spot stands"),
        ),
        (
            "facing_deg",
            bearing_schema(
                "which way a portal faces (players walk in against it; default: towards the camera)",
            ),
        ),
        (
            "object",
            json!({ "type": "integer", "description": "object id (kind object)" }),
        ),
        (
            "asset",
            json!({ "type": "string", "enum": edit::ENTRANCE_PROPS, "description": "kind object: a ready-made way in to put down at `position` (cellar door, trapdoor, old well) instead of an existing object" }),
        ),
        (
            "cave",
            json!({ "type": "integer", "description": "cave id (kind cave)" }),
        ),
        (
            "color",
            json!({ "type": "string", "enum": PortalColor::ALL.map(|c| c.id()) }),
        ),
        (
            "frame",
            json!({ "type": "string", "enum": edit::FRAMES.map(|f| f.asset), "description": "a portal's style: its frame model (stone arch, ruined arch, rune ring, temple gate, dwarven door, crystal spires; default: the first the library has)" }),
        ),
        ("label", json!({ "type": "string" })),
    ]
}

fn shape_arg(v: &Value) -> Result<Shape, String> {
    let ty = v
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or("shape needs a `type`: rect, round, octagon, outline or path")?;
    let pt = |k: &str| -> Result<Vec2, String> {
        let a: [f32; 2] = serde_json::from_value(
            v.get(k)
                .cloned()
                .ok_or_else(|| format!("shape needs `{k}` [x, z]"))?,
        )
        .map_err(|e| format!("`{k}`: {e}"))?;
        Ok(Vec2::from_array(a))
    };
    let num = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    let pts = |k: &str| -> Result<Vec<Vec2>, String> {
        let a: Vec<[f32; 2]> = serde_json::from_value(
            v.get(k)
                .cloned()
                .ok_or_else(|| format!("shape needs `{k}` [[x, z], …]"))?,
        )
        .map_err(|e| format!("`{k}`: {e}"))?;
        Ok(a.into_iter().map(Vec2::from_array).collect())
    };
    Ok(match ty {
        "rect" => {
            let c = pt("center")?;
            let size: [f32; 2] = serde_json::from_value(
                v.get("size")
                    .cloned()
                    .ok_or("rect needs `size` [width (x), length (z)]")?,
            )
            .map_err(|e| e.to_string())?;
            Shape::Rect {
                c: c.to_array(),
                half: [size[0].max(1.0) * 0.5, size[1].max(1.0) * 0.5],
                yaw: -num("turn_deg").unwrap_or(0.0).to_radians(),
            }
        }
        "round" => Shape::Round {
            c: pt("center")?.to_array(),
            r: num("radius").ok_or("round needs `radius`")?.max(0.8),
        },
        "octagon" | "ngon" => Shape::Ngon {
            c: pt("center")?.to_array(),
            r: num("radius").ok_or("octagon needs `radius`")?.max(0.8),
            sides: num("sides").map(|s| s as u8).unwrap_or(8).clamp(3, 24),
            yaw: -num("turn_deg").unwrap_or(0.0).to_radians(),
        },
        "outline" | "poly" => {
            let p = pts("points")?;
            if p.len() < 3 {
                return Err("an outline needs 3 points or more".into());
            }
            Shape::Poly {
                pts: p.iter().map(|p| p.to_array()).collect(),
            }
        }
        "path" => {
            let p = pts("points")?;
            if p.len() < 2 {
                return Err("a path needs 2 points or more".into());
            }
            Shape::path(&p, num("width").unwrap_or(4.0).max(1.0))
        }
        t => return Err(format!("unknown shape type `{t}`")),
    })
}

fn room_schema(with_shape_required: bool) -> Vec<(&'static str, Value)> {
    let _ = with_shape_required;
    vec![
        (
            "kind",
            json!({ "type": "string", "enum": RoomKind::ALL.map(|k| k.id()), "description": "room, hall (tall, pillared), corridor (use a path shape), cavern (natural rock: lumpy floor, domed roof), stairs (a path from its level to `to_level`), pit (a shaft down `depth` m or to the level below; railings round it), pool (sunken basin of `liquid`), dais (raised platform with steps, `height` = rise, inside a room), bridge (walkway across a pit or chasm, a path), balcony (a gallery deck inside a taller room at `floor` m above its level — or on the level above with floor 0 — railings and posts where it overlooks a drop; dungeons_gallery_hall makes a whole galleried hall), chasm (a deep rift of rough rock `depth` m down, optional `liquid` at the bottom; a path shape works well), spiral (a round tower stair, round shape 2.4–6 m radius, winding from its level to `to_level` round a central column; its top landing faces the nearest room there)" }),
        ),
        (
            "level",
            json!({ "type": "integer", "description": "level id (0 = where players arrive; −1 below, 1 above; new levels are made 14 m apart)" }),
        ),
        (
            "shape",
            json!({ "type": "object", "description": "{type: rect, center: [x, z], size: [w, l], turn_deg} | {type: round, center, radius} | {type: octagon, center, radius, sides} | {type: outline, points: [[x, z], …]} | {type: path, points: [[x, z], …], width}" }),
        ),
        ("name", json!({ "type": "string" })),
        (
            "height",
            json!({ "type": "number", "description": "clear height to the top of the ceiling (m); a dais's rise" }),
        ),
        (
            "floor",
            json!({ "type": "number", "description": "floor above (+) or below (−) its level (m); a pool's depth (−)" }),
        ),
        (
            "irregular",
            json!({ "type": "boolean", "description": "rooms and halls drawn as a rect, polygon or circle: uneven walls (alcoves, jogs, bulges) or back to regular" }),
        ),
        (
            "reshape",
            json!({ "type": "boolean", "description": "a new uneven outline for an irregular room" }),
        ),
        (
            "unevenness",
            json!({ "type": "number", "minimum": 0.1, "maximum": 1.0, "description": "how far an irregular room's walls reach out" }),
        ),
        (
            "ceiling",
            json!({ "type": "string", "enum": Ceiling::ALL.map(|c| c.id()) }),
        ),
        (
            "pillars",
            json!({ "type": "string", "enum": Pillars::ALL.map(|p| p.id()) }),
        ),
        (
            "role",
            json!({ "type": "string", "enum": Role::ALL.map(|r| r.id()), "description": "what it's for: Dress furnishes it so (crypt, prison, library, forge, boss…)" }),
        ),
        (
            "style",
            json!({ "type": ["string", "null"], "description": "a style of its own (dungeons_catalog styles), null for the dungeon's" }),
        ),
        (
            "to_level",
            json!({ "type": "integer", "description": "stairs: the level they lead to" }),
        ),
        (
            "smooth",
            json!({ "type": "boolean", "description": "stairs: a smooth ramp instead of steps" }),
        ),
        (
            "depth",
            json!({ "type": "number", "description": "pit: how far it drops (m)" }),
        ),
        (
            "liquid",
            json!({ "type": "string", "enum": Liquid::ALL.map(|l| l.id()) }),
        ),
        (
            "locked",
            json!({ "type": "boolean", "description": "kept when the layout is shuffled" }),
        ),
    ]
}

fn apply_room_args(r: &mut Room, args: &Value) -> Result<(), String> {
    if let Some(k) = enum_arg(args, "kind", RoomKind::from_id)? {
        if k != r.kind {
            r.height = k.default_height();
            if k == RoomKind::Cavern {
                r.ceiling = Ceiling::Rock;
            }
            if k == RoomKind::Stairs && r.to_level.is_none() {
                r.to_level = Some(r.level - 1);
            }
            if k == RoomKind::Spiral && r.to_level.is_none() {
                r.to_level = Some(r.level + 1);
            }
        }
        r.kind = k;
    }
    if let Some(l) = opt_arg::<i32>(args, "level")? {
        r.level = l;
    }
    if let Some(s) = args.get("shape") {
        r.irregular = None;
        r.shape = shape_arg(s)?;
    }
    let amount = opt_arg::<f32>(args, "unevenness")?.map(|v| v.clamp(0.1, 1.0));
    let (seed, cur) = r
        .irregular
        .as_ref()
        .map_or((fastrand::u32(..), 0.6), |i| (i.seed, i.amount));
    match opt_arg::<bool>(args, "irregular")? {
        Some(false) => r.set_regular(),
        Some(true) => r.set_irregular(seed, amount.unwrap_or(cur)),
        None if amount.is_some() && r.irregular.is_some() => {
            r.set_irregular(seed, amount.unwrap_or(cur))
        }
        None => {}
    }
    if opt_arg::<bool>(args, "reshape")?.unwrap_or(false) && r.irregular.is_some() {
        r.set_irregular(fastrand::u32(..), amount.unwrap_or(cur));
    }
    if let Some(n) = opt_arg::<String>(args, "name")? {
        r.name = n;
    }
    if let Some(h) = opt_arg::<f32>(args, "height")? {
        r.height = h.clamp(0.3, 120.0);
    }
    if let Some(f) = opt_arg::<f32>(args, "floor")? {
        r.floor = f.clamp(-20.0, 20.0);
    }
    if let Some(c) = enum_arg(args, "ceiling", Ceiling::from_id)? {
        r.ceiling = c;
    }
    if let Some(p) = enum_arg(args, "pillars", Pillars::from_id)? {
        r.pillars = p;
    }
    if let Some(x) = enum_arg(args, "role", Role::from_id)? {
        r.role = x;
    }
    match args.get("style") {
        Some(Value::Null) => r.style = None,
        Some(Value::String(s)) => {
            if !styles::all().iter().any(|st| st.id == s) {
                return Err(format!("unknown style `{s}`"));
            }
            r.style = Some(s.clone());
        }
        _ => {}
    }
    if let Some(t) = opt_arg::<i32>(args, "to_level")? {
        r.to_level = Some(t);
    }
    if let Some(v) = opt_arg::<bool>(args, "smooth")? {
        r.smooth = v;
    }
    if let Some(dp) = opt_arg::<f32>(args, "depth")? {
        r.depth = dp.clamp(1.0, 200.0);
    }
    if let Some(l) = enum_arg(args, "liquid", Liquid::from_id)? {
        r.liquid = l;
    }
    if let Some(l) = opt_arg::<bool>(args, "locked")? {
        r.locked = l;
    }
    Ok(())
}

fn light_schema() -> Vec<(&'static str, Value)> {
    vec![
        (
            "kind",
            json!({ "type": "string", "enum": LightKind::ALL.map(|k| k.id()), "description": "torch, brazier, hanging_brazier, chandelier, candles, lantern, crystal (each with its own colour, strength, reach and flicker), work_lamp and shaft (spots), glow and spot (plain)" }),
        ),
        (
            "position",
            json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "[x, y, z]: where the light is (the flame)" }),
        ),
        (
            "color",
            json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "srgb 0..1" }),
        ),
        (
            "lumens",
            json!({ "type": "number", "description": "strength (a torch ~260 000, a brazier ~650 000, a shaft of light ~3 500 000)" }),
        ),
        (
            "range",
            json!({ "type": "number", "description": "reach (m)" }),
        ),
        (
            "flicker",
            json!({ "type": "number", "minimum": 0, "maximum": 1 }),
        ),
        (
            "cone_deg",
            json!({ "type": "number", "description": "a spot's cone (0: all round)" }),
        ),
        ("heading_deg", bearing_schema("which way a spot points")),
        (
            "tilt_deg",
            json!({ "type": "number", "description": "how far a spot points down (90: straight down)" }),
        ),
        ("shadows", json!({ "type": "boolean" })),
        (
            "haze",
            json!({ "type": "boolean", "description": "glows in the mood's haze (dungeons_mood haze): a halo round it, or a visible beam for a spot or shaft" }),
        ),
    ]
}

fn apply_light_args(l: &mut LightDef, args: &Value) -> Result<(), String> {
    if let Some(p) = opt_arg::<[f32; 3]>(args, "position")? {
        l.pos = p;
    }
    if let Some(c) = opt_arg::<[f32; 3]>(args, "color")? {
        l.color = c.map(|v| v.clamp(0.0, 1.0));
    }
    if let Some(v) = opt_arg::<f32>(args, "lumens")? {
        l.lumens = v.clamp(0.0, 50_000_000.0);
    }
    if let Some(v) = opt_arg::<f32>(args, "range")? {
        l.range = v.clamp(0.5, 200.0);
    }
    if let Some(v) = opt_arg::<f32>(args, "flicker")? {
        l.flicker = v.clamp(0.0, 1.0);
    }
    if let Some(v) = opt_arg::<f32>(args, "cone_deg")? {
        l.cone = v.clamp(0.0, 170.0);
    }
    if let Some(v) = opt_arg::<f32>(args, "heading_deg")? {
        l.dir[0] = heading_of(v);
    }
    if let Some(v) = opt_arg::<f32>(args, "tilt_deg")? {
        l.dir[1] = v.clamp(-90.0, 90.0).to_radians();
    }
    if let Some(v) = opt_arg::<bool>(args, "shadows")? {
        l.shadows = v;
    }
    if let Some(v) = opt_arg::<bool>(args, "haze")? {
        l.haze = v;
    }
    l.auto = false;
    Ok(())
}

macro_rules! sys {
    ($w:expr, $f:expr, $a:expr) => {
        $w.run_system_cached_with($f, $a)
            .map_err(|e| format!("{e}"))?
    };
}

pub(super) fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "dungeons_catalog",
        description: "What dungeons can be made of: one-click layouts (underground) and realms (open-sky dungeons), styles, moods, room kinds, ceilings, pillars, roles, liquids, light kinds, portal colours and frames.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |_, _| {
            Ok(vec![McpContent::Json(json!({
                "layouts": presets::all().iter().map(|p| json!({ "id": p.id, "name": p.name, "about": p.blurb, "style": p.style, "mood": p.mood })).collect::<Vec<_>>(),
                "realms": presets::realms().iter().map(|r| { let (sh, sz) = super::realm_shapes::defaults(r.id); json!({ "id": r.id, "name": r.name, "about": r.blurb, "shape": sh.id(), "realm_size": sz.id() }) }).collect::<Vec<_>>(),
                "realm_shapes": super::realm_shapes::catalog_json().0,
                "compound_themes": super::compound::Theme::ALL.map(|t| json!({ "id": t.id(), "name": t.name(), "about": t.hint() })),
                "compound_orders": super::compound::Order::ALL.map(|o| json!({ "id": o.id(), "name": o.name() })),
                "realm_sizes": super::realm_shapes::catalog_json().1,
                "sizes": Size::ALL.map(|s| s.id()),
                "styles": styles::all().iter().map(|s| json!({ "id": s.id, "name": s.name, "art": s.art().id(), "ready": s.art().ready(), "about": s.blurb })).collect::<Vec<_>>(),
                "arts": styles::Art::ALL.map(|a| json!({ "id": a.id(), "name": a.name(), "ready": a.ready() })),
                "moods": mood::PRESETS.iter().map(|(id, n, b)| json!({ "id": id, "name": n, "about": b })).collect::<Vec<_>>(),
                "room_kinds": RoomKind::ALL.map(|k| k.id()),
                "ceilings": Ceiling::ALL.map(|c| c.id()),
                "pillars": Pillars::ALL.map(|p| p.id()),
                "roles": Role::ALL.map(|r| r.id()),
                "liquids": Liquid::ALL.map(|l| l.id()),
                "light_kinds": LightKind::ALL.map(|k| k.id()),
                "portal_colors": PortalColor::ALL.map(|c| c.id()),
                "portal_frames": edit::FRAMES.iter().map(|f| json!({ "id": f.asset, "name": f.name })).collect::<Vec<_>>(),
            }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_state",
        description: "What is open: the world (its dungeons and their ways in) or a dungeon's map (its levels, rooms, lights, style, mood, how far its building is).",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_state, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_create",
        description: "Create a dungeon in the world, with its way in, in one call — like Dungeons › Create a dungeon. kind interior: an underground map laid out from `layout` (dungeons_catalog) in `style` and `mood`; kind realm: a world of its own under a sky (`layout` = a realm), its land from `shape` (round isle, long isle, crescent bay, twin isles, island chain, coastal shelf, enclosed valley, highland plateau, lakeland, canyon, glade), `realm_size` (cozy ½ km … large 4 km) and `seed` (dungeons_realm_preview shows the land first; each realm has its own default shape and size). Its map is made when it is first opened (dungeons_open). One undo step.",
        input_schema: || {
            let mut p = vec![
                ("name", json!({ "type": "string" })),
                ("kind", json!({ "type": "string", "enum": MapKind::ALL.map(|k| k.id()) })),
                ("layout", json!({ "type": "string", "description": "a layout id (interior) or realm id (realm)" })),
                ("size", json!({ "type": "string", "enum": Size::ALL.map(|s| s.id()) })),
                ("room_size", json!({ "type": "string", "enum": super::presets::RoomSize::ALL.map(|s| s.id()), "description": "how big the rooms are: normal, large (×1.35 across) or extra_large (×1.7); `size` is how many rooms" })),
                ("irregularity", json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "description": "irregular rooms: 0 off (default), a little: barely uneven, 1: deep alcoves and many jogs (most rooms and halls; pillared halls stay regular)" })),
                ("art", json!({ "type": "string", "enum": ["handpainted", "photoreal"], "description": "the art it is made in (default: the world's art style where that is ready; photoreal is coming soon, so hand-painted for now); its map is painted in it" })),
                ("style", json!({ "type": "string" })),
                ("mood", json!({ "type": "string" })),
                ("instancing", json!({ "type": "string", "enum": Instancing::ALL.map(|i| i.id()) })),
                ("shape", json!({ "type": "string", "enum": super::realm_shapes::RealmShape::ALL.map(|s| s.id()), "description": "realm only: the land's shape (dungeons_catalog realm_shapes); default: the realm's own" })),
                ("realm_size", json!({ "type": "string", "enum": super::realm_shapes::RealmSize::ALL.map(|s| s.id()), "description": "realm only: cozy ½ km, small 1 km, medium 2 km, large 4 km across; default: the realm's own" })),
                ("seed", json!({ "type": "integer", "minimum": 0, "description": "realm only: the roll of its land (dungeons_realm_preview shows it first); default: from its id" })),
                ("compound", super::compound::options_schema()),
                ("sheet", json!({ "type": "string", "description": "build the realm from a design sheet: a library sheet's id (dungeons_sheets_library) or a custom one of this world's (dungeons_sheet_generate / dungeons_sheet_import). Implies kind realm; its land, paths, stairs, water and markers come from the sheet when dungeons_open first makes the map (dungeons_sheet_status reports the build). `name` defaults to the sheet's." })),
            ];
            p.push(("way_in", obj(entrance_schema(), &[])));
            obj(p, &[])
        },
        run: |w, a| sys!(w, mcp_create, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_set",
        description: "Change a dungeon's name, instancing (shared / party / raid), level range [from, to], players or notes. While a dungeon's map is open: renames that dungeon (`name` only; `id` may be left out). One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" })), ("name", json!({ "type": "string" })), ("instancing", json!({ "type": "string", "enum": Instancing::ALL.map(|i| i.id()) })), ("levels", json!({ "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 })), ("players", json!({ "type": "integer" })), ("notes", json!({ "type": "string" }))], &[]),
        run: |w, a| sys!(w, mcp_set, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_remove",
        description: "Remove a dungeon and its portals (its map's folder is kept aside in dungeons/_removed). One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" }))], &["id"]),
        run: |w, a| sys!(w, mcp_remove, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_entrance_add",
        description: "Add a way into dungeon `id`: a portal anywhere, an object, a spot, or the end of a cave. One undo step.",
        input_schema: || {
            let mut p = entrance_schema();
            p.insert(0, ("id", json!({ "type": "integer" })));
            obj(p, &["id", "kind"])
        },
        run: |w, a| sys!(w, mcp_entrance_add, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_entrance_remove",
        description: "Remove a way in (a portal's frame goes too; a linked object or cave stays). One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" })), ("entrance", json!({ "type": "integer" }))], &["id", "entrance"]),
        run: |w, a| sys!(w, mcp_entrance_remove, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_entrance_set",
        description: "Change a way in's portal colour, label or frame (`frame`: a portal's style, standing where it is now; \"none\" for the liquid alone). One undo step (a frame change is one of its own).",
        input_schema: || {
            obj(
                vec![
                    ("id", json!({ "type": "integer" })),
                    ("entrance", json!({ "type": "integer" })),
                    (
                        "color",
                        json!({ "type": "string", "enum": PortalColor::ALL.map(|c| c.id()) }),
                    ),
                    ("label", json!({ "type": "string" })),
                    (
                        "frame",
                        json!({ "type": "string", "enum": edit::FRAMES.map(|f| f.asset).into_iter().chain(["none"]).collect::<Vec<_>>(), "description": "a portal's style: its frame model (stone arch, ruined arch, rune ring, temple gate, dwarven door, crystal spires), or \"none\" for the liquid alone" }),
                    ),
                ],
                &["id", "entrance"],
            )
        },
        run: |w, a| sys!(w, mcp_entrance_set, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_open",
        description: "Open dungeon `id`'s map to build it: the world is saved first; the map is made the first time (laid out from its layout and then lit and dressed by Auto-light and Dress, or generated for a realm). Undo history starts fresh (like opening a world). dungeons_back returns.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" }))], &["id"]),
        run: |w, a| sys!(w, mcp_open, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_back",
        description: "Save the open dungeon's map and go back to its world.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_back, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_layout",
        description: "Lay the open dungeon out from a one-click layout (dungeons_catalog layouts): rooms, halls, corridors, caverns, stairs, pits, pools, daises, bridges and their roles. Locked rooms stay; auto lights and dressing are dropped (dungeons_lights_auto, dungeons_dress make them again). Same seed: same layout. One undo step.",
        input_schema: || obj(vec![("layout", json!({ "type": "string" })), ("size", json!({ "type": "string", "enum": Size::ALL.map(|s| s.id()) })), ("room_size", json!({ "type": "string", "enum": super::presets::RoomSize::ALL.map(|s| s.id()) })), ("irregularity", json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "description": "irregular rooms: 0 off, 1 very uneven" })), ("seed", json!({ "type": "integer", "minimum": 0 }))], &["layout"]),
        run: |w, a| sys!(w, mcp_layout, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_rooms",
        description: "The open dungeon's rooms (all, or of `level`): id, kind, level, floor height, shape, bounds, height, ceiling, pillars, role, style.",
        input_schema: || obj(vec![("level", json!({ "type": "integer" }))], &[]),
        run: |w, a| sys!(w, mcp_rooms, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_room_add",
        description: "Add a room to the open dungeon: a room, hall, corridor, cavern, stairs, pit, pool, dais or bridge on a level, with its shape. Rooms join where they overlap (doorways, wider halls); walls, floors, ceilings, pillars and trims are built for you in the dungeon's style. One undo step.",
        input_schema: || obj(room_schema(true), &["kind", "shape"]),
        run: |w, a| sys!(w, mcp_room_add, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_gallery_hall",
        description: "Add a galleried hall to the open dungeon in one step: a hall two storeys high on `level` (at least 18 × 24 m), a gallery round it at the floor of the level above (railings and an arcade of posts where it overlooks the hall), and a stair up to it. Rooms you add on the level above that reach the hall's walls open onto the gallery. One undo step; returns the rooms made.",
        input_schema: || obj(vec![
            ("level", json!({ "type": "integer" })),
            ("center", json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, z]" })),
            ("size", json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[width along x, length along z] (m)" })),
        ], &["center", "size"]),
        run: |w, a| sys!(w, mcp_gallery_hall, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_wall_opening",
        description: "Cut a window or an alcove into the wall of the open dungeon's room nearest `position` (within 3 m) on `level`: an arched window through to the room beyond the wall (or a blind one), or an arched recess. It's a small room of its own (change or remove it like any room). One undo step.",
        input_schema: || obj(vec![
            ("level", json!({ "type": "integer" })),
            ("position", json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, z] near the wall" })),
            ("kind", json!({ "type": "string", "enum": ["window", "alcove"] })),
        ], &["position", "kind"]),
        run: |w, a| sys!(w, mcp_wall_opening, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_room_set",
        description: "Change a room of the open dungeon (any field of dungeons_room_add). One undo step.",
        input_schema: || {
            let mut p = room_schema(false);
            p.insert(0, ("id", json!({ "type": "integer" })));
            obj(p, &["id"])
        },
        run: |w, a| sys!(w, mcp_room_set, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_room_remove",
        description: "Remove a room of the open dungeon. One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" }))], &["id"]),
        run: |w, a| sys!(w, mcp_room_remove, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_level",
        description: "Add a level to the open dungeon or change one: its floor height `y` (m) and name. One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" })), ("y", json!({ "type": "number" })), ("name", json!({ "type": "string" }))], &["id"]),
        run: |w, a| sys!(w, mcp_level, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_style",
        description: "The open dungeon's style (dungeons_catalog styles): what walls, floors, ceilings, trims and pillars are made of, and the ceilings rooms get (rooms with a style of their own keep it); and/or its weathering on top of the style's own (multipliers, 1 = the style's, 0 none): moss (patches where it's damp), grime (the foot of walls, corners, streaks), wet (stone by water), puddles (on floors). One undo step.",
        input_schema: || obj(vec![
            ("style", json!({ "type": "string" })),
            ("moss", json!({ "type": "number", "minimum": 0, "maximum": 2.5 })),
            ("grime", json!({ "type": "number", "minimum": 0, "maximum": 2.0 })),
            ("wet", json!({ "type": "number", "minimum": 0, "maximum": 2.5 })),
            ("puddles", json!({ "type": "number", "minimum": 0, "maximum": 2.5 })),
            ("rubble", json!({ "type": "number", "minimum": 0, "maximum": 3.0, "description": "broken stones at the foot of walls, in corners and round pillars" })),
        ], &[]),
        run: |w, a| sys!(w, mcp_style, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_lights_all",
        description: "Every light of the open dungeon at once (Lights › All lights), on top of each light's own values: brightness, reach and flicker multipliers (1 = each as set), and the colour all fires and lamps lean towards (tint, srgb) with tint_amount (0..1; the mood's light tint). Without arguments it reports them. One undo step.",
        input_schema: || {
            obj(
                vec![
                    ("brightness", json!({ "type": "number", "minimum": 0.1, "maximum": 4.0 })),
                    ("reach", json!({ "type": "number", "minimum": 0.3, "maximum": 3.0 })),
                    ("flicker", json!({ "type": "number", "minimum": 0.0, "maximum": 2.5 })),
                    ("tint", json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 })),
                    ("tint_amount", json!({ "type": "number", "minimum": 0.0, "maximum": 1.0 })),
                    ("reset", json!({ "type": "boolean", "description": "brightness, reach and flicker back to 1" })),
                ],
                &[],
            )
        },
        run: |w, a| sys!(w, mcp_lights_all, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_music",
        description: "The open dungeon's music: one track that loops while players are in it, over the mood's ambience (exported with it). Either a demo track that ships with the app (`demo`: its id, e.g. layout_crypt or mood_torchlit; `list` true lists them all; a new dungeon starts with the one for its layout) or your own file (`file`: absolute path of an MP3 or Ogg file, copied into the dungeon's folder, replacing an earlier one). Setting one replaces the other; `demo` \"none\" or `remove` true for no music. `volume` (0..1.5), `ambience_under` (0..1: how loud the mood's ambience stays under it). Without arguments it reports the track (and `playing_now`, what is heard). One undo step.",
        input_schema: || {
            obj(
                vec![
                    ("demo", json!({ "type": "string", "description": "a demo track's id (see `list`), or \"none\"" })),
                    ("file", json!({ "type": "string", "description": "absolute path of an .mp3 / .ogg" })),
                    ("volume", json!({ "type": "number", "minimum": 0.0, "maximum": 1.5 })),
                    ("ambience_under", json!({ "type": "number", "minimum": 0.0, "maximum": 1.0 })),
                    ("remove", json!({ "type": "boolean" })),
                    ("list", json!({ "type": "boolean", "description": "also list the demo tracks (id, name, the layout or mood it is for, seconds)" })),
                ],
                &[],
            )
        },
        run: |w, a| sys!(w, mcp_music, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_way_out",
        description: "The open dungeon's (or realm's) way back out: a portal that looks like the way in players came through (its frame and colour; else the dungeon's first portal) and stands behind where players arrive, until changed here. `frame`: a portal style (dungeons_catalog frames: id or name, e.g. hp:portal_ruin or \"Ruined arch\"), \"none\" for the liquid alone, \"match\" for the way in's. `color`: a portal colour id, or \"match\". `position` ([x, z] on the floor there, or [x, y, z]) and/or `facing_deg` (compass bearing it faces; players walk out against it) put it somewhere; `reset_position` true stands it behind where players arrive again. Its frame is an object of the map (move it like any object and the way out goes with it). Without arguments it reports the way out. One undo step.",
        input_schema: || {
            obj(
                vec![
                    ("frame", json!({ "type": "string", "description": "a frame id or name, \"none\" (liquid alone) or \"match\" (the way in's)" })),
                    ("color", json!({ "type": "string", "description": "arcane, void, fel, fire, holy, frost, blood, tide, or \"match\" (the way in's)" })),
                    ("position", position_schema("where its foot stands")),
                    ("facing_deg", bearing_schema("the way it faces")),
                    ("reset_position", json!({ "type": "boolean", "description": "stand it behind where players arrive again" })),
                ],
                &[],
            )
        },
        run: |w, a| sys!(w, mcp_way_out, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_mood",
        description: "The open dungeon's mood: a preset (dungeons_catalog moods) and/or any of fill (srgb), fill_strength (0..1.5: 0 pitch black away from lights), exposure (stops), fog (srgb), fog_density (0..1), haze, motes, tint (srgb the lights are pulled towards) and tint_amount, sound (ambience stem), volume. One undo step.",
        input_schema: || {
            obj(
                vec![
                    ("preset", json!({ "type": "string" })),
                    ("fill", json!({ "type": "array", "items": { "type": "number" } })),
                    ("fill_strength", json!({ "type": "number" })),
                    ("exposure", json!({ "type": "number" })),
                    ("fog", json!({ "type": "array", "items": { "type": "number" } })),
                    ("fog_density", json!({ "type": "number" })),
                    ("haze", json!({ "type": "number" })),
                    ("sky", json!({ "type": "string", "enum": super::model::SkyMode::ALL.map(|k| k.id()), "description": "over rooms with an open ceiling: none (darkness), day, dusk or night (a sky with the sun or moon shining in; walls and roofs cast shadows)" })),
                    ("motes", json!({ "type": "number" })),
                    ("tint", json!({ "type": "array", "items": { "type": "number" } })),
                    ("tint_amount", json!({ "type": "number" })),
                    ("sound", json!({ "type": "string" })),
                    ("volume", json!({ "type": "number" })),
                ],
                &[],
            )
        },
        run: |w, a| sys!(w, mcp_mood, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_lights_auto",
        description: "Auto-light the open dungeon: torches along walls (alternating in corridors), braziers round big rooms and boss daises, candles in crypts and shrines, chandeliers or hanging fires over halls, crystals in caverns — in the style's own fixtures (ordinary objects). Replaces earlier auto lights; lights placed by hand stay (`clear`: just remove the auto lights and their fixtures). `density` 0.4..2.5 (1 = normal). One undo step.",
        input_schema: || obj(vec![("clear", json!({ "type": "boolean" })), ("density", json!({ "type": "number" })), ("room", json!({ "type": "integer", "description": "just this room (its earlier auto lights are replaced; the rest stay)" }))], &[]),
        run: |w, a| sys!(w, mcp_lights_auto, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_decals_age",
        description: "Age the open dungeon's walls: cracks up the walls, damp drips and stains (in damp styles and moods), soot over torches and round braziers, blood in prisons, lairs, arenas and boss rooms, glowing rune circles on ritual, shrine and boss floors. Replaces earlier aged decals; ones placed by hand stay (`clear`: just remove the aged decals). `density` 0.3..3 (1 = the style's own). One undo step.",
        input_schema: || obj(vec![("clear", json!({ "type": "boolean" })), ("density", json!({ "type": "number" }))], &[]),
        run: |w, a| sys!(w, mcp_decals_age, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_decals",
        description: "The open dungeon's decals (cracks, stains, drips, blood, scorch, runes): where, facing, size, colour, glow.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_decals, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_decal_add",
        description: "Put a decal on the open dungeon's walls or floor: a crack, stain, drip, blood, scorch or rune circle at `position` [x, y, z] on a surface facing `normal` (default up: a floor), `size` m across, turned `spin_deg` about its facing, in `color` (sRGB) with `glow` (runes). One undo step.",
        input_schema: || obj(vec![
            ("kind", json!({ "type": "string", "enum": super::decals::DecalKind::ALL.map(|k| k.id()) })),
            ("position", json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 })),
            ("normal", json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 })),
            ("size", json!({ "type": "number" })),
            ("spin_deg", json!({ "type": "number" })),
            ("color", json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 })),
            ("glow", json!({ "type": "number" })),
        ], &["kind", "position"]),
        run: |w, a| sys!(w, mcp_decal_add, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_decal_remove",
        description: "Remove a decal of the open dungeon by `id`, or every aged one (`aged`: true). One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" })), ("aged", json!({ "type": "boolean" }))], &[]),
        run: |w, a| sys!(w, mcp_decal_remove, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_pieces",
        description: "The open dungeon's pieces (doors, double doors, portcullises, bars, secret doors, levers, pressure plates, spike traps, and markers for the game: boss arenas, checkpoints, loot spots, spawn spots): id, kind, position (foot), yaw, size, whether it starts open, what it works (`links`), label, and whether the template layout placed it (`auto`). Also the doorways a door or gate can go in.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_pieces, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_piece_add",
        description: "Add a piece to the open dungeon. Doors, double doors, portcullises, bars and secret doors go into the doorway nearest `position` (within 4 m; replacing a door already there) and take its size; a lever goes at `position` (the middle of its plate, on a wall) facing `yaw_deg` (0 = +z, away from the wall); the rest stand at `position` on the floor. `links`: what a lever or plate works (piece ids). Markers (arena, checkpoint, loot, spawn) are only data for the game — nothing to see. One undo step.",
        input_schema: || obj(piece_schema(), &["kind", "position"]),
        run: |w, a| sys!(w, mcp_piece_add, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_piece_set",
        description: "Change a piece: kind, position (a door or gate moves to the doorway nearest it), yaw, size, whether it starts open, links, label. It stops being a template piece. One undo step.",
        input_schema: || {
            let mut p = piece_schema();
            p.insert(0, ("id", json!({ "type": "integer" })));
            obj(p, &["id"])
        },
        run: |w, a| sys!(w, mcp_piece_set, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_piece_remove",
        description: "Remove a piece by `id`, or every piece the template layout placed (`auto`: true), or all of them (`all`: true). Links to it go too. One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" })), ("auto", json!({ "type": "boolean" })), ("all", json!({ "type": "boolean" }))], &[]),
        run: |w, a| sys!(w, mcp_piece_remove, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_pieces_auto",
        description: "Put the template layout's doors and gates in the open dungeon: a portcullis into the boss's room, bars into prison cells, stout doors into treasure rooms, crypts and libraries, a door in some other doorways. Replaces earlier template pieces; ones placed or changed by hand stay. One undo step.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_pieces_auto, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_piece_use",
        description: "Work a piece as the player would (open or shut a door or gate, pull a lever — which works what it's linked to — spring a trap). Changes only how it stands now, not the map; walking resets it.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" }))], &["id"]),
        run: |w, a| sys!(w, mcp_piece_use, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_minimap",
        description: "The open dungeon's minimaps: a map of each level drawn from the plan as the walls stand (ink walls on parchment floors, pillars, pits and chasms dark, water blue, stairs hatched, doors brown, gates and bars iron, secret doors purple, traps and plates as red rings, the game's markers: arena ring, checkpoint green, loot gold, spawn blue; the way out blue). Returns each as an image with where it lies (`origin` [x, z] of its top-left corner, `metres_per_pixel`: x east with the columns, z with the rows) and saves them as `minimap_L<level>.png` in the dungeon's folder (`save`: false to only look). `level`: just that one. The export carries them too.",
        input_schema: || obj(vec![("level", json!({ "type": "integer" })), ("save", json!({ "type": "boolean" }))], &[]),
        run: |w, a| sys!(w, mcp_minimap, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_walkable",
        description: "Walk check of the open dungeon: from where players arrive (or `from` [x, y, z]), which rooms a walker can reach — over floors and the tops of daises, decks, stairs and steps with headroom, up steps of at most 0.6 m, round pillars, rails and pieces that block; pools and pits are no floor. `doors`: `open` (default: doors, gates and secret doors are open, bars stay shut), `as_placed` (each as it starts) or `shut` (all shut: what a lever or key must open). Returns the share of the floor reached and every room with how much of it is reached; `unreachable` lists the rooms out of reach.",
        input_schema: || obj(vec![("from", json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 })), ("doors", json!({ "type": "string", "enum": ["open", "as_placed", "shut"] }))], &[]),
        run: |w, a| sys!(w, mcp_walkable, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_lights",
        description: "The open dungeon's light sources.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_lights, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_light_add",
        description: "Add a light source to the open dungeon (no fixture object: place one from the library if wanted). `fixture` true (brazier or candles), like Brazier here / Candles here: the style's own brazier or candles stand on the floor at `position` (the floor point) with the light over them; the other settings are then the kind's own (dungeons_light_set after). One undo step.",
        input_schema: || {
            let mut p = light_schema();
            p.push(("fixture", json!({ "type": "boolean", "description": "brazier / candles: stand the style's fixture object on the floor at `position` with its light (applied this frame)" })));
            obj(p, &["kind", "position"])
        },
        run: |w, a| sys!(w, mcp_light_add, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_light_set",
        description: "Change a light source (colour, strength, reach, flicker, cone, direction, position, shadows). It stops being an auto light. One undo step.",
        input_schema: || {
            let mut p = light_schema();
            p.insert(0, ("id", json!({ "type": "integer" })));
            obj(p, &["id"])
        },
        run: |w, a| sys!(w, mcp_light_set, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_light_remove",
        description: "Remove a light source (its fixture object stays). One undo step.",
        input_schema: || obj(vec![("id", json!({ "type": "integer" }))], &["id"]),
        run: |w, a| sys!(w, mcp_light_remove, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_dress",
        description: "Furnish every room of the open dungeon by its role (crypt, prison, library, forge, boss, treasure, lair, storage…) with library objects set against the walls or in the middle, clear of doorways and pillars; replaces what it placed before (`clear`: just remove it). One undo step.",
        input_schema: || obj(vec![("clear", json!({ "type": "boolean" })), ("room", json!({ "type": "integer", "description": "just furnish this room (its earlier Dress pieces are replaced; the rest stay)" }))], &[]),
        run: |w, a| sys!(w, mcp_dress, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_wall",
        description: "Do something to the open dungeon's wall nearest `position` [x, z] (within 3 m) on `level` (default 0), like Layout › On the walls and dragging walls: `action` door / secret_door (a door through to the room behind the wall — a short passage is cut when one is within 6 m — or a locked door set in the wall; the door goes in once the dungeon has rebuilt: dungeons_wait), torch / lantern (with its sconce or bracket, `height` m over the floor, default 2), push (the wall moves `distance` m out, or in when negative: the room grows or shrinks on that side), alcove (a vaulted alcove `distance` m deep pushed out of the wall). Windows and arched recesses: dungeons_wall_opening. One undo step.",
        input_schema: || {
            obj(
                vec![
                    ("action", json!({ "type": "string", "enum": ["door", "secret_door", "torch", "lantern", "push", "alcove"] })),
                    ("position", json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, z] at or near the wall" })),
                    ("level", json!({ "type": "integer" })),
                    ("distance", json!({ "type": "number", "description": "push: metres out (+) or in (−); alcove: its depth (default 1.5)" })),
                    ("height", json!({ "type": "number", "description": "torch / lantern: metres over the floor (default 2)" })),
                ],
                &["action", "position"],
            )
        },
        run: |w, a| sys!(w, mcp_wall, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_view",
        description: "The builder's view (not saved): the `level` being edited, `cut` (cut away everything above it to look in), `top` (look straight down), `focus` [x, y, z] (frame a spot).",
        input_schema: || obj(vec![("level", json!({ "type": "integer" })), ("cut", json!({ "type": "boolean" })), ("top", json!({ "type": "boolean" })), ("focus", json!({ "type": "array", "items": { "type": "number" } })), ("distance", json!({ "type": "number" }))], &[]),
        run: |w, a| sys!(w, mcp_view, a),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_wait",
        description: "Wait until the open dungeon's architecture has been built (after layout or room edits), up to `seconds` (default 20).",
        input_schema: || obj(vec![("seconds", json!({ "type": "number" }))], &[]),
        run: |w, a| {
            let secs: f32 = opt_arg(&a, "seconds")?.unwrap_or(20.0);
            let start = w.resource::<Time<Real>>().elapsed_secs();
            Ok(vec![McpContent::deferred(move |w| {
                let dw = w.resource::<DungeonWorld>();
                let now = w.resource::<Time<Real>>().elapsed_secs();
                if !dw.busy() || now - start > secs {
                    let busy = dw.busy();
                    Some(Ok(vec![McpContent::Json(json!({ "built": !busy, "triangles": dw.triangles, "ms": dw.build_ms }))]))
                } else {
                    None
                }
            })])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_probe",
        description: "Ask the open dungeon's space about points [[x, y, z], …]: in its air or not, the floor under and the roof over each.",
        input_schema: || json!({ "type": "object", "properties": { "points": { "type": "array", "minItems": 1, "maxItems": 200, "items": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 } } }, "required": ["points"] }),
        run: |w, a| {
            let pts: Vec<[f32; 3]> = arg(&a, "points")?;
            let Some(c) = w.resource::<DungeonWorld>().compiled.clone() else { return Err("no dungeon is open (or it has no rooms)".into()) };
            let out: Vec<Value> = pts
                .iter()
                .map(|p| {
                    let p = Vec3::from_array(*p);
                    json!({ "point": [p.x, p.y, p.z], "air": c.air(p), "floor": c.floor_under(p, 40.0).map(r1), "roof": c.roof_over(p, 80.0).map(r1) })
                })
                .collect();
            Ok(vec![McpContent::Json(json!({ "points": out }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_walk_in",
        description: "Start walk mode in the open dungeon where players arrive (the player start).",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| sys!(w, mcp_walk_in, a),
    });
}

fn json_out(v: Value) -> Result<Vec<McpContent>, String> {
    Ok(vec![McpContent::Json(v)])
}

fn mcp_state(
    In(_): In<Value>,
    d: WorldData,
    dw: Res<DungeonWorld>,
    view: Res<DungeonView>,
) -> Result<Vec<McpContent>, String> {
    json_out(state_json(&d, &dw, &view))
}

fn mcp_create(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    // a temple compound is a realm of its own layout
    let compound = match a.get("compound") {
        Some(v) if v.is_object() => Some(super::compound::Options::from_json(v)?.clamped()),
        _ => None,
    };
    let given: Option<String> = opt_arg(&a, "layout")?;
    if compound.is_some()
        && given
            .as_deref()
            .is_some_and(|l| l != super::compound::LAYOUT)
    {
        return Err(format!(
            "a `compound` is laid out in the realm `{}` (leave `layout` out)",
            super::compound::LAYOUT
        ));
    }
    let compound = compound
        .or_else(|| (given.as_deref() == Some(super::compound::LAYOUT)).then(Default::default));
    // a design sheet is a realm of its own layout
    let sheet = match opt_arg::<String>(&a, "sheet")?.filter(|s| !s.trim().is_empty()) {
        Some(id) => Some(super::sheets::mcp::sheet_ref(&d, id.trim())?),
        None => None,
    };
    if sheet.is_some() && compound.is_some() {
        return Err(
            "a dungeon is built from a `sheet` or laid out as a `compound`, not both".into(),
        );
    }
    let kind = if compound.is_some() || sheet.is_some() {
        MapKind::Realm
    } else {
        enum_arg(&a, "kind", MapKind::from_id)?.unwrap_or(MapKind::Interior)
    };
    let layout: String = if compound.is_some() {
        super::compound::LAYOUT.into()
    } else if sheet.is_some() {
        super::sheets::LAYOUT.into()
    } else {
        given.unwrap_or_else(|| {
            if kind == MapKind::Realm {
                presets::realms()[0].id.into()
            } else {
                "crypt".into()
            }
        })
    };
    if kind == MapKind::Interior && presets::get(&layout).is_none() {
        return Err(format!("unknown layout `{layout}` (dungeons_catalog)"));
    }
    if kind == MapKind::Realm && sheet.is_none() && presets::realm(&layout).is_none() {
        return Err(format!("unknown realm `{layout}` (dungeons_catalog)"));
    }
    let style: String = opt_arg(&a, "style")?.unwrap_or_default();
    // (default: the world's art style, where that art is ready to build with)
    let art = enum_arg(&a, "art", super::styles::Art::from_id)?.unwrap_or_else(|| {
        let world = if d.meta.art_style.painted() {
            super::styles::Art::HandPainted
        } else {
            super::styles::Art::Photoreal
        };
        if world.ready() {
            world
        } else {
            super::styles::Art::default()
        }
    });
    if !style.is_empty() {
        style_fits(&style, art)?;
    }
    let way_in = match a.get("way_in") {
        Some(v) if v.is_object() => Some(entrance_arg(v, &d)?),
        _ => None,
    };
    let sheet_name = sheet
        .as_ref()
        .map(|r| super::sheets::mcp::sheet_name(&d, r))
        .unwrap_or_default();
    let n = NewDungeon {
        name: opt_arg::<String>(&a, "name")?
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(sheet_name),
        kind,
        layout,
        size: enum_arg(&a, "size", Size::from_id)?.unwrap_or_default(),
        room_size: enum_arg(&a, "room_size", super::presets::RoomSize::from_id)?
            .unwrap_or_default(),
        irregularity: opt_arg::<f32>(&a, "irregularity")?
            .unwrap_or(0.0)
            .clamp(0.0, 1.0),
        art,
        style,
        mood: opt_arg(&a, "mood")?.unwrap_or_default(),
        instancing: enum_arg(&a, "instancing", |s| {
            Instancing::ALL.into_iter().find(|i| i.id() == s)
        })?
        .unwrap_or_default(),
        way_in,
        shape: enum_arg(&a, "shape", super::realm_shapes::RealmShape::from_id)?,
        realm_size: enum_arg(&a, "realm_size", super::realm_shapes::RealmSize::from_id)?.or_else(
            || {
                sheet
                    .as_ref()
                    .map(|r| super::sheets::mcp::realm_size_for(&d, r))
            },
        ),
        seed: opt_arg(&a, "seed")?,
        compound,
        sheet,
    };
    if n.compound.is_some() && n.shape.is_some_and(|s| !s.walled()) {
        return Err("a temple compound is laid out on walled land: shape enclosed_valley (the default), canyon or glade".into());
    }
    if kind != MapKind::Realm && (n.shape.is_some() || n.realm_size.is_some() || n.seed.is_some()) {
        return Err("`shape`, `realm_size` and `seed` are a realm's (kind realm)".into());
    }
    let id = edit::create(&mut d, &n)?;
    let note = if n.sheet.is_some() {
        "one undo step; dungeons_open makes its land from the sheet and builds it over a few seconds (dungeons_sheet_status with wait: true reports what was made)"
    } else if n.compound.is_some() {
        "one undo step; dungeons_open makes its land and builds the compound over a few seconds (dungeons_compound with wait: true reports what was made)"
    } else {
        "one undo step; dungeons_open builds its map"
    };
    json_out(json!({ "dungeon": dungeon_json(d.dungeons.get(id).unwrap()), "note": note }))
}

fn mcp_set(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    // in a dungeon's map: its name (the world's list takes it on the way back)
    if let Some(open) = d.dungeon_map.0.as_ref().map(|m| m.id) {
        if opt_arg::<u32>(&a, "id")?.is_some_and(|id| id != open) {
            return Err("only the open dungeon can be changed here (dungeons_back first)".into());
        }
        if ["instancing", "levels", "players", "notes"]
            .iter()
            .any(|k| a.get(*k).is_some())
        {
            return Err("in a dungeon's map only `name` can be changed (its instancing, levels, players and notes: from the world, dungeons_back first)".into());
        }
        let name: String = arg::<String>(&a, "name")?.trim().to_string();
        if name.is_empty() {
            return Err("give a `name`".into());
        }
        let n = name.clone();
        edit::map(&mut d, "Rename dungeon", move |m| {
            m.name = n;
            Ok(())
        })?;
        d.meta.name = name.clone();
        return json_out(json!({ "id": open, "name": name }));
    }
    let id: u32 = arg(&a, "id")?;
    let name: Option<String> = opt_arg(&a, "name")?;
    let inst = enum_arg(&a, "instancing", |s| {
        Instancing::ALL.into_iter().find(|i| i.id() == s)
    })?;
    let levels: Option<[u8; 2]> = opt_arg(&a, "levels")?;
    let players: Option<u8> = opt_arg(&a, "players")?;
    let notes: Option<String> = opt_arg(&a, "notes")?;
    edit::update_def(&mut d, id, "Change dungeon", |x| {
        if let Some(n) = name.filter(|n| !n.trim().is_empty()) {
            x.name = n;
        }
        if let Some(i) = inst {
            x.instancing = i;
        }
        if let Some(l) = levels {
            x.levels = [l[0].min(l[1]), l[0].max(l[1])];
        }
        if let Some(p) = players {
            x.players = p;
        }
        if let Some(n) = notes {
            x.notes = n;
        }
    })?;
    json_out(dungeon_json(d.dungeons.get(id).unwrap()))
}

fn mcp_remove(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    edit::remove(&mut d, id)?;
    json_out(json!({ "removed": id }))
}

fn mcp_entrance_add(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    let e = entrance_arg(&a, &d)?;
    let eid = edit::add_entrance(&mut d, id, &e)?;
    json_out(json!({ "entrance": eid, "dungeon": dungeon_json(d.dungeons.get(id).unwrap()) }))
}

fn mcp_entrance_remove(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let (id, eid): (u32, u32) = (arg(&a, "id")?, arg(&a, "entrance")?);
    edit::remove_entrance(&mut d, id, eid)?;
    json_out(dungeon_json(d.dungeons.get(id).unwrap()))
}

fn mcp_entrance_set(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let (id, eid): (u32, u32) = (arg(&a, "id")?, arg(&a, "entrance")?);
    let color = enum_arg(&a, "color", PortalColor::from_id)?;
    let label: Option<String> = opt_arg(&a, "label")?;
    let frame = match opt_arg::<String>(&a, "frame")?.as_deref().map(str::trim) {
        None => None,
        Some("none" | "") => Some(String::new()),
        Some(f) => {
            let f = edit::frame_named(f).ok_or_else(|| {
                let names: Vec<_> = edit::FRAMES.iter().map(|f| f.asset).collect();
                format!(
                    "unknown frame `{f}`; the frames are: {}, none",
                    names.join(", ")
                )
            })?;
            crate::market::check(f.asset)?;
            Some(f.asset.to_string())
        }
    };
    if color.is_some() || label.is_some() {
        edit::update_entrance(&mut d, id, eid, "Change way in", |e| {
            if let Some(c) = color {
                e.color = c;
            }
            if let Some(l) = label {
                e.label = l;
            }
        })?;
    }
    if let Some(f) = frame {
        edit::set_frame(&mut d, id, eid, &f)?;
    }
    json_out(dungeon_json(d.dungeons.get(id).unwrap()))
}

fn mcp_open(
    In(a): In<Value>,
    mut d: WorldData,
    mut s: ResMut<Session>,
    mut view: ResMut<DungeonView>,
    dw: Res<DungeonWorld>,
    mut walk: ResMut<crate::walk::WalkState>,
) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    super::session::enter(&mut d, &mut s, id).map_err(|e| {
        if e == super::session::NEEDS_SAVE {
            format!("{e} (world_save first)")
        } else {
            e
        }
    })?;
    if walk.active
        && let Some((p, yaw)) = super::session::dungeon_arrival(&d)
    {
        walk.teleport(p, Some(yaw));
    }
    view.level = 0;
    d.ed.tab = crate::editor::Tab::Dungeons;
    json_out(
        json!({ "opened": state_json(&d, &dw, &view), "note": "the architecture builds in the background (dungeons_wait); a new underground map is lit and dressed within a few frames" }),
    )
}

fn mcp_back(
    In(_): In<Value>,
    mut d: WorldData,
    mut s: ResMut<Session>,
    dw: Res<DungeonWorld>,
    view: Res<DungeonView>,
    mut walk: ResMut<crate::walk::WalkState>,
) -> Result<Vec<McpContent>, String> {
    let id = d.dungeon_map.0.as_ref().map(|m| m.id);
    super::session::back(&mut d, &mut s)?;
    if walk.active
        && let Some(id) = id
        && let Some((p, yaw)) =
            super::session::world_arrival(&d, id, s.via.filter(|v| v.0 == id).map(|v| v.1))
    {
        walk.teleport(p, Some(yaw));
    }
    json_out(state_json(&d, &dw, &view))
}

fn mcp_layout(
    In(a): In<Value>,
    mut d: WorldData,
    mut tool: ResMut<super::ui::DungeonTool>,
) -> Result<Vec<McpContent>, String> {
    let layout: String = arg(&a, "layout")?;
    let size = enum_arg(&a, "size", Size::from_id)?.unwrap_or_default();
    let room_size =
        enum_arg(&a, "room_size", super::presets::RoomSize::from_id)?.unwrap_or_default();
    let irregular: f32 = opt_arg::<f32>(&a, "irregularity")?
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let seed: Option<u32> = opt_arg(&a, "seed")?;
    let seed = seed.unwrap_or_else(|| d.dungeon_map.0.as_ref().map(|m| m.plan.seed).unwrap_or(1));
    tool.requests.push(super::ui::Request::Layout {
        layout,
        size,
        room_size,
        irregular,
        seed,
    });
    let _ = &mut d;
    json_out(
        json!({ "queued": "layout", "note": "applied this frame; dungeons_state / dungeons_wait" }),
    )
}

fn mcp_rooms(In(a): In<Value>, d: WorldData) -> Result<Vec<McpContent>, String> {
    let m = d
        .dungeon_map
        .0
        .as_ref()
        .ok_or("no dungeon is open (dungeons_open)")?;
    let level: Option<i32> = opt_arg(&a, "level")?;
    let rooms: Vec<Value> = m
        .plan
        .rooms
        .iter()
        .filter(|r| level.is_none_or(|l| r.level == l))
        .map(|r| room_json(&m.plan, r))
        .collect();
    json_out(json!({ "rooms": rooms }))
}

fn mcp_room_add(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let kind = enum_arg(&a, "kind", RoomKind::from_id)?.ok_or("give `kind`")?;
    let shape = shape_arg(a.get("shape").ok_or("give `shape`")?)?;
    let level: i32 = opt_arg(&a, "level")?.unwrap_or(0);
    room_style_fits(&a, &d)?;
    let mut r = Room::new(0, kind, level, shape);
    apply_room_args(&mut r, &a)?;
    let id = edit::add_room(&mut d, r)?;
    let m = d.dungeon_map.0.as_ref().unwrap();
    json_out(room_json(&m.plan, m.plan.room(id).unwrap()))
}

fn mcp_gallery_hall(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let level: i32 = opt_arg(&a, "level")?.unwrap_or(0);
    let c: [f32; 2] = arg(&a, "center")?;
    let size: [f32; 2] = arg(&a, "size")?;
    let made = edit::map(&mut d, "Add galleried hall", |m| {
        let before: std::collections::HashSet<u32> = m.plan.rooms.iter().map(|r| r.id).collect();
        let ceiling = super::styles::get(&m.style).ceiling_for(RoomKind::Hall);
        super::presets::gallery_hall(
            &mut m.plan,
            level,
            Vec2::from_array(c),
            size[0],
            size[1],
            ceiling,
        );
        Ok(m.plan
            .rooms
            .iter()
            .filter(|r| !before.contains(&r.id))
            .map(|r| r.id)
            .collect::<Vec<u32>>())
    })?;
    let m = d.dungeon_map.0.as_ref().unwrap();
    json_out(
        json!({ "rooms": made.iter().filter_map(|id| m.plan.room(*id)).map(|r| room_json(&m.plan, r)).collect::<Vec<_>>() }),
    )
}

fn mcp_wall_opening(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let level: i32 = opt_arg(&a, "level")?.unwrap_or(0);
    let p: [f32; 2] = arg(&a, "position")?;
    let kind: String = arg(&a, "kind")?;
    let window = match kind.as_str() {
        "window" => true,
        "alcove" => false,
        _ => return Err("`kind` is window or alcove".into()),
    };
    let r = {
        let m = d
            .dungeon_map
            .0
            .as_ref()
            .ok_or("No dungeon's map is open (dungeons_open)")?;
        super::presets::wall_opening(&m.plan, level, Vec2::from_array(p), window)
            .ok_or("no wall within 3 m of that position on that level")?
    };
    let id = edit::add_room(&mut d, r)?;
    let m = d.dungeon_map.0.as_ref().unwrap();
    json_out(room_json(&m.plan, m.plan.room(id).unwrap()))
}

fn mcp_room_set(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    room_style_fits(&a, &d)?;
    let mut err = None;
    edit::update_room(&mut d, id, "Change room", |r| {
        if let Err(e) = apply_room_args(r, &a) {
            err = Some(e);
        }
    })?;
    if let Some(e) = err {
        return Err(e);
    }
    let m = d.dungeon_map.0.as_ref().unwrap();
    json_out(room_json(&m.plan, m.plan.room(id).unwrap()))
}

fn mcp_room_remove(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    edit::remove_room(&mut d, id)?;
    json_out(json!({ "removed": id }))
}

fn mcp_level(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: i32 = arg(&a, "id")?;
    let y: Option<f32> = opt_arg(&a, "y")?;
    let name: Option<String> = opt_arg(&a, "name")?;
    edit::map(&mut d, "Level", |m| {
        m.plan.ensure_level(id);
        if let Some(l) = m.plan.levels.iter_mut().find(|l| l.id == id) {
            if let Some(y) = y {
                l.y = y;
            }
            if let Some(n) = name {
                l.name = n;
            }
        }
        Ok(())
    })?;
    let m = d.dungeon_map.0.as_ref().unwrap();
    json_out(json!({ "levels": m.plan.levels }))
}

/// A dungeon's styles are of its art (a hand-painted dungeon is built from hand-painted styles).
fn style_fits(style: &str, art: styles::Art) -> Result<(), String> {
    let s = styles::all()
        .iter()
        .find(|s| s.id == style)
        .ok_or_else(|| format!("unknown style `{style}` (dungeons_catalog)"))?;
    if s.art() != art {
        return Err(format!(
            "`{style}` is a {} style and this dungeon is {} (dungeons_catalog lists each style's art)",
            s.art().name().to_lowercase(),
            art.name().to_lowercase()
        ));
    }
    Ok(())
}

/// The open map's art.
fn map_art(d: &WorldData) -> Result<styles::Art, String> {
    d.dungeon_map
        .0
        .as_ref()
        .map(|m| m.art)
        .ok_or_else(|| "No dungeon's map is open (dungeons_open)".to_string())
}

/// A room's `style` argument fits the map's art.
fn room_style_fits(a: &Value, d: &WorldData) -> Result<(), String> {
    match a.get("style") {
        Some(Value::String(s)) => style_fits(s, map_art(d)?),
        _ => Ok(()),
    }
}

fn mcp_style(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let st: Option<String> = opt_arg(&a, "style")?;
    if let Some(st) = &st {
        style_fits(st, map_art(&d)?)?;
    }
    let num = |k: &str| -> Result<Option<f32>, String> { opt_arg(&a, k) };
    let (moss, grime, wet, puddles, rubble) = (
        num("moss")?,
        num("grime")?,
        num("wet")?,
        num("puddles")?,
        num("rubble")?,
    );
    let out = edit::map(&mut d, "Dungeon style", |m| {
        if let Some(st) = &st {
            m.style = st.clone();
            let style = styles::get(st);
            for r in &mut m.plan.rooms {
                if r.style.is_none() && r.kind != RoomKind::Cavern && r.ceiling != Ceiling::Open {
                    r.ceiling = style.ceiling_for(r.kind);
                }
            }
        }
        let w = &mut m.weathering;
        if let Some(v) = moss {
            w.moss = v.clamp(0.0, 2.5);
        }
        if let Some(v) = grime {
            w.grime = v.clamp(0.0, 2.0);
        }
        if let Some(v) = wet {
            w.wet = v.clamp(0.0, 2.5);
        }
        if let Some(v) = puddles {
            w.puddles = v.clamp(0.0, 2.5);
        }
        if let Some(v) = rubble {
            w.rubble = v.clamp(0.0, 3.0);
        }
        Ok(
            json!({ "style": m.style, "weathering": { "moss": w.moss, "grime": w.grime, "wet": w.wet, "puddles": w.puddles, "rubble": w.rubble } }),
        )
    })?;
    json_out(out)
}

fn mcp_mood(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let preset: Option<String> = opt_arg(&a, "preset")?;
    let base = match &preset {
        Some(p) => {
            Some(mood::preset(p).ok_or_else(|| format!("unknown mood `{p}` (dungeons_catalog)"))?)
        }
        None => None,
    };
    let f3 = |k: &str| -> Result<Option<[f32; 3]>, String> { opt_arg(&a, k) };
    let (fill, fog, tint) = (f3("fill")?, f3("fog")?, f3("tint")?);
    let num = |k: &str| -> Result<Option<f32>, String> { opt_arg(&a, k) };
    let (fs, ex, fd, hz, mt, ta, vol) = (
        num("fill_strength")?,
        num("exposure")?,
        num("fog_density")?,
        num("haze")?,
        num("motes")?,
        num("tint_amount")?,
        num("volume")?,
    );
    let sound: Option<String> = opt_arg(&a, "sound")?;
    let sky = enum_arg(&a, "sky", super::model::SkyMode::from_id)?;
    edit::map(&mut d, "Mood", |m| {
        if let Some(k) = sky {
            m.sky = k;
        }
        if let Some(b) = base {
            m.mood = b;
        }
        let md = &mut m.mood;
        if let Some(v) = fill {
            md.fill = v;
        }
        if let Some(v) = fog {
            md.fog = v;
        }
        if let Some(v) = tint {
            md.tint = v;
        }
        if let Some(v) = fs {
            md.fill_strength = v.clamp(0.0, 1.5);
        }
        if let Some(v) = ex {
            md.exposure = v.clamp(-4.0, 4.0);
        }
        if let Some(v) = fd {
            md.fog_density = v.clamp(0.0, 1.0);
        }
        if let Some(v) = hz {
            md.haze = v.clamp(0.0, 1.0);
        }
        if let Some(v) = mt {
            md.motes = v.clamp(0.0, 1.0);
        }
        if let Some(v) = ta {
            md.tint_amount = v.clamp(0.0, 1.0);
        }
        if let Some(v) = vol {
            md.volume = v.clamp(0.0, 2.0);
        }
        if let Some(s) = sound {
            md.sound = s;
        }
        Ok(())
    })?;
    json_out(serde_json::to_value(&d.dungeon_map.0.as_ref().unwrap().mood).unwrap_or_default())
}

fn mcp_music(
    In(a): In<Value>,
    mut d: WorldData,
    sound: Option<Res<super::sound::DungeonSound>>,
) -> Result<Vec<McpContent>, String> {
    let file: Option<String> = opt_arg(&a, "file")?;
    let demo: Option<String> = opt_arg(&a, "demo")?;
    let volume: Option<f32> = opt_arg(&a, "volume")?;
    let under: Option<f32> = opt_arg(&a, "ambience_under")?;
    let remove: bool = opt_arg(&a, "remove")?.unwrap_or(false);
    let list: bool = opt_arg(&a, "list")?.unwrap_or(false);
    let demos = super::sound::demo_tracks();
    // "none" (or "") for no music; else a demo track's id
    let demo = match demo.as_deref().map(str::trim) {
        Some("none" | "") => Some(String::new()),
        Some(id) => match super::sound::demo_track(id) {
            Some(t) => Some(t.id.clone()),
            None => {
                return Err(format!(
                    "no demo track `{id}`; the demo tracks are: {}",
                    demos
                        .iter()
                        .map(|t| t.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        },
        None => None,
    };
    let old = d
        .dungeon_map
        .0
        .as_ref()
        .ok_or("no dungeon is open")?
        .music
        .file
        .clone();
    let name = match file {
        Some(f) => {
            let src = std::path::PathBuf::from(&f);
            if !src.is_absolute() {
                return Err("`file` must be an absolute path".into());
            }
            let folder = d
                .meta
                .path
                .clone()
                .ok_or("the world isn't saved: save it first")?;
            Some(super::sound::import_music(&folder, &src, &old)?)
        }
        None => None,
    };
    // one track at a time: none, a demo track or the dungeon's own file
    edit::map(&mut d, "Music", |m| {
        if remove {
            m.music.file.clear();
            m.music.demo.clear();
        }
        if let Some(id) = demo {
            m.music.file.clear();
            m.music.demo = id;
        }
        if let Some(n) = name {
            m.music.file = n;
            m.music.demo.clear();
        }
        if let Some(v) = volume {
            m.music.volume = v.clamp(0.0, 1.5);
        }
        if let Some(v) = under {
            m.music.hush = 1.0 - v.clamp(0.0, 1.0);
        }
        Ok(())
    })?;
    let mu = &d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?.music;
    let track = super::sound::demo_track(&mu.demo).filter(|_| mu.file.is_empty());
    let mut out = json!({
        "file": if mu.file.is_empty() { Value::Null } else { json!(mu.file) },
        "demo": track.map(|t| json!(t.id)).unwrap_or(Value::Null),
        "name": if !mu.file.is_empty() { json!(format!("Your own: {}", mu.file)) } else { track.map(|t| json!(t.name)).unwrap_or(json!("None")) },
        "volume": mu.volume,
        "ambience_under": 1.0 - mu.hush,
        // what the dungeon's sound plays right now (a change shows from the next frame on)
        "playing_now": sound.as_ref().and_then(|ds| ds.playing.iter().find(|p| p.starts_with("music ")).cloned()),
        "note": "plays while walking the dungeon (and in the editor with Audio › preview)",
    });
    if list {
        out["demo_tracks"] = json!(demos.iter().map(|t| json!({ "id": t.id, "name": t.name, "for": format!("{} {}", t.kind, t.suits), "seconds": t.seconds })).collect::<Vec<_>>());
    }
    json_out(out)
}

fn mcp_wall(
    In(a): In<Value>,
    d: WorldData,
    mut tool: ResMut<super::ui::DungeonTool>,
    world: Res<DungeonWorld>,
) -> Result<Vec<McpContent>, String> {
    use super::ui::Request;
    let m = d
        .dungeon_map
        .0
        .as_ref()
        .filter(|m| m.kind == MapKind::Interior)
        .ok_or("no underground dungeon is open")?;
    let action: String = arg(&a, "action")?;
    let p: [f32; 2] = arg(&a, "position")?;
    let level: i32 = opt_arg(&a, "level")?.unwrap_or(0);
    let (id, at, out) = super::presets::nearest_wall(&m.plan, level, Vec2::from_array(p), 3.0)
        .ok_or("no wall within 3 m of that position on that level")?;
    let r = m.plan.room(id).ok_or("no such room")?;
    let floor = m.plan.level_y(level) + r.floor;
    let dist: Option<f32> = opt_arg(&a, "distance")?;
    let what = match action.as_str() {
        "door" | "secret_door" => {
            let kind = if action == "door" {
                super::pieces::PieceKind::Door
            } else {
                super::pieces::PieceKind::SecretDoor
            };
            let (room, door) =
                super::presets::door_opening(&m.plan, id, at, out).ok_or("no wall there")?;
            tool.requests.push(Request::Map(
                format!("Cut a doorway for a {}", kind.name().to_lowercase()),
                Box::new(move |m| {
                    m.plan.add(room);
                }),
            ));
            tool.pending_doors.push((kind, door, world.want));
            "a doorway is cut; the door goes in once the dungeon has rebuilt (dungeons_wait)"
        }
        "torch" | "lantern" => {
            let kind = if action == "torch" {
                LightKind::Torch
            } else {
                LightKind::Lantern
            };
            let h: f32 = opt_arg(&a, "height")?.unwrap_or(2.0);
            tool.requests.push(Request::WallFixture {
                kind,
                at: Vec3::new(at.x, floor + h, at.y),
                normal: Vec3::new(-out.x, 0.0, -out.y),
            });
            "hung on the wall"
        }
        "push" => {
            let d = dist.ok_or("`distance` is required for push")?;
            let shape = super::viewport::push_wall(
                &r.irregular
                    .as_ref()
                    .map_or_else(|| r.shape.clone(), |i| i.base.clone()),
                at,
                out,
                d,
            );
            tool.requests.push(Request::Map(
                "Move wall".into(),
                Box::new(move |m| {
                    if let Some(x) = m.plan.room_mut(id) {
                        x.set_shape(shape);
                    }
                }),
            ));
            "the wall moved"
        }
        "alcove" => {
            let room =
                super::viewport::alcove_room(r, at, out, dist.unwrap_or(1.5).clamp(0.4, 8.0));
            tool.requests.push(Request::Map(
                "Push out an alcove".into(),
                Box::new(move |m| {
                    m.plan.add(room);
                }),
            ));
            "an alcove was pushed out"
        }
        o => {
            return Err(format!(
                "unknown action `{o}`: door, secret_door, torch, lantern, push, alcove"
            ));
        }
    };
    json_out(
        json!({ "room": id, "wall_point": [r1(at.x), r1(at.y)], "outward": [r1(out.x), r1(out.y)], "done": what, "note": "applied this frame" }),
    )
}

fn mcp_lights_all(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let num = |k: &str| -> Result<Option<f32>, String> { opt_arg(&a, k) };
    let (b, r, f, ta) = (
        num("brightness")?,
        num("reach")?,
        num("flicker")?,
        num("tint_amount")?,
    );
    let tint: Option<[f32; 3]> = opt_arg(&a, "tint")?;
    let reset: bool = opt_arg(&a, "reset")?.unwrap_or(false);
    edit::map(&mut d, "All lights", |m| {
        if reset {
            m.all_lights = Default::default();
        }
        let al = &mut m.all_lights;
        if let Some(v) = b {
            al.brightness = v.clamp(0.1, 4.0);
        }
        if let Some(v) = r {
            al.reach = v.clamp(0.3, 3.0);
        }
        if let Some(v) = f {
            al.flicker = v.clamp(0.0, 2.5);
        }
        if let Some(v) = tint {
            m.mood.tint = v;
        }
        if let Some(v) = ta {
            m.mood.tint_amount = v.clamp(0.0, 1.0);
        }
        Ok(())
    })?;
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    json_out(
        json!({ "brightness": m.all_lights.brightness, "reach": m.all_lights.reach, "flicker": m.all_lights.flicker, "tint": m.mood.tint, "tint_amount": m.mood.tint_amount, "lights": m.lights.len() }),
    )
}

fn mcp_lights_auto(
    In(a): In<Value>,
    mut tool: ResMut<super::ui::DungeonTool>,
) -> Result<Vec<McpContent>, String> {
    let clear: bool = opt_arg(&a, "clear")?.unwrap_or(false);
    let density: f32 = opt_arg(&a, "density")?.unwrap_or(1.0);
    let room: Option<u32> = opt_arg(&a, "room")?;
    tool.requests.push(match (clear, room) {
        (true, _) => super::ui::Request::ClearAutoLights,
        (false, Some(id)) => super::ui::Request::AutoLightRoom(id),
        (false, None) => super::ui::Request::AutoLight {
            density: density.clamp(0.3, 3.0),
        },
    });
    json_out(
        json!({ "queued": if clear { "clear auto lights" } else { "auto-light" }, "note": "applied this frame; dungeons_lights lists them" }),
    )
}

fn mcp_decals_age(
    In(a): In<Value>,
    mut tool: ResMut<super::ui::DungeonTool>,
) -> Result<Vec<McpContent>, String> {
    let clear: bool = opt_arg(&a, "clear")?.unwrap_or(false);
    let density: f32 = opt_arg(&a, "density")?.unwrap_or(1.0);
    tool.requests.push(if clear {
        super::ui::Request::ClearAged
    } else {
        super::ui::Request::AgeWalls {
            density: density.clamp(0.3, 3.0),
        }
    });
    json_out(
        json!({ "queued": if clear { "clear aging" } else { "age the walls" }, "note": "applied this frame; dungeons_decals lists them" }),
    )
}

fn decal_json(x: &super::decals::Decal) -> Value {
    json!({ "id": x.id, "kind": x.kind.id(), "position": x.pos, "normal": x.normal, "size": x.size, "spin_deg": x.spin.to_degrees(), "color": x.color, "glow": x.glow, "aged": x.auto })
}

fn mcp_decals(In(_): In<Value>, d: WorldData) -> Result<Vec<McpContent>, String> {
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    json_out(json!({ "decals": m.decals.iter().map(decal_json).collect::<Vec<_>>() }))
}

fn mcp_decal_add(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let kind = enum_arg(&a, "kind", super::decals::DecalKind::from_id)?.ok_or("give `kind`")?;
    let pos: [f32; 3] = arg(&a, "position")?;
    let normal: [f32; 3] = opt_arg(&a, "normal")?.unwrap_or([0.0, 1.0, 0.0]);
    let size: f32 = opt_arg(&a, "size")?.unwrap_or(1.6);
    let mut x = super::decals::Decal::new(
        kind,
        Vec3::from_array(pos),
        Vec3::from_array(normal),
        size.clamp(0.1, 20.0),
    );
    if let Some(s) = opt_arg::<f32>(&a, "spin_deg")? {
        x.spin = s.to_radians();
    }
    if let Some(c) = opt_arg::<[f32; 3]>(&a, "color")? {
        x.color = c.map(|v| v.clamp(0.0, 1.0));
    }
    if let Some(g) = opt_arg::<f32>(&a, "glow")? {
        x.glow = g.clamp(0.0, 20.0);
    }
    let out = edit::map(
        &mut d,
        &format!("Add {}", kind.name().to_lowercase()),
        |m| {
            x.id = m.alloc_decal();
            m.decals.push(x.clone());
            Ok(decal_json(&x))
        },
    )?;
    json_out(out)
}

fn mcp_decal_remove(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: Option<u32> = opt_arg(&a, "id")?;
    let aged: bool = opt_arg(&a, "aged")?.unwrap_or(false);
    if id.is_none() && !aged {
        return Err("give `id`, or `aged`: true".into());
    }
    let n = edit::map(&mut d, "Remove decals", |m| {
        let before = m.decals.len();
        m.decals.retain(|x| !(Some(x.id) == id || (aged && x.auto)));
        Ok(before - m.decals.len())
    })?;
    json_out(json!({ "removed": n }))
}

fn piece_schema() -> Vec<(&'static str, Value)> {
    vec![
        (
            "kind",
            json!({ "type": "string", "enum": super::pieces::PieceKind::ALL.map(|k| k.id()) }),
        ),
        (
            "position",
            json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 }),
        ),
        ("yaw_deg", json!({ "type": "number" })),
        (
            "width",
            json!({ "type": "number", "description": "m (an arena: across)" }),
        ),
        ("height", json!({ "type": "number" })),
        (
            "open",
            json!({ "type": "boolean", "description": "starts open (a door, gate, secret door), pulled (a lever), sprung (a trap)" }),
        ),
        (
            "links",
            json!({ "type": "array", "items": { "type": "integer" }, "description": "a lever's or plate's: the pieces it works" }),
        ),
        (
            "label",
            json!({ "type": "string", "description": "for the game: a key's name, an event, a loot table" }),
        ),
    ]
}

fn piece_json(p: &super::pieces::Piece) -> Value {
    json!({ "id": p.id, "kind": p.kind.id(), "position": p.pos, "yaw_deg": p.yaw.to_degrees(), "width": p.width, "height": p.height, "open": p.open, "links": p.links, "label": p.label, "auto": p.auto })
}

/// Size, state, links and label from the args (`ids`: the map's pieces, for checking links).
fn apply_piece_args(p: &mut super::pieces::Piece, a: &Value, ids: &[u32]) -> Result<(), String> {
    if let Some(v) = opt_arg::<f32>(a, "yaw_deg")? {
        p.yaw = v.to_radians();
    }
    if let Some(v) = opt_arg::<f32>(a, "width")? {
        p.width = v.clamp(0.3, 40.0);
    }
    if let Some(v) = opt_arg::<f32>(a, "height")? {
        p.height = v.clamp(0.02, 12.0);
    }
    if let Some(v) = opt_arg::<bool>(a, "open")? {
        p.open = v;
    }
    if let Some(v) = opt_arg::<Vec<u32>>(a, "links")? {
        if let Some(bad) = v.iter().find(|l| !ids.contains(l) || **l == p.id) {
            return Err(format!("no piece {bad} to link to"));
        }
        p.links = v;
    }
    if let Some(v) = opt_arg::<String>(a, "label")? {
        p.label = v;
    }
    Ok(())
}

fn mcp_pieces(In(_): In<Value>, d: WorldData) -> Result<Vec<McpContent>, String> {
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    let mut seen: Vec<Vec2> = Vec::new();
    let mut doorways = Vec::new();
    for w in super::pieces::doorways_of(m) {
        let mid = (w.a + w.b) * 0.5;
        if seen.iter().any(|q| q.distance(mid) < 1.0) {
            continue;
        }
        seen.push(mid);
        doorways.push(json!({ "middle": [mid.x, w.floor, mid.y], "width": w.a.distance(w.b), "height": w.top - w.floor, "into_room": w.room }));
    }
    json_out(
        json!({ "pieces": m.pieces.iter().map(piece_json).collect::<Vec<_>>(), "doorways": doorways }),
    )
}

fn mcp_piece_add(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    use super::pieces::{Piece, at_doorway, nearest_doorway};
    let kind = enum_arg(&a, "kind", super::pieces::PieceKind::from_id)?.ok_or("give `kind`")?;
    let pos = Vec3::from_array(arg::<[f32; 3]>(&a, "position")?);
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    let mut p = if kind.in_doorway() {
        let ways = super::pieces::doorways_of(m);
        at_doorway(
            kind,
            nearest_doorway(&ways, pos, 4.0)
                .ok_or("no doorway within 4 m of `position` (dungeons_pieces lists them)")?,
        )
    } else {
        Piece::new(kind, pos, 0.0)
    };
    let mut out = Value::Null;
    edit::map(
        &mut d,
        &format!("Add {}", kind.name().to_lowercase()),
        |m| {
            let ids: Vec<u32> = m.pieces.iter().map(|x| x.id).collect();
            apply_piece_args(&mut p, &a, &ids)?;
            if kind.in_doorway() {
                let gone: Vec<u32> = m
                    .pieces
                    .iter()
                    .filter(|x| x.kind.in_doorway() && x.position().distance(p.position()) < 1.0)
                    .map(|x| x.id)
                    .collect();
                m.pieces.retain(|x| !gone.contains(&x.id));
                p.id = m.alloc_piece();
                for x in &mut m.pieces {
                    for l in &mut x.links {
                        if gone.contains(l) {
                            *l = p.id;
                        }
                    }
                }
            } else {
                p.id = m.alloc_piece();
            }
            out = piece_json(&p);
            m.pieces.push(p.clone());
            Ok(())
        },
    )?;
    json_out(out)
}

fn mcp_piece_set(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    let cur = m
        .pieces
        .iter()
        .find(|x| x.id == id)
        .ok_or_else(|| format!("no piece {id}"))?
        .clone();
    let kind = enum_arg(&a, "kind", super::pieces::PieceKind::from_id)?.unwrap_or(cur.kind);
    let pos: Option<[f32; 3]> = opt_arg(&a, "position")?;
    // a door or gate given somewhere new (or a piece becoming one): into the doorway there
    let into = if kind.in_doorway() && (pos.is_some() || !cur.kind.in_doorway()) {
        let at = pos.map(Vec3::from_array).unwrap_or(cur.position());
        let ways = super::pieces::doorways_of(m);
        Some(super::pieces::at_doorway(
            kind,
            super::pieces::nearest_doorway(&ways, at, 4.0)
                .ok_or("no doorway within 4 m (dungeons_pieces lists them)")?,
        ))
    } else {
        None
    };
    let mut out = Value::Null;
    edit::map(&mut d, "Change piece", |m| {
        let ids: Vec<u32> = m.pieces.iter().map(|x| x.id).collect();
        let p = m
            .pieces
            .iter_mut()
            .find(|x| x.id == id)
            .ok_or_else(|| format!("no piece {id}"))?;
        p.kind = kind;
        if let Some(at) = &into {
            (p.pos, p.yaw, p.width, p.height) = (at.pos, at.yaw, at.width, at.height);
        } else if let Some(v) = pos {
            p.pos = v;
        }
        apply_piece_args(p, &a, &ids)?;
        p.auto = false;
        out = piece_json(p);
        Ok(())
    })?;
    json_out(out)
}

fn mcp_piece_remove(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: Option<u32> = opt_arg(&a, "id")?;
    let auto: bool = opt_arg(&a, "auto")?.unwrap_or(false);
    let all: bool = opt_arg(&a, "all")?.unwrap_or(false);
    if id.is_none() && !auto && !all {
        return Err("give `id`, `auto`: true or `all`: true".into());
    }
    let n = edit::map(&mut d, "Remove pieces", |m| {
        let gone: Vec<u32> = m
            .pieces
            .iter()
            .filter(|x| all || Some(x.id) == id || (auto && x.auto))
            .map(|x| x.id)
            .collect();
        m.pieces.retain(|x| !gone.contains(&x.id));
        for x in &mut m.pieces {
            x.links.retain(|l| !gone.contains(l));
        }
        Ok(gone.len())
    })?;
    json_out(json!({ "removed": n }))
}

fn mcp_pieces_auto(In(_): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let n = edit::map(&mut d, "Template doors", |m| {
        super::pieces::place_for_layout(m);
        Ok(m.pieces.iter().filter(|p| p.auto).count())
    })?;
    json_out(json!({ "placed": n }))
}

fn mcp_piece_use(
    In(a): In<Value>,
    map: Res<super::model::DungeonMap>,
    mut st: ResMut<super::pieces::PieceState>,
) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    let m = map.0.as_ref().ok_or("no dungeon is open")?;
    let p = m
        .pieces
        .iter()
        .find(|x| x.id == id)
        .ok_or_else(|| format!("no piece {id}"))?;
    if p.kind.is_marker() {
        return Err(format!(
            "{} is a marker for the game: nothing to work",
            p.kind.name()
        ));
    }
    super::pieces::work(&mut st, &m.pieces, id);
    let now: Vec<Value> = std::iter::once(id)
        .chain(
            if matches!(
                p.kind,
                super::pieces::PieceKind::Lever | super::pieces::PieceKind::PressurePlate
            ) {
                p.links.clone()
            } else {
                vec![]
            },
        )
        .filter_map(|i| m.pieces.iter().find(|x| x.id == i))
        .map(|x| json!({ "id": x.id, "kind": x.kind.id(), "open": st.is_open(x) }))
        .collect();
    json_out(json!({ "now": now }))
}

fn mcp_way_out(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    let names = || {
        edit::FRAMES
            .iter()
            .map(|f| format!("{} ({})", f.asset, f.name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // Some(None): back to the way in's
    let frame = match opt_arg::<String>(&a, "frame")?.as_deref().map(str::trim) {
        None => None,
        Some("match" | "way_in" | "auto") => Some(None),
        Some("none" | "" | "liquid" | "liquid_only") => Some(Some(String::new())),
        Some(f) => Some(Some(
            edit::frame_named(f)
                .ok_or_else(|| format!("unknown frame `{f}`; the frames are: {}", names()))?
                .asset
                .to_string(),
        )),
    };
    let color = match opt_arg::<String>(&a, "color")?.as_deref().map(str::trim) {
        None => None,
        Some("match" | "way_in" | "auto") => Some(None),
        Some(c) => Some(Some(
            PortalColor::from_id(c).ok_or_else(|| format!("unknown color `{c}`"))?,
        )),
    };
    let pos = position_arg(&a, "position")?;
    let facing = opt_arg::<f32>(&a, "facing_deg")?.map(heading_of);
    let reset = opt_arg::<bool>(&a, "reset_position")?.unwrap_or(false);
    // a new spot: what isn't given stays as it is (on the floor of the room there, underground)
    let spot = match (pos, facing) {
        (None, None) => None,
        _ => {
            let (cur, cur_yaw) = way_out_at(m, &d).unwrap_or((Vec3::ZERO, 0.0));
            let at = match pos {
                None => cur,
                Some((p, Some(y))) => Vec3::new(p.x, y, p.y),
                Some((p, None)) if m.kind == MapKind::Realm => {
                    Vec3::new(p.x, d.terrain.height_at(p.x, p.y), p.y)
                }
                Some((p, None)) => {
                    let plan = &m.plan;
                    let r = plan
                        .rooms
                        .iter()
                        .filter(|r| r.kind.is_space() && r.shape.sdf(p) > 0.0)
                        .min_by(|a, b| {
                            ((plan.level_y(a.level) + a.floor - cur.y).abs())
                                .total_cmp(&(plan.level_y(b.level) + b.floor - cur.y).abs())
                        });
                    let r = r.ok_or("that spot isn't inside a room")?;
                    Vec3::new(p.x, plan.level_y(r.level) + r.floor, p.y)
                }
            };
            Some((at, facing.unwrap_or(cur_yaw)))
        }
    };
    if frame.is_some() || color.is_some() || spot.is_some() || reset {
        edit::way_out(&mut d, "Way out", |w| {
            if let Some(f) = frame {
                w.frame = f;
            }
            if let Some(c) = color {
                w.color = c;
            }
            if reset {
                w.at = None;
                w.yaw = 0.0;
            }
            if let Some((at, yaw)) = spot {
                w.at = Some(at.to_array());
                w.yaw = yaw;
            }
        })?;
    }
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    json_out(
        json!({ "way_out": way_out_json(m, &d), "frames": edit::FRAMES.iter().map(|f| json!({ "id": f.asset, "name": f.name })).collect::<Vec<_>>() }),
    )
}

/// The open map's way out (its foot), for the minimaps.
fn exit_of(m: &MapDoc, d: &WorldData) -> Option<Vec3> {
    super::portal::exit_want(m, d.gameplay.player_start.as_ref(), |id| {
        d.store.items.get(&id).cloned()
    })
    .map(|w| w.tf.translation)
}

fn mcp_minimap(
    In(a): In<Value>,
    d: WorldData,
    world: Res<DungeonWorld>,
) -> Result<Vec<McpContent>, String> {
    let m = d
        .dungeon_map
        .0
        .as_ref()
        .filter(|m| m.kind == MapKind::Interior)
        .ok_or("no underground dungeon is open")?;
    let c = world
        .compiled
        .clone()
        .ok_or("the dungeon is still being built (dungeons_wait)")?;
    let only: Option<i32> = opt_arg(&a, "level")?;
    let save = opt_arg::<bool>(&a, "save")?.unwrap_or(true);
    let dir = d.meta.path.clone().filter(|_| save);
    let exit = exit_of(m, &d);
    let mut out = vec![];
    let mut info = vec![];
    for lv in super::survey::levels(m)
        .into_iter()
        .filter(|l| only.is_none_or(|o| o == *l))
    {
        let Some(mm) = super::survey::minimap(m, &c, lv, exit) else {
            continue;
        };
        let png = super::survey::png(&mm);
        let file = match &dir {
            Some(dir) => {
                let p = dir.join(format!("minimap_L{lv}.png"));
                std::fs::write(&p, &png).map_err(|e| format!("{}: {e}", p.display()))?;
                Some(p.display().to_string())
            }
            None => None,
        };
        info.push(json!({ "level": lv, "file": file, "origin": [mm.origin.x, mm.origin.y], "metres_per_pixel": mm.mpp, "size": [mm.image.width(), mm.image.height()] }));
        out.push(McpContent::Png(png));
    }
    if info.is_empty() {
        return Err(format!("no rooms on level {}", only.unwrap_or(0)));
    }
    out.insert(0, McpContent::Json(json!({ "minimaps": info })));
    Ok(out)
}

fn mcp_walkable(
    In(a): In<Value>,
    d: WorldData,
    world: Res<DungeonWorld>,
) -> Result<Vec<McpContent>, String> {
    let m = d
        .dungeon_map
        .0
        .as_ref()
        .filter(|m| m.kind == MapKind::Interior)
        .ok_or("no underground dungeon is open")?;
    let c = world
        .compiled
        .clone()
        .ok_or("the dungeon is still being built (dungeons_wait)")?;
    let from = match opt_arg::<[f32; 3]>(&a, "from")? {
        Some(p) => Vec3::from_array(p),
        None => d
            .gameplay
            .player_start
            .as_ref()
            .map(|p| Vec3::from_array(p.pos))
            .or_else(|| super::survey::entrance(m, &c))
            .ok_or("no rooms")?,
    };
    let doors: String = opt_arg(&a, "doors")?.unwrap_or_else(|| "open".into());
    let open: std::collections::HashMap<u32, bool> = match doors.as_str() {
        "open" => m.pieces.iter().map(|p| (p.id, true)).collect(),
        "as_placed" => Default::default(),
        "shut" => m.pieces.iter().map(|p| (p.id, false)).collect(),
        x => return Err(format!("doors: open, as_placed or shut (not {x})")),
    };
    let blockers = super::pieces::blockers(&m.pieces, &open);
    let w = super::survey::walk(m, &c, from, &blockers);
    let r2 = |v: f32| (v * 100.0).round() / 100.0;
    let unreachable: Vec<Value> = w
        .rooms
        .iter()
        .filter(|r| r.3 < 0.05)
        .map(|r| json!({ "id": r.0, "name": r.1, "level": r.2 }))
        .collect();
    json_out(json!({
        "from": from.to_array(),
        "doors": doors,
        "floor_spots": w.spots,
        "reached": w.reached,
        "share_reached": r2(w.reached as f32 / w.spots.max(1) as f32),
        "unreachable": unreachable,
        "rooms": w.rooms.iter().map(|r| json!({ "id": r.0, "name": r.1, "level": r.2, "reached": r2(r.3) })).collect::<Vec<_>>(),
    }))
}

fn mcp_lights(In(_): In<Value>, d: WorldData) -> Result<Vec<McpContent>, String> {
    let m = d.dungeon_map.0.as_ref().ok_or("no dungeon is open")?;
    json_out(json!({ "lights": m.lights.iter().map(light_json).collect::<Vec<_>>() }))
}

fn mcp_light_add(
    In(a): In<Value>,
    mut d: WorldData,
    mut tool: ResMut<super::ui::DungeonTool>,
) -> Result<Vec<McpContent>, String> {
    let kind = enum_arg(&a, "kind", LightKind::from_id)?.ok_or("give `kind`")?;
    let pos: [f32; 3] = arg(&a, "position")?;
    if opt_arg(&a, "fixture")?.unwrap_or(false) {
        if !matches!(kind, LightKind::Brazier | LightKind::Candles) {
            return Err("`fixture` is for kind brazier or candles (wall torches and lanterns: dungeons_wall)".into());
        }
        if d.dungeon_map.0.is_none() {
            return Err("no dungeon is open".into());
        }
        tool.requests.push(super::ui::Request::FloorFixture {
            kind,
            at: Vec3::from_array(pos),
        });
        return json_out(
            json!({ "queued": format!("{} on the floor", kind.id()), "note": "applied this frame; dungeons_lights lists it" }),
        );
    }
    let mut out = None;
    edit::map(&mut d, "Add light", |m| {
        let id = m.alloc_light();
        let mut l = LightDef::new(id, kind, Vec3::from_array(pos));
        apply_light_args(&mut l, &a)?;
        out = Some(light_json(&l));
        m.lights.push(l);
        Ok(())
    })?;
    json_out(out.unwrap_or_default())
}

fn mcp_light_set(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    let mut out = None;
    edit::map(&mut d, "Change light", |m| {
        let l = m
            .lights
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or_else(|| format!("no light {id}"))?;
        if let Some(k) = enum_arg(&a, "kind", LightKind::from_id)? {
            l.kind = k;
        }
        apply_light_args(l, &a)?;
        out = Some(light_json(l));
        Ok(())
    })?;
    json_out(out.unwrap_or_default())
}

fn mcp_light_remove(In(a): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&a, "id")?;
    edit::map(&mut d, "Remove light", |m| {
        let n = m.lights.len();
        m.lights.retain(|l| l.id != id);
        if m.lights.len() == n {
            Err(format!("no light {id}"))
        } else {
            Ok(())
        }
    })?;
    json_out(json!({ "removed": id }))
}

fn mcp_dress(
    In(a): In<Value>,
    mut tool: ResMut<super::ui::DungeonTool>,
) -> Result<Vec<McpContent>, String> {
    let clear: bool = opt_arg(&a, "clear")?.unwrap_or(false);
    let room: Option<u32> = opt_arg(&a, "room")?;
    tool.requests.push(match (clear, room) {
        (true, _) => super::ui::Request::ClearDressing,
        (false, Some(id)) => super::ui::Request::DressRoom(id),
        (false, None) => super::ui::Request::Dress,
    });
    json_out(json!({ "queued": if clear { "clear dressing" } else { "dress" } }))
}

fn mcp_view(
    In(a): In<Value>,
    mut view: ResMut<DungeonView>,
    mut orbit: ResMut<crate::camera::OrbitCamera>,
) -> Result<Vec<McpContent>, String> {
    if let Some(l) = opt_arg::<i32>(&a, "level")? {
        view.level = l;
    }
    if let Some(c) = opt_arg::<bool>(&a, "cut")? {
        view.cut = c;
    }
    let dist: Option<f32> = opt_arg(&a, "distance")?;
    if let Some(f) = opt_arg::<[f32; 3]>(&a, "focus")? {
        let (yaw, pitch, cur) = (orbit.yaw, orbit.pitch, orbit.distance);
        orbit.set_view(Vec3::from_array(f), yaw, pitch, dist.unwrap_or(cur));
    }
    if opt_arg::<bool>(&a, "top")?.unwrap_or(false) {
        let (f, yaw) = (orbit.focus, orbit.yaw);
        let dd = dist.unwrap_or(orbit.distance.max(60.0));
        orbit.set_view(f, yaw, 1.5, dd);
    }
    json_out(json!({ "level": view.level, "cut": view.cut }))
}

fn mcp_walk_in(
    In(_): In<Value>,
    mut tool: ResMut<super::ui::DungeonTool>,
) -> Result<Vec<McpContent>, String> {
    tool.requests.push(super::ui::Request::WalkIn);
    json_out(json!({ "queued": "walk in" }))
}
