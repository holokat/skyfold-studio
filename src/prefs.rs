//! Editor preferences shared by every feature: one small JSON object in `worlds/.prefs.json`.
//! Each feature owns a few keys; `set` merges a key into the file and keeps the others.
//! Automated runs (`WB_SHOT` / `WB_FRESH`) read the prefs but never write them.

use std::path::PathBuf;

use serde_json::{Map, Value};

fn path() -> PathBuf {
    let d = crate::io::worlds_dir();
    let _ = std::fs::create_dir_all(&d);
    d.join(".prefs.json")
}

fn read_all() -> Map<String, Value> {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// The stored value of `key`, if any.
pub fn get(key: &str) -> Option<Value> {
    read_all().remove(key)
}

pub fn get_bool(key: &str) -> Option<bool> {
    get(key).and_then(|v| v.as_bool())
}

pub fn get_u64(key: &str) -> Option<u64> {
    get(key).and_then(|v| v.as_u64())
}

/// Stores `key` (merged with the other keys; written atomically).
pub fn set(key: &str, value: Value) {
    if std::env::var("WB_SHOT").is_ok() || std::env::var("WB_FRESH").is_ok() {
        return;
    }
    let mut all = read_all();
    all.insert(key.to_string(), value);
    let p = path();
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, Value::Object(all).to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}
