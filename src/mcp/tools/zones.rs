//! Zone tools: list, create (with a painted area), update (rules, area, biome) and delete.
//! Zones are the crafter's gameplay regions: agents create them only when asked.

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use super::*;
use crate::biomes::BiomePreset;
use crate::haze::HazeMode;
use crate::io::WorldData;
use crate::look::LookPreset;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::world::{Biome, Mood, Soundscape, Weather, Zone, ZoneLight, ZoneType};

const NAMES: [&str; 12] = [
    "Starfall Basin",
    "Sunken Steps",
    "Bridgehaven",
    "Bloomwood",
    "Broken Reach",
    "Mirrorwater",
    "Upturned Wilds",
    "Fallen Spire",
    "Ashen Vale",
    "Frostmere",
    "Gloomfen",
    "Highwatch",
];

fn nullable(values: Vec<String>, desc: &str) -> Value {
    json!({ "anyOf": [{ "type": "string", "enum": values }, { "type": "null" }], "description": format!("{desc} (null turns the rule off)") })
}

/// Schema properties of a zone's settings.
fn zone_props() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("name".into(), json!({ "type": "string" }));
    m.insert("color".into(), color_schema("map colour"));
    m.insert(
        "description".into(),
        json!({ "type": "string", "description": "what players find here" }),
    );
    m.insert(
        "level_min".into(),
        json!({ "type": "integer", "minimum": 1, "maximum": 200 }),
    );
    m.insert(
        "level_max".into(),
        json!({ "type": "integer", "minimum": 1, "maximum": 200 }),
    );
    m.insert("zone_type".into(), enum_schema(ids(&ZoneType::ALL), "open_world (PvE), sanctuary (no combat), contested (PvP), city, dungeon (entrance), raid"));
    m.insert(
        "weather".into(),
        nullable(ids(&Weather::ALL), "weather inside the zone"),
    );
    m.insert(
        "weather_intensity".into(),
        json!({ "type": "number", "minimum": 0.1, "maximum": 1 }),
    );
    m.insert(
        "ambience".into(),
        nullable(ids(&Soundscape::ALL), "ambient soundscape"),
    );
    m.insert(
        "ambience_volume".into(),
        json!({ "type": "number", "minimum": 0, "maximum": 1 }),
    );
    m.insert(
        "mood".into(),
        nullable(ids(&Mood::ALL), "the zone's own lighting mood: bright (open, safe), vibrant (alive), warm, cool, misty, gloomy, dark (foreboding), eerie, infernal. It blends in as the viewer enters the zone and out as they leave. Picking one resets `lighting` to it; the rule covers the zone's whole lighting & style"),
    );
    m.insert("style".into(), nullable(ids(&LookPreset::ALL), "art style the zone switches to while the viewer is inside (its colours, light and haze; off: the world's own style). Turns the zone's lighting & style on"));
    m.insert("style_strength".into(), json!({ "type": "number", "minimum": 0, "maximum": 1, "description": "how fully `style` replaces the world's style" }));
    m.insert("haze".into(), json!({
        "description": "Outdoor haze override for this painted zone. `inherit` follows the world switch; `on` uses this zone's saved amount even when world haze is off; `off` removes outdoor haze here. Null returns to the world setting.",
        "anyOf": [
            { "type": "null" },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "mode": { "type": "string", "enum": ["inherit", "on", "off"] },
                    "amount": { "type": "number", "minimum": 0, "maximum": 1, "description": "used when mode is on; kept while mode is inherit or off" }
                }
            }
        ]
    }));
    m.insert("lighting".into(), lighting_schema());
    m.insert("forest_diversity".into(), json!({
        "description": "Forest diversity override independent of lighting mood. Null inherits the world; pass an object with enabled false to keep legacy selection or enabled true to select this layout.",
        "anyOf": [{ "type": "null" }, { "type": "object", "additionalProperties": false, "required": ["enabled"], "properties": {
            "enabled": { "type": "boolean" },
            "layout": { "type": "string", "enum": ["natural_groves", "mixed"] }
        }}]
    }));
    m.insert(
        "biome_filter".into(),
        nullable(
            ids(&Biome::ALL),
            "nature brushes only place species of this biome here",
        ),
    );
    m.insert("wind_speed".into(), json!({ "description": "override the world's wind (m/s, 0..25; null: off)", "anyOf": [{ "type": "number" }, { "type": "null" }] }));
    m.insert("cloud_cover".into(), json!({ "description": "override the cloud cover (0..1; null: off)", "anyOf": [{ "type": "number" }, { "type": "null" }] }));
    m.insert("temperature".into(), json!({ "type": "number", "minimum": -40, "maximum": 55, "description": "typical °C (gameplay, exported)" }));
    for (k, d) in [
        ("limit_placement", "nature brushes don't place objects here"),
        ("allow_mounts", "mounts allowed"),
        ("allow_flying", "flying allowed"),
        ("rested_area", "rested area (inns, safe camps)"),
        ("visible", "shown in the editor"),
    ] {
        m.insert(k.into(), json!({ "type": "boolean", "description": d }));
    }
    m
}

/// Schema of the fine-tuned zone light (`ZoneLight`).
fn lighting_schema() -> Value {
    let n = |lo: f32, hi: f32, d: &str| json!({ "type": "number", "minimum": lo, "maximum": hi, "description": d });
    json!({
        "description": "Fine-tune the zone's light on top of its mood (any subset; unset values keep the mood's). Turns the zone's lighting & style on; null resets it to the mood as designed.",
        "anyOf": [{ "type": "null" }, {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "brightness": n(-3.0, 2.0, "exposure offset in stops: − darker, + brighter (0 = unchanged)"),
                "contrast": n(0.6, 1.5, "contrast multiplier (1 = unchanged)"),
                "saturation": n(0.0, 2.0, "colour multiplier: < 1 muted, > 1 vivid"),
                "warmth": n(-1.0, 1.0, "cool (−) / warm (+)"),
                "tint": n(-1.0, 1.0, "green (−) / magenta (+)"),
                "sun": n(0.0, 2.0, "sunlight multiplier"),
                "shadow_fill": n(0.2, 2.5, "sky light in the shadows: < 1 deeper, > 1 softer"),
                "fog": n(0.0, 1.0, "extra haze and fog (0.1 light haze, 0.45 misty, 1 thick fog)"),
                "fog_tint": { "type": "array", "items": { "type": "number", "minimum": 0, "maximum": 1.5 }, "minItems": 3, "maxItems": 3, "description": "fog colour multiplier [r, g, b] (1 = the sky's own colour)" },
                "glow": n(0.0, 1.0, "added bloom"),
                "vignette": n(0.0, 1.0, "darkened corners"),
            }
        }]
    })
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "zones_list",
        description: "All zones with every setting (levels, type, weather, ambience, haze override, lighting mood / art style / fine-tuned light, biome, wind, clouds, temperature, rules), their painted area (km²) and label position.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, list, a),
    });
    app.mcp_tool(McpTool {
        name: "zones_create",
        description: "Create a zone (a gameplay region: name, level range, type, weather, ambience, haze override, lighting mood and art style…) and paint its `area` on the map. Zones belong to the user: only create them when asked. Up to 63 zones. Returns its id. One undo step.",
        input_schema: || {
            let mut p = zone_props();
            p.insert("area".into(), area_schema("The zone's ground (painted; later zones paint over earlier ones)."));
            json!({ "type": "object", "properties": p })
        },
        run: |w, a| run(w, create, a),
    });
    app.mcp_tool(McpTool {
        name: "zones_update",
        description: "Change a zone by `id`: any setting (e.g. its own lighting & style: `mood`, `style`, `lighting` with brightness, contrast, colour, warmth, haze & fog…), `paint` more ground into it, `erase` ground from it (an area), or `biome` to make the whole zone a biome (ground, plants, weather, ambience, mood and water follow; applied in the next frame). One undo step.",
        input_schema: || {
            let mut p = zone_props();
            p.insert("id".into(), json!({ "type": "integer" }));
            p.insert("paint".into(), area_schema("Add this ground to the zone."));
            p.insert("erase".into(), area_schema("Remove this ground from the zone."));
            p.insert("biome".into(), enum_schema(ids(&BiomePreset::ALL), "apply a biome to the whole painted zone"));
            json!({ "type": "object", "properties": p, "required": ["id"] })
        },
        run: |w, a| run(w, update, a),
    });
    app.mcp_tool(McpTool {
        name: "zones_delete",
        description: "Delete a zone and clear its painted ground. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
        run: |w, a| run(w, delete, a),
    });
}

/// Applies the settings in `args`; returns what changed.
fn apply_fields(z: &mut Zone, args: &Value) -> Result<Vec<String>, String> {
    let mut out = vec![];
    if let Some(n) = opt_str(args, "name")? {
        z.name = n.to_string();
        out.push(format!("name \"{n}\""));
    }
    if let Some(c) = opt_color(args, "color")? {
        z.color = c;
        out.push("colour".into());
    }
    if let Some(s) = opt_str(args, "description")? {
        z.description = s.to_string();
        out.push("description".into());
    }
    if let Some(v) = opt_num(args, "level_min")? {
        z.level_min = v.round().clamp(1.0, 200.0) as u32;
        z.level_max = z.level_max.max(z.level_min);
    }
    if let Some(v) = opt_num(args, "level_max")? {
        z.level_max = (v.round().clamp(1.0, 200.0) as u32).max(z.level_min);
    }
    if args.get("level_min").is_some() || args.get("level_max").is_some() {
        out.push(z.level_label());
    }
    if let Some(t) = opt_pick(
        args,
        "zone_type",
        &ZoneType::ALL,
        "zone type",
        ZoneType::name,
    )? {
        z.zone_type = t;
        out.push(t.name().into());
    }
    // rules that can be switched off with null
    if let Some(v) = args.get("weather") {
        z.weather_enabled = !v.is_null();
        if let Some(s) = v.as_str() {
            z.weather = pick(&Weather::ALL, s, "weather", Weather::name)?;
        }
        out.push(if z.weather_enabled {
            format!("weather {}", z.weather.name())
        } else {
            "weather off".into()
        });
    }
    if let Some(v) = opt_num(args, "weather_intensity")? {
        z.weather_intensity = v.clamp(0.1, 1.0);
    }
    if let Some(v) = args.get("ambience") {
        z.audio_enabled = !v.is_null();
        if let Some(s) = v.as_str() {
            z.soundscape = pick(&Soundscape::ALL, s, "ambience", Soundscape::name)?;
        }
        out.push(if z.audio_enabled {
            format!("ambience {}", z.soundscape.name())
        } else {
            "ambience off".into()
        });
    }
    if let Some(v) = opt_num(args, "ambience_volume")? {
        z.audio_volume = v.clamp(0.0, 1.0);
    }
    if let Some(v) = args.get("mood") {
        z.mood_enabled = !v.is_null();
        if let Some(s) = v.as_str() {
            z.mood = pick(&Mood::ALL, s, "mood", Mood::name)?;
            z.light = None;
        }
        out.push(if z.mood_enabled {
            format!("mood {}", z.mood.name())
        } else {
            "lighting & style off".into()
        });
    }
    if let Some(v) = args.get("style") {
        z.style = match v.as_str() {
            Some(s) => Some(pick(&LookPreset::ALL, s, "art style", LookPreset::name)?),
            None if v.is_null() => None,
            None => return Err("`style` must be an art style id or null".into()),
        };
        if z.style.is_some() {
            z.mood_enabled = true;
        }
        out.push(z.style.map_or("the world's art style".into(), |p| {
            format!("art style {}", p.name())
        }));
    }
    if let Some(v) = opt_num(args, "style_strength")? {
        z.style_strength = v.clamp(0.0, 1.0);
        out.push(format!("style strength {:.0}%", z.style_strength * 100.0));
    }
    if let Some(v) = args.get("haze") {
        match v {
            Value::Null => {
                z.haze.mode = HazeMode::Inherit;
                out.push("haze uses world setting".into());
            }
            Value::Object(values) => {
                for key in values.keys() {
                    if key != "mode" && key != "amount" {
                        return Err(format!("unknown haze value `{key}`; known: mode, amount"));
                    }
                }
                if let Some(mode) = values.get("mode") {
                    let Some(mode) = mode.as_str() else {
                        return Err("haze `mode` must be inherit, on, or off".into());
                    };
                    z.haze.mode = match mode {
                        "inherit" => HazeMode::Inherit,
                        "on" => HazeMode::On,
                        "off" => HazeMode::Off,
                        _ => return Err("haze `mode` must be inherit, on, or off".into()),
                    };
                }
                if let Some(amount) = values.get("amount") {
                    let Some(amount) = amount.as_f64() else {
                        return Err("haze `amount` must be a number from 0 to 1".into());
                    };
                    z.haze.amount = (amount as f32).clamp(0.0, 1.0);
                }
                out.push(format!(
                    "haze {} ({:.0}%)",
                    match z.haze.mode {
                        HazeMode::Inherit => "uses world setting",
                        HazeMode::On => "on",
                        HazeMode::Off => "off",
                    },
                    z.haze.amount * 100.0
                ));
            }
            _ => return Err("`haze` must be an object or null".into()),
        }
    }
    if let Some(v) = args.get("lighting") {
        match v {
            Value::Null => {
                z.light = None;
                out.push(format!("lighting reset to the {} mood", z.mood.name()));
            }
            Value::Object(o) => {
                let mut cur = serde_json::to_value(z.light.unwrap_or_else(|| z.mood.light()))
                    .map_err(|e| e.to_string())?;
                let known: Vec<String> = cur
                    .as_object()
                    .map(|c| c.keys().cloned().collect())
                    .unwrap_or_default();
                for (k, val) in o {
                    if !known.contains(k) {
                        return Err(format!(
                            "unknown lighting value `{k}`; known: {}",
                            known.join(", ")
                        ));
                    }
                    let ok = if k == "fog_tint" {
                        val.as_array()
                            .is_some_and(|a| a.len() == 3 && a.iter().all(|c| c.is_number()))
                    } else {
                        val.is_number()
                    };
                    if !ok {
                        return Err(format!(
                            "lighting `{k}` must be {}",
                            if k == "fog_tint" {
                                "[r, g, b] numbers"
                            } else {
                                "a number"
                            }
                        ));
                    }
                    cur[k] = val.clone();
                }
                let l: ZoneLight =
                    serde_json::from_value(cur).map_err(|e| format!("lighting: {e}"))?;
                z.light = Some(l.clamped());
                z.mood_enabled = true;
                out.push(format!(
                    "lighting fine-tuned ({})",
                    o.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            _ => return Err("`lighting` must be an object of values or null".into()),
        }
    }
    if let Some(v) = args.get("biome_filter") {
        z.biome_enabled = !v.is_null();
        if let Some(s) = v.as_str() {
            z.biome = pick(&Biome::ALL, s, "biome", Biome::name)?;
        }
        out.push(if z.biome_enabled {
            format!("biome filter {}", z.biome.name())
        } else {
            "biome filter off".into()
        });
    }
    if let Some(v) = args.get("forest_diversity") {
        z.forest = match v {
            Value::Null => None,
            Value::Object(o) => {
                let enabled = o
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or("forest_diversity.enabled must be a boolean")?;
                let layout = match o.get("layout").and_then(Value::as_str) {
                    None => z.forest.map(|f| f.layout).unwrap_or_default(),
                    Some("natural_groves") => crate::nature::ForestLayout::NaturalGroves,
                    Some("mixed") => crate::nature::ForestLayout::Mixed,
                    Some(_) => {
                        return Err(
                            "forest_diversity.layout must be natural_groves or mixed".into()
                        );
                    }
                };
                Some(crate::nature::ForestDiversity { enabled, layout })
            }
            _ => return Err("forest_diversity must be an object or null".into()),
        };
        out.push(match z.forest {
            None => "forest diversity inherits world".into(),
            Some(f) => format!(
                "forest diversity {} ({})",
                if f.enabled { "on" } else { "off" },
                f.layout.name()
            ),
        });
    }
    if let Some(v) = args.get("wind_speed") {
        z.wind_enabled = !v.is_null();
        if let Some(s) = v.as_f64() {
            z.wind_speed = (s as f32).clamp(0.0, 25.0);
        }
        out.push(if z.wind_enabled {
            format!("wind {:.1} m/s", z.wind_speed)
        } else {
            "wind override off".into()
        });
    }
    if let Some(v) = args.get("cloud_cover") {
        z.clouds_enabled = !v.is_null();
        if let Some(s) = v.as_f64() {
            z.cloud_cover = (s as f32).clamp(0.0, 1.0);
        }
        out.push(if z.clouds_enabled {
            format!("cloud cover {:.0}%", z.cloud_cover * 100.0)
        } else {
            "cloud override off".into()
        });
    }
    if let Some(v) = opt_num(args, "temperature")? {
        z.temperature = v.clamp(-40.0, 55.0);
        out.push(format!("{:.0} °C", z.temperature));
    }
    for (k, f) in [
        ("limit_placement", &mut z.limit_placement),
        ("allow_mounts", &mut z.allow_mounts),
        ("allow_flying", &mut z.allow_flying),
        ("rested_area", &mut z.rested_area),
        ("visible", &mut z.visible),
    ] {
        if let Some(b) = opt_bool(args, k)? {
            *f = b;
            out.push(format!("{k} {b}"));
        }
    }
    Ok(out)
}

/// The grid rect an area covers (an error if it lies outside the world).
fn area_rect(t: &crate::terrain::Terrain, area: &Area) -> Result<crate::terrain::GridRect, String> {
    let (lo, hi) = area.bounds();
    t.rect_for_circle((lo + hi) * 0.5, (hi - lo).max_element() * 0.5 + t.cell)
        .ok_or_else(|| "the area lies outside the world".into())
}

/// Paints zone `id` (or 0 to erase, only over `only`) wherever `area` covers the ground.
fn paint_area(d: &mut WorldData, area: &Area, id: u8, only: Option<u8>) -> Result<usize, String> {
    let rect = area_rect(&d.terrain, area)?;
    let t = &mut d.terrain;
    d.history.touch_terrain(t, rect);
    let mut n = 0;
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            if area.contains(t.grid_to_world(i, j)) {
                let k = t.idx(i, j);
                if only.is_none_or(|z| t.zones[k] == z) && t.zones[k] != id {
                    t.zones[k] = id;
                    n += 1;
                }
            }
        }
    }
    t.mark_zone_dirty(rect);
    Ok(n)
}

fn km2(d: &WorldData, n: usize) -> f32 {
    (n as f32 * d.terrain.cell * d.terrain.cell / 1.0e4).round() / 100.0
}

fn create(In(args): In<Value>, mut d: WorldData) -> R {
    let id = d.zones.next_id().ok_or("zone limit reached (63)")?;
    let area = args
        .get("area")
        .filter(|a| !a.is_null())
        .map(Area::parse)
        .transpose()?;
    let n = d.zones.list.len();
    let mut z = Zone::new(
        id,
        NAMES
            .get(n)
            .map(|s| s.to_string())
            .unwrap_or(format!("Zone {id}")),
    );
    z.level_min = 1 + n as u32 * 5;
    z.level_max = 5 + n as u32 * 5;
    let changed = apply_fields(&mut z, &args)?;
    if let Some(a) = &area {
        area_rect(&d.terrain, a)?;
    }
    d.history.begin("Add zone");
    d.history.touch_zones(&d.zones);
    let name = z.name.clone();
    let levels = z.level_label();
    d.zones.list.push(z);
    d.zones.selected = Some(d.zones.list.len() - 1);
    let painted = match &area {
        Some(a) => paint_area(&mut d, a, id, None)?,
        None => 0,
    };
    d.history.commit(&d.store);
    d.meta.dirty = true;
    let area_note = if painted > 0 {
        format!("painted {:.2} km²", km2(&d, painted))
    } else {
        "no ground painted yet (zones_update › paint)".into()
    };
    let extra = if changed.is_empty() {
        String::new()
    } else {
        format!(" ({})", changed.join(", "))
    };
    text(format!(
        "Zone {id} \"{name}\" {levels}{extra}: {area_note}. One undo step."
    ))
}

fn zone_index(d: &WorldData, args: &Value) -> Result<usize, String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_u64())
        .ok_or("missing `id`")?;
    d.zones
        .list
        .iter()
        .position(|z| z.id as u64 == id)
        .ok_or_else(|| format!("no zone with id {id} (see zones_list)"))
}

fn update(
    In(args): In<Value>,
    mut d: WorldData,
    mut biome_msgs: MessageWriter<crate::biomes::ApplyBiomeToZone>,
) -> R {
    let i = zone_index(&d, &args)?;
    let id = d.zones.list[i].id;
    let paint = args
        .get("paint")
        .filter(|a| !a.is_null())
        .map(Area::parse)
        .transpose()?;
    let erase = args
        .get("erase")
        .filter(|a| !a.is_null())
        .map(Area::parse)
        .transpose()?;
    let biome = opt_pick(&args, "biome", &BiomePreset::ALL, "biome", |b| b.def().name)?;
    // check the settings on a copy first
    let mut z = d.zones.list[i].clone();
    let mut changed = apply_fields(&mut z, &args)?;
    if changed.is_empty() && paint.is_none() && erase.is_none() && biome.is_none() {
        return Err("nothing to change".into());
    }
    for a in paint.iter().chain(erase.iter()) {
        area_rect(&d.terrain, a)?;
    }
    d.history.begin("Edit zone");
    d.history.touch_zones(&d.zones);
    d.zones.list[i] = z;
    if let Some(a) = &paint {
        let n = paint_area(&mut d, a, id, None)?;
        changed.push(format!("painted {:.2} km²", km2(&d, n)));
    }
    if let Some(a) = &erase {
        let n = paint_area(&mut d, a, 0, Some(id))?;
        changed.push(format!("erased {:.2} km²", km2(&d, n)));
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    let name = d.zones.list[i].name.clone();
    let mut out = vec![McpContent::Text(format!(
        "Zone {id} \"{name}\": {}. One undo step.",
        if changed.is_empty() {
            "unchanged".into()
        } else {
            changed.join(", ")
        }
    ))];
    if let Some(p) = biome {
        // the inspector's "make this zone a biome" (its own undo step)
        biome_msgs.write(crate::biomes::ApplyBiomeToZone {
            zone: id,
            preset: p,
        });
        out.push(McpContent::deferred(move |w| {
            let toast = w
                .resource::<crate::editor::Editor>()
                .toast
                .as_ref()
                .map(|t| t.0.clone())
                .unwrap_or_default();
            Some(Ok(vec![McpContent::Text(format!(
                "Biome: {toast} (a separate undo step)."
            ))]))
        }));
    }
    Ok(out)
}

fn delete(In(args): In<Value>, mut d: WorldData) -> R {
    let i = zone_index(&d, &args)?;
    let id = d.zones.list[i].id;
    d.history.begin("Delete zone");
    d.history.touch_zones(&d.zones);
    let n = d.terrain.n();
    let all = crate::terrain::GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    };
    d.history.touch_terrain(&d.terrain, all);
    for v in d.terrain.zones.iter_mut() {
        if *v == id {
            *v = 0;
        }
    }
    d.terrain.mark_zone_dirty(all);
    let z = d.zones.list.remove(i);
    d.zones.selected = None;
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(format!(
        "Deleted zone {id} \"{}\" and cleared its ground. One undo step.",
        z.name
    ))
}

fn list(In(_): In<Value>, d: WorldData, stats: Res<crate::zones::ZoneStats>) -> R {
    let zones: Vec<Value> = d
        .zones
        .list
        .iter()
        .map(|z| {
            let opt = |on: bool, v: String| if on { json!(v) } else { Value::Null };
            let mut zone = json!({
                "id": z.id, "name": z.name, "color": z.color, "description": z.description,
                "level_min": z.level_min, "level_max": z.level_max, "zone_type": snake(&format!("{:?}", z.zone_type)),
                "weather": opt(z.weather_enabled, snake(&format!("{:?}", z.weather))), "weather_intensity": z.weather_intensity,
                "ambience": opt(z.audio_enabled, snake(&format!("{:?}", z.soundscape))), "ambience_volume": z.audio_volume,
                "mood": opt(z.mood_enabled, snake(&format!("{:?}", z.mood))),
                "style": z.own_style().map(|(p, _)| snake(&format!("{p:?}"))), "style_strength": z.style_strength,
                "lighting": z.own_light(), "lighting_fine_tuned": z.mood_enabled && z.light.is_some(),
                "biome_filter": opt(z.biome_enabled, snake(&format!("{:?}", z.biome))),
                "forest_diversity": z.forest.map(|f| json!({ "enabled": f.enabled, "layout": match f.layout { crate::nature::ForestLayout::NaturalGroves => "natural_groves", crate::nature::ForestLayout::Mixed => "mixed" } })).unwrap_or(Value::Null),
                "biome": z.biome_preset.map(|b| snake(&format!("{b:?}"))),
                "wind_speed": if z.wind_enabled { json!(z.wind_speed) } else { Value::Null },
                "cloud_cover": if z.clouds_enabled { json!(z.cloud_cover) } else { Value::Null },
                "temperature": z.temperature,
                "limit_placement": z.limit_placement, "allow_mounts": z.allow_mounts, "allow_flying": z.allow_flying, "rested_area": z.rested_area, "visible": z.visible,
                "area_km2": stats.area_km2.get(&z.id).map(|a| (a * 100.0).round() / 100.0),
                "label_at": stats.anchor.get(&z.id).map(|a| [a.x.round(), a.z.round()]),
            });
            zone["haze"] = json!({ "mode": snake(&format!("{:?}", z.haze.mode)), "amount": z.haze.amount });
            zone
        })
        .collect();
    json_out(json!({ "zones": zones, "note": "areas update a moment after painting" }))
}
