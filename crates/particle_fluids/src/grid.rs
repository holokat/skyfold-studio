//! Neighbor search: a cell-sorted grid, and per-particle neighbor lists.
//!
//! Port of `core/hashGrid/` for the CPU. Each rebuild bins every occupied
//! slot's predicted position into a cell, counting-sorts the particles by
//! cell, and keeps a cell-ordered copy of the positions so neighbor walks read
//! memory contiguously. Queries visit the 27 cells around a point.
//!
//! When the particles' bounding box holds few enough cells, the grid is dense
//! (row-major cells over the box, so the three cells along x are one
//! contiguous run and a query reads 9 runs). Otherwise it falls back to the
//! reference's spatial hash: Morton-ordered buckets with a bias of 512 cells
//! per axis, origin centred on the particles. Distant cells can then share a
//! bucket, so callers always filter candidates by distance. With at least
//! 1024 buckets every axis keeps three Morton bits, so the 27 cells around a
//! point never share a bucket and no deduplication is needed.

use crate::math::part1by2;
use crate::par::SharedMut;
use glam::Vec3A;
use rayon::prelude::*;

/// Most neighbors kept per particle; the rest are dropped and counted by
/// [`Simulation::neighbor_overflow`](crate::Simulation::neighbor_overflow).
pub const MAX_NEIGHBORS: usize = 64;

const MORTON_BIAS: i32 = 512;
const CELL_COORD_MAX: f32 = (1 << 30) as f32;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Dense { nx: i32, ny: i32, nz: i32 },
    Hashed { mask: u32 },
}

pub(crate) struct HashGrid {
    pub cell: f32,
    k: Keyer,
    /// Prefix sums: cell (or bucket) `c` holds sorted slots `start[c]..start[c + 1]`.
    start: Vec<u32>,
    /// Particle indices in cell order.
    pub sorted: Vec<u32>,
    /// Positions in cell order, refreshed by every rebuild.
    pub sorted_pos: Vec<Vec3A>,
    keys: Vec<u32>,
    cursor: Vec<u32>,
    /// Hashed mode only: a particle was more than 512 cells from the origin.
    pub overflow: bool,
}

impl HashGrid {
    pub fn new(cell: f32) -> Self {
        Self {
            cell,
            k: Keyer {
                inv_cell: 1.0 / cell,
                origin: Vec3A::ZERO,
                mode: Mode::Hashed { mask: 1023 },
            },
            start: Vec::new(),
            sorted: Vec::new(),
            sorted_pos: Vec::new(),
            keys: Vec::new(),
            cursor: Vec::new(),
            overflow: false,
        }
    }

    /// Re-bin the particles in `active` by `positions`.
    pub fn rebuild(&mut self, positions: &[Vec3A], active: &[u32]) {
        let n = active.len();
        self.sorted.resize(n, 0);
        self.sorted_pos.resize(n, Vec3A::ZERO);
        if n == 0 {
            self.start.clear();
            self.start.push(0);
            self.k.mode = Mode::Dense {
                nx: 1,
                ny: 1,
                nz: 1,
            };
            self.start.push(0);
            return;
        }
        let (lo, hi) = if n < crate::par::CHEAP_SEQ_THRESHOLD {
            active.iter().fold(
                (Vec3A::splat(f32::INFINITY), Vec3A::splat(f32::NEG_INFINITY)),
                |(lo, hi), &i| {
                    let p = positions[i as usize];
                    (lo.min(p), hi.max(p))
                },
            )
        } else {
            active
                .par_iter()
                .with_min_len(4096)
                .fold(
                    || (Vec3A::splat(f32::INFINITY), Vec3A::splat(f32::NEG_INFINITY)),
                    |(lo, hi), &i| {
                        let p = positions[i as usize];
                        (lo.min(p), hi.max(p))
                    },
                )
                .reduce(
                    || (Vec3A::splat(f32::INFINITY), Vec3A::splat(f32::NEG_INFINITY)),
                    |a, b| (a.0.min(b.0), a.1.max(b.1)),
                )
        };
        let extent = ((hi - lo) * self.k.inv_cell).floor() + Vec3A::splat(3.0);
        let cells = extent.x as f64 * extent.y as f64 * extent.z as f64;
        let dense_limit = ((64 * n).max(1 << 16) as f64).min((1u64 << 22) as f64);
        let table = if lo.is_finite() && hi.is_finite() && cells <= dense_limit {
            // One empty cell of margin on every side.
            self.k.origin = lo - Vec3A::splat(self.cell);
            self.k.mode = Mode::Dense {
                nx: extent.x as i32,
                ny: extent.y as i32,
                nz: extent.z as i32,
            };
            self.overflow = false;
            cells as usize
        } else {
            let center = (lo + hi) * 0.5;
            if center.is_finite() {
                self.k.origin = (center * self.k.inv_cell).floor() * self.cell;
            }
            self.overflow = ((hi - lo) * self.k.inv_cell * 0.5).max_element() > MORTON_BIAS as f32;
            let table = (2 * n).next_power_of_two().clamp(1024, 1 << 20);
            self.k.mode = Mode::Hashed {
                mask: table as u32 - 1,
            };
            table
        };
        self.keys.resize(n, 0);
        let this = self.k;
        if n < crate::par::CHEAP_SEQ_THRESHOLD {
            for (k, &i) in active.iter().enumerate() {
                self.keys[k] = this.key_of(positions[i as usize]);
            }
        } else {
            self.keys
                .par_iter_mut()
                .with_min_len(4096)
                .zip(active.par_iter())
                .for_each(|(key, &i)| *key = this.key_of(positions[i as usize]));
        }
        self.start.clear();
        self.start.resize(table + 1, 0);
        for &key in &self.keys {
            self.start[key as usize + 1] += 1;
        }
        for c in 0..table {
            self.start[c + 1] += self.start[c];
        }
        self.cursor.clear();
        self.cursor.extend_from_slice(&self.start[..table]);
        for (k, &i) in active.iter().enumerate() {
            let key = self.keys[k] as usize;
            let slot = self.cursor[key] as usize;
            self.cursor[key] += 1;
            self.sorted[slot] = i;
            self.sorted_pos[slot] = positions[i as usize];
        }
    }

    /// Call `f(first_slot, end_slot)` for each contiguous run of sorted slots
    /// covering the cells within `radius` of `p` (on a dense grid, one run per
    /// (y, z) row of the box around `p`). Candidates still need a distance test.
    #[inline]
    pub fn for_each_run(&self, p: Vec3A, radius: f32, mut f: impl FnMut(usize, usize)) {
        if self.sorted.is_empty() {
            return;
        }
        let g = (p - self.k.origin) * self.k.inv_cell;
        if !g.is_finite() {
            return;
        }
        let r = radius * self.k.inv_cell;
        let span =
            |lo: f32, hi: f32, n: i32| ((lo.floor() as i32).max(0), (hi.floor() as i32).min(n - 1));
        match self.k.mode {
            Mode::Dense { nx, ny, nz } => {
                // A box query: with cells as large as the radius this is the
                // reference's 3 × 3 × 3 block, read as 9 contiguous rows.
                let (z0, z1) = span(g.z - r, g.z + r, nz);
                let (y0, y1) = span(g.y - r, g.y + r, ny);
                let (x0, x1) = span(g.x - r, g.x + r, nx);
                if x0 > x1 {
                    return;
                }
                for z in z0..=z1 {
                    for y in y0..=y1 {
                        let row = nx * (y + ny * z);
                        f(
                            self.start[(row + x0) as usize] as usize,
                            self.start[(row + x1) as usize + 1] as usize,
                        );
                    }
                }
            }
            Mode::Hashed { mask } => {
                let lo = (g - Vec3A::splat(r)).floor();
                let hi = (g + Vec3A::splat(r)).floor();
                let clamp = |v: f32| v.clamp(-CELL_COORD_MAX, CELL_COORD_MAX - 1.0) as i32;
                let (x0, x1, y0, y1, z0, z1) = (
                    clamp(lo.x),
                    clamp(hi.x),
                    clamp(lo.y),
                    clamp(hi.y),
                    clamp(lo.z),
                    clamp(hi.z),
                );
                // With at least 1024 buckets each axis keeps three Morton bits, so
                // cells in a window of up to 8 never share a bucket; wider
                // windows dedupe explicitly.
                let wide = x1 - x0 >= 8 || y1 - y0 >= 8 || z1 - z0 >= 8;
                let mut seen: Vec<usize> = Vec::new();
                let r2 = r * r;
                for z in z0..=z1 {
                    let dz = axis_gap(g.z, z);
                    let sz = part1by2(z.wrapping_add(MORTON_BIAS) as u32) << 2;
                    for y in y0..=y1 {
                        let dy = axis_gap(g.y, y);
                        if dy * dy + dz * dz > r2 {
                            continue;
                        }
                        let syz = (part1by2(y.wrapping_add(MORTON_BIAS) as u32) << 1) | sz;
                        for x in x0..=x1 {
                            let dx = axis_gap(g.x, x);
                            if dx * dx + dy * dy + dz * dz > r2 {
                                continue;
                            }
                            let b = ((part1by2(x.wrapping_add(MORTON_BIAS) as u32) | syz) & mask)
                                as usize;
                            if wide {
                                if seen.contains(&b) {
                                    continue;
                                }
                                seen.push(b);
                            }
                            f(self.start[b] as usize, self.start[b + 1] as usize);
                        }
                    }
                }
            }
        }
    }

    /// Call `f(index, sorted_slot)` for every particle in cells within
    /// `radius` of `p`. Candidates can be farther than `radius` (or far away,
    /// through shared hash buckets), so filter by distance.
    #[inline]
    pub fn for_each(&self, p: Vec3A, radius: f32, mut f: impl FnMut(u32, usize)) {
        self.for_each_run(p, radius, |s, e| {
            for slot in s..e {
                f(self.sorted[slot], slot);
            }
        });
    }
}

/// Distance from continuous cell coordinate `g` to the cell interval `[c, c + 1]`.
#[inline(always)]
fn axis_gap(g: f32, c: i32) -> f32 {
    let c = c as f32;
    if g < c {
        c - g
    } else if g > c + 1.0 {
        g - c - 1.0
    } else {
        0.0
    }
}

/// Cell quantization and keying of one grid configuration, copyable into parallel closures.
#[derive(Clone, Copy)]
struct Keyer {
    origin: Vec3A,
    inv_cell: f32,
    mode: Mode,
}

impl Keyer {
    #[inline(always)]
    fn cell_of(&self, p: Vec3A) -> [i32; 3] {
        let c = ((p - self.origin) * self.inv_cell).floor();
        [
            c.x.clamp(-CELL_COORD_MAX, CELL_COORD_MAX - 1.0) as i32,
            c.y.clamp(-CELL_COORD_MAX, CELL_COORD_MAX - 1.0) as i32,
            c.z.clamp(-CELL_COORD_MAX, CELL_COORD_MAX - 1.0) as i32,
        ]
    }

    #[inline(always)]
    fn key(&self, c: [i32; 3]) -> u32 {
        match self.mode {
            Mode::Dense { nx, ny, nz } => {
                let x = c[0].clamp(0, nx - 1);
                let y = c[1].clamp(0, ny - 1);
                let z = c[2].clamp(0, nz - 1);
                (x + nx * (y + ny * z)) as u32
            }
            Mode::Hashed { mask } => {
                let sx = part1by2(c[0].wrapping_add(MORTON_BIAS) as u32);
                let sy = part1by2(c[1].wrapping_add(MORTON_BIAS) as u32);
                let sz = part1by2(c[2].wrapping_add(MORTON_BIAS) as u32);
                (sx | (sy << 1) | (sz << 2)) & mask
            }
        }
    }

    #[inline(always)]
    fn key_of(&self, p: Vec3A) -> u32 {
        self.key(self.cell_of(p))
    }
}

/// Neighbors within a radius for a set of rows (particles), gathered once per
/// substep so kernels that visit neighbors several times don't each walk the
/// grid. Row `i` lives at `i · MAX_NEIGHBORS` and includes particle `i` itself.
pub(crate) struct NeighborLists {
    pub idx: Vec<u32>,
    pub count: Vec<u8>,
    /// Some row had more than [`MAX_NEIGHBORS`] neighbors at the last build.
    pub overflow: bool,
}

impl NeighborLists {
    pub fn new(capacity: usize) -> Self {
        Self {
            idx: vec![0; capacity * MAX_NEIGHBORS],
            count: vec![0; capacity],
            overflow: false,
        }
    }

    #[inline(always)]
    pub fn row(&self, i: usize) -> &[u32] {
        let n = self.count[i] as usize;
        &self.idx[i * MAX_NEIGHBORS..i * MAX_NEIGHBORS + n]
    }

    /// Rebuild the rows of every particle flagged in `is_row` from `grid`,
    /// keeping neighbors closer than `sqrt(radius2)`. Rows are visited in the
    /// grid's cell order, so neighboring queries share cached cells.
    pub fn build(&mut self, grid: &HashGrid, is_row: &[bool], radius2: f32) {
        use std::sync::atomic::{AtomicBool, Ordering};
        let overflow = AtomicBool::new(false);
        let idx = SharedMut::new(&mut self.idx[..]);
        let count = SharedMut::new(&mut self.count[..]);
        let n = grid.sorted.len();
        let radius = radius2.sqrt();
        let body = |slot: usize| {
            let i = grid.sorted[slot] as usize;
            if !is_row[i] {
                return;
            }
            let xi = grid.sorted_pos[slot];
            // Branch-free compaction into a scratch row one entry longer than
            // the list, so the first MAX_NEIGHBORS hits are kept.
            let mut buf = [0u32; MAX_NEIGHBORS + 1];
            let mut m = 0usize;
            grid.for_each_run(xi, radius, |s, e| {
                for k in s..e {
                    buf[m.min(MAX_NEIGHBORS)] = grid.sorted[k];
                    m += ((xi - grid.sorted_pos[k]).length_squared() < radius2) as usize;
                }
            });
            let kept = m.min(MAX_NEIGHBORS);
            // SAFETY: row i is written only by this task (each particle has one slot).
            unsafe {
                idx.slice(i * MAX_NEIGHBORS, kept)
                    .copy_from_slice(&buf[..kept]);
                *count.get(i) = kept as u8;
            }
            if m > MAX_NEIGHBORS {
                overflow.store(true, Ordering::Relaxed);
            }
        };
        crate::par::for_each_range(n, body);
        self.overflow = overflow.into_inner();
    }
}

impl NeighborLists {
    /// Like [`build`](Self::build) with periodic x/z boundaries of the box at
    /// `min` with `size`: particles within the radius of a seam also find
    /// neighbors across it (queries at the shifted images).
    pub fn build_periodic(
        &mut self,
        grid: &HashGrid,
        is_row: &[bool],
        radius2: f32,
        min: Vec3A,
        size: Vec3A,
    ) {
        use std::sync::atomic::{AtomicBool, Ordering};
        let overflow = AtomicBool::new(false);
        let idx = SharedMut::new(&mut self.idx[..]);
        let count = SharedMut::new(&mut self.count[..]);
        let radius = radius2.sqrt();
        let n = grid.sorted.len();
        crate::par::for_each_range(n, |slot| {
            let i = grid.sorted[slot] as usize;
            if !is_row[i] {
                return;
            }
            let xi = grid.sorted_pos[slot];
            let rel = xi - min;
            let sx: &[f32] = if rel.x < radius {
                &[0.0, 1.0]
            } else if size.x - rel.x < radius {
                &[0.0, -1.0]
            } else {
                &[0.0]
            };
            let sz: &[f32] = if rel.z < radius {
                &[0.0, 1.0]
            } else if size.z - rel.z < radius {
                &[0.0, -1.0]
            } else {
                &[0.0]
            };
            let mut buf = [0u32; MAX_NEIGHBORS + 1];
            let mut m = 0usize;
            for &ax in sx {
                for &az in sz {
                    let q = xi + Vec3A::new(ax * size.x, 0.0, az * size.z);
                    grid.for_each_run(q, radius, |s, e| {
                        for k in s..e {
                            buf[m.min(MAX_NEIGHBORS)] = grid.sorted[k];
                            m += ((q - grid.sorted_pos[k]).length_squared() < radius2) as usize;
                        }
                    });
                }
            }
            let kept = m.min(MAX_NEIGHBORS);
            // SAFETY: row i is written only by this task.
            unsafe {
                idx.slice(i * MAX_NEIGHBORS, kept)
                    .copy_from_slice(&buf[..kept]);
                *count.get(i) = kept as u8;
            }
            if m > MAX_NEIGHBORS {
                overflow.store(true, Ordering::Relaxed);
            }
        });
        self.overflow = overflow.into_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_finds_all_neighbors() {
        let mut positions = Vec::new();
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };
        for _ in 0..2000 {
            positions.push(Vec3A::new(rnd() * 2.0 + 100.0, rnd(), rnd() * 2.0 - 50.0));
        }
        let active: Vec<u32> = (0..positions.len() as u32).collect();
        let h = 0.08;
        let mut grid = HashGrid::new(h);
        grid.rebuild(&positions, &active);
        for i in (0..positions.len()).step_by(37) {
            let mut found = Vec::new();
            grid.for_each(positions[i], h, |j, _| {
                if (positions[j as usize] - positions[i]).length_squared() < h * h {
                    found.push(j);
                }
            });
            found.sort();
            let brute: Vec<u32> = (0..positions.len() as u32)
                .filter(|&j| (positions[j as usize] - positions[i]).length_squared() < h * h)
                .collect();
            assert_eq!(found, brute);
        }
    }
}
