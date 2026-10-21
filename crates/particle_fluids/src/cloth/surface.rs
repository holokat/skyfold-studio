//! Smooth render surface for grid cloth (CPU port of `cloth/surface.ts`).
//!
//! Each render vertex sits on a bicubic surface over the particle grid: a
//! uniform cubic B-spline near the particles (`smooth`, the default, which
//! filters out grid-scale buckling) or a Catmull-Rom spline through them.
//! Normals come from the analytic tangents, `normalize(∂p/∂v × ∂p/∂u)`, and
//! face the cloth's front (+z of the cloth as added).

use glam::{Vec3, Vec3A};
use rayon::prelude::*;

/// Render data for one cloth: a `PlaneGeometry`-ordered grid of
/// `((columns − 1)·subdivisions + 1) × ((rows − 1)·subdivisions + 1)` vertices.
/// The topology (`uvs`, `indices`) is filled once; positions, normals and
/// wetness are rewritten by every fill.
#[derive(Clone, Debug, Default)]
pub struct ClothSurface {
    /// Render vertices per grid cell along each axis. Default 3.
    pub subdivisions: u32,
    /// B-spline (true, default) or Catmull-Rom (false).
    pub smooth: bool,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// `(u, v)` with v = 1 at the top row, like three.js.
    pub uvs: Vec<[f32; 2]>,
    /// Counter-clockwise triangles seen from the front.
    pub indices: Vec<u32>,
    /// Interpolated cloth wetness, 0 (dry) to 1 (soaked).
    pub wetness: Vec<f32>,
    /// Render vertices per row and number of rows.
    pub grid: [u32; 2],
}

impl ClothSurface {
    pub fn new(subdivisions: u32, smooth: bool) -> Self {
        Self {
            subdivisions: subdivisions.max(1),
            smooth,
            ..Default::default()
        }
    }
}

#[inline(always)]
fn bspline(t: f32) -> ([f32; 4], [f32; 4]) {
    let t2 = t * t;
    let t3 = t2 * t;
    let s = 1.0 - t;
    (
        [
            s * s * s / 6.0,
            (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
            (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
            t3 / 6.0,
        ],
        [
            -0.5 * s * s,
            1.5 * t2 - 2.0 * t,
            -1.5 * t2 + t + 0.5,
            0.5 * t2,
        ],
    )
}

#[inline(always)]
fn catmull_rom(t: f32) -> ([f32; 4], [f32; 4]) {
    let t2 = t * t;
    let t3 = t2 * t;
    (
        [
            -0.5 * t + t2 - 0.5 * t3,
            -2.5 * t2 + 1.5 * t3 + 1.0,
            0.5 * t + 2.0 * t2 - 1.5 * t3,
            0.5 * (t3 - t2),
        ],
        [
            2.0 * t - 1.5 * t2 - 0.5,
            -5.0 * t + 4.5 * t2,
            4.0 * t - 4.5 * t2 + 0.5,
            1.5 * t2 - t,
        ],
    )
}

/// Fill `out` from the cloth's particle positions (row-major `columns × rows`)
/// and per-particle wetness.
pub(crate) fn fill_surface(
    particles: &[Vec3A],
    wetness: &[f32],
    columns: u32,
    rows: u32,
    out: &mut ClothSurface,
) {
    let sub = out.subdivisions.max(1);
    let gx = (columns - 1) * sub;
    let gy = (rows - 1) * sub;
    let count = ((gx + 1) * (gy + 1)) as usize;
    if out.grid != [gx + 1, gy + 1] || out.indices.is_empty() {
        out.grid = [gx + 1, gy + 1];
        out.uvs = (0..=gy)
            .flat_map(|iy| {
                (0..=gx).map(move |ix| [ix as f32 / gx as f32, 1.0 - iy as f32 / gy as f32])
            })
            .collect();
        out.indices = super::graph::plane_indices(gx, gy);
        out.positions = vec![Vec3::ZERO; count];
        out.normals = vec![Vec3::Z; count];
        out.wetness = vec![0.0; count];
    }
    let basis = if out.smooth { bspline } else { catmull_rom };
    let (cols, rws) = (columns as i32, rows as i32);
    let row_len = (gx + 1) as usize;
    let fill_row = |iy: usize, pos: &mut [Vec3], nrm: &mut [Vec3], wet: &mut [f32]| {
        let y = (iy as f32 / gy as f32 * (rows - 1) as f32).min(rows as f32 - 1.0001);
        let iyf = y.floor() as i32;
        let (wy, dy) = basis(y - iyf as f32);
        for ix in 0..row_len {
            let x = (ix as f32 / gx as f32 * (columns - 1) as f32).min(columns as f32 - 1.0001);
            let ixf = x.floor() as i32;
            let (wx, dx) = basis(x - ixf as f32);
            let mut p = Vec3A::ZERO;
            let mut tu = Vec3A::ZERO;
            let mut tv = Vec3A::ZERO;
            let mut w = 0.0;
            for v in 0..4 {
                let r = (iyf + v as i32 - 1).clamp(0, rws - 1);
                for u in 0..4 {
                    let c = (ixf + u as i32 - 1).clamp(0, cols - 1);
                    let k = (r * cols + c) as usize;
                    let s = particles[k];
                    p += s * (wx[u] * wy[v]);
                    tu += s * (dx[u] * wy[v]);
                    tv += s * (wx[u] * dy[v]);
                    w += wetness[k] * wx[u] * wy[v];
                }
            }
            pos[ix] = p.into();
            nrm[ix] = tv.cross(tu).normalize_or(Vec3A::Z).into();
            wet[ix] = w.clamp(0.0, 1.0);
        }
    };
    out.positions
        .par_chunks_mut(row_len)
        .zip(out.normals.par_chunks_mut(row_len))
        .zip(out.wetness.par_chunks_mut(row_len))
        .enumerate()
        .for_each(|(iy, ((p, n), w))| fill_row(iy, p, n, w));
}
