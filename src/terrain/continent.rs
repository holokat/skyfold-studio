//! Continents and coastlines. A preset gives the landmass its macro shape (one continent, an
//! archipelago, twin islands, a peninsula, a ring atoll, a fjord coast, a mountain island or land
//! around an inland sea); domain-warped fractal noise roughens the coast down to a few metres, so
//! it reads natural at every scale (bays, headlands, offshore islets); the land share is hit
//! exactly by thresholding at the matching percentile. Heights follow the distance to the coast
//! (beaches, a rising interior, a continental shelf dropping to deep sea), mountain ranges grow
//! along wandering spines, and an erosion pass gives them real drainage and ridgelines. The
//! biome / climate generator (`biomegen`) runs on the result like on any other world.

use serde::{Deserialize, Serialize};

use super::GridRect;
use super::erosion::{self, ErosionPreset, Stage};
use crate::noise::{Rng, fbm, perlin2, smoothstep};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContinentPreset {
    #[default]
    Continent,
    Archipelago,
    TwinIslands,
    Peninsula,
    RingAtoll,
    FjordCoast,
    MountainIsland,
    InlandSea,
}

impl ContinentPreset {
    pub const ALL: [ContinentPreset; 8] = [
        ContinentPreset::Continent,
        ContinentPreset::Archipelago,
        ContinentPreset::TwinIslands,
        ContinentPreset::Peninsula,
        ContinentPreset::RingAtoll,
        ContinentPreset::FjordCoast,
        ContinentPreset::MountainIsland,
        ContinentPreset::InlandSea,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ContinentPreset::Continent => "Continent",
            ContinentPreset::Archipelago => "Archipelago",
            ContinentPreset::TwinIslands => "Twin Islands",
            ContinentPreset::Peninsula => "Peninsula",
            ContinentPreset::RingAtoll => "Ring Atoll",
            ContinentPreset::FjordCoast => "Fjord Coast",
            ContinentPreset::MountainIsland => "Mountain Island",
            ContinentPreset::InlandSea => "Inland Sea",
        }
    }
    /// snake_case id (MCP).
    pub fn id(self) -> &'static str {
        match self {
            ContinentPreset::Continent => "continent",
            ContinentPreset::Archipelago => "archipelago",
            ContinentPreset::TwinIslands => "twin_islands",
            ContinentPreset::Peninsula => "peninsula",
            ContinentPreset::RingAtoll => "ring_atoll",
            ContinentPreset::FjordCoast => "fjord_coast",
            ContinentPreset::MountainIsland => "mountain_island",
            ContinentPreset::InlandSea => "inland_sea",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|p| p.id() == s)
    }
    pub fn hint(self) -> &'static str {
        match self {
            ContinentPreset::Continent => {
                "One large landmass with ranges, plains and a ragged coast"
            }
            ContinentPreset::Archipelago => {
                "Dozens of islands of every size scattered across the sea"
            }
            ContinentPreset::TwinIslands => "Two big islands facing each other across a strait",
            ContinentPreset::Peninsula => {
                "A mainland coast with a long arm of land reaching into the sea"
            }
            ContinentPreset::RingAtoll => "A ring of low islands around a shallow turquoise lagoon",
            ContinentPreset::FjordCoast => {
                "A mountainous coast cut by deep, narrow ice-carved inlets"
            }
            ContinentPreset::MountainIsland => "A single island crowned by a high central massif",
            ContinentPreset::InlandSea => {
                "Land wrapped around a great inner sea, open to the ocean by a strait"
            }
        }
    }
    /// Default parameters of the preset.
    pub fn params(self, seed: u32) -> ContinentParams {
        let (land, roughness, ranges, relief, erosion) = match self {
            ContinentPreset::Continent => (0.5, 0.55, 3, 1.0, ErosionPreset::HeavyRain),
            ContinentPreset::Archipelago => (0.24, 0.65, 4, 0.75, ErosionPreset::HeavyRain),
            ContinentPreset::TwinIslands => (0.36, 0.5, 2, 1.0, ErosionPreset::HeavyRain),
            ContinentPreset::Peninsula => (0.42, 0.55, 2, 0.95, ErosionPreset::HeavyRain),
            ContinentPreset::RingAtoll => (0.13, 0.6, 0, 0.18, ErosionPreset::LightRain),
            ContinentPreset::FjordCoast => (0.5, 0.45, 4, 1.35, ErosionPreset::Glacial),
            ContinentPreset::MountainIsland => (0.3, 0.5, 3, 1.45, ErosionPreset::HeavyRain),
            ContinentPreset::InlandSea => (0.5, 0.5, 3, 0.95, ErosionPreset::AncientMountains),
        };
        ContinentParams {
            preset: self,
            land,
            roughness,
            ranges,
            range_dir: None,
            relief,
            seed,
            erosion: Some(erosion),
        }
    }
}

/// Settings of a generated landmass (presets fill them in).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContinentParams {
    pub preset: ContinentPreset,
    /// Share of the map that is land (0.05..0.9).
    pub land: f32,
    /// Coastline raggedness: bays, headlands and offshore islets (0..1).
    pub roughness: f32,
    /// Number of mountain ranges (0..8).
    pub ranges: u32,
    /// Main direction of the ranges in degrees (0 = east–west), or random from the seed.
    pub range_dir: Option<f32>,
    /// Mountain height (×, 0.2..2).
    pub relief: f32,
    pub seed: u32,
    /// Erosion run on the new land (None: raw shapes).
    pub erosion: Option<ErosionPreset>,
}

impl Default for ContinentParams {
    fn default() -> Self {
        ContinentPreset::Continent.params(7)
    }
}

/// Signed distance (map units) into a disc of radius `r` at `c`.
fn blob(u: f32, v: f32, c: (f32, f32), r: f32) -> f32 {
    r - ((u - c.0).powi(2) + (v - c.1).powi(2)).sqrt()
}

/// Macro shape of the preset at map position (u, v) ∈ [-1, 1]²: > 0 leans land.
struct Shape {
    preset: ContinentPreset,
    seed: u32,
    rot: (f32, f32),
    aspect: f32,
    blobs: Vec<((f32, f32), f32)>,
    axis: (f32, f32),
}

impl Shape {
    fn new(preset: ContinentPreset, seed: u32) -> Shape {
        let mut rng = Rng::new(seed as u64 ^ 0xc0_7111e7);
        let a = rng.f32() * std::f32::consts::TAU;
        let axis = (a.cos(), a.sin());
        let mut blobs = vec![];
        match preset {
            ContinentPreset::Archipelago => {
                let k = 18 + rng.below(10) as usize;
                for i in 0..k {
                    // a few big islands, many small ones
                    let r = if i < 3 {
                        rng.range(0.13, 0.2)
                    } else {
                        rng.range(0.03, 0.1)
                    };
                    let lim = 0.8 - r;
                    blobs.push(((rng.range(-lim, lim), rng.range(-lim, lim)), r));
                }
            }
            ContinentPreset::TwinIslands => {
                for s in [-1.0f32, 1.0] {
                    let r = rng.range(0.34, 0.4);
                    let d = rng.range(0.38, 0.44);
                    blobs.push((
                        (
                            axis.0 * d * s + rng.range(-0.06, 0.06),
                            axis.1 * d * s + rng.range(-0.06, 0.06),
                        ),
                        r,
                    ));
                }
            }
            ContinentPreset::InlandSea => {
                blobs.push((
                    (rng.range(-0.12, 0.12), rng.range(-0.12, 0.12)),
                    rng.range(0.34, 0.42),
                ));
            }
            _ => {}
        }
        Shape {
            preset,
            seed,
            rot: (a.cos(), a.sin()),
            aspect: 1.0 + rng.f32() * 0.55,
            blobs,
            axis,
        }
    }

    /// The macro shape wanders a little (domain warp); atolls stay rounder.
    fn warp(&self, u: f32, v: f32) -> (f32, f32) {
        let s = self.seed;
        let k = if self.preset == ContinentPreset::RingAtoll {
            0.09
        } else {
            0.22
        };
        (
            u + fbm(u * 1.1 + 4.1, v * 1.1 - 2.3, 3, 2.0, 0.5, s + 21) * k,
            v + fbm(u * 1.1 - 7.9, v * 1.1 + 5.2, 3, 2.0, 0.5, s + 22) * k,
        )
    }

    fn at(&self, u: f32, v: f32) -> f32 {
        let (wu, wv) = self.warp(u, v);
        // rotated, stretched coordinates
        let (c, sn) = self.rot;
        let (ru, rv) = (wu * c - wv * sn, wu * sn + wv * c);
        let ell = ((ru / self.aspect).powi(2) + (rv * self.aspect.sqrt()).powi(2)).sqrt();
        let d = (wu * wu + wv * wv).sqrt();
        // every shape is (roughly) a signed distance to its coast in map units, so the coast
        // noise moves every shore by the same amount whatever the preset
        match self.preset {
            ContinentPreset::Continent => 0.62 - ell,
            ContinentPreset::Archipelago => self
                .blobs
                .iter()
                .map(|(c, r)| blob(wu, wv, *c, *r))
                .fold(-2.0f32, f32::max),
            ContinentPreset::TwinIslands => {
                let m = self
                    .blobs
                    .iter()
                    .map(|(c, r)| blob(wu, wv, *c, *r))
                    .fold(-2.0f32, f32::max);
                // keep the strait open between them
                let across = (wu * self.axis.0 + wv * self.axis.1).abs();
                m.min((across - 0.08) * 3.0)
            }
            ContinentPreset::Peninsula => {
                // mainland along one edge, an arm reaching from it across the map
                let along = wu * self.axis.0 + wv * self.axis.1; // towards the mainland edge: +1
                let side = -wu * self.axis.1 + wv * self.axis.0;
                let mainland = along - 0.5;
                // the arm bends gently and narrows as it reaches out
                let bend = 0.16 * (along * 2.4 + 0.7).sin();
                let t = ((0.75 - along) / 1.35).clamp(0.0, 1.0);
                let arm = (0.15 * (1.0 - t * 0.65) - (side - bend).abs()).min(along + 0.62);
                mainland.max(arm)
            }
            ContinentPreset::RingAtoll => 0.06 - (d - 0.52).abs(),
            ContinentPreset::FjordCoast => {
                // land on one side of a coast running across the map
                wu * self.axis.0 + wv * self.axis.1 + 0.05
            }
            ContinentPreset::MountainIsland => 0.46 - (ell * 0.85 + d * 0.15),
            ContinentPreset::InlandSea => {
                let outer = 0.74 - (ell * 0.85 + d * 0.15);
                let (c, r) = self.blobs[0];
                let inner = -blob(wu, wv, c, r);
                // a winding strait from the inner sea out to the ocean
                let (pu, pv) = (wu - c.0, wv - c.1);
                let along = pu * self.axis.0 + pv * self.axis.1;
                let across =
                    (-pu * self.axis.1 + pv * self.axis.0 + 0.05 * (along * 7.0).sin()).abs();
                let strait = if along > 0.0 {
                    (across - 0.05) * 3.0
                } else {
                    1.0
                };
                outer.min(inner).min(strait)
            }
        }
    }
}

/// Two-pass chamfer distance (m) to the nearest cell where `seed` is true (n×n grid).
pub(super) fn chamfer(seed: &[bool], n: usize, cell: f32) -> Vec<f32> {
    let mut d: Vec<f32> = seed
        .iter()
        .map(|s| if *s { 0.0 } else { f32::MAX })
        .collect();
    let (a, b) = (cell, cell * std::f32::consts::SQRT_2);
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            let mut v = d[k];
            if i > 0 {
                v = v.min(d[k - 1] + a);
            }
            if j > 0 {
                v = v.min(d[k - n] + a);
                if i > 0 {
                    v = v.min(d[k - n - 1] + b);
                }
                if i + 1 < n {
                    v = v.min(d[k - n + 1] + b);
                }
            }
            d[k] = v;
        }
    }
    for j in (0..n).rev() {
        for i in (0..n).rev() {
            let k = j * n + i;
            let mut v = d[k];
            if i + 1 < n {
                v = v.min(d[k + 1] + a);
            }
            if j + 1 < n {
                v = v.min(d[k + n] + a);
                if i + 1 < n {
                    v = v.min(d[k + n + 1] + b);
                }
                if i > 0 {
                    v = v.min(d[k + n - 1] + b);
                }
            }
            d[k] = v;
        }
    }
    d
}

/// A wandering line (mountain spine, fjord) with a half-width and a peak value.
struct Line {
    pts: Vec<(f32, f32)>,
    width: f32,
    height: f32,
}

/// Soft ridges (or trenches) along lines, on a coarse grid: the maximum over the lines of
/// `height × taper(position along the line) × profile(distance / width)`. Exact segment distances,
/// so it is smooth everywhere (no seams where two lines meet).
struct LineField {
    c: usize,
    cell: f32,
    half: f32,
    v: Vec<f32>,
}

impl LineField {
    fn new(
        lines: &[Line],
        size: f32,
        c: usize,
        seed: u32,
        taper: impl Fn(f32) -> f32 + Sync,
    ) -> LineField {
        let half = size * 0.5;
        let cell = size / (c - 1) as f32;
        let mut v = vec![0.0f32; c * c];
        let bounds: Vec<((f32, f32), (f32, f32))> = lines
            .iter()
            .map(|l| {
                let r = l.width * 1.6;
                let lo = l
                    .pts
                    .iter()
                    .fold((f32::MAX, f32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
                let hi = l
                    .pts
                    .iter()
                    .fold((f32::MIN, f32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
                ((lo.0 - r, lo.1 - r), (hi.0 + r, hi.1 + r))
            })
            .collect();
        erosion::par_rows(&mut v, c, |j, row| {
            let z = j as f32 * cell - half;
            for (i, out) in row.iter_mut().enumerate() {
                let x = i as f32 * cell - half;
                // the edge of each band wanders
                let wob = (1.0 + 0.35 * fbm(x * 0.0017, z * 0.0017, 3, 2.0, 0.5, seed + 977))
                    .clamp(0.6, 1.55);
                let mut best = 0.0f32;
                for (l, (lo, hi)) in lines.iter().zip(&bounds) {
                    if x < lo.0 || z < lo.1 || x > hi.0 || z > hi.1 || l.pts.len() < 2 {
                        continue;
                    }
                    // the best segment (not the nearest point) keeps the field seamless where a
                    // line bends back on itself
                    let segs = (l.pts.len() - 1) as f32;
                    let w2 = (l.width * wob).powi(2);
                    for k in 0..l.pts.len() - 1 {
                        let (a, b) = (l.pts[k], l.pts[k + 1]);
                        let (abx, abz) = (b.0 - a.0, b.1 - a.1);
                        let len2 = (abx * abx + abz * abz).max(1e-6);
                        let s = (((x - a.0) * abx + (z - a.1) * abz) / len2).clamp(0.0, 1.0);
                        let (dx, dz) = (x - a.0 - abx * s, z - a.1 - abz * s);
                        let dd = dx * dx + dz * dz;
                        if dd < w2 {
                            let e = 1.0 - smoothstep(0.0, 1.0, (dd / w2).sqrt());
                            best = best.max(
                                l.height * taper((k as f32 + s) / segs) * e * e * (3.0 - 2.0 * e),
                            );
                        }
                    }
                }
                *out = best;
            }
        });
        // round the creases where bands overlap or a line bends
        let v = erosion::smoothed(&v, c, 1, 2);
        LineField { c, cell, half, v }
    }

    fn sample(&self, x: f32, z: f32) -> f32 {
        let gx = ((x + self.half) / self.cell).clamp(0.0, (self.c - 1) as f32 - 0.001);
        let gz = ((z + self.half) / self.cell).clamp(0.0, (self.c - 1) as f32 - 0.001);
        let (i, j) = (gx as usize, gz as usize);
        let (fx, fz) = (gx - i as f32, gz - j as f32);
        let k = j * self.c + i;
        let a = self.v[k] + (self.v[k + 1] - self.v[k]) * fx;
        let b = self.v[k + self.c] + (self.v[k + self.c + 1] - self.v[k + self.c]) * fx;
        a + (b - a) * fz
    }
}

/// Generates absolute heights for an `res`×`res`-cell world. Returns None when cancelled.
pub fn generate(
    p: &ContinentParams,
    res: usize,
    cell: f32,
    sea: f32,
    progress: Stage,
) -> Option<Vec<f32>> {
    let n = res + 1;
    let size = res as f32 * cell;
    let half = size * 0.5;
    let seed = p.seed;
    let shape = Shape::new(p.preset, seed);
    let rough = p.roughness.clamp(0.0, 1.0);
    let atoll = p.preset == ContinentPreset::RingAtoll;
    let shape_stage = progress.sub(0.0, 0.25);

    // --- landness: macro shape + warped fractal coast noise from ~1/3 of the map down to ~6 m
    let oct = ((size * 0.33 / 30.0).log2().ceil() as u32).clamp(6, 11);
    let gain = 0.55 + 0.1 * rough;
    let amp = 0.1 + 0.8 * rough;
    let base_f = 3.0 / size;
    let mut fjords: Vec<f32> = vec![];
    if p.preset == ContinentPreset::FjordCoast {
        fjords = fjord_field(&shape, seed, n, size);
    }
    let mut land = vec![0.0f32; n * n];
    erosion::par_rows(&mut land, n, |j, row| {
        for (i, out) in row.iter_mut().enumerate() {
            let (x, z) = (i as f32 * cell - half, j as f32 * cell - half);
            let (u, v) = (x / half, z / half);
            // large and medium domain warps: headlands, bays and coves
            let wx = fbm(
                x * base_f * 0.7 + 1.7,
                z * base_f * 0.7 - 3.3,
                4,
                2.0,
                0.5,
                seed + 31,
            ) * size
                * 0.06
                + fbm(x / 650.0, z / 650.0, 3, 2.0, 0.5, seed + 34) * 140.0;
            let wz = fbm(
                x * base_f * 0.7 - 6.1,
                z * base_f * 0.7 + 2.9,
                4,
                2.0,
                0.5,
                seed + 32,
            ) * size
                * 0.06
                + fbm(x / 650.0 + 9.1, z / 650.0, 3, 2.0, 0.5, seed + 35) * 140.0;
            let coast = fbm(
                (x + wx) * base_f,
                (z + wz) * base_f,
                oct,
                2.0,
                gain,
                seed + 33,
            );
            let mut l = shape.at(u, v) + coast * amp;
            if !fjords.is_empty() {
                l -= fjords[j * n + i];
            }
            // the world's rim is always sea (a rounded, ragged rim, never a straight shore)
            let edge = (u.powi(6) + v.powi(6)).powf(1.0 / 6.0)
                + fbm(u * 3.0, v * 3.0, 3, 2.0, 0.5, seed + 36) * 0.08;
            l -= smoothstep(0.76, 0.97, edge) * 1.2;
            *out = l;
        }
    });
    shape_stage.set(0.6);
    if progress.cancelled() {
        return None;
    }
    // threshold at the percentile that gives the asked share of land
    let mut sample: Vec<f32> = land.iter().step_by(7).copied().collect();
    sample.sort_by(|a, b| a.total_cmp(b));
    let q = ((1.0 - p.land.clamp(0.03, 0.92)) * (sample.len() - 1) as f32) as usize;
    let thr = sample[q];
    let is_land: Vec<bool> = land.iter().map(|l| *l > thr).collect();
    // distance inland from the coast and out to sea
    let to_sea = chamfer(&is_land.iter().map(|b| !b).collect::<Vec<_>>(), n, cell);
    let to_land = chamfer(&is_land, n, cell);
    // smooth versions for the broad rise inland and the fall offshore: a raw distance field has
    // sharp creases along its medial axis that water would follow in dead-straight lines
    let mut f = 1;
    while cell * (f * 2) as f32 <= 16.0 {
        f *= 2;
    }
    let r = ((260.0 * (size / 4096.0).max(0.5).sqrt()) / (cell * f as f32))
        .round()
        .max(2.0) as usize;
    let to_sea_s = erosion::smoothed(
        &to_sea.iter().map(|d| d.min(3000.0)).collect::<Vec<_>>(),
        n,
        f,
        r,
    );
    let to_land_s = erosion::smoothed(
        &to_land.iter().map(|d| d.min(3000.0)).collect::<Vec<_>>(),
        n,
        f,
        r,
    );
    shape_stage.set(1.0);
    if progress.cancelled() {
        return None;
    }

    // --- mountain ranges along wandering spines (on a coarse grid, then sampled)
    let ranges = mountain_ranges(p, &shape, &is_land, &to_sea, n, cell, seed);
    let island = p.preset == ContinentPreset::MountainIsland && p.ranges > 0;
    let summit = farthest_from_sea(&to_sea, n, cell);
    let mountain_stage = progress.sub(0.25, 0.4);
    mountain_stage.set(0.5);

    // --- heights
    let scale = (size / 4096.0).max(0.5);
    let peak = 430.0 * p.relief.clamp(0.1, 2.5) * scale.powf(0.22);
    let inland = size * 0.11;
    let cliffs = matches!(
        p.preset,
        ContinentPreset::FjordCoast | ContinentPreset::MountainIsland
    );
    let lagoon_r = 0.52;
    let mut heights = vec![0.0f32; n * n];
    erosion::par_rows(&mut heights, n, |j, row| {
        for (i, out) in row.iter_mut().enumerate() {
            let k = j * n + i;
            let (x, z) = (i as f32 * cell - half, j as f32 * cell - half);
            if is_land[k] {
                let dl = to_sea[k];
                let interior = (1.0 - (-to_sea_s[k] / inland).exp()) * smoothstep(0.0, 220.0, dl);
                // a gentle beach, then the interior rising away from the sea
                let plateau = match p.preset {
                    ContinentPreset::RingAtoll => 4.0,
                    // fjords cut into high, rugged ground
                    ContinentPreset::FjordCoast => 110.0,
                    _ => 42.0,
                };
                let mut h = 3.0 * (1.0 - (-dl / 45.0).exp()) + plateau * interior;
                // rolling hills, none on the beach
                let hills = fbm(x * 0.0021, z * 0.0021, 5, 2.0, 0.5, seed + 41) * 0.5 + 0.5;
                // and broad swells (uplands and lowlands) that scale with the world
                let swells =
                    fbm(x / (size * 0.09), z / (size * 0.09), 3, 2.0, 0.5, seed + 42) * 0.5 + 0.5;
                let relief = if atoll { 0.3 } else { 1.0 };
                h += (hills * hills * 34.0 + swells * swells * 45.0 * scale.sqrt())
                    * relief
                    * smoothstep(20.0, 320.0, dl);
                // mountains
                if let Some(r) = &ranges {
                    let m = r.sample(x, z);
                    if m > 0.0 {
                        let coast_fade = if cliffs {
                            smoothstep(0.0, 60.0, dl)
                        } else {
                            smoothstep(30.0, 420.0, dl)
                        };
                        let ridge = soft_ridges(x, z, seed);
                        let massif =
                            fbm(x * 0.0011 - 3.0, z * 0.0011 + 8.0, 4, 2.0, 0.45, seed + 7) * 0.5
                                + 0.5;
                        h += peak * m * coast_fade * (0.22 + 0.55 * ridge + 0.23 * massif);
                    }
                    if island {
                        // the summit itself: a broad cone the ridges run down from
                        let d = ((x - summit.0).powi(2) + (z - summit.1).powi(2)).sqrt()
                            / (size * 0.16);
                        let cone = (1.0 - d).max(0.0);
                        h +=
                            peak * 0.35 * cone * cone * (0.75 + 0.25 * soft_ridges(x, z, seed + 3));
                    }
                }
                *out = sea + h;
            } else {
                let ds = to_land[k];
                let dss = to_land_s[k];
                let shelf = 11.0 * (1.0 - (-dss / 220.0).exp())
                    + 75.0 * smoothstep(250.0, 1500.0 * scale.sqrt(), dss);
                let mut depth =
                    3.0 * (1.0 - (-ds / 40.0).exp()) + shelf * smoothstep(0.0, 160.0, ds);
                if atoll {
                    // the lagoon inside the ring is shallow and sandy
                    let (u, v) = shape.warp(x / half, z / half);
                    let d = (u * u + v * v).sqrt();
                    let inside = 1.0 - smoothstep(lagoon_r - 0.12, lagoon_r - 0.02, d);
                    depth =
                        depth * (1.0 - inside) + (1.5 + 5.5 * smoothstep(0.0, 180.0, ds)) * inside;
                }
                // sea floor relief
                depth += fbm(x * 0.004, z * 0.004, 3, 2.0, 0.5, seed + 51)
                    * 3.0
                    * smoothstep(10.0, 200.0, ds);
                *out = sea - depth.max(0.4);
            }
        }
    });
    mountain_stage.set(1.0);
    if progress.cancelled() {
        return None;
    }

    // --- erosion gives the ranges real drainage and ridgelines
    if let Some(e) = p.erosion {
        let mut ep = e.params();
        ep.seed = seed as u64;
        let all = GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        };
        if !erosion::erode(
            &mut heights,
            n,
            cell,
            sea,
            all,
            None,
            &ep,
            progress.sub(0.4, 1.0),
        ) {
            return None;
        }
    }
    progress.set(1.0);
    Some(heights)
}

/// World position of the land farthest from the sea.
fn farthest_from_sea(to_sea: &[f32], n: usize, cell: f32) -> (f32, f32) {
    let half = (n - 1) as f32 * cell * 0.5;
    let k = (0..n * n)
        .max_by(|a, b| to_sea[*a].total_cmp(&to_sea[*b]))
        .unwrap_or(0);
    ((k % n) as f32 * cell - half, (k / n) as f32 * cell - half)
}

/// Ridged noise with rounded crests in [0, 1]: secondary ridges and spurs inside a range.
pub(super) fn soft_ridges(x: f32, z: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm, mut weight) = (0.0, 0.5, 0.0024, 0.0, 1.0f32);
    for o in 0..5 {
        let (s, c) = (0.7f32 * o as f32).sin_cos();
        let (rx, rz) = (x * c - z * s, x * s + z * c);
        let n = perlin2(rx * freq, rz * freq, seed.wrapping_add(4 + o * 7919));
        let r = (1.0 - (n * n + 0.01).sqrt()).max(0.0);
        let r = r * r * weight;
        weight = (r * 1.6).clamp(0.0, 1.0);
        sum += r * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    (sum / norm / 0.75).clamp(0.0, 1.0)
}

/// Mountain ranges: spines wandering across the land, as a field of relative peak heights.
fn mountain_ranges(
    p: &ContinentParams,
    shape: &Shape,
    is_land: &[bool],
    to_sea: &[f32],
    n: usize,
    cell: f32,
    seed: u32,
) -> Option<LineField> {
    let count = p.ranges.min(8) as usize;
    if count == 0 {
        return None;
    }
    let size = (n - 1) as f32 * cell;
    let half = size * 0.5;
    let mut rng = Rng::new(seed as u64 ^ 0x4a96_e5);
    let land_cells = is_land.iter().filter(|b| **b).count();
    let land_extent = ((land_cells as f32).sqrt() * cell).max(size * 0.15);
    let scale = (size / 4096.0).max(0.5);
    let base_dir = p
        .range_dir
        .map(|d| d.to_radians())
        .unwrap_or_else(|| match p.preset {
            // fjord coasts: ranges run along the coast
            ContinentPreset::FjordCoast | ContinentPreset::Peninsula => {
                shape.axis.1.atan2(shape.axis.0) + std::f32::consts::FRAC_PI_2
            }
            _ => rng.f32() * std::f32::consts::PI,
        });
    let land_at = |x: f32, z: f32| -> Option<f32> {
        let i = ((x + half) / cell).round();
        let j = ((z + half) / cell).round();
        if i < 0.0 || j < 0.0 || i >= n as f32 || j >= n as f32 {
            return None;
        }
        let k = j as usize * n + i as usize;
        is_land[k].then_some(to_sea[k])
    };
    // the mountain island's summit: the spot farthest from the sea
    let summit = farthest_from_sea(to_sea, n, cell);
    let mut spines: Vec<Line> = vec![];
    for r in 0..count {
        // start well inland (twin islands: alternate between the islands)
        let mut start = None;
        for _ in 0..400 {
            let (x, z) = (
                rng.range(-half * 0.85, half * 0.85),
                rng.range(-half * 0.85, half * 0.85),
            );
            if p.preset == ContinentPreset::TwinIslands
                && ((x * shape.axis.0 + z * shape.axis.1) > 0.0) != (r % 2 == 0)
            {
                continue;
            }
            if p.preset == ContinentPreset::MountainIsland {
                start = Some(summit);
                break;
            }
            // fjord coasts: the ranges stand right behind the coast, where the fjords cut in
            let band = if p.preset == ContinentPreset::FjordCoast {
                (size * 0.03, size * 0.1)
            } else {
                (land_extent * 0.08, f32::MAX)
            };
            if land_at(x, z).is_some_and(|d| d > band.0 && d < band.1) {
                start = Some((x, z));
                break;
            }
        }
        let Some(start) = start else { continue };
        let dir = match p.preset {
            // radiating from the summit
            ContinentPreset::MountainIsland => {
                base_dir + r as f32 * std::f32::consts::TAU / count as f32
            }
            _ => base_dir + rng.range(-0.6, 0.6),
        };
        let island = p.preset == ContinentPreset::MountainIsland;
        let length = land_extent * rng.range(0.45, 0.9) * if island { 0.9 } else { 1.0 };
        let width =
            560.0 * scale.powf(0.45) * rng.range(0.8, 1.25) * if island { 1.3 } else { 1.0 };
        let height = rng.range(0.75, 1.0);
        let step = 30.0 * scale.sqrt();
        let mut pts = vec![start];
        let sides: &[f32] = if p.preset == ContinentPreset::MountainIsland {
            &[1.0]
        } else {
            &[1.0, -1.0]
        };
        for &side in sides {
            let (mut x, mut z) = start;
            let mut a = dir
                + if side < 0.0 {
                    std::f32::consts::PI
                } else {
                    0.0
                };
            let mut walked = 0.0;
            let turn_seed = seed + 300 + r as u32 * 7 + (side > 0.0) as u32;
            while walked < length * 0.5 {
                a += perlin2(walked * 0.0012, 0.5, turn_seed) * 0.06;
                x += a.cos() * step;
                z += a.sin() * step;
                walked += step;
                let reach = if matches!(
                    p.preset,
                    ContinentPreset::FjordCoast | ContinentPreset::MountainIsland
                ) {
                    0.0
                } else {
                    (land_extent * 0.05).min(120.0)
                };
                if land_at(x, z).is_none_or(|d| d < reach) {
                    break;
                }
                if side > 0.0 {
                    pts.push((x, z));
                } else {
                    pts.insert(0, (x, z));
                }
            }
        }
        if pts.len() < 2 {
            // a small island: a short ridge through its middle
            let (x, z) = pts[0];
            pts = vec![
                (x - dir.cos() * width * 0.3, z - dir.sin() * width * 0.3),
                (x + dir.cos() * width * 0.3, z + dir.sin() * width * 0.3),
            ];
        }
        spines.push(Line { pts, width, height });
    }
    if spines.is_empty() {
        return None;
    }
    // heights taper towards the ends of each range (a mountain island's ridges from the summit down)
    let island = p.preset == ContinentPreset::MountainIsland;
    let taper = move |t: f32| {
        if island {
            (1.0 - t * 0.75).max(0.2)
        } else {
            (t * std::f32::consts::PI)
                .sin()
                .max(0.0)
                .powf(0.6)
                .max(0.25)
        }
    };
    Some(LineField::new(&spines, size, 320, seed, taper))
}

/// Fjords: narrow inlets running inland from the coast (subtracted from the landness).
fn fjord_field(shape: &Shape, seed: u32, n: usize, size: f32) -> Vec<f32> {
    let mut rng = Rng::new(seed as u64 ^ 0xf10d);
    let half = size * 0.5;
    let mut lines = vec![];
    let (ax, az) = shape.axis; // towards the land
    let (px, pz) = (-az, ax); // along the coast
    let count = 6 + rng.below(5) as usize;
    let scale = (size / 4096.0).max(0.5);
    for k in 0..count {
        let along = (k as f32 + 0.5 + rng.range(-0.3, 0.3)) / count as f32 * 1.5 - 0.75;
        // start offshore, run inland with a wandering, branching course
        let (mut x, mut z) = (
            (px * along - ax * 0.35) * half,
            (pz * along - az * 0.35) * half,
        );
        let mut a = az.atan2(ax) + rng.range(-0.3, 0.3);
        let length = rng.range(0.45, 0.85) * half;
        let width = rng.range(55.0, 110.0) * scale.sqrt();
        let step = 20.0 * scale.sqrt();
        let mut walked = 0.0;
        let mut pts = vec![(x, z)];
        let mut branch_at = length * rng.range(0.3, 0.6);
        while walked < length {
            a += perlin2(walked * 0.0025, k as f32 * 3.7, seed + 600) * 0.22;
            x += a.cos() * step;
            z += a.sin() * step;
            walked += step;
            if x.abs() > half || z.abs() > half {
                break;
            }
            pts.push((x, z));
            // a side arm splits off part way in
            if walked > branch_at {
                branch_at = f32::MAX;
                let side = if rng.f32() < 0.5 { -1.0 } else { 1.0 };
                let (mut bx, mut bz, mut ba) = (x, z, a + side * rng.range(0.5, 0.9));
                let mut bpts = vec![(bx, bz)];
                for s in 0..(length * rng.range(0.2, 0.4) / step) as usize {
                    ba += perlin2(s as f32 * 0.05, k as f32 * 5.1, seed + 640) * 0.25;
                    bx += ba.cos() * step;
                    bz += ba.sin() * step;
                    bpts.push((bx, bz));
                }
                lines.push(Line {
                    pts: bpts,
                    width: width * 0.7,
                    height: 0.8,
                });
            }
        }
        lines.push(Line {
            pts,
            width,
            height: 1.0,
        });
    }
    // deep at the mouth, shallowing towards the head of each fjord
    let field = LineField::new(&lines, size, 640, seed + 600, |t| 1.0 - t * t * 0.6);
    let mut out = vec![0.0f32; n * n];
    let (cell, half) = (size / (n - 1) as f32, size * 0.5);
    erosion::par_rows(&mut out, n, |j, row| {
        for (i, o) in row.iter_mut().enumerate() {
            *o = field.sample(i as f32 * cell - half, j as f32 * cell - half) * 0.6;
        }
    });
    out
}
