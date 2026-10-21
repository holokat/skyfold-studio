//! Ships ride the waves: every placed object that `bobs()` (ships and boats, not piers or docks)
//! samples the water surface under its bow, stern and both sides each frame and heaves, pitches
//! and rolls towards it, damped like a heavy hull (bigger ships move less and slower). Only the
//! render entity moves — the placed object keeps its saved position, so saves, undo and the walk
//! colliders are untouched. While the world holds still (the editor overview without motion
//! preview) ships sit level at their placed height.

use bevy::prelude::*;

use crate::objects::{Library, ObjectStore, PlacedObject};
use crate::ocean::OceanWaves;
use crate::terrain::Terrain;
use crate::water::{WaterBodies, WaterStyle};

/// The hull's current motion (smoothed).
#[derive(Component, Default)]
pub struct Afloat {
    heave: f32,
    /// tilt as (pitch about world x, roll about world z), radians
    tilt: Vec2,
}

/// Ships further than this from the camera don't move (nobody can see the difference).
const NEAR: f32 = 900.0;

#[allow(clippy::too_many_arguments)]
pub(super) fn ride_waves(
    mut commands: Commands,
    motion: Res<crate::wind::Motion>,
    time: Res<Time>,
    terrain: Res<Terrain>,
    water: Res<WaterBodies>,
    style: Res<WaterStyle>,
    waves: Res<OceanWaves>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut q: Query<(Entity, &PlacedObject, &mut Transform, Option<&mut Afloat>)>,
) {
    let Some(lib) = lib else { return };
    let eye = cam.single().map(|c| c.translation()).unwrap_or(Vec3::ZERO);
    let dt = time.delta_secs().min(0.1);
    for (e, po, mut tf, afloat) in &mut q {
        let Some(p) = store.items.get(&po.id) else {
            continue;
        };
        let Some(entry) = lib.get(&p.asset) else {
            continue;
        };
        if !entry.bobs() {
            continue;
        }
        let base_t = p.translation();
        let base_r = p.rotation();
        if !motion.live || base_t.distance(eye) > NEAR {
            // level at the placed height
            if afloat.is_some() {
                tf.translation = base_t;
                tf.rotation = base_r;
                commands.entity(e).remove::<Afloat>();
            }
            continue;
        }
        let Some(mut a) = afloat else {
            commands.entity(e).insert(Afloat::default());
            continue;
        };
        // hull half-extents from the footprint, along the ship's own axes
        let reach = (entry.radius * p.scale * 0.75).max(1.0);
        let fwd = (base_r * Vec3::Z).xz().normalize_or(Vec2::Y) * reach;
        let side = Vec2::new(-fwd.y, fwd.x);
        let c = base_t.xz();
        let h = |o: Vec2| {
            crate::motion::waves::surface_at(&terrain, &water, &style, &waves, c + o, &time)
        };
        let (Some(h0), Some(hf), Some(hb), Some(hl), Some(hr)) =
            (h(Vec2::ZERO), h(fwd), h(-fwd), h(side), h(-side))
        else {
            continue;
        };
        // the hull averages the waves under it: heave to the mean, tilt to the plane through them
        let mean = (h0 * 2.0 + hf + hb + hl + hr) / 6.0;
        let target_heave = (mean - base_t.y).clamp(-3.0, 3.0);
        let slope_f = (hf - hb) / (2.0 * reach);
        let slope_s = (hl - hr) / (2.0 * reach);
        // world-space gradient of the surface → a tilt about the world axes; big hulls respond less
        let damp = (8.0 / reach).clamp(0.25, 1.0);
        let fn_ = fwd.normalize();
        let sn = side.normalize();
        let grad = fn_ * slope_f + sn * slope_s;
        let max_tilt = 7f32.to_radians();
        let target_tilt = Vec2::new((grad.y * damp).atan(), (-grad.x * damp).atan())
            .clamp(Vec2::splat(-max_tilt), Vec2::splat(max_tilt));
        // heavy hulls: slow, smooth response
        let kh = 1.0 - (-dt * 1.6 * damp.sqrt()).exp();
        let kt = 1.0 - (-dt * 1.2 * damp.sqrt()).exp();
        let (heave, tilt0) = (a.heave, a.tilt);
        a.heave = heave + (target_heave - heave) * kh;
        a.tilt = tilt0 + (target_tilt - tilt0) * kt;
        let tilt = Quat::from_rotation_x(-a.tilt.x) * Quat::from_rotation_z(a.tilt.y);
        tf.translation = base_t + Vec3::Y * a.heave;
        tf.rotation = tilt * base_r;
    }
}
