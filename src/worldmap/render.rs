//! The map generator: derived fields (hillshade, coast distance, forest density, zone borders),
//! the per-pixel base of each style, then the vector layers, symbols, lettering and
//! decorations on top. Runs on a background thread from a [`MapInput`] snapshot.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use bevy::math::{Vec2, Vec3};
use rayon::prelude::*;

use super::canvas::{Canvas, Fonts, Rgba};
use super::field::{Grid, edt};
use super::snapshot::{MapInput, TreeKind};
use super::{MapSettings, MapStyle, decor, features, labels};
use crate::biomes::BiomePreset;
use crate::noise::{smoothstep, value2};

/// Progress and cancellation shared with the UI.
#[derive(Default)]
pub struct Progress {
    permille: AtomicU32,
    stage: Mutex<&'static str>,
    cancel: AtomicBool,
}

impl Progress {
    pub fn set(&self, frac: f32, stage: &'static str) {
        self.permille
            .store((frac.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        if let Ok(mut s) = self.stage.lock() {
            *s = stage;
        }
    }
    pub fn get(&self) -> (f32, &'static str) {
        (
            self.permille.load(Ordering::Relaxed) as f32 / 1000.0,
            self.stage.lock().map(|s| *s).unwrap_or(""),
        )
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A finished map.
pub struct MapResult {
    pub settings: MapSettings,
    pub size: u32,
    /// Final RGBA8 image.
    pub rgba: Vec<u8>,
    /// The same without lettering, icons and decorations (minimap tiles).
    pub clean: Vec<u8>,
    /// Fog-of-war mask (L8, `fog_size`²): 255 hidden, 0 revealed.
    pub fog: Vec<u8>,
    pub fog_size: u32,
    /// Everything a game needs to use the map (transform, labels, icons, zones…).
    pub meta: serde_json::Value,
    /// The places drawn on it (for the minimap).
    pub pois: Vec<super::minimap::Marker>,
    /// World xz of the map's north-west corner and its side (m).
    pub world_min: Vec2,
    pub world_size: f32,
    pub millis: u32,
    pub timings: Vec<(&'static str, u32)>,
}

/// A style's palette and weights.
pub struct Kit {
    pub style: MapStyle,
    pub coast: Rgba,
    pub coast_w: f32,
    pub ripple: Option<Rgba>,
    pub water_ink: Rgba,
    pub river: Rgba,
    /// River water (rivers are drawn as filled lines with inked banks).
    pub river_fill: Rgba,
    pub border: Rgba,
    pub border_shadow: Option<Rgba>,
    /// Zone tint along borders (alpha of the band).
    pub band: f32,
    pub road: Rgba,
    pub road_casing: Option<Rgba>,
    pub building: Rgba,
    pub ink: Rgba,
    pub label: Rgba,
    pub halo: Rgba,
    pub water_label: Rgba,
    pub region_label: Rgba,
    pub region_sub: Rgba,
    pub grid: Rgba,
    pub serif: bool,
}

impl Kit {
    pub fn for_style(s: MapStyle) -> Kit {
        use super::canvas::{rgb, rgba};
        match s {
            MapStyle::Parchment => Kit {
                style: s,
                coast: rgb(58, 42, 30),
                coast_w: 1.9,
                ripple: Some(rgba(66, 84, 92, 255)),
                water_ink: rgb(48, 78, 104),
                river: rgb(52, 86, 116),
                river_fill: rgb(172, 196, 198),
                border: rgb(122, 38, 46),
                border_shadow: None,
                band: 0.30,
                road: rgb(118, 56, 36),
                road_casing: None,
                building: rgb(70, 50, 36),
                ink: rgb(60, 44, 32),
                label: rgb(46, 34, 24),
                halo: rgba(240, 229, 200, 215),
                water_label: rgb(38, 70, 98),
                region_label: rgb(92, 34, 30),
                region_sub: rgb(98, 72, 50),
                grid: rgba(92, 70, 48, 70),
                serif: true,
            },
            MapStyle::Atlas => Kit {
                style: s,
                coast: rgb(42, 96, 146),
                coast_w: 1.1,
                ripple: None,
                water_ink: rgb(52, 112, 164),
                river: rgb(64, 132, 196),
                river_fill: rgb(160, 206, 232),
                border: rgb(150, 54, 140),
                border_shadow: Some(rgba(255, 255, 255, 150)),
                band: 0.22,
                road: rgb(246, 164, 70),
                road_casing: Some(rgb(110, 84, 64)),
                building: rgb(92, 80, 74),
                ink: rgb(70, 60, 52),
                label: rgb(38, 38, 42),
                halo: rgba(255, 255, 255, 210),
                water_label: rgb(36, 96, 156),
                region_label: rgb(84, 46, 92),
                region_sub: rgb(96, 84, 100),
                grid: rgba(60, 60, 70, 60),
                serif: false,
            },
            MapStyle::Minimal => Kit {
                style: s,
                coast: rgba(30, 52, 70, 150),
                coast_w: 0.9,
                ripple: None,
                water_ink: rgba(36, 70, 96, 140),
                river: rgb(62, 108, 138),
                river_fill: rgb(78, 128, 158),
                border: rgba(255, 255, 255, 170),
                border_shadow: Some(rgba(20, 30, 40, 90)),
                band: 0.16,
                road: rgb(244, 236, 214),
                road_casing: Some(rgba(60, 54, 46, 150)),
                building: rgb(88, 78, 70),
                ink: rgb(40, 40, 40),
                label: rgb(255, 255, 255),
                halo: rgba(20, 28, 36, 200),
                water_label: rgb(214, 236, 250),
                region_label: rgb(255, 250, 236),
                region_sub: rgb(222, 226, 232),
                grid: rgba(255, 255, 255, 40),
                serif: false,
            },
            MapStyle::Satellite => Kit {
                style: s,
                coast: rgba(0, 0, 0, 0),
                coast_w: 0.0,
                ripple: None,
                water_ink: rgba(0, 0, 0, 0),
                river: rgba(0, 0, 0, 0),
                river_fill: rgba(0, 0, 0, 0),
                border: rgba(255, 236, 170, 220),
                border_shadow: Some(rgba(0, 0, 0, 120)),
                band: 0.0,
                road: rgba(250, 238, 210, 200),
                road_casing: Some(rgba(0, 0, 0, 90)),
                building: rgba(0, 0, 0, 0),
                ink: rgb(30, 30, 30),
                label: rgb(255, 255, 255),
                halo: rgba(0, 0, 0, 170),
                water_label: rgb(206, 232, 252),
                region_label: rgb(255, 246, 222),
                region_sub: rgb(236, 236, 236),
                grid: rgba(255, 255, 255, 55),
                serif: false,
            },
        }
    }
}

/// Derived rasters shared by every layer.
pub struct Ctx<'a> {
    pub inp: &'a MapInput,
    pub settings: &'a MapSettings,
    pub kit: Kit,
    /// Map size (px).
    pub w: u32,
    /// Map px per terrain cell.
    pub k: f32,
    /// Highest land above sea level (m), at least 1.
    pub hmax: f32,
    pub dmax: f32,
    /// Hillshade relative to flat ground (1 = flat), per vertex.
    pub relief: Grid,
    /// 1 − normal.y per vertex.
    pub slope: Grid,
    /// Signed distance to the coast in map px (+ at sea), on `coast_f`× the vertex grid.
    pub coast: Grid,
    pub coast_f: f32,
    /// Heights resampled onto the coast grid (the coastline is its sea-level iso-line).
    pub coast_h: Grid,
    /// Lake depth (river corridors removed: rivers are drawn as lines).
    pub water: Grid,
    /// Woodland density 0..1 on a grid of `forest_cell` m.
    pub forest: Grid,
    pub forest_kind: Vec<TreeKind>,
    pub forest_cell: f32,
    /// Distance (map px) to the nearest zone border, per vertex.
    pub zone_d: Grid,
}

impl Ctx<'_> {
    /// Map px → terrain grid coordinates (vertex units).
    #[inline]
    pub fn grid(&self, p: Vec2) -> Vec2 {
        p / self.k
    }
    #[inline]
    pub fn world_to_px(&self, p: Vec2) -> Vec2 {
        (p + Vec2::splat(self.inp.half())) / self.inp.cell * self.k
    }
    #[inline]
    pub fn px_to_world(&self, p: Vec2) -> Vec2 {
        p / self.k * self.inp.cell - Vec2::splat(self.inp.half())
    }
    /// Map px per metre.
    pub fn ppm(&self) -> f32 {
        self.k / self.inp.cell
    }
    pub fn height(&self, p: Vec2) -> f32 {
        let g = self.grid(p);
        self.inp.heights.bicubic(g.x, g.y)
    }
    pub fn inland_water(&self, p: Vec2) -> bool {
        let g = self.grid(p);
        self.water.bilinear(g.x, g.y) > 0.0
    }
    pub fn forest_at(&self, p: Vec2) -> f32 {
        let g = self.px_to_world(p) / self.forest_cell
            + Vec2::splat(self.inp.half() / self.forest_cell);
        self.forest.bilinear(g.x - 0.5, g.y - 0.5)
    }
    pub fn forest_kind_at(&self, p: Vec2) -> TreeKind {
        let g = (self.px_to_world(p) + Vec2::splat(self.inp.half())) / self.forest_cell;
        let (i, j) = (
            (g.x as usize).min(self.forest.w - 1),
            (g.y as usize).min(self.forest.h - 1),
        );
        self.forest_kind[j * self.forest.w + i]
    }
    pub fn biome_at(&self, p: Vec2) -> u8 {
        let g = self.grid(p);
        let n = self.inp.n();
        let (i, j) = (
            (g.x.round().max(0.0) as usize).min(n - 1),
            (g.y.round().max(0.0) as usize).min(n - 1),
        );
        self.inp.biomes[j * n + i]
    }
    pub fn coast_d(&self, p: Vec2) -> f32 {
        let g = self.grid(p) * self.coast_f;
        self.coast.bilinear(g.x, g.y)
    }
}

/// Builds the derived rasters.
fn fields<'a>(inp: &'a MapInput, settings: &'a MapSettings) -> Ctx<'a> {
    let n = inp.n();
    let w = settings.size;
    let k = w as f32 / inp.res as f32;
    let h = &inp.heights;
    let hmax = h.v.iter().fold(1.0f32, |m, v| m.max(v - inp.sea));
    let dmax = h.v.iter().fold(1.0f32, |m, v| m.max(inp.sea - v));

    // hillshade: north-west light with a little fill from the west and north (multi-directional),
    // vertically exaggerated so gentle relief still reads
    let exag = 2.2;
    let mut relief = Grid::new(n, n, 1.0);
    let mut slope = Grid::new(n, n, 0.0);
    let lights: [(f32, f32); 3] = [(315.0, 0.62), (270.0, 0.2), (0.0, 0.18)];
    let alt = 42f32.to_radians();
    let ls: Vec<Vec3> = lights
        .iter()
        .map(|(az, _)| {
            let a = az.to_radians();
            // east, south, up
            Vec3::new(a.sin() * alt.cos(), -a.cos() * alt.cos(), alt.sin())
        })
        .collect();
    let flat = alt.sin();
    relief
        .v
        .par_chunks_mut(n)
        .zip(slope.v.par_chunks_mut(n))
        .enumerate()
        .for_each(|(j, (rrow, srow))| {
            for i in 0..n {
                let (ii, jj) = (i as isize, j as isize);
                let dx = (h.at(ii + 1, jj) - h.at(ii - 1, jj)) / (2.0 * inp.cell);
                let dz = (h.at(ii, jj + 1) - h.at(ii, jj - 1)) / (2.0 * inp.cell);
                let nn = Vec3::new(-dx * exag, -dz * exag, 1.0).normalize();
                let mut s = 0.0;
                for (l, (_, wgt)) in ls.iter().zip(lights.iter()) {
                    s += nn.dot(*l).max(0.0) * wgt;
                }
                rrow[i] = s / flat;
                srow[i] = 1.0 - Vec3::new(-dx, -dz, 1.0).normalize().z;
            }
        });

    // signed coast distance on a finer grid for big maps (smooth ripple lines)
    let coast_f = if w as f32 / inp.res as f32 >= 3.5 {
        2.0
    } else {
        1.0
    };
    let cn = ((n - 1) as f32 * coast_f) as usize + 1;
    let hc = if coast_f > 1.0 {
        h.resized(cn, cn)
    } else {
        h.clone()
    };
    let land: Vec<bool> = hc.v.iter().map(|v| *v > inp.sea).collect();
    let sea: Vec<bool> = land.iter().map(|l| !l).collect();
    let to_land = edt(&land, cn, cn);
    let to_sea = edt(&sea, cn, cn);
    let px_per = k / coast_f;
    // (each side measured to the nearest cell of the other, half a cell short of the shore; a
    // light blur rounds off the cell steps so offset lines stay smooth)
    let mut coast = Grid::from_vec(
        cn,
        cn,
        (0..cn * cn)
            .map(|i| {
                if land[i] {
                    (0.5 - to_sea[i]) * px_per
                } else {
                    (to_land[i] - 0.5) * px_per
                }
            })
            .collect(),
    );
    coast.blur(1, 2);

    // rivers are drawn as lines along their course: take their corridors out of the water raster
    let mut water = inp.water.clone();
    for r in &inp.rivers {
        if r.pts.len() < 2 {
            continue;
        }
        let reach = r.width * 0.5 + inp.cell * 1.5;
        for seg in r.pts.windows(2) {
            let (a, b) = (seg[0], seg[1]);
            let lo = (a.min(b) - Vec2::splat(reach) + Vec2::splat(inp.half())) / inp.cell;
            let hi = (a.max(b) + Vec2::splat(reach) + Vec2::splat(inp.half())) / inp.cell;
            for j in (lo.y.floor().max(0.0) as usize)..=(hi.y.ceil() as usize).min(n - 1) {
                for i in (lo.x.floor().max(0.0) as usize)..=(hi.x.ceil() as usize).min(n - 1) {
                    let p = Vec2::new(i as f32, j as f32) * inp.cell - Vec2::splat(inp.half());
                    let ab = b - a;
                    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    if p.distance(a + ab * t) < reach {
                        water.v[j * n + i] = water.v[j * n + i].min(-0.5);
                    }
                }
            }
        }
    }

    // woodland density from the placed trees (or from forest biomes in a world without trees)
    let forest_cell = (inp.cell * 2.0).max(6.0);
    let fw = (inp.size_m() / forest_cell).ceil() as usize;
    let mut counts = vec![0.0f32; fw * fw];
    let mut kinds = vec![[0u32; 4]; fw * fw];
    for t in &inp.trees {
        let g = (t.pos + Vec2::splat(inp.half())) / forest_cell;
        if g.x < 0.0 || g.y < 0.0 {
            continue;
        }
        let (i, j) = (g.x as usize, g.y as usize);
        if i < fw && j < fw {
            counts[j * fw + i] += 1.0;
            kinds[j * fw + i][t.kind as usize] += 1;
        }
    }
    let mut forest = Grid::from_vec(fw, fw, counts);
    if inp.trees.is_empty() {
        let woods = [
            BiomePreset::TemperateForest,
            BiomePreset::Taiga,
            BiomePreset::AutumnWoods,
            BiomePreset::Jungle,
            BiomePreset::HauntedWoods,
            BiomePreset::Swamp,
        ];
        for j in 0..fw {
            for i in 0..fw {
                let p = (Vec2::new(i as f32, j as f32) + 0.5) * forest_cell;
                let g = p / inp.cell;
                let (vi, vj) = ((g.x as usize).min(n - 1), (g.y as usize).min(n - 1));
                let b = BiomePreset::from_id(inp.biomes[vj * n + vi]);
                let hh = inp.heights.bilinear(g.x, g.y) - inp.sea;
                let wooded = b.is_some_and(|b| woods.contains(&b));
                let noise = value2(p.x * 0.012, p.y * 0.012, 91);
                if wooded
                    && hh > 3.0
                    && inp.heights.bilinear(g.x, g.y) < inp.snow_line
                    && noise > 0.35
                {
                    forest.v[j * fw + i] = 1.0;
                }
                if let Some(b) = b {
                    let kind = match b {
                        BiomePreset::Taiga | BiomePreset::Alpine | BiomePreset::Tundra => {
                            TreeKind::Conifer
                        }
                        BiomePreset::Jungle | BiomePreset::TropicalCoast => TreeKind::Palm,
                        BiomePreset::HauntedWoods | BiomePreset::Volcanic => TreeKind::Dead,
                        _ => TreeKind::Broadleaf,
                    };
                    kinds[j * fw + i][kind as usize] += 1;
                }
            }
        }
    }
    // trees per hectare over ~25 m, judged against this world's own woods (an open savanna
    // and a dense jungle both get their forests on the map)
    forest.blur(2, 2);
    let per_ha = 10_000.0 / (forest_cell * forest_cell);
    forest.v.iter_mut().for_each(|c| *c *= per_ha);
    let mut dense: Vec<f32> = forest.v.iter().copied().filter(|d| *d > 3.0).collect();
    dense.sort_by(|a, b| a.total_cmp(b));
    let reference = dense
        .get(dense.len() * 85 / 100)
        .copied()
        .unwrap_or(0.0)
        .max(25.0);
    forest
        .v
        .iter_mut()
        .for_each(|d| *d = smoothstep(reference * 0.18, reference * 0.7, *d));
    // majority kind per cell, spread to empty neighbours so glyphs at forest edges keep their kind
    let mut forest_kind = vec![TreeKind::Broadleaf; fw * fw];
    let order = [
        TreeKind::Broadleaf,
        TreeKind::Conifer,
        TreeKind::Palm,
        TreeKind::Dead,
    ];
    for j in 0..fw {
        for i in 0..fw {
            let mut acc = [0u32; 4];
            for dj in -2i32..=2 {
                for di in -2i32..=2 {
                    let (x, y) = (i as i32 + di, j as i32 + dj);
                    if x >= 0 && y >= 0 && (x as usize) < fw && (y as usize) < fw {
                        let c = kinds[y as usize * fw + x as usize];
                        for q in 0..4 {
                            acc[q] += c[q];
                        }
                    }
                }
            }
            let best = (0..4).max_by_key(|q| acc[*q]).unwrap_or(0);
            forest_kind[j * fw + i] = order[best];
        }
    }

    // distance to zone borders (only borders between a zone and anything else)
    let z = &inp.zone_ids;
    let shown: Vec<bool> = (0..=255u16)
        .map(|id| inp.zones.iter().any(|zz| zz.id as u16 == id))
        .collect();
    let mut border = vec![false; n * n];
    for j in 0..n {
        for i in 0..n {
            let a = z[j * n + i];
            let mut b = false;
            for (di, dj) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                let (x, y) = (i as isize + di, j as isize + dj);
                if x < 0 || y < 0 || x >= n as isize || y >= n as isize {
                    continue;
                }
                let c = z[y as usize * n + x as usize];
                if c != a && (shown[a as usize] || shown[c as usize]) {
                    b = true;
                }
            }
            border[j * n + i] = b;
        }
    }
    let zd = edt(&border, n, n);
    let zone_d = Grid::from_vec(n, n, zd.into_iter().map(|d| d * k).collect());

    Ctx {
        inp,
        settings,
        kit: Kit::for_style(settings.style),
        w,
        k,
        hmax,
        dmax,
        relief,
        slope,
        coast,
        coast_f,
        coast_h: hc,
        water,
        forest,
        forest_kind,
        forest_cell,
        zone_d,
    }
}

// ------------------------------------------------------------------ base rasters

type C3 = [f32; 3];

#[inline]
fn mix(a: C3, b: C3, t: f32) -> C3 {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
#[inline]
fn mul(a: C3, b: C3) -> C3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
#[inline]
fn scale(a: C3, s: f32) -> C3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn c3(r: u8, g: u8, b: u8) -> C3 {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}
fn ramp(stops: &[(f32, C3)], t: f32) -> C3 {
    if t <= stops[0].0 {
        return stops[0].1;
    }
    for w in stops.windows(2) {
        if t <= w[1].0 {
            let f = (t - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
            return mix(w[0].1, w[1].1, smoothstep(0.0, 1.0, f));
        }
    }
    stops[stops.len() - 1].1
}

/// Aged paper: mottled, fibrous, a few stains, darker towards the edges.
fn paper(x: f32, y: f32, w: f32, u: f32) -> C3 {
    let base = c3(239, 227, 198);
    let nx = x / w;
    let ny = y / w;
    let mot = crate::noise::fbm(nx * 5.0, ny * 5.0, 4, 2.1, 0.5, 11);
    let mid = value2(x / (u * 38.0), y / (u * 38.0), 12) - 0.5;
    let fibre = value2(x / (u * 1.6), y / (u * 9.0), 13) - 0.5;
    let grain = value2(x / (u * 0.9), y / (u * 0.9), 14) - 0.5;
    let mut c = scale(
        base,
        1.0 + mot * 0.045 + mid * 0.03 + fibre * 0.022 + grain * 0.02,
    );
    // water stains: faint darker rings
    let s = value2(nx * 3.1 + 7.0, ny * 3.1, 15);
    let ring = smoothstep(0.72, 0.75, s) * (1.0 - smoothstep(0.75, 0.80, s));
    let blot = smoothstep(0.74, 0.9, s);
    c = mul(
        c,
        mix([1.0; 3], c3(220, 198, 160), ring * 0.16 + blot * 0.12),
    );
    // burnt, handled edges
    let edge = nx.min(ny).min(1.0 - nx).min(1.0 - ny);
    let wob = value2(nx * 14.0, ny * 14.0, 16) * 0.025;
    let burn = 1.0 - smoothstep(0.0, 0.07 + wob, edge);
    mix(c, c3(150, 112, 70), burn.powf(1.6) * 0.55)
}

fn biome_wash(b: u8) -> Option<C3> {
    let p = BiomePreset::from_id(b)?;
    Some(match p {
        BiomePreset::Desert => c3(255, 240, 205),
        BiomePreset::Swamp => c3(226, 234, 206),
        BiomePreset::Volcanic => c3(214, 200, 192),
        BiomePreset::Tundra | BiomePreset::Alpine => c3(246, 246, 240),
        BiomePreset::TropicalCoast | BiomePreset::Jungle => c3(236, 246, 210),
        BiomePreset::AutumnWoods => c3(252, 236, 210),
        BiomePreset::HauntedWoods => c3(228, 228, 218),
        _ => c3(244, 246, 222),
    })
}

/// Colour of one pixel of the base layer (everything raster: land, water, relief, zone bands).
fn base_px(c: &Ctx, x: f32, y: f32) -> C3 {
    let inp = c.inp;
    let p = Vec2::new(x, y);
    let g = c.grid(p);
    let h = inp.heights.bicubic(g.x, g.y);
    let alt = h - inp.sea;
    let r = c.relief.bicubic(g.x, g.y);
    let sl = c.slope.bilinear(g.x, g.y);
    let wd = c.water.bilinear(g.x, g.y);
    let u = c.w as f32 / 2048.0;
    match c.kit.style {
        MapStyle::Parchment => {
            let pap = paper(x, y, c.w as f32, u);
            if alt <= 0.0 {
                let d = c.coast_d(p);
                let near = (-d / (u * 46.0)).exp();
                let deep = smoothstep(0.0, c.dmax.max(30.0), -alt);
                let wash = mix(
                    mix(c3(214, 220, 204), c3(196, 208, 200), deep),
                    c3(170, 198, 198),
                    near,
                );
                return mul(pap, scale(wash, 1.0 / 0.93));
            }
            let mut wash = mix(c3(246, 244, 220), c3(250, 240, 222), alt / c.hmax);
            if let Some(b) = biome_wash(c.biome_at(p)) {
                wash = mix(wash, b, 0.7);
            }
            let f = c.forest_at(p);
            wash = mix(wash, c3(214, 226, 178), smoothstep(0.2, 0.6, f) * 0.85);
            if h > inp.snow_line {
                wash = mix(
                    wash,
                    c3(255, 255, 255),
                    smoothstep(inp.snow_line, inp.snow_line + 40.0, h),
                );
            }
            // beaches
            wash = mix(
                wash,
                c3(255, 244, 214),
                (1.0 - smoothstep(0.5, 3.0, alt)) * 0.8,
            );
            let mut col = mul(pap, scale(wash, 1.0 / 0.97));
            // relief: sepia shadows, warm light
            let shade = (1.0 - r).max(0.0);
            col = mul(
                col,
                mix([1.0; 3], c3(168, 138, 104), (shade * 0.8).min(0.6)),
            );
            col = mix(col, c3(255, 250, 236), ((r - 1.0).max(0.0) * 0.45).min(0.3));
            col = zone_band(c, g, col);
            if wd > -0.2 {
                let wet = smoothstep(-0.05, 0.12, wd);
                col = mix(col, mul(pap, c3(170, 198, 206)), wet);
            }
            col
        }
        MapStyle::Atlas => {
            if alt <= 0.0 {
                let t = (-alt / c.dmax.max(40.0)).clamp(0.0, 1.0).powf(0.7);
                let mut col = ramp(
                    &[
                        (0.0, c3(206, 232, 244)),
                        (0.25, c3(170, 210, 234)),
                        (0.6, c3(126, 176, 214)),
                        (1.0, c3(92, 140, 190)),
                    ],
                    t,
                );
                col = mul(col, mix([1.0; 3], [r.min(1.2); 3], 0.07));
                return col;
            }
            let t = alt / c.hmax;
            let mut col = ramp(
                &[
                    (0.0, c3(150, 196, 140)),
                    (0.06, c3(172, 208, 146)),
                    (0.18, c3(214, 226, 162)),
                    (0.32, c3(238, 226, 170)),
                    (0.48, c3(226, 196, 146)),
                    (0.64, c3(202, 164, 128)),
                    (0.78, c3(178, 156, 146)),
                    (0.9, c3(214, 210, 208)),
                    (1.0, c3(250, 250, 250)),
                ],
                t,
            );
            if let Some(b) = BiomePreset::from_id(c.biome_at(p)) {
                let tint = match b {
                    BiomePreset::Desert => Some((c3(240, 216, 160), 0.55)),
                    BiomePreset::Swamp => Some((c3(150, 170, 120), 0.3)),
                    BiomePreset::Volcanic => Some((c3(150, 130, 124), 0.45)),
                    BiomePreset::Tundra => Some((c3(220, 226, 220), 0.4)),
                    _ => None,
                };
                if let Some((tc, a)) = tint {
                    col = mix(col, tc, a);
                }
            }
            if h > inp.snow_line {
                col = mix(
                    col,
                    c3(252, 252, 255),
                    smoothstep(inp.snow_line, inp.snow_line + 30.0, h) * 0.85,
                );
            }
            let f = c.forest_at(p);
            col = mix(col, c3(104, 160, 92), smoothstep(0.25, 0.65, f) * 0.42);
            col = mul(col, mix([1.0; 3], [r.min(1.0); 3], 0.62));
            col = mix(col, [1.0; 3], ((r - 1.0).max(0.0) * 0.35).min(0.25));
            col = zone_band(c, g, col);
            if wd > -0.2 {
                col = mix(col, c3(160, 206, 232), smoothstep(-0.05, 0.12, wd));
            }
            col
        }
        MapStyle::Minimal => {
            let shade = 1.0 + (r - 1.0).clamp(-0.6, 0.5) * 0.16;
            if alt <= 0.0 {
                let shallow = (1.0 - smoothstep(4.0 * u, 16.0 * u, c.coast_d(p)))
                    * (1.0 - smoothstep(4.0, 14.0, -alt));
                return scale(
                    mix(c3(64, 104, 134), c3(100, 152, 176), shallow),
                    1.0 + (r - 1.0).clamp(-0.5, 0.5) * 0.05,
                );
            }
            let mut col = match BiomePreset::from_id(c.biome_at(p)) {
                Some(BiomePreset::Desert) => c3(222, 198, 144),
                Some(BiomePreset::Swamp) => c3(124, 138, 96),
                Some(BiomePreset::Volcanic) => c3(108, 94, 90),
                Some(BiomePreset::Tundra) => c3(196, 204, 192),
                Some(BiomePreset::Alpine) => c3(160, 174, 132),
                Some(BiomePreset::AutumnWoods) => c3(186, 166, 108),
                Some(BiomePreset::Jungle | BiomePreset::TropicalCoast) => c3(124, 168, 98),
                Some(BiomePreset::HauntedWoods) => c3(122, 122, 106),
                Some(BiomePreset::Taiga) => c3(132, 160, 112),
                _ => c3(156, 184, 116),
            };
            let f = c.forest_at(p);
            col = mix(col, mul(col, c3(196, 212, 186)), smoothstep(0.4, 0.46, f));
            col = mix(col, c3(150, 144, 132), smoothstep(0.5, 0.56, sl));
            col = mix(col, c3(232, 220, 182), 1.0 - smoothstep(1.4, 2.2, alt));
            if h > inp.snow_line {
                col = mix(
                    col,
                    c3(238, 242, 244),
                    smoothstep(inp.snow_line, inp.snow_line + 6.0, h),
                );
            }
            col = scale(col, shade);
            col = zone_band(c, g, col);
            if wd > -0.2 {
                col = mix(col, c3(78, 128, 158), smoothstep(-0.05, 0.05, wd));
            }
            col
        }
        MapStyle::Satellite => [0.0; 3],
    }
}

/// A watercolour band of each zone's colour along the inside of its border.
fn zone_band(c: &Ctx, g: Vec2, col: C3) -> C3 {
    if !c.settings.layers.zones || c.kit.band <= 0.0 || c.inp.zones.is_empty() {
        return col;
    }
    let d = c.zone_d.bilinear(g.x, g.y);
    let band = 16.0 * c.w as f32 / 2048.0;
    if d > band {
        return col;
    }
    let n = c.inp.n();
    let (i0, j0) = (g.x.floor().max(0.0) as usize, g.y.floor().max(0.0) as usize);
    let (fx, fy) = (g.x - i0 as f32, g.y - j0 as f32);
    let mut acc = [0.0f32; 3];
    let mut wsum = 0.0;
    for (di, dj, wgt) in [
        (0, 0, (1.0 - fx) * (1.0 - fy)),
        (1, 0, fx * (1.0 - fy)),
        (0, 1, (1.0 - fx) * fy),
        (1, 1, fx * fy),
    ] {
        let (i, j) = ((i0 + di).min(n - 1), (j0 + dj).min(n - 1));
        let id = c.inp.zone_ids[j * n + i];
        if let Some(z) = c.inp.zones.iter().find(|z| z.id == id) {
            // soften towards the paper: hand-tinted, not neon
            let zc = mix(z.color, [0.6, 0.55, 0.5], 0.25);
            acc = [
                acc[0] + zc[0] * wgt,
                acc[1] + zc[1] * wgt,
                acc[2] + zc[2] * wgt,
            ];
            wsum += wgt;
        }
    }
    if wsum <= 0.0 {
        return col;
    }
    let a = c.kit.band * (1.0 - d / band).powi(2) * wsum;
    let zc = scale(acc, 1.0 / wsum);
    if c.kit.style == MapStyle::Parchment {
        mix(col, mul(col, mix([1.0; 3], zc, 0.9)), a * 2.2)
    } else {
        mix(col, zc, a)
    }
}

fn to_u8(c: C3) -> [u8; 3] {
    [
        (c[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

/// Generates the map. `satellite` is the captured top-down render (RGBA8, `size`²) for that style.
pub fn render(
    inp: &MapInput,
    settings: &MapSettings,
    satellite: Option<&[u8]>,
    progress: &Progress,
) -> Result<MapResult, String> {
    let t0 = Instant::now();
    let mut timings = Vec::new();
    let mut lap = |name: &'static str, t: &mut Instant| {
        timings.push((name, t.elapsed().as_millis() as u32));
        *t = Instant::now();
    };
    let mut t = Instant::now();
    let fonts = Fonts::get().ok_or("the map fonts (assets/fonts/map) are missing")?;
    let size = settings.size;
    progress.set(0.02, "Reading the terrain");
    let c = fields(inp, settings);
    if std::env::var("WB_MAP_STATS").is_ok() {
        let land =
            inp.heights.v.iter().filter(|h| **h > inp.sea).count() as f32 * inp.cell * inp.cell
                / 1e6;
        let wet = c.water.v.iter().filter(|d| **d > 0.0).count();
        let rivers: Vec<(usize, f32)> = inp.rivers.iter().map(|r| (r.pts.len(), r.width)).collect();
        let fmax = c.forest.v.iter().fold(0.0f32, |a, b| a.max(*b));
        let fover = c.forest.v.iter().filter(|v| **v > 0.36).count();
        bevy::log::info!(
            "MAPLOG stats: land {land:.2} km², {} trees, {} buildings, hmax {:.0} m, rivers {rivers:?}, wet cells {wet}, forest max {fmax:.2}, cells>0.36 {fover}/{}",
            inp.trees.len(),
            inp.buildings.len(),
            c.hmax,
            c.forest.v.len()
        );
    }
    lap("fields", &mut t);
    if progress.cancelled() {
        return Err("cancelled".into());
    }
    progress.set(0.12, "Painting the land");
    let mut cv = Canvas::new(size);
    {
        let w = size as usize;
        let data = cv.pm.data_mut();
        if let Some(sat) = satellite {
            if sat.len() != data.len() {
                return Err("satellite capture has the wrong size".into());
            }
            data.copy_from_slice(sat);
            data.par_chunks_mut(4).for_each(|px| px[3] = 255);
        } else {
            data.par_chunks_mut(w * 4).enumerate().for_each(|(j, row)| {
                for i in 0..w {
                    let col = to_u8(base_px(&c, i as f32 + 0.5, j as f32 + 0.5));
                    row[i * 4..i * 4 + 3].copy_from_slice(&col);
                    row[i * 4 + 3] = 255;
                }
            });
        }
    }
    lap("base", &mut t);
    if progress.cancelled() {
        return Err("cancelled".into());
    }
    progress.set(0.35, "Inking coasts and rivers");
    let drawn = features::water_and_relief(&mut cv, &c, progress);
    lap("water+relief", &mut t);
    progress.set(0.6, "Roads and borders");
    let zone_lines = features::zone_borders(&c);
    if settings.layers.roads {
        features::roads(&mut cv, &c);
    }
    features::buildings(&mut cv, &c);
    if settings.layers.zones {
        features::draw_zone_borders(&mut cv, &c, &zone_lines);
    }
    lap("roads+borders", &mut t);
    let clean = cv.pm.data().to_vec();
    if progress.cancelled() {
        return Err("cancelled".into());
    }
    progress.set(0.72, "Lettering");
    let deco = decor::plan(&c, fonts);
    let placed = labels::place_and_draw(&mut cv, &c, fonts, &deco, &drawn);
    lap("labels", &mut t);
    progress.set(0.85, "Compass and frame");
    decor::draw(&mut cv, &c, fonts, &deco);
    lap("decor", &mut t);
    progress.set(0.9, "Fog of war");
    let (fog, fog_size) = fog_mask(&c);
    let meta = labels::meta(&c, &placed, &zone_lines, &drawn, fog_size);
    lap("fog+meta", &mut t);
    // un-premultiply (the map is opaque, so this is a copy)
    let rgba = cv.pm.data().to_vec();
    progress.set(1.0, "Done");
    Ok(MapResult {
        settings: settings.clone(),
        size,
        rgba,
        clean,
        fog,
        fog_size,
        meta,
        pois: placed
            .icons
            .iter()
            .map(|i| super::minimap::Marker {
                world: i.world,
                kind: i.kind,
            })
            .collect(),
        world_min: Vec2::splat(-inp.half()),
        world_size: inp.size_m(),
        millis: t0.elapsed().as_millis() as u32,
        timings,
    })
}

/// Fog-of-war: everything hidden but a soft, cloudy-edged clearing around the player start (or the
/// first town, or the centre). The game reveals more as players explore (see map.json).
fn fog_mask(c: &Ctx) -> (Vec<u8>, u32) {
    let fs = (c.w / 2).max(256);
    let k = c.w as f32 / fs as f32;
    let centre = c
        .inp
        .start
        .or_else(|| {
            c.inp
                .pois
                .iter()
                .find(|p| p.kind == crate::gameplay::PoiKind::Town)
                .map(|p| p.pos)
        })
        .unwrap_or(Vec2::ZERO);
    let cp = c.world_to_px(centre);
    let radius = 320.0 * c.ppm();
    let mut out = vec![255u8; (fs * fs) as usize];
    out.par_chunks_mut(fs as usize)
        .enumerate()
        .for_each(|(j, row)| {
            for (i, o) in row.iter_mut().enumerate() {
                let p = Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * k;
                let d = p.distance(cp) / radius;
                let wob = crate::noise::fbm(
                    p.x / c.w as f32 * 18.0,
                    p.y / c.w as f32 * 18.0,
                    3,
                    2.0,
                    0.5,
                    77,
                ) * 0.22;
                let v = smoothstep(0.75, 1.05, d + wob);
                *o = (v * 255.0) as u8;
            }
        });
    (out, fs)
}
