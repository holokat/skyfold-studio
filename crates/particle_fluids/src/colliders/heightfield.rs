//! Terrain collider: a regular grid of heights (not in the reference library).
//!
//! Heights are sampled bilinearly; the gradient of the bilinear patch gives
//! the surface normal. A particle's signed distance is its height above the
//! terrain projected on that normal (`(y − h) · n_y`, the distance to the
//! local tangent plane), so particles below the surface are pushed back out
//! along the normal with the same contact and friction model as every other
//! collider. Outside the grid the edge heights extend outward.

use glam::{Vec3, Vec3A};

/// A height grid in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct Heightfield {
    /// World position of sample `(0, 0)`. Heights are added to `origin.y`.
    pub origin: Vec3,
    /// Spacing between samples along x and z, m.
    pub cell: f32,
    /// Samples along x.
    pub res_x: usize,
    /// Samples along z.
    pub res_z: usize,
    /// Row-major heights: sample `(ix, iz)` is `heights[iz * res_x + ix]`, m.
    pub heights: Vec<f32>,
    /// Static friction coefficient. Default 0.5.
    pub mu_s: f32,
    /// Kinetic friction coefficient. Default 0.4.
    pub mu_k: f32,
}

impl Heightfield {
    /// A height grid with default friction. Panics if `heights` doesn't hold
    /// `res_x · res_z` values or a resolution is below 2.
    pub fn new(origin: Vec3, cell: f32, res_x: usize, res_z: usize, heights: Vec<f32>) -> Self {
        assert!(
            res_x >= 2 && res_z >= 2,
            "Heightfield: resolution must be at least 2×2"
        );
        assert_eq!(
            heights.len(),
            res_x * res_z,
            "Heightfield: heights must hold res_x × res_z values"
        );
        assert!(
            cell > 0.0 && cell.is_finite(),
            "Heightfield: cell must be positive"
        );
        Self {
            origin,
            cell,
            res_x,
            res_z,
            heights,
            mu_s: 0.5,
            mu_k: 0.4,
        }
    }

    /// A heightfield from a function of world `(x, z)`.
    pub fn from_fn(
        origin: Vec3,
        cell: f32,
        res_x: usize,
        res_z: usize,
        f: impl Fn(f32, f32) -> f32,
    ) -> Self {
        let mut heights = Vec::with_capacity(res_x * res_z);
        for iz in 0..res_z {
            for ix in 0..res_x {
                heights.push(f(origin.x + ix as f32 * cell, origin.z + iz as f32 * cell));
            }
        }
        Self::new(origin, cell, res_x, res_z, heights)
    }

    /// World height at `(x, z)` and the slopes `∂h/∂x`, `∂h/∂z`.
    #[inline]
    pub fn height_and_slope(&self, x: f32, z: f32) -> (f32, f32, f32) {
        let gx = ((x - self.origin.x) / self.cell).clamp(0.0, (self.res_x - 1) as f32);
        let gz = ((z - self.origin.z) / self.cell).clamp(0.0, (self.res_z - 1) as f32);
        let ix = (gx.floor() as usize).min(self.res_x - 2);
        let iz = (gz.floor() as usize).min(self.res_z - 2);
        let tx = gx - ix as f32;
        let tz = gz - iz as f32;
        let row = iz * self.res_x + ix;
        let h00 = self.heights[row];
        let h10 = self.heights[row + 1];
        let h01 = self.heights[row + self.res_x];
        let h11 = self.heights[row + self.res_x + 1];
        let h0 = h00 + (h10 - h00) * tx;
        let h1 = h01 + (h11 - h01) * tx;
        let h = h0 + (h1 - h0) * tz;
        let dx = ((h10 - h00) + ((h11 - h01) - (h10 - h00)) * tz) / self.cell;
        let dz = (h1 - h0) / self.cell;
        (self.origin.y + h, dx, dz)
    }

    /// World height at `(x, z)`.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        self.height_and_slope(x, z).0
    }

    /// Upward unit normal at `(x, z)`.
    pub fn normal(&self, x: f32, z: f32) -> Vec3 {
        let (_, dx, dz) = self.height_and_slope(x, z);
        Vec3::new(-dx, 1.0, -dz).normalize()
    }

    /// Signed distance of `p` above the terrain's tangent plane, and the normal.
    #[inline]
    pub fn sdf(&self, p: Vec3A) -> (f32, Vec3A) {
        let (h, dx, dz) = self.height_and_slope(p.x, p.z);
        let n = Vec3A::new(-dx, 1.0, -dz).normalize();
        ((p.y - h) * n.y, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_slope() {
        let hf = Heightfield::from_fn(Vec3::new(-1.0, 0.5, -1.0), 0.25, 9, 9, |x, _| 0.5 * x);
        let (h, dx, dz) = hf.height_and_slope(0.1, 0.3);
        assert!((h - (0.5 + 0.05)).abs() < 1e-5);
        assert!((dx - 0.5).abs() < 1e-5 && dz.abs() < 1e-6);
        let (phi, n) = hf.sdf(Vec3A::new(0.0, 1.5, 0.0));
        assert!(n.x < 0.0 && (phi - 1.0 / (1.25f32).sqrt()).abs() < 1e-4);
    }
}
