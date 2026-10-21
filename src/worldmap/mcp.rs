//! MCP tools: `map_generate`, `map_status`, `map_export`.

use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use serde_json::{Value, json};

use super::{
    MapLayers, MapResult, MapSettings, MapStyle, SIZES, WorldMap, export, generate_now, start,
};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "map_generate",
        description: "Generate the world map (Map tab) from the current world: terrain, water, forests, zones, roads, POIs, \
            lettering, compass, scale and title. Styles: parchment (inked fantasy map), satellite (top-down render of the 3D \
            world; renders over several seconds: poll map_status), atlas (topographic), minimal (flat in-game minimap colours). \
            Returns a downscaled PNG preview, the path of the full-size PNG and the world↔pixel transform. Omitted options \
            keep the Map tab's settings. With style satellite and `rect` and/or `width`/`height`: a bare orthographic render \
            (world only: trees drawn, no clouds, fog, lettering or editor overlays) of exactly that world rectangle at that \
            pixel size, for pixel-exact comparison with reference images; poll map_status for the image and its transform.",
        input_schema: generate_schema,
        run: generate,
    })
    .mcp_tool(McpTool {
        name: "map_status",
        description: "Progress of a running map generation, or the finished map (preview PNG + saved path).",
        input_schema: || {
            json!({"type": "object", "properties": {
                "preview_px": {"type": "integer", "description": "preview size (default 768)"},
                "out": {"type": "string", "description": "folder for the full-size PNG (default: where map_generate saved it)"}
            }})
        },
        run: status,
    })
    .mcp_tool(McpTool {
        name: "map_export",
        description: "Export the current map for a game into a folder: full and clean PNGs, preview, fog-of-war mask, 256 px \
            minimap tile pyramid (tiles/{z}/{x}_{y}.png) and map.json (world↔map transform, label anchors, POI icon positions, \
            zone polygons, roads). Generates the map first if there is none or the world changed (not for satellite).",
        input_schema: || json!({"type": "object", "properties": {"dir": {"type": "string", "description": "output folder"}}, "required": ["dir"]}),
        run: export_tool,
    });
}

fn generate_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "style": {"type": "string", "enum": ["parchment", "satellite", "atlas", "minimal"]},
            "size": {"type": "integer", "enum": SIZES, "description": "output side in px"},
            "layers": {
                "type": "object",
                "properties": {
                    "labels": {"type": "boolean"}, "zones": {"type": "boolean"}, "roads": {"type": "boolean"},
                    "pois": {"type": "boolean"}, "grid": {"type": "boolean"}, "compass": {"type": "boolean", "description": "compass and scale bar"}
                }
            },
            "rect": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4, "description": "satellite capture: world rectangle [min_x, min_z, max_x, max_z] in metres (default: the whole world)"},
            "width": {"type": "integer", "description": "satellite capture width in px (16-8192; default from height and the rect's aspect, or 2048)"},
            "height": {"type": "integer", "description": "satellite capture height in px (16-8192; default from width and the rect's aspect)"},
            "title": {"type": "string", "description": "map title (empty: the world's name)"},
            "show_title": {"type": "boolean"},
            "sea_name": {"type": "string", "description": "name lettered across the open sea"},
            "out": {"type": "string", "description": "folder for the full-size PNG (default: a temp folder)"},
            "preview_px": {"type": "integer", "description": "preview size (default 768)"}
        }
    })
}

fn settings_from(args: &Value, base: &MapSettings) -> Result<MapSettings, String> {
    let mut s = base.clone();
    if let Some(st) = opt_arg::<String>(args, "style")? {
        s.style = MapStyle::from_id(&st).ok_or_else(|| {
            format!("unknown style `{st}` (parchment, satellite, atlas, minimal)")
        })?;
    }
    if let Some(sz) = opt_arg::<u32>(args, "size")? {
        if !(512..=8192).contains(&sz) || !sz.is_power_of_two() {
            return Err("size must be 2048, 4096 or 8192".into());
        }
        s.size = sz;
    }
    if let Some(l) = args.get("layers").filter(|v| v.is_object()) {
        let mut v = serde_json::to_value(&s.layers).map_err(|e| e.to_string())?;
        for (k, val) in l.as_object().into_iter().flatten() {
            v[k] = val.clone();
        }
        s.layers = serde_json::from_value::<MapLayers>(v).map_err(|e| format!("layers: {e}"))?;
    }
    if let Some(t) = opt_arg::<String>(args, "title")? {
        s.title = t;
    }
    if let Some(t) = opt_arg::<bool>(args, "show_title")? {
        s.show_title = t;
    }
    if let Some(t) = opt_arg::<String>(args, "sea_name")? {
        s.sea_name = t;
    }
    Ok(s)
}

fn out_dir(args: &Value) -> Result<PathBuf, String> {
    Ok(opt_arg::<String>(args, "out")?
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("world_builder_map")))
}

/// Saves the full-size PNG (once per map and folder) and returns it with a preview.
fn finished(
    world: &mut World,
    r: &Arc<MapResult>,
    dir: &std::path::Path,
    preview: u32,
) -> Result<Vec<McpContent>, String> {
    let path = dir.join(format!("map_{}.png", r.settings.style.id()));
    let key = (Arc::as_ptr(r) as usize, path.clone());
    let mut map = world.resource_mut::<WorldMap>();
    if map.mcp_saved.as_ref() != Some(&key) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        export::write_png(
            &path,
            &export::rgb(&r.rgba),
            r.size,
            r.size,
            image::ExtendedColorType::Rgb8,
        )?;
        map.mcp_saved = Some(key);
    }
    let labels = r.meta["labels"].as_array().map(|a| a.len()).unwrap_or(0);
    Ok(vec![
        McpContent::Json(json!({
            "path": path,
            "style": r.settings.style.id(),
            "size_px": r.size,
            "millis": r.millis,
            "labels": labels,
            "pois": r.pois.len(),
            "transform": r.meta["transform"],
        })),
        McpContent::Png(export::preview_png(r, preview.clamp(128, 2048))?),
    ])
}

fn generate(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    if world.resource::<WorldMap>().busy() {
        return Err("a map is being generated: poll map_status".into());
    }
    let s = settings_from(&args, &world.resource::<WorldMap>().settings)?;
    let rect = opt_arg::<[f32; 4]>(&args, "rect")?;
    let (w, h) = (
        opt_arg::<u32>(&args, "width")?,
        opt_arg::<u32>(&args, "height")?,
    );
    if s.style == MapStyle::Satellite && (rect.is_some() || w.is_some() || h.is_some()) {
        let half = world.resource::<crate::terrain::Terrain>().half();
        let rect = rect.unwrap_or([-half, -half, half, half]);
        let aspect = (rect[3] - rect[1]) / (rect[2] - rect[0]).max(1e-3);
        let (w, h) = match (w, h) {
            (Some(w), Some(h)) => (w, h),
            (Some(w), None) => (w, ((w as f32 * aspect).round() as u32).max(1)),
            (None, Some(h)) => (((h as f32 / aspect).round() as u32).max(1), h),
            (None, None) if aspect <= 1.0 => (2048, ((2048.0 * aspect).round() as u32).max(1)),
            (None, None) => (((2048.0 / aspect).round() as u32).max(1), 2048),
        };
        let shot = super::satellite::Shot {
            rect,
            width: w,
            height: h,
            path: out_dir(&args)?.join("satellite_capture.png"),
        };
        let transform = shot.transform();
        let path = shot.path.clone();
        super::start_shot(world, shot)?;
        return Ok(vec![McpContent::Json(json!({
            "status": "rendering",
            "path": path,
            "width": w,
            "height": h,
            "transform": transform,
            "hint": "the tiles render over the next frames; poll map_status for the image",
        }))]);
    }
    world.resource_mut::<WorldMap>().settings = s.clone();
    let preview = opt_arg::<u32>(&args, "preview_px")?.unwrap_or(768);
    if s.style == MapStyle::Satellite {
        start(world);
        let map = world.resource::<WorldMap>();
        if let Some(msg) = map.status.clone().filter(|_| !map.busy()) {
            return Err(msg);
        }
        return Ok(vec![McpContent::Json(
            json!({"status": "rendering", "hint": "the satellite tiles render over the next frames; poll map_status"}),
        )]);
    }
    let r = generate_now(world, &s)?;
    finished(world, &r, &out_dir(&args)?, preview)
}

fn status(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let map = world.resource::<WorldMap>();
    if let Some((f, stage)) = map.progress() {
        return Ok(vec![McpContent::Json(
            json!({"status": "running", "progress": f, "stage": stage}),
        )]);
    }
    if map.last_was_shot {
        if map.busy() {
            return Ok(vec![McpContent::Json(
                json!({"status": "running", "progress": 1.0, "stage": "Saving"}),
            )]);
        }
        let Some(r) = &map.shot else {
            return Ok(vec![McpContent::Json(
                json!({"status": "none", "message": map.status}),
            )]);
        };
        return Ok(vec![
            McpContent::Json(json!({
                "status": "done",
                "path": r.shot.path,
                "width": r.shot.width,
                "height": r.shot.height,
                "millis": r.millis,
                "transform": r.shot.transform(),
            })),
            McpContent::Png(r.preview.clone()),
        ]);
    }
    let Some(r) = map.result.clone() else {
        return Ok(vec![McpContent::Json(
            json!({"status": "none", "message": map.status}),
        )]);
    };
    let preview = opt_arg::<u32>(&args, "preview_px")?.unwrap_or(768);
    // the folder map_generate used, unless another is asked for
    let dir = match opt_arg::<String>(&args, "out")? {
        Some(d) => PathBuf::from(d),
        None => world
            .resource::<WorldMap>()
            .mcp_saved
            .as_ref()
            .and_then(|(_, p)| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or(out_dir(&args)?),
    };
    let mut out = finished(world, &r, &dir, preview)?;
    if let Some(McpContent::Json(v)) = out.first_mut() {
        v["status"] = json!("done");
        v["stale"] = json!(world.resource::<WorldMap>().stale);
    }
    Ok(out)
}

fn export_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let dir: String = crate::mcp_tools::arg(&args, "dir")?;
    let dir = PathBuf::from(dir);
    let map = world.resource::<WorldMap>();
    if map.busy() {
        return Err("a map is being generated: poll map_status, then export".into());
    }
    let r = match (&map.result, map.stale) {
        (Some(r), false) => r.clone(),
        _ => {
            let s = map.settings.clone();
            if s.style == MapStyle::Satellite {
                return Err(
                    "generate the satellite map first (map_generate, then map_status until done)"
                        .into(),
                );
            }
            generate_now(world, &s)?
        }
    };
    let e = export::export(&dir, &r)?;
    world.resource_mut::<WorldMap>().last_export = Some(dir.clone());
    Ok(vec![McpContent::Json(
        json!({"dir": dir, "files": e.files, "bytes": e.bytes, "map": e.main, "style": r.settings.style.id(), "size_px": r.size}),
    )])
}
