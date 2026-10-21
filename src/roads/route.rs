//! Auto routing: A* from A to B over a grid of the terrain (4 m or coarser for long routes) with
//! 16 headings. Every step costs its length times a grade factor (cheap up to the kind's design
//! grade, steeply more beyond, so steep ground is climbed in switchbacks), plus cross-slope (deep
//! cuts), water (a bridge per metre and per crossing, so rivers are crossed square and narrow) and
//! turning (squared heading change, so bends are gentle unless the slope demands a hairpin). The
//! grid path is simplified (Douglas–Peucker) into control points for the road spline.
//!
//! `prepare` samples the ground on the main thread; `solve` is pure and runs in the background.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use bevy::prelude::*;

use super::RoadKind;
use super::ground::Base;
use crate::water::WaterBodies;

const DIRS: [(i32, i32); 16] = [
    (1, 0),
    (2, 1),
    (1, 1),
    (1, 2),
    (0, 1),
    (-1, 2),
    (-1, 1),
    (-2, 1),
    (-1, 0),
    (-2, -1),
    (-1, -1),
    (-1, -2),
    (0, -1),
    (1, -2),
    (1, -1),
    (2, -1),
];
/// Give up after this many expanded states (the grid is sized well below it).
const MAX_EXPAND: usize = 6_000_000;

/// The ground around A and B, sampled for the search.
pub struct RouteInput {
    origin: Vec2,
    cell: f32,
    nx: usize,
    nz: usize,
    /// smoothed ground height per node
    h: Vec<f32>,
    /// water depth per node (0 on land)
    wet: Vec<f32>,
    /// below the sea (open water, not a river or lake)
    sea: Vec<bool>,
    max_grade: f32,
    /// turning cost per (heading step)² per metre of cell
    turn: f32,
    from: Vec2,
    to: Vec2,
}

pub fn prepare(
    base: &Base,
    water: &WaterBodies,
    from: Vec2,
    to: Vec2,
    kind: RoadKind,
    width: f32,
) -> RouteInput {
    let t = base.t;
    let dist = from.distance(to);
    // wide roads turn on a coarser grid: wider hairpins, switchback legs further apart
    let cell = (dist / 240.0).max(width).clamp(t.cell, t.cell * 4.0);
    let margin = (dist * 0.45).clamp(160.0, 1500.0);
    let half = t.half() - t.cell;
    let lo = (from.min(to) - Vec2::splat(margin)).max(Vec2::splat(-half));
    let hi = (from.max(to) + Vec2::splat(margin)).min(Vec2::splat(half));
    let nx = ((hi.x - lo.x) / cell).ceil().max(2.0) as usize + 1;
    let nz = ((hi.y - lo.y) / cell).ceil().max(2.0) as usize + 1;
    let mut h = vec![0.0f32; nx * nz];
    let mut wet = vec![0.0f32; nx * nz];
    let mut sea = vec![false; nx * nz];
    for j in 0..nz {
        for i in 0..nx {
            let p = lo + Vec2::new(i as f32, j as f32) * cell;
            let g = base.at(p.x, p.y);
            let k = j * nx + i;
            h[k] = g;
            let lake = water.water_level(Vec3::new(p.x, g, p.y));
            let surf = lake.unwrap_or(f32::NEG_INFINITY).max(t.sea_level);
            wet[k] = (surf - g).max(0.0);
            sea[k] = lake.is_none() && g < t.sea_level - 1.0;
        }
    }
    // smooth once (3×3): the road profile is smoothed anyway, small bumps must not steer it
    for _ in 0..1 {
        let src = h.clone();
        for j in 0..nz {
            for i in 0..nx {
                let (mut s, mut c) = (0.0, 0.0);
                for dj in -1i32..=1 {
                    for di in -1i32..=1 {
                        let (a, b) = (i as i32 + di, j as i32 + dj);
                        if a >= 0 && b >= 0 && (a as usize) < nx && (b as usize) < nz {
                            s += src[b as usize * nx + a as usize];
                            c += 1.0;
                        }
                    }
                }
                h[j * nx + i] = s / c;
            }
        }
    }
    RouteInput {
        origin: lo,
        cell,
        nx,
        nz,
        h,
        wet,
        sea,
        max_grade: kind.style().max_grade,
        turn: 0.8 + 0.12 * width,
        from,
        to,
    }
}

/// The cheapest route as road control points (xz), or None if B can't be reached.
pub fn solve(inp: &RouteInput) -> Option<Vec<Vec2>> {
    let (nx, nz, g) = (inp.nx, inp.nz, inp.cell);
    let node = |p: Vec2| -> usize {
        let q = ((p - inp.origin) / g).round();
        (q.y.clamp(0.0, (nz - 1) as f32) as usize) * nx + q.x.clamp(0.0, (nx - 1) as f32) as usize
    };
    let (start, goal) = (node(inp.from), node(inp.to));
    let (gx, gz) = ((goal % nx) as f32, (goal / nx) as f32);
    let heur =
        |n: usize| -> f32 { Vec2::new((n % nx) as f32 - gx, (n / nx) as f32 - gz).length() * g };
    // cross slope at a node in a heading (rise per metre sideways)
    let grad = |n: usize| -> Vec2 {
        let (i, j) = (n % nx, n / nx);
        let hx = inp.h[j * nx + (i + 1).min(nx - 1)] - inp.h[j * nx + i.saturating_sub(1)];
        let hz = inp.h[(j + 1).min(nz - 1) * nx + i] - inp.h[j.saturating_sub(1) * nx + i];
        Vec2::new(hx, hz) / (2.0 * g)
    };
    let states = nx * nz * 16;
    let mut cost = vec![f32::INFINITY; states];
    let mut parent = vec![u32::MAX; states];
    let mut heap: BinaryHeap<(Reverse<u32>, u32)> = BinaryHeap::new();
    let key = |f: f32| Reverse(f.max(0.0).to_bits());
    for k in 0..16 {
        let s = start * 16 + k;
        cost[s] = 0.0;
        heap.push((key(heur(start)), s as u32));
    }
    let mut expanded = 0usize;
    let mut found = None;
    while let Some((Reverse(f), s)) = heap.pop() {
        let s = s as usize;
        let (n, k) = (s / 16, s % 16);
        // a stale entry (this state was reached more cheaply since)
        if f32::from_bits(f) > cost[s] + heur(n) + 1e-3 {
            continue;
        }
        if n == goal {
            found = Some(s);
            break;
        }
        expanded += 1;
        if expanded > MAX_EXPAND {
            return None;
        }
        let c = cost[s];
        let (i, j) = ((n % nx) as i32, (n / nx) as i32);
        for (d, (di, dj)) in DIRS.iter().enumerate() {
            let (a, b) = (i + di, j + dj);
            if a < 0 || b < 0 || a as usize >= nx || b as usize >= nz {
                continue;
            }
            let m = b as usize * nx + a as usize;
            let len = Vec2::new(*di as f32, *dj as f32).length() * g;
            // the steeper half of the step counts (a ridge or dip between the nodes)
            let mid = bilinear(
                &inp.h,
                nx,
                i as f32 + *di as f32 * 0.5,
                j as f32 + *dj as f32 * 0.5,
            );
            let grade = ((inp.h[m] - inp.h[n]).abs() / len)
                .max((mid - inp.h[n]).abs().max((inp.h[m] - mid).abs()) * 2.0 / len);
            let gp = grade / inp.max_grade;
            let mut mult = if gp <= 1.0 {
                1.0 + 0.5 * gp * gp
            } else {
                1.5 + 6.0 * (gp - 1.0) + 20.0 * (gp - 1.0).powi(2)
            };
            let dir = Vec2::new(*di as f32, *dj as f32).normalize();
            let cross = grad(m).dot(dir.perp()).abs();
            mult += 3.0 * (cross - 0.2).max(0.0);
            // water: bridges cost per metre, plus each crossing; open sea and deep water far more
            let w = inp.wet[m].max(inp.wet[n]);
            if w > 0.15 {
                mult +=
                    10.0 + if w > 6.0 { 30.0 } else { 0.0 } + if inp.sea[m] { 30.0 } else { 0.0 };
            }
            let mut step = len * mult;
            if inp.wet[m] > 0.15 && inp.wet[n] <= 0.15 {
                step += 40.0;
            }
            let turn = {
                let dk = (d as i32 - k as i32).unsigned_abs() as f32;
                dk.min(16.0 - dk)
            };
            if n != start {
                step += turn * turn * g * inp.turn;
            }
            let ns = m * 16 + d;
            let nc = c + step;
            if nc < cost[ns] {
                cost[ns] = nc;
                parent[ns] = s as u32;
                heap.push((key(nc + heur(m)), ns as u32));
            }
        }
    }
    let mut s = found?;
    let mut path = vec![];
    loop {
        let n = s / 16;
        path.push(inp.origin + Vec2::new((n % nx) as f32, (n / nx) as f32) * g);
        let p = parent[s];
        if p == u32::MAX {
            break;
        }
        s = p as usize;
    }
    path.reverse();
    if let Some(f) = path.first_mut() {
        *f = inp.from;
    }
    if let Some(l) = path.last_mut() {
        *l = inp.to;
    }
    let mut pts = douglas_peucker(&path, g * 0.5);
    // long straight legs get extra points so the spline keeps to the route between them
    let mut out = vec![pts[0]];
    for w in pts.windows(2) {
        let n = (w[0].distance(w[1]) / 40.0).ceil().max(1.0) as usize;
        for k in 1..=n {
            out.push(w[0].lerp(w[1], k as f32 / n as f32));
        }
    }
    pts = out;
    info!(
        "ROADS route: {} grid steps -> {} control points ({} states expanded, cell {g:.1} m)",
        path.len(),
        pts.len(),
        expanded
    );
    Some(pts)
}

/// Bilinear sample of a node grid at fractional node coordinates.
fn bilinear(v: &[f32], nx: usize, x: f32, z: f32) -> f32 {
    let nz = v.len() / nx;
    let (x0, z0) = (
        (x.floor() as usize).min(nx - 1),
        (z.floor() as usize).min(nz - 1),
    );
    let (x1, z1) = ((x0 + 1).min(nx - 1), (z0 + 1).min(nz - 1));
    let (fx, fz) = (x - x0 as f32, z - z0 as f32);
    let a = v[z0 * nx + x0] + (v[z0 * nx + x1] - v[z0 * nx + x0]) * fx;
    let b = v[z1 * nx + x0] + (v[z1 * nx + x1] - v[z1 * nx + x0]) * fx;
    a + (b - a) * fz
}

fn douglas_peucker(p: &[Vec2], tol: f32) -> Vec<Vec2> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (p[a], p[b]);
        let ab = pb - pa;
        let (mut best, mut bi) = (0.0f32, 0usize);
        for (i, q) in p.iter().enumerate().take(b).skip(a + 1) {
            let u = ((*q - pa).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            let d = q.distance(pa + ab * u);
            if d > best {
                best = d;
                bi = i;
            }
        }
        if best > tol {
            keep[bi] = true;
            stack.push((a, bi));
            stack.push((bi, b));
        }
    }
    p.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(q, _)| *q)
        .collect()
}
