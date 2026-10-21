//! Honest frame-rate numbers. Bevy's "smoothed" FPS diagnostic treats its smoothing factor as
//! seconds, so with the default (≈0.017) it is effectively unsmoothed below 60 fps: one slow
//! frame made the counter read 2. Here the frame rate is frames per second actually rendered over
//! the last second, and stutter is reported separately (worst frame, frame-time history).

use std::collections::VecDeque;

use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct FrameStats {
    /// (real time, frame time) of recent frames, oldest first (the last 3 s).
    pub frames: VecDeque<(f64, f32)>,
    /// Frames per second over the last second.
    pub fps: f32,
    /// Longest frame of the last 3 s (ms).
    pub worst_ms: f32,
    /// Median frame of the last 3 s (ms).
    pub median_ms: f32,
    /// Frames over 50 ms in the last 3 s.
    pub hitches: u32,
}

impl FrameStats {
    /// Frame times (ms) of the last `n` frames, oldest first.
    pub fn recent_ms(&self, n: usize) -> impl Iterator<Item = f32> + '_ {
        self.frames
            .iter()
            .skip(self.frames.len().saturating_sub(n))
            .map(|f| f.1 * 1000.0)
    }
}

fn record(time: Res<Time<Real>>, mut st: ResMut<FrameStats>) {
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    st.frames.push_back((now, dt));
    while st.frames.front().is_some_and(|f| now - f.0 > 3.0) {
        st.frames.pop_front();
    }
    let last_s: Vec<f32> = st
        .frames
        .iter()
        .filter(|f| now - f.0 <= 1.0)
        .map(|f| f.1)
        .collect();
    let span: f32 = last_s.iter().sum();
    st.fps = if span > 0.0 {
        last_s.len() as f32 / span
    } else {
        0.0
    };
    let mut ms: Vec<f32> = st.frames.iter().map(|f| f.1 * 1000.0).collect();
    st.worst_ms = ms.iter().copied().fold(0.0, f32::max);
    st.hitches = ms.iter().filter(|m| **m > 50.0).count() as u32;
    ms.sort_by(|a, b| a.total_cmp(b));
    st.median_ms = ms.get(ms.len() / 2).copied().unwrap_or(0.0);
}

pub struct FrameStatsPlugin;

impl Plugin for FrameStatsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameStats>().add_systems(First, record);
    }
}
