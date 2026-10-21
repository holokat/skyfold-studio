//! Object tools: the asset library, placing, moving and removing objects, and finding them.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpTool, compass, turn_quat};
use crate::objects::{Category, Library, Placed};

const CATEGORIES: [Category; 7] = [
    Category::Trees,
    Category::Rocks,
    Category::Plants,
    Category::Debris,
    Category::Structures,
    Category::Props,
    Category::Imported,
];
const MAX_BATCH: usize = 2000;

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "objects_library",
        description: "The placeable asset library (photoscanned and detailed procedural models): id, name, category (trees, rocks, plants, debris, structures, props, imported), group (furniture, lighting, camp…, when it has one), footprint radius and height (m), number of variants, the biomes it belongs to and whether players can walk on it. \
Lists the world's art style by default (world_info › art_style: photoreal or handpainted; style all lists every style; imported models and the procedural trees, bushes, palms, cacti, reeds and lily pads fit every style). Filter by `category`, `group`, `style`, `pack` and/or `search` words, and by `kind`: objects (single pieces) or scenes (complete scenes in one model, e.g. a whole monster-encounter site with every piece in place, marked scene: true; placed like any object). Use the ids with objects_place and nature_scatter; objects_preview shows what one looks like. \
Only assets this account can use are listed: assets in asset packs it hasn't got are left out and summed up in locked_packs (pack, price, how many matched); tell the user when one would suit (market_packs has details).",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "category": enum_schema(ids(&CATEGORIES), "only this category"),
                    "group": { "type": "string", "description": "only this group (e.g. furniture, lighting, containers, camp, food)" },
                    "style": { "type": "string", "description": "only this style: photoreal, handpainted or all (default: the world's art style; imported models and the style-following procedural plants count as every style)" },
                    "pack": { "type": "string", "description": "only this asset pack (an id from market_packs)" },
                    "search": { "type": "string", "description": "words in the id, name or group" },
                    "kind": { "type": "string", "enum": ["objects", "scenes"], "description": "only single objects, or only complete scenes in one model (default: both)" }
                }
            })
        },
        run: |w, a| run(w, library, a),
    });
    app.mcp_tool(McpTool {
        name: "objects_place",
        description: "Place objects (Objects tab › Place), up to 2000 per call (assets of asset packs this account hasn't got are refused: see objects_library). Each item: `asset` id, `position` [x, z] (set on the ground, sunk 5 cm like the editor does; ships and pier pieces float on the water there) or [x, y, z] (exact height), optional `yaw_deg` (turn about the vertical in compass degrees: clockwise seen from above, 0 = as modelled), \
`scale` (× the asset's default size: a number, or [x, y, z] to stretch along the model's own axes, y up), `variant`, `align` (tilt to the ground's slope) and `y_offset` (m, e.g. raise a bridge deck over water). \
Buildings and other structures fit the ground as they're placed: seated at the median ground under their footprint, the ground levelled under them with banks into the slope, grass and nature kept off (their height is chosen then, so a given y is replaced — except in a cave: at [x, y, z] well under the ground inside a cave's bounds an object stands on the cave's floor and the ground over it is left alone); `fit_ground` false (per item, or for the whole call) leaves the ground alone and keeps the position as given, true fits any object (props, imported models). Returns the new ids and which were fitted. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array", "minItems": 1, "maxItems": MAX_BATCH,
                        "items": {
                            "type": "object",
                            "properties": {
                                "asset": { "type": "string" },
                                "position": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "[x, z] on the ground or [x, y, z]" },
                                "yaw_deg": { "type": "number", "default": 0, "description": "turn, clockwise seen from above (degrees)" },
                                "scale": scale_schema(),
                                "variant": { "type": "integer", "minimum": 0, "default": 0 },
                                "align": { "type": "boolean", "description": "tilt to the slope (default: the asset's own setting)" },
                                "y_offset": { "type": "number", "default": 0 },
                                "fit_ground": fit_schema()
                            },
                            "required": ["asset", "position"]
                        }
                    },
                    "fit_ground": fit_schema()
                },
                "required": ["items"]
            })
        },
        run: |w, a| {
            // assets of packs this account hasn't got are refused before anything is placed
            locked_items(w, &a)?;
            run(w, place, a)
        },
    });
    app.mcp_tool(McpTool {
        name: "objects_transform",
        description: "Move, turn or resize placed objects by id (like Select & Move): `position` [x, z] (re-seated on the ground) or [x, y, z], `yaw_deg` (absolute turn, clockwise seen from above), `rotate_deg` (turn further, clockwise), `scale` (absolute: a number or [x, y, z] per model axis), `y_offset`. Fitted buildings and structures are fitted again where they end up (the old spot gets its ground back); lifting or lowering one in place (y only) takes its levelled ground along. `fit_ground` changes whether it levels the ground (see objects_place). One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array", "minItems": 1, "maxItems": MAX_BATCH,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "integer" },
                                "position": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3 },
                                "yaw_deg": { "type": "number", "description": "turn, clockwise seen from above (degrees)" },
                                "rotate_deg": { "type": "number", "description": "turn further by this many degrees (clockwise)" },
                                "scale": scale_schema(),
                                "y_offset": { "type": "number" },
                                "fit_ground": fit_schema()
                            },
                            "required": ["id"]
                        }
                    }
                },
                "required": ["items"]
            })
        },
        run: |w, a| run(w, transform, a),
    });
    app.mcp_tool(McpTool {
        name: "objects_group",
        description: "Set pieces and towns are groups: their objects share a `group` id (objects_query shows it) and the editor moves, turns, resizes and deletes them as one. \
`action`: `info` (the group's centre, size and pieces), `move` (by `offset` [dx, dz] m, or `to` a position [x, z]; every piece is set back on the ground at its new spot), `turn` (by `yaw_deg` about the group's centre, clockwise seen from above), `scale` (by `factor` about the centre), `ungroup` (free the pieces), `remove` (delete every piece). Give `group`, or the `id` of any piece. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "group": { "type": "integer" },
                    "id": { "type": "integer", "description": "any piece of the group (instead of `group`)" },
                    "action": { "type": "string", "enum": ["info", "move", "turn", "scale", "ungroup", "remove"] },
                    "offset": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "move: [dx, dz] metres" },
                    "to": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "move: the new centre [x, z]" },
                    "yaw_deg": { "type": "number", "description": "turn: degrees, clockwise seen from above" },
                    "factor": { "type": "number", "minimum": 0.1, "maximum": 10, "description": "scale" }
                },
                "required": ["action"]
            })
        },
        run: |w, a| run(w, group_tool, a),
    });
    app.mcp_tool(McpTool {
        name: "objects_remove",
        description: "Remove placed objects: by `ids`, or everything in an `area` (optionally only a `category` or `asset`). One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } },
                    "area": area_schema("Remove inside this area."),
                    "category": enum_schema(ids(&CATEGORIES), "area filter"),
                    "asset": { "type": "string", "description": "area filter: only this asset id" }
                }
            })
        },
        run: |w, a| run(w, remove, a),
    });
    app.mcp_tool(McpTool {
        name: "objects_lock",
        description: "Lock or free placed objects (like right-click › Lock in the editor): a locked object can't be moved, turned, resized or deleted, by the user or by tools, until it's freed again; it still shows, selects and copies. A set piece or town locks as a whole (give any piece, or `group`). `ids` and/or `group`; `locked` true (default) or false. objects_query shows `locked`. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } },
                    "group": { "type": "integer", "description": "a set piece or town's group id (instead of, or with, `ids`)" },
                    "locked": { "type": "boolean", "default": true }
                }
            })
        },
        run: |w, a| run(w, lock_tool, a),
    });
    app.mcp_tool(McpTool {
        name: "objects_fit_ground",
        description: "Fit placed objects to the terrain (buildings and structures already do this whenever they're placed or moved): the ground under the footprint is levelled to the object's floor (seated at the median ground, so the cut uphill and the fill downhill balance), banks blend into the slope, a packed-earth (gravel for stone) apron is painted and grass and nature keep off. \
`ids` (default: every building and structure), `fit`: true (level the ground under them, any kind of object), false (give the ground back and leave them standing as they are) or omitted (their kind's default). Returns each object's state: fitted, floor height, footprint size and the deepest cut / highest fill (m). One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } },
                    "fit": { "type": "boolean", "description": "true: level the ground under them, false: leave the ground alone (default: their kind's choice)" }
                }
            })
        },
        run: |w, a| run(w, fit_ground, a),
    });
    app.mcp_tool(McpTool {
        name: "objects_query",
        description: "Find placed objects: in an `area` (default: the whole world), optionally only a `category` or `asset`. Returns the total, counts per asset and up to `limit` objects (id, asset, position [x, y, z], yaw_deg (clockwise turn), scale (a number, or [x, y, z] when stretched)), nearest to the area's centre first.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "area": area_schema("Search area."),
                    "category": enum_schema(ids(&CATEGORIES), "only this category"),
                    "asset": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 0, "maximum": 2000, "default": 50 }
                }
            })
        },
        run: |w, a| run(w, query, a),
    });
}

fn lib_of<'a>(d: &'a WorldData) -> Result<&'a Library, String> {
    d.lib
        .as_deref()
        .ok_or_else(|| "the asset library is still loading; try again in a moment".to_string())
}

/// The turn as a compass bearing (clockwise seen from above).
pub(super) fn yaw_of(p: &Placed) -> f32 {
    compass(-p.yaw().to_degrees())
}

/// `scale`: × the default size, a number or per model axis [x, y, z].
fn scale_schema() -> Value {
    json!({ "type": ["number", "array"], "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "minimum": 0.05, "maximum": 20, "default": 1, "description": "× the asset's default size: a number, or [x, y, z] along the model's axes (y up)" })
}

fn fit_schema() -> Value {
    json!({ "type": "boolean", "description": "level the ground under it (default: buildings and other structures do, props and imported models don't)" })
}

/// The fit's view of the world.
fn fit_world<'a>(d: &'a mut WorldData) -> Result<crate::placement::FitWorld<'a>, String> {
    let WorldData {
        terrain,
        history,
        store,
        lib,
        water,
        fit,
        ..
    } = d;
    let lib = lib
        .as_deref()
        .ok_or("the asset library is still loading; try again in a moment")?;
    Ok(crate::placement::FitWorld {
        terrain,
        history,
        store,
        lib,
        water,
        found: &mut fit.found,
        bounds: &fit.bounds,
    })
}

fn scale_arg(it: &Value) -> Result<Option<Vec3>, String> {
    let ok = |v: f64| (v.is_finite() && v > 0.0).then_some((v as f32).clamp(0.05, 20.0));
    match it.get("scale") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_f64()
            .and_then(ok)
            .map(|s| Some(Vec3::splat(s)))
            .ok_or_else(|| "`scale` must be a positive number".into()),
        Some(Value::Array(a)) if a.len() == 3 => {
            let v: Vec<f32> = a.iter().filter_map(|x| x.as_f64().and_then(ok)).collect();
            if v.len() == 3 {
                Ok(Some(Vec3::new(v[0], v[1], v[2])))
            } else {
                Err("`scale` [x, y, z] needs three positive numbers".into())
            }
        }
        Some(_) => Err("`scale` must be a number or [x, y, z]".into()),
    }
}

/// The size for output: a number, or [x, y, z] when stretched.
pub(super) fn scale_of(p: &Placed) -> Value {
    let r = |v: f32| (v * 100.0).round() / 100.0;
    match p.stretch {
        None => json!(r(p.scale)),
        Some(_) => json!(p.scale3().to_array().map(r)),
    }
}

/// `objects_place` items whose asset is in a pack this account hasn't got (unknown ids are left
/// to `place`, which suggests close ones).
fn locked_items(w: &World, args: &Value) -> Result<(), String> {
    let lib = w.get_resource::<Library>();
    let market = crate::market::snapshot();
    for (i, it) in args
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(asset) = it.get("asset").and_then(Value::as_str) else {
            continue;
        };
        if lib.is_none_or(|l| l.get(asset).is_some()) {
            market.check(asset).map_err(|e| format!("item {i}: {e}"))?;
        }
    }
    Ok(())
}

fn library(In(args): In<Value>, d: WorldData) -> R {
    json_out(library_json(
        lib_of(&d)?,
        &crate::market::snapshot(),
        &args,
        Some(d.meta.art_style.id()),
    )?)
}

/// The `objects_library` listing: the usable assets matching the filters, and the locked packs.
/// Without a `style` argument it lists `default_style` (the world's art style; `None`: all).
fn library_json(
    lib: &Library,
    market: &crate::market::Market,
    args: &Value,
    default_style: Option<&str>,
) -> Result<Value, String> {
    let cat = opt_pick(args, "category", &CATEGORIES, "category", Category::name)?;
    let words: Vec<String> = opt_str(args, "search")?
        .unwrap_or("")
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let group = opt_str(args, "group")?.map(str::to_string);
    let style = match opt_str(args, "style")?.map(str::to_lowercase) {
        Some(s) if s == "all" || s == "any" || s.is_empty() => None,
        Some(s) => Some(
            crate::art_style::ArtStyle::parse(&s)
                .map(|a| a.id().to_string())
                .unwrap_or(s),
        ),
        None => default_style.map(str::to_string),
    };
    let kind = match opt_str(args, "kind")? {
        None => None,
        Some("objects") => Some(false),
        Some("scenes") => Some(true),
        Some(k) => return Err(format!("`kind` must be objects or scenes, not {k}")),
    };
    let pack = match opt_str(args, "pack")? {
        Some(p) => Some(
            market
                .pack_index(p)
                .ok_or_else(|| format!("unknown pack `{p}` (see market_packs)"))?,
        ),
        None => None,
    };
    let matching: Vec<&crate::objects::LibraryEntry> = lib
        .entries
        .iter()
        .filter(|e| cat.is_none_or(|c| e.category == c))
        .filter(|e| kind.is_none_or(|k| e.whole_scene == k))
        .filter(|e| {
            group
                .as_deref()
                .is_none_or(|g| e.group.eq_ignore_ascii_case(g))
        })
        .filter(|e| {
            style
                .as_deref()
                .is_none_or(|s| crate::art_style::fits(e, s))
        })
        .filter(|e| pack.is_none_or(|p| market.pack_index_of(&e.id) == Some(p)))
        .filter(|e| {
            let hay = format!("{} {} {}", e.id, e.name, e.group).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect();
    let list: Vec<Value> = matching
        .iter()
        .filter(|e| market.usable(&e.id))
        .map(|e| {
            let mut v = json!({
                "id": e.id, "name": e.name, "category": snake(&format!("{:?}", e.category)),
                "radius": super::terrain::round1(e.radius), "height": super::terrain::round1(e.height),
            });
            if !e.group.is_empty() {
                v["group"] = json!(e.group);
            }
            if e.meshes.len() > 1 {
                v["variants"] = json!(e.meshes.len());
            }
            // (most structures and props fit every biome: listed only when it narrows things down)
            if e.biomes.len() < crate::world::Biome::ALL.len() {
                v["biomes"] = json!(e.biomes.iter().map(|b| snake(&format!("{b:?}"))).collect::<Vec<_>>());
            }
            if e.whole_scene {
                v["scene"] = json!(true);
            }
            if e.walkable {
                v["walkable"] = json!(true);
            }
            v
        })
        .collect();
    let mut out = json!({ "count": list.len(), "assets": list, "note": "biomes: only listed for assets that don't fit every biome; walkable: players can stand on it" });
    if let Some(s) = &style {
        out["style"] = json!(s);
        if args.get("style").is_none() {
            out["style_note"] = json!(format!(
                "only the world's art style ({s}) is listed; style \"all\" lists every style"
            ));
        }
    }
    let locked = crate::market::mcp::locked_packs(market, matching.iter().copied());
    if !locked.is_empty() {
        out["locked_packs"] = json!(locked);
        out["locked_note"] = json!(
            "assets in these packs matched but aren't listed: this account hasn't got the packs (only their free samples, which are listed). They can't be placed; tell the user if one would suit."
        );
    }
    Ok(out)
}

/// [x, z] → on the ground (sunk like the editor's placement), [x, y, z] → exact.
fn position(v: &Value, t: &crate::terrain::Terrain) -> Result<Vec3, String> {
    let a = v
        .as_array()
        .ok_or("`position` must be [x, z] or [x, y, z]")?;
    let f: Vec<f32> = a
        .iter()
        .filter_map(|x| x.as_f64())
        .map(|x| x as f32)
        .collect();
    let p = match f[..] {
        [x, z] => Vec3::new(x, t.height_at(x, z) - 0.05, z),
        [x, y, z] => Vec3::new(x, y, z),
        _ => return Err("`position` must be [x, z] or [x, y, z]".into()),
    };
    if !t.in_bounds(p.x, p.z) {
        return Err(format!(
            "[{:.0}, {:.0}] is outside the world (±{:.0} m)",
            p.x,
            p.z,
            t.half()
        ));
    }
    Ok(p)
}

fn place(In(args): In<Value>, mut d: WorldData) -> R {
    let items = args
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or("missing `items`")?
        .clone();
    if items.is_empty() || items.len() > MAX_BATCH {
        return Err(format!("give 1..{MAX_BATCH} items"));
    }
    // check every item before anything is placed
    let mut todo = vec![];
    {
        let lib = lib_of(&d)?;
        for (i, it) in items.iter().enumerate() {
            let asset = opt_str(it, "asset")?.ok_or(format!("item {i}: missing `asset`"))?;
            let e = lib.get(asset).ok_or_else(|| {
                let w = asset.to_lowercase();
                let close: Vec<&str> = lib
                    .entries
                    .iter()
                    .filter(|e| {
                        e.id.to_lowercase().contains(&w) || w.contains(&e.id.to_lowercase())
                    })
                    .map(|e| e.id.as_str())
                    .take(6)
                    .collect();
                format!(
                    "item {i}: unknown asset `{asset}`{} (see objects_library)",
                    if close.is_empty() {
                        String::new()
                    } else {
                        format!("; did you mean {}", close.join(", "))
                    }
                )
            })?;
            let at = it
                .get("position")
                .ok_or(format!("item {i}: missing `position`"))?;
            let mut pos = position(at, &d.terrain).map_err(|e| format!("item {i}: {e}"))?;
            // [x, z]: ships and piers float on the water there
            if at.as_array().is_some_and(|a| a.len() == 2) {
                pos = e.seat(pos, &d.water, d.terrain.sea_level);
            }
            pos.y += num(it, "y_offset", 0.0)?;
            let turn = turn_quat(num(it, "yaw_deg", 0.0)?);
            let scale = scale_arg(it)
                .map_err(|e| format!("item {i}: {e}"))?
                .unwrap_or(Vec3::ONE)
                * e.default_scale;
            let variant =
                (num(it, "variant", 0.0)?.max(0.0) as u32).min(e.meshes.len().max(1) as u32 - 1);
            let align = opt_bool(it, "align")?.unwrap_or(e.align);
            let rot = if align {
                Quat::from_rotation_arc(Vec3::Y, d.terrain.normal_at(pos.x, pos.z)) * turn
            } else {
                turn
            };
            let fit = opt_bool(it, "fit_ground")?.or(opt_bool(&args, "fit_ground")?);
            todo.push((e.id.clone(), variant, pos, rot, scale, fit));
        }
    }
    d.history.begin(if todo.len() == 1 {
        "Place object"
    } else {
        "Place objects"
    });
    let mut ids = vec![];
    let mut choice = vec![];
    for (asset, variant, pos, rot, scale, fit) in todo {
        let id = d.store.new_id();
        d.history.touch_object(&d.store, id);
        let mut p = Placed {
            group: 0,
            id,
            asset,
            variant,
            pos: pos.to_array(),
            rot: rot.to_array(),
            scale: 1.0,
            stretch: None,
            locked: false,
        };
        p.set_scale3(scale);
        d.store.insert(p);
        ids.push(id);
        if fit.is_some() {
            choice.push((id, fit));
        }
    }
    let fitted = fit_world(&mut d)
        .map(|mut w| crate::placement::fit::fit_objects(&mut w, &ids, &choice))
        .unwrap_or_default();
    d.history.commit(&d.store);
    d.meta.dirty = true;
    json_out(
        json!({ "placed": ids.len(), "ids": ids, "fitted_to_ground": fitted, "note": "one undo step" }),
    )
}

fn transform(In(args): In<Value>, mut d: WorldData) -> R {
    let items = args
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or("missing `items`")?
        .clone();
    let mut todo = vec![];
    let mut choice = vec![];
    for (i, it) in items.iter().enumerate() {
        let id = it
            .get("id")
            .and_then(|v| v.as_u64())
            .ok_or(format!("item {i}: missing `id`"))?;
        let mut p = d
            .store
            .items
            .get(&id)
            .cloned()
            .ok_or(format!("item {i}: no object with id {id}"))?;
        if d.store.is_locked(id) {
            return Err(format!(
                "item {i}: object {id} is locked (objects_lock with locked: false frees it, if the user wants it changed)"
            ));
        }
        if let Some(v) = it.get("position") {
            let mut pos = position(v, &d.terrain).map_err(|e| format!("item {i}: {e}"))?;
            pos.y += num(it, "y_offset", 0.0)?;
            p.pos = pos.to_array();
        } else if let Some(o) = opt_num(it, "y_offset")? {
            p.pos[1] += o;
        }
        if let Some(y) = opt_num(it, "yaw_deg")? {
            // keep any tilt, replace the heading
            let turn = -y.to_radians() - p.yaw();
            p.rot = (Quat::from_rotation_y(turn) * p.rotation()).to_array();
        }
        if let Some(r) = opt_num(it, "rotate_deg")? {
            p.rot = (turn_quat(r) * p.rotation()).to_array();
        }
        if let Some(s) = scale_arg(it).map_err(|e| format!("item {i}: {e}"))? {
            p.set_scale3(s);
        }
        if let Some(f) = opt_bool(it, "fit_ground")? {
            choice.push((id, Some(f)));
        }
        todo.push(p);
    }
    d.history.begin("Move objects");
    for p in &todo {
        d.history.touch_object(&d.store, p.id);
        d.store.update(p.clone());
    }
    let ids: Vec<u64> = todo.iter().map(|p| p.id).collect();
    let fitted = fit_world(&mut d)
        .map(|mut w| {
            let mut out = crate::placement::fit::fit_objects(&mut w, &ids, &choice);
            // lifted or lowered in place: a fitted object's ground follows it
            out.extend(
                ids.iter()
                    .copied()
                    .filter(|id| crate::placement::fit::follow_lift(&mut w, *id)),
            );
            out
        })
        .unwrap_or_default();
    d.history.commit(&d.store);
    d.meta.dirty = true;
    let out: Vec<Value> = ids
        .iter()
        .filter_map(|id| d.store.items.get(id))
        .map(|p| json!({ "id": p.id, "position": p.pos.map(super::terrain::round1), "yaw_deg": yaw_of(p), "scale": scale_of(p), "fitted": d.fit.found.pads.contains_key(&p.id) }))
        .collect();
    json_out(json!({ "updated": out, "refitted": fitted, "note": "one undo step" }))
}

/// Ids in `area` matching the category / asset filters.
fn matching(d: &WorldData, args: &Value, area: Option<&Area>) -> Result<Vec<u64>, String> {
    let lib = lib_of(d)?;
    let cat = opt_pick(args, "category", &CATEGORIES, "category", Category::name)?;
    let asset = opt_str(args, "asset")?;
    let ok = |o: &Placed| {
        asset.is_none_or(|a| o.asset == a)
            && cat.is_none_or(|c| lib.get(&o.asset).is_some_and(|e| e.category == c))
    };
    Ok(match area {
        Some(a) => {
            let (c, r) = a.circle();
            d.store
                .query(c, r)
                .into_iter()
                .filter(|id| {
                    d.store
                        .items
                        .get(id)
                        .is_some_and(|o| ok(o) && a.contains(Vec2::new(o.pos[0], o.pos[2])))
                })
                .collect()
        }
        None => d
            .store
            .items
            .values()
            .filter(|o| ok(o))
            .map(|o| o.id)
            .collect(),
    })
}

fn remove(In(args): In<Value>, mut d: WorldData) -> R {
    let mut kept = 0;
    let ids: Vec<u64> = if let Some(ids) = crate::mcp_tools::opt_arg::<Vec<u64>>(&args, "ids")? {
        if let Some(id) = ids.iter().find(|id| d.store.is_locked(**id)) {
            return Err(format!(
                "object {id} is locked (objects_lock with locked: false frees it, if the user wants it removed); nothing was removed"
            ));
        }
        ids
    } else if let Some(a) = args.get("area") {
        let area = Area::parse(a)?;
        // locked objects in the area stay
        let all = matching(&d, &args, Some(&area))?;
        let free: Vec<u64> = all
            .iter()
            .copied()
            .filter(|id| !d.store.is_locked(*id))
            .collect();
        kept = all.len() - free.len();
        free
    } else {
        return Err("give `ids` or an `area`".into());
    };
    d.history.begin(if ids.len() == 1 {
        "Delete object"
    } else {
        "Delete objects"
    });
    let mut n = 0;
    for id in &ids {
        if d.store.items.contains_key(id) {
            d.history.touch_object(&d.store, *id);
            d.store.remove(*id);
            n += 1;
        }
    }
    // fitted ones give their ground back
    if let Ok(mut w) = fit_world(&mut d) {
        crate::placement::fit::sync(&mut w, Some(&ids));
    }
    d.history.commit(&d.store);
    if n > 0 {
        d.meta.dirty = true;
    }
    let locked = if kept > 0 {
        format!(" {kept} locked object(s) in the area were kept.")
    } else {
        String::new()
    };
    text(format!(
        "Removed {n} object(s){}.{locked} One undo step.",
        if n < ids.len() {
            format!(" ({} ids not found)", ids.len() - n)
        } else {
            String::new()
        }
    ))
}

fn fit_ground(In(args): In<Value>, mut d: WorldData) -> R {
    let on = opt_bool(&args, "fit")?;
    let ids: Vec<u64> = match crate::mcp_tools::opt_arg::<Vec<u64>>(&args, "ids")? {
        Some(ids) => {
            if let Some(bad) = ids.iter().find(|id| !d.store.items.contains_key(id)) {
                return Err(format!("no object with id {bad}"));
            }
            ids
        }
        // every structure that fits by default (given `fit`: whatever it was set to before), or
        // every object that wants to
        None => {
            let lib = lib_of(&d)?;
            let by_kind = |p: &Placed| {
                lib.get(&p.asset)
                    .is_some_and(|e| crate::placement::fit::fits_by_default(e, d.fit.bounds.of(e)))
            };
            let mut v: Vec<u64> = d
                .store
                .items
                .values()
                .filter(|p| {
                    if on.is_some() {
                        by_kind(p)
                    } else {
                        crate::placement::fit::wants(&d.fit.found, lib, &d.fit.bounds, p)
                    }
                })
                .map(|p| p.id)
                .collect();
            v.sort_unstable();
            v
        }
    };
    let choice: Vec<(u64, Option<bool>)> = if args.get("fit").is_some_and(|v| !v.is_null()) {
        ids.iter().map(|id| (*id, on)).collect()
    } else {
        vec![]
    };
    lib_of(&d)?;
    d.history.begin(if on == Some(false) {
        "Release the ground"
    } else {
        "Fit to the ground"
    });
    let changed = fit_world(&mut d)
        .map(|mut w| crate::placement::fit::fit_objects(&mut w, &ids, &choice))
        .unwrap_or_default();
    d.history.commit(&d.store);
    if !changed.is_empty() {
        d.meta.dirty = true;
    }
    let r = super::terrain::round1;
    let list: Vec<Value> = ids
        .iter()
        .take(500)
        .filter_map(|id| d.store.items.get(id))
        .map(|p| match d.fit.found.pads.get(&p.id) {
            Some(pad) => {
                let (cut, fill) = crate::placement::fit::cut_fill(&d.fit.found, &d.terrain, pad);
                let f = pad.footprint();
                json!({ "id": p.id, "asset": p.asset, "fitted": true, "floor_y": r(pad.level), "footprint_m": [r(f.half.x * 2.0), r(f.half.y * 2.0)], "cut_m": r(cut), "fill_m": r(fill) })
            }
            None => json!({ "id": p.id, "asset": p.asset, "fitted": false }),
        })
        .collect();
    json_out(json!({ "objects": list, "changed": changed.len(), "note": "one undo step" }))
}

fn group_tool(In(args): In<Value>, mut d: WorldData) -> R {
    use crate::placement::group;
    let g = match (
        args.get("group").and_then(|v| v.as_u64()),
        args.get("id").and_then(|v| v.as_u64()),
    ) {
        (Some(g), _) => g,
        (None, Some(id)) => d
            .store
            .items
            .get(&id)
            .map(|p| p.group)
            .ok_or_else(|| format!("no object {id}"))?,
        _ => return Err("give `group`, or the `id` of one of its pieces".into()),
    };
    let members = d.store.group_members(g);
    if g == 0 || members.is_empty() {
        return Err(format!(
            "{g} isn't a group (objects_query lists `group` for set pieces and towns)"
        ));
    }
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading; try again in a moment")?;
    let frame = group::frame(&d.store, lib, &d.fit.bounds, g)
        .ok_or("the group's models aren't loaded yet")?;
    let centre = frame.translation();
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or("missing `action`")?;
    if matches!(action, "move" | "turn" | "scale" | "remove")
        && members
            .iter()
            .any(|id| d.store.items.get(id).is_some_and(|p| p.locked))
    {
        return Err(format!(
            "group {g} is locked (objects_lock with locked: false frees it, if the user wants it changed)"
        ));
    }
    let mut ch = group::Change::none(centre);
    match action {
        "info" => {
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            for id in &members {
                if let Some(p) = d.store.items.get(id) {
                    *counts.entry(p.asset.clone()).or_default() += 1;
                }
            }
            let s3 = frame.scale3();
            let (c, sz) = (
                [centre.x, centre.z].map(super::terrain::round1),
                [s3.x, s3.z].map(super::terrain::round1),
            );
            return Ok(vec![McpContent::Json(
                json!({ "group": g, "pieces": members.len(), "centre": c, "size_m": sz, "assets": counts }),
            )]);
        }
        "ungroup" | "remove" => {
            d.history.begin(if action == "remove" {
                "Delete set"
            } else {
                "Ungroup set"
            });
            for id in &members {
                d.history.touch_object(&d.store, *id);
                if action == "remove" {
                    d.store.remove(*id);
                } else if let Some(mut p) = d.store.items.get(id).cloned() {
                    p.group = 0;
                    d.store.items.insert(*id, p);
                }
            }
            d.store.revision += 1;
            d.history.commit(&d.store);
            let mut v = json!({ "group": g, "note": "one undo step" });
            v[action] = json!(members.len());
            return Ok(vec![McpContent::Json(v)]);
        }
        "move" => {
            if let Some(to) = args.get("to").filter(|v| !v.is_null()) {
                let f: Vec<f32> = to
                    .as_array()
                    .ok_or("`to` must be [x, z]")?
                    .iter()
                    .filter_map(|v| v.as_f64())
                    .map(|v| v as f32)
                    .collect();
                let t = match f[..] {
                    [x, z] | [x, _, z] => Vec2::new(x, z),
                    _ => return Err("`to` must be [x, z]".into()),
                };
                ch.shift = t - centre.xz();
            } else {
                let o: [f32; 2] = crate::mcp_tools::opt_arg(&args, "offset")?
                    .ok_or("move needs `offset` [dx, dz] or `to` [x, z]")?;
                ch.shift = Vec2::from_array(o);
            }
        }
        "turn" => ch.yaw = -(num(&args, "yaw_deg", 0.0)?).to_radians(),
        "scale" => ch.k = num(&args, "factor", 1.0)?.clamp(0.1, 10.0),
        other => {
            return Err(format!(
                "unknown action `{other}` (info, move, turn, scale, ungroup, remove)"
            ));
        }
    }
    let starts = group::snapshot(&d.store, g);
    d.history.begin(format!(
        "{} set",
        match action {
            "move" => "Move",
            "turn" => "Turn",
            _ => "Resize",
        }
    ));
    let WorldData {
        store,
        history,
        terrain,
        ..
    } = &mut d;
    group::transform(store, history, terrain, g, &starts, ch);
    d.history.commit(&d.store);
    Ok(vec![McpContent::Json(
        json!({ "group": g, "pieces": members.len(), "action": action, "note": "one undo step; buildings refit the ground" }),
    )])
}

fn query(In(args): In<Value>, d: WorldData) -> R {
    let area = args
        .get("area")
        .filter(|a| !a.is_null())
        .map(Area::parse)
        .transpose()?;
    let found = matching(&d, &args, area.as_ref())?;
    let limit = num(&args, "limit", 50.0)?.clamp(0.0, 2000.0) as usize;
    let centre = area.as_ref().map(|a| a.circle().0).unwrap_or(Vec2::ZERO);
    let mut objs: Vec<&Placed> = found
        .iter()
        .filter_map(|id| d.store.items.get(id))
        .collect();
    let mut per_asset = std::collections::BTreeMap::new();
    for o in &objs {
        *per_asset.entry(o.asset.clone()).or_insert(0usize) += 1;
    }
    objs.sort_by(|a, b| {
        Vec2::new(a.pos[0], a.pos[2])
            .distance_squared(centre)
            .total_cmp(&Vec2::new(b.pos[0], b.pos[2]).distance_squared(centre))
    });
    let list: Vec<Value> = objs
        .iter()
        .take(limit)
        .map(|o| {
            let mut v = json!({ "id": o.id, "asset": o.asset, "position": o.pos.map(super::terrain::round1), "yaw_deg": yaw_of(o), "scale": scale_of(o) });
            if o.group != 0 {
                v["group"] = json!(o.group);
            }
            if o.locked {
                v["locked"] = json!(true);
            }
            v
        })
        .collect();
    json_out(json!({ "total": objs.len(), "per_asset": per_asset, "objects": list }))
}

fn lock_tool(In(args): In<Value>, mut d: WorldData) -> R {
    let locked = opt_bool(&args, "locked")?.unwrap_or(true);
    let mut ids: Vec<u64> =
        crate::mcp_tools::opt_arg::<Vec<u64>>(&args, "ids")?.unwrap_or_default();
    if let Some(g) = args.get("group").and_then(|v| v.as_u64()) {
        let m = d.store.group_members(g);
        if m.is_empty() {
            return Err(format!("{g} isn't a group (objects_query lists `group`)"));
        }
        ids.extend(m);
    }
    if ids.is_empty() {
        return Err("give `ids` or a `group`".into());
    }
    if let Some(bad) = ids.iter().find(|id| !d.store.items.contains_key(id)) {
        return Err(format!("no object with id {bad}"));
    }
    // a set's piece takes its whole set along
    let mut all: Vec<u64> = ids
        .iter()
        .flat_map(|id| crate::placement::lock::lock_targets(&d.store, *id))
        .collect();
    all.sort_unstable();
    all.dedup();
    d.history.begin(if locked { "Lock" } else { "Unlock" });
    for id in &all {
        d.history.touch_object(&d.store, *id);
    }
    d.store.set_locked(&all, locked);
    d.history.commit(&d.store);
    d.meta.dirty = true;
    json_out(json!({ "objects": all.len(), "locked": locked, "note": "one undo step" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, style: &str) -> crate::objects::LibraryEntry {
        crate::objects::LibraryEntry {
            id: id.into(),
            name: id.split(':').next_back().unwrap_or(id).replace('_', " "),
            category: Category::Structures,
            group: "buildings".into(),
            kind: None,
            import_path: None,
            surface: crate::terrain::layers::Surface::Wood,
            radius: 2.0,
            align: false,
            walkable: false,
            whole_scene: false,
            biomes: crate::world::Biome::ALL.to_vec(),
            sway: false,
            default_scale: 1.0,
            meshes: vec![],
            scene: None,
            height: 3.0,
            tree: None,
            textured: false,
            pack: style.into(),
        }
    }

    fn world() -> World {
        let mut lib = Library::empty_for_tests();
        for (id, style) in [
            ("hp:inn", "handpainted"),
            ("hp:well", "handpainted"),
            ("ph:wine_barrel_01", "photoreal"),
            ("sb:blacksmith", "photoreal"),
            ("WoodenBridge", "photoreal"),
        ] {
            lib.add(entry(id, style));
        }
        let mut w = World::new();
        w.insert_resource(lib);
        w
    }

    fn tool(name: &str) -> McpTool {
        let mut app = App::new();
        register(&mut app);
        crate::thumbs::mcp(&mut app);
        app.world()
            .resource::<crate::mcp_tools::McpTools>()
            .tools
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .expect("registered")
    }

    /// (the tests' market: the bundled catalog, nothing owned, no developer unlock)
    #[test]
    fn locked_assets_are_refused() {
        let mut w = world();
        let place = |w: &mut World, asset: &str| {
            (tool("objects_place").run)(
                w,
                json!({ "items": [{ "asset": "WoodenBridge", "position": [0, 0] }, { "asset": asset, "position": [1, 1] }] }),
            )
        };
        let e = place(&mut w, "hp:inn")
            .err()
            .expect("a locked asset is refused");
        assert!(
            e.starts_with("item 1: `hp:inn` is in the Skyfold Studio Collection pack ($49.99)")
                && e.contains("Asset packs"),
            "{e}"
        );
        // free samples, free packs (CC0: Poly Haven, Skyborn) and built-ins pass the gate (unknown
        // ids are left to the tool)
        for ok in [
            "hp:well",
            "ph:wine_barrel_01",
            "sb:blacksmith",
            "WoodenBridge",
            "hp:no_such_model",
        ] {
            assert!(
                locked_items(
                    &w,
                    &json!({ "items": [{ "asset": ok, "position": [0, 0] }] })
                )
                .is_ok(),
                "{ok}"
            );
        }
        // agents don't get to look at it either
        let e = (tool("objects_preview").run)(&mut w, json!({ "asset": "hp:inn" }))
            .err()
            .expect("refused");
        assert!(e.contains("hasn't unlocked"), "{e}");
    }

    #[test]
    fn the_library_lists_usable_assets_and_sums_up_locked_packs() {
        let w = world();
        let lib = w.resource::<Library>();
        let m = crate::market::snapshot();
        let v = library_json(lib, &m, &json!({}), None).unwrap();
        let ids: Vec<&str> = v["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "hp:well",
                "ph:wine_barrel_01",
                "sb:blacksmith",
                "WoodenBridge"
            ]
        );
        // by default only the world's art style (here: hand-painted), "all" for every style
        let v = library_json(lib, &m, &json!({}), Some("handpainted")).unwrap();
        let ids: Vec<&str> = v["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["hp:well"]);
        assert_eq!(
            library_json(lib, &m, &json!({ "style": "all" }), Some("handpainted")).unwrap()["count"],
            4
        );
        assert_eq!(
            library_json(lib, &m, &json!({}), Some("photoreal")).unwrap()["count"],
            3
        );
        let locked = v["locked_packs"].as_array().unwrap();
        let summary: Vec<(&str, &str, u64)> = locked
            .iter()
            .map(|p| {
                (
                    p["pack"].as_str().unwrap(),
                    p["price"].as_str().unwrap(),
                    p["locked_assets"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(summary, [("skyfold_studio", "$49.99", 1)]);
        // a search that only finds locked assets says where they are
        let v = library_json(lib, &m, &json!({ "search": "inn" }), None).unwrap();
        assert_eq!(v["count"], 0);
        assert_eq!(v["locked_packs"][0]["pack"], "skyfold_studio");
        // style and pack filters
        let v = library_json(lib, &m, &json!({ "style": "handpainted" }), None).unwrap();
        assert_eq!(v["assets"].as_array().unwrap().len(), 1);
        let v = library_json(lib, &m, &json!({ "pack": "polyhaven_scans" }), None).unwrap();
        assert_eq!(
            (v["count"].as_u64(), v.get("locked_packs")),
            (Some(1), None)
        );
        assert!(
            library_json(lib, &m, &json!({ "pack": "nope" }), None)
                .unwrap_err()
                .contains("market_packs")
        );
    }
}
