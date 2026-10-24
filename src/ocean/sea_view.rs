//! Whether any open sea can be seen from the camera. The ocean is a disc of water 5 km round
//! the eye, drawn every frame wherever it is above the ground; deep in a valley or forest every
//! one of its pixels is behind the terrain, yet it is still drawn (and its waves simulated). This
//! casts rays round the eye over a conservative picture of the terrain: each cell keeps the lowest
//! height under it and its neighbours (so the ground hides no more than it really does), and a
//! cell counts as sea when that lowest point is under the highest a wave can reach. Rays far out
//! read coarser cells, never coarser than the gap between two rays, so no stretch of sea falls
//! between them. Trees and buildings are left out (they only ever hide more). When no ray finds
//! sea in sight, the sea can't be seen: `SeaView::visible` is false.
//!
//! The picture is made again after the heights change (a few rows a frame, the old one is not
//! trusted meanwhile) and the rays are cast again whenever the eye moves a little.

use bevy::prelude::*;

use crate::terrain::Terrain;

/// Rays round the eye.
const RAYS: usize = 256;
/// Finest cell of the picture (m).
const FINEST: f32 = 8.0;
/// Out to here (m): the ocean disc's radius.
const REACH: f32 = 5000.0;
/// Recast after the eye moved this far (m).
const MOVE: f32 = 2.0;
/// Terrain rows read into the picture per frame.
const ROWS_PER_FRAME: usize = 192;

#[derive(Resource)]
pub struct SeaView {
    /// any open sea in sight (true until worked out)
    pub visible: bool,
    /// where the rays were last cast from, and for which sea surface height
    cast_at: Option<(Vec3, f32)>,
    picture: Picture,
    build: Option<Build>,
}

impl Default for SeaView {
    fn default() -> Self {
        Self {
            visible: true,
            cast_at: None,
            picture: Picture::default(),
            build: None,
        }
    }
}

/// Min-height pyramid: level k has cells of `cell * 2^k` m; each cell holds the lowest terrain
/// height within it and its eight neighbours.
#[derive(Default)]
struct Picture {
    revision: Option<u64>,
    half: f32,
    cell: f32,
    levels: Vec<(usize, Vec<f32>)>,
}

/// A picture being read from the terrain, a band of rows at a time.
struct Build {
    revision: u64,
    stride: usize,
    n: usize,
    row: usize,
    mins: Vec<f32>,
}

impl Picture {
    /// The lowest height near `p` at a cell size of at least `size` m (None: off the map).
    fn low(&self, p: Vec2, size: f32) -> Option<f32> {
        let k = ((size / self.cell).max(1.0).log2().ceil() as usize).min(self.levels.len() - 1);
        let (n, v) = &self.levels[k];
        let c = self.cell * (1 << k) as f32;
        let g = ((p + Vec2::splat(self.half)) / c).floor();
        if g.x < 0.0 || g.y < 0.0 || g.x >= *n as f32 || g.y >= *n as f32 {
            return None;
        }
        Some(v[g.y as usize * n + g.x as usize])
    }

    /// From the per-cell minimums of the finest level: dilate, then halve upwards.
    fn from_mins(revision: u64, half: f32, cell: f32, n: usize, mins: Vec<f32>) -> Self {
        let mut levels = vec![];
        let (mut n, mut cur) = (n, mins);
        loop {
            levels.push((n, dilate(n, &cur)));
            if n <= 2 {
                break;
            }
            let m = n.div_ceil(2);
            let mut next = vec![f32::MAX; m * m];
            for j in 0..n {
                for i in 0..n {
                    let o = &mut next[(j / 2) * m + i / 2];
                    *o = o.min(cur[j * n + i]);
                }
            }
            (n, cur) = (m, next);
        }
        Self {
            revision: Some(revision),
            half,
            cell,
            levels,
        }
    }
}

/// Each cell's minimum over itself and its eight neighbours.
fn dilate(n: usize, v: &[f32]) -> Vec<f32> {
    let mut rows = vec![f32::MAX; n * n];
    for j in 0..n {
        for i in 0..n {
            let mut m = v[j * n + i];
            if i > 0 {
                m = m.min(v[j * n + i - 1]);
            }
            if i + 1 < n {
                m = m.min(v[j * n + i + 1]);
            }
            rows[j * n + i] = m;
        }
    }
    let mut out = rows.clone();
    for j in 0..n {
        for i in 0..n {
            if j > 0 {
                out[j * n + i] = out[j * n + i].min(rows[(j - 1) * n + i]);
            }
            if j + 1 < n {
                out[j * n + i] = out[j * n + i].min(rows[(j + 1) * n + i]);
            }
        }
    }
    out
}

impl SeaView {
    /// Keeps the picture in step with the terrain (a band of rows a frame) and, when the eye or
    /// the sea moved, casts the rays again. `crest`: the highest the sea surface reaches (m).
    pub fn update(&mut self, terrain: &Terrain, eye: Vec3, crest: f32) {
        let rev = terrain.height_revision;
        if self.picture.revision != Some(rev) {
            // (heights changed: until the new picture is ready, the sea may be anywhere)
            self.visible = true;
            self.cast_at = None;
            self.step_build(terrain);
            if self.picture.revision != Some(rev) {
                return;
            }
        }
        if self
            .cast_at
            .is_some_and(|(at, c)| at.distance(eye) < MOVE && (c - crest).abs() < 0.25)
        {
            return;
        }
        self.cast_at = Some((eye, crest));
        let visible = self.cast(eye, crest);
        if visible != self.visible {
            info!("SEA {} from {eye:.0}", if visible { "in sight" } else { "out of sight" });
        }
        self.visible = visible;
    }

    fn step_build(&mut self, t: &Terrain) {
        let rev = t.height_revision;
        if self.build.as_ref().is_none_or(|b| b.revision != rev) {
            let stride = ((FINEST / t.cell).round() as usize).max(1);
            let n = t.res.div_ceil(stride);
            self.build = Some(Build {
                revision: rev,
                stride,
                n,
                row: 0,
                mins: vec![f32::MAX; n * n],
            });
        }
        let Some(b) = self.build.as_mut() else {
            return;
        };
        let verts = t.n();
        let end = (b.row + ROWS_PER_FRAME).min(verts);
        for vj in b.row..end {
            // (a vertex on a cell's edge belongs to the cells on both sides)
            let cj = (vj / b.stride).min(b.n - 1);
            let cj0 = if vj % b.stride == 0 && cj > 0 { cj - 1 } else { cj };
            for vi in 0..verts {
                let h = t.heights[vj * verts + vi];
                let ci = (vi / b.stride).min(b.n - 1);
                let ci0 = if vi % b.stride == 0 && ci > 0 { ci - 1 } else { ci };
                for j in cj0..=cj {
                    for i in ci0..=ci {
                        let o = &mut b.mins[j * b.n + i];
                        if h < *o {
                            *o = h;
                        }
                    }
                }
            }
        }
        b.row = end;
        if b.row >= verts
            && let Some(b) = self.build.take()
        {
            self.picture =
                Picture::from_mins(b.revision, t.half(), t.cell * b.stride as f32, b.n, b.mins);
            info!(
                "SEA view: terrain picture {}² of {:.0} m cells ready",
                b.n,
                self.picture.cell
            );
        }
    }

    /// Any ray reaching sea before the ground rises above its line of sight? Every bound errs
    /// towards "in sight": the ground's lowest point, the sea's farthest, the hill's nearest.
    fn cast(&self, eye: Vec3, crest: f32) -> bool {
        if self.picture.levels.is_empty() || eye.y <= crest {
            return true;
        }
        let gap = std::f32::consts::TAU / RAYS as f32;
        for r in 0..RAYS {
            let a = r as f32 * gap;
            let dir = Vec2::new(a.cos(), a.sin());
            // the steepest the ground has risen so far (rise over distance from the eye)
            let mut steepest = f32::MIN;
            let mut d = 0.0;
            while d < REACH {
                // (cells at least as big as the gap between rays here: the window they cover
                // reaches 1.5 cells either way)
                let size = (d * gap).max(self.picture.cell);
                let (near, far) = ((d - size * 1.5).max(1.0), d + size * 1.5);
                // off the map is open sea
                let low = self.picture.low(eye.xz() + dir * d, size).unwrap_or(f32::MIN);
                if low < crest && (crest - eye.y) / far >= steepest {
                    return true;
                }
                let rise = low - eye.y;
                steepest = steepest.max(rise / if rise > 0.0 { far } else { near });
                d += size * 0.5;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(n: usize, cell: f32, h: impl Fn(usize, usize) -> f32) -> Picture {
        let mins = (0..n * n).map(|k| h(k % n, k / n)).collect();
        Picture::from_mins(1, n as f32 * cell * 0.5, cell, n, mins)
    }

    fn view(p: Picture) -> SeaView {
        SeaView {
            picture: p,
            ..default()
        }
    }

    #[test]
    fn a_ridge_hides_the_sea_behind_it() {
        // 4 km square: a valley at 50 m, a 300 m ridge round it, sea beyond 1.6 km
        let v = view(picture(500, 8.0, |i, j| {
            let d = Vec2::new(i as f32 - 250.0, j as f32 - 250.0).length() * 8.0;
            if d > 1600.0 {
                -10.0
            } else if d > 1000.0 {
                300.0
            } else {
                50.0
            }
        }));
        assert!(!v.cast(Vec3::new(0.0, 52.0, 0.0), 1.0));
        // high above the ridge the sea comes into view
        assert!(v.cast(Vec3::new(0.0, 700.0, 0.0), 1.0));
    }

    #[test]
    fn a_narrow_inlet_is_not_missed() {
        // flat land at 2 m, one 8 m wide channel of sea 600 m off
        let v = view(picture(400, 8.0, |i, j| if i == 275 && j > 150 { -5.0 } else { 2.0 }));
        assert!(v.cast(Vec3::new(0.0, 200.0, 0.0), 1.0));
    }

    #[test]
    fn under_the_crest_the_sea_is_always_there() {
        let v = view(picture(64, 8.0, |_, _| 100.0));
        assert!(v.cast(Vec3::new(0.0, 0.5, 0.0), 1.0));
    }
}
