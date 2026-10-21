//! Error-bounded, seam-aware mesh simplification for model LODs: half-edge collapses ordered by
//! quadric error, in the spirit of meshoptimizer's simplifier.
//!
//! Vertices are "wedges" (a position plus its other attributes); wedges sharing a position form
//! one geometric vertex. A vertex with one wedge and no open or seam edges collapses onto any
//! neighbour; a vertex on an open border or an attribute seam (UV island edge, hard normal edge)
//! only slides along that border or seam, carrying its wedges with it, so texture seams and hard
//! edges stay closed; anything else is locked. Only original vertices are kept, so attributes are
//! never interpolated and a LOD can reuse the source mesh's vertex data. Collapses stop once the
//! quadric (sum of squared distances to the original planes merged into a vertex) passes `tol²`.
//! Far LODs may also drop whole loose pieces smaller than a given size (fur strands, nails).

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Manifold,
    Border,
    Seam,
    Locked,
}

/// A symmetric 4×4 quadric (upper triangle).
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: [f64; 3], d: f64, w: f64) -> Self {
        let [a, b, c] = n;
        Quadric([
            a * a * w,
            a * b * w,
            a * c * w,
            a * d * w,
            b * b * w,
            b * c * w,
            b * d * w,
            c * c * w,
            c * d * w,
            d * d * w,
        ])
    }
    fn add(&mut self, o: &Quadric) {
        for (x, y) in self.0.iter_mut().zip(o.0) {
            *x += y;
        }
    }
    fn eval(&self, p: [f32; 3]) -> f64 {
        let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let q = &self.0;
        q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9]
    }
}

#[derive(PartialEq)]
struct Cand(f64, u32, u32, u32);
impl Eq for Cand {}
impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cand {
    // a min-heap on the cost
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f64; 3] {
    [
        (a[0] - b[0]) as f64,
        (a[1] - b[1]) as f64,
        (a[2] - b[2]) as f64,
    ]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Simplifies an indexed triangle list. `pos[i]` and `wedge[i]` (a hash of every other attribute
/// of vertex i) tell vertices apart; `tol` is the deviation allowed (mesh units); loose pieces
/// whose bounding box diagonal is under `prune` are dropped first (0: keep all). With `normals`, a
/// vertex more than 3 × `tol` from its target only goes if its normal is within `max_turn`
/// (cosine) of what its neighbours' normals will blend to across the new faces: shading that lives
/// in scattered normals (displaced bumps) never streaks across big faces, while small rounded
/// details (bevels) and flat or smooth areas still simplify. Returns the kept triangles as indices
/// into the same vertices.
pub fn simplify(
    pos: &[[f32; 3]],
    wedge: &[u64],
    tris: &[[u32; 3]],
    tol: f32,
    prune: f32,
    normals: Option<(&[[f32; 3]], f32)>,
) -> Vec<[u32; 3]> {
    // one id per distinct position, one canonical vertex per distinct (position, attributes)
    let mut pid_of: HashMap<[u32; 3], u32> = HashMap::new();
    let pid: Vec<u32> = pos
        .iter()
        .map(|p| {
            let n = pid_of.len() as u32;
            *pid_of.entry(p.map(f32::to_bits)).or_insert(n)
        })
        .collect();
    let npos = pid_of.len();
    let mut at = vec![[0f32; 3]; npos];
    for (i, &p) in pid.iter().enumerate() {
        at[p as usize] = pos[i];
    }
    let mut canon_of: HashMap<(u32, u64), u32> = HashMap::new();
    let canon: Vec<u32> = (0..pos.len())
        .map(|i| *canon_of.entry((pid[i], wedge[i])).or_insert(i as u32))
        .collect();
    let mut tw: Vec<[u32; 3]> = tris.iter().map(|t| t.map(|i| canon[i as usize])).collect();
    let mut tp: Vec<[u32; 3]> = tris.iter().map(|t| t.map(|i| pid[i as usize])).collect();
    let mut alive: Vec<bool> = tp
        .iter()
        .map(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect();

    if prune > 0.0 {
        prune_pieces(&tp, &mut alive, &at, npos, prune);
    }

    let mut vt: Vec<Vec<u32>> = vec![vec![]; npos];
    for (t, p) in tp.iter().enumerate() {
        if alive[t] {
            for &v in p {
                vt[v as usize].push(t as u32);
            }
        }
    }
    // edges: (a, b) with a < b -> (triangles, wedge pair of the first, differs)
    let mut edges: HashMap<(u32, u32), (u8, (u32, u32), bool)> = HashMap::new();
    for t in 0..tp.len() {
        if !alive[t] {
            continue;
        }
        for k in 0..3 {
            let (mut a, mut b) = (tp[t][k], tp[t][(k + 1) % 3]);
            let (mut wa, mut wb) = (tw[t][k], tw[t][(k + 1) % 3]);
            if a > b {
                std::mem::swap(&mut a, &mut b);
                std::mem::swap(&mut wa, &mut wb);
            }
            let e = edges.entry((a, b)).or_insert((0, (wa, wb), false));
            e.0 = e.0.saturating_add(1);
            if e.1 != (wa, wb) {
                e.2 = true;
            }
        }
    }
    let mut kind_e: HashMap<(u32, u32), Kind> = HashMap::with_capacity(edges.len());
    let (mut nb, mut ns, mut nc) = (vec![0u8; npos], vec![0u8; npos], vec![0u8; npos]);
    for (&(a, b), &(n, _, differs)) in &edges {
        let k = match n {
            1 => Kind::Border,
            2 if differs => Kind::Seam,
            2 => Kind::Manifold,
            _ => Kind::Locked,
        };
        kind_e.insert((a, b), k);
        for v in [a, b] {
            let c = match k {
                Kind::Border => &mut nb,
                Kind::Seam => &mut ns,
                Kind::Locked => &mut nc,
                Kind::Manifold => continue,
            };
            c[v as usize] = c[v as usize].saturating_add(1);
        }
    }
    let mut wedges: Vec<Vec<u32>> = vec![vec![]; npos];
    for t in 0..tp.len() {
        if alive[t] {
            for k in 0..3 {
                let w = &mut wedges[tp[t][k] as usize];
                if !w.contains(&tw[t][k]) {
                    w.push(tw[t][k]);
                }
            }
        }
    }
    let kind: Vec<Kind> = (0..npos)
        .map(|v| {
            let nw = wedges[v].len();
            if nw == 0 || nc[v] > 0 {
                Kind::Locked
            } else if nb[v] == 0 && ns[v] == 0 && nw == 1 {
                Kind::Manifold
            } else if nb[v] == 2 && ns[v] == 0 && nw == 1 {
                Kind::Border
            } else if ns[v] == 2 && nb[v] == 0 && nw == 2 {
                Kind::Seam
            } else {
                Kind::Locked
            }
        })
        .collect();
    // quadrics: unweighted planes, so Q(p) bounds the worst squared distance from above
    let mut q = vec![Quadric::default(); npos];
    for t in 0..tp.len() {
        if !alive[t] {
            continue;
        }
        let [a, b, c] = tp[t].map(|v| at[v as usize]);
        let n = cross(sub(b, a), sub(c, a));
        let l = len(n);
        if l < 1e-14 {
            continue;
        }
        let n = [n[0] / l, n[1] / l, n[2] / l];
        let d = -dot(n, [a[0] as f64, a[1] as f64, a[2] as f64]);
        let k = Quadric::plane(n, d, 1.0);
        for &v in &tp[t] {
            q[v as usize].add(&k);
        }
    }
    // borders and seams keep their line: a plane through the edge, perpendicular to its face
    for (&(a, b), &(_, _, _)) in &edges {
        if !matches!(kind_e[&(a, b)], Kind::Border | Kind::Seam) {
            continue;
        }
        let Some(&t) = vt[a as usize]
            .iter()
            .find(|&&t| tp[t as usize].contains(&b))
        else {
            continue;
        };
        let [x, y, z] = tp[t as usize].map(|v| at[v as usize]);
        let n = cross(sub(y, x), sub(z, x));
        let m = cross(sub(at[b as usize], at[a as usize]), n);
        let l = len(m);
        if l < 1e-14 {
            continue;
        }
        let m = [m[0] / l, m[1] / l, m[2] / l];
        let pa = at[a as usize];
        let k = Quadric::plane(m, -dot(m, [pa[0] as f64, pa[1] as f64, pa[2] as f64]), 4.0);
        q[a as usize].add(&k);
        q[b as usize].add(&k);
    }

    let tol2 = (tol as f64) * (tol as f64);
    let neighbours = |vt: &Vec<Vec<u32>>, tp: &Vec<[u32; 3]>, u: u32| -> Vec<u32> {
        let mut s: Vec<u32> = vec![];
        for &t in &vt[u as usize] {
            for &v in &tp[t as usize] {
                if v != u && !s.contains(&v) {
                    s.push(v);
                }
            }
        }
        s
    };
    let allowed = |kind_e: &HashMap<(u32, u32), Kind>, u: u32, v: u32| -> bool {
        let ku = kind[u as usize];
        if ku == Kind::Locked {
            return false;
        }
        match kind_e.get(&(u.min(v), u.max(v))) {
            None => false,
            Some(_) if ku == Kind::Manifold => true,
            Some(&ke) => ke == ku,
        }
    };
    let best = |q: &Vec<Quadric>,
                vt: &Vec<Vec<u32>>,
                tp: &Vec<[u32; 3]>,
                kind_e: &HashMap<(u32, u32), Kind>,
                u: u32|
     -> Option<(f64, u32)> {
        if kind[u as usize] == Kind::Locked || vt[u as usize].is_empty() {
            return None;
        }
        let mut b: Option<(f64, u32)> = None;
        for v in neighbours(vt, tp, u) {
            if allowed(kind_e, u, v) {
                let c = q[u as usize].eval(at[v as usize]);
                if b.is_none_or(|x| c < x.0) {
                    b = Some((c, v));
                }
            }
        }
        b
    };
    let mut ver = vec![0u32; npos];
    let mut heap = BinaryHeap::new();
    for u in 0..npos as u32 {
        if let Some((c, v)) = best(&q, &vt, &tp, &kind_e, u)
            && c <= tol2
        {
            heap.push(Cand(c, u, v, 0));
        }
    }
    while let Some(Cand(c, u, v, s)) = heap.pop() {
        let (ui, vi) = (u as usize, v as usize);
        if s != ver[ui] || vt[ui].is_empty() || vt[vi].is_empty() || !allowed(&kind_e, u, v) {
            continue;
        }
        let c2 = q[ui].eval(at[vi]);
        if (c2 - c).abs() > 1e-12 {
            if c2 <= tol2 {
                heap.push(Cand(c2, u, v, ver[ui]));
            }
            continue;
        }
        let shared: Vec<u32> = vt[ui]
            .iter()
            .copied()
            .filter(|&t| tp[t as usize].contains(&v))
            .collect();
        // link condition: u and v share only the vertices across their shared triangles
        let nu = neighbours(&vt, &tp, u);
        let nv = neighbours(&vt, &tp, v);
        if nu.iter().filter(|x| nv.contains(x)).count() > shared.len() {
            continue;
        }
        // wedge map u -> v, consistent on both sides of a seam
        let mut wmap: Vec<(u32, u32)> = vec![];
        let mut ok = true;
        for &t in &shared {
            let t = t as usize;
            let (ku, kv) = (
                tp[t].iter().position(|&x| x == u).unwrap(),
                tp[t].iter().position(|&x| x == v).unwrap(),
            );
            let (wu, wv) = (tw[t][ku], tw[t][kv]);
            match wmap.iter().find(|m| m.0 == wu) {
                Some(m) if m.1 != wv => ok = false,
                Some(_) => {}
                None => wmap.push((wu, wv)),
            }
        }
        if !ok || wedges[ui].iter().any(|w| !wmap.iter().any(|m| m.0 == *w)) {
            continue;
        }
        if let Some((n, max_turn)) = normals
            && len(sub(at[ui], at[vi])) > 3.0 * tol as f64
        {
            let turned = wmap.iter().any(|&(wu, wv)| {
                // u's side of the fan, blended: v and the other corners of u's triangles
                let mut acc = n[wv as usize].map(|x| x as f64);
                for &t in &vt[ui] {
                    let t = t as usize;
                    let k = tp[t].iter().position(|&x| x == u).unwrap();
                    if tw[t][k] != wu {
                        continue;
                    }
                    for j in [(k + 1) % 3, (k + 2) % 3] {
                        let m = n[tw[t][j] as usize];
                        acc = [
                            acc[0] + m[0] as f64,
                            acc[1] + m[1] as f64,
                            acc[2] + m[2] as f64,
                        ];
                    }
                }
                let a = n[wu as usize].map(|x| x as f64);
                dot(a, acc) < max_turn as f64 * len(a) * len(acc)
            });
            if turned {
                continue;
            }
        }
        // no flipped or collapsed triangles
        for &t in &vt[ui] {
            let t = t as usize;
            if shared.contains(&(t as u32)) {
                continue;
            }
            let mut p = tp[t].map(|x| at[x as usize]);
            let n0 = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let k = tp[t].iter().position(|&x| x == u).unwrap();
            p[k] = at[vi];
            let n1 = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let (l0, l1) = (len(n0), len(n1));
            if l1 < 1e-12 || dot(n0, n1) < 0.25 * l0 * l1 {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        // collapse
        for &t in &shared {
            alive[t as usize] = false;
            for x in tp[t as usize] {
                vt[x as usize].retain(|&y| y != t);
            }
        }
        let moved = std::mem::take(&mut vt[ui]);
        for &t in &moved {
            let t = t as usize;
            let k = tp[t].iter().position(|&x| x == u).unwrap();
            tp[t][k] = v;
            tw[t][k] = wmap
                .iter()
                .find(|m| m.0 == tw[t][k])
                .map_or(tw[t][k], |m| m.1);
            vt[vi].push(t as u32);
        }
        let qu = q[ui];
        q[vi].add(&qu);
        // u's edges now belong to v (each keeps its kind)
        for x in neighbours(&vt, &tp, v) {
            let e = (v.min(x), v.max(x));
            if !kind_e.contains_key(&e) {
                let k = kind_e
                    .get(&(u.min(x), u.max(x)))
                    .copied()
                    .unwrap_or(Kind::Manifold);
                kind_e.insert(e, k);
            }
        }
        for w in std::iter::once(v).chain(neighbours(&vt, &tp, v)) {
            ver[w as usize] += 1;
            if let Some((c, t)) = best(&q, &vt, &tp, &kind_e, w)
                && c <= tol2
            {
                heap.push(Cand(c, w, t, ver[w as usize]));
            }
        }
    }
    tw.iter()
        .zip(&alive)
        .filter(|(_, a)| **a)
        .map(|(t, _)| *t)
        .collect()
}

/// Drops the loose pieces (connected by shared positions) whose bounding box diagonal is under
/// `size`.
fn prune_pieces(tp: &[[u32; 3]], alive: &mut [bool], at: &[[f32; 3]], npos: usize, size: f32) {
    let mut parent: Vec<u32> = (0..npos as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for (t, a) in tp.iter().zip(alive.iter()) {
        if *a {
            let r = find(&mut parent, t[0]);
            for &v in &t[1..] {
                let s = find(&mut parent, v);
                parent[s as usize] = r;
            }
        }
    }
    let mut lo: HashMap<u32, ([f32; 3], [f32; 3])> = HashMap::new();
    for v in 0..npos as u32 {
        let r = find(&mut parent, v);
        let p = at[v as usize];
        let e = lo.entry(r).or_insert((p, p));
        for k in 0..3 {
            e.0[k] = e.0[k].min(p[k]);
            e.1[k] = e.1[k].max(p[k]);
        }
    }
    for (t, a) in tp.iter().zip(alive.iter_mut()) {
        if *a {
            let (l, h) = lo[&find(&mut parent, t[0])];
            let d = ((h[0] - l[0]).powi(2) + (h[1] - l[1]).powi(2) + (h[2] - l[2]).powi(2)).sqrt();
            if d < size {
                *a = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An n × n grid of quads on z = 0, with a UV seam down the middle column of vertices.
    fn grid(n: u32, seam: bool) -> (Vec<[f32; 3]>, Vec<u64>, Vec<[u32; 3]>) {
        let (mut pos, mut key, mut tris) = (vec![], vec![], vec![]);
        let half = n / 2;
        let mut idx = HashMap::new();
        for side in 0..2u64 {
            for y in 0..=n {
                for x in 0..=n {
                    if (side == 0 && x > half) || (side == 1 && x < half) {
                        continue;
                    }
                    idx.insert((side, x, y), pos.len() as u32);
                    pos.push([x as f32, y as f32, 0.0]);
                    key.push(if seam { side } else { 0 });
                }
            }
        }
        for y in 0..n {
            for x in 0..n {
                let s = (x >= half) as u64;
                let v = |xx, yy| {
                    if seam || s == 0 {
                        idx[&(s, xx, yy)]
                    } else {
                        idx.get(&(1, xx, yy))
                            .copied()
                            .unwrap_or_else(|| idx[&(0, xx, yy)])
                    }
                };
                let (a, b, c, d) = (v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1));
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        (pos, key, tris)
    }

    #[test]
    fn flat_grid_collapses_and_keeps_its_seam() {
        let (pos, key, tris) = grid(8, true);
        let out = simplify(&pos, &key, &tris, 0.001, 0.0, None);
        assert!(
            out.len() < tris.len() / 4,
            "{} of {}",
            out.len(),
            tris.len()
        );
        // both sides still meet along x = 4 and no triangle mixes the two UV islands
        for t in &out {
            let sides: Vec<u64> = t.iter().map(|&i| key[i as usize]).collect();
            assert!(sides.iter().all(|s| *s == sides[0]), "{t:?}");
        }
        let area: f32 = out
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| pos[i as usize]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
            })
            .sum();
        assert!((area - 64.0).abs() < 1e-3, "area {area}");
    }

    #[test]
    fn normal_turns_are_kept() {
        let (pos, key, tris) = grid(8, false);
        // a bump field in the normals only: neighbouring vertices turn ~30° apart
        let normals: Vec<[f32; 3]> = pos
            .iter()
            .map(|p| {
                if (p[0] as i32 + p[1] as i32) % 2 == 0 {
                    [0.0, 0.0, 1.0]
                } else {
                    [0.5, 0.0, 0.866]
                }
            })
            .collect();
        let free = simplify(&pos, &key, &tris, 0.001, 0.0, None);
        let kept = simplify(&pos, &key, &tris, 0.001, 0.0, Some((&normals, 0.95)));
        assert!(
            kept.len() > free.len() * 2,
            "{} vs {}",
            kept.len(),
            free.len()
        );
    }

    #[test]
    fn small_pieces_are_pruned() {
        let (mut pos, mut key, mut tris) = grid(4, false);
        let n = pos.len() as u32;
        pos.extend([[10.0, 0.0, 0.0], [10.01, 0.0, 0.0], [10.0, 0.01, 0.0]]);
        key.extend([0, 0, 0]);
        tris.push([n, n + 1, n + 2]);
        let kept = simplify(&pos, &key, &tris, 0.0, 0.0, None);
        let pruned = simplify(&pos, &key, &tris, 0.0, 0.1, None);
        assert_eq!(pruned.len(), kept.len() - 1);
        assert!(pruned.iter().all(|t| t.iter().all(|&i| i < n)));
    }
}
