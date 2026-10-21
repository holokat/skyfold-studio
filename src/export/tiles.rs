//! World-scale exports: the terrain split into tiles (full resolution for 8–16 km worlds, which
//! no single engine terrain holds) and instances grouped by world cell.
//!
//! Tiles share their edge samples (tile `i` covers samples `i·q … (i + 1)·q`, `q` = samples − 1),
//! so neighbouring engine terrains meet exactly. Unity, Godot and the generic package keep one
//! tile up to 4097 samples and use 2049-sample tiles beyond; Unreal uses its landscape sizes
//! (up to 4033 in one piece, 2017-sample tiles beyond, named `_x<i>_y<j>` for World Partition's
//! tiled heightmap import).

use serde_json::{Value, json};

use super::Target;

/// Width of the world cells instances are grouped in (m): one HISM / parent / list per cell.
pub const CELL_M: f32 = 512.0;

/// (tiles per side, samples per tile side) for a terrain of `n` samples per side.
pub fn split(target: Target, n: usize) -> (usize, usize) {
    let q = n - 1;
    let (max, tile) = match target {
        Target::Unreal => (4032, 2016),
        _ => (4096, 2048),
    };
    if q <= max || !q.is_multiple_of(tile) {
        return (1, n);
    }
    (q / tile, tile + 1)
}

/// Rows `j0 … j0 + h` and columns `i0 … i0 + w` (exclusive ends) of an `n`-wide row-major grid.
pub fn block<T: Copy>(data: &[T], n: usize, i0: usize, j0: usize, w: usize, h: usize) -> Vec<T> {
    let mut out = Vec::with_capacity(w * h);
    for j in j0..j0 + h {
        out.extend_from_slice(&data[j * n + i0..j * n + i0 + w]);
    }
    out
}

/// The world cell (i, j) of a position (cells of `CELL_M` from the map's north-west corner).
pub fn cell_of(half: f32, x: f32, z: f32) -> (i32, i32) {
    (
        ((x + half) / CELL_M).floor() as i32,
        ((z + half) / CELL_M).floor() as i32,
    )
}

/// Per instance, its world cell as one index (`j · cells + i`), and the cells per side.
pub fn cell_indices(half: f32, positions: impl Iterator<Item = [f32; 3]>) -> (Vec<u32>, u32) {
    let cells = ((half * 2.0) / CELL_M).ceil().max(1.0) as i32;
    let v = positions.map(|p| {
        let (i, j) = cell_of(half, p[0], p[2]);
        (j.clamp(0, cells - 1) * cells + i.clamp(0, cells - 1)) as u32
    });
    (v.collect(), cells as u32)
}

pub fn cells_json(half: f32) -> Value {
    let cells = ((half * 2.0) / CELL_M).ceil().max(1.0) as u32;
    json!({ "size_m": CELL_M, "per_side": cells, "origin_xz": [-half, -half], "index": "cell = j * per_side + i, i = floor((x - origin_x) / size_m), j = floor((z - origin_z) / size_m)" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_share_edges() {
        assert_eq!(split(Target::Unity, 4097), (1, 4097));
        assert_eq!(split(Target::Unity, 8193), (4, 2049));
        assert_eq!(split(Target::Godot, 16385), (8, 2049));
        assert_eq!(split(Target::Unreal, 4033), (1, 4033));
        assert_eq!(split(Target::Unreal, 8065), (4, 2017));
        let n = 9;
        let grid: Vec<usize> = (0..n * n).collect();
        let a = block(&grid, n, 0, 0, 5, 5);
        let b = block(&grid, n, 4, 0, 5, 5);
        // the last column of tile 0 is the first of tile 1
        for r in 0..5 {
            assert_eq!(a[r * 5 + 4], b[r * 5]);
        }
        let (c, per) = cell_indices(
            2048.0,
            [
                [-2048.0, 0.0, -2048.0],
                [2047.0, 0.0, 2047.0],
                [0.0, 0.0, 0.0],
            ]
            .into_iter(),
        );
        assert_eq!(per, 8);
        assert_eq!(c, vec![0, 63, 36]);
    }
}
