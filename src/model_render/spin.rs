//! Moving parts of library and imported glTF models: windmill sails, water wheels, saw blades,
//! spinning training targets. A glTF node whose extras hold
//! `{"spin": {"axis": [x, y, z], "rpm": r, "wind": w}}` (the style packs' `Asset.spin_node`) turns
//! about `axis` (node space) at `rpm` turns a minute once its model's scene has spawned; `wind`
//! (0..1) is how much the speed follows the world's wind: 1 for sails (still in a calm, about
//! twice as fast in a strong breeze), 0 for a mill wheel turned by its stream. Like trees, grass
//! and cloth, parts only move while the world animates (walk mode, cinematics, Preview motion);
//! while editing they hold still. Each copy starts at its own angle, so a row of windmills
//! doesn't turn in lockstep. Colliders keep the rest pose. The same models carry a looping glTF
//! animation "spin" for other engines; Skyfold drives the motion itself (wind-aware).

use bevy::camera::primitives::Frustum;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::wind::{Motion, WindState};

/// A turning part: its rest rotation, axis (local), speed and how much the wind drives it.
#[derive(Component, Debug, Clone)]
pub struct Spin {
    pub axis: Vec3,
    pub rpm: f32,
    pub wind: f32,
    base: Quat,
    angle: f32,
}

/// Reads a node's `spin` extras (None when it has none or they don't parse).
pub fn parse_spin(extras: &str) -> Option<(Vec3, f32, f32)> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let s = v.get("spin")?;
    let a = s.get("axis")?.as_array()?;
    let axis = Vec3::new(
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    )
    .try_normalize()?;
    let rpm = s.get("rpm").and_then(|x| x.as_f64()).unwrap_or(6.0) as f32;
    let wind = s
        .get("wind")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0) as f32;
    Some((axis, rpm, wind))
}

/// How fast a part turns relative to its rpm: a wind-driven part (w = 1) stands still in a calm,
/// turns at its rpm in a 6 m/s breeze and tops out at 2.5x.
pub fn speed_factor(wind_mps: f32, w: f32) -> f32 {
    let wind_k = (wind_mps / 6.0).clamp(0.0, 2.5);
    1.0 + (wind_k - 1.0) * w
}

/// A model's scene is in: its spinning nodes get their `Spin`.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    nodes: Query<(&GltfExtras, &Transform)>,
    mut commands: Commands,
) {
    let root = ev.event().entity;
    for c in children.iter_descendants(root) {
        let Ok((x, tf)) = nodes.get(c) else { continue };
        let Some((axis, rpm, wind)) = parse_spin(&x.value) else {
            continue;
        };
        // a start angle of its own (stable per entity)
        let angle = ((c.index_u32().wrapping_mul(2_654_435_761)) % 1000) as f32 / 1000.0
            * std::f32::consts::TAU;
        commands.entity(c).insert(Spin {
            axis,
            rpm,
            wind,
            base: tf.rotation,
            angle,
        });
    }
}

/// How far (m) from the camera sails and wheels still turn, and the radius (m, at scale 1)
/// assumed round a part when testing it against the view.
const SPIN_REACH: f32 = 1500.0;
const SPIN_RADIUS: f32 = 12.0;

/// Whether a moving part at `p` (world space, about `radius` round) is worth moving this frame:
/// within `reach` of the camera and in its view. Every moved part re-propagates its transforms
/// and re-uploads its meshes, so parts out of sight hold still (and carry on once seen again).
pub fn in_sight(
    cam: Option<(&GlobalTransform, &Frustum)>,
    p: Vec3,
    radius: f32,
    reach: f32,
) -> bool {
    let Some((cam, frustum)) = cam else {
        return true;
    };
    cam.translation().distance_squared(p) < reach * reach
        && frustum.intersects_sphere(
            &bevy::camera::primitives::Sphere {
                center: p.into(),
                radius,
            },
            true,
        )
}

/// Turns every spinning part in sight while the world animates.
pub fn turn(
    time: Res<Time>,
    motion: Res<Motion>,
    wind: Res<WindState>,
    cam: Query<(&GlobalTransform, &Frustum), With<crate::camera::MainCamera>>,
    mut parts: Query<(&mut Spin, &mut Transform, &GlobalTransform)>,
) {
    if !motion.live {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    let cam = cam.single().ok();
    for (mut s, mut tf, gt) in &mut parts {
        if !in_sight(
            cam,
            gt.translation(),
            SPIN_RADIUS * gt.scale().max_element(),
            SPIN_REACH,
        ) {
            continue;
        }
        let w = std::f32::consts::TAU * s.rpm / 60.0 * speed_factor(wind.speed, s.wind);
        s.angle = (s.angle + w * dt).rem_euclid(std::f32::consts::TAU);
        tf.rotation = s.base * Quat::from_axis_angle(s.axis, s.angle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_spin_extras() {
        let (axis, rpm, wind) =
            parse_spin(r#"{"spin": {"axis": [0, 0, 2], "rpm": 9, "wind": 1}}"#).unwrap();
        assert_eq!((axis, rpm, wind), (Vec3::Z, 9.0, 1.0));
        assert!(parse_spin(r#"{"kind": "fire"}"#).is_none());
        assert!(parse_spin(r#"{"spin": {"axis": [0, 0, 0]}}"#).is_none());
    }

    #[test]
    fn wind_drives_sails_not_mill_wheels() {
        assert_eq!(speed_factor(0.0, 1.0), 0.0);
        assert_eq!(speed_factor(6.0, 1.0), 1.0);
        assert_eq!(speed_factor(30.0, 1.0), 2.5);
        assert_eq!(speed_factor(0.0, 0.0), 1.0);
        assert_eq!(speed_factor(20.0, 0.0), 1.0);
    }
}
