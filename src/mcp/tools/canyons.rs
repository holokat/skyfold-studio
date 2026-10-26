//! Layered canyons and rock strata: the Strata brush (`terrain::strata`) and the canyonlands
//! generator (`terrain::canyons`), through the same code as Sculpt › Strata / Canyonlands.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::terrain::{DAB_DT, check_dabs, dab_rect, dabs, height_report, heights_in, passes};
use super::*;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpTool};
use crate::terrain::brush::BrushArgs;
use crate::terrain::canyons::CanyonParams;
use crate::terrain::strata::{self, StrataParams};

fn strata_props() -> serde_json::Map<String, Value> {
    let v = json!({
        "step_height": { "type": "number", "minimum": 1, "maximum": 120, "description": "thickness of one rock bed = height of a step (m); default the editor's Strata setting (14)" },
        "sharpness": { "type": "number", "minimum": 0, "maximum": 1, "description": "0 soft rounded steps … 1 crisp cliffs with flat benches (default 0.7)" },
        "irregularity": { "type": "number", "minimum": 0, "maximum": 1, "description": "0 even stairs … 1 beds of very different thickness, wandering eroded ledge edges, rubble (default 0.5)" }
    });
    v.as_object().cloned().unwrap_or_default()
}

fn strata_args(args: &Value, base: StrataParams) -> Result<StrataParams, String> {
    Ok(StrataParams {
        step: num_in(args, "step_height", base.step, 1.0, 120.0)?,
        sharpness: num_in(args, "sharpness", base.sharpness, 0.0, 1.0)?,
        irregularity: num_in(args, "irregularity", base.irregularity, 0.0, 1.0)?,
    })
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "terrain_strata",
        description: "Weather the ground into stepped rock strata with the Sculpt › Strata brush, along a stroke (`path`, a dab every 0.3 × radius) or at `points` (held half a second each): heights are quantised into beds of `step_height` m, hard beds standing as short cliffs and soft ones as benches, with slightly irregular, eroded ledge edges (not perfect stairs). \
Works best on slopes and hills (flat ground stays flat); repeat with `passes` or a higher `strength` for crisper steps. Returns how much the ground moved. One undo step.",
        input_schema: || {
            let mut p = strata_props();
            for (k, v) in json!({
                "path": points_schema("stroke centre line [[x, z], …]", 1),
                "points": points_schema("stamp spots instead of a stroke", 1),
                "radius": { "type": "number", "minimum": 2, "maximum": 2000, "default": 90, "description": "brush radius (m)" },
                "strength": { "type": "number", "minimum": 0.02, "maximum": 1, "default": 0.6 },
                "falloff": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.6, "description": "0 hard edge … 1 soft edge" },
                "passes": { "type": "integer", "minimum": 1, "maximum": 20, "default": 1 },
                "remember": { "type": "boolean", "default": false, "description": "also make these the Strata brush's settings in the editor" }
            })
            .as_object()
            .unwrap()
            {
                p.insert(k.clone(), v.clone());
            }
            json!({ "type": "object", "properties": p })
        },
        run: |w, a| run(w, strata_stroke, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_canyonlands",
        description: "Raise layered canyonlands (Sculpt › Canyonlands): high plateaus cut by branching, meandering canyon networks with terraced walls (rock strata ledges and a broad bench part-way down), open basins with isolated mesas and buttes, and canyon floors at a few distinct levels (`tiers`) so lakes sit at different heights and water can fall from tier to tier; a narrow inner gorge runs along the main canyons for rivers. \
`area` \"world\" replaces the whole world's heights (lowest floor 6 m above sea unless `floor`); or give `center` [x, z] + `radius` m for a disc that blends into the ground round it (lowest floor = the ground's mean height there unless `floor`). `preset` \"alien\" is the vast Sci-Fi canyonland, \"default\" a gentler one; any field overrides it. \
With `lakes` > 0 shallow basins are dug on the canyon floors of every tier and (unless `fill_lakes` is false) filled with water, each lake at its own height. Paint, biomes and existing water are left alone. Returns the floor, plateau and tier heights and the lake sites. One undo step.",
        input_schema: || {
            let mut p = strata_props();
            for (k, v) in json!({
                "area": { "type": "string", "enum": ["world", "disc"], "default": "disc", "description": "\"world\": everywhere; \"disc\": `center` + `radius`" },
                "center": xz_schema("disc centre"),
                "radius": { "type": "number", "minimum": 50, "maximum": 8000, "default": 800, "description": "disc radius (m); the outer ~30% blends into the ground" },
                "preset": { "type": "string", "enum": ["alien", "default"], "default": "default" },
                "seed": { "type": "integer", "minimum": 0, "description": "default: the world's seed" },
                "relief": { "type": "number", "minimum": 10, "maximum": 1200, "description": "plateau height above the lowest canyon floor (m); alien 260, default 180" },
                "floor": { "type": "number", "description": "absolute height of the lowest canyon floor (m)" },
                "scale": { "type": "number", "minimum": 150, "maximum": 20000, "description": "typical spacing of the main canyons (m); alien 1300, default 1100 (a disc caps it at 1.6 × radius)" },
                "canyon_width": { "type": "number", "minimum": 0.2, "maximum": 3, "description": "× canyon width (0.4 slot canyons … 2 broad valleys)" },
                "branching": { "type": "number", "minimum": 0, "maximum": 1, "description": "side canyons and hanging valleys" },
                "mesas": { "type": "number", "minimum": 0, "maximum": 1, "description": "open basins with isolated mesas and buttes" },
                "tiers": { "type": "integer", "minimum": 1, "maximum": 5, "description": "distinct canyon-floor levels" },
                "plateau_steps": { "type": "number", "minimum": 0, "maximum": 1, "description": "how much the plateau itself steps into levels" },
                "lakes": { "type": "number", "minimum": 0, "maximum": 1, "description": "lake basins on the canyon floors (0 none; alien 0.6)" },
                "fill_lakes": { "type": "boolean", "default": true, "description": "fill the basins with water" },
                "strata_bands": { "type": "number", "minimum": 0, "maximum": 1, "description": "banded rock layers on every cliff face in this world (the ground style); area \"world\" sets at least 0.8 unless given" }
            })
            .as_object()
            .unwrap()
            {
                p.insert(k.clone(), v.clone());
            }
            json!({ "type": "object", "properties": p })
        },
        run: |w, a| run(w, canyonlands, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_canyon_settings",
        description: "Read or change the editor's Sculpt › Canyonlands settings (what a click of the Canyonlands tool and its whole-world button make) and Strata brush settings: the same fields as terrain_canyonlands (`preset` first, then any field), `step_height` / `sharpness` / `irregularity` set both. `open` shows Sculpt › Canyonlands (canyons) or the Strata brush (strata) in the editor. Returns the settings. Not an undo step.",
        input_schema: || {
            let mut p = strata_props();
            for (k, v) in json!({
                "preset": { "type": "string", "enum": ["alien", "default"], "description": "start from this look (omit: keep the current settings)" },
                "relief": { "type": "number", "minimum": 10, "maximum": 1200 },
                "scale": { "type": "number", "minimum": 150, "maximum": 20000 },
                "canyon_width": { "type": "number", "minimum": 0.2, "maximum": 3 },
                "branching": { "type": "number", "minimum": 0, "maximum": 1 },
                "mesas": { "type": "number", "minimum": 0, "maximum": 1 },
                "tiers": { "type": "integer", "minimum": 1, "maximum": 5 },
                "plateau_steps": { "type": "number", "minimum": 0, "maximum": 1 },
                "lakes": { "type": "number", "minimum": 0, "maximum": 1 },
                "open": { "type": "string", "enum": ["canyons", "strata"], "description": "canyons: open Sculpt › Canyonlands; strata: the Strata brush" }
            })
            .as_object()
            .unwrap()
            {
                p.insert(k.clone(), v.clone());
            }
            json!({ "type": "object", "properties": p })
        },
        run: |w, a| run(w, canyon_settings, a),
    });
}

fn canyon_settings(In(args): In<Value>, mut d: WorldData) -> R {
    let mut a = args.clone();
    if opt_str(&args, "preset")?.is_none()
        && let Some(o) = a.as_object_mut()
    {
        // no preset: the current settings are the base
        let c = d.ed.canyons;
        for (k, v) in [
            ("relief", c.relief),
            ("scale", c.scale),
            ("canyon_width", c.canyon_width),
            ("branching", c.branching),
            ("mesas", c.mesas),
            ("tiers", c.tiers as f32),
            ("plateau_steps", c.plateau_steps),
            ("lakes", c.lakes),
            ("step_height", c.strata.step),
            ("sharpness", c.strata.sharpness),
            ("irregularity", c.strata.irregularity),
        ] {
            o.entry(k).or_insert(json!(v));
        }
    }
    let had_strata = ["step_height", "sharpness", "irregularity"]
        .iter()
        .any(|k| args.get(*k).is_some());
    d.ed.canyons = CanyonParams {
        floor: None,
        ..canyon_args(&a)?
    };
    if had_strata {
        d.ed.strata = strata_args(&args, d.ed.strata)?;
    }
    let open = match args.get("open") {
        Some(Value::Bool(true)) => Some(crate::editor::SculptTool::Canyons),
        Some(Value::String(s)) if s == "canyons" => Some(crate::editor::SculptTool::Canyons),
        Some(Value::String(s)) if s == "strata" => Some(crate::editor::SculptTool::Strata),
        _ => None,
    };
    if let Some(tool) = open {
        d.ed.tab = crate::editor::Tab::Sculpt;
        d.ed.sculpt_tool = tool;
        if tool == crate::editor::SculptTool::Canyons && d.ed.sculpt_brush.size < 300.0 {
            d.ed.sculpt_brush.size = 700.0;
        }
    }
    let c = d.ed.canyons;
    json_out(json!({
        "canyons": {
            "relief": c.relief, "scale": c.scale, "canyon_width": c.canyon_width,
            "branching": c.branching, "mesas": c.mesas, "tiers": c.tiers,
            "plateau_steps": c.plateau_steps, "lakes": c.lakes,
            "step_height": c.strata.step, "sharpness": c.strata.sharpness,
            "irregularity": c.strata.irregularity, "area_radius": d.ed.sculpt_brush.size
        },
        "strata_brush": {
            "step_height": d.ed.strata.step, "sharpness": d.ed.strata.sharpness,
            "irregularity": d.ed.strata.irregularity
        }
    }))
}

fn strata_stroke(In(args): In<Value>, mut d: WorldData) -> R {
    let sp = strata_args(&args, d.ed.strata)?;
    let radius = num_in(&args, "radius", 90.0, 2.0, 2000.0)?;
    let strength = num_in(&args, "strength", 0.6, 0.02, 1.0)?;
    let falloff = num_in(&args, "falloff", 0.6, 0.0, 1.0)?;
    let passes = passes(&args)?;
    let dabs = dabs(&args, (radius * 0.3).max(0.5))?;
    check_dabs(&d.terrain, &dabs, radius, passes)?;
    if opt_bool(&args, "remember")?.unwrap_or(false) {
        d.ed.strata = sp;
    }
    let rect = dab_rect(&d.terrain, &dabs, radius).ok_or("the stroke lies outside the world")?;
    let before = heights_in(&d.terrain, rect);
    let seed = d.meta.seed;
    d.history.begin("Sculpt: Strata");
    for _ in 0..passes {
        for c in &dabs {
            let a = BrushArgs {
                center: *c,
                radius,
                falloff,
                strength,
                dt: DAB_DT,
            };
            strata::strata(&mut d.terrain, &mut d.history, &a, &sp, seed + 17);
        }
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    let report = height_report(&d.terrain, rect, &before);
    text(format!(
        "Strata ({:.0} m beds, sharpness {:.2}, irregularity {:.2}): {} dabs × {passes} pass(es), radius {radius:.0} m. {report} One undo step.",
        sp.step,
        sp.sharpness,
        sp.irregularity,
        dabs.len()
    ))
}

/// The canyon settings from the arguments over a preset.
fn canyon_args(args: &Value) -> Result<CanyonParams, String> {
    let base = match opt_str(args, "preset")?.unwrap_or("default") {
        "alien" => CanyonParams::alien(),
        "default" => CanyonParams::default(),
        p => return Err(format!("unknown preset `{p}` (alien or default)")),
    };
    Ok(CanyonParams {
        relief: num_in(args, "relief", base.relief, 10.0, 1200.0)?,
        floor: opt_num(args, "floor")?.or(base.floor),
        scale: num_in(args, "scale", base.scale, 150.0, 20_000.0)?,
        canyon_width: num_in(args, "canyon_width", base.canyon_width, 0.2, 3.0)?,
        branching: num_in(args, "branching", base.branching, 0.0, 1.0)?,
        mesas: num_in(args, "mesas", base.mesas, 0.0, 1.0)?,
        tiers: num_in(args, "tiers", base.tiers as f32, 1.0, 5.0)?.round() as u32,
        plateau_steps: num_in(args, "plateau_steps", base.plateau_steps, 0.0, 1.0)?,
        strata: strata_args(args, base.strata)?,
        lakes: num_in(args, "lakes", base.lakes, 0.0, 1.0)?,
    })
}

fn canyonlands(In(args): In<Value>, mut d: WorldData) -> R {
    let params = canyon_args(&args)?;
    let area = match opt_str(&args, "area")?.unwrap_or("disc") {
        "world" => None,
        "disc" => {
            let c = opt_xz(&args, "center")?.ok_or("a disc needs `center` [x, z] (or area \"world\")")?;
            let r = num_in(&args, "radius", 800.0, 50.0, 8000.0)?;
            if !d.terrain.in_bounds(c.x, c.y) {
                return Err(format!(
                    "`center` is outside the world (x and z within ±{:.0} m)",
                    d.terrain.half()
                ));
            }
            Some((c, r))
        }
        a => return Err(format!("unknown area `{a}` (world or disc)")),
    };
    let seed = match opt_num(&args, "seed")? {
        Some(s) => s.max(0.0) as u32,
        None => d.meta.seed + 19,
    };
    let fill = opt_bool(&args, "fill_lakes")?.unwrap_or(true) && params.lakes > 0.0;
    d.history.begin("Canyonlands");
    let WorldData {
        terrain,
        history,
        store,
        water,
        lib,
        ..
    } = &mut d;
    let made = crate::terrain::canyon_ops::canyonlands(
        terrain,
        history,
        store,
        lib.as_deref(),
        water,
        &params,
        seed,
        area,
        fill,
    );
    d.history.commit(&d.store);
    if let Some(b) = opt_num(&args, "strata_bands")? {
        d.terrain.style.strata_bands = b.clamp(0.0, 1.0);
    }
    let Some((rep, lakes)) = made else {
        return Err("the disc lies outside the world".into());
    };
    d.meta.dirty = true;
    let sea = d.terrain.sea_level;
    let sites: Vec<Value> = rep
        .lake_sites
        .iter()
        .map(|s| {
            json!({
                "center": [s.center.x.round(), s.center.y.round()],
                "radius": s.radius.round(),
                "rim_height": (s.rim * 10.0).round() / 10.0,
                "tier": s.tier
            })
        })
        .collect();
    json_out(json!({
        "summary": format!(
            "Canyonlands ({}): lowest floor {:.1} m ({:+.1} m from sea), plateaus up to {:.1} m; {} floor tiers; {} lake basins{}. One undo step.",
            if area.is_some() { "disc" } else { "whole world" },
            rep.floor,
            rep.floor - sea,
            rep.top,
            rep.tier_heights.len(),
            rep.lake_sites.len(),
            if fill { format!(", {lakes} filled with water") } else { String::new() }
        ),
        "floor": rep.floor,
        "top": rep.top,
        "tier_heights": rep.tier_heights,
        "lake_sites": sites,
        "lakes_made": lakes
    }))
}
