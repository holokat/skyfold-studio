//! Effect anchors of library models: the points where engine effects attach to a model. The
//! style packs write them as empty glTF nodes `fx_<kind>_<n>` and list them in their library's
//! `models.json` (`fx`: `kind`, `pos` in model space — glTF, y up — plus the kind's own extras such
//! as `radius`, `height` or `pulse`). This module indexes them per library model and places them in
//! the world for each placed object; effects read it: `auras` turns `aura_dark` anchors (the dark
//! spire's) into a dark aura, `flames` burns the `fire` anchors (braziers, torch stands, fire
//! pits), `visuals::chimneys` smokes the `smoke` anchors (chimneys, forge stacks, cauldrons) and
//! `fixture_lights` lights the `light` anchors (downlights, lanterns, street lights).
//!
//! The index reloads when a `models.json` changes on disk (a pack released while the editor runs).
//! MCP: `effect_anchors` lists the anchored models and the placed objects that carry anchors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::prelude::*;
use serde_json::{Map, Value, json};

use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg, position_arg, position_schema};
use crate::objects::{ObjectStore, Placed};

/// One effect anchor of a model.
#[derive(Clone, Debug, PartialEq)]
pub struct FxAnchor {
    /// "aura_dark", "fire", "smoke"…
    pub kind: String,
    /// where (model space: glTF, y up; metres)
    pub pos: Vec3,
    /// how wide the effect spreads (m; 0: the kind's default)
    pub radius: f32,
    /// how high it reaches (m; 0: the kind's default)
    pub height: f32,
    /// the anchor's other extras (e.g. `pulse`: a material whose glow the effect breathes with)
    pub extras: Map<String, Value>,
}

impl FxAnchor {
    /// Reads one `fx` entry (None without a kind or a position).
    pub fn parse(v: &Value) -> Option<FxAnchor> {
        let o = v.as_object()?;
        let kind = o.get("kind")?.as_str()?.to_string();
        let p = o.get("pos")?.as_array()?;
        let f = |i: usize| p.get(i).and_then(|x| x.as_f64()).map(|x| x as f32);
        let pos = Vec3::new(f(0)?, f(1)?, f(2)?);
        let num = |k: &str| o.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        let extras = o
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "kind" | "pos" | "radius" | "height"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Some(FxAnchor {
            kind,
            pos,
            radius: num("radius"),
            height: num("height"),
            extras,
        })
    }

    /// A string extra (e.g. `pulse`).
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        self.extras.get(key).and_then(|v| v.as_str())
    }
}

/// An anchor placed in the world on a placed object.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldAnchor {
    pub pos: Vec3,
    pub radius: f32,
    pub height: f32,
}

/// Where `a` is on the placed object `p`: turned, sized (and stretched) with it.
pub fn world_anchor(p: &Placed, a: &FxAnchor) -> WorldAnchor {
    let s = p.scale3();
    WorldAnchor {
        pos: p.translation() + p.rotation() * (s * a.pos),
        radius: a.radius * s.x.max(s.z),
        height: a.height * s.y,
    }
}

/// The anchors of every bundled library model, by library id ("hp:dark_spire").
#[derive(Resource, Default)]
pub struct FxAnchors {
    pub by_model: HashMap<String, Vec<FxAnchor>>,
    /// bumped each time the index is rebuilt
    pub revision: u64,
    /// the manifests read and their dates (reloaded when one changes)
    stamps: Vec<(PathBuf, Option<SystemTime>)>,
}

impl FxAnchors {
    /// The anchors of the library model `id` (none for most).
    pub fn of(&self, id: &str) -> &[FxAnchor] {
        self.by_model.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Reads every bundled library's `models.json` under `dir` (`assets/models`).
    pub fn load(dir: &Path) -> FxAnchors {
        let mut out = FxAnchors::default();
        for lib in crate::objects::model_libraries(dir) {
            let path = lib.dir.join("models.json");
            out.stamps.push((path.clone(), modified(&path)));
            let Some(doc) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            else {
                continue;
            };
            for (id, anchors) in manifest_anchors(&doc) {
                out.by_model.insert(format!("{}:{id}", lib.prefix), anchors);
            }
        }
        out
    }

    fn stale(&self) -> bool {
        self.stamps.iter().any(|(p, t)| modified(p) != *t)
    }
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// (model id, anchors) of every model in a `models.json` document that has any.
pub fn manifest_anchors(doc: &Value) -> Vec<(String, Vec<FxAnchor>)> {
    let Some(models) = doc.get("models").and_then(|m| m.as_array()) else {
        return vec![];
    };
    models
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            let fx: Vec<FxAnchor> = m
                .get("fx")?
                .as_array()?
                .iter()
                .filter_map(FxAnchor::parse)
                .collect();
            (!fx.is_empty()).then(|| (id.to_string(), fx))
        })
        .collect()
}

pub struct FxAnchorsPlugin;

impl Plugin for FxAnchorsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(FxAnchors::load(&models_dir()))
            .add_systems(Update, reload);
        app.mcp_tool(McpTool {
            name: "effect_anchors",
            description: "Effect anchors: the points on a model where engine effects attach (the style packs' `fx_<kind>` nodes: `aura_dark` = a dark aura of rising wisps and motes, low mist, a pulsing purple light and a drone with whispers, e.g. on hp:dark_spire; `fire` on braziers, torch stands and fire pits (burning: `fires`), `smoke` on chimneys, forge stacks and cauldrons (smoking: `smoke`), `light` on light fixtures: downlights, cove strips, lanterns, path and street lights (shining: `lights`)). Lists the placed objects that carry anchors near `position` ([x, z]) within `radius` (m, default 250; without a position: the whole world), nearest first (at most `limit`, default 50): object id, model, and per anchor its kind, world position, radius and height. `models: true` also lists every library model with anchors. Read-only; auras are tuned with `auras`.",
            input_schema: || {
                json!({ "type": "object", "properties": {
                    "position": position_schema("centre of the search"),
                    "radius": { "type": "number", "description": "metres (default 250)" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                    "models": { "type": "boolean", "description": "also list the library models that carry anchors" } } })
            },
            run: effect_anchors,
        });
    }
}

fn models_dir() -> PathBuf {
    PathBuf::from(crate::assets_path()).join("models")
}

/// Re-reads the index when a manifest changed (checked every 2 s).
fn reload(mut fx: ResMut<FxAnchors>, time: Res<Time>, mut timer: Local<f32>) {
    *timer -= time.delta_secs();
    if *timer > 0.0 {
        return;
    }
    *timer = 2.0;
    if fx.stale() {
        let rev = fx.revision + 1;
        *fx = FxAnchors::load(&models_dir());
        fx.revision = rev;
        info!("effect anchors reloaded: {} models", fx.by_model.len());
    }
}

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn effect_anchors(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let centre = position_arg(&args, "position")?.map(|(p, _)| p);
    let radius = opt_arg::<f32>(&args, "radius")?.unwrap_or(250.0).max(0.1);
    let limit = opt_arg::<usize>(&args, "limit")?
        .unwrap_or(50)
        .clamp(1, 500);
    let fx = world.resource::<FxAnchors>();
    let store = world.resource::<ObjectStore>();
    let mut rows: Vec<(f32, Value)> = store
        .items
        .values()
        .filter_map(|p| {
            let anchors = fx.of(&p.asset);
            if anchors.is_empty() {
                return None;
            }
            let d = centre.map(|c| c.distance(Vec2::new(p.pos[0], p.pos[2]))).unwrap_or(0.0);
            if d > radius && centre.is_some() {
                return None;
            }
            let list: Vec<Value> = anchors
                .iter()
                .map(|a| {
                    let w = world_anchor(p, a);
                    let mut o = json!({ "kind": a.kind, "position": [r1(w.pos.x), r1(w.pos.y), r1(w.pos.z)], "radius": r1(w.radius), "height": r1(w.height) });
                    for (k, v) in &a.extras {
                        o[k] = v.clone();
                    }
                    o
                })
                .collect();
            Some((d, json!({ "id": p.id, "model": p.asset, "distance": centre.map(|_| r1(d)), "anchors": list })))
        })
        .collect();
    rows.sort_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1["id"].as_u64().cmp(&b.1["id"].as_u64()))
    });
    let total = rows.len();
    let mut out = json!({ "objects": rows.into_iter().take(limit).map(|r| r.1).collect::<Vec<_>>(), "objects_total": total });
    if opt_arg::<bool>(&args, "models")?.unwrap_or(false) {
        let mut models: Vec<(&String, &Vec<FxAnchor>)> = fx.by_model.iter().collect();
        models.sort_by(|a, b| a.0.cmp(b.0));
        out["models"] = Value::Array(models.into_iter().map(|(id, a)| json!({ "model": id, "kinds": a.iter().map(|x| x.kind.clone()).collect::<Vec<_>>() })).collect());
    }
    Ok(vec![McpContent::Json(out)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(pos: [f32; 3], yaw_deg: f32, scale: f32) -> Placed {
        serde_json::from_value(json!({ "id": 1, "asset": "hp:x", "variant": 0, "pos": pos, "rot": Quat::from_rotation_y(yaw_deg.to_radians()).to_array(), "scale": scale })).unwrap()
    }

    #[test]
    fn reads_manifest_anchors() {
        let doc = json!({ "models": [
            { "id": "dark_spire", "fx": [{ "kind": "aura_dark", "pos": [0.0, 0.0, -0.0], "radius": 7.5, "height": 24.0, "pulse": "glow_purple" }] },
            { "id": "plain" },
            { "id": "brazier", "fx": [{ "kind": "fire", "pos": [0.0, 1.2, 0.0], "radius": 0.3, "height": 0.9 }, { "pos": [0, 0, 0] }] },
        ] });
        let m = manifest_anchors(&doc);
        assert_eq!(m.len(), 2);
        let (id, a) = &m[0];
        assert_eq!(
            (id.as_str(), a[0].kind.as_str(), a[0].radius, a[0].height),
            ("dark_spire", "aura_dark", 7.5, 24.0)
        );
        assert_eq!(a[0].extra_str("pulse"), Some("glow_purple"));
        assert!(!a[0].extras.contains_key("radius"));
        // an entry without a kind is skipped
        assert_eq!(m[1].1.len(), 1);
        assert_eq!(m[1].1[0].pos, Vec3::new(0.0, 1.2, 0.0));
    }

    #[test]
    fn anchors_follow_the_placed_object() {
        let a = FxAnchor {
            kind: "fire".into(),
            pos: Vec3::new(1.0, 2.0, 0.0),
            radius: 0.5,
            height: 1.0,
            extras: Map::new(),
        };
        let w = world_anchor(&placed([10.0, 5.0, -3.0], 90.0, 2.0), &a);
        // turned 90° about y: model +x points along world −z; twice the size
        assert!(
            w.pos.distance(Vec3::new(10.0, 9.0, -5.0)) < 1e-4,
            "{:?}",
            w.pos
        );
        assert_eq!((w.radius, w.height), (1.0, 2.0));
        let mut p = placed([0.0; 3], 0.0, 1.0);
        p.set_scale3(Vec3::new(1.0, 3.0, 2.0));
        let w = world_anchor(&p, &a);
        assert!((w.radius - 1.0).abs() < 1e-5 && (w.height - 3.0).abs() < 1e-5);
    }

    #[test]
    fn the_bundled_dark_spire_has_its_aura() {
        let fx = FxAnchors::load(&crate::asset_file("models"));
        if let Some(a) = fx.by_model.get("hp:dark_spire") {
            assert!(
                a.iter()
                    .any(|a| a.kind == "aura_dark" && a.radius > 0.0 && a.height > 0.0)
            );
        }
    }
}
