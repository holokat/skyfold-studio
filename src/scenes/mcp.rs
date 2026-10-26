//! Scenes over MCP: list, save (from a loop, ids or a set), place, export, import, rename, delete.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::file;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "scenes_list",
        description: "The user's saved scenes (a decorated spot saved for reuse: a shrine with its props, a camp, a temple inside and out): each scene's id, name, description, piece count, size (m), the assets it uses (with counts) and whether every piece can be placed here. Place one with scenes_place. Also the ready-made scenes of the asset packs this account can use (complete scenes in one model, e.g. a monster-encounter site): id, name, size; place one with objects_place.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: list,
    });
    app.mcp_tool(McpTool {
        name: "scenes_save",
        description: "Save placed objects as a scene in the user's library (like Objects › Scenes › Save a scene): everything inside `polygon` [[x, z], …], or the objects `ids`, or a set's `group`. Each piece keeps its exact placement relative to the others and its height above the ground. The user's imported models are copied into the scene. `name` (required), `description`. Returns the scene id.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "polygon": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "minItems": 3, "description": "the area [[x, z], …]: every object whose middle is inside" },
                    "ids": { "type": "array", "items": { "type": "integer" } },
                    "group": { "type": "integer", "description": "a set piece, town or placed scene" }
                },
                "required": ["name"]
            })
        },
        run: |w, a| w.run_system_cached_with(save, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "scenes_place",
        description: "Place a saved scene (an id from scenes_list) with its middle at `position` [x, z], turned by `yaw_deg` (clockwise seen from above). It lands as one set (objects_group moves, turns, resizes or ungroups it); buildings level their ground, the other pieces sit on the ground at their spots and nature is cleared from its area. Pieces from asset packs this account hasn't got are left out and listed. Returns the group, ids and anything left out. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "scene": { "type": "string", "description": "scene id" },
                    "position": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3 },
                    "yaw_deg": { "type": "number", "default": 0 }
                },
                "required": ["scene", "position"]
            })
        },
        run: |w, a| w.run_system_cached_with(place, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "scenes_export",
        description: "Export a saved scene to a file: `format` zip (a scene file that opens in any Skyfold Studio; the user's own models inside) or glb (one glTF model with every piece in place, for Unreal, Unity, Godot or Blender). `path`: absolute, ending in .zip or .glb.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "scene": { "type": "string" },
                    "format": { "type": "string", "enum": ["zip", "glb"] },
                    "path": { "type": "string" }
                },
                "required": ["scene", "format", "path"]
            })
        },
        run: export,
    });
    app.mcp_tool(McpTool {
        name: "scenes_import",
        description: "Open a scene file (like Objects › Scenes › Open a scene file…): `path` to a scene .zip exported from Skyfold Studio (scenes_export). It joins the user's scenes, its own models copied into the project. Returns the scene id; place it with scenes_place.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "absolute path to the scene .zip" }
                },
                "required": ["path"]
            })
        },
        run: import,
    });
    app.mcp_tool(McpTool {
        name: "scenes_rename",
        description: "Rename one of the user's saved scenes (an id from scenes_list) to `name`. Its id stays the same.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "scene": { "type": "string", "description": "scene id" },
                    "name": { "type": "string" }
                },
                "required": ["scene", "name"]
            })
        },
        run: rename,
    });
    app.mcp_tool(McpTool {
        name: "scenes_delete",
        description: "Delete one of the user's saved scenes (an id from scenes_list) from their library. Copies already placed in the world stay. Ready-made scenes from asset packs can't be deleted. Can't be undone.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "scene": { "type": "string", "description": "scene id" }
                },
                "required": ["scene"]
            })
        },
        run: delete,
    });
}

fn scene_arg(args: &Value) -> Result<&str, String> {
    args.get("scene")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing `scene`".to_string())
}

/// The Scenes tab reads the library again.
fn reload(world: &mut World) {
    if let Some(mut s) = world.get_resource_mut::<super::Scenes>() {
        s.reload();
    }
}

fn import(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let path = std::path::PathBuf::from(
        args.get("path")
            .and_then(|v| v.as_str())
            .ok_or("missing `path`")?,
    );
    if !path.is_absolute() {
        return Err("`path` must be absolute".into());
    }
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
    {
        return Err("a scene file ends in .zip".into());
    }
    if !path.is_file() {
        return Err(format!("no file at {}", path.display()));
    }
    let id = file::import_zip(&path).map_err(|e| format!("couldn't open that scene: {e}"))?;
    reload(world);
    let scene = file::read(&id).ok_or("the scene was opened but can't be read back")?;
    let sz = scene.size();
    Ok(vec![McpContent::Json(
        json!({ "scene": id, "name": scene.name, "pieces": scene.pieces.len(), "size_m": [sz.x.round(), sz.y.round()], "models": scene.models.len(), "note": "in the user's Objects › Scenes; place it with scenes_place" }),
    )])
}

fn rename(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id = scene_arg(&args)?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("give the scene a `name`".into());
    }
    let mut sc = file::read(id).ok_or_else(|| format!("no scene `{id}` (scenes_list)"))?;
    let old = std::mem::replace(&mut sc.name, name.clone());
    file::write(&sc).map_err(|e| format!("couldn't rename it: {e}"))?;
    reload(world);
    Ok(vec![McpContent::Text(format!(
        "Renamed scene `{id}` from \"{old}\" to \"{name}\"."
    ))])
}

fn delete(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id = scene_arg(&args)?;
    let sc = file::read(id).ok_or_else(|| {
        format!("no saved scene `{id}` (scenes_list; ready-made scenes can't be deleted)")
    })?;
    file::delete(id)?;
    if let Some(mut s) = world.get_resource_mut::<super::Scenes>() {
        // (as the tab does: a scene being placed, or armed for delete, is let go)
        if s.mode == super::Mode::Place(id.to_string()) {
            s.mode = super::Mode::Off;
        }
        if s.confirm_delete.as_deref() == Some(id) {
            s.confirm_delete = None;
        }
        s.reload();
    }
    Ok(vec![McpContent::Text(format!(
        "Deleted scene `{id}` \"{}\" from the user's scenes (placed copies stay).",
        sc.name
    ))])
}

fn list(world: &mut World, _args: Value) -> Result<Vec<McpContent>, String> {
    let lib = world.get_resource::<crate::objects::Library>();
    let m = crate::market::snapshot();
    let out: Vec<Value> = file::list()
        .iter()
        .map(|s| {
            let sz = s.size();
            let bundled: Vec<&str> = s.models.iter().map(|b| b.asset.as_str()).collect();
            let assets: Vec<Value> = s.assets().into_iter().map(|(a, n)| json!({ "asset": a, "pieces": n })).collect();
            let blocked = s.pieces.iter().filter(|p| !bundled.contains(&p.asset.as_str()) && (lib.is_some_and(|l| l.get(&p.asset).is_none()) || !m.usable(&p.asset))).count();
            json!({ "id": s.id, "name": s.name, "description": s.description, "pieces": s.pieces.len(), "size_m": [sz.x.round(), sz.y.round()], "assets": assets, "pieces_unavailable_here": blocked })
        })
        .collect();
    // the asset packs' complete scenes in one model (Objects › Scenes › Ready-made scenes)
    let ready: Vec<Value> = lib
        .map(|l| l.entries.iter().filter(|e| e.whole_scene && m.usable(&e.id)).map(|e| json!({ "id": e.id, "name": e.name, "pack": e.pack, "radius_m": e.radius, "height_m": e.height })).collect())
        .unwrap_or_default();
    Ok(vec![McpContent::Json(
        json!({ "scenes": out, "ready_made": ready }),
    )])
}

fn save(
    In(args): In<Value>,
    d: WorldData,
    mut scenes: ResMut<super::Scenes>,
) -> Result<Vec<McpContent>, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("give the scene a `name`".into());
    }
    let description = args
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut outline = None;
    let ids: Vec<u64> =
        if let Some(poly) = crate::mcp_tools::opt_arg::<Vec<[f32; 2]>>(&args, "polygon")? {
            let poly: Vec<Vec2> = poly.into_iter().map(Vec2::from_array).collect();
            let ids = file::inside(&d.store, &poly);
            outline = Some(poly);
            ids
        } else if let Some(g) = args.get("group").and_then(|v| v.as_u64()) {
            d.store.group_members(g)
        } else {
            crate::mcp_tools::opt_arg::<Vec<u64>>(&args, "ids")?
                .ok_or("give a `polygon`, `ids` or a `group`")?
        };
    if ids.is_empty() {
        return Err("no placed objects there".into());
    }
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading; try again in a moment")?;
    let scene = file::capture(
        &d.store,
        &d.terrain,
        &ids,
        outline.as_deref(),
        &name,
        description,
    )?;
    let (n, size) = (scene.pieces.len(), scene.size());
    let id = file::save_new(scene, lib)?;
    // the Scenes tab reads the library again
    scenes.loaded = false;
    Ok(vec![McpContent::Json(
        json!({ "scene": id, "pieces": n, "size_m": [size.x.round(), size.y.round()], "note": "in the user's Objects › Scenes" }),
    )])
}

fn place(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id = args
        .get("scene")
        .and_then(|v| v.as_str())
        .ok_or("missing `scene`")?;
    let scene = file::read(id).ok_or_else(|| format!("no scene `{id}` (scenes_list)"))?;
    let pos: Vec<f32> =
        crate::mcp_tools::opt_arg(&args, "position")?.ok_or("missing `position`")?;
    let at = match pos[..] {
        [x, z] | [x, _, z] => Vec2::new(x, z),
        _ => return Err("`position` is [x, z]".into()),
    };
    let yaw = -(args.get("yaw_deg").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32).to_radians();
    let p = super::place(&mut d, &scene, at, yaw)?;
    let skipped: Vec<Value> = p
        .skipped
        .iter()
        .map(|(a, w)| json!({ "asset": a, "why": w }))
        .collect();
    Ok(vec![McpContent::Json(
        json!({ "scene": id, "group": p.group, "placed": p.ids.len(), "ids": p.ids, "left_out": skipped, "note": "one undo step; objects_group moves, turns, resizes or ungroups it" }),
    )])
}

fn export(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id = args
        .get("scene")
        .and_then(|v| v.as_str())
        .ok_or("missing `scene`")?;
    let path = std::path::PathBuf::from(
        args.get("path")
            .and_then(|v| v.as_str())
            .ok_or("missing `path`")?,
    );
    if !path.is_absolute() {
        return Err("`path` must be absolute".into());
    }
    let scene = file::read(id).ok_or_else(|| format!("no scene `{id}` (scenes_list)"))?;
    let text = match args.get("format").and_then(|v| v.as_str()) {
        Some("zip") => {
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
            {
                return Err("a scene file ends in .zip".into());
            }
            let n = file::export_zip(id, &path)?;
            format!("Wrote {} ({:.1} MB)", path.display(), n as f64 / 1e6)
        }
        Some("glb") => {
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("glb"))
            {
                return Err("a model ends in .glb".into());
            }
            let s = crate::export::scene_glb(world, &scene, &path)?;
            format!("Wrote {} ({s})", path.display())
        }
        _ => return Err("`format`: zip or glb".into()),
    };
    Ok(vec![McpContent::Text(text)])
}
