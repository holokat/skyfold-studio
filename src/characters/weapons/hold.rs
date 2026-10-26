//! How a hand holds a weapon, and the upper-body pose that raises it to the aim line.
//!
//! **The grip.** A weapon's origin is its holding hand's palm centre (`grip_r`, a bow's `grip_l`;
//! weapons conventions: glTF +Z forward, +Y up). On any humanoid with finger bones the hand's own
//! rest frame says where that is: the metacarpal direction (wrist → `middle_01`), the thumb side
//! (`pinky_01` → `index_01`) and the palm side. Guns and bows lie along the knuckles (barrel /
//! arrow = metacarpal direction, top = thumb side); blades come out of the top of the fist
//! (blade = thumb side, cutting edge = the knuckles). `grip_in_hand` turns that into the weapon's
//! transform in the hand bone's space, so a weapon parented to the bone sits in the fist whatever
//! the skeleton's bone axes are.
//!
//! **The pose** (`Hold`, applied by `puppet` after the clip and the rig): the spine pitches toward
//! the aim; when an aim frame is given, the main arm reaches (two-bone IK with an elbow pole) so
//! the weapon lands on the aim line in front of the shoulder or the eye and the hand turns to hold
//! it there (blended with the clip's own hand by `w`); the support hand reaches for a socket of
//! the weapon (a pistol's `grip_l`, a katana's pommel, a bow's `grip_r`, which rides the nock as
//! the `draw` clip pulls the string).

use autorig::{Skeleton, Xform};
use bevy::math::{Mat3, Mat4, Quat, Vec3};
use rigkit::pose::Pose;

/// Which hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Right,
    Left,
}

impl Side {
    pub fn suffix(self) -> &'static str {
        match self {
            Side::Right => "r",
            Side::Left => "l",
        }
    }
    pub fn other(self) -> Side {
        match self {
            Side::Right => Side::Left,
            Side::Left => Side::Right,
        }
    }
    pub fn bone(self, stem: &str) -> String {
        format!("{stem}_{}", self.suffix())
    }
    /// +1 on the character's left (model +X), −1 on its right.
    fn x(self) -> f32 {
        match self {
            Side::Right => -1.0,
            Side::Left => 1.0,
        }
    }
}

/// How the fist closes on the weapon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GripStyle {
    /// guns and bows: forward along the knuckles, top on the thumb side
    Along,
    /// blades: the blade out of the top of the fist, the edge toward the knuckles
    Upright,
}

/// The weapon's frame in the hand bone's space (rest pose), or None without finger bones.
pub fn grip_in_hand(sk: &Skeleton, side: Side, style: GripStyle) -> Option<Xform> {
    let rest = sk.rest_world();
    let at = |stem: &str| sk.find(&side.bone(stem)).map(|i| rest[i]);
    let hand = at("hand")?;
    let p = |m: Mat4| m.w_axis.truncate();
    let (h, mid) = (p(hand), p(at("middle_01")?));
    let (idx, pinky) = (p(at("index_01")?), p(at("pinky_01")?));
    let d = (mid - h).normalize_or_zero();
    let thumb = idx - pinky;
    let up = (thumb - d * thumb.dot(d)).normalize_or_zero();
    if d.length_squared() < 0.5 || up.length_squared() < 0.5 {
        return None;
    }
    // the palm faces down in a T-pose: −(d × up) for the right hand, d × up for the left
    let palm = match side {
        Side::Right => -d.cross(up),
        Side::Left => d.cross(up),
    };
    let centre = h + (mid - h) * 0.45 + palm * 0.03;
    let (z, y) = match style {
        GripStyle::Along => (d, up),
        GripStyle::Upright => (up, -d),
    };
    let rot = Quat::from_mat3(&Mat3::from_cols(y.cross(z), y, z)).normalize();
    let w = Mat4::from_rotation_translation(rot, centre);
    let (_, r, t) = (hand.inverse() * w).to_scale_rotation_translation();
    Some(Xform {
        t,
        r: r.normalize(),
        s: Vec3::ONE,
    })
}

/// Where the aim line starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// in front of this shoulder (hip fire, a bow arm)
    Shoulder(Side),
    /// in front of the eyes (aiming down the sights; for the combat port's stances)
    #[allow(dead_code)]
    Eye,
}

/// The weapon put on the aim line: its `socket` point (weapon space; the origin, or the `aim`
/// sight point) `reach` metres out along `dir` from the anchor and `drop` below it, turned by
/// `roll` (radians, about the aim line; a bow's cant).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimFrame {
    /// model space
    pub dir: Vec3,
    pub anchor: Anchor,
    pub reach: f32,
    pub drop: f32,
    /// sideways (m, toward the main hand's side: + = out)
    pub side: f32,
    pub socket: Vec3,
    pub roll: f32,
    /// weapon-space offset added after placing it (recoil kick, swing bob)
    pub kick: Vec3,
    /// pitch up of the muzzle (recoil, radians)
    pub kick_pitch: f32,
}

/// The upper-body pose round a held weapon (model space of the puppet).
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    pub main: Side,
    /// the weapon in the main hand's bone space
    pub grip: Xform,
    /// raise the weapon to the aim line (with how much: 0 the clip's own hand, 1 on the line)
    pub aim: Option<(AimFrame, f32)>,
    /// the support hand to this weapon-space point, with its weight
    pub support: Option<(Vec3, f32)>,
    /// spine pitch toward the aim (radians, + up)
    pub pitch: f32,
    /// the main arm's elbow pole (model space; None: down and out)
    pub main_pole: Option<Vec3>,
    pub support_pole: Option<Vec3>,
}

/// A rotation whose +Z is `dir` (+Y as near up as it can be).
pub fn look_z(dir: Vec3) -> Quat {
    let z = dir.normalize_or(Vec3::Z);
    let mut x = Vec3::Y.cross(z);
    if x.length_squared() < 1e-6 {
        x = Vec3::X;
    }
    let x = x.normalize();
    let y = z.cross(x);
    Quat::from_mat3(&Mat3::from_cols(x, y, z)).normalize()
}

fn index(sk: &Skeleton, name: &str) -> Option<usize> {
    sk.find(name)
}

/// Two-bone IK with an elbow pole: `a` → `b` → `c` (upper arm, forearm, hand) so `c` reaches
/// `target`, the elbow bending toward `pole` (a model-space direction).
pub fn two_bone(p: &mut Pose, a: usize, b: usize, c: usize, target: Vec3, pole: Vec3) {
    let (pa, pb, pc) = (p.pos(a), p.pos(b), p.pos(c));
    let (lab, lbc) = (pa.distance(pb), pb.distance(pc));
    if lab < 1e-4 || lbc < 1e-4 {
        return;
    }
    let to = target - pa;
    let dist = to.length().clamp(1e-3, (lab + lbc) * 0.999);
    let dir = to.normalize_or(Vec3::Z);
    let cos_a = ((lab * lab + dist * dist - lbc * lbc) / (2.0 * lab * dist)).clamp(-1.0, 1.0);
    let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();
    let mut bend = pole - dir * pole.dot(dir);
    if bend.length_squared() < 1e-6 {
        bend = (pb - pa) - dir * (pb - pa).dot(dir);
    }
    let bend = bend.normalize_or(Vec3::NEG_Y);
    let elbow = pa + dir * (cos_a * lab) + bend * (sin_a * lab);
    let r = Quat::from_rotation_arc((pb - pa).normalize(), (elbow - pa).normalize());
    p.turn(a, r);
    let (pb, pc) = (p.pos(b), p.pos(c));
    let reach = pa + dir * dist;
    let (u, v) = ((pc - pb).normalize_or_zero(), (reach - pb).normalize_or_zero());
    if u.length_squared() > 0.5 && v.length_squared() > 0.5 {
        p.turn(b, Quat::from_rotation_arc(u, v));
    }
}

/// Applies `h` to the pose (`local`: the bones' local transforms after the clip and the rig).
/// Returns the weapon's model-space frame.
pub fn apply(sk: &Skeleton, local: &mut [Xform], h: &Hold) -> Option<Mat4> {
    let mut p = Pose::new(sk, local);
    // the spine leans into the aim (pitch about the model's side axis, shared up the spine)
    if h.pitch.abs() > 1e-4 {
        for (name, share) in [("spine_01", 0.2), ("spine_02", 0.35), ("spine_03", 0.45)] {
            if let Some(i) = index(sk, name) {
                p.turn(i, Quat::from_rotation_x(-h.pitch * share));
            }
        }
    }
    let m = h.main;
    let (ua, la, hand) = (
        index(sk, &m.bone("upperarm"))?,
        index(sk, &m.bone("lowerarm"))?,
        index(sk, &m.bone("hand"))?,
    );
    let grip = h.grip.matrix();
    let held_clip = p.world[hand] * grip;
    let mut held = held_clip;
    if let Some((f, w)) = h.aim
        && w > 1e-3
    {
        let anchor = match f.anchor {
            Anchor::Shoulder(s) => index(sk, &s.bone("upperarm")).map(|i| p.pos(i)),
            Anchor::Eye => index(sk, "Head")
                .or_else(|| index(sk, "head"))
                .map(|i| p.pos(i) + Vec3::new(0.0, 0.09, 0.09)),
        }?;
        let rot = Quat::from_axis_angle(f.dir.normalize_or(Vec3::Z), f.roll) * look_z(f.dir);
        let rot = rot * Quat::from_rotation_x(-f.kick_pitch);
        let side = Vec3::X * (m.x() * f.side);
        let at = anchor + f.dir.normalize_or(Vec3::Z) * f.reach - Vec3::Y * f.drop + side;
        let pos = at - rot * f.socket + rot * f.kick;
        let want = Mat4::from_rotation_translation(rot, pos);
        held = blend(held_clip, want, w.min(1.0));
        let hand_w = held * grip.inverse();
        let (_, hr, ht) = hand_w.to_scale_rotation_translation();
        let pole = h.main_pole.unwrap_or(Vec3::new(m.x() * 0.6, -1.0, -0.2));
        two_bone(&mut p, ua, la, hand, ht, pole);
        p.set_rot(hand, hr.normalize());
        held = p.world[hand] * grip;
    }
    if let Some((socket, w)) = h.support
        && w > 1e-3
    {
        let s = m.other();
        if let (Some(ua), Some(la), Some(hd)) = (
            index(sk, &s.bone("upperarm")),
            index(sk, &s.bone("lowerarm")),
            index(sk, &s.bone("hand")),
        ) {
            let target = held.transform_point3(socket);
            let now = p.pos(hd);
            let pole = h.support_pole.unwrap_or(Vec3::new(s.x() * 0.6, -1.0, -0.2));
            two_bone(&mut p, ua, la, hd, now.lerp(target, w.min(1.0)), pole);
        }
    }
    Some(held)
}

fn blend(a: Mat4, b: Mat4, w: f32) -> Mat4 {
    let (_, ra, ta) = a.to_scale_rotation_translation();
    let (_, rb, tb) = b.to_scale_rotation_translation();
    let rb = if ra.dot(rb) < 0.0 { -rb } else { rb };
    Mat4::from_rotation_translation(ra.slerp(rb, w).normalize(), ta.lerp(tb, w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use autorig::Bone;

    /// A T-posed right arm like the UE mannequin's: facing +Z, the arm along −X, palm down.
    fn arm() -> Skeleton {
        let bone = |name: &str, parent: Option<usize>, t: Vec3| Bone {
            name: name.into(),
            parent,
            rest: Xform {
                t,
                r: Quat::IDENTITY,
                s: Vec3::ONE,
            },
            inverse_bind: Mat4::IDENTITY,
        };
        Skeleton {
            bones: vec![
                bone("spine_03", None, Vec3::new(0.0, 1.3, 0.0)),
                bone("upperarm_r", Some(0), Vec3::new(-0.19, 0.14, 0.0)),
                bone("lowerarm_r", Some(1), Vec3::new(-0.27, 0.0, 0.0)),
                bone("hand_r", Some(2), Vec3::new(-0.27, 0.0, 0.0)),
                bone("middle_01_r", Some(3), Vec3::new(-0.12, 0.0, 0.0)),
                bone("index_01_r", Some(3), Vec3::new(-0.12, 0.0, 0.03)),
                bone("pinky_01_r", Some(3), Vec3::new(-0.11, 0.0, -0.04)),
            ],
        }
    }

    #[test]
    fn a_gun_lies_along_the_knuckles_and_a_blade_stands_up() {
        let sk = arm();
        let rest = sk.rest_world();
        let hand = rest[3];
        let g = grip_in_hand(&sk, Side::Right, GripStyle::Along).unwrap();
        let w = hand * g.matrix();
        let fwd = w.transform_vector3(Vec3::Z);
        let up = w.transform_vector3(Vec3::Y);
        // barrel out along the arm (−X), top toward the thumb side (+Z forward)
        assert!(fwd.dot(Vec3::NEG_X) > 0.99, "{fwd}");
        assert!(up.dot(Vec3::Z) > 0.99, "{up}");
        // in the palm: below the hand bone, part way to the knuckles
        let c = w.w_axis.truncate();
        assert!(c.y < rest[3].w_axis.y && c.x < rest[3].w_axis.x);
        let b = grip_in_hand(&sk, Side::Right, GripStyle::Upright).unwrap();
        let w = hand * b.matrix();
        assert!(w.transform_vector3(Vec3::Z).dot(Vec3::Z) > 0.99);
        // the edge (−Y) toward the knuckles
        assert!(w.transform_vector3(Vec3::NEG_Y).dot(Vec3::NEG_X) > 0.99);
    }

    #[test]
    fn the_hand_reaches_the_aim_line() {
        let sk = arm();
        let grip = grip_in_hand(&sk, Side::Right, GripStyle::Along).unwrap();
        let mut local: Vec<Xform> = sk.bones.iter().map(|b| b.rest).collect();
        let f = AimFrame {
            dir: Vec3::Z,
            anchor: Anchor::Shoulder(Side::Right),
            reach: 0.45,
            drop: 0.05,
            side: 0.0,
            socket: Vec3::ZERO,
            roll: 0.0,
            kick: Vec3::ZERO,
            kick_pitch: 0.0,
        };
        let h = Hold {
            main: Side::Right,
            grip,
            aim: Some((f, 1.0)),
            support: None,
            pitch: 0.0,
            main_pole: None,
            support_pole: None,
        };
        let held = apply(&sk, &mut local, &h).unwrap();
        let shoulder = sk.rest_world()[1].w_axis.truncate();
        let want = shoulder + Vec3::Z * 0.45 - Vec3::Y * 0.05;
        let got = held.w_axis.truncate();
        assert!(got.distance(want) < 0.01, "{got} vs {want}");
        assert!(held.transform_vector3(Vec3::Z).dot(Vec3::Z) > 0.99);
        // bone lengths kept
        let p = Pose::new(&sk, &mut local);
        assert!((p.pos(2).distance(p.pos(1)) - 0.27).abs() < 1e-3);
        assert!((p.pos(3).distance(p.pos(2)) - 0.27).abs() < 1e-3);
    }

    #[test]
    fn look_z_points_forward_and_keeps_up() {
        let q = look_z(Vec3::new(1.0, 0.2, 1.0));
        assert!((q * Vec3::Z).dot(Vec3::new(1.0, 0.2, 1.0).normalize()) > 0.999);
        assert!((q * Vec3::Y).y > 0.9);
    }
}
