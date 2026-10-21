//! Liquid surface field (CPU port of `fluids/render/SurfaceField.ts`).
//!
//! 1. **Splat**: each liquid particle (and, with motion stretch, two more
//!    samples trailing back along its velocity) adds cloud-in-cell weights to
//!    the 8 voxels around it: the weight, and the weight times the particle's
//!    offset from each voxel centre. Solid particles (cloth, soft bodies)
//!    splat a separate weight used for wetting.
//! 2. **Blur**: a separable Gaussian (σ = 0.75 particle spacings, 2–6 taps
//!    each side) in three passes. Offsets are re-based onto each destination
//!    voxel as they move, so the result is the exact kernel-weighted mean
//!    particle position `x̄` around every voxel. Empty space is skipped in
//!    bricks of 8³ voxels.
//! 3. **Field**: `φ = |p − x̄| − R` in voxels (Zhu & Bridson 2005), `R = 1.25 r`,
//!    capped at [`FIELD_BAND`]; near wetted walls φ is lowered by up to the
//!    meniscus height so the liquid climbs them, and colliders are carved out.
//!
//! The output matches the reference texture: R = φ (voxels, surface at 0,
//! negative inside), G = liquid volume fraction (≤ 4), B = wall proximity.

use super::splat_two_slices;
use crate::colliders::ColliderSet;
use crate::math::Aabb;
use glam::{Vec3, Vec3A, Vec4};
use rayon::prelude::*;

/// Field value (voxels) stored where no particle is within the filter footprint.
pub const FIELD_BAND: f32 = 6.0;
const BRICK: usize = 8;

/// How the liquid surface field is built.
#[derive(Clone, Debug, PartialEq)]
pub struct LiquidSurfaceOptions {
    /// World box the liquid can occupy. `None`: the container expanded by
    /// (0.03, 0.02, 0.03) m, as the reference `Simulation`.
    pub bounds: Option<Aabb>,
    /// Upper bound on voxels (the voxel is never smaller than the particle
    /// radius). Default 250 000 (the reference GPU default is 600 000).
    pub voxel_budget: usize,
    /// Stretch each particle back along its velocity by this many seconds of
    /// travel, up to 3 spacings. Default 0.02 (the reference `Simulation`).
    pub motion_stretch: f32,
    /// Surface radius around an isolated particle, in particle radii. Default 1.25.
    pub radius: f32,
    /// How high the liquid climbs wetted walls, in particle radii. Default 1.6.
    pub meniscus_height: f32,
    /// Horizontal reach of the meniscus, in particle radii. Default 2.5.
    pub meniscus_width: f32,
    /// Let cloth and soft-body particles wet the surface. Default true.
    pub wet_solids: bool,
    /// Let colliders wet the surface and carve it. Default true.
    pub wet_colliders: bool,
}

impl Default for LiquidSurfaceOptions {
    fn default() -> Self {
        Self {
            bounds: None,
            voxel_budget: 250_000,
            motion_stretch: 0.02,
            radius: 1.25,
            meniscus_height: 1.6,
            meniscus_width: 2.5,
            wet_solids: true,
            wet_colliders: true,
        }
    }
}

/// A liquid surface field ready for an RGBA 3D texture: voxel `(x, y, z)` at
/// `x + y·nx + z·nx·ny`, centred at `origin + (i + ½)·voxel`.
#[derive(Clone, Debug, Default)]
pub struct LiquidSurfaceField {
    pub origin: Vec3,
    /// Voxel edge length, m. Multiply φ by it to get metres.
    pub voxel: f32,
    pub resolution: [u32; 3],
    /// `(φ in voxels, volume fraction, wall proximity, 1)` per voxel.
    pub data: Vec<[f32; 4]>,
    /// Iso value of the surface (always 0; φ < 0 is liquid).
    pub iso: f32,
    /// World size of the grid (`resolution · voxel`).
    pub extent: Vec3,
    /// Brick occupancy (8³ voxels each, x fastest), for empty-space skipping.
    pub bricks: Vec<bool>,
    pub brick_resolution: [u32; 3],
    walls: Vec<[f32; 2]>,
    walls_version: Option<u64>,
}

pub(crate) struct SurfaceInputs<'a> {
    pub positions: &'a [Vec3A],
    pub velocities: &'a [Vec3A],
    pub liquid: &'a [u32],
    pub solids: &'a [u32],
    pub radius: f32,
    pub spacing: f32,
    pub colliders: &'a ColliderSet,
}

/// Rebuild `out` for `bounds`.
pub(crate) fn build(
    input: &SurfaceInputs<'_>,
    bounds: Aabb,
    opts: &LiquidSurfaceOptions,
    out: &mut LiquidSurfaceField,
) {
    let r = input.radius;
    let spacing = input.spacing;
    let size = bounds.size();
    let voxel = r.max((size.x * size.y * size.z / opts.voxel_budget.max(1) as f32).cbrt());
    let dims = (size / voxel).ceil();
    let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
    let count = nx * ny * nz;
    let slice = nx * ny;
    let resized = out.resolution != [nx as u32, ny as u32, nz as u32]
        || out.origin != bounds.min
        || out.voxel != voxel;
    out.origin = bounds.min;
    out.voxel = voxel;
    out.resolution = [nx as u32, ny as u32, nz as u32];
    out.extent = dims * voxel;
    out.iso = 0.0;
    let inv_voxel = 1.0 / voxel;
    let origin = Vec3A::from(bounds.min);
    let grid_coord = |p: Vec3A| (p - origin) * inv_voxel - Vec3A::splat(0.5);
    let upper = Vec3A::new(nx as f32 - 1.0, ny as f32 - 1.0, nz as f32 - 1.0);
    // Splat items: (slice, sample id). Fluid samples first, then solids.
    let samples = if opts.motion_stretch > 0.0 { 3 } else { 1 };
    let trail_max = 3.0 * spacing;
    let sample_pos = |k: usize| -> Vec3A {
        let i = input.liquid[k / samples] as usize;
        let s = k % samples;
        let x = input.positions[i];
        if s == 0 {
            return x;
        }
        let v = input.velocities[i] * opts.motion_stretch;
        let trail = v * (trail_max / v.length().max(trail_max));
        x - trail * (s as f32 / (samples - 1) as f32)
    };
    let n_fluid = input.liquid.len() * samples;
    let solids: &[u32] = if opts.wet_solids { input.solids } else { &[] };
    let position_of = |item: usize| -> Vec3A {
        if item < n_fluid {
            sample_pos(item)
        } else {
            input.positions[solids[item - n_fluid] as usize]
        }
    };
    let in_grid = |g: Vec3A| g.cmpge(Vec3A::ZERO).all() && g.cmplt(upper).all();
    let items: Vec<(u32, u32)> = (0..n_fluid + solids.len())
        .filter_map(|item| {
            let g = grid_coord(position_of(item));
            in_grid(g).then(|| (g.z.floor() as u32, item as u32))
        })
        .collect();
    // Bricks touched by the splat, then dilated by one brick.
    let (bx, by, bz) = (nx.div_ceil(BRICK), ny.div_ceil(BRICK), nz.div_ceil(BRICK));
    let mut marked = vec![false; bx * by * bz];
    for &(_, item) in &items {
        let g = grid_coord(position_of(item as usize)).floor();
        let (x, y, z) = (
            g.x as usize / BRICK,
            g.y as usize / BRICK,
            g.z as usize / BRICK,
        );
        marked[(z * by + y) * bx + x] = true;
    }
    let mut active = vec![false; marked.len()];
    for z in 0..bz {
        for y in 0..by {
            for x in 0..bx {
                let mut any = false;
                for dz in z.saturating_sub(1)..=(z + 1).min(bz - 1) {
                    for dy in y.saturating_sub(1)..=(y + 1).min(by - 1) {
                        for dx in x.saturating_sub(1)..=(x + 1).min(bx - 1) {
                            any |= marked[(dz * by + dy) * bx + dx];
                        }
                    }
                }
                active[(z * by + y) * bx + x] = any;
            }
        }
    }
    let brick_active =
        |x: usize, y: usize, z: usize| active[((z / BRICK) * by + y / BRICK) * bx + x / BRICK];
    // 1. Splat: (mass, mass · offset) per fluid sample; mass per solid particle.
    let inv_samples = 1.0 / samples as f32;
    let mut sums = vec![[0.0f32; 5]; count];
    splat_two_slices(&mut sums, slice, nz, &items, |item, lo, hi| {
        let item = item as usize;
        let g = grid_coord(position_of(item));
        let base = g.floor();
        let f = g - base;
        let (x0, y0) = (base.x as usize, base.y as usize);
        let fluid = item < n_fluid;
        for (zs, z, wz) in [(&mut *lo, 0.0, 1.0 - f.z), (&mut *hi, 1.0, f.z)] {
            for (dy, wy) in [(0, 1.0 - f.y), (1, f.y)] {
                for (dx, wx) in [(0, 1.0 - f.x), (1, f.x)] {
                    let cell = &mut zs[(y0 + dy) * nx + x0 + dx];
                    if fluid {
                        let w = wx * wy * wz * inv_samples;
                        cell[0] += w;
                        cell[1] += w * (f.x - dx as f32);
                        cell[2] += w * (f.y - dy as f32);
                        cell[3] += w * (f.z - z);
                    } else {
                        cell[4] += wx * wy * wz;
                    }
                }
            }
        }
    });
    // Wall proximity: recomputed when the grid or a collider changes.
    if resized || out.walls.len() != count || out.walls_version != Some(input.colliders.version) {
        out.walls.resize(count, [0.0, 1e3]);
        let width = (opts.meniscus_width * r).max(1e-5);
        let colliders = input.colliders;
        let enabled = opts.wet_colliders && !colliders.is_empty();
        out.walls
            .par_chunks_mut(slice)
            .enumerate()
            .for_each(|(z, row)| {
                for y in 0..ny {
                    for x in 0..nx {
                        row[y * nx + x] = if enabled {
                            let p = origin
                                + (Vec3A::new(x as f32, y as f32, z as f32) + Vec3A::splat(0.5))
                                    * voxel;
                            let nearest = colliders.nearest_distance(p, true).min(1e3);
                            [(-(nearest.max(0.0)) / width).exp(), nearest]
                        } else {
                            [0.0, 1e3]
                        };
                    }
                }
            });
        out.walls_version = Some(colliders.version);
    }
    // 2. Blur with re-based offsets, three passes.
    let sigma = 0.75 * spacing / voxel;
    let taps = ((2.5 * sigma).ceil() as i32).clamp(2, 6);
    let mut weights: Vec<f32> = (-taps..=taps)
        .map(|k| (-((k * k) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let wsum: f32 = weights.iter().sum();
    weights.iter_mut().for_each(|w| *w /= wsum);
    let dims_arr = [nx, ny, nz];
    let blur = |src: &[[f32; 5]], dst: &mut [[f32; 5]], axis: usize| {
        dst.par_chunks_mut(slice).enumerate().for_each(|(z, out)| {
            for y in 0..ny {
                for x in 0..nx {
                    let mut acc = [0.0f32; 5];
                    if brick_active(x, y, z) {
                        let c = [x as i32, y as i32, z as i32];
                        for (t, &w) in weights.iter().enumerate() {
                            let k = t as i32 - taps;
                            let mut nb = c;
                            nb[axis] += k;
                            if nb[axis] < 0 || nb[axis] >= dims_arr[axis] as i32 {
                                continue;
                            }
                            let s =
                                src[nb[2] as usize * slice + nb[1] as usize * nx + nb[0] as usize];
                            acc[0] += s[0] * w;
                            for a in 0..3 {
                                let shift = if a == axis { k as f32 * s[0] } else { 0.0 };
                                acc[1 + a] += (s[1 + a] + shift) * w;
                            }
                            acc[4] += s[4] * w;
                        }
                    }
                    out[y * nx + x] = acc;
                }
            }
        });
    };
    let mut tmp = vec![[0.0f32; 5]; count];
    blur(&sums, &mut tmp, 0);
    blur(&tmp, &mut sums, 1);
    blur(&sums, &mut tmp, 2);
    // 3. Field.
    let volume_scale = (spacing / voxel).powi(3);
    let radius_vox = opts.radius * r * inv_voxel;
    let lift = opts.meniscus_height * r * inv_voxel;
    out.data.resize(count, [0.0; 4]);
    let walls = &out.walls;
    out.data
        .par_iter_mut()
        .enumerate()
        .with_min_len(1024)
        .for_each(|(i, cell)| {
            let s = tmp[i];
            let w = s[0];
            let mean = Vec3::new(s[1], s[2], s[3]) / w.max(1e-8);
            let mut phi = if w > 1e-5 {
                mean.length() - radius_vox
            } else {
                FIELD_BAND
            }
            .min(FIELD_BAND);
            let proximity = (s[4] * volume_scale * 1.5).clamp(0.0, 1.0).max(walls[i][0]);
            let reach = (1.0 - phi / (lift * 1.5).max(1e-5)).clamp(0.0, 1.0);
            phi -= lift * proximity * reach * reach;
            phi = phi.max(-walls[i][1] * inv_voxel);
            *cell = Vec4::new(phi, (w * volume_scale).min(4.0), proximity, 1.0).to_array();
        });
    out.bricks = active;
    out.brick_resolution = [bx as u32, by as u32, bz as u32];
}
