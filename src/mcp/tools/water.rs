//! Water tools: the ocean, rivers, lakes (drawn or basin-filled), waterfalls, fish and every
//! body's water style, through the Water tab's code (`make_river`, `create_drawn_lake`…).

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use super::*;
use crate::fish::{FishSchool, FishSpecies, Water};
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpTool, bearing_schema};
use crate::water::presets::{self, WaterCategory};
use crate::water::{
    BodyKind, Lake, ReflectionQuality, WaterBehaviour, WaterSel, WaterStyle, lakes,
};

fn preset_ids() -> Vec<String> {
    presets::presets()
        .iter()
        .map(|p| p.id.to_string())
        .collect()
}

fn shape_ids() -> Vec<String> {
    lakes::SHAPES
        .iter()
        .filter(|s| s.listed)
        .map(|s| s.id.to_string())
        .collect()
}

/// Schema properties for a new body's water: a preset, a behaviour and any style field.
fn new_water_props() -> Map<String, Value> {
    let mut m = style_props();
    m.insert(
        "preset".into(),
        enum_schema(
            preset_ids(),
            "water preset (see water_presets); the style fields fine-tune it",
        ),
    );
    m
}

/// Schema properties of a water style (all optional).
fn style_props() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "preset".into(),
        enum_schema(
            preset_ids(),
            "start from a water preset (see water_presets); other fields then fine-tune it",
        ),
    );
    m.insert(
        "behaviour".into(),
        enum_schema(
            ids(&WaterBehaviour::ALL),
            "how the surface moves (also suggests matching wave / flow values)",
        ),
    );
    m.insert(
        "shallow".into(),
        color_schema("water colour (a sandy floor `clarity` m down)"),
    );
    m.insert("deep".into(), color_schema("depth colour (deep water)"));
    for (k, lo, hi, d) in [
        ("clarity", 0.3, 30.0, "underwater visibility (m)"),
        (
            "transparency",
            0.0,
            1.0,
            "1 see-through … 0 opaque like paint",
        ),
        (
            "waves",
            0.0,
            2.5,
            "wave height: the ocean's sea state (0 glassy … 2.5 storm), ripples elsewhere",
        ),
        ("wave_speed", 0.2, 2.5, "wave animation speed"),
        (
            "flow",
            0.0,
            4.0,
            "current speed (× 1.6 m/s); rivers flow along their course",
        ),
        ("foam", 0.0, 1.0, "foam amount"),
        ("shore_foam", 0.0, 1.5, "foam where waves meet the shore"),
        ("roughness", 0.0, 1.0, "0 mirror … 1 frosted reflections"),
        ("metallic", 0.0, 1.0, "0 water … 1 liquid metal"),
        ("iridescence", 0.0, 1.0, "thin-film rainbow sheen"),
        (
            "glow_strength",
            0.0,
            2.0,
            "bioluminescence after dusk (0 off)",
        ),
        (
            "emissive_strength",
            0.0,
            1.5,
            "inner glow, day and night (0 off)",
        ),
    ] {
        m.insert(
            k.into(),
            json!({ "type": "number", "minimum": lo, "maximum": hi, "description": d }),
        );
    }
    m.insert(
        "flow_dir".into(),
        bearing_schema("direction lakes and the sea flow towards (rivers flow along their course)"),
    );
    m.insert("glow".into(), color_schema("bioluminescence colour"));
    m.insert("emissive".into(), color_schema("inner glow colour"));
    m.insert(
        "reflections".into(),
        enum_schema(
            ids(&ReflectionQuality::ALL),
            "reflection quality: Low environment fallback, Medium budgeted capture, High full cadence",
        ),
    );
    m
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "water_presets",
        description: "The water catalogue: water presets (id, name, category ocean/river/lake/swamp/fantasy, description), lake types for water_add_lake (depth profile, shore, plants, their water), behaviours and fish species.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |_, _| {
            let p: Vec<Value> = presets::presets().iter().map(|p| json!({ "id": p.id, "name": p.name, "category": p.category.name(), "about": p.blurb })).collect();
            let s: Vec<Value> = lakes::SHAPES.iter().filter(|s| s.listed).map(|s| json!({ "id": s.id, "name": s.name, "about": s.blurb, "water": s.water, "depth_m": s.depth })).collect();
            let b: Vec<Value> = WaterBehaviour::ALL.iter().map(|b| json!({ "id": snake(&format!("{b:?}")), "about": b.hint() })).collect();
            let f: Vec<Value> = FishSpecies::ALL
                .iter()
                .map(|f| {
                    let i = f.info();
                    let water = match i.water {
                        Water::Fresh => "fresh",
                        Water::Sea => "sea",
                        Water::Both => "both",
                    };
                    json!({ "id": f.id(), "name": i.name, "looks_like": i.look, "habitat": i.habitat, "water": water, "rarity": i.rarity, "length_m": i.length, "emberfall": i.emberfall })
                })
                .collect();
            json_out(json!({ "presets": p, "lake_types": s, "behaviours": b, "fish": f }))
        },
    });
    app.mcp_tool(McpTool {
        name: "water_list",
        description: "Every body of water with its id: the ocean (preset, sea level), rivers (width, depth, start / end), lakes (type, level, depth, centre), waterfalls (drop, top / foot) and fish schools. Ids are shared by all kinds.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, list, a),
    });
    app.mcp_tool(McpTool {
        name: "water_set_ocean",
        description: "Set the ocean: `sea_level` (m, −50..50; the whole coastline moves) and its look — a `preset`, a `behaviour` (storm also makes the weather stormy) and any style field. \
The ocean's style and sea level are settings (not undo steps).",
        input_schema: || {
            let mut p = style_props();
            p.insert("sea_level".into(), json!({ "type": "number", "minimum": -50, "maximum": 50, "description": "sea level (m)" }));
            json!({ "type": "object", "properties": p })
        },
        run: |w, a| run(w, set_ocean, a),
    });
    app.mcp_tool(McpTool {
        name: "water_set_style",
        description: "Change one body of water (`body`: an id from water_list, or \"ocean\"): its `preset`, `behaviour` and style fields, its `name`; a basin lake's `level`; a drawn lake's `lake_type`, `depth` and `plants` (re-carves basin and shore); a waterfall's `width`, `fall_flow` and `mist`. One undo step (ocean changes are settings).",
        input_schema: || {
            let mut p = style_props();
            p.insert("body".into(), json!({ "description": "id from water_list, or \"ocean\"", "oneOf": [{ "type": "integer" }, { "type": "string", "enum": ["ocean"] }] }));
            p.insert("name".into(), json!({ "type": "string" }));
            p.insert("level".into(), json!({ "type": "number", "description": "basin lakes: water level (m)" }));
            p.insert("lake_type".into(), enum_schema(shape_ids(), "drawn lakes: lake type (re-carves; switches to its water unless `preset` is given)"));
            p.insert("depth".into(), json!({ "type": "number", "minimum": 0.3, "maximum": 30, "description": "drawn lakes: deepest point (m)" }));
            p.insert("plants".into(), json!({ "type": "number", "minimum": 0, "maximum": 2, "description": "drawn lakes: shore plant density" }));
            p.insert("width".into(), json!({ "type": "number", "minimum": 1.5, "maximum": 40, "description": "waterfalls: width (m)" }));
            p.insert("fall_flow".into(), json!({ "type": "number", "minimum": 0.2, "maximum": 3, "description": "waterfalls: 0.2 trickle … 3 torrent" }));
            p.insert("mist".into(), json!({ "type": "number", "minimum": 0, "maximum": 1, "description": "waterfalls: spray and mist at the foot" }));
            json!({ "type": "object", "properties": p, "required": ["body"] })
        },
        run: |w, a| run(w, set_style, a),
    });
    app.mcp_tool(McpTool {
        name: "water_add_river",
        description: "Draw a river along `points` ([[x, z], …], two or more; it is smoothed and resampled). It flows from the higher end downhill, its surface never rises, and its bed is carved into the terrain. \
Defaults: 18 m wide, 4 m deep, \"mountain_stream\" water. Returns its id. One undo step.",
        input_schema: || {
            let mut p = new_water_props();
            p.insert("points".into(), points_schema("course of the river", 2));
            p.insert("width".into(), json!({ "type": "number", "minimum": 4, "maximum": 80, "default": 18 }));
            p.insert("depth".into(), json!({ "type": "number", "minimum": 0.5, "maximum": 12, "default": 4 }));
            p.insert("name".into(), json!({ "type": "string" }));
            json!({ "type": "object", "properties": p, "required": ["points"] })
        },
        run: |w, a| run(w, add_river, a),
    });
    app.mcp_tool(McpTool {
        name: "water_add_lake",
        description: "Add a lake. Either draw it: `outline` ([[x, z], …], smoothed into a natural shore) with a `lake_type` (pond, lake, marsh, mountain tarn, oasis…: decides basin depth, beach, shore ground and plants, and its water); the basin is carved and shore plants grow. \
Or fill a basin: `point` [x, z] inside a hollow and `fill_depth` m of water above the ground there (if it would spill, the error says where the rim is and the deepest fill that holds). Optional water `preset` / style fields. Returns its id. One undo step.",
        input_schema: || {
            let mut p = new_water_props();
            p.insert("outline".into(), points_schema("drawn lake: shoreline loop", 3));
            p.insert("lake_type".into(), enum_schema(shape_ids(), "drawn lake type (default pond)"));
            p.insert("depth".into(), json!({ "type": "number", "minimum": 0.3, "maximum": 2.5, "default": 1, "description": "drawn lake: × the type's depth" }));
            p.insert("plants".into(), json!({ "type": "number", "minimum": 0, "maximum": 2, "default": 1, "description": "drawn lake: shore plant density" }));
            p.insert("point".into(), xz_schema("basin lake: a point inside the hollow"));
            p.insert("fill_depth".into(), json!({ "type": "number", "minimum": 0.5, "maximum": 40, "default": 4, "description": "basin lake: water depth above the ground at `point` (m)" }));
            p.insert("name".into(), json!({ "type": "string" }));
            json!({ "type": "object", "properties": p })
        },
        run: |w, a| run(w, add_lake, a),
    });
    app.mcp_tool(McpTool {
        name: "water_add_waterfall",
        description: "Place a waterfall from `top` [x, z] (a cliff edge or a river's edge) down to `foot` [x, z] (at least ~2 m lower). The sheet follows a ballistic arc, mist rises at the foot and a plunge pool is carved unless it lands in water or `pool` is false. \
Spilling from a river it takes the river's water. Returns its id. One undo step.",
        input_schema: || {
            let mut p = new_water_props();
            p.insert("top".into(), xz_schema("where the water spills over"));
            p.insert("foot".into(), xz_schema("where it lands"));
            p.insert("width".into(), json!({ "type": "number", "minimum": 1.5, "maximum": 40, "default": 7 }));
            p.insert("fall_flow".into(), json!({ "type": "number", "minimum": 0.2, "maximum": 3, "default": 1, "description": "0.2 trickle … 3 torrent" }));
            p.insert("mist".into(), json!({ "type": "number", "minimum": 0, "maximum": 1, "default": 0.7 }));
            p.insert("pool".into(), json!({ "type": "boolean", "default": true, "description": "carve a plunge pool at the foot" }));
            json!({ "type": "object", "properties": p, "required": ["top", "foot"] })
        },
        run: |w, a| run(w, add_waterfall, a),
    });
    app.mcp_tool(McpTool {
        name: "water_update_river",
        description: "Re-route or resize a river by `id`: new course `points` and/or `width` / `depth` (the old bed is filled back in and the new one carved), a `name`, or water `preset` / style fields. One undo step.",
        input_schema: || {
            let mut p = new_water_props();
            p.insert("id".into(), json!({ "type": "integer", "description": "river id (water_list)" }));
            p.insert("points".into(), points_schema("new course of the river (default: the current one)", 2));
            p.insert("width".into(), json!({ "type": "number", "minimum": 4, "maximum": 80 }));
            p.insert("depth".into(), json!({ "type": "number", "minimum": 0.5, "maximum": 12 }));
            p.insert("name".into(), json!({ "type": "string" }));
            json!({ "type": "object", "properties": p, "required": ["id"] })
        },
        run: |w, a| run(w, update_river, a),
    });
    app.mcp_tool(McpTool {
        name: "water_remove",
        description: "Remove rivers, lakes, waterfalls or fish schools by id. The ground a river's bed or a drawn lake's basin replaced comes back (and a drawn lake's shore plants go); undo restores everything. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "integer" }, "minItems": 1 } }, "required": ["ids"] }),
        run: |w, a| run(w, remove, a),
    });
    app.mcp_tool(McpTool {
        name: "fish_add",
        description: "Release a school of fish at `position` [x, z] in water at least knee-deep (sea, lake or river). They swim in the water around it, at the depth their species likes, and are exported as a fishing spot. 27 species (water_presets lists them: fresh-water, sea and both; the Emberfall models plus salmon, cod, tuna and reef fish). Without `species`: the Fish tool's species if it is a fresh-water fish released in fresh water (or a sea fish in the sea), else trout in fresh water and mackerel in the sea. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "position": xz_schema("a point on water"),
                    "species": enum_schema(FishSpecies::ALL.iter().map(|f| f.id()).collect(), "species id (see water_presets); default: one that lives in this water"),
                    "count": { "type": "integer", "minimum": 1, "maximum": 60, "default": 12 },
                    "radius": { "type": "number", "minimum": 3, "maximum": 80, "default": 14, "description": "swim area (m); shrunk to fit the water" }
                },
                "required": ["position"]
            })
        },
        run: |w, a| run(w, add_fish, a),
    });
    app.mcp_tool(McpTool {
        name: "water_shape_lake",
        description: "Shape a drawn lake's shore with the Shore brush (like Water › Shape Lake): `mode` expand (push the shore out), shrink (pull it in) or smooth (relax kinks), brushing at `position` [x, z] on or near the shore, or along a stroke of `points` [[x, z], …]. `radius` is the brush size (m), `strength` 0.05..1 and `seconds` how long the brush scrubs there (at full strength about 0.9 × the brush size in metres per second). The basin, beach and shore plants are re-carved to the new shore. Basin lakes have no shore to shape (change their water level with water_set_style). One undo step (the old shore and ground come back).",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "drawn lake id (water_list)" },
                    "mode": enum_schema(ids(&SHAPE_MODES), "expand, shrink or smooth"),
                    "position": xz_schema("where the brush works (within 1.5 × `radius` of the shore)"),
                    "points": points_schema("instead of `position`: a stroke the brush moves along", 1),
                    "radius": { "type": "number", "minimum": 4, "maximum": 200, "default": 25, "description": "brush size (m)" },
                    "strength": { "type": "number", "minimum": 0.05, "maximum": 1, "default": 0.5 },
                    "seconds": { "type": "number", "minimum": 0.05, "maximum": 10, "default": 1, "description": "how long the brush scrubs (spread along a stroke)" }
                },
                "required": ["id", "mode"]
            })
        },
        run: |w, a| run(w, shape_lake, a),
    });
    app.mcp_tool(McpTool {
        name: "fish_set",
        description: "Change a fish school in place by `id` (water_list › fish_schools), like its card in the Water tab: `species` (see water_presets), `count` of fish and swim area `radius` (m; shrunk to fit the water around it). Exported as a fishing spot. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "fish school id (water_list)" },
                    "species": enum_schema(FishSpecies::ALL.iter().map(|f| f.id()).collect(), "species id (see water_presets)"),
                    "count": { "type": "integer", "minimum": 1, "maximum": 60 },
                    "radius": { "type": "number", "minimum": 3, "maximum": 80, "description": "swim area (m); shrunk to fit the water" }
                },
                "required": ["id"]
            })
        },
        run: |w, a| run(w, set_fish, a),
    });
}

const SHAPE_MODES: [lakes::ShapeMode; 3] = [
    lakes::ShapeMode::Expand,
    lakes::ShapeMode::Shrink,
    lakes::ShapeMode::Smooth,
];

/// Applies `preset`, `behaviour` and the style fields in `args`. Returns whether anything was set.
fn apply_style(st: &mut WaterStyle, args: &Value, body: BodyKind) -> Result<bool, String> {
    let mut any = false;
    if let Some(id) = opt_str(args, "preset")? {
        let p = presets::preset(id)
            .ok_or_else(|| format!("unknown water preset `{id}` (see water_presets)"))?;
        // keep what belongs to the body rather than the look (as the Water tab does)
        let flow_dir = st.flow_dir;
        let old_flow = st.flow;
        *st = p.style.clone();
        st.flow_dir = flow_dir;
        if body == BodyKind::River {
            st.flow = if p.category == WaterCategory::River {
                p.style.flow
            } else {
                old_flow.max(0.5)
            };
        }
        any = true;
    }
    if let Some(b) = opt_pick(
        args,
        "behaviour",
        &WaterBehaviour::ALL,
        "behaviour",
        WaterBehaviour::name,
    )? {
        if b == WaterBehaviour::Whirlpool && body == BodyKind::Ocean {
            return Err("whirlpools swirl in lakes and rivers, not the open sea".into());
        }
        st.set_behaviour(b, body);
        any = true;
    }
    let before = st.clone();
    for (k, f, lo, hi) in [
        ("clarity", &mut st.clarity, 0.3, 30.0),
        ("transparency", &mut st.transparency, 0.0, 1.0),
        ("waves", &mut st.waves, 0.0, 2.5),
        ("wave_speed", &mut st.wave_speed, 0.2, 2.5),
        ("flow", &mut st.flow, 0.0, 4.0),
        ("foam", &mut st.foam, 0.0, 1.0),
        ("shore_foam", &mut st.shore_foam, 0.0, 1.5),
        ("roughness", &mut st.roughness, 0.0, 1.0),
        ("metallic", &mut st.metallic, 0.0, 1.0),
        ("iridescence", &mut st.iridescence, 0.0, 1.0),
        ("glow_strength", &mut st.glow_strength, 0.0, 2.0),
        ("emissive_strength", &mut st.emissive_strength, 0.0, 1.5),
    ] {
        if let Some(v) = opt_num(args, k)? {
            *f = v.clamp(lo, hi);
        }
    }
    if let Some(b) = opt_num(args, "flow_dir")? {
        // (internally 0 = east, 90 = south)
        st.flow_dir = (b - 90.0 + 180.0).rem_euclid(360.0) - 180.0;
    }
    for (k, c) in [
        ("shallow", &mut st.shallow),
        ("deep", &mut st.deep),
        ("glow", &mut st.glow),
        ("emissive", &mut st.emissive),
    ] {
        if let Some(v) = opt_color(args, k)? {
            *c = v;
        }
    }
    if let Some(q) = opt_pick(
        args,
        "reflections",
        &ReflectionQuality::ALL,
        "reflection quality",
        ReflectionQuality::name,
    )? {
        st.reflections = q;
    }
    if *st != before {
        any = true;
        // edited away from its preset
        if presets::preset(&st.preset).is_none_or(|p| p.style != *st) {
            st.preset = "custom".into();
        }
    }
    Ok(any)
}

fn describe(st: &WaterStyle) -> String {
    format!(
        "{} water, {} (waves {:.2}, flow {:.2}, clarity {:.1} m)",
        st.preset_name(),
        st.behaviour.name().to_lowercase(),
        st.waves,
        st.flow,
        st.clarity
    )
}

fn set_ocean(In(args): In<Value>, mut d: WorldData) -> R {
    let mut parts = vec![];
    if let Some(l) = opt_num(&args, "sea_level")? {
        d.terrain.sea_level = l.clamp(-50.0, 50.0);
        parts.push(format!("sea level {:.1} m", d.terrain.sea_level));
    }
    let before = d.water_style.behaviour;
    let mut st = d.water_style.clone();
    if apply_style(&mut st, &args, BodyKind::Ocean)? {
        if st.behaviour == WaterBehaviour::Whirlpool {
            st.behaviour = WaterBehaviour::OceanSwells;
        }
        *d.water_style = st;
        storm_weather(&mut d, before);
        parts.push(describe(&d.water_style));
    }
    if parts.is_empty() {
        return Err(
            "nothing to change: give `sea_level`, `preset`, `behaviour` or style fields".into(),
        );
    }
    d.meta.dirty = true;
    text(format!("Ocean: {}.", parts.join("; ")))
}

/// A storm on the ocean brings stormy weather (and takes it away again), as in the Water tab.
fn storm_weather(d: &mut WorldData, before: WaterBehaviour) {
    use crate::world::Weather;
    let after = d.water_style.behaviour;
    if after == before {
        return;
    }
    if after == WaterBehaviour::Storm && d.lighting.weather_override.is_none() {
        d.lighting.weather_override = Some(Weather::Storm);
    } else if before == WaterBehaviour::Storm && d.lighting.weather_override == Some(Weather::Storm)
    {
        d.lighting.weather_override = None;
    }
}

fn body_arg(args: &Value) -> Result<Option<u32>, String> {
    match args.get("body") {
        Some(Value::String(s)) if s.eq_ignore_ascii_case("ocean") => Ok(None),
        Some(Value::String(s)) => s
            .parse::<u32>()
            .map(Some)
            .map_err(|_| "`body` is an id from water_list or \"ocean\"".into()),
        Some(v) => v
            .as_u64()
            .map(|v| Some(v as u32))
            .ok_or("`body` is an id from water_list or \"ocean\"".into()),
        None => Err("missing `body` (an id from water_list, or \"ocean\")".into()),
    }
}

fn sel_of(w: &crate::water::WaterBodies, id: u32) -> Option<WaterSel> {
    if w.rivers.iter().any(|r| r.id == id) {
        Some(WaterSel::River(id))
    } else if w.lakes.iter().any(|l| l.id == id) {
        Some(WaterSel::Lake(id))
    } else if w.waterfalls.iter().any(|f| f.id == id) {
        Some(WaterSel::Waterfall(id))
    } else if w.fish.iter().any(|f| f.id == id) {
        Some(WaterSel::Fish(id))
    } else {
        None
    }
}

fn set_style(In(args): In<Value>, mut d: WorldData) -> R {
    let Some(id) = body_arg(&args)? else {
        return set_ocean(In(args), d);
    };
    let sel = sel_of(&d.water, id)
        .ok_or_else(|| format!("no water body with id {id} (see water_list)"))?;
    let body = match sel {
        WaterSel::River(_) | WaterSel::Waterfall(_) => BodyKind::River,
        WaterSel::Lake(_) => BodyKind::Lake,
        _ => {
            return Err(format!(
                "{id} is a fish school: change its species, count and swim area with fish_set"
            ));
        }
    };
    // check everything before the undo step opens
    apply_style(&mut d.water.style_mut(sel).unwrap().clone(), &args, body)?;
    let shape = opt_str(&args, "lake_type")?
        .map(|s| {
            lakes::SHAPES.iter().find(|x| x.id == s).ok_or_else(|| {
                format!(
                    "unknown lake type `{s}` (one of {})",
                    shape_ids().join(", ")
                )
            })
        })
        .transpose()?;
    let reshape = shape.is_some()
        || opt_num(&args, "depth")?.is_some()
        || opt_num(&args, "plants")?.is_some();
    if let WaterSel::Lake(_) = sel {
        let drawn = d.water.lakes.iter().any(|l| l.id == id && l.is_drawn());
        if reshape && !drawn {
            return Err("only drawn lakes have a type, depth and plants (this is a basin lake: set its `level`)".into());
        }
        if opt_num(&args, "level")?.is_some() && drawn {
            return Err(
                "a drawn lake's level follows its shore; change its `depth` instead".into(),
            );
        }
    }
    let given = |k: &str| args.get(k).is_some_and(|v| !v.is_null());
    let body_keys = [
        "name",
        "level",
        "lake_type",
        "depth",
        "plants",
        "width",
        "fall_flow",
        "mist",
    ];
    if !body_keys.iter().any(|k| given(k)) && !style_props().keys().any(|k| given(k)) {
        return Err(
            "nothing to change: give a `preset`, `behaviour`, style fields or body settings".into(),
        );
    }
    d.history.begin("Water style");
    d.history.touch_water(&d.water);
    let mut changed = vec![];
    // drawn lakes: type, depth and plants re-carve the basin
    if let WaterSel::Lake(_) = sel {
        let depth = opt_num(&args, "depth")?;
        let plants = opt_num(&args, "plants")?;
        let lake = d.water.lakes.iter_mut().find(|l| l.id == id).unwrap();
        if reshape {
            let prev = lake.clone();
            if let Some(sh) = shape {
                lake.shape = sh.id.to_string();
                lake.depth = sh.depth;
                lake.style = presets::preset_style(sh.water);
            }
            if let Some(v) = depth {
                lake.depth = v.clamp(0.3, 30.0);
            }
            if let Some(v) = plants {
                lake.plants = v.clamp(0.0, 2.0);
            }
            let WorldData {
                terrain,
                history,
                store,
                water,
                lib,
                ..
            } = &mut d;
            crate::water::recarve_lake(terrain, history, store, lib.as_deref(), water, id, &prev);
            changed.push("re-carved".to_string());
        }
        if let Some(l) = opt_num(&args, "level")? {
            d.water.lakes.iter_mut().find(|l| l.id == id).unwrap().level = l;
            changed.push(format!("level {l:.1} m"));
        }
    }
    if let WaterSel::Waterfall(_) = sel {
        let f = d.water.waterfalls.iter_mut().find(|f| f.id == id).unwrap();
        for (k, field, lo, hi) in [
            ("width", &mut f.width, 1.5, 40.0),
            ("fall_flow", &mut f.flow, 0.2, 3.0),
            ("mist", &mut f.mist, 0.0, 1.0),
        ] {
            if let Some(v) = opt_num(&args, k)? {
                *field = v.clamp(lo, hi);
                changed.push(format!("{k} {:.1}", *field));
            }
        }
    }
    if let Some(n) = opt_str(&args, "name")? {
        match sel {
            WaterSel::River(_) => {
                d.water.rivers.iter_mut().find(|r| r.id == id).unwrap().name = n.to_string()
            }
            WaterSel::Lake(_) => {
                d.water.lakes.iter_mut().find(|r| r.id == id).unwrap().name = n.to_string()
            }
            WaterSel::Waterfall(_) => {
                d.water
                    .waterfalls
                    .iter_mut()
                    .find(|r| r.id == id)
                    .unwrap()
                    .name = n.to_string()
            }
            _ => {}
        }
        changed.push(format!("named \"{n}\""));
    }
    let st = d.water.style_mut(sel).unwrap();
    if apply_style(st, &args, body)? {
        changed.push(describe(st));
    }
    d.water.dirty = true;
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(format!(
        "Water {id}: {}. One undo step.",
        changed.join("; ")
    ))
}

fn add_river(In(args): In<Value>, mut d: WorldData) -> R {
    let pts = points(&args, "points", 2)?;
    let width = num_in(&args, "width", d.ed.river_width, 4.0, 80.0)?;
    let depth = num_in(&args, "depth", d.ed.river_depth, 0.5, 12.0)?;
    let mut style = d.ed.new_river_style.clone();
    apply_style(&mut style, &args, BodyKind::River)?;
    // the drawn stroke: dense points on the ground, as a mouse drag leaves them
    let path: Vec<Vec3> = along(&pts, 2.0)
        .into_iter()
        .filter(|p| d.terrain.in_bounds(p.x, p.y))
        .map(|p| Vec3::new(p.x, d.terrain.height_at(p.x, p.y), p.y))
        .collect();
    let length: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
    if path.len() < 4 || length < width * 2.5 {
        return Err(format!(
            "the river must lie inside the world and be at least {:.0} m long (2.5 × its width)",
            width * 2.5
        ));
    }
    d.history.begin("River");
    d.history.touch_water(&d.water);
    d.water.next_id += 1;
    let id = d.water.next_id;
    let WorldData {
        terrain,
        history,
        water,
        ..
    } = &mut d;
    let Some(mut r) = crate::water::make_river(terrain, history, &path, width, depth, id) else {
        d.history.commit(&d.store);
        return Err("that course is too short for a river this wide".into());
    };
    r.style = style;
    if let Some(n) = opt_str(&args, "name")? {
        r.name = n.to_string();
    }
    let (a, b) = (r.points[0], r.points[r.points.len() - 1]);
    let len: f32 = r
        .points
        .windows(2)
        .map(|w| Vec3::from_array(w[0]).distance(Vec3::from_array(w[1])))
        .sum();
    let mut msg = format!(
        "River {id} \"{}\": {len:.0} m long, {width:.0} m wide, {depth:.1} m deep, flowing from [{:.0}, {:.0}] (surface {:.1} m) to [{:.0}, {:.0}] (surface {:.1} m); {}",
        r.name,
        a[0],
        a[2],
        a[1],
        b[0],
        b[2],
        b[1],
        describe(&r.style)
    );
    water.rivers.push(r);
    water.selected = Some(WaterSel::River(id));
    water.dirty = true;
    // steep drops pour over waterfalls
    let falls = crate::water::river_falls(terrain, water, id);
    if falls > 0 {
        msg += &format!("; {falls} waterfall(s) where it drops steeply");
    }
    msg += ". One undo step.";
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn add_lake(In(args): In<Value>, mut d: WorldData) -> R {
    if let Some(outline) = opt_points(&args, "outline")? {
        if outline.len() < 3 {
            return Err("`outline` needs at least 3 points".into());
        }
        let shape_id = opt_str(&args, "lake_type")?.unwrap_or("pond");
        let shape = lakes::SHAPES
            .iter()
            .find(|s| s.id == shape_id)
            .ok_or_else(|| {
                format!(
                    "unknown lake type `{shape_id}` (one of {})",
                    shape_ids().join(", ")
                )
            })?;
        let depth = num_in(&args, "depth", 1.0, 0.3, 2.5)?;
        let plants = num_in(&args, "plants", 1.0, 0.0, 2.0)?;
        apply_style(
            &mut presets::preset_style(shape.water),
            &args,
            BodyKind::Lake,
        )?;
        let perimeter: f32 = outline
            .iter()
            .zip(outline.iter().cycle().skip(1))
            .map(|(a, b)| a.distance(*b))
            .sum();
        let raw = densify_closed(&outline, (perimeter / 64.0).max(1.0));
        d.history.begin("Draw lake");
        let WorldData {
            terrain,
            history,
            store,
            water,
            lib,
            ..
        } = &mut d;
        let id = crate::water::create_drawn_lake(
            terrain,
            history,
            store,
            lib.as_deref(),
            water,
            &raw,
            shape,
            depth,
            plants,
        )?;
        let lake = d.water.lakes.iter_mut().find(|l| l.id == id).unwrap();
        apply_style(&mut lake.style, &args, BodyKind::Lake)?;
        if let Some(n) = opt_str(&args, "name")? {
            lake.name = n.to_string();
        }
        let poly: Vec<Vec2> = lake.outline.iter().map(|p| Vec2::from_array(*p)).collect();
        let msg = format!(
            "Lake {id} \"{}\" ({}): {:.0} m², {:.1} m deep, water level {:.1} m, {} shore plants; {}. One undo step.",
            lake.name,
            shape.name,
            lakes::signed_area(&poly),
            lake.depth,
            lake.level,
            lake.plant_ids.len(),
            describe(&lake.style)
        );
        d.history.commit(&d.store);
        d.meta.dirty = true;
        return text(msg);
    }
    let p = opt_xz(&args, "point")?
        .ok_or("give an `outline` (drawn lake) or a `point` (basin fill)")?;
    if !d.terrain.in_bounds(p.x, p.y) {
        return Err("`point` is outside the world".into());
    }
    let fill = num_in(&args, "fill_depth", d.ed.lake_fill, 0.5, 40.0)?;
    let level = d.terrain.height_at(p.x, p.y) + fill;
    let cells = crate::water::ground::flood_explained(&d.terrain, p, level)?;
    if cells.len() <= 4 {
        return Err("that point isn't in a basin (a low area surrounded by higher ground)".into());
    }
    let mut style = d.ed.new_lake_style.clone();
    apply_style(&mut style, &args, BodyKind::Lake)?;
    d.history.begin("Fill lake");
    d.history.touch_water(&d.water);
    d.water.next_id += 1;
    let id = d.water.next_id;
    let name = opt_str(&args, "name")?
        .map(str::to_string)
        .unwrap_or(format!("Lake {id}"));
    let msg = format!(
        "Lake {id} \"{name}\" filled: {:.0} m², level {level:.1} m; {}. One undo step.",
        cells.len() as f32 * d.terrain.cell * d.terrain.cell,
        describe(&style)
    );
    d.water.lakes.push(Lake {
        id,
        name,
        seed: [p.x, p.y],
        level,
        style,
        outline: vec![],
        shape: String::new(),
        depth: 0.0,
        plants: 0.0,
        plant_ids: vec![],
        ground: None,
        contained: false,
    });
    d.water.selected = Some(WaterSel::Lake(id));
    d.water.dirty = true;
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn add_waterfall(In(args): In<Value>, mut d: WorldData) -> R {
    let a = opt_xz(&args, "top")?.ok_or("missing `top`")?;
    let b = opt_xz(&args, "foot")?.ok_or("missing `foot`")?;
    if !d.terrain.in_bounds(a.x, a.y) || !d.terrain.in_bounds(b.x, b.y) {
        return Err("`top` and `foot` must be inside the world".into());
    }
    let mut s = d.ed.water_tools.clone();
    s.fall_width = num_in(&args, "width", s.fall_width, 1.5, 40.0)?;
    s.fall_flow = num_in(&args, "fall_flow", s.fall_flow, 0.2, 3.0)?;
    s.fall_mist = num_in(&args, "mist", s.fall_mist, 0.0, 1.0)?;
    s.fall_pool = opt_bool(&args, "pool")?.unwrap_or(s.fall_pool);
    apply_style(&mut s.fall_style, &args, BodyKind::River)?;
    let a3 = Vec3::new(a.x, d.terrain.height_at(a.x, a.y), a.y);
    let b3 = Vec3::new(b.x, d.terrain.height_at(b.x, b.y), b.y);
    d.history.begin("Waterfall");
    let WorldData {
        terrain,
        history,
        store,
        water,
        lib,
        ..
    } = &mut d;
    let id =
        crate::water::create_waterfall(terrain, history, store, lib.as_deref(), water, a3, b3, &s)?;
    let f = d.water.waterfalls.iter().find(|f| f.id == id).unwrap();
    let msg = format!(
        "Waterfall {id}: {:.0} m drop, {:.1} m wide{}; {}. One undo step.",
        f.drop(),
        f.width,
        f.pool
            .map(|p| format!(", plunge pool {p}"))
            .unwrap_or_default(),
        describe(&f.style)
    );
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn remove(In(args): In<Value>, mut d: WorldData) -> R {
    let ids: Vec<u32> = crate::mcp_tools::arg(&args, "ids")?;
    d.history.begin("Remove water");
    let WorldData {
        terrain,
        history,
        store,
        water,
        ..
    } = &mut d;
    let r = crate::water::ground::remove_bodies(terrain, history, store, water, &ids);
    d.history.commit(&d.store);
    if r.done.is_empty() {
        return Err(format!(
            "no water bodies with ids {:?} (see water_list)",
            r.missing
        ));
    }
    d.meta.dirty = true;
    let mut notes = vec![];
    if r.restored > 0 {
        notes.push(format!(
            "the ground under {} restored",
            if r.restored == 1 {
                "it was".to_string()
            } else {
                format!("{} of them was", r.restored)
            }
        ));
    }
    if r.kept_beds > 0 {
        notes.push(format!(
            "{} older river bed(s) stay (drawn before rivers remembered their ground)",
            r.kept_beds
        ));
    }
    if !r.missing.is_empty() {
        notes.push(format!("not found: {:?}", r.missing));
    }
    text(format!(
        "Removed {:?}{}. One undo step.",
        r.done,
        if notes.is_empty() {
            String::new()
        } else {
            format!("; {}", notes.join("; "))
        }
    ))
}

fn update_river(In(args): In<Value>, mut d: WorldData) -> R {
    let id = crate::mcp_tools::arg::<u32>(&args, "id")?;
    let old = d
        .water
        .rivers
        .iter()
        .find(|r| r.id == id)
        .cloned()
        .ok_or_else(|| format!("no river with id {id} (see water_list)"))?;
    let pts = match opt_points(&args, "points")? {
        Some(p) if p.len() < 2 => return Err("`points` needs at least 2 points".into()),
        Some(p) => p,
        None => old.points.iter().map(|p| Vec2::new(p[0], p[2])).collect(),
    };
    let width = num_in(&args, "width", old.width, 4.0, 80.0)?;
    let depth = num_in(&args, "depth", old.depth, 0.5, 12.0)?;
    let mut style = old.style.clone();
    apply_style(&mut style, &args, BodyKind::River)?;
    let reroute =
        opt_points(&args, "points")?.is_some() || width != old.width || depth != old.depth;
    let inside: Vec<Vec2> = along(&pts, 2.0)
        .into_iter()
        .filter(|p| d.terrain.in_bounds(p.x, p.y))
        .collect();
    let length: f32 = inside.windows(2).map(|w| w[0].distance(w[1])).sum();
    if reroute && (inside.len() < 4 || length < width * 2.5) {
        return Err(format!(
            "the river must lie inside the world and be at least {:.0} m long (2.5 × its width)",
            width * 2.5
        ));
    }
    d.history.begin("Edit river");
    d.history.touch_water(&d.water);
    let i = d.water.rivers.iter().position(|r| r.id == id).unwrap();
    let mut notes = vec![];
    if reroute {
        let WorldData {
            terrain,
            history,
            water,
            ..
        } = &mut d;
        let restored = crate::water::ground::restore_river(terrain, history, &old);
        // the stroke on the restored ground, as a mouse drag leaves it
        let path: Vec<Vec3> = inside
            .iter()
            .map(|p| Vec3::new(p.x, terrain.height_at(p.x, p.y), p.y))
            .collect();
        let Some(mut r) = crate::water::make_river(terrain, history, &path, width, depth, id)
        else {
            // nothing usable: put everything back (inside a batch, the batch decides)
            d.history.commit(&d.store);
            let WorldData {
                history,
                terrain,
                store,
                water,
                zones,
                gameplay,
                roads,
                ..
            } = &mut d;
            if !history.in_group()
                && history
                    .step(false, terrain, store, water, zones, gameplay, roads)
                    .is_some()
            {
                history.drop_redo();
            }
            return Err("that course is too short for a river this wide".into());
        };
        r.name = old.name.clone();
        r.style = style.clone();
        water.rivers[i] = r;
        notes.push(if restored {
            "old bed filled back in, new bed carved".to_string()
        } else {
            "new bed carved (the old bed stays: it was drawn before rivers remembered their ground)"
                .to_string()
        });
    }
    let r = &mut d.water.rivers[i];
    r.style = style;
    if let Some(n) = opt_str(&args, "name")? {
        r.name = n.to_string();
    }
    let (a, b) = (r.points[0], r.points[r.points.len() - 1]);
    let len: f32 = r
        .points
        .windows(2)
        .map(|w| Vec3::from_array(w[0]).distance(Vec3::from_array(w[1])))
        .sum();
    let msg = format!(
        "River {id} \"{}\": {len:.0} m long, {:.0} m wide, {:.1} m deep, flowing from [{:.0}, {:.0}] to [{:.0}, {:.0}]; {}{}. One undo step.",
        r.name,
        r.width,
        r.depth,
        a[0],
        a[2],
        b[0],
        b[2],
        describe(&r.style),
        if notes.is_empty() {
            String::new()
        } else {
            format!("; {}", notes.join("; "))
        }
    );
    d.water.selected = Some(WaterSel::River(id));
    d.water.dirty = true;
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn add_fish(In(args): In<Value>, mut d: WorldData) -> R {
    let p = opt_xz(&args, "position")?.ok_or("missing `position`")?;
    if !d.terrain.in_bounds(p.x, p.y) {
        return Err("`position` is outside the world".into());
    }
    let fresh = crate::fish::is_fresh(&d.water, &d.terrain, p);
    let species = match opt_str(&args, "species")? {
        Some(s) => FishSpecies::parse(s).ok_or_else(|| {
            format!(
                "unknown fish species `{s}`; one of: {}",
                FishSpecies::ALL.map(|f| f.id()).join(", ")
            )
        })?,
        // the Fish tool's species when it belongs in exactly this kind of water
        None if d.ed.fish_species.info().water == if fresh { Water::Fresh } else { Water::Sea } => {
            d.ed.fish_species
        }
        None => FishSpecies::default_for(fresh),
    };
    let count = num_in(&args, "count", d.ed.fish_count as f32, 1.0, 60.0)?.round() as u32;
    let radius = num_in(&args, "radius", d.ed.fish_radius, 3.0, 80.0)?;
    let ground = Vec3::new(p.x, d.terrain.height_at(p.x, p.y), p.y);
    let surface = d.water.surface_at(ground, d.terrain.sea_level);
    if surface - ground.y < 0.8 {
        return Err(format!(
            "the water at [{:.0}, {:.0}] is {:.1} m deep: fish need at least 0.8 m",
            p.x,
            p.y,
            (surface - ground.y).max(0.0)
        ));
    }
    d.history.begin("Release fish");
    d.history.touch_water(&d.water);
    d.water.next_id += 1;
    let id = d.water.next_id;
    let school = FishSchool::new(
        id,
        species,
        Vec3::new(p.x, surface, p.y),
        count,
        radius,
        &d.terrain,
        &d.water,
    );
    let hint = crate::fish::habitat_hint(species, fresh)
        .map(|h| format!(" Note: {h}."))
        .unwrap_or_default();
    let msg = format!(
        "Fish school {id}: {} {} swimming within {:.0} m of [{:.0}, {:.0}] ({:.1} m deep, {} water). One undo step.{hint}",
        school.count,
        species.name(),
        school.radius,
        p.x,
        p.y,
        surface - ground.y,
        if fresh { "fresh" } else { "sea" }
    );
    d.water.fish.push(school);
    d.water.selected = Some(WaterSel::Fish(id));
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn shape_lake(In(args): In<Value>, mut d: WorldData) -> R {
    let id = crate::mcp_tools::arg::<u32>(&args, "id")?;
    let lake = d
        .water
        .lakes
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| format!("no lake with id {id} (see water_list)"))?;
    if !lake.is_drawn() {
        return Err(format!(
            "lake {id} is a basin lake: only drawn lakes have a shore to shape (set its `level` with water_set_style)"
        ));
    }
    let mode = pick(
        &SHAPE_MODES,
        opt_str(&args, "mode")?.ok_or("missing `mode` (expand, shrink or smooth)")?,
        "mode",
        |m| match m {
            lakes::ShapeMode::Expand => "Expand",
            lakes::ShapeMode::Shrink => "Shrink",
            lakes::ShapeMode::Smooth => "Smooth",
        },
    )?;
    let brush = d.ed.water_tools.brush;
    let radius = num_in(&args, "radius", brush.size, 4.0, 200.0)?;
    let strength = num_in(&args, "strength", brush.strength, 0.05, 1.0)?;
    let seconds = num_in(&args, "seconds", 1.0, 0.05, 10.0)?;
    let stroke: Vec<Vec2> = match (opt_points(&args, "points")?, opt_xz(&args, "position")?) {
        (Some(p), _) if p.is_empty() => return Err("`points` needs at least 1 point".into()),
        (Some(p), _) if p.len() == 1 => p,
        (Some(p), _) => along(&p, (radius * 0.25).max(1.0)),
        (None, Some(p)) => vec![p],
        (None, None) => return Err("give a `position` [x, z] or a stroke of `points`".into()),
    };
    // the brush must touch the shore somewhere along the stroke (as the brush picks its lake)
    let poly: Vec<Vec2> = lake.outline.iter().map(|q| Vec2::from_array(*q)).collect();
    let area_before = lakes::signed_area(&poly).abs();
    let near = stroke
        .iter()
        .map(|p| lakes::sdf(&poly, *p).abs())
        .fold(f32::MAX, f32::min);
    if near >= radius * 1.5 {
        return Err(format!(
            "the brush is {near:.0} m from lake {id}'s shore: brush within {:.0} m of it (1.5 × `radius`), or make `radius` bigger",
            radius * 1.5
        ));
    }
    // frames of the Shore brush: 0.05 s each, spread evenly along the stroke
    let speed = (radius * 0.9).max(12.0);
    let frames = ((seconds / 0.05).round() as usize).max(1);
    let amount = strength * speed * seconds / frames as f32;
    d.history.begin("Shape lake");
    d.history.touch_water(&d.water);
    let prev = lake.clone();
    {
        let l = d.water.lakes.iter_mut().find(|l| l.id == id).unwrap();
        for f in 0..frames {
            let at = stroke[(f * stroke.len() / frames).min(stroke.len() - 1)];
            lakes::deform(&mut l.outline, at, radius, amount, mode);
        }
    }
    let WorldData {
        terrain,
        history,
        store,
        water,
        lib,
        ..
    } = &mut d;
    crate::water::recarve_lake(terrain, history, store, lib.as_deref(), water, id, &prev);
    water.selected = Some(WaterSel::Lake(id));
    water.dirty = true;
    let l = water.lakes.iter().find(|l| l.id == id).unwrap();
    let poly: Vec<Vec2> = l.outline.iter().map(|q| Vec2::from_array(*q)).collect();
    let msg = format!(
        "Lake {id} \"{}\": shore {} ({:.0} m brush, strength {strength:.2}, {seconds:.2} s); {:.0} m² → {:.0} m², basin and shore re-carved. One undo step.",
        l.name,
        match mode {
            lakes::ShapeMode::Expand => "expanded",
            lakes::ShapeMode::Shrink => "shrunk",
            lakes::ShapeMode::Smooth => "smoothed",
        },
        radius,
        area_before,
        lakes::signed_area(&poly).abs(),
    );
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn set_fish(In(args): In<Value>, mut d: WorldData) -> R {
    let id = crate::mcp_tools::arg::<u32>(&args, "id")?;
    let old = d
        .water
        .fish
        .iter()
        .find(|f| f.id == id)
        .cloned()
        .ok_or_else(|| format!("no fish school with id {id} (see water_list)"))?;
    let species = match opt_str(&args, "species")? {
        Some(s) => FishSpecies::parse(s).ok_or_else(|| {
            format!(
                "unknown fish species `{s}`; one of: {}",
                FishSpecies::ALL.map(|f| f.id()).join(", ")
            )
        })?,
        None => old.species,
    };
    let count = num_in(&args, "count", old.count as f32, 1.0, 60.0)?.round() as u32;
    let radius = opt_num(&args, "radius")?.map(|r| r.clamp(3.0, 80.0));
    if opt_str(&args, "species")?.is_none() && opt_num(&args, "count")?.is_none() && radius.is_none() {
        return Err("nothing to change: give `species`, `count` and/or `radius`".into());
    }
    let center = Vec3::from_array(old.center);
    let fresh = crate::fish::is_fresh(&d.water, &d.terrain, center.xz());
    d.history.begin("Edit fish school");
    d.history.touch_water(&d.water);
    // a new swim area is fitted to the water round the school, as fish_add does
    let fitted = radius.map(|r| {
        FishSchool::new(id, species, center, count, r, &d.terrain, &d.water).radius
    });
    let f = d.water.fish.iter_mut().find(|f| f.id == id).unwrap();
    f.species = species;
    f.count = count;
    if let Some(r) = fitted {
        f.radius = r;
    }
    let hint = crate::fish::habitat_hint(species, fresh)
        .map(|h| format!(" Note: {h}."))
        .unwrap_or_default();
    let msg = format!(
        "Fish school {id}: {} {} swimming within {:.0} m of [{:.0}, {:.0}]. One undo step.{hint}",
        f.count,
        species.name(),
        f.radius,
        f.center[0],
        f.center[2]
    );
    d.water.selected = Some(WaterSel::Fish(id));
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(msg)
}

fn list(In(_): In<Value>, d: WorldData) -> R {
    let r1 = super::terrain::round1;
    let w = &*d.water;
    let rivers: Vec<Value> = w
        .rivers
        .iter()
        .map(|r| {
            let (a, b) = (r.points.first().copied().unwrap_or_default(), r.points.last().copied().unwrap_or_default());
            json!({ "id": r.id, "name": r.name, "preset": r.style.preset, "width": r1(r.width), "depth": r1(r.depth), "from": [a[0].round(), a[2].round()], "to": [b[0].round(), b[2].round()], "points": r.points.len() })
        })
        .collect();
    let lakes: Vec<Value> = w
        .lakes
        .iter()
        .map(|l| {
            let kind = if l.is_drawn() { lakes::shape_by_id(&l.shape).id } else { "basin" };
            let area = l.is_drawn().then(|| {
                let poly: Vec<Vec2> = l.outline.iter().map(|p| Vec2::from_array(*p)).collect();
                crate::water::lakes::signed_area(&poly).abs().round()
            });
            json!({ "id": l.id, "name": l.name, "type": kind, "preset": l.style.preset, "level": r1(l.level), "depth": r1(l.depth), "centre": [l.seed[0].round(), l.seed[1].round()], "area_m2": area })
        })
        .collect();
    let falls: Vec<Value> = w
        .waterfalls
        .iter()
        .map(|f| json!({ "id": f.id, "name": f.name, "preset": f.style.preset, "drop": r1(f.drop()), "width": r1(f.width), "top": [f.top().x.round(), f.top().z.round()], "foot": [f.bottom().x.round(), f.bottom().z.round()], "pool": f.pool }))
        .collect();
    let fish: Vec<Value> = w
        .fish
        .iter()
        .map(|f| json!({ "id": f.id, "species": snake(&format!("{:?}", f.species)), "count": f.count, "centre": [f.center[0].round(), f.center[2].round()], "radius": r1(f.radius) }))
        .collect();
    json_out(json!({
        "ocean": { "preset": d.water_style.preset, "behaviour": snake(&format!("{:?}", d.water_style.behaviour)), "waves": d.water_style.waves, "sea_level": d.terrain.sea_level },
        "rivers": rivers, "lakes": lakes, "waterfalls": falls, "fish_schools": fish,
    }))
}
