//! How a tool call runs, for every registered tool (HTTP, `WB_MCP_CALL` and `batch` alike):
//! its arguments are checked against the tool's schema (`validate`), `items` runs an add-style
//! tool several times as one undo step, and `batch` runs a list of calls as one undo step
//! (atomic batches roll back on the first error).

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use super::validate;
use crate::history::History;
use crate::mcp_tools::{McpContent, McpTool, McpTools};
use crate::objects::ObjectStore;

pub(crate) type R = Result<Vec<McpContent>, String>;

/// Add-style tools that also take `items` (several at once, shared arguments as defaults).
pub(crate) const ITEM_TOOLS: &[&str] = &[
    "terrain_land",
    "gameplay_add_poi",
    "gameplay_add_npc",
    "gameplay_add_resource_node",
    "zones_create",
    "fish_add",
];
const MAX_ITEMS: usize = 500;
const MAX_CALLS: usize = 200;
/// Calls a batch can't contain: they replace the world or step the undo history.
const NOT_IN_BATCH: &[&str] = &[
    "batch",
    "undo",
    "redo",
    "world_new",
    "world_load",
];

/// The arguments schema a tool takes (`items` added for [`ITEM_TOOLS`]).
pub(crate) fn schema_of(tool: &McpTool) -> Value {
    let base = (tool.input_schema)();
    if !ITEM_TOOLS.contains(&tool.name) {
        return base;
    }
    let mut item = json!({ "type": "object", "properties": base.get("properties").cloned().unwrap_or(json!({})) });
    if let Some(a) = base.get("x-aliases") {
        item["x-aliases"] = a.clone();
    }
    let mut s = base;
    if let Some(o) = s.as_object_mut() {
        o.remove("required");
    }
    s["properties"]["items"] = json!({
        "type": "array", "minItems": 1, "maxItems": MAX_ITEMS, "items": item,
        "description": "Several at once: each item takes this tool's arguments; arguments given next to `items` are shared defaults. All items are one undo step, and nothing changes if one fails."
    });
    s
}

fn lookup(world: &World, name: &str) -> Result<McpTool, String> {
    let reg = world
        .get_resource::<McpTools>()
        .ok_or("no MCP tools are registered")?;
    reg.tools
        .iter()
        .find(|t| t.name == name)
        .cloned()
        .ok_or_else(|| {
            let names: Vec<&str> = reg.tools.iter().map(|t| t.name).collect();
            let hint = validate::did_you_mean(name, &names)
                .map(|m| format!(" (did you mean {m}?)"))
                .unwrap_or_default();
            format!("Unknown tool `{name}`{hint}")
        })
}

/// Checks the arguments, then runs the tool (catching panics so a bad call can't take the
/// editor down).
pub fn call_tool(world: &mut World, name: &str, mut args: Value) -> R {
    let tool = lookup(world, name)?;
    validate::check(&schema_of(&tool), &mut args)
        .map_err(|e| format!("Invalid arguments for {name}:\n{e}"))?;
    dispatch(world, &tool, args)
}

/// Runs checked arguments (`items` expanded).
fn dispatch(world: &mut World, tool: &McpTool, args: Value) -> R {
    if ITEM_TOOLS.contains(&tool.name) && args.get("items").is_some_and(|v| !v.is_null()) {
        return run_items(world, tool, args);
    }
    run_guarded(world, tool, args)
}

fn run_guarded(world: &mut World, tool: &McpTool, args: Value) -> R {
    let name = tool.name;
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (tool.run)(world, args)));
    // a tool must leave no undo step open (a panic may have)
    if world.resource::<History>().is_recording() && !world.resource::<History>().in_group() {
        world
            .resource_scope(|world, mut h: Mut<History>| h.commit(world.resource::<ObjectStore>()));
    }
    // edits stay inside the selection while it masks them
    crate::prompt::mask::settle_new_steps(world);
    match res {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            error!("MCP tool {name} panicked: {msg}");
            Err(format!(
                "The tool `{name}` failed internally ({msg}). The editor is still running; check the world with world_info."
            ))
        }
    }
}

// ------------------------------------------------------------------ one undo step for many calls

/// Settings outside the undo history, kept so an atomic batch can put them back.
struct Settings {
    lighting: Option<crate::sky::Lighting>,
    wind: Option<crate::objects::Wind>,
    water_style: Option<crate::water::WaterStyle>,
    amounts: Option<crate::objects::NatureAmounts>,
    layers: Option<crate::terrain::textures::TerrainLayerChoices>,
    parts: Option<crate::surfaces::SurfaceChoices>,
    terrain: Option<(f32, crate::terrain::StyleRules)>,
}

fn snap<T: Resource + Clone>(w: &World) -> Option<T> {
    w.get_resource::<T>().cloned()
}

/// Puts a setting back (only if it changed, so nothing reacts to an unchanged value).
fn put<T: Resource + serde::Serialize>(w: &mut World, v: Option<T>) {
    if let Some(v) = v
        && w.get_resource::<T>()
            .map(|cur| serde_json::to_value(cur).ok())
            != Some(serde_json::to_value(&v).ok())
    {
        w.insert_resource(v);
    }
}

impl Settings {
    fn take(w: &World) -> Self {
        Self {
            lighting: snap(w),
            wind: snap(w),
            water_style: snap(w),
            amounts: snap(w),
            layers: snap(w),
            parts: snap(w),
            terrain: w
                .get_resource::<crate::terrain::Terrain>()
                .map(|t| (t.sea_level, t.style.clone())),
        }
    }
    fn restore(self, w: &mut World) {
        put(w, self.lighting);
        if let Some(v) = self.wind {
            w.insert_resource(v);
        }
        put(w, self.water_style);
        put(w, self.amounts);
        put(w, self.layers);
        put(w, self.parts);
        if let Some((sea, style)) = self.terrain
            && let Some(mut t) = w.get_resource_mut::<crate::terrain::Terrain>()
        {
            if t.sea_level != sea
                || serde_json::to_value(&t.style).ok() != serde_json::to_value(&style).ok()
            {
                t.sea_level = sea;
                t.style = style;
            }
        }
    }
}

/// An open undo group (nested groups join the outer one).
struct Group {
    outer: bool,
    settings: Option<Settings>,
}

impl Group {
    fn open(world: &mut World, keep_settings: bool) -> Group {
        if world.resource::<History>().in_group() {
            return Group {
                outer: false,
                settings: None,
            };
        }
        world.resource_scope(|w, mut h: Mut<History>| h.begin_group(w.resource::<ObjectStore>()));
        Group {
            outer: true,
            settings: keep_settings.then(|| Settings::take(world)),
        }
    }

    fn close(self, world: &mut World, label: String) {
        if self.outer {
            world.resource_scope(|w, mut h: Mut<History>| {
                h.end_group(w.resource::<ObjectStore>(), label)
            });
        }
    }

    /// Undoes everything done since `open` (and puts the settings back). False when this group
    /// is nested in another, which owns the rollback.
    fn rollback(self, world: &mut World) -> bool {
        if !self.outer {
            return false;
        }
        let recorded = world.resource_scope(|w, mut h: Mut<History>| {
            h.end_group(w.resource::<ObjectStore>(), "Rolled back")
        });
        if recorded {
            let _ = world.run_system_cached(undo_rolled_back);
        }
        if let Some(s) = self.settings {
            s.restore(world);
        }
        true
    }
}

fn undo_rolled_back(
    mut history: ResMut<History>,
    mut terrain: ResMut<crate::terrain::Terrain>,
    mut store: ResMut<ObjectStore>,
    mut water: ResMut<crate::water::WaterBodies>,
    mut zones: ResMut<crate::world::Zones>,
    mut gameplay: ResMut<crate::gameplay::Gameplay>,
    mut roads: ResMut<crate::roads::Roads>,
) {
    if history
        .step(
            false,
            &mut terrain,
            &mut store,
            &mut water,
            &mut zones,
            &mut gameplay,
            &mut roads,
        )
        .is_some()
    {
        history.drop_redo();
    }
}

/// A call's output for a summary: text as a string, JSON as itself; images and parts that
/// finish later are passed on.
fn compact(contents: Vec<McpContent>, later: &mut Vec<McpContent>) -> Value {
    let mut parts = vec![];
    for c in contents {
        match c {
            McpContent::Text(t) => parts.push(Value::String(t)),
            McpContent::Json(v) => parts.push(v),
            McpContent::Png(b) => {
                parts.push(json!(format!("image ({} KB, below)", b.len() / 1024)));
                later.push(McpContent::Png(b));
            }
            d @ McpContent::Deferred(_) => {
                parts.push(json!("finishing over the next frames (result below)"));
                later.push(d);
            }
        }
    }
    match parts.len() {
        0 => json!("done"),
        1 => parts.pop().unwrap(),
        _ => Value::Array(parts),
    }
}

fn label_of(name: &str) -> String {
    name.replace('_', " ")
}

/// `items`: the tool once per item (merged over the shared arguments), as one undo step; any
/// failure undoes all of them.
fn run_items(world: &mut World, tool: &McpTool, args: Value) -> R {
    let mut shared = args.as_object().cloned().unwrap_or_default();
    let items = shared
        .remove("items")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let base = (tool.input_schema)();
    let mut merged = vec![];
    for (i, it) in items.iter().enumerate() {
        let mut m: Map<String, Value> = shared.clone();
        if let Some(o) = it.as_object() {
            m.extend(o.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        let mut m = Value::Object(m);
        validate::check(&base, &mut m)
            .map_err(|e| format!("Invalid arguments for {} items[{i}]:\n{e}", tool.name))?;
        merged.push(m);
    }
    let n = merged.len();
    let group = Group::open(world, false);
    let mut results = vec![];
    let mut later = vec![];
    for (i, m) in merged.into_iter().enumerate() {
        match run_guarded(world, tool, m) {
            Ok(c) => results.push(compact(c, &mut later)),
            Err(e) => {
                let note = if group.rollback(world) {
                    format!("nothing was changed (the {i} item(s) before it were undone)")
                } else {
                    "the batch it belongs to decides what is kept".into()
                };
                return Err(format!("{} items[{i}] failed: {e}\n{note}", tool.name));
            }
        }
    }
    group.close(world, format!("{} ×{n}", label_of(tool.name)));
    let mut out = vec![McpContent::Json(
        json!({ "count": n, "results": results, "undo": "one step" }),
    )];
    out.extend(later);
    Ok(out)
}

/// The `batch` tool.
pub(super) fn batch(world: &mut World, args: Value) -> R {
    let calls = args
        .get("calls")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let atomic = args
        .get("atomic")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // everything is checked before anything runs
    let mut plan = vec![];
    for (i, c) in calls.iter().enumerate() {
        let name = c.get("tool").and_then(|t| t.as_str()).unwrap_or_default();
        if NOT_IN_BATCH.contains(&name) {
            return Err(format!(
                "calls[{i}]: `{name}` must run on its own because it changes worlds, steps history, or completes asynchronously"
            ));
        }
        let tool = lookup(world, name).map_err(|e| format!("calls[{i}]: {e}"))?;
        let mut a = c.get("args").cloned().unwrap_or(json!({}));
        validate::check(&schema_of(&tool), &mut a)
            .map_err(|e| format!("calls[{i}] ({name}): invalid arguments:\n{e}\nNothing ran."))?;
        plan.push((tool, a));
    }
    let n = plan.len();
    let group = Group::open(world, atomic);
    let mut results = vec![];
    let mut later = vec![];
    let (mut ok, mut failed) = (0, 0);
    for (i, (tool, a)) in plan.into_iter().enumerate() {
        match dispatch(world, &tool, a) {
            Ok(c) => {
                ok += 1;
                results.push(json!({ "i": i, "tool": tool.name, "ok": true, "result": compact(c, &mut later) }));
            }
            Err(e) => {
                failed += 1;
                results.push(json!({ "i": i, "tool": tool.name, "ok": false, "error": e }));
                if atomic {
                    group.rollback(world);
                    let before: Vec<String> = results[..i]
                        .iter()
                        .map(|r| format!("{} {}", r["i"], r["tool"].as_str().unwrap_or_default()))
                        .collect();
                    return Err(format!(
                        "Batch rolled back: calls[{i}] ({}) failed: {e}\nNothing was changed{}. (Results of calls that finish over later frames, like whole-world generation, can't be rolled back.)",
                        tool.name,
                        if before.is_empty() {
                            String::new()
                        } else {
                            format!("; undone: {}", before.join(", "))
                        }
                    ));
                }
            }
        }
    }
    group.close(world, format!("Batch of {n}"));
    let mut out = vec![McpContent::Json(
        json!({ "calls": n, "ok": ok, "failed": failed, "results": results, "undo": "one step" }),
    )];
    out.extend(later);
    Ok(out)
}

pub(super) fn batch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "calls": {
                "type": "array", "minItems": 1, "maxItems": MAX_CALLS,
                "description": "the calls, run in order",
                "items": {
                    "type": "object",
                    "properties": {
                        "tool": { "type": "string", "description": "tool name" },
                        "args": { "type": "object", "description": "that tool's arguments", "additionalProperties": true }
                    },
                    "required": ["tool"]
                }
            },
            "atomic": { "type": "boolean", "default": false, "description": "stop at the first error and undo everything the batch did (settings like lighting and weather included); otherwise keep going and report each result" }
        },
        "required": ["calls"]
    })
}
