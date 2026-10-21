//! Sounds of moving through water: a splash going in and under, strokes while swimming, and the
//! muffled sea while the camera is under water (the zone ambience ducks meanwhile, see audio.rs).

use bevy::audio::{AudioSink, AudioSinkPlayback, PlaybackMode, Volume};
use bevy::prelude::*;

use super::movement::InWater;
use crate::audio::AudioSettings;
use crate::walk::WalkState;

#[derive(Resource)]
struct Sfx {
    splash: Handle<AudioSource>,
    swim: Handle<AudioSource>,
    under: Handle<AudioSource>,
}

#[derive(Component)]
struct UnderwaterBed {
    current: f32,
}

pub struct SfxPlugin;

impl Plugin for SfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load)
            .add_systems(Update, play.after(crate::walk::WalkSystems));
    }
}

fn load(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(Sfx {
        splash: server.load("audio/motion/splash_0.mp3"),
        swim: server.load("audio/motion/swim_0.mp3"),
        under: server.load("audio/motion/underwater_0.mp3"),
    });
}

fn one_shot(commands: &mut Commands, h: &Handle<AudioSource>, vol: f32) {
    commands.spawn((
        AudioPlayer(h.clone()),
        PlaybackSettings::DESPAWN
            .with_volume(Volume::Linear(vol))
            .with_speed(0.9 + fastrand::f32() * 0.2),
    ));
}

#[allow(clippy::too_many_arguments)]
fn play(
    mut commands: Commands,
    sfx: Option<Res<Sfx>>,
    walk: Res<WalkState>,
    settings: Res<AudioSettings>,
    time: Res<Time>,
    mut bed: Query<(Entity, &mut UnderwaterBed, Option<&mut AudioSink>)>,
    mut stroke: Local<f32>,
    mut seen: Local<f32>,
) {
    let Some(sfx) = sfx else { return };
    let master = if settings.muted { 0.0 } else { settings.master };
    let dt = time.delta_secs();
    // the sea heard from under the surface
    let want = walk.submerged * master * settings.ambience * 0.9;
    match bed.single_mut() {
        Ok((e, mut b, sink)) => {
            b.current += (want - b.current).clamp(-dt * 1.5, dt * 1.5);
            if let Some(mut s) = sink {
                s.set_volume(Volume::Linear(b.current.max(0.0)));
            }
            if want <= 0.0 && b.current <= 0.001 {
                commands.entity(e).despawn();
            }
        }
        Err(_) if want > 0.0 => {
            commands.spawn((
                Name::new("Underwater ambience"),
                UnderwaterBed { current: 0.0 },
                AudioPlayer(sfx.under.clone()),
                PlaybackSettings {
                    mode: PlaybackMode::Loop,
                    volume: Volume::Linear(0.0),
                    ..default()
                },
            ));
        }
        Err(_) => {}
    }
    if !walk.active {
        return;
    }
    // one-shots from the last step (each step reported once)
    if let Some((out, t)) = &walk.last
        && *t != *seen
    {
        *seen = *t;
        if out.splash || out.dived {
            one_shot(&mut commands, &sfx.splash, master * 0.55);
        }
        if out.surfaced {
            one_shot(&mut commands, &sfx.splash, master * 0.25);
        }
    }
    // strokes while swimming along the surface
    let speed = walk.velocity.xz().length();
    if walk.water == InWater::Swimming && speed > 0.6 {
        *stroke += dt * speed / 2.6;
        if *stroke >= 1.0 {
            *stroke = 0.0;
            // (as loud as before the footsteps default went from 0.9 to 0.7)
            one_shot(&mut commands, &sfx.swim, master * settings.footsteps * 0.58);
        }
    } else {
        *stroke = 0.7;
    }
}
