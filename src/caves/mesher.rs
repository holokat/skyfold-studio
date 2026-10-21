//! Meshing the cave volume: surface nets on a 0.4 m grid in 12.8 m chunks, built in parallel
//! (the vast rock of the giant caves on a 0.8 m grid in 25.6 m chunks: `ChunkKey::w` is the
//! level, the two never overlap). Chunks far from any surface are passed over after one sample,
//! and inside a chunk whole 4×4×4 blocks well inside rock or air are filled in from their
//! corners, so only the voxels near a surface are evaluated.
//!
//! - Only what differs from the terrain is kept: carved walls, added rock, formations, plus the
//!   copy of the ground over the terrain cells taken out round a cave mouth (and one cell more,
//!   tucked `SINK` under the terrain, so no crack shows where the two meet).
//! - Each vertex carries its lighting, baked: `colour.r` how much daylight reaches it (light
//!   flooded in from the open air through the carved space, fading with every metre and corner),
//!   `colour.g` ambient occlusion from the field (crevices dark), `colour.b` how damp it is,
//!   `colour.a` how much it is the outside ground (1) rather than cave rock (0);
//!   `uv` = (theme slot, darkness fill of its cave).
//! - Chunks are keyed by position and fingerprinted (shapes, ground, holes, light), so an edit
//!   rebuilds only the chunks it changed.

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use bevy::math::{IVec2, IVec3, IVec4, UVec3, Vec3};
use rayon::prelude::*;

use super::sdf::{NONE, Volume};

pub const VOXEL: f32 = 0.4;
pub const CHUNK: i32 = 32;
pub const CHUNK_M: f32 = VOXEL * CHUNK as f32;

/// A chunk: its position in chunks (x, y, z) of its level (w: 0 the 0.4 m grid, 1 the 0.8 m
/// grid of the giant caves).
pub type ChunkKey = IVec4;

/// The voxel size and the chunk size (m) of a level.
pub fn level_size(level: i32) -> (f32, f32) {
    let v = VOXEL * (1 << level) as f32;
    (v, v * CHUNK as f32)
}

/// The world-space corner of a chunk.
pub fn chunk_origin(key: ChunkKey) -> Vec3 {
    key.truncate().as_vec3() * level_size(key.w).1
}

/// One chunk's mesh, ready for the GPU.
#[derive(Clone, Debug, Default)]
pub struct ChunkMesh {
    pub key: ChunkKey,
    pub hash: u64,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub lo: Vec3,
    pub hi: Vec3,
    /// the cave most of it belongs to (index in `Caves::list`)
    pub owner: u16,
}

// ------------------------------------------------------------------ daylight

/// How much daylight reaches the carved space: one grid per cave (caves far apart don't share a
/// coarse one), and the terrain cells over carved space under the ground (x-ray opens them).
pub struct Light {
    pub grids: Vec<LightGrid>,
    pub footprint: HashSet<IVec2>,
}

impl Light {
    pub fn empty() -> Light {
        Light {
            grids: vec![],
            footprint: HashSet::new(),
        }
    }

    /// Floods daylight from the open air into each cave's carved space.
    pub fn build(vol: &Volume) -> Light {
        // the shapes of each cave (dug strokes: on their own)
        let mut groups: HashMap<u16, (Vec3, Vec3)> = HashMap::new();
        for s in &vol.shapes {
            let e = groups.entry(s.owner).or_insert((s.lo, s.hi));
            e.0 = e.0.min(s.lo);
            e.1 = e.1.max(s.hi);
        }
        let mut keys: Vec<u16> = groups.keys().copied().collect();
        keys.sort();
        let mut out = Light::empty();
        for k in keys {
            let (lo, hi) = groups[&k];
            let g = LightGrid::build(vol, lo - Vec3::splat(4.0), hi + Vec3::splat(4.0));
            out.footprint.extend(g.footprint.iter().copied());
            out.grids.push(g);
        }
        out
    }

    /// Daylight at a point in the air (the brightest of the grids it falls in; 1 outside them).
    pub fn at(&self, p: Vec3) -> f32 {
        let mut best: Option<f32> = None;
        for g in &self.grids {
            if let Some(v) = g.at(p) {
                best = Some(best.map_or(v, |b| b.max(v)));
            }
        }
        best.unwrap_or(1.0)
    }

    fn hash_box(&self, lo: Vec3, hi: Vec3, h: &mut impl Hasher) {
        for g in &self.grids {
            g.hash_box(lo, hi, h);
        }
    }
}

/// Daylight in each 2 m cell of one box (0..1).
pub struct LightGrid {
    pub lo: Vec3,
    pub n: UVec3,
    pub step: f32,
    pub v: Vec<f32>,
    /// solid cells
    pub solid: Vec<bool>,
    pub footprint: HashSet<IVec2>,
}

#[derive(PartialEq, PartialOrd)]
struct Item(f32, u32);
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&o.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl LightGrid {
    /// Floods daylight from the open air into the carved space within `lo`..`hi`.
    pub fn build(vol: &Volume, lo: Vec3, hi: Vec3) -> LightGrid {
        let ext = hi - lo;
        let mut step = 2.0f32;
        while (ext.x / step) * (ext.y / step) * (ext.z / step) > 1.5e6 {
            step *= 1.25;
        }
        let n = UVec3::new(
            (ext.x / step).ceil() as u32 + 1,
            (ext.y / step).ceil() as u32 + 1,
            (ext.z / step).ceil() as u32 + 1,
        );
        let total = (n.x * n.y * n.z) as usize;
        let idx_of = |x: u32, y: u32, z: u32| ((z * n.y + y) * n.x + x) as usize;
        // sample the field at every cell centre (rows in parallel)
        let rows: Vec<(Vec<bool>, Vec<bool>, Vec<IVec2>)> = (0..n.z * n.y)
            .into_par_iter()
            .map(|r| {
                let (z, y) = (r / n.y, r % n.y);
                let p0 = lo + Vec3::new(0.0, y as f32, z as f32) * step;
                let idx = vol.shapes_near(p0, p0 + Vec3::new(ext.x, 0.0, 0.0), step);
                let mut solid = Vec::with_capacity(n.x as usize);
                let mut sky = Vec::with_capacity(n.x as usize);
                let mut under = vec![];
                for x in 0..n.x {
                    let p = p0 + Vec3::X * (x as f32 * step);
                    let s = vol.eval(p, &idx);
                    solid.push(s.d < 0.0);
                    sky.push(s.d > 0.0 && s.ground > 1.0);
                    if s.d > 0.0 && s.ground < -0.3 {
                        under.push(vol.ground.cell_of(p.x, p.z));
                    }
                }
                (solid, sky, under)
            })
            .collect();
        let mut solid = Vec::with_capacity(total);
        let mut v = vec![0.0f32; total];
        let mut heap = BinaryHeap::new();
        let mut footprint = HashSet::new();
        for (r, (srow, skyrow, under)) in rows.into_iter().enumerate() {
            footprint.extend(under);
            for (x, s) in skyrow.iter().enumerate() {
                if *s {
                    let i = r * n.x as usize + x;
                    v[i] = 1.0;
                    heap.push(Item(1.0, i as u32));
                }
            }
            solid.extend(srow);
        }
        // spread: each step into the dark loses a share (falling down a shaft loses less)
        while let Some(Item(val, i)) = heap.pop() {
            if val < v[i as usize] - 1e-6 || val < 0.004 {
                continue;
            }
            let i = i as u32;
            let (x, y, z) = (i % n.x, (i / n.x) % n.y, i / (n.x * n.y));
            let k = step / 2.0;
            let side = 0.8f32.powf(k);
            let down = 0.9f32.powf(k);
            let up = 0.74f32.powf(k);
            for (dx, dy, dz, f) in [
                (1i32, 0i32, 0i32, side),
                (-1, 0, 0, side),
                (0, 0, 1, side),
                (0, 0, -1, side),
                (0, -1, 0, down),
                (0, 1, 0, up),
            ] {
                let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                if nx < 0
                    || ny < 0
                    || nz < 0
                    || nx >= n.x as i32
                    || ny >= n.y as i32
                    || nz >= n.z as i32
                {
                    continue;
                }
                let j = idx_of(nx as u32, ny as u32, nz as u32);
                if solid[j] {
                    continue;
                }
                let nv = val * f;
                if nv > v[j] + 1e-4 {
                    v[j] = nv;
                    heap.push(Item(nv, j as u32));
                }
            }
        }
        LightGrid {
            lo,
            n,
            step,
            v,
            solid,
            footprint,
        }
    }

    /// Daylight at a point in the air (averaged over the surrounding air cells; None outside the
    /// grid).
    pub fn at(&self, p: Vec3) -> Option<f32> {
        if self.v.is_empty() {
            return None;
        }
        let g = (p - self.lo) / self.step;
        if g.x < 0.0
            || g.y < 0.0
            || g.z < 0.0
            || g.x > (self.n.x - 1) as f32
            || g.y > (self.n.y - 1) as f32
            || g.z > (self.n.z - 1) as f32
        {
            return None;
        }
        let b = g.floor();
        let f = g - b;
        let (bx, by, bz) = (b.x as u32, b.y as u32, b.z as u32);
        let mut sum = 0.0;
        let mut wsum = 0.0;
        for dz in 0..2u32 {
            for dy in 0..2u32 {
                for dx in 0..2u32 {
                    let (x, y, z) = (
                        (bx + dx).min(self.n.x - 1),
                        (by + dy).min(self.n.y - 1),
                        (bz + dz).min(self.n.z - 1),
                    );
                    let i = ((z * self.n.y + y) * self.n.x + x) as usize;
                    if self.solid[i] {
                        continue;
                    }
                    let w = (if dx == 0 { 1.0 - f.x } else { f.x })
                        * (if dy == 0 { 1.0 - f.y } else { f.y })
                        * (if dz == 0 { 1.0 - f.z } else { f.z });
                    sum += self.v[i] * w;
                    wsum += w;
                }
            }
        }
        Some(if wsum < 1e-4 { 0.0 } else { sum / wsum })
    }

    /// Fingerprint of the light over a box (chunk cache keys).
    fn hash_box(&self, lo: Vec3, hi: Vec3, h: &mut impl Hasher) {
        if self.v.is_empty() {
            return;
        }
        let top = self.lo + self.n.as_vec3() * self.step;
        if hi.x < self.lo.x
            || hi.y < self.lo.y
            || hi.z < self.lo.z
            || lo.x > top.x
            || lo.y > top.y
            || lo.z > top.z
        {
            return;
        }
        let a = ((lo - self.lo) / self.step).floor().max(Vec3::ZERO);
        let b = ((hi - self.lo) / self.step).ceil();
        let m = self.n.as_vec3() - 1.0;
        let (a, b) = (a.min(m), b.min(m).max(Vec3::ZERO));
        for z in a.z as u32..=b.z as u32 {
            for y in a.y as u32..=b.y as u32 {
                for x in a.x as u32..=b.x as u32 {
                    let i = ((z * self.n.y + y) * self.n.x + x) as usize;
                    ((self.v[i] * 64.0) as u32).hash(h);
                }
            }
        }
    }
}

// ------------------------------------------------------------------ chunks

/// The chunks that can hold cave surfaces, with the shapes near each: 0.4 m chunks, except
/// where only giant caves' rock is near (whole 25.6 m cells of it), which mesh at 0.8 m.
pub fn chunks(vol: &Volume) -> Vec<(ChunkKey, Vec<u32>)> {
    let mut keys: HashSet<IVec3> = HashSet::new();
    let to_key = |p: Vec3| (p / CHUNK_M).floor().as_ivec3();
    for s in &vol.shapes {
        let (a, b) = (
            to_key(s.lo - Vec3::splat(VOXEL * 2.0)),
            to_key(s.hi + Vec3::splat(VOXEL * 2.0)),
        );
        for z in a.z..=b.z {
            for y in a.y..=b.y {
                for x in a.x..=b.x {
                    keys.insert(IVec3::new(x, y, z));
                }
            }
        }
    }
    // the ground over (and round) the holes
    let cell = vol.ground.cell();
    for c in &vol.holes {
        for dj in -1..=1 {
            for di in -1..=1 {
                let o = vol.ground.cell_corner(*c + IVec2::new(di, dj));
                for (x, z) in [
                    (o.x, o.y),
                    (o.x + cell, o.y),
                    (o.x, o.y + cell),
                    (o.x + cell, o.y + cell),
                ] {
                    let y = vol.ground.h(x, z);
                    keys.insert(to_key(Vec3::new(x, y, z)));
                    keys.insert(to_key(Vec3::new(x, y - 1.0, z)));
                }
            }
        }
    }
    // the giant caves' cells: a coarse chunk for the whole cell where nothing else is near
    let coarse_m = level_size(1).1;
    let mut cells: HashMap<IVec3, bool> = HashMap::new();
    if vol.giant.iter().any(|g| *g) {
        for k in &keys {
            let c = k.div_euclid(IVec3::splat(2));
            cells.entry(c).or_insert_with(|| {
                let lo = c.as_vec3() * coarse_m;
                let near = vol.shapes_near(lo, lo + Vec3::splat(coarse_m), VOXEL * 3.0);
                let owners = near
                    .iter()
                    .map(|&i| vol.shapes[i as usize].owner)
                    .filter(|o| *o != NONE);
                let mut any = false;
                let mut all = true;
                for o in owners {
                    let g = vol.giant.get(o as usize).copied().unwrap_or(false);
                    any |= g;
                    all &= g;
                }
                any && all
            });
        }
    }
    let mut list: HashSet<ChunkKey> = HashSet::new();
    for k in keys {
        let c = k.div_euclid(IVec3::splat(2));
        if cells.get(&c).copied().unwrap_or(false) {
            list.insert(c.extend(1));
        } else {
            list.insert(k.extend(0));
        }
    }
    let mut out: Vec<(ChunkKey, Vec<u32>)> = list
        .into_iter()
        .map(|k| {
            let (v, m) = level_size(k.w);
            let lo = chunk_origin(k);
            (k, vol.shapes_near(lo, lo + Vec3::splat(m), v * 3.0))
        })
        .collect();
    out.sort_by_key(|(k, _)| (k.w, k.x, k.y, k.z));
    out
}

/// The cells where the copy of the ground is drawn: the holes and one cell round them.
pub fn ground_cells(vol: &Volume) -> HashSet<IVec2> {
    let mut out = HashSet::new();
    for c in &vol.holes {
        for dj in -1..=1 {
            for di in -1..=1 {
                out.insert(*c + IVec2::new(di, dj));
            }
        }
    }
    out
}

/// Fingerprint of everything a chunk's mesh depends on.
pub fn chunk_hash(
    vol: &Volume,
    key: ChunkKey,
    idx: &[u32],
    keep: &HashSet<IVec2>,
    light: &Light,
    fills: &[f32],
) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    for &i in idx {
        let s = &vol.shapes[i as usize];
        format!("{:?}{:?}{}{}{}", s.prim, s.op, s.rough, s.floor, s.owner).hash(&mut h);
        if (s.owner as usize) < vol.themes.len() {
            vol.themes[s.owner as usize].slot().hash(&mut h);
            fills
                .get(s.owner as usize)
                .map(|f| f.to_bits())
                .hash(&mut h);
        }
    }
    let lo = chunk_origin(key);
    let hi = lo + Vec3::splat(level_size(key.w).1);
    let c0 = vol.ground.cell_of(lo.x, lo.z);
    let c1 = vol.ground.cell_of(hi.x, hi.z);
    vol.ground.hash_rect(c0, c1).hash(&mut h);
    for j in c0.y..=c1.y {
        for i in c0.x..=c1.x {
            let c = IVec2::new(i, j);
            (vol.holes.contains(&c), keep.contains(&c)).hash(&mut h);
        }
    }
    light.hash_box(lo - Vec3::splat(2.0), hi + Vec3::splat(2.0), &mut h);
    h.finish()
}

const EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// Meshes one chunk (None when nothing in it is drawn).
/// How far (m) the field can be from the true distance (the rock's noise, at most about
/// `sdf::NOISE_REACH` either way) and how steeply it can change (per metre): a sample this far
/// plus the steepness times the span from a surface has none within that span.
const SKIP_SLACK: f32 = 5.4;
const SKIP_STEEP: f32 = 1.35;

pub fn mesh_chunk(
    vol: &Volume,
    key: ChunkKey,
    idx: &[u32],
    keep: &HashSet<IVec2>,
    light: &Light,
    fills: &[f32],
    hash: u64,
) -> Option<ChunkMesh> {
    let (voxel, chunk_m) = level_size(key.w);
    let n = (CHUNK + 2) as usize;
    let origin = chunk_origin(key);
    let at = |i: usize, j: usize, k: usize| {
        origin + Vec3::new(i as f32 - 1.0, j as f32 - 1.0, k as f32 - 1.0) * voxel
    };
    let id = |i: usize, j: usize, k: usize| (k * n + j) * n + i;
    // a chunk far from any surface: nothing to draw
    let centre = origin + Vec3::splat(chunk_m * 0.5);
    if vol.eval(centre, idx).d.abs() > SKIP_SLACK + SKIP_STEEP * chunk_m * 0.9 {
        return None;
    }
    // the field on every 4th voxel; blocks far from any surface are filled in from their corners
    const B: usize = 4;
    let lat: Vec<usize> = (0..n)
        .step_by(B)
        .chain(std::iter::once(n - 1))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let nl = lat.len();
    let lid = |a: usize, b: usize, c: usize| (c * nl + b) * nl + a;
    let mut coarse = vec![0.0f32; nl * nl * nl];
    for (c, &k) in lat.iter().enumerate() {
        for (b, &j) in lat.iter().enumerate() {
            for (a, &i) in lat.iter().enumerate() {
                coarse[lid(a, b, c)] = vol.eval(at(i, j, k), idx).d;
            }
        }
    }
    let mut d = vec![f32::NAN; n * n * n];
    let span = voxel * B as f32 * 1.75;
    for c in 0..nl - 1 {
        for b in 0..nl - 1 {
            for a in 0..nl - 1 {
                let corners = [
                    lid(a, b, c),
                    lid(a + 1, b, c),
                    lid(a, b + 1, c),
                    lid(a + 1, b + 1, c),
                    lid(a, b, c + 1),
                    lid(a + 1, b, c + 1),
                    lid(a, b + 1, c + 1),
                    lid(a + 1, b + 1, c + 1),
                ]
                .map(|x| coarse[x]);
                let far = corners
                    .iter()
                    .all(|v| v.abs() > SKIP_SLACK + SKIP_STEEP * span)
                    && (corners.iter().all(|v| *v > 0.0) || corners.iter().all(|v| *v < 0.0));
                let (i0, i1, j0, j1, k0, k1) =
                    (lat[a], lat[a + 1], lat[b], lat[b + 1], lat[c], lat[c + 1]);
                for k in k0..=k1 {
                    for j in j0..=j1 {
                        for i in i0..=i1 {
                            let x = id(i, j, k);
                            if !d[x].is_nan() {
                                continue;
                            }
                            d[x] = if far {
                                // (trilinear between the corners: the sign is all that matters)
                                let (u, v, w) = (
                                    (i - i0) as f32 / (i1 - i0).max(1) as f32,
                                    (j - j0) as f32 / (j1 - j0).max(1) as f32,
                                    (k - k0) as f32 / (k1 - k0).max(1) as f32,
                                );
                                let x0 = corners[0] + (corners[1] - corners[0]) * u;
                                let x1 = corners[2] + (corners[3] - corners[2]) * u;
                                let x2 = corners[4] + (corners[5] - corners[4]) * u;
                                let x3 = corners[6] + (corners[7] - corners[6]) * u;
                                let y0 = x0 + (x1 - x0) * v;
                                let y1 = x2 + (x3 - x2) * v;
                                y0 + (y1 - y0) * w
                            } else {
                                vol.eval(at(i, j, k), idx).d
                            };
                        }
                    }
                }
            }
        }
    }
    let (any_neg, any_pos) = (d.iter().any(|v| *v < 0.0), d.iter().any(|v| *v >= 0.0));
    if !(any_neg && any_pos) {
        return None;
    }
    // a vertex in every cell the surface crosses (the mean of its edge crossings)
    let nc = n - 1;
    let cid = |i: usize, j: usize, k: usize| (k * nc + j) * nc + i;
    let mut vid = vec![u32::MAX; nc * nc * nc];
    let mut verts: Vec<Vec3> = vec![];
    for k in 0..nc {
        for j in 0..nc {
            for i in 0..nc {
                let mut c = [0.0f32; 8];
                let mut mask = 0u8;
                for (b, cv) in c.iter_mut().enumerate() {
                    *cv = d[id(i + (b & 1), j + ((b >> 1) & 1), k + ((b >> 2) & 1))];
                    if *cv < 0.0 {
                        mask |= 1 << b;
                    }
                }
                if mask == 0 || mask == 255 {
                    continue;
                }
                let mut sum = Vec3::ZERO;
                let mut cnt = 0.0;
                for (a, b) in EDGES {
                    if (c[a] < 0.0) != (c[b] < 0.0) {
                        let t = c[a] / (c[a] - c[b]);
                        let pa =
                            Vec3::new((a & 1) as f32, ((a >> 1) & 1) as f32, ((a >> 2) & 1) as f32);
                        let pb =
                            Vec3::new((b & 1) as f32, ((b >> 1) & 1) as f32, ((b >> 2) & 1) as f32);
                        sum += pa + (pb - pa) * t;
                        cnt += 1.0;
                    }
                }
                vid[cid(i, j, k)] = verts.len() as u32;
                verts.push(
                    origin + (Vec3::new(i as f32, j as f32, k as f32) - 1.0 + sum / cnt) * voxel,
                );
            }
        }
    }
    // which vertices are cave (not plain terrain), and the terrain cells drawn as ground here
    let samples: Vec<_> = verts.iter().map(|p| vol.eval(*p, idx)).collect();
    let owned: Vec<bool> = samples
        .iter()
        .map(|s| (s.d - s.ground).abs() > 0.02)
        .collect();
    let in_keep: Vec<bool> = verts
        .iter()
        .map(|p| keep.contains(&vol.ground.cell_of(p.x, p.z)))
        .collect();
    // quads across every crossed edge owned by this chunk
    let mut quads: Vec<[u32; 4]> = vec![];
    for axis in 0..3 {
        for k in 1..=CHUNK as usize {
            for j in 1..=CHUNK as usize {
                for i in 1..=CHUNK as usize {
                    let (i1, j1, k1) = match axis {
                        0 => (i + 1, j, k),
                        1 => (i, j + 1, k),
                        _ => (i, j, k + 1),
                    };
                    let (d0, d1) = (d[id(i, j, k)], d[id(i1, j1, k1)]);
                    if (d0 < 0.0) == (d1 < 0.0) {
                        continue;
                    }
                    // the four cells round the edge
                    let cells = match axis {
                        0 => [
                            cid(i, j - 1, k - 1),
                            cid(i, j, k - 1),
                            cid(i, j, k),
                            cid(i, j - 1, k),
                        ],
                        1 => [
                            cid(i - 1, j, k - 1),
                            cid(i, j, k - 1),
                            cid(i, j, k),
                            cid(i - 1, j, k),
                        ],
                        _ => [
                            cid(i - 1, j - 1, k),
                            cid(i, j - 1, k),
                            cid(i, j, k),
                            cid(i - 1, j, k),
                        ],
                    };
                    let q = cells.map(|c| vid[c]);
                    if q.contains(&u32::MAX) {
                        continue;
                    }
                    if !q.iter().any(|&v| owned[v as usize] || in_keep[v as usize]) {
                        continue;
                    }
                    // face the air: the cells in this order wind round +x / −y / +z (counter-
                    // clockwise front faces); the surface faces +axis where rock is at the low end
                    let flip = (axis == 1) != (d0 >= 0.0);
                    quads.push(if flip { [q[0], q[3], q[2], q[1]] } else { q });
                }
            }
        }
    }
    if quads.is_empty() {
        return None;
    }
    // keep only the vertices the quads use
    let mut remap = vec![u32::MAX; verts.len()];
    let mut used: Vec<u32> = vec![];
    for q in &quads {
        for &v in q {
            if remap[v as usize] == u32::MAX {
                remap[v as usize] = used.len() as u32;
                used.push(v);
            }
        }
    }
    let mut out = ChunkMesh {
        key,
        hash,
        ..Default::default()
    };
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    let mut owners: HashMap<u16, u32> = HashMap::new();
    for &v in &used {
        let p = verts[v as usize];
        let s = samples[v as usize];
        let nrm = vol.normal(p, idx, voxel * 0.5);
        // occlusion from the field along the normal
        let mut occ = 0.0;
        for (dist, w) in [(0.25f32, 0.35f32), (0.6, 0.3), (1.3, 0.22), (2.4, 0.13)] {
            let dd = vol.eval(p + nrm * dist, idx).d;
            occ += ((dist - dd) / dist).clamp(0.0, 1.0) * w;
        }
        let ao = (1.0 - occ * 0.85).clamp(0.08, 1.0);
        let day = light.at(p + nrm * 1.1);
        let skin = 1.0 - smooth(0.03, 0.4, (s.d - s.ground).abs());
        let owner = if s.owner == NONE { 0 } else { s.owner };
        *owners.entry(owner).or_default() += 1;
        let slot = vol
            .themes
            .get(owner as usize)
            .map(|t| t.slot())
            .unwrap_or(0) as f32;
        let fill = fills.get(owner as usize).copied().unwrap_or(0.35);
        // damp: under ceilings and low down, less near the daylight
        let wet = ((-nrm.y).max(0.0) * 0.6 + 0.25) * (1.0 - day * 0.7);
        lo = lo.min(p);
        hi = hi.max(p);
        out.positions.push(p.to_array());
        out.normals.push(nrm.to_array());
        out.colors.push([day, ao, wet, skin]);
        out.uvs.push([slot, fill]);
    }
    out.owner = owners
        .into_iter()
        .max_by_key(|o| o.1)
        .map(|o| o.0)
        .unwrap_or(0);
    for q in quads {
        let r = q.map(|v| remap[v as usize]);
        let (p0, p1, p2, p3) = (
            verts[q[0] as usize],
            verts[q[1] as usize],
            verts[q[2] as usize],
            verts[q[3] as usize],
        );
        // split along the shorter diagonal
        if p0.distance_squared(p2) <= p1.distance_squared(p3) {
            out.indices
                .extend_from_slice(&[r[0], r[1], r[2], r[0], r[2], r[3]]);
        } else {
            out.indices
                .extend_from_slice(&[r[0], r[1], r[3], r[1], r[2], r[3]]);
        }
    }
    out.lo = lo;
    out.hi = hi;
    Some(out)
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What a rebuild produced: every chunk that should exist now (new meshes for the changed ones).
pub struct Built {
    /// (key, hash) of every chunk with a surface
    pub all: Vec<(ChunkKey, u64)>,
    /// meshes of chunks whose hash changed
    pub meshes: Vec<ChunkMesh>,
    pub holes: HashSet<IVec2>,
    pub light: Light,
    /// per cave: its timbers, pools, crystals, webs and their lights (`dress`)
    pub dress: Vec<super::dress::Dress>,
}

/// Meshes every chunk whose fingerprint differs from `old` (key → hash), in parallel.
pub fn build(vol: &Volume, old: &HashMap<ChunkKey, u64>, fills: &[f32]) -> Built {
    let light = Light::build(vol);
    let keep = ground_cells(vol);
    let list = chunks(vol);
    let results: Vec<(ChunkKey, u64, Option<ChunkMesh>, bool)> = list
        .par_iter()
        .map(|(key, idx)| {
            let hash = chunk_hash(vol, *key, idx, &keep, &light, fills);
            if old.get(key) == Some(&hash) {
                return (*key, hash, None, true);
            }
            let m = mesh_chunk(vol, *key, idx, &keep, &light, fills, hash);
            let has = m.is_some();
            (*key, hash, m, has)
        })
        .collect();
    let mut all = vec![];
    let mut meshes = vec![];
    for (key, hash, m, has) in results {
        if has {
            all.push((key, hash));
        }
        if let Some(m) = m {
            meshes.push(m);
        }
    }
    Built {
        all,
        meshes,
        holes: vol.holes.clone(),
        light,
        dress: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caves::{layout, model::*, sdf};
    use bevy::math::Vec2;

    fn hill() -> crate::terrain::Terrain {
        let mut t = crate::terrain::Terrain::new(256, 4.0, 0.0);
        let n = t.n();
        for j in 0..n {
            for i in 0..n {
                let p = t.grid_to_world(i, j);
                t.heights[j * n + i] = 10.0 + 70.0 * (-(p.length() / 180.0).powi(2)).exp();
            }
        }
        t
    }

    #[test]
    fn a_cave_meshes_closed_and_dark_inside() {
        let t = hill();
        let e = Vec2::new(150.0, 0.0);
        let (heading, _) = layout::uphill(&t, e, Vec2::ZERO);
        let mut c = Cave {
            id: 1,
            name: "T".into(),
            kind: CaveKind::Cave,
            size: CaveSize::Small,
            levels: 1,
            up: false,
            theme: Theme::Limestone,
            resolved: Theme::Auto,
            features: Features::default(),
            darkness: Darkness::Dim,
            seed: 4,
            entrance: [e.x, t.height_at(e.x, e.y), e.y],
            heading,
            mouth: Mouth::Hillside,
            chambers: vec![],
            tunnels: vec![],
            dungeon: None,
            arch: None,
            props: vec![],
        };
        layout::layout(&mut c, &t);
        let caves = Caves {
            list: vec![c.clone()],
            strokes: vec![],
            next_id: 1,
        };
        let vol = sdf::compile(&caves, &t);
        let t0 = std::time::Instant::now();
        let b = build(&vol, &HashMap::new(), &[0.35]);
        let tris: usize = b.meshes.iter().map(|m| m.indices.len() / 3).sum();
        println!(
            "{} chunks, {} triangles, {:?}",
            b.meshes.len(),
            tris,
            t0.elapsed()
        );
        assert!(!b.meshes.is_empty());
        assert!(tris > 2000);
        // deep in the last chamber it is darker than at the mouth
        let deep = c.chambers.last().unwrap().center() + Vec3::Y * 1.5;
        let mouth = Vec3::from_array(c.entrance) + Vec3::Y * 1.5;
        assert!(
            b.light.at(deep) < b.light.at(mouth) * 0.6,
            "deep {} mouth {}",
            b.light.at(deep),
            b.light.at(mouth)
        );
        // a rebuild with nothing changed meshes nothing
        let old: HashMap<ChunkKey, u64> = b.all.iter().copied().collect();
        let b2 = build(&vol, &old, &[0.35]);
        assert!(b2.meshes.is_empty());
        assert_eq!(b2.all.len(), b.all.len());
        // all normals unit, all indices in range
        for m in &b.meshes {
            for nrm in &m.normals {
                let l = Vec3::from_array(*nrm).length();
                assert!((l - 1.0).abs() < 1e-3);
            }
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        }
    }

    #[test]
    fn giant_caves_mesh_coarse() {
        let t = super::super::layout::tests_hill();
        let e = bevy::math::Vec2::new(150.0, 0.0);
        let (heading, _) = layout::uphill(&t, e, bevy::math::Vec2::ZERO);
        let mut c = Cave {
            id: 1,
            name: "G".into(),
            kind: CaveKind::Cave,
            size: CaveSize::Giant,
            levels: 1,
            up: false,
            theme: Theme::Limestone,
            resolved: Theme::Auto,
            features: Features::default(),
            darkness: Darkness::Dim,
            seed: 3,
            entrance: [e.x, t.height_at(e.x, e.y), e.y],
            heading,
            mouth: Mouth::Hillside,
            chambers: vec![],
            tunnels: vec![],
            dungeon: None,
            arch: None,
            props: vec![],
        };
        layout::layout(&mut c, &t);
        let caves = Caves {
            list: vec![c],
            strokes: vec![],
            next_id: 1,
        };
        let t0 = std::time::Instant::now();
        let vol = sdf::compile(&caves, &t);
        let t1 = t0.elapsed();
        let b = build(&vol, &HashMap::new(), &[0.35]);
        let tris: usize = b.meshes.iter().map(|m| m.indices.len() / 3).sum();
        let coarse = b.meshes.iter().filter(|m| m.key.w == 1).count();
        println!(
            "giant: {} shapes, {} chunks ({coarse} coarse), {tris} triangles, field {t1:?}, total {:?}",
            vol.shapes.len(),
            b.meshes.len(),
            t0.elapsed()
        );
        assert!(
            coarse > 0 && coarse == b.meshes.len(),
            "a giant cave alone meshes coarse"
        );
        assert!(tris > 10_000);
    }

    #[test]
    fn export_rock_simplifies_within_tolerance() {
        let t = super::super::layout::tests_hill();
        for (size, seed) in [(CaveSize::Medium, 2u32), (CaveSize::Giant, 3)] {
            let e = bevy::math::Vec2::new(150.0, 0.0);
            let (heading, _) = layout::uphill(&t, e, bevy::math::Vec2::ZERO);
            let mut c = Cave {
                id: 1,
                name: "S".into(),
                kind: CaveKind::Cave,
                size,
                levels: 2,
                up: false,
                theme: Theme::Limestone,
                resolved: Theme::Auto,
                features: Features::default(),
                darkness: Darkness::Dim,
                seed,
                entrance: [e.x, t.height_at(e.x, e.y), e.y],
                heading,
                mouth: Mouth::Hillside,
                chambers: vec![],
                tunnels: vec![],
                dungeon: None,
                arch: None,
                props: vec![],
            };
            layout::layout(&mut c, &t);
            let caves = Caves {
                list: vec![c],
                strokes: vec![],
                next_id: 1,
            };
            let vol = sdf::compile(&caves, &t);
            let b = build(&vol, &HashMap::new(), &[0.35]);
            let mut pos: Vec<Vec3> = vec![];
            let mut idx: Vec<u32> = vec![];
            for m in &b.meshes {
                let base = pos.len() as u32;
                pos.extend(m.positions.iter().map(|p| Vec3::from_array(*p)));
                idx.extend(m.indices.iter().map(|k| k + base));
            }
            let (keep, welded) = super::super::simplify::weld(&pos, &idx);
            let pos: Vec<Vec3> = keep.iter().map(|k| pos[*k as usize]).collect();
            // how far triangle centres sit off the rock (the field's zero), as a fraction of the
            // tolerance: the editor's own mesh first, then simplified at a few tolerances
            let off = |idx: &[u32]| {
                let mut v: Vec<f32> = idx
                    .chunks(3)
                    .step_by(5)
                    .map(|t| {
                        vol.at((pos[t[0] as usize] + pos[t[1] as usize] + pos[t[2] as usize]) / 3.0)
                            .d
                            .abs()
                    })
                    .collect();
                v.sort_by(|a, b| a.total_cmp(b));
                (v[v.len() / 2], v[v.len() * 99 / 100])
            };
            let base = off(&welded);
            println!(
                "{size:?}: {} triangles, centres off the rock: median {:.3} m, 99% {:.3} m",
                welded.len() / 3,
                base.0,
                base.1
            );
            for tol in [0.05f32, super::super::simplify::EXPORT_TOLERANCE, 0.25] {
                let t0 = std::time::Instant::now();
                let out = super::super::simplify::simplify(&pos, &welded, tol, &[]);
                let took = t0.elapsed();
                let o = off(&out);
                println!(
                    "   tol {tol}: {:.0}% kept in {took:?}, median {:.3} m, 99% {:.3} m",
                    out.len() as f32 / welded.len() as f32 * 100.0,
                    o.0,
                    o.1
                );
            }
            // at the export's tolerance: well under half the triangles, the shape where it was
            let out = super::super::simplify::simplify(
                &pos,
                &welded,
                super::super::simplify::EXPORT_TOLERANCE,
                &[],
            );
            assert!(
                out.len() < welded.len() * 55 / 100,
                "too little gone: {} of {}",
                out.len() / 3,
                welded.len() / 3
            );
            let o = off(&out);
            assert!(
                o.0 < base.0 + 0.04 && o.1 < base.1 + 0.1,
                "strays: {o:?} against {base:?}"
            );
        }
    }
}
