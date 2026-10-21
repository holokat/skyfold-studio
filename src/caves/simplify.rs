//! Making a cave's rock lighter for export (`export::core::caves`, only there: the editor keeps
//! every triangle): the chunks welded into one mesh, then simplified by edge collapses chosen
//! by quadric error (Garland & Heckbert) — the cheapest first, until the next would move the
//! surface more than `max_error`.
//!
//! - A collapse moves one end onto the other (no new positions: the normals and anything else
//!   carried per vertex stay right).
//! - It never flips or squashes a triangle, never pinches the surface (the two ends may share
//!   only the neighbours of their own edge), and never moves a vertex on an open border (the
//!   seams with the terrain round a mouth, a giant cave's coarse chunks) or one the caller locks.
//! - Flat floors and smooth walls shed most of their triangles; ledges, formations and rough
//!   rock keep theirs.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use bevy::math::{DVec3, Vec3};

/// How far (m) an exported cave's simplified rock may stray from the editor's (measured: the
/// triangles' middles sit ~2 cm further off the true rock than the editor's own mesh does, for
/// about two thirds fewer triangles; `caves::mesher` tests).
pub const EXPORT_TOLERANCE: f32 = 0.12;

/// Welds vertices closer than a millimetre (the chunks' meshes share their edges' vertices
/// exactly): the new vertex list (the first of each), and the index buffer remapped onto it.
pub fn weld(positions: &[Vec3], indices: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let mut keep: Vec<u32> = vec![];
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::with_capacity(positions.len());
    let mut remap = vec![0u32; positions.len()];
    for (i, p) in positions.iter().enumerate() {
        let k = (
            (p.x as f64 * 1000.0).round() as i64,
            (p.y as f64 * 1000.0).round() as i64,
            (p.z as f64 * 1000.0).round() as i64,
        );
        let id = *map.entry(k).or_insert_with(|| {
            keep.push(i as u32);
            keep.len() as u32 - 1
        });
        remap[i] = id;
    }
    let mut out = Vec::with_capacity(indices.len());
    for t in indices.chunks(3) {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a != b && b != c && a != c {
            out.extend_from_slice(&[a, b, c]);
        }
    }
    (keep, out)
}

/// A symmetric 4×4 quadric (the sum of squared distances to a set of planes), upper triangle.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: DVec3, d: f64) -> Quadric {
        let (a, b, c) = (n.x, n.y, n.z);
        Quadric([
            a * a,
            a * b,
            a * c,
            a * d,
            b * b,
            b * c,
            b * d,
            c * c,
            c * d,
            d * d,
        ])
    }
    fn add(&mut self, o: &Quadric) {
        for k in 0..10 {
            self.0[k] += o.0[k];
        }
    }
    fn error(&self, p: DVec3) -> f64 {
        let q = &self.0;
        let (x, y, z) = (p.x, p.y, p.z);
        (q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9])
            .max(0.0)
    }
}

#[derive(PartialEq)]
struct Candidate {
    cost: f64,
    from: u32,
    to: u32,
    /// the two ends' versions when it was costed (stale once either changed)
    stamp: (u32, u32),
}
impl Eq for Candidate {}
impl Ord for Candidate {
    fn cmp(&self, o: &Self) -> Ordering {
        // (a min-heap on cost)
        o.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Simplifies the triangle list `indices` over `positions` (welded: shared vertices) until the
/// cheapest collapse would move the surface more than `max_error` metres. `lock`: vertices that
/// must not move (open borders are locked anyway). Returns the new index list (into the same
/// vertices; unused ones are simply no longer referenced).
pub fn simplify(positions: &[Vec3], indices: &[u32], max_error: f32, lock: &[bool]) -> Vec<u32> {
    let nv = positions.len();
    let mut faces: Vec<[u32; 3]> = indices.chunks(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut alive = vec![true; faces.len()];
    let pos: Vec<DVec3> = positions.iter().map(|p| p.as_dvec3()).collect();
    // the faces round each vertex
    let mut around: Vec<Vec<u32>> = vec![vec![]; nv];
    for (f, t) in faces.iter().enumerate() {
        for &v in t {
            around[v as usize].push(f as u32);
        }
    }
    // each vertex's quadric: the planes of its faces
    let mut quad = vec![Quadric::default(); nv];
    for t in &faces {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let n = (b - a).cross(c - a);
        let len = n.length();
        if len < 1e-12 {
            continue;
        }
        let n = n / len;
        let q = Quadric::plane(n, -n.dot(a));
        for &v in t {
            quad[v as usize].add(&q);
        }
    }
    // open borders (an edge on one face) and pinched ones (more than two) stay put
    let mut edges: HashMap<(u32, u32), u32> = HashMap::with_capacity(faces.len() * 2);
    for t in &faces {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut fixed: Vec<bool> = (0..nv)
        .map(|i| lock.get(i).copied().unwrap_or(false))
        .collect();
    for (&(a, b), &n) in &edges {
        if n != 2 {
            fixed[a as usize] = true;
            fixed[b as usize] = true;
        }
    }
    let mut version = vec![0u32; nv];
    let mut gone = vec![false; nv];
    let limit = (max_error as f64).powi(2);
    let cost = |quad: &[Quadric], a: u32, b: u32, fixed: &[bool]| -> Option<(f64, u32, u32)> {
        let q = {
            let mut q = quad[a as usize];
            q.add(&quad[b as usize]);
            q
        };
        let (fa, fb) = (fixed[a as usize], fixed[b as usize]);
        match (fa, fb) {
            (true, true) => None,
            (true, false) => Some((q.error(pos[a as usize]), b, a)),
            (false, true) => Some((q.error(pos[b as usize]), a, b)),
            _ => {
                let (ea, eb) = (q.error(pos[a as usize]), q.error(pos[b as usize]));
                Some(if ea < eb { (ea, b, a) } else { (eb, a, b) })
            }
        }
    };
    let mut heap = BinaryHeap::with_capacity(edges.len());
    for &(a, b) in edges.keys() {
        if let Some((c, from, to)) = cost(&quad, a, b, &fixed)
            && c <= limit
        {
            heap.push(Candidate {
                cost: c,
                from,
                to,
                stamp: (0, 0),
            });
        }
    }
    let neighbours =
        |around: &[Vec<u32>], faces: &[[u32; 3]], alive: &[bool], v: u32| -> Vec<u32> {
            let mut out: Vec<u32> = around[v as usize]
                .iter()
                .filter(|f| alive[**f as usize])
                .flat_map(|f| faces[*f as usize])
                .filter(|x| *x != v)
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        };
    while let Some(c) = heap.pop() {
        if c.cost > limit {
            break;
        }
        let (from, to) = (c.from, c.to);
        if gone[from as usize]
            || gone[to as usize]
            || c.stamp != (version[from as usize], version[to as usize])
        {
            // stale: cost it again if both ends are still there and still joined
            if !gone[from as usize]
                && !gone[to as usize]
                && c.stamp != (version[from as usize], version[to as usize])
            {
                let joined = around[from as usize]
                    .iter()
                    .any(|f| alive[*f as usize] && faces[*f as usize].contains(&to));
                if joined && let Some((cc, f2, t2)) = cost(&quad, from, to, &fixed) {
                    heap.push(Candidate {
                        cost: cc,
                        from: f2,
                        to: t2,
                        stamp: (version[f2 as usize], version[t2 as usize]),
                    });
                }
            }
            continue;
        }
        // the link condition: the ends share only the far corners of the edge's two faces
        let (nf, nt) = (
            neighbours(&around, &faces, &alive, from),
            neighbours(&around, &faces, &alive, to),
        );
        let shared = nf.iter().filter(|v| nt.binary_search(v).is_ok()).count();
        let edge_faces = around[from as usize]
            .iter()
            .filter(|f| alive[**f as usize] && faces[**f as usize].contains(&to))
            .count();
        if edge_faces != 2 || shared != 2 {
            continue;
        }
        // no face of `from` may flip or squash when it moves onto `to`
        let p_to = pos[to as usize];
        let mut ok = true;
        for &f in &around[from as usize] {
            if !alive[f as usize] {
                continue;
            }
            let t = faces[f as usize];
            if t.contains(&to) {
                continue;
            }
            let p = t.map(|v| pos[v as usize]);
            let q = t.map(|v| if v == from { p_to } else { pos[v as usize] });
            let n0 = (p[1] - p[0]).cross(p[2] - p[0]);
            let n1 = (q[1] - q[0]).cross(q[2] - q[0]);
            let (l0, l1) = (n0.length(), n1.length());
            if l1 < 1e-9 || n0.dot(n1) < 0.2 * l0 * l1 {
                ok = false;
                break;
            }
            // (and no sliver: its smallest angle stays sane)
            let e = [
                (q[1] - q[0]).length(),
                (q[2] - q[1]).length(),
                (q[0] - q[2]).length(),
            ];
            let longest = e.iter().copied().fold(0.0, f64::max);
            if l1 / (longest * longest).max(1e-12) < 0.05 {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        // collapse
        for &f in &around[from as usize].clone() {
            if !alive[f as usize] {
                continue;
            }
            let t = &mut faces[f as usize];
            if t.contains(&to) {
                alive[f as usize] = false;
                continue;
            }
            for v in t.iter_mut() {
                if *v == from {
                    *v = to;
                }
            }
            around[to as usize].push(f);
        }
        around[to as usize].retain(|f| alive[*f as usize]);
        let qf = quad[from as usize];
        quad[to as usize].add(&qf);
        gone[from as usize] = true;
        version[to as usize] += 1;
        for n in neighbours(&around, &faces, &alive, to) {
            if let Some((cc, f2, t2)) = cost(&quad, to, n, &fixed)
                && cc <= limit
            {
                heap.push(Candidate {
                    cost: cc,
                    from: f2,
                    to: t2,
                    stamp: (version[f2 as usize], version[t2 as usize]),
                });
            }
        }
    }
    faces
        .iter()
        .zip(&alive)
        .filter(|(_, a)| **a)
        .flat_map(|(t, _)| *t)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bumpy grid (a flat part and a hill).
    fn grid(n: usize, bump: f32) -> (Vec<Vec3>, Vec<u32>) {
        let mut p = vec![];
        for j in 0..n {
            for i in 0..n {
                let (x, z) = (i as f32 * 0.4, j as f32 * 0.4);
                let h = if x > 6.0 {
                    ((x - 6.0) * 0.8).sin() * bump
                } else {
                    0.0
                };
                p.push(Vec3::new(x, h, z));
            }
        }
        let mut idx = vec![];
        for j in 0..n as u32 - 1 {
            for i in 0..n as u32 - 1 {
                let a = j * n as u32 + i;
                idx.extend_from_slice(&[
                    a,
                    a + n as u32,
                    a + 1,
                    a + 1,
                    a + n as u32,
                    a + n as u32 + 1,
                ]);
            }
        }
        (p, idx)
    }

    #[test]
    fn flat_parts_shed_triangles_and_the_shape_stays() {
        let (p, idx) = grid(40, 1.0);
        let out = simplify(&p, &idx, 0.05, &[]);
        assert!(
            out.len() < idx.len() / 2,
            "{} of {}",
            out.len() / 3,
            idx.len() / 3
        );
        // every kept vertex is an original one, every triangle faces up still
        for t in out.chunks(3) {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            assert!((b - a).cross(c - a).y < 0.0 || (b - a).cross(c - a).y > 0.0);
        }
        let up = |t: &[u32]| {
            (p[t[1] as usize] - p[t[0] as usize])
                .cross(p[t[2] as usize] - p[t[0] as usize])
                .y
                .signum()
        };
        let s0 = up(&idx[0..3]);
        assert!(out.chunks(3).all(|t| up(t) == s0), "a triangle flipped");
        // the open border (the grid's edge) kept every vertex
        let border: Vec<u32> = (0..40u32).collect();
        for v in border {
            assert!(out.contains(&v), "border vertex {v} gone");
        }
    }

    #[test]
    fn welding_joins_chunk_edges() {
        let p = vec![
            Vec3::ZERO,
            Vec3::X,
            Vec3::Z,
            Vec3::X,
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::Z,
        ];
        let (keep, idx) = weld(&p, &[0, 2, 1, 3, 5, 4]);
        assert_eq!(keep.len(), 4);
        assert_eq!(idx, vec![0, 2, 1, 1, 2, 3]);
    }
}
