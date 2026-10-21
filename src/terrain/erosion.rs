//! Erosion that reads like real land.
//!
//! Running water is simulated on a drainage network rather than with individual droplets: every
//! round the terrain is routed (priority-flood, so water always finds a way out of hollows and
//! lakes spill over their lowest rim), rain is accumulated downhill with multiple-flow directions
//! (no grid-aligned streaks), and each spot is cut by stream power (`√area × slope`) while the
//! water carries the sediment on until its transport capacity drops, where it settles: fans at
//! the foot of mountains, flat sediment-filled valley floors, filled hollows. Repeated rounds grow
//! dendritic gully networks and leave sharp ridgelines between them, the way real DEMs look.
//!
//! The network runs coarse-to-fine (big valleys on a coarse grid, gullies on the full grid) so a
//! 4 km world erodes in a few seconds and a 16 km one in well under a minute. Soil creep rounds
//! crests, thermal weathering sheds scree below cliffs (talus angle), glaciers widen high valleys
//! into U shapes, and wind (a Werner-style saltation model) drifts sand into dunes and lee ramps.
//!
//! Everything works on any rectangle of the heightfield with an optional per-vertex weight mask,
//! which serves the whole-world button, painted areas / polygons and the interactive brush.
//! `flow_map` turns the same drainage network into the wetness map the ground style uses.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use super::GridRect;
use crate::noise::{Rng, smoothstep};

/// Progress (per mille) and cancellation, shared between a background job and the UI.
#[derive(Default)]
pub struct Progress {
    permille: AtomicU32,
    cancel: AtomicBool,
}

impl Progress {
    pub fn get(&self) -> f32 {
        self.permille.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// The part of the progress bar from `from` to `to` (fractions of the whole job).
    pub fn stage(&self, from: f32, to: f32) -> Stage<'_> {
        Stage { p: self, from, to }
    }
}

/// A sub-range of a job's progress.
#[derive(Clone, Copy)]
pub struct Stage<'a> {
    p: &'a Progress,
    from: f32,
    to: f32,
}

impl<'a> Stage<'a> {
    pub fn set(&self, f: f32) {
        let v = self.from + (self.to - self.from) * f.clamp(0.0, 1.0);
        self.p
            .permille
            .store((v * 1000.0) as u32, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.p.cancelled()
    }
    pub fn sub(&self, a: f32, b: f32) -> Stage<'a> {
        let span = self.to - self.from;
        Stage {
            p: self.p,
            from: self.from + span * a,
            to: self.from + span * b,
        }
    }
}

/// Erosion looks, from a gentle wash to wind-carved desert.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ErosionPreset {
    LightRain,
    #[default]
    HeavyRain,
    AncientMountains,
    Glacial,
    DesertWind,
}

impl ErosionPreset {
    pub const ALL: [ErosionPreset; 5] = [
        ErosionPreset::LightRain,
        ErosionPreset::HeavyRain,
        ErosionPreset::AncientMountains,
        ErosionPreset::Glacial,
        ErosionPreset::DesertWind,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ErosionPreset::LightRain => "Light Rain",
            ErosionPreset::HeavyRain => "Heavy Rain",
            ErosionPreset::AncientMountains => "Ancient Mountains",
            ErosionPreset::Glacial => "Glacial / Thermal",
            ErosionPreset::DesertWind => "Desert Wind",
        }
    }
    /// snake_case id (MCP).
    pub fn id(self) -> &'static str {
        match self {
            ErosionPreset::LightRain => "light_rain",
            ErosionPreset::HeavyRain => "heavy_rain",
            ErosionPreset::AncientMountains => "ancient_mountains",
            ErosionPreset::Glacial => "glacial",
            ErosionPreset::DesertWind => "desert_wind",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '-', '/'], "_");
        Self::ALL.into_iter().find(|p| {
            p.id() == s
                || s.starts_with(p.id())
                || p.name().to_ascii_lowercase().replace([' ', '/'], "_") == s
        })
    }
    pub fn hint(self) -> &'static str {
        match self {
            ErosionPreset::LightRain => {
                "A few centuries of rain: fine gullies on slopes, small fans, ridges stay crisp"
            }
            ErosionPreset::HeavyRain => {
                "Deep dendritic valleys, sharp ridgelines, alluvial fans and sediment-filled valley floors"
            }
            ErosionPreset::AncientMountains => {
                "Old, worn ranges: rounded crests, broad mature valleys, thick sediment in the lowlands"
            }
            ErosionPreset::Glacial => {
                "Ice-carved U-shaped high valleys, frost-shattered cliffs and scree slopes"
            }
            ErosionPreset::DesertWind => {
                "Wind-scoured slopes, sand ramps in the lee of hills and dune fields on open ground"
            }
        }
    }
    pub fn params(self) -> ErosionParams {
        let d = ErosionParams::default();
        match self {
            ErosionPreset::LightRain => ErosionParams {
                years: 40,
                rain: 0.55,
                capacity: 30.0,
                deposition: 0.35,
                creep: 0.06,
                talus_angle: 50.0,
                thermal: 0.15,
                ..d
            },
            ErosionPreset::HeavyRain => d,
            ErosionPreset::AncientMountains => ErosionParams {
                years: 110,
                rain: 0.8,
                capacity: 26.0,
                deposition: 0.6,
                creep: 0.55,
                talus_angle: 38.0,
                thermal: 0.4,
                ..d
            },
            ErosionPreset::Glacial => ErosionParams {
                years: 70,
                rain: 0.45,
                capacity: 30.0,
                deposition: 0.4,
                creep: 0.05,
                talus_angle: 34.0,
                thermal: 1.0,
                glacial: 1.0,
                ..d
            },
            ErosionPreset::DesertWind => ErosionParams {
                years: 25,
                rain: 0.18,
                capacity: 20.0,
                deposition: 0.55,
                creep: 0.02,
                talus_angle: 33.0,
                thermal: 0.6,
                wind: 1.0,
                ..d
            },
        }
    }
}

/// Erosion settings. Presets fill them in; agents can tune each one over MCP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ErosionParams {
    /// Simulated time: rounds of rain (each round routes all water and moves all sediment).
    pub years: u32,
    /// How hard running water cuts into the ground (stream-power incision, 0..2).
    pub rain: f32,
    /// How much sediment water carries before it must drop it (relative to what it cuts).
    pub capacity: f32,
    /// How quickly surplus sediment settles (0..1): alluvial fans, flat valley floors, filled hollows.
    pub deposition: f32,
    /// Soil creep rounding crests and hilltops (0..1).
    pub creep: f32,
    /// Steepest slope loose rock rests at (degrees); steeper ground sheds scree.
    pub talus_angle: f32,
    /// Frost weathering: how much rock falls from slopes steeper than the talus angle (0..1).
    pub thermal: f32,
    /// Glaciers widening high valleys into U shapes (0..1).
    pub glacial: f32,
    /// Wind moving sand (0..1).
    pub wind: f32,
    /// Direction the wind blows towards (degrees; 0 = +X / east, 90 = +Z / south).
    pub wind_dir: f32,
    /// Overall strength (multiplies `rain`, `thermal`, `glacial` and `wind`).
    pub strength: f32,
    pub seed: u64,
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            years: 70,
            rain: 1.0,
            capacity: 28.0,
            deposition: 0.5,
            creep: 0.12,
            talus_angle: 48.0,
            thermal: 0.2,
            glacial: 0.0,
            wind: 0.0,
            wind_dir: 20.0,
            strength: 1.0,
            seed: 1,
        }
    }
}

/// Neighbour offsets (x, z) and their distance in cells.
const NB: [(isize, isize, f32); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    (-1, -1, std::f32::consts::SQRT_2),
    (1, -1, std::f32::consts::SQRT_2),
    (-1, 1, std::f32::consts::SQRT_2),
    (1, 1, std::f32::consts::SQRT_2),
];

/// Largest grid the drainage network runs on (per side) before it works coarser.
const NET_MAX: usize = 2049;
/// Coarsest grid of the coarse-to-fine cascade (per side).
const NET_MIN: usize = 192;

fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 12)
}

/// Runs `f(row_index, row)` over the rows of `out` (width `w`) on all cores.
pub(super) fn par_rows(out: &mut [f32], w: usize, f: impl Fn(usize, &mut [f32]) + Sync) {
    let rows = out.len() / w;
    let per = rows.div_ceil(threads() * 4).max(1);
    std::thread::scope(|s| {
        let f = &f;
        let chunks: Vec<_> = out.chunks_mut(per * w).enumerate().collect();
        let groups: Vec<Vec<(usize, &mut [f32])>> = {
            let t = threads();
            let mut g: Vec<Vec<(usize, &mut [f32])>> = (0..t).map(|_| vec![]).collect();
            for (k, c) in chunks {
                g[k % t].push((k, c));
            }
            g
        };
        for group in groups {
            s.spawn(move || {
                for (k, chunk) in group {
                    for (r, row) in chunk.chunks_mut(w).enumerate() {
                        f(k * per + r, row);
                    }
                }
            });
        }
    });
}

/// A rectangular working grid of heights.
#[derive(Clone)]
struct Grid {
    w: usize,
    h: usize,
    /// metres between samples
    dx: f32,
    v: Vec<f32>,
}

impl Grid {
    fn extract(heights: &[f32], n: usize, r: GridRect, dx: f32) -> Grid {
        let (w, h) = (r.width(), r.height());
        let mut v = Vec::with_capacity(w * h);
        for j in r.z0..=r.z1 {
            v.extend_from_slice(&heights[j * n + r.x0..=j * n + r.x1]);
        }
        Grid { w, h, dx, v }
    }
    /// Box-filtered copy with every `f`-th sample (corner aligned; the last row/column is kept).
    fn downsample(&self, f: usize) -> Grid {
        let cw = (self.w - 1).div_ceil(f) + 1;
        let ch = (self.h - 1).div_ceil(f) + 1;
        let mut v = vec![0.0; cw * ch];
        let r = (f / 2) as isize;
        par_rows(&mut v, cw, |j, row| {
            let cz = (j * f).min(self.h - 1) as isize;
            for (i, out) in row.iter_mut().enumerate() {
                let cx = (i * f).min(self.w - 1) as isize;
                let (mut s, mut c) = (0.0, 0.0);
                for dz in -r..=r {
                    for dxx in -r..=r {
                        let x = (cx + dxx).clamp(0, self.w as isize - 1) as usize;
                        let z = (cz + dz).clamp(0, self.h as isize - 1) as usize;
                        s += self.v[z * self.w + x];
                        c += 1.0;
                    }
                }
                *out = s / c;
            }
        });
        Grid {
            w: cw,
            h: ch,
            dx: self.dx * f as f32,
            v,
        }
    }
    /// Adds the bilinearly upsampled `delta` (a grid made by `downsample(f)`) to this grid.
    fn add_upsampled(&mut self, delta: &Grid, f: usize) {
        let (w, h) = (self.w, self.h);
        let pos = |i: usize, lim: usize| (i * f).min(lim - 1);
        let lookup = |x: usize, lim: usize, cn: usize| -> (usize, usize, f32) {
            let i0 = (x / f).min(cn - 1);
            let i1 = (i0 + 1).min(cn - 1);
            let (p0, p1) = (pos(i0, lim), pos(i1, lim));
            let t = if p1 > p0 {
                (x - p0) as f32 / (p1 - p0) as f32
            } else {
                0.0
            };
            (i0, i1, t)
        };
        let xs: Vec<(usize, usize, f32)> = (0..w).map(|x| lookup(x, w, delta.w)).collect();
        par_rows(&mut self.v, w, |z, row| {
            let (j0, j1, tz) = lookup(z, h, delta.h);
            for (x, out) in row.iter_mut().enumerate() {
                let (i0, i1, tx) = xs[x];
                let a = delta.v[j0 * delta.w + i0] * (1.0 - tx) + delta.v[j0 * delta.w + i1] * tx;
                let b = delta.v[j1 * delta.w + i0] * (1.0 - tx) + delta.v[j1 * delta.w + i1] * tx;
                *out += a + (b - a) * tz;
            }
        });
    }
}

/// Orders f32 keys as u32 (all finite values).
#[inline]
fn key(h: f32, i: usize) -> u64 {
    let b = h.to_bits();
    let k = if b & 0x8000_0000 != 0 {
        !b
    } else {
        b | 0x8000_0000
    };
    ((k as u64) << 32) | i as u64
}

/// Incision per metre of stream power (`√area × slope`) and round, at `rain` 1.
const KF: f32 = 0.0063;
/// Glacial incision per metre of `√ice area × slope` and round, at `glacial` 1.
const KG: f32 = 0.012;

/// Share of the surplus sediment that settles in hollows (lake beds) each round: hollows fill
/// slowly, so their outlets have time to cut down and drain them (no unnaturally flat basins).
const LAKE_SETTLE: f32 = 0.12;

const UNSEEN: u32 = u32::MAX;
const QUEUED: u32 = u32::MAX - 1;
/// Gradient given to filled hollows so water still knows where to go (m per cell).
const FILL_EPS: f32 = 1e-3;

/// Drainage network of a grid: routing order, filled surface and per-cell accumulations.
struct Net {
    filled: Vec<f32>,
    /// 0 = outlet (border or sea), else position in the routing order (+1)
    rank: Vec<u32>,
    /// non-outlet cells, lowest (nearest an outlet) first
    order: Vec<u32>,
    area: Vec<f32>,
    sed: Vec<f32>,
    heap: BinaryHeap<Reverse<u64>>,
    pit: VecDeque<u32>,
}

impl Net {
    fn new(n: usize) -> Net {
        Net {
            filled: vec![0.0; n],
            rank: vec![0; n],
            order: Vec::with_capacity(n),
            area: vec![0.0; n],
            sed: vec![0.0; n],
            heap: BinaryHeap::new(),
            pit: VecDeque::new(),
        }
    }

    /// Priority-flood routing (Barnes et al. 2014): outlets are the grid border and the sea;
    /// hollows are filled with a tiny gradient so every cell drains to an outlet.
    fn route(&mut self, g: &Grid, sea: f32) {
        let (w, h) = (g.w, g.h);
        self.filled.copy_from_slice(&g.v);
        self.order.clear();
        self.heap.clear();
        self.pit.clear();
        let border = |c: usize| {
            let (x, z) = (c % w, c / w);
            x == 0 || z == 0 || x == w - 1 || z == h - 1
        };
        for c in 0..w * h {
            self.rank[c] = if border(c) || g.v[c] <= sea {
                0
            } else {
                UNSEEN
            };
        }
        for c in 0..w * h {
            if self.rank[c] != 0 {
                continue;
            }
            // only outlets next to land need expanding
            let (x, z) = ((c % w) as isize, (c / w) as isize);
            let near_land = NB.iter().any(|&(dx, dz, _)| {
                let (a, b) = (x + dx, z + dz);
                a >= 0
                    && b >= 0
                    && a < w as isize
                    && b < h as isize
                    && self.rank[b as usize * w + a as usize] == UNSEEN
            });
            if near_land {
                self.heap.push(Reverse(key(self.filled[c], c)));
            }
        }
        let mut next = 1u32;
        let offs: [isize; 8] = NB.map(|(dx, dz, _)| dz * w as isize + dx);
        loop {
            let c = if let Some(c) = self.pit.pop_front() {
                c as usize
            } else if let Some(Reverse(k)) = self.heap.pop() {
                (k & 0xffff_ffff) as usize
            } else {
                break;
            };
            if self.rank[c] != 0 {
                self.rank[c] = next;
                next += 1;
                self.order.push(c as u32);
            }
            let fc = self.filled[c];
            let interior = !border(c);
            for (k, &off) in offs.iter().enumerate() {
                let nb = if interior {
                    (c as isize + off) as usize
                } else {
                    let (x, z) = ((c % w) as isize + NB[k].0, (c / w) as isize + NB[k].1);
                    if x < 0 || z < 0 || x >= w as isize || z >= h as isize {
                        continue;
                    }
                    z as usize * w + x as usize
                };
                if self.rank[nb] != UNSEEN {
                    continue;
                }
                self.rank[nb] = QUEUED;
                if self.filled[nb] <= fc {
                    self.filled[nb] = fc + FILL_EPS;
                    self.pit.push_back(nb as u32);
                } else {
                    self.heap.push(Reverse(key(self.filled[nb], nb)));
                }
            }
        }
    }

    /// Downhill neighbours of a routed cell and their flow weights (multiple flow directions,
    /// slope⁴ on the filled surface: concentrated like real channels, without D8 streaks).
    /// Returns the weights' sum, the steepest real slope and the lowest receiver's height.
    #[inline]
    fn receivers(
        &self,
        g: &Grid,
        c: usize,
        offs: &[isize; 8],
        inv_d: &[f32; 8],
        ws: &mut [f32; 8],
    ) -> (f32, f32, f32) {
        let (fc, rc, hc) = (self.filled[c], self.rank[c], g.v[c]);
        // across a filled hollow the flow spreads out (its tiny fill gradient would draw lines)
        let lake = fc - hc > 0.01;
        let mut sum = 0.0;
        let mut smax = 0.0f32;
        let mut low = f32::MAX;
        for k in 0..8 {
            let nb = (c as isize + offs[k]) as usize;
            ws[k] = 0.0;
            if self.rank[nb] < rc && self.filled[nb] < fc {
                let s = (fc - self.filled[nb]) * inv_d[k];
                let s2 = s * s;
                ws[k] = if lake { s } else { s2 * s2 } + 1e-30;
                sum += ws[k];
                smax = smax.max((hc - g.v[nb]) * inv_d[k]);
                low = low.min(g.v[nb]);
            }
        }
        (sum, smax, low)
    }

    /// Drainage area (m²) of every cell.
    fn accumulate(&mut self, g: &Grid) {
        let w = g.w as isize;
        let offs: [isize; 8] = NB.map(|(dx, dz, _)| dz * w + dx);
        let inv_d: [f32; 8] = NB.map(|(_, _, d)| 1.0 / (d * g.dx));
        self.area.fill(g.dx * g.dx);
        let mut ws = [0.0f32; 8];
        for i in (0..self.order.len()).rev() {
            let c = self.order[i] as usize;
            let (sum, _, _) = self.receivers(g, c, &offs, &inv_d, &mut ws);
            let a = self.area[c] / sum;
            for k in 0..8 {
                if ws[k] > 0.0 {
                    let nb = (c as isize + offs[k]) as usize;
                    self.area[nb] += a * ws[k];
                }
            }
        }
    }
}

/// Per-round fluvial coefficients.
struct Fluvial {
    /// incision per metre of stream power per round
    kf: f32,
    /// capacity (m³ per m of stream power per m² of cell)
    kc: f32,
    deposition: f32,
    /// glacial incision (0 = none) and the equilibrium-line altitude
    kg: f32,
    ela: f32,
    sea: f32,
}

/// One round of rain: route, accumulate, cut and deposit (in place).
fn fluvial_round(g: &mut Grid, net: &mut Net, f: &Fluvial, glacier: &mut Vec<f32>) {
    net.route(g, f.sea);
    let w = g.w as isize;
    let offs: [isize; 8] = NB.map(|(dx, dz, _)| dz * w + dx);
    let inv_d: [f32; 8] = NB.map(|(_, _, d)| 1.0 / (d * g.dx));
    let dx2 = g.dx * g.dx;
    net.area.fill(dx2);
    net.sed.fill(0.0);
    // deposits per round stay a fraction of a cell's size, so fans build up over many rounds
    let dep_cap = 0.06 * g.dx;
    let glacial = f.kg > 0.0;
    if glacial {
        glacier.clear();
        glacier.resize(g.v.len(), 0.0);
    }
    // ice accumulates above the equilibrium line and melts below it
    let mut ice = if glacial {
        vec![0.0f32; g.v.len()]
    } else {
        vec![]
    };
    let mut ws = [0.0f32; 8];
    for i in (0..net.order.len()).rev() {
        let c = net.order[i] as usize;
        let (sum, slope, low) = net.receivers(g, c, &offs, &inv_d, &mut ws);
        let hc = g.v[c];
        let a = net.area[c];
        let power = a.sqrt() * slope;
        let cap = f.kc * power * dx2;
        let qin = net.sed[c];
        let dh = if qin > cap {
            let lake = net.filled[c] - hc > 0.01;
            let room = (net.filled[c] - hc).max(0.0) + dep_cap;
            ((qin - cap) * f.deposition * if lake { LAKE_SETTLE } else { 1.0 } / dx2).min(room)
        } else {
            let cut_room = ((hc - low) * 0.5).max(0.0);
            -(f.kf * power).min((cap - qin) / dx2).min(cut_room)
        };
        g.v[c] = hc + dh;
        let out = (qin - dh * dx2).max(0.0);
        let mut ice_out = 0.0;
        if glacial {
            // ice gathers above the equilibrium line and melts away below it
            let here = ice[c] + smoothstep(f.ela - 20.0, f.ela + 80.0, hc) * dx2;
            ice_out = here * (0.6 + 0.4 * smoothstep(f.ela - 160.0, f.ela, hc));
            glacier[c] = f.kg * here.sqrt() * slope.min(1.2);
        }
        let inv = 1.0 / sum;
        for k in 0..8 {
            if ws[k] > 0.0 {
                let nb = (c as isize + offs[k]) as usize;
                let share = ws[k] * inv;
                net.area[nb] += a * share;
                if net.rank[nb] != 0 {
                    net.sed[nb] += out * share;
                } else if g.v[nb] <= f.sea {
                    // sediment reaching the sea builds a small delta below the waterline
                    let d = (out * share * f.deposition * 0.25 / dx2)
                        .min((f.sea - 0.8 - g.v[nb]).max(0.0))
                        .min(dep_cap);
                    g.v[nb] += d;
                }
                if glacial {
                    ice[nb] += ice_out * share;
                }
            }
        }
    }
    if glacial {
        // a glacier is as wide as it is thick: spread the incision across the valley (U shape)
        let r = (90.0 / g.dx).round().clamp(1.0, 6.0) as usize;
        let mut ice_cut = Grid {
            w: g.w,
            h: g.h,
            dx: g.dx,
            v: std::mem::take(glacier),
        };
        blur(&mut ice_cut, r);
        blur(&mut ice_cut, r);
        for (h, e) in g.v.iter_mut().zip(&ice_cut.v) {
            if *h > f.sea {
                *h -= e.min(2.0 * g.dx);
            }
        }
        *glacier = ice_cut.v;
    }
}

/// Separable box blur of radius `r` (in place).
fn blur(g: &mut Grid, r: usize) {
    let (w, h) = (g.w, g.h);
    let src = g.v.clone();
    par_rows(&mut g.v, w, |z, row| {
        let line = &src[z * w..z * w + w];
        let mut acc = 0.0;
        let mut cnt = 0.0;
        for x in 0..=r.min(w - 1) {
            acc += line[x];
            cnt += 1.0;
        }
        for x in 0..w {
            row[x] = acc / cnt;
            if x + r + 1 < w {
                acc += line[x + r + 1];
                cnt += 1.0;
            }
            if x >= r {
                acc -= line[x - r];
                cnt -= 1.0;
            }
        }
    });
    let src = g.v.clone();
    par_rows(&mut g.v, w, |z, row| {
        let z0 = z.saturating_sub(r);
        let z1 = (z + r).min(h - 1);
        let k = 1.0 / (z1 - z0 + 1) as f32;
        row.fill(0.0);
        for zz in z0..=z1 {
            let line = &src[zz * w..zz * w + w];
            for (o, v) in row.iter_mut().zip(line) {
                *o += v * k;
            }
        }
    });
}

/// A smooth copy of an `n`×`n` field: box-filtered `f`× coarser, blurred twice by `radius`
/// coarse cells and sampled back bilinearly.
pub(super) fn smoothed(v: &[f32], n: usize, f: usize, radius: usize) -> Vec<f32> {
    let mut c = Grid {
        w: n,
        h: n,
        dx: 1.0,
        v: v.to_vec(),
    }
    .downsample(f);
    blur(&mut c, radius);
    blur(&mut c, radius);
    let mut out = Grid {
        w: n,
        h: n,
        dx: 1.0,
        v: vec![0.0; n * n],
    };
    out.add_upsampled(&c, f);
    out.v
}

/// Thermal weathering and soil creep (parallel, mass conserving): material moves from every
/// slope steeper than the talus angle to its lower neighbour; creep diffuses gently everywhere.
fn thermal(
    g: &mut Grid,
    talus_tan: f32,
    rate: f32,
    creep: f32,
    rounds: usize,
    mask_sea: Option<f32>,
) {
    let (w, h) = (g.w, g.h);
    let mut src = g.v.clone();
    for _ in 0..rounds {
        src.copy_from_slice(&g.v);
        let s = &src;
        par_rows(&mut g.v, w, |z, row| {
            for (x, out) in row.iter_mut().enumerate() {
                let c = z * w + x;
                let hc = s[c];
                let mut d = 0.0;
                for &(ox, oz, dist) in &NB {
                    let xx = (x as isize + ox).clamp(0, w as isize - 1) as usize;
                    let zz = (z as isize + oz).clamp(0, h as isize - 1) as usize;
                    let diff = s[zz * w + xx] - hc;
                    let lim = talus_tan * dist * g.dx;
                    if diff > lim {
                        d += rate * (diff - lim);
                    } else if diff < -lim {
                        d -= rate * (-diff - lim);
                    }
                    d += creep * diff / (dist * dist);
                }
                *out = hc + d;
            }
        });
        if let Some(sea) = mask_sea {
            // keep the sea floor where it was (creep would slowly fill bays)
            for (o, s) in g.v.iter_mut().zip(&src) {
                if *s < sea - 1.0 {
                    *o = *s;
                }
            }
        }
    }
}

/// Wind: a Werner-style saltation model. Slabs of sand are lifted wherever the ground isn't in
/// the wind shadow of something upwind, hop downwind and settle (always in the lee, sometimes on
/// open ground); avalanches keep slip faces at the angle of repose. Dunes, lee ramps and scoured
/// windward slopes emerge.
fn aeolian(
    g: &mut Grid,
    sea: f32,
    dir_deg: f32,
    amount: f32,
    sweeps: usize,
    seed: u64,
    progress: &Stage,
) {
    let (w, h) = (g.w, g.h);
    let a = dir_deg.to_radians();
    let wd = (a.cos(), a.sin());
    let hop = (12.0 / g.dx).clamp(1.5, 4.0);
    let slab = 0.12 * amount * (g.dx / 4.0).sqrt();
    let shadow_tan = 15f32.to_radians().tan();
    let mut rng = Rng::new(seed ^ 0xa3e0_11a1);
    let sample = |v: &[f32], x: f32, z: f32| -> f32 {
        let x = x.clamp(0.0, (w - 1) as f32);
        let z = z.clamp(0.0, (h - 1) as f32);
        let (i, j) = ((x as usize).min(w - 2), (z as usize).min(h - 2));
        let (fx, fz) = (x - i as f32, z - j as f32);
        let c = j * w + i;
        let a = v[c] + (v[c + 1] - v[c]) * fx;
        let b = v[c + w] + (v[c + w + 1] - v[c + w]) * fx;
        a + (b - a) * fz
    };
    let mut shadow = vec![false; w * h];
    let repose = 33f32.to_radians().tan();
    for sweep in 0..sweeps {
        if progress.cancelled() {
            return;
        }
        progress.set(sweep as f32 / sweeps as f32);
        // wind shadow: something upwind rises above a 15° line from this spot
        {
            let (v, dx) = (&g.v, g.dx);
            let steps = (60.0 / dx).clamp(4.0, 16.0) as usize;
            let rows: Vec<(usize, Vec<bool>)> = std::thread::scope(|s| {
                let per = h.div_ceil(threads());
                let hs: Vec<_> = (0..h)
                    .step_by(per)
                    .map(|z0| {
                        s.spawn(move || {
                            (z0..(z0 + per).min(h))
                                .map(|z| {
                                    let row = (0..w)
                                        .map(|x| {
                                            let hc = v[z * w + x];
                                            (1..=steps).any(|k| {
                                                let d = k as f32 * 1.5;
                                                sample(v, x as f32 - wd.0 * d, z as f32 - wd.1 * d)
                                                    > hc + shadow_tan * d * dx
                                            })
                                        })
                                        .collect();
                                    (z, row)
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
            });
            for (z, row) in rows {
                shadow[z * w..z * w + w].copy_from_slice(&row);
            }
        }
        // one sweep: as many lifts as cells, in random order
        for _ in 0..w * h / 2 {
            let x = 1 + rng.below((w - 2) as u32) as usize;
            let z = 1 + rng.below((h - 2) as u32) as usize;
            let c = z * w + x;
            if shadow[c] || g.v[c] <= sea + 0.3 {
                continue;
            }
            // steep rock doesn't give up sand; gentle ground does
            let (gx, gz) = (
                (g.v[c + 1] - g.v[c - 1]) / (2.0 * g.dx),
                (g.v[c + w] - g.v[c - w]) / (2.0 * g.dx),
            );
            if gx * gx + gz * gz > 0.5 {
                continue;
            }
            let q = slab;
            g.v[c] -= q;
            let (mut px, mut pz) = (x as f32, z as f32);
            for _ in 0..8 {
                px += wd.0 * hop;
                pz += wd.1 * hop;
                if px < 1.0 || pz < 1.0 || px >= (w - 2) as f32 || pz >= (h - 2) as f32 {
                    break;
                }
                let d = (pz.round() as usize) * w + px.round() as usize;
                let p_settle = if shadow[d] {
                    1.0
                } else if g.v[d] <= sea {
                    1.0
                } else {
                    0.4
                };
                if rng.f32() < p_settle {
                    g.v[d] += q;
                    // slip face: avalanche down the steepest neighbour while above repose
                    let mut at = d;
                    for _ in 0..6 {
                        let (ax, az) = (at % w, at / w);
                        if ax == 0 || az == 0 || ax >= w - 1 || az >= h - 1 {
                            break;
                        }
                        let mut best = (0.0f32, at);
                        for &(ox, oz, dist) in &NB {
                            let nb =
                                ((az as isize + oz) as usize) * w + (ax as isize + ox) as usize;
                            let s = (g.v[at] - g.v[nb]) / (dist * g.dx);
                            if s > best.0 {
                                best = (s, nb);
                            }
                        }
                        if best.0 <= repose {
                            break;
                        }
                        let mv = q.min((best.0 - repose) * g.dx * 0.5);
                        g.v[at] -= mv;
                        g.v[best.1] += mv;
                        at = best.1;
                    }
                    break;
                }
            }
        }
    }
}

/// The coarse-to-fine cascade of grids the network runs on: (decimation factor, rounds).
fn levels(w: usize, h: usize, years: u32) -> Vec<(usize, u32)> {
    let side = w.max(h);
    let mut f = 1;
    while (side - 1) / f + 1 > NET_MAX {
        f *= 2;
    }
    let finest = f;
    while (side - 1) / (f * 2) + 1 >= NET_MIN {
        f *= 2;
    }
    // most rounds on the coarsest grid (cheap, shapes the big valleys); finer grids refine
    let mut out = vec![];
    let mut rounds = years as f32;
    let fine_rounds = (years as f32 * 0.35) as u32;
    while f >= finest {
        // the full grid gets enough rounds of its own to grow gullies on every slope
        out.push((
            f,
            (rounds.round() as u32).max(if f == finest { fine_rounds } else { 1 }),
        ));
        if f == finest {
            break;
        }
        f /= 2;
        rounds *= 0.42;
    }
    out
}

/// Erodes `rect` of an `n`×`n` heightfield in place. `mask` (one weight 0..1 per vertex of the
/// rect, row-major) blends the result in (painted areas, polygons). Returns false when cancelled
/// (the heights are then left untouched).
pub fn erode(
    heights: &mut [f32],
    n: usize,
    cell: f32,
    sea: f32,
    rect: GridRect,
    mask: Option<&[f32]>,
    p: &ErosionParams,
    progress: Stage,
) -> bool {
    let orig = Grid::extract(heights, n, rect, cell);
    let mut g = orig.clone();
    let st = p.strength.max(0.0);
    let land_max = g.v.iter().copied().fold(sea, f32::max);
    let fl = Fluvial {
        kf: KF * p.rain * st,
        kc: p.capacity * KF * p.rain.max(0.05),
        deposition: p.deposition.clamp(0.0, 1.0),
        kg: KG * p.glacial * st,
        ela: sea + (land_max - sea) * 0.3,
        sea,
    };
    let plan = levels(g.w, g.h, p.years);
    let total: f32 = plan
        .iter()
        .map(|(f, r)| *r as f32 / (f * f) as f32)
        .sum::<f32>()
        .max(1e-6);
    let mut done = 0.0;
    let fluvial_part = progress.sub(0.0, if p.wind > 0.0 { 0.55 } else { 0.85 });
    let mut glacier = vec![];
    for &(f, rounds) in &plan {
        let mut work = if f > 1 { g.downsample(f) } else { g.clone() };
        let before = work.v.clone();
        let mut net = Net::new(work.v.len());
        for _ in 0..rounds {
            if progress.cancelled() {
                return false;
            }
            fluvial_round(&mut work, &mut net, &fl, &mut glacier);
            done += 1.0 / (f * f) as f32;
            fluvial_part.set(done / total);
        }
        // creep and a little weathering on the network's own grid keep its ridges from aliasing
        if p.creep > 0.0 {
            thermal(
                &mut work,
                (p.talus_angle.to_radians()).tan(),
                0.0,
                (p.creep * 0.06).min(0.1),
                2,
                Some(sea),
            );
        }
        if f > 1 {
            for (v, b) in work.v.iter_mut().zip(&before) {
                *v -= b;
            }
            g.add_upsampled(&work, f);
        } else {
            g = work;
        }
    }
    if p.wind > 0.0 {
        let f = plan
            .last()
            .map(|l| l.0)
            .unwrap_or(1)
            .max(if g.w.max(g.h) > 1100 { 2 } else { 1 });
        let mut work = if f > 1 { g.downsample(f) } else { g.clone() };
        let before = work.v.clone();
        let sweeps = (12.0 + 30.0 * p.wind) as usize;
        aeolian(
            &mut work,
            sea,
            p.wind_dir,
            p.wind * st,
            sweeps,
            p.seed,
            &progress.sub(0.55, 0.85),
        );
        if progress.cancelled() {
            return false;
        }
        if f > 1 {
            for (v, b) in work.v.iter_mut().zip(&before) {
                *v -= b;
            }
            g.add_upsampled(&work, f);
        } else {
            g = work;
        }
    }
    // weathering on the full grid: scree below cliffs, no spikes left by the network
    let rounds = (4.0 + 26.0 * p.thermal * st.min(2.0)) as usize;
    thermal(
        &mut g,
        p.talus_angle.clamp(20.0, 70.0).to_radians().tan(),
        0.07,
        (p.creep * 0.02).min(0.05),
        rounds,
        Some(sea),
    );
    if progress.cancelled() {
        return false;
    }
    progress.set(1.0);
    // blend back
    let w = g.w;
    for z in 0..g.h {
        let row = (rect.z0 + z) * n + rect.x0;
        for x in 0..w {
            let k = z * w + x;
            let m = mask.map_or(1.0, |m| m[k]);
            heights[row + x] = orig.v[k] + (g.v[k] - orig.v[k]) * m;
        }
    }
    true
}

/// One brush dab: a quick local round of the same processes around a centre (grid coordinates),
/// blended by the brush falloff. `amount` ≈ rounds of rain this dab is worth. Returns the
/// changed rect.
#[allow(clippy::too_many_arguments)]
pub fn erode_brush(
    heights: &mut [f32],
    n: usize,
    cell: f32,
    sea: f32,
    center: (f32, f32),
    radius: f32,
    falloff: f32,
    amount: f32,
    p: &ErosionParams,
) -> Option<GridRect> {
    let rc = radius * 1.25 + 3.0;
    let max = (n - 1) as f32;
    let (x0, x1) = (
        (center.0 - rc).floor().clamp(0.0, max) as usize,
        (center.0 + rc).ceil().clamp(0.0, max) as usize,
    );
    let (z0, z1) = (
        (center.1 - rc).floor().clamp(0.0, max) as usize,
        (center.1 + rc).ceil().clamp(0.0, max) as usize,
    );
    if x1 < x0 + 4 || z1 < z0 + 4 {
        return None;
    }
    let rect = GridRect { x0, z0, x1, z1 };
    let orig = Grid::extract(heights, n, rect, cell);
    let mut f = 1;
    while (orig.w.max(orig.h) - 1) / f + 1 > 160 {
        f *= 2;
    }
    let mut work = if f > 1 {
        orig.downsample(f)
    } else {
        orig.clone()
    };
    let before = work.v.clone();
    let land_max = work.v.iter().copied().fold(sea, f32::max);
    // a big dab is split into rounds (each re-routes the water)
    let rounds = amount.ceil().clamp(1.0, 4.0);
    let st = p.strength.max(0.0) * amount / rounds;
    let fl = Fluvial {
        kf: KF * p.rain * st,
        kc: p.capacity * KF * p.rain.max(0.05),
        deposition: p.deposition.clamp(0.0, 1.0),
        kg: KG * p.glacial * st,
        ela: sea + (land_max - sea) * 0.3,
        sea,
    };
    let mut net = Net::new(work.v.len());
    let mut glacier = vec![];
    for _ in 0..rounds as usize {
        fluvial_round(&mut work, &mut net, &fl, &mut glacier);
    }
    if p.wind > 0.0 {
        let stage_p = Progress::default();
        aeolian(
            &mut work,
            sea,
            p.wind_dir,
            p.wind * st.min(1.0),
            1,
            p.seed ^ ((center.0 as u64) << 20),
            &stage_p.stage(0.0, 1.0),
        );
    }
    let mut g = orig.clone();
    if f > 1 {
        for (v, b) in work.v.iter_mut().zip(&before) {
            *v -= b;
        }
        g.add_upsampled(&work, f);
    } else {
        g = work;
    }
    thermal(
        &mut g,
        p.talus_angle.clamp(20.0, 70.0).to_radians().tan(),
        (0.07 * (p.thermal * amount * 3.0).min(1.0)).max(0.01),
        (p.creep * 0.02 * amount).min(0.05),
        1,
        Some(sea),
    );
    let inner = 1.0 - falloff.clamp(0.0, 1.0);
    for z in 0..g.h {
        for x in 0..g.w {
            let d = (((x0 + x) as f32 - center.0).powi(2) + ((z0 + z) as f32 - center.1).powi(2))
                .sqrt()
                / radius;
            let m = if d >= 1.0 {
                0.0
            } else if d <= inner {
                1.0
            } else {
                1.0 - smoothstep(inner, 1.0, d)
            };
            let k = z * g.w + x;
            heights[(z0 + z) * n + x0 + x] = orig.v[k] + (g.v[k] - orig.v[k]) * m;
        }
    }
    Some(rect)
}

/// Wetness per vertex (0..255) from where rain gathers: the logarithm of the drainage area
/// (multiple flow directions), so gullies, stream beds and valley floors light up. 0 at sea.
pub fn flow_map(heights: &[f32], n: usize, cell: f32, sea: f32) -> Vec<u8> {
    let all = GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    };
    let full = Grid::extract(heights, n, all, cell);
    let mut f = 1;
    while (n - 1) / f + 1 > NET_MAX {
        f *= 2;
    }
    let g = if f > 1 { full.downsample(f) } else { full };
    let mut net = Net::new(g.v.len());
    net.route(&g, sea);
    net.accumulate(&g);
    // log10 of the area through each cell: ~2.6 (a slope) .. 4.6 (a stream bed); the beds of filled
    // hollows stay merely damp (water spreads out there instead of running in a channel)
    let wet: Vec<f32> = (0..g.v.len())
        .map(|k| {
            if g.v[k] <= sea {
                return 0.0;
            }
            let w = ((net.area[k].log10() - 2.6) / 2.0).clamp(0.0, 1.0);
            if net.filled[k] - g.v[k] > 0.01 {
                w.min(0.45)
            } else {
                w
            }
        })
        .collect();
    // channels are a cell wide: widen them a little (3×3 maximum), then soften the edges
    let (w, h) = (g.w, g.h);
    let mut wide = wet.clone();
    par_rows(&mut wide, w, |z, row| {
        for (x, o) in row.iter_mut().enumerate() {
            for zz in z.saturating_sub(1)..=(z + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    *o = o.max(wet[zz * w + xx] * 0.9);
                }
            }
        }
    });
    let mut wet = Grid {
        w,
        h,
        dx: g.dx,
        v: wide,
    };
    blur(&mut wet, 1);
    let mut up = Grid {
        w: n,
        h: n,
        dx: cell,
        v: vec![0.0; n * n],
    };
    up.add_upsampled(&wet, f);
    let mut out = vec![0u8; n * n];
    for (o, v) in out.iter_mut().zip(&up.v) {
        *o = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    }
    out
}
