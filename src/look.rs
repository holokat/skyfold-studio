//! Art styles ("looks"): one click switches the whole world between the physically based look and
//! stylised looks inspired by well-known MMOs. A look bundles colour grading, lighting, atmosphere,
//! lens and painterly stylisation. It is layered on top of the physical day/night lighting and the
//! per-zone moods, so it holds at every hour and in every weather.
//!
//! A zone with its own lighting & style (or the biome under the viewer) is blended into the
//! effective look here: its art style replaces the world's by its strength, and its light
//! (brightness, contrast, colour, sun, shadow fill, glow, vignette) stacks on top. It crossfades
//! as the viewer crosses the zone's border; its fog goes to the distance fog in `sky`.
//!
//! Where each parameter lands:
//! * camera components: tone curve, colour grading (in `sky`), bloom, sharpening, chromatic
//!   aberration, vignette;
//! * `sky`: exposure, night lift, sun strength and warmth, sky fill light, haze density/colour;
//! * terrain shader: texture softening (mip bias) and surface detail (normal-map strength);
//! * the style post pass (`look_post`): painterly filter, ink outlines, colour haze, sky tint,
//!   split toning, vibrance, foliage hue and film grain.

use bevy::anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening;
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::post_process::effect_stack::{ChromaticAberration, Vignette};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::camera::MainCamera;
use crate::haze::{self, HazeState};
use crate::grade::{Lut3, UserLut};
use crate::look_post::{GradeLut, LookPost, LookPostPlugin};
use crate::sky::{Lighting, SkyState};
use crate::terrain::Terrain;
use crate::world::{Mood, ZoneLight, Zones};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum LookPreset {
    // retired looks fall back to Natural in old saves
    #[default]
    #[serde(alias = "Painted", alias = "Storybook", alias = "Toon")]
    Natural,
    Nordic,
    Cinematic,
    // scene templates: a grade plus the time of day, sky and haze that go with it
    EpicSunbeams,
    Overcast,
    Lavender,
    GoldenVintage,
    RoseHaze,
    Hellfire,
    Enchanted,
    GoldenHour,
}

impl LookPreset {
    pub const ALL: [LookPreset; 11] = [
        LookPreset::Natural,
        LookPreset::Nordic,
        LookPreset::Cinematic,
        LookPreset::EpicSunbeams,
        LookPreset::Overcast,
        LookPreset::Lavender,
        LookPreset::GoldenVintage,
        LookPreset::RoseHaze,
        LookPreset::Hellfire,
        LookPreset::Enchanted,
        LookPreset::GoldenHour,
    ];

    /// Scene templates also set (hour, sky, haze) when picked.
    pub fn scene(self) -> Option<(f32, crate::clouds::CloudPreset, f32)> {
        use crate::clouds::CloudPreset as C;
        match self {
            LookPreset::EpicSunbeams => Some((10.5, C::Broken, 0.25)),
            LookPreset::Overcast => Some((11.5, C::Broken, 0.45)),
            LookPreset::Lavender => Some((16.6, C::Scattered, 0.4)),
            LookPreset::GoldenVintage => Some((16.2, C::FairWeather, 0.35)),
            LookPreset::RoseHaze => Some((17.8, C::Scattered, 0.45)),
            LookPreset::Hellfire => Some((18.9, C::Towering, 0.5)),
            LookPreset::Enchanted => Some((17.3, C::Scattered, 0.55)),
            LookPreset::GoldenHour => Some((17.9, C::FairWeather, 0.3)),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LookPreset::Natural => "Natural",
            LookPreset::Nordic => "Nordic Mist",
            LookPreset::Cinematic => "Cinematic",
            LookPreset::EpicSunbeams => "Epic Sunbeams",
            LookPreset::Overcast => "Overcast",
            LookPreset::Lavender => "Lavender Dream",
            LookPreset::GoldenVintage => "Golden Vintage",
            LookPreset::RoseHaze => "Rose Haze",
            LookPreset::Hellfire => "Hellfire Dusk",
            LookPreset::Enchanted => "Enchanted Grove",
            LookPreset::GoldenHour => "Golden Hour",
        }
    }

    /// The game the look is modelled on (shown as a hint, never exported).
    pub fn inspired_by(self) -> &'static str {
        match self {
            LookPreset::Natural => "physically based",
            LookPreset::Nordic => "like The Elder Scrolls",
            LookPreset::Cinematic => "like Black Desert Online",
            LookPreset::EpicSunbeams
            | LookPreset::Overcast
            | LookPreset::Lavender
            | LookPreset::GoldenVintage
            | LookPreset::RoseHaze
            | LookPreset::Hellfire
            | LookPreset::Enchanted
            | LookPreset::GoldenHour => "scene template: grade, time, sky",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            LookPreset::Natural => {
                "Realistic light and colour with a clean, contrasty camera grade."
            }
            LookPreset::Nordic => "Cool, muted colours, pale misty distances, crisp detail.",
            LookPreset::Cinematic => {
                "Punchy photoreal: golden sun, deep teal shadows, sharp detail, filmic lens."
            }
            LookPreset::EpicSunbeams => {
                "Dramatic bright storm clouds, turquoise sea, rich saturated greens and crisp contrast."
            }
            LookPreset::Overcast => "Cool, soft, blue-grey light under a heavy sky; muted greens.",
            LookPreset::Lavender => "Dreamy violet pastel sky and lavender haze over soft greens.",
            LookPreset::GoldenVintage => {
                "Warm yellow-sepia afternoon, gentle contrast, a little film grain."
            }
            LookPreset::RoseHaze => "Pink pastel evening haze, soft and romantic.",
            LookPreset::Hellfire => {
                "A red-black storm sunset: deep crimson light, heavy shadows, smoky air."
            }
            LookPreset::Enchanted => {
                "Teal-green fairy light and glowing mist in the low evening sun."
            }
            LookPreset::GoldenHour => {
                "Low warm sun, glowing rim light, long shadows and amber haze."
            }
        }
    }

    /// Four colours that summarise the look (UI swatch strip).
    pub fn palette(self) -> [[u8; 3]; 4] {
        match self {
            LookPreset::Natural => [
                [86, 118, 66],
                [150, 160, 150],
                [120, 160, 210],
                [230, 225, 210],
            ],
            LookPreset::Nordic => [
                [62, 84, 78],
                [120, 132, 138],
                [190, 202, 210],
                [228, 232, 236],
            ],
            LookPreset::Cinematic => [[24, 44, 52], [92, 120, 40], [214, 160, 72], [250, 226, 170]],
            LookPreset::EpicSunbeams => [
                [40, 90, 40],
                [60, 190, 200],
                [130, 150, 170],
                [250, 245, 220],
            ],
            LookPreset::Overcast => [
                [70, 95, 85],
                [130, 145, 150],
                [175, 190, 200],
                [215, 222, 228],
            ],
            LookPreset::Lavender => [
                [90, 110, 80],
                [150, 140, 200],
                [190, 170, 235],
                [235, 215, 240],
            ],
            LookPreset::GoldenVintage => [
                [110, 110, 40],
                [180, 160, 80],
                [230, 200, 120],
                [250, 235, 180],
            ],
            LookPreset::RoseHaze => [
                [100, 110, 80],
                [200, 150, 150],
                [240, 190, 190],
                [250, 225, 220],
            ],
            LookPreset::Hellfire => [[30, 12, 10], [110, 35, 20], [220, 70, 30], [250, 150, 60]],
            LookPreset::Enchanted => [
                [30, 90, 60],
                [60, 160, 120],
                [140, 210, 170],
                [240, 230, 140],
            ],
            LookPreset::GoldenHour => [
                [70, 90, 30],
                [200, 150, 60],
                [250, 200, 110],
                [255, 240, 200],
            ],
        }
    }

    pub fn params(self) -> LookParams {
        let n = LookParams::default();
        match self {
            // the neutral tone curve with a light camera grade: AgX alone reads grey and washed out
            LookPreset::Natural => LookParams {
                saturation: 1.12,
                contrast: 1.12,
                // (0.75 was set here but never reached the renderer; 1.0 is the look as approved)
                sky_fill: 1.0,
                // aerial perspective: far hills fade towards a pale blue, so the distance reads as
                // distance (it starts behind what the camera looks at)
                color_haze: 0.3,
                haze_distance: 1200.0,
                sunbeams: 0.4,
                glare: 0.4,
                flare: 0.12,
                tone: ToneCurve::Soft,
                ..n
            },
            LookPreset::EpicSunbeams => LookParams {
                exposure: 0.25,
                temperature: 0.05,
                saturation: 1.22,
                contrast: 1.16,
                vibrance: 0.3,
                shadow_tint: [0.9, 1.0, 1.08],
                highlight_tint: [1.05, 1.02, 0.95],
                sun: 1.25,
                haze: 0.6,
                sunbeams: 1.0,
                air: 1.3,
                mist: 0.1,
                bloom: 0.45,
                vignette: 0.25,
                sharpen: 0.55,
                glare: 0.9,
                flare: 0.5,
                detail: 1.3,
                ..n
            },
            LookPreset::Overcast => LookParams {
                temperature: -0.3,
                saturation: 0.78,
                contrast: 0.95,
                lift: 0.03,
                shadow_tint: [0.95, 0.98, 1.06],
                sun: 0.85,
                sky_fill: 1.25,
                haze: 1.4,
                haze_color: [0.82, 0.86, 0.9],
                color_haze: 0.3,
                sky_tint: [0.8, 0.85, 0.9],
                sky_tint_amount: 0.3,
                occlusion: 1.3,
                sunbeams: 0.0,
                air: 1.2,
                mist: 0.25,
                bloom: 0.15,
                sharpen: 0.3,
                ..n
            },
            LookPreset::Lavender => LookParams {
                exposure: 0.15,
                temperature: -0.15,
                tint: 0.55,
                saturation: 0.95,
                contrast: 0.92,
                lift: 0.05,
                shadow_tint: [1.0, 0.92, 1.12],
                highlight_tint: [1.04, 0.98, 1.06],
                haze: 1.6,
                haze_color: [0.85, 0.8, 0.98],
                color_haze: 0.45,
                sky_tint: [0.78, 0.72, 1.0],
                sky_tint_amount: 0.45,
                occlusion: 0.8,
                sunbeams: 0.45,
                air: 1.1,
                mist: 0.2,
                bloom: 0.5,
                ..n
            },
            LookPreset::GoldenVintage => LookParams {
                exposure: 0.1,
                temperature: 0.6,
                tint: 0.05,
                saturation: 0.85,
                lift: 0.04,
                shadow_tint: [1.02, 0.98, 0.85],
                highlight_tint: [1.08, 1.02, 0.8],
                sun_warmth: 0.35,
                haze: 1.2,
                haze_color: [0.95, 0.88, 0.7],
                color_haze: 0.35,
                sky_tint: [1.0, 0.92, 0.7],
                sky_tint_amount: 0.4,
                sunbeams: 0.6,
                air: 1.2,
                bloom: 0.35,
                vignette: 0.35,
                grain: 0.05,
                ..n
            },
            LookPreset::RoseHaze => LookParams {
                exposure: 0.15,
                temperature: 0.15,
                tint: 0.4,
                saturation: 0.9,
                contrast: 0.9,
                lift: 0.05,
                shadow_tint: [1.0, 0.93, 1.0],
                highlight_tint: [1.08, 0.98, 0.98],
                haze: 1.7,
                haze_color: [0.98, 0.82, 0.85],
                color_haze: 0.5,
                sky_tint: [1.0, 0.8, 0.82],
                sky_tint_amount: 0.45,
                occlusion: 0.8,
                sunbeams: 0.5,
                air: 1.4,
                mist: 0.15,
                bloom: 0.5,
                ..n
            },
            LookPreset::Hellfire => LookParams {
                exposure: -0.5,
                temperature: 1.0,
                tint: 0.2,
                saturation: 1.2,
                contrast: 1.3,
                shadow_tint: [1.1, 0.75, 0.65],
                highlight_tint: [1.2, 0.8, 0.55],
                sun_warmth: 0.8,
                sky_fill: 0.6,
                haze: 1.3,
                haze_color: [0.6, 0.25, 0.15],
                color_haze: 0.5,
                sky_tint: [1.0, 0.35, 0.2],
                sky_tint_amount: 0.6,
                occlusion: 1.3,
                sunbeams: 0.6,
                air: 1.8,
                bloom: 0.4,
                vignette: 0.5,
                ..n
            },
            LookPreset::Enchanted => LookParams {
                exposure: 0.1,
                temperature: -0.1,
                tint: -0.6,
                saturation: 1.1,
                lift: 0.04,
                shadow_tint: [0.85, 1.05, 1.0],
                highlight_tint: [1.0, 1.06, 0.9],
                haze: 1.8,
                haze_color: [0.6, 0.9, 0.75],
                color_haze: 0.55,
                haze_distance: 600.0,
                sky_tint: [0.6, 0.95, 0.8],
                sky_tint_amount: 0.5,
                sunbeams: 0.8,
                air: 1.3,
                mist: 0.55,
                bloom: 0.55,
                ..n
            },
            LookPreset::GoldenHour => LookParams {
                exposure: 0.15,
                temperature: 0.55,
                saturation: 1.15,
                contrast: 1.12,
                vibrance: 0.2,
                shadow_tint: [0.92, 0.96, 1.08],
                highlight_tint: [1.1, 1.0, 0.82],
                sun: 1.2,
                sun_warmth: 0.6,
                haze: 1.1,
                haze_color: [1.0, 0.85, 0.6],
                color_haze: 0.3,
                sunbeams: 0.9,
                air: 1.2,
                mist: 0.1,
                bloom: 0.5,
                vignette: 0.25,
                glare: 1.0,
                flare: 0.4,
                ..n
            },
            LookPreset::Nordic => LookParams {
                exposure: 0.1,
                temperature: -0.35,
                tint: -0.1,
                saturation: 0.82,
                contrast: 1.08,
                shadow_tint: [0.88, 0.95, 1.08],
                highlight_tint: [1.03, 1.01, 0.97],
                lift: 0.02,
                foliage_hue: 0.12,
                sun_warmth: -0.05,
                sky_fill: 1.15,
                night_lift: 0.6,
                haze: 1.8,
                haze_color: [0.78, 0.83, 0.88],
                color_haze: 0.35,
                haze_distance: 900.0,
                sky_tint: [0.74, 0.8, 0.86],
                sky_tint_amount: 0.25,
                sunbeams: 0.3,
                mist: 0.3,
                bloom: 0.22,
                vignette: 0.25,
                grain: 0.02,
                sharpen: 0.35,
                detail: 1.15,
                ..n
            },
            // ACES ("Punchy") gives the deep blacks and clean whites the look promises; AgX (the
            // default curve) read grey and washed out here. ACES maps mid-tones darker and adds
            // saturation of its own, so the exposure is higher and saturation / vibrance lower.
            LookPreset::Cinematic => LookParams {
                exposure: 0.8,
                temperature: 0.25,
                saturation: 1.0,
                contrast: 1.14,
                vibrance: 0.1,
                shadow_tint: [0.9, 0.98, 1.06],
                highlight_tint: [1.07, 1.0, 0.88],
                foliage_hue: -0.08,
                sun: 1.2,
                sun_warmth: 0.25,
                sky_fill: 0.9,
                night_lift: 0.6,
                haze: 0.8,
                color_haze: 0.08,
                haze_distance: 2000.0,
                // (0.3 spread a wide glow over everything: a grey veil, worst at night)
                occlusion: 1.2,
                sunbeams: 0.5,
                air: 1.1,
                bloom: 0.12,
                vignette: 0.3,
                grain: 0.03,
                sharpen: 0.6,
                aberration: 0.006,
                glare: 0.75,
                flare: 0.55,
                detail: 1.35,
                tone: ToneCurve::Punchy,
                ..n
            },
        }
    }
}

/// Tone curve (HDR → display). Swapped as a whole, not blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToneCurve {
    /// AgX: gently desaturated highlights, flat and grey-ish overall.
    #[default]
    Filmic,
    /// ACES: high contrast and saturation.
    Punchy,
    /// Tony McMapface: neutral, keeps colours clean (the Natural look and stylised looks).
    Soft,
    /// Blender Filmic: low contrast, wide latitude.
    Classic,
}

impl ToneCurve {
    pub const ALL: [ToneCurve; 4] = [
        ToneCurve::Filmic,
        ToneCurve::Punchy,
        ToneCurve::Soft,
        ToneCurve::Classic,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ToneCurve::Filmic => "Filmic",
            ToneCurve::Punchy => "Punchy",
            ToneCurve::Soft => "Clean",
            ToneCurve::Classic => "Classic",
        }
    }
    fn tonemapping(self) -> Tonemapping {
        match self {
            ToneCurve::Filmic => Tonemapping::AgX,
            ToneCurve::Punchy => Tonemapping::AcesFitted,
            ToneCurve::Soft => Tonemapping::TonyMcMapface,
            ToneCurve::Classic => Tonemapping::BlenderFilmic,
        }
    }
}

/// Every knob of a look. `LookParams::default()` is the Natural look (no change).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookParams {
    // colour
    /// Exposure offset in stops (+ brighter).
    pub exposure: f32,
    pub temperature: f32,
    pub tint: f32,
    pub saturation: f32,
    pub contrast: f32,
    /// Boosts muted colours more than already saturated ones.
    pub vibrance: f32,
    /// Split toning: colour multiplier for dark and bright tones.
    pub shadow_tint: [f32; 3],
    pub highlight_tint: [f32; 3],
    /// Raises (+) or crushes (−) the blacks.
    pub lift: f32,
    /// Rotates greens towards yellow (−) or teal (+).
    pub foliage_hue: f32,
    // light
    pub sun: f32,
    /// Sun colour: − cooler, + warmer.
    pub sun_warmth: f32,
    /// Sky (ambient) light multiplier: > 1 softens shadows.
    pub sky_fill: f32,
    /// Ambient occlusion: how far the soft shade in creases and round objects reaches (× the
    /// standard; 0 = none). Its quality (and whether it runs at all) is Lighting › Ambient
    /// occlusion.
    pub occlusion: f32,
    /// Extra stops of exposure at night (stylised games keep nights readable).
    pub night_lift: f32,
    // atmosphere
    /// Fog density multiplier.
    pub haze: f32,
    /// sRGB colour the distance fades into.
    pub haze_color: [f32; 3],
    pub color_haze: f32,
    /// Distance (m) at which the colour haze reaches ~63 %.
    pub haze_distance: f32,
    /// sRGB colour the sky leans towards.
    pub sky_tint: [f32; 3],
    pub sky_tint_amount: f32,
    /// Sun shafts: rays of sunlight streaming past trees, buildings and cloud edges when the
    /// camera looks towards the sun (`sun_shafts`).
    pub sunbeams: f32,
    /// How hazy the lit air is (× the volumetric light's clear-air haze): what sunbeams, the glow
    /// round the sun and the halos round lamps are made of. 0 = crystal clear.
    pub air: f32,
    /// Ground mist lying a few metres over the terrain, thickest in hollows (0..1; weather fog
    /// and wet ground add their own).
    pub mist: f32,
    // lens
    pub bloom: f32,
    pub vignette: f32,
    pub grain: f32,
    pub sharpen: f32,
    pub aberration: f32,
    /// Sun glare: the soft veiling glare and faint starburst round a bright sun in view
    /// (`camera_fx::flare`); it lifts the shadows of a backlit frame a little, like a real eye or lens.
    pub glare: f32,
    /// Lens flare: ghosts (aperture images, rings) along the line from the sun through the centre
    /// of the view (`camera_fx::flare`).
    pub flare: f32,
    // stylise
    pub painterly: f32,
    pub outline: f32,
    /// Texture softening in mip levels (hand-painted look).
    pub texture_soften: f32,
    /// Surface detail (normal-map strength) multiplier.
    pub detail: f32,
    pub tone: ToneCurve,
}

impl Default for LookParams {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            temperature: 0.0,
            tint: 0.0,
            saturation: 1.0,
            contrast: 1.0,
            vibrance: 0.0,
            shadow_tint: [1.0; 3],
            highlight_tint: [1.0; 3],
            lift: 0.0,
            foliage_hue: 0.0,
            sun: 1.0,
            sun_warmth: 0.0,
            sky_fill: 1.0,
            occlusion: 1.0,
            night_lift: 0.0,
            haze: 1.0,
            haze_color: [0.72, 0.77, 0.84],
            color_haze: 0.0,
            haze_distance: 1500.0,
            sky_tint: [0.55, 0.7, 0.95],
            sky_tint_amount: 0.0,
            sunbeams: 0.0,
            air: 1.0,
            mist: 0.0,
            bloom: Bloom::NATURAL.intensity,
            vignette: 0.0,
            grain: 0.0,
            sharpen: 0.0,
            aberration: 0.0,
            glare: 0.25,
            flare: 0.0,
            painterly: 0.0,
            outline: 0.0,
            texture_soften: 0.0,
            detail: 1.0,
            tone: ToneCurve::Filmic,
        }
    }
}

impl LookParams {
    pub fn lerp(&self, o: &LookParams, t: f32) -> LookParams {
        let l = |a: f32, b: f32| a + (b - a) * t;
        let l3 = |a: [f32; 3], b: [f32; 3]| [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])];
        LookParams {
            exposure: l(self.exposure, o.exposure),
            temperature: l(self.temperature, o.temperature),
            tint: l(self.tint, o.tint),
            saturation: l(self.saturation, o.saturation),
            contrast: l(self.contrast, o.contrast),
            vibrance: l(self.vibrance, o.vibrance),
            shadow_tint: l3(self.shadow_tint, o.shadow_tint),
            highlight_tint: l3(self.highlight_tint, o.highlight_tint),
            lift: l(self.lift, o.lift),
            foliage_hue: l(self.foliage_hue, o.foliage_hue),
            sun: l(self.sun, o.sun),
            sun_warmth: l(self.sun_warmth, o.sun_warmth),
            sky_fill: l(self.sky_fill, o.sky_fill),
            occlusion: l(self.occlusion, o.occlusion),
            night_lift: l(self.night_lift, o.night_lift),
            haze: l(self.haze, o.haze),
            haze_color: if self.color_haze <= 0.0 {
                o.haze_color
            } else {
                l3(self.haze_color, o.haze_color)
            },
            color_haze: l(self.color_haze, o.color_haze),
            haze_distance: l(self.haze_distance, o.haze_distance),
            sky_tint: if self.sky_tint_amount <= 0.0 {
                o.sky_tint
            } else {
                l3(self.sky_tint, o.sky_tint)
            },
            sky_tint_amount: l(self.sky_tint_amount, o.sky_tint_amount),
            sunbeams: l(self.sunbeams, o.sunbeams),
            air: l(self.air, o.air),
            mist: l(self.mist, o.mist),
            bloom: l(self.bloom, o.bloom),
            vignette: l(self.vignette, o.vignette),
            grain: l(self.grain, o.grain),
            sharpen: l(self.sharpen, o.sharpen),
            aberration: l(self.aberration, o.aberration),
            glare: l(self.glare, o.glare),
            flare: l(self.flare, o.flare),
            painterly: l(self.painterly, o.painterly),
            outline: l(self.outline, o.outline),
            texture_soften: l(self.texture_soften, o.texture_soften),
            detail: l(self.detail, o.detail),
            tone: if t >= 0.5 { o.tone } else { self.tone },
        }
    }

    /// Sun colour multiplier from `sun_warmth`.
    pub fn sun_color(&self) -> Vec3 {
        let w = self.sun_warmth.clamp(-1.0, 1.0);
        if w >= 0.0 {
            Vec3::new(1.0, 1.0 - 0.12 * w, 1.0 - 0.35 * w)
        } else {
            Vec3::new(1.0 + 0.25 * w, 1.0 + 0.06 * w, 1.0)
        }
    }

    /// Equal within slider precision (sliders round-trip floats with tiny noise).
    pub fn approx_eq(&self, o: &LookParams) -> bool {
        let a = serde_json::to_value(self).unwrap_or_default();
        let b = serde_json::to_value(o).unwrap_or_default();
        fn close(a: &serde_json::Value, b: &serde_json::Value) -> bool {
            match (a, b) {
                (serde_json::Value::Number(x), serde_json::Value::Number(y)) => {
                    (x.as_f64().unwrap_or(0.0) - y.as_f64().unwrap_or(0.0)).abs() < 1e-4
                }
                (serde_json::Value::Array(x), serde_json::Value::Array(y)) => {
                    x.len() == y.len() && x.iter().zip(y).all(|(a, b)| close(a, b))
                }
                (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
                    x.len() == y.len()
                        && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| close(v, w)))
                }
                _ => a == b,
            }
        }
        close(&a, &b)
    }

    /// This look inside a zone: an art style blended in by its weight (keeping this tone curve:
    /// swapping it would pop), then the zone's light on top.
    pub fn with_zone(&self, style: Option<(&LookParams, f32)>, z: &ZoneLight) -> LookParams {
        let mut p = match style {
            Some((s, w)) if w > 1e-3 => self.lerp(
                &LookParams {
                    tone: self.tone,
                    ..*s
                },
                w.min(1.0),
            ),
            _ => *self,
        };
        p.exposure += z.brightness;
        p.temperature += z.warmth;
        p.tint += z.tint;
        p.saturation *= z.saturation;
        p.contrast *= z.contrast;
        p.sun *= z.sun;
        p.sky_fill *= z.shadow_fill;
        p.bloom = (p.bloom + z.glow).min(1.0);
        p.vignette = p.vignette.max(z.vignette);
        p
    }

    fn needs_post_pass(&self, contrast: f32) -> bool {
        let tinted = |c: [f32; 3]| c.iter().any(|v| (v - 1.0).abs() > 1e-3);
        (contrast - 1.0).abs() > 1e-3
            || self.painterly > 1e-3
            || self.outline > 1e-3
            || self.grain > 1e-3
            || self.color_haze > 1e-3
            || self.sky_tint_amount > 1e-3
            || self.vibrance.abs() > 1e-3
            || self.lift.abs() > 1e-3
            || self.foliage_hue.abs() > 1e-3
            || tinted(self.shadow_tint)
            || tinted(self.highlight_tint)
    }
}

/// The world's look (saved with the world as part of [`Lighting`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub preset: LookPreset,
    /// 0 = Natural, 1 = the full look.
    pub strength: f32,
    /// Fine-tuned parameters; `None` uses the preset as designed.
    pub custom: Option<LookParams>,
    /// The user's saved style (`look_styles`) these parameters came from, by name.
    pub saved: Option<String>,
    /// The user's own LUT (Style › Fine-tune › Pro grading › LUT), applied after the grade. It
    /// stays when another style is picked; zones with their own art style fade it out.
    pub lut: Option<UserLut>,
    /// How much of `lut` shows (0..1).
    pub lut_strength: f32,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            preset: LookPreset::Natural,
            strength: 1.0,
            custom: None,
            saved: None,
            lut: None,
            lut_strength: 1.0,
        }
    }
}

impl Look {
    pub fn base(&self) -> LookParams {
        self.custom.unwrap_or_else(|| self.preset.params())
    }
    pub fn effective(&self) -> LookParams {
        LookParams::default().lerp(&self.base(), self.strength.clamp(0.0, 1.0))
    }
    pub fn select(&mut self, p: LookPreset) {
        self.preset = p;
        self.custom = None;
        self.saved = None;
    }
    /// The user LUT and how strongly it shows (the look's strength folded in).
    pub fn user_lut(&self) -> Option<(&Lut3, f32)> {
        self.lut.as_ref().map(|l| {
            (
                &*l.lut,
                (self.lut_strength * self.strength).clamp(0.0, 1.0),
            )
        })
    }
}

/// The look in effect this frame, for the systems that apply it.
#[derive(Resource, Default, Clone)]
pub struct LookState {
    /// The world's look with the viewer's zone blended in.
    pub params: LookParams,
    /// The viewer's zone light as blended this frame (already in `params`; `sky` adds its fog).
    pub zone: ZoneLight,
    /// The world/zone outdoor-haze result, faded with the rest of the zone contribution.
    pub haze: HazeState,
    /// Holding `\` shows Natural for a before/after comparison (the zone's light stays).
    pub comparing: bool,
    /// The user LUT and its strength this frame (faded out in zones with their own art style,
    /// off while comparing).
    pub lut: Option<(Arc<Lut3>, f32)>,
}

/// Seconds a zone's lighting & style takes to blend in or out at its border.
pub const ZONE_FADE: f32 = 3.0;

/// A zone's contribution to the look, as blended.
#[derive(Clone, Copy, Default)]
struct ZoneLook {
    light: ZoneLight,
    haze: HazeState,
    /// the zone's art style and its weight (0: the world's own)
    style: LookParams,
    style_w: f32,
}

impl ZoneLook {
    fn lerp(&self, o: &ZoneLook, t: f32) -> ZoneLook {
        // a style only fading in or out keeps its colours instead of morphing from Natural
        let style = if self.style_w < 1e-3 {
            o.style
        } else if o.style_w < 1e-3 {
            self.style
        } else {
            self.style.lerp(&o.style, t)
        };
        ZoneLook {
            light: self.light.lerp(&o.light, t),
            haze: self.haze.lerp(o.haze, t),
            style,
            style_w: self.style_w + (o.style_w - self.style_w) * t,
        }
    }
}

/// Crossfade between what the last zone gave and what the current one gives. The target is read
/// live, so edits to the zone's settings show at once when it has blended in.
#[derive(Default)]
struct ZoneFade {
    /// zone and biome the light comes from
    source: Option<(u8, u8)>,
    from: ZoneLook,
    current: ZoneLook,
    t: f32,
}

pub struct LookPlugin;

impl Plugin for LookPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((LookPostPlugin, crate::sun_shafts::SunShaftsPlugin))
            .init_resource::<LookState>()
            .add_systems(
                Update,
                (
                    update_look_state.before(crate::sky::SkySystems),
                    bake_grade_lut.after(update_look_state),
                    apply_camera_look.after(crate::sky::SkySystems),
                ),
            )
            .add_systems(PreUpdate, run_ui_calls);
    }
}

#[allow(clippy::too_many_arguments)]
fn update_look_state(
    lighting: Res<Lighting>,
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<crate::camera::Pointer>,
    zones: Res<Zones>,
    terrain: Option<Res<Terrain>>,
    sky: Res<SkyState>,
    interior: Option<Res<crate::dungeons::render::InteriorLook>>,
    time: Res<Time>,
    mut fade: Local<ZoneFade>,
    mut state: ResMut<LookState>,
) {
    let comparing = keys.pressed(KeyCode::Backslash) && !pointer.keyboard_captured;
    let world = if comparing {
        LookParams::default()
    } else {
        lighting.look.effective()
    };
    // a zone's own lighting & style wins; elsewhere the biome under the viewer sets the mood (a
    // dungeon's underground map has its own)
    let p = sky.viewer_pos;
    let inside = interior.is_some_and(|i| i.active);
    let (zone_id, biome_id) = match terrain.as_deref() {
        Some(t) if !inside => (t.zone_at(p.x, p.z), t.biome_at(p.x, p.z)),
        _ => (0, 0),
    };
    let painted_zone = zones.by_id(zone_id);
    let zone = painted_zone.filter(|z| z.mood_enabled);
    let biome_mood = crate::biomes::BiomePreset::from_id(biome_id)
        .map(|b| b.def().mood)
        .filter(|m| *m != Mood::Natural);
    let haze = haze::resolve(
        lighting.haze_enabled,
        lighting.haze,
        painted_zone.map(|z| (z.id, z.haze)),
    );
    let mut target = match (zone, biome_mood) {
        (Some(z), _) => {
            let style = z.own_style();
            ZoneLook {
                light: z.own_light().unwrap_or_default(),
                style: style.map(|(s, _)| s.params()).unwrap_or_default(),
                style_w: style.map_or(0.0, |(_, w)| w),
                ..default()
            }
        }
        (None, Some(m)) => ZoneLook {
            light: m.light(),
            ..default()
        },
        _ => ZoneLook::default(),
    };
    target.haze = haze;
    let source = (
        zone_id,
        if zone.is_some() {
            0
        } else {
            biome_mood.map_or(0, |_| biome_id)
        },
    );
    if fade.source.is_none() {
        // Haze must start resolved (a saved world with it off must not flash it on), while the
        // established zone light and style still fade from their neutral initial state.
        fade.from.haze = target.haze;
        fade.current.haze = target.haze;
        fade.t = 0.0;
        fade.source = Some(source);
    } else if fade.source != Some(source) {
        fade.from = fade.current;
        fade.t = 0.0;
        fade.source = Some(source);
    }
    fade.t = (fade.t + time.delta_secs() / ZONE_FADE).min(1.0);
    let k = crate::noise::smoothstep(0.0, 1.0, fade.t);
    fade.current = fade.from.lerp(&target, k);
    let z = fade.current;
    let params = world.with_zone(Some((&z.style, z.style_w)), &z.light);
    // a zone's own art style replaces the world's colours, its LUT included
    let lut = lighting
        .look
        .lut
        .as_ref()
        .filter(|_| !comparing)
        .map(|l| {
            let s = (lighting.look.lut_strength * lighting.look.strength).clamp(0.0, 1.0);
            (l.lut.clone(), s * (1.0 - z.style_w.clamp(0.0, 1.0)))
        })
        .filter(|(_, s)| *s > 1e-3);
    let lut_changed = match (&state.lut, &lut) {
        (None, None) => false,
        (Some((a, sa)), Some((b, sb))) => !Arc::ptr_eq(a, b) || (sa - sb).abs() > 1e-4,
        _ => true,
    };
    if state.params != params
        || state.comparing != comparing
        || state.zone != z.light
        || state.haze != z.haze
        || lut_changed
    {
        state.params = params;
        state.zone = z.light;
        state.haze = z.haze;
        state.comparing = comparing;
        state.lut = lut;
    }
}

/// The least sharpening under temporal anti-aliasing.
const TAA_SHARPEN: f32 = 0.3;

/// Tool calls the Style tab asks for that need the whole world (the round-trip capture takes a
/// screenshot): run at the start of the next frame, their outcome shown as a toast.
static UI_CALLS: std::sync::Mutex<Vec<(&'static str, serde_json::Value)>> =
    std::sync::Mutex::new(Vec::new());

pub fn ui_call(name: &'static str, args: serde_json::Value) {
    UI_CALLS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((name, args));
}

fn run_ui_calls(world: &mut World) {
    use crate::mcp_tools::{McpContent, McpDeferred};
    let calls = std::mem::take(&mut *UI_CALLS.lock().unwrap_or_else(|e| e.into_inner()));
    fn toast(w: &mut World, out: Result<Vec<McpContent>, String>) -> Option<McpDeferred> {
        let mut msg = None;
        let mut later = None;
        match out {
            Ok(v) => {
                for c in v {
                    match c {
                        McpContent::Text(t) => msg = Some(t),
                        McpContent::Deferred(d) => later = Some(d),
                        _ => {}
                    }
                }
            }
            Err(e) => msg = Some(e),
        }
        if let Some(m) = msg {
            w.resource_mut::<crate::editor::Editor>().toast(m);
        }
        later
    }
    for (name, args) in calls {
        let out = crate::mcp::call_tool(world, name, args);
        if let Some(McpDeferred(mut poll)) = toast(world, out) {
            let wrapped = McpContent::deferred(move |w| {
                let done = poll(w)?;
                let _ = toast(w, done);
                Some(Ok(vec![]))
            });
            if let McpContent::Deferred(d) = wrapped {
                crate::mcp::log_deferred(world, name, d);
            }
        }
    }
}

/// Bakes the grade (and the user LUT) into the style pass's 3D LUT when either changed.
fn bake_grade_lut(
    look: Res<LookState>,
    grade: Res<GradeLut>,
    mut images: ResMut<Assets<Image>>,
    mut last: Local<Option<([f32; 10], u64, f32)>>,
) {
    let (hash, strength) = look
        .lut
        .as_ref()
        .map_or((0, 0.0), |(l, s)| (Arc::as_ptr(l) as u64, *s));
    let key = (crate::grade::runtime_key(&look.params), hash, strength);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let t = std::time::Instant::now();
    let lut = crate::grade::bake_runtime(
        &look.params,
        look.lut.as_ref().map(|(l, s)| (&**l, *s)),
        crate::grade::RUNTIME_SIZE,
    );
    let _ = images.insert(grade.0.id(), crate::look_post::lut_image(&lut));
    debug!("grade LUT baked in {:.2} ms", t.elapsed().as_secs_f32() * 1000.0);
}

#[allow(clippy::type_complexity)]
fn apply_camera_look(
    mut commands: Commands,
    look: Res<LookState>,
    lighting: Res<crate::sky::Lighting>,
    sky: Res<SkyState>,
    time: Res<Time>,
    window: Query<&Window, With<PrimaryWindow>>,
    orbit: Res<crate::camera::OrbitCamera>,
    mut cams: Query<
        (
            Entity,
            &GlobalTransform,
            &Projection,
            &Camera,
            &mut Tonemapping,
            &mut Bloom,
            Option<&mut ContrastAdaptiveSharpening>,
            Option<&mut ChromaticAberration>,
            Option<&mut Vignette>,
            Option<&mut LookPost>,
            Has<TemporalAntiAliasing>,
        ),
        With<MainCamera>,
    >,
) {
    let p = &look.params;
    let window_scale = window.single().map(|w| w.scale_factor()).unwrap_or(1.0);
    for (e, gt, proj, cam, mut tone, mut bloom, cas, ca, vig, post, taa) in &mut cams {
        // pixels per point of the image the camera draws (below 100% render resolution fewer
        // than the window's: `render_scale`)
        let scale = cam.target_scaling_factor().unwrap_or(window_scale);
        let t = p.tone.tonemapping();
        if *tone != t {
            *tone = t;
        }
        if (bloom.intensity - p.bloom).abs() > 1e-4 {
            bloom.intensity = p.bloom;
        }
        // sharpening (temporal anti-aliasing always gets a little: it softens the image)
        let sharpen = if taa {
            p.sharpen.max(TAA_SHARPEN)
        } else {
            p.sharpen
        };
        match (sharpen > 0.01, cas) {
            (true, Some(mut c)) => {
                if (c.sharpening_strength - sharpen).abs() > 1e-4 {
                    c.sharpening_strength = sharpen;
                }
            }
            (true, None) => {
                commands.entity(e).insert(ContrastAdaptiveSharpening {
                    enabled: true,
                    sharpening_strength: sharpen,
                    denoise: false,
                });
            }
            (false, Some(_)) => {
                commands.entity(e).remove::<ContrastAdaptiveSharpening>();
            }
            (false, None) => {}
        }
        match (p.aberration > 1e-4, ca) {
            (true, Some(mut c)) => {
                if (c.intensity - p.aberration).abs() > 1e-5 {
                    c.intensity = p.aberration;
                }
            }
            (true, None) => {
                commands.entity(e).insert(ChromaticAberration {
                    intensity: p.aberration,
                    ..default()
                });
            }
            (false, Some(_)) => {
                commands.entity(e).remove::<ChromaticAberration>();
            }
            (false, None) => {}
        }
        match (p.vignette > 1e-3, vig) {
            (true, Some(mut v)) => {
                if (v.intensity - p.vignette).abs() > 1e-4 {
                    v.intensity = p.vignette;
                }
            }
            (true, None) => {
                commands.entity(e).insert(Vignette {
                    intensity: p.vignette,
                    radius: 0.9,
                    smoothness: 2.5,
                    ..default()
                });
            }
            (false, Some(_)) => {
                commands.entity(e).remove::<Vignette>();
            }
            (false, None) => {}
        }

        // style post pass (also carries the contrast)
        let contrast = p.contrast;
        let forest = &lighting.forest_contours;
        let forest_strength = if forest.enabled {
            forest.strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !p.needs_post_pass(contrast) && look.lut.is_none() && forest_strength <= 0.001 {
            if post.is_some() {
                commands.entity(e).remove::<LookPost>();
            }
            continue;
        }
        let (fov, near) = match proj {
            Projection::Perspective(pp) => (pp.fov, pp.near),
            _ => (0.9, 0.3),
        };
        let aspect = cam
            .logical_viewport_size()
            .map(|v| v.x / v.y.max(1.0))
            .or(window
                .single()
                .ok()
                .map(|w| w.width() / w.height().max(1.0)))
            .unwrap_or(1.6);
        let th = (fov * 0.5).tan();
        let srgb = |c: [f32; 3]| Color::srgb(c[0], c[1], c[2]).to_linear();
        // the colour haze dims with the light: full by day, a deep blue-grey at night
        let day = 1.0 - sky.night;
        let hz = srgb(p.haze_color);
        let night_haze = Vec3::new(0.035, 0.045, 0.075);
        let haze = Vec3::new(hz.red, hz.green, hz.blue).lerp(night_haze, sky.night)
            * (1.0 - sky.weather.dim * 0.35);
        let st = srgb(p.sky_tint);
        let painterly_px = (2.0 + 2.0 * p.painterly) * scale;
        let haze_start = if orbit.enabled {
            orbit.distance * 0.7
        } else {
            20.0
        };
        let u = LookPost {
            paint: Vec4::new(p.painterly, painterly_px, p.outline, p.grain),
            sky: Vec4::new(
                st.red,
                st.green,
                st.blue,
                p.sky_tint_amount * (0.35 + 0.65 * day),
            ),
            haze: haze.extend(p.color_haze * look.haze.strength),
            misc: Vec4::new(
                near,
                time.elapsed_secs() % 1000.0,
                0.0,
                p.haze_distance.max(10.0),
            ),
            cam_right: gt.right().as_vec3().extend(th * aspect),
            cam_up: gt.up().as_vec3().extend(th),
            cam_fwd: gt.forward().as_vec3().extend(scale),
            // haze starts behind what the camera looks at, so the editor overview stays clear
            extra: Vec4::new(haze_start, contrast, 0.0, 0.0),
            forest: Vec4::new(
                forest_strength,
                forest.start_distance.clamp(30.0, 1000.0),
                forest.start_distance.clamp(30.0, 1000.0)
                    + (forest.start_distance * 0.75).max(80.0),
                scale,
            ),
        };
        match post {
            Some(mut cur) => *cur = u,
            None => {
                commands.entity(e).insert(u);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Zone;

    #[test]
    fn zone_light_stacks_on_the_look_and_neutral_changes_nothing() {
        let world = LookPreset::Nordic.params();
        assert_eq!(world.with_zone(None, &ZoneLight::default()), world);
        assert_eq!(Mood::Natural.light(), ZoneLight::default());
        let dark = Mood::Dark.light();
        let p = world.with_zone(None, &dark);
        assert!(
            (p.exposure - (world.exposure + dark.brightness)).abs() < 1e-6
                && p.exposure < world.exposure
        );
        assert!((p.contrast - world.contrast * dark.contrast).abs() < 1e-6);
        assert!((p.sky_fill - world.sky_fill * dark.shadow_fill).abs() < 1e-6);
        assert!(p.vignette >= dark.vignette);
        // a style replaces the world's colours by its strength but keeps the world's tone curve
        let hell = LookPreset::Hellfire.params();
        let full = LookPreset::Natural
            .params()
            .with_zone(Some((&hell, 1.0)), &ZoneLight::default());
        assert!((full.temperature - hell.temperature).abs() < 1e-6);
        assert_eq!(full.tone, LookPreset::Natural.params().tone);
        let half = LookPreset::Natural
            .params()
            .with_zone(Some((&hell, 0.5)), &ZoneLight::default());
        assert!(half.temperature > 0.0 && half.temperature < hell.temperature);
    }

    #[test]
    fn a_style_fading_out_keeps_its_colours() {
        let a = ZoneLook {
            light: Mood::Warm.light(),
            style: LookPreset::Enchanted.params(),
            style_w: 1.0,
            ..default()
        };
        let mid = a.lerp(&ZoneLook::default(), 0.5);
        assert_eq!(mid.style, LookPreset::Enchanted.params());
        assert!((mid.style_w - 0.5).abs() < 1e-6);
        assert!((mid.light.warmth - Mood::Warm.light().warmth * 0.5).abs() < 1e-6);
    }

    #[test]
    fn old_zones_load_without_lighting_and_keep_their_mood() {
        let mut v = serde_json::to_value(Zone::new(1, "Old".into())).unwrap();
        for k in ["light", "style", "style_strength"] {
            v.as_object_mut().unwrap().remove(k);
        }
        v["mood_enabled"] = serde_json::json!(true);
        v["mood"] = serde_json::json!("Gloomy");
        let z: Zone = serde_json::from_value(v).unwrap();
        assert_eq!((z.light, z.style, z.style_strength), (None, None, 1.0));
        assert_eq!(z.own_light(), Some(Mood::Gloomy.light()));
        assert_eq!(z.own_style(), None);
    }
}
