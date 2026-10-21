//! Growing the tree skeleton (`#generateSkeleton` / `#growBranch` /
//! `generateChildBranches` / `generateLeaves` / `#recordLeaf`). All RNG use
//! happens here, in exactly the JS order.

use std::collections::VecDeque;
use std::f64::consts::PI;

use glam::DVec3;

use crate::math::{
    Euler, Quat, apply_euler, apply_quat, cross, divide_scalar, dot, js_max, js_min, length, lerp,
    normalize,
};
use crate::options::{TreeOptions, TreeType};
use crate::rng::Rng;

/// One ring of a branch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Section {
    /// Ring centre.
    pub origin: DVec3,
    /// Ring orientation (local +Y runs along the branch).
    pub orientation: Euler,
    /// Ring radius.
    pub radius: f64,
    /// Hierarchy parameter `p` used for the wind sway weight (not part of
    /// ez-tree): 0 at the trunk base, rising by `1 / (levels + 1)` over the
    /// length of every branch, and inherited continuously by child and
    /// terminal branches from their attach point.
    pub hierarchy: f64,
}

/// A grown branch: its rings plus what meshing needs.
#[derive(Clone, Debug, PartialEq)]
pub struct SkeletonBranch {
    /// `sectionCount + 1` rings from base to tip.
    pub sections: Vec<Section>,
    /// Radial segment count at full detail.
    pub segment_count: usize,
    /// Radius at the branch base (sets the bark texture wrap count).
    pub base_radius: f64,
    /// Branch level, 0 = trunk.
    pub level: usize,
}

/// A leaf placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkeletonLeaf {
    /// Attach point on the branch.
    pub origin: DVec3,
    /// Leaf orientation (the card grows along local +Y).
    pub orientation: Euler,
    /// Card size, variance already applied.
    pub size: f64,
    /// Hierarchy parameter `p` of the branch at the attach point (see
    /// [`Section::hierarchy`]).
    pub hierarchy: f64,
}

/// The RNG-dependent part of a tree. Mesh it with [`crate::mesh`] at any
/// [`crate::Detail`] without regrowing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skeleton {
    /// Branches in growth (breadth-first queue) order.
    pub branches: Vec<SkeletonBranch>,
    /// Leaves in growth order.
    pub leaves: Vec<SkeletonLeaf>,
}

/// A queued branch (`branch.js`), plus the sway hierarchy start value.
#[derive(Clone, Copy, Debug)]
struct Branch {
    origin: DVec3,
    orientation: Euler,
    length: f64,
    radius: f64,
    level: usize,
    section_count: usize,
    segment_count: usize,
    hierarchy: f64,
}

/// Grows the skeleton for `opts`. Deterministic for a given seed and
/// identical to ez-tree's `#generateSkeleton`.
pub fn grow(opts: &TreeOptions) -> Skeleton {
    let mut grower = Grower {
        opts,
        levels: opts.branch.levels.min(3),
        rng: Rng::new(opts.seed),
        queue: VecDeque::new(),
        skeleton: Skeleton::default(),
    };

    grower.queue.push_back(Branch {
        origin: DVec3::ZERO,
        orientation: Euler::default(),
        length: opts.branch.length[0],
        radius: opts.branch.radius[0],
        level: 0,
        section_count: opts.branch.sections[0],
        segment_count: opts.branch.segments[0],
        hierarchy: 0.0,
    });

    // FIFO, like `branchQueue.shift()`.
    while let Some(branch) = grower.queue.pop_front() {
        grower.grow_branch(branch);
    }

    grower.skeleton
}

/// Where a child (branch or leaf) attaches to its parent's sections.
struct Attachment {
    origin: DVec3,
    /// Interpolated parent orientation.
    orientation: Euler,
    radius_a: f64,
    radius_b: f64,
    alpha: f64,
    hierarchy: f64,
}

struct Grower<'a> {
    opts: &'a TreeOptions,
    levels: usize,
    rng: Rng,
    queue: VecDeque<Branch>,
    skeleton: Skeleton,
}

impl Grower<'_> {
    fn grow_branch(&mut self, branch: Branch) {
        let opts = self.opts;
        let level = branch.level;
        let levels = self.levels;
        let section_count = branch.section_count as f64;

        let mut orientation = branch.orientation;
        let mut origin = branch.origin;
        // ez-tree divides by `(levels - 1)` when `type === 'Deciduous'`, but
        // the type is always lowercase, so the divisor is always 1. The
        // presets were tuned with that behaviour.
        let section_length = branch.length / section_count / 1.0;

        let force_target = normalize(DVec3::new(
            opts.branch.force.direction.x,
            opts.branch.force.direction.y,
            opts.branch.force.direction.z,
        ));

        let mut sections = Vec::with_capacity(branch.section_count + 1);

        for i in 0..=branch.section_count {
            // Fraction along the branch (`i / branch.sectionCount`).
            let t = i as f64 / section_count;
            let mut radius = branch.radius;

            // Final section of the final level tapers to (almost) zero.
            if i == branch.section_count && level == levels {
                radius = 0.001;
            } else if opts.kind == TreeType::Deciduous {
                radius *= 1.0 - opts.branch.taper[level] * t;
            } else {
                // Evergreens have no terminal branch, so they taper fully.
                radius *= 1.0 - t;
            }

            sections.push(Section {
                origin,
                orientation,
                radius,
                hierarchy: branch.hierarchy + t / (levels as f64 + 1.0),
            });

            origin += apply_euler(DVec3::new(0.0, section_length, 0.0), orientation);

            // Perturb the orientation of the next section.
            let gnarliness = js_max(1.0, 1.0 / radius.sqrt()) * opts.branch.gnarliness[level];
            orientation.x += self.rng.random(gnarliness, -gnarliness);
            orientation.z += self.rng.random(gnarliness, -gnarliness);

            let mut q_section = Quat::from_euler(orientation);
            let q_twist = Quat::from_axis_angle(DVec3::Y, opts.branch.twist[level]);
            q_section = q_section * q_twist;

            // Rotate the growth direction towards (or away from) the force.
            let section_up = apply_quat(DVec3::Y, q_section);
            let axis = cross(section_up, force_target);
            let sin_full = length(axis);
            if sin_full > 1e-6 {
                let axis = divide_scalar(axis, sin_full);
                let full_angle = sin_full.atan2(dot(section_up, force_target));
                let step = opts.branch.force.strength / radius;
                let clamped = js_max(-full_angle, js_min(full_angle, step));
                q_section = Quat::from_axis_angle(axis, clamped) * q_section;
            }

            // Trellis forces are not ported.

            orientation = Euler::from_quat(q_section);
        }

        // Deciduous trees grow a terminal branch out of the tip, or end in a
        // leaf on the last level.
        if opts.kind == TreeType::Deciduous {
            let last = *sections.last().expect("a branch has at least one section");
            if level < levels {
                self.queue.push_back(Branch {
                    origin: last.origin,
                    orientation: last.orientation,
                    length: opts.branch.length[level + 1],
                    radius: last.radius,
                    level: level + 1,
                    // Same section/segment counts as the parent so the
                    // junction lines up.
                    section_count: branch.section_count,
                    segment_count: branch.segment_count,
                    hierarchy: last.hierarchy,
                });
            } else {
                self.record_leaf(last.origin, last.orientation, last.hierarchy);
            }
        }

        if level == levels {
            self.generate_leaves(&sections);
        } else if level < levels {
            self.generate_child_branches(opts.branch.children[level], level + 1, &sections);
        }

        self.skeleton.branches.push(SkeletonBranch {
            sections,
            segment_count: branch.segment_count,
            base_radius: branch.radius,
            level,
        });
    }

    /// Finds the two sections around `start` (a fraction of the branch) and
    /// interpolates between them like ez-tree. Note the orientation is
    /// `qB.slerp(qA, alpha)`, i.e. reversed relative to the origin lerp.
    fn attach(sections: &[Section], start: f64) -> Attachment {
        let last = sections.len() - 1;
        let span = last as f64;
        // JS indexes out of bounds (and throws) for starts outside [0, 1];
        // clamp instead.
        let index = ((start * span).floor().max(0.0) as usize).min(last);
        let a = sections[index];
        let b = if index == last {
            a
        } else {
            sections[index + 1]
        };

        let alpha = (start - index as f64 / span) / (1.0 / span);

        let origin = lerp(a.origin, b.origin, alpha);
        let qa = Quat::from_euler(a.orientation);
        let qb = Quat::from_euler(b.orientation);
        let orientation = Euler::from_quat(qb.slerp(qa, alpha));

        Attachment {
            origin,
            orientation,
            radius_a: a.radius,
            radius_b: b.radius,
            alpha,
            hierarchy: a.hierarchy + (b.hierarchy - a.hierarchy) * alpha,
        }
    }

    /// Orientation of a child at `angle_deg` from the parent, rotated
    /// `radial_angle` around it: `q3.multiply(q2.multiply(q1))`.
    fn child_orientation(parent: Euler, angle_deg: f64, radial_angle: f64) -> Euler {
        let q1 = Quat::from_axis_angle(DVec3::X, angle_deg / (180.0 / PI));
        let q2 = Quat::from_axis_angle(DVec3::Y, radial_angle);
        let q3 = Quat::from_euler(parent);
        Euler::from_quat(q3 * (q2 * q1))
    }

    fn generate_child_branches(&mut self, count: usize, level: usize, sections: &[Section]) {
        let opts = self.opts;
        let radial_offset = self.rng.next_f64();
        let start_min = opts.branch.start[level];
        let height_step = (1.0 - start_min) / count as f64;
        let angle_slots = self.shuffled_indices(count);

        for (i, &slot) in angle_slots.iter().enumerate() {
            // Stratified sampling along the parent: jitter within slot i.
            let start = start_min + (i as f64 + self.rng.next_f64()) * height_step;
            let at = Self::attach(sections, start);

            let radius = opts.branch.radius[level]
                * ((1.0 - at.alpha) * at.radius_a + at.alpha * at.radius_b);

            // Stratified radial angle with a permuted slot assignment.
            let radial_jitter = self.rng.random(0.5, -0.5);
            let radial_angle =
                2.0 * PI * (radial_offset + (slot as f64 + radial_jitter) / count as f64);
            let orientation =
                Self::child_orientation(at.orientation, opts.branch.angle[level], radial_angle);

            let length = opts.branch.length[level]
                * if opts.kind == TreeType::Evergreen {
                    1.0 - start
                } else {
                    1.0
                };

            self.queue.push_back(Branch {
                origin: at.origin,
                orientation,
                length,
                radius,
                level,
                section_count: opts.branch.sections[level],
                segment_count: opts.branch.segments[level],
                hierarchy: at.hierarchy,
            });
        }
    }

    fn generate_leaves(&mut self, sections: &[Section]) {
        let opts = self.opts;
        let radial_offset = self.rng.next_f64();
        let count = opts.leaves.count;
        let start_min = opts.leaves.start;
        let height_step = (1.0 - start_min) / count as f64;
        let angle_slots = self.shuffled_indices(count);

        for (i, &slot) in angle_slots.iter().enumerate() {
            let start = start_min + (i as f64 + self.rng.next_f64()) * height_step;
            let at = Self::attach(sections, start);

            let radial_jitter = self.rng.random(0.5, -0.5);
            let radial_angle =
                2.0 * PI * (radial_offset + (slot as f64 + radial_jitter) / count as f64);
            let orientation =
                Self::child_orientation(at.orientation, opts.leaves.angle, radial_angle);

            self.record_leaf(at.origin, orientation, at.hierarchy);
        }
    }

    /// Records a leaf; the size variance is drawn here so meshing stays
    /// RNG-free.
    fn record_leaf(&mut self, origin: DVec3, orientation: Euler, hierarchy: f64) {
        let variance = self.opts.leaves.size_variance;
        let size = self.opts.leaves.size * (1.0 + self.rng.random(variance, -variance));
        self.skeleton.leaves.push(SkeletonLeaf {
            origin,
            orientation,
            size,
            hierarchy,
        });
    }

    /// Fisher–Yates shuffle of `0..count` using the tree RNG.
    fn shuffled_indices(&mut self, count: usize) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..count).collect();
        for k in (1..count).rev() {
            let r = (self.rng.next_f64() * (k + 1) as f64).floor() as usize;
            indices.swap(k, r);
        }
        indices
    }
}
