//! MCP tools for roads: list, create (from points), route (A* from A to B), update and remove.
//! They run the same edit code as the tools and the inspector, each one undoable.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use serde_json::{Value, json};

use super::build::{Edit, RoadWorld};
use super::{Road, RoadKind, RoadOptions};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "roads_list",
        description: "List the roads: id, name, kind, width (m), length (m), steepest and average grade (%), bridges, build options and control points [[x,z],…].",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: list,
    })
    .mcp_tool(McpTool {
        name: "roads_create",
        description: "Create a road through control points (world x,z; the spline passes through each). It follows the ground with a smoothed profile, carves a bed, clears vegetation, bridges water and (cobble / paved) gets lamp posts. Kinds: trail, footpath, dirt_road, gravel, cobblestone, paved (Paved Highway). Undoable.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "kind": {"type": "string", "description": "trail | footpath | dirt_road | gravel | cobblestone | paved"},
                "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 2, "description": "[[x,z],…] world metres"},
                "width": {"type": "number", "description": "metres (default: the kind's typical width)"},
                "name": {"type": "string"},
                "options": options_schema()
            }, "required": ["kind", "points"]})
        },
        run: create,
    })
    .mcp_tool(McpTool {
        name: "roads_route",
        description: "Auto-route a road from A to B (A* over the terrain: keeps to the kind's design grade with switchbacks on steep ground, gentle bends, crosses rivers where narrow with a bridge). Returns the new road. Undoable.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "from": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "[x,z]"},
                "to": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "[x,z]"},
                "kind": {"type": "string"},
                "width": {"type": "number"},
                "name": {"type": "string"},
                "options": options_schema()
            }, "required": ["from", "to", "kind"]})
        },
        run: route,
    })
    .mcp_tool(McpTool {
        name: "roads_update",
        description: "Change a road: kind, width, name, control points and/or options. Ordinary roads use [[x,z],…]. Authored bridge roads also accept [x,y,z]; two-number points preserve the existing deck profile by interpolation. Set rebuild=true alone to re-carve with the current settings. Undoable.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "id": {"type": "integer"},
                "kind": {"type": "string"},
                "width": {"type": "number"},
                "name": {"type": "string"},
                "points": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}},
                "options": options_schema(),
                "rebuild": {"type": "boolean"}
            }, "required": ["id"]})
        },
        run: update,
    })
    .mcp_tool(McpTool {
        name: "roads_remove",
        description: "Remove a road; the ground, paint and vegetation under it come back. Undoable.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "integer"}}, "required": ["id"]}),
        run: remove,
    });
}

fn options_schema() -> Value {
    json!({"type": "object", "description": "build options (all default true)", "properties": {
        "bed": {"type": "boolean", "description": "carve / fill a smooth terrain bed"},
        "clear": {"type": "boolean", "description": "remove trees, plants and rocks in the way"},
        "bridges": {"type": "boolean", "description": "bridge rivers, lakes and straits"},
        "props": {"type": "boolean", "description": "lamp posts (cobblestone and paved roads)"}
    }})
}

fn kind_arg(args: &Value) -> Result<Option<RoadKind>, String> {
    match opt_arg::<String>(args, "kind")? {
        None => Ok(None),
        Some(k) => RoadKind::parse(&k).map(Some).ok_or(format!(
            "unknown road kind `{k}` (trail, footpath, dirt_road, gravel, cobblestone, paved)"
        )),
    }
}

fn options_arg(args: &Value, base: RoadOptions) -> Result<RoadOptions, String> {
    let Some(o) = args.get("options").filter(|v| v.is_object()) else {
        return Ok(base);
    };
    let b = |k: &str, d: bool| o.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
    Ok(RoadOptions {
        bed: b("bed", base.bed),
        clear: b("clear", base.clear),
        bridges: b("bridges", base.bridges),
        props: b("props", base.props),
    })
}

fn authored_height_at(road: &Road, p: Vec2) -> Option<f32> {
    let line = road.centreline(0.5);
    line.windows(2)
        .filter_map(|w| {
            let ab = w[1].xz() - w[0].xz();
            let u = ((p - w[0].xz()).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            let q = w[0].xz().lerp(w[1].xz(), u);
            Some((p.distance_squared(q), w[0].y.lerp(w[1].y, u)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, y)| y)
}

fn points_arg(
    world: &World,
    args: &Value,
    existing: Option<&Road>,
) -> Result<Option<Vec<[f32; 3]>>, String> {
    let Some(pts) = opt_arg::<Vec<Vec<f32>>>(args, "points")? else {
        return Ok(None);
    };
    let t = world.resource::<crate::terrain::Terrain>();
    let out: Vec<[f32; 3]> = pts
        .iter()
        .filter(|p| {
            p.len() >= 2
                && p[0].is_finite()
                && p[1].is_finite()
                && (p.len() < 3 || p[2].is_finite())
        })
        .map(|p| {
            let xz = Vec2::new(p[0], if p.len() >= 3 { p[2] } else { p[1] });
            let y = if p.len() >= 3 && p[2].is_finite() && p[1].is_finite() {
                p[1]
            } else if let Some(road) = existing.filter(|r| r.authored.is_some()) {
                authored_height_at(road, xz).unwrap_or_else(|| t.height_at(xz.x, xz.y))
            } else {
                t.height_at(xz.x, xz.y)
            };
            [xz.x, y, xz.y]
        })
        .collect();
    if out.len() < 2 {
        return Err("`points` needs at least two finite [x,z] or [x,y,z] points".into());
    }
    Ok(Some(out))
}

fn width_arg(args: &Value, default: f32) -> Result<f32, String> {
    Ok(opt_arg::<f32>(args, "width")?
        .unwrap_or(default)
        .clamp(0.8, 24.0))
}

/// Runs an edit with the same system parameters the tools use.
fn run_edit(world: &mut World, edit: Edit, label: &str) -> Result<u32, String> {
    let mut st: SystemState<RoadWorld> = SystemState::new(world);
    let mut rw = st
        .get_mut(world)
        .map_err(|e| format!("roads unavailable: {e}"))?;
    let r = rw.apply(edit, label);
    st.apply(world);
    r
}

fn describe(world: &mut World, id: u32) -> Value {
    let roads = world.resource::<super::Roads>();
    let Some(r) = roads.get(id) else {
        return json!({"id": id, "removed": true});
    };
    // the plan as the inspector sees it (fresh: the visuals update next frame)
    let t = world.resource::<crate::terrain::Terrain>();
    let water = world.resource::<crate::water::WaterBodies>();
    let base = super::ground::Base {
        t,
        g: &roads.ground,
    };
    let plan = super::plan::plan_all(roads, &base, water)
        .into_iter()
        .find(|p| p.id == id);
    let (len, maxg, avgg, bridges, bridge_len) = match &plan {
        Some(p) => {
            let (m, a) = p.grades();
            (
                p.length(),
                m,
                a,
                p.spans.len(),
                p.spans
                    .iter()
                    .map(|sp| p.samples[sp.i1].s - p.samples[sp.i0].s)
                    .sum::<f32>(),
            )
        }
        None => (r.length(), 0.0, 0.0, 0, 0.0),
    };
    json!({
        "id": r.id,
        "name": r.name,
        "kind": format!("{:?}", r.kind),
        "kind_name": r.kind.name(),
        "width": r.width,
        "length_m": (len * 10.0).round() / 10.0,
        "max_grade_pct": (maxg * 1000.0).round() / 10.0,
        "avg_grade_pct": (avgg * 1000.0).round() / 10.0,
        "bridges": bridges,
        "bridge_length_m": bridge_len.round().max(0.0),
        "cleared_vegetation": r.cleared.len(),
        "lamp_posts": r.prop_ids.len(),
        "options": {"bed": r.options.bed, "clear": r.options.clear, "bridges": r.options.bridges, "props": r.options.props},
        "points": r.points.iter().map(|p| {
            if r.authored.is_some() {
                json!([(p[0] * 10.0).round() / 10.0, (p[1] * 10.0).round() / 10.0, (p[2] * 10.0).round() / 10.0])
            } else {
                json!([(p[0] * 10.0).round() / 10.0, (p[2] * 10.0).round() / 10.0])
            }
        }).collect::<Vec<_>>(),
    })
}

fn list(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    let ids: Vec<u32> = world
        .resource::<super::Roads>()
        .roads
        .iter()
        .map(|r| r.id)
        .collect();
    let roads: Vec<Value> = ids.into_iter().map(|id| describe(world, id)).collect();
    Ok(vec![McpContent::Json(json!({"roads": roads}))])
}

fn create(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let kind = kind_arg(&args)?.ok_or("missing argument `kind`")?;
    let points = points_arg(world, &args, None)?.ok_or("missing argument `points`")?;
    let width = width_arg(&args, kind.default_width())?;
    let mut road = Road::new(
        0,
        kind,
        points,
        width,
        options_arg(&args, RoadOptions::default())?,
    );
    road.name = opt_arg::<String>(&args, "name")?.unwrap_or_default();
    let id = run_edit(world, Edit::Create(road), "Draw road")?;
    Ok(vec![McpContent::Json(describe(world, id))])
}

fn route(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let kind = kind_arg(&args)?.ok_or("missing argument `kind`")?;
    let from: Vec<f32> = arg(&args, "from")?;
    let to: Vec<f32> = arg(&args, "to")?;
    if from.len() < 2 || to.len() < 2 {
        return Err("`from` and `to` are [x,z]".into());
    }
    let (a, b) = (Vec2::new(from[0], from[1]), Vec2::new(to[0], to[1]));
    let width = width_arg(&args, kind.default_width())?;
    let input = {
        let t = world.resource::<crate::terrain::Terrain>();
        if !t.in_bounds(a.x, a.y) || !t.in_bounds(b.x, b.y) {
            return Err("`from` / `to` are outside the world".into());
        }
        let roads = world.resource::<super::Roads>();
        let base = super::ground::Base {
            t,
            g: &roads.ground,
        };
        super::route::prepare(
            &base,
            world.resource::<crate::water::WaterBodies>(),
            a,
            b,
            kind,
            width,
        )
    };
    let pts = super::route::solve(&input).ok_or("no route found between those points")?;
    let pts: Vec<[f32; 3]> = {
        let t = world.resource::<crate::terrain::Terrain>();
        pts.iter()
            .map(|p| [p.x, t.height_at(p.x, p.y), p.y])
            .collect()
    };
    let mut road = Road::new(
        0,
        kind,
        pts,
        width,
        options_arg(&args, RoadOptions::default())?,
    );
    road.name = opt_arg::<String>(&args, "name")?.unwrap_or_default();
    let id = run_edit(world, Edit::Create(road), "Auto route")?;
    Ok(vec![McpContent::Json(describe(world, id))])
}

fn update(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&args, "id")?;
    let mut road = world
        .resource::<super::Roads>()
        .get(id)
        .cloned()
        .ok_or(format!("no road {id}"))?;
    let only_rebuild = opt_arg::<bool>(&args, "rebuild")?.unwrap_or(false)
        && ["kind", "width", "name", "points", "options"]
            .iter()
            .all(|k| args.get(*k).is_none());
    if only_rebuild {
        run_edit(world, Edit::Rebuild(id), "Rebuild road")?;
        return Ok(vec![McpContent::Json(describe(world, id))]);
    }
    if let Some(k) = kind_arg(&args)? {
        if args.get("width").is_none() && (road.width - road.kind.default_width()).abs() < 0.01 {
            road.width = k.default_width();
        }
        road.kind = k;
    }
    road.width = width_arg(&args, road.width)?;
    if let Some(n) = opt_arg::<String>(&args, "name")? {
        road.name = n;
    }
    if let Some(p) = points_arg(world, &args, Some(&road))? {
        road.points = p;
    }
    road.options = options_arg(&args, road.options)?;
    run_edit(world, Edit::Update(road), "Edit road")?;
    Ok(vec![McpContent::Json(describe(world, id))])
}

fn remove(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: u32 = arg(&args, "id")?;
    run_edit(world, Edit::Remove(id), "Remove road")?;
    Ok(vec![McpContent::Text(format!("removed road {id}"))])
}
