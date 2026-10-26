//! The standard humanoid skeleton every character is rigged with: the 65 bones of the Unreal 5
//! mannequin, as the CC0 Quaternius Universal Animation Library ships them (so its clips, Unreal's
//! own and anything on Manny play with no bone map at all). The template is that skeleton's
//! T-pose: 1.83 m tall, facing +Z, feet on y = 0 (`humanoid_template.json`, generated from
//! `UAL1_Standard.glb`).
//!
//! `map_bones` names the canonical bone of every bone of another skeleton (Mixamo, VRoid, Unity
//! humanoid, Rigify-style, UE4/UE5), which is what retargeting works from.

use std::sync::OnceLock;

use glam::{Quat, Vec3};
use serde::Deserialize;

use crate::model::{Bone, Skeleton, Xform};

#[derive(Deserialize)]
struct Row {
    name: String,
    parent: Option<usize>,
    pos: [f32; 3],
    rot: [f32; 4],
}

/// One template bone in model space.
#[derive(Clone, Debug)]
pub struct TemplateBone {
    pub name: &'static str,
    pub parent: Option<usize>,
    pub pos: Vec3,
    pub rot: Quat,
}

pub struct Template {
    pub bones: Vec<TemplateBone>,
}

pub fn template() -> &'static Template {
    static T: OnceLock<Template> = OnceLock::new();
    T.get_or_init(|| {
        let rows: Vec<Row> = serde_json::from_str(include_str!("humanoid_template.json"))
            .expect("humanoid_template.json");
        Template {
            bones: rows
                .into_iter()
                .map(|r| TemplateBone {
                    name: Box::leak(r.name.into_boxed_str()),
                    parent: r.parent,
                    pos: Vec3::from(r.pos),
                    rot: Quat::from_array(r.rot).normalize(),
                })
                .collect(),
        }
    })
}

impl Template {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }
    pub fn idx(&self, name: &str) -> usize {
        self.find(name)
            .unwrap_or_else(|| panic!("no template bone {name}"))
    }
    pub fn pos(&self, name: &str) -> Vec3 {
        self.bones[self.idx(name)].pos
    }
    /// The template as a skeleton (its rest pose = the T-pose, bound to it).
    pub fn skeleton(&self) -> Skeleton {
        skeleton_from_world(
            &self
                .bones
                .iter()
                .map(|b| (b.name.to_string(), b.parent, b.pos, b.rot))
                .collect::<Vec<_>>(),
        )
    }
}

/// A skeleton from model-space joint positions and rotations (parents before children).
pub fn skeleton_from_world(rows: &[(String, Option<usize>, Vec3, Quat)]) -> Skeleton {
    let mut sk = Skeleton {
        bones: rows
            .iter()
            .map(|(name, parent, pos, rot)| {
                let rest = match parent {
                    Some(p) => {
                        let (pp, pr) = (rows[*p].2, rows[*p].3);
                        let inv = pr.inverse();
                        Xform {
                            t: inv * (*pos - pp),
                            r: (inv * *rot).normalize(),
                            s: Vec3::ONE,
                        }
                    }
                    None => Xform {
                        t: *pos,
                        r: *rot,
                        s: Vec3::ONE,
                    },
                };
                Bone {
                    name: name.clone(),
                    parent: *parent,
                    rest,
                    inverse_bind: glam::Mat4::IDENTITY,
                }
            })
            .collect(),
    };
    sk.bind_to_rest();
    sk
}

/// The child a bone points at (its direction), for every bone that has one.
pub fn primary_child(name: &str) -> Option<&'static str> {
    Some(match name {
        "root" => "pelvis",
        "pelvis" => "spine_01",
        "spine_01" => "spine_02",
        "spine_02" => "spine_03",
        "spine_03" => "neck_01",
        "neck_01" => "Head",
        _ => {
            let (stem, side) = split_side(name)?;
            let child = match stem {
                "clavicle" => "upperarm",
                "upperarm" => "lowerarm",
                "lowerarm" => "hand",
                "hand" => "middle_01",
                "thigh" => "calf",
                "calf" => "foot",
                "foot" => "ball",
                "ball" => "ball_leaf",
                s => {
                    // fingers: index_01 → index_02 … → index_04_leaf
                    let (finger, n) = s.rsplit_once('_')?;
                    match n {
                        "01" => return leak(format!("{finger}_02_{side}")),
                        "02" => return leak(format!("{finger}_03_{side}")),
                        "03" => return leak(format!("{finger}_04_leaf_{side}")),
                        _ => return None,
                    }
                }
            };
            return leak(format!("{child}_{side}"));
        }
    })
}

fn leak(s: String) -> Option<&'static str> {
    // (a fixed set of 65 names: interned once)
    static NAMES: OnceLock<std::sync::Mutex<Vec<&'static str>>> = OnceLock::new();
    let names = NAMES.get_or_init(Default::default);
    let mut v = names.lock().unwrap();
    if let Some(n) = v.iter().find(|n| **n == s) {
        return Some(n);
    }
    let n: &'static str = Box::leak(s.into_boxed_str());
    v.push(n);
    Some(n)
}

/// "upperarm_l" → ("upperarm", "l"); "index_04_leaf_r" → ("index_04_leaf", "r").
pub fn split_side(name: &str) -> Option<(&str, &str)> {
    let (stem, side) = name.rsplit_once('_')?;
    matches!(side, "l" | "r").then_some((stem, side))
}

/// Bones that never get skin weights: the ground root and the tips.
pub fn is_weightless(name: &str) -> bool {
    name == "root" || name.contains("_leaf")
}

/// The bones that must be found for a rig to count as humanoid.
pub const REQUIRED: [&str; 15] = [
    "pelvis", "spine_01", "Head", "upperarm_l", "lowerarm_l", "hand_l", "upperarm_r",
    "lowerarm_r", "hand_r", "thigh_l", "calf_l", "foot_l", "thigh_r", "calf_r", "foot_r",
];

// ------------------------------------------------------------------ bone name mapping

/// For every bone of `sk`, the canonical bone it plays (or `None`). Each canonical bone is used
/// at most once. Exact names first (UE/UAL, Mixamo, VRoid, Unity humanoid), then keywords.
pub fn map_bones(sk: &Skeleton) -> Vec<Option<&'static str>> {
    let t = template();
    let mut out: Vec<Option<&'static str>> = vec![None; sk.bones.len()];
    let mut used = std::collections::HashSet::new();
    // (Skyfold's own moving-part bones, `sf_…`, never play a body bone)
    let ours: Vec<bool> = sk.bones.iter().map(|b| b.name.starts_with("sf_")).collect();
    // 1: exact (case-insensitive) canonical names
    for (i, b) in sk.bones.iter().enumerate() {
        let n = strip_prefix(&b.name);
        if let Some(tb) = t.bones.iter().find(|tb| tb.name.eq_ignore_ascii_case(n))
            && used.insert(tb.name)
        {
            out[i] = Some(tb.name);
        }
    }
    // 2: known schemes
    for (i, b) in sk.bones.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        if let Some(c) = scheme_name(strip_prefix(&b.name))
            && let Some(c) = t.find(&c).map(|k| t.bones[k].name)
            && used.insert(c)
        {
            out[i] = Some(c);
        }
    }
    // 3: keywords, with the spine spread over its chain
    keyword_pass(sk, &mut out, &mut used);
    for (o, ours) in out.iter_mut().zip(ours) {
        if ours {
            *o = None;
        }
    }
    out
}

/// "mixamorig:LeftArm" → "LeftArm", "Armature|DEF-spine" → "spine", "J_Bip_L_UpperArm" stays.
fn strip_prefix(name: &str) -> &str {
    let n = name.rsplit(['|', ':']).next().unwrap_or(name);
    let n = n.strip_prefix("DEF-").unwrap_or(n);
    let n = n.strip_prefix("ORG-").unwrap_or(n);
    let n = n
        .strip_prefix("Bip01 ")
        .or_else(|| n.strip_prefix("Bip001 "))
        .or_else(|| n.strip_prefix("Bip01_"))
        .or_else(|| n.strip_prefix("Bip001_"))
        .unwrap_or(n);
    n
}

fn scheme_name(n: &str) -> Option<String> {
    let lower = n.to_ascii_lowercase();
    // VRoid: J_Bip_C_Hips, J_Bip_L_UpperArm, J_Bip_L_Index1
    if let Some(rest) = lower.strip_prefix("j_bip_") {
        let (side, part) = rest.split_once('_')?;
        let part = match side {
            "l" => format!("left{part}"),
            "r" => format!("right{part}"),
            _ => part.to_string(),
        };
        return mixamo_like(&part);
    }
    mixamo_like(&lower.replace(['_', ' ', '.', '-'], ""))
}

/// Mixamo / Unity humanoid style names (lower case, no separators).
fn mixamo_like(n: &str) -> Option<String> {
    let (side, part) = if let Some(p) = n.strip_prefix("left") {
        (Some("l"), p)
    } else if let Some(p) = n.strip_prefix("right") {
        (Some("r"), p)
    } else if let Some(p) = n.strip_suffix("left") {
        (Some("l"), p)
    } else if let Some(p) = n.strip_suffix("right") {
        (Some("r"), p)
    } else if let Some(p) = n.strip_suffix("l").filter(|p| p.len() > 2) {
        (Some("l"), p)
    } else if let Some(p) = n.strip_suffix("r").filter(|p| p.len() > 2) {
        (Some("r"), p)
    } else {
        (None, n)
    };
    let centre = match part {
        "hips" | "pelvis" | "hip" => Some("pelvis"),
        "spine" | "spine0" | "spine1" if side.is_none() && part == "spine" => Some("spine_01"),
        "spine1" | "chest" => Some("spine_02"),
        "spine2" | "upperchest" | "spine3" => Some("spine_03"),
        "neck" | "neck1" => Some("neck_01"),
        "head" => Some("Head"),
        _ => None,
    };
    if side.is_none() {
        return centre.map(str::to_string);
    }
    let s = side.unwrap();
    let limb = match part {
        "shoulder" | "clavicle" | "collar" => "clavicle",
        "arm" | "upperarm" => "upperarm",
        "forearm" | "lowerarm" | "elbow" => "lowerarm",
        "hand" | "wrist" => "hand",
        "upleg" | "upperleg" | "thigh" => "thigh",
        "leg" | "lowerleg" | "calf" | "shin" | "knee" => "calf",
        "foot" | "ankle" => "foot",
        "toebase" | "toes" | "toe" | "ball" => "ball",
        "toeend" | "toe_end" | "toesend" => "ball_leaf",
        f => {
            // fingers: handindex1 / index1 / indexproximal / thumb2 …
            let f = f.strip_prefix("hand").unwrap_or(f);
            for name in ["thumb", "index", "middle", "ring", "pinky", "little"] {
                if let Some(rest) = f.strip_prefix(name) {
                    let canon = if name == "little" { "pinky" } else { name };
                    let n = match rest {
                        "1" | "proximal" | "01" => "01",
                        "2" | "intermediate" | "02" => "02",
                        "3" | "distal" | "03" => "03",
                        "4" | "end" | "04" | "tip" => "04_leaf",
                        _ => return None,
                    };
                    return Some(format!("{canon}_{n}_{s}"));
                }
            }
            return None;
        }
    };
    Some(format!("{limb}_{s}"))
}

/// Keyword matching for anything still unmapped (e.g. "Leg_Upper.L", "arm_l_2").
fn keyword_pass(
    sk: &Skeleton,
    out: &mut [Option<&'static str>],
    used: &mut std::collections::HashSet<&'static str>,
) {
    let t = template();
    let side_of = |n: &str| -> Option<&'static str> {
        let n = n.to_ascii_lowercase();
        let ends = |s: &str| n.ends_with(s);
        if n.contains("left") || ends(".l") || ends("_l") || ends(" l") || n.starts_with("l_") {
            Some("l")
        } else if n.contains("right")
            || ends(".r")
            || ends("_r")
            || ends(" r")
            || n.starts_with("r_")
        {
            Some("r")
        } else {
            None
        }
    };
    let words: [(&[&str], &str); 9] = [
        (&["clavicle", "shoulder", "collar"], "clavicle"),
        (&["forearm", "lowerarm", "elbow"], "lowerarm"),
        (&["upperarm", "arm"], "upperarm"),
        (&["hand", "wrist", "palm"], "hand"),
        (&["thigh", "upleg", "upperleg"], "thigh"),
        (&["calf", "shin", "lowerleg", "knee", "leg"], "calf"),
        (&["foot", "ankle"], "foot"),
        (&["toe", "ball"], "ball"),
        (&["hips", "pelvis"], "pelvis"),
    ];
    for (i, b) in sk.bones.iter().enumerate() {
        if out[i].is_some() {
            continue;
        }
        let n = strip_prefix(&b.name).to_ascii_lowercase();
        if ["finger", "thumb", "index", "middle", "ring", "pinky", "twist", "roll", "ik", "pole"]
            .iter()
            .any(|w| n.contains(w))
        {
            continue;
        }
        // "Leg_Upper.L" → "upperleg"
        let flat = n
            .replace(['_', ' ', '.', '-'], "")
            .replace("legupper", "upperleg")
            .replace("leglower", "lowerleg")
            .replace("armupper", "upperarm")
            .replace("armlower", "lowerarm");
        for (keys, stem) in words {
            if keys.iter().any(|k| flat.contains(k)) {
                let canon = if stem == "pelvis" {
                    "pelvis".to_string()
                } else if let Some(s) = side_of(&b.name) {
                    format!("{stem}_{s}")
                } else {
                    break;
                };
                if let Some(c) = t.find(&canon).map(|k| t.bones[k].name)
                    && used.insert(c)
                {
                    out[i] = Some(c);
                }
                break;
            }
        }
        if out[i].is_none() && n == "head" && used.insert("Head") {
            out[i] = Some("Head");
        }
        if out[i].is_none() && n.contains("neck") && used.insert("neck_01") {
            out[i] = Some("neck_01");
        }
    }
    // spine: the unmapped bones on the chain from the pelvis to the neck / head, spread over
    // spine_01..03
    let find = |c: &str| out.iter().position(|o| *o == Some(c));
    let top = find("neck_01").or_else(|| find("Head"));
    if let (Some(pelvis), Some(top)) = (find("pelvis"), top) {
        let mut chain = vec![];
        let mut k = sk.bones[top].parent;
        while let Some(p) = k {
            if p == pelvis {
                break;
            }
            chain.push(p);
            k = sk.bones[p].parent;
        }
        if k == Some(pelvis) {
            chain.reverse();
            let chain: Vec<usize> = chain.into_iter().filter(|&i| out[i].is_none()).collect();
            let slots = ["spine_01", "spine_02", "spine_03"];
            let free: Vec<&str> = slots.iter().copied().filter(|s| !used.contains(s)).collect();
            if !chain.is_empty() && !free.is_empty() {
                for (j, s) in free.iter().enumerate() {
                    // spread: first, middle, last
                    let at = if free.len() == 1 {
                        0
                    } else {
                        j * (chain.len() - 1) / (free.len() - 1)
                    };
                    let bi = chain[at.min(chain.len() - 1)];
                    if out[bi].is_none() {
                        let c = t.bones[t.idx(s)].name;
                        used.insert(c);
                        out[bi] = Some(c);
                    }
                }
            }
        }
    }
}

/// Whether the mapping covers every required bone.
pub fn is_humanoid(map: &[Option<&'static str>]) -> bool {
    REQUIRED.iter().all(|r| map.iter().any(|m| *m == Some(*r)))
}

/// The required bones the mapping lacks.
pub fn missing(map: &[Option<&'static str>]) -> Vec<&'static str> {
    REQUIRED
        .iter()
        .copied()
        .filter(|r| !map.iter().any(|m| m == &Some(*r)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_loads() {
        let t = template();
        assert_eq!(t.bones.len(), 65);
        assert!((t.pos("hand_l").x - 0.739).abs() < 0.01);
        let sk = t.skeleton();
        let w = sk.rest_world();
        let h = sk.find("hand_l").unwrap();
        assert!((w[h].w_axis.truncate() - t.pos("hand_l")).length() < 1e-3);
        assert_eq!(primary_child("index_03_l"), Some("index_04_leaf_l"));
        assert_eq!(primary_child("upperarm_r"), Some("lowerarm_r"));
    }

    fn named(names: &[(&str, Option<usize>)]) -> Skeleton {
        Skeleton {
            bones: names
                .iter()
                .map(|(n, p)| Bone {
                    name: n.to_string(),
                    parent: *p,
                    rest: Xform::IDENTITY,
                    inverse_bind: glam::Mat4::IDENTITY,
                })
                .collect(),
        }
    }

    #[test]
    fn maps_mixamo() {
        let sk = named(&[
            ("mixamorig:Hips", None),
            ("mixamorig:Spine", Some(0)),
            ("mixamorig:Spine1", Some(1)),
            ("mixamorig:Spine2", Some(2)),
            ("mixamorig:Neck", Some(3)),
            ("mixamorig:Head", Some(4)),
            ("mixamorig:LeftShoulder", Some(3)),
            ("mixamorig:LeftArm", Some(6)),
            ("mixamorig:LeftForeArm", Some(7)),
            ("mixamorig:LeftHand", Some(8)),
            ("mixamorig:LeftHandIndex1", Some(9)),
            ("mixamorig:LeftUpLeg", Some(0)),
            ("mixamorig:LeftLeg", Some(11)),
            ("mixamorig:LeftFoot", Some(12)),
            ("mixamorig:LeftToeBase", Some(13)),
            ("mixamorig:LeftToe_End", Some(14)),
        ]);
        let m = map_bones(&sk);
        let want = [
            "pelvis", "spine_01", "spine_02", "spine_03", "neck_01", "Head", "clavicle_l",
            "upperarm_l", "lowerarm_l", "hand_l", "index_01_l", "thigh_l", "calf_l", "foot_l",
            "ball_l", "ball_leaf_l",
        ];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(m[i], Some(*w), "{}", sk.bones[i].name);
        }
    }

    #[test]
    fn maps_vroid_and_keywords() {
        let sk = named(&[
            ("J_Bip_C_Hips", None),
            ("J_Bip_C_Spine", Some(0)),
            ("J_Bip_C_Chest", Some(1)),
            ("J_Bip_C_UpperChest", Some(2)),
            ("J_Bip_C_Neck", Some(3)),
            ("J_Bip_C_Head", Some(4)),
            ("J_Bip_L_UpperArm", Some(3)),
            ("J_Bip_L_LowerArm", Some(6)),
            ("J_Bip_L_Hand", Some(7)),
            ("Leg_Upper.R", Some(0)),
            ("Leg_Lower.R", Some(9)),
            ("Foot.R", Some(10)),
        ]);
        let m = map_bones(&sk);
        assert_eq!(m[0], Some("pelvis"));
        assert_eq!(m[2], Some("spine_02"));
        assert_eq!(m[3], Some("spine_03"));
        assert_eq!(m[6], Some("upperarm_l"));
        assert_eq!(m[8], Some("hand_l"));
        assert_eq!(m[9], Some("thigh_r"));
        assert_eq!(m[10], Some("calf_r"));
        assert_eq!(m[11], Some("foot_r"));
    }
}
