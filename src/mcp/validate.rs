//! Checks a call's arguments against the tool's `input_schema` before the tool runs, for every
//! registered tool: unknown keys (with the valid keys and a "did you mean"), wrong types, enum
//! values, required keys and array lengths. Numeric ranges are left to the tools (they clamp to
//! the editor's slider ranges, as the UI does).
//!
//! Supported schema keywords: `type` (one or a list), `enum`, `properties`, `required`,
//! `additionalProperties`, `items`, `minItems`, `maxItems`, `anyOf` / `oneOf`, and the
//! server's own `x-aliases` (object: accepted old argument names with their schemas, not listed
//! to clients: see [`public_schema`]). `null` for an optional key means "not given", as the tools
//! read it.
//!
//! Whole numbers given as floats (`3.0`) are turned into integers where the schema asks for an
//! integer, so tools reading integers accept them.

use serde_json::{Map, Value};

/// At most this many problems are reported per call.
const MAX_ERRORS: usize = 6;

/// Checks (and lightly normalises) `args` against `schema`. The error lists every problem found.
pub fn check(schema: &Value, args: &mut Value) -> Result<(), String> {
    if args.is_null() {
        *args = Value::Object(Map::new());
    }
    let mut errs = vec![];
    walk(schema, args, "", &mut errs);
    if errs.is_empty() {
        return Ok(());
    }
    let more = errs.len().saturating_sub(MAX_ERRORS);
    errs.truncate(MAX_ERRORS);
    let mut msg = errs.join("\n");
    if more > 0 {
        msg += &format!("\n(and {more} more)");
    }
    Err(msg)
}

/// The schema as clients see it (the server-only `x-aliases` removed).
pub fn public_schema(mut schema: Value) -> Value {
    strip(&mut schema);
    schema
}

fn strip(v: &mut Value) {
    match v {
        Value::Object(o) => {
            o.remove("x-aliases");
            for (_, c) in o.iter_mut() {
                strip(c);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(strip),
        _ => {}
    }
}

/// "`items[2].position`" (or "arguments" for the top level).
fn at(path: &str) -> String {
    if path.is_empty() {
        "arguments".into()
    } else {
        format!("`{path}`")
    }
}

fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// Does `v` have JSON-schema type `t`? Whole floats count as integers (and are converted).
fn is_type(v: &mut Value, t: &str) -> bool {
    match (t, &*v) {
        ("integer", Value::Number(n)) => {
            if n.is_i64() || n.is_u64() {
                return true;
            }
            match n.as_f64() {
                Some(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => {
                    *v = if f < 0.0 {
                        Value::from(f as i64)
                    } else {
                        Value::from(f as u64)
                    };
                    true
                }
                _ => false,
            }
        }
        ("number", Value::Number(_)) => true,
        ("string", Value::String(_)) => true,
        ("boolean", Value::Bool(_)) => true,
        ("array", Value::Array(_)) => true,
        ("object", Value::Object(_)) => true,
        ("null", Value::Null) => true,
        _ => false,
    }
}

/// Lower-case letters and digits only: "Quest Hub", "quest_hub" and "QuestHub" all match.
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn enum_ok(allowed: &[Value], v: &Value) -> bool {
    allowed.iter().any(|a| match (a, v) {
        (Value::String(a), Value::String(v)) => a == v || norm(a) == norm(v),
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (a, v) => a == v,
    })
}

fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}

fn walk(schema: &Value, v: &mut Value, path: &str, errs: &mut Vec<String>) {
    let Some(s) = schema.as_object() else { return };
    // alternatives: the first that fits wins (and its normalisation is kept)
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(alts)) = s.get(key) {
            let mut fits = None;
            for alt in alts {
                let mut c = v.clone();
                let mut e = vec![];
                walk(alt, &mut c, path, &mut e);
                if e.is_empty() {
                    fits = Some(c);
                    break;
                }
            }
            match fits {
                Some(c) => *v = c,
                None => {
                    let kinds: Vec<String> = alts.iter().map(describe).collect();
                    errs.push(format!(
                        "{} must be {} (got {})",
                        at(path),
                        kinds.join(" or "),
                        brief(v)
                    ));
                    return;
                }
            }
        }
    }
    match s.get("type") {
        Some(Value::String(t)) => {
            if !is_type(v, t) {
                errs.push(format!(
                    "{} must be {} (got {})",
                    at(path),
                    article(t),
                    brief(v)
                ));
                return;
            }
        }
        Some(Value::Array(ts)) => {
            if !ts.iter().filter_map(|t| t.as_str()).any(|t| is_type(v, t)) {
                let names: Vec<&str> = ts.iter().filter_map(|t| t.as_str()).collect();
                errs.push(format!(
                    "{} must be {} (got {})",
                    at(path),
                    names
                        .iter()
                        .map(|t| article(t))
                        .collect::<Vec<_>>()
                        .join(" or "),
                    brief(v)
                ));
                return;
            }
        }
        _ => {}
    }
    if let Some(Value::Array(allowed)) = s.get("enum")
        && !enum_ok(allowed, v)
    {
        let opts: Vec<String> = allowed.iter().map(show).collect();
        let mean = match v {
            Value::String(x) => {
                did_you_mean(x, &opts.iter().map(String::as_str).collect::<Vec<_>>())
            }
            _ => None,
        };
        errs.push(format!(
            "{}: unknown value {}{}; one of: {}",
            at(path),
            brief(v),
            mean.map(|m| format!(" (did you mean {m}?)"))
                .unwrap_or_default(),
            opts.join(", ")
        ));
        return;
    }
    match v {
        Value::Object(o) => object(s, o, path, errs),
        Value::Array(a) => {
            let n = a.len() as u64;
            if let Some(min) = s.get("minItems").and_then(|m| m.as_u64())
                && n < min
            {
                errs.push(format!(
                    "{} needs at least {min} item{} (got {n})",
                    at(path),
                    if min == 1 { "" } else { "s" }
                ));
            }
            if let Some(max) = s.get("maxItems").and_then(|m| m.as_u64())
                && n > max
            {
                errs.push(format!("{} takes at most {max} items (got {n})", at(path)));
            }
            if let Some(items) = s.get("items") {
                for (i, x) in a.iter_mut().enumerate() {
                    if errs.len() > MAX_ERRORS {
                        return;
                    }
                    walk(items, x, &format!("{path}[{i}]"), errs);
                }
            }
        }
        _ => {}
    }
}

fn object(s: &Map<String, Value>, o: &mut Map<String, Value>, path: &str, errs: &mut Vec<String>) {
    let props = s.get("properties").and_then(|p| p.as_object());
    let aliases = s.get("x-aliases").and_then(|p| p.as_object());
    let extra = s.get("additionalProperties");
    let required: Vec<&str> = s
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|k| k.as_str()).collect())
        .unwrap_or_default();
    for k in &required {
        if o.get(*k).is_none_or(|v| v.is_null()) {
            let what = props
                .and_then(|p| p.get(*k))
                .and_then(|p| p.get("description"))
                .and_then(|d| d.as_str())
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            errs.push(format!("{}: missing required `{k}`{what}", at(path)));
        }
    }
    // an object schema without `properties` takes any keys (unless it says otherwise)
    let closed = match extra {
        Some(Value::Bool(b)) => !b,
        Some(Value::Object(_)) => false,
        _ => props.is_some(),
    };
    let known: Vec<&str> = props
        .map(|p| p.keys().map(String::as_str).collect())
        .unwrap_or_default();
    for (k, val) in o.iter_mut() {
        let sub = child(path, k);
        if let Some(p) = props
            .and_then(|p| p.get(k))
            .or_else(|| aliases.and_then(|a| a.get(k)))
        {
            // null: "not given" (tools read it so) unless the key is required
            if val.is_null() && !required.contains(&k.as_str()) && !accepts_null(p) {
                continue;
            }
            walk(p, val, &sub, errs);
        } else if let Some(x @ Value::Object(_)) = extra {
            walk(x, val, &sub, errs);
        } else if closed {
            errs.push(unknown_key(k, val, props, &known, path));
        }
    }
}

fn accepts_null(schema: &Value) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) => t == "null",
        Some(Value::Array(ts)) => ts.iter().any(|t| t == "null"),
        _ => ["anyOf", "oneOf"].iter().any(|k| {
            schema
                .get(*k)
                .and_then(|a| a.as_array())
                .is_some_and(|a| a.iter().any(accepts_null))
        }),
    }
}

/// "unknown argument `clouds` (did you mean `cloud_preset`? "clear" is one of its values). Valid
/// arguments: …"
fn unknown_key(
    k: &str,
    val: &Value,
    props: Option<&Map<String, Value>>,
    known: &[&str],
    path: &str,
) -> String {
    let what = if path.is_empty() {
        format!("unknown argument `{k}`")
    } else {
        format!("{}: unknown key `{k}`", at(path))
    };
    // a property whose enum holds the given value is the best guess
    let by_value = val.as_str().and_then(|sv| {
        props?
            .iter()
            .find(|(_, p)| {
                p.get("enum")
                    .and_then(|e| e.as_array())
                    .is_some_and(|e| enum_ok(e, &Value::String(sv.into())))
            })
            .map(|(name, _)| name.clone())
    });
    let hint = match (by_value, did_you_mean(k, known)) {
        (Some(p), _) => format!(" (did you mean `{p}`? {} is one of its values)", brief(val)),
        (None, Some(m)) => format!(" (did you mean {m}?)"),
        (None, None) => String::new(),
    };
    let valid = if known.is_empty() {
        "none".to_string()
    } else {
        known.join(", ")
    };
    format!("{what}{hint}. Valid: {valid}")
}

/// Up to three close names, best first: "`a`, `b` or `c`".
pub fn did_you_mean(word: &str, options: &[&str]) -> Option<String> {
    let w = norm(word);
    if w.is_empty() {
        return None;
    }
    let stem = w
        .strip_suffix('s')
        .filter(|s| s.len() >= 3)
        .unwrap_or(&w)
        .to_string();
    let mut scored: Vec<(usize, &str)> = options
        .iter()
        .filter_map(|o| {
            let n = norm(o);
            if n == w {
                return Some((0, *o));
            }
            let d = edit_distance(&w, &n);
            if d <= (w.len().max(n.len()) / 4).max(1) {
                return Some((d, *o));
            }
            // same word stem as one of the option's parts ("clouds" → cloud_cover)
            let parts: Vec<String> = o.split(['_', ' ', '-']).map(norm).collect();
            if stem.len() >= 3
                && (parts.iter().any(|p| {
                    *p == stem
                        || p.starts_with(&stem)
                        || (p.len() >= 4 && stem.starts_with(p.as_str()))
                }) || n.starts_with(&stem))
            {
                return Some((10 + d, *o));
            }
            None
        })
        .collect();
    if scored.is_empty() {
        return None;
    }
    scored.sort_by_key(|(d, o)| (*d, o.len()));
    let names: Vec<String> = scored
        .iter()
        .take(3)
        .map(|(_, o)| format!("`{o}`"))
        .collect();
    Some(match names.len() {
        1 => names[0].clone(),
        _ => format!(
            "{} or {}",
            names[..names.len() - 1].join(", "),
            names[names.len() - 1]
        ),
    })
}

/// Damerau–Levenshtein (optimal string alignment) distance.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=m {
        d[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = (a[i - 1] != b[j - 1]) as usize;
            d[i][j] = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[n][m]
}

fn article(t: &str) -> String {
    match t {
        "integer" | "array" | "object" => format!("an {t}"),
        "null" => "null".into(),
        t => format!("a {t}"),
    }
}

/// A short description of an alternative schema ("an integer", "\"ocean\"", "null"…).
fn describe(s: &Value) -> String {
    if let Some(Value::Array(e)) = s.get("enum") {
        return e
            .iter()
            .map(|v| format!("\"{}\"", show(v)))
            .collect::<Vec<_>>()
            .join(" / ");
    }
    match s.get("type") {
        Some(Value::String(t)) => article(t),
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(|t| t.as_str())
            .map(article)
            .collect::<Vec<_>>()
            .join(" or "),
        _ => "a value".into(),
    }
}

/// The value, shortened for messages.
fn brief(v: &Value) -> String {
    let s = match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Array(a) => format!("an array of {}", a.len()),
        Value::Object(_) => "an object".into(),
        v => v.to_string(),
    };
    if s.chars().count() > 40 {
        s.chars().take(37).collect::<String>() + "…\""
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn weather() -> Value {
        json!({ "type": "object", "properties": {
            "cloud_preset": { "type": "string", "enum": ["clear", "fair_weather", "overcast"] },
            "cloud_cover": { "type": "number", "minimum": 0, "maximum": 1 },
            "cloud_seed": { "type": "integer" },
            "wind_speed": { "type": "number" }
        }})
    }

    #[test]
    fn unknown_key_suggests_by_value_and_name() {
        let e = check(&weather(), &mut json!({ "clouds": "clear" })).unwrap_err();
        assert!(e.contains("unknown argument `clouds`"), "{e}");
        assert!(e.contains("did you mean `cloud_preset`"), "{e}");
        assert!(
            e.contains("Valid: cloud_cover, cloud_preset, cloud_seed, wind_speed"),
            "{e}"
        );
        let e = check(&weather(), &mut json!({ "wind_sped": 3 })).unwrap_err();
        assert!(e.contains("did you mean `wind_speed`"), "{e}");
        let e = check(&weather(), &mut json!({ "cloud": 0.5 })).unwrap_err();
        assert!(e.contains("`cloud_"), "{e}");
    }

    #[test]
    fn types_enums_and_required() {
        let s = json!({ "type": "object", "required": ["kind"], "properties": {
            "kind": { "type": "string", "enum": ["QuestHub", "Town"] },
            "level": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 },
            "on": { "type": "boolean" }
        }});
        // display-name spellings of enum values pass (tools match them loosely)
        assert!(check(&s, &mut json!({ "kind": "quest_hub" })).is_ok());
        let e = check(&s, &mut json!({ "kind": "tavern" })).unwrap_err();
        assert!(
            e.contains("unknown value \"tavern\"") && e.contains("QuestHub, Town"),
            "{e}"
        );
        let e = check(&s, &mut json!({ "kind": "Town", "on": "yes" })).unwrap_err();
        assert!(e.contains("`on` must be a boolean (got \"yes\")"), "{e}");
        let e = check(&s, &mut json!({})).unwrap_err();
        assert!(e.contains("missing required `kind`"), "{e}");
        let e = check(&s, &mut json!({ "kind": "Town", "level": [1, 2, 3] })).unwrap_err();
        assert!(e.contains("at most 2"), "{e}");
        // whole floats become integers; null means "not given"
        let mut a = json!({ "kind": "Town", "level": [3.0, 7.0], "on": null });
        check(&s, &mut a).unwrap();
        assert!(a["level"][0].is_u64());
        let e = check(&s, &mut json!({ "kind": "Town", "level": [1.5, 2] })).unwrap_err();
        assert!(e.contains("`level[0]` must be an integer"), "{e}");
    }

    #[test]
    fn nested_items_aliases_and_alternatives() {
        let s = json!({ "type": "object", "properties": {
            "items": { "type": "array", "items": { "type": "object", "properties": { "asset": { "type": "string" }, "position": { "type": "array" } }, "required": ["asset"] } },
            "body": { "oneOf": [{ "type": "integer" }, { "type": "string", "enum": ["ocean"] }] },
            "params": { "type": "object", "additionalProperties": true }
        }, "x-aliases": { "x": { "type": "number" } } });
        let e = check(
            &s,
            &mut json!({ "items": [{ "asset": "a", "positon": [1, 2] }] }),
        )
        .unwrap_err();
        assert!(
            e.contains("`items[0]`: unknown key `positon` (did you mean `position`?)"),
            "{e}"
        );
        assert!(
            check(
                &s,
                &mut json!({ "x": 3, "body": "ocean", "params": { "anything": 1 } })
            )
            .is_ok()
        );
        assert!(check(&s, &mut json!({ "body": 4 })).is_ok());
        let e = check(&s, &mut json!({ "body": "lake" })).unwrap_err();
        assert!(e.contains("`body` must be an integer or \"ocean\""), "{e}");
        let e = check(&s, &mut json!({ "x": "3" })).unwrap_err();
        assert!(e.contains("`x` must be a number"), "{e}");
        // aliases stay hidden from clients
        assert!(public_schema(s).get("x-aliases").is_none());
    }
}
