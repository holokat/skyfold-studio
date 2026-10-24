//! The market over MCP: `market_packs` lists the packs (read-only: no tool buys, unlocks,
//! signs in or flips the developer unlock), and `locked_packs` sums up what an
//! `objects_library` listing left out.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::{Market, Pack, PackState};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::objects::{Library, LibraryEntry};

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "market_packs",
        description: "The asset packs: each pack's id, name, style, publisher, description, price (null when free), status for this account (free, owned or locked), how many assets it has and its free sample asset ids (usable even while the pack is locked). \
Assets of a locked pack can't be placed or previewed (objects_library lists only usable ones). Read-only: the user gets packs in the editor (Objects › Asset packs), and no tool can buy or unlock one, so when a locked pack would suit, tell the user what it offers and what it costs. Filter by `style`.",
        input_schema: || json!({ "type": "object", "properties": { "style": { "type": "string", "description": "only packs of this style, e.g. photoreal or handpainted" } } }),
        run: packs,
    });
    app.mcp_tool(McpTool {
        name: "market_listings",
        description: "Community listings published on skyfoldstudio.com (the Skyfold marketplace website): each listing's id, title, kind (asset: one model, pack: several), category, price (null when free), creator, rating and its web page. \
Read-only: the user gets listings in the editor (Objects › Asset packs › Community); no tool can acquire one, so when a listing would suit, tell the user its title, price and page. `q` searches titles and descriptions; `cursor` (from next_cursor) continues.",
        input_schema: || json!({ "type": "object", "properties": {
            "q": { "type": "string", "description": "search words" },
            "cursor": { "type": "string", "description": "next_cursor of the previous page" },
        } }),
        run: listings,
    });
}

/// `market_listings`: fetched off the main thread, answered when it arrives.
fn listings(_world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let q = crate::mcp_tools::opt_arg::<String>(&args, "q")?.unwrap_or_default();
    let cursor = crate::mcp_tools::opt_arg::<String>(&args, "cursor")?;
    let slot: std::sync::Arc<std::sync::Mutex<Option<Result<Vec<McpContent>, String>>>> =
        Default::default();
    let fill = slot.clone();
    std::thread::spawn(move || {
        let base = super::site::base();
        let r = super::site::listings(&base, &q, cursor.as_deref()).map(|(l, next)| {
            let list: Vec<Value> = l
                .iter()
                .map(|l| {
                    json!({
                        "id": l.id, "title": l.title, "kind": if l.kind.is_empty() { "asset" } else { l.kind.as_str() }, "category": l.category,
                        "description": l.description, "price": if l.is_free() { Value::Null } else { json!({ "label": l.price_label(), "amount_minor": l.price_cents, "currency": l.currency }) },
                        "creator": l.by(), "official": l.official(), "rating": if l.rating_count > 0 { json!({ "average": l.rating_average, "count": l.rating_count }) } else { Value::Null },
                        "page": l.page_url(&base),
                    })
                })
                .collect();
            let note = if list.is_empty() { "No listings match (or none are published yet)." } else { "Listings are added in the editor by the user (Objects › Asset packs › Community); paid ones need their Skyfold sign-in." };
            vec![McpContent::Json(json!({ "listings": list, "next_cursor": next, "note": note }))]
        });
        *fill.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
    });
    Ok(vec![McpContent::deferred(move |_| {
        slot.lock().unwrap_or_else(|p| p.into_inner()).take()
    })])
}

fn state_id(s: PackState) -> &'static str {
    match s {
        PackState::Free => "free",
        PackState::Owned => "owned",
        PackState::Locked => "locked",
    }
}

fn price_json(p: &Pack) -> Value {
    match &p.price {
        Some(pr) if !p.unlisted => {
            json!({ "label": pr.label(), "amount_minor": pr.amount_minor, "currency": pr.currency })
        }
        _ => Value::Null,
    }
}

/// `market_packs`.
fn packs(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let style = crate::mcp_tools::opt_arg::<String>(&args, "style")?.map(|s| s.to_lowercase());
    let m = super::snapshot();
    let lib = world.get_resource::<Library>();
    let list: Vec<Value> = m
        .packs()
        .iter()
        .filter(|p| style.as_deref().is_none_or(|s| p.style == s))
        .map(|p| {
            let state = m.state(p);
            let mut v = json!({
                "id": p.id, "name": p.name, "style": p.style, "style_name": crate::objects::pack_label(&p.style), "publisher": p.publisher,
                "description": p.description, "price": price_json(p), "status": state_id(state), "assets": p.assets(),
            });
            if state == PackState::Locked {
                // only the samples that exist here
                let samples: Vec<&String> = p.free_assets.iter().filter(|a| lib.is_none_or(|l| l.get(a).is_some())).collect();
                v["free_samples"] = json!(samples);
            }
            if p.unlisted {
                v["note"] = json!("installed, but not for sale yet");
            }
            v
        })
        .collect();
    let mut out = json!({ "packs": list, "note": "status: free (anyone), owned (this account; usable), locked (only its free_samples are usable). Purchases happen in the editor, not over MCP." });
    if let Some(e) = &m.error {
        out["marketplace_problem"] = json!(e);
    }
    Ok(vec![McpContent::Json(out)])
}

/// The packs with assets an `objects_library` listing left out because they're locked: pack,
/// name, style, price and how many of its assets matched the listing's filters.
pub fn locked_packs<'a>(m: &Market, entries: impl Iterator<Item = &'a LibraryEntry>) -> Vec<Value> {
    let mut counts: Vec<(usize, usize)> = vec![];
    for e in entries {
        if m.usable(&e.id) {
            continue;
        }
        let Some(i) = m.pack_index_of(&e.id) else {
            continue;
        };
        match counts.iter_mut().find(|c| c.0 == i) {
            Some(c) => c.1 += 1,
            None => counts.push((i, 1)),
        }
    }
    counts
        .into_iter()
        .map(|(i, n)| {
            let p = &m.packs()[i];
            json!({ "pack": p.id, "name": p.name, "style": p.style, "price": p.price_label(), "locked_assets": n })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_packs_reads_and_changes_nothing() {
        let mut w = World::new();
        let state = || {
            (
                super::super::snapshot().owned.clone(),
                super::super::snapshot().dev(),
            )
        };
        let before = state();
        // even arguments that ask for it change nothing (the tool reads)
        let out = packs(
            &mut w,
            json!({ "owned": ["skyfold_studio"], "unlock": true, "dev": true, "token": "x" }),
        )
        .expect("lists");
        let McpContent::Json(v) = &out[0] else {
            panic!("json expected")
        };
        let hp = v["packs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "skyfold_studio")
            .expect("the hand-painted pack");
        assert_eq!(
            (hp["status"].as_str(), hp["price"]["label"].as_str()),
            (Some("locked"), Some("$49.99"))
        );
        assert_eq!(hp["free_samples"].as_array().map(|a| a.len()), Some(10));
        let ph = v["packs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "polyhaven_scans")
            .unwrap();
        assert_eq!(
            (ph["status"].as_str(), &ph["price"]),
            (Some("free"), &Value::Null)
        );
        assert_eq!(before, state());
        // style filter
        let out = packs(&mut w, json!({ "style": "handpainted" })).unwrap();
        let McpContent::Json(v) = &out[0] else {
            panic!()
        };
        assert!(
            v["packs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["style"] == "handpainted")
        );
    }
}
