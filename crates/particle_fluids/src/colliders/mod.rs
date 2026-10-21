//! Colliders: analytic primitives, baked SDF meshes and a terrain heightfield.
//!
//! Every collider uses the reference's contact model (`core/collision/collider.ts`):
//! inside the solver iterations a touching particle is pushed out along the
//! surface normal by its penetration depth, and its slip relative to the
//! surface is cancelled while the accumulated tangential multiplier stays inside
//! the static friction cone `λ_t ≤ μ_s λ_n` (Macklin et al. 2020 §3.5). After
//! the velocity update, kinetic friction removes up to `μ_k λ_n w / dt` of the
//! tangential velocity relative to the surface (§3.6). Corrections from all
//! colliders (and particle contacts) are summed and applied together.

pub mod heightfield;
pub mod primitives;
pub mod sdf;

use glam::{Quat, Vec3, Vec3A};
use heightfield::Heightfield;
use primitives::{ColliderOptions, Primitive, Shape};
use sdf::SdfCollider;

/// `(λ_n, λ_t)` for one particle–collider pair, reset every substep.
pub(crate) type PairLambda = [f32; 2];

/// Normal push plus static friction for one particle touching one collider.
#[inline(always)]
pub(crate) fn collider_contact(
    w: f32,
    displacement: Vec3A,
    collider_displacement: Vec3A,
    n: Vec3A,
    depth: f32,
    mu_s: f32,
    lambda: &mut PairLambda,
) -> Vec3A {
    let lambda_n = lambda[0] + depth / w;
    lambda[0] = lambda_n;
    let slip = displacement - collider_displacement;
    let tangential = slip - n * slip.dot(n);
    let tl = tangential.length();
    let lambda_t = lambda[1];
    let push = n * depth;
    if tl > 1e-10 && tl / w <= mu_s * lambda_n - lambda_t {
        lambda[1] = lambda_t + tl / w;
        push - tangential
    } else {
        push
    }
}

/// Kinetic friction velocity change for one particle against one collider.
#[inline(always)]
pub(crate) fn collider_friction(
    w: f32,
    v: Vec3A,
    collider_v: Vec3A,
    n: Vec3A,
    lambda_n: f32,
    mu_k: f32,
    dt: f32,
) -> Vec3A {
    let rel = v - collider_v;
    let vt = rel - n * n.dot(rel);
    let len = vt.length();
    if len > 1e-6 {
        let change = (mu_k * lambda_n * w / dt).min(len);
        -vt / len * change
    } else {
        Vec3A::ZERO
    }
}

/// Integral of the Poly6 kernel (radius `h`) over the half-space beyond a
/// plane at distance `d`, and its derivative with respect to `d`:
/// `ψ(d) = 315/(256 h⁹) ∫_d^h (h² − s²)⁴ ds`, `ψ(0) = ½`.
#[inline(always)]
pub(crate) fn half_space_kernel(d: f32, h: f32) -> (f32, f32) {
    let x = (d / h).min(1.0);
    let x2 = x * x;
    // ∫_0^x (1 − s²)⁴ ds = x − 4x³/3 + 6x⁵/5 − 4x⁷/7 + x⁹/9; the full integral is 128/315.
    let f = x * (1.0 + x2 * (-4.0 / 3.0 + x2 * (6.0 / 5.0 + x2 * (-4.0 / 7.0 + x2 / 9.0))));
    let psi = 0.5 - 315.0 / 256.0 * f;
    let g = 1.0 - x2;
    let dpsi = -315.0 / 256.0 * g * g * g * g / h;
    (psi.max(0.0), dpsi)
}

/// A primitive's motion target for the current frame.
#[derive(Clone, Copy, Debug)]
struct Target {
    pos: Vec3,
    rot: Quat,
}

/// Everything particles collide with.
#[derive(Default)]
pub(crate) struct ColliderSet {
    pub prims: Vec<Primitive>,
    targets: Vec<Option<Target>>,
    follow: Vec<bool>,
    frame_start: Vec<(Vec3, Quat)>,
    pub sdfs: Vec<SdfCollider>,
    pub heightfield: Option<Heightfield>,
    /// Seconds before the end of the frame the current substep ends at.
    pub clock: f32,
    /// Incremented when any collider changes shape or placement.
    pub version: u64,
}

impl ColliderSet {
    /// Lambda slots per particle: primitives, then SDFs, then the heightfield.
    pub fn slots(&self) -> usize {
        self.prims.len() + self.sdfs.len() + 1
    }

    pub fn heightfield_slot(&self) -> usize {
        self.prims.len() + self.sdfs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prims.is_empty() && self.sdfs.is_empty() && self.heightfield.is_none()
    }

    pub fn add_primitive(&mut self, shape: Shape, opts: ColliderOptions) -> usize {
        self.prims.push(Primitive::new(shape, opts));
        self.targets.push(None);
        self.follow.push(false);
        self.frame_start.push((shape.pivot(), Quat::IDENTITY));
        self.version += 1;
        self.prims.len() - 1
    }

    /// Request a move to `pos`/`rot` by the end of the next frame (swept, with velocity).
    pub fn move_primitive(&mut self, slot: usize, pos: Vec3, rot: Quat) {
        self.targets[slot] = Some(Target {
            pos,
            rot: rot.normalize(),
        });
        self.follow[slot] = true;
    }

    /// Place a primitive immediately, with no velocity.
    pub fn teleport_primitive(&mut self, slot: usize, pos: Vec3, rot: Quat) {
        let p = &mut self.prims[slot];
        p.pending = Some((pos, rot, true));
        p.update(0.0);
        p.sweep = false;
        self.targets[slot] = None;
        self.frame_start[slot] = (pos, rot.normalize());
        self.version += 1;
    }

    /// Start of a host frame that runs `steps` fixed steps.
    pub fn begin_frame(&mut self) {
        for (k, p) in self.prims.iter().enumerate() {
            self.frame_start[k] = (p.pose_pos, p.pose_rot);
        }
    }

    /// Before fixed step `k` of `n` in this frame: move each targeted primitive
    /// to its interpolated placement, deriving velocity and spin.
    pub fn update(&mut self, dt: f32, k: u32, n: u32) {
        let frac = (k + 1) as f32 / n.max(1) as f32;
        for (s, p) in self.prims.iter_mut().enumerate() {
            match self.targets[s] {
                Some(t) => {
                    let (fp, fr) = self.frame_start[s];
                    let pos = fp.lerp(t.pos, frac);
                    let rot = fr.slerp(t.rot, frac);
                    p.pending = Some((pos, rot, false));
                    p.update(dt);
                    self.version += 1;
                }
                None if self.follow[s] => {
                    p.velocity = Vec3A::ZERO;
                    p.spin = Vec3A::ZERO;
                }
                None => {}
            }
        }
    }

    /// End of a host frame: targets have been reached.
    pub fn end_frame(&mut self) {
        for t in &mut self.targets {
            *t = None;
        }
    }

    pub fn frame_start_clock(&mut self, sub_dt: f32, substeps: u32) {
        self.clock = sub_dt * (substeps.saturating_sub(1)) as f32;
    }

    pub fn tick(&mut self, sub_dt: f32) {
        self.clock = (self.clock - sub_dt).max(0.0);
    }

    /// Position correction for one particle against every collider.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn solve_particle(
        &self,
        x_star: Vec3A,
        x: Vec3A,
        w: f32,
        radius: f32,
        dt: f32,
        lambda: &mut [PairLambda],
        hf_touch: &mut bool,
    ) -> Vec3A {
        let displacement = x_star - x;
        let mut corr = Vec3A::ZERO;
        for (s, prim) in self.prims.iter().enumerate() {
            if !prim.enabled {
                continue;
            }
            let (phi, n) = prim.eval(x_star, self.clock);
            if phi < radius {
                let cd = prim.surface_velocity(x_star, self.clock) * dt;
                corr += collider_contact(
                    w,
                    displacement,
                    cd,
                    n,
                    radius - phi,
                    prim.opts.mu_s,
                    &mut lambda[s],
                );
            }
        }
        let base = self.prims.len();
        for (k, sdf) in self.sdfs.iter().enumerate() {
            if !sdf.enabled {
                continue;
            }
            let reach = radius + sdf.thickness;
            let (phi, g) = sdf.eval(x_star);
            let len = g.length();
            if len > 1e-6 && phi < reach {
                corr += collider_contact(
                    w,
                    displacement,
                    Vec3A::ZERO,
                    g / len,
                    reach - phi,
                    sdf.mu_s,
                    &mut lambda[base + k],
                );
            }
        }
        if let Some(hf) = &self.heightfield {
            let (phi, n) = hf.sdf(x_star);
            if phi < radius {
                *hf_touch = true;
                let slot = self.heightfield_slot();
                corr += collider_contact(
                    w,
                    displacement,
                    Vec3A::ZERO,
                    n,
                    radius - phi,
                    hf.mu_s,
                    &mut lambda[slot],
                );
            }
        }
        corr
    }

    /// Kinetic friction velocity change for one particle.
    #[inline]
    pub fn friction_particle(
        &self,
        x_star: Vec3A,
        v: Vec3A,
        w: f32,
        dt: f32,
        lambda: &[PairLambda],
    ) -> Vec3A {
        let mut dv = Vec3A::ZERO;
        for (s, prim) in self.prims.iter().enumerate() {
            let ln = lambda[s][0];
            if ln <= 0.0 || !prim.enabled {
                continue;
            }
            let (_, g) = prim.eval(x_star, self.clock);
            let len = g.length();
            if len > 1e-6 {
                let cv = prim.surface_velocity(x_star, self.clock);
                dv += collider_friction(w, v, cv, g / len, ln, prim.opts.mu_k, dt);
            }
        }
        let base = self.prims.len();
        for (k, sdf) in self.sdfs.iter().enumerate() {
            let ln = lambda[base + k][0];
            if ln <= 0.0 || !sdf.enabled {
                continue;
            }
            let (_, g) = sdf.eval(x_star);
            let len = g.length();
            if len > 1e-6 {
                dv += collider_friction(w, v, Vec3A::ZERO, g / len, ln, sdf.mu_k, dt);
            }
        }
        if let Some(hf) = &self.heightfield {
            let ln = lambda[self.heightfield_slot()][0];
            if ln > 0.0 {
                let (_, n) = hf.sdf(x_star);
                dv += collider_friction(w, v, Vec3A::ZERO, n, ln, hf.mu_k, dt);
            }
        }
        dv
    }

    /// Density a fluid particle at `x` gets from colliders within `h`: each
    /// surface is backed by a virtual half-space of fluid at rest density
    /// (the kernel-integral form of Akinci boundary particles). Returns the
    /// volume fraction `Σ ψ(φ)` and its gradient `Σ ψ'(φ) n`; multiply both by ρ0.
    #[inline]
    pub fn wall_density(&self, x: Vec3A, h: f32) -> (f32, Vec3A) {
        let mut psi = 0.0;
        let mut grad = Vec3A::ZERO;
        let mut add = |phi: f32, n: Vec3A| {
            if phi < h {
                let (v, dv) = half_space_kernel(phi.max(0.0), h);
                psi += v;
                grad += n * dv;
            }
        };
        for prim in &self.prims {
            if prim.enabled {
                let (phi, n) = prim.eval(x, self.clock);
                add(phi, n);
            }
        }
        for sdf in &self.sdfs {
            if sdf.enabled {
                let (phi, g) = sdf.eval(x);
                let len = g.length();
                if len > 1e-6 {
                    add(phi - sdf.thickness, g / len);
                }
            }
        }
        if let Some(hf) = &self.heightfield {
            let (phi, n) = hf.sdf(x);
            add(phi, n);
        }
        (psi, grad)
    }

    /// Smallest distance from `x` to any collider (no sweep), and whether any
    /// collider exists. Used for spawning clearance and the liquid surface's walls.
    pub fn nearest_distance(&self, x: Vec3A, sdf_bound: bool) -> f32 {
        let mut nearest = f32::INFINITY;
        for prim in &self.prims {
            if prim.enabled {
                nearest = nearest.min(prim.eval(x, 0.0).0);
            }
        }
        for sdf in &self.sdfs {
            if sdf.enabled {
                let phi = sdf.eval(x).0;
                nearest = nearest.min(if sdf_bound {
                    phi.max(sdf.bound_distance(x))
                } else {
                    phi
                });
            }
        }
        if let Some(hf) = &self.heightfield {
            nearest = nearest.min(hf.sdf(x).0);
        }
        nearest
    }
}
