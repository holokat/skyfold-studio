//! The crop plants, made procedurally, and the 8 m row patches a field is tiled with.
//!
//! Every crop is a few plant variants grown from a small kit — leaf blades with a folded midrib
//! that arch and droop, leaf cards, ribbons (awns, petals, tassel branches), tubes (stalks, vines,
//! posts), round or ribbed fruit, fan leaves, discs — at three levels of detail that keep the same
//! plants in the same places (the far ones drop leaves, florets and segments). Every part is
//! painted from a region of the shared crop atlas (`tools/crops/paint.py`, spec in
//! `tools/crops/ATLAS_SPEC.md`): its colour, its cut-out outline (serrations, lobes, feathery
//! fronds, awns: the geometry is only the envelope) and its normal map; before the atlas exists a
//! part falls back to a plain colour. Ridges, beds and furrows wear the terrain's photoscanned
//! ground. A patch is one 8 × 8 m tile of rows (rows run along x), seamless when tiled. Each vertex
//! keeps its offset from its plant's base, how far it bends in the wind, how often an optional
//! piece shows (a hay bale in some tiles only), its plant's base in the tile (where the shader
//! seats it on the ground) and its surface (`kind`).

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::Crop;
use crate::noise::Rng;
use crate::terrain::layers::Layer;

mod tulips;

/// Patch size (m).
pub const TILE: f32 = 8.0;

/// The atlas's named regions, [u0, v0, u1, v1] (`atlas.json`).
pub type Regions = HashMap<String, [f32; 4]>;

const PI: f32 = std::f32::consts::PI;
const TAU: f32 = std::f32::consts::TAU;

/// sRGB 0–255 → linear.
fn c(r: u8, g: u8, b: u8) -> Vec3 {
    let l = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Vec3::new(l(r), l(g), l(b))
}

/// Surface kinds the crop shader knows (`shaders/crops_v2.wesl`, the tangent's z).
pub mod kind {
    /// vertex colour
    pub const PLAIN: f32 = 0.0;
    /// the terrain's photoscanned ground layer `aux` (ridges, furrows)
    pub const GROUND: f32 = 4.0;
    /// the atlas at (u, v): colour, cut-out, normal map
    pub const ATLAS: f32 = 20.0;
    /// a tiling atlas region repeated every 2 m over the world's x/z (the photoscanned field
    /// soil, the straw mat under grain): the tangent holds its corner u, v, this kind plus a
    /// tenth of its height-to-width (in uv), and its width
    pub const TILED: f32 = 21.0;
}

/// Tulip-only part markers in the tangent's otherwise-unused `aux` slot.  The fractional part
/// carries the unscaled stem height, so the vertex shader can keep the flower head attached to
/// the smoothly bent stem after `Patch::put` applies its per-plant size variation.
pub(crate) mod tulip_aux {
    pub const STEM: f32 = 81.0;
    pub const LEAF: f32 = 82.0;
    pub const PETAL: f32 = 83.0;
    pub const CORE: f32 = 84.0;

    pub fn part(part: f32, stem_height: f32) -> f32 {
        part + stem_height.clamp(0.0, 0.999)
    }

    pub fn is_part(aux: f32) -> bool {
        (STEM..=CORE).contains(&aux.floor())
    }
}

/// The atlas's tiling soil region (`kind::TILED`).
const SOIL: &str = "field_soil";

/// How the parts added next are painted.
#[derive(Clone, Copy)]
struct Paint {
    /// the atlas region (None: no atlas yet, or a plain part)
    region: Option<[f32; 4]>,
    /// the plain colour without the atlas (the atlas colour is multiplied by the tint only)
    base: Vec3,
    rough: f32,
    kind: f32,
    aux: f32,
    /// the region tiles over the world (`kind::TILED`)
    tiled: bool,
}

impl Default for Paint {
    fn default() -> Self {
        Paint {
            region: None,
            base: Vec3::ONE,
            rough: 0.7,
            kind: kind::PLAIN,
            aux: 0.0,
            tiled: false,
        }
    }
}

/// One plant (or piece of row furniture) around its base.
#[derive(Default, Clone)]
pub struct Plant {
    pos: Vec<Vec3>,
    nor: Vec<Vec3>,
    col: Vec<Vec3>,
    rough: Vec<f32>,
    /// 1 flexible … 0 rigid
    flex: Vec<f32>,
    /// the vertex finds its own ground (a ridge, a wire) instead of its plant's
    own_ground: Vec<bool>,
    /// [u, v, kind, aux] for the shader
    tan: Vec<[f32; 4]>,
    idx: Vec<u32>,
    /// height the bend weights are measured against
    pub height: f32,
    paint: PaintState,
}

#[derive(Default, Clone, Copy)]
struct PaintState(Option<Paint>);

impl Plant {
    fn new(height: f32) -> Self {
        Plant {
            height,
            ..default()
        }
    }

    fn cur(&self) -> Paint {
        self.paint.0.unwrap_or_default()
    }

    /// The next parts: atlas region `name` (without the atlas: plain `base` colour), `rough`.
    fn paint(&mut self, reg: &Regions, name: &str, base: Vec3, rough: f32) {
        self.paint = PaintState(Some(Paint {
            region: reg.get(name).copied(),
            base,
            rough,
            ..default()
        }));
    }

    /// The next parts: the photoscanned field soil tiled over the world (the atlas's
    /// `field_soil`), or else the terrain's ground `layer`.
    fn soil(&mut self, reg: &Regions, layer: Layer) {
        match reg.get(SOIL) {
            Some(r) => {
                self.paint = PaintState(Some(Paint {
                    region: Some(*r),
                    rough: 0.92,
                    tiled: true,
                    ..default()
                }))
            }
            None => self.ground(layer),
        }
    }

    /// The next parts: plain colour.
    fn plain(&mut self, base: Vec3, rough: f32) {
        self.paint = PaintState(Some(Paint {
            base,
            rough,
            ..default()
        }));
    }

    /// Associates the next vertices with a shader-specific part marker.  Atlas and plain
    /// surfaces both preserve it in the tangent's otherwise-unused `aux` component.
    fn part(&mut self, aux: f32) {
        if let Some(paint) = &mut self.paint.0 {
            paint.aux = aux;
        }
    }

    /// The next parts: the terrain's photoscanned ground `layer` (the tint shades it).
    fn ground(&mut self, layer: Layer) {
        self.paint = PaintState(Some(Paint {
            rough: 0.9,
            kind: kind::GROUND,
            aux: layer as usize as f32,
            ..default()
        }));
    }

    /// Whether the next parts come from the atlas (their geometry is then the outline's envelope).
    fn textured(&self) -> bool {
        self.cur().region.is_some()
    }

    fn vert(&mut self, p: Vec3, n: Vec3, tint: Vec3, flex: f32, u: f32, v: f32) -> u32 {
        let pt = self.cur();
        let (col, tan) = match pt.region {
            // (its height to width rides in the kind's fraction: kind + aspect / 10)
            Some(r) if pt.tiled => (
                tint,
                [
                    r[0],
                    r[1],
                    kind::TILED + ((r[3] - r[1]) / (r[2] - r[0]).max(1e-6)).clamp(0.05, 4.0) * 0.1,
                    r[2] - r[0],
                ],
            ),
            Some(r) => (
                tint,
                [
                    r[0] + (r[2] - r[0]) * u.clamp(0.0, 1.0),
                    r[1] + (r[3] - r[1]) * v.clamp(0.0, 1.0),
                    kind::ATLAS,
                    pt.aux,
                ],
            ),
            None => (pt.base * tint, [u, v, pt.kind, pt.aux]),
        };
        self.pos.push(p);
        self.nor.push(n.normalize_or(Vec3::Y));
        self.col.push(col);
        self.rough.push(pt.rough);
        self.flex.push(flex);
        self.own_ground.push(false);
        self.tan.push(tan);
        self.pos.len() as u32 - 1
    }

    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.idx.extend_from_slice(&[a, b, c]);
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.idx.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// A ribbon through `pts` (`widths` each), its flat side facing `face(i)`; u across (0…1),
    /// v along its length (0 at the first point).
    fn ribbon(
        &mut self,
        pts: &[Vec3],
        widths: &[f32],
        tints: &[Vec3],
        flex: f32,
        face: impl Fn(usize) -> Vec3,
    ) {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let mut acc = vec![0.0f32];
        for i in 1..n {
            acc.push(acc[i - 1] + pts[i].distance(pts[i - 1]));
        }
        let total = acc[n - 1].max(1e-5);
        let mut prev: Option<(u32, u32)> = None;
        for i in 0..n {
            let t = (pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)]).normalize_or(Vec3::Y);
            let across = t.cross(face(i)).normalize_or(Vec3::X);
            let nrm = across.cross(t).normalize_or(Vec3::Y);
            let w = widths[i.min(widths.len() - 1)] * 0.5;
            let tint = tints[i.min(tints.len() - 1)];
            let v = acc[i] / total;
            let l = self.vert(pts[i] - across * w, nrm, tint, flex, 0.0, v);
            let r = self.vert(pts[i] + across * w, nrm, tint, flex, 1.0, v);
            if let Some((pl, pr)) = prev {
                self.quad(pl, pr, r, l);
            }
            prev = Some((l, r));
        }
    }

    /// A tube through `pts` with `radii`, `sides` round; u around, v from `v0` to `v1` along it.
    #[allow(clippy::too_many_arguments)]
    fn tube(
        &mut self,
        pts: &[Vec3],
        radii: &[f32],
        sides: u32,
        tints: &[Vec3],
        flex: f32,
        v0: f32,
        v1: f32,
    ) {
        let n = pts.len();
        if n < 2 {
            return;
        }
        let mut acc = vec![0.0f32];
        for i in 1..n {
            acc.push(acc[i - 1] + pts[i].distance(pts[i - 1]));
        }
        let total = acc[n - 1].max(1e-5);
        let mut rings: Vec<u32> = vec![];
        for i in 0..n {
            let t = (pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)]).normalize_or(Vec3::Y);
            let a = if t.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
            let u = t.cross(a).normalize();
            let w = t.cross(u);
            let r = radii[i.min(radii.len() - 1)];
            let tint = tints[i.min(tints.len() - 1)];
            let v = v0 + (v1 - v0) * acc[i] / total;
            let first = self.pos.len() as u32;
            for k in 0..=sides {
                let ang = k as f32 / sides as f32 * TAU;
                let d = u * ang.cos() + w * ang.sin();
                self.vert(pts[i] + d * r, d, tint, flex, k as f32 / sides as f32, v);
            }
            rings.push(first);
        }
        for w in rings.windows(2) {
            for k in 0..sides {
                self.quad(w[0] + k, w[0] + k + 1, w[1] + k + 1, w[1] + k);
            }
        }
    }

    /// An ellipsoid (radii `r`, its own axis +y turned by `rot`), optionally ribbed (`ribs` lobes
    /// with grooves `rib_depth` deep, shaded `rib_tint`) and tapered towards its bottom (`taper`:
    /// a cone-ish berry); u around, v from its top (v 0) to its bottom.
    #[allow(clippy::too_many_arguments)]
    fn blob(
        &mut self,
        center: Vec3,
        rot: Quat,
        r: Vec3,
        lon: u32,
        lat: u32,
        tint: Vec3,
        rib_tint: Vec3,
        ribs: u32,
        rib_depth: f32,
        taper: f32,
        flex: f32,
    ) {
        let first = self.pos.len() as u32;
        for j in 0..=lat {
            let th = j as f32 / lat as f32 * PI;
            let v = j as f32 / lat as f32;
            let tp = 1.0 - taper * v.powf(1.4);
            for i in 0..=lon {
                let ph = i as f32 / lon as f32 * TAU;
                // (narrow grooves at u = k / ribs, where the atlas paints them; broad rounded ribs)
                let groove = if ribs > 0 {
                    (0.5 + 0.5 * (ph * ribs as f32).cos()).powf(6.0)
                } else {
                    0.0
                };
                let rib = 1.0 - rib_depth * groove;
                // (ribbed fruit dimple in at the top and bottom)
                let flat = if ribs > 0 {
                    1.0 - 0.2 * th.cos().abs().powf(6.0)
                } else {
                    1.0
                };
                let d = Vec3::new(
                    th.sin() * ph.cos() * rib * tp,
                    th.cos() * flat,
                    th.sin() * ph.sin() * rib * tp,
                );
                let k = groove;
                let nrm = rot * (d / r.max(Vec3::splat(1e-3)));
                self.vert(
                    center + rot * (d * r),
                    nrm,
                    tint.lerp(rib_tint, k),
                    flex,
                    i as f32 / lon as f32,
                    v,
                );
            }
        }
        let w = lon + 1;
        for j in 0..lat {
            for i in 0..lon {
                let a = first + j * w + i;
                self.quad(a, a + 1, a + w + 1, a + w);
            }
        }
    }

    /// A strap leaf blade from `base` heading `dir`, arching under `droop`, folded along its
    /// midrib by `fold`; u across, v base → tip. Textured: an envelope (the texture cuts the
    /// outline); plain: shaped.
    #[allow(clippy::too_many_arguments)]
    fn blade(
        &mut self,
        base: Vec3,
        dir: Vec3,
        len: f32,
        width: f32,
        droop: f32,
        fold: f32,
        segs: usize,
        t0: Vec3,
        t1: Vec3,
        flex: f32,
    ) {
        let shaped = !self.textured();
        let mut d = dir.normalize_or(Vec3::Y);
        let mut p = base;
        let step = len / segs as f32;
        let mut prev: Option<[u32; 3]> = None;
        for s in 0..=segs {
            let t = s as f32 / segs as f32;
            let side = d.cross(Vec3::Y).normalize_or(Vec3::X);
            let up = side.cross(d).normalize_or(Vec3::Y);
            let w =
                if shaped {
                    width
                        * 0.5
                        * ((t * PI).sin().max(0.0).powf(0.55) * (1.0 - 0.75 * t * t))
                            .max(if s == 0 { 0.25 } else { 0.0 })
                } else {
                    width * 0.5 * (1.0 - 0.25 * t)
                };
            let tint = t0.lerp(t1, t);
            let lift = up * fold * w;
            let l = self.vert(p - side * w - lift, up - side * fold, tint, flex, 0.0, t);
            let m = self.vert(p, up, tint, flex, 0.5, t);
            let r = self.vert(p + side * w - lift, up + side * fold, tint, flex, 1.0, t);
            if let Some([pl, pm, pr]) = prev {
                self.quad(pl, pm, m, l);
                self.quad(pm, pr, r, m);
            }
            prev = Some([l, m, r]);
            p += d * step;
            d = (d - Vec3::Y * droop * step).normalize();
        }
    }

    /// A broad leaf card from `base` towards `dir` (`half(t)` its half-width), cupped by `cup`;
    /// u across, v base → tip.
    #[allow(clippy::too_many_arguments)]
    fn card(
        &mut self,
        base: Vec3,
        dir: Vec3,
        len: f32,
        half: impl Fn(f32) -> f32,
        droop: f32,
        cup: f32,
        segs: usize,
        t0: Vec3,
        t1: Vec3,
        flex: f32,
    ) {
        let mut d = dir.normalize_or(Vec3::X);
        let mut p = base;
        let step = len / segs as f32;
        let mut prev: Option<[u32; 3]> = None;
        for s in 0..=segs {
            let t = s as f32 / segs as f32;
            let side = d.cross(Vec3::Y).normalize_or(Vec3::Z);
            let up = side.cross(d).normalize_or(Vec3::Y);
            let w = half(t);
            let tint = t0.lerp(t1, t);
            let lift = up * cup * w;
            let l = self.vert(p - side * w + lift, up - side * cup, tint, flex, 0.0, t);
            let m = self.vert(p, up, tint, flex, 0.5, t);
            let r = self.vert(p + side * w + lift, up + side * cup, tint, flex, 1.0, t);
            if let Some([pl, pm, pr]) = prev {
                self.quad(pl, pm, m, l);
                self.quad(pm, pr, r, m);
            }
            prev = Some([l, m, r]);
            p += d * step;
            d = (d - Vec3::Y * droop * step).normalize();
        }
    }

    /// A fan leaf (palmate) or flower, its stalk joining at `base`, its tip towards `fwd`, tilted
    /// down by `tilt` (0 flat … 1 hanging), cupped by `cup`: rings round the joint, mapped
    /// top-down into the region (tip up, joint at the centre).
    #[allow(clippy::too_many_arguments)]
    fn fan(
        &mut self,
        base: Vec3,
        fwd: Vec3,
        size: f32,
        tilt: f32,
        cup: f32,
        rim: u32,
        rings: u32,
        tint: Vec3,
        edge: Vec3,
        flex: f32,
    ) {
        let f = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or(Vec3::X);
        let s = f.cross(Vec3::Y);
        let q = Quat::from_axis_angle(s, -tilt.clamp(-1.0, 1.0) * 1.2);
        let (fd, up) = (q * f, q * Vec3::Y);
        let mid = self.vert(base, up, tint, flex, 0.5, 0.5);
        let mut prev_ring: Option<u32> = None;
        for j in 1..=rings {
            let r = j as f32 / rings as f32;
            let first = self.pos.len() as u32;
            for k in 0..=rim {
                let a = k as f32 / rim as f32 * TAU;
                let planar = (fd * a.cos() + s * a.sin()) * r * size;
                let p = base + planar + up * cup * r * r * size;
                let n = (up - planar.normalize_or(fd) * cup * 1.2 * r).normalize();
                self.vert(
                    p,
                    n,
                    edge.lerp(tint, 1.0 - 0.5 * r),
                    flex,
                    0.5 + 0.5 * r * a.sin(),
                    0.5 - 0.5 * r * a.cos(),
                );
            }
            match prev_ring {
                None => {
                    for k in 0..rim {
                        self.tri(mid, first + k, first + k + 1);
                    }
                }
                Some(pr) => {
                    for k in 0..rim {
                        self.quad(pr + k, pr + k + 1, first + k + 1, first + k);
                    }
                }
            }
            prev_ring = Some(first);
        }
    }

    /// A flat or slightly domed disc (`rot` turns its +y normal), mapped into the region's circle.
    #[allow(clippy::too_many_arguments)]
    fn disc(
        &mut self,
        center: Vec3,
        rot: Quat,
        radius: f32,
        rings: u32,
        segs: u32,
        dome: f32,
        tint: Vec3,
        flex: f32,
    ) {
        let mid = self.vert(
            center + rot * (Vec3::Y * dome),
            rot * Vec3::Y,
            tint,
            flex,
            0.5,
            0.5,
        );
        let mut prev: Option<u32> = None;
        for j in 1..=rings {
            let r = j as f32 / rings as f32;
            let first = self.pos.len() as u32;
            for k in 0..=segs {
                let a = k as f32 / segs as f32 * TAU;
                let l = Vec3::new(
                    a.cos() * r * radius,
                    dome * (1.0 - r * r),
                    a.sin() * r * radius,
                );
                let n = rot
                    * Vec3::new(
                        a.cos() * r * dome * 2.0 / radius.max(1e-3),
                        1.0,
                        a.sin() * r * dome * 2.0 / radius.max(1e-3),
                    );
                self.vert(
                    center + rot * l,
                    n,
                    tint,
                    flex,
                    0.5 + 0.5 * r * a.cos(),
                    0.5 + 0.5 * r * a.sin(),
                );
            }
            match prev {
                None => {
                    for k in 0..segs {
                        self.tri(mid, first + k + 1, first + k);
                    }
                }
                Some(pr) => {
                    for k in 0..segs {
                        self.quad(pr + k, first + k, first + k + 1, pr + k + 1);
                    }
                }
            }
            prev = Some(first);
        }
    }

    /// Closes a tube's open end at `at` (heading `dir`, radius `r`) with a low cone.
    fn cap(&mut self, at: Vec3, dir: Vec3, r: f32, sides: u32, tint: Vec3, flex: f32) {
        let d = dir.normalize_or(Vec3::Y);
        let a = if d.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = d.cross(a).normalize();
        let w = d.cross(u);
        let tip = self.vert(at + d * r * 0.25, d, tint, flex, 0.5, 0.5);
        let first = self.pos.len() as u32;
        for k in 0..=sides {
            let ang = k as f32 / sides as f32 * TAU;
            let o = u * ang.cos() + w * ang.sin();
            self.vert(
                at + o * r,
                (d + o * 0.6).normalize(),
                tint,
                flex,
                0.5 + 0.5 * ang.cos(),
                0.5 + 0.5 * ang.sin(),
            );
        }
        for k in 0..sides {
            self.tri(tip, first + k, first + k + 1);
        }
    }

    /// Raises the whole plant by `dy` (it stands on a ridge or bed that high).
    fn lift(mut self, dy: f32) -> Self {
        self.pos.iter_mut().for_each(|p| p.y += dy);
        self.height += dy;
        self
    }

    /// Everything added from `from` on finds its own ground (ridges, mulch, wires).
    fn hug_ground_from(&mut self, from: usize) {
        for g in &mut self.own_ground[from..] {
            *g = true;
        }
    }
}

/// A curve from `a` to `b` bowing by `bow` in the middle.
fn curve(a: Vec3, b: Vec3, bow: Vec3, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            a.lerp(b, t) + bow * (4.0 * t * (1.0 - t))
        })
        .collect()
}

/// A direction at compass angle `yaw` rising by `rise` (−1 down … 1 up).
fn dir_at(yaw: f32, rise: f32) -> Vec3 {
    Vec3::new(
        yaw.cos() * (1.0 - rise.abs()).max(0.0).sqrt(),
        rise,
        yaw.sin() * (1.0 - rise.abs()).max(0.0).sqrt(),
    )
    .normalize()
}

const ONE: Vec3 = Vec3::ONE;

// ---------------------------------------------------------------- the crops

/// A cereal: a clump of stalks with ears (wheat, barley, rye) or panicles (rice).
#[derive(Clone, Copy)]
struct Cereal {
    height: (f32, f32),
    stalks: u32,
    stalk: Vec3,
    leaf: Vec3,
    ear: Vec3,
    ear_len: f32,
    /// the ear's envelope width (its texture cuts the spikelets and awns)
    ear_w: f32,
    nod: f32,
    leafy: bool,
    leaf_region: &'static str,
    ear_region: &'static str,
}

fn cereal(rng: &mut Rng, k: &Cereal, lod: u8, reg: &Regions) -> Plant {
    let h = rng.range(k.height.0, k.height.1);
    let mut p = Plant::new(h);
    // (all the stalks at mid detail too: thin as they are, a grain field only looks full when
    // they are all there)
    let stalks = match lod {
        0 | 1 => k.stalks,
        _ => 1,
    };
    // leaves low on the clump
    p.paint(reg, k.leaf_region, k.leaf, 0.6);
    let leaves = if k.leafy {
        [4, 3, 2][lod as usize]
    } else {
        [2, 1, 0][lod as usize]
    };
    for _ in 0..leaves {
        let yaw = rng.f32() * TAU;
        let len = if k.leafy {
            h * rng.range(0.55, 0.85)
        } else {
            h * rng.range(0.3, 0.45)
        };
        let base = Vec3::new(
            0.0,
            if k.leafy {
                0.02
            } else {
                h * rng.range(0.12, 0.35)
            },
            0.0,
        );
        let segs = if lod == 0 { 4 } else { 2 };
        p.blade(
            base,
            dir_at(yaw, rng.range(0.5, 0.8)),
            len,
            if k.leafy { 0.016 } else { 0.018 },
            rng.range(0.8, 1.6) / len.max(0.2),
            0.3,
            segs,
            ONE * rng.range(0.85, 1.0),
            ONE,
            1.0,
        );
    }
    for s in 0..stalks {
        let yaw = rng.f32() * TAU;
        let lean = rng.range(0.02, 0.12) + 0.04 * s as f32 / stalks.max(1) as f32;
        let sh = h * rng.range(0.88, 1.05);
        let off = Vec3::new(yaw.cos(), 0.0, yaw.sin()) * rng.range(0.0, 0.045);
        let top = off + Vec3::new(yaw.cos() * lean * sh, sh, yaw.sin() * lean * sh);
        let segs = if lod == 0 { 2 } else { 1 };
        let pts = curve(
            off,
            top,
            Vec3::new(yaw.cos(), 0.0, yaw.sin()) * lean * 0.15 * sh,
            segs,
        );
        let face = Vec3::new(-yaw.sin(), 0.0, yaw.cos());
        p.paint(reg, "cereal_stalk", k.stalk, 0.45);
        p.ribbon(&pts, &[0.007, 0.0055], &[ONE * 0.9, ONE], 1.0, |_| face);
        // the ear: two crossed ribbons, nodding over when heavy
        let tip_dir = (top - pts[pts.len() - 2]).normalize_or(Vec3::Y);
        let nod_dir = Vec3::new(yaw.cos(), 0.0, yaw.sin());
        let ear_segs = if lod == 0 { 3 } else { 1 };
        let mut ear = vec![top - tip_dir * 0.01];
        let mut d = tip_dir;
        let el = k.ear_len * rng.range(0.85, 1.15);
        for _ in 0..ear_segs {
            d = (d + nod_dir * k.nod * 0.8 - Vec3::Y * k.nod * 0.9).normalize();
            let last = *ear.last().unwrap();
            ear.push(last + d * el / ear_segs as f32);
        }
        p.paint(reg, k.ear_region, k.ear, 0.5);
        let tex = p.textured();
        let widths: Vec<f32> = (0..=ear_segs)
            .map(|i| {
                if tex {
                    k.ear_w
                } else {
                    k.ear_w * [0.6, 0.8, 0.7, 0.3][(i * 3 / ear_segs.max(1)).min(3)]
                }
            })
            .collect();
        let tints: Vec<Vec3> = (0..=ear_segs).map(|_| ONE * rng.range(0.9, 1.05)).collect();
        p.ribbon(&ear, &widths, &tints, 1.0, |_| face);
        if lod < 2 {
            p.ribbon(&ear, &widths, &tints, 1.0, |_| nod_dir);
        }
    }
    p
}

/// A maize plant: a jointed stalk on brace roots, long arching leaves with wavy margins in two
/// ranks, an ear in its husk with silk, and a branched tassel on top.
fn corn(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let h = rng.range(2.1, 2.6);
    let mut p = Plant::new(h);
    let top_y = h * 0.84;
    let lean = Vec3::new(rng.range(-0.04, 0.04), 0.0, rng.range(-0.04, 0.04));
    let segs = [7, 4, 2][lod as usize];
    let pts = curve(
        Vec3::ZERO,
        Vec3::new(lean.x, top_y, lean.z),
        Vec3::new(0.015, 0.0, -0.01),
        segs,
    );
    let radii: Vec<f32> = (0..=segs)
        .map(|i| {
            if i == segs {
                0.004
            } else {
                0.0135 * (1.0 - 0.45 * i as f32 / segs as f32)
            }
        })
        .collect();
    p.paint(reg, "corn_stalk", c(126, 146, 62), 0.5);
    p.tube(
        &pts,
        &radii,
        [6, 4, 3][lod as usize],
        &[ONE],
        0.85,
        0.0,
        1.0,
    );
    let at = |t: f32| -> Vec3 {
        let f = t * segs as f32;
        let i = (f.floor() as usize).min(segs - 1);
        pts[i].lerp(pts[i + 1], f - i as f32)
    };
    // brace roots
    if lod == 0 {
        for k in 0..5 {
            let a = k as f32 / 5.0 * TAU + rng.f32() * 0.5;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            p.tube(
                &curve(
                    Vec3::Y * rng.range(0.06, 0.12) + d * 0.012,
                    d * rng.range(0.09, 0.14) - Vec3::Y * 0.03,
                    d * 0.02 + Vec3::Y * 0.02,
                    2,
                ),
                &[0.006, 0.004],
                4,
                &[ONE * 0.85],
                0.0,
                0.0,
                0.08,
            );
        }
    }
    // leaves from the nodes, in two ranks across the row
    let leaves = [12, 8, 5][lod as usize];
    let rank = rng.range(-0.3, 0.3) + PI * 0.5;
    p.paint(reg, "corn_leaf", c(56, 98, 32), 0.5);
    for i in 0..leaves {
        let t = 0.1 + 0.74 * i as f32 / leaves as f32;
        let yaw = rank + if i % 2 == 0 { 0.0 } else { PI } + rng.range(-0.35, 0.35);
        let d0 = dir_at(yaw, rng.range(0.55, 0.8));
        let base = at(t) + Vec3::new(d0.x, 0.0, d0.z).normalize() * 0.015;
        // longest in the middle of the plant
        let len = h * (0.3 + 0.16 * (1.0 - (t - 0.45).abs() * 2.2).max(0.0)) * rng.range(0.9, 1.1);
        let width = 0.1 * rng.range(0.85, 1.1) * (1.0 - 0.3 * (t - 0.45).abs());
        p.blade(
            base,
            d0,
            len,
            width,
            rng.range(1.9, 2.9),
            0.28,
            [7, 4, 2][lod as usize],
            ONE * rng.range(0.88, 1.0),
            ONE * rng.range(0.95, 1.08),
            1.0,
        );
    }
    // the ear on a short shank, wrapped in husk leaves with flags and silk
    if lod < 2 {
        let t = rng.range(0.4, 0.5);
        let yaw = rank + PI * 0.5 + rng.range(-0.5, 0.5);
        let d = dir_at(yaw, 0.72);
        let base = at(t);
        let centre = base + d * 0.13;
        let rot = Quat::from_rotation_arc(Vec3::Y, d);
        p.paint(reg, "green_stem", c(120, 140, 60), 0.6);
        p.tube(
            &[base, base + d * 0.04],
            &[0.009, 0.011],
            4,
            &[ONE],
            0.6,
            0.0,
            0.2,
        );
        p.paint(reg, "corn_husk", c(150, 168, 92), 0.55);
        p.blob(
            centre,
            rot,
            Vec3::new(0.026, 0.1, 0.026),
            [8, 5, 4][lod as usize],
            [6, 4, 3][lod as usize],
            ONE,
            ONE * 0.9,
            0,
            0.0,
            0.25,
            0.55,
        );
        let tip = centre + d * 0.1;
        if lod == 0 {
            for f in 0..3 {
                let a = f as f32 / 3.0 * TAU + rng.f32();
                let side = (rot * Vec3::new(a.cos(), 0.0, a.sin())).normalize();
                p.blade(
                    tip - d * 0.02,
                    (d + side * 0.6).normalize(),
                    rng.range(0.06, 0.1),
                    0.03,
                    4.0,
                    0.1,
                    2,
                    ONE * 0.95,
                    ONE * 1.05,
                    0.6,
                );
            }
            p.paint(reg, "corn_silk", c(150, 96, 56), 0.7);
            let face = d.cross(Vec3::Y).normalize_or(Vec3::X);
            p.ribbon(
                &curve(tip, tip + d * 0.05 - Vec3::Y * 0.04, Vec3::Y * 0.01, 2),
                &[0.03, 0.04],
                &[ONE],
                0.6,
                |_| face,
            );
            p.ribbon(
                &curve(tip, tip + d * 0.05 - Vec3::Y * 0.04, Vec3::Y * 0.01, 2),
                &[0.03, 0.04],
                &[ONE],
                0.6,
                |_| Vec3::Y.cross(face),
            );
        }
    }
    // the tassel: a central spike and drooping side branches of spikelets
    let top = pts[segs];
    // (thin and pale: from afar a maize field is deep green with a faint beige haze on top)
    p.paint(reg, "corn_tassel", c(176, 158, 104), 0.6);
    let spike = [top, top + Vec3::Y * 0.14, top + Vec3::new(0.01, 0.3, 0.0)];
    p.ribbon(&spike, &[0.024], &[ONE], 1.0, |_| Vec3::X);
    if lod < 2 {
        p.ribbon(&spike, &[0.024], &[ONE], 1.0, |_| Vec3::Z);
    }
    let n = [8, 4, 0][lod as usize];
    for i in 0..n {
        let yaw = i as f32 / n as f32 * TAU + rng.f32() * 0.6;
        let d = dir_at(yaw, rng.range(0.25, 0.5));
        let s0 = top + Vec3::Y * rng.range(0.02, 0.1);
        let len = rng.range(0.16, 0.26);
        let pts = curve(
            s0,
            s0 + d * len - Vec3::Y * len * 0.45,
            Vec3::Y * len * 0.25,
            if lod == 0 { 3 } else { 1 },
        );
        p.ribbon(
            &pts,
            &[0.022, 0.018],
            &[ONE * rng.range(0.9, 1.05)],
            1.0,
            |_| Vec3::Y.cross(d).normalize_or(Vec3::X),
        );
    }
    p
}

/// A sunflower: a hairy stem curving into a nodding head (a seed disc in golden-angle spirals,
/// two rings of curling petals, green bracts behind), big heart-shaped leaves on stalks.
fn sunflower(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let h = rng.range(1.6, 2.05);
    let mut p = Plant::new(h);
    // the head looks out and down, roughly the same way across the field
    let face = Vec3::new(1.0, -rng.range(0.25, 0.75), rng.range(-0.3, 0.3)).normalize();
    let fh = Vec3::new(face.x, 0.0, face.z).normalize();
    let segs = [6, 3, 2][lod as usize];
    let top = Vec3::new(-fh.x * 0.04, h * 0.86, -fh.z * 0.04);
    let mut pts = curve(
        Vec3::ZERO,
        top,
        Vec3::new(rng.range(-0.03, 0.03), 0.0, rng.range(-0.03, 0.03)),
        segs,
    );
    // the neck arcs over smoothly from upright into the way the head faces
    let neck = top + Vec3::Y * h * 0.11 + fh * 0.16;
    let ctrl = neck - face * 0.2;
    for i in 1..=[6, 2, 1][lod as usize] {
        let t = i as f32 / [6, 2, 1][lod as usize] as f32;
        pts.push(top * (1.0 - t) * (1.0 - t) + ctrl * 2.0 * t * (1.0 - t) + neck * t * t);
    }
    let radii: Vec<f32> = (0..pts.len())
        .map(|i| 0.022 * (1.0 - 0.4 * i as f32 / pts.len() as f32))
        .collect();
    p.paint(reg, "sunflower_stem", c(84, 120, 40), 0.65);
    p.tube(&pts, &radii, [6, 4, 3][lod as usize], &[ONE], 0.7, 0.0, 1.0);
    // leaves on stalks, spiralling up, smaller towards the top
    let leaves = [9, 6, 3][lod as usize];
    for i in 0..leaves {
        let t = 0.12 + 0.72 * i as f32 / leaves as f32;
        let yaw = i as f32 * 2.4 + rng.f32() * 0.5;
        let k = ((t * segs as f32) as usize).min(segs - 1);
        let base = pts[k].lerp(pts[k + 1], t * segs as f32 - k as f32);
        let d = dir_at(yaw, 0.35);
        let stalk_end = base + d * rng.range(0.08, 0.14);
        p.paint(reg, "sunflower_stem", c(84, 120, 40), 0.65);
        p.tube(
            &[base, stalk_end],
            &[0.006, 0.004],
            3,
            &[ONE],
            0.8,
            0.0,
            0.15,
        );
        let size = 0.32 * (1.0 - 0.5 * t) * rng.range(0.85, 1.15);
        p.paint(reg, "sunflower_leaf", c(52, 92, 28), 0.7);
        let tex = p.textured();
        p.card(
            stalk_end,
            dir_at(yaw, -0.05),
            size,
            move |t| {
                if tex {
                    size * 0.46
                } else {
                    size * 0.45 * (t * PI).sin().max(0.0).powf(0.6) * (1.0 - 0.5 * t)
                }
            },
            1.3,
            0.18,
            [3, 3, 2][lod as usize],
            ONE * 0.92,
            ONE,
            0.85,
        );
    }
    // the head
    let head = neck + face * 0.04;
    let rot = Quat::from_rotation_arc(Vec3::Y, face);
    let r = rng.range(0.12, 0.16);
    // the back: a flat, dark green receptacle shingled with pointed bracts
    p.paint(reg, "sunflower_stem", c(84, 120, 40), 0.65);
    p.blob(
        head - face * 0.014,
        rot,
        Vec3::new(r * 0.94, 0.013, r * 0.94),
        [12, 8, 6][lod as usize],
        [4, 3, 2][lod as usize],
        ONE * 0.6,
        ONE * 0.6,
        0,
        0.0,
        0.0,
        0.6,
    );
    p.paint(reg, "sunflower_bract", c(70, 110, 36), 0.6);
    // (shingled over the whole back like scales: the outer ring flares past the rim, the
    // inner ones lie flat on it)
    for (ring, n) in [
        (0, [16, 10, 0][lod as usize]),
        (1, [14, 0, 0][lod as usize]),
        (2, [9, 0, 0][lod as usize]),
    ] {
        for k in 0..n {
            let a = (k as f32 + 0.5 * ring as f32) / n as f32 * TAU + rng.f32() * 0.15;
            let radial = rot * Vec3::new(a.cos(), 0.0, a.sin());
            let (at, lean, len) = [
                (0.8, 0.55, rng.range(0.07, 0.1)),
                (0.52, 0.2, rng.range(0.06, 0.08)),
                (0.22, 0.16, rng.range(0.05, 0.07)),
            ][ring];
            let d = (radial - face * lean).normalize();
            let base = head - face * (0.018 + 0.006 * ring as f32) + radial * r * at;
            p.blade(
                base,
                d,
                len,
                0.044,
                1.2,
                0.1,
                if ring == 0 { 2 } else { 1 },
                ONE * (0.8 - 0.06 * ring as f32),
                ONE * 0.95,
                0.6,
            );
        }
    }
    // the disc
    p.paint(reg, "sunflower_disc", c(58, 36, 16), 0.65);
    p.disc(
        head,
        rot,
        r,
        [3, 2, 1][lod as usize],
        [18, 12, 8][lod as usize],
        0.008,
        ONE,
        0.6,
    );
    // petals: two overlapping rings, curling and twisting a little
    p.paint(reg, "sunflower_petal", c(250, 186, 24), 0.45);
    let tex = p.textured();
    let petals = [22, 16, 10][lod as usize];
    for k in 0..petals {
        let ring = k % 2;
        let a = k as f32 / petals as f32 * TAU + rng.f32() * 0.08;
        let radial = rot * Vec3::new(a.cos(), 0.0, a.sin());
        let side = face.cross(radial).normalize_or(Vec3::X);
        let base = head + radial * r * 0.93 + face * (0.004 * ring as f32);
        let len = r * rng.range(0.55, 0.78);
        let curl = rng.range(-0.25, 0.35);
        let segs = if lod == 0 { 3 } else { 1 };
        let pts: Vec<Vec3> = (0..=segs)
            .map(|i| {
                let t = i as f32 / segs as f32;
                base + radial * len * t + face * (curl * t * t * len + 0.012 * t)
                    - side * 0.01 * t * t
            })
            .collect();
        let w = if tex { 0.028 } else { 0.03 };
        let widths: Vec<f32> = (0..=segs)
            .map(|i| {
                if tex {
                    w
                } else {
                    w * (1.0 - (i as f32 / segs as f32 - 0.45).abs())
                }
            })
            .collect();
        let twist = rng.range(-0.25, 0.25);
        p.ribbon(&pts, &widths, &[ONE * rng.range(0.92, 1.05)], 0.6, |i| {
            (face + side * twist * i as f32 / segs as f32).normalize()
        });
    }
    p
}

fn lavender(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let h = rng.range(0.62, 0.78);
    let mut p = Plant::new(h);
    // a dense grey-green tuft of narrow leafy shoots, fanning out from the woody base
    p.paint(reg, "lavender_leaf", c(112, 130, 98), 0.8);
    let shoots = [140, 50, 18][lod as usize];
    for i in 0..shoots {
        let yaw = i as f32 * 2.39996 + rng.f32() * 0.5;
        let out = (i as f32 / shoots as f32).sqrt();
        let base =
            Vec3::new(yaw.cos(), 0.0, yaw.sin()) * 0.14 * out + Vec3::Y * rng.range(0.0, 0.08);
        let len = rng.range(0.16, 0.3) * (1.0 - 0.25 * out);
        p.blade(
            base,
            dir_at(yaw, 0.95 - 0.45 * out),
            len,
            0.016,
            1.2,
            0.1,
            1,
            ONE * rng.range(0.72, 0.9),
            ONE * rng.range(0.95, 1.1),
            0.6,
        );
    }
    // the flower stems rise out of it in a loose crown, each tipped with a spike of tiny
    // flowers: a purple haze over the top
    let stems = [70, 28, 10][lod as usize];
    let base = Vec3::Y * 0.06;
    for i in 0..stems {
        let a = i as f32 * 2.39996 + rng.f32() * 0.3;
        let tilt = (i as f32 / stems as f32).sqrt() * 0.62 + rng.range(-0.05, 0.05);
        let d = Vec3::new(a.cos() * tilt.sin(), tilt.cos(), a.sin() * tilt.sin()).normalize();
        let len = h * rng.range(0.85, 1.0);
        let top = base + d * len;
        let face = Vec3::new(-a.sin(), 0.0, a.cos());
        p.paint(reg, "green_stem", c(120, 136, 86), 0.7);
        p.ribbon(
            &[base + d * 0.1, top - d * 0.12],
            &[0.004, 0.003],
            &[ONE * 0.85],
            1.0,
            |_| face,
        );
        p.paint(reg, "lavender_spike", c(126, 92, 190), 0.65);
        let spike = [top - d * 0.14, top + d * 0.02];
        let tint = ONE * rng.range(0.88, 1.08);
        p.ribbon(&spike, &[0.036], &[tint], 1.0, |_| face);
        if lod == 0 {
            p.ribbon(&spike, &[0.036], &[tint], 1.0, |_| {
                d.cross(face).normalize_or(Vec3::X)
            });
        }
    }
    p
}

const TULIP_COLOURS: [(u8, u8, u8); 6] = [
    (214, 28, 40),
    (250, 200, 36),
    (240, 118, 168),
    (248, 246, 236),
    (124, 44, 150),
    (250, 118, 30),
];

fn tulip(rng: &mut Rng, lod: u8, colour: usize, reg: &Regions) -> Plant {
    tulips::build(rng, lod, colour, reg)
}

/// Mown hay meadow: a tuft of cut stems, pale straw and fresh green regrowth.
fn stubble(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.15);
    let n = [14, 6, 2][lod as usize];
    for k in 0..n {
        let at = Vec3::new(rng.range(-0.2, 0.2), 0.0, rng.range(-0.2, 0.2));
        let yaw = rng.f32() * TAU;
        let green = k % 3 != 0;
        if green {
            p.paint(reg, "rice_leaf", c(110, 140, 64), 0.7);
        } else {
            p.paint(reg, "cereal_stalk", c(206, 176, 104), 0.6);
        }
        let tint = if green && p.textured() {
            Vec3::new(0.85, 0.92, 0.7) * rng.range(0.8, 1.05)
        } else {
            ONE * rng.range(0.75, 1.0)
        };
        let lean = rng.range(0.0, 0.05);
        let top = at
            + Vec3::new(
                yaw.cos() * lean,
                rng.range(0.05, if green { 0.16 } else { 0.1 }),
                yaw.sin() * lean,
            );
        p.ribbon(&[at, top], &[0.007, 0.004], &[tint], 0.5, |_| {
            Vec3::new(-yaw.sin(), 0.0, yaw.cos())
        });
    }
    p
}

/// Loose stems lying on a windrow (`width` wide, `height` high, along x across the tile).
fn loose_straw(rng: &mut Rng, lod: u8, width: f32, height: f32, reg: &Regions) -> Plant {
    let mut p = Plant::new(1.0);
    p.paint(reg, "cereal_stalk", c(214, 186, 116), 0.6);
    let first = p.pos.len();
    let n = [240, 80, 0][lod as usize];
    for _ in 0..n {
        let x = rng.range(-TILE * 0.5 + 0.3, TILE * 0.5 - 0.3);
        // (many straggling over the edges)
        let t = if rng.f32() < 0.45 {
            rng.range(0.75, 1.15) * if rng.f32() < 0.5 { -1.0 } else { 1.0 }
        } else {
            rng.range(-0.8, 0.8)
        };
        let yaw = rng.range(-0.7, 0.7) + if rng.f32() < 0.35 { PI * 0.5 } else { 0.0 };
        let len = rng.range(0.18, 0.5);
        let d = Vec3::new(yaw.cos(), 0.0, yaw.sin());
        let y = |tt: f32| height * (1.0 - tt * tt).max(0.0).powf(0.8) + 0.012;
        let a = Vec3::new(x, 0.0, t * width * 0.5) - d * len * 0.5;
        let b = a + d * len;
        let (ta, tb) = (
            (a.z / (width * 0.5)).clamp(-1.0, 1.0),
            (b.z / (width * 0.5)).clamp(-1.0, 1.0),
        );
        let pts = [
            Vec3::new(a.x, y(ta), a.z),
            Vec3::new(
                (a.x + b.x) * 0.5,
                y((ta + tb) * 0.5) + 0.01,
                (a.z + b.z) * 0.5,
            ),
            Vec3::new(b.x, y(tb), b.z),
        ];
        p.ribbon(&pts, &[0.007], &[ONE * rng.range(0.75, 1.1)], 0.0, |_| {
            Vec3::Y
        });
    }
    p.hug_ground_from(first);
    p
}

/// A round bale lying on its side (along z): straw round the drum, rolled spiral ends, net wrap.
fn bale(lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(1.5);
    let (r, w) = (0.72, 0.6);
    let sides = [24, 14, 8][lod as usize];
    let centre = |z: f32| Vec3::new(0.0, r * 0.97, z);
    // the drum: slightly barrel-shaped, a little flattened where it sits
    p.paint(reg, "straw", c(204, 172, 102), 0.85);
    let rings = [5, 3, 2][lod as usize];
    let first = p.pos.len() as u32;
    for j in 0..=rings {
        let t = j as f32 / rings as f32;
        let z = -w + 2.0 * w * t;
        let bulge = 1.0 + 0.03 * (1.0 - (2.0 * t - 1.0).powi(2));
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * TAU;
            let mut d = Vec3::new(a.cos(), a.sin(), 0.0);
            if d.y < -0.85 {
                d.y = -0.85 - (d.y + 0.85) * 0.2;
            }
            p.vert(
                centre(z) + d * r * bulge,
                d,
                ONE,
                0.0,
                k as f32 / sides as f32,
                t,
            );
        }
    }
    let row = sides as u32 + 1;
    for j in 0..rings as u32 {
        for k in 0..sides as u32 {
            let a = first + j * row + k;
            p.quad(a, a + row, a + row + 1, a + 1);
        }
    }
    // the ends: the rolled spiral
    p.paint(reg, "bale_end", c(176, 142, 82), 0.9);
    for s in [-1.0f32, 1.0] {
        p.disc(
            centre(s * w),
            Quat::from_rotation_arc(Vec3::Y, Vec3::Z * s),
            r * 0.99,
            [3, 2, 1][lod as usize],
            sides as u32,
            0.03,
            ONE,
            0.0,
        );
    }
    // net wrap over the drum
    if lod == 0 && reg.contains_key("bale_net") {
        p.paint(reg, "bale_net", c(230, 230, 225), 0.6);
        let first = p.pos.len() as u32;
        for j in 0..=1 {
            let z = if j == 0 { -w * 0.98 } else { w * 0.98 };
            for k in 0..=sides {
                let a = k as f32 / sides as f32 * TAU;
                let d = Vec3::new(a.cos(), a.sin().max(-0.85), 0.0);
                p.vert(
                    centre(z) + d * (r * 1.035),
                    d,
                    ONE,
                    0.0,
                    k as f32 / sides as f32 * 3.0 % 1.0001,
                    j as f32,
                );
            }
        }
        for k in 0..sides as u32 {
            p.quad(
                first + k,
                first + row + k,
                first + row + k + 1,
                first + k + 1,
            );
        }
    }
    p
}

/// What a ridge is made of.
enum Surface {
    /// tilled soil (the photoscanned field soil, or else the terrain's ground layer)
    Soil(Layer),
    /// an atlas region repeated every 2 m along the row (straw), or else a plain colour
    Atlas(&'static str, Vec3),
}

/// Ridges, beds, furrows and windrows along the row, across the whole tile, hugging the ground.
#[allow(clippy::too_many_arguments)]
fn ridge(
    width: f32,
    height: f32,
    lod: u8,
    top: Vec3,
    side: Vec3,
    reg: &Regions,
    surface: Surface,
    lumpy: f32,
) -> Plant {
    let mut p = Plant::new(1.0);
    match surface {
        Surface::Soil(layer) => p.soil(reg, layer),
        Surface::Atlas(name, base) => p.paint(reg, name, base, 0.85),
    }
    let segs_per_2m = [8, 4, 2][lod as usize];
    let across = [7, 5, 3][lod as usize];
    let first = p.pos.len();
    for seg in 0..4 {
        let x0 = -TILE * 0.5 + seg as f32 * 2.0;
        let start = p.pos.len() as u32;
        for i in 0..=segs_per_2m {
            let f = i as f32 / segs_per_2m as f32;
            let x = x0 + 2.0 * f;
            // irregular clods and a lumpy crest
            let wob = (x * 2.3).sin() * 0.5 + (x * 5.7 + 1.3).sin() * 0.3 + (x * 11.1).sin() * 0.2;
            for j in 0..across {
                let t = j as f32 / (across - 1) as f32 * 2.0 - 1.0;
                let y = height
                    * (1.0 - t * t).max(0.0).powf(0.8)
                    * (1.0 + lumpy * wob * (1.0 - t.abs()));
                let n = Vec3::new(0.0, 1.0, -1.6 * t * height / (width * 0.5)).normalize();
                p.vert(
                    Vec3::new(x, y, t * width * 0.5 * (1.0 + (0.04 + 0.3 * lumpy) * wob)),
                    n,
                    side.lerp(top, 1.0 - t.abs()),
                    0.0,
                    (t + 1.0) * 0.5,
                    f,
                );
            }
        }
        for i in 0..segs_per_2m as u32 {
            for j in 0..across as u32 - 1 {
                let a = start + i * across as u32 + j;
                p.quad(a, a + 1, a + across as u32 + 1, a + across as u32);
            }
        }
    }
    p.hug_ground_from(first);
    p
}

fn furrows(lod: u8, reg: &Regions) -> Plant {
    ridge(
        0.5,
        0.22,
        lod,
        Vec3::splat(1.05),
        Vec3::splat(0.75),
        reg,
        Surface::Soil(Layer::Dirt),
        0.25,
    )
}

/// The tilled soil between the plants: a sheet over the whole tile hugging the ground, in the
/// photoscanned field soil (none without it: the terrain's own ground shows).
fn floor(lod: u8, tint: Vec3, straw: bool, reg: &Regions) -> Option<Plant> {
    reg.get(SOIL)?;
    let mut p = Plant::new(1.0);
    p.soil(reg, Layer::Dirt);
    if straw && let Some(r) = reg.get("straw") {
        // (under grain: a mat of fallen straw and stubble)
        p.paint = PaintState(Some(Paint {
            region: Some(*r),
            rough: 0.9,
            tiled: true,
            ..default()
        }));
    }
    let n = [17u32, 9, 5][lod as usize];
    for j in 0..n {
        for i in 0..n {
            let (x, z) = (
                i as f32 / (n - 1) as f32 * TILE - TILE * 0.5,
                j as f32 / (n - 1) as f32 * TILE - TILE * 0.5,
            );
            // (a little above the ground: the terrain's own triangles don't follow it exactly)
            p.vert(Vec3::new(x, 0.05, z), Vec3::Y, tint, 0.0, 0.0, 0.0);
        }
    }
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = j * n + i;
            p.quad(a, a + n, a + n + 1, a + 1);
        }
    }
    p.hug_ground_from(0);
    Some(p)
}

/// A pumpkin patch plant: hairy vines with tendrils, big lobed leaves standing on stalks, a
/// ribbed pumpkin (two now and then) with a twisted woody stem.
fn pumpkin_patch(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.5);
    let runs = [3, 2, 1][lod as usize];
    for v in 0..runs {
        let yaw = v as f32 / runs as f32 * TAU + rng.f32();
        let d = Vec3::new(yaw.cos(), 0.0, yaw.sin());
        let len = rng.range(0.9, 1.4);
        let pts = curve(
            Vec3::new(0.0, 0.025, 0.0),
            d * len + Vec3::Y * 0.025,
            d.cross(Vec3::Y) * rng.range(-0.3, 0.3),
            if lod == 0 { 5 } else { 2 },
        );
        p.paint(reg, "pumpkin_vine", c(92, 120, 50), 0.7);
        p.tube(
            &pts,
            &[0.012, 0.008],
            if lod == 0 { 5 } else { 3 },
            &[ONE],
            0.15,
            0.0,
            1.0,
        );
        // tendrils
        if lod == 0 {
            for _ in 0..2 {
                let at = pts[rng.below(pts.len() as u32 - 1) as usize + 1];
                let curl: Vec<Vec3> = (0..6)
                    .map(|i| {
                        at + Vec3::new(
                            (i as f32 * 1.4).cos() * 0.025,
                            0.02 + i as f32 * 0.012,
                            (i as f32 * 1.4).sin() * 0.025,
                        )
                    })
                    .collect();
                p.paint(reg, "green_stem", c(110, 140, 60), 0.6);
                p.tube(&curl, &[0.002], 3, &[ONE], 0.6, 0.0, 0.3);
            }
        }
        let leaves = [4, 2, 1][lod as usize];
        for l in 0..leaves {
            let at = pts[(l + 1).min(pts.len() - 1)];
            let ly = yaw + rng.range(-1.2, 1.2);
            let size = rng.range(0.2, 0.28);
            let up = Vec3::Y * rng.range(0.22, 0.42) + Vec3::new(ly.cos(), 0.0, ly.sin()) * 0.05;
            p.paint(reg, "pumpkin_vine", c(92, 120, 50), 0.7);
            p.tube(
                &curve(at, at + up, Vec3::new(ly.cos(), 0.0, ly.sin()) * 0.04, 2),
                &[0.007, 0.005],
                4,
                &[ONE],
                0.6,
                0.0,
                0.4,
            );
            p.paint(reg, "pumpkin_leaf", c(58, 108, 38), 0.75);
            p.fan(
                at + up,
                Vec3::new(ly.cos(), 0.0, ly.sin()),
                size,
                rng.range(0.15, 0.45),
                0.18,
                [16, 10, 6][lod as usize],
                [2, 1, 1][lod as usize],
                ONE * rng.range(0.9, 1.05),
                ONE * 0.85,
                0.6,
            );
        }
    }
    // the pumpkins: ribbed, sitting on the ground among the leaves
    let n = if rng.f32() < 0.35 { 2 } else { 1 };
    for _ in 0..n {
        let a = rng.f32() * TAU;
        let at = Vec3::new(a.cos(), 0.0, a.sin()) * rng.range(0.25, 0.65);
        let s = rng.range(0.15, 0.24);
        let lean =
            Quat::from_rotation_z(rng.range(-0.15, 0.15)) * Quat::from_rotation_y(rng.f32() * TAU);
        p.paint(reg, "pumpkin_skin", c(232, 112, 20), 0.45);
        p.blob(
            at + Vec3::Y * s * 0.78,
            lean,
            Vec3::new(s, s * 0.8, s),
            [40, 20, 10][lod as usize],
            [10, 6, 4][lod as usize],
            ONE * rng.range(0.9, 1.06),
            ONE * 0.9,
            10,
            0.1,
            0.0,
            0.0,
        );
        let top = at + Vec3::Y * s * 1.4;
        p.paint(reg, "pumpkin_stem", c(112, 100, 58), 0.85);
        let bend = Vec3::new(rng.range(-0.04, 0.04), 0.0, rng.range(-0.04, 0.04));
        p.tube(
            &curve(
                top - Vec3::Y * 0.03,
                top + Vec3::Y * 0.07 + bend,
                bend * 0.5,
                3,
            ),
            &[0.02, 0.016, 0.013],
            [6, 5, 3][lod as usize],
            &[ONE],
            0.0,
            0.0,
            1.0,
        );
        if lod < 2 {
            p.cap(
                top + Vec3::Y * 0.07 + bend,
                (Vec3::Y * 0.1 + bend).normalize(),
                0.013,
                [6, 5, 3][lod as usize],
                ONE * 0.8,
                0.0,
            );
        }
    }
    p
}

fn cabbage(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.35);
    let r = rng.range(0.09, 0.12);
    p.paint(reg, "cabbage_head", c(170, 202, 142), 0.5);
    p.blob(
        Vec3::Y * r * 0.95,
        Quat::IDENTITY,
        Vec3::new(r, r * 0.92, r),
        [10, 7, 5][lod as usize],
        [7, 5, 3][lod as usize],
        ONE * 0.85,
        ONE * 0.8,
        0,
        0.0,
        0.0,
        0.1,
    );
    // big blue-green outer leaves: an inner ring cupped round the head, an outer one spread wide
    p.paint(reg, "cabbage_leaf", c(98, 146, 112), 0.5);
    let tex = p.textured();
    for (ring, n) in [(0, [6, 4, 3][lod as usize]), (1, [7, 5, 3][lod as usize])] {
        for i in 0..n {
            let yaw = (i as f32 + 0.5 * ring as f32) / n as f32 * TAU + rng.f32() * 0.4;
            let size = if ring == 0 {
                rng.range(0.2, 0.26)
            } else {
                rng.range(0.28, 0.36)
            };
            let (rise, droop, cup) = if ring == 0 {
                (0.62, 2.4, 0.55)
            } else {
                (0.3, 1.6, 0.35)
            };
            p.card(
                Vec3::new(0.0, 0.02, 0.0),
                dir_at(yaw, rise),
                size,
                move |t| {
                    if tex {
                        size * 0.5
                    } else {
                        size * 0.55 * (t * PI).sin().max(0.0).powf(0.5)
                    }
                },
                droop,
                cup,
                [4, 3, 2][lod as usize],
                ONE * rng.range(0.78, 0.9),
                ONE * 0.92,
                0.3,
            );
        }
    }
    p
}

fn potato(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.55);
    let stems = [4, 3, 2][lod as usize];
    for s in 0..stems {
        let yaw = s as f32 / stems as f32 * TAU + rng.f32();
        let top = Vec3::new(yaw.cos() * 0.18, rng.range(0.35, 0.55), yaw.sin() * 0.18);
        let pts = curve(Vec3::ZERO, top, Vec3::Y * 0.05, 2);
        p.paint(reg, "green_stem", c(80, 112, 46), 0.6);
        p.ribbon(&pts, &[0.008], &[ONE], 1.0, |_| {
            Vec3::new(-yaw.sin(), 0.0, yaw.cos())
        });
        // compound leaves: pairs of leaflets along the stem, one at its tip
        p.paint(reg, "potato_leaf", c(56, 106, 38), 0.6);
        let tex = p.textured();
        let pairs = [3, 2, 1][lod as usize];
        for k in 0..pairs {
            let at = pts[0].lerp(top, 0.35 + 0.6 * k as f32 / pairs as f32);
            for sgn in [-1.0f32, 1.0] {
                let ly = yaw + sgn * 1.3 + rng.range(-0.3, 0.3);
                p.card(
                    at,
                    dir_at(ly, 0.1),
                    0.09,
                    move |t| {
                        if tex {
                            0.032
                        } else {
                            0.035 * (t * PI).sin().max(0.0)
                        }
                    },
                    1.5,
                    0.2,
                    2,
                    ONE * 0.92,
                    ONE,
                    1.0,
                );
            }
        }
        p.card(
            top,
            dir_at(yaw, 0.4),
            0.1,
            move |t| {
                if tex {
                    0.036
                } else {
                    0.04 * (t * PI).sin().max(0.0)
                }
            },
            1.0,
            0.2,
            2,
            ONE,
            ONE,
            1.0,
        );
    }
    // a few flowers on top
    if lod < 2 && rng.f32() < 0.6 {
        p.paint(
            reg,
            "potato_flower",
            if rng.f32() < 0.5 {
                c(240, 238, 246)
            } else {
                c(184, 152, 222)
            },
            0.5,
        );
        for _ in 0..3 {
            let at = Vec3::new(
                rng.range(-0.12, 0.12),
                rng.range(0.5, 0.6),
                rng.range(-0.12, 0.12),
            );
            p.fan(
                at,
                Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)),
                0.022,
                -0.3,
                0.2,
                6,
                1,
                ONE,
                ONE,
                1.0,
            );
        }
    }
    p
}

/// A carrot: a rosette of finely divided, feathery fronds over its orange shoulder.
fn carrot(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.35);
    p.paint(reg, "carrot_frond", c(66, 138, 40), 0.65);
    let tex = p.textured();
    // (fresh, light green: the atlas frond a little brighter)
    let fresh = if tex { Vec3::new(1.12, 1.2, 1.0) } else { ONE };
    let fronds = [10, 6, 3][lod as usize];
    for i in 0..fronds {
        let yaw = i as f32 / fronds as f32 * TAU + rng.f32() * 0.8;
        let len = rng.range(0.22, 0.36);
        let w = if tex { len * 0.26 } else { 0.03 };
        // (on its own little stalk, then the feathery blade arching out)
        p.card(
            Vec3::Y * 0.005,
            dir_at(yaw, rng.range(0.78, 0.95)),
            len,
            move |t| {
                if tex {
                    w * 0.5
                } else {
                    w * (t * PI).sin().max(0.0)
                }
            },
            rng.range(1.0, 2.0),
            0.05,
            [3, 2, 2][lod as usize],
            fresh * rng.range(0.88, 1.0),
            fresh * 1.05,
            1.0,
        );
    }
    if lod == 0 {
        p.paint(reg, "carrot_root", c(236, 118, 28), 0.5);
        p.blob(
            Vec3::Y * 0.006,
            Quat::IDENTITY,
            Vec3::new(0.017, 0.016, 0.017),
            6,
            3,
            ONE,
            ONE * 0.9,
            0,
            0.0,
            0.3,
            0.0,
        );
    }
    p
}

/// A leaf's own shade (deterministic, no rng borrow inside closures).
fn rng_tint(i: u32, k: f32) -> f32 {
    0.86 + 0.14
        * (((i as f32 * 12.9898 + k * 78.233).sin() * 43_758.547)
            .fract()
            .abs())
}

/// A strawberry: a rosette of trifoliate leaves on hairy stalks, berries hanging on their
/// stalks with green calyxes, a flower or two.
fn strawberry(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.24);
    let leaves = [10, 5, 3][lod as usize];
    for i in 0..leaves {
        // a full mound: the leaf stalks rise close round the crown (the young ones tallest in
        // the middle), and each leaf's three leaflets fan out their own way, overlapping
        let t = i as f32 / leaves as f32;
        let yaw = i as f32 * 2.39996 + rng.f32() * 0.4;
        let reach = 0.075 * t.sqrt() * rng.range(0.6, 1.0);
        let top = Vec3::new(
            yaw.cos() * reach,
            rng.range(0.1, 0.17) * (1.0 - 0.35 * t) + 0.04,
            yaw.sin() * reach,
        );
        let d = (top - Vec3::Y * 0.01).normalize();
        p.paint(reg, "green_stem", c(92, 120, 50), 0.6);
        if lod == 0 {
            p.tube(
                &curve(Vec3::Y * 0.01, top, Vec3::new(d.x, 0.0, d.z) * 0.03, 2),
                &[0.0025, 0.002],
                3,
                &[ONE],
                1.0,
                0.0,
                0.3,
            );
        } else {
            p.ribbon(&[Vec3::Y * 0.01, top], &[0.004], &[ONE], 1.0, |_| {
                Vec3::new(-d.z, 0.0, d.x)
            });
        }
        // trifoliate: three toothed leaflets
        p.paint(reg, "strawberry_leaf", c(50, 104, 38), 0.4);
        let tex = p.textured();
        let facing = yaw + rng.range(-1.3, 1.3);
        for k in [-1.0f32, 0.0, 1.0] {
            let ly = facing + k * 0.85;
            p.card(
                top,
                dir_at(ly, 0.3 - 0.35 * t - 0.12 * k.abs()),
                0.12,
                move |t| {
                    if tex {
                        0.058
                    } else {
                        0.058 * (t * PI).sin().max(0.0).powf(0.7)
                    }
                },
                1.2,
                0.25,
                if lod == 0 { 2 } else { 1 },
                ONE * rng_tint(i, k),
                ONE,
                0.8,
            );
        }
    }
    // berries hang on their stalks
    let n = [2, 1, 0][lod as usize];
    for _ in 0..n {
        let a = rng.f32() * TAU;
        let d = Vec3::new(a.cos(), 0.0, a.sin());
        let hang = d * rng.range(0.12, 0.2) + Vec3::Y * 0.03;
        let peak = d * 0.06 + Vec3::Y * 0.08;
        p.paint(reg, "green_stem", c(92, 120, 50), 0.6);
        p.tube(
            &[Vec3::Y * 0.01, peak, hang + Vec3::Y * 0.022],
            &[0.0018],
            3,
            &[ONE],
            1.0,
            0.0,
            0.3,
        );
        let ripe = rng.f32();
        let (name, base) = if ripe < 0.2 {
            ("strawberry_berry", c(222, 222, 160))
        } else {
            ("strawberry_berry", c(204, 22, 30))
        };
        p.paint(reg, name, base, 0.3);
        let tint = if p.textured() && ripe < 0.2 {
            Vec3::new(1.0, 1.4, 0.9)
        } else {
            ONE
        };
        let rot = Quat::from_rotation_z(rng.range(-0.4, 0.4));
        p.blob(
            hang,
            rot,
            Vec3::new(0.016, 0.02, 0.016),
            [7, 5, 4][lod as usize],
            [5, 4, 3][lod as usize],
            tint,
            tint * 0.9,
            0,
            0.0,
            0.55,
            0.3,
        );
        if lod < 2 {
            p.paint(reg, "strawberry_calyx", c(70, 120, 40), 0.6);
            p.fan(
                hang + Vec3::Y * 0.02,
                d,
                0.022,
                -0.25,
                0.3,
                10,
                1,
                ONE,
                ONE,
                0.3,
            );
        }
    }
    if lod == 0 && rng.f32() < 0.5 {
        let at = Vec3::new(rng.range(-0.08, 0.08), 0.15, rng.range(-0.08, 0.08));
        p.paint(reg, "green_stem", c(92, 120, 50), 0.6);
        p.tube(&[Vec3::Y * 0.01, at], &[0.0018], 3, &[ONE], 1.0, 0.0, 0.3);
        p.paint(reg, "strawberry_flower", c(246, 246, 240), 0.5);
        p.fan(at, Vec3::X, 0.02, 0.0, 0.15, 10, 1, ONE, ONE, 1.0);
    }
    p
}

fn tomato(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let h = rng.range(1.1, 1.45);
    let mut p = Plant::new(h);
    // a brown cane stake
    p.paint(reg, "wood_post", c(124, 92, 60), 0.85);
    let cane = if p.textured() {
        Vec3::new(0.85, 0.68, 0.5)
    } else {
        ONE
    };
    p.tube(
        &[Vec3::new(0.06, 0.0, 0.0), Vec3::new(0.06, h * 1.1, 0.0)],
        &[0.011],
        4,
        &[cane],
        0.0,
        0.0,
        1.0,
    );
    p.cap(
        Vec3::new(0.06, h * 1.1, 0.0),
        Vec3::Y,
        0.011,
        4,
        cane * 0.85,
        0.0,
    );
    let pts = curve(
        Vec3::ZERO,
        Vec3::new(0.04, h, 0.0),
        Vec3::new(-0.04, 0.0, 0.03),
        [4, 2, 1][lod as usize],
    );
    p.paint(reg, "green_stem", c(84, 118, 52), 0.6);
    p.tube(&pts, &[0.012, 0.007], 4, &[ONE], 0.6, 0.0, 1.0);
    p.paint(reg, "tomato_leaf", c(58, 108, 40), 0.6);
    let tex = p.textured();
    let leaves = [26, 12, 6][lod as usize];
    for i in 0..leaves {
        let t = 0.08 + 0.88 * i as f32 / leaves as f32;
        let at = pts[0].lerp(*pts.last().unwrap(), t);
        let yaw = i as f32 * 2.4 + rng.f32();
        let size = rng.range(0.22, 0.3) * (1.0 - 0.35 * t);
        p.card(
            at,
            dir_at(yaw, rng.range(-0.1, 0.3)),
            size,
            move |t| {
                if tex {
                    size * 0.42
                } else {
                    size * 0.35 * (t * PI).sin().max(0.0)
                }
            },
            2.0,
            0.25,
            [3, 2, 2][lod as usize],
            ONE * rng.range(0.85, 1.0),
            ONE,
            0.7,
        );
    }
    // filler clumps of leaves make it a bush
    if reg.contains_key("leaf_clump") {
        p.paint(reg, "leaf_clump", c(60, 104, 44), 0.65);
        for i in 0..[5, 3, 2][lod as usize] {
            let t = 0.25 + 0.6 * i as f32 / 5.0;
            let yaw = i as f32 * 2.1 + rng.f32();
            let at =
                pts[0].lerp(*pts.last().unwrap(), t) + Vec3::new(yaw.cos(), 0.0, yaw.sin()) * 0.08;
            p.fan(
                at,
                Vec3::new(yaw.cos(), 0.0, yaw.sin()),
                rng.range(0.16, 0.22),
                rng.range(0.4, 0.8),
                0.15,
                [10, 8, 6][lod as usize],
                1,
                ONE * rng.range(0.85, 1.0),
                ONE * 0.9,
                0.6,
            );
        }
    }
    let fruit = [8, 4, 2][lod as usize];
    for i in 0..fruit {
        let t = 0.2 + 0.55 * rng.f32();
        let a = rng.f32() * TAU;
        let at = pts[0].lerp(*pts.last().unwrap(), t) + Vec3::new(a.cos(), -0.03, a.sin()) * 0.08;
        let ripe = if i % 3 == 0 { rng.f32() } else { 1.0 };
        p.paint(
            reg,
            "tomato_fruit",
            c(214, 40, 28).lerp(c(140, 176, 62), 1.0 - ripe),
            0.25,
        );
        let tint = if p.textured() {
            ONE.lerp(Vec3::new(0.7, 1.8, 0.8), (1.0 - ripe) * 0.6)
        } else {
            ONE
        };
        p.blob(
            at,
            Quat::IDENTITY,
            Vec3::splat(0.035) * rng.range(0.8, 1.2),
            [8, 6, 4][lod as usize],
            [6, 4, 3][lod as usize],
            tint,
            tint,
            0,
            0.0,
            0.0,
            0.4,
        );
    }
    p
}

/// A grapevine on its trellis: a gnarled trunk, two cordon arms along the wire, upright shoots
/// carrying the leaves, and bunches hanging on their stalks — berries packed round the rachis.
fn grapevine(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(1.8);
    let arm_h = 0.9;
    p.paint(reg, "vine_bark", c(92, 66, 44), 0.9);
    let trunk = curve(
        Vec3::ZERO,
        Vec3::new(rng.range(-0.04, 0.04), arm_h, 0.0),
        Vec3::new(rng.range(-0.06, 0.06), 0.0, rng.range(-0.05, 0.05)),
        [4, 2, 1][lod as usize],
    );
    p.tube(
        &trunk,
        &[0.045, 0.032, 0.036],
        [6, 4, 3][lod as usize],
        &[ONE],
        0.0,
        0.0,
        1.0,
    );
    let mut spurs = vec![];
    for s in [-1.0f32, 1.0] {
        let end = Vec3::new(0.64 * s, arm_h + 0.03, rng.range(-0.02, 0.02));
        let arm = curve(
            Vec3::new(0.0, arm_h, 0.0),
            end,
            Vec3::new(0.0, 0.06, 0.0),
            [3, 2, 1][lod as usize],
        );
        p.tube(
            &arm,
            &[0.026, 0.017],
            [5, 4, 3][lod as usize],
            &[ONE],
            0.0,
            0.0,
            0.6,
        );
        for k in 0..3 {
            spurs.push(Vec3::new(s * (0.15 + 0.2 * k as f32), arm_h + 0.04, 0.0));
        }
    }
    // shoots trained up between the catch wires, leaves along them facing out of the row
    let leaves_per = [8, 4, 2][lod as usize];
    for (i, s0) in spurs.iter().enumerate() {
        let top = *s0
            + Vec3::new(
                rng.range(-0.08, 0.08),
                rng.range(0.65, 0.85),
                rng.range(-0.06, 0.06),
            );
        let shoot = curve(
            *s0,
            top,
            Vec3::new(0.0, 0.0, rng.range(-0.05, 0.05)),
            [3, 2, 1][lod as usize],
        );
        p.paint(reg, "green_stem", c(110, 128, 60), 0.6);
        p.tube(&shoot, &[0.007, 0.004], 3, &[ONE], 0.7, 0.0, 0.5);
        for k in 0..leaves_per {
            let t = (k as f32 + 0.5) / leaves_per as f32;
            let at = s0.lerp(top, t);
            let out = if (k + i) % 2 == 0 { 1.0 } else { -1.0 };
            let ly = PI * 0.5 * out + rng.range(-0.7, 0.7);
            let d = Vec3::new(ly.cos(), 0.0, ly.sin());
            let pet = at + d * 0.06 + Vec3::Y * 0.02;
            p.paint(reg, "green_stem", c(110, 128, 60), 0.6);
            p.tube(&[at, pet], &[0.0025], 3, &[ONE], 0.8, 0.0, 0.1);
            p.paint(reg, "grape_leaf", c(70, 118, 40), 0.7);
            p.fan(
                pet,
                d,
                rng.range(0.11, 0.15),
                rng.range(0.55, 0.95),
                0.12,
                [12, 8, 6][lod as usize],
                1,
                ONE * rng.range(0.9, 1.06),
                ONE * 0.85,
                0.4 + 0.5 * t,
            );
        }
    }
    // bunches: a peduncle from a spur, a tapering rachis, berries packed round it
    let bunches = [4, 3, 2][lod as usize];
    for b in 0..bunches {
        let s0 = spurs[(b * 2 + 1) % spurs.len()];
        let start = s0 + Vec3::new(rng.range(-0.04, 0.04), -0.01, rng.range(-0.05, 0.05));
        let len = rng.range(0.12, 0.17);
        let rachis = [
            start,
            start + Vec3::new(0.01, -0.035, 0.0),
            start + Vec3::new(0.015, -0.035 - len, 0.0),
        ];
        p.paint(reg, "grape_stalk", c(110, 110, 60), 0.7);
        p.tube(&rachis, &[0.004, 0.003, 0.002], 3, &[ONE], 0.2, 0.0, 1.0);
        let berries = [34, 16, 7][lod as usize];
        p.paint(reg, "grape_berry", c(70, 30, 82), 0.55);
        for k in 0..berries {
            let t = (k as f32 + 0.5) / berries as f32;
            let a = k as f32 * 2.39996;
            let along = rachis[1].lerp(rachis[2], t);
            // a cone: widest near the top of the bunch, with a little shoulder
            let rr = 0.032 * (1.0 - t).powf(0.7) + 0.006 + if t < 0.2 { 0.008 } else { 0.0 };
            let at = along + Vec3::new(a.cos() * rr, rng.range(-0.006, 0.006), a.sin() * rr);
            let s = rng.range(0.0075, 0.0092);
            p.blob(
                at,
                Quat::IDENTITY,
                Vec3::new(s, s * 1.08, s),
                [6, 5, 4][lod as usize],
                [4, 3, 2][lod as usize],
                ONE * rng.range(0.85, 1.12),
                ONE,
                0,
                0.0,
                0.0,
                0.2,
            );
        }
    }
    p
}

/// A row's posts and wires (vineyard, hops), across the whole tile.
fn trellis(height: f32, wires: &[f32], lod: u8, tall: bool, reg: &Regions) -> Plant {
    let mut p = Plant::new(height);
    let first = p.pos.len();
    p.paint(reg, "wood_post", c(112, 86, 60), 0.85);
    for x in [-TILE * 0.5 + 0.5, 0.5] {
        let r = if tall { 0.06 } else { 0.04 };
        // (tall hop poles are old dark timber)
        let wood = if tall {
            Vec3::new(0.62, 0.56, 0.5)
        } else {
            ONE
        };
        p.tube(
            &[Vec3::new(x, -0.05, 0.0), Vec3::new(x, height, 0.0)],
            &[r, r * 0.9],
            [6, 4, 3][lod as usize],
            &[wood],
            0.0,
            0.0,
            1.0,
        );
        p.cap(
            Vec3::new(x, height, 0.0),
            Vec3::Y,
            r * 0.9,
            [6, 4, 3][lod as usize],
            wood * 0.85,
            0.0,
        );
    }
    if lod < 2 {
        p.plain(c(150, 150, 146), 0.35);
        for &h in wires {
            p.ribbon(
                &[
                    Vec3::new(-TILE * 0.5, h, 0.0),
                    Vec3::new(TILE * 0.5, h, 0.0),
                ],
                &[0.005],
                &[ONE],
                0.0,
                |_| Vec3::Y,
            );
        }
    }
    p.hug_ground_from(first);
    p
}

/// A hop bine climbing its string to the overhead wire, with leaves and cones.
fn hop_bine(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let h = 5.2;
    let mut p = Plant::new(h);
    let lean = rng.range(-0.5, 0.5);
    let top = Vec3::new(lean, h, 0.0);
    p.plain(c(180, 168, 136), 0.8);
    p.ribbon(&[Vec3::ZERO, top], &[0.006], &[ONE], 0.4, |_| Vec3::Z);
    // a column of foliage up the string, fuller towards the top where the side arms grow
    p.paint(reg, "hop_leaf", c(60, 118, 40), 0.7);
    let leaves = [96, 36, 12][lod as usize];
    for i in 0..leaves {
        let t = 0.06 + 0.92 * i as f32 / leaves as f32;
        let spread = 0.12 + 0.2 * t;
        let at = top * t + Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)) * spread;
        let yaw = rng.f32() * TAU;
        p.fan(
            at,
            Vec3::new(yaw.cos(), 0.0, yaw.sin()),
            rng.range(0.12, 0.19),
            rng.range(0.6, 0.95),
            0.1,
            [9, 7, 5][lod as usize],
            1,
            ONE * rng.range(0.85, 1.05),
            ONE * 0.85,
            0.4 + 0.6 * t,
        );
    }
    if reg.contains_key("leaf_clump") {
        p.paint(reg, "leaf_clump", c(60, 104, 44), 0.7);
        for i in 0..[16, 8, 4][lod as usize] {
            let t = 0.15 + 0.8 * i as f32 / 16.0_f32.min([16.0, 8.0, 4.0][lod as usize]);
            let yaw = i as f32 * 2.39996 + rng.f32();
            let d = Vec3::new(yaw.cos(), 0.0, yaw.sin());
            p.fan(
                top * t.min(0.97) + d * (0.1 + 0.15 * t),
                d,
                rng.range(0.3, 0.42),
                0.95,
                0.1,
                [10, 8, 6][lod as usize],
                1,
                ONE * rng.range(0.85, 1.0),
                ONE * 0.9,
                0.4 + 0.6 * t,
            );
        }
    }
    p.paint(reg, "hop_cone", c(156, 192, 92), 0.6);
    let cones = [26, 12, 0][lod as usize];
    for _ in 0..cones {
        let t = rng.range(0.35, 0.95);
        let at =
            top * t + Vec3::new(rng.range(-0.18, 0.18), 0.0, rng.range(-0.18, 0.18)) * (0.5 + t);
        p.blob(
            at,
            Quat::IDENTITY,
            Vec3::new(0.018, 0.03, 0.018),
            6,
            4,
            ONE,
            ONE,
            0,
            0.0,
            0.3,
            0.4 + 0.6 * t,
        );
    }
    p
}

fn cotton(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let h = rng.range(0.8, 1.1);
    let mut p = Plant::new(h);
    let branches = [6, 4, 2][lod as usize];
    p.paint(reg, "green_stem", c(104, 78, 54), 0.7);
    let brown = if p.textured() {
        Vec3::new(1.1, 0.75, 0.6)
    } else {
        ONE
    };
    p.tube(
        &[Vec3::ZERO, Vec3::Y * h * 0.8],
        &[0.012, 0.007],
        4,
        &[brown],
        0.6,
        0.0,
        1.0,
    );
    for b in 0..branches {
        let yaw = b as f32 * 2.4 + rng.f32();
        let at = Vec3::Y * h * (0.25 + 0.5 * b as f32 / branches as f32);
        let end = at + dir_at(yaw, 0.55) * h * 0.4;
        p.paint(reg, "green_stem", c(104, 78, 54), 0.7);
        p.ribbon(&[at, end], &[0.008, 0.005], &[brown], 0.8, |_| Vec3::Y);
        p.paint(reg, "cotton_leaf", c(62, 102, 40), 0.7);
        for _ in 0..[4, 3, 1][lod as usize] {
            p.fan(
                at.lerp(end, rng.range(0.2, 1.0)),
                Vec3::new(rng.range(-1.0, 1.0), 0.0, rng.range(-1.0, 1.0)),
                rng.range(0.1, 0.14),
                rng.range(0.2, 0.6),
                0.15,
                [8, 6, 5][lod as usize],
                1,
                ONE * rng.range(0.85, 1.0),
                ONE * 0.85,
                0.8,
            );
        }
        p.paint(reg, "cotton_boll", c(246, 246, 240), 0.95);
        for _ in 0..[2, 2, 1][lod as usize] {
            let at = at.lerp(end, rng.range(0.4, 1.0)) + Vec3::Y * 0.03;
            p.blob(
                at,
                Quat::IDENTITY,
                Vec3::new(0.036, 0.032, 0.036),
                [6, 5, 4][lod as usize],
                4,
                ONE,
                ONE * 0.95,
                4,
                0.25,
                0.0,
                0.8,
            );
        }
    }
    p
}

/// A clipped tea hedge along the row, across the tile: a rounded mound, leaf flushes on top.
fn tea_hedge(rng: &mut Rng, lod: u8, reg: &Regions) -> Plant {
    let mut p = Plant::new(0.9);
    let (body, top) = (c(36, 92, 36), c(92, 150, 52));
    let (w, h) = (0.55, 0.85);
    // the mound's surface at x along the row, angle a round its cross-section (0 … π)
    let surf = |x: f32, a: f32| {
        let lump = (x * 3.1).sin() * (a * 5.0).cos() * 0.06
            + (x * 9.7 + a * 3.0).sin() * (a * 11.0).cos() * 0.035;
        let d = Vec3::new(0.0, a.sin(), -a.cos());
        let pos = Vec3::new(x, d.y * h * (1.0 + lump), d.z * w * (1.0 + lump));
        let nrm = Vec3::new((x * 9.7).cos() * 0.15, d.y, d.z / w * h).normalize();
        let shade = 0.82 + 0.36 * (0.5 + 0.5 * ((x * 7.3).sin() * (a * 9.1).cos()));
        (pos, nrm, shade)
    };
    let first = p.pos.len();
    if reg.contains_key("tea_canopy") {
        // the photo-like sheared foliage, repeating every metre along and half way round
        p.paint(reg, "tea_canopy", top, 0.5);
        let steps = [4u32, 2, 1][lod as usize];
        let around = [4u32, 3, 2][lod as usize];
        for seg in 0..8 {
            let x0 = -TILE * 0.5 + seg as f32;
            for half in 0..2 {
                let a0 = half as f32 * PI * 0.5;
                let start = p.pos.len() as u32;
                for i in 0..=steps {
                    let u = i as f32 / steps as f32;
                    for j in 0..=around {
                        let v = j as f32 / around as f32;
                        let a = a0 + v * PI * 0.5;
                        let (pos, nrm, shade) = surf(x0 + u, a);
                        // (lighter where the sun and the new growth are: the top)
                        let tint = ONE * shade * (0.82 + 0.25 * a.sin().powf(3.0));
                        p.vert(pos, nrm, tint, 0.0, u, v);
                    }
                }
                for i in 0..steps {
                    for j in 0..around {
                        let k = start + i * (around + 1) + j;
                        p.quad(k, k + around + 1, k + around + 2, k + 1);
                    }
                }
            }
        }
    } else {
        p.plain(Vec3::ONE, 0.6);
        let along = [33, 17, 9][lod as usize];
        let around = [9, 7, 5][lod as usize];
        for i in 0..along {
            let x = i as f32 / (along - 1) as f32 * TILE - TILE * 0.5;
            for j in 0..around {
                let a = j as f32 / (around - 1) as f32 * PI;
                let (pos, nrm, shade) = surf(x, a);
                p.vert(
                    pos,
                    nrm,
                    body.lerp(top, a.sin().powf(3.0)) * shade,
                    0.0,
                    0.0,
                    0.0,
                );
            }
        }
        for i in 0..along as u32 - 1 {
            for j in 0..around as u32 - 1 {
                let a = first as u32 + i * around as u32 + j;
                p.quad(a, a + around as u32, a + around as u32 + 1, a + 1);
            }
        }
    }
    p.hug_ground_from(first);
    // fresh leaf flushes on top
    p.paint(reg, "tea_leaf", top, 0.4);
    let tex = p.textured();
    let flush = [80, 24, 0][lod as usize];
    for _ in 0..flush {
        let x = rng.range(-TILE * 0.5, TILE * 0.5);
        let z = rng.range(-0.4, 0.4) * w;
        let y = h * (1.0 - (z / w).powi(2)).max(0.0).sqrt() * 0.98;
        let yaw = rng.f32() * TAU;
        let f0 = p.pos.len();
        p.card(
            Vec3::new(x, y, z),
            dir_at(yaw, 0.6),
            0.07,
            move |t| {
                if tex {
                    0.022
                } else {
                    0.022 * (t * PI).sin().max(0.0)
                }
            },
            1.0,
            0.2,
            2,
            ONE,
            ONE,
            0.0,
        );
        p.hug_ground_from(f0);
    }
    p
}

/// How a crop's rows are laid out in a tile, and its plants.
struct Rows {
    /// row spacing and spacing along the row (m), rounded so the tile repeats seamlessly
    row: f32,
    step: f32,
    jitter: f32,
    /// plant variants made per level of detail
    variants: u32,
    /// twin rows (strawberry beds): two rows this far either side of each row's centre line
    twin: f32,
}

fn rows(crop: Crop) -> Rows {
    use Crop::*;
    let r = |row: f32, step: f32, jitter: f32| Rows {
        row,
        step,
        jitter,
        variants: 8,
        twin: 0.0,
    };
    match crop {
        Wheat | Barley | Rye => r(0.25, 0.26, 0.07),
        Rice => r(0.35, 0.3, 0.04),
        Corn => r(0.76, 0.3, 0.04),
        Sunflowers => r(0.76, 0.36, 0.05),
        Lavender => r(1.15, 0.58, 0.05),
        Tulips => r(0.36, 0.17, 0.03),
        Hay => r(0.4, 0.4, 0.15),
        Pumpkins => r(1.6, 1.3, 0.25),
        Cabbages => r(0.6, 0.48, 0.05),
        Potatoes => r(0.8, 0.38, 0.05),
        Carrots => r(0.4, 0.16, 0.03),
        // raised straw beds 1.15 m apart, two rows of plants on each
        Strawberries => Rows {
            twin: 0.18,
            ..r(1.15, 0.31, 0.04)
        },
        Tomatoes => r(1.0, 0.6, 0.03),
        Vineyard => r(2.0, 1.33, 0.0),
        Hops => r(2.7, 1.6, 0.0),
        Cotton => r(0.9, 0.36, 0.06),
        Tea => r(1.6, 8.0, 0.0),
        Plowed => r(0.6, 8.0, 0.0),
    }
}

fn plant(crop: Crop, rng: &mut Rng, lod: u8, variant: u32, reg: &Regions) -> Plant {
    use Crop::*;
    let wheat = Cereal {
        height: (0.82, 1.02),
        stalks: 3,
        stalk: c(196, 166, 84),
        leaf: c(170, 152, 84),
        ear: c(220, 178, 92),
        ear_len: 0.09,
        ear_w: 0.024,
        nod: 0.05,
        leafy: false,
        leaf_region: "cereal_leaf",
        ear_region: "wheat_ear",
    };
    match crop {
        Wheat => cereal(rng, &wheat, lod, reg),
        Barley => cereal(
            rng,
            &Cereal {
                height: (0.7, 0.88),
                stalks: 2,
                ear: c(212, 190, 116),
                ear_len: 0.16,
                ear_w: 0.05,
                nod: 0.55,
                ear_region: "barley_ear",
                ..wheat
            },
            lod,
            reg,
        ),
        Rye => cereal(
            rng,
            &Cereal {
                height: (1.2, 1.5),
                stalks: 2,
                ear: c(186, 164, 110),
                ear_len: 0.13,
                ear_w: 0.03,
                nod: 0.2,
                ear_region: "rye_ear",
                ..wheat
            },
            lod,
            reg,
        ),
        Rice => cereal(
            rng,
            &Cereal {
                height: (0.75, 0.95),
                stalks: 3,
                stalk: c(96, 140, 56),
                leaf: c(72, 140, 44),
                ear: c(196, 186, 98),
                ear_len: 0.18,
                ear_w: 0.06,
                nod: 0.9,
                leafy: true,
                leaf_region: "rice_leaf",
                ear_region: "rice_panicle",
            },
            lod,
            reg,
        ),
        Corn => corn(rng, lod, reg),
        Sunflowers => sunflower(rng, lod, reg),
        Lavender => lavender(rng, lod, reg),
        // (the colour comes from the row: see `patch`)
        Tulips => tulip(rng, lod, variant as usize, reg),
        Hay => stubble(rng, lod, reg),
        Pumpkins => pumpkin_patch(rng, lod, reg),
        Cabbages => cabbage(rng, lod, reg),
        // (on their ridge or bed: the crest's height where they stand)
        Potatoes => potato(rng, lod, reg).lift(0.16),
        Carrots => carrot(rng, lod, reg).lift(0.055),
        Strawberries => strawberry(rng, lod, reg).lift(0.115),
        Tomatoes => tomato(rng, lod, reg),
        Vineyard => grapevine(rng, lod, reg),
        Hops => hop_bine(rng, lod, reg),
        Cotton => cotton(rng, lod, reg),
        Tea => tea_hedge(rng, lod, reg),
        Plowed => furrows(lod, reg),
    }
}

/// Once-per-row pieces (ridges, beds, trellis).
fn row_pieces(crop: Crop, lod: u8, reg: &Regions) -> Vec<(Plant, f32)> {
    use Crop::*;
    match crop {
        Potatoes => vec![(
            ridge(
                0.62,
                0.2,
                lod,
                Vec3::splat(1.05),
                Vec3::splat(0.8),
                reg,
                Surface::Soil(Layer::Dirt),
                0.2,
            ),
            0.0,
        )],
        Carrots => vec![(
            ridge(
                0.3,
                0.07,
                lod,
                Vec3::splat(1.05),
                Vec3::splat(0.85),
                reg,
                Surface::Soil(Layer::Dirt),
                0.15,
            ),
            0.0,
        )],
        Strawberries => vec![(
            ridge(
                0.82,
                0.15,
                lod,
                Vec3::splat(1.0),
                Vec3::splat(0.85),
                reg,
                Surface::Atlas("straw", c(206, 186, 118)),
                0.1,
            ),
            0.0,
        )],
        Vineyard => vec![(trellis(1.75, &[0.9, 1.3, 1.7], lod, false, reg), 0.0)],
        Hops => vec![(trellis(5.4, &[5.3], lod, true, reg), 0.0)],
        _ => vec![],
    }
}

/// The patch vertex data, before it becomes a mesh.
#[derive(Default)]
pub struct Patch {
    pub pos: Vec<[f32; 3]>,
    pub nor: Vec<[f32; 3]>,
    /// tint (atlas) or colour (plain), and roughness in alpha
    pub col: Vec<[f32; 4]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// [u, v, kind, aux] for the shader (`kind`)
    pub tan: Vec<[f32; 4]>,
    pub idx: Vec<u32>,
    /// the tallest plant (m), for culling bounds
    pub height: f32,
}

impl Patch {
    /// Adds `p` with its base at `at` (tile coordinates, 0…TILE), turned by `yaw`, sized `k`;
    /// `chance`: an optional piece's share of tiles (0: always).
    fn put(&mut self, p: &Plant, at: Vec2, yaw: f32, k: f32, chance: f32) {
        let base = self.pos.len() as u32;
        let rot = Quat::from_rotation_y(yaw);
        for i in 0..p.pos.len() {
            let v = rot * (p.pos[i] * k);
            let n = rot * p.nor[i];
            let bend = (p.pos[i].y / p.height.max(0.05)).clamp(0.0, 1.2).powf(1.5) * p.flex[i];
            let col = p.col[i];
            self.pos.push(v.to_array());
            self.nor.push(n.to_array());
            self.col.push([col.x, col.y, col.z, p.rough[i]]);
            self.uv0.push([bend, chance]);
            let mut tan = p.tan[i];
            // Tulip part tags reserve their fractional component for the stem height.  Scale it
            // with this instance so the shader's head pivot remains at the actual stem tip.
            // Ground and tiled-atlas metadata retain their exact existing payloads.
            if tulip_aux::is_part(tan[3]) {
                tan[3] = tan[3].floor() + tan[3].fract() * k;
            }
            self.tan.push(tan);
            // a vertex that hugs the ground finds its own: its base is itself
            let slot = if p.own_ground[i] {
                at + Vec2::new(v.x, v.z)
            } else {
                at
            };
            self.uv1.push(slot.to_array());
            if p.own_ground[i] {
                // (its height then counts from its own ground)
                self.pos.last_mut().unwrap()[0] = 0.0;
                self.pos.last_mut().unwrap()[2] = 0.0;
            }
        }
        self.idx.extend(p.idx.iter().map(|i| i + base));
        self.height = self.height.max(p.height * k * 1.2);
    }

    /// Adds one of a crop model's plants (a level of detail of a variant, `assets`).
    fn put_raw(&mut self, m: &crate::photoscan::RawMesh, at: Vec2, yaw: f32, k: f32, rough: f32) {
        let base = self.pos.len() as u32;
        let rot = Quat::from_rotation_y(yaw);
        let h = m.pos.iter().map(|p| p[1]).fold(0.05f32, f32::max);
        for i in 0..m.pos.len() {
            let v = rot * (Vec3::from_array(m.pos[i]) * k);
            self.pos.push(v.to_array());
            self.nor.push((rot * Vec3::from_array(m.nrm[i])).to_array());
            self.col.push([1.0, 1.0, 1.0, rough]);
            // (the model's sway weights, or else its height)
            let sway = m
                .sway
                .get(i)
                .copied()
                .unwrap_or_else(|| (m.pos[i][1] / h).clamp(0.0, 1.0).powf(1.5));
            self.uv0.push([sway, 0.0]);
            self.tan.push([m.uv[i][0], m.uv[i][1], kind::ATLAS, 0.0]);
            self.uv1.push(at.to_array());
        }
        self.idx.extend(m.idx.iter().map(|i| *i as u32 + base));
        self.height = self.height.max(h * k * 1.2);
    }

    pub fn mesh(&self) -> Mesh {
        let mut m = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.pos.clone());
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.nor.clone());
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uv0.clone());
        m.insert_attribute(Mesh::ATTRIBUTE_UV_1, self.uv1.clone());
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.col.clone());
        m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, self.tan.clone());
        m.insert_indices(Indices::U32(self.idx.clone()));
        m
    }
}

/// One 8 × 8 m tile of `crop` at a level of detail (0 near … 2 far). The same plants stand in
/// the same places at every level. With a Blender-built `model` the plants are its variants
/// (its level `lod`, or its last), otherwise the procedural ones painted from the atlas
/// `regions` (plain colours where a region is missing).
pub fn patch(
    crop: Crop,
    lod: u8,
    model: Option<&super::assets::CropModel>,
    reg: &Regions,
    colour: Option<u8>,
) -> Patch {
    let lay = rows(crop);
    let nrows = (TILE / lay.row).round().max(1.0) as u32;
    let nstep = (TILE / lay.step).round().max(1.0) as u32;
    let (row, step) = (TILE / nrows as f32, TILE / nstep as f32);
    // the plant variants (same seeds at every level of detail)
    let variants: Vec<Plant> = if model.is_some_and(|m| !m.variants.is_empty()) {
        vec![]
    } else {
        (0..lay.variants)
            .map(|v| {
                plant(
                    crop,
                    &mut Rng::new(crop as u64 * 7919 + v as u64 * 104_729 + 17),
                    lod,
                    v,
                    reg,
                )
            })
            .collect()
    };
    let pieces = row_pieces(crop, lod, reg);
    let mut out = Patch::default();
    // tilled soil between the rows (under grain: dry and strewn with straw; none under the hay
    // meadow or the rice paddy)
    use Crop::*;
    let grain = matches!(crop, Wheat | Barley | Rye);
    if !matches!(crop, Hay | Rice)
        && let Some(f) = floor(lod, ONE * if grain { 0.55 } else { 0.92 }, grain, reg)
    {
        out.put(&f, Vec2::splat(TILE * 0.5), 0.0, 1.0, 0.0);
    }
    let mut rng = Rng::new(crop as u64 * 31 + 5);
    for r in 0..nrows {
        let z = (r as f32 + 0.5) * row;
        for (piece, chance) in &pieces {
            out.put(piece, Vec2::new(TILE * 0.5, z), 0.0, 1.0, *chance);
        }
        // hay: raked windrows of cut hay every few rows, loose stems lying on them
        if crop == Crop::Hay && r % 6 == 2 {
            out.put(
                &ridge(
                    1.25,
                    0.32,
                    lod,
                    Vec3::splat(1.0),
                    Vec3::splat(0.78),
                    reg,
                    Surface::Atlas("straw", c(214, 184, 112)),
                    0.5,
                ),
                Vec2::new(TILE * 0.5, z),
                0.0,
                1.0,
                0.0,
            );
            if lod < 2 {
                out.put(
                    &loose_straw(&mut Rng::new(r as u64 + 11), lod, 1.25, 0.32 * 0.9, reg),
                    Vec2::new(TILE * 0.5, z),
                    0.0,
                    1.0,
                    0.0,
                );
            }
        }
        let twins: &[f32] = if lay.twin > 0.0 { &[-1.0, 1.0] } else { &[0.0] };
        for (s, &side) in (0..nstep).flat_map(|s| twins.iter().map(move |t| (s, t))) {
            // (staggered rows; a hedge or furrow spans the whole tile)
            let stagger = (r % 2 == 1) != (side > 0.0);
            let x = (s as f32 + 0.5) * step
                + if stagger && nstep > 1 {
                    step * 0.5
                } else {
                    0.0
                };
            let z = z + side * lay.twin;
            let at = Vec2::new(
                (x + rng.range(-lay.jitter, lay.jitter)).rem_euclid(TILE),
                (z + rng.range(-lay.jitter, lay.jitter) * 0.5).clamp(0.0, TILE),
            );
            let v = match crop {
                // tulips: the colour changes every few rows (bands across the field)
                Crop::Tulips => colour.map_or((r / 4) % TULIP_COLOURS.len() as u32, u32::from),
                _ => rng.below(lay.variants),
            };
            let (yaw, k) = match crop {
                // rows of hedges and furrows stay square to the row; trellised plants along it;
                // sunflower heads all look the same way
                Crop::Tea | Crop::Plowed => (0.0, 1.0),
                Crop::Vineyard | Crop::Hops => (
                    if rng.f32() < 0.5 { 0.0 } else { PI },
                    rng.range(0.92, 1.06),
                ),
                Crop::Sunflowers => (rng.range(-0.35, 0.35), rng.range(0.88, 1.1)),
                _ => (rng.f32() * TAU, rng.range(0.86, 1.12)),
            };
            match model.filter(|m| !m.variants.is_empty()) {
                Some(m) => {
                    let var = &m.variants[v as usize % m.variants.len()];
                    if let Some(mesh) = var.get(lod as usize).or(var.last()) {
                        out.put_raw(mesh, at, yaw, k, m.roughness);
                    }
                }
                None => out.put(&variants[v as usize], at, yaw, k, 0.0),
            }
        }
    }
    // hay: a round bale in about a third of the tiles
    if crop == Crop::Hay {
        out.put(
            &bale(lod, reg),
            Vec2::new(rng.range(1.5, 6.5), rng.range(1.5, 6.5)),
            rng.f32() * TAU,
            1.0,
            0.33,
        );
    }
    out
}

/// What a crop looks like from afar: its atlas regions and their shares of the view.
fn far_look(crop: Crop) -> &'static [(&'static str, f32)] {
    use Crop::*;
    match crop {
        Wheat => &[("wheat_ear", 0.6), ("cereal_stalk", 0.4)],
        Barley => &[("barley_ear", 0.6), ("cereal_stalk", 0.4)],
        Rye => &[("rye_ear", 0.6), ("cereal_stalk", 0.4)],
        Rice => &[("rice_leaf", 0.75), ("rice_panicle", 0.25)],
        Corn => &[("corn_leaf", 0.92), ("corn_tassel", 0.08)],
        Sunflowers => &[
            ("sunflower_leaf", 0.5),
            ("sunflower_petal", 0.3),
            ("sunflower_disc", 0.2),
        ],
        Lavender => &[("lavender_spike", 0.7), ("lavender_leaf", 0.3)],
        Pumpkins => &[("pumpkin_leaf", 0.82), ("pumpkin_skin", 0.18)],
        Cabbages => &[("cabbage_leaf", 0.6), ("cabbage_head", 0.4)],
        Potatoes => &[("potato_leaf", 1.0)],
        Carrots => &[("carrot_frond", 1.0)],
        Strawberries => &[("strawberry_leaf", 0.8), ("straw", 0.2)],
        Tomatoes => &[("tomato_leaf", 0.85), ("tomato_fruit", 0.15)],
        Vineyard => &[("grape_leaf", 1.0)],
        Hops => &[("hop_leaf", 0.8), ("hop_cone", 0.2)],
        Cotton => &[("cotton_boll", 0.5), ("cotton_leaf", 0.5)],
        Hay | Tulips | Tea | Plowed => &[],
    }
}

/// The far view of a field: its canopy as a surface at the crop's height in its colour — rows
/// of it (with sides) where rows show from afar, an even sheet for grain — drawn far beyond the
/// plants (a "level of detail 3"). The colour comes from the atlas's region averages (`means`)
/// when it has them, so the far field matches its plants. Every vertex finds its own ground;
/// `uv0.y` = -1 marks it for the shader (it rises out of the ground past the plants' draw
/// distance instead of fading).
pub fn canopy(crop: Crop, means: &Regions, colour: Option<u8>) -> Patch {
    use Crop::*;
    // the cheap crops are their own far view
    if matches!(crop, Tea | Plowed) {
        let mut p = patch(crop, 2, None, &Regions::new(), None);
        p.uv0.iter_mut().for_each(|u| *u = [0.0, -1.0]);
        return p;
    }
    let (h, col, rows_show): (f32, (u8, u8, u8), bool) = match crop {
        Wheat => (0.85, (198, 160, 86), false),
        Barley => (0.75, (190, 174, 114), false),
        Rye => (1.25, (176, 164, 104), false),
        Rice => (0.8, (90, 148, 56), false),
        Hay => (0.12, (150, 160, 90), false),
        Corn => (2.2, (64, 98, 38), true),
        Sunflowers => (1.75, (176, 150, 50), true),
        Lavender => (0.6, (110, 96, 146), true),
        Tulips => (0.42, (220, 80, 80), true),
        Pumpkins => (0.3, (96, 112, 40), false),
        Cabbages => (0.26, (132, 174, 124), true),
        Potatoes => (0.45, (64, 98, 42), true),
        Carrots => (0.3, (72, 116, 44), true),
        Strawberries => (0.2, (74, 116, 48), true),
        Tomatoes => (1.15, (86, 104, 44), true),
        Vineyard => (1.6, (74, 116, 42), true),
        Hops => (5.0, (88, 136, 58), true),
        Cotton => (0.9, (196, 194, 176), true),
        Tea | Plowed => unreachable!(),
    };

    let look = far_look(crop);
    let base = if !look.is_empty() && look.iter().all(|(n, _)| means.contains_key(*n)) {
        // (the plants are lit from all sides and shade each other: a little darker than a sheet)
        let srgb: Vec3 = look
            .iter()
            .map(|(n, w)| Vec3::new(means[*n][0], means[*n][1], means[*n][2]) * *w)
            .sum();
        c(
            (srgb.x * 255.0) as u8,
            (srgb.y * 255.0) as u8,
            (srgb.z * 255.0) as u8,
        ) * if rows_show { 0.82 } else { 0.6 }
    } else {
        c(col.0, col.1, col.2)
    };
    // (a little under the plants' tops, so the plants stand out of it while they are drawn)
    let h = h * if rows_show { 0.72 } else { 0.88 };
    let mut out = Patch::default();
    let mut rng = Rng::new(crop as u64 * 97 + 3);
    let lay = rows(crop);
    let nrows = (TILE / lay.row).round().max(1.0) as u32;
    let row = TILE / nrows as f32;
    let along = 5;
    let mut sheet = |out: &mut Patch, z0: f32, z1: f32, col: Vec3, sides: bool| {
        let first = out.pos.len() as u32;
        // across: ground, top, top, ground (sides) or just the top
        let across: Vec<(f32, f32)> = if sides {
            vec![(z0, 0.0), (z0, h), (z1, h), (z1, 0.0)]
        } else {
            (0..along)
                .map(|k| (z0 + (z1 - z0) * k as f32 / (along - 1) as f32, h))
                .collect()
        };
        for i in 0..along {
            let x = i as f32 / (along - 1) as f32 * TILE;
            for (k, (z, y)) in across.iter().enumerate() {
                let n = if sides {
                    [
                        Vec3::new(0.0, 0.3, -1.0),
                        Vec3::Y,
                        Vec3::Y,
                        Vec3::new(0.0, 0.3, 1.0),
                    ][k]
                } else {
                    Vec3::Y
                };
                let shade = if sides {
                    0.9 + 0.2 * rng.f32()
                } else {
                    0.96 + 0.08 * rng.f32()
                } - if *y == 0.0 { 0.25 } else { 0.0 };
                out.pos.push([0.0, *y, 0.0]);
                out.nor.push(n.normalize().to_array());
                out.col
                    .push([col.x * shade, col.y * shade, col.z * shade, 0.8]);
                out.uv0.push([0.0, -1.0]);
                out.uv1.push([x, *z]);
                out.tan.push([0.0, 0.0, kind::PLAIN, 0.0]);
            }
        }
        let w = across.len() as u32;
        for i in 0..along as u32 - 1 {
            for k in 0..w - 1 {
                let a = first + i * w + k;
                out.idx
                    .extend_from_slice(&[a, a + 1, a + w + 1, a, a + w + 1, a + w]);
            }
        }
        out.height = out.height.max(h * 1.2);
    };
    if rows_show && row >= 0.3 {
        for r in 0..nrows {
            let z = (r as f32 + 0.5) * row;
            let col = if crop == Tulips {
                let band = colour.map_or(r / 4, u32::from);
                let (cr, cg, cb) = TULIP_COLOURS[(band % TULIP_COLOURS.len() as u32) as usize];
                c(cr, cg, cb)
            } else {
                base
            };
            let half = row
                * if matches!(crop, Vineyard | Hops) {
                    0.22
                } else {
                    0.36
                };
            sheet(&mut out, z - half, z + half, col, true);
        }
    } else {
        sheet(&mut out, 0.0, TILE, base, false);
    }
    out
}

/// Geometry borrowed by the small-flora library without bringing the field renderer or its
/// shader-specific tangent payload along.  `uv` is ordinary atlas UV data; callers own the
/// material and generate their usual normal-map tangents.
///
/// The supplied regions must name the field parts used by the requested plant.  This lets the
/// field plant recipes serve a separate flora atlas with the same botanical cells, rather than
/// coupling a wild plant to the crop atlas or its field-placement shader.
#[derive(Clone, Debug, Default)]
pub(crate) struct FloraGeometry {
    pub pos: Vec<[f32; 3]>,
    pub nor: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    /// 0 at a planted root; 1 at the freest leaf, flower, or fruit tip.
    pub sway: Vec<f32>,
    pub idx: Vec<u32>,
    pub height: f32,
    /// linear vertex colour: the paint's tint over the atlas, a plain part's own colour
    pub color: Vec<[f32; 3]>,
}

/// Field recipes that also make botanically useful, root-centred small flora.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FieldFlora {
    Lavender,
    WildStrawberry,
    /// a single tulip in one of the field's colours (`TULIP_COLOURS`, wrapping)
    Tulip(u8),
}

/// The colours a tulip comes in (the field's bands, a single tulip's variants).
pub(crate) const TULIP_COLOUR_NAMES: [&str; 6] = ["red", "yellow", "pink", "white", "purple", "orange"];

/// Makes one field-grown plant as ordinary foliage geometry.
///
/// This intentionally preserves only the conventional data a foliage material needs.  In
/// particular, it does not expose `Patch::tan`: that attribute encodes crop-shader surface and
/// placement metadata, which would be invalid for a normal foliage material.
pub(crate) fn flora_geometry(
    kind: FieldFlora,
    seed: u64,
    lod: u8,
    regions: &Regions,
) -> FloraGeometry {
    let lod = lod.min(2);
    let mut rng = Rng::new(seed);
    let plant = match kind {
        FieldFlora::Lavender => lavender(&mut rng, lod, regions),
        FieldFlora::WildStrawberry => strawberry(&mut rng, lod, regions),
        FieldFlora::Tulip(colour) => tulip(&mut rng, lod, colour as usize, regions),
    };
    // plain-colour parts sample the `plain` region's middle (a neutral, opaque texel) when the
    // caller names one; their colour is in the vertex colour
    let plain = regions
        .get("plain")
        .map(|r| [(r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5]);
    FloraGeometry {
        pos: plant.pos.iter().map(|p| p.to_array()).collect(),
        nor: plant.nor.iter().map(|n| n.to_array()).collect(),
        // Plant writes its selected atlas rectangle into tangent.xy.  Those are ordinary UVs;
        // tangent.zw belongs solely to the field shader and is discarded here.
        uv: plant
            .tan
            .iter()
            .map(|t| match plain {
                Some(uv) if t[2] < kind::ATLAS => uv,
                _ => [t[0], t[1]],
            })
            .collect(),
        sway: plant.flex,
        idx: plant.idx,
        height: plant.height,
        color: plant.col.iter().map(|c| c.to_array()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every region named in ATLAS_SPEC.md, at a dummy place, so the textured paths run too.
    fn all_regions() -> Regions {
        let spec = include_str!("../../tools/crops/ATLAS_SPEC.md");
        spec.lines()
            .filter_map(|l| l.strip_prefix("| ")?.split(" |").next().map(str::trim))
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
            .map(|n| (n.to_string(), [0.1, 0.1, 0.2, 0.2]))
            .collect()
    }

    fn tulip_landmarks(seed: u64) -> (f32, f32, Vec3) {
        let mut rng = Rng::new(seed);
        let stem_h = rng.range(0.38, 0.5);
        let yaw = rng.f32() * TAU;
        let bend = Vec3::new(rng.range(-0.018, 0.018), 0.0, rng.range(-0.018, 0.018));
        (stem_h, yaw, Vec3::Y * stem_h + bend)
    }

    #[test]
    fn every_crop_makes_a_tile_within_budget_and_deterministic() {
        let mut over = vec![];
        for reg in [Regions::new(), all_regions()] {
            for crop in Crop::ALL {
                let mut prev = usize::MAX;
                for lod in 0..3u8 {
                    let p = patch(crop, lod, None, &reg, None);
                    let n = p.pos.len();
                    assert!(n > 0, "{crop:?} lod {lod}: empty");
                    let budget = [210_000, 90_000, 40_000][lod as usize];
                    if n > budget {
                        over.push(format!("{crop:?} lod {lod}: {n}"));
                    }
                    assert!(
                        n <= prev,
                        "{crop:?}: lod {lod} has more vertices ({n}) than lod {}",
                        lod - 1
                    );
                    assert!(p.idx.iter().all(|i| (*i as usize) < n));
                    assert!(
                        p.pos
                            .iter()
                            .flatten()
                            .chain(p.nor.iter().flatten())
                            .all(|v| v.is_finite()),
                        "{crop:?}: NaN"
                    );
                    assert_eq!(p.tan.len(), n);
                    // every plant stands inside the tile
                    assert!(
                        p.uv1.iter().all(|s| s[0] >= -0.01
                            && s[0] <= TILE + 0.01
                            && s[1] >= -0.01
                            && s[1] <= TILE + 0.01),
                        "{crop:?}"
                    );
                    if !reg.is_empty() {
                        eprintln!(
                            "{crop:?} lod {lod}: {n} vertices, {} triangles",
                            p.idx.len() / 3
                        );
                    }
                    prev = n;
                }
                assert_eq!(
                    patch(crop, 0, None, &reg, None).pos,
                    patch(crop, 0, None, &reg, None).pos
                );
            }
        }
        assert!(over.is_empty(), "over budget: {over:?}");
        assert!(
            all_regions().len() > 50,
            "the spec's regions: {}",
            all_regions().len()
        );
    }

    #[test]
    fn tulips_keep_a_six_petal_cup_and_tinted_fallback_at_every_lod() {
        const SEED: u64 = 7_291;
        let (stem_h, yaw, flower_base) = tulip_landmarks(SEED);
        let near = tulip(&mut Rng::new(SEED), 0, 0, &Regions::new());
        let near_height = near.pos.iter().map(|p| p.y - stem_h).fold(0.0, f32::max);
        let near_rim_extent = near
            .pos
            .iter()
            .copied()
            .filter(|p| p.y > stem_h + 0.088)
            .map(|p| (p - flower_base).xz().length())
            .fold(0.0, f32::max);

        for lod in 0..3 {
            let plain = tulip(&mut Rng::new(SEED), lod, 0, &Regions::new());
            assert!(plain.pos.len() < [200, 80, 40][lod as usize]);
            assert!(plain.idx.iter().all(|i| (*i as usize) < plain.pos.len()));
            assert!(
                plain
                    .pos
                    .iter()
                    .chain(plain.nor.iter())
                    .all(|v| v.is_finite())
            );
            // Every azimuth retains an upright, raised tepal, including the far mesh.
            for petal in 0..6 {
                let a = yaw + petal as f32 * TAU / 6.0;
                let direction = Vec3::new(a.cos(), 0.0, a.sin());
                assert!(plain.pos.iter().any(|p| {
                    let offset = *p - flower_base;
                    offset.y > 0.08 && offset.dot(direction) > 0.022
                }));
            }
            let head: Vec<Vec3> = plain
                .pos
                .iter()
                .copied()
                .filter(|p| p.y > stem_h + 0.045)
                .collect();
            let radial = |p: Vec3| (p - flower_base).xz().length();
            let body_radius = head.iter().copied().map(radial).fold(0.0, f32::max);
            let rim: Vec<Vec3> = head
                .iter()
                .copied()
                .filter(|p| p.y > stem_h + 0.088)
                .collect();
            assert!(!rim.is_empty(), "lod {lod}: missing open rim");
            let rim_min = rim
                .iter()
                .copied()
                .map(radial)
                .fold(f32::INFINITY, f32::min);
            let rim_max = rim.iter().copied().map(radial).fold(0.0, f32::max);
            assert!(rim_min > 0.024, "lod {lod}: rim crossed the flower axis");
            assert!(
                rim_max >= body_radius * 0.65,
                "lod {lod}: rim closed too far"
            );
            let cup_height = head.iter().map(|p| p.y - stem_h).fold(0.0, f32::max);
            let cup_aspect = cup_height / (body_radius * 2.0);
            if lod < 2 {
                assert!(
                    (1.2..1.45).contains(&cup_aspect),
                    "lod {lod}: cup proportions are not tulip-like ({cup_aspect:.2})"
                );
            }
            assert!(
                (cup_height - near_height).abs() < 0.001,
                "lod {lod}: lost the near cup height"
            );
            assert!(
                (rim_max - near_rim_extent).abs() < 0.001,
                "lod {lod}: changed the near rim extent"
            );
            if lod == 0 {
                // The final 150 near vertices are six disjoint 5 × 5 tepal grids. Address each
                // top row directly so a neighbouring inner shoulder cannot mask an outer arch.
                let petal_grids = &plain.pos[plain.pos.len() - 6 * 25..];
                for petal_index in 0..6 {
                    let top = &petal_grids[petal_index * 25 + 20..petal_index * 25 + 25];
                    let top_centre = top[2].y;
                    let top_side = top[0].y.max(top[4].y);
                    assert!(
                        top_centre > top_side + 0.015,
                        "petal {petal_index} rim is not visibly rounded"
                    );
                }
            }
            // The recessed brown disc must be real surface area, rather than a degenerate
            // one-latitude blob that leaves a green stem hole in the open cup.
            if lod < 2 {
                assert!(plain.idx.chunks_exact(3).any(|tri| {
                    let a = tri[0] as usize;
                    let b = tri[1] as usize;
                    let c = tri[2] as usize;
                    let brown = [a, b, c].into_iter().all(|i| {
                        let tint = plain.col[i];
                        tint.x > tint.y
                            && tint.y > tint.z
                            && tint.z < tint.x * 0.5
                            && (stem_h + 0.01..stem_h + 0.025).contains(&plain.pos[i].y)
                    });
                    brown
                        && (plain.pos[b] - plain.pos[a])
                            .cross(plain.pos[c] - plain.pos[a])
                            .length()
                            > 1e-7
                }));
            }
            // The cup, leaf folds and tube must supply several real lighting directions.
            assert!(plain.nor.iter().copied().any(|n| n.y.abs() < 0.45));
            assert!(
                plain
                    .nor
                    .iter()
                    .all(|n| (0.98..=1.02).contains(&n.length()))
            );
            for petal in 0..6 {
                let a = yaw + petal as f32 * TAU / 6.0;
                let direction = Vec3::new(a.cos(), 0.0, a.sin());
                assert!(
                    plain
                        .nor
                        .iter()
                        .any(|n| n.y.abs() < 0.8 && n.dot(direction) > 0.55)
                );
            }
            // Plain fallback owns the row colour instead of becoming an uncoloured atlas tint.
            assert!(
                plain
                    .col
                    .iter()
                    .any(|tint| tint.x > tint.y * 8.0 && tint.x > tint.z * 8.0)
            );

            let textured = tulip(&mut Rng::new(SEED), lod, 0, &all_regions());
            assert!(textured.tan.iter().any(|uv| uv[2] == kind::ATLAS));
            assert!(
                textured
                    .col
                    .iter()
                    .zip(&textured.tan)
                    .any(|(tint, uv)| uv[2] == kind::ATLAS && tint.x > tint.y * 8.0)
            );
        }
    }

    #[test]
    fn tulip_part_tags_survive_atlas_fallback_and_patch_scaling() {
        const SEED: u64 = 7_291;
        let parts = [
            tulip_aux::STEM,
            tulip_aux::LEAF,
            tulip_aux::PETAL,
            tulip_aux::CORE,
        ];
        for reg in [Regions::new(), all_regions()] {
            let plant = tulip(&mut Rng::new(SEED), 0, 0, &reg);
            for part in parts {
                assert!(
                    plant.tan.iter().any(|tan| tan[3].floor() == part),
                    "missing tulip part tag {part}"
                );
            }
            if reg.is_empty() {
                assert!(
                    plant
                        .tan
                        .iter()
                        .any(|tan| tan[2] == kind::PLAIN && tan[3].floor() == tulip_aux::PETAL)
                );
            } else {
                assert!(
                    plant
                        .tan
                        .iter()
                        .any(|tan| tan[2] == kind::ATLAS && tan[3].floor() == tulip_aux::PETAL)
                );
            }

            let encoded_stem = plant
                .tan
                .iter()
                .find(|tan| tan[3].floor() == tulip_aux::STEM)
                .unwrap()[3]
                .fract();
            for scale in [0.46, 0.73, 1.19] {
                let mut patch = Patch::default();
                patch.put(&plant, Vec2::new(1.0, 2.0), 0.0, scale, 0.0);
                assert!(
                    patch
                        .tan
                        .iter()
                        .filter(|tan| tulip_aux::is_part(tan[3]))
                        .all(|tan| {
                            // At an aux value around 81, f32's encoded-height precision is about 8 µm.
                            (tan[3].fract() - encoded_stem * scale).abs() < 1e-5
                        })
                );
            }
        }

        // Ordinary terrain and tiled-atlas aux payloads must remain byte-for-byte intact.
        let mut ground = Plant::new(0.1);
        ground.ground(Layer::Dirt);
        ground.vert(Vec3::ZERO, Vec3::Y, Vec3::ONE, 0.0, 0.0, 0.0);
        let ground_aux = ground.tan[0][3];
        let mut ground_patch = Patch::default();
        ground_patch.put(&ground, Vec2::ZERO, 0.0, 0.73, 0.0);
        assert_eq!(ground_patch.tan[0][3], ground_aux);

        let mut tiled_regions = Regions::new();
        tiled_regions.insert(SOIL.to_string(), [0.1, 0.1, 0.2, 0.3]);
        let mut tiled = Plant::new(0.1);
        tiled.soil(&tiled_regions, Layer::Dirt);
        tiled.vert(Vec3::ZERO, Vec3::Y, Vec3::ONE, 0.0, 0.0, 0.0);
        let tiled_aux = tiled.tan[0][3];
        let mut tiled_patch = Patch::default();
        tiled_patch.put(&tiled, Vec2::ZERO, 0.0, 0.73, 0.0);
        assert_eq!(tiled_patch.tan[0][3], tiled_aux);
    }

    /// Exports the actual procedural tulips for Blender clay review. Invoke explicitly with
    /// `SKYFOLD_TULIP_REVIEW_DIR=/absolute/output cargo test ... -- --ignored`; normal test runs
    /// never create review files.
    #[test]
    #[ignore]
    fn export_tulips_for_shape_review() {
        let output = std::env::var_os("SKYFOLD_TULIP_REVIEW_DIR")
            .expect("set SKYFOLD_TULIP_REVIEW_DIR for tulip mesh review");
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).expect("create tulip review directory");
        for seed in [7_291u64, 41] {
            let (stem_h, _, _) = tulip_landmarks(seed);
            for lod in 0..3 {
                // Pink makes the exported silhouette directly comparable with the supplied photos.
                let p = tulip(&mut Rng::new(seed), lod, 2, &Regions::new());
                let doc = serde_json::json!({
                    "pos": p.pos.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
                    "nor": p.nor.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
                    "col": p.col.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
                    "tan": &p.tan,
                    "idx": &p.idx,
                    "height": p.height,
                    "stem_h": stem_h,
                    "seed": seed,
                    "lod": lod,
                });
                let path = output.join(format!("tulip-{seed}-lod{lod}.json"));
                std::fs::write(path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
            }
        }
    }
}
