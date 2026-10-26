//! Fields over MCP: list, add (an outline), change, remove, and lay out farmland automatically.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::{Crop, PlantWorld};
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};

type R = Result<Vec<McpContent>, String>;

fn crop_ids() -> Vec<&'static str> {
    Crop::ALL.iter().map(|c| c.id()).collect()
}

fn colour_names() -> Vec<&'static str> {
    super::plants::TULIP_COLOUR_NAMES.to_vec()
}

/// `colour` (a tulip colour, or "mixed" / null for bands of every colour), if given.
fn colour_arg(args: &Value) -> Result<Option<Option<u8>>, String> {
    match args.get("colour") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(v) => {
            let s = v.as_str().ok_or("`colour` is a colour name")?;
            if s.eq_ignore_ascii_case("mixed") {
                return Ok(Some(None));
            }
            super::tulip_colour(s)
                .map(|c| Some(Some(c)))
                .ok_or_else(|| {
                    format!(
                        "unknown colour `{s}` ({}, mixed)",
                        colour_names().join(", ")
                    )
                })
        }
    }
}

fn colour_schema() -> Value {
    let mut e: Vec<Value> = colour_names().into_iter().map(Value::from).collect();
    e.push("mixed".into());
    json!({ "type": "string", "enum": e, "description": "tulips only: every tulip this colour (\"mixed\", the default: bands of red, yellow, pink, white, purple and orange). Pictures and flags from flowers: one field per colour, each its own outline that don't overlap (fields that overlap both grow there): a red disc in a white field is a red field with the disc's outline plus a white field with that disc as one of its `holes`" })
}

/// rows_deg (0 = along +x, 90 = along +z) ↔ the field's row angle (`Quat::from_rotation_y`).
fn rows_deg(rows: f32) -> f32 {
    let d = (Quat::from_rotation_y(rows) * Vec3::X).xz();
    (d.y.atan2(d.x).to_degrees().rem_euclid(180.0) * 10.0).round() / 10.0
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "fields_list",
        description: "The farm fields: each field's id, crop, area (m²), centre [x, z], row direction (rows_deg: 0 = rows along +x, 90 = along +z) and outline. Also lists the crops a field can grow.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| w.run_system_cached_with(list, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "fields_add",
        description: "Plant a farm field (like Nature › Fields: draw a loop): `crop` in rows inside `polygon` [[x, z], …] (at least 3 corners, 30 m² … 60 ha). Rows run along the field's long side unless `rows_deg` is given (0 = along +x, 90 = along +z). The ground is tilled (soil, or stubble for grain), wild nature cleared out of it; the plants are procedural and sway in the wind. Crops: wheat, barley, rye, corn, sunflowers, hay (stubble and round bales), potatoes, cabbages, carrots, pumpkins, strawberries, tomatoes, lavender, tulips (bands of colours, or one `colour`), vineyard (trellised vines with grapes), hops (5 m poles and bines), cotton, rice (paddy), tea (hedge rows), plowed (bare furrows). Returns the id. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "crop": { "type": "string", "enum": crop_ids() },
                    "polygon": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "minItems": 3 },
                    "rows_deg": { "type": "number", "description": "row direction: 0 = along +x, 90 = along +z (default: the field's long side)" },
                    "colour": colour_schema(),
                    "holes": { "type": "array", "description": "outlines inside `polygon` left unplanted (for another field to fill: a disc, a stripe, a letter)", "items": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "minItems": 3 } }
                },
                "required": ["crop", "polygon"]
            })
        },
        run: |w, a| w.run_system_cached_with(add, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "fields_set",
        description: "Change a farm field `id` (like Nature › Fields › a field): its `crop`, its row direction `rows_deg` (0 = rows along +x, 90 = along +z) and, for tulips, its `colour`. The outline stays; to reshape a field remove it and add it again. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "crop": { "type": "string", "enum": crop_ids() },
                    "rows_deg": { "type": "number" },
                    "colour": colour_schema()
                },
                "required": ["id"]
            })
        },
        run: |w, a| w.run_system_cached_with(set, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "fields_remove",
        description: "Remove farm fields by `ids` (their tilled ground stays as painted). One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "integer" }, "minItems": 1 } }, "required": ["ids"] }),
        run: |w, a| w.run_system_cached_with(remove, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "fields_auto",
        description: "Lay out farmland automatically (like Nature › Fields › Plant fields round the world): `clusters` patchworks (default by world size) of a few rectangular fields with lanes between them, on flat, dry lowland near villages first, crops suited to each biome (none in deserts, swamps or volcanic land), or only `crops`. `area`: only inside this rectangle {min: [x, z], max: [x, z]}. `seed` varies the layout. Returns the new fields. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "clusters": { "type": "integer", "minimum": 1, "maximum": 60 },
                    "crops": { "type": "array", "items": { "type": "string", "enum": crop_ids() } },
                    "area": { "type": "object", "properties": { "min": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "max": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 } }, "required": ["min", "max"] },
                    "seed": { "type": "integer" }
                }
            })
        },
        run: |w, a| w.run_system_cached_with(auto, a).map_err(|e| format!("{e}"))?,
    });
}

fn field_json(f: &super::Field) -> Value {
    let c = f.centre();
    let r = |v: f32| (v * 10.0).round() / 10.0;
    let mut v = json!({ "id": f.id, "crop": f.crop.id(), "area_m2": f.area().round(), "centre": [r(c.x), r(c.y)], "rows_deg": rows_deg(f.rows), "outline": f.outline.iter().map(|p| [r(p[0]), r(p[1])]).collect::<Vec<_>>() });
    if !f.holes.is_empty() {
        v["holes"] = f
            .holes
            .iter()
            .map(|h| h.iter().map(|p| [r(p[0]), r(p[1])]).collect::<Vec<_>>())
            .collect::<Vec<_>>()
            .into();
    }
    if f.crop == Crop::Tulips {
        v["colour"] = f
            .colour
            .and_then(|c| colour_names().get(c as usize).copied())
            .unwrap_or("mixed")
            .into();
    }
    v
}

fn list(In(_args): In<Value>, d: WorldData) -> R {
    let out: Vec<Value> = d.fields.list.iter().map(field_json).collect();
    Ok(vec![McpContent::Json(
        json!({ "fields": out, "crops": crop_ids(), "tulip_colours": colour_names() }),
    )])
}

fn add(In(args): In<Value>, mut d: WorldData) -> R {
    let crop_s = args
        .get("crop")
        .and_then(|v| v.as_str())
        .ok_or("missing `crop`")?;
    let crop = Crop::from_id(crop_s)
        .ok_or_else(|| format!("unknown crop `{crop_s}` ({})", crop_ids().join(", ")))?;
    let poly: Vec<[f32; 2]> =
        crate::mcp_tools::opt_arg(&args, "polygon")?.ok_or("missing `polygon`")?;
    let poly: Vec<Vec2> = poly.into_iter().map(Vec2::from_array).collect();
    let holes: Vec<Vec<[f32; 2]>> = crate::mcp_tools::opt_arg(&args, "holes")?.unwrap_or_default();
    let holes: Vec<Vec<Vec2>> = holes
        .into_iter()
        .map(|h| h.into_iter().map(Vec2::from_array).collect())
        .collect();
    // rows_deg: 0 along +x, 90 along +z → the row angle
    let rows = args
        .get("rows_deg")
        .and_then(|v| v.as_f64())
        .map(|deg| -(deg as f32).to_radians());
    let colour = colour_arg(&args)?.flatten();
    if colour.is_some() && crop != Crop::Tulips {
        return Err("`colour` is for tulip fields only".into());
    }
    let label = format!("Plant {}", crop.name().to_lowercase());
    d.history.begin(label.clone());
    let WorldData {
        fields,
        terrain,
        history,
        nature,
        water,
        ..
    } = &mut d;
    let r = super::add_with_holes(
        &mut PlantWorld {
            fields,
            terrain,
            history,
            nature,
            water,
        },
        crop,
        &poly,
        &holes,
        rows,
        &label,
    )
    .and_then(|id| {
        if colour.is_some() {
            super::set_colour(&mut d.fields, &mut d.history, id, colour)?;
        }
        Ok(id)
    });
    d.history.commit(&d.store);
    let id = r?;
    d.meta.dirty = true;
    let f = d.fields.get(id).map(field_json).unwrap_or(Value::Null);
    Ok(vec![McpContent::Json(
        json!({ "field": f, "note": "one undo step" }),
    )])
}

fn set(In(args): In<Value>, mut d: WorldData) -> R {
    let id = args
        .get("id")
        .and_then(|v| v.as_u64())
        .ok_or("missing `id` (fields_list)")?;
    if d.fields.get(id).is_none() {
        return Err(format!("no field {id} (fields_list)"));
    }
    let crop = match args.get("crop").and_then(|v| v.as_str()) {
        Some(c) => Some(
            Crop::from_id(c)
                .ok_or_else(|| format!("unknown crop `{c}` ({})", crop_ids().join(", ")))?,
        ),
        None => None,
    };
    let rows = args
        .get("rows_deg")
        .and_then(|v| v.as_f64())
        .map(|deg| -(deg as f32).to_radians());
    let colour = colour_arg(&args)?;
    d.history.begin("Change field");
    let r = (|| {
        if crop.is_some() || rows.is_some() {
            super::edit(&mut d.fields, &mut d.history, id, crop, rows)?;
        }
        if let Some(c) = colour {
            super::set_colour(&mut d.fields, &mut d.history, id, c)?;
        }
        Ok::<_, String>(())
    })();
    d.history.commit(&d.store);
    r?;
    d.meta.dirty = true;
    let f = d.fields.get(id).map(field_json).unwrap_or(Value::Null);
    Ok(vec![McpContent::Json(
        json!({ "field": f, "note": "one undo step" }),
    )])
}

fn remove(In(args): In<Value>, mut d: WorldData) -> R {
    let ids: Vec<u64> = crate::mcp_tools::opt_arg(&args, "ids")?.ok_or("missing `ids`")?;
    if let Some(bad) = ids.iter().find(|id| d.fields.get(**id).is_none()) {
        return Err(format!("no field {bad} (fields_list)"));
    }
    d.history.begin("Remove fields");
    for id in &ids {
        let WorldData {
            fields, history, ..
        } = &mut d;
        super::remove(fields, history, *id)?;
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    Ok(vec![McpContent::Text(format!(
        "Removed {} field(s). One undo step.",
        ids.len()
    ))])
}

fn auto(In(args): In<Value>, mut d: WorldData) -> R {
    let crops: Vec<Crop> = match args.get("crops").and_then(|v| v.as_array()) {
        Some(a) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .and_then(Crop::from_id)
                    .ok_or_else(|| format!("unknown crop {v}"))
            })
            .collect::<Result<_, _>>()?,
        None => vec![],
    };
    let area = match args.get("area").filter(|v| !v.is_null()) {
        Some(a) => {
            let p = |k: &str| -> Result<Vec2, String> {
                let v: [f32; 2] = serde_json::from_value(a.get(k).cloned().unwrap_or_default())
                    .map_err(|_| format!("`area.{k}` is [x, z]"))?;
                Ok(Vec2::from_array(v))
            };
            let (a, b) = (p("min")?, p("max")?);
            Some((a.min(b), a.max(b)))
        }
        None => None,
    };
    let clusters = args
        .get("clusters")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or_else(|| ((d.terrain.size() / 900.0).powi(2) as usize).clamp(3, 24));
    let seed = args.get("seed").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let opts = super::auto::Auto {
        clusters,
        area,
        crops,
        seed,
    };
    d.history.begin("Plant fields");
    let WorldData {
        fields,
        terrain,
        history,
        nature,
        water,
        store,
        ..
    } = &mut d;
    let r = super::auto::auto(
        &mut PlantWorld {
            fields,
            terrain,
            history,
            nature,
            water,
        },
        store,
        &opts,
    );
    d.history.commit(&d.store);
    let ids = r?;
    d.meta.dirty = true;
    let out: Vec<Value> = ids.iter().filter_map(|id| d.fields.get(*id)).map(|f| json!({ "id": f.id, "crop": f.crop.id(), "area_m2": f.area().round(), "centre": [f.centre().x.round(), f.centre().y.round()] })).collect();
    Ok(vec![McpContent::Json(
        json!({ "planted": out.len(), "fields": out, "note": "one undo step" }),
    )])
}

#[cfg(test)]
mod tests {
    use super::super::{Crop, Field};
    use bevy::prelude::Vec2;

    #[test]
    fn a_hole_is_left_unplanted() {
        let disc: Vec<[f32; 2]> = (0..24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                [a.cos() * 6.0, a.sin() * 6.0]
            })
            .collect();
        let f = Field {
            id: 1,
            crop: Crop::Tulips,
            outline: vec![[-20.0, -20.0], [20.0, -20.0], [20.0, 20.0], [-20.0, 20.0]],
            rows: 0.0,
            colour: Some(3),
            holes: vec![disc],
        };
        assert!(!f.contains(Vec2::ZERO), "the hole's middle is planted");
        assert!(!f.contains(Vec2::new(3.0, 2.0)));
        assert!(f.contains(Vec2::new(12.0, 12.0)));
        assert!(f.contains(Vec2::new(-15.0, 3.0)));
        assert!(!f.contains(Vec2::new(25.0, 0.0)));
        // (no slit: the ground between the hole and the edge is planted all round)
        for k in 0..16 {
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            assert!(f.contains(Vec2::new(a.cos(), a.sin()) * 10.0));
        }
        assert!(
            (f.area() - (1600.0 - 111.0)).abs() < 2.0,
            "area {}",
            f.area()
        );
    }
}
