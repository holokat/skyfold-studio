//! The in-memory character every stage works on: meshes in model space, their materials and
//! images, an optional skeleton (with per-vertex skin) and animation clips.
//!
//! Conventions, whatever the source file used: metres, Y up, right-handed, the character faces
//! +Z (its left hand is on +X). Mesh positions are in model space as they appear in the bind
//! pose; a mesh's `skin` indexes `Skeleton::bones`.

use glam::{Mat4, Quat, Vec2, Vec3};

#[derive(Clone, Debug, Default)]
pub struct Model {
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    /// The rig the file came with, if any.
    pub skeleton: Option<Skeleton>,
    /// Clips the file came with (they target `skeleton`).
    pub clips: Vec<Clip>,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// empty when the source had none
    pub uvs: Vec<Vec2>,
    /// triangles
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    /// Per vertex: up to four (bone index, weight) pairs, weights summing to 1. `None` when the
    /// mesh isn't skinned.
    pub skin: Option<Vec<VertexSkin>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VertexSkin {
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TexRef {
    /// index into `Model::images`
    pub image: usize,
}

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    /// linear RGBA
    pub base_color: [f32; 4],
    pub base_color_tex: Option<TexRef>,
    pub normal_tex: Option<TexRef>,
    /// glTF layout: G = roughness, B = metallic
    pub metallic_roughness_tex: Option<TexRef>,
    pub occlusion_tex: Option<TexRef>,
    pub emissive: [f32; 3],
    pub emissive_tex: Option<TexRef>,
    pub metallic: f32,
    pub roughness: f32,
    pub alpha: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0; 4],
            base_color_tex: None,
            normal_tex: None,
            metallic_roughness_tex: None,
            occlusion_tex: None,
            emissive: [0.0; 3],
            emissive_tex: None,
            metallic: 0.0,
            roughness: 0.8,
            alpha: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

/// An encoded picture (PNG or JPEG bytes, as they were in the file).
#[derive(Clone, Debug)]
pub struct Image {
    pub name: String,
    /// "image/png" or "image/jpeg"
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// A local transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub t: Vec3,
    pub r: Quat,
    pub s: Vec3,
}

impl Default for Xform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Xform {
    pub const IDENTITY: Xform = Xform {
        t: Vec3::ZERO,
        r: Quat::IDENTITY,
        s: Vec3::ONE,
    };
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }
    pub fn from_matrix(m: Mat4) -> Self {
        let (s, r, t) = m.to_scale_rotation_translation();
        Self { t, r, s }
    }
}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    /// always an earlier index (bones are stored parents first)
    pub parent: Option<usize>,
    /// local rest transform (relative to the parent, or to model space for a root bone)
    pub rest: Xform,
    /// model space → this bone's space in the bind pose
    pub inverse_bind: Mat4,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

impl Skeleton {
    pub fn find(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }
    /// Model-space rest matrices of every bone.
    pub fn rest_world(&self) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let m = b.rest.matrix();
            out.push(match b.parent {
                Some(p) => out[p] * m,
                None => m,
            });
        }
        out
    }
    /// Sets every bone's inverse bind matrix from its rest pose.
    pub fn bind_to_rest(&mut self) {
        let w = self.rest_world();
        for (b, m) in self.bones.iter_mut().zip(w) {
            b.inverse_bind = m.inverse();
        }
    }
}

/// A clip sampled at a fixed rate: frame `i` is at `i / fps` seconds. A track's arrays are
/// either empty (the channel isn't animated) or `frames` long.
#[derive(Clone, Debug, Default)]
pub struct Clip {
    pub name: String,
    pub fps: f32,
    pub frames: usize,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    /// index into the skeleton the clip targets
    pub bone: usize,
    pub rot: Vec<Quat>,
    pub pos: Vec<Vec3>,
    pub scale: Vec<Vec3>,
}

impl Clip {
    pub fn duration(&self) -> f32 {
        if self.frames > 1 {
            (self.frames - 1) as f32 / self.fps
        } else {
            0.0
        }
    }
}

impl Model {
    /// Axis-aligned bounds of every mesh vertex.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.meshes.iter().flat_map(|m| m.positions.iter().copied());
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }
    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().map(|m| m.positions.len()).sum()
    }
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(|m| m.indices.len() / 3).sum()
    }
    /// Applies `m` to every vertex (and normal), and to the skeleton's roots.
    pub fn transform(&mut self, m: Mat4) {
        let n = Mat4::from_mat3(glam::Mat3::from_mat4(m).inverse().transpose());
        for mesh in &mut self.meshes {
            for p in &mut mesh.positions {
                *p = m.transform_point3(*p);
            }
            for v in &mut mesh.normals {
                *v = n.transform_vector3(*v).normalize_or_zero();
            }
        }
        if let Some(sk) = &mut self.skeleton {
            for b in &mut sk.bones {
                if b.parent.is_none() {
                    b.rest = Xform::from_matrix(m * b.rest.matrix());
                }
                b.inverse_bind *= m.inverse();
            }
        }
        // (clip root tracks are in the root's parent space: model space)
        let roots: Vec<usize> = self
            .skeleton
            .as_ref()
            .map(|sk| {
                sk.bones
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.parent.is_none())
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        for c in &mut self.clips {
            for t in &mut c.tracks {
                if !roots.contains(&t.bone) {
                    continue;
                }
                let (s, r, _) = m.to_scale_rotation_translation();
                for q in &mut t.rot {
                    *q = r * *q;
                }
                for p in &mut t.pos {
                    *p = m.transform_point3(*p);
                }
                let _ = s;
            }
        }
    }
    /// Computes smooth normals for meshes without them.
    pub fn ensure_normals(&mut self) {
        for m in &mut self.meshes {
            if m.normals.len() == m.positions.len() {
                continue;
            }
            let mut n = vec![Vec3::ZERO; m.positions.len()];
            for t in m.indices.chunks_exact(3) {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                let f = (m.positions[b] - m.positions[a]).cross(m.positions[c] - m.positions[a]);
                n[a] += f;
                n[b] += f;
                n[c] += f;
            }
            m.normals = n.into_iter().map(|v| v.normalize_or(Vec3::Y)).collect();
        }
    }
}
