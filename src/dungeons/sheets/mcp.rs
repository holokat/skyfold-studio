//! The design-sheet tools: the library (`dungeons_sheets_library`, `dungeons_sheet_preview`), a
//! sheet realm's build (`dungeons_sheet_status`), its markers (`dungeons_sheet_markers`,
//! `dungeons_sheet_fill`, `dungeons_sheet_markers_show`), new sheets (`dungeons_sheet_generate`,
//! `dungeons_sheet_jobs`, `dungeons_sheet_import`, `dungeons_sheet_read_prompt`). A dungeon is made
//! from a sheet with `dungeons_create` `sheet`.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::SheetRun;
use super::blueprint::Kind;
use super::generate::{self, Request, SheetJobs};
use super::library::{self, SheetRef};
use super::markers;
use super::ui::SheetUi;
use crate::dungeons::realm_shapes::RealmSize;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, arg, opt_arg};

/// The sheet `id` names: a library sheet, or one of the user's own.
pub fn sheet_ref(d: &WorldData, id: &str) -> Result<SheetRef, String> {
    if let Some(e) = library::entry(id) {
        return Ok(SheetRef {
            id: id.into(),
            custom: false,
            family: e.family.clone(),
        });
    }
    let r = SheetRef {
        id: id.into(),
        custom: true,
        family: String::new(),
    };
    let world = d.meta.path.clone();
    match library::load(&r, world.as_deref()) {
        Ok((bp, _)) => Ok(SheetRef {
            family: bp.family,
            ..r
        }),
        Err(_) => Err(format!(
            "no sheet `{id}` (dungeons_sheets_library lists them)"
        )),
    }
}

pub fn sheet_name(d: &WorldData, r: &SheetRef) -> String {
    library::entry(&r.id)
        .map(|e| e.name.clone())
        .or_else(|| {
            library::load(r, d.meta.path.as_deref())
                .ok()
                .map(|(bp, _)| bp.name)
        })
        .unwrap_or_default()
}

/// The realm size a sheet needs (its map and the cliffs round it).
pub fn realm_size_for(d: &WorldData, r: &SheetRef) -> RealmSize {
    let size = library::entry(&r.id)
        .map(|e| e.size)
        .or_else(|| {
            library::load(r, d.meta.path.as_deref())
                .ok()
                .map(|(bp, _)| bp.size)
        })
        .unwrap_or([300.0, 210.0]);
    if size[0].max(size[1]) > 420.0 {
        RealmSize::Small
    } else {
        RealmSize::Cozy
    }
}

fn png_of(path: &std::path::Path, px: u32) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let img = if img.width() > px {
        img.resize(
            px,
            px * img.height() / img.width().max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let mut png = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &rgba,
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(png)
}

fn status_json(world: &World) -> Value {
    let r = world.resource::<SheetRun>();
    if r.running() {
        return json!({ "building": true, "step": r.done, "steps": r.total, "log": r.log.iter().rev().take(6).rev().collect::<Vec<_>>() });
    }
    let saved = || {
        let dir = super::open_sheet_dir(world)?;
        serde_json::from_str::<Value>(&std::fs::read_to_string(dir.join("build.json")).ok()?).ok()
    };
    match r.summary.clone().filter(|_| r.finished).or_else(saved) {
        Some(s) => json!({ "building": false, "summary": s }),
        None => {
            json!({ "building": false, "summary": Value::Null, "note": "the open map wasn't made from a sheet (dungeons_create with `sheet`, then dungeons_open)" })
        }
    }
}

fn markers_json(world: &World, zone: Option<u8>, letter: Option<&str>, suggest: bool) -> Value {
    let Some(doc) = world.resource::<crate::dungeons::DungeonMap>().0.as_ref() else {
        return json!({ "markers": [], "note": "open a sheet realm first" });
    };
    let list: Vec<Value> = doc
        .markers
        .iter()
        .filter(|m| zone.is_none_or(|z| m.zone == z) && letter.is_none_or(|l| m.letter.eq_ignore_ascii_case(l)))
        .map(|m| {
            let mut v = json!({ "id": m.id, "letter": m.letter, "name": m.name, "group": m.group, "size_m": m.size, "position": [m.at[0], m.at[1]], "yaw_deg": m.yaw, "zone": m.zone, "filled": m.filled });
            if suggest && m.filled.is_none() {
                v["suggestions"] = json!(markers::suggest(m, 3).into_iter().map(|(a, s)| json!({ "asset": a, "scale": s })).collect::<Vec<_>>());
            }
            v
        })
        .collect();
    let open = doc.markers.iter().filter(|m| m.filled.is_none()).count();
    json!({ "markers": list, "unfilled": open, "total": doc.markers.len() })
}

/// Fills markers with a model (or each one's best suggestion), placing ordinary objects.
pub fn fill_markers(
    world: &mut World,
    ids: Vec<u32>,
    asset: Option<String>,
    scale: Option<f32>,
) -> Result<Value, String> {
    let ms: Vec<markers::Marker> = {
        let doc = world
            .resource::<crate::dungeons::DungeonMap>()
            .0
            .as_ref()
            .ok_or("open a sheet realm first")?;
        ids.iter()
            .filter_map(|id| doc.markers.iter().find(|m| m.id == *id).cloned())
            .collect()
    };
    if ms.is_empty() {
        return Err("no such markers (dungeons_sheet_markers lists them)".into());
    }
    let mut items = vec![];
    let mut done = vec![];
    let mut none = vec![];
    for m in &ms {
        let pick = match &asset {
            Some(a) => Some((a.clone(), scale.unwrap_or(1.0))),
            None => markers::suggest(m, 1)
                .into_iter()
                .next()
                .map(|(a, s)| (a, scale.unwrap_or(s))),
        };
        match pick {
            Some((a, s)) => {
                items.push(markers::place_item(m, &a, s));
                done.push((m.id, a));
            }
            None => none.push(m.id),
        }
    }
    if !items.is_empty() {
        crate::mcp::call_tool(world, "objects_place", json!({ "items": items }))?;
    }
    if let Some(doc) = world
        .resource_mut::<crate::dungeons::DungeonMap>()
        .0
        .as_mut()
    {
        for (id, a) in &done {
            if let Some(m) = doc.markers.iter_mut().find(|m| m.id == *id) {
                m.filled = Some(a.clone());
            }
        }
    }
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(
        json!({ "filled": done.iter().map(|(id, a)| json!({ "marker": id, "asset": a })).collect::<Vec<_>>(), "no_fit": none, "note": if none.is_empty() { "placed as ordinary objects (undo removes them)" } else { "no installed model fits some markers: give `asset` (objects_search)" } }),
    )
}

pub(super) fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "dungeons_sheets_library",
        description: "The design sheets dungeons can be built from: the bundled library (top-down level-design sheets of open-air dungeons — each with its zones, paths, stairs, water, boss rings and object key read into a blueprint) and the user's own sheets (painted for their worlds by dungeons_sheet_generate). Filter by `family` (temperate, arid, cold, tropical, volcanic, twilight), `kind` (party, raid) or layout `type` (gauntlet, hub, switchback, spiral_in, pit, islands, fork, siege, maze, figure8, ridge, river, city, battlefield, loop). Build one with dungeons_create {sheet: id}.",
        input_schema: || json!({ "type": "object", "properties": { "family": { "type": "string" }, "kind": { "type": "string", "enum": ["party", "raid"] }, "type": { "type": "string" } } }),
        run: |_, a| {
            let fam: Option<String> = opt_arg(&a, "family")?;
            let kind: Option<String> = opt_arg(&a, "kind")?;
            let ty: Option<String> = opt_arg(&a, "type")?;
            let lib: Vec<Value> = library::index()
                .iter()
                .filter(|e| fam.as_deref().is_none_or(|f| e.family == f) && kind.as_deref().is_none_or(|k| serde_json::to_value(e.kind).ok().and_then(|v| v.as_str().map(|s| s == k)).unwrap_or(false)) && ty.as_deref().is_none_or(|t| e.layout_type == t))
                .map(|e| json!({ "id": e.id, "name": e.name, "family": e.family, "kind": e.kind, "type": e.layout_type, "size_m": e.size, "zones": e.zones, "bosses": e.bosses }))
                .collect();
            let mine: Vec<Value> = library::user_sheets().into_iter().filter(|(_, _, f)| fam.as_deref().is_none_or(|x| x == f)).map(|(id, name, family)| json!({ "id": id, "name": name, "family": family })).collect();
            Ok(vec![McpContent::Json(json!({ "library": lib, "yours": mine, "note": if library::index().is_empty() { "the bundled library isn't installed (assets/dungeon_sheets)" } else { "" } }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_preview",
        description: "A design sheet's picture (PNG, `px` wide, default 1024) and its blueprint: the zones (number, name, role, position m from the map's centre with y north, size, elevation, boss ring), the links, the water and chasms, the object key and the ending; plus anything that would stop it being built. `sheet`: a library id or one of the user's own; or leave it out in an open sheet realm for its own sheet.",
        input_schema: || json!({ "type": "object", "properties": { "sheet": { "type": "string" }, "px": { "type": "integer", "minimum": 256, "maximum": 1536 }, "full": { "type": "boolean", "description": "the whole blueprint, not its summary" } } }),
        run: |w, a| {
            let px = opt_arg::<u32>(&a, "px")?.unwrap_or(1024).clamp(256, 1536);
            let full: bool = opt_arg(&a, "full")?.unwrap_or(false);
            let (bp, pic) = match opt_arg::<String>(&a, "sheet")? {
                Some(id) => {
                    let r = if library::entry(&id).is_some() { SheetRef { id: id.clone(), custom: false, family: String::new() } } else { SheetRef { id: id.clone(), custom: true, family: String::new() } };
                    let world = w.resource::<crate::world::WorldMeta>().path.clone();
                    library::load(&r, world.as_deref())?
                }
                None => {
                    let dir = super::open_sheet_dir(w).ok_or("give a `sheet` (the open map wasn't made from one)")?;
                    let bp = serde_json::from_str(&std::fs::read_to_string(dir.join("blueprint.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                    (bp, library::picture(&dir))
                }
            };
            let mut out = vec![];
            if let Some(p) = pic {
                out.push(McpContent::Png(png_of(&p, px)?));
            }
            out.push(McpContent::Json(json!({ "blueprint": if full { serde_json::to_value(&bp).unwrap_or_default() } else { bp.summary() }, "problems": bp.problems(), "notes": bp.notes })));
            Ok(out)
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_status",
        description: "The open sheet realm's build: while it runs (over a few seconds after dungeons_open first makes the map) its progress; once done, what was made — the sheet's zones and links, how many markers and how many were filled with installed models, the objects placed and anything skipped. `wait` true waits for the build (up to `seconds`, default 60).",
        input_schema: || json!({ "type": "object", "properties": { "wait": { "type": "boolean" }, "seconds": { "type": "number" } } }),
        run: |w, a| {
            let wait: bool = opt_arg(&a, "wait")?.unwrap_or(false);
            if !wait || !super::running(w) {
                return Ok(vec![McpContent::Json(status_json(w))]);
            }
            let secs: f32 = opt_arg(&a, "seconds")?.unwrap_or(60.0);
            let start = w.resource::<Time<Real>>().elapsed_secs();
            Ok(vec![McpContent::deferred(move |w| {
                let now = w.resource::<Time<Real>>().elapsed_secs();
                (!super::running(w) || now - start > secs).then(|| Ok(vec![McpContent::Json(status_json(w))]))
            })])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_markers",
        description: "The open sheet realm's markers: where its sheet's object key puts something (id, key letter, name, structure or prop, size w × d × h m, position [x, z], yaw, zone, and the model filling it if any). `zone` or `letter` narrow it; `suggestions` true adds the installed models that fit each unfilled one (asset, scale). Fill them with dungeons_sheet_fill.",
        input_schema: || json!({ "type": "object", "properties": { "zone": { "type": "integer" }, "letter": { "type": "string" }, "suggestions": { "type": "boolean" } } }),
        run: |w, a| {
            let zone: Option<u8> = opt_arg(&a, "zone")?;
            let letter: Option<String> = opt_arg(&a, "letter")?;
            let s: bool = opt_arg(&a, "suggestions")?.unwrap_or(false);
            Ok(vec![McpContent::Json(markers_json(w, zone, letter.as_deref(), s))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_fill",
        description: "Puts models where markers stand in the open sheet realm: `markers` (ids) or every marker of a key `letter`; `asset` (an objects_search id, e.g. hp:stone_pillar) and `scale`, or leave `asset` out for each marker's best-fitting installed model. The objects are ordinary (move, swap, delete them); the markers stay, hidden, so they can be filled again.",
        input_schema: || json!({ "type": "object", "properties": { "markers": { "type": "array", "items": { "type": "integer" } }, "letter": { "type": "string" }, "asset": { "type": "string" }, "scale": { "type": "number", "minimum": 0.05, "maximum": 20.0 } } }),
        run: |w, a| {
            let mut ids: Vec<u32> = opt_arg(&a, "markers")?.unwrap_or_default();
            if let Some(l) = opt_arg::<String>(&a, "letter")? {
                if let Some(doc) = w.resource::<crate::dungeons::DungeonMap>().0.as_ref() {
                    ids.extend(doc.markers.iter().filter(|m| m.letter.eq_ignore_ascii_case(&l)).map(|m| m.id));
                }
            }
            if ids.is_empty() {
                return Err("give `markers` or a `letter`".into());
            }
            let v = fill_markers(w, ids, opt_arg(&a, "asset")?, opt_arg(&a, "scale")?)?;
            Ok(vec![McpContent::Json(v)])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_markers_show",
        description: "Shows or hides the sheet realm's markers in the viewport (`on`), and in walk mode (`walk`). Returns the setting.",
        input_schema: || json!({ "type": "object", "properties": { "on": { "type": "boolean" }, "walk": { "type": "boolean" } } }),
        run: |w, a| {
            let on: Option<bool> = opt_arg(&a, "on")?;
            let walk: Option<bool> = opt_arg(&a, "walk")?;
            let mut s = w.resource_mut::<SheetUi>();
            if let Some(v) = on {
                s.on = v;
            }
            if let Some(v) = walk {
                s.walk = v;
            }
            Ok(vec![McpContent::Json(json!({ "on": s.on, "walk": s.walk }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_generate",
        description: "Paints a new, unique design sheet with the user's own AI (the Codex CLI of the ChatGPT app, signed in with ChatGPT; it uses their image allowance) and reads it into a blueprint, in the background (several minutes): Codex writes an original brief for the `family` (temperate, arid, cold, tropical, volcanic, twilight), `kind` (party, raid) and layout `type` (default: one the world hasn't used), paints the sheet, checks it, and it is read and measured. With `dungeon` (an id whose map isn't made yet) that dungeon switches to the new sheet when it's ready. dungeons_sheet_jobs reports progress; finished sheets are listed under `yours` in dungeons_sheets_library.",
        input_schema: || json!({ "type": "object", "properties": { "family": { "type": "string", "enum": super::family::FAMILIES }, "kind": { "type": "string", "enum": ["party", "raid"] }, "type": { "type": "string" }, "dungeon": { "type": "integer" } }, "required": ["family"] }),
        run: |w, a| {
            let family: String = arg(&a, "family")?;
            if !super::family::FAMILIES.contains(&family.as_str()) {
                return Err(format!("unknown family `{family}`: {}", super::family::FAMILIES.join(", ")));
            }
            let kind = match opt_arg::<String>(&a, "kind")?.as_deref() {
                Some("raid") => Kind::Raid,
                _ => Kind::Party,
            };
            let req = request_for(w, family, kind, opt_arg(&a, "type")?, opt_arg(&a, "dungeon")?);
            let id = generate::queue(&mut w.resource_mut::<SheetJobs>(), req)?;
            Ok(vec![McpContent::Json(json!({ "job": id, "note": "painting and reading take several minutes; dungeons_sheet_jobs reports progress" }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_jobs",
        description: "Progress of the sheets being painted (dungeons_sheet_generate): each job's stage (queued, painting, reading, ready, failed), its note, the finished sheet's name, and whether its dungeon switched to it.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, _| {
            let mut j = w.resource_mut::<SheetJobs>();
            let codex = j.codex().is_some();
            Ok(vec![McpContent::Json(json!({ "codex": codex, "jobs": j.jobs }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_read_prompt",
        description: "The instructions for reading a design sheet (an image) into the JSON dungeons_sheet_import takes as `reading`: what to measure in the image's pixels — the zones, links, stairs, water, chasms, object key, start, ending and art direction.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |_, _| Ok(vec![McpContent::Text(super::read::READ_PROMPT.to_string())]),
    });
    app.mcp_tool(McpTool {
        name: "dungeons_sheet_import",
        description: "Adds a design sheet to the user's own sheets: `image` (a PNG or JPEG path) and either `reading` (the JSON dungeons_sheet_read_prompt asks for, measured in the image's pixels — it is measured and repaired into a blueprint here) or `blueprint` (a finished blueprint object). Optional `family`, `kind`, `name`. Returns its id (dungeons_create {sheet: id}) and anything that stops it being built.",
        input_schema: || json!({ "type": "object", "properties": { "image": { "type": "string" }, "reading": {}, "blueprint": { "type": "object" }, "family": { "type": "string" }, "kind": { "type": "string", "enum": ["party", "raid"] }, "name": { "type": "string" } }, "required": ["image"] }),
        run: |_, a| {
            let image: String = arg(&a, "image")?;
            let img = image::open(image.trim()).map_err(|e| format!("couldn't read {image}: {e}"))?;
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let mut brief = json!({});
            for k in ["family", "kind", "name"] {
                if let Some(v) = a.get(k) {
                    brief[k] = v.clone();
                }
            }
            let (mut bp, notes) = if let Some(b) = a.get("blueprint").filter(|b| b.is_object()) {
                (serde_json::from_value::<super::Blueprint>(b.clone()).map_err(|e| format!("bad blueprint: {e}"))?, vec![])
            } else {
                let r = a.get("reading").ok_or("give `reading` or `blueprint`")?;
                let text = if let Some(s) = r.as_str() { s.to_string() } else { r.to_string() };
                let reading = super::read::parse(&text)?;
                super::read::normalize(&reading, Some(&img.to_rgba8()), Some(&brief))?
            };
            let id = format!("i{stamp}_{}", crate::dungeons::sheets::library::slug(&bp.name));
            if bp.id.is_empty() {
                bp.id = id.clone();
            }
            if let Some(n) = brief["name"].as_str() {
                bp.name = n.into();
            }
            let dir = library::user_dir().join(&id);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            img.save(dir.join("sheet.png")).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("blueprint.json"), serde_json::to_string_pretty(&bp).unwrap_or_default()).map_err(|e| e.to_string())?;
            Ok(vec![McpContent::Json(json!({ "sheet": id, "summary": bp.summary(), "problems": bp.problems(), "notes": notes }))])
        },
    });
}

/// A generation request for this world (its name, its dungeons' names and layout types).
pub fn request_for(
    w: &World,
    family: String,
    kind: Kind,
    layout_type: Option<String>,
    dungeon: Option<u32>,
) -> Request {
    let ds = w.resource::<crate::dungeons::Dungeons>();
    let world = w.resource::<crate::world::WorldMeta>().name.clone();
    let avoid: Vec<String> = ds.list.iter().map(|d| d.name.clone()).collect();
    let used_types: Vec<String> = ds
        .list
        .iter()
        .filter_map(|d| d.sheet.as_ref())
        .filter_map(|r| library::entry(&r.id).map(|e| e.layout_type.clone()))
        .collect();
    let seed = (ds.list.len() as u32).wrapping_mul(2654435761) ^ dungeon.unwrap_or(7);
    Request {
        family,
        kind,
        layout_type,
        world,
        avoid,
        used_types,
        dungeon,
        seed,
    }
}
