//! Fill a closed shape with particles on a cubic grid (port of `softbody/voxelize.ts`).

use crate::colliders::sdf::SdfData;
use crate::mesh::TriangleMesh;
use glam::Vec3;
use rayon::prelude::*;

/// Particles filling a shape, surface particles first.
#[derive(Clone, Debug, Default)]
pub struct VoxelizeResult {
    /// Positions in the shape's own space.
    pub positions: Vec<Vec3>,
    /// Number of leading particles on the surface (missing a face neighbor).
    pub surface_count: usize,
    /// Face-adjacent pairs `[i, j]` with `i < j`.
    pub edges: Vec<[u32; 2]>,
}

/// What to fill.
pub enum VoxelShape<'a> {
    /// A closed mesh; inside is decided by ray-cast parity.
    Mesh(&'a TriangleMesh),
    /// A distance field; points with distance below `dilation` are inside.
    Sdf { sdf: &'a SdfData, dilation: f32 },
}

const INTERSECT_EPS: f32 = 1e-9;

fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let e1 = b - a;
    let e2 = c - a;
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det > -INTERSECT_EPS && det < INTERSECT_EPS {
        return -1.0;
    }
    let f = 1.0 / det;
    let s = o - a;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return -1.0;
    }
    let q = s.cross(e1);
    let v = f * d.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return -1.0;
    }
    let t = f * e2.dot(q);
    if t < INTERSECT_EPS {
        return -1.0;
    }
    t
}

/// Point-in-mesh by ray parity along a slightly off-axis ray.
pub(crate) fn inside_mesh(mesh: &TriangleMesh, p: Vec3) -> bool {
    let dir = Vec3::new(1.0, 0.00137, 0.00241);
    let mut crossings = 0;
    for t in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (
            mesh.vertices[t[0] as usize],
            mesh.vertices[t[1] as usize],
            mesh.vertices[t[2] as usize],
        );
        if ray_triangle(p, dir, a, b, c) > 0.0 {
            crossings += 1;
        }
    }
    crossings & 1 == 1
}

/// Fill `shape` with particles spaced `2 · particle_radius` apart.
pub fn voxelize(
    shape: VoxelShape<'_>,
    particle_radius: f32,
    largest_piece: bool,
) -> VoxelizeResult {
    let spacing = 2.0 * particle_radius;
    let (min, max) = match &shape {
        VoxelShape::Mesh(m) => match m.bounds() {
            Some(b) => b,
            None => return VoxelizeResult::default(),
        },
        VoxelShape::Sdf { sdf, .. } => {
            let r = Vec3::new(
                sdf.resolution[0] as f32,
                sdf.resolution[1] as f32,
                sdf.resolution[2] as f32,
            );
            (sdf.origin, sdf.origin + r * sdf.voxel_size)
        }
    };
    let dims = ((max - min) / spacing).ceil().max(Vec3::ONE);
    let (nx, ny, nz) = (dims.x as usize, dims.y as usize, dims.z as usize);
    let cells = nx * ny * nz;
    let center = |cell: usize| {
        let (x, y, z) = (cell % nx, (cell / nx) % ny, cell / (nx * ny));
        min + Vec3::splat(particle_radius) + spacing * Vec3::new(x as f32, y as f32, z as f32)
    };
    let mut occupied: Vec<bool> = (0..cells)
        .into_par_iter()
        .map(|cell| match &shape {
            VoxelShape::Mesh(m) => inside_mesh(m, center(cell)),
            VoxelShape::Sdf { sdf, dilation } => sdf.sample(center(cell)) < *dilation,
        })
        .collect();
    let neighbors = |cell: usize, occ: &[bool]| -> Vec<usize> {
        let (x, y, z) = (cell % nx, (cell / nx) % ny, cell / (nx * ny));
        let mut out = Vec::with_capacity(6);
        if x > 0 {
            out.push(cell - 1);
        }
        if x + 1 < nx {
            out.push(cell + 1);
        }
        if y > 0 {
            out.push(cell - nx);
        }
        if y + 1 < ny {
            out.push(cell + nx);
        }
        if z > 0 {
            out.push(cell - nx * ny);
        }
        if z + 1 < nz {
            out.push(cell + nx * ny);
        }
        out.retain(|&n| occ[n]);
        out
    };
    if largest_piece {
        let mut piece = vec![usize::MAX; cells];
        let mut largest: Vec<usize> = Vec::new();
        for seed in 0..cells {
            if !occupied[seed] || piece[seed] != usize::MAX {
                continue;
            }
            let mut members = vec![seed];
            piece[seed] = seed;
            let mut k = 0;
            while k < members.len() {
                for n in neighbors(members[k], &occupied) {
                    if piece[n] == usize::MAX {
                        piece[n] = seed;
                        members.push(n);
                    }
                }
                k += 1;
            }
            if members.len() > largest.len() {
                largest = members;
            }
        }
        occupied.iter_mut().for_each(|o| *o = false);
        for c in largest {
            occupied[c] = true;
        }
    }
    let filled: Vec<usize> = (0..cells).filter(|&c| occupied[c]).collect();
    let is_surface = |c: usize| neighbors(c, &occupied).len() < 6;
    let surface: Vec<usize> = filled.iter().copied().filter(|&c| is_surface(c)).collect();
    let mut ordered = surface.clone();
    ordered.extend(filled.iter().copied().filter(|&c| !is_surface(c)));
    let mut particle_of = std::collections::HashMap::with_capacity(ordered.len());
    for (i, &c) in ordered.iter().enumerate() {
        particle_of.insert(c, i as u32);
    }
    let positions = ordered.iter().map(|&c| center(c)).collect();
    let mut edges = Vec::new();
    for &c in &ordered {
        let i = particle_of[&c];
        for n in neighbors(c, &occupied) {
            let j = particle_of[&n];
            if i < j {
                edges.push([i, j]);
            }
        }
    }
    VoxelizeResult {
        positions,
        surface_count: surface.len(),
        edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_fills_with_surface_first() {
        let mesh = TriangleMesh::cuboid(Vec3::ZERO, Vec3::splat(0.1));
        let r = voxelize(VoxelShape::Mesh(&mesh), 0.025, true);
        assert_eq!(r.positions.len(), 64);
        assert_eq!(r.surface_count, 56);
        assert_eq!(r.edges.len(), 3 * 4 * 4 * 3);
    }
}
