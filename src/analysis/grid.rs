//! The walkability grid: the world cut into ~2 m cells, each classified for someone on foot
//! (walkable, wading, steep, cliff, obstacle, swim) with the height they stand at, how fast they
//! move there, the road under them and the zone. Travel time, content gaps, level flow,
//! reachability, the world doctor and the navmesh all run on it.
//!
//! Object blockers come from `object_footprint`, an approximation per asset kind; real colliders
//! can replace that one function later.

use bevy::prelude::*;

use super::{AnalysisSettings, SteepRule};
use crate::objects::{Category, LibraryEntry, Placed, kind_category};
use crate::procgen::meshes::AssetKind;
use crate::roads::RoadKind;

/// What a walker finds in a cell.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Walk,
    /// Water shallower than the swim depth: walkable, slower.
    Shallow,
    /// Steeper than the walk limit: slide down only, climb slowly or blocked (`SteepRule`).
    Steep,
    Cliff,
    /// Tree trunks, rocks, buildings.
    Obstacle,
    Swim,
    /// Water farther than `OPEN_SEA_M` from any shore: nobody swims out there.
    OpenSea,
    Outside,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Walk => "walkable",
            Class::Shallow => "wading",
            Class::Steep => "steep",
            Class::Cliff => "cliff",
            Class::Obstacle => "blocked by an object",
            Class::Swim => "swimming",
            Class::OpenSea => "open water (too far from shore to swim)",
            Class::Outside => "outside the world",
        }
    }
    /// Ground players walk on (the "walkable area" every percentage refers to).
    pub fn land(self) -> bool {
        matches!(self, Class::Walk | Class::Shallow)
    }
}

/// The 16-neighbourhood: 4 straight, 4 diagonal and 8 knight moves. Knight moves make travel
/// fronts nearly round (an 8-neighbour grid draws octagons, up to 8% off).
pub const NEIGHBOURS: [(i32, i32); 16] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
    (2, 1),
    (2, -1),
    (-2, 1),
    (-2, -1),
    (1, 2),
    (1, -2),
    (-1, 2),
    (-1, -2),
];
const SQRT2: f32 = std::f32::consts::SQRT_2;
const SQRT5: f32 = 2.236_068;
/// Length of each neighbour move in cells.
pub const STEP_LEN: [f32; 16] = [
    1.0, 1.0, 1.0, 1.0, SQRT2, SQRT2, SQRT2, SQRT2, SQRT5, SQRT5, SQRT5, SQRT5, SQRT5, SQRT5,
    SQRT5, SQRT5,
];

/// A copy of the terrain heights, water levels and zone map that worker threads sample.
pub struct Field {
    pub res: usize,
    pub cell: f32,
    pub half: f32,
    pub heights: Vec<f32>,
    pub sea: f32,
    /// Lake / river surface per terrain vertex (NEG_INFINITY = dry); empty without inland water.
    pub water: Vec<f32>,
    pub zones: Vec<u8>,
}

impl Field {
    pub fn from_world(t: &crate::terrain::Terrain, w: &crate::water::WaterBodies) -> Self {
        let n = t.n();
        // the level map follows the terrain grid it was built for
        let water = if w.level_map.len() == n * n
            && w.grid_cache
                .is_some_and(|(h, c)| (h - t.half()).abs() < 1e-3 && (c - t.cell).abs() < 1e-4)
        {
            w.level_map.clone()
        } else {
            Vec::new()
        };
        Self {
            res: t.res,
            cell: t.cell,
            half: t.half(),
            heights: t.heights.clone(),
            sea: t.sea_level,
            water,
            zones: t.zones.clone(),
        }
    }
    pub fn size(&self) -> f32 {
        self.res as f32 * self.cell
    }
    fn grid(&self, x: f32, z: f32) -> Vec2 {
        Vec2::new((x + self.half) / self.cell, (z + self.half) / self.cell)
    }
    /// Bilinear ground height (clamped at the edge), like `Terrain::height_at`.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let g = self.grid(x, z);
        let max = self.res as f32;
        let (gx, gz) = (g.x.clamp(0.0, max), g.y.clamp(0.0, max));
        let i0 = (gx.floor() as usize).min(self.res - 1);
        let j0 = (gz.floor() as usize).min(self.res - 1);
        let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
        let n = self.res + 1;
        let h = |i: usize, j: usize| self.heights[j * n + i];
        let a = h(i0, j0) + (h(i0 + 1, j0) - h(i0, j0)) * fx;
        let b = h(i0, j0 + 1) + (h(i0 + 1, j0 + 1) - h(i0, j0 + 1)) * fx;
        a + (b - a) * fz
    }
    fn vertex(&self, x: f32, z: f32) -> usize {
        let g = self.grid(x, z);
        let i = (g.x.round().max(0.0) as usize).min(self.res);
        let j = (g.y.round().max(0.0) as usize).min(self.res);
        j * (self.res + 1) + i
    }
    /// Water surface over a point (sea, lake or river), NEG_INFINITY-free: at least the sea.
    pub fn water_surface(&self, x: f32, z: f32) -> f32 {
        let lake = if self.water.is_empty() {
            f32::NEG_INFINITY
        } else {
            self.water[self.vertex(x, z)]
        };
        lake.max(self.sea)
    }
    pub fn zone_at(&self, x: f32, z: f32) -> u8 {
        if x.abs() > self.half || z.abs() > self.half {
            return 0;
        }
        self.zones[self.vertex(x, z)]
    }
    /// Ground slope in degrees.
    pub fn slope_deg(&self, x: f32, z: f32, e: f32) -> f32 {
        let dx = (self.height_at(x + e, z) - self.height_at(x - e, z)) / (2.0 * e);
        let dz = (self.height_at(x, z + e) - self.height_at(x, z - e)) / (2.0 * e);
        (dx * dx + dz * dz).sqrt().atan().to_degrees()
    }
}

/// Where an object stops a walker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Footprint {
    /// Walked through or over (plants, pebbles, small props).
    None,
    /// Blocks a disc (trunks, boulders, barrels).
    Disc { center: Vec2, radius: f32 },
    /// Blocks a rotated rectangle (huts, tents, walls, logs); `yaw` turns local +x.
    Rect { center: Vec2, half: Vec2, yaw: f32 },
    /// A walkable deck at `height` (bridges, docks): walkable over water and gorges.
    Deck {
        center: Vec2,
        half: Vec2,
        yaw: f32,
        height: f32,
    },
}

impl Footprint {
    /// Bounding radius in metres (0 for `None`).
    pub fn reach(&self) -> f32 {
        match *self {
            Footprint::None => 0.0,
            Footprint::Disc { radius, .. } => radius,
            Footprint::Rect { half, .. } | Footprint::Deck { half, .. } => half.length(),
        }
    }
    pub fn center(&self) -> Option<Vec2> {
        match *self {
            Footprint::None => None,
            Footprint::Disc { center, .. }
            | Footprint::Rect { center, .. }
            | Footprint::Deck { center, .. } => Some(center),
        }
    }
    pub fn contains(&self, p: Vec2) -> bool {
        let local = |center: Vec2, yaw: f32| {
            let d = p - center;
            // rotation_y(yaw) maps local +x to (cos, −sin) in xz
            let (s, c) = yaw.sin_cos();
            Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c)
        };
        match *self {
            Footprint::None => false,
            Footprint::Disc { center, radius } => p.distance_squared(center) <= radius * radius,
            Footprint::Rect { center, half, yaw }
            | Footprint::Deck {
                center, half, yaw, ..
            } => {
                let l = local(center, yaw);
                l.x.abs() <= half.x && l.y.abs() <= half.y
            }
        }
    }
}

/// The asset kind behind a placed object (also for kinds the library hides, e.g. bridges).
pub fn asset_kind(p: &Placed, entry: Option<&LibraryEntry>) -> Option<AssetKind> {
    static IDS: std::sync::OnceLock<std::collections::HashMap<String, AssetKind>> =
        std::sync::OnceLock::new();
    entry.and_then(|e| e.kind).or_else(|| {
        IDS.get_or_init(|| {
            AssetKind::ALL
                .iter()
                .map(|k| (crate::objects::builtin_id(*k), *k))
                .collect()
        })
        .get(&p.asset)
        .copied()
    })
}

/// Approximate collision footprint of a placed object for the walkability grid. This is the one
/// place object blocking is decided, so real colliders can replace it later.
pub fn object_footprint(p: &Placed, entry: Option<&LibraryEntry>) -> Footprint {
    use AssetKind::*;
    let s = p.scale.max(0.01);
    let center = Vec2::new(p.pos[0], p.pos[2]);
    let yaw = p.yaw();
    let kind = asset_kind(p, entry);
    let Some(k) = kind else {
        // imported models: decks if marked walkable, otherwise a disc inside their spacing radius
        return match entry {
            Some(e) if e.category == Category::Imported && e.walkable => Footprint::Deck {
                center,
                half: Vec2::splat(e.radius * s * 0.8),
                yaw,
                height: p.pos[1] + 0.2 * s,
            },
            Some(e) if e.category == Category::Imported => Footprint::Disc {
                center,
                radius: e.radius * s * 0.7,
            },
            _ => Footprint::None,
        };
    };
    let r = entry.map(|e| e.radius).unwrap_or(4.0) * s;
    let disc = |radius: f32| Footprint::Disc { center, radius };
    let rect = |hx: f32, hz: f32| Footprint::Rect {
        center,
        half: Vec2::new(hx, hz),
        yaw,
    };
    match k {
        WoodenBridge | StoneBridge => Footprint::Deck {
            center,
            half: Vec2::new(r * 0.95, 2.4 * s),
            yaw,
            height: p.pos[1] + 0.3 * s,
        },
        Dock => Footprint::Deck {
            center,
            half: Vec2::new(r * 0.9, 2.0 * s),
            yaw,
            height: p.pos[1] + 0.3 * s,
        },
        Pebbles | Branches | Driftwood | Roots | Signpost | Banner | TarpShelter | LilyPads
        | DesertShrub => Footprint::None,
        Cactus => disc(0.3 * s),
        Stump => disc(0.4 * r),
        FallenLog => rect(0.85 * r, 0.35 * s),
        Boulder | MossyRock => disc(0.75 * r),
        RockCluster => disc(0.7 * r),
        CliffRock => disc(0.8 * r),
        RockSpire => disc(0.6 * r),
        Hut | Watchtower => rect(0.8 * r, 0.8 * r),
        Tent => rect(0.7 * r, 0.6 * r),
        Fence => rect(r, 0.15 * s),
        RuinWall => rect(r, 0.4 * s),
        RuinPillar | Well => disc(0.6 * r),
        Campfire => disc(0.5 * s),
        Crate | Barrel => disc(0.45 * s),
        _ => match kind_category(k) {
            // trunks: about a tenth of the canopy spacing
            Category::Trees => disc((0.1 * r).max(0.15)),
            _ => Footprint::None,
        },
    }
}

/// Swimmers stay within this distance of a shore (m); farther out is open sea. Keeps the ocean
/// around an island from dominating every search.
pub const OPEN_SEA_M: f32 = 300.0;

/// Cells per side at most (a 4 km world at 2 m; larger worlds use larger cells).
pub const MAX_SIDE: usize = 2048;

/// Turns swim cells far from any shore into open sea (two-pass chamfer distance transform).
fn open_sea(n: usize, cell: f32, class: &mut [Class], speed: &mut [f32]) {
    const FAR: u32 = u32::MAX / 2;
    // distance to the nearest non-water cell in units of cell / 2 (2 straight, 3 diagonal)
    let mut d: Vec<u32> = class
        .iter()
        .map(|c| if matches!(c, Class::Swim) { FAR } else { 0 })
        .collect();
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            if d[k] == 0 {
                continue;
            }
            let mut v = d[k];
            if i > 0 {
                v = v.min(d[k - 1] + 2);
            }
            if j > 0 {
                v = v.min(d[k - n] + 2);
                if i > 0 {
                    v = v.min(d[k - n - 1] + 3);
                }
                if i + 1 < n {
                    v = v.min(d[k - n + 1] + 3);
                }
            }
            d[k] = v;
        }
    }
    for j in (0..n).rev() {
        for i in (0..n).rev() {
            let k = j * n + i;
            if d[k] == 0 {
                continue;
            }
            let mut v = d[k];
            if i + 1 < n {
                v = v.min(d[k + 1] + 2);
            }
            if j + 1 < n {
                v = v.min(d[k + n] + 2);
                if i + 1 < n {
                    v = v.min(d[k + n + 1] + 3);
                }
                if i > 0 {
                    v = v.min(d[k + n - 1] + 3);
                }
            }
            d[k] = v;
        }
    }
    let limit = (OPEN_SEA_M / cell * 2.0) as u32;
    for k in 0..class.len() {
        if class[k] == Class::Swim && d[k] > limit {
            class[k] = Class::OpenSea;
            speed[k] = 0.0;
        }
    }
}

/// A road centreline sampled every couple of metres.
pub struct RoadLine {
    pub name: String,
    pub kind: RoadKind,
    pub width: f32,
    pub pts: Vec<Vec3>,
}

pub struct WalkGrid {
    /// Cells per side.
    pub n: usize,
    /// Metres per cell.
    pub cell: f32,
    /// World xz of the −x/−z corner.
    pub origin: Vec2,
    pub class: Vec<Class>,
    /// Height a walker stands at (ground, bridge deck or water surface).
    pub surface: Vec<f32>,
    /// Travel speed (m/s) before slope effects; 0 = impassable.
    pub speed: Vec<f32>,
    /// Ground slope in whole degrees.
    pub slope: Vec<u8>,
    /// 0 = no road, else `RoadKind::ALL` index + 1.
    pub road: Vec<u8>,
    pub zone: Vec<u8>,
    pub steep: SteepRule,
    /// tan of the cliff angle: steeper single steps are ledges nobody walks up.
    pub max_grade: f32,
}

impl WalkGrid {
    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        j * self.n + i
    }
    pub fn len(&self) -> usize {
        self.n * self.n
    }
    pub fn center(&self, k: usize) -> Vec2 {
        self.origin + Vec2::new((k % self.n) as f32 + 0.5, (k / self.n) as f32 + 0.5) * self.cell
    }
    pub fn world(&self, k: usize) -> Vec3 {
        let c = self.center(k);
        Vec3::new(c.x, self.surface[k], c.y)
    }
    pub fn cell_at(&self, p: Vec2) -> Option<usize> {
        let g = (p - self.origin) / self.cell;
        if g.x < 0.0 || g.y < 0.0 || g.x >= self.n as f32 || g.y >= self.n as f32 {
            return None;
        }
        Some(self.idx(g.x as usize, g.y as usize))
    }
    pub fn passable(&self, k: usize) -> bool {
        self.speed[k] > 0.0
    }
    pub fn cell_area_km2(&self) -> f64 {
        (self.cell as f64 * self.cell as f64) / 1.0e6
    }
    /// The passable cell nearest to `p` within `radius` metres (land preferred over water).
    pub fn nearest_passable(&self, p: Vec2, radius: f32) -> Option<usize> {
        let k0 = self.cell_at(p.clamp(
            self.origin,
            self.origin + Vec2::splat(self.n as f32 * self.cell - 0.01),
        ))?;
        if self.passable(k0) && self.class[k0] != Class::Swim {
            return Some(k0);
        }
        let r = (radius / self.cell).ceil() as i32;
        let (ci, cj) = ((k0 % self.n) as i32, (k0 / self.n) as i32);
        let mut best: Option<(f32, usize)> = None;
        for dj in -r..=r {
            for di in -r..=r {
                let (i, j) = (ci + di, cj + dj);
                if i < 0 || j < 0 || i >= self.n as i32 || j >= self.n as i32 {
                    continue;
                }
                let k = self.idx(i as usize, j as usize);
                if !self.passable(k) {
                    continue;
                }
                let d = self.center(k).distance(p)
                    + if self.class[k] == Class::Swim {
                        radius
                    } else {
                        0.0
                    };
                if d <= radius * 2.0 && best.is_none_or(|b| d < b.0) {
                    best = Some((d, k));
                }
            }
        }
        best.map(|b| b.1)
            .or_else(|| self.passable(k0).then_some(k0))
    }

    /// The move from cell `a` = (ai, aj) along neighbour `o`: the target cell, if it's inside the
    /// grid and the cells it cuts across are passable (no squeezing past corners).
    #[inline]
    pub fn step(&self, ai: usize, aj: usize, o: usize) -> Option<usize> {
        let (dx, dz) = NEIGHBOURS[o];
        let (bi, bj) = (ai as i32 + dx, aj as i32 + dz);
        let n = self.n as i32;
        if bi < 0 || bj < 0 || bi >= n || bj >= n {
            return None;
        }
        let b = bj as usize * self.n + bi as usize;
        if self.speed[b] <= 0.0 {
            return None;
        }
        if o >= 4 {
            let (sx, sz) = (dx.signum(), dz.signum());
            let (c1, c2) = if o < 8 {
                ((ai as i32 + sx, aj as i32), (ai as i32, aj as i32 + sz))
            } else if dx.abs() == 2 {
                (
                    (ai as i32 + sx, aj as i32),
                    (ai as i32 + sx, aj as i32 + sz),
                )
            } else {
                (
                    (ai as i32, aj as i32 + sz),
                    (ai as i32 + sx, aj as i32 + sz),
                )
            };
            if self.speed[c1.1 as usize * self.n + c1.0 as usize] <= 0.0
                || self.speed[c2.1 as usize * self.n + c2.0 as usize] <= 0.0
            {
                return None;
            }
        }
        Some(b)
    }

    /// Seconds to move from `a` to `b` (`len` cells apart), or infinity if the move isn't
    /// possible (climbing a slide-only slope, walking up a ledge).
    #[inline]
    pub fn edge_time(&self, a: usize, b: usize, len: f32) -> f32 {
        let (va, vb) = (self.speed[a], self.speed[b]);
        let d = len * self.cell;
        let grade = (self.surface[b] - self.surface[a]) / d;
        let mut slow = 1.0;
        if grade > 0.0 {
            if grade > self.max_grade {
                return f32::INFINITY;
            }
            // uphill is slower: ≈ e^(1.2·grade), about 30% slower on a 17° climb
            slow = 1.0 + 1.2 * grade + 0.72 * grade * grade;
            if grade > 0.05 && (self.class[a] == Class::Steep || self.class[b] == Class::Steep) {
                match self.steep {
                    SteepRule::SlideDown | SteepRule::Block => return f32::INFINITY,
                    SteepRule::Climb => slow *= 3.0,
                }
            }
        }
        d * 0.5 * (1.0 / va + 1.0 / vb) * slow
    }

    /// Cells per class.
    pub fn counts(&self) -> [usize; 8] {
        let mut c = [0usize; 8];
        for k in &self.class {
            c[*k as usize] += 1;
        }
        c
    }
}

/// Builds the grid from a terrain copy, object footprints and roads (rows in parallel).
pub fn build(
    field: &Field,
    foot: &[Footprint],
    roads: &[RoadLine],
    zones: &[super::job::ZoneInfo],
    st: &AnalysisSettings,
    progress: &std::sync::atomic::AtomicU32,
) -> WalkGrid {
    use std::sync::atomic::Ordering;
    let size = field.size();
    // big worlds get coarser cells: at most MAX_SIDE² cells
    let n = ((size / st.cell.max(0.5)).round() as usize).clamp(8, MAX_SIDE);
    let cell = size / n as f32;
    let origin = Vec2::splat(-field.half);
    let total = n * n;

    // object coverage (4×4 samples per cell) and decks
    let mut cover = vec![0u8; total];
    let mut deck = vec![f32::NAN; total];
    if st.objects_block {
        for f in foot {
            let (Some(c), r) = (f.center(), f.reach()) else {
                continue;
            };
            let i0 = (((c.x - r - origin.x) / cell).floor().max(0.0) as usize).min(n - 1);
            let i1 = (((c.x + r - origin.x) / cell).floor().max(0.0) as usize).min(n - 1);
            let j0 = (((c.y - r - origin.y) / cell).floor().max(0.0) as usize).min(n - 1);
            let j1 = (((c.y + r - origin.y) / cell).floor().max(0.0) as usize).min(n - 1);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let k = j * n + i;
                    let base = origin + Vec2::new(i as f32, j as f32) * cell;
                    if let Footprint::Deck { height, .. } = *f {
                        if f.contains(base + Vec2::splat(cell * 0.5)) {
                            deck[k] = if deck[k].is_nan() {
                                height
                            } else {
                                deck[k].max(height)
                            };
                        }
                        continue;
                    }
                    let mut hits = 0u8;
                    for sj in 0..4 {
                        for si in 0..4 {
                            if f.contains(
                                base + Vec2::new(si as f32 + 0.5, sj as f32 + 0.5) * (cell * 0.25),
                            ) {
                                hits += 1;
                            }
                        }
                    }
                    cover[k] = (cover[k] + hits).min(16);
                }
            }
        }
    }
    // roads: the fastest surface wins where roads cross
    let mut road = vec![0u8; total];
    for r in roads {
        let hw = r.width * 0.5 + cell * 0.35;
        let kind_i = RoadKind::ALL.iter().position(|k| *k == r.kind).unwrap_or(0) as u8 + 1;
        for w in r.pts.windows(2) {
            let (a, b) = (w[0].xz(), w[1].xz());
            let lo = a.min(b) - Vec2::splat(hw);
            let hi = a.max(b) + Vec2::splat(hw);
            let i0 = (((lo.x - origin.x) / cell).floor().max(0.0) as usize).min(n - 1);
            let i1 = (((hi.x - origin.x) / cell).floor().max(0.0) as usize).min(n - 1);
            let j0 = (((lo.y - origin.y) / cell).floor().max(0.0) as usize).min(n - 1);
            let j1 = (((hi.y - origin.y) / cell).floor().max(0.0) as usize).min(n - 1);
            let ab = b - a;
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let p = origin + Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * cell;
                    let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    if p.distance(a + ab * u) <= hw {
                        let k = j * n + i;
                        let cur = road[k];
                        if cur == 0
                            || RoadKind::ALL[kind_i as usize - 1].travel_factor()
                                > RoadKind::ALL[cur as usize - 1].travel_factor()
                        {
                            road[k] = kind_i;
                        }
                    }
                }
            }
        }
    }
    progress.store(300, Ordering::Relaxed);

    let mut mounts = [true; 256];
    for z in zones {
        mounts[z.id as usize] = z.allow_mounts;
    }
    let walk_tan = st.max_walk_slope;
    let cliff = st.cliff_slope.max(st.max_walk_slope + 1.0);
    let mut class = vec![Class::Outside; total];
    let mut surface = vec![0.0f32; total];
    let mut speed = vec![0.0f32; total];
    let mut slope = vec![0u8; total];
    let mut zone = vec![0u8; total];
    let threads = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(8)
        .clamp(1, 16);
    let rows_per = n.div_ceil(threads);
    let done = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|sc| {
        let mut rest = (
            &mut class[..],
            &mut surface[..],
            &mut speed[..],
            &mut slope[..],
            &mut zone[..],
        );
        let mut j0 = 0;
        while j0 < n {
            let rows = rows_per.min(n - j0);
            let (c, c2) = std::mem::take(&mut rest.0).split_at_mut(rows * n);
            let (s, s2) = std::mem::take(&mut rest.1).split_at_mut(rows * n);
            let (sp, sp2) = std::mem::take(&mut rest.2).split_at_mut(rows * n);
            let (sl, sl2) = std::mem::take(&mut rest.3).split_at_mut(rows * n);
            let (zo, zo2) = std::mem::take(&mut rest.4).split_at_mut(rows * n);
            rest = (c2, s2, sp2, sl2, zo2);
            let (cover, deck, road, mounts, done) = (&cover, &deck, &road, &mounts, &done);
            sc.spawn(move || {
                for r in 0..rows {
                    let j = j0 + r;
                    for i in 0..n {
                        let k = j * n + i;
                        let o = r * n + i;
                        let p = origin + Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * cell;
                        let g = field.height_at(p.x, p.y);
                        let ws = field.water_surface(p.x, p.y);
                        let deg = field.slope_deg(p.x, p.y, cell * 0.5);
                        let zid = field.zone_at(p.x, p.y);
                        sl[o] = deg.round().min(90.0) as u8;
                        zo[o] = zid;
                        let land_speed = if st.mounted && mounts[zid as usize] {
                            st.mount_speed
                        } else {
                            st.run_speed
                        };
                        let depth = ws - g;
                        let (cls, surf, mut v) = if !deck[k].is_nan() {
                            (Class::Walk, deck[k], land_speed)
                        } else if depth > st.swim_depth {
                            (
                                Class::Swim,
                                ws,
                                if st.allow_swim { st.swim_speed } else { 0.0 },
                            )
                        } else if deg > cliff {
                            (Class::Cliff, g, 0.0)
                        } else if deg > walk_tan {
                            (
                                Class::Steep,
                                g,
                                if st.steep == SteepRule::Block {
                                    0.0
                                } else {
                                    land_speed
                                },
                            )
                        } else if depth > 0.25 {
                            (Class::Shallow, g, st.run_speed * 0.6)
                        } else {
                            (Class::Walk, g, land_speed)
                        };
                        let mut cls = cls;
                        if cls != Class::Swim && deck[k].is_nan() {
                            let cv = cover[k];
                            if cv >= 8 {
                                cls = Class::Obstacle;
                                v = 0.0;
                            } else if cv > 0 {
                                // squeezing between trunks and rocks
                                v *= 1.0 - cv as f32 / 16.0 * 0.6;
                            }
                        }
                        if matches!(cls, Class::Walk | Class::Steep) && road[k] > 0 {
                            v *= RoadKind::ALL[road[k] as usize - 1].travel_factor();
                        }
                        c[o] = cls;
                        s[o] = surf;
                        sp[o] = v;
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if d % 64 == 0 {
                        progress.store(300 + (d * 700 / n) as u32, Ordering::Relaxed);
                    }
                }
            });
            j0 += rows;
        }
    });
    open_sea(n, cell, &mut class, &mut speed);
    progress.store(1000, Ordering::Relaxed);
    WalkGrid {
        n,
        cell,
        origin,
        class,
        surface,
        speed,
        slope,
        road,
        zone,
        steep: st.steep,
        max_grade: cliff.to_radians().tan(),
    }
}
