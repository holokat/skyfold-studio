//! Floating rocks for alien and sky worlds (`AssetKind::FloatingRock`): chunks of land torn loose
//! and hanging in the air, and drifting asteroid boulders.
//!
//! * Floating islands (variants 0–2): a broken, gently domed top over a jagged underside that
//!   tapers to a point — a rounded inverted cone of eroded strata, ribbed by the ridges and gullies
//!   rain cuts into hanging rock, with shards dangling from it and loose boulders on top.
//! * Asteroid boulders (3, 4): lumpy, cratered and cut by flat fracture faces where pieces split off.
//! * A broken shard (5): a long split-off slab of the same rock.
//!
//! The shapes are procedural and wear the photoscanned rock texture (`photoscan::DRESSED`), its
//! tiles sized to the object (`rock_material`). Their origin is their lowest point, so a placed
//! rock's height above the ground is the air under it (`hover_for` is the default gap). They bob,
//! sway and turn slowly where they hang (`drift`): a transform per render entity, from the object's
//! stored placement, so picking, saving and undo see the still pose.

use std::collections::HashMap;

use bevy::prelude::*;

use super::formations::{fbm3, finish, hash3, noise3};
use crate::noise::Rng;
use crate::objects::{ObjectStore, PlacedObject};
use crate::procgen::meshes::{AssetKind, MeshData};

/// Variants: 0–2 floating islands, 3–4 asteroid boulders, 5 a broken shard.
pub const VARIANTS: u32 = 6;

/// The mesh of a floating rock variant at level of detail `lod` (0 near, 1 far), its lowest point
/// at y = 0 (both levels shifted alike, so the far level doesn't jump).
pub fn floating_rock(variant: u32, lod: u32) -> MeshData {
    let v = variant % VARIANTS;
    let mut m = shape(v, lod.min(1));
    let near = if lod == 0 { None } else { Some(shape(v, 0)) };
    let lowest = near.as_ref().unwrap_or(&m).bounds().0[1];
    for p in m.positions.iter_mut() {
        p[1] -= lowest;
    }
    m
}

fn shape(v: u32, lod: u32) -> MeshData {
    let (pos, idx) = match v {
        0..=2 => island(v, lod),
        3 | 4 => asteroid(
            &mut Rng::new(0xa57e_u64 + v as u64 * 7_919),
            4.2,
            Vec3::new(1.0, 0.78, 0.9),
            lod,
        ),
        _ => asteroid(
            &mut Rng::new(0x5a4d_u64 + v as u64 * 104_729),
            3.2,
            Vec3::new(0.62, 1.9, 0.42),
            lod,
        ),
    };
    let (pos, idx) = weld(pos, idx);
    finish(pos, idx, [0.42, 0.4, 0.38])
}

/// The air under a floating rock of size `scale` (m between the ground and its lowest point) when
/// it's placed on a spot without a height: a few metres under a pebble, tens under an island.
/// 0 for every kind that stands on the ground.
pub fn hover_for(kind: Option<AssetKind>, scale: f32) -> f32 {
    if kind == Some(AssetKind::FloatingRock) {
        4.0 + 9.0 * scale.max(0.05).powf(0.7)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------------------------
// Floating islands

struct Island {
    /// radius of the top (m)
    r: f32,
    /// from the rim down to the tip (m)
    depth: f32,
    seed: u32,
    /// strata thickness (m)
    layer: f32,
    /// cross-section lobes: (count, amplitude, phase)
    lobes: [(f32, f32, f32); 3],
    /// where the tip hangs, off the top's centre (m)
    tip: Vec2,
    /// the beds' dip: their height changes by this much per metre across (x, z)
    dip: Vec2,
    /// fracture faces down the sides: (outward direction, distance from the axis as a share
    /// of the radius)
    cuts: Vec<(Vec2, f32)>,
}

impl Island {
    fn new(v: u32) -> Self {
        let mut rng = Rng::new(0xf10a_u64 + v as u64 * 31_337);
        let r = rng.range(5.5, 7.2);
        let mut lobe = |n: f32, lo: f32, hi: f32| {
            (
                n + rng.below(2) as f32,
                rng.range(lo, hi),
                rng.f32() * std::f32::consts::TAU,
            )
        };
        let lobes = [
            lobe(2.0, 0.1, 0.2),
            lobe(4.0, 0.04, 0.09),
            lobe(7.0, 0.02, 0.05),
        ];
        let a = rng.f32() * std::f32::consts::TAU;
        let tip = Vec2::new(a.cos(), a.sin()) * r * rng.range(0.1, 0.35);
        let d = rng.f32() * std::f32::consts::TAU;
        let dip = Vec2::new(d.cos(), d.sin()) * rng.range(0.08, 0.2);
        let cuts = (0..3 + rng.below(3))
            .map(|k| {
                let a = k as f32 * 1.9 + rng.range(-0.5, 0.5);
                (Vec2::new(a.cos(), a.sin()), rng.range(0.72, 0.9))
            })
            .collect();
        Island {
            r,
            depth: r * rng.range(1.4, 2.1),
            seed: rng.next_u32(),
            layer: rng.range(0.7, 1.3),
            lobes,
            tip,
            dip,
            cuts,
        }
    }

    /// 0 at the tip … 1 at the rim, for height `y` (the rim is y = 0).
    fn s(&self, y: f32) -> f32 {
        ((y + self.depth) / self.depth).clamp(0.0, 1.0)
    }

    /// The axis drifts from the top's centre to the tip.
    fn axis(&self, y: f32) -> Vec2 {
        let d = 1.0 - self.s(y);
        self.tip * d.powf(1.5)
            + Vec2::new(
                fbm3(Vec3::new(y * 0.12, 0.0, 0.0), 2, self.seed ^ 11),
                fbm3(Vec3::new(0.0, y * 0.12, 5.0), 2, self.seed ^ 13),
            ) * self.r
                * 0.25
                * d
    }

    /// Radius of the underside at height `y` and angle `a`.
    fn radius_at(&self, y: f32, a: f32, fine: bool) -> f32 {
        let s = self.s(y);
        // a rounded inverted cone, fullest just under the rim, the rim's broken edge drawn in
        let mut p = s.powf(0.62) * (1.0 + 0.1 * (1.0 - ((s - 0.82) / 0.18).powi(2)).max(0.0));
        p *= 1.0 - 0.14 * ((s - 0.93) / 0.07).max(0.0).powi(2);
        let mut r = self.r * p;
        let mut lobe = 1.0;
        for (n, amp, ph) in self.lobes {
            lobe += amp * (a * n + ph + s * 1.7).cos();
        }
        r *= lobe;
        // strata: dipping, wavy beds of uneven thickness, each sticking out or eroded back,
        // a small ledge at its top; the softer beds cut back into notches
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let along = self.dip.dot(Vec2::new(d.x, d.z)) * self.r;
        let wave = 0.9 * fbm3(d * 1.2 + Vec3::Y * (y * 0.05), 2, self.seed ^ 0x77);
        let b = (y + along) / self.layer + wave;
        let bed = b.floor();
        let hard = hash3(bed as i32, 0, 0, self.seed ^ 0x51);
        let f = b - bed;
        let soft = (hard < 0.25) as u8 as f32;
        r *= 1.0 + 0.12 * (hard - 0.5) + 0.07 * (f * f - 0.33)
            - 0.08 * soft * (1.0 - (2.0 * f - 1.0).powi(2));
        // ridges and gullies running down the hanging rock to the tip
        let fin = 1.0
            - (2.0 * noise3(Vec3::new(d.x * 3.2, y * 0.16, d.z * 3.2), self.seed ^ 9) - 1.0).abs();
        r *= 1.0 + 0.18 * (fin - 0.5) * (1.0 - 0.5 * s);
        // rock relief: big lumps, fine roughness
        let q = d * self.r + Vec3::Y * y;
        r *= 1.0
            + 0.22 * fbm3(q * (0.8 / self.r), 3, self.seed)
            + 0.08 * fbm3(q * (2.6 / self.r), 3, self.seed ^ 41);
        if fine {
            r *= 1.0 + 0.035 * fbm3(q * 0.9, 3, self.seed ^ 5);
        }
        // fracture faces: where a slab split off the side, flat and steep (the upper part)
        let c = Vec2::new(d.x, d.z);
        for &(n, o) in &self.cuts {
            let k = c.dot(n);
            let lim =
                o * self.r * (0.3 + 0.7 * s) * (1.0 + 0.04 * fbm3(q * 0.7, 2, self.seed ^ 23));
            if k > 0.2 && r * k > lim {
                r = lim / k;
            }
        }
        r.max(0.02)
    }

    /// The rim's broken edge: how far a spot near the top (s > 0.8) rises or drops (m).
    fn rim(&self, a: f32, s: f32) -> f32 {
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let t = ((s - 0.8) / 0.2).clamp(0.0, 1.0);
        self.r * 0.14 * fbm3(d * 2.2, 3, self.seed ^ 31) * t * t
    }

    /// The top's relief at (x, z): a gentle dome with knolls and hollows.
    fn top_rise(&self, x: f32, z: f32, f: f32) -> f32 {
        let q = Vec3::new(x, 0.0, z);
        self.r
            * (0.05 * (1.0 - f * f)
                + 0.06 * fbm3(q * (1.4 / self.r), 3, self.seed ^ 17) * (1.0 - f).sqrt()
                + 0.012 * fbm3(q * 1.3, 2, self.seed ^ 19))
    }
}

fn island(v: u32, lod: u32) -> (Vec<Vec3>, Vec<u32>) {
    let s = Island::new(v);
    let fine = lod == 0;
    let (rings, sides, caps) = if fine { (60, 56, 7) } else { (16, 16, 2) };
    let mut pos: Vec<Vec3> = vec![];
    let mut idx: Vec<u32> = vec![];
    // the tip
    let tip_y = -s.depth;
    let c = s.axis(tip_y);
    pos.push(Vec3::new(c.x, tip_y, c.y));
    for i in 0..=rings {
        // (rings bunch up towards the rim, where the shape turns)
        let t = 0.03 + 0.97 * (i as f32 / rings as f32).powf(0.85);
        let y = -s.depth + s.depth * t;
        let c = s.axis(y);
        for j in 0..sides {
            let a = j as f32 / sides as f32 * std::f32::consts::TAU;
            let r = s.radius_at(y, a, fine);
            let lift = s.rim(a, s.s(y));
            pos.push(Vec3::new(c.x + a.cos() * r, y + lift, c.y + a.sin() * r));
        }
    }
    let sides_u = sides as u32;
    for j in 0..sides_u {
        idx.extend_from_slice(&[0, 1 + j, 1 + (j + 1) % sides_u]);
    }
    for i in 0..rings as u32 {
        for j in 0..sides_u {
            let a = 1 + i * sides_u + j;
            let b = 1 + i * sides_u + (j + 1) % sides_u;
            let (c, d) = (a + sides_u, b + sides_u);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // the top: shrinking rings over the broken, domed surface
    let rim = 1 + rings as u32 * sides_u;
    let c0 = s.axis(0.0);
    let mut prev = rim;
    for k in 1..=caps {
        let f = 1.0 - k as f32 / (caps + 1) as f32;
        let start = pos.len() as u32;
        for j in 0..sides_u {
            let q = pos[(rim + j) as usize];
            let rel = Vec2::new(q.x - c0.x, q.z - c0.y) * f;
            let (x, z) = (c0.x + rel.x, c0.y + rel.y);
            pos.push(Vec3::new(x, q.y * f + s.top_rise(x, z, f), z));
        }
        for j in 0..sides_u {
            let (a, b) = (prev + j, prev + (j + 1) % sides_u);
            let (c, d) = (start + j, start + (j + 1) % sides_u);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
        prev = start;
    }
    let apex = pos.len() as u32;
    pos.push(Vec3::new(c0.x, s.top_rise(c0.x, c0.y, 0.0), c0.y));
    for j in 0..sides_u {
        idx.extend_from_slice(&[prev + j, apex, prev + (j + 1) % sides_u]);
    }
    // shards dangling from the underside, and loose boulders lying on top
    let mut rng = Rng::new(s.seed as u64 ^ 0xd4a6);
    for k in 0..2 + rng.below(3) {
        let at = rng.range(0.3, 0.62);
        let y = -s.depth * (1.0 - at);
        let a = k as f32 * 2.4 + rng.range(-0.4, 0.4);
        let c = s.axis(y);
        let r = s.radius_at(y, a, false);
        let base = Vec3::new(c.x + a.cos() * r * 0.75, y, c.y + a.sin() * r * 0.75);
        let out = Vec3::new(a.cos(), 0.0, a.sin());
        let dir = (-Vec3::Y + out * rng.range(0.05, 0.35)).normalize();
        let len = s.depth * rng.range(0.3, 0.55);
        spike(
            &mut pos,
            &mut idx,
            base,
            dir,
            len,
            r * rng.range(0.25, 0.4),
            rng.next_u32(),
            fine,
        );
    }
    if fine {
        for _ in 0..2 {
            let a = rng.f32() * std::f32::consts::TAU;
            let d = rng.range(0.1, 0.6) * s.r;
            let (x, z) = (c0.x + a.cos() * d, c0.y + a.sin() * d);
            let size = rng.range(0.5, 1.1);
            let (bp, bi) = asteroid(&mut rng, size, Vec3::new(1.0, 0.6, 0.85), 2);
            let at = Vec3::new(x, s.top_rise(x, z, d / s.r) + size * 0.25, z);
            let n = pos.len() as u32;
            pos.extend(bp.into_iter().map(|p| p + at));
            idx.extend(bi.into_iter().map(|i| i + n));
        }
    }
    (pos, idx)
}

/// A jagged rock shard from `base` along `dir`: a lathe of eroded beds tapering to a point, its
/// open root hidden in the rock it hangs from.
#[allow(clippy::too_many_arguments)]
fn spike(
    pos: &mut Vec<Vec3>,
    idx: &mut Vec<u32>,
    base: Vec3,
    dir: Vec3,
    len: f32,
    r0: f32,
    seed: u32,
    fine: bool,
) {
    let (rings, sides) = if fine { (16, 12) } else { (4, 6) };
    let rot = Quat::from_rotation_arc(Vec3::Y, dir);
    let start = pos.len() as u32;
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let y = t * len;
        let bend = Vec2::new(
            fbm3(Vec3::new(y * 0.3, 0.0, 1.0), 2, seed),
            fbm3(Vec3::new(1.0, y * 0.3, 0.0), 2, seed ^ 3),
        ) * r0
            * t;
        for j in 0..sides {
            let a = j as f32 / sides as f32 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            let bed = (y / (r0 * 0.6)).floor();
            let mut r = r0 * (1.0 - t).powf(0.8);
            r *= 1.0 + 0.12 * (hash3(bed as i32, 0, 0, seed) - 0.5);
            r *= 1.0 + 0.3 * fbm3(d * 1.5 + Vec3::Y * y * (1.2 / r0), 3, seed ^ 7);
            pos.push(base + rot * Vec3::new(bend.x + d.x * r, y, bend.y + d.z * r));
        }
    }
    for i in 0..rings as u32 {
        for j in 0..sides as u32 {
            let a = start + i * sides as u32 + j;
            let b = start + i * sides as u32 + (j + 1) % sides as u32;
            let (c, d) = (a + sides as u32, b + sides as u32);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let last = start + rings as u32 * sides as u32;
    let apex = pos.len() as u32;
    pos.push(base + dir * len * 1.04);
    for j in 0..sides as u32 {
        idx.extend_from_slice(&[last + j, apex, last + (j + 1) % sides as u32]);
    }
}

// ---------------------------------------------------------------------------------------------
// Asteroid boulders

/// A lumpy boulder of radius `r` (m) squashed by `squash`: a subdivided cube pushed out to a
/// sphere, displaced by big lumps and fine grain, cratered, and cut by a few flat fracture faces.
/// `lod` 0 near, 1 far, 2 small (rubble on an island's top). Unwelded at the cube's edges.
fn asteroid(rng: &mut Rng, r: f32, squash: Vec3, lod: u32) -> (Vec<Vec3>, Vec<u32>) {
    let n = match lod {
        0 => 22,
        1 => 6,
        _ => 7,
    };
    let seed = rng.next_u32();
    let dir = |rng: &mut Rng| {
        Vec3::new(
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
        )
        .normalize_or(Vec3::X)
    };
    let craters: Vec<(Vec3, f32, f32)> = (0..2 + rng.below(4))
        .map(|_| (dir(rng), rng.range(0.25, 0.55), rng.range(0.06, 0.14)))
        .collect();
    let cuts: Vec<(Vec3, f32)> = (0..5 + rng.below(5))
        .map(|_| (dir(rng), rng.range(0.45, 0.72)))
        .collect();
    let fine = lod != 1;
    let surface = |p: Vec3| -> Vec3 {
        let u = p.normalize();
        let mut k = 1.0 + 0.28 * fbm3(u * 1.3, 4, seed) + 0.07 * fbm3(u * 3.5, 3, seed ^ 2);
        for &(c, size, depth) in &craters {
            let d = u.dot(c).clamp(-1.0, 1.0).acos() / size;
            if d < 1.3 {
                // a bowl with a low raised rim
                k -= depth * (1.0 - d * d).max(0.0)
                    - depth * 0.35 * (1.0 - ((d - 1.05) / 0.25).powi(2)).max(0.0);
            }
        }
        if fine {
            k += 0.06 * fbm3(u * 6.0, 3, seed ^ 5);
        }
        let mut q = u * squash * k;
        // fracture faces: whatever sticks out past a plane is sheared off flat
        for &(c, o) in &cuts {
            let along = q.dot(c);
            let lim = o * (squash * c).length() * (1.0 + 0.03 * fbm3(q * 3.0, 2, seed ^ 9));
            if along > lim {
                q -= c * (along - lim);
            }
        }
        q * r
    };
    let faces = [
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
    ];
    let (mut pos, mut idx) = (vec![], vec![]);
    for (nrm, u, v) in faces {
        let start = pos.len() as u32;
        for j in 0..=n {
            for i in 0..=n {
                let (a, b) = (
                    i as f32 / n as f32 * 2.0 - 1.0,
                    j as f32 / n as f32 * 2.0 - 1.0,
                );
                pos.push(surface(nrm + u * a + v * b));
            }
        }
        let w = n as u32 + 1;
        for j in 0..n as u32 {
            for i in 0..n as u32 {
                let p00 = start + j * w + i;
                let (p10, p01, p11) = (p00 + 1, p00 + w, p00 + w + 1);
                idx.extend_from_slice(&[p00, p10, p11, p00, p11, p01]);
            }
        }
    }
    (pos, idx)
}

/// Merges vertices at the same spot (the cube faces' shared edges) and drops degenerate
/// triangles, so normals smooth across the seams.
pub(crate) fn weld(pos: Vec<Vec3>, idx: Vec<u32>) -> (Vec<Vec3>, Vec<u32>) {
    let key = |p: Vec3| {
        [
            (p.x * 1e4).round() as i64,
            (p.y * 1e4).round() as i64,
            (p.z * 1e4).round() as i64,
        ]
    };
    let mut map: HashMap<[i64; 3], u32> = HashMap::new();
    let mut out: Vec<Vec3> = vec![];
    let remap: Vec<u32> = pos
        .iter()
        .map(|p| {
            *map.entry(key(*p)).or_insert_with(|| {
                out.push(*p);
                out.len() as u32 - 1
            })
        })
        .collect();
    let mut tris = vec![];
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a != b && b != c && a != c {
            tris.extend_from_slice(&[a, b, c]);
        }
    }
    (out, tris)
}

// ---------------------------------------------------------------------------------------------
// Drift

/// How the floating rocks move where they hang (MCP `objects_hover` › motion): the bob's height
/// and the turn's speed, × their natural amount (0 holds them still). For the session.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct FloatMotion {
    pub bob: f32,
    pub spin: f32,
}

impl Default for FloatMotion {
    fn default() -> Self {
        Self {
            bob: 1.0,
            spin: 1.0,
        }
    }
}

/// On a floating rock's render entity: it drifts (`drift`).
#[derive(Component)]
pub struct Drifts;

/// Marks the render entities of floating rocks as they're spawned.
pub fn mark_drifting(
    mut commands: Commands,
    store: Res<ObjectStore>,
    added: Query<(Entity, &PlacedObject), Added<PlacedObject>>,
) {
    for (e, o) in &added {
        if store
            .items
            .get(&o.id)
            .is_some_and(|p| p.asset == crate::objects::builtin_id(AssetKind::FloatingRock))
        {
            commands.entity(e).insert(Drifts);
        }
    }
}

/// Floating rocks bob, sway and turn slowly about their stored pose: the bigger, the slower and
/// the further they move, each on its own phase.
pub fn drift(
    time: Res<Time>,
    motion: Res<FloatMotion>,
    store: Res<ObjectStore>,
    mut q: Query<(&PlacedObject, &mut Transform), With<Drifts>>,
) {
    let t = time.elapsed_secs();
    for (o, mut tf) in &mut q {
        let Some(p) = store.items.get(&o.id) else {
            continue;
        };
        let size = p.scale.max(0.05);
        let phase = (crate::noise::hash_u32(o.id as u32) & 0xffff) as f32 / 65535.0 * 100.0;
        // (a pebble bobs ~0.3 m every 7 s, an island a metre or two over half a minute)
        let period = 7.0 * size.powf(0.35);
        let w = std::f32::consts::TAU / period;
        let bob = motion.bob * 0.3 * size.powf(0.6) * (t * w + phase).sin();
        let sway = motion.bob * 0.012 / size.powf(0.25);
        let turn = motion.spin * 0.05 / size.powf(0.5) * t;
        tf.translation = p.translation() + Vec3::Y * bob;
        tf.rotation = Quat::from_rotation_y(turn + phase)
            * Quat::from_rotation_x(sway * (t * w * 0.7 + phase * 1.3).sin())
            * Quat::from_rotation_z(sway * (t * w * 0.53 + phase * 0.7).cos())
            * p.rotation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant builds a sound mesh resting on y = 0, its far level much lighter.
    #[test]
    fn floating_rocks_build() {
        for v in 0..VARIANTS {
            let near = floating_rock(v, 0);
            let far = floating_rock(v, 1);
            for (m, what) in [(&near, "near"), (&far, "far")] {
                assert!(
                    !m.indices.is_empty() && m.indices.len() % 3 == 0,
                    "{v} {what}: no triangles"
                );
                assert!(
                    m.indices.iter().all(|i| (*i as usize) < m.positions.len()),
                    "{v} {what}: index out of range"
                );
                assert!(
                    m.positions.iter().all(|p| p.iter().all(|x| x.is_finite())),
                    "{v} {what}: non-finite"
                );
            }
            let (mn, mx) = near.bounds();
            assert!(mn[1].abs() < 1e-3, "{v}: lowest point at {}", mn[1]);
            assert!(mx[1] > 3.0 && mx[1] < 25.0, "{v}: {} m tall", mx[1]);
            assert!(far.bounds().0[1] > -1.5, "{v}: far level hangs lower");
            assert!(
                far.triangle_count() * 5 < near.triangle_count() && near.triangle_count() < 12_000,
                "{v}: {} / {} triangles",
                near.triangle_count(),
                far.triangle_count()
            );
        }
    }
}
