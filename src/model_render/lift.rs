//! Elevators of library and imported glTF models. A glTF node whose extras hold
//! `{"lift": {"travel": t, "radius": r, "shaft": s, "wait_s": w, "speed": v, "accel": a}}` is a lift car: its
//! origin is the centre of the car's floor at the bottom stop, and it rides `t` metres up its
//! parent's +Y to the top stop. `r` is the car's inner radius, `s` the shaft's (the walker can't
//! step into an empty shaft). Children of the car with `{"lift_door": {"slide": [x, y, z]}}` are
//! its doors (they slide by that offset, node space, to open; `"turn_deg": a` turns them about
//! their node's Y instead or as well: curved doors of a round car pivot on its axis); other nodes
//! of the model with `{"lift_door": {…, "level": 0 | 1}}` are the landing doors at the bottom or
//! top.
//!
//! While walking: step into the car and stand there `w` seconds, the doors close, the car rides
//! to the other stop (easing in and out at `a` m/s², at most `v` m/s) and opens; step out. A rider
//! is carried however fast the car goes (the Sky Spire's capsule rides at 150 m/s). Waiting at a
//! landing calls the car there. The car and its doors are never part of the model's walk-in
//! collider: the explorer stands on the car's floor through `LiftFloors` (`crate::walk`), kept
//! inside the car while it's shut or moving and out of the shaft while the car is elsewhere.
//!
//! Call buttons and floor displays: a node with `{"lift_button": {"level": 0 | 1}}` is a landing's
//! call button, one with `{"lift_button": {"car": true}}` (under the car) the car's ride button.
//! A model with buttons runs on them alone: E on a landing's button (looked at, or within about
//! 4 m) calls the car there; in the car, E rides it to the other stop. A node with
//! `{"lift_digit": {"place": "u" | "t" | "h", "digit": d}}` is one glyph of a floor display (units,
//! tens, hundreds): it shows while the car's floor has `d` in that place (no leading zeros), the
//! floor counted from the car's height with the lift's `"floors": [first, last]` (bottom and top
//! stops). Buttons and digits outside the car go with the model's (first) car.
//! Agents read and call lifts with the `lifts` MCP tool, and press buttons with `interact`. Sounds (CC0, `assets/audio/props/lift_*`):
//! the doors running, a chime on arrival, and rushing air while the car moves, its pitch and level
//! following the car's speed.

use bevy::audio::{AudioSink, AudioSinkPlayback, SpatialScale, Volume};
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use bevy_egui::EguiContexts;
use serde_json::{Value, json};

use crate::audio::AudioSettings;
use crate::camera::MainCamera;
use crate::mcp_tools::McpContent;
use crate::walk::WalkState;

/// Seconds the doors take to open or shut.
const DOOR_S: f32 = 1.1;
/// Car acceleration when the model doesn't say (m/s²).
const ACCEL: f32 = 0.7;
/// Waiting this long at the other landing calls the car (seconds).
const CALL_S: f32 = 0.8;
/// Feet within this of the car's floor count as standing in it (metres).
const ON_FLOOR: f32 = 0.7;
/// A landing's call button within this of the explorer's eye (metres) is in reach...
const BUTTON_REACH: f32 = 4.0;
/// ...looked at within this (radians), or else the nearest one in reach.
const BUTTON_AIM: f32 = 0.5;

/// A car's parameters from its node extras (model units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiftSpec {
    pub travel: f32,
    pub radius: f32,
    pub shaft: f32,
    pub wait: f32,
    pub speed: f32,
    /// m/s²
    pub accel: f32,
    /// the floor numbers of the bottom and top stops (the displays count between them)
    pub floors: Option<[u32; 2]>,
}

/// Reads a node's `lift` extras; None when it has none.
pub fn parse_lift(extras: &str) -> Option<LiftSpec> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let l = v.get("lift")?;
    let num = |k: &str, d: f64| l.get(k).and_then(|x| x.as_f64()).unwrap_or(d) as f32;
    let travel = num("travel", 0.0);
    if travel <= 0.1 {
        return None;
    }
    let radius = num("radius", 1.0).max(0.4);
    Some(LiftSpec {
        travel,
        radius,
        shaft: num("shaft", radius as f64 + 0.25).max(radius),
        wait: num("wait_s", 2.5).clamp(0.5, 30.0),
        speed: num("speed", 1.5).clamp(0.3, 200.0),
        accel: num("accel", ACCEL as f64).clamp(0.2, 50.0),
        floors: l.get("floors").and_then(|f| f.as_array()).and_then(|a| {
            Some([a.first()?.as_u64()?.min(999) as u32, a.get(1)?.as_u64()?.min(999) as u32])
        }),
    })
}

/// Reads a node's `lift_button` extras: Some(Some(level)) a landing's call button, Some(None) the
/// car's ride button.
pub fn parse_lift_button(extras: &str) -> Option<Option<u8>> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let b = v.get("lift_button")?;
    if b.get("car").and_then(|c| c.as_bool()) == Some(true) {
        return Some(None);
    }
    Some(Some(b.get("level")?.as_u64()?.min(1) as u8))
}

/// Reads a node's `lift_digit` extras: (place: 0 units, 1 tens, 2 hundreds; digit).
pub fn parse_lift_digit(extras: &str) -> Option<(u8, u8)> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let d = v.get("lift_digit")?;
    let place = match d.get("place")? {
        Value::String(p) => match p.as_str() {
            "u" => 0,
            "t" => 1,
            "h" => 2,
            _ => return None,
        },
        n => n.as_u64()?.min(2) as u8,
    };
    Some((place, d.get("digit")?.as_u64()?.min(9) as u8))
}

/// Does floor `n` show `digit` at `place` (no leading zeros)?
pub fn shows_digit(n: u32, place: u8, digit: u8) -> bool {
    let p = 10u32.pow(place as u32);
    (place == 0 || n >= p) && (n / p) % 10 == digit as u32
}

/// How a lift door opens: slide offset (node space) and turn about its node's Y (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DoorMove {
    pub slide: Vec3,
    pub turn: f32,
}

/// Reads a node's `lift_door` extras: (how it opens, landing level: None for the car's own doors).
pub fn parse_lift_door(extras: &str) -> Option<(DoorMove, Option<u8>)> {
    let v: serde_json::Value = serde_json::from_str(extras).ok()?;
    let d = v.get("lift_door")?;
    let slide = match d.get("slide").and_then(|a| a.as_array()) {
        Some(a) => Vec3::new(
            a.first()?.as_f64()? as f32,
            a.get(1)?.as_f64()? as f32,
            a.get(2)?.as_f64()? as f32,
        ),
        None => Vec3::ZERO,
    };
    let turn = (d.get("turn_deg").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32).to_radians();
    if slide == Vec3::ZERO && turn == 0.0 {
        return None;
    }
    let level = d
        .get("level")
        .and_then(|x| x.as_u64())
        .map(|l| l.min(1) as u8);
    Some((DoorMove { slide, turn }, level))
}

/// Is this node (and everything under it) left out of a model's collider?
pub fn not_solid(extras: &str) -> bool {
    parse_lift(extras).is_some() || parse_lift_door(extras).is_some()
}

/// A lift car.
#[derive(Component, Debug, Clone)]
pub struct Lift {
    pub spec: LiftSpec,
    /// the car's offset above the bottom stop (model units)
    pub at: f32,
    /// its speed (model units / s, signed)
    vel: f32,
    /// the stop it's at or heading for (0 bottom, 1 top)
    pub target: u8,
    /// 0 shut .. 1 open
    pub doors: f32,
    /// seconds the explorer has stood in the docked car
    dwell: f32,
    /// seconds the explorer has waited at the other landing
    call: f32,
    /// the explorer rode in on this trip: wait until they step out before leaving again
    rode: bool,
    /// the explorer is in the car now
    pub rider: bool,
    /// the model has call buttons: the car goes only where they send it
    pub buttons: bool,
    base: Vec3,
}

impl Lift {
    fn new(spec: LiftSpec, base: Vec3) -> Self {
        Self {
            spec,
            at: 0.0,
            vel: 0.0,
            target: 0,
            doors: 1.0,
            dwell: 0.0,
            call: 0.0,
            rode: false,
            rider: false,
            buttons: false,
            base,
        }
    }

    /// The floor its displays show (None without `floors`).
    pub fn floor_number(&self) -> Option<u32> {
        let [a, b] = self.spec.floors?;
        let f = (self.at / self.spec.travel).clamp(0.0, 1.0);
        Some((a as f32 + f * (b as f32 - a as f32)).round() as u32)
    }

    /// Presses a button of this car (`level` Some: a landing's call button, None: the ride
    /// button in the car); returns what it did, None when there was nothing to do.
    pub fn press(&mut self, level: Option<u8>) -> Option<&'static str> {
        match level {
            Some(k) if self.docked() == Some(k) => None,
            Some(k) if self.target == k => Some("on its way"),
            Some(k) => {
                self.call_to(k);
                Some("called")
            }
            None => {
                let at = self.docked()?;
                self.call_to(1 - at);
                Some(if at == 0 { "going up" } else { "going down" })
            }
        }
    }

    /// What pressing that button would do, for the hint (None: nothing).
    pub fn button_label(&self, level: Option<u8>) -> Option<&'static str> {
        match level {
            Some(k) if self.docked() == Some(k) => None,
            Some(k) if self.target == k => Some("Lift on its way"),
            Some(_) => Some("Call lift"),
            None => match self.docked()? {
                0 => Some("Ride up"),
                _ => Some("Ride down"),
            },
        }
    }

    /// Docked at a stop (Some(level)) or between stops.
    pub fn docked(&self) -> Option<u8> {
        let stop = self.target as f32 * self.spec.travel;
        ((self.at - stop).abs() < 1e-3).then_some(self.target)
    }

    /// Send the car to `level` (it shuts its doors first).
    pub fn call_to(&mut self, level: u8) {
        if self.target != level.min(1) {
            self.target = level.min(1);
            self.rode = false;
            self.dwell = 0.0;
        }
    }

    /// The car's speed (model units / s, signed: up +).
    pub fn velocity(&self) -> f32 {
        self.vel
    }

    pub fn state(&self) -> &'static str {
        match self.docked() {
            Some(_) if self.doors > 0.999 => "open",
            Some(_) if self.doors < 1e-3 && self.vel == 0.0 => "shut",
            Some(_) => "doors moving",
            None if self.target == 1 => "going up",
            None => "going down",
        }
    }

    /// One frame of the car: who's where decides where it goes. `rider`: the explorer stands in
    /// the car; `waiting`: at the landing of the other stop.
    fn tick(&mut self, dt: f32, rider: bool, waiting: Option<u8>) {
        self.rider = rider;
        let stop = self.target as f32 * self.spec.travel;
        match self.docked() {
            Some(_) if self.buttons => {
                // button-run: it waits, doors open, for the next press
                self.vel = 0.0;
                self.doors = (self.doors + dt / DOOR_S).min(1.0);
            }
            Some(level) => {
                self.vel = 0.0;
                if rider {
                    if !self.rode {
                        self.dwell += dt;
                        if self.dwell >= self.spec.wait {
                            self.target = 1 - level;
                            self.rode = true;
                            self.dwell = 0.0;
                        }
                    }
                } else {
                    self.rode = false;
                    self.dwell = 0.0;
                }
                if waiting == Some(1 - level) && !rider {
                    self.call += dt;
                    if self.call >= CALL_S {
                        self.target = 1 - level;
                        self.call = 0.0;
                    }
                } else {
                    self.call = 0.0;
                }
                if self.docked().is_some() {
                    self.doors = (self.doors + dt / DOOR_S).min(1.0);
                }
            }
            None => {
                // shut first, then ride: accelerate, cruise, brake to stop exactly at the stop
                if self.doors > 0.0 {
                    self.doors = (self.doors - dt / DOOR_S).max(0.0);
                    return;
                }
                let left = stop - self.at;
                let dir = left.signum();
                let brake = (2.0 * self.spec.accel * left.abs()).sqrt();
                let want = dir * self.spec.speed.min(brake).max(0.05);
                let dv = self.spec.accel * dt;
                self.vel += (want - self.vel).clamp(-dv, dv);
                let step = self.vel * dt;
                if step.abs() >= left.abs() || left.abs() < 1e-3 {
                    self.at = stop;
                    self.vel = 0.0;
                } else {
                    self.at += step;
                }
            }
        }
        // leaving: the doors shut before the car moves (it isn't docked at its target any more)
        if self.docked().is_none() && self.doors > 0.0 {
            self.doors = (self.doors - dt / DOOR_S).max(0.0);
        }
    }
}

/// A sliding door of a car (`level` None) or of a landing.
#[derive(Component, Debug, Clone)]
pub struct LiftDoor {
    how: DoorMove,
    level: Option<u8>,
    base: Vec3,
    base_rot: Quat,
    /// the car it belongs to
    lift: Entity,
}

/// A call button at a landing (`level` Some) or the ride button in a car (None).
#[derive(Component, Debug, Clone)]
pub struct LiftButton {
    pub level: Option<u8>,
    pub lift: Entity,
}

/// One glyph of a floor display: shown while its car's floor has `digit` at `place`.
#[derive(Component, Debug, Clone)]
pub struct LiftDigit {
    place: u8,
    digit: u8,
    pub lift: Entity,
}

/// The lift button the explorer can press right now.
#[derive(Resource, Default)]
pub struct LiftButtonFocus {
    pub entity: Option<Entity>,
    pub distance: f32,
}

/// Where the explorer can stand on lift cars this frame (world space).
#[derive(Clone, Copy, Debug)]
pub struct LiftFloor {
    pub centre: Vec2,
    pub floor: f32,
    pub radius: f32,
    pub shaft: f32,
    /// heights of the bottom and top stops
    pub stops: [f32; 2],
    /// docked with its doors open (the explorer may step in and out)
    pub open: bool,
    /// the explorer stood in the car as this frame began: it carries them wherever it moved,
    /// even further than a step (a fast car, a slow frame)
    pub rider: bool,
}

#[derive(Resource, Default)]
pub struct LiftFloors(pub Vec<LiftFloor>);

impl LiftFloors {
    /// The car floor under feet at `p` (standing in a car, or in its open doorway).
    pub fn floor_at(&self, p: Vec3) -> Option<f32> {
        self.0.iter().find_map(|l| {
            let d = p.xz().distance(l.centre);
            let reach = if l.open { l.shaft } else { l.radius };
            let carried = l.rider && d < l.radius;
            (d < reach && ((p.y - l.floor).abs() < ON_FLOOR || carried)).then_some(l.floor)
        })
    }

    /// Keeps a body of radius `r` at `p` inside a shut or moving car, and out of a shaft whose
    /// car isn't here; returns the corrected position.
    pub fn constrain(&self, mut p: Vec3, r: f32) -> Vec3 {
        for l in &self.0 {
            let off = p.xz() - l.centre;
            let d = off.length();
            if d > l.shaft + r || p.y < l.stops[0] - 0.5 || p.y > l.stops[1] + 2.5 {
                continue;
            }
            let dir = if d > 1e-4 { off / d } else { Vec2::X };
            let on_floor = (p.y - l.floor).abs() < ON_FLOOR || (l.rider && d < l.radius);
            if on_floor && d < l.radius {
                if !l.open {
                    // riding: the car's walls hold you
                    let max = (l.radius - r).max(0.05);
                    if d > max {
                        let q = l.centre + dir * max;
                        p.x = q.x;
                        p.z = q.y;
                    }
                    p.y = l.floor;
                }
            } else if !(on_floor && l.open) && d < l.shaft + r {
                // the car isn't here: the empty shaft is closed off
                let q = l.centre + dir * (l.shaft + r);
                p.x = q.x;
                p.z = q.y;
            }
        }
        p
    }
}

/// A model's scene is in: its cars get their `Lift`, their doors and landing doors a `LiftDoor`.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    nodes: Query<(&GltfExtras, &Transform)>,
    mut commands: Commands,
) {
    let root = ev.event().entity;
    let mut cars = vec![];
    let mut doors = vec![];
    let mut buttons = vec![];
    let mut digits = vec![];
    for c in children.iter_descendants(root) {
        let Ok((x, tf)) = nodes.get(c) else { continue };
        if let Some(spec) = parse_lift(&x.value) {
            cars.push((c, spec, tf.translation));
        } else if let Some((how, level)) = parse_lift_door(&x.value) {
            doors.push((c, how, level, tf.translation, tf.rotation));
        } else if let Some(level) = parse_lift_button(&x.value) {
            buttons.push((c, level));
        } else if let Some(d) = parse_lift_digit(&x.value) {
            digits.push((c, d));
        }
    }
    let Some(&(first, ..)) = cars.first() else { return };
    let ids: Vec<Entity> = cars.iter().map(|c| c.0).collect();
    // a car's own parts hang under it; a landing's go with the model's (first) car
    let car_of = |c: Entity| parents.iter_ancestors(c).find(|a| ids.contains(a));
    let buttons: Vec<(Entity, Entity, Option<u8>)> = buttons
        .into_iter()
        .map(|(c, level)| match car_of(c) {
            Some(car) => (c, car, None),
            None => (c, first, Some(level.unwrap_or(0))),
        })
        .collect();
    for &(c, spec, at) in &cars {
        let mut l = Lift::new(spec, at);
        l.buttons = buttons.iter().any(|b| b.1 == c);
        commands.entity(c).insert(l);
    }
    for (c, how, level, base, base_rot) in doors {
        let car = car_of(c);
        commands.entity(c).insert(LiftDoor {
            how,
            level: if car.is_some() { None } else { level.or(Some(0)) },
            base,
            base_rot,
            lift: car.unwrap_or(first),
        });
    }
    for (c, lift, level) in buttons {
        commands.entity(c).insert(LiftButton { level, lift });
    }
    for (c, (place, digit)) in digits {
        commands.entity(c).insert(LiftDigit { place, digit, lift: car_of(c).unwrap_or(first) });
    }
}

/// Runs the cars and their doors, and publishes where the explorer can stand on them.
pub fn run(
    time: Res<Time>,
    walk: Res<WalkState>,
    mut floors: ResMut<LiftFloors>,
    mut lifts: Query<(Entity, &mut Lift, &mut Transform, &GlobalTransform), Without<LiftDoor>>,
    mut doors: Query<(&LiftDoor, &mut Transform), Without<Lift>>,
) {
    let dt = time.delta_secs().min(0.1);
    let who = walk.active.then_some(walk.position);
    floors.0.clear();
    for (_, mut l, mut tf, gt) in &mut lifts {
        let s = gt.scale().y.max(1e-3);
        let centre = gt.translation().xz();
        let floor = gt.translation().y;
        let bottom = floor - l.at * s;
        let stops = [bottom, bottom + l.spec.travel * s];
        let (rider, waiting) = match who {
            Some(p) if p.xz().distance(centre) < l.spec.shaft * gt.scale().x + 2.6 => {
                let d = p.xz().distance(centre);
                let rider = d < l.spec.radius * gt.scale().x && (p.y - floor).abs() < ON_FLOOR;
                let waiting = (!rider)
                    .then(|| (0..2u8).find(|&k| (p.y - stops[k as usize]).abs() < 1.2))
                    .flatten();
                (rider, waiting)
            }
            _ => (false, None),
        };
        l.tick(dt, rider, waiting);
        tf.translation = l.base + Vec3::Y * l.at;
        let floor = bottom + l.at * s;
        floors.0.push(LiftFloor {
            centre,
            floor,
            radius: l.spec.radius * gt.scale().x,
            shaft: l.spec.shaft * gt.scale().x,
            stops,
            open: l.docked().is_some() && l.doors > 0.9,
            rider: l.rider,
        });
    }
    for (d, mut tf) in &mut doors {
        let Ok((_, l, ..)) = lifts.get(d.lift) else {
            continue;
        };
        let open = match d.level {
            None => l.doors,
            Some(k) if l.docked() == Some(k) => l.doors,
            Some(_) => 0.0,
        };
        let eased = open * open * (3.0 - 2.0 * open);
        tf.translation = d.base + d.how.slide * eased;
        tf.rotation = d.base_rot * Quat::from_rotation_y(d.how.turn * eased);
    }
}

/// Shows each floor display's glyphs for its car's floor.
pub fn displays(lifts: Query<&Lift>, mut digits: Query<(&LiftDigit, &mut Visibility)>) {
    for (d, mut vis) in &mut digits {
        let Some(n) = lifts.get(d.lift).ok().and_then(|l| l.floor_number()) else {
            continue;
        };
        let want = if shows_digit(n, d.place, d.digit) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        vis.set_if_neq(want);
    }
}

/// Picks the lift button the explorer can press: the car's ride button while riding, else a
/// landing's call button looked at (or the nearest) within reach. A chest in reach comes first.
pub fn aim_buttons(
    walk: Res<WalkState>,
    lid: Res<crate::model_render::lid::LidFocus>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    buttons: Query<(Entity, &LiftButton, &GlobalTransform)>,
    lifts: Query<&Lift>,
    mut focus: ResMut<LiftButtonFocus>,
) {
    focus.entity = None;
    if !walk.active || walk.cinematic || lid.entity.is_some() {
        return;
    }
    let Ok(cam) = cam.single() else { return };
    let (eye, fwd) = (cam.translation(), cam.forward().as_vec3());
    let mut best: Option<(f32, Entity, f32)> = None;
    for (e, b, gt) in &buttons {
        let Ok(l) = lifts.get(b.lift) else { continue };
        if l.button_label(b.level).is_none() {
            continue;
        }
        let to = gt.translation() - eye;
        let dist = to.length();
        let score = match b.level {
            None if l.rider => -1.0,
            None => continue,
            Some(_) if dist > BUTTON_REACH || to.y.abs() > 2.2 => continue,
            Some(_) => {
                let ang = fwd.angle_between(to);
                if ang <= BUTTON_AIM { ang } else { 1.0 + dist }
            }
        };
        if best.is_none_or(|x| score < x.0) {
            best = Some((score, e, dist));
        }
    }
    if let Some((_, e, d)) = best {
        focus.entity = Some(e);
        focus.distance = d;
    }
}

/// E: press the lift button in reach.
pub fn press_buttons(
    keys: Res<ButtonInput<KeyCode>>,
    walk: Res<WalkState>,
    pointer: Res<crate::camera::Pointer>,
    focus: Res<LiftButtonFocus>,
    buttons: Query<&LiftButton>,
    mut lifts: Query<&mut Lift>,
) {
    if !walk.active || pointer.keyboard_captured || !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    // (⌘E / Ctrl+E is export)
    if keys.any_pressed([
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
    ]) {
        return;
    }
    let Some(b) = focus.entity.and_then(|e| buttons.get(e).ok()) else { return };
    if let Ok(mut l) = lifts.get_mut(b.lift) {
        l.press(b.level);
    }
}

/// The hint by the crosshair for the lift button in reach.
pub fn button_hint(
    mut contexts: EguiContexts,
    walk: Res<WalkState>,
    focus: Res<LiftButtonFocus>,
    buttons: Query<&LiftButton>,
    lifts: Query<&Lift>,
) -> Result {
    if !walk.active || walk.cinematic {
        return Ok(());
    }
    let Some(b) = focus.entity.and_then(|e| buttons.get(e).ok()) else {
        return Ok(());
    };
    let Some(what) = lifts.get(b.lift).ok().and_then(|l| l.button_label(b.level)) else {
        return Ok(());
    };
    crate::model_render::lid::key_hint(&contexts.ctx_mut()?.clone(), what, None);
    Ok(())
}

fn button_json(world: &World, e: Entity) -> Option<Value> {
    let b = world.get::<LiftButton>(e)?;
    let l = world.get::<Lift>(b.lift)?;
    let p = world.get::<GlobalTransform>(e)?.translation();
    Some(json!({
        "kind": if b.level.is_some() { "lift_call_button" } else { "lift_ride_button" },
        "name": world.get::<Name>(e).map(|n| n.as_str().to_string()).unwrap_or_default(),
        "position": [p.x, p.y, p.z],
        "level": b.level,
        "lift_state": l.state(),
        "floor": l.floor_number(),
        "key": "E",
        "hint": l.button_label(b.level),
    }))
}

/// The lift button in reach, for `walk_state`.
pub fn focus_json(world: &mut World) -> Option<Value> {
    let f = world.resource::<LiftButtonFocus>();
    let (e, d) = (f.entity?, f.distance);
    let mut v = button_json(world, e)?;
    v["distance"] = json!(d);
    Some(v)
}

/// MCP `interact` on the lift button in reach: None when there's none.
pub fn press_focused(world: &mut World) -> Option<Result<Vec<McpContent>, String>> {
    let e = world.resource::<LiftButtonFocus>().entity?;
    let b = world.get::<LiftButton>(e)?.clone();
    let did = world.get_mut::<Lift>(b.lift)?.press(b.level);
    let v = button_json(world, e).unwrap_or(Value::Null);
    Some(Ok(vec![McpContent::Json(json!({"did": did.unwrap_or("nothing"), "button": v}))]))
}

/// The lift sounds (`assets/audio/props/lift_*`, CC0).
#[derive(Resource)]
pub struct LiftSounds {
    ding: Handle<AudioSource>,
    door: Handle<AudioSource>,
    ride: Handle<AudioSource>,
}

pub fn load_sounds(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(LiftSounds {
        ding: server.load("audio/props/lift_ding_0.mp3"),
        door: server.load("audio/props/lift_door_0.mp3"),
        ride: server.load("audio/props/lift_ride_0.mp3"),
    });
}

/// What a car last sounded like: its doors' motion, whether it was docked, its ride loop.
#[derive(Component, Default)]
pub struct LiftAudio {
    doors: f32,
    opening: Option<bool>,
    docked: bool,
    ride: Option<Entity>,
}

/// The ride loop playing under a moving car.
#[derive(Component)]
pub struct LiftRideSound;

/// Plays each car's sounds: the doors when they start to run, the chime when it arrives, and
/// the rushing-air loop while it moves (pitch and level follow its speed: a hum on a slow car,
/// a roar on a fast one).
pub fn sounds(
    mut commands: Commands,
    sounds: Option<Res<LiftSounds>>,
    settings: Option<Res<AudioSettings>>,
    mut cars: Query<(Entity, &Lift, &GlobalTransform, Option<&mut LiftAudio>)>,
    mut sinks: Query<&mut AudioSink, With<LiftRideSound>>,
) {
    let (Some(s), Some(settings)) = (sounds, settings) else { return };
    let master = if settings.muted { 0.0 } else { settings.master };
    let shot = |commands: &mut Commands, clip: &Handle<AudioSource>, at: Vec3, vol: f32, speed: f32| {
        commands.spawn((
            Name::new("Lift sound"),
            AudioPlayer(clip.clone()),
            PlaybackSettings::DESPAWN
                .with_volume(Volume::Linear(vol))
                .with_speed(speed)
                .with_spatial(true)
                .with_spatial_scale(SpatialScale::new(0.45)),
            Transform::from_translation(at),
        ));
    };
    for (e, l, gt, audio) in &mut cars {
        let Some(mut a) = audio else {
            commands.entity(e).insert(LiftAudio { doors: l.doors, docked: l.docked().is_some(), ..default() });
            continue;
        };
        let at = gt.translation() + Vec3::Y * 1.5;
        // the doors: one run of the motor each time they start to open or to shut
        let moving = if l.doors > a.doors + 1e-4 {
            Some(true)
        } else if l.doors < a.doors - 1e-4 {
            Some(false)
        } else {
            None
        };
        if moving.is_some() && moving != a.opening {
            shot(&mut commands, &s.door, at, master * 0.7, 0.96 + fastrand::f32() * 0.08);
        }
        a.opening = moving;
        a.doors = l.doors;
        // arrived: the chime
        let docked = l.docked().is_some();
        if docked && !a.docked {
            shot(&mut commands, &s.ding, at, master * 0.8, 1.0);
        }
        a.docked = docked;
        // the ride: rushing air while it moves, louder and higher the faster it goes
        let v = l.velocity().abs();
        if v > 0.05 {
            let f = (v / 60.0).min(1.0).sqrt();
            let vol = master * (0.25 + 0.75 * f);
            let speed = 0.75 + 0.55 * f;
            match a.ride.and_then(|r| sinks.get_mut(r).ok()) {
                Some(mut sink) => {
                    sink.set_volume(Volume::Linear(vol));
                    sink.set_speed(speed);
                }
                None if a.ride.is_none() => {
                    let r = commands
                        .spawn((
                            Name::new("Lift ride sound"),
                            LiftRideSound,
                            AudioPlayer(s.ride.clone()),
                            PlaybackSettings::LOOP
                                .with_volume(Volume::Linear(vol))
                                .with_speed(speed)
                                .with_spatial(true)
                                .with_spatial_scale(SpatialScale::new(0.45)),
                            Transform::from_xyz(0.0, 1.5, 0.0),
                            ChildOf(e),
                        ))
                        .id();
                    a.ride = Some(r);
                }
                None => {}
            }
        } else if let Some(r) = a.ride.take() {
            commands.entity(r).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LiftSpec {
        parse_lift(r#"{"lift": {"travel": 20, "radius": 1.0, "shaft": 1.3, "wait_s": 2, "speed": 1.5}}"#)
            .unwrap()
    }

    #[test]
    fn reads_lift_extras() {
        let s = spec();
        assert_eq!((s.travel, s.radius, s.shaft, s.wait, s.speed), (20.0, 1.0, 1.3, 2.0, 1.5));
        assert!(parse_lift(r#"{"lift": {"travel": 0}}"#).is_none());
        assert!(parse_lift(r#"{"door": {"axis": [0, 1, 0]}}"#).is_none());
        let (how, level) = parse_lift_door(r#"{"lift_door": {"slide": [0.9, 0, 0], "level": 1}}"#).unwrap();
        assert_eq!((how.slide, how.turn, level), (Vec3::new(0.9, 0.0, 0.0), 0.0, Some(1)));
        let (how, level) = parse_lift_door(r#"{"lift_door": {"turn_deg": -90}}"#).unwrap();
        assert!((how.turn + std::f32::consts::FRAC_PI_2).abs() < 1e-6 && level.is_none());
        assert!(parse_lift_door(r#"{"lift_door": {}}"#).is_none());
        assert!(not_solid(r#"{"lift_door": {"slide": [0, 0, 1]}}"#));
    }

    /// Runs the car for `secs` at 60 fps.
    fn run_for(l: &mut Lift, secs: f32, rider: bool, waiting: Option<u8>) {
        for _ in 0..(secs * 60.0) as usize {
            l.tick(1.0 / 60.0, rider, waiting);
        }
    }

    #[test]
    fn a_rider_goes_up_once_and_back_down_after_stepping_out_and_in() {
        let mut l = Lift::new(spec(), Vec3::ZERO);
        // stepping in: nothing happens before the wait is up
        run_for(&mut l, 1.5, true, None);
        assert_eq!((l.docked(), l.doors), (Some(0), 1.0));
        // then the doors shut and it rides: 20 m at up to 1.5 m/s takes a while
        run_for(&mut l, 2.0, true, None);
        assert_eq!(l.docked(), None);
        run_for(&mut l, 6.0, true, None);
        assert!(l.at > 1.0 && l.at < 20.0, "mid-ride: {}", l.at);
        assert!(l.doors < 1e-3, "shut while moving");
        run_for(&mut l, 20.0, true, None);
        assert_eq!(l.docked(), Some(1));
        assert!(l.doors > 0.999, "open at the top");
        // staying in after arriving doesn't send it back
        run_for(&mut l, 10.0, true, None);
        assert_eq!(l.docked(), Some(1));
        // step out, step back in: down it goes
        run_for(&mut l, 0.5, false, None);
        run_for(&mut l, 40.0, true, None);
        assert_eq!(l.docked(), Some(0));
    }

    #[test]
    fn waiting_at_the_other_landing_calls_the_car() {
        let mut l = Lift::new(spec(), Vec3::ZERO);
        run_for(&mut l, 0.5, false, Some(1));
        assert_eq!(l.docked(), Some(0), "not called yet");
        run_for(&mut l, 40.0, false, Some(1));
        assert_eq!(l.docked(), Some(1));
        assert!(l.doors > 0.999);
    }

    #[test]
    fn the_empty_shaft_is_closed_and_a_moving_car_holds_its_rider() {
        let at = |floor: f32, open: bool| LiftFloors(vec![LiftFloor {
            centre: Vec2::ZERO,
            floor,
            radius: 1.0,
            shaft: 1.3,
            stops: [0.0, 20.0],
            open,
            rider: false,
        }]);
        // the car is up top: walking into the shaft at the bottom pushes you back out
        let p = at(20.0, true).constrain(Vec3::new(0.5, 0.0, 0.0), 0.32);
        assert!((p.xz().length() - 1.62).abs() < 1e-4, "{p}");
        // the car is here and open: step in
        let p = at(0.0, true).constrain(Vec3::new(0.5, 0.0, 0.0), 0.32);
        assert_eq!(p, Vec3::new(0.5, 0.0, 0.0));
        // riding: kept inside, on its floor
        let p = at(7.0, false).constrain(Vec3::new(0.9, 6.8, 0.0), 0.32);
        assert!((p.x - 0.68).abs() < 1e-4 && p.y == 7.0, "{p}");
        assert_eq!(at(7.0, false).floor_at(Vec3::new(0.2, 6.9, 0.0)), Some(7.0));
        assert_eq!(at(7.0, false).floor_at(Vec3::new(0.2, 2.0, 0.0)), None);
    }

    #[test]
    fn a_fast_car_carries_its_rider_further_than_a_step() {
        // the car moved 2.5 m since the rider stood on it (150 m/s on a slow frame)
        let up = LiftFloors(vec![LiftFloor {
            centre: Vec2::ZERO,
            floor: 102.5,
            radius: 2.25,
            shaft: 3.3,
            stops: [0.0, 1196.0],
            open: false,
            rider: true,
        }]);
        assert_eq!(up.floor_at(Vec3::new(0.4, 100.0, 0.3)), Some(102.5));
        let p = up.constrain(Vec3::new(0.4, 100.0, 0.3), 0.32);
        assert_eq!(p, Vec3::new(0.4, 102.5, 0.3), "kept in the car, not pushed out of the shaft");
        // going down: the rider is brought down with it
        let down = LiftFloors(vec![LiftFloor { floor: 97.5, ..up.0[0].clone() }]);
        assert_eq!(down.constrain(Vec3::new(0.4, 100.0, 0.3), 0.32).y, 97.5);
        // a walker who isn't riding gets no such pull
        let empty = LiftFloors(vec![LiftFloor { rider: false, ..up.0[0].clone() }]);
        assert_eq!(empty.floor_at(Vec3::new(0.4, 100.0, 0.3)), None);
    }

    #[test]
    fn buttons_call_and_send_the_car_and_the_display_counts_floors() {
        assert_eq!(parse_lift_button(r#"{"lift_button": {"level": 1}}"#), Some(Some(1)));
        assert_eq!(parse_lift_button(r#"{"lift_button": {"car": true}}"#), Some(None));
        assert_eq!(parse_lift_button(r#"{"lift": {"travel": 3}}"#), None);
        assert_eq!(parse_lift_digit(r#"{"lift_digit": {"place": "t", "digit": 7}}"#), Some((1, 7)));
        assert_eq!(parse_lift_digit(r#"{"lift_digit": {"place": "x", "digit": 7}}"#), None);
        // 300: "300"; 42: no hundreds glyph; 7: units only
        assert!(shows_digit(300, 2, 3) && shows_digit(300, 1, 0) && shows_digit(300, 0, 0));
        assert!(!shows_digit(42, 2, 0) && shows_digit(42, 1, 4) && shows_digit(42, 0, 2));
        assert!(!shows_digit(7, 1, 0) && shows_digit(7, 0, 7));
        let mut l = Lift::new(
            parse_lift(r#"{"lift": {"travel": 20, "radius": 1.0, "speed": 4, "floors": [1, 6]}}"#).unwrap(),
            Vec3::ZERO,
        );
        l.buttons = true;
        assert_eq!(l.floor_number(), Some(1));
        // a button-run car doesn't leave by itself: not with a rider, not with someone waiting
        run_for(&mut l, 10.0, true, None);
        run_for(&mut l, 10.0, false, Some(1));
        assert_eq!(l.docked(), Some(0));
        // the top landing's button calls it; its display counts up on the way
        assert_eq!(l.button_label(Some(0)), None, "already here");
        assert_eq!(l.press(Some(1)), Some("called"));
        assert_eq!(l.button_label(Some(1)), Some("Lift on its way"));
        let mut seen = vec![];
        for _ in 0..60 * 30 {
            l.tick(1.0 / 60.0, false, Some(1));
            let n = l.floor_number().unwrap();
            if seen.last() != Some(&n) {
                seen.push(n);
            }
        }
        assert_eq!(seen, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!((l.docked(), l.doors), (Some(1), 1.0));
        // in the car: ride back down
        assert_eq!(l.button_label(None), Some("Ride down"));
        assert_eq!(l.press(None), Some("going down"));
        run_for(&mut l, 30.0, true, None);
        assert_eq!((l.docked(), l.floor_number()), (Some(0), Some(1)));
        assert_eq!(l.press(None), Some("going up"));
        run_for(&mut l, 0.5, true, None);
        assert_eq!(l.press(None), None, "no press while it leaves");
    }

    #[test]
    fn a_fast_spec_reaches_the_top_quickly() {
        let mut l = Lift::new(
            parse_lift(r#"{"lift": {"travel": 1196, "radius": 2.25, "shaft": 3.3, "wait_s": 1.5, "speed": 150, "accel": 30}}"#).unwrap(),
            Vec3::ZERO,
        );
        assert_eq!((l.spec.speed, l.spec.accel), (150.0, 30.0));
        l.target = 1;
        let mut t = 0.0;
        while l.docked() != Some(1) && t < 60.0 {
            l.tick(1.0 / 30.0, true, None);
            t += 1.0 / 30.0;
        }
        assert!(t < 15.0, "rode 1196 m in {t:.1} s");
    }
}
