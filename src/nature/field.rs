//! The painted nature data saved with a world: density maps on an 8 m grid, kept as sparse
//! 512 m chunks (land nobody painted costs nothing and grows exactly what the rules say).
//!
//! Per texel:
//! - a multiplier per category (trees, rocks, plants, debris): `(v / 128)²`, so 128 is "as the
//!   rules say", 0 is none and 255 almost four times as dense (erase / thin / boost);
//! - a painted ecosystem preset (0 = the biome's own, else preset index + 1);
//! - extra density of single species ("more ferns here"), `v / 255 × ADD_MAX` per m².
//!
//! Plus the procedural instances removed by hand (deleted or pinned as placed objects).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::objects::kind_category;
use crate::procgen::meshes::AssetKind;

/// Texel size (m).
pub const TEXEL: f32 = 8.0;
/// Texels per chunk side.
pub const CHUNK: usize = 64;
const N: usize = CHUNK * CHUNK;
/// Neutral multiplier value.
pub const NEUTRAL: u8 = 128;
/// Density (per m²) of a fully painted species, per category.
pub const ADD_MAX: [f32; 4] = [0.035, 0.012, 0.12, 0.012];

#[derive(Clone, Default)]
pub struct Chunk {
    /// multipliers, 4 per texel (category order of `CATS`)
    pub mult: Option<Box<[[u8; 4]; N]>>,
    /// painted preset per texel
    pub preset: Option<Box<[u8; N]>>,
    /// added density per species (`AssetKind as u8`)
    pub adds: Vec<(u8, Box<[u8; N]>)>,
    /// per species multiplier of what the rules grow (255 = all of it), for thinning or erasing
    /// one species
    pub smul: Vec<(u8, Box<[u8; N]>)>,
}

impl Chunk {
    fn is_neutral(&self, def: u8) -> bool {
        self.mult
            .as_ref()
            .is_none_or(|m| m.iter().all(|v| *v == [def; 4]))
            && self
                .preset
                .as_ref()
                .is_none_or(|p| p.iter().all(|v| *v == 0))
            && self.adds.iter().all(|(_, a)| a.iter().all(|v| *v == 0))
            && self.smul.iter().all(|(_, a)| a.iter().all(|v| *v == 255))
    }
    fn smul_mut(&mut self, kind: u8) -> &mut [u8; N] {
        if let Some(i) = self.smul.iter().position(|(k, _)| *k == kind) {
            return &mut self.smul[i].1;
        }
        self.smul.push((kind, Box::new([255; N])));
        &mut self.smul.last_mut().unwrap().1
    }
    fn mult_mut(&mut self, def: u8) -> &mut [[u8; 4]; N] {
        self.mult.get_or_insert_with(|| Box::new([[def; 4]; N]))
    }
    fn add_mut(&mut self, kind: u8) -> &mut [u8; N] {
        if let Some(i) = self.adds.iter().position(|(k, _)| *k == kind) {
            return &mut self.adds[i].1;
        }
        self.adds.push((kind, Box::new([0; N])));
        &mut self.adds.last_mut().unwrap().1
    }
}

/// What the Nature brush paints.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// every category
    All,
    Category(usize),
    /// one or more species
    Species(Vec<AssetKind>),
    /// a whole ecosystem (preset index)
    Preset(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintMode {
    /// more (the target's species, its preset, or its categories up to boosted)
    Grow,
    /// fewer, gradually
    Thin,
    /// none
    Erase,
}

#[derive(Resource, Default, Clone)]
pub struct NatureField {
    pub chunks: HashMap<(i32, i32), Chunk>,
    /// procedural instances removed by hand (deleted, or pinned as placed objects)
    pub excluded: HashSet<u64>,
    /// land without a biome grows nature too (the world was populated)
    pub fallback: bool,
    /// the rules grow nothing where nobody painted (a world converted from objects: its nature
    /// is all painted species until regrown or populated)
    pub rules_off: bool,
    /// bumped on every change
    pub revision: u64,
    /// world-space rects changed since the renderers last looked
    pub dirty: Vec<Rect>,
}

/// Texel coordinates of a world point.
pub fn texel_of(x: f32, z: f32) -> IVec2 {
    IVec2::new((x / TEXEL).floor() as i32, (z / TEXEL).floor() as i32)
}

fn split(t: IVec2) -> ((i32, i32), usize) {
    let c = (t.x.div_euclid(CHUNK as i32), t.y.div_euclid(CHUNK as i32));
    let (i, j) = (
        t.x.rem_euclid(CHUNK as i32) as usize,
        t.y.rem_euclid(CHUNK as i32) as usize,
    );
    (c, j * CHUNK + i)
}

/// One texel's painted values.
#[derive(Clone, Copy, Debug, Default)]
pub struct Texel {
    /// multiplier per category
    pub mult: [f32; 4],
    /// painted preset (0 = none, else index + 1)
    pub preset: u8,
}

impl NatureField {
    /// The multiplier of texels nobody painted.
    pub fn default_mult(&self) -> u8 {
        if self.rules_off { 0 } else { NEUTRAL }
    }

    pub fn texel(&self, t: IVec2) -> Texel {
        let (c, k) = split(t);
        let def = self.default_mult();
        let Some(ch) = self.chunks.get(&c) else {
            return Texel {
                mult: [decode_mult(def); 4],
                preset: 0,
            };
        };
        let m = ch.mult.as_ref().map(|m| m[k]).unwrap_or([def; 4]);
        Texel {
            mult: m.map(decode_mult),
            preset: ch.preset.as_ref().map(|p| p[k]).unwrap_or(0),
        }
    }

    /// Added species densities at a texel: (kind, per m²).
    pub fn adds(&self, t: IVec2, out: &mut Vec<(u8, f32)>) {
        out.clear();
        let (c, k) = split(t);
        let Some(ch) = self.chunks.get(&c) else {
            return;
        };
        for (kind, a) in &ch.adds {
            if a[k] > 0 {
                let cat = super::kind_from_u8(*kind)
                    .and_then(|x| super::rules::cat_index(kind_category(x)))
                    .unwrap_or(2);
                out.push((*kind, a[k] as f32 / 255.0 * ADD_MAX[cat]));
            }
        }
    }

    /// Species multipliers at a texel (kind, 0..1), for species painted thinner.
    pub fn species_mults(&self, t: IVec2, out: &mut Vec<(u8, f32)>) {
        out.clear();
        let (c, k) = split(t);
        let Some(ch) = self.chunks.get(&c) else {
            return;
        };
        for (kind, m) in &ch.smul {
            if m[k] < 255 {
                out.push((*kind, m[k] as f32 / 255.0));
            }
        }
    }

    /// Whether the chunk under a texel carries species additions or multipliers.
    pub fn has_species(&self, t: IVec2) -> bool {
        self.chunks
            .get(&split(t).0)
            .is_some_and(|c| !c.adds.is_empty() || !c.smul.is_empty())
    }

    pub fn touch(&mut self, r: Rect) {
        self.revision += 1;
        self.dirty.push(r);
    }

    /// Chunk keys a circle touches.
    pub fn chunks_in(center: Vec2, radius: f32) -> Vec<(i32, i32)> {
        let a = split(texel_of(center.x - radius, center.y - radius)).0;
        let b = split(texel_of(center.x + radius, center.y + radius)).0;
        let mut out = vec![];
        for z in a.1..=b.1 {
            for x in a.0..=b.0 {
                out.push((x, z));
            }
        }
        out
    }

    /// One brush dab: `amount` 0..1 is how far this dab moves the texels (times the falloff).
    pub fn paint(
        &mut self,
        center: Vec2,
        radius: f32,
        falloff: f32,
        mode: PaintMode,
        target: &Target,
        amount: f32,
    ) {
        self.paint_masked(center, radius, falloff, mode, target, amount, |_| 1.0);
    }

    /// A brush dab that reaches only as far as `mask` (0..1 per spot) lets it (a selection that
    /// masks edits).
    #[allow(clippy::too_many_arguments)]
    pub fn paint_masked(
        &mut self,
        center: Vec2,
        radius: f32,
        falloff: f32,
        mode: PaintMode,
        target: &Target,
        amount: f32,
        mask: impl Fn(Vec2) -> f32,
    ) {
        let inner = 1.0 - falloff;
        let r = radius.max(0.01);
        self.apply(
            center - radius,
            center + radius,
            mode,
            target,
            amount.clamp(0.0, 1.0),
            |p| {
                let d = p.distance(center) / r;
                if d > 1.0 {
                    return 0.0;
                }
                let k = if d <= inner {
                    1.0
                } else {
                    1.0 - crate::noise::smoothstep(inner, 1.0, d)
                };
                if k > 0.0 {
                    k * mask(p).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            },
        );
        self.prune(center, radius);
        self.touch(Rect::from_center_half_size(
            center,
            Vec2::splat(radius + TEXEL),
        ));
    }

    /// Paints an area at once (`inside` decides the texels, `amount` how far they move).
    pub fn paint_area(
        &mut self,
        min: Vec2,
        max: Vec2,
        inside: impl Fn(Vec2) -> bool,
        mode: PaintMode,
        target: &Target,
        amount: f32,
    ) {
        self.apply(min, max, mode, target, amount.clamp(0.0, 1.0), |p| {
            if inside(p) { 1.0 } else { 0.0 }
        });
        self.prune((min + max) * 0.5, (max - min).max_element() * 0.75);
        self.touch(Rect::from_corners(min - TEXEL, max + TEXEL));
    }

    /// Sets the multiplier of the categories of `target` to `m` over an area (agents: "half as
    /// many trees here", "exactly the preset's density").
    pub fn set_area(
        &mut self,
        min: Vec2,
        max: Vec2,
        inside: impl Fn(Vec2) -> bool,
        cats: &[usize],
        m: f32,
    ) {
        let t0 = texel_of(min.x, min.y);
        let t1 = texel_of(max.x, max.y);
        for tz in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let t = IVec2::new(tx, tz);
                if inside((t.as_vec2() + 0.5) * TEXEL) {
                    for c in cats {
                        self.set_mult(t, *c, m);
                    }
                }
            }
        }
        self.prune((min + max) * 0.5, (max - min).max_element() * 0.75);
        self.touch(Rect::from_corners(min - TEXEL, max + TEXEL));
    }

    /// Keeps `keep` (0..1) of what grows over an area: of the target's categories, or of its
    /// species (their rule share and painted additions).
    pub fn scale_area(
        &mut self,
        min: Vec2,
        max: Vec2,
        inside: impl Fn(Vec2) -> bool,
        target: &Target,
        keep: f32,
    ) {
        let def = self.default_mult();
        let t0 = texel_of(min.x, min.y);
        let t1 = texel_of(max.x, max.y);
        let scale = |v: u8, unit: f32| -> u8 {
            let x = v as f32 / unit;
            (x * keep * unit).round().clamp(0.0, 255.0) as u8
        };
        for tz in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let t = IVec2::new(tx, tz);
                if !inside((t.as_vec2() + 0.5) * TEXEL) {
                    continue;
                }
                let (c, k) = split(t);
                let ch = self.chunks.entry(c).or_default();
                match target {
                    Target::Species(ks) => {
                        for kind in ks {
                            let m = &mut ch.smul_mut(*kind as u8)[k];
                            *m = scale(*m, 255.0);
                            if let Some(i) = ch.adds.iter().position(|(x, _)| *x == *kind as u8) {
                                ch.adds[i].1[k] = scale(ch.adds[i].1[k], 255.0);
                            }
                        }
                    }
                    _ => {
                        let cats: Vec<usize> = match target {
                            Target::Category(c) => vec![*c],
                            _ => (0..4).collect(),
                        };
                        for c in &cats {
                            let m = &mut ch.mult_mut(def)[k][*c];
                            // multipliers are stored as √ (see `decode_mult`)
                            *m = encode_mult(decode_mult(*m) * keep);
                        }
                        for (kind, a) in ch.adds.iter_mut() {
                            if super::kind_from_u8(*kind)
                                .and_then(|x| super::rules::cat_index(kind_category(x)))
                                .is_some_and(|kc| cats.contains(&kc))
                            {
                                a[k] = scale(a[k], 255.0);
                            }
                        }
                    }
                }
            }
        }
        self.prune((min + max) * 0.5, (max - min).max_element() * 0.75);
        self.touch(Rect::from_corners(min - TEXEL, max + TEXEL));
    }

    /// Paints the texels of a rect: `shape` is the brush's coverage of a texel (0..1), `amount`
    /// how far a covered texel moves.
    fn apply(
        &mut self,
        min: Vec2,
        max: Vec2,
        mode: PaintMode,
        target: &Target,
        amount: f32,
        shape: impl Fn(Vec2) -> f32,
    ) {
        let def = self.default_mult();
        let t0 = texel_of(min.x, min.y);
        let t1 = texel_of(max.x, max.y);
        let cats: Vec<usize> = match target {
            Target::All | Target::Preset(_) => (0..4).collect(),
            Target::Category(c) => vec![*c],
            Target::Species(ks) => {
                let mut v: Vec<usize> = ks
                    .iter()
                    .filter_map(|k| super::rules::cat_index(kind_category(*k)))
                    .collect();
                v.dedup();
                v
            }
        };
        for tz in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let p = (Vec2::new(tx as f32, tz as f32) + 0.5) * TEXEL;
                let cover = shape(p);
                let w = cover * amount;
                if cover <= 0.0 {
                    continue;
                }
                let (c, k) = split(IVec2::new(tx, tz));
                let ch = self.chunks.entry(c).or_default();
                match (mode, target) {
                    (PaintMode::Grow, Target::Species(ks)) => {
                        for kind in ks {
                            let a = &mut ch.add_mut(*kind as u8)[k];
                            *a = lerp_u8(*a, 255, w);
                            if let Some(i) = ch.smul.iter().position(|(x, _)| *x == *kind as u8) {
                                let m = &mut ch.smul[i].1[k];
                                *m = lerp_u8(*m, 255, w);
                            }
                        }
                        // an erased category comes back for its painted species
                        for c in &cats {
                            let m = &mut ch.mult_mut(def)[k][*c];
                            *m = (*m).max(lerp_u8(*m, NEUTRAL, w));
                        }
                    }
                    (PaintMode::Thin | PaintMode::Erase, Target::Species(ks)) => {
                        let f = if mode == PaintMode::Erase {
                            w
                        } else {
                            w * 0.35
                        };
                        for kind in ks {
                            let m = &mut ch.smul_mut(*kind as u8)[k];
                            *m = lerp_u8(*m, 0, f);
                            if let Some(i) = ch.adds.iter().position(|(x, _)| *x == *kind as u8) {
                                let a = &mut ch.adds[i].1[k];
                                *a = lerp_u8(*a, 0, f);
                            }
                        }
                    }
                    (PaintMode::Grow, Target::Preset(i)) => {
                        // the ecosystem itself is categorical: wherever the brush covers
                        if cover > 0.35 {
                            ch.preset.get_or_insert_with(|| Box::new([0; N]))[k] = i + 1;
                        }
                        // up to its natural density at once, then denser while the brush stays
                        // (up to 4×, like growing a category)
                        for m in ch.mult_mut(def)[k].iter_mut() {
                            *m = if *m < NEUTRAL {
                                (*m).max(lerp_u8(*m, NEUTRAL, w))
                            } else {
                                lerp_u8(*m, 255, w * 0.25)
                            };
                        }
                    }
                    (PaintMode::Grow, _) => {
                        for c in &cats {
                            let m = &mut ch.mult_mut(def)[k][*c];
                            *m = lerp_u8(*m, 255, w * 0.5);
                        }
                    }
                    (PaintMode::Thin | PaintMode::Erase, _) => {
                        let f = if mode == PaintMode::Erase {
                            w
                        } else {
                            w * 0.35
                        };
                        for c in &cats {
                            let m = &mut ch.mult_mut(def)[k][*c];
                            *m = lerp_u8(*m, 0, f);
                        }
                        // painted species of those categories go too
                        for (kind, a) in ch.adds.iter_mut() {
                            if super::kind_from_u8(*kind)
                                .and_then(|x| super::rules::cat_index(kind_category(x)))
                                .is_some_and(|kc| cats.contains(&kc))
                            {
                                a[k] = lerp_u8(a[k], 0, f);
                            }
                        }
                        if matches!(target, Target::Preset(_) | Target::All)
                            && mode == PaintMode::Erase
                            && cover > 0.5
                        {
                            if let Some(p) = ch.preset.as_mut() {
                                p[k] = 0;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Drops chunks that went back to neutral.
    fn prune(&mut self, center: Vec2, radius: f32) {
        let def = self.default_mult();
        for c in Self::chunks_in(center, radius) {
            if self.chunks.get(&c).is_some_and(|ch| ch.is_neutral(def)) {
                self.chunks.remove(&c);
            }
        }
    }

    /// Sets the multiplier of a category at a texel.
    pub fn set_mult(&mut self, t: IVec2, cat: usize, m: f32) {
        let (c, k) = split(t);
        let v = encode_mult(m);
        let def = self.default_mult();
        if v == def && !self.chunks.contains_key(&c) {
            return;
        }
        self.chunks.entry(c).or_default().mult_mut(def)[k][cat] = v;
    }

    pub fn set_add(&mut self, t: IVec2, kind: AssetKind, density: f32) {
        let (c, k) = split(t);
        let cat = super::rules::cat_index(kind_category(kind)).unwrap_or(2);
        let v = (density / ADD_MAX[cat] * 255.0).round().clamp(0.0, 255.0) as u8;
        if v == 0 {
            return;
        }
        self.chunks.entry(c).or_default().add_mut(kind as u8)[k] = v;
    }

    /// Back to what the rules say in the texels of a rect where `inside` holds. Returns whether
    /// anything changed.
    pub fn reset_where(&mut self, min: Vec2, max: Vec2, inside: impl Fn(Vec2) -> bool) -> bool {
        let t0 = texel_of(min.x, min.y);
        let t1 = texel_of(max.x, max.y);
        let def = self.default_mult();
        let mut changed = false;
        for tz in t0.y..=t1.y {
            for tx in t0.x..=t1.x {
                let t = IVec2::new(tx, tz);
                let (c, k) = split(t);
                if !inside((t.as_vec2() + 0.5) * TEXEL) {
                    continue;
                }
                // (where the rules are off, regrowing turns them on)
                if self.rules_off {
                    self.chunks.entry(c).or_default().mult_mut(def);
                }
                let Some(ch) = self.chunks.get_mut(&c) else {
                    continue;
                };
                if let Some(m) = ch.mult.as_mut() {
                    changed |= m[k] != [NEUTRAL; 4];
                    m[k] = [NEUTRAL; 4];
                }
                if let Some(p) = ch.preset.as_mut() {
                    changed |= p[k] != 0;
                    p[k] = 0;
                }
                for (_, a) in ch.adds.iter_mut() {
                    changed |= a[k] != 0;
                    a[k] = 0;
                }
                for (_, a) in ch.smul.iter_mut() {
                    changed |= a[k] != 255;
                    a[k] = 255;
                }
            }
        }
        self.prune((min + max) * 0.5, (max - min).max_element() * 0.75);
        if changed {
            self.touch(Rect::from_corners(min, max));
        }
        changed
    }

    /// Puts the texels of an undo step's chunks back towards how they were before it, by
    /// `weight` (0 = all the way back, 1 = keep the change; a selection masking edits). The
    /// painted ecosystem is not blendable: it stays where `keep_preset` says. Returns the step's
    /// new after-state when anything changed.
    pub fn settle_outside(
        &mut self,
        before: &NatureUndo,
        weight: impl Fn(Vec2) -> f32,
        keep_preset: impl Fn(Vec2, f32) -> bool,
    ) -> Option<NatureUndo> {
        if before.whole {
            return None;
        }
        let def = self.default_mult();
        let mut changed = false;
        for (key, b) in &before.chunks {
            let o = Vec2::new(key.0 as f32, key.1 as f32) * CHUNK as f32;
            let outside: Vec<(usize, f32, bool)> = (0..N)
                .filter_map(|k| {
                    let p = (o + Vec2::new((k % CHUNK) as f32, (k / CHUNK) as f32) + 0.5) * TEXEL;
                    let m = weight(p).clamp(0.0, 1.0);
                    (m < 1.0).then(|| (k, m, keep_preset(p, m)))
                })
                .collect();
            if outside.is_empty() {
                continue;
            }
            let empty = Chunk::default();
            let b = b.as_ref().unwrap_or(&empty);
            let Some(cur) = self.chunks.get_mut(key) else {
                // nothing painted here now: only a chunk that had paint before needs it back
                if b.is_neutral(def) {
                    continue;
                }
                let mut c = Chunk::default();
                restore_texels(&mut c, b, &outside, def);
                self.chunks.insert(*key, c);
                changed = true;
                continue;
            };
            changed |= restore_texels(cur, b, &outside, def);
        }
        // instances removed by hand outside come back
        let mut ex_changed = false;
        if let Some(ex) = &before.excluded {
            let back: Vec<u64> = self
                .excluded
                .iter()
                .filter(|id| {
                    !ex.contains(id)
                        && weight(
                            (super::place::cell_of_id(**id).as_vec2() + 0.5) * super::place::CELL,
                        ) < 0.5
                })
                .copied()
                .collect();
            for id in back {
                self.excluded.remove(&id);
                ex_changed = true;
            }
        }
        if !changed && !ex_changed {
            return None;
        }
        let keys: Vec<(i32, i32)> = before.chunks.iter().map(|(k, _)| *k).collect();
        for k in &keys {
            if self.chunks.get(k).is_some_and(|c| c.is_neutral(def)) {
                self.chunks.remove(k);
            }
            let o = Vec2::new(k.0 as f32, k.1 as f32) * CHUNK as f32 * TEXEL;
            self.dirty
                .push(Rect::from_corners(o, o + Vec2::splat(CHUNK as f32 * TEXEL)));
        }
        if ex_changed {
            self.dirty
                .push(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
        }
        self.revision += 1;
        let mut after = self.snapshot(&keys);
        after.excluded = before.excluded.as_ref().map(|_| self.excluded.clone());
        Some(after)
    }

    /// Sets every texel of a rect at once (in parallel): `f` gives a texel's per-category
    /// multipliers and painted preset (0 = the biome's own, else preset index + 1), or `None` to
    /// leave it as it is.
    pub fn set_texels(
        &mut self,
        min: Vec2,
        max: Vec2,
        f: impl Fn(Vec2) -> Option<([f32; 4], u8)> + Sync,
    ) {
        use rayon::prelude::*;
        let def = self.default_mult();
        let c0 = split(texel_of(min.x, min.y)).0;
        let c1 = split(texel_of(max.x, max.y)).0;
        let keys: Vec<(i32, i32)> = (c0.1..=c1.1)
            .flat_map(|z| (c0.0..=c1.0).map(move |x| (x, z)))
            .collect();
        let old = std::mem::take(&mut self.chunks);
        let built: Vec<((i32, i32), Option<Chunk>)> = keys
            .par_iter()
            .map(|key| {
                let mut ch = old.get(key).cloned().unwrap_or_default();
                let o = Vec2::new(key.0 as f32, key.1 as f32) * CHUNK as f32;
                for k in 0..N {
                    let p = (o + Vec2::new((k % CHUNK) as f32, (k / CHUNK) as f32) + 0.5) * TEXEL;
                    if p.cmplt(min).any() || p.cmpgt(max).any() {
                        continue;
                    }
                    let Some((m, preset)) = f(p) else { continue };
                    let m = m.map(encode_mult);
                    if m != [def; 4] || ch.mult.is_some() {
                        ch.mult_mut(def)[k] = m;
                    }
                    if preset != 0 || ch.preset.is_some() {
                        ch.preset.get_or_insert_with(|| Box::new([0; N]))[k] = preset;
                    }
                }
                (*key, (!ch.is_neutral(def)).then_some(ch))
            })
            .collect();
        self.chunks = old;
        for (k, c) in built {
            match c {
                Some(c) => {
                    self.chunks.insert(k, c);
                }
                None => {
                    self.chunks.remove(&k);
                }
            }
        }
        self.touch(Rect::from_corners(min, max));
    }

    /// Snapshot of some chunks (for undo).
    pub fn snapshot(&self, keys: &[(i32, i32)]) -> NatureUndo {
        NatureUndo {
            chunks: keys
                .iter()
                .map(|k| (*k, self.chunks.get(k).cloned()))
                .collect(),
            ..default()
        }
    }

    /// Restores an undo snapshot.
    pub fn restore(&mut self, u: &NatureUndo) {
        if u.whole {
            self.chunks.clear();
            self.dirty
                .push(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
        }
        if let Some(f) = u.fallback {
            self.fallback = f;
        }
        if let Some(r) = u.rules_off {
            self.rules_off = r;
        }
        for (k, c) in &u.chunks {
            match c {
                Some(c) => {
                    self.chunks.insert(*k, c.clone());
                }
                None => {
                    self.chunks.remove(k);
                }
            }
            let o = Vec2::new(k.0 as f32, k.1 as f32) * CHUNK as f32 * TEXEL;
            self.dirty
                .push(Rect::from_corners(o, o + Vec2::splat(CHUNK as f32 * TEXEL)));
        }
        if let Some(ex) = &u.excluded {
            self.excluded = ex.clone();
            self.dirty
                .push(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
        }
        self.revision += 1;
    }

    /// Painted 512 m chunks (for the world summary).
    pub fn painted_chunks(&self) -> usize {
        self.chunks.len()
    }

    // ---------------------------------------------------------------- file

    /// `nature.bin`: magic, version, flags, chunks (each layer present or not), exclusions.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(64 + self.chunks.len() * N * 5 + self.excluded.len() * 8);
        b.extend_from_slice(b"WBNF");
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&(self.fallback as u32 | (self.rules_off as u32) << 1).to_le_bytes());
        let mut keys: Vec<&(i32, i32)> = self.chunks.keys().collect();
        keys.sort();
        b.extend_from_slice(&(keys.len() as u32).to_le_bytes());
        for k in keys {
            let c = &self.chunks[k];
            b.extend_from_slice(&k.0.to_le_bytes());
            b.extend_from_slice(&k.1.to_le_bytes());
            b.push(c.mult.is_some() as u8);
            if let Some(m) = &c.mult {
                for v in m.iter() {
                    b.extend_from_slice(v);
                }
            }
            b.push(c.preset.is_some() as u8);
            if let Some(p) = &c.preset {
                b.extend_from_slice(&p[..]);
            }
            for list in [&c.adds, &c.smul] {
                b.push(list.len() as u8);
                for (kind, a) in list {
                    b.push(*kind);
                    b.extend_from_slice(&a[..]);
                }
            }
        }
        let mut ex: Vec<&u64> = self.excluded.iter().collect();
        ex.sort();
        b.extend_from_slice(&(ex.len() as u32).to_le_bytes());
        for e in ex {
            b.extend_from_slice(&e.to_le_bytes());
        }
        b
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Reader { b: bytes, at: 0 };
        if r.take(4)? != b"WBNF" {
            return Err("not a nature file".into());
        }
        let version = r.u32()?;
        if version != 1 {
            return Err(format!(
                "nature file version {version} is newer than this editor"
            ));
        }
        let flags = r.u32()?;
        let mut f = NatureField {
            fallback: flags & 1 != 0,
            rules_off: flags & 2 != 0,
            ..default()
        };
        let n = r.u32()?;
        for _ in 0..n {
            let key = (r.u32()? as i32, r.u32()? as i32);
            let mut c = Chunk::default();
            if r.take(1)?[0] != 0 {
                let raw = r.take(N * 4)?;
                let mut m = Box::new([[NEUTRAL; 4]; N]);
                for (i, v) in m.iter_mut().enumerate() {
                    v.copy_from_slice(&raw[i * 4..i * 4 + 4]);
                }
                c.mult = Some(m);
            }
            if r.take(1)?[0] != 0 {
                let mut p = Box::new([0u8; N]);
                p.copy_from_slice(r.take(N)?);
                c.preset = Some(p);
            }
            for list in [&mut c.adds, &mut c.smul] {
                let n = r.take(1)?[0];
                for _ in 0..n {
                    let kind = r.take(1)?[0];
                    let mut a = Box::new([0u8; N]);
                    a.copy_from_slice(r.take(N)?);
                    list.push((kind, a));
                }
            }
            f.chunks.insert(key, c);
        }
        let ex = r.u32()?;
        for _ in 0..ex {
            let lo = r.u32()? as u64;
            let hi = r.u32()? as u64;
            f.excluded.insert(lo | hi << 32);
        }
        Ok(f)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self
            .b
            .get(self.at..self.at + n)
            .ok_or("nature file is truncated")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// Moves the texels `ks` (index, weight, keep the preset) of `to` back towards `from`: each
/// value to `from + (to − from) × weight`. Returns whether any value changed.
fn restore_texels(to: &mut Chunk, from: &Chunk, ks: &[(usize, f32, bool)], def: u8) -> bool {
    let mut changed = false;
    let mut blend = |dst: &mut u8, src: u8, m: f32| {
        let v = (src as f32 + (*dst as f32 - src as f32) * m).round() as u8;
        changed |= *dst != v;
        *dst = v;
    };
    // multipliers
    if to.mult.is_some() || from.mult.is_some() {
        let src = from.mult.as_deref();
        let dst = to.mult_mut(def);
        for &(k, m, _) in ks {
            let v = src.map(|x| x[k]).unwrap_or([def; 4]);
            for c in 0..4 {
                blend(&mut dst[k][c], v[c], m);
            }
        }
    }
    // species additions and multipliers
    let mut kinds: Vec<u8> = to.adds.iter().chain(&from.adds).map(|(k, _)| *k).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for kind in kinds {
        let src = from
            .adds
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, a)| &**a);
        let dst = to.add_mut(kind);
        for &(k, m, _) in ks {
            blend(&mut dst[k], src.map(|a| a[k]).unwrap_or(0), m);
        }
    }
    let mut kinds: Vec<u8> = to.smul.iter().chain(&from.smul).map(|(k, _)| *k).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for kind in kinds {
        let src = from
            .smul
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, a)| &**a);
        let dst = to.smul_mut(kind);
        for &(k, m, _) in ks {
            blend(&mut dst[k], src.map(|a| a[k]).unwrap_or(255), m);
        }
    }
    // the painted ecosystem: one side or the other
    if to.preset.is_some() || from.preset.is_some() {
        let src = from.preset.as_deref();
        let dst = to.preset.get_or_insert_with(|| Box::new([0; N]));
        for &(k, _, keep) in ks {
            let v = src.map(|p| p[k]).unwrap_or(0);
            if !keep && dst[k] != v {
                dst[k] = v;
                changed = true;
            }
        }
    }
    changed
}

pub fn decode_mult(v: u8) -> f32 {
    let x = v as f32 / NEUTRAL as f32;
    x * x
}

pub fn encode_mult(m: f32) -> u8 {
    (m.max(0.0).sqrt() * NEUTRAL as f32)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let v = a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0);
    // always move at least one step towards the target
    let v = if (v.round() - a as f32).abs() < 1.0 && a != b && t > 0.01 {
        a as f32 + (b as f32 - a as f32).signum()
    } else {
        v
    };
    v.round().clamp(0.0, 255.0) as u8
}

/// An undo step of the nature paint (chunks before or after) — see `History::push_ext`.
#[derive(Clone, Default)]
pub struct NatureUndo {
    pub chunks: Vec<((i32, i32), Option<Chunk>)>,
    /// `chunks` is every painted chunk (others are dropped on restore)
    pub whole: bool,
    /// the whole set of hand removals, when it changed
    pub excluded: Option<HashSet<u64>>,
    pub fallback: Option<bool>,
    pub rules_off: Option<bool>,
    /// World amounts (trees, rocks, plants, debris, flowers, mushrooms, berries).
    pub amounts: Option<[f32; 7]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_roundtrips_through_the_file() {
        let mut f = NatureField {
            fallback: true,
            ..default()
        };
        f.paint(
            Vec2::new(100.0, -40.0),
            60.0,
            0.3,
            PaintMode::Grow,
            &Target::Species(vec![AssetKind::Fern, AssetKind::Pine]),
            0.6,
        );
        f.paint(
            Vec2::new(-700.0, 300.0),
            30.0,
            0.0,
            PaintMode::Erase,
            &Target::Category(0),
            1.0,
        );
        f.paint(
            Vec2::new(-650.0, 300.0),
            30.0,
            0.0,
            PaintMode::Grow,
            &Target::Preset(3),
            1.0,
        );
        f.scale_area(
            Vec2::new(0.0, 0.0),
            Vec2::new(64.0, 64.0),
            |_| true,
            &Target::Species(vec![AssetKind::Oak]),
            0.5,
        );
        f.excluded.insert(PROC | 12345);
        let g = NatureField::decode(&f.encode()).unwrap();
        assert_eq!(g.fallback, f.fallback);
        assert_eq!(g.excluded, f.excluded);
        assert_eq!(g.chunks.len(), f.chunks.len());
        for t in [
            IVec2::new(12, -5),
            texel_of(-700.0, 300.0),
            texel_of(-650.0, 300.0),
            IVec2::new(3, 3),
        ] {
            let (a, b) = (f.texel(t), g.texel(t));
            assert_eq!((a.mult, a.preset), (b.mult, b.preset));
            let (mut x, mut y) = (vec![], vec![]);
            f.adds(t, &mut x);
            g.adds(t, &mut y);
            assert_eq!(x, y);
            f.species_mults(t, &mut x);
            g.species_mults(t, &mut y);
            assert_eq!(x, y);
        }
        // erased trees, painted ecosystem, painted ferns
        assert_eq!(f.texel(texel_of(-700.0, 300.0)).mult[0], 0.0);
        assert_eq!(f.texel(texel_of(-650.0, 300.0)).preset, 4);
        let mut adds = vec![];
        f.adds(texel_of(100.0, -40.0), &mut adds);
        assert!(
            adds.iter()
                .any(|(k, d)| *k == AssetKind::Fern as u8 && *d > 0.0)
        );
    }

    #[test]
    fn undo_snapshots_restore_and_neutral_chunks_go() {
        let mut f = NatureField::default();
        let keys = NatureField::chunks_in(Vec2::ZERO, 40.0);
        let before = f.snapshot(&keys);
        f.paint(Vec2::ZERO, 40.0, 0.2, PaintMode::Thin, &Target::All, 1.0);
        assert!(!f.chunks.is_empty());
        f.restore(&before);
        assert!(f.chunks.is_empty());
        // painting back to the rules' values drops the chunk again
        f.set_mult(IVec2::new(1, 1), 2, 0.25);
        assert_eq!(f.chunks.len(), 1);
        f.reset_where(Vec2::splat(-100.0), Vec2::splat(100.0), |_| true);
        assert!(f.chunks.is_empty());
    }

    #[test]
    fn multipliers_encode_both_ways() {
        for m in [0.0, 0.25, 1.0, 2.0, 3.9] {
            assert!(
                (decode_mult(encode_mult(m)) - m).abs() < 0.05 * m.max(0.2),
                "{m}"
            );
        }
        assert_eq!(encode_mult(1.0), NEUTRAL);
    }

    /// A selection masking edits: an area paint reaching past it goes back outside, fades across
    /// the feather and stays inside; redo gets the masked state.
    #[test]
    fn masked_paint_settles_outside() {
        let mut f = NatureField::default();
        let keys = NatureField::chunks_in(Vec2::ZERO, 300.0);
        let before = f.snapshot(&keys);
        f.scale_area(
            Vec2::splat(-300.0),
            Vec2::splat(300.0),
            |_| true,
            &Target::Category(0),
            0.0,
        );
        // inside x < 0, a feather 0 < x < 80, outside beyond
        let w = |p: Vec2| 1.0 - (p.x / 80.0).clamp(0.0, 1.0);
        let after = f
            .settle_outside(&before, w, |_, m| m > 0.5)
            .expect("changed");
        assert_eq!(f.texel(texel_of(-100.0, 0.0)).mult[0], 0.0);
        assert_eq!(f.texel(texel_of(200.0, 0.0)).mult[0], 1.0);
        let mid = f.texel(texel_of(40.0, 0.0)).mult[0];
        assert!(mid > 0.1 && mid < 0.9, "{mid}");
        let mut g = NatureField::default();
        g.restore(&after);
        assert_eq!(g.texel(texel_of(200.0, 0.0)).mult[0], 1.0);
        assert_eq!(g.texel(texel_of(-100.0, 0.0)).mult[0], 0.0);
        // a whole-world texel fill
        let mut h = NatureField::default();
        h.set_texels(Vec2::splat(-100.0), Vec2::splat(100.0), |p| {
            (p.x > 0.0).then_some(([2.0, 1.0, 1.0, 0.5], 0))
        });
        assert!((h.texel(texel_of(50.0, 0.0)).mult[0] - 2.0).abs() < 0.05);
        assert_eq!(h.texel(texel_of(-50.0, 0.0)).mult[0], 1.0);
    }

    const PROC: u64 = super::super::place::PROC_BIT;
}
