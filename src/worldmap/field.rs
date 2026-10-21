//! Raster helpers for the map generator: scalar grids with bicubic sampling and blurs, an exact
//! distance transform, marching-squares iso-lines chained into polylines, line smoothing and
//! simplification, and Poisson-disc scattering.

use std::collections::HashMap;

use bevy::math::Vec2;

/// A row-major scalar grid.
#[derive(Clone)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Grid {
    pub fn new(w: usize, h: usize, fill: f32) -> Self {
        Self {
            w,
            h,
            v: vec![fill; w * h],
        }
    }
    pub fn from_vec(w: usize, h: usize, v: Vec<f32>) -> Self {
        debug_assert_eq!(v.len(), w * h);
        Self { w, h, v }
    }
    #[inline]
    pub fn at(&self, i: isize, j: isize) -> f32 {
        let i = i.clamp(0, self.w as isize - 1) as usize;
        let j = j.clamp(0, self.h as isize - 1) as usize;
        self.v[j * self.w + i]
    }
    /// Bilinear sample at grid coordinates (clamped).
    pub fn bilinear(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (i, j) = (x.floor() as isize, y.floor() as isize);
        let (fx, fy) = (x - i as f32, y - j as f32);
        let a = self.at(i, j) + (self.at(i + 1, j) - self.at(i, j)) * fx;
        let b = self.at(i, j + 1) + (self.at(i + 1, j + 1) - self.at(i, j + 1)) * fx;
        a + (b - a) * fy
    }
    /// Catmull-Rom bicubic sample at grid coordinates (clamped): smooth slopes for hillshading.
    pub fn bicubic(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (i, j) = (x.floor() as isize, y.floor() as isize);
        let (fx, fy) = (x - i as f32, y - j as f32);
        let wx = cr_weights(fx);
        let wy = cr_weights(fy);
        let mut s = 0.0;
        for (b, wyb) in wy.iter().enumerate() {
            let jj = j - 1 + b as isize;
            let row = self.at(i - 1, jj) * wx[0]
                + self.at(i, jj) * wx[1]
                + self.at(i + 1, jj) * wx[2]
                + self.at(i + 2, jj) * wx[3];
            s += row * wyb;
        }
        s
    }
    /// Separable box blur, `passes` times (three passes ≈ gaussian).
    pub fn blur(&mut self, radius: usize, passes: usize) {
        if radius == 0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0.0f32; w * h];
        for _ in 0..passes {
            box_1d(&self.v, &mut tmp, w, h, radius, true);
            box_1d(&tmp, &mut self.v, w, h, radius, false);
        }
    }
    /// Resampled to `w × h` (bicubic).
    pub fn resized(&self, w: usize, h: usize) -> Grid {
        let sx = (self.w - 1) as f32 / (w - 1).max(1) as f32;
        let sy = (self.h - 1) as f32 / (h - 1).max(1) as f32;
        let mut v = vec![0.0; w * h];
        use rayon::prelude::*;
        v.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                *o = self.bicubic(i as f32 * sx, j as f32 * sy);
            }
        });
        Grid { w, h, v }
    }
}

fn cr_weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

fn box_1d(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    use rayon::prelude::*;
    let norm = 1.0 / (2 * r + 1) as f32;
    if horizontal {
        dst.par_chunks_mut(w).enumerate().for_each(|(j, out)| {
            let row = &src[j * w..(j + 1) * w];
            let get = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
            let mut acc: f32 = (-(r as isize)..=r as isize).map(get).sum();
            for (i, o) in out.iter_mut().enumerate() {
                *o = acc * norm;
                acc += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
            }
        });
    } else {
        // column pass, rows in parallel blocks of columns
        let cols: Vec<Vec<f32>> = (0..w)
            .into_par_iter()
            .map(|i| {
                let get = |j: isize| src[j.clamp(0, h as isize - 1) as usize * w + i];
                let mut acc: f32 = (-(r as isize)..=r as isize).map(get).sum();
                let mut col = vec![0.0; h];
                for (j, c) in col.iter_mut().enumerate() {
                    *c = acc * norm;
                    acc += get(j as isize + r as isize + 1) - get(j as isize - r as isize);
                }
                col
            })
            .collect();
        for (i, col) in cols.iter().enumerate() {
            for (j, v) in col.iter().enumerate() {
                dst[j * w + i] = *v;
            }
        }
    }
}

/// Exact Euclidean distance (in cells) from every cell to the nearest `true` cell
/// (Felzenszwalb & Huttenlocher). Cells far from any seed get a large finite value.
pub fn edt(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    use rayon::prelude::*;
    const INF: f32 = 1e12;
    let mut f: Vec<f32> = mask.iter().map(|m| if *m { 0.0 } else { INF }).collect();
    // columns
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|i| {
            let col: Vec<f32> = (0..h).map(|j| f[j * w + i]).collect();
            dt_1d(&col)
        })
        .collect();
    for (i, col) in cols.iter().enumerate() {
        for (j, v) in col.iter().enumerate() {
            f[j * w + i] = *v;
        }
    }
    // rows
    f.par_chunks_mut(w).for_each(|row| {
        let r = dt_1d(row);
        row.copy_from_slice(&r);
    });
    f.par_iter_mut().for_each(|v| *v = v.sqrt());
    f
}

/// 1D squared distance transform of sampled function `f` (lower envelope of parabolas).
fn dt_1d(f: &[f32]) -> Vec<f32> {
    let n = f.len();
    let f: Vec<f64> = f
        .iter()
        .map(|v| if *v >= 1e11 { 1e20 } else { *v as f64 })
        .collect();
    let mut v = vec![0usize; n];
    let mut z = vec![0.0f64; n + 1];
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let inter = |q: usize, p: usize| {
        ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * (q as f64 - p as f64))
    };
    for q in 1..n {
        let mut s = inter(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = inter(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    let mut d = vec![0.0f32; n];
    for (q, dq) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *dq = ((q as f64 - p as f64).powi(2) + f[p]).min(1e12) as f32;
    }
    d
}

/// A polyline in some 2D space; `closed` loops repeat no point.
#[derive(Clone, Debug)]
pub struct Line {
    pub pts: Vec<Vec2>,
    pub closed: bool,
}

impl Line {
    pub fn length(&self) -> f32 {
        let mut l: f32 = self.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.closed && self.pts.len() > 2 {
            l += self.pts[self.pts.len() - 1].distance(self.pts[0]);
        }
        l
    }
}

/// Iso-lines of `g` at `iso` (marching squares), chained into polylines in grid coordinates.
/// Values above `iso` count as inside. Lines ending at the grid border stay open.
pub fn contours(g: &Grid, iso: f32) -> Vec<Line> {
    let (w, h) = (g.w, g.h);
    if w < 2 || h < 2 {
        return vec![];
    }
    // edge keys: horizontal edge (i,j)-(i+1,j) = 2*(j*w+i), vertical (i,j)-(i,j+1) = 2*(j*w+i)+1
    let hkey = |i: usize, j: usize| 2 * (j * w + i);
    let vkey = |i: usize, j: usize| 2 * (j * w + i) + 1;
    let val = |i: usize, j: usize| g.v[j * w + i];
    let mut points: HashMap<usize, Vec2> = HashMap::new();
    let mut point = |key: usize, a: (usize, usize), b: (usize, usize)| {
        points.entry(key).or_insert_with(|| {
            let (va, vb) = (val(a.0, a.1), val(b.0, b.1));
            let t = ((iso - va) / (vb - va)).clamp(0.0, 1.0);
            Vec2::new(
                a.0 as f32 + (b.0 as f32 - a.0 as f32) * t,
                a.1 as f32 + (b.1 as f32 - a.1 as f32) * t,
            )
        });
        key
    };
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            let (a, b, c, d) = (val(i, j), val(i + 1, j), val(i + 1, j + 1), val(i, j + 1));
            let case = ((a > iso) as u8)
                | (((b > iso) as u8) << 1)
                | (((c > iso) as u8) << 2)
                | (((d > iso) as u8) << 3);
            if case == 0 || case == 15 {
                continue;
            }
            // edges: top (a-b), right (b-c), bottom (d-c), left (a-d)
            let t = point(hkey(i, j), (i, j), (i + 1, j));
            let r = point(vkey(i + 1, j), (i + 1, j), (i + 1, j + 1));
            let bo = point(hkey(i, j + 1), (i, j + 1), (i + 1, j + 1));
            let l = point(vkey(i, j), (i, j), (i, j + 1));
            let centre_in = (a + b + c + d) * 0.25 > iso;
            match case {
                1 | 14 => segs.push((l, t)),
                2 | 13 => segs.push((t, r)),
                3 | 12 => segs.push((l, r)),
                4 | 11 => segs.push((r, bo)),
                6 | 9 => segs.push((t, bo)),
                7 | 8 => segs.push((l, bo)),
                5 => {
                    if centre_in {
                        segs.push((l, bo));
                        segs.push((t, r));
                    } else {
                        segs.push((l, t));
                        segs.push((r, bo));
                    }
                }
                10 => {
                    if centre_in {
                        segs.push((l, t));
                        segs.push((r, bo));
                    } else {
                        segs.push((t, r));
                        segs.push((l, bo));
                    }
                }
                _ => {}
            }
        }
    }
    chain(&segs, &points)
}

/// Chains undirected segments between keyed points into polylines.
pub fn chain(segs: &[(usize, usize)], points: &HashMap<usize, Vec2>) -> Vec<Line> {
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::with_capacity(segs.len() * 2);
    for (k, (a, b)) in segs.iter().enumerate() {
        adj.entry(*a).or_default().push(k);
        adj.entry(*b).or_default().push(k);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    let other = |s: usize, p: usize| if segs[s].0 == p { segs[s].1 } else { segs[s].0 };
    // walk from open ends first, then the remaining loops
    let mut starts: Vec<usize> = adj
        .iter()
        .filter(|(_, v)| v.len() == 1)
        .map(|(k, _)| *k)
        .collect();
    starts.sort_unstable();
    let mut order: Vec<usize> = starts;
    let mut rest: Vec<usize> = segs.iter().map(|s| s.0).collect();
    rest.sort_unstable();
    rest.dedup();
    order.extend(rest);
    for start in order {
        while let Some(&first) = adj.get(&start).and_then(|v| v.iter().find(|s| !used[**s])) {
            let mut keys = vec![start];
            let mut cur = start;
            let mut seg = first;
            loop {
                used[seg] = true;
                cur = other(seg, cur);
                keys.push(cur);
                match adj.get(&cur).and_then(|v| v.iter().find(|s| !used[**s])) {
                    Some(&n) => seg = n,
                    None => break,
                }
            }
            let closed = keys.len() > 3 && keys[0] == *keys.last().unwrap();
            if closed {
                keys.pop();
            }
            out.push(Line {
                pts: keys.iter().map(|k| points[k]).collect(),
                closed,
            });
        }
    }
    out
}

/// Chaikin corner cutting; open lines keep their end points.
pub fn chaikin(pts: &[Vec2], closed: bool, iterations: usize) -> Vec<Vec2> {
    let mut p = pts.to_vec();
    for _ in 0..iterations {
        let n = p.len();
        if n < 3 {
            return p;
        }
        let mut q = Vec::with_capacity(n * 2);
        if !closed {
            q.push(p[0]);
        }
        let segs = if closed { n } else { n - 1 };
        for i in 0..segs {
            let (a, b) = (p[i], p[(i + 1) % n]);
            q.push(a * 0.75 + b * 0.25);
            q.push(a * 0.25 + b * 0.75);
        }
        if !closed {
            q.push(p[n - 1]);
        }
        p = q;
    }
    p
}

/// Douglas–Peucker simplification.
pub fn simplify(pts: &[Vec2], eps: f32) -> Vec<Vec2> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (pts[a], pts[b]);
        let ab = pb - pa;
        let len2 = ab.length_squared().max(1e-9);
        let mut best = (0.0f32, 0usize);
        for (k, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let t = ((*p - pa).dot(ab) / len2).clamp(0.0, 1.0);
            let d = p.distance(pa + ab * t);
            if d > best.0 {
                best = (d, k);
            }
        }
        if best.0 > eps {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

/// Resamples a polyline to points `step` apart along its length.
pub fn resample(pts: &[Vec2], step: f32) -> Vec<Vec2> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        let mut t = step - carry;
        while t <= len {
            out.push(a.lerp(b, t / len.max(1e-6)));
            t += step;
        }
        carry = len - (t - step);
    }
    if out
        .last()
        .is_some_and(|l| l.distance(*pts.last().unwrap()) > step * 0.3)
    {
        out.push(*pts.last().unwrap());
    }
    out
}

/// Point and unit tangent at arc length `s` along an open polyline.
pub fn point_at(pts: &[Vec2], mut s: f32) -> (Vec2, Vec2) {
    for w in pts.windows(2) {
        let len = w[0].distance(w[1]);
        if s <= len || len <= 0.0 {
            let d = (w[1] - w[0]).normalize_or(Vec2::X);
            return (w[0] + d * s.max(0.0), d);
        }
        s -= len;
    }
    let n = pts.len();
    let d = (pts[n - 1] - pts[n.saturating_sub(2)]).normalize_or(Vec2::X);
    (pts[n - 1], d)
}

/// Poisson-disc scatter over `[0, w) × [0, h)` with spacing `r(p)` (blue noise, dart throwing
/// on a jittered candidate lattice); `accept` filters candidates. Deterministic for a seed.
pub fn poisson(
    w: f32,
    h: f32,
    r_min: f32,
    r: impl Fn(Vec2) -> f32,
    accept: impl Fn(Vec2) -> bool,
    seed: u64,
) -> Vec<Vec2> {
    let mut rng = crate::noise::Rng::new(seed);
    let cell = r_min / std::f32::consts::SQRT_2;
    let gw = (w / cell).ceil() as usize + 1;
    let gh = (h / cell).ceil() as usize + 1;
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); gw * gh];
    let mut out: Vec<Vec2> = Vec::new();
    // candidates: a jittered lattice at half the minimum spacing, visited in random order
    let step = r_min * 0.5;
    let nx = (w / step) as usize;
    let ny = (h / step) as usize;
    let mut cands: Vec<Vec2> = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            cands.push(Vec2::new(
                (i as f32 + rng.f32()) * step,
                (j as f32 + rng.f32()) * step,
            ));
        }
    }
    for k in (1..cands.len()).rev() {
        let s = (rng.next_u32() as usize) % (k + 1);
        cands.swap(k, s);
    }
    for p in cands {
        if !accept(p) {
            continue;
        }
        let rp = r(p).max(r_min);
        let (ci, cj) = ((p.x / cell) as isize, (p.y / cell) as isize);
        let reach = (rp / cell).ceil() as isize + 1;
        let mut ok = true;
        'scan: for dj in -reach..=reach {
            for di in -reach..=reach {
                let (i, j) = (ci + di, cj + dj);
                if i < 0 || j < 0 || i >= gw as isize || j >= gh as isize {
                    continue;
                }
                for &q in &grid[j as usize * gw + i as usize] {
                    if out[q as usize].distance_squared(p) < rp * rp {
                        ok = false;
                        break 'scan;
                    }
                }
            }
        }
        if ok {
            grid[cj as usize * gw + ci as usize].push(out.len() as u32);
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edt_matches_brute_force() {
        let (w, h) = (23, 17);
        let mask: Vec<bool> = (0..w * h).map(|k| (k * 7919) % 53 == 0).collect();
        let d = edt(&mask, w, h);
        for j in 0..h {
            for i in 0..w {
                let mut best = f32::MAX;
                for (k, m) in mask.iter().enumerate() {
                    if *m {
                        let (x, y) = ((k % w) as f32, (k / w) as f32);
                        best = best.min(((x - i as f32).powi(2) + (y - j as f32).powi(2)).sqrt());
                    }
                }
                assert!(
                    (d[j * w + i] - best).abs() < 1e-3,
                    "{i},{j}: {} vs {best}",
                    d[j * w + i]
                );
            }
        }
    }

    #[test]
    fn contour_of_a_disc_is_one_closed_loop() {
        let n = 40;
        let g = Grid::from_vec(
            n,
            n,
            (0..n * n)
                .map(|k| {
                    12.0 - Vec2::new((k % n) as f32, (k / n) as f32).distance(Vec2::splat(20.0))
                })
                .collect(),
        );
        let lines = contours(&g, 0.0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        let r = lines[0]
            .pts
            .iter()
            .map(|p| p.distance(Vec2::splat(20.0)))
            .sum::<f32>()
            / lines[0].pts.len() as f32;
        assert!((r - 12.0).abs() < 0.2, "radius {r}");
    }
}
