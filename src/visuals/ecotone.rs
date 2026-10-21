//! Ecotones: natural borders between biomes.
//!
//! * A drawn outline (biome outline tool, `terrain_set_biome` over MCP) becomes a wandering border
//!   at every scale: [`OrganicOutline`] displaces the outline's signed distance by fractal noise
//!   (tens of metres over hundreds, metres over tens) before it decides the biome map and how the
//!   ground feathers in.
//! * Near a border a spot belongs partly to both sides: [`biome_weights`] reads the biome map
//!   through a wandering warp at a ring of points around the spot, so each side's claim fades over
//!   a band ~60 m wide. [`vegetation_density`] turns a claim into a density multiplier for a
//!   biome's plants (they thin out and mix with the neighbour's across the band); the grass styles
//!   blend with the same weights. The terrain shader interleaves the two grounds in that band.

use bevy::prelude::*;

use crate::biomes::{distance_to_polyline, point_in_polygon};
use crate::noise::{fbm, smoothstep};
use crate::terrain::Terrain;

/// Radius (m) of the ring the biome claim is averaged over: the half width of an ecotone.
const ECOTONE: f32 = 30.0;

/// Fractal displacement (m) of a border at `p`, about ±`amp` (three octaves: ~500 m, ~120 m, ~30 m
/// across).
pub fn border_offset(p: Vec2, amp: f32, seed: u32) -> f32 {
    let a = fbm(
        p.x * 0.002,
        p.y * 0.002,
        2,
        2.0,
        0.5,
        seed.wrapping_add(9101),
    );
    let b = fbm(
        p.x * 0.0085,
        p.y * 0.0085,
        2,
        2.0,
        0.5,
        seed.wrapping_add(9103),
    );
    let c = fbm(
        p.x * 0.034,
        p.y * 0.034,
        2,
        2.0,
        0.5,
        seed.wrapping_add(9107),
    );
    (a * 1.25 + b * 0.45 + c * 0.14) * amp * 1.6
}

/// A drawn closed outline with a natural, wandering edge. Its signed distance is sampled on a
/// coarse grid (bilinear in between), so even long outlines over large areas stay fast.
pub struct OrganicOutline {
    lo: Vec2,
    spacing: f32,
    w: usize,
    h: usize,
    /// signed distance to the drawn outline (+ inside), per grid node
    sd: Vec<f32>,
    amp: f32,
    seed: u32,
}

impl OrganicOutline {
    /// How far (m) the border may wander from the drawn line, and the feather either side.
    pub const FEATHER: f32 = 14.0;

    pub fn new(poly: &[Vec2], seed: u32) -> Self {
        let (lo, hi) = poly
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
        // small shapes wander less: a 100 m pond stays recognisably what was drawn
        let amp = (0.06 * (hi - lo).min_element()).clamp(2.0, 45.0);
        let margin = amp * 2.2 + Self::FEATHER + 8.0;
        let (lo, hi) = (lo - Vec2::splat(margin), hi + Vec2::splat(margin));
        let spacing = ((hi - lo).max_element() / 700.0).max(4.0);
        let w = ((hi.x - lo.x) / spacing).ceil() as usize + 2;
        let h = ((hi.y - lo.y) / spacing).ceil() as usize + 2;
        let rows: Vec<Vec<f32>> = std::thread::scope(|s| {
            let chunk = h.div_ceil(10).max(1);
            let handles: Vec<_> = (0..h)
                .collect::<Vec<_>>()
                .chunks(chunk)
                .map(|js| {
                    let js = js.to_vec();
                    s.spawn(move || {
                        js.iter()
                            .map(|&j| {
                                (0..w)
                                    .map(|i| {
                                        let p = lo + Vec2::new(i as f32, j as f32) * spacing;
                                        let d = distance_to_polyline(p, poly);
                                        if point_in_polygon(p, poly) { d } else { -d }
                                    })
                                    .collect::<Vec<f32>>()
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        Self {
            lo,
            spacing,
            w,
            h,
            sd: rows.into_iter().flatten().collect(),
            amp,
            seed,
        }
    }

    /// The area the wandering border can reach.
    pub fn bounds(&self) -> (Vec2, Vec2) {
        (
            self.lo,
            self.lo + Vec2::new(self.w as f32 - 1.0, self.h as f32 - 1.0) * self.spacing,
        )
    }

    /// Signed depth (m) inside the wandering border (negative outside).
    pub fn depth(&self, p: Vec2) -> f32 {
        let g = ((p - self.lo) / self.spacing).clamp(
            Vec2::ZERO,
            Vec2::new(self.w as f32 - 1.001, self.h as f32 - 1.001),
        );
        let (i, j) = (g.x as usize, g.y as usize);
        let f = g - Vec2::new(i as f32, j as f32);
        let at = |a: usize, b: usize| self.sd[b * self.w + a];
        let top = at(i, j) + (at(i + 1, j) - at(i, j)) * f.x;
        let bot = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * f.x;
        top + (bot - top) * f.y + border_offset(p, self.amp, self.seed)
    }

    pub fn inside(&self, p: Vec2) -> bool {
        self.depth(p) > 0.0
    }

    /// How strongly the biome's ground is painted at `p` (0 outside .. 1 well inside).
    pub fn ground_weight(&self, p: Vec2) -> f32 {
        smoothstep(-Self::FEATHER * 0.35, Self::FEATHER, self.depth(p))
    }
}

/// How much each biome claims spot `p`: up to four (biome id, weight) pairs, weights summing to 1
/// (id 0 = no biome). Inside a biome that is just (its id, 1); across a border the claim fades
/// over ~2 × `ECOTONE` m, along a border that wanders on its own (independent of the biome map's
/// cells).
pub fn biome_weights(t: &Terrain, p: Vec2, seed: u32) -> [(u8, f32); 4] {
    let here = t.biome_at(p.x, p.y);
    let mut out = [(here, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)];
    // fast path: nothing different anywhere near
    let r = ECOTONE * 1.6;
    if [
        Vec2::new(r, 0.0),
        Vec2::new(-r, 0.0),
        Vec2::new(0.0, r),
        Vec2::new(0.0, -r),
    ]
    .iter()
    .all(|o| t.biome_at(p.x + o.x, p.y + o.y) == here)
    {
        return out;
    }
    let warp = Vec2::new(
        fbm(
            p.x * 0.006,
            p.y * 0.006,
            2,
            2.0,
            0.5,
            seed.wrapping_add(9201),
        ),
        fbm(
            p.x * 0.006,
            p.y * 0.006,
            2,
            2.0,
            0.5,
            seed.wrapping_add(9203),
        ),
    ) * ECOTONE
        * 1.2;
    let c = p + warp;
    let mut acc: [(u8, f32); 9] = [(0, 0.0); 9];
    let mut used = 0;
    let mut add = |id: u8, w: f32| {
        if let Some(e) = acc[..used].iter_mut().find(|e| e.0 == id) {
            e.1 += w;
        } else if used < acc.len() {
            acc[used] = (id, w);
            used += 1;
        }
    };
    add(t.biome_at(c.x, c.y), 2.0);
    let turn = crate::noise::value2(p.x * 0.05, p.y * 0.05, seed.wrapping_add(9209))
        * std::f32::consts::TAU;
    for k in 0..8 {
        let a = turn + k as f32 * std::f32::consts::FRAC_PI_4;
        let q = c + Vec2::new(a.cos(), a.sin()) * ECOTONE;
        add(t.biome_at(q.x, q.y), 1.0);
    }
    let acc = &mut acc[..used];
    acc.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f32 = acc.iter().take(4).map(|e| e.1).sum();
    for (k, e) in acc.iter().take(4).enumerate() {
        out[k] = (e.0, e.1 / total);
    }
    out
}

/// Breaks a gradual fade (0..1, e.g. grass density across a border) into clumps and tongues: a
/// value near 0 or 1 stays put, one halfway is pushed up or down by wandering noise (~11 m and
/// ~3 m), so the grass of a meadow thins out into a dry neighbour in patches, not a smooth ramp.
pub fn patchy(v: f32, p: Vec2) -> f32 {
    if v <= 0.02 || v >= 0.98 {
        return v;
    }
    let n = crate::noise::value2(p.x * 0.09, p.y * 0.09, 9301) * 0.65
        + crate::noise::value2(p.x * 0.33, p.y * 0.33, 9311) * 0.35
        - 0.5;
    let contested = (v * (1.0 - v) * 4.0).min(1.0);
    (v + n * 1.4 * contested * contested).clamp(0.0, 1.0)
}

/// Density multiplier (0..1) for the vegetation of `biome` at `p`: 1 well inside it, fading across
/// its borders, where the plants of both sides thin out a little and intermingle.
pub fn vegetation_density(t: &Terrain, p: Vec2, biome: u8, seed: u32) -> f32 {
    let w = biome_weights(t, p, seed)
        .iter()
        .find(|e| e.0 == biome)
        .map_or(0.0, |e| e.1);
    w * (0.65 + 0.35 * w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square drawn with ruler-straight edges gets a wandering border: well inside and well
    /// outside stay put, but walking along an edge the border keeps crossing the drawn line.
    #[test]
    fn outlines_wander() {
        let poly = [
            Vec2::new(-1000.0, -1000.0),
            Vec2::new(1000.0, -1000.0),
            Vec2::new(1000.0, 1000.0),
            Vec2::new(-1000.0, 1000.0),
        ];
        let shape = OrganicOutline::new(&poly, 7);
        assert!(shape.inside(Vec2::ZERO));
        assert!(!shape.inside(Vec2::new(1300.0, 0.0)));
        let along: Vec<bool> = (0..200)
            .map(|k| shape.inside(Vec2::new(-900.0 + k as f32 * 9.0, 1000.0)))
            .collect();
        let flips = along.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(
            flips >= 2,
            "the border follows the drawn line ({flips} crossings)"
        );
        assert!((0.0..=1.0).contains(&shape.ground_weight(Vec2::new(0.0, 990.0))));
    }
}
