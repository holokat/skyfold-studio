//! The Sci-Fi template: an alien canyonland at dusk under a ringed giant (the look of the
//! reference concept). Each part is a plain function over the world so the New world dialog,
//! `world_kind_apply` and the guided setup share it; every value it sets is an ordinary setting
//! the user can change afterwards.

use bevy::prelude::*;

use super::Part;
use super::looks::ScifiLook;

/// `scatter_scifi_props` density over the whole world (1 = about 60 floating rocks, 22
/// monoliths and 5 spires per km²).
const PROP_DENSITY: f32 = 0.3;

/// The look a new Sci-Fi world starts with (the reference concept: a ringed giant over the
/// canyons at dusk, a cloud sea, crimson scrub); the Style tab offers the others.
pub const LOOK: ScifiLook = ScifiLook::AlienDusk;

/// Carves the whole land into layered canyonlands (`terrain::canyons`, the alien preset): stepped
/// plateaus, branching canyons with floors on several tiers, mesas and buttes, banded cliffs,
/// lakes perched on the tiers. One undo step.
fn terrain(world: &mut World, seed: u32) -> Option<String> {
    use bevy::ecs::system::SystemState;
    let mut state: SystemState<(
        ResMut<crate::terrain::Terrain>,
        ResMut<crate::history::History>,
        ResMut<crate::objects::ObjectStore>,
        Option<Res<crate::objects::Library>>,
        ResMut<crate::water::WaterBodies>,
    )> = SystemState::new(world);
    let (mut t, mut h, mut store, lib, mut water) = state.get_mut(world).ok()?;
    let params = crate::terrain::canyons::CanyonParams::alien();
    let (report, lakes) = crate::terrain::canyon_ops::canyonlands(
        &mut t,
        &mut h,
        &mut store,
        lib.as_deref(),
        &mut water,
        &params,
        seed,
        None,
        true,
    )?;
    Some(format!(
        "layered canyonlands ({:.0}–{:.0} m, {} floor tiers, {lakes} perched lakes)",
        report.floor,
        report.top,
        report.tier_heights.len()
    ))
}

/// Grows the Alien Scrub ecosystem (quiver trees, succulents, dry scrub) over the whole land,
/// through the same tool as Nature › Biome brushes (one undo step).
fn scrub(world: &mut World) -> Option<String> {
    let tool = world
        .resource::<crate::mcp_tools::McpTools>()
        .tools
        .iter()
        .find(|t| t.name == "nature_scatter")?
        .clone();
    let half = world.resource::<crate::terrain::Terrain>().size() * 0.5;
    let args = serde_json::json!({
        "mode": "add",
        "preset": "alien_scrub",
        "density": 1.0,
        "area": { "min": [-half, -half], "max": [half, half] },
    });
    match (tool.run)(world, args) {
        Ok(_) => Some("quiver trees and succulents".into()),
        Err(e) => {
            warn!("sci-fi template: alien scrub: {e}");
            None
        }
    }
}

/// Puts the chosen parts of the template on the open world. Returns one line per part done.
pub fn apply(world: &mut World, parts: &[Part], seed: u32) -> Vec<String> {
    let mut done = Vec::new();
    for p in parts {
        match p {
            Part::Atmosphere => {
                LOOK.atmosphere(&mut world.resource_mut::<crate::sky::Lighting>());
                done.push(format!("the {} look", LOOK.name()));
            }
            Part::Sky => {
                LOOK.sky(&mut world.resource_mut::<crate::sky::Lighting>());
                done.push("a ringed giant and a moon in the sky".into());
            }
            Part::Flora => {
                world.resource_mut::<crate::nature::NatureAppearance>().alien = LOOK.flora();
                // no earthly flowers, mushrooms or berries among the alien scrub
                let mut a = world.resource_mut::<crate::objects::NatureAmounts>();
                a.flowers = 0.0;
                a.mushrooms = 0.0;
                a.berries = 0.0;
                done.push("crimson alien flora".into());
                if let Some(line) = scrub(world) {
                    done.push(line);
                }
            }
            Part::Props => {
                // spread over the whole land, thinly: a few spires per square km, floating rocks
                // and monoliths between them (`visuals::scifi_props`)
                let size = world.resource::<crate::terrain::Terrain>().size();
                let ids = crate::visuals::scifi_props::scatter_scifi_props(
                    world,
                    Vec2::ZERO,
                    size * 0.48,
                    PROP_DENSITY,
                    seed as u64,
                );
                done.push(format!(
                    "{} spires, floating rocks and monoliths",
                    ids.len()
                ));
            }
            Part::Terrain => {
                if let Some(line) = terrain(world, seed) {
                    done.push(line);
                }
                // red dust, ochre sand, layered sandstone: no green ground anywhere
                done.push(crate::terrain::alien_ground::ground(world));
            }
        }
    }
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sky::Lighting;

    #[test]
    fn the_giant_stands_away_from_the_sun() {
        let mut l = Lighting::default();
        LOOK.atmosphere(&mut l);
        LOOK.sky(&mut l);
        let sun = crate::sky::sun_direction(l.hour, l.sun_tilt, l.sun_azimuth);
        let giant = l.celestial.bodies[0].direction(l.hour);
        let apart = sun.dot(giant).acos().to_degrees();
        assert!((25.0..120.0).contains(&apart), "{apart}");
        assert!(l.celestial.hide_moon);
        // the sun is still up
        assert!(sun.y > 0.05);
    }
}
