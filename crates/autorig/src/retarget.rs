//! Plays a clip made for one skeleton on another: bones are matched through their canonical
//! humanoid names (`humanoid::map_bones`), and every matched bone copies the source bone's
//! model-space rotation *relative to its rest pose*, after the target's rest pose is turned to
//! face the same way as the source's (a T-posed clip on an A-posed character still lowers its
//! arms to the right place). Bone lengths stay the target's own; the pelvis' travel is scaled
//! by the ratio of hip heights.
//!
//! Per matched target bone `b` with source bone `s`:
//! `W_t(b, time) = W_s(s, time) · C_b`, `C_b = W_s(s, rest)⁻¹ · A_b · W_t(b, rest)`,
//! where `A_b` turns the target's rest bone direction onto the source's.

use glam::{Mat4, Quat, Vec3};

use crate::humanoid::{self, primary_child};
use crate::model::{Clip, Skeleton, Track, Xform};

/// Samples `clip` on `sk` at `time` (seconds): every bone's local transform (the rest pose
/// where the clip has no channel).
pub fn sample_local(sk: &Skeleton, clip: &Clip, time: f32, looped: bool) -> Vec<Xform> {
    let mut out: Vec<Xform> = sk.bones.iter().map(|b| b.rest).collect();
    if clip.frames == 0 {
        return out;
    }
    let (i0, i1, f) = frame_at(clip, time, looped);
    for tr in &clip.tracks {
        let Some(x) = out.get_mut(tr.bone) else {
            continue;
        };
        if tr.rot.len() == clip.frames {
            x.r = nlerp_short(tr.rot[i0], tr.rot[i1], f);
        }
        if tr.pos.len() == clip.frames {
            x.t = tr.pos[i0].lerp(tr.pos[i1], f);
        }
        if tr.scale.len() == clip.frames {
            x.s = tr.scale[i0].lerp(tr.scale[i1], f);
        }
    }
    out
}

fn frame_at(clip: &Clip, time: f32, looped: bool) -> (usize, usize, f32) {
    let n = clip.frames;
    if n < 2 {
        return (0, 0, 0.0);
    }
    let d = clip.duration().max(1e-6);
    let t = if looped {
        time.rem_euclid(d)
    } else {
        time.clamp(0.0, d)
    };
    let x = t * clip.fps;
    let i0 = (x.floor() as usize).min(n - 1);
    let i1 = (i0 + 1).min(n - 1);
    (i0, i1, x - i0 as f32)
}

fn nlerp_short(a: Quat, b: Quat, f: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    a.slerp(b, f).normalize()
}

/// Model-space matrices from local transforms.
pub fn world_of(sk: &Skeleton, local: &[Xform]) -> Vec<Mat4> {
    let mut out: Vec<Mat4> = Vec::with_capacity(local.len());
    for (b, x) in sk.bones.iter().zip(local) {
        let m = x.matrix();
        out.push(match b.parent {
            Some(p) => out[p] * m,
            None => m,
        });
    }
    out
}

fn rot_of(m: &Mat4) -> Quat {
    m.to_scale_rotation_translation().1.normalize()
}

/// How a clip is played.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlayOpts {
    pub looped: bool,
    /// keep the pelvis over its start: no travel across the ground (vertical bob stays)
    pub in_place: bool,
}

/// A source skeleton → target skeleton binding, built once and reused for every clip.
pub struct Retarget {
    source: Skeleton,
    target: Skeleton,
    /// per target bone: the source bone it follows
    pub map: Vec<Option<usize>>,
    corr: Vec<Quat>,
    pelvis: Option<(usize, usize)>,
    src_pelvis_rest: Vec3,
    tgt_pelvis_rest: Vec3,
    scale: f32,
}

impl Retarget {
    pub fn new(source: &Skeleton, target: &Skeleton) -> Self {
        let sm = humanoid::map_bones(source);
        let tm = humanoid::map_bones(target);
        let src_of = |canon: &str| sm.iter().position(|m| *m == Some(canon));
        let map: Vec<Option<usize>> = tm.iter().map(|c| c.and_then(src_of)).collect();
        let sw = source.rest_world();
        let tw = target.rest_world();
        let tgt_of = |canon: &str| tm.iter().position(|m| *m == Some(canon));
        // A_b: the target's rest direction turned onto the source's (inherited down the chain
        // where a bone has no mapped child)
        let mut align: Vec<Quat> = vec![Quat::IDENTITY; target.bones.len()];
        for (b, bone) in target.bones.iter().enumerate() {
            let inherited = bone.parent.map(|p| align[p]).unwrap_or(Quat::IDENTITY);
            let own = (|| {
                let s = map[b]?;
                let canon = tm[b]?;
                let child = primary_child(canon)?;
                let (tc, sc) = (tgt_of(child)?, src_of(child)?);
                let dt = (tw[tc].w_axis - tw[b].w_axis).truncate();
                let ds = (sw[sc].w_axis - sw[s].w_axis).truncate();
                (dt.length() > 1e-5 && ds.length() > 1e-5)
                    .then(|| Quat::from_rotation_arc(dt.normalize(), ds.normalize()))
            })();
            align[b] = own.unwrap_or(inherited);
        }
        let corr = (0..target.bones.len())
            .map(|b| match map[b] {
                Some(s) => (rot_of(&sw[s]).inverse() * align[b] * rot_of(&tw[b])).normalize(),
                None => Quat::IDENTITY,
            })
            .collect();
        let pelvis = tgt_of("pelvis").zip(src_of("pelvis"));
        let (src_pelvis_rest, tgt_pelvis_rest) = pelvis
            .map(|(t, s)| (sw[s].w_axis.truncate(), tw[t].w_axis.truncate()))
            .unwrap_or_default();
        let scale = if src_pelvis_rest.y > 0.01 && tgt_pelvis_rest.y > 0.01 {
            tgt_pelvis_rest.y / src_pelvis_rest.y
        } else {
            1.0
        };
        Self {
            source: source.clone(),
            target: target.clone(),
            map,
            corr,
            pelvis,
            src_pelvis_rest,
            tgt_pelvis_rest,
            scale,
        }
    }

    pub fn mapped(&self) -> usize {
        self.map.iter().filter(|m| m.is_some()).count()
    }

    /// The target's local pose at `time`.
    pub fn pose(&self, clip: &Clip, time: f32, opts: PlayOpts) -> Vec<Xform> {
        let src_local = sample_local(&self.source, clip, time, opts.looped);
        let sw = world_of(&self.source, &src_local);
        let pelvis_pos = self.pelvis.map(|(_, s)| {
            let mut d = sw[s].w_axis.truncate() - self.src_pelvis_rest;
            if opts.in_place {
                let start = world_of(&self.source, &sample_local(&self.source, clip, 0.0, false));
                let d0 = start[s].w_axis.truncate() - self.src_pelvis_rest;
                d.x = d0.x;
                d.z = d0.z;
            }
            self.tgt_pelvis_rest + d * self.scale
        });
        let mut world: Vec<Mat4> = Vec::with_capacity(self.target.bones.len());
        let mut out = Vec::with_capacity(self.target.bones.len());
        for (b, bone) in self.target.bones.iter().enumerate() {
            let parent = bone.parent.map(|p| world[p]).unwrap_or(Mat4::IDENTITY);
            let mut x = bone.rest;
            if let Some(s) = self.map[b] {
                let w = rot_of(&sw[s]) * self.corr[b];
                x.r = (rot_of(&parent).inverse() * w).normalize();
            }
            if let (Some((tp, _)), Some(p)) = (self.pelvis, pelvis_pos)
                && tp == b
            {
                // (the parent's scale applies to the local offset)
                let local = parent.inverse().transform_point3(p);
                x.t = local;
            }
            let m = parent * x.matrix();
            world.push(m);
            out.push(x);
        }
        out
    }

    /// The clip re-made for the target skeleton (every bone's rotation, the pelvis' position).
    pub fn bake(&self, clip: &Clip, opts: PlayOpts) -> Clip {
        let n = self.target.bones.len();
        let mut tracks: Vec<Track> = (0..n)
            .map(|b| Track {
                bone: b,
                rot: Vec::with_capacity(clip.frames),
                pos: vec![],
                scale: vec![],
            })
            .collect();
        let pelvis = self.pelvis.map(|(t, _)| t);
        for f in 0..clip.frames {
            let pose = self.pose(clip, f as f32 / clip.fps, PlayOpts { looped: false, ..opts });
            for (b, x) in pose.iter().enumerate() {
                tracks[b].rot.push(x.r);
                if Some(b) == pelvis {
                    tracks[b].pos.push(x.t);
                }
            }
        }
        // drop bones that never move
        tracks.retain(|t| {
            !t.pos.is_empty()
                || t.rot
                    .iter()
                    .any(|q| q.dot(self.target.bones[t.bone].rest.r).abs() < 0.99999)
        });
        Clip {
            name: clip.name.clone(),
            fps: clip.fps,
            frames: clip.frames,
            tracks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid::template;

    /// A clip that holds `bone` at `rot` (local) for two frames.
    fn hold(sk: &Skeleton, bone: &str, rot: Quat) -> Clip {
        let b = sk.find(bone).unwrap();
        Clip {
            name: "hold".into(),
            fps: 30.0,
            frames: 2,
            tracks: vec![Track {
                bone: b,
                rot: vec![rot; 2],
                pos: vec![],
                scale: vec![],
            }],
        }
    }

    #[test]
    fn identity_on_same_skeleton() {
        let sk = template().skeleton();
        let r = Retarget::new(&sk, &sk);
        assert_eq!(r.mapped(), 65);
        let b = sk.find("lowerarm_l").unwrap();
        let q = sk.bones[b].rest.r * Quat::from_rotation_z(0.7);
        let clip = hold(&sk, "lowerarm_l", q);
        let pose = r.pose(&clip, 0.0, PlayOpts::default());
        assert!(pose[b].r.dot(q).abs() > 0.9999);
        let w = world_of(&sk, &pose);
        let src = world_of(&sk, &sample_local(&sk, &clip, 0.0, false));
        let h = sk.find("hand_l").unwrap();
        assert!((w[h].w_axis - src[h].w_axis).length() < 1e-4);
    }

    #[test]
    fn a_pose_target_follows_t_pose_source() {
        // target: the template with its arms lowered 45° (an A-pose rest)
        let t = template();
        let mut rows: Vec<(String, Option<usize>, Vec3, Quat)> = t
            .bones
            .iter()
            .map(|b| (b.name.to_string(), b.parent, b.pos, b.rot))
            .collect();
        for (side, sign) in [("l", -1.0f32), ("r", 1.0)] {
            let sh = t.pos(&format!("upperarm_{side}"));
            let turn = Quat::from_rotation_z(sign * std::f32::consts::FRAC_PI_4);
            for r in rows.iter_mut() {
                let under = {
                    let mut k = t.find(&r.0);
                    let mut yes = false;
                    while let Some(i) = k {
                        if t.bones[i].name == format!("upperarm_{side}") {
                            yes = true;
                            break;
                        }
                        k = t.bones[i].parent;
                    }
                    yes
                };
                if under {
                    r.2 = sh + turn * (r.2 - sh);
                    r.3 = turn * r.3;
                }
            }
        }
        let target = humanoid::skeleton_from_world(&rows);
        let source = t.skeleton();
        let r = Retarget::new(&source, &target);
        // the source's rest pose: the target's arms come up to horizontal
        let rest = Clip {
            name: "rest".into(),
            fps: 30.0,
            frames: 1,
            tracks: vec![],
        };
        let pose = r.pose(&rest, 0.0, PlayOpts::default());
        let w = world_of(&target, &pose);
        let h = target.find("hand_l").unwrap();
        let s = target.find("upperarm_l").unwrap();
        let d = (w[h].w_axis - w[s].w_axis).truncate().normalize();
        assert!(d.dot(Vec3::X) > 0.999, "{d}");
    }
}
