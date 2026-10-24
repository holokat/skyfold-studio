//! The sky's lighting probe, filtered only when the sky changed. Bevy draws the atmosphere into
//! the main camera's environment map and filters it into the diffuse and specular maps the scene
//! is lit with (`lightprobe_radiance_map` / `lightprobe_irradiance_map`, about 1.5 ms a frame on
//! a Mac) every frame, though they only change with the sun and moon, the atmosphere and the
//! camera's height. On frames where none of those moved the filtering is skipped (the maps keep
//! last frame's result, which is the same), and done again at least once a second regardless.
//! `WB_PERF=probe` filters every frame (A/B tests).

use bevy::pbr::generate::{RenderEnvironmentMap, downsampling_system};
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::sync_world::RenderEntity;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::camera::MainCamera;

/// Whether this frame filters the probe (extracted to the render world).
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
struct ProbeFrame {
    skip: bool,
    camera: Option<Entity>,
}

#[derive(Default)]
struct Last {
    key: Vec<u32>,
    /// frames since the key last changed
    still: u32,
    since: f32,
}

/// Camera moves smaller than these (m) don't change the sky it sees.
const HEIGHT_STEP: f32 = 2.0;
const GROUND_STEP: f32 = 100.0;
/// Frames to keep filtering after a change (a new map's pipelines may still be compiling).
const SETTLE_FRAMES: u32 = 8;
/// Filter at least this often (s), changed or not.
const REFRESH: f32 = 1.0;

pub struct SkyProbePlugin;

impl Plugin for SkyProbePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ProbeFrame>()
            .add_plugins(ExtractResourcePlugin::<ProbeFrame>::default())
            .add_systems(PostUpdate, decide.after(TransformSystems::Propagate));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                skip_filtering
                    .after(RenderSystems::PrepareBindGroups)
                    .before(downsampling_system)
                    .before(RenderSystems::Render),
            );
        }
    }
}

#[allow(clippy::type_complexity)]
fn decide(
    mut frame: ResMut<ProbeFrame>,
    mut last: Local<Last>,
    time: Res<Time<Real>>,
    lights: Query<(&DirectionalLight, &GlobalTransform)>,
    atmosphere: Query<Ref<bevy::light::Atmosphere>>,
    cam: Query<
        (
            &GlobalTransform,
            Option<&bevy::pbr::AtmosphereSettings>,
            Option<&bevy::light::AtmosphereEnvironmentMapLight>,
            Option<&bevy::light::EnvironmentMapLight>,
            &RenderEntity,
        ),
        With<MainCamera>,
    >,
) {
    let Ok((tf, settings, probe, env, render)) = cam.single() else {
        *frame = ProbeFrame::default();
        return;
    };
    let p = tf.translation();
    let mut key: Vec<u32> = vec![
        (p.y / HEIGHT_STEP).round() as i32 as u32,
        (p.x / GROUND_STEP).round() as i32 as u32,
        (p.z / GROUND_STEP).round() as i32 as u32,
    ];
    let mut lit: Vec<[u32; 7]> = lights
        .iter()
        .map(|(l, t)| {
            let d = t.forward();
            let c = l.color.to_linear();
            [d.x, d.y, d.z, l.illuminance, c.red, c.green, c.blue].map(f32::to_bits)
        })
        .collect();
    lit.sort_unstable();
    key.extend(lit.into_iter().flatten());
    // (the atmosphere's settings, the probe's size and the maps it's filtered into: their
    // intensities are applied when lighting, not filtered in)
    if let Some(s) = settings {
        key.extend(
            [
                s.transmittance_lut_size.to_array(),
                s.multiscattering_lut_size.to_array(),
                s.sky_view_lut_size.to_array(),
            ]
            .into_iter()
            .flatten(),
        );
        key.extend(s.aerial_view_lut_size.to_array());
        key.extend([
            s.transmittance_lut_samples,
            s.multiscattering_lut_dirs,
            s.multiscattering_lut_samples,
            s.sky_view_lut_samples,
            s.aerial_view_lut_samples,
            s.aerial_view_lut_max_distance.to_bits(),
            s.sky_max_samples,
        ]);
    }
    let text = format!(
        "{:?}{:?}",
        probe.map(|p| p.size),
        env.map(|e| (e.diffuse_map.id(), e.specular_map.id()))
    );
    key.extend(text.bytes().map(u32::from));
    let changed = atmosphere.iter().any(|a| a.is_changed()) || key != last.key;
    if changed {
        last.key = key;
        last.still = 0;
    } else {
        last.still = last.still.saturating_add(1);
    }
    last.since += time.delta_secs();
    let skip = last.still > SETTLE_FRAMES
        && last.since < REFRESH
        && !std::env::var("WB_PERF").is_ok_and(|p| p.contains("probe"));
    if !skip {
        last.since = 0.0;
    }
    *frame = ProbeFrame {
        skip,
        camera: Some(render.id()),
    };
}

/// Without its `RenderEnvironmentMap` this frame, Bevy's downsampling and filtering pass over the
/// camera's probe (they run right after this); it is extracted again next frame.
fn skip_filtering(world: &mut World) {
    let Some(frame) = world.get_resource::<ProbeFrame>() else {
        return;
    };
    let (true, Some(cam)) = (frame.skip, frame.camera) else {
        return;
    };
    if let Ok(mut e) = world.get_entity_mut(cam) {
        e.remove::<RenderEnvironmentMap>();
    }
}
