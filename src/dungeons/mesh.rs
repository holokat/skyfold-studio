//! Builds the dungeon's architecture from the compiled plan: walls (with a lower band, a
//! skirting, a cornice, pilasters and frames round doorways), floors, ceilings (flat, beamed,
//! vaulted, pointed, stepped, rock), pillars, daises, bridges, railings, stairs and the surface of
//! pools. Everything derives from the same air the walker moves in (`plan::Compiled`): a wall
//! is wherever a room's air meets no other air, so doorways, headers over lower passages, steps,
//! shafts and stairwells all come out of the same rule.
//!
//! Pure data (positions, normals, UVs, colours, indices) grouped by level and material, made off
//! the main thread (`render`).

use bevy::math::{Vec2, Vec3};
use std::collections::HashMap;

use super::geom::{self, resample_loop};
use super::plan::{
    CRoom, Ceiling, Compiled, Liquid, RoomKind, STEP_D, STEP_H, Solid, Span, dais_top,
};
use super::styles::{self, PillarShape, Slot, Style};

/// Which pass a mesh belongs to: the cut-away view hides ceilings and shows wall caps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    Main,
    Ceiling,
    Cap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartKey {
    pub level: i32,
    pub layer: Layer,
    pub style: u16,
    pub slot: Slot,
}

#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub col: Vec<[f32; 4]>,
    pub idx: Vec<u32>,
    /// weathering (`weather`): the floor height under each vertex, how wet it is
    pub aux: Vec<[f32; 2]>,
}

impl MeshData {
    fn base(&self) -> u32 {
        self.pos.len() as u32
    }
    fn v(&mut self, p: Vec3, n: Vec3, uv: Vec2, ao: f32) {
        self.pos.push(p.to_array());
        self.nrm.push(n.to_array());
        self.uv.push(uv.to_array());
        self.col.push([ao, ao, ao, 1.0]);
    }
    /// A quad a-b-c-d (counter-clockwise seen from the side `n` faces).
    #[allow(clippy::too_many_arguments)]
    fn quad(&mut self, p: [Vec3; 4], n: Vec3, uv: [Vec2; 4], ao: [f32; 4]) {
        let b = self.base();
        for k in 0..4 {
            self.v(p[k], n, uv[k], ao[k]);
        }
        // wind so the face looks along n
        let face = (p[1] - p[0]).cross(p[2] - p[0]);
        if face.dot(n) >= 0.0 {
            self.idx
                .extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        } else {
            self.idx
                .extend_from_slice(&[b, b + 2, b + 1, b, b + 3, b + 2]);
        }
    }
    fn tri(&mut self, p: [Vec3; 3], n: [Vec3; 3], uv: [Vec2; 3], ao: [f32; 3], face_n: Vec3) {
        let b = self.base();
        for k in 0..3 {
            self.v(p[k], n[k], uv[k], ao[k]);
        }
        let face = (p[1] - p[0]).cross(p[2] - p[0]);
        if face.dot(face_n) >= 0.0 {
            self.idx.extend_from_slice(&[b, b + 1, b + 2]);
        } else {
            self.idx.extend_from_slice(&[b, b + 2, b + 1]);
        }
    }
    /// A quad (counter-clockwise seen from the side `n` faces), unshaded.
    pub fn push_quad(&mut self, p: [Vec3; 4], n: Vec3, uv: [Vec2; 4]) {
        self.quad(p, n, uv, [1.0; 4]);
    }
    pub fn is_empty(&self) -> bool {
        self.idx.is_empty()
    }
    pub fn append(&mut self, o: MeshData) {
        let b = self.base();
        self.pos.extend(o.pos);
        self.nrm.extend(o.nrm);
        self.uv.extend(o.uv);
        self.col.extend(o.col);
        // (weathering is worked out once the whole part is built)
        if self.aux.len() + o.aux.len() == self.pos.len() {
            self.aux.extend(o.aux);
        } else {
            self.aux.clear();
        }
        self.idx.extend(o.idx.into_iter().map(|i| i + b));
    }
    pub fn triangles(&self) -> usize {
        self.idx.len() / 3
    }
}

/// A liquid surface (pools, pit bottoms).
#[derive(Clone, Debug)]
pub struct LiquidMesh {
    pub level: i32,
    pub liquid: Liquid,
    pub mesh: MeshData,
}

/// A doorway found in a wall (for frames, and for the builder's doors and markers).
#[derive(Clone, Debug)]
pub struct Doorway {
    pub room: u32,
    pub a: Vec2,
    pub b: Vec2,
    pub floor: f32,
    pub top: f32,
    /// pointing into the room
    pub inward: Vec2,
}

#[derive(Default)]
pub struct Built {
    pub parts: HashMap<PartKey, MeshData>,
    pub liquids: Vec<LiquidMesh>,
    pub doorways: Vec<Doorway>,
}

impl Built {
    fn part(&mut self, level: i32, layer: Layer, style: usize, slot: Slot) -> &mut MeshData {
        self.parts
            .entry(PartKey {
                level,
                layer,
                style: style as u16,
                slot,
            })
            .or_default()
    }
    pub fn triangles(&self) -> usize {
        self.parts.values().map(|m| m.triangles()).sum::<usize>()
            + self
                .liquids
                .iter()
                .map(|l| l.mesh.triangles())
                .sum::<usize>()
    }
    fn merge(&mut self, o: Built) {
        for (k, m) in o.parts {
            self.parts.entry(k).or_default().append(m);
        }
        self.liquids.extend(o.liquids);
        self.doorways.extend(o.doorways);
    }
}

/// Where a wall piece's bottom or top comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Src {
    /// this room's floor / ceiling
    Own,
    /// another room's ceiling (bottom of a header) or floor (top of a riser)
    Other(u32),
}

/// How a style is picked for a room.
pub type StyleOf<'a> = &'a (dyn Fn(&CRoom) -> usize + Sync);

/// Builds everything. `style_of` gives each room's style index (`styles::all`).
pub fn build(c: &Compiled, style_of: StyleOf) -> Built {
    build_with(c, style_of, 1.0)
}

/// `build` with the map's rubble (a multiplier on each style's own).
pub fn build_with(c: &Compiled, style_of: StyleOf, rubble_amount: f32) -> Built {
    use rayon::prelude::*;
    let rooms: Vec<&CRoom> = c.rooms.iter().collect();
    let mut out = rooms
        .par_iter()
        .map(|r| {
            let mut b = Built::default();
            let si = style_of(r);
            let st = &styles::all()[si];
            if r.kind.is_space() {
                rubble(&mut b, c, r, si, st, rubble_amount);
                walls(&mut b, c, r, si, st);
                if r.kind == RoomKind::Stairs {
                    stairs(&mut b, c, r, si);
                } else {
                    floor(&mut b, c, r, si);
                }
                if !r.kind.is_pit() && r.kind != RoomKind::Pool && r.ceiling != Ceiling::Open {
                    ceiling(&mut b, c, r, si, st);
                }
                liquid(&mut b, r);
            }
            b
        })
        .reduce(Built::default, |mut a, b| {
            a.merge(b);
            a
        });
    // solids
    for s in &c.solids {
        match s {
            Solid::Pillar {
                at,
                r,
                square,
                y0,
                room,
                ..
            } => {
                let Some(owner) = c.room(*room) else { continue };
                let si = style_of(owner);
                let st = &styles::all()[si];
                let top = c
                    .rooms_near(*at)
                    .filter(|o| o.inside(*at) > 0.0)
                    .map(|o| o.ceil_at(*at))
                    .fold(owner.ceil_at(*at), f32::max);
                let shape = if *square {
                    PillarShape::Square
                } else {
                    st.pillar_shape
                };
                // up to the walls' top (a capital there); the shaft on into a vault is part of
                // the ceiling (the cut-away hides it with the vault)
                let wall_top = if owner.ceiling.curved() {
                    owner.base + super::plan::vault_spring(owner.height, owner.inr, owner.ceiling)
                } else {
                    top
                };
                let split = wall_top.min(top);
                let tile = st.mat(Slot::Pillar).tile;
                pillar(
                    out.part(owner.level, Layer::Main, si, Slot::Pillar),
                    *at,
                    *r,
                    *y0 + 0.2,
                    split,
                    shape,
                    tile,
                );
                if top > split + 0.3 {
                    pillar_shaft(
                        out.part(owner.level, Layer::Ceiling, si, Slot::Pillar),
                        *at,
                        *r * 0.85,
                        split,
                        top,
                        shape,
                        tile,
                    );
                }
            }
            Solid::Dais { room, base, rise } => {
                if let Some(r) = c.room(*room) {
                    let si = style_of(r);
                    dais(&mut out, r, si, *base, *rise);
                }
            }
            Solid::Deck { room, top, thick } => {
                if let Some(r) = c.room(*room) {
                    let si = style_of(r);
                    // (a balcony's railings are solids of their own: only where it overlooks a drop)
                    deck(&mut out, r, si, *top, *thick, r.kind == RoomKind::Bridge);
                }
            }
            Solid::Rail { a, b, y0, y1, room } => {
                let (level, si) = c
                    .room(*room)
                    .map(|o| (o.level, style_of(o)))
                    .unwrap_or((0, 0));
                rail(
                    out.part(level, Layer::Main, si, Slot::Trim),
                    *a,
                    *b,
                    *y0,
                    *y1,
                );
            }
            Solid::Stair { .. } => {}
            Solid::Spiral {
                room,
                c: centre,
                r_in,
                r_out,
                y0,
                y1,
                rise,
                start,
            } => {
                if let Some(r) = c.room(*room) {
                    let si = style_of(r);
                    spiral(
                        &mut out, r.level, si, *centre, *r_in, *r_out, *y0, *y1, *rise, *start,
                    );
                }
            }
        }
    }
    weather(&mut out, c);
    out
}

/// Broken stones at the foot of a room's walls: chunks of its own masonry, in little heaps that
/// gather in corners and round the feet of pillars, more in old and ruined styles.
fn rubble(out: &mut Built, c: &Compiled, r: &CRoom, si: usize, st: &Style, amount: f32) {
    if r.kind.is_pit() || matches!(r.kind, RoomKind::Pool | RoomKind::Stairs | RoomKind::Spiral) {
        return;
    }
    let k = (st.rubble() * amount).clamp(0.0, 3.0);
    if k < 0.02 {
        return;
    }
    let slot = if r.kind.is_rock() {
        Slot::Rock
    } else {
        Slot::Wall
    };
    let tile = st.mat(slot).tile;
    let mut rng = crate::noise::Rng::new(r.seed as u64 ^ 0x5ab1e);
    let mut m = MeshData::default();
    let lp = resample_loop(&r.shape.outline(), 0.7);
    let n = lp.len();
    for i in 0..n {
        let (a, b) = (lp[i], lp[(i + 1) % n]);
        let mid = (a + b) * 0.5;
        let nrm = (b - a).perp().normalize_or_zero();
        let inward = if r.inside(mid + nrm * 0.2) > 0.0 {
            nrm
        } else {
            -nrm
        };
        // only against a wall (not in a doorway): rock just outside, floor just inside
        let foot = mid + inward * 0.25;
        let floor = r.floor_at(foot);
        if c.air(Vec3::new(
            mid.x - inward.x * 0.3,
            floor + 0.6,
            mid.y - inward.y * 0.3,
        )) {
            continue;
        }
        // corners (the outline turns) and dark spots gather more
        let turn = {
            let (p, q) = (lp[(i + n - 1) % n], lp[(i + 2) % n]);
            1.0 - (a - p)
                .normalize_or_zero()
                .dot((q - b).normalize_or_zero())
                .clamp(-1.0, 1.0)
        };
        let chance = (0.05 + turn * 0.6) * k;
        if rng.f32() > chance {
            continue;
        }
        let at = foot + inward * rng.range(0.0, 0.25);
        heap(&mut m, &mut rng, at, floor, inward, 0.6 + turn, tile);
    }
    // round the feet of pillars
    for s in c.solids.iter() {
        if let Solid::Pillar {
            at, r: pr, room, ..
        } = s
            && *room == r.id
            && rng.f32() < 0.35 * k
        {
            let dir = Vec2::from_angle(rng.range(0.0, std::f32::consts::TAU));
            heap(
                &mut m,
                &mut rng,
                *at + dir * (pr + 0.25),
                r.floor_at(*at + dir * (pr + 0.25)),
                dir,
                0.6,
                tile,
            );
        }
    }
    if !m.is_empty() {
        out.part(r.level, Layer::Main, si, slot).append(m);
    }
}

/// A little heap of broken stones on the floor at `at`, spreading `out` from the wall.
fn heap(
    m: &mut MeshData,
    rng: &mut crate::noise::Rng,
    at: Vec2,
    floor: f32,
    out: Vec2,
    size: f32,
    tile: f32,
) {
    let count = 2 + (rng.f32() * 5.0 * size) as usize;
    for _ in 0..count {
        let s = rng.range(0.07, 0.24) * (0.7 + size * 0.5);
        let off = out * rng.range(0.0, 0.5 * size) + out.perp() * rng.range(-0.45, 0.45) * size;
        let c = Vec3::new(at.x + off.x, floor + s * rng.range(0.25, 0.6), at.y + off.y);
        let half = Vec3::new(
            s * rng.range(0.8, 1.5),
            s * rng.range(0.5, 0.9),
            s * rng.range(0.7, 1.3),
        );
        let yaw = rng.range(0.0, std::f32::consts::TAU);
        chunk(m, rng, c, half, yaw, tile);
    }
}

/// One broken stone: a box with chipped, jittered corners, turned by `yaw`, its faces textured in
/// world space.
fn chunk(m: &mut MeshData, rng: &mut crate::noise::Rng, c: Vec3, half: Vec3, yaw: f32, tile: f32) {
    let q = bevy::math::Quat::from_rotation_y(yaw)
        * bevy::math::Quat::from_rotation_x(rng.range(-0.25, 0.25));
    let mut corner = [Vec3::ZERO; 8];
    for (k, v) in corner.iter_mut().enumerate() {
        let sx = if k & 1 == 0 { -1.0 } else { 1.0 };
        let sy = if k & 2 == 0 { -1.0 } else { 1.0 };
        let sz = if k & 4 == 0 { -1.0 } else { 1.0 };
        let j = Vec3::new(
            rng.range(0.7, 1.15),
            rng.range(0.7, 1.15),
            rng.range(0.7, 1.15),
        );
        *v = c + q * (Vec3::new(sx, sy, sz) * half * j);
    }
    // faces: -x, +x, -y, +y, -z, +z (corner indices)
    const FACES: [[usize; 4]; 6] = [
        [0, 4, 6, 2],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 2, 3, 1],
        [4, 5, 7, 6],
    ];
    for f in FACES {
        let p = [corner[f[0]], corner[f[1]], corner[f[2]], corner[f[3]]];
        let n = (p[1] - p[0]).cross(p[3] - p[0]).normalize_or(Vec3::Y);
        let uv = |v: Vec3| {
            if n.y.abs() > 0.6 {
                Vec2::new(v.x, v.z) / tile
            } else {
                Vec2::new(v.x + v.z, -v.y) / tile
            }
        };
        let ao = if n.y < -0.5 { 0.45 } else { 0.8 };
        m.quad(p, n, [uv(p[0]), uv(p[1]), uv(p[2]), uv(p[3])], [ao; 4]);
    }
}

/// What the surfaces' weathering needs per vertex (`shaders/dungeon_surface.wesl`): the floor
/// under it (grime gathers at the foot of walls), how wet it is (near pools and water-filled pits)
/// and how mossy it may get (in the colour's alpha: dark corners, damp spots, upward faces).
fn weather(out: &mut Built, c: &Compiled) {
    use rayon::prelude::*;
    let wet_rooms: Vec<&CRoom> = c
        .rooms
        .iter()
        .filter(|r| {
            matches!(r.liquid, Liquid::Water | Liquid::Slime)
                && (r.kind == RoomKind::Pool || r.kind.is_pit())
        })
        .collect();
    let wet_at = |p: Vec3| -> f32 {
        let q = Vec2::new(p.x, p.z);
        wet_rooms
            .iter()
            .filter(|r| (p.y - r.level_y).abs() < 6.0)
            .map(|r| 1.0 - (-r.inside(q)).max(0.0).min(4.0) / 4.0)
            .fold(0.0, f32::max)
    };
    out.parts.par_iter_mut().for_each(|(_, m)| {
        m.aux = (0..m.pos.len())
            .map(|i| {
                let p = Vec3::from_array(m.pos[i]);
                let n = Vec3::from_array(m.nrm[i]);
                let floor = c
                    .floor_under(p + n * 0.08 + Vec3::Y * 0.3, 14.0)
                    .unwrap_or(p.y - 10.0);
                [floor.min(p.y), wet_at(p)]
            })
            .collect();
        for i in 0..m.col.len() {
            let ao = m.col[i][0];
            let up = m.nrm[i][1].max(0.0);
            m.col[i][3] = ((1.0 - ao) * 1.4 + up * 0.25 + m.aux[i][1] * 0.6 + 0.2).clamp(0.0, 1.0);
        }
    });
}

// ------------------------------------------------------------------ walls

/// How far inside and outside a wall point its own air and the other rooms' air are sampled.
const WALL_IN: f32 = 0.03;
const WALL_OUT: f32 = 0.08;

/// The outline's points with more wherever the wall's bands change between two (a doorway's
/// edge, or where a header over a lower roof starts or runs out, found to a few millimetres):
/// walls then end exactly at their openings instead of up to a sample short (a sliver of sky or
/// darkness beside the doorway, or a missing corner of header over an arch).
fn refine(c: &Compiled, r: &CRoom, pts: &[Vec2]) -> Vec<Vec2> {
    let n = pts.len();
    let sig = |p: Vec2, o: Vec2| -> Vec<(Src, Src)> {
        bands_at(c, r, p - o * WALL_IN, p + o * WALL_OUT)
            .into_iter()
            .map(|b| (b.2, b.3))
            .collect()
    };
    let mut out = Vec::with_capacity(n + 16);
    for k in 0..n {
        let (a, b) = (pts[k], pts[(k + 1) % n]);
        out.push(a);
        let m = (a + b) * 0.5;
        let cand = (b - a).perp().normalize_or_zero();
        let o = if r.inside(m + cand * 0.05) < r.inside(m - cand * 0.05) {
            cand
        } else {
            -cand
        };
        let end = sig(a.lerp(b, 0.98), o);
        // (a few changes in one piece: a header can run out right beside a doorway's edge)
        let mut lo0 = 0.02f32;
        let mut sa = sig(a.lerp(b, lo0), o);
        for _ in 0..3 {
            if sa == end {
                break;
            }
            let (mut lo, mut hi) = (lo0, 0.98f32);
            for _ in 0..8 {
                let t = (lo + hi) * 0.5;
                if sig(a.lerp(b, t), o) == sa {
                    lo = t;
                } else {
                    hi = t;
                }
            }
            out.push(a.lerp(b, (lo + hi) * 0.5));
            lo0 = hi;
            sa = sig(a.lerp(b, hi), o);
        }
    }
    out
}

/// The wall at a point of `r`'s outline: its own air sampled at `inn` (just inside), the other
/// rooms' carved out of it at `outp` (just outside). Bands from the bottom up: (bottom, top,
/// where the bottom comes from, where the top does).
fn bands_at(c: &Compiled, r: &CRoom, inn: Vec2, outp: Vec2) -> Vec<(f32, f32, Src, Src)> {
    let (lo_m, hi_m) = own_span(c, r, inn);
    let air = air_outside(c, outp, r.id);
    let mut bands: Vec<(f32, f32, Src, Src)> = Vec::new();
    let mut cur = (lo_m, Src::Own);
    for (alo, ahi, rlo, rhi) in &air {
        if *ahi <= cur.0 + 0.01 {
            continue;
        }
        if *alo >= hi_m - 0.01 {
            break;
        }
        if *alo > cur.0 + 0.01 {
            bands.push((cur.0, *alo, cur.1, Src::Other(*rlo)));
        }
        cur = (*ahi, Src::Other(*rhi));
    }
    if cur.0 < hi_m - 0.01 {
        bands.push((cur.0, hi_m, cur.1, Src::Own));
    }
    bands
}

/// Merged air spans at `p` excluding room `skip`, sorted, with the rooms whose floor and roof
/// bound each.
fn air_outside(c: &Compiled, p: Vec2, skip: u32) -> Vec<(f32, f32, u32, u32)> {
    let mut spans: Vec<Span> = Vec::new();
    c.spans(p, &mut spans);
    spans.retain(|s| s.room != skip);
    spans.sort_by(|a, b| a.lo.total_cmp(&b.lo));
    let mut out: Vec<(f32, f32, u32, u32)> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(l) if s.lo <= l.1 + 0.01 => {
                if s.hi > l.1 {
                    l.1 = s.hi;
                    l.3 = s.room;
                }
            }
            _ => out.push((s.lo, s.hi, s.room, s.room)),
        }
    }
    out
}

/// A room's air at a wall point: (floor, roof).
fn own_span(c: &Compiled, r: &CRoom, p: Vec2) -> (f32, f32) {
    let lo = r.floor_at(p);
    let hi = match r.kind {
        RoomKind::Pit | RoomKind::Chasm | RoomKind::Pool => c.parent_ceil(r, p),
        _ => r.ceil_at(p),
    };
    (lo, hi.max(lo))
}

/// The height of a span bound of room `id` at `p` (`top`: its roof, else its floor).
fn other_bound(c: &Compiled, id: u32, p: Vec2, top: bool) -> Option<f32> {
    let o = c.room(id)?;
    Some(if top {
        own_span(c, o, p).1
    } else {
        o.floor_at(p)
    })
}

fn walls(out: &mut Built, c: &Compiled, r: &CRoom, si: usize, st: &Style) {
    let cavern = r.kind == RoomKind::Cavern;
    let loops: Vec<Vec<Vec2>> = if cavern {
        geom::contours(r.lo, r.hi, 0.35, |p| r.inside(p))
            .into_iter()
            .map(|l| geom::simplify_loop(&l, 0.03))
            .collect()
    } else {
        vec![r.shape.outline()]
    };
    let wall_tile = st.mat(if cavern { Slot::Rock } else { Slot::Wall }).tile;
    let trims = !cavern && !r.kind.is_pit() && r.kind != RoomKind::Pool;
    for lp in loops {
        if lp.len() < 3 {
            continue;
        }
        let pts = refine(c, r, &resample_loop(&lp, if cavern { 0.6 } else { 0.5 }));
        let n = pts.len();
        // outward normal of each piece, and mitred ones at each point
        let mut seg_out: Vec<Vec2> = Vec::with_capacity(n);
        for k in 0..n {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            let m = (a + b) * 0.5;
            let cand = (b - a).perp().normalize_or_zero();
            seg_out.push(if r.inside(m + cand * 0.05) < r.inside(m - cand * 0.05) {
                cand
            } else {
                -cand
            });
        }
        let vert_out: Vec<Vec2> = (0..n)
            .map(|k| {
                let (p, q) = (seg_out[(k + n - 1) % n], seg_out[k]);
                let s = (p + q).normalize_or(q);
                s / s.dot(q).max(0.35)
            })
            .collect();
        let mut s_along = 0.0f32;
        // per piece: the wall bands (bottom, top, sources) at its middle
        struct Piece {
            a: Vec2,
            b: Vec2,
            na: Vec2,
            nb: Vec2,
            out: Vec2,
            s0: f32,
            s1: f32,
            bands: Vec<(f32, f32, f32, f32, Src, Src)>,
            /// own floor and roof at a and b
            own: [(f32, f32); 2],
        }
        let mut pieces: Vec<Piece> = Vec::with_capacity(n);
        for k in 0..n {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            let len = a.distance(b);
            let o = seg_out[k];
            let m = (a + b) * 0.5;
            let inn = |p: Vec2| p - o * WALL_IN;
            let outp = |p: Vec2| p + o * WALL_OUT;
            let own_a = own_span(c, r, inn(a));
            let own_b = own_span(c, r, inn(b));
            // carve the other rooms' air out of ours
            let bands = bands_at(c, r, inn(m), outp(m));
            // heights at the piece's ends
            let at = |p: Vec2, src: Src, top: bool, own: (f32, f32)| -> f32 {
                match src {
                    Src::Own => {
                        if top {
                            own.1
                        } else {
                            own.0
                        }
                    }
                    // a header's bottom is the other room's roof; a riser's top its floor
                    Src::Other(id) => {
                        other_bound(c, id, outp(p), !top).unwrap_or(if top { own.1 } else { own.0 })
                    }
                }
            };
            let bands: Vec<(f32, f32, f32, f32, Src, Src)> = bands
                .into_iter()
                .map(|(_, _, sb, stp)| {
                    let (ba, bb) = (at(a, sb, false, own_a), at(b, sb, false, own_b));
                    let (ta, tb) = (at(a, stp, true, own_a), at(b, stp, true, own_b));
                    (ba, bb, ta.max(ba), tb.max(bb), sb, stp)
                })
                .collect();
            pieces.push(Piece {
                a,
                b,
                na: vert_out[k],
                nb: vert_out[(k + 1) % n],
                out: o,
                s0: s_along,
                s1: s_along + len,
                bands,
                own: [own_a, own_b],
            });
            s_along += len;
        }
        // emit
        let lower = if trims { st.lower } else { None };
        for p in &pieces {
            let inward = Vec3::new(-p.out.x, 0.0, -p.out.y);
            let (ua, ub) = (p.s0 / wall_tile, p.s1 / wall_tile);
            for &(ba, bb, ta, tb, sb, stp) in &p.bands {
                if ta - ba < 0.02 && tb - bb < 0.02 {
                    continue;
                }
                // a lower band (wainscot) where the wall stands on this room's floor
                let split = match (lower, sb) {
                    (Some((_, h)), Src::Own) => Some(h),
                    _ => None,
                };
                let slot_main = if cavern { Slot::Rock } else { Slot::Wall };
                let mut emit = |y0a: f32,
                                y0b: f32,
                                y1a: f32,
                                y1b: f32,
                                slot: Slot,
                                ao0: f32,
                                ao1: f32,
                                tile: f32| {
                    if y1a - y0a < 0.005 && y1b - y0b < 0.005 {
                        return;
                    }
                    let mesh = out.part(r.level, Layer::Main, si, slot);
                    let (ua, ub) = (p.s0 / tile, p.s1 / tile);
                    mesh.quad(
                        [
                            Vec3::new(p.a.x, y0a, p.a.y),
                            Vec3::new(p.b.x, y0b, p.b.y),
                            Vec3::new(p.b.x, y1b, p.b.y),
                            Vec3::new(p.a.x, y1a, p.a.y),
                        ],
                        inward,
                        [
                            Vec2::new(ua, -y0a / tile),
                            Vec2::new(ub, -y0b / tile),
                            Vec2::new(ub, -y1b / tile),
                            Vec2::new(ua, -y1a / tile),
                        ],
                        [ao0, ao0, ao1, ao1],
                    );
                };
                // ambient occlusion: darker into the corner with the floor
                let foot = if matches!(sb, Src::Own) { 0.55 } else { 0.85 };
                let band_h = 0.9f32;
                let (ma, mb) = ((ba + band_h).min(ta), (bb + band_h).min(tb));
                if let Some(h) = split {
                    let (la, lb) = ((ba + h).min(ta), (bb + h).min(tb));
                    let lt = st.mat(Slot::WallLower).tile;
                    emit(
                        ba,
                        bb,
                        ma.min(la),
                        mb.min(lb),
                        Slot::WallLower,
                        foot,
                        0.95,
                        lt,
                    );
                    emit(
                        ma.min(la),
                        mb.min(lb),
                        la,
                        lb,
                        Slot::WallLower,
                        0.95,
                        1.0,
                        lt,
                    );
                    emit(la, lb, ta, tb, slot_main, 1.0, 0.9, wall_tile);
                } else {
                    emit(ba, bb, ma, mb, slot_main, foot, 0.97, wall_tile);
                    emit(ma, mb, ta, tb, slot_main, 0.97, 0.88, wall_tile);
                }
                let _ = (ua, ub);
                // the cap over a wall that meets rock above (shown by the cut-away)
                if matches!(stp, Src::Own) {
                    let t = 0.9;
                    let cap = out.part(r.level, Layer::Cap, si, Slot::Cap);
                    let (oa, ob) = (p.a + p.out * t, p.b + p.out * t);
                    cap.quad(
                        [
                            Vec3::new(p.a.x, ta, p.a.y),
                            Vec3::new(p.b.x, tb, p.b.y),
                            Vec3::new(ob.x, tb, ob.y),
                            Vec3::new(oa.x, ta, oa.y),
                        ],
                        Vec3::Y,
                        [
                            Vec2::new(p.a.x, p.a.y) / 2.0,
                            Vec2::new(p.b.x, p.b.y) / 2.0,
                            Vec2::new(ob.x, ob.y) / 2.0,
                            Vec2::new(oa.x, oa.y) / 2.0,
                        ],
                        [0.6; 4],
                    );
                }
            }
        }
        if !trims {
            continue;
        }
        // skirting and cornice along the walls that stand on the floor / reach the roof
        let tt = st.mat(Slot::Trim).tile;
        for p in &pieces {
            let Some(&(ba, bb, ta, tb, sb, stp)) = p.bands.first() else {
                continue;
            };
            if st.plinth && matches!(sb, Src::Own) {
                strip(
                    out.part(r.level, Layer::Main, si, Slot::Trim),
                    p.a,
                    p.b,
                    p.na,
                    p.nb,
                    ba,
                    bb,
                    0.32,
                    0.1,
                    p.s0,
                    p.s1,
                    tt,
                    true,
                );
            }
            let top = p.bands.last().copied().unwrap_or((ba, bb, ta, tb, sb, stp));
            if st.cornice && matches!(top.5, Src::Own) && r.ceiling != Ceiling::Rock {
                let (ta, tb) = (top.2, top.3);
                strip(
                    out.part(r.level, Layer::Main, si, Slot::Trim),
                    p.a,
                    p.b,
                    p.na,
                    p.nb,
                    ta - 0.34,
                    tb - 0.34,
                    0.34,
                    0.16,
                    p.s0,
                    p.s1,
                    tt,
                    false,
                );
            }
        }
        // pilasters on whole stretches of straight wall
        if st.pilasters > 0.0 {
            let mut next = st.pilasters * 0.5;
            for (k, p) in pieces.iter().enumerate() {
                if p.s1 < next {
                    continue;
                }
                next += st.pilasters;
                let whole = |q: &Piece| {
                    q.bands.len() == 1
                        && matches!(q.bands[0].4, Src::Own)
                        && matches!(q.bands[0].5, Src::Own)
                };
                let near_ok = (0..5).all(|d| {
                    let q = &pieces[(k + n - 2 + d) % n];
                    whole(q) && q.out.dot(p.out) > 0.985
                });
                if !near_ok {
                    continue;
                }
                let m = (p.a + p.b) * 0.5;
                let (lo, hi) = (p.own[0].0.min(p.own[1].0), p.bands[0].2.min(p.bands[0].3));
                pilaster(
                    out.part(r.level, Layer::Main, si, Slot::Trim),
                    m,
                    -p.out,
                    lo,
                    hi,
                    0.7,
                    0.2,
                    tt,
                );
            }
        }
        // doorways: openings at floor level with wall over them (a header)
        let mut k = 0;
        while k < n {
            let opening = |q: &Piece| {
                q.bands.first().is_some_and(|b| !matches!(b.4, Src::Own)) || q.bands.is_empty()
            };
            let header = |q: &Piece| {
                q.bands
                    .last()
                    .is_some_and(|b| matches!(b.5, Src::Own) && !matches!(b.4, Src::Own))
            };
            if opening(&pieces[k]) && header(&pieces[k]) && !opening(&pieces[(k + n - 1) % n]) {
                let mut j = k;
                let mut w = 0.0;
                while opening(&pieces[j % n]) && header(&pieces[j % n]) && w < 12.0 {
                    w += pieces[j % n].a.distance(pieces[j % n].b);
                    j += 1;
                }
                if w > 0.8 && w < 10.0 && !opening(&pieces[j % n]) {
                    let first = &pieces[k];
                    let last = &pieces[(j - 1) % n];
                    let floor = first.own[0].0;
                    let top = first.bands.last().map(|b| b.0).unwrap_or(floor + 3.0);
                    out.doorways.push(Doorway {
                        room: r.id,
                        a: first.a,
                        b: last.b,
                        floor,
                        top,
                        inward: -first.out,
                    });
                    if st.frames {
                        let mesh = out.part(r.level, Layer::Main, si, Slot::Trim);
                        let tops: Vec<f32> = (k..j)
                            .map(|q| pieces[q % n].bands.last().map(|b| b.0).unwrap_or(top))
                            .collect();
                        let jamb_top_a = tops.first().copied().unwrap_or(top);
                        let jamb_top_b = tops.last().copied().unwrap_or(top);
                        pilaster(
                            mesh,
                            first.a + first.out * 0.0
                                - (first.b - first.a).normalize_or_zero() * 0.3,
                            -first.out,
                            floor,
                            jamb_top_a + 0.35,
                            0.6,
                            0.24,
                            tt,
                        );
                        pilaster(
                            mesh,
                            last.b + (last.b - last.a).normalize_or_zero() * 0.3,
                            -last.out,
                            last.own[1].0,
                            jamb_top_b + 0.35,
                            0.6,
                            0.24,
                            tt,
                        );
                        // the archivolt: a band round the opening's top
                        for q in k..j {
                            let pc = &pieces[q % n];
                            let Some(hb) = pc.bands.last() else { continue };
                            let (ya, yb) = (hb.0, hb.1);
                            strip(
                                mesh, pc.a, pc.b, pc.na, pc.nb, ya, yb, 0.36, 0.18, pc.s0, pc.s1,
                                tt, false,
                            );
                        }
                    }
                }
                k = j.max(k + 1);
            } else {
                k += 1;
            }
        }
    }
}

/// A moulding along a wall piece: `h` tall from `y0a`/`y0b`, standing `d` out of the wall, its
/// front and its top (`top_face`) or bottom.
#[allow(clippy::too_many_arguments)]
fn strip(
    m: &mut MeshData,
    a: Vec2,
    b: Vec2,
    na: Vec2,
    nb: Vec2,
    y0a: f32,
    y0b: f32,
    h: f32,
    d: f32,
    s0: f32,
    s1: f32,
    tile: f32,
    top_face: bool,
) {
    let (fa, fb) = (a - na * d, b - nb * d);
    let inward = (-(na + nb)).normalize_or_zero();
    let n3 = Vec3::new(inward.x, 0.0, inward.y);
    let (ua, ub) = (s0 / tile, s1 / tile);
    m.quad(
        [
            Vec3::new(fa.x, y0a, fa.y),
            Vec3::new(fb.x, y0b, fb.y),
            Vec3::new(fb.x, y0b + h, fb.y),
            Vec3::new(fa.x, y0a + h, fa.y),
        ],
        n3,
        [
            Vec2::new(ua, 0.0),
            Vec2::new(ub, 0.0),
            Vec2::new(ub, -h / tile),
            Vec2::new(ua, -h / tile),
        ],
        [0.8, 0.8, 0.95, 0.95],
    );
    let (ya, yb, n) = if top_face {
        (y0a + h, y0b + h, Vec3::Y)
    } else {
        (y0a, y0b, Vec3::NEG_Y)
    };
    m.quad(
        [
            Vec3::new(a.x, ya, a.y),
            Vec3::new(b.x, yb, b.y),
            Vec3::new(fb.x, yb, fb.y),
            Vec3::new(fa.x, ya, fa.y),
        ],
        n,
        [
            Vec2::new(ua, 0.0),
            Vec2::new(ub, 0.0),
            Vec2::new(ub, d / tile),
            Vec2::new(ua, d / tile),
        ],
        [0.9; 4],
    );
}

/// A pilaster (or a door jamb): `w` wide, standing `d` out of the wall at `at` (facing `inward`),
/// from `lo` to `hi`.
#[allow(clippy::too_many_arguments)]
fn pilaster(m: &mut MeshData, at: Vec2, inward: Vec2, lo: f32, hi: f32, w: f32, d: f32, tile: f32) {
    let along = inward.perp();
    let c0 = at;
    let c1 = at + inward * d;
    let corners = [
        c0 - along * w * 0.5,
        c0 + along * w * 0.5,
        c1 + along * w * 0.5,
        c1 - along * w * 0.5,
    ];
    // three faces: front and the two sides (the back is in the wall); and a cap
    let faces = [
        (corners[3], corners[2], inward),
        (corners[0], corners[3], -along),
        (corners[2], corners[1], along),
    ];
    for (a, b, n) in faces {
        let len = a.distance(b);
        m.quad(
            [
                Vec3::new(a.x, lo, a.y),
                Vec3::new(b.x, lo, b.y),
                Vec3::new(b.x, hi, b.y),
                Vec3::new(a.x, hi, a.y),
            ],
            Vec3::new(n.x, 0.0, n.y),
            [
                Vec2::new(0.0, -lo / tile),
                Vec2::new(len / tile, -lo / tile),
                Vec2::new(len / tile, -hi / tile),
                Vec2::new(0.0, -hi / tile),
            ],
            [0.7, 0.7, 1.0, 1.0],
        );
    }
    let y = hi;
    m.quad(
        [
            Vec3::new(corners[0].x, y, corners[0].y),
            Vec3::new(corners[1].x, y, corners[1].y),
            Vec3::new(corners[2].x, y, corners[2].y),
            Vec3::new(corners[3].x, y, corners[3].y),
        ],
        Vec3::Y,
        [
            Vec2::ZERO,
            Vec2::new(w / tile, 0.0),
            Vec2::new(w / tile, d / tile),
            Vec2::new(0.0, d / tile),
        ],
        [1.0; 4],
    );
}

// ------------------------------------------------------------------ floors and ceilings

/// Fills where `g` is positive over `r`'s bounds with marching-squares cells, heights from `h`.
/// `up`: the surface faces up (floors) or down (ceilings). Flat surfaces join whole cells into rows.
#[allow(clippy::too_many_arguments)]
fn fill(
    m: &mut MeshData,
    lo: Vec2,
    hi: Vec2,
    cell: f32,
    g: &dyn Fn(Vec2) -> f32,
    h: &dyn Fn(Vec2) -> f32,
    flat: bool,
    up: bool,
    tile: f32,
    ao_edge: bool,
) {
    let nx = (((hi.x - lo.x) / cell).ceil() as i32).max(1);
    let nz = (((hi.y - lo.y) / cell).ceil() as i32).max(1);
    let w = (nx + 1) as usize;
    let mut gv = vec![0.0f32; w * (nz + 1) as usize];
    for j in 0..=nz {
        for i in 0..=nx {
            let p = lo + Vec2::new(i as f32, j as f32) * cell;
            let v = g(p);
            gv[j as usize * w + i as usize] = if v == 0.0 { -1e-6 } else { v };
        }
    }
    let at = |i: i32, j: i32| gv[j as usize * w + i as usize];
    let pos = |i: i32, j: i32| lo + Vec2::new(i as f32, j as f32) * cell;
    let face_n = if up { Vec3::Y } else { Vec3::NEG_Y };
    let normal = |p: Vec2| -> Vec3 {
        if flat {
            return face_n;
        }
        let e = cell * 0.5;
        let dx = h(p + Vec2::X * e) - h(p - Vec2::X * e);
        let dz = h(p + Vec2::Y * e) - h(p - Vec2::Y * e);
        let n = Vec3::new(-dx, 2.0 * e, -dz).normalize_or(Vec3::Y);
        if up { n } else { -n }
    };
    // where the cell edge from `a` (inside) to `b` (outside) leaves the surface, found by halving
    // rather than read off the corners: where another room's ceiling or floor takes over, `g`
    // jumps there, and two rooms' surfaces cut on their own grids would stop up to a cell short
    // of the seam they share (a gap of sky between a cavern's dome and a vault)
    let cross = |a: Vec2, b: Vec2| -> Vec2 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..8 {
            let t = (lo + hi) * 0.5;
            if g(a.lerp(b, t)) > 0.0 {
                lo = t;
            } else {
                hi = t;
            }
        }
        a.lerp(b, (lo + hi) * 0.5)
    };
    // ambient occlusion: darker near the edge (where it meets a wall)
    let ao = |p: Vec2| {
        if ao_edge {
            (0.62 + g(p).clamp(0.0, 1.2) / 1.2 * 0.38).min(1.0)
        } else {
            1.0
        }
    };
    let vert = |m: &mut MeshData, p: Vec2| -> u32 {
        let b = m.base();
        let y = h(p);
        let n = normal(p);
        // (steep curves of vaults and rock take their texture from the side they face)
        let uv = if flat || n.y.abs() > 0.6 {
            p / tile
        } else if n.x.abs() > n.z.abs() {
            Vec2::new(p.y, -y) / tile
        } else {
            Vec2::new(p.x, -y) / tile
        };
        m.v(Vec3::new(p.x, y, p.y), n, uv, ao(p));
        b
    };
    for j in 0..nz {
        let mut i = 0;
        while i < nx {
            // whole cells (away from the edge when the edge is shaded) join into rows
            let deep = if ao_edge { 1.2 } else { 0.0 };
            let full = |i: i32| {
                at(i, j) > deep
                    && at(i + 1, j) > deep
                    && at(i + 1, j + 1) > deep
                    && at(i, j + 1) > deep
            };
            if flat && full(i) {
                // a run of whole cells: one quad
                let mut e = i + 1;
                while e < nx && full(e) {
                    e += 1;
                }
                let (a, b, cc, d) = (pos(i, j), pos(e, j), pos(e, j + 1), pos(i, j + 1));
                let y = h((a + cc) * 0.5);
                m.quad(
                    [
                        Vec3::new(a.x, y, a.y),
                        Vec3::new(b.x, y, b.y),
                        Vec3::new(cc.x, y, cc.y),
                        Vec3::new(d.x, y, d.y),
                    ],
                    face_n,
                    [a / tile, b / tile, cc / tile, d / tile],
                    [1.0; 4],
                );
                i = e;
                continue;
            }
            // the cell's inside polygon: corners inside and edge crossings, in order
            let cs = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let vals = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            let inside = vals.iter().filter(|v| **v > 0.0).count();
            if inside == 0 {
                i += 1;
                continue;
            }
            let saddle = inside == 2 && (vals[0] > 0.0) == (vals[2] > 0.0);
            let centre = vals.iter().sum::<f32>() * 0.25;
            let mut polys: Vec<Vec<Vec2>> = Vec::new();
            if saddle && centre <= 0.0 {
                // two separate corners
                for k in 0..4 {
                    if vals[k] > 0.0 {
                        let (pk, pn, pp) = (k, (k + 1) % 4, (k + 3) % 4);
                        let a = pos(cs[pk].0, cs[pk].1);
                        let edge = |o: usize| cross(a, pos(cs[o].0, cs[o].1));
                        polys.push(vec![a, edge(pn), edge(pp)]);
                    }
                }
            } else {
                let mut poly = Vec::with_capacity(6);
                for k in 0..4 {
                    let kn = (k + 1) % 4;
                    let a = pos(cs[k].0, cs[k].1);
                    if vals[k] > 0.0 {
                        poly.push(a);
                    }
                    if (vals[k] > 0.0) != (vals[kn] > 0.0) {
                        // (always from the inside corner: the cell next door finds the same point)
                        let b = pos(cs[kn].0, cs[kn].1);
                        poly.push(if vals[k] > 0.0 {
                            cross(a, b)
                        } else {
                            cross(b, a)
                        });
                    }
                }
                polys.push(poly);
            }
            for poly in polys {
                if poly.len() < 3 {
                    continue;
                }
                let ids: Vec<u32> = poly.iter().map(|p| vert(m, *p)).collect();
                for t in 1..ids.len() - 1 {
                    let (a, b, cc) = (ids[0], ids[t], ids[t + 1]);
                    let pa = Vec3::from_array(m.pos[a as usize]);
                    let pb = Vec3::from_array(m.pos[b as usize]);
                    let pc = Vec3::from_array(m.pos[cc as usize]);
                    if (pb - pa).cross(pc - pa).dot(face_n) >= 0.0 {
                        m.idx.extend_from_slice(&[a, b, cc]);
                    } else {
                        m.idx.extend_from_slice(&[a, cc, b]);
                    }
                }
            }
            i += 1;
        }
    }
}

/// Does room `q`'s air hide room `r`'s floor at `p` (a lower floor under it, or the same floor
/// drawn by `q`)?
fn beats_floor(c: &Compiled, r: &CRoom, q: &CRoom, p: Vec2, fr: f32) -> bool {
    let (qlo, qhi) = own_span(c, q, p);
    if qhi <= fr + 0.05 {
        return false;
    }
    // the lower floor shows (ties to the higher priority); each reaches `SEAM` past where the
    // two cross, so no crack opens between them along it
    let tie = if (q.prio(), q.id) > (r.prio(), r.id) {
        0.03
    } else {
        -0.03
    };
    qlo < fr + tie - SEAM
}

/// How far (m, up or down) a floor or ceiling reaches past where another room's takes over: the
/// two overlap a little along the line where they cross (less than the 3 cm of a tie, so equal
/// surfaces never both show).
const SEAM: f32 = 0.02;

fn floor(out: &mut Built, c: &Compiled, r: &CRoom, si: usize) {
    let st = &styles::all()[si];
    let cavern = r.kind.is_rock();
    let slot = if cavern { Slot::RockFloor } else { Slot::Floor };
    let tile = st.mat(slot).tile;
    let others: Vec<&CRoom> = c
        .rooms
        .iter()
        .filter(|q| {
            q.id != r.id
                && q.kind.is_space()
                && q.lo.x <= r.hi.x
                && q.hi.x >= r.lo.x
                && q.lo.y <= r.hi.y
                && q.hi.y >= r.lo.y
        })
        .collect();
    let g = |p: Vec2| -> f32 {
        let mut v = r.inside(p);
        if v <= -0.5 {
            return v;
        }
        let fr = r.floor_at(p);
        for q in &others {
            let qi = q.inside(p);
            if qi > -0.6 && beats_floor(c, r, q, p, fr) {
                v = v.min(-qi);
            }
        }
        v
    };
    let h = |p: Vec2| r.floor_at(p);
    let cell = if cavern { 0.5 } else { 0.4 };
    let m = out.part(r.level, Layer::Main, si, slot);
    fill(m, r.lo, r.hi, cell, &g, &h, !cavern, true, tile, true);
}

fn ceiling(out: &mut Built, c: &Compiled, r: &CRoom, si: usize, st: &Style) {
    let cavern = r.kind == RoomKind::Cavern || r.ceiling == Ceiling::Rock;
    let slot = if cavern { Slot::Rock } else { Slot::Ceiling };
    let tile = st.mat(slot).tile;
    // (pits and pools draw no ceiling: they never hide one)
    let others: Vec<&CRoom> = c
        .rooms
        .iter()
        .filter(|q| {
            q.id != r.id
                && q.kind.is_space()
                && !q.kind.is_pit()
                && q.kind != RoomKind::Pool
                && q.lo.x <= r.hi.x
                && q.hi.x >= r.lo.x
                && q.lo.y <= r.hi.y
                && q.hi.y >= r.lo.y
        })
        .collect();
    let g = |p: Vec2| -> f32 {
        let mut v = r.inside(p);
        if v <= -0.5 {
            return v;
        }
        let hr = own_span(c, r, p).1;
        for q in &others {
            let qi = q.inside(p);
            if qi <= -0.6 {
                continue;
            }
            let (qlo, qhi) = own_span(c, q, p);
            // the higher roof shows (ties to the higher priority), each `SEAM` past the crossing
            let tie = if (q.prio(), q.id) > (r.prio(), r.id) {
                -0.03
            } else {
                0.03
            };
            let beats = qlo < hr - 0.05 && qhi > hr + tie + SEAM;
            if beats {
                v = v.min(-qi);
            }
        }
        v
    };
    let h = |p: Vec2| own_span(c, r, p).1;
    let flat = matches!(r.ceiling, Ceiling::Flat | Ceiling::Beams);
    let cell = if flat {
        0.5
    } else {
        (r.inr / 9.0).clamp(0.15, 0.5)
    };
    let m = out.part(r.level, Layer::Ceiling, si, slot);
    fill(m, r.lo, r.hi, cell, &g, &h, flat, false, tile, false);
    // beams across the room
    if r.ceiling == Ceiling::Beams {
        let (ax, half_l, _) = r.shape.axes();
        let across = ax.perp();
        let c0 = r.shape.centre();
        let n = ((half_l * 2.0) / 3.2).floor() as i32;
        let bt = st.mat(Slot::Beam).tile;
        let mesh = out.part(r.level, Layer::Ceiling, si, Slot::Beam);
        for k in 0..=n {
            let t = -half_l + 1.6 + k as f32 * 3.2;
            if t > half_l - 1.0 {
                break;
            }
            let mid = c0 + ax * t;
            // reach across until the wall
            let reach = |dir: f32| {
                let mut s = 0.0;
                while s < 200.0 && r.inside(mid + across * dir * (s + 0.25)) > 0.0 {
                    s += 0.25;
                }
                s
            };
            let (s0, s1) = (reach(-1.0), reach(1.0));
            if s0 + s1 < 1.0 {
                continue;
            }
            let (a, b) = (mid - across * s0, mid + across * s1);
            let top = r.ceil_at(mid);
            beam(mesh, a, b, top - 0.5, top, 0.36, bt);
        }
    }
}

/// A beam (a box) from `a` to `b`, from `y0` to `y1`, `w` wide: its underside and sides.
fn beam(m: &mut MeshData, a: Vec2, b: Vec2, y0: f32, y1: f32, w: f32, tile: f32) {
    let d = (b - a).normalize_or_zero();
    let s = d.perp() * w * 0.5;
    let len = a.distance(b);
    let (a0, a1, b0, b1) = (a - s, a + s, b - s, b + s);
    m.quad(
        [
            Vec3::new(a0.x, y0, a0.y),
            Vec3::new(b0.x, y0, b0.y),
            Vec3::new(b1.x, y0, b1.y),
            Vec3::new(a1.x, y0, a1.y),
        ],
        Vec3::NEG_Y,
        [
            Vec2::ZERO,
            Vec2::new(len / tile, 0.0),
            Vec2::new(len / tile, w / tile),
            Vec2::new(0.0, w / tile),
        ],
        [0.85; 4],
    );
    for (p, q, n) in [(a0, b0, -d.perp()), (b1, a1, d.perp())] {
        m.quad(
            [
                Vec3::new(p.x, y0, p.y),
                Vec3::new(q.x, y0, q.y),
                Vec3::new(q.x, y1, q.y),
                Vec3::new(p.x, y1, p.y),
            ],
            Vec3::new(n.x, 0.0, n.y),
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(len / tile, 0.0),
                Vec2::new(len / tile, (y1 - y0) / tile),
                Vec2::new(0.0, (y1 - y0) / tile),
            ],
            [0.9, 0.9, 0.7, 0.7],
        );
    }
}

// ------------------------------------------------------------------ solids

fn pillar(m: &mut MeshData, at: Vec2, r: f32, y0: f32, y1: f32, shape: PillarShape, tile: f32) {
    let sides = match shape {
        PillarShape::Square => 4,
        PillarShape::Octagon => 8,
        PillarShape::Round => 16,
    };
    let rot = if sides == 4 {
        std::f32::consts::FRAC_PI_4
    } else {
        std::f32::consts::PI / sides as f32
    };
    let rr = if sides == 4 {
        r * std::f32::consts::SQRT_2
    } else {
        r / (std::f32::consts::PI / sides as f32).cos()
    };
    let base_h = 0.55f32.min((y1 - y0) * 0.15);
    let cap_h = 0.6f32.min((y1 - y0) * 0.15);
    // (bottom, top, radius scale)
    let tiers = [
        (y0, y0 + base_h * 0.55, 1.32),
        (y0 + base_h * 0.55, y0 + base_h, 1.16),
        (y0 + base_h, y1 - cap_h, 1.0),
        (y1 - cap_h, y1 - cap_h * 0.45, 1.14),
        (y1 - cap_h * 0.45, y1, 1.3),
    ];
    for (b, t, s) in tiers {
        if t - b < 0.01 {
            continue;
        }
        let rad = rr * s;
        let perim = std::f32::consts::TAU * rad;
        for k in 0..sides {
            let a0 = rot + std::f32::consts::TAU * k as f32 / sides as f32;
            let a1 = rot + std::f32::consts::TAU * (k + 1) as f32 / sides as f32;
            let p0 = at + Vec2::from_angle(a0) * rad;
            let p1 = at + Vec2::from_angle(a1) * rad;
            let mid = (a0 + a1) * 0.5;
            let n = if sides >= 16 {
                None
            } else {
                Some(Vec2::from_angle(mid))
            };
            let (u0, u1) = (
                perim * k as f32 / sides as f32 / tile,
                perim * (k + 1) as f32 / sides as f32 / tile,
            );
            let base = m.base();
            let ns = [
                n.unwrap_or(Vec2::from_angle(a0)),
                n.unwrap_or(Vec2::from_angle(a1)),
            ];
            let ao_b = if b <= y0 + 0.01 { 0.6 } else { 0.95 };
            for (p, nn, u, y, ao) in [
                (p0, ns[0], u0, b, ao_b),
                (p1, ns[1], u1, b, ao_b),
                (p1, ns[1], u1, t, 1.0),
                (p0, ns[0], u0, t, 1.0),
            ] {
                m.v(
                    Vec3::new(p.x, y, p.y),
                    Vec3::new(nn.x, 0.0, nn.y),
                    Vec2::new(u, -y / tile),
                    ao,
                );
            }
            // outward faces: (p0, p1 counter-clockwise round the centre seen from above)
            m.idx
                .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
        }
        // the ledge on top of a wider tier
        if s > 1.05 {
            for k in 0..sides {
                let a0 = rot + std::f32::consts::TAU * k as f32 / sides as f32;
                let a1 = rot + std::f32::consts::TAU * (k + 1) as f32 / sides as f32;
                let (o0, o1) = (
                    at + Vec2::from_angle(a0) * rad,
                    at + Vec2::from_angle(a1) * rad,
                );
                let (i0, i1) = (
                    at + Vec2::from_angle(a0) * rr,
                    at + Vec2::from_angle(a1) * rr,
                );
                let y = if b < y0 + base_h + 0.01 { t } else { b };
                let n = if b < y0 + base_h + 0.01 {
                    Vec3::Y
                } else {
                    Vec3::NEG_Y
                };
                m.quad(
                    [
                        Vec3::new(o0.x, y, o0.y),
                        Vec3::new(o1.x, y, o1.y),
                        Vec3::new(i1.x, y, i1.y),
                        Vec3::new(i0.x, y, i0.y),
                    ],
                    n,
                    [o0 / tile, o1 / tile, i1 / tile, i0 / tile],
                    [0.95; 4],
                );
            }
        }
    }
}

/// A plain shaft (a pillar going on up into a vault).
fn pillar_shaft(
    m: &mut MeshData,
    at: Vec2,
    r: f32,
    y0: f32,
    y1: f32,
    shape: PillarShape,
    tile: f32,
) {
    let sides = match shape {
        PillarShape::Square => 4,
        PillarShape::Octagon => 8,
        PillarShape::Round => 16,
    };
    let rot = if sides == 4 {
        std::f32::consts::FRAC_PI_4
    } else {
        std::f32::consts::PI / sides as f32
    };
    let rr = if sides == 4 {
        r * std::f32::consts::SQRT_2
    } else {
        r / (std::f32::consts::PI / sides as f32).cos()
    };
    let perim = std::f32::consts::TAU * rr;
    for k in 0..sides {
        let a0 = rot + std::f32::consts::TAU * k as f32 / sides as f32;
        let a1 = rot + std::f32::consts::TAU * (k + 1) as f32 / sides as f32;
        let (p0, p1) = (
            at + Vec2::from_angle(a0) * rr,
            at + Vec2::from_angle(a1) * rr,
        );
        let n = Vec2::from_angle((a0 + a1) * 0.5);
        let (u0, u1) = (
            perim * k as f32 / sides as f32 / tile,
            perim * (k + 1) as f32 / sides as f32 / tile,
        );
        let base = m.base();
        for (p, u, y) in [(p0, u0, y0), (p1, u1, y0), (p1, u1, y1), (p0, u0, y1)] {
            m.v(
                Vec3::new(p.x, y, p.y),
                Vec3::new(n.x, 0.0, n.y),
                Vec2::new(u, -y / tile),
                0.9,
            );
        }
        m.idx
            .extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}

fn dais(out: &mut Built, r: &CRoom, si: usize, base: f32, rise: f32) {
    let st = &styles::all()[si];
    let tiers = (rise / STEP_H).ceil().max(1.0) as i32;
    let ft = st.mat(Slot::Floor).tile;
    let tt = st.mat(Slot::Trim).tile;
    for k in 0..tiers {
        let inset = k as f32 * STEP_D;
        let top = base + dais_top(inset + 0.001, rise);
        let below = if k == 0 {
            base
        } else {
            base + dais_top(inset - STEP_D + 0.001, rise)
        };
        if top <= below + 0.001 && k > 0 {
            break;
        }
        // its top
        let g = |p: Vec2| r.inside(p) - inset;
        let h = |_: Vec2| top;
        fill(
            out.part(r.level, Layer::Main, si, Slot::Floor),
            r.lo,
            r.hi,
            0.35,
            &g,
            &h,
            true,
            true,
            ft,
            false,
        );
        // its riser
        let loops = if inset == 0.0 {
            vec![r.shape.outline()]
        } else {
            geom::contours(r.lo, r.hi, 0.2, |p| r.inside(p) - inset)
        };
        let m = out.part(r.level, Layer::Main, si, Slot::Trim);
        for lp in loops {
            let pts = resample_loop(&lp, 1.0);
            let n = pts.len();
            let mut s = 0.0;
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                let mid = (a + b) * 0.5;
                let cand = (b - a).perp().normalize_or_zero();
                let out_n = if r.inside(mid + cand * 0.05) < r.inside(mid - cand * 0.05) {
                    cand
                } else {
                    -cand
                };
                let len = a.distance(b);
                m.quad(
                    [
                        Vec3::new(a.x, below, a.y),
                        Vec3::new(b.x, below, b.y),
                        Vec3::new(b.x, top, b.y),
                        Vec3::new(a.x, top, a.y),
                    ],
                    Vec3::new(out_n.x, 0.0, out_n.y),
                    [
                        Vec2::new(s / tt, 0.0),
                        Vec2::new((s + len) / tt, 0.0),
                        Vec2::new((s + len) / tt, -(top - below) / tt),
                        Vec2::new(s / tt, -(top - below) / tt),
                    ],
                    [0.75, 0.75, 1.0, 1.0],
                );
                s += len;
            }
        }
        if top >= base + rise - 0.001 {
            break;
        }
    }
}

fn deck(out: &mut Built, r: &CRoom, si: usize, top: f32, thick: f32, parapets: bool) {
    let st = &styles::all()[si];
    let ft = st.mat(Slot::Floor).tile;
    let tt = st.mat(Slot::Trim).tile;
    let g = |p: Vec2| r.inside(p);
    let h_top = |_: Vec2| top;
    let h_bot = |_: Vec2| top - thick;
    fill(
        out.part(r.level, Layer::Main, si, Slot::Floor),
        r.lo,
        r.hi,
        0.3,
        &g,
        &h_top,
        true,
        true,
        ft,
        false,
    );
    fill(
        out.part(r.level, Layer::Main, si, Slot::Trim),
        r.lo,
        r.hi,
        0.3,
        &g,
        &h_bot,
        true,
        false,
        tt,
        false,
    );
    let line = r.shape.centreline().unwrap_or_default();
    let ends = [line.first().copied(), line.last().copied()];
    let half_w = r.inr;
    let pts = resample_loop(&r.shape.outline(), 0.5);
    let n = pts.len();
    let m = out.part(r.level, Layer::Main, si, Slot::Trim);
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let mid = (a + b) * 0.5;
        let cand = (b - a).perp().normalize_or_zero();
        let out_n = if r.inside(mid + cand * 0.05) < r.inside(mid - cand * 0.05) {
            cand
        } else {
            -cand
        };
        m.quad(
            [
                Vec3::new(a.x, top - thick, a.y),
                Vec3::new(b.x, top - thick, b.y),
                Vec3::new(b.x, top, b.y),
                Vec3::new(a.x, top, a.y),
            ],
            Vec3::new(out_n.x, 0.0, out_n.y),
            [
                Vec2::new(a.x / tt, 0.0),
                Vec2::new(b.x / tt, 0.0),
                Vec2::new(b.x / tt, -thick / tt),
                Vec2::new(a.x / tt, -thick / tt),
            ],
            [0.8; 4],
        );
        // a parapet along the sides (open at both ends)
        let near_end = ends
            .iter()
            .flatten()
            .any(|e| e.distance(mid) < half_w * 1.3);
        if parapets && !near_end {
            let (ia, ib) = (a - out_n * 0.25, b - out_n * 0.25);
            rail_box(m, ia, ib, top, top + 0.9, 0.25, tt);
        }
    }
}

fn rail(m: &mut MeshData, a: Vec2, b: Vec2, y0: f32, y1: f32) {
    rail_box(m, a, b, y0, y1, 0.24, 1.6);
}

/// A low wall from `a` to `b`, `w` thick: both sides and its top.
fn rail_box(m: &mut MeshData, a: Vec2, b: Vec2, y0: f32, y1: f32, w: f32, tile: f32) {
    let d = (b - a).normalize_or_zero();
    let s = d.perp() * w * 0.5;
    let len = a.distance(b);
    for (p, q, n) in [(a - s, b - s, -d.perp()), (b + s, a + s, d.perp())] {
        m.quad(
            [
                Vec3::new(p.x, y0, p.y),
                Vec3::new(q.x, y0, q.y),
                Vec3::new(q.x, y1, q.y),
                Vec3::new(p.x, y1, p.y),
            ],
            Vec3::new(n.x, 0.0, n.y),
            [
                Vec2::new(0.0, -y0 / tile),
                Vec2::new(len / tile, -y0 / tile),
                Vec2::new(len / tile, -y1 / tile),
                Vec2::new(0.0, -y1 / tile),
            ],
            [0.75, 0.75, 1.0, 1.0],
        );
    }
    let (a0, a1, b0, b1) = (a - s, a + s, b - s, b + s);
    m.quad(
        [
            Vec3::new(a0.x, y1, a0.y),
            Vec3::new(b0.x, y1, b0.y),
            Vec3::new(b1.x, y1, b1.y),
            Vec3::new(a1.x, y1, a1.y),
        ],
        Vec3::Y,
        [
            Vec2::ZERO,
            Vec2::new(len / tile, 0.0),
            Vec2::new(len / tile, w / tile),
            Vec2::new(0.0, w / tile),
        ],
        [1.0; 4],
    );
}

/// A spiral stair's steps: stone wedges round the column, each a step higher, the last few the
/// landing at the top.
#[allow(clippy::too_many_arguments)]
fn spiral(
    out: &mut Built,
    level: i32,
    si: usize,
    c: Vec2,
    r_in: f32,
    r_out: f32,
    y0: f32,
    y1: f32,
    rise: f32,
    start: f32,
) {
    use super::plan::{SPIRAL_STEPS, Solid as S};
    let st = &styles::all()[si];
    let (ft, tt) = (st.mat(Slot::Floor).tile, st.mat(Slot::Trim).tile);
    let step = std::f32::consts::TAU / SPIRAL_STEPS as f32;
    let n = S::spiral_steps(y0, y1, rise);
    let mut tops = MeshData::default();
    let mut sides = MeshData::default();
    let at = |a: f32, r: f32| c + Vec2::from_angle(a) * r;
    let v3 = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
    for k in 0..n {
        let top = S::spiral_top(y0, y1, rise, k);
        let bot = top - 0.3;
        let (a0, a1) = (start + k as f32 * step, start + (k + 1) as f32 * step);
        // the tread, in two slices so it follows the curve
        for (b0, b1) in [(a0, (a0 + a1) * 0.5), ((a0 + a1) * 0.5, a1)] {
            let q = [at(b0, r_in), at(b0, r_out), at(b1, r_out), at(b1, r_in)];
            tops.quad(
                [v3(q[0], top), v3(q[3], top), v3(q[2], top), v3(q[1], top)],
                Vec3::Y,
                [q[0] / ft, q[3] / ft, q[2] / ft, q[1] / ft],
                [0.85, 0.85, 0.95, 0.95],
            );
            sides.quad(
                [v3(q[0], bot), v3(q[1], bot), v3(q[2], bot), v3(q[3], bot)],
                Vec3::NEG_Y,
                [q[0] / tt, q[1] / tt, q[2] / tt, q[3] / tt],
                [0.7; 4],
            );
            // the outer edge (seen from below and from the next turn down)
            let (o0, o1) = (at(b0, r_out), at(b1, r_out));
            let nrm = Vec2::from_angle((b0 + b1) * 0.5);
            sides.quad(
                [v3(o0, bot), v3(o1, bot), v3(o1, top), v3(o0, top)],
                Vec3::new(nrm.x, 0.0, nrm.y),
                [
                    Vec2::new(0.0, -bot / tt),
                    Vec2::new(r_out * step * 0.5 / tt, -bot / tt),
                    Vec2::new(r_out * step * 0.5 / tt, -top / tt),
                    Vec2::new(0.0, -top / tt),
                ],
                [0.75, 0.75, 0.9, 0.9],
            );
        }
        // the riser at its front (from the step below), the face at its back
        let below = if k == 0 {
            y0 - 0.3
        } else {
            S::spiral_top(y0, y1, rise, k - 1)
        };
        let (i0, o0) = (at(a0, r_in), at(a0, r_out));
        let fwd = Vec2::from_angle(a0 - std::f32::consts::FRAC_PI_2);
        sides.quad(
            [
                v3(i0, below.min(bot)),
                v3(o0, below.min(bot)),
                v3(o0, top),
                v3(i0, top),
            ],
            Vec3::new(fwd.x, 0.0, fwd.y),
            [
                Vec2::new(0.0, -bot / tt),
                Vec2::new((r_out - r_in) / tt, -bot / tt),
                Vec2::new((r_out - r_in) / tt, -top / tt),
                Vec2::new(0.0, -top / tt),
            ],
            [0.7, 0.7, 0.9, 0.9],
        );
        let (i1, o1) = (at(a1, r_in), at(a1, r_out));
        sides.quad(
            [v3(o1, bot), v3(i1, bot), v3(i1, top), v3(o1, top)],
            Vec3::new(-fwd.x, 0.0, -fwd.y),
            [
                Vec2::new(0.0, -bot / tt),
                Vec2::new((r_out - r_in) / tt, -bot / tt),
                Vec2::new((r_out - r_in) / tt, -top / tt),
                Vec2::new(0.0, -top / tt),
            ],
            [0.7, 0.7, 0.85, 0.85],
        );
    }
    out.part(level, Layer::Main, si, Slot::Floor).append(tops);
    out.part(level, Layer::Main, si, Slot::Trim).append(sides);
}

fn stairs(out: &mut Built, c: &Compiled, r: &CRoom, si: usize) {
    let Some(rp) = &r.ramp else { return };
    if r.smooth {
        ramp(out, r, si, rp);
        return;
    }
    let st = &styles::all()[si];
    let ft = st.mat(Slot::Floor).tile;
    let tt = st.mat(Slot::Trim).tile;
    let half_w = r.inr;
    let rise = (rp.y1 - rp.y0).abs();
    let land = rp.landing();
    let run = (rp.len - 2.0 * land).max(0.1);
    let steps = ((rise / 0.24).round() as i32).max(1);
    let depth = run / steps as f32;
    let bottom = rp.y0.min(rp.y1) - 0.3;
    // the treads (and the landings at both ends)
    let mut marks: Vec<(f32, f32, f32)> = Vec::new();
    marks.push((0.0, land, rp.y0));
    for k in 0..steps {
        let s0 = land + depth * k as f32;
        // each tread at the ramp's height half way along it (the walker glides on the ramp)
        let y = rp.y0 + (rp.y1 - rp.y0) * ((k as f32 + 0.5) / steps as f32);
        marks.push((s0, s0 + depth, y));
    }
    marks.push((rp.len - land, rp.len, rp.y1));
    let mut m = MeshData::default();
    let mut sides = MeshData::default();
    let mut prev_y: Option<f32> = None;
    for (s0, s1, y) in marks {
        let (p0, d0) = geom::polyline_at(&rp.line, s0);
        let (p1, d1) = geom::polyline_at(&rp.line, s1);
        let (l0, l1) = (d0.perp() * half_w, d1.perp() * half_w);
        let quad = [p0 - l0, p1 - l1, p1 + l1, p0 + l0];
        m.quad(
            [
                Vec3::new(quad[0].x, y, quad[0].y),
                Vec3::new(quad[1].x, y, quad[1].y),
                Vec3::new(quad[2].x, y, quad[2].y),
                Vec3::new(quad[3].x, y, quad[3].y),
            ],
            Vec3::Y,
            [quad[0] / ft, quad[1] / ft, quad[2] / ft, quad[3] / ft],
            [0.9, 0.9, 0.9, 0.9],
        );
        // the riser from the step before
        if let Some(py) = prev_y
            && (py - y).abs() > 0.01
        {
            let (lo, hi) = (py.min(y), py.max(y));
            let face = if y > py { -d0 } else { d0 };
            m.quad(
                [
                    Vec3::new(quad[0].x, lo, quad[0].y),
                    Vec3::new(quad[3].x, lo, quad[3].y),
                    Vec3::new(quad[3].x, hi, quad[3].y),
                    Vec3::new(quad[0].x, hi, quad[0].y),
                ],
                Vec3::new(face.x, 0.0, face.y),
                [
                    Vec2::new(0.0, -lo / tt),
                    Vec2::new(half_w * 2.0 / tt, -lo / tt),
                    Vec2::new(half_w * 2.0 / tt, -hi / tt),
                    Vec2::new(0.0, -hi / tt),
                ],
                [0.7, 0.7, 0.85, 0.85],
            );
        }
        // the sides, down to the bottom of the flight (seen where it climbs through a hall)
        for (a, b, n) in [
            (p0 - l0, p1 - l1, -d0.perp()),
            (p1 + l1, p0 + l0, d0.perp()),
        ] {
            sides.quad(
                [
                    Vec3::new(a.x, bottom, a.y),
                    Vec3::new(b.x, bottom, b.y),
                    Vec3::new(b.x, y, b.y),
                    Vec3::new(a.x, y, a.y),
                ],
                Vec3::new(n.x, 0.0, n.y),
                [
                    Vec2::new(a.x / tt, -bottom / tt),
                    Vec2::new(b.x / tt, -bottom / tt),
                    Vec2::new(b.x / tt, -y / tt),
                    Vec2::new(a.x / tt, -y / tt),
                ],
                [0.7, 0.7, 0.9, 0.9],
            );
        }
        prev_y = Some(y);
    }
    let _ = c;
    out.part(r.level, Layer::Main, si, Slot::Floor).append(m);
    out.part(r.level, Layer::Main, si, Slot::Trim).append(sides);
}

/// Stairs drawn as a smooth ramp: one sloped surface (the walker climbs it as it does steps).
fn ramp(out: &mut Built, r: &CRoom, si: usize, rp: &super::plan::Ramp) {
    let st = &styles::all()[si];
    let (ft, tt) = (st.mat(Slot::Floor).tile, st.mat(Slot::Trim).tile);
    let half_w = r.inr;
    let bottom = rp.y0.min(rp.y1) - 0.3;
    let n = ((rp.len / 0.75).ceil() as usize).max(2);
    let mut m = MeshData::default();
    let mut sides = MeshData::default();
    for i in 0..n {
        let (s0, s1) = (
            rp.len * i as f32 / n as f32,
            rp.len * (i + 1) as f32 / n as f32,
        );
        let (p0, d0) = geom::polyline_at(&rp.line, s0);
        let (p1, d1) = geom::polyline_at(&rp.line, s1);
        let (y0, y1) = (rp.at_s(s0), rp.at_s(s1));
        let (l0, l1) = (d0.perp() * half_w, d1.perp() * half_w);
        let q = [p0 - l0, p1 - l1, p1 + l1, p0 + l0];
        let ys = [y0, y1, y1, y0];
        let along = Vec3::new(p1.x - p0.x, y1 - y0, p1.y - p0.y);
        let nrm = along
            .cross(Vec3::new(l0.x, 0.0, l0.y))
            .normalize_or(Vec3::Y);
        let nrm = if nrm.y < 0.0 { -nrm } else { nrm };
        m.quad(
            [
                Vec3::new(q[0].x, ys[0], q[0].y),
                Vec3::new(q[1].x, ys[1], q[1].y),
                Vec3::new(q[2].x, ys[2], q[2].y),
                Vec3::new(q[3].x, ys[3], q[3].y),
            ],
            nrm,
            [q[0] / ft, q[1] / ft, q[2] / ft, q[3] / ft],
            [0.9; 4],
        );
        for (a, b, ya, yb, n2) in [
            (p0 - l0, p1 - l1, y0, y1, -d0.perp()),
            (p1 + l1, p0 + l0, y1, y0, d0.perp()),
        ] {
            sides.quad(
                [
                    Vec3::new(a.x, bottom, a.y),
                    Vec3::new(b.x, bottom, b.y),
                    Vec3::new(b.x, yb, b.y),
                    Vec3::new(a.x, ya, a.y),
                ],
                Vec3::new(n2.x, 0.0, n2.y),
                [
                    Vec2::new(a.x / tt, -bottom / tt),
                    Vec2::new(b.x / tt, -bottom / tt),
                    Vec2::new(b.x / tt, -yb / tt),
                    Vec2::new(a.x / tt, -ya / tt),
                ],
                [0.7, 0.7, 0.9, 0.9],
            );
        }
    }
    out.part(r.level, Layer::Main, si, Slot::Floor).append(m);
    out.part(r.level, Layer::Main, si, Slot::Trim).append(sides);
}

fn liquid(out: &mut Built, r: &CRoom) {
    if r.liquid == Liquid::None {
        return;
    }
    let y = match r.kind {
        RoomKind::Pool => r.level_y - 0.28,
        RoomKind::Pit | RoomKind::Chasm => r.level_y - r.depth.max(1.0) + 0.7,
        _ => return,
    };
    let mut m = MeshData::default();
    let g = |p: Vec2| r.inside(p) + 0.3;
    let h = |_: Vec2| y;
    fill(
        &mut m,
        r.lo - Vec2::splat(0.5),
        r.hi + Vec2::splat(0.5),
        0.5,
        &g,
        &h,
        true,
        true,
        4.0,
        false,
    );
    out.liquids.push(LiquidMesh {
        level: r.level,
        liquid: r.liquid,
        mesh: m,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeons::plan::*;

    fn build_plan(plan: &Plan) -> (Compiled, Built) {
        let c = Compiled::new(plan, &|_| (0.6, false));
        let b = build(&c, &|_| 0);
        (c, b)
    }

    #[test]
    fn a_room_and_corridor_make_walls_floors_ceilings_and_a_doorway() {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(8.0, 6.0)),
        ));
        plan.add(Room::new(
            0,
            RoomKind::Corridor,
            0,
            Shape::path(&[Vec2::new(6.0, 0.0), Vec2::new(30.0, 0.0)], 3.0),
        ));
        let (_, b) = build_plan(&plan);
        let wall = b
            .parts
            .iter()
            .filter(|(k, _)| k.slot == Slot::Wall)
            .map(|(_, m)| m.triangles())
            .sum::<usize>();
        let floor = b
            .parts
            .iter()
            .filter(|(k, _)| k.slot == Slot::Floor)
            .map(|(_, m)| m.triangles())
            .sum::<usize>();
        let ceil = b
            .parts
            .iter()
            .filter(|(k, _)| k.layer == Layer::Ceiling)
            .map(|(_, m)| m.triangles())
            .sum::<usize>();
        assert!(
            wall > 20 && floor > 4 && ceil > 4,
            "walls {wall}, floors {floor}, ceilings {ceil}"
        );
        assert_eq!(
            b.doorways.len(),
            1,
            "the corridor enters the room through one doorway: {:?}",
            b.doorways
        );
        let d = &b.doorways[0];
        assert!(
            (d.a.x - 8.0).abs() < 0.3 && (d.b.x - 8.0).abs() < 0.3,
            "on the room's east wall: {d:?}"
        );
        // no wall across the opening at head height
        let blocking = b
            .parts
            .iter()
            .filter(|(k, _)| k.slot == Slot::Wall || k.slot == Slot::WallLower)
            .any(|(_, m)| {
                m.idx.chunks(3).any(|t| {
                    let p: Vec<Vec3> = t
                        .iter()
                        .map(|i| Vec3::from_array(m.pos[*i as usize]))
                        .collect();
                    let c = (p[0] + p[1] + p[2]) / 3.0;
                    (c.x - 8.0).abs() < 0.05 && c.z.abs() < 1.0 && c.y > 0.5 && c.y < 3.0
                })
            });
        assert!(!blocking, "the doorway is open");
    }

    #[test]
    fn floors_dont_overlap() {
        // a corridor overlapping a room: each spot of floor drawn once
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(8.0, 6.0)),
        ));
        plan.add(Room::new(
            0,
            RoomKind::Corridor,
            0,
            Shape::path(&[Vec2::new(0.0, 0.0), Vec2::new(30.0, 0.0)], 3.0),
        ));
        let (_, b) = build_plan(&plan);
        let mut area = 0.0;
        for (k, m) in &b.parts {
            if k.slot != Slot::Floor {
                continue;
            }
            for t in m.idx.chunks(3) {
                let p: Vec<Vec3> = t
                    .iter()
                    .map(|i| Vec3::from_array(m.pos[*i as usize]))
                    .collect();
                area += (p[1] - p[0]).cross(p[2] - p[0]).length() * 0.5;
            }
        }
        // room 16x12 = 192, corridor outside the room: 22 m x 3 m + a half disc
        let want = 192.0 + 22.0 * 3.0 + std::f32::consts::PI * 1.5 * 1.5 * 0.5;
        assert!(
            (area - want).abs() < want * 0.04,
            "floor area {area} vs {want}"
        );
    }

    /// The surfaces the walker sees (front faces only: the dungeon's are culled from behind),
    /// binned on a coarse grid for casting rays.
    struct Faces {
        tris: Vec<[Vec3; 3]>,
        bins: HashMap<(i32, i32, i32), Vec<u32>>,
    }

    const BIN: f32 = 0.75;

    fn bin_of(p: Vec3) -> (i32, i32, i32) {
        (
            (p.x / BIN).floor() as i32,
            (p.y / BIN).floor() as i32,
            (p.z / BIN).floor() as i32,
        )
    }

    fn faces(b: &Built) -> Faces {
        let mut tris = Vec::new();
        let mut bins: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (k, m) in &b.parts {
            // (the cut-away's caps are not drawn while walking)
            if k.layer == Layer::Cap {
                continue;
            }
            for t in m.idx.chunks(3) {
                let p = [
                    Vec3::from_array(m.pos[t[0] as usize]),
                    Vec3::from_array(m.pos[t[1] as usize]),
                    Vec3::from_array(m.pos[t[2] as usize]),
                ];
                let (lo, hi) = (
                    bin_of(p[0].min(p[1]).min(p[2]) - Vec3::splat(0.1)),
                    bin_of(p[0].max(p[1]).max(p[2]) + Vec3::splat(0.1)),
                );
                let id = tris.len() as u32;
                tris.push(p);
                for x in lo.0..=hi.0 {
                    for y in lo.1..=hi.1 {
                        for z in lo.2..=hi.2 {
                            bins.entry((x, y, z)).or_default().push(id);
                        }
                    }
                }
            }
        }
        Faces { tris, bins }
    }

    /// Distance along `d` to the first triangle facing back at the ray, up to `reach`.
    fn first_hit(f: &Faces, o: Vec3, d: Vec3, reach: f32) -> Option<f32> {
        let mut best = f32::MAX;
        let mut seen: Vec<(i32, i32, i32)> = Vec::new();
        let mut s = 0.0;
        while s <= reach && s < best {
            let cell = bin_of(o + d * s);
            s += 0.1;
            if seen.last() == Some(&cell) {
                continue;
            }
            seen.push(cell);
            for &i in f.bins.get(&cell).map(|v| v.as_slice()).unwrap_or(&[]) {
                let t = &f.tris[i as usize];
                let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
                if e1.cross(e2).dot(d) >= 0.0 {
                    continue;
                }
                let p = d.cross(e2);
                let det = e1.dot(p);
                if det.abs() < 1e-12 {
                    continue;
                }
                let inv = 1.0 / det;
                let sv = o - t[0];
                let u = sv.dot(p) * inv;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = sv.cross(e1);
                let v = d.dot(q) * inv;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let tt = e2.dot(q) * inv;
                if tt > 1e-4 && tt < best {
                    best = tt;
                }
            }
        }
        (best <= reach).then_some(best)
    }

    /// Rays from `eye` (in the air) that pass out of the air into the rock and meet no surface
    /// facing them soon after: the sky shows through there. Where each left the air.
    fn leaks(c: &Compiled, f: &Faces, eye: Vec3, rays: usize) -> Vec<Vec3> {
        use rayon::prelude::*;
        (0..rays)
            .into_par_iter()
            .filter_map(|i| {
                let y = 1.0 - 2.0 * (i as f32 + 0.5) / rays as f32;
                let rr = (1.0 - y * y).max(0.0).sqrt();
                let a = i as f32 * 2.399_963;
                let d = Vec3::new(rr * a.cos(), y, rr * a.sin());
                let mut t = 0.0;
                while t < 16.0 && c.air(eye + d * t) {
                    t += 0.05;
                }
                if t >= 16.0 {
                    return None;
                }
                match first_hit(f, eye, d, t + 2.5) {
                    Some(_) => None,
                    None => Some(eye + d * t),
                }
            })
            .collect()
    }

    /// Corridors with pointed vaults running into a rock cavern (to its middle, as the layouts
    /// draw them): no sky between the cavern's dome and the vaults, nor beside the openings.
    #[test]
    fn corridors_into_a_cavern_leave_no_gaps() {
        let mut total = 0;
        for seed in [7u32, 1234, 99_001] {
            let mut plan = Plan {
                seed,
                ..Plan::default()
            };
            plan.add(Room::new(
                0,
                RoomKind::Cavern,
                0,
                Shape::Round {
                    c: [0.0, 0.0],
                    r: 8.5,
                },
            ));
            for k in 0..5 {
                let dir = Vec2::from_angle(0.3 + k as f32 * 1.25);
                let mut cor = Room::new(
                    0,
                    RoomKind::Corridor,
                    0,
                    Shape::path(&[Vec2::ZERO, dir * 22.0], 4.5),
                );
                cor.height = 4.6;
                cor.ceiling = Ceiling::Pointed;
                plan.add(cor);
            }
            let (c, b) = build_plan(&plan);
            let f = faces(&b);
            for k in 0..9 {
                let e = if k == 0 {
                    Vec2::ZERO
                } else {
                    Vec2::from_angle(k as f32 * 0.7) * 4.5
                };
                for hgt in [1.7f32, 3.6] {
                    let eye = Vec3::new(e.x, c.rooms[0].floor_at(e) + hgt, e.y);
                    if c.air(eye) {
                        let l = leaks(&c, &f, eye, 2500);
                        if !l.is_empty() {
                            println!(
                                "seed {seed} eye {eye}: {} leaks, e.g. {:?}",
                                l.len(),
                                &l[..l.len().min(4)]
                            );
                        }
                        total += l.len();
                    }
                }
            }
        }
        // (of 135000 rays; the seams used to let about 1250 through, now a few dozen graze
        // hairline cracks at the floor's edge)
        assert!(
            total < 250,
            "{total} rays escape through gaps at the junctions"
        );
    }
}
