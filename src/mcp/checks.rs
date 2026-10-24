//! Tests over the whole tool registry (every feature's tools) and the call pipeline: names,
//! descriptions that match their schemas, schemas the checker understands, and `batch` /
//! `items` undo grouping and rollback.

use std::collections::{BTreeSet, HashSet};

use bevy::prelude::*;
use serde_json::{Value, json};

use super::validate::{self, norm};
use crate::mcp_tools::{McpContent, McpTool, McpTools};

/// Every tool the editor registers.
fn registry() -> Vec<McpTool> {
    let mut app = App::new();
    super::tools::register(&mut app);
    crate::gameplay::mcp::register(&mut app);
    crate::analysis::mcp::register(&mut app);
    crate::motion::mcp::register(&mut app);
    crate::worldmap::mcp::register(&mut app);
    crate::roads::mcp::register(&mut app);
    crate::refmap::mcp::register(&mut app);
    crate::prompt::tools::register(&mut app);
    crate::thumbs::mcp(&mut app);
    crate::asset_rules::mcp(&mut app);
    crate::market::mcp::register(&mut app);
    crate::scenes::mcp::register(&mut app);
    crate::fields::mcp::register(&mut app);
    crate::characters::mcp::register(&mut app);
    Plugin::build(&crate::export::ExportPlugin, &mut app);
    Plugin::build(&crate::bigworld::BigWorldPlugin, &mut app);
    app.world().resource::<McpTools>().tools.clone()
}

/// Every property name and every enum value (normalised) anywhere in a schema.
fn collect(s: &Value, props: &mut BTreeSet<String>, values: &mut BTreeSet<String>) {
    match s {
        Value::Object(o) => {
            for key in ["properties", "x-aliases"] {
                if let Some(Value::Object(p)) = o.get(key) {
                    props.extend(p.keys().cloned());
                }
            }
            if let Some(Value::Array(e)) = o.get("enum") {
                values.extend(e.iter().filter_map(|v| v.as_str()).map(norm));
            }
            o.values().for_each(|v| collect(v, props, values));
        }
        Value::Array(a) => a.iter().for_each(|v| collect(v, props, values)),
        _ => {}
    }
}

/// Every string enum in a schema: (property path, normalised values).
fn enums(s: &Value, path: &str, out: &mut Vec<(String, Vec<String>)>) {
    let Some(o) = s.as_object() else { return };
    if let Some(Value::Array(e)) = o.get("enum") {
        let v: Vec<String> = e.iter().filter_map(|v| v.as_str()).map(norm).collect();
        if v.len() >= 2 {
            out.push((path.to_string(), v));
        }
    }
    if let Some(Value::Object(p)) = o.get("properties") {
        for (k, v) in p {
            enums(v, k, out);
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(v) = o.get(key) {
            enums(v, path, out);
        }
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(a)) = o.get(key) {
            a.iter().for_each(|v| enums(v, path, out));
        }
    }
}

/// Problems with one (sub)schema: not an object, no type, arrays without items, required keys
/// that aren't properties, empty or repeated enums, aliases shadowing properties.
fn check_schema(s: &Value, path: &str, out: &mut Vec<String>) {
    let Some(o) = s.as_object() else {
        out.push(format!("{path}: schema is not an object"));
        return;
    };
    let typed = ["type", "enum", "anyOf", "oneOf"]
        .iter()
        .any(|k| o.contains_key(*k));
    if !typed {
        out.push(format!("{path}: no type"));
    }
    let types: Vec<&str> = match o.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(|t| t.as_str()).collect(),
        _ => vec![],
    };
    if types.contains(&"array") && !o.contains_key("items") {
        out.push(format!("{path}: array without `items`"));
    }
    if let Some(Value::Array(e)) = o.get("enum") {
        let set: HashSet<String> = e.iter().map(|v| v.to_string()).collect();
        if e.is_empty() || set.len() != e.len() {
            out.push(format!("{path}: empty or repeated enum values"));
        }
    }
    let props = o.get("properties").and_then(|p| p.as_object());
    if types.contains(&"object") && props.is_none() && o.get("additionalProperties").is_none() {
        out.push(format!(
            "{path}: object without `properties` (say `additionalProperties` if any key is fine)"
        ));
    }
    if let Some(Value::Array(req)) = o.get("required") {
        for r in req.iter().filter_map(|r| r.as_str()) {
            if !props.is_some_and(|p| p.contains_key(r)) {
                out.push(format!("{path}: required `{r}` is not a property"));
            }
        }
    }
    if let (Some(Value::Object(a)), Some(p)) = (o.get("x-aliases"), props) {
        for k in a.keys().filter(|k| p.contains_key(*k)) {
            out.push(format!("{path}: alias `{k}` shadows a property"));
        }
    }
    if let Some(p) = props {
        for (k, v) in p {
            check_schema(v, &format!("{path}.{k}"), out);
        }
    }
    if let Some(v @ Value::Object(_)) = o.get("items") {
        check_schema(v, &format!("{path}[]"), out);
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(a)) = o.get(key) {
            for v in a {
                check_schema(v, &format!("{path}|"), out);
            }
        }
    }
}

/// Words written as `code` in a description.
fn backticked(d: &str) -> Vec<String> {
    d.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Lower-case snake words written in double quotes in a description.
fn quoted(d: &str) -> Vec<String> {
    d.split('"')
        .skip(1)
        .step_by(2)
        .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .map(str::to_string)
        .collect()
}

/// Comma-separated lists in a description (three or more items), each item as its words.
fn lists(d: &str) -> Vec<Vec<Vec<String>>> {
    let mut out = vec![];
    for clause in d.split(['.', ';', ':', '(', ')', '—']) {
        let items: Vec<Vec<String>> = clause
            .split(',')
            .map(|i| {
                let i = i
                    .trim()
                    .trim_start_matches("and ")
                    .trim_start_matches("or ")
                    .trim_start_matches("e.g. ");
                i.split_whitespace()
                    .map(|w| {
                        w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                            .to_string()
                    })
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|w: &Vec<String>| !w.is_empty())
            .collect();
        if items.len() >= 3 {
            out.push(items);
        }
    }
    out
}

/// Does a list item end in one of `vals` (its last one to three words, as in "the scene
/// templates epic_sunbeams" or "quest hub")?
fn names_value(item: &[String], vals: &[String]) -> bool {
    (1..=item.len().min(3)).any(|k| vals.contains(&norm(&item[item.len() - k..].concat())))
}

/// Values listed next to an enum's values that the enum doesn't have (short items only: longer
/// ones are prose).
fn check_lists(d: &str, en: &[(String, Vec<String>)], out: &mut Vec<String>) {
    for list in lists(d) {
        for (prop, vals) in en {
            // a list of the enum's values: at least half of its items are values
            let hits = list.iter().filter(|w| names_value(w, vals)).count();
            let extra: Vec<String> = list
                .iter()
                .filter(|w| w.len() <= 2 && !names_value(w, vals))
                .map(|w| w.join(" "))
                .collect();
            if hits >= 2 && hits * 2 >= list.len() && !extra.is_empty() {
                out.push(format!("description lists {extra:?} among the values of `{prop}`, which doesn't have them"));
            }
        }
    }
}

/// Angles follow the compass convention (degrees, 0 = north): no radians, no east-based angles.
fn check_angles(d: &str, what: &str, out: &mut Vec<String>) {
    let l = d.to_lowercase();
    for bad in [
        "radian",
        "0 = east",
        "0 east",
        "0: east",
        "counter-clockwise",
    ] {
        // ("90 east" is fine)
        if l.match_indices(bad)
            .any(|(i, _)| !l[..i].ends_with(|c: char| c.is_ascii_digit()))
        {
            out.push(format!(
                "{what} uses another angle convention (\"{bad}\"); use compass degrees"
            ));
        }
    }
}

/// Every property's own description against its enum, and its angle wording.
fn check_props(s: &Value, path: &str, out: &mut Vec<String>) {
    let Some(o) = s.as_object() else { return };
    if let Some(d) = o.get("description").and_then(|d| d.as_str()) {
        let mut en = vec![];
        if let Some(Value::Array(e)) = o.get("enum") {
            en.push((
                path.to_string(),
                e.iter().filter_map(|v| v.as_str()).map(norm).collect(),
            ));
        }
        check_lists(d, &en, out);
        check_angles(d, path, out);
    }
    if let Some(Value::Object(p)) = o.get("properties") {
        for (k, v) in p {
            check_props(v, &format!("{path}.{k}"), out);
        }
    }
    if let Some(v) = o.get("items") {
        check_props(v, &format!("{path}[]"), out);
    }
}

/// Description problems: values listed next to an enum's values that the enum doesn't have,
/// `code` and "quoted" names that are neither arguments, values nor tools.
fn check_description(t: &McpTool, schema: &Value, tools: &BTreeSet<String>, out: &mut Vec<String>) {
    let d = t.description;
    if d.trim().len() < 30 {
        out.push("description missing or too short".into());
        return;
    }
    let (mut props, mut values) = (BTreeSet::new(), BTreeSet::new());
    collect(schema, &mut props, &mut values);
    let known = |w: &str| props.contains(w) || values.contains(&norm(w)) || tools.contains(w);
    for w in backticked(d) {
        let id = w.split(['[', ' ', '.', '=']).next().unwrap_or(&w);
        if id.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !id.is_empty() && !known(id) {
            out.push(format!(
                "description mentions `{id}`, which is no argument, value or tool"
            ));
        }
    }
    for w in quoted(d) {
        if !known(&w) {
            out.push(format!(
                "description quotes \"{w}\", which is no argument, value or tool"
            ));
        }
    }
    let mut en = vec![];
    enums(schema, "", &mut en);
    check_lists(d, &en, out);
    check_angles(d, "description", out);
    check_props(schema, "args", out);
}

#[test]
fn every_tool_is_named_described_and_schema_checked() {
    let tools = registry();
    assert!(tools.len() >= 80, "only {} tools registered", tools.len());
    let names: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();
    let mut problems = vec![];
    let mut seen = HashSet::new();
    for t in &tools {
        let mut out = vec![];
        if !seen.insert(t.name) {
            out.push("duplicate name".into());
        }
        if t.name.is_empty()
            || !t
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            out.push("name is not snake_case".into());
        }
        let schema = super::calls::schema_of(t);
        if schema.get("type") != Some(&json!("object")) {
            out.push("schema is not {\"type\": \"object\", …}".into());
        }
        check_schema(&schema, "args", &mut out);
        check_description(t, &schema, &names, &mut out);
        // the central check rejects unknown keys for every tool
        match validate::check(&schema, &mut json!({ "no_such_argument": 1 })) {
            Err(e) if e.contains("unknown argument `no_such_argument`") => {}
            other => out.push(format!("an unknown argument isn't rejected ({other:?})")),
        }
        problems.extend(out.into_iter().map(|p| format!("{}: {p}", t.name)));
    }
    for n in super::tools::READ_ONLY
        .iter()
        .chain(super::calls::ITEM_TOOLS)
    {
        if !names.contains(*n) {
            problems.push(format!(
                "{n} is listed as read-only / items but isn't registered"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "{} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn the_checker_catches_stale_descriptions() {
    let t = McpTool {
        name: "look_test",
        description: "A `preset` regrades the world (natural, nordic, cinematic, painted, storybook, toon…). `strengh` blends; \"follow_zones\" too.",
        input_schema: || json!({ "type": "object", "properties": { "preset": { "type": "string", "enum": ["natural", "nordic", "cinematic"] }, "strength": { "type": "number" }, "yaw": { "type": "number", "description": "heading in radians" } } }),
        run: |_, _| Ok(vec![]),
    };
    let mut out = vec![];
    check_description(&t, &(t.input_schema)(), &BTreeSet::new(), &mut out);
    let all = out.join("\n");
    assert!(
        all.contains("painted") && all.contains("storybook") && all.contains("toon"),
        "{all}"
    );
    assert!(all.contains("`strengh`"), "{all}");
    assert!(all.contains("\"follow_zones\""), "{all}");
    assert!(
        all.contains("args.yaw uses another angle convention"),
        "{all}"
    );
    let mut out = vec![];
    check_schema(
        &json!({ "type": "object", "properties": { "pts": { "type": "array" }, "x": {} }, "required": ["y"] }),
        "args",
        &mut out,
    );
    assert_eq!(out.len(), 3, "{out:?}");
}

/// Agents can't buy, unlock or sign in to asset packs: the only market tool reads, and no tool
/// is named for, or takes arguments for, purchases, entitlements, tokens, the marketplace
/// address or the developer unlock. (The market's state itself only changes through private
/// functions in `crate::market`, which no tool calls.)
#[test]
fn no_tool_can_change_asset_pack_entitlements() {
    let tools = registry();
    let market: Vec<&str> = tools
        .iter()
        .map(|t| t.name)
        .filter(|n| n.starts_with("market"))
        .collect();
    assert_eq!(market, ["market_packs", "market_listings"]);
    assert!(market.iter().all(|t| super::tools::READ_ONLY.contains(t)));
    const WORDS: [&str; 12] = [
        "purchase",
        "buy",
        "unlock",
        "entitle",
        "owned",
        "license",
        "licence",
        "token",
        "market_url",
        "marketplace",
        "developer",
        "checkout",
    ];
    let mut problems = vec![];
    for t in &tools {
        if let Some(w) = WORDS.iter().find(|w| t.name.contains(*w)) {
            problems.push(format!("{}: named for `{w}`", t.name));
        }
        let (mut props, mut values) = (BTreeSet::new(), BTreeSet::new());
        collect(&super::calls::schema_of(t), &mut props, &mut values);
        for p in props.iter().chain(&values) {
            if let Some(w) = WORDS.iter().find(|w| p.to_lowercase().contains(*w)) {
                problems.push(format!("{}: takes `{p}` ({w})", t.name));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

// ------------------------------------------------------------------ batch and items

fn test_world() -> World {
    let mut w = World::new();
    w.insert_resource(crate::history::History::default());
    w.insert_resource(crate::objects::ObjectStore::default());
    w.insert_resource(crate::terrain::Terrain::new(64, 4.0, 0.0));
    w.insert_resource(crate::water::WaterBodies::default());
    w.insert_resource(crate::world::Zones::default());
    w.insert_resource(crate::gameplay::Gameplay::default());
    w.insert_resource(crate::roads::Roads::default());
    w.insert_resource(crate::sky::Lighting::default());
    let tools = vec![
        McpTool {
            name: "batch",
            description: "test batch",
            input_schema: super::calls::batch_schema,
            run: super::calls::batch,
        },
        McpTool {
            name: "test_place",
            description: "places a test object at `x`",
            input_schema: || json!({ "type": "object", "properties": { "x": { "type": "number" } }, "required": ["x"] }),
            run: |w, a| {
                let x = a["x"].as_f64().unwrap() as f32;
                w.resource_scope(|w, mut h: Mut<crate::history::History>| {
                    let mut store = w.resource_mut::<crate::objects::ObjectStore>();
                    h.begin("Place test");
                    let id = store.new_id();
                    h.touch_object(&store, id);
                    store.insert(crate::objects::Placed {
                        group: 0,
                        id,
                        asset: "t".into(),
                        variant: 0,
                        pos: [x, 0.0, 0.0],
                        rot: [0.0, 0.0, 0.0, 1.0],
                        scale: 1.0,
                        stretch: None,
                        locked: false,
                    });
                    h.commit(&store);
                });
                Ok(vec![McpContent::Text(format!("placed at {x}"))])
            },
        },
        McpTool {
            name: "test_hour",
            description: "sets the hour (a setting outside undo)",
            input_schema: || json!({ "type": "object", "properties": { "hour": { "type": "number" } } }),
            run: |w, a| {
                w.resource_mut::<crate::sky::Lighting>().hour = a["hour"].as_f64().unwrap() as f32;
                Ok(vec![])
            },
        },
        McpTool {
            name: "test_fail",
            description: "always fails",
            input_schema: || json!({ "type": "object", "properties": {} }),
            run: |_, _| Err("it failed".into()),
        },
    ];
    w.insert_resource(McpTools { tools });
    w
}

fn undo(
    mut history: ResMut<crate::history::History>,
    mut terrain: ResMut<crate::terrain::Terrain>,
    mut store: ResMut<crate::objects::ObjectStore>,
    mut water: ResMut<crate::water::WaterBodies>,
    mut zones: ResMut<crate::world::Zones>,
    mut gameplay: ResMut<crate::gameplay::Gameplay>,
    mut roads: ResMut<crate::roads::Roads>,
) {
    history.step(
        false,
        &mut terrain,
        &mut store,
        &mut water,
        &mut zones,
        &mut gameplay,
        &mut roads,
    );
}

fn objects(w: &World) -> usize {
    w.resource::<crate::objects::ObjectStore>().count()
}

#[test]
fn batch_is_one_undo_step_and_atomic_batches_roll_back() {
    let mut w = test_world();
    let hour = w.resource::<crate::sky::Lighting>().hour;
    // atomic: the failure undoes the objects and the setting, and leaves no undo or redo step
    let calls = json!({ "atomic": true, "calls": [
        { "tool": "test_place", "args": { "x": 1 } }, { "tool": "test_hour", "args": { "hour": 3.0 } },
        { "tool": "test_place", "args": { "x": 2 } }, { "tool": "test_fail" }, { "tool": "test_place", "args": { "x": 3 } } ] });
    let e = super::call_tool(&mut w, "batch", calls)
        .err()
        .expect("atomic batch must fail");
    assert!(e.contains("rolled back") && e.contains("calls[3]"), "{e}");
    assert_eq!(objects(&w), 0);
    assert_eq!(w.resource::<crate::sky::Lighting>().hour, hour);
    let h = w.resource::<crate::history::History>();
    assert!(!h.can_undo() && !h.can_redo() && !h.in_group() && !h.is_recording());

    // not atomic: keeps going; everything that worked is a single undo step
    let calls = json!({ "calls": [{ "tool": "test_place", "args": { "x": 1 } }, { "tool": "test_fail" }, { "tool": "test_place", "args": { "x": 2 } }] });
    let out = super::call_tool(&mut w, "batch", calls).expect("batch runs");
    let McpContent::Json(v) = &out[0] else {
        panic!("json summary expected")
    };
    assert_eq!(
        (v["ok"].as_u64(), v["failed"].as_u64()),
        (Some(2), Some(1)),
        "{v}"
    );
    assert_eq!(objects(&w), 2);
    assert_eq!(
        w.resource::<crate::history::History>().undo_label(),
        Some("Batch of 3")
    );
    let _ = w.run_system_cached(undo);
    assert_eq!(objects(&w), 0, "one undo takes back the whole batch");
    assert!(!w.resource::<crate::history::History>().can_undo());

    // arguments are checked before anything runs
    let calls = json!({ "calls": [{ "tool": "test_place", "args": { "x": 1 } }, { "tool": "test_place", "args": { "y": 1 } }] });
    let e = super::call_tool(&mut w, "batch", calls).err().unwrap();
    assert!(
        e.contains("calls[1]") && e.contains("unknown argument `y`") && e.contains("Nothing ran"),
        "{e}"
    );
    assert_eq!(objects(&w), 0);
    let e = super::call_tool(
        &mut w,
        "batch",
        json!({ "calls": [{ "tool": "test_plac" }] }),
    )
    .err()
    .unwrap();
    assert!(e.contains("did you mean `test_place`"), "{e}");
}

#[test]
fn items_run_as_one_step_and_fail_as_a_whole() {
    let mut w = test_world();
    let tool = McpTool {
        name: "zones_create",
        ..w.resource::<McpTools>().tools[1].clone()
    };
    w.resource_mut::<McpTools>().tools.push(tool);
    let out = super::call_tool(
        &mut w,
        "zones_create",
        json!({ "items": [{ "x": 1 }, { "x": 2 }, { "x": 3 }] }),
    )
    .expect("items run");
    let McpContent::Json(v) = &out[0] else {
        panic!()
    };
    assert_eq!(v["count"], 3);
    assert_eq!(objects(&w), 3);
    assert_eq!(
        w.resource::<crate::history::History>().undo_label(),
        Some("zones create ×3")
    );
    // a missing required argument in one item stops all of them before anything runs
    let e = super::call_tool(&mut w, "zones_create", json!({ "items": [{ "x": 1 }, {}] }))
        .err()
        .unwrap();
    assert!(
        e.contains("items[1]") && e.contains("missing required `x`"),
        "{e}"
    );
    assert_eq!(objects(&w), 3);
}
