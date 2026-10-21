//! Searches over the walkability grid: travel time from one or many sources (Dijkstra on the
//! 16-neighbourhood with direction-dependent costs), directed reachability flood fills, and
//! connected regions of cells (pockets, dead zones).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::grid::{NEIGHBOURS, STEP_LEN, WalkGrid};

#[inline]
fn key(t: f32, k: usize) -> u64 {
    // positive floats order like their bit patterns
    ((t.to_bits() as u64) << 32) | k as u64
}

/// Seconds from the nearest source to every cell (infinity = unreachable). Progress goes to
/// `progress` in ‰ of passable cells; `cancel` stops early (the result is then partial).
pub fn travel_times(
    g: &WalkGrid,
    sources: &[usize],
    progress: &AtomicU32,
    cancel: &AtomicBool,
) -> Vec<f32> {
    let n = g.n;
    let mut t = vec![f32::INFINITY; g.len()];
    let passable = g.speed.iter().filter(|v| **v > 0.0).count().max(1);
    let mut heap = BinaryHeap::with_capacity(1 << 16);
    for &s in sources {
        if s < t.len() && g.passable(s) && t[s] > 0.0 {
            t[s] = 0.0;
            heap.push(Reverse(key(0.0, s)));
        }
    }
    let mut settled = 0usize;
    while let Some(Reverse(kv)) = heap.pop() {
        let a = (kv & 0xffff_ffff) as usize;
        let ta = f32::from_bits((kv >> 32) as u32);
        if ta > t[a] {
            continue;
        }
        settled += 1;
        if settled & 0xffff == 0 {
            progress.store(
                (settled * 1000 / passable).min(999) as u32,
                Ordering::Relaxed,
            );
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        let (ai, aj) = (a % n, a / n);
        for o in 0..16 {
            let Some(b) = g.step(ai, aj, o) else { continue };
            let nt = ta + g.edge_time(a, b, STEP_LEN[o]);
            if nt < t[b] {
                t[b] = nt;
                heap.push(Reverse(key(nt, b)));
            }
        }
    }
    progress.store(1000, Ordering::Relaxed);
    t
}

/// Cells reachable from `start` (`forward`), or cells that can reach `start` (backward: walking
/// out of a pit counts against the steep-slope rule). Marks `bit` in `flags`.
pub fn flood(g: &WalkGrid, start: usize, forward: bool, flags: &mut [u8], bit: u8) {
    if start >= g.len() || !g.passable(start) {
        return;
    }
    let n = g.n;
    let mut stack = vec![start];
    flags[start] |= bit;
    while let Some(c) = stack.pop() {
        let (ci, cj) = (c % n, c / n);
        for o in 0..16 {
            if forward {
                let Some(b) = g.step(ci, cj, o) else { continue };
                if flags[b] & bit == 0 && g.edge_time(c, b, STEP_LEN[o]).is_finite() {
                    flags[b] |= bit;
                    stack.push(b);
                }
            } else {
                // predecessors: a cell `a` whose move `o` lands on `c`
                let (dx, dz) = NEIGHBOURS[o];
                let (ai, aj) = (ci as i32 - dx, cj as i32 - dz);
                if ai < 0 || aj < 0 || ai >= n as i32 || aj >= n as i32 {
                    continue;
                }
                let a = aj as usize * n + ai as usize;
                if flags[a] & bit != 0 || g.step(ai as usize, aj as usize, o) != Some(c) {
                    continue;
                }
                if g.edge_time(a, c, STEP_LEN[o]).is_finite() {
                    flags[a] |= bit;
                    stack.push(a);
                }
            }
        }
    }
}

/// A connected (8-neighbour) group of cells.
pub struct Region {
    pub cells: usize,
    /// The member cell closest to the group's centroid (a good place to look at).
    pub center: usize,
}

/// Groups of cells matching `pred`, largest first, keeping groups of at least `min_cells`.
/// Also returns the number of matching cells in all groups.
pub fn regions(n: usize, pred: impl Fn(usize) -> bool, min_cells: usize) -> (Vec<Region>, usize) {
    let mut seen = vec![false; n * n];
    let mut out = Vec::new();
    let mut members: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let mut total = 0;
    for s in 0..n * n {
        if seen[s] || !pred(s) {
            continue;
        }
        members.clear();
        seen[s] = true;
        stack.push(s as u32);
        while let Some(c) = stack.pop() {
            members.push(c);
            let (ci, cj) = ((c as usize % n) as i32, (c as usize / n) as i32);
            for (dx, dz) in &NEIGHBOURS[..8] {
                let (i, j) = (ci + dx, cj + dz);
                if i < 0 || j < 0 || i >= n as i32 || j >= n as i32 {
                    continue;
                }
                let k = j as usize * n + i as usize;
                if !seen[k] && pred(k) {
                    seen[k] = true;
                    stack.push(k as u32);
                }
            }
        }
        total += members.len();
        if members.len() < min_cells {
            continue;
        }
        let (mut sx, mut sz) = (0.0f64, 0.0f64);
        for &m in &members {
            sx += (m as usize % n) as f64;
            sz += (m as usize / n) as f64;
        }
        let (mx, mz) = (sx / members.len() as f64, sz / members.len() as f64);
        let d = |m: u32| {
            ((m as usize % n) as f64 - mx).powi(2) + ((m as usize / n) as f64 - mz).powi(2)
        };
        let center = members
            .iter()
            .min_by(|a, b| d(**a).total_cmp(&d(**b)))
            .map(|m| *m as usize)
            .unwrap_or(members[0] as usize);
        out.push(Region {
            cells: members.len(),
            center,
        });
    }
    out.sort_by(|a, b| b.cells.cmp(&a.cells));
    (out, total)
}

/// "1 m 42 s", "45 s", "1 h 05 m".
pub fn fmt_time(s: f32) -> String {
    if !s.is_finite() {
        return "unreachable".into();
    }
    let s = s.max(0.0).round() as u32;
    if s >= 3600 {
        format!("{} h {:02} m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{} m {:02} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// Percentile of the finite values of `v` selected by `keep` (p in 0..1).
pub fn percentile(v: &[f32], keep: impl Fn(usize) -> bool, p: f32) -> Option<f32> {
    let mut vals: Vec<f32> = v
        .iter()
        .enumerate()
        .filter(|(k, t)| t.is_finite() && keep(*k))
        .map(|(_, t)| *t)
        .collect();
    if vals.is_empty() {
        return None;
    }
    let i = ((vals.len() - 1) as f32 * p.clamp(0.0, 1.0)).round() as usize;
    let (_, x, _) = vals.select_nth_unstable_by(i, |a, b| a.total_cmp(b));
    Some(*x)
}
