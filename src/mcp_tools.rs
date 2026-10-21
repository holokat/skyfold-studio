//! Registry of the editor's MCP tools. Every feature registers the tools that drive it from its
//! own plugin (`app.mcp_tool(McpTool { .. })`); the MCP server (`crate::mcp`) lists and calls
//! them, so agents can build anything a person can build in the editor.
//!
//! A tool runs on the main thread with exclusive world access between frames. Use
//! `world.run_system_cached_with(..)` or resource access to reuse the same code paths as the UI
//! (and record undo history the same way the UI does).

use bevy::prelude::*;
use serde_json::Value;

/// What a tool call returns.
pub enum McpContent {
    Text(String),
    /// Serialised as JSON text (indented when small).
    Json(Value),
    /// PNG bytes (e.g. a screenshot); sent as image content.
    Png(Vec<u8>),
    /// The call finishes in a later frame (screenshots, work done by other systems): see
    /// [`McpContent::deferred`]. Content returned next to it is sent first.
    Deferred(McpDeferred),
}

/// A result that needs frames to pass. The server polls it once per frame (exclusive world
/// access, before `Update`) until it returns `Some`; the result may itself contain another
/// `Deferred`.
pub struct McpDeferred(pub Box<McpPoll>);

/// Polled once per frame; `Some` finishes the call.
pub type McpPoll = dyn FnMut(&mut World) -> Option<Result<Vec<McpContent>, String>> + Send + Sync;

impl McpContent {
    /// Finish the call later: `poll` runs once per frame until it returns `Some(result)`.
    pub fn deferred(
        poll: impl FnMut(&mut World) -> Option<Result<Vec<McpContent>, String>> + Send + Sync + 'static,
    ) -> Self {
        McpContent::Deferred(McpDeferred(Box::new(poll)))
    }
}

pub type McpRun = fn(&mut World, Value) -> Result<Vec<McpContent>, String>;

#[derive(Clone)]
pub struct McpTool {
    /// snake_case, unique, e.g. "roads_create".
    pub name: &'static str,
    /// What it does and when to use it (shown to the agent).
    pub description: &'static str,
    /// JSON Schema of the arguments object.
    pub input_schema: fn() -> Value,
    pub run: McpRun,
}

#[derive(Resource, Default)]
pub struct McpTools {
    pub tools: Vec<McpTool>,
}

pub trait McpAppExt {
    fn mcp_tool(&mut self, tool: McpTool) -> &mut Self;
}

impl McpAppExt for App {
    fn mcp_tool(&mut self, tool: McpTool) -> &mut Self {
        let mut reg = self
            .world_mut()
            .get_resource_or_insert_with(McpTools::default);
        debug_assert!(
            !reg.tools.iter().any(|t| t.name == tool.name),
            "duplicate MCP tool {}",
            tool.name
        );
        reg.tools.push(tool);
        self
    }
}

/// Reads a required argument.
pub fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    let v = args
        .get(key)
        .ok_or_else(|| format!("missing argument `{key}`"))?;
    serde_json::from_value(v.clone()).map_err(|e| format!("argument `{key}`: {e}"))
}

/// Reads an optional argument.
pub fn opt_arg<T: serde::de::DeserializeOwned>(
    args: &Value,
    key: &str,
) -> Result<Option<T>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("argument `{key}`: {e}")),
    }
}

// ------------------------------------------------------------------ shared conventions
//
// Every tool uses the same conventions (stated once in the server's instructions):
// - positions are `[x, z]` (or `[x, y, z]`) arrays in metres;
// - every yaw / heading / direction in degrees is a compass bearing: 0 = north (−z), 90 = east
//   (+x), 180 = south, 270 = west, clockwise seen from above. Cameras and explorers: where they
//   look; characters: where they face; objects: their turn from how they were modelled;
//   wind, water flow and the sunrise: the direction they go towards / come from as stated.
// The helpers below convert to the editor's internal angles.

/// Compass bearing in 0..360.
pub fn compass(deg: f32) -> f32 {
    let d = deg.rem_euclid(360.0);
    if (d * 10.0).round() >= 3600.0 {
        0.0
    } else {
        (d * 10.0).round() / 10.0
    }
}

/// Camera / explorer / camera-path yaw (radians, looking along (−sin, −cos)) from a bearing.
pub fn view_yaw(bearing_deg: f32) -> f32 {
    -bearing_deg.to_radians()
}

/// The bearing a camera / explorer yaw looks along.
pub fn view_bearing(yaw: f32) -> f32 {
    compass(-yaw.to_degrees())
}

/// Gameplay facing (radians, 0 = +x, counter-clockwise seen from above) from a bearing.
pub fn facing_yaw(bearing_deg: f32) -> f32 {
    (90.0 - bearing_deg).to_radians()
}

/// The bearing a gameplay facing points along.
pub fn facing_bearing(yaw: f32) -> f32 {
    compass(90.0 - yaw.to_degrees())
}

/// An object's turn about the vertical: positive bearings turn it clockwise seen from above.
pub fn turn_quat(bearing_deg: f32) -> Quat {
    Quat::from_rotation_y(-bearing_deg.to_radians())
}

/// A unit (x, z) direction from a bearing.
pub fn bearing_dir(bearing_deg: f32) -> Vec2 {
    let a = bearing_deg.to_radians();
    Vec2::new(a.sin(), -a.cos())
}

/// The bearing of an (x, z) direction.
pub fn dir_bearing(d: Vec2) -> f32 {
    compass(d.x.atan2(-d.y).to_degrees())
}

/// A position from `key` (`[x, z]` or `[x, y, z]`; y is `None` for `[x, z]`) or, as an old quiet
/// alias, separate `x` / `z` (and `y`) numbers.
pub fn position_arg(args: &Value, key: &str) -> Result<Option<(Vec2, Option<f32>)>, String> {
    let f = |v: &Value| v.as_f64().map(|f| f as f32).filter(|f| f.is_finite());
    match args.get(key) {
        Some(Value::Array(a)) => {
            let n: Vec<f32> = a.iter().filter_map(f).collect();
            match (n.len() == a.len(), &n[..]) {
                (true, [x, z]) => Ok(Some((Vec2::new(*x, *z), None))),
                (true, [x, y, z]) => Ok(Some((Vec2::new(*x, *z), Some(*y)))),
                _ => Err(format!(
                    "`{key}` must be [x, z] or [x, y, z] (numbers, metres)"
                )),
            }
        }
        Some(Value::Null) | None => match (args.get("x").and_then(f), args.get("z").and_then(f)) {
            (Some(x), Some(z)) => Ok(Some((Vec2::new(x, z), args.get("y").and_then(f)))),
            (None, None) => Ok(None),
            _ => Err(format!("give `{key}` [x, z]")),
        },
        Some(_) => Err(format!(
            "`{key}` must be [x, z] or [x, y, z] (numbers, metres)"
        )),
    }
}

/// Schema of a `[x, z]` / `[x, y, z]` position.
pub fn position_schema(desc: &str) -> Value {
    serde_json::json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": format!("{desc}: [x, z] in metres (height from the ground) or [x, y, z]") })
}

/// Schema of a compass bearing.
pub fn bearing_schema(desc: &str) -> Value {
    serde_json::json!({ "type": "number", "description": format!("{desc}: compass degrees (0 north, 90 east, 180 south, 270 west; clockwise seen from above)") })
}

/// The quiet aliases of a `position` argument: separate `x` / `z` (and `y`) numbers.
pub fn xz_aliases() -> Value {
    serde_json::json!({ "x": { "type": "number" }, "y": { "type": "number" }, "z": { "type": "number" } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearings_round_trip_every_convention() {
        for b in [0.0f32, 45.0, 90.0, 180.0, 270.0, 359.0] {
            assert!((view_bearing(view_yaw(b)) - b).abs() < 0.05, "view {b}");
            assert!(
                (facing_bearing(facing_yaw(b)) - b).abs() < 0.05,
                "facing {b}"
            );
            assert!((dir_bearing(bearing_dir(b)) - b).abs() < 0.05, "dir {b}");
        }
        // north is −z, east is +x
        assert!(bearing_dir(0.0).distance(Vec2::new(0.0, -1.0)) < 1e-5);
        assert!(bearing_dir(90.0).distance(Vec2::new(1.0, 0.0)) < 1e-5);
        // a camera at yaw(90°) looks east: its forward (−Z rotated by yaw about Y) is +x
        let fwd = Quat::from_rotation_y(view_yaw(90.0)) * Vec3::NEG_Z;
        assert!(fwd.distance(Vec3::X) < 1e-5, "{fwd}");
        // a gameplay facing of 90° points east (export: facing = [cos, 0, −sin])
        let y = facing_yaw(90.0);
        assert!(Vec2::new(y.cos(), -y.sin()).distance(Vec2::X) < 1e-5);
        // an object turned 90° clockwise: its +x (east) side now faces south (+z)
        assert!((turn_quat(90.0) * Vec3::X).distance(Vec3::Z) < 1e-5);
        assert_eq!(compass(-90.0), 270.0);
        assert_eq!(compass(359.99), 0.0);
    }

    #[test]
    fn positions_accept_arrays_and_old_xz() {
        let a = serde_json::json!({ "position": [10, -4] });
        assert_eq!(
            position_arg(&a, "position").unwrap(),
            Some((Vec2::new(10.0, -4.0), None))
        );
        let a = serde_json::json!({ "position": [10, 3, -4] });
        assert_eq!(
            position_arg(&a, "position").unwrap(),
            Some((Vec2::new(10.0, -4.0), Some(3.0)))
        );
        let a = serde_json::json!({ "x": 1, "z": 2 });
        assert_eq!(
            position_arg(&a, "position").unwrap(),
            Some((Vec2::new(1.0, 2.0), None))
        );
        assert!(position_arg(&serde_json::json!({ "x": 1 }), "position").is_err());
        assert!(position_arg(&serde_json::json!({ "position": [1] }), "position").is_err());
        assert_eq!(
            position_arg(&serde_json::json!({}), "position").unwrap(),
            None
        );
    }
}
