//! The world's art style (look) for Godot: the tone curve as Godot's tonemapper, and everything the
//! editor grades after tonemapping (`crate::grade`: saturation, contrast round display mid-grey,
//! split toning, lift, foliage hue, vibrance, the user's LUT) baked into a 3D colour-correction
//! LUT, which Godot applies after its own tonemapping too. The lens effects (vignette, grain, sharpening, chromatic
//! aberration) and the painterly / ink styles run in `wb_post.gdshader`.

use crate::grade::Lut3;
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

/// The grade as a Godot 3D-texture PNG: `LUT_SIZE` slices side by side (blue across the slices,
/// red across, green down each slice).
pub fn grade_lut(lk: &LookParams, user: Option<(&Lut3, f32)>) -> image::RgbImage {
    grade_lut_sized(lk, user, LUT_SIZE)
}

/// The grade as a strip of `n` slices of `n` x `n` (blue across the slices, red across, green
/// down each slice): Godot's 3D texture at 32, Unreal's colour grading LUT at 16.
pub fn grade_lut_sized(lk: &LookParams, user: Option<(&Lut3, f32)>, n: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(n * n, n);
    let at = |i: u32| i as f32 / (n - 1) as f32;
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let c = crate::grade::srgb(lk, user, [at(r), at(g), at(b)]);
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
