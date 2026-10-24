//! The world's art style (look) for Godot: the tone curve as Godot's tonemapper, and everything the
//! editor grades after tonemapping (`look_post.wesl`: saturation, contrast round display mid-grey,
//! split toning, lift, foliage hue, vibrance) baked into a 3D colour-correction LUT, which Godot
//! applies after its own tonemapping too. The lens effects (vignette, grain, sharpening, chromatic
//! aberration) and the painterly / ink styles run in `wb_post.gdshader`.

use crate::look::{LookParams, ToneCurve};

/// LUT cells per side.
pub const LUT_SIZE: u32 = 32;

/// Godot's `Environment.tonemap_mode` for the editor's tone curve (Bevy's): AgX and ACES exist in
/// Godot (4.4+); Blender Filmic is closest to Godot's Filmic, and Tony McMapface (neutral, clean
/// colours) to AgX.
pub fn tonemap_mode(tone: ToneCurve) -> u32 {
    match tone {
        ToneCurve::Filmic | ToneCurve::Soft => 4,
        ToneCurve::Punchy => 3,
        ToneCurve::Classic => 2,
    }
}

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
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

/// One display colour (sRGB, after tonemapping) through the editor's post-tonemap grade.
pub fn grade(lk: &LookParams, srgb: [f32; 3]) -> [f32; 3] {
    // Bevy's post-tonemap saturation (linear)
    let lin = srgb.map(srgb_to_linear);
    let l = luma(lin);
    let lin = lin.map(|v| (l + (v - l) * lk.saturation).max(0.0));
    // the style pass works in gamma 2.2
    let mut col = lin.map(|v| v.powf(1.0 / 2.2));
    // contrast round display mid-grey
    col = col.map(|v| (0.45 + (v - 0.45) * lk.contrast).max(0.0));
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
    col.map(|v| linear_to_srgb(v.max(0.0).powf(2.2)))
}

/// The grade as a Godot 3D-texture PNG: `LUT_SIZE` slices side by side (blue across the slices,
/// red across, green down each slice).
pub fn grade_lut(lk: &LookParams) -> image::RgbImage {
    grade_lut_sized(lk, LUT_SIZE)
}

/// The grade as a strip of `n` slices of `n` x `n` (blue across the slices, red across, green
/// down each slice): Godot's 3D texture at 32, Unreal's colour grading LUT at 16.
pub fn grade_lut_sized(lk: &LookParams, n: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(n * n, n);
    let at = |i: u32| i as f32 / (n - 1) as f32;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let c = grade(lk, [at(r), at(g), at(b)]);
                img.put_pixel(
                    b * n + r,
                    g,
                    image::Rgb(c.map(|v| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8)),
                );
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_natural_look_is_an_identity_lut() {
        let lk = LookParams::default();
        for c in [[0.0, 0.0, 0.0], [0.2, 0.5, 0.8], [1.0, 1.0, 1.0], [0.9, 0.1, 0.3]] {
            let g = grade(&lk, c);
            for i in 0..3 {
                assert!((g[i] - c[i]).abs() < 0.01, "{c:?} -> {g:?}");
            }
        }
    }

    #[test]
    fn contrast_and_split_toning_move_colours() {
        let lk = LookParams {
            contrast: 1.3,
            shadow_tint: [0.8, 0.9, 1.2],
            ..Default::default()
        };
        let dark = grade(&lk, [0.2, 0.2, 0.2]);
        assert!(dark[2] > dark[0], "{dark:?}");
        let bright = grade(&lk, [0.9, 0.9, 0.9]);
        assert!(bright[0] > 0.9, "{bright:?}");
    }
}
