//! MCP tools for the world's kind (Fantasy / Sci-Fi) and its template.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::{Part, WorldKind};
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::sky::Lighting;

/// Puts the Fantasy defaults back on the chosen parts (what a new fantasy world starts with).
fn fantasy(world: &mut World, parts: &[Part]) -> Vec<String> {
    let mut done = Vec::new();
    let mut l = world.resource_mut::<Lighting>();
    let d = Lighting::default();
    for p in parts {
        match p {
            Part::Atmosphere => {
                l.scifi_look = None;
                l.air = d.air.clone();
                l.look = d.look.clone();
                l.hour = d.hour;
                l.sun_tilt = d.sun_tilt;
                l.haze = d.haze;
                l.cloud_coverage = d.cloud_coverage;
                l.cloud_type = d.cloud_type;
                l.cloud_density = d.cloud_density;
                l.cirrus = d.cirrus;
                l.cloud_sea = d.cloud_sea.clone();
                l.precipitation = d.precipitation;
                done.push("the natural grade, afternoon sun and clouds".into());
            }
            Part::Sky => {
                l.celestial = Default::default();
                done.push("an empty sky with the world's own moon".into());
            }
            Part::Flora | Part::Terrain | Part::Props => {}
        }
    }
    if parts.contains(&Part::Terrain) {
        done.push(crate::terrain::alien_ground::apply(
            world,
            crate::terrain::alien_ground::GroundPalette::Fantasy,
        ));
    }
    if parts.contains(&Part::Flora) {
        world.resource_mut::<crate::nature::NatureAppearance>().alien = Default::default();
        done.push("the world's own plant colours".into());
    }
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    done
}

/// Applies `kind`'s template parts to the open world (and records the kind).
pub fn apply(world: &mut World, kind: WorldKind, parts: &[Part], seed: u32) -> Vec<String> {
    world.resource_mut::<crate::world::WorldMeta>().kind = kind;
    match kind {
        WorldKind::Fantasy => fantasy(world, parts),
        WorldKind::SciFi => super::scifi::apply(world, parts, seed),
    }
}

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "look_scifi_set",
        description: "Put a Sci-Fi look on the world (Style › Look › Sci-Fi): a whole alien mood in one call — the colour grade, the time of day, the clouds and the cloud sea, the bodies in the sky and the plants' colour. \
alien_dusk (ringed giant, cloud sea, crimson scrub), event_horizon (a black hole fills the night), twin_suns (two suns over amber desert), ashfall (volcanic sky, grey ash flora), frozen_moon (ice world, frost), bioluminescent (glowing teal night under a giant), toxic_bloom (acid haze, lime growth), red_planet (butterscotch sky, rust dust), nebula_drift (violet dusk, a crowded sky), giant_dawn (a banded giant at sunrise). \
Everything stays editable (look_set params, lighting_set, weather_set, sky_body_*, alien_flora); look_set with a preset goes back to a Fantasy look. Works in any world; a setting, not an undo step.",
        input_schema: || json!({ "type": "object", "properties": {
            "look": { "type": "string", "enum": super::looks::ScifiLook::ALL.map(|l| l.id()) }
        }, "required": ["look"] }),
        run: |w, a: Value| {
            let look = a
                .get("look")
                .and_then(|v| v.as_str())
                .and_then(super::looks::ScifiLook::from_id)
                .ok_or("unknown `look` (see the list in the description)")?;
            super::looks::apply(w, look);
            Ok(vec![McpContent::Json(json!({ "look": look.id(), "name": look.name(), "about": look.blurb() }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "world_kind_get",
        description: "What kind of world is open: fantasy (forests, rivers, a familiar sky) or scifi (alien canyonlands, a ringed giant overhead, crimson flora, spires and floating rocks), and the template parts world_kind_apply can put on it.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, _| {
            let k = w.resource::<crate::world::WorldMeta>().kind;
            Ok(vec![McpContent::Json(json!({
                "kind": k.id(),
                "name": k.name(),
                "kinds": WorldKind::ALL.map(|k| json!({ "id": k.id(), "about": k.blurb() })),
                "parts": Part::ALL.map(|p| p.id()),
            }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "world_kind_apply",
        description: "Make the open world a fantasy or scifi world and put that kind's template on it — the parts listed (default: atmosphere and sky): atmosphere (grade, time of day, clouds, haze, air), sky (scifi: a ringed giant and a moon, the world's moon hidden; fantasy: an empty sky), terrain (scifi: layered canyonlands carved into the land), props (scifi: spires, floating rocks, monoliths and crystals spread over the land), flora (scifi: crimson alien plant colours). \
Everything it sets stays editable (lighting_set, look_set, sky_body_*, objects tools). A new world of a kind: world_new with `kind`.",
        input_schema: || json!({ "type": "object", "properties": {
            "kind": { "type": "string", "enum": WorldKind::ALL.map(|k| k.id()) },
            "parts": { "type": "array", "items": { "type": "string", "enum": Part::ALL.map(|p| p.id()) }, "description": "template parts to apply (default atmosphere, sky)" },
            "seed": { "type": "integer", "minimum": 0 }
        }, "required": ["kind"] }),
        run: |w, a: Value| {
            let kind = a
                .get("kind")
                .and_then(|v| v.as_str())
                .and_then(WorldKind::from_id)
                .ok_or("`kind` must be fantasy or scifi")?;
            let parts = match a.get("parts").and_then(|v| v.as_array()) {
                Some(list) => list
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .and_then(Part::from_id)
                            .ok_or_else(|| format!("unknown part {v}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                None => vec![Part::Atmosphere, Part::Sky],
            };
            let seed = a
                .get("seed")
                .and_then(|v| v.as_u64())
                .map(|s| s as u32)
                .unwrap_or_else(|| w.resource::<crate::world::WorldMeta>().seed);
            let done = apply(w, kind, &parts, seed);
            Ok(vec![McpContent::Json(json!({ "kind": kind.id(), "done": done }))])
        },
    });
}
