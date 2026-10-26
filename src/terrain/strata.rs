//! Rock strata: heights quantised into stepped layers, the way flat-lying sandstone and shale
//! weather into ledges and benches. Hard beds stand as short cliffs (risers), soft beds wear back
//! into gentle treads; the bed boundaries wander a little in height and the ledge edges wobble
//! sideways, so the result reads as eroded rock rather than stairs.
//!
//! `strata_height` is the pure height function (used by the Strata brush and the canyonlands
//! generator); `strata` is the brush.

use bevy::prelude::*;

use super::Terrain;
use super::brush::{BrushArgs, for_each_in_circle};
use crate::history::History;
use crate::noise::{fbm, hash_u32, smoothstep, value2};

/// How the beds weather. Shared by the brush (`Editor::strata`), MCP and the generator.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StrataParams {
    /// Average thickness of one bed: the height of a step (m).
    pub step: f32,
    /// 0 soft, rounded steps (shale slopes) … 1 crisp cliffs with flat benches.
    pub sharpness: f32,
    /// 0 regular layers … 1 beds of very different thickness, wandering ledge edges, rubble.
    pub irregularity: f32,
}

impl Default for StrataParams {
    fn default() -> Self {
        Self {
            step: 14.0,
            sharpness: 0.7,
            irregularity: 0.5,
        }
    }
}

impl StrataParams {
    pub fn clamped(self) -> Self {
        Self {
            step: self.step.clamp(1.0, 120.0),
            sharpness: self.sharpness.clamp(0.0, 1.0),
            irregularity: self.irregularity.clamp(0.0, 1.0),
        }
    }
}

/// Where bed `k`'s base lies, in steps (bed thicknesses vary with `irr`).
#[inline]
fn bed_base(k: i32, irr: f32, seed: u32) -> f32 {
    let r = (hash_u32((k as u32).wrapping_mul(0x9e37_79b9) ^ seed) & 0xffff) as f32 / 65535.0;
    k as f32 + (r - 0.5) * 0.6 * irr
}

/// How hard bed `k` is (0..1): hard beds keep short steep risers, soft ones wear into slopes.
#[inline]
fn bed_hardness(k: i32, seed: u32) -> f32 {
    (hash_u32((k as u32).wrapping_mul(0x85eb_ca6b) ^ seed ^ 0x51ed) & 0xffff) as f32 / 65535.0
}

/// The stratified height for ground at `h` (m) at world position `p`. Within ± one step of `h`;
/// applying it again only flattens the benches further (the brush converges, never runs away).
pub fn strata_height(h: f32, p: Vec2, sp: &StrataParams, seed: u32) -> f32 {
    let sp = sp.clamped();
    let (step, irr) = (sp.step, sp.irregularity);
    // the beds dip and undulate gently (a fraction of a step over a few hundred metres)
    let dip = fbm(p.x / 420.0, p.y / 420.0, 3, 2.0, 0.5, seed ^ 0xd1b5) * 0.45 * irr;
    let u = h / step + dip;
    // which bed: boundaries jitter in height, so search the neighbours
    let mut k = u.floor() as i32;
    if u < bed_base(k, irr, seed) {
        k -= 1;
    } else if u >= bed_base(k + 1, irr, seed) {
        k += 1;
    }
    let (b0, b1) = (bed_base(k, irr, seed), bed_base(k + 1, irr, seed));
    let f = ((u - b0) / (b1 - b0).max(1e-3)).clamp(0.0, 1.0);
    // the riser (cliff) takes the top part of each bed: narrow and crisp when sharp, wide and
    // soft for soft beds; its foot wobbles sideways (eroded, notched ledge edges)
    let hard = bed_hardness(k, seed);
    let riser = (0.62 - 0.5 * sp.sharpness) * (1.25 - 0.6 * hard * irr.max(0.3));
    let wobble = (value2(p.x / 23.0, p.y / 23.0, seed ^ 0x2c1b) - 0.5) * 0.35 * irr
        + (value2(p.x / 7.0, p.y / 7.0, seed ^ 0x77) - 0.5) * 0.12 * irr;
    let a = (1.0 - riser + wobble).clamp(0.2, 0.96);
    // benches keep a little of the original slope (never perfectly flat)
    let keep = 0.05 + 0.3 * (1.0 - sp.sharpness);
    let s = keep * f + (1.0 - keep) * smoothstep(a, 1.0, f);
    let q = b0 + s * (b1 - b0);
    // a little rubble and bedding roughness on the benches
    let rubble = fbm(p.x / 9.0, p.y / 9.0, 2, 2.0, 0.5, seed ^ 0x3a7) * 0.05 * irr;
    (q - dip + rubble) * step
}

/// Strata brush: work the ground under the brush towards stepped rock layers.
pub fn strata(t: &mut Terrain, h: &mut History, a: &BrushArgs, sp: &StrataParams, seed: u32) {
    let k = (a.strength * a.dt * 4.0).min(1.0);
    let origin = t.sea_level;
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, idx, p, w| {
        let hh = t.heights[idx];
        let target = origin + strata_height(hh - origin, p, sp, seed);
        t.heights[idx] += (target - hh) * w * k;
    }) {
        t.mark_heights_dirty(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_within_a_step() {
        let sp = StrataParams::default();
        for i in 0..2000 {
            let h = i as f32 * 0.37 - 100.0;
            let p = Vec2::new(i as f32 * 3.1, i as f32 * -1.7);
            let s = strata_height(h, p, &sp, 7);
            assert!((s - h).abs() <= sp.step * 1.2, "{h} -> {s}");
        }
    }

    #[test]
    fn a_ramp_becomes_benches_and_risers() {
        // a steady 1:1 ramp: most of it turns into flattish benches, the rest into steep risers
        let sp = StrataParams {
            step: 10.0,
            sharpness: 0.8,
            irregularity: 0.3,
        };
        let (mut flat, mut steep, n) = (0, 0, 400);
        for i in 0..n {
            let x = i as f32 * 0.5;
            let p0 = Vec2::new(x, 40.0);
            let p1 = Vec2::new(x + 0.5, 40.0);
            let d = strata_height(x + 0.5, p1, &sp, 3) - strata_height(x, p0, &sp, 3);
            let slope = d / 0.5;
            if slope < 0.45 {
                flat += 1;
            } else if slope > 1.6 {
                steep += 1;
            }
        }
        assert!(flat > n / 2, "benches: {flat}/{n}");
        assert!(steep > n / 10, "risers: {steep}/{n}");
    }

    #[test]
    fn monotonic_in_height() {
        // higher ground never ends up lower than lower ground at the same spot
        let sp = StrataParams::default();
        let p = Vec2::new(123.0, -45.0);
        let mut prev = f32::MIN;
        for i in 0..1000 {
            let s = strata_height(i as f32 * 0.2, p, &sp, 11);
            assert!(s >= prev - 1e-3);
            prev = s;
        }
    }
}
