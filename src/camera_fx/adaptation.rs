//! Eye adaptation: the exposure follows what is on screen, round the procedural exposure.
//!
//! `sky::update_sun_moon` sets the exposure from the light (sun height, twilight, weather, the
//! look's and the slider's offsets, indoors, caves) and records the part that comes from the light
//! alone (`SkyState::scene_ev`). The meter (`meter`) gives the view's average scene luminance;
//! against `scene_ev` that says how much brighter or darker the view is than a well exposed view
//! in this light. Inside a dead band of ±0.6 EV nothing happens (the procedural exposure is the
//! anchor, and the view doesn't pump as the camera turns); beyond it the eye takes up most of the
//! difference, eased into its limits: opening up to +1.5 EV into the dark (less at night, when the
//! dark is the point), closing up to −1.25 EV into the light. It closes quickly (stepping out
//! into the sun dazzles for a moment) and opens slowly (a dark lane takes a couple of seconds to
//! see into).
//!
//! The look's and the slider's exposure offsets are kept: they are taken out before comparing and
//! stay in the final exposure. Indoors the procedural +1.9 EV and the eye share the work
//! (`INDOOR_SHARE`): the procedural part opens at once and the eye measures the room and opens
//! the rest of the way (or less, in a bright room with big windows), instead of both opening in
//! full. Caves (`sky::CAVE_EV`) and dungeons (`dungeons::render`) set their own exposure: there
//! the eye lets go (in a dungeon at once). The editor's orbit view stays fixed unless the setting
//! is Always (then it adapts at half strength).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::meter::{LOG2_1_2, MeterReading};

/// Lighting › Camera › Eye adaptation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EyeAdaptation {
    Off,
    /// Walking and camera paths; the editor view keeps the fixed exposure (lighting is judged there).
    #[default]
    Walking,
    /// Also the editor view (at half strength).
    Always,
}

impl EyeAdaptation {
    pub const ALL: [EyeAdaptation; 3] = [
        EyeAdaptation::Off,
        EyeAdaptation::Walking,
        EyeAdaptation::Always,
    ];
    pub fn name(self) -> &'static str {
        match self {
            EyeAdaptation::Off => "Off",
            EyeAdaptation::Walking => "Walking",
            EyeAdaptation::Always => "Always",
        }
    }
}

/// The share of the indoor exposure opening (`sky::INDOOR_EV`) the eye takes over at full strength.
pub const INDOOR_SHARE: f32 = 0.45;
/// log2 of the HDR brightness of a well exposed view: the trimmed, centre-weighted mean the meter
/// reads under the procedural exposure in daylight (measured: an open meadow at noon −2.0, a sunny
/// village street looking at a house −3.1, a backlit meadow at 17:00 −2.5).
const WELL_EXPOSED: f32 = -2.5;
/// A night view is meant to be darker than that (stops).
const NIGHT_DARKER: f32 = 1.5;
/// No adaptation within this many stops of well exposed.
const DEAD_BAND: f32 = 0.6;
/// The share of the difference beyond the dead band the eye takes up.
const RESPONSE: f32 = 0.8;
/// Limits (stops): opening into the dark, closing into the light.
const OPEN_MAX: f32 = 1.5;
const CLOSE_MAX: f32 = 1.25;
/// Rates (1/s, exponential): opening into the dark is slow, closing into the light quick.
const OPEN_RATE: f32 = 0.9;
const CLOSE_RATE: f32 = 3.0;

/// The eye's state; `sky::update_sun_moon` subtracts `open` from the exposure's EV.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct Adaptation {
    /// Stops the eye has opened (+, brighter) or closed (−) beyond the procedural exposure.
    pub open: f32,
    /// Where it is heading.
    pub target: f32,
    /// How much adaptation applies here (0 … 1): the setting and where the camera is.
    pub strength: f32,
    /// How much brighter (+) or darker (−) than well exposed the view reads (stops), if measured.
    pub deviation: Option<f32>,
}

impl Adaptation {
    /// One line for MCP replies and the panel.
    pub fn describe(&self, mode: EyeAdaptation) -> String {
        if mode == EyeAdaptation::Off {
            return "eye adaptation off".into();
        }
        if self.strength < 1e-3 && self.open.abs() < 0.01 {
            return format!("eye adaptation {} (idle here)", mode.name().to_lowercase());
        }
        let reads = self
            .deviation
            .map(|d| format!("; the view reads {d:+.1} EV against well exposed"))
            .unwrap_or_default();
        format!(
            "eye adaptation {} ({:+.2} EV now, heading for {:+.2}{reads})",
            mode.name().to_lowercase(),
            self.open,
            self.target
        )
    }
}

/// The eye's response to a view `x` stops darker (+) or brighter (−) than well exposed: nothing in
/// the dead band, then `RESPONSE` of the rest, eased (tanh) into the limits.
pub fn response(x: f32, open_max: f32, close_max: f32) -> f32 {
    let m = (x.abs() - DEAD_BAND).max(0.0) * RESPONSE;
    let lim = if x > 0.0 { open_max } else { close_max };
    if lim <= 1e-4 {
        0.0
    } else {
        x.signum() * lim * (m / lim).tanh()
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn adapt(
    lighting: Res<crate::sky::Lighting>,
    sky: Res<crate::sky::SkyState>,
    walk: Res<crate::walk::WalkState>,
    meter: Res<MeterReading>,
    indoors: Option<Res<crate::motion::indoors::Indoors>>,
    interior: Option<Res<crate::dungeons::render::InteriorLook>>,
    under: Query<Has<crate::motion::underwater::UnderwaterView>, With<crate::camera::MainCamera>>,
    time: Res<Time>,
    mut a: ResMut<Adaptation>,
) {
    let playing = walk.active || walk.cinematic;
    let mut strength = match lighting.eye_adaptation {
        EyeAdaptation::Off => 0.0,
        EyeAdaptation::Walking => {
            if playing {
                1.0
            } else {
                0.0
            }
        }
        EyeAdaptation::Always => {
            if playing {
                1.0
            } else {
                0.5
            }
        }
    };
    // a cave opens the exposure itself (eyes adjusting to the dark), a dungeon sets its own
    strength *= 1.0
        - indoors
            .as_ref()
            .map(|i| i.cave)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
    if under.iter().any(|u| u) {
        strength *= 0.5;
    }
    if interior.is_some_and(|i| i.active) {
        *a = Adaptation::default();
        return;
    }
    a.strength = strength;
    let night = sky.night.clamp(0.0, 1.0);
    a.deviation = meter
        .scene_log_lum
        .map(|l| l - sky.scene_ev - LOG2_1_2 - (WELL_EXPOSED - NIGHT_DARKER * night));
    a.target = match a.deviation {
        Some(d) if strength > 1e-3 => response(
            -d,
            OPEN_MAX * strength * (1.0 - 0.6 * night),
            CLOSE_MAX * strength,
        ),
        _ => 0.0,
    };
    let dt = time.delta_secs().min(0.1);
    let rate = if a.target > a.open {
        OPEN_RATE
    } else {
        CLOSE_RATE
    };
    a.open += (a.target - a.open) * (1.0 - (-rate * dt).exp());
    if a.target == 0.0 && a.open.abs() < 1e-3 {
        a.open = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_has_a_dead_band_and_limits() {
        assert_eq!(response(0.3, OPEN_MAX, CLOSE_MAX), 0.0);
        assert_eq!(response(-0.3, OPEN_MAX, CLOSE_MAX), 0.0);
        // a little darker: opens a little; much darker: opens up to (not past) the limit
        assert_eq!(response(0.55, OPEN_MAX, CLOSE_MAX), 0.0);
        let small = response(1.2, OPEN_MAX, CLOSE_MAX);
        assert!(small > 0.3 && small < 0.6, "{small}");
        let big = response(6.0, OPEN_MAX, CLOSE_MAX);
        assert!(big > 1.4 && big <= OPEN_MAX, "{big}");
        assert!(response(-6.0, OPEN_MAX, CLOSE_MAX) >= -CLOSE_MAX);
        assert!(response(-6.0, OPEN_MAX, CLOSE_MAX) < -1.1);
        // no strength: nothing
        assert_eq!(response(3.0, 0.0, 0.0), 0.0);
    }
}
