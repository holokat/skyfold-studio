//! Smoke density and light volume (CPU port of `gas/render/GasVolumeRenderer.ts`).
//!
//! 1. **Splat**: every live tracer adds a cloud-in-cell weight to the 8 nodes
//!    of a node-centred grid (`p = (x − min)/extent · (n − 1)`), weighted by
//!    `max(1 − age/lifetime, 0) · (0.4 + 1.2 · grain)` with a fixed random grain
//!    per tracer, so the smoke's texture moves with the smoke. Tracers in the
//!    outermost node layer are skipped.
//! 2. **Blur**: three separable 7-tap passes, weights
//!    `[0.006, 0.0606, 0.2417, 0.3829, 0.2417, 0.0606, 0.006]`, clamped at the edges.
//! 3. **Resolve**: at each voxel centre, the trilinear blurred value times
//!    `extinction = 0.00015 / voxel volume` times `density` times a smooth fade
//!    near the walls gives an extinction coefficient, 1/m.
//! 4. **Light**: six taps toward the light at 0.03, 0.07, 0.13, 0.22, 0.36,
//!    0.58 m accumulate optical depth `τ`; the stored light is
//!    `0.7 e^(−2.5τ) + 0.3 e^(−0.35τ)`.

use super::splat_two_slices;
use crate::gas::Tracers;
use crate::math::Aabb;
use glam::{UVec3, Vec3};
use rayon::prelude::*;

/// How the smoke volume is built. Defaults match the reference renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct SmokeVolumeOptions {
    /// Box the volume covers. `None`: the container, with its top tenth cut off
    /// (where tracers retire).
    pub bounds: Option<Aabb>,
    /// Voxels per axis, 4–128. `None`: 64 along the longest axis, proportional elsewhere.
    pub resolution: Option<[u32; 3]>,
    /// Opacity multiplier. Default 0.7 (the reference `Smoke.opacity`).
    pub density: f32,
    /// Direction toward the light. Default `(−0.35, 0.8, 0.4)`.
    pub light_direction: Vec3,
    /// Fade width near the four side walls, the floor and the ceiling, as a
    /// fraction of the box. Default `(0.07, 0.025, 0.24)`.
    pub edge_fade: [f32; 3],
}

impl Default for SmokeVolumeOptions {
    fn default() -> Self {
        Self {
            bounds: None,
            resolution: None,
            density: 0.7,
            light_direction: Vec3::new(-0.35, 0.8, 0.4),
            edge_fade: [0.07, 0.025, 0.24],
        }
    }
}

/// A smoke volume ready for a 3D texture: voxel `(x, y, z)` at
/// `x + y·nx + z·nx·ny`, voxel centres at `min + (i + ½)·extent/n`.
#[derive(Clone, Debug, Default)]
pub struct SmokeVolume {
    pub min: Vec3,
    pub max: Vec3,
    pub resolution: [u32; 3],
    /// Extinction coefficient per voxel, 1/m (the R channel of the reference texture).
    pub density: Vec<f32>,
    /// Fraction of light reaching each voxel, 0–1 (the G channel).
    pub light: Vec<f32>,
}

const BLUR: [f32; 7] = [0.006, 0.0606, 0.2417, 0.3829, 0.2417, 0.0606, 0.006];
const TAPS: [f32; 6] = [0.03, 0.07, 0.13, 0.22, 0.36, 0.58];

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Build the volume from live tracers.
pub(crate) fn build(
    tracers: &Tracers,
    lifetime: f32,
    bounds: Aabb,
    opts: &SmokeVolumeOptions,
    out: &mut SmokeVolume,
) {
    let size = bounds.size();
    let res = opts.resolution.unwrap_or_else(|| {
        let scale = 64.0 / size.max_element();
        [size.x, size.y, size.z].map(|s| ((s * scale).round() as u32).clamp(8, 128))
    });
    let [nx, ny, nz] = res.map(|r| r.clamp(4, 128) as usize);
    let count = nx * ny * nz;
    let slice = nx * ny;
    let scale = Vec3::new((nx - 1) as f32, (ny - 1) as f32, (nz - 1) as f32);
    // 1. Splat.
    let mut nodes = vec![0.0f32; count];
    let mut items = Vec::with_capacity(tracers.len());
    for s in 0..tracers.len() {
        let p = (Vec3::from(tracers.pos[s]) - bounds.min) / size * scale;
        if p.cmpge(Vec3::ONE).all() && p.cmplt(scale - Vec3::ONE).all() {
            items.push((p.z.floor() as u32, s as u32));
        }
    }
    splat_two_slices(&mut nodes, slice, nz, &items, |s, lo, hi| {
        let s = s as usize;
        let p = (Vec3::from(tracers.pos[s]) - bounds.min) / size * scale;
        let base = p.floor();
        let f = p - base;
        let weight = tracers.seed[s] * 1.2 + 0.4;
        let fade = (1.0 - tracers.age[s] / lifetime).max(0.0) * weight;
        let (bx, by) = (base.x as usize, base.y as usize);
        for (zs, wz) in [(&mut *lo, 1.0 - f.z), (&mut *hi, f.z)] {
            for (dy, wy) in [(0, 1.0 - f.y), (1, f.y)] {
                for (dx, wx) in [(0, 1.0 - f.x), (1, f.x)] {
                    zs[(by + dy) * nx + bx + dx] += wx * wy * wz * fade;
                }
            }
        }
    });
    // 2. Blur along x, y, z.
    let mut tmp = vec![0.0f32; count];
    let dims = [nx, ny, nz];
    for axis in 0..3 {
        let (src, dst) = if axis % 2 == 0 {
            (&nodes, &mut tmp)
        } else {
            (&tmp, &mut nodes)
        };
        dst.par_chunks_mut(slice).enumerate().for_each(|(z, out)| {
            for y in 0..ny {
                for x in 0..nx {
                    let c = [x, y, z];
                    let mut sum = 0.0;
                    for (k, w) in BLUR.iter().enumerate() {
                        let mut n = c;
                        n[axis] = (c[axis] as i32 + k as i32 - 3).clamp(0, dims[axis] as i32 - 1)
                            as usize;
                        sum += src[n[2] * slice + n[1] * nx + n[0]] * w;
                    }
                    out[y * nx + x] = sum;
                }
            }
        });
    }
    let blurred = &tmp; // three passes: nodes → tmp → nodes → tmp
    // 3. Resolve to voxel centres.
    let extinction = 0.00015 / (size.x * size.y * size.z / count as f32);
    let [sides, bottom, top] = opts.edge_fade;
    let ramp = |d: f32, w: f32| if w > 0.0 { smoothstep(0.0, w, d) } else { 1.0 };
    let sample_nodes = |unit: Vec3| -> f32 {
        let p = (unit * scale).clamp(Vec3::ZERO, scale - Vec3::splat(0.001));
        let c = p.floor();
        let f = p - c;
        let (cx, cy, cz) = (c.x as usize, c.y as usize, c.z as usize);
        let mut sum = 0.0;
        for (dz, wz) in [(0, 1.0 - f.z), (1, f.z)] {
            for (dy, wy) in [(0, 1.0 - f.y), (1, f.y)] {
                for (dx, wx) in [(0, 1.0 - f.x), (1, f.x)] {
                    let (x, y, z) = (
                        (cx + dx).min(nx - 1),
                        (cy + dy).min(ny - 1),
                        (cz + dz).min(nz - 1),
                    );
                    sum += blurred[z * slice + y * nx + x] * wx * wy * wz;
                }
            }
        }
        sum
    };
    out.min = bounds.min;
    out.max = bounds.max;
    out.resolution = [nx as u32, ny as u32, nz as u32];
    out.density.resize(count, 0.0);
    out.light.resize(count, 0.0);
    let n_f = Vec3::new(nx as f32, ny as f32, nz as f32);
    out.density
        .par_chunks_mut(slice)
        .enumerate()
        .for_each(|(z, out)| {
            for y in 0..ny {
                for x in 0..nx {
                    let unit = (Vec3::new(x as f32, y as f32, z as f32) + Vec3::splat(0.5)) / n_f;
                    let edge = unit.min(Vec3::ONE - unit);
                    let fade = ramp(edge.x.min(edge.z), sides)
                        * ramp(unit.y, bottom)
                        * ramp(1.0 - unit.y, top);
                    out[y * nx + x] = sample_nodes(unit) * extinction * opts.density * fade;
                }
            }
        });
    // 4. Light: march toward the light through the (cell-centred, clamped) density.
    let sun = opts.light_direction.normalize_or(Vec3::Y);
    let density = &out.density;
    let dim = UVec3::new(nx as u32, ny as u32, nz as u32);
    let sample_density =
        |world: Vec3| -> f32 { sample_cells(density, dim, (world - bounds.min) / size) };
    out.light
        .par_chunks_mut(slice)
        .enumerate()
        .for_each(|(z, out)| {
            for y in 0..ny {
                for x in 0..nx {
                    let p = bounds.min
                        + (Vec3::new(x as f32, y as f32, z as f32) + Vec3::splat(0.5)) / n_f * size;
                    let mut shadow = 0.0;
                    let mut prev = 0.0;
                    for &d in &TAPS {
                        shadow += sample_density(p + sun * d) * (d - prev);
                        prev = d;
                    }
                    out[y * nx + x] = 0.7 * (-2.5 * shadow).exp() + 0.3 * (-0.35 * shadow).exp();
                }
            }
        });
}

/// Hardware-style trilinear sample of a cell-centred grid at normalized `uvw`,
/// clamped to the edge voxels.
pub(crate) fn sample_cells(data: &[f32], dim: UVec3, uvw: Vec3) -> f32 {
    let n = dim.as_vec3();
    let g = (uvw * n - Vec3::splat(0.5)).clamp(Vec3::ZERO, n - Vec3::ONE);
    let c = g.floor();
    let f = g - c;
    let (nx, ny) = (dim.x as usize, dim.y as usize);
    let (cx, cy, cz) = (c.x as usize, c.y as usize, c.z as usize);
    let (x1, y1, z1) = (
        (cx + 1).min(nx - 1),
        (cy + 1).min(ny - 1),
        (cz + 1).min(dim.z as usize - 1),
    );
    let at = |x: usize, y: usize, z: usize| data[z * nx * ny + y * nx + x];
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c00 = lerp(at(cx, cy, cz), at(x1, cy, cz), f.x);
    let c10 = lerp(at(cx, y1, cz), at(x1, y1, cz), f.x);
    let c01 = lerp(at(cx, cy, z1), at(x1, cy, z1), f.x);
    let c11 = lerp(at(cx, y1, z1), at(x1, y1, z1), f.x);
    lerp(lerp(c00, c10, f.y), lerp(c01, c11, f.y), f.z)
}
