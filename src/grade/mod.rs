//! The look's colour grade after tonemapping, written once and baked into a 3D lookup table (LUT).
//!
//! The editor's style pass (`look_post`) samples the baked LUT instead of doing the maths per
//! pixel, and the exports (Godot, Unity, Unreal) bake the same function into their own LUT
//! strips, so the editor and every engine grade with one implementation.
//!
//! Two domains:
//! * the editor's pass works in display gamma 2.2 (`display`); Bevy has already applied the
//!   look's saturation (`ColorGrading::post_saturation`) before the pass, and the colour haze and
//!   sky tint are mixed in before the LUT (they need depth and the view direction);
//! * the exports take sRGB-encoded colour after their engine's tonemapper (`srgb`), so their LUT
//!   also carries the saturation.
//!
//! A user LUT (`.cube`, a LUT strip, a HALD image or a Photoshop round-trip: `files`) is applied
//! last, in sRGB, faded by its strength.

pub mod files;
pub mod lut;

pub use lut::{Lut3, UserLut};

use crate::look::LookParams;

/// Cells per side of the editor's runtime LUT.
pub const RUNTIME_SIZE: usize = 33;

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let d = mx - mn;
    let h = if d <= 1e-10 {
        0.0
    } else if mx == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0) / 6.0
    } else if mx == c[1] {
        ((c[2] - c[0]) / d + 2.0) / 6.0
    } else {
        ((c[0] - c[1]) / d + 4.0) / 6.0
    };
    [h, if mx > 1e-10 { d / mx } else { 0.0 }, mx]
}

fn hsv_to_rgb(c: [f32; 3]) -> [f32; 3] {
    let k = [1.0, 2.0 / 3.0, 1.0 / 3.0];
    let mut out = [0.0; 3];
    for i in 0..3 {
        let p = ((c[0] + k[i]).fract() * 6.0 - 3.0).abs();
        out[i] = c[2] * (1.0 + ((p - 1.0).clamp(0.0, 1.0) - 1.0) * c[1]);
    }
    out
}

/// The grade in display gamma 2.2: contrast round display mid-grey, split toning + lift,
/// foliage hue, vibrance. Input and output are ≥ 0 and may exceed 1.
pub fn display(lk: &LookParams, c: [f32; 3]) -> [f32; 3] {
    let mut col = c.map(|v| (0.45 + (v - 0.45) * lk.contrast).max(0.0));
    // split toning + lift
    let l = luma(col);
    let ws = 1.0 - smoothstep(0.0, 0.5, l);
    let wh = smoothstep(0.5, 1.0, l);
    for (i, c) in col.iter_mut().enumerate() {
        *c *= (1.0 + (lk.shadow_tint[i] - 1.0) * ws) * (1.0 + (lk.highlight_tint[i] - 1.0) * wh);
        *c = *c * (1.0 - lk.lift) + lk.lift;
    }
    // foliage hue: greens towards yellow (−) or teal (+)
    if lk.foliage_hue.abs() > 0.001 {
        let mut hsv = rgb_to_hsv(col.map(|v| v.clamp(0.0, 1.0)));
        let g = (1.0 - smoothstep(0.05, 0.13, (hsv[0] - 0.3).abs())) * smoothstep(0.05, 0.2, hsv[1]);
        hsv[0] = (hsv[0] + lk.foliage_hue * 0.07 * g).rem_euclid(1.0);
        let rot = hsv_to_rgb(hsv);
        for i in 0..3 {
            col[i] += (rot[i] - col[i]) * g;
        }
    }
    // vibrance: muted colours more than saturated ones
    if lk.vibrance.abs() > 0.001 {
        let sat = col[0].max(col[1]).max(col[2]) - col[0].min(col[1]).min(col[2]);
        let lv = luma(col);
        let k = 1.0 + lk.vibrance * (1.0 - sat);
        col = col.map(|v| lv + (v - lv) * k);
    }
    col
}

/// A user LUT applied to an sRGB colour, faded by `strength`.
fn apply_user(user: Option<(&Lut3, f32)>, srgb: [f32; 3]) -> [f32; 3] {
    match user {
        Some((lut, s)) if s > 1e-4 => {
            let g = lut.sample(srgb);
            [0, 1, 2].map(|i| srgb[i] + (g[i] - srgb[i]) * s)
        }
        _ => srgb,
    }
}

/// One display colour (sRGB, after an engine's tonemapper) through the whole grade: Bevy's
/// post-tonemap saturation, then `display`, then the user LUT. What the exports bake.
pub fn srgb(lk: &LookParams, user: Option<(&Lut3, f32)>, c: [f32; 3]) -> [f32; 3] {
    // Bevy's post-tonemap saturation (linear)
    let lin = c.map(srgb_to_linear);
    let l = luma(lin);
    let lin = lin.map(|v| (l + (v - l) * lk.saturation).max(0.0));
    // the style pass works in gamma 2.2
    let col = display(lk, lin.map(|v| v.powf(1.0 / 2.2)));
    apply_user(user, col.map(|v| linear_to_srgb(v.max(0.0).powf(2.2))))
}

/// The editor's runtime LUT: `display` (and the user LUT) over a grid in display gamma 2.2.
pub fn bake_runtime(lk: &LookParams, user: Option<(&Lut3, f32)>, n: usize) -> Lut3 {
    let user = user.filter(|(_, s)| *s > 1e-4);
    Lut3::from_fn(n, |c| {
        let col = display(lk, c);
        match user {
            None => col,
            Some(_) => {
                let s = apply_user(user, col.map(|v| linear_to_srgb(v.max(0.0).powf(2.2))));
                s.map(|v| srgb_to_linear(v).powf(1.0 / 2.2))
            }
        }
    })
}

/// The parameters `bake_runtime` reads, to tell when the LUT must be baked again.
pub fn runtime_key(lk: &LookParams) -> [f32; 10] {
    [
        lk.contrast,
        lk.shadow_tint[0],
        lk.shadow_tint[1],
        lk.shadow_tint[2],
        lk.highlight_tint[0],
        lk.highlight_tint[1],
        lk.highlight_tint[2],
        lk.lift,
        lk.foliage_hue,
        lk.vibrance,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::look::LookPreset;

    #[test]
    fn the_neutral_look_is_an_identity_lut() {
        let lk = LookParams::default();
        for c in [[0.0, 0.0, 0.0], [0.2, 0.5, 0.8], [1.0, 1.0, 1.0], [0.9, 0.1, 0.3]] {
            let g = srgb(&lk, None, c);
            for i in 0..3 {
                assert!((g[i] - c[i]).abs() < 0.01, "{c:?} -> {g:?}");
            }
        }
        // a full-strength identity user LUT changes nothing either
        let id = Lut3::identity(17);
        let g = srgb(&lk, Some((&id, 1.0)), [0.3, 0.6, 0.1]);
        assert!((g[0] - 0.3).abs() < 1e-3 && (g[1] - 0.6).abs() < 1e-3, "{g:?}");
        let r = bake_runtime(&lk, Some((&id, 1.0)), 9);
        let s = r.sample([0.25, 0.5, 0.75]);
        assert!((s[0] - 0.25).abs() < 2e-3 && (s[2] - 0.75).abs() < 2e-3, "{s:?}");
    }

    #[test]
    fn contrast_and_split_toning_move_colours() {
        let lk = LookParams {
            contrast: 1.3,
            shadow_tint: [0.8, 0.9, 1.2],
            ..Default::default()
        };
        let dark = srgb(&lk, None, [0.2, 0.2, 0.2]);
        assert!(dark[2] > dark[0], "{dark:?}");
        let bright = srgb(&lk, None, [0.9, 0.9, 0.9]);
        assert!(bright[0] > 0.9, "{bright:?}");
    }

    /// The baked runtime LUT stands in for the per-pixel maths: sampled between its cells it must
    /// stay well within an 8-bit step on average for every built-in look. The worst cells are
    /// saturated greens at the edge of the foliage-hue window and the crushed blacks, a few
    /// 8-bit steps at most; real frames (the in-app parity check against the old per-pixel
    /// shader, 2026-10-26, every built-in look) stayed under 1/255.
    #[test]
    fn the_runtime_lut_matches_the_maths() {
        for p in LookPreset::ALL {
            let lk = p.params();
            let lut = bake_runtime(&lk, None, RUNTIME_SIZE);
            let mut worst = 0.0f32;
            let mut sum = 0.0f32;
            let mut n = 0;
            let mut rng = fastrand::Rng::with_seed(7);
            for _ in 0..20_000 {
                let c = [rng.f32(), rng.f32(), rng.f32()];
                let a = display(&lk, c);
                let b = lut.sample(c);
                for i in 0..3 {
                    let e = (a[i] - b[i]).abs();
                    worst = worst.max(e);
                    sum += e;
                    n += 1;
                }
            }
            let mean = sum / n as f32;
            assert!(
                mean * 255.0 < 0.1 && worst * 255.0 < 4.5,
                "{p:?}: mean {:.3}/255, worst {:.3}/255",
                mean * 255.0,
                worst * 255.0
            );
        }
    }
}
