//! Style › Fine-tune › Shuffle: new fine-tune values for the style in use, in one click. The style
//! (Overcast, Lavender Dream…) stays: a shuffle picks a mood (sunlit, crisp alpine, misty,
//! storybook, dusky, moody film, faded vintage, eerie, dreamy pastel, bold heroic), mostly one
//! that suits the style, draws every value from the mood's ranges, with split toning from a
//! matching pair of hues, so colour, light, haze and lens agree with each other instead of being
//! independent dice, and blends that with the style as designed, so it reads as a take on the
//! style, not another one. The same seed and style give the same values.

use crate::look::{LookParams, LookPreset, ToneCurve};

/// A shuffled look.
pub struct Shuffled {
    pub params: LookParams,
    /// the mood it was drawn from ("Dusky")
    pub mood: &'static str,
    /// a time of day that suits it (h)
    pub hour: f32,
}

type R = (f32, f32);

struct Mood {
    name: &'static str,
    bases: &'static [LookPreset],
    tones: &'static [ToneCurve],
    exposure: R,
    temperature: R,
    tint: R,
    saturation: R,
    vibrance: R,
    contrast: R,
    lift: R,
    /// split toning: the highlights' hue and the shadows' (degrees), and how strong
    hi_hue: R,
    lo_hue: R,
    split: R,
    sun: R,
    sun_warmth: R,
    sky_fill: R,
    haze: R,
    color_haze: R,
    haze_distance: R,
    sky_hue: R,
    sky_tint: R,
    sunbeams: R,
    bloom: R,
    vignette: R,
    grain: R,
    sharpen: R,
    painterly: R,
    outline: R,
    soften: R,
    foliage: R,
    hours: R,
}

const MOODS: [Mood; 10] = [
    Mood {
        name: "Sunlit",
        bases: &[LookPreset::Natural, LookPreset::GoldenHour],
        tones: &[ToneCurve::Soft, ToneCurve::Filmic],
        exposure: (0.0, 0.2),
        temperature: (0.08, 0.3),
        tint: (-0.03, 0.05),
        saturation: (1.08, 1.28),
        vibrance: (0.05, 0.3),
        contrast: (1.02, 1.15),
        lift: (-0.02, 0.01),
        hi_hue: (35.0, 50.0),
        lo_hue: (200.0, 225.0),
        split: (0.04, 0.12),
        sun: (1.0, 1.25),
        sun_warmth: (0.15, 0.45),
        sky_fill: (0.9, 1.15),
        haze: (0.6, 1.1),
        color_haze: (0.1, 0.35),
        haze_distance: (1200.0, 2600.0),
        sky_hue: (195.0, 215.0),
        sky_tint: (0.0, 0.2),
        sunbeams: (0.2, 0.6),
        bloom: (0.12, 0.25),
        vignette: (0.0, 0.15),
        grain: (0.0, 0.01),
        sharpen: (0.0, 0.25),
        painterly: (0.0, 0.2),
        outline: (0.0, 0.0),
        soften: (0.0, 0.6),
        foliage: (-0.12, 0.02),
        hours: (10.0, 15.5),
    },
    Mood {
        name: "Crisp alpine",
        bases: &[LookPreset::Natural, LookPreset::Nordic],
        tones: &[ToneCurve::Soft, ToneCurve::Punchy],
        exposure: (0.0, 0.15),
        temperature: (-0.2, 0.0),
        tint: (-0.03, 0.02),
        saturation: (1.0, 1.15),
        vibrance: (0.1, 0.3),
        contrast: (1.1, 1.22),
        lift: (-0.03, 0.0),
        hi_hue: (40.0, 60.0),
        lo_hue: (205.0, 230.0),
        split: (0.02, 0.08),
        sun: (1.05, 1.3),
        sun_warmth: (-0.15, 0.1),
        sky_fill: (0.95, 1.2),
        haze: (0.4, 0.8),
        color_haze: (0.05, 0.25),
        haze_distance: (2000.0, 4500.0),
        sky_hue: (200.0, 220.0),
        sky_tint: (0.05, 0.3),
        sunbeams: (0.0, 0.3),
        bloom: (0.08, 0.16),
        vignette: (0.0, 0.12),
        grain: (0.0, 0.006),
        sharpen: (0.2, 0.5),
        painterly: (0.0, 0.1),
        outline: (0.0, 0.0),
        soften: (0.0, 0.3),
        foliage: (0.0, 0.12),
        hours: (9.0, 13.0),
    },
    Mood {
        name: "Misty",
        bases: &[LookPreset::Nordic, LookPreset::Overcast],
        tones: &[ToneCurve::Filmic, ToneCurve::Classic],
        exposure: (0.05, 0.25),
        temperature: (-0.15, 0.05),
        tint: (-0.03, 0.03),
        saturation: (0.8, 0.95),
        vibrance: (0.0, 0.15),
        contrast: (0.9, 1.0),
        lift: (0.0, 0.04),
        hi_hue: (40.0, 60.0),
        lo_hue: (190.0, 215.0),
        split: (0.02, 0.07),
        sun: (0.75, 0.95),
        sun_warmth: (-0.1, 0.15),
        sky_fill: (1.1, 1.4),
        haze: (1.5, 2.4),
        color_haze: (0.35, 0.65),
        haze_distance: (500.0, 1100.0),
        sky_hue: (195.0, 220.0),
        sky_tint: (0.1, 0.35),
        sunbeams: (0.2, 0.7),
        bloom: (0.15, 0.3),
        vignette: (0.05, 0.2),
        grain: (0.0, 0.01),
        sharpen: (0.0, 0.1),
        painterly: (0.0, 0.25),
        outline: (0.0, 0.0),
        soften: (0.2, 1.0),
        foliage: (0.0, 0.1),
        hours: (7.0, 9.5),
    },
    Mood {
        name: "Storybook",
        bases: &[
            LookPreset::Enchanted,
            LookPreset::Natural,
            LookPreset::GoldenHour,
        ],
        tones: &[ToneCurve::Soft],
        exposure: (0.05, 0.25),
        temperature: (0.0, 0.2),
        tint: (-0.02, 0.06),
        saturation: (1.2, 1.4),
        vibrance: (0.2, 0.45),
        contrast: (1.0, 1.12),
        lift: (0.0, 0.03),
        hi_hue: (40.0, 60.0),
        lo_hue: (220.0, 260.0),
        split: (0.06, 0.14),
        sun: (1.0, 1.2),
        sun_warmth: (0.1, 0.35),
        sky_fill: (1.05, 1.3),
        haze: (0.7, 1.2),
        color_haze: (0.2, 0.45),
        haze_distance: (900.0, 2000.0),
        sky_hue: (190.0, 230.0),
        sky_tint: (0.2, 0.45),
        sunbeams: (0.2, 0.6),
        bloom: (0.2, 0.35),
        vignette: (0.0, 0.15),
        grain: (0.0, 0.0),
        sharpen: (0.0, 0.15),
        painterly: (0.3, 0.6),
        outline: (0.0, 0.25),
        soften: (0.8, 2.0),
        foliage: (-0.15, 0.0),
        hours: (10.0, 16.0),
    },
    Mood {
        name: "Dusky",
        bases: &[
            LookPreset::Lavender,
            LookPreset::GoldenHour,
            LookPreset::RoseHaze,
        ],
        tones: &[ToneCurve::Filmic, ToneCurve::Soft],
        exposure: (-0.15, 0.05),
        temperature: (0.15, 0.35),
        tint: (0.03, 0.12),
        saturation: (1.0, 1.2),
        vibrance: (0.1, 0.3),
        contrast: (1.05, 1.18),
        lift: (-0.02, 0.02),
        hi_hue: (20.0, 40.0),
        lo_hue: (250.0, 285.0),
        split: (0.08, 0.16),
        sun: (0.9, 1.15),
        sun_warmth: (0.35, 0.7),
        sky_fill: (0.8, 1.05),
        haze: (0.9, 1.5),
        color_haze: (0.3, 0.55),
        haze_distance: (800.0, 1800.0),
        sky_hue: (270.0, 320.0),
        sky_tint: (0.15, 0.4),
        sunbeams: (0.4, 0.9),
        bloom: (0.2, 0.35),
        vignette: (0.1, 0.25),
        grain: (0.0, 0.012),
        sharpen: (0.0, 0.15),
        painterly: (0.0, 0.25),
        outline: (0.0, 0.0),
        soften: (0.0, 0.8),
        foliage: (-0.1, 0.05),
        hours: (17.8, 19.0),
    },
    Mood {
        name: "Moody film",
        bases: &[LookPreset::Cinematic],
        tones: &[ToneCurve::Filmic, ToneCurve::Soft],
        exposure: (-0.05, 0.12),
        temperature: (0.0, 0.15),
        tint: (-0.03, 0.03),
        saturation: (0.9, 1.08),
        vibrance: (0.0, 0.15),
        contrast: (1.1, 1.22),
        lift: (-0.01, 0.02),
        hi_hue: (28.0, 40.0),
        lo_hue: (180.0, 200.0),
        split: (0.1, 0.18),
        sun: (0.95, 1.2),
        sun_warmth: (0.1, 0.35),
        sky_fill: (0.9, 1.1),
        haze: (0.8, 1.3),
        color_haze: (0.15, 0.4),
        haze_distance: (900.0, 2200.0),
        sky_hue: (185.0, 205.0),
        sky_tint: (0.1, 0.3),
        sunbeams: (0.2, 0.6),
        bloom: (0.12, 0.25),
        vignette: (0.15, 0.3),
        grain: (0.006, 0.02),
        sharpen: (0.1, 0.35),
        painterly: (0.0, 0.0),
        outline: (0.0, 0.0),
        soften: (0.0, 0.2),
        foliage: (0.05, 0.2),
        hours: (15.0, 18.0),
    },
    Mood {
        name: "Faded vintage",
        bases: &[LookPreset::GoldenVintage],
        tones: &[ToneCurve::Classic, ToneCurve::Filmic],
        exposure: (0.0, 0.15),
        temperature: (0.15, 0.35),
        tint: (0.0, 0.06),
        saturation: (0.72, 0.9),
        vibrance: (-0.1, 0.1),
        contrast: (0.92, 1.05),
        lift: (0.03, 0.07),
        hi_hue: (38.0, 50.0),
        lo_hue: (20.0, 40.0),
        split: (0.06, 0.14),
        sun: (0.95, 1.15),
        sun_warmth: (0.2, 0.45),
        sky_fill: (1.0, 1.2),
        haze: (0.9, 1.4),
        color_haze: (0.25, 0.5),
        haze_distance: (900.0, 1800.0),
        sky_hue: (35.0, 50.0),
        sky_tint: (0.1, 0.3),
        sunbeams: (0.1, 0.4),
        bloom: (0.15, 0.3),
        vignette: (0.15, 0.35),
        grain: (0.01, 0.03),
        sharpen: (0.0, 0.1),
        painterly: (0.0, 0.2),
        outline: (0.0, 0.0),
        soften: (0.0, 0.6),
        foliage: (-0.2, -0.05),
        hours: (14.0, 17.0),
    },
    Mood {
        name: "Eerie",
        bases: &[
            LookPreset::Overcast,
            LookPreset::Nordic,
            LookPreset::Cinematic,
        ],
        tones: &[ToneCurve::Filmic, ToneCurve::Classic],
        exposure: (-0.1, 0.08),
        temperature: (-0.25, -0.05),
        tint: (-0.12, -0.03),
        saturation: (0.75, 0.92),
        vibrance: (-0.1, 0.1),
        contrast: (1.05, 1.2),
        lift: (-0.02, 0.02),
        hi_hue: (80.0, 120.0),
        lo_hue: (170.0, 200.0),
        split: (0.06, 0.14),
        sun: (0.7, 0.9),
        sun_warmth: (-0.3, 0.0),
        sky_fill: (0.95, 1.15),
        haze: (1.4, 2.4),
        color_haze: (0.35, 0.65),
        haze_distance: (450.0, 1000.0),
        sky_hue: (100.0, 160.0),
        sky_tint: (0.15, 0.4),
        sunbeams: (0.0, 0.4),
        bloom: (0.1, 0.2),
        vignette: (0.2, 0.35),
        grain: (0.006, 0.02),
        sharpen: (0.0, 0.2),
        painterly: (0.0, 0.15),
        outline: (0.0, 0.0),
        soften: (0.0, 0.5),
        foliage: (0.1, 0.3),
        hours: (16.5, 18.6),
    },
    Mood {
        name: "Dreamy pastel",
        bases: &[
            LookPreset::Lavender,
            LookPreset::RoseHaze,
            LookPreset::Enchanted,
        ],
        tones: &[ToneCurve::Soft, ToneCurve::Classic],
        exposure: (0.1, 0.3),
        temperature: (0.0, 0.15),
        tint: (0.04, 0.14),
        saturation: (0.95, 1.12),
        vibrance: (0.1, 0.3),
        contrast: (0.88, 0.98),
        lift: (0.03, 0.07),
        hi_hue: (330.0, 360.0),
        lo_hue: (240.0, 275.0),
        split: (0.06, 0.14),
        sun: (0.95, 1.1),
        sun_warmth: (0.0, 0.3),
        sky_fill: (1.15, 1.45),
        haze: (1.1, 1.8),
        color_haze: (0.35, 0.6),
        haze_distance: (700.0, 1500.0),
        sky_hue: (290.0, 340.0),
        sky_tint: (0.25, 0.5),
        sunbeams: (0.2, 0.6),
        bloom: (0.3, 0.45),
        vignette: (0.0, 0.15),
        grain: (0.0, 0.0),
        sharpen: (0.0, 0.05),
        painterly: (0.15, 0.45),
        outline: (0.0, 0.0),
        soften: (0.5, 1.6),
        foliage: (0.0, 0.15),
        hours: (6.3, 7.6),
    },
    Mood {
        name: "Bold heroic",
        bases: &[LookPreset::EpicSunbeams, LookPreset::Natural],
        tones: &[ToneCurve::Punchy, ToneCurve::Soft],
        exposure: (0.0, 0.15),
        temperature: (0.05, 0.22),
        tint: (-0.03, 0.03),
        saturation: (1.2, 1.38),
        vibrance: (0.15, 0.35),
        contrast: (1.12, 1.25),
        lift: (-0.03, 0.0),
        hi_hue: (35.0, 48.0),
        lo_hue: (205.0, 235.0),
        split: (0.05, 0.12),
        sun: (1.1, 1.35),
        sun_warmth: (0.15, 0.4),
        sky_fill: (0.9, 1.1),
        haze: (0.7, 1.2),
        color_haze: (0.15, 0.35),
        haze_distance: (1300.0, 2800.0),
        sky_hue: (200.0, 220.0),
        sky_tint: (0.1, 0.3),
        sunbeams: (0.5, 1.0),
        bloom: (0.15, 0.3),
        vignette: (0.05, 0.2),
        grain: (0.0, 0.006),
        sharpen: (0.15, 0.4),
        painterly: (0.0, 0.3),
        outline: (0.0, 0.1),
        soften: (0.0, 1.0),
        foliage: (-0.08, 0.08),
        hours: (13.0, 17.5),
    },
];

/// Every mood's name (MCP).
pub fn moods() -> Vec<&'static str> {
    MOODS.iter().map(|m| m.name).collect()
}

/// The pure colour of `hue` (degrees), its brightest channel 1.
fn hue_rgb(hue: f32) -> [f32; 3] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

/// A multiplier round 1.0 that leans towards `hue` (degrees) by `amount`, keeping brightness.
fn hue_tint(hue: f32, amount: f32) -> [f32; 3] {
    let c = hue_rgb(hue);
    let avg = (c[0] + c[1] + c[2]) / 3.0;
    c.map(|v| 1.0 + amount * (v - avg) * 1.5)
}

/// The sRGB colour of `hue` at saturation `s` and value `v`.
fn hue_color(hue: f32, s: f32, v: f32) -> [f32; 3] {
    hue_rgb(hue).map(|c| (v * (1.0 - s + s * c)).clamp(0.0, 1.0))
}

/// New fine-tune values for `preset` from `seed`; `mood` picks one by name (any case; None: mostly
/// one that suits the style, sometimes any).
pub fn shuffle(seed: u64, preset: LookPreset, mood: Option<&str>) -> Shuffled {
    let mut r = fastrand::Rng::with_seed(seed);
    let suits: Vec<&Mood> = MOODS.iter().filter(|m| m.bases.contains(&preset)).collect();
    let m = match mood.and_then(|n| MOODS.iter().find(|m| m.name.eq_ignore_ascii_case(n.trim()))) {
        Some(m) => m,
        None if !suits.is_empty() && r.f32() < 0.75 => suits[r.usize(..suits.len())],
        None => &MOODS[r.usize(..MOODS.len())],
    };
    let fits = m.bases.contains(&preset);
    let mut pick = |range: R| range.0 + (range.1 - range.0) * r.f32();
    let design = preset.params();
    let mut p = design;
    p.exposure = pick(m.exposure);
    p.temperature = pick(m.temperature);
    p.tint = pick(m.tint);
    p.saturation = pick(m.saturation);
    p.vibrance = pick(m.vibrance);
    p.contrast = pick(m.contrast);
    p.lift = pick(m.lift);
    let split = pick(m.split);
    p.highlight_tint = hue_tint(pick(m.hi_hue), split);
    p.shadow_tint = hue_tint(pick(m.lo_hue), split);
    p.foliage_hue = pick(m.foliage);
    p.sun = pick(m.sun);
    p.sun_warmth = pick(m.sun_warmth);
    p.sky_fill = pick(m.sky_fill);
    p.occlusion = pick((0.8, 1.3));
    p.haze = pick(m.haze);
    p.color_haze = pick(m.color_haze);
    p.haze_distance = pick(m.haze_distance);
    let sky_hue = pick(m.sky_hue);
    p.sky_tint = hue_color(sky_hue, 0.45, 0.95);
    p.sky_tint_amount = pick(m.sky_tint);
    // the distance fades into a pale mix of the sky's colour and the light's warmth
    let warm = hue_color(pick(m.hi_hue), 0.25, 0.92);
    let k = pick((0.2, 0.6));
    p.haze_color = [
        p.sky_tint[0] * (1.0 - k) + warm[0] * k,
        p.sky_tint[1] * (1.0 - k) + warm[1] * k,
        p.sky_tint[2] * (1.0 - k) + warm[2] * k,
    ];
    p.sunbeams = pick(m.sunbeams);
    p.bloom = pick(m.bloom);
    p.vignette = pick(m.vignette);
    p.grain = pick(m.grain);
    p.sharpen = pick(m.sharpen);
    p.aberration = 0.0;
    p.painterly = pick(m.painterly);
    p.outline = pick(m.outline);
    p.texture_soften = pick(m.soften);
    p.detail = pick((0.85, 1.15));
    p.tone = m.tones[(pick((0.0, m.tones.len() as f32 - 0.001))) as usize];
    // the knobs the moods leave alone vary round the style's own
    p.air = (design.air * pick((0.7, 1.4))).clamp(0.0, 3.0);
    p.mist = (design.mist + pick((-0.1, 0.3))).clamp(0.0, 1.0);
    p.glare = pick((0.1, 0.5));
    p.flare = pick((0.0, 0.3));
    let hour = pick(m.hours);
    // a take on the style: a mood that suits it leads, any other one only leans on it
    let k = if fits {
        pick((0.55, 0.9))
    } else {
        pick((0.3, 0.55))
    };
    Shuffled {
        params: design.lerp(&p, k),
        mood: m.name,
        hour,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shuffles_stay_in_range_and_repeat_per_seed() {
        for seed in 0..400u64 {
            let s = shuffle(
                seed,
                LookPreset::ALL[seed as usize % LookPreset::ALL.len()],
                None,
            );
            let p = s.params;
            assert!(
                (-1.5..=1.5).contains(&p.exposure)
                    && (0.0..=2.0).contains(&p.saturation)
                    && (0.6..=1.5).contains(&p.contrast),
                "{seed}"
            );
            assert!(
                (0.0..=4.0).contains(&p.haze)
                    && (150.0..=6000.0).contains(&p.haze_distance)
                    && (0.0..=0.1).contains(&p.grain),
                "{seed}"
            );
            for c in p.highlight_tint.iter().chain(&p.shadow_tint) {
                assert!((0.6..=1.4).contains(c), "{seed}: tint {c}");
            }
            for c in p.sky_tint.iter().chain(&p.haze_color) {
                assert!((0.0..=1.0).contains(c), "{seed}: colour {c}");
            }
            assert!((0.0..24.0).contains(&s.hour));
        }
        let (a, b) = (
            shuffle(7, LookPreset::Overcast, None),
            shuffle(7, LookPreset::Overcast, None),
        );
        assert_eq!(a.params, b.params);
        assert_eq!(shuffle(3, LookPreset::Natural, Some("misty")).mood, "Misty");
        assert_ne!(
            shuffle(7, LookPreset::Overcast, None).params,
            LookPreset::Overcast.params()
        );
    }
}
