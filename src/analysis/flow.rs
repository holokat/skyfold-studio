//! Level flow: where zones touch across ground players can walk, and how big the level jump is
//! there. A starter zone with an open border onto a much higher zone lets new players wander
//! into monsters they can't fight; designers fix it with a barrier (river, cliff, wall) or by
//! bringing the level ranges closer.

use std::collections::HashMap;

use bevy::prelude::*;

use super::grid::{Class, WalkGrid};
use super::job::ZoneInfo;

/// One stretch of open border between two zones, for a flag on the map.
pub struct BorderSegment {
    /// Middle of the stretch (on the ground).
    pub pos: Vec3,
    /// Across the border, from the lower-level zone into the higher one (xz, unit).
    pub dir: Vec2,
    pub len_m: f32,
}

/// Two zones that share a border.
pub struct ZoneBorder {
    /// Lower-level zone first.
    pub low: u8,
    pub high: u8,
    /// Difference between the zones' average levels.
    pub jump: f32,
    /// Border players can walk across (m).
    pub open_m: f32,
    /// Border closed by cliffs, deep water or objects (m).
    pub closed_m: f32,
    pub flagged: bool,
    /// Longest open stretches first (flagged borders only).
    pub segments: Vec<BorderSegment>,
}

pub struct FlowResult {
    pub threshold: u32,
    /// Flagged first, then by jump.
    pub borders: Vec<ZoneBorder>,
    /// Per cell: 0 = not a border, 1 = closed border, 2 = open border, 3 = open border with a big jump.
    pub border_map: Vec<u8>,
}

fn mid(z: &ZoneInfo) -> f32 {
    (z.level_min + z.level_max.max(z.level_min)) as f32 * 0.5
}

/// Can someone on foot cross between neighbouring cells `a` and `b` (either way)?
fn open(g: &WalkGrid, a: usize, b: usize) -> bool {
    let walkish = |k: usize| {
        matches!(g.class[k], Class::Walk | Class::Shallow | Class::Steep) && g.passable(k)
    };
    walkish(a)
        && walkish(b)
        && (g.edge_time(a, b, 1.0).is_finite() || g.edge_time(b, a, 1.0).is_finite())
}

pub fn compute(g: &WalkGrid, zones: &[ZoneInfo], threshold: u32) -> FlowResult {
    let n = g.n;
    let info: HashMap<u8, &ZoneInfo> = zones.iter().map(|z| (z.id, z)).collect();
    #[derive(Default)]
    struct Acc {
        open: usize,
        closed: usize,
        /// (low-zone cell, high-zone cell) of each open edge
        edges: Vec<(u32, u32)>,
    }
    let mut acc: HashMap<(u8, u8), Acc> = HashMap::new();
    let mut border_map = vec![0u8; n * n];
    let jump_of = |a: u8, b: u8| match (info.get(&a), info.get(&b)) {
        (Some(za), Some(zb)) => (mid(zb) - mid(za)).abs(),
        _ => 0.0,
    };
    for j in 0..n {
        for i in 0..n {
            let a = j * n + i;
            let za = g.zone[a];
            if za == 0 {
                continue;
            }
            for b in [(i + 1 < n).then(|| a + 1), (j + 1 < n).then(|| a + n)]
                .into_iter()
                .flatten()
            {
                let zb = g.zone[b];
                if zb == 0 || zb == za || !info.contains_key(&za) || !info.contains_key(&zb) {
                    continue;
                }
                // order the pair low → high level
                let (lo, hi, cl, ch) = if mid(info[&za]) <= mid(info[&zb]) {
                    (za, zb, a, b)
                } else {
                    (zb, za, b, a)
                };
                let e = acc.entry((lo, hi)).or_default();
                if open(g, a, b) {
                    e.open += 1;
                    let mark = if jump_of(lo, hi) > threshold as f32 {
                        3
                    } else {
                        2
                    };
                    border_map[a] = border_map[a].max(mark);
                    border_map[b] = border_map[b].max(mark);
                    e.edges.push((cl as u32, ch as u32));
                } else {
                    e.closed += 1;
                    border_map[a] = border_map[a].max(1);
                    border_map[b] = border_map[b].max(1);
                }
            }
        }
    }
    let mut borders: Vec<ZoneBorder> = acc
        .into_iter()
        .map(|((lo, hi), e)| {
            let jump = jump_of(lo, hi);
            let flagged = jump > threshold as f32 && e.open > 0;
            let segments = if flagged {
                segments(g, &e.edges)
            } else {
                Vec::new()
            };
            ZoneBorder {
                low: lo,
                high: hi,
                jump,
                open_m: e.open as f32 * g.cell,
                closed_m: e.closed as f32 * g.cell,
                flagged,
                segments,
            }
        })
        .collect();
    borders.sort_by(|a, b| {
        b.flagged
            .cmp(&a.flagged)
            .then(b.jump.total_cmp(&a.jump))
            .then(b.open_m.total_cmp(&a.open_m))
    });
    FlowResult {
        threshold,
        borders,
        border_map,
    }
}

/// Groups open border edges into stretches (edges within a few cells of each other).
fn segments(g: &WalkGrid, edges: &[(u32, u32)]) -> Vec<BorderSegment> {
    let n = g.n;
    let mut at: HashMap<u32, Vec<usize>> = HashMap::new();
    for (e, (lo, _)) in edges.iter().enumerate() {
        at.entry(*lo).or_default().push(e);
    }
    let mut seen = vec![false; edges.len()];
    let mut out = Vec::new();
    for s in 0..edges.len() {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut group = vec![s];
        let mut stack = vec![s];
        while let Some(e) = stack.pop() {
            let c = edges[e].0 as usize;
            let (ci, cj) = ((c % n) as i32, (c / n) as i32);
            for dj in -2..=2 {
                for di in -2..=2 {
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i >= n as i32 || j >= n as i32 {
                        continue;
                    }
                    if let Some(list) = at.get(&((j as usize * n + i as usize) as u32)) {
                        for &f in list {
                            if !seen[f] {
                                seen[f] = true;
                                group.push(f);
                                stack.push(f);
                            }
                        }
                    }
                }
            }
        }
        let len_m = group.len() as f32 * g.cell;
        let mut c = Vec2::ZERO;
        let mut dir = Vec2::ZERO;
        for &e in &group {
            let (lo, hi) = (edges[e].0 as usize, edges[e].1 as usize);
            c += g.center(lo);
            dir += g.center(hi) - g.center(lo);
        }
        c /= group.len() as f32;
        let mid = group
            .iter()
            .map(|&e| edges[e].0 as usize)
            .min_by(|a, b| {
                g.center(*a)
                    .distance(c)
                    .total_cmp(&g.center(*b).distance(c))
            })
            .unwrap_or(edges[s].0 as usize);
        out.push(BorderSegment {
            pos: g.world(mid),
            dir: dir.normalize_or(Vec2::X),
            len_m,
        });
    }
    out.sort_by(|a, b| b.len_m.total_cmp(&a.len_m));
    out
}

/// What a designer could do about a flagged border.
pub fn suggestion(b: &ZoneBorder, zones: &[ZoneInfo], threshold: u32) -> String {
    let name = |id: u8| {
        zones
            .iter()
            .find(|z| z.id == id)
            .map(|z| z.name.clone())
            .unwrap_or(format!("Zone {id}"))
    };
    let lo = zones.iter().find(|z| z.id == b.low);
    let hi = zones.iter().find(|z| z.id == b.high);
    let open = b.open_m.round();
    let barrier = if open > 600.0 {
        "a river, a mountain ridge or a cliff line along it"
    } else if open > 150.0 {
        "a river, a cliff or a wall"
    } else {
        "a gate, a wall or a cliff"
    };
    let mut s = format!("{open:.0} m of open border. Close it with {barrier}");
    if let (Some(lo), Some(hi)) = (lo, hi) {
        // lower the high zone's start so the jump fits the threshold
        let span = hi.level_max.saturating_sub(hi.level_min);
        let new_min = (mid(lo) + threshold as f32 - span as f32 * 0.5)
            .floor()
            .max(lo.level_min as f32) as u32;
        s += &format!(
            ", or bring the levels closer (e.g. {} Lv {}–{}).",
            name(b.high),
            new_min,
            new_min + span
        );
    } else {
        s.push('.');
    }
    s
}
