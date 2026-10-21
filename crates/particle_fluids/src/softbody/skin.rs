//! Skinning a render mesh to a soft body (CPU port of `bindMesh.ts`,
//! `dlb.ts` and `skinMaterial.ts`).
//!
//! Each render vertex follows its four nearest *stable* rest particles (at
//! least three face neighbors, so one-particle-thick chains don't tear thin
//! features), weighted by inverse distance. Per influence `k` the rigid motion
//! is the dual quaternion `q̂_k = q_k + ε ½ t_k q_k` with `t_k = x_k − R(q_k) r_k`
//! (`r_k` the particle's rest offset from the body's rest centre). The blend
//! (Kavan et al. 2008, DLB) flips antipodal lanes against lane 0, sums the
//! weighted dual quaternions, normalizes by the real part's length, and maps
//! the vertex's rest offset `v` to `R(â)·v + 2(a₀d_ε − a_ε d₀ + d₀ × d_ε)`.
//! Normals turn with `R(â)`. Under local shape matching each particle has its
//! own orientation; under global matching every lane uses the body rotation
//! and the blend reduces to linear blend skinning.

use super::{ShapeMatching, SoftbodySystem};
use crate::math::{quat_from_m3, quat_rotate};
use crate::particles::Particles;
use glam::{Quat, Vec3, Vec3A, Vec4};
use rayon::prelude::*;

const INFLUENCES: usize = 4;
const STABLE_DEGREE: usize = 3;

/// Per-vertex influences of a render mesh bound to one soft body.
#[derive(Clone, Debug, Default)]
pub struct SkinBinding {
    pub(crate) body: usize,
    /// Global particle slots per vertex.
    pub influences: Vec<[u32; INFLUENCES]>,
    /// Inverse-distance weights per vertex, summing to one.
    pub weights: Vec<[f32; INFLUENCES]>,
    /// Vertex positions relative to the body's rest centre.
    rest: Vec<Vec3A>,
    normals: Vec<Vec3A>,
}

impl SkinBinding {
    pub fn vertex_count(&self) -> usize {
        self.rest.len()
    }
}

impl SoftbodySystem {
    /// Bind `vertices` (world space, placed over the body's rest shape) and
    /// their `normals` (may be empty) to body `body`.
    pub(crate) fn bind(
        &self,
        body: usize,
        radius: f32,
        vertices: &[Vec3],
        normals: &[Vec3],
    ) -> SkinBinding {
        let b = &self.bodies[body];
        let n = b.count as usize;
        let mut stable = vec![true; n];
        if !b.edges.is_empty() {
            let mut degree = vec![0usize; n];
            for &[i, j] in &b.edges {
                degree[i as usize] += 1;
                degree[j as usize] += 1;
            }
            for i in 0..n {
                stable[i] = degree[i] >= STABLE_DEGREE;
            }
            if !stable.iter().any(|&s| s) {
                stable.fill(true);
            }
        }
        let min_distance = 0.1 * radius;
        let rest = &b.rest_positions;
        let (influences, weights): (Vec<_>, Vec<_>) = vertices
            .par_iter()
            .map(|&v| {
                let mut best_d = [f32::INFINITY; INFLUENCES];
                let mut best_i = [-1i64; INFLUENCES];
                for i in 0..n {
                    if !stable[i] {
                        continue;
                    }
                    let d = (rest[i] - v).length_squared();
                    let mut worst = 0;
                    for k in 1..INFLUENCES {
                        if best_d[k] > best_d[worst] {
                            worst = k;
                        }
                    }
                    if d < best_d[worst] {
                        best_d[worst] = d;
                        best_i[worst] = i as i64;
                    }
                }
                let total: f32 = (0..INFLUENCES)
                    .filter(|&k| best_i[k] >= 0)
                    .map(|k| 1.0 / best_d[k].sqrt().max(min_distance))
                    .sum();
                let mut inf = [b.start; INFLUENCES];
                let mut w = [0.0; INFLUENCES];
                for k in 0..INFLUENCES {
                    if best_i[k] >= 0 {
                        inf[k] = b.start + best_i[k] as u32;
                        w[k] = 1.0 / best_d[k].sqrt().max(min_distance) / total;
                    }
                }
                (inf, w)
            })
            .unzip();
        SkinBinding {
            body,
            influences,
            weights,
            rest: vertices
                .iter()
                .map(|&v| Vec3A::from(v - b.rest_center))
                .collect(),
            normals: normals.iter().map(|&v| Vec3A::from(v)).collect(),
        }
    }

    /// Deformed world positions and normals of a bound mesh.
    pub(crate) fn skin(
        &self,
        p: &Particles,
        binding: &SkinBinding,
        positions: &mut Vec<Vec3>,
        normals: &mut Vec<Vec3>,
    ) {
        let b = &self.bodies[binding.body];
        let shared =
            (b.mode == ShapeMatching::Global).then(|| quat_from_m3(&b.rotation).normalize());
        let rot = |i: usize| shared.unwrap_or(p.rot[i]);
        let rest = &self.rest_offsets;
        let n = binding.rest.len();
        positions.resize(n, Vec3::ZERO);
        normals.resize(if binding.normals.is_empty() { 0 } else { n }, Vec3::ZERO);
        let blend = |v: usize| -> (Vec4, Vec4) {
            let inf = binding.influences[v];
            let w = binding.weights[v];
            let mut real = Vec4::ZERO;
            let mut dual = Vec4::ZERO;
            let mut pivot = Vec4::ZERO;
            for k in 0..INFLUENCES {
                let i = inf[k] as usize;
                let q = rot(i);
                let t = p.pos[i] - quat_rotate(q, rest[i]);
                let d = Quat::from_xyzw(t.x, t.y, t.z, 0.0) * q;
                let (mut qr, mut qd) = (Vec4::from(q), Vec4::from(d) * 0.5);
                if k == 0 {
                    pivot = qr;
                } else if pivot.dot(qr) < 0.0 {
                    qr = -qr;
                    qd = -qd;
                }
                real += qr * w[k];
                dual += qd * w[k];
            }
            let inv = 1.0 / real.length().max(1e-20);
            (real * inv, dual * inv)
        };
        positions
            .par_iter_mut()
            .enumerate()
            .with_min_len(256)
            .for_each(|(v, out)| {
                let (r, d) = blend(v);
                let q = Quat::from_vec4(r);
                let (a0, d0) = (r.w, Vec3A::new(r.x, r.y, r.z));
                let (ae, de) = (d.w, Vec3A::new(d.x, d.y, d.z));
                let translation = (de * a0 - d0 * ae + d0.cross(de)) * 2.0;
                *out = (quat_rotate(q, binding.rest[v]) + translation).into();
            });
        if !binding.normals.is_empty() {
            normals
                .par_iter_mut()
                .enumerate()
                .with_min_len(256)
                .for_each(|(v, out)| {
                    let (r, _) = blend(v);
                    *out = quat_rotate(Quat::from_vec4(r), binding.normals[v])
                        .normalize_or_zero()
                        .into();
                });
        }
    }
}
