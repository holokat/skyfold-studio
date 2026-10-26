//! World tools: overview, new / generate, save / load / list / export, undo / redo, the editor
//! log and view.

use std::path::PathBuf;

use bevy::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::biomegen::Climate;
use crate::biomes::BiomePreset;
use crate::editor::{Editor, Tab};
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::objects::Category;
use crate::terrain::worldgen::WorldPreset;
use crate::ui::UiAction;

const SIZES: [(&str, usize); 3] = [("small", 512), ("medium", 1024), ("large", 2048)];

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "world_info",
        description: "Overview of the open world: name, file, unsaved changes, art style (photoreal or handpainted), size and coordinate system, sea level, height range and highest point, land share, biomes (coverage_pct: share of the land above sea level, so sea painted with a biome doesn't count), zones, \
object counts by category, water bodies, roads and gameplay counts, nature amounts, time of day, look, weather, effective haze at the current viewer, wind (bearing it blows towards), camera (yaw as the bearing it looks along) and the undo / redo steps. Call this first.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, info, a),
    });
    app.mcp_tool(McpTool {
        name: "world_new",
        description: "Start a new world (replaces the open one; it can't be undone). Presets: empty ocean, island, continent, archipelago, flat plains; sizes small 2 km, medium 4 km, large 8 km (4 m grid). \
With a `climate` the land is filled with climate-driven biomes, ground, forests, plants and water styles (\"none\" for bare terrain). `kind` scifi puts the whole Sci-Fi template on the new world once it is made (alien canyonlands, dusk grade, a ringed giant in the sky, crimson flora, spires, floating rocks, monoliths, crystals; see world_kind_apply); fantasy (default) is today's world. `forest_diversity`, `forest_layout` and `grass_lushness` are staged only for this new world. No zones are created. Refuses if the open world has unsaved changes unless `discard_unsaved` is true.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "preset": enum_schema(ids(&WorldPreset::ALL), "starting landmass"),
                    "kind": { "type": "string", "enum": crate::world_kind::WorldKind::ALL.map(|k| k.id()), "default": "fantasy", "description": "fantasy or scifi world" },
                    "size": { "type": "string", "enum": ["small", "medium", "large"], "default": "medium" },
                    "seed": { "type": "integer", "minimum": 0, "maximum": 999999, "description": "default: random" },
                    "climate": enum_schema(ids(&Climate::ALL).into_iter().chain(["none".to_string()]).collect(), "biomes and nature (default varied)"),
                    "forest_diversity": { "type": "boolean", "default": false, "description": "optional rich automatic tree species; false preserves the legacy selection" },
                    "forest_layout": { "type": "string", "enum": ["natural_groves", "mixed"], "default": "natural_groves" },
                    "grass_lushness": { "type": "number", "minimum": 0, "maximum": 1, "default": 0, "description": "global grass lushness; 0 preserves the existing look" },
                    "discard_unsaved": { "type": "boolean", "default": false }
                },
                "required": ["preset"]
            })
        },
        run: |w, a| run(w, new_world, a),
    });
    app.mcp_tool(McpTool {
        name: "world_generate",
        description: "Generate fresh terrain for the open world (keeps its size; undoable). Old forests, rivers and lakes are cleared; structures stay and follow the new ground. \
With a `climate` (default varied) biomes and nature are generated too; \"none\" keeps bare terrain (any previous nature is regrown to fit).",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "preset": enum_schema(ids(&WorldPreset::ALL), "landmass"),
                    "seed": { "type": "integer", "minimum": 0, "maximum": 999999, "description": "default: random" },
                    "climate": enum_schema(ids(&Climate::ALL).into_iter().chain(["none".to_string()]).collect(), "biomes and nature (default varied)")
                },
                "required": ["preset"]
            })
        },
        run: |w, a| run(w, generate, a),
    });
    app.mcp_tool(McpTool {
        name: "world_save",
        description: "Save the world. Without `path` it saves in place (or, for a new world, as worlds/<name>.world in the project). `path` may be a name (saved under worlds/) or an absolute path to a .world folder.",
        input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string", "description": "name or absolute .world folder path" } } }),
        run: |w, a| run(w, save, a),
    });
    app.mcp_tool(McpTool {
        name: "world_load",
        description: "Open a saved world (a name from world_list or an absolute .world folder path). Clears the undo history. Refuses if the open world has unsaved changes unless `discard_unsaved` is true.",
        input_schema: || json!({ "type": "object", "properties": { "path": { "type": "string" }, "discard_unsaved": { "type": "boolean", "default": false } }, "required": ["path"] }),
        run: |w, a| run(w, load, a),
    });
    app.mcp_tool(McpTool {
        name: "world_list",
        description: "Saved worlds in the project's worlds folder (name, path, last saved).",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |_, _| list(),
    });
    app.mcp_tool(McpTool {
        name: "world_export",
        description: "Export the world for a game engine (engine-agnostic package): 16-bit heightmap (png, r16, f32), material weight maps, footstep-surface map, zone and biome maps, and world.json with zones, water, lighting, wind and every object. \
Default folder: <project>/exports/<world name>; `dir` must be an absolute path.",
        input_schema: || json!({ "type": "object", "properties": { "dir": { "type": "string", "description": "absolute output folder" } } }),
        run: |w, a| run(w, export, a),
    });
    app.mcp_tool(McpTool {
        name: "undo",
        description: "Undo the last edit(s), like ⌘Z: brush strokes, placed or removed objects, water, zones, biomes, generation… (lighting, look, weather, camera and sea level are settings, not undo steps).",
        input_schema: || json!({ "type": "object", "properties": { "steps": { "type": "integer", "minimum": 1, "maximum": 60, "default": 1 } } }),
        run: |w, a| run(w, undo, a),
    });
    app.mcp_tool(McpTool {
        name: "redo",
        description: "Redo what undo took back, like ⇧⌘Z.",
        input_schema: || json!({ "type": "object", "properties": { "steps": { "type": "integer", "minimum": 1, "maximum": 60, "default": 1 } } }),
        run: |w, a| run(w, redo, a),
    });
    app.mcp_tool(McpTool {
        name: "get_log",
        description: "Recent editor messages (the status toasts a person would see: what an action did, warnings) and recent MCP calls with their results.",
        input_schema: || json!({ "type": "object", "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 60, "default": 15 } } }),
        run: |w, a| run(w, log, a),
    });
    app.mcp_tool(McpTool {
        name: "editor_view",
        description: "Switch the editor's tab and overlays (what the person sees and what screenshots show): e.g. tab \"zones\" with zone colours and labels, \"audio\" for the footstep-surface overlay.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "tab": enum_schema(ids(&Tab::ALL), "editor tab"),
                    "zone_colors": { "type": "boolean" },
                    "zone_boundaries": { "type": "boolean" },
                    "zone_labels": { "type": "boolean" },
                    "level_ranges": { "type": "boolean" },
                    "surface_overlay": { "type": "boolean", "description": "footstep surfaces in colour" }
                }
            })
        },
        run: |w, a| run(w, view, a),
    });
    app.mcp_tool(McpTool {
        name: "settings_search",
        description: "Find where a setting, tool or section lives in the editor (the ⌘K search): every labelled control in every tab's panel, inspector and dock, with its path, e.g. thunder → Atmosphere › Weather › Rain. \
settings_open takes the person there.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "words in the setting's name or path" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
                },
                "required": ["query"]
            })
        },
        run: |_, a| settings_search(a),
    });
    app.mcp_tool(McpTool {
        name: "settings_open",
        description: "Take the person to a setting, as clicking it in the ⌘K search does: switches to its tab and tool, opens its sections, scrolls to it and rings it for a moment. \
`query` opens the best settings_search match, or pass a result's `index`. With `palette` true it opens the ⌘K search with the query typed instead.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "index": { "type": "integer", "minimum": 0 },
                    "palette": { "type": "boolean", "default": false }
                }
            })
        },
        run: |_, a| settings_open(a),
    });
    app.mcp_tool(McpTool {
        name: "app_update_check",
        description: "Check for a newer Skyfold Studio, like File › Check for Updates…: Sparkle shows the person what it finds (release notes, Install Update, then Install and Relaunch, which saves the world first). \
Only the packaged app updates itself; a development build says so.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |_, _| {
            if crate::updates::check_now() {
                text("Checking for updates: Sparkle's window shows the result.")
            } else {
                text("This build doesn't update itself (only the packaged app from skyfoldstudio.com does).")
            }
        },
    });
}

fn settings_search(args: Value) -> R {
    use crate::ui::search::{SETTINGS, find};
    let q = opt_str(&args, "query")?.ok_or("missing `query`")?;
    let limit = opt_num(&args, "limit")?.unwrap_or(10.0).clamp(1.0, 50.0) as usize;
    let hits = find(q);
    let list: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|&i| {
            let st = &SETTINGS[i];
            json!({ "index": i, "label": st.label, "kind": st.kind, "path": st.path() })
        })
        .collect();
    Ok(vec![McpContent::Json(
        json!({ "matches": hits.len(), "results": list }),
    )])
}

fn settings_open(args: Value) -> R {
    use crate::ui::search::{Request, SETTINGS, find, request};
    let q = opt_str(&args, "query")?;
    if opt_bool(&args, "palette")?.unwrap_or(false) {
        request(Request::Palette(q.unwrap_or("").to_string()));
        return text("Opened the ⌘K search.");
    }
    let i = match (opt_num(&args, "index")?, q) {
        (Some(i), _) => i as usize,
        (None, Some(q)) => *find(q)
            .first()
            .ok_or_else(|| format!("No setting matches “{q}”."))?,
        (None, None) => return Err("pass `query` or `index`".into()),
    };
    let st = SETTINGS.get(i).ok_or("no setting with that index")?;
    if !st.visible() {
        return Err(format!("{} isn't available here ({}).", st.label, st.path()));
    }
    request(Request::Open(i));
    text(format!("Opened {} › {}.", st.path(), st.label))
}

/// Sends a UI action (handled by the same systems as the button) and reports when it's done:
/// the toast it showed, else `done`.
pub(super) fn ui_action(world: &mut World, action: UiAction, done: fn(&mut World) -> String) -> R {
    world.write_message(action);
    Ok(vec![McpContent::deferred(move |w| {
        let fresh = w
            .resource::<Editor>()
            .toast
            .as_ref()
            .filter(|(_, age)| *age < 0.5)
            .map(|(m, _)| m.clone());
        Some(Ok(vec![McpContent::Text(fresh.unwrap_or_else(|| done(w)))]))
    })])
}

fn climate_arg(args: &Value) -> Result<Option<Climate>, String> {
    match opt_str(args, "climate")? {
        None => Ok(Some(Climate::Varied)),
        Some(s) if s.eq_ignore_ascii_case("none") => Ok(None),
        Some(s) => pick(&Climate::ALL, s, "climate", Climate::name).map(Some),
    }
}

fn seed_arg(args: &Value) -> Result<u32, String> {
    Ok(match opt_num(args, "seed")? {
        Some(s) => s.max(0.0) as u32 % 1_000_000,
        None => fastrand::u32(0..999_999),
    })
}

fn summary(w: &mut World) -> String {
    let t = w.resource::<crate::terrain::Terrain>();
    let (size, sea) = (t.size(), t.sea_level);
    let land = t.heights.iter().filter(|h| **h > sea).count() as f32 / t.heights.len() as f32;
    let objects = w.resource::<crate::objects::ObjectStore>().count();
    format!(
        "World is {:.1} × {:.1} km, {:.0}% land, {objects} objects.",
        size / 1000.0,
        size / 1000.0,
        land * 100.0
    )
}

fn new_world(
    In(args): In<Value>,
    d: WorldData,
    mut actions: MessageWriter<UiAction>,
    mut template: ResMut<crate::world_kind::TemplateRequest>,
) -> R {
    if d.meta.dirty && !opt_bool(&args, "discard_unsaved")?.unwrap_or(false) {
        return Err("The open world has unsaved changes: save it (world_save) first, or pass discard_unsaved: true".into());
    }
    let preset = pick(
        &WorldPreset::ALL,
        opt_str(&args, "preset")?.ok_or("missing `preset`")?,
        "preset",
        WorldPreset::name,
    )?;
    let size = opt_str(&args, "size")?.unwrap_or("medium");
    let res = SIZES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(size))
        .map(|s| s.1)
        .ok_or("`size` must be small, medium or large")?;
    let seed = seed_arg(&args)?;
    let biomes = climate_arg(&args)?;
    let mut appearance = crate::nature::NatureAppearance::default();
    if let Some(enabled) = opt_bool(&args, "forest_diversity")? {
        appearance.forest.enabled = enabled;
    }
    if let Some(layout) = opt_str(&args, "forest_layout")? {
        appearance.forest.layout = match layout {
            "natural_groves" => crate::nature::ForestLayout::NaturalGroves,
            "mixed" => crate::nature::ForestLayout::Mixed,
            _ => return Err("`forest_layout` must be natural_groves or mixed".into()),
        };
    }
    if let Some(lushness) = opt_num(&args, "grass_lushness")? {
        appearance.grass_lushness = lushness.clamp(0.0, 1.0);
    }
    let kind = match opt_str(&args, "kind")? {
        None => crate::world_kind::WorldKind::Fantasy,
        Some(k) => crate::world_kind::WorldKind::from_id(k).ok_or("`kind` must be fantasy or scifi")?,
    };
    if kind != crate::world_kind::WorldKind::Fantasy {
        template.after_new_world(kind, d.terrain.generation, seed);
    }
    actions.write(UiAction::NewWorld {
        preset,
        res,
        cell: 4.0,
        seed,
        biomes,
        art_style: crate::art_style::ArtStyle::Realistic,
        appearance,
    });
    Ok(vec![McpContent::deferred(move |w| {
        Some(Ok(vec![McpContent::Text(format!(
            "{} (seed {seed}). {}",
            w.resource::<Editor>()
                .toast
                .as_ref()
                .map(|t| t.0.clone())
                .unwrap_or_default(),
            summary(w)
        ))]))
    })])
}

fn generate(In(args): In<Value>, mut actions: MessageWriter<UiAction>) -> R {
    let preset = pick(
        &WorldPreset::ALL,
        opt_str(&args, "preset")?.ok_or("missing `preset`")?,
        "preset",
        WorldPreset::name,
    )?;
    let seed = seed_arg(&args)?;
    let biomes = climate_arg(&args)?;
    actions.write(UiAction::Regenerate {
        preset,
        seed,
        biomes,
    });
    Ok(vec![McpContent::deferred(move |w| {
        Some(Ok(vec![McpContent::Text(format!(
            "{} (seed {seed}). {}",
            w.resource::<Editor>()
                .toast
                .as_ref()
                .map(|t| t.0.clone())
                .unwrap_or_default(),
            summary(w)
        ))]))
    })])
}

/// A world folder from a name (under worlds/) or an absolute path.
fn world_path(s: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(s);
    let p = if p.is_absolute() {
        p
    } else {
        if s.contains('/') || s.contains("..") {
            return Err("give a world name (saved under worlds/) or an absolute path".into());
        }
        crate::io::worlds_dir().join(crate::io::sanitize(s.trim_end_matches(".world")))
    };
    Ok(if p.extension().is_some_and(|e| e == "world") {
        p
    } else {
        p.with_extension("world")
    })
}

fn save(In(args): In<Value>, mut d: WorldData) -> R {
    let path = match opt_str(&args, "path")? {
        Some(s) => world_path(s)?,
        None => match d
            .meta
            .path
            .clone()
            .filter(|p| *p != crate::io::recovery_path())
        {
            Some(p) => p,
            None => world_path(&crate::io::sanitize(&d.meta.name))?,
        },
    };
    crate::io::save_and_report(&path, &mut d);
    let msg = d.ed.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default();
    if msg.starts_with("Save failed") {
        Err(msg)
    } else {
        text(msg)
    }
}

fn load(In(args): In<Value>, mut d: WorldData) -> R {
    if d.meta.dirty && !opt_bool(&args, "discard_unsaved")?.unwrap_or(false) {
        return Err("The open world has unsaved changes: save it (world_save) first, or pass discard_unsaved: true".into());
    }
    let path = world_path(opt_str(&args, "path")?.ok_or("missing `path`")?)?;
    if !path.join("world.json").exists() {
        return Err(format!("No world at {} (see world_list)", path.display()));
    }
    crate::io::load_world(&path, &mut d).map_err(|e| format!("Couldn't open world: {e}"))?;
    if path == crate::io::recovery_path() {
        // restored work still needs a real home
        d.meta.path = None;
        d.meta.dirty = true;
    }
    d.ed.toast(format!("Opened {}", d.meta.name));
    text(format!(
        "Opened {} from {}. {} objects, {} zones.",
        d.meta.name,
        path.display(),
        d.store.count(),
        d.zones.list.len()
    ))
}

fn list() -> R {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(crate::io::worlds_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "world") && p.join("world.json").exists() {
                let mins = std::fs::metadata(p.join("world.json"))
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|m| m.elapsed().ok())
                    .map(|d| d.as_secs() / 60);
                out.push(json!({
                    "name": p.file_stem().map(|s| s.to_string_lossy().replace('_', " ")),
                    "path": p.to_string_lossy(),
                    "saved_minutes_ago": mins,
                }));
            }
        }
    }
    json_out(json!({ "worlds": out }))
}

fn export(In(args): In<Value>, mut d: WorldData) -> R {
    let dir = match opt_str(&args, "dir")? {
        Some(s) => {
            let p = PathBuf::from(s);
            if !p.is_absolute() {
                return Err("`dir` must be an absolute path".into());
            }
            p
        }
        None => crate::export::exports_dir().join(crate::io::sanitize(&d.meta.name)),
    };
    crate::io::export_world(&dir, &d).map_err(|e| format!("Export failed: {e}"))?;
    d.ed.toast(format!("Exported to {}", dir.display()));
    text(format!(
        "Exported to {} (heightmap.png/.r16/.f32, materials_*.png, footstep_surfaces.png, zones.png, biomes.png, world.json).",
        dir.display()
    ))
}

fn step(redo: bool, args: &Value, d: &mut WorldData) -> R {
    let n = num(args, "steps", 1.0)?.round().clamp(1.0, 60.0) as usize;
    let mut labels = vec![];
    for _ in 0..n {
        let WorldData {
            history,
            terrain,
            store,
            water,
            zones,
            gameplay,
            roads,
            ..
        } = d;
        match history.step(redo, terrain, store, water, zones, gameplay, roads) {
            Some(l) => labels.push(l),
            None => break,
        }
    }
    let verb = if redo { "Redo" } else { "Undo" };
    if labels.is_empty() {
        return Err(format!("Nothing to {}", verb.to_lowercase()));
    }
    d.meta.dirty = true;
    d.ed.toast(format!("{verb}: {}", labels.join(", ")));
    let next = if redo {
        d.history.redo_label()
    } else {
        d.history.undo_label()
    };
    text(format!(
        "{verb}: {}. Next {}: {}",
        labels.join(", "),
        verb.to_lowercase(),
        next.unwrap_or("nothing")
    ))
}

fn undo(In(args): In<Value>, mut d: WorldData) -> R {
    step(false, &args, &mut d)
}

fn redo(In(args): In<Value>, mut d: WorldData) -> R {
    step(true, &args, &mut d)
}

fn log(
    In(args): In<Value>,
    log: Res<crate::mcp::McpLog>,
    server: Res<crate::mcp::McpServer>,
    time: Res<Time<Real>>,
) -> R {
    let n = num(&args, "limit", 15.0)?.round().clamp(1.0, 60.0) as usize;
    let now = time.elapsed_secs();
    let toasts: Vec<Value> = log
        .toasts
        .iter()
        .take(n)
        .map(|(t, m)| json!({ "seconds_ago": (now - t).round(), "message": m }))
        .collect();
    let calls: Vec<Value> = server
        .recent
        .iter()
        .take(n)
        .map(|c| json!({ "seconds_ago": (now - c.at).round(), "tool": c.name, "ok": c.ok, "ms": c.ms.round(), "error": c.error }))
        .collect();
    json_out(json!({ "messages": toasts, "mcp_calls": calls }))
}

fn view(In(args): In<Value>, mut ed: ResMut<Editor>) -> R {
    if let Some(t) = opt_pick(&args, "tab", &Tab::ALL, "tab", Tab::name)? {
        ed.tab = t;
    }
    let set = |key: &str, field: &mut bool| -> Result<(), String> {
        if let Some(v) = opt_bool(&args, key)? {
            *field = v;
        }
        Ok(())
    };
    set("zone_colors", &mut ed.show_zone_fill)?;
    set("zone_boundaries", &mut ed.show_zone_boundaries)?;
    set("zone_labels", &mut ed.show_zone_labels)?;
    set("level_ranges", &mut ed.show_level_ranges)?;
    set("surface_overlay", &mut ed.surface_overlay)?;
    text(format!(
        "Tab: {}. Zone colours {}, boundaries {}, labels {}, surface overlay {}.",
        ed.tab.name(),
        on(ed.show_zone_fill),
        on(ed.show_zone_boundaries),
        on(ed.show_zone_labels),
        on(ed.surface_overlay)
    ))
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

fn info(
    In(_): In<Value>,
    d: WorldData,
    stats: Res<crate::zones::ZoneStats>,
    rs: Res<crate::render_scale::RenderScale>,
    look: Res<crate::look::LookState>,
) -> R {
    let t = &*d.terrain;
    let (mut lo, mut hi, mut hi_at, mut land) = (f32::MAX, f32::MIN, 0usize, 0usize);
    for (k, h) in t.heights.iter().enumerate() {
        lo = lo.min(*h);
        if *h > hi {
            hi = *h;
            hi_at = k;
        }
        land += (*h > t.sea_level) as usize;
    }
    let n = t.n();
    let peak = t.grid_to_world(hi_at % n, hi_at / n);
    let effective_haze = json!({
        "world_enabled": d.lighting.haze_enabled,
        "effective_strength": look.haze.strength,
        "effective_amount": look.haze.effective_amount(),
        "source_zone": look.haze.source_zone,
    });
    // biome coverage counts land only (sea painted with a biome is still sea)
    let mut biome_count = vec![0usize; 256];
    for (b, h) in t.biomes.iter().zip(&t.heights) {
        if *h > t.sea_level {
            biome_count[*b as usize] += 1;
        }
    }
    let biomes: Vec<Value> = BiomePreset::ALL
        .iter()
        .filter(|b| biome_count[b.id() as usize] > 0)
        .map(|b| json!({ "biome": snake(&format!("{b:?}")), "name": b.def().name, "coverage_pct": super::terrain::round1(biome_count[b.id() as usize] as f32 * 100.0 / land.max(1) as f32) }))
        .collect();
    let zones: Vec<Value> = d
        .zones
        .list
        .iter()
        .map(|z| json!({ "id": z.id, "name": z.name, "levels": z.level_label(), "type": snake(&format!("{:?}", z.zone_type)), "area_km2": stats.area_km2.get(&z.id).map(|a| super::terrain::round1(*a * 10.0) / 10.0) }))
        .collect();
    let mut by_cat = serde_json::Map::new();
    if let Some(lib) = d.lib.as_deref() {
        let mut counts = std::collections::BTreeMap::new();
        for o in d.store.items.values() {
            let c = lib
                .get(&o.asset)
                .map(|e| e.category.name())
                .unwrap_or("Unknown");
            *counts.entry(c).or_insert(0usize) += 1;
        }
        for (k, v) in counts {
            by_cat.insert(k.to_lowercase(), json!(v));
        }
    }
    let l = &*d.lighting;
    let wind_deg = crate::mcp_tools::dir_bearing(d.wind.dir);
    let nature_total: usize = Category::NATURE
        .iter()
        .map(|c| {
            by_cat
                .get(&c.name().to_lowercase())
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize
        })
        .sum();
    let mut info = json!({
        "name": d.meta.name,
        "path": d.meta.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        "unsaved_changes": d.meta.dirty,
        "seed": d.meta.seed,
        "art_style": { "style": d.meta.art_style.id(), "name": d.meta.art_style.name(), "note": "photoreal or handpainted (world_set_art_style): the surfaces the engine paints with and the assets objects_library offers by default" },
        "size_m": t.size(),
        "grid": { "resolution": t.res, "cell_m": t.cell },
        "coordinates": format!("metres; x east, z south, y up; (0,0) is the centre; x and z run from {:.0} to {:.0}", -t.half(), t.half()),
        "sea_level": t.sea_level,
        "height": { "min": super::terrain::round1(lo), "max": super::terrain::round1(hi), "highest_point": [peak.x, peak.y] },
        "land_pct": super::terrain::round1(land as f32 * 100.0 / t.heights.len() as f32),
        "snow_line_above_sea": t.style.snow_line,
        "biomes": biomes,
        "zones": zones,
        "objects": { "total": d.store.count(), "nature": nature_total, "by_category": by_cat },
        "procedural_nature": {
            "note": "trees, rocks, plants and debris grow from the biome rules and painted density (nature_query lists them; nature_rules shows the rules)",
            "populated": d.nature.fallback,
            "painted_chunks_512m": d.nature.painted_chunks(),
            "removed_by_hand": d.nature.excluded.len(),
        },
        "nature_appearance": {
            "forest": {
                "enabled": d.appearance.sanitized().forest.enabled,
                "layout": match d.appearance.sanitized().forest.layout {
                    crate::nature::ForestLayout::NaturalGroves => "natural_groves",
                    crate::nature::ForestLayout::Mixed => "mixed",
                },
            },
            "grass_lushness": d.appearance.sanitized().grass_lushness,
        },
        "water": {
            "ocean_preset": d.water_style.preset_name(), "waves": d.water_style.waves,
            "rivers": d.water.rivers.len(), "lakes": d.water.lakes.len(), "waterfalls": d.water.waterfalls.len(), "fish_schools": d.water.fish.len()
        },
        "roads": d.roads.roads.len(),
        "gameplay": { "player_start": d.gameplay.player_start.is_some(), "pois": d.gameplay.pois.len(), "npcs": d.gameplay.npcs.len(), "spawn_regions": d.gameplay.spawns.len(), "resource_nodes": d.gameplay.nodes.len() },
        "nature_amounts": { "grass": d.amounts.grass, "trees": d.amounts.trees, "rocks": d.amounts.rocks, "plants": d.amounts.plants, "flowers": d.amounts.flowers, "mushrooms": d.amounts.mushrooms, "berries": d.amounts.berries, "debris": d.amounts.debris },
        "time": { "hour": super::terrain::round1(l.hour), "day_cycle_playing": l.playing },
        "forest_contours": l.forest_contours,
        "look": { "preset": snake(&format!("{:?}", l.look.preset)), "strength": l.look.strength, "fine_tuned": l.look.custom.is_some() },
        "weather": {
            "override": l.weather_override.map(|w| snake(&format!("{w:?}"))), "rain_and_snow": l.precipitation,
            "cloud_cover": l.cloud_coverage, "haze": l.haze, "haze_enabled": l.haze_enabled,
            "wind_speed_mps": d.wind.speed, "wind_direction_deg": wind_deg.round(), "gustiness": d.wind.gustiness
        },
        "camera": { "focus": [d.orbit.focus.x.round(), d.orbit.focus.z.round()], "yaw_deg": crate::mcp_tools::view_bearing(d.orbit.yaw).round(), "pitch_deg": d.orbit.pitch.to_degrees().round(), "distance": d.orbit.distance.round() },
        "undo": d.history.undo_label(),
        "redo": d.history.redo_label(),
        "editor_tab": d.ed.tab.name(),
        "render_resolution": { "setting": rs.setting.id(), "scale_pct": (rs.active * 100.0).round(), "pixels": [rs.pixels.x, rs.pixels.y] },
    });
    // Keep the already-large overview macro shallow; this is runtime state rather than a saved
    // world setting, so it is assembled separately.
    info["haze"] = effective_haze;
    json_out(info)
}
