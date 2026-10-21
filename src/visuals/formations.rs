//! Tall rock formations and crystals.
//!
//! Rock pinnacles (`RockSpire`), hoodoos and sea stacks are assembled offline from CC0 photoscans
//! (cliff facades and boulders, `tools/process_formations.py`) and load with the other scanned
//! props (`crate::photoscan`). The procedural shapes here are their fallback when those files are
//! missing: a lathe around a wandering vertical axis whose radius follows the formation's profile,
//! cross-section lobes and joints, strata and 3D noise.
//!
//! * Rock pinnacle: a broad-based, tapering spire of layered rock with a jagged tip.
//! * Hoodoo: a column of soft, banded rock pinched at the waist under a harder, overhanging cap.
//! * Sea stack: a massive, steep pillar cut by vertical joints and strata, with a wave-cut notch.
//! * Crystals: clusters of hexagonal prisms with pyramid tips radiating from a rocky base, glossy
//!   and glowing from within (an emissive gradient towards the tips), for magical places.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::noise::Rng;
use crate::objects::{FoliageExt, FoliageMaterial};
use crate::procgen::meshes::{AssetKind, MeshData};
use crate::trees::{TreeParts, TreeVariant};

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let h = crate::noise::hash2(x, y, seed) ^ (z as u32).wrapping_mul(0x9e37_79b9);
    (crate::noise::hash_u32(h) & 0xffff) as f32 / 65535.0
}

/// Smooth 3D value noise in 0..1.
fn noise3(p: Vec3, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx, dy, dz| hash3(x + dx, y + dy, z + dz, seed);
    l(
        l(
            l(c(0, 0, 0), c(1, 0, 0), u.x),
            l(c(0, 1, 0), c(1, 1, 0), u.x),
            u.y,
        ),
        l(
            l(c(0, 0, 1), c(1, 0, 1), u.x),
            l(c(0, 1, 1), c(1, 1, 1), u.x),
            u.y,
        ),
        u.z,
    )
}

/// Fractal noise in about -0.5..0.5.
fn fbm3(p: Vec3, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 0.5, 0.0, p);
    for o in 0..octaves {
        sum += amp * (noise3(q, seed.wrapping_add(o * 131)) - 0.5);
        norm += amp;
        amp *= 0.5;
        q = q * 2.03 + Vec3::new(1.7, 9.2, 3.1);
    }
    sum / norm
}

#[derive(Clone, Copy, PartialEq)]
enum Form {
    Pinnacle,
    Hoodoo,
    SeaStack,
}

struct Shape {
    form: Form,
    height: f32,
    radius: f32,
    seed: u32,
    /// strata thickness (m)
    layer: f32,
    /// cross-section lobes: (count, amplitude, phase)
    lobes: [(f32, f32, f32); 3],
    /// vertical joints: facets around the column (0: none)
    joints: f32,
    lean: Vec2,
    waist: f32,
}

impl Shape {
    fn new(form: Form, variant: u32) -> Self {
        let mut rng = Rng::new(0x5a1e_u64 ^ ((form as u64) << 20) ^ (variant as u64 * 7919));
        let (height, radius) = match form {
            Form::Pinnacle => {
                let h = rng.range(9.0, 15.0);
                (h, h * rng.range(0.17, 0.23))
            }
            Form::Hoodoo => {
                let h = rng.range(7.0, 13.0);
                (h, h * rng.range(0.13, 0.17))
            }
            Form::SeaStack => {
                let h = rng.range(22.0, 38.0);
                (h, h * rng.range(0.24, 0.32))
            }
        };
        let lobes = [
            (
                2.0 + rng.below(2) as f32,
                rng.range(0.08, 0.18),
                rng.f32() * std::f32::consts::TAU,
            ),
            (
                4.0 + rng.below(2) as f32,
                rng.range(0.04, 0.08),
                rng.f32() * std::f32::consts::TAU,
            ),
            (
                7.0 + rng.below(3) as f32,
                rng.range(0.02, 0.05),
                rng.f32() * std::f32::consts::TAU,
            ),
        ];
        Shape {
            form,
            height,
            radius,
            seed: rng.next_u32(),
            layer: match form {
                Form::Pinnacle => rng.range(0.7, 1.3),
                Form::Hoodoo => rng.range(0.45, 0.8),
                Form::SeaStack => rng.range(5.0, 8.5),
            },
            lobes,
            joints: match form {
                Form::SeaStack => 4.0 + rng.below(3) as f32,
                Form::Pinnacle => 4.0 + rng.below(3) as f32,
                Form::Hoodoo => 0.0,
            },
            lean: Vec2::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0))
                * height
                * rng.range(0.02, 0.06),
            waist: rng.range(0.55, 0.7),
        }
    }

    /// Height profile: radius factor at height fraction `t`.
    fn profile(&self, t: f32) -> f32 {
        match self.form {
            // a tower of jointed blocks: a flared foot, steep sides narrowing higher up, a broken
            // top of smaller blocks
            Form::Pinnacle => {
                (1.0 - 0.5 * t.powf(1.5))
                    * (1.0 + 0.6 * (1.0 - t * 4.0).max(0.0).powi(2))
                    * (1.0 - 0.35 * ((t - 0.85) / 0.15).max(0.0))
            }
            // a flared foot, the pinched waist, a little wider under the cap
            Form::Hoodoo => {
                let flare = 1.0 + 0.9 * (1.0 - t * 3.0).max(0.0).powi(2);
                let pinch =
                    1.0 - (1.0 - self.waist) * (t * std::f32::consts::PI * 0.9).sin().powf(1.4);
                flare * pinch * (1.0 - 0.15 * t)
            }
            // steep walls narrowing upwards from a flared, wave-cut foot, the rim of the flat top
            // rounded off (the top itself is the cap)
            Form::SeaStack => {
                let rim = 1.0 - 0.3 * ((t - 0.93) / 0.07).max(0.0).powi(2);
                (1.0 - 0.32 * t) * (1.0 + 0.3 * (1.0 - t * 6.0).max(0.0).powi(2)) * rim
            }
        }
    }

    /// Radius of the surface at height `y` (m) and angle `a`.
    fn radius_at(&self, y: f32, a: f32, fine: bool) -> f32 {
        let t = (y / self.height).clamp(0.0, 1.0);
        let mut r = self.radius * self.profile(t);
        // lobed cross-section, twisting slowly with height
        let mut lobe = 1.0;
        for (n, amp, ph) in self.lobes {
            lobe += amp * (a * n + ph + t * 1.3).cos();
        }
        r *= lobe;
        // vertical joints: flat facets between rounded corners
        if self.joints > 0.0 {
            let seg = std::f32::consts::TAU / self.joints;
            let off = noise3(Vec3::new(0.0, y * 0.05, 3.0), self.seed ^ 7) * seg;
            let k = ((a + off) % seg + seg) % seg - seg * 0.5;
            let facet = (seg * 0.5).cos() / k.cos();
            r *= 1.0
                + (facet - 1.0)
                    * if self.form == Form::SeaStack {
                        0.95
                    } else if self.form == Form::Pinnacle {
                        0.8
                    } else {
                        0.55
                    };
        }
        // strata: each bed sticks out or is eroded back, with a ledge at its top
        let s = y / self.layer;
        let bed = s.floor();
        let hard = hash3(bed as i32, 0, 0, self.seed ^ 0x51);
        let f = s - bed;
        let (bulge, ledge) = match self.form {
            Form::Hoodoo => (0.16, 0.08),
            Form::SeaStack => (0.12, 0.05),
            Form::Pinnacle => (0.08, 0.06),
        };
        r *= 1.0 + bulge * (hard - 0.5) + ledge * (f * f - 0.33);
        // the wave-cut notch
        if self.form == Form::SeaStack {
            r *= 1.0 - 0.16 * (1.0 - ((y - 1.3) / 1.4).powi(2)).max(0.0);
        }
        // rock relief: big lumps, vertical grooves, fine roughness
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let q = d * self.radius + Vec3::Y * y;
        let lumps = if self.form == Form::SeaStack {
            0.3
        } else {
            0.2
        };
        r *= 1.0 + lumps * fbm3(q * (0.9 / self.radius.max(1.0)), 3, self.seed);
        r *= 1.0 + 0.06 * fbm3(Vec3::new(d.x * 6.0, y * 0.06, d.z * 6.0), 2, self.seed ^ 3);
        // sea stacks: sharp vertical fins and gullies where the joints weather out
        if self.form == Form::SeaStack {
            let fin = 1.0
                - (2.0 * noise3(Vec3::new(d.x * 3.5, y * 0.035, d.z * 3.5), self.seed ^ 9) - 1.0)
                    .abs();
            r *= 1.0 + 0.14 * (fin - 0.5);
        }
        if fine {
            r *= 1.0 + 0.035 * fbm3(q * 0.9, 3, self.seed ^ 5);
        }
        r.max(0.05)
    }

    /// The axis wanders and leans a little; a pinnacle's beds sit slightly off each other, like
    /// stacked, weathered blocks.
    fn axis(&self, y: f32) -> Vec2 {
        let t = (y / self.height).clamp(0.0, 1.0);
        let mut c = self.lean * t * t
            + Vec2::new(
                fbm3(Vec3::new(y * 0.08, 0.0, 0.0), 2, self.seed ^ 11),
                fbm3(Vec3::new(0.0, y * 0.08, 5.0), 2, self.seed ^ 13),
            ) * self.radius
                * 0.25
                * t;
        if self.form == Form::Pinnacle {
            let bed = (y / self.layer).floor() as i32;
            c += Vec2::new(
                hash3(bed, 1, 0, self.seed) - 0.5,
                hash3(bed, 2, 0, self.seed) - 0.5,
            ) * self.radius
                * 0.1
                * t;
        }
        c
    }
}

/// The lathed surface with a closed, weathered top; vertex normals smoothed over the faces.
fn lathe(s: &Shape, lod: u32) -> MeshData {
    let fine = lod == 0;
    let (rings, sides) = if fine { (84, 56) } else { (24, 18) };
    // (deep enough into the ground to stand on a slope without a gap on its downhill side)
    let sink = 1.5 + s.radius * 0.4;
    let mut pos: Vec<Vec3> = vec![];
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        // (rings bunch up towards the top, where the shape turns)
        let y = -sink + (s.height + sink) * (1.0 - (1.0 - t).powf(1.15));
        let c = s.axis(y);
        for j in 0..sides {
            let a = j as f32 / sides as f32 * std::f32::consts::TAU;
            let r = s.radius_at(y, a, fine);
            pos.push(Vec3::new(c.x + a.cos() * r, y, c.y + a.sin() * r));
        }
    }
    let mut idx: Vec<u32> = vec![];
    for i in 0..rings as u32 {
        for j in 0..sides as u32 {
            let a = i * sides as u32 + j;
            let b = i * sides as u32 + (j + 1) % sides as u32;
            let (c, d) = (a + sides as u32, b + sides as u32);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // the top: a few shrinking rings domed (or jagged for a pinnacle) up to a centre point
    let top_ring = rings as u32 * sides as u32;
    let top_y = pos[top_ring as usize].y;
    let c0 = s.axis(top_y);
    let caps = if fine { 4 } else { 2 };
    let mut prev = top_ring;
    for k in 1..=caps {
        let f = 1.0 - k as f32 / (caps + 1) as f32;
        let start = pos.len() as u32;
        for j in 0..sides {
            let q = pos[(top_ring + j as u32) as usize];
            let rel = Vec2::new(q.x - c0.x, q.z - c0.y) * f;
            let rise = match s.form {
                Form::Pinnacle => {
                    s.radius
                        * 0.25
                        * (1.0 - f)
                        * (1.0 + 0.8 * fbm3(Vec3::new(rel.x, 0.0, rel.y) * 0.8, 2, s.seed ^ 17))
                }
                _ => {
                    s.radius
                        * 0.1
                        * (1.0 - f * f)
                        * (1.0 + 0.5 * fbm3(Vec3::new(rel.x, 0.0, rel.y) * 0.4, 2, s.seed ^ 17))
                }
            };
            pos.push(Vec3::new(c0.x + rel.x, top_y + rise, c0.y + rel.y));
        }
        for j in 0..sides as u32 {
            let (a, b) = (prev + j, prev + (j + 1) % sides as u32);
            let (c, d) = (start + j, start + (j + 1) % sides as u32);
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
        prev = start;
    }
    let apex = pos.len() as u32;
    let apex_rise = match s.form {
        Form::Pinnacle => s.radius * 0.45,
        _ => s.radius * 0.07,
    };
    pos.push(Vec3::new(c0.x, top_y + apex_rise, c0.y));
    for j in 0..sides as u32 {
        idx.extend_from_slice(&[prev + j, apex, prev + (j + 1) % sides as u32]);
    }
    finish(pos, idx, [0.5, 0.48, 0.45])
}

/// Smooth normals and flat colour into a `MeshData`.
fn finish(pos: Vec<Vec3>, idx: Vec<u32>, color: [f32; 3]) -> MeshData {
    let mut nrm = vec![Vec3::ZERO; pos.len()];
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let n = (b - a).cross(c - a);
        for &i in t {
            nrm[i as usize] += n;
        }
    }
    MeshData {
        positions: pos.iter().map(|p| p.to_array()).collect(),
        normals: nrm
            .iter()
            .map(|n| n.normalize_or(Vec3::Y).to_array())
            .collect(),
        colors: vec![[color[0], color[1], color[2], 0.0]; pos.len()],
        indices: idx,
    }
}

/// The procedural mesh of a rock formation kind (`procgen::meshes::build_asset` delegates here).
pub fn formation(kind: AssetKind, variant: u32, lod: u32) -> MeshData {
    if kind == AssetKind::Crystals {
        let (pos, idx) = crystal_cluster(variant);
        return finish(pos, idx, [0.35, 0.32, 0.7]);
    }
    let form = match kind {
        AssetKind::Hoodoo => Form::Hoodoo,
        AssetKind::SeaStack => Form::SeaStack,
        _ => Form::Pinnacle,
    };
    let s = Shape::new(form, variant);
    let mut m = lathe(&s, lod);
    if form == Form::Hoodoo {
        // the hard cap rock, overhanging the column
        let top = s.height;
        let cap = Shape {
            form: Form::SeaStack,
            height: s.radius * 1.1,
            radius: s.radius * s.profile(1.0) * 1.9,
            seed: s.seed ^ 0xca9,
            layer: s.radius * 0.5,
            joints: 0.0,
            lean: Vec2::ZERO,
            ..Shape::new(Form::SeaStack, variant + 17)
        };
        let mut c = lathe(&cap, lod);
        let base = s.axis(top);
        let off = Vec3::new(base.x, top - s.radius * 0.35, base.y);
        let n = m.positions.len() as u32;
        m.positions.extend(
            c.positions
                .drain(..)
                .map(|p| (Vec3::from_array(p) + off).to_array()),
        );
        m.normals.append(&mut c.normals);
        m.colors.append(&mut c.colors);
        m.indices.extend(c.indices.drain(..).map(|i| i + n));
    }
    m
}

/// One crystal: a hexagonal prism with a pyramid tip along `dir` from `base`.
fn crystal(
    pos: &mut Vec<Vec3>,
    idx: &mut Vec<u32>,
    base: Vec3,
    dir: Vec3,
    len: f32,
    r: f32,
    rng: &mut Rng,
) {
    let up = dir.normalize();
    let side = up
        .cross(if up.y.abs() > 0.9 { Vec3::X } else { Vec3::Y })
        .normalize();
    let fwd = up.cross(side);
    let tip_len = r * rng.range(1.2, 2.2);
    let shaft = (len - tip_len).max(r);
    let twist = rng.f32() * std::f32::consts::TAU;
    // (slightly irregular hexagon, each face its own width)
    let ring: Vec<Vec3> = (0..6)
        .map(|k| {
            let a = twist + k as f32 / 6.0 * std::f32::consts::TAU + rng.range(-0.08, 0.08);
            (side * a.cos() + fwd * a.sin()) * r * rng.range(0.88, 1.08)
        })
        .collect();
    let b0 = pos.len() as u32;
    for o in &ring {
        pos.push(base - up * r * 0.6 + *o * 0.92);
    }
    for o in &ring {
        pos.push(base + up * shaft + *o);
    }
    let tip = base + up * (shaft + tip_len) + side * rng.range(-0.15, 0.15) * r;
    pos.push(tip);
    for k in 0..6u32 {
        let (a, b) = (b0 + k, b0 + (k + 1) % 6);
        let (c, d) = (a + 6, b + 6);
        idx.extend_from_slice(&[a, c, b, b, c, d]);
        idx.extend_from_slice(&[c, b0 + 12, d]);
    }
}

/// A cluster: a few big crystals and many small ones radiating up from a rocky knot.
fn crystal_cluster(variant: u32) -> (Vec<Vec3>, Vec<u32>) {
    let mut rng = Rng::new(0xc7a5_u64 + variant as u64 * 104_729);
    let (mut pos, mut idx) = (vec![], vec![]);
    let big = 3 + rng.below(3) as usize;
    let small = 9 + rng.below(8) as usize;
    let scale = rng.range(1.6, 2.6);
    for k in 0..big + small {
        let is_big = k < big;
        let a = rng.f32() * std::f32::consts::TAU;
        let spread = if is_big {
            rng.range(0.05, 0.45)
        } else {
            rng.range(0.35, 1.1)
        };
        let dir = Vec3::new(a.cos() * spread, 1.0, a.sin() * spread);
        let off = Vec3::new(a.cos(), 0.0, a.sin())
            * rng.range(0.0, if is_big { 0.25 } else { 0.7 })
            * scale;
        let len = if is_big {
            rng.range(1.4, 2.6)
        } else {
            rng.range(0.35, 1.0)
        } * scale;
        let r = len
            * if is_big {
                rng.range(0.13, 0.18)
            } else {
                rng.range(0.12, 0.2)
            };
        crystal(&mut pos, &mut idx, off, dir, len, r, &mut rng);
    }
    (pos, idx)
}

/// Unwelded (flat facets read as cut crystal) with uv.y = height fraction for the inner glow.
fn crystal_mesh(variant: u32, lod: u32) -> (Mesh, f32, f32, f32) {
    let (pos, idx) = crystal_cluster(variant);
    let keep = if lod == 0 {
        idx.len()
    } else {
        idx.len().min(idx.len() * 2 / 3 / 3 * 3)
    };
    let (lo, hi) = pos
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
    let (mut p, mut n, mut uv) = (vec![], vec![], vec![]);
    let mut rng = Rng::new(variant as u64 * 31 + lod as u64);
    for t in idx[..keep].chunks_exact(3) {
        let c = [pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]];
        let nr = (c[1] - c[0]).cross(c[2] - c[0]).normalize_or(Vec3::Y);
        // (each facet its own column of the gradients: cut faces catch the light differently)
        let face = rng.f32();
        for q in c {
            p.push(q.to_array());
            n.push(nr.to_array());
            uv.push([
                face,
                ((q.y - lo.y) / (hi.y - lo.y).max(0.1)).clamp(0.0, 1.0),
            ]);
        }
    }
    let count = p.len() as u32;
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, p);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U32((0..count).collect()));
    (m, hi.y - lo.y, (hi.x - lo.x).max(hi.z - lo.z), lo.y)
}

/// A vertical gradient texture (bottom row = uv.y 0), each of its 16 columns a little brighter or
/// darker (uv.x picks a facet's column).
fn gradient(stops: &[(f32, [f32; 3])], srgb: bool) -> Image {
    let (w, h) = (16u32, 64u32);
    let mut data = vec![];
    for row in 0..h {
        let v = 1.0 - row as f32 / (h - 1) as f32;
        let k = stops
            .iter()
            .position(|s| s.0 >= v)
            .unwrap_or(stops.len() - 1)
            .max(1);
        let (a, b) = (stops[k - 1], stops[k]);
        let f = ((v - a.0) / (b.0 - a.0).max(1e-4)).clamp(0.0, 1.0);
        for col in 0..w {
            let gain = 0.6 + 0.8 * crate::noise::value2(col as f32 * 3.7, 0.5, 4421);
            let c: Vec<u8> = (0..3)
                .map(|i| ((a.1[i] + (b.1[i] - a.1[i]) * f) * gain).clamp(0.0, 1.0))
                .map(|x| (x * 255.0).round() as u8)
                .collect();
            data.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Adds the crystal cluster to the library parts: glossy violet-blue crystal, darker and opaque at
/// the root, glowing from within towards the tips.
pub fn build_crystals(
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    parts: &mut std::collections::HashMap<AssetKind, TreeParts>,
) {
    let albedo = gradient(
        &[
            (0.0, [0.04, 0.03, 0.07]),
            (0.3, [0.15, 0.11, 0.34]),
            (1.0, [0.45, 0.48, 0.88]),
        ],
        true,
    );
    let glow = gradient(
        &[
            (0.0, [0.0, 0.0, 0.0]),
            (0.45, [0.05, 0.02, 0.18]),
            (1.0, [0.5, 0.4, 1.0]),
        ],
        true,
    );
    let mat = foliage.add(FoliageMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(albedo)),
            emissive: LinearRgba::new(0.8, 0.8, 0.8, 0.0),
            emissive_texture: Some(images.add(glow)),
            perceptual_roughness: 0.05,
            reflectance: 0.8,
            clearcoat: 1.0,
            clearcoat_perceptual_roughness: 0.04,
            ..default()
        },
        extension: FoliageExt::default(),
    });
    let variants = (0..3)
        .map(|v| {
            let lods: Vec<(Mesh, f32, f32, f32)> = (0..2).map(|lod| crystal_mesh(v, lod)).collect();
            let (height, width, min_y) = (lods[0].1, lods[0].2, lods[0].3);
            TreeVariant {
                lods: lods.into_iter().map(|l| (meshes.add(l.0), None)).collect(),
                cell: None,
                height,
                width,
                min_y,
                trunk: None,
            }
        })
        .collect();
    parts.insert(
        AssetKind::Crystals,
        TreeParts {
            variants,
            bark: mat.clone(),
            leaves: mat,
            ranges: [60.0, 600.0, 600.0, 600.0],
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &MeshData, what: &str) {
        assert!(
            !m.indices.is_empty() && m.indices.len() % 3 == 0,
            "{what}: no triangles"
        );
        assert!(
            m.indices.iter().all(|i| (*i as usize) < m.positions.len()),
            "{what}: index out of range"
        );
        assert!(
            m.normals
                .iter()
                .all(|n| (Vec3::from_array(*n).length() - 1.0).abs() < 1e-3),
            "{what}: bad normal"
        );
        assert!(
            m.positions.iter().all(|p| p.iter().all(|v| v.is_finite())),
            "{what}: non-finite position"
        );
    }

    /// Every formation and crystal variant builds a sound mesh, its far LOD much lighter, standing
    /// on (and sunk into) the ground at its real size.
    #[test]
    fn formations_build() {
        for (kind, lo, hi) in [
            (AssetKind::RockSpire, 9.0, 24.0),
            (AssetKind::Hoodoo, 6.0, 18.0),
            (AssetKind::SeaStack, 18.0, 48.0),
            (AssetKind::Crystals, 1.0, 9.0),
        ] {
            for v in 0..3 {
                let near = formation(kind, v, 0);
                let far = formation(kind, v, 1);
                check(&near, &format!("{kind:?} {v} lod 0"));
                check(&far, &format!("{kind:?} {v} lod 1"));
                let (mn, mx) = near.bounds();
                assert!(mx[1] > lo && mx[1] < hi, "{kind:?} {v}: top at {}", mx[1]);
                if kind != AssetKind::Crystals {
                    assert!(mn[1] < -1.0, "{kind:?} {v}: not sunk into the ground");
                    assert!(
                        far.triangle_count() * 3 < near.triangle_count(),
                        "{kind:?} {v}: far LOD too heavy"
                    );
                }
            }
        }
    }
}
