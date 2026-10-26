//! The player character: the world's chosen character (`WorldCharacters::player`) walks for
//! you in Walk mode, seen the way the world's game camera frames it (`super::play_camera`:
//! over the shoulder, first person, a classic MMO chase camera, isometric or top-down).
//! It plays its moves from how you move: idle, walk, run, sprint, crouch, jump, swim; and acts
//! on the mouse and keys: left click a light attack, right click a heavy one, Q (held) block,
//! X a dodge roll (with a free pointer, attacks swing toward it). Walk mode itself
//! (`crate::walk`) still moves and collides the explorer; this only puts the character where it
//! stands.

use bevy::prelude::*;

use super::moves::{self, Move};
use super::puppet::{self, Puppet};
use super::{Characters, WorldCharacters, labs_enabled};
use super::play_camera::CameraStyle;
use crate::camera::{MainCamera, Pointer};
use crate::motion::movement::{Gait, InWater, MoveMode};
use crate::walk::WalkState;

/// How the player character is seen right now (not saved: the world's settings are
/// `WorldCharacters::camera`; `super::play_camera` keeps this).
#[derive(Resource)]
pub struct PlayerView {
    pub third_person: bool,
    /// camera distance (m, for a 1.8 m character; the wheel changes it when zooming is allowed)
    pub distance: f32,
    /// the style in effect
    pub style: CameraStyle,
    /// the camera is outside the character (it shows)
    pub seen: bool,
    /// the mouse pointer shows and aims (no mouse look)
    pub free_pointer: bool,
    /// the camera distance shown (eases toward `distance`)
    pub shown_distance: f32,
    /// classic MMO: the chase camera's yaw and how far it looks down (radians)
    pub orbit: Option<(f32, f32)>,
    /// the steadied height the chase and locked views look at
    pub pivot_y: Option<f32>,
    /// how far out the camera has room for (m, shown: walls and ceilings pull it in)
    pub room: f32,
    /// locked views under a roof: the cut's height, the roof's plan bounds and its top
    pub roof_cut: Option<(f32, Vec4, f32)>,
    /// the world's settings last applied
    pub applied: Option<super::play_camera::GameCamera>,
}

impl Default for PlayerView {
    fn default() -> Self {
        Self {
            third_person: true,
            distance: 3.4,
            style: CameraStyle::Shoulder,
            seen: false,
            free_pointer: false,
            shown_distance: 0.0,
            orbit: None,
            pivot_y: None,
            room: 0.0,
            roof_cut: None,
            applied: None,
        }
    }
}

/// The player character's puppet (weapons pose its arms).
#[derive(Component)]
pub struct PlayerBody;

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
    view: Res<PlayerView>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    pointer: Res<Pointer>,
    time: Res<Time>,
    server: Res<AssetServer>,
    mut body: Local<Body>,
    mut puppets: Query<(&mut Puppet, &mut Transform, &mut Visibility), Without<MainCamera>>,
    mut strikes: ResMut<super::creatures::Strikes>,
    armory: Res<super::weapons::Armory>,
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
                commands.entity(b).insert((Name::new("Player character"), PlayerBody));
            }
        }
        return;
    }
    let (Some(e), Some(ent)) = (entry, body.entity) else {
        return;
    };
    let Ok((mut p, mut tf, mut vis)) = puppets.get_mut(ent) else {
        return;
    };
    p.gaze = false;
    // (`play_camera` decides: riding is seen from the saddle, the locked views always see it)
    let shown = view.seen;
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
    // armed and aiming, drawing, firing or swinging: it turns to where the camera looks
    if armory.faces_aim() && !view.free_pointer {
        body.yaw = turn(body.yaw, walk.yaw + std::f32::consts::PI, dt * 14.0);
    }
    tf.translation = walk.position;
    tf.rotation = Quat::from_rotation_y(body.yaw);
    let anim = |m: Move| moves::anim_for(&e.meta.moves, m);

    // actions first (they play out, then it goes back to moving)
    let input = !walk.input_paused;
    // (a free pointer clicking the walk's HUD doesn't swing)
    let clicks = !(view.free_pointer && pointer.over_ui);
    let swimming = matches!(walk.water, InWater::Swimming | InWater::Diving);
    // (a weapon in hand takes the mouse: `weapons::drive`)
    if input && !swimming && walk.mode == MoveMode::Foot && !armory.takes_mouse() {
        let pressed = if clicks && mouse.just_pressed(MouseButton::Left) {
            Some(Move::LightAttack)
        } else if clicks && mouse.just_pressed(MouseButton::Right) {
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
                // (swings go where the camera looks, or toward a free pointer)
                body.yaw = view
                    .free_pointer
                    .then(|| aim(&pointer, walk.position))
                    .flatten()
                    .unwrap_or(walk.yaw + std::f32::consts::PI);
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
}

/// The facing toward where the pointer meets the ground at the character's feet.
fn aim(pointer: &Pointer, feet: Vec3) -> Option<f32> {
    let ray = pointer.ray?;
    let dy = ray.direction.y;
    if dy > -1e-4 {
        return None;
    }
    let at = ray.origin + *ray.direction * ((feet.y - ray.origin.y) / dy);
    let d = (at - feet).xz();
    (d.length() > 0.05).then(|| d.x.atan2(d.y))
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
