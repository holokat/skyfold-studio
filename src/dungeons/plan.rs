//! The plan of a dungeon: its levels and the rooms drawn on them (rooms, halls, corridors,
//! caverns, stairs, pits, pools, daises, bridges), and what that makes in 3D.
//!
//! Every room is an outline on its level plus a floor and a ceiling (each a height at any point
//! of the outline: flat, a stair's ramp, a vault's curve, a cavern's lumps). The space the plan
//! makes is the union of the rooms' air minus solids (pillars, daises, bridges, railings).
//! `Compiled` answers every question about it — is a point in the air, where are the floor and
//! the roof, how far is the nearest wall — and both the meshes (`mesh`) and the walker, the
//! camera and the pointer (`space`) ask it, so what you see is what you bump into.

use bevy::math::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::geom::{self, rot};

/// Metres between the floors of two levels (a new level's default).
pub const LEVEL_STEP: f32 = 14.0;

/// An outline on the plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Shape {
    /// a rectangle: centre, half size, turned by `yaw` (radians)
    Rect {
        c: [f32; 2],
        half: [f32; 2],
        #[serde(default)]
        yaw: f32,
    },
    /// a regular polygon (an octagon unless told): centre, radius to its corners
    Ngon {
        c: [f32; 2],
        r: f32,
        #[serde(default = "eight")]
        sides: u8,
        #[serde(default)]
        yaw: f32,
    },
    /// a circle
    Round { c: [f32; 2], r: f32 },
    /// a drawn outline
    Poly { pts: Vec<[f32; 2]> },
    /// a passage along a path (its centreline) of a width
    Path { pts: Vec<[f32; 2]>, width: f32 },
}

fn eight() -> u8 {
    8
}

fn v2(a: [f32; 2]) -> Vec2 {
    Vec2::new(a[0], a[1])
}

impl Shape {
    pub fn rect(c: Vec2, half: Vec2) -> Shape {
        Shape::Rect {
            c: c.to_array(),
            half: half.to_array(),
            yaw: 0.0,
        }
    }
    pub fn path(pts: &[Vec2], width: f32) -> Shape {
        Shape::Path {
            pts: pts.iter().map(|p| p.to_array()).collect(),
            width,
        }
    }
    /// The corners of a polygon-like shape (rect, n-gon, drawn outline).
    pub fn corners(&self) -> Option<Vec<Vec2>> {
        match self {
            Shape::Rect { c, half, yaw } => {
                let (c, h) = (v2(*c), v2(*half));
                Some(
                    [
                        Vec2::new(-h.x, -h.y),
                        Vec2::new(h.x, -h.y),
                        Vec2::new(h.x, h.y),
                        Vec2::new(-h.x, h.y),
                    ]
                    .into_iter()
                    .map(|q| c + rot(q, *yaw))
                    .collect(),
                )
            }
            Shape::Ngon { c, r, sides, yaw } => {
                let n = (*sides).max(3) as usize;
                let c = v2(*c);
                Some(
                    (0..n)
                        .map(|k| {
                            c + Vec2::from_angle(
                                yaw + std::f32::consts::TAU * k as f32 / n as f32
                                    + std::f32::consts::PI / n as f32,
                            ) * *r
                        })
                        .collect(),
                )
            }
            Shape::Poly { pts } => Some(pts.iter().map(|p| v2(*p)).collect()),
            _ => None,
        }
    }
    /// Signed distance to the outline: positive inside (metres).
    pub fn sdf(&self, p: Vec2) -> f32 {
        match self {
            Shape::Rect { c, half, yaw } => {
                let q = rot(p - v2(*c), -*yaw).abs() - v2(*half);
                -(q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0))
            }
            Shape::Round { c, r } => *r - p.distance(v2(*c)),
            Shape::Path { pts, width } => {
                let pts: Vec<Vec2> = pts.iter().map(|a| v2(*a)).collect();
                width * 0.5 - geom::polyline_dist(&pts, p).0
            }
            _ => geom::poly_sdf(&self.corners().unwrap_or_default(), p),
        }
    }
    pub fn bounds(&self) -> (Vec2, Vec2) {
        match self {
            Shape::Round { c, r } => (v2(*c) - Vec2::splat(*r), v2(*c) + Vec2::splat(*r)),
            Shape::Path { pts, width } => {
                let mut lo = Vec2::splat(f32::MAX);
                let mut hi = Vec2::splat(f32::MIN);
                for p in pts {
                    lo = lo.min(v2(*p));
                    hi = hi.max(v2(*p));
                }
                (lo - Vec2::splat(width * 0.5), hi + Vec2::splat(width * 0.5))
            }
            _ => {
                let cs = self.corners().unwrap_or_default();
                let lo = cs.iter().fold(Vec2::splat(f32::MAX), |a, b| a.min(*b));
                let hi = cs.iter().fold(Vec2::splat(f32::MIN), |a, b| a.max(*b));
                (lo, hi)
            }
        }
    }
    pub fn centre(&self) -> Vec2 {
        match self {
            Shape::Rect { c, .. } | Shape::Ngon { c, .. } | Shape::Round { c, .. } => v2(*c),
            Shape::Poly { pts } => {
                pts.iter().map(|p| v2(*p)).sum::<Vec2>() / pts.len().max(1) as f32
            }
            Shape::Path { pts, .. } => {
                let ps: Vec<Vec2> = pts.iter().map(|p| v2(*p)).collect();
                geom::polyline_at(&ps, geom::polyline_len(&ps) * 0.5).0
            }
        }
    }
    pub fn translate(&mut self, d: Vec2) {
        match self {
            Shape::Rect { c, .. } | Shape::Ngon { c, .. } | Shape::Round { c, .. } => {
                *c = (v2(*c) + d).to_array()
            }
            Shape::Poly { pts } | Shape::Path { pts, .. } => {
                pts.iter_mut().for_each(|p| *p = (v2(*p) + d).to_array())
            }
        }
    }
    /// Distance from the outline to the middle (the largest circle that fits, roughly).
    pub fn inradius(&self) -> f32 {
        match self {
            Shape::Rect { half, .. } => half[0].min(half[1]),
            Shape::Ngon { r, sides, .. } => {
                r * (std::f32::consts::PI / (*sides).max(3) as f32).cos()
            }
            Shape::Round { r, .. } => *r,
            Shape::Path { width, .. } => width * 0.5,
            Shape::Poly { .. } => {
                let (lo, hi) = self.bounds();
                let mut best: f32 = 0.5;
                let n = 16;
                for j in 0..=n {
                    for i in 0..=n {
                        let p = lo + (hi - lo) * Vec2::new(i as f32, j as f32) / n as f32;
                        best = best.max(self.sdf(p));
                    }
                }
                best
            }
        }
    }
    /// The outline as a loop (a path: its offset outline), for drawing and walls.
    pub fn outline(&self) -> Vec<Vec2> {
        match self {
            Shape::Round { c, r } => {
                let n = ((std::f32::consts::TAU * r / 0.6).ceil() as usize).clamp(24, 360);
                (0..n)
                    .map(|k| {
                        v2(*c) + Vec2::from_angle(std::f32::consts::TAU * k as f32 / n as f32) * *r
                    })
                    .collect()
            }
            Shape::Path { width, .. } => {
                let (lo, hi) = self.bounds();
                let m = Vec2::splat(0.5);
                let cell = (width * 0.08).clamp(0.12, 0.3);
                let loops = geom::contours(lo - m, hi + m, cell, |p| self.sdf(p));
                let lp = loops
                    .into_iter()
                    .max_by(|a, b| {
                        geom::signed_area(a)
                            .abs()
                            .total_cmp(&geom::signed_area(b).abs())
                    })
                    .unwrap_or_default();
                geom::simplify_loop(&lp, 0.02)
            }
            _ => self.corners().unwrap_or_default(),
        }
    }
    /// The long axis (unit) and the half length / half width along and across it.
    pub fn axes(&self) -> (Vec2, f32, f32) {
        match self {
            Shape::Rect { half, yaw, .. } => {
                if half[0] >= half[1] {
                    (rot(Vec2::X, *yaw), half[0], half[1])
                } else {
                    (rot(Vec2::Y, *yaw), half[1], half[0])
                }
            }
            Shape::Round { r, .. } | Shape::Ngon { r, .. } => (Vec2::X, *r, *r),
            Shape::Path { pts, width } => {
                let ps: Vec<Vec2> = pts.iter().map(|p| v2(*p)).collect();
                let d = (ps.last().copied().unwrap_or_default()
                    - ps.first().copied().unwrap_or_default())
                .normalize_or(Vec2::X);
                (d, geom::polyline_len(&ps) * 0.5, width * 0.5)
            }
            Shape::Poly { .. } => {
                let (lo, hi) = self.bounds();
                let h = (hi - lo) * 0.5;
                if h.x >= h.y {
                    (Vec2::X, h.x, h.y)
                } else {
                    (Vec2::Y, h.y, h.x)
                }
            }
        }
    }
    /// A path's centreline.
    pub fn centreline(&self) -> Option<Vec<Vec2>> {
        match self {
            Shape::Path { pts, .. } => Some(pts.iter().map(|p| v2(*p)).collect()),
            _ => None,
        }
    }
    /// An uneven version of the shape (`Room::irregular`) that never reaches inside the original,
    /// so the corridors and doorways that met it still do. Rectangles and polygons get
    /// straight-walled jogs and alcoves (the width changes along the room, the walls stay built);
    /// circles get smooth bulges. `amount` 0..1: how far the walls reach out, as a share of the
    /// room's size. Paths and drawn outlines stay as they are.
    pub fn irregular(&self, seed: u32, amount: f32) -> Shape {
        let mut rng = crate::noise::Rng::new(seed as u64 ^ 0x1e6e_5a1d);
        let amount = amount.clamp(0.0, 1.0);
        match self {
            Shape::Rect { .. } | Shape::Ngon { .. } => {
                let cs = self.corners().unwrap_or_default();
                let n = cs.len();
                let c = self.centre();
                let reach = (amount * self.inradius() * 0.8).clamp(0.4, 6.0);
                // each wall split into 1–3 stretches, each pushed out by its own amount (some not)
                let mut walls: Vec<(Vec2, Vec2, Vec2, Vec<(f32, f32, f32)>)> =
                    Vec::with_capacity(n);
                for k in 0..n {
                    let (a, b) = (cs[k], cs[(k + 1) % n]);
                    let len = a.distance(b).max(1e-3);
                    let dir = (b - a) / len;
                    let mut out = dir.perp();
                    if out.dot((a + b) * 0.5 - c) < 0.0 {
                        out = -out;
                    }
                    // (more jogs the stronger it is)
                    let parts = if len < 4.0 {
                        1
                    } else {
                        1 + rng.below(2 + (amount * 2.0) as u32) as usize
                    };
                    let mut cuts: Vec<f32> = (1..parts).map(|_| rng.range(0.2, 0.8)).collect();
                    cuts.sort_by(f32::total_cmp);
                    let mut ts = vec![0.0];
                    ts.extend(cuts);
                    ts.push(1.0);
                    // offsets in whole half metres: a jog is never a sliver of wall (whose top
                    // misses a vaulted ceiling); neighbours that come out the same merge
                    let mut stretches: Vec<(f32, f32, f32)> = Vec::new();
                    for w in ts.windows(2) {
                        let off = if rng.f32() < 0.3 {
                            0.0
                        } else {
                            ((rng.range(0.25, 1.0) * reach) / 0.5).round().max(1.0) * 0.5
                        };
                        match stretches.last_mut() {
                            Some(last) if (last.2 - off).abs() < 0.25 => last.1 = w[1] * len,
                            _ => stretches.push((w[0] * len, w[1] * len, off)),
                        }
                    }
                    walls.push((a, dir, out, stretches));
                }
                let mut pts: Vec<Vec2> = Vec::new();
                for k in 0..n {
                    let (a, dir, out, st) = &walls[k];
                    for (s0, s1, off) in st {
                        pts.push(*a + *dir * *s0 + *out * *off);
                        pts.push(*a + *dir * *s1 + *out * *off);
                    }
                    // the corner where this wall's last stretch meets the next wall's first
                    let (b2, dir2, out2, st2) = &walls[(k + 1) % n];
                    let (o1, o2) = (
                        st.last().map(|x| x.2).unwrap_or(0.0),
                        st2.first().map(|x| x.2).unwrap_or(0.0),
                    );
                    let p1 = *b2 + *out * o1;
                    let p2 = *b2 + *out2 * o2;
                    let det = dir.perp_dot(*dir2);
                    if det.abs() > 1e-3 {
                        let t = (p2 - p1).perp_dot(*dir2) / det;
                        pts.push(p1 + *dir * t);
                    }
                }
                pts.dedup_by(|a, b| a.distance(*b) < 0.05);
                Shape::Poly {
                    pts: pts.into_iter().map(|p| p.to_array()).collect(),
                }
            }
            Shape::Round { c, r } => {
                let (c, r) = (v2(*c), *r);
                let reach = amount * 0.45;
                let ph = [
                    rng.range(0.0, 6.28),
                    rng.range(0.0, 6.28),
                    rng.range(0.0, 6.28),
                ];
                let n = ((std::f32::consts::TAU * r / 1.2).ceil() as usize).clamp(24, 96);
                let pts = (0..n)
                    .map(|k| {
                        let a = std::f32::consts::TAU * k as f32 / n as f32;
                        let f = 0.5
                            + 0.5
                                * ((2.0 * a + ph[0]).sin() * 0.5
                                    + (3.0 * a + ph[1]).sin() * 0.33
                                    + (5.0 * a + ph[2]).sin() * 0.17);
                        (c + Vec2::from_angle(a) * r * (1.0 + reach * f.clamp(0.0, 1.0))).to_array()
                    })
                    .collect();
                Shape::Poly { pts }
            }
            _ => self.clone(),
        }
    }
    pub fn kind_name(&self) -> &'static str {
        match self {
            Shape::Rect { .. } => "rect",
            Shape::Ngon { .. } => "octagon",
            Shape::Round { .. } => "round",
            Shape::Poly { .. } => "outline",
            Shape::Path { .. } => "path",
        }
    }
}

/// What a room is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    /// a chamber
    #[default]
    Room,
    /// a great hall: tall, pillared
    Hall,
    /// a passage between rooms
    Corridor,
    /// natural rock: lumpy floor, a domed roof, rough walls
    Cavern,
    /// a stairway from its level to another, along its path
    Stairs,
    /// a shaft: no floor, open down (to the level below, or a drop)
    Pit,
    /// a sunken basin of water, lava, slime or blood
    Pool,
    /// a raised platform with steps up (inside a room)
    Dais,
    /// a walkway across a pit or a chasm (railings, no walls)
    Bridge,
    /// a gallery inside a taller room at its floor's height: a deck with railings where it
    /// overlooks a drop and posts down to the floor below
    Balcony,
    /// a deep rift in the rock: rough walls, no railings, a long drop (or lava) at the bottom
    Chasm,
    /// a round tower stair: steps winding round a central column from its level to `to_level`
    Spiral,
}

impl RoomKind {
    pub const ALL: [RoomKind; 12] = [
        RoomKind::Room,
        RoomKind::Hall,
        RoomKind::Corridor,
        RoomKind::Cavern,
        RoomKind::Stairs,
        RoomKind::Pit,
        RoomKind::Pool,
        RoomKind::Dais,
        RoomKind::Bridge,
        RoomKind::Balcony,
        RoomKind::Chasm,
        RoomKind::Spiral,
    ];
    pub fn name(self) -> &'static str {
        match self {
            RoomKind::Room => "Room",
            RoomKind::Hall => "Hall",
            RoomKind::Corridor => "Corridor",
            RoomKind::Cavern => "Cavern",
            RoomKind::Stairs => "Stairs",
            RoomKind::Pit => "Pit",
            RoomKind::Pool => "Pool",
            RoomKind::Dais => "Dais",
            RoomKind::Bridge => "Bridge",
            RoomKind::Balcony => "Balcony",
            RoomKind::Chasm => "Chasm",
            RoomKind::Spiral => "Spiral stair",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            RoomKind::Room => "room",
            RoomKind::Hall => "hall",
            RoomKind::Corridor => "corridor",
            RoomKind::Cavern => "cavern",
            RoomKind::Stairs => "stairs",
            RoomKind::Pit => "pit",
            RoomKind::Pool => "pool",
            RoomKind::Dais => "dais",
            RoomKind::Bridge => "bridge",
            RoomKind::Balcony => "balcony",
            RoomKind::Chasm => "chasm",
            RoomKind::Spiral => "spiral",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// Its air is space (walls round it); daises, bridges and balconies are solids in another
    /// room's air.
    pub fn is_space(self) -> bool {
        !matches!(self, RoomKind::Dais | RoomKind::Bridge | RoomKind::Balcony)
    }
    /// A drop through the level's floor (pits and chasms).
    pub fn is_pit(self) -> bool {
        matches!(self, RoomKind::Pit | RoomKind::Chasm)
    }
    /// Walls, floor and edges of natural rock.
    pub fn is_rock(self) -> bool {
        matches!(self, RoomKind::Cavern | RoomKind::Chasm)
    }
    /// Default clear height (m).
    pub fn default_height(self) -> f32 {
        match self {
            RoomKind::Room => 6.5,
            RoomKind::Hall => 12.0,
            RoomKind::Corridor | RoomKind::Stairs => 4.6,
            RoomKind::Spiral => 3.8,
            RoomKind::Cavern => 10.0,
            RoomKind::Pit | RoomKind::Pool | RoomKind::Chasm => 6.5,
            RoomKind::Dais => 0.9,
            RoomKind::Bridge | RoomKind::Balcony => 0.0,
        }
    }
    /// Which floor shows where rooms overlap (higher wins).
    fn priority(self) -> u8 {
        match self {
            RoomKind::Pit | RoomKind::Pool | RoomKind::Chasm => 5,
            RoomKind::Stairs | RoomKind::Spiral => 4,
            RoomKind::Cavern => 2,
            _ => 1,
        }
    }
}

/// The shape of a ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Ceiling {
    /// flat
    #[default]
    Flat,
    /// flat with heavy beams across
    Beams,
    /// a round barrel vault (a dome over round rooms)
    Vault,
    /// a pointed (gothic) vault
    Pointed,
    /// stepped corbels (temples)
    Stepped,
    /// none: darkness overhead (vast halls, chasms)
    Open,
    /// natural rock (caverns)
    Rock,
}

impl Ceiling {
    pub const ALL: [Ceiling; 7] = [
        Ceiling::Flat,
        Ceiling::Beams,
        Ceiling::Vault,
        Ceiling::Pointed,
        Ceiling::Stepped,
        Ceiling::Open,
        Ceiling::Rock,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Ceiling::Flat => "Flat",
            Ceiling::Beams => "Beams",
            Ceiling::Vault => "Vault",
            Ceiling::Pointed => "Pointed",
            Ceiling::Stepped => "Stepped",
            Ceiling::Open => "Open",
            Ceiling::Rock => "Rock",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Ceiling::Flat => "flat",
            Ceiling::Beams => "beams",
            Ceiling::Vault => "vault",
            Ceiling::Pointed => "pointed",
            Ceiling::Stepped => "stepped",
            Ceiling::Open => "open",
            Ceiling::Rock => "rock",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// Curved (the walls stop at the spring line, the rest is the curve).
    pub fn curved(self) -> bool {
        matches!(
            self,
            Ceiling::Vault | Ceiling::Pointed | Ceiling::Stepped | Ceiling::Rock
        )
    }
}

/// Pillars standing in a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Pillars {
    #[default]
    None,
    /// two rows along the room
    Rows,
    /// a ring round the middle
    Ring,
    /// a grid (a forest of columns)
    Grid,
}

impl Pillars {
    pub const ALL: [Pillars; 4] = [Pillars::None, Pillars::Rows, Pillars::Ring, Pillars::Grid];
    pub fn name(self) -> &'static str {
        match self {
            Pillars::None => "None",
            Pillars::Rows => "Rows",
            Pillars::Ring => "Ring",
            Pillars::Grid => "Grid",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Pillars::None => "none",
            Pillars::Rows => "rows",
            Pillars::Ring => "ring",
            Pillars::Grid => "grid",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
}

/// What a room is for (what Dress puts in it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    None,
    /// where players arrive (the exit portal)
    Entrance,
    /// the final fight: a raised dais, braziers, banners
    Boss,
    Treasure,
    Prison,
    Crypt,
    Shrine,
    Library,
    Forge,
    Barracks,
    Throne,
    Storage,
    Lair,
    Ritual,
    Arena,
}

impl Role {
    pub const ALL: [Role; 15] = [
        Role::None,
        Role::Entrance,
        Role::Boss,
        Role::Treasure,
        Role::Prison,
        Role::Crypt,
        Role::Shrine,
        Role::Library,
        Role::Forge,
        Role::Barracks,
        Role::Throne,
        Role::Storage,
        Role::Lair,
        Role::Ritual,
        Role::Arena,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Role::None => "—",
            Role::Entrance => "Entrance",
            Role::Boss => "Boss",
            Role::Treasure => "Treasure",
            Role::Prison => "Prison",
            Role::Crypt => "Crypt",
            Role::Shrine => "Shrine",
            Role::Library => "Library",
            Role::Forge => "Forge",
            Role::Barracks => "Barracks",
            Role::Throne => "Throne room",
            Role::Storage => "Storage",
            Role::Lair => "Lair",
            Role::Ritual => "Ritual",
            Role::Arena => "Arena",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Role::None => "none",
            Role::Entrance => "entrance",
            Role::Boss => "boss",
            Role::Treasure => "treasure",
            Role::Prison => "prison",
            Role::Crypt => "crypt",
            Role::Shrine => "shrine",
            Role::Library => "library",
            Role::Forge => "forge",
            Role::Barracks => "barracks",
            Role::Throne => "throne",
            Role::Storage => "storage",
            Role::Lair => "lair",
            Role::Ritual => "ritual",
            Role::Arena => "arena",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
}

/// What fills a pool (or the bottom of a pit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Liquid {
    #[default]
    None,
    Water,
    Lava,
    Slime,
    Blood,
    Arcane,
}

impl Liquid {
    pub const ALL: [Liquid; 6] = [
        Liquid::None,
        Liquid::Water,
        Liquid::Lava,
        Liquid::Slime,
        Liquid::Blood,
        Liquid::Arcane,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Liquid::None => "None",
            Liquid::Water => "Water",
            Liquid::Lava => "Lava",
            Liquid::Slime => "Slime",
            Liquid::Blood => "Blood",
            Liquid::Arcane => "Arcane",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Liquid::None => "none",
            Liquid::Water => "water",
            Liquid::Lava => "lava",
            Liquid::Slime => "slime",
            Liquid::Blood => "blood",
            Liquid::Arcane => "arcane",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
}

/// A room (of any kind) drawn on a level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Room {
    pub id: u32,
    #[serde(default)]
    pub kind: RoomKind,
    #[serde(default)]
    pub level: i32,
    pub shape: Shape,
    #[serde(default)]
    pub name: String,
    /// clear height from its floor to the top of its ceiling (m); a dais's rise
    pub height: f32,
    /// its floor above (+) or below (−) its level (m); a pool's depth (−)
    #[serde(default)]
    pub floor: f32,
    #[serde(default)]
    pub ceiling: Ceiling,
    #[serde(default)]
    pub pillars: Pillars,
    #[serde(default)]
    pub role: Role,
    /// a style of its own (`styles::Style::id`)
    #[serde(default)]
    pub style: Option<String>,
    /// kept by Shuffle
    #[serde(default)]
    pub locked: bool,
    /// stairs: the level they lead to
    #[serde(default)]
    pub to_level: Option<i32>,
    /// pit: how far it drops when nothing is under it (m)
    #[serde(default)]
    pub depth: f32,
    /// pool / pit bottom
    #[serde(default)]
    pub liquid: Liquid,
    /// stairs: a smooth ramp (no steps)
    #[serde(default)]
    pub smooth: bool,
    /// made uneven (`Shape::irregular`): the regular shape it was made from, so it can be
    /// reshaped, resized or turned back
    #[serde(default)]
    pub irregular: Option<Irregular>,
}

/// How a room was made irregular.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Irregular {
    pub base: Shape,
    pub seed: u32,
    /// 0..1: how far the walls reach out
    pub amount: f32,
}

impl Room {
    pub fn new(id: u32, kind: RoomKind, level: i32, shape: Shape) -> Room {
        let ceiling = match kind {
            RoomKind::Cavern => Ceiling::Rock,
            RoomKind::Corridor | RoomKind::Stairs | RoomKind::Spiral => Ceiling::Vault,
            _ => Ceiling::Flat,
        };
        let pillars = if kind == RoomKind::Hall {
            Pillars::Rows
        } else {
            Pillars::None
        };
        let (floor, liquid) = match kind {
            RoomKind::Pool => (-1.4, Liquid::Water),
            _ => (0.0, Liquid::None),
        };
        Room {
            id,
            kind,
            level,
            shape,
            name: String::new(),
            height: kind.default_height(),
            floor,
            ceiling,
            pillars,
            role: Role::None,
            style: None,
            locked: false,
            to_level: match kind {
                RoomKind::Stairs => Some(level - 1),
                RoomKind::Spiral => Some(level + 1),
                _ => None,
            },
            depth: match kind {
                RoomKind::Pit => LEVEL_STEP,
                RoomKind::Chasm => 40.0,
                _ => 0.0,
            },
            liquid,
            smooth: false,
            irregular: None,
        }
    }
    /// Rooms and halls drawn as a rectangle, polygon or circle can be made irregular.
    pub fn can_be_irregular(&self) -> bool {
        let base = self.irregular.as_ref().map_or(&self.shape, |i| &i.base);
        matches!(self.kind, RoomKind::Room | RoomKind::Hall)
            && matches!(
                base,
                Shape::Rect { .. } | Shape::Ngon { .. } | Shape::Round { .. }
            )
    }
    /// Makes it uneven (from its regular shape; again with a new `seed` to reshape it).
    pub fn set_irregular(&mut self, seed: u32, amount: f32) {
        if !self.can_be_irregular() {
            return;
        }
        let base = self
            .irregular
            .take()
            .map_or_else(|| self.shape.clone(), |i| i.base);
        self.shape = base.irregular(seed, amount);
        self.irregular = Some(Irregular { base, seed, amount });
    }
    /// Back to the regular shape it was made from.
    pub fn set_regular(&mut self) {
        if let Some(i) = self.irregular.take() {
            self.shape = i.base;
        }
    }
    /// A new regular shape (resized, turned): an irregular room keeps its unevenness round it.
    pub fn set_shape(&mut self, shape: Shape) {
        match self.irregular.as_mut() {
            Some(i) => {
                i.base = shape;
                self.shape = i.base.irregular(i.seed, i.amount);
            }
            None => self.shape = shape,
        }
    }
    /// Moves it (and the shape it was made irregular from).
    pub fn translate(&mut self, d: Vec2) {
        self.shape.translate(d);
        if let Some(i) = self.irregular.as_mut() {
            i.base.translate(d);
        }
    }
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            format!("{} {}", self.kind.name(), self.id)
        } else {
            self.name.clone()
        }
    }
}

/// A floor of the dungeon.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Level {
    pub id: i32,
    /// height of its floor (m)
    pub y: f32,
    #[serde(default)]
    pub name: String,
}

/// The whole plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub levels: Vec<Level>,
    pub rooms: Vec<Room>,
    #[serde(default)]
    pub next_id: u32,
    #[serde(default)]
    pub seed: u32,
}

impl Default for Plan {
    fn default() -> Self {
        Plan {
            levels: vec![Level {
                id: 0,
                y: 0.0,
                name: "Ground".into(),
            }],
            rooms: vec![],
            next_id: 0,
            seed: 1,
        }
    }
}

impl Plan {
    pub fn alloc_id(&mut self) -> u32 {
        self.next_id = self
            .next_id
            .max(self.rooms.iter().map(|r| r.id).max().unwrap_or(0))
            + 1;
        self.next_id
    }
    pub fn level_y(&self, id: i32) -> f32 {
        self.levels
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.y)
            .unwrap_or(id as f32 * LEVEL_STEP)
    }
    /// Makes sure level `id` exists (new ones are `LEVEL_STEP` apart).
    pub fn ensure_level(&mut self, id: i32) {
        if self.levels.iter().any(|l| l.id == id) {
            return;
        }
        let name = match id {
            0 => "Ground".to_string(),
            i if i < 0 => format!("Depth {}", -i),
            i => format!("Upper {i}"),
        };
        self.levels.push(Level {
            id,
            y: id as f32 * LEVEL_STEP,
            name,
        });
        self.levels.sort_by_key(|l| -l.id);
    }
    pub fn room(&self, id: u32) -> Option<&Room> {
        self.rooms.iter().find(|r| r.id == id)
    }
    pub fn room_mut(&mut self, id: u32) -> Option<&mut Room> {
        self.rooms.iter_mut().find(|r| r.id == id)
    }
    /// Adds a room (its level made if missing); its id.
    pub fn add(&mut self, mut r: Room) -> u32 {
        let id = self.alloc_id();
        r.id = id;
        self.ensure_level(r.level);
        if let Some(t) = r.to_level {
            self.ensure_level(t);
        }
        self.rooms.push(r);
        id
    }
    pub fn remove(&mut self, id: u32) -> bool {
        let n = self.rooms.len();
        self.rooms.retain(|r| r.id != id);
        self.rooms.len() != n
    }
    /// Levels that have no rooms (except 0) are dropped.
    pub fn prune_levels(&mut self) {
        let used: std::collections::HashSet<i32> = self
            .rooms
            .iter()
            .flat_map(|r| std::iter::once(r.level).chain(r.to_level))
            .collect();
        self.levels.retain(|l| l.id == 0 || used.contains(&l.id));
    }
    /// The topmost room of `level` at `p` (for picking in the plan).
    pub fn room_at(&self, level: i32, p: Vec2) -> Option<u32> {
        self.rooms
            .iter()
            .filter(|r| r.level == level && r.shape.sdf(p) > 0.0)
            .max_by_key(|r| {
                (
                    matches!(
                        r.kind,
                        RoomKind::Dais
                            | RoomKind::Bridge
                            | RoomKind::Balcony
                            | RoomKind::Pit
                            | RoomKind::Chasm
                            | RoomKind::Pool
                            | RoomKind::Stairs
                    ) as u8,
                    r.id,
                )
            })
            .map(|r| r.id)
    }
    /// Bounds of everything (x, z).
    pub fn bounds2(&self) -> Option<(Vec2, Vec2)> {
        let mut it = self.rooms.iter().map(|r| r.shape.bounds());
        let first = it.next()?;
        Some(it.fold(first, |a, b| (a.0.min(b.0), a.1.max(b.1))))
    }
}

// ------------------------------------------------------------------ compiled

/// A ramp along a stair's path: height `y0` at its start, `y1` at its end, flat landings.
#[derive(Clone, Debug)]
pub struct Ramp {
    pub line: Vec<Vec2>,
    pub len: f32,
    pub y0: f32,
    pub y1: f32,
}

impl Ramp {
    /// The floor height at `p` and the slope (rise per metre along the path).
    pub fn at(&self, p: Vec2) -> f32 {
        let (_, s) = geom::polyline_dist(&self.line, p);
        self.at_s(s)
    }
    pub fn at_s(&self, s: f32) -> f32 {
        let land = (self.len * 0.12).clamp(0.6, 2.0);
        let t = ((s - land) / (self.len - 2.0 * land).max(0.1)).clamp(0.0, 1.0);
        self.y0 + (self.y1 - self.y0) * t
    }
    pub fn landing(&self) -> f32 {
        (self.len * 0.12).clamp(0.6, 2.0)
    }
}

/// A room made ready to ask about.
#[derive(Clone, Debug)]
pub struct CRoom {
    pub id: u32,
    pub kind: RoomKind,
    pub level: i32,
    pub shape: Shape,
    /// polygon corners (rect, n-gon, outline): faster distance
    corners: Option<Vec<Vec2>>,
    line: Option<Vec<Vec2>>,
    half_w: f32,
    pub lo: Vec2,
    pub hi: Vec2,
    /// its level's floor height (m)
    pub level_y: f32,
    /// its floor (level + offset)
    pub base: f32,
    pub height: f32,
    pub ceiling: Ceiling,
    pub inr: f32,
    pub ramp: Option<Ramp>,
    pub depth: f32,
    pub liquid: Liquid,
    /// stairs drawn as a smooth ramp
    pub smooth: bool,
    pub style: Option<String>,
    pub role: Role,
    pub seed: u32,
    prio: u8,
}

impl CRoom {
    /// Inside distance (+ inside), with a cavern's ragged edge.
    #[inline]
    pub fn inside(&self, p: Vec2) -> f32 {
        let d = if let Some(c) = &self.corners {
            geom::poly_sdf(c, p)
        } else if let Some(l) = &self.line {
            self.half_w - geom::polyline_dist(l, p).0
        } else {
            self.shape.sdf(p)
        };
        if self.kind.is_rock() {
            d + cavern_edge(p, self.seed) * (self.inr * 0.35).clamp(0.4, 2.2)
        } else {
            d
        }
    }
    /// The floor's height at `p` (absolute).
    pub fn floor_at(&self, p: Vec2) -> f32 {
        match self.kind {
            RoomKind::Stairs => self.ramp.as_ref().map_or(self.base, |r| r.at(p)),
            RoomKind::Pit | RoomKind::Chasm => self.level_y - self.depth.max(1.0),
            RoomKind::Cavern => {
                let d = self.inside(p).max(0.0);
                // a lumpy floor that curls up into the walls
                self.base
                    + crate::noise::fbm(p.x * 0.07, p.y * 0.07, 3, 2.0, 0.5, self.seed ^ 0x51) * 0.6
                    + (1.4 - d).max(0.0).powi(2) * 0.35
            }
            _ => self.base,
        }
    }
    /// The ceiling's height at `p` (absolute). Pits and pools: `parent` is the ceiling of the
    /// space they open in.
    pub fn ceil_at(&self, p: Vec2) -> f32 {
        let floor = match self.kind {
            RoomKind::Stairs => self.floor_at(p),
            RoomKind::Pit | RoomKind::Chasm | RoomKind::Pool => self.level_y,
            _ => self.base,
        };
        let top = floor + self.height;
        match self.ceiling {
            Ceiling::Flat | Ceiling::Beams | Ceiling::Open => top,
            Ceiling::Rock => {
                let d = self.inside(p).max(0.0);
                let x = (d / self.inr.max(1.0)).min(1.0);
                floor
                    + self.height * (0.42 + 0.58 * (1.0 - (1.0 - x).powi(2)).sqrt())
                    + crate::noise::fbm(p.x * 0.09, p.y * 0.09, 3, 2.0, 0.5, self.seed ^ 0x77) * 1.1
            }
            c => {
                let spring = floor + vault_spring(self.height, self.inr, c);
                let rise = top - spring;
                let d = self.inside(p).max(0.0);
                let x = (d / self.inr.max(0.5)).min(1.0);
                spring + rise * vault_profile(c, x)
            }
        }
    }
    pub fn prio(&self) -> u8 {
        self.prio
    }
    pub fn corners(&self) -> Option<&[Vec2]> {
        self.corners.as_deref()
    }
}

/// Height of a vault's spring line above the floor: the walls are this tall.
pub fn vault_spring(height: f32, inr: f32, c: Ceiling) -> f32 {
    // the curve rises about as much as the room is wide (a half circle), at most half the height
    let rise = match c {
        Ceiling::Pointed => inr * 1.35,
        Ceiling::Stepped => inr * 0.8,
        _ => inr,
    };
    (height - rise).max(height * 0.45).max(2.6).min(height)
}

/// A vault's shape: 0 at the wall, 1 at the crown (`x`: 0 at the wall, 1 in the middle).
pub fn vault_profile(c: Ceiling, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    match c {
        Ceiling::Pointed => {
            // two arcs of radius 1.6 (in half-widths) meeting at the crown
            let r = 1.6f32;
            let u = 1.0 - x;
            let y = (r * r - (u + (r - 1.0)).powi(2)).max(0.0).sqrt();
            let top = (r * r - (r - 1.0).powi(2)).sqrt();
            y / top
        }
        Ceiling::Stepped => {
            ((1.0 - (1.0 - x).powi(2)).sqrt() * 4.0).floor().min(4.0) / 4.0 * 0.85 + x * 0.15
        }
        _ => (1.0 - (1.0 - x).powi(2)).max(0.0).sqrt(),
    }
}

/// A cavern's ragged edge: roughly −1..1.
fn cavern_edge(p: Vec2, seed: u32) -> f32 {
    crate::noise::fbm(p.x * 0.09, p.y * 0.09, 3, 2.1, 0.5, seed) * 0.9
        + crate::noise::perlin2(p.x * 0.35, p.y * 0.35, seed ^ 0x9e37) * 0.25
}

/// Something solid standing in the air: a pillar, a dais, a bridge's deck, a railing.
#[derive(Clone, Debug)]
pub enum Solid {
    /// a column: round or square (half width), from `y0` to `y1`
    Pillar {
        at: Vec2,
        r: f32,
        square: bool,
        y0: f32,
        y1: f32,
        room: u32,
    },
    /// a stepped platform: its outline, its floor, rise, step height and depth
    Dais { room: u32, base: f32, rise: f32 },
    /// a deck along a path: its top, thickness
    Deck { room: u32, top: f32, thick: f32 },
    /// a low wall: a segment, its top and bottom; the room it guards (its style)
    Rail {
        a: Vec2,
        b: Vec2,
        y0: f32,
        y1: f32,
        room: u32,
    },
    /// the solid under a stair's steps (where it climbs through another room's air)
    Stair { room: u32 },
    /// a spiral stair's steps round its column: centre, inner and outer radius, the bottom and
    /// top floors, each step's rise, and the angle its steps start from (the landing at the top
    /// faces the way out)
    Spiral {
        room: u32,
        c: Vec2,
        r_in: f32,
        r_out: f32,
        y0: f32,
        y1: f32,
        rise: f32,
        start: f32,
    },
}

/// Steps per turn of a spiral stair.
pub const SPIRAL_STEPS: usize = 16;

impl Solid {
    /// A spiral stair's steps: how many (the last few make the landing at the top) and the top of
    /// step `k` (0 at the bottom).
    pub fn spiral_steps(y0: f32, y1: f32, rise: f32) -> usize {
        ((y1 - y0) / rise).ceil().max(1.0) as usize + 3
    }
    pub fn spiral_top(y0: f32, y1: f32, rise: f32, k: usize) -> f32 {
        (y0 + (k + 1) as f32 * rise).min(y1)
    }
}

/// One stretch of air over a point: (bottom, top, room id).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub lo: f32,
    pub hi: f32,
    pub room: u32,
}

/// Dais steps: how high and how deep each is.
pub const STEP_H: f32 = 0.3;
pub const STEP_D: f32 = 0.6;

/// The plan, ready for questions. Cheap to share (`Arc`) with the mesher and physics.
#[derive(Clone, Debug, Default)]
pub struct Compiled {
    pub rooms: Vec<CRoom>,
    pub solids: Vec<Solid>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    sgrid: HashMap<(i32, i32), Vec<u32>>,
    by_id: HashMap<u32, usize>,
    /// everything (with a margin)
    pub lo: Vec3,
    pub hi: Vec3,
}

const GRID: f32 = 8.0;

fn cell_of(p: Vec2) -> (i32, i32) {
    ((p.x / GRID).floor() as i32, (p.y / GRID).floor() as i32)
}

impl Compiled {
    pub fn new(plan: &Plan, pillar: &dyn Fn(&Room) -> (f32, bool)) -> Compiled {
        let mut out = Compiled::default();
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for r in &plan.rooms {
            let level_y = plan.level_y(r.level);
            let (blo, bhi) = r.shape.bounds();
            let margin = if r.kind.is_rock() {
                (r.shape.inradius() * 0.35).clamp(0.4, 2.2) + 0.5
            } else {
                0.1
            };
            let (blo, bhi) = (blo - Vec2::splat(margin), bhi + Vec2::splat(margin));
            let line = r.shape.centreline();
            let ramp = if r.kind == RoomKind::Stairs {
                let l = line.clone().unwrap_or_else(|| {
                    let (ax, half, _) = r.shape.axes();
                    let c = r.shape.centre();
                    vec![c - ax * half, c + ax * half]
                });
                let len = geom::polyline_len(&l);
                let y1 = r
                    .to_level
                    .map(|t| plan.level_y(t))
                    .unwrap_or(level_y - LEVEL_STEP);
                Some(Ramp {
                    line: l,
                    len,
                    y0: level_y + r.floor,
                    y1,
                })
            } else {
                None
            };
            // a spiral stair's air: from the lower of its two floors to its ceiling over the upper
            let (spiral_lo, spiral_hi) = {
                let a = level_y + r.floor;
                let b = r
                    .to_level
                    .map(|t| plan.level_y(t))
                    .unwrap_or(a + LEVEL_STEP);
                (a.min(b), a.max(b))
            };
            let c = CRoom {
                id: r.id,
                kind: r.kind,
                level: r.level,
                shape: r.shape.clone(),
                corners: r.shape.corners(),
                half_w: match &r.shape {
                    Shape::Path { width, .. } => width * 0.5,
                    _ => 0.0,
                },
                line: match (&r.shape, r.kind) {
                    (Shape::Path { .. }, _) => line,
                    _ => None,
                },
                lo: blo,
                hi: bhi,
                level_y,
                base: if r.kind == RoomKind::Spiral {
                    spiral_lo
                } else {
                    level_y + r.floor
                },
                height: if r.kind == RoomKind::Spiral {
                    spiral_hi - spiral_lo + r.height.max(2.6)
                } else {
                    r.height.max(0.3)
                },
                ceiling: r.ceiling,
                inr: r.shape.inradius().max(0.5),
                ramp,
                depth: if r.depth > 0.0 { r.depth } else { LEVEL_STEP },
                liquid: r.liquid,
                smooth: r.smooth,
                style: r.style.clone(),
                role: r.role,
                seed: plan.seed ^ r.id.wrapping_mul(0x9e37_79b9),
                prio: r.kind.priority(),
            };
            let (ylo, yhi) = match r.kind {
                RoomKind::Stairs => {
                    let rp = c.ramp.as_ref().unwrap();
                    (rp.y0.min(rp.y1) - 0.5, rp.y0.max(rp.y1) + c.height + 1.0)
                }
                RoomKind::Pit | RoomKind::Chasm => {
                    (level_y - c.depth - 1.0, level_y + c.height + 1.0)
                }
                _ => (c.base - 1.0, c.base + c.height + 2.0),
            };
            lo = lo.min(Vec3::new(blo.x, ylo, blo.y));
            hi = hi.max(Vec3::new(bhi.x, yhi, bhi.y));
            let idx = out.rooms.len() as u32;
            out.by_id.insert(r.id, idx as usize);
            if r.kind.is_space() {
                for gx in cell_of(blo).0..=cell_of(bhi).0 {
                    for gz in cell_of(blo).1..=cell_of(bhi).1 {
                        out.grid.entry((gx, gz)).or_default().push(idx);
                    }
                }
            }
            out.rooms.push(c);
        }
        // solids: daises, decks, pillars, railings round pits
        for r in &plan.rooms {
            let ci = out.by_id[&r.id];
            let level_y = plan.level_y(r.level);
            match r.kind {
                RoomKind::Dais => out.solids.push(Solid::Dais {
                    room: r.id,
                    base: level_y,
                    rise: r.height.max(STEP_H),
                }),
                RoomKind::Bridge => out.solids.push(Solid::Deck {
                    room: r.id,
                    top: level_y + r.floor,
                    thick: 0.6,
                }),
                RoomKind::Balcony => out.solids.push(Solid::Deck {
                    room: r.id,
                    top: level_y + r.floor,
                    thick: 0.45,
                }),
                RoomKind::Stairs => out.solids.push(Solid::Stair { room: r.id }),
                RoomKind::Spiral => {
                    let room = &out.rooms[ci];
                    let c = r.shape.centre();
                    let r_out = (room.inr - 0.05).max(1.2);
                    let (y0, y1) = (room.base, room.base + room.height - r.height.max(2.6));
                    let n = Solid::spiral_steps(y0, y1, 0.22);
                    // the landing at the top faces the nearest room of the upper floor
                    let upper = out
                        .rooms
                        .iter()
                        .filter(|o| o.id != r.id && o.kind.is_space() && (o.base - y1).abs() < 1.0)
                        .map(|o| o.shape.centre() - c)
                        .filter(|d| d.length() > 0.5)
                        .min_by(|a, b| a.length().total_cmp(&b.length()));
                    let face = upper.map(|d| d.y.atan2(d.x)).unwrap_or(0.0);
                    let step = std::f32::consts::TAU / SPIRAL_STEPS as f32;
                    // (the landing's middle step at `face`)
                    let start = face - (n as f32 - 1.5) * step;
                    out.solids.push(Solid::Spiral {
                        room: r.id,
                        c,
                        r_in: 0.42,
                        r_out,
                        y0,
                        y1,
                        rise: (y1 - y0) / (n - 3) as f32,
                        start,
                    });
                    out.solids.push(Solid::Pillar {
                        at: c,
                        r: 0.42,
                        square: false,
                        y0: y0 - 0.2,
                        y1: room.base + room.height + 0.5,
                        room: r.id,
                    });
                }
                _ => {}
            }
            if r.pillars != Pillars::None && r.kind.is_space() {
                let (pr, square) = pillar(r);
                let room = &out.rooms[ci];
                let (y0, y1) = (room.base - 0.2, room.base + room.height + 0.5);
                for at in pillar_spots(r, pr) {
                    out.solids.push(Solid::Pillar {
                        at,
                        r: pr,
                        square,
                        y0,
                        y1,
                        room: r.id,
                    });
                }
            }
        }
        // railings along the open edges of pits (where the floor round them is walkable)
        let pits: Vec<usize> = (0..out.rooms.len())
            .filter(|i| out.rooms[*i].kind == RoomKind::Pit)
            .collect();
        for i in pits {
            let room = out.rooms[i].clone();
            let lp = geom::resample_loop(&room.shape.outline(), 1.0);
            let n = lp.len();
            for k in 0..n {
                let (a, b) = (lp[k], lp[(k + 1) % n]);
                let m = (a + b) * 0.5;
                let nrm = (b - a).perp().normalize_or_zero();
                let out_side = if room.inside(m + nrm * 0.1) > 0.0 {
                    -nrm
                } else {
                    nrm
                };
                let q = m + out_side * 0.4;
                // the floor just outside: a room of the same level whose floor is the level's
                let floor_out = out
                    .rooms
                    .iter()
                    .filter(|o| {
                        o.kind.is_space()
                            && !o.kind.is_pit()
                            && o.level == room.level
                            && o.inside(q) > 0.0
                    })
                    .map(|o| o.floor_at(q))
                    .fold(f32::MIN, f32::max);
                if floor_out > room.level_y - 0.3 && !out.bridge_at(m, floor_out) {
                    out.solids.push(Solid::Rail {
                        a,
                        b,
                        y0: floor_out - 0.05,
                        y1: floor_out + 1.05,
                        room: room.id,
                    });
                }
            }
        }
        // balconies: railings where their edge overlooks a drop (not against a wall, not where a
        // doorway or another stretch of gallery carries on), posts under them every few metres
        // down to the floor below and on up to the roof (an arcade)
        let decks: Vec<usize> = (0..out.rooms.len())
            .filter(|i| out.rooms[*i].kind == RoomKind::Balcony)
            .collect();
        // (the solids so far, findable: a stair arriving at a gallery is no drop)
        out.index_solids();
        let mut rails = vec![];
        for i in decks {
            let room = out.rooms[i].clone();
            let top = room.base;
            let lp = geom::resample_loop(&room.shape.outline(), 1.0);
            let n = lp.len();
            let mut since_post = 3.0f32;
            for k in 0..n {
                let (a, b) = (lp[k], lp[(k + 1) % n]);
                let m = (a + b) * 0.5;
                let nrm = (b - a).perp().normalize_or_zero();
                let out_side = if room.inside(m + nrm * 0.1) > 0.0 {
                    -nrm
                } else {
                    nrm
                };
                let q = m + out_side * 0.35;
                let drop = out
                    .floor_under(Vec3::new(q.x, top + 0.5, q.y), 80.0)
                    .is_none_or(|f| f < top - 1.2);
                let open = out.air(Vec3::new(q.x, top + 1.0, q.y)) && drop;
                if !open {
                    since_post = 3.0;
                    continue;
                }
                let (ia, ib) = (a - out_side * 0.14, b - out_side * 0.14);
                rails.push(Solid::Rail {
                    a: ia,
                    b: ib,
                    y0: top - 0.05,
                    y1: top + 1.05,
                    room: room.id,
                });
                since_post += a.distance(b);
                if since_post >= 4.5 {
                    since_post = 0.0;
                    let at = m - out_side * 0.14;
                    let owner = out
                        .rooms
                        .iter()
                        .filter(|o| {
                            o.kind.is_space()
                                && !o.kind.is_pit()
                                && o.inside(at) > 0.3
                                && o.floor_at(at) < top - 2.0
                                && o.base + o.height > top + 1.0
                        })
                        .max_by(|x, y| x.height.total_cmp(&y.height));
                    if let Some(o) = owner {
                        rails.push(Solid::Pillar {
                            at,
                            r: 0.26,
                            square: true,
                            y0: o.floor_at(at) - 0.2,
                            y1: o.ceil_at(at) + 0.5,
                            room: o.id,
                        });
                    }
                }
            }
        }
        out.solids.extend(rails);
        out.index_solids();
        if lo.x > hi.x {
            lo = Vec3::splat(-1.0);
            hi = Vec3::splat(1.0);
        }
        out.lo = lo - Vec3::splat(2.0);
        out.hi = hi + Vec3::splat(2.0);
        out
    }

    /// The grid that finds the solids near a spot (again, after solids were added).
    fn index_solids(&mut self) {
        self.sgrid.clear();
        for (si, s) in self.solids.iter().enumerate() {
            let (a, b) = self.solid_bounds(s);
            for gx in cell_of(a).0..=cell_of(b).0 {
                for gz in cell_of(a).1..=cell_of(b).1 {
                    self.sgrid.entry((gx, gz)).or_default().push(si as u32);
                }
            }
        }
    }

    fn bridge_at(&self, p: Vec2, y: f32) -> bool {
        self.rooms
            .iter()
            .any(|r| r.kind == RoomKind::Bridge && r.inside(p) > -0.6 && (r.base - y).abs() < 0.6)
    }

    pub fn room(&self, id: u32) -> Option<&CRoom> {
        self.by_id.get(&id).map(|i| &self.rooms[*i])
    }

    fn solid_bounds(&self, s: &Solid) -> (Vec2, Vec2) {
        match s {
            Solid::Pillar { at, r, .. } => (*at - Vec2::splat(*r), *at + Vec2::splat(*r)),
            Solid::Dais { room, .. } | Solid::Deck { room, .. } | Solid::Stair { room } => {
                self.room(*room).map(|r| (r.lo, r.hi)).unwrap_or_default()
            }
            Solid::Rail { a, b, .. } => {
                (a.min(*b) - Vec2::splat(0.2), a.max(*b) + Vec2::splat(0.2))
            }
            Solid::Spiral { c, r_out, .. } => (*c - Vec2::splat(*r_out), *c + Vec2::splat(*r_out)),
        }
    }

    /// The space rooms whose outline may hold `p`.
    pub fn rooms_near(&self, p: Vec2) -> impl Iterator<Item = &CRoom> {
        self.grid
            .get(&cell_of(p))
            .into_iter()
            .flatten()
            .map(|i| &self.rooms[*i as usize])
    }

    fn solids_near(&self, p: Vec2) -> impl Iterator<Item = &Solid> {
        self.sgrid
            .get(&cell_of(p))
            .into_iter()
            .flatten()
            .map(|i| &self.solids[*i as usize])
    }

    /// The air over `p` (unmerged): every space room holding `p`.
    pub fn spans(&self, p: Vec2, out: &mut Vec<Span>) {
        out.clear();
        for r in self.rooms_near(p) {
            if p.x < r.lo.x || p.y < r.lo.y || p.x > r.hi.x || p.y > r.hi.y || r.inside(p) <= 0.0 {
                continue;
            }
            let lo = r.floor_at(p);
            let hi = match r.kind {
                RoomKind::Pit | RoomKind::Chasm | RoomKind::Pool => self.parent_ceil(r, p),
                _ => r.ceil_at(p),
            };
            if hi > lo {
                out.push(Span { lo, hi, room: r.id });
            }
        }
    }

    /// The ceiling over a pit or pool: the tallest room of its level round it (else its own).
    pub fn parent_ceil(&self, r: &CRoom, p: Vec2) -> f32 {
        let own = r.ceil_at(p);
        self.rooms_near(p)
            .filter(|o| {
                o.id != r.id
                    && o.level == r.level
                    && !o.kind.is_pit()
                    && o.kind != RoomKind::Pool
                    && o.inside(p) > -0.5
            })
            .map(|o| o.ceil_at(p))
            .fold(own, f32::max)
    }

    /// The solid tops and bottoms over `p`: (bottom, top) of each solid holding `p`.
    pub fn solid_spans(&self, p: Vec2, out: &mut Vec<(f32, f32)>) {
        out.clear();
        for s in self.solids_near(p) {
            if let Solid::Spiral {
                c,
                r_in,
                r_out,
                y0,
                y1,
                rise,
                start,
                ..
            } = s
            {
                // a step on every turn over this spot
                let d = p.distance(*c);
                if d >= *r_in && d <= *r_out {
                    let step = std::f32::consts::TAU / SPIRAL_STEPS as f32;
                    let a =
                        ((p - *c).y.atan2((p - *c).x) - start).rem_euclid(std::f32::consts::TAU);
                    let n = Solid::spiral_steps(*y0, *y1, *rise);
                    let mut k = (a / step) as usize;
                    while k < n {
                        let top = Solid::spiral_top(*y0, *y1, *rise, k);
                        out.push((top - 0.3, top));
                        k += SPIRAL_STEPS;
                    }
                }
                continue;
            }
            if let Some(v) = self.solid_at(s, p) {
                out.push(v);
            }
        }
    }

    /// A solid's (bottom, top) over `p`, if it covers `p`.
    pub fn solid_at(&self, s: &Solid, p: Vec2) -> Option<(f32, f32)> {
        match s {
            Solid::Pillar {
                at,
                r,
                square,
                y0,
                y1,
                ..
            } => {
                let d = *at - p;
                let inside = if *square {
                    d.x.abs() <= *r && d.y.abs() <= *r
                } else {
                    d.length() <= *r
                };
                inside.then_some((*y0, *y1))
            }
            Solid::Dais { room, base, rise } => {
                let r = self.room(*room)?;
                let d = r.inside(p);
                (d > 0.0).then(|| (*base - 0.1, base + dais_top(d, *rise)))
            }
            Solid::Deck { room, top, thick } => {
                let r = self.room(*room)?;
                (r.inside(p) > 0.0).then_some((top - thick, *top))
            }
            Solid::Rail { a, b, y0, y1, .. } => {
                (geom::seg_dist2(p, *a, *b) <= 0.12 * 0.12).then_some((*y0, *y1))
            }
            Solid::Stair { room } => {
                let r = self.room(*room)?;
                let rp = r.ramp.as_ref()?;
                (r.inside(p) > 0.0).then(|| (rp.y0.min(rp.y1) - 0.3, r.floor_at(p)))
            }
            // (its steps stack: `solid_spans` lists them)
            Solid::Spiral { .. } => None,
        }
    }

    /// Is `p` in the open air of the dungeon?
    pub fn air(&self, p: Vec3) -> bool {
        let q = Vec2::new(p.x, p.z);
        let mut spans = Vec::new();
        self.spans(q, &mut spans);
        if !spans.iter().any(|s| p.y >= s.lo && p.y <= s.hi) {
            return false;
        }
        let mut solids = Vec::new();
        self.solid_spans(q, &mut solids);
        !solids.iter().any(|(a, b)| p.y >= *a && p.y <= *b)
    }

    /// Roughly the distance to the nearest wall, floor or roof: positive in the air, negative
    /// in the rock. (A bound good enough to march rays and push walkers out of walls.)
    pub fn field(&self, p: Vec3) -> f32 {
        let q = Vec2::new(p.x, p.z);
        // outside everything: the distance to the box
        let outside = (Vec3::new(self.lo.x, self.lo.y, self.lo.z) - p)
            .max(p - self.hi)
            .max(Vec3::ZERO);
        if outside.length_squared() > 0.0 {
            return -outside.length() - 0.5;
        }
        let mut d = -8.0f32;
        let mut any = false;
        for r in self.rooms_near(q) {
            if q.x < r.lo.x - 8.0 || q.y < r.lo.y - 8.0 || q.x > r.hi.x + 8.0 || q.y > r.hi.y + 8.0
            {
                continue;
            }
            any = true;
            let inside = r.inside(q);
            let (lo, hi) = if inside > -2.0 {
                (
                    r.floor_at(q),
                    if r.kind.is_pit() || r.kind == RoomKind::Pool {
                        self.parent_ceil(r, q)
                    } else {
                        r.ceil_at(q)
                    },
                )
            } else {
                (r.base, r.base + r.height)
            };
            let dv = (p.y - lo).min(hi - p.y);
            let di = if inside >= 0.0 && dv >= 0.0 {
                inside.min(dv)
            } else {
                -(inside.min(0.0).powi(2) + dv.min(0.0).powi(2)).sqrt()
            };
            d = d.max(di);
        }
        if !any {
            return -8.0;
        }
        // minus solids
        for s in self.solids_near(q) {
            let ds = match s {
                Solid::Pillar {
                    at,
                    r,
                    square,
                    y0,
                    y1,
                    ..
                } => {
                    let dh = if *square {
                        let v = (q - *at).abs() - Vec2::splat(*r);
                        v.max(Vec2::ZERO).length() + v.x.max(v.y).min(0.0)
                    } else {
                        q.distance(*at) - r
                    };
                    let dv = (y0 - p.y).max(p.y - y1);
                    if dh > 0.0 || dv > 0.0 {
                        (dh.max(0.0).powi(2) + dv.max(0.0).powi(2)).sqrt()
                    } else {
                        dh.max(dv)
                    }
                }
                Solid::Dais { room, base, rise } => match self.room(*room) {
                    Some(r) => {
                        let di = r.inside(q);
                        let top = base + dais_top(di.max(0.0), *rise);
                        let dv = (base - 0.1 - p.y).max(p.y - top);
                        if di < 0.0 || dv > 0.0 {
                            (di.min(0.0).powi(2) + dv.max(0.0).powi(2)).sqrt()
                        } else {
                            (-di).max(dv)
                        }
                    }
                    None => 100.0,
                },
                Solid::Deck { room, top, thick } => match self.room(*room) {
                    Some(r) => {
                        let di = r.inside(q);
                        let dv = (top - thick - p.y).max(p.y - top);
                        if di < 0.0 || dv > 0.0 {
                            (di.min(0.0).powi(2) + dv.max(0.0).powi(2)).sqrt()
                        } else {
                            (-di).max(dv)
                        }
                    }
                    None => 100.0,
                },
                Solid::Rail { a, b, y0, y1, .. } => {
                    let dh = geom::seg_dist2(q, *a, *b).sqrt() - 0.12;
                    let dv = (y0 - p.y).max(p.y - y1);
                    if dh > 0.0 || dv > 0.0 {
                        (dh.max(0.0).powi(2) + dv.max(0.0).powi(2)).sqrt()
                    } else {
                        dh.max(dv)
                    }
                }
                Solid::Stair { room } => match self
                    .room(*room)
                    .and_then(|r| r.ramp.as_ref().map(|rp| (r, rp)))
                {
                    Some((r, rp)) => {
                        let di = r.inside(q);
                        let (y0, y1) = (rp.y0.min(rp.y1) - 0.3, r.floor_at(q));
                        let dv = (y0 - p.y).max(p.y - y1);
                        if di < 0.0 || dv > 0.0 {
                            (di.min(0.0).powi(2) + dv.max(0.0).powi(2)).sqrt()
                        } else {
                            (-di).max(dv)
                        }
                    }
                    None => 100.0,
                },
                Solid::Spiral { c, r_in, r_out, .. } => {
                    // the nearest step over (or under) this spot
                    let dr = (r_in - q.distance(*c)).max(q.distance(*c) - r_out);
                    let mut spans = Vec::new();
                    self.solid_spans(q, &mut spans);
                    let dv = spans
                        .iter()
                        .map(|(a, b)| (a - p.y).max(p.y - b))
                        .fold(f32::MAX, f32::min);
                    if dv == f32::MAX {
                        100.0
                    } else if dr > 0.0 || dv > 0.0 {
                        (dr.max(0.0).powi(2) + dv.max(0.0).powi(2)).sqrt()
                    } else {
                        dr.max(dv)
                    }
                }
            };
            d = d.min(ds);
        }
        d
    }

    /// The floor under `p` (within `reach` below it) when `p` is in the air.
    pub fn floor_under(&self, p: Vec3, reach: f32) -> Option<f32> {
        let q = Vec2::new(p.x, p.z);
        let mut spans = Vec::new();
        self.spans(q, &mut spans);
        // the air holding p (overlapping rooms join)
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut found = false;
        loop {
            let mut grew = false;
            for s in &spans {
                let touches = if found {
                    s.hi >= lo - 0.01 && s.lo <= hi + 0.01
                } else {
                    p.y >= s.lo - 0.05 && p.y <= s.hi
                };
                if touches && (s.lo < lo || s.hi > hi) {
                    lo = lo.min(s.lo);
                    hi = hi.max(s.hi);
                    found = true;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        if !found {
            return None;
        }
        let mut floor = lo;
        let mut solids = Vec::new();
        self.solid_spans(q, &mut solids);
        for (a, b) in solids {
            if p.y >= a && p.y < b - 0.01 {
                // inside a solid: its top (a step up), if within a step
                if b - p.y < 0.5 {
                    floor = floor.max(b);
                } else {
                    return None;
                }
            } else if b <= p.y + 0.01 && b > floor {
                floor = b;
            }
        }
        (p.y - floor <= reach).then_some(floor)
    }

    /// The roof over `p` (within `reach` above it) when `p` is in the air.
    pub fn roof_over(&self, p: Vec3, reach: f32) -> Option<f32> {
        let q = Vec2::new(p.x, p.z);
        let mut spans = Vec::new();
        self.spans(q, &mut spans);
        let mut top = None::<f32>;
        for s in &spans {
            if p.y >= s.lo - 0.05 && p.y <= s.hi {
                top = Some(top.map_or(s.hi, |t: f32| t.max(s.hi)));
            }
        }
        let mut top = top?;
        let mut solids = Vec::new();
        self.solid_spans(q, &mut solids);
        for (a, _) in solids {
            if a > p.y && a < top {
                top = a;
            }
        }
        (top - p.y <= reach).then_some(top)
    }
}

/// A dais's top at inside distance `d`: steps of `STEP_H` every `STEP_D` until the full rise.
pub fn dais_top(d: f32, rise: f32) -> f32 {
    ((d / STEP_D).floor() * STEP_H + STEP_H).min(rise)
}

/// Where a room's pillars stand (centres), for pillars of half width `pr`.
pub fn pillar_spots(r: &Room, pr: f32) -> Vec<Vec2> {
    let shape = &r.shape;
    let c = shape.centre();
    let (ax, half_l, half_w) = shape.axes();
    let across = ax.perp();
    let mut out = Vec::new();
    match r.pillars {
        Pillars::None => {}
        Pillars::Rows => {
            if half_w < pr * 2.0 + 2.5 {
                return out;
            }
            let off = (half_w * 0.55).max(pr + 1.5).min(half_w - pr - 1.5);
            let usable = half_l - (pr + 2.5);
            let n = ((usable * 2.0 / 7.0).floor() as i32).max(1);
            for k in 0..=n {
                let t = if n == 0 {
                    0.0
                } else {
                    -usable + 2.0 * usable * k as f32 / n as f32
                };
                for s in [-1.0, 1.0] {
                    out.push(c + ax * t + across * off * s);
                }
            }
        }
        Pillars::Ring => {
            let ring = shape.inradius() * 0.62;
            if ring < pr * 2.0 + 1.5 {
                return out;
            }
            let n = ((std::f32::consts::TAU * ring / 7.5).round() as usize).clamp(4, 24);
            let n = if n % 2 == 1 { n + 1 } else { n };
            for k in 0..n {
                out.push(
                    c + Vec2::from_angle(std::f32::consts::TAU * (k as f32 + 0.5) / n as f32)
                        * ring,
                );
            }
        }
        Pillars::Grid => {
            let step = 8.0;
            let (lo, hi) = shape.bounds();
            let nx = ((hi.x - lo.x) / step).floor() as i32;
            let nz = ((hi.y - lo.y) / step).floor() as i32;
            let o = (lo + hi) * 0.5 - Vec2::new(nx as f32, nz as f32) * step * 0.5;
            for i in 0..=nx {
                for j in 0..=nz {
                    let p = o + Vec2::new(i as f32, j as f32) * step;
                    if shape.sdf(p) > pr + 2.5 {
                        out.push(p);
                    }
                }
            }
        }
    }
    out.retain(|p| shape.sdf(*p) > pr + 0.8);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple() -> Plan {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(6.0, 5.0)),
        ));
        plan.add(Room::new(
            0,
            RoomKind::Corridor,
            0,
            Shape::path(&[Vec2::new(4.0, 0.0), Vec2::new(20.0, 0.0)], 3.0),
        ));
        plan
    }

    fn compile(p: &Plan) -> Compiled {
        Compiled::new(p, &|_| (0.6, false))
    }

    #[test]
    fn a_gallery_hall_has_railings_over_the_drop_and_a_stair_up_to_it() {
        let mut plan = Plan::default();
        let hall = crate::dungeons::presets::gallery_hall(
            &mut plan,
            0,
            Vec2::ZERO,
            24.0,
            30.0,
            Ceiling::Vault,
        );
        let top = plan.level_y(1);
        let c = compile(&plan);
        // standing on the gallery along a long side, and the drop beside it
        assert_eq!(
            c.floor_under(Vec3::new(-11.0, top + 1.0, 0.0), 2.0),
            Some(top),
            "on the gallery"
        );
        let below = c
            .floor_under(Vec3::new(-5.0, top + 1.0, 0.0), 40.0)
            .expect("the hall's floor");
        assert!(below < 0.5, "a drop beside the gallery: {below}");
        // railings along the gallery's open edges, none against the walls
        let rails: Vec<_> = c
            .solids
            .iter()
            .filter_map(|s| match s {
                Solid::Rail { a, b, y0, room, .. } if (*y0 - top).abs() < 0.2 => {
                    Some((*a, *b, *room))
                }
                _ => None,
            })
            .collect();
        assert!(rails.len() > 10, "{} railings", rails.len());
        assert!(
            rails.iter().all(|(a, b, _)| a.x.abs() < 11.9
                && b.x.abs() < 11.9
                && a.y.abs() < 14.9
                && b.y.abs() < 14.9),
            "no railing against the hall's walls"
        );
        // the arcade: posts from the hall's floor
        assert!(
            c.solids
                .iter()
                .any(|s| matches!(s, Solid::Pillar { room, y0, .. } if *room == hall && *y0 < 0.0)),
            "posts under the gallery"
        );
        // the stair climbs from the floor to the gallery's height at its far end
        let st = plan
            .rooms
            .iter()
            .find(|r| r.kind == RoomKind::Stairs)
            .expect("a stair");
        let line = st.shape.centreline().unwrap();
        let (a, b) = (line[0], line[line.len() - 1]);
        assert!(
            c.floor_under(Vec3::new(a.x, 1.0, a.y), 3.0)
                .is_some_and(|f| f < 0.5)
        );
        assert!(
            c.floor_under(Vec3::new(b.x, top + 1.0, b.y), 3.0)
                .is_some_and(|f| (f - top).abs() < 0.4),
            "the stair's top is the gallery's floor"
        );
    }

    #[test]
    fn a_spiral_stair_winds_up_a_step_at_a_time_to_a_landing_facing_the_room_above() {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Spiral,
            0,
            Shape::Round {
                c: [0.0, 0.0],
                r: 3.5,
            },
        ));
        plan.ensure_level(1);
        // the room above, to the east
        plan.add(Room::new(
            0,
            RoomKind::Room,
            1,
            Shape::rect(Vec2::new(9.0, 0.0), Vec2::new(5.0, 4.0)),
        ));
        let top = plan.level_y(1);
        let c = compile(&plan);
        // round the ring: a step on every turn, each turn a turn's rise above the last
        let mut spans = Vec::new();
        c.solid_spans(Vec2::new(-2.0, 0.5), &mut spans);
        assert!(spans.len() >= 3, "{spans:?}");
        // walking round it: never more than a step up
        let mut prev: Option<f32> = None;
        let mut y = 0.3;
        for i in 0..200 {
            let a = i as f32 * 0.05;
            let p = Vec2::from_angle(a) * 2.0;
            let Some(f) = c.floor_under(Vec3::new(p.x, y + 0.45, p.y), 1.0) else {
                continue;
            };
            if let Some(q) = prev {
                assert!(f - q < 0.46, "a step too high at {a}: {q} -> {f}");
            }
            prev = Some(f);
            y = f;
        }
        // the landing at the top faces the room above
        assert!(
            c.floor_under(Vec3::new(2.5, top + 0.5, 0.0), 1.0)
                .is_some_and(|f| (f - top).abs() < 0.05),
            "a landing to the east at the upper floor"
        );
    }

    #[test]
    fn a_chasm_drops_away_and_a_bridge_crosses_it() {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Cavern,
            0,
            Shape::path(&[Vec2::new(-30.0, 0.0), Vec2::new(30.0, 0.0)], 24.0),
        ));
        let mut ch = Room::new(
            0,
            RoomKind::Chasm,
            0,
            Shape::path(&[Vec2::new(-30.0, 0.0), Vec2::new(30.0, 0.0)], 8.0),
        );
        ch.depth = 40.0;
        plan.add(ch);
        plan.add(Room::new(
            0,
            RoomKind::Bridge,
            0,
            Shape::path(&[Vec2::new(0.0, -7.0), Vec2::new(0.0, 7.0)], 3.0),
        ));
        let c = compile(&plan);
        assert!(
            c.floor_under(Vec3::new(10.0, 1.0, 0.0), 20.0).is_none(),
            "no floor within 20 m over the chasm"
        );
        assert!(
            c.floor_under(Vec3::new(10.0, 1.0, 0.0), 60.0)
                .is_some_and(|f| f < -35.0),
            "its bottom"
        );
        assert!(
            c.floor_under(Vec3::new(0.0, 1.0, 0.0), 2.0)
                .is_some_and(|f| f.abs() < 0.2),
            "on the bridge"
        );
        // no railings round a chasm
        assert!(!c.solids.iter().any(|s| matches!(s, Solid::Rail { .. })));
    }

    #[test]
    fn a_window_opens_through_the_wall_to_the_next_room() {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::new(-5.0, 0.0), Vec2::new(4.0, 4.0)),
        ));
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::new(5.5, 0.0), Vec2::new(4.0, 4.0)),
        ));
        // the wall between them: x from -1 to 1.5
        let w = crate::dungeons::presets::wall_opening(&plan, 0, Vec2::new(-1.2, 0.0), true)
            .expect("a window");
        plan.add(w);
        let c = compile(&plan);
        assert!(
            c.air(Vec3::new(0.2, 2.0, 0.0)),
            "the window's air is in the wall"
        );
        assert!(
            !c.air(Vec3::new(0.2, 0.5, 0.0)),
            "below its sill is still wall"
        );
        assert!(!c.air(Vec3::new(0.2, 2.0, 2.5)), "beside it is still wall");
        let a = crate::dungeons::presets::wall_opening(&plan, 0, Vec2::new(-5.0, 3.8), false)
            .expect("an alcove");
        assert_eq!(a.name, "Alcove");
    }

    #[test]
    fn rooms_and_corridors_join() {
        let c = compile(&simple());
        assert!(c.air(Vec3::new(0.0, 1.0, 0.0)));
        assert!(c.air(Vec3::new(10.0, 1.0, 0.0)), "in the corridor");
        assert!(
            !c.air(Vec3::new(10.0, 1.0, 4.0)),
            "beside the corridor is rock"
        );
        assert!(!c.air(Vec3::new(0.0, -0.5, 0.0)), "under the floor");
        assert_eq!(c.floor_under(Vec3::new(10.0, 1.5, 0.0), 3.0), Some(0.0));
        let roof = c.roof_over(Vec3::new(0.0, 1.0, 0.0), 20.0).unwrap();
        assert!((roof - 6.5).abs() < 0.01, "{roof}");
        // the field agrees with the air test
        for p in [
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(10.0, 1.0, 4.0),
            Vec3::new(10.0, 1.0, 0.0),
            Vec3::new(30.0, 1.0, 0.0),
        ] {
            assert_eq!(c.field(p) > 0.0, c.air(p), "{p}");
        }
    }

    #[test]
    fn stairs_ramp_between_levels() {
        let mut plan = simple();
        plan.ensure_level(-1);
        let mut st = Room::new(
            0,
            RoomKind::Stairs,
            0,
            Shape::path(&[Vec2::new(20.0, 0.0), Vec2::new(20.0, 30.0)], 3.0),
        );
        st.to_level = Some(-1);
        plan.add(st);
        plan.add(Room::new(
            0,
            RoomKind::Room,
            -1,
            Shape::rect(Vec2::new(20.0, 36.0), Vec2::new(6.0, 6.0)),
        ));
        let c = compile(&plan);
        let y_mid = c
            .floor_under(Vec3::new(20.0, -5.0, 15.0), 20.0)
            .expect("in the stairwell");
        assert!(y_mid < -3.0 && y_mid > -11.0, "half way down: {y_mid}");
        let bottom = c
            .floor_under(Vec3::new(20.0, -LEVEL_STEP + 1.0, 36.0), 3.0)
            .unwrap();
        assert!((bottom + LEVEL_STEP).abs() < 0.01);
    }

    #[test]
    fn pillars_and_daises_are_solid() {
        let mut plan = Plan::default();
        let mut hall = Room::new(
            0,
            RoomKind::Hall,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(20.0, 10.0)),
        );
        hall.pillars = Pillars::Rows;
        plan.add(hall);
        plan.add(Room::new(
            0,
            RoomKind::Dais,
            0,
            Shape::rect(Vec2::new(15.0, 0.0), Vec2::new(3.0, 3.0)),
        ));
        let c = compile(&plan);
        let pillars: Vec<_> = c
            .solids
            .iter()
            .filter_map(|s| {
                if let Solid::Pillar { at, .. } = s {
                    Some(*at)
                } else {
                    None
                }
            })
            .collect();
        assert!(pillars.len() >= 4);
        assert!(!c.air(Vec3::new(pillars[0].x, 2.0, pillars[0].y)));
        // standing on the dais's top step
        let f = c.floor_under(Vec3::new(15.0, 3.0, 0.0), 5.0).unwrap();
        assert!((f - 0.9).abs() < 0.01, "{f}");
    }

    #[test]
    fn a_pit_opens_down() {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(10.0, 10.0)),
        ));
        plan.add(Room::new(
            0,
            RoomKind::Pit,
            0,
            Shape::Round {
                c: [0.0, 0.0],
                r: 3.0,
            },
        ));
        let c = compile(&plan);
        assert!(c.air(Vec3::new(0.0, -3.0, 0.0)));
        let bottom = c.floor_under(Vec3::new(0.0, 1.0, 0.0), 30.0).unwrap();
        assert!(bottom < -10.0);
        assert!(
            c.solids.iter().any(|s| matches!(s, Solid::Rail { .. })),
            "railings round it"
        );
    }
}
