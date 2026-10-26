//! Ground-level visuals: how the world looks walking in it.
//!
//! * `ecotone`: natural borders between biomes (wandering outlines, mixed vegetation bands); the
//!   terrain shader interleaves the grounds across them and lays snow naturally.
//! * `shore`: the shore map (nearest water per spot) behind wet sand, swash and surf on beaches,
//!   lapped lake shores, damp gravelly river banks and the charred rims of lava.
//! * `volcano`: lava glows on its surroundings and smokes (ash columns, heat wisps, steam).
//! * `formations`: the procedural fallback of the rock pinnacles, hoodoos and sea stacks (which
//!   are assembled from photoscans); glowing crystals.
//! * `motes`: drifting points of light in eerie and magical places, fireflies over warm swamps.
//! * `air`: what sunlit air is full of (dust glinting in sunbeams, pollen, falling leaves and
//!   petals, ash and embers), by the place, the hour and the weather.
//! * `chimneys`: smoke rising from the placed models' chimneys, forges and cauldrons.
//! * `scifi_props`: alien-world rocks — `floating` rocks hanging in the air, `monoliths`, giant
//!   spires — in a `rock_material` that keeps their grain at any size.

use bevy::prelude::*;

use crate::terrain::TerrainSystems;
use crate::terrain::material::{TerrainGpu, TerrainMaterial};

pub mod air;
pub mod chimneys;
pub mod ecotone;
pub mod floating;
pub mod formations;
mod hooks;
pub mod monoliths;
pub mod motes;
pub mod rock_material;
pub mod scifi_props;
pub mod shore;
mod volcano;

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<volcano::PlumeMaterial>::default(),
            MaterialPlugin::<motes::MoteMaterial>::default(),
        ))
        .add_plugins((
            air::AirPlugin,
            chimneys::ChimneysPlugin,
            scifi_props::ScifiPropsPlugin,
        ))
        .init_resource::<shore::ShoreMap>()
        .init_resource::<volcano::VolcanoState>()
        .add_systems(Startup, motes::spawn_motes)
        .add_systems(
            Update,
            (
                hooks::vis_seq,
                volcano::rebuild_volcanoes,
                volcano::animate_volcanoes,
                motes::update_motes,
            ),
        )
        .add_systems(
            PostUpdate,
            (shore::update_shore_map, terrain_uniforms).before(TerrainSystems::Gpu),
        );
    }
}

/// The sea state and wind the ground's shading follows (surf on beaches, snow streaks).
fn terrain_uniforms(
    gpu: Option<Res<TerrainGpu>>,
    mut mats: ResMut<Assets<TerrainMaterial>>,
    sea: Res<crate::ocean::OceanState>,
    ocean: Res<crate::water::WaterStyle>,
    wind: Res<crate::objects::Wind>,
) {
    let Some(gpu) = gpu else { return };
    let v = Vec4::new(sea.state.hs(), ocean.shore_foam, wind.dir.x, wind.dir.y);
    if mats
        .get(&gpu.material)
        .is_some_and(|m| m.extension.params.visuals != v)
        && let Some(mut m) = mats.get_mut(&gpu.material)
    {
        m.extension.params.visuals = v;
    }
}

/// Ambience loops the water and lava around the listener play (loaded by `audio`).
pub const SOUND_STEMS: [&str; 3] = ["amb_near_waterfall_0", "amb_water_0", "amb_fire_0"];

/// What the listener at `p` hears from nearby water: the roar of a waterfall, the rush of a river,
/// the hiss and crackle of lava. (stem, volume 0..1) pairs, loudest within a few tens of metres.
pub fn near_water_sounds(water: &crate::water::WaterBodies, p: Vec3) -> Vec<(&'static str, f32)> {
    use crate::noise::smoothstep;
    use crate::water::WaterBehaviour;
    let mut fall = 0.0f32;
    let mut river = 0.0f32;
    let mut lava = 0.0f32;
    for f in &water.waterfalls {
        let d = f
            .distance_xz(p.xz())
            .max((p.y - f.top().y).max(f.bottom().y - p.y));
        fall = fall.max(
            (1.0 - smoothstep(8.0, 90.0 + 40.0 * f.flow, d)) * (0.55 + 0.25 * f.flow.min(2.0)),
        );
    }
    for r in &water.rivers {
        let near = r
            .points
            .iter()
            .map(|q| Vec3::from_array(*q).distance(p))
            .fold(f32::MAX, f32::min);
        let edge = r.width * 0.5;
        let v = 1.0 - smoothstep(edge + 2.0, edge + 55.0, near);
        if r.style.behaviour == WaterBehaviour::Molten {
            lava = lava.max(v * 0.8);
        } else {
            river = river.max(v * (0.3 + 0.25 * r.style.flow.min(2.0)));
        }
    }
    for l in water
        .lakes
        .iter()
        .filter(|l| l.style.behaviour == WaterBehaviour::Molten)
    {
        let c = if l.outline.is_empty() {
            Vec2::from_array(l.seed)
        } else {
            l.outline.iter().map(|q| Vec2::from_array(*q)).sum::<Vec2>() / l.outline.len() as f32
        };
        lava = lava
            .max((1.0 - smoothstep(30.0, 160.0, Vec3::new(c.x, l.level, c.y).distance(p))) * 0.9);
    }
    [
        ("amb_near_waterfall_0", fall),
        ("amb_water_0", river),
        ("amb_fire_0", lava),
    ]
    .into_iter()
    .filter(|s| s.1 > 0.001)
    .collect()
}
