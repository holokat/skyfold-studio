//! Walk mode: explore the world on foot, swimming and diving, on a mount or flying
//! (`motion::movement`), bumping into trees, rocks and buildings and standing on them
//! (`motion::collide`). Footsteps are classified automatically from the ground (terrain material,
//! water, the object underfoot); entering a zone shows its name and level range and switches
//! weather and ambience.

use bevy::ecs::system::SystemParam;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::audio::{SoundEvent, StepKind, classify_surface};
use crate::camera::{MainCamera, OrbitCamera};
use crate::editor::Editor;
use crate::motion::collide::{Body, ColliderMeshes, Colliders};
use crate::motion::movement::{self, Gait, InWater, MoveInput, MoveMode, MoveWorld};
use crate::objects::{Library, ObjectStore};
use crate::sky::{Lighting, SkyState};
use crate::terrain::Terrain;
use crate::terrain::layers::Surface;
use crate::ui::UiAction;
use crate::water::WaterBodies;
use crate::world::Zones;

/// The camera's field of view outside walk mode (degrees).
const BASE_FOV: f32 = 50.0;
/// Near plane (m) in the editor and on foot: up close (a trunk, the water a swimmer floats in)
/// nothing may be clipped away.
const EDITOR_NEAR: f32 = 0.3;
const WALK_NEAR: f32 = 0.08;

#[derive(Resource, Default)]
pub struct WalkState {
    pub active: bool,
    /// A bar over the walk view is being typed in (`dungeons::ui::walk_build_bar`): the walker
    /// stands still and keys don't steer it (Esc closes the bar, not the walk).
    pub input_paused: bool,
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
    /// Step cycle 0..1 (a footfall at each wrap): drives footsteps and the head bob.
    pub stride: f32,
    /// Footfalls heard since walking began (landings included) and the measured cadence
    /// (steps per second, smoothed): MCP `walk_state` reports them.
    pub footfalls: u32,
    pub step_rate: f32,
    /// seconds of walking (simulated time) since the last footfall
    since_footfall: f32,
    /// Smoothed eye height and bob amplitude (the camera never follows every ground bump).
    eye_y: Option<f32>,
    bob_amp: f32,
    pub surface: Option<Surface>,
    /// (zone name, level label, age seconds)
    pub banner: Option<(String, String, f32)>,
    pub last_zone: u8,
    pub fast_time: bool,
    saved_day_minutes: f32,
    /// On foot, mounted or flying.
    pub mode: MoveMode,
    pub gait: Gait,
    pub water: InWater,
    /// Eye height above the feet (eases between standing, crouching and riding).
    pub eye_off: f32,
    /// 1 = full lungs, 0 = out of breath.
    pub breath: f32,
    /// Free-flight speed (m/s), set with the mouse wheel.
    pub fly_speed: f32,
    /// How far under water the camera is (0 above, 1 under): audio and the HUD read it.
    pub submerged: f32,
    /// A playtest session is running (its HUD replaces the walk readout).
    pub playtest: bool,
    /// A camera path is playing: the editor UI hides and the world animates.
    pub cinematic: bool,
    /// H while walking: no UI at all (HUD, panels, markers) for clean screenshots.
    pub ui_hidden: bool,
    /// Start walking (or change where / how) next frame: from the Playtest button, MCP, tests.
    pub request: Option<WalkStart>,
    /// What the last frame bumped into, stood on and climbed (logs, tests).
    pub last: Option<(movement::StepOut, f32)>,
    /// Caps Lock walk toggle.
    walk_toggle: bool,
    fov: f32,
    /// rider's canter phase (0..1 per stride)
    canter: f32,
    /// first frames after a start / teleport: lift out of anything solid
    settle: u32,
    /// Walking itself along waypoints (MCP `walk_to`): agents test routes, stairs and doors.
    pub autopilot: Option<Autopilot>,
}

/// A walk along waypoints, steered like a player holding W: it ends when the last point is
/// reached, when it stops getting closer for a few seconds (blocked), at its time limit, or when
/// the player touches a movement key.
#[derive(Clone, Debug)]
pub struct Autopilot {
    pub points: Vec<Vec2>,
    /// The point being walked to (index into `points`).
    pub next: usize,
    pub run: bool,
    pub elapsed: f32,
    pub timeout: f32,
    /// How it ended: arrived, stuck, timeout or cancelled (None while walking).
    pub result: Option<&'static str>,
    best: f32,
    since: f32,
}

impl Autopilot {
    pub const ARRIVE: f32 = 0.55;
    const STUCK_SECS: f32 = 3.0;

    pub fn new(points: Vec<Vec2>, run: bool, timeout: f32) -> Self {
        Self {
            points,
            next: 0,
            run,
            elapsed: 0.0,
            timeout,
            result: None,
            best: f32::INFINITY,
            since: 0.0,
        }
    }

    /// One frame from `at`: the bearing to steer along (None when it has ended).
    pub fn steer(&mut self, at: Vec2, dt: f32) -> Option<Vec2> {
        if self.result.is_some() {
            return None;
        }
        self.elapsed += dt;
        loop {
            let Some(t) = self.points.get(self.next) else {
                self.result = Some("arrived");
                return None;
            };
            let d = *t - at;
            if d.length() > Self::ARRIVE {
                if d.length() < self.best - 0.2 {
                    self.best = d.length();
                    self.since = 0.0;
                } else {
                    self.since += dt;
                }
                if self.since > Self::STUCK_SECS {
                    self.result = Some("stuck");
                    return None;
                }
                if self.elapsed > self.timeout {
                    self.result = Some("timeout");
                    return None;
                }
                return Some(d / d.length());
            }
            self.next += 1;
            self.best = f32::INFINITY;
            self.since = 0.0;
        }
    }
}

/// Where and how to start walking.
#[derive(Clone, Debug, Default)]
pub struct WalkStart {
    pub pos: Option<Vec3>,
    pub yaw: Option<f32>,
    pub mode: Option<MoveMode>,
    pub playtest: bool,
}

impl WalkState {
    /// Leave a mode and land in another (R mount, F fly; MCP `set_motion_mode`).
    pub fn set_mode(&mut self, mode: MoveMode) {
        movement::set_mode(self, mode);
    }
    /// Walking pace on foot (the Caps Lock toggle; MCP `set_motion_mode walk|run`).
    pub fn set_walk_pace(&mut self, on: bool) {
        self.walk_toggle = on;
    }
    /// Move the explorer (keeps the mode; lifts it out of anything solid).
    pub fn teleport(&mut self, pos: Vec3, yaw: Option<f32>) {
        self.position = pos;
        self.velocity = Vec3::ZERO;
        if let Some(y) = yaw {
            self.yaw = y;
        }
        self.eye_y = None;
        self.settle = 3;
        self.grounded = false;
        self.water = InWater::Dry;
    }
}

/// Starting / stopping walk mode and moving the explorer (and its camera).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WalkSystems;

pub struct WalkPlugin;

impl Plugin for WalkPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WalkState>()
            .add_systems(
                Update,
                (toggle_walk, walk_move)
                    .chain()
                    .in_set(WalkSystems)
                    .after(crate::camera::CameraSystems),
            )
            .add_systems(Update, viewer_position.before(crate::camera::CameraSystems))
            .add_systems(Update, toggle_ui.after(WalkSystems))
            .add_systems(
                PostUpdate,
                hide_ui_output.after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
            );
    }
}

/// H while walking hides every bit of UI (and shows it again); leaving the walk shows it.
fn toggle_ui(
    keys: Res<ButtonInput<KeyCode>>,
    pointer: Res<crate::camera::Pointer>,
    mut walk: ResMut<WalkState>,
) {
    if !walk.active {
        if walk.ui_hidden {
            walk.ui_hidden = false;
        }
        return;
    }
    if keys.just_pressed(KeyCode::KeyH) && !walk.input_paused && !pointer.keyboard_captured {
        walk.ui_hidden = !walk.ui_hidden;
    }
}

/// Drops the frame's egui drawing while the walk's UI is hidden (every window, HUD and marker,
/// whichever system drew it); textures still update.
fn hide_ui_output(walk: Res<WalkState>, mut out: Query<&mut bevy_egui::EguiRenderOutput>) {
    if walk.active && walk.ui_hidden {
        for mut o in &mut out {
            o.paint_jobs.clear();
        }
    }
}

/// Feeds the sky / audio systems with where the "viewer" is (camera focus or the walker).
fn viewer_position(walk: Res<WalkState>, orbit: Res<OrbitCamera>, mut sky: ResMut<SkyState>) {
    sky.viewer_pos = if walk.active {
        walk.position
    } else {
        orbit.focus
    };
}

#[allow(clippy::too_many_arguments)]
fn toggle_walk(
    mut actions: MessageReader<crate::ui::UiAction>,
    keys: Res<ButtonInput<KeyCode>>,
    mut walk: ResMut<WalkState>,
    mut orbit: ResMut<OrbitCamera>,
    terrain: Res<Terrain>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut lighting: ResMut<Lighting>,
    mut ed: ResMut<Editor>,
    sky: Res<SkyState>,
    mut proj: Query<&mut Projection, With<MainCamera>>,
    caves: Res<crate::caves::render::CaveWorld>,
) {
    let mut toggle = false;
    for a in actions.read() {
        if matches!(a, UiAction::ToggleWalk) {
            toggle = true;
        }
    }
    if walk.active && keys.just_pressed(KeyCode::Escape) && !walk.input_paused {
        toggle = true;
    }
    let request = walk.request.take();
    if !toggle && request.is_none() {
        return;
    }
    let Ok(mut cur) = cursor.single_mut() else {
        return;
    };
    if walk.active && request.is_none() {
        walk.active = false;
        walk.playtest = false;
        walk.autopilot = None;
        orbit.enabled = true;
        let p = walk.position;
        let yaw = walk.yaw;
        orbit.set_view(p, yaw, 0.35, 60.0);
        orbit.snap();
        cur.grab_mode = CursorGrabMode::None;
        cur.visible = true;
        if walk.fast_time {
            lighting.day_minutes = walk.saved_day_minutes;
            lighting.playing = false;
            walk.fast_time = false;
        }
        if let Ok(mut p) = proj.single_mut()
            && let Projection::Perspective(pp) = &mut *p
        {
            pp.fov = BASE_FOV.to_radians();
            pp.near = EDITOR_NEAR;
        }
    } else {
        let req = request.unwrap_or_default();
        let f = req.pos.unwrap_or(orbit.focus);
        if !terrain.in_bounds(f.x, f.z) {
            ed.toast("Move the camera over land to start walking");
            return;
        }
        let ground = terrain.height_at(f.x, f.z);
        let was_active = walk.active;
        walk.active = true;
        walk.playtest = req.playtest || (was_active && walk.playtest);
        // (a spot in a cave, or the camera turning round one: on the cave's floor there)
        let underground = orbit.in_cave() || caves.dungeon.as_ref().is_some_and(|d| d.interior);
        let cave_floor = req
            .pos
            .or(underground.then_some(orbit.focus))
            .and_then(|p| caves.floor_under(p + Vec3::Y * 1.0, 8.0));
        let y = match cave_floor {
            Some(c) => c,
            None => req
                .pos
                .map(|p| p.y.max(ground))
                .unwrap_or(ground.max(terrain.sea_level - 1.2)),
        };
        let yaw = req
            .yaw
            .unwrap_or(if was_active { walk.yaw } else { orbit.yaw });
        walk.teleport(Vec3::new(f.x, y, f.z), Some(yaw));
        if !was_active {
            walk.stride = 0.0;
            walk.pitch = -0.05;
            walk.last_zone = sky.viewer_zone;
            walk.banner = None;
            walk.mode = MoveMode::Foot;
            walk.gait = Gait::Run;
            walk.eye_off = movement::EYE_STAND;
            walk.breath = 1.0;
            walk.fov = BASE_FOV;
            if walk.fly_speed <= 0.0 {
                walk.fly_speed = movement::FLY_DEFAULT;
            }
        }
        if let Some(m) = req.mode {
            walk.set_mode(m);
        }
        orbit.enabled = false;
        if let Ok(mut p) = proj.single_mut()
            && let Projection::Perspective(pp) = &mut *p
        {
            pp.near = WALK_NEAR;
        }
        // (automated screenshot runs must not capture the user's mouse)
        if std::env::var("WB_SHOT").is_err() {
            cur.grab_mode = CursorGrabMode::Locked;
            cur.visible = false;
        }
    }
}

/// What the walker needs from the world besides the terrain.
#[derive(SystemParam)]
pub struct WalkWorld<'w> {
    water: Res<'w, WaterBodies>,
    ocean: Res<'w, crate::water::WaterStyle>,
    waves: Res<'w, crate::ocean::OceanWaves>,
    zones: Res<'w, Zones>,
    sky: Res<'w, SkyState>,
    ed: Res<'w, Editor>,
    store: Res<'w, ObjectStore>,
    lib: Option<Res<'w, Library>>,
    colliders: ResMut<'w, Colliders>,
    shapes: ResMut<'w, ColliderMeshes>,
    server: Res<'w, AssetServer>,
    nature: Res<'w, crate::nature::render::Near>,
    deposits: Res<'w, crate::gameplay::deposits::DepositSet>,
    caves: Res<'w, crate::caves::render::CaveWorld>,
}

#[allow(clippy::too_many_arguments)]
fn walk_move(
    mut walk: ResMut<WalkState>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
    terrain: Res<Terrain>,
    mut w: WalkWorld,
    mut lighting: ResMut<Lighting>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut sounds: MessageWriter<SoundEvent>,
    mut graphics: ResMut<crate::streaming::Graphics>,
    road_floors: Res<crate::roads::render::RoadFloors>,
) {
    if !walk.active {
        return;
    }
    // (typing in a bar over the view: no steering, looking or shortcuts)
    let none = ButtonInput::<KeyCode>::default();
    let keys: &ButtonInput<KeyCode> = if walk.input_paused { &none } else { &keys };
    let look = if walk.input_paused {
        Vec2::ZERO
    } else {
        motion.delta
    };
    let dt = time.delta_secs().min(0.05);
    // look
    walk.yaw -= look.x * 0.0022;
    walk.pitch = (walk.pitch - look.y * 0.0022).clamp(-1.45, 1.45);

    // V: draw distance Close / Medium / Far (to feel the frame-rate cost)
    if keys.just_pressed(KeyCode::KeyV) {
        graphics.cycle();
    }
    if keys.just_pressed(KeyCode::KeyT) {
        if walk.fast_time {
            lighting.day_minutes = walk.saved_day_minutes;
            lighting.playing = false;
        } else {
            walk.saved_day_minutes = lighting.day_minutes;
            lighting.day_minutes = 0.4;
            lighting.playing = true;
        }
        walk.fast_time = !walk.fast_time;
    }
    // R: mount / dismount, F: fly / land
    if keys.just_pressed(KeyCode::KeyR) {
        let m = if walk.mode == MoveMode::Mount {
            MoveMode::Foot
        } else {
            MoveMode::Mount
        };
        if m == MoveMode::Mount && matches!(walk.water, InWater::Swimming | InWater::Diving) {
            // (no horse in deep water)
        } else {
            walk.set_mode(m);
        }
    }
    if keys.just_pressed(KeyCode::KeyF) {
        let m = if walk.mode == MoveMode::Fly {
            MoveMode::Foot
        } else {
            MoveMode::Fly
        };
        walk.set_mode(m);
    }
    // Caps Lock: walking pace (macOS reports the lock itself: "pressed" while it is on)
    if cfg!(target_os = "macos") {
        if keys.just_pressed(KeyCode::CapsLock) || keys.just_released(KeyCode::CapsLock) {
            walk.walk_toggle = keys.pressed(KeyCode::CapsLock);
        }
    } else if keys.just_pressed(KeyCode::CapsLock) {
        walk.walk_toggle = !walk.walk_toggle;
    }

    // controls
    let held = |k: &[KeyCode]| k.iter().any(|k| keys.pressed(*k));
    let axis = |pos: &[KeyCode], neg: &[KeyCode]| held(pos) as i32 as f32 - held(neg) as i32 as f32;
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    let inp = MoveInput {
        fwd: axis(
            &[KeyCode::KeyW, KeyCode::ArrowUp],
            &[KeyCode::KeyS, KeyCode::ArrowDown],
        ),
        right: axis(
            &[KeyCode::KeyD, KeyCode::ArrowRight],
            &[KeyCode::KeyA, KeyCode::ArrowLeft],
        ),
        up: held(&[KeyCode::Space]),
        jump: keys.just_pressed(KeyCode::Space),
        down: held(&[KeyCode::KeyC, KeyCode::ControlLeft, KeyCode::ControlRight]),
        sprint: held(&[KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        walk: held(&[KeyCode::AltLeft, KeyCode::AltRight]) || walk.walk_toggle,
        scroll: notches,
    };
    // the autopilot (MCP `walk_to`) holds W toward its next point; any movement key takes over
    let mut inp = inp;
    let at = walk.position.xz();
    let keyed = inp.fwd != 0.0 || inp.right != 0.0 || inp.jump;
    let steer = match &mut walk.autopilot {
        Some(ap) if keyed => {
            ap.result = ap.result.or(Some("cancelled"));
            None
        }
        Some(ap) => ap.steer(at, dt).map(|d| (d, ap.run)),
        None => None,
    };
    if let Some((dir, run)) = steer {
        // fwd = (-sin yaw, -cos yaw)
        walk.yaw = (-dir.x).atan2(-dir.y);
        inp.fwd = 1.0;
        inp.walk = !run;
    }

    // solid objects around the explorer (built lazily, a few per frame)
    let p = walk.position;
    let now = time.elapsed_secs_wrapped();
    let mut missing = 0;
    if let Some(lib) = &w.lib {
        let r = if walk.mode == MoveMode::Mount {
            16.0
        } else {
            10.0
        };
        let budget = if walk.settle > 0 { 400 } else { 24 };
        missing = w
            .colliders
            .ensure(p.xz(), r, &w.store, lib, &mut w.shapes, &w.server, budget);
        // procedural trunks, rocks and logs around the explorer (`crate::nature`)
        let mut near = vec![];
        w.nature.query(p.xz(), r + 8.0, &mut near);
        let mut near: Vec<crate::objects::Placed> = near
            .iter()
            .filter(|i| i.cat != 2 || i.kind == crate::procgen::meshes::AssetKind::Cactus)
            .map(crate::nature::Instance::placed)
            .collect();
        // ore deposits are rocks too
        near.extend(w.deposits.near(p.xz(), r + 8.0).cloned());
        w.colliders
            .ensure_procedural(p.xz(), r, &near, lib, &w.shapes, budget);
    }
    // just arrived: stand on top of whatever is here instead of inside it (once the colliders
    // around are ready: a building's arrive from the background a few frames later)
    if walk.settle > 0 {
        if missing == 0 {
            walk.settle -= 1;
        }
        let body = Body::WALKER;
        if w.colliders.blocked(&w.store, walk.position, body) {
            // dropped against furniture or a wall: step aside on the same floor first; only when
            // that doesn't free it (inside a boulder, a solid block) stand on top of what's there
            // (never lifted onto a roof because a shelf stands close by)
            let (q, _) = w.colliders.resolve(&w.store, walk.position, body);
            if q.xz().distance(walk.position.xz()) < 1.2 && !w.colliders.blocked(&w.store, q, body)
            {
                walk.position = q;
            } else if let Some(top) = w.colliders.lift(&w.store, walk.position, body) {
                walk.position.y = walk.position.y.max(top);
            }
        }
    }
    let surface = |xz: Vec2| {
        crate::motion::waves::surface_at(&terrain, &w.water, &w.ocean, &w.waves, xz, &time)
    };
    let decks = |p: Vec3| road_floors.floor_at(p);
    let world = MoveWorld {
        terrain: &terrain,
        colliders: &w.colliders,
        store: &w.store,
        surface: &surface,
        decks: &decks,
        caves: Some(&w.caves),
    };
    let was_grounded = walk.grounded;
    let fall_speed = walk.velocity.y;
    let out = movement::step(&mut walk, &inp, &world, dt);
    let np = walk.position;
    let v = walk.velocity;
    if out.dismounted {
        sounds.write(SoundEvent::Ui("ui_click_0"));
    }

    // footsteps
    let swimming = matches!(walk.water, InWater::Swimming | InWater::Diving);
    let surface_kind = if swimming || walk.mode == MoveMode::Fly {
        None
    } else {
        classify_surface(
            np,
            &terrain,
            &w.water,
            &w.ed.layer_surface,
            out.on_object
                .map(|o| o.1)
                .or_else(|| road_floors.surface_at(np))
                .or_else(|| w.caves.dungeon.as_ref().and_then(|d| d.surface_at(np))),
        )
    };
    walk.surface = surface_kind;
    let ground_speed = Vec2::new(v.x, v.z).length();
    let moved = ground_speed * dt;
    let mounted = walk.mode == MoveMode::Mount;
    if walk.grounded && moved > 0.0 && walk.mode != MoveMode::Fly {
        walk.stride += moved / step_length(mounted, walk.gait);
        if walk.stride >= 1.0 {
            walk.stride = walk.stride.fract();
            if let Some(s) = surface_kind {
                sounds.write(SoundEvent::Step(s, step_kind(mounted, walk.gait)));
                // the cadence heard (a pause of more than a second and a half starts a new count)
                let gap = walk.since_footfall;
                if gap > 0.0 && gap < 1.5 {
                    walk.step_rate += (1.0 / gap - walk.step_rate) * 0.5;
                }
                walk.since_footfall = 0.0;
                walk.footfalls += 1;
            }
        }
    }
    if mounted && walk.grounded {
        walk.canter =
            (walk.canter + moved / if walk.gait == Gait::Walk { 2.4 } else { 6.2 }).fract();
    }
    // a landing: both feet at once, heavier; the next step comes a full stride later
    if walk.grounded
        && !was_grounded
        && fall_speed < -4.0
        && let Some(s) = surface_kind
    {
        sounds.write(SoundEvent::Step(s, StepKind::Land));
        walk.stride = 0.0;
        walk.since_footfall = 0.0;
        walk.footfalls += 1;
    }
    walk.since_footfall += dt;
    if walk.since_footfall > 1.5 {
        walk.step_rate = 0.0; // standing still
    }
    walk.last = Some((out, now));

    // zone entry banner
    if w.sky.viewer_zone != walk.last_zone {
        walk.last_zone = w.sky.viewer_zone;
        if let Some(z) = w.zones.by_id(w.sky.viewer_zone) {
            walk.banner = Some((
                z.name.clone(),
                format!("{}  ·  {}", z.level_label(), z.zone_type.name()),
                0.0,
            ));
            // The banner and ambience change already make the new place clear. Keep zone
            // transitions quiet so walking does not interrupt footsteps, music, or ambience.
        }
    }
    if let Some(b) = &mut walk.banner {
        b.2 += dt;
        if b.2 > 4.5 {
            walk.banner = None;
        }
    }

    // camera
    let target_off = movement::eye_target(&walk);
    walk.eye_off += (target_off - walk.eye_off) * (1.0 - (-dt * 7.0).exp());
    if let Ok((mut t, mut proj)) = cam.single_mut() {
        // eye height follows the ground through a short low-pass (≈ 50 ms): small bumps and
        // heightmap facets no longer shake the view, slopes and jumps still track. Flying and
        // swimming follow exactly (the swell already moves smoothly).
        let target_y = np.y + walk.eye_off;
        let exact = walk.mode == MoveMode::Fly || swimming;
        let eye_y = match walk.eye_y {
            Some(y) if !exact && (y - target_y).abs() < 4.0 => {
                y + (target_y - y) * (1.0 - (-dt * 20.0).exp())
            }
            _ => target_y,
        };
        walk.eye_y = Some(eye_y);
        // gentle bob: lowest at each footfall, one cycle per step, eased in and out with speed;
        // riding, a slow rocking canter (two lifts per stride) with a touch of pitch
        let gait_amp = match (mounted, walk.gait) {
            (true, Gait::Walk) => 0.03,
            (true, _) => 0.05,
            (false, Gait::Sprint) => 0.016,
            (false, Gait::Walk) => 0.006,
            (false, Gait::Crouch) => 0.004,
            _ => 0.011,
        };
        let on_ground = walk.grounded && walk.mode != MoveMode::Fly;
        let want = if on_ground {
            gait_amp * crate::noise::smoothstep(0.4, 2.5, ground_speed)
        } else {
            0.0
        };
        walk.bob_amp += (want - walk.bob_amp) * (1.0 - (-dt * 6.0).exp());
        let tau = std::f32::consts::TAU;
        let (bob, rock) = if mounted {
            let ph = walk.canter * tau;
            (
                -walk.bob_amp * (ph * 2.0).cos(),
                walk.bob_amp * 0.22 * (ph * 2.0 + 0.9).sin(),
            )
        } else {
            (-walk.bob_amp * (walk.stride * tau).cos(), 0.0)
        };
        t.translation = Vec3::new(np.x, eye_y + bob, np.z);
        t.rotation = Quat::from_euler(EulerRot::YXZ, walk.yaw, walk.pitch + rock, 0.0);
        // a slightly wider view at speed (sprinting, galloping, fast flight)
        let extra = match (walk.mode, walk.gait) {
            (MoveMode::Mount, Gait::Sprint) => 5.0,
            (MoveMode::Mount, Gait::Run) => 3.0,
            (MoveMode::Fly, _) => crate::noise::smoothstep(20.0, 120.0, v.length()) * 6.0,
            (_, Gait::Sprint) => 3.0 * crate::noise::smoothstep(6.0, 9.0, ground_speed),
            _ => 0.0,
        };
        walk.fov += (BASE_FOV + extra - walk.fov) * (1.0 - (-dt * 3.0).exp());
        if let Projection::Perspective(pp) = &mut *proj {
            pp.fov = walk.fov.to_radians();
        }
    }
}

/// Ground covered per footfall (m). On foot: a natural cadence for each pace (walk 2.0 steps/s at
/// 2 m/s, run 2.9 at 6 m/s, sprint 3.5 at 9.5 m/s, a slow 1.7 crouching). Riding: hoof beats, a
/// two-beat trot (one per lift of the rocking canter, 2.4 m a stride) and a four-beat gallop (6.2 m).
pub fn step_length(mounted: bool, gait: Gait) -> f32 {
    match (mounted, gait) {
        (true, Gait::Walk) => 1.2,
        (true, _) => 1.6,
        (false, Gait::Walk) => 1.0,
        (false, Gait::Run) => 2.1,
        (false, Gait::Sprint) => 2.75,
        (false, Gait::Crouch) => 1.06,
    }
}

/// How a footfall sounds at this pace.
fn step_kind(mounted: bool, gait: Gait) -> StepKind {
    match (mounted, gait) {
        (true, _) => StepKind::Hoof,
        (false, Gait::Walk) => StepKind::Walk,
        (false, Gait::Run) => StepKind::Run,
        (false, Gait::Sprint) => StepKind::Sprint,
        (false, Gait::Crouch) => StepKind::Crouch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footstep_cadence_is_natural_for_each_pace() {
        let rate = |gait, speed: f32| speed / step_length(false, gait);
        let walk = rate(Gait::Walk, movement::WALK);
        let run = rate(Gait::Run, movement::RUN);
        let sprint = rate(Gait::Sprint, movement::SPRINT);
        let crouch = rate(Gait::Crouch, movement::CROUCH);
        assert!((1.9..=2.1).contains(&walk), "walk {walk:.2} steps/s");
        assert!((2.8..=3.0).contains(&run), "run {run:.2} steps/s");
        assert!((3.3..=3.6).contains(&sprint), "sprint {sprint:.2} steps/s");
        assert!((1.6..=1.8).contains(&crouch), "crouch {crouch:.2} steps/s");
        assert!(walk < run && run < sprint && crouch < walk);
        // riding: a trot beat per canter lift (two per 2.4 m stride), four gallop beats per 6.2 m
        assert!((2.4 / step_length(true, Gait::Walk) - 2.0).abs() < 0.01);
        assert!((6.2 / step_length(true, Gait::Run) - 4.0).abs() < 0.2);
    }

    #[test]
    fn autopilot_follows_waypoints_and_notices_being_blocked() {
        let mut ap = Autopilot::new(
            vec![Vec2::new(0.0, -5.0), Vec2::new(3.0, -5.0)],
            false,
            60.0,
        );
        let d = ap.steer(Vec2::ZERO, 0.1).unwrap();
        assert!((d - Vec2::new(0.0, -1.0)).length() < 1e-5);
        // at the first point: on to the second
        let d = ap.steer(Vec2::new(0.0, -4.8), 0.1).unwrap();
        assert!(d.x > 0.9);
        // no progress for a few seconds: stuck
        for _ in 0..40 {
            ap.steer(Vec2::new(1.0, -5.0), 0.1);
        }
        assert_eq!(ap.result, Some("stuck"));
        let mut ap = Autopilot::new(vec![Vec2::new(0.0, -1.0)], true, 60.0);
        assert!(ap.steer(Vec2::new(0.0, -0.8), 0.1).is_none());
        assert_eq!(ap.result, Some("arrived"));
    }
}
