//! MCP tools of the area prompt: the selection (`selection_set/get/clear`, `selection_info`,
//! `selection_screenshot`), filling an area with one edit (`area_edit`: paint, sculpt in metres,
//! biome, nature, with height / slope filters and organic edges) and the high-level landforms
//! (`terrain_feature`).

use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use serde_json::{Value, json};

use super::features::{self, Volcano};
use super::selection::{self, MaskField, Selection, area_text, auto_feather, bounds};
use crate::biomes::BiomePreset;
use crate::camera::OrbitCamera;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};
use crate::nature::AreaOp;
use crate::nature::field::Target;
use crate::noise::{Rng, fbm, smoothstep, value2};
use crate::objects::{Category, presets};
use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer};

type R = Result<Vec<McpContent>, String>;

/// Tools the area prompt's agent may use (everything it needs to shape, paint, plant, add water
/// and look; nothing world-wide, no zones, saving, undo or settings).
pub const AGENT_TOOLS: &[&str] = &[
    "selection_info",
    "selection_screenshot",
    "terrain_feature",
    "area_edit",
    "terrain_sample",
    "terrain_sculpt",
    "terrain_paint",
    "terrain_set_biome",
    "nature_scatter",
    "objects_library",
    "objects_query",
    "objects_place",
    "objects_transform",
    "objects_remove",
    "objects_place_town",
    "water_presets",
    "water_list",
    "water_add_lake",
    "water_add_river",
    "water_set_style",
    "water_remove",
    "fish_add",
    "materials_list",
    "world_info",
    // Trace a Picture (image to world), for a map of the whole world
    "reference_load",
    "reference_analyse",
    "reference_label",
    "reference_merge",
    "reference_split",
    "reference_delete",
    "reference_build",
    "reference_overlay",
];

/// The tools an agent gets in a dungeon (Prompt an area while an underground dungeon is open):
/// looking at it, and its rooms, lights, pieces, decals and props — not what changes the whole
/// dungeon at once (layout, mood, style, Auto-light, Dress) or leaves it.
pub const DUNGEON_AGENT_TOOLS: &[&str] = &[
    "selection_info",
    "selection_screenshot",
    "dungeons_catalog",
    "dungeons_state",
    "dungeons_rooms",
    "dungeons_room_add",
    "dungeons_room_set",
    "dungeons_room_remove",
    "dungeons_wall_opening",
    "dungeons_gallery_hall",
    "dungeons_level",
    "dungeons_lights",
    "dungeons_light_add",
    "dungeons_light_set",
    "dungeons_light_remove",
    "dungeons_pieces",
    "dungeons_piece_add",
    "dungeons_piece_set",
    "dungeons_piece_remove",
    "dungeons_decals",
    "dungeons_decal_add",
    "dungeons_decal_remove",
    "dungeons_probe",
    "dungeons_walkable",
    "dungeons_wait",
    "objects_library",
    "objects_query",
    "objects_place",
    "objects_transform",
    "objects_remove",
];

/// The tools an agent gets when it shapes a whole new dungeon from a description (Dungeons ›
/// Create a dungeon › Describe it): the area tools, plus what changes the whole dungeon at once —
/// its layout, style, mood, music, Auto-light, Dress, aged walls, template doors, the way out and
/// its name. Still nothing that leaves the dungeon, saves or moves the camera.
pub const DUNGEON_SHAPE_TOOLS: &[&str] = &[
    "selection_info",
    "selection_screenshot",
    "dungeons_catalog",
    "dungeons_state",
    "dungeons_set",
    "dungeons_layout",
    "dungeons_style",
    "dungeons_mood",
    "dungeons_music",
    "dungeons_rooms",
    "dungeons_room_add",
    "dungeons_room_set",
    "dungeons_room_remove",
    "dungeons_wall_opening",
    "dungeons_wall",
    "dungeons_gallery_hall",
    "dungeons_level",
    "dungeons_lights",
    "dungeons_lights_auto",
    "dungeons_lights_all",
    "dungeons_light_add",
    "dungeons_light_set",
    "dungeons_light_remove",
    "dungeons_pieces",
    "dungeons_pieces_auto",
    "dungeons_piece_add",
    "dungeons_piece_set",
    "dungeons_piece_remove",
    "dungeons_decals",
    "dungeons_decals_age",
    "dungeons_decal_add",
    "dungeons_decal_remove",
    "dungeons_dress",
    "dungeons_way_out",
    "dungeons_minimap",
    "dungeons_probe",
    "dungeons_walkable",
    "dungeons_wait",
    "objects_library",
    "objects_query",
    "objects_place",
    "objects_transform",
    "objects_remove",
];

/// Feature names for `terrain_feature`.
const FEATURES: &[&str] = &[
    "volcano",
    "mountain_range",
    "ridge",
    "crater",
    "mesas",
    "buttes",
    "canyon",
    "valley",
    "dunes",
    "hills",
    "plateau",
    "sea_cliffs",
    "lagoon",
    "swamp",
    "lake",
];

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "selection_set",
        description: "Select an area of the world (like the editor's lasso, G). While a selection exists and masking is on, every edit — brushes, MCP tools, nature, objects, water — only changes the inside, fading out over a soft edge. \
Give `polygon` [[x, z], …], or `center` + `radius`, or `min` + `max`. `mask` (default true) turns edit masking on or off.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "polygon": pts_schema("outline [[x, z], …]"),
                "center": xz_schema("circle centre"), "radius": { "type": "number", "minimum": 10 },
                "min": xz_schema("rectangle corner (smallest x, z)"), "max": xz_schema("rectangle corner (largest x, z)"),
                "mask": { "type": "boolean", "default": true }
            } })
        },
        run: |w, a| run(w, selection_set, a),
    });
    app.mcp_tool(McpTool {
        name: "selection_get",
        description: "The selected area: outline, size and whether edits are masked to it (null when nothing is selected).",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, selection_get, a),
    });
    app.mcp_tool(McpTool {
        name: "selection_clear",
        description: "Clear the selection (edits are no longer masked).",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, _| {
            w.resource_mut::<Selection>().clear();
            Ok(vec![McpContent::Text("Selection cleared.".into())])
        },
    });
    app.mcp_tool(McpTool {
        name: "selection_info",
        description: "Everything about the selected area: outline, bounding box, size, height and slope statistics, sea / lake / river coverage, biomes and ground materials, zones, nature and object counts, an ascii height map, and suggested camera views. Start here.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, selection_info, a),
    });
    app.mcp_tool(McpTool {
        name: "selection_screenshot",
        description: "Look at the selected area: renders a top-down view and an angled 3/4 view framed on it (clear sky, editor UI hidden, the area's outline drawn thinly on the ground) and returns them as images; a close view shows plants and rocks up close. The camera returns to where it was. Use it to plan and to check your work.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "views": { "type": "array", "items": { "type": "string", "enum": ["top", "angled", "close"] }, "default": ["top", "angled"], "description": "close: a low view into the middle of the area, near enough to see plants, flowers and rocks" },
                "yaw_deg": { "type": "number", "description": "angled view: compass direction the camera looks in (0 = north, 90 = east); default 330" },
                "pitch_deg": { "type": "number", "minimum": 10, "maximum": 80, "default": 34, "description": "angled view: elevation" },
                "max_size": { "type": "integer", "minimum": 256, "maximum": 2048, "default": 1024, "description": "longest side of each image (px)" },
                "outline": { "type": "boolean", "default": true, "description": "draw the selection's outline on the ground" },
                "save_dir": { "type": "string", "description": "also save full-size PNGs into this absolute directory" }
            } })
        },
        run: selection_screenshot,
    });
    app.mcp_tool(McpTool {
        name: "area_edit",
        description: "Fill a whole area with one edit. The `area` defaults to the selection; it has a soft feathered edge (`feather_m`, default ~8% of its size, 15–60 m) and an organic, noisy outline (`edge_noise_m`, default 0.9 × feather; 0 for an exact edge). \
Filters limit it softly: `min_height` / `max_height` (m above sea level), `min_slope_deg` / `max_slope_deg`, `land_only` (e.g. rock on slopes over 35°, snow above 200 m). \
Ops: \"paint\" (`layer`, `strength` 0..1; `erase` returns to the automatic style), \"raise\" / \"lower\" by `metres` at full weight, \"flatten\" to `target_height` (m, absolute; default the area's median), \"smooth\" (`strength`), \"roughen\" (`amplitude_m`, `wavelength_m`), \"terrace\" (`step_m`), \
\"biome\" (`biome`; sets the biome map, its ground and vegetation), \"scatter\" (nature `preset` or `assets`, `density`), \"clear_nature\" (`amount` 0..1, optional `preset`/`assets` kinds), \"zone\" (`zone` id: only when the user asks for zones). Sculpt ops take absolute metres; `max_lift_m` caps any height change. One undo step.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "op": { "type": "string", "enum": ["paint", "raise", "lower", "flatten", "smooth", "roughen", "terrace", "biome", "scatter", "clear_nature", "zone"] },
                "area": area_schema(),
                "feather_m": { "type": "number", "minimum": 0, "maximum": 400 },
                "edge_noise_m": { "type": "number", "minimum": 0, "maximum": 300 },
                "min_height": { "type": "number", "description": "m above sea level" }, "max_height": { "type": "number", "description": "m above sea level" },
                "min_slope_deg": { "type": "number" }, "max_slope_deg": { "type": "number" },
                "land_only": { "type": "boolean", "default": false, "description": "skip sea, lakes and rivers" },
                "layer": { "type": "string", "enum": crate::terrain::layers::Layer::ALL.iter().map(|l| snake(&format!("{l:?}"))).collect::<Vec<_>>() },
                "erase": { "type": "boolean", "default": false },
                "strength": { "type": "number", "minimum": 0, "maximum": 1, "default": 1 },
                "variation": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.3, "description": "paint: how much the strength varies in soft patches (0 = uniform)" },
                "metres": { "type": "number", "minimum": 0, "maximum": 600, "default": 5 },
                "target_height": { "type": "number" },
                "max_lift_m": { "type": "number", "minimum": 0 },
                "amplitude_m": { "type": "number", "minimum": 0, "maximum": 200, "default": 4 },
                "wavelength_m": { "type": "number", "minimum": 4, "maximum": 2000, "default": 40 },
                "step_m": { "type": "number", "minimum": 1, "maximum": 80, "default": 10 },
                "radius_m": { "type": "number", "minimum": 1, "maximum": 40, "description": "smooth: blur radius (m), default 3 grid cells" },
                "biome": { "type": "string", "enum": BiomePreset::ALL.iter().map(|b| snake(&format!("{b:?}"))).collect::<Vec<_>>() },
                "vegetation": { "type": "boolean", "default": true },
                "preset": { "type": "string", "description": "nature preset (see nature_scatter)" },
                "assets": { "type": "array", "items": { "type": "string" } },
                "density": { "type": "number", "minimum": 0.05, "maximum": 3, "default": 1 },
                "amount": { "type": "number", "minimum": 0, "maximum": 1, "default": 1 },
                "zone": { "type": "integer" },
                "seed": { "type": "integer" }
            }, "required": ["op"] })
        },
        run: |w, a| run(w, area_edit, a),
    });
    app.mcp_tool(McpTool {
        name: "terrain_feature",
        description: "Build a realistic landform (deterministic with `seed`; sizes in metres; defaults fit the selection). \
volcano (`center`, `radius` of the foot, `height`, `crater_radius`, `crater_depth`, `lava`: lake | channels | both | none; black rock and ash ground, volcanic biome, glowing lava), \
mountain_range / ridge (along `path`, `width`, `height` added to the ground, eroded; peaks above ~260 m above sea get snow), crater (`center`, `radius`, `depth`), \
mesas / buttes (in `area`: `count`, `height`; sheer banded cliffs), canyon (along `path`: `depth`, `width`, `floor_width`, `river` true adds a river on its floor), valley (along `path`: `depth`, `width`, `shape` u | v), \
dunes (in `area`: `height`, `wavelength`, `wind_deg`; sand), hills (in `area`: `height`, `wavelength`), plateau (in `area`: `height`), sea_cliffs (along a coast `path`, default the coast in the selection: `height`, `width`), \
lagoon (`center`, `radius`, `depth`, `inlets`: a shallow basin behind a sand barrier, best on a coast), swamp (in `area`: low wet ground, channels and ponds filled with swamp water), lake (`center`, `radius`, `lake_type`, `island` true raises an island). \
`paint` (default true) paints the feature's ground. Paths default to the selection's long axis, centres to its middle, areas to the selection. One undo step.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "feature": { "type": "string", "enum": FEATURES },
                "center": xz_schema("centre (default: the selection's)"),
                "radius": { "type": "number", "minimum": 20, "maximum": 3000 },
                "path": pts_schema("centre line [[x, z], …] (default: the selection's long axis)"),
                "area": area_schema(),
                "height": { "type": "number", "minimum": 1, "maximum": 1200 },
                "depth": { "type": "number", "minimum": 0.5, "maximum": 600 },
                "width": { "type": "number", "minimum": 10, "maximum": 4000 },
                "floor_width": { "type": "number", "minimum": 4, "maximum": 1000 },
                "crater_radius": { "type": "number", "minimum": 5 },
                "crater_depth": { "type": "number", "minimum": 1 },
                "lava": { "type": "string", "enum": ["lake", "channels", "both", "none"], "default": "lake" },
                "count": { "type": "integer", "minimum": 1, "maximum": 40 },
                "wavelength": { "type": "number", "minimum": 10, "maximum": 3000 },
                "wind_deg": { "type": "number", "description": "dunes: compass direction the wind blows towards (0 = north, 90 = east); default 150" },
                "shape": { "type": "string", "enum": ["u", "v"], "default": "u" },
                "river": { "type": "boolean" },
                "river_preset": { "type": "string", "description": "water preset of the canyon's river (see water_presets)" },
                "inlets": { "type": "integer", "minimum": 0, "maximum": 4, "default": 1 },
                "lake_type": { "type": "string", "description": "lake: drawn lake type (lake, pond, alpine_lake, tarn, oasis, tropical_lagoon, crater_lake, …)" },
                "island": { "type": "boolean", "default": false },
                "erode": { "type": "boolean", "default": true, "description": "mountain_range / ridge: rain erosion afterwards" },
                "paint": { "type": "boolean", "default": true },
                "seed": { "type": "integer" }
            }, "required": ["feature"] })
        },
        run: |w, a| run(w, terrain_feature, a),
    });
}

fn run<M, S>(world: &mut World, sys: S, args: Value) -> R
where
    S: IntoSystem<In<Value>, R, M> + 'static,
{
    world
        .run_system_cached_with(sys, args)
        .map_err(|e| format!("{e}"))?
}

fn text(s: impl Into<String>) -> R {
    Ok(vec![McpContent::Text(s.into())])
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn xz_schema(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": desc })
}

fn pts_schema(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "description": desc })
}

fn area_schema() -> Value {
    json!({
        "description": "Where: the selection (default), a `polygon`, a circle (`center` + `radius`), a rectangle (`min` + `max`) or a band along a `path` (`radius` = half its width).",
        "anyOf": [
            { "type": "string", "enum": ["selection"] },
            { "type": "object", "properties": {
                "selection": { "type": "boolean", "description": "true: the selected area" },
                "polygon": pts_schema("outline [[x, z], …]"),
                "center": xz_schema("circle centre [x, z]"),
                "radius": { "type": "number", "minimum": 2, "description": "circle radius, or half the width of a path band (m)" },
                "min": xz_schema("rectangle corner with the smallest x and z"),
                "max": xz_schema("rectangle corner with the largest x and z"),
                "path": pts_schema("centre line of a band [[x, z], …]")
            } }
        ]
    })
}

fn num(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .filter(|f| f.is_some_and(|f| f.is_finite()))
            .ok_or_else(|| format!("`{key}` must be a number")),
    }
}

fn boolean(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

fn string<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn point(v: &Value) -> Result<Vec2, String> {
    let a = v
        .as_array()
        .filter(|a| a.len() >= 2)
        .ok_or("a point is [x, z]")?;
    match (a[0].as_f64(), a[a.len() - 1].as_f64()) {
        (Some(x), Some(z)) => Ok(Vec2::new(x as f32, z as f32)),
        _ => Err("a point is [x, z] numbers".into()),
    }
}

fn opt_point(args: &Value, key: &str) -> Result<Option<Vec2>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => point(v).map(Some).map_err(|e| format!("`{key}`: {e}")),
    }
}

fn opt_points(args: &Value, key: &str) -> Result<Option<Vec<Vec2>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(point)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
            .map_err(|e| format!("`{key}`: {e}")),
        Some(_) => Err(format!("`{key}` must be a list of [x, z] points")),
    }
}

fn circle(c: Vec2, r: f32) -> Vec<Vec2> {
    (0..48)
        .map(|k| c + Vec2::from_angle(k as f32 / 48.0 * std::f32::consts::TAU) * r)
        .collect()
}

/// A band of half-width `r` around a polyline, as a polygon.
fn band(path: &[Vec2], r: f32) -> Vec<Vec2> {
    if path.len() == 1 {
        return circle(path[0], r);
    }
    let pts = features::resample(path, (r * 0.5).max(2.0));
    let n = pts.len();
    let normal = |i: usize| {
        let a = pts[i.saturating_sub(1)];
        let b = pts[(i + 1).min(n - 1)];
        (b - a).perp().normalize_or_zero()
    };
    let mut out: Vec<Vec2> = (0..n).map(|i| pts[i] + normal(i) * r).collect();
    let dir = (pts[n - 1] - pts[n - 2]).normalize_or_zero();
    for k in 1..8 {
        out.push(
            pts[n - 1]
                + Vec2::from_angle(k as f32 / 8.0 * std::f32::consts::PI).rotate(normal(n - 1)) * r
                + dir * 0.0,
        );
    }
    out.extend((0..n).rev().map(|i| pts[i] - normal(i) * r));
    for k in 1..8 {
        out.push(
            pts[0] + Vec2::from_angle(k as f32 / 8.0 * std::f32::consts::PI).rotate(-normal(0)) * r,
        );
    }
    out
}

/// The polygon of an `area` argument (default: the selection).
fn area_poly(v: Option<&Value>, sel: &Selection) -> Result<Vec<Vec2>, String> {
    let selection = || {
        sel.polygon().map(|p| p.to_vec()).ok_or_else(|| {
            "nothing is selected: give an `area` (or select one with selection_set)".to_string()
        })
    };
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return selection();
    };
    if v.as_str() == Some("selection")
        || v.get("selection")
            .is_some_and(|s| s.as_bool() == Some(true))
    {
        return selection();
    }
    if let Some(p) = opt_points(v, "polygon")? {
        if p.len() < 3 {
            return Err("`polygon` needs at least 3 points".into());
        }
        return Ok(p);
    }
    if let Some(p) = opt_points(v, "path")? {
        let r = num(v, "radius")?.unwrap_or(30.0).max(2.0);
        if p.is_empty() {
            return Err("`path` needs a point".into());
        }
        return Ok(band(&p, r));
    }
    if let (Some(a), Some(b)) = (opt_point(v, "min")?, opt_point(v, "max")?) {
        let (a, b) = (a.min(b), a.max(b));
        return Ok(vec![a, Vec2::new(b.x, a.y), b, Vec2::new(a.x, b.y)]);
    }
    if let Some(c) = opt_point(v, "center")? {
        let r = num(v, "radius")?.ok_or("a circle needs `radius`")?.max(2.0);
        return Ok(circle(c, r));
    }
    Err("an area is \"selection\", {\"polygon\":…}, {\"center\":…,\"radius\":…}, {\"min\":…,\"max\":…} or {\"path\":…,\"radius\":…}".into())
}

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

// ------------------------------------------------------------------ selection tools

fn selection_set(In(args): In<Value>, mut sel: ResMut<Selection>, terrain: Res<Terrain>) -> R {
    let poly = if args.get("polygon").is_some()
        || args.get("center").is_some()
        || args.get("min").is_some()
    {
        area_poly(Some(&args), &sel)?
    } else {
        return Err("give `polygon`, `center` + `radius` or `min` + `max`".into());
    };
    sel.set(&terrain, &poly)?;
    if let Some(m) = boolean(&args, "mask")? {
        sel.mask_on = m;
    }
    let p = sel.polygon().unwrap_or_default();
    let (lo, hi) = bounds(p);
    text(format!(
        "Selected {} ({:.0} × {:.0} m, {} points) around [{:.0}, {:.0}]; edits are {}masked to it (soft edge {:.0} m).",
        area_text(selection::area(p)),
        hi.x - lo.x,
        hi.y - lo.y,
        p.len(),
        (lo.x + hi.x) * 0.5,
        (lo.y + hi.y) * 0.5,
        if sel.masking() { "" } else { "not " },
        auto_feather(p)
    ))
}

fn selection_get(In(_): In<Value>, sel: Res<Selection>) -> R {
    let Some(p) = sel.polygon() else {
        return Ok(vec![McpContent::Json(Value::Null)]);
    };
    let (lo, hi) = bounds(p);
    Ok(vec![McpContent::Json(json!({
        "polygon": p.iter().map(|q| [r1(q.x), r1(q.y)]).collect::<Vec<_>>(),
        "min": [r1(lo.x), r1(lo.y)], "max": [r1(hi.x), r1(hi.y)],
        "area_m2": selection::area(p).round(),
        "masking": sel.masking(),
        "feather_m": r1(auto_feather(p)),
    }))])
}

/// Camera views framing a polygon: (top, angled) as (focus, yaw, pitch, distance).
pub fn framing(
    t: &Terrain,
    poly: &[Vec2],
    yaw: f32,
    pitch: f32,
    aspect: f32,
) -> [(Vec3, f32, f32, f32); 2] {
    let (lo, hi) = bounds(poly);
    let c = (lo + hi) * 0.5;
    let ground = |p: Vec2| t.height_at(p.x, p.y).max(t.sea_level);
    let mut hs: Vec<f32> = (0..9)
        .map(|k| ground(lo + (hi - lo) * Vec2::new((k % 3) as f32 * 0.5, (k / 3) as f32 * 0.5)))
        .collect();
    hs.sort_by(|a, b| a.total_cmp(b));
    let focus = Vec3::new(c.x, hs[4], c.y);
    let tan = (25f32).to_radians().tan();
    let size = hi - lo;
    let top_d =
        ((size.y * 0.5) / tan).max((size.x * 0.5) / (tan * aspect)) * 1.12 + (hs[8] - hs[4]) * 0.5;
    let diag = size.length();
    let angled_d = diag * 0.5 / tan * 1.0;
    [
        (focus, 0.0, 1.52, top_d.max(60.0)),
        (focus, yaw, pitch, angled_d.max(80.0)),
    ]
}

fn selection_info(
    In(_): In<Value>,
    mut sel: ResMut<Selection>,
    d: WorldData,
    view: Res<crate::dungeons::render::DungeonView>,
) -> R {
    let poly = sel
        .polygon()
        .ok_or("nothing is selected (selection_set, or ask the user to lasso an area)")?
        .to_vec();
    // in an underground dungeon: its rooms, lights and pieces there
    if let Some(m) = d
        .dungeon_map
        .0
        .as_ref()
        .filter(|_| d.dungeon_map.interior())
    {
        return Ok(vec![McpContent::Json(crate::dungeons::selection_json(
            m, &poly, view.level,
        ))]);
    }
    let t = &*d.terrain;
    let masking = sel.masking();
    let feather = sel
        .field(t)
        .map(|f| f.feather)
        .unwrap_or_else(|| auto_feather(&poly));
    let (lo, hi) = bounds(&poly);
    let size = hi - lo;
    let cols = 40usize;
    let cell = size.x.max(size.y) / cols as f32;
    let rows = ((size.y / cell).round() as usize).clamp(1, 40);
    let cols = ((size.x / cell).round() as usize).clamp(1, 40);
    let mut hs = vec![];
    let mut slopes = vec![];
    let (mut sea, mut fresh, mut n) = (0usize, 0usize, 0usize);
    let mut biomes = std::collections::BTreeMap::<String, usize>::new();
    let mut ground = std::collections::BTreeMap::<String, usize>::new();
    let mut zones = std::collections::BTreeSet::new();
    let mut grid = vec![vec![None; cols]; rows];
    for (r, row) in grid.iter_mut().enumerate() {
        for (c, slot) in row.iter_mut().enumerate() {
            let p = Vec2::new(
                lo.x + (c as f32 + 0.5) * size.x / cols as f32,
                lo.y + (r as f32 + 0.5) * size.y / rows as f32,
            );
            if !crate::biomes::point_in_polygon(p, &poly) || !t.in_bounds(p.x, p.y) {
                continue;
            }
            n += 1;
            let h = t.height_at(p.x, p.y);
            let wet = d
                .water
                .water_level(Vec3::new(p.x, h, p.y))
                .is_some_and(|l| l > h + 0.05);
            if h < t.sea_level {
                sea += 1;
            } else if wet {
                fresh += 1;
            }
            hs.push(h);
            slopes.push(t.normal_at(p.x, p.y).y.clamp(-1.0, 1.0).acos().to_degrees());
            let b = BiomePreset::from_id(t.biome_at(p.x, p.y))
                .map(|b| snake(&format!("{b:?}")))
                .unwrap_or("none".into());
            *biomes.entry(b).or_default() += 1;
            *ground
                .entry(snake(&format!("{:?}", t.dominant_layer_at(p.x, p.y))))
                .or_default() += 1;
            let z = t.zone_at(p.x, p.y);
            if z > 0 {
                zones.insert(z);
            }
            *slot = Some((h, wet));
        }
    }
    if n == 0 {
        return Err("the selection lies outside the world".into());
    }
    let mut sorted = hs.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let pct = |q: f32| r1(sorted[((sorted.len() - 1) as f32 * q).round() as usize]);
    let share = |m: std::collections::BTreeMap<String, usize>| -> Value {
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        json!(
            v.into_iter()
                .map(|(k, c)| (k, json!(((c as f32 / n as f32) * 100.0).round())))
                .collect::<serde_json::Map<_, _>>()
        )
    };
    // ascii map: low … high relative to this area
    let (hmin, hmax) = (sorted[0], sorted[sorted.len() - 1]);
    let mut map = String::new();
    for row in &grid {
        for v in row {
            map.push(match v {
                None => ' ',
                Some((h, _)) if *h < t.sea_level - 8.0 => '~',
                Some((h, _)) if *h < t.sea_level => '-',
                Some((_, true)) => 'w',
                Some((h, _)) => {
                    let q = ((h - hmin.max(t.sea_level)) / (hmax - hmin.max(t.sea_level)).max(1.0))
                        .clamp(0.0, 0.999);
                    [b'.', b':', b'+', b'^', b'M'][(q * 5.0) as usize] as char
                }
            });
        }
        map.push('\n');
    }
    // nature and objects inside
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let c = (lo + hi) * 0.5;
    for id in d.store.query(c, size.length() * 0.5 + 10.0) {
        let Some(o) = d.store.items.get(&id) else {
            continue;
        };
        if !crate::biomes::point_in_polygon(Vec2::new(o.pos[0], o.pos[2]), &poly) {
            continue;
        }
        let cat = d
            .lib
            .as_deref()
            .and_then(|l| l.get(&o.asset))
            .map(|e| e.category.name().to_lowercase())
            .unwrap_or("other".into());
        *counts.entry(cat).or_default() += 1;
    }
    // procedural nature growing inside
    if let Some(ctx) = crate::nature::data_ctx(&d) {
        let n = crate::nature::place::count_area(&ctx, lo, hi, &|p| {
            crate::biomes::point_in_polygon(p, &poly)
        });
        for (c, k) in crate::nature::rules::CATS.iter().zip(n) {
            if k > 0 {
                *counts.entry(c.name().to_lowercase()).or_default() += k;
            }
        }
    }
    let inside = |p: Vec2| crate::biomes::point_in_polygon(p, &poly);
    let lakes: Vec<Value> = d
        .water
        .lakes
        .iter()
        .filter(|l| inside(Vec2::from_array(l.seed)))
        .map(|l| json!({ "id": l.id, "name": l.name, "level": r1(l.level) }))
        .collect();
    let rivers: Vec<Value> = d
        .water
        .rivers
        .iter()
        .filter(|r| r.points.iter().any(|p| inside(Vec2::new(p[0], p[2]))))
        .map(|r| json!({ "id": r.id, "name": r.name, "width": r1(r.width) }))
        .collect();
    let zone_list: Vec<Value> = zones.iter().map(|z| json!({ "id": z, "name": d.zones.by_id(*z).map(|z| z.name.clone()).unwrap_or_default() })).collect();
    let views = framing(t, &poly, -(330f32.to_radians()), 34f32.to_radians(), 1.6);
    let simplified: Vec<[f32; 2]> = poly
        .iter()
        .step_by(poly.len().div_ceil(48).max(1))
        .map(|p| [r1(p.x), r1(p.y)])
        .collect();
    let mean_slope = slopes.iter().sum::<f32>() / slopes.len() as f32;
    let max_slope = slopes.iter().fold(0f32, |a, b| a.max(*b));
    let steep = slopes.iter().filter(|s| **s > 30.0).count() as f32 / slopes.len() as f32;
    Ok(vec![McpContent::Json(json!({
        "polygon": simplified,
        "min": [r1(lo.x), r1(lo.y)], "max": [r1(hi.x), r1(hi.y)],
        "centre": [r1(c.x), r1(c.y)],
        "size_m": [size.x.round(), size.y.round()],
        "area": area_text(selection::area(&poly)),
        "edits_masked": masking,
        "soft_edge_m": r1(feather),
        "sea_level": t.sea_level,
        "snow_line_above_sea": t.style.snow_line,
        "height_m": { "min": pct(0.0), "p10": pct(0.1), "median": pct(0.5), "p90": pct(0.9), "max": pct(1.0) },
        "slope_deg": { "mean": r1(mean_slope), "max": r1(max_slope), "share_over_30": (steep * 100.0).round() },
        "water_percent": { "sea": ((sea as f32 / n as f32) * 100.0).round(), "lakes_rivers": ((fresh as f32 / n as f32) * 100.0).round() },
        "lakes": lakes, "rivers": rivers,
        "biomes_percent": share(biomes),
        "ground_percent": share(ground),
        "zones": zone_list,
        "objects": counts,
        "ascii_map": map,
        "ascii_legend": format!("row 0 = north edge (z {:.0}), columns west→east (x {:.0}…{:.0}); ' ' outside the area, ~ deep sea, - shallow sea, w lake/river, . : + ^ M = low → high ({:.0}…{:.0} m)", lo.y, lo.x, hi.x, hmin, hmax),
        "camera_views": {
            "top": { "focus": [r1(views[0].0.x), r1(views[0].0.z)], "pitch_deg": 87, "distance": views[0].3.round() },
            "angled": { "focus": [r1(views[1].0.x), r1(views[1].0.z)], "yaw_deg": 330, "pitch_deg": 34, "distance": views[1].3.round() }
        }
    }))])
}

/// Clean-sky captures in progress: the clouds and fog are held at zero meanwhile.
#[derive(Resource, Default)]
pub struct ShotSky(pub u32);

/// Holds the sky clear during `selection_screenshot` captures; resets counters a failed capture
/// left behind.
pub(crate) fn hold_clear_sky(
    mut shot: ResMut<ShotSky>,
    mut sky: ResMut<crate::sky::SkyState>,
    mcp: Option<Res<crate::mcp::McpServer>>,
    mut sel: ResMut<Selection>,
) {
    if crate::mcp::ui_visible(mcp) {
        shot.0 = 0;
        sel.shot_outline = 0;
        return;
    }
    if shot.0 > 0 {
        let w = &mut sky.weather;
        w.clouds = 0.0;
        w.fog = 0.0;
        w.rain = 0.0;
        w.snow = 0.0;
        w.dust = 0.0;
    }
}

fn selection_screenshot(world: &mut World, args: Value) -> R {
    let poly = world
        .resource::<Selection>()
        .polygon()
        .ok_or("nothing is selected")?
        .to_vec();
    let views: Vec<String> =
        opt_arg(&args, "views")?.unwrap_or_else(|| vec!["top".into(), "angled".into()]);
    if views.is_empty()
        || views
            .iter()
            .any(|v| !["top", "angled", "close"].contains(&v.as_str()))
    {
        return Err("`views` lists \"top\", \"angled\" and/or \"close\"".into());
    }
    // compass heading the camera looks in → the orbit camera's yaw (its offset from the focus)
    let yaw = -num(&args, "yaw_deg")?.unwrap_or(330.0).to_radians();
    let pitch = num(&args, "pitch_deg")?
        .unwrap_or(34.0)
        .clamp(10.0, 80.0)
        .to_radians();
    let max = num(&args, "max_size")?
        .unwrap_or(1024.0)
        .clamp(256.0, 2048.0) as u32;
    let outline = boolean(&args, "outline")?.unwrap_or(true);
    let save_dir = string(&args, "save_dir")?.map(std::path::PathBuf::from);
    if save_dir.as_ref().is_some_and(|d| !d.is_absolute()) {
        return Err("`save_dir` must be an absolute path".into());
    }
    let aspect = world
        .query::<&Window>()
        .iter(world)
        .next()
        .map(|w| w.width() / w.height().max(1.0))
        .unwrap_or(1.6);
    let mut frames = framing(world.resource::<Terrain>(), &poly, yaw, pitch, aspect);
    let floor = world.resource::<Selection>().dungeon_floor;
    let close = {
        let (focus, _, _, _) = frames[1];
        let (lo, hi) = bounds(&poly);
        (
            focus,
            yaw,
            0.32,
            ((hi - lo).max_element() * 0.18).clamp(70.0, 260.0),
        )
    };
    // in a dungeon: framed on the edited level's floor, as near as its rooms need (the builder's
    // view is cut away above the level, so the rooms show from above)
    let close = match floor {
        Some(f) => {
            let (lo, hi) = bounds(&poly);
            let size = (hi - lo).max_element();
            for fr in frames.iter_mut() {
                fr.0.y = f;
            }
            frames[0].3 = (size * 1.15).max(12.0);
            frames[1].3 = (size * 1.05).max(14.0);
            (
                Vec3::new(close.0.x, f, close.0.z),
                yaw,
                0.55,
                (size * 0.45).clamp(8.0, 40.0),
            )
        }
        None => close,
    };
    let shots: Vec<(String, (Vec3, f32, f32, f32))> = views
        .iter()
        .map(|v| {
            (
                v.clone(),
                match v.as_str() {
                    "top" => frames[0],
                    "angled" => frames[1],
                    _ => close,
                },
            )
        })
        .collect();
    let saved_cam = world.resource::<OrbitCamera>().clone();
    world.resource_mut::<crate::mcp::McpServer>().hide_ui += 1;
    world.resource_mut::<ShotSky>().0 += 1;
    if outline {
        world.resource_mut::<Selection>().shot_outline += 1;
    }
    let slot: Arc<Mutex<Option<Image>>> = Arc::default();
    let (mut k, mut frame, mut asked) = (0usize, 0u32, false);
    let mut out: Vec<McpContent> = vec![];
    let restore = move |w: &mut World, cam: &OrbitCamera| {
        *w.resource_mut::<OrbitCamera>() = cam.clone();
        let mut s = w.resource_mut::<crate::mcp::McpServer>();
        s.hide_ui = s.hide_ui.saturating_sub(1);
        let mut sky = w.resource_mut::<ShotSky>();
        sky.0 = sky.0.saturating_sub(1);
        if outline {
            let mut sel = w.resource_mut::<Selection>();
            sel.shot_outline = sel.shot_outline.saturating_sub(1);
        }
    };
    Ok(vec![McpContent::deferred(move |w| {
        let (name, (focus, yaw, pitch, dist)) = shots[k].clone();
        if frame == 0 {
            let mut o = w.resource_mut::<OrbitCamera>();
            o.set_view(focus, yaw, pitch, dist);
            o.snap();
        }
        frame += 1;
        // the camera snaps; give streaming, LODs and shadows time to settle
        if !asked {
            if frame >= 36 {
                let slot = slot.clone();
                w.spawn(Screenshot::primary_window())
                    .observe(move |ev: On<ScreenshotCaptured>| {
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(ev.image.clone());
                    });
                asked = true;
            }
            return None;
        }
        let Some(img) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            if frame > 600 {
                restore(w, &saved_cam);
                return Some(Err(
                    "The screenshot never arrived (is the editor window minimised?)".into(),
                ));
            }
            return None;
        };
        let full = match img.try_into_dynamic() {
            Ok(d) => d.into_rgb8(),
            Err(e) => {
                restore(w, &saved_cam);
                return Some(Err(format!("could not read the screenshot: {e:?}")));
            }
        };
        if let Some(dir) = &save_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = full.save(dir.join(format!("{name}.png")));
        }
        let (fw, fh) = full.dimensions();
        let s = (max as f32 / fw.max(fh) as f32).min(1.0);
        let small = image::imageops::resize(
            &full,
            ((fw as f32 * s) as u32).max(1),
            ((fh as f32 * s) as u32).max(1),
            image::imageops::FilterType::Triangle,
        );
        let mut png = std::io::Cursor::new(Vec::new());
        if small.write_to(&mut png, image::ImageFormat::Png).is_ok() {
            out.push(McpContent::Png(png.into_inner()));
            let what = match name.as_str() {
                "top" => "Top-down view (north up)".to_string(),
                "close" => format!(
                    "Close view into the middle, looking {:.0}° (compass)",
                    (-yaw.to_degrees()).rem_euclid(360.0)
                ),
                _ => format!(
                    "Angled view looking {:.0}° (compass), pitch {:.0}°",
                    (-yaw.to_degrees()).rem_euclid(360.0),
                    pitch.to_degrees()
                ),
            };
            out.push(McpContent::Text(format!(
                "{what}, focus [{:.0}, {:.0}], distance {:.0} m.",
                focus.x, focus.z, dist
            )));
        }
        k += 1;
        frame = 0;
        asked = false;
        if k < shots.len() {
            return None;
        }
        restore(w, &saved_cam);
        Some(Ok(std::mem::take(&mut out)))
    })])
}

// ------------------------------------------------------------------ area_edit

/// Soft filter weights for `area_edit` (1 passes).
struct Filter {
    min_h: Option<f32>,
    max_h: Option<f32>,
    min_s: Option<f32>,
    max_s: Option<f32>,
    land: bool,
}

impl Filter {
    fn weight(&self, t: &Terrain, water: &crate::water::WaterBodies, i: usize, j: usize) -> f32 {
        let h = t.h(i, j);
        let alt = h - t.sea_level;
        let mut w = 1.0;
        if let Some(m) = self.min_h {
            w *= smoothstep(m - 3.0, m + 3.0, alt);
        }
        if let Some(m) = self.max_h {
            w *= 1.0 - smoothstep(m - 3.0, m + 3.0, alt);
        }
        if self.min_s.is_some() || self.max_s.is_some() {
            let s = t
                .normal_at_grid(i, j)
                .y
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            if let Some(m) = self.min_s {
                w *= smoothstep(m - 2.5, m + 2.5, s);
            }
            if let Some(m) = self.max_s {
                w *= 1.0 - smoothstep(m - 2.5, m + 2.5, s);
            }
        }
        if self.land {
            let p = t.grid_to_world(i, j);
            let wet = alt < 0.2
                || water
                    .water_level(Vec3::new(p.x, h, p.y))
                    .is_some_and(|l| l > h + 0.05);
            if wet {
                w = 0.0;
            }
        }
        w
    }
}

#[allow(clippy::too_many_lines)]
fn area_edit(In(args): In<Value>, mut d: WorldData, sel: Res<Selection>) -> R {
    let op = string(&args, "op")?.ok_or("missing `op`")?.to_string();
    let poly = area_poly(args.get("area"), &sel)?;
    let feather = num(&args, "feather_m")?
        .unwrap_or_else(|| auto_feather(&poly))
        .max(0.0);
    let edge_noise = num(&args, "edge_noise_m")?
        .unwrap_or(feather * 0.9)
        .max(0.0);
    let seed = num(&args, "seed")?
        .map(|s| s as u32)
        .unwrap_or_else(|| d.meta.seed.wrapping_add(fastrand::u32(..1000)));
    let filter = Filter {
        min_h: num(&args, "min_height")?,
        max_h: num(&args, "max_height")?,
        min_s: num(&args, "min_slope_deg")?,
        max_s: num(&args, "max_slope_deg")?,
        land: boolean(&args, "land_only")?.unwrap_or(false),
    };
    let field = MaskField::build(&d.terrain, &poly, feather, edge_noise, false, seed)
        .ok_or("the area lies outside the world")?;
    let rect = field.rect;
    // weights over the rect
    let w = rect.width();
    let weights: Vec<f32> = {
        let t = &*d.terrain;
        let water = &*d.water;
        (rect.z0..=rect.z1)
            .flat_map(|j| (rect.x0..=rect.x1).map(move |i| (i, j)))
            .map(|(i, j)| {
                let m = field.at_grid(i, j);
                if m <= 0.0 {
                    0.0
                } else {
                    m * filter.weight(t, water, i, j)
                }
            })
            .collect()
    };
    let wt = |i: usize, j: usize| weights[(j - rect.z0) * w + (i - rect.x0)];
    let covered: f32 =
        weights.iter().filter(|v| **v > 0.5).count() as f32 * d.terrain.cell * d.terrain.cell;
    if covered <= 0.0 && weights.iter().all(|v| *v <= 0.01) {
        return Err(
            "nothing in the area passes the filters (check min/max height and slope)".into(),
        );
    }
    let strength = num(&args, "strength")?.unwrap_or(1.0).clamp(0.0, 1.0);
    let max_lift = num(&args, "max_lift_m")?;
    let WorldData {
        terrain,
        history,
        store,
        water,
        zones,
        lib,
        meta,
        amounts,
        appearance,
        nature,
        nature_rules,
        nature_masks,
        ..
    } = &mut d;
    let t = &mut **terrain;
    let label = format!("Area: {op}");
    history.begin(label.clone());
    let res: Result<String, String> = (|| {
        Ok(match op.as_str() {
            "paint" => {
                let erase = boolean(&args, "erase")?.unwrap_or(false);
                let layer = match string(&args, "layer")? {
                    Some(l) => Some(
                        Layer::ALL
                            .into_iter()
                            .find(|x| {
                                norm(&format!("{x:?}")) == norm(l) || norm(x.name()) == norm(l)
                            })
                            .ok_or_else(|| format!("unknown layer `{l}`"))?,
                    ),
                    None if erase => None,
                    None => return Err("missing `layer` (or `erase`: true)".into()),
                };
                // large areas of one solid layer look stamped: the strength varies in soft patches
                let variation = num(&args, "variation")?.unwrap_or(0.3).clamp(0.0, 1.0);
                history.touch_terrain(t, rect);
                for j in rect.z0..=rect.z1 {
                    for i in rect.x0..=rect.x1 {
                        let p = t.grid_to_world(i, j);
                        let patch = fbm(p.x / 45.0, p.y / 45.0, 3, 2.0, 0.5, seed + 77) * 0.5 + 0.5;
                        let k = wt(i, j)
                            * strength
                            * (1.0 - variation + variation * smoothstep(0.25, 0.75, patch));
                        if k <= 0.003 {
                            continue;
                        }
                        let idx = t.idx(i, j);
                        let s = &mut t.splat[idx];
                        for (l, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
                            let target = if layer.is_some_and(|x| x as usize == l) {
                                255.0
                            } else {
                                0.0
                            };
                            *v = (*v as f32 + (target - *v as f32) * k)
                                .round()
                                .clamp(0.0, 255.0) as u8;
                        }
                        let sum: u32 = s.iter().map(|v| *v as u32).sum();
                        if sum > 255 {
                            let f = 255.0 / sum as f32;
                            s.iter_mut()
                                .for_each(|v| *v = (*v as f32 * f).floor() as u8);
                        }
                    }
                }
                t.mark_splat_dirty(rect);
                match layer {
                    Some(l) => format!(
                        "Painted {} over {} of ground.",
                        l.name(),
                        area_text(covered)
                    ),
                    None => format!(
                        "Returned {} of ground to the automatic style.",
                        area_text(covered)
                    ),
                }
            }
            "raise" | "lower" | "flatten" | "smooth" | "roughen" | "terrace" => {
                history.touch_terrain(t, rect);
                let before: Vec<f32> = (rect.z0..=rect.z1)
                    .flat_map(|j| (rect.x0..=rect.x1).map(move |i| (i, j)))
                    .map(|(i, j)| t.h(i, j))
                    .collect();
                let at = |i: usize, j: usize| before[(j - rect.z0) * w + (i - rect.x0)];
                let metres = num(&args, "metres")?.unwrap_or(5.0).clamp(0.0, 600.0);
                let target = match num(&args, "target_height")? {
                    Some(v) => v,
                    None => {
                        let mut hs: Vec<f32> = weights
                            .iter()
                            .zip(&before)
                            .filter(|(w, _)| **w > 0.5)
                            .map(|(_, h)| *h)
                            .collect();
                        hs.sort_by(|a, b| a.total_cmp(b));
                        hs.get(hs.len() / 2).copied().unwrap_or(t.sea_level)
                    }
                };
                let amp = num(&args, "amplitude_m")?.unwrap_or(4.0);
                let wl = num(&args, "wavelength_m")?.unwrap_or(40.0).max(4.0);
                let step = num(&args, "step_m")?.unwrap_or(10.0).max(1.0);
                let kr = ((num(&args, "radius_m")?.unwrap_or(t.cell * 3.0) / t.cell).round()
                    as i32)
                    .clamp(1, 8);
                let n = t.n() as i32;
                let (mut up, mut down) = (0f32, 0f32);
                for j in rect.z0..=rect.z1 {
                    for i in rect.x0..=rect.x1 {
                        let k = wt(i, j);
                        if k <= 0.0 {
                            continue;
                        }
                        let old = at(i, j);
                        let p = t.grid_to_world(i, j);
                        let mut new = match op.as_str() {
                            "raise" => old + metres * k,
                            "lower" => old - metres * k,
                            "flatten" => old + (target - old) * k * strength,
                            "roughen" => old + fbm(p.x / wl, p.y / wl, 4, 2.0, 0.5, seed) * amp * k,
                            "terrace" => {
                                let jit =
                                    (value2(p.x * 0.01, p.y * 0.01, seed) - 0.5) * step * 0.35;
                                let x = (old + jit) / step;
                                let tgt =
                                    (x.floor() + smoothstep(0.55, 0.85, x.fract())) * step - jit;
                                old + (tgt - old) * k * strength
                            }
                            _ => {
                                let (mut s, mut c) = (0.0, 0.0);
                                for dz in -kr..=kr {
                                    for dx in -kr..=kr {
                                        let (x, z) = (
                                            (i as i32 + dx).clamp(0, n - 1) as usize,
                                            (j as i32 + dz).clamp(0, n - 1) as usize,
                                        );
                                        s += if x >= rect.x0
                                            && x <= rect.x1
                                            && z >= rect.z0
                                            && z <= rect.z1
                                        {
                                            at(x, z)
                                        } else {
                                            t.h(x, z)
                                        };
                                        c += 1.0;
                                    }
                                }
                                old + (s / c - old) * k * strength
                            }
                        };
                        if let Some(m) = max_lift {
                            new = new.clamp(old - m, old + m);
                        }
                        up = up.max(new - old);
                        down = down.min(new - old);
                        let idx = t.idx(i, j);
                        t.heights[idx] = new;
                    }
                }
                t.mark_heights_dirty(rect);
                format!(
                    "{op}: ground moved up to {up:+.1} m / {down:+.1} m over {}.",
                    area_text(covered)
                )
            }
            "biome" => {
                let b = string(&args, "biome")?.ok_or("missing `biome`")?;
                let preset = BiomePreset::ALL
                    .into_iter()
                    .find(|p| norm(&format!("{p:?}")) == norm(b) || norm(p.def().name) == norm(b))
                    .ok_or_else(|| format!("unknown biome `{b}`"))?;
                let def = preset.def();
                let id = preset.id();
                history.touch_terrain(t, rect);
                for j in rect.z0..=rect.z1 {
                    for i in rect.x0..=rect.x1 {
                        let p = t.grid_to_world(i, j);
                        if wt(i, j) > 0.3 + 0.4 * value2(p.x / 14.0, p.y / 14.0, seed + 5) {
                            let k = t.idx(i, j);
                            t.biomes[k] = id;
                        }
                    }
                }
                t.mark_biome_dirty(rect);
                crate::biomes::paint_ground_masked(t, history, &def, rect, meta.seed, |_, i, j| {
                    wt(i, j)
                });
                let vegetation = boolean(&args, "vegetation")?.unwrap_or(true);
                if vegetation {
                    // painted nature edits inside go: the biome's own forests and plants grow there
                    let (lo, hi) = bounds(&poly);
                    crate::nature::regrow_area(
                        nature,
                        history,
                        lo - feather,
                        hi + feather,
                        |p| field.at(p) > 0.5,
                        1.0,
                    );
                }
                // (lava keeps its glow)
                let lava: Vec<(u32, crate::water::WaterStyle)> = water
                    .lakes
                    .iter()
                    .map(|l| (l.id, &l.style))
                    .chain(water.rivers.iter().map(|r| (r.id, &r.style)))
                    .filter(|(_, s)| s.preset == "lava")
                    .map(|(i, s)| (i, s.clone()))
                    .collect();
                let restyled = crate::biomes::restyle_water(t, water, history, &def, id);
                for (i, st) in lava {
                    if let Some(l) = water.lakes.iter_mut().find(|l| l.id == i) {
                        l.style = st.clone();
                    }
                    if let Some(r) = water.rivers.iter_mut().find(|r| r.id == i) {
                        r.style = st;
                    }
                }
                format!(
                    "{}: {} of biome map and ground{}; {restyled} lakes/rivers restyled.",
                    def.name,
                    area_text(covered),
                    if vegetation {
                        "; its forests, plants and rocks grow there (procedural)"
                    } else {
                        ""
                    }
                )
            }
            "scatter" | "clear_nature" => {
                let lib = lib.as_deref().ok_or("the asset library is still loading")?;
                let rules = nature_rules
                    .as_deref()
                    .ok_or("the asset library is still loading")?;
                let all = presets();
                let preset = match string(&args, "preset")? {
                    Some(p) => Some(
                        all.iter()
                            .position(|x| norm(&x.name) == norm(p))
                            .ok_or_else(|| format!("unknown nature preset `{p}`"))?,
                    ),
                    None => None,
                };
                let assets: Vec<String> = opt_arg(&args, "assets")?.unwrap_or_default();
                assets.iter().try_for_each(|a| crate::market::check(a))?;
                let kinds = assets
                    .iter()
                    .map(|a| match lib.get(a) {
                        Some(e) if Category::NATURE.contains(&e.category) && e.kind.is_some() => {
                            Ok(e.kind.unwrap())
                        }
                        Some(e) => Err(format!(
                            "`{a}` is a {}: place it with objects_place",
                            e.category.name().to_lowercase()
                        )),
                        None => Err(format!("unknown asset `{a}` (see objects_library)")),
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let (lo, hi) = bounds(&poly);
                let (lo, hi) = (lo - feather, hi + feather);
                // the area's weight (mask, filters) at a spot
                let weight = |p: Vec2| -> f32 {
                    let g = t.world_to_grid(p.x, p.y);
                    let (i, j) = (g.x.round().max(0.0) as usize, g.y.round().max(0.0) as usize);
                    if i >= rect.x0 && i <= rect.x1 && j >= rect.z0 && j <= rect.z1 {
                        wt(i, j)
                    } else {
                        0.0
                    }
                };
                // ragged edges: copses reach out, glades bite in
                let inside = |p: Vec2| {
                    let w = weight(p);
                    let clump = fbm(p.x / 60.0, p.y / 60.0, 3, 2.0, 0.5, seed + 78) * 0.5;
                    w > 0.0 && w + clump * 4.0 * w * (1.0 - w) > 0.5
                };
                let count = |field: &crate::nature::NatureField| -> usize {
                    let ctx = crate::nature::Ctx {
                        terrain: t,
                        water,
                        zones,
                        lib,
                        field,
                        rules,
                        masks: nature_masks,
                        amounts: crate::nature::amounts(amounts),
                        flora_amounts: amounts.flora_amounts(),
                        appearance: **appearance,
                        seed: meta.seed,
                    };
                    crate::nature::place::count_area(&ctx, lo, hi, &|p| weight(p) > 0.5)
                        .iter()
                        .sum()
                };
                let before = count(nature);
                if op == "clear_nature" {
                    let amount = num(&args, "amount")?.unwrap_or(1.0).clamp(0.0, 1.0);
                    let target = match preset {
                        Some(i) => Target::Species(
                            rules.presets[i]
                                .cats
                                .iter()
                                .flatten()
                                .map(|l| l.kind)
                                .collect(),
                        ),
                        None if !kinds.is_empty() => Target::Species(kinds.clone()),
                        None => Target::All,
                    };
                    crate::nature::paint_region(
                        nature,
                        history,
                        &label,
                        lo,
                        hi,
                        inside,
                        &target,
                        AreaOp::Keep(1.0 - amount),
                    );
                    // placed (pinned) nature objects too
                    let mut rng = Rng::new(seed as u64 ^ 0xA5A5);
                    let mut n = 0;
                    for id in store.query((lo + hi) * 0.5, (hi - lo).length() * 0.5 + 10.0) {
                        let Some(o) = store.items.get(&id) else {
                            continue;
                        };
                        let Some(e) = lib.get(&o.asset) else { continue };
                        let matches = match &target {
                            Target::Species(ks) => e.kind.is_some_and(|k| ks.contains(&k)),
                            _ => Category::NATURE.contains(&e.category),
                        };
                        if matches && rng.f32() < weight(Vec2::new(o.pos[0], o.pos[2])) * amount {
                            history.touch_object(store, id);
                            store.remove(id);
                            n += 1;
                        }
                    }
                    let after = count(nature);
                    format!(
                        "Cleared nature over {}: {} trees, plants and rocks grow there now (was {before}){}.",
                        area_text(covered),
                        after,
                        if n > 0 {
                            format!("; {n} placed ones removed")
                        } else {
                            String::new()
                        }
                    )
                } else {
                    let density = num(&args, "density")?.unwrap_or(1.0).clamp(0.05, 3.0);
                    let target = match preset {
                        Some(i) => Target::Preset(i as u8),
                        None if !kinds.is_empty() => Target::Species(kinds.clone()),
                        None => return Err("scatter needs a nature `preset` or `assets`".into()),
                    };
                    crate::nature::paint_region(
                        nature,
                        history,
                        &label,
                        lo,
                        hi,
                        inside,
                        &target,
                        AreaOp::Add(density),
                    );
                    let after = count(nature);
                    format!(
                        "Planted over {}: {after} trees, plants and rocks grow there now (was {before}).",
                        area_text(covered)
                    )
                }
            }
            "zone" => {
                let id = num(&args, "zone")?.ok_or("missing `zone` (an existing zone id)")? as u8;
                if zones.by_id(id).is_none() {
                    return Err(format!(
                        "there is no zone {id} (zones are created by the user)"
                    ));
                }
                history.touch_terrain(t, rect);
                for j in rect.z0..=rect.z1 {
                    for i in rect.x0..=rect.x1 {
                        if wt(i, j) > 0.5 {
                            let k = t.idx(i, j);
                            t.zones[k] = id;
                        }
                    }
                }
                t.mark_zone_dirty(rect);
                format!("Zone {id} now covers {} more.", area_text(covered))
            }
            o => return Err(format!("unknown op `{o}`")),
        })
    })();
    history.commit(store);
    let report = res?;
    meta.dirty = true;
    text(format!("{report} One undo step."))
}

// ------------------------------------------------------------------ terrain_feature

/// The selection's long axis as a 3-point path across it (80% of its length).
fn long_axis(poly: &[Vec2]) -> Vec<Vec2> {
    let c = poly.iter().copied().sum::<Vec2>() / poly.len() as f32;
    let (mut xx, mut xz, mut zz) = (0.0, 0.0, 0.0);
    for p in poly {
        let d = *p - c;
        xx += d.x * d.x;
        xz += d.x * d.y;
        zz += d.y * d.y;
    }
    let ang = 0.5 * (2.0 * xz).atan2(xx - zz);
    let dir = Vec2::from_angle(ang);
    let ext = poly
        .iter()
        .map(|p| (*p - c).dot(dir).abs())
        .fold(0.0, f32::max)
        * 0.8;
    let bend = dir.perp() * ext * 0.12;
    vec![c - dir * ext, c + bend, c + dir * ext]
}

/// Points along the coastline inside the selection, ordered along its long axis.
fn coast_in(t: &Terrain, poly: &[Vec2]) -> Option<Vec<Vec2>> {
    let axis = long_axis(poly);
    let dir = (axis[2] - axis[0]).normalize_or_zero();
    let (lo, hi) = bounds(poly);
    let step = (t.cell * 2.0).max(6.0);
    let mut pts = vec![];
    let mut z = lo.y;
    while z <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            let p = Vec2::new(x, z);
            if crate::biomes::point_in_polygon(p, poly) {
                let h = t.height_at(p.x, p.y) - t.sea_level;
                let hx = t.height_at(p.x + step, p.y) - t.sea_level;
                let hz = t.height_at(p.x, p.y + step) - t.sea_level;
                if (h >= 0.0) != (hx >= 0.0) || (h >= 0.0) != (hz >= 0.0) {
                    pts.push(p);
                }
            }
            x += step;
        }
        z += step;
    }
    if pts.len() < 4 {
        return None;
    }
    pts.sort_by(|a, b| a.dot(dir).total_cmp(&b.dot(dir)));
    // average into ~24 stations along the axis
    let n = 24.min(pts.len() / 2).max(2);
    let per = pts.len().div_ceil(n);
    Some(
        pts.chunks(per)
            .map(|c| c.iter().copied().sum::<Vec2>() / c.len() as f32)
            .collect(),
    )
}

/// The nearest open sea from `c` within `reach`: (the coast point, direction towards the sea).
fn nearest_sea(t: &Terrain, c: Vec2, reach: f32) -> (Option<Vec2>, Vec2) {
    let mut best: Option<(f32, Vec2)> = None;
    for k in 0..48 {
        let dir = Vec2::from_angle(k as f32 / 48.0 * std::f32::consts::TAU);
        let mut d = 0.0;
        while d < reach {
            let p = c + dir * d;
            if t.height_at(p.x, p.y) < t.sea_level - 1.0 {
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, dir));
                }
                break;
            }
            d += t.cell * 2.0;
        }
    }
    match best {
        Some((d, dir)) => (Some(c + dir * d), dir),
        None => {
            let n = t.normal_at(c.x, c.y);
            (None, Vec2::new(n.x, n.z).normalize_or(Vec2::Y))
        }
    }
}

/// The lowest dry ground in a polygon where a lake of radius `r` fits inside it (smoothed over
/// `r`), for a lake's centre; the middle if nothing fits.
fn lowest_spot(t: &Terrain, poly: &[Vec2], r: f32) -> Vec2 {
    let (lo, hi) = bounds(poly);
    let c = (lo + hi) * 0.5;
    let mut best = (f32::MAX, c);
    let ring: Vec<Vec2> = (0..12)
        .map(|k| Vec2::from_angle(k as f32 / 12.0 * std::f32::consts::TAU) * r * 1.35)
        .collect();
    for j in 0..17 {
        for i in 0..17 {
            let p = lo + (hi - lo) * Vec2::new(i as f32 / 16.0, j as f32 / 16.0);
            if !ring.iter().all(|o| {
                crate::biomes::point_in_polygon(p + *o, poly)
                    && t.height_at(p.x + o.x, p.y + o.y) > t.sea_level + 3.0
            }) {
                continue;
            }
            let h: f32 = ring
                .iter()
                .map(|o| t.height_at(p.x + o.x * 0.5, p.y + o.y * 0.5))
                .sum::<f32>()
                / ring.len() as f32
                + t.height_at(p.x, p.y)
                + p.distance(c) * 0.03;
            if h < best.0 {
                best = (h, p);
            }
        }
    }
    best.1
}

/// What landforms need to clear nature off their ground.
struct NatureEdit<'a> {
    field: &'a mut crate::nature::NatureField,
    rules: Option<&'a crate::nature::Rules>,
    masks: &'a crate::nature::Masks,
    zones: &'a crate::world::Zones,
    lib: Option<&'a crate::objects::Library>,
    amounts: [f32; 4],
    flora_amounts: [f32; 4],
    appearance: crate::nature::NatureAppearance,
    seed: u32,
}

/// Clears trees, plants, rocks and debris where `inside` holds (within `reach` of `c`): the
/// procedural nature is erased from the density map, placed (pinned) nature objects removed.
/// Returns how many went.
#[allow(clippy::too_many_arguments)]
fn clear_nature(
    n: &mut NatureEdit,
    store: &mut crate::objects::ObjectStore,
    history: &mut crate::history::History,
    t: &Terrain,
    water: &crate::water::WaterBodies,
    c: Vec2,
    reach: f32,
    inside: impl Fn(Vec2) -> bool + Sync,
) -> usize {
    let Some(lib) = n.lib else { return 0 };
    let mut count = 0;
    for id in store.query(c, reach) {
        let Some(o) = store.items.get(&id) else {
            continue;
        };
        if lib
            .get(&o.asset)
            .is_some_and(|e| Category::NATURE.contains(&e.category))
            && inside(Vec2::new(o.pos[0], o.pos[2]))
        {
            history.touch_object(store, id);
            store.remove(id);
            count += 1;
        }
    }
    let (lo, hi) = (c - reach, c + reach);
    if let Some(rules) = n.rules {
        let ctx = crate::nature::Ctx {
            terrain: t,
            water,
            zones: n.zones,
            lib,
            field: n.field,
            rules,
            masks: n.masks,
            amounts: n.amounts,
            flora_amounts: n.flora_amounts,
            appearance: n.appearance,
            seed: n.seed,
        };
        count += crate::nature::place::count_area(&ctx, lo, hi, &inside)
            .iter()
            .sum::<usize>();
    }
    crate::nature::paint_region(
        n.field,
        history,
        "Clear nature",
        lo,
        hi,
        inside,
        &Target::All,
        AreaOp::Keep(0.0),
    );
    count
}

fn new_lake(
    water: &mut crate::water::WaterBodies,
    name: &str,
    seed: Vec2,
    level: f32,
    style: crate::water::WaterStyle,
) -> u32 {
    water.next_id += 1;
    let id = water.next_id;
    water.lakes.push(crate::water::Lake {
        id,
        name: name.to_string(),
        seed: seed.to_array(),
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
    water.dirty = true;
    id
}

#[allow(clippy::too_many_lines)]
fn terrain_feature(In(args): In<Value>, mut d: WorldData, mut sel: ResMut<Selection>) -> R {
    let feature = string(&args, "feature")?
        .ok_or("missing `feature`")?
        .to_string();
    if !FEATURES.contains(&feature.as_str()) {
        return Err(format!(
            "unknown feature `{feature}` (one of {})",
            FEATURES.join(", ")
        ));
    }
    let seed = num(&args, "seed")?.map(|s| s as u32).unwrap_or_else(|| {
        d.meta
            .seed
            .wrapping_mul(31)
            .wrapping_add(fastrand::u32(..10_000))
    });
    let paint = boolean(&args, "paint")?.unwrap_or(true);
    let sel_poly = sel.polygon().map(|p| p.to_vec());
    let (slo, shi) = sel_poly
        .as_deref()
        .map(bounds)
        .unwrap_or((Vec2::splat(-200.0), Vec2::splat(200.0)));
    let size = (shi - slo).min_element().max(50.0);
    let centre = match opt_point(&args, "center")? {
        Some(c) => c,
        None => sel_poly
            .as_ref()
            .map(|_| (slo + shi) * 0.5)
            .ok_or("give a `center` (or select an area first)")?,
    };
    let path = || -> Result<Vec<Vec2>, String> {
        match opt_points(&args, "path")? {
            Some(p) if p.len() >= 2 => Ok(p),
            Some(_) => Err("`path` needs at least 2 points".into()),
            None => sel_poly
                .as_deref()
                .map(long_axis)
                .ok_or_else(|| "give a `path` (or select an area first)".to_string()),
        }
    };
    // the selection's own mask (for features whose edges must line up with it)
    let sel_field = sel.field(&d.terrain).cloned();
    let sel = &*sel;
    let area_field = |t: &Terrain, feather_k: f32| -> Result<MaskField, String> {
        let poly = area_poly(args.get("area"), sel)?;
        let f = (auto_feather(&poly) * feather_k).max(8.0);
        MaskField::build(t, &poly, f, f * 0.6, false, seed)
            .ok_or_else(|| "the area lies outside the world".to_string())
    };
    let WorldData {
        terrain,
        history,
        store,
        water,
        lib,
        meta,
        zones,
        nature,
        nature_rules,
        nature_masks,
        amounts,
        appearance,
        ..
    } = &mut d;
    let t = &mut **terrain;
    let lib = lib.as_deref();
    let mut nat = NatureEdit {
        field: nature,
        rules: nature_rules.as_deref(),
        masks: nature_masks,
        zones,
        lib,
        amounts: crate::nature::amounts(amounts),
        flora_amounts: amounts.flora_amounts(),
        appearance: **appearance,
        seed: meta.seed,
    };
    history.begin(format!("Feature: {}", feature.replace('_', " ")));
    let res: Result<String, String> = (|| match feature.as_str() {
        "volcano" => {
            let radius = num(&args, "radius")?.unwrap_or((size * 0.42).clamp(120.0, 1200.0));
            let height = num(&args, "height")?.unwrap_or((radius * 0.5).clamp(70.0, 480.0));
            let v = Volcano {
                center: centre,
                radius,
                height,
                crater_radius: num(&args, "crater_radius")?.unwrap_or(radius * 0.16),
                crater_depth: num(&args, "crater_depth")?.unwrap_or(height * 0.3),
                seed,
            };
            let (base, floor) =
                features::volcano(t, history, &v).ok_or("the volcano lies outside the world")?;
            let cleared = clear_nature(&mut nat, store, history, t, water, centre, radius, |p| {
                p.distance(centre) < radius * (0.72 + 0.2 * value2(p.x / 30.0, p.y / 30.0, seed))
            });
            let mut notes = vec![];
            if paint {
                features::paint_volcano(t, history, &v);
                // the volcanic biome on the cone (its sparse plants regrow on the lower flanks)
                if let Some(rect) = t.rect_for_circle(centre, radius * 1.1) {
                    history.touch_terrain(t, rect);
                    let id = BiomePreset::Volcanic.id();
                    for j in rect.z0..=rect.z1 {
                        for i in rect.x0..=rect.x1 {
                            let p = t.grid_to_world(i, j);
                            if p.distance(centre)
                                < radius
                                    * (1.0 + 0.1 * fbm(p.x / 80.0, p.y / 80.0, 2, 2.0, 0.5, seed))
                            {
                                let k = t.idx(i, j);
                                t.biomes[k] = id;
                            }
                        }
                    }
                    t.mark_biome_dirty(rect);
                }
            }
            let lava = string(&args, "lava")?.unwrap_or("lake");
            let lava_style = crate::water::preset_style("lava");
            if matches!(lava, "lake" | "both") {
                let level = floor + (v.crater_depth * 0.28).min(25.0);
                match crate::water::flood(t, centre, level) {
                    Ok(cells) if cells.len() > 8 => {
                        history.touch_water(water);
                        let id = new_lake(water, "Lava lake", centre, level, lava_style.clone());
                        notes.push(format!(
                            "a lava lake (water body {id}) fills the crater at {level:.0} m"
                        ));
                    }
                    _ => notes.push("the crater could not hold a lava lake".into()),
                }
            }
            if matches!(lava, "channels" | "both") {
                let mut rng = Rng::new(seed as u64 + 3);
                let n = 2;
                let mut made = 0;
                for k in 0..n {
                    let a = rng.range(0.0, std::f32::consts::TAU) + k as f32 * 2.4;
                    let mut p = centre + Vec2::from_angle(a) * v.crater_radius * 1.25;
                    let mut pts = vec![Vec3::new(p.x, t.height_at(p.x, p.y), p.y)];
                    let mut dir = Vec2::from_angle(a);
                    for _ in 0..400 {
                        let nrm = t.normal_at(p.x, p.y);
                        let down = Vec2::new(nrm.x, nrm.z).normalize_or(dir);
                        dir = (dir * 0.6 + down * 0.4).normalize_or(dir);
                        p += dir * 6.0;
                        pts.push(Vec3::new(p.x, t.height_at(p.x, p.y), p.y));
                        if p.distance(centre) > radius * 0.82 || !t.in_bounds(p.x, p.y) {
                            break;
                        }
                    }
                    history.touch_water(water);
                    water.next_id += 1;
                    let id = water.next_id;
                    if let Some(mut r) = crate::water::make_river(t, history, &pts, 9.0, 1.4, id) {
                        r.style = lava_style.clone();
                        r.style.flow = 0.3;
                        r.name = format!("Lava flow {id}");
                        water.rivers.push(r);
                        water.dirty = true;
                        made += 1;
                    }
                }
                if made > 0 {
                    notes.push(format!("{made} lava channels run down the flanks"));
                }
            }
            Ok(format!(
                "Volcano at [{:.0}, {:.0}]: foot {:.0} m across at {base:.0} m, summit rim {:.0} m, crater {:.0} m wide and {:.0} m deep; {cleared} trees/plants cleared from the cone{}.",
                centre.x,
                centre.y,
                radius * 2.0,
                base + height * 1.03,
                v.crater_radius * 2.0,
                v.crater_depth,
                if notes.is_empty() {
                    String::new()
                } else {
                    format!("; {}", notes.join("; "))
                }
            ))
        }
        "mountain_range" | "ridge" => {
            let ridge = feature == "ridge";
            let p = path()?;
            let minor = size;
            let width = num(&args, "width")?.unwrap_or(if ridge {
                (minor * 0.25).clamp(80.0, 400.0)
            } else {
                (minor * 0.45).clamp(150.0, 900.0)
            });
            let height = num(&args, "height")?.unwrap_or(if ridge { 140.0 } else { 260.0 });
            let erode = boolean(&args, "erode")?.unwrap_or(true);
            let top = features::mountain_range(t, history, &p, width, height, ridge, seed, erode)
                .ok_or("the path lies outside the world")?;
            Ok(format!(
                "{} along {} points ({:.0} m long, {:.0} m wide): crest up to {:.0} m added, highest peak {:.0} m ({:.0} m above sea; snow starts ~{:.0} m above sea){}.",
                if ridge { "Ridge" } else { "Mountain range" },
                p.len(),
                features::path_length(&p),
                width,
                height,
                top,
                top - t.sea_level,
                t.style.snow_line,
                if erode { ", eroded" } else { "" }
            ))
        }
        "crater" => {
            let r = num(&args, "radius")?.unwrap_or((size * 0.3).clamp(30.0, 900.0));
            let depth = num(&args, "depth")?.unwrap_or(r * 0.25);
            features::crater(t, history, centre, r, depth, seed)
                .ok_or("the crater lies outside the world")?;
            if paint && let Some(rect) = t.rect_for_circle(centre, r * 1.6) {
                features::paint_where(t, history, rect, Layer::Gravel, |p, _, _| {
                    (1.0 - smoothstep(1.1, 1.6, p.distance(centre) / r)) * 0.8
                });
            }
            clear_nature(&mut nat, store, history, t, water, centre, r * 1.2, |p| {
                p.distance(centre) < r * 1.1
            });
            Ok(format!(
                "Crater at [{:.0}, {:.0}], {:.0} m across and {depth:.0} m deep with a raised rim.",
                centre.x,
                centre.y,
                r * 2.0
            ))
        }
        "mesas" | "buttes" => {
            let buttes = feature == "buttes";
            let field = area_field(t, 0.5)?;
            let a = selection::area(&field.poly);
            let count = num(&args, "count")?
                .map(|c| c as usize)
                .unwrap_or(((a / if buttes { 30_000.0 } else { 90_000.0 }) as usize).clamp(3, 16));
            let height = num(&args, "height")?.unwrap_or(if buttes { 55.0 } else { 70.0 });
            let n = features::mesas(t, history, &field, count, height, buttes, seed);
            if paint {
                // banded cliffs, scree aprons, dry grass on the tops
                features::paint_where(t, history, field.rect, Layer::Gravel, |p, _, slope| {
                    field.at(p)
                        * smoothstep(14.0, 24.0, slope)
                        * (1.0 - smoothstep(30.0, 40.0, slope))
                        * 0.6
                });
                features::paint_where(t, history, field.rect, Layer::Dirt, |p, h, slope| {
                    let k = (h / 11.0 + fbm(p.x / 50.0, p.y / 50.0, 2, 2.0, 0.5, seed) * 0.5)
                        .floor() as i32;
                    field.at(p)
                        * smoothstep(30.0, 42.0, slope)
                        * if k.rem_euclid(2) == 0 { 0.85 } else { 0.25 }
                });
            }
            Ok(format!(
                "{n} {} up to ~{height:.0} m tall with sheer banded cliffs and talus aprons.",
                if buttes { "buttes" } else { "mesas" }
            ))
        }
        "canyon" => {
            let p = path()?;
            let width = num(&args, "width")?.unwrap_or((size * 0.25).clamp(80.0, 400.0));
            let depth = num(&args, "depth")?.unwrap_or(70.0);
            let floor_w = num(&args, "floor_width")?.unwrap_or(width * 0.25);
            let centre_line = features::canyon(t, history, &p, depth, width, floor_w, seed)
                .ok_or("the path lies outside the world")?;
            let near = |q: Vec2| {
                features::polyline_dist(q, &centre_line.iter().map(|v| v.xz()).collect::<Vec<_>>())
                    .0
            };
            let (lo, hi) = bounds(&p);
            clear_nature(
                &mut nat,
                store,
                history,
                t,
                water,
                (lo + hi) * 0.5,
                (hi - lo).length() * 0.5 + width,
                |q| near(q) < floor_w * 0.8,
            );
            if paint
                && let Some(rect) =
                    t.rect_for_circle((lo + hi) * 0.5, (hi - lo).length() * 0.5 + width)
            {
                let line: Vec<Vec2> = centre_line.iter().map(|v| v.xz()).collect();
                features::paint_where(t, history, rect, Layer::Sand, |q, _, slope| {
                    (1.0 - smoothstep(
                        floor_w * 0.4,
                        floor_w * 0.7,
                        features::polyline_dist(q, &line).0,
                    )) * (1.0 - smoothstep(14.0, 24.0, slope))
                });
                features::paint_where(t, history, rect, Layer::Gravel, |q, _, slope| {
                    let d = features::polyline_dist(q, &line).0;
                    smoothstep(floor_w * 0.35, floor_w * 0.6, d)
                        * (1.0 - smoothstep(width * 0.4, width * 0.5, d))
                        * (1.0 - smoothstep(20.0, 30.0, slope))
                        * 0.7
                });
            }
            let mut msg = format!(
                "Canyon {:.0} m long, {width:.0} m wide, {depth:.0} m deep with a {floor_w:.0} m floor that descends all the way",
                features::path_length(&p)
            );
            if boolean(&args, "river")?.unwrap_or(true) && centre_line.len() >= 4 {
                history.touch_water(water);
                water.next_id += 1;
                let id = water.next_id;
                let rw = (floor_w * 0.45).clamp(6.0, 40.0);
                if let Some(mut r) = crate::water::make_river(t, history, &centre_line, rw, 2.0, id)
                {
                    let preset = string(&args, "river_preset")?.unwrap_or("mountain_stream");
                    r.style = crate::water::preset_style(preset);
                    r.name = format!("Canyon river {id}");
                    water.rivers.push(r);
                    water.dirty = true;
                    msg += &format!("; a {rw:.0} m river (water body {id}) runs along the floor");
                }
            }
            Ok(msg + ".")
        }
        "valley" => {
            let p = path()?;
            let width = num(&args, "width")?.unwrap_or((size * 0.5).clamp(120.0, 1500.0));
            let depth = num(&args, "depth")?.unwrap_or(45.0);
            let u = string(&args, "shape")?.unwrap_or("u") != "v";
            features::valley(t, history, &p, depth, width, u, seed)
                .ok_or("the path lies outside the world")?;
            Ok(format!(
                "{}-shaped valley {:.0} m long, {width:.0} m wide, {depth:.0} m deep.",
                if u { "U" } else { "V" },
                features::path_length(&p)
            ))
        }
        "dunes" => {
            let field = area_field(t, 1.0)?;
            let height = num(&args, "height")?.unwrap_or(16.0);
            let wl = num(&args, "wavelength")?.unwrap_or((height * 8.5).clamp(40.0, 400.0));
            let wind = num(&args, "wind_deg")?.unwrap_or(150.0);
            features::dunes(t, history, &field, height, wl, wind, seed);
            let cleared = clear_nature(
                &mut nat,
                store,
                history,
                t,
                water,
                centre,
                (shi - slo).length(),
                |q| field.at(q) > 0.35,
            );
            if paint {
                features::paint_where(t, history, field.rect, Layer::DryGrass, |q, _, _| {
                    let w = field.at(q);
                    (smoothstep(0.0, 0.25, w) * (1.0 - smoothstep(0.35, 0.7, w))) * 0.7
                });
                features::paint_where(t, history, field.rect, Layer::Sand, |q, _, _| {
                    smoothstep(
                        0.2,
                        0.65,
                        field.at(q) + fbm(q.x / 35.0, q.y / 35.0, 3, 2.0, 0.5, seed + 3) * 0.3,
                    )
                });
            }
            Ok(format!(
                "Dune field: {height:.0} m dunes every ~{wl:.0} m, wind towards {wind:.0}°, sand painted; {cleared} plants and trees buried."
            ))
        }
        "hills" => {
            let field = area_field(t, 1.0)?;
            let height = num(&args, "height")?.unwrap_or(30.0);
            let wl = num(&args, "wavelength")?.unwrap_or(220.0);
            features::hills(t, history, &field, height, wl, seed);
            Ok(format!(
                "Rolling hills up to {height:.0} m, ~{wl:.0} m apart."
            ))
        }
        "plateau" => {
            let poly = area_poly(args.get("area"), sel)?;
            // a talus-and-cliff edge about a fifth of its size wide, with deep lobes
            let (lo, hi) = bounds(&poly);
            let ext = (hi - lo).max_element() * 0.5;
            let field = MaskField::build(
                t,
                &poly,
                (ext * 0.22).clamp(22.0, 120.0),
                (ext * 0.35).clamp(30.0, 200.0),
                false,
                seed,
            )
            .ok_or("the area lies outside the world")?;
            let height = num(&args, "height")?.unwrap_or(50.0);
            let top = features::plateau(t, history, &field, height, seed);
            if paint {
                features::paint_where(t, history, field.rect, Layer::Dirt, |p, h, slope| {
                    let k = (h / 9.0 + fbm(p.x / 50.0, p.y / 50.0, 2, 2.0, 0.5, seed) * 0.5).floor()
                        as i32;
                    field.at(p)
                        * smoothstep(30.0, 42.0, slope)
                        * if k.rem_euclid(2) == 0 { 0.8 } else { 0.3 }
                });
                features::paint_where(t, history, field.rect, Layer::Gravel, |p, _, slope| {
                    field.at(p)
                        * smoothstep(16.0, 24.0, slope)
                        * (1.0 - smoothstep(30.0, 38.0, slope))
                        * 0.6
                });
            }
            Ok(format!(
                "Plateau with a banded escarpment, top at ~{top:.0} m ({height:.0} m above the area's median ground)."
            ))
        }
        "sea_cliffs" => {
            let p = match opt_points(&args, "path")? {
                Some(p) if p.len() >= 2 => p,
                _ => sel_poly
                    .as_deref()
                    .and_then(|poly| coast_in(t, poly))
                    .ok_or("give a coast `path` (the selection has no coastline to follow)")?,
            };
            let height = num(&args, "height")?.unwrap_or(35.0);
            let width = num(&args, "width")?.unwrap_or(140.0);
            features::sea_cliffs(t, history, &p, height, width, seed)
                .ok_or("the path lies outside the world")?;
            if paint {
                let (lo, hi) = bounds(&p);
                if let Some(rect) =
                    t.rect_for_circle((lo + hi) * 0.5, (hi - lo).length() * 0.5 + width)
                {
                    let sea = t.sea_level;
                    features::paint_where(t, history, rect, Layer::Gravel, |q, h, _| {
                        (1.0 - smoothstep(30.0, 60.0, features::polyline_dist(q, &p).0))
                            * (1.0 - smoothstep(sea + 1.0, sea + 4.0, h))
                    });
                }
            }
            Ok(format!(
                "Sea cliffs {:.0} m high along {:.0} m of coast.",
                height,
                features::path_length(&p)
            ))
        }
        "lagoon" => {
            let mut r = num(&args, "radius")?.unwrap_or((size * 0.3).clamp(60.0, 900.0));
            let depth = num(&args, "depth")?.unwrap_or(2.5);
            let inlets = num(&args, "inlets")?.map(|v| v as u32).unwrap_or(1);
            // straddle the nearest coast: the basin on the land side, the barrier along the shore
            let (coast, seaward) = nearest_sea(t, centre, r * 3.0);
            let mut centre = match (coast, opt_point(&args, "center")?) {
                (Some(cp), None) => cp - seaward * r * 0.6,
                _ => centre,
            };
            // the barrier must lie where the selection's mask is whole (in its soft edge it would
            // sink back under the sea): shrink and pull inland until it does
            if let (Some(f), None) = (&sel_field, num(&args, "radius")?) {
                for _ in 0..10 {
                    let b = (r * 0.14).clamp(18.0, 80.0);
                    let along = seaward.perp();
                    let fits = (0..32).all(|k| {
                        let a = Vec2::from_angle(k as f32 / 32.0 * std::f32::consts::TAU);
                        let e = (a.x * along * 1.5 + a.y * seaward) * (r * 1.15 + b * 1.2);
                        f.at(centre + e) >= 0.97
                            || t.height_at(centre.x + e.x, centre.y + e.y) > t.sea_level + 3.0
                    });
                    if fits {
                        break;
                    }
                    r = (r * 0.88).max(50.0);
                    if let Some(cp) = coast {
                        centre = cp - seaward * r * 0.6;
                    }
                }
            }
            let barrier = (r * 0.14).clamp(18.0, 80.0);
            features::lagoon(t, history, centre, r, depth, barrier, inlets, seaward, seed)
                .ok_or("the lagoon lies outside the world")?;
            let sea = t.sea_level;
            clear_nature(&mut nat, store, history, t, water, centre, r * 2.0, |q| {
                q.distance(centre) < r * 1.6 && t.height_at(q.x, q.y) < sea + 1.0
            });
            if paint && let Some(rect) = t.rect_for_circle(centre, r * 1.6 + barrier * 3.0) {
                features::paint_where(t, history, rect, Layer::Sand, |q, h, _| {
                    let n = fbm(q.x / 30.0, q.y / 30.0, 3, 2.0, 0.5, seed + 9) * 1.2;
                    (1.0 - smoothstep(
                        r * 1.1 + barrier * 1.5,
                        r * 1.2 + barrier * 2.8,
                        q.distance(centre),
                    )) * (1.0 - smoothstep(sea + 1.8 + n, sea + 3.5 + n, h))
                });
            }
            Ok(format!(
                "Lagoon {:.0} m across, {depth:.1} m deep, behind a sand barrier with {inlets} inlet(s) to the sea.",
                r * 2.0
            ))
        }
        "swamp" => {
            // on the selection, its banks line up with the mask (so they hold the water)
            let field = match (&sel_field, args.get("area")) {
                (Some(f), None) => f.clone(),
                _ => area_field(t, 1.0)?,
            };
            let (level, seeds) = features::swamp(t, history, &field, num(&args, "height")?, seed);
            if paint {
                // wet meadow and forest floor on the hummocks (no beach sand), mud at the water
                features::paint_where(t, history, field.rect, Layer::Grass, |q, _, _| {
                    smoothstep(0.1, 0.5, field.at(q)) * 0.85
                });
                features::paint_where(t, history, field.rect, Layer::ForestFloor, |q, h, _| {
                    field.at(q)
                        * smoothstep(
                            -0.1,
                            0.35,
                            fbm(q.x / 28.0, q.y / 28.0, 3, 2.0, 0.5, seed + 4),
                        )
                        * smoothstep(level + 0.3, level + 0.6, h)
                        * 0.6
                });
                features::paint_where(t, history, field.rect, Layer::Mud, |q, h, _| {
                    field.at(q)
                        * (1.0 - smoothstep(level + 0.05, level + 0.25, h))
                        * smoothstep(level - 0.6, level - 0.1, h)
                        * 0.7
                });
            }
            let cleared = clear_nature(
                &mut nat,
                store,
                history,
                t,
                water,
                centre,
                (shi - slo).length(),
                |q| field.at(q) > 0.3 && t.height_at(q.x, q.y) < level + 0.1,
            );
            let style = crate::water::preset_style("murky_swamp");
            let mut covered = vec![false; t.n() * t.n()];
            let mut lakes = 0;
            for s in seeds {
                let g = t.world_to_grid(s.x, s.y);
                let k = t.idx(g.x.round() as usize, g.y.round() as usize);
                if covered[k] {
                    continue;
                }
                if let Ok(cells) = crate::water::flood(t, s, level)
                    && cells.len() > 12
                {
                    for c in &cells {
                        covered[*c] = true;
                    }
                    // water whose basin reaches out past the banks would flood the land around it
                    let n = t.n();
                    if cells.iter().any(|c| field.at_grid(c % n, c / n) < 0.999) {
                        continue;
                    }
                    if lakes == 0 {
                        history.touch_water(water);
                    }
                    new_lake(water, "Swamp water", s, level, style.clone());
                    lakes += 1;
                }
            }
            Ok(format!(
                "Swamp: ground settled to ~{level:.1} m with hummocks, channels and ponds{}; {cleared} dry-land plants removed from the wet ground.",
                if lakes > 0 {
                    format!(
                        ", already filled with murky swamp water ({lakes} bodies at {level:.1} m: restyle them rather than adding more)"
                    )
                } else {
                    " (channels open to lower ground stay dry: add water where you want it)".into()
                }
            ))
        }
        "lake" => {
            let r = num(&args, "radius")?.unwrap_or((size * 0.3).clamp(25.0, 800.0));
            let ty = string(&args, "lake_type")?.unwrap_or("lake");
            let shape = crate::water::lakes::SHAPES
                .iter()
                .find(|s| s.id == ty)
                .ok_or_else(|| format!("unknown lake type `{ty}`"))?;
            let centre = match (opt_point(&args, "center")?, sel_poly.as_deref()) {
                (None, Some(poly)) => lowest_spot(t, poly, r),
                _ => centre,
            };
            let stretch = 1.0 + 0.35 * (seed % 7) as f32 / 6.0;
            let axis = Vec2::from_angle((seed % 360) as f32 * 0.0174);
            let outline: Vec<Vec2> = (0..72)
                .map(|k| {
                    let a = k as f32 / 72.0 * std::f32::consts::TAU;
                    let dir = Vec2::from_angle(a);
                    let rr = r
                        * (1.0
                            + 0.6 * fbm(a.cos() * 1.2, a.sin() * 1.2, 4, 2.1, 0.6, seed)
                            + 0.16 * fbm(a.cos() * 4.5, a.sin() * 4.5, 2, 2.0, 0.5, seed + 1))
                        .max(0.35);
                    let v = dir * rr;
                    centre
                        + axis * v.dot(axis) * stretch
                        + axis.perp() * v.dot(axis.perp()) / stretch.sqrt()
                })
                .collect();
            let depth = num(&args, "depth")?
                .map(|d| (d / shape.depth).clamp(0.3, 2.5))
                .unwrap_or(1.0);
            let id = crate::water::create_drawn_lake(
                t, history, store, lib, water, &outline, shape, depth, 1.0,
            )?;
            let level = water
                .lakes
                .iter()
                .find(|l| l.id == id)
                .map(|l| l.level)
                .unwrap_or(t.sea_level);
            let mut msg = format!(
                "{} (water body {id}) {:.0} m across, water at {level:.1} m",
                shape.name,
                r * 2.0
            );
            if boolean(&args, "island")?.unwrap_or(false) {
                let ir = r * 0.3;
                let ic = centre
                    + Vec2::new(
                        fbm(0.3, 0.7, 2, 2.0, 0.5, seed),
                        fbm(0.9, 0.1, 2, 2.0, 0.5, seed),
                    ) * r
                        * 0.5;
                let ih = num(&args, "height")?.unwrap_or((ir * 0.12).clamp(3.0, 25.0));
                features::island(t, history, ic, ir, level, ih, seed)
                    .ok_or("the island lies outside the world")?;
                clear_nature(&mut nat, store, history, t, water, ic, ir * 1.4, |q| {
                    q.distance(ic) < ir * 1.35 && t.height_at(q.x, q.y) < level + 0.3
                });
                if paint && let Some(rect) = t.rect_for_circle(ic, ir * 1.4) {
                    features::paint_where(t, history, rect, Layer::Sand, |q, h, _| {
                        (1.0 - smoothstep(level + 0.8, level + 1.8, h))
                            * (1.0 - smoothstep(ir * 1.2, ir * 1.4, q.distance(ic)))
                    });
                    features::paint_where(t, history, rect, Layer::Grass, |q, h, _| {
                        smoothstep(level + 1.6, level + 2.6, h)
                            * (1.0 - smoothstep(ir * 0.9, ir * 1.1, q.distance(ic)))
                            * 0.8
                    });
                }
                msg += &format!(
                    "; an island {:.0} m across rises {ih:.0} m out of it at [{:.0}, {:.0}]",
                    ir * 2.0,
                    ic.x,
                    ic.y
                );
            }
            Ok(msg + ".")
        }
        _ => unreachable!(),
    })();
    history.commit(store);
    match res {
        Ok(msg) => {
            meta.dirty = true;
            text(format!("{msg} One undo step."))
        }
        Err(e) => Err(e),
    }
}
