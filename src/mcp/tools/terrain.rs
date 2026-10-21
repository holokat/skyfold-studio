//! Terrain tools: sample, sculpt, paint, land / coastline, biomes and whole-world operations,
//! through the Sculpt / Paint / Land brushes and the biome brush code.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::biomes::BiomePreset;
use crate::editor::SculptTool;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpTool};
use crate::terrain::brush::{self, BrushArgs};
use crate::terrain::layers::Layer;
use crate::terrain::{GridRect, Terrain};

/// Brush time per dab: one dab is one 20 fps frame of holding the mouse button.
const DAB_DT: f32 = 0.05;
/// Dabs per point when stamping at `points`: holding the brush still for half a second.
const HOLD_DABS: usize = 10;
const MAX_DABS: usize = 20_000;
/// terrain_sample grids: columns and rows.
const MAX_COLS: usize = 96;

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "terrain_sample",
        description: "Read the terrain. With `points`: height, height above sea, slope, water (sea/lake/river and depth), biome, zone and dominant ground material at each [x, z]. \
Without `points`: a grid over `min`..`max` (default: the whole world). format \"ascii\" (default for grids) returns a character map per cell — layer \"height\": ~ deep sea, - shallow sea, w lake/river, then . : + ^ M from lowland to peaks (thresholds in the legend); \
layer \"biome\" / \"zone\": one letter per biome / zone (legend included). format \"values\" returns rows of numbers (heights in m, plus slope in degrees if asked). Up to 96 columns and 96 rows. Row 0 is the north edge (smallest z); cell centres are listed as x and z.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "points": points_schema("positions to probe (max 500)", 1),
                    "min": xz_schema("grid: corner with the smallest x and z (default: world corner)"),
                    "max": xz_schema("grid: corner with the largest x and z (default: world corner)"),
                    "cols": { "type": "integer", "minimum": 2, "maximum": MAX_COLS, "description": "grid columns (default 48 for ascii, 16 for values); rows follow the aspect ratio (at most 96)" },
                    "format": { "type": "string", "enum": ["ascii", "values"], "default": "ascii" },
                    "layer": { "type": "string", "enum": ["height", "biome", "zone"], "default": "height", "description": "what the ascii map shows" },
                    "slope": { "type": "boolean", "default": false, "description": "values format: also return slope rows (degrees)" }
                }
            })
        },
        run: |w, a| run(w, sample, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_sculpt",
        description: "Sculpt the ground with the Sculpt brushes along a stroke (`path`, dabs every 0.3 × radius like a mouse drag) or at `points` (the brush held still for half a second on each). \
Tools: raise, lower, smooth, flatten (to `target_height`, default the height under the first dab), mountains (rugged ridges; invert carves canyons), hills (rolling bumps), terraces (steps of `terrace_step` m), erode (rain gullies and sediment). \
One pass of a raise stroke with radius 80 and strength 0.5 lifts its centre line by roughly 4-6 m; repeat with `passes` to go further. Each raise dab lifts its centre by strength × (0.35 × radius + 6) × 0.05 m, a `path` lays a dab every 0.3 × radius and each of `points` holds 10 dabs, so wide brushes at points climb fast: cap the change with `max_lift_m` (or use area_edit raise / flatten for exact metres). Objects ride the ground. Returns how much the ground moved. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "tool": enum_schema(SculptTool::ALL.iter().map(|t| t.name().to_lowercase()).collect(), "sculpt brush"),
                    "path": points_schema("stroke centre line [[x, z], …]", 1),
                    "points": points_schema("stamp spots instead of a stroke", 1),
                    "radius": { "type": "number", "minimum": 2, "maximum": 2000, "default": 80, "description": "brush radius (m)" },
                    "strength": { "type": "number", "minimum": 0.02, "maximum": 1, "default": 0.5 },
                    "falloff": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.7, "description": "0 hard edge … 1 soft edge" },
                    "passes": { "type": "integer", "minimum": 1, "maximum": 20, "default": 1, "description": "repeat the whole stroke" },
                    "invert": { "type": "boolean", "default": false, "description": "the Alt action: raise↔lower, mountains→canyons, hills→dents" },
                    "target_height": { "type": "number", "description": "flatten: absolute height (m) to level towards" },
                    "terrace_step": { "type": "number", "minimum": 2, "maximum": 60, "description": "terraces: step height (m), default the editor's (12)" },
                    "max_lift_m": { "type": "number", "minimum": 0, "description": "cap on how far any point may move up or down (m)" }
                },
                "required": ["tool"]
            })
        },
        run: |w, a| run(w, sculpt, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_paint",
        description: "Paint a ground material with the Paint brush along a stroke (`path`) or at `points` (held half a second each). Painted ground overrides the automatic style (snow, rock, beaches still appear where you don't paint); `erase` blends back to automatic. \
Strength 1 covers the stroke's middle in one pass; use `passes` for full cover at the edges. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "layer": enum_schema(ids(&Layer::ALL), "ground material"),
                    "path": points_schema("stroke centre line [[x, z], …]", 1),
                    "points": points_schema("stamp spots instead of a stroke", 1),
                    "radius": { "type": "number", "minimum": 1, "maximum": 1500, "default": 30 },
                    "strength": { "type": "number", "minimum": 0.02, "maximum": 1, "default": 1 },
                    "falloff": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.5 },
                    "passes": { "type": "integer", "minimum": 1, "maximum": 20, "default": 1 },
                    "erase": { "type": "boolean", "default": false, "description": "remove paint (back to the automatic style) instead" }
                }
            })
        },
        run: |w, a| run(w, paint, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_land",
        description: "Shape continents like the Land tab. Either brush land up out of the sea along `path` / at `points` (mode \"raise\"; natural ragged coasts and gentle hills, `land_height` m above sea) or sink it into the sea (mode \"sink\": bays, straits, inland seas); \
or fill a whole coastline `outline` polygon at once (Draw Coastline: raise makes a landmass, sink carves sea). Wide brushes (150-400 m) work best. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["raise", "sink"], "default": "raise" },
                    "outline": points_schema("closed coastline polygon [[x, z], …] to fill at once", 3),
                    "path": points_schema("brush stroke centre line", 1),
                    "points": points_schema("stamp spots instead of a stroke", 1),
                    "radius": { "type": "number", "minimum": 10, "maximum": 2000, "default": 220 },
                    "strength": { "type": "number", "minimum": 0.02, "maximum": 1, "default": 0.6 },
                    "falloff": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.6 },
                    "passes": { "type": "integer", "minimum": 1, "maximum": 20, "default": 1 },
                    "land_height": { "type": "number", "minimum": 2, "maximum": 60, "default": 8, "description": "height of new land above sea (m)" },
                    "roughness": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.55, "description": "coastline raggedness" }
                }
            })
        },
        run: |w, a| run(w, land, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_set_biome",
        description: "Make an area a biome (Zones tab › Biomes): sets the biome map, paints the biome's ground, regrows its trees, plants and rocks (replacing the nature there) and restyles lakes and rivers on it. \
The biome also brings its weather, ambience and grass. Give a `polygon` (outline fill: its border wanders naturally around the drawn line, by up to ~6% of the area's size, and the grounds interleave across it) or a `path` with `radius` (brush); `density` scales the vegetation (1 = the biome's natural amount). \
Returns what grew by category (trees, plants, rocks, debris) and how the grass grows there. Biomes never create zones. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "biome": enum_schema(ids(&BiomePreset::ALL), "biome preset"),
                    "polygon": points_schema("area outline [[x, z], …]", 3),
                    "path": points_schema("brush stroke centre line", 1),
                    "radius": { "type": "number", "minimum": 10, "maximum": 1500, "default": 150, "description": "brush radius for `path` (m)" },
                    "vegetation": { "type": "boolean", "default": true, "description": "clear painted nature edits in the area so the biome's own forests, plants and rocks grow there (nature always follows the biome map)" },
                    "density": { "type": "number", "minimum": 0.1, "maximum": 3, "default": 1, "description": "× the biome's vegetation density in the area (with `vegetation`)" }
                },
                "required": ["biome"]
            })
        },
        run: |w, a| run(w, set_biome, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_global",
        description: "Whole-world terrain operations (Sculpt tab buttons): \"erode\" (rain erosion everywhere: gullies, sediment fans), \"smooth\" (soften the whole world), \"clear_paint\" (remove all painted ground; the automatic style returns). One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "op": { "type": "string", "enum": ["erode", "smooth", "clear_paint"] } }, "required": ["op"] }),
        run: |w, a| {
            let action = match opt_str(&a, "op")? {
                Some("erode") => crate::ui::UiAction::ErodeWorld,
                Some("smooth") => crate::ui::UiAction::SmoothWorld,
                Some("clear_paint") => crate::ui::UiAction::ClearPaint,
                _ => return Err("`op` must be erode, smooth or clear_paint".into()),
            };
            super::world::ui_action(w, action, |_| "Done (one undo step).".into())
        },
    });
}

/// Brush dab centres: along `path` at `spacing`, or the given `points`.
fn dabs(args: &Value, spacing: f32) -> Result<Vec<Vec2>, String> {
    if let Some(p) = opt_points(args, "path")?.filter(|p| !p.is_empty()) {
        return Ok(along(&p, spacing));
    }
    if let Some(p) = opt_points(args, "points")?.filter(|p| !p.is_empty()) {
        return Ok(p
            .iter()
            .flat_map(|q| std::iter::repeat_n(*q, HOLD_DABS))
            .collect());
    }
    Err("give a `path` (a stroke) or `points` (stamp spots)".into())
}

fn passes(args: &Value) -> Result<usize, String> {
    Ok(num(args, "passes", 1.0)?.round().clamp(1.0, 20.0) as usize)
}

/// Grid rect covering the dabs (± radius).
fn dab_rect(t: &Terrain, dabs: &[Vec2], radius: f32) -> Option<GridRect> {
    let (lo, hi) = dabs
        .iter()
        .fold((Vec2::MAX, Vec2::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
    let c = (lo + hi) * 0.5;
    t.rect_for_circle(c, (hi - lo).max_element() * 0.5 + radius * 1.25)
}

fn check_dabs(t: &Terrain, dabs: &[Vec2], radius: f32, passes: usize) -> Result<(), String> {
    if dabs.len() * passes > MAX_DABS {
        return Err(format!(
            "that stroke needs {} dabs (max {MAX_DABS}): use a larger radius, fewer passes or a shorter path",
            dabs.len() * passes
        ));
    }
    let h = t.half() + radius;
    if !dabs.iter().any(|p| p.x.abs() <= h && p.y.abs() <= h) {
        return Err(format!(
            "the stroke lies outside the world (x and z must be within ±{:.0} m)",
            t.half()
        ));
    }
    Ok(())
}

fn heights_in(t: &Terrain, r: GridRect) -> Vec<f32> {
    let mut v = Vec::with_capacity(r.width() * r.height());
    for j in r.z0..=r.z1 {
        for i in r.x0..=r.x1 {
            v.push(t.h(i, j));
        }
    }
    v
}

/// "Ground changed at N points: up to +a m / −b m (mean c m); heights there now lo–hi m."
fn height_report(t: &Terrain, r: GridRect, before: &[f32]) -> String {
    let after = heights_in(t, r);
    let (mut n, mut up, mut down, mut sum, mut lo, mut hi) =
        (0usize, 0f32, 0f32, 0f32, f32::MAX, f32::MIN);
    for (a, b) in after.iter().zip(before) {
        let d = a - b;
        if d.abs() > 0.01 {
            n += 1;
            up = up.max(d);
            down = down.min(d);
            sum += d;
            lo = lo.min(*a);
            hi = hi.max(*a);
        }
    }
    if n == 0 {
        return "The ground did not change (outside the world, or nothing to do here).".into();
    }
    format!(
        "Ground changed at {n} grid points ({:.0} m²): up to {up:+.1} m / {down:+.1} m (mean {:+.2} m); heights there are now {lo:.1}..{hi:.1} m (sea level {:.1} m).",
        n as f32 * t.cell * t.cell,
        sum / n as f32,
        t.sea_level
    )
}

fn sculpt(In(args): In<Value>, mut d: WorldData) -> R {
    let tool_s = opt_str(&args, "tool")?.ok_or("missing `tool`")?;
    let tool = pick(&SculptTool::ALL, tool_s, "sculpt tool", SculptTool::name).map_err(|_| {
        format!(
            "unknown sculpt tool `{tool_s}`; one of: {}",
            SculptTool::ALL.map(|t| t.name().to_lowercase()).join(", ")
        )
    })?;
    let radius = num_in(&args, "radius", 80.0, 2.0, 2000.0)?;
    let strength = num_in(&args, "strength", 0.5, 0.02, 1.0)?;
    let falloff = num_in(&args, "falloff", 0.7, 0.0, 1.0)?;
    let invert = opt_bool(&args, "invert")?.unwrap_or(false);
    let step = num_in(&args, "terrace_step", d.ed.terrace_step, 2.0, 60.0)?;
    let passes = passes(&args)?;
    let dabs = dabs(&args, (radius * 0.3).max(0.5))?;
    check_dabs(&d.terrain, &dabs, radius, passes)?;
    let target = match opt_num(&args, "target_height")? {
        Some(h) => h,
        None => d.terrain.height_at(dabs[0].x, dabs[0].y),
    };
    let rect = dab_rect(&d.terrain, &dabs, radius).ok_or("the stroke lies outside the world")?;
    let before = heights_in(&d.terrain, rect);
    let seed = d.meta.seed;
    let sign = if invert { -1.0 } else { 1.0 };
    d.history.begin(format!("Sculpt: {}", tool.name()));
    for _ in 0..passes {
        for c in &dabs {
            let a = BrushArgs {
                center: *c,
                radius,
                falloff,
                strength,
                dt: DAB_DT,
            };
            let (t, h) = (&mut *d.terrain, &mut *d.history);
            match tool {
                SculptTool::Raise => brush::raise(t, h, &a, sign),
                SculptTool::Lower => brush::raise(t, h, &a, -sign),
                SculptTool::Smooth => brush::smooth(t, h, &a),
                SculptTool::Flatten => brush::flatten(t, h, &a, target),
                SculptTool::Mountain => brush::mountain(t, h, &a, sign, seed + 11),
                SculptTool::Hills => brush::hills(t, h, &a, sign, seed + 13),
                SculptTool::Terrace => brush::terrace(t, h, &a, step),
                // the brush's erosion (whole-area presets: terrain_erode)
                SculptTool::Erode => brush::erode(
                    t,
                    h,
                    &a,
                    &crate::terrain::erosion::ErosionPreset::default().params(),
                ),
            }
        }
    }
    if let Some(m) = opt_num(&args, "max_lift_m")?.map(|m| m.max(0.0)) {
        let t = &mut *d.terrain;
        for (k, (i, j)) in (rect.z0..=rect.z1)
            .flat_map(|j| (rect.x0..=rect.x1).map(move |i| (i, j)))
            .enumerate()
        {
            let idx = t.idx(i, j);
            t.heights[idx] = t.heights[idx].clamp(before[k] - m, before[k] + m);
        }
        t.mark_heights_dirty(rect);
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    let report = height_report(&d.terrain, rect, &before);
    text(format!(
        "Sculpt {}: {} dabs × {passes} pass(es), radius {radius:.0} m. {report} One undo step.",
        tool.name(),
        dabs.len()
    ))
}

fn paint(In(args): In<Value>, mut d: WorldData) -> R {
    let layer = opt_pick(&args, "layer", &Layer::ALL, "ground layer", Layer::name)?;
    let erase = opt_bool(&args, "erase")?.unwrap_or(false);
    let layer = match (layer, erase) {
        (Some(l), _) => l,
        (None, true) => Layer::Grass,
        (None, false) => {
            return Err(format!(
                "missing `layer` (one of {})",
                ids(&Layer::ALL).join(", ")
            ));
        }
    };
    let radius = num_in(&args, "radius", 30.0, 1.0, 1500.0)?;
    let strength = num_in(&args, "strength", 1.0, 0.02, 1.0)?;
    let falloff = num_in(&args, "falloff", 0.5, 0.0, 1.0)?;
    let passes = passes(&args)?;
    let dabs = dabs(&args, (radius * 0.3).max(0.5))?;
    check_dabs(&d.terrain, &dabs, radius, passes)?;
    d.history.begin(if erase {
        "Erase paint".to_string()
    } else {
        format!("Paint {}", layer.name())
    });
    for _ in 0..passes {
        for c in &dabs {
            let a = BrushArgs {
                center: *c,
                radius,
                falloff,
                strength,
                dt: DAB_DT,
            };
            brush::paint_layer(&mut d.terrain, &mut d.history, &a, layer as usize, erase);
        }
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    // how much of the stroke's core carries the paint now
    let t = &*d.terrain;
    let (mut n, mut covered) = (0, 0);
    for c in dabs.iter().step_by((dabs.len() / 200).max(1)) {
        let g = t.world_to_grid(c.x, c.y);
        if t.in_bounds(c.x, c.y) {
            let s = t.splat[t.idx(g.x.round() as usize, g.y.round() as usize)];
            n += 1;
            let v = if erase {
                s.iter().map(|v| *v as u32).sum::<u32>() < 64
            } else {
                s[layer as usize] >= 128
            };
            covered += v as usize;
        }
    }
    let what = if erase {
        "cleared".to_string()
    } else {
        format!("mostly {}", layer.name())
    };
    text(format!(
        "Painted {} dabs × {passes} pass(es), radius {radius:.0} m: {}% of the stroke centre is {what}. One undo step.",
        dabs.len(),
        (covered * 100).checked_div(n).unwrap_or(0)
    ))
}

fn land(In(args): In<Value>, mut d: WorldData) -> R {
    let sink = match opt_str(&args, "mode")?.unwrap_or("raise") {
        "raise" => false,
        "sink" => true,
        m => return Err(format!("unknown mode `{m}` (raise or sink)")),
    };
    let land_height = num_in(&args, "land_height", d.ed.land_height, 2.0, 60.0)?;
    let rough = num_in(&args, "roughness", d.ed.coast_roughness, 0.0, 1.0)?;
    let seed = d.meta.seed;
    if let Some(outline) = opt_points(&args, "outline")? {
        if outline.len() < 3 {
            return Err("`outline` needs at least 3 points".into());
        }
        let (lo, hi) = outline
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
        let rect = d
            .terrain
            .rect_for_circle(
                (lo + hi) * 0.5,
                (hi - lo).length() * 0.5 + (hi - lo).max_element() * 0.12 + 60.0,
            )
            .ok_or("the outline lies outside the world")?;
        let before = heights_in(&d.terrain, rect);
        d.history.begin("Draw coastline");
        brush::fill_outline(
            &mut d.terrain,
            &mut d.history,
            &outline,
            sink,
            land_height,
            rough,
            seed,
        );
        d.history.commit(&d.store);
        d.meta.dirty = true;
        let what = if sink {
            "Carved the outlined area into the sea"
        } else {
            "Raised a new landmass from the outline"
        };
        return text(format!(
            "{what}. {} One undo step.",
            height_report(&d.terrain, rect, &before)
        ));
    }
    let radius = num_in(&args, "radius", 220.0, 10.0, 2000.0)?;
    let strength = num_in(&args, "strength", 0.6, 0.02, 1.0)?;
    let falloff = num_in(&args, "falloff", 0.6, 0.0, 1.0)?;
    let passes = passes(&args)?;
    let dabs = dabs(&args, (radius * 0.3).max(0.5))?;
    check_dabs(&d.terrain, &dabs, radius, passes)?;
    let rect = dab_rect(&d.terrain, &dabs, radius).ok_or("the stroke lies outside the world")?;
    let before = heights_in(&d.terrain, rect);
    d.history
        .begin(if sink { "Paint sea" } else { "Paint land" });
    for _ in 0..passes {
        for c in &dabs {
            let a = BrushArgs {
                center: *c,
                radius,
                falloff,
                strength,
                dt: DAB_DT,
            };
            brush::paint_land(
                &mut d.terrain,
                &mut d.history,
                &a,
                if sink { -1.0 } else { 1.0 },
                land_height,
                rough,
                seed,
            );
        }
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    text(format!(
        "{} {} dabs × {passes} pass(es). {} One undo step.",
        if sink { "Sank" } else { "Raised land with" },
        dabs.len(),
        height_report(&d.terrain, rect, &before)
    ))
}

fn set_biome(In(args): In<Value>, mut d: WorldData) -> R {
    use crate::biomes::{paint_ground_dab, paint_ground_masked, restyle_water};
    let preset = pick(
        &BiomePreset::ALL,
        opt_str(&args, "biome")?.ok_or("missing `biome`")?,
        "biome",
        |b| b.def().name,
    )?;
    let vegetation = opt_bool(&args, "vegetation")?.unwrap_or(true);
    let density = num_in(&args, "density", 1.0, 0.1, 3.0)?;
    let mut def = preset.def();
    def.density *= density;
    let biome = preset.id();
    let seed = d.meta.seed;
    let WorldData {
        terrain,
        history,
        store,
        water,
        nature,
        meta,
        ..
    } = &mut d;
    history.begin(format!("Biome: {}", def.name));
    // the area, to report what grows there
    let area: (Vec2, Vec2, Box<dyn Fn(Vec2) -> bool + Sync>);
    if let Some(poly) = opt_points(&args, "polygon")? {
        if poly.len() < 3 {
            return Err("`polygon` needs at least 3 points".into());
        }
        // the outline becomes a natural, wandering border (as the outline tool does)
        let shape = crate::visuals::ecotone::OrganicOutline::new(&poly, seed);
        let (lo, hi) = shape.bounds();
        let rect = terrain
            .rect_for_circle((lo + hi) * 0.5, ((hi - lo) * 0.5).max_element())
            .ok_or("the polygon lies outside the world")?;
        history.touch_terrain(terrain, rect);
        for j in rect.z0..=rect.z1 {
            for i in rect.x0..=rect.x1 {
                if shape.inside(terrain.grid_to_world(i, j)) {
                    let k = terrain.idx(i, j);
                    terrain.biomes[k] = biome;
                }
            }
        }
        terrain.mark_biome_dirty(rect);
        paint_ground_masked(terrain, history, &def, rect, seed, |t, i, j| {
            shape.ground_weight(t.grid_to_world(i, j))
        });
        if vegetation {
            crate::nature::regrow_area(nature, history, lo, hi, |p| shape.inside(p), density);
        }
        area = (lo, hi, Box::new(move |p| shape.inside(p)));
    } else if let Some(path) = opt_points(&args, "path")?.filter(|p| !p.is_empty()) {
        let r = num_in(&args, "radius", 150.0, 10.0, 1500.0)?;
        let dabs = along(&path, (r * 0.3).max(0.5));
        check_dabs(terrain, &dabs, r, 1)?;
        for c in &dabs {
            if let Some(rect) =
                brush::for_each_in_circle(terrain, history, *c, r, 0.0, |t, idx, _p, w| {
                    if w > 0.0 {
                        t.biomes[idx] = biome;
                    }
                })
            {
                terrain.mark_biome_dirty(rect);
            }
            paint_ground_dab(terrain, history, &def, *c, r * 1.1, 0.35, 0.35, seed);
        }
        let (lo, hi) = path.iter().fold((Vec2::MAX, Vec2::MIN), |(a, b), p| {
            (a.min(*p - r), b.max(*p + r))
        });
        let samples: Vec<Vec2> = dabs
            .iter()
            .step_by((dabs.len() / 400).max(1))
            .copied()
            .collect();
        if vegetation {
            crate::nature::regrow_area(
                nature,
                history,
                lo,
                hi,
                |p| samples.iter().any(|s| s.distance_squared(p) <= r * r),
                density,
            );
        }
        area = (
            lo,
            hi,
            Box::new(move |p| samples.iter().any(|s| s.distance_squared(p) <= r * r)),
        );
    } else {
        history.commit(store);
        return Err("give a `polygon` or a `path` (with `radius`)".into());
    }
    let restyled = restyle_water(terrain, water, history, &def, biome);
    history.commit(store);
    meta.dirty = true;
    // what grows there now, by category
    let mut grown = std::collections::BTreeMap::new();
    if let Some(ctx) = crate::nature::data_ctx(&d) {
        let n = crate::nature::place::count_area(&ctx, area.0, area.1, &area.2);
        for (c, k) in crate::nature::rules::CATS.iter().zip(n) {
            grown.insert(c.name().to_lowercase(), k);
        }
    }
    let grass = if def.grass.density <= 0.01 {
        "none (shrubs instead)".to_string()
    } else {
        format!(
            "{:.0}% of the meadow density, blades × {:.2}",
            def.grass.density * 100.0,
            def.grass.height
        )
    };
    json_out(json!({
        "biome": snake(&format!("{preset:?}")),
        // (procedural: what grows in the area now)
        "planted": grown.values().sum::<usize>(),
        "planted_by_category": grown,
        "density": density,
        "grass": grass,
        "lakes_rivers_restyled": restyled,
        "note": if vegetation { "biome map and ground set; painted nature edits inside cleared so the biome's own nature grows there (procedural); one undo step" } else { "biome map and ground set; one undo step" },
    }))
}

/// Letters for the ascii maps of biomes and zones.
const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

fn sample(In(args): In<Value>, d: WorldData) -> R {
    let t = &*d.terrain;
    let water = &*d.water;
    let biome_name = |id: u8| BiomePreset::from_id(id).map(|b| snake(&format!("{b:?}")));
    if let Some(pts) = opt_points(&args, "points")? {
        if pts.len() > 500 {
            return Err("at most 500 points per call".into());
        }
        let out: Vec<Value> = pts
            .iter()
            .map(|p| {
                if !t.in_bounds(p.x, p.y) {
                    return json!({ "x": p.x, "z": p.y, "outside_world": true });
                }
                let h = t.height_at(p.x, p.y);
                let slope = t.normal_at(p.x, p.y).y.clamp(-1.0, 1.0).acos().to_degrees();
                let ground = Vec3::new(p.x, h, p.y);
                let water_v = match water.water_level(ground) {
                    Some(l) if l > h + 0.02 => {
                        let kind = match water.body_at(t, *p) {
                            Some(crate::water::WaterSel::River(id)) => format!("river {id}"),
                            Some(crate::water::WaterSel::Lake(id)) => format!("lake {id}"),
                            _ => "lake/river".into(),
                        };
                        json!({ "kind": kind, "depth": round1(l - h), "surface": round1(l) })
                    }
                    _ if h < t.sea_level => json!({ "kind": "sea", "depth": round1(t.sea_level - h), "surface": round1(t.sea_level) }),
                    _ => Value::Null,
                };
                let zid = t.zone_at(p.x, p.y);
                let zone = d.zones.by_id(zid).map(|z| json!({ "id": z.id, "name": z.name })).unwrap_or(Value::Null);
                json!({
                    "x": p.x, "z": p.y, "height": round1(h), "above_sea": round1(h - t.sea_level), "slope_deg": round1(slope),
                    "water": water_v, "biome": biome_name(t.biome_at(p.x, p.y)), "zone": zone,
                    "ground": snake(&format!("{:?}", t.dominant_layer_at(p.x, p.y))),
                })
            })
            .collect();
        return json_out(json!({ "sea_level": t.sea_level, "points": out }));
    }
    let half = t.half();
    let lo = opt_xz(&args, "min")?
        .unwrap_or(Vec2::splat(-half))
        .max(Vec2::splat(-half));
    let hi = opt_xz(&args, "max")?
        .unwrap_or(Vec2::splat(half))
        .min(Vec2::splat(half));
    if hi.x <= lo.x || hi.y <= lo.y {
        return Err("`max` must be larger than `min` (and inside the world)".into());
    }
    let format = opt_str(&args, "format")?.unwrap_or("ascii");
    let ascii = match format {
        "ascii" => true,
        "values" => false,
        f => return Err(format!("unknown format `{f}` (ascii or values)")),
    };
    let cols = num(&args, "cols", if ascii { 48.0 } else { 16.0 })?
        .round()
        .clamp(2.0, MAX_COLS as f32) as usize;
    let cell = (hi.x - lo.x) / cols as f32;
    let rows = (((hi.y - lo.y) / cell).round() as usize).clamp(1, MAX_COLS);
    let cell_z = (hi.y - lo.y) / rows as f32;
    let xs: Vec<f32> = (0..cols).map(|c| lo.x + (c as f32 + 0.5) * cell).collect();
    let zs: Vec<f32> = (0..rows)
        .map(|r| lo.y + (r as f32 + 0.5) * cell_z)
        .collect();
    let r0 = |v: f32| (v * 10.0).round() / 10.0;
    if !ascii {
        let heights: Vec<Vec<f32>> = zs
            .iter()
            .map(|z| xs.iter().map(|x| r0(t.height_at(*x, *z))).collect())
            .collect();
        let mut out = json!({ "sea_level": t.sea_level, "x": xs.iter().map(|v| r0(*v)).collect::<Vec<_>>(), "z": zs.iter().map(|v| r0(*v)).collect::<Vec<_>>(), "height": heights });
        if opt_bool(&args, "slope")?.unwrap_or(false) {
            out["slope_deg"] = json!(
                zs.iter()
                    .map(|z| xs
                        .iter()
                        .map(|x| t
                            .normal_at(*x, *z)
                            .y
                            .clamp(-1.0, 1.0)
                            .acos()
                            .to_degrees()
                            .round())
                        .collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            );
        }
        return json_out(out);
    }
    let layer = opt_str(&args, "layer")?.unwrap_or("height");
    let mut legend = serde_json::Map::new();
    let mut map = String::with_capacity((cols + 1) * rows);
    match layer {
        "height" => {
            let top = t.heights.iter().fold(f32::MIN, |a, h| a.max(*h)) - t.sea_level;
            let th = height_bands(top);
            for (r, z) in zs.iter().enumerate() {
                if r > 0 {
                    map.push('\n');
                }
                for x in &xs {
                    let h = t.height_at(*x, *z);
                    let a = h - t.sea_level;
                    let wet = water
                        .water_level(Vec3::new(*x, h, *z))
                        .is_some_and(|l| l > h + 0.05);
                    map.push(if a < -15.0 {
                        '~'
                    } else if a < 0.0 {
                        '-'
                    } else if wet {
                        'w'
                    } else if a < th[0] {
                        '.'
                    } else if a < th[1] {
                        ':'
                    } else if a < th[2] {
                        '+'
                    } else if a < th[3] {
                        '^'
                    } else {
                        'M'
                    });
                }
            }
            legend.insert("~".into(), json!("sea deeper than 15 m"));
            legend.insert("-".into(), json!("sea 0-15 m deep"));
            legend.insert("w".into(), json!("lake or river"));
            let m = |v: f32| {
                if top < 20.0 {
                    format!("{v:.1}")
                } else {
                    format!("{v:.0}")
                }
            };
            for (k, i) in [(".", 0usize), (":", 1), ("+", 2), ("^", 3)] {
                let lo = if i == 0 { 0.0 } else { th[i - 1] };
                legend.insert(
                    k.into(),
                    json!(format!("land {}-{} m above sea", m(lo), m(th[i]))),
                );
            }
            legend.insert(
                "M".into(),
                json!(format!(
                    "{}-{} m above sea (the peak)",
                    m(th[3]),
                    m(top.max(th[3]))
                )),
            );
        }
        "biome" | "zone" => {
            let biome = layer == "biome";
            let mut seen = std::collections::BTreeSet::new();
            for (r, z) in zs.iter().enumerate() {
                if r > 0 {
                    map.push('\n');
                }
                for x in &xs {
                    let h = t.height_at(*x, *z);
                    let id = if biome {
                        t.biome_at(*x, *z)
                    } else {
                        t.zone_at(*x, *z)
                    };
                    let ch = if id > 0 {
                        seen.insert(id);
                        LETTERS[(id as usize - 1) % LETTERS.len()] as char
                    } else if h < t.sea_level {
                        '~'
                    } else {
                        '.'
                    };
                    map.push(ch);
                }
            }
            legend.insert("~".into(), json!("sea"));
            legend.insert(
                ".".into(),
                json!(if biome {
                    "land without a biome"
                } else {
                    "no zone"
                }),
            );
            for id in seen {
                let name = if biome {
                    biome_name(id).unwrap_or_default()
                } else {
                    d.zones
                        .by_id(id)
                        .map(|z| format!("zone {id}: {}", z.name))
                        .unwrap_or(format!("zone {id} (deleted)"))
                };
                legend.insert(
                    (LETTERS[(id as usize - 1) % LETTERS.len()] as char).to_string(),
                    json!(name),
                );
            }
        }
        l => return Err(format!("unknown layer `{l}` (height, biome or zone)")),
    }
    json_out(json!({
        "cols": cols, "rows": rows,
        "x_first": r0(xs[0]), "z_first": r0(zs[0]), "cell_m": r0(cell),
        "how_to_read": "character c of row r is at x = x_first + c·cell_m, z = z_first + r·cell_m (row 0 is north)",
        "sea_level": t.sea_level,
        "legend": legend,
        "map": map,
    }))
}

/// Upper bounds (m above sea) of the ascii height classes . : + ^ (M above the last), from the
/// world's peak `top`: rising, and all below the peak.
fn height_bands(top: f32) -> [f32; 4] {
    let top = top.max(0.4);
    let nice = |v: f32| {
        if top < 20.0 {
            (v * 10.0).round() / 10.0
        } else {
            v.round()
        }
    };
    let mut th = [(top * 0.1).min(10.0), top * 0.25, top * 0.5, top * 0.75].map(nice);
    for i in 1..4 {
        if th[i] <= th[i - 1] {
            th[i] = th[i - 1] + top * 0.05;
        }
    }
    th
}

pub(super) fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::height_bands;

    #[test]
    fn height_legend_bands_rise_and_stay_under_the_peak() {
        // (the report that prompted this: "M: over 80 m (peak 27 m)")
        for top in [0.5f32, 3.0, 12.0, 27.0, 80.0, 172.0, 640.0, 2400.0] {
            let th = height_bands(top);
            assert!(th.windows(2).all(|w| w[1] > w[0]), "{top}: {th:?}");
            assert!(th[3] < top, "{top}: {th:?}");
        }
        assert_eq!(height_bands(27.0), [3.0, 7.0, 14.0, 20.0]);
    }
}
