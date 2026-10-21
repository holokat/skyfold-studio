//! Holes in the terrain: cells whose two triangles are left out of the terrain mesh, where a cave
//! mouth, a sinkhole or a skylight opens the ground (`crate::caves` owns them and draws the
//! ground round them itself). Patches holding a hole always draw at full detail, and no grass
//! grows in a hole or the cell round it.

use std::collections::HashSet;

use bevy::prelude::*;

use super::mesh::PATCH;
use super::{GridRect, Terrain};

#[derive(Resource, Default)]
pub struct TerrainHoles {
    /// cells (i, j) without terrain triangles
    pub cells: HashSet<IVec2>,
    /// full-detail patches (level-0 patch x, z) holding a hole
    pub patches: HashSet<(u32, u32)>,
    /// cells over a cave's space (under the ground): the terrain's skirts stay short there, so
    /// none hangs down into a chamber
    pub under: HashSet<IVec2>,
    /// bumped on every change
    pub revision: u64,
}

impl TerrainHoles {
    #[inline]
    pub fn contains(&self, i: usize, j: usize) -> bool {
        !self.cells.is_empty() && self.cells.contains(&IVec2::new(i as i32, j as i32))
    }

    /// Is the terrain open at world (x, z)?
    pub fn at(&self, t: &Terrain, x: f32, z: f32) -> bool {
        if self.cells.is_empty() {
            return false;
        }
        let g = t.world_to_grid(x, z);
        self.cells
            .contains(&IVec2::new(g.x.floor() as i32, g.y.floor() as i32))
    }

    /// Is grid vertex (i, j) a corner of a cell over a cave?
    #[inline]
    pub fn over_cave(&self, i: usize, j: usize) -> bool {
        if self.under.is_empty() {
            return false;
        }
        let (i, j) = (i as i32, j as i32);
        [(i - 1, j - 1), (i, j - 1), (i - 1, j), (i, j)]
            .iter()
            .any(|&(a, b)| self.under.contains(&IVec2::new(a, b)))
    }

    /// Replaces the cells over caves; the patches drawn over a change are rebuilt.
    pub fn set_under(&mut self, cells: HashSet<IVec2>, t: &mut Terrain) {
        if cells == self.under {
            return;
        }
        let n = t.n();
        let (mut lo, mut hi) = (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN));
        for c in cells.symmetric_difference(&self.under) {
            lo = lo.min(*c);
            hi = hi.max(*c);
        }
        self.under = cells;
        self.revision += 1;
        if lo.x <= hi.x {
            let r = GridRect {
                x0: (lo.x - 2).max(0) as usize,
                z0: (lo.y - 2).max(0) as usize,
                x1: ((hi.x + 3) as usize).min(n - 1),
                z1: ((hi.y + 3) as usize).min(n - 1),
            };
            t.dirty_mesh = Some(match t.dirty_mesh {
                Some(d) => d.union(r),
                None => r,
            });
        }
    }

    /// Any hole in the patches (level-0 coordinates) `x0..x1`, `z0..z1` (exclusive)?
    pub fn in_patches(&self, x0: u32, z0: u32, x1: u32, z1: u32) -> bool {
        self.patches
            .iter()
            .any(|&(x, z)| x >= x0 && x < x1 && z >= z0 && z < z1)
    }

    /// Replaces the holes; the terrain mesh and the grass are rebuilt where they changed.
    pub fn set(&mut self, cells: HashSet<IVec2>, t: &mut Terrain) {
        if cells == self.cells {
            return;
        }
        let changed: Vec<IVec2> = cells.symmetric_difference(&self.cells).copied().collect();
        let n = t.n();
        let (mut lo, mut hi) = (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN));
        for c in &changed {
            lo = lo.min(*c);
            hi = hi.max(*c);
        }
        self.patches = cells
            .iter()
            .map(|c| (c.x as u32 / PATCH as u32, c.y as u32 / PATCH as u32))
            .collect();
        self.cells = cells;
        self.revision += 1;
        if lo.x <= hi.x {
            let r = GridRect {
                x0: (lo.x - 2).max(0) as usize,
                z0: (lo.y - 2).max(0) as usize,
                x1: ((hi.x + 3) as usize).min(n - 1),
                z1: ((hi.y + 3) as usize).min(n - 1),
            };
            t.dirty_mesh = Some(match t.dirty_mesh {
                Some(d) => d.union(r),
                None => r,
            });
            t.dirty_grass = Some(match t.dirty_grass {
                Some(d) => d.union(r),
                None => r,
            });
        }
    }

    /// Takes the grass out of the holes and the cells round them (density map with `n`
    /// vertices per side, within `rect`).
    pub fn clear_grass(&self, data: &mut [[u8; 4]], n: usize, rect: GridRect) {
        if self.cells.is_empty() {
            return;
        }
        for c in &self.cells {
            // the hole's corners and the ring of vertices round it
            for dj in -1..=2 {
                for di in -1..=2 {
                    let (i, j) = (c.x + di, c.y + dj);
                    if i < 0 || j < 0 || i as usize >= n || j as usize >= n {
                        continue;
                    }
                    let (i, j) = (i as usize, j as usize);
                    if i >= rect.x0 && i <= rect.x1 && j >= rect.z0 && j <= rect.z1 {
                        data[j * n + i][0] = 0;
                    }
                }
            }
        }
    }
}
