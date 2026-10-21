//! Biome world generation: a world generated from a seed comes alive in one go. A climate
//! (temperature from a seeded "latitude" gradient, altitude and noise; moisture from noise and
//! nearness to the sea) is classified into biomes and cleaned into natural regions, written to
//! the terrain's biome map (which drives grass, ambience, weather and mood where no zone says
//! otherwise). The ground is painted to match, blended across borders; each biome's trees,
//! plants, rocks and debris grow procedurally from the biome map (`crate::nature`); lakes and
//! rivers take the biome's water. No zones are
//! made: zones are the world crafter's own (names, levels, rules).

use bevy::prelude::*;

use crate::biomes::{BiomePreset, ground_target, restyle_water};
use crate::history::History;
use crate::noise::{fbm, smoothstep};
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::terrain::{GridRect, Terrain};
use crate::water::WaterBodies;
use crate::world::Zones;

/// Overall climate of a generated world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Climate {
    /// Cold north to hot south: every biome somewhere.
    #[default]
    Varied,
    Temperate,
    Tropical,
    Arid,
    Cold,
}

impl Climate {
    pub const ALL: [Climate; 5] = [
        Climate::Varied,
        Climate::Temperate,
        Climate::Tropical,
        Climate::Arid,
        Climate::Cold,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Climate::Varied => "Varied",
            Climate::Temperate => "Temperate",
            Climate::Tropical => "Tropical",
            Climate::Arid => "Arid",
            Climate::Cold => "Cold",
        }
    }
    /// (mean °C, ± across the map, moisture bias)
    fn params(self) -> (f32, f32, f32) {
        match self {
            Climate::Varied => (14.0, 14.0, 0.0),
            Climate::Temperate => (13.0, 5.0, 0.05),
            Climate::Tropical => (26.0, 4.0, 0.12),
            Climate::Arid => (25.0, 6.0, -0.25),
            Climate::Cold => (1.0, 6.0, 0.0),
        }
    }
}

pub struct Generated {
    /// biome regions on the map
    pub regions: usize,
    /// a desert realm's dune sand: its depth (m) per terrain vertex (empty otherwise)
    pub sand: Vec<f32>,
}

/// Coarse climate grid resolution (cells per side).
const G: usize = 96;

/// Biome for one spot of the climate.
fn classify(
    temp: f32,
    moist: f32,
    alt: f32,
    coast: f32,
    snow_line: f32,
    autumn: f32,
) -> BiomePreset {
    use BiomePreset::*;
    if alt > snow_line * 0.85 {
        return Tundra;
    }
    if alt > snow_line * 0.5 && temp < 10.0 {
        return Alpine;
    }
    if temp < 0.0 {
        return Tundra;
    }
    if temp < 7.0 {
        return Taiga;
    }
    if temp > 22.0 {
        if moist < 0.4 {
            return Desert;
        }
        if coast < 140.0 && alt < 12.0 {
            return TropicalCoast;
        }
        return Jungle;
    }
    if alt < 6.0 && moist > 0.68 && coast < 500.0 {
        return Swamp;
    }
    if temp > 19.0 && moist < 0.32 {
        return Desert;
    }
    if moist < 0.42 {
        return Meadow;
    }
    if autumn > 0.6 && temp < 15.0 {
        return AutumnWoods;
    }
    TemperateForest
}

/// Two-pass chamfer distance (m) to the nearest seed cell.
fn distance_field(seed: &[bool], n: usize, cell: f32) -> Vec<f32> {
    let mut d: Vec<f32> = seed
        .iter()
        .map(|s| if *s { 0.0 } else { f32::MAX })
        .collect();
    let diag = cell * std::f32::consts::SQRT_2;
    for pass in 0..2 {
        let order: Vec<usize> = if pass == 0 {
            (0..n * n).collect()
        } else {
            (0..n * n).rev().collect()
        };
        for k in order {
            let (i, j) = ((k % n) as isize, (k / n) as isize);
            let dirs: [(isize, isize, f32); 4] = if pass == 0 {
                [(-1, 0, cell), (0, -1, cell), (-1, -1, diag), (1, -1, diag)]
            } else {
                [(1, 0, cell), (0, 1, cell), (1, 1, diag), (-1, 1, diag)]
            };
            for (di, dj, w) in dirs {
                let (a, b) = (i + di, j + dj);
                if a >= 0 && b >= 0 && a < n as isize && b < n as isize {
                    let v = d[b as usize * n + a as usize] + w;
                    if v < d[k] {
                        d[k] = v;
                    }
                }
            }
        }
    }
    d
}

/// Generates the biome map, ground, vegetation and water styles over all land.
#[allow(clippy::too_many_arguments)]
pub fn generate(
    t: &mut Terrain,
    zones: &mut Zones,
    history: &mut History,
    water: &mut WaterBodies,
    seed: u32,
    climate: Climate,
) -> Generated {
    generate_only(t, zones, history, water, seed, climate, None)
}

/// `generate`, with every bit of land given one biome (`only`: a dungeon's desert or frozen realm).
pub fn generate_only(
    t: &mut Terrain,
    zones: &mut Zones,
    history: &mut History,
    water: &mut WaterBodies,
    seed: u32,
    climate: Climate,
    only: Option<BiomePreset>,
) -> Generated {
    generate_realm(t, zones, history, water, seed, climate, only, 1.0)
}

/// `generate_only`; a desert's sand fills the land's basins by `fill` (1: half way to the land's
/// mean height, burying low hills; 0: the sand follows the ground, so a realm's canyon, walled
/// valley or plateau keeps its shape under its dunes).
#[allow(clippy::too_many_arguments)]
pub fn generate_realm(
    t: &mut Terrain,
    zones: &mut Zones,
    history: &mut History,
    water: &mut WaterBodies,
    seed: u32,
    climate: Climate,
    only: Option<BiomePreset>,
    fill: f32,
) -> Generated {
    let t0 = std::time::Instant::now();
    let n_all = t.n();
    let all = GridRect {
        x0: 0,
        z0: 0,
        x1: n_all - 1,
        z1: n_all - 1,
    };
    history.touch_zones(zones);
    history.touch_terrain(t, all);

    // zones made by an earlier version of the generator belong to the old landscape
    let old: Vec<u8> = zones
        .list
        .iter()
        .filter(|z| z.generated)
        .map(|z| z.id)
        .collect();
    if !old.is_empty() {
        for z in t.zones.iter_mut() {
            if old.contains(z) {
                *z = 0;
            }
        }
        zones.list.retain(|z| !z.generated);
    }

    // a desert realm is a sand sea: its land drowns in wind-shaped dunes
    let sand = if only == Some(BiomePreset::Desert) {
        dune_sea(t, seed, fill)
    } else {
        vec![]
    };

    // --- climate on a coarse grid
    let half = t.half();
    let cell = t.size() / G as f32;
    let centre = |i: usize, j: usize| {
        Vec2::new(
            -half + (i as f32 + 0.5) * cell,
            -half + (j as f32 + 0.5) * cell,
        )
    };
    let mut alt = vec![0.0f32; G * G];
    for j in 0..G {
        for i in 0..G {
            let p = centre(i, j);
            alt[j * G + i] = t.height_at(p.x, p.y) - t.sea_level;
        }
    }
    let sea: Vec<bool> = (0..G * G)
        .map(|k| alt[k] < -0.5 || k % G == 0 || k / G == 0 || k % G == G - 1 || k / G == G - 1)
        .collect();
    let coast = distance_field(&sea, G, cell);
    let mut rng = crate::noise::Rng::new(seed as u64 ^ 0xb10e_5eed);
    let ang = rng.f32() * std::f32::consts::TAU;
    let north = Vec2::new(ang.cos(), ang.sin());
    let (mean, span, wet_bias) = climate.params();
    let snow = t.style.snow_line;
    // the climate gradient spans the land itself (an island in a big sea still gets cold and
    // warm ends), not the whole map
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for j in 0..G {
        for i in 0..G {
            if alt[j * G + i] >= 0.3 {
                let d = centre(i, j).dot(north);
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
    }
    let (mid, extent) = if lo < hi {
        ((lo + hi) * 0.5, ((hi - lo) * 0.5).max(cell))
    } else {
        (0.0, half)
    };
    let mut temp = vec![0.0f32; G * G];
    let mut label: Vec<Option<BiomePreset>> = vec![None; G * G];
    for j in 0..G {
        for i in 0..G {
            let k = j * G + i;
            if alt[k] < 0.3 {
                continue;
            }
            let p = centre(i, j);
            let lat = ((p.dot(north) - mid) / extent).clamp(-1.0, 1.0);
            let tn = fbm(p.x * 0.0011, p.y * 0.0011, 3, 2.0, 0.5, seed + 811);
            temp[k] = mean - span * lat + 4.0 * tn - 0.035 * alt[k].max(0.0);
            // fbm clusters around 0.5: stretch it so worlds get genuinely dry and wet regions
            let mn = ((fbm(p.x * 0.0009, p.y * 0.0009, 4, 2.0, 0.5, seed + 823) * 0.5) * 2.4 + 0.5)
                .clamp(0.0, 1.0);
            let moist =
                (0.08 + mn * 0.75 + (-coast[k] / 400.0).exp() * 0.3 + wet_bias).clamp(0.0, 1.0);
            let autumn = fbm(p.x * 0.0016, p.y * 0.0016, 2, 2.0, 0.5, seed + 839) * 0.5 + 0.5;
            label[k] = Some(
                only.unwrap_or_else(|| classify(temp[k], moist, alt[k], coast[k], snow, autumn)),
            );
        }
    }

    // --- clean up: majority filter, then merge regions too small to be a zone
    for _ in 0..2 {
        let prev = label.clone();
        for j in 0..G {
            for i in 0..G {
                let k = j * G + i;
                if prev[k].is_none() {
                    continue;
                }
                let mut counts: Vec<(BiomePreset, u32)> = vec![];
                for dj in -1i32..=1 {
                    for di in -1i32..=1 {
                        let (a, b) = (i as i32 + di, j as i32 + dj);
                        if a < 0 || b < 0 || a >= G as i32 || b >= G as i32 {
                            continue;
                        }
                        if let Some(l) = prev[b as usize * G + a as usize] {
                            match counts.iter_mut().find(|c| c.0 == l) {
                                Some(c) => c.1 += 1,
                                None => counts.push((l, 1)),
                            }
                        }
                    }
                }
                if let Some(best) = counts.iter().max_by_key(|c| c.1)
                    && best.1 >= 5
                {
                    label[k] = Some(best.0);
                }
            }
        }
    }
    let min_cells = ((0.12e6 / (cell * cell)) as usize).max(6);
    let mut comp = vec![usize::MAX; G * G];
    let mut regions: Vec<(BiomePreset, Vec<usize>)>;
    let mut round = 0;
    loop {
        // connected regions of one biome
        comp.iter_mut().for_each(|c| *c = usize::MAX);
        regions = vec![];
        for start in 0..G * G {
            let Some(b) = label[start] else { continue };
            if comp[start] != usize::MAX {
                continue;
            }
            let id = regions.len();
            let mut cells = vec![start];
            comp[start] = id;
            let mut q = 0;
            while q < cells.len() {
                let k = cells[q];
                q += 1;
                let (i, j) = (k % G, k / G);
                let nb = [
                    (i.wrapping_sub(1), j),
                    (i + 1, j),
                    (i, j.wrapping_sub(1)),
                    (i, j + 1),
                ];
                for (a, c) in nb {
                    if a < G && c < G {
                        let kk = c * G + a;
                        if comp[kk] == usize::MAX && label[kk] == Some(b) {
                            comp[kk] = id;
                            cells.push(kk);
                        }
                    }
                }
            }
            regions.push((b, cells));
        }
        // regions below the size limit join their commonest neighbour
        let mut order: Vec<usize> = (0..regions.len()).collect();
        order.sort_by_key(|r| regions[*r].1.len());
        let mut changed = false;
        for &r in &order {
            if regions[r].1.len() >= min_cells {
                break;
            }
            let mut counts: Vec<(BiomePreset, u32)> = vec![];
            for &k in &regions[r].1 {
                let (i, j) = (k % G, k / G);
                for (a, c) in [
                    (i.wrapping_sub(1), j),
                    (i + 1, j),
                    (i, j.wrapping_sub(1)),
                    (i, j + 1),
                ] {
                    if a < G && c < G {
                        let kk = c * G + a;
                        if comp[kk] != r
                            && let Some(l) = label[kk]
                        {
                            match counts.iter_mut().find(|x| x.0 == l) {
                                Some(x) => x.1 += 1,
                                None => counts.push((l, 1)),
                            }
                        }
                    }
                }
            }
            // (an island of its own keeps its biome)
            if let Some(best) = counts.iter().max_by_key(|c| c.1) {
                for &k in &regions[r].1 {
                    label[k] = Some(best.0);
                }
                changed = true;
            }
        }
        round += 1;
        if !changed || round > 8 {
            break;
        }
    }
    let cell_biome: Vec<Option<BiomePreset>> =
        comp.iter().map(|c| regions.get(*c).map(|r| r.0)).collect();
    // land in a sea cell (the coast's last strip) has no biome and takes the automatic ground: a
    // green meadow fringe. Fine beside woods and meadows, wrong beside a desert or volcanic
    // wastes: there it takes the nearest land cell's biome
    let coast_biome: Vec<Option<BiomePreset>> = {
        let mut near = cell_biome.clone();
        let mut queue: std::collections::VecDeque<usize> =
            (0..G * G).filter(|k| near[*k].is_some()).collect();
        while let Some(k) = queue.pop_front() {
            let (i, j) = (k % G, k / G);
            for (a, c) in [
                (i.wrapping_sub(1), j),
                (i + 1, j),
                (i, j.wrapping_sub(1)),
                (i, j + 1),
            ] {
                if a < G && c < G && near[c * G + a].is_none() {
                    near[c * G + a] = near[k];
                    queue.push_back(c * G + a);
                }
            }
        }
        near.iter()
            .zip(&cell_biome)
            .map(|(n, c)| c.or(n.filter(|b| b.def().grass.density <= 0.0)))
            .collect()
    };

    // --- biome map and ground at full resolution: borders wander (domain warp) and the ground
    // blends across them over about one climate cell
    let warp = cell * 1.3;
    let sample = |p: Vec2| -> usize {
        let g = ((p + Vec2::splat(half)) / cell).floor();
        let (i, j) = (
            (g.x.max(0.0) as usize).min(G - 1),
            (g.y.max(0.0) as usize).min(G - 1),
        );
        j * G + i
    };
    let rows: Vec<(usize, Vec<(u8, Option<[u8; LAYER_COUNT]>)>)> = {
        let tr = &*t;
        let zs: Vec<usize> = (0..n_all).collect();
        std::thread::scope(|s| {
            let chunk = zs.len().div_ceil(10).max(1);
            let hs: Vec<_> = zs
                .chunks(chunk)
                .map(|js| {
                    let (cell_biome, coast_biome) = (&cell_biome, &coast_biome);
                    s.spawn(move || {
                        js.iter()
                            .map(|&j| {
                                let row = (0..n_all)
                                    .map(|i| {
                                        let p = tr.grid_to_world(i, j);
                                        let w = Vec2::new(
                                            fbm(p.x * 0.004, p.y * 0.004, 3, 2.0, 0.5, seed + 851),
                                            fbm(p.x * 0.004, p.y * 0.004, 3, 2.0, 0.5, seed + 857),
                                        ) * warp;
                                        let c = sample(p + w);
                                        let cell_biome = if tr.h(i, j) > tr.sea_level {
                                            coast_biome
                                        } else {
                                            cell_biome
                                        };
                                        let Some(here) = cell_biome[c] else {
                                            return (0, None);
                                        };
                                        // ground: the biomes of the four surrounding cells, bilinear
                                        let g =
                                            (p + w + Vec2::splat(half)) / cell - Vec2::splat(0.5);
                                        let f = g - g.floor();
                                        let (i0, j0) = (g.x.floor() as isize, g.y.floor() as isize);
                                        let mut acc = [0.0f32; LAYER_COUNT];
                                        let mut wsum = 0.0;
                                        for (di, dj, wt) in [
                                            (0, 0, (1.0 - f.x) * (1.0 - f.y)),
                                            (1, 0, f.x * (1.0 - f.y)),
                                            (0, 1, (1.0 - f.x) * f.y),
                                            (1, 1, f.x * f.y),
                                        ] {
                                            let (a, b) = (
                                                (i0 + di).clamp(0, G as isize - 1) as usize,
                                                (j0 + dj).clamp(0, G as isize - 1) as usize,
                                            );
                                            let Some(bp) = cell_biome[b * G + a].or(cell_biome[c])
                                            else {
                                                continue;
                                            };
                                            let tg = ground_target(tr, &bp.def(), p, seed);
                                            for l in 0..LAYER_COUNT {
                                                acc[l] += tg[l] * wt;
                                            }
                                            wsum += wt;
                                        }
                                        let splat = (wsum > 0.0).then(|| {
                                            let mut out = [0u8; LAYER_COUNT];
                                            for l in 0..LAYER_COUNT {
                                                out[l] =
                                                    (acc[l] / wsum).round().clamp(0.0, 255.0) as u8;
                                            }
                                            out
                                        });
                                        (here.id(), splat)
                                    })
                                    .collect::<Vec<_>>();
                                (j, row)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        })
    };
    for (j, row) in rows {
        for (i, (biome, splat)) in row.into_iter().enumerate() {
            let k = t.idx(i, j);
            t.biomes[k] = biome;
            if let Some(s) = splat {
                t.splat[k] = s;
            }
        }
    }
    // dune sand is all sand, slip faces too (sand rests at up to ~34°); the old ground standing
    // out of it is wind-swept gravel and rock, not more sand
    for (k, d) in sand.iter().enumerate() {
        let a = smoothstep(0.2, 1.5, *d);
        if a < 1.0 && t.heights[k] > t.sea_level + 5.0 {
            let s = &mut t.splat[k];
            let moved = s[Layer::Sand as usize] as f32 * 0.75 * (1.0 - a);
            s[Layer::Sand as usize] -= moved.round() as u8;
            s[Layer::Gravel as usize] =
                s[Layer::Gravel as usize].saturating_add(moved.round() as u8);
        }
        if a > 0.0 {
            let s = &mut t.splat[k];
            for (l, v) in s.iter_mut().enumerate() {
                let target = if l == Layer::Sand as usize {
                    255.0
                } else {
                    0.0
                };
                *v = (*v as f32 + (target - *v as f32) * a).round() as u8;
            }
        }
    }
    t.mark_zone_dirty(all);
    t.mark_splat_dirty(all);
    t.mark_biome_dirty(all);

    // vegetation needs nothing here: procedural nature grows by the biome map (`crate::nature`)
    let mut present: Vec<BiomePreset> = regions.iter().map(|r| r.0).collect();
    present.sort_by_key(|b| b.id());
    present.dedup();

    // --- lakes and rivers take their biome's water
    for bp in &present {
        restyle_water(t, water, history, &bp.def(), bp.id());
    }
    let (tmin, tmax) = temp
        .iter()
        .zip(&alt)
        .filter(|(_, a)| **a >= 0.3)
        .fold((f32::MAX, f32::MIN), |(lo, hi), (t, _)| {
            (lo.min(*t), hi.max(*t))
        });
    let names: Vec<&str> = present.iter().map(|b| b.short()).collect();
    info!(
        "biome generation: {} regions of {} biomes ({}), land {tmin:.0}..{tmax:.0} °C in {:?}",
        regions.len(),
        present.len(),
        names.join(", "),
        t0.elapsed()
    );
    Generated {
        regions: regions.len(),
        sand,
    }
}

/// A desert realm's sand sea (erg): wind-blown sand fills the land up to a broad, flattened level
/// and is heaped into transverse dunes, all facing one wind (a long gentle windward slope, a
/// steep slip face downwind, sinuous crests breaking into crescents, smaller cross-dunes on
/// them). Old ground higher than the sand stands out of it as rocky outcrops. The coast keeps
/// its beaches. Returns the sand's depth per vertex.
fn dune_sea(t: &mut Terrain, seed: u32, fill: f32) -> Vec<f32> {
    let n = t.n();
    let sea = t.sea_level;
    let old = t.heights.clone();
    // (a small realm's dunes are smaller and closer: a cozy desert still has rows of them)
    let k = (t.size() / 2048.0).clamp(0.4, 1.0);
    // the land's broad shape: heights blurred over ~150 m (the sea counts as sea level)
    let mut broad: Vec<f32> = old.iter().map(|h| h.max(sea)).collect();
    let r = (36.0 * k / t.cell).round().max(2.0) as usize;
    for _ in 0..3 {
        box_blur(&mut broad, n, r);
    }
    let land: Vec<f32> = broad.iter().copied().filter(|b| *b > sea + 4.0).collect();
    let mean = if land.is_empty() {
        sea + 20.0
    } else {
        land.iter().sum::<f32>() / land.len() as f32
    };
    let mut rng = crate::noise::Rng::new(seed as u64 ^ 0xd0e5_5a1d);
    let heading = rng.f32() * std::f32::consts::TAU;
    let dir = Vec2::new(heading.cos(), heading.sin());
    let perp = dir.perp();
    let dir2 = Vec2::from_angle(heading + 0.9);
    let k_size = k;
    // wavelength and height of the main dunes (m)
    let (l, height) = (190.0 * k, 17.0 * k.powf(0.8));
    let dune = |u: f32| -> f32 {
        let f = u.rem_euclid(1.0);
        if f < 0.72 {
            smoothstep(0.0, 0.72, f).powf(1.2)
        } else {
            1.0 - smoothstep(0.72, 1.0, f)
        }
    };
    let mut depth = vec![0.0f32; n * n];
    for j in 0..n {
        for i in 0..n {
            let k = t.idx(i, j);
            let h = old[k];
            if h < sea + 0.5 {
                continue;
            }
            let p = t.grid_to_world(i, j);
            // dunes calm down towards the coast (beaches stay)
            let shore = smoothstep(2.0, 16.0, broad[k] - sea) * smoothstep(0.5, 6.0, h - sea);
            if shore <= 0.0 {
                continue;
            }
            // the sand's level: the broad shape with half its relief (sand fills the basins and
            // buries the lower hills; mountains stand out of it). Filling less, it lies on the
            // ground itself (a canyon's floor, not half way up its walls)
            let fill = fill.clamp(0.0, 1.0);
            let base = broad[k] * fill + broad[k].min(h) * (1.0 - fill);
            let level = base
                + (mean - base) * 0.5 * fill
                + 6.0 * k_size
                + fbm(
                    p.x / (700.0 * k_size),
                    p.y / (700.0 * k_size),
                    3,
                    2.0,
                    0.5,
                    seed + 861,
                ) * 6.0
                    * k_size;
            let warp = fbm(p.x / (l * 2.6), p.y / (l * 2.6), 3, 2.0, 0.5, seed + 862) * 0.55
                + fbm(p.x / (l * 0.8), p.y / (l * 0.8), 2, 2.0, 0.5, seed + 863) * 0.12;
            let u = p.dot(dir) / l + warp + 0.18 * (p.dot(perp) / (l * 1.9)).sin();
            // crests rise and fall along their length (crescents), and from field to field
            let along = fbm(
                p.dot(perp) / (l * 1.1),
                p.dot(dir) / (l * 2.5),
                2,
                2.0,
                0.5,
                seed + 864,
            );
            let amp = height
                * (0.6 + 0.4 * fill)
                * (0.6 + 0.4 * fbm(p.x / (l * 5.0), p.y / (l * 5.0), 3, 2.0, 0.5, seed + 865))
                * (0.7 + 0.45 * along).max(0.25);
            let u2 = p.dot(dir2) / (l * 0.42) + warp * 1.3;
            let surface = level
                + amp * dune(u)
                + amp * 0.2 * dune(u2)
                + fbm(p.x / 11.0, p.y / 11.0, 2, 2.0, 0.5, seed + 866) * 0.12;
            // (old ground higher than the sand stands out of it as rocky outcrops)
            let top = h.max(surface);
            t.heights[k] = h + (top - h) * shore;
            depth[k] = ((surface - h) * shore).max(0.0);
        }
    }
    t.mark_heights_dirty(GridRect {
        x0: 0,
        z0: 0,
        x1: n - 1,
        z1: n - 1,
    });
    depth
}

/// Where a desert realm's oasis lies: the deepest level hollow between the dunes some way from
/// the middle (where players arrive), as a rough outline for a drawn oasis lake. None if the
/// sand sea has no such hollow.
pub fn oasis_outline(t: &Terrain, seed: u32) -> Option<Vec<Vec2>> {
    let tau = std::f32::consts::TAU;
    // (dunes are smaller in a small realm: so are the hollows between them, `dune_sea`)
    let ks = (t.size() / 2048.0).clamp(0.4, 1.0);
    let reach = 650.0f32.min(t.half() - 120.0 * ks);
    let mut best: Option<(f32, Vec2)> = None;
    let step = 16.0 * ks;
    let steps = (reach / step) as i32;
    // (a small or narrow sand sea may have no such hollow: then a shallower, less level one, else
    // the lowest level trough between its dunes)
    for pass in 0..3 {
        let (near, above, level, tol, deep) = [
            (140.0, 6.0, 0.985, 2.0, 1.5),
            (70.0, 3.0, 0.97, 3.0, 0.5),
            (70.0, 2.5, 0.97, 3.0, f32::MIN),
        ][pass];
        for gz in -steps..=steps {
            for gx in -steps..=steps {
                let p = Vec2::new(gx as f32, gz as f32) * step;
                if p.length() < near * ks || p.length() > reach {
                    continue;
                }
                let h = t.height_at(p.x, p.y);
                if h < t.sea_level + above || t.normal_at(p.x, p.y).y < level {
                    continue;
                }
                let ring =
                    |r: f32| (0..12).map(move |k| p + Vec2::from_angle(k as f32 / 12.0 * tau) * r);
                // a level floor ~45 m across, closed in all round by higher sand
                if ring(20.0 * ks).any(|q| (t.height_at(q.x, q.y) - h).abs() > tol) {
                    continue;
                }
                let rim = ring(55.0 * ks)
                    .map(|q| t.height_at(q.x, q.y))
                    .fold(f32::MAX, f32::min);
                let score = if pass < 2 { rim - h } else { -h };
                if (pass == 2 || score > deep * ks) && best.is_none_or(|b| score > b.0) {
                    best = Some((score, p));
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    let (_, c) = best?;
    let phase = crate::noise::Rng::new(seed as u64 ^ 0x0a51_5000).f32() * tau;
    Some(
        (0..32)
            .map(|k| {
                let a = k as f32 / 32.0 * tau;
                c + Vec2::from_angle(a)
                    * 19.0
                    * ks.max(0.6)
                    * (1.0 + 0.16 * (a * 2.0 + phase).sin() + 0.07 * (a * 3.0 + phase * 1.7).cos())
            })
            .collect(),
    )
}

/// Separable box blur of a square grid (`n` per side) with radius `r` cells.
fn box_blur(v: &mut [f32], n: usize, r: usize) {
    let mut tmp = vec![0.0f32; v.len()];
    for pass in 0..2 {
        let (src, dst): (&[f32], &mut [f32]) = if pass == 0 {
            (&*v, &mut tmp)
        } else {
            (&tmp, &mut *v)
        };
        for a in 0..n {
            let at = |b: usize| if pass == 0 { a * n + b } else { b * n + a };
            let mut sum = 0.0;
            let mut cnt = 0.0;
            for b in 0..=r.min(n - 1) {
                sum += src[at(b)];
                cnt += 1.0;
            }
            for b in 0..n {
                dst[at(b)] = sum / cnt;
                if b + r + 1 < n {
                    sum += src[at(b + r + 1)];
                    cnt += 1.0;
                }
                if b >= r {
                    sum -= src[at(b - r)];
                    cnt -= 1.0;
                }
            }
        }
    }
}

/// On a desert realm's dune sand little grows: no trees, the odd rock and shrub (the nature
/// rules' multipliers over the sand, by its depth).
pub fn thin_nature_on_sand(nature: &mut crate::nature::NatureField, t: &Terrain, sand: &[f32]) {
    if sand.len() != t.n() * t.n() {
        return;
    }
    let half = t.half();
    nature.set_texels(Vec2::splat(-half), Vec2::splat(half), |p| {
        let g = t.world_to_grid(p.x, p.y);
        let (i, j) = (
            (g.x.round().max(0.0) as usize).min(t.res),
            (g.y.round().max(0.0) as usize).min(t.res),
        );
        let d = sand[t.idx(i, j)];
        if d < 0.5 {
            return None;
        }
        let k = smoothstep(0.5, 4.0, d);
        // trees, rocks, plants, debris (`nature::rules::CATS`)
        Some(([1.0 - k, 1.0 - k * 0.92, 1.0 - k * 0.7, 1.0 - k * 0.85], 0))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::realm_land::{Detail, RealmShape, RealmSize, generate};

    fn terrain(shape: RealmShape, size: RealmSize, seed: u32) -> (Terrain, Vec2) {
        let land = generate(shape, size, seed, Detail::Full);
        let mut t = Terrain::new(land.res, land.cell, 0.0);
        t.heights = land.heights;
        (t, land.arrival)
    }

    #[test]
    fn a_desert_realm_has_dunes_and_an_oasis_at_every_size() {
        for (shape, size, seed) in [
            (RealmShape::CoastalShelf, RealmSize::Cozy, 22u32),
            (RealmShape::LongIsle, RealmSize::Cozy, 9),
            (RealmShape::CoastalShelf, RealmSize::Medium, 5),
            (RealmShape::LongIsle, RealmSize::Large, 23),
        ] {
            let (mut t, _) = terrain(shape, size, seed);
            let land = t.heights.iter().filter(|h| **h > 0.5).count() as f32;
            let sand = dune_sea(&mut t, seed, 1.0);
            let covered = sand.iter().filter(|d| **d > 0.5).count() as f32 / land;
            let oasis = oasis_outline(&t, seed);
            println!(
                "{} {}: sand on {:.0}% of the land, oasis {:?}",
                shape.id(),
                size.id(),
                covered * 100.0,
                oasis.as_ref().map(|o| o[0])
            );
            assert!(
                covered > 0.3,
                "{} {}: sand on only {:.0}% of the land",
                shape.id(),
                size.id(),
                covered * 100.0
            );
            assert!(oasis.is_some(), "{} {}: no oasis", shape.id(), size.id());
        }
    }

    #[test]
    fn a_desert_canyon_keeps_its_shape_under_its_dunes() {
        for (size, seed) in [(RealmSize::Medium, 17u32), (RealmSize::Cozy, 4)] {
            let (mut t, at) = terrain(RealmShape::Canyon, size, seed);
            let floor = t.height_at(at.x, at.y);
            let before = t.heights.clone();
            let mut tops: Vec<f32> = before
                .iter()
                .copied()
                .filter(|h| *h > floor + 20.0)
                .collect();
            tops.sort_by(|a, b| a.total_cmp(b));
            let mesa = tops[tops.len() / 2];
            dune_sea(&mut t, seed, 0.0);
            // how much sand lies on the canyon's floor
            let mut rise: Vec<f32> = before
                .iter()
                .zip(&t.heights)
                .filter(|(b, _)| (**b - floor).abs() < 4.0)
                .map(|(b, a)| a - b)
                .collect();
            rise.sort_by(|a, b| a.total_cmp(b));
            let (median, high) = (rise[rise.len() / 2], rise[rise.len() * 9 / 10]);
            println!(
                "canyon {}: floor {floor:.0} m, mesas {mesa:.0} m; sand on the floor {median:.1} m (90%: {high:.1} m)",
                size.id()
            );
            assert!(
                high < (mesa - floor) * 0.45,
                "the canyon filled with sand: {high:.0} m of {:.0} m",
                mesa - floor
            );
        }
    }
}
