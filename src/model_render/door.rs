//! Hinged doors, gates and hatches of library and imported glTF models. A glTF node whose extras
//! hold `{"door": {"axis": [x, y, z], "open_deg": d, "leaf": [x, y, z]}}` (the style packs'
//! `Asset.door_node`) swings about `axis` (node space, through the node's origin: the hinge) up to
//! `d` degrees; `leaf` is the leaf's centre from the hinge. While walking, a door swings open as
//! the explorer comes within a couple of metres, away from them (it opens either way), and shuts
//! once they have gone; agents can hold doors open or shut (`doors` MCP tool). Doors are never
//! part of a model's walk-in collider, so the wall opening behind a door is the way in. The same
//! models carry a glTF animation "door_open" for other engines.

use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::walk::WalkState;

/// Opens when the explorer's within this of the leaf (horizontal metres)...
const OPEN_R: f32 = 2.4;
/// ...and shuts once they're further than this.
const CLOSE_R: f32 = 3.2;
/// Swing speeds (radians a second).
const OPEN_SPEED: f32 = 2.4;
const CLOSE_SPEED: f32 = 1.6;

/// What a door does: follow the explorer (default), or stay open or shut.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DoorHold {
    #[default]
    Auto,
    Open,
    Closed,
}

impl DoorHold {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "open" => Some(Self::Open),
            "closed" | "shut" | "close" => Some(Self::Closed),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

/// A hinged leaf: its closed rotation, hinge axis (local), how far it opens and where it is.
#[derive(Component, Debug, Clone)]
pub struct Door {
    pub axis: Vec3,
    /// Fully open (radians).
    pub open: f32,
    /// The leaf's centre from the hinge (node space).
    pub leaf: Vec3,
    pub hold: DoorHold,
    /// Current swing (radians, signed: which way it opened).
    pub angle: f32,
    base: Quat,
    /// The explorer is close (with hysteresis).
    near: bool,
}

impl Door {
    /// 0 shut .. 1 wide open.
    pub fn openness(&self) -> f32 {
        (self.angle.abs() / self.open.max(1e-3)).min(1.0)
    }
}

/// Reads a node's `door` extras: (axis, open radians, leaf centre); None when it has none.
pub fn parse_door(extras: &str) -> Option<(Vec3, f32, Vec3)> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let d = v.get("door")?;
    let vec = |k: &str| -> Option<Vec3> {
        let a = d.get(k)?.as_array()?;
        Some(Vec3::new(
            a.first()?.as_f64()? as f32,
            a.get(1)?.as_f64()? as f32,
            a.get(2)?.as_f64()? as f32,
        ))
    };
    let axis = vec("axis")?.try_normalize()?;
    let open = (d.get("open_deg").and_then(|x| x.as_f64()).unwrap_or(95.0) as f32)
        .clamp(5.0, 180.0)
        .to_radians();
    Some((axis, open, vec("leaf").unwrap_or(Vec3::ZERO)))
}

/// Which way to swing (+1 / −1) so the leaf moves away from someone at `from` (all world
/// space): `swing` is where a positive turn moves the leaf's centre.
pub fn away_from(from: Vec3, leaf: Vec3, swing: Vec3) -> f32 {
    if (from - leaf).dot(swing) > 0.0 {
        -1.0
    } else {
        1.0
    }
}

/// A model's scene is in: its door nodes get their `Door`.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    nodes: Query<(&GltfExtras, &Transform)>,
    mut commands: Commands,
) {
    let root = ev.event().entity;
    for c in children.iter_descendants(root) {
        let Ok((x, tf)) = nodes.get(c) else { continue };
        let Some((axis, open, leaf)) = parse_door(&x.value) else {
            continue;
        };
        commands.entity(c).insert(Door {
            axis,
            open,
            leaf,
            hold: DoorHold::Auto,
            angle: 0.0,
            base: tf.rotation,
            near: false,
        });
    }
}

/// The closed leaf's centre and the direction a positive swing moves it (world space).
pub fn leaf_frame(door: &Door, gt: &GlobalTransform) -> (Vec3, Vec3) {
    let (scale, rot, pivot) = gt.to_scale_rotation_translation();
    let closed = rot * Quat::from_axis_angle(door.axis, door.angle).inverse();
    let leaf = pivot + closed * (scale * door.leaf);
    (leaf, closed * door.axis.cross(door.leaf))
}

/// Swings doors toward open or shut: open while the explorer is near (or held open).
pub fn swing(
    time: Res<Time>,
    walk: Res<WalkState>,
    mut doors: Query<(&mut Door, &mut Transform, &GlobalTransform)>,
) {
    let dt = time.delta_secs().min(0.1);
    let who = walk.active.then_some(walk.position);
    for (mut d, mut tf, gt) in &mut doors {
        // (most doors are far from the explorer and shut: settled without working out the leaf)
        let hinge = gt.translation();
        let span = d.leaf.length() * gt.scale().max_element() + CLOSE_R + 3.0;
        let close = who.is_some_and(|p| p.distance_squared(hinge) < span * span);
        if !close && d.hold == DoorHold::Auto && d.angle.abs() < 1e-4 {
            if d.near {
                d.near = false;
            }
            continue;
        }
        let (leaf, swing_dir) = leaf_frame(&d, gt);
        if let Some(p) = who.filter(|_| close) {
            let flat = Vec2::new(p.x - leaf.x, p.z - leaf.z).length();
            let r = if d.near { CLOSE_R } else { OPEN_R };
            d.near = flat < r && (p.y + 0.9 - leaf.y).abs() < 2.2;
        } else {
            d.near = false;
        }
        let want_open = match d.hold {
            DoorHold::Open => true,
            DoorHold::Closed => false,
            DoorHold::Auto => d.near,
        };
        let target = if !want_open {
            0.0
        } else if d.angle.abs() > 1e-3 {
            d.open * d.angle.signum()
        } else {
            // starting to open: away from the explorer
            d.open * who.map_or(1.0, |p| away_from(p, leaf, swing_dir))
        };
        if (target - d.angle).abs() < 1e-4 {
            continue;
        }
        let speed = if target.abs() > d.angle.abs() {
            OPEN_SPEED
        } else {
            CLOSE_SPEED
        };
        let step = speed * dt;
        d.angle += (target - d.angle).clamp(-step, step);
        tf.rotation = d.base * Quat::from_axis_angle(d.axis, d.angle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_door_extras() {
        let (axis, open, leaf) =
            parse_door(r#"{"door": {"axis": [0, 2, 0], "open_deg": 90, "leaf": [0.6, 1.1, 0]}}"#)
                .unwrap();
        assert_eq!(axis, Vec3::Y);
        assert!((open - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(leaf, Vec3::new(0.6, 1.1, 0.0));
        assert!(parse_door(r#"{"spin": {"axis": [0, 1, 0], "rpm": 9}}"#).is_none());
        assert!(parse_door(r#"{"door": {"axis": [0, 0, 0]}}"#).is_none());
        assert_eq!(DoorHold::parse("shut"), Some(DoorHold::Closed));
    }

    #[test]
    fn swings_away_from_the_explorer() {
        // hinge at the origin, leaf along +x, vertical axis: a positive turn moves it toward -z
        let leaf = Vec3::new(0.6, 1.0, 0.0);
        let swing = Vec3::Y.cross(leaf);
        assert!(swing.z < 0.0);
        // explorer on the -z side: swing the other way (+z is negative turn)
        assert_eq!(away_from(Vec3::new(0.5, 0.0, -2.0), leaf, swing), -1.0);
        assert_eq!(away_from(Vec3::new(0.5, 0.0, 2.0), leaf, swing), 1.0);
    }
}
