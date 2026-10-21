//! Soft bodies: shape matching as XPBD constraints (port of `softbody/`).
//!
//! - **Global** matching (Müller et al. 2005) fits one mass-weighted rotation
//!   (polar decomposition of `A = Σ m_i x*_i r_iᵀ`) and centre to the whole
//!   body every iteration and pulls each particle toward `R r_i + c`.
//! - **Local** matching with oriented particles (Müller & Chentanez 2011 §5.1),
//!   the reference `Simulation`'s mode: every particle's group is itself plus
//!   its face neighbors; each group fits `R_i = polar((r²/5) Σ_j R_j +
//!   Σ_j (x*_j − c_i)(x̃_j − c̄_i)ᵀ)` and pulls its members toward
//!   `R_i (x̃_j − c̄_i) + c_i`, each (group, member) pair with its own multiplier,
//!   corrections averaged over the groups a particle is in and clamped to 5 radii.
//!   Particle orientations are integrated like positions.

pub mod skin;
pub mod voxelize;

use crate::math::{M3, M3_IDENTITY, m3_from_quat, m3_mul_vec, polar_rotation, quat_from_m3};
use crate::mesh::TriangleMesh;
use crate::par::{SharedMut, chunked_sum, for_each_index};
use crate::particles::Particles;
use glam::{Quat, Vec3, Vec3A};

/// Shape-matching mode of a soft body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShapeMatching {
    /// One rotation for the whole body: nearly rigid, wobbles.
    Global,
    /// One rotation per particle neighborhood: bends and folds (reference default).
    #[default]
    Local,
}

/// A soft body made by filling a closed mesh with particles.
#[derive(Clone, Debug)]
pub struct SoftbodyOptions {
    /// A closed mesh in world space. Only its largest connected piece is kept.
    pub mesh: TriangleMesh,
    /// 0 for firm rubber to 1 for loose jelly. Default 0.3.
    pub softness: f32,
    /// kg/m³; water is 1000. Default 500.
    pub density: f32,
    pub shape_matching: ShapeMatching,
    /// Explicit compliance, s²/kg, overriding the `softness` formula.
    pub compliance: Option<f32>,
}

impl SoftbodyOptions {
    pub fn new(mesh: TriangleMesh) -> Self {
        Self {
            mesh,
            softness: 0.3,
            density: 500.0,
            shape_matching: ShapeMatching::Local,
            compliance: None,
        }
    }
}

/// The reference's softness → compliance mapping: about the same feel at any particle count.
pub fn softbody_compliance(softness: f32, count: usize) -> f32 {
    10f32.powf(-6.0 + softness * 3.0) * (count as f32 / 200.0)
}

/// One built body.
pub(crate) struct Body {
    pub start: u32,
    pub count: u32,
    pub surface_count: u32,
    pub mode: ShapeMatching,
    pub softness: f32,
    pub explicit_compliance: Option<f32>,
    pub compliance: f32,
    pub rest_center: Vec3,
    /// Rest positions, world space at build.
    pub rest_positions: Vec<Vec3>,
    /// Local edges `[i, j]`.
    pub edges: Vec<[u32; 2]>,
    /// Current best-fit centre and rotation (row-major).
    pub center: Vec3A,
    pub rotation: M3,
}

impl Body {
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.count) as usize
    }
}

pub(crate) struct SoftbodySystem {
    pub bodies: Vec<Body>,
    /// Rest offset of each particle from its body's rest centre (capacity-sized).
    pub rest_offsets: Vec<Vec3A>,
    weights: Vec<f32>,
    /// Particle compliance (its body's).
    compliance: Vec<f32>,
    /// Local groups: CSR over global indices; row `i` is `i` then its neighbors.
    offsets: Vec<u32>,
    indices: Vec<u32>,
    /// For entry `k` of row `j` (pointing at group `i`), the entry of `j` in row `i`.
    reverse: Vec<u32>,
    pair_lambda: Vec<Vec3A>,
    rest_centers: Vec<Vec3A>,
    centers: Vec<Vec3A>,
    rotations: Vec<M3>,
    accum: Vec<Vec3A>,
    global_lambda: Vec<Vec3A>,
    /// Particles of local bodies, ascending.
    local_rows: Vec<u32>,
    /// Particles of global bodies, ascending.
    global_rows: Vec<u32>,
    radius: f32,
}

impl SoftbodySystem {
    pub fn new(capacity: usize, radius: f32) -> Self {
        Self {
            bodies: Vec::new(),
            rest_offsets: vec![Vec3A::ZERO; capacity],
            weights: vec![1.0; capacity],
            compliance: vec![0.0; capacity],
            offsets: vec![0; capacity + 1],
            indices: Vec::new(),
            reverse: Vec::new(),
            pair_lambda: Vec::new(),
            rest_centers: vec![Vec3A::ZERO; capacity],
            centers: vec![Vec3A::ZERO; capacity],
            rotations: vec![M3_IDENTITY; capacity],
            accum: vec![Vec3A::ZERO; capacity],
            global_lambda: vec![Vec3A::ZERO; capacity],
            local_rows: Vec::new(),
            global_rows: Vec::new(),
            radius,
        }
    }

    /// Register a body whose particles were already written to `p`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_body(
        &mut self,
        p: &Particles,
        start: u32,
        count: u32,
        surface_count: u32,
        edges: Vec<[u32; 2]>,
        mode: ShapeMatching,
        softness: f32,
        explicit_compliance: Option<f32>,
    ) {
        let range = start as usize..(start + count) as usize;
        let masses: Vec<f32> = range
            .clone()
            .map(|i| {
                if p.inv_mass[i] > 0.0 {
                    1.0 / p.inv_mass[i]
                } else {
                    0.0
                }
            })
            .collect();
        let dynamic: Vec<f32> = masses.iter().copied().filter(|&m| m > 0.0).collect();
        let uniform = dynamic.windows(2).all(|w| w[0] == w[1]);
        let mean = if dynamic.is_empty() {
            1.0
        } else {
            dynamic.iter().sum::<f32>() / dynamic.len() as f32
        };
        for (k, i) in range.clone().enumerate() {
            self.weights[i] = if uniform || masses[k] <= 0.0 {
                1.0
            } else {
                masses[k] / mean
            };
        }
        let rest_positions: Vec<Vec3> = range.clone().map(|i| Vec3::from(p.pos[i])).collect();
        let total: f32 = range.clone().map(|i| self.weights[i]).sum();
        let center = range
            .clone()
            .map(|i| Vec3::from(p.pos[i]) * self.weights[i])
            .sum::<Vec3>()
            / total;
        for i in range.clone() {
            self.rest_offsets[i] = p.pos[i] - Vec3A::from(center);
        }
        let compliance =
            explicit_compliance.unwrap_or_else(|| softbody_compliance(softness, count as usize));
        for i in range {
            self.compliance[i] = compliance;
        }
        self.bodies.push(Body {
            start,
            count,
            surface_count,
            mode,
            softness,
            explicit_compliance,
            compliance,
            rest_center: center,
            rest_positions,
            edges,
            center: Vec3A::from(center),
            rotation: M3_IDENTITY,
        });
    }

    /// Build the local-group graph once every body is added.
    pub fn finalize(&mut self) {
        let cap = self.offsets.len() - 1;
        let mut rows: Vec<Vec<u32>> = vec![Vec::new(); cap];
        self.local_rows.clear();
        self.global_rows.clear();
        for b in &self.bodies {
            let rows_of = if b.mode == ShapeMatching::Local {
                &mut self.local_rows
            } else {
                &mut self.global_rows
            };
            rows_of.extend(b.range().map(|i| i as u32));
            if b.mode != ShapeMatching::Local {
                continue;
            }
            for i in b.range() {
                rows[i].push(i as u32);
            }
            for &[i, j] in &b.edges {
                let (gi, gj) = (b.start + i, b.start + j);
                rows[gi as usize].push(gj);
                rows[gj as usize].push(gi);
            }
        }
        self.offsets[0] = 0;
        for (i, row) in rows.iter().enumerate() {
            self.offsets[i + 1] = self.offsets[i] + row.len() as u32;
        }
        self.indices = rows.iter().flatten().copied().collect();
        self.reverse = vec![0; self.indices.len()];
        for j in 0..cap {
            let (a, b) = (self.offsets[j] as usize, self.offsets[j + 1] as usize);
            for k in a..b {
                let i = self.indices[k] as usize;
                let (ia, ib) = (self.offsets[i] as usize, self.offsets[i + 1] as usize);
                let pos = self.indices[ia..ib]
                    .iter()
                    .position(|&x| x as usize == j)
                    .expect("symmetric groups");
                self.reverse[k] = (ia + pos) as u32;
            }
        }
        self.pair_lambda = vec![Vec3A::ZERO; self.indices.len()];
        for i in 0..cap {
            let (a, b) = (self.offsets[i] as usize, self.offsets[i + 1] as usize);
            if b > a {
                let sum: Vec3A = self.indices[a..b]
                    .iter()
                    .map(|&j| self.rest_offsets[j as usize])
                    .sum();
                self.rest_centers[i] = sum / (b - a) as f32;
            }
        }
    }

    pub fn set_softness(&mut self, body: usize, softness: f32) {
        let b = &mut self.bodies[body];
        b.softness = softness;
        b.compliance = b
            .explicit_compliance
            .unwrap_or_else(|| softbody_compliance(softness, b.count as usize));
        let c = b.compliance;
        for i in b.range() {
            self.compliance[i] = c;
        }
    }

    /// Orientation prediction for local bodies (Müller & Chentanez 2011 eq. 11).
    pub fn predict_rotations(&self, p: &mut Particles, dt: f32) {
        let Particles {
            rot,
            pred_rot,
            ang_vel,
            ..
        } = p;
        let out = SharedMut::new(&mut pred_rot[..]);
        let (rot, ang_vel) = (&*rot, &*ang_vel);
        for_each_index(&self.local_rows, |i| {
            let q = rot[i];
            let omega = ang_vel[i];
            let mag = omega.length();
            let next = if mag < 1e-8 {
                q
            } else {
                let half = mag * dt * 0.5;
                let s = omega * (half.sin() / mag);
                let spin = Quat::from_xyzw(s.x, s.y, s.z, half.cos());
                spin * q
            };
            // SAFETY: index i written only by this task.
            unsafe { *out.get(i) = next };
        });
    }

    /// Angular velocity from the solved orientation (eqs. 12–15), capped at 100 rad/s.
    pub fn advect_rotations(&self, p: &mut Particles, dt: f32) {
        let Particles {
            rot,
            pred_rot,
            ang_vel,
            ..
        } = p;
        let rot_out = SharedMut::new(&mut rot[..]);
        let w_out = SharedMut::new(&mut ang_vel[..]);
        let pred_rot = &*pred_rot;
        for_each_index(&self.local_rows, |i| {
            // SAFETY: index i read and written only by this task.
            let q = unsafe { *rot_out.read(i) };
            let qp = pred_rot[i];
            let qp = qp * (1.0 / qp.length_squared().max(1e-12).sqrt());
            let mut r = qp * q.conjugate();
            if r.w < 0.0 {
                r = -r;
            }
            let half_sin = Vec3A::new(r.x, r.y, r.z).length();
            let omega = if half_sin < 1e-8 {
                Vec3A::ZERO
            } else {
                let angle = 2.0 * half_sin.atan2(r.w);
                let w = Vec3A::new(r.x, r.y, r.z) * (angle / half_sin / dt);
                let scale = (100.0f32 * 100.0 / w.length_squared().max(100.0 * 100.0)).sqrt();
                w * scale
            };
            // SAFETY: as above.
            unsafe {
                *w_out.get(i) = omega;
                *rot_out.get(i) = qp;
            }
        });
    }

    pub fn reset_lambdas(&mut self) {
        self.pair_lambda.iter_mut().for_each(|l| *l = Vec3A::ZERO);
        for &i in &self.global_rows {
            self.global_lambda[i as usize] = Vec3A::ZERO;
        }
    }

    /// Best-fit centre and rotation of each body (mass-weighted, deterministic sums).
    pub fn fit_frames(&mut self, p: &Particles, only_global: bool) {
        for b in &mut self.bodies {
            if only_global && b.mode != ShapeMatching::Global {
                continue;
            }
            let (start, n) = (b.start as usize, b.count as usize);
            let weights = &self.weights;
            let s = chunked_sum(n, WeightedSum::default(), |k| {
                let i = start + k;
                WeightedSum(p.pred[i] * weights[i], weights[i])
            });
            b.center = s.0 / s.1;
            let rest = &self.rest_offsets;
            let rows = chunked_sum(n, Mat3Sum::default(), |k| {
                let i = start + k;
                let x = p.pred[i] * weights[i];
                let r = rest[i];
                Mat3Sum([r * x.x, r * x.y, r * x.z])
            });
            let a = [
                rows.0[0].to_array(),
                rows.0[1].to_array(),
                rows.0[2].to_array(),
            ];
            b.rotation = polar_rotation(&a);
        }
    }

    /// One iteration of shape matching for every body.
    pub fn solve(&mut self, p: &mut Particles, dt: f32) {
        if !self.global_rows.is_empty() {
            self.fit_frames(p, true);
            let alpha_dt = 1.0 / (dt * dt);
            for b in &self.bodies {
                if b.mode != ShapeMatching::Global {
                    continue;
                }
                let alpha_t = b.compliance * alpha_dt;
                for i in b.range() {
                    let goal = m3_mul_vec(&b.rotation, self.rest_offsets[i]) + b.center;
                    let c = p.pred[i] - goal;
                    let w = p.inv_mass[i];
                    let dl = (-c - self.global_lambda[i] * alpha_t) / (w + alpha_t);
                    self.global_lambda[i] += dl;
                    p.pred[i] += dl * w;
                }
            }
        }
        if !self.local_rows.is_empty() {
            self.solve_local(p, dt);
        }
    }

    fn solve_local(&mut self, p: &mut Particles, dt: f32) {
        let rows = &self.local_rows;
        let (offsets, indices) = (&self.offsets, &self.indices);
        let row = |i: usize| &indices[offsets[i] as usize..offsets[i + 1] as usize];
        // 1. Group centres.
        {
            let pp = &*p;
            let out = SharedMut::new(&mut self.centers[..]);
            for_each_index(rows, |i| {
                let r = row(i);
                let sum: Vec3A = r.iter().map(|&j| pp.pred[j as usize]).sum();
                // SAFETY: index i written only by this task.
                unsafe { *out.get(i) = sum / r.len().max(1) as f32 };
            });
        }
        // 2. Group rotations, with the members' orientations (eq. 7–8).
        {
            let pp = &*p;
            let ai = self.radius * self.radius / 5.0;
            let (centers, rest_centers, rest) =
                (&self.centers, &self.rest_centers, &self.rest_offsets);
            let out = SharedMut::new(&mut self.rotations[..]);
            for_each_index(rows, |i| {
                let c = centers[i];
                let cbar = rest_centers[i];
                let mut a = [[0.0f32; 3]; 3];
                for &j in row(i) {
                    let j = j as usize;
                    let rj = m3_from_quat(pp.pred_rot[j]);
                    let x = pp.pred[j] - c;
                    let xt = rest[j] - cbar;
                    for (r, (rr, xr)) in a.iter_mut().zip(rj.iter().zip([x.x, x.y, x.z])) {
                        r[0] += ai * rr[0] + xr * xt.x;
                        r[1] += ai * rr[1] + xr * xt.y;
                        r[2] += ai * rr[2] + xr * xt.z;
                    }
                }
                // SAFETY: index i written only by this task.
                unsafe { *out.get(i) = polar_rotation(&a) };
            });
        }
        // 3. Pull every member toward its goal in each of its groups (gather
        //    form of the reference's scatter; pair multipliers owned by the member).
        {
            let pp = &*p;
            let max_corr = 5.0 * self.radius;
            let (centers, rest_centers, rest, rotations, compliance) = (
                &self.centers,
                &self.rest_centers,
                &self.rest_offsets,
                &self.rotations,
                &self.compliance,
            );
            let reverse = &self.reverse;
            let lambda = SharedMut::new(&mut self.pair_lambda[..]);
            let out = SharedMut::new(&mut self.accum[..]);
            let alpha_dt = 1.0 / (dt * dt);
            for_each_index(rows, |j| {
                let wj = pp.inv_mass[j];
                let xj = pp.pred[j];
                let (a, b) = (offsets[j] as usize, offsets[j + 1] as usize);
                let mj = ((b - a) as f32).max(1.0);
                let mut sum = Vec3A::ZERO;
                for k in a..b {
                    let i = indices[k] as usize;
                    let goal = m3_mul_vec(&rotations[i], rest[j] - rest_centers[i]) + centers[i];
                    let c = xj - goal;
                    let alpha_t = compliance[i] * alpha_dt;
                    // SAFETY: entry reverse[k] of row i is the (i, j) pair, owned by j's task.
                    let l = unsafe { lambda.get(reverse[k] as usize) };
                    let dl = (-c - *l * alpha_t) / (wj + alpha_t);
                    *l += dl;
                    let raw = dl * wj / mj;
                    let scale = (max_corr * max_corr
                        / raw.length_squared().max(max_corr * max_corr))
                    .sqrt();
                    sum += raw * scale;
                }
                // SAFETY: index j written only by this task.
                unsafe { *out.get(j) = sum };
            });
        }
        for &i in rows {
            p.pred[i as usize] += self.accum[i as usize];
        }
        // 4. Store each group's rotation as its centre's predicted orientation.
        for &i in rows {
            let i = i as usize;
            let mut q = quat_from_m3(&self.rotations[i]);
            if p.rot[i].dot(q) < 0.0 {
                q = -q;
            }
            p.pred_rot[i] = q;
        }
    }
}

/// A weighted position sum and its total weight.
#[derive(Clone, Copy, Default)]
struct WeightedSum(Vec3A, f32);

impl std::ops::Add for WeightedSum {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        WeightedSum(self.0 + o.0, self.1 + o.1)
    }
}

/// Three row sums of a 3×3 moment matrix, addable for chunked reductions.
#[derive(Clone, Copy, Default)]
struct Mat3Sum([Vec3A; 3]);

impl std::ops::Add for Mat3Sum {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Mat3Sum([self.0[0] + o.0[0], self.0[1] + o.0[1], self.0[2] + o.0[2]])
    }
}
