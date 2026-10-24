//! Reading characters from disk into a [`Model`]: glTF / GLB through the `gltf` crate, FBX and
//! OBJ through `ufbx`.
//!
//! Each format fills a small format-neutral scene ([`Src`]: a node graph, skins, meshes in
//! geometry space, node-local animation tracks), and [`build`] turns that into the model
//! conventions: one skeleton (parents first), bind-pose vertices in model space, top-4 skin
//! weights and clips resampled at [`FPS`] as local bone tracks.

mod fbx;
mod gltf;

use crate::model::*;
use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use std::collections::HashMap;
use std::path::Path;

/// Rate every imported clip is resampled at.
pub const FPS: f32 = 30.0;

/// Loads a character file, choosing the reader by extension (.glb, .gltf, .fbx, .obj).
pub fn load(path: &Path) -> Result<Model, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let src = match ext.as_str() {
        "glb" | "gltf" => gltf::load(path)?,
        "fbx" | "obj" => fbx::load(path)?,
        _ => return Err(format!("unsupported file type: .{ext}")),
    };
    let mut model = build(src);
    face_plus_z(&mut model);
    Ok(model)
}

/// Whether a bone name marks the character's left (`Some(true)`) or right side.
fn side(name: &str) -> Option<bool> {
    let n = name.to_ascii_lowercase();
    let n = n.rsplit([':', '|']).next().unwrap_or(&n);
    let tagged = |c: char| {
        n.ends_with(&format!("_{c}"))
            || n.ends_with(&format!(".{c}"))
            || n.ends_with(&format!(" {c}"))
            || n.ends_with(&format!("-{c}"))
            || n.starts_with(&format!("{c}_"))
            || n.starts_with(&format!("{c}."))
            || n.contains(&format!("_{c}_"))
    };
    let left = n.contains("left") || tagged('l');
    let right = n.contains("right") || tagged('r');
    match (left, right) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Turns a rigged character to face +Z (its left on +X) when its left/right bone names say
/// it faces -Z, as some FBX exports do after axis conversion.
fn face_plus_z(model: &mut Model) {
    let Some(sk) = &model.skeleton else { return };
    let world = sk.rest_world();
    let (mut l, mut r) = ((0.0f32, 0usize), (0.0f32, 0usize));
    for (b, w) in sk.bones.iter().zip(&world) {
        match side(&b.name) {
            Some(true) => l = (l.0 + w.w_axis.x, l.1 + 1),
            Some(false) => r = (r.0 + w.w_axis.x, r.1 + 1),
            None => {}
        }
    }
    if l.1 == 0 || r.1 == 0 {
        return;
    }
    let (lx, rx) = (l.0 / l.1 as f32, r.0 / r.1 as f32);
    if lx < rx - 1e-3 {
        model.transform(Mat4::from_rotation_y(std::f32::consts::PI));
    }
}

/// A node of the source scene graph.
pub(crate) struct SrcNode {
    pub name: String,
    pub parent: Option<usize>,
    /// transform relative to the parent node (or the scene for a root)
    pub local: Mat4,
}

/// A skin: joint nodes and, per joint, geometry space → joint space in the bind pose.
pub(crate) struct SrcSkin {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

/// One triangle list with one material, vertices in its geometry space.
#[derive(Default)]
pub(crate) struct SrcMesh {
    pub name: String,
    /// node the geometry hangs from
    pub node: usize,
    /// geometry space → scene space (used when the mesh ends up unskinned)
    pub to_world: Mat4,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    /// (skin index, per vertex top-4 weights whose joints index that skin's `joints`)
    pub skin: Option<(usize, Vec<VertexSkin>)>,
}

/// Node-local channels sampled at `FPS`; each array empty or `frames` long.
#[derive(Default)]
pub(crate) struct SrcTrack {
    pub node: usize,
    pub rot: Vec<Quat>,
    pub pos: Vec<Vec3>,
    pub scale: Vec<Vec3>,
}

pub(crate) struct SrcAnim {
    pub name: String,
    pub frames: usize,
    pub tracks: Vec<SrcTrack>,
}

#[derive(Default)]
pub(crate) struct Src {
    pub nodes: Vec<SrcNode>,
    pub skins: Vec<SrcSkin>,
    pub meshes: Vec<SrcMesh>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub anims: Vec<SrcAnim>,
    /// Nodes that are bones even without a skin (FBX limb nodes, animated glTF nodes): the
    /// skeleton of an animation-only file.
    pub bone_hint: Vec<bool>,
}

/// Keeps the four largest (joint, weight) influences, normalized; `None` when all are zero.
pub(crate) fn top4(influences: &mut Vec<(u16, f32)>) -> Option<VertexSkin> {
    influences.retain(|&(_, w)| w > 0.0 && w.is_finite());
    influences.sort_by(|a, b| b.1.total_cmp(&a.1));
    influences.truncate(4);
    let sum: f32 = influences.iter().map(|i| i.1).sum();
    if sum <= 0.0 {
        return None;
    }
    let mut s = VertexSkin::default();
    for (k, &(j, w)) in influences.iter().enumerate() {
        s.joints[k] = j;
        s.weights[k] = w / sum;
    }
    Some(s)
}

/// Frame count for a clip `duration` seconds long.
pub(crate) fn frame_count(duration: f32) -> usize {
    if duration.is_finite() && duration > 0.0 {
        (duration * FPS).round() as usize + 1
    } else {
        1
    }
}

fn world_matrices(nodes: &[SrcNode]) -> Vec<Mat4> {
    fn get(i: usize, nodes: &[SrcNode], out: &mut [Option<Mat4>], depth: usize) -> Mat4 {
        if let Some(m) = out[i] {
            return m;
        }
        let m = match nodes[i].parent {
            // (depth guard: a malformed file with a parent cycle)
            Some(p) if depth < 4096 => get(p, nodes, out, depth + 1) * nodes[i].local,
            _ => nodes[i].local,
        };
        out[i] = Some(m);
        m
    }
    let mut out = vec![None; nodes.len()];
    (0..nodes.len()).map(|i| get(i, nodes, &mut out, 0)).collect()
}

fn normal_matrix(m: Mat4) -> Mat3 {
    Mat3::from_mat4(m).inverse().transpose()
}

fn transformed(mesh: &SrcMesh, m: Mat4) -> (Vec<Vec3>, Vec<Vec3>) {
    let n = normal_matrix(m);
    (
        mesh.positions.iter().map(|&p| m.transform_point3(p)).collect(),
        mesh.normals.iter().map(|&v| (n * v).normalize_or_zero()).collect(),
    )
}

/// The skin joint carrying the most weight on `weights`.
fn dominant_joint(weights: &[VertexSkin], joint_count: usize) -> usize {
    let mut total = vec![0.0f32; joint_count.max(1)];
    for s in weights {
        for k in 0..4 {
            if let Some(t) = total.get_mut(s.joints[k] as usize) {
                *t += s.weights[k];
            }
        }
    }
    (0..total.len()).max_by(|&a, &b| total[a].total_cmp(&total[b])).unwrap_or(0)
}

fn near_identity(m: Mat4) -> bool {
    m.abs_diff_eq(Mat4::IDENTITY, 1e-5)
}

/// Turns a format-neutral scene into a [`Model`].
pub(crate) fn build(src: Src) -> Model {
    let world = world_matrices(&src.nodes);
    let nn = src.nodes.len();

    // Group skins that share joints; keep the group with the most joints.
    let mut group: Vec<usize> = (0..src.skins.len()).collect();
    fn find(g: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while g[r] != r {
            r = g[r];
        }
        g[i] = r;
        r
    }
    let mut owner: HashMap<usize, usize> = HashMap::new();
    for (si, skin) in src.skins.iter().enumerate() {
        for &j in &skin.joints {
            if let Some(&o) = owner.get(&j) {
                let (a, b) = (find(&mut group, o), find(&mut group, si));
                group[a] = b;
            } else {
                owner.insert(j, si);
            }
        }
    }
    let mut group_joints: HashMap<usize, Vec<usize>> = HashMap::new();
    for (j, &si) in &owner {
        let g = find(&mut group, si);
        group_joints.entry(g).or_default().push(*j);
    }
    let chosen = group_joints
        .iter()
        .max_by_key(|(g, js)| (js.len(), std::cmp::Reverse(**g)))
        .map(|(g, _)| *g);
    let in_group: Vec<bool> =
        (0..src.skins.len()).map(|si| Some(find(&mut group, si)) == chosen).collect();

    // Skeleton nodes: the chosen joints plus any ancestors linking one joint to another.
    let mut is_bone = vec![false; nn];
    if let Some(g) = chosen {
        for &j in &group_joints[&g] {
            is_bone[j] = true;
        }
    } else {
        for (i, &h) in src.bone_hint.iter().enumerate().take(nn) {
            is_bone[i] = h;
        }
    }
    {
        let joints = is_bone.clone();
        for j in 0..nn {
            if !joints[j] {
                continue;
            }
            let mut chain = Vec::new();
            let mut p = src.nodes[j].parent;
            while let Some(pi) = p {
                if joints[pi] {
                    for &c in &chain {
                        is_bone[c] = true;
                    }
                    break;
                }
                if chain.len() > nn {
                    break;
                }
                chain.push(pi);
                p = src.nodes[pi].parent;
            }
        }
    }
    // Depth-first from the roots in node order, so parents come first.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nn];
    for (i, n) in src.nodes.iter().enumerate() {
        if let Some(p) = n.parent {
            children[p].push(i);
        }
    }
    let mut order: Vec<usize> = Vec::new();
    let mut bone_of: Vec<Option<usize>> = vec![None; nn];
    for r in 0..nn {
        let parent_is_bone = src.nodes[r].parent.is_some_and(|p| is_bone[p]);
        if !is_bone[r] || parent_is_bone {
            continue;
        }
        let mut stack = vec![r];
        while let Some(i) = stack.pop() {
            if bone_of[i].is_some() {
                continue;
            }
            bone_of[i] = Some(order.len());
            order.push(i);
            for &c in children[i].iter().rev() {
                if is_bone[c] {
                    stack.push(c);
                }
            }
        }
    }

    let mut model = Model {
        materials: src.materials,
        images: src.images,
        ..Default::default()
    };

    // Bones: rest from the node graph; inverse bind from the largest skin of the group,
    // re-based so model space is the space that skin's dominant joint binds into.
    if !order.is_empty() {
        let mut bones: Vec<Bone> = order
            .iter()
            .map(|&i| {
                let n = &src.nodes[i];
                let parent = n.parent.and_then(|p| bone_of[p]);
                let rest = match parent {
                    Some(_) => n.local,
                    None => world[i],
                };
                Bone {
                    name: n.name.clone(),
                    parent,
                    rest: Xform::from_matrix(rest),
                    inverse_bind: world[i].inverse(),
                }
            })
            .collect();
        let mut skins_by_size: Vec<usize> = (0..src.skins.len()).filter(|&s| in_group[s]).collect();
        skins_by_size.sort_by_key(|&s| std::cmp::Reverse(src.skins[s].joints.len()));
        if let Some(&s0) = skins_by_size.first() {
            // Bind-pose geometry space → model space for the main skin, from its top joint.
            let skin = &src.skins[s0];
            let top = (0..skin.joints.len())
                .min_by_key(|&k| bone_of[skin.joints[k]].unwrap_or(usize::MAX))
                .unwrap_or(0);
            let geo_to_model = world[skin.joints[top]] * skin.inverse_bind[top];
            let model_to_geo = geo_to_model.inverse();
            let mut set = vec![false; bones.len()];
            for &s in &skins_by_size {
                let skin = &src.skins[s];
                for (k, &j) in skin.joints.iter().enumerate() {
                    let b = bone_of[j].expect("skin joint is a bone");
                    if !set[b] {
                        set[b] = true;
                        bones[b].inverse_bind = skin.inverse_bind[k] * model_to_geo;
                    }
                }
            }
        }
        model.skeleton = Some(Skeleton { bones });
    }

    // Meshes.
    for m in &src.meshes {
        let mut out = Mesh {
            name: m.name.clone(),
            uvs: m.uvs.clone(),
            indices: m.indices.clone(),
            material: m.material,
            ..Default::default()
        };
        match &m.skin {
            Some((si, weights)) if in_group[*si] => {
                let skin = &src.skins[*si];
                let sk = model.skeleton.as_ref().expect("skeleton for skin");
                let d = dominant_joint(weights, skin.joints.len());
                let b = bone_of[skin.joints[d]].expect("skin joint is a bone");
                // geometry → model: undo this skin's bind of `d`, apply the bone's.
                let geo_to_model = sk.bones[b].inverse_bind.inverse() * skin.inverse_bind[d];
                (out.positions, out.normals) = transformed(m, geo_to_model);
                let map: Vec<u16> =
                    skin.joints.iter().map(|&j| bone_of[j].unwrap_or(b) as u16).collect();
                out.skin = Some(
                    weights
                        .iter()
                        .map(|s| {
                            let mut v = *s;
                            for k in 0..4 {
                                v.joints[k] = if v.weights[k] > 0.0 {
                                    map.get(s.joints[k] as usize).copied().unwrap_or(b as u16)
                                } else {
                                    0
                                };
                            }
                            if v.weights.iter().sum::<f32>() <= 0.0 {
                                v = VertexSkin { joints: [b as u16, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] };
                            }
                            v
                        })
                        .collect(),
                );
            }
            Some((si, weights)) => {
                // A skin we dropped: freeze the geometry where its rest pose puts it.
                let skin = &src.skins[*si];
                let d = dominant_joint(weights, skin.joints.len());
                let to = world[skin.joints[d]] * skin.inverse_bind[d];
                (out.positions, out.normals) = transformed(m, to);
            }
            None => {
                (out.positions, out.normals) = transformed(m, m.to_world);
                // Rigid props under a bone (a sword in a hand) follow that bone.
                let mut p = Some(m.node);
                while let Some(i) = p {
                    if let Some(b) = bone_of[i] {
                        let one = VertexSkin { joints: [b as u16, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] };
                        out.skin = Some(vec![one; out.positions.len()]);
                        break;
                    }
                    p = src.nodes[i].parent;
                }
            }
        }
        model.meshes.push(out);
    }
    model.ensure_normals();

    // Clips: tracks of skeleton nodes; a root bone's channels move into model space.
    if model.skeleton.is_some() {
        for a in src.anims {
            let mut clip = Clip { name: a.name, fps: FPS, frames: a.frames, tracks: Vec::new() };
            for t in a.tracks {
                let Some(b) = bone_of.get(t.node).copied().flatten() else { continue };
                if t.rot.is_empty() && t.pos.is_empty() && t.scale.is_empty() {
                    continue;
                }
                let n = &src.nodes[t.node];
                let parent_world = match n.parent {
                    Some(p) if bone_of[p].is_none() => world[p],
                    _ => Mat4::IDENTITY,
                };
                if near_identity(parent_world) {
                    clip.tracks.push(Track { bone: b, rot: t.rot, pos: t.pos, scale: t.scale });
                    continue;
                }
                let rest = Xform::from_matrix(n.local);
                let mut out = Track { bone: b, ..Default::default() };
                let animated_scale = !t.scale.is_empty();
                for f in 0..a.frames {
                    let x = Xform {
                        t: t.pos.get(f).copied().unwrap_or(rest.t),
                        r: t.rot.get(f).copied().unwrap_or(rest.r),
                        s: t.scale.get(f).copied().unwrap_or(rest.s),
                    };
                    let w = Xform::from_matrix(parent_world * x.matrix());
                    out.rot.push(w.r);
                    out.pos.push(w.t);
                    if animated_scale {
                        out.scale.push(w.s);
                    }
                }
                clip.tracks.push(out);
            }
            clip.tracks.sort_by_key(|t| t.bone);
            model.clips.push(clip);
        }
    }
    model
}

/// "image/png" / "image/jpeg" from the leading bytes, if they're either.
pub(crate) fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}
