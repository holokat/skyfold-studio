//! The whole automatic rig in one call, and linear blend skinning (posing a rigged model on the
//! CPU: checks, tests and picking).

use glam::{Mat4, Vec3};

use crate::fit;
use crate::markers::{self, Markers};
use crate::model::{Model, Skeleton, Xform};
use crate::retarget::world_of;
use crate::skin::{self, SkinOpts, SkinReport};

/// Rigs `model` (stood up with `markers::normalize`) with the standard humanoid skeleton:
/// fits it to `m` (guessed when `None`) and computes skin weights. Any rig and clips the model
/// came with are replaced.
pub fn auto_rig(model: &mut Model, m: Option<Markers>, opts: SkinOpts) -> (Markers, SkinReport) {
    let m = m.unwrap_or_else(|| markers::guess(model));
    let sk = fit::skeleton(&fit::joints(model, &m));
    model.skeleton = None;
    model.clips.clear();
    for mesh in &mut model.meshes {
        mesh.skin = None;
    }
    let report = skin::skin(model, &sk, opts);
    model.skeleton = Some(sk);
    (m, report)
}

/// Skinning matrices (model space) for a local pose of `sk`.
pub fn skin_matrices(sk: &Skeleton, pose: &[Xform]) -> Vec<Mat4> {
    world_of(sk, pose)
        .into_iter()
        .zip(&sk.bones)
        .map(|(w, b)| w * b.inverse_bind)
        .collect()
}

/// Every mesh's vertices in `pose` (unskinned meshes stay put).
pub fn deform(model: &Model, pose: &[Xform]) -> Vec<Vec<Vec3>> {
    let Some(sk) = &model.skeleton else {
        return model.meshes.iter().map(|m| m.positions.clone()).collect();
    };
    let mats = skin_matrices(sk, pose);
    model
        .meshes
        .iter()
        .map(|m| match &m.skin {
            Some(skin) => m
                .positions
                .iter()
                .zip(skin)
                .map(|(p, s)| {
                    let mut out = Vec3::ZERO;
                    for k in 0..4 {
                        if s.weights[k] > 0.0 {
                            out += mats[s.joints[k] as usize].transform_point3(*p) * s.weights[k];
                        }
                    }
                    out
                })
                .collect(),
            None => m.positions.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid::template;
    use crate::retarget::{PlayOpts, Retarget};
    use std::path::Path;

    const UAL: &str = "/Users/k/Documents/Codex/2026-09-05/http-127-0-0-1-4174/work/external-animations/UAL1_Standard.glb";

    fn ual() -> Option<Model> {
        Path::new(UAL)
            .exists()
            .then(|| crate::import::load(Path::new(UAL)).expect("UAL loads"))
    }

    /// Prints the template's own markers (paste into `markers::TEMPLATE`).
    #[test]
    #[ignore]
    fn calibrate() {
        let Some(mut m) = ual() else { return };
        let n = markers::normalize(&mut m, false);
        let n_shift = n.transform.w_axis.truncate();
        let g = markers::guess(&m);
        println!("{g:#?}");
        let sil = markers::Silhouette::new(&m, 240).unwrap();
        let solid = crate::skin::Solid::new(&m, markers::SOLID_RES).unwrap();
        // (the template's positions are before `normalize`: shift them the same way)
        let shift = m.bounds().unwrap();
        let _ = shift;
        for name in ["pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "Head", "clavicle_l", "upperarm_l", "thigh_l"] {
            let p = template().pos(name) + n_shift;
            let x = if name.ends_with("_l") { p.x } else { 0.0 };
            let (a, b) = solid.depth_near(x, p.y, g.groin.z).unwrap();
            println!("solid mid (\"{name}\", {:.4}),  back {:.4}", p.z - (a + b) * 0.5, p.z - a);
        }
        let h = 1.8292f32;
        for name in ["pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "Head", "clavicle_l", "upperarm_l", "thigh_l"] {
            let p = template().pos(name);
            let x = if name.ends_with("_l") { p.x } else { 0.0 };
            let mid = markers::depth_mid(&m, x, p.y, 0.025 * h).unwrap();
            println!("(\"{name}\", {:.4}),", p.z - mid);
        }
        for name in ["spine_01", "spine_02", "spine_03"] {
            let p = template().pos(name);
            let (back, _) = markers::depth_range(&m, 0.0, p.y, 0.025 * h).unwrap();
            println!("back (\"{name}\", {:.4}),", p.z - back);
        }
        for side in ["l"] {
            let sh = template().pos(&format!("upperarm_{side}"));
            let el = template().pos(&format!("lowerarm_{side}"));
            let q = sh + (el - sh) * 0.35;
            let mid = markers::depth_mid(&m, q.x, q.y, 0.025 * h).unwrap();
            println!("arm shoulder offset {:.4}", sh.z - mid);
        }
        for left in [true, false] {
            let el = if left { g.elbow_l } else { g.elbow_r };
            if let Some((line, t)) = sil.trace_arm(el.truncate(), g.groin.x) {
                let root = *line.last().unwrap();
                let tpl = template().pos(if left { "upperarm_l" } else { "upperarm_r" });
                println!("arm root {:?} thickness {t:.4}; template shoulder {:?} → SHOULDER_OUT {:.3}", root, tpl, (tpl.x.abs() - root.x.abs()) / t);
            }
        }
        let toe = m.meshes.iter().flat_map(|mm| mm.positions.iter()).filter(|p| p.y < 0.06 * h && p.x > 0.0).map(|p| p.z).fold(f32::MIN, f32::max);
        println!("toe z {toe:.4} foot z {:.4} leaf z {:.4}", template().pos("foot_l").z, template().pos("ball_leaf_l").z);
        println!(
            "torso half {}",
            sil.torso_half_width(g.groin.y, g.chin.y, g.groin.x)
        );
    }

    /// Re-rigs the UAL mannequin from scratch and compares how it deforms in real clips with
    /// its hand-made weights.
    #[test]
    fn rerig_mannequin_matches_original() {
        let Some(mut orig) = ual() else { return };
        markers::normalize(&mut orig, true);
        let clips = orig.clips.clone();
        check(&orig, &orig, &clips, "male", 0.02);
    }

    /// The female mannequin: other proportions, its own hand-made weights. Its shipped rig is
    /// the male skeleton unchanged (same joint heights and depths), so our fitted skeleton
    /// deforms it a few cm differently (most in a deep crouch, where its groin sits 3 cm higher
    /// than the male's over the same hips); the weights themselves, on its own skeleton, must
    /// match closely.
    #[test]
    fn rerig_female_mannequin() {
        let p = Path::new(UAL).with_file_name("Mannequin_F.glb");
        let (Some(male), true) = (ual(), p.exists()) else { return };
        let mut orig = crate::import::load(&p).unwrap();
        markers::normalize(&mut orig, true);
        check(&orig, &orig, &male.clips, "female", 0.08);
        weights_only(&orig, &male.clips, "female", 0.012);
    }

    /// The mannequin re-posed with its arms 40° down (an A-pose), rigged from that, must still
    /// play T-pose clips like the original.
    #[test]
    fn rerig_a_pose() {
        let Some(mut orig) = ual() else { return };
        markers::normalize(&mut orig, true);
        let sk = orig.skeleton.clone().unwrap();
        let mut pose: Vec<Xform> = sk.bones.iter().map(|b| b.rest).collect();
        let w = sk.rest_world();
        for (name, a) in [("upperarm_l", -0.7f32), ("upperarm_r", 0.7)] {
            let b = sk.find(name).unwrap();
            let pw = w[sk.bones[b].parent.unwrap()].to_scale_rotation_translation().1;
            pose[b].r = (pw.inverse() * glam::Quat::from_rotation_z(a) * pw * pose[b].r).normalize();
        }
        let mut posed = orig.clone();
        for (m, p) in posed.meshes.iter_mut().zip(deform(&orig, &pose)) {
            m.positions = p;
            m.normals.clear();
            m.skin = None;
        }
        posed.ensure_normals();
        posed.skeleton = None;
        let clips = orig.clips.clone();
        check(&orig, &posed, &clips, "a-pose", 0.025);
    }

    /// A real Mixamo character (clothes, separate eyes and hair, 1.77 m): rigged here, it
    /// must deform like Mixamo's own rig does under the same clips.
    #[test]
    fn rerig_mixamo_character() {
        let p = Path::new(
            "/Users/k/code/character-studio/data/assets/cd513462-2f27-416b-af4d-09d4e3542be2.fbx",
        );
        let (Some(male), true) = (ual(), p.exists()) else { return };
        let mut orig = crate::import::load(p).unwrap();
        let n = markers::normalize(&mut orig, true);
        assert_eq!(n.quarter_turns, 0);
        // (Mixamo's hips joint sits ~0.15 m higher than the Unreal skeleton's pelvis, so the
        // whole-rig comparison only guards against regressions; the weights must match)
        check(&orig, &orig, &male.clips, "mixamo", 0.16);
        weights_only(&orig, &male.clips, "mixamo", 0.015);
    }

    /// Our weights put on `orig`'s own skeleton (each of our bones → its bone of the same
    /// canonical name, else the nearest such ancestor): it must deform like `orig`.
    fn weights_only(orig: &Model, clips: &[crate::model::Clip], label: &str, tol: f32) {
        let mut ours = orig.clone();
        auto_rig(&mut ours, None, SkinOpts::default());
        let osk = orig.skeleton.clone().unwrap();
        let nsk = ours.skeleton.clone().unwrap();
        let omap = crate::humanoid::map_bones(&osk);
        let to_theirs = |mut j: usize| -> u16 {
            loop {
                let n = nsk.bones[j].name.as_str();
                if let Some(k) = omap.iter().position(|m| *m == Some(n)) {
                    return k as u16;
                }
                match nsk.bones[j].parent {
                    Some(p) => j = p,
                    None => return 0,
                }
            }
        };
        let mut w_only = orig.clone();
        for (mo, mn) in w_only.meshes.iter_mut().zip(&ours.meshes) {
            mo.skin = mn.skin.as_ref().map(|s| {
                s.iter()
                    .map(|v| {
                        let mut v = *v;
                        for j in &mut v.joints {
                            *j = to_theirs(*j as usize);
                        }
                        v
                    })
                    .collect()
            });
        }
        let r = Retarget::new(&template().skeleton(), &osk);
        for name in ["Walk_Loop", "Sword_Attack", "Roll"] {
            let clip = clips.iter().find(|c| c.name == name).unwrap();
            let pose = r.pose(clip, clip.duration() * 0.5, PlayOpts::default());
            let (a, b) = (deform(orig, &pose), deform(&w_only, &pose));
            let (mut e, mut c) = (0.0, 0.0);
            for (ma, mb) in a.iter().zip(&b) {
                for (pa, pb) in ma.iter().zip(mb) {
                    e += (*pa - *pb).length();
                    c += 1.0;
                }
            }
            println!("  {label} {name}: weights only, mean {:.4} m", e / c);
            assert!(e / c < tol, "{label} weights {name}: {}", e / c);
        }
    }

    /// What an export writes: a rigged character with library clips baked onto it, as one GLB
    /// that reads back with the same clips (and that the glTF crate validates).
    #[test]
    fn baked_export_round_trip() {
        let p = Path::new(UAL).with_file_name("Mannequin_F.glb");
        let (Some(male), true) = (ual(), p.exists()) else { return };
        let mut m = crate::import::load(&p).unwrap();
        markers::normalize(&mut m, true);
        auto_rig(&mut m, None, SkinOpts::default());
        let sk = m.skeleton.clone().unwrap();
        let r = Retarget::new(male.skeleton.as_ref().unwrap(), &sk);
        let clips: Vec<_> = ["Walk_Loop", "Sword_Attack", "Jump_Start"]
            .iter()
            .map(|n| {
                let c = male.clips.iter().find(|c| c.name == *n).unwrap();
                r.bake(c, PlayOpts { looped: false, in_place: true })
            })
            .collect();
        let bytes = crate::glb::write(&m, &clips);
        gltf::import_slice(&bytes).expect("valid glTF");
        let tmp = std::env::temp_dir().join("autorig_baked_export.glb");
        std::fs::write(&tmp, &bytes).unwrap();
        let back = crate::import::load(&tmp).unwrap();
        assert_eq!(back.clips.len(), 3);
        for (a, b) in clips.iter().zip(&back.clips) {
            assert_eq!(a.name, b.name);
            assert!((a.frames as i32 - b.frames as i32).abs() <= 1);
        }
        // the baked walk poses the character as the live retarget does
        let t = clips[0].duration() * 0.5;
        let live = deform(&m, &r.pose(&male.clips.iter().find(|c| c.name == "Walk_Loop").unwrap(), t, PlayOpts { looped: false, in_place: true }));
        let baked = deform(&back, &crate::retarget::sample_local(back.skeleton.as_ref().unwrap(), &back.clips[0], t, false));
        let (mut e, mut n) = (0.0, 0.0);
        for (ma, mb) in live.iter().zip(&baked) {
            for (pa, pb) in ma.iter().zip(mb) {
                e += (*pa - *pb).length();
                n += 1.0;
            }
        }
        println!("baked vs live: {:.4} m", e / n);
        assert!(e / n < 0.005);
        let _ = std::fs::remove_file(tmp);
    }

    /// Rigs `input` and plays `clips` on it next to `orig` (rigged by hand): the mean vertex
    /// distance must stay under `tol`.
    fn check(orig: &Model, input: &Model, clips: &[crate::model::Clip], label: &str, tol: f32) {
        let mut m = input.clone();
        let (mk, rep) = auto_rig(&mut m, None, SkinOpts::default());
        println!("{label}: {rep:?} chin {:?} groin {:?} wrist_l {:?} elbow_l {:?}", mk.chin, mk.groin, mk.wrist_l, mk.elbow_l);
        if let Some(sk) = &m.skeleton {
            let w = sk.rest_world();
            for n in ["upperarm_l", "lowerarm_l", "hand_l", "clavicle_l"] {
                println!("  fitted {n} {:?}", w[sk.find(n).unwrap()].w_axis.truncate());
            }
        }
        let sk = m.skeleton.as_ref().unwrap();
        let src_full = template().skeleton();
        let ours = Retarget::new(&src_full, sk);
        let theirs = Retarget::new(&src_full, orig.skeleton.as_ref().unwrap());
        for name in ["Walk_Loop", "Sword_Attack", "Punch_Cross", "Crouch_Idle_Loop", "Roll"] {
            let clip = clips.iter().find(|c| c.name == name).unwrap();
            let (mut mean, mut max, mut count) = (0.0, 0.0f32, 0.0);
            for f in [0.25f32, 0.5, 0.75] {
                let t = clip.duration() * f;
                let a = deform(orig, &theirs.pose(clip, t, PlayOpts::default()));
                let b = deform(&m, &ours.pose(clip, t, PlayOpts::default()));
                for (ma, mb) in a.iter().zip(&b) {
                    for (pa, pb) in ma.iter().zip(mb) {
                        let e = (*pa - *pb).length();
                        mean += e;
                        max = max.max(e);
                        count += 1.0;
                    }
                }
            }
            println!("  {name}: mean {:.4} m, max {:.3} m", mean / count, max);
            assert!(mean / count < tol, "{label} {name} mean {}", mean / count);
        }
    }
}


