//! Cloth topology from an indexed triangle mesh (port of `cloth/graph.ts`),
//! and the row-major rectangle grid `Simulation.addCloth` builds (three.js
//! `PlaneGeometry` vertex and triangle order).

use glam::{Quat, Vec3};
use std::collections::HashMap;

/// Welded vertices, masses, and constraint topology of a cloth.
#[derive(Clone, Debug)]
pub struct ClothGraph {
    /// Welded vertex positions, one per particle.
    pub positions: Vec<Vec3>,
    /// Inverse mass per vertex, 1/kg; 0 for pinned vertices. A vertex's mass is
    /// `surface_density` times a third of the area of its triangles.
    pub inv_mass: Vec<f32>,
    /// Every unique triangle edge `[i, j]`, `i < j`, in order of first appearance.
    pub distance_pairs: Vec<[u32; 2]>,
    pub distance_rest_lengths: Vec<f32>,
    /// Non-degenerate triangles, in input order.
    pub triangles: Vec<[u32; 3]>,
    /// `[p1, p2, p3, p4]` per edge shared by exactly two triangles: `p1, p2` the
    /// edge, `p3, p4` the opposite vertices.
    pub bending_tuples: Vec<[u32; 4]>,
    /// Signed rest dihedral angle per tuple, radians in (−π, π]; 0 is flat.
    pub bending_rest_angles: Vec<f32>,
}

/// Errors from [`create_cloth_graph`].
#[derive(Clone, Debug, PartialEq)]
pub enum ClothGraphError {
    BadSurfaceDensity,
    BadIndexCount,
    BadPin(usize),
    IsolatedVertex(usize),
}

impl std::fmt::Display for ClothGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClothGraphError::BadSurfaceDensity => {
                write!(f, "create_cloth_graph: surface_density must be positive")
            }
            ClothGraphError::BadIndexCount => {
                write!(f, "create_cloth_graph: index count is not a multiple of 3")
            }
            ClothGraphError::BadPin(k) => {
                write!(f, "create_cloth_graph: pinned index {k} is not a vertex")
            }
            ClothGraphError::IsolatedVertex(v) => {
                write!(
                    f,
                    "create_cloth_graph: vertex {v} is in no non-degenerate triangle"
                )
            }
        }
    }
}

impl std::error::Error for ClothGraphError {}

const WELD_EPSILON: f64 = 1e-6;

/// Build a cloth graph from an indexed triangle mesh. Vertices within 1e-6 m
/// of an earlier vertex are welded; `pinned` indexes the welded list.
pub fn create_cloth_graph(
    vertices: &[Vec3],
    indices: &[u32],
    surface_density: f32,
    pinned: &[u32],
) -> Result<ClothGraph, ClothGraphError> {
    if !(surface_density > 0.0 && surface_density.is_finite()) {
        return Err(ClothGraphError::BadSurfaceDensity);
    }
    if !indices.len().is_multiple_of(3) {
        return Err(ClothGraphError::BadIndexCount);
    }
    // Weld in a hashed grid of WELD_EPSILON cells.
    let mut old_to_new = Vec::with_capacity(vertices.len());
    let mut positions: Vec<Vec3> = Vec::new();
    let mut cells: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    for p in vertices {
        let c = (
            (p.x as f64 / WELD_EPSILON).floor() as i64,
            (p.y as f64 / WELD_EPSILON).floor() as i64,
            (p.z as f64 / WELD_EPSILON).floor() as i64,
        );
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = cells.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) {
                        for &k in list {
                            if (positions[k as usize] - *p).length_squared() as f64
                                <= WELD_EPSILON * WELD_EPSILON
                            {
                                found = Some(k);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        let idx = match found {
            Some(k) => k,
            None => {
                positions.push(*p);
                let k = positions.len() as u32 - 1;
                cells.entry(c).or_default().push(k);
                k
            }
        };
        old_to_new.push(idx);
    }
    let n = positions.len();
    for &k in pinned {
        if k as usize >= n {
            return Err(ClothGraphError::BadPin(k as usize));
        }
    }
    let mut area = vec![0.0f64; n];
    let mut triangles = Vec::new();
    // Edge key -> index into `edges` (first-appearance order, like a JS Map).
    let mut edge_index: HashMap<(u32, u32), usize> = HashMap::new();
    let mut edges: Vec<(u32, u32, Vec<u32>)> = Vec::new();
    for t in indices.chunks_exact(3) {
        let (ia, ib, ic) = (
            old_to_new[t[0] as usize],
            old_to_new[t[1] as usize],
            old_to_new[t[2] as usize],
        );
        if ia == ib || ib == ic || ia == ic {
            continue;
        }
        let (a, b, c) = (
            positions[ia as usize],
            positions[ib as usize],
            positions[ic as usize],
        );
        let tri_area = 0.5 * ((b - a).as_dvec3().cross((c - a).as_dvec3())).length();
        if !(tri_area > 0.0 && tri_area.is_finite()) {
            continue;
        }
        triangles.push([ia, ib, ic]);
        for v in [ia, ib, ic] {
            area[v as usize] += tri_area / 3.0;
        }
        for (u, v, far) in [(ia, ib, ic), (ib, ic, ia), (ic, ia, ib)] {
            let key = (u.min(v), u.max(v));
            let e = *edge_index.entry(key).or_insert_with(|| {
                edges.push((key.0, key.1, Vec::new()));
                edges.len() - 1
            });
            edges[e].2.push(far);
        }
    }
    if let Some(v) = area.iter().position(|&a| a <= 0.0) {
        return Err(ClothGraphError::IsolatedVertex(v));
    }
    let mut is_pinned = vec![false; n];
    for &k in pinned {
        is_pinned[k as usize] = true;
    }
    let inv_mass = (0..n)
        .map(|i| {
            if is_pinned[i] {
                0.0
            } else {
                (1.0 / (area[i] * surface_density as f64)) as f32
            }
        })
        .collect();
    let mut graph = ClothGraph {
        positions,
        inv_mass,
        distance_pairs: Vec::with_capacity(edges.len()),
        distance_rest_lengths: Vec::with_capacity(edges.len()),
        triangles,
        bending_tuples: Vec::new(),
        bending_rest_angles: Vec::new(),
    };
    for (a, b, far) in &edges {
        graph.distance_pairs.push([*a, *b]);
        graph
            .distance_rest_lengths
            .push(graph.positions[*a as usize].distance(graph.positions[*b as usize]));
        if far.len() == 2 {
            let tuple = [*a, *b, far[0], far[1]];
            graph
                .bending_rest_angles
                .push(dihedral_angle(&graph.positions, tuple));
            graph.bending_tuples.push(tuple);
        }
    }
    Ok(graph)
}

/// Signed dihedral angle `atan2((N1×N2)·E, (N1·N2)|E|)` with
/// `N1 = (p3−p1)×(p3−p2)`, `N2 = (p4−p2)×(p4−p1)`, `E = p2−p1` (Bridson 2003 §4).
fn dihedral_angle(positions: &[Vec3], t: [u32; 4]) -> f32 {
    let [p1, p2, p3, p4] = t.map(|i| positions[i as usize].as_dvec3());
    let e = p2 - p1;
    let el = e.length();
    if el < 1e-12 {
        return 0.0;
    }
    let n1 = (p3 - p1).cross(p3 - p2);
    let n2 = (p4 - p2).cross(p4 - p1);
    if n1.length_squared() < 1e-24 || n2.length_squared() < 1e-24 {
        return 0.0;
    }
    n1.cross(n2).dot(e).atan2(n1.dot(n2) * el) as f32
}

/// A `width × height` grid with `columns × rows` cells, upright in the XY plane
/// facing +z, centred at the origin, rotated by `rotation` then moved to
/// `position`. Vertex `row · (columns + 1) + column`, row 0 at the top (+y).
/// Returns vertices and triangle indices in three.js `PlaneGeometry` order.
pub fn plane_grid(
    width: f32,
    height: f32,
    columns: u32,
    rows: u32,
    position: Vec3,
    rotation: Quat,
) -> (Vec<Vec3>, Vec<u32>) {
    let gx1 = columns + 1;
    let gy1 = rows + 1;
    let mut vertices = Vec::with_capacity((gx1 * gy1) as usize);
    for iy in 0..gy1 {
        let y = height * 0.5 - iy as f32 * height / rows as f32;
        for ix in 0..gx1 {
            let x = ix as f32 * width / columns as f32 - width * 0.5;
            vertices.push(rotation * Vec3::new(x, y, 0.0) + position);
        }
    }
    (vertices, plane_indices(columns, rows))
}

/// Triangle indices of a `columns × rows` cell grid in `PlaneGeometry` order.
pub fn plane_indices(columns: u32, rows: u32) -> Vec<u32> {
    let gx1 = columns + 1;
    let mut indices = Vec::with_capacity((columns * rows * 6) as usize);
    for iy in 0..rows {
        for ix in 0..columns {
            let a = ix + gx1 * iy;
            let b = ix + gx1 * (iy + 1);
            let c = (ix + 1) + gx1 * (iy + 1);
            let d = (ix + 1) + gx1 * iy;
            indices.extend_from_slice(&[a, b, d, b, c, d]);
        }
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_grid_topology() {
        let (v, i) = plane_grid(1.0, 1.0, 4, 3, Vec3::ZERO, Quat::IDENTITY);
        let g = create_cloth_graph(&v, &i, 0.2, &[0, 1]).unwrap();
        assert_eq!(g.positions.len(), 20);
        assert_eq!(g.triangles.len(), 24);
        // Edges: horizontal 4·4, vertical 5·3, diagonals 12.
        assert_eq!(g.distance_pairs.len(), 16 + 15 + 12);
        assert!(g.bending_rest_angles.iter().all(|a| a.abs() < 1e-6));
        assert_eq!(g.inv_mass[0], 0.0);
        let total_mass: f64 = g.inv_mass.iter().skip(2).map(|&w| 1.0 / w as f64).sum();
        assert!(total_mass < 0.2 + 1e-6);
    }
}
