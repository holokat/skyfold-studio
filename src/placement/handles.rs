//! Freehand handles on the selected object (Objects tab, Place and Select & Move): drag the
//! object itself to slide it along the ground (ships stay on the water), the knob on the ring
//! around its base to turn it (Shift: 15° steps), a footprint corner to resize it — always in
//! proportion (Shift: ×0.25 steps; per-axis stretch is in the panel's Exact values) — and the
//! arrow above it, or Alt-drag the object, to lift or lower it. Clicking another object selects it in either mode; in Place mode a click on open ground
//! still places, and the new object gets its handles straight away. Each drag is one undo step
//! (the stroke's); Delete, ⌘D and Esc are in `objects::select_tool`.
//!
//! The handles are drawn with the egui painter at a fixed size on screen (the ring never shrinks
//! below a grabbable size), so they read the same at any zoom.
//!
//! Test hook: `WB_HANDLE=<grip>:<value>[:hold]` at frame `WB_HANDLE_FRAME` (default 430) selects
//! the newest object (or `WB_HANDLE_ID`) and drags one of its handles through the same code as
//! the mouse: `turn:35` (degrees clockwise), `scale:1.25`, `stretch:1.3,0.8`, `lift:1.5` (m),
//! `move:12,-4` (m), or just `select`s it. With `hold` the drag stays open (its readout shows in screenshots); otherwise
//! it is released 30 frames later (one undo step, fitted to the ground like a real drag).

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, Pos2, Shape, Stroke, pos2,
    vec2,
};

use super::footprint::{Footprint, ModelBounds};
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, ObjectTool, Tab};
use crate::history::History;
use crate::objects::{Library, LibraryEntry, ObjectStore, Placed};
use crate::terrain::Terrain;
use crate::tools::Stroke as ToolStroke;
use crate::ui::theme::{ACCENT, ON_ACCENT, semibold};
use crate::water::WaterBodies;

/// What a drag holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Body,
    Turn,
    /// a footprint corner (counter-clockwise from the model's −x −z corner)
    Corner(usize),
    Lift,
}

impl Grip {
    fn group_label(self) -> &'static str {
        match self {
            Grip::Body => "Move set",
            Grip::Turn => "Turn set",
            Grip::Corner(_) => "Resize set",
            Grip::Lift => "Lift set",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Grip::Body => "Move object",
            Grip::Turn => "Turn object",
            Grip::Corner(_) => "Resize object",
            Grip::Lift => "Lift object",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub grip: Grip,
    pub id: u64,
    /// the object when the drag began
    pub start: Placed,
    /// where the cursor met the ground (moving) or the object's base plane (turning, resizing)
    /// when the drag began
    pub grab: Vec3,
    /// the cursor on screen when the drag began
    pub screen0: Vec2,
    /// readout next to the cursor ("35°", "×1.25", "+1.2 m")
    pub label: String,
    /// a set piece dragged as one (`start` is then the set's frame)
    pub group: Option<GroupDrag>,
}

/// A set piece being dragged: its pieces as they were, and the frame as it is now.
#[derive(Clone, Debug)]
pub struct GroupDrag {
    pub id: u64,
    pub starts: Vec<Placed>,
    pub now: Placed,
}

#[derive(Resource, Default)]
pub struct Handles {
    pub drag: Option<Drag>,
    /// The press that began the current stroke was taken by the handles (a grip, or picking an
    /// object): Place mode doesn't place with it.
    pub took_press: bool,
    /// the press landed on a locked object (where): it selects, but nothing moves
    pub blocked: Option<Vec2>,
}

/// The handles' geometry for one object.
pub struct Gizmo {
    pub pivot: Vec3,
    pub fp: Footprint,
    /// ring radius (m)
    pub ring: f32,
    /// the model's front (+z) in the world (x, z)
    pub front: Vec2,
    /// base of the lift arrow (above the object's top)
    pub top: Vec3,
    /// the corners are big enough on screen to grab
    pub corners: bool,
}

const RING_SEGS: usize = 72;
/// Screen sizes (logical px).
const KNOB_R: f32 = 8.0;
const CORNER_R: f32 = 6.0;
const ARROW_LEN: f32 = 46.0;
const MIN_RING_PX: f32 = 44.0;

fn project(cam: &Camera, cam_t: &GlobalTransform, p: Vec3) -> Option<Vec2> {
    cam.world_to_viewport(cam_t, p).ok()
}

/// The handles of object `p` as seen from the camera.
pub fn gizmo(
    p: &Placed,
    e: &LibraryEntry,
    bounds: &ModelBounds,
    cam: &Camera,
    cam_t: &GlobalTransform,
) -> Gizmo {
    gizmo_b(p, bounds.of(e), cam, cam_t)
}

/// The handles of an object with local bounds `b` (a set piece's frame uses unit bounds).
pub fn gizmo_b(p: &Placed, b: (Vec3, Vec3), cam: &Camera, cam_t: &GlobalTransform) -> Gizmo {
    let fp = Footprint::of(p, b);
    let pivot = p.translation();
    let reach = fp
        .corners()
        .iter()
        .map(|c| c.distance(pivot.xz()))
        .fold(0.0f32, f32::max);
    let mut ring = (reach * 1.1 + 0.3).max(0.6);
    // never smaller on screen than a comfortable grab
    let right = cam_t.right().as_vec3();
    if let (Some(a), Some(c)) = (
        project(cam, cam_t, pivot),
        project(cam, cam_t, pivot + right * ring),
    ) {
        let px = a.distance(c);
        if px > 1.0 && px < MIN_RING_PX {
            ring *= MIN_RING_PX / px;
        }
    }
    let corners = {
        let c = fp
            .corners()
            .map(|c| project(cam, cam_t, Vec3::new(c.x, pivot.y, c.y)));
        match (c[0], c[2]) {
            (Some(a), Some(b)) => a.distance(b) > 46.0,
            _ => false,
        }
    };
    let front3 = p.rotation() * Vec3::Z;
    let front = Vec2::new(front3.x, front3.z)
        .try_normalize()
        .unwrap_or(Vec2::Y);
    let top = pivot + Vec3::Y * (b.1.y * p.scale3().y + 0.4).max(0.6);
    Gizmo {
        pivot,
        fp,
        ring,
        front,
        top,
        corners,
    }
}

impl Gizmo {
    pub fn knob(&self) -> Vec3 {
        self.pivot + Vec3::new(self.front.x, 0.0, self.front.y) * self.ring
    }
    pub fn corner(&self, k: usize) -> Vec3 {
        let c = self.fp.corners()[k];
        Vec3::new(c.x, self.pivot.y, c.y)
    }
    fn ring_points(&self) -> Vec<Vec3> {
        (0..=RING_SEGS)
            .map(|i| {
                let a = i as f32 / RING_SEGS as f32 * std::f32::consts::TAU;
                self.pivot + Vec3::new(a.cos(), 0.0, a.sin()) * self.ring
            })
            .collect()
    }
    /// The lift arrow on screen: base and tip.
    fn arrow(&self, cam: &Camera, cam_t: &GlobalTransform) -> Option<(Vec2, Vec2)> {
        let a = project(cam, cam_t, self.top)?;
        Some((a, a - Vec2::Y * ARROW_LEN))
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * u)
}

/// The grip under the cursor `at` (screen), if any.
pub fn hit(g: &Gizmo, cam: &Camera, cam_t: &GlobalTransform, at: Vec2) -> Option<Grip> {
    if project(cam, cam_t, g.knob()).is_some_and(|k| k.distance(at) < KNOB_R + 5.0) {
        return Some(Grip::Turn);
    }
    if g.corners {
        for k in 0..4 {
            if project(cam, cam_t, g.corner(k)).is_some_and(|c| c.distance(at) < CORNER_R + 5.0) {
                return Some(Grip::Corner(k));
            }
        }
    }
    if let Some((a, b)) = g.arrow(cam, cam_t)
        && seg_dist(at, a, b - Vec2::Y * 6.0) < 9.0
    {
        return Some(Grip::Lift);
    }
    let ring: Vec<Option<Vec2>> = g
        .ring_points()
        .into_iter()
        .map(|p| project(cam, cam_t, p))
        .collect();
    if ring
        .windows(2)
        .any(|w| matches!((w[0], w[1]), (Some(a), Some(b)) if seg_dist(at, a, b) < 6.0))
    {
        return Some(Grip::Turn);
    }
    None
}

/// Where a ray meets the horizontal plane at height `y`.
fn on_plane(ray: Ray3d, y: f32) -> Option<Vec3> {
    let d = ray.direction.y;
    if d.abs() < 1e-5 {
        return None;
    }
    let t = (y - ray.origin.y) / d;
    (t > 0.0).then(|| ray.origin + *ray.direction * t)
}

/// The object under a ray (its model's box, small things grown to a grabbable size), nearest
/// first and never behind the ground the ray hits.
pub fn pick_box(
    store: &ObjectStore,
    lib: &Library,
    bounds: &ModelBounds,
    ray: Option<Ray3d>,
    ground: Option<Vec3>,
) -> Option<u64> {
    let ray = ray?;
    let limit = ground
        .map(|g| g.distance(ray.origin) + 1.0)
        .unwrap_or(4000.0)
        .min(4000.0);
    let mut best: Option<(f32, u64)> = None;
    let mut t = 0.0;
    let mut after_hit = 0;
    while t < limit + 40.0 && after_hit < 2 {
        let c = ray.origin + *ray.direction * t;
        for id in store.query(Vec2::new(c.x, c.z), 40.0) {
            let Some(o) = store.items.get(&id) else {
                continue;
            };
            let Some(e) = lib.get(&o.asset) else { continue };
            let (lo, hi) = bounds.of(e);
            let s = o.scale3();
            let (mut lo, mut hi) = (lo * s, hi * s);
            let grow = (Vec3::splat(0.35) - (hi - lo) * 0.5).max(Vec3::ZERO);
            lo -= grow;
            hi += grow;
            let inv = o.rotation().inverse();
            let ro = inv * (ray.origin - o.translation());
            let rd = inv * *ray.direction;
            let (mut t0, mut t1) = (0.0f32, f32::MAX);
            let mut ok = true;
            for a in 0..3 {
                if rd[a].abs() < 1e-8 {
                    if ro[a] < lo[a] || ro[a] > hi[a] {
                        ok = false;
                        break;
                    }
                    continue;
                }
                let (u, v) = ((lo[a] - ro[a]) / rd[a], (hi[a] - ro[a]) / rd[a]);
                t0 = t0.max(u.min(v));
                t1 = t1.min(u.max(v));
            }
            if ok && t0 <= t1 && t0 < limit && best.is_none_or(|(b, _)| t0 < b) {
                best = Some((t0, id));
            }
        }
        if best.is_some() {
            after_hit += 1;
        }
        t += 30.0;
    }
    best.map(|b| b.1)
}

/// What the cursor is doing during a drag.
pub struct Cursor<'a> {
    pub ray: Option<Ray3d>,
    /// where it meets the terrain (or a cave's rock)
    pub ground: Option<Vec3>,
    /// `ground` is on a cave's rock: the caves, and the rock's normal there
    pub cave: Option<(&'a crate::caves::render::CaveWorld, Vec3)>,
    pub screen: Vec2,
    /// metres of height per pixel of vertical cursor travel at the object (lifting)
    pub m_per_px: f32,
    pub shift: bool,
}

/// Metres per pixel of an upright metre at `p` on screen, so dragging up by a pixel lifts by it.
fn metres_per_px(cam: &Camera, cam_t: &GlobalTransform, p: Vec3) -> f32 {
    match (project(cam, cam_t, p), project(cam, cam_t, p + Vec3::Y)) {
        (Some(a), Some(b)) => 1.0 / a.distance(b).max(4.0),
        _ => 0.05,
    }
}

/// The object as a drag leaves it, and the readout.
#[allow(clippy::too_many_arguments)]
pub fn drag_to(
    d: &Drag,
    c: &Cursor<'_>,
    e: &LibraryEntry,
    bounds: &ModelBounds,
    terrain: &Terrain,
    water: &WaterBodies,
) -> (Placed, String) {
    let s = &d.start;
    let pivot = s.translation();
    let mut p = s.clone();
    let label;
    match d.grip {
        Grip::Body => {
            label = String::new();
            if let Some(g) = c.ground {
                let t = g + (pivot - d.grab);
                let ground = match c.cave {
                    // in a cave: on its floor there (the terrain is the ground over it), else on
                    // the floor in front of the rock the cursor is on
                    Some((cw, n)) => {
                        let on_floor = |q: Vec3| {
                            cw.floor_under(Vec3::new(q.x, g.y.max(t.y) + 1.2, q.z), 30.0)
                                .map(|y| Vec3::new(q.x, y, q.z))
                        };
                        on_floor(t).or_else(|| on_floor(g + n * 0.5)).unwrap_or(g) - Vec3::Y * 0.05
                    }
                    None => Vec3::new(t.x, terrain.height_at(t.x, t.z) - 0.05, t.z),
                };
                p.pos = e.seat(ground, water, terrain.sea_level).to_array();
            }
        }
        Grip::Turn => {
            let q = c.ray.and_then(|r| on_plane(r, pivot.y)).unwrap_or(d.grab);
            let ang = |v: Vec3| (v.z - pivot.z).atan2(v.x - pivot.x);
            let mut delta = ang(q) - ang(d.grab);
            delta = (delta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            // (a turn of the cursor by +a about the vertical is a rotation by −a about +y here)
            let mut yaw = s.yaw() - delta;
            if c.shift {
                let step = 15f32.to_radians();
                yaw = (yaw / step).round() * step;
            }
            p.rot = (Quat::from_rotation_y(yaw - s.yaw()) * s.rotation()).to_array();
            label = format!("{:.0}°", crate::mcp_tools::compass(-yaw.to_degrees()));
        }
        Grip::Corner(k) => {
            let q = c
                .ray
                .and_then(|r| on_plane(r, pivot.y))
                .unwrap_or(d.grab)
                .xz()
                - pivot.xz();
            let fp0 = Footprint::of(s, bounds.of(e));
            let v0 = fp0.corners()[k] - pivot.xz();
            let s3 = s.scale3();
            // always in proportion (the object keeps its shape); Shift snaps the resulting size
            // to ×0.25 steps
            let mut k = (q.dot(v0) / v0.length_squared().max(1e-4))
                .clamp(0.05 / s3.min_element(), 20.0 / s3.max_element());
            if c.shift {
                let target = ((s.scale * k) / 0.25).round().max(1.0) * 0.25;
                k = target / s.scale.max(1e-4);
            }
            p.set_scale3(s3 * k);
            label = format!("×{:.2}", p.scale);
        }
        Grip::Lift => {
            let mut dy = (d.screen0.y - c.screen.y) * c.m_per_px;
            if c.shift {
                dy = (dy / 0.25).round() * 0.25;
            }
            p.pos[1] = s.pos[1] + dy;
            label = format!("{}{:.2} m", if dy >= 0.0 { "+" } else { "−" }, dy.abs());
        }
    }
    (p, label)
}

/// A set piece's drag: the change to apply to its pieces, the frame as it now stands, and the
/// readout (moves follow the ground under the cursor; turns, resizes and lifts like one object).
pub fn group_drag_to(d: &Drag, c: &Cursor<'_>) -> (super::group::Change, Placed, String) {
    let s = &d.start;
    let pivot = s.translation();
    let mut ch = super::group::Change::none(pivot);
    let mut f = s.clone();
    let label;
    match d.grip {
        Grip::Body => {
            label = String::new();
            if let Some(g) = c.ground {
                ch.shift = (g - d.grab).xz();
                f.pos = (pivot + Vec3::new(ch.shift.x, 0.0, ch.shift.y)).to_array();
            }
        }
        Grip::Turn => {
            let q = c.ray.and_then(|r| on_plane(r, pivot.y)).unwrap_or(d.grab);
            let ang = |v: Vec3| (v.z - pivot.z).atan2(v.x - pivot.x);
            let mut delta = ang(q) - ang(d.grab);
            delta = (delta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            let mut yaw = -delta;
            if c.shift {
                let step = 15f32.to_radians();
                yaw = (yaw / step).round() * step;
            }
            ch.yaw = yaw;
            f.rot = Quat::from_rotation_y(yaw).to_array();
            label = format!("{:.0}°", crate::mcp_tools::compass(-yaw.to_degrees()));
        }
        Grip::Corner(k) => {
            let q = c
                .ray
                .and_then(|r| on_plane(r, pivot.y))
                .unwrap_or(d.grab)
                .xz()
                - pivot.xz();
            let fp0 = Footprint::of(s, super::group::FRAME_BOUNDS);
            let v0 = fp0.corners()[k] - pivot.xz();
            let mut k = (q.dot(v0) / v0.length_squared().max(1e-4)).clamp(0.1, 10.0);
            if c.shift {
                k = ((k / 0.25).round() * 0.25).max(0.25);
            }
            ch.k = k;
            f.set_scale3(s.scale3() * k);
            label = format!("×{k:.2}");
        }
        Grip::Lift => {
            let mut dy = (d.screen0.y - c.screen.y) * c.m_per_px;
            if c.shift {
                dy = (dy / 0.25).round() * 0.25;
            }
            ch.lift = dy;
            f.pos[1] = s.pos[1] + dy;
            label = format!("{}{:.2} m", if dy >= 0.0 { "+" } else { "−" }, dy.abs());
        }
    }
    (ch, f, label)
}

fn moved(a: &Placed, b: &Placed) -> bool {
    a.pos != b.pos || a.rot != b.rot || a.scale != b.scale || a.stretch != b.stretch
}

/// Grabbing, dragging and releasing the handles (before the place / select tools run).
#[allow(clippy::too_many_arguments)]
pub(crate) fn input(
    stroke: Res<ToolStroke>,
    ed: Res<Editor>,
    pointer: Res<Pointer>,
    keys: Res<ButtonInput<KeyCode>>,
    lib: Option<Res<Library>>,
    bounds: Res<ModelBounds>,
    mut terrain: ResMut<Terrain>,
    water: Res<WaterBodies>,
    mut found: ResMut<super::Foundations>,
    mut store: ResMut<ObjectStore>,
    mut history: ResMut<History>,
    mut handles: ResMut<Handles>,
    sets: Res<crate::setpieces::SetPieceSettings>,
    scenes: Res<crate::scenes::Scenes>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    caves: Res<crate::caves::render::CaveWorld>,
) {
    if stroke.just_started {
        handles.took_press = false;
        handles.drag = None;
        handles.blocked = None;
    }
    if ed.tab != Tab::Objects {
        handles.drag = None;
        return;
    }
    let (Some(lib), Ok((cam, cam_t))) = (lib, camera.single()) else {
        return;
    };
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    if stroke.just_started {
        let at = pointer.screen.unwrap_or(Vec2::splat(-1e4));
        // a grip of the selection first, then any object under the cursor
        let grip = store
            .selected
            .filter(|&id| !store.is_locked(id))
            .and_then(|id| {
                let p = store.items.get(&id)?;
                let g = if p.group != 0 {
                    gizmo_b(
                        &super::group::frame(&store, &lib, &bounds, p.group)?,
                        super::group::FRAME_BOUNDS,
                        cam,
                        cam_t,
                    )
                } else {
                    gizmo(p, lib.get(&p.asset)?, &bounds, cam, cam_t)
                };
                hit(&g, cam, cam_t, at).map(|grip| (id, grip))
            });
        // (while a set piece's area is being drawn, a loop may start over other objects)
        let drawing_set =
            ed.object_tool == ObjectTool::Place && (sets.selected.is_some() || scenes.capturing());
        let target = grip.or_else(|| {
            if drawing_set {
                None
            } else {
                pick_box(&store, &lib, &bounds, pointer.ray, pointer.hit)
                    .map(|id| (id, if alt { Grip::Lift } else { Grip::Body }))
            }
        });
        match target {
            Some((id, _)) if store.is_locked(id) => {
                // locked: selected, never dragged
                store.selected = Some(id);
                handles.took_press = true;
                handles.blocked = Some(at);
            }
            Some((id, grip)) => {
                let Some(p) = store.items.get(&id).cloned() else {
                    return;
                };
                store.selected = Some(id);
                handles.took_press = true;
                // a set piece's piece: the whole set, through its frame
                let group = (p.group != 0)
                    .then(|| super::group::frame(&store, &lib, &bounds, p.group))
                    .flatten()
                    .map(|f| GroupDrag {
                        id: p.group,
                        starts: super::group::snapshot(&store, p.group),
                        now: f,
                    });
                let base = group.as_ref().map(|g| g.now.clone()).unwrap_or(p);
                let grab = match grip {
                    Grip::Body => pointer.hit.unwrap_or(base.translation()),
                    _ => pointer
                        .ray
                        .and_then(|r| on_plane(r, base.pos[1]))
                        .unwrap_or(base.translation()),
                };
                history.set_label(if group.is_some() {
                    grip.group_label()
                } else {
                    grip.label()
                });
                handles.drag = Some(Drag {
                    grip,
                    id,
                    start: base,
                    grab,
                    screen0: at,
                    label: String::new(),
                    group,
                });
            }
            // open ground: Select & Move drops the selection, Place places
            None if ed.object_tool == ObjectTool::Select => store.selected = None,
            None => {}
        }
    } else if stroke.active
        && let Some(d) = handles.drag.as_ref().filter(|d| d.group.is_some())
    {
        // a set piece: every piece moved in place from where it started
        let c = Cursor {
            ray: pointer.ray,
            ground: pointer.hit,
            cave: pointer.cave_normal.map(|n| (&*caves, n)),
            screen: pointer.screen.unwrap_or(d.screen0),
            m_per_px: metres_per_px(cam, cam_t, d.start.translation()),
            shift,
        };
        let (ch, frame, label) = group_drag_to(d, &c);
        let g = d.group.as_ref().expect("a group drag");
        if moved(&frame, &g.now) {
            let (gid, starts) = (g.id, g.starts.clone());
            super::group::transform(&mut store, &mut history, &terrain, gid, &starts, ch);
        }
        if let Some(h) = handles.drag.as_mut() {
            h.label = label;
            if let Some(g) = h.group.as_mut() {
                g.now = frame;
            }
        }
    } else if stroke.active
        && let Some(d) = handles.drag.clone()
    {
        let (Some(cur), Some(e)) = (store.items.get(&d.id).cloned(), lib.get(&d.start.asset))
        else {
            handles.drag = None;
            return;
        };
        let c = Cursor {
            ray: pointer.ray,
            ground: pointer.hit,
            cave: pointer.cave_normal.map(|n| (&*caves, n)),
            screen: pointer.screen.unwrap_or(d.screen0),
            m_per_px: metres_per_px(cam, cam_t, d.start.translation()),
            shift,
        };
        let (np, label) = drag_to(&d, &c, e, &bounds, &terrain, &water);
        if moved(&np, &cur) {
            history.touch_object(&store, d.id);
            store.update(np);
            store.selected = Some(d.id);
        }
        if let Some(h) = handles.drag.as_mut() {
            h.label = label;
        }
    }
    if stroke.just_ended
        && let Some(d) = handles.drag.take()
    {
        release(
            &d,
            &mut super::FitWorld {
                terrain: &mut terrain,
                history: &mut history,
                store: &mut store,
                lib: &lib,
                water: &water,
                found: &mut found,
                bounds: &bounds,
            },
        );
    }
}

/// The end of a drag (inside its undo step): a lifted or lowered fitted building takes its
/// levelled ground along. (Moves, turns and resizes are fitted again by `fit::auto_fit`.)
fn release(d: &Drag, w: &mut super::FitWorld) {
    if d.grip == Grip::Lift {
        super::fit::follow_lift(w, d.id);
    }
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

/// Draws the selected object's handles (and the drag's readout).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    mut contexts: EguiContexts,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    bounds: Res<ModelBounds>,
    handles: Res<Handles>,
    pointer: Res<Pointer>,
    found: Res<super::Foundations>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) -> Result {
    if ed.tab != Tab::Objects || walk.active || walk.cinematic {
        return Ok(());
    }
    let (Some(lib), Ok((cam, cam_t))) = (lib, camera.single()) else {
        return Ok(());
    };
    let Some(sel) = store.selected.and_then(|id| store.items.get(&id)) else {
        return Ok(());
    };
    // a locked selection has no handles (`lock::draw` outlines it); dragging it says why not
    if store.is_locked(sel.id) {
        if let (Some(at), Some(now)) = (handles.blocked, pointer.screen)
            && at.distance(now) > 6.0
            && let Ok(ctx) = contexts.ctx_mut()
        {
            ctx.set_cursor_icon(egui::CursorIcon::NotAllowed);
        }
        return Ok(());
    }
    // a set piece shows its whole set's handles (while dragged: the frame as it now stands)
    let frame = match (
        sel.group,
        handles.drag.as_ref().and_then(|d| d.group.as_ref()),
    ) {
        (0, _) => None,
        (_, Some(g)) => Some(g.now.clone()),
        (g, None) => super::group::frame(&store, &lib, &bounds, g),
    };
    let p = frame.as_ref().unwrap_or(sel);
    let b = match &frame {
        Some(_) => super::group::FRAME_BOUNDS,
        None => match lib.get(&p.asset) {
            Some(e) => bounds.of(e),
            None => return Ok(()),
        },
    };
    let ctx = contexts.ctx_mut()?.clone();
    if !ctx.fonts(|f| f.definitions().families.contains_key(&semibold())) {
        return Ok(());
    }
    // viewport pixels → egui points (the UI may be zoomed)
    let k = 1.0 / ctx.zoom_factor();
    let pt = |v: Vec2| pos2(v.x * k, v.y * k);
    let proj = |w: Vec3| project(cam, cam_t, w).map(pt);
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("object_handles")));
    let g = gizmo_b(p, b, cam, cam_t);
    let active = handles.drag.as_ref().map(|d| d.grip);
    let hover = if active.is_none() && !pointer.over_ui {
        pointer.screen.and_then(|s| hit(&g, cam, cam_t, s))
    } else {
        None
    };
    let hot = |grip: Grip| active == Some(grip) || hover == Some(grip);
    let shadow = Color32::from_black_alpha(110);
    let line = |pts: Vec<Pos2>, w: f32, c: Color32| {
        painter.add(Shape::line(pts.clone(), Stroke::new((w + 2.5) * k, shadow)));
        painter.add(Shape::line(pts, Stroke::new(w * k, c)));
    };

    // the footprint (dashed while it stands on levelled ground)
    let outline: Vec<Pos2> =
        g.fp.corners()
            .iter()
            .chain(std::iter::once(&g.fp.corners()[0]))
            .filter_map(|c| proj(Vec3::new(c.x, g.pivot.y, c.y)))
            .collect();
    if outline.len() == 5 {
        let col = with_alpha(Color32::WHITE, 0.55);
        if found.pads.contains_key(&p.id) {
            painter.add(Shape::dashed_line(
                &outline,
                Stroke::new(1.2 * k, col),
                6.0 * k,
                4.0 * k,
            ));
        } else {
            painter.add(Shape::line(outline, Stroke::new(1.0 * k, col)));
        }
    }
    // the turning ring, and the sweep of the turn while dragging
    let ring: Vec<Pos2> = g.ring_points().into_iter().filter_map(proj).collect();
    let turning = hot(Grip::Turn);
    line(
        ring,
        if turning { 2.6 } else { 1.8 },
        if turning {
            ACCENT
        } else {
            with_alpha(Color32::WHITE, 0.9)
        },
    );
    if let Some(d) = handles.drag.as_ref().filter(|d| d.grip == Grip::Turn) {
        let (a0, a1) = (
            (d.start.rotation() * Vec3::Z).xz(),
            (p.rotation() * Vec3::Z).xz(),
        );
        let (s0, s1) = (a0.y.atan2(a0.x), a1.y.atan2(a1.x));
        let sweep = (s1 - s0 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let n = ((sweep.abs() / 0.05).ceil() as usize).max(2);
        let mut fan = vec![];
        if let Some(c) = proj(g.pivot) {
            fan.push(c);
        }
        for i in 0..=n {
            let a = s0 + sweep * i as f32 / n as f32;
            fan.extend(proj(g.pivot + Vec3::new(a.cos(), 0.0, a.sin()) * g.ring));
        }
        if fan.len() > 2 {
            // where it faced and where it faces now
            let (c, first, last) = (fan[0], fan[1], fan[fan.len() - 1]);
            painter.add(Shape::convex_polygon(
                fan,
                with_alpha(ACCENT, 0.3),
                Stroke::NONE,
            ));
            painter.add(Shape::dashed_line(
                &[c, first],
                Stroke::new(1.5 * k, with_alpha(Color32::WHITE, 0.8)),
                5.0 * k,
                4.0 * k,
            ));
            painter.add(Shape::line_segment([c, last], Stroke::new(2.0 * k, ACCENT)));
        }
    }
    // the knob at the object's front
    if let Some(c) = proj(g.knob()) {
        let r = if turning { KNOB_R + 1.5 } else { KNOB_R } * k;
        painter.circle_filled(c, r + 1.5 * k, shadow);
        if turning {
            painter.circle_stroke(
                c,
                r + 3.5 * k,
                Stroke::new(2.0 * k, with_alpha(ACCENT, 0.35)),
            );
        }
        painter.circle_filled(c, r, ACCENT);
        painter.text(
            c,
            Align2::CENTER_CENTER,
            egui_phosphor::regular::ARROWS_CLOCKWISE,
            FontId::proportional(10.5 * k),
            ON_ACCENT,
        );
    }
    // resize corners
    if g.corners {
        for i in 0..4 {
            let Some(c) = proj(g.corner(i)) else { continue };
            let on = hot(Grip::Corner(i));
            let r = if on { CORNER_R + 1.5 } else { CORNER_R } * k;
            let rect = egui::Rect::from_center_size(c, vec2(r * 2.0, r * 2.0));
            painter.rect_filled(
                rect.expand(1.5 * k),
                CornerRadius::same((2.5 * k) as u8),
                shadow,
            );
            if on {
                painter.rect_stroke(
                    rect.expand(3.5 * k),
                    CornerRadius::same((3.5 * k) as u8),
                    Stroke::new(2.0 * k, with_alpha(ACCENT, 0.35)),
                    egui::StrokeKind::Middle,
                );
            }
            painter.rect_filled(rect, CornerRadius::same((2.0 * k) as u8), ACCENT);
        }
    }
    // the lift arrow above the top
    if let Some((a, b)) = g.arrow(cam, cam_t) {
        let on = hot(Grip::Lift);
        let col = if on { ACCENT } else { Color32::WHITE };
        let (a, b) = (pt(a), pt(b));
        line(vec![a, b], if on { 2.6 } else { 2.0 }, col);
        let head = |tip: Pos2, dir: f32| {
            let w = 6.0 * k;
            vec![
                tip,
                pos2(tip.x - w, tip.y + dir * w * 1.3),
                pos2(tip.x + w, tip.y + dir * w * 1.3),
            ]
        };
        painter.add(Shape::convex_polygon(
            head(b, 1.0),
            col,
            Stroke::new(1.0 * k, shadow),
        ));
        painter.add(Shape::convex_polygon(
            head(a + vec2(0.0, 6.0 * k), -1.0),
            col,
            Stroke::new(1.0 * k, shadow),
        ));
    }
    // the readout next to the cursor
    if let Some(d) = handles.drag.as_ref().filter(|d| !d.label.is_empty()) {
        let at = pointer.screen.map(pt).or_else(|| {
            proj(match d.grip {
                Grip::Corner(i) => g.corner(i),
                Grip::Lift => g.top,
                _ => g.knob(),
            })
        });
        if let Some(at) = at {
            let font = FontId::new(13.0 * k, semibold());
            let galley = painter.layout_no_wrap(d.label.clone(), font, Color32::WHITE);
            let rect = egui::Rect::from_min_size(
                at + vec2(16.0 * k, -26.0 * k),
                galley.size() + vec2(14.0 * k, 8.0 * k),
            );
            painter.rect_filled(
                rect,
                CornerRadius::same((6.0 * k) as u8),
                Color32::from_rgba_unmultiplied(15, 20, 28, 225),
            );
            painter.galley(rect.min + vec2(7.0 * k, 4.0 * k), galley, Color32::WHITE);
        }
    }
    // the cursor says what a press would do
    let icon = match active.or(hover) {
        Some(Grip::Turn) => Some(if active.is_some() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        }),
        Some(Grip::Corner(i)) => Some(if i % 2 == 0 {
            egui::CursorIcon::ResizeNeSw
        } else {
            egui::CursorIcon::ResizeNwSe
        }),
        Some(Grip::Lift) => Some(egui::CursorIcon::ResizeVertical),
        Some(Grip::Body) => Some(egui::CursorIcon::Move),
        None => None,
    };
    if let Some(i) = icon {
        ctx.set_cursor_icon(i);
    }
    Ok(())
}

/// `WB_HANDLE` (see the module docs).
#[allow(clippy::too_many_arguments)]
pub(super) fn test_hook(
    mut frame: Local<u32>,
    mut ed: ResMut<Editor>,
    lib: Option<Res<Library>>,
    bounds: Res<ModelBounds>,
    mut terrain: ResMut<Terrain>,
    water: Res<WaterBodies>,
    mut found: ResMut<super::Foundations>,
    mut store: ResMut<ObjectStore>,
    mut history: ResMut<History>,
    mut handles: ResMut<Handles>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok(spec) = std::env::var("WB_HANDLE") else {
        return;
    };
    *frame += 1;
    let start: u32 = std::env::var("WB_HANDLE_FRAME")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(430);
    let parts: Vec<&str> = spec.split(':').collect();
    let hold = parts.get(2) == Some(&"hold");
    if *frame == start + 30
        && !hold
        && let (Some(d), Some(lib)) = (handles.drag.take(), lib.as_deref())
    {
        release(
            &d,
            &mut super::FitWorld {
                terrain: &mut terrain,
                history: &mut history,
                store: &mut store,
                lib,
                water: &water,
                found: &mut found,
                bounds: &bounds,
            },
        );
        history.commit(&store);
        info!("WB_HANDLE released");
        return;
    }
    if *frame != start {
        return;
    }
    let (Some(lib), Ok((cam, cam_t))) = (lib, camera.single()) else {
        return;
    };
    let id = std::env::var("WB_HANDLE_ID")
        .ok()
        .and_then(|v| v.parse().ok())
        .or_else(|| store.items.keys().max().copied());
    let Some(p) = id.and_then(|id| store.items.get(&id)).cloned() else {
        return;
    };
    let Some(e) = lib.get(&p.asset) else { return };
    ed.tab = Tab::Objects;
    ed.object_tool = ObjectTool::Select;
    store.selected = Some(p.id);
    if parts[0] == "select" {
        return;
    }
    let nums: Vec<f32> = parts
        .get(1)
        .unwrap_or(&"0")
        .split(',')
        .filter_map(|v| v.trim().parse().ok())
        .collect();
    let v = |i: usize| nums.get(i).copied().unwrap_or(0.0);
    let g = gizmo(&p, e, &bounds, cam, cam_t);
    let pivot = p.translation();
    let screen0 = project(cam, cam_t, g.top).unwrap_or(Vec2::ZERO);
    let (grip, grab, cursor) = match parts[0] {
        "turn" => {
            let grab = g.knob();
            let a0 = (grab.z - pivot.z).atan2(grab.x - pivot.x) + v(0).to_radians();
            (
                Grip::Turn,
                grab,
                pivot + Vec3::new(a0.cos(), 0.0, a0.sin()) * g.ring,
            )
        }
        "scale" | "stretch" => {
            let c = g.corner(2);
            let ky = if parts[0] == "stretch" { v(1) } else { v(0) };
            let l = Vec2::new(
                (c - pivot).xz().dot(g.fp.ax) * v(0),
                (c - pivot).xz().dot(g.fp.az()) * ky,
            );
            (
                Grip::Corner(2),
                c,
                pivot
                    + Vec3::new(
                        g.fp.ax.x * l.x + g.fp.az().x * l.y,
                        0.0,
                        g.fp.ax.y * l.x + g.fp.az().y * l.y,
                    ),
            )
        }
        "lift" => (Grip::Lift, pivot, pivot),
        _ => (Grip::Body, pivot, pivot + Vec3::new(v(0), 0.0, v(1))),
    };
    // the cursor on screen for that point (lifting: dragged up by the height)
    let m_per_px = metres_per_px(cam, cam_t, pivot);
    let screen = if grip == Grip::Lift {
        screen0 - Vec2::Y * (v(0) / m_per_px)
    } else {
        project(cam, cam_t, cursor).unwrap_or(screen0)
    };
    let ray = cam.viewport_to_world(cam_t, screen).ok();
    let ground = if grip == Grip::Body {
        Some(Vec3::new(
            cursor.x,
            terrain.height_at(cursor.x, cursor.z),
            cursor.z,
        ))
    } else {
        None
    };
    history.begin(grip.label());
    let d = Drag {
        grip,
        id: p.id,
        start: p.clone(),
        grab,
        screen0,
        label: String::new(),
        group: None,
    };
    let c = Cursor {
        ray,
        ground,
        cave: None,
        screen,
        m_per_px,
        shift: false,
    };
    let (np, label) = drag_to(&d, &c, e, &bounds, &terrain, &water);
    history.touch_object(&store, p.id);
    store.update(np.clone());
    info!(
        "WB_HANDLE {} on {} → pos {:?} yaw {:.1}° scale {:?} ({label})",
        parts[0],
        p.asset,
        np.pos,
        crate::mcp_tools::compass(-np.yaw().to_degrees()),
        np.scale3()
    );
    handles.drag = Some(Drag { label, ..d });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_hits() {
        let r = Ray3d::new(
            Vec3::new(0.0, 10.0, 0.0),
            Dir3::new(Vec3::new(1.0, -1.0, 0.0)).unwrap(),
        );
        let q = on_plane(r, 0.0).unwrap();
        assert!((q - Vec3::new(10.0, 0.0, 0.0)).length() < 1e-4);
        assert!(on_plane(r, 20.0).is_none());
    }

    fn hall() -> (LibraryEntry, ModelBounds) {
        let e = LibraryEntry {
            id: "sb:hall".into(),
            name: "Hall".into(),
            category: crate::objects::Category::Structures,
            group: "buildings".into(),
            kind: None,
            import_path: None,
            surface: crate::terrain::layers::Surface::Wood,
            radius: 5.5,
            align: false,
            walkable: true,
            whole_scene: false,
            biomes: vec![],
            sway: false,
            default_scale: 1.0,
            meshes: vec![],
            scene: None,
            height: 6.0,
            tree: None,
            textured: false,
            pack: crate::objects::PHOTOREAL.into(),
        };
        let mut b = ModelBounds::default();
        b.0.insert(
            "sb:hall".into(),
            (Vec3::new(-3.0, 0.0, -5.0), Vec3::new(3.0, 6.0, 5.0)),
        );
        (e, b)
    }

    /// A drag of `grip` that began at `grab` with the cursor now over `at` (ray straight down).
    fn drag(grip: Grip, grab: Vec3, at: Vec3, screen_dy: f32, shift: bool) -> (Placed, String) {
        let (e, b) = hall();
        let mut t = Terrain::new(32, 4.0, -10.0);
        t.heights.iter_mut().for_each(|h| *h = 5.0);
        let start = Placed {
            group: 0,
            id: 1,
            asset: "sb:hall".into(),
            variant: 0,
            pos: [0.0, 5.0, 0.0],
            rot: Quat::IDENTITY.to_array(),
            scale: 1.0,
            stretch: None,
            locked: false,
        };
        let d = Drag {
            grip,
            id: 1,
            start,
            grab,
            screen0: Vec2::new(500.0, 500.0),
            label: String::new(),
            group: None,
        };
        let ray = Ray3d::new(at + Vec3::Y * 50.0, Dir3::NEG_Y);
        let c = Cursor {
            ray: Some(ray),
            ground: Some(Vec3::new(at.x, 5.0, at.z)),
            cave: None,
            screen: Vec2::new(500.0, 500.0 + screen_dy),
            m_per_px: 0.02,
            shift,
        };
        drag_to(&d, &c, &e, &b, &t, &WaterBodies::default())
    }

    #[test]
    fn handles_turn_resize_lift_and_move() {
        // the knob (front, +z) dragged a quarter round clockwise seen from above (+z → −x)
        let (p, label) = drag(
            Grip::Turn,
            Vec3::new(0.0, 5.0, 8.0),
            Vec3::new(-8.0, 5.0, 0.0),
            0.0,
            false,
        );
        assert_eq!(label, "90°");
        assert!((p.rotation() * Vec3::Z - Vec3::NEG_X).length() < 1e-4);
        // Shift snaps to 15° steps
        let a = 37f32.to_radians();
        let (p, label) = drag(
            Grip::Turn,
            Vec3::new(0.0, 5.0, 8.0),
            Vec3::new(-8.0 * a.sin(), 5.0, 8.0 * a.cos()),
            0.0,
            true,
        );
        assert_eq!(label, "30°");
        assert!(
            (p.yaw().to_degrees() + 30.0).abs() < 1e-3
                || (p.yaw().to_degrees() - 30.0).abs() < 1e-3
        );
        // a corner pulled out to 1.5× its distance: 1.5× the size
        let (p, label) = drag(
            Grip::Corner(2),
            Vec3::new(3.0, 5.0, 5.0),
            Vec3::new(4.5, 5.0, 7.5),
            0.0,
            false,
        );
        assert_eq!(label, "×1.50");
        assert!((p.scale - 1.5).abs() < 1e-4 && p.stretch.is_none());
        // a corner pulled sideways still resizes in proportion; Shift snaps to ×0.25 steps
        let (p, _) = drag(
            Grip::Corner(2),
            Vec3::new(3.0, 5.0, 5.0),
            Vec3::new(6.0, 5.0, 5.0),
            0.0,
            false,
        );
        assert!(p.stretch.is_none(), "{:?}", p.stretch);
        let (p, label) = drag(
            Grip::Corner(2),
            Vec3::new(3.0, 5.0, 5.0),
            Vec3::new(4.4, 5.0, 7.4),
            0.0,
            true,
        );
        assert!(
            p.stretch.is_none() && (p.scale - 1.5).abs() < 1e-4,
            "{}",
            p.scale
        );
        assert_eq!(label, "×1.50");
        // dragging the arrow up 100 px at 2 cm a pixel lifts it 2 m
        let (p, label) = drag(
            Grip::Lift,
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::ZERO,
            -100.0,
            false,
        );
        assert!((p.pos[1] - 7.0).abs() < 1e-4);
        assert_eq!(label, "+2.00 m");
        // the body follows the cursor along the ground
        let (p, _) = drag(
            Grip::Body,
            Vec3::new(1.0, 5.0, 1.0),
            Vec3::new(11.0, 5.0, -3.0),
            0.0,
            false,
        );
        assert!(
            (Vec3::from_array(p.pos) - Vec3::new(10.0, 4.95, -4.0)).length() < 1e-4,
            "{:?}",
            p.pos
        );
    }

    #[test]
    fn segment_distance() {
        assert!(
            (seg_dist(
                Vec2::new(0.0, 1.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(1.0, 0.0)
            ) - 1.0)
                .abs()
                < 1e-6
        );
        assert!(
            (seg_dist(
                Vec2::new(3.0, 0.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(1.0, 0.0)
            ) - 2.0)
                .abs()
                < 1e-6
        );
    }
}
