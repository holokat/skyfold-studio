//! Paints the overlay image for each view (one texel per grid cell, rows in parallel), plus the
//! legend each view shows. Colours are chosen to read at a glance over grass, rock and snow:
//! green = good, amber = careful, red = problem, blue = water.

use super::View;
use super::grid::{Class, WalkGrid};
use super::job::Parts;

pub type Rgba = [u8; 4];

pub const WALK: Rgba = [88, 200, 110, 95];
pub const ROAD: Rgba = [250, 214, 92, 170];
pub const SHALLOW: Rgba = [120, 214, 240, 150];
pub const STEEP: Rgba = [247, 160, 58, 175];
pub const CLIFF: Rgba = [222, 52, 52, 205];
pub const OBSTACLE: Rgba = [176, 78, 222, 220];
pub const SWIM: Rgba = [40, 112, 235, 160];
pub const OPEN_SEA: Rgba = [24, 52, 120, 110];
pub const UNREACHED: Rgba = [36, 40, 50, 170];
pub const DEAD: Rgba = [228, 54, 54, 190];
pub const NEAR: Rgba = [64, 204, 124, 70];
/// Reachable ground stays untinted: only the problems stand out.
pub const REACHABLE: Rgba = [0, 0, 0, 0];
pub const POCKET: Rgba = [255, 64, 170, 210];
pub const TRAP: Rgba = [255, 150, 40, 215];
pub const BORDER_JUMP: Rgba = [255, 58, 58, 255];
pub const BORDER_OPEN: Rgba = [255, 255, 255, 215];
pub const BORDER_CLOSED: Rgba = [70, 74, 84, 200];

/// Travel-time colour stops (seconds → colour).
pub const RAMP: [(f32, [u8; 3]); 6] = [
    (0.0, [40, 205, 130]),
    (60.0, [140, 220, 70]),
    (120.0, [250, 222, 60]),
    (240.0, [250, 142, 40]),
    (480.0, [228, 58, 52]),
    (900.0, [150, 44, 170]),
];

pub fn ramp(t: f32) -> [u8; 3] {
    if t <= RAMP[0].0 {
        return RAMP[0].1;
    }
    for w in RAMP.windows(2) {
        let ((t0, c0), (t1, c1)) = (w[0], w[1]);
        if t <= t1 {
            let f = (t - t0) / (t1 - t0);
            return std::array::from_fn(|i| {
                (c0[i] as f32 + (c1[i] as f32 - c0[i] as f32) * f).round() as u8
            });
        }
    }
    RAMP[RAMP.len() - 1].1
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t.clamp(0.0, 1.0)).round() as u8
    })
}

fn rgb(c: [u8; 3], a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// Diagonal hatching for "can't get here".
fn hatch(i: usize, j: usize) -> bool {
    (i + j) / 3 % 2 == 0
}

pub fn class_colour(g: &WalkGrid, k: usize) -> Rgba {
    match g.class[k] {
        Class::Walk if g.road[k] > 0 => ROAD,
        Class::Walk => WALK,
        Class::Shallow => SHALLOW,
        Class::Steep => STEEP,
        Class::Cliff => CLIFF,
        Class::Obstacle => OBSTACLE,
        Class::Swim if g.passable(k) => SWIM,
        Class::Swim => CLIFF,
        Class::OpenSea => OPEN_SEA,
        Class::Outside => [0; 4],
    }
}

/// The overlay for `view`, or None if its data isn't ready. `zone_colours` maps zone id → sRGB.
pub fn paint(view: View, parts: &Parts, zone_colours: &[[u8; 3]; 256]) -> Option<(usize, Vec<u8>)> {
    let g = parts.grid.as_ref()?.1.clone();
    let n = g.n;
    let reach = parts.reach.as_ref().map(|r| r.1.clone());
    let travel = parts.travel.as_ref().map(|r| r.1.clone());
    let gapf = parts.gapfield.as_ref().map(|r| r.1.clone());
    let gaps = parts.gaps.as_ref().map(|r| r.1.clone());
    let flow = parts.flow.as_ref().map(|r| r.1.clone());
    match view {
        View::Travel if travel.is_none() => return None,
        View::Gaps if gapf.is_none() || gaps.is_none() => return None,
        View::Flow if flow.is_none() => return None,
        View::Reach if reach.is_none() => return None,
        _ => {}
    }
    let mut img = vec![0u8; n * n * 4];
    let threads = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(8)
        .clamp(1, 16);
    let rows = n.div_ceil(threads);
    std::thread::scope(|sc| {
        for (t, chunk) in img.chunks_mut(rows * n * 4).enumerate() {
            let (g, reach, travel, gapf, gaps, flow) = (&g, &reach, &travel, &gapf, &gaps, &flow);
            sc.spawn(move || {
                let j0 = t * rows;
                for (r, row) in chunk.chunks_mut(n * 4).enumerate() {
                    let j = j0 + r;
                    for i in 0..n {
                        let k = j * n + i;
                        let c = texel(
                            view,
                            g,
                            k,
                            i,
                            j,
                            reach.as_deref(),
                            travel.as_deref(),
                            gapf.as_deref(),
                            gaps.as_deref(),
                            flow.as_deref(),
                            zone_colours,
                        );
                        row[i * 4..i * 4 + 4].copy_from_slice(&c);
                    }
                }
            });
        }
    });
    Some((n, img))
}

#[allow(clippy::too_many_arguments)]
fn texel(
    view: View,
    g: &WalkGrid,
    k: usize,
    i: usize,
    j: usize,
    reach: Option<&super::job::Reach>,
    travel: Option<&super::job::Travel>,
    gapf: Option<&super::job::GapField>,
    gaps: Option<&super::job::Gaps>,
    flow: Option<&super::flow::FlowResult>,
    zone_colours: &[[u8; 3]; 256],
) -> Rgba {
    let n = g.n;
    let land = g.class[k].land();
    // a contour where `band` changes between this cell and its right / lower neighbour
    let contour = |v: &[f32], step: f32| -> Option<f32> {
        let b = |t: f32| {
            if t.is_finite() {
                (t / step).floor()
            } else {
                -1.0
            }
        };
        let here = b(v[k]);
        for nb in [(i + 1 < n).then(|| k + 1), (j + 1 < n).then(|| k + n)]
            .into_iter()
            .flatten()
        {
            let there = b(v[nb]);
            if there != here && here >= 0.0 && there >= 0.0 {
                return Some(here.max(there));
            }
        }
        None
    };
    match view {
        View::Walkability => class_colour(g, k),
        View::Doctor => {
            let c = class_colour(g, k);
            [c[0], c[1], c[2], c[3] / 3]
        }
        View::Travel => {
            let tr = travel.unwrap();
            let t = tr.times[k];
            if !(land || g.class[k] == Class::Steep || g.class[k] == Class::Swim) {
                return [0; 4];
            }
            if !t.is_finite() {
                return if land && hatch(i, j) {
                    UNREACHED
                } else if land {
                    [UNREACHED[0], UNREACHED[1], UNREACHED[2], 110]
                } else {
                    [0; 4]
                };
            }
            if let Some(band) = contour(&tr.times, 30.0) {
                // full minutes stronger than half minutes
                return if band as i32 % 2 == 0 {
                    [255, 255, 255, 235]
                } else {
                    [255, 255, 255, 140]
                };
            }
            rgb(ramp(t), if g.class[k] == Class::Swim { 120 } else { 165 })
        }
        View::Gaps => {
            let (f, gp) = (gapf.unwrap(), gaps.unwrap());
            if !land || reach.is_some_and(|r| !r.reachable(k)) {
                return [0; 4];
            }
            let t = f.times[k];
            let th = gp.threshold.max(1.0);
            if gp.sources > 0 && t.is_finite() {
                // a line two to three texels wide
                let edge = [
                    (i + 1 < n).then(|| k + 1),
                    (i + 2 < n).then(|| k + 2),
                    (j + 1 < n).then(|| k + n),
                    (j + 2 < n).then(|| k + 2 * n),
                ]
                .into_iter()
                .flatten()
                .any(|nb| g.class[nb].land() && (f.times[nb] > th) != (t > th));
                if edge {
                    return [255, 255, 255, 240];
                }
            }
            if t <= th {
                let c = mix(
                    NEAR[..3].try_into().unwrap(),
                    [240, 214, 70],
                    (t / th).powf(1.5),
                );
                rgb(c, (NEAR[3] as f32 + (t / th) * 70.0) as u8)
            } else if t.is_finite() {
                let a = (DEAD[3] as f32 * (0.75 + 0.25 * ((t - th) / (2.0 * th)).min(1.0))) as u8;
                rgb([DEAD[0], DEAD[1], DEAD[2]], a)
            } else if hatch(i, j) {
                DEAD
            } else {
                [DEAD[0], DEAD[1], DEAD[2], 120]
            }
        }
        View::Flow => {
            let fl = flow.unwrap();
            match fl.border_map[k] {
                3 => return BORDER_JUMP,
                2 => return BORDER_OPEN,
                1 => return BORDER_CLOSED,
                _ => {}
            }
            let z = g.zone[k];
            if z == 0 || !(land || g.class[k] == Class::Steep) {
                return [0; 4];
            }
            rgb(zone_colours[z as usize], 85)
        }
        View::Reach => {
            let r = reach.unwrap();
            if !land {
                return [0; 4];
            }
            if r.origin.is_none() {
                return [0; 4];
            }
            match r.flags[k] {
                0 | 2 => POCKET,
                1 => TRAP,
                _ => REACHABLE,
            }
        }
    }
}

/// (colour, label) rows of a view's legend.
pub fn legend(view: View) -> Vec<(Rgba, &'static str)> {
    match view {
        View::Doctor => vec![],
        View::Walkability => vec![
            (WALK, "Walkable"),
            (ROAD, "Road (faster)"),
            (SHALLOW, "Wading (slower)"),
            (STEEP, "Steep: steeper than the walk limit"),
            (CLIFF, "Cliff: nobody passes"),
            (OBSTACLE, "Blocked by trees, rocks, buildings"),
            (SWIM, "Swimming"),
            (OPEN_SEA, "Open water: too far from shore"),
        ],
        View::Travel => vec![
            (
                [255, 255, 255, 235],
                "Contour every 30 s (bold = full minute)",
            ),
            (UNREACHED, "Can't get there on foot"),
        ],
        View::Gaps => vec![
            (NEAR, "Close to content"),
            ([240, 214, 70, 140], "Approaching the limit"),
            (DEAD, "Dead zone: nothing to do nearby"),
            ([255, 255, 255, 240], "The limit line"),
        ],
        View::Flow => vec![
            (BORDER_JUMP, "Open border, big level jump"),
            (BORDER_OPEN, "Open border"),
            (BORDER_CLOSED, "Closed by cliffs or water"),
        ],
        View::Reach => vec![
            (REACHABLE, "No tint: reachable from the start"),
            (POCKET, "Walkable but unreachable"),
            (TRAP, "Trap: can get in, can't get out"),
        ],
    }
}
