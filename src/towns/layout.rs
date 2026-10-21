//! The town layout generator: pure and deterministic per seed. Given an outline in the town's
//! own frame (its long side along x), a size, a style, the buildings to place and the ground
//! (`Site`), it composes the settlement the way hand-made MMO towns are (Goldshire, Riverwood,
//! Whiterun's market): few buildings, well apart, each standing where it has a reason to.
//!
//! - **The main road** runs through along the long side, curving gently with the ground (`route`:
//!   a shortest path over sideways offsets that keeps the grade gentle, off water and steep
//!   ground). Settlements grow along it.
//! - **The centre** sits beside the road on its flattest stretch (the side with a gentle rise
//!   behind it preferred): a cobbled square (towns, cities) or a green (hamlets, villages) with a
//!   landmark in the middle (a well; a statue or a rotunda in towns; the photoreal bell
//!   pavilion), benches round it, a few market stalls and a cart at its edge (never filling it),
//!   a notice board and lamps.
//! - **The key buildings frame it and face it**: the town hall (a village's chapel) at the end
//!   of the axis across the square, or beside it when that ground is clearly higher; the inn,
//!   the biggest, on a flank by the road; the chapel and the store on the other flank or across
//!   the road; a city's bank and best shops on the corners.
//! - **Lanes, not grids**: one to four lanes branch off the main road at varied, gentle angles
//!   and bend with the ground. Homes and the other shops take lots along the road and the lanes,
//!   nearest the centre first: each faces its street, its setback and angle varied a little, kept
//!   2.5–3.5 × a house's width from the next (wider toward the edge), never straight across the
//!   street from another, on the flattest spot of its lot. The smithy stands where the road comes
//!   in; farmsteads (farmhouse, barn, haystacks, a field-edge fence) on the outskirts; the
//!   windmill or watchtower on the highest ground near the edge, a track up to it.
//! - **Dressing**, restrained: lamps along the road near the centre and round the square, short
//!   garden fences round some house plots, flower boxes, barrels and a cart by some shops, and
//!   trees on the open ground between the buildings (when the land has trees to give).
//! - **A palisade** only when asked: wall sections along the outline at their exact 10 m length,
//!   gates where the streets leave.

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use bevy::prelude::*;

use super::kinds::{Catalog, Mix, Role, Style, TYPES, TownSize, Want};
use crate::noise::{Rng, hash_u32};
use crate::roads::RoadKind;
use crate::setpieces::poly_sdf;
use crate::terrain::layers::Layer;

/// The ground the town is laid out on, in the town's frame.
pub trait Site {
    /// Ground height (m).
    fn height(&self, p: Vec2) -> f32;
    /// Under water or off the world: nothing is built there.
    fn wet(&self, p: Vec2) -> bool;
}

/// Level dry ground (tests).
#[cfg(test)]
pub struct Flat;

#[cfg(test)]
impl Site for Flat {
    fn height(&self, _: Vec2) -> f32 {
        0.0
    }
    fn wet(&self, _: Vec2) -> bool {
        false
    }
}

// ------------------------------------------------------------------ geometry

/// Turns (x, z) as `Quat::from_rotation_y(a)` does.
pub fn rot2(v: Vec2, a: f32) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(v.x * c + v.y * s, -v.x * s + v.y * c)
}

/// The yaw that turns a model's front (+Z) toward `d`.
pub fn yaw_facing(d: Vec2) -> f32 {
    d.x.atan2(d.y)
}

/// The yaw that turns a model's +X along `d`.
pub fn yaw_along(d: Vec2) -> f32 {
    (-d.y).atan2(d.x)
}

/// An oriented rectangle: centre, its local x axis and half sizes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obb {
    pub c: Vec2,
    pub ax: Vec2,
    pub half: Vec2,
}

impl Obb {
    pub fn az(&self) -> Vec2 {
        Vec2::new(-self.ax.y, self.ax.x)
    }
    pub fn local(&self, p: Vec2) -> Vec2 {
        let d = p - self.c;
        Vec2::new(d.dot(self.ax), d.dot(self.az()))
    }
    /// Signed distance to the edge (negative inside).
    pub fn sdist(&self, p: Vec2) -> f32 {
        let q = self.local(p).abs() - self.half;
        q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0)
    }
    pub fn corners(&self) -> [Vec2; 4] {
        let (a, b) = (self.ax * self.half.x, self.az() * self.half.y);
        [
            self.c - a - b,
            self.c + a - b,
            self.c + a + b,
            self.c - a + b,
        ]
    }
    pub fn grown(&self, m: f32) -> Obb {
        Obb {
            half: (self.half + Vec2::splat(m)).max(Vec2::ZERO),
            ..*self
        }
    }
    pub fn radius(&self) -> f32 {
        self.half.length()
    }
    /// Whether two rectangles come closer than `gap` (separating axes).
    pub fn overlaps(&self, o: &Obb, gap: f32) -> bool {
        let d = o.c - self.c;
        if d.length() > self.radius() + o.radius() + gap {
            return false;
        }
        for axis in [self.ax, self.az(), o.ax, o.az()] {
            let r1 =
                self.half.x * self.ax.dot(axis).abs() + self.half.y * self.az().dot(axis).abs();
            let r2 = o.half.x * o.ax.dot(axis).abs() + o.half.y * o.az().dot(axis).abs();
            if d.dot(axis).abs() > r1 + r2 + gap {
                return false;
            }
        }
        true
    }
    /// The gap between two rectangles (0 when they touch or overlap), to within 0.1 m.
    #[cfg(test)]
    pub fn gap_to(&self, o: &Obb) -> f32 {
        if self.overlaps(o, 0.0) {
            return 0.0;
        }
        let (mut lo, mut hi) = (0.0f32, (self.c.distance(o.c)).max(0.1));
        while hi - lo > 0.1 {
            let m = (lo + hi) * 0.5;
            if self.overlaps(o, m) {
                hi = m;
            } else {
                lo = m;
            }
        }
        lo
    }
}

/// An open polyline with its arc length.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub pts: Vec<Vec2>,
    cum: Vec<f32>,
}

impl Line {
    pub fn new(pts: Vec<Vec2>) -> Line {
        let mut cum = vec![0.0];
        for w in pts.windows(2) {
            cum.push(cum[cum.len() - 1] + w[0].distance(w[1]));
        }
        Line { pts, cum }
    }
    pub fn len(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }
    fn seg(&self, s: f32) -> (usize, f32) {
        let n = self.pts.len();
        if n < 2 {
            return (0, 0.0);
        }
        let s = s.clamp(0.0, self.len());
        let i = self.cum.partition_point(|c| *c <= s).clamp(1, n - 1) - 1;
        let l = (self.cum[i + 1] - self.cum[i]).max(1e-6);
        (i, ((s - self.cum[i]) / l).clamp(0.0, 1.0))
    }
    /// The point `s` m along (clamped to the ends).
    pub fn at(&self, s: f32) -> Vec2 {
        if self.pts.len() < 2 {
            return self.pts.first().copied().unwrap_or_default();
        }
        let (i, t) = self.seg(s);
        self.pts[i].lerp(self.pts[i + 1], t)
    }
    /// The direction at `s` (over a couple of metres, so corners turn smoothly; past an end,
    /// the end's).
    pub fn tangent(&self, s: f32) -> Vec2 {
        let l = self.len();
        let s = s.clamp(0.0, l);
        let (a, b) = (
            (s - 1.5).max(0.0).min(l - 3.0).max(0.0),
            (s + 1.5).min(l).max(3.0f32.min(l)),
        );
        (self.at(b) - self.at(a)).normalize_or(Vec2::X)
    }
    /// The nearest point: (arc length, distance).
    pub fn nearest(&self, p: Vec2) -> (f32, f32) {
        let mut best = (0.0, f32::MAX);
        for i in 0..self.pts.len().saturating_sub(1) {
            let (a, b) = (self.pts[i], self.pts[i + 1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = p.distance(a + ab * t);
            if d < best.1 {
                best = (self.cum[i] + t * ab.length(), d);
            }
        }
        best
    }
    /// The part from `s0` to `s1` m.
    pub fn sub(&self, s0: f32, s1: f32) -> Line {
        let (s0, s1) = (s0.max(0.0), s1.min(self.len()));
        let mut pts = vec![self.at(s0)];
        for (p, c) in self.pts.iter().zip(&self.cum) {
            if *c > s0 + 0.5 && *c < s1 - 0.5 {
                pts.push(*p);
            }
        }
        pts.push(self.at(s1));
        Line::new(pts)
    }
    /// Points every `step` m (ends included).
    pub fn samples(&self, step: f32) -> Vec<Vec2> {
        let n = (self.len() / step).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| self.at(self.len() * i as f32 / n as f32))
            .collect()
    }
}

pub fn poly_area(poly: &[Vec2]) -> f32 {
    crate::water::lakes::signed_area(poly).abs()
}

fn bounds(poly: &[Vec2]) -> (Vec2, Vec2) {
    poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(a, b), q| (a.min(*q), b.max(*q)),
    )
}

/// The inside point farthest from the outline (near the middle when many are as deep).
fn deepest(poly: &[Vec2]) -> Vec2 {
    let (lo, hi) = bounds(poly);
    let mid = (lo + hi) * 0.5;
    let step = ((hi - lo).max_element() / 48.0).max(0.5);
    let mut best = (mid, f32::MIN);
    let mut z = lo.y;
    while z <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            let q = Vec2::new(x, z);
            let d = -poly_sdf(poly, q);
            let score = d - 0.08 * q.distance(mid);
            if d > 0.0 && score > best.1 {
                best = (q, score);
            }
            x += step;
        }
        z += step;
    }
    best.0
}

/// How far from `from` along `dir` the outline is (m, up to `max`).
fn reach(poly: &[Vec2], from: Vec2, dir: Vec2, max: f32) -> f32 {
    let mut s = 0.0;
    while s < max {
        if poly_sdf(poly, from + dir * (s + 1.0)) > 0.0 {
            return s;
        }
        s += 1.0;
    }
    max
}

/// Where a line across x = `x` is inside the outline: the stretch (z from, to) holding or
/// nearest `near`.
fn chord(poly: &[Vec2], x: f32, near: f32) -> Option<(f32, f32)> {
    let n = poly.len();
    let mut zs: Vec<f32> = vec![];
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.x <= x) != (b.x <= x) {
            zs.push(a.y + (x - a.x) / (b.x - a.x) * (b.y - a.y));
        }
    }
    zs.sort_by(f32::total_cmp);
    zs.chunks_exact(2).map(|c| (c[0], c[1])).min_by(|p, q| {
        let d = |c: &(f32, f32)| {
            if near < c.0 {
                c.0 - near
            } else if near > c.1 {
                near - c.1
            } else {
                0.0
            }
        };
        d(p).total_cmp(&d(q))
    })
}

fn slope(site: &dyn Site, p: Vec2) -> f32 {
    let e = 1.5;
    let gx = site.height(p + Vec2::new(e, 0.0)) - site.height(p - Vec2::new(e, 0.0));
    let gz = site.height(p + Vec2::new(0.0, e)) - site.height(p - Vec2::new(0.0, e));
    Vec2::new(gx, gz).length() / (2.0 * e)
}

/// A street from `a` heading `dir`, `len` m long, through stations `step` apart that may each
/// move sideways (perpendicular to `dir`, left positive) within `lat(s)`. Picks the line (a
/// shortest path over the offsets) that keeps the grade gentle and the bends easy, stays off
/// water and steep ground and near `pref(s)`, then smooths it.
fn route(
    site: &dyn Site,
    a: Vec2,
    dir: Vec2,
    len: f32,
    step: f32,
    lat: &dyn Fn(f32) -> (f32, f32),
    pref: &dyn Fn(f32) -> f32,
) -> Vec<Vec2> {
    let n = (len / step).ceil().max(1.0) as usize;
    let step = len / n as f32;
    let side = dir.perp();
    const K: usize = 9;
    // per station: (offset, point, height, cost)
    let mut st: Vec<Vec<(f32, Vec2, f32, f32)>> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let s = i as f32 * step;
        let (lo, hi) = lat(s);
        let pr = pref(s).clamp(lo, hi);
        let mut offs: Vec<f32> = if hi - lo < 0.05 {
            vec![(lo + hi) * 0.5]
        } else {
            (0..K)
                .map(|k| lo + (hi - lo) * k as f32 / (K - 1) as f32)
                .chain([pr])
                .collect()
        };
        offs.dedup_by(|x, y| (*x - *y).abs() < 0.01);
        st.push(
            offs.into_iter()
                .map(|o| {
                    let p = a + dir * s + side * o;
                    let g = slope(site, p);
                    let wet = if site.wet(p) { 400.0 } else { 0.0 };
                    (
                        o,
                        p,
                        site.height(p),
                        wet + 30.0 * g * g + 0.08 * (o - pr) * (o - pr),
                    )
                })
                .collect(),
        );
    }
    // shortest path over the stations
    let mut cost: Vec<Vec<f32>> = st.iter().map(|v| vec![f32::MAX; v.len()]).collect();
    let mut from: Vec<Vec<usize>> = st.iter().map(|v| vec![0; v.len()]).collect();
    for (k, c) in st[0].iter().enumerate() {
        cost[0][k] = c.3;
    }
    for i in 1..=n {
        for k in 0..st[i].len() {
            let (o, _, h, c) = st[i][k];
            for j in 0..st[i - 1].len() {
                let (po, _, ph, _) = st[i - 1][j];
                let grade = (h - ph) / step;
                let bend = (o - po) / step;
                let v = cost[i - 1][j] + c + 80.0 * grade * grade + 8.0 * bend * bend;
                if v < cost[i][k] {
                    cost[i][k] = v;
                    from[i][k] = j;
                }
            }
        }
    }
    let mut k = (0..st[n].len())
        .min_by(|x, y| cost[n][*x].total_cmp(&cost[n][*y]))
        .unwrap_or(0);
    let mut offs = vec![0.0; n + 1];
    for i in (0..=n).rev() {
        offs[i] = st[i][k].0;
        k = from[i][k];
    }
    // smooth the offsets (stations held in place stay put)
    for _ in 0..2 {
        let prev = offs.clone();
        for i in 1..n {
            let (lo, hi) = lat(i as f32 * step);
            if hi - lo >= 0.05 {
                offs[i] = (prev[i - 1] * 0.25 + prev[i] * 0.5 + prev[i + 1] * 0.25).clamp(lo, hi);
            }
        }
    }
    (0..=n)
        .map(|i| a + dir * (i as f32 * step) + side * offs[i])
        .collect()
}

/// A building's model frame with the middle of its front at `front`, facing `f` (turned a little
/// further by `turn`): (origin, yaw, rectangle).
fn at_front(b: (Vec3, Vec3), front: Vec2, f: Vec2, turn: f32) -> (Vec2, f32, Obb) {
    let yaw = yaw_facing(f) + turn;
    let (ax, az) = (rot2(Vec2::X, yaw), rot2(Vec2::Y, yaw));
    let (bx0, bx1, bz0, bz1) = (b.0.x, b.1.x, b.0.z, b.1.z);
    let origin = front - ax * ((bx0 + bx1) * 0.5) - az * bz1;
    let obb = Obb {
        c: origin + ax * ((bx0 + bx1) * 0.5) + az * ((bz0 + bz1) * 0.5),
        ax,
        half: Vec2::new((bx1 - bx0) * 0.5, (bz1 - bz0) * 0.5),
    };
    (origin, yaw, obb)
}

/// The same with its footprint centred on `c`.
fn at_centre(b: (Vec3, Vec3), c: Vec2, f: Vec2, scale: f32) -> (Vec2, f32, Obb) {
    let yaw = yaw_facing(f);
    let (ax, az) = (rot2(Vec2::X, yaw), rot2(Vec2::Y, yaw));
    let mid = Vec2::new(b.0.x + b.1.x, b.0.z + b.1.z) * 0.5 * scale;
    let origin = c - ax * mid.x - az * mid.y;
    (
        origin,
        yaw,
        Obb {
            c,
            ax,
            half: Vec2::new(b.1.x - b.0.x, b.1.z - b.0.z) * 0.5 * scale,
        },
    )
}

/// A deterministic value in [0, 1) for a seed and three numbers.
fn h01(seed: u32, a: u32, b: u32, c: u32) -> f32 {
    let x = hash_u32(
        seed.wrapping_mul(0x9E37_79B9)
            ^ hash_u32(a ^ hash_u32(b.wrapping_mul(0x85EB_CA6B) ^ c.wrapping_mul(0xC2B2_AE35))),
    );
    (x >> 8) as f32 / (1u32 << 24) as f32
}

// ------------------------------------------------------------------ output

/// What a piece is, for placing it: buildings level the ground under them, walls and fences
/// follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceRole {
    Building,
    Prop,
    Wall,
}

/// One object of the town, in the town's frame (models face +Z as modelled).
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub asset: String,
    pub at: Vec2,
    pub yaw: f32,
    pub scale: Vec3,
    /// a wall or fence run this long along its own x: it tilts to follow the ground
    pub span: f32,
    pub role: PieceRole,
    /// buildings: their type (`kinds::TYPES`)
    pub ty: Option<usize>,
    /// buildings: the ground they cover, and how far back from their street they stand (m)
    pub obb: Option<Obb>,
    pub setback: f32,
}

impl Piece {
    fn prop(asset: &str, at: Vec2, yaw: f32) -> Piece {
        Piece {
            asset: asset.to_string(),
            at,
            yaw,
            scale: Vec3::ONE,
            span: 0.0,
            role: PieceRole::Prop,
            ty: None,
            obb: None,
            setback: 0.0,
        }
    }
    fn scaled(self, k: f32) -> Piece {
        Piece {
            scale: Vec3::splat(k),
            ..self
        }
    }
}

/// A street: a road of this kind along the points.
#[derive(Clone, Debug, PartialEq)]
pub struct Street {
    pub kind: RoadKind,
    pub width: f32,
    pub line: Line,
    pub name: &'static str,
}

/// Ground paint over a rounded rectangle (`obb` grown by `round`, soft ragged edge).
#[derive(Clone, Debug, PartialEq)]
pub struct PaintArea {
    pub obb: Obb,
    pub round: f32,
    pub layer: Layer,
    pub patches: Option<Layer>,
    pub strength: f32,
}

impl PaintArea {
    pub fn sdf(&self, p: Vec2) -> f32 {
        self.obb.sdist(p) - self.round
    }
}

/// A laid-out town (in its frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Town {
    pub streets: Vec<Street>,
    pub pieces: Vec<Piece>,
    pub paint: Vec<PaintArea>,
    /// where nature is cleared (trees a few metres further out)
    pub clear: Vec<Obb>,
    /// the square (towns) or the green (villages): its outer rectangle
    pub square: Option<Obb>,
    /// the model standing in the middle of it
    pub landmark: Option<String>,
    /// buildings placed per type name, and the square's stalls and well
    pub counts: BTreeMap<String, usize>,
    pub notes: Vec<String>,
}

impl Town {
    pub fn buildings(&self) -> impl Iterator<Item = &Piece> {
        self.pieces
            .iter()
            .filter(|p| p.role == PieceRole::Building && p.ty.is_some())
    }
}

/// What to lay out.
pub struct Spec<'a> {
    pub size: TownSize,
    pub style: Style,
    pub walls: bool,
    pub mix: &'a Mix,
    pub seed: u32,
    /// the streets run on past the outline (the road leads out of the village); else they stop
    /// just past it (filling a selection, whose edge fades the ground back)
    pub lead_out: bool,
    /// buildings already standing there, and points of roads already crossing it (with their
    /// half widths): the town builds round them
    pub obstacles: &'a [Obb],
    pub keep_off: &'a [(Vec2, f32)],
    /// trees that suit the land there (library ids), planted between the buildings; empty: none
    pub trees: &'a [String],
}

// ------------------------------------------------------------------ props per style

/// The model in the middle of the square or the green.
#[derive(Clone)]
struct Landmark {
    asset: String,
    /// the room it takes round its centre (m)
    room: f32,
    scale: f32,
    /// a structure (levels the ground under it) rather than a prop
    structure: bool,
    name: &'static str,
}

/// The props a style dresses a town with (only those this account can place).
struct Kit {
    lamp: Option<String>,
    landmark: Option<Landmark>,
    /// a well beside a statue or a rotunda
    well: Option<String>,
    bench: Option<String>,
    notice: Option<String>,
    signpost: Option<String>,
    stall: Option<String>,
    carts: Vec<String>,
    goods: Vec<String>,
    groups: Vec<Vec<String>>,
    yard: Vec<Vec<String>>,
    smithy: Vec<String>,
    garden_fence: Option<(String, f32)>,
    flowers: Vec<String>,
    farm: Vec<Vec<String>>,
    hay: Option<String>,
    field_fence: Option<(String, f32)>,
    palisade: Option<(String, String)>,
}

impl Kit {
    fn of(style: Style, size: TownSize, cat: &Catalog) -> Kit {
        let one = |id: &str| cat.has(id).then(|| id.to_string());
        let many = |ids: &[&str]| {
            ids.iter()
                .filter(|i| cat.has(i))
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        };
        let sets = |sets: &[&[&str]]| {
            sets.iter()
                .map(|s| many(s))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        };
        let lm = |asset: &str, room: f32, scale: f32, structure: bool, name: &'static str| {
            one(asset).map(|asset| Landmark {
                asset,
                room,
                scale,
                structure,
                name,
            })
        };
        if style == Style::Photoreal {
            return Kit {
                lamp: one("ph:street_lamp_01"),
                landmark: lm("sb:bell_pavilion", 4.6, 1.0, true, "Bell pavilion"),
                well: None,
                bench: one("ph:painted_wooden_bench"),
                notice: one("ph:standing_chalkboard_01"),
                signpost: None,
                stall: None,
                carts: vec![],
                goods: many(&["sb:flour_sack", "sb:open_crate", "ph:wooden_crate_02"]),
                groups: sets(&[
                    &["sb:barrel", "sb:barrel", "sb:open_crate"],
                    &["sb:flour_sack", "sb:flour_sack", "ph:wooden_crate_01"],
                    &["ph:wine_barrel_01", "ph:wooden_bucket_01"],
                    &["ph:wooden_military_crate", "sb:flour_sack"],
                ]),
                yard: sets(&[
                    &["ph:wooden_picnic_table"],
                    &["sb:barrel", "ph:wine_barrel_01"],
                ]),
                smithy: many(&["sb:chopping_block_axe", "sb:barrel", "sb:log_pile"]),
                garden_fence: one("sb:fence_short").map(|f| (f, 2.7)),
                flowers: vec![],
                farm: sets(&[
                    &["sb:log_pile", "sb:chopping_block_axe"],
                    &["sb:log_pile"],
                    &["ph:wooden_barrels_01"],
                ]),
                hay: None,
                field_fence: one("sb:fence_two_bay").map(|f| (f, 4.3)),
                palisade: None,
            };
        }
        let well = || lm("hp:well", 2.0, 1.0, false, "Well");
        let statue = || lm("hp:winged_statue", 2.2, 1.6, false, "Statue");
        let landmark = match size {
            TownSize::Hamlet | TownSize::Village => well(),
            TownSize::Town => statue().or_else(well),
            TownSize::City => lm("hp:moon_shrine", 4.6, 0.85, true, "Rotunda")
                .or_else(statue)
                .or_else(well),
        };
        Kit {
            lamp: one("hp:lamp_post"),
            well: one("hp:well").filter(|w| landmark.as_ref().is_some_and(|l| &l.asset != w)),
            landmark,
            bench: one("hp:bench"),
            notice: one("hp:notice_board"),
            signpost: one("hp:signpost"),
            stall: one("hp:market_stall"),
            carts: many(&["hp:wagon", "hp:hand_cart"]),
            goods: many(&[
                "hp:apple_crate",
                "hp:cabbage_crate",
                "hp:sacks",
                "hp:barrel",
                "hp:crate",
                "hp:bucket",
            ]),
            groups: sets(&[
                &["hp:barrel", "hp:barrel_squat", "hp:crate"],
                &["hp:crate", "hp:crate_open", "hp:sacks"],
                &["hp:sacks", "hp:barrel"],
                &["hp:barrel", "hp:bucket"],
            ]),
            yard: sets(&[
                &["hp:table", "hp:chair", "hp:chair"],
                &["hp:barrel", "hp:barrel", "hp:barrel_squat"],
            ]),
            smithy: many(&["hp:anvil", "hp:water_trough", "hp:barrel"]),
            garden_fence: one("hp:picket_fence").map(|f| (f, 2.2)),
            flowers: many(&["hp:flower_box_red", "hp:flower_box_violet"]),
            farm: sets(&[
                &["hp:wheelbarrow"],
                &["hp:water_trough", "hp:bucket"],
                &["hp:firewood"],
                &["hp:hand_cart", "hp:sacks"],
            ]),
            hay: one("hp:haystack"),
            field_fence: one("hp:fence").map(|f| (f, 2.5)),
            palisade: match (one("hp:palisade_wall"), one("hp:palisade_gate")) {
                (Some(w), Some(g)) => Some((w, g)),
                _ => None,
            },
        }
    }
}

/// About how far a prop reaches from its centre (m).
fn reach_of(asset: &str) -> f32 {
    let a = asset.split(':').nth(1).unwrap_or(asset);
    match a {
        "market_stall" => 2.3,
        "wagon" | "wooden_barrels_01" | "wooden_picnic_table" => 1.8,
        "hand_cart" | "haystack" | "water_trough" | "table" | "bench" | "wheelbarrow" | "well" => {
            1.1
        }
        "notice_board" | "signpost" | "winged_statue" => 1.2,
        _ => 0.55,
    }
}

// ------------------------------------------------------------------ the generator

/// How a building finds its place (the square's frame aside).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Plan {
    /// a lot along a street, nearest the centre first (homes, shops)
    Lot,
    /// along the main road where it comes in (the smithy)
    Entry,
    /// on the outskirts, a farmstead's buildings together
    Farm,
    /// on the highest ground near the edge (the windmill, the watchtower)
    Rise,
}

fn plan_of(ty: usize) -> Plan {
    let t = &TYPES[ty];
    match t.id {
        "windmill" | "watchtower" => Plan::Rise,
        "blacksmith" => Plan::Entry,
        _ if t.role == Role::Farm => Plan::Farm,
        _ => Plan::Lot,
    }
}

/// What stands on a piece of built ground: how far others keep from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Other,
    /// round the square
    Frame,
    /// a farmstead's buildings stand close together
    Farm,
    /// the windmill or tower stands alone
    Rise,
    /// already standing there
    Obstacle,
}

/// How far apart two buildings keep (m): `gap` for most, less round the square and within a
/// farmstead.
fn need_gap(a: Spot, b: Spot, gap: f32) -> f32 {
    use Spot::*;
    match (a, b) {
        (Obstacle, _) | (_, Obstacle) => 2.0,
        (Farm, Farm) => 4.0,
        (Frame, Frame) => 3.5,
        (Rise, _) | (_, Rise) => gap.max(12.0),
        _ => gap,
    }
}

/// The centre: the square (or the green) beside the main road.
#[derive(Clone, Copy, Debug)]
struct Core {
    /// how far along the main road it lies
    s: f32,
    /// along the road, and away from it into the square
    t: Vec2,
    n: Vec2,
    c: Vec2,
    /// half its size (along the road, across) and its corners' rounding
    half: Vec2,
    round: f32,
}

/// The places round the square a key building can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// the far side, at the end of the axis across the square
    Back,
    Left,
    Right,
    BackLeft,
    BackRight,
    /// across the road from the square
    Across,
    AcrossLeft,
    AcrossRight,
}

/// A building lining a street: which street, how far along, which side.
struct Fronting {
    street: usize,
    s: f32,
    side: f32,
}

/// A place a building could take on a lot.
struct Cand {
    origin: Vec2,
    yaw: f32,
    obb: Obb,
    setback: f32,
    street: usize,
    s: f32,
    side: f32,
    score: f32,
}

struct Gen<'a> {
    poly: &'a [Vec2],
    site: &'a dyn Site,
    cat: &'a Catalog,
    spec: &'a Spec<'a>,
    kit: Kit,
    rng: Rng,
    seed: u32,
    area: f32,
    size: TownSize,
    town: bool,
    /// how far streets run on past the outline (m)
    ext: f32,
    /// how far buildings keep inside the outline (m)
    edge: f32,
    centre: Vec2,
    /// how far from the square the settlement reaches (m)
    reach: f32,
    /// how far from the square the smithy stands, along the road in (m)
    entry: f32,
    streets: Vec<Street>,
    /// street points every metre, their half width and street
    street_pts: Vec<(Vec2, f32, usize)>,
    core: Option<Core>,
    square: Option<Obb>,
    pieces: Vec<Piece>,
    /// ground taken by buildings
    built: Vec<(Obb, Spot)>,
    fronts: Vec<Fronting>,
    /// (street, from, to): the stretch of each street a building lines
    lined: Vec<(usize, f32, f32)>,
    /// ground taken by props (centre, reach)
    props: Vec<(Vec2, f32)>,
    trees: Vec<Vec2>,
    /// the windmill's (or tower's) door and the street point it faces: its track
    rises: Vec<(Vec2, Vec2)>,
    clear: Vec<Obb>,
    paint: Vec<PaintArea>,
    notes: Vec<String>,
    counts: BTreeMap<String, usize>,
    landmark: Option<String>,
}

/// Lays a town out inside `poly` (the town's frame: its long side along x).
pub fn layout(poly: &[Vec2], spec: &Spec, cat: &Catalog, site: &dyn Site) -> Result<Town, String> {
    let area = poly_area(poly);
    let min = if spec.size.is_town() { 9000.0 } else { 1500.0 };
    if poly.len() < 3 || area < min {
        return Err(format!(
            "Draw a bigger area for a {} (at least {min:.0} m²; this is {area:.0} m²)",
            spec.size.name().to_lowercase()
        ));
    }
    let mut g = Gen {
        poly,
        site,
        cat,
        spec,
        kit: Kit::of(spec.style, spec.size, cat),
        rng: Rng::new(spec.seed as u64 ^ 0x70E4_17A5 ^ ((spec.size as u64 + 1) << 40)),
        seed: spec.seed ^ ((spec.size as u32 + 1) << 24),
        area,
        size: spec.size,
        town: spec.size.is_town(),
        ext: if spec.lead_out { 14.0 } else { 3.0 },
        edge: if spec.walls { 7.0 } else { 1.5 },
        centre: deepest(poly),
        reach: 1.0,
        entry: 0.0,
        streets: vec![],
        street_pts: vec![],
        core: None,
        square: None,
        pieces: vec![],
        built: spec
            .obstacles
            .iter()
            .map(|o| (*o, Spot::Obstacle))
            .collect(),
        fronts: vec![],
        lined: vec![],
        props: vec![],
        trees: vec![],
        rises: vec![],
        clear: vec![],
        paint: vec![],
        notes: spec.mix.notes.clone(),
        counts: BTreeMap::new(),
        landmark: None,
    };
    g.main_road();
    g.plan_core();
    g.measure();
    g.lanes();
    g.index_streets();
    g.buildings();
    g.dress_core();
    g.road_lamps();
    g.dressing();
    g.add_trees();
    if spec.walls {
        g.palisade();
    }
    let Gen {
        streets,
        pieces,
        paint,
        clear,
        square,
        landmark,
        counts,
        notes,
        ..
    } = g;
    Ok(Town {
        streets,
        pieces,
        paint,
        clear,
        square,
        landmark,
        counts,
        notes,
    })
}

impl Gen<'_> {
    /// The square's centre (the outline's middle without one).
    fn hub(&self) -> Vec2 {
        self.core.map(|k| k.c).unwrap_or(self.centre)
    }

    /// The main road along x, through the middle in a gentle arc, held near the middle by the
    /// outline and turned off steep ground and water.
    fn main_road(&mut self) {
        let (kind, width, bend) = match self.size {
            TownSize::Hamlet => (RoadKind::DirtRoad, 3.8, 0.06),
            TownSize::Village => (RoadKind::DirtRoad, RoadKind::DirtRoad.default_width(), 0.07),
            TownSize::Town => (RoadKind::Cobblestone, 5.5, 0.05),
            TownSize::City => (
                RoadKind::Cobblestone,
                RoadKind::Cobblestone.default_width(),
                0.045,
            ),
        };
        let poly = self.poly;
        let (lo, hi) = bounds(poly);
        let c = self.centre;
        // where the outline reaches along the middle
        let inside = |x: f32| chord(poly, x, c.y).is_some_and(|(a, b)| b - a > 2.0);
        let mut x0 = c.x;
        while x0 > lo.x && inside(x0 - 1.0) {
            x0 -= 1.0;
        }
        let mut x1 = c.x;
        while x1 < hi.x && inside(x1 + 1.0) {
            x1 += 1.0;
        }
        let a = Vec2::new(x0 - self.ext, 0.0);
        let len = x1 - x0 + 2.0 * self.ext;
        let arc = bend * if self.rng.f32() < 0.5 { -1.0 } else { 1.0 } * (x1 - x0);
        let pref = move |s: f32| {
            let t = ((a.x + s - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0);
            c.y + arc * (PI * t).sin()
        };
        let lat = move |s: f32| {
            let x = (a.x + s).clamp(x0, x1);
            match chord(poly, x, c.y) {
                Some((z0, z1)) => {
                    let m = ((z1 - z0) * 0.32).min(18.0);
                    (z0 + m, z1 - m)
                }
                None => (c.y, c.y),
            }
        };
        let line = Line::new(route(self.site, a, Vec2::X, len, 10.0, &lat, &pref));
        let name = if self.town {
            "High street"
        } else {
            "Village street"
        };
        self.streets.push(Street {
            kind,
            width,
            line,
            name,
        });
    }

    /// The square (or the green) beside the main road near the middle: on the flattest dry
    /// stretch with room behind it for the buildings framing it, a gentle rise behind preferred.
    fn plan_core(&mut self) {
        let main = self.streets[0].line.clone();
        let hw = self.streets[0].width * 0.5;
        let (half, round) = match self.size {
            TownSize::Hamlet => (Vec2::splat(5.0), 5.0),
            TownSize::Village => (Vec2::new(10.0, 8.5), 6.0),
            TownSize::Town => (Vec2::new(15.0, 12.0), 4.0),
            TownSize::City => (Vec2::new(21.0, 16.0), 5.0),
        };
        let k = (self.area / self.size.default_area())
            .powf(0.15)
            .clamp(0.85, 1.3);
        let half = half * k;
        let round = (round * k).min(half.min_element());
        let behind = match self.size {
            TownSize::Hamlet => 6.0,
            TownSize::Village => 14.0,
            _ => 16.0,
        };
        let (s_mid, _) = main.nearest(self.centre);
        let len = main.len();
        let mut best: Option<(f32, Core)> = None;
        for ds in [0.0f32, -8.0, 8.0, -16.0, 16.0, -26.0, 26.0] {
            let s = s_mid + ds;
            if s < 8.0 || s > len - 8.0 {
                continue;
            }
            let p = main.at(s);
            let t = main.tangent(s);
            for side in [1.0f32, -1.0] {
                let n = t.perp() * side;
                let c = p + n * (hw + 0.8 + half.y);
                let sq = Obb { c, ax: t, half };
                let k4 = sq.corners();
                if k4
                    .iter()
                    .chain([&c])
                    .any(|q| poly_sdf(self.poly, *q) > -(self.edge + 2.0))
                {
                    continue;
                }
                let room = reach(self.poly, c, n, 150.0);
                if room < half.y + behind {
                    continue;
                }
                let pts = [
                    k4[0],
                    k4[1],
                    k4[2],
                    k4[3],
                    c,
                    (k4[0] + k4[1]) * 0.5,
                    (k4[1] + k4[2]) * 0.5,
                    (k4[2] + k4[3]) * 0.5,
                    (k4[3] + k4[0]) * 0.5,
                ];
                if pts.iter().any(|q| self.site.wet(*q)) {
                    continue;
                }
                let hs: Vec<f32> = pts.iter().map(|q| self.site.height(*q)).collect();
                let drop = hs.iter().copied().fold(f32::MIN, f32::max)
                    - hs.iter().copied().fold(f32::MAX, f32::min);
                let rise = (self.site.height(c + n * (half.y + 12.0)) - self.site.height(c))
                    .clamp(-2.0, 3.0);
                let score = drop * 3.0 + ds.abs() * 0.12 - room.min(70.0) * 0.05 - rise * 0.8;
                if best.as_ref().is_none_or(|b| score < b.0) {
                    best = Some((
                        score,
                        Core {
                            s,
                            t,
                            n,
                            c,
                            half,
                            round,
                        },
                    ));
                }
            }
        }
        if best.is_none() {
            // a tight area: a smaller green right by the road at the middle, the roomier side
            let p = main.at(s_mid);
            let t = main.tangent(s_mid);
            let n = if reach(self.poly, p, t.perp(), 80.0) >= reach(self.poly, p, -t.perp(), 80.0) {
                t.perp()
            } else {
                -t.perp()
            };
            let half = half * 0.7;
            let round = round.min(half.min_element());
            let c = p + n * (hw + 0.8 + half.y);
            if poly_sdf(self.poly, c) < -(half.max_element() + 1.0) && !self.site.wet(c) {
                best = Some((
                    0.0,
                    Core {
                        s: s_mid,
                        t,
                        n,
                        c,
                        half,
                        round,
                    },
                ));
            }
        }
        self.core = best.map(|b| b.1);
        self.square = self.core.map(|k| Obb {
            c: k.c,
            ax: k.t,
            half: k.half,
        });
    }

    /// How far the settlement reaches from its square, and how far in the smithy stands.
    fn measure(&mut self) {
        let hub = self.hub();
        self.reach = self
            .poly
            .iter()
            .map(|p| p.distance(hub))
            .fold(0.0, f32::max)
            .max(20.0);
        let main = &self.streets[0].line;
        let (s0, s1) = self.inside_span(main, self.edge + 2.0);
        let ends = (hub.distance(main.at(s0)) + hub.distance(main.at(s1))) * 0.5;
        self.entry = ends * 0.62;
    }

    /// Where a street is inside the outline (`margin` m in): its arc lengths from, to.
    fn inside_span(&self, line: &Line, margin: f32) -> (f32, f32) {
        let len = line.len();
        let inside = |s: f32| poly_sdf(self.poly, line.at(s)) < -margin;
        let mut s0 = 0.0;
        while s0 < len && !inside(s0) {
            s0 += 1.0;
        }
        let mut s1 = len;
        while s1 > s0 && !inside(s1) {
            s1 -= 1.0;
        }
        (s0, s1)
    }

    /// One to four lanes off the main road at varied, gentle angles (leaning outward, away from
    /// the centre), bending with the ground, clear of the square and of each other.
    fn lanes(&mut self) {
        let wanted = match self.size {
            TownSize::Hamlet => usize::from(self.area > 4500.0),
            TownSize::Village => 1 + usize::from(self.area > 13000.0),
            TownSize::Town => 3,
            TownSize::City => 4 + usize::from(self.area > 100000.0),
        };
        if wanted == 0 {
            return;
        }
        let main = self.streets[0].line.clone();
        let (kind, width) = if self.town {
            (RoadKind::DirtRoad, 4.0)
        } else {
            (RoadKind::Footpath, RoadKind::Footpath.default_width())
        };
        let wall = if self.spec.walls { 10.0 } else { 6.0 };
        let long = match self.size {
            TownSize::Hamlet => 0.8,
            TownSize::Village => 1.0,
            TownSize::Town => 1.25,
            TownSize::City => 1.5,
        };
        let (core_s, core_n, core_hx) =
            self.core
                .map(|k| (k.s, k.n, k.half.x))
                .unwrap_or((main.len() * 0.5, Vec2::Y, 0.0));
        let (s0, s1) = self.inside_span(&main, wall);
        if s1 - s0 < 40.0 {
            return;
        }
        let sq = self.square.map(|q| q.grown(5.0));
        let mut made: Vec<(f32, f32, Line)> = vec![];
        let mut side = if self.rng.f32() < 0.5 { 1.0f32 } else { -1.0 };
        for _ in 0..wanted * 16 {
            if made.len() >= wanted {
                break;
            }
            let s = self.rng.range(s0 + 10.0, s1 - 10.0);
            let t = main.tangent(s);
            let square_side = (t.perp() * side).dot(core_n) > 0.0;
            // clear of the square's frame (its flanks on its side, the store across the road)
            if (s - core_s).abs() < if square_side { core_hx + 24.0 } else { 16.0 } {
                continue;
            }
            if made
                .iter()
                .any(|(ms, mside, _)| (ms - s).abs() < if *mside == side { 48.0 } else { 26.0 })
            {
                continue;
            }
            let p = main.at(s);
            let out = if s >= core_s { t } else { -t };
            let tilt = self.rng.range(0.15, 0.55);
            let dir = (t.perp() * side * tilt.cos() + out * tilt.sin()).normalize_or(t.perp());
            let room = reach(self.poly, p, dir, 300.0) - wall;
            if room < 28.0 {
                side = -side;
                continue;
            }
            let l = (room * self.rng.range(0.7, 0.95)).min(78.0 * long);
            let amp = self.rng.range(2.5, 6.0) * if self.rng.f32() < 0.5 { 1.0 } else { -1.0 };
            let pts = route(
                self.site,
                p,
                dir,
                l,
                8.0,
                &|s| if s < 7.0 { (0.0, 0.0) } else { (-8.0, 8.0) },
                &|s| amp * (PI * (s / l).clamp(0.0, 1.0)).sin(),
            );
            let line = Line::new(pts);
            let clash = line.samples(2.0).iter().skip(4).any(|q| {
                sq.is_some_and(|o| o.sdist(*q) < 0.0)
                    || made.iter().any(|(_, _, m)| m.nearest(*q).1 < 18.0)
                    || poly_sdf(self.poly, *q) > -1.0
            });
            if clash {
                continue;
            }
            made.push((s, side, line));
            side = -side;
        }
        for (_, _, line) in made {
            self.streets.push(Street {
                kind,
                width,
                line,
                name: "Lane",
            });
        }
    }

    fn index_streets(&mut self) {
        self.street_pts.clear();
        for (i, s) in self.streets.iter().enumerate() {
            for p in s.line.samples(1.0) {
                self.street_pts.push((p, s.width * 0.5, i));
            }
        }
        self.street_pts.extend(
            self.spec
                .keep_off
                .iter()
                .map(|(p, hw)| (*p, *hw, usize::MAX)),
        );
    }

    /// How far apart buildings keep here (m): wider toward the edge.
    fn spacing(&self, at: Vec2) -> f32 {
        let base = match self.size {
            TownSize::Hamlet => 11.0,
            TownSize::Village => 10.0,
            TownSize::Town => 8.5,
            TownSize::City => 7.5,
        };
        let t = (at.distance(self.hub()) / self.reach).clamp(0.0, 1.0);
        base * (1.0 + 0.6 * t)
    }

    /// Whether a building can stand here: inside, off the square and the streets, apart from the
    /// others, dry and not too steep to level. Its ground's drop (m) when it can.
    fn fit(&self, obb: &Obb, spot: Spot, relax: f32) -> Option<f32> {
        if obb
            .grown(0.5)
            .corners()
            .iter()
            .chain([&obb.c])
            .any(|q| poly_sdf(self.poly, *q) > -self.edge)
        {
            return None;
        }
        if let Some(sq) = &self.square
            && obb.overlaps(sq, if spot == Spot::Frame { 0.8 } else { 6.0 })
        {
            return None;
        }
        let gap = self.spacing(obb.c) * relax;
        if self
            .built
            .iter()
            .any(|(b, s)| b.overlaps(obb, need_gap(spot, *s, gap)))
        {
            return None;
        }
        let clear = if self.town { 1.0 } else { 1.4 };
        let r = obb.radius();
        if self
            .street_pts
            .iter()
            .any(|(q, hw, _)| q.distance(obb.c) < r + hw + clear && obb.sdist(*q) < hw + clear)
        {
            return None;
        }
        let g = obb.grown(1.2);
        let k = obb.corners();
        let pts = [
            k[0],
            k[1],
            k[2],
            k[3],
            obb.c,
            (k[0] + k[1]) * 0.5,
            (k[1] + k[2]) * 0.5,
            (k[2] + k[3]) * 0.5,
            (k[3] + k[0]) * 0.5,
        ];
        if g.corners()
            .iter()
            .chain(pts.iter())
            .any(|q| self.site.wet(*q))
        {
            return None;
        }
        let hs: Vec<f32> = pts.iter().map(|q| self.site.height(*q)).collect();
        let drop = hs.iter().copied().fold(f32::MIN, f32::max)
            - hs.iter().copied().fold(f32::MAX, f32::min);
        if drop > (0.24 * obb.half.max_element() * 2.0).clamp(1.6, 3.2) {
            return None;
        }
        Some(drop)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_building(
        &mut self,
        w: &Want,
        origin: Vec2,
        yaw: f32,
        obb: Obb,
        setback: f32,
        spot: Spot,
        front: Option<(usize, f32, f32)>,
    ) {
        self.built.push((obb, spot));
        self.clear.push(obb.grown(1.5));
        *self.counts.entry(TYPES[w.ty].name.to_string()).or_default() += 1;
        if let Some((street, s, side)) = front {
            self.fronts.push(Fronting { street, s, side });
            let half = obb.half.x;
            self.lined.push((street, s - half, s + half));
        }
        self.pieces.push(Piece {
            asset: w.asset.clone(),
            at: origin,
            yaw,
            scale: Vec3::ONE,
            span: 0.0,
            role: PieceRole::Building,
            ty: Some(w.ty),
            obb: Some(obb),
            setback,
        });
    }

    fn buildings(&mut self) {
        let mut queue: Vec<Want> = self.spec.mix.wants.clone();
        let wanted = queue.len();
        self.frame(&mut queue);
        // the smithy, then homes and shops (central ones first: the mix's order), the farms,
        // and the windmill or tower last, once the lanes are trimmed to their houses
        queue.sort_by_key(|w| match plan_of(w.ty) {
            Plan::Entry => 0,
            Plan::Lot => 1,
            Plan::Farm => 2,
            Plan::Rise => 3,
        });
        let (rise, rest): (Vec<Want>, Vec<Want>) =
            queue.into_iter().partition(|w| plan_of(w.ty) == Plan::Rise);
        let mut left: Vec<Want> = vec![];
        for w in rest {
            if !self.place_on_lot(&w) {
                left.push(w);
            }
        }
        self.trim_streets();
        for w in rise {
            if !self.place_rise(&w) && !self.place_on_lot(&w) {
                left.push(w);
            }
        }
        self.tracks();
        if !left.is_empty() {
            let mut names: BTreeMap<&str, usize> = BTreeMap::new();
            for w in &left {
                *names.entry(TYPES[w.ty].name).or_default() += 1;
            }
            let list: Vec<String> = names
                .iter()
                .map(|(n, k)| {
                    if *k > 1 {
                        format!("{n} ×{k}")
                    } else {
                        n.to_string()
                    }
                })
                .collect();
            self.notes.push(format!("{} of {wanted} buildings didn't fit ({}): draw a bigger area or try flatter ground", left.len(), list.join(", ")));
        }
    }

    /// Where a square slot's building fronts and which way it faces.
    fn slot(&mut self, k: Slot) -> Option<(Vec2, Vec2, Option<(usize, f32, f32)>)> {
        let core = self.core?;
        let Core {
            c,
            t,
            n,
            half,
            round,
            ..
        } = core;
        let sb = if self.town {
            self.rng.range(1.4, 2.6)
        } else {
            self.rng.range(2.2, 3.6)
        };
        let jitter = self.rng.range(-1.0, 1.0);
        let lift = self.rng.range(0.05, 0.3);
        let hw = self.streets[0].width * 0.5;
        Some(match k {
            Slot::Back => (c + n * (half.y + sb) + t * jitter * 1.5, -n, None),
            Slot::Left | Slot::Right => {
                let sx = if k == Slot::Left { -1.0 } else { 1.0 };
                (
                    c + t * sx * (half.x + sb) + n * (half.y * lift),
                    -t * sx,
                    None,
                )
            }
            Slot::BackLeft | Slot::BackRight => {
                let sx = if k == Slot::BackLeft { -1.0 } else { 1.0 };
                let corner = c + t * sx * (half.x - round * 0.3) + n * (half.y - round * 0.3);
                let d = (t * sx + n).normalize();
                (corner + d * (sb + 2.0), -d, None)
            }
            Slot::Across | Slot::AcrossLeft | Slot::AcrossRight => {
                let ds = match k {
                    Slot::Across => jitter * 2.0,
                    Slot::AcrossLeft => -(half.x + 8.0),
                    _ => half.x + 8.0,
                };
                let line = &self.streets[0].line;
                let s = core.s + ds;
                let mut away = line.tangent(s).perp();
                let side = if away.dot(n) > 0.0 {
                    away = -away;
                    -1.0
                } else {
                    1.0
                };
                (
                    line.at(s) + away * (hw + 1.4 + sb),
                    -away,
                    Some((0, s, side)),
                )
            }
        })
    }

    /// The key buildings round the square, facing it: the hall at the end of the axis (or
    /// beside the square on clearly higher ground), the inn on a flank by the road, the chapel
    /// and the store on the other flank or across the road, a city's bank and best shops on the
    /// corners. A village's chapel takes the end of the axis, its inn a flank, a shop across.
    fn frame(&mut self, queue: &mut Vec<Want>) {
        use Slot::*;
        let Some(core) = self.core else { return };
        if self.size == TownSize::Hamlet {
            return;
        }
        let probe = |k: Slot| -> Vec2 {
            match k {
                Left => core.c - core.t * (core.half.x + 8.0),
                Right => core.c + core.t * (core.half.x + 8.0),
                _ => core.c + core.n * (core.half.y + 8.0),
            }
        };
        let h = |k: Slot| self.site.height(probe(k));
        let (hb, hl, hr) = (h(Back), h(Left), h(Right));
        let civic = if hl > hb + 1.5 && hl >= hr {
            Left
        } else if hr > hb + 1.5 {
            Right
        } else {
            Back
        };
        let flank = match civic {
            Left => Right,
            Right => Left,
            _ if self.rng.f32() < 0.5 => Left,
            _ => Right,
        };
        let other = if flank == Left { Right } else { Left };
        let (b1, b2) = if self.rng.f32() < 0.5 {
            (BackLeft, BackRight)
        } else {
            (BackRight, BackLeft)
        };
        let (a1, a2) = if self.rng.f32() < 0.5 {
            (AcrossLeft, AcrossRight)
        } else {
            (AcrossRight, AcrossLeft)
        };
        let civic_alt = if civic == Back { other } else { Back };
        // (no hall or chapel, as in the photoreal style: the inn takes the end of the axis)
        let hall = queue
            .iter()
            .any(|w| matches!(TYPES[w.ty].id, "town_hall" | "chapel"));
        let inn = if hall {
            vec![flank, other, Across, a1, a2]
        } else {
            vec![civic, flank, other, Across]
        };
        let mut plan: Vec<(Option<&str>, Vec<Slot>)> = if self.town {
            vec![
                (Some("town_hall"), vec![civic, Back, b1, b2]),
                (Some("inn"), inn),
                (Some("chapel"), vec![civic_alt, other, b1, b2, a1]),
                (Some("general_store"), vec![Across, a1, a2, b2]),
                (Some("bank"), vec![b1, b2, a1, a2]),
            ]
        } else {
            vec![
                (Some("town_hall"), vec![civic, Back, b1]),
                (Some("chapel"), vec![civic, Back, b1, b2]),
                (Some("inn"), inn),
            ]
        };
        // the best shops after those (a village's store or bakery; a town's finest trades)
        let extra = match self.size {
            TownSize::Village => 1,
            TownSize::Town => 2,
            _ => 3,
        };
        for _ in 0..extra {
            plan.push((
                None,
                if self.town {
                    vec![a1, a2, b1, b2, Across]
                } else {
                    vec![Across, other, a1, a2]
                },
            ));
        }
        // a square with few key buildings (a small or photoreal mix) is framed by homes too
        let least = match self.size {
            TownSize::Village => 3,
            TownSize::Town => 4,
            _ => 5,
        };
        for _ in 0..least {
            plan.push((
                Some("*"),
                if self.town {
                    vec![Across, other, flank, b1, b2, a1, a2]
                } else {
                    vec![Across, other, flank, a1, a2]
                },
            ));
        }
        let mut used: Vec<Slot> = vec![];
        for (id, slots) in plan {
            let found = queue.iter().position(|w| match id {
                Some("*") => used.len() < least && plan_of(w.ty) == Plan::Lot,
                Some(id) => TYPES[w.ty].id == id,
                None => {
                    plan_of(w.ty) == Plan::Lot
                        && matches!(TYPES[w.ty].role, Role::Shop | Role::Civic)
                        && w.key >= 0.62
                        && !matches!(TYPES[w.ty].id, "inn" | "chapel" | "town_hall")
                }
            });
            let Some(qi) = found else { continue };
            let Some(b) = self.cat.bounds(&queue[qi].asset) else {
                continue;
            };
            for k in slots {
                if used.contains(&k) {
                    continue;
                }
                let Some((front, f, fronting)) = self.slot(k) else {
                    continue;
                };
                let turn = self.rng.range(-0.03, 0.03);
                let (origin, yaw, obb) = at_front(b, front, f, turn);
                if self.fit(&obb, Spot::Frame, 1.0).is_none() {
                    continue;
                }
                let w = queue.remove(qi);
                let setback = (front - core.c).length() - core.half.max_element();
                self.add_building(
                    &w,
                    origin,
                    yaw,
                    obb,
                    setback.max(1.0),
                    Spot::Frame,
                    fronting,
                );
                used.push(k);
                break;
            }
        }
    }

    fn place_on_lot(&mut self, w: &Want) -> bool {
        let Some(b) = self.cat.bounds(&w.asset) else {
            return false;
        };
        let plan = plan_of(w.ty);
        let spot = match plan {
            Plan::Farm => Spot::Farm,
            Plan::Rise => Spot::Rise,
            _ => Spot::Other,
        };
        for relax in [1.0, 0.7, 0.45] {
            if let Some(c) = self.best_lot(w, b, plan, spot, relax) {
                self.add_building(
                    w,
                    c.origin,
                    c.yaw,
                    c.obb,
                    c.setback,
                    spot,
                    Some((c.street, c.s, c.side)),
                );
                return true;
            }
        }
        false
    }

    /// The best lot along the streets for a building: nearest the centre for homes and shops
    /// (shops on the main road, homes on the lanes), the road in for the smithy, the outskirts
    /// for a farmhouse and beside it for its barn; never straight across the street from
    /// another; the flattest.
    fn best_lot(
        &self,
        w: &Want,
        b: (Vec3, Vec3),
        plan: Plan,
        spot: Spot,
        relax: f32,
    ) -> Option<Cand> {
        let hub = self.hub();
        let ty = &TYPES[w.ty];
        let shop = matches!(ty.role, Role::Shop | Role::Civic | Role::Guard);
        let farmhouses: Vec<Vec2> = self
            .pieces
            .iter()
            .filter(|p| p.ty.is_some_and(|t| TYPES[t].id == "farmhouse"))
            .filter_map(|p| p.obb.map(|o| o.c))
            .collect();
        let farms: Vec<Vec2> = self
            .built
            .iter()
            .filter(|(_, s)| *s == Spot::Farm)
            .map(|(o, _)| o.c)
            .collect();
        let far = if self.size == TownSize::Hamlet {
            0.6
        } else {
            0.8
        } * self.reach;
        // how far out it wants to stand: central trades near the square, homes further, each a
        // little nearer or further than the next (so they spread over the lanes, not in a ring)
        let jitter = h01(self.seed, self.pieces.len() as u32, w.ty as u32, 31) - 0.5;
        let target = self.reach * (0.16 + 0.5 * (1.0 - w.key).clamp(0.0, 1.0) + 0.25 * jitter);
        let (sb_lo, sb_hi, turn_max) = match plan {
            Plan::Farm => (4.0, 9.0, 0.08),
            Plan::Entry => (2.5, 5.0, 0.05),
            _ if self.town => (1.6, 5.2, 0.07),
            _ => (2.6, 6.5, 0.09),
        };
        let clear = if self.town { 1.0 } else { 1.4 };
        let by_water = matches!(ty.id, "sawmill" | "fisherman");
        let mut best: Option<Cand> = None;
        for (si, st) in self.streets.iter().enumerate() {
            if st.name == "Track" || (plan == Plan::Entry && si != 0) {
                continue;
            }
            let hw = st.width * 0.5;
            let lane = si != 0;
            let len = st.line.len();
            let mut k = 0u32;
            let mut s = 4.0;
            while s < len - 2.0 {
                k += 1;
                let p = st.line.at(s);
                let here = s;
                s += 3.0;
                if poly_sdf(self.poly, p) > -2.0 {
                    continue;
                }
                let tg = st.line.tangent(here);
                for side in [1.0f32, -1.0] {
                    let nrm = tg.perp() * side;
                    for v in 0..2u32 {
                        let r = |salt: u32| {
                            h01(
                                self.seed,
                                (si as u32) << 12 | k,
                                u32::from(side > 0.0) * 2 + v,
                                salt,
                            )
                        };
                        let setback = sb_lo + (sb_hi - sb_lo) * r(1);
                        let turn = (r(2) - 0.5) * 2.0 * turn_max;
                        let front =
                            p + tg * ((r(3) - 0.5) * 2.0) + nrm * (hw + clear + 0.4 + setback);
                        let (origin, yaw, obb) = at_front(b, front, -nrm, turn);
                        let d = obb.c.distance(hub);
                        let noise = r(4);
                        let mut score = match plan {
                            Plan::Lot => {
                                let street = match (lane, shop) {
                                    (true, true) => 0.0,
                                    (true, false) => -16.0,
                                    (false, true) => -5.0,
                                    (false, false) => 6.0,
                                };
                                (d - target).abs() + street + noise * 10.0
                            }
                            Plan::Entry => (d - self.entry).abs() + noise * 6.0,
                            Plan::Farm | Plan::Rise => {
                                let lone =
                                    (d - far).abs() - if lane { 4.0 } else { 0.0 } + noise * 8.0;
                                let nearest = |v: &[Vec2]| {
                                    v.iter().map(|q| q.distance(obb.c)).fold(f32::MAX, f32::min)
                                };
                                if ty.id == "farmhouse" {
                                    // a second farmstead away from the first
                                    lone - if farmhouses.is_empty() {
                                        0.0
                                    } else {
                                        nearest(&farmhouses).min(150.0) * 0.35
                                    }
                                } else if !farms.is_empty() {
                                    // the barn, granary or stable beside the farmhouse
                                    nearest(if farmhouses.is_empty() {
                                        farms.as_slice()
                                    } else {
                                        farmhouses.as_slice()
                                    }) + noise * 4.0
                                } else {
                                    lone
                                }
                            }
                        };
                        if plan == Plan::Lot {
                            // never straight across the street from another, nor a row of them
                            // down one side
                            for f in self.fronts.iter().filter(|f| f.street == si) {
                                let ds = (f.s - here).abs();
                                if f.side != side && ds < 9.0 {
                                    score += 10.0;
                                } else if f.side == side && ds < 28.0 {
                                    score += 7.0;
                                }
                            }
                        }
                        if by_water {
                            // the mill (or the fisher) on the bank
                            score += if self.water_by(obb.c, obb.radius() + 14.0) {
                                -40.0
                            } else {
                                0.0
                            };
                        }
                        if best.as_ref().is_some_and(|c| score >= c.score) {
                            continue;
                        }
                        let Some(drop) = self.fit(&obb, spot, relax) else {
                            continue;
                        };
                        score += drop * 5.0;
                        if best.as_ref().is_none_or(|c| score < c.score) {
                            best = Some(Cand {
                                origin,
                                yaw,
                                obb,
                                setback,
                                street: si,
                                s: here,
                                side,
                                score,
                            });
                        }
                    }
                }
            }
        }
        best
    }

    /// Whether there is water within `r` m of `q`.
    fn water_by(&self, q: Vec2, r: f32) -> bool {
        (0..16).any(|i| {
            let d = Vec2::from_angle(TAU * i as f32 / 16.0);
            [0.5f32, 1.0].iter().any(|k| self.site.wet(q + d * r * *k))
        })
    }

    /// The nearest street point to `q` (not a track): (point, distance from its edge).
    fn nearest_street(&self, q: Vec2) -> Option<(Vec2, f32)> {
        self.streets
            .iter()
            .filter(|s| s.name != "Track")
            .map(|s| {
                let (at, d) = s.line.nearest(q);
                (s.line.at(at), d - s.width * 0.5)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// The windmill or watchtower: on the highest open ground toward the edge, facing the
    /// nearest street (a track leads up to it).
    fn place_rise(&mut self, w: &Want) -> bool {
        let Some(b) = self.cat.bounds(&w.asset) else {
            return false;
        };
        let hub = self.hub();
        let h0 = self.site.height(hub);
        let (lo, hi) = bounds(self.poly);
        let step = 5.0;
        let mut best: Option<(f32, Vec2, f32, Obb, Vec2)> = None;
        let (mut z, mut iz) = (lo.y, 0u32);
        while z <= hi.y {
            let (mut x, mut ix) = (lo.x, 0u32);
            while x <= hi.x {
                let q = Vec2::new(x, z);
                x += step;
                ix += 1;
                let d = q.distance(hub);
                if d < 0.5 * self.reach || poly_sdf(self.poly, q) > -(self.edge + 6.0) {
                    continue;
                }
                let Some((sp, sd)) = self.nearest_street(q) else {
                    continue;
                };
                let (origin, yaw, obb) = at_centre(b, q, (sp - q).normalize_or(Vec2::Y), 1.0);
                if sd < obb.half.y + 4.0 {
                    continue;
                }
                let pre = -(self.site.height(q) - h0).clamp(-6.0, 14.0) * 2.0
                    - d / self.reach * 8.0
                    + sd.min(60.0) * 0.05
                    + h01(self.seed, ix, iz, 9) * 4.0;
                if best.as_ref().is_some_and(|c| pre >= c.0) {
                    continue;
                }
                let Some(drop) = self.fit(&obb, Spot::Rise, 1.0) else {
                    continue;
                };
                let score = pre + drop * 6.0;
                if best.as_ref().is_none_or(|c| score < c.0) {
                    best = Some((score, origin, yaw, obb, sp));
                }
            }
            z += step;
            iz += 1;
        }
        let Some((_, origin, yaw, obb, sp)) = best else {
            return false;
        };
        self.add_building(w, origin, yaw, obb, 0.0, Spot::Rise, None);
        self.rises
            .push((obb.c + rot2(Vec2::Y, yaw) * obb.half.y, sp));
        true
    }

    /// Tracks from the street up to the windmill's (or tower's) door, round nothing.
    fn tracks(&mut self) {
        let kind = if self.town {
            RoadKind::Footpath
        } else {
            RoadKind::Trail
        };
        let short = kind.default_width() * 0.5 + 0.5;
        for (door, sp) in std::mem::take(&mut self.rises) {
            let d = door.distance(sp) - short;
            if d < 6.0 {
                continue;
            }
            let dir = (door - sp).normalize();
            let pts = route(
                self.site,
                sp,
                dir,
                d,
                5.0,
                &|s| {
                    if s < 3.0 || s > d - 2.5 {
                        (0.0, 0.0)
                    } else {
                        (-2.5, 2.5)
                    }
                },
                &|_| 0.0,
            );
            let line = Line::new(pts);
            if line.samples(1.0).iter().any(|q| {
                self.built.iter().any(|(o, _)| o.sdist(*q) < short - 0.1) || self.site.wet(*q)
            }) {
                continue;
            }
            self.streets.push(Street {
                kind,
                width: kind.default_width(),
                line,
                name: "Track",
            });
        }
        self.index_streets();
    }

    /// Lanes end a few metres past their last building; one no building lines goes. The main
    /// road stays whole.
    fn trim_streets(&mut self) {
        let mut keep = vec![true; self.streets.len()];
        for (i, st) in self.streets.iter_mut().enumerate() {
            if st.name != "Lane" {
                continue;
            }
            let along: Vec<(f32, f32)> = self
                .lined
                .iter()
                .filter(|l| l.0 == i)
                .map(|l| (l.1, l.2))
                .collect();
            if along.is_empty() {
                keep[i] = false;
                continue;
            }
            let hi = along.iter().map(|a| a.1).fold(f32::MIN, f32::max);
            let to = (hi + 7.0).min(st.line.len());
            if to < st.line.len() - 0.5 {
                st.line = st.line.sub(0.0, to);
            }
        }
        // (renumber what refers to streets)
        let mut map = vec![usize::MAX; self.streets.len()];
        let mut n = 0;
        for (i, k) in keep.iter().enumerate() {
            if *k {
                map[i] = n;
                n += 1;
            }
        }
        let mut k = keep.iter();
        self.streets.retain(|_| *k.next().unwrap_or(&true));
        self.fronts.retain_mut(|f| {
            f.street = map[f.street];
            f.street != usize::MAX
        });
        self.lined.retain_mut(|l| {
            l.0 = map[l.0];
            l.0 != usize::MAX
        });
        self.index_streets();
    }

    /// Is a prop spot free (off the roads and buildings, inside, dry)?
    fn free(&self, q: Vec2, r: f32) -> bool {
        poly_sdf(self.poly, q) < -(r + 0.5)
            && !self.site.wet(q)
            && self.built.iter().all(|(b, _)| b.sdist(q) > r + 0.25)
            && self
                .props
                .iter()
                .all(|(p, pr)| p.distance(q) > r + pr + 0.15)
            && self
                .street_pts
                .iter()
                .all(|(p, hw, _)| p.distance(q) > hw + r + 0.4)
    }

    fn put(&mut self, asset: &str, at: Vec2, yaw: f32) {
        self.props.push((at, reach_of(asset)));
        self.pieces.push(Piece::prop(asset, at, yaw));
    }

    /// A small group of props round `c` (the first on it, the rest close by).
    fn group(&mut self, models: &[String], c: Vec2) -> bool {
        let mut any = false;
        for (i, m) in models.iter().enumerate() {
            let r = reach_of(m);
            for attempt in 0..14 {
                let at = if i == 0 && attempt == 0 {
                    c
                } else {
                    c + Vec2::from_angle(self.rng.range(0.0, TAU))
                        * self.rng.range(0.9, 1.6 + attempt as f32 * 0.06)
                };
                if self.free(at, r) {
                    let yaw = self.rng.range(0.0, TAU);
                    self.put(m, at, yaw);
                    any = true;
                    break;
                }
            }
        }
        any
    }

    /// The square's (or the green's) landmark, benches, stalls, cart, notice board, lamps and
    /// signpost, its paint (cobbles; a green's grass) and a big tree on a green.
    fn dress_core(&mut self) {
        let Some(core) = self.core else { return };
        let Core {
            c,
            t,
            n,
            half,
            round,
            ..
        } = core;
        let inner = Obb {
            c,
            ax: t,
            half: half - Vec2::splat(round),
        };
        // how far inside the square's rounded edge (negative inside)
        let rim = move |p: Vec2| inner.sdist(p) - round;
        // the point `inset` m inside the edge, `deg` round from the back (180: the road side)
        let edge_at = move |deg: f32, inset: f32| {
            let a = deg.to_radians();
            let dir = t * a.sin() + n * a.cos();
            let mut k = 0.0;
            while k < 80.0 && rim(c + dir * (k + 0.25)) < -inset {
                k += 0.25;
            }
            c + dir * k
        };
        let mut room = 1.5;
        if let Some(lm) = self.kit.landmark.clone() {
            let yaw = yaw_facing(-n) + self.rng.range(-0.12, 0.12);
            if lm.structure {
                if let Some(bb) = self.cat.bounds(&lm.asset) {
                    let (origin, yaw, obb) = at_centre(bb, c, rot2(Vec2::Y, yaw), lm.scale);
                    self.pieces.push(Piece {
                        asset: lm.asset.clone(),
                        at: origin,
                        yaw,
                        scale: Vec3::splat(lm.scale),
                        span: 0.0,
                        role: PieceRole::Building,
                        ty: None,
                        obb: Some(obb),
                        setback: 0.0,
                    });
                    self.built.push((obb, Spot::Frame));
                }
            } else {
                self.pieces
                    .push(Piece::prop(&lm.asset, c, yaw).scaled(lm.scale));
            }
            self.props.push((c, lm.room));
            room = lm.room;
            self.landmark = Some(lm.asset.clone());
            *self.counts.entry(lm.name.to_string()).or_default() += 1;
        }
        // benches round it, facing it (a green's either side, leaving its road side open)
        if let Some(bench) = self.kit.bench.clone() {
            let k = if self.town { 4 } else { 2 };
            let r = room + 2.4;
            for i in 0..k {
                let a = TAU * (i as f32 + 0.5) / k as f32 + if self.town { 0.0 } else { FRAC_PI_2 };
                let at = c + (t * a.cos() + n * a.sin()) * r;
                if rim(at) < -0.8 && self.free(at, 0.8) {
                    self.put(&bench, at, yaw_facing(c - at));
                }
            }
        }
        // a well to one side of a statue or rotunda
        if let Some(well) = self.kit.well.clone().filter(|_| self.town) {
            for deg in [-145.0f32, 145.0, -120.0, 120.0] {
                let at = edge_at(deg, 3.0);
                if self.free(at, 1.9) {
                    let yaw = self.rng.range(0.0, TAU);
                    self.put(&well, at, yaw);
                    self.props.push((at, 1.8));
                    *self.counts.entry("Well".to_string()).or_default() += 1;
                    break;
                }
            }
        }
        // a few market stalls round the edge, counters toward the middle, off the ways in
        let wants_stalls = self.spec.mix.stalls;
        let stalls = wants_stalls.unwrap_or(match self.size {
            TownSize::Hamlet => 0,
            TownSize::Village => 1,
            TownSize::Town => 3,
            TownSize::City => 5,
        });
        let mut placed = 0;
        if let Some(stall) = self.kit.stall.clone() {
            let angles: &[f32] = if self.town {
                &[
                    40.0, -40.0, 68.0, -68.0, 128.0, -128.0, 108.0, -108.0, 22.0, -22.0, 150.0,
                    -150.0,
                ]
            } else {
                &[150.0, -150.0, 125.0, -125.0, 60.0, -60.0]
            };
            for deg in angles {
                if placed >= stalls {
                    break;
                }
                let at = edge_at(*deg, 2.6);
                if !self.free(at, 2.2) {
                    continue;
                }
                let yaw = yaw_facing(c - at);
                self.put(&stall, at, yaw);
                placed += 1;
                // goods on the counter side
                let (ax, az) = (rot2(Vec2::X, yaw), rot2(Vec2::Y, yaw));
                for g in [-1.25f32, 1.25] {
                    if self.kit.goods.is_empty() {
                        break;
                    }
                    let m = self.kit.goods[self.rng.below(self.kit.goods.len() as u32) as usize]
                        .clone();
                    let at = at + az * 1.75 + ax * (g + self.rng.range(-0.15, 0.15));
                    self.pieces
                        .push(Piece::prop(&m, at, yaw + self.rng.range(-0.4, 0.4)));
                }
            }
        }
        if placed > 0 {
            *self.counts.entry("Market stall".to_string()).or_default() += placed;
        }
        if wants_stalls.is_some_and(|k| placed < k) {
            self.notes.push(format!(
                "{placed} of {stalls} market stalls fit on the {}",
                if self.town { "square" } else { "green" }
            ));
        }
        // a cart by the stalls
        if !self.kit.carts.is_empty() && self.size != TownSize::Hamlet {
            let m = self.kit.carts[self.rng.below(self.kit.carts.len() as u32) as usize].clone();
            for deg in [92.0f32, -92.0, 100.0, -100.0, 84.0, -84.0, 160.0, -160.0] {
                let at = edge_at(deg, 2.2);
                if self.free(at, 1.9) {
                    let a = deg.to_radians();
                    let along = (t * a.cos() - n * a.sin()).normalize();
                    let yaw = yaw_facing(along) + self.rng.range(-0.3, 0.3);
                    self.put(&m, at, yaw);
                    break;
                }
            }
        }
        // the notice board by the road, readable from it
        if let Some(m) = self
            .kit
            .notice
            .clone()
            .filter(|_| self.size != TownSize::Hamlet)
        {
            for deg in [-162.0f32, 162.0, -140.0, 140.0] {
                let at = edge_at(deg, 1.6);
                if self.free(at, 1.3) {
                    self.put(&m, at, yaw_facing(-n));
                    break;
                }
            }
        }
        // lamps: a square's four corners, a green's road side
        if let Some(lamp) = self.kit.lamp.clone() {
            let spots: &[f32] = match self.size {
                TownSize::Hamlet => &[],
                TownSize::Village => &[-128.0, 128.0],
                _ => &[-52.0, 52.0, -128.0, 128.0],
            };
            let mut lamps = 0;
            for deg in spots {
                let at = edge_at(*deg, 1.2);
                if self.free(at, 0.6) {
                    self.put(&lamp, at, yaw_along(c - at));
                    lamps += 1;
                }
            }
            if lamps > 0 {
                *self.counts.entry("Lamp post".to_string()).or_default() += lamps;
            }
        }
        // the signpost by the road, at the square's corner
        if let Some(m) = self.kit.signpost.clone() {
            let main = &self.streets[0];
            let hw = main.width * 0.5;
            let sx = if self.rng.f32() < 0.5 { 1.0 } else { -1.0 };
            let s = core.s + sx * (half.x + 3.0);
            let mut away = main.line.tangent(s).perp();
            if away.dot(n) < 0.0 {
                away = -away;
            }
            let at = main.line.at(s) + away * (hw + 1.2);
            if self.free(at, 0.9) {
                let yaw = self.rng.range(-0.6, 0.6);
                self.put(&m, at, yaw);
            }
        }
        if self.town {
            self.paint.push(PaintArea {
                obb: inner,
                round,
                layer: Layer::Cobblestone,
                patches: Some(Layer::Dirt),
                strength: 0.92,
            });
        } else {
            self.paint.push(PaintArea {
                obb: inner,
                round,
                layer: Layer::Grass,
                patches: None,
                strength: 0.85,
            });
            self.paint.push(PaintArea {
                obb: Obb {
                    c,
                    ax: t,
                    half: Vec2::ZERO,
                },
                round: room + 1.2,
                layer: Layer::Dirt,
                patches: Some(Layer::Gravel),
                strength: 0.75,
            });
            // a big tree at the green's back corner
            if !self.spec.trees.is_empty() {
                for deg in [40.0f32, -40.0, 60.0, -60.0] {
                    let at = edge_at(deg, 2.0);
                    if self.free(at, 2.2) {
                        let m = self.spec.trees
                            [self.rng.below(self.spec.trees.len() as u32) as usize]
                            .clone();
                        let yaw = self.rng.range(0.0, TAU);
                        self.pieces
                            .push(Piece::prop(&m, at, yaw).scaled(self.rng.range(1.6, 1.9)));
                        self.props.push((at, 1.5));
                        self.trees.push(at);
                        *self.counts.entry("Tree".to_string()).or_default() += 1;
                        break;
                    }
                }
            }
        }
        self.clear.push(Obb { c, ax: t, half }.grown(1.0));
    }

    /// Lamp posts along the main road near the centre: a town's every 26 m or so (sides taking
    /// turns), a village's one each side of its green.
    fn road_lamps(&mut self) {
        let (Some(lamp), Some(core)) = (self.kit.lamp.clone(), self.core) else {
            return;
        };
        let span = match self.size {
            TownSize::Hamlet => return,
            TownSize::Village => core.half.x + 12.0,
            TownSize::Town => 0.42 * self.reach,
            TownSize::City => 0.5 * self.reach,
        };
        let photo = lamp.starts_with("ph:");
        let main = self.streets[0].line.clone();
        let hw = self.streets[0].width * 0.5;
        let step = if self.town { 26.0 } else { 2.0 * span };
        let mut lamps: Vec<Vec2> = vec![];
        let mut side = 1.0f32;
        let mut s = core.s - span;
        while s <= core.s + span + 0.1 {
            if (s - core.s).abs() > core.half.x + 3.0 {
                for nudge in [0.0f32, 3.0, -3.0, 6.0] {
                    let at_s = s + nudge;
                    let tg = main.tangent(at_s);
                    let nn = tg.perp() * side;
                    let q = main.at(at_s) + nn * (hw + 0.75);
                    let ok = poly_sdf(self.poly, q) < -1.0
                        && !self.site.wet(q)
                        && self.square.is_none_or(|sq| sq.sdist(q) > 1.0)
                        && self.built.iter().all(|(b, _)| b.sdist(q) > 0.7)
                        && self.props.iter().all(|(p, r)| p.distance(q) > r + 0.6)
                        && lamps.iter().all(|l| l.distance(q) > 9.0)
                        && self
                            .street_pts
                            .iter()
                            .all(|(p, phw, i)| *i == 0 || p.distance(q) > phw + 1.8);
                    if ok {
                        // the lantern arm reaches over the road
                        let yaw = if photo { yaw_along(tg) } else { yaw_along(-nn) };
                        self.put(&lamp, q, yaw);
                        lamps.push(q);
                        break;
                    }
                }
                side = -side;
            }
            s += step;
        }
        if !lamps.is_empty() {
            *self.counts.entry("Lamp post".to_string()).or_default() += lamps.len();
        }
    }

    /// Restrained dressing: the inn's yard, the smithy's anvil and trough toward the road,
    /// barrels by some shops, a fenced garden beside some homes (flower boxes by others),
    /// haystacks and a field-edge fence by the farmsteads.
    fn dressing(&mut self) {
        let cap = ((self.area.sqrt() * 0.05).round() as usize).max(2);
        let mut groups = 0;
        let buildings: Vec<(usize, f32, Obb)> = self
            .pieces
            .iter()
            .filter_map(|p| Some((p.ty?, p.yaw, p.obb?)))
            .collect();
        let mut fields = 0;
        for (ty, yaw, obb) in buildings {
            let t = &TYPES[ty];
            let (ax, az) = (rot2(Vec2::X, yaw), rot2(Vec2::Y, yaw));
            let front = obb.c + az * obb.half.y;
            let sx = if self.rng.f32() < 0.5 { 1.0 } else { -1.0 };
            match (t.id, t.role) {
                ("inn", _) => {
                    let g = self.kit.yard.first().cloned().unwrap_or_default();
                    for side in [sx, -sx] {
                        let at = front + ax * side * (obb.half.x + 2.4) - az * 2.2;
                        if !g.is_empty() && self.free(at, 0.9) {
                            self.group(&g, at);
                            break;
                        }
                    }
                }
                ("blacksmith", _) => {
                    let g = self.kit.smithy.clone();
                    for side in [sx, -sx] {
                        let at = front + az * 1.7 + ax * side * (obb.half.x * 0.55);
                        if !g.is_empty() && self.free(at, 0.7) {
                            self.group(&g, at);
                            break;
                        }
                    }
                }
                (_, Role::Shop) if groups < cap && self.rng.f32() < 0.4 => {
                    if self.kit.groups.is_empty() {
                        continue;
                    }
                    let g = self.kit.groups[self.rng.below(self.kit.groups.len() as u32) as usize]
                        .clone();
                    for side in [sx, -sx] {
                        let at = front + ax * side * (obb.half.x + 1.0) - az * 1.4;
                        if self.free(at, 0.7) {
                            if self.group(&g, at) {
                                groups += 1;
                            }
                            break;
                        }
                    }
                }
                ("farmhouse" | "barn", _) => {
                    // haystacks beside it, a cart or barrow, and a fence along the field behind
                    if let Some(hay) = self.kit.hay.clone() {
                        for side in [sx, -sx] {
                            let at = obb.c + ax * side * (obb.half.x + 3.2)
                                - az * self.rng.range(0.0, 2.5);
                            if self.free(at, 1.3) {
                                let (y1, y2) = (self.rng.range(0.0, TAU), self.rng.range(0.0, TAU));
                                self.put(&hay, at, y1);
                                let at2 = at + ax * side * 2.6 - az * self.rng.range(1.0, 2.5);
                                if self.free(at2, 1.3) {
                                    self.put(&hay, at2, y2);
                                }
                                break;
                            }
                        }
                    }
                    if !self.kit.farm.is_empty() && self.rng.f32() < 0.6 {
                        let g = self.kit.farm[self.rng.below(self.kit.farm.len() as u32) as usize]
                            .clone();
                        let at = front + az * 1.8 + ax * -sx * (obb.half.x + 1.2);
                        if self.free(at, 1.0) {
                            self.group(&g, at);
                        }
                    }
                    if fields < 2
                        && let Some((fence, pitch)) = self.kit.field_fence.clone()
                    {
                        let back = obb.c - az * (obb.half.y + self.rng.range(4.5, 6.5));
                        let l = self.rng.range(12.0, 18.0);
                        let (a, b) = (back - ax * l * 0.5, back + ax * l * 0.5);
                        if self.fence_clear(a, b, None) {
                            fence_run(&mut self.pieces, a, b, &fence, pitch);
                            fields += 1;
                        }
                    }
                }
                (_, Role::Home) => {
                    if self.rng.f32() < 0.45
                        && let Some((fence, pitch)) = self.kit.garden_fence.clone()
                    {
                        // a fenced garden beside the house, its gate at the front
                        let gw = self.rng.range(4.5, 6.5);
                        let (x0, x1) = (obb.half.x + 0.3, obb.half.x + 0.3 + gw);
                        let (zf, zb) = (obb.half.y - 0.6, -obb.half.y + 0.4);
                        let q = |x: f32, z: f32| obb.c + ax * (x * sx) + az * z;
                        let runs = [
                            (q(x0 + 1.4, zf), q(x1, zf)),
                            (q(x1, zf), q(x1, zb)),
                            (q(x1, zb), q(x0, zb)),
                        ];
                        if runs
                            .iter()
                            .all(|(a, b)| self.fence_clear(*a, *b, Some(&obb)))
                        {
                            for (a, b) in runs {
                                fence_run(&mut self.pieces, a, b, &fence, pitch);
                            }
                            self.props
                                .push((q((x0 + x1) * 0.5, (zf + zb) * 0.5), gw * 0.5));
                            continue;
                        }
                    }
                    if !self.kit.flowers.is_empty() && self.rng.f32() < 0.4 {
                        let m = self.kit.flowers
                            [self.rng.below(self.kit.flowers.len() as u32) as usize]
                            .clone();
                        for s in [-1.0f32, 1.0] {
                            if s > 0.0 && self.rng.f32() < 0.5 {
                                continue;
                            }
                            let at = front + az * 0.45 + ax * s * (obb.half.x * 0.55);
                            if self
                                .street_pts
                                .iter()
                                .all(|(p, hw, _)| p.distance(at) > hw + 0.6)
                            {
                                self.pieces.push(Piece::prop(&m, at, yaw));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether a fence from `a` to `b` stays inside, dry, off the streets and clear of
    /// buildings (but `own`, which it may run along).
    fn fence_clear(&self, a: Vec2, b: Vec2, own: Option<&Obb>) -> bool {
        let n = (a.distance(b) / 1.0).ceil().max(1.0) as usize;
        (0..=n).map(|i| a.lerp(b, i as f32 / n as f32)).all(|q| {
            poly_sdf(self.poly, q) < -0.5
                && !self.site.wet(q)
                && self
                    .built
                    .iter()
                    .all(|(o, _)| own == Some(o) || o.sdist(q) > 0.6)
                && self.props.iter().all(|(p, r)| p.distance(q) > r + 0.2)
                && self
                    .street_pts
                    .iter()
                    .all(|(p, hw, _)| p.distance(q) > hw + 0.8)
                && self.square.is_none_or(|sq| sq.sdist(q) > 1.0)
        })
    }

    /// Trees on the open ground between the buildings: behind and beside them and in the gaps,
    /// spread out (each the farthest from the others), never on a street, a plot or the square.
    fn add_trees(&mut self) {
        if self.spec.trees.is_empty() {
            return;
        }
        let n_b = self.pieces.iter().filter(|p| p.ty.is_some()).count();
        let want = ((n_b as f32 * 0.8).round() as usize).clamp(3, 26);
        let mut cands: Vec<Vec2> = vec![];
        for (o, s) in &self.built {
            if *s == Spot::Obstacle {
                continue;
            }
            let az = o.az();
            for (dx, dz) in [
                (0.0f32, -1.0f32),
                (1.0, -0.6),
                (-1.0, -0.6),
                (1.0, 0.2),
                (-1.0, 0.2),
            ] {
                cands.push(o.c + o.ax * dx * (o.half.x + 6.5) + az * dz * (o.half.y + 6.5));
            }
        }
        let (lo, hi) = bounds(self.poly);
        let (mut z, mut j) = (lo.y + 4.0, 0u32);
        while z < hi.y {
            let (mut x, mut i) = (lo.x + 4.0, 0u32);
            while x < hi.x {
                cands.push(Vec2::new(
                    x + (h01(self.seed, i, j, 21) - 0.5) * 8.0,
                    z + (h01(self.seed, i, j, 22) - 0.5) * 8.0,
                ));
                x += 15.0;
                i += 1;
            }
            z += 15.0;
            j += 1;
        }
        let sq = self.square.map(|s| s.grown(4.0));
        let ok: Vec<Vec2> = cands
            .into_iter()
            .filter(|q| {
                poly_sdf(self.poly, *q) < -3.0
                    && !self.site.wet(*q)
                    && sq.is_none_or(|s| s.sdist(*q) > 0.0)
                    && self.built.iter().all(|(o, _)| o.sdist(*q) > 5.5)
                    && self.props.iter().all(|(p, r)| p.distance(*q) > r + 2.0)
                    && self
                        .street_pts
                        .iter()
                        .all(|(p, hw, _)| p.distance(*q) > hw + 3.5)
                    && slope(self.site, *q) < 0.45
            })
            .collect();
        let mut placed = 0;
        for k in 0..want {
            let best = ok
                .iter()
                .enumerate()
                .filter(|(_, q)| self.trees.iter().all(|t| t.distance(**q) > 10.0))
                .map(|(i, q)| {
                    let spread = self
                        .trees
                        .iter()
                        .map(|t| t.distance(*q))
                        .fold(28.0f32, f32::min);
                    let near = self
                        .built
                        .iter()
                        .map(|(o, _)| o.sdist(*q))
                        .fold(f32::MAX, f32::min);
                    (
                        i,
                        spread - near.min(40.0) * 0.7
                            + h01(self.seed, i as u32, k as u32, 23) * 6.0,
                    )
                })
                .max_by(|a, b| a.1.total_cmp(&b.1));
            let Some((i, _)) = best else { break };
            let q = ok[i];
            let m = self.spec.trees[self.rng.below(self.spec.trees.len() as u32) as usize].clone();
            let yaw = self.rng.range(0.0, TAU);
            // (big, old trees, like the ones shading a hand-made town)
            self.pieces
                .push(Piece::prop(&m, q, yaw).scaled(self.rng.range(1.25, 1.65)));
            self.trees.push(q);
            placed += 1;
        }
        if placed > 0 {
            *self.counts.entry("Tree".to_string()).or_default() += placed;
        }
    }

    /// A palisade along the outline: 10 m sections (stretched a touch to meet), a gate where
    /// each street leaves the town.
    fn palisade(&mut self) {
        let Some((wall, gate)) = self.kit.palisade.clone() else {
            self.notes.push(
                "Town walls need the hand-painted palisade (this style has none): no wall".into(),
            );
            return;
        };
        let poly = self.poly;
        let ring = Ring::new(poly);
        // gates: where streets cross the outline
        let mut gates: Vec<(f32, Vec2, Vec2)> = vec![];
        for st in &self.streets {
            let n = st.line.len();
            let mut prev = poly_sdf(poly, st.line.at(0.0));
            let mut s = 0.5;
            while s <= n {
                let d = poly_sdf(poly, st.line.at(s));
                if (d > 0.0) != (prev > 0.0) {
                    let at = st.line.at(s);
                    let t = st.line.tangent(s);
                    let out = if d > 0.0 { t } else { -t };
                    if !gates.iter().any(|g| ring.gap(g.0, ring.project(at)) < 16.0) {
                        gates.push((ring.project(at), at, out));
                    }
                }
                prev = d;
                s += 0.5;
            }
        }
        let mut walls = 0;
        // the wing points of each gate, in perimeter order
        let mut spans: Vec<(f32, f32, Vec2, Vec2)> = vec![];
        for (_, at, out) in &gates {
            let ax = rot2(Vec2::X, yaw_facing(*out));
            let (wa, wb) = (*at - ax * 6.25, *at + ax * 6.25);
            let (pa, pb) = (ring.project(wa), ring.project(wb));
            // order the wings along the perimeter
            let (s0, s1, p0, p1) = if ring.ahead(pa, pb) < ring.len / 2.0 {
                (pa, pb, wa, wb)
            } else {
                (pb, pa, wb, wa)
            };
            spans.push((s0, s1, p0, p1));
            self.pieces.push(Piece {
                asset: gate.clone(),
                at: *at,
                yaw: yaw_facing(*out),
                scale: Vec3::ONE,
                span: 0.0,
                role: PieceRole::Building,
                ty: None,
                obb: None,
                setback: 0.0,
            });
            self.clear.push(Obb {
                c: *at,
                ax,
                half: Vec2::new(7.0, 3.0),
            });
        }
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let runs: Vec<(f32, f32, Option<Vec2>, Option<Vec2>)> = if spans.is_empty() {
            vec![(0.0, ring.len, None, None)]
        } else {
            (0..spans.len())
                .map(|i| {
                    let a = spans[i];
                    let b = spans[(i + 1) % spans.len()];
                    let len = ring.ahead(a.1, b.0);
                    (a.1, a.1 + len, Some(a.3), Some(b.2))
                })
                .collect()
        };
        for (s0, s1, pa, pb) in runs {
            let len = s1 - s0;
            if len < 4.0 {
                continue;
            }
            let mut n = (len / 10.0).round().max(1.0) as usize;
            if len / n as f32 > 12.0 {
                n += 1;
            }
            let pts: Vec<Vec2> = (0..=n)
                .map(|k| match (k, pa, pb) {
                    (0, Some(p), _) => p,
                    (k, _, Some(p)) if k == n => p,
                    _ => ring.at(s0 + len * k as f32 / n as f32),
                })
                .collect();
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let l = a.distance(b);
                if l < 2.0 {
                    continue;
                }
                let d = (b - a) / l;
                let m = (a + b) * 0.5;
                let mut out = Vec2::new(d.y, -d.x);
                if poly_sdf(poly, m + out * 0.5) < poly_sdf(poly, m - out * 0.5) {
                    out = -out;
                }
                // a touch longer, so turns close
                let k = (l + 0.35) / 10.0;
                self.pieces.push(Piece {
                    asset: wall.clone(),
                    at: m,
                    yaw: yaw_facing(out),
                    scale: Vec3::new(k, 1.0, 1.0),
                    span: l,
                    role: PieceRole::Wall,
                    ty: None,
                    obb: None,
                    setback: 0.0,
                });
                self.clear.push(Obb {
                    c: m - out * 1.3,
                    ax: d,
                    half: Vec2::new(l * 0.5, 2.2),
                });
                walls += 1;
            }
        }
        *self
            .counts
            .entry("Palisade section".to_string())
            .or_default() += walls;
        if !gates.is_empty() {
            *self.counts.entry("Palisade gate".to_string()).or_default() += gates.len();
        }
    }
}

/// Fence sections from a to b, each stretched to fit (`pitch`: a section's length), tilting
/// to follow the ground.
fn fence_run(out: &mut Vec<Piece>, a: Vec2, b: Vec2, model: &str, pitch: f32) {
    let len = a.distance(b);
    if len < 0.6 {
        return;
    }
    let n = (len / pitch).round().max(1.0);
    let step = len / n;
    let d = (b - a) / len;
    for i in 0..n as usize {
        let c = a + d * step * (i as f32 + 0.5);
        out.push(Piece {
            asset: model.to_string(),
            at: c,
            yaw: yaw_along(d),
            scale: Vec3::new(step / pitch, 1.0, 1.0),
            span: step,
            role: PieceRole::Wall,
            ty: None,
            obb: None,
            setback: 0.0,
        });
    }
}

/// A closed outline walked by arc length.
struct Ring {
    pts: Vec<Vec2>,
    cum: Vec<f32>,
    len: f32,
}

impl Ring {
    fn new(poly: &[Vec2]) -> Ring {
        let mut pts = poly.to_vec();
        pts.push(poly[0]);
        let l = Line::new(pts);
        Ring {
            len: l.len(),
            pts: l.pts,
            cum: l.cum,
        }
    }
    fn at(&self, s: f32) -> Vec2 {
        let s = s.rem_euclid(self.len);
        let i = self
            .cum
            .partition_point(|c| *c <= s)
            .clamp(1, self.pts.len() - 1)
            - 1;
        let l = (self.cum[i + 1] - self.cum[i]).max(1e-6);
        self.pts[i].lerp(self.pts[i + 1], ((s - self.cum[i]) / l).clamp(0.0, 1.0))
    }
    fn project(&self, p: Vec2) -> f32 {
        let mut best = (0.0, f32::MAX);
        for i in 0..self.pts.len() - 1 {
            let (a, b) = (self.pts[i], self.pts[i + 1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = p.distance(a + ab * t);
            if d < best.1 {
                best = (self.cum[i] + t * ab.length(), d);
            }
        }
        best.0
    }
    /// How far ahead `b` is from `a` going round.
    fn ahead(&self, a: f32, b: f32) -> f32 {
        (b - a).rem_euclid(self.len)
    }
    /// The shorter way between two places on the ring.
    fn gap(&self, a: f32, b: f32) -> f32 {
        let d = self.ahead(a, b);
        d.min(self.len - d)
    }
}

#[cfg(test)]
mod tests {
    use super::super::kinds::{Wanted, default_counts, draw_up, parse_type, type_index};
    use super::*;

    fn handpainted_pack_installed() -> bool {
        crate::asset_file("models/stylepacks/handpainted/models.json").is_file()
    }

    fn core_style() -> Style {
        if handpainted_pack_installed() {
            Style::Handpainted
        } else {
            Style::Photoreal
        }
    }

    fn ellipse(a: f32, b: f32) -> Vec<Vec2> {
        (0..36)
            .map(|i| Vec2::from_angle(TAU * i as f32 / 36.0) * Vec2::new(a, b))
            .collect()
    }

    fn mix_for(size: TownSize, style: Style, cat: &Catalog, area: f32, seed: u32) -> Mix {
        let mut rng = Rng::new(seed as u64);
        let counts: Vec<(usize, usize)> = default_counts(size, size.buildings_for(area), &mut rng)
            .into_iter()
            .map(|(id, k)| (type_index(id).unwrap(), k))
            .collect();
        draw_up(&counts, size, style, cat, false, &mut rng)
    }

    fn spec<'a>(
        size: TownSize,
        style: Style,
        walls: bool,
        mix: &'a Mix,
        seed: u32,
        trees: &'a [String],
    ) -> Spec<'a> {
        Spec {
            size,
            style,
            walls,
            mix,
            seed,
            lead_out: true,
            obstacles: &[],
            keep_off: &[],
            trees,
        }
    }

    fn town(
        size: TownSize,
        poly: &[Vec2],
        style: Style,
        cat: &Catalog,
        seed: u32,
        walls: bool,
    ) -> Town {
        let mix = mix_for(size, style, cat, poly_area(poly), seed);
        layout(poly, &spec(size, style, walls, &mix, seed, &[]), cat, &Flat).unwrap()
    }

    /// The building counts a size's own area should give.
    fn range(size: TownSize) -> std::ops::RangeInclusive<usize> {
        match size {
            TownSize::Hamlet => 3..=4,
            TownSize::Village => 6..=9,
            TownSize::Town => 12..=16,
            TownSize::City => 24..=32,
        }
    }

    /// A hilly site with a pond: streets and buildings keep off the water and steep ground.
    struct Hills;
    impl Site for Hills {
        fn height(&self, p: Vec2) -> f32 {
            // a steep bank along the north edge
            (p.y + 30.0).min(0.0).abs() * 0.6 + (p.x * 0.05).sin() * 1.5
        }
        fn wet(&self, p: Vec2) -> bool {
            p.distance(Vec2::new(35.0, 18.0)) < 9.0
        }
    }

    /// Every building inside, dry, off the streets, at least 3 m from any other, and facing a
    /// street near it or the square, within 30°.
    fn check(t: &Town, poly: &[Vec2], site: &dyn Site) {
        let b: Vec<&Piece> = t.buildings().collect();
        let cos30 = 30f32.to_radians().cos();
        for (i, p) in b.iter().enumerate() {
            let o = p.obb.unwrap();
            for c in o.corners() {
                assert!(
                    poly_sdf(poly, c) < 0.0,
                    "{} sticks out of the outline",
                    p.asset
                );
                assert!(!site.wet(c), "{} stands in water", p.asset);
            }
            for q in &b[i + 1..] {
                assert!(
                    !o.overlaps(&q.obb.unwrap(), 3.0),
                    "{} and {} stand closer than 3 m",
                    p.asset,
                    q.asset
                );
            }
            for s in &t.streets {
                for q in s.line.samples(0.5) {
                    assert!(
                        o.sdist(q) > s.width * 0.5,
                        "{} stands on the {} at {q}",
                        p.asset,
                        s.name
                    );
                }
            }
            let front = rot2(Vec2::Y, p.yaw);
            let toward: Vec<Vec2> = t
                .streets
                .iter()
                .map(|s| s.line.at(s.line.nearest(o.c).0) - o.c)
                .collect();
            let near = toward.iter().map(|d| d.length()).fold(f32::MAX, f32::min);
            let to_street = toward
                .iter()
                .any(|d| d.length() < near + 12.0 && front.dot(d.normalize()) > cos30);
            let to_square = t.square.is_some_and(|sq| {
                sq.sdist(o.c) < 30.0 && front.dot((sq.c - o.c).normalize()) > cos30
            });
            assert!(
                to_street || to_square,
                "{} at {:?} faces {front}; the nearest street is {near:.1} m off",
                p.asset,
                o.c
            );
        }
    }

    /// `WB_TOWN_SVG=<dir> cargo test town_svgs -- --ignored`: plans of every size as SVG (for
    /// looking at the layouts without running the editor).
    #[test]
    #[ignore]
    fn town_svgs() {
        let Ok(dir) = std::env::var("WB_TOWN_SVG") else {
            return;
        };
        let cat = Catalog::bundled(|_| true);
        let style = if std::env::var("WB_TOWN_SVG_STYLE").is_ok_and(|s| s == "photoreal") {
            Style::Photoreal
        } else {
            core_style()
        };
        let trees = vec!["Oak".to_string()];
        for size in TownSize::ALL {
            for seed in [1, 2, 3] {
                let e = size.extent();
                let poly = ellipse(e.x, e.y);
                let mix = mix_for(size, style, &cat, poly_area(&poly), seed);
                let t = layout(
                    &poly,
                    &spec(size, style, false, &mix, seed, &trees),
                    &cat,
                    &Flat,
                )
                .unwrap();
                let k = 3.0;
                let (w, h) = (e.x * 2.0 * k + 80.0, e.y * 2.0 * k + 80.0);
                let tx = |p: Vec2| ((p.x + e.x) * k + 40.0, (p.y + e.y) * k + 40.0);
                let mut svg = format!(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='{w}' height='{h}'><rect width='100%' height='100%' fill='#cfe3b0'/>"
                );
                let pts = |v: &[Vec2]| {
                    v.iter()
                        .map(|p| {
                            let (x, y) = tx(*p);
                            format!("{x:.1},{y:.1}")
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                svg += &format!(
                    "<polygon points='{}' fill='none' stroke='#666' stroke-dasharray='6 4'/>",
                    pts(&poly)
                );
                if let Some(sq) = t.square {
                    svg += &format!("<polygon points='{}' fill='#bbb'/>", pts(&sq.corners()));
                }
                for st in &t.streets {
                    svg += &format!(
                        "<polyline points='{}' fill='none' stroke='#a08060' stroke-width='{}' stroke-linecap='round'/>",
                        pts(&st.line.pts),
                        st.width * k
                    );
                }
                for p in &t.pieces {
                    match (p.obb, p.role) {
                        (Some(o), _) => {
                            svg += &format!(
                                "<polygon points='{}' fill='#c0503c' stroke='#000' stroke-width='1'/>",
                                pts(&o.corners())
                            );
                            let f = o.c + rot2(Vec2::Y, p.yaw) * o.half.y;
                            let (a, b) = (tx(o.c), tx(f));
                            svg += &format!(
                                "<line x1='{:.1}' y1='{:.1}' x2='{:.1}' y2='{:.1}' stroke='#fff' stroke-width='2'/>",
                                a.0, a.1, b.0, b.1
                            );
                            if let Some(ty) = p.ty {
                                svg += &format!(
                                    "<text x='{:.1}' y='{:.1}' font-size='9' text-anchor='middle'>{}</text>",
                                    a.0,
                                    a.1 - 3.0,
                                    TYPES[ty].id
                                );
                            }
                        }
                        (None, PieceRole::Wall) => {
                            let d = rot2(Vec2::X, p.yaw) * p.span.max(1.0) * 0.5;
                            let (a, b) = (tx(p.at - d), tx(p.at + d));
                            svg += &format!(
                                "<line x1='{:.1}' y1='{:.1}' x2='{:.1}' y2='{:.1}' stroke='#5a3a1a' stroke-width='2'/>",
                                a.0, a.1, b.0, b.1
                            );
                        }
                        _ => {
                            let (x, y) = tx(p.at);
                            let (col, r) = if p.asset == "Oak" {
                                ("#2f7a2f", 5.0 * k * 0.5)
                            } else if p.asset.contains("lamp") {
                                ("#ffd000", 2.5)
                            } else {
                                ("#2050c0", 2.5)
                            };
                            svg += &format!(
                                "<circle cx='{x:.1}' cy='{y:.1}' r='{r:.1}' fill='{col}' fill-opacity='0.8'/>"
                            );
                        }
                    }
                }
                svg += "</svg>";
                std::fs::write(format!("{dir}/{}_{seed}.svg", size.id()), svg).unwrap();
                println!(
                    "{} {seed}: {} buildings {:?} {:?}",
                    size.id(),
                    t.buildings().count(),
                    t.counts,
                    t.notes
                );
            }
        }
    }

    #[test]
    fn sizes_have_few_buildings_on_room_to_spare() {
        for size in TownSize::ALL {
            let e = size.extent();
            let n = size.buildings_for(poly_area(&ellipse(e.x, e.y)));
            assert!(range(size).contains(&n), "{size:?}: {n} buildings");
            // a bigger area gets somewhat more, a smaller fewer
            let a = size.default_area();
            assert!(
                size.buildings_for(a * 2.0) > n && size.buildings_for(a * 2.0) < n * 2,
                "{size:?}"
            );
            assert!(size.buildings_for(a * 0.5) <= n);
            // an area drawn at a size's own extent is that size
            assert_eq!(TownSize::for_area(size.is_town(), a), size);
        }
    }

    #[test]
    fn layouts_are_deterministic_per_seed() {
        let cat = Catalog::bundled(|_| true);
        for size in TownSize::ALL {
            let poly = ellipse(size.extent().x, size.extent().y);
            let style = core_style();
            let a = town(size, &poly, style, &cat, 7, false);
            assert_eq!(
                a,
                town(size, &poly, style, &cat, 7, false),
                "{size:?}"
            );
            assert_ne!(
                a,
                town(size, &poly, style, &cat, 8, false),
                "{size:?}: a new seed re-rolls it"
            );
        }
    }

    /// The composition, on flat ground, for every size, both styles and several seeds: the
    /// building count, the spacing, every building facing a street or the square, no palisade,
    /// the landmark on the square, buildings spread over the area, setbacks varied.
    #[test]
    fn towns_are_composed_like_hand_made_ones() {
        let cat = Catalog::bundled(|_| true);
        for size in TownSize::ALL {
            for style in [Style::Handpainted, Style::Photoreal]
                .into_iter()
                .filter(|style| *style != Style::Handpainted || handpainted_pack_installed())
            {
                for seed in [1, 2, 3, 4] {
                    let e = size.extent();
                    let poly = ellipse(e.x, e.y);
                    let t = town(size, &poly, style, &cat, seed, false);
                    let what = format!("{size:?} {style:?} {seed}");
                    let b: Vec<&Piece> = t.buildings().collect();
                    assert!(
                        range(size).contains(&b.len()),
                        "{what}: {} buildings ({:?})",
                        b.len(),
                        t.notes
                    );
                    assert!(
                        t.notes.iter().all(|n| !n.contains("didn't fit")),
                        "{what}: {:?}",
                        t.notes
                    );
                    check(&t, &poly, &Flat);
                    // well apart: the typical nearest neighbour is several metres off
                    let mut gaps: Vec<f32> = b
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            b.iter()
                                .enumerate()
                                .filter(|(j, _)| *j != i)
                                .map(|(_, q)| p.obb.unwrap().gap_to(&q.obb.unwrap()))
                                .fold(f32::MAX, f32::min)
                        })
                        .collect();
                    gaps.sort_by(f32::total_cmp);
                    assert!(gaps[gaps.len() / 2] >= 5.0, "{what}: nearest gaps {gaps:?}");
                    assert!(
                        t.pieces.iter().all(|p| !p.asset.contains("palisade")),
                        "{what}: no palisade unless asked"
                    );
                    let sq = t.square.unwrap_or_else(|| panic!("{what}: no square"));
                    let lm = t
                        .landmark
                        .clone()
                        .unwrap_or_else(|| panic!("{what}: no landmark"));
                    assert!(
                        t.pieces
                            .iter()
                            .any(|p| p.asset == lm && p.at.distance(sq.c) < 3.0),
                        "{what}: the {lm} isn't in the middle"
                    );
                    // spread over the area, not bunched on one side
                    let (lo, hi) = bounds(&b.iter().map(|p| p.obb.unwrap().c).collect::<Vec<_>>());
                    let share = if size == TownSize::Hamlet { 0.35 } else { 0.45 };
                    assert!(
                        hi.x - lo.x > e.x * 2.0 * share,
                        "{what}: buildings span {:.0} m of {:.0}",
                        hi.x - lo.x,
                        e.x * 2.0
                    );
                    // never a straight row: setbacks along the streets vary
                    let lots: Vec<f32> = b
                        .iter()
                        .filter(|p| p.setback > 0.0)
                        .map(|p| p.setback)
                        .collect();
                    if lots.len() >= 3 {
                        let (mn, mx) = lots
                            .iter()
                            .fold((f32::MAX, f32::MIN), |(a, z), s| (a.min(*s), z.max(*s)));
                        assert!(mx - mn > 1.0, "{what}: setbacks {lots:?}");
                    }
                    if style == Style::Photoreal {
                        assert!(
                            t.pieces.iter().all(|p| !p.asset.starts_with("hp:")),
                            "photoreal uses no hand-painted models"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn key_buildings_frame_the_centre() {
        if !handpainted_pack_installed() {
            return; // asserts the optional pack's named landmark models
        }
        let cat = Catalog::bundled(|_| true);
        let faces_square = |t: &Town, p: &Piece| {
            let (sq, o) = (t.square.unwrap(), p.obb.unwrap());
            sq.sdist(o.c) < 25.0 && rot2(Vec2::Y, p.yaw).dot((sq.c - o.c).normalize()) > 0.8
        };
        for seed in [1, 2, 3, 5] {
            let e = TownSize::Town.extent();
            let poly = ellipse(e.x, e.y);
            let t = town(TownSize::Town, &poly, Style::Handpainted, &cat, seed, false);
            for name in ["hp:town_hall", "hp:inn", "hp:chapel"] {
                let b = t
                    .pieces
                    .iter()
                    .find(|p| p.asset == name)
                    .unwrap_or_else(|| panic!("{seed}: {name}"));
                assert!(
                    faces_square(&t, b),
                    "{seed}: the {name} doesn't face the square"
                );
            }
            assert!(
                t.counts["Market stall"] >= 2 && t.counts["Lamp post"] >= 4,
                "{seed}: {:?}",
                t.counts
            );
            // the smithy lines the main road; the farmhouse and the windmill or tower stand out on the edge
            let hub = t.square.unwrap().c;
            let dist = |p: &Piece| p.obb.unwrap().c.distance(hub);
            let homes: Vec<f32> = t
                .buildings()
                .filter(|p| p.asset.contains("house_"))
                .map(dist)
                .collect();
            let mid = homes.iter().sum::<f32>() / homes.len() as f32;
            let smithy = t
                .pieces
                .iter()
                .find(|p| p.asset == "hp:blacksmith")
                .unwrap();
            let main = &t.streets[0].line;
            assert!(
                main.nearest(smithy.obb.unwrap().c).1 < 20.0,
                "{seed}: the smithy is off the road"
            );
            let farm = t.pieces.iter().find(|p| p.asset == "hp:farmhouse").unwrap();
            assert!(
                dist(farm) > mid,
                "{seed}: the farm ({:.0} m out) is nearer than the homes ({mid:.0})",
                dist(farm)
            );
            let v = town(
                TownSize::Village,
                &ellipse(80.0, 60.0),
                Style::Handpainted,
                &cat,
                seed,
                false,
            );
            let inn = v.pieces.iter().find(|p| p.asset == "hp:inn").unwrap();
            assert!(
                faces_square(&v, inn),
                "{seed}: the village inn doesn't face the green"
            );
            assert_eq!(v.streets[0].kind, RoadKind::DirtRoad);
            assert!(
                v.streets
                    .iter()
                    .skip(1)
                    .all(|s| matches!(s.kind, RoadKind::Footpath | RoadKind::Trail))
            );
            // a village's lamps are few
            assert!(
                v.pieces
                    .iter()
                    .filter(|p| p.asset == "hp:lamp_post")
                    .count()
                    <= 4
            );
            // dressing stays sparse
            let props = v
                .pieces
                .iter()
                .filter(|p| p.role == PieceRole::Prop)
                .count();
            assert!(props < 40, "{seed}: {props} props in a village");
        }
    }

    #[test]
    fn trees_grow_between_the_buildings() {
        let cat = Catalog::bundled(|_| true);
        let trees = vec!["Oak".to_string(), "Maple".to_string()];
        for size in [TownSize::Village, TownSize::Town] {
            let e = size.extent();
            let poly = ellipse(e.x, e.y);
            let style = core_style();
            let mix = mix_for(size, style, &cat, poly_area(&poly), 3);
            let t = layout(
                &poly,
                &spec(size, style, false, &mix, 3, &trees),
                &cat,
                &Flat,
            )
            .unwrap();
            let planted: Vec<&Piece> = t
                .pieces
                .iter()
                .filter(|p| trees.contains(&p.asset))
                .collect();
            assert!(planted.len() >= 4, "{size:?}: {} trees", planted.len());
            for p in planted {
                assert!(
                    t.buildings().all(|b| b.obb.unwrap().sdist(p.at) > 4.0),
                    "{size:?}: a tree by a wall"
                );
                assert!(
                    t.streets
                        .iter()
                        .all(|s| s.line.nearest(p.at).1 > s.width * 0.5 + 3.0),
                    "{size:?}: a tree on a street"
                );
            }
            check(&t, &poly, &Flat);
        }
    }

    #[test]
    fn towns_keep_off_water_and_steep_ground() {
        let cat = Catalog::bundled(|_| true);
        let e = TownSize::Town.extent();
        let poly = ellipse(e.x, e.y);
        let mix = mix_for(
            TownSize::Town,
            core_style(),
            &cat,
            poly_area(&poly),
            4,
        );
        let t = layout(
            &poly,
            &spec(TownSize::Town, core_style(), false, &mix, 4, &[]),
            &cat,
            &Hills,
        )
        .unwrap();
        check(&t, &poly, &Hills);
        assert!(t.buildings().count() >= 10, "{:?}", t.notes);
    }

    #[test]
    fn a_requested_mix_is_placed_as_asked() {
        if !handpainted_pack_installed() {
            return; // exercises named optional-pack models and their locked variants
        }
        let cat = Catalog::bundled(|_| true);
        let poly = ellipse(80.0, 60.0);
        let mut rng = Rng::new(1);
        let mut counts = vec![];
        for (w, k) in [("blacksmith", 1), ("inn", 1), ("houses", 9), ("bakery", 2)] {
            let Some(Wanted::Building(i)) = parse_type(w) else {
                panic!("{w}")
            };
            counts.push((i, k));
        }
        let mix = draw_up(
            &counts,
            TownSize::Village,
            Style::Handpainted,
            &cat,
            true,
            &mut rng,
        );
        let t = layout(
            &poly,
            &spec(TownSize::Village, Style::Handpainted, false, &mix, 2, &[]),
            &cat,
            &Flat,
        )
        .unwrap();
        assert_eq!(t.counts.get("Blacksmith"), Some(&1));
        assert_eq!(t.counts.get("Inn"), Some(&1));
        assert_eq!(t.counts.get("Bakery"), Some(&2));
        assert_eq!(t.counts.get("House"), Some(&9));
        assert_eq!(t.buildings().count(), 13, "{:?}", t.counts);
        check(&t, &poly, &Flat);
        // a locked pack: its buildings are skipped with a note, the rest placed
        let locked = Catalog::bundled(|id| id != "hp:inn" && id != "hp:bakery");
        let mix = draw_up(
            &counts,
            TownSize::Village,
            Style::Handpainted,
            &locked,
            true,
            &mut rng,
        );
        let t = layout(
            &poly,
            &spec(TownSize::Village, Style::Handpainted, false, &mix, 2, &[]),
            &locked,
            &Flat,
        )
        .unwrap();
        assert_eq!(t.counts.get("Inn"), None);
        assert_eq!(t.buildings().count(), 10);
        assert!(
            t.notes
                .iter()
                .filter(|n| n.contains("hasn't unlocked"))
                .count()
                == 2,
            "{:?}",
            t.notes
        );
        assert!(t.pieces.iter().all(|p| locked.has(&p.asset)));
    }

    #[test]
    fn towns_build_round_what_stands_there() {
        let cat = Catalog::bundled(|_| true);
        let e = TownSize::Town.extent();
        let poly = ellipse(e.x, e.y);
        let mix = mix_for(
            TownSize::Town,
            core_style(),
            &cat,
            poly_area(&poly),
            6,
        );
        // a tower already standing near the middle, an old road running across the east side
        let tower = Obb {
            c: Vec2::new(30.0, -22.0),
            ax: Vec2::new(0.8, 0.6),
            half: Vec2::new(5.0, 5.0),
        };
        let road: Vec<(Vec2, f32)> = (0..=180)
            .map(|i| (Vec2::new(60.0 + i as f32 * 0.2, -90.0 + i as f32), 2.5))
            .collect();
        let s = Spec {
            obstacles: &[tower],
            keep_off: &road,
            ..spec(TownSize::Town, core_style(), false, &mix, 6, &[])
        };
        let t = layout(&poly, &s, &cat, &Flat).unwrap();
        assert!(t.buildings().count() >= 12, "{:?}", t.notes);
        for b in t.buildings() {
            let o = b.obb.unwrap();
            assert!(!o.overlaps(&tower, 0.0), "{} on the tower", b.asset);
            assert!(
                road.iter().all(|(p, hw)| o.sdist(*p) > *hw),
                "{} on the old road",
                b.asset
            );
        }
        // a selection's streets stop just past its edge
        let s = Spec {
            lead_out: false,
            ..spec(TownSize::Town, core_style(), false, &mix, 6, &[])
        };
        let t = layout(&poly, &s, &cat, &Flat).unwrap();
        for st in &t.streets {
            for q in st.line.samples(1.0) {
                assert!(poly_sdf(&poly, q) < 4.0, "the {} runs out to {q}", st.name);
            }
        }
    }

    #[test]
    fn walls_close_round_the_town_with_gates_on_the_streets() {
        if !handpainted_pack_installed() {
            return; // asserts the optional pack's palisade modules
        }
        let cat = Catalog::bundled(|_| true);
        let e = TownSize::City.extent();
        let poly = ellipse(e.x, e.y);
        let t = town(TownSize::City, &poly, Style::Handpainted, &cat, 3, true);
        let walls: Vec<&Piece> = t
            .pieces
            .iter()
            .filter(|p| p.asset == "hp:palisade_wall")
            .collect();
        let gates: Vec<&Piece> = t
            .pieces
            .iter()
            .filter(|p| p.asset == "hp:palisade_gate")
            .collect();
        assert_eq!(gates.len(), 2, "both ends of the main road");
        // sections are the modular 10 m, barely stretched, and run all round
        let total: f32 = walls.iter().map(|w| w.span).sum::<f32>() + gates.len() as f32 * 12.5;
        let perim = crate::water::lakes::perimeter(&poly);
        assert!(
            (total - perim).abs() < perim * 0.04,
            "{total} m of wall round {perim} m"
        );
        assert!(
            walls.iter().all(|w| (0.65..=1.25).contains(&w.scale.x)),
            "{:?}",
            walls.iter().map(|w| w.scale.x).collect::<Vec<_>>()
        );
        // facing out
        for w in &walls {
            let out = rot2(Vec2::Y, w.yaw);
            assert!(poly_sdf(&poly, w.at + out * 2.0) > poly_sdf(&poly, w.at - out * 2.0));
        }
        // buildings keep clear of the wall
        for b in t.buildings() {
            for c in b.obb.unwrap().corners() {
                assert!(poly_sdf(&poly, c) < -6.0, "{} by the wall", b.asset);
            }
        }
    }
}
