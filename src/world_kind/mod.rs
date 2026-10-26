//! What kind of world it is: Fantasy (every world made before this, and still the default) or
//! Sci-Fi. File › New world picks it; a Sci-Fi world starts as a whole alien template (`scifi`):
//! layered canyonlands, an alien dusk grade and sky, a ringed giant over the horizon, a cloud
//! sea in the canyons, crimson flora, spires, floating rocks, monoliths and crystals. Every part
//! stays editable afterwards, and the parts are tools of their own (sky bodies, canyons,
//! floating rocks…) that work in any world. Saved in world.json as `world_kind`.
//!
//! MCP: `world_kind_get`, `world_kind_apply` (and `world_new`'s `kind`).

pub mod looks;
pub mod mcp;
pub mod scifi;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldKind {
    #[default]
    Fantasy,
    #[serde(rename = "scifi", alias = "sci_fi")]
    SciFi,
}

impl WorldKind {
    pub const ALL: [WorldKind; 2] = [WorldKind::Fantasy, WorldKind::SciFi];

    pub fn name(self) -> &'static str {
        match self {
            WorldKind::Fantasy => "Fantasy",
            WorldKind::SciFi => "Sci-Fi",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            WorldKind::Fantasy => "fantasy",
            WorldKind::SciFi => "scifi",
        }
    }

    pub fn from_id(id: &str) -> Option<WorldKind> {
        match id.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "fantasy" => Some(WorldKind::Fantasy),
            "scifi" | "sciencefiction" | "alien" => Some(WorldKind::SciFi),
            _ => None,
        }
    }

    /// One line for the New world dialog.
    pub fn blurb(self) -> &'static str {
        match self {
            WorldKind::Fantasy => "Continents, forests, rivers and a familiar sky.",
            WorldKind::SciFi => {
                "Alien canyonlands under a ringed giant: spires, floating rocks, crimson flora."
            }
        }
    }
}

/// The parts of a template `world_kind_apply` can put on the open world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Grade, time of day, clouds, haze, air (Lighting / Style).
    Atmosphere,
    /// Planets and moons (Lighting › Sky bodies).
    Sky,
    /// The land: layered canyons, mesas, terraces.
    Terrain,
    /// Spires, floating rocks, monoliths, crystals.
    Props,
    /// Alien plant colours.
    Flora,
}

impl Part {
    pub const ALL: [Part; 5] = [
        Part::Atmosphere,
        Part::Sky,
        Part::Terrain,
        Part::Props,
        Part::Flora,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Part::Atmosphere => "atmosphere",
            Part::Sky => "sky",
            Part::Terrain => "terrain",
            Part::Props => "props",
            Part::Flora => "flora",
        }
    }

    pub fn from_id(id: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|p| p.id() == id)
    }
}

pub struct WorldKindPlugin;

impl bevy::prelude::Plugin for WorldKindPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        use bevy::prelude::*;
        app.init_resource::<TemplateRequest>()
            .add_systems(Update, apply_pending);
        mcp::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_old_worlds_are_fantasy() {
        for k in WorldKind::ALL {
            assert_eq!(WorldKind::from_id(k.id()), Some(k));
        }
        assert_eq!(WorldKind::from_id("Sci-Fi"), Some(WorldKind::SciFi));
        assert_eq!(WorldKind::default(), WorldKind::Fantasy);
        assert_eq!(serde_json::to_string(&WorldKind::SciFi).unwrap(), "\"scifi\"");
        let k: Option<WorldKind> = serde_json::from_str("null").unwrap();
        assert_eq!(k.unwrap_or_default(), WorldKind::Fantasy);
    }
}

/// The kind picked in File › New world, and a template waiting for the new world to be made
/// (applied once the terrain's generation moves past the one it was asked at).
#[derive(bevy::prelude::Resource, Default)]
pub struct TemplateRequest {
    /// the New world dialog's choice
    pub pick: WorldKind,
    /// (kind, terrain generation when asked, seed)
    pub pending: Option<(WorldKind, u64, u32)>,
}

impl TemplateRequest {
    /// Put `kind`'s whole template on the next new world.
    pub fn after_new_world(&mut self, kind: WorldKind, generation: u64, seed: u32) {
        self.pending = Some((kind, generation, seed));
    }
}

/// Once the new world exists (and its land has settled), records its kind and puts the whole
/// template on it.
fn apply_pending(world: &mut bevy::prelude::World) {
    let Some((kind, generation, seed)) = world.resource::<TemplateRequest>().pending else {
        return;
    };
    let now = world.resource::<crate::terrain::Terrain>().generation;
    if now == generation {
        return;
    }
    // a background generator (Continent) may still be building the land
    if world
        .get_resource::<crate::bigworld::BigWorld>()
        .is_some_and(|b| b.busy().is_some())
    {
        return;
    }
    world.resource_mut::<TemplateRequest>().pending = None;
    let done = mcp::apply(world, kind, &Part::ALL, seed);
    if kind != WorldKind::Fantasy {
        world.resource_mut::<crate::editor::Editor>().toast(format!(
            "A new {} world: {}",
            kind.name(),
            done.join(", ")
        ));
    }
}
