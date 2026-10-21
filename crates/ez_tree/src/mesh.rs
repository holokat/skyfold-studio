//! Meshing a skeleton into vertex buffers (`#meshSkeleton`, `#meshBranch`,
//! `#meshLeaf`). Consumes no RNG.

use std::f64::consts::PI;

use glam::DVec3;

use crate::math::{Euler, apply_euler, js_max, js_round, normalize};
use crate::options::{Billboard, TreeOptions};
use crate::skeleton::{Skeleton, SkeletonBranch, SkeletonLeaf};

/// Meshing detail (ez-tree's `LODDetail`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detail {
    /// Keep every Nth section ring; the first and last rings are always kept
    /// so branch endpoints and junctions stay put. Values below 1 act as 1.
    pub section_stride: usize,
    /// Radial segment multiplier: `max(3, round(segments * factor))`.
    pub segment_factor: f64,
    /// Keep every Nth leaf. Values below 1 act as 1.
    pub leaf_stride: usize,
    /// Size multiplier for the kept leaves.
    pub leaf_scale: f64,
    /// Billboard override; `None` uses `options.leaves.billboard`.
    pub billboard: Option<Billboard>,
}

impl Detail {
    /// Full detail (ez-tree's `{}` detail / LOD 0).
    pub const FULL: Detail = Detail {
        section_stride: 1,
        segment_factor: 1.0,
        leaf_stride: 1,
        leaf_scale: 1.0,
        billboard: None,
    };

    /// Number of levels in ez-tree's `Tree.defaultLODLevels`.
    pub const LOD_COUNT: usize = 3;

    /// Camera distances at which ez-tree's demo switches to each default
    /// level (`Tree.defaultLODLevels[i].distance`, in tree units, with 5%
    /// hysteresis). Informational; pick distances for your own world scale.
    pub const DEFAULT_LOD_DISTANCES: [f64; 3] = [0.0, 100.0, 250.0];

    /// ez-tree's `Tree.defaultLODLevels[level].detail`. LOD 1 is roughly 40%
    /// of the full triangle count, LOD 2 roughly 20%. Levels above 2 return
    /// LOD 2.
    pub fn lod(level: usize) -> Detail {
        match level {
            0 => Self::FULL,
            1 => Detail {
                section_stride: 3,
                segment_factor: 0.75,
                leaf_stride: 2,
                leaf_scale: 1.25,
                billboard: None,
            },
            _ => Detail {
                section_stride: 6,
                segment_factor: 0.4,
                leaf_stride: 2,
                leaf_scale: 1.3,
                billboard: Some(Billboard::Single),
            },
        }
    }
}

impl Default for Detail {
    fn default() -> Self {
        Self::FULL
    }
}

/// Vertex buffers for one mesh (bark or leaves).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Buffers {
    /// Vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals.
    pub normals: Vec<[f32; 3]>,
    /// Texture coordinates. Bark `v` already includes `1 / textureScale.y`.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list indices (counter-clockwise front faces, as in three.js).
    pub indices: Vec<u32>,
    /// Per-vertex wind sway weight in `[0, 1]` (not part of ez-tree): 0 at
    /// the trunk base, continuous across branch junctions, highest at twig
    /// and leaf tips.
    pub sway: Vec<f32>,
    /// Bark only (not part of ez-tree): per vertex `[around, along, girth]` = the fraction of the
    /// way around the branch (0..=1), the distance from the branch base and the circumference
    /// at the base (tree units), for physically scaled bark mapping. Empty for leaves.
    pub bark: Vec<[f32; 3]>,
}

impl Buffers {
    /// Number of vertices.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn push(&mut self, position: DVec3, normal: DVec3, uv: [f64; 2], sway: f64) {
        self.positions.push(position.as_vec3().to_array());
        self.normals.push(normal.as_vec3().to_array());
        self.uvs.push([uv[0] as f32, uv[1] as f32]);
        self.sway.push(sway as f32);
    }
}

/// Bark and leaf geometry of one tree at one detail level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TreeMesh {
    /// Bark geometry.
    pub branches: Buffers,
    /// Leaf card geometry (render double-sided with alpha test).
    pub leaves: Buffers,
    /// Bounding box minimum over both meshes (zero if empty).
    pub min: [f32; 3],
    /// Bounding box maximum over both meshes (zero if empty).
    pub max: [f32; 3],
}

impl TreeMesh {
    /// Total triangles (bark + leaves).
    pub fn triangle_count(&self) -> usize {
        self.branches.triangle_count() + self.leaves.triangle_count()
    }

    /// Total vertices (bark + leaves).
    pub fn vertex_count(&self) -> usize {
        self.branches.vertex_count() + self.leaves.vertex_count()
    }

    /// Bounding box height (`max.y - min.y`).
    pub fn height(&self) -> f32 {
        self.max[1] - self.min[1]
    }
}

/// Branch vertex sway weight from the hierarchy parameter.
fn branch_sway(hierarchy: f64) -> f64 {
    hierarchy.clamp(0.0, 1.0).powf(1.5)
}

/// Meshes `skeleton` (grown from `opts`) at `detail`, like ez-tree's
/// `#meshSkeleton(detail)`.
pub fn mesh(opts: &TreeOptions, skeleton: &Skeleton, detail: &Detail) -> TreeMesh {
    let section_stride = detail.section_stride.max(1);
    let leaf_stride = detail.leaf_stride.max(1);
    let billboard = detail.billboard.unwrap_or(opts.leaves.billboard);

    let mut branches = Buffers::default();
    for branch in &skeleton.branches {
        mesh_branch(
            &mut branches,
            branch,
            opts,
            section_stride,
            detail.segment_factor,
        );
    }

    let mut leaves = Buffers::default();
    for leaf in skeleton.leaves.iter().step_by(leaf_stride) {
        mesh_leaf(
            &mut leaves,
            leaf,
            detail.leaf_scale,
            billboard,
            opts.leaves.rounded_normals,
        );
    }

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in branches.positions.iter().chain(&leaves.positions) {
        for c in 0..3 {
            min[c] = min[c].min(p[c]);
            max[c] = max[c].max(p[c]);
        }
    }
    if branches.positions.is_empty() && leaves.positions.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }

    TreeMesh {
        branches,
        leaves,
        min,
        max,
    }
}

/// Emits the rings and indices of one branch (`#meshBranch`).
fn mesh_branch(
    buffers: &mut Buffers,
    branch: &SkeletonBranch,
    opts: &TreeOptions,
    section_stride: usize,
    segment_factor: f64,
) {
    let sections = &branch.sections;
    if sections.is_empty() {
        return;
    }

    // `max(3, round(segmentCount * factor))`. Terminal branches inherit the
    // parent's segment count, so parent and child reduce to the same count
    // and junctions stay sealed. (Clamping after the cast also maps a NaN
    // factor to 3 instead of failing like the JS would.)
    let segments = (js_round(branch.segment_count as f64 * segment_factor) as usize).max(3);

    // Texture wraps around the circumference, constant per branch.
    let wraps_x = js_max(
        1.0,
        js_round(branch.base_radius * opts.bark.texture_scale.x),
    );
    // ez-tree sets `texture.repeat.y = 1 / textureScale.y` at runtime; bake it.
    let v_scale = opts.bark.texture_scale.y;

    // Every Nth ring, always including the first and the last.
    let mut sampled: Vec<usize> = (0..sections.len()).step_by(section_stride).collect();
    if !(sections.len() - 1).is_multiple_of(section_stride) {
        sampled.push(sections.len() - 1);
    }

    let index_offset = buffers.positions.len() as u32;

    // Distance of every ring from the branch base (all rings, so LODs agree) and the base girth.
    let mut along = Vec::with_capacity(sections.len());
    for (i, section) in sections.iter().enumerate() {
        along.push(if i == 0 {
            0.0
        } else {
            along[i - 1] + (section.origin - sections[i - 1].origin).length()
        });
    }
    let girth = (2.0 * PI * branch.base_radius) as f32;

    for (k, &s) in sampled.iter().enumerate() {
        let section = &sections[s];
        let sway = branch_sway(section.hierarchy);
        // v alternates by sampled ring so skipping rings keeps the 0/1 tiling.
        let v = if k % 2 == 0 { 0.0 } else { 1.0 };
        let v = v / v_scale;

        let mut first = None;
        for j in 0..segments {
            let angle = (2.0 * PI * j as f64) / segments as f64;
            let dir = DVec3::new(angle.cos(), 0.0, angle.sin());

            let vertex = apply_euler(dir * section.radius, section.orientation) + section.origin;
            let normal = normalize(apply_euler(dir, section.orientation));
            let uv = [(j as f64 / segments as f64) * wraps_x, v];

            buffers.push(vertex, normal, uv, sway);
            buffers
                .bark
                .push([(j as f64 / segments as f64) as f32, along[s] as f32, girth]);
            if j == 0 {
                first = Some((vertex, normal));
            }
        }

        // Duplicate the first vertex for UV continuity (u = wrapsX).
        let (vertex, normal) = first.expect("at least three segments");
        buffers.push(vertex, normal, [wraps_x, v], sway);
        buffers.bark.push([1.0, along[s] as f32, girth]);
    }

    // Cylinder walls without end caps.
    let n = segments as u32 + 1;
    for i in 0..sampled.len().saturating_sub(1) as u32 {
        for j in 0..segments as u32 {
            let v1 = index_offset + i * n + j;
            let v2 = index_offset + i * n + (j + 1);
            let v3 = v1 + n;
            let v4 = v2 + n;
            buffers.indices.extend_from_slice(&[v1, v3, v2, v2, v3, v4]);
        }
    }
}

/// Emits one leaf's card(s) (`#meshLeaf`).
fn mesh_leaf(
    buffers: &mut Buffers,
    leaf: &SkeletonLeaf,
    scale: f64,
    billboard: Billboard,
    rounded_normals: bool,
) {
    let size = leaf.size * scale;
    let (w, l) = (size, size);
    let base_sway = leaf.hierarchy.clamp(0.0, 1.0).powf(1.5);

    let mut card = |rotation: f64| {
        let i = buffers.positions.len() as u32;
        let corners = [
            DVec3::new(-w / 2.0, l, 0.0),
            DVec3::new(-w / 2.0, 0.0, 0.0),
            DVec3::new(w / 2.0, 0.0, 0.0),
            DVec3::new(w / 2.0, l, 0.0),
        ];
        const UVS: [[f64; 2]; 4] = [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];

        let n = apply_euler(DVec3::Z, leaf.orientation);
        for (corner, uv) in corners.into_iter().zip(UVS) {
            let v = apply_euler(
                apply_euler(corner, Euler::new(0.0, rotation, 0.0)),
                leaf.orientation,
            ) + leaf.origin;
            // Average of the leaf direction and the direction to the vertex,
            // implying a rounded canopy.
            let normal = if rounded_normals {
                normalize((n + v) - leaf.origin)
            } else {
                n
            };
            let sway = (base_sway + 0.15 * uv[1]).clamp(0.0, 1.0);
            buffers.push(v, normal, uv, sway);
        }
        buffers
            .indices
            .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    };

    card(0.0);
    if billboard == Billboard::Double {
        card(PI / 2.0);
    }
}
