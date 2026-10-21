//! Light indoors: the sky is the only fill light, so inside a walk-in building the floors and
//! walls reflected a blue sky. While the camera stands under a roof (most of a small ring of
//! points around it covered), `Indoors::amount` eases towards 1 and the sky lighting
//! (`sky::update_sun_moon`) turns the sky fill down, adds a warm ambient fill and opens the
//! exposure, the way eyes adjust stepping inside. Sunlight still comes in through doors and
//! windows (it casts shadows).

use bevy::prelude::*;

use super::collide::Colliders;
use crate::camera::MainCamera;
use crate::objects::ObjectStore;
use crate::walk::WalkState;

/// How far inside a building the camera is (0 outdoors … 1 well inside), smoothed.
#[derive(Resource, Default)]
pub struct Indoors {
    pub amount: f32,
    /// how deep in a cave the eye is (0 outside … 1 where no daylight reaches), smoothed: the
    /// exposure opens further (eyes adjusting to the dark), walking or not
    pub cave: f32,
}

/// Points around the eye that must be roofed: the eye itself and a ring 1.6 m out (a porch or a
/// tree crown only covers some of them).
const RING: [Vec2; 7] = [
    Vec2::ZERO,
    Vec2::new(1.6, 0.0),
    Vec2::new(-1.6, 0.0),
    Vec2::new(0.0, 1.6),
    Vec2::new(0.0, -1.6),
    Vec2::new(1.13, 1.13),
    Vec2::new(-1.13, -1.13),
];

#[allow(clippy::too_many_arguments)]
pub(super) fn update_indoors(
    walk: Res<WalkState>,
    colliders: Res<Colliders>,
    store: Res<ObjectStore>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    time: Res<Time<Real>>,
    mut indoors: ResMut<Indoors>,
    caves: Res<crate::caves::render::CaveWorld>,
) {
    let off = std::env::var("WB_NOINDOOR").is_ok();
    let target = match cam.single() {
        // (`WB_NOINDOOR=1`: off, for comparisons)
        Ok(t) if walk.active && !off => {
            let p = t.translation();
            let covered = RING
                .iter()
                .filter(|o| {
                    let q = p + Vec3::new(o.x, 0.0, o.y);
                    colliders.covered(&store, q, 0.15, 12.0)
                        || (caves.in_space(q) && caves.roof_over(q, 12.0).is_some())
                })
                .count();
            // all but one point roofed → fully indoors; half → half
            ((covered as f32 - 3.0) / (RING.len() as f32 - 4.0)).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    // in a cave (walking or not): how little daylight reaches the eye
    let cave_target = match (cam.single(), &caves.light) {
        (Ok(t), Some(l)) if !off && caves.in_space(t.translation()) => {
            (1.0 - l.at(t.translation())).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    // eyes adjust in about a second
    let k = 1.0 - (-time.delta_secs() * 2.5).exp();
    indoors.amount += (target - indoors.amount) * k;
    if indoors.amount < 1e-3 && target == 0.0 {
        indoors.amount = 0.0;
    }
    // (into the dark more slowly than out into the light)
    let kc = 1.0 - (-time.delta_secs() * if cave_target > indoors.cave { 1.2 } else { 3.0 }).exp();
    indoors.cave += (cave_target - indoors.cave) * kc;
    if indoors.cave < 1e-3 && cave_target == 0.0 {
        indoors.cave = 0.0;
    }
}
