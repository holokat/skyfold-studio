//! Indexed triangle meshes, as flat arrays (port of `core/mesh.ts`).

use glam::Vec3;

/// An indexed triangle mesh: vertex positions and three indices per triangle.
#[derive(Clone, Debug, Default)]
pub struct TriangleMesh {
    pub vertices: Vec<Vec3>,
    pub indices: Vec<u32>,
}

impl TriangleMesh {
    pub fn new(vertices: Vec<Vec3>, indices: Vec<u32>) -> Self {
        Self { vertices, indices }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Check the index buffer: a multiple of 3, every index in range.
    pub fn validate(&self) -> Result<(), String> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(format!(
                "TriangleMesh: indices length {} is not a multiple of 3",
                self.indices.len()
            ));
        }
        let n = self.vertices.len() as u32;
        if let Some(bad) = self.indices.iter().find(|&&i| i >= n) {
            return Err(format!("TriangleMesh: index {bad} is out of range"));
        }
        Ok(())
    }

    /// Bounding box of the vertices, or `None` for an empty mesh.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.vertices.first()?;
        Some(
            self.vertices
                .iter()
                .fold((first, first), |(lo, hi), &v| (lo.min(v), hi.max(v))),
        )
    }

    /// A closed axis-aligned box mesh (12 triangles, outward winding).
    pub fn cuboid(center: Vec3, half: Vec3) -> Self {
        let mut vertices = Vec::with_capacity(8);
        for i in 0..8u32 {
            let s = Vec3::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { -1.0 } else { 1.0 },
            );
            vertices.push(center + s * half);
        }
        let indices = vec![
            0, 2, 1, 1, 2, 3, // -z
            4, 5, 6, 5, 7, 6, // +z
            0, 1, 4, 1, 5, 4, // -y
            2, 6, 3, 3, 6, 7, // +y
            0, 4, 2, 2, 4, 6, // -x
            1, 3, 5, 3, 7, 5, // +x
        ];
        Self { vertices, indices }
    }

    /// A closed UV sphere.
    pub fn uv_sphere(center: Vec3, radius: f32, segments: u32, rings: u32) -> Self {
        let segments = segments.max(3);
        let rings = rings.max(2);
        let mut vertices = vec![center + Vec3::Y * radius];
        for r in 1..rings {
            let phi = std::f32::consts::PI * r as f32 / rings as f32;
            for s in 0..segments {
                let theta = std::f32::consts::TAU * s as f32 / segments as f32;
                vertices.push(
                    center
                        + radius
                            * Vec3::new(
                                phi.sin() * theta.cos(),
                                phi.cos(),
                                phi.sin() * theta.sin(),
                            ),
                );
            }
        }
        vertices.push(center - Vec3::Y * radius);
        let bottom = vertices.len() as u32 - 1;
        let ring = |r: u32, s: u32| 1 + (r - 1) * segments + (s % segments);
        let mut indices = Vec::new();
        for s in 0..segments {
            indices.extend_from_slice(&[0, ring(1, s + 1), ring(1, s)]);
        }
        for r in 1..rings - 1 {
            for s in 0..segments {
                let (a, b, c, d) = (
                    ring(r, s),
                    ring(r, s + 1),
                    ring(r + 1, s),
                    ring(r + 1, s + 1),
                );
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        for s in 0..segments {
            indices.extend_from_slice(&[bottom, ring(rings - 1, s), ring(rings - 1, s + 1)]);
        }
        Self { vertices, indices }
    }
}
