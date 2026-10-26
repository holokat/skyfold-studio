//! Fish: click on any water to release a school. Fish swim in wandering loops inside the
//! water (never onto land), at the depth their species likes, are visible through clear water,
//! and are exported as fishing spots. The models and paintings are the user's Emberfall fish
//! (`assets/models/fish/`, see `species.rs`), swimming with a vertex-shader body wave (`model.rs`).

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::terrain::Terrain;
use crate::water::WaterBodies;

pub mod model;
mod species;

pub use model::{FishMaterial, FishModels, spawn_fish, swim_tag};
pub use species::{FishSpecies, Water};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FishSchool {
    pub id: u32,
    pub species: FishSpecies,
    /// xz centre, y = water surface
    pub center: [f32; 3],
    pub radius: f32,
    pub count: u32,
}

impl FishSchool {
    /// Fits the school's radius inside the water around `center`.
    pub fn new(
        id: u32,
        species: FishSpecies,
        center: Vec3,
        count: u32,
        radius: f32,
        t: &Terrain,
        w: &WaterBodies,
    ) -> Self {
        let mut fit = radius;
        for a in 0..16 {
            let ang = a as f32 / 16.0 * std::f32::consts::TAU;
            let dir = Vec2::new(ang.cos(), ang.sin());
            let mut r = 0.0;
            while r < radius {
                let p = center.xz() + dir * (r + 1.0);
                let g = t.height_at(p.x, p.y);
                if w.surface_at(Vec3::new(p.x, g, p.y), t.sea_level) - g < 0.7 {
                    break;
                }
                r += 1.0;
            }
            fit = fit.min(r);
        }
        Self {
            id,
            species,
            center: center.to_array(),
            radius: (fit * 0.85).max(1.5),
            count,
        }
    }
}

/// Whether the water at `p` is fresh (a river or lake above the sea) rather than the sea.
pub fn is_fresh(w: &WaterBodies, t: &Terrain, p: Vec2) -> bool {
    let g = t.height_at(p.x, p.y);
    w.water_level(Vec3::new(p.x, g, p.y))
        .is_some_and(|l| l > t.sea_level + 0.05)
}

/// A hint when a species is released in water it doesn't live in (`None` when it suits).
pub fn habitat_hint(sp: FishSpecies, fresh: bool) -> Option<String> {
    (!sp.info().water.suits(fresh)).then(|| {
        format!(
            "{} usually live in {}",
            sp.name(),
            if fresh { "the sea" } else { "fresh water" }
        )
    })
}

#[derive(Component)]
struct Fish {
    school: u32,
    phase: f32,
    orbit: f32,
    depth: f32,
    dir: f32,
    speed: f32,
    length: f32,
}

pub struct FishPlugin;

impl Plugin for FishPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<FishMaterial>::default())
            .init_resource::<FishModels>()
            .add_systems(PreStartup, model::init_clock)
            .add_systems(
                Update,
                (model::finish_skins, model::tick_clock, sync_fish).chain(),
            )
            // (after the camera moved this frame: see `swim`)
            .add_systems(
                PostUpdate,
                swim.before(bevy::transform::TransformSystems::Propagate),
            );
    }
}

/// Respawn fish entities when the set of schools changes.
#[allow(clippy::too_many_arguments)]
fn sync_fish(
    mut commands: Commands,
    water: Res<WaterBodies>,
    mut models: ResMut<FishModels>,
    server: Res<AssetServer>,
    mut materials: ResMut<Assets<FishMaterial>>,
    mut images: ResMut<Assets<Image>>,
    existing: Query<Entity, With<Fish>>,
    mut signature: Local<Vec<(u32, FishSpecies, u32, [i32; 3], i32)>>,
) {
    let sig: Vec<_> = water
        .fish
        .iter()
        .map(|f| {
            (
                f.id,
                f.species,
                f.count,
                [
                    f.center[0] as i32,
                    (f.center[1] * 10.0) as i32,
                    f.center[2] as i32,
                ],
                (f.radius * 10.0) as i32,
            )
        })
        .collect();
    if *signature == sig {
        return;
    }
    *signature = sig;
    for e in &existing {
        commands.entity(e).despawn();
    }
    for school in &water.fish {
        let info = school.species.info();
        let model = models.model(school.species, &server, &mut materials, &mut images);
        for i in 0..school.count {
            let h = crate::noise::hash_u32(school.id * 7919 + i * 104_729);
            let f = |k: u32| ((h.rotate_left(k * 7) & 0xffff) as f32) / 65535.0;
            let orbit = 0.25 + 0.75 * f(2);
            let dir = if f(4) > 0.25 { 1.0 } else { -1.0 };
            let speed = info.speed * (0.8 + 0.4 * f(5));
            let length = info.length * (0.85 + 0.3 * f(6));
            // the body curls into its loop (the loop's centre is on the fish's right when dir > 0):
            // the turn rate × 0.12 like Emberfall, in units of the shader's 0.35 maximum
            let bend = -dir * (speed / (school.radius * orbit).max(1.0) * 0.12 / 0.35).min(0.55);
            let tf = Transform::from_translation(Vec3::from_array(school.center))
                .with_scale(Vec3::splat(length));
            let e = spawn_fish(
                &mut commands,
                &model,
                tf,
                swim_tag(f(1), bend, 0.8 + 0.4 * f(7), false),
                None,
            );
            commands.entity(e).insert((
                Name::new(format!("{} {}", info.name, i)),
                Fish {
                    school: school.id,
                    phase: f(1) * std::f32::consts::TAU,
                    orbit,
                    depth: f(3),
                    dir,
                    speed,
                    length,
                },
            ));
        }
    }
}

fn swim(
    time: Res<Time>,
    motion: Res<crate::wind::Motion>,
    water: Res<WaterBodies>,
    terrain: Res<Terrain>,
    mut clock: Local<f32>,
    mut q: Query<(&Fish, &mut Transform), Without<crate::camera::MainCamera>>,
    cam: Query<(&Transform, &Projection), With<crate::camera::MainCamera>>,
) {
    // fish hold still with the rest of the world while editing (wind::Motion)
    if motion.live {
        *clock += time.delta_secs();
    }
    let t = *clock;
    // Where a fish is depends only on the clock, so schools out of view aren't moved: each of a
    // fish's parts is a mesh instance the renderer would upload again (a school of trout is
    // thousands a frame), and in view again they are exactly where they would have been.
    // (this frame's view: the camera's `Frustum` is last frame's until transforms propagate)
    let frustum = cam.single().ok().map(|(t, p)| {
        use bevy::camera::CameraProjection;
        p.compute_frustum(&GlobalTransform::from(*t))
    });
    let schools: HashMap<u32, (&FishSchool, bool)> = water
        .fish
        .iter()
        .map(|s| {
            let seen = frustum.is_none_or(|f| {
                f.intersects_sphere(
                    &bevy::camera::primitives::Sphere {
                        center: Vec3::from_array(s.center).into(),
                        // (the loop breathes to the radius and drifts an eighth of it, plus a
                        // body length)
                        radius: s.radius * 1.2 + s.species.info().length * 1.5 + 2.0,
                    },
                    false,
                )
            });
            (s.id, (s, seen))
        })
        .collect();
    for (fish, mut tf) in &mut q {
        let Some(&(s, seen)) = schools.get(&fish.school) else {
            continue;
        };
        if !seen {
            continue;
        }
        let info = s.species.info();
        let c = Vec3::from_array(s.center);
        let r = s.radius * fish.orbit;
        // wandering loop: a circle whose radius breathes, plus a slow drift of the whole school
        let ang = fish.phase + fish.dir * t * fish.speed / r.max(1.0);
        let rr = r * (0.75 + 0.25 * (t * 0.23 + fish.phase * 3.0).sin());
        let drift = Vec2::new(
            (t * 0.05 + fish.phase).sin(),
            (t * 0.04 + fish.phase * 1.7).cos(),
        ) * s.radius
            * 0.12;
        let p2 = c.xz() + Vec2::new(ang.cos(), ang.sin()) * rr + drift;
        let vel = Vec2::new(-ang.sin(), ang.cos()) * fish.dir;
        // the bed under the nose and the tail too: on a sloping bed the middle alone lets the
        // head or tail sink into the ground
        let reach = vel * fish.length * 0.55;
        let ground = [p2, p2 + reach, p2 - reach]
            .iter()
            .map(|q| terrain.height_at(q.x, q.y))
            .fold(f32::MIN, f32::max);
        // the species' depth (bottom dwellers hug the floor), spread a little per fish, kept a
        // body height clear of the surface and the bottom
        let half = fish.length
            * if info.plan == species::Plan::Ray {
                0.08
            } else {
                0.16
            };
        let top = c.y - half - 0.15;
        let bottom = ground + half + 0.12;
        let want = (info.depth + (fish.depth - 0.5) * 0.35).clamp(0.0, 1.0);
        // too shallow for the whole body: stay above the bed (a back breaking the surface reads
        // as a fish in the shallows; one sunk into the ground reads as broken)
        let y = if top > bottom {
            top + (bottom - top) * want
        } else {
            bottom
        };
        tf.translation = Vec3::new(p2.x, y, p2.y);
        // the models point along +Z
        tf.rotation = Quat::from_rotation_y(vel.x.atan2(vel.y));
    }
}
