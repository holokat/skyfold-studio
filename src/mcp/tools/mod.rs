//! MCP tools for the editor's own features, grouped like the tool rail. Each tool runs through
//! the same functions as the UI (brushes, water, scatter, history…) so agents' edits behave,
//! save and undo exactly like a person's. Shared argument helpers live here.

mod materials;
mod nature;
mod objects;
mod scene;
mod terrain;
mod water;
mod world;
mod zones;

use std::fmt::Debug;

use bevy::prelude::*;
use serde_json::{Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool};

pub(super) type R = Result<Vec<McpContent>, String>;

pub(super) fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "batch",
        description: "Run several tool calls in order as one undo step: `calls` [{tool, args}, …] (each checked before anything runs). With `atomic` true the batch stops at the first error and undoes everything it did; otherwise it keeps going. \
Returns each call's result compactly. undo, redo, world_new, world_load and batch itself can't be inside a batch; screenshots and whole-world actions (world_generate, terrain_global, nature_auto) finish after it and aren't rolled back.",
        input_schema: super::calls::batch_schema,
        run: super::calls::batch,
    });
    world::register(app);
    terrain::register(app);
    materials::register(app);
    water::register(app);
    nature::register(app);
    objects::register(app);
    zones::register(app);
    scene::register(app);
}

/// Tools that only read (MCP `readOnlyHint`).
pub(super) const READ_ONLY: &[&str] = &[
    "world_info",
    "world_list",
    "terrain_sample",
    "materials_list",
    "water_list",
    "water_presets",
    "objects_library",
    "objects_check_model",
    "objects_preview",
    "objects_query",
    "look_style_list",
    "market_packs",
    "market_listings",
    "scenes_list",
    "fields_list",
    "zones_list",
    "get_log",
    "screenshot",
    "selection_get",
    "selection_info",
    "selection_screenshot",
    "walk_state",
];

/// The `tools/list` result for the registry.
pub(super) fn list_json(tools: &[McpTool]) -> Value {
    let list: Vec<Value> = tools
        .iter()
        .map(|t| {
            let mut schema = super::validate::public_schema(super::calls::schema_of(t));
            if schema.get("type").is_none() {
                schema["type"] = json!("object");
            }
            let mut v =
                json!({ "name": t.name, "description": t.description, "inputSchema": schema });
            if READ_ONLY.contains(&t.name) {
                v["annotations"] = json!({ "readOnlyHint": true });
            }
            v
        })
        .collect();
    json!({ "tools": list })
}

/// Runs a system that takes the call's arguments with exclusive world access (system params
/// like `WorldData` give tools the same view of the world the UI systems have).
pub(super) fn run<M, S>(world: &mut World, sys: S, args: Value) -> R
where
    S: IntoSystem<In<Value>, R, M> + 'static,
{
    world
        .run_system_cached_with(sys, args)
        .map_err(|e| format!("{e}"))?
}

pub(super) fn text(s: impl Into<String>) -> R {
    Ok(vec![McpContent::Text(s.into())])
}

pub(super) fn json_out(v: Value) -> R {
    Ok(vec![McpContent::Json(v)])
}

// ------------------------------------------------------------------ arguments

pub(super) fn num(args: &Value, key: &str, default: f32) -> Result<f32, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_f64()
            .map(|f| f as f32)
            .filter(|f| f.is_finite())
            .ok_or_else(|| format!("`{key}` must be a number")),
    }
}

pub(super) fn opt_num(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => num(args, key, 0.0).map(Some),
    }
}

/// A number clamped to the range the UI slider allows.
pub(super) fn num_in(
    args: &Value,
    key: &str,
    default: f32,
    lo: f32,
    hi: f32,
) -> Result<f32, String> {
    Ok(num(args, key, default)?.clamp(lo, hi))
}

pub(super) fn opt_bool(args: &Value, key: &str) -> Result<Option<bool>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` must be true or false")),
    }
}

pub(super) fn opt_str<'a>(args: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

/// `[x, z]` (also accepts `{"x":…,"z":…}`).
pub(super) fn xz(v: &Value) -> Result<Vec2, String> {
    let f = |v: &Value| v.as_f64().map(|f| f as f32).filter(|f| f.is_finite());
    match v {
        Value::Array(a) if a.len() >= 2 => match (f(&a[0]), f(&a[a.len() - 1])) {
            // [x, z] or [x, y, z]
            (Some(x), Some(z)) => Ok(Vec2::new(x, z)),
            _ => Err("a point must be [x, z] numbers".into()),
        },
        Value::Object(o) => match (o.get("x").and_then(f), o.get("z").and_then(f)) {
            (Some(x), Some(z)) => Ok(Vec2::new(x, z)),
            _ => Err("a point must be [x, z]".into()),
        },
        _ => Err("a point must be [x, z]".into()),
    }
}

pub(super) fn opt_xz(args: &Value, key: &str) -> Result<Option<Vec2>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => xz(v).map(Some).map_err(|e| format!("`{key}`: {e}")),
    }
}

pub(super) fn opt_points(args: &Value, key: &str) -> Result<Option<Vec<Vec2>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => a
            .iter()
            .map(xz)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
            .map_err(|e| format!("`{key}`: {e}")),
        Some(_) => Err(format!("`{key}` must be a list of [x, z] points")),
    }
}

pub(super) fn points(args: &Value, key: &str, min: usize) -> Result<Vec<Vec2>, String> {
    let p = opt_points(args, key)?
        .ok_or_else(|| format!("missing `{key}` (a list of [x, z] points)"))?;
    if p.len() < min {
        return Err(format!("`{key}` needs at least {min} points"));
    }
    Ok(p)
}

/// An sRGB colour `[r, g, b]` in 0..1 (or a "#rrggbb" string).
pub(super) fn opt_color(args: &Value, key: &str) -> Result<Option<[f32; 3]>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let h = s.trim_start_matches('#');
            let p = |i: usize| {
                u8::from_str_radix(h.get(i..i + 2).unwrap_or("x"), 16).map(|v| v as f32 / 255.0)
            };
            match (h.len(), p(0), p(2), p(4)) {
                (6, Ok(r), Ok(g), Ok(b)) => Ok(Some([r, g, b])),
                _ => Err(format!(
                    "`{key}`: colour must be [r, g, b] in 0..1 or \"#rrggbb\""
                )),
            }
        }
        Some(Value::Array(a)) if a.len() == 3 => {
            let c: Vec<f32> = a
                .iter()
                .filter_map(|v| v.as_f64())
                .map(|v| v.clamp(0.0, 1.0) as f32)
                .collect();
            if c.len() == 3 {
                Ok(Some([c[0], c[1], c[2]]))
            } else {
                Err(format!("`{key}`: colour must be [r, g, b] in 0..1"))
            }
        }
        Some(_) => Err(format!(
            "`{key}`: colour must be [r, g, b] in 0..1 or \"#rrggbb\""
        )),
    }
}

// ------------------------------------------------------------------ enums

/// `TemperateForest` → `temperate_forest`.
pub(super) fn snake(name: &str) -> String {
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

/// snake_case ids of an enum's variants (for schemas).
pub(super) fn ids<T: Debug>(all: &[T]) -> Vec<String> {
    all.iter().map(|v| snake(&format!("{v:?}"))).collect()
}

/// Finds a variant by id, Debug name or display name (case, spaces and underscores ignored).
pub(super) fn pick<T: Debug + Copy>(
    all: &[T],
    s: &str,
    what: &str,
    name: impl Fn(T) -> &'static str,
) -> Result<T, String> {
    let n = norm(s);
    all.iter()
        .copied()
        .find(|v| norm(&format!("{v:?}")) == n || norm(name(*v)) == n)
        .ok_or_else(|| format!("unknown {what} `{s}`; one of: {}", ids(all).join(", ")))
}

pub(super) fn opt_pick<T: Debug + Copy>(
    args: &Value,
    key: &str,
    all: &[T],
    what: &str,
    name: impl Fn(T) -> &'static str,
) -> Result<Option<T>, String> {
    match opt_str(args, key)? {
        None => Ok(None),
        Some(s) => pick(all, s, what, name).map(Some),
    }
}

// ------------------------------------------------------------------ schemas

pub(super) fn xz_schema(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": desc })
}

pub(super) fn points_schema(desc: &str, min: usize) -> Value {
    json!({ "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "minItems": min, "description": desc })
}

pub(super) fn color_schema(desc: &str) -> Value {
    json!({ "type": ["array", "string"], "items": { "type": "number", "minimum": 0, "maximum": 1 }, "minItems": 3, "maxItems": 3, "description": format!("{desc}: sRGB [r, g, b] in 0..1 (a \"#rrggbb\" string works too)") })
}

pub(super) fn enum_schema(values: Vec<String>, desc: &str) -> Value {
    json!({ "type": "string", "enum": values, "description": desc })
}

pub(super) fn area_schema(desc: &str) -> Value {
    json!({
        "type": "object",
        "description": format!("{desc} One of: {{\"center\":[x,z],\"radius\":m}}, {{\"min\":[x,z],\"max\":[x,z]}}, {{\"polygon\":[[x,z],…]}} or {{\"path\":[[x,z],…],\"radius\":m}} (a band along a line)."),
        "properties": {
            "center": xz_schema("circle centre"),
            "radius": { "type": "number", "description": "circle radius, or half-width of a path band (m)" },
            "min": xz_schema("rectangle corner with the smallest x and z"),
            "max": xz_schema("rectangle corner with the largest x and z"),
            "polygon": points_schema("closed outline", 3),
            "path": points_schema("centre line of a band", 2)
        }
    })
}

// ------------------------------------------------------------------ areas

/// A region of the map given by the caller.
pub(super) enum Area {
    Circle(Vec2, f32),
    Rect(Vec2, Vec2),
    Polygon(Vec<Vec2>),
    Band(Vec<Vec2>, f32),
}

impl Area {
    pub fn parse(v: &Value) -> Result<Area, String> {
        if let Some(p) = opt_points(v, "polygon")? {
            if p.len() < 3 {
                return Err("`polygon` needs at least 3 points".into());
            }
            return Ok(Area::Polygon(p));
        }
        if let Some(p) = opt_points(v, "path")? {
            let r = num(v, "radius", 20.0)?.max(0.5);
            if p.is_empty() {
                return Err("`path` needs at least 1 point".into());
            }
            return Ok(Area::Band(p, r));
        }
        if let (Some(a), Some(b)) = (opt_xz(v, "min")?, opt_xz(v, "max")?) {
            return Ok(Area::Rect(a.min(b), a.max(b)));
        }
        if let Some(c) = opt_xz(v, "center")? {
            let r = opt_num(v, "radius")?.ok_or("a circle needs `radius`")?;
            return Ok(Area::Circle(c, r.max(0.5)));
        }
        Err("an area is {\"center\":[x,z],\"radius\":m}, {\"min\":[x,z],\"max\":[x,z]}, {\"polygon\":[[x,z],…]} or {\"path\":[[x,z],…],\"radius\":m}".into())
    }

    pub fn contains(&self, p: Vec2) -> bool {
        match self {
            Area::Circle(c, r) => p.distance_squared(*c) <= r * r,
            Area::Rect(a, b) => p.x >= a.x && p.y >= a.y && p.x <= b.x && p.y <= b.y,
            Area::Polygon(poly) => crate::biomes::point_in_polygon(p, poly),
            Area::Band(path, r) => dist_to_path(p, path) <= *r,
        }
    }

    /// (min, max) corners.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        match self {
            Area::Circle(c, r) => (*c - Vec2::splat(*r), *c + Vec2::splat(*r)),
            Area::Rect(a, b) => (*a, *b),
            Area::Polygon(p) => p
                .iter()
                .fold((Vec2::MAX, Vec2::MIN), |(a, b), q| (a.min(*q), b.max(*q))),
            Area::Band(p, r) => {
                let (a, b) = p
                    .iter()
                    .fold((Vec2::MAX, Vec2::MIN), |(a, b), q| (a.min(*q), b.max(*q)));
                (a - Vec2::splat(*r), b + Vec2::splat(*r))
            }
        }
    }

    /// (centre, radius) of a circle around the area (for spatial queries).
    pub fn circle(&self) -> (Vec2, f32) {
        let (a, b) = self.bounds();
        ((a + b) * 0.5, (b - a).length() * 0.5)
    }
}

pub(super) fn dist_to_path(p: Vec2, path: &[Vec2]) -> f32 {
    if path.len() == 1 {
        return p.distance(path[0]);
    }
    path.windows(2)
        .map(|w| {
            let ab = w[1] - w[0];
            let t = ((p - w[0]).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            p.distance(w[0] + ab * t)
        })
        .fold(f32::MAX, f32::min)
}

/// Points along a polyline every `spacing` metres (first and last included).
pub(super) fn along(path: &[Vec2], spacing: f32) -> Vec<Vec2> {
    let mut out = vec![path[0]];
    for w in path.windows(2) {
        let n = (w[0].distance(w[1]) / spacing.max(0.1)).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(w[0].lerp(w[1], k as f32 / n as f32));
        }
    }
    out
}

/// Closes and densifies a polygon (for code expecting a hand-drawn loop).
pub(super) fn densify_closed(poly: &[Vec2], spacing: f32) -> Vec<Vec2> {
    let mut closed = poly.to_vec();
    closed.push(poly[0]);
    let mut out = along(&closed, spacing);
    out.pop();
    out
}
