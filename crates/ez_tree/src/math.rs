//! Bit-for-bit reimplementations of the three.js math routines ez-tree uses.
//!
//! Everything is `f64`, mirroring JavaScript numbers, and every routine keeps
//! three.js's exact operation order so rounding matches. Sources are three.js
//! r167–r179 (`Quaternion.js`, `Euler.js`, `Vector3.js`, `Matrix4.js`,
//! `MathUtils.js`); r180 replaced `Quaternion.slerp` with a different
//! algorithm, but ez-tree pins r167, so the classic version lives here.
//!
//! Vectors are plain [`glam::DVec3`]; only component-wise `+`, `-` and scalar
//! `*` (which are identical to three.js's `add`/`sub`/`multiplyScalar`) are
//! taken from glam. Everything else (dot, cross, length, normalize, ...) is
//! spelled out here so the evaluation order is under our control.

use glam::DVec3;

/// `Math.max(a, b)`: propagates NaN (Rust's `f64::max` ignores it).
#[inline]
pub(crate) fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b {
        a
    } else {
        b
    }
}

/// `Math.min(a, b)`: propagates NaN.
#[inline]
pub(crate) fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b {
        a
    } else {
        b
    }
}

/// three.js `MathUtils.clamp`: `Math.max(min, Math.min(max, value))`.
#[inline]
pub(crate) fn js_clamp(value: f64, min: f64, max: f64) -> f64 {
    js_max(min, js_min(max, value))
}

/// `Math.round`: halves round towards +∞ (Rust's `f64::round` rounds them
/// away from zero).
#[inline]
pub(crate) fn js_round(x: f64) -> f64 {
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `Vector3.dot`.
#[inline]
pub(crate) fn dot(a: DVec3, b: DVec3) -> f64 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

/// `Vector3.length`.
#[inline]
pub(crate) fn length(v: DVec3) -> f64 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

/// `Vector3.crossVectors(a, b)`.
#[inline]
pub(crate) fn cross(a: DVec3, b: DVec3) -> DVec3 {
    DVec3::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// `Vector3.divideScalar(s)`, which three.js implements as a multiplication
/// by the reciprocal.
#[inline]
pub(crate) fn divide_scalar(v: DVec3, s: f64) -> DVec3 {
    v * (1.0 / s)
}

/// `Vector3.normalize`: `divideScalar(length() || 1)`.
#[inline]
pub(crate) fn normalize(v: DVec3) -> DVec3 {
    let len = length(v);
    // `x || 1` replaces both 0 and NaN.
    let len = if len == 0.0 || len.is_nan() { 1.0 } else { len };
    divide_scalar(v, len)
}

/// `Vector3.lerpVectors(a, b, alpha)`.
#[inline]
pub(crate) fn lerp(a: DVec3, b: DVec3, alpha: f64) -> DVec3 {
    DVec3::new(
        a.x + (b.x - a.x) * alpha,
        a.y + (b.y - a.y) * alpha,
        a.z + (b.z - a.z) * alpha,
    )
}

/// `Vector3.applyQuaternion(q)` (the `t = 2 * cross(q.xyz, v)` form).
#[inline]
pub(crate) fn apply_quat(v: DVec3, q: Quat) -> DVec3 {
    let (vx, vy, vz) = (v.x, v.y, v.z);
    let (qx, qy, qz, qw) = (q.x, q.y, q.z, q.w);

    let tx = 2.0 * (qy * vz - qz * vy);
    let ty = 2.0 * (qz * vx - qx * vz);
    let tz = 2.0 * (qx * vy - qy * vx);

    DVec3::new(
        vx + qw * tx + qy * tz - qz * ty,
        vy + qw * ty + qz * tx - qx * tz,
        vz + qw * tz + qx * ty - qy * tx,
    )
}

/// `Vector3.applyEuler(e)`: `applyQuaternion(Quaternion.setFromEuler(e))`.
#[inline]
pub(crate) fn apply_euler(v: DVec3, e: Euler) -> DVec3 {
    apply_quat(v, Quat::from_euler(e))
}

/// Euler angles in radians with three.js's default `'XYZ'` order (the only
/// order ez-tree uses).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Euler {
    /// Rotation about X, radians.
    pub x: f64,
    /// Rotation about Y, radians.
    pub y: f64,
    /// Rotation about Z, radians.
    pub z: f64,
}

impl Euler {
    /// Creates XYZ-order Euler angles.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// `Euler.setFromQuaternion(q)`: goes through
    /// `Matrix4.makeRotationFromQuaternion` (i.e. `compose` with unit scale)
    /// and `Euler.setFromRotationMatrix`, including its clamp and the
    /// `0.9999999` gimbal-lock branch.
    pub fn from_quat(q: Quat) -> Self {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);

        // Matrix elements as named by setFromRotationMatrix (row/column).
        // `compose` multiplies each by the scale, which is exactly 1 here.
        let m11 = 1.0 - (yy + zz);
        let m12 = xy - wz;
        let m13 = xz + wy;
        let m22 = 1.0 - (xx + zz);
        let m23 = yz - wx;
        let m32 = yz + wx;
        let m33 = 1.0 - (xx + yy);

        let ey = js_clamp(m13, -1.0, 1.0).asin();
        if m13.abs() < 0.9999999 {
            Self::new((-m23).atan2(m33), ey, (-m12).atan2(m11))
        } else {
            Self::new(m32.atan2(m22), ey, 0.0)
        }
    }
}

/// A quaternion with three.js `Quaternion` semantics.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
    /// W (scalar) component.
    pub w: f64,
}

impl Quat {
    /// The identity rotation.
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// `Quaternion.setFromEuler(e)` for order `'XYZ'`.
    pub fn from_euler(e: Euler) -> Self {
        let c1 = (e.x / 2.0).cos();
        let c2 = (e.y / 2.0).cos();
        let c3 = (e.z / 2.0).cos();
        let s1 = (e.x / 2.0).sin();
        let s2 = (e.y / 2.0).sin();
        let s3 = (e.z / 2.0).sin();

        Self {
            x: s1 * c2 * c3 + c1 * s2 * s3,
            y: c1 * s2 * c3 - s1 * c2 * s3,
            z: c1 * c2 * s3 + s1 * s2 * c3,
            w: c1 * c2 * c3 - s1 * s2 * s3,
        }
    }

    /// `Quaternion.setFromAxisAngle(axis, angle)`; `axis` must be normalized.
    pub fn from_axis_angle(axis: DVec3, angle: f64) -> Self {
        let half = angle / 2.0;
        let s = half.sin();
        Self {
            x: axis.x * s,
            y: axis.y * s,
            z: axis.z * s,
            w: half.cos(),
        }
    }

    /// `Quaternion.length`.
    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }

    /// `Quaternion.normalize` (a zero quaternion becomes the identity).
    pub fn normalize(self) -> Self {
        let l = self.length();
        if l == 0.0 {
            Self::IDENTITY
        } else {
            let l = 1.0 / l;
            Self {
                x: self.x * l,
                y: self.y * l,
                z: self.z * l,
                w: self.w * l,
            }
        }
    }

    /// `this.slerp(qb, t)` as implemented in three.js r167–r179 (returns the
    /// new value of `this`): early-outs for `t == 0` / `t == 1`, flips `qb`
    /// when `cosHalfTheta < 0`, returns `this` unchanged when
    /// `cosHalfTheta >= 1`, and lerps + normalizes when
    /// `sqrSinHalfTheta <= f64::EPSILON`.
    pub fn slerp(self, qb: Quat, t: f64) -> Self {
        if t == 0.0 {
            return self;
        }
        if t == 1.0 {
            return qb;
        }

        let Quat { x, y, z, w } = self;
        let mut cos_half_theta = w * qb.w + x * qb.x + y * qb.y + z * qb.z;

        // `this` now holds qb (possibly negated).
        let target = if cos_half_theta < 0.0 {
            cos_half_theta = -cos_half_theta;
            Quat {
                x: -qb.x,
                y: -qb.y,
                z: -qb.z,
                w: -qb.w,
            }
        } else {
            qb
        };

        if cos_half_theta >= 1.0 {
            return self;
        }

        let sqr_sin_half_theta = 1.0 - cos_half_theta * cos_half_theta;

        if sqr_sin_half_theta <= f64::EPSILON {
            let s = 1.0 - t;
            return Quat {
                w: s * w + t * target.w,
                x: s * x + t * target.x,
                y: s * y + t * target.y,
                z: s * z + t * target.z,
            }
            .normalize();
        }

        let sin_half_theta = sqr_sin_half_theta.sqrt();
        let half_theta = sin_half_theta.atan2(cos_half_theta);
        let ratio_a = ((1.0 - t) * half_theta).sin() / sin_half_theta;
        let ratio_b = (t * half_theta).sin() / sin_half_theta;

        Quat {
            w: w * ratio_a + target.w * ratio_b,
            x: x * ratio_a + target.x * ratio_b,
            y: y * ratio_a + target.y * ratio_b,
            z: z * ratio_a + target.z * ratio_b,
        }
    }
}

/// `a * b` is three.js `multiplyQuaternions(a, b)`, i.e. `a.multiply(b)` and
/// `b.premultiply(a)`.
impl std::ops::Mul for Quat {
    type Output = Quat;

    fn mul(self, b: Quat) -> Quat {
        let (qax, qay, qaz, qaw) = (self.x, self.y, self.z, self.w);
        let (qbx, qby, qbz, qbw) = (b.x, b.y, b.z, b.w);
        Quat {
            x: qax * qbw + qaw * qbx + qay * qbz - qaz * qby,
            y: qay * qbw + qaw * qby + qaz * qbx - qax * qbz,
            z: qaz * qbw + qaw * qbz + qax * qby - qay * qbx,
            w: qaw * qbw - qax * qbx - qay * qby - qaz * qbz,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_round_matches_math_round() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(4.5), 5.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
        assert_eq!(js_round(3.2), 3.0);
    }

    #[test]
    fn js_max_min_propagate_nan() {
        assert!(js_max(1.0, f64::NAN).is_nan());
        assert!(js_min(f64::NAN, 1.0).is_nan());
        assert_eq!(js_clamp(2.0, -1.0, 1.0), 1.0);
    }

    #[test]
    fn euler_quat_round_trip() {
        let e = Euler::new(0.3, -0.7, 1.1);
        let back = Euler::from_quat(Quat::from_euler(e));
        assert!((back.x - e.x).abs() < 1e-12);
        assert!((back.y - e.y).abs() < 1e-12);
        assert!((back.z - e.z).abs() < 1e-12);
    }

    #[test]
    fn slerp_endpoints() {
        let a = Quat::from_euler(Euler::new(0.1, 0.2, 0.3));
        let b = Quat::from_euler(Euler::new(-0.4, 0.5, 0.0));
        assert_eq!(a.slerp(b, 0.0), a);
        assert_eq!(a.slerp(b, 1.0), b);
        let mid = a.slerp(b, 0.5);
        assert!((mid.length() - 1.0).abs() < 1e-12);
    }
}
