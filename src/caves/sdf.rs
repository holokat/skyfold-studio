//! The rock around caves as a signed distance field: negative inside rock, positive in the air.
//!
//! The field is the terrain (solid under its surface, exactly the terrain mesh's triangles sunk
//! `SINK` below them), joined with added rock (outcrops round cave mouths, arches, scree
//! ramps), minus everything carved (tunnels with their cross-section profiles, chambers, pits,
//! skylight shafts, pools, dug strokes), plus formations hanging into the carved space
//! (stalactites, stalagmites, columns, rubble). Carved and added rock get noise that looks like
//! stone: big lumps, a broken mid scale, ridged edges and (limestone, sandstone) bedding-plane
//! ledges; floors stay walkable.
//!
//! `compile` turns the saved recipes (`model::Caves`) into shapes; `Volume::eval` samples the
//! field. Everything is deterministic and thread-safe (`Volume` is shared with the mesher).

use bevy::math::{IVec2, Vec2, Vec3, Vec3Swizzles};
use std::collections::HashSet;

use super::model::*;
use super::noise3::{fbm, gnoise, ridged, warp};
use crate::noise::Rng;

/// The cave's copy of the ground sits this far under the terrain mesh, so where both are drawn
/// (round a cave mouth) the terrain covers it.
pub const SINK: f32 = 0.06;
/// No owner (the terrain itself).
pub const NONE: u16 = u16::MAX;

// ------------------------------------------------------------------ ground

/// A copy of part of the terrain's heights, read exactly as the terrain mesh triangulates them.
#[derive(Clone, Debug)]
pub struct Ground {
    /// first vertex (i, j) of the copy and its vertices per side
    i0: i32,
    j0: i32,
    w: i32,
    h: i32,
    res: i32,
    cell: f32,
    half: f32,
    heights: Vec<f32>,
}

impl Ground {
    /// Copies the heights over `lo`..`hi` (world x, z) plus a margin.
    pub fn copy(t: &crate::terrain::Terrain, lo: Vec2, hi: Vec2) -> Ground {
        let g0 = t.world_to_grid(lo.x, lo.y);
        let g1 = t.world_to_grid(hi.x, hi.y);
        let res = t.res as i32;
        let i0 = (g0.x.floor() as i32 - 2).clamp(0, res);
        let j0 = (g0.y.floor() as i32 - 2).clamp(0, res);
        let i1 = (g1.x.ceil() as i32 + 2).clamp(0, res);
        let j1 = (g1.y.ceil() as i32 + 2).clamp(0, res);
        let (w, h) = (i1 - i0 + 1, j1 - j0 + 1);
        let mut heights = Vec::with_capacity((w * h) as usize);
        for j in j0..=j1 {
            for i in i0..=i1 {
                heights.push(t.h(i as usize, j as usize));
            }
        }
        Ground {
            i0,
            j0,
            w,
            h,
            res,
            cell: t.cell,
            half: t.half(),
            heights,
        }
    }

    #[inline]
    fn at(&self, i: i32, j: i32) -> f32 {
        let li = (i - self.i0).clamp(0, self.w - 1);
        let lj = (j - self.j0).clamp(0, self.h - 1);
        self.heights[(lj * self.w + li) as usize]
    }

    /// Height of the terrain mesh at (x, z): the triangle the point falls in (the mesh alternates
    /// its diagonals like `terrain::mesh::build_patch`).
    #[inline]
    pub fn h(&self, x: f32, z: f32) -> f32 {
        let max = self.res as f32;
        let gx = ((x + self.half) / self.cell).clamp(0.0, max);
        let gz = ((z + self.half) / self.cell).clamp(0.0, max);
        let i = (gx.floor() as i32).min(self.res - 1);
        let j = (gz.floor() as i32).min(self.res - 1);
        let (fx, fz) = (gx - i as f32, gz - j as f32);
        let (h00, h10, h01, h11) = (
            self.at(i, j),
            self.at(i + 1, j),
            self.at(i, j + 1),
            self.at(i + 1, j + 1),
        );
        if (i + j) % 2 == 0 {
            if fx + fz <= 1.0 {
                h00 + (h10 - h00) * fx + (h01 - h00) * fz
            } else {
                h11 + (h01 - h11) * (1.0 - fx) + (h10 - h11) * (1.0 - fz)
            }
        } else if fx >= fz {
            h00 + (h10 - h00) * fx + (h11 - h10) * fz
        } else {
            h00 + (h01 - h00) * fz + (h11 - h01) * fx
        }
    }

    /// The terrain cell (i, j) under (x, z).
    pub fn cell_of(&self, x: f32, z: f32) -> IVec2 {
        let max = self.res - 1;
        IVec2::new(
            (((x + self.half) / self.cell).floor() as i32).clamp(0, max),
            (((z + self.half) / self.cell).floor() as i32).clamp(0, max),
        )
    }
    /// World x, z of the corner of cell (i, j).
    pub fn cell_corner(&self, c: IVec2) -> Vec2 {
        Vec2::new(
            c.x as f32 * self.cell - self.half,
            c.y as f32 * self.cell - self.half,
        )
    }
    pub fn cell(&self) -> f32 {
        self.cell
    }
    /// A cheap fingerprint of the heights over a rectangle of cells (chunk cache keys).
    pub fn hash_rect(&self, c0: IVec2, c1: IVec2) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for j in c0.y..=c1.y + 1 {
            for i in c0.x..=c1.x + 1 {
                h = (h ^ self.at(i, j).to_bits() as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        h
    }
}

// ------------------------------------------------------------------ shapes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// adds rock before carving (outcrops, arches)
    Add,
    /// removes rock (tunnels, chambers, pits, shafts)
    Carve,
    /// adds rock after carving (formations, rubble, ramps)
    Decor,
    /// a freehand dig stroke (applied in order after everything else)
    Dig,
    /// a freehand fill stroke
    Fill,
}

/// How the rock's noise looks (per theme).
#[derive(Clone, Copy, Debug)]
pub struct Rock {
    /// big lumps (m)
    pub big: f32,
    /// broken mid-scale relief (m)
    pub mid: f32,
    /// sharp ridged edges (m)
    pub ridge: f32,
    /// bedding-plane ledges: depth (m) and spacing (m)
    pub strata: f32,
    pub strata_gap: f32,
    /// smooth scallops (water-worn), 0..1 of `mid`
    pub smooth: f32,
}

impl Rock {
    pub fn for_theme(t: Theme) -> Rock {
        match t {
            Theme::Limestone | Theme::Auto => Rock {
                big: 1.3,
                mid: 0.45,
                ridge: 0.18,
                strata: 0.32,
                strata_gap: 1.35,
                smooth: 0.5,
            },
            Theme::Ice => Rock {
                big: 1.0,
                mid: 0.3,
                ridge: 0.05,
                strata: 0.0,
                strata_gap: 1.0,
                smooth: 0.9,
            },
            Theme::Mossy => Rock {
                big: 1.4,
                mid: 0.5,
                ridge: 0.2,
                strata: 0.15,
                strata_gap: 1.6,
                smooth: 0.3,
            },
            Theme::Sandstone => Rock {
                big: 1.1,
                mid: 0.3,
                ridge: 0.08,
                strata: 0.45,
                strata_gap: 0.9,
                smooth: 0.7,
            },
            Theme::Volcanic => Rock {
                big: 0.7,
                mid: 0.22,
                ridge: 0.25,
                strata: 0.0,
                strata_gap: 1.0,
                smooth: 0.6,
            },
            Theme::Crystal => Rock {
                big: 1.2,
                mid: 0.4,
                ridge: 0.3,
                strata: 0.1,
                strata_gap: 1.8,
                smooth: 0.2,
            },
            Theme::Mine => Rock {
                big: 0.25,
                mid: 0.22,
                ridge: 0.3,
                strata: 0.0,
                strata_gap: 1.0,
                smooth: 0.0,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum Prim {
    /// a tunnel along a floor centreline (cumulative length per point for the width's breathing)
    /// (`calm`: the relief fades out towards the start / end, where a tunnel opens outside)
    Tube {
        pts: Vec<Vec3>,
        along: Vec<f32>,
        w: f32,
        h: f32,
        profile: Profile,
        phase: f32,
        calm: (bool, bool),
    },
    /// an ellipsoid chamber lobe: centre, radii, turned by (sin, cos) of its yaw, cut flat at
    /// its floor
    Room {
        c: Vec3,
        r: Vec3,
        sc: (f32, f32),
        floor: f32,
    },
    /// an open pit: floor centre, top height, radius at the floor and at the top
    Pit { c: Vec3, top: f32, r0: f32, r1: f32 },
    /// a round vertical-ish shaft
    Shaft { a: Vec3, b: Vec3, r: f32 },
    /// a sphere swept from a to b
    Capsule { a: Vec3, b: Vec3, r: f32 },
    /// an ellipsoid of rock
    Blob { c: Vec3, r: Vec3, sc: (f32, f32) },
    /// a rounded block of rock (superellipsoid, `n` > 2 squarer): crags, cliff faces
    Block {
        c: Vec3,
        r: Vec3,
        sc: (f32, f32),
        n: f32,
    },
    /// a walkable slope of scree from `a` (top) to `b` (bottom), `w` wide
    Ramp { a: Vec3, b: Vec3, w: f32 },
    /// a band of rock along a curve (arches): thickness (in the curve's plane) per point, width
    /// across it (`across`: the horizontal unit vector across the span)
    Band {
        pts: Vec<Vec3>,
        thick: Vec<f32>,
        w: f32,
        across: Vec3,
    },
    /// a cone from `base` along `u`: stalactite, stalagmite (or an hourglass column)
    Spike {
        base: Vec3,
        u: Vec3,
        len: f32,
        r: f32,
        column: bool,
    },
    /// rock under a walkway's floor line where it crosses open space (a ramp into a chamber, a
    /// ledge, a bridge): its top is the floor line; low down a scree bank to the floor under it
    /// (`base`), higher up a slab with an arched underside. `lean`: per point, how far the rock
    /// reaches out sideways (towards a wall it leans on)
    /// (`pts`: the floor line, its relief baked in; `base`: the floor under each point)
    Causeway {
        pts: Vec<Vec3>,
        w: f32,
        base: Vec<f32>,
        lean: Vec<Vec2>,
    },
}

/// How deep the rock under a walkway goes at height `hgt` over the floor below it: down to the
/// floor while low (a bank), a slab once high (the underside arching up into it).
#[inline]
fn causeway_depth(hgt: f32) -> f32 {
    if hgt < 3.5 {
        hgt + 0.6
    } else {
        (4.1 - (hgt - 3.5) * 0.6).max(2.6)
    }
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub prim: Prim,
    pub op: Op,
    /// bounds including the noise margin
    pub lo: Vec3,
    pub hi: Vec3,
    /// noise strength (0..1.5) and its character
    pub rough: f32,
    pub rock: Rock,
    /// floor height of carved shapes (walls get the full noise, the floor stays walkable)
    pub floor: f32,
    /// index of the owning cave in `Caves::list` (`NONE` for dig strokes)
    pub owner: u16,
}

#[inline]
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

#[inline]
fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Approximate distance to an ellipse (radii a, b) — Inigo Quilez's fast bound.
#[inline]
fn ellipse(x: f32, y: f32, a: f32, b: f32) -> f32 {
    let k0 = ((x / a).powi(2) + (y / b).powi(2)).sqrt();
    let k1 = ((x / (a * a)).powi(2) + (y / (b * b)).powi(2)).sqrt();
    if k1 < 1e-6 {
        -a.min(b)
    } else {
        k0 * (k0 - 1.0) / k1
    }
}

/// Approximate distance to a superellipse |x/a|^n + |y/b|^n = 1 (n > 2: squarer).
#[inline]
fn superellipse(x: f32, y: f32, a: f32, b: f32, n: f32) -> f32 {
    let k = ((x / a).abs().powf(n) + (y / b).abs().powf(n)).powf(1.0 / n);
    (k - 1.0) * a.min(b)
}

#[inline]
fn ellipsoid(q: Vec3, r: Vec3) -> f32 {
    let k0 = (q / r).length();
    let k1 = (q / (r * r)).length();
    if k1 < 1e-6 {
        -r.min_element()
    } else {
        k0 * (k0 - 1.0) / k1
    }
}

/// Gentle humps and hollows in cave floors (m, about ±0.9): the same everywhere, so a chamber's
/// floor and the tunnels leaving it agree.
#[inline]
pub fn relief(x: f32, z: f32) -> f32 {
    gnoise(Vec3::new(x * 0.055, 0.0, z * 0.055), 211) * 0.75
        + gnoise(Vec3::new(x * 0.17, 3.1, z * 0.17), 223) * 0.22
}

/// Cross-section of a tunnel at lateral offset `x` (≥ 0) and height `y` above its floor.
#[inline]
fn profile(p: Profile, x: f32, y: f32, w: f32, h: f32) -> f32 {
    let d = match p {
        Profile::Tube => ellipse(x, y - 0.42 * h, 0.5 * w, 0.58 * h),
        // (both stay walkable-wide down at the floor: no slot narrower than ~0.6 of the width)
        Profile::Canyon => superellipse(x, y - 0.44 * h, 0.42 * w, 0.6 * h, 3.2),
        Profile::Keyhole => {
            let top = ellipse(x, y - 0.68 * h, 0.5 * w, 0.34 * h);
            let slot = superellipse(x, y - 0.32 * h, 0.34 * w, 0.42 * h, 3.2);
            smin(top, slot, 0.5)
        }
        Profile::Low => superellipse(x, y - 0.42 * h, 0.5 * w, 0.58 * h, 3.6),
        Profile::Drift => {
            // a squarish drift with a slightly arched roof
            let arch = 0.12 * h * (1.0 - (x / (0.5 * w)).clamp(0.0, 1.0).powi(2));
            superellipse(x, y - 0.5 * (h + arch), 0.5 * w, 0.5 * (h + arch), 5.0)
        }
    };
    d.max(-y)
}

/// The clear walking corridor kept through every tunnel, whatever the rock's noise does: width
/// and height (m) over the floor line.
pub const WALK_W: f32 = 2.6;
pub const WALK_H: f32 = 2.85;

/// The walking corridor's cross-section at lateral offset `x` and height `y` over the floor.
#[inline]
fn walk_core(x: f32, y: f32) -> f32 {
    superellipse(x, y - WALK_H * 0.5, WALK_W * 0.5, WALK_H * 0.5 + 0.05, 3.0).max(-y)
}

impl Prim {
    /// Distance (negative inside the shape).
    fn dist(&self, p: Vec3) -> f32 {
        self.dist_floor(p).0
    }

    /// Distance and the height of the shape's floor under `p` (NaN: no floor).
    fn dist_floor(&self, p: Vec3) -> (f32, f32) {
        let (d, f, _) = self.dist_floor_core(p);
        (d, f)
    }

    /// Distance, floor height and (tunnels) the distance to the clear walking corridor inside
    /// it (`f32::MAX` for other shapes).
    fn dist_floor_core(&self, p: Vec3) -> (f32, f32, f32) {
        match self {
            Prim::Tube {
                pts,
                along,
                w,
                h,
                profile: pf,
                phase,
                calm,
            } => {
                // the cross-section stood upright along the floor line in plan: the nearest
                // point of the line (seen from above) gives the floor under `p` and how far to the
                // side it is — so a ramp's floor runs on smoothly through its joints (a sloping
                // capsule's rounded end would cut a notch into the floor above it) and its roof
                // stays a full height over the floor
                let total = *along.last().unwrap_or(&0.0);
                let p2 = Vec2::new(p.x, p.z);
                let mut near = (f32::MAX, 0usize, 0.0f32);
                for k in 0..pts.len() - 1 {
                    let (a2, b2) = (
                        Vec2::new(pts[k].x, pts[k].z),
                        Vec2::new(pts[k + 1].x, pts[k + 1].z),
                    );
                    let ab = b2 - a2;
                    let t = ((p2 - a2).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let l = (p2 - (a2 + ab * t)).length_squared();
                    if l < near.0 {
                        near = (l, k, t);
                    }
                }
                let (l, k, t) = near;
                let hz = l.sqrt();
                let line = pts[k].y + (pts[k + 1].y - pts[k].y) * t;
                let vy = p.y - line;
                // quick reject: far from the tunnel (a lower bound that keeps the sign: the shape
                // is within 0.65 w sideways and 0..1.15 h up)
                if hz > *w * 1.5 + 2.0 || vy > *h * 1.5 + 2.0 || vy < -3.0 {
                    let d = (hz - *w * 0.65).max(vy - *h * 1.15).max(-vy);
                    let core = (hz - WALK_W * 0.5).max(vy - WALK_H).max(-vy);
                    return (d, line, core);
                }
                // the tunnel breathes: wider and lower in places
                let s = along[k] + (along[k + 1] - along[k]) * t;
                let breath = (s * 0.11 + phase).sin() * 0.5 + (s * 0.29 + phase * 2.3).sin() * 0.5;
                let ww = w * (1.0 + 0.18 * breath);
                let hh = h * (1.0 - 0.1 * breath);
                let mut kc = 1.0;
                if calm.0 {
                    kc *= smoothstep(4.0, 14.0, s);
                }
                if calm.1 {
                    kc *= smoothstep(4.0, 14.0, total - s);
                }
                let rel = relief(p.x, p.z) * kc;
                (
                    profile(*pf, hz, vy - rel, ww, hh),
                    line + rel,
                    walk_core(hz, vy - rel),
                )
            }
            Prim::Room { c, r, sc, floor } => {
                let d = p - *c;
                let q = Vec3::new(d.x * sc.1 - d.z * sc.0, d.y, d.x * sc.0 + d.z * sc.1);
                let fl = floor + relief(p.x, p.z);
                (ellipsoid(q, *r).max(fl - p.y), fl, f32::MAX)
            }
            Prim::Pit { c, top, r0, r1 } => {
                let y = p.y - c.y;
                let k = (y / (top - c.y).max(1.0)).clamp(0.0, 1.0);
                let r = r0 + (r1 - r0) * k * k;
                let radial = Vec2::new(p.x - c.x, p.z - c.z).length();
                let rel = relief(p.x, p.z) * 0.6;
                ((radial - r).max(rel - y), c.y + rel, f32::MAX)
            }
            Prim::Shaft { a, b, r } | Prim::Capsule { a, b, r } => {
                let ab = *b - *a;
                let t = ((p - *a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                ((p - (*a + ab * t)).length() - r, f32::NAN, f32::MAX)
            }
            Prim::Blob { c, r, sc } => {
                let d = p - *c;
                let q = Vec3::new(d.x * sc.1 - d.z * sc.0, d.y, d.x * sc.0 + d.z * sc.1);
                (ellipsoid(q, *r), f32::NAN, f32::MAX)
            }
            Prim::Block { c, r, sc, n } => {
                let d = p - *c;
                let q = Vec3::new(d.x * sc.1 - d.z * sc.0, d.y, d.x * sc.0 + d.z * sc.1);
                let k = ((q.x / r.x).abs().powf(*n)
                    + (q.y / r.y).abs().powf(*n)
                    + (q.z / r.z).abs().powf(*n))
                .powf(1.0 / n);
                ((k - 1.0) * r.min_element(), f32::NAN, f32::MAX)
            }
            Prim::Ramp { a, b, w } => {
                let ab = Vec2::new(b.x - a.x, b.z - a.z);
                let len = ab.length().max(0.1);
                let u = ab / len;
                let q = Vec2::new(p.x - a.x, p.z - a.z);
                let s = q.dot(u);
                let lat = (q.x * -u.y + q.y * u.x).abs();
                // a convex scree cone: highest along its middle line
                let surf =
                    a.y + (b.y - a.y) * (s / len).clamp(0.0, 1.0) - (lat / (w * 0.5)).powi(2) * 0.8;
                let d_top = p.y - surf;
                let d_side = lat - w * 0.5;
                let d_end = (-s - 1.5).max(s - len - 1.5);
                (d_top.max(d_side * 0.7).max(d_end), f32::NAN, f32::MAX)
            }
            Prim::Band {
                pts,
                thick,
                w,
                across,
            } => {
                let mut best = f32::MAX;
                for k in 0..pts.len() - 1 {
                    let (a, b) = (pts[k], pts[k + 1]);
                    let ab = b - a;
                    let l2 = ab.length_squared().max(1e-6);
                    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
                    let v = p - (a + ab * t);
                    // the cross-section swept round the curve: its width across the span, its
                    // thickness in the curve's plane (all of the offset in that plane, so the
                    // band stays whole round bends and caps its ends)
                    let x = v.dot(*across);
                    let y = (v - *across * x).length();
                    let th = thick[k] + (thick[k + 1] - thick[k]) * t;
                    let d = ellipse(x, y, *w * 0.5, th * 0.5);
                    best = best.min(d);
                }
                (best, f32::NAN, f32::MAX)
            }
            Prim::Spike {
                base,
                u,
                len,
                r,
                column,
            } => {
                let q = p - *base;
                let s = q.dot(*u);
                let radial = (q - *u * s).length();
                if *column {
                    let k = (s / len).clamp(0.0, 1.0);
                    let rr = r
                        * (0.42 + 0.58 * (2.0 * k - 1.0).abs().powf(1.6))
                        * (1.0 + 0.06 * (s * 5.3).sin());
                    (
                        (radial - rr).max(-s).max(s - len) * 0.85,
                        f32::NAN,
                        f32::MAX,
                    )
                } else if s < 0.0 {
                    ((radial - r).max(-s - 2.0), f32::NAN, f32::MAX)
                } else {
                    let k = (s / len).min(1.0);
                    // concave taper to a drip tip, with drip rings
                    let rr = r * (1.0 - k).powf(1.3) * (1.0 + 0.07 * (s * 6.1).sin()) + 0.03;
                    (
                        if s > *len {
                            (p - (*base + *u * *len)).length() - 0.03
                        } else {
                            (radial - rr) * 0.8
                        },
                        f32::NAN,
                        f32::MAX,
                    )
                }
            }
            Prim::Causeway { pts, w, base, lean } => {
                // (the floor value returned is the walkway's top over `p`: the noise spares it)
                // a ribbon: the nearest point of the run's line (in plan) sets the top over `p`
                // (not the nearest segment by distance: a higher one's end would poke out over
                // the lower floor it runs on to)
                let p2 = Vec2::new(p.x, p.z);
                let last = pts.len() - 2;
                let mut near = (f32::MAX, 0usize, 0.0f32, 0.0f32, 0.0f32);
                for k in 0..=last {
                    let (a2, b2) = (
                        Vec2::new(pts[k].x, pts[k].z),
                        Vec2::new(pts[k + 1].x, pts[k + 1].z),
                    );
                    let ab = b2 - a2;
                    let len = ab.length().max(1e-3);
                    let tu = (p2 - a2).dot(ab) / (len * len);
                    let t = tu.clamp(0.0, 1.0);
                    let sh = lean[k].lerp(lean[k + 1], t);
                    let off = p2 - (a2 + ab * t) - sh * 0.5;
                    // sideways and (at the run's two ends) past the end
                    let past = if (k == 0 && tu < 0.0) || (k == last && tu > 1.0) {
                        (tu - t).abs() * len
                    } else {
                        0.0
                    };
                    let lat = (off.length_squared() - past * past).max(0.0).sqrt();
                    if off.length() < near.0 {
                        near = (off.length(), k, t, lat, past);
                    }
                }
                let (_, k, t, lat, past) = near;
                let (a, b) = (pts[k], pts[k + 1]);
                let sh = lean[k].lerp(lean[k + 1], t);
                let top = a.y + (b.y - a.y) * t - 0.03;
                let hgt = top - (base[k] + (base[k + 1] - base[k]) * t);
                let z = top - p.y;
                let k_side = 0.85 + (0.3 - 0.85) * smoothstep(3.0, 5.5, hgt);
                let halfw = w * 0.5 + sh.length() * 0.5 + z.max(0.0) * k_side;
                // (the bank flares out sideways as it goes down; its ends stay blunt, so a run
                // never spreads on over the floor that carries on past it)
                let d = (p.y - top)
                    .max((lat - halfw) * 0.75)
                    .max(past - 0.3)
                    .max((top - causeway_depth(hgt)) - p.y);
                (d, top, f32::MAX)
            }
        }
    }
}

/// What the field is at a point.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// the field (negative in rock)
    pub d: f32,
    /// the terrain term alone (height above the sunk terrain surface)
    pub ground: f32,
    /// the cave whose shape is nearest (`NONE`: none near)
    pub owner: u16,
}

/// The shapes of every cave plus the ground they cut into.
pub struct Volume {
    pub shapes: Vec<Shape>,
    pub ground: Ground,
    /// themes of the caves (by index in `Caves::list`)
    pub themes: Vec<Theme>,
    /// which caves are giant (meshed on the coarse grid)
    pub giant: Vec<bool>,
    /// terrain cells (i, j) whose triangles are taken out (the carving breaks the surface there)
    pub holes: HashSet<IVec2>,
}

/// Noise on carved walls: big lumps, broken relief, ridged edges, bedding-plane ledges; floors
/// get only gentle bumps.
#[inline]
fn carve_noise(p: Vec3, floor: f32, rough: f32, r: &Rock) -> f32 {
    let above = if floor.is_finite() { p.y - floor } else { 10.0 };
    let wall = smoothstep(-0.2, 1.8, above);
    let wv = warp(p * 0.045, 71) * 3.5;
    let q = p + wv;
    let big = fbm(q * 0.1, 2, 11) * r.big;
    let mid_raw = fbm(p * 0.38, 2, 23);
    // water-worn scallops: the mid relief rounded off
    let mid = (mid_raw * (1.0 - r.smooth) + (mid_raw.abs() * 2.0 - 0.6) * r.smooth * 0.6) * r.mid;
    let ridge = if r.ridge > 0.0 {
        (ridged(p * 0.7, 2, 31) - 0.55) * r.ridge
    } else {
        0.0
    };
    // (floors keep only a little of it: walkable, no ledges)
    let mut n = (big + mid + ridge) * rough * (0.07 + 0.93 * wall);
    if r.strata > 0.0 {
        // bedding planes: thin recesses every `strata_gap`, wandering a little
        let s = (p.y + wv.y * 0.4 + gnoise(p * 0.07, 43) * 0.6) / r.strata_gap;
        let f = s - s.floor();
        let groove = smoothstep(0.18, 0.0, (f - 0.5).abs() - 0.32);
        n -= groove * r.strata * wall * rough;
    }
    n -= joints(p, 2.8, 103) * 0.14 * wall * rough * (1.0 - r.smooth * 0.6);
    // floors: gentle bumps only
    n + (1.0 - wall) * fbm(p * 0.22, 2, 57) * 0.18
}

/// Joints: two sets of near-vertical cracks and the bedding planes between them (1 in a joint),
/// wandering a little, so rock breaks into blocks.
#[inline]
fn joints(p: Vec3, gap: f32, seed: u32) -> f32 {
    let q = p + warp(p * 0.05, seed) * 2.2;
    let a = q.x * 0.94 + q.z * 0.34;
    let b = -q.x * 0.34 + q.z * 0.94;
    let g = |v: f32, gap: f32| {
        let f = v / gap;
        let f = f - f.floor();
        smoothstep(0.1, 0.0, (f - 0.5).abs() - 0.42)
    };
    (g(a, gap * 1.4) + g(b, gap * 1.9) + g(q.y + gnoise(q * 0.1, seed ^ 7) * 0.8, gap * 0.6))
        .min(1.0)
}

/// Noise on added rock (crags, arches): lumps, broken relief, ridges, ledges and joints.
#[inline]
fn add_noise(p: Vec3, rough: f32, r: &Rock) -> f32 {
    let wv = warp(p * 0.06, 91) * 2.5;
    let mut n = fbm((p + wv) * 0.14, 2, 93) * (r.big * 0.8 + 0.3)
        + (ridged(p * 0.42, 2, 97) - 0.55) * (r.ridge * 2.0 + 0.3)
        + fbm(p * 0.9, 1, 99) * 0.12;
    if r.strata > 0.0 {
        let s = (p.y + wv.y * 0.4 + gnoise(p * 0.07, 43) * 0.6) / (r.strata_gap * 1.6);
        let f = s - s.floor();
        n += smoothstep(0.12, 0.0, (f - 0.5).abs() - 0.38) * r.strata * 0.9;
    }
    n += joints(p, 3.2, 101) * 0.35;
    n * rough
}

/// Most noise any shape can add (m): bounds and culling margins.
pub const NOISE_REACH: f32 = 2.6;

impl Volume {
    /// The field at `p` from the shapes `idx` (those near `p`; see `shapes_near`).
    pub fn eval(&self, p: Vec3, idx: &[u32]) -> Sample {
        let h = self.ground.h(p.x, p.z);
        let ground = p.y
            - (h - if (p.y - h).abs() < 1.5 {
                self.sink_at(p.x, p.z)
            } else {
                SINK
            });
        let mut solid = ground;
        let mut owner = NONE;
        let mut own_d = f32::MAX;
        // added rock (blended into the ground)
        for &i in idx {
            let s = &self.shapes[i as usize];
            if s.op != Op::Add {
                continue;
            }
            let mut d = s.prim.dist(p);
            if d < NOISE_REACH {
                d += add_noise(p, s.rough, &s.rock);
            }
            if d.abs() < own_d {
                own_d = d.abs();
                owner = s.owner;
            }
            solid = smin(solid, d, 1.2);
        }
        // carved space, then its stone noise: walls join smoothly, floors stay exactly flat (a
        // smooth union of several shapes sharing a floor would sag it)
        let mut hard = f32::MAX;
        let mut soft = f32::MAX;
        let mut floor = f32::NAN;
        let mut floor_near = f32::NAN;
        let mut rough = 0.0;
        let mut rock = None;
        let mut core = f32::MAX;
        for &i in idx {
            let s = &self.shapes[i as usize];
            if s.op != Op::Carve {
                continue;
            }
            let (d, fl, c) = s.prim.dist_floor_core(p);
            core = core.min(c);
            if d < hard {
                floor = fl;
                rough = s.rough;
                rock = Some(s.rock);
                if d.abs() < own_d {
                    own_d = d.abs();
                    owner = s.owner;
                }
            }
            if d < 2.0 && fl.is_finite() {
                floor_near = if floor_near.is_nan() {
                    fl
                } else {
                    floor_near.max(fl)
                };
            }
            soft = if hard == f32::MAX {
                d
            } else {
                smin(soft, d, 1.4)
            };
            hard = hard.min(d);
        }
        if let Some(r) = rock {
            let w = if floor_near.is_finite() {
                smoothstep(floor_near - 0.3, floor_near + 1.3, p.y)
            } else {
                1.0
            };
            let mut carve = hard + (soft - hard) * w;
            if carve < NOISE_REACH + 0.5 {
                carve += carve_noise(p, floor, rough, &r);
            }
            // (the noise never closes a tunnel's walking corridor)
            if core < 1.0 {
                carve = smin(carve, core, 0.35);
            }
            solid = smax(solid, -carve, 0.45);
        }
        // formations and scree (they grow out of the rock into the carved space)
        for &i in idx {
            let s = &self.shapes[i as usize];
            if s.op != Op::Decor {
                continue;
            }
            let (mut d, top, _) = s.prim.dist_floor_core(p);
            if d < 1.0 {
                d += match s.prim {
                    Prim::Ramp { .. }
                    | Prim::Blob { .. }
                    | Prim::Block { .. }
                    | Prim::Band { .. } => add_noise(p, s.rough * 0.45, &s.rock),
                    // (a pool's dam: flowstone, gently lumpy)
                    Prim::Capsule { .. } => fbm(p * 1.3, 2, 67) * 0.12 * s.rough,
                    // (rough sides and underside, a walkable top)
                    Prim::Causeway { .. } => {
                        add_noise(p, s.rough * 0.5, &s.rock) * smoothstep(0.2, 1.0, top - p.y)
                            + gnoise(p * 1.7, 63) * 0.04
                    }
                    _ => gnoise(p * 2.3, 61) * 0.05 * s.rough,
                };
            }
            if d < own_d {
                own_d = d.abs();
                owner = s.owner;
            }
            solid = smin(solid, d, 0.35);
        }
        // freehand strokes, in order
        for &i in idx {
            let s = &self.shapes[i as usize];
            match s.op {
                Op::Dig => {
                    let mut d = s.prim.dist(p);
                    if d < 1.5 {
                        d += fbm(p * 0.5, 2, 13) * 0.3 * s.rough;
                    }
                    solid = smax(solid, -d, 0.3);
                }
                Op::Fill => {
                    let mut d = s.prim.dist(p);
                    if d < 1.5 {
                        d += add_noise(p, s.rough * 0.4, &s.rock);
                    }
                    solid = smin(solid, d, 0.4);
                }
                _ => {}
            }
        }
        Sample {
            d: solid,
            ground,
            owner,
        }
    }

    /// How far the cave's copy of the ground sits under the terrain at (x, z): exactly on it in
    /// a hole (the terrain is gone there, so the two meet without a step at its edge), sinking
    /// to `SINK` within a metre outside it, where the terrain covers it.
    #[inline]
    pub fn sink_at(&self, x: f32, z: f32) -> f32 {
        if self.holes.is_empty() {
            return SINK;
        }
        let c = self.ground.cell_of(x, z);
        if self.holes.contains(&c) {
            return 0.0;
        }
        let cell = self.ground.cell();
        let mut near = f32::MAX;
        for dj in -1..=1 {
            for di in -1..=1 {
                let n = c + IVec2::new(di, dj);
                if (di != 0 || dj != 0) && self.holes.contains(&n) {
                    let o = self.ground.cell_corner(n);
                    let dx = (o.x - x).max(x - (o.x + cell)).max(0.0);
                    let dz = (o.y - z).max(z - (o.y + cell)).max(0.0);
                    near = near.min((dx * dx + dz * dz).sqrt());
                }
            }
        }
        if near == f32::MAX {
            SINK
        } else {
            SINK * smoothstep(0.0, 1.0, near)
        }
    }

    /// Indices of the shapes whose bounds come within `margin` of the box `lo`..`hi`.
    pub fn shapes_near(&self, lo: Vec3, hi: Vec3, margin: f32) -> Vec<u32> {
        self.shapes
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.lo.x <= hi.x + margin
                    && s.hi.x >= lo.x - margin
                    && s.lo.y <= hi.y + margin
                    && s.hi.y >= lo.y - margin
                    && s.lo.z <= hi.z + margin
                    && s.hi.z >= lo.z - margin
            })
            .map(|(i, _)| i as u32)
            .collect()
    }

    /// The field at one point (finds the shapes itself: slow, for probes and physics).
    pub fn at(&self, p: Vec3) -> Sample {
        let idx = self.shapes_near(p, p, 0.5);
        self.eval(p, &idx)
    }

    /// Gradient (outward normal) by central differences.
    pub fn normal(&self, p: Vec3, idx: &[u32], e: f32) -> Vec3 {
        let dx = self.eval(p + Vec3::X * e, idx).d - self.eval(p - Vec3::X * e, idx).d;
        let dy = self.eval(p + Vec3::Y * e, idx).d - self.eval(p - Vec3::Y * e, idx).d;
        let dz = self.eval(p + Vec3::Z * e, idx).d - self.eval(p - Vec3::Z * e, idx).d;
        Vec3::new(dx, dy, dz).normalize_or(Vec3::Y)
    }

    /// Bounds of all shapes.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.shapes.iter();
        let f = it.next()?;
        Some(it.fold((f.lo, f.hi), |(lo, hi), s| (lo.min(s.lo), hi.max(s.hi))))
    }

    /// Is anything carved near `p` (cheap bounds test)?
    pub fn near_any(&self, p: Vec3, margin: f32) -> bool {
        self.shapes.iter().any(|s| {
            p.x >= s.lo.x - margin
                && p.x <= s.hi.x + margin
                && p.y >= s.lo.y - margin
                && p.y <= s.hi.y + margin
                && p.z >= s.lo.z - margin
                && p.z <= s.hi.z + margin
        })
    }
}

// ------------------------------------------------------------------ compiling recipes

fn shape(prim: Prim, op: Op, rough: f32, rock: Rock, floor: f32, owner: u16) -> Shape {
    let m = NOISE_REACH * rough.max(0.3) + 0.6;
    let (lo, hi) = match &prim {
        Prim::Tube { pts, w, h, .. } => {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in pts {
                lo = lo.min(*p - Vec3::new(w * 0.75, 1.0, w * 0.75));
                hi = hi.max(*p + Vec3::new(w * 0.75, h * 1.15, w * 0.75));
            }
            (lo, hi)
        }
        Prim::Room { c, r, .. } | Prim::Blob { c, r, .. } | Prim::Block { c, r, .. } => {
            let rr = r.x.max(r.z);
            (*c - Vec3::new(rr, r.y, rr), *c + Vec3::new(rr, r.y, rr))
        }
        Prim::Pit { c, top, r1, .. } => (
            *c - Vec3::new(*r1, 1.0, *r1),
            Vec3::new(c.x + r1, *top, c.z + r1),
        ),
        Prim::Shaft { a, b, r } | Prim::Capsule { a, b, r } => {
            (a.min(*b) - Vec3::splat(*r), a.max(*b) + Vec3::splat(*r))
        }
        Prim::Ramp { a, b, w } => (
            a.min(*b) - Vec3::new(w + 2.0, 6.0, w + 2.0),
            a.max(*b) + Vec3::new(w + 2.0, 1.0, w + 2.0),
        ),
        Prim::Band { pts, thick, w, .. } => {
            let t = thick.iter().copied().fold(0.0, f32::max);
            let r = t.max(*w);
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in pts {
                lo = lo.min(*p - Vec3::splat(r));
                hi = hi.max(*p + Vec3::splat(r));
            }
            (lo, hi)
        }
        Prim::Spike {
            base, u, len, r, ..
        } => {
            let tip = *base + *u * *len;
            (
                base.min(tip) - Vec3::splat(*r + 0.3),
                base.max(tip) + Vec3::splat(*r + 0.3),
            )
        }
        Prim::Causeway { pts, w, base, lean } => {
            let side = lean.iter().map(|l| l.length()).fold(0.0, f32::max) + w * 0.5 + 4.2;
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in pts {
                lo = lo.min(*p - Vec3::new(side, 0.0, side));
                hi = hi.max(*p + Vec3::new(side, 1.2, side));
            }
            lo.y =
                lo.y.min(base.iter().copied().fold(f32::MAX, f32::min) - 1.0);
            (lo, hi)
        }
    };
    Shape {
        prim,
        op,
        lo: lo - Vec3::splat(m),
        hi: hi + Vec3::splat(m),
        rough,
        rock,
        floor,
        owner,
    }
}

fn tube(
    pts: &[[f32; 3]],
    w: f32,
    h: f32,
    profile: Profile,
    phase: f32,
    calm: (bool, bool),
) -> Prim {
    let pts: Vec<Vec3> = pts.iter().map(|p| Vec3::from_array(*p)).collect();
    let mut along = vec![0.0];
    for k in 1..pts.len() {
        along.push(along[k - 1] + pts[k].distance(pts[k - 1]));
    }
    Prim::Tube {
        pts,
        along,
        w,
        h,
        profile,
        phase,
        calm,
    }
}

/// The carved and added shapes of one cave (no formations yet).
fn cave_shapes(cave: &Cave, owner: u16, t: &Ground, out: &mut Vec<Shape>) {
    let theme = cave.theme();
    let rock = Rock::for_theme(theme);
    let mut rng = Rng::new(cave.seed as u64 ^ 0x5eed_ca7e);
    let rough = if cave.kind == CaveKind::Mine {
        0.8
    } else {
        1.0
    };

    if let Some(a) = &cave.arch {
        arch_shapes(cave, a, owner, t, out);
        return;
    }

    // chambers: a main lobe and a few smaller ones round it for an irregular outline
    for c in &cave.chambers {
        let p = c.center();
        let [rx, h, rz] = c.size;
        let mut lr = Rng::new((cave.seed as u64) << 16 ^ c.id as u64);
        let ry = h * 0.62;
        let centre = p + Vec3::Y * (h - ry);
        let sc = c.yaw.sin_cos();
        out.push(shape(
            Prim::Room {
                c: centre,
                r: Vec3::new(rx, ry, rz),
                sc,
                floor: p.y,
            },
            Op::Carve,
            rough,
            rock,
            p.y,
            owner,
        ));
        let lobes = if cave.kind == CaveKind::Mine {
            0
        } else {
            1 + lr.below(3) as usize
        };
        for _ in 0..lobes {
            let a = lr.f32() * std::f32::consts::TAU;
            let off = Vec2::from_angle(a) * Vec2::new(rx, rz) * lr.range(0.35, 0.6);
            let s = lr.range(0.45, 0.7);
            let lh = h * lr.range(0.55, 0.9);
            let lry = lh * 0.62;
            let lc = Vec3::new(p.x + off.x, p.y + lh - lry, p.z + off.y);
            let lsc = (c.yaw + lr.range(-0.8, 0.8)).sin_cos();
            out.push(shape(
                Prim::Room {
                    c: lc,
                    r: Vec3::new(rx * s, lry, rz * s),
                    sc: lsc,
                    floor: p.y,
                },
                Op::Carve,
                rough,
                rock,
                p.y,
                owner,
            ));
        }
        if c.pool
            && let Some((pc, pr)) = super::dress::pool_of(cave, c)
        {
            // a basin with shelving shores (the lower part of a wide ellipsoid: about a quarter
            // of its radius deep, its rim no steeper than ~0.6), so a walker wades in and out
            let depth = (pr * 0.3).clamp(0.7, 1.5);
            let rb = pr / 0.745;
            let floor = p.y - depth - 0.02;
            out.push(shape(
                Prim::Room {
                    c: Vec3::new(pc.x, p.y + depth * 2.0, pc.y),
                    r: Vec3::new(rb, depth * 3.0, rb),
                    sc: (0.0, 1.0),
                    floor,
                },
                Op::Carve,
                0.4,
                rock,
                floor,
                owner,
            ));
        }
        if c.skylight {
            let top = t.h(p.x, p.z) + 4.0;
            let r = lr.range(1.6, 2.6);
            let off = Vec2::from_angle(lr.f32() * 6.28) * rx.min(rz) * 0.25;
            out.push(shape(
                Prim::Shaft {
                    a: Vec3::new(p.x + off.x, p.y + h * 0.6, p.z + off.y),
                    b: Vec3::new(p.x + off.x * 1.4, top, p.z + off.y * 1.4),
                    r,
                },
                Op::Carve,
                0.7,
                rock,
                p.y + h * 0.6,
                owner,
            ));
        }
    }

    // waterfalls: the notch high in the wall they pour from
    for f in super::dress::fall_sites(cave) {
        let back = Vec3::new(-f.out.x, 0.0, -f.out.y);
        let a = f.lip - back * 0.5;
        let b = f.lip + back * 3.5 + Vec3::Y * 0.8;
        out.push(shape(
            Prim::Capsule {
                a,
                b,
                r: f.width * 0.6 + 0.4,
            },
            Op::Carve,
            0.6,
            rock,
            f.lip.y - f.width * 0.3,
            owner,
        ));
    }

    // tunnels
    for tn in &cave.tunnels {
        if tn.pts.len() < 2 {
            continue;
        }
        let floor = tn.pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let mut sh = shape(
            tube(
                &tn.pts,
                tn.width,
                tn.height,
                tn.profile,
                rng.f32() * 10.0,
                (tn.a == OUTSIDE, tn.b == OUTSIDE),
            ),
            Op::Carve,
            rough,
            rock,
            floor,
            owner,
        );
        // a sloping tunnel's floor is everywhere along it: let the noise judge by the profile
        if tn.pts.iter().any(|p| (p[1] - floor).abs() > 1.0) {
            sh.floor = f32::NAN;
        }
        out.push(sh);
    }

    // the dungeon's doorway: a squared passage from the chamber to the door, its floor level
    if let Some(dg) = &cave.dungeon
        && let Some(c) = cave.chamber(dg.chamber)
    {
        let door = Vec3::from_array(dg.door);
        let back = Vec2::new(dg.door_yaw.sin(), dg.door_yaw.cos());
        let start = Vec3::new(c.pos[0], c.pos[1], c.pos[2]);
        let end = Vec3::new(door.x - back.x * 0.6, c.pos[1], door.z - back.y * 0.6);
        let pts = [
            start.to_array(),
            Vec3::new(door.x + back.x * 3.0, c.pos[1], door.z + back.y * 3.0).to_array(),
            end.to_array(),
        ];
        let mut sh = shape(
            tube(&pts, 3.6, 4.0, Profile::Drift, 0.0, (false, false)),
            Op::Carve,
            0.35,
            Rock::for_theme(Theme::Mine),
            c.pos[1],
            owner,
        );
        sh.floor = c.pos[1];
        out.push(sh);
    }

    // the way in
    let e = Vec3::from_array(cave.entrance);
    let d = super::layout::dir(cave.heading);
    match cave.mouth {
        Mouth::Hillside => {
            // a crag round the mouth: the tunnel opens in a steep rock face pushed into the
            // hill, not a gash in the turf; a block or two beside it breaks the outline
            if let Some(tn) = cave.tunnels.iter().find(|t| t.a == OUTSIDE) {
                let (w, h) = (tn.width, tn.height);
                let half_d = 7.5 + h * 0.3;
                let a = w * 0.75 + 3.2 + rng.range(0.0, 1.5);
                let top = e.y + h + rng.range(2.2, 3.6);
                let b = (h + 6.0) * 0.5;
                let c = Vec3::new(
                    e.x + d.x * (half_d - 0.6),
                    top - b,
                    e.z + d.y * (half_d - 0.6),
                );
                let sc = (cave.heading + rng.range(-0.12, 0.12)).sin_cos();
                out.push(shape(
                    Prim::Block {
                        c,
                        r: Vec3::new(a, b, half_d),
                        sc,
                        n: 3.2,
                    },
                    Op::Add,
                    1.0,
                    rock,
                    f32::NAN,
                    owner,
                ));
                let side_n = 1 + rng.below(2);
                for k in 0..side_n {
                    let sgn = if (k == 0) == (rng.f32() < 0.5) {
                        1.0
                    } else {
                        -1.0
                    };
                    let lat = Vec2::new(d.y, -d.x) * sgn * (a * rng.range(0.75, 1.05));
                    let back = rng.range(1.0, 3.5);
                    let bh = b * rng.range(0.6, 1.05);
                    let c2 = Vec3::new(
                        c.x + lat.x + d.x * back,
                        c.y - b + bh + rng.range(-0.5, 1.5),
                        c.z + lat.y + d.y * back,
                    );
                    let r2 = Vec3::new(a * rng.range(0.45, 0.7), bh, half_d * rng.range(0.6, 0.9));
                    out.push(shape(
                        Prim::Block {
                            c: c2,
                            r: r2,
                            sc: (cave.heading + rng.range(-0.4, 0.4)).sin_cos(),
                            n: 2.8,
                        },
                        Op::Add,
                        1.1,
                        rock,
                        f32::NAN,
                        owner,
                    ));
                }
            }
        }
        Mouth::Pit => {
            if let Some(c) = cave.chambers.first() {
                let p = c.center();
                let g = t.h(p.x, p.z);
                let r0 = c.size[0].min(c.size[2]) * 0.85;
                let r1 = r0 * 1.35 + 2.0;
                out.push(shape(
                    Prim::Pit {
                        c: p,
                        top: g + 8.0,
                        r0,
                        r1,
                    },
                    Op::Carve,
                    1.0,
                    rock,
                    p.y,
                    owner,
                ));
            }
        }
        Mouth::Adit | Mouth::Shaft | Mouth::None => {}
    }
}

fn arch_shapes(cave: &Cave, a: &ArchSpec, owner: u16, t: &Ground, out: &mut Vec<Shape>) {
    let rock = Rock::for_theme(cave.theme());
    let (pa, pb) = (Vec2::from_array(a.a), Vec2::from_array(a.b));
    let (ha, hb) = (t.h(pa.x, pa.y), t.h(pb.x, pb.y));
    out.push(shape(
        arch_band(pa, pb, ha, hb, a.height, a.thickness),
        Op::Add,
        0.8,
        rock,
        f32::NAN,
        owner,
    ));
}

/// A natural arch from foot `pa` (ground at `ha`) to foot `pb` (at `hb`): `height` clear under
/// its middle, a band of rock `thickness` thick (thicker at the feet, sunk into the ground).
fn arch_band(pa: Vec2, pb: Vec2, ha: f32, hb: f32, height: f32, thickness: f32) -> Prim {
    let span = pa.distance(pb).max(2.0);
    let apex = ha.max(hb) + height + thickness * 0.5;
    let n = 18;
    let mut pts = vec![];
    let mut thick = vec![];
    for k in 0..=n {
        // a little past each foot, so the feet sink into the ground
        let s = -0.06 + 1.12 * k as f32 / n as f32;
        let q = pa.lerp(pb, s);
        let base = ha + (hb - ha) * s.clamp(0.0, 1.0) - 1.5;
        let m = (1.0 - (2.0 * s - 1.0).powi(2)).max(0.0).powf(0.62);
        let y = base + (apex - base) * m;
        pts.push(Vec3::new(q.x, y, q.y));
        thick.push(thickness * (1.0 + 1.1 * (2.0 * s - 1.0).powi(4)));
    }
    let w = (thickness * 1.5).max(span * 0.22).max(4.0);
    let d = (pb - pa).normalize_or(Vec2::X);
    let across = Vec3::new(-d.y, 0.0, d.x);
    Prim::Band {
        pts,
        thick,
        w,
        across,
    }
}

/// Distance from `p` to the segment `a`–`b` (on the ground plane).
pub(super) fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The ways through a cave in plan (start, end, half width): every tunnel's floor line, on into
/// each chamber's middle, and the walk to the dungeon door. (A pit's scree ramp is added where
/// it is built.)
pub fn lanes(cave: &Cave) -> Vec<(Vec2, Vec2, f32)> {
    let xz = |p: [f32; 3]| Vec2::new(p[0], p[2]);
    let mid = |id: u16| {
        cave.chambers
            .iter()
            .find(|c| c.id == id)
            .map(|c| xz(c.center().to_array()))
    };
    let mut lanes: Vec<(Vec2, Vec2, f32)> = vec![];
    for t in &cave.tunnels {
        let half = t.width * 0.5;
        lanes.extend(t.pts.windows(2).map(|w| (xz(w[0]), xz(w[1]), half)));
        for (end, p) in [(t.a, t.pts.first()), (t.b, t.pts.last())] {
            if let (Some(m), Some(p)) = (mid(end), p) {
                lanes.push((m, xz(*p), half));
            }
        }
    }
    if let Some(dg) = &cave.dungeon
        && let Some(m) = mid(dg.chamber)
    {
        lanes.push((m, xz(dg.door), 1.8));
    }
    lanes
}

/// Rock under every stretch of a tunnel's floor line that crosses open space: where a ramp runs
/// on into a chamber (its floor would otherwise end in a ledge over the chamber's), where a
/// ledge climbs a tall chamber or a walkway bridges it, where the way in starts out over a
/// falling hillside. Low stretches become scree banks down to the floor, high ones bridges.
fn walk_supports(cave: &Cave, owner: u16, probe: &Volume, out: &mut Vec<Shape>) {
    let rock = Rock::for_theme(cave.theme());
    for tn in &cave.tunnels {
        let pts: Vec<Vec3> = tn.pts.iter().map(|p| Vec3::from_array(*p)).collect();
        if pts.len() < 2 {
            continue;
        }
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        // the floor line every half metre, as the tunnel draws it (relief eased off where it
        // opens outside), and whether there is open air just under it (and the floor below)
        let mut line: Vec<(Vec3, Option<f32>)> = vec![];
        let mut s0 = 0.0;
        for (wi, w) in pts.windows(2).enumerate() {
            let len = w[0].distance(w[1]);
            let n = ((len / 0.5).ceil() as usize).max(1);
            let last = wi + 2 == pts.len();
            for k in 0..n + usize::from(last) {
                let u = k as f32 / n as f32;
                let q = w[0].lerp(w[1], u);
                let s = s0 + len * u;
                let mut calm = 1.0;
                if tn.a == OUTSIDE {
                    calm *= smoothstep(4.0, 14.0, s);
                }
                if tn.b == OUTSIDE {
                    calm *= smoothstep(4.0, 14.0, total - s);
                }
                let top = Vec3::new(q.x, q.y + relief(q.x, q.z) * calm, q.z);
                // (deeper than the floor's own roughness, ±0.25 m)
                let under = probe.at(top - Vec3::Y * 0.3);
                let floor = if under.d > 0.05 {
                    // sphere-step down to the floor under it (or a long way: a bridge over a void)
                    let mut y = top.y - 0.3;
                    let mut d = under.d;
                    while y > top.y - 40.0 && d > 0.02 {
                        y -= d.clamp(0.1, 2.0);
                        d = probe.at(Vec3::new(q.x, y, q.z)).d;
                    }
                    // never across another tunnel's walking corridor (two ways sharing the space
                    // below: it stays open; a bridge high over a way below is fine)
                    let bottom = top.y - causeway_depth(top.y - y);
                    let idx = probe.shapes_near(Vec3::new(q.x, bottom, q.z), top, 0.5);
                    // (its own corridor ends at its floor: 0.6 m under it is clear of it)
                    let blocks = [
                        top.y - 0.6,
                        (top.y - 0.6).min((top.y + bottom) * 0.5),
                        (top.y - 0.6).min(bottom + 0.2),
                    ]
                    .iter()
                    .any(|&yy| {
                        idx.iter().any(|&i| {
                            matches!(probe.shapes[i as usize].prim, Prim::Tube { .. })
                                && probe.shapes[i as usize]
                                    .prim
                                    .dist_floor_core(Vec3::new(q.x, yy, q.z))
                                    .2
                                    < 0.0
                        })
                    });
                    (!blocks).then_some(y)
                } else {
                    None
                };
                line.push((top, floor));
            }
            s0 += len;
        }
        // each run of open stretches (two metres more at both ends, into the rock it starts from,
        // so the last of the drop — under the 0.3 m it looks for — is covered too)
        let mut i = 0;
        while i < line.len() {
            if line[i].1.is_none() {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < line.len() && line[j + 1].1.is_some() {
                j += 1;
            }
            let (a, b) = (i.saturating_sub(4), (j + 4).min(line.len() - 1));
            let run = &line[a..=b];
            let fallback = run.iter().filter_map(|x| x.1).fold(f32::MAX, f32::min);
            let pts: Vec<Vec3> = run.iter().map(|x| x.0).collect();
            let base: Vec<f32> = run
                .iter()
                .map(|x| x.1.unwrap_or(fallback).min(x.0.y))
                .collect();
            let lean = vec![Vec2::ZERO; pts.len()];
            if pts.len() >= 2 {
                let w = (tn.width * 0.8).max(WALK_W + 0.8);
                out.push(shape(
                    Prim::Causeway { pts, w, base, lean },
                    Op::Decor,
                    1.0,
                    rock,
                    f32::NAN,
                    owner,
                ));
            }
            i = j + 1;
        }
    }
}

/// The lip round a pool (centre `pc`, radius `pr`, water at `level`) in the given style. Every
/// style closes the ring at least a hand over the water, so the pool holds wherever the floor
/// round it dips.
fn pool_rim(
    style: RimStyle,
    pc: Vec2,
    pr: f32,
    level: f32,
    seed: u64,
    clear: &dyn Fn(Vec2, f32) -> bool,
    ground: &dyn Fn(Vec2) -> f32,
    rock: Rock,
    owner: u16,
    out: &mut Vec<Shape>,
) {
    let mut rr = Rng::new(seed);
    let tau = std::f32::consts::TAU;
    // how high the lip's top stands at a spot: `proud` over the floor there, and always a hand
    // over the water
    let top = |q: Vec2, proud: f32| (ground(q) + proud).max(level + 0.12);
    // a closed ring of capsules round the pool: radius, tube and how proud of the floor round
    // the angle (`plain`: near a way through a stretch keeps to the plain dam)
    let dam = |out: &mut Vec<Shape>,
               rad: &dyn Fn(f32) -> f32,
               tube: &dyn Fn(f32) -> f32,
               proud: &dyn Fn(f32) -> f32,
               plain: bool| {
        let n = ((pr * tau / 0.6) as usize).clamp(16, 90);
        let p = |a: f32| {
            let q = pc + Vec2::from_angle(a) * rad(a);
            let (q, r, pr2) = if plain && !clear(q, tube(a)) {
                (pc + Vec2::from_angle(a) * pr * 1.04, 0.3, 0.0)
            } else {
                (q, tube(a), proud(a))
            };
            (q, top(q, pr2) - r, r)
        };
        for k in 0..n {
            let (a0, a1) = (k as f32 / n as f32 * tau, (k + 1) as f32 / n as f32 * tau);
            let ((q0, y0, r0), (q1, y1, r1)) = (p(a0), p(a1));
            out.push(shape(
                Prim::Capsule {
                    a: Vec3::new(q0.x, y0, q0.y),
                    b: Vec3::new(q1.x, y1, q1.y),
                    r: r0.max(r1),
                },
                Op::Decor,
                0.6,
                rock,
                f32::NAN,
                owner,
            ));
        }
    };
    // the plain dam every style stands on: the water held all round (buried where the floor is
    // higher)
    dam(out, &|_| pr * 1.04, &|_| 0.3, &|_| 0.0, false);
    let (lobes, ph) = (5.0 + rr.below(5) as f32, rr.f32() * tau);
    match style {
        // (all a little larger than life: the editor's 0.4 m grid rounds off anything smaller)
        RimStyle::Auto | RimStyle::Rimstone => dam(
            out,
            &|_| pr * 1.05,
            &|a| 0.4 + 0.05 * (a * 3.0 + ph).sin(),
            &|a| 0.36 + 0.06 * (a * 5.0 + ph).sin(),
            true,
        ),
        RimStyle::Scalloped => dam(
            out,
            &|a| pr * (1.07 + 0.14 * (lobes * a + ph).sin()),
            &|a| 0.32 + 0.08 * (lobes * 2.0 * a + ph).cos(),
            &|a| 0.3 + 0.1 * (lobes * a + ph * 1.3).sin(),
            true,
        ),
        RimStyle::Terraces => {
            // steps of travertine going out from the water, each a little higher (the outer
            // ones broken off where a way passes)
            for (k, f) in [1.05f32, 1.3, 1.55].into_iter().enumerate() {
                let p2 = ph + k as f32 * 1.7;
                let proud = 0.2 + k as f32 * 0.18;
                if k == 0 {
                    dam(
                        out,
                        &|a| pr * (f + 0.06 * (lobes * a + p2).sin()),
                        &|_| 0.32,
                        &|a| proud + 0.04 * (lobes * a + p2).cos(),
                        true,
                    );
                    continue;
                }
                let n = ((pr * f * tau / 0.6) as usize).clamp(16, 110);
                let r = |a: f32| pr * (f + 0.06 * (lobes * a + p2).sin());
                for j in 0..n {
                    let (a0, a1) = (j as f32 / n as f32 * tau, (j + 1) as f32 / n as f32 * tau);
                    let (q0, q1) = (
                        pc + Vec2::from_angle(a0) * r(a0),
                        pc + Vec2::from_angle(a1) * r(a1),
                    );
                    if clear(q0, 0.35) && clear(q1, 0.35) {
                        let (y0, y1) = (top(q0, proud) - 0.32, top(q1, proud) - 0.32);
                        out.push(shape(
                            Prim::Capsule {
                                a: Vec3::new(q0.x, y0, q0.y),
                                b: Vec3::new(q1.x, y1, q1.y),
                                r: 0.34,
                            },
                            Op::Decor,
                            0.3,
                            rock,
                            f32::NAN,
                            owner,
                        ));
                    }
                }
            }
        }
        RimStyle::Boulders => {
            // rounded boulders sitting round the water's edge
            let n = ((pr * tau / 0.85) as usize).max(8);
            for k in 0..n {
                let a = (k as f32 + rr.range(-0.3, 0.3)) / n as f32 * tau;
                let q = pc + Vec2::from_angle(a) * pr * (1.08 + rr.range(-0.04, 0.18));
                let r = rr.range(0.45, 1.05);
                let sc = (rr.f32() * tau).sin_cos();
                let squash = rr.range(1.0, 1.4);
                if clear(q, r * 1.4) {
                    out.push(shape(
                        Prim::Blob {
                            c: Vec3::new(q.x, ground(q).max(level) + r * 0.3, q.y),
                            r: Vec3::new(r * squash, r * 0.8, r),
                            sc,
                        },
                        Op::Decor,
                        0.8,
                        rock,
                        f32::NAN,
                        owner,
                    ));
                }
            }
        }
        RimStyle::Shelf => {
            // a wide, flat apron of flowstone round it, its edge a step proud of the floor
            let n = ((pr * tau / 0.65) as usize).max(12);
            for k in 0..n {
                let a = k as f32 / n as f32 * tau;
                let q = pc + Vec2::from_angle(a) * pr * 1.14;
                // (its long axis along the shore)
                let sc = (-a.cos(), -a.sin());
                if clear(q, 1.1) {
                    let t = top(q, 0.3);
                    out.push(shape(
                        Prim::Blob {
                            c: Vec3::new(q.x, t - 0.26, q.y),
                            r: Vec3::new(0.85, 0.26, 1.15),
                            sc,
                        },
                        Op::Decor,
                        0.3,
                        rock,
                        f32::NAN,
                        owner,
                    ));
                }
            }
        }
        RimStyle::Crust => {
            // sharp crystals bristling round the water, leaning out and up
            let n = ((pr * tau * 3.5) as usize).max(18);
            for _ in 0..n {
                let a = rr.f32() * tau;
                let out_dir = Vec2::from_angle(a);
                let q = pc + out_dir * pr * rr.range(1.0, 1.14);
                let lean = rr.range(-0.2, 0.8);
                let u = Vec3::new(out_dir.x * lean, 1.0, out_dir.y * lean).normalize();
                let len = rr.range(0.6, 1.3);
                let r = len * rr.range(0.18, 0.28);
                if clear(q, len) {
                    out.push(shape(
                        Prim::Spike {
                            base: Vec3::new(q.x, ground(q).max(level) - 0.05, q.y),
                            u,
                            len,
                            r,
                            column: false,
                        },
                        Op::Decor,
                        0.3,
                        rock,
                        f32::NAN,
                        owner,
                    ));
                }
            }
        }
    }
}

/// Formations, scree and ramps of one cave, placed by probing the carved field (`probe`).
fn cave_decor(cave: &Cave, owner: u16, probe: &Volume, out: &mut Vec<Shape>) {
    let theme = cave.theme();
    let rock = Rock::for_theme(theme);
    let mut rng = Rng::new(cave.seed as u64 ^ 0xdec0_7a7e);
    // the ways through, and the pools: nothing standing on the floor goes there
    let mut lanes = lanes(cave);
    for c in cave.chambers.iter().filter(|c| c.pool) {
        if let Some((pc, pr)) = super::dress::pool_of(cave, c) {
            lanes.push((pc, pc, pr + 0.6));
        }
    }
    // pit: a scree slope from the rim down to the floor, across the pit
    if cave.mouth == Mouth::Pit
        && let Some(c) = cave.chambers.first()
    {
        let p = c.center();
        let d = super::layout::dir(cave.heading);
        let r0 = c.size[0].min(c.size[2]) * 0.85;
        let r1 = r0 * 1.35 + 2.0;
        let top = Vec2::new(p.x, p.z) - d * (r1 * 1.05);
        let g = probe.ground.h(top.x, top.y) + 0.4;
        let bot = Vec2::new(p.x, p.z) + d * (r0 * 0.55);
        out.push(shape(
            Prim::Ramp {
                a: Vec3::new(top.x, g, top.y),
                b: Vec3::new(bot.x, p.y - 0.3, bot.y),
                w: r0 * 0.9 + 2.0,
            },
            Op::Decor,
            1.0,
            rock,
            f32::NAN,
            owner,
        ));
        lanes.push((top, bot, 2.0));
    }
    let in_the_way = |q: Vec2, r: f32| {
        lanes
            .iter()
            .any(|(a, b, half)| seg_dist(q, *a, *b) < half + r + 0.4)
    };
    let feats = cave.features;
    let formations = feats.formations
        && matches!(
            theme,
            Theme::Limestone | Theme::Ice | Theme::Mossy | Theme::Crystal
        );
    let ice = theme == Theme::Ice;
    for c in &cave.chambers {
        let p = c.center();
        let [rx, h, rz] = c.size;
        let area = rx * rz;
        // formations grow with the room: giant ones in great halls and in the big caves
        let fs = if c.hall {
            (rx.min(rz) / 12.0).clamp(2.5, 5.0)
        } else {
            cave.size.scale().max(1.0)
        };
        let (s, co) = c.yaw.sin_cos();
        let idx = probe.shapes_near(
            p - Vec3::new(rx + 3.0, 3.0, rx.max(rz) + 3.0),
            p + Vec3::new(rx.max(rz) + 3.0, h + 6.0, rx.max(rz) + 3.0),
            1.0,
        );
        let at = |u: f32, v: f32| {
            let l = Vec2::new(u * rx, v * rz);
            Vec2::new(p.x + l.x * co + l.y * s, p.z - l.x * s + l.y * co)
        };
        // the air column at a spot: floor and roof (None when open to the sky, as in a pit)
        let column = |q: Vec2| -> Option<(f32, f32)> {
            let mut y = p.y + 0.6;
            if probe.eval(Vec3::new(q.x, y, q.y), &idx).d < 0.0 {
                return None;
            }
            let mut floor = y;
            while floor > p.y - 3.0 && probe.eval(Vec3::new(q.x, floor - 0.2, q.y), &idx).d > 0.0 {
                floor -= 0.2;
            }
            let top = p.y + h + 6.0;
            while y < top {
                let d = probe.eval(Vec3::new(q.x, y + 0.25, q.y), &idx).d;
                if d <= 0.0 {
                    break;
                }
                y += d.clamp(0.25, 2.0);
            }
            // a roof of rock: found, and under the ground (not the open sky over a pit)
            if y >= top || y > probe.ground.h(q.x, q.y) - 0.5 {
                return None;
            }
            Some((floor, y))
        };
        if formations {
            let n = ((area / (if ice { 14.0 } else { 22.0 } * fs)) as usize)
                .clamp(2, if c.hall { 48 } else { 26 });
            for _ in 0..n {
                let (u, v) = (rng.range(-0.75, 0.75), rng.range(-0.75, 0.75));
                if u * u + v * v > 0.6 {
                    continue;
                }
                let q = at(u, v);
                let Some((floor, roof)) = column(q) else {
                    continue;
                };
                let gap = roof - floor;
                if gap < 2.4 * fs.sqrt() {
                    continue;
                }
                let tilt = Vec3::new(rng.range(-0.06, 0.06), 0.0, rng.range(-0.06, 0.06));
                let roll = rng.f32();
                if roll < 0.12 && gap < 9.0 * fs && !ice {
                    // a column floor to roof
                    let r = rng.range(0.55, 1.3) * fs;
                    if in_the_way(q, r) {
                        continue;
                    }
                    out.push(shape(
                        Prim::Spike {
                            base: Vec3::new(q.x, floor - 0.6, q.y),
                            u: Vec3::Y,
                            len: gap + 1.2,
                            r,
                            column: true,
                        },
                        Op::Decor,
                        1.0,
                        rock,
                        f32::NAN,
                        owner,
                    ));
                    continue;
                }
                // a stalactite (icicle), often with a stalagmite under it
                let len = (rng.range(0.9, 1.0) * gap * if ice { 0.45 } else { 0.32 })
                    .clamp(fs, if ice { 5.5 } else { 3.8 } * fs);
                let r = if ice {
                    rng.range(0.28, 0.6)
                } else {
                    rng.range(0.35, 0.85)
                } * (0.6 + len / fs * 0.18)
                    * fs;
                out.push(shape(
                    Prim::Spike {
                        base: Vec3::new(q.x, roof + 0.7, q.y),
                        u: (-Vec3::Y + tilt).normalize(),
                        len: len + 0.7,
                        r,
                        column: false,
                    },
                    Op::Decor,
                    1.0,
                    rock,
                    f32::NAN,
                    owner,
                ));
                // a few smaller ones round it (they hang in clusters)
                for _ in 0..rng.below(3) {
                    let o = Vec2::from_angle(rng.f32() * 6.28) * rng.range(0.8, 2.0) * fs;
                    let q2 = q + o;
                    if let Some((_, roof2)) = column(q2) {
                        let l2 = len * rng.range(0.35, 0.75);
                        out.push(shape(
                            Prim::Spike {
                                base: Vec3::new(q2.x, roof2 + 0.6, q2.y),
                                u: -Vec3::Y,
                                len: l2 + 0.6,
                                r: r * rng.range(0.5, 0.8),
                                column: false,
                            },
                            Op::Decor,
                            1.0,
                            rock,
                            f32::NAN,
                            owner,
                        ));
                    }
                }
                if rng.f32() < 0.55 && !c.pool {
                    let l = (gap * rng.range(0.12, 0.25)).clamp(0.6 * fs, 2.6 * fs);
                    let rb = r * rng.range(1.1, 1.6);
                    if !in_the_way(q, rb) {
                        out.push(shape(
                            Prim::Spike {
                                base: Vec3::new(q.x, floor - 0.5, q.y),
                                u: Vec3::Y,
                                len: l + 0.5,
                                r: rb,
                                column: false,
                            },
                            Op::Decor,
                            1.0,
                            rock,
                            f32::NAN,
                            owner,
                        ));
                    }
                }
            }
        }
        if feats.rubble {
            let n = ((area / 30.0) as usize).clamp(1, 10);
            for _ in 0..n {
                let a = rng.f32() * std::f32::consts::TAU;
                let k = rng.range(0.55, 0.85);
                let q = at(a.cos() * k, a.sin() * k);
                let fy = {
                    let mut y = p.y + 0.6;
                    if probe.eval(Vec3::new(q.x, y, q.y), &idx).d < 0.0 {
                        continue;
                    }
                    while y > p.y - 3.0 && probe.eval(Vec3::new(q.x, y - 0.2, q.y), &idx).d > 0.0 {
                        y -= 0.2;
                    }
                    y
                };
                let floor = fy;
                let r = rng.range(0.5, 1.5) * fs.powf(0.7);
                if in_the_way(q, r * 1.6) {
                    continue;
                }
                out.push(shape(
                    Prim::Blob {
                        c: Vec3::new(q.x, floor - r * 0.35, q.y),
                        r: Vec3::new(r * rng.range(1.0, 1.6), r * 0.75, r),
                        sc: (rng.f32() * 6.28).sin_cos(),
                    },
                    Op::Decor,
                    1.0,
                    rock,
                    f32::NAN,
                    owner,
                ));
            }
        }
        // a pool's lip: the rock round it that holds its water in (`Cave::rim_of`) — a dam where
        // the floor round the pool dips under the water's level, buried in the floor elsewhere
        if c.pool
            && let Some((pc, pr)) = super::dress::pool_of(cave, c)
        {
            // (its showier parts stay off the ways through: only the plain dam goes near them)
            let ways = crate::caves::sdf::lanes(cave);
            let clear = move |q: Vec2, r: f32| {
                !ways
                    .iter()
                    .any(|(a, b, half)| seg_dist(q, *a, *b) < half + r + 0.3)
            };
            // (it stands on the floor round the pool, wherever that lies)
            let ground = |q: Vec2| column(q).map(|f| f.0).unwrap_or(p.y);
            pool_rim(
                cave.rim_of(c),
                pc,
                pr,
                super::dress::pool_level(c),
                cave.seed as u64 ^ ((c.id as u64) << 12) ^ 0x41e,
                &clear,
                &ground,
                rock,
                owner,
                out,
            );
        }
        // great halls: rocky hills on the floor, off the ways through
        if c.hall {
            for _ in 0..3 + rng.below(4) {
                let a = rng.f32() * std::f32::consts::TAU;
                let k = rng.range(0.15, 0.6);
                let q = at(a.cos() * k, a.sin() * k);
                let (big, tall) = (
                    rng.range(5.0, 11.0) * fs / 2.5,
                    rng.range(3.0, 8.0) * fs / 2.5,
                );
                let sc = (rng.f32() * 6.28).sin_cos();
                let squash = rng.range(0.6, 1.0);
                if in_the_way(q, big * 1.25) {
                    continue;
                }
                let Some((floor, roof)) = column(q) else {
                    continue;
                };
                if roof - floor < tall * 2.5 {
                    continue;
                }
                out.push(shape(
                    Prim::Blob {
                        c: Vec3::new(q.x, floor - tall * 0.35, q.y),
                        r: Vec3::new(big, tall, big * squash),
                        sc,
                    },
                    Op::Decor,
                    1.2,
                    rock,
                    f32::NAN,
                    owner,
                ));
            }
        }
        // natural arches standing over a way through a tall chamber (with ledges and bridges)
        if feats.bridges && h >= 8.0 && rx.min(rz) * 0.79 >= 9.0 && cave.kind != CaveKind::Mine {
            let count = if c.hall {
                1 + rng.below(3)
            } else {
                u32::from(rng.f32() < 0.5)
            };
            // the level stretches of the ways across this chamber's floor
            let reach = rx.min(rz) * 0.79;
            let flat: Vec<(Vec3, Vec3, f32)> = cave
                .tunnels
                .iter()
                .flat_map(|t| {
                    t.pts.windows(2).map(move |w| {
                        (
                            Vec3::from_array(w[0]),
                            Vec3::from_array(w[1]),
                            t.width * 0.5,
                        )
                    })
                })
                .filter(|(a, b, _)| {
                    (a.y - p.y).abs() < 1.0
                        && (b.y - p.y).abs() < 1.0
                        && a.xz().distance(p.xz()) < reach * 0.85
                        && b.xz().distance(p.xz()) < reach * 0.85
                        && a.xz().distance(b.xz()) > 3.0
                })
                .collect();
            for _ in 0..count {
                if flat.is_empty() {
                    break;
                }
                let (a, b, half) = flat[rng.below(flat.len() as u32) as usize];
                let m = a.lerp(b, rng.range(0.35, 0.65));
                let u = (b - a).xz().normalize_or(Vec2::X);
                let n = Vec2::new(-u.y, u.x);
                let thick = rng.range(1.8, 3.2) * fs.sqrt();
                let span =
                    half.max(WALK_W * 0.5) + thick * 0.9 + 1.5 + rng.range(0.0, 2.0) * fs.sqrt();
                let (fa, fb) = (m.xz() - n * span, m.xz() + n * span);
                let Some((_, roof)) = column(m.xz()) else {
                    continue;
                };
                let rise = rng.range(0.45, 0.75) * (roof - m.y - thick - 1.5);
                let rise = rise.max((WALK_H + 1.2) / 0.7);
                if m.y + rise + thick * 1.6 > roof - 1.0
                    || column(fa).is_none()
                    || column(fb).is_none()
                {
                    continue;
                }
                // (its feet off every other way)
                let others = |q: Vec2| {
                    lanes.iter().any(|(la, lb, lh)| {
                        seg_dist(q, *la, *lb) < lh + thick * 0.7 + 0.4
                            && seg_dist(m.xz(), *la, *lb) > 0.5
                    })
                };
                if others(fa) || others(fb) {
                    continue;
                }
                out.push(shape(
                    arch_band(fa, fb, m.y, m.y, rise, thick),
                    Op::Decor,
                    0.8,
                    rock,
                    f32::NAN,
                    owner,
                ));
            }
        }
    }
}

/// A copy of the ground under every cave and stroke (with room for outcrops and noise).
pub fn ground_for(caves: &Caves, t: &crate::terrain::Terrain) -> Ground {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for c in &caves.list {
        let (a, b) = c.bounds();
        lo = lo.min(Vec2::new(a.x, a.z));
        hi = hi.max(Vec2::new(b.x, b.z));
    }
    for s in &caves.strokes {
        for p in &s.pts {
            lo = lo.min(Vec2::new(p[0], p[2]) - s.radius);
            hi = hi.max(Vec2::new(p[0], p[2]) + s.radius);
        }
    }
    if lo.x > hi.x {
        return Ground::copy(t, Vec2::ZERO, Vec2::ZERO);
    }
    Ground::copy(t, lo - 24.0, hi + 24.0)
}

/// Every cave and dig stroke as shapes over the ground they cut into, with the terrain cells
/// their carving opens up.
pub fn compile(caves: &Caves, t: &crate::terrain::Terrain) -> Volume {
    compile_on(caves, ground_for(caves, t))
}

/// `compile` without working out the holes (enough to probe floors and walls).
pub fn compile_probe(caves: &Caves, t: &crate::terrain::Terrain) -> Volume {
    build(caves, ground_for(caves, t), false)
}

/// `compile` on a copy of the ground (`ground_for`), e.g. off the main thread.
pub fn compile_on(caves: &Caves, ground: Ground) -> Volume {
    build(caves, ground, true)
}

fn build(caves: &Caves, ground: Ground, with_holes: bool) -> Volume {
    let mut shapes = vec![];
    let mut themes = vec![];
    for (i, c) in caves.list.iter().enumerate() {
        themes.push(c.theme());
        cave_shapes(c, i as u16, &ground, &mut shapes);
    }
    for s in &caves.strokes {
        let pts: Vec<Vec3> = s.pts.iter().map(|p| Vec3::from_array(*p)).collect();
        let op = if s.fill { Op::Fill } else { Op::Dig };
        let segs: Vec<(Vec3, Vec3)> = match pts.len() {
            0 => vec![],
            1 => vec![(pts[0], pts[0])],
            _ => pts.windows(2).map(|w| (w[0], w[1])).collect(),
        };
        for (a, b) in segs {
            shapes.push(shape(
                Prim::Capsule { a, b, r: s.radius },
                op,
                1.0,
                Rock::for_theme(Theme::Limestone),
                f32::NAN,
                NONE,
            ));
        }
    }
    let giant = caves.list.iter().map(|c| c.size.giant()).collect();
    let mut vol = Volume {
        shapes,
        ground,
        themes,
        giant,
        holes: HashSet::new(),
    };
    // formations need the carved space: probe it, then add them
    let mut decor = vec![];
    for (i, c) in caves.list.iter().enumerate() {
        walk_supports(c, i as u16, &vol, &mut decor);
        cave_decor(c, i as u16, &vol, &mut decor);
    }
    // (dig strokes stay last: they apply in order on top of everything)
    let digs: Vec<Shape> = vol
        .shapes
        .iter()
        .filter(|s| matches!(s.op, Op::Dig | Op::Fill))
        .cloned()
        .collect();
    vol.shapes.retain(|s| !matches!(s.op, Op::Dig | Op::Fill));
    vol.shapes.extend(decor);
    vol.shapes.extend(digs);
    if with_holes {
        vol.holes = holes(&vol);
    }
    vol
}

/// Terrain cells whose surface the carving opens (sampled just under the terrain mesh).
pub fn holes(vol: &Volume) -> HashSet<IVec2> {
    let mut out = HashSet::new();
    let cell = vol.ground.cell();
    for s in &vol.shapes {
        if !matches!(s.op, Op::Carve | Op::Dig) {
            continue;
        }
        // only shapes reaching up to the ground can open it
        let c0 = vol.ground.cell_of(s.lo.x, s.lo.z);
        let c1 = vol.ground.cell_of(s.hi.x, s.hi.z);
        for j in c0.y..=c1.y {
            for i in c0.x..=c1.x {
                let c = IVec2::new(i, j);
                if out.contains(&c) {
                    continue;
                }
                let o = vol.ground.cell_corner(c);
                let mut hit = false;
                'cell: for b in 0..4 {
                    for a in 0..4 {
                        let x = o.x + (a as f32 + 0.5) * cell * 0.25;
                        let z = o.y + (b as f32 + 0.5) * cell * 0.25;
                        let y = vol.ground.h(x, z) - SINK - 0.12;
                        if y < s.lo.y || y > s.hi.y {
                            continue;
                        }
                        if vol.at(Vec3::new(x, y, z)).d > 0.0 {
                            hit = true;
                            break 'cell;
                        }
                    }
                }
                if hit {
                    out.insert(c);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caves::layout;

    fn sample_cave(t: &crate::terrain::Terrain, kind: CaveKind, seed: u32) -> Caves {
        let e = Vec2::new(150.0, 0.0);
        let (heading, _) = layout::uphill(t, e, Vec2::ZERO);
        let mut c = Cave {
            id: 1,
            name: "T".into(),
            kind,
            size: CaveSize::Medium,
            levels: 2,
            up: false,
            theme: Theme::Limestone,
            resolved: Theme::Auto,
            features: Features::default(),
            darkness: Darkness::Dim,
            seed,
            entrance: [e.x, t.height_at(e.x, e.y), e.y],
            heading,
            mouth: Mouth::Hillside,
            chambers: vec![],
            tunnels: vec![],
            dungeon: None,
            arch: None,
            props: vec![],
        };
        layout::layout(&mut c, t);
        Caves {
            list: vec![c],
            strokes: vec![],
            next_id: 1,
        }
    }

    fn hill() -> crate::terrain::Terrain {
        let mut t = crate::terrain::Terrain::new(256, 4.0, 0.0);
        let n = t.n();
        for j in 0..n {
            for i in 0..n {
                let p = t.grid_to_world(i, j);
                t.heights[j * n + i] =
                    10.0 + 70.0 * (-(p.length() / 180.0).powi(2)).exp() + (p.x * 0.05).sin() * 0.7;
            }
        }
        t
    }

    #[test]
    fn ground_matches_terrain_triangles() {
        let t = hill();
        let g = Ground::copy(&t, Vec2::new(-100.0, -100.0), Vec2::new(100.0, 100.0));
        for k in 0..200 {
            let x = -90.0 + k as f32 * 0.9;
            let z = 40.0 - k as f32 * 0.37;
            // on grid vertices it is the height itself
            let gi = t.world_to_grid(x, z);
            let (i, j) = (gi.x.round() as usize, gi.y.round() as usize);
            let v = t.grid_to_world(i, j);
            assert!((g.h(v.x, v.y) - t.h(i, j)).abs() < 1e-3);
            // between them, close to the bilinear height
            assert!((g.h(x, z) - t.height_at(x, z)).abs() < 1.0);
        }
    }

    #[test]
    fn chambers_are_hollow_and_rock_is_solid() {
        let t = hill();
        for seed in 0..4 {
            let caves = sample_cave(&t, CaveKind::Cave, seed);
            let vol = compile(&caves, &t);
            for c in &caves.list[0].chambers {
                // a metre and a half above the floor in the middle: air
                let p = c.center() + Vec3::Y * 1.5;
                assert!(
                    vol.at(p).d > 0.0,
                    "seed {seed} chamber {} not hollow ({})",
                    c.id,
                    vol.at(p).d
                );
                // under the floor: rock (levels stack, so only just under it)
                if !c.pool {
                    let below = c.center() - Vec3::Y * 0.7;
                    let near: Vec<_> = vol
                        .shapes_near(below, below, 0.0)
                        .iter()
                        .map(|i| {
                            let sh = &vol.shapes[*i as usize];
                            let (d, f) = sh.prim.dist_floor(below);
                            format!(
                                "{:?} d={d:.2} floor={f:.2} {}",
                                sh.op,
                                match &sh.prim {
                                    Prim::Tube { pts, .. } => format!(
                                        "tube {:?}",
                                        pts.iter()
                                            .map(|p| (p.y * 10.0).round() / 10.0)
                                            .collect::<Vec<_>>()
                                    ),
                                    Prim::Room { c, r, floor, .. } =>
                                        format!("room c={c:?} r={r:?} floor={floor}"),
                                    _ => String::new(),
                                }
                            )
                        })
                        .collect();

                    assert!(
                        vol.at(below).d < 0.0,
                        "seed {seed} chamber {} floor ({}) {near:?}",
                        c.id,
                        vol.at(below).d
                    );
                }
            }
            // high above the mouth: just the terrain (the field is the height above it)
            let e = Vec3::from_array(caves.list[0].entrance);
            let high = Vec3::new(e.x, t.height_at(e.x, e.z) + 30.0, e.z);
            assert!(
                (vol.at(high).d - 30.0 - SINK).abs() < 0.15,
                "{}",
                vol.at(high).d
            );
            // the mouth opens the ground
            assert!(!vol.holes.is_empty(), "seed {seed}: no holes");
        }
    }

    #[test]
    fn ways_through_stay_clear() {
        let t = hill();
        for seed in 0..8 {
            let mut caves = sample_cave(&t, CaveKind::Cave, seed);
            let c = &mut caves.list[0];
            c.features.formations = true;
            c.features.rubble = true;
            c.dungeon = Some(Dungeon {
                name: "D".into(),
                chamber: 0,
                door: [0.0; 3],
                door_yaw: 0.0,
                instancing: Instancing::Party,
            });
            layout::layout(c, &t);
            let vol = compile(&caves, &t);
            let c = &caves.list[0];
            // head height over every tunnel's floor line
            for tn in &c.tunnels {
                for w in tn.pts.windows(2) {
                    let (a, b) = (Vec3::from_array(w[0]), Vec3::from_array(w[1]));
                    for k in 0..=8 {
                        let p = a.lerp(b, k as f32 / 8.0) + Vec3::Y * 1.2;
                        assert!(
                            vol.at(p).d > 0.0,
                            "seed {seed}: tunnel {}–{} blocked at {p}",
                            tn.a,
                            tn.b
                        );
                    }
                }
            }
            // the walk from the middle of its chamber up to the dungeon door
            let dg = c.dungeon.as_ref().expect("a door");
            let m = c
                .chambers
                .iter()
                .find(|ch| ch.id == dg.chamber)
                .expect("door chamber")
                .center();
            let door = Vec3::from_array(dg.door);
            for k in 0..=9 {
                let q = m.lerp(door, k as f32 / 10.0);
                let p = Vec3::new(q.x, door[1] + 1.5, q.z);
                assert!(
                    vol.at(p).d > 0.0,
                    "seed {seed}: the way to the door blocked at {p}"
                );
            }
        }
    }

    /// The floor under `top` (probing down from a little over it).
    fn floor_below(vol: &Volume, top: Vec3) -> Option<f32> {
        let mut y = top.y + 1.4;
        if vol.at(Vec3::new(top.x, y, top.z)).d <= 0.0 {
            return None;
        }
        while y > top.y - 3.0 {
            if vol.at(Vec3::new(top.x, y - 0.05, top.z)).d <= 0.0 {
                return Some(y);
            }
            y -= 0.05;
        }
        None
    }

    #[test]
    fn floors_have_no_steps() {
        let t = hill();
        for (size, levels, seeds) in [
            (CaveSize::Medium, 2, 8),
            (CaveSize::Large, 3, 8),
            (CaveSize::Small, 1, 8),
            (CaveSize::Huge, 2, 4),
            (CaveSize::Giant, 2, 3),
            (CaveSize::Colossal, 1, 2),
        ] {
            for seed in 0..seeds {
                let mut caves = sample_cave(&t, CaveKind::Cave, seed);
                let c = &mut caves.list[0];
                c.size = size;
                c.levels = levels;
                layout::layout(c, &t);
                let vol = compile(&caves, &t);
                for tn in &caves.list[0].tunnels {
                    let pts: Vec<Vec3> = tn.pts.iter().map(|p| Vec3::from_array(*p)).collect();
                    let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
                    let mut s0 = 0.0;
                    let mut last: Option<f32> = None;
                    for w in pts.windows(2) {
                        let len = w[0].distance(w[1]);
                        let n = ((len / 0.25).ceil() as usize).max(1);
                        for k in 0..n {
                            let q = w[0].lerp(w[1], k as f32 / n as f32);
                            let s = s0 + len * k as f32 / n as f32;
                            let mut calm = 1.0;
                            if tn.a == OUTSIDE {
                                calm *= smoothstep(4.0, 14.0, s);
                            }
                            if tn.b == OUTSIDE {
                                calm *= smoothstep(4.0, 14.0, total - s);
                            }
                            let top = Vec3::new(q.x, q.y + relief(q.x, q.z) * calm, q.z);
                            let f = floor_below(&vol, top).unwrap_or_else(|| {
                                panic!(
                                    "{size:?} seed {seed}: no floor under tunnel {}–{} at {top}",
                                    tn.a, tn.b
                                )
                            });
                            // (the floor's own roughness is about ±0.5 m)
                            assert!(
                                (f - top.y).abs() < 0.75,
                                "{size:?} seed {seed}: tunnel {}–{} floor {f:.2} off its line {:.2} at {top} ({s:.1} m along)",
                                tn.a,
                                tn.b,
                                top.y
                            );
                            // (a walker climbs any slope in a cave but not a ledge over 0.45 m: no
                            // more than 0.35 m over a quarter metre, ~54°)
                            if let Some(l) = last {
                                assert!(
                                    (f - l).abs() < 0.35,
                                    "{size:?} seed {seed}: a step of {:.2} m in tunnel {}–{} at {top}",
                                    f - l,
                                    tn.a,
                                    tn.b
                                );
                            }
                            last = Some(f);
                        }
                        s0 += len;
                    }
                }
            }
        }
    }

    #[test]
    fn waterfalls_pour_into_their_pools() {
        let t = hill();
        let mut n = 0;
        for seed in 0..5 {
            let mut caves = sample_cave(&t, CaveKind::Cave, seed);
            let c = &mut caves.list[0];
            c.size = CaveSize::Giant;
            c.levels = 1;
            c.features.waterfalls = true;
            c.features.pools = true;
            layout::layout(c, &t);
            let vol = compile(&caves, &t);
            for f in crate::caves::dress::fall_sites(&caves.list[0]) {
                n += 1;
                // the lip is open (its notch carved), the fall lands where the pool holds water
                assert!(
                    vol.at(f.lip).d > 0.0,
                    "seed {seed}: the lip at {} is in the rock",
                    f.lip
                );
                let floor = floor_below(&vol, Vec3::new(f.land.x, f.level + 0.5, f.land.z))
                    .expect("a floor under the landing");
                assert!(
                    floor < f.level - 0.1,
                    "seed {seed}: lands on dry floor {floor:.2} (water at {:.2})",
                    f.level
                );
            }
        }
        assert!(n > 0, "no waterfalls at all");
    }

    #[test]
    fn every_rim_holds_its_pool() {
        let t = hill();
        let mut seen = std::collections::HashSet::new();
        for style in RimStyle::ALL {
            let mut pools = 0;
            for seed in 0..4 {
                let mut caves = sample_cave(&t, CaveKind::Cave, seed);
                let c = &mut caves.list[0];
                c.size = CaveSize::Large;
                c.features.pools = true;
                layout::layout(c, &t);
                for ch in c.chambers.iter_mut() {
                    ch.rim = style;
                }
                let vol = compile(&caves, &t);
                let c = &caves.list[0];
                for ch in c.chambers.iter().filter(|x| x.pool) {
                    let Some((pc, pr)) = crate::caves::dress::pool_of(c, ch) else {
                        continue;
                    };
                    pools += 1;
                    seen.insert(c.rim_of(ch));
                    let level = crate::caves::dress::pool_level(ch);
                    // all round, somewhere out from the water the ground rises over its line (the
                    // dam, or the floor itself)
                    for k in 0..48 {
                        let u = Vec2::from_angle(k as f32 / 48.0 * std::f32::consts::TAU);
                        let top = (0..12)
                            .map(|j| {
                                let q = pc + u * pr * (0.95 + j as f32 * 0.05);
                                floor_below(&vol, Vec3::new(q.x, level + 0.8, q.y))
                                    .unwrap_or(f32::MAX)
                            })
                            .fold(f32::MIN, f32::max);
                        assert!(
                            top > level + 0.03,
                            "{style:?} ({:?}) seed {seed} chamber {}: no rim that way, the ground stays at {top:.2} (water {level:.2})",
                            c.rim_of(ch),
                            ch.id
                        );
                    }
                }
            }
            assert!(pools > 0, "{style:?}: no pools to try");
        }
        // Auto gives the pools of a cave different lips
        assert!(seen.len() >= 4, "only {seen:?}");
    }

    #[test]
    fn every_tunnel_is_walkable_wide() {
        let t = hill();
        for theme in [
            Theme::Limestone,
            Theme::Ice,
            Theme::Sandstone,
            Theme::Crystal,
            Theme::Mine,
        ] {
            for size in [CaveSize::Small, CaveSize::Medium] {
                for seed in 0..5 {
                    let mut caves = sample_cave(&t, CaveKind::Cave, seed);
                    let c = &mut caves.list[0];
                    c.theme = theme;
                    c.size = size;
                    layout::layout(c, &t);
                    let vol = compile(&caves, &t);
                    for tn in &caves.list[0].tunnels {
                        let pts: Vec<Vec3> = tn.pts.iter().map(|p| Vec3::from_array(*p)).collect();
                        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
                        let mut s0 = 0.0;
                        for w in pts.windows(2) {
                            let (a, b) = (w[0], w[1]);
                            let len = a.distance(b);
                            let side =
                                Vec3::new(-(b.z - a.z), 0.0, b.x - a.x).normalize_or(Vec3::X);
                            for k in 0..=6 {
                                let s = s0 + len * k as f32 / 6.0;
                                // (where a tunnel opens outside its floor eases off: skip that)
                                if (tn.a == OUTSIDE && s < 15.0)
                                    || (tn.b == OUTSIDE && total - s < 15.0)
                                {
                                    continue;
                                }
                                let c = a.lerp(b, k as f32 / 6.0);
                                for lat in [-1.0f32, 0.0, 1.0] {
                                    // (clearance over the floor actually there: on a bending
                                    // ramp a metre aside can be higher or lower)
                                    let q = c + side * lat;
                                    let Some(f) = floor_below(
                                        &vol,
                                        Vec3::new(q.x, c.y + relief(q.x, q.z), q.z),
                                    ) else {
                                        panic!(
                                            "{theme:?} {size:?} seed {seed}: {:?} tunnel {}–{} no room at {q} ({lat} m aside)",
                                            tn.profile, tn.a, tn.b
                                        );
                                    };
                                    for up in [0.4f32, 1.2, 2.3] {
                                        let p = Vec3::new(q.x, f + up, q.z);
                                        assert!(
                                            vol.at(p).d > 0.0,
                                            "{theme:?} {size:?} seed {seed}: {:?} tunnel {}–{} tight at {p} ({lat} m aside, {up} m up)",
                                            tn.profile,
                                            tn.a,
                                            tn.b
                                        );
                                    }
                                }
                            }
                            s0 += len;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pits_and_caverns() {
        let t = hill();
        let caves = sample_cave(&t, CaveKind::Pit, 2);
        let vol = compile(&caves, &t);
        assert!(!vol.holes.is_empty());
        let caves = sample_cave(&t, CaveKind::Cavern, 2);
        let vol = compile(&caves, &t);
        assert!(vol.holes.is_empty(), "a cavern stays sealed");
    }
}

#[cfg(test)]
mod arch_tests {
    use super::*;

    #[test]
    fn arch_has_an_opening() {
        let mut t = crate::terrain::Terrain::new(64, 4.0, 0.0);
        for h in t.heights.iter_mut() {
            *h = 10.0;
        }
        let cave = Cave {
            id: 1,
            name: "A".into(),
            kind: CaveKind::Arch,
            size: CaveSize::Medium,
            levels: 1,
            up: false,
            theme: Theme::Limestone,
            resolved: Theme::Auto,
            features: Features::default(),
            darkness: Darkness::Dim,
            seed: 31,
            entrance: [0.0, 10.0, 0.0],
            heading: 0.0,
            mouth: Mouth::None,
            chambers: vec![],
            tunnels: vec![],
            dungeon: None,
            arch: Some(ArchSpec {
                a: [-9.0, 0.0],
                b: [9.0, 0.0],
                height: 8.4,
                thickness: 3.2,
            }),
            props: vec![],
        };
        let caves = Caves {
            list: vec![cave],
            strokes: vec![],
            next_id: 1,
        };
        let vol = compile(&caves, &t);
        let s = &vol.shapes[0];
        let mut col = String::new();
        for k in 0..20 {
            let p = Vec3::new(0.0, 10.0 + k as f32, 0.0);
            col += &format!("{:.0}:{:.1}/{:.1} ", p.y, vol.at(p).d, s.prim.dist(p));
        }
        // under the middle of the span there is room to walk through
        for y in [11.0, 13.0, 15.0, 17.0] {
            assert!(
                vol.at(Vec3::new(0.0, y, 0.0)).d > 0.0,
                "solid at {y}: {col}"
            );
        }
        // and rock overhead
        assert!(
            vol.at(Vec3::new(0.0, 10.0 + 8.4 + 1.6, 0.0)).d < 0.0,
            "{col}"
        );
    }
}
