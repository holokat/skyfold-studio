//! Laying a temple compound out on a walled realm's land: where the entrance plaza, the hub, the
//! boss courts, the ceremonial axis and the final court go, the lakes and the island temple in
//! the low ground between them, the winding trails that join them, the camps along those trails,
//! and the ruins and tree clumps scattered between.
//!
//! It reads the land's heights only (the realm's own, or a coarse preview of them), so the same
//! land and seed always give the same compound. The recipe: the valley floor's long axis runs
//! from the entrance end (made the open edge, behind a perimeter wall) to the far end, where the
//! final court stands; the hub sits in the middle; a straight processional way runs from the
//! hub's gate through a round plaza to the final court's terrace; the boss courts are spread
//! round the hub on both sides of the axis (or one after another, for a linear compound), each
//! as far out as its bay of the floor allows, each a raised platform of its own shape with its
//! stairs toward its trail.

use bevy::math::Vec2;
use serde::Serialize;
use serde_json::{Value, json};

use super::super::geom;
use super::theme::{Order, Theme, boss_name, fill};
use crate::noise::{Rng, fbm};
use crate::terrain::layers::Layer;

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

/// Heights on a square grid (`n`² vertices, `cell` m apart, centred on the origin).
pub struct Grid<'a> {
    pub n: usize,
    pub cell: f32,
    pub h: &'a [f32],
}

impl Grid<'_> {
    pub fn half(&self) -> f32 {
        (self.n - 1) as f32 * self.cell * 0.5
    }
    pub fn xz(&self, k: usize) -> Vec2 {
        Vec2::new(
            (k % self.n) as f32 * self.cell - self.half(),
            (k / self.n) as f32 * self.cell - self.half(),
        )
    }
    /// Height at a world position (bilinear, clamped to the map).
    pub fn at(&self, p: Vec2) -> f32 {
        sample(self.h, self.n, self.cell, p)
    }
}

fn sample(v: &[f32], n: usize, cell: f32, p: Vec2) -> f32 {
    let half = (n - 1) as f32 * cell * 0.5;
    let g = ((p + Vec2::splat(half)) / cell).clamp(Vec2::ZERO, Vec2::splat((n - 1) as f32 - 1e-3));
    let (i, j) = (g.x.floor() as usize, g.y.floor() as usize);
    let (fx, fz) = (g.x - i as f32, g.y - j as f32);
    let i1 = (i + 1).min(n - 1);
    let j1 = (j + 1).min(n - 1);
    let a = v[j * n + i] * (1.0 - fx) + v[j * n + i1] * fx;
    let b = v[j1 * n + i] * (1.0 - fx) + v[j1 * n + i1] * fx;
    a * (1.0 - fz) + b * fz
}

// ------------------------------------------------------------------ the valley floor

/// The level floor of a walled realm: which ground it is, how far each point is from its walls,
/// and its long axis.
pub struct Floor {
    pub level: f32,
    n: usize,
    cell: f32,
    mask: Vec<bool>,
    room: Vec<f32>,
    pub area: f32,
    pub centroid: Vec2,
    /// the floor's long axis (unit)
    pub axis: Vec2,
}

impl Floor {
    pub fn find(g: &Grid) -> Option<Floor> {
        let n = g.n;
        let mut land: Vec<f32> = g.h.iter().copied().filter(|h| *h > 1.0).collect();
        if land.len() < 200 {
            return None;
        }
        land.sort_by(|a, b| a.total_cmp(b));
        let level = land[land.len() * 12 / 100];
        let low: Vec<bool> = g.h.iter().map(|h| *h > 0.5 && *h < level + 3.0).collect();
        // the low ground joined to the middle of the map
        let start = (0..n * n).filter(|k| low[*k]).min_by(|a, b| {
            g.xz(*a)
                .length_squared()
                .total_cmp(&g.xz(*b).length_squared())
        })?;
        let mut mask = vec![false; n * n];
        let mut stack = vec![start];
        mask[start] = true;
        while let Some(k) = stack.pop() {
            let (i, j) = (k % n, k / n);
            for (a, b) in [
                (i.wrapping_sub(1), j),
                (i + 1, j),
                (i, j.wrapping_sub(1)),
                (i, j + 1),
            ] {
                if a < n && b < n && !mask[b * n + a] && low[b * n + a] {
                    mask[b * n + a] = true;
                    stack.push(b * n + a);
                }
            }
        }
        let count = mask.iter().filter(|m| **m).count();
        let area = count as f32 * g.cell * g.cell;
        if area < 15_000.0 {
            return None;
        }
        // (the map's border counts as wall)
        let mut wall: Vec<bool> = mask.iter().map(|m| !m).collect();
        for i in 0..n {
            for k in [i, (n - 1) * n + i, i * n, i * n + n - 1] {
                wall[k] = true;
            }
        }
        let room = chamfer(&wall, n, g.cell);
        let (mut c, mut w) = (Vec2::ZERO, 0.0);
        for k in (0..n * n).filter(|k| mask[*k]) {
            c += g.xz(k);
            w += 1.0;
        }
        let c = c / w;
        let (mut cxx, mut cxz, mut czz) = (0.0f32, 0.0f32, 0.0f32);
        for k in (0..n * n).filter(|k| mask[*k]) {
            let d = g.xz(k) - c;
            cxx += d.x * d.x;
            cxz += d.x * d.y;
            czz += d.y * d.y;
        }
        let a = 0.5 * (2.0 * cxz).atan2(cxx - czz);
        Some(Floor {
            level,
            n,
            cell: g.cell,
            mask,
            room,
            area,
            centroid: c,
            axis: Vec2::from_angle(a),
        })
    }
    /// How far each vertex is outside the floor (m; 0 on it).
    pub fn outside(&self) -> Vec<f32> {
        chamfer(&self.mask, self.n, self.cell)
    }
    /// The room on a grid `f` times coarser.
    pub fn coarse(&self, f: usize) -> RoomMap {
        let n2 = (self.n - 1) / f + 1;
        let mut v = vec![0.0; n2 * n2];
        for j in 0..n2 {
            for i in 0..n2 {
                v[j * n2 + i] =
                    self.room[(j * f).min(self.n - 1) * self.n + (i * f).min(self.n - 1)];
            }
        }
        RoomMap {
            n: n2,
            cell: self.cell * f as f32,
            v,
        }
    }
    /// How far `p` is from the floor's walls (m; 0 off the floor).
    pub fn room(&self, p: Vec2) -> f32 {
        sample(&self.room, self.n, self.cell, p)
    }
    pub fn on(&self, p: Vec2) -> bool {
        self.room(p) > 0.5
    }
    fn half(&self) -> f32 {
        (self.n - 1) as f32 * self.cell * 0.5
    }
    /// The floor's points (every `step`-th vertex each way) with their room.
    fn points(&self, step: usize) -> Vec<(Vec2, f32)> {
        let half = self.half();
        let mut v = vec![];
        for j in (0..self.n).step_by(step) {
            for i in (0..self.n).step_by(step) {
                let k = j * self.n + i;
                if self.mask[k] {
                    v.push((
                        Vec2::new(i as f32 * self.cell - half, j as f32 * self.cell - half),
                        self.room[k],
                    ));
                }
            }
        }
        v
    }
    /// The floor's outline (its walls' foot, `inset` m in).
    pub fn outline(&self, inset: f32) -> Vec<Vec<Vec2>> {
        let half = self.half();
        geom::contours(
            Vec2::splat(-half),
            Vec2::splat(half),
            self.cell * 2.0,
            |p| self.room(p) - inset,
        )
    }
}

/// Distance (m) from every vertex to the nearest `seed` vertex (two-pass chamfer).
pub(crate) fn chamfer(seed: &[bool], n: usize, cell: f32) -> Vec<f32> {
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

// ------------------------------------------------------------------ what a compound is

/// The shape of a boss court's platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CourtKind {
    /// a cross-shaped yard (four arms)
    CrossYard,
    /// a long pillared hall (two rows of pillars)
    PillaredHall,
    /// a deep platform with a stepped pyramid at its back
    SteppedPyramid,
    /// a round shrine ringed by pillars and fire
    RoundShrine,
    /// an octagonal arena
    Octagon,
    /// a temple on an island in a lake, reached by a bridge
    IslandTemple,
    /// the final court: a two-tier throne terrace at the end of the axis
    ThroneTerrace,
}

impl CourtKind {
    pub fn name(self) -> &'static str {
        match self {
            CourtKind::CrossYard => "Cross-shaped yard",
            CourtKind::PillaredHall => "Pillared hall",
            CourtKind::SteppedPyramid => "Stepped pyramid",
            CourtKind::RoundShrine => "Round shrine",
            CourtKind::Octagon => "Octagonal arena",
            CourtKind::IslandTemple => "Island temple",
            CourtKind::ThroneTerrace => "Throne terrace",
        }
    }
    /// How high its platform stands over the floor (m).
    pub fn rise(self) -> f32 {
        match self {
            CourtKind::CrossYard => 3.5,
            CourtKind::PillaredHall => 4.0,
            CourtKind::SteppedPyramid => 5.5,
            CourtKind::RoundShrine => 4.5,
            CourtKind::Octagon => 4.0,
            CourtKind::IslandTemple => 2.6,
            CourtKind::ThroneTerrace => 4.5,
        }
    }
}

/// A raised court: where it stands, which way its stairs face, its outline and floor.
#[derive(Clone, Debug, Serialize)]
pub struct Court {
    pub name: String,
    pub boss: String,
    pub kind: CourtKind,
    #[serde(serialize_with = "ser_v2")]
    pub c: Vec2,
    /// towards its stairs (and its trail)
    #[serde(serialize_with = "ser_v2")]
    pub f: Vec2,
    /// half extents across (x) and along its facing (z); a round or octagonal court's radius is
    /// `hx`
    pub hx: f32,
    pub hz: f32,
    /// the platform's top (absolute height, m)
    pub top: f32,
    /// the stair: where its head meets the platform's edge, its foot on the ground, its width
    #[serde(serialize_with = "ser_v2")]
    pub stair_head: Vec2,
    #[serde(serialize_with = "ser_v2")]
    pub stair_foot: Vec2,
    pub stair_w: f32,
    #[serde(skip)]
    pub floor: Layer,
    /// the boss's level, and the order it is met in (1…)
    pub level: u8,
    pub order: usize,
    /// a lake's island it stands on (lake index)
    pub island: Option<usize>,
    /// the throne terrace's upper tier: its top, half extents and how far back it sits
    pub top2: f32,
    pub hx2: f32,
    pub hz2: f32,
    pub back2: f32,
    /// the forecourt: a lower terrace in front of the main one, the court's stairs coming down
    /// off its front (half extents across and along; none when `fz` is 0) and its top
    pub fx: f32,
    pub fz: f32,
    pub ftop: f32,
    /// the inner stair, from the main terrace down onto the forecourt
    #[serde(skip)]
    pub inner_head: Vec2,
    #[serde(skip)]
    pub inner_foot: Vec2,
}

impl Court {
    /// How far in front of its centre the main terrace's front edge is.
    pub fn edge(&self) -> f32 {
        match self.kind {
            CourtKind::RoundShrine | CourtKind::IslandTemple | CourtKind::Octagon => self.hx,
            _ => self.hz,
        }
    }
    /// The forecourt's centre in the court's frame (z).
    pub fn fore_z(&self) -> f32 {
        self.edge() + self.fz
    }
    /// The main terrace's signed distance (positive inside).
    pub fn sdf_main(&self, p: Vec2) -> f32 {
        let d = p - self.c;
        self.sdf_local(Vec2::new(d.dot(side(self.f)), d.dot(self.f)))
    }
    /// The forecourt's signed distance (positive inside; far outside when there is none).
    pub fn sdf_fore(&self, p: Vec2) -> f32 {
        if self.fz <= 0.0 {
            return -1e6;
        }
        let d = p - self.c;
        rect_sdf(
            Vec2::new(d.dot(side(self.f)), d.dot(self.f) - self.fore_z()),
            Vec2::new(self.fx, self.fz),
        )
    }
    /// The whole court's outline points in its own frame (corners, the stair foot).
    fn extent_pts(&self) -> Vec<Vec2> {
        let mut v = vec![
            Vec2::new(self.hx, self.hz),
            Vec2::new(-self.hx, self.hz),
            Vec2::new(self.hx, -self.hz),
            Vec2::new(-self.hx, -self.hz),
        ];
        if matches!(
            self.kind,
            CourtKind::RoundShrine
                | CourtKind::IslandTemple
                | CourtKind::Octagon
                | CourtKind::CrossYard
        ) {
            v = (0..16)
                .map(|i| Vec2::from_angle(i as f32 / 16.0 * TAU) * self.hx)
                .collect();
        }
        if self.fz > 0.0 {
            let z = self.fore_z();
            v.extend([
                Vec2::new(self.fx, z + self.fz),
                Vec2::new(-self.fx, z + self.fz),
                Vec2::new(self.fx, z - self.fz),
                Vec2::new(-self.fx, z - self.fz),
            ]);
        }
        let d = self.stair_foot - self.c;
        v.push(Vec2::new(d.dot(side(self.f)), d.dot(self.f)));
        v
    }
    /// The world points round the whole court (for checking it stands on the floor).
    pub fn extent_world(&self) -> Vec<Vec2> {
        self.extent_pts()
            .into_iter()
            .map(|q| self.at(q.x, q.y))
            .collect()
    }
    /// A circle round the whole court (centre, radius): for spacing.
    pub fn bound(&self) -> (Vec2, f32) {
        let pts = self.extent_pts();
        let (lo, hi) = pts
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.y), b.max(q.y)));
        let mz = (lo + hi) * 0.5;
        let r = pts
            .iter()
            .map(|q| q.distance(Vec2::new(0.0, mz)))
            .fold(0.0f32, f32::max);
        (self.at(0.0, mz), r)
    }
    /// A point in the court's frame (x across, z towards its stairs) in the world.
    pub fn at(&self, x: f32, z: f32) -> Vec2 {
        self.c + side(self.f) * x + self.f * z
    }
    /// How far its main terrace reaches from its centre (for spacing).
    pub fn reach(&self) -> f32 {
        match self.kind {
            CourtKind::RoundShrine
            | CourtKind::Octagon
            | CourtKind::IslandTemple
            | CourtKind::CrossYard => self.hx,
            _ => (self.hx * self.hx + self.hz * self.hz).sqrt(),
        }
    }
    /// Signed distance to the platform's outline (positive inside), in the court's frame.
    pub fn sdf_local(&self, q: Vec2) -> f32 {
        match self.kind {
            CourtKind::RoundShrine | CourtKind::IslandTemple => self.hx - q.length(),
            CourtKind::Octagon => {
                // a regular octagon of inradius hx
                let a = q.abs();
                let d =
                    a.x.max(a.y)
                        .max((a.x + a.y) * std::f32::consts::FRAC_1_SQRT_2);
                self.hx - d
            }
            CourtKind::CrossYard => {
                let arm = self.hx * ARM;
                rect_sdf(q, Vec2::new(arm, self.hz)).max(rect_sdf(q, Vec2::new(self.hx, arm)))
            }
            _ => rect_sdf(q, Vec2::new(self.hx, self.hz)),
        }
    }
    /// The whole court's signed distance: main terrace and forecourt.
    pub fn sdf(&self, p: Vec2) -> f32 {
        self.sdf_main(p).max(self.sdf_fore(p))
    }
    /// The upper tier's signed distance (the throne terrace), positive inside.
    pub fn sdf2(&self, p: Vec2) -> f32 {
        if self.hx2 <= 0.0 {
            return -1e6;
        }
        let d = p - self.c;
        let q = Vec2::new(d.dot(side(self.f)), d.dot(self.f) + self.back2);
        rect_sdf(q, Vec2::new(self.hx2, self.hz2))
    }
}

/// A cross yard's arms: their half width over its half size.
pub const ARM: f32 = 0.62;

fn rect_sdf(q: Vec2, h: Vec2) -> f32 {
    let d = q.abs() - h;
    -(d.max(Vec2::ZERO).length() + d.x.max(d.y).min(0.0))
}

/// The left-hand side of a facing (x of a court's frame).
pub fn side(f: Vec2) -> Vec2 {
    Vec2::new(-f.y, f.x)
}

fn ser_v2<S: serde::Serializer>(v: &Vec2, s: S) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    [r1(v.x), r1(v.y)].serialize(s)
}

fn r1(x: f32) -> f32 {
    (x * 10.0).round() / 10.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrailKind {
    /// from the entrance plaza to the hub
    Main,
    /// out to a court
    Spoke,
    /// a stone bridge over water (to the island)
    Bridge,
    /// the processional way to the final court
    Axis,
}

#[derive(Clone, Debug, Serialize)]
pub struct Trail {
    pub name: String,
    pub kind: TrailKind,
    #[serde(skip)]
    pub pts: Vec<Vec2>,
    pub width: f32,
    /// the court it leads to
    pub to: Option<usize>,
}

impl Trail {
    pub fn len(&self) -> f32 {
        geom::polyline_len(&self.pts)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LakePlan {
    pub name: String,
    #[serde(serialize_with = "ser_v2")]
    pub c: Vec2,
    pub r: f32,
    #[serde(skip)]
    pub outline: Vec<Vec2>,
    /// the court on its island
    pub island: Option<usize>,
}

/// A trash camp beside a trail.
#[derive(Clone, Debug, Serialize)]
pub struct Camp {
    pub name: String,
    #[serde(serialize_with = "ser_v2")]
    pub c: Vec2,
    pub r: f32,
    /// facing the trail
    #[serde(skip)]
    pub f: Vec2,
    pub level: [u8; 2],
    pub trail: usize,
}

/// A ruin or monument standing loose between the courts.
#[derive(Clone, Debug, Serialize)]
pub struct Ruin {
    pub asset: &'static str,
    pub scale: f32,
    #[serde(serialize_with = "ser_v2")]
    pub p: Vec2,
    pub yaw_deg: f32,
    /// stretched along its own axes (a broken pillar's stump, a tumbled wall)
    #[serde(skip)]
    pub scale3: Option<[f32; 3]>,
}

impl Ruin {
    fn new(asset: &'static str, scale: f32, p: Vec2, yaw_deg: f32) -> Ruin {
        Ruin {
            asset,
            scale,
            p,
            yaw_deg,
            scale3: None,
        }
    }
    /// Cut down to `h` of its height.
    fn broken(mut self, h: f32) -> Ruin {
        self.scale3 = Some([self.scale, self.scale * h, self.scale]);
        self
    }
}

/// How far the valley floor is from its walls, on a coarse grid (for the nature).
#[derive(Clone, Debug, Default)]
pub struct RoomMap {
    n: usize,
    cell: f32,
    v: Vec<f32>,
}

impl RoomMap {
    pub fn at(&self, p: Vec2) -> f32 {
        if self.v.is_empty() {
            return 100.0;
        }
        sample(&self.v, self.n, self.cell, p)
    }
}

/// The entrance plaza at the open end: its paving, stairs toward the hub, the wall behind it.
#[derive(Clone, Debug, Serialize)]
pub struct Entrance {
    #[serde(serialize_with = "ser_v2")]
    pub c: Vec2,
    pub r: f32,
    /// into the compound (toward the hub)
    #[serde(serialize_with = "ser_v2")]
    pub f: Vec2,
    pub top: f32,
    #[serde(serialize_with = "ser_v2")]
    pub stair_foot: Vec2,
    /// where players arrive (the way out stands behind them)
    #[serde(serialize_with = "ser_v2")]
    pub start: Vec2,
    /// the perimeter wall along the open edge
    #[serde(skip)]
    pub wall: Vec<Vec2>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Hub {
    pub name: String,
    #[serde(serialize_with = "ser_v2")]
    pub c: Vec2,
    pub r: f32,
    pub top: f32,
}

/// The ceremonial axis: the gate at the hub, a round plaza, then the final court's stairs.
#[derive(Clone, Debug, Serialize)]
pub struct Axis {
    #[serde(serialize_with = "ser_v2")]
    pub gate: Vec2,
    #[serde(serialize_with = "ser_v2")]
    pub end: Vec2,
    #[serde(serialize_with = "ser_v2")]
    pub dir: Vec2,
    pub hw: f32,
    #[serde(serialize_with = "ser_v2")]
    pub plaza: Vec2,
    pub plaza_r: f32,
    /// heights at the gate and at the end
    pub h0: f32,
    pub h1: f32,
    /// opens once every court has fallen (hub compounds)
    pub gated: bool,
}

impl Axis {
    pub fn len(&self) -> f32 {
        self.gate.distance(self.end)
    }
    /// The way's height at `p` (along it).
    pub fn height(&self, p: Vec2) -> f32 {
        let t = ((p - self.gate).dot(self.dir) / self.len().max(1.0)).clamp(0.0, 1.0);
        self.h0 + (self.h1 - self.h0) * t
    }
    pub fn dist(&self, p: Vec2) -> f32 {
        geom::seg_dist2(p, self.gate, self.end).sqrt()
    }
}

/// A whole compound, ready to be shaped into the land and built.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub theme: Theme,
    pub order: Order,
    pub seed: u32,
    pub floor: f32,
    /// how much bigger than a small realm's compound it is laid out (spacing)
    pub scale: f32,
    /// how much bigger its courts and their temples are made
    pub grand: f32,
    pub entrance: Entrance,
    pub hub: Hub,
    pub axis: Axis,
    pub courts: Vec<Court>,
    #[serde(rename = "final")]
    pub final_court: Court,
    pub lakes: Vec<LakePlan>,
    pub trails: Vec<Trail>,
    pub camps: Vec<Camp>,
    pub ruins: Vec<Ruin>,
    #[serde(skip)]
    pub clumps: Vec<(Vec2, f32)>,
    /// the outer temple district's quarters (centre, reach)
    #[serde(skip)]
    pub district: Vec<(Vec2, f32)>,
    /// how far the floor is from the valley's walls
    #[serde(skip)]
    pub room: RoomMap,
    /// the courts that didn't fit (the land was too small for that many)
    pub dropped: usize,
    /// the open edge's direction from the entrance (behind the wall)
    #[serde(skip)]
    pub open: Vec2,
}

/// What a compound's layout is asked for.
#[derive(Clone, Copy, Debug)]
pub struct Ask {
    pub theme: Theme,
    pub courts: usize,
    pub order: Order,
    pub seed: u32,
    /// the dungeon's level range
    pub levels: [u8; 2],
}

struct Sizes {
    k: f32,
    kk: f32,
    plaza: f32,
    hub: f32,
    court: f32,
    axis_hw: f32,
    stair_w: f32,
    final_hx: f32,
    final_hz: f32,
}

// ------------------------------------------------------------------ laying it out

/// Lays a compound out on the land `g`. Err when the land has no floor to build on.
pub fn lay_out(g: &Grid, ask: &Ask) -> Result<Plan, String> {
    let floor = Floor::find(g).ok_or(
        "this land has no level floor to build a compound on (make it an enclosed valley)",
    )?;
    let mut rng = Rng::new(ask.seed as u64 ^ 0xc0a1_7e3b_5eed);
    let k = (floor.area / 300_000.0).sqrt().clamp(0.55, 1.9);
    let kk = k.powf(0.7).clamp(0.9, 1.6);
    let sz = Sizes {
        k,
        kk,
        plaza: 22.0 * kk,
        hub: 30.0 * kk,
        court: 25.0 * kk,
        axis_hw: 7.0 * kk,
        stair_w: 9.0 * kk,
        final_hx: 30.0 * kk,
        final_hz: 34.0 * kk,
    };
    let mut axis = floor.axis;
    if rng.f32() < 0.5 {
        axis = -axis;
    }
    let c0 = floor.centroid;
    let pts = floor.points(2);
    let s_of = |p: Vec2| (p - c0).dot(axis);
    let (mut s_lo, mut s_hi) = (f32::MAX, f32::MIN);
    for (p, _) in &pts {
        s_lo = s_lo.min(s_of(*p));
        s_hi = s_hi.max(s_of(*p));
    }
    let len = (s_hi - s_lo).max(1.0);

    // --- the entrance plaza at the near end, the final court at the far end, the hub between
    let e = pts
        .iter()
        .filter(|(_, r)| *r >= sz.plaza + 8.0)
        .min_by(|a, b| {
            (s_of(a.0) - 0.35 * a.1.min(sz.plaza * 2.5))
                .total_cmp(&(s_of(b.0) - 0.35 * b.1.min(sz.plaza * 2.5)))
        })
        .map(|x| x.0)
        .ok_or("the valley floor is too narrow for an entrance plaza")?;
    let s_e = s_of(e);
    let hub_at = match ask.order {
        Order::Hub => 0.44,
        Order::Linear => 0.6,
    };
    let s_mid = s_e + (s_hi - s_e) * hub_at;
    let h = pts
        .iter()
        .filter(|(_, r)| *r >= sz.hub + 12.0)
        .max_by(|a, b| {
            let sa = a.1.min(sz.hub + 90.0 * k) - 1.6 * (s_of(a.0) - s_mid).abs();
            let sb = b.1.min(sz.hub + 90.0 * k) - 1.6 * (s_of(b.0) - s_mid).abs();
            sa.total_cmp(&sb)
        })
        .map(|x| x.0)
        .ok_or("the valley floor has no room for a hub")?;
    let final_reach = (sz.final_hx * sz.final_hx + sz.final_hz * sz.final_hz).sqrt();
    // the final court: as far along as there is room for it, with a clear, straight way to it
    let mut cands: Vec<(Vec2, f32)> = pts
        .iter()
        .filter(|(p, r)| *r >= sz.final_hx.max(sz.final_hz) + 6.0 && s_of(*p) > s_of(h) + 60.0 * k)
        .copied()
        .collect();
    cands.sort_by(|a, b| s_of(b.0).total_cmp(&s_of(a.0)));
    let clear = |a: Vec2, b: Vec2| -> f32 {
        let n = (a.distance(b) / 6.0).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| floor.room(a.lerp(b, i as f32 / n as f32)))
            .fold(f32::MAX, f32::min)
    };
    let mut fin = None;
    let mut best = (f32::MIN, Vec2::ZERO);
    for (p, r) in cands.iter().take(1600).step_by(2) {
        let dir = (*p - h).normalize_or(axis);
        let gate = h + dir * (sz.hub + 4.0);
        let foot = *p - dir * (sz.final_hz + 12.0);
        let cl = clear(gate, foot);
        let score = s_of(*p) + 2.0 * cl.min(sz.axis_hw + 30.0) + 0.5 * r.min(final_reach + 20.0);
        if cl >= sz.axis_hw + 8.0 && *r >= final_reach * 0.8 && fin.is_none() {
            fin = Some(*p);
        }
        if score > best.0 {
            best = (score, *p);
        }
    }
    let f_pos = fin.unwrap_or(best.1);
    if f_pos == Vec2::ZERO && cands.is_empty() {
        return Err("the valley floor is too short for a final court beyond the hub".into());
    }
    let dir = (f_pos - h).normalize_or(axis);
    let level = floor.level;
    let ground = |p: Vec2| g.at(p);

    // --- the axis: the gate at the hub's far side, a round plaza half way, the final court's stairs
    let hub_top = ground(h).max(level) + 0.3;
    let fin_base = median_ground(g, f_pos, sz.final_hx.min(sz.final_hz));
    let fin_top = fin_base + CourtKind::ThroneTerrace.rise();
    let gate = h + dir * (sz.hub + 4.0);
    let stair_len = |rise: f32| rise * 2.6;
    let fin_head = f_pos - dir * sz.final_hz;
    let fin_foot = fin_head - dir * stair_len(fin_top - (fin_base + 1.2));
    let alen = gate.distance(fin_foot);
    let plaza_r = if alen > 90.0 * k {
        (16.0 * kk).min(alen * 0.18)
    } else {
        0.0
    };
    let plaza = gate.lerp(fin_foot, 0.5);
    let axis_plan = Axis {
        gate,
        end: fin_foot,
        dir,
        hw: sz.axis_hw,
        plaza,
        plaza_r,
        h0: hub_top,
        h1: fin_base + 1.2,
        gated: ask.order == Order::Hub,
    };
    let final_court = Court {
        name: String::new(),
        boss: String::new(),
        kind: CourtKind::ThroneTerrace,
        c: f_pos,
        f: -dir,
        hx: sz.final_hx,
        hz: sz.final_hz,
        top: fin_top,
        stair_head: fin_head,
        stair_foot: fin_foot,
        stair_w: sz.stair_w * 1.4,
        floor: ask.theme.paving(),
        level: ask.levels[1].max(ask.levels[0]),
        order: 0,
        island: None,
        top2: fin_top + 4.0,
        hx2: sz.final_hx * 0.62,
        hz2: sz.final_hz * 0.55,
        back2: sz.final_hz * 0.45 - 1.0 * kk,
        fx: 0.0,
        fz: 0.0,
        ftop: 0.0,
        inner_head: fin_head,
        inner_foot: fin_head,
    };

    // --- the entrance: facing the hub, its stairs down that way
    let ef = (h - e).normalize_or(axis);
    let plaza_top = median_ground(g, e, sz.plaza) + 1.2;
    let e_head = e + ef * sz.plaza;
    let e_foot = e_head + ef * stair_len(1.2).max(5.0);

    // --- the courts: spread over the whole floor, each where it stands clearest of everything
    // placed before it (the biggest first, so the stepped pyramid gets the grandest spot)
    let base_kinds = [
        CourtKind::CrossYard,
        CourtKind::PillaredHall,
        CourtKind::SteppedPyramid,
        CourtKind::RoundShrine,
        CourtKind::Octagon,
    ];
    let mut kinds: Vec<CourtKind> = base_kinds.to_vec();
    shuffle(&mut kinds, &mut rng);
    while kinds.len() < ask.courts {
        let extra = kinds[rng.below(kinds.len() as u32) as usize];
        kinds.push(extra);
    }
    let want_n = ask.courts.clamp(1, 6);
    // (the stepped pyramid is the compound's landmark: always one of them)
    if let Some(i) = kinds.iter().position(|k| *k == CourtKind::SteppedPyramid)
        && i >= want_n
    {
        let j = rng.below(want_n as u32) as usize;
        kinds.swap(i, j);
    }
    kinds.truncate(want_n);
    let shape = |kind: CourtKind| -> (f32, f32) {
        let r = sz.court;
        match kind {
            CourtKind::CrossYard => (r, r),
            CourtKind::PillaredHall => (r * 0.8, r * 1.1),
            CourtKind::SteppedPyramid => (r * 0.95, r * 1.25),
            CourtKind::RoundShrine | CourtKind::Octagon => (r, r),
            CourtKind::IslandTemple => (r * 0.8, r * 0.8),
            CourtKind::ThroneTerrace => (sz.final_hx, sz.final_hz),
        }
    };
    let reach_of = |kind: CourtKind| {
        let (hx, hz) = shape(kind);
        match kind {
            CourtKind::RoundShrine
            | CourtKind::Octagon
            | CourtKind::IslandTemple
            | CourtKind::CrossYard => hx,
            _ => (hx * hx + hz * hz).sqrt(),
        }
    };
    kinds.sort_by(|a, b| reach_of(*b).total_cmp(&reach_of(*a)));
    let entrance_seg = (e_foot, h);
    let feasible = |p: Vec2, reach: f32, placed: &[(Vec2, f32)]| -> bool {
        floor.room(p) >= reach + 9.0
            && axis_plan.dist(p) >= reach + sz.axis_hw + 16.0 * k
            && p.distance(f_pos) >= reach + final_reach + 20.0 * k
            && p.distance(e) >= reach + sz.plaza + 24.0 * k
            && p.distance(h) >= reach + sz.hub + 22.0 * k
            && geom::seg_dist2(p, entrance_seg.0, entrance_seg.1).sqrt() >= reach + 12.0 * k
            && placed
                .iter()
                .all(|(q, r)| q.distance(p) >= reach + r + 34.0 * k)
    };
    let clearance = |p: Vec2, reach: f32, placed: &[(Vec2, f32)]| -> f32 {
        let mut c = (p.distance(h) - sz.hub)
            .min(p.distance(e) - sz.plaza)
            .min(p.distance(f_pos) - final_reach)
            .min(axis_plan.dist(p) - sz.axis_hw);
        for (q, r) in placed {
            c = c.min(q.distance(p) - r);
        }
        c - reach
    };
    let walkable = |a: Vec2, b: Vec2| clear(a, b) >= 3.0;
    let cands: Vec<Vec2> = pts
        .iter()
        .step_by(2)
        .filter(|(_, r)| *r >= sz.court * 0.8 + 9.0)
        .map(|x| x.0)
        .collect();
    let mut placed: Vec<(Vec2, f32, CourtKind)> = vec![];
    for kind in kinds.iter().copied() {
        let reach = reach_of(kind);
        let taken: Vec<(Vec2, f32)> = placed.iter().map(|(p, r, _)| (*p, *r)).collect();
        let mut best: Option<(f32, Vec2)> = None;
        for p in &cands {
            if !feasible(*p, reach, &taken) || !walkable(h, *p) {
                continue;
            }
            // clear of everything, roomy, not too far out from the hub, a little of the seed's own
            let score = clearance(*p, reach, &taken) + 0.35 * floor.room(*p).min(reach + 40.0)
                - 0.2 * (p.distance(h) - 0.42 * len).max(0.0)
                + 6.0 * fbm(p.x / 90.0, p.y / 90.0, 2, 2.0, 0.5, ask.seed & 0xffff);
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, *p));
            }
        }
        if let Some((_, p)) = best {
            placed.push((p, reach, kind));
        }
    }
    let dropped = kinds.len() - placed.len();
    // the order they are met in: sweeping round the hub from the way in (the levels rise that
    // way); a linear compound's trail visits them in that order
    let from_e = (e - h).normalize_or(-dir);
    let sweep = |p: Vec2| {
        let d = p - h;
        (d.y.atan2(d.x) - from_e.y.atan2(from_e.x)).rem_euclid(TAU)
    };
    placed.sort_by(|a, b| sweep(a.0).total_cmp(&sweep(b.0)));
    // (the side of the hub away from its gate: where a linear trail crosses from one side of the
    // axis to the other)
    let hub_back = h - dir * sz.hub * 0.92;
    let crosses =
        |a: Vec2, b: Vec2| seg_seg_dist(a, b, axis_plan.gate, axis_plan.end) < sz.axis_hw + 12.0;
    let mut courts: Vec<Court> = vec![];
    let mut prev = e_foot;
    for (p, _, kind) in &placed {
        let toward = match ask.order {
            Order::Hub => h,
            Order::Linear => {
                if crosses(prev, *p) {
                    hub_back
                } else {
                    prev
                }
            }
        };
        let c = new_court(*kind, *p, toward, shape(*kind));
        prev = c.stair_foot;
        courts.push(c);
    }

    // --- the island temple: the court with the most open ground round it sits in a lake
    let mut lakes: Vec<LakePlan> = vec![];
    let corridor = |c: &Court| -> (Vec2, Vec2) {
        match ask.order {
            Order::Hub => (h, c.stair_foot),
            Order::Linear => (c.stair_foot, c.stair_foot),
        }
    };
    let free_round = |p: Vec2, skip: Option<usize>, courts: &[Court], lakes: &[LakePlan]| -> f32 {
        let mut f = floor.room(p) - 8.0;
        f = f.min(axis_plan.dist(p) - sz.axis_hw - 10.0);
        f = f.min(p.distance(h) - sz.hub - 12.0);
        f = f.min(p.distance(e) - sz.plaza - 14.0);
        f = f.min(p.distance(f_pos) - final_reach - 12.0);
        f = f.min(geom::seg_dist2(p, entrance_seg.0, entrance_seg.1).sqrt() - 10.0);
        for (j, c) in courts.iter().enumerate() {
            if Some(j) == skip {
                continue;
            }
            f = f.min(p.distance(c.c) - c.reach() - 12.0);
            let (a, b) = corridor(c);
            f = f.min(geom::seg_dist2(p, a, b).sqrt() - 9.0);
        }
        for l in lakes {
            f = f.min(p.distance(l.c) - l.r - 16.0);
        }
        f
    };
    let isle_r = shape(CourtKind::IslandTemple).0;
    let pick = (0..courts.len())
        .filter(|i| courts[*i].kind != CourtKind::SteppedPyramid && courts.len() >= 3)
        .map(|i| (i, free_round(courts[i].c, Some(i), &courts, &lakes)))
        .filter(|(_, f)| *f >= isle_r + 7.0 + 14.0 * kk)
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((i, free)) = pick {
        let c = courts[i].c;
        let toward = courts[i].stair_foot + courts[i].f * 40.0;
        let lake_r = (free - 2.0).min(isle_r + 7.0 + 34.0 * kk);
        let mut ct = new_court(
            CourtKind::IslandTemple,
            c,
            toward,
            shape(CourtKind::IslandTemple),
        );
        ct.island = Some(lakes.len());
        courts[i] = ct;
        lakes.push(LakePlan {
            name: String::new(),
            c,
            r: lake_r,
            outline: blob(c, lake_r, isle_r + 10.0, &mut rng, 0.12),
            island: Some(i),
        });
    }
    // --- a second lake in the low ground where there is the most room left
    {
        let mut best = (f32::MIN, Vec2::ZERO);
        for (p, _) in pts.iter().step_by(3) {
            let f = free_round(*p, None, &courts, &lakes);
            if f > best.0 {
                best = (f, *p);
            }
        }
        let r = (best.0 * 0.92).min(52.0 * k.max(0.8));
        if r >= 16.0 {
            let c = best.1;
            lakes.push(LakePlan {
                name: String::new(),
                c,
                r,
                outline: blob(c, r, r * 0.55, &mut rng, 0.22),
                island: None,
            });
        }
    }

    // --- names, levels, the order the courts are met in
    let kit = ask.theme.kit();
    let mut totems: Vec<&str> = kit.totems.to_vec();
    shuffle(&mut totems, &mut rng);
    let n = courts.len();
    // (already in the order they are met in)
    for (o, c) in courts.iter_mut().enumerate() {
        c.order = o + 1;
    }
    let [lo, hi] = [
        ask.levels[0].min(ask.levels[1]),
        ask.levels[0].max(ask.levels[1]),
    ];
    let step = (hi as f32 - lo as f32) / (n as f32 + 1.0);
    let floors = ask.theme.court_floors();
    for (i, c) in courts.iter_mut().enumerate() {
        let totem = totems[i % totems.len()];
        let pattern = match c.kind {
            CourtKind::SteppedPyramid => kit.court_names[3],
            CourtKind::RoundShrine | CourtKind::IslandTemple => kit.court_names[4],
            CourtKind::PillaredHall => kit.court_names[2],
            CourtKind::CrossYard => kit.court_names[0],
            CourtKind::Octagon => kit.court_names[5],
            CourtKind::ThroneTerrace => kit.court_names[1],
        };
        c.name = fill(pattern, totem);
        let title = kit.boss_titles[rng.below(kit.boss_titles.len() as u32) as usize];
        c.boss = format!("{} {}", boss_name(&mut rng), fill(title, totem));
        c.level = (lo as f32 + step * (i as f32 + 1.0)).round() as u8;
        c.floor = floors[i % floors.len()];
        let base = median_ground(g, c.c, c.hx.min(c.hz));
        c.top = base + c.kind.rise();
        c.ftop = base + c.kind.rise() * FORE_RISE;
    }
    let mut final_court = final_court;
    final_court.name = kit.final_name.to_string();
    final_court.boss = format!("{} {}", boss_name(&mut rng), kit.final_title);
    final_court.order = n + 1;
    let lake_words = [
        "Mirror", "Serpent", "Jade", "Still", "Sunken", "Silver", "Black", "Moon",
    ];
    for l in lakes.iter_mut() {
        let w = lake_words[rng.below(lake_words.len() as u32) as usize];
        l.name = match l.island {
            Some(_) => format!("{w} Lagoon"),
            None => format!("{w} Pool"),
        };
    }

    // --- the trails
    let mut obstacles: Vec<Ob> = vec![];
    for c in &courts {
        obstacles.push(Ob::Court(Box::new(c.clone()), 7.0));
    }
    obstacles.push(Ob::Court(Box::new(final_court.clone()), 6.0));
    obstacles.push(Ob::Disc(e, sz.plaza + 5.0));
    obstacles.push(Ob::Seg(axis_plan.gate, axis_plan.end, sz.axis_hw + 7.0));
    if plaza_r > 0.0 {
        obstacles.push(Ob::Disc(plaza, plaza_r + 6.0));
    }
    for (i, l) in lakes.iter().enumerate() {
        obstacles.push(Ob::Lake(i, l.outline.clone(), 6.0));
    }
    let mut trails: Vec<Trail> = vec![];
    let hub_edge = |towards: Vec2| h + (towards - h).normalize_or(dir) * sz.hub * 0.92;
    trails.push(Trail {
        name: "The Way In".into(),
        kind: TrailKind::Main,
        pts: wind(e_foot, hub_edge(e_foot), &obstacles, &floor, &mut rng, 0.6),
        width: 5.0 * kk,
        to: None,
    });
    let lake_of = |c: &Court| c.island.map(|l| lakes[l].clone());
    let mut prev = e_foot;
    for (i, c) in courts.iter().enumerate() {
        let start = match ask.order {
            Order::Hub => hub_edge(c.stair_foot),
            Order::Linear => prev,
        };
        match lake_of(c) {
            Some(l) => {
                // the trail to the shore straight out from the island's stairs, then a stone
                // bridge across to them
                let mut r = c.hx;
                while r < l.r * 2.0 && geom::poly_sdf(&l.outline, c.c + c.f * r) > -5.0 {
                    r += 1.0;
                }
                let shore = c.c + c.f * r;
                let li = c.island.unwrap_or(usize::MAX);
                let obs: Vec<Ob> = obstacles
                    .iter()
                    .filter(|o| !matches!(o, Ob::Lake(i, _, _) if *i == li))
                    .cloned()
                    .collect();
                let path = route(
                    start,
                    shore,
                    crosses(start, shore).then_some(hub_back),
                    &obs,
                    &floor,
                    &mut rng,
                );
                trails.push(Trail {
                    name: format!("Trail to {}", c.name),
                    kind: TrailKind::Spoke,
                    pts: path,
                    width: 3.6 * kk,
                    to: Some(i),
                });
                trails.push(Trail {
                    name: format!("Bridge to {}", c.name),
                    kind: TrailKind::Bridge,
                    pts: vec![shore, c.stair_foot],
                    width: 4.5 * kk,
                    to: Some(i),
                });
            }
            None => {
                let end = c.stair_foot + c.f * 2.0;
                trails.push(Trail {
                    name: format!("Trail to {}", c.name),
                    kind: TrailKind::Spoke,
                    pts: route(
                        start,
                        end,
                        crosses(start, end).then_some(hub_back),
                        &obstacles,
                        &floor,
                        &mut rng,
                    ),
                    width: 3.6 * kk,
                    to: Some(i),
                });
            }
        }
        prev = c.stair_foot;
    }
    if ask.order == Order::Linear
        && let Some(last) = courts.last()
    {
        let end = hub_edge(last.stair_foot);
        trails.push(Trail {
            name: "Trail to the hub".into(),
            kind: TrailKind::Spoke,
            pts: route(
                last.stair_foot,
                end,
                crosses(last.stair_foot, end).then_some(hub_back),
                &obstacles,
                &floor,
                &mut rng,
            ),
            width: 3.6 * kk,
            to: None,
        });
    }
    trails.push(Trail {
        name: "The Processional Way".into(),
        kind: TrailKind::Axis,
        pts: vec![gate - dir * 6.0, fin_foot + dir * 3.0],
        width: sz.axis_hw * 2.0,
        to: None,
    });

    // --- camps along the trails (the trash between the courts)
    let mut camps: Vec<Camp> = vec![];
    for (ti, t) in trails.iter().enumerate() {
        if !matches!(t.kind, TrailKind::Spoke | TrailKind::Main) || t.len() < 50.0 {
            continue;
        }
        let at = t.len() * rng.range(0.42, 0.6);
        let (p, d) = geom::polyline_at(&t.pts, at);
        let sd = side(d) * if rng.f32() < 0.5 { 1.0 } else { -1.0 };
        let r = 9.0 * kk;
        let mut c = p + sd * (r + 4.0);
        if !floor.on(c) || floor.room(c) < r || hits(&obstacles, c, r) {
            c = p - sd * (r + 4.0);
        }
        if floor.room(c) < r || hits(&obstacles, c, r) {
            continue;
        }
        let lv = match t.to {
            Some(i) => courts[i].level.saturating_sub(1).max(lo),
            None => lo,
        };
        let name = match (t.to, t.kind) {
            (Some(i), _) => format!("Camp before {}", courts[i].name),
            (None, TrailKind::Main) => "Camp on the Way In".into(),
            _ => "Hub Guard Camp".into(),
        };
        camps.push(Camp {
            name,
            c,
            r,
            f: (p - c).normalize_or(Vec2::X),
            level: [lv, lv.saturating_add(1)],
            trail: ti,
        });
    }
    // guards on the round plaza
    if plaza_r > 0.0 {
        let lv = hi.saturating_sub(1).max(lo);
        for sgn in [-1.0f32, 1.0] {
            let c = plaza + side(dir) * sgn * (plaza_r + 10.0 * kk);
            if floor.on(c) {
                camps.push(Camp {
                    name: "Processional Guard".into(),
                    c,
                    r: 9.0 * kk,
                    f: (plaza - c).normalize_or(Vec2::X),
                    level: [lv, lv],
                    trail: trails.len() - 1,
                });
            }
        }
    }

    // --- ruins and tree clumps between everything
    let trail_near = |p: Vec2, m: f32| {
        trails
            .iter()
            .any(|t| geom::polyline_dist(&t.pts, p).0 < t.width * 0.5 + m)
    };
    let mut ruins: Vec<Ruin> = vec![];
    let total: u32 = kit.ruins.iter().map(|(_, w)| *w).sum();
    let want = (floor.area / 6500.0).clamp(12.0, 90.0) as usize;
    let mut tries = 0;
    let mut taken: Vec<Vec2> = vec![];
    while ruins.len() < want && tries < want * 40 {
        tries += 1;
        let (p, room) = pts[rng.below(pts.len() as u32) as usize];
        if room < 6.0
            || hits(&obstacles, p, 6.0)
            || trail_near(p, 4.0)
            || p.distance(h) < sz.hub + 2.0
            || taken.iter().any(|q| q.distance(p) < 26.0 * k)
        {
            continue;
        }
        // a piece, a broken run or a cluster
        let mut pick = rng.below(total.max(1));
        let mut a = kit.ruins[0].0;
        for (x, w) in kit.ruins {
            if pick < *w {
                a = *x;
                break;
            }
            pick -= w;
        }
        let yaw = rng.range(0.0, 360.0);
        taken.push(p);
        if a.0 == "stone_wall" {
            // a broken run of wall: a few sections, gaps between
            let d = Vec2::from_angle(yaw.to_radians());
            let m = 2 + rng.below(4) as usize;
            for s in 0..m {
                if rng.f32() < 0.25 {
                    continue;
                }
                let q = p + d * (s as f32 * 3.0 * a.1);
                ruins.push(Ruin::new(
                    a.0,
                    a.1 * rng.range(0.9, 1.1),
                    q,
                    yaw_along(d) + rng.range(-6.0, 6.0),
                ));
            }
        } else if a.0 == "stone_pillar" {
            let m = 1 + rng.below(4) as usize;
            for s in 0..m {
                let q = p + Vec2::from_angle(yaw.to_radians() + s as f32 * 1.3) * (s as f32 * 3.5);
                let r = Ruin::new(a.0, a.1 * rng.range(0.8, 1.15), q, rng.range(0.0, 360.0));
                ruins.push(if rng.f32() < 0.45 {
                    r.broken(rng.range(0.25, 0.6))
                } else {
                    r
                });
            }
        } else {
            ruins.push(Ruin::new(a.0, a.1 * rng.range(0.9, 1.1), p, yaw));
        }
    }
    let camp_near = |p: Vec2, m: f32| camps.iter().any(|c| c.c.distance(p) < c.r + m);
    // (the additions roll on a stream of their own: what came before stays as it was per seed)
    let mut rng2 = Rng::new(ask.seed as u64 ^ 0x7a11_5e7f_0d0d);
    let rubble: &'static str = kit
        .ruins
        .iter()
        .map(|(a, _)| a.0)
        .find(|a| a.contains("rubble"))
        .unwrap_or("stonefist_rubble_pile");
    // --- ruins along the trails: broken walls, pillar stumps, statues, rubble, pillar gateways
    for t in &trails {
        if !matches!(t.kind, TrailKind::Main | TrailKind::Spoke) {
            continue;
        }
        let len = t.len();
        let mut at = rng2.range(12.0, 30.0);
        while at < len - 12.0 {
            let (p, d) = geom::polyline_at(&t.pts, at);
            let sd = side(d) * if rng2.f32() < 0.5 { 1.0 } else { -1.0 };
            let q = p + sd * (t.width * 0.5 + rng2.range(3.0, 8.0));
            let ok = rng2.f32() < 0.75
                && floor.room(q) > 4.0
                && !hits(&obstacles, q, 3.0)
                && !camp_near(q, 3.0)
                && q.distance(h) > sz.hub + 4.0
                && !trail_near(q, 1.5);
            if ok {
                match rng2.below(5) {
                    0 => {
                        let m = 2 + rng2.below(3) as usize;
                        for j in 0..m {
                            if rng2.f32() < 0.25 {
                                continue;
                            }
                            let r = Ruin::new(
                                "stone_wall",
                                1.3 * rng2.range(0.9, 1.1),
                                q + d * (j as f32 * 3.1),
                                yaw_along(d) + rng2.range(-5.0, 5.0),
                            );
                            ruins.push(if rng2.f32() < 0.4 {
                                r.broken(rng2.range(0.4, 0.8))
                            } else {
                                r
                            });
                        }
                    }
                    1 => {
                        for j in 0..2 + rng2.below(2) as usize {
                            let r = Ruin::new(
                                kit.pillar.0,
                                kit.pillar.1 * rng2.range(0.9, 1.1),
                                q + d * (j as f32 * 3.4) + sd * rng2.range(-1.0, 1.0),
                                rng2.range(0.0, 360.0),
                            );
                            ruins.push(r.broken(rng2.range(0.22, 0.6)));
                        }
                    }
                    2 => ruins.push(Ruin::new(
                        kit.statue.0,
                        kit.statue.1 * rng2.range(0.9, 1.2),
                        q,
                        yaw_facing(-sd),
                    )),
                    3 => ruins.push(Ruin::new(
                        rubble,
                        1.3 * rng2.range(0.8, 1.3),
                        q,
                        rng2.range(0.0, 360.0),
                    )),
                    _ => {
                        // a pillar gateway over the trail
                        for sg in [-1.0f32, 1.0] {
                            ruins.push(Ruin::new(
                                kit.pillar.0,
                                kit.pillar.1 * 1.1,
                                p + side(d) * sg * (t.width * 0.5 + 1.6),
                                yaw_facing(d),
                            ));
                        }
                    }
                }
            }
            at += rng2.range(22.0, 38.0);
        }
    }
    let mut clumps: Vec<(Vec2, f32)> = vec![];
    let want = (floor.area / 11000.0).clamp(8.0, 60.0) as usize;
    let mut tries = 0;
    while clumps.len() < want && tries < want * 40 {
        tries += 1;
        let (p, room) = pts[rng.below(pts.len() as u32) as usize];
        let r = rng.range(10.0, 22.0) * kk;
        if room < r * 0.5
            || hits(&obstacles, p, r * 0.6 + 4.0)
            || trail_near(p, r * 0.5 + 3.0)
            || p.distance(h) < sz.hub + r
            || clumps.iter().any(|(q, rq)| q.distance(p) < (r + rq) * 1.1)
        {
            continue;
        }
        clumps.push((p, r));
    }
    // a few big trees round the hub (it reads as a jungle clearing)
    for i in 0..5 {
        let a = rng.f32() * TAU + i as f32 * TAU / 5.0;
        let p = h + Vec2::from_angle(a) * (sz.hub + 14.0 * kk);
        let r = 9.0 * kk;
        if floor.room(p) > r && !hits(&obstacles, p, r) && !trail_near(p, r * 0.5 + 2.0) {
            clumps.push((p, r));
        }
    }

    // the perimeter wall along the open edge: the floor's outline behind the entrance
    let back = -ef;
    let wall = open_edge_wall(&floor, e, back, (len * 0.42).max(sz.plaza * 6.0));
    Ok(Plan {
        theme: ask.theme,
        order: ask.order,
        seed: ask.seed,
        floor: level,
        scale: k,
        grand: kk.max(1.0),
        entrance: Entrance {
            c: e,
            r: sz.plaza,
            f: ef,
            top: plaza_top,
            stair_foot: e_foot,
            start: e - ef * (sz.plaza - 7.0),
            wall,
        },
        hub: Hub {
            name: kit.hub_name.to_string(),
            c: h,
            r: sz.hub,
            top: hub_top,
        },
        axis: axis_plan,
        courts,
        final_court,
        lakes,
        trails,
        camps,
        ruins,
        clumps,
        district: vec![],
        room: floor.coarse(4),
        dropped,
        open: back,
    })
}

/// How high a forecourt stands, as a share of its court's rise.
pub const FORE_RISE: f32 = 0.45;

fn new_court(kind: CourtKind, c: Vec2, toward: Vec2, size: (f32, f32)) -> Court {
    new_court_with(kind, c, toward, size, false)
}

/// A court, with a forecourt in front of it when `fore` (all but an island's).
pub fn new_court_with(
    kind: CourtKind,
    c: Vec2,
    toward: Vec2,
    (hx, hz): (f32, f32),
    fore: bool,
) -> Court {
    let f = (toward - c).normalize_or(Vec2::X);
    let edge = match kind {
        CourtKind::RoundShrine | CourtKind::IslandTemple | CourtKind::Octagon => hx,
        _ => hz,
    };
    let rise = kind.rise();
    // (a forecourt in front of every court but the island's: as wide as the court, a third as deep)
    let (fx, fz) = if kind == CourtKind::IslandTemple || !fore {
        (0.0, 0.0)
    } else {
        (hx * 0.92, hx.max(hz) * 0.3)
    };
    let (inner_head, inner_foot, head, foot) = if fz > 0.0 {
        let ih = c + f * edge;
        let ifo = ih + f * (rise * (1.0 - FORE_RISE) * 2.4).max(4.0);
        let h = c + f * (edge + 2.0 * fz);
        (ih, ifo, h, h + f * (rise * FORE_RISE * 2.4).max(4.0))
    } else {
        let h = c + f * edge;
        (h, h, h, h + f * (rise * 2.4).max(5.0))
    };
    Court {
        name: String::new(),
        boss: String::new(),
        kind,
        c,
        f,
        hx,
        hz,
        top: 0.0,
        stair_head: head,
        stair_foot: foot,
        stair_w: if kind == CourtKind::IslandTemple {
            6.5
        } else {
            8.0
        },
        floor: Layer::Cobblestone,
        level: 1,
        order: 0,
        island: None,
        top2: 0.0,
        hx2: 0.0,
        hz2: 0.0,
        back2: 0.0,
        fx,
        fz,
        ftop: 0.0,
        inner_head,
        inner_foot,
    }
}

/// The median ground height in a disc (a platform's base).
pub(crate) fn median_ground(g: &Grid, c: Vec2, r: f32) -> f32 {
    let mut v = vec![];
    for i in 0..7 {
        for j in 0..7 {
            let q = c + Vec2::new(i as f32 / 3.0 - 1.0, j as f32 / 3.0 - 1.0) * r;
            if q.distance(c) <= r * 1.01 {
                v.push(g.at(q));
            }
        }
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied().unwrap_or_else(|| g.at(c))
}

/// The wall along the open edge: the part of the floor's outline behind the entrance plaza, out
/// to where the valley's sides close in (about ±80° round it).
fn open_edge_wall(floor: &Floor, e: Vec2, back: Vec2, reach: f32) -> Vec<Vec2> {
    let loops = floor.outline(3.0);
    let Some(lp) = loops.iter().max_by(|a, b| {
        geom::signed_area(a)
            .abs()
            .total_cmp(&geom::signed_area(b).abs())
    }) else {
        return vec![];
    };
    let n = lp.len();
    if n < 8 {
        return vec![];
    }
    let inside = |p: Vec2| {
        let d = p - e;
        d.length() < reach && d.normalize_or(back).dot(back) > (88f32).to_radians().cos()
    };
    // the longest run of the loop in the sector
    let mut best: Vec<Vec2> = vec![];
    let start = (0..n).find(|i| !inside(lp[*i])).unwrap_or(0);
    let mut run: Vec<Vec2> = vec![];
    for s in 0..=n {
        let p = lp[(start + s) % n];
        if inside(p) {
            run.push(p);
        } else {
            if run.len() > best.len() {
                best = std::mem::take(&mut run);
            }
            run.clear();
        }
    }
    if run.len() > best.len() {
        best = run;
    }
    geom::simplify_open(&best, 1.5)
}

/// A rounded blob of radius `r` round `c` (never in nearer than `min_r`), `wobble` uneven.
pub(crate) fn blob(c: Vec2, r: f32, min_r: f32, rng: &mut Rng, wobble: f32) -> Vec<Vec2> {
    let seed = rng.next_u32() % 10_000;
    let stretch = 1.0 + rng.range(0.0, wobble * 1.5);
    let turn = rng.f32() * PI;
    (0..48)
        .map(|i| {
            let a = i as f32 / 48.0 * TAU;
            let w = 1.0 + wobble * fbm(a.cos() * 1.3 + 3.0, a.sin() * 1.3, 3, 2.0, 0.5, seed);
            let q = Vec2::from_angle(a) * r * w;
            let q = geom::rot(Vec2::new(q.x * stretch, q.y / stretch), turn);
            let l = q.length().max(min_r);
            c + q.normalize_or(Vec2::X) * l
        })
        .collect()
}

/// The shortest distance between two segments.
fn seg_seg_dist(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> f32 {
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p - o).perp_dot(q - o);
    let (d1, d2, d3, d4) = (
        cross(c, d, a),
        cross(c, d, b),
        cross(a, b, c),
        cross(a, b, d),
    );
    if (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0) {
        return 0.0;
    }
    geom::seg_dist2(a, c, d)
        .min(geom::seg_dist2(b, c, d))
        .min(geom::seg_dist2(c, a, b))
        .min(geom::seg_dist2(d, a, b))
        .sqrt()
}

pub(crate) fn shuffle<T>(v: &mut [T], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        let j = rng.below(i as u32 + 1) as usize;
        v.swap(i, j);
    }
}

/// The turn (objects_place `yaw_deg`) that faces a model's front (+Z) along `f`.
pub fn yaw_facing(f: Vec2) -> f32 {
    (-f.x).atan2(f.y).to_degrees()
}

/// The turn that lays a model's length (+X) along `d`.
pub fn yaw_along(d: Vec2) -> f32 {
    d.y.atan2(d.x).to_degrees()
}

/// A compass bearing (degrees, clockwise from north = −z) of a direction.
pub fn compass_of(d: Vec2) -> f32 {
    d.x.atan2(-d.y).to_degrees().rem_euclid(360.0)
}

// ------------------------------------------------------------------ winding trails

/// Something a trail winds round: a disc, a band along a segment, a polygon with a margin.
#[derive(Clone, Debug)]
pub enum Ob {
    Disc(Vec2, f32),
    Seg(Vec2, Vec2, f32),
    /// a lake (its index) and a margin
    Lake(usize, Vec<Vec2>, f32),
    /// a court's own outline and a margin
    Court(Box<Court>, f32),
}

impl Ob {
    /// > 0 inside (m).
    fn sdf(&self, p: Vec2) -> f32 {
        match self {
            Ob::Disc(c, r) => r - p.distance(*c),
            Ob::Seg(a, b, w) => w - geom::seg_dist2(p, *a, *b).sqrt(),
            Ob::Lake(_, pts, m) => geom::poly_sdf(pts, p) + m,
            Ob::Court(c, m) => c.sdf(p).max(c.sdf2(p)) + m,
        }
    }
}

pub(crate) fn hits(obs: &[Ob], p: Vec2, r: f32) -> bool {
    obs.iter().any(|o| o.sdf(p) > -r)
}

/// A trail from `a` to `b` (by way of `via`), winding.
fn route(
    a: Vec2,
    b: Vec2,
    via: Option<Vec2>,
    obs: &[Ob],
    floor: &Floor,
    rng: &mut Rng,
) -> Vec<Vec2> {
    match via {
        Some(w) => {
            let mut p = wind(a, w, obs, floor, rng, 0.8);
            p.pop();
            p.extend(wind(w, b, obs, floor, rng, 0.8));
            p
        }
        None => wind(a, b, obs, floor, rng, 1.0),
    }
}

/// A trail from `a` to `b` that winds (a lazy S, its own each time) and bends round what is in
/// its way, kept on the floor. `bend` scales how much it winds.
pub(crate) fn wind(
    a: Vec2,
    b: Vec2,
    obs: &[Ob],
    floor: &Floor,
    rng: &mut Rng,
    bend: f32,
) -> Vec<Vec2> {
    let len = a.distance(b);
    if len < 4.0 {
        return vec![a, b];
    }
    let d = (b - a) / len;
    let s = side(d);
    let amp = (len * 0.13).clamp(3.0, 34.0) * bend * if rng.f32() < 0.5 { 1.0 } else { -1.0 };
    let (ph, f2) = (rng.f32() * TAU, rng.range(1.4, 2.6));
    let n = ((len / 5.0).ceil() as usize).max(3);
    let mut pts: Vec<Vec2> = (0..=n)
        .map(|i| {
            let u = i as f32 / n as f32;
            let env = (PI * u).sin();
            a + d * len * u + s * (amp * env + amp * 0.45 * (TAU * f2 * u + ph).sin() * env)
        })
        .collect();
    // (the obstacles the ends start in, they may leave)
    let skip: Vec<bool> = obs
        .iter()
        .map(|o| o.sdf(a) > -2.0 || o.sdf(b) > -2.0)
        .collect();
    for _ in 0..24 {
        for i in 1..n {
            let mut p = pts[i];
            for (o, sk) in obs.iter().zip(&skip) {
                if *sk {
                    continue;
                }
                let v = o.sdf(p);
                if v > -1.0 {
                    let e = 0.75;
                    let g = Vec2::new(
                        o.sdf(p + Vec2::X * e) - o.sdf(p - Vec2::X * e),
                        o.sdf(p + Vec2::Y * e) - o.sdf(p - Vec2::Y * e),
                    );
                    p -= g.normalize_or(s) * (v + 1.5).min(6.0);
                }
            }
            let room = floor.room(p);
            if room < 5.0 {
                let e = 2.0;
                let g = Vec2::new(
                    floor.room(p + Vec2::X * e) - floor.room(p - Vec2::X * e),
                    floor.room(p + Vec2::Y * e) - floor.room(p - Vec2::Y * e),
                );
                p += g.normalize_or(Vec2::ZERO) * (5.0 - room).min(4.0);
            }
            pts[i] = p;
        }
        let prev = pts.clone();
        for i in 1..n {
            pts[i] = prev[i - 1] * 0.25 + prev[i] * 0.5 + prev[i + 1] * 0.25;
        }
    }
    pts
}

// ------------------------------------------------------------------ what it says it made

impl Plan {
    /// Every court, the final one last.
    pub fn all_courts(&self) -> impl Iterator<Item = &Court> {
        self.courts.iter().chain(std::iter::once(&self.final_court))
    }
    /// The summary MCP returns.
    pub fn summary(&self) -> Value {
        let court = |c: &Court| {
            json!({
                "name": c.name, "kind": c.kind, "shape": c.kind.name(), "boss": c.boss, "level": c.level, "order": c.order,
                "position": [r1(c.c.x), r1(c.c.y)], "top_m": r1(c.top), "stairs_face_deg": r1(compass_of(c.f)),
                "stair_foot": [r1(c.stair_foot.x), r1(c.stair_foot.y)], "island_in": c.island.map(|l| self.lakes[l].name.clone()),
            })
        };
        json!({
            "theme": self.theme.id(),
            "order": self.order.id(),
            "seed": self.seed,
            "floor_m": r1(self.floor),
            "entrance": { "position": [r1(self.entrance.c.x), r1(self.entrance.c.y)], "faces_deg": r1(compass_of(self.entrance.f)), "players_arrive": [r1(self.entrance.start.x), r1(self.entrance.start.y)], "wall_m": r1(geom::polyline_len(&self.entrance.wall)) },
            "hub": { "name": self.hub.name, "position": [r1(self.hub.c.x), r1(self.hub.c.y)], "radius_m": r1(self.hub.r) },
            "axis": { "gate": [r1(self.axis.gate.x), r1(self.axis.gate.y)], "round_plaza": (self.axis.plaza_r > 0.0).then(|| [r1(self.axis.plaza.x), r1(self.axis.plaza.y)]), "end": [r1(self.axis.end.x), r1(self.axis.end.y)], "length_m": r1(self.axis.len()), "gated": self.axis.gated },
            "courts": self.courts.iter().map(court).collect::<Vec<_>>(),
            "final": court(&self.final_court),
            "lakes": self.lakes.iter().map(|l| json!({ "name": l.name, "position": [r1(l.c.x), r1(l.c.y)], "radius_m": r1(l.r), "island_court": l.island.map(|i| self.courts[i].name.clone()) })).collect::<Vec<_>>(),
            "trails": self.trails.iter().map(|t| json!({ "name": t.name, "kind": t.kind, "length_m": r1(t.len()), "width_m": r1(t.width), "from": [r1(t.pts[0].x), r1(t.pts[0].y)], "to": [r1(t.pts[t.pts.len() - 1].x), r1(t.pts[t.pts.len() - 1].y)], "points": geom::simplify_open(&t.pts, 1.5).iter().map(|p| [r1(p.x), r1(p.y)]).collect::<Vec<_>>() })).collect::<Vec<_>>(),
            "camps": self.camps.iter().map(|c| json!({ "name": c.name, "position": [r1(c.c.x), r1(c.c.y)], "level": c.level })).collect::<Vec<_>>(),
            "ruins": self.ruins.len(),
            "tree_clumps": self.clumps.len(),
            "courts_dropped": self.dropped,
        })
    }
}
