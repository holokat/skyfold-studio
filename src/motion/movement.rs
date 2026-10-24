//! How the explorer moves: on foot (walk, run, sprint, crouch, jump), in water (swimming on the
//! surface, diving where you look, surfacing, a breath meter), on a mount (a horse's gallop and
//! canter, camera only) and flying (free-fly noclip). One character controller steps every mode:
//! inertia that suits it, terrain and solid objects (`collide`), slopes too steep to climb, and
//! smooth hand-overs between modes (the eye never jumps).

use bevy::prelude::*;

use super::collide::{Body, Colliders, Contact};
use crate::objects::ObjectStore;
use crate::terrain::Terrain;
use crate::terrain::layers::Surface;
use crate::walk::WalkState;

/// How the explorer gets around.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MoveMode {
    #[default]
    Foot,
    Mount,
    Fly,
}

/// Pace on foot (or on the mount: walk = trot, run = gallop).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Gait {
    Walk,
    #[default]
    Run,
    Sprint,
    Crouch,
}

/// Where the explorer is relative to water.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum InWater {
    #[default]
    Dry,
    /// feet in water, still walking
    Wading,
    /// floating at the surface
    Swimming,
    /// under water
    Diving,
}

/// Speeds (m/s).
pub const WALK: f32 = 2.0;
pub const RUN: f32 = 6.0;
pub const SPRINT: f32 = 9.5;
pub const CROUCH: f32 = 1.8;
pub const SWIM: f32 = 2.2;
pub const SWIM_FAST: f32 = 3.3;
pub const DIVE: f32 = 2.0;
pub const DIVE_FAST: f32 = 3.2;
pub const TROT: f32 = 4.5;
pub const GALLOP: f32 = 16.0;
pub const GALLOP_SPRINT: f32 = 22.0;
pub const FLY_DEFAULT: f32 = 25.0;

/// Eye heights above the feet (m).
pub const EYE_STAND: f32 = 1.72;
pub const EYE_CROUCH: f32 = 1.05;
pub const EYE_MOUNT: f32 = 2.6;
pub const EYE_SWIM: f32 = 1.5;
pub const EYE_DIVE: f32 = 0.45;

const GRAVITY: f32 = 22.0;
const JUMP: f32 = 7.0;
const MOUNT_JUMP: f32 = 6.6;
/// Water deeper than this (m) floats you.
const SWIM_DEPTH: f32 = 1.35;
/// Seconds of breath under water.
pub const BREATH_SECS: f32 = 45.0;

/// This frame's controls.
#[derive(Clone, Copy, Default, Debug)]
pub struct MoveInput {
    /// forward / right, each -1..1
    pub fwd: f32,
    pub right: f32,
    /// Space held / pressed this frame
    pub up: bool,
    pub jump: bool,
    /// C or Ctrl held
    pub down: bool,
    pub sprint: bool,
    pub walk: bool,
    /// mouse wheel notches (fly speed)
    pub scroll: f32,
}

/// What the world looks like to the controller this frame.
pub struct MoveWorld<'a> {
    pub terrain: &'a Terrain,
    pub colliders: &'a Colliders,
    pub store: &'a ObjectStore,
    /// the water surface at a point (lakes, rivers, the sea with its waves)
    pub surface: &'a dyn Fn(Vec2) -> Option<f32>,
    /// walkable surfaces that aren't objects (bridge decks): their height and surface near a point
    pub decks: &'a dyn Fn(Vec3) -> Option<(f32, Surface)>,
    /// the caves' rock: where a cave shapes the space it decides the floor and the walls
    pub caves: Option<&'a crate::caves::render::CaveWorld>,
}

impl MoveWorld<'_> {
    /// The surface currently supporting the feet, excluding bridges overhead or underneath.
    fn supporting_deck(&self, feet: Vec3) -> Option<f32> {
        (self.decks)(feet)
            .map(|(height, _)| height)
            .filter(|height| (height - feet.y).abs() < 0.1)
    }

    /// The ground under the feet: a cave's floor where a cave shapes the space (looked for from
    /// `up` metres above the feet), else the terrain.
    pub fn ground(&self, feet: Vec3, up: f32) -> f32 {
        if let Some(c) = self.caves {
            let probe = feet + Vec3::Y * up;
            if let Some(f) = c.floor_under(probe, up + 8.0) {
                return f;
            }
            // in the rock of a cave below the ground (pressing into a wall): stay where you are,
            // the wall pushes you back (never up through the hill to its surface)
            let t = self.terrain.height_at(feet.x, feet.z);
            if feet.y < t - 0.5 && c.field(probe).is_some_and(|d| d <= 0.0) {
                return feet.y;
            }
            return t;
        }
        self.terrain.height_at(feet.x, feet.z)
    }

    /// Is `p` in a cave's space?
    pub fn in_cave(&self, p: Vec3) -> bool {
        self.caves.is_some_and(|c| c.in_space(p))
    }
}

/// What happened (for sounds, the HUD and logs).
#[derive(Default, Debug)]
pub struct StepOut {
    pub contact: Contact,
    /// standing on an object: (id, surface)
    pub on_object: Option<(u64, Surface)>,
    /// climbed this much onto something this frame (m)
    pub stepped: f32,
    /// hit the water falling
    pub splash: bool,
    /// went under / came up
    pub dived: bool,
    pub surfaced: bool,
    /// a mount refused deep water
    pub dismounted: bool,
}

/// Target eye height for the current stance.
pub fn eye_target(s: &WalkState) -> f32 {
    match (s.mode, s.water) {
        (MoveMode::Fly, _) => EYE_STAND,
        (MoveMode::Mount, _) => EYE_MOUNT,
        (_, InWater::Swimming) => EYE_SWIM,
        (_, InWater::Diving) => EYE_DIVE,
        _ if s.gait == Gait::Crouch => EYE_CROUCH,
        _ => EYE_STAND,
    }
}

/// Change the stance so the eye stays where it is (water and flight hand-overs).
fn keep_eye(s: &mut WalkState, new_off: f32) {
    s.position.y += s.eye_off - new_off;
    s.eye_off = new_off;
}

/// Switch mode (R mount / F fly), keeping the view continuous.
pub fn set_mode(s: &mut WalkState, mode: MoveMode) {
    if s.mode == mode {
        return;
    }
    let was = s.mode;
    s.mode = mode;
    match mode {
        MoveMode::Fly => {
            keep_eye(s, EYE_STAND);
            s.velocity.y = 0.0;
            s.water = InWater::Dry;
        }
        // climbing on or off a horse raises / lowers the eye smoothly (feet stay)
        MoveMode::Mount => {
            s.gait = Gait::Run;
        }
        MoveMode::Foot => {
            if was == MoveMode::Fly {
                s.grounded = false;
            }
        }
    }
}

/// A body's speed along the uphill direction is cut on slopes steeper than `soft`..`hard`
/// (radians): nobody walks up a cliff, but nothing ever traps you either.
fn slope_limit(terrain: &Terrain, p: Vec3, v: Vec2, soft: f32, hard: f32) -> Vec2 {
    let n = terrain.normal_at(p.x, p.z);
    let slope = n.y.clamp(-1.0, 1.0).acos();
    if slope <= soft {
        return v;
    }
    let up = -Vec2::new(n.x, n.z).normalize_or_zero();
    let along = v.dot(up);
    if along <= 0.0 {
        return v;
    }
    let keep = crate::noise::smoothstep(hard, soft, slope);
    v - up * along * (1.0 - keep)
}

/// Advance the explorer by `dt` seconds.
pub fn step(s: &mut WalkState, inp: &MoveInput, w: &MoveWorld, dt: f32) -> StepOut {
    let mut out = StepOut::default();
    let p = s.position;
    let terrain_ground = w.ground(p, 0.5);
    let ground = if s.grounded {
        w.supporting_deck(p)
            .unwrap_or(terrain_ground)
            .max(terrain_ground)
    } else {
        terrain_ground
    };
    // (in a cave under a river or lake the rock keeps the water out)
    let surface = if w.in_cave(p + Vec3::Y * 0.5) {
        None
    } else {
        (w.surface)(p.xz())
    };
    let depth = surface.map(|l| l - ground).unwrap_or(f32::NEG_INFINITY);

    // a mount won't swim
    if s.mode == MoveMode::Mount && depth > 1.5 {
        set_mode(s, MoveMode::Foot);
        out.dismounted = true;
    }
    // look axes
    let (sy, cy) = s.yaw.sin_cos();
    let fwd = Vec3::new(-sy, 0.0, -cy);
    let right = Vec3::new(cy, 0.0, -sy);
    let flat = (fwd * inp.fwd + right * inp.right).normalize_or_zero();

    match s.mode {
        MoveMode::Fly => fly(s, inp, w, right, dt),
        MoveMode::Mount => {
            s.water = if depth > 0.05 {
                InWater::Wading
            } else {
                InWater::Dry
            };
            ground_move(s, inp, w, flat, depth, dt, &mut out);
        }
        MoveMode::Foot => {
            // water state hand-overs
            let eye = p.y + s.eye_off;
            match s.water {
                InWater::Dry | InWater::Wading => {
                    if let Some(l) = surface
                        && depth > SWIM_DEPTH
                        && p.y < l - 1.2
                    {
                        out.splash = s.velocity.y < -4.0;
                        s.water = InWater::Swimming;
                        s.gait = Gait::Run;
                        keep_eye(s, EYE_SWIM);
                    } else {
                        s.water = if depth > 0.05 && p.y < surface.unwrap_or(f32::MIN) {
                            InWater::Wading
                        } else {
                            InWater::Dry
                        };
                    }
                }
                InWater::Swimming => {
                    if depth < SWIM_DEPTH - 0.15 {
                        s.water = InWater::Wading;
                        s.grounded = true;
                        // stand up out of the water: feet go to the bottom, the eye rises smoothly
                        let off = s.eye_off;
                        s.position.y = s.position.y.max(ground);
                        s.eye_off = (eye - s.position.y).clamp(0.3, off.max(0.3));
                    } else if inp.down {
                        s.water = InWater::Diving;
                        keep_eye(s, EYE_DIVE);
                        s.velocity.y = s.velocity.y.min(-1.6);
                        out.dived = true;
                    }
                }
                InWater::Diving => {
                    let l = surface.unwrap_or(f32::MIN);
                    if depth < 1.0 {
                        s.water = InWater::Wading;
                        keep_eye(s, EYE_STAND);
                        s.grounded = false;
                    } else if eye > l - 0.2 && s.velocity.y > -0.2 && !inp.down {
                        s.water = InWater::Swimming;
                        keep_eye(s, EYE_SWIM);
                        out.surfaced = true;
                    }
                }
            }
            match s.water {
                InWater::Swimming => swim(s, inp, w, flat, surface.unwrap_or(ground), dt, &mut out),
                InWater::Diving => dive(s, inp, w, right, dt, &mut out),
                _ => ground_move(s, inp, w, flat, depth, dt, &mut out),
            }
        }
    }

    // breath: used up under water, back in a few breaths at the surface
    let under = surface.is_some_and(|l| s.position.y + s.eye_off < l - 0.05)
        && !w.in_cave(s.position + Vec3::Y * s.eye_off);
    if under {
        s.breath = (s.breath - dt / BREATH_SECS).max(0.0);
    } else {
        s.breath = (s.breath + dt / 3.0).min(1.0);
    }

    // stay inside the world
    let lim = w.terrain.half() - 2.0;
    s.position.x = s.position.x.clamp(-lim, lim);
    s.position.z = s.position.z.clamp(-lim, lim);
    out
}

/// Walking, running, crouching and riding over terrain and objects.
fn ground_move(
    s: &mut WalkState,
    inp: &MoveInput,
    w: &MoveWorld,
    flat: Vec3,
    depth: f32,
    dt: f32,
    out: &mut StepOut,
) {
    let mounted = s.mode == MoveMode::Mount;
    let p = s.position;
    // gait
    let crouch_held = inp.down && !mounted;
    s.gait = if crouch_held {
        Gait::Crouch
    } else if inp.sprint {
        Gait::Sprint
    } else if inp.walk {
        Gait::Walk
    } else {
        Gait::Run
    };
    let mut speed = match (mounted, s.gait) {
        (true, Gait::Walk) => TROT,
        (true, Gait::Sprint) => GALLOP_SPRINT,
        (true, _) => GALLOP,
        (false, Gait::Walk) => WALK,
        (false, Gait::Run) => RUN,
        (false, Gait::Sprint) => SPRINT,
        (false, Gait::Crouch) => CROUCH,
    };
    // wading: water drags at the legs
    if depth > 0.05 {
        let k = (depth / if mounted { 1.5 } else { SWIM_DEPTH }).clamp(0.0, 1.0);
        speed *= 1.0 - 0.5 * k;
    }
    let body = if mounted {
        Body::RIDER
    } else if s.gait == Gait::Crouch {
        Body::CROUCHED
    } else {
        Body::WALKER
    };
    // standing up under something low keeps you crouched
    if !mounted
        && s.gait != Gait::Crouch
        && s.eye_off < EYE_STAND - 0.2
        && w.colliders.blocked(
            w.store,
            p,
            Body {
                step: 0.45,
                ..Body::WALKER
            },
        )
    {
        s.gait = Gait::Crouch;
        speed = CROUCH;
    }

    let mut v = s.velocity;
    let mut target = flat.xz() * speed;
    let (soft, hard) = if mounted { (0.55, 0.72) } else { (0.75, 0.95) };
    // A bridge can cross a steep bank without inheriting its uphill movement restriction.
    // Only waive the terrain slope while the feet actually rest on the deck, not while walking
    // underneath it or falling past it. Cave floors are likewise walkable by design.
    let on_deck = s.grounded && w.supporting_deck(p).is_some();
    if !on_deck && !w.in_cave(p + Vec3::Y * 0.5) {
        target = slope_limit(w.terrain, p, target, soft, hard);
    }
    // inertia: a runner reaches speed in ~0.2 s, a horse in ~1.5 s; little control in the air
    let accel = if !s.grounded {
        2.0
    } else if mounted {
        if target.length() > Vec2::new(v.x, v.z).length() {
            1.6
        } else {
            3.0
        }
    } else {
        12.0
    };
    let k = 1.0 - (-accel * dt).exp();
    v.x += (target.x - v.x) * k;
    v.z += (target.y - v.z) * k;
    v.y -= GRAVITY * dt;
    if s.grounded && inp.jump && s.gait != Gait::Crouch {
        v.y = if mounted { MOUNT_JUMP } else { JUMP };
        s.grounded = false;
    }

    let mut np = p + v * dt;
    // solid objects: slide along them (in the air the run-up is kept, so a jump against a rock
    // still carries you onto it once your feet clear its top)
    let (rp, contact) = w.colliders.resolve(w.store, np, body);
    if contact.blocked_by.is_some() {
        let n = Vec3::new(contact.normal.x, 0.0, contact.normal.y);
        let into = v.dot(n);
        if into < 0.0 && s.grounded {
            v -= n * into;
        }
        np.x = rp.x;
        np.z = rp.z;
    }
    out.contact = contact;
    // cave walls: slide along them
    if let Some(c) = w.caves
        && let Some(q) = c.push_out(np, body.radius, body.height, body.step)
    {
        let n = (q - Vec2::new(np.x, np.z)).normalize_or_zero();
        let into = Vec2::new(v.x, v.z).dot(n);
        if into < 0.0 {
            v.x -= n.x * into;
            v.z -= n.y * into;
        }
        np.x = q.x;
        np.z = q.y;
    }

    // the floor: terrain (or a cave's floor) or the top of something low enough to step onto
    let reach = if s.grounded {
        body.step
    } else {
        (-v.y * dt).max(0.0) + 0.05
    };
    let floor_at = |q: Vec3| {
        let t = w.ground(q, body.step.max(0.45));
        let col = w
            .colliders
            .floor(w.store, q, body.radius * 0.7, reach)
            .filter(|f| f.0 > t);
        // a bridge deck within a step counts like the top of an object (id 0: not an object)
        let deck = (w.decks)(q)
            .filter(|d| d.0 > t && d.0 <= q.y + reach)
            .map(|d| (d.0, d.1, 0u64));
        let top = match (col, deck) {
            (Some(a), Some(b)) => Some(if b.0 > a.0 { b } else { a }),
            (a, b) => a.or(b),
        };
        (t, top)
    };
    let (mut terrain_floor, mut obj) = floor_at(np);
    // a steep object surface (a tent's canvas, a boulder's flank) is a wall, not a staircase:
    // stepping up is for ledges with a top to stand on (a walk-in building's stairs are stairs)
    if let Some(f) = obj
        && s.grounded
        && f.0 > p.y + 0.05
        && !w.colliders.walk_in(f.2)
    {
        let dir = Vec2::new(np.x - p.x, np.z - p.z).normalize_or_zero();
        let ahead = Vec3::new(np.x + dir.x * 0.25, f.0, np.z + dir.y * 0.25);
        if w.colliders
            .floor(w.store, ahead, body.radius * 0.7, 0.6)
            .is_some_and(|f2| f2.0 - f.0 > 0.22)
        {
            np.x = p.x;
            np.z = p.z;
            v.x = 0.0;
            v.z = 0.0;
            (terrain_floor, obj) = floor_at(np);
        }
    }
    let floor = obj.map(|f| f.0).unwrap_or(terrain_floor);
    // stick to the ground going downhill or over small drops instead of hopping off every crest
    let step_down = 0.3 + Vec2::new(v.x, v.z).length() * dt;
    let stick = s.grounded && v.y <= 0.0 && np.y - floor <= step_down;
    let mut grounded = false;
    if np.y <= floor + 0.02 || stick {
        if floor > p.y + 0.05 && s.grounded {
            out.stepped = floor - p.y;
        }
        np.y = floor;
        v.y = v.y.max(0.0);
        grounded = true;
    }
    out.on_object = obj
        .filter(|_| grounded && (np.y - floor).abs() < 0.03)
        .map(|f| (f.2, f.1));
    s.velocity = v;
    s.position = np;
    s.grounded = grounded;
}

/// Floating at the surface: the head rides the waves.
fn swim(
    s: &mut WalkState,
    inp: &MoveInput,
    w: &MoveWorld,
    flat: Vec3,
    surface: f32,
    dt: f32,
    out: &mut StepOut,
) {
    s.gait = if inp.sprint {
        Gait::Sprint
    } else if inp.walk {
        Gait::Walk
    } else {
        Gait::Run
    };
    let speed = match s.gait {
        Gait::Sprint => SWIM_FAST,
        Gait::Walk => SWIM * 0.55,
        _ => SWIM,
    };
    let mut v = s.velocity;
    let k = 1.0 - (-3.0 * dt).exp();
    v.x += (flat.x * speed - v.x) * k;
    v.z += (flat.z * speed - v.z) * k;
    // float: the eye a hand above the water, following the swell
    let eye = s.position.y + s.eye_off;
    let want = (surface + 0.2 - eye) * 5.0;
    v.y += (want - v.y) * (1.0 - (-8.0 * dt).exp());
    let mut np = s.position + v * dt;
    let (rp, contact) = w.colliders.resolve(w.store, np, Body::WALKER);
    if contact.blocked_by.is_some() {
        let n = Vec3::new(contact.normal.x, 0.0, contact.normal.y);
        let into = v.dot(n);
        if into < 0.0 {
            v -= n * into;
        }
        np.x = rp.x;
        np.z = rp.z;
    }
    out.contact = contact;
    s.velocity = v;
    s.position = np;
    s.grounded = false;
}

/// Under water: swim where you look, Space up, C / Ctrl down, a slow drift to the surface.
fn dive(
    s: &mut WalkState,
    inp: &MoveInput,
    w: &MoveWorld,
    right: Vec3,
    dt: f32,
    out: &mut StepOut,
) {
    s.gait = if inp.sprint { Gait::Sprint } else { Gait::Run };
    let speed = if inp.sprint { DIVE_FAST } else { DIVE };
    let look = Quat::from_euler(EulerRot::YXZ, s.yaw, s.pitch, 0.0) * Vec3::NEG_Z;
    let vert = (inp.up as i32 - inp.down as i32) as f32;
    let mut target = (look * inp.fwd + right * inp.right).normalize_or_zero() * speed
        + Vec3::Y * vert * speed * 0.8;
    if vert == 0.0 && inp.fwd == 0.0 {
        // a little buoyancy
        target.y += 0.25;
    }
    if s.breath <= 0.0 {
        // out of breath: kick for the surface
        target.y = target.y.max(2.6);
    }
    let mut v = s.velocity;
    v += (target - v) * (1.0 - (-2.5 * dt).exp());
    let mut np = s.position + v * dt;
    // the body is a short capsule around the eye
    let body = Body {
        radius: 0.35,
        height: 0.9,
        step: 0.0,
    };
    let (rp, contact) = w.colliders.resolve(w.store, np, body);
    if contact.blocked_by.is_some() {
        let n = Vec3::new(contact.normal.x, 0.0, contact.normal.y);
        let into = v.dot(n);
        if into < 0.0 {
            v -= n * into;
        }
        np.x = rp.x;
        np.z = rp.z;
    }
    out.contact = contact;
    // the sea floor and rock tops
    let mut floor = w.ground(np, 0.3);
    if let Some(f) = w.colliders.floor(w.store, np, body.radius, 0.3) {
        floor = floor.max(f.0);
    }
    if np.y < floor + 0.15 {
        np.y = floor + 0.15;
        v.y = v.y.max(0.0);
    }
    s.velocity = v;
    s.position = np;
    s.grounded = false;
}

/// Free flight: no collisions, never under the ground, any speed (wheel), Space / C up and down.
fn fly(s: &mut WalkState, inp: &MoveInput, w: &MoveWorld, right: Vec3, dt: f32) {
    if inp.scroll != 0.0 {
        s.fly_speed = (s.fly_speed * 1.2f32.powf(inp.scroll)).clamp(2.0, 400.0);
    }
    let look = Quat::from_euler(EulerRot::YXZ, s.yaw, s.pitch, 0.0) * Vec3::NEG_Z;
    let vert = (inp.up as i32 - inp.down as i32) as f32;
    let boost = if inp.sprint {
        3.0
    } else if inp.walk {
        0.25
    } else {
        1.0
    };
    let target = ((look * inp.fwd + right * inp.right).normalize_or_zero() + Vec3::Y * vert * 0.8)
        * s.fly_speed
        * boost;
    let mut v = s.velocity;
    v += (target - v) * (1.0 - (-4.0 * dt).exp());
    let mut np = s.position + v * dt;
    let floor = w.ground(np, 0.6) + 0.6 - s.eye_off;
    if np.y < floor {
        np.y = floor;
        v.y = v.y.max(0.0);
    }
    s.gait = if inp.sprint {
        Gait::Sprint
    } else if inp.walk {
        Gait::Walk
    } else {
        Gait::Run
    };
    s.velocity = v;
    s.position = np;
    s.grounded = false;
}

/// One line of key hints for the current mode.
pub fn key_hints(s: &WalkState) -> String {
    let fly_speed = s.fly_speed;
    match (s.mode, s.water) {
        (MoveMode::Fly, _) => format!("WASD fly where you look · Space up · C down · Wheel speed ({fly_speed:.0} m/s) · Shift boost · Alt slow · F land · H hide UI · Esc exit"),
        (MoveMode::Mount, _) => "WASD ride · Shift sprint · Alt trot · Space jump · R dismount · F fly · H hide UI · Esc exit".into(),
        (_, InWater::Swimming) => "WASD swim · Shift faster · C / Ctrl dive · F fly · H hide UI · Esc exit".into(),
        (_, InWater::Diving) => "WASD swim where you look · Space up · C / Ctrl down · Shift faster · H hide UI · Esc exit".into(),
        _ => "WASD move · Shift sprint · Alt / Caps walk · C crouch · Space jump · R mount · F fly · T time ×60 · V draw · H hide UI · Esc exit".into(),
    }
}

/// Short name of what the explorer is doing, and the speed it aims for.
pub fn mode_label(s: &WalkState) -> (&'static str, f32) {
    match (s.mode, s.water, s.gait) {
        (MoveMode::Fly, _, Gait::Sprint) => ("Flying fast", s.fly_speed * 3.0),
        (MoveMode::Fly, _, _) => ("Flying", s.fly_speed),
        (MoveMode::Mount, _, Gait::Walk) => ("Trotting", TROT),
        (MoveMode::Mount, _, Gait::Sprint) => ("Galloping hard", GALLOP_SPRINT),
        (MoveMode::Mount, _, _) => ("Galloping", GALLOP),
        (_, InWater::Swimming, Gait::Sprint) => ("Swimming fast", SWIM_FAST),
        (_, InWater::Swimming, _) => ("Swimming", SWIM),
        (_, InWater::Diving, Gait::Sprint) => ("Diving fast", DIVE_FAST),
        (_, InWater::Diving, _) => ("Diving", DIVE),
        (_, _, Gait::Walk) => ("Walking", WALK),
        (_, _, Gait::Sprint) => ("Sprinting", SPRINT),
        (_, _, Gait::Crouch) => ("Crouching", CROUCH),
        _ => ("Running", RUN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk_over_bank(on_bridge: bool) -> WalkState {
        let mut terrain = Terrain::new(32, 1.0, -100.0);
        for z in 0..=terrain.res {
            for x in 0..=terrain.res {
                let index = terrain.idx(x, z);
                terrain.heights[index] = ((x as f32 - 16.0) * 2.0).clamp(-8.0, 8.0);
            }
        }
        let colliders = Colliders::default();
        let store = ObjectStore::default();
        let deck = |_p: Vec3| Some((10.0, Surface::Cobble));
        let world = MoveWorld {
            terrain: &terrain,
            colliders: &colliders,
            store: &store,
            surface: &|_| None,
            decks: &deck,
            caves: None,
        };
        let mut state = WalkState::default();
        state.position = Vec3::new(-3.0, if on_bridge { 10.0 } else { -6.0 }, 0.0);
        state.grounded = true;
        state.eye_off = EYE_STAND;
        for _ in 0..90 {
            ground_move(
                &mut state,
                &MoveInput::default(),
                &world,
                Vec3::X,
                0.0,
                1.0 / 60.0,
                &mut StepOut::default(),
            );
        }
        state
    }

    #[test]
    fn bridge_walk_crosses_a_steep_bank_below_the_deck() {
        let state = walk_over_bank(true);
        assert!(state.position.x > 4.0, "stuck on the bank below the bridge");
        assert!((state.position.y - 10.0).abs() < 0.01);
        assert!(state.grounded);
    }

    #[test]
    fn bridge_overhead_does_not_allow_walking_up_a_cliff() {
        let state = walk_over_bank(false);
        assert!(
            state.position.x < -2.9,
            "an overhead deck bypassed the terrain slope"
        );
    }

    #[test]
    fn bridge_over_deep_water_keeps_a_rider_dry_and_mounted() {
        let terrain = Terrain::new(32, 1.0, 0.0);
        let colliders = Colliders::default();
        let store = ObjectStore::default();
        let world = MoveWorld {
            terrain: &terrain,
            colliders: &colliders,
            store: &store,
            surface: &|_| Some(0.0),
            decks: &|_| Some((10.0, Surface::Cobble)),
            caves: None,
        };
        let mut state = WalkState::default();
        state.position = Vec3::new(0.0, 10.0, 0.0);
        state.grounded = true;
        state.mode = MoveMode::Mount;
        state.eye_off = EYE_MOUNT;
        let result = step(&mut state, &MoveInput::default(), &world, 1.0 / 60.0);
        assert!(!result.dismounted);
        assert_eq!(state.mode, MoveMode::Mount);
        assert_eq!(state.water, InWater::Dry);
    }
}
