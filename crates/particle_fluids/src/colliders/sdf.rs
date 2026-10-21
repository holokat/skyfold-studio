//! Signed distance fields: the mesh → SDF bake, CPU sampling, and the
//! `.sdf.bin` format. Port of `sdf/bake.ts`, `sdf/sample.ts`,
//! `sdf/writeBinary.ts` and the CPU side of `core/collision/SDFCollider.ts`.

use crate::mesh::TriangleMesh;
use glam::{Quat, Vec3, Vec3A};
use rayon::prelude::*;

/// A signed distance field on a regular grid, in a mesh's local space.
/// Distances are in metres, negative inside.
#[derive(Clone, Debug, PartialEq)]
pub struct SdfData {
    /// One distance per voxel; voxel `(x, y, z)` is at `x + y·resX + z·resX·resY`.
    pub data: Vec<f32>,
    pub resolution: [u32; 3],
    /// Outer corner of voxel `(0, 0, 0)`; voxel `(i, j, k)` is centred at
    /// `origin + (i + ½, j + ½, k + ½) · voxel_size`.
    pub origin: Vec3,
    pub voxel_size: Vec3,
}

/// Errors from the SDF bake and codec.
#[derive(Clone, Debug, PartialEq)]
pub enum SdfError {
    /// `resolution` below 4 (bake) or 2 (decode).
    BadResolution,
    BadPadding,
    EmptyMesh,
    BadMesh(String),
    /// More than 1% of voxels got conflicting inside votes.
    NonWatertight {
        inconsistent: usize,
        total: usize,
    },
    BadBinary(&'static str),
}

impl std::fmt::Display for SdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SdfError::BadResolution => {
                write!(f, "bake_mesh_to_sdf: resolution must be an integer ≥ 4")
            }
            SdfError::BadPadding => write!(
                f,
                "bake_mesh_to_sdf: padding must be a non-negative finite number"
            ),
            SdfError::EmptyMesh => write!(f, "bake_mesh_to_sdf: mesh has zero triangles"),
            SdfError::BadMesh(m) => write!(f, "{m}"),
            SdfError::NonWatertight {
                inconsistent,
                total,
            } => write!(
                f,
                "bake_mesh_to_sdf: mesh appears non-watertight — {inconsistent} of {total} voxels had disagreeing axis sign votes"
            ),
            SdfError::BadBinary(m) => write!(f, "decode_sdf_binary: {m}"),
        }
    }
}

impl std::error::Error for SdfError {}

impl SdfData {
    /// Trilinear distance at `p` (local space). Voxel centres as on the GPU;
    /// points outside the grid read the nearest edge values.
    pub fn sample(&self, p: Vec3) -> f32 {
        let [rx, ry, rz] = self.resolution.map(|r| r as i32);
        let g = (p - self.origin) / self.voxel_size - Vec3::splat(0.5);
        let i0 = (g.x.floor() as i32).clamp(0, rx - 2);
        let j0 = (g.y.floor() as i32).clamp(0, ry - 2);
        let k0 = (g.z.floor() as i32).clamp(0, rz - 2);
        let tx = (g.x - i0 as f32).clamp(0.0, 1.0);
        let ty = (g.y - j0 as f32).clamp(0.0, 1.0);
        let tz = (g.z - k0 as f32).clamp(0.0, 1.0);
        let stride = rx as usize;
        let slice = (rx * ry) as usize;
        let base = i0 as usize + j0 as usize * stride + k0 as usize * slice;
        let d = &self.data;
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let c00 = lerp(d[base], d[base + 1], tx);
        let c10 = lerp(d[base + stride], d[base + stride + 1], tx);
        let c01 = lerp(d[base + slice], d[base + slice + 1], tx);
        let c11 = lerp(d[base + slice + stride], d[base + slice + stride + 1], tx);
        lerp(lerp(c00, c10, ty), lerp(c01, c11, ty), tz)
    }

    /// Central-difference gradient one voxel to either side (six samples), not normalized.
    pub fn gradient(&self, p: Vec3) -> Vec3 {
        let h = self.voxel_size;
        Vec3::new(
            (self.sample(p + Vec3::X * h.x) - self.sample(p - Vec3::X * h.x)) / (2.0 * h.x),
            (self.sample(p + Vec3::Y * h.y) - self.sample(p - Vec3::Y * h.y)) / (2.0 * h.y),
            (self.sample(p + Vec3::Z * h.z) - self.sample(p - Vec3::Z * h.z)) / (2.0 * h.z),
        )
    }

    /// Serialize to the `.sdf.bin` format (little-endian, 48-byte header).
    pub fn encode_binary(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(48 + self.data.len() * 4);
        out.extend_from_slice(b"PSDF");
        out.extend_from_slice(&1u32.to_le_bytes());
        for r in self.resolution {
            out.extend_from_slice(&r.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        for v in [self.origin, self.voxel_size] {
            for c in v.to_array() {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        for v in &self.data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Parse a `.sdf.bin` buffer.
    pub fn decode_binary(buffer: &[u8]) -> Result<Self, SdfError> {
        if buffer.len() < 48 {
            return Err(SdfError::BadBinary("buffer too small for header"));
        }
        if &buffer[0..4] != b"PSDF" {
            return Err(SdfError::BadBinary("bad magic"));
        }
        let u32_at = |o: usize| u32::from_le_bytes(buffer[o..o + 4].try_into().unwrap());
        let f32_at = |o: usize| f32::from_le_bytes(buffer[o..o + 4].try_into().unwrap());
        if u32_at(4) != 1 {
            return Err(SdfError::BadBinary("unsupported version"));
        }
        let resolution = [u32_at(8), u32_at(12), u32_at(16)];
        if resolution.iter().any(|&r| r < 2) {
            return Err(SdfError::BadBinary("resolution must be ≥ 2"));
        }
        let origin = Vec3::new(f32_at(24), f32_at(28), f32_at(32));
        let voxel_size = Vec3::new(f32_at(36), f32_at(40), f32_at(44));
        if !(voxel_size.cmpgt(Vec3::ZERO).all() && voxel_size.is_finite()) {
            return Err(SdfError::BadBinary("voxelSize must be positive"));
        }
        let count = resolution.iter().map(|&r| r as usize).product::<usize>();
        if buffer.len() < 48 + 4 * count {
            return Err(SdfError::BadBinary("buffer too small"));
        }
        let data = (0..count).map(|i| f32_at(48 + 4 * i)).collect();
        Ok(Self {
            data,
            resolution,
            origin,
            voxel_size,
        })
    }
}

#[derive(Clone, Copy)]
struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    min: Vec3,
    max: Vec3,
}

const RAY_EPS: f32 = 1e-7;
const MAX_INCONSISTENT_VOXEL_FRACTION: f64 = 0.01;

/// Bake a closed triangle mesh into a cubic SDF of `resolution³` voxels.
///
/// The grid is centred on the mesh bounds, each side the longest axis plus
/// `padding` at both ends (default: two voxels, one below resolution 8).
/// Distances are to the nearest triangle; inside is decided by a majority
/// vote of +X, +Y and +Z ray parities. Runs in parallel over z slices.
pub fn bake_mesh_to_sdf(
    mesh: &TriangleMesh,
    resolution: u32,
    padding: Option<f32>,
) -> Result<SdfData, SdfError> {
    if resolution < 4 {
        return Err(SdfError::BadResolution);
    }
    if padding.is_some_and(|p| !(p >= 0.0 && p.is_finite())) {
        return Err(SdfError::BadPadding);
    }
    mesh.validate().map_err(SdfError::BadMesh)?;
    if mesh.indices.is_empty() {
        return Err(SdfError::EmptyMesh);
    }
    let tris: Vec<Tri> = mesh
        .indices
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (
                mesh.vertices[t[0] as usize],
                mesh.vertices[t[1] as usize],
                mesh.vertices[t[2] as usize],
            );
            Tri {
                a,
                b,
                c,
                min: a.min(b).min(c),
                max: a.max(b).max(c),
            }
        })
        .collect();
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for t in &tris {
        lo = lo.min(t.min);
        hi = hi.max(t.max);
    }
    let res = resolution as usize;
    let padding = padding.unwrap_or_else(|| {
        let extent = (hi - lo).max_element();
        let k = if resolution >= 8 { 2.0 } else { 1.0 };
        k * extent / (resolution as f32 - 2.0 * k)
    });
    lo -= Vec3::splat(padding);
    hi += Vec3::splat(padding);
    let side = (hi - lo).max_element();
    let voxel = side / resolution as f32;
    let center = (lo + hi) * 0.5;
    let origin = center - Vec3::splat(0.5 * side);
    let jitter = voxel * 1e-4;
    let mut data = vec![0.0f32; res * res * res];
    let inconsistent: usize = data
        .par_chunks_mut(res * res)
        .enumerate()
        .map(|(zi, slab)| {
            let pz = origin.z + (zi as f32 + 0.5) * voxel;
            let mut bad = 0;
            for yi in 0..res {
                let py = origin.y + (yi as f32 + 0.5) * voxel;
                for xi in 0..res {
                    let px = origin.x + (xi as f32 + 0.5) * voxel;
                    let p = Vec3::new(px, py, pz);
                    let dist = closest_dist_sq(p, &tris).sqrt();
                    let (inside, incons) = inside_by_majority(p, &tris, jitter);
                    if incons {
                        bad += 1;
                    }
                    slab[yi * res + xi] = if inside { -dist } else { dist };
                }
            }
            bad
        })
        .sum();
    let total = res * res * res;
    if inconsistent as f64 / total as f64 > MAX_INCONSISTENT_VOXEL_FRACTION {
        return Err(SdfError::NonWatertight {
            inconsistent,
            total,
        });
    }
    Ok(SdfData {
        data,
        resolution: [resolution; 3],
        origin,
        voxel_size: Vec3::splat(voxel),
    })
}

fn closest_dist_sq(p: Vec3, tris: &[Tri]) -> f32 {
    let mut best = f32::INFINITY;
    for t in tris {
        let d = (t.min - p).max(Vec3::ZERO).max(p - t.max);
        if d.length_squared() >= best {
            continue;
        }
        let v = point_triangle_dist_sq(p, t.a, t.b, t.c);
        if v < best {
            best = v;
        }
    }
    best
}

/// Squared distance from `p` to triangle `abc` (Ericson, RTCD §5.1.5).
pub(crate) fn point_triangle_dist_sq(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length_squared();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length_squared();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (ap - ab * v).length_squared();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length_squared();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (ap - ac * w).length_squared();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (bp - (c - b) * w).length_squared();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (ap - ab * v - ac * w).length_squared()
}

fn inside_by_majority(p: Vec3, tris: &[Tri], jitter: f32) -> (bool, bool) {
    let a = jitter * std::f32::consts::SQRT_2;
    let b = jitter * std::f32::consts::PI;
    let hx = count_hits(Vec3::new(p.x, p.y + a, p.z + b), 0, tris) & 1 == 1;
    let hy = count_hits(Vec3::new(p.x + b, p.y, p.z + a), 1, tris) & 1 == 1;
    let hz = count_hits(Vec3::new(p.x + a, p.y + b, p.z), 2, tris) & 1 == 1;
    let n = hx as u32 + hy as u32 + hz as u32;
    (n >= 2, n == 1 || n == 2)
}

fn count_hits(p: Vec3, axis: usize, tris: &[Tri]) -> u32 {
    let mut hits = 0;
    for t in tris {
        let skip = match axis {
            0 => p.y < t.min.y || p.y > t.max.y || p.z < t.min.z || p.z > t.max.z || p.x > t.max.x,
            1 => p.x < t.min.x || p.x > t.max.x || p.z < t.min.z || p.z > t.max.z || p.y > t.max.y,
            _ => p.x < t.min.x || p.x > t.max.x || p.y < t.min.y || p.y > t.max.y || p.z > t.max.z,
        };
        if !skip && ray_tri_hit(p, axis, t) {
            hits += 1;
        }
    }
    hits
}

fn ray_tri_hit(p: Vec3, axis: usize, t: &Tri) -> bool {
    let e1 = t.b - t.a;
    let e2 = t.c - t.a;
    let h = match axis {
        0 => Vec3::new(0.0, -e2.z, e2.y),
        1 => Vec3::new(e2.z, 0.0, -e2.x),
        _ => Vec3::new(-e2.y, e2.x, 0.0),
    };
    let a = e1.dot(h);
    if a.abs() < RAY_EPS {
        return false;
    }
    let inv_a = 1.0 / a;
    let s = p - t.a;
    let u = inv_a * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = inv_a * q[axis];
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    inv_a * e2.dot(q) > RAY_EPS
}

/// A mesh obstacle: a baked field placed in the world with a rigid transform
/// and uniform scale. Friction treats it as static, like the reference.
#[derive(Clone, Debug)]
pub(crate) struct SdfCollider {
    pub sdf: SdfData,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
    /// Contact distance beyond the particle radius, m (not scaled).
    pub thickness: f32,
    pub mu_s: f32,
    pub mu_k: f32,
    pub enabled: bool,
}

impl SdfCollider {
    /// World distance (`scale · φ_local`) and world gradient (rotated, not
    /// normalized) at `x`.
    pub fn eval(&self, x: Vec3A) -> (f32, Vec3A) {
        let local = self.rotation.conjugate() * (Vec3::from(x) - self.position) / self.scale;
        let phi = self.sdf.sample(local) * self.scale;
        let g = self.rotation * self.sdf.gradient(local);
        (phi, Vec3A::from(g))
    }

    /// Lower bound on the distance from `x` to the field's bounding sphere
    /// (used by the surface field, where the edge-clamped field is invalid).
    pub fn bound_distance(&self, x: Vec3A) -> f32 {
        let half = Vec3::new(
            self.sdf.resolution[0] as f32,
            self.sdf.resolution[1] as f32,
            self.sdf.resolution[2] as f32,
        ) * self.sdf.voxel_size
            * 0.5;
        let centre = self.position + self.rotation * (self.sdf.origin + half) * self.scale;
        (Vec3::from(x) - centre).length() - half.length() * self.scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_bake_is_accurate() {
        let mesh = TriangleMesh::uv_sphere(Vec3::ZERO, 0.5, 48, 32);
        let sdf = bake_mesh_to_sdf(&mesh, 24, None).unwrap();
        for p in [
            Vec3::new(0.0, -0.2, 0.25),
            Vec3::new(0.3, 0.0, 0.0),
            Vec3::new(0.0, 0.6, 0.1),
        ] {
            let expected = p.length() - 0.5;
            assert!(
                (sdf.sample(p) - expected).abs() < 0.03,
                "{p} {} {expected}",
                sdf.sample(p)
            );
        }
        let g = sdf.gradient(Vec3::new(0.4, 0.0, 0.0));
        assert!((g.length() - 1.0).abs() < 0.1 && g.x > 0.9);
        let back = SdfData::decode_binary(&sdf.encode_binary()).unwrap();
        assert_eq!(back, sdf);
    }

    #[test]
    fn open_mesh_is_rejected() {
        let mut mesh = TriangleMesh::cuboid(Vec3::ZERO, Vec3::ONE);
        mesh.indices.truncate(mesh.indices.len() - 6);
        assert!(matches!(
            bake_mesh_to_sdf(&mesh, 12, None),
            Err(SdfError::NonWatertight { .. })
        ));
    }
}
