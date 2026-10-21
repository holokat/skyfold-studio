//! Measuring scale: frame rate and editing speed should depend on what is near the camera, not
//! on how big or dense the world is (procedural nature lives in `crate::nature`).
//!
//! - `bench`: the benchmark harness (`WB_BENCH` phases) and stress-world hooks
//!   (`WB_STRESS_AMOUNTS`, `WB_NATURE_COUNT`, `WB_WALK_LOG`).

pub mod bench;

use bevy::prelude::*;

pub struct ScalePlugin;

impl Plugin for ScalePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((bench::BenchPlugin, bench::StressPlugin));
    }
}
