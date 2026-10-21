//! Parts of library and imported glTF models that rock or bob: lanterns swinging on their hooks,
//! a gondola on its rope, a skiff rolling and bobbing on the water, bubbles rising in a basin. A
//! glTF node whose extras hold `{"swing": {"axis": [x, y, z], "deg": a, "period": s, "wind": w}}`
//! (the style packs' `Asset.swing_node`) rocks ±`a` degrees about `axis` (node space, through the
//! node's origin: its hook) once every `s` seconds; `{"bob": {"axis": [...], "m": a, ...}}`
//! (`Asset.bob_node`) moves ±`a` metres along it. `wind` (0..1) is how much the wind livens the
//! motion (still air keeps a third of it, a gale about doubles it). Like spinning parts, cloth
//! and trees they only move while the world animates (walk mode, cinematics, Preview motion);
//! each copy has its own phase. Colliders keep the rest pose. The same models carry a looping
//! glTF animation per part ("swing_<part>" / "bob_<part>") for other engines.

use bevy::camera::primitives::Frustum;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::wind::{Motion, WindState};

/// A rocking (`bob` false) or bobbing part: its rest pose, axis (local) and motion.
#[derive(Component, Debug, Clone)]
pub struct Swing {
    pub bob: bool,
    pub axis: Vec3,
    /// radians (swing) or metres (bob)
    pub amp: f32,
    pub period: f32,
    pub wind: f32,
    base: Transform,
    phase: f32,
}

/// Reads a node's `swing` or `bob` extras: (bob?, axis, amplitude in radians or metres, period
/// s, wind 0..1); None when it has neither.
pub fn parse_swing(extras: &str) -> Option<(bool, Vec3, f32, f32, f32)> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let (bob, s) = match (v.get("swing"), v.get("bob")) {
        (Some(s), _) => (false, s),
        (None, Some(s)) => (true, s),
        _ => return None,
    };
    let a = s.get("axis")?.as_array()?;
    let axis = Vec3::new(
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    )
    .try_normalize()?;
    let amp = if bob {
        s.get("m").and_then(|x| x.as_f64()).unwrap_or(0.03) as f32
    } else {
        (s.get("deg").and_then(|x| x.as_f64()).unwrap_or(5.0) as f32).to_radians()
    };
    let period = (s.get("period").and_then(|x| x.as_f64()).unwrap_or(3.0) as f32).max(0.2);
    let wind = s
        .get("wind")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0) as f32;
    Some((bob, axis, amp, period, wind))
}

/// How lively a part is relative to its amplitude: a wind-driven part (w = 1) keeps a third of
/// its motion in still air, all of it in a 6 m/s breeze, about twice it in a gale.
pub fn liveliness(wind_mps: f32, w: f32) -> f32 {
    let k = 0.33 + 0.67 * (wind_mps / 6.0).clamp(0.0, 2.5);
    1.0 + (k.min(2.0) - 1.0) * w
}

/// A model's scene is in: its rocking and bobbing nodes get their `Swing`.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    nodes: Query<(&GltfExtras, &Transform)>,
    mut commands: Commands,
) {
    let root = ev.event().entity;
    for c in children.iter_descendants(root) {
        let Ok((x, tf)) = nodes.get(c) else { continue };
        let Some((bob, axis, amp, period, wind)) = parse_swing(&x.value) else {
            continue;
        };
        let phase = ((c.index_u32().wrapping_mul(2_654_435_761)) % 1000) as f32 / 1000.0
            * std::f32::consts::TAU;
        commands.entity(c).insert(Swing {
            bob,
            axis,
            amp,
            period,
            wind,
            base: *tf,
            phase,
        });
    }
}

/// How far (m) from the camera parts still rock and bob (a lantern's sway is lost in a few
/// pixels well before), and the radius (m, at scale 1) assumed round one for the view test.
const SWING_REACH: f32 = 300.0;
const SWING_RADIUS: f32 = 5.0;

/// Rocks and bobs every such part in sight while the world animates.
pub fn rock(
    time: Res<Time>,
    motion: Res<Motion>,
    wind: Res<WindState>,
    cam: Query<(&GlobalTransform, &Frustum), With<crate::camera::MainCamera>>,
    mut parts: Query<(&Swing, &mut Transform, &GlobalTransform)>,
) {
    if !motion.live {
        return;
    }
    let t = time.elapsed_secs_wrapped();
    let cam = cam.single().ok();
    for (s, mut tf, gt) in &mut parts {
        if !super::spin::in_sight(
            cam,
            gt.translation(),
            SWING_RADIUS * gt.scale().max_element(),
            SWING_REACH,
        ) {
            continue;
        }
        let x = (t * std::f32::consts::TAU / s.period + s.phase).sin()
            * s.amp
            * liveliness(wind.speed, s.wind);
        if s.bob {
            tf.translation = s.base.translation + s.base.rotation * (s.axis * x);
        } else {
            tf.rotation = s.base.rotation * Quat::from_axis_angle(s.axis, x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_swing_and_bob_extras() {
        let (bob, axis, amp, period, wind) =
            parse_swing(r#"{"swing": {"axis": [0, 0, -2], "deg": 6, "period": 3, "wind": 1}}"#)
                .unwrap();
        assert!(
            !bob && axis == Vec3::NEG_Z
                && (amp - 6f32.to_radians()).abs() < 1e-6
                && period == 3.0
                && wind == 1.0
        );
        let (bob, axis, amp, ..) =
            parse_swing(r#"{"bob": {"axis": [0, 1, 0], "m": 0.05, "period": 2}}"#).unwrap();
        assert!(bob && axis == Vec3::Y && amp == 0.05);
        assert!(parse_swing(r#"{"spin": {"axis": [0, 1, 0], "rpm": 9}}"#).is_none());
    }

    #[test]
    fn wind_livens_swinging() {
        assert!((liveliness(0.0, 1.0) - 0.33).abs() < 1e-6);
        assert!((liveliness(6.0, 1.0) - 1.0).abs() < 1e-6);
        assert_eq!(liveliness(40.0, 1.0), 2.0);
        assert_eq!(liveliness(0.0, 0.0), 1.0);
    }
}
