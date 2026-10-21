//! MCP tools for walking, motion modes, playtests and camera paths.

use std::path::PathBuf;

use bevy::prelude::*;
use serde_json::{Value, json};

use super::flythrough::{self, CameraPath, CaptureSettings, Flythrough};
use super::movement::{InWater, MoveMode};
use super::playtest::{self, Playtest};
use super::{MotionData, edit};
use crate::camera::{MainCamera, OrbitCamera};
use crate::history::History;
use crate::mcp_tools::{
    McpAppExt, McpContent, McpTool, bearing_schema, opt_arg, position_arg, position_schema,
    view_bearing, view_yaw,
};
use crate::walk::{WalkStart, WalkState};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "walk_start",
        description: "Enter walk mode (first-person exploration with collisions, swimming, riding and flying). Optional start `position` ([x, z] on the ground or [x, y, z]), `yaw_deg` (compass bearing to look along) and `mode`. Without a position it starts where the editor camera looks.",
        input_schema: || json!({"type": "object", "properties": {
            "position": position_schema("where to start"),
            "yaw_deg": bearing_schema("the direction to look"),
            "mode": {"type": "string", "enum": ["foot", "mount", "fly", "swim", "dive"]}}, "x-aliases": old_aliases()}),
        run: walk_start,
    })
    .mcp_tool(McpTool {
        name: "walk_stop",
        description: "Leave walk mode (or end a playtest session) and return to the editor camera.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: walk_stop,
    })
    .mcp_tool(McpTool {
        name: "set_motion_mode",
        description: "While walking, change how the explorer moves: foot, mount (horse, camera only), fly (free-fly noclip; optional speed m/s), swim (float at the surface), dive (go under; optional depth m), walk / run (pace on foot). Returns the resulting state.",
        input_schema: || json!({"type": "object", "properties": {
            "mode": {"type": "string", "enum": ["foot", "walk", "run", "mount", "fly", "swim", "dive"]},
            "speed": {"type": "number", "description": "fly speed (m/s)"},
            "depth": {"type": "number", "description": "dive depth below the surface (m)"}}, "required": ["mode"]}),
        run: set_motion_mode,
    })
    .mcp_tool(McpTool {
        name: "walk_state",
        description: "The explorer's state while walking: position, mode, speed, breath, footsteps (the surface underfoot as footsteps hear it: the ground's material, or the material of the model you stand on, e.g. Wood on a deck, Carpet / Cloth on a rug, Stone on a stair), `footfalls` heard so far and `steps_per_second` (the footstep cadence: about 2 walking, 2.9 running, 3.5 sprinting, 1.7 crouching; 0 standing still), the autopilot (walk_to: walking, arrived, stuck, timeout or cancelled; waypoint reached) and `interactable`: what the interact key (E) acts on now (a chest in reach and looked at: its name, whether open, the hint shown), else null. Read-only.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: walk_state,
    })
    .mcp_tool(McpTool {
        name: "walk_to",
        description: "While walking, walk the explorer along `path` (waypoints [[x, z], ...]) as a player holding W would: through doors (they open), up stairs and ramps, blocked by walls. Returns at once; read walk_state's autopilot for the outcome (walking, arrived, stuck: no progress for 3 s, timeout, cancelled) and the position it got to (its height shows whether stairs were climbed). `run` (default false: walking pace), `timeout_s` (default 60). An empty path stops it.",
        input_schema: || json!({"type": "object", "properties": {
            "path": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "waypoints [x, z] in order"},
            "run": {"type": "boolean"},
            "timeout_s": {"type": "number"}}, "required": ["path"]}),
        run: walk_to,
    })
    .mcp_tool(McpTool {
        name: "doors",
        description: "Hinged doors, gates and hatches of placed models near `position` ([x, z]; default: the explorer while walking, else the whole world) within `radius` (m, default 25): name, position, how open (0 shut .. 1 open) and what it holds to (auto, open or closed). While walking, doors swing open (away from you) as you walk up and shut behind you; doors are never solid, the wall opening is the way in. Pass `state` to hold the listed doors `open` or `closed`, or back to `auto`.",
        input_schema: || json!({"type": "object", "properties": {
            "position": position_schema("centre of the search"),
            "radius": {"type": "number", "description": "metres (default 25)"},
            "state": {"type": "string", "enum": ["open", "closed", "auto"]}}}),
        run: doors,
    })
    .mcp_tool(McpTool {
        name: "teleport",
        description: "Move the explorer (starts walk mode if needed) to `position` ([x, z] on the ground or [x, y, z]; dropped against something solid it steps aside on the same floor, inside something big it stands on top), optionally looking along `yaw_deg` (compass bearing).",
        input_schema: || json!({"type": "object", "properties": {"position": position_schema("where to go"), "yaw_deg": bearing_schema("the direction to look")}, "x-aliases": old_aliases()}),
        run: teleport,
    })
    .mcp_tool(McpTool {
        name: "playtest_start",
        description: "Start a playtest session at the gameplay player start (else where the camera looks): game HUD with compass, minimap and discoveries; the route is recorded.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: playtest_start,
    })
    .mcp_tool(McpTool {
        name: "playtest_stop",
        description: "End the running playtest session; returns its summary (time, distance, discoveries, longest gap).",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: playtest_stop,
    })
    .mcp_tool(McpTool {
        name: "playtest_status",
        description: "The running session's stats (time, distance, discoveries, position, mode) and the saved sessions (summaries; pass `route: true` for the recorded routes as [x, y, z, t] rows).",
        input_schema: || json!({"type": "object", "properties": {"route": {"type": "boolean"}}}),
        run: playtest_status,
    })
    .mcp_tool(McpTool {
        name: "camera_path_add_key",
        description: "Add a keyframe to a camera path (created if it doesn't exist; `path` is an id or a name, default the last path). The key is the current camera view unless `position` ([x, y, z], or [x, z] for 40 m above the ground), `yaw_deg` (compass bearing to look along) or `pitch_deg` (up + / down −) are given. `dur`: seconds to fly from the previous key (default by distance). Undoable.",
        input_schema: || json!({"type": "object", "properties": {
            "path": {"type": ["string", "integer"]}, "position": position_schema("camera position"),
            "yaw_deg": bearing_schema("the direction to look"), "pitch_deg": {"type": "number", "minimum": -89, "maximum": 89, "description": "look up (+) or down (−), degrees"},
            "fov": {"type": "number", "description": "field of view (degrees, 10..120)"}, "dur": {"type": "number"},
            "ease": {"type": "string", "enum": ["linear", "inout", "in", "out"]}}, "x-aliases": old_aliases()}),
        run: camera_path_add_key,
    })
    .mcp_tool(McpTool {
        name: "camera_path_from_places",
        description: "Make a camera flight through places on the ground, in order (the editor's Pick places): the keys are generated along a smooth curve that clears the hills. One place makes a circle around it. `style`: glide (~30 m over the ground, looking ahead), aerial (bird's-eye, ~95 m, the default) or showcase (high, turning to each place); `speed`: slow, normal or fast. `path` (id or name) re-makes that flight instead of adding one (its style and speed are kept unless given). Undoable; play it with camera_path_play.",
        input_schema: || json!({"type": "object", "properties": {
            "places": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 3}, "minItems": 1, "description": "[x, z] ground positions in metres, in flight order"},
            "style": {"type": "string", "enum": ["glide", "aerial", "showcase"]},
            "speed": {"type": "string", "enum": ["slow", "normal", "fast"]},
            "name": {"type": "string"},
            "path": {"type": ["string", "integer"], "description": "an existing flight to re-make (id or name)"}}, "required": ["places"]}),
        run: camera_path_from_places,
    })
    .mcp_tool(McpTool {
        name: "camera_path_list",
        description: "List the camera paths (keys, timing, duration) and the state of playback / the last capture.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: camera_path_list,
    })
    .mcp_tool(McpTool {
        name: "camera_path_play",
        description: "Preview a camera path full screen (editor UI hidden; Esc or walk_start stops it). `path`: id or name (default the last).",
        input_schema: || json!({"type": "object", "properties": {"path": {"type": ["string", "integer"]}}}),
        run: camera_path_play,
    })
    .mcp_tool(McpTool {
        name: "capture_flythrough",
        description: "Render a camera path to video: frames at a fixed time step, encoded to an H.264 MP4 with ffmpeg (PNG frames without ffmpeg) in `dir`. Runs in the background; camera_path_list reports progress and the output file.",
        input_schema: || json!({"type": "object", "properties": {
            "path": {"type": ["string", "integer"]}, "dir": {"type": "string"}, "fps": {"type": "integer"},
            "width": {"type": "integer"}, "height": {"type": "integer"}, "keep_frames": {"type": "boolean"}}, "required": ["dir"]}),
        run: capture_flythrough,
    });
}

/// The old argument forms, still accepted: separate x / y / z, `yaw` and `pitch` in radians.
fn old_aliases() -> Value {
    json!({"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}, "yaw": {"type": "number"}, "pitch": {"type": "number"}})
}

/// The view yaw from `yaw_deg` (compass bearing) or the old `yaw` (radians).
fn yaw_of(args: &Value) -> Result<Option<f32>, String> {
    Ok(match opt_arg::<f32>(args, "yaw_deg")? {
        Some(b) => Some(view_yaw(b)),
        None => opt_arg(args, "yaw")?,
    })
}

fn state(w: &WalkState) -> Value {
    let (label, target) = super::movement::mode_label(w);
    json!({
        "active": w.active,
        "playtest": w.playtest,
        "position": [w.position.x, w.position.y, w.position.z],
        "yaw_deg": view_bearing(w.yaw),
        "mode": format!("{:?}", w.mode).to_lowercase(),
        "water": format!("{:?}", w.water).to_lowercase(),
        "moving": label,
        "target_speed": target,
        "speed": w.velocity.length(),
        "breath": w.breath,
        "fly_speed": w.fly_speed,
        "underwater": w.submerged > 0.5,
        // what the footsteps sound like: the ground, or the material of the object underfoot
        "footsteps": w.surface.map(|s| s.name()),
        // footfalls heard since walking began and the cadence (steps per second; 0 standing still)
        "footfalls": w.footfalls,
        "steps_per_second": (w.step_rate * 100.0).round() / 100.0,
        // walk_to: how the last walk along waypoints went
        "autopilot": w.autopilot.as_ref().map(|a| json!({
            "state": a.result.unwrap_or("walking"),
            "waypoint": a.next.min(a.points.len()),
            "waypoints": a.points.len(),
            "seconds": a.elapsed,
        })),
    })
}

fn walk_state(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    let mut v = state(&world.resource::<WalkState>());
    // what the interact key (E) would act on: the chest in reach
    v["interactable"] = crate::model_render::lid::focus_json(world).unwrap_or(Value::Null);
    Ok(vec![McpContent::Json(v)])
}

fn walk_to(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let path: Vec<[f32; 2]> = opt_arg(&args, "path")?.ok_or("missing `path` [[x, z], ...]")?;
    let run = opt_arg::<bool>(&args, "run")?.unwrap_or(false);
    let timeout = opt_arg::<f32>(&args, "timeout_s")?
        .unwrap_or(60.0)
        .clamp(1.0, 600.0);
    let mut w = world.resource_mut::<WalkState>();
    if path.is_empty() {
        w.autopilot = None;
        return Ok(vec![McpContent::Text("Autopilot off.".into())]);
    }
    if !w.active {
        return Err("not walking: walk_start or teleport first".into());
    }
    let n = path.len();
    w.autopilot = Some(crate::walk::Autopilot::new(
        path.into_iter().map(|[x, z]| Vec2::new(x, z)).collect(),
        run,
        timeout,
    ));
    Ok(vec![McpContent::Text(format!(
        "Walking {n} waypoint(s); read walk_state's autopilot for the outcome."
    ))])
}

fn doors(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    use crate::model_render::door::{Door, DoorHold};
    let hold = match args.get("state").and_then(|v| v.as_str()) {
        Some(s) => Some(DoorHold::parse(s).ok_or("`state` must be open, closed or auto")?),
        None => None,
    };
    let radius = opt_arg::<f32>(&args, "radius")?.unwrap_or(25.0).max(0.1);
    let centre = match position_arg(&args, "position")? {
        Some((p, _)) => Some(p),
        None => {
            let w = world.resource::<WalkState>();
            w.active.then(|| Vec2::new(w.position.x, w.position.z))
        }
    };
    let mut q = world.query::<(&mut Door, &GlobalTransform, Option<&Name>)>();
    let mut out = Vec::new();
    for (mut d, gt, name) in q.iter_mut(world) {
        let p = gt.translation();
        let dist = centre.map(|c| c.distance(Vec2::new(p.x, p.z)));
        if dist.is_some_and(|r| r > radius) {
            continue;
        }
        if let Some(h) = hold {
            d.hold = h;
        }
        out.push((
            dist.unwrap_or(0.0),
            json!({
                "name": name.map(|n| n.as_str().to_string()).unwrap_or_default(),
                "position": [p.x, p.y, p.z],
                "distance": dist,
                "open": d.openness(),
                "hold": d.hold.name(),
            }),
        ));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let n = out.len();
    let list: Vec<Value> = out.into_iter().take(100).map(|(_, v)| v).collect();
    Ok(vec![McpContent::Json(
        json!({"count": n, "doors": list, "set": hold.map(|h| h.name())}),
    )])
}

fn mode_of(s: &str) -> Option<MoveMode> {
    match s {
        "foot" | "walk" | "run" | "swim" | "dive" => Some(MoveMode::Foot),
        "mount" => Some(MoveMode::Mount),
        "fly" => Some(MoveMode::Fly),
        _ => None,
    }
}

fn walk_start(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let at = position_arg(&args, "position")?;
    let yaw = yaw_of(&args)?;
    let mode: Option<String> = opt_arg(&args, "mode")?;
    let pos = at.map(|(p, y)| {
        let ground = world
            .resource::<crate::terrain::Terrain>()
            .height_at(p.x, p.y);
        Vec3::new(p.x, y.unwrap_or(ground), p.y)
    });
    let m = mode
        .as_deref()
        .map(|m| mode_of(m).ok_or(format!("unknown mode `{m}`")))
        .transpose()?;
    if world.resource::<Flythrough>().playing.is_some() {
        return Err("a camera path is playing".into());
    }
    world.resource_mut::<WalkState>().request = Some(WalkStart {
        pos,
        yaw,
        mode: m,
        playtest: false,
    });
    if let Some(m) = mode.filter(|m| m == "swim" || m == "dive") {
        // (applied once walking has started)
        world.resource_mut::<super::hooks::Pending>().water = Some((m, 3.0));
    }
    Ok(vec![McpContent::Text(
        "Walk mode starts next frame (walk_stop or Esc returns to the editor).".into(),
    )])
}

fn walk_stop(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    if !world.resource::<WalkState>().active {
        return Ok(vec![McpContent::Text("Not walking.".into())]);
    }
    world.write_message(crate::ui::UiAction::ToggleWalk);
    Ok(vec![McpContent::Text("Leaving walk mode.".into())])
}

/// Put the explorer in the water: floating at the surface or `depth` m under it.
pub fn enter_water(world: &mut World, dive: bool, depth: f32) -> Result<(), String> {
    let p = world.resource::<WalkState>().position;
    let surface = {
        let t = world.resource::<crate::terrain::Terrain>();
        super::waves::surface_at(
            t,
            world.resource::<crate::water::WaterBodies>(),
            world.resource::<crate::water::WaterStyle>(),
            world.resource::<crate::ocean::OceanWaves>(),
            p.xz(),
            world.resource::<Time>(),
        )
    };
    let ground = world
        .resource::<crate::terrain::Terrain>()
        .height_at(p.x, p.z);
    let Some(l) = surface.filter(|l| *l - ground > 1.4) else {
        return Err("there is no deep water here".into());
    };
    let mut w = world.resource_mut::<WalkState>();
    w.set_mode(MoveMode::Foot);
    w.velocity = Vec3::ZERO;
    if dive {
        let eye = (l - depth.max(0.5)).max(ground + 0.7);
        w.water = InWater::Diving;
        w.eye_off = super::movement::EYE_DIVE;
        w.position.y = eye - w.eye_off;
    } else {
        w.water = InWater::Swimming;
        w.eye_off = super::movement::EYE_SWIM;
        w.position.y = l + 0.2 - w.eye_off;
    }
    Ok(())
}

fn set_motion_mode(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let mode: String = crate::mcp_tools::arg(&args, "mode")?;
    let speed: Option<f32> = opt_arg(&args, "speed")?;
    let depth: Option<f32> = opt_arg(&args, "depth")?;
    if !world.resource::<WalkState>().active {
        return Err("not walking (call walk_start first)".into());
    }
    match mode.as_str() {
        "swim" | "dive" => enter_water(world, mode == "dive", depth.unwrap_or(3.0))?,
        m => {
            let mm = mode_of(m).ok_or(format!("unknown mode `{m}`"))?;
            let mut w = world.resource_mut::<WalkState>();
            w.set_mode(mm);
            if m == "walk" || m == "run" {
                w.set_walk_pace(m == "walk");
            }
            if let Some(s) = speed {
                w.fly_speed = s.clamp(2.0, 400.0);
            }
        }
    }
    Ok(vec![McpContent::Json(state(
        &world.resource::<WalkState>(),
    ))])
}

fn teleport(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let (p, y) = position_arg(&args, "position")?.ok_or("missing `position` [x, z]")?;
    let (x, z) = (p.x, p.y);
    let yaw = yaw_of(&args)?;
    let (ground, inside) = {
        let t = world.resource::<crate::terrain::Terrain>();
        (t.height_at(x, z), t.in_bounds(x, z))
    };
    if !inside {
        return Err("outside the world".into());
    }
    // in a cave's or a dungeon's space: on its floor there (an underground dungeon map: the top
    // floor under the spot when no height is given)
    let cw = world.resource::<crate::caves::render::CaveWorld>();
    let under = match y {
        Some(y) => cw.floor_under(Vec3::new(x, y + 1.2, z), 3.0),
        None => cw
            .dungeon
            .as_ref()
            .filter(|d| d.interior)
            .and_then(|d| d.raycast(Vec3::new(x, d.bounds().1.y - 0.5, z), Vec3::NEG_Y, 2000.0))
            .map(|(h, _)| h.y),
    };
    // (underground there's nothing but the dungeon: a spot in its rock is no place to stand)
    if under.is_none()
        && let Some(d) = cw.dungeon.as_ref().filter(|d| d.interior)
    {
        let q = Vec2::new(x, z);
        let near = d
            .plan
            .rooms
            .iter()
            .filter(|r| r.kind.is_space())
            .map(|r| r.shape.centre())
            .min_by(|a, b| a.distance(q).total_cmp(&b.distance(q)));
        return Err(match near {
            Some(c) => format!(
                "[{x:.0}, {z:.0}] is inside the dungeon's rock; the nearest room's middle is [{:.0}, {:.0}] (dungeons_rooms lists them)",
                c.x, c.y
            ),
            None => "the dungeon has no rooms yet".into(),
        });
    }
    let pos = match under {
        Some(f) => Vec3::new(x, f, z),
        None => Vec3::new(x, y.unwrap_or(ground).max(ground), z),
    };
    let mut w = world.resource_mut::<WalkState>();
    if w.active {
        w.teleport(pos, yaw);
        Ok(vec![McpContent::Text(format!(
            "Teleported to ({x:.1}, {:.1}, {z:.1}).",
            pos.y
        ))])
    } else {
        w.request = Some(WalkStart {
            pos: Some(pos),
            yaw,
            mode: None,
            playtest: false,
        });
        Ok(vec![McpContent::Text(format!(
            "Walk mode starts at ({x:.1}, {:.1}, {z:.1}).",
            pos.y
        ))])
    }
}

fn playtest_start(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    if world.resource::<Flythrough>().playing.is_some() {
        return Err("a camera path is playing".into());
    }
    let focus = world.resource::<OrbitCamera>().focus;
    let gameplay = world.resource::<crate::gameplay::Gameplay>().clone();
    let at = playtest::spawn_point(&gameplay, focus).0;
    world.resource_scope(|world, mut pt: Mut<Playtest>| {
        let mut walk = world.resource_mut::<WalkState>();
        playtest::begin(&mut pt, &mut walk, &gameplay, focus);
    });
    let from = if gameplay.player_start.is_some() {
        "the player start"
    } else {
        "the camera focus"
    };
    Ok(vec![McpContent::Text(format!(
        "Playtest started at {from} ({:.0}, {:.0}, {:.0}); {} POIs to discover.",
        at.x,
        at.y,
        at.z,
        gameplay.pois.len()
    ))])
}

fn live_json(pt: &Playtest, walk: &WalkState) -> Value {
    match &pt.live {
        Some(l) => json!({
            "running": true,
            "time_s": l.t,
            "distance_m": l.distance,
            "discoveries": l.found.iter().map(|d| json!({"t": d.t, "poi": d.poi, "name": d.name})).collect::<Vec<_>>(),
            "poi_count": l.poi_count,
            "breadcrumbs": l.route.len(),
            "explorer": state(walk),
        }),
        None => json!({"running": false}),
    }
}

fn playtest_stop(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    let (running, live) = {
        let pt = world.resource::<Playtest>();
        (
            pt.live.is_some(),
            live_json(pt, world.resource::<WalkState>()),
        )
    };
    if !running {
        return Err("no playtest is running".into());
    }
    if world.resource::<WalkState>().active {
        world.write_message(crate::ui::UiAction::ToggleWalk);
    }
    Ok(vec![
        McpContent::Text("Playtest ends next frame (the session is saved with the world).".into()),
        McpContent::Json(live),
    ])
}

fn playtest_status(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let route: bool = opt_arg(&args, "route")?.unwrap_or(false);
    let live = live_json(world.resource::<Playtest>(), world.resource::<WalkState>());
    let sessions: Vec<Value> = world
        .resource::<MotionData>()
        .sessions
        .iter()
        .map(|s| {
            let mut v = s.export_json();
            if !route && let Some(o) = v.as_object_mut() {
                o.remove("route");
                o.remove("route_columns");
                o.insert("breadcrumbs".into(), json!(s.points().len()));
            }
            v
        })
        .collect();
    Ok(vec![McpContent::Json(
        json!({"live": live, "sessions": sessions}),
    )])
}

/// A path by id or name (default: the last one).
fn find_path(data: &MotionData, sel: &Option<Value>) -> Option<u32> {
    match sel {
        Some(Value::Number(n)) => data
            .paths
            .iter()
            .find(|p| Some(p.id as u64) == n.as_u64())
            .map(|p| p.id),
        Some(Value::String(s)) => data
            .paths
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(s) || p.id.to_string() == *s)
            .map(|p| p.id),
        _ => data.paths.last().map(|p| p.id),
    }
}

/// Add a key (the current view unless given) to a path, creating the path if needed.
pub fn add_key(world: &mut World, args: &Value) -> Result<(u32, usize), String> {
    let sel: Option<Value> = opt_arg(args, "path")?;
    let num = |k: &str| opt_arg::<f32>(args, k);
    let at = position_arg(args, "position")?;
    let (x, y, z) = (
        at.map(|a| a.0.x),
        at.and_then(|a| a.1).or(num("y")?),
        at.map(|a| a.0.y),
    );
    let yaw = yaw_of(args)?;
    let pitch = num("pitch_deg")?
        .map(|p| p.clamp(-89.0, 89.0).to_radians())
        .or(num("pitch")?);
    let (fov, dur) = (num("fov")?, num("dur")?);
    let ease: Option<String> = opt_arg(args, "ease")?;
    let view = world
        .query_filtered::<(&GlobalTransform, &Projection), With<MainCamera>>()
        .single(world)
        .ok()
        .map(|(t, p)| {
            flythrough::key_from_view(
                t,
                if let Projection::Perspective(p) = p {
                    p.fov.to_degrees()
                } else {
                    50.0
                },
                4.0,
            )
        })
        .ok_or("no camera")?;
    let mut key = view;
    if let (Some(x), Some(z)) = (x, z) {
        let ground = world.resource::<crate::terrain::Terrain>().height_at(x, z);
        key.pos = [x, y.unwrap_or(ground + 40.0), z];
    } else if let Some(y) = y {
        key.pos[1] = y;
    }
    key.yaw = yaw.unwrap_or(key.yaw);
    key.pitch = pitch.unwrap_or(key.pitch);
    key.fov = fov.unwrap_or(key.fov).clamp(10.0, 120.0);
    key.ease = match ease.as_deref() {
        Some("inout") => flythrough::Ease::InOut,
        Some("in") => flythrough::Ease::In,
        Some("out") => flythrough::Ease::Out,
        _ => flythrough::Ease::Linear,
    };
    let out = world.resource_scope(|world, mut data: Mut<MotionData>| {
        let mut history = world.resource_mut::<History>();
        let named = match &sel {
            Some(Value::String(s)) if find_path(&data, &sel).is_none() => Some(s.clone()),
            _ => None,
        };
        let mut res = (0, 0);
        edit(&mut data, &mut history, "Add camera key", |d| {
            let id = match find_path(d, &sel).filter(|_| named.is_none()) {
                Some(id) => id,
                None => {
                    let id = d.alloc_id();
                    let name = named
                        .clone()
                        .unwrap_or_else(|| format!("Path {}", d.paths.len() + 1));
                    d.paths.push(CameraPath {
                        id,
                        name,
                        keys: vec![],
                        timelapse_hours: 0.0,
                        looped: false,
                        route: None,
                    });
                    id
                }
            };
            let Some(p) = d.paths.iter_mut().find(|p| p.id == id) else {
                return false;
            };
            if let Some(prev) = p.keys.last_mut() {
                prev.dur = dur
                    .unwrap_or_else(|| flythrough::auto_duration(prev, key.pos))
                    .max(0.2);
            }
            p.keys.push(key);
            res = (id, p.keys.len());
            true
        });
        res
    });
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(out)
}

fn camera_path_add_key(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let (id, n) = add_key(world, &args)?;
    Ok(vec![McpContent::Text(format!(
        "Path {id} now has {n} keys."
    ))])
}

fn camera_path_from_places(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let places: Vec<Vec<f32>> = crate::mcp_tools::arg(&args, "places")?;
    let places: Vec<Vec2> = places
        .iter()
        .map(|p| match p[..] {
            [x, z] | [x, _, z] if x.is_finite() && z.is_finite() => Ok(Vec2::new(x, z)),
            _ => Err("each place must be [x, z] (metres)".to_string()),
        })
        .collect::<Result<_, _>>()?;
    if places.is_empty() {
        return Err("give at least one place".into());
    }
    let style = match opt_arg::<String>(&args, "style")? {
        Some(s) => Some(
            flythrough::FlightStyle::from_id(&s)
                .ok_or("style must be glide, aerial or showcase")?,
        ),
        None => None,
    };
    let speed = match opt_arg::<String>(&args, "speed")? {
        Some(s) => {
            Some(flythrough::FlightSpeed::from_id(&s).ok_or("speed must be slow, normal or fast")?)
        }
        None => None,
    };
    let name: Option<String> = opt_arg(&args, "name")?;
    let sel: Option<Value> = opt_arg(&args, "path")?;
    let replace = match &sel {
        Some(_) => {
            Some(find_path(world.resource::<MotionData>(), &sel).ok_or("no such camera path")?)
        }
        None => None,
    };
    let id = world.resource_scope(|world, mut data: Mut<MotionData>| {
        world.resource_scope(|world, mut history: Mut<History>| {
            let terrain = world.resource::<crate::terrain::Terrain>();
            super::pick::make_flight(
                &mut data,
                &mut history,
                terrain,
                &places,
                style,
                speed,
                name,
                replace,
            )
        })
    });
    let id = id.ok_or("couldn't make the flight")?;
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    let data = world.resource::<MotionData>();
    let p = data
        .paths
        .iter()
        .find(|p| p.id == id)
        .ok_or("flight vanished")?;
    let r = p.route.as_ref().ok_or("flight has no route")?;
    let lowest = p
        .keys
        .iter()
        .map(|k| {
            k.pos[1]
                - world
                    .resource::<crate::terrain::Terrain>()
                    .height_at(k.pos[0], k.pos[2])
        })
        .fold(f32::INFINITY, f32::min);
    Ok(vec![McpContent::Json(json!({
        "id": id, "name": p.name, "places": r.places.len(), "style": r.style.id(), "speed": r.speed.id(),
        "keys": p.keys.len(), "duration_s": (p.duration() * 10.0).round() / 10.0, "lowest_clearance_m": lowest.round(),
    }))])
}

fn camera_path_list(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    let data = world.resource::<MotionData>();
    let fly = world.resource::<Flythrough>();
    let paths: Vec<Value> = data
        .paths
        .iter()
        .map(|p| {
            json!({
                "id": p.id, "name": p.name, "duration_s": p.duration(), "timelapse_hours": p.timelapse_hours, "looped": p.looped,
                "route": p.route.as_ref().map(|r| json!({"places": r.places, "style": r.style.id(), "speed": r.speed.id()})),
                "keys": p.keys.iter().map(|k| json!({"pos": k.pos, "yaw_deg": view_bearing(k.yaw), "pitch_deg": (k.pitch.to_degrees() * 10.0).round() / 10.0, "fov": k.fov, "dur": k.dur, "ease": format!("{:?}", k.ease)})).collect::<Vec<_>>(),
            })
        })
        .collect();
    let playing = fly.playing.as_ref().map(|p| {
        let cap = p.capture.as_ref().map(|c| {
            let st = c.status.lock().map(|s| s.clone()).unwrap_or_default();
            json!({"frames": c.total, "requested": c.next, "written": st.written, "dir": c.settings.dir})
        });
        json!({"path": p.path, "t": p.t, "capture": cap})
    });
    let last = fly
        .last
        .as_ref()
        .map(|s| json!({"output": s.output, "frames": s.written, "error": s.error}));
    Ok(vec![McpContent::Json(
        json!({"paths": paths, "playing": playing, "last_capture": last, "ffmpeg": flythrough::ffmpeg_path()}),
    )])
}

fn camera_path_play(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let sel: Option<Value> = opt_arg(&args, "path")?;
    let id = find_path(world.resource::<MotionData>(), &sel).ok_or("no such camera path")?;
    if world.resource::<WalkState>().active {
        return Err("leave walk mode first (walk_stop)".into());
    }
    world.resource_mut::<Flythrough>().play(id, None);
    Ok(vec![McpContent::Text(format!("Playing path {id}."))])
}

fn capture_flythrough(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let sel: Option<Value> = opt_arg(&args, "path")?;
    let dir: String = crate::mcp_tools::arg(&args, "dir")?;
    let d = CaptureSettings::default();
    let settings = CaptureSettings {
        dir: PathBuf::from(dir),
        fps: opt_arg::<u32>(&args, "fps")?.unwrap_or(d.fps).clamp(1, 120),
        width: opt_arg::<u32>(&args, "width")?
            .unwrap_or(d.width)
            .clamp(64, 7680)
            & !1,
        height: opt_arg::<u32>(&args, "height")?
            .unwrap_or(d.height)
            .clamp(64, 4320)
            & !1,
        keep_frames: opt_arg(&args, "keep_frames")?.unwrap_or(false),
    };
    let data = world.resource::<MotionData>();
    let id = find_path(data, &sel).ok_or("no such camera path")?;
    let dur = data
        .paths
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.duration())
        .unwrap_or(0.0);
    if world.resource::<WalkState>().active {
        return Err("leave walk mode first (walk_stop)".into());
    }
    let frames = (dur * settings.fps as f32).ceil() as usize + 1;
    let msg = format!(
        "Capturing path {id}: {frames} frames ({dur:.1} s at {} fps, {}×{}) to {} ({}).",
        settings.fps,
        settings.width,
        settings.height,
        settings.dir.display(),
        if flythrough::ffmpeg_path().is_some() {
            "MP4 via ffmpeg"
        } else {
            "PNG frames: ffmpeg not found"
        }
    );
    world.resource_mut::<Flythrough>().play(id, Some(settings));
    Ok(vec![McpContent::Text(msg)])
}
