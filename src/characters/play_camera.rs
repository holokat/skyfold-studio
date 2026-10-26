//! The game's camera while walking (saved per world in `world.json` `characters.camera`, set in
//! Characters › Player › Camera and MCP `characters_camera`): a style, picked from presets
//! the way MMOs and action RPGs frame their player, plus its distance (fixed, or zoomed by the
//! player between a closest and a furthest), and for the locked views their angle.
//!
//! - Over the shoulder: mouse look, the camera behind and a little right (action MMOs).
//! - First person: through the eyes.
//! - Classic MMO: a chase camera; the pointer stays free, the camera swings in behind as you
//!   run forward, the middle mouse button (held) turns it.
//! - Isometric: locked high three-quarter angle (action RPGs), the pointer aims.
//! - Top-down: locked steep view (top-down MMOs, twin-stick), the pointer aims.
//!
//! With the free pointer, W A S D move across the screen and attacks swing toward the pointer.
//! Over the shoulder and classic MMO never hide anything: walls, floors and ceilings between the
//! character and the camera pull it in (indoors it stays in the room) and it eases back out.
//! The locked views see in instead (`crate::model_render::cutaway`): under a roof the building is
//! cut away above the player's head, and what stands in front of the player opens a see-through
//! circle round it.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use serde::{Deserialize, Serialize};

use super::player::PlayerView;
use super::{Characters, WorldCharacters, labs_enabled};
use crate::camera::{MainCamera, Pointer};
use crate::model_render::cutaway::Cutaway;
use crate::motion::collide::Colliders;
use crate::objects::ObjectStore;
use crate::terrain::Terrain;
use crate::motion::movement::MoveMode;
use crate::walk::WalkState;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraStyle {
    #[default]
    Shoulder,
    FirstPerson,
    Chase,
    Isometric,
    TopDown,
}

impl CameraStyle {
    pub const ALL: [Self; 5] = [
        Self::Shoulder,
        Self::FirstPerson,
        Self::Chase,
        Self::Isometric,
        Self::TopDown,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Shoulder => "shoulder",
            Self::FirstPerson => "first_person",
            Self::Chase => "chase",
            Self::Isometric => "isometric",
            Self::TopDown => "top_down",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Shoulder => "Over the shoulder",
            Self::FirstPerson => "First person",
            Self::Chase => "Classic MMO",
            Self::Isometric => "Isometric",
            Self::TopDown => "Top-down",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Self::Shoulder => "Mouse look with the camera behind the character, like action MMOs.",
            Self::FirstPerson => "Through the character's eyes. The mouse looks around.",
            Self::Chase => "The pointer stays free. The camera follows behind as you run; hold the middle mouse button to turn it.",
            Self::Isometric => "A locked high angle, like action RPGs. The view never turns; attacks swing toward the pointer.",
            Self::TopDown => "A locked, steep view from above. Attacks swing toward the pointer.",
        }
    }

    /// The view's angle is the game maker's (the player can't turn it).
    pub fn locked(self) -> bool {
        matches!(self, Self::Isometric | Self::TopDown)
    }

    /// The pointer shows and aims (no mouse look) while the camera is outside the character.
    pub fn free_pointer(self) -> bool {
        matches!(self, Self::Chase | Self::Isometric | Self::TopDown)
    }
}

/// The world's game camera. Distances are metres for a 1.8 m character (scaled with its height).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameCamera {
    pub style: CameraStyle,
    /// how far from the character (the fixed distance, or where zooming starts)
    pub distance: f32,
    /// the player may zoom with the mouse wheel, between `zoom_min` and `zoom_max`
    pub zoom: bool,
    pub zoom_min: f32,
    pub zoom_max: f32,
    /// locked views: compass bearing the camera looks toward (0 north = −z, 90 east = +x)
    pub heading_deg: f32,
    /// locked views: how steeply it looks down (0 level, 90 straight down)
    pub tilt_deg: f32,
    /// field of view (classic MMO and the locked views; the others keep the walk's own)
    pub fov_deg: f32,
    /// over the shoulder / classic MMO: P (or zooming all the way in) goes first person
    pub first_person: bool,
}

impl Default for GameCamera {
    fn default() -> Self {
        Self::preset(CameraStyle::Shoulder)
    }
}

impl GameCamera {
    /// A style's starting settings.
    pub fn preset(style: CameraStyle) -> Self {
        let base = Self {
            style,
            distance: 3.4,
            zoom: true,
            zoom_min: 1.1,
            zoom_max: 9.0,
            heading_deg: 0.0,
            tilt_deg: 20.0,
            fov_deg: 50.0,
            first_person: true,
        };
        match style {
            CameraStyle::Shoulder => base,
            CameraStyle::FirstPerson => Self { zoom: false, ..base },
            CameraStyle::Chase => Self {
                distance: 7.0,
                zoom_min: 3.0,
                zoom_max: 20.0,
                tilt_deg: 18.0,
                first_person: false,
                ..base
            },
            CameraStyle::Isometric => Self {
                distance: 18.0,
                zoom_min: 12.0,
                zoom_max: 28.0,
                heading_deg: 45.0,
                tilt_deg: 50.0,
                fov_deg: 35.0,
                first_person: false,
                ..base
            },
            CameraStyle::TopDown => Self {
                distance: 24.0,
                zoom_min: 16.0,
                zoom_max: 36.0,
                heading_deg: 0.0,
                tilt_deg: 72.0,
                fov_deg: 40.0,
                first_person: false,
                ..base
            },
        }
    }

    /// Values in range (after an edit from the UI or MCP).
    pub fn sanitize(&mut self) {
        let fix = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.zoom_min = fix(self.zoom_min, 0.8, 80.0, 1.1);
        self.zoom_max = fix(self.zoom_max, self.zoom_min, 80.0, self.zoom_min);
        self.distance = fix(self.distance, 0.8, 80.0, 3.4);
        self.heading_deg = fix(self.heading_deg, -720.0, 720.0, 0.0).rem_euclid(360.0);
        self.tilt_deg = fix(self.tilt_deg, 10.0, 89.0, 50.0);
        self.fov_deg = fix(self.fov_deg, 20.0, 90.0, 50.0);
    }

    /// Where the camera starts: the distance, inside the zoom range when zooming.
    pub fn start_distance(&self) -> f32 {
        if self.zoom {
            self.distance.clamp(self.zoom_min, self.zoom_max)
        } else {
            self.distance
        }
    }
}

/// The game camera placed for the frame (weapons aim along it after).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlayCameraSystems;

pub struct PlayCameraPlugin;

impl Plugin for PlayCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (place, pointer_grab, crate::model_render::cutaway::apply)
                .chain()
                .in_set(PlayCameraSystems)
                .after(super::player::PlayerSystems)
                .before(bevy::transform::TransformSystems::Propagate),
        );
    }
}

/// The style in effect (characters are a Labs feature: without Labs the walk is as it always was).
pub fn effective(world: &WorldCharacters) -> CameraStyle {
    if labs_enabled() {
        world.camera.style
    } else {
        CameraStyle::Shoulder
    }
}

/// Puts the camera where the style wants it (after Walk mode set the first-person eye).
#[allow(clippy::too_many_arguments)]
fn place(
    mut walk: ResMut<WalkState>,
    world: Res<WorldCharacters>,
    chars: Res<Characters>,
    mut view: ResMut<PlayerView>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    pointer: Res<Pointer>,
    terrain: Res<Terrain>,
    colliders: Res<Colliders>,
    store: Res<ObjectStore>,
    time: Res<Time>,
    mut cam: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut cutaway: ResMut<Cutaway>,
) {
    cutaway.on = false;
    let cfg = &world.camera;
    let style = effective(&world);
    view.style = style;
    // a new world or an edited camera starts over from its settings
    if view.applied.as_ref() != Some(cfg) {
        view.applied = Some(cfg.clone());
        view.distance = cfg.start_distance();
        view.third_person = style != CameraStyle::FirstPerson;
        view.shown_distance = 0.0;
        view.orbit = None;
    }
    if !walk.active || walk.cinematic {
        view.seen = false;
        view.free_pointer = false;
        view.pivot_y = None;
        view.orbit = None;
        view.room = 0.0;
        view.roof_cut = None;
        return;
    }
    let body_h = world
        .player
        .as_ref()
        .filter(|_| labs_enabled())
        .and_then(|id| chars.get(id))
        .filter(|e| e.rigged())
        .map(|e| e.meta.height.clamp(0.4, 4.0));
    let input = !walk.input_paused;
    let can_first = cfg.first_person && matches!(style, CameraStyle::Shoulder | CameraStyle::Chase);
    if input && can_first && keys.just_pressed(KeyCode::KeyP) {
        view.third_person = !view.third_person;
    }
    match style {
        CameraStyle::FirstPerson => view.third_person = false,
        s if s.locked() => view.third_person = true,
        _ if !can_first => view.third_person = true,
        _ => {}
    }

    // the wheel (flying, it sets the speed; over a panel, it scrolls the panel)
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    let wheel = input
        && notches != 0.0
        && walk.mode != MoveMode::Fly
        && !(view.free_pointer && pointer.over_ui);
    if !cfg.zoom {
        view.distance = cfg.distance;
    } else if wheel && style != CameraStyle::FirstPerson {
        let (lo, hi) = (cfg.zoom_min, cfg.zoom_max.max(cfg.zoom_min));
        if view.third_person {
            // in toward the shoulder and on into first person (when it may), out again
            let d = view.distance * 0.85f32.powf(notches);
            if can_first && d < lo * 0.95 {
                view.third_person = false;
            }
            view.distance = d.clamp(lo, hi);
        } else if notches < 0.0 {
            view.third_person = true;
            view.distance = (lo * 1.3).min(hi);
            view.shown_distance = lo;
        }
    }

    // outside the character: always in the locked views; else with a character to see (riding
    // is seen from the saddle: no horse yet)
    let seen = style.locked()
        || (view.third_person && body_h.is_some() && walk.mode != MoveMode::Mount);
    view.seen = seen;
    view.free_pointer = seen && style.free_pointer();
    if !seen {
        view.pivot_y = None;
        view.orbit = None;
        view.room = 0.0;
        view.roof_cut = None;
        return;
    }
    let Ok((mut c, mut proj)) = cam.single_mut() else {
        return;
    };
    let dt = time.delta_secs().min(0.1);
    let h = body_h.unwrap_or(1.8);
    if view.shown_distance <= 0.0 {
        view.shown_distance = view.distance;
    }
    view.shown_distance += (view.distance - view.shown_distance) * (1.0 - (-12.0 * dt).exp());
    let dist = view.shown_distance * (h / 1.8).clamp(0.6, 1.6);
    let p = walk.position;

    if style == CameraStyle::Shoulder {
        let look = Quat::from_euler(EulerRot::YXZ, walk.yaw, walk.pitch, 0.0);
        let pivot = p + Vec3::Y * (h * 0.85);
        let eye = pivot + look * Vec3::Z * dist + look * Vec3::X * (0.35 * h / 1.8);
        c.translation = pull_in(&mut view, &colliders, &store, &terrain, pivot, eye, dt);
        c.rotation = look;
        return;
    }

    // the chase and locked views follow a steadied pivot (small bumps don't shake the view)
    let want_y = p.y + h * if style == CameraStyle::Chase { 0.75 } else { 0.5 };
    let py = match view.pivot_y {
        Some(y) if (y - want_y).abs() < 4.0 => y + (want_y - y) * (1.0 - (-dt * 10.0).exp()),
        _ => want_y,
    };
    view.pivot_y = Some(py);
    let pivot = Vec3::new(p.x, py, p.z);
    let (yaw, pitch) = if style == CameraStyle::Chase {
        let (mut yaw, mut down) = view.orbit.unwrap_or((walk.yaw, cfg.tilt_deg.to_radians()));
        if input && mouse.pressed(MouseButton::Middle) {
            yaw -= motion.delta.x * 0.004;
            down = (down + motion.delta.y * 0.004).clamp(0.05, 1.3);
        } else {
            // running forward-ish, it swings in behind (backing up or strafing it stays put)
            let v = walk.velocity.xz();
            if v.length() > 1.0 {
                let d = wrap(v.x.atan2(v.y) + std::f32::consts::PI - yaw);
                if d.abs() < 1.1 {
                    yaw += d * (1.0 - (-dt * 1.8).exp());
                }
            }
        }
        view.orbit = Some((yaw, down));
        (yaw, -down)
    } else {
        (-cfg.heading_deg.to_radians(), -cfg.tilt_deg.to_radians())
    };
    // W A S D go where the view looks
    walk.yaw = yaw;
    walk.pitch = 0.0;
    let rot = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    let mut eye = pivot + rot * Vec3::Z * dist;
    if style == CameraStyle::Chase {
        eye = pull_in(&mut view, &colliders, &store, &terrain, pivot, eye, dt);
    } else {
        eye.y = eye.y.max(terrain.height_at(eye.x, eye.z) + 0.35);
    }
    c.translation = eye;
    c.look_at(pivot, Vec3::Y);
    if let Projection::Perspective(pp) = &mut *proj {
        pp.fov = cfg.fov_deg.to_radians();
    }
    if style.locked() {
        see_in(&mut view, &mut cutaway, &colliders, &store, p, h, eye, dt);
    }
}

/// The locked views see in: under a roof (a building's upper floor or roof, a bridge) the cut comes
/// down from its top to just over the player's head (following it up and down stairs) and goes back
/// up on the way out; what stands in front of the player opens a see-through circle.
#[allow(clippy::too_many_arguments)]
fn see_in(
    view: &mut PlayerView,
    cutaway: &mut Cutaway,
    colliders: &Colliders,
    store: &ObjectStore,
    p: Vec3,
    h: f32,
    eye: Vec3,
    dt: f32,
) {
    let ease = 1.0 - (-dt * 8.0).exp();
    let over = colliders.roof_over(store, p, 0.3, h.min(1.4), 14.0);
    view.roof_cut = match (over, view.roof_cut) {
        (Some((lo, hi)), was) => {
            let want = p.y + (h * 1.25).max(2.3);
            let from = was.map_or(hi.y, |w| w.0);
            let b = Vec4::new(lo.x - 0.3, lo.z - 0.3, hi.x + 0.3, hi.z + 0.3);
            Some((from + (want - from) * ease, b, hi.y))
        }
        (None, Some((cut, b, top))) if cut < top => Some((cut + (top + 0.5 - cut) * ease, b, top)),
        (None, _) => None,
    };
    *cutaway = Cutaway {
        on: true,
        player: p + Vec3::Y * (h * 0.6),
        feet: p.y,
        camera: eye,
        circle: 0.13,
        roof: view.roof_cut.map(|(cut, b, _)| (b, cut)),
    };
}

/// The camera brought in front of whatever stands between it and the character (walls, floors,
/// ceilings, a boulder, the hillside): in at once, back out gently once there's room again.
fn pull_in(
    view: &mut PlayerView,
    colliders: &Colliders,
    store: &ObjectStore,
    terrain: &Terrain,
    pivot: Vec3,
    eye: Vec3,
    dt: f32,
) -> Vec3 {
    let full = pivot.distance(eye);
    if full < 1e-3 {
        return eye;
    }
    let dir = (eye - pivot) / full;
    // (thin things, a trunk or a post, don't make it jump in and out)
    let mut reach = colliders.sweep(store, pivot, eye, 0.2, 1.5) * full;
    // the ground (underground, in a cave, the terrain is overhead: it doesn't count)
    if pivot.y > terrain.height_at(pivot.x, pivot.z) - 0.5 {
        let n = (full / 0.25).ceil().max(1.0) as usize;
        for i in 1..=n {
            let q = pivot + dir * (full * i as f32 / n as f32);
            if q.y < terrain.height_at(q.x, q.z) + 0.3 {
                reach = reach.min(full * (i - 1) as f32 / n as f32);
                break;
            }
        }
    }
    let want = reach.max(0.25_f32.min(full));
    view.room = if view.room <= 0.0 || want < view.room {
        want
    } else {
        view.room + (want - view.room) * (1.0 - (-dt * 4.0).exp())
    };
    pivot + dir * view.room.min(full)
}

/// The free-pointer styles show the mouse pointer while walking; the others capture it
/// (a bar typed in over the walk manages it itself; screenshot runs never capture it).
fn pointer_grab(
    walk: Res<WalkState>,
    view: Res<PlayerView>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if !walk.active || walk.input_paused || std::env::var("WB_SHOT").is_ok() {
        return;
    }
    let Ok(mut cur) = cursor.single_mut() else {
        return;
    };
    let (grab, visible) = if view.free_pointer {
        (CursorGrabMode::None, true)
    } else {
        (CursorGrabMode::Locked, false)
    };
    if cur.grab_mode != grab || cur.visible != visible {
        cur.grab_mode = grab;
        cur.visible = visible;
    }
}

/// An angle into −π..π.
fn wrap(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let a = (a + std::f32::consts::PI).rem_euclid(t);
    a - std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip_and_stay_in_range() {
        for s in CameraStyle::ALL {
            let mut c = GameCamera::preset(s);
            let before = c.clone();
            c.sanitize();
            assert_eq!(c, before, "{s:?} preset already in range");
            assert_eq!(CameraStyle::from_id(s.id()), Some(s));
            assert!(c.zoom_min <= c.start_distance() && c.start_distance() <= c.zoom_max);
            let j = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<GameCamera>(&j).unwrap(), c);
        }
        // old worlds have no camera: over the shoulder, as before
        let c: GameCamera = serde_json::from_str("{}").unwrap();
        assert_eq!(c, GameCamera::default());
        assert_eq!(c.style, CameraStyle::Shoulder);
    }

    #[test]
    fn sanitize_orders_the_zoom_range() {
        let mut c = GameCamera {
            zoom_min: 30.0,
            zoom_max: 5.0,
            tilt_deg: 120.0,
            heading_deg: -45.0,
            ..GameCamera::preset(CameraStyle::Isometric)
        };
        c.sanitize();
        assert!(c.zoom_min <= c.zoom_max);
        assert_eq!(c.tilt_deg, 89.0);
        assert_eq!(c.heading_deg, 315.0);
    }

    #[test]
    fn locked_heading_points_w_where_the_view_looks() {
        // heading 45° (north-east): the walk's forward (−sin yaw, −cos yaw) is (+x, −z)
        let yaw = -45f32.to_radians();
        let fwd = Vec2::new(-yaw.sin(), -yaw.cos());
        assert!((fwd - Vec2::new(1.0, -1.0).normalize()).length() < 1e-5);
        assert!((wrap(3.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs() < 1e-5);
    }
}
