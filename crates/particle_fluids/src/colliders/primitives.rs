//! Analytic collider shapes: planes, spheres, oriented boxes, capsules.
//!
//! Port of `core/collision/primitives.ts` and the CPU half of
//! `PrimitiveSet.ts`. Distances are positive outside the shape; inverted
//! shapes flip both the distance and the gradient. Moving shapes are swept
//! within a frame: `rewind` seconds before the end of the frame a shape sat
//! `v·t` back and turned `R(−ωt)` about its pivot, so its field then at `x` is
//! its current field at `c + R(ωt)(x − c + v·t)`.

use crate::math::{quat_from_spin, quat_rotate};
use glam::{Quat, Vec3, Vec3A};

const EPSILON_GRADIENT: f32 = 1e-8;

/// A collider shape, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// Infinite plane through `point`; particles stay on the side `normal` points to.
    Plane {
        normal: Vec3,
        point: Vec3,
    },
    Sphere {
        center: Vec3,
        radius: f32,
    },
    /// Oriented box: `half_extents` along the axes of `rotation`.
    Box {
        center: Vec3,
        half_extents: Vec3,
        rotation: Quat,
    },
    /// Capsule around the segment `a`–`b`.
    Capsule {
        a: Vec3,
        b: Vec3,
        radius: f32,
    },
}

impl Shape {
    /// The point the shape turns about: the plane point, the capsule midpoint,
    /// or the center.
    pub fn pivot(&self) -> Vec3 {
        match *self {
            Shape::Plane { point, .. } => point,
            Shape::Sphere { center, .. } | Shape::Box { center, .. } => center,
            Shape::Capsule { a, b, .. } => (a + b) * 0.5,
        }
    }

    /// The shape moved so its pivot is at `pivot` and turned by `rotation`
    /// (relative to `self`, about the pivot).
    pub(crate) fn transformed(&self, pivot: Vec3, rotation: Quat) -> Shape {
        match *self {
            Shape::Plane { normal, .. } => Shape::Plane {
                normal: (rotation * normal).normalize(),
                point: pivot,
            },
            Shape::Sphere { radius, .. } => Shape::Sphere {
                center: pivot,
                radius,
            },
            Shape::Box {
                half_extents,
                rotation: r0,
                ..
            } => Shape::Box {
                center: pivot,
                half_extents,
                rotation: (rotation * r0).normalize(),
            },
            Shape::Capsule { a, b, radius } => {
                let half = rotation * ((b - a) * 0.5);
                Shape::Capsule {
                    a: pivot - half,
                    b: pivot + half,
                    radius,
                }
            }
        }
    }

    /// Signed distance (positive outside) and outward unit gradient at `x`.
    #[inline]
    pub fn sdf(&self, x: Vec3A) -> (f32, Vec3A) {
        match *self {
            Shape::Plane { normal, point } => {
                let n = Vec3A::from(normal);
                (n.dot(x - Vec3A::from(point)), n)
            }
            Shape::Sphere { center, radius } => {
                let d = x - Vec3A::from(center);
                let len = d.length();
                (len - radius, d / len.max(EPSILON_GRADIENT))
            }
            Shape::Box {
                center,
                half_extents,
                rotation,
            } => {
                let local = quat_rotate(rotation.conjugate(), x - Vec3A::from(center));
                let q = local.abs() - Vec3A::from(half_extents);
                let q_pos = q.max(Vec3A::ZERO);
                let outside = q_pos.length();
                let inside = q.x.max(q.y).max(q.z).min(0.0);
                let signs = Vec3A::new(
                    if local.x >= 0.0 { 1.0 } else { -1.0 },
                    if local.y >= 0.0 { 1.0 } else { -1.0 },
                    if local.z >= 0.0 { 1.0 } else { -1.0 },
                );
                let local_grad = if outside > EPSILON_GRADIENT {
                    q_pos * signs / outside.max(EPSILON_GRADIENT)
                } else {
                    let dom_x = q.x >= q.y && q.x >= q.z;
                    let dom_y = q.y >= q.x && q.y >= q.z;
                    if dom_x {
                        Vec3A::new(signs.x, 0.0, 0.0)
                    } else if dom_y {
                        Vec3A::new(0.0, signs.y, 0.0)
                    } else {
                        Vec3A::new(0.0, 0.0, signs.z)
                    }
                };
                (outside + inside, quat_rotate(rotation, local_grad))
            }
            Shape::Capsule { a, b, radius } => {
                let (a, b) = (Vec3A::from(a), Vec3A::from(b));
                let ab = b - a;
                let t =
                    ((x - a).dot(ab) / ab.length_squared().max(EPSILON_GRADIENT)).clamp(0.0, 1.0);
                let d = x - (a + ab * t);
                let len = d.length();
                (len - radius, d / len.max(EPSILON_GRADIENT))
            }
        }
    }
}

/// Friction and options shared by every collider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColliderOptions {
    /// Static friction coefficient. Default 0.5.
    pub mu_s: f32,
    /// Kinetic friction coefficient. Default 0.4.
    pub mu_k: f32,
    /// Keep particles inside the shape instead of outside (spheres and boxes).
    pub invert: bool,
    /// Surface velocity for friction, m/s (a conveyor belt). The shape itself
    /// doesn't move. Replaced by the derived velocity once the shape is moved
    /// with [`crate::Simulation::move_collider`].
    pub surface_velocity: Vec3,
}

impl Default for ColliderOptions {
    fn default() -> Self {
        Self {
            mu_s: 0.5,
            mu_k: 0.4,
            invert: false,
            surface_velocity: Vec3::ZERO,
        }
    }
}

impl ColliderOptions {
    /// One friction value as `Simulation.add*` takes it: `μ_s = f`, `μ_k = 0.8 f`.
    pub fn friction(friction: f32) -> Self {
        Self {
            mu_s: friction,
            mu_k: 0.8 * friction,
            ..Default::default()
        }
    }
}

/// One primitive and its motion.
#[derive(Clone, Debug)]
pub(crate) struct Primitive {
    /// Shape as added; moves are applied relative to it.
    pub base: Shape,
    /// Current placement (end of the frame).
    pub shape: Shape,
    pub opts: ColliderOptions,
    /// Linear velocity of the pivot, m/s.
    pub velocity: Vec3A,
    /// Angular velocity about the pivot, rad/s.
    pub spin: Vec3A,
    /// Sweep the shape across the frame's substeps.
    pub sweep: bool,
    pub pose_pos: Vec3,
    pub pose_rot: Quat,
    pub pending: Option<(Vec3, Quat, bool)>,
    pub enabled: bool,
}

impl Primitive {
    pub fn new(shape: Shape, opts: ColliderOptions) -> Self {
        Self {
            base: shape,
            shape,
            opts,
            velocity: Vec3A::from(opts.surface_velocity),
            spin: Vec3A::ZERO,
            sweep: false,
            pose_pos: shape.pivot(),
            pose_rot: Quat::IDENTITY,
            pending: None,
            enabled: true,
        }
    }

    /// Apply a pending move: velocity and spin are how far the shape moved and
    /// turned since the last step, divided by `dt` (as `PrimitiveSet.update`).
    pub fn update(&mut self, dt: f32) {
        let Some((pos, rot, teleport)) = self.pending.take() else {
            return;
        };
        let rot = rot.normalize();
        if teleport || dt <= 0.0 {
            self.velocity = Vec3A::ZERO;
            self.spin = Vec3A::ZERO;
        } else {
            self.velocity = Vec3A::from((pos - self.pose_pos) / dt);
            let mut turn = rot * self.pose_rot.conjugate();
            if turn.w < 0.0 {
                turn = -turn;
            }
            let s = (turn.x * turn.x + turn.y * turn.y + turn.z * turn.z).sqrt();
            let rate = if s > 1e-12 {
                2.0 * s.atan2(turn.w) / (s * dt)
            } else {
                0.0
            };
            self.spin = Vec3A::new(turn.x, turn.y, turn.z) * rate;
            self.sweep = true;
        }
        self.pose_pos = pos;
        self.pose_rot = rot;
        self.shape = self.base.transformed(pos, rot);
    }

    /// Distance and outward gradient at `x`, `rewind` seconds before the end of the frame.
    #[inline]
    pub fn eval(&self, x: Vec3A, rewind: f32) -> (f32, Vec3A) {
        let (phi, grad) = if self.sweep && rewind > 0.0 {
            let pivot = Vec3A::from(self.shape.pivot());
            let shifted = x + self.velocity * rewind;
            let turn = quat_from_spin(self.spin, rewind);
            let xr = pivot + quat_rotate(turn, shifted - pivot);
            let (phi, g) = self.shape.sdf(xr);
            (phi, quat_rotate(turn.conjugate(), g))
        } else {
            self.shape.sdf(x)
        };
        if self.opts.invert {
            (-phi, -grad)
        } else {
            (phi, grad)
        }
    }

    /// Surface velocity at `x`: `v + ω × (x − c)`, pivot taken `rewind` seconds ago.
    #[inline]
    pub fn surface_velocity(&self, x: Vec3A, rewind: f32) -> Vec3A {
        if self.spin == Vec3A::ZERO {
            return self.velocity;
        }
        let t = if self.sweep { rewind } else { 0.0 };
        let arm = x - Vec3A::from(self.shape.pivot()) + self.velocity * t;
        self.velocity + self.spin.cross(arm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_distance() {
        let b = Shape::Box {
            center: Vec3::ZERO,
            half_extents: Vec3::ONE,
            rotation: Quat::IDENTITY,
        };
        let (phi, g) = b.sdf(Vec3A::new(2.0, 0.0, 0.0));
        assert!((phi - 1.0).abs() < 1e-6 && (g.x - 1.0).abs() < 1e-6);
        let (phi, g) = b.sdf(Vec3A::new(0.0, -0.5, 0.0));
        assert!((phi + 0.5).abs() < 1e-6 && (g.y + 1.0).abs() < 1e-6);
    }

    #[test]
    fn inverted_box_keeps_particles_inside() {
        let mut p = Primitive::new(
            Shape::Box {
                center: Vec3::ZERO,
                half_extents: Vec3::ONE,
                rotation: Quat::IDENTITY,
            },
            ColliderOptions {
                invert: true,
                ..Default::default()
            },
        );
        let (phi, g) = p.eval(Vec3A::new(0.5, 0.0, 0.0), 0.0);
        assert!(
            (phi - 0.5).abs() < 1e-6 && g.x < 0.0,
            "inside is the valid side"
        );
        // A swept move: velocity and spin come from the placement change.
        p.pending = Some((Vec3::new(0.1, 0.0, 0.0), Quat::from_rotation_y(0.1), false));
        p.update(0.1);
        assert!((p.velocity.x - 1.0).abs() < 1e-5);
        assert!((p.spin.y - 1.0).abs() < 1e-4);
        assert!(p.sweep);
    }

    #[test]
    fn capsule_distance() {
        let c = Shape::Capsule {
            a: Vec3::ZERO,
            b: Vec3::Y,
            radius: 0.1,
        };
        let (phi, _) = c.sdf(Vec3A::new(0.5, 0.5, 0.0));
        assert!((phi - 0.4).abs() < 1e-6);
    }
}
