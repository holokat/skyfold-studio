//! The equipped weapon each frame: equip and switch (number keys, MCP), input → `WState`, the
//! weapon's own clips, the character's upper-body clips and holds (third person), the view
//! model (first person) and the zoom.
//!
//! Input while walking: 1–4 pick a weapon set (the same key again, or 0, puts it away).
//! Blades: left click a light attack (clicks chain the combo), right click a heavy one.
//! Bows: hold the left button to draw, keep holding at full draw, let go to loose; hold the right
//! button to zoom while drawn. Guns: left click fires (the weapon comes up to the aim line and
//! stays up a moment), hold the right button to aim down the sights (narrower view; first person
//! puts the sight line on the eye), R reloads.

use std::collections::HashMap;

use autorig::Xform;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;

use super::hold::{AimFrame, Anchor, Hold, Side, grip_in_hand};
use super::{
    ActionKind, Armory, GUN_AIM_CLIP, GUN_RELOAD_CLIP, MeleeMove, UseCmd, Use, WState,
    WeaponAction, WeaponCatalog, WeaponDef, moveset,
};
use crate::camera::{MainCamera, Pointer};
use crate::characters::creatures::Strikes;
use crate::characters::player::{PlayerBody, PlayerView};
use crate::characters::puppet::Puppet;
use crate::characters::{AnimLibrary, Characters};
use crate::model_render::clips::{ClipMode, ModelClips};
use crate::walk::WalkState;

/// The equipped weapon's model (its root entity).
#[derive(Component)]
pub struct HeldWeapon {
    #[allow(dead_code)]
    pub id: String,
    /// socket name → node entity, once the scene is in
    sockets: HashMap<String, Entity>,
}

/// Per-frame memory.
#[derive(Default)]
pub struct Rt {
    entity: Option<Entity>,
    id: Option<String>,
    /// (character, its hand bone entity, which hand)
    hand: Option<(Entity, Entity, Side)>,
    grips: HashMap<(String, bool, bool), Option<Xform>>,
    /// the next light attack of the chain and how long since the last swing ended
    combo: usize,
    since_swing: f32,
    /// recoil left (1 just fired → 0)
    kick: f32,
    /// seconds since walking with a weapon began
    clock: f32,
}

fn approach(v: &mut f32, to: f32, rate: f32, dt: f32) {
    let step = rate * dt;
    *v = if to > *v { (*v + step).min(to) } else { (*v - step).max(to) };
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn drive(
    mut armory: ResMut<Armory>,
    cat: Res<WeaponCatalog>,
    walk: Res<WalkState>,
    view: Res<PlayerView>,
    (keys, mouse, pointer, time, server): (
        Res<ButtonInput<KeyCode>>,
        Res<ButtonInput<MouseButton>>,
        Res<Pointer>,
        Res<Time>,
        Res<AssetServer>,
    ),
    (mut chars, lib): (ResMut<Characters>, Res<AnimLibrary>),
    mut rt: Local<Rt>,
    mut cam: Query<(&mut Transform, &mut Projection), (With<MainCamera>, Without<HeldWeapon>)>,
    mut held: Query<
        (&mut HeldWeapon, &mut Transform, &mut ModelClips, Option<&ChildOf>),
        Without<MainCamera>,
    >,
    mut bodies: Query<
        (Entity, &mut Puppet, &Transform),
        (With<PlayerBody>, Without<MainCamera>, Without<HeldWeapon>),
    >,
    (children, names, globals): (Query<&Children>, Query<&Name>, Query<&GlobalTransform>),
    (mut actions, mut strikes): (MessageWriter<WeaponAction>, ResMut<Strikes>),
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.1);
    let active = walk.active && !walk.cinematic;
    let input = active && !walk.input_paused && !pointer.keyboard_captured;

    // weapon sets on the number keys
    if input {
        armory.default_sets(&cat);
        let keys_ = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4];
        for (i, k) in keys_.into_iter().enumerate() {
            if keys.just_pressed(k) {
                let want = armory.sets[i].clone();
                armory.equipped = if armory.equipped == want { None } else { want };
            }
        }
        if keys.just_pressed(KeyCode::Digit0) || keys.just_pressed(KeyCode::Backquote) {
            armory.equipped = None;
        }
    }

    // the model in hand follows what's equipped (and only exists while walking)
    let want = armory
        .equipped
        .clone()
        .filter(|id| active && cat.get(id).is_some());
    if want != rt.id {
        if let Some(e) = rt.entity.take() {
            commands.entity(e).despawn();
            if let Some(old) = rt.id.take() {
                send(&mut armory, &mut actions, &old, ActionKind::Unequip, walk.position, Vec3::ZERO, None, rt.clock);
            }
        }
        rt.id = want.clone();
        rt.hand = None;
        armory.state = WState::Holstered;
        armory.raise = 0.0;
        armory.ads = 0.0;
        armory.clip_asked = None;
        if armory.equipped.as_ref().is_some_and(|id| cat.get(id).is_none()) {
            armory.equipped = None;
        }
        if let Some(def) = want.as_ref().and_then(|id| cat.get(id)) {
            let scene = server.load(GltfAssetLabel::Scene(0).from_asset(def.gltf.clone()));
            let mut clips = ModelClips::new(&server, &def.gltf);
            if let Some(c) = &def.equip_clip {
                clips.play(c, ClipMode::Once);
            }
            let e = commands
                .spawn((
                    Name::new(format!("Weapon {}", def.name)),
                    HeldWeapon {
                        id: def.id.clone(),
                        sockets: HashMap::new(),
                    },
                    Transform::from_translation(walk.position),
                    Visibility::Inherited,
                    clips,
                    WorldAssetRoot(scene),
                ))
                .id();
            rt.entity = Some(e);
            armory.state = WState::Ready;
            send(&mut armory, &mut actions, &def.id, ActionKind::Equip, walk.position, Vec3::ZERO, None, rt.clock);
        }
    }
    let (Some(def), Some(we)) = (rt.id.as_ref().and_then(|id| cat.get(id)).cloned(), rt.entity)
    else {
        if let Ok((_, mut p, _)) = bodies.single_mut() {
            p.hold = None;
            p.set_upper(None, false);
        }
        armory.mcp_draw = false;
        armory.queue.clear();
        super::GUN_IN_HAND.store(false, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    super::GUN_IN_HAND.store(def.usage() == Use::Gun, std::sync::atomic::Ordering::Relaxed);
    let Ok((mut hw, mut wtf, mut clips, parent)) = held.get_mut(we) else {
        return;
    };
    rt.clock += dt;
    // the weapon's sockets, once its scene is in
    if hw.sockets.is_empty() {
        for d in children.iter_descendants(we) {
            if let Ok(n) = names.get(d)
                && def.sockets.iter().any(|s| s == n.as_str())
            {
                hw.sockets.insert(n.to_string(), d);
            }
        }
    }
    let root_g = globals.get(we).ok().copied();
    // a socket in the weapon's own space (live: a bow's grip_r rides the nock)
    let socket_local = |name: &str| -> Option<Vec3> {
        let s = globals.get(*hw.sockets.get(name)?).ok()?;
        let r = root_g?;
        Some(r.affine().inverse().transform_point3(s.translation()))
    };
    let socket_world = |name: Option<&str>| -> Option<Vec3> {
        globals.get(*hw.sockets.get(name?)?).ok().map(|g| g.translation())
    };

    // ------------------------------------------------------------------ input
    let clicks = input && !(view.free_pointer && pointer.over_ui);
    let (lmb, lmb_just, rmb, rmb_just) = (
        clicks && mouse.pressed(MouseButton::Left),
        clicks && mouse.just_pressed(MouseButton::Left),
        clicks && mouse.pressed(MouseButton::Right),
        clicks && mouse.just_pressed(MouseButton::Right),
    );
    let mut fire = false;
    let mut reload = input && keys.just_pressed(KeyCode::KeyR);
    let mut attack: Option<bool> = None;
    let u = def.usage();
    match u {
        Use::Melee if lmb_just => attack = Some(false),
        Use::Melee if rmb_just => attack = Some(true),
        Use::Gun if lmb_just => fire = true,
        _ => {}
    }
    for c in std::mem::take(&mut armory.queue) {
        match c {
            UseCmd::Attack { heavy } => attack = Some(heavy),
            UseCmd::Draw => armory.mcp_draw = true,
            UseCmd::Release => armory.mcp_draw = false,
            UseCmd::Fire => {
                if u == Use::Bow {
                    armory.mcp_draw = false;
                } else {
                    fire = true;
                }
            }
            UseCmd::Aim(on) => armory.mcp_aim = on,
            UseCmd::Reload => reload = true,
        }
    }
    let draw_held = lmb || armory.mcp_draw;
    armory.aiming = (rmb || armory.mcp_aim) && u != Use::Melee;

    // the camera's aim
    let (cam_pos, cam_rot) = cam
        .single()
        .map(|(t, _)| (t.translation, t.rotation))
        .unwrap_or((walk.position, Quat::IDENTITY));
    let aim_dir = cam_rot * Vec3::NEG_Z;
    armory.aim_dir = aim_dir;
    let chest = walk.position + Vec3::Y * 1.4;

    // ------------------------------------------------------------------ state
    let fire_len = def.clip_len(def.fire_clip.as_deref()).unwrap_or(0.3).max(0.12);
    armory.since_shot += dt;
    let mut state = armory.state.clone();
    match u {
        Use::Gun => {
            match &mut state {
                WState::Reloading { t, len } => {
                    *t += dt;
                    if *t >= *len {
                        state = WState::Ready;
                    }
                }
                WState::Firing { t } => {
                    *t += dt;
                    if *t >= fire_len {
                        state = WState::Ready;
                    }
                }
                _ => state = WState::Ready,
            }
            let can_fire = match state {
                WState::Ready => true,
                WState::Firing { t } => t >= fire_len * 0.85,
                _ => false,
            };
            if reload && !matches!(state, WState::Reloading { .. }) {
                if let Some(c) = &def.reload_clip {
                    let len = def.clip_len(Some(c)).unwrap_or(1.5).max(0.3);
                    clips.play(c, ClipMode::Once);
                    state = WState::Reloading { t: 0.0, len };
                    send(&mut armory, &mut actions, &def.id, ActionKind::Reload, chest, aim_dir, None, rt.clock);
                }
            } else if fire && can_fire {
                if let Some(c) = &def.fire_clip {
                    clips.play(c, ClipMode::Once);
                }
                state = WState::Firing { t: 0.0 };
                rt.kick = 1.0;
                armory.since_shot = 0.0;
                armory.shots += 1;
                let from = socket_world(def.muzzle.as_deref()).unwrap_or(chest);
                send(&mut armory, &mut actions, &def.id, ActionKind::Shot, from, aim_dir, None, rt.clock);
            }
        }
        Use::Bow => {
            let draw_len = def.clip_len(def.draw_clip.as_deref()).unwrap_or(0.9).max(0.1);
            let rel_len = def.clip_len(def.release_clip.as_deref()).unwrap_or(0.35).max(0.1);
            let mut loose: Option<f32> = None;
            match &mut state {
                WState::Drawing { t } => {
                    *t += dt;
                    if !draw_held {
                        loose = Some((*t / draw_len).min(1.0));
                    } else if *t >= draw_len {
                        state = WState::Drawn { t: 0.0 };
                        if let Some(c) = &def.hold_clip {
                            clips.play(c, ClipMode::Loop);
                        }
                    }
                }
                WState::Drawn { t } => {
                    *t += dt;
                    if !draw_held {
                        loose = Some(1.0);
                    }
                }
                WState::Releasing { t } => {
                    *t += dt;
                    if *t >= rel_len {
                        state = WState::Ready;
                    }
                }
                _ => {
                    state = WState::Ready;
                    if draw_held {
                        if let Some(c) = &def.draw_clip {
                            clips.play(c, ClipMode::Once);
                        }
                        state = WState::Drawing { t: 0.0 };
                    }
                }
            }
            if let Some(power) = loose {
                if let Some(c) = &def.release_clip {
                    clips.play(c, ClipMode::Once);
                }
                state = WState::Releasing { t: 0.0 };
                armory.arrows += 1;
                armory.mcp_draw = false;
                let from = socket_world(Some("nock")).unwrap_or(chest);
                send(&mut armory, &mut actions, &def.id, ActionKind::Arrow { draw: power }, from, aim_dir, None, rt.clock);
            }
        }
        Use::Melee => {
            let set = moveset(u);
            rt.since_swing += dt;
            let start_move = |mv: &MeleeMove, combo: usize, p: Option<&mut Puppet>| {
                if let Some(p) = p {
                    p.restart_upper(mv.clip, true);
                    if let Some(l) = p.upper.as_mut() {
                        l.speed = mv.speed;
                        l.weight = l.weight.max(0.6);
                        l.time = mv.start * clip_seconds(&lib, mv.clip);
                    }
                }
                WState::Swing {
                    mv: mv.clone(),
                    combo,
                    t: mv.start * clip_seconds(&lib, mv.clip),
                    sent: 0,
                    queued: None,
                }
            };
            let pick = |heavy: bool, combo: usize| -> Option<(MeleeMove, usize)> {
                if heavy {
                    set.heavy.clone().map(|m| (m, 0))
                } else if set.light.is_empty() {
                    None
                } else {
                    let i = combo % set.light.len();
                    Some((set.light[i].clone(), i))
                }
            };
            let mut next: Option<(MeleeMove, usize)> = None;
            match &mut state {
                WState::Swing { mv, combo, t, sent, queued } => {
                    *t += dt * mv.speed;
                    let len = clip_seconds(&lib, mv.clip);
                    let frac = *t / len;
                    if let Some(h) = attack {
                        *queued = Some(h);
                    }
                    while *sent < mv.hits.len() && frac >= mv.hits[*sent] {
                        *sent += 1;
                        armory.swings += 1;
                        let facing = Vec2::new(aim_dir.x, aim_dir.z).normalize_or(Vec2::Y);
                        strikes.0.push((walk.position, facing));
                        let base = socket_world(Some("trail_base")).unwrap_or(chest);
                        let tip = socket_world(Some("trail_tip"));
                        let (heavy, c) = (mv.heavy, *combo);
                        send(&mut armory, &mut actions, &def.id, ActionKind::Swing { heavy, combo: c }, base, aim_dir, tip, rt.clock);
                    }
                    if let Some(h) = *queued
                        && frac >= mv.cancel
                    {
                        next = pick(h, if h { 0 } else { *combo + 1 });
                    } else if frac >= mv.end {
                        rt.combo = if mv.heavy { 0 } else { *combo + 1 };
                        rt.since_swing = 0.0;
                        state = WState::Ready;
                    }
                }
                _ => {
                    state = WState::Ready;
                    if let Some(h) = attack {
                        if rt.since_swing > 0.8 {
                            rt.combo = 0;
                        }
                        next = pick(h, rt.combo);
                    }
                }
            }
            if let Some((mv, c)) = next {
                let mut p = bodies.single_mut().ok().map(|(_, p, _)| p);
                state = start_move(&mv, c, p.as_deref_mut());
            }
        }
    }
    armory.state = state.clone();

    // how raised the weapon is and how far down the sights
    let (raise_to, ads_to) = match (u, &state) {
        (Use::Gun, WState::Reloading { .. }) => (0.0, 0.0),
        (Use::Gun, _) => {
            let up = armory.aiming || armory.since_shot < 1.4;
            (up as u8 as f32, (armory.aiming && def.ads) as u8 as f32)
        }
        (Use::Bow, WState::Drawing { .. } | WState::Drawn { .. } | WState::Releasing { .. }) => {
            (1.0, (armory.aiming && !matches!(state, WState::Releasing { .. })) as u8 as f32)
        }
        _ => (0.0, 0.0),
    };
    let mut raise = armory.raise;
    let mut ads = armory.ads;
    let rate = if raise_to > raise { 7.0 } else { 3.0 };
    approach(&mut raise, raise_to, rate, dt);
    approach(&mut ads, ads_to, 5.0, dt);
    armory.raise = raise;
    armory.ads = ads;
    rt.kick *= (-dt * 12.0).exp();
    let kick = rt.kick;

    // ------------------------------------------------------------------ third person: the body
    let tweak = Xform {
        t: armory.grip_offset,
        r: Quat::from_euler(
            EulerRot::XYZ,
            armory.grip_rot_deg.x.to_radians(),
            armory.grip_rot_deg.y.to_radians(),
            armory.grip_rot_deg.z.to_radians(),
        ),
        s: Vec3::ONE,
    };
    let side = def.main_side();
    let mut in_hand = false;
    if let Ok((be, mut p, btf)) = bodies.single_mut() {
        let grip = chars.get(&p.character).cloned().and_then(|entry| {
            let key = (p.rig_key.clone(), side == Side::Left, u == Use::Melee);
            if let Some(g) = rt.grips.get(&key) {
                return *g;
            }
            let g = chars
                .skeleton(&entry)
                .and_then(|sk| grip_in_hand(&sk, side, def.grip_style()));
            if g.is_none() {
                warn!("character {}: no finger bones to grip a weapon with", entry.meta.id);
            }
            rt.grips.insert(key, g);
            g
        });
        let grip = grip.map(|g| Xform::from_matrix(g.matrix() * tweak.matrix()));
        // the hand bone, once the character's scene is in
        if rt.hand.is_none_or(|(b, _, s)| b != be || s != side) {
            let want = side.bone("hand");
            rt.hand = children
                .iter_descendants(be)
                .find(|d| names.get(*d).is_ok_and(|n| n.as_str() == want))
                .map(|bone| (be, bone, side));
        }
        // upper-body clips
        let set = moveset(u);
        match (u, &state) {
            (Use::Melee, WState::Swing { .. }) => {}
            (Use::Gun, WState::Reloading { t, len }) => {
                if *t <= dt * 1.5 {
                    p.restart_upper(GUN_RELOAD_CLIP, true);
                    if let Some(l) = p.upper.as_mut() {
                        l.speed = clip_seconds(&lib, GUN_RELOAD_CLIP) / len.max(0.3);
                    }
                }
            }
            (Use::Gun, _) if raise > 0.3 => p.set_upper(Some(GUN_AIM_CLIP), false),
            _ => match set.ready {
                Some(c) => p.set_upper(Some(c), false),
                None => p.set_upper(None, false),
            },
        }
        // the hold: the arms on the aim line
        let root_rot = btf.rotation;
        let to_model = root_rot.inverse();
        let h = chars.get(&p.character).map_or(1.8, |e| e.meta.height.clamp(0.4, 4.0));
        let shoulder_w = btf.translation + root_rot * Vec3::new(side_x(side) * 0.19, 1.44 * h / 1.8, 0.0);
        let target = cam_pos + aim_dir * 40.0;
        let dir_m = to_model * (target - shoulder_w).normalize_or(aim_dir);
        let pitch = dir_m.y.clamp(-0.9, 0.9).asin();
        if let Some(grip) = grip {
            let support_socket = def.support.as_deref().and_then(&socket_local);
            let (aim, support, main_pole, support_pole) = match u {
                Use::Gun => {
                    let aim_socket = def.aim.as_deref().and_then(&socket_local).unwrap_or(Vec3::ZERO);
                    let f = AimFrame {
                        dir: dir_m,
                        anchor: Anchor::Shoulder(side),
                        reach: 0.44 - 0.14 * ads,
                        drop: 0.05 - 0.27 * ads,
                        side: -0.06 - 0.13 * ads,
                        socket: aim_socket * ads,
                        roll: 0.0,
                        kick: Vec3::new(0.0, 0.0, -0.035 * kick),
                        kick_pitch: 0.16 * kick,
                    };
                    let reloading = matches!(state, WState::Reloading { .. });
                    (
                        Some((f, raise)),
                        support_socket.map(|s| (s, if reloading { 0.0 } else { raise })),
                        None,
                        None,
                    )
                }
                Use::Bow => {
                    // low ready (the bow upright at the side, pointing ahead and down) → drawn
                    let low = Vec3::new(0.0, -0.55, 0.85).normalize();
                    let k = smooth(raise);
                    let f = AimFrame {
                        dir: low.lerp(dir_m, k).normalize_or(dir_m),
                        anchor: Anchor::Shoulder(side),
                        reach: 0.42 + 0.08 * k,
                        drop: 0.32 * (1.0 - k) - 0.03 * k,
                        side: 0.1 * (1.0 - k) - 0.04 * k,
                        socket: Vec3::ZERO,
                        roll: -0.35 * (1.0 - k) - 0.12 * k,
                        kick: Vec3::ZERO,
                        kick_pitch: 0.0,
                    };
                    (
                        Some((f, 1.0)),
                        support_socket.map(|s| (s, raise)),
                        None,
                        Some(Vec3::new(side_x(side.other()) * 1.0, 0.25, -0.8)),
                    )
                }
                Use::Melee => {
                    let w = match state {
                        WState::Swing { .. } => 0.0,
                        _ if def.hands >= 2 => 1.0,
                        _ => 0.0,
                    };
                    (None, support_socket.map(|s| (s, w)), None, None)
                }
            };
            p.hold = Some(Hold {
                main: side,
                grip,
                aim,
                support,
                pitch: pitch * 0.55 * raise,
                main_pole,
                support_pole,
            });
            if view.seen
                && let Some((_, bone, _)) = rt.hand
            {
                if parent.is_none_or(|c| c.parent() != bone) {
                    commands.entity(we).insert(ChildOf(bone));
                }
                *wtf = Transform {
                    translation: grip.t,
                    rotation: grip.r,
                    scale: Vec3::ONE,
                };
                in_hand = true;
            }
        } else {
            p.hold = None;
        }
    }

    // ------------------------------------------------------------------ first person: the view model
    if !in_hand {
        if parent.is_some() {
            commands.entity(we).remove::<ChildOf>();
        }
        let aim_local = def.aim.as_deref().and_then(&socket_local).unwrap_or(Vec3::new(0.0, 0.08, -0.06));
        let local = view_model(&def, &state, &lib, raise, ads, kick, aim_local);
        let tw = tweak.matrix();
        let local = local.to_matrix() * tw;
        let m = Mat4::from_rotation_translation(cam_rot, cam_pos) * local;
        let (_, r, t) = m.to_scale_rotation_translation();
        *wtf = Transform {
            translation: t,
            rotation: r,
            scale: Vec3::ONE,
        };
    }

    // ------------------------------------------------------------------ the zoom
    let zoom_w = match u {
        Use::Gun => ads,
        Use::Bow => ads,
        Use::Melee => 0.0,
    };
    if zoom_w > 1e-3
        && let Ok((mut ct, mut proj)) = cam.single_mut()
    {
        if let Projection::Perspective(pp) = &mut *proj {
            pp.fov *= 1.0 + (def.zoom() - 1.0) * zoom_w;
        }
        if view.seen {
            // over the shoulder the camera comes in toward it
            let (f, r) = (ct.rotation * Vec3::NEG_Z, ct.rotation * Vec3::X);
            ct.translation += (f * 1.2 + r * 0.25) * zoom_w;
        }
    }
    // a preview turn round the character
    if view.seen
        && armory.view_orbit_deg.abs() > 0.01
        && let Ok((mut ct, _)) = cam.single_mut()
    {
        let pivot = walk.position + Vec3::Y * 1.3;
        let q = Quat::from_rotation_y(armory.view_orbit_deg.to_radians());
        ct.translation = pivot + q * (ct.translation - pivot);
        ct.rotation = q * ct.rotation;
    }
}

fn side_x(s: Side) -> f32 {
    match s {
        Side::Right => -1.0,
        Side::Left => 1.0,
    }
}

/// A clip's length in the animation library (s).
fn clip_seconds(lib: &AnimLibrary, key: &str) -> f32 {
    lib.find(key).map(|a| a.seconds).filter(|s| *s > 0.05).unwrap_or(1.0)
}

#[allow(clippy::too_many_arguments)]
fn send(
    armory: &mut Armory,
    out: &mut MessageWriter<WeaponAction>,
    weapon: &str,
    kind: ActionKind,
    origin: Vec3,
    dir: Vec3,
    tip: Option<Vec3>,
    time: f32,
) {
    let a = WeaponAction {
        weapon: weapon.to_string(),
        kind,
        origin,
        dir,
        tip,
        time,
    };
    debug!("weapon action {a:?}");
    armory.log.push(a.clone());
    if armory.log.len() > 16 {
        armory.log.remove(0);
    }
    out.write(a);
}

/// The weapon in front of the first-person camera (camera space: −Z ahead, +Y up, +X right).
pub fn view_model(
    def: &WeaponDef,
    state: &WState,
    lib: &AnimLibrary,
    raise: f32,
    ads: f32,
    kick: f32,
    aim_local: Vec3,
) -> Transform {
    // the weapon's +Z (forward) ahead
    let base = Quat::from_rotation_y(std::f32::consts::PI);
    let cam_x = |a: f32| Quat::from_rotation_x(a);
    let cam_z = |a: f32| Quat::from_rotation_z(a);
    match def.usage() {
        Use::Gun => {
            let mut hip = Vec3::new(0.19, -0.18 - 0.04 * (1.0 - raise), -0.46);
            let mut rot = base * Quat::from_rotation_x(-0.14 * kick);
            if let WState::Reloading { t, len } = state {
                let k = (std::f32::consts::PI * (t / len).clamp(0.0, 1.0)).sin();
                hip.y -= 0.1 * k;
                rot = cam_z(0.5 * k) * cam_x(0.35 * k) * rot;
            }
            let sight = Vec3::new(0.0, 0.0, -0.3) - base * aim_local;
            let pos = hip.lerp(sight, ads) + base * Vec3::new(0.0, 0.0, -0.045 * kick);
            Transform::from_translation(pos).with_rotation(rot)
        }
        Use::Bow => {
            let ready = (Vec3::new(-0.2, -0.3, -0.62), cam_z(0.5));
            let drawn = (Vec3::new(-0.1, -0.07, -0.74), cam_z(0.22));
            let pos = ready.0.lerp(drawn.0, raise);
            let rot = ready.1.slerp(drawn.1, raise) * base;
            Transform::from_translation(pos).with_rotation(rot)
        }
        Use::Melee => {
            // ready: blade up and leaning in; a swing winds up high right, cuts down to the left
            let keys: [(f32, Vec3, f32, f32); 4] = [
                (0.0, Vec3::new(0.24, -0.32, -0.5), 0.95, 0.3),
                (0.3, Vec3::new(0.34, -0.06, -0.4), 1.5, -0.7),
                (0.6, Vec3::new(-0.3, -0.34, -0.52), 0.25, 1.25),
                (1.0, Vec3::new(0.24, -0.32, -0.5), 0.95, -0.5),
            ];
            let phase = match state {
                WState::Swing { mv, t, .. } => {
                    let frac = t / clip_seconds(lib, mv.clip);
                    let p = ((frac - mv.start) / (mv.end - mv.start)).clamp(0.0, 1.0);
                    if mv.heavy { (p * 2.0).fract() } else { p }
                }
                _ => 0.0,
            };
            let mut k = 0;
            while k + 2 < keys.len() && phase > keys[k + 1].0 {
                k += 1;
            }
            let (a, b) = (keys[k], keys[k + 1]);
            let s = smooth((phase - a.0) / (b.0 - a.0).max(1e-3));
            let pos = a.1.lerp(b.1, s);
            let pitch = a.2 + (b.2 - a.2) * s;
            let roll = a.3 + (b.3 - a.3) * s;
            Transform::from_translation(pos).with_rotation(cam_z(roll) * cam_x(pitch) * base)
        }
    }
}
