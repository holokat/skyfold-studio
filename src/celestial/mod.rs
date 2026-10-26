//! Sky bodies: planets, extra moons, distant stars and black holes hung in the sky far above the
//! world (Lighting › Sky bodies). They are art-directed rather than simulated: each one sits at a
//! compass bearing and an elevation with an angular size, lit by the world's own sun (so a planet
//! near the sun shows a crescent and the phases follow the time of day), seen through the
//! world's atmosphere (dimmer and warmer near the horizon, washed by the day sky, covered by the
//! clouds). Fantasy worlds have none; the Sci-Fi world template (`crate::world_kind`) adds a
//! ringed giant and a moon.
//!
//! * `render`: one inward dome drawn between the stars and the clouds (`celestial.wesl`).
//! * `mcp`: `sky_bodies_*` tools. `ui`: the Lighting tab section.

pub mod mcp;
mod render;
pub mod ui;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// How many bodies the sky shader draws at once.
pub const MAX_BODIES: usize = 8;

/// What kind of body it is: sets its surface, how it is lit and what it emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    /// A grey cratered moon with dark maria.
    #[default]
    Moon,
    /// Oceans, continents, ice caps and weather under a blue atmosphere.
    Terran,
    /// Banded clouds, storms and usually rings.
    GasGiant,
    /// Pale cracked ice.
    Ice,
    /// Black crust split by glowing lava.
    Lava,
    /// Rust-red dunes, canyons and craters.
    Desert,
    /// A distant sun: a glowing disc with a corona (it does not light the world).
    Star,
    /// A black hole: its shadow, a glowing accretion disc and the stars bent round it.
    BlackHole,
}

impl BodyKind {
    pub const ALL: [BodyKind; 8] = [
        BodyKind::Moon,
        BodyKind::Terran,
        BodyKind::GasGiant,
        BodyKind::Ice,
        BodyKind::Lava,
        BodyKind::Desert,
        BodyKind::Star,
        BodyKind::BlackHole,
    ];

    pub fn name(self) -> &'static str {
        match self {
            BodyKind::Moon => "Moon",
            BodyKind::Terran => "Earth-like planet",
            BodyKind::GasGiant => "Gas giant",
            BodyKind::Ice => "Ice world",
            BodyKind::Lava => "Lava world",
            BodyKind::Desert => "Desert world",
            BodyKind::Star => "Star",
            BodyKind::BlackHole => "Black hole",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            BodyKind::Moon => "moon",
            BodyKind::Terran => "terran",
            BodyKind::GasGiant => "gas_giant",
            BodyKind::Ice => "ice",
            BodyKind::Lava => "lava",
            BodyKind::Desert => "desert",
            BodyKind::Star => "star",
            BodyKind::BlackHole => "black_hole",
        }
    }

    pub fn from_id(id: &str) -> Option<BodyKind> {
        BodyKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// The shader's index for it (`celestial.wesl`).
    fn index(self) -> f32 {
        BodyKind::ALL.iter().position(|k| *k == self).unwrap_or(0) as f32
    }

    /// Glows by itself rather than being lit by the sun.
    pub fn emissive(self) -> bool {
        matches!(self, BodyKind::Star | BodyKind::BlackHole)
    }
}

/// One body in the sky.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkyBody {
    pub kind: BodyKind,
    pub name: String,
    pub enabled: bool,
    /// Compass bearing (0 north, 90 east).
    pub bearing: f32,
    /// Degrees above the horizon (negative: partly set behind it).
    pub elevation: f32,
    /// Angular diameter in degrees (the moon is about 1.8 here; a looming giant 30–60).
    pub size: f32,
    /// Lean of the body's spin axis (degrees): tilts its bands, caps and ring.
    pub tilt: f32,
    /// Turns the surface about its axis (degrees): another face of the same world.
    pub spin: f32,
    /// Surface colour (sun-lit albedo tint; for a star its glow, for a black hole its disc).
    pub color: [f32; 3],
    /// A second surface colour (bands, land, lava, the disc's outer glow).
    pub color2: [f32; 3],
    /// Surface variation: another seed gives another world of the same kind.
    pub seed: u32,
    /// How bright: albedo for lit bodies, emission for stars and black holes (1 = as designed).
    pub brightness: f32,
    /// Scattering atmosphere round the limb, 0..1.
    pub atmosphere: f32,
    pub atmosphere_color: [f32; 3],
    /// Ring opacity, 0 = no ring.
    pub ring: f32,
    /// Ring inner and outer radius in body radii.
    pub ring_inner: f32,
    pub ring_outer: f32,
    /// How open the ring is seen (degrees): 0 edge-on, 90 face-on.
    pub ring_open: f32,
    pub ring_color: [f32; 3],
    /// Slowly circles the sky with the stars over the day instead of staying put.
    pub follow_sky: bool,
}

impl Default for SkyBody {
    fn default() -> Self {
        SkyBody::preset(BodyKind::Moon)
    }
}

impl SkyBody {
    /// A body of this kind as designed (placed south-east, a third of the way up the sky).
    pub fn preset(kind: BodyKind) -> SkyBody {
        let base = SkyBody {
            kind,
            name: kind.name().to_string(),
            enabled: true,
            bearing: 140.0,
            elevation: 28.0,
            size: 8.0,
            tilt: 12.0,
            spin: 0.0,
            color: [0.72, 0.72, 0.74],
            color2: [0.42, 0.42, 0.45],
            seed: 1,
            brightness: 1.0,
            atmosphere: 0.0,
            atmosphere_color: [0.45, 0.65, 1.0],
            ring: 0.0,
            ring_inner: 1.35,
            ring_outer: 2.3,
            ring_open: 14.0,
            ring_color: [0.86, 0.82, 0.76],
            follow_sky: false,
        };
        match kind {
            BodyKind::Moon => SkyBody { size: 5.0, ..base },
            BodyKind::Terran => SkyBody {
                size: 14.0,
                color: [0.12, 0.26, 0.48],
                color2: [0.38, 0.42, 0.26],
                atmosphere: 0.8,
                atmosphere_color: [0.42, 0.62, 1.0],
                ..base
            },
            BodyKind::GasGiant => SkyBody {
                size: 30.0,
                tilt: 18.0,
                color: [0.86, 0.76, 0.62],
                color2: [0.62, 0.45, 0.32],
                atmosphere: 0.35,
                atmosphere_color: [0.9, 0.78, 0.62],
                ring: 0.75,
                ..base
            },
            BodyKind::Ice => SkyBody {
                size: 10.0,
                color: [0.82, 0.9, 0.96],
                color2: [0.5, 0.68, 0.82],
                atmosphere: 0.25,
                atmosphere_color: [0.6, 0.8, 1.0],
                ..base
            },
            BodyKind::Lava => SkyBody {
                size: 9.0,
                color: [0.16, 0.13, 0.12],
                color2: [1.0, 0.42, 0.1],
                atmosphere: 0.2,
                atmosphere_color: [1.0, 0.5, 0.25],
                ..base
            },
            BodyKind::Desert => SkyBody {
                size: 11.0,
                color: [0.72, 0.42, 0.26],
                color2: [0.5, 0.28, 0.18],
                atmosphere: 0.25,
                atmosphere_color: [0.95, 0.68, 0.5],
                ..base
            },
            BodyKind::Star => SkyBody {
                size: 1.2,
                elevation: 18.0,
                color: [1.0, 0.72, 0.5],
                color2: [1.0, 0.55, 0.3],
                ..base
            },
            // a black hole is seen huge: its shadow a third of the sky, its disc spanning the horizon
            BodyKind::BlackHole => SkyBody {
                size: 30.0,
                elevation: 4.0,
                tilt: 6.0,
                ring_outer: 3.2,
                color: [1.0, 0.78, 0.5],
                color2: [1.0, 0.36, 0.12],
                ring_open: 6.0,
                ..base
            },
        }
    }

    /// Direction towards the body's centre (at `hour` when it follows the sky).
    pub fn direction(&self, hour: f32) -> Vec3 {
        let b = if self.follow_sky {
            self.bearing + hour / 24.0 * 360.0
        } else {
            self.bearing
        };
        let (b, e) = (b.to_radians(), self.elevation.clamp(-30.0, 90.0).to_radians());
        Vec3::new(b.sin() * e.cos(), e.sin(), -b.cos() * e.cos()).normalize()
    }

    /// Keeps every value in the range the UI allows.
    pub fn clamp(&mut self) {
        self.bearing = self.bearing.rem_euclid(360.0);
        self.elevation = self.elevation.clamp(-30.0, 90.0);
        self.size = self.size.clamp(0.1, 80.0);
        self.tilt = self.tilt.clamp(-90.0, 90.0);
        self.spin = self.spin.rem_euclid(360.0);
        self.brightness = self.brightness.clamp(0.0, 10.0);
        self.atmosphere = self.atmosphere.clamp(0.0, 1.0);
        self.ring = self.ring.clamp(0.0, 1.0);
        self.ring_inner = self.ring_inner.clamp(1.05, 4.0);
        self.ring_outer = self.ring_outer.clamp(self.ring_inner + 0.05, 5.0);
        self.ring_open = self.ring_open.clamp(0.0, 90.0);
        for c in [
            &mut self.color,
            &mut self.color2,
            &mut self.atmosphere_color,
            &mut self.ring_color,
        ] {
            for v in c.iter_mut() {
                *v = v.clamp(0.0, 1.0);
            }
        }
    }
}

/// The world's sky bodies (saved with the world's lighting).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkyBodies {
    pub bodies: Vec<SkyBody>,
    /// Hide the world's own moon (an alien sky may have only its own).
    pub hide_moon: bool,
}

/// Ready-made skies (the UI's quick picks and `sky_bodies_preset`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyPreset {
    None,
    RingedGiant,
    TwinMoons,
    TwinSuns,
    BlackHole,
    Crowded,
}

impl SkyPreset {
    pub const ALL: [SkyPreset; 6] = [
        SkyPreset::None,
        SkyPreset::RingedGiant,
        SkyPreset::TwinMoons,
        SkyPreset::TwinSuns,
        SkyPreset::BlackHole,
        SkyPreset::Crowded,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SkyPreset::None => "Empty sky",
            SkyPreset::RingedGiant => "Ringed giant",
            SkyPreset::TwinMoons => "Two moons",
            SkyPreset::TwinSuns => "Two distant suns",
            SkyPreset::BlackHole => "Black hole",
            SkyPreset::Crowded => "Crowded sky",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SkyPreset::None => "none",
            SkyPreset::RingedGiant => "ringed_giant",
            SkyPreset::TwinMoons => "twin_moons",
            SkyPreset::TwinSuns => "twin_suns",
            SkyPreset::BlackHole => "black_hole",
            SkyPreset::Crowded => "crowded",
        }
    }

    pub fn from_id(id: &str) -> Option<SkyPreset> {
        SkyPreset::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The bodies, placed round `bearing` (where the camera looks, so the sky shows at once).
    pub fn bodies(self, bearing: f32) -> Vec<SkyBody> {
        let at = |kind, db: f32, el: f32, size: f32| SkyBody {
            bearing: (bearing + db).rem_euclid(360.0),
            elevation: el,
            size,
            ..SkyBody::preset(kind)
        };
        match self {
            SkyPreset::None => vec![],
            // the reference: a pale cratered giant with a thin ring looming over the horizon, a
            // small moon beside it
            SkyPreset::RingedGiant => vec![
                SkyBody {
                    name: "Giant".into(),
                    brightness: 1.6,
                    color: [0.74, 0.78, 0.84],
                    color2: [0.5, 0.54, 0.6],
                    atmosphere: 0.45,
                    atmosphere_color: [0.62, 0.74, 1.0],
                    ring: 0.55,
                    ring_inner: 1.5,
                    ring_outer: 2.25,
                    ring_open: 6.0,
                    tilt: -16.0,
                    ..at(BodyKind::Moon, 18.0, 24.0, 42.0)
                },
                SkyBody {
                    name: "Moon".into(),
                    seed: 7,
                    ..at(BodyKind::Moon, -14.0, 34.0, 4.2)
                },
            ],
            SkyPreset::TwinMoons => vec![
                at(BodyKind::Moon, -10.0, 30.0, 6.0),
                SkyBody {
                    seed: 4,
                    color: [0.78, 0.66, 0.56],
                    ..at(BodyKind::Moon, 14.0, 22.0, 3.4)
                },
            ],
            SkyPreset::TwinSuns => vec![
                at(BodyKind::Star, -8.0, 12.0, 1.4),
                SkyBody {
                    color: [0.62, 0.78, 1.0],
                    color2: [0.45, 0.6, 1.0],
                    brightness: 0.7,
                    ..at(BodyKind::Star, 10.0, 20.0, 0.8)
                },
            ],
            SkyPreset::BlackHole => vec![at(BodyKind::BlackHole, 0.0, 4.0, 30.0)],
            SkyPreset::Crowded => vec![
                at(BodyKind::GasGiant, 20.0, 26.0, 36.0),
                at(BodyKind::Terran, -24.0, 40.0, 7.0),
                at(BodyKind::Lava, -6.0, 16.0, 3.0),
                SkyBody {
                    seed: 9,
                    ..at(BodyKind::Moon, -40.0, 22.0, 2.5)
                },
            ],
        }
    }
}

pub struct CelestialPlugin;

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        render::build(app);
        mcp::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearing_and_elevation_point_the_right_way() {
        let mut b = SkyBody::preset(BodyKind::Moon);
        b.elevation = 0.0;
        b.bearing = 0.0;
        assert!(b.direction(12.0).abs_diff_eq(Vec3::NEG_Z, 1e-5));
        b.bearing = 90.0;
        assert!(b.direction(12.0).abs_diff_eq(Vec3::X, 1e-5));
        b.elevation = 90.0;
        assert!(b.direction(12.0).abs_diff_eq(Vec3::Y, 1e-5));
    }

    #[test]
    fn old_worlds_have_an_empty_sky() {
        let s: SkyBodies = serde_json::from_str("{}").unwrap();
        assert!(s.bodies.is_empty() && !s.hide_moon);
        // a body saved with only a kind fills in the rest
        let b: SkyBody = serde_json::from_str(r#"{"kind":"gas_giant"}"#).unwrap();
        assert_eq!(b.kind, BodyKind::GasGiant);
    }

    #[test]
    fn clamp_keeps_the_ring_outside_the_body() {
        let mut b = SkyBody {
            ring_inner: 0.2,
            ring_outer: 0.1,
            size: 500.0,
            ..default()
        };
        b.clamp();
        assert!(b.ring_inner >= 1.05 && b.ring_outer > b.ring_inner && b.size <= 80.0);
    }
}
