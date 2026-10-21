//! Rain on the ground and storms in the sky: how wet the world is (with a slow dry-out after the
//! rain stops), puddles filling the flats, splashes round the walker, and lightning.
//!
//! The weather (`sky::update_weather`) says how wet it *should* be; `RainState` follows it with
//! hysteresis, the way real ground does: surfaces soak in a minute of steady rain and take
//! minutes to dry, puddles fill only under steady rain and linger after it. Everything wet reads
//! these values:
//! - placed models (`models`): a CPU pass darkens porous materials and makes them glossy, from
//!   their cached dry look, only when the wetness has moved a step (never per frame);
//! - the terrain and roads: their wetness (`terrain.style2.y`) and the puddle / ripple channels
//!   (`terrain.overlay_image.yz`, `road.misc.zw`), shaded in `terrain.wesl` / `road.wesl` with
//!   `rain.wesl`'s puddle mask and drop ripples, all behind a uniform branch so a dry world pays
//!   nothing;
//! - splashes round the camera (`splash`);
//! - lightning in thunderstorms (and now and then in heavy rain): `lightning`.
//!
//! The world's settings (`RainSettings`: how puddly, how often lightning strikes) are saved with
//! the lighting. Everything is driven over MCP by `weather_set` (`wetness`, `puddles`,
//! `lightning`, `lightning_now`).

pub mod lightning;
pub mod models;
pub mod splash;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::noise::smoothstep;
use crate::sky::SkyState;

/// The world's rain look (saved with `Lighting`; old worlds load the defaults).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RainSettings {
    /// How much standing water rain leaves on the ground: 0 none … 1 a lot (the flats and the
    /// low spots of the ground that holds water: earth, mud, gravel, cobbles, flat rock).
    pub puddles: f32,
    /// How often lightning strikes in thunderstorms: 0 never … 1 normal (a few strikes a
    /// minute) … 3 a violent storm. Heavy rain sees the odd strike too.
    pub lightning: f32,
    /// Thunder after each strike (later and softer the further away it struck).
    pub thunder: bool,
}

impl Default for RainSettings {
    fn default() -> Self {
        Self {
            puddles: 0.6,
            lightning: 1.0,
            thunder: true,
        }
    }
}

/// How wet the world is now (follows the weather with hysteresis; see the module docs).
#[derive(Resource, Clone, Debug, Default)]
pub struct RainState {
    /// Wet surfaces: 0 dry … 1 soaked.
    pub surface: f32,
    /// Standing water: 0 none … 1 the puddles are full.
    pub puddles: f32,
    /// Rain falling here now (0..1): ripples on the puddles and splashes on the ground.
    pub rain: f32,
    /// MCP / tests: hold surfaces and puddles at this wetness (None: follow the weather).
    pub hold: Option<f32>,
}

impl RainState {
    /// Wetness for the GPU, in steps (every write of a terrain or road material rebuilds its
    /// bind group, so they move in 1/64 steps rather than every frame).
    pub fn surface_q(&self) -> f32 {
        quantize(self.surface)
    }
    /// Puddle fill × the world's puddle amount, in steps.
    pub fn puddles_q(&self, s: &RainSettings) -> f32 {
        quantize(self.puddles * s.puddles.clamp(0.0, 1.0))
    }
    /// Falling rain, in steps.
    pub fn rain_q(&self) -> f32 {
        quantize(self.rain)
    }
}

pub fn quantize(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) * 64.0).round() / 64.0
}

/// What a see-through rain effect binds to fog itself (`volumetrics::FroxelFog`: the view is
/// fogged before the transparent pass): the volume (None while the volumetric light is off) and
/// its params.
pub fn froxel(fog: Option<&crate::volumetrics::FroxelFog>) -> (Option<Handle<Image>>, Vec4) {
    match fog.filter(|f| f.params.w > 0.5) {
        Some(f) => (Some(f.image.clone()), f.params),
        None => (None, Vec4::ZERO),
    }
}

/// Time constants (s). Wetting runs faster the harder it rains (÷ (0.25 + rain)).
const WET_SECS: f32 = 18.0;
const DRY_SECS: f32 = 150.0;
const FILL_SECS: f32 = 40.0;
const DRAIN_SECS: f32 = 320.0;
/// The first seconds after start-up follow the weather at once (a world opened in the rain
/// starts wet, not drying out of a sunny default).
const PRIME_SECS: f32 = 8.0;

pub struct RainPlugin;

impl Plugin for RainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RainState>()
            .add_systems(Update, track.after(crate::sky::SkySystems));
        models::register(app);
        splash::register(app);
        lightning::register(app);
    }
}

/// Follows the weather's wetness: quickly up while it rains, slowly down after.
fn track(sky: Res<SkyState>, time: Res<Time>, mut st: ResMut<RainState>) {
    let dt = time.delta_secs().min(0.25);
    let target = sky.weather.wet.clamp(0.0, 1.0);
    // (`update_weather` already keeps both at 0 with rain and snow switched off)
    let rain = sky.weather.rain.clamp(0.0, 1.0);
    // (steady rain fills them; a drizzle only darkens the ground)
    let fill = smoothstep(0.2, 0.85, rain) * target;
    let priming = time.elapsed_secs() < PRIME_SECS;
    let follow = |cur: f32, to: f32, up: f32, down: f32| {
        let tau = if priming {
            0.4
        } else if to > cur {
            up
        } else {
            down
        };
        cur + (to - cur) * (1.0 - (-dt / tau).exp())
    };
    let s = &mut *st;
    s.rain = rain;
    if let Some(h) = s.hold {
        s.surface = h.clamp(0.0, 1.0);
        s.puddles = h.clamp(0.0, 1.0);
        return;
    }
    s.surface = follow(s.surface, target, WET_SECS / (0.25 + rain), DRY_SECS);
    s.puddles = follow(s.puddles, fill, FILL_SECS / (0.2 + rain), DRAIN_SECS);
    // (snap the tails: the GPU values are steps anyway)
    if s.surface < 1e-3 {
        s.surface = 0.0;
    }
    if s.puddles < 1e-3 {
        s.puddles = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantized_steps() {
        assert_eq!(quantize(0.0), 0.0);
        assert_eq!(quantize(1.2), 1.0);
        assert_eq!(quantize(0.5 + 0.001), 0.5);
    }

    #[test]
    fn settings_default_for_old_worlds() {
        let s: RainSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, RainSettings::default());
        let s: RainSettings = serde_json::from_str("{\"puddles\":0.2}").unwrap();
        assert!((s.puddles - 0.2).abs() < 1e-6 && s.thunder);
    }
}
