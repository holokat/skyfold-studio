//! Particle–particle contacts (port of `core/contact/`).
//!
//! The GPU finds each pair once and scatters corrections with atomics. On the
//! CPU every particle keeps its own row of partners and computes its own side
//! of each pair (gather form). Both rows of a pair evolve bit-identically,
//! because every pair quantity is computed from the pair alone and negating a
//! vector is exact, so the result matches the scattered sum without atomics
//! and is deterministic at any thread count.

use crate::grid::{HashGrid, NeighborLists};
use crate::par::{SharedMut, for_each_index};
use crate::particles::Particles;
use glam::Vec3A;

/// Candidates are gathered within `2r · 1.1`, so particles that move into
/// contact during the solve are still caught (Macklin et al. 2014 §9).
pub(crate) const CONTACT_RADIUS_EXPANSION: f32 = 1.1;
/// Most contacts kept per particle.
pub(crate) const MAX_CONTACTS: usize = 32;

pub(crate) struct Contacts {
    pub mu_s: f32,
    pub mu_k: f32,
    partner: Vec<u32>,
    lambda: Vec<[f32; 2]>,
    normal: Vec<Vec3A>,
    count: Vec<u8>,
    /// Some particle had more than [`MAX_CONTACTS`] candidates at the last generation.
    pub overflow: bool,
}

impl Contacts {
    pub fn new(capacity: usize, mu_s: f32, mu_k: f32) -> Self {
        Self {
            mu_s,
            mu_k,
            partner: vec![0; capacity * MAX_CONTACTS],
            lambda: vec![[0.0; 2]; capacity * MAX_CONTACTS],
            normal: vec![Vec3A::ZERO; capacity * MAX_CONTACTS],
            count: vec![0; capacity],
            overflow: false,
        }
    }

    /// Find overlapping pairs among `active` particles, with multipliers zeroed.
    /// Pairs of two fluid particles, two pinned particles, or two particles in
    /// the same non-zero group are skipped. Fluid particles take their
    /// candidates from their fluid neighbor list (`fluid_rows`).
    pub fn generate(
        &mut self,
        grid: &HashGrid,
        p: &Particles,
        active: &[u32],
        nbrs: Option<&NeighborLists>,
    ) {
        let cand = 2.0 * p.radius * CONTACT_RADIUS_EXPANSION;
        let cand2 = cand * cand;
        let overflow = std::sync::atomic::AtomicBool::new(false);
        let partner = SharedMut::new(&mut self.partner[..]);
        let lambda = SharedMut::new(&mut self.lambda[..]);
        let count = SharedMut::new(&mut self.count[..]);
        for_each_index(active, |i| {
            let xi = p.pred[i];
            let gi = p.group[i];
            let wi = p.inv_mass[i];
            let fluid_i = p.kind[i].is_fluid();
            // SAFETY: row i is written only by this task.
            let row = unsafe { partner.slice(i * MAX_CONTACTS, MAX_CONTACTS) };
            let mut n = 0usize;
            let mut over = false;
            let mut consider = |j: u32, xj: Vec3A| {
                let ju = j as usize;
                if ju == i {
                    return;
                }
                if fluid_i && p.kind[ju].is_fluid() {
                    return;
                }
                if gi != 0 && gi == p.group[ju] {
                    return;
                }
                if wi <= 0.0 && p.inv_mass[ju] <= 0.0 {
                    return;
                }
                if (xi - xj).length_squared() >= cand2 {
                    return;
                }
                if n < MAX_CONTACTS {
                    row[n] = j;
                    n += 1;
                } else {
                    over = true;
                }
            };
            if fluid_i {
                if let Some(nb) = nbrs {
                    for &j in nb.row(i) {
                        if !p.kind[j as usize].is_fluid() {
                            consider(j, p.pred[j as usize]);
                        }
                    }
                }
            } else {
                grid.for_each(xi, cand, |j, slot| consider(j, grid.sorted_pos[slot]));
            }
            // SAFETY: row i is written only by this task.
            unsafe {
                *count.get(i) = n as u8;
                for l in lambda.slice(i * MAX_CONTACTS, n) {
                    *l = [0.0, 0.0];
                }
            }
            if over {
                overflow.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        self.overflow = overflow.into_inner();
    }

    /// Separate pairs that already overlap at the start of the substep, moving
    /// both the current and predicted positions (Macklin et al. 2014 §4.4).
    pub fn stabilize(&self, p: &mut Particles, active: &[u32], delta: &mut [Vec3A]) {
        let contact = 2.0 * p.radius;
        {
            let out = SharedMut::new(delta);
            let pp = &*p;
            for_each_index(active, |i| {
                let n = self.count[i] as usize;
                let mut corr = Vec3A::ZERO;
                let wi = pp.inv_mass[i];
                for &j in &self.partner[i * MAX_CONTACTS..i * MAX_CONTACTS + n] {
                    let j = j as usize;
                    let offset = pp.pos[i] - pp.pos[j];
                    let d = offset.length();
                    let w_sum = wi + pp.inv_mass[j];
                    if d <= 1e-8 || d >= contact || w_sum <= 0.0 {
                        continue;
                    }
                    let dl = (contact - d) / w_sum;
                    corr += offset / d * (wi * dl);
                }
                // SAFETY: index i written only by this task.
                unsafe { *out.get(i) = corr };
            });
        }
        for &i in active {
            let i = i as usize;
            if self.count[i] > 0 {
                p.pos[i] += delta[i];
                p.pred[i] += delta[i];
            }
        }
    }

    /// Non-penetration plus static friction for every particle's side of each
    /// of its contacts (Macklin et al. 2014 eq. 22; 2020 §3.5). Updates the
    /// multipliers and writes each particle's position correction to `delta`
    /// (overwriting it).
    pub fn solve(&mut self, p: &Particles, active: &[u32], delta: &mut [Vec3A]) {
        let contact = 2.0 * p.radius;
        let mu_s = self.mu_s;
        let partner = &self.partner;
        let count = &self.count;
        let lambda = SharedMut::new(&mut self.lambda[..]);
        let normal = SharedMut::new(&mut self.normal[..]);
        let out = SharedMut::new(delta);
        for_each_index(active, |i| {
            let n = count[i] as usize;
            let mut corr = Vec3A::ZERO;
            if n > 0 {
                let wi = p.inv_mass[i];
                let xi = p.pred[i];
                let di = xi - p.pos[i];
                // SAFETY: row i is accessed only by this task.
                let (lam, nor) = unsafe {
                    (
                        lambda.slice(i * MAX_CONTACTS, n),
                        normal.slice(i * MAX_CONTACTS, n),
                    )
                };
                for k in 0..n {
                    let j = partner[i * MAX_CONTACTS + k] as usize;
                    let xj = p.pred[j];
                    let offset = xi - xj;
                    let d = offset.length();
                    let w_sum = wi + p.inv_mass[j];
                    if d <= 1e-8 || d >= contact || w_sum <= 0.0 {
                        continue;
                    }
                    let nrm = offset / d;
                    let dln = (contact - d) / w_sum;
                    let ln = lam[k][0] + dln;
                    lam[k][0] = ln;
                    nor[k] = nrm;
                    let slip = di - (xj - p.pos[j]);
                    let tangential = slip - nrm * slip.dot(nrm);
                    let tl = tangential.length();
                    let dlt = tl / w_sum;
                    let sticks = tl > 1e-10 && dlt <= mu_s * ln - lam[k][1];
                    corr += nrm * (wi * dln);
                    if sticks {
                        corr -= tangential * (wi / w_sum);
                        lam[k][1] += dlt;
                    }
                }
            }
            // SAFETY: index i written only by this task.
            unsafe { *out.get(i) = corr };
        });
    }

    /// Kinetic friction for every particle's side of its contacts (Macklin et
    /// al. 2020 §3.6, eq. 30), as a velocity change written to `dv` (overwriting it).
    pub fn friction(&self, p: &Particles, active: &[u32], dt: f32, dv: &mut [Vec3A]) {
        let out = SharedMut::new(dv);
        for_each_index(active, |i| {
            let n = self.count[i] as usize;
            let wi = p.inv_mass[i];
            let mut acc = Vec3A::ZERO;
            for k in 0..n {
                let at = i * MAX_CONTACTS + k;
                let j = self.partner[at] as usize;
                let w_sum = wi + p.inv_mass[j];
                let ln = self.lambda[at][0];
                if w_sum <= 0.0 || ln <= 0.0 {
                    continue;
                }
                let nrm = self.normal[at];
                let v = p.vel[i] - p.vel[j];
                let vt = v - nrm * nrm.dot(v);
                let len = vt.length();
                if len < 1e-6 {
                    continue;
                }
                let change = (self.mu_k * ln * w_sum / dt).min(len);
                acc += -vt / len * change / w_sum * wi;
            }
            // SAFETY: index i written only by this task.
            unsafe { *out.get(i) = acc };
        });
    }
}
