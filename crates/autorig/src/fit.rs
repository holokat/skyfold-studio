//! Fits the template skeleton (`humanoid`) into a character from its markers: the torso chain
//! is stretched between the groin and the chin, arms run shoulder → elbow → wrist, legs hip →
//! knee → ankle → toes, and the hands' fingers are carried over in the forearm's frame. Every
//! joint sits in the middle of the body's depth. Rest rotations are the template's, turned so
//! each bone points where the character's does (what `retarget` relies on).

use glam::{Quat, Vec2, Vec3};

use crate::humanoid::{self, primary_child, template};
use crate::markers::{self, Markers, Silhouette, TEMPLATE};
use crate::model::{Model, Skeleton};

/// The character's joint positions, in template bone order.
pub fn joints(model: &Model, m: &Markers) -> Vec<Vec3> {
    let t = template();
    let (lo, hi) = model.bounds().unwrap_or((Vec3::ZERO, Vec3::splat(1.8)));
    let h = (hi.y - lo.y).max(1e-3);
    let sil = Silhouette::new(model, 240);
    let r = 0.025 * h;
    // (depths in the body's own layer of the solid: a tail or cape apart from it doesn't count)
    let body_z = m.groin.z;
    let solid = crate::skin::Solid::new(model, markers::SOLID_RES);
    let range = |x: f32, y: f32, z_ref: f32| match &solid {
        Some(s) => s.depth_near(x, y, z_ref),
        None => markers::depth_range(model, x, y, r),
    };
    let mid_z = |x: f32, y: f32, or: f32| {
        range(x, y, body_z).map(|(a, b)| (a + b) * 0.5).unwrap_or(or)
    };
    // the limbs' markers, in the middle of their limb
    let limb = |p: Vec3| Vec3::new(p.x, p.y, range(p.x, p.y, p.z).map(|(a, b)| (a + b) * 0.5).unwrap_or(p.z));
    let m = &Markers {
        wrist_l: limb(m.wrist_l),
        wrist_r: limb(m.wrist_r),
        elbow_l: limb(m.elbow_l),
        elbow_r: limb(m.elbow_r),
        knee_l: limb(m.knee_l),
        knee_r: limb(m.knee_r),
        ..*m
    };
    let cx = m.groin.x;
    // torso: template heights mapped groin → groin, chin → chin
    let s = (m.chin.y - m.groin.y) / (TEMPLATE.chin.y - TEMPLATE.groin.y);
    let map_y = |y: f32| m.groin.y + (y - TEMPLATE.groin.y) * s;
    let half = sil
        .as_ref()
        .map(|sil| sil.torso_half_width(m.groin.y, m.chin.y, cx))
        .unwrap_or(markers::TEMPLATE_TORSO_HALF * s);
    let sw = half / markers::TEMPLATE_TORSO_HALF;
    let mut out = vec![Vec3::ZERO; t.bones.len()];
    let torso_z = |name: &str, x: f32, y: f32| mid_z(x, y, 0.0) + depth_offset(name) * s;
    for name in ["pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "Head"] {
        let y = map_y(t.pos(name).y);
        // (the spine follows the back: a chest, belly or breasts don't move it forward)
        let z = match back_offset(name) {
            Some(off) => range(cx, y, body_z)
                .map(|(back, _)| back + off * s)
                .unwrap_or_else(|| torso_z(name, cx, y)),
            None => torso_z(name, cx, y),
        };
        put(&mut out, name, Vec3::new(cx, y, z));
    }
    let pelvis_z = out[t.idx("pelvis")].z;
    // (the template's root is 0.05 m in front of its pelvis)
    put(&mut out, "root", Vec3::new(cx, lo.y, pelvis_z + 0.0501 * s));
    for side in ["l", "r"] {
        let left = side == "l";
        let n = |stem: &str| format!("{stem}_{side}");
        let (wrist, elbow, knee) = if left {
            (m.wrist_l, m.elbow_l, m.knee_l)
        } else {
            (m.wrist_r, m.elbow_r, m.knee_r)
        };
        // shoulder girdle
        let tc = t.pos(&n("clavicle"));
        let spine3 = out[t.idx("spine_03")];
        let cy = map_y(tc.y);
        let clav_x = cx + tc.x * sw;
        let clav = Vec3::new(clav_x, cy, torso_z("clavicle", clav_x, cy));
        let _ = spine3;
        put(&mut out, &n("clavicle"), clav);
        let sh2 = match &sil {
            Some(sil) => markers::traced_shoulder(sil, elbow.truncate(), cx)
                .unwrap_or_else(|| markers::shoulder(sil, m.chin, m.groin, left)),
            None => Vec2::new(cx + t.pos(&n("upperarm")).x * sw, map_y(t.pos(&n("upperarm")).y)),
        };
        // (its depth from the upper arm itself, a third of the way to the elbow, clear of the chest)
        let q = sh2 + (elbow.truncate() - sh2) * 0.35;
        let sh = Vec3::new(sh2.x, sh2.y, mid_z(q.x, q.y, clav.z));
        put(&mut out, &n("upperarm"), sh);
        put(&mut out, &n("lowerarm"), elbow);
        put(&mut out, &n("hand"), wrist);
        // fingers: the template hand's offsets, in the forearm's frame, scaled with it
        let (th, tl) = (t.pos(&n("hand")), t.pos(&n("lowerarm")));
        let rot = Quat::from_rotation_arc((th - tl).normalize(), (wrist - elbow).normalize_or(th - tl));
        let k = (wrist - elbow).length() / (th - tl).length();
        for (i, b) in t.bones.iter().enumerate() {
            if is_finger(b.name) && b.name.ends_with(side) {
                out[i] = wrist + rot * (b.pos - th) * k;
            }
        }
        // legs
        let tt = t.pos(&n("thigh"));
        let hip_w = (m.knee_l.x - m.knee_r.x).abs() / (TEMPLATE.knee_l.x - TEMPLATE.knee_r.x);
        let ty = map_y(tt.y);
        put(&mut out, 
            &n("thigh"),
            Vec3::new(cx + tt.x * hip_w, ty, torso_z("thigh", cx + tt.x * hip_w, ty)),
        );
        put(&mut out, &n("calf"), knee);
        let leg_s = (knee.y - lo.y) / TEMPLATE.knee_l.y;
        let tf = t.pos(&n("foot"));
        let fy = lo.y + tf.y * leg_s;
        let fx = sil
            .as_ref()
            .and_then(|sil| sil.run_at(fy, knee.x))
            .map(|(a, b)| (a + b) * 0.5)
            .filter(|x| (x - knee.x).abs() < 0.1 * h)
            .unwrap_or(knee.x);
        let mut foot = Vec3::new(fx, fy, mid_z(fx, fy, knee.z));
        // toes: the foot's furthest-forward point near the ground
        let toe_z = model
            .meshes
            .iter()
            .flat_map(|m| m.positions.iter())
            .filter(|p| p.y < lo.y + 0.06 * h && (p.x - fx).abs() < 0.07 * h)
            .map(|p| p.z)
            .fold(f32::MIN, f32::max);
        let tb = t.pos(&n("ball"));
        let tbl = t.pos(&n("ball_leaf"));
        // (the template's ankle is 0.241 m behind its toe tip, the toe joint 0.013 m)
        let leaf = if toe_z > foot.z {
            foot.z = toe_z - 0.2412 * leg_s;
            Vec3::new(fx, lo.y + tbl.y * leg_s, toe_z - 0.0133 * leg_s)
        } else {
            foot + (tbl - tf) * leg_s
        };
        put(&mut out, &n("foot"), foot);
        put(&mut out, &n("ball_leaf"), leaf);
        let along = (tb.z - tf.z) / (tbl.z - tf.z);
        let ball = foot.lerp(leaf, along);
        put(&mut out, &n("ball"), Vec3::new(ball.x, lo.y + tb.y * leg_s, ball.z));
    }
    out
}

/// How far (z) in front of the back the template's spine joints sit (`rig::tests::calibrate`).
fn back_offset(name: &str) -> Option<f32> {
    match name {
        "spine_01" => Some(0.0878),
        "spine_02" => Some(0.0908),
        "spine_03" => Some(0.1471),
        _ => None,
    }
}

/// How far (z) the template's joint sits from the middle of its body's depth at that point
/// (`rig::tests::calibrate`): the spine runs behind the middle at the hips, in front of it at
/// the neck.
fn depth_offset(name: &str) -> f32 {
    match name {
        "pelvis" => -0.0439,
        "spine_01" => -0.0216,
        "spine_02" => -0.0185,
        "spine_03" => 0.0073,
        "neck_01" => 0.0382,
        "Head" => 0.0052,
        "clavicle" => 0.1019,
        _ => 0.0,
    }
}

fn put(out: &mut [Vec3], name: &str, p: Vec3) {
    out[template().idx(name)] = p;
}

fn is_finger(name: &str) -> bool {
    ["thumb_", "index_", "middle_", "ring_", "pinky_"]
        .iter()
        .any(|f| name.starts_with(f))
}

/// The character's skeleton for joint positions `p` (template order): template rotations,
/// each turned onto the character's own bone direction; bound to that rest pose.
pub fn skeleton(p: &[Vec3]) -> Skeleton {
    let t = template();
    let mut turn: Vec<Quat> = vec![Quat::IDENTITY; t.bones.len()];
    for (i, b) in t.bones.iter().enumerate() {
        let inherited = b.parent.map(|p| turn[p]).unwrap_or(Quat::IDENTITY);
        turn[i] = primary_child(b.name)
            .and_then(|c| t.find(c))
            .and_then(|c| {
                let dt = t.bones[c].pos - b.pos;
                let dc = p[c] - p[i];
                (dt.length() > 1e-5 && dc.length() > 1e-5)
                    .then(|| Quat::from_rotation_arc(dt.normalize(), dc.normalize()))
            })
            .unwrap_or(inherited);
    }
    let rows: Vec<_> = t
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.to_string(), b.parent, p[i], (turn[i] * b.rot).normalize()))
        .collect();
    humanoid::skeleton_from_world(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_joints_rebuild_the_template() {
        let t = template();
        let p: Vec<Vec3> = t.bones.iter().map(|b| b.pos).collect();
        let sk = skeleton(&p);
        let w = sk.rest_world();
        for (i, b) in t.bones.iter().enumerate() {
            let (_, r, tr) = w[i].to_scale_rotation_translation();
            assert!((tr - b.pos).length() < 1e-4, "{}", b.name);
            assert!(r.dot(b.rot).abs() > 0.9999, "{}", b.name);
        }
    }
}
