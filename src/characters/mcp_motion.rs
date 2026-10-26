//! MCP tools for a character's motion (rig behaviors): the same code paths as the Motion tab.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::rigging::{self, NewPart, RigSpecs, Source};
use super::Characters;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};
use rigkit::spec::{PartKind, Preset};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "characters_motion",
        description: "A character's motion on top of its clips: feet planted on the ground (hips drop on slopes), looking at the viewer / player, style (posture −1 slouched … 1 proud, arm_swing ×, breathing 0–1), wind, and its moving parts (capes, skirts, hair, tails, pouches simulated as chains / cloth sheets / jiggle bones: preset, stiffness, damping, gravity, wind, stretch, bones). Also the loose pieces found on its model that could be made to move (`suggestions`, by index) and its mesh names. Waits for the model scan.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        run: motion,
    })
    .mcp_tool(McpTool {
        name: "characters_motion_set",
        description: "Change a character's motion settings (any subset): feet_on, feet_max_drop_m, feet_align (0–1), look_on, look_body (0–1: how much the chest turns), look_max_yaw_deg, look_range_m, posture (−1…1), arm_swing (0–2), breathing (0–1), wind (0–2); and one moving part's settings with `part`: {id, name, on, preset (silk, cotton, leather, heavy_wool, chainmail, hair, bouncy, stiff), stiffness, damping, gravity, wind, stretch, collide}. Applies at once to the stage and the world. Returns the motion.",
        input_schema: || json!({"type": "object", "properties": {
            "id": {"type": "string"},
            "feet_on": {"type": "boolean"}, "feet_max_drop_m": {"type": "number"}, "feet_align": {"type": "number"},
            "look_on": {"type": "boolean"}, "look_body": {"type": "number"}, "look_max_yaw_deg": {"type": "number"}, "look_range_m": {"type": "number"},
            "posture": {"type": "number"}, "arm_swing": {"type": "number"}, "breathing": {"type": "number"}, "wind": {"type": "number"},
            "part": {"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "on": {"type": "boolean"},
                "preset": {"type": "string"}, "stiffness": {"type": "number"}, "damping": {"type": "number"}, "gravity": {"type": "number"},
                "wind": {"type": "number"}, "stretch": {"type": "number"}, "collide": {"type": "boolean"}}, "required": ["id"]}},
            "required": ["id"]}),
        run: motion_set,
    })
    .mcp_tool(McpTool {
        name: "characters_parts_add",
        description: "Make part of a character move: a found loose piece (`suggestion`: its index from characters_motion) or a whole mesh (`mesh`: its name). Bones are laid through it (a chain, a cloth sheet of columns — closed round the body for a skirt — or one jiggle bone), it is skinned to them (blending into the body where attached) and simulated with gravity, wind and body collision. Optional: name, kind (chain, cloth, jiggle), preset, closed, rows (bones down), columns (cloth). Writes the character's next rig version; waits until done and returns the motion.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"},
            "suggestion": {"type": "integer"}, "mesh": {"type": "string"}, "name": {"type": "string"},
            "kind": {"type": "string", "enum": ["chain", "cloth", "jiggle"]}, "preset": {"type": "string"},
            "closed": {"type": "boolean"}, "rows": {"type": "integer"}, "columns": {"type": "integer"}}, "required": ["id"]}),
        run: parts_add,
    })
    .mcp_tool(McpTool {
        name: "characters_parts_remove",
        description: "Take a moving part out of a character (`part`: its id from characters_motion): its bones go and the vertices they moved follow the bone it hung from again. Waits until done.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}, "part": {"type": "string"}}, "required": ["id", "part"]}),
        run: parts_remove,
    })
    .mcp_tool(McpTool {
        name: "characters_fit_garment",
        description: "Dress a rigged character with a garment from a file on this computer (.fbx, .glb, .gltf, .obj or a .zip holding one): each garment vertex takes the skin weights of the body right under it, the rest are filled in from neighbours, sunk-in vertices are lifted out. A garment modelled in the character's own scene fits exactly; others are placed on its body's bounds. Its loose parts then show up in characters_motion suggestions. Waits until done.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}, "path": {"type": "string", "description": "absolute file path"}}, "required": ["id", "path"]}),
        run: fit_garment,
    })
    .mcp_tool(McpTool {
        name: "characters_copy",
        description: "Copy a built-in mannequin into the character library so it can be given moving parts and clothing. Returns the copy's id.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        run: copy,
    });
}

fn need_labs() -> Result<(), String> {
    if super::labs_enabled() {
        Ok(())
    } else {
        Err("the Characters lab is off: turn it on with characters_labs {\"enabled\": true}".into())
    }
}

fn motion_json(world: &mut World, id: &str) -> Option<Result<Value, String>> {
    let e = match world.resource::<Characters>().get(id) {
        Some(e) => e.clone(),
        None => return Some(Err("no such character".into())),
    };
    let mut specs = world.resource_mut::<RigSpecs>();
    let found = match specs.suggestions(&e) {
        Ok(None) => return None,
        Ok(Some(f)) => Some(f),
        Err(_) => None,
    };
    let spec = specs.get(&e).clone();
    let parts: Vec<Value> = spec
        .parts
        .iter()
        .map(|p| {
            json!({
                "id": p.id, "name": p.name, "kind": kind_id(p.kind), "on": p.on, "preset": p.preset.id(),
                "stiffness": p.stiffness, "damping": p.damping, "gravity": p.gravity, "wind": p.wind,
                "stretch": p.stretch, "collide": p.collide, "attach": p.attach, "closed": p.closed, "equipped": p.mesh,
                "columns": p.columns.len(), "bones": p.bone_count(),
            })
        })
        .collect();
    let suggestions: Vec<Value> = found
        .as_ref()
        .map(|f| {
            f.suggestions
                .iter()
                .enumerate()
                .map(|(i, s)| json!({"index": i, "name": s.name, "kind": kind_id(s.kind), "closed": s.closed,
                    "preset": s.preset.id(), "size_m": (s.size * 100.0).round() / 100.0, "vertices": s.region.len()}))
                .collect()
        })
        .unwrap_or_default();
    Some(Ok(json!({
        "id": id,
        "feet": {"on": spec.feet.on, "max_drop_m": spec.feet.max_drop, "align": spec.feet.align},
        "look": {"on": spec.look.on, "body": spec.look.body, "max_yaw_deg": spec.look.max_yaw_deg,
            "max_pitch_deg": spec.look.max_pitch_deg, "range_m": spec.look.range},
        "style": {"posture": spec.style.posture, "arm_swing": spec.style.arm_swing, "breathing": spec.style.breathing},
        "wind": spec.wind,
        "parts": parts,
        "suggestions": suggestions,
        "meshes": found.map(|f| f.meshes.clone()).unwrap_or_default(),
        "editable": e.builtin.is_none(),
    })))
}

fn kind_id(k: PartKind) -> &'static str {
    match k {
        PartKind::Chain => "chain",
        PartKind::Sheet => "cloth",
        PartKind::Jiggle => "jiggle",
    }
}

fn motion_reply(id: String) -> McpContent {
    McpContent::deferred(move |world: &mut World| {
        motion_json(world, &id).map(|r| r.map(|v| vec![McpContent::Json(v)]))
    })
}

/// Waits for the character's job, then replies with its motion (or the job's error).
fn after_job(id: String) -> McpContent {
    McpContent::deferred(move |world: &mut World| {
        if world.resource::<Characters>().busy(&id).is_some() {
            return None;
        }
        if let Some(Err(e)) = world.resource_mut::<RigSpecs>().outcome.remove(&id) {
            return Some(Err(e));
        }
        motion_json(world, &id).map(|r| r.map(|v| vec![McpContent::Json(v)]))
    })
}

fn motion(_world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    Ok(vec![motion_reply(id)])
}

fn motion_set(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let e = world.resource::<Characters>().get(&id).cloned().ok_or("no such character")?;
    let f = |k: &str| opt_arg::<f32>(&args, k);
    let b = |k: &str| opt_arg::<bool>(&args, k);
    let (feet_on, drop, align) = (b("feet_on")?, f("feet_max_drop_m")?, f("feet_align")?);
    let (look_on, body, yaw, range) = (b("look_on")?, f("look_body")?, f("look_max_yaw_deg")?, f("look_range_m")?);
    let (posture, swing, breath, wind) = (f("posture")?, f("arm_swing")?, f("breathing")?, f("wind")?);
    let part: Option<serde_json::Map<String, Value>> = opt_arg(&args, "part")?;
    let mut specs = world.resource_mut::<RigSpecs>();
    let mut spec = specs.get(&e).clone();
    if let Some(v) = feet_on { spec.feet.on = v; }
    if let Some(v) = drop { spec.feet.max_drop = v.clamp(0.0, 1.0); }
    if let Some(v) = align { spec.feet.align = v.clamp(0.0, 1.0); }
    if let Some(v) = look_on { spec.look.on = v; }
    if let Some(v) = body { spec.look.body = v.clamp(0.0, 1.0); }
    if let Some(v) = yaw { spec.look.max_yaw_deg = v.clamp(0.0, 120.0); }
    if let Some(v) = range { spec.look.range = v.clamp(0.0, 100.0); }
    if let Some(v) = posture { spec.style.posture = v.clamp(-1.0, 1.0); }
    if let Some(v) = swing { spec.style.arm_swing = v.clamp(0.0, 2.5); }
    if let Some(v) = breath { spec.style.breathing = v.clamp(0.0, 1.0); }
    if let Some(v) = wind { spec.wind = v.clamp(0.0, 3.0); }
    if let Some(pm) = part {
        let pid = pm.get("id").and_then(|v| v.as_str()).ok_or("part: give its id")?;
        let p = spec.part_mut(pid).ok_or_else(|| format!("no part {pid}"))?;
        let num = |k: &str| pm.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
        if let Some(s) = pm.get("preset").and_then(|v| v.as_str()) {
            p.apply_preset(Preset::from_id(s).ok_or_else(|| format!("unknown preset {s}"))?);
        }
        let mut custom = false;
        for (k, slot, max) in [
            ("stiffness", &mut p.stiffness, 1.0),
            ("damping", &mut p.damping, 1.0),
            ("gravity", &mut p.gravity, 3.0),
            ("wind", &mut p.wind, 3.0),
            ("stretch", &mut p.stretch, 0.3),
        ] {
            if let Some(v) = num(k) {
                *slot = v.clamp(0.0, max);
                custom = true;
            }
        }
        if custom {
            p.preset = Preset::Custom;
        }
        if let Some(v) = pm.get("on").and_then(|v| v.as_bool()) { p.on = v; }
        if let Some(v) = pm.get("collide").and_then(|v| v.as_bool()) { p.collide = v; }
        if let Some(v) = pm.get("name").and_then(|v| v.as_str()) { p.name = v.to_string(); }
    }
    specs.set(&e, spec);
    Ok(vec![motion_reply(id)])
}

fn parts_add(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let src = match (opt_arg::<usize>(&args, "suggestion")?, opt_arg::<String>(&args, "mesh")?) {
        (Some(i), _) => Source::Suggestion(i),
        (None, Some(m)) => Source::Mesh(m),
        _ => return Err("give `suggestion` (an index from characters_motion) or `mesh`".into()),
    };
    let kind = match opt_arg::<String>(&args, "kind")?.as_deref() {
        None => None,
        Some("chain") => Some(PartKind::Chain),
        Some("cloth" | "sheet") => Some(PartKind::Sheet),
        Some("jiggle") => Some(PartKind::Jiggle),
        Some(k) => return Err(format!("kind: unknown `{k}`")),
    };
    let preset = match opt_arg::<String>(&args, "preset")? {
        Some(s) => Some(Preset::from_id(&s).ok_or_else(|| format!("unknown preset {s}"))?),
        None => None,
    };
    let opts = NewPart {
        name: opt_arg(&args, "name")?,
        kind,
        preset,
        closed: opt_arg(&args, "closed")?,
        rows: opt_arg(&args, "rows")?,
        columns: opt_arg(&args, "columns")?,
    };
    // (a suggestion needs the scan done first)
    if let Source::Suggestion(_) = src {
        let e = world.resource::<Characters>().get(&id).cloned().ok_or("no such character")?;
        if world.resource_mut::<RigSpecs>().suggestions(&e)?.is_none() {
            return Ok(vec![McpContent::deferred(move |world: &mut World| {
                let e = world.resource::<Characters>().get(&id).cloned()?;
                match world.resource_mut::<RigSpecs>().suggestions(&e) {
                    Ok(None) => return None,
                    Err(err) => return Some(Err(err)),
                    Ok(Some(_)) => {}
                }
                let r = world.resource_scope(|world, mut specs: Mut<RigSpecs>| {
                    let mut chars = world.resource_mut::<Characters>();
                    rigging::add_part(&mut chars, &mut specs, &id, src.clone(), opts.clone())
                });
                Some(r.map(|()| vec![after_job(id.clone())]))
            })]);
        }
    }
    world.resource_scope(|world, mut specs: Mut<RigSpecs>| {
        let mut chars = world.resource_mut::<Characters>();
        rigging::add_part(&mut chars, &mut specs, &id, src, opts)
    })?;
    Ok(vec![after_job(id)])
}

fn parts_remove(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let part: String = arg(&args, "part")?;
    world.resource_scope(|world, mut specs: Mut<RigSpecs>| {
        let mut chars = world.resource_mut::<Characters>();
        rigging::remove_part(&mut chars, &mut specs, &id, &part)
    })?;
    Ok(vec![after_job(id)])
}

fn fit_garment(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let path: String = arg(&args, "path")?;
    let p = std::path::PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("no file at {path}"));
    }
    world.resource_scope(|world, mut specs: Mut<RigSpecs>| {
        let mut chars = world.resource_mut::<Characters>();
        rigging::fit_garment(&mut chars, &mut specs, &id, p)
    })?;
    Ok(vec![after_job(id)])
}

fn copy(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let nid = rigging::copy_builtin(&mut world.resource_mut::<Characters>(), &id)?;
    Ok(vec![McpContent::Json(json!({"id": nid}))])
}
