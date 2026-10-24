//! The player character: the world's chosen character (`WorldCharacters::player`) walks for
//! you in Walk mode, seen from behind over its shoulder (P switches to first person and back).
//! It plays its moves from how you move: idle, walk, run, sprint, crouch, jump, swim; and acts
//! on the mouse and keys: left click a light attack, right click a heavy one, Q (held) block,
//! X a dodge roll. Walk mode itself (`crate::walk`) still moves and collides the explorer; this
//! only puts the character where it stands and the camera behind it.

use bevy::prelude::*;

use super::moves::{self, Move};
use super::puppet::{self, Puppet};
use super::{Characters, WorldCharacters, labs_enabled};
use crate::camera::MainCamera;
use crate::motion::movement::{Gait, InWater, MoveMode};
use crate::walk::WalkState;

/// How the player character is seen (not saved: first person is a key away).
#[derive(Resource)]
pub struct PlayerView {
    pub third_person: bool,
    /// camera distance behind (m)
    pub distance: f32,
}

impl Default for PlayerView {
    fn default() -> Self {
        Self {
            third_person: true,
            distance: 3.4,
        }
    }
}

#[derive(Default)]
struct Body {
    entity: Option<Entity>,
    /// the character and rig it was spawned as
    as_: Option<(String, String)>,
    yaw: f32,
    /// an action playing (attack, roll…) and whether it is held (block)
    action: Option<Move>,
    airborne: f32,
    landing: bool,
}

/// The player character's systems (creatures read its swings after them).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlayerSystems;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerView>().add_systems(
            Update,
            drive
                .in_set(PlayerSystems)
                .after(crate::walk::WalkSystems)
                .before(bevy::transform::TransformSystems::Propagate),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    walk: Res<WalkState>,
    world: Res<WorldCharacters>,
    chars: Res<Characters>,
    mut view: ResMut<PlayerView>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    terrain: Res<crate::terrain::Terrain>,
    time: Res<Time>,
    server: Res<AssetServer>,
    mut body: Local<Body>,
    mut puppets: Query<(&mut Puppet, &mut Transform, &mut Visibility), Without<MainCamera>>,
    mut cam: Query<&mut Transform, With<MainCamera>>,
    mut strikes: ResMut<super::creatures::Strikes>,
    mut commands: Commands,
) {
    let entry = world
        .player
        .as_ref()
        .filter(|_| labs_enabled() && walk.active && !walk.cinematic)
        .and_then(|id| chars.get(id))
        .filter(|e| e.rigged());
    let want = entry.map(|e| (e.meta.id.clone(), e.rig_key()));
    if want != body.as_ {
        if let Some(e) = body.entity.take() {
            commands.entity(e).despawn();
        }
        body.as_ = want.clone();
        body.action = None;
        if let Some(e) = entry {
            body.yaw = walk.yaw + std::f32::consts::PI;
            body.entity = puppet::spawn(
                &mut commands,
                &server,
                e,
                Transform::from_translation(walk.position),
                Some(moves::anim_for(&e.meta.moves, Move::Idle)),
                None,
            );
            if let Some(b) = body.entity {
                commands.entity(b).insert(Name::new("Player character"));
            }
        }
        return;
    }
    let (Some(e), Some(ent)) = (entry, body.entity) else {
        return;
    };
    if !walk.input_paused && keys.just_pressed(KeyCode::KeyP) {
        view.third_person = !view.third_person;
    }
    let Ok((mut p, mut tf, mut vis)) = puppets.get_mut(ent) else {
        return;
    };
    // no horse yet: riding is seen from the saddle
    let shown = view.third_person && walk.mode != MoveMode::Mount;
    *vis = if shown {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let dt = time.delta_secs().min(0.1);
    let v = walk.velocity;
    let ground = Vec2::new(v.x, v.z);
    let speed = ground.length();
    // face where it goes (or, standing, keep facing)
    if speed > 0.3 {
        let want = ground.x.atan2(ground.y);
        body.yaw = turn(body.yaw, want, dt * 10.0);
    }
    tf.translation = walk.position;
    tf.rotation = Quat::from_rotation_y(body.yaw);
    let anim = |m: Move| moves::anim_for(&e.meta.moves, m);

    // actions first (they play out, then it goes back to moving)
    let input = !walk.input_paused;
    let swimming = matches!(walk.water, InWater::Swimming | InWater::Diving);
    if input && !swimming && walk.mode == MoveMode::Foot {
        let pressed = if mouse.just_pressed(MouseButton::Left) {
            Some(Move::LightAttack)
        } else if mouse.just_pressed(MouseButton::Right) {
            Some(Move::HeavyAttack)
        } else if keys.just_pressed(KeyCode::KeyX) {
            Some(Move::Roll)
        } else {
            None
        };
        if let Some(m) = pressed {
            body.action = Some(m);
            p.speed = 1.0;
            p.restart(&anim(m), true);
            if m != Move::Roll {
                // (swings go where the camera looks)
                body.yaw = walk.yaw + std::f32::consts::PI;
                tf.rotation = Quat::from_rotation_y(body.yaw);
                strikes
                    .0
                    .push((walk.position, Vec2::new(body.yaw.sin(), body.yaw.cos())));
            }
        } else if keys.pressed(KeyCode::KeyQ) {
            if body.action != Some(Move::Block) {
                body.action = Some(Move::Block);
                p.speed = 1.0;
                p.play(&anim(Move::Block), false);
            }
        } else if body.action == Some(Move::Block) {
            body.action = None;
        }
    }
    if let Some(m) = body.action
        && m != Move::Block
    {
        if p.finished {
            body.action = None;
        }
    }
    if body.action.is_none() {
        // moving
        let air = !walk.grounded && walk.mode == MoveMode::Foot && !swimming;
        if air {
            body.airborne += dt;
        }
        let m = if swimming {
            if speed > 0.3 { Move::Swim } else { Move::SwimIdle }
        } else if walk.mode == MoveMode::Fly {
            Move::JumpAir
        } else if air {
            if v.y > 1.0 && body.airborne < 0.35 {
                Move::JumpStart
            } else {
                Move::JumpAir
            }
        } else if body.landing && !p.finished && speed < 1.0 {
            Move::JumpLand
        } else if speed < 0.25 {
            if walk.gait == Gait::Crouch { Move::Crouch } else { Move::Idle }
        } else {
            match walk.gait {
                Gait::Crouch => Move::CrouchWalk,
                Gait::Walk => Move::Walk,
                Gait::Sprint => Move::Sprint,
                _ => {
                    if speed < 2.6 {
                        Move::Walk
                    } else {
                        Move::Run
                    }
                }
            }
        };
        if !air && body.airborne > 0.45 {
            // just landed after a real fall
            body.landing = true;
            p.play(&anim(Move::JumpLand), true);
        } else if m != Move::JumpLand {
            body.landing = false;
            p.play(&anim(m), m.one_shot());
        }
        if !air {
            body.airborne = 0.0;
        }
        p.speed = match m.speed() {
            Some(clip) if speed > 0.2 => (speed / clip).clamp(0.6, 1.8),
            _ => 1.0,
        };
    }

    // the camera over its shoulder
    if !shown {
        return;
    }
    let Ok(mut c) = cam.single_mut() else {
        return;
    };
    let h = e.meta.height.clamp(0.4, 4.0);
    let look = Quat::from_euler(EulerRot::YXZ, walk.yaw, walk.pitch, 0.0);
    let pivot = walk.position + Vec3::Y * (h * 0.85);
    let right = look * Vec3::X;
    let back = look * Vec3::Z;
    let dist = view.distance * (h / 1.8).clamp(0.6, 1.6);
    let mut eye = pivot + back * dist + right * (0.35 * h / 1.8);
    // (never under the ground)
    let floor = terrain.height_at(eye.x, eye.z) + 0.35;
    if eye.y < floor {
        eye.y = floor;
    }
    c.translation = eye;
    c.rotation = look;
}

/// `from` turned towards `to` by at most `max` radians.
fn turn(from: f32, to: f32, max: f32) -> f32 {
    let mut d = (to - from) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    } else if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    from + d.clamp(-max, max)
}
