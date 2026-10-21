//! MCP: `objects_place_town` places a whole hamlet, village, town or city in one call, like a
//! click (or a drawn loop) with Objects › Set pieces › Village / Town.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::kinds::{Style, TYPES, TownSize};
use super::place::{Area, Request, place_town};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg, position_arg};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "objects_place_town",
        description: "Place a whole settlement in one call, composed like a hand-made MMO town (Goldshire, Riverwood): few buildings, well apart, with grass, trees and gardens between them. A hamlet (about 4 buildings: a farmstead and cottages round a well by the road), a village (about 8: a green with a well beside the road, the inn and chapel facing it, a store across the road, the smithy where the road comes in, homes along a lane, a farm and a windmill on the outskirts), a town (about 14: a cobbled square beside the high street with a statue, a well, a few market stalls, benches and lamps, framed by the town hall at the end of its axis, the inn, chapel and general store facing it; homes and shops spaced along lanes; a farmstead and a watchtower outside) or a city (about 28: a grand square with a rotunda, the bank and best shops on its corners, several lanes, farmsteads, windmill, towers). \
Streets follow the ground (off water and steep slopes) and become real roads (roads_list / roads_update edit them); lanes branch off at gentle angles. Every building faces its street or the square, its setback and angle varied a little, none on a road, on water, on another building or on steep ground; the important ones stand on the square or on a rise. Dressing is restrained: lamps near the centre, garden fences round some house plots, flower boxes, barrels and a cart by some shops, haystacks by the farms, and trees that suit the land between the buildings. Buildings level the ground under them, the square is cobbled, the open ground between gets a soft lawn on grassy land, and nature is cleared off the plots only. All its objects share one group id. One undo step. \
Where: `position` [x, z] for an area of the size's own extent there (about 90 × 70 m hamlet, 160 × 120 village, 230 × 180 town, 340 × 270 city), turned so its main road runs the flattest way (`yaw_deg` turns it instead: the main road's bearing); or `polygon` [[x, z], …] to fill an exact area (its size then follows the area; a bigger area gets somewhat more buildings, spaced the same); or neither to fill the current selection (the area prompt's selected area). \
`buildings` asks for a mix instead of the default one, e.g. [{\"type\": \"blacksmith\", \"count\": 1}, {\"type\": \"inn\", \"count\": 1}, {\"type\": \"house\", \"count\": 6}] (or {\"inn\": 1, \"house\": 6}); plain words work (house, cottage, townhouse, inn, tavern, blacksmith, smithy, chapel, church, temple, town hall, bank, shop, general store, bakery, butcher, tailor, cobbler, alchemist, jeweller, library, chandler, brewery, weaver, herbalist, carpenter, cooper, bowyer, armoury, stable, barracks, mason, storehouse, fisherman, potter, tannery, hunter's lodge, sawmill, farmhouse, watchtower, granary, barn, windmill; market / stall puts that many stalls on the square). Unknown words and models this account can't use (a locked asset pack) are skipped and listed in `notes`. \
`style`: auto (the world's art style, world_info › art_style: photoreal in a realistic world, hand-painted in a hand-painted one when that pack is usable) | handpainted | photoreal. `roads` (default true): streets as real roads. `walls` (towns and cities; default false: MMO towns lie open in the land): a log palisade along the outline with gates where the streets leave (hand-painted). `seed` picks the arrangement (same seed and area, same town). Returns the name, counts per building type, the road ids, the group id and notes.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": TownSize::ALL.map(|k| k.id()), "description": "hamlet | village | town | city (default village)" },
                    "size": { "type": "string", "enum": TownSize::ALL.map(|k| k.id()), "description": "the same as `kind` (either works)" },
                    "position": crate::mcp_tools::position_schema("centre of the settlement (instead of `polygon`)"),
                    "yaw_deg": crate::mcp_tools::bearing_schema("with `position`: the main street's bearing (default: the flattest way)"),
                    "polygon": { "type": "array", "minItems": 3, "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3 }, "description": "the exact area, [[x, z], …] in metres" },
                    "style": { "type": "string", "enum": Style::ALL.map(|s| s.id()), "default": "auto" },
                    "buildings": {
                        "description": "the buildings wanted (default: the size's own mix): [{type, count}] or {type: count}",
                        "oneOf": [
                            { "type": "array", "items": { "type": "object", "properties": { "type": { "type": "string" }, "count": { "type": "integer", "minimum": 1 } }, "required": ["type"] } },
                            { "type": "object", "additionalProperties": { "type": "integer", "minimum": 0 } }
                        ]
                    },
                    "roads": { "type": "boolean", "default": true, "description": "streets as real roads" },
                    "walls": { "type": "boolean", "default": false, "description": "a palisade with gates round a town or city (default: none)" },
                    "seed": { "type": "integer", "minimum": 0, "description": "arrangement (default: a new one)" },
                    "name": { "type": "string", "description": "the settlement's name (its streets and group are named after it; default: one made up)" }
                },
                "x-aliases": crate::mcp_tools::xz_aliases()
            })
        },
        run,
    });
}

fn xz(v: &Value) -> Option<Vec2> {
    let f: Vec<f32> = v
        .as_array()?
        .iter()
        .filter_map(|v| v.as_f64())
        .map(|v| v as f32)
        .collect();
    match f[..] {
        [x, z] | [x, _, z] => Some(Vec2::new(x, z)),
        _ => None,
    }
}

/// `buildings` as (word, count): `[{type, count}]`, `{type: count}` or `["inn", "house"]`.
fn buildings_arg(v: &Value) -> Result<Vec<(String, usize)>, String> {
    let bad = || {
        "`buildings` must be [{\"type\": \"inn\", \"count\": 1}, …] or {\"inn\": 1, …}".to_string()
    };
    match v {
        Value::Array(a) => a
            .iter()
            .map(|e| match e {
                Value::String(s) => Ok((s.clone(), 1)),
                Value::Object(o) => {
                    let t = o
                        .get("type")
                        .or_else(|| o.get("name"))
                        .and_then(|t| t.as_str())
                        .ok_or_else(bad)?;
                    let k = o.get("count").and_then(|c| c.as_u64()).unwrap_or(1) as usize;
                    Ok((t.to_string(), k.min(200)))
                }
                _ => Err(bad()),
            })
            .collect(),
        Value::Object(o) => o
            .iter()
            .map(|(t, k)| Ok((t.clone(), k.as_u64().ok_or_else(bad)? as usize)))
            .collect(),
        _ => Err(bad()),
    }
}

fn run(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let kind = opt_arg::<String>(&args, "size")?.or(opt_arg::<String>(&args, "kind")?);
    let size = match kind {
        None => TownSize::Village,
        Some(k) => TownSize::parse(&k)
            .ok_or_else(|| format!("unknown kind `{k}` (hamlet, village, town, city)"))?,
    };
    let style = match opt_arg::<String>(&args, "style")? {
        None => Style::Auto,
        Some(s) => Style::parse(&s)
            .ok_or_else(|| format!("unknown style `{s}` (auto, handpainted, photoreal)"))?,
    };
    // auto follows the world's art style (`crate::art_style`)
    let style = match (style, world.resource::<crate::world::WorldMeta>().art_style) {
        (Style::Auto, crate::art_style::ArtStyle::Realistic) => Style::Photoreal,
        (Style::Auto, crate::art_style::ArtStyle::HandPainted)
            if crate::market::usable("hp:inn") =>
        {
            Style::Handpainted
        }
        (s, _) => s,
    };
    let seed = match args.get("seed") {
        None | Some(Value::Null) => fastrand::u32(..),
        Some(v) => v.as_u64().ok_or("`seed` must be a whole number")? as u32,
    };
    let buildings = match args.get("buildings").filter(|v| !v.is_null()) {
        Some(v) => Some(buildings_arg(v)?),
        None => None,
    };
    let o = world.resource::<crate::camera::OrbitCamera>();
    let cam = (o.focus + Vec3::new(o.yaw.sin(), 0.0, o.yaw.cos()) * 100.0).xz();
    let mut lead_out = true;
    let area =
        if let Some(poly) = args.get("polygon").filter(|v| !v.is_null()) {
            let pts: Vec<Vec2> = poly
                .as_array()
                .ok_or("`polygon` must be [[x, z], …]")?
                .iter()
                .map(|p| xz(p).ok_or("a `polygon` corner must be [x, z]"))
                .collect::<Result<_, _>>()?;
            if pts.len() < 3 {
                return Err("`polygon` needs at least 3 corners".into());
            }
            Area::Outline(pts, cam)
        } else if let Some((at, _)) = position_arg(&args, "position")? {
            match opt_arg::<f32>(&args, "yaw_deg")? {
                // the main street's bearing: its direction is bearing_dir; the frame's x runs along it
                Some(b) => {
                    let d = crate::mcp_tools::bearing_dir(b);
                    Area::ClickTurned(at, super::layout::yaw_along(d))
                }
                None => Area::Click(at),
            }
        } else {
            let sel = world.resource::<crate::prompt::selection::Selection>();
            match sel.polygon() {
                Some(p) if p.len() >= 3 => {
                    // (the selection's edge fades edits out: streets stop at it)
                    lead_out = false;
                    Area::Outline(p.to_vec(), cam)
                }
                _ => return Err(
                    "give `position` [x, z] (or `polygon` [[x, z], …]), or select an area first"
                        .into(),
                ),
            }
        };
    let walls = opt_arg::<bool>(&args, "walls")?.unwrap_or(false);
    let name: Option<String> = opt_arg(&args, "name")?;
    let req = Request {
        size,
        area,
        style,
        roads: opt_arg::<bool>(&args, "roads")?.unwrap_or(true),
        walls,
        buildings,
        seed,
        lead_out,
        name,
    };
    let r = place_town(world, &req)?;
    let roads: Vec<Value> = {
        let all = world.resource::<crate::roads::Roads>();
        r.roads.iter().filter_map(|id| all.get(*id)).map(|rd| json!({ "id": rd.id, "name": rd.name, "kind": format!("{:?}", rd.kind), "length_m": rd.length().round() })).collect()
    };
    let types: Vec<&str> = TYPES.iter().map(|t| t.name).collect();
    let (buildings, extras): (
        serde_json::Map<String, Value>,
        serde_json::Map<String, Value>,
    ) = {
        let (b, e): (Vec<_>, Vec<_>) = r
            .counts
            .iter()
            .partition(|(k, _)| types.contains(&k.as_str()));
        (
            b.into_iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
            e.into_iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
        )
    };
    Ok(vec![McpContent::Json(json!({
        "name": r.name,
        "size": r.size.id(),
        "style": r.style.id(),
        "seed": seed,
        "centre": [r.centre.x.round(), r.centre.y.round()],
        "area_m2": r.area.round(),
        "outline": r.outline.iter().map(|p| [p.x.round(), p.y.round()]).collect::<Vec<_>>(),
        "group": r.group,
        "objects": r.ids.len(),
        "buildings_total": r.buildings,
        "buildings": buildings,
        "square_and_streets": extras,
        "roads": roads,
        "notes": r.notes,
        "undo": "one undo step",
    }))])
}
