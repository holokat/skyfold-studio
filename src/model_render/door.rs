//! Hinged doors, gates and hatches of library and imported glTF models. A glTF node whose extras
//! hold `{"door": {"axis": [x, y, z], "open_deg": d, "leaf": [x, y, z]}}` (the style packs'
//! `Asset.door_node`) swings about `axis` (node space, through the node's origin: the hinge) up to
//! `d` degrees; `leaf` is the leaf's centre from the hinge. While walking, a door swings open as
//! the explorer comes within a couple of metres, away from them (it opens either way), and shuts
//! once they have gone. A sliding door adds `"slide": [x, y, z]`: how far the leaf moves when open
//! (node space, e.g. along the wall), instead of swinging (`axis` is then only for other engines);
//! `"one_way": true` always swings the positive turn about `axis` (the author picks the axis sign so
//! that is outward: a cabinet door that must not swing into its insides); agents can hold doors open or shut (`doors` MCP tool). Doors are never
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
    /// Current swing (radians, signed: which way it opened); a sliding door: 0 shut .. 1 open.
    pub angle: f32,
    /// A sliding door: how far its leaf moves when open (node space).
    pub slide: Option<Vec3>,
    /// Always opens with the positive turn (not away from the explorer).
    pub one_way: bool,
    base: Quat,
    base_t: Vec3,
    /// The explorer is close (with hysteresis).
    near: bool,
}

impl Door {
    /// 0 shut .. 1 wide open.
    pub fn openness(&self) -> f32 {
        if self.slide.is_some() {
            return self.angle.clamp(0.0, 1.0);
        }
        (self.angle.abs() / self.open.max(1e-3)).min(1.0)
    }
}

/// Reads a node's `door` extras: (axis, open radians, leaf centre); None when it has none.
pub fn parse_door(extras: &str) -> Option<(Vec3, f32, Vec3)> {
    parse_door_slide(extras).map(|(a, o, l, _)| (a, o, l))
}

/// Reads a node's `door` extras with a sliding door's travel (`slide`, node space); a sliding door
/// needs no `axis` (then +Y).
pub fn parse_door_slide(extras: &str) -> Option<(Vec3, f32, Vec3, Option<Vec3>)> {
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
    let slide = vec("slide").filter(|v| v.length() > 1e-3);
    let axis = match vec("axis") {
        Some(a) => a.try_normalize()?,
        None if slide.is_some() => Vec3::Y,
        None => return None,
    };
    let open = (d.get("open_deg").and_then(|x| x.as_f64()).unwrap_or(95.0) as f32)
        .clamp(5.0, 180.0)
        .to_radians();
    Some((axis, open, vec("leaf").unwrap_or(Vec3::ZERO), slide))
}

/// A door's `one_way` extra (opens only with its positive turn).
pub fn door_one_way(extras: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(extras)
        .ok()
        .and_then(|v| v.get("door")?.get("one_way")?.as_bool())
        .unwrap_or(false)
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
        let Some((axis, open, leaf, slide)) = parse_door_slide(&x.value) else {
            continue;
        };
        let one_way = door_one_way(&x.value);
        commands.entity(c).insert(Door {
            axis,
            open,
            leaf,
            hold: DoorHold::Auto,
            angle: 0.0,
            slide,
            one_way,
            base: tf.rotation,
            base_t: tf.translation,
            near: false,
        });
    }
}

/// The closed leaf's centre and the direction a positive swing moves it (world space).
pub fn leaf_frame(door: &Door, gt: &GlobalTransform) -> (Vec3, Vec3) {
    let (scale, rot, pivot) = gt.to_scale_rotation_translation();
    if let Some(sl) = door.slide {
        // (where the leaf stands shut: back along its travel; it moves along the slide)
        let closed = pivot - rot * (scale * sl * door.angle);
        return (closed + rot * (scale * door.leaf), rot * sl);
    }
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
        if let Some(sl) = d.slide {
            let target = if want_open { 1.0 } else { 0.0 };
            if (target - d.angle).abs() < 1e-4 {
                continue;
            }
            // (about 0.8 s to open, 1.1 s to shut)
            let step = if target > d.angle { 1.25 } else { 0.9 } * dt;
            d.angle += (target - d.angle).clamp(-step, step);
            let t = d.angle * d.angle * (3.0 - 2.0 * d.angle);
            tf.translation = d.base_t + d.base * (sl * t);
            continue;
        }
        let target = if !want_open {
            0.0
        } else if d.angle.abs() > 1e-3 {
            d.open * d.angle.signum()
        } else {
            // starting to open: away from the explorer (a one-way door: its positive turn)
            if d.one_way {
                d.open
            } else {
                d.open * who.map_or(1.0, |p| away_from(p, leaf, swing_dir))
            }
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
        // a sliding door: no axis needed (then +Y), its travel read
        let (axis, _, leaf, slide) =
            parse_door_slide(r#"{"door": {"slide": [0.9, 0, 0], "leaf": [0, 1.1, 0]}}"#).unwrap();
        assert_eq!((axis, leaf, slide), (Vec3::Y, Vec3::new(0.0, 1.1, 0.0), Some(Vec3::new(0.9, 0.0, 0.0))));
        assert!(parse_door(r#"{"door": {"slide": [0.9, 0, 0]}}"#).is_some());
        assert_eq!(parse_door_slide(r#"{"door": {"axis": [0, 1, 0]}}"#).unwrap().3, None);
        // one-way doors: only when asked
        assert!(door_one_way(r#"{"door": {"axis": [0, 1, 0], "one_way": true}}"#));
        assert!(!door_one_way(r#"{"door": {"axis": [0, 1, 0]}}"#));
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
