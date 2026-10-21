//! Realm land: the shape and size of a realm's world (Dungeons › Create a dungeon › Realm).
//!
//! A realm needn't be a giant round island. It can be a rounded isle with lobes and bays, a long
//! bent isle, land curved round a bay, twin isles across a strait, a chain of islands, a coast
//! with mountains behind it, a flat valley walled in by cliffs, a highland plateau, land round a
//! lake, a canyon or a cozy forest glade; from half a kilometre across (cozy) to four (large).
//!
//! It is the continent generator's method (`continent`) brought down to realm sizes: a macro shape
//! (a signed distance in map units; turned, stretched and domain-warped, so no two read alike and
//! none reads as a circle) is roughened by warped fractal coast noise and thresholded at the
//! percentile that gives the shape its share of the map; chamfer distances to that outline then
//! shape the heights: beaches, a rising interior, mountains along the land's spine and a shelf
//! offshore, or, for the walled shapes, a level floor ringed by cliffs, stepped walls or wooded
//! hills (their outer slopes fall to a strip of sea at the very rim, never seen from the floor).
//! Erosion gives the land its drainage, and a level pad is laid where players arrive, near the
//! middle. Every length scales with the realm's size: a cozy realm is cozy, not a shrunken
//! continent. `Detail::Preview` runs the same steps on a coarse grid without erosion, for the
//! live preview map of the create flow.

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};

use super::GridRect;
use super::continent::{chamfer, soft_ridges};
use super::erosion::{self, ErosionPreset, Progress};
use crate::noise::{Rng, fbm, smoothstep};

const TAU: f32 = std::f32::consts::TAU;
const PI: f32 = std::f32::consts::PI;

/// The shape of a realm's land.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealmShape {
    RoundIsle,
    LongIsle,
    CrescentBay,
    TwinIsles,
    IslandChain,
    CoastalShelf,
    EnclosedValley,
    HighlandPlateau,
    Lakeland,
    Canyon,
    Glade,
}

impl RealmShape {
    pub const ALL: [RealmShape; 11] = [
        RealmShape::RoundIsle,
        RealmShape::LongIsle,
        RealmShape::CrescentBay,
        RealmShape::TwinIsles,
        RealmShape::IslandChain,
        RealmShape::CoastalShelf,
        RealmShape::EnclosedValley,
        RealmShape::HighlandPlateau,
        RealmShape::Lakeland,
        RealmShape::Canyon,
        RealmShape::Glade,
    ];
    /// snake_case id (saved, MCP).
    pub fn id(self) -> &'static str {
        match self {
            RealmShape::RoundIsle => "round_isle",
            RealmShape::LongIsle => "long_isle",
            RealmShape::CrescentBay => "crescent_bay",
            RealmShape::TwinIsles => "twin_isles",
            RealmShape::IslandChain => "island_chain",
            RealmShape::CoastalShelf => "coastal_shelf",
            RealmShape::EnclosedValley => "enclosed_valley",
            RealmShape::HighlandPlateau => "highland_plateau",
            RealmShape::Lakeland => "lakeland",
            RealmShape::Canyon => "canyon",
            RealmShape::Glade => "glade",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            RealmShape::RoundIsle => "Round isle",
            RealmShape::LongIsle => "Long isle",
            RealmShape::CrescentBay => "Crescent bay",
            RealmShape::TwinIsles => "Twin isles",
            RealmShape::IslandChain => "Island chain",
            RealmShape::CoastalShelf => "Coastal shelf",
            RealmShape::EnclosedValley => "Enclosed valley",
            RealmShape::HighlandPlateau => "Highland plateau",
            RealmShape::Lakeland => "Land round a lake",
            RealmShape::Canyon => "Canyon",
            RealmShape::Glade => "Cozy glade",
        }
    }
    /// A tile's caption.
    pub fn short(self) -> &'static str {
        match self {
            RealmShape::RoundIsle => "Round isle",
            RealmShape::LongIsle => "Long isle",
            RealmShape::CrescentBay => "Crescent",
            RealmShape::TwinIsles => "Twin isles",
            RealmShape::IslandChain => "Chain",
            RealmShape::CoastalShelf => "Coast",
            RealmShape::EnclosedValley => "Valley",
            RealmShape::HighlandPlateau => "Plateau",
            RealmShape::Lakeland => "Lake",
            RealmShape::Canyon => "Canyon",
            RealmShape::Glade => "Glade",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            RealmShape::RoundIsle => {
                "A rounded island with lobes and bays, its hills rising to a ridge"
            }
            RealmShape::LongIsle => "A long island bent like a bean, a ridge along its spine",
            RealmShape::CrescentBay => "Land curved round a sheltered bay that opens to the sea",
            RealmShape::TwinIsles => "A big island and a smaller one across a narrow strait",
            RealmShape::IslandChain => {
                "A string of islands along an arc, the largest in the middle"
            }
            RealmShape::CoastalShelf => {
                "Land along one side with mountains behind it, the sea along the other"
            }
            RealmShape::EnclosedValley => {
                "A wide, level valley floor walled in by cliffs and ridges, no sea in sight: room for a temple compound"
            }
            RealmShape::HighlandPlateau => {
                "A high table land with cliff edges, ramps down to a shore below"
            }
            RealmShape::Lakeland => "Rolling land round a lake near the middle",
            RealmShape::Canyon => "A winding canyon between stepped cliffs, high mesas above",
            RealmShape::Glade => "A small clearing ringed by wooded hills",
        }
    }
    /// By id or name ("Long isle", "long-isle"), or a plain word ("valley", "lake", "rift").
    pub fn from_id(s: &str) -> Option<Self> {
        let k = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        if let Some(sh) = Self::ALL
            .into_iter()
            .find(|x| x.id() == k || x.name().to_ascii_lowercase().replace(' ', "_") == k)
        {
            return Some(sh);
        }
        Some(match k.as_str() {
            "round" | "isle" | "island" | "round_island" => RealmShape::RoundIsle,
            "long" | "long_island" | "elongated" => RealmShape::LongIsle,
            "crescent" | "bay" => RealmShape::CrescentBay,
            "twin" | "twins" | "twin_islands" => RealmShape::TwinIsles,
            "chain" | "archipelago" | "islands" => RealmShape::IslandChain,
            "coast" | "shelf" | "coastal" => RealmShape::CoastalShelf,
            "valley" | "basin" => RealmShape::EnclosedValley,
            "plateau" | "highland" | "mesa" => RealmShape::HighlandPlateau,
            "lake" | "land_round_a_lake" | "lake_land" => RealmShape::Lakeland,
            "rift" | "gorge" => RealmShape::Canyon,
            "cozy_glade" | "clearing" | "forest_glade" => RealmShape::Glade,
            _ => return None,
        })
    }
    /// A level floor ringed by walls (no coast to walk to).
    pub fn walled(self) -> bool {
        matches!(
            self,
            RealmShape::EnclosedValley | RealmShape::Canyon | RealmShape::Glade
        )
    }
}

/// How big a realm is (its terrain grid follows: a cozy realm is cheap).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RealmSize {
    /// half a kilometre
    Cozy,
    /// 1 km
    Small,
    /// 2 km (realms made before sizes)
    #[default]
    Medium,
    /// 4 km
    Large,
}

impl RealmSize {
    pub const ALL: [RealmSize; 4] = [
        RealmSize::Cozy,
        RealmSize::Small,
        RealmSize::Medium,
        RealmSize::Large,
    ];
    pub fn id(self) -> &'static str {
        match self {
            RealmSize::Cozy => "cozy",
            RealmSize::Small => "small",
            RealmSize::Medium => "medium",
            RealmSize::Large => "large",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            RealmSize::Cozy => "Cozy",
            RealmSize::Small => "Small",
            RealmSize::Medium => "Medium",
            RealmSize::Large => "Large",
        }
    }
    /// "½ km", "1 km"…
    pub fn across(self) -> &'static str {
        match self {
            RealmSize::Cozy => "½ km",
            RealmSize::Small => "1 km",
            RealmSize::Medium => "2 km",
            RealmSize::Large => "4 km",
        }
    }
    pub fn km(self) -> f32 {
        self.metres() / 1024.0
    }
    /// Across the map (m).
    pub fn metres(self) -> f32 {
        let (res, cell) = self.grid();
        res as f32 * cell
    }
    /// Terrain cells per side and the cell size (m).
    pub fn grid(self) -> (usize, f32) {
        match self {
            RealmSize::Cozy => (256, 2.0),
            RealmSize::Small => (512, 2.0),
            RealmSize::Medium => (512, 4.0),
            RealmSize::Large => (1024, 4.0),
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        let k = s.trim().to_ascii_lowercase().replace(' ', "");
        Self::ALL
            .into_iter()
            .find(|x| x.id() == k)
            .or(match k.trim_end_matches("km") {
                "0.5" | "½" | "half" => Some(RealmSize::Cozy),
                "1" => Some(RealmSize::Small),
                "2" => Some(RealmSize::Medium),
                "4" => Some(RealmSize::Large),
                _ => None,
            })
    }
}

/// How much work a generation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// the realm itself: its size's grid, eroded
    Full,
    /// a quick look: this many cells per side, no erosion
    Preview(usize),
}

/// A generated realm's land.
pub struct Land {
    pub res: usize,
    pub cell: f32,
    /// heights (m, the sea at 0), `(res + 1)²` row-major
    pub heights: Vec<f32>,
    /// where players arrive (x, z): level ground near the middle
    pub arrival: Vec2,
    /// a lake to fill with water (its outline, world x z): land round a lake
    pub lake: Option<Vec<Vec2>>,
    /// per vertex, -1..1: a glade's clearing kept clear of trees (> 0) and the wooded hills round
    /// it grown thicker (< 0); empty when none
    pub clearing: Vec<f32>,
    /// per vertex: under the lake (the preview paints it as water)
    pub wet: Vec<bool>,
    /// share of the map above the sea
    pub land_share: f32,
}

impl Land {
    pub fn n(&self) -> usize {
        self.res + 1
    }
    pub fn size(&self) -> f32 {
        self.res as f32 * self.cell
    }
    /// Height at a world position (bilinear).
    #[allow(dead_code)]
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let half = self.size() * 0.5;
        sample(
            &self.heights,
            self.n(),
            (x + half) / self.cell,
            (z + half) / self.cell,
        )
    }
}

fn sample(h: &[f32], n: usize, gx: f32, gz: f32) -> f32 {
    let m = (n - 1) as f32;
    let (gx, gz) = (gx.clamp(0.0, m - 0.001), gz.clamp(0.0, m - 0.001));
    let (i, j) = (gx as usize, gz as usize);
    let (fx, fz) = (gx - i as f32, gz - j as f32);
    let k = j * n + i;
    let a = h[k] + (h[k + 1] - h[k]) * fx;
    let b = h[k + n] + (h[k + n + 1] - h[k + n]) * fx;
    a + (b - a) * fz
}

// ------------------------------------------------------------------ macro shapes

/// An ellipse in the shape's frame: centre, radii, its turn (cos, sin).
#[derive(Clone, Copy)]
struct Ell {
    c: Vec2,
    r: Vec2,
    cs: (f32, f32),
}

impl Ell {
    fn new(c: Vec2, rx: f32, ry: f32, a: f32) -> Self {
        Ell {
            c,
            r: Vec2::new(rx, ry),
            cs: (a.cos(), a.sin()),
        }
    }
    /// About the signed distance into it (map units).
    fn sdf(&self, p: Vec2) -> f32 {
        let d = p - self.c;
        let q = Vec2::new(
            d.x * self.cs.0 + d.y * self.cs.1,
            -d.x * self.cs.1 + d.y * self.cs.0,
        );
        let k = ((q.x / self.r.x).powi(2) + (q.y / self.r.y).powi(2)).sqrt();
        (1.0 - k) * self.r.x.min(self.r.y)
    }
}

/// Smooth union (polynomial smooth max).
fn smax(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (a - b) / k).clamp(0.0, 1.0);
    b + (a - b) * h + k * h * (1.0 - h)
}

/// Signed distance into a tube along a polyline whose half-width changes from point to point.
fn tube(p: Vec2, pts: &[Vec2], w: &[f32]) -> f32 {
    let mut best = f32::MIN;
    for k in 0..pts.len().saturating_sub(1) {
        let (a, b) = (pts[k], pts[k + 1]);
        let ab = b - a;
        let s = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        let d = (p - (a + ab * s)).length();
        best = best.max(w[k] + (w[k + 1] - w[k]) * s - d);
    }
    best
}

fn bezier(c: [Vec2; 4], t: f32) -> Vec2 {
    let u = 1.0 - t;
    c[0] * (u * u * u) + c[1] * (3.0 * u * u * t) + c[2] * (3.0 * u * t * t) + c[3] * (t * t * t)
}

fn dir(a: f32) -> Vec2 {
    Vec2::new(a.cos(), a.sin())
}

/// The macro shape of one realm (from its shape and seed), in map units: (u, v) ∈ [-1, 1]².
struct Plan {
    shape: RealmShape,
    seed: u32,
    turn: (f32, f32),
    /// how far the shape wanders (domain warp, map units)
    warp: f32,
    blobs: Vec<Ell>,
    smooth: f32,
    spine: Vec<Vec2>,
    widths: Vec<f32>,
    spurs: Vec<(Vec<Vec2>, Vec<f32>)>,
    lake: Vec<Ell>,
    /// crescent: arc centre x, radius, horn angle, width, stretch across
    crescent: [f32; 5],
    /// coastal shelf: coast x, its sway, sway frequency, phase
    coast: [f32; 4],
    bays: Vec<Ell>,
    /// twin isles: the strait's x and half width
    strait: (f32, f32),
    /// share of the map that is land (floor, for walled shapes)
    share: f32,
    /// coast raggedness (amplitude of the coast noise, map units)
    rough: f32,
    /// mountains (× the size's peak)
    relief: f32,
    /// the rim: where land (floor) gives way, along the edge metric, and how hard
    rim: (f32, f32, f32),
    /// plateau ramps: directions down from the top
    ramps: Vec<f32>,
    /// how square the rim is (exponent of the edge metric the rim term reads)
    rim_p: f32,
}

impl Plan {
    fn new(shape: RealmShape, seed: u32) -> Plan {
        let mut rng = Rng::new(((seed as u64) << 8) ^ shape as u64 ^ 0x5ea1_3a9d);
        let a = rng.f32() * TAU;
        let mut p = Plan {
            shape,
            seed,
            turn: (a.cos(), a.sin()),
            warp: 0.16,
            blobs: vec![],
            smooth: 0.1,
            spine: vec![],
            widths: vec![],
            spurs: vec![],
            lake: vec![],
            crescent: [0.0; 5],
            coast: [0.0; 4],
            bays: vec![],
            strait: (0.0, 0.0),
            share: 0.3,
            rough: 0.28,
            relief: 1.0,
            rim: (0.76, 0.97, 1.2),
            ramps: vec![],
            rim_p: 6.0,
        };
        let vary = rng.range(0.7, 1.35);
        match shape {
            RealmShape::RoundIsle | RealmShape::HighlandPlateau => {
                let plateau = shape == RealmShape::HighlandPlateau;
                let rx = rng.range(0.54, 0.62);
                let asp = if plateau {
                    rng.range(1.45, 1.85)
                } else {
                    rng.range(1.25, 1.65)
                };
                p.blobs.push(Ell::new(
                    Vec2::new(rng.range(-0.06, 0.06), rng.range(-0.06, 0.06)),
                    rx,
                    rx / asp,
                    0.0,
                ));
                for _ in 0..2 + rng.below(2) {
                    let r = rng.range(0.17, 0.27);
                    p.blobs.push(Ell::new(
                        dir(rng.f32() * TAU)
                            * if plateau {
                                rng.range(0.32, 0.46)
                            } else {
                                rng.range(0.26, 0.4)
                            },
                        r,
                        r / rng.range(1.0, 1.6),
                        rng.f32() * PI,
                    ));
                }
                if plateau {
                    (p.share, p.rough, p.relief, p.warp) = (0.3, 0.28, 0.0, 0.2);
                    let r0 = rng.f32() * TAU;
                    p.ramps = vec![r0, r0 + rng.range(2.0, 4.2)];
                } else {
                    (p.share, p.rough, p.relief, p.warp) = (0.3, 0.3, 1.0, 0.2);
                }
            }
            RealmShape::LongIsle => {
                let len = rng.range(1.3, 1.5);
                let (mut b1, b2) = (rng.range(-0.42, 0.42), rng.range(-0.42, 0.42));
                if b1.abs() + b2.abs() < 0.3 {
                    b1 = if b1 < 0.0 { -0.32 } else { 0.32 };
                }
                let c = [
                    Vec2::new(-len / 2.0, 0.0),
                    Vec2::new(-len / 6.0, b1),
                    Vec2::new(len / 6.0, b2),
                    Vec2::new(len / 2.0, 0.0),
                ];
                // (the land passes near the middle)
                let mid = bezier(c, 0.5) * 0.7;
                let w0 = rng.range(0.24, 0.3);
                let head = rng.range(-0.5, 0.5);
                for k in 0..=28 {
                    let t = k as f32 / 28.0;
                    p.spine.push(bezier(c, t) - mid);
                    p.widths.push(
                        w0 * (0.5 + 0.5 * (PI * t).sin().powf(0.6)) * (1.0 + head * (t - 0.5)),
                    );
                }
                (p.share, p.rough, p.relief, p.warp) = (0.21, 0.26, 0.9, 0.15);
            }
            RealmShape::CrescentBay => {
                p.crescent = [
                    rng.range(0.24, 0.32),
                    rng.range(0.46, 0.54),
                    rng.range(1.95, 2.35),
                    rng.range(0.22, 0.26),
                    rng.range(1.0, 1.3),
                ];
                if rng.f32() < 0.6 {
                    // an islet in the bay
                    let r = rng.range(0.05, 0.08);
                    p.blobs.push(Ell::new(
                        Vec2::new(
                            p.crescent[0] + rng.range(-0.08, 0.08),
                            rng.range(-0.14, 0.14),
                        ),
                        r,
                        r / rng.range(1.0, 1.5),
                        rng.f32() * PI,
                    ));
                }
                (p.share, p.rough, p.relief, p.warp, p.smooth) = (0.21, 0.26, 0.8, 0.16, 0.03);
            }
            RealmShape::TwinIsles => {
                let xs = rng.range(-0.02, 0.12);
                let gap = rng.range(0.045, 0.07);
                let (rb, rs) = (rng.range(0.4, 0.46), rng.range(0.28, 0.33));
                let big = Vec2::new(xs - gap - rb * 0.92, rng.range(-0.12, 0.12));
                let small = Vec2::new(xs + gap + rs * 0.92, rng.range(-0.15, 0.15));
                for (c, r) in [(big, rb), (small, rs)] {
                    let asp = rng.range(1.25, 1.6);
                    let a = rng.range(-0.6, 0.6) + if rng.f32() < 0.5 { PI * 0.5 } else { 0.0 };
                    p.blobs.push(Ell::new(c, r, r / asp, a));
                    let rl = r * rng.range(0.45, 0.6);
                    p.blobs.push(Ell::new(
                        c + dir(rng.f32() * TAU) * r * 0.55,
                        rl,
                        rl / rng.range(1.0, 1.4),
                        rng.f32() * PI,
                    ));
                }
                p.strait = (xs, gap);
                (p.share, p.rough, p.relief, p.warp, p.smooth) = (0.24, 0.26, 0.85, 0.13, 0.06);
            }
            RealmShape::IslandChain => {
                let c = [
                    Vec2::new(-0.74, rng.range(-0.2, 0.2)),
                    Vec2::new(-0.25, rng.range(-0.5, 0.5)),
                    Vec2::new(0.25, rng.range(-0.5, 0.5)),
                    Vec2::new(0.74, rng.range(-0.2, 0.2)),
                ];
                let mid = bezier(c, 0.5) * 0.8;
                // islands strung along the arc, each its own length along it, a strait between
                // each and the next; the largest near the middle
                let k = 4 + rng.below(2) as usize;
                let big = (k - 1) / 2 + rng.below(2) as usize;
                let isles: Vec<(f32, f32)> = (0..k)
                    .map(|i| {
                        let r = if i == big {
                            rng.range(0.23, 0.27)
                        } else {
                            rng.range(0.13, 0.17)
                                * (1.0 - 0.3 * (i as f32 - big as f32).abs() / k as f32)
                        };
                        (r * rng.range(1.0, 1.25), r * rng.range(0.85, 1.0))
                    })
                    .collect();
                let gaps: Vec<f32> = (0..k - 1).map(|_| rng.range(0.025, 0.05)).collect();
                let arc: Vec<Vec2> = (0..=64).map(|i| bezier(c, i as f32 / 64.0) - mid).collect();
                let len: f32 = arc.windows(2).map(|w| w[0].distance(w[1])).sum();
                let need: f32 =
                    isles.iter().map(|(a, _)| a * 2.0).sum::<f32>() + gaps.iter().sum::<f32>();
                // (a chain longer than its arc runs on past it, the ends fading at the rim)
                let fit = (len / need).clamp(0.85, 1.1);
                let at_len = |l: f32| {
                    let mut acc = 0.0;
                    for w in arc.windows(2) {
                        let d = w[0].distance(w[1]);
                        if acc + d >= l {
                            let f = (l - acc) / d.max(1e-6);
                            return (w[0] + (w[1] - w[0]) * f, (w[1] - w[0]).normalize_or_zero());
                        }
                        acc += d;
                    }
                    (arc[64], (arc[64] - arc[63]).normalize_or_zero())
                };
                let mut l = (len - need * fit) * 0.5;
                let mut area = 0.0;
                for (i, (ra, rb)) in isles.iter().enumerate() {
                    let (ra, rb) = (ra * fit, rb * fit);
                    l += ra;
                    let (at, tan) = at_len(l);
                    let nrm = Vec2::new(-tan.y, tan.x);
                    p.blobs.push(Ell::new(
                        at + nrm * rng.range(-0.04, 0.04),
                        ra,
                        rb,
                        tan.y.atan2(tan.x) + rng.range(-0.35, 0.35),
                    ));
                    area += PI * ra * rb;
                    l += ra + gaps.get(i).copied().unwrap_or(0.0) * fit;
                }
                (p.share, p.rough, p.relief, p.warp, p.smooth) =
                    (area / 4.0 * 0.95, 0.16, 0.55, 0.1, 0.02);
            }
            RealmShape::CoastalShelf => {
                p.coast = [
                    rng.range(-0.2, -0.06),
                    rng.range(0.06, 0.14),
                    rng.range(1.6, 2.8),
                    rng.f32() * TAU,
                ];
                let coast_x = |y: f32, c: &[f32; 4]| c[0] + c[1] * (y * c[2] + c[3]).sin();
                for _ in 0..1 + rng.below(2) {
                    let y = rng.range(-0.55, 0.55);
                    let r = rng.range(0.12, 0.2);
                    p.bays.push(Ell::new(
                        Vec2::new(coast_x(y, &p.coast) + r * 0.25, y),
                        r,
                        r / rng.range(1.0, 1.5),
                        rng.f32() * PI,
                    ));
                }
                if rng.f32() < 0.6 {
                    let y = rng.range(-0.5, 0.5);
                    let r = rng.range(0.09, 0.15);
                    p.blobs.push(Ell::new(
                        Vec2::new(coast_x(y, &p.coast) - r * 0.8, y),
                        r * 1.4,
                        r,
                        rng.range(-0.4, 0.4),
                    ));
                }
                (p.share, p.rough, p.relief, p.warp, p.smooth) = (0.5, 0.3, 0.5, 0.15, 0.05);
                p.rim = (0.9, 0.99, 2.0);
            }
            RealmShape::Lakeland => {
                let rx = rng.range(0.64, 0.72);
                p.blobs.push(Ell::new(
                    Vec2::new(rng.range(-0.05, 0.05), rng.range(-0.05, 0.05)),
                    rx,
                    rx / rng.range(1.3, 1.6),
                    0.0,
                ));
                for _ in 0..1 + rng.below(2) {
                    let r = rng.range(0.2, 0.27);
                    p.blobs.push(Ell::new(
                        dir(rng.f32() * TAU) * rng.range(0.34, 0.44),
                        r,
                        r / rng.range(1.0, 1.5),
                        rng.f32() * PI,
                    ));
                }
                // the lake, off the middle a little (players arrive on its shore)
                let lc = dir(rng.f32() * TAU) * rng.range(0.14, 0.2);
                let (lr, la) = (rng.range(0.22, 0.27), rng.f32() * PI);
                p.lake.push(Ell::new(lc, lr, lr / rng.range(1.25, 1.6), la));
                p.lake.push(Ell::new(
                    lc + dir(la) * lr * 0.7 * if rng.f32() < 0.5 { 1.0 } else { -1.0 },
                    lr * 0.55,
                    lr * 0.4,
                    la + rng.range(-0.8, 0.8),
                ));
                (p.share, p.rough, p.relief, p.warp, p.smooth) = (0.4, 0.26, 0.6, 0.14, 0.08);
            }
            RealmShape::EnclosedValley => {
                // a long valley with side bays
                let (rx, ry) = (rng.range(0.68, 0.76), rng.range(0.36, 0.42));
                p.blobs.push(Ell::new(
                    Vec2::new(rng.range(-0.05, 0.05), rng.range(-0.05, 0.05)),
                    rx,
                    ry,
                    0.0,
                ));
                for k in 0..2 {
                    let r = rng.range(0.2, 0.26);
                    let along = rng.range(-0.45, 0.45) * rx;
                    let side = if (k == 0) == (rng.f32() < 0.5) {
                        1.0
                    } else {
                        -1.0
                    };
                    p.blobs.push(Ell::new(
                        Vec2::new(along, side * ry * rng.range(0.75, 0.95)),
                        r * rng.range(1.0, 1.4),
                        r,
                        rng.range(-0.8, 0.8) + PI * 0.5,
                    ));
                }
                (p.share, p.rough, p.relief, p.warp, p.smooth) = (0.32, 0.16, 0.0, 0.18, 0.08);
                p.rim = (0.66, 0.84, 2.0);
                p.rim_p = 4.0;
            }
            RealmShape::Canyon => {
                let c = [
                    Vec2::new(-0.66, rng.range(-0.15, 0.15)),
                    Vec2::new(-0.22, rng.range(-0.42, 0.42)),
                    Vec2::new(0.22, rng.range(-0.42, 0.42)),
                    Vec2::new(0.66, rng.range(-0.15, 0.15)),
                ];
                let mid = bezier(c, 0.5) * 0.8;
                let w0 = rng.range(0.085, 0.1);
                let rooms = [
                    (rng.range(0.2, 0.4), rng.range(0.7, 1.1)),
                    (rng.range(0.6, 0.8), rng.range(0.7, 1.1)),
                ];
                for k in 0..=32 {
                    let t = k as f32 / 32.0;
                    p.spine.push(bezier(c, t) - mid);
                    let room: f32 = rooms
                        .iter()
                        .map(|(at, amp)| amp * (-((t - at) / 0.07).powi(2)).exp())
                        .sum();
                    p.widths.push(
                        w0 * (0.75 + 0.25 * (t * 9.0 + seed as f32).sin())
                            * (1.0 + room)
                            * smoothstep(0.0, 0.06, t.min(1.0 - t)).max(0.35),
                    );
                }
                // side gorges
                for _ in 0..1 + rng.below(2) {
                    let t = rng.range(0.25, 0.75);
                    let at = bezier(c, t) - mid;
                    let tan = (bezier(c, t + 0.01) - bezier(c, t - 0.01)).normalize_or_zero();
                    let mut a = tan.y.atan2(tan.x)
                        + rng.range(0.8, 1.3) * if rng.f32() < 0.5 { 1.0 } else { -1.0 };
                    let len = rng.range(0.16, 0.26);
                    let (mut pts, mut ws) = (vec![at], vec![w0 * 0.6]);
                    for s in 1..=6 {
                        a += rng.range(-0.25, 0.25);
                        pts.push(pts[s - 1] + dir(a) * len / 6.0);
                        ws.push(w0 * (0.6 - 0.35 * s as f32 / 6.0));
                    }
                    p.spurs.push((pts, ws));
                }
                (p.share, p.rough, p.relief, p.warp) = (0.13, 0.1, 0.0, 0.1);
                p.rim = (0.64, 0.82, 2.0);
                p.rim_p = 3.0;
            }
            RealmShape::Glade => {
                let rx = rng.range(0.5, 0.58);
                p.blobs.push(Ell::new(
                    Vec2::new(rng.range(-0.06, 0.06), rng.range(-0.06, 0.06)),
                    rx,
                    rx / rng.range(1.5, 1.9),
                    0.0,
                ));
                for _ in 0..1 + rng.below(2) {
                    let r = rng.range(0.17, 0.23);
                    let a = rng.f32() * TAU;
                    p.blobs.push(Ell::new(
                        Vec2::new(a.cos() * rx * 0.55, a.sin() * rx * 0.45),
                        r,
                        r / rng.range(1.0, 1.5),
                        rng.f32() * PI,
                    ));
                }
                (p.share, p.rough, p.relief, p.warp, p.smooth) = (0.2, 0.22, 0.0, 0.24, 0.08);
                p.rim = (0.6, 0.8, 2.0);
                p.rim_p = 3.0;
            }
        }
        p.rough *= vary;
        p
    }

    /// (u, v) warped and turned into the shape's own frame.
    fn frame(&self, u: f32, v: f32) -> Vec2 {
        let (s, k) = (self.seed, self.warp);
        let wu = u + fbm(u * 1.1 + 4.1, v * 1.1 - 2.3, 3, 2.0, 0.5, s + 21) * k;
        let wv = v + fbm(u * 1.1 - 7.9, v * 1.1 + 5.2, 3, 2.0, 0.5, s + 22) * k;
        let (c, sn) = self.turn;
        Vec2::new(wu * c + wv * sn, -wu * sn + wv * c)
    }

    fn blobs_at(&self, p: Vec2) -> f32 {
        self.blobs
            .iter()
            .fold(-4.0f32, |a, e| smax(a, e.sdf(p), self.smooth))
    }

    /// Landness (floor-ness, for walled shapes): > 0 leans land, about a distance in map units.
    fn at(&self, u: f32, v: f32) -> f32 {
        let p = self.frame(u, v);
        match self.shape {
            RealmShape::RoundIsle
            | RealmShape::HighlandPlateau
            | RealmShape::EnclosedValley
            | RealmShape::Glade
            | RealmShape::Lakeland => self.blobs_at(p),
            RealmShape::IslandChain => self.blobs_at(p),
            RealmShape::TwinIsles => self
                .blobs_at(p)
                .min(((p.x - self.strait.0).abs() - self.strait.1) * 2.5),
            RealmShape::LongIsle => tube(p, &self.spine, &self.widths),
            RealmShape::Canyon => self
                .spurs
                .iter()
                .fold(tube(p, &self.spine, &self.widths), |a, (s, w)| {
                    smax(a, tube(p, s, w), 0.03)
                }),
            RealmShape::CrescentBay => {
                let [cx, r, horn, w0, st] = self.crescent;
                let q = Vec2::new(p.x, p.y * st) - Vec2::new(cx, 0.0);
                // 0 at the back of the arc, π at the bay's mouth
                let back = PI - q.y.atan2(q.x).abs();
                let w = if back < horn {
                    w0 * (back / horn * PI * 0.5).cos().max(0.0).powf(0.55)
                } else {
                    -(back - horn) * r
                };
                smax(w - (q.length() - r).abs(), self.blobs_at(p), self.smooth)
            }
            RealmShape::CoastalShelf => {
                let [x0, amp, f, ph] = self.coast;
                let mut d = p.x - (x0 + amp * (p.y * f + ph).sin());
                for b in &self.bays {
                    d = d.min(-b.sdf(p));
                }
                smax(d, self.blobs_at(p), self.smooth)
            }
        }
    }

    /// The lake of land round a lake: > 0 inside.
    fn lake_at(&self, u: f32, v: f32) -> f32 {
        let p = self.frame(u, v);
        self.lake
            .iter()
            .fold(-4.0f32, |a, e| smax(a, e.sdf(p), 0.06))
    }
}

/// How near the map's rim (a rounded square): 0 in the middle, about 1 at the edge, wandering.
fn edge(u: f32, v: f32, seed: u32) -> f32 {
    edge_p(u, v, seed, 6.0)
}

/// `edge` with its own squareness (2: round, 6: a rounded square).
fn edge_p(u: f32, v: f32, seed: u32, p: f32) -> f32 {
    (u.abs().powf(p) + v.abs().powf(p)).powf(1.0 / p)
        + fbm(u * 3.0, v * 3.0, 3, 2.0, 0.5, seed + 36) * 0.05
}

fn percentile(v: &[f32], q: f32) -> f32 {
    let mut s: Vec<f32> = v.iter().step_by(3).copied().collect();
    s.sort_by(|a, b| a.total_cmp(b));
    s[((q.clamp(0.0, 1.0)) * (s.len() - 1) as f32) as usize]
}

/// Heights of a realm's land. Same shape, size and seed: same land (the preview shows it coarser).
pub fn generate(shape: RealmShape, size: RealmSize, seed: u32, detail: Detail) -> Land {
    // (noise seeds are offset from it: keep clear of overflow)
    let seed = seed & 0x0fff_ffff;
    let (res, cell) = match detail {
        Detail::Full => size.grid(),
        Detail::Preview(r) => (r.max(16), size.metres() / r.max(16) as f32),
    };
    let n = res + 1;
    let size_m = res as f32 * cell;
    let half = size_m * 0.5;
    // lengths scale with the size; heights a little less than lengths (a cozy realm's hills are
    // lower, not just narrower)
    let s = size_m / 2048.0;
    let vs = s.powf(0.6);
    let plan = Plan::new(shape, seed);
    let walled = shape.walled();
    let xz = |k: usize| Vec2::new((k % n) as f32 * cell - half, (k / n) as f32 * cell - half);

    // --- the outline: the macro shape roughened by warped fractal coast noise, thresholded at the
    // shape's share of the map
    let oct = (size_m / 3.0 / (cell * 2.5)).log2().ceil().clamp(2.0, 9.0) as u32;
    let base_f = 3.0 / size_m;
    let gain = 0.5 + 0.12 * plan.rough.min(1.0);
    let mut field = vec![0.0f32; n * n];
    let mut lake_raw = vec![
        -1.0f32;
        if shape == RealmShape::Lakeland {
            n * n
        } else {
            0
        }
    ];
    erosion::par_rows(&mut field, n, |j, row| {
        for (i, out) in row.iter_mut().enumerate() {
            let (x, z) = (i as f32 * cell - half, j as f32 * cell - half);
            let (u, v) = (x / half, z / half);
            let wx = fbm(
                x * base_f * 0.7 + 1.7,
                z * base_f * 0.7 - 3.3,
                4,
                2.0,
                0.5,
                seed + 31,
            ) * size_m
                * 0.05
                + fbm(
                    x / (size_m * 0.16),
                    z / (size_m * 0.16),
                    3,
                    2.0,
                    0.5,
                    seed + 34,
                ) * size_m
                    * 0.03;
            let wz = fbm(
                x * base_f * 0.7 - 6.1,
                z * base_f * 0.7 + 2.9,
                4,
                2.0,
                0.5,
                seed + 32,
            ) * size_m
                * 0.05
                + fbm(
                    x / (size_m * 0.16) + 9.1,
                    z / (size_m * 0.16),
                    3,
                    2.0,
                    0.5,
                    seed + 35,
                ) * size_m
                    * 0.03;
            let coast = fbm(
                (x + wx) * base_f,
                (z + wz) * base_f,
                oct,
                2.0,
                gain,
                seed + 33,
            );
            *out = plan.at(u, v) + coast * plan.rough
                - smoothstep(plan.rim.0, plan.rim.1, edge_p(u, v, seed, plan.rim_p)) * plan.rim.2;
        }
    });
    if !lake_raw.is_empty() {
        erosion::par_rows(&mut lake_raw, n, |j, row| {
            for (i, out) in row.iter_mut().enumerate() {
                let (x, z) = (i as f32 * cell - half, j as f32 * cell - half);
                *out = plan.lake_at(x / half, z / half)
                    + fbm(
                        x * base_f * 2.0,
                        z * base_f * 2.0,
                        oct.min(6),
                        2.0,
                        0.5,
                        seed + 61,
                    ) * 0.05;
            }
        });
    }
    let thr = percentile(&field, 1.0 - plan.share);
    let mut inside: Vec<bool> = field.iter().map(|f| *f > thr).collect();
    drop(field);
    if !walled {
        fill_holes(&mut inside, n);
    }
    // distances to the outline, inside and out, and smooth versions of them (a raw distance field
    // has creases along its medial axis)
    let to_out = chamfer(&inside.iter().map(|b| !b).collect::<Vec<_>>(), n, cell);
    let to_in = chamfer(&inside, n, cell);
    let mut f = 1;
    while cell * (f * 2) as f32 <= 8.0 && (n - 1) / (f * 2) >= 64 {
        f *= 2;
    }
    let r = ((0.05 * size_m) / (cell * f as f32)).round().max(2.0) as usize;
    let to_out_s = erosion::smoothed(
        &to_out.iter().map(|d| d.min(size_m)).collect::<Vec<_>>(),
        n,
        f,
        r,
    );
    let to_in_s = erosion::smoothed(
        &to_in.iter().map(|d| d.min(size_m)).collect::<Vec<_>>(),
        n,
        f,
        r,
    );
    // the lake lies well inside the land
    let lake: Vec<bool> = if lake_raw.is_empty() {
        vec![false; n * n]
    } else {
        (0..n * n)
            .map(|k| inside[k] && lake_raw[k] > 0.0 && to_out[k] > 0.05 * size_m)
            .collect()
    };
    let has_lake = lake.iter().any(|b| *b);
    let (to_lake, in_lake) = if has_lake {
        (
            chamfer(&lake, n, cell),
            chamfer(&lake.iter().map(|b| !b).collect::<Vec<_>>(), n, cell),
        )
    } else {
        (vec![f32::MAX; n * n], vec![0.0; n * n])
    };

    // --- where players arrive: the land (floor) nearest the middle, some way from the sea (walls)
    let margin = 8.0 + 18.0 * s.sqrt();
    let cliff_at = |x: f32, z: f32| {
        0.075
            * size_m
            * (1.0
                + 0.4
                    * fbm(
                        x / (0.09 * size_m),
                        z / (0.09 * size_m),
                        3,
                        2.0,
                        0.5,
                        seed + 71,
                    ))
    };
    let cliff_w = 6.0 + 5.0 * s.sqrt();
    let ok = |k: usize| -> bool {
        if !inside[k] || to_out[k] < margin || lake[k] {
            return false;
        }
        match shape {
            RealmShape::HighlandPlateau => {
                let p = xz(k);
                to_out[k] > cliff_at(p.x, p.y) + cliff_w + margin
            }
            RealmShape::Lakeland => to_lake[k] > margin * 0.8,
            _ => true,
        }
    };
    let arrival = (0..n * n)
        .filter(|k| ok(*k))
        .min_by(|a, b| xz(*a).length_squared().total_cmp(&xz(*b).length_squared()))
        .or_else(|| {
            (0..n * n)
                .filter(|k| inside[*k] && !lake[*k])
                .max_by(|a, b| to_out[*a].total_cmp(&to_out[*b]))
        })
        .map(xz)
        .unwrap_or(Vec2::ZERO);

    // --- heights
    let peak = 120.0 * s.powf(0.72) * plan.relief;
    let range_dir = dir(Rng::new(seed as u64 ^ 0x2a4e).f32() * TAU);
    let ridge_k = s.clamp(0.3, 2.0);
    let beach_w = 4.0 + 10.0 * s.sqrt();
    let low_h = 10.0 * vs;
    let lake_lvl = low_h * 0.5 + 2.5;
    let plateau_h = 50.0 * vs;
    let ramp_w = 8.0 + 6.0 * s.sqrt();
    let (floor_h, wall_h, wall_w) = match shape {
        RealmShape::Canyon => (6.0 + 3.0 * vs, 60.0 * vs, 0.035 * size_m),
        RealmShape::Glade => (6.0 + 3.0 * vs, 45.0 * vs, 0.12 * size_m),
        _ => (8.0 + 4.0 * vs, 75.0 * vs, 0.07 * size_m),
    };
    let mut h = vec![0.0f32; n * n];
    erosion::par_rows(&mut h, n, |j, row| {
        for (i, out) in row.iter_mut().enumerate() {
            let k = j * n + i;
            let (x, z) = (i as f32 * cell - half, j as f32 * cell - half);
            let e = edge(x / half, z / half, seed);
            let hills = fbm(
                x / (0.09 * size_m),
                z / (0.09 * size_m),
                4,
                2.0,
                0.5,
                seed + 41,
            ) * 0.5
                + 0.5;
            let swells = fbm(
                x / (0.3 * size_m),
                z / (0.3 * size_m),
                3,
                2.0,
                0.5,
                seed + 42,
            ) * 0.5
                + 0.5;
            let ridge = soft_ridges(x / ridge_k, z / ridge_k, seed);
            let away = smoothstep(
                0.05 * size_m,
                0.15 * size_m,
                Vec2::new(x, z).distance(arrival),
            );
            let mut y;
            if walled {
                if inside[k] {
                    // the floor: level, a gentle swell, scree fans at the foot of the walls
                    let d = to_out[k];
                    y = floor_h
                        + fbm(
                            x / (0.12 * size_m),
                            z / (0.12 * size_m),
                            3,
                            2.0,
                            0.5,
                            seed + 44,
                        ) * 1.6
                            * vs
                        + wall_h * 0.05 * (-d / (0.025 * size_m)).exp();
                    if shape == RealmShape::Glade {
                        y += hills * hills * 3.0 * vs;
                    }
                } else {
                    let d = to_in[k];
                    let w = wall_w
                        * (1.0
                            + 0.35
                                * fbm(
                                    x / (0.1 * size_m),
                                    z / (0.1 * size_m),
                                    3,
                                    2.0,
                                    0.5,
                                    seed + 45,
                                ));
                    let t = d / w;
                    let wh = wall_h
                        * (1.0
                            + 0.3
                                * fbm(
                                    x / (0.2 * size_m),
                                    z / (0.2 * size_m),
                                    3,
                                    2.0,
                                    0.5,
                                    seed + 46,
                                ));
                    y = floor_h
                        + match shape {
                            // stepped walls (risers and ledges), mesas above
                            RealmShape::Canyon => {
                                let q = t.min(1.0) * 3.0;
                                let fl = q.floor().min(2.0);
                                wh * (fl + smoothstep(0.0, 0.35, q - fl)) / 3.0
                                    + hills * hills * 8.0 * vs * smoothstep(1.0, 1.6, t)
                                    + wh * 0.12 * smoothstep(1.0, 3.0, t)
                            }
                            // wooded hills rising gently all round
                            RealmShape::Glade => {
                                wh * smoothstep(0.0, 1.0, t).powf(0.8)
                                    + wh * 0.4 * hills * hills * smoothstep(0.3, 1.2, t)
                                    + wh * 0.2 * smoothstep(1.0, 3.0, t)
                            }
                            // cliffs, then ridges and peaks
                            _ => {
                                wh * (0.55 * smoothstep(0.0, 0.3, t)
                                    + 0.45 * smoothstep(0.15, 1.0, t))
                                    + wh * 0.5 * ridge * smoothstep(0.4, 1.4, t)
                                    + wh * 0.15 * smoothstep(1.0, 2.5, t)
                            }
                        };
                }
            } else if inside[k] {
                let dl = to_out[k];
                let dls = to_out_s[k];
                // a beach, then the interior rising away from the sea
                y = 2.2 * (1.0 - (-dl / beach_w).exp())
                    + low_h * (1.0 - (-dls / (0.06 * size_m)).exp());
                y += (hills * hills * 12.0 * vs + swells * swells * 14.0 * vs)
                    * smoothstep(0.0, 0.05 * size_m, dl);
                // mountains along the land's spine (where it is farthest from the sea), none where
                // players arrive
                if peak > 0.0 {
                    let pot = smoothstep(0.2, 1.0, dls / (0.11 * size_m));
                    let mask = smoothstep(
                        -0.2,
                        0.25,
                        fbm(
                            x / (0.4 * size_m),
                            z / (0.4 * size_m),
                            3,
                            2.0,
                            0.5,
                            seed + 43,
                        ) + 0.12,
                    );
                    let massif = fbm(
                        x / (0.2 * size_m),
                        z / (0.2 * size_m),
                        4,
                        2.0,
                        0.45,
                        seed + 7,
                    ) * 0.5
                        + 0.5;
                    // rounded land keeps its high ground to one side (a lowland round the arrival,
                    // not a crater ring)
                    let side = match shape {
                        RealmShape::RoundIsle | RealmShape::Lakeland => smoothstep(
                            -0.1,
                            0.3,
                            (Vec2::new(x, z) - arrival).dot(range_dir) / (0.2 * size_m),
                        ),
                        _ => 1.0,
                    };
                    y += peak * pot * mask * side * away * (0.25 + 0.55 * ridge + 0.2 * massif);
                }
                match shape {
                    // mountains behind the coastal plain, out to the map's far edges (their foot
                    // wanders in and out)
                    RealmShape::CoastalShelf => {
                        let foot = 0.52
                            + 0.14
                                * fbm(
                                    x / (0.25 * size_m),
                                    z / (0.25 * size_m),
                                    3,
                                    2.0,
                                    0.5,
                                    seed + 47,
                                );
                        y += 120.0
                            * s.powf(0.72)
                            * smoothstep(foot, foot + 0.28, e)
                            * smoothstep(0.0, 0.05 * size_m, dl)
                            * (0.35 + 0.65 * ridge)
                            * away;
                    }
                    // the table land: a cliff some way in from the shore, ramps down to it
                    RealmShape::HighlandPlateau => {
                        let cl = cliff_at(x, z);
                        let step = smoothstep(cl, cl + cliff_w, dl);
                        let ramp_len = plateau_h * 4.5;
                        let slope = smoothstep(cl - ramp_len, cl + cliff_w, dl);
                        let on_ramp = plan.ramps.iter().map(|a| {
                            let d = dir(*a);
                            let rel = Vec2::new(x, z) - arrival;
                            let along = rel.dot(d);
                            let off = (rel - d * along.max(0.0)).length();
                            1.0 - smoothstep(ramp_w, ramp_w * 1.8, off)
                        });
                        let w = on_ramp.fold(0.0f32, f32::max);
                        y += plateau_h * (step + (slope - step) * w)
                            + hills * hills * 4.0 * vs * step;
                    }
                    // a level shore round the lake
                    RealmShape::Lakeland if has_lake => {
                        let near = smoothstep(0.0, 0.12 * size_m, to_lake[k]);
                        y = (lake_lvl + to_lake[k].min(0.12 * size_m) * 0.05) * (1.0 - near)
                            + y * near;
                    }
                    _ => {}
                }
            } else {
                // offshore: a shallow margin, a shelf, then deep water
                let ds = to_in[k];
                let dss = to_in_s[k];
                let depth = 2.5 * (1.0 - (-ds / (6.0 + 6.0 * s.sqrt())).exp())
                    + 8.0 * vs * (1.0 - (-dss / (0.05 * size_m)).exp())
                    + 22.0 * vs * smoothstep(0.08 * size_m, 0.35 * size_m, dss)
                    + fbm(
                        x / (0.06 * size_m),
                        z / (0.06 * size_m),
                        3,
                        2.0,
                        0.5,
                        seed + 51,
                    ) * 1.5
                        * smoothstep(5.0, 60.0, ds);
                y = -depth.max(0.4);
            }
            // walled realms and the coast's far side fall to a strip of sea at the very rim
            if walled || shape == RealmShape::CoastalShelf {
                let fall = smoothstep(
                    0.88,
                    0.975,
                    edge_p(x / half, z / half, seed, 4.0)
                        + fbm(x / half * 2.2 + 3.3, z / half * 2.2, 3, 2.0, 0.5, seed + 37) * 0.06,
                );
                y = y * (1.0 - fall) + (-4.0 - 10.0 * vs) * fall;
            }
            *out = y;
        }
    });

    // --- erosion gives it drainage (the full realm only)
    if detail == Detail::Full {
        let preset = match shape {
            RealmShape::EnclosedValley
            | RealmShape::Canyon
            | RealmShape::HighlandPlateau
            | RealmShape::Glade => ErosionPreset::LightRain,
            _ if size >= RealmSize::Medium => ErosionPreset::HeavyRain,
            _ => ErosionPreset::LightRain,
        };
        let mut ep = preset.params();
        ep.seed = seed as u64;
        let pr = Progress::default();
        // a walled realm's floor is where the rain off its walls ends (as the sea is for an
        // island): routed on across a level, closed basin instead, the water ran in straight
        // parallel rills over the filled floor and cut a straight channel out through the walls
        // at their lowest pass
        let sink = if walled { floor_h + 1.6 * vs } else { 0.0 };
        erosion::erode(
            &mut h,
            n,
            cell,
            sink,
            GridRect {
                x0: 0,
                z0: 0,
                x1: n - 1,
                z1: n - 1,
            },
            None,
            &ep,
            pr.stage(0.0, 1.0),
        );
    }

    // --- the lake's bowl (a drawn lake fills it), below the shore round it
    let mut outline = None;
    if has_lake {
        let mut rim: Vec<f32> = (0..n * n)
            .filter(|k| !lake[*k] && to_lake[*k] <= cell * 1.5)
            .map(|k| h[k])
            .collect();
        rim.sort_by(|a, b| a.total_cmp(b));
        let level = rim.get(rim.len() / 4).copied().unwrap_or(lake_lvl);
        for k in 0..n * n {
            if lake[k] {
                h[k] =
                    h[k].min(level - 0.6 - 2.5 * vs * smoothstep(0.0, 0.04 * size_m, in_lake[k]));
            }
        }
        outline = lake_outline(&lake, &in_lake, n, cell);
    }

    // --- a level pad where players arrive (the way back out stands behind them)
    level_pad(&mut h, n, cell, arrival, 9.0, 10.0);

    let clearing = if shape == RealmShape::Glade {
        (0..n * n)
            .map(|k| {
                if inside[k] {
                    smoothstep(0.0, 6.0 + 0.02 * size_m, to_out[k])
                } else {
                    let p = xz(k);
                    -smoothstep(0.0, 12.0, to_in[k])
                        * (1.0 - smoothstep(0.8, 0.9, edge_p(p.x / half, p.y / half, seed, 4.0)))
                }
            })
            .collect()
    } else {
        vec![]
    };
    let land_share = h.iter().filter(|y| **y > 0.0).count() as f32 / (n * n) as f32;
    Land {
        res,
        cell,
        heights: h,
        arrival,
        lake: outline,
        clearing,
        wet: lake,
        land_share,
    }
}

/// Sea cut off from the open sea (a pocket inside the land) becomes land.
fn fill_holes(inside: &mut [bool], n: usize) {
    let mut open = vec![false; n * n];
    let mut stack: Vec<usize> = (0..n)
        .flat_map(|i| [i, (n - 1) * n + i, i * n, i * n + n - 1])
        .filter(|k| !inside[*k])
        .collect();
    for k in &stack {
        open[*k] = true;
    }
    while let Some(k) = stack.pop() {
        let (i, j) = (k % n, k / n);
        for (a, b) in [
            (i.wrapping_sub(1), j),
            (i + 1, j),
            (i, j.wrapping_sub(1)),
            (i, j + 1),
        ] {
            if a < n && b < n && !inside[b * n + a] && !open[b * n + a] {
                open[b * n + a] = true;
                stack.push(b * n + a);
            }
        }
    }
    for k in 0..n * n {
        if !open[k] {
            inside[k] = true;
        }
    }
}

/// The lake's outline (world x z): rays from its deepest point out to its shore.
fn lake_outline(lake: &[bool], in_lake: &[f32], n: usize, cell: f32) -> Option<Vec<Vec2>> {
    let half = (n - 1) as f32 * cell * 0.5;
    let deepest = (0..n * n)
        .filter(|k| lake[*k])
        .max_by(|a, b| in_lake[*a].total_cmp(&in_lake[*b]))?;
    let c = Vec2::new(
        (deepest % n) as f32 * cell - half,
        (deepest / n) as f32 * cell - half,
    );
    let wet = |p: Vec2| {
        let (i, j) = (((p.x + half) / cell).round(), ((p.y + half) / cell).round());
        i >= 0.0
            && j >= 0.0
            && (i as usize) < n
            && (j as usize) < n
            && lake[j as usize * n + i as usize]
    };
    let pts: Vec<Vec2> = (0..48)
        .map(|a| {
            let d = dir(a as f32 / 48.0 * TAU);
            let mut r = 0.0;
            while r < half && wet(c + d * (r + cell * 0.5)) {
                r += cell * 0.5;
            }
            c + d * r.max(cell)
        })
        .collect();
    (pts.len() >= 8).then_some(pts)
}

/// Levels the ground round `at` (radius `r`, blended over `blend`), at least 2.5 m above the sea.
fn level_pad(h: &mut [f32], n: usize, cell: f32, at: Vec2, r: f32, blend: f32) {
    let half = (n - 1) as f32 * cell * 0.5;
    let g = (at + Vec2::splat(half)) / cell;
    let reach = ((r + blend) / cell).ceil() as i32 + 1;
    let (gi, gj) = (g.x.round() as i32, g.y.round() as i32);
    let cells = move || {
        (-reach..=reach)
            .flat_map(move |dj| (-reach..=reach).map(move |di| (gi + di, gj + dj)))
            .filter(move |(i, j)| *i >= 0 && *j >= 0 && (*i as usize) < n && (*j as usize) < n)
    };
    let (mut sum, mut cnt) = (0.0, 0.0);
    for (i, j) in cells() {
        let p = Vec2::new(i as f32 * cell - half, j as f32 * cell - half);
        if p.distance(at) <= r {
            sum += h[j as usize * n + i as usize];
            cnt += 1.0;
        }
    }
    let target = if cnt > 0.0 {
        sum / cnt
    } else {
        sample(h, n, g.x, g.y)
    }
    .max(2.5);
    for (i, j) in cells() {
        let p = Vec2::new(i as f32 * cell - half, j as f32 * cell - half);
        let w = 1.0 - smoothstep(r, r + blend, p.distance(at));
        let k = j as usize * n + i as usize;
        h[k] += (target - h[k]) * w;
    }
}

// ------------------------------------------------------------------ the preview map

/// The colours a preview map is painted in (the realm's climate).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Palette {
    #[default]
    Green,
    Sand,
    Snow,
    Jungle,
    Ember,
    Dusk,
}

impl Palette {
    /// (beach, low, mid, high, rock, peak) srgb
    fn ramp(self) -> [[f32; 3]; 6] {
        match self {
            Palette::Green => [
                [214., 200., 158.],
                [112., 150., 82.],
                [128., 142., 86.],
                [138., 122., 96.],
                [150., 146., 140.],
                [236., 236., 236.],
            ],
            Palette::Sand => [
                [230., 210., 164.],
                [222., 188., 132.],
                [206., 160., 108.],
                [176., 126., 88.],
                [150., 116., 96.],
                [214., 196., 170.],
            ],
            Palette::Snow => [
                [198., 204., 210.],
                [232., 238., 244.],
                [214., 222., 232.],
                [190., 198., 210.],
                [146., 152., 164.],
                [250., 252., 255.],
            ],
            Palette::Jungle => [
                [228., 214., 168.],
                [58., 128., 62.],
                [66., 116., 56.],
                [98., 112., 70.],
                [122., 120., 108.],
                [220., 222., 220.],
            ],
            Palette::Ember => [
                [78., 68., 62.],
                [104., 74., 62.],
                [86., 62., 56.],
                [66., 52., 50.],
                [48., 42., 42.],
                [140., 70., 40.],
            ],
            Palette::Dusk => [
                [196., 182., 176.],
                [104., 126., 116.],
                [118., 116., 140.],
                [124., 106., 132.],
                [138., 128., 150.],
                [226., 220., 236.],
            ],
        }
    }
}

/// The land as a small shaded map (RGBA, `px`×`px`): sea by depth, land by height, hill shading,
/// and where players arrive (a white ring).
pub fn render(land: &Land, px: usize, palette: Palette) -> Vec<u8> {
    let n = land.n();
    let res = land.res as f32;
    let hmax = land
        .heights
        .iter()
        .copied()
        .fold(0.0f32, f32::max)
        .max(130.0 * (land.size() / 2048.0).powf(0.72));
    let ramp = palette.ramp();
    let stops = [0.0f32, 0.04, 0.3, 0.6, 0.85, 1.0];
    let light = bevy::math::Vec3::new(-0.55, 0.75, -0.4).normalize();
    // (relief reads on any size: shading on slopes per pixel, exaggerated)
    let mpp = land.size() / px as f32;
    let exag = 2.2;
    let mut out = vec![0u8; px * px * 4];
    for py in 0..px {
        for qx in 0..px {
            let gx = (qx as f32 + 0.5) / px as f32 * res;
            let gz = (py as f32 + 0.5) / px as f32 * res;
            let y = sample(&land.heights, n, gx, gz);
            let d = 0.75 * res / px as f32;
            let dx = (sample(&land.heights, n, gx + d, gz) - sample(&land.heights, n, gx - d, gz))
                / (2.0 * d * land.cell);
            let dz = (sample(&land.heights, n, gx, gz + d) - sample(&land.heights, n, gx, gz - d))
                / (2.0 * d * land.cell);
            let nrm = bevy::math::Vec3::new(-dx * exag, 1.0, -dz * exag).normalize();
            let shade = nrm.dot(light).max(0.0) / light.y;
            let wet = !land.wet.is_empty() && {
                let (i, j) = (
                    (gx.round() as usize).min(n - 1),
                    (gz.round() as usize).min(n - 1),
                );
                land.wet[j * n + i]
            };
            let c: [f32; 3] = if wet {
                [62., 128., 168.]
            } else if y <= 0.0 {
                let t = (-y / 24.0).clamp(0.0, 1.0).sqrt();
                let (a, b) = ([92., 176., 184.], [26., 58., 98.]);
                [
                    a[0] + (b[0] - a[0]) * t,
                    a[1] + (b[1] - a[1]) * t,
                    a[2] + (b[2] - a[2]) * t,
                ]
            } else {
                let t = if y < 1.6 {
                    0.0
                } else {
                    (y / hmax).clamp(0.0, 1.0).max(0.041)
                };
                let s = stops.iter().rposition(|v| *v <= t).unwrap_or(0).min(4);
                let f = ((t - stops[s]) / (stops[s + 1] - stops[s])).clamp(0.0, 1.0);
                let (a, b) = (ramp[s], ramp[s + 1]);
                let base = [
                    a[0] + (b[0] - a[0]) * f,
                    a[1] + (b[1] - a[1]) * f,
                    a[2] + (b[2] - a[2]) * f,
                ];
                let k = 0.5 + 0.55 * shade.min(1.4);
                [base[0] * k, base[1] * k, base[2] * k]
            };
            let o = (py * px + qx) * 4;
            out[o] = c[0].clamp(0.0, 255.0) as u8;
            out[o + 1] = c[1].clamp(0.0, 255.0) as u8;
            out[o + 2] = c[2].clamp(0.0, 255.0) as u8;
            out[o + 3] = 255;
        }
    }
    // where players arrive
    let a = (land.arrival + Vec2::splat(land.size() * 0.5)) / mpp;
    let r = (px as f32 / 48.0).max(2.2);
    for py in 0..px {
        for qx in 0..px {
            let d = Vec2::new(qx as f32 + 0.5, py as f32 + 0.5).distance(a);
            let o = (py * px + qx) * 4;
            if d < r * 0.55 {
                out[o..o + 3].copy_from_slice(&[40, 40, 48]);
            } else if d < r + 0.6 {
                out[o..o + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest connected region of `mask` (4-neighbours).
    fn largest(mask: &[bool], n: usize) -> Vec<usize> {
        let mut seen = vec![false; n * n];
        let mut best: Vec<usize> = vec![];
        for s in 0..n * n {
            if !mask[s] || seen[s] {
                continue;
            }
            let mut cells = vec![s];
            seen[s] = true;
            let mut q = 0;
            while q < cells.len() {
                let k = cells[q];
                q += 1;
                let (i, j) = (k % n, k / n);
                for (a, b) in [
                    (i.wrapping_sub(1), j),
                    (i + 1, j),
                    (i, j.wrapping_sub(1)),
                    (i, j + 1),
                ] {
                    if a < n && b < n && mask[b * n + a] && !seen[b * n + a] {
                        seen[b * n + a] = true;
                        cells.push(b * n + a);
                    }
                }
            }
            if cells.len() > best.len() {
                best = cells;
            }
        }
        best
    }

    /// The land (floor, for walled shapes) of a generated realm.
    fn region(land: &Land, shape: RealmShape) -> Vec<bool> {
        if shape.walled() {
            // the floor: the ground about as high as where players arrive
            let lo = land.height_at(land.arrival.x, land.arrival.y);
            land.heights.iter().map(|h| (*h - lo).abs() < 5.0).collect()
        } else {
            land.heights.iter().map(|h| *h > 0.0).collect()
        }
    }

    /// How far the region is from a circle: the spread of its outline's distance from its
    /// centroid (relative), and how stretched it is (ratio of its principal axes).
    fn roundness(cells: &[usize], mask: &[bool], n: usize) -> (f32, f32) {
        let c = cells.iter().fold(Vec2::ZERO, |a, k| {
            a + Vec2::new((k % n) as f32, (k / n) as f32)
        }) / cells.len() as f32;
        let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
        for k in cells {
            let d = Vec2::new((k % n) as f32, (k / n) as f32) - c;
            sxx += d.x * d.x;
            syy += d.y * d.y;
            sxy += d.x * d.y;
        }
        let (a, b, cc) = (
            sxx / cells.len() as f32,
            syy / cells.len() as f32,
            sxy / cells.len() as f32,
        );
        let tr = a + b;
        let det = a * b - cc * cc;
        let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
        let stretch = ((tr / 2.0 + disc) / (tr / 2.0 - disc).max(1e-3)).sqrt();
        let radii: Vec<f32> = (0..64)
            .map(|a| {
                let d = dir(a as f32 / 64.0 * TAU);
                let mut r = 0.0;
                loop {
                    let p = c + d * (r + 0.5);
                    if p.x < 0.0
                        || p.y < 0.0
                        || p.x >= n as f32 - 1.0
                        || p.y >= n as f32 - 1.0
                        || !mask[p.y.round() as usize * n + p.x.round() as usize]
                    {
                        break r;
                    }
                    r += 0.5;
                }
            })
            .collect();
        let mean = radii.iter().sum::<f32>() / 64.0;
        let var = radii.iter().map(|r| (r - mean).powi(2)).sum::<f32>() / 64.0;
        (var.sqrt() / mean.max(1e-3), stretch)
    }

    #[test]
    fn no_shape_reads_as_a_circle() {
        for shape in RealmShape::ALL {
            for seed in [1u32, 2, 3, 77, 1234] {
                let land = generate(shape, RealmSize::Medium, seed, Detail::Preview(128));
                let n = land.n();
                let mask = region(&land, shape);
                // (islands together: the chain and the twins as a whole)
                let main = if matches!(shape, RealmShape::TwinIsles | RealmShape::IslandChain) {
                    (0..n * n).filter(|k| mask[*k]).collect()
                } else {
                    largest(&mask, n)
                };
                let mut only = vec![false; n * n];
                for k in &main {
                    only[*k] = true;
                }
                let (spread, stretch) = roundness(&main, &only, n);
                println!(
                    "{:16} seed {seed:5}: spread {spread:.2} stretch {stretch:.2} land {:.2}",
                    shape.id(),
                    land.land_share
                );
                assert!(
                    spread > 0.09 || stretch > 1.3,
                    "{} seed {seed} reads as a circle: spread {spread:.2} stretch {stretch:.2}",
                    shape.id()
                );
            }
        }
    }

    #[test]
    fn every_realm_arrives_on_level_dry_ground_near_the_middle() {
        for shape in RealmShape::ALL {
            for (size, seed) in [(RealmSize::Cozy, 5u32), (RealmSize::Small, 11)] {
                let t0 = std::time::Instant::now();
                let land = generate(shape, size, seed, Detail::Full);
                let a = land.arrival;
                let y = land.height_at(a.x, a.y);
                let rough = (0..16)
                    .map(|k| dir(k as f32 / 16.0 * TAU) * 6.0)
                    .map(|d| (land.height_at(a.x + d.x, a.y + d.y) - y).abs())
                    .fold(0.0f32, f32::max);
                println!(
                    "{:16} {:6}: arrive {a:?} at {y:.1} m (±{rough:.2}), land {:.2}, {:.2} s",
                    shape.id(),
                    size.id(),
                    land.land_share,
                    t0.elapsed().as_secs_f32()
                );
                assert!(y > 2.0, "{} {}: arrives at {y:.1} m", shape.id(), size.id());
                assert!(
                    rough < 0.4,
                    "{} {}: arrival not level (±{rough:.2} m)",
                    shape.id(),
                    size.id()
                );
                assert!(
                    a.length() < land.size() * 0.3,
                    "{} {}: arrives {:.0} m from the middle",
                    shape.id(),
                    size.id(),
                    a.length()
                );
                assert_eq!(land.heights.len(), (size.grid().0 + 1).pow(2));
            }
        }
    }

    #[test]
    fn shuffling_gives_different_land() {
        for shape in [
            RealmShape::LongIsle,
            RealmShape::EnclosedValley,
            RealmShape::IslandChain,
        ] {
            let masks: Vec<Vec<bool>> = [3u32, 4, 5]
                .iter()
                .map(|s| {
                    region(
                        &generate(shape, RealmSize::Medium, *s, Detail::Preview(96)),
                        shape,
                    )
                })
                .collect();
            for a in 0..3 {
                for b in a + 1..3 {
                    let inter = masks[a]
                        .iter()
                        .zip(&masks[b])
                        .filter(|(x, y)| **x && **y)
                        .count() as f32;
                    let uni = masks[a]
                        .iter()
                        .zip(&masks[b])
                        .filter(|(x, y)| **x || **y)
                        .count() as f32;
                    assert!(
                        inter / uni < 0.8,
                        "{}: seeds look alike (IoU {:.2})",
                        shape.id(),
                        inter / uni
                    );
                }
            }
        }
    }

    /// Rain off the walls ends on a walled realm's floor: no straight rills run across it (the
    /// floor's fine relief, its heights less their local mean, stays small).
    #[test]
    fn walled_floors_have_no_erosion_rills() {
        for (shape, size, seed) in [
            (RealmShape::EnclosedValley, RealmSize::Small, 1u32),
            (RealmShape::EnclosedValley, RealmSize::Small, 3),
            (RealmShape::EnclosedValley, RealmSize::Medium, 9),
            (RealmShape::Canyon, RealmSize::Small, 2),
        ] {
            let land = generate(shape, size, seed, Detail::Full);
            let n = land.n();
            let floor = land.height_at(land.arrival.x, land.arrival.y);
            let on: Vec<bool> = land
                .heights
                .iter()
                .map(|h| (*h - floor).abs() < 2.5)
                .collect();
            let r = 4usize;
            let mut fine = vec![];
            for j in r..n - r {
                for i in r..n - r {
                    // (well inside the floor)
                    if !(0..=2 * r).all(|d| on[(j - r + d) * n + i] && on[j * n + i - r + d]) {
                        continue;
                    }
                    let (mut sum, mut cnt) = (0.0, 0.0);
                    for dj in 0..=2 * r {
                        for di in 0..=2 * r {
                            sum += land.heights[(j - r + dj) * n + i - r + di];
                            cnt += 1.0;
                        }
                    }
                    fine.push((land.heights[j * n + i] - sum / cnt).abs());
                }
            }
            fine.sort_by(|a, b| a.total_cmp(b));
            let p99 = fine[fine.len() * 99 / 100];
            println!(
                "{} {} seed {seed}: floor fine relief p99 {p99:.3} m over {} points",
                shape.id(),
                size.id(),
                fine.len()
            );
            assert!(
                p99 < 0.35,
                "{} {} seed {seed}: rills across the floor (p99 {p99:.2} m)",
                shape.id(),
                size.id()
            );
        }
    }

    #[test]
    fn the_valley_is_walled_round_a_wide_level_floor() {
        for (size, seed) in [
            (RealmSize::Cozy, 3u32),
            (RealmSize::Medium, 9),
            (RealmSize::Cozy, 21),
        ] {
            let land = generate(RealmShape::EnclosedValley, size, seed, Detail::Full);
            let n = land.n();
            let a = land.arrival;
            let floor = land.height_at(a.x, a.y);
            // level floor: gentle slopes, close to the arrival's height
            let mut flat = 0;
            for j in 1..n - 1 {
                for i in 1..n - 1 {
                    let k = j * n + i;
                    let gx = (land.heights[k + 1] - land.heights[k - 1]) / (2.0 * land.cell);
                    let gz = (land.heights[k + n] - land.heights[k - n]) / (2.0 * land.cell);
                    if (gx * gx + gz * gz).sqrt() < 0.14 && (land.heights[k] - floor).abs() < 6.0 {
                        flat += 1;
                    }
                }
            }
            let share = flat as f32 / ((n - 2) * (n - 2)) as f32;
            // walls all round: every way out from the arrival climbs well above the floor
            let lowest_wall = (0..48)
                .map(|k| {
                    let d = dir(k as f32 / 48.0 * TAU);
                    let mut top = f32::MIN;
                    let mut r = 0.0;
                    while (a + d * r).abs().max_element() < land.size() * 0.5 - land.cell {
                        top = top.max(land.height_at(a.x + d.x * r, a.y + d.y * r));
                        r += land.cell;
                    }
                    top - floor
                })
                .fold(f32::MAX, f32::min);
            // the basin: everything below half way up the walls
            let basin = land
                .heights
                .iter()
                .filter(|h| **h > 0.0 && **h < floor + lowest_wall * 0.5)
                .count() as f32
                / (n * n) as f32;
            println!(
                "valley {} seed {seed}: level floor {:.0}% of the map, {:.0}% of the basin; lowest wall {lowest_wall:.0} m",
                size.id(),
                share * 100.0,
                share / basin * 100.0
            );
            assert!(
                share > 0.27,
                "level floor only {:.0}% of the map",
                share * 100.0
            );
            assert!(
                share / basin > 0.55,
                "level floor only {:.0}% of the basin",
                share / basin * 100.0
            );
            assert!(lowest_wall > 15.0, "a gap in the walls: {lowest_wall:.0} m");
        }
    }

    #[test]
    fn previews_are_quick_and_paint_the_map() {
        let t0 = std::time::Instant::now();
        for shape in RealmShape::ALL {
            let land = generate(shape, RealmSize::Large, 42, Detail::Preview(128));
            let img = render(&land, 160, Palette::Green);
            assert_eq!(img.len(), 160 * 160 * 4);
            // land and sea both show (walled shapes too: their rim)
            assert!(
                land.land_share > 0.06 && land.land_share < 0.97,
                "{}: land {:.2}",
                shape.id(),
                land.land_share
            );
        }
        let per = t0.elapsed().as_secs_f32() / RealmShape::ALL.len() as f32;
        println!("preview: {per:.3} s each");
        assert!(per < 1.0, "previews take {per:.2} s");
    }

    /// Contact sheets of every shape (rows) over a few seeds (columns): `REALM_SHEET=<dir>
    /// cargo test --bin world_builder realm_land::tests::contact_sheet -- --ignored`.
    #[test]
    #[ignore]
    fn contact_sheet() {
        let Ok(dir) = std::env::var("REALM_SHEET") else {
            return;
        };
        let detail = || {
            if std::env::var("REALM_FULL").is_ok() {
                Detail::Full
            } else {
                Detail::Preview(160)
            }
        };
        for size in [RealmSize::Cozy, RealmSize::Medium, RealmSize::Large] {
            let (px, seeds) = (180usize, [1u32, 2, 3, 77, 1234, 9001]);
            let (w, hgt) = (px * seeds.len(), px * RealmShape::ALL.len());
            let mut img = vec![0u8; w * hgt * 4];
            for (r, shape) in RealmShape::ALL.iter().enumerate() {
                for (c, seed) in seeds.iter().enumerate() {
                    let land = generate(*shape, size, *seed, detail());
                    let tile = render(&land, px, Palette::Green);
                    for y in 0..px {
                        let o = ((r * px + y) * w + c * px) * 4;
                        img[o..o + px * 4].copy_from_slice(&tile[y * px * 4..(y + 1) * px * 4]);
                    }
                }
            }
            image::save_buffer(
                format!("{dir}/sheet_{}.png", size.id()),
                &img,
                w as u32,
                hgt as u32,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        }
    }

    #[test]
    fn ids_round_trip() {
        for s in RealmShape::ALL {
            assert_eq!(RealmShape::from_id(s.id()), Some(s));
            assert_eq!(RealmShape::from_id(s.name()), Some(s));
        }
        for s in RealmSize::ALL {
            assert_eq!(RealmSize::from_id(s.id()), Some(s));
        }
        assert_eq!(RealmSize::from_id("0.5 km"), Some(RealmSize::Cozy));
        assert_eq!(
            RealmShape::from_id("valley"),
            Some(RealmShape::EnclosedValley)
        );
    }
}
