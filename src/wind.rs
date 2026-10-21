//! The wind actually blowing where the viewer is: the world's wind (speed, direction,
//! gustiness), overridden by the zone the camera is in, strengthened by storms. Trees, plants,
//! grass, cloth and smoke all read it, and share one travelling gust field so wind waves roll
//! across forests and meadows together.

use bevy::prelude::*;

use crate::objects::Wind;
use crate::sky::SkyState;
use crate::world::Zones;

/// Effective wind this frame.
#[derive(Resource, Clone, Copy)]
pub struct WindState {
    /// Unit direction (x, z).
    pub dir: Vec2,
    /// m/s
    pub speed: f32,
    pub gustiness: f32,
    /// How far the gust pattern has travelled (m): integrated so speed changes never jump.
    pub gust_offset: Vec2,
}

impl Default for WindState {
    fn default() -> Self {
        Self {
            dir: Vec2::new(0.8, 0.6),
            speed: 2.5,
            gustiness: 0.5,
            gust_offset: Vec2::ZERO,
        }
    }
}

impl WindState {
    /// Wind velocity (m/s, world xz) with slow gusts — for cloth and smoke.
    pub fn velocity(&self, t: f32) -> Vec3 {
        Vec3::new(self.dir.x, 0.0, self.dir.y) * self.speed * self.gust(t)
    }

    /// The slow gusts at time `t`: a factor on the speed around 1 (cloth, smoke and the wind heard).
    pub fn gust(&self, t: f32) -> f32 {
        1.0 + self.gustiness
            * (0.35 * (t * 0.37).sin() * (t * 0.13 + 1.3).sin() + 0.15 * (t * 1.7).sin())
    }
}

/// Beaufort-style label for a wind speed.
pub fn describe(speed: f32) -> &'static str {
    match speed {
        s if s < 0.5 => "Calm",
        s if s < 1.6 => "Light air",
        s if s < 3.4 => "Light breeze",
        s if s < 5.5 => "Gentle breeze",
        s if s < 8.0 => "Moderate breeze",
        s if s < 10.8 => "Fresh breeze",
        s if s < 13.9 => "Strong breeze",
        s if s < 17.2 => "Near gale",
        s if s < 20.8 => "Gale",
        _ => "Storm",
    }
}

/// Whether the world animates. Editing from the overview everything holds still — smoke, cloth,
/// waves, wind sway, drifting clouds — which is calmer to edit in and frees the CPU and GPU for
/// the editor. Walking the world (or with Preview motion on) everything moves.
#[derive(Resource, Default)]
pub struct Motion {
    /// "Preview motion" toggle: animate while editing too.
    pub preview: bool,
    /// Animating this frame.
    pub live: bool,
}

pub struct WindPlugin;

impl Plugin for WindPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WindState>()
            .init_resource::<Motion>()
            .add_systems(First, update_motion)
            .add_systems(Update, update_wind_state.after(crate::sky::SkySystems));
    }
}

fn update_motion(walk: Res<crate::walk::WalkState>, mut motion: ResMut<Motion>) {
    let live = walk.active || walk.cinematic || motion.preview;
    if motion.live != live {
        motion.live = live;
    }
}

fn update_wind_state(
    wind: Res<Wind>,
    zones: Res<Zones>,
    sky: Res<SkyState>,
    time: Res<Time>,
    motion: Res<Motion>,
    mut state: ResMut<WindState>,
) {
    // paused: the gusts stop travelling (foliage and grass read speed 0, see their updates), but
    // speed and direction still follow the settings, so the wind heard while editing does too
    let dt = time.delta_secs();
    let mut target = wind.speed;
    if let Some(z) = zones.by_id(sky.viewer_zone)
        && z.wind_enabled
    {
        target = z.wind_speed;
    }
    // weather adds its own wind
    let w = sky.weather;
    target = target * (1.0 + w.rain * 0.6) + w.dim * 5.0 * w.rain + w.dust * 9.0;
    let k = 1.0 - (-dt * 0.8).exp();
    state.speed += (target - state.speed) * k;
    let d = wind.dir.normalize_or(Vec2::X);
    state.dir = state.dir.lerp(d, k).normalize_or(d);
    state.gustiness = wind.gustiness;
    // gusts travel a little slower than the wind itself
    let travel = if motion.live {
        state.dir * state.speed.max(0.4) * 0.8 * dt
    } else {
        Vec2::ZERO
    };
    state.gust_offset += travel;
}
