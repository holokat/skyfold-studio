//! Set pieces: a whole graveyard, boneyard, village square or market filling an area drawn on
//! the ground (Objects tab › Set pieces: drag a loop, like drawing a lake), or the MCP tool
//! `objects_place_set_piece` (a `polygon`).
//!
//! - `layout_area` (pure, deterministic per seed) arranges hand-painted style pack models
//!   (`hp:`) inside the outline, in the set's own frame (`area_frame`: its long side along x,
//!   its front toward the camera), sized by the area: a graveyard is an iron fence along the
//!   outline with the arched gate on the front side, a crypt (or statue) at the far end, rows of
//!   mixed headstones and markers, dead trees in the corners, an open grave with its coffin,
//!   candles and withered flowers; a boneyard is the giant skeleton (skull, walk-under ribcage,
//!   trailing vertebrae, claws), scaled to the room it has and half sunk among bone piles; a
//!   square is a well with lamps along its edge, benches, a notice board and dressing; a market
//!   is rows of stalls along aisles. Bigger areas get more of everything.
//! - `stamp_area` puts it on the ground as ordinary placed objects (move, turn and resize each
//!   one afterwards like any other): fence sections tilt to follow the ground, buildings level
//!   the ground under them (`placement::fit`), the ground inside is painted (dark dirt and dead
//!   grass, dry dirt, cobbles) and procedural nature is cleared off it. One undo step.
//!
//! Shuffle re-rolls the last set in the same outline with the next seed (one undo step too).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::editor::{ObjectTool, Tab};
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::noise::{Rng, fbm, smoothstep};
use crate::objects::Placed;
use crate::terrain::Terrain;
use crate::terrain::layers::{LAYER_COUNT, Layer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetPieceKind {
    Graveyard,
    Boneyard,
    VillageSquare,
    Market,
}

impl SetPieceKind {
    pub const ALL: [SetPieceKind; 4] = [
        SetPieceKind::Graveyard,
        SetPieceKind::Boneyard,
        SetPieceKind::VillageSquare,
        SetPieceKind::Market,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SetPieceKind::Graveyard => "Graveyard",
            SetPieceKind::Boneyard => "Boneyard",
            SetPieceKind::VillageSquare => "Village square",
            SetPieceKind::Market => "Market",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SetPieceKind::Graveyard => "graveyard",
            SetPieceKind::Boneyard => "boneyard",
            SetPieceKind::VillageSquare => "village_square",
            SetPieceKind::Market => "market",
        }
    }

    /// The model whose thumbnail stands for it.
    pub fn icon_asset(self) -> &'static str {
        match self {
            SetPieceKind::Graveyard => "hp:crypt",
            SetPieceKind::Boneyard => "hp:giant_skull",
            SetPieceKind::VillageSquare => "hp:well",
            SetPieceKind::Market => "hp:market_stall",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SetPieceKind::Graveyard => {
                "An iron-fenced graveyard: rows of headstones and markers, the arched gate toward you, a crypt at the back, dead trees, an open grave, candles. The ground turns to dark dirt and dead grass."
            }
            SetPieceKind::Boneyard => {
                "A giant's bones half sunk in dry dirt: the huge skull, a ribcage you can walk under, vertebrae trailing away, claws, and bone piles and skulls all around."
            }
            SetPieceKind::VillageSquare => {
                "A cobbled square round a well: lamp posts, benches, a notice board and signpost, barrels, crates and flowers."
            }
            SetPieceKind::Market => {
                "Rows of striped market stalls along a cobbled aisle, with crates of apples and cabbages, sacks, barrels and lamp posts."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl SetSize {
    pub const ALL: [SetSize; 3] = [SetSize::Small, SetSize::Medium, SetSize::Large];

    pub fn id(self) -> &'static str {
        match self {
            SetSize::Small => "small",
            SetSize::Medium => "medium",
            SetSize::Large => "large",
        }
    }

    fn pick<T: Copy>(self, s: T, m: T, l: T) -> T {
        match self {
            SetSize::Small => s,
            SetSize::Medium => m,
            SetSize::Large => l,
        }
    }

    /// The outline a set of this size gets round a point (MCP `position` + `size`), in the
    /// set's frame: a rectangle (wider than deep) or a circle.
    pub fn outline(self, kind: SetPieceKind) -> Vec<Vec2> {
        let rect = |hw: f32, hd: f32| {
            vec![
                Vec2::new(-hw, -hd),
                Vec2::new(hw, -hd),
                Vec2::new(hw, hd),
                Vec2::new(-hw, hd),
            ]
        };
        let disc = |r: f32| {
            (0..24)
                .map(|i| Vec2::from_angle(std::f32::consts::TAU * i as f32 / 24.0) * r)
                .collect()
        };
        match kind {
            SetPieceKind::Graveyard => {
                let (w, d) = self.pick((6.5, 5.5), (9.5, 7.5), (13.5, 10.5));
                rect(w, d)
            }
            SetPieceKind::Boneyard => disc(self.pick(8.5, 12.0, 16.0)),
            SetPieceKind::VillageSquare => disc(self.pick(7.0, 10.0, 13.0)),
            SetPieceKind::Market => {
                let (w, d) = self.pick((6.5, 6.0), (9.0, 6.0), (12.5, 6.0));
                rect(w, d)
            }
        }
    }
}

/// One piece of a layout, in the set's own frame (metres, x right, +z toward the viewer: the
/// front). Models face +Z as modelled.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub asset: String,
    pub at: Vec2,
    /// turn about the vertical (radians, as `Quat::from_rotation_y`)
    pub yaw: f32,
    /// lean about its own x and z axes (radians)
    pub tilt: Vec2,
    /// size along its own axes (1: as modelled)
    pub scale: Vec3,
    /// how far it sinks into the ground (m): half-buried bones
    pub sink: f32,
    /// a fence run this long along its own x: it tilts to follow the ground between its ends
    pub span: f32,
}

impl Piece {
    fn new(asset: &str, at: Vec2, yaw: f32) -> Piece {
        Piece {
            asset: format!("hp:{asset}"),
            at,
            yaw,
            tilt: Vec2::ZERO,
            scale: Vec3::ONE,
            sink: 0.0,
            span: 0.0,
        }
    }
    fn tilted(mut self, t: Vec2) -> Piece {
        self.tilt = t;
        self
    }
    fn sunk(mut self, s: f32) -> Piece {
        self.sink = s;
        self
    }
    fn scaled(mut self, s: Vec3) -> Piece {
        self.scale = s;
        self
    }
}

/// An area in the set's frame: an outline grown by a margin (m).
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub outline: std::sync::Arc<[Vec2]>,
    pub grow: f32,
}

impl Area {
    fn new(outline: std::sync::Arc<[Vec2]>, grow: f32) -> Area {
        Area { outline, grow }
    }
    /// Signed distance from the area's edge (negative inside).
    fn sdf(&self, p: Vec2) -> f32 {
        poly_sdf(&self.outline, p) - self.grow
    }
    fn radius(&self) -> f32 {
        self.outline.iter().map(|p| p.length()).fold(0.0, f32::max) + self.grow
    }
}

/// Signed distance to a polygon's outline (negative inside).
pub fn poly_sdf(poly: &[Vec2], p: Vec2) -> f32 {
    let n = poly.len();
    if n < 3 {
        return f32::MAX;
    }
    let mut d = f32::MAX;
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        d = d.min(p.distance(a + ab * t));
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    if inside { -d } else { d }
}

fn poly_area(poly: &[Vec2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| poly[i].perp_dot(poly[(i + 1) % n]))
        .sum::<f32>()
        .abs()
        * 0.5
}

/// Douglas–Peucker on a closed outline (corners within `tol` m of a straight run go).
fn simplify(poly: &[Vec2], tol: f32) -> Vec<Vec2> {
    fn dp(pts: &[Vec2], tol: f32, out: &mut Vec<Vec2>) {
        let (a, b) = (pts[0], pts[pts.len() - 1]);
        let ab = b - a;
        let (mut k, mut far) = (0, 0.0);
        for (i, p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
            let t = ((*p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            let d = p.distance(a + ab * t);
            if d > far {
                far = d;
                k = i;
            }
        }
        if far > tol && k > 0 {
            dp(&pts[..=k], tol, out);
            dp(&pts[k..], tol, out);
        } else {
            out.push(a);
        }
    }
    if poly.len() < 4 {
        return poly.to_vec();
    }
    // split at the point farthest from the first so the closed loop simplifies evenly
    let far = (1..poly.len())
        .max_by(|a, b| {
            poly[*a]
                .distance(poly[0])
                .total_cmp(&poly[*b].distance(poly[0]))
        })
        .unwrap_or(1);
    let mut out = vec![];
    dp(&poly[..=far], tol, &mut out);
    let mut back: Vec<Vec2> = poly[far..].to_vec();
    back.push(poly[0]);
    dp(&back, tol, &mut out);
    out
}

/// Ground paint: a layer over an area (soft, ragged edge), with patches of a second layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    pub center: Vec2,
    pub area: Area,
    pub layer: Layer,
    pub patches: Option<Layer>,
    pub strength: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub pieces: Vec<Piece>,
    pub paint: Vec<Paint>,
    /// where procedural nature is cleared
    pub clear: Area,
}

fn rot2(v: Vec2, a: f32) -> Vec2 {
    // as Quat::from_rotation_y(a) turns (x, z)
    let (s, c) = a.sin_cos();
    Vec2::new(v.x * c + v.y * s, -v.x * s + v.y * c)
}

/// The yaw that turns a model's own +x along direction `d` (x, z).
fn yaw_along(d: Vec2) -> f32 {
    (-d.y).atan2(d.x)
}

/// Picks one of `items` by weight.
fn weighted<'a>(rng: &mut Rng, items: &[(&'a str, f32)]) -> &'a str {
    let total: f32 = items.iter().map(|i| i.1).sum();
    let mut x = rng.f32() * total;
    for (a, w) in items {
        if x < *w {
            return a;
        }
        x -= w;
    }
    items[items.len() - 1].0
}

/// Fence sections from a to b (a section's single post sits at its +x end: the run's posts
/// land on the next section's start and, at b, inside the corner post or pillar).
fn fence_run(out: &mut Vec<Piece>, a: Vec2, b: Vec2, model: &str, pitch: f32) {
    let len = a.distance(b);
    if len < 0.4 {
        return;
    }
    let n = (len / pitch).round().max(1.0);
    let step = len / n;
    let d = (b - a) / len;
    for i in 0..n as usize {
        let c = a + d * step * (i as f32 + 0.5);
        let mut p = Piece::new(model, c, yaw_along(d)).scaled(Vec3::new(step / pitch, 1.0, 1.0));
        p.span = step;
        out.push(p);
    }
}

const HEADSTONES: [(&str, f32); 10] = [
    ("gravestone_round", 3.0),
    ("gravestone_tall", 2.0),
    ("gravestone_wide", 1.4),
    ("gravestone_skull", 1.2),
    ("gravestone_broken", 1.2),
    ("gravestone_leaning", 1.0),
    ("gravestone_obelisk", 0.8),
    ("gravestone_rock", 1.0),
    ("grave_marker_planks", 1.2),
    ("grave_marker_post", 1.2),
];

const BONES: [(&str, f32); 7] = [
    ("bone_pile", 3.0),
    ("bone_pile_skull", 2.0),
    ("skull_pile", 1.5),
    ("beast_skulls", 1.2),
    ("loose_ribs", 1.5),
    ("crossed_bones", 2.0),
    ("vertebrae", 1.5),
];

// ------------------------------------------------------------------ drawn areas

/// The frame of a drawn outline: its centre, the set's facing (its long side along the set's x,
/// its front — the gate side — toward the camera) and the outline in that frame.
pub fn area_frame(poly: &[Vec2], cam: Vec2) -> (Vec2, f32, Vec<Vec2>) {
    let n = poly.len().max(1) as f32;
    let c = poly.iter().copied().sum::<Vec2>() / n;
    // principal axis of the outline (the corners, each weighted by its two edges)
    let (mut xx, mut xz, mut zz) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let w = a.distance(b);
        let m = (a + b) * 0.5 - c;
        xx += w * m.x * m.x;
        xz += w * m.x * m.y;
        zz += w * m.y * m.y;
    }
    let ang = 0.5 * (2.0 * xz).atan2(xx - zz);
    let long = Vec2::new(ang.cos(), ang.sin());
    let mut front = Vec2::new(-long.y, long.x);
    if front.dot(cam - c) < 0.0 {
        front = -front;
    }
    // local +z is the front: rot2((0, 1), facing) = (sin, cos)
    let facing = front.x.atan2(front.y);
    let local = poly.iter().map(|p| rot2(*p - c, -facing)).collect();
    (c, facing, local)
}

/// How many of a repeated thing (graves, stalls, lamps, bone piles) a set gets for its area:
/// `k` × √area, between `lo` and `hi`. A field four times as big gets twice as many, not four
/// times: big sets keep open ground to walk and fight in instead of filling up.
fn per_area(area: f32, k: f32, lo: usize, hi: usize) -> usize {
    ((area.sqrt() * k).round() as usize).clamp(lo, hi)
}

fn bounds(poly: &[Vec2]) -> (Vec2, Vec2) {
    poly.iter().fold(
        (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
        |(a, b), q| (a.min(*q), b.max(*q)),
    )
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn shuffle_in_place<T>(v: &mut [T], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i as u32 + 1) as usize);
    }
}

/// About how far a prop reaches from its centre (m), for keeping a group's props apart.
fn prop_reach(model: &str) -> f32 {
    match model {
        "market_stall" => 2.3,
        "hand_cart" | "wagon" => 1.3,
        "bench" => 1.0,
        "table" => 0.9,
        "haystack" | "water_trough" => 1.0,
        "crate" | "crate_open" | "apple_crate" | "cabbage_crate" => 0.6,
        _ => 0.45,
    }
}

/// Props set down in a few small groups (two barrels and a crate, a cart with sacks, a trough
/// and a bucket) the way a level designer dresses a space, instead of one by one evenly
/// round it: `n` groups at least `sep` apart, between `edge.0` and `edge.1` m inside the
/// outline, clear of everything in `taken`. Group kinds take turns so neighbours differ.
#[allow(clippy::too_many_arguments)]
fn groups(
    p: &mut Vec<Piece>,
    taken: &mut Vec<(Vec2, f32)>,
    poly: &[Vec2],
    kinds: &[&[&str]],
    n: usize,
    sep: f32,
    edge: (f32, f32),
    rng: &mut Rng,
) {
    let (lo, hi) = bounds(poly);
    let mut order: Vec<usize> = (0..kinds.len()).collect();
    shuffle_in_place(&mut order, rng);
    let mut spots: Vec<Vec2> = vec![];
    for _ in 0..n * 80 {
        if spots.len() >= n || kinds.is_empty() {
            break;
        }
        let c = Vec2::new(rng.range(lo.x, hi.x), rng.range(lo.y, hi.y));
        let d = -poly_sdf(poly, c);
        if d < edge.0
            || d > edge.1
            || spots.iter().any(|s| s.distance(c) < sep)
            || taken.iter().any(|(t, r)| t.distance(c) < r + 2.0)
        {
            continue;
        }
        let kind = kinds[order[spots.len() % order.len()]];
        let mut here: Vec<(Vec2, f32)> = vec![];
        for model in kind {
            let r = prop_reach(model);
            for attempt in 0..16 {
                let at = if here.is_empty() {
                    c
                } else {
                    c + Vec2::from_angle(rng.range(0.0, std::f32::consts::TAU))
                        * rng.range(0.9, 1.6 + attempt as f32 * 0.05)
                };
                if poly_sdf(poly, at) < -(r + 0.3)
                    && here.iter().all(|(q, qr)| q.distance(at) > qr + r + 0.15)
                {
                    p.push(Piece::new(model, at, rng.range(0.0, std::f32::consts::TAU)));
                    here.push((at, r));
                    break;
                }
            }
        }
        taken.push((c, 2.2));
        spots.push(c);
    }
}

/// The inside point farthest from the outline (and how far that is), searched on a grid.
fn deepest(poly: &[Vec2]) -> (Vec2, f32) {
    let (lo, hi) = bounds(poly);
    // (a long rectangle is equally deep all along its middle: take the point nearest the centre)
    let centre = (lo + hi) * 0.5;
    let mut best = (
        poly.iter().copied().sum::<Vec2>() / poly.len().max(1) as f32,
        0.0,
        f32::MIN,
    );
    let step = ((hi - lo).max_element() / 40.0).max(0.25);
    let mut z = lo.y;
    while z <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            let q = Vec2::new(x, z);
            let d = -poly_sdf(poly, q);
            let score = d - 0.05 * q.distance(centre);
            if d > 0.0 && score > best.2 {
                best = (q, d, score);
            }
            x += step;
        }
        z += step;
    }
    (best.0, best.1)
}

/// A set laid out inside a drawn outline (in the outline's frame, `area_frame`).
pub fn layout_area(kind: SetPieceKind, poly: &[Vec2], seed: u32) -> Result<Layout, String> {
    let area = poly_area(poly);
    let min = match kind {
        SetPieceKind::Graveyard => 50.0,
        SetPieceKind::Boneyard => 60.0,
        SetPieceKind::VillageSquare => 30.0,
        SetPieceKind::Market => 40.0,
    };
    if poly.len() < 3 || area < min {
        return Err(format!(
            "Draw a bigger area for a {} (at least {min:.0} m²; this is {area:.0} m²)",
            kind.name().to_lowercase()
        ));
    }
    let mut rng = Rng::new(seed as u64 ^ ((kind as u64 + 1) * 0x9E37_79B9) ^ 0xA5A5);
    let shape: std::sync::Arc<[Vec2]> = poly.to_vec().into();
    Ok(match kind {
        SetPieceKind::Graveyard => graveyard_in(poly, shape, area, &mut rng),
        SetPieceKind::Boneyard => boneyard_in(poly, shape, area, &mut rng),
        SetPieceKind::VillageSquare => square_in(poly, shape, area, &mut rng),
        SetPieceKind::Market => market_in(poly, shape, area, &mut rng)?,
    })
}

fn graveyard_in(poly: &[Vec2], shape: std::sync::Arc<[Vec2]>, area: f32, rng: &mut Rng) -> Layout {
    let n = poly.len();
    let inside = |p: Vec2, m: f32| poly_sdf(poly, p) < -m;
    let mut p = vec![];
    // the gate: on the longest front-facing side (outward normal toward +z, the camera)
    let outward = |a: Vec2, b: Vec2| {
        let d = (b - a).normalize_or_zero();
        let nrm = Vec2::new(d.y, -d.x);
        if inside((a + b) * 0.5 + nrm * 0.3, 0.0) {
            -nrm
        } else {
            nrm
        }
    };
    let gate_edge = (0..n)
        .filter(|i| poly[*i].distance(poly[(i + 1) % n]) >= 6.0)
        .max_by(|i, j| {
            let score = |k: usize| {
                outward(poly[k], poly[(k + 1) % n]).y * 2.0
                    + poly[k].distance(poly[(k + 1) % n]) * 0.05
            };
            score(*i).total_cmp(&score(*j))
        });
    let mut gate = None;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let d = (b - a).normalize_or_zero();
        p.push(Piece::new("iron_fence_corner", a, yaw_along(d)));
        if Some(i) == gate_edge {
            let m = (a + b) * 0.5;
            let out = outward(a, b);
            let mut yaw = yaw_along(d);
            if rot2(Vec2::new(0.0, 1.0), yaw).dot(out) < 0.0 {
                yaw += std::f32::consts::PI;
            }
            p.push(Piece::new("iron_gate", m, yaw));
            for s in [-1.0, 1.0] {
                p.push(Piece::new("stone_pillar", m + d * s * 1.95, yaw));
            }
            fence_run(&mut p, a, m - d * 2.38, "iron_fence", 2.4);
            fence_run(&mut p, b, m + d * 2.38, "iron_fence", 2.4);
            gate = Some((m, out));
        } else {
            fence_run(&mut p, a, b, "iron_fence", 2.4);
        }
    }
    let (gate_at, gate_out) = gate.unwrap_or((poly[0], Vec2::Y));
    let entry = gate_at - gate_out * 1.2;
    let (lo, hi) = bounds(poly);
    // inside points on a fine grid, for picking spots
    let grid = |step: f32, margin: f32| -> Vec<Vec2> {
        let mut v = vec![];
        let mut z = lo.y;
        while z <= hi.y {
            let mut x = lo.x;
            while x <= hi.x {
                let q = Vec2::new(x, z);
                if inside(q, margin) {
                    v.push(q);
                }
                x += step;
            }
            z += step;
        }
        v
    };
    let mut taken: Vec<(Vec2, f32)> = vec![(entry, 1.8)];
    let free = |taken: &[(Vec2, f32)], q: Vec2, pad: f32| {
        taken.iter().all(|(c, r)| c.distance(q) >= r + pad)
    };

    // crypts (a big cemetery has two or three) at the back, spread apart, each facing the gate,
    // each with a gravel path from the gate; a small yard gets a statue instead
    let crypts = if area < 110.0 {
        0
    } else if area < 1400.0 {
        1
    } else if area < 3200.0 {
        2
    } else {
        3
    };
    let mut paths: Vec<(Vec2, Vec2)> = vec![];
    let spots = grid(0.75, 3.8);
    let mut placed: Vec<Vec2> = vec![];
    for _ in 0..crypts {
        // far from the gate, and far from the other crypts
        let best = spots
            .iter()
            .copied()
            .filter(|q| free(&taken, *q, 2.0))
            .max_by(|a, b| {
                let score = |q: Vec2| {
                    q.distance(entry) * 0.6
                        + placed
                            .iter()
                            .map(|c| c.distance(q))
                            .fold(1e4f32, f32::min)
                            .min(40.0)
                            * 1.0
                };
                score(*a).total_cmp(&score(*b))
            });
        let Some(q) = best else { break };
        let g = (entry - q).normalize_or(Vec2::Y);
        p.push(Piece::new(
            "crypt",
            q,
            g.x.atan2(g.y) + rng.range(-0.08, 0.08),
        ));
        taken.push((q, 4.4));
        paths.push((entry, q + g * 3.4));
        placed.push(q);
        // candles at its door
        for s in [-1.0f32, 1.0] {
            let at = q + g * 3.3 + g.perp() * s * 0.9;
            p.push(Piece::new("candles", at, rng.range(0.0, 6.0)));
        }
    }
    if crypts == 0 {
        if let Some(q) = grid(0.5, 1.7)
            .into_iter()
            .max_by(|a, b| a.distance(entry).total_cmp(&b.distance(entry)))
        {
            let g = (entry - q).normalize_or(Vec2::Y);
            p.push(Piece::new("winged_statue", q, g.x.atan2(g.y)));
            taken.push((q, 1.6));
            paths.push((entry, q + g * 1.0));
        }
    }
    let near_path = |q: Vec2, w: f32| paths.iter().any(|&(a, b)| seg_dist(q, a, b) < w);

    // dead trees spread over the yard (3–4 in a big one), never on a path or crowding the crypts
    let trees = ((area.sqrt() / 13.0).round() as usize).clamp(1, 7);
    let tree_gap = (area.sqrt() / 3.2).max(7.0);
    let mut cand = grid(1.0, 1.8);
    shuffle_in_place(&mut cand, rng);
    let mut tree_at: Vec<Vec2> = vec![];
    for q in cand.iter().copied() {
        if tree_at.len() >= trees {
            break;
        }
        if near_path(q, 2.2)
            || !free(&taken, q, 1.5)
            || tree_at.iter().any(|t| t.distance(q) < tree_gap)
        {
            continue;
        }
        p.push(
            Piece::new("dead_tree", q, rng.range(0.0, std::f32::consts::TAU))
                .scaled(Vec3::splat(rng.range(1.2, 1.55))),
        );
        taken.push((q, 2.4));
        tree_at.push(q);
    }

    // winged statues beside the paths (one per ~900 m², up to three), candles at their feet
    let statues = ((area / 900.0) as usize).min(3);
    for k in 0..statues {
        let Some(&(a, b)) = paths.get(k % paths.len().max(1)) else {
            break;
        };
        let t = rng.range(0.35, 0.75);
        let side = (b - a).normalize_or_zero().perp() * if k % 2 == 0 { 2.7 } else { -2.7 };
        let at = a.lerp(b, t) + side;
        if inside(at, 1.3) && free(&taken, at, 1.0) {
            p.push(
                Piece::new(
                    "winged_statue",
                    at,
                    yaw_along(b - a)
                        + if k % 2 == 0 {
                            0.0
                        } else {
                            std::f32::consts::PI
                        },
                )
                .scaled(Vec3::splat(1.2)),
            );
            p.push(Piece::new(
                "candles",
                at + (b - a).normalize_or_zero() * 0.9,
                rng.range(0.0, 6.0),
            ));
            taken.push((at, 1.7));
        }
    }

    // open graves with their coffins (one per ~900 m², up to three), and a spare coffin by a crypt
    let opens = if area < 150.0 {
        0
    } else {
        ((area / 900.0).round() as usize).clamp(1, 3)
    };
    shuffle_in_place(&mut cand, rng);
    let mut made = 0;
    for q in cand.iter().copied() {
        if made >= opens {
            break;
        }
        if near_path(q, 2.0) || !inside(q, 2.4) || !free(&taken, q, 1.6) {
            continue;
        }
        let yaw = rng.range(-0.2, 0.2);
        p.push(Piece::new("open_grave", q, yaw));
        p.push(Piece::new(
            "coffin",
            q + rot2(Vec2::new(-1.6, 0.2), yaw),
            yaw + rng.range(0.0, 0.3),
        ));
        taken.push((q + rot2(Vec2::new(-0.5, 0.0), yaw), 2.8));
        made += 1;
    }
    if area >= 600.0
        && let Some(&c) = placed.first()
    {
        let g = (entry - c).normalize_or(Vec2::Y);
        let at = c + g * 4.2 + g.perp() * 2.4;
        if inside(at, 1.2) && free(&taken, at, 0.4) {
            p.push(Piece::new(
                "coffin",
                at,
                yaw_along(g.perp()) + rng.range(-0.3, 0.3),
            ));
            taken.push((at, 1.3));
        }
    }
    // benches along the main path in a big cemetery, facing it
    if area >= 1200.0
        && let Some(&(a, b)) = paths.first()
    {
        let d = (b - a).normalize_or_zero();
        for (t, s) in [(0.3f32, 1.0f32), (0.6, -1.0)]
            .into_iter()
            .take(if area >= 2400.0 { 2 } else { 1 })
        {
            let at = a.lerp(b, t) + d.perp() * s * 2.0;
            if inside(at, 1.0) && free(&taken, at, 0.6) {
                let face = -d.perp() * s;
                p.push(Piece::new("bench", at, face.x.atan2(face.y)));
                taken.push((at, 1.4));
            }
        }
    }

    // headstones spread over the whole yard: blue-noise spots (closer in a small yard, looser in a
    // big one), about 2.4 √area of them (30 in a small yard, 100 at most), some in little family
    // rows of two or three; facing the front, a little askew
    let target = per_area(area, 2.4, 8, 100);
    let usable = (area * 0.8).max(1.0);
    let gap = ((usable / target as f32).sqrt() * 0.72).clamp(1.7, 6.0);
    let mut stones: Vec<Vec2> = vec![];
    let mut spots = grid(0.35, 1.1);
    shuffle_in_place(&mut spots, rng);
    let headstone = |p: &mut Vec<Piece>, at: Vec2, rng: &mut Rng| {
        let model = weighted(rng, &HEADSTONES);
        let lean = if model == "gravestone_leaning" {
            Vec2::ZERO
        } else {
            Vec2::new(rng.range(-0.1, 0.1), rng.range(-0.08, 0.08))
        };
        p.push(
            Piece::new(model, at, rng.range(-0.25, 0.25))
                .tilted(lean)
                .sunk(rng.range(0.0, 0.06))
                .scaled(Vec3::splat(rng.range(1.2, 1.45))),
        );
        let r = rng.f32();
        if r < 0.12 {
            p.push(Piece::new(
                "candles",
                at + Vec2::new(rng.range(-0.5, 0.5), rng.range(0.45, 0.6)),
                rng.range(0.0, 6.0),
            ));
        } else if r < 0.3 {
            p.push(Piece::new(
                "withered_flowers",
                at + Vec2::new(rng.range(-0.3, 0.3), rng.range(0.4, 0.55)),
                rng.range(0.0, 6.0),
            ));
        }
    };
    for q in spots {
        if stones.len() >= target {
            break;
        }
        if near_path(q, 1.6) || !free(&taken, q, 0.2) || stones.iter().any(|s| s.distance(q) < gap)
        {
            continue;
        }
        headstone(&mut p, q, rng);
        stones.push(q);
        // a family row: one or two more beside it
        if rng.f32() < 0.3 {
            for k in 1..=(1 + rng.below(2) as i32) {
                let at = q + Vec2::new(
                    1.55 * k as f32 * if rng.f32() < 0.5 { 1.0 } else { -1.0 },
                    rng.range(-0.15, 0.15),
                );
                if stones.len() < target
                    && inside(at, 1.1)
                    && !near_path(at, 1.6)
                    && free(&taken, at, 0.2)
                    && stones.iter().all(|s| s.distance(at) > 1.3)
                {
                    headstone(&mut p, at, rng);
                    stones.push(at);
                }
            }
        }
    }
    let mut paint = vec![Paint {
        center: Vec2::ZERO,
        area: Area::new(shape.clone(), 0.3),
        layer: Layer::Dirt,
        patches: Some(Layer::DryGrass),
        strength: 0.85,
    }];
    for &(a, b) in &paths {
        let side = (b - a).normalize_or_zero().perp() * 0.95;
        let quad: std::sync::Arc<[Vec2]> = vec![a - side, b - side, b + side, a + side].into();
        paint.push(Paint {
            center: Vec2::ZERO,
            area: Area::new(quad, 0.0),
            layer: Layer::Gravel,
            patches: Some(Layer::Dirt),
            strength: 0.8,
        });
    }
    Layout {
        pieces: p,
        paint,
        clear: Area::new(shape, 1.5),
    }
}

fn boneyard_in(poly: &[Vec2], shape: std::sync::Arc<[Vec2]>, area: f32, rng: &mut Rng) -> Layout {
    let inside = |q: Vec2, m: f32| poly_sdf(poly, q) < -m;
    let (mid, room) = deepest(poly);
    // the skeleton's size follows the room it has; its spine runs along the long side
    let k = (room / 9.0).clamp(0.45, 1.5);
    let mut p = vec![];
    let mut taken: Vec<(Vec2, f32)> = vec![];
    let rib_yaw = -std::f32::consts::FRAC_PI_2 + rng.range(-0.25, 0.25);
    p.push(
        Piece::new("giant_ribcage", mid, rib_yaw)
            .sunk(0.6 * k)
            .scaled(Vec3::splat(k))
            .tilted(Vec2::new(rng.range(-0.05, 0.05), rng.range(-0.06, 0.06))),
    );
    taken.push((mid, 4.6 * k));
    let back = rot2(Vec2::new(0.0, -1.0), rib_yaw);
    let side = Vec2::new(-back.y, back.x);
    let mut at = mid + back * 4.6 * k;
    let mut dir = back;
    for i in 0..6 {
        let s = k * (1.0 - 0.12 * i as f32);
        let c = at + dir * 2.0 * s;
        if !inside(c, 1.2 * s) {
            break;
        }
        p.push(
            Piece::new("giant_vertebrae", c, yaw_along(dir))
                .scaled(Vec3::splat(s))
                .sunk(0.5 * s)
                .tilted(Vec2::new(rng.range(-0.06, 0.06), rng.range(-0.08, 0.08))),
        );
        taken.push((c, 2.4 * s));
        at = c + dir * 2.0 * s;
        dir = rot2(dir, rng.range(-0.3, 0.3));
    }
    for d in [8.0f32, 6.0, 4.5] {
        let c = mid - back * d * k + side * rng.range(-1.5, 1.5) * k;
        if inside(c, 2.0 * k) {
            p.push(
                Piece::new("giant_skull", c, rng.range(-0.6, 0.6))
                    .scaled(Vec3::splat(k))
                    .sunk(0.35 * k)
                    .tilted(Vec2::new(rng.range(-0.12, 0.0), rng.range(-0.15, 0.15))),
            );
            taken.push((c, 3.2 * k));
            break;
        }
    }
    for s in [1.0f32, -1.0] {
        let c = mid + side * s * 5.0 * k + back * rng.range(-1.5, 1.5) * k;
        if inside(c, 1.4 * k) {
            p.push(
                Piece::new("giant_claw", c, rng.range(0.0, std::f32::consts::TAU))
                    .scaled(Vec3::splat(k))
                    .sunk(0.3 * k),
            );
            taken.push((c, 2.2 * k));
        }
    }
    // a big boneyard holds a second, smaller giant: a skull and a trail of vertebrae half sunk,
    // as far from the first as it fits
    let (lo, hi) = bounds(poly);
    if area >= 900.0 {
        let k2 = k * 0.75;
        let mut best: Option<(Vec2, f32)> = None;
        let mut z = lo.y;
        while z <= hi.y {
            let mut x = lo.x;
            while x <= hi.x {
                let q = Vec2::new(x, z);
                if inside(q, 4.0 * k2) && !taken.iter().any(|(t, r)| t.distance(q) < r + 6.0) {
                    let d = q.distance(mid);
                    if best.is_none_or(|b| d > b.1) {
                        best = Some((q, d));
                    }
                }
                x += 1.0;
            }
            z += 1.0;
        }
        if let Some((c, _)) = best {
            let away = (c - mid).normalize_or(Vec2::X);
            p.push(
                Piece::new("giant_skull", c, rng.range(0.0, std::f32::consts::TAU))
                    .scaled(Vec3::splat(k2))
                    .sunk(0.55 * k2)
                    .tilted(Vec2::new(rng.range(-0.15, 0.0), rng.range(-0.2, 0.2))),
            );
            taken.push((c, 3.2 * k2));
            let mut at = c + away * 3.0 * k2;
            let mut dir = rot2(away, rng.range(-0.6, 0.6));
            for _ in 0..3 {
                if !inside(at, 1.2 * k2) {
                    break;
                }
                p.push(
                    Piece::new("giant_vertebrae", at, yaw_along(dir))
                        .scaled(Vec3::splat(k2))
                        .sunk(0.7 * k2)
                        .tilted(Vec2::new(rng.range(-0.1, 0.1), rng.range(-0.12, 0.12))),
                );
                taken.push((at, 2.2 * k2));
                at += dir * 2.1 * k2;
                dir = rot2(dir, rng.range(-0.4, 0.4));
            }
        }
    }
    // bone piles and skulls, thick round the giants and thinning out away from them, about
    // 1.3 √area of them (16 in a small boneyard, 40 at most)
    let giants: Vec<(Vec2, f32)> = taken.clone();
    let falloff = room.max(6.0);
    let n = per_area(area, 1.3, 6, 40);
    let mut placed = 0;
    for _ in 0..n * 60 {
        if placed >= n {
            break;
        }
        let c = Vec2::new(rng.range(lo.x, hi.x), rng.range(lo.y, hi.y));
        if !inside(c, 0.8) || taken.iter().any(|(t, rr)| t.distance(c) < *rr + 0.8) {
            continue;
        }
        let near = giants
            .iter()
            .map(|(t, r)| (t.distance(c) - r).max(0.0))
            .fold(f32::MAX, f32::min);
        if rng.f32() > (-near / falloff).exp() {
            continue;
        }
        let model = weighted(rng, &BONES);
        let sc = if model.contains("skull") {
            rng.range(1.1, 1.4)
        } else {
            rng.range(1.5, 2.2)
        };
        p.push(
            Piece::new(model, c, rng.range(0.0, std::f32::consts::TAU))
                .sunk(rng.range(0.0, 0.08))
                .scaled(Vec3::splat(sc)),
        );
        taken.push((c, 1.4));
        placed += 1;
    }
    Layout {
        pieces: p,
        paint: vec![Paint {
            center: Vec2::ZERO,
            area: Area::new(shape.clone(), 0.4),
            layer: Layer::Dirt,
            patches: Some(Layer::Sand),
            strength: 1.0,
        }],
        clear: Area::new(shape, 2.0),
    }
}

fn square_in(poly: &[Vec2], shape: std::sync::Arc<[Vec2]>, area: f32, rng: &mut Rng) -> Layout {
    let inside = |q: Vec2, m: f32| poly_sdf(poly, q) < -m;
    let (mid, room) = deepest(poly);
    let mut p = vec![Piece::new("well", mid, rng.range(-0.4, 0.4))];
    let mut taken: Vec<(Vec2, f32)> = vec![(mid, 2.2)];
    // benches round the well, facing it (2 in a small square, 6 at most)
    let benches = per_area(area, 0.17, 2, 6);
    let br = (room * 0.4).clamp(2.8, 6.0);
    for k in 0..benches {
        let a = std::f32::consts::TAU * k as f32 / benches as f32 + 0.3;
        let c = mid + Vec2::new(a.cos(), a.sin()) * br;
        if inside(c, 0.8) {
            let d = mid - c;
            p.push(Piece::new("bench", c, d.x.atan2(d.y)));
            taken.push((c, 1.4));
        }
    }
    // lamp posts evenly round the edge, their arms over the square: 3 in a small square, 8 at
    // most however big it is
    let n = poly.len();
    let lamps = per_area(area, 0.22, 3, 8);
    let perim = crate::water::lakes::perimeter(poly);
    let step = perim / lamps as f32;
    let mut next = step * rng.range(0.3, 0.7);
    let mut walked = 0.0f32;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let len = a.distance(b);
        while next < walked + len {
            let e = a.lerp(b, (next - walked) / len);
            let d = (b - a).normalize_or_zero();
            let mut inward = Vec2::new(-d.y, d.x);
            if !inside(e + inward * 0.5, 0.0) {
                inward = -inward;
            }
            let at = e + inward * 0.9;
            if inside(at, 0.4) && !taken.iter().any(|(t, r)| t.distance(at) < *r) {
                p.push(Piece::new("lamp_post", at, yaw_along(inward)));
                taken.push((at, 1.2));
            }
            next += step;
        }
        walked += len;
    }
    // the notice board at the back, the signpost at the front
    let (lo, hi) = bounds(poly);
    for (model, z, yaw) in [
        ("notice_board", lo.y + 1.4, 0.0),
        ("signpost", hi.y - 1.4, rng.range(-0.5, 0.5)),
    ] {
        let c = Vec2::new(mid.x + rng.range(-1.0, 1.0), z);
        if inside(c, 0.6) && !taken.iter().any(|(t, r)| t.distance(c) < *r + 0.6) {
            p.push(Piece::new(model, c, yaw));
            taken.push((c, 1.4));
        }
    }
    // a few small groups of props near the edge (2 in a small square, 7 at most), well apart
    let kinds: &[&[&str]] = &[
        &["barrel", "barrel_squat", "crate"],
        &["hand_cart", "sacks", "sacks"],
        &["haystack", "bucket"],
        &["water_trough", "bucket"],
        &["crate", "crate_open", "sacks"],
        &["flower_box_red", "flower_box_violet"],
    ];
    // a big square gets a few things going on out in the open too: tables outside, a stall,
    // a wagon being unloaded, a brazier by a bench
    if area >= 500.0 {
        let open: &[&[&str]] = &[
            &["table", "chair", "chair", "barrel"],
            &["market_stall", "apple_crate", "sacks"],
            &["wagon", "crate", "sacks"],
            &["brazier", "bench"],
        ];
        groups(
            &mut p,
            &mut taken,
            poly,
            open,
            per_area(area, 0.07, 1, 3),
            (area.sqrt() * 0.5).max(9.0),
            (5.0, f32::MAX),
            rng,
        );
    }
    groups(
        &mut p,
        &mut taken,
        poly,
        kinds,
        per_area(area, 0.14, 2, 7),
        (area.sqrt() * 0.55).max(7.0),
        (1.4, 4.5),
        rng,
    );
    Layout {
        pieces: p,
        paint: vec![Paint {
            center: Vec2::ZERO,
            area: Area::new(shape.clone(), 0.0),
            layer: Layer::Cobblestone,
            patches: Some(Layer::Dirt),
            strength: 0.9,
        }],
        clear: Area::new(shape, 1.5),
    }
}

fn market_in(
    poly: &[Vec2],
    shape: std::sync::Arc<[Vec2]>,
    area: f32,
    rng: &mut Rng,
) -> Result<Layout, String> {
    let inside = |q: Vec2, m: f32| poly_sdf(poly, q) < -m;
    // a stall's footprint (about 4.1 × 3.7 m) must be inside
    let fits = |c: Vec2| {
        [
            Vec2::new(-2.1, -1.9),
            Vec2::new(2.1, -1.9),
            Vec2::new(2.1, 1.9),
            Vec2::new(-2.1, 1.9),
            Vec2::ZERO,
        ]
        .iter()
        .all(|o| inside(c + *o, 0.3))
    };
    let goods: &[(&str, f32)] = &[
        ("apple_crate", 2.0),
        ("cabbage_crate", 2.0),
        ("sacks", 1.5),
        ("barrel", 1.2),
        ("barrel_squat", 1.0),
        ("crate", 1.2),
        ("flower_box_red", 0.6),
        ("bucket", 0.6),
    ];
    let (lo, hi) = bounds(poly);
    let (mid, _) = deepest(poly);
    // the stall slots of a row at depth z: centred on the stretch the row fits in, 4.8 m
    // apart, every fourth left out as a way through to the next aisle
    let slots = |z: f32| -> Vec<Vec2> {
        let xs: Vec<f32> = (0..=((hi.x - lo.x) / 0.25) as usize)
            .map(|i| lo.x + i as f32 * 0.25)
            .filter(|x| fits(Vec2::new(*x, z)))
            .collect();
        let (Some(first), Some(last)) = (xs.first().copied(), xs.last().copied()) else {
            return vec![];
        };
        let n = ((last - first) / 4.8).floor() as usize + 1;
        let x0 = (first + last) * 0.5 - (n - 1) as f32 * 2.4;
        (0..n)
            .filter(|k| n <= 4 || k % 4 != 3)
            .map(|k| Vec2::new(x0 + k as f32 * 4.8, z))
            .filter(|c| fits(*c))
            .collect()
    };
    // aisles across the long side with a row of stalls either side facing it (counters 3.4 m
    // apart); blocks of two rows (11 m deep) repeat every 14.5 m, centred in the area
    let depth = hi.y - lo.y;
    let blocks = (((depth - 11.6) / 14.5).floor() as i32 + 1).max(1);
    let span = 11.0 + (blocks - 1) as f32 * 14.5;
    // (slot, yaw, its aisle's z)
    let mut cands: Vec<(Vec2, f32, f32)> = vec![];
    for b in 0..blocks {
        let za = (lo.y + hi.y) * 0.5 + span * 0.5 - 5.5 - b as f32 * 14.5;
        cands.extend(
            slots(za + 3.6)
                .into_iter()
                .map(|c| (c, std::f32::consts::PI, za)),
        );
        cands.extend(slots(za - 3.6).into_iter().map(|c| (c, 0.0, za)));
    }
    if cands.is_empty() {
        // too shallow for an aisle: one row, counters toward the front
        let z = (lo.y + hi.y) * 0.5;
        cands.extend(slots(z).into_iter().map(|c| (c, 0.0, z + 3.6)));
    }
    if cands.is_empty() {
        return Err("Draw a wider area for a market (a stall needs about 5 × 4.5 m)".into());
    }
    // about 0.42 √area stalls (5 in a small market, 16 at most), the ones nearest the middle:
    // a busy cluster with open cobbles round it rather than stalls wall to wall
    let want = per_area(area, 0.42, 2, 16);
    let mut ranked: Vec<(f32, (Vec2, f32, f32))> = cands
        .into_iter()
        .map(|c| (c.0.distance(mid) + rng.range(0.0, 1.5), c))
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut chosen: Vec<(Vec2, f32, f32)> = ranked.into_iter().take(want).map(|r| r.1).collect();
    chosen.sort_by(|a, b| {
        (a.0.y, a.0.x)
            .partial_cmp(&(b.0.y, b.0.x))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut p = vec![];
    let mut taken: Vec<(Vec2, f32)> = vec![];
    let mut aisles: Vec<(f32, f32, f32)> = vec![];
    for (c, yaw, za) in chosen {
        let toward = if yaw == 0.0 { 1.0 } else { -1.0 };
        let c = c + Vec2::new(rng.range(-0.2, 0.2), 0.0);
        let yaw = yaw + rng.range(-0.06, 0.06);
        p.push(Piece::new("market_stall", c, yaw));
        for g in [-1.3f32, 1.3] {
            let at = Vec2::new(c.x + g + rng.range(-0.2, 0.2), c.y + toward * 1.75);
            p.push(Piece::new(
                weighted(rng, goods),
                at,
                rng.range(-0.4, 0.4) + yaw,
            ));
        }
        let at = Vec2::new(c.x + rng.range(-1.4, 1.4), c.y - toward * 1.6);
        if rng.f32() < 0.6 && inside(at, 0.4) {
            p.push(Piece::new(
                weighted(rng, goods),
                at,
                rng.range(0.0, std::f32::consts::TAU),
            ));
        }
        taken.push((c, 2.9));
        match aisles.iter_mut().find(|a| (a.0 - za).abs() < 0.1) {
            Some(a) => {
                a.1 = a.1.min(c.x);
                a.2 = a.2.max(c.x);
            }
            None => aisles.push((za, c.x, c.x)),
        }
    }
    // lamp posts at the aisles' ends, their arms over the aisle
    for (za, x0, x1) in &aisles {
        for (x, yaw) in [(x0 - 2.9, 0.0), (x1 + 2.9, std::f32::consts::PI)] {
            let c = Vec2::new(x, *za);
            if inside(c, 0.4) && !taken.iter().any(|(t, r)| t.distance(c) < *r - 0.6) {
                p.push(Piece::new("lamp_post", c, yaw));
                taken.push((c, 1.0));
            }
        }
    }
    // a wagon parked at one end if there's room
    let (za, x0, x1) = aisles[0];
    for c in [Vec2::new(x0 - 4.2, za), Vec2::new(x1 + 4.2, za)] {
        if [
            Vec2::new(-1.4, -2.2),
            Vec2::new(1.4, -2.2),
            Vec2::new(1.4, 2.2),
            Vec2::new(-1.4, 2.2),
        ]
        .iter()
        .all(|o| inside(c + *o, 0.2))
            && !taken.iter().any(|(t, r)| t.distance(c) < r + 1.4)
        {
            p.push(Piece::new(
                "wagon",
                c,
                std::f32::consts::FRAC_PI_2 + rng.range(-0.3, 0.3),
            ));
            taken.push((c, 2.6));
            break;
        }
    }
    // a few groups of stock waiting in the open (1 in a small market, 4 at most)
    let kinds: &[&[&str]] = &[
        &["crate", "crate", "barrel"],
        &["hand_cart", "sacks"],
        &["apple_crate", "cabbage_crate", "sacks"],
        &["barrel", "barrel_squat", "bucket"],
    ];
    groups(
        &mut p,
        &mut taken,
        poly,
        kinds,
        per_area(area, 0.07, 0, 4),
        9.0,
        (1.2, f32::MAX),
        rng,
    );
    Ok(Layout {
        pieces: p,
        paint: vec![Paint {
            center: Vec2::ZERO,
            area: Area::new(shape.clone(), 0.0),
            layer: Layer::Cobblestone,
            patches: Some(Layer::Dirt),
            strength: 0.9,
        }],
        clear: Area::new(shape, 1.5),
    })
}

/// A hand-drawn loop (world x, z) → a clean outline: the mouse's wobble straightened into
/// sides, corners closer than 2 m merged, counter-clockwise. None if it's too small to be a loop.
pub fn clean_outline(raw: &[Vec2]) -> Option<Vec<Vec2>> {
    if raw.len() < 4 {
        return None;
    }
    let total = crate::water::lakes::perimeter(raw);
    if total < 14.0 {
        return None;
    }
    let mut p = crate::water::lakes::resample_closed(raw, 1.0);
    // a little relaxation evens out the jitter
    for _ in 0..2 {
        let n = p.len();
        p = (0..n)
            .map(|i| p[i] * 0.5 + (p[(i + n - 1) % n] + p[(i + 1) % n]) * 0.25)
            .collect();
    }
    let mut p = simplify(&p, (total / 45.0).clamp(0.9, 3.0));
    merge_short(&mut p, 2.0);
    if crate::water::lakes::signed_area(&p) < 0.0 {
        p.reverse();
    }
    (p.len() >= 3 && poly_area(&p) >= 12.0).then_some(p)
}

/// Merges corners closer than `min` (the shortest side first).
fn merge_short(p: &mut Vec<Vec2>, min: f32) {
    while p.len() > 3 {
        let n = p.len();
        let Some(i) = (0..n)
            .filter(|i| p[*i].distance(p[(i + 1) % n]) < min)
            .min_by(|a, b| {
                p[*a]
                    .distance(p[(a + 1) % n])
                    .total_cmp(&p[*b].distance(p[(b + 1) % n]))
            })
        else {
            break;
        };
        let j = (i + 1) % n;
        p[i] = (p[i] + p[j]) * 0.5;
        p.remove(j);
    }
}

/// Lays a set out inside an outline (world x, z) and places it, its front toward the point
/// `toward` (the camera). Records into the open undo step (the caller begins and commits it).
/// Returns the new objects' ids.
fn missing_pack_error(kind: SetPieceKind) -> String {
    format!(
        "The {} set needs the optional hand-painted asset pack, which is not installed",
        kind.name().to_lowercase()
    )
}

pub fn stamp_area(
    d: &mut WorldData,
    kind: SetPieceKind,
    outline: &[Vec2],
    seed: u32,
    toward: Vec2,
) -> Result<Vec<u64>, String> {
    if !crate::asset_file("models/stylepacks/handpainted/models.json").is_file() {
        return Err(missing_pack_error(kind));
    }
    let mut outline = simplify(outline, 0.3);
    merge_short(&mut outline, 0.8);
    let (center, facing, local) = area_frame(&outline, toward);
    let lay = layout_area(kind, &local, seed)?;
    // a set needs its asset pack (crate::market)
    crate::market::check_set(
        &kind.name().to_lowercase(),
        lay.pieces.iter().map(|p| p.asset.as_str()),
    )?;
    stamp_layout(d, kind, &lay, seed, center, facing)
}

// ------------------------------------------------------------------ stamping

/// Places a laid-out set centred at `center` (x, z), its frame turned by `facing` (its +z points
/// along `Quat::from_rotation_y(facing) * Z`).
fn stamp_layout(
    d: &mut WorldData,
    kind: SetPieceKind,
    lay: &Layout,
    seed: u32,
    center: Vec2,
    facing: f32,
) -> Result<Vec<u64>, String> {
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading; try again in a moment")?;
    if !d.terrain.in_bounds(center.x, center.y) {
        return Err(format!(
            "[{:.0}, {:.0}] is outside the world",
            center.x, center.y
        ));
    }
    let set_rot = Quat::from_rotation_y(facing);
    let mut ids = vec![];
    // the set is one combined object: its pieces share a group (placement::group)
    let group = d.store.new_id();
    for pc in &lay.pieces {
        let Some(e) = lib.get(&pc.asset) else {
            continue;
        };
        let at = center + rot2(pc.at, facing);
        if !d.terrain.in_bounds(at.x, at.y) {
            continue;
        }
        let turn = set_rot * Quat::from_rotation_y(pc.yaw);
        let (y, lean) = if pc.span > 0.0 {
            // a fence section: level across, tilted along its run to meet the ground at both ends
            let ax = turn * Vec3::X;
            let half = Vec2::new(ax.x, ax.z) * pc.span * 0.5;
            let (h0, h1) = (
                d.terrain.height_at(at.x - half.x, at.y - half.y),
                d.terrain.height_at(at.x + half.x, at.y + half.y),
            );
            (
                (h0 + h1) * 0.5 - 0.06,
                Quat::from_rotation_z((h1 - h0).atan2(pc.span)),
            )
        } else {
            // on a slope, sink a little more so no edge floats
            let n = d.terrain.normal_at(at.x, at.y);
            let slope = (1.0 - n.y * n.y).max(0.0).sqrt() / n.y.max(0.2);
            let extra = slope * e.radius.min(2.0) * 0.45;
            (
                d.terrain.height_at(at.x, at.y) - 0.05 - pc.sink - extra,
                Quat::from_rotation_x(pc.tilt.x) * Quat::from_rotation_z(pc.tilt.y),
            )
        };
        let id = d.store.new_id();
        d.history.touch_object(&d.store, id);
        let mut p = Placed {
            group,
            id,
            asset: e.id.clone(),
            variant: 0,
            pos: [at.x, y, at.y],
            rot: (turn * lean).to_array(),
            scale: 1.0,
            stretch: None,
            locked: false,
        };
        p.set_scale3(pc.scale * e.default_scale);
        d.store.insert(p);
        ids.push(id);
    }
    // buildings level the ground under them
    {
        let WorldData {
            terrain,
            history,
            store,
            lib,
            water,
            fit,
            ..
        } = d;
        if let Some(lib) = lib.as_deref() {
            let mut w = crate::placement::FitWorld {
                terrain,
                history,
                store,
                lib,
                water,
                found: &mut fit.found,
                bounds: &fit.bounds,
            };
            crate::placement::fit::fit_objects(&mut w, &ids, &[]);
        }
    }
    for (k, paint) in lay.paint.iter().enumerate() {
        paint_ground(
            &mut d.terrain,
            &mut d.history,
            center + rot2(paint.center, facing),
            facing,
            paint,
            seed.wrapping_add(k as u32 * 101),
        );
    }
    let r = lay.clear.radius();
    let area = lay.clear.clone();
    crate::nature::paint_region(
        &mut d.nature,
        &mut d.history,
        kind.name(),
        center - Vec2::splat(r),
        center + Vec2::splat(r),
        move |q| area.sdf(rot2(q - center, -facing)) < 0.0,
        &crate::nature::field::Target::All,
        crate::nature::AreaOp::Keep(0.0),
    );
    // trees a few metres further out too, so their crowns don't hang over it
    let (r, area) = (r + TREE_CLEAR, lay.clear.clone());
    let trees = crate::nature::field::Target::Category(
        crate::nature::rules::cat_index(crate::objects::Category::Trees).unwrap_or(0),
    );
    crate::nature::paint_region(
        &mut d.nature,
        &mut d.history,
        kind.name(),
        center - Vec2::splat(r),
        center + Vec2::splat(r),
        move |q| area.sdf(rot2(q - center, -facing)) < TREE_CLEAR,
        &trees,
        crate::nature::AreaOp::Keep(0.0),
    );
    d.meta.dirty = true;
    Ok(ids)
}

/// How much further out than the rest of nature trees are cleared (m).
const TREE_CLEAR: f32 = 4.5;

/// Blends the ground paint toward a layer over an area with a soft, ragged edge.
fn paint_ground(
    t: &mut Terrain,
    history: &mut crate::history::History,
    center: Vec2,
    facing: f32,
    p: &Paint,
    seed: u32,
) {
    let Some(rect) = t.rect_for_circle(center, p.area.radius() + 2.5) else {
        return;
    };
    history.touch_terrain(t, rect);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let w = t.grid_to_world(i, j);
            let local = rot2(w - center, -facing);
            let edge = fbm(w.x / 6.0, w.y / 6.0, 2, 2.0, 0.5, seed) * 1.6;
            let k = smoothstep(1.0, -1.4, p.area.sdf(local) + edge) * p.strength;
            if k <= 0.004 {
                continue;
            }
            let layer = match p.patches {
                Some(l) if fbm(w.x / 4.5, w.y / 4.5, 2, 2.0, 0.5, seed + 9) > 0.18 => l,
                _ => p.layer,
            };
            let idx = t.idx(i, j);
            let s = &mut t.splat[idx];
            for (l, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
                let target = if l == layer as usize { 255.0 } else { 0.0 };
                *v = (*v as f32 + (target - *v as f32) * k)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            let sum: u32 = s.iter().map(|v| *v as u32).sum();
            if sum > 255 {
                let f = 255.0 / sum as f32;
                s.iter_mut()
                    .for_each(|v| *v = (*v as f32 * f).floor() as u8);
            }
        }
    }
    t.mark_splat_dirty(rect);
}

// ------------------------------------------------------------------ editor tool

/// The last set placed (Shuffle re-rolls it in place).
#[derive(Clone, Debug)]
pub struct LastSet {
    pub kind: SetPieceKind,
    /// its outline (world x, z) and where its front faced
    pub outline: Vec<Vec2>,
    pub toward: Vec2,
    pub ids: Vec<u64>,
}

/// Objects tab › Set pieces (kept for the session).
#[derive(Resource)]
pub struct SetPieceSettings {
    /// the set that fills the next loop drawn on the ground (None: placing single objects)
    pub selected: Option<SetPieceKind>,
    pub seed: u32,
    pub last: Option<LastSet>,
    /// Shuffle was pressed (handled by `shuffle`)
    pub shuffle: bool,
}

impl Default for SetPieceSettings {
    fn default() -> Self {
        Self {
            selected: None,
            seed: 1,
            last: None,
            shuffle: false,
        }
    }
}

pub struct SetPiecesPlugin;

impl Plugin for SetPiecesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SetPieceSettings>()
            .add_systems(Update, test_hooks)
            .add_systems(Update, draw_place.in_set(crate::tools::ToolSystems::Apply))
            .add_systems(
                Update,
                (shuffle, cursor).after(crate::tools::ToolSystems::Commit),
            );
        register_mcp(app);
    }
}

/// Test hooks (screenshots): `WB_STYLE=<pack>` sets the Objects tab's Style filter and
/// `WB_SETPIECE=<kind>` selects a set piece.
fn test_hooks(
    mut done: Local<bool>,
    mut sets: ResMut<SetPieceSettings>,
    mut place: ResMut<crate::objects::PlaceSettings>,
    mut ed: ResMut<crate::editor::Editor>,
) {
    if std::mem::replace(&mut *done, true) {
        return;
    }
    if let Ok(s) = std::env::var("WB_STYLE") {
        place.style = s;
    }
    if let Some(k) = std::env::var("WB_SETPIECE")
        .ok()
        .and_then(|k| SetPieceKind::ALL.into_iter().find(|x| x.id() == k))
    {
        sets.selected = Some(k);
        place.selected = None;
        ed.object_tool = ObjectTool::Place;
    }
}

/// The set fills the loop dragged on the ground (like drawing a lake): one undo step with the
/// stroke.
fn draw_place(
    stroke: Res<crate::tools::Stroke>,
    handles: Res<crate::placement::Handles>,
    mut sets: ResMut<SetPieceSettings>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut on_handle: Local<bool>,
    mut d: WorldData,
) {
    let Some(kind) = sets.selected else { return };
    if d.ed.tab != Tab::Objects || d.ed.object_tool != ObjectTool::Place {
        return;
    }
    if stroke.just_started {
        // a press on a move / turn / resize handle drags that, not a loop
        *on_handle = handles.took_press;
    }
    if !stroke.just_ended || std::mem::take(&mut *on_handle) {
        return;
    }
    let raw: Vec<Vec2> = stroke.path.iter().map(|p| p.xz()).collect();
    let Some(outline) = clean_outline(&raw) else {
        d.ed.toast(format!(
            "Hold the mouse and draw a loop round the area for the {} — like drawing a lake",
            kind.name().to_lowercase()
        ));
        return;
    };
    let toward = cam
        .single()
        .map(|c| c.translation().xz())
        .unwrap_or(outline[0]);
    d.history
        .set_label(format!("Place {}", kind.name().to_lowercase()));
    let seed = sets.seed;
    match stamp_area(&mut d, kind, &outline, seed, toward) {
        Ok(ids) => {
            let area = poly_area(&outline);
            info!(
                "SETPIECE {} placed: {} objects over {area:.0} m²",
                kind.id(),
                ids.len()
            );
            d.ed.toast(format!(
                "{} placed: {} pieces over {area:.0} m² — Shuffle re-rolls it",
                kind.name(),
                ids.len()
            ));
            d.store.selected = None;
            sets.last = Some(LastSet {
                kind,
                outline,
                toward,
                ids,
            });
        }
        Err(e) => {
            warn!("SETPIECE {}: {e}", kind.id());
            d.ed.toast(e);
        }
    }
}

/// Shuffle: the last set again in the same outline with the next seed (or just the next seed).
fn shuffle(mut sets: ResMut<SetPieceSettings>, mut d: WorldData) {
    if !std::mem::take(&mut sets.shuffle) {
        return;
    }
    sets.seed = sets.seed.wrapping_add(1);
    let Some(last) = sets.last.clone() else {
        return;
    };
    if !last.ids.iter().any(|id| d.store.items.contains_key(id)) {
        sets.last = None;
        return;
    }
    let kind = sets.selected.unwrap_or(last.kind);
    d.history
        .begin(format!("Shuffle {}", kind.name().to_lowercase()));
    for id in &last.ids {
        if d.store.items.contains_key(id) {
            d.history.touch_object(&d.store, *id);
            d.store.remove(*id);
        }
    }
    let seed = sets.seed;
    let res = stamp_area(&mut d, kind, &last.outline, seed, last.toward);
    d.history.commit(&d.store);
    match res {
        Ok(ids) => sets.last = Some(LastSet { kind, ids, ..last }),
        Err(e) => d.ed.toast(e),
    }
}

/// A ring under the cursor while a set piece waits for its loop (the loop itself is drawn by
/// the UI's stroke preview).
fn cursor(
    sets: Res<SetPieceSettings>,
    ed: Res<crate::editor::Editor>,
    pointer: Res<crate::camera::Pointer>,
    stroke: Res<crate::tools::Stroke>,
    terrain: Res<Terrain>,
    mut gizmos: Gizmos,
) {
    if sets.selected.is_none()
        || ed.tab != Tab::Objects
        || ed.object_tool != ObjectTool::Place
        || pointer.over_ui
        || stroke.active
    {
        return;
    }
    let Some(hit) = pointer.hit else { return };
    let ring: Vec<Vec3> = (0..=32)
        .map(|i| {
            let q = hit.xz() + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 32.0) * 1.2;
            Vec3::new(q.x, terrain.height_at(q.x, q.y) + 0.25, q.y)
        })
        .collect();
    gizmos.linestrip(ring, Color::srgba(1.0, 0.85, 0.4, 0.9));
}

// ------------------------------------------------------------------ MCP

fn register_mcp(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "objects_place_set_piece",
        description: "Fill an area with a whole set piece in one call, like drawing a loop with Objects › Set pieces: a graveyard (iron fence along the outline, arched gate on the front side, rows of headstones and markers, crypt at the far end, dead trees, open grave, candles), a boneyard (a giant half-buried skeleton scaled to the area: skull, walk-under ribcage, vertebrae, claws; bone piles and skulls), a village_square (well, lamps along the edge, benches, notice board, barrels) or a market (rows of striped stalls along aisles, with produce). \
Give the area as `polygon` [[x, z], …] (at least 3 corners; bigger areas get more graves / stalls / bones; minimums: graveyard 50 m², boneyard 60, village_square 30, market 40), or `position` + `size` for a default rectangle / circle there. \
Hand-painted style pack models; every piece becomes a normal object (objects_query / objects_transform / objects_remove work on them). Fences follow the ground, buildings level it, the ground inside is painted (dark dirt and dead grass / dry dirt / cobbles) and nature is cleared. The set's long side lies along the area's long side and its front (the graveyard gate) faces `face_toward` [x, z] (default: the camera); with `position`, `yaw_deg` turns it instead. `seed` picks the arrangement (same seed and area, same layout). Returns the ids. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": SetPieceKind::ALL.map(|k| k.id()) },
                    "polygon": { "type": "array", "minItems": 3, "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3 }, "description": "the area's outline, [[x, z], …] in metres" },
                    "face_toward": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3, "description": "[x, z] the set's front faces (default: the camera)" },
                    "position": crate::mcp_tools::position_schema("centre of a default-sized set (instead of `polygon`)"),
                    "size": { "type": "string", "enum": SetSize::ALL.map(|s| s.id()), "default": "medium", "description": "with `position`" },
                    "seed": { "type": "integer", "minimum": 0, "description": "arrangement (default: a new one)" },
                    "yaw_deg": crate::mcp_tools::bearing_schema("with `position`: which way the set's front faces")
                },
                "required": ["kind"],
                "x-aliases": crate::mcp_tools::xz_aliases()
            })
        },
        run: |w, a| w.run_system_cached_with(mcp_place, a).map_err(|e| format!("{e}"))?,
    });
}

fn xz(v: &Value, what: &str) -> Result<Vec2, String> {
    let f: Vec<f32> = v
        .as_array()
        .ok_or_else(|| format!("`{what}` must be [x, z]"))?
        .iter()
        .filter_map(|v| v.as_f64())
        .map(|v| v as f32)
        .collect();
    match f[..] {
        [x, z] | [x, _, z] => Ok(Vec2::new(x, z)),
        _ => Err(format!("`{what}` must be [x, z]")),
    }
}

fn mcp_place(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let kind_s = args
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or("missing `kind`")?;
    let kind = SetPieceKind::ALL
        .into_iter()
        .find(|k| k.id() == kind_s)
        .ok_or_else(|| {
            format!("unknown kind `{kind_s}` (graveyard, boneyard, village_square, market)")
        })?;
    let seed = match args.get("seed") {
        None | Some(Value::Null) => fastrand::u32(..),
        Some(v) => v.as_u64().ok_or("`seed` must be a whole number")? as u32,
    };
    let o = &*d.orbit;
    let cam = (o.focus + Vec3::new(o.yaw.sin(), 0.0, o.yaw.cos()) * 100.0).xz();
    let (outline, toward) = if let Some(poly) = args.get("polygon").filter(|v| !v.is_null()) {
        let pts = poly
            .as_array()
            .ok_or("`polygon` must be [[x, z], …]")?
            .iter()
            .map(|p| xz(p, "polygon corner"))
            .collect::<Result<Vec<Vec2>, String>>()?;
        if pts.len() < 3 {
            return Err("`polygon` needs at least 3 corners".into());
        }
        let toward = match args.get("face_toward").filter(|v| !v.is_null()) {
            Some(v) => xz(v, "face_toward")?,
            None => cam,
        };
        (pts, toward)
    } else {
        let center = xz(
            args.get("position")
                .ok_or("give the area as `polygon` [[x, z], …] (or `position` [x, z] + `size`)")?,
            "position",
        )?;
        let size = match args.get("size").and_then(|v| v.as_str()) {
            None => SetSize::Medium,
            Some(s) => SetSize::ALL
                .into_iter()
                .find(|x| x.id() == s)
                .ok_or_else(|| format!("unknown size `{s}` (small, medium, large)"))?,
        };
        let facing = match args.get("yaw_deg").and_then(|v| v.as_f64()) {
            // compass bearing of the front: 0 = +Z, clockwise seen from above
            Some(deg) => -(deg as f32).to_radians(),
            None => {
                let d = cam - center;
                if d.length_squared() < 1e-6 {
                    0.0
                } else {
                    d.x.atan2(d.y)
                }
            }
        };
        let pts = size
            .outline(kind)
            .into_iter()
            .map(|p| center + rot2(p, facing))
            .collect();
        (pts, center + rot2(Vec2::new(0.0, 1000.0), facing))
    };
    d.history
        .begin(format!("Place {}", kind.name().to_lowercase()));
    let res = stamp_area(&mut d, kind, &outline, seed, toward);
    d.history.commit(&d.store);
    let ids = res?;
    let group = ids
        .first()
        .and_then(|id| d.store.items.get(id))
        .map(|p| p.group)
        .unwrap_or(0);
    Ok(vec![McpContent::Json(
        json!({ "kind": kind.id(), "seed": seed, "area_m2": poly_area(&outline).round(), "placed": ids.len(), "group": group, "ids": ids, "note": "one undo step; the set is one group (objects_group moves, turns or removes it as one)" }),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(hw: f32, hd: f32) -> Vec<Vec2> {
        vec![
            Vec2::new(-hw, -hd),
            Vec2::new(hw, -hd),
            Vec2::new(hw, hd),
            Vec2::new(-hw, hd),
        ]
    }

    /// An L-shaped and a slanted, wobbly outline besides plain rectangles and circles.
    fn shapes() -> Vec<Vec<Vec2>> {
        let l = vec![
            Vec2::new(-12.0, -9.0),
            Vec2::new(12.0, -9.0),
            Vec2::new(12.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 9.0),
            Vec2::new(-12.0, 9.0),
        ];
        let blob: Vec<Vec2> = (0..40)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 40.0;
                Vec2::new(a.cos() * 16.0, a.sin() * 9.0) * (1.0 + 0.08 * (a * 3.0).sin())
            })
            .collect();
        let mut v = vec![l, blob];
        for z in SetSize::ALL {
            for k in SetPieceKind::ALL {
                v.push(z.outline(k));
            }
        }
        v
    }

    #[test]
    fn layouts_are_deterministic_per_seed() {
        for k in SetPieceKind::ALL {
            let poly = rect(11.0, 8.0);
            assert_eq!(layout_area(k, &poly, 7), layout_area(k, &poly, 7), "{k:?}");
            assert_ne!(
                layout_area(k, &poly, 7),
                layout_area(k, &poly, 8),
                "{k:?}: a new seed re-rolls it"
            );
        }
    }

    #[test]
    fn missing_handpainted_pack_is_explained() {
        let error = missing_pack_error(SetPieceKind::Graveyard);
        assert!(error.contains("graveyard") && error.contains("not installed"));
    }

    #[test]
    fn layouts_use_pack_models_and_stay_inside() {
        let manifest = crate::asset_file("models/stylepacks/handpainted/models.json");
        let ids = manifest.is_file().then(|| {
            let doc: Value = serde_json::from_str(&std::fs::read_to_string(manifest).unwrap())
                .unwrap();
            doc["models"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| format!("hp:{}", m["id"].as_str().unwrap()))
                .collect::<Vec<_>>()
        });
        for poly in shapes() {
            let (_, _, local) = area_frame(&poly, Vec2::new(0.0, 100.0));
            for k in SetPieceKind::ALL {
                let Ok(l) = layout_area(k, &local, 3) else {
                    continue;
                };
                assert!(
                    l.pieces.len() >= 6,
                    "{k:?} {:.0} m²: {} pieces",
                    poly_area(&poly),
                    l.pieces.len()
                );
                for p in &l.pieces {
                    if let Some(ids) = &ids {
                        assert!(
                            ids.contains(&p.asset),
                            "{k:?}: {} is not in the pack",
                            p.asset
                        );
                    }
                    // fences run on the outline; everything else inside it
                    assert!(
                        poly_sdf(&local, p.at) < 0.05,
                        "{k:?} {:.0} m²: {} at {:?} outside",
                        poly_area(&poly),
                        p.asset,
                        p.at
                    );
                }
            }
        }
        if ids.is_some() {
            // the big rectangle: a fence all round with one gate on the front side, a crypt
            let g = layout_area(SetPieceKind::Graveyard, &rect(13.5, 10.5), 3).unwrap();
            let gates: Vec<&Piece> = g
                .pieces
                .iter()
                .filter(|p| p.asset == "hp:iron_gate")
                .collect();
            assert_eq!(gates.len(), 1);
            assert!(
                gates[0].at.y > 10.0 && gates[0].yaw.abs() < 1e-4,
                "the gate is at the front, facing out: {:?}",
                gates[0]
            );
            assert_eq!(
                g.pieces
                    .iter()
                    .filter(|p| p.asset == "hp:iron_fence_corner")
                    .count(),
                4
            );
            assert!(
                g.pieces
                    .iter()
                    .filter(|p| p.asset == "hp:iron_fence")
                    .count()
                    >= 16
            );
            assert!(g.pieces.iter().any(|p| p.asset == "hp:crypt"));
            // bigger areas hold more graves
            let graves = |hw: f32, hd: f32| {
                layout_area(SetPieceKind::Graveyard, &rect(hw, hd), 3)
                    .unwrap()
                    .pieces
                    .iter()
                    .filter(|p| p.asset.contains("grave"))
                    .count()
            };
            assert!(graves(6.5, 5.5) < graves(9.5, 7.5) && graves(9.5, 7.5) < graves(13.5, 10.5));
            let stalls = |hw: f32, hd: f32| {
                layout_area(SetPieceKind::Market, &rect(hw, hd), 3)
                    .unwrap()
                    .pieces
                    .iter()
                    .filter(|p| p.asset == "hp:market_stall")
                    .count()
            };
            assert!(
                stalls(6.5, 5.2) < stalls(12.0, 5.2)
                    && stalls(12.0, 5.2) < stalls(12.0, 11.0)
            );
            // the boneyard's giant pieces are all there, sunk into the ground
            let b = layout_area(SetPieceKind::Boneyard, &rect(16.0, 12.0), 3).unwrap();
            for part in [
                "hp:giant_skull",
                "hp:giant_ribcage",
                "hp:giant_vertebrae",
                "hp:giant_claw",
            ] {
                assert!(
                    b.pieces.iter().any(|p| p.asset == part && p.sink > 0.2),
                    "{part}"
                );
            }
        }
        // too small: a clear message
        assert!(
            layout_area(SetPieceKind::Graveyard, &rect(2.0, 2.0), 3)
                .unwrap_err()
                .contains("bigger")
        );
    }

    #[test]
    fn big_areas_stay_open() {
        // a 50 × 40 m field: content grows with √area, not area, and spreads out
        let big = rect(25.0, 20.0);
        let count = |k: SetPieceKind, f: &dyn Fn(&str) -> bool| {
            layout_area(k, &big, 5)
                .unwrap()
                .pieces
                .iter()
                .filter(|p| f(&p.asset))
                .count()
        };
        let graves = count(SetPieceKind::Graveyard, &|a| {
            a.contains("gravestone") || a.contains("grave_marker")
        });
        assert!((60..=100).contains(&graves), "{graves} graves");
        // …spread over the whole yard (every quarter has graves), with room to walk between them
        let g = layout_area(SetPieceKind::Graveyard, &big, 5).unwrap();
        let stones: Vec<Vec2> = g
            .pieces
            .iter()
            .filter(|p| p.asset.contains("gravestone") || p.asset.contains("grave_marker"))
            .map(|p| p.at)
            .collect();
        for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let n = stones
                .iter()
                .filter(|s| s.x * sx > 0.0 && s.y * sz > 0.0)
                .count();
            assert!(n >= 8, "quarter ({sx}, {sz}) has {n} graves");
        }
        let open = (0..20)
            .flat_map(|i| {
                (0..16).map(move |j| Vec2::new(-22.0 + i as f32 * 2.3, -17.0 + j as f32 * 2.3))
            })
            .filter(|q| stones.iter().all(|s| s.distance(*q) > 3.0))
            .count();
        assert!(open > 40, "only {open} open spots");
        // a big cemetery: two crypts, several trees, open graves with coffins, candles and flowers
        let n = |a: &str| g.pieces.iter().filter(|p| p.asset == a).count();
        assert_eq!(n("hp:crypt"), 2);
        assert!(
            (3..=5).contains(&n("hp:dead_tree")),
            "{} trees",
            n("hp:dead_tree")
        );
        assert!(n("hp:open_grave") >= 2 && n("hp:coffin") >= n("hp:open_grave"));
        assert!(n("hp:candles") >= 6 && n("hp:withered_flowers") >= 8);
        assert!(count(SetPieceKind::Market, &|a| a == "hp:market_stall") <= 16);
        assert!(count(SetPieceKind::VillageSquare, &|a| a == "hp:lamp_post") <= 8);
        assert!(
            count(SetPieceKind::VillageSquare, &|_| true) <= 40,
            "{}",
            count(SetPieceKind::VillageSquare, &|_| true)
        );
        assert!(count(SetPieceKind::Boneyard, &|a| !a.contains("giant")) <= 40);
        // a big boneyard has a second giant
        assert_eq!(count(SetPieceKind::Boneyard, &|a| a == "hp:giant_skull"), 2);
        // props come in small groups, not one by one round the edge
        let sq = layout_area(SetPieceKind::VillageSquare, &big, 5).unwrap();
        let props: Vec<Vec2> = sq
            .pieces
            .iter()
            .filter(|p| {
                [
                    "barrel",
                    "crate",
                    "sacks",
                    "haystack",
                    "bucket",
                    "hand_cart",
                    "water_trough",
                    "flower_box",
                ]
                .iter()
                .any(|m| p.asset.contains(m))
            })
            .map(|p| p.at)
            .collect();
        let lonely = props
            .iter()
            .filter(|a| props.iter().filter(|b| a.distance(**b) < 3.5).count() < 2)
            .count();
        assert!(
            lonely * 4 <= props.len(),
            "{lonely} of {} props stand alone",
            props.len()
        );
    }

    /// How far the spot in the yard farthest from any grave is from one (spots 4 m inside the fence).
    fn widest_gap(poly: &[Vec2], stones: &[Vec2]) -> f32 {
        let (lo, hi) = bounds(poly);
        let mut gap = 0.0f32;
        let mut z = lo.y;
        while z <= hi.y {
            let mut x = lo.x;
            while x <= hi.x {
                let q = Vec2::new(x, z);
                if poly_sdf(poly, q) < -4.0 {
                    gap = gap.max(
                        stones
                            .iter()
                            .map(|s| s.distance(q))
                            .fold(f32::MAX, f32::min),
                    );
                }
                x += 1.0;
            }
            z += 1.0;
        }
        gap
    }

    #[test]
    fn big_yards_spread_their_graves() {
        // a ~7900 m² graveyard drawn in the editor (world corners, seen from the west): its 100
        // graves used to crowd the gate → crypt path, the far half of the yard bare
        let world = [
            (536.6, 1016.5),
            (511.7, 1002.1),
            (498.8, 954.2),
            (513.3, 905.5),
            (531.4, 879.2),
            (551.4, 873.7),
            (579.6, 920.9),
            (569.3, 989.9),
            (548.4, 1006.5),
        ]
        .map(|(x, z)| Vec2::new(x, z));
        let (_, _, drawn) = area_frame(&world, Vec2::new(330.0, 880.0));
        for (poly, seed) in [
            (drawn.clone(), 3),
            (drawn, 11),
            (rect(40.0, 25.0), 5),
            (rect(75.0, 50.0), 2),
        ] {
            let l = layout_area(SetPieceKind::Graveyard, &poly, seed).unwrap();
            let stones: Vec<Vec2> = l
                .pieces
                .iter()
                .filter(|p| p.asset.contains("gravestone") || p.asset.contains("grave_marker"))
                .map(|p| p.at)
                .collect();
            let area = poly_area(&poly);
            assert!(
                (60..=100).contains(&stones.len()),
                "{area:.0} m²: {} graves",
                stones.len()
            );
            let gap = widest_gap(&poly, &stones);
            assert!(
                gap < 0.3 * area.sqrt(),
                "{area:.0} m²: a spot {gap:.1} m from any grave"
            );
        }
    }

    #[test]
    fn drawn_loops_become_clean_outlines() {
        // a wobbly hand-drawn rectangle, points every ~2 m like the stroke
        let mut raw = vec![];
        let corners = rect(10.0, 6.0);
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            let n = (a.distance(b) / 2.0) as usize;
            for k in 0..n {
                let t = k as f32 / n as f32;
                raw.push(
                    a.lerp(b, t) + Vec2::new((k as f32 * 1.7).sin(), (k as f32 * 2.3).cos()) * 0.25,
                );
            }
        }
        let p = clean_outline(&raw).unwrap();
        assert!((4..=8).contains(&p.len()), "{} corners", p.len());
        assert!((poly_area(&p) - 240.0).abs() < 30.0, "{}", poly_area(&p));
        assert!(crate::water::lakes::signed_area(&p) > 0.0);
        // a click (or a tiny scribble) isn't a loop
        assert!(clean_outline(&[Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE]).is_none());
        // the frame lays the long side along x, the front toward the camera
        let (c, facing, local) = area_frame(&p, Vec2::new(0.0, -50.0));
        assert!(c.length() < 1.0);
        let front = rot2(Vec2::new(0.0, 1.0), facing);
        assert!(front.y < -0.95, "front toward the camera: {front:?}");
        let (lo, hi) = local.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(a, b), q| (a.min(*q), b.max(*q)),
        );
        assert!(hi.x - lo.x > hi.y - lo.y);
    }

    #[test]
    fn fence_runs_close_their_gaps() {
        let mut out = vec![];
        fence_run(
            &mut out,
            Vec2::new(-5.0, 0.0),
            Vec2::new(5.0, 0.0),
            "iron_fence",
            2.4,
        );
        assert_eq!(out.len(), 4);
        let total: f32 = out.iter().map(|p| p.span).sum();
        assert!((total - 10.0).abs() < 1e-4);
        assert!(out.iter().all(|p| (p.scale.x * 2.4 - p.span).abs() < 1e-4));
        // a run along -z turns the sections' x along it
        let mut v = vec![];
        fence_run(
            &mut v,
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, -4.8),
            "iron_fence",
            2.4,
        );
        let x = Quat::from_rotation_y(v[0].yaw) * Vec3::X;
        assert!((x - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-4);
    }

    #[test]
    fn rot2_matches_the_yaw_quaternion() {
        for a in [0.0f32, 0.7, 2.0, -1.3] {
            let v = Vec2::new(1.5, -0.4);
            let q = Quat::from_rotation_y(a) * Vec3::new(v.x, 0.0, v.y);
            assert!((rot2(v, a) - Vec2::new(q.x, q.z)).length() < 1e-5);
        }
    }
}
