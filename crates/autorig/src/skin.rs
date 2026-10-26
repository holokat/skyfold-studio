//! Skin weights by geodesic voxel binding (Dionne & de Lasa, 2013): the character is filled
//! with voxels (which also closes small holes and joins separate pieces such as clothes, belts
//! and eyes into one solid), and each bone's distance to every surface point is measured *through
//! the inside of the body*. A leg never pulls the other leg, an arm never pulls the hip it hangs
//! beside, and loose clothing follows the limb it wraps. Each vertex takes up to four bones,
//! weighted by how much closer they are than the rest, then the weights are smoothed along the
//! surface.
//!
//! Parts no bone can reach through the solid (a floating hat, a separate weapon) fall back to
//! straight-line distance.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use glam::{IVec3, Vec3};
use rayon::prelude::*;

use crate::humanoid::{is_weightless, primary_child};
use crate::model::{Model, Skeleton, VertexSkin};

#[derive(Clone, Copy, Debug)]
pub struct SkinOpts {
    /// voxels along the model's longest side
    pub resolution: usize,
    /// how sharply a bone's pull drops with distance (higher = stiffer limbs, crisper joints)
    pub falloff: f32,
    /// surface smoothing passes (on a game-density mesh; denser meshes get more)
    pub smooth: usize,
    /// model-space height of the chin: everything above it (bar the arms) moves with the head
    pub head_from: Option<f32>,
}

impl Default for SkinOpts {
    fn default() -> Self {
        Self {
            resolution: 192,
            falloff: 4.0,
            smooth: 2,
            head_from: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkinReport {
    pub voxels: [usize; 3],
    pub solid: usize,
    /// vertices no bone reached through the solid (weighted by straight-line distance)
    pub unreached: usize,
    pub millis: u128,
}

struct Grid {
    lo: Vec3,
    vox: f32,
    n: [usize; 3],
}

impl Grid {
    fn idx(&self, c: IVec3) -> Option<usize> {
        let [nx, ny, nz] = self.n;
        (c.x >= 0
            && c.y >= 0
            && c.z >= 0
            && (c.x as usize) < nx
            && (c.y as usize) < ny
            && (c.z as usize) < nz)
            .then(|| (c.z as usize * ny + c.y as usize) * nx + c.x as usize)
    }
    fn cell(&self, p: Vec3) -> IVec3 {
        ((p - self.lo) / self.vox).floor().as_ivec3()
    }
    fn centre(&self, c: IVec3) -> Vec3 {
        self.lo + (c.as_vec3() + 0.5) * self.vox
    }
    fn coord(&self, i: usize) -> IVec3 {
        let [nx, ny, _] = self.n;
        IVec3::new(
            (i % nx) as i32,
            ((i / nx) % ny) as i32,
            (i / (nx * ny)) as i32,
        )
    }
    fn len(&self) -> usize {
        self.n[0] * self.n[1] * self.n[2]
    }
}

/// One weighted bone: a segment from its joint to its child's (or a point).
struct Seg {
    bone: usize,
    a: Vec3,
    b: Vec3,
}

fn seg_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-12 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

fn segments(sk: &Skeleton, top_y: f32) -> Vec<Seg> {
    let w = sk.rest_world();
    let pos = |i: usize| w[i].w_axis.truncate();
    sk.bones
        .iter()
        .enumerate()
        .filter(|(_, b)| !is_weightless(&b.name))
        .map(|(i, b)| {
            let a = pos(i);
            let end = match primary_child(&b.name).and_then(|c| sk.find(c)) {
                Some(c) => pos(c),
                // the head reaches to the top of the skull
                None if b.name == "Head" => Vec3::new(a.x, top_y.max(a.y), a.z),
                None => a,
            };
            Seg { bone: i, a, b: end }
        })
        .collect()
}

/// The model filled in: a voxel grid where the inside of the body (everything the outside
/// can't reach through the surface, thickened by one voxel to seal cracks) is solid.
pub struct Solid {
    grid: Grid,
    outside: Vec<bool>,
}

impl Solid {
    /// `resolution` voxels along the model's longest side.
    pub fn new(model: &Model, resolution: usize) -> Option<Self> {
        let (lo, hi) = model.bounds()?;
    let ext = hi - lo;
    let vox = ext.max_element().max(1e-3) / resolution.max(16) as f32;
    let margin = 2.0 * vox;
    let glo = lo - Vec3::splat(margin);
    let n = ((ext + Vec3::splat(2.0 * margin)) / vox).ceil().as_uvec3() + 1;
    let g = Grid {
        lo: glo,
        vox,
        n: [n.x as usize, n.y as usize, n.z as usize],
    };
    let len = g.len();

    // 1: the surface, one voxel thicker (seals cracks and pinholes)
    let mut shell = vec![false; len];
    for m in &model.meshes {
        for tri in m.indices.chunks_exact(3) {
            let (a, b, c) = (
                m.positions[tri[0] as usize],
                m.positions[tri[1] as usize],
                m.positions[tri[2] as usize],
            );
            let longest = (b - a).length().max((c - b).length()).max((a - c).length());
            let steps = ((longest / (vox * 0.5)).ceil() as usize).clamp(1, 4096);
            for i in 0..=steps {
                for j in 0..=(steps - i) {
                    let (u, v) = (i as f32 / steps as f32, j as f32 / steps as f32);
                    let p = a + (b - a) * u + (c - a) * v;
                    if let Some(k) = g.idx(g.cell(p)) {
                        shell[k] = true;
                    }
                }
            }
        }
    }
    let surface = shell.clone();
    for k in 0..len {
        if surface[k] {
            let c = g.coord(k);
            for d in N6 {
                if let Some(j) = g.idx(c + d) {
                    shell[j] = true;
                }
            }
        }
    }
    // 2: everything the outside can't reach is solid
    let mut outside = vec![false; len];
    let mut stack: Vec<usize> = vec![];
    for k in 0..len {
        let c = g.coord(k);
        let border = c.x == 0
            || c.y == 0
            || c.z == 0
            || c.x as usize == g.n[0] - 1
            || c.y as usize == g.n[1] - 1
            || c.z as usize == g.n[2] - 1;
        if border && !shell[k] && !outside[k] {
            outside[k] = true;
            stack.push(k);
        }
    }
    while let Some(k) = stack.pop() {
        let c = g.coord(k);
        for d in N6 {
            if let Some(j) = g.idx(c + d)
                && !shell[j]
                && !outside[j]
            {
                outside[j] = true;
                stack.push(j);
            }
        }
    }
        Some(Self { grid: g, outside })
    }

    /// The solid stretches along z through the front-view point (x, y): (z0, z1) each.
    pub fn z_runs(&self, x: f32, y: f32) -> Vec<(f32, f32)> {
        let g = &self.grid;
        let c = g.cell(Vec3::new(x, y, g.lo.z));
        let mut out = vec![];
        let mut start: Option<i32> = None;
        for k in 0..=g.n[2] as i32 {
            let on = k < g.n[2] as i32
                && g
                    .idx(IVec3::new(c.x, c.y, k))
                    .is_some_and(|i| !self.outside[i]);
            match (on, start) {
                (true, None) => start = Some(k),
                (false, Some(s0)) => {
                    out.push((
                        g.lo.z + s0 as f32 * g.vox,
                        g.lo.z + k as f32 * g.vox,
                    ));
                    start = None;
                }
                _ => {}
            }
        }
        out
    }

    /// The body's depth (back, front) at the front-view point (x, y): the solid stretch nearest
    /// `z_ref`. A tail, cape or shield apart from the body is a stretch of its own.
    pub fn depth_near(&self, x: f32, y: f32, z_ref: f32) -> Option<(f32, f32)> {
        let v = self.grid.vox;
        // (a thin limb can fall between columns: look one voxel round too)
        let mut runs = self.z_runs(x, y);
        if runs.is_empty() {
            for (dx, dy) in [(v, 0.0), (-v, 0.0), (0.0, v), (0.0, -v)] {
                runs = self.z_runs(x + dx, y + dy);
                if !runs.is_empty() {
                    break;
                }
            }
        }
        runs.into_iter()
            .min_by(|a, b| {
                let d = |r: &(f32, f32)| {
                    if z_ref >= r.0 && z_ref <= r.1 { 0.0 } else { (r.0 - z_ref).abs().min((r.1 - z_ref).abs()) }
                };
                d(a).total_cmp(&d(b))
            })
            // (the surface was thickened by a voxel on each side)
            .map(|(a, b)| if b - a > 3.0 * v { (a + v, b - v) } else { (a, b) })
    }
    pub fn voxel(&self) -> f32 {
        self.grid.vox
    }
}

/// Computes skin weights for every mesh of `model` against `sk` (the fitted skeleton, in its
/// rest pose, which is the pose the meshes are in).
pub fn skin(model: &mut Model, sk: &Skeleton, opts: SkinOpts) -> SkinReport {
    let t0 = std::time::Instant::now();
    let Some((lo, hi)) = model.bounds() else {
        return SkinReport::default();
    };
    let ext = hi - lo;
    let Some(Solid { grid: g, outside }) = Solid::new(model, opts.resolution) else {
        return SkinReport::default();
    };
    let vox = g.vox;
    let len = g.len();
    // compact solid indices
    let mut solid_of = vec![u32::MAX; len];
    let mut solid: Vec<u32> = vec![];
    for k in 0..len {
        if !outside[k] {
            solid_of[k] = solid.len() as u32;
            solid.push(k as u32);
        }
    }
    drop(outside);

    // 3: each vertex's voxel (the nearest solid one)
    let near_solid = |p: Vec3| -> Option<(u32, f32)> {
        let c = g.cell(p);
        let mut best: Option<(u32, f32)> = None;
        for r in 0..=3i32 {
            for dz in -r..=r {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        let cc = c + IVec3::new(dx, dy, dz);
                        if let Some(k) = g.idx(cc)
                            && solid_of[k] != u32::MAX
                        {
                            let d = (g.centre(cc) - p).length();
                            if best.is_none_or(|(_, bd)| d < bd) {
                                best = Some((solid_of[k], d));
                            }
                        }
                    }
                }
            }
            if best.is_some() {
                break;
            }
        }
        best
    };
    // the solid voxels round each vertex, trilinearly weighted (a vertex reads a blend of its
    // neighbouring voxels, not just the nearest one: weights then vary smoothly inside a voxel
    // instead of in voxel-sized steps, which a dense mesh's few smoothing passes can't hide)
    let corners = |p: Vec3| -> Vec<(u32, f32)> {
        let f = (p - g.lo) / vox - Vec3::splat(0.5);
        let base = f.floor().as_ivec3();
        let t = f - base.as_vec3();
        let mut out = Vec::with_capacity(8);
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    let c = base + IVec3::new(dx, dy, dz);
                    let w = (if dx == 1 { t.x } else { 1.0 - t.x })
                        * (if dy == 1 { t.y } else { 1.0 - t.y })
                        * (if dz == 1 { t.z } else { 1.0 - t.z });
                    if w > 1e-4
                        && let Some(k) = g.idx(c)
                        && solid_of[k] != u32::MAX
                    {
                        out.push((solid_of[k], w));
                    }
                }
            }
        }
        out
    };
    // per vertex: its corners (when any is solid), else the nearest solid voxel and how far off
    let vert_vox: Vec<Vec<(Vec<(u32, f32)>, Option<(u32, f32)>)>> = model
        .meshes
        .par_iter()
        .map(|m| {
            m.positions
                .iter()
                .map(|p| {
                    let c = corners(*p);
                    if c.is_empty() { (c, near_solid(*p)) } else { (c, None) }
                })
                .collect()
        })
        .collect();
    // the solid voxels any vertex uses, and their slot in the per-bone tables
    let mut slot: HashMap<u32, u32> = HashMap::new();
    for vv in &vert_vox {
        for (c, n) in vv {
            for (s, _) in c.iter().chain(n.iter()) {
                let next = slot.len() as u32;
                slot.entry(*s).or_insert(next);
            }
        }
    }
    let slots: Vec<u32> = {
        let mut v = vec![0u32; slot.len()];
        for (s, i) in &slot {
            v[*i as usize] = *s;
        }
        v
    };

    // 4: geodesic distance from every bone, through the solid
    let segs = segments(sk, hi.y);
    // (far enough for a tail, a cape or a long skirt to reach its bone)
    let max_d = 2.0 * ext.y.max(ext.x).max(ext.z);
    let dists: Vec<Vec<f32>> = segs
        .par_iter()
        .map(|s| {
            let mut seeds = vec![];
            let len = (s.b - s.a).length();
            let steps = ((len / (vox * 0.5)).ceil() as usize).max(1);
            for i in 0..=steps {
                let p = s.a.lerp(s.b, i as f32 / steps as f32);
                if let Some((k, d)) = near_solid(p) {
                    seeds.push((k, d));
                }
            }
            let field = dijkstra(&g, &solid, &solid_of, &seeds, max_d);
            slots.iter().map(|&k| field[k as usize]).collect()
        })
        .collect();

    // 5: weights per vertex
    let mut unreached = 0;
    let mut reached: Vec<Vec<bool>> = vec![];
    for (mi, m) in model.meshes.iter_mut().enumerate() {
        let mut out = Vec::with_capacity(m.positions.len());
        let mut ok = Vec::with_capacity(m.positions.len());
        for vi in 0..m.positions.len() {
            let (cs, near) = &vert_vox[mi][vi];
            let mut d: Vec<(usize, f32)> = vec![];
            if !cs.is_empty() {
                // a bone counts where every solid corner reaches it (else that corner's ∞ wins)
                for bi in 0..segs.len() {
                    let (mut acc, mut wsum, mut all) = (0.0, 0.0, true);
                    for &(k, w) in cs {
                        let x = dists[bi][slot[&k] as usize];
                        if !x.is_finite() {
                            all = false;
                            break;
                        }
                        acc += x * w;
                        wsum += w;
                    }
                    if all && wsum > 0.0 {
                        d.push((bi, acc / wsum));
                    }
                }
            } else if let Some((k, off)) = near {
                let si = slot[k] as usize;
                d = (0..segs.len())
                    .filter_map(|bi| {
                        let x = dists[bi][si];
                        x.is_finite().then_some((bi, x + off))
                    })
                    .collect();
            }
            ok.push(!d.is_empty());
            if d.is_empty() {
                unreached += 1;
                // (filled in from the nearest reached vertex below; this is a placeholder)
                d = segs
                    .iter()
                    .enumerate()
                    .map(|(bi, s)| (bi, seg_dist(m.positions[vi], s.a, s.b)))
                    .collect();
            }
            out.push(weigh(&mut d, &segs, vox, opts.falloff));
        }
        m.skin = Some(out);
        reached.push(ok);
    }
    // parts outside the solid (fur tips, loose strands, a floating piece): the weights of the
    // nearest vertex that was reached, so they move with the body under them instead of being
    // pulled toward whichever bone happens to be closest in a straight line
    if unreached > 0 {
        borrow_nearest(model, &reached, vox * 2.0);
    }

    // 6: smoothing along the (welded) surface
    // (passes in proportion to how many edges span a voxel: a dense scan needs more of them to
    // blur the same distance as a game mesh)
    if opts.smooth > 0 {
        let edge = median_edge(model).max(1e-5);
        let span = (vox / edge).max(1.0);
        let passes = ((opts.smooth as f32 * span * span * 0.5).round() as usize).clamp(opts.smooth, 48);
        smooth(model, vox * 0.25, passes);
    }
    // 7: a rigid skull: above the chin the head bone takes over (faded in over a short band, so
    // the neck still bends smoothly). Geodesic distances alone share a big head with the neck
    // and shoulders, and lifting an arm then squashes it (chibi heads sit right beside them).
    if let Some(chin) = opts.head_from
        && let Some(head) = sk.find("Head")
    {
        rigid_head(model, sk, head, chin, ext.y);
    }
    SkinReport {
        voxels: g.n,
        solid: solid.len(),
        unreached,
        millis: t0.elapsed().as_millis(),
    }
}

fn rigid_head(model: &mut Model, sk: &Skeleton, head: usize, chin: f32, height: f32) {
    let (lo, hi) = (chin - 0.015 * height, chin + 0.035 * height);
    // (an arm that happens to sit above the chin keeps its own weights)
    let arm: Vec<bool> = sk
        .bones
        .iter()
        .map(|b| {
            let n = b.name.to_ascii_lowercase();
            ["upperarm", "lowerarm", "hand", "thumb", "index", "middle", "ring", "pinky"]
                .iter()
                .any(|k| n.contains(k))
        })
        .collect();
    for m in &mut model.meshes {
        let positions = &m.positions;
        let Some(skin) = &mut m.skin else {
            continue;
        };
        for (p, s) in positions.iter().zip(skin.iter_mut()) {
            let t = ((p.y - lo) / (hi - lo)).clamp(0.0, 1.0);
            let f = t * t * (3.0 - 2.0 * t);
            if f <= 0.0 {
                continue;
            }
            let arm_w: f32 = s
                .joints
                .iter()
                .zip(s.weights)
                .filter(|(j, _)| arm.get(**j as usize).copied().unwrap_or(false))
                .map(|(_, w)| w)
                .sum();
            if arm_w > 0.5 {
                continue;
            }
            let mut v: Vec<(u16, f32)> = vec![];
            for k in 0..4 {
                if s.weights[k] > 0.0 {
                    add(&mut v, s.joints[k], s.weights[k] * (1.0 - f));
                }
            }
            add(&mut v, head as u16, f);
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            v.truncate(4);
            let sum: f32 = v.iter().map(|x| x.1).sum();
            *s = VertexSkin::default();
            for (k, (b, x)) in v.iter().enumerate() {
                s.joints[k] = *b;
                s.weights[k] = x / sum.max(1e-12);
            }
            tidy(s);
        }
    }
}

/// The median triangle edge length (sampled).
fn median_edge(model: &Model) -> f32 {
    let mut e: Vec<f32> = vec![];
    for m in &model.meshes {
        let step = (m.indices.len() / 3 / 20_000).max(1);
        for t in m.indices.chunks_exact(3).step_by(step) {
            e.push(m.positions[t[0] as usize].distance(m.positions[t[1] as usize]));
        }
    }
    if e.is_empty() {
        return 0.0;
    }
    e.sort_by(f32::total_cmp);
    e[e.len() / 2]
}

/// Gives every vertex not `reached` the weights of the nearest reached vertex (bucketed on a
/// grid of `cell`, searching outward).
fn borrow_nearest(model: &mut Model, reached: &[Vec<bool>], cell: f32) {
    let cell = cell.max(1e-4);
    let mut grid: HashMap<IVec3, Vec<(Vec3, VertexSkin)>> = HashMap::new();
    for (mi, m) in model.meshes.iter().enumerate() {
        let Some(skin) = &m.skin else {
            continue;
        };
        for (vi, p) in m.positions.iter().enumerate() {
            if reached[mi][vi] {
                grid.entry((*p / cell).floor().as_ivec3()).or_default().push((*p, skin[vi]));
            }
        }
    }
    if grid.is_empty() {
        return;
    }
    for (mi, m) in model.meshes.iter_mut().enumerate() {
        let positions = &m.positions;
        let Some(skin) = &mut m.skin else {
            continue;
        };
        let fixed: Vec<(usize, VertexSkin)> = (0..positions.len())
            .into_par_iter()
            .filter(|&vi| !reached[mi][vi])
            .filter_map(|vi| {
                let p = positions[vi];
                let c = (p / cell).floor().as_ivec3();
                for r in 0..=24i32 {
                    let mut best: Option<(f32, VertexSkin)> = None;
                    for dz in -r..=r {
                        for dy in -r..=r {
                            for dx in -r..=r {
                                if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                                    continue;
                                }
                                if let Some(list) = grid.get(&(c + IVec3::new(dx, dy, dz))) {
                                    for (q, s) in list {
                                        let d = q.distance_squared(p);
                                        if best.is_none_or(|b| d < b.0) {
                                            best = Some((d, *s));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // (one ring further can still hold a closer point than a corner hit)
                    if let Some((_, s)) = best {
                        return Some((vi, s));
                    }
                }
                None
            })
            .collect();
        for (vi, s) in fixed {
            skin[vi] = s;
        }
    }
}

const N6: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

/// Distances (metres) from the seeds to every solid voxel (∞ beyond `max_d`), 26-connected.
fn dijkstra(g: &Grid, solid: &[u32], solid_of: &[u32], seeds: &[(u32, f32)], max_d: f32) -> Vec<f32> {
    let mut dist = vec![f32::INFINITY; solid.len()];
    let mut heap: BinaryHeap<Reverse<(u32, u32)>> = BinaryHeap::new();
    for &(k, d) in seeds {
        let k = k as usize;
        if d < dist[k] {
            dist[k] = d;
            heap.push(Reverse((d.to_bits(), k as u32)));
        }
    }
    let mut nb: Vec<(IVec3, f32)> = Vec::with_capacity(26);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy, dz) != (0, 0, 0) {
                    let o = IVec3::new(dx, dy, dz);
                    nb.push((o, o.as_vec3().length() * g.vox));
                }
            }
        }
    }
    while let Some(Reverse((bits, k))) = heap.pop() {
        let d = f32::from_bits(bits);
        let k = k as usize;
        if d > dist[k] || d > max_d {
            continue;
        }
        let c = g.coord(solid[k] as usize);
        for (o, w) in &nb {
            if let Some(j) = g.idx(c + *o) {
                let s = solid_of[j];
                if s == u32::MAX {
                    continue;
                }
                let nd = d + w;
                if nd < dist[s as usize] {
                    dist[s as usize] = nd;
                    heap.push(Reverse((nd.to_bits(), s)));
                }
            }
        }
    }
    dist
}

/// Up to four bones from distances: each bone's pull is (nearest / its distance)^falloff.
fn weigh(d: &mut [(usize, f32)], segs: &[Seg], vox: f32, falloff: f32) -> VertexSkin {
    let floor = vox * 0.5;
    let dmin = d.iter().map(|x| x.1).fold(f32::MAX, f32::min).max(floor);
    let mut w: Vec<(usize, f32)> = d
        .iter()
        .map(|&(b, x)| (b, (dmin / x.max(floor)).powf(falloff)))
        .collect();
    w.sort_by(|a, b| b.1.total_cmp(&a.1));
    w.truncate(4);
    let sum: f32 = w.iter().map(|x| x.1).sum();
    let mut out = VertexSkin::default();
    for (i, (b, x)) in w.iter().enumerate() {
        out.joints[i] = segs[*b].bone as u16;
        out.weights[i] = x / sum.max(1e-12);
    }
    tidy(&mut out);
    out
}

/// Drops weights under 2 % and renormalises (the strongest bone first).
fn tidy(s: &mut VertexSkin) {
    let mut pairs: Vec<(u16, f32)> = (0..4).map(|i| (s.joints[i], s.weights[i])).collect();
    pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f32 = pairs.iter().filter(|p| p.1 >= 0.02).map(|p| p.1).sum();
    let total = if total > 0.0 { total } else { 1.0 };
    for i in 0..4 {
        let (j, w) = pairs[i];
        if w >= 0.02 || i == 0 {
            s.joints[i] = j;
            s.weights[i] = if i == 0 && w < 0.02 { 1.0 } else { w / total };
        } else {
            s.joints[i] = pairs[0].0;
            s.weights[i] = 0.0;
        }
    }
    let sum: f32 = s.weights.iter().sum();
    for w in &mut s.weights {
        *w /= sum.max(1e-12);
    }
}

/// Averages each vertex's weights with its surface neighbours (`passes` times); vertices at the
/// same place (UV seams, split normals, separate pieces touching) count as one.
fn smooth(model: &mut Model, weld: f32, passes: usize) {
    let mut ids: HashMap<IVec3, u32> = HashMap::new();
    let mut welded: Vec<Vec<u32>> = vec![];
    for m in &model.meshes {
        welded.push(
            m.positions
                .iter()
                .map(|p| {
                    let key = (*p / weld.max(1e-6)).round().as_ivec3();
                    let next = ids.len() as u32;
                    *ids.entry(key).or_insert(next)
                })
                .collect(),
        );
    }
    let n = ids.len();
    let mut adj: Vec<Vec<u32>> = vec![vec![]; n];
    for (mi, m) in model.meshes.iter().enumerate() {
        for t in m.indices.chunks_exact(3) {
            let v = [
                welded[mi][t[0] as usize],
                welded[mi][t[1] as usize],
                welded[mi][t[2] as usize],
            ];
            for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
                if a != b {
                    adj[a as usize].push(b);
                    adj[b as usize].push(a);
                }
            }
        }
    }
    for a in &mut adj {
        a.sort_unstable();
        a.dedup();
    }
    // per welded vertex: the mean of its copies' weights
    let mut cur: Vec<Vec<(u16, f32)>> = vec![vec![]; n];
    let mut copies = vec![0u32; n];
    for (mi, m) in model.meshes.iter().enumerate() {
        if let Some(skin) = &m.skin {
            for (vi, s) in skin.iter().enumerate() {
                let w = welded[mi][vi] as usize;
                copies[w] += 1;
                for k in 0..4 {
                    if s.weights[k] > 0.0 {
                        add(&mut cur[w], s.joints[k], s.weights[k]);
                    }
                }
            }
        }
    }
    for (w, c) in cur.iter_mut().zip(&copies) {
        for x in w.iter_mut() {
            x.1 /= (*c).max(1) as f32;
        }
    }
    for _ in 0..passes {
        let next: Vec<Vec<(u16, f32)>> = (0..n)
            .into_par_iter()
            .map(|i| {
                if adj[i].is_empty() {
                    return cur[i].clone();
                }
                let mut acc: Vec<(u16, f32)> = cur[i].iter().map(|x| (x.0, x.1 * 0.5)).collect();
                let k = 0.5 / adj[i].len() as f32;
                for &j in &adj[i] {
                    for &(b, w) in &cur[j as usize] {
                        add(&mut acc, b, w * k);
                    }
                }
                acc
            })
            .collect();
        cur = next;
    }
    for (mi, m) in model.meshes.iter_mut().enumerate() {
        let Some(skin) = &mut m.skin else {
            continue;
        };
        for (vi, s) in skin.iter_mut().enumerate() {
            let mut w = cur[welded[mi][vi] as usize].clone();
            w.sort_by(|a, b| b.1.total_cmp(&a.1));
            w.truncate(4);
            let sum: f32 = w.iter().map(|x| x.1).sum();
            if sum <= 0.0 {
                continue;
            }
            *s = VertexSkin::default();
            for (k, (b, x)) in w.iter().enumerate() {
                s.joints[k] = *b;
                s.weights[k] = x / sum;
            }
            tidy(s);
        }
    }
}

fn add(v: &mut Vec<(u16, f32)>, b: u16, w: f32) {
    match v.iter_mut().find(|x| x.0 == b) {
        Some(x) => x.1 += w,
        None => v.push((b, w)),
    }
}
