//! Depth of field for cinematic shots: Bevy's bokeh depth of field (`DepthOfField`) on the main
//! camera, only while a camera path plays (or captures), or, with Always, in the editor view too
//! for stills; never while walking. It runs after temporal anti-aliasing and bloom (Bevy orders
//! it before tonemapping), so the two coexist.
//!
//! Focus: a distance (m), or automatic: what is at the centre of the view (the meter's median
//! depth over a small patch), racked over to smoothly as the shot moves (about a third of a
//! second). The blur follows the physical lens: the aperture in f-stops and a full-frame sensor
//! (24 mm high: at the editor's 50° field of view a 26 mm lens; Bevy's default Super 35 gave a
//! 20 mm lens and barely any blur), the focal length from the field of view, so a wide landscape
//! shot keeps the distance sharp and a close shot (or a narrow field of view) melts the background.

use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode as BevyDofMode};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::meter::MeterReading;
use crate::camera::MainCamera;

/// Lighting › Camera › Depth of field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DepthOfFieldMode {
    #[default]
    Off,
    /// While a camera path plays or captures.
    CameraPaths,
    /// Camera paths and the editor view (stills, screenshots); never while walking.
    Always,
}

impl DepthOfFieldMode {
    pub const ALL: [DepthOfFieldMode; 3] = [
        DepthOfFieldMode::Off,
        DepthOfFieldMode::CameraPaths,
        DepthOfFieldMode::Always,
    ];
    pub fn name(self) -> &'static str {
        match self {
            DepthOfFieldMode::Off => "Off",
            DepthOfFieldMode::CameraPaths => "Camera paths",
            DepthOfFieldMode::Always => "Always",
        }
    }
}

/// Depth of field settings (saved with the world, in `Lighting`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DepthOfFieldSettings {
    pub mode: DepthOfFieldMode,
    /// Focus distance (m); 0 = automatic (what is at the centre of the view).
    pub focus: f32,
    /// Aperture (f-stops): lower blurs more.
    pub f_stops: f32,
}

impl Default for DepthOfFieldSettings {
    fn default() -> Self {
        Self {
            mode: DepthOfFieldMode::Off,
            focus: 0.0,
            f_stops: 2.0,
        }
    }
}

/// Focus range (m).
const NEAREST: f32 = 0.4;
const FARTHEST: f32 = 20_000.0;
/// Sensor height (m): full frame.
const SENSOR_HEIGHT: f32 = 0.024;
/// How fast automatic focus racks to a new distance (1/s, in log distance).
const RACK: f32 = 3.0;

/// Whether depth of field shows now (never while walking, only in a camera path or the editor view).
pub fn active(s: &DepthOfFieldSettings, walk: &crate::walk::WalkState) -> bool {
    match s.mode {
        DepthOfFieldMode::Off => false,
        DepthOfFieldMode::CameraPaths => walk.cinematic,
        DepthOfFieldMode::Always => walk.cinematic || !walk.active,
    }
}

pub(super) fn update_dof(
    mut commands: Commands,
    lighting: Res<crate::sky::Lighting>,
    walk: Res<crate::walk::WalkState>,
    meter: Res<MeterReading>,
    orbit: Res<crate::camera::OrbitCamera>,
    time: Res<Time>,
    mut cams: Query<(Entity, Option<&mut DepthOfField>), With<MainCamera>>,
    mut focus: Local<Option<f32>>,
) {
    let s = &lighting.depth_of_field;
    let on = active(s, &walk);
    for (e, dof) in &mut cams {
        if !on {
            if dof.is_some() {
                commands.entity(e).remove::<DepthOfField>();
            }
            *focus = None;
            continue;
        }
        let want = if s.focus > 0.0 {
            s.focus
        } else {
            meter.centre_distance.unwrap_or(orbit.distance)
        }
        .clamp(NEAREST, FARTHEST);
        let f = match *focus {
            // a set distance holds exactly; auto focus racks over in log distance
            Some(cur) if s.focus <= 0.0 => (cur.ln()
                + (want.ln() - cur.ln()) * (1.0 - (-RACK * time.delta_secs().min(0.1)).exp()))
            .exp(),
            _ => want,
        };
        *focus = Some(f);
        let u = DepthOfField {
            mode: BevyDofMode::Bokeh,
            focal_distance: f,
            aperture_f_stops: s.f_stops.clamp(0.5, 32.0),
            sensor_height: SENSOR_HEIGHT,
            // (keeps the bokeh passes' cost bounded at 5K)
            max_circle_of_confusion_diameter: 48.0,
            max_depth: FARTHEST,
            ..default()
        };
        match dof {
            Some(mut d) => {
                if d.focal_distance != u.focal_distance || d.aperture_f_stops != u.aperture_f_stops
                {
                    *d = u;
                }
            }
            None => {
                commands.entity(e).insert(u);
            }
        }
    }
}
