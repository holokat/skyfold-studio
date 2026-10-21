//! Position Based Fluids (Macklin & Müller 2013) for liquids and air.
//!
//! Port of `fluids/FluidSystem.ts`, `fluids/sim/*` and `fluids/ViscositySolver.ts`.
//! Every kernel is a per-particle gather over the particle's neighbor list
//! (at most [`MAX_NEIGHBORS`] neighbors within the smoothing radius `h`,
//! including itself), so the passes run in parallel without atomics:
//!
//! - preSolve: boundary volumes (Akinci et al. 2012), neighbor lists, surface
//!   tension (Akinci et al. 2013 cohesion + curvature) and adhesion, applied
//!   as velocity changes that also move the predicted positions;
//! - solve (every iteration): density and the XPBD multiplier
//!   `λ_i = −C_i / (Σ_k w_k |∇_k C_i|² + α/dt²)` with `C_i = ρ_i/ρ0 − 1`
//!   clamped to compression; the position correction
//!   `Δx_i = (w_i/ρ0) Σ_j (λ_i + λ_j) ψ_j ∇W(x_i − x_j)`, and the matching push
//!   on dynamic boundary particles (buoyancy);
//! - postSolve: vorticity confinement and XSPH viscosity, then the implicit
//!   Jacobi viscosity solve for thick liquids.

use crate::colliders::{ColliderSet, PairLambda};
use crate::grid::{HashGrid, MAX_NEIGHBORS, NeighborLists};
use crate::math::Sph;
use crate::par::{SharedMut, for_each_index};
use crate::particles::{Kind, NO_BOUNDARY, Particles};
use glam::Vec3A;

/// Settings of one liquid. Defaults are the reference `Simulation.addFluid` values.
#[derive(Clone, Debug, PartialEq)]
pub struct LiquidParams {
    /// XSPH viscosity blend factor. About 0.01 for water, up to 0.3 for syrup.
    pub viscosity: f32,
    /// Pulls the liquid into drops and smooth sheets (Akinci 2013). Default 0.1.
    pub surface_tension: f32,
    /// Vorticity confinement strength. Default 0.02.
    pub vorticity: f32,
    /// Attraction toward cloth and soft-body surfaces. Default 0.1.
    pub adhesion: f32,
    /// Kinematic viscosity of the implicit solver for honey-like liquids
    /// (about 20 for honey). `None` skips the solver.
    pub thickness: Option<f32>,
    /// Jacobi sweeps of the implicit viscosity solver. `None`: the reference's
    /// `round(16 · cbrt(count / 10000))`, clamped to 1–64.
    pub thickness_iterations: Option<u32>,
    /// Rest density, kg/m³. Default 1000.
    pub rest_density: f32,
    /// XPBD compliance of the density constraint. Default 1e-4.
    pub compliance: f32,
    /// Count colliders (planes, boxes, terrain, meshes) as fluid at rest
    /// density beyond their surface when computing density (extension).
    /// Without it, particles next to a wall miss half their kernel and pack
    /// tighter there, so shallow liquid loses visible volume and air sags.
    /// `false` reproduces the reference exactly. Default true.
    pub wall_density: bool,
}

impl Default for LiquidParams {
    fn default() -> Self {
        Self {
            viscosity: 0.01,
            surface_tension: 0.1,
            vorticity: 0.02,
            adhesion: 0.1,
            thickness: None,
            thickness_iterations: None,
            rest_density: 1000.0,
            compliance: 1e-4,
            wall_density: true,
        }
    }
}

/// One fluid (a liquid or the air of a smoke simulation).
pub(crate) struct FluidMat {
    pub kind: Kind,
    pub params: LiquidParams,
    /// Mass of one particle, `ρ0 · spacing³`.
    pub mass: f32,
    /// Rest volume of one particle, `spacing³`.
    pub particle_volume: f32,
    /// Live particles, ascending.
    pub members: Vec<u32>,
}

impl FluidMat {
    pub fn new(kind: Kind, params: LiquidParams, spacing: f32) -> Self {
        let particle_volume = spacing * spacing * spacing;
        Self {
            kind,
            mass: params.rest_density * particle_volume,
            particle_volume,
            params,
            members: Vec::new(),
        }
    }
}

/// Adhesion spline `A(r)` (Akinci et al. 2013, eq. 7), non-zero on `(h/2, h]`.
#[inline(always)]
fn adhesion_spline(r: f32, h: f32, coef: f32) -> f32 {
    if 2.0 * r > h && r <= h {
        let root = (-4.0 * r * r / h + 6.0 * r - 2.0 * h)
            .max(0.0)
            .sqrt()
            .sqrt();
        root * coef
    } else {
        0.0
    }
}

/// Cohesion spline `C(r)` (Akinci et al. 2013, eq. 2).
#[inline(always)]
fn cohesion_spline(r: f32, h: f32, coef: f32, h6_64: f32) -> f32 {
    if r > 0.0 && r <= h {
        let gap = (h - r).max(0.0);
        let shared = gap * gap * gap * r * r * r;
        let value = if 2.0 * r > h {
            shared
        } else {
            2.0 * shared - h6_64
        };
        value * coef
    } else {
        0.0
    }
}

/// Collider work fused into the fluid passes when every particle is a fluid
/// particle: collider corrections depend only on the particle itself, so they
/// can run in the same parallel pass as the fluid's own correction, saving a
/// fork-join per iteration (and one for friction).
pub(crate) struct FusedColliders<'a> {
    pub colliders: &'a ColliderSet,
    pub lambda: SharedMut<'a, PairLambda>,
    pub touch: SharedMut<'a, bool>,
    pub slots: usize,
    pub radius: f32,
}

/// Periodic x/z boundaries (the smoke wind tunnel): offsets between particles
/// use the minimum image, and neighbor lists include particles across the seams.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Periodic {
    pub min: Vec3A,
    pub size: Vec3A,
}

impl Periodic {
    /// Minimum-image offset along x and z.
    #[inline(always)]
    pub fn wrap(&self, d: Vec3A) -> Vec3A {
        Vec3A::new(
            d.x - self.size.x * (d.x / self.size.x).round(),
            d.y,
            d.z - self.size.z * (d.z / self.size.z).round(),
        )
    }
}

#[inline(always)]
fn wrap(periodic: Option<Periodic>, d: Vec3A) -> Vec3A {
    match periodic {
        Some(p) => p.wrap(d),
        None => d,
    }
}

/// Per-particle scratch and neighbor lists shared by every fluid.
pub(crate) struct FluidSolver {
    pub sph: Sph,
    pub nbrs: NeighborLists,
    /// Every live fluid particle (all fluids), ascending.
    pub rows: Vec<u32>,
    /// The same particles in the grid's cell order, refreshed with the lists;
    /// per-row passes walk this order.
    order: Vec<u32>,
    /// Periodic x/z boundaries, if any.
    pub periodic: Option<Periodic>,
    /// Boundary particles (cloth and soft-body surfaces), ascending.
    pub boundary_rows: Vec<u32>,
    /// `rows ∪ boundary_rows`: the particles that get a neighbor list.
    pub list_rows: Vec<u32>,
    /// Per particle: has a neighbor list.
    is_row: Vec<bool>,
    pub density: Vec<f32>,
    pub lambda: Vec<f32>,
    /// Per particle: fluid index, or `u16::MAX` for non-fluid particles.
    fid: Vec<u16>,
    /// Per particle, refreshed each iteration: predicted position in xyz and the
    /// SPH mass in w (negated boundary volume for boundary particles).
    packed: Vec<glam::Vec4>,
    /// `ψ_j ∇W(x_i − x_j)` per (fluid row, neighbor) from the λ pass, reused by the Δx pass.
    grad_cache: Vec<Vec3A>,
    /// Unit offset (xyz) and distance (w) per (fluid row, neighbor), shared by
    /// the two surface-tension passes.
    pair_cache: Vec<glam::Vec4>,
    /// Gradient of the collider (wall) density per particle, from the λ pass.
    wall_grad: Vec<Vec3A>,
    delta: Vec<Vec3A>,
    normal: Vec<Vec3A>,
    dv: Vec<Vec3A>,
    omega: Vec<Vec3A>,
    omega_len: Vec<f32>,
    eta: Vec<Vec3A>,
    ping: Vec<Vec3A>,
    pong: Vec<Vec3A>,
    rhs_w: Vec<f32>,
    vis_w: Vec<f32>,
}

impl FluidSolver {
    pub fn new(capacity: usize, h: f32) -> Self {
        Self {
            sph: Sph::new(h),
            nbrs: NeighborLists::new(capacity),
            rows: Vec::new(),
            order: Vec::new(),
            periodic: None,
            boundary_rows: Vec::new(),
            list_rows: Vec::new(),
            is_row: vec![false; capacity],
            density: vec![0.0; capacity],
            lambda: vec![0.0; capacity],
            fid: vec![u16::MAX; capacity],
            packed: vec![glam::Vec4::ZERO; capacity],
            grad_cache: Vec::new(),
            pair_cache: Vec::new(),
            wall_grad: vec![Vec3A::ZERO; capacity],
            delta: vec![Vec3A::ZERO; capacity],
            normal: vec![Vec3A::ZERO; capacity],
            dv: vec![Vec3A::ZERO; capacity],
            omega: vec![Vec3A::ZERO; capacity],
            omega_len: vec![0.0; capacity],
            eta: vec![Vec3A::ZERO; capacity],
            ping: Vec::new(),
            pong: Vec::new(),
            rhs_w: Vec::new(),
            vis_w: Vec::new(),
        }
    }

    /// Recompute the row lists after particles were added or removed.
    pub fn refresh_rows(&mut self, fluids: &[FluidMat], boundary: &[u32]) {
        self.rows.clear();
        for f in fluids {
            self.rows.extend_from_slice(&f.members);
        }
        self.rows.sort_unstable();
        self.boundary_rows.clear();
        self.boundary_rows.extend_from_slice(boundary);
        self.boundary_rows.sort_unstable();
        self.list_rows.clear();
        self.list_rows.extend_from_slice(&self.rows);
        self.list_rows.extend_from_slice(&self.boundary_rows);
        self.list_rows.sort_unstable();
        self.list_rows.dedup();
        self.is_row.iter_mut().for_each(|r| *r = false);
        for &i in &self.list_rows {
            self.is_row[i as usize] = true;
        }
    }

    /// Reset the density of a newly emitted particle to rest density.
    pub fn reset_particle(&mut self, i: usize, rest_density: f32) {
        self.density[i] = rest_density;
        self.lambda[i] = 0.0;
        self.omega_len[i] = 0.0;
    }

    /// Boundary volume `V_i = 1 / Σ_k W(x_i − x_k)` over the particle's own
    /// boundary range (Akinci et al. 2012, eq. 4), from predicted positions.
    pub fn boundary_volumes(&self, grid: &HashGrid, p: &mut Particles) {
        let sph = self.sph;
        let Particles {
            pred,
            boundary_id,
            boundary_volume,
            ..
        } = p;
        let out = SharedMut::new(&mut boundary_volume[..]);
        let (pred, boundary_id) = (&*pred, &*boundary_id);
        for_each_index(&self.boundary_rows, |i| {
            let id = boundary_id[i];
            let xi = pred[i];
            let mut sum = 0.0;
            grid.for_each(xi, sph.h, |j, _| {
                if boundary_id[j as usize] != id {
                    return;
                }
                let r2 = (xi - pred[j as usize]).length_squared();
                if r2 < sph.h2 {
                    sum += sph.poly6_r2(r2);
                }
            });
            // SAFETY: index i written only by this task.
            unsafe { *out.get(i) = 1.0 / sum.max(1e-12) };
        });
    }

    /// Rebuild every neighbor list from the grid.
    pub fn build_lists(&mut self, grid: &HashGrid, p: &Particles) {
        for (i, f) in self.fid.iter_mut().enumerate() {
            *f = if p.kind[i].is_fluid() {
                p.material[i]
            } else {
                u16::MAX
            };
        }
        match self.periodic {
            None => self.nbrs.build(grid, &self.is_row, self.sph.h2),
            Some(pw) => self
                .nbrs
                .build_periodic(grid, &self.is_row, self.sph.h2, pw.min, pw.size),
        }
        // Fluid rows in the grid's cell order: consecutive rows share most of
        // their neighbors, so the per-row passes stay in cache.
        self.order.clear();
        for &i in &grid.sorted {
            if self.fid[i as usize] != u16::MAX && self.is_row[i as usize] {
                self.order.push(i);
            }
        }
    }

    #[inline(always)]
    fn same_fluid(p: &Particles, i: usize, j: usize) -> bool {
        p.kind[j].is_fluid() && p.material[j] == p.material[i]
    }

    /// Surface tension and adhesion as velocity changes, also moving the
    /// predicted positions by `Δv · dt` (before the pressure solve).
    pub fn surface_forces(&mut self, fluids: &[FluidMat], p: &mut Particles, dt: f32) {
        let any_tension = fluids
            .iter()
            .any(|f| f.params.surface_tension != 0.0 && !f.members.is_empty());
        let any_adhesion =
            fluids.iter().any(|f| f.params.adhesion != 0.0) && !self.boundary_rows.is_empty();
        if !any_tension && !any_adhesion {
            return;
        }
        let sph = self.sph;
        let h = sph.h;
        let periodic = self.periodic;
        let nbrs = &self.nbrs;
        let rows = &self.order;
        if self.pair_cache.len() < rows.len() * MAX_NEIGHBORS {
            self.pair_cache
                .resize(rows.len() * MAX_NEIGHBORS, glam::Vec4::ZERO);
        }
        let pp = &*p;
        // Pass 1: unit offsets and distances to every neighbor (cached for
        // pass 2), and the color-field normal `n_i = h Σ_j V_j ∇W`.
        {
            let out = SharedMut::new(&mut self.normal[..]);
            let cache = SharedMut::new(&mut self.pair_cache[..]);
            crate::par::for_each_range(rows.len(), |r| {
                let i = rows[r] as usize;
                let f = &fluids[pp.material[i] as usize];
                let xi = pp.pred[i];
                let row = nbrs.row(i);
                // SAFETY: cache row r is written only by this task.
                let pc = unsafe { cache.slice(r * MAX_NEIGHBORS, row.len()) };
                let mut sum = Vec3A::ZERO;
                for (k, &j) in row.iter().enumerate() {
                    let j = j as usize;
                    let offset = wrap(periodic, xi - pp.pred[j]);
                    let len = offset.length();
                    let unit = if len > 1e-20 {
                        offset / len
                    } else {
                        Vec3A::ZERO
                    };
                    pc[k] = unit.extend(len);
                    let bv = pp.boundary_volume[j];
                    let vol = if bv > 0.0 { bv } else { f.particle_volume };
                    let t = (h - len).max(0.0);
                    sum += unit * (-(sph.spiky * t * t) * vol);
                }
                // SAFETY: index i written only by this task.
                unsafe { *out.get(i) = sum * h };
            });
        }
        let coh_coef = (32.0 / (std::f64::consts::PI * (h as f64).powi(9))) as f32;
        let h6_64 = h.powi(6) / 64.0;
        let adh_coef = (0.007 / (h as f64).powf(3.25)) as f32;
        let normal = &self.normal;
        let density = &self.density;
        let cache = &self.pair_cache;
        let fid = &self.fid;
        let out = SharedMut::new(&mut self.dv[..]);
        crate::par::for_each_range(rows.len(), |r| {
            let i = rows[r] as usize;
            let f = &fluids[pp.material[i] as usize];
            let (gamma, beta) = (f.params.surface_tension, f.params.adhesion);
            let m = f.mass;
            let rho0 = f.params.rest_density;
            let me = fid[i];
            let mut dv = Vec3A::ZERO;
            let ni = normal[i];
            let rho_i = density[i];
            for (k, &j) in nbrs.row(i).iter().enumerate() {
                let j = j as usize;
                let c = cache[r * MAX_NEIGHBORS + k];
                let len = c.w;
                if len < 1e-20 {
                    continue;
                }
                let unit = Vec3A::from(c.truncate());
                if gamma != 0.0 && fid[j] == me && j != i {
                    // Cohesion and curvature, K_ij = 2ρ0 / (ρ_i + ρ_j) (Akinci et al. 2013).
                    let kij = rho0 * 2.0 / (rho_i + density[j]).max(1.0);
                    let cohesion =
                        unit * (-(gamma * m * m * cohesion_spline(len, h, coh_coef, h6_64)));
                    let curvature = (ni - normal[j]) * (-(gamma * m));
                    dv += (cohesion + curvature) * kij / m * dt;
                }
                if beta != 0.0 {
                    let vol = pp.boundary_volume[j];
                    if vol > 0.0 {
                        dv += unit * (-(beta * m * rho0 * vol * adhesion_spline(len, h, adh_coef)))
                            / m
                            * dt;
                    }
                }
            }
            // SAFETY: index i written only by this task.
            unsafe { *out.get(i) = dv };
        });
        for &i in &self.rows {
            let i = i as usize;
            let dv = self.dv[i];
            p.vel[i] += dv;
            p.pred[i] += dv * dt;
        }
    }

    /// One pressure iteration: density, λ, the position corrections and the
    /// boundary reaction, applied to the predicted positions.
    pub fn solve_density(
        &mut self,
        fluids: &[FluidMat],
        p: &mut Particles,
        dt: f32,
        walls: Option<&ColliderSet>,
        fused: Option<&FusedColliders<'_>>,
    ) {
        let sph = self.sph;
        let periodic = self.periodic;
        // Pack each particle's position and SPH mass into one 16-byte record.
        for (i, q) in self.packed.iter_mut().enumerate() {
            let x = p.pred[i];
            let v = p.boundary_volume[i];
            let w = p.inv_mass[i];
            let psi = if v > 0.0 {
                -v
            } else if w > 0.0 {
                1.0 / w
            } else {
                0.0
            };
            *q = glam::Vec4::new(x.x, x.y, x.z, psi);
        }
        let rows = &self.order;
        if self.grad_cache.len() < rows.len() * MAX_NEIGHBORS {
            self.grad_cache
                .resize(rows.len() * MAX_NEIGHBORS, Vec3A::ZERO);
        }
        let nbrs = &self.nbrs;
        let packed = &self.packed;
        {
            let pp = &*p;
            let out_rho = SharedMut::new(&mut self.density[..]);
            let out_lambda = SharedMut::new(&mut self.lambda[..]);
            let out_wall = SharedMut::new(&mut self.wall_grad[..]);
            let cache = SharedMut::new(&mut self.grad_cache[..]);
            crate::par::for_each_range(rows.len(), |r| {
                let i = rows[r] as usize;
                let f = &fluids[pp.material[i] as usize];
                let rho0 = f.params.rest_density;
                let xi = Vec3A::from(packed[i].truncate());
                let mut rho = 0.0;
                let mut grad_sum = Vec3A::ZERO;
                let mut grad_sq = 0.0;
                let row = nbrs.row(i);
                // SAFETY: cache row r is written only by this task.
                let gc = unsafe { cache.slice(r * MAX_NEIGHBORS, row.len()) };
                for (k, &j) in row.iter().enumerate() {
                    let q = packed[j as usize];
                    let offset = wrap(periodic, xi - Vec3A::from(q.truncate()));
                    let psi = if q.w < 0.0 { -q.w * rho0 } else { q.w };
                    rho += psi * sph.poly6_r2(offset.length_squared());
                    let g = sph.spiky_grad(offset) * psi;
                    gc[k] = g;
                    grad_sum += g;
                    grad_sq += g.length_squared() * pp.inv_mass[j as usize];
                }
                let mut wall = Vec3A::ZERO;
                if let Some(colliders) = walls.filter(|_| f.params.wall_density) {
                    let (psi, g) = colliders.wall_density(xi, sph.h);
                    rho += rho0 * psi;
                    wall = g * rho0;
                    grad_sum += wall;
                }
                // SAFETY: index i written only by this task.
                unsafe { *out_wall.get(i) = wall };
                let inv_rho0 = 1.0 / rho0;
                let c = rho * inv_rho0 - 1.0;
                let lambda = if c <= 0.0 {
                    0.0
                } else {
                    let norm = (grad_sum.length_squared() * pp.inv_mass[i] + grad_sq)
                        * inv_rho0
                        * inv_rho0;
                    let alpha = f.params.compliance / (dt * dt);
                    -c / (norm + alpha).max(1e-20)
                };
                // SAFETY: index i written only by this task.
                unsafe {
                    *out_rho.get(i) = rho;
                    *out_lambda.get(i) = lambda;
                }
            });
        }
        {
            let pp = &*p;
            let lambda = &self.lambda;
            let fid = &self.fid;
            let cache = &self.grad_cache;
            let wall_grad = &self.wall_grad;
            let out = SharedMut::new(&mut self.delta[..]);
            crate::par::for_each_range(rows.len(), |r| {
                let i = rows[r] as usize;
                let rho0 = fluids[pp.material[i] as usize].params.rest_density;
                let li = lambda[i];
                let me = fid[i];
                let mut sum = Vec3A::ZERO;
                let row = nbrs.row(i);
                for (k, &j) in row.iter().enumerate() {
                    let j = j as usize;
                    let lj = if fid[j] == me { lambda[j] } else { 0.0 };
                    sum += cache[r * MAX_NEIGHBORS + k] * (li + lj);
                }
                sum += wall_grad[i] * li;
                let w = pp.inv_mass[i];
                let mut delta = sum / rho0 * w;
                if let Some(fc) = fused
                    && w > 0.0
                {
                    // SAFETY: particle i's collider row and touch flag belong to this task.
                    unsafe {
                        let row = fc.lambda.slice(i * fc.slots, fc.slots);
                        delta += fc.colliders.solve_particle(
                            pp.pred[i] + delta,
                            pp.pos[i],
                            w,
                            fc.radius,
                            dt,
                            row,
                            fc.touch.get(i),
                        );
                    }
                }
                // SAFETY: index i written only by this task.
                unsafe { *out.get(i) = delta };
            });
            // Reaction on dynamic boundary particles: Δx_b = −(λ_i/ρ0) ψ_b w_b ∇W(x_i − x_b).
            for_each_index(&self.boundary_rows, |b| {
                let wb = pp.inv_mass[b];
                let vb = pp.boundary_volume[b];
                let mut sum = Vec3A::ZERO;
                if wb > 0.0 && vb > 0.0 {
                    let xb = pp.pred[b];
                    for &i in nbrs.row(b) {
                        let i = i as usize;
                        if fid[i] == u16::MAX {
                            continue;
                        }
                        let li = lambda[i];
                        if li == 0.0 {
                            continue;
                        }
                        let rho0 = fluids[fid[i] as usize].params.rest_density;
                        let psi = rho0 * vb;
                        sum += sph.spiky_grad(pp.pred[i] - xb) * (-(li / rho0) * psi * wb);
                    }
                }
                // SAFETY: index b written only by this task (boundary and fluid rows are disjoint).
                unsafe { *out.get(b) = sum };
            });
        }
        for &i in &self.rows {
            p.pred[i as usize] += self.delta[i as usize];
        }
        for &b in &self.boundary_rows {
            p.pred[b as usize] += self.delta[b as usize];
        }
    }

    /// Vorticity confinement and XSPH viscosity (Macklin & Müller 2013 §5, eq. 17).
    pub fn velocity_post(
        &mut self,
        fluids: &[FluidMat],
        p: &mut Particles,
        dt: f32,
        fused: Option<&FusedColliders<'_>>,
    ) {
        let any_vort = fluids
            .iter()
            .any(|f| f.params.vorticity != 0.0 && !f.members.is_empty());
        let any_visc = fluids
            .iter()
            .any(|f| f.params.viscosity != 0.0 && !f.members.is_empty());
        if !any_vort && !any_visc && fused.is_none() {
            return;
        }
        let sph = self.sph;
        let periodic = self.periodic;
        let nbrs = &self.nbrs;
        let rows = &self.order;
        if self.grad_cache.len() < rows.len() * MAX_NEIGHBORS {
            self.grad_cache
                .resize(rows.len() * MAX_NEIGHBORS, Vec3A::ZERO);
        }
        {
            let pp = &*p;
            let out_omega = SharedMut::new(&mut self.omega[..]);
            let out_len = SharedMut::new(&mut self.omega_len[..]);
            let out_dv = SharedMut::new(&mut self.dv[..]);
            let cache = SharedMut::new(&mut self.grad_cache[..]);
            crate::par::for_each_range(rows.len(), |r| {
                let i = rows[r] as usize;
                let f = &fluids[pp.material[i] as usize];
                let (eps, c) = (f.params.vorticity, f.params.viscosity);
                let xi = pp.pos[i];
                let vi = pp.vel[i];
                let mut curl = Vec3A::ZERO;
                let mut smoothing = Vec3A::ZERO;
                if eps != 0.0 || c != 0.0 {
                    let row = nbrs.row(i);
                    // SAFETY: cache row r is written only by this task.
                    let gc = unsafe { cache.slice(r * MAX_NEIGHBORS, row.len()) };
                    for (k, &j) in row.iter().enumerate() {
                        let j = j as usize;
                        let offset = wrap(periodic, xi - pp.pos[j]);
                        let vj = pp.vel[j];
                        let bv = pp.boundary_volume[j];
                        let vol = if bv > 0.0 { bv } else { f.particle_volume };
                        if eps != 0.0 {
                            let g = sph.spiky_grad(offset);
                            gc[k] = g;
                            curl += (vi - vj).cross(g) * vol;
                        }
                        if c != 0.0 {
                            smoothing += (vj - vi) * (sph.poly6_r2(offset.length_squared()) * vol);
                        }
                    }
                }
                // SAFETY: index i written only by this task.
                unsafe {
                    *out_omega.get(i) = curl;
                    *out_len.get(i) = curl.length();
                    *out_dv.get(i) = smoothing * c;
                }
            });
        }
        // η = ∇|ω| from fluid neighbors, then the velocity change
        // `ε (η̂ × ω) dt + Δv_XSPH`, then (fused) collider friction. Nothing in
        // this pass reads velocities, so each task updates its own particle.
        let pp = &mut *p;
        let omega_len = &self.omega_len;
        let omega = &self.omega;
        let dv = &self.dv;
        let fid = &self.fid;
        let cache = &self.grad_cache;
        let Particles {
            vel,
            pred,
            inv_mass,
            boundary_volume,
            material,
            ..
        } = pp;
        let (pred, inv_mass, boundary_volume, material) =
            (&*pred, &*inv_mass, &*boundary_volume, &*material);
        let vel = SharedMut::new(&mut vel[..]);
        let out_eta = SharedMut::new(&mut self.eta[..]);
        crate::par::for_each_range(rows.len(), |r| {
            let i = rows[r] as usize;
            let f = &fluids[material[i] as usize];
            let eps = f.params.vorticity;
            let mut change = dv[i];
            if eps != 0.0 {
                let oi = omega_len[i];
                let me = fid[i];
                let mut eta = Vec3A::ZERO;
                for (k, &j) in nbrs.row(i).iter().enumerate() {
                    let j = j as usize;
                    if boundary_volume[j] > 0.0 {
                        continue;
                    }
                    let oj = if fid[j] == me { omega_len[j] } else { 0.0 };
                    eta += cache[r * MAX_NEIGHBORS + k] * ((oj - oi) * f.particle_volume);
                }
                // SAFETY: index i written only by this task.
                unsafe { *out_eta.get(i) = eta };
                let len = eta.length();
                if len >= 1e-12 {
                    change += (eta / len).cross(omega[i]) * (eps * dt);
                }
            }
            // SAFETY: velocity i is touched only by this task; no task reads velocities here.
            let v = unsafe { vel.get(i) };
            *v += change;
            if let Some(fc) = fused {
                let w = inv_mass[i];
                if w > 0.0 {
                    // SAFETY: particle i's collider row belongs to this task.
                    let row = unsafe { fc.lambda.slice(i * fc.slots, fc.slots) };
                    *v += fc.colliders.friction_particle(pred[i], *v, w, dt, row);
                }
            }
        });
    }

    /// Implicit viscosity for thick liquids: Jacobi sweeps of
    /// `(I − dt ν L) v = v₀`, with `L` approximated as `10/h²` times the
    /// difference from the neighbors' weighted mean (port of `ViscositySolver`).
    pub fn viscosity_solve(&mut self, fluid: &FluidMat, p: &mut Particles, dt: f32) {
        let Some(nu) = fluid.params.thickness else {
            return;
        };
        if fluid.members.is_empty() {
            return;
        }
        let cap = p.capacity();
        if self.ping.len() != cap {
            self.ping = vec![Vec3A::ZERO; cap];
            self.pong = vec![Vec3A::ZERO; cap];
            self.rhs_w = vec![0.0; cap];
            self.vis_w = vec![0.0; cap * MAX_NEIGHBORS];
        }
        let h = self.sph.h;
        let sph = self.sph;
        let nbrs = &self.nbrs;
        let rows = &fluid.members;
        let iterations = fluid.params.thickness_iterations.unwrap_or_else(|| {
            ((16.0 * (fluid.members.len() as f32 / 10000.0).cbrt()).round() as u32).clamp(1, 64)
        });
        {
            let pp = &*p;
            let w_out = SharedMut::new(&mut self.vis_w[..]);
            let rhs_out = SharedMut::new(&mut self.rhs_w[..]);
            let ping = SharedMut::new(&mut self.ping[..]);
            for_each_index(rows, |i| {
                let xi = pp.pos[i];
                let mut total = 0.0;
                // SAFETY: row i written only by this task.
                let ws = unsafe { w_out.slice(i * MAX_NEIGHBORS, MAX_NEIGHBORS) };
                for (k, &j) in nbrs.row(i).iter().enumerate() {
                    let w = sph.poly6_r2((xi - pp.pos[j as usize]).length_squared())
                        * fluid.particle_volume;
                    ws[k] = w;
                    total += w;
                }
                // SAFETY: index i written only by this task.
                unsafe {
                    *rhs_out.get(i) = total;
                    *ping.get(i) = pp.vel[i];
                }
            });
        }
        let rate = nu * dt * 10.0 / (h * h);
        let pp = &*p;
        let in_fluid = |j: usize, i: usize| Self::same_fluid(pp, i, j);
        for it in 0..iterations {
            let (src, dst) = if it % 2 == 0 {
                (&self.ping, &mut self.pong)
            } else {
                (&self.pong, &mut self.ping)
            };
            let out = SharedMut::new(&mut dst[..]);
            let vis_w = &self.vis_w;
            let rhs_w = &self.rhs_w;
            for_each_index(rows, |i| {
                let mut sum = Vec3A::ZERO;
                for (k, &j) in nbrs.row(i).iter().enumerate() {
                    let j = j as usize;
                    if in_fluid(j, i) {
                        sum += src[j] * vis_w[i * MAX_NEIGHBORS + k];
                    }
                }
                let v = (pp.vel[i] + sum * rate) / (rhs_w[i] * rate + 1.0);
                // SAFETY: index i written only by this task.
                unsafe { *out.get(i) = v };
            });
        }
        let result = if iterations % 2 == 1 {
            &self.pong
        } else {
            &self.ping
        };
        for &i in rows {
            p.vel[i as usize] = result[i as usize];
        }
    }

    /// Number of liquid particles within `radius` of boundary particle `b`
    /// (from its neighbor list), for cloth wetness.
    pub fn liquid_touches(&self, p: &Particles, b: usize, radius2: f32) -> u32 {
        if p.boundary_id[b] == NO_BOUNDARY {
            return 0;
        }
        let xb = p.pos[b];
        self.nbrs
            .row(b)
            .iter()
            .filter(|&&j| {
                let j = j as usize;
                p.kind[j] == Kind::Liquid && (p.pos[j] - xb).length_squared() < radius2
            })
            .count() as u32
    }
}
