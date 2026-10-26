//! MCP tools for the sky bodies (Lighting › Sky bodies): the same edits as the panel. Settings,
//! not undo steps (like `lighting_set`); saved with the world.

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use super::{BodyKind, MAX_BODIES, SkyBody, SkyPreset};
use crate::camera::MainCamera;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, bearing_schema, dir_bearing};
use crate::sky::Lighting;

type R = Result<Vec<McpContent>, String>;

fn out(v: Value) -> R {
    Ok(vec![McpContent::Json(v)])
}

fn color_schema(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "number", "minimum": 0, "maximum": 1 }, "minItems": 3, "maxItems": 3, "description": format!("{desc}: linear [r, g, b] 0..1") })
}

/// The settable fields of a body (`sky_body_add` / `sky_body_set`).
fn body_props() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("kind".into(), json!({ "type": "string", "enum": BodyKind::ALL.map(|k| k.id()), "description": "moon (grey, cratered), terran (oceans, land, clouds), gas_giant (bands, storms), ice, lava (glowing cracks), desert (rust-red), star (a distant sun: a glowing disc; it does not light the world), black_hole (shadow, Doppler-bright accretion disc, bent starlight)" }));
    m.insert("name".into(), json!({ "type": "string" }));
    m.insert("enabled".into(), json!({ "type": "boolean" }));
    m.insert("bearing".into(), bearing_schema("where in the sky"));
    m.insert("elevation".into(), json!({ "type": "number", "minimum": -30, "maximum": 90, "description": "degrees above the horizon (negative: partly set behind it)" }));
    m.insert("size".into(), json!({ "type": "number", "minimum": 0.1, "maximum": 80, "description": "angular diameter in degrees (the world's moon ≈ 1.8; a looming giant 30–60)" }));
    m.insert("tilt".into(), json!({ "type": "number", "minimum": -90, "maximum": 90, "description": "lean of the spin axis (degrees): tilts bands, caps and the ring" }));
    m.insert("spin".into(), json!({ "type": "number", "minimum": 0, "maximum": 360, "description": "turns the surface (degrees): another face of the same world" }));
    m.insert("color".into(), color_schema("surface colour (a star's glow, a black hole's inner disc)"));
    m.insert("color2".into(), color_schema("second colour (bands, land, lava, a star's corona, the disc's outer glow)"));
    m.insert("seed".into(), json!({ "type": "integer", "minimum": 0, "description": "surface variation" }));
    m.insert("brightness".into(), json!({ "type": "number", "minimum": 0, "maximum": 10, "description": "albedo for lit bodies, emission for stars and black holes (1 as designed)" }));
    m.insert("atmosphere".into(), json!({ "type": "number", "minimum": 0, "maximum": 1, "description": "scattering rim round the limb" }));
    m.insert("atmosphere_color".into(), color_schema("rim colour"));
    m.insert("ring".into(), json!({ "type": "number", "minimum": 0, "maximum": 1, "description": "ring opacity (0 none)" }));
    m.insert("ring_inner".into(), json!({ "type": "number", "minimum": 1.05, "maximum": 4, "description": "ring inner radius in body radii" }));
    m.insert("ring_outer".into(), json!({ "type": "number", "minimum": 1.1, "maximum": 5, "description": "ring outer radius in body radii (a black hole's disc size)" }));
    m.insert("ring_open".into(), json!({ "type": "number", "minimum": 0, "maximum": 90, "description": "how open the ring (or a black hole's disc) is seen: 0 edge-on, 90 face-on" }));
    m.insert("ring_color".into(), color_schema("ring colour"));
    m.insert("follow_sky".into(), json!({ "type": "boolean", "description": "circle the sky once a day instead of staying put" }));
    m
}

/// The body as JSON (with its place in the list).
fn body_json(i: usize, b: &SkyBody) -> Value {
    let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
    v["index"] = json!(i);
    v
}

/// The camera's compass bearing (new bodies go where the user is looking).
fn view_bearing(w: &mut World) -> f32 {
    let mut q = w.query_filtered::<&GlobalTransform, With<MainCamera>>();
    q.iter(w)
        .next()
        .map(|t| {
            let f = t.forward();
            dir_bearing(Vec2::new(f.x, f.z))
        })
        .unwrap_or(140.0)
}

/// `body` with the call's fields written over it.
fn merged(body: &SkyBody, a: &Value) -> Result<SkyBody, String> {
    let props = body_props();
    let mut v = serde_json::to_value(body).map_err(|e| e.to_string())?;
    if let Some(o) = a.as_object() {
        for (k, val) in o {
            if props.contains_key(k) {
                v[k] = val.clone();
            }
        }
    }
    let mut b: SkyBody =
        serde_json::from_value(v).map_err(|e| format!("bad sky body field: {e}"))?;
    b.clamp();
    Ok(b)
}

fn index(a: &Value, len: usize) -> Result<usize, String> {
    let i = a
        .get("index")
        .and_then(|v| v.as_u64())
        .ok_or("`index` (from sky_bodies_list) is required")? as usize;
    if i >= len {
        return Err(format!("no sky body {i} (there are {len})"));
    }
    Ok(i)
}

fn dirty(w: &mut World) {
    w.resource_mut::<crate::world::WorldMeta>().dirty = true;
}

fn list(w: &mut World) -> Value {
    let s = &w.resource::<Lighting>().celestial;
    json!({
        "bodies": s.bodies.iter().enumerate().map(|(i, b)| body_json(i, b)).collect::<Vec<_>>(),
        "hide_moon": s.hide_moon,
        "max": MAX_BODIES,
    })
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "sky_bodies_list",
        description: "The sky bodies (Lighting › Sky bodies): planets, moons, distant stars and black holes hung in the sky, each with its index, kind, bearing, elevation, angular size, colours, ring and atmosphere; and whether the world's own moon is hidden.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, _| out(list(w)),
    });
    app.mcp_tool(McpTool {
        name: "sky_body_add",
        description: "Add a planet, moon, distant star or black hole to the sky (Lighting › Sky bodies; up to 8). It starts as its `kind` was designed — placed where the camera looks unless `bearing` is given — and any other field overrides it. \
Bodies are lit by the world's sun (a planet near the sun is a crescent), seen through the air (pale by day, bright at night, covered by clouds). A setting saved with the world, not an undo step.",
        input_schema: || {
            let mut p = body_props();
            p.remove("enabled");
            json!({ "type": "object", "properties": p, "required": ["kind"] })
        },
        run: |w, a| {
            let kind = a
                .get("kind")
                .and_then(|v| v.as_str())
                .and_then(BodyKind::from_id)
                .ok_or("`kind` must be one of moon, terran, gas_giant, ice, lava, desert, star, black_hole")?;
            if w.resource::<Lighting>().celestial.bodies.len() >= MAX_BODIES {
                return Err(format!("the sky holds {MAX_BODIES} bodies: remove one first"));
            }
            let mut b = SkyBody::preset(kind);
            b.bearing = view_bearing(w);
            let b = merged(&b, &a)?;
            let mut l = w.resource_mut::<Lighting>();
            l.celestial.bodies.push(b.clone());
            let i = l.celestial.bodies.len() - 1;
            dirty(w);
            out(body_json(i, &b))
        },
    });
    app.mcp_tool(McpTool {
        name: "sky_body_set",
        description: "Change a sky body (by `index` from sky_bodies_list): any of its fields — move it (bearing, elevation), resize it, recolour it, give it a ring or an atmosphere, turn it off (`enabled` false) or change its kind (keeps the other fields).",
        input_schema: || {
            let mut p = body_props();
            p.insert("index".into(), json!({ "type": "integer", "minimum": 0 }));
            json!({ "type": "object", "properties": p, "required": ["index"] })
        },
        run: |w, a| {
            let bodies = &w.resource::<Lighting>().celestial.bodies;
            let i = index(&a, bodies.len())?;
            let b = merged(&bodies[i], &a)?;
            w.resource_mut::<Lighting>().celestial.bodies[i] = b.clone();
            dirty(w);
            out(body_json(i, &b))
        },
    });
    app.mcp_tool(McpTool {
        name: "sky_body_remove",
        description: "Remove a sky body (by `index` from sky_bodies_list), or every one with `all`.",
        input_schema: || json!({ "type": "object", "properties": {
            "index": { "type": "integer", "minimum": 0 },
            "all": { "type": "boolean" }
        } }),
        run: |w, a| {
            if a.get("all").and_then(|v| v.as_bool()).unwrap_or(false) {
                w.resource_mut::<Lighting>().celestial.bodies.clear();
            } else {
                let i = index(&a, w.resource::<Lighting>().celestial.bodies.len())?;
                w.resource_mut::<Lighting>().celestial.bodies.remove(i);
            }
            dirty(w);
            out(list(w))
        },
    });
    app.mcp_tool(McpTool {
        name: "sky_bodies_preset",
        description: "A ready-made sky (Lighting › Sky bodies quick picks), placed round where the camera looks (or `bearing`): ringed_giant (a pale cratered giant with a thin ring looming over the horizon and a small moon), twin_moons, twin_suns (two distant suns), black_hole, crowded (a gas giant, an earth-like planet, a lava world and a moon), none (clear the sky). \
Replaces the sky's bodies unless `add` is true. `hide_moon` hides (or shows) the world's own moon.",
        input_schema: || json!({ "type": "object", "properties": {
            "preset": { "type": "string", "enum": SkyPreset::ALL.map(|p| p.id()) },
            "bearing": bearing_schema("centre of the group (default: where the camera looks)"),
            "add": { "type": "boolean", "default": false },
            "hide_moon": { "type": "boolean" }
        } }),
        run: |w, a| {
            if let Some(id) = a.get("preset").and_then(|v| v.as_str()) {
                let p = SkyPreset::from_id(id).ok_or_else(|| format!("unknown sky preset `{id}`"))?;
                let bearing = match a.get("bearing").and_then(|v| v.as_f64()) {
                    Some(b) => b as f32,
                    None => view_bearing(w),
                };
                let add = a.get("add").and_then(|v| v.as_bool()).unwrap_or(false);
                let mut l = w.resource_mut::<Lighting>();
                if !add {
                    l.celestial.bodies.clear();
                }
                for b in p.bodies(bearing) {
                    if l.celestial.bodies.len() < MAX_BODIES {
                        l.celestial.bodies.push(b);
                    }
                }
            }
            if let Some(h) = a.get("hide_moon").and_then(|v| v.as_bool()) {
                w.resource_mut::<Lighting>().celestial.hide_moon = h;
            }
            dirty(w);
            out(list(w))
        },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_overrides_only_known_fields_and_clamps() {
        let b = SkyBody::preset(BodyKind::GasGiant);
        let m = merged(&b, &json!({ "size": 500, "ring": 0.2, "index": 3, "nonsense": 1 })).unwrap();
        assert_eq!(m.size, 80.0);
        assert_eq!(m.ring, 0.2);
        assert_eq!(m.kind, BodyKind::GasGiant);
        assert!(merged(&b, &json!({ "kind": "teapot" })).is_err());
    }
}
