//! Small math helpers shared by every solver: boxes, SPH kernels, the 3×3
//! polar decomposition, quaternion conversions, Morton hashing and a seeded
//! random number generator.

use glam::{Quat, Vec3, Vec3A};

/// An axis-aligned box in world space, metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// Box from a center and half extents.
    pub fn from_center_half(center: Vec3, half: Vec3) -> Self {
        Self::new(center - half, center + half)
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn volume(&self) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        let s = self.size();
        s.x * s.y * s.z
    }

    /// True when `max` is not above `min` on some axis.
    pub fn is_empty(&self) -> bool {
        self.max.x <= self.min.x || self.max.y <= self.min.y || self.max.z <= self.min.z
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Grow (positive) or shrink (negative) the box by `amount` on every side.
    pub fn expanded(&self, amount: f32) -> Self {
        Self::new(
            self.min - Vec3::splat(amount),
            self.max + Vec3::splat(amount),
        )
    }

    pub fn intersect(&self, other: &Aabb) -> Self {
        Self::new(self.min.max(other.min), self.max.min(other.max))
    }

    pub fn union_point(&self, p: Vec3) -> Self {
        Self::new(self.min.min(p), self.max.max(p))
    }

    /// An "empty" box that any point expands.
    pub fn inverted() -> Self {
        Self::new(Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY))
    }
}

/// Poly6 and Spiky smoothing kernels (Müller et al. 2003) for radius `h`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sph {
    pub h: f32,
    pub h2: f32,
    pub poly6: f32,
    pub spiky: f32,
}

impl Sph {
    pub fn new(h: f32) -> Self {
        let hd = h as f64;
        Self {
            h,
            h2: h * h,
            poly6: (315.0 / (64.0 * std::f64::consts::PI * hd.powi(9))) as f32,
            spiky: (45.0 / (std::f64::consts::PI * hd.powi(6))) as f32,
        }
    }

    /// `poly6 · (h² − r²)³`, zero beyond `h`.
    #[inline(always)]
    pub fn poly6_r2(&self, r2: f32) -> f32 {
        let d = (self.h2 - r2).max(0.0);
        d * d * d * self.poly6
    }

    /// Gradient of the Spiky kernel with respect to `r`:
    /// `−spiky · (h − |r|)² · r̂`; zero beyond `h` and at `r = 0`.
    #[inline(always)]
    pub fn spiky_grad(&self, r: Vec3A) -> Vec3A {
        let len = r.length();
        let t = (self.h - len).max(0.0);
        if len <= 1e-20 {
            return Vec3A::ZERO;
        }
        r * (-(self.spiky * t * t) / len)
    }
}

/// Row-major 3×3 matrix used by the shape-matching code, matching the
/// reference implementation's `m{row}{col}` naming.
pub(crate) type M3 = [[f32; 3]; 3];

pub(crate) const M3_IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Rotation `R` of the polar decomposition `A = R·S`, by Jacobi
/// diagonalization of `AᵀA` (8 sweeps of the (0,1), (0,2), (1,2) rotations),
/// `S⁻¹ = Q diag(1/√λ) Qᵀ`, `R = A·S⁻¹`. Port of `polarDecomp.ts`.
pub(crate) fn polar_rotation(a: &M3) -> M3 {
    let mut m00 = a[0][0] * a[0][0] + a[1][0] * a[1][0] + a[2][0] * a[2][0];
    let mut m01 = a[0][0] * a[0][1] + a[1][0] * a[1][1] + a[2][0] * a[2][1];
    let mut m02 = a[0][0] * a[0][2] + a[1][0] * a[1][2] + a[2][0] * a[2][2];
    let mut m11 = a[0][1] * a[0][1] + a[1][1] * a[1][1] + a[2][1] * a[2][1];
    let mut m12 = a[0][1] * a[0][2] + a[1][1] * a[1][2] + a[2][1] * a[2][2];
    let mut m22 = a[0][2] * a[0][2] + a[1][2] * a[1][2] + a[2][2] * a[2][2];
    let mut q = M3_IDENTITY;
    const EPS: f32 = 1e-12;
    #[inline(always)]
    fn givens(apq: f32, app: f32, aqq: f32) -> (f32, f32, f32) {
        let tau = (aqq - app) / (apq * 2.0);
        let sign = if tau >= 0.0 { 1.0 } else { -1.0 };
        let ta = tau.abs();
        let t = sign / (ta + (ta * ta + 1.0).sqrt());
        let c = 1.0 / (t * t + 1.0).sqrt();
        (t, c, t * c)
    }
    for _ in 0..8 {
        if m01.abs() > EPS {
            let (t, c, s) = givens(m01, m00, m11);
            let (o02, o12) = (m02, m12);
            m00 -= t * m01;
            m11 += t * m01;
            m01 = 0.0;
            m02 = c * o02 - s * o12;
            m12 = s * o02 + c * o12;
            for row in q.iter_mut() {
                let (a0, a1) = (row[0], row[1]);
                row[0] = c * a0 - s * a1;
                row[1] = s * a0 + c * a1;
            }
        }
        if m02.abs() > EPS {
            let (t, c, s) = givens(m02, m00, m22);
            let (o01, o12) = (m01, m12);
            m00 -= t * m02;
            m22 += t * m02;
            m02 = 0.0;
            m01 = c * o01 - s * o12;
            m12 = s * o01 + c * o12;
            for row in q.iter_mut() {
                let (a0, a2) = (row[0], row[2]);
                row[0] = c * a0 - s * a2;
                row[2] = s * a0 + c * a2;
            }
        }
        if m12.abs() > EPS {
            let (t, c, s) = givens(m12, m11, m22);
            let (o01, o02) = (m01, m02);
            m11 -= t * m12;
            m22 += t * m12;
            m12 = 0.0;
            m01 = c * o01 - s * o02;
            m02 = s * o01 + c * o02;
            for row in q.iter_mut() {
                let (a1, a2) = (row[1], row[2]);
                row[1] = c * a1 - s * a2;
                row[2] = s * a1 + c * a2;
            }
        }
    }
    let inv = [
        1.0 / m00.max(1e-20).sqrt(),
        1.0 / m11.max(1e-20).sqrt(),
        1.0 / m22.max(1e-20).sqrt(),
    ];
    let mut si = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            si[i][j] = q[i][0] * q[j][0] * inv[0]
                + q[i][1] * q[j][1] * inv[1]
                + q[i][2] * q[j][2] * inv[2];
        }
    }
    let mut r = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * si[0][j] + a[i][1] * si[1][j] + a[i][2] * si[2][j];
        }
    }
    r
}

/// `R·v` for a row-major matrix.
#[inline(always)]
pub(crate) fn m3_mul_vec(m: &M3, v: Vec3A) -> Vec3A {
    Vec3A::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

/// Quaternion from a rotation matrix (Shoemake's branched form, as in
/// `shapeMatchImplicit.ts` and `skinMaterial.ts`).
pub(crate) fn quat_from_m3(m: &M3) -> Quat {
    let (m00, m01, m02) = (m[0][0], m[0][1], m[0][2]);
    let (m10, m11, m12) = (m[1][0], m[1][1], m[1][2]);
    let (m20, m21, m22) = (m[2][0], m[2][1], m[2][2]);
    let trace = m00 + m11 + m22;
    let (x, y, z, w);
    if trace > 0.0 {
        let s = 0.5 / (trace + 1.0).sqrt();
        w = 0.25 / s;
        x = (m21 - m12) * s;
        y = (m02 - m20) * s;
        z = (m10 - m01) * s;
    } else if m00 > m11 && m00 > m22 {
        let s = 2.0 * (1.0 + m00 - m11 - m22).sqrt();
        w = (m21 - m12) / s;
        x = 0.25 * s;
        y = (m01 + m10) / s;
        z = (m02 + m20) / s;
    } else if m11 > m22 {
        let s = 2.0 * (1.0 + m11 - m00 - m22).sqrt();
        w = (m02 - m20) / s;
        x = (m01 + m10) / s;
        y = 0.25 * s;
        z = (m12 + m21) / s;
    } else {
        let s = 2.0 * (1.0 + m22 - m00 - m11).sqrt();
        w = (m10 - m01) / s;
        x = (m02 + m20) / s;
        y = (m12 + m21) / s;
        z = 0.25 * s;
    }
    Quat::from_xyzw(x, y, z, w)
}

/// Rotation matrix (row-major) of a unit quaternion.
pub(crate) fn m3_from_quat(q: Quat) -> M3 {
    let (x, y, z, w) = (q.x, q.y, q.z, q.w);
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (w * x, w * y, w * z);
    [
        [1.0 - 2.0 * (yy + zz), 2.0 * (xy - wz), 2.0 * (xz + wy)],
        [2.0 * (xy + wz), 1.0 - 2.0 * (xx + zz), 2.0 * (yz - wx)],
        [2.0 * (xz - wy), 2.0 * (yz + wx), 1.0 - 2.0 * (xx + yy)],
    ]
}

/// Spread the lower 10 bits of `n` over every third bit (Morton "part1by2").
#[inline(always)]
pub(crate) fn part1by2(n: u32) -> u32 {
    let mut m = n & 0x0000_03ff;
    m = (m | (m << 16)) & 0xff00_00ff;
    m = (m | (m << 8)) & 0x0300_f00f;
    m = (m | (m << 4)) & 0x030c_30c3;
    m = (m | (m << 2)) & 0x0924_9249;
    m
}

/// Seeded random numbers (SplitMix64), so every run with the same seed is identical.
#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Rotate `v` by unit quaternion `q` (Vec3A flavour).
#[inline(always)]
pub(crate) fn quat_rotate(q: Quat, v: Vec3A) -> Vec3A {
    let qv = Vec3A::new(q.x, q.y, q.z);
    let t = 2.0 * qv.cross(v);
    v + t * q.w + qv.cross(t)
}

/// Unit quaternion for a rotation by `|ω|·t` about `ω`.
#[inline]
pub(crate) fn quat_from_spin(spin: Vec3A, t: f32) -> Quat {
    let rate = spin.length();
    if rate * t == 0.0 {
        return Quat::IDENTITY;
    }
    let half = rate * t * 0.5;
    let axis = spin / rate.max(1e-8);
    let s = half.sin();
    Quat::from_xyzw(axis.x * s, axis.y * s, axis.z * s, half.cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polar_of_rotation_is_itself() {
        let q = Quat::from_euler(glam::EulerRot::XYZ, 0.3, -0.7, 1.1);
        let r = m3_from_quat(q);
        let scaled: M3 = [
            [r[0][0] * 2.0, r[0][1] * 1.5, r[0][2] * 0.5],
            [r[1][0] * 2.0, r[1][1] * 1.5, r[1][2] * 0.5],
            [r[2][0] * 2.0, r[2][1] * 1.5, r[2][2] * 0.5],
        ];
        let out = polar_rotation(&scaled);
        for i in 0..3 {
            for j in 0..3 {
                assert!((out[i][j] - r[i][j]).abs() < 1e-4, "{out:?} vs {r:?}");
            }
        }
        let back = quat_from_m3(&out);
        assert!(back.dot(q).abs() > 0.9999);
    }

    #[test]
    fn spiky_gradient_points_inward() {
        let sph = Sph::new(0.1);
        let g = sph.spiky_grad(Vec3A::new(0.05, 0.0, 0.0));
        assert!(g.x < 0.0);
        assert_eq!(sph.spiky_grad(Vec3A::ZERO), Vec3A::ZERO);
        assert_eq!(sph.poly6_r2(0.02), 0.0);
    }
}
