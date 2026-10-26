//! Sci-Fi looks (Style › Look › Sci-Fi): whole alien moods in one click. Each sets the colour
//! grade, the time of day, the clouds and the cloud sea, the bodies in the sky and the colour of
//! the plants; everything stays editable afterwards (Fine-tune, Lighting, Weather, Sky bodies,
//! Land › nature dials). The Sci-Fi world template starts from Alien Dusk.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::celestial::{BodyKind, SkyBodies, SkyBody, SkyPreset};
use crate::clouds::sea::CloudSea;
use crate::look::{LookParams, LookPreset};
use crate::nature::alien::{AlienFlora, FloraMode};
use crate::visuals::air::AirTheme;
use crate::sky::Lighting;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScifiLook {
    AlienDusk,
    EventHorizon,
    TwinSuns,
    Ashfall,
    FrozenMoon,
    Bioluminescent,
    ToxicBloom,
    RedPlanet,
    NebulaDrift,
    GiantDawn,
}

impl ScifiLook {
    pub const ALL: [ScifiLook; 10] = [
        ScifiLook::AlienDusk,
        ScifiLook::EventHorizon,
        ScifiLook::TwinSuns,
        ScifiLook::Ashfall,
        ScifiLook::FrozenMoon,
        ScifiLook::Bioluminescent,
        ScifiLook::ToxicBloom,
        ScifiLook::RedPlanet,
        ScifiLook::NebulaDrift,
        ScifiLook::GiantDawn,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ScifiLook::AlienDusk => "Alien Dusk",
            ScifiLook::EventHorizon => "Event Horizon",
            ScifiLook::TwinSuns => "Twin Suns",
            ScifiLook::Ashfall => "Ashfall",
            ScifiLook::FrozenMoon => "Frozen Moon",
            ScifiLook::Bioluminescent => "Bioluminescent",
            ScifiLook::ToxicBloom => "Toxic Bloom",
            ScifiLook::RedPlanet => "Red Planet",
            ScifiLook::NebulaDrift => "Nebula Drift",
            ScifiLook::GiantDawn => "Giant Dawn",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            ScifiLook::AlienDusk => "alien_dusk",
            ScifiLook::EventHorizon => "event_horizon",
            ScifiLook::TwinSuns => "twin_suns",
            ScifiLook::Ashfall => "ashfall",
            ScifiLook::FrozenMoon => "frozen_moon",
            ScifiLook::Bioluminescent => "bioluminescent",
            ScifiLook::ToxicBloom => "toxic_bloom",
            ScifiLook::RedPlanet => "red_planet",
            ScifiLook::NebulaDrift => "nebula_drift",
            ScifiLook::GiantDawn => "giant_dawn",
        }
    }

    pub fn from_id(id: &str) -> Option<ScifiLook> {
        ScifiLook::ALL.into_iter().find(|l| l.id() == id)
    }

    /// The card's subtitle.
    pub fn blurb(self) -> &'static str {
        match self {
            ScifiLook::AlienDusk => "ringed giant, cloud sea, crimson scrub",
            ScifiLook::EventHorizon => "a black hole fills the night",
            ScifiLook::TwinSuns => "two suns over amber desert",
            ScifiLook::Ashfall => "volcanic sky, grey ash flora",
            ScifiLook::FrozenMoon => "ice world, frost and cold light",
            ScifiLook::Bioluminescent => "glowing teal night under a giant",
            ScifiLook::ToxicBloom => "acid haze and lime growth",
            ScifiLook::RedPlanet => "butterscotch sky, rust dust",
            ScifiLook::NebulaDrift => "violet dusk, a crowded sky",
            ScifiLook::GiantDawn => "a banded giant at sunrise",
        }
    }

    /// The card picture (`assets/onboarding/<key>.jpg`, rendered by the engine).
    pub fn picture(self) -> String {
        format!("scifi_{}", self.id())
    }

    /// Everything the look sets: grade, time, clouds, sky bodies; and the plants' colour.
    pub fn apply(self, l: &mut Lighting, flora: &mut AlienFlora) {
        self.atmosphere(l);
        self.sky(l);
        *flora = self.flora();
    }

    /// Grade, time of day, clouds, haze and the cloud sea.
    pub fn atmosphere(self, l: &mut Lighting) {
        let s = self.settings();
        l.look.select(LookPreset::GoldenHour);
        l.look.custom = Some(s.grade);
        l.look.saved = None;
        l.hour = s.hour;
        l.sun_tilt = s.sun_tilt;
        l.haze_enabled = true;
        l.haze = s.haze;
        l.cloud_coverage = s.clouds[0];
        l.cloud_type = s.clouds[1];
        l.cloud_density = s.clouds[2];
        l.cirrus = s.clouds[3];
        l.stars = true;
        // no earthly rain or snow out of the biomes' weather (Weather turns it back on)
        l.precipitation = false;
        l.cloud_sea = s.sea;
        let (theme, density) = self.air();
        l.air.theme = Some(theme);
        l.air.density = density;
        l.scifi_look = Some(self);
    }

    /// The bodies in the sky, placed round the sun so they share the view with the sunset.
    pub fn sky(self, l: &mut Lighting) {
        let sun = sun_bearing(l);
        l.celestial = self.bodies(sun);
    }

    /// What the plants become: recoloured, burnt, frosted or glowing (`nature::alien`).
    pub fn flora(self) -> AlienFlora {
        use FloraMode::*;
        let (mode, hue, amount, saturation) = match self {
            ScifiLook::AlienDusk => (Recolour, 352.0, 1.0, 1.35),
            ScifiLook::EventHorizon => (Glow, 278.0, 1.0, 1.2),
            ScifiLook::TwinSuns => (Recolour, 36.0, 0.9, 1.1),
            ScifiLook::Ashfall => (Charred, 20.0, 1.0, 1.0),
            ScifiLook::FrozenMoon => (Frosted, 200.0, 0.9, 1.0),
            ScifiLook::Bioluminescent => (Glow, 178.0, 1.0, 1.6),
            ScifiLook::ToxicBloom => (Glow, 78.0, 1.0, 1.6),
            ScifiLook::RedPlanet => (Petrified, 20.0, 1.0, 1.0),
            ScifiLook::NebulaDrift => (Glow, 320.0, 1.0, 1.4),
            ScifiLook::GiantDawn => (Recolour, 178.0, 0.85, 1.0),
        };
        AlienFlora {
            enabled: true,
            mode,
            hue,
            amount,
            saturation,
        }
    }

    /// What drifts in the air round the explorer (`visuals::air`), and how thick.
    pub fn air(self) -> (AirTheme, f32) {
        match self {
            ScifiLook::AlienDusk => (AirTheme::RedDust, 1.2),
            ScifiLook::EventHorizon => (AirTheme::Stardust, 1.5),
            ScifiLook::TwinSuns => (AirTheme::MagicDust, 1.8),
            ScifiLook::Ashfall => (AirTheme::Ashfall, 2.6),
            ScifiLook::FrozenMoon => (AirTheme::IceGlitter, 1.8),
            ScifiLook::Bioluminescent => (AirTheme::Spores, 1.4),
            ScifiLook::ToxicBloom => (AirTheme::Toxic, 1.4),
            ScifiLook::RedPlanet => (AirTheme::RedDust, 2.6),
            ScifiLook::NebulaDrift => (AirTheme::Nebula, 1.8),
            ScifiLook::GiantDawn => (AirTheme::MagicDust, 1.5),
        }
    }

    /// The hour the look is designed for.
    pub fn hour(self) -> f32 {
        self.settings().hour
    }

    /// The colour grade as designed (the Style tab marks a look "edited" when it differs).
    pub fn settings_grade(self) -> LookParams {
        self.settings().grade
    }

    fn settings(self) -> Settings {
        let n = LookPreset::GoldenHour.params();
        let sea = |altitude: f32, thickness: f32, coverage: f32| CloudSea {
            enabled: true,
            altitude,
            thickness,
            coverage,
            softness: 0.3,
            density: 1.0,
        };
        let no_sea = CloudSea::default();
        match self {
            ScifiLook::AlienDusk => Settings {
                hour: 17.4,
                sun_tilt: 42.0,
                haze: 0.2,
                clouds: [0.22, 0.65, 0.9, 0.45],
                sea: sea(10.0, 110.0, 0.85),
                grade: LookParams {
                    exposure: 0.1,
                    temperature: 0.15,
                    tint: 0.06,
                    saturation: 1.1,
                    contrast: 1.16,
                    vibrance: 0.28,
                    shadow_tint: [0.86, 0.92, 1.14],
                    highlight_tint: [1.1, 0.98, 0.86],
                    lift: -0.01,
                    sun: 1.15,
                    sun_warmth: 0.5,
                    sky_fill: 1.05,
                    haze: 0.6,
                    haze_color: [0.96, 0.84, 0.8],
                    color_haze: 0.22,
                    haze_distance: 3200.0,
                    sky_tint: [0.62, 0.74, 1.0],
                    sky_tint_amount: 0.2,
                    sunbeams: 0.5,
                    air: 0.5,
                    mist: 0.1,
                    bloom: 0.45,
                    vignette: 0.22,
                    glare: 0.6,
                    flare: 0.2,
                    ..n
                },
            },
            // night under the black hole: its disc lights the scene's mood, the land kept
            // readable by a lifted, cool night
            ScifiLook::EventHorizon => Settings {
                hour: 22.6,
                sun_tilt: 40.0,
                haze: 0.15,
                clouds: [0.1, 0.5, 0.8, 0.2],
                sea: sea(10.0, 80.0, 0.75),
                grade: LookParams {
                    exposure: 0.25,
                    temperature: -0.1,
                    tint: 0.12,
                    saturation: 1.05,
                    contrast: 1.2,
                    vibrance: 0.2,
                    shadow_tint: [0.84, 0.88, 1.16],
                    highlight_tint: [1.12, 0.96, 0.84],
                    night_lift: 0.6,
                    haze: 0.4,
                    haze_color: [0.32, 0.3, 0.45],
                    color_haze: 0.25,
                    sky_tint: [0.5, 0.45, 0.8],
                    sky_tint_amount: 0.15,
                    bloom: 0.7,
                    vignette: 0.3,
                    glare: 0.4,
                    flare: 0.1,
                    sunbeams: 0.2,
                    air: 0.4,
                    mist: 0.15,
                    ..n
                },
            },
            ScifiLook::TwinSuns => Settings {
                hour: 16.4,
                sun_tilt: 34.0,
                haze: 0.35,
                clouds: [0.05, 0.4, 0.6, 0.3],
                sea: no_sea,
                grade: LookParams {
                    exposure: 0.15,
                    temperature: 0.45,
                    saturation: 1.12,
                    contrast: 1.12,
                    vibrance: 0.15,
                    shadow_tint: [0.92, 0.94, 1.08],
                    highlight_tint: [1.12, 1.0, 0.8],
                    sun: 1.25,
                    sun_warmth: 0.7,
                    haze: 0.7,
                    haze_color: [1.0, 0.82, 0.6],
                    color_haze: 0.3,
                    haze_distance: 3000.0,
                    sky_tint: [1.0, 0.86, 0.66],
                    sky_tint_amount: 0.18,
                    sunbeams: 0.6,
                    air: 0.6,
                    bloom: 0.5,
                    glare: 0.9,
                    flare: 0.35,
                    ..n
                },
            },
            ScifiLook::Ashfall => Settings {
                hour: 17.6,
                sun_tilt: 46.0,
                haze: 0.45,
                clouds: [0.48, 0.85, 1.2, 0.1],
                sea: no_sea,
                grade: LookParams {
                    exposure: 0.35,
                    temperature: 0.4,
                    tint: -0.05,
                    saturation: 0.85,
                    contrast: 1.25,
                    shadow_tint: [0.9, 0.86, 0.86],
                    highlight_tint: [1.2, 0.86, 0.62],
                    lift: -0.02,
                    sun: 1.0,
                    sun_warmth: 0.9,
                    sky_fill: 1.0,
                    haze: 1.2,
                    haze_color: [0.62, 0.42, 0.32],
                    color_haze: 0.45,
                    haze_distance: 1300.0,
                    sky_tint: [0.75, 0.42, 0.3],
                    sky_tint_amount: 0.4,
                    sunbeams: 0.4,
                    air: 1.4,
                    mist: 0.2,
                    bloom: 0.55,
                    vignette: 0.3,
                    glare: 0.5,
                    ..n
                },
            },
            ScifiLook::FrozenMoon => Settings {
                hour: 10.5,
                sun_tilt: 60.0,
                haze: 0.25,
                clouds: [0.25, 0.3, 0.8, 0.6],
                sea: sea(10.0, 140.0, 0.9),
                grade: LookParams {
                    exposure: 0.05,
                    temperature: -0.55,
                    tint: -0.04,
                    saturation: 0.88,
                    contrast: 1.12,
                    shadow_tint: [0.84, 0.94, 1.16],
                    highlight_tint: [1.0, 1.02, 1.06],
                    lift: 0.02,
                    sun: 1.05,
                    sun_warmth: -0.3,
                    sky_fill: 1.2,
                    haze: 1.0,
                    haze_color: [0.78, 0.86, 0.96],
                    color_haze: 0.35,
                    sky_tint: [0.7, 0.84, 1.0],
                    sky_tint_amount: 0.2,
                    sunbeams: 0.4,
                    air: 0.7,
                    mist: 0.2,
                    bloom: 0.4,
                    glare: 0.6,
                    ..n
                },
            },
            ScifiLook::Bioluminescent => Settings {
                hour: 23.4,
                sun_tilt: 40.0,
                haze: 0.25,
                clouds: [0.15, 0.4, 0.7, 0.2],
                sea: sea(8.0, 60.0, 0.7),
                grade: LookParams {
                    exposure: 0.3,
                    temperature: -0.3,
                    tint: 0.05,
                    saturation: 1.25,
                    contrast: 1.15,
                    vibrance: 0.35,
                    shadow_tint: [0.8, 0.95, 1.12],
                    highlight_tint: [0.95, 1.08, 1.1],
                    night_lift: 0.7,
                    haze: 0.5,
                    haze_color: [0.2, 0.45, 0.5],
                    color_haze: 0.35,
                    sky_tint: [0.35, 0.6, 0.75],
                    sky_tint_amount: 0.15,
                    mist: 0.35,
                    bloom: 0.8,
                    vignette: 0.3,
                    air: 0.6,
                    ..n
                },
            },
            ScifiLook::ToxicBloom => Settings {
                hour: 14.0,
                sun_tilt: 38.0,
                haze: 0.5,
                clouds: [0.35, 0.55, 1.1, 0.3],
                sea: sea(10.0, 90.0, 0.6),
                grade: LookParams {
                    temperature: 0.1,
                    tint: -0.35,
                    saturation: 1.1,
                    contrast: 1.15,
                    vibrance: 0.2,
                    shadow_tint: [0.9, 1.02, 0.9],
                    highlight_tint: [1.06, 1.08, 0.8],
                    sun: 0.95,
                    haze: 1.6,
                    haze_color: [0.72, 0.8, 0.42],
                    color_haze: 0.5,
                    haze_distance: 1600.0,
                    sky_tint: [0.75, 0.85, 0.5],
                    sky_tint_amount: 0.35,
                    air: 1.2,
                    mist: 0.3,
                    bloom: 0.35,
                    vignette: 0.25,
                    ..n
                },
            },
            ScifiLook::RedPlanet => Settings {
                hour: 15.2,
                sun_tilt: 44.0,
                haze: 0.45,
                clouds: [0.04, 0.3, 0.5, 0.2],
                sea: no_sea,
                grade: LookParams {
                    temperature: 0.35,
                    saturation: 1.05,
                    contrast: 1.12,
                    shadow_tint: [0.92, 0.9, 1.0],
                    highlight_tint: [1.1, 0.98, 0.86],
                    sun: 0.95,
                    sun_warmth: 0.3,
                    haze: 0.9,
                    haze_color: [0.82, 0.6, 0.45],
                    color_haze: 0.35,
                    haze_distance: 2600.0,
                    // dust lit by the sun turns the sky butterscotch
                    sky_tint: [0.92, 0.66, 0.46],
                    sky_tint_amount: 0.55,
                    air: 1.2,
                    bloom: 0.3,
                    glare: 0.5,
                    ..n
                },
            },
            ScifiLook::NebulaDrift => Settings {
                hour: 18.4,
                sun_tilt: 40.0,
                haze: 0.25,
                clouds: [0.2, 0.6, 0.8, 0.6],
                sea: sea(10.0, 100.0, 0.8),
                grade: LookParams {
                    exposure: 0.15,
                    temperature: -0.05,
                    tint: 0.35,
                    saturation: 1.15,
                    contrast: 1.12,
                    vibrance: 0.3,
                    shadow_tint: [0.92, 0.86, 1.16],
                    highlight_tint: [1.12, 0.94, 1.02],
                    night_lift: 0.4,
                    haze: 0.6,
                    haze_color: [0.8, 0.62, 0.9],
                    color_haze: 0.35,
                    sky_tint: [0.8, 0.6, 1.0],
                    sky_tint_amount: 0.3,
                    bloom: 0.6,
                    vignette: 0.25,
                    glare: 0.5,
                    air: 0.6,
                    ..n
                },
            },
            ScifiLook::GiantDawn => Settings {
                hour: 6.5,
                sun_tilt: 40.0,
                haze: 0.25,
                clouds: [0.2, 0.55, 0.85, 0.55],
                sea: sea(10.0, 130.0, 0.9),
                grade: LookParams {
                    exposure: 0.15,
                    temperature: 0.05,
                    tint: 0.05,
                    saturation: 1.08,
                    contrast: 1.1,
                    vibrance: 0.25,
                    shadow_tint: [0.86, 0.92, 1.14],
                    highlight_tint: [1.1, 0.98, 0.9],
                    sun_warmth: 0.4,
                    haze: 0.8,
                    haze_color: [0.95, 0.82, 0.8],
                    color_haze: 0.25,
                    sky_tint: [0.7, 0.8, 1.0],
                    sky_tint_amount: 0.15,
                    sunbeams: 0.6,
                    air: 0.7,
                    mist: 0.25,
                    bloom: 0.45,
                    glare: 0.7,
                    flare: 0.2,
                    ..n
                },
            },
        }
    }

    /// The sky's bodies, `sun` being the sun's compass bearing.
    fn bodies(self, sun: f32) -> SkyBodies {
        let at = |kind, db: f32, el: f32, size: f32| SkyBody {
            bearing: (sun + db).rem_euclid(360.0),
            elevation: el,
            size,
            ..SkyBody::preset(kind)
        };
        let bodies = match self {
            ScifiLook::AlienDusk => {
                let mut b = SkyPreset::RingedGiant.bodies(sun + 30.0);
                b[0].elevation = 9.0;
                b[1].bearing = (sun + 16.0).rem_euclid(360.0);
                b[1].elevation = 30.0;
                b
            }
            // the black hole fills the sky away from where the sun went down
            ScifiLook::EventHorizon => vec![at(BodyKind::BlackHole, 150.0, 4.0, 32.0)],
            ScifiLook::TwinSuns => vec![
                SkyBody {
                    color: [1.0, 0.86, 0.66],
                    color2: [1.0, 0.7, 0.4],
                    brightness: 1.6,
                    ..at(BodyKind::Star, -22.0, 13.0, 3.6)
                },
                at(BodyKind::Desert, 30.0, 14.0, 9.0),
            ],
            ScifiLook::Ashfall => vec![SkyBody {
                atmosphere: 0.5,
                ..at(BodyKind::Lava, 35.0, 9.0, 30.0)
            }],
            ScifiLook::FrozenMoon => vec![
                SkyBody {
                    ring: 0.35,
                    ring_open: 18.0,
                    ..at(BodyKind::Ice, 120.0, 22.0, 30.0)
                },
                at(BodyKind::Moon, 150.0, 40.0, 3.0),
            ],
            ScifiLook::Bioluminescent => vec![SkyBody {
                color: [0.42, 0.72, 0.82],
                color2: [0.2, 0.36, 0.55],
                atmosphere_color: [0.4, 0.9, 1.0],
                ..at(BodyKind::GasGiant, 160.0, 8.0, 46.0)
            }],
            ScifiLook::ToxicBloom => vec![SkyBody {
                color: [0.72, 0.78, 0.4],
                color2: [0.5, 0.56, 0.22],
                atmosphere_color: [0.8, 1.0, 0.4],
                ..at(BodyKind::GasGiant, 60.0, 18.0, 26.0)
            }],
            ScifiLook::RedPlanet => vec![
                SkyBody {
                    seed: 11,
                    color: [0.6, 0.52, 0.46],
                    ..at(BodyKind::Moon, 30.0, 16.0, 3.2)
                },
                SkyBody {
                    seed: 12,
                    color: [0.58, 0.5, 0.44],
                    ..at(BodyKind::Moon, 55.0, 9.0, 1.6)
                },
            ],
            ScifiLook::NebulaDrift => SkyPreset::Crowded.bodies(sun + 40.0),
            ScifiLook::GiantDawn => vec![SkyBody {
                ring: 0.85,
                ring_open: 12.0,
                ..at(BodyKind::GasGiant, 72.0, 12.0, 50.0)
            }],
        };
        SkyBodies {
            bodies,
            hide_moon: true,
        }
    }
}

struct Settings {
    hour: f32,
    sun_tilt: f32,
    haze: f32,
    /// coverage, type, density, cirrus
    clouds: [f32; 4],
    sea: CloudSea,
    grade: LookParams,
}

/// Where the sun stands at the world's hour (compass bearing).
fn sun_bearing(l: &Lighting) -> f32 {
    let d = crate::sky::sun_direction(l.hour, l.sun_tilt, l.sun_azimuth);
    crate::mcp_tools::dir_bearing(Vec2::new(d.x, d.z))
}

/// Puts a look on the open world (the Style tab and `look_scifi_set`).
pub fn apply(world: &mut World, look: ScifiLook) {
    let mut flora = world.resource::<crate::nature::NatureAppearance>().alien;
    look.apply(&mut world.resource_mut::<Lighting>(), &mut flora);
    world.resource_mut::<crate::nature::NatureAppearance>().alien = flora;
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_look_sets_a_sky_and_alien_plants() {
        for look in ScifiLook::ALL {
            let mut l = Lighting::default();
            let mut f = AlienFlora::default();
            look.apply(&mut l, &mut f);
            assert!(!l.celestial.bodies.is_empty(), "{}", look.name());
            assert!(f.enabled && l.scifi_look == Some(look) && l.air.theme.is_some());
            assert_eq!(ScifiLook::from_id(look.id()), Some(look));
            for b in &l.celestial.bodies {
                let mut c = b.clone();
                c.clamp();
                assert_eq!(&c, b, "{} has an out-of-range body", look.name());
            }
        }
    }
}
