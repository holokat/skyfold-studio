//! A/B switch between the terrain shading (`terrain.wesl`) and its reference
//! (`terrain_reference.wesl`: the shading before its cost diet), for screenshots and frame times.
//! `WB_TERRAIN_REF=1` draws the whole run with the reference; a `WB_BENCH` phase whose entry
//! contains `terrain-ref` (e.g. `WB_BENCH="all;terrain-ref;all"`) draws that phase with it, so one
//! run measures both (the phase clock mirrors `scale::bench`: `WB_BENCH_START`, then
//! `WB_BENCH_SECS` + 1.5 s per phase). Automation only: normal runs don't even load the reference.
//! Likewise `WB_TERRAIN_NOBAKE=1` / a phase containing `terrain-nobake` evaluates the ground rules'
//! noise per pixel instead of reading the baked noise maps (`bake`).

use bevy::prelude::*;

use super::material::{REFERENCE_SHADERS, TerrainGpu, TerrainMaterial};

pub struct TerrainAbPlugin;

impl Plugin for TerrainAbPlugin {
    fn build(&self, app: &mut App) {
        let bench = std::env::var("WB_BENCH")
            .is_ok_and(|b| b.contains("terrain-ref") || b.contains("terrain-nobake"));
        if std::env::var("WB_TERRAIN_REF").is_ok()
            || std::env::var("WB_TERRAIN_NOBAKE").is_ok()
            || bench
        {
            app.add_systems(Startup, load_shaders)
                .add_systems(PostUpdate, apply.after(super::TerrainSystems::Gpu));
        }
    }
}

fn load_shaders(server: Res<AssetServer>) {
    let _ = REFERENCE_SHADERS.set((
        server.load("shaders/terrain.wesl"),
        server.load("shaders/terrain_reference.wesl"),
    ));
}

/// The `WB_BENCH` phase running at `now` (s since launch), if any.
fn bench_phase(now: f32) -> Option<String> {
    let spec = std::env::var("WB_BENCH").ok()?;
    let phases: Vec<&str> = spec
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let secs: f32 = std::env::var("WB_BENCH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6.0);
    let start: f32 = std::env::var("WB_BENCH_START")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25.0);
    if now < start {
        return None;
    }
    phases
        .get(((now - start) / (secs + 1.5)) as usize)
        .map(|p| p.to_string())
}

fn apply(
    time: Res<Time<Real>>,
    gpu: Option<Res<TerrainGpu>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut last: Local<Option<(bool, bool)>>,
) {
    let phase = bench_phase(time.elapsed_secs());
    let want = std::env::var("WB_TERRAIN_REF").is_ok()
        || phase.as_ref().is_some_and(|p| p.contains("terrain-ref"));
    let per_pixel = std::env::var("WB_TERRAIN_NOBAKE").is_ok()
        || phase.as_ref().is_some_and(|p| p.contains("terrain-nobake"));
    let Some(gpu) = gpu else { return };
    if materials
        .get(&gpu.material)
        .is_some_and(|m| m.extension.reference != want || m.extension.per_pixel_rules != per_pixel)
        && let Some(mut m) = materials.get_mut(&gpu.material)
    {
        m.extension.reference = want;
        m.extension.per_pixel_rules = per_pixel;
    }
    if *last != Some((want, per_pixel)) {
        let shading = match (want, per_pixel) {
            (true, _) => "reference (terrain_reference.wesl)",
            (false, true) => "current (terrain.wesl), rule noise per pixel",
            (false, false) => "current (terrain.wesl)",
        };
        info!("terrain shading: {shading}");
        *last = Some((want, per_pixel));
    }
}
