//! XPBD constraints for cloth: distance, bending, and long-range tethers.
//!
//! Port of `core/constraints/` and `cloth/{distance,bending,tether}.ts`.
//! Constraints are greedily colored so no two in a group share a particle;
//! groups are solved in order (Gauss–Seidel between groups), and each group
//! in parallel. The XPBD update is `Δλ = (−C − α̃λ) / (Σ w_k |∇_k C|² + α̃)`
//! with `α̃ = α / dt²` (Macklin et al. 2016, eq. 18).

use crate::par::{GROUP_SEQ_THRESHOLD, SharedMut};
use glam::Vec3A;
use rayon::prelude::*;

/// Greedy coloring: each constraint, in index order, takes the lowest group
/// none of its particles is in yet (`colorConstraints`).
pub(crate) fn color_constraints(arity: usize, participants: &[u32]) -> Vec<Vec<u32>> {
    let n = participants.len() / arity;
    let mut groups_of: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for c in 0..n {
        let parts = &participants[c * arity..(c + 1) * arity];
        let mut g = 0u32;
        loop {
            let conflict = parts
                .iter()
                .any(|p| groups_of.get(p).is_some_and(|gs| gs.contains(&g)));
            if !conflict {
                break;
            }
            g += 1;
        }
        if groups.len() <= g as usize {
            groups.resize(g as usize + 1, Vec::new());
        }
        groups[g as usize].push(c as u32);
        for &p in parts {
            groups_of.entry(p).or_default().push(g);
        }
    }
    groups
}

/// Constraints of one kind, with their colors and multipliers.
pub(crate) struct ConstraintSet<const N: usize> {
    /// Global particle indices per constraint.
    pub particles: Vec<[u32; N]>,
    /// Rest value per constraint (length, angle, or radius).
    pub rest: Vec<f32>,
    pub compliance: f32,
    pub lambda: Vec<f32>,
    pub groups: Vec<Vec<u32>>,
}

impl<const N: usize> ConstraintSet<N> {
    pub fn new(particles: Vec<[u32; N]>, rest: Vec<f32>, compliance: f32) -> Self {
        let flat: Vec<u32> = particles.iter().flatten().copied().collect();
        let groups = color_constraints(N, &flat);
        let lambda = vec![0.0; particles.len()];
        Self {
            particles,
            rest,
            compliance,
            lambda,
            groups,
        }
    }

    pub fn reset(&mut self) {
        self.lambda.iter_mut().for_each(|l| *l = 0.0);
    }
}

/// Solve every group in order with `solve(constraint, positions, lambda)`;
/// each group in parallel when it is large.
fn run_groups<F>(groups: &[Vec<u32>], lambda: &mut [f32], pred: &mut [Vec3A], solve: F)
where
    F: Fn(usize, &SharedMut<'_, Vec3A>, &mut f32) + Sync + Send,
{
    let pos = SharedMut::new(pred);
    let lam = SharedMut::new(lambda);
    for group in groups {
        let body = |&c: &u32| {
            // SAFETY: constraints in a group touch disjoint particles, and each
            // constraint's multiplier belongs to it alone.
            let l = unsafe { lam.get(c as usize) };
            solve(c as usize, &pos, l);
        };
        if group.len() < GROUP_SEQ_THRESHOLD {
            group.iter().for_each(body);
        } else {
            group.par_iter().with_min_len(1024).for_each(body);
        }
    }
}

impl ConstraintSet<2> {
    /// Distance constraints `C = |x_i − x_j| − L` (Macklin et al. 2016 §6.1).
    pub fn solve_distance(&mut self, pred: &mut [Vec3A], inv_mass: &[f32], dt: f32) {
        let Self {
            particles,
            rest,
            lambda,
            groups,
            compliance,
        } = self;
        let alpha_t = *compliance / (dt * dt);
        run_groups(groups, lambda, pred, |c, pos, lambda| {
            let [i, j] = particles[c].map(|v| v as usize);
            let (wi, wj) = (inv_mass[i], inv_mass[j]);
            let w_sum = wi + wj;
            // SAFETY: the group's constraints touch disjoint particles.
            let (xi, xj) = unsafe { (*pos.read(i), *pos.read(j)) };
            let offset = xi - xj;
            let len = offset.length();
            if w_sum <= 0.0 || len < 1e-12 {
                return;
            }
            let n = offset / len;
            let dl = (-(len - rest[c]) - alpha_t * *lambda) / (w_sum + alpha_t);
            // SAFETY: as above.
            unsafe {
                *pos.get(i) = xi + n * (wi * dl);
                *pos.get(j) = xj - n * (wj * dl);
            }
            *lambda += dl;
        });
    }
}

impl ConstraintSet<4> {
    /// Bending on the signed dihedral angle with Bridson's closed-form
    /// gradients (`cloth/bending.ts`).
    pub fn solve_bending(&mut self, pred: &mut [Vec3A], inv_mass: &[f32], dt: f32) {
        let Self {
            particles,
            rest,
            lambda,
            groups,
            compliance,
        } = self;
        let alpha_t = *compliance / (dt * dt);
        run_groups(groups, lambda, pred, |c, pos, lambda| {
            let [i1, i2, i3, i4] = particles[c].map(|v| v as usize);
            // SAFETY: the group's constraints touch disjoint particles.
            let (x1, x2, x3, x4) =
                unsafe { (*pos.read(i1), *pos.read(i2), *pos.read(i3), *pos.read(i4)) };
            let (w1, w2, w3, w4) = (inv_mass[i1], inv_mass[i2], inv_mass[i3], inv_mass[i4]);
            let e = x2 - x1;
            let el2 = e.length_squared();
            if el2 < 1e-24 {
                return;
            }
            let el = el2.sqrt();
            let a3 = x3 - x1;
            let b3 = x3 - x2;
            let n1 = a3.cross(b3);
            let a4 = x4 - x2;
            let b4 = x4 - x1;
            let n2 = a4.cross(b4);
            let (n1s, n2s) = (n1.length_squared(), n2.length_squared());
            if n1s < 1e-24 || n2s < 1e-24 {
                return;
            }
            let theta = n1.cross(n2).dot(e).atan2(n1.dot(n2) * el);
            let cval = theta - rest[c];
            let n1k = n1 / n1s;
            let n2k = n2 / n2s;
            let g3 = -(n1k * el);
            let g4 = -(n2k * el);
            let c31 = b3.dot(e) / el;
            let c32 = a4.dot(e) / el;
            let c41 = a3.dot(e) / el;
            let c42 = b4.dot(e) / el;
            let g1 = -(n1k * c31 + n2k * c32);
            let g2 = n1k * c41 + n2k * c42;
            let sum = g1.length_squared() * w1
                + g2.length_squared() * w2
                + g3.length_squared() * w3
                + g4.length_squared() * w4;
            if sum <= 1e-30 {
                return;
            }
            let dl = (-cval - alpha_t * *lambda) / (sum + alpha_t);
            // SAFETY: as above.
            unsafe {
                *pos.get(i1) = x1 + g1 * (w1 * dl);
                *pos.get(i2) = x2 + g2 * (w2 * dl);
                *pos.get(i3) = x3 + g3 * (w3 * dl);
                *pos.get(i4) = x4 + g4 * (w4 * dl);
            }
            *lambda += dl;
        });
    }
}

impl ConstraintSet<1> {
    /// Unilateral long-range attachments `C = |x − a| − r ≤ 0` (Kim et al.
    /// 2012). `anchors[c]` is the pinned particle each tether hangs from.
    pub fn solve_tethers(
        &mut self,
        pred: &mut [Vec3A],
        inv_mass: &[f32],
        anchors: &[u32],
        dt: f32,
    ) {
        let Self {
            particles,
            rest,
            lambda,
            groups,
            compliance,
        } = self;
        let alpha_t = *compliance / (dt * dt);
        run_groups(groups, lambda, pred, |c, pos, lambda| {
            let i = particles[c][0] as usize;
            let w = inv_mass[i];
            if w <= 0.0 {
                return;
            }
            // SAFETY: the particle is touched by this constraint alone in its
            // group; the anchor is pinned, so no constraint writes it.
            let (x, a) = unsafe { (*pos.read(i), *pos.read(anchors[c] as usize)) };
            let diff = x - a;
            let len = diff.length();
            if len <= rest[c] {
                *lambda = 0.0;
                return;
            }
            if len < 1e-12 {
                return;
            }
            let n = diff / len;
            let dl = (-(len - rest[c]) - alpha_t * *lambda) / (w + alpha_t);
            // SAFETY: as above.
            unsafe { *pos.get(i) = x + n * (w * dl) };
            *lambda += dl;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coloring_separates_shared_particles() {
        let pairs = [0u32, 1, 1, 2, 2, 3, 0, 3, 0, 2];
        let groups = color_constraints(2, &pairs);
        for g in &groups {
            let mut seen = std::collections::HashSet::new();
            for &c in g {
                assert!(seen.insert(pairs[c as usize * 2]));
                assert!(seen.insert(pairs[c as usize * 2 + 1]));
            }
        }
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 5);
    }
}
