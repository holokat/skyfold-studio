//! MCP tools for the Characters lab: the same code paths as the Character view.

use autorig::markers::{Marker, Markers};
use bevy::prelude::*;
use serde_json::{Value, json};

use super::view::CharacterView;
use super::{AnimLibrary, Characters, OnImport, RigKind, WorldCharacters};
use crate::mcp_tools::{
    McpAppExt, McpContent, McpTool, arg, bearing_schema, opt_arg, position_arg,
    position_schema,
};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "characters_labs",
        description: "Turn the Characters lab (testing: character import, automatic rigging, animation preview, animated characters in the world) on or off. Returns whether it is on.",
        input_schema: || json!({"type": "object", "properties": {"enabled": {"type": "boolean"}}}),
        run: labs,
    })
    .mcp_tool(McpTool {
        name: "characters_list",
        description: "The character library: id, name, rig (none / auto / imported / builtin), height (m), triangles, whether it's included in exports and its export animations, notes, and any work in progress.",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: list,
    })
    .mcp_tool(McpTool {
        name: "characters_import",
        description: "Import a character model from a file on this computer (.fbx, .glb, .gltf, .obj or a .zip holding one), standing in a T- or A-pose. `rig`: auto (default: keep a humanoid rig it came with, else rig it automatically), rerig (always rig it here), prepare (stop before rigging: markers guessed). Waits for the import (and rig) to finish and returns the character.",
        input_schema: || json!({"type": "object", "properties": {
            "path": {"type": "string", "description": "absolute file path"},
            "rig": {"type": "string", "enum": ["auto", "rerig", "prepare"]}}, "required": ["path"]}),
        run: import,
    })
    .mcp_tool(McpTool {
        name: "characters_markers",
        description: "A character's body markers (chin, groin, wrist_l/r, elbow_l/r, knee_l/r) in its model space (metres, feet on y = 0, facing +z, its left on +x): the ones its rig used, or guessed from its shape. Edit them and pass them to characters_rig to fix a rig.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"},
            "guess": {"type": "boolean", "description": "guess afresh from the shape"}}, "required": ["id"]}),
        run: markers,
    })
    .mcp_tool(McpTool {
        name: "characters_rig",
        description: "Rig (or re-rig) a character automatically with the standard humanoid skeleton: skeleton fitted to its markers (guessed, or `markers` as from characters_markers: any subset, the rest kept), skin weights computed through its body. Waits until done.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"},
            "markers": {"type": "object", "description": "any of the markers, each [x, y, z] (model space)", "properties": {
                "chin": xyz(), "groin": xyz(), "wrist_l": xyz(), "wrist_r": xyz(),
                "elbow_l": xyz(), "elbow_r": xyz(), "knee_l": xyz(), "knee_r": xyz()}}}, "required": ["id"]}),
        run: rig,
    })
    .mcp_tool(McpTool {
        name: "characters_animations",
        description: "The animation library every character can play: key (pass to other tools), label, category (Idle, Move, Combat, Actions, Social, Creature, Mine), seconds, looped, source and licence. Optional `query` (label contains) and `category`.",
        input_schema: || json!({"type": "object", "properties": {"query": {"type": "string"}, "category": {"type": "string"}}}),
        run: animations,
    })
    .mcp_tool(McpTool {
        name: "characters_import_animations",
        description: "Import the user's own animation files (FBX or glTF on any humanoid rig: Mixamo downloads, packs they own) into their animation library (kept on this computer only). Returns the new animation keys.",
        input_schema: || json!({"type": "object", "properties": {"paths": {"type": "array", "items": {"type": "string"}}}, "required": ["paths"]}),
        run: import_animations,
    })
    .mcp_tool(McpTool {
        name: "characters_preview",
        description: "Open the Characters tab with a model selected (`id`), playing `animation` (a library key) on the stage; or with an NPC (`npc_id`), a creature (`creature_id`) or the player (`player`: true) open in its inspector instead. `tab` picks the inspector tab (rig, animations, moves, export; for an NPC: basics, behavior, talk, moves). Take a screenshot to see it. `yaw_deg` turns the stage camera (0 = straight at its front), `front` shows the marker editor's front view.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}, "animation": {"type": "string"},
            "npc_id": {"type": "integer"}, "creature_id": {"type": "integer"}, "player": {"type": "boolean"},
            "tab": {"type": "string", "enum": ["rig", "animations", "moves", "export", "basics", "behavior", "talk"]},
            "yaw_deg": {"type": "number"}, "front": {"type": "boolean"}, "time": {"type": "number", "description": "pause at this many seconds into the clip"}}}),
        run: preview,
    })
    .mcp_tool(McpTool {
        name: "characters_add_npc",
        description: "Stand a rigged character in the world as an NPC (a Gameplay NPC with that model) at `position` ([x, z] on the ground, or [x, y, z]), facing `facing_deg` (compass). Optional `name` and `role` (quest_giver, vendor, trainer, guard, innkeeper, flight_master, banker, ambient, custom). It stands idle; give it a behavior with characters_npc_set. Returns the NPC id.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string", "description": "the character"},
            "position": position_schema("where its feet go"), "facing_deg": bearing_schema("where it faces"),
            "name": {"type": "string"}, "role": {"type": "string"}}, "required": ["id", "position"]}),
        run: add_npc,
    })
    .mcp_tool(McpTool {
        name: "characters_npcs",
        description: "The world's NPCs: id, name, role, model (character), position, facing (compass) and life (behavior, wander radius, route, schedule, leash, speed, attitude, faction, interactions, dialogue, moves).",
        input_schema: || json!({"type": "object", "properties": {}}),
        run: npcs,
    })
    .mcp_tool(McpTool {
        name: "characters_npc_set",
        description: "Change an NPC (any subset): `character` (model id, \"\" for none), `name`, `role`, `position`, `facing_deg`, `behavior` (stand, wander, patrol, or follow its schedule), `wander_radius` (m), `route` ([[x, z], …] waypoints), `route_loop`, `schedule` (day lines, each an hour, an activity and an optional spot `at` [x, z]), `typical_day` (true fills a day), `leash` (m), `speed` (× walk), `attitude` (friendly, neutral, hostile), `faction`, `can_talk`, `can_trade`, `gives_quests`, `can_join`, `attackable`, `greet` / `idle_line` / `farewell` (dialogue), `moves` ({move id: animation key, or \"\" to clear}). Returns the NPC.",
        input_schema: || json!({"type": "object", "properties": {"npc_id": {"type": "integer"},
            "character": {"type": "string"}, "name": {"type": "string"}, "role": {"type": "string"},
            "position": position_schema("where it stands"), "facing_deg": bearing_schema("where it faces"),
            "behavior": {"type": "string", "enum": ["stand", "wander", "patrol", "schedule"]},
            "wander_radius": {"type": "number"},
            "route": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}},
            "route_loop": {"type": "boolean"},
            "schedule": {"type": "array", "items": {"type": "object", "properties": {"hour": {"type": "number"},
                "activity": {"type": "string", "enum": ["stand", "wander", "patrol", "sit", "work", "talk", "sleep"]},
                "at": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}}, "required": ["hour", "activity"]}},
            "typical_day": {"type": "boolean"}, "leash": {"type": "number"}, "speed": {"type": "number"},
            "attitude": {"type": "string", "enum": ["friendly", "neutral", "hostile"]}, "faction": {"type": "string"},
            "can_talk": {"type": "boolean"}, "can_trade": {"type": "boolean"}, "gives_quests": {"type": "boolean"},
            "can_join": {"type": "boolean"}, "attackable": {"type": "boolean"},
            "greet": {"type": "string"}, "idle_line": {"type": "string"}, "farewell": {"type": "string"},
            "moves": {"type": "object", "additionalProperties": {"type": "string"}}}, "required": ["npc_id"]}),
        run: npc_set,
    })
    .mcp_tool(McpTool {
        name: "characters_set_player",
        description: "Choose the player character: Walk mode then plays as it in third person (P toggles first person; left click light attack, right click heavy, Q block, X roll). `id` null or \"\" goes back to first person only.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": ["string", "null"]},
            "third_person": {"type": "boolean"}}}),
        run: set_player,
    })
    .mcp_tool(McpTool {
        name: "characters_creature_set",
        description: "Give a creature type of the creature library (the monsters spawn areas draw from) a model: `character` (id, \"\" for none). It then roams its spawn areas while you walk. Returns it.",
        input_schema: || json!({"type": "object", "properties": {"creature_id": {"type": "integer"}, "character": {"type": "string"}}, "required": ["creature_id", "character"]}),
        run: creature_set,
    })
    .mcp_tool(McpTool {
        name: "characters_live",
        description: "NPCs and creatures act out their behavior in the editor too (they always do while walking): `enabled`. Returns whether it's on.",
        input_schema: || json!({"type": "object", "properties": {"enabled": {"type": "boolean"}}}),
        run: live,
    })
    .mcp_tool(McpTool {
        name: "characters_set",
        description: "Change a library character: `name`, `export` (include in world exports), `export_animations` (the full list of animation keys it exports with), `add_animations` / `remove_animations`, `idle` (the key it plays when placed).",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}, "name": {"type": "string"}, "export": {"type": "boolean"},
            "export_animations": {"type": "array", "items": {"type": "string"}},
            "add_animations": {"type": "array", "items": {"type": "string"}},
            "remove_animations": {"type": "array", "items": {"type": "string"}},
            "idle": {"type": "string"}}, "required": ["id"]}),
        run: set,
    })
    .mcp_tool(McpTool {
        name: "characters_delete",
        description: "Delete an imported character (its folder is moved aside, not erased) and remove it from the world.",
        input_schema: || json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        run: delete,
    });
}

fn xyz() -> Value {
    json!({"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3})
}

fn need_labs() -> Result<(), String> {
    if super::labs_enabled() {
        Ok(())
    } else {
        Err("the Characters lab is off: turn it on with characters_labs {\"enabled\": true}".into())
    }
}

fn labs(_: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    if let Some(on) = opt_arg::<bool>(&args, "enabled")? {
        super::set_labs(on);
    }
    Ok(vec![McpContent::Json(json!({"enabled": super::labs_enabled()}))])
}

fn describe(chars: &Characters, e: &super::Entry) -> Value {
    json!({
        "id": e.meta.id,
        "name": e.meta.name,
        "rig": if e.builtin.is_some() { "builtin" } else { match e.meta.rig { RigKind::None => "none", RigKind::Auto => "auto", RigKind::Imported => "imported" } },
        "height_m": (e.meta.height * 100.0).round() / 100.0,
        "triangles": e.meta.triangles,
        "source": e.meta.source,
        "export": e.meta.export,
        "export_animations": e.meta.export_anims,
        "idle": e.meta.idle,
        "notes": e.meta.notes,
        "working": chars.busy(&e.meta.id).map(|j| format!("{}: {}", j.what, j.status.lock().unwrap())),
    })
}

fn list(world: &mut World, _: Value) -> Result<Vec<McpContent>, String> {
    let chars = world.resource::<Characters>();
    let v: Vec<Value> = chars.entries.iter().map(|e| describe(chars, e)).collect();
    Ok(vec![McpContent::Json(json!({"characters": v, "labs": super::labs_enabled()}))])
}

/// Finishes when `id` has no job running: its description, or the job's error.
fn wait_for(id: String) -> McpContent {
    McpContent::deferred(move |world: &mut World| {
        // (the job's result is collected by `poll_jobs`; its error arrives as an event)
        let chars = world.resource::<Characters>();
        if chars.busy(&id).is_some() {
            return None;
        }
        Some(match chars.get(&id) {
            Some(e) => Ok(vec![McpContent::Json(describe(chars, e))]),
            None => Err(format!("{id}: the import failed (see the log)")),
        })
    })
}

fn import(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let path: String = arg(&args, "path")?;
    let p = std::path::PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("no file at {path}"));
    }
    let on = match opt_arg::<String>(&args, "rig")?.as_deref() {
        None | Some("auto") => OnImport::Auto,
        Some("rerig") => OnImport::Rerig,
        Some("prepare") => OnImport::Prepare,
        Some(o) => return Err(format!("rig: unknown `{o}`")),
    };
    let id = world.resource_mut::<Characters>().import(&p, on);
    world.resource_mut::<CharacterView>().select_character(&id);
    Ok(vec![wait_for(id)])
}

fn markers_json(m: &Markers) -> Value {
    let mut o = serde_json::Map::new();
    for k in Marker::ALL {
        let p = m.get(k);
        o.insert(
            k.id().into(),
            json!([round3(p.x), round3(p.y), round3(p.z)]),
        );
    }
    Value::Object(o)
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

fn markers(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let fresh = opt_arg::<bool>(&args, "guess")?.unwrap_or(false);
    {
        let chars = world.resource::<Characters>();
        let e = chars.get(&id).ok_or("no such character")?;
        if !fresh && let Some(m) = &e.meta.markers {
            return Ok(vec![McpContent::Json(markers_json(m))]);
        }
    }
    Ok(vec![McpContent::deferred(move |world: &mut World| {
        let mut chars = world.resource_mut::<Characters>();
        let m = chars.prepared_model(&id)?;
        Some(Ok(vec![McpContent::Json(markers_json(&autorig::markers::guess(&m)))]))
    })])
}

fn rig(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let given: Option<serde_json::Map<String, Value>> = opt_arg(&args, "markers")?;
    let mut markers = None;
    if let Some(given) = given {
        let chars = world.resource::<Characters>();
        let e = chars.get(&id).ok_or("no such character")?;
        let mut m = e
            .meta
            .markers
            .ok_or("this character has no markers yet: call characters_markers first")?;
        for (k, v) in given {
            let mk = Marker::from_id(&k).ok_or(format!("unknown marker `{k}`"))?;
            let a: [f32; 3] = serde_json::from_value(v).map_err(|e| format!("{k}: {e}"))?;
            m.set(mk, Vec3::from(a));
        }
        markers = Some(m);
    }
    world.resource_mut::<Characters>().rig(&id, markers)?;
    Ok(vec![wait_for(id)])
}

fn animations(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let q = opt_arg::<String>(&args, "query")?.unwrap_or_default().to_lowercase();
    let cat = opt_arg::<String>(&args, "category")?.map(|c| c.to_lowercase());
    let ready = world.resource_mut::<AnimLibrary>().ensure();
    if !ready {
        return Ok(vec![McpContent::deferred(move |world: &mut World| {
            world
                .resource_mut::<AnimLibrary>()
                .ensure()
                .then(|| Ok(vec![McpContent::Json(anim_list(world, &q, cat.as_deref()))]))
        })]);
    }
    Ok(vec![McpContent::Json(anim_list(world, &q, cat.as_deref()))])
}

fn anim_list(world: &World, q: &str, cat: Option<&str>) -> Value {
    let lib = world.resource::<AnimLibrary>();
    let v: Vec<Value> = lib
        .anims
        .iter()
        .filter(|a| q.is_empty() || a.label.to_lowercase().contains(q) || a.key.to_lowercase().contains(q))
        .filter(|a| cat.is_none_or(|c| a.category.name().to_lowercase() == c))
        .map(|a| {
            let s = &lib.sources[a.source];
            json!({"key": a.key, "label": a.label, "category": a.category.name(),
                "seconds": round3(a.seconds), "looped": a.looped, "source": s.name, "license": s.license})
        })
        .collect();
    json!({"animations": v})
}

fn import_animations(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let paths: Vec<String> = arg(&args, "paths")?;
    let paths: Vec<std::path::PathBuf> = paths.into_iter().map(Into::into).collect();
    let keys = AnimLibrary::import_files(&paths)?;
    world.resource_mut::<AnimLibrary>().reload();
    Ok(vec![McpContent::Json(json!({"added": keys}))])
}

fn preview(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    use super::view::{CharTab, NpcTab, Section, Sel};
    let id: Option<String> = opt_arg(&args, "id")?;
    let npc: Option<u32> = opt_arg(&args, "npc_id")?;
    let creature: Option<u32> = opt_arg(&args, "creature_id")?;
    let player = opt_arg::<bool>(&args, "player")?.unwrap_or(false);
    if let Some(id) = &id
        && world.resource::<Characters>().get(id).is_none()
    {
        return Err("no such character".into());
    }
    world.resource_mut::<crate::editor::Editor>().tab = crate::editor::Tab::Characters;
    {
        let tab: Option<String> = opt_arg(&args, "tab")?;
        let mut v = world.resource_mut::<CharacterView>();
        let (section, sel) = if let Some(n) = npc {
            (Section::Npcs, Sel::Npc(n))
        } else if let Some(c) = creature {
            (Section::Creatures, Sel::Creature(c))
        } else if player {
            (Section::Player, Sel::Player)
        } else {
            let id = id.clone().ok_or("give `id`, `npc_id`, `creature_id` or `player`")?;
            (Section::Characters, Sel::Character(id))
        };
        if let Sel::Character(c) = &sel {
            // (on the stage now: the view keeps the mode asked for)
            v.selected = Some(c.clone());
        }
        v.section = section;
        v.sel = Some(sel);
        match tab.as_deref() {
            Some("rig") => v.char_tab = CharTab::Rig,
            Some("animations") => v.char_tab = CharTab::Animations,
            Some("moves") => {
                v.char_tab = CharTab::Moves;
                v.npc_tab = NpcTab::Moves;
            }
            Some("export") => v.char_tab = CharTab::Export,
            Some("basics") => v.npc_tab = NpcTab::Basics,
            Some("behavior") => v.npc_tab = NpcTab::Behavior,
            Some("talk") => v.npc_tab = NpcTab::Talk,
            _ => {}
        }
        v.placing = None;
        v.looking = false;
        v.mode = if opt_arg::<bool>(&args, "front")?.unwrap_or(false) {
            super::view::Mode::Markers
        } else {
            super::view::Mode::Preview
        };
    }
    let mut stage = world.resource_mut::<super::stage::Stage>();
    if let Some(a) = opt_arg::<String>(&args, "animation")? {
        stage.anim = Some(a);
    }
    if let Some(y) = opt_arg::<f32>(&args, "yaw_deg")? {
        stage.orbit.yaw = y.to_radians();
    }
    match opt_arg::<f32>(&args, "time")? {
        Some(t) => {
            stage.seek = Some(t);
            stage.playing = false;
        }
        None => stage.playing = true,
    }
    Ok(vec![McpContent::Text("Character view open".into())])
}

/// The world position for a `position` argument (snapped to the ground for [x, z]).
fn ground(world: &World, args: &Value) -> Result<Option<Vec3>, String> {
    let Some((xz, y)) = position_arg(args, "position")? else {
        return Ok(None);
    };
    let y = y.unwrap_or_else(|| {
        world
            .resource::<crate::terrain::Terrain>()
            .height_at(xz.x, xz.y)
    });
    Ok(Some(Vec3::new(xz.x, y, xz.y)))
}


fn parse<T: serde::de::DeserializeOwned>(name: &str, v: &str) -> Result<T, String> {
    // ("quest_giver" → "QuestGiver")
    let camel: String = v
        .split(['_', ' ', '-'])
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect();
    serde_json::from_value(Value::String(camel)).map_err(|_| format!("{name}: unknown `{v}`"))
}

fn npc_json(n: &crate::gameplay::Npc) -> Value {
    json!({"npc_id": n.id, "name": n.name, "role": n.role.name(), "character": n.life.character,
        "position": n.pos, "facing_deg": crate::mcp_tools::facing_bearing(n.yaw), "life": n.life})
}

fn add_npc(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id: String = arg(&args, "id")?;
    let name = {
        let chars = world.resource::<Characters>();
        let e = chars.get(&id).ok_or("no such character")?;
        if !e.rigged() {
            return Err("rig it first (characters_rig)".into());
        }
        e.meta.name.clone()
    };
    let pos = ground(world, &args)?.ok_or("missing argument `position`")?;
    let yaw = opt_arg::<f32>(&args, "facing_deg")?
        .map(crate::mcp_tools::facing_yaw)
        .unwrap_or(0.0);
    let role = match opt_arg::<String>(&args, "role")? {
        Some(r) => parse::<crate::gameplay::NpcRole>("role", &r)?,
        None => crate::gameplay::NpcRole::Ambient,
    };
    let given = opt_arg::<String>(&args, "name")?.unwrap_or(name);
    let n = world.resource_scope(|world, mut gp: Mut<crate::gameplay::Gameplay>| {
        let g = crate::gameplay::edit::Ground {
            terrain: world.resource::<crate::terrain::Terrain>(),
            zones: world.resource::<crate::world::Zones>(),
            water: world.resource::<crate::water::WaterBodies>(),
            nature: None,
        };
        let n = crate::gameplay::edit::add_npc(&mut gp, &g, role, pos.xz(), yaw, Some(given), None);
        if let Some(x) = gp.npcs.iter_mut().find(|x| x.id == n) {
            x.life.character = Some(id.clone());
            x.pos[1] = pos.y;
        }
        n
    });
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(vec![McpContent::Json(json!({"npc_id": n}))])
}

fn npcs(world: &mut World, _: Value) -> Result<Vec<McpContent>, String> {
    let gp = world.resource::<crate::gameplay::Gameplay>();
    let v: Vec<Value> = gp.npcs.iter().map(npc_json).collect();
    Ok(vec![McpContent::Json(json!({"npcs": v}))])
}

fn npc_set(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let n: u32 = arg(&args, "npc_id")?;
    let pos = ground(world, &args)?;
    if let Some(c) = opt_arg::<String>(&args, "character")?.filter(|c| !c.is_empty()) {
        let chars = world.resource::<Characters>();
        if !chars.get(&c).is_some_and(|e| e.rigged()) {
            return Err(format!("no rigged character `{c}`"));
        }
    }
    let mut gp = world.resource_mut::<crate::gameplay::Gameplay>();
    let x = gp.npcs.iter_mut().find(|x| x.id == n).ok_or("no such NPC")?;
    let l = &mut x.life;
    if let Some(c) = opt_arg::<String>(&args, "character")? {
        l.character = (!c.is_empty()).then_some(c);
    }
    if let Some(v) = opt_arg::<String>(&args, "name")? {
        x.name = v;
    }
    if let Some(v) = opt_arg::<String>(&args, "role")? {
        x.role = parse("role", &v)?;
    }
    if let Some(p) = pos {
        x.pos = p.to_array();
    }
    if let Some(f) = opt_arg::<f32>(&args, "facing_deg")? {
        x.yaw = crate::mcp_tools::facing_yaw(f);
    }
    let l = &mut x.life;
    if let Some(v) = opt_arg::<String>(&args, "behavior")? {
        l.behavior = parse("behavior", &v)?;
    }
    if let Some(v) = opt_arg(&args, "wander_radius")? {
        l.wander_radius = v;
    }
    if let Some(v) = opt_arg(&args, "route")? {
        l.route = v;
    }
    if let Some(v) = opt_arg(&args, "route_loop")? {
        l.route_loop = v;
    }
    if let Some(list) = opt_arg::<Vec<Value>>(&args, "schedule")? {
        let mut out = vec![];
        for s in list {
            let hour = s.get("hour").and_then(|h| h.as_f64()).ok_or("schedule: each line needs an `hour`")? as f32;
            let act = s.get("activity").and_then(|a| a.as_str()).ok_or("schedule: each line needs an `activity`")?;
            let at = s.get("at").and_then(|a| serde_json::from_value::<[f32; 2]>(a.clone()).ok());
            out.push(super::npc::Slot { hour: hour.rem_euclid(24.0), activity: parse("activity", act)?, at });
        }
        l.schedule = out;
    }
    if opt_arg::<bool>(&args, "typical_day")?.unwrap_or(false) {
        l.typical_day();
    }
    if let Some(v) = opt_arg(&args, "leash")? {
        l.leash = v;
    }
    if let Some(v) = opt_arg(&args, "speed")? {
        l.speed = v;
    }
    if let Some(v) = opt_arg::<String>(&args, "attitude")? {
        l.attitude = parse("attitude", &v)?;
    }
    if let Some(v) = opt_arg(&args, "faction")? {
        l.faction = v;
    }
    for (k, f) in [
        ("can_talk", &mut l.can_talk as &mut bool),
        ("can_trade", &mut l.can_trade),
        ("gives_quests", &mut l.gives_quests),
        ("can_join", &mut l.can_join),
        ("attackable", &mut l.attackable),
    ] {
        if let Some(v) = opt_arg::<bool>(&args, k)? {
            *f = v;
        }
    }
    if let Some(v) = opt_arg(&args, "greet")? {
        l.lines.greet = v;
    }
    if let Some(v) = opt_arg(&args, "idle_line")? {
        l.lines.idle = v;
    }
    if let Some(v) = opt_arg(&args, "farewell")? {
        l.lines.farewell = v;
    }
    if let Some(m) = opt_arg::<std::collections::BTreeMap<String, String>>(&args, "moves")? {
        for (k, a) in m {
            if super::moves::Move::from_id(&k).is_none() {
                return Err(format!("moves: unknown move `{k}`"));
            }
            if a.is_empty() {
                l.moves.remove(&k);
            } else {
                l.moves.insert(k, a);
            }
        }
    }
    let out = npc_json(x);
    gp.dirty = true;
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(vec![McpContent::Json(out)])
}

fn set_player(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    need_labs()?;
    let id = opt_arg::<String>(&args, "id")?.filter(|s| !s.is_empty());
    if let Some(c) = &id
        && !world.resource::<Characters>().get(c).is_some_and(|e| e.rigged())
    {
        return Err(format!("no rigged character `{c}`"));
    }
    world.resource_mut::<WorldCharacters>().player = id.clone();
    if let Some(t) = opt_arg::<bool>(&args, "third_person")? {
        world.resource_mut::<super::player::PlayerView>().third_person = t;
    }
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(vec![McpContent::Json(json!({"player": id}))])
}

fn creature_set(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let n: u32 = arg(&args, "creature_id")?;
    let c: String = arg(&args, "character")?;
    if !c.is_empty() && !world.resource::<Characters>().get(&c).is_some_and(|e| e.rigged()) {
        return Err(format!("no rigged character `{c}`"));
    }
    let mut gp = world.resource_mut::<crate::gameplay::Gameplay>();
    let x = gp.creatures.iter_mut().find(|x| x.id == n).ok_or("no such creature")?;
    x.character = (!c.is_empty()).then_some(c);
    let out = json!({"creature_id": x.id, "name": x.name, "character": x.character});
    gp.dirty = true;
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(vec![McpContent::Json(out)])
}

fn live(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let mut e = world.resource_mut::<super::npc::NpcEdit>();
    if let Some(v) = opt_arg::<bool>(&args, "enabled")? {
        e.preview = v;
    }
    Ok(vec![McpContent::Json(json!({"enabled": e.preview}))])
}

fn set(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: String = arg(&args, "id")?;
    let mut chars = world.resource_mut::<Characters>();
    let mut meta = chars.get(&id).ok_or("no such character")?.meta.clone();
    if let Some(n) = opt_arg::<String>(&args, "name")? {
        meta.name = n;
    }
    if let Some(x) = opt_arg::<bool>(&args, "export")? {
        meta.export = x;
    }
    if let Some(list) = opt_arg::<Vec<String>>(&args, "export_animations")? {
        meta.export_anims = list;
    }
    for a in opt_arg::<Vec<String>>(&args, "add_animations")?.unwrap_or_default() {
        if !meta.export_anims.contains(&a) {
            meta.export_anims.push(a);
        }
    }
    let remove = opt_arg::<Vec<String>>(&args, "remove_animations")?.unwrap_or_default();
    meta.export_anims.retain(|a| !remove.contains(a));
    if let Some(i) = opt_arg::<String>(&args, "idle")? {
        meta.idle = (!i.is_empty()).then_some(i);
    }
    chars.save_meta(&meta);
    let chars = world.resource::<Characters>();
    let e = chars.get(&id).ok_or("no such character")?;
    Ok(vec![McpContent::Json(describe(chars, e))])
}

fn delete(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let id: String = arg(&args, "id")?;
    world.resource_mut::<Characters>().delete(&id)?;
    {
        let mut w = world.resource_mut::<WorldCharacters>();
        if w.player.as_deref() == Some(id.as_str()) {
            w.player = None;
        }
    }
    {
        let mut gp = world.resource_mut::<crate::gameplay::Gameplay>();
        for n in &mut gp.npcs {
            if n.life.character.as_deref() == Some(id.as_str()) {
                n.life.character = None;
            }
        }
        for c in &mut gp.creatures {
            if c.character.as_deref() == Some(id.as_str()) {
                c.character = None;
            }
        }
    }
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    Ok(vec![McpContent::Text("deleted".into())])
}
