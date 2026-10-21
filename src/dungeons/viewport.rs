//! The view: clicks that pick a way in (in the world) and the builder's drawing on the floor of the
//! level being edited (rooms, halls, corridors, caverns, stairs, pits, pools, daises, bridges),
//! moving and removing rooms, placing lights; outlines and markers over the view; and the
//! requests the panels make, done with the world (`apply_requests`).

use std::sync::Arc;

use bevy::prelude::*;

use super::edit::{self, NewEntrance};
use super::lights::{self, LightDef, LightKind, Mount};
use super::model::*;
use super::plan::{Ceiling, Pillars, Plan, Room, RoomKind, Shape};
use super::render::{DungeonView, DungeonWorld};
use super::session::{self, Session};
use super::ui::{Draw, DungeonTool, Pick, Request, WallDrag, WallMode};
use crate::camera::{MainCamera, Pointer};
use crate::editor::Tab;
use crate::io::WorldData;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct DungeonGizmos;

pub(super) fn setup_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<DungeonGizmos>();
    cfg.depth_bias = -0.6;
    cfg.line.width = 2.0;
}

fn ray_plane(ray: Ray3d, y: f32) -> Option<Vec3> {
    let d = ray.direction.y;
    if d.abs() < 1e-5 {
        return None;
    }
    let t = (y - ray.origin.y) / d;
    (t > 0.0).then(|| ray.origin + *ray.direction * t)
}

/// The piece the pointer's ray passes closest to (doors and gates by their middle, markers by
/// their spot), if any is near enough.
fn piece_near(m: &MapDoc, ray: Option<Ray3d>) -> Option<u32> {
    let ray = ray?;
    m.pieces
        .iter()
        .map(|p| {
            let c = p.position()
                + Vec3::Y
                    * if p.kind.is_marker() {
                        0.3
                    } else {
                        (p.height * 0.5).min(1.2)
                    };
            let t = (c - ray.origin).dot(*ray.direction).max(0.0);
            let reach = (p.width * 0.5).clamp(0.6, 1.6);
            ((ray.origin + *ray.direction * t).distance(c) / reach, p.id)
        })
        .filter(|(d, _)| *d < 1.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|x| x.1)
}

fn remove_piece(id: u32) -> Request {
    Request::Map(
        "Remove piece".into(),
        Box::new(move |m| {
            m.pieces.retain(|x| x.id != id);
            for x in &mut m.pieces {
                x.links.retain(|l| *l != id);
            }
        }),
    )
}

/// A new piece where the click went: doors, gates and bars into the nearest doorway; a lever on
/// the wall clicked; anything else on the floor (`floor`: the floor's height there).
fn place_piece(
    kind: super::pieces::PieceKind,
    at: Vec3,
    normal: Option<Vec3>,
    floor: f32,
    doorways: &[super::mesh::Doorway],
) -> Result<super::pieces::Piece, &'static str> {
    use super::pieces::{Piece, PieceKind, at_doorway, nearest_doorway};
    if kind.in_doorway() {
        return nearest_doorway(doorways, at, 4.0)
            .map(|d| at_doorway(kind, d))
            .ok_or("Click in or near a doorway");
    }
    let n = normal.unwrap_or(Vec3::Y);
    if kind == PieceKind::Lever {
        if n.y.abs() > 0.5 {
            return Err("Click on a wall for a lever");
        }
        let flat = Vec3::new(n.x, 0.0, n.z).normalize_or(Vec3::Z);
        return Ok(Piece::new(
            kind,
            at - Vec3::Y * kind.size().1 * 0.5 + flat * 0.02,
            flat.x.atan2(flat.z),
        ));
    }
    let foot = if n.y > 0.5 {
        at
    } else {
        Vec3::new(at.x, floor, at.z)
    };
    Ok(Piece::new(kind, foot, 0.0))
}

/// A piece dragged toward `to`: doors jump from doorway to doorway, levers slide along the walls,
/// the rest follow the pointer over the floor. Its state, links and label stay.
fn drag_piece(
    p: &super::pieces::Piece,
    to: Vec3,
    hit: Option<Vec3>,
    normal: Option<Vec3>,
    doorways: &[super::mesh::Doorway],
) -> Option<super::pieces::Piece> {
    use super::pieces::{PieceKind, at_doorway, nearest_doorway};
    let mut out = p.clone();
    if p.kind.in_doorway() {
        let d = nearest_doorway(doorways, to, 3.0)?;
        let at = at_doorway(p.kind, d);
        (out.pos, out.yaw, out.width, out.height) = (at.pos, at.yaw, at.width, at.height);
    } else if p.kind == PieceKind::Lever {
        let (hit, n) = (hit?, normal?);
        if n.y.abs() > 0.5 {
            return None;
        }
        let flat = Vec3::new(n.x, 0.0, n.z).normalize_or(Vec3::Z);
        out.pos = (hit - Vec3::Y * p.height * 0.5 + flat * 0.02).to_array();
        out.yaw = flat.x.atan2(flat.z);
    } else {
        out.pos = [to.x, p.pos[1], to.z];
    }
    out.auto = false;
    Some(out)
}

fn snap(p: Vec2, s: f32) -> Vec2 {
    if s <= 0.0 { p } else { (p / s).round() * s }
}

/// Picking a way in (in the world): the click under the cursor.
#[allow(clippy::too_many_arguments)]
pub(super) fn world_pick(
    mut tool: ResMut<DungeonTool>,
    pointer: Res<Pointer>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    map: Res<DungeonMap>,
    store: Res<crate::objects::ObjectStore>,
    lib: Option<Res<crate::objects::Library>>,
    bounds: Res<crate::placement::footprint::ModelBounds>,
    caves: Res<crate::caves::Caves>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
) {
    let Some(pick) = tool.pick else { return };
    if map.0.is_some() || (keys.just_pressed(KeyCode::Escape) && !pointer.keyboard_captured) {
        tool.pick = None;
        tool.new_prop = None;
        return;
    }
    if !buttons.just_pressed(MouseButton::Left) || pointer.over_ui || keys.pressed(KeyCode::Space) {
        return;
    }
    let kind = match pick {
        Pick::New(k) | Pick::Add(_, k) => k,
    };
    let Some(hit) = pointer.hit else { return };
    let eye = cam
        .single()
        .map(|c| c.translation())
        .unwrap_or(hit + Vec3::Z);
    let to_eye = eye - hit;
    let yaw = to_eye.x.atan2(to_eye.z);
    let mut e = NewEntrance {
        kind,
        at: hit,
        yaw,
        color: tool.new_color,
        frame: tool.new_frame.clone(),
        ..default()
    };
    match kind {
        EntranceKind::Portal | EntranceKind::Point => {}
        EntranceKind::Object if tool.new_prop.is_some() => e.prop = tool.new_prop.clone(),
        EntranceKind::Object => {
            let Some(lib) = lib.as_deref() else { return };
            let Some(id) =
                crate::placement::handles::pick_box(&store, lib, &bounds, pointer.ray, pointer.hit)
            else {
                return;
            };
            e.object = Some(id);
        }
        EntranceKind::Cave => {
            let p = Vec2::new(hit.x, hit.z);
            let best = caves
                .list
                .iter()
                .filter(|c| c.kind != crate::caves::CaveKind::Arch)
                .map(|c| {
                    let (lo, hi) = c.bounds();
                    let inside = p.x >= lo.x - 6.0
                        && p.x <= hi.x + 6.0
                        && p.y >= lo.z - 6.0
                        && p.y <= hi.z + 6.0;
                    let d = Vec2::new(c.entrance[0], c.entrance[2]).distance(p);
                    (if inside { d * 0.1 } else { d }, c.id)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match best {
                Some((d, id)) if d < 40.0 => e.cave = Some(id),
                _ => return,
            }
        }
    }
    let req = match pick {
        Pick::New(_) => {
            let t = &*tool;
            Request::Create(edit::NewDungeon {
                name: t.new_name.clone(),
                kind: if t.new_realm {
                    MapKind::Realm
                } else {
                    MapKind::Interior
                },
                layout: t.new_layout.clone(),
                size: t.new_size,
                room_size: t.new_room_size,
                irregularity: t.new_irregular,
                art: t.new_art,
                style: t.new_style.clone(),
                mood: t.new_mood.clone(),
                instancing: t.new_instancing,
                way_in: Some(e),
                shape: t.new_realm.then_some(t.new_land.shape),
                realm_size: t.new_realm.then_some(t.new_land.size),
                seed: t.new_realm.then_some(t.new_land.seed),
                compound: (t.new_realm && t.new_layout == super::compound::LAYOUT)
                    .then_some(t.new_compound),
                sheet: (t.new_realm && t.new_layout == super::sheets::LAYOUT)
                    .then(|| t.new_sheet.clone())
                    .flatten(),
            })
        }
        Pick::Add(id, _) => Request::AddEntrance(id, e),
    };
    tool.requests.push(req);
    tool.pick = None;
    tool.new_prop = None;
}

/// Drawing in a dungeon's map: the floor of the edited level under the cursor.
#[allow(clippy::too_many_arguments)]
pub(super) fn builder_input(
    mut tool: ResMut<DungeonTool>,
    mut map: ResMut<DungeonMap>,
    view: Res<DungeonView>,
    ed: Res<crate::editor::Editor>,
    pointer: Res<Pointer>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    walk: Res<crate::walk::WalkState>,
    time: Res<Time<Real>>,
    mut last_click: Local<f32>,
    mut meta: ResMut<crate::world::WorldMeta>,
    world: Res<DungeonWorld>,
    (portals, store, gameplay): (
        Res<super::portal::Portals>,
        Res<crate::objects::ObjectStore>,
        Res<crate::gameplay::Gameplay>,
    ),
) {
    if !map.interior() || walk.active || !matches!(ed.tab, Tab::Dungeons | Tab::DungeonLights) {
        tool.hover = None;
        tool.moving_piece = None;
        tool.moving_exit = None;
        return;
    }
    let Some(m) = map.0.as_ref() else { return };
    let level_y = m.plan.level_y(view.level);
    let raw = pointer
        .ray
        .and_then(|r| ray_plane(r, level_y + 0.02))
        .map(|p| Vec2::new(p.x, p.z));
    let free = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
    let p = raw.map(|p| if free { p } else { snap(p, tool.snap) });
    tool.hover = p;
    let typing = pointer.keyboard_captured;
    if !typing && keys.just_pressed(KeyCode::Escape) {
        tool.path.clear();
        tool.drag_from = None;
        tool.placing_light = None;
        tool.placing_decal = None;
        tool.placing_piece = None;
        tool.moving = None;
        tool.moving_piece = None;
        tool.moving_exit = None;
    }
    // ---- decals (the Look tab)
    if ed.tab == Tab::DungeonLook {
        let Some(kind) = tool.placing_decal else {
            return;
        };
        if !buttons.just_pressed(MouseButton::Left) || pointer.over_ui {
            return;
        }
        let Some(hit) = pointer.hit else { return };
        if free {
            // Alt-click: the nearest decal goes
            if let Some(id) = m
                .decals
                .iter()
                .map(|x| (Vec3::from_array(x.pos).distance(hit), x.id))
                .filter(|(d, _)| *d < 1.2)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|x| x.1)
            {
                tool.requests.push(Request::Map(
                    "Remove decal".into(),
                    Box::new(move |m| m.decals.retain(|x| x.id != id)),
                ));
            }
            return;
        }
        let n = pointer.cave_normal.unwrap_or(Vec3::Y);
        let size = tool.decal_size;
        tool.requests.push(Request::Map(
            format!("Add {}", kind.name().to_lowercase()),
            Box::new(move |m| {
                let mut d = super::decals::Decal::new(kind, hit, n, size);
                d.spin = (hit.x * 12.9898 + hit.z * 78.233).sin().abs() * std::f32::consts::TAU;
                d.id = m.alloc_decal();
                m.decals.push(d);
            }),
        ));
        return;
    }
    // ---- lights (their tab)
    if ed.tab == Tab::DungeonLights {
        // dragging a light: along the floor at its own height (its fixture follows:
        // `fixtures_follow`); one undo step
        if let Some((id, off)) = tool.moving_light {
            if buttons.pressed(MouseButton::Left)
                && let Some(l) = m.lights.iter().find(|l| l.id == id)
                && let Some(ray) = pointer.ray
                && let Some(q) = ray_plane(ray, l.pos[1])
            {
                let mut to = q + off;
                if !free {
                    let s = snap(Vec2::new(to.x, to.z), tool.snap);
                    to = Vec3::new(s.x, to.y, s.y);
                }
                if to.distance(l.position()) > 0.01 {
                    let before = Arc::new(m.clone());
                    if tool.live.is_none() {
                        tool.live = Some(("Move light".into(), before));
                    }
                    if let Some(mm) = map.0.as_mut()
                        && let Some(x) = mm.lights.iter_mut().find(|x| x.id == id)
                    {
                        x.pos = to.to_array();
                        x.auto = false;
                        meta.dirty = true;
                    }
                }
            }
            if buttons.just_released(MouseButton::Left) || !buttons.pressed(MouseButton::Left) {
                tool.moving_light = None;
            }
            return;
        }
        if !buttons.just_pressed(MouseButton::Left) || pointer.over_ui {
            return;
        }
        if let Some(kind) = tool.placing_light {
            let Some(hit) = pointer.hit else { return };
            let n = pointer.cave_normal.unwrap_or(Vec3::Y);
            tool.requests.push(place_light(kind, hit, n));
            return;
        }
        // pick the nearest marker along the ray; holding the button drags it
        if let Some(ray) = pointer.ray {
            let best = m
                .lights
                .iter()
                .map(|l| {
                    let q = l.position() - ray.origin;
                    let t = q.dot(*ray.direction).max(0.0);
                    (
                        (ray.origin + *ray.direction * t).distance(l.position()),
                        l.id,
                    )
                })
                .filter(|(d, _)| *d < 0.9)
                .min_by(|a, b| a.0.total_cmp(&b.0));
            tool.selected_light = best.map(|b| b.1);
            tool.moving_light = best.and_then(|(_, id)| {
                let l = m.lights.iter().find(|l| l.id == id)?;
                Some((id, l.position() - ray_plane(ray, l.pos[1])?))
            });
        }
        return;
    }
    let level = view.level;
    // the floor under a spot of this level (its room's raised floor, if any)
    let floor_y = |q: Vec2| {
        level_y
            + m.plan
                .room_at(level, q)
                .and_then(|id| m.plan.room(id))
                .map(|r| r.floor.max(0.0))
                .unwrap_or(0.0)
    };
    // ---- pieces: place one (Alt-click: remove the nearest)
    if let Some(kind) = tool.placing_piece {
        if !buttons.just_pressed(MouseButton::Left)
            || pointer.over_ui
            || keys.pressed(KeyCode::Space)
        {
            return;
        }
        if free {
            if let Some(id) = piece_near(m, pointer.ray) {
                tool.requests.push(remove_piece(id));
            }
            return;
        }
        let Some(at) = pointer
            .hit
            .or_else(|| raw.map(|q| Vec3::new(q.x, floor_y(q), q.y)))
        else {
            return;
        };
        match place_piece(
            kind,
            at,
            pointer.cave_normal,
            floor_y(Vec2::new(at.x, at.z)),
            &world.doorways,
        ) {
            Ok(piece) => tool.requests.push(Request::Map(
                format!("Add {}", kind.name().to_lowercase()),
                Box::new(move |m| {
                    // a doorway takes one door: the new one replaces whatever stood there
                    if piece.kind.in_doorway() {
                        let gone: Vec<u32> = m
                            .pieces
                            .iter()
                            .filter(|x| {
                                x.kind.in_doorway() && x.position().distance(piece.position()) < 1.0
                            })
                            .map(|x| x.id)
                            .collect();
                        m.pieces.retain(|x| !gone.contains(&x.id));
                        for x in &mut m.pieces {
                            x.links = x
                                .links
                                .iter()
                                .map(|l| if gone.contains(l) { u32::MAX } else { *l })
                                .collect();
                        }
                        let mut piece = piece.clone();
                        piece.id = m.alloc_piece();
                        // (levers and plates that worked the old door work the new one)
                        for x in &mut m.pieces {
                            for l in &mut x.links {
                                if *l == u32::MAX {
                                    *l = piece.id;
                                }
                            }
                        }
                        m.pieces.push(piece);
                    } else {
                        let mut piece = piece.clone();
                        piece.id = m.alloc_piece();
                        m.pieces.push(piece);
                    }
                }),
            )),
            Err(why) => tool.requests.push(Request::Toast(why.into())),
        }
        return;
    }
    let Some(p) = p else { return };
    let draw = tool.draw;
    // keys for the points tools and the picked room
    if !typing {
        if keys.just_pressed(KeyCode::Backspace) && !tool.path.is_empty() {
            tool.path.pop();
        }
        if (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace))
            && tool.path.is_empty()
            && draw == Draw::Select
            && let Some(id) = tool.selected_piece
        {
            tool.requests.push(remove_piece(id));
            tool.selected_piece = None;
        } else if (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace))
            && tool.path.is_empty()
            && draw == Draw::Select
            && let Some(id) = tool.selected_room
        {
            tool.requests.push(Request::Map(
                "Remove room".into(),
                Box::new(move |m| {
                    m.plan.remove(id);
                }),
            ));
            tool.selected_room = None;
        }
        if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
            && draw.by_points()
        {
            finish_path(&mut tool, level);
        }
    }
    if pointer.over_ui
        && tool.drag_from.is_none()
        && tool.moving.is_none()
        && tool.moving_piece.is_none()
        && tool.moving_exit.is_none()
    {
        return;
    }
    let pressed = buttons.just_pressed(MouseButton::Left)
        && !pointer.over_ui
        && !keys.pressed(KeyCode::Space);
    let released = buttons.just_released(MouseButton::Left);
    // dragging a wall (push / pull, an alcove) or a room's height
    if let Some(mut wd) = tool.wall_drag.clone() {
        if buttons.pressed(MouseButton::Left) {
            let changed = match wd.mode {
                WallMode::Height => {
                    let dy = pointer.screen.map_or(0.0, |s| s.y - wd.screen.y);
                    let h = (wd.height - dy * 0.04).clamp(1.5, 60.0);
                    let id = wd.room;
                    m.plan
                        .room(id)
                        .is_some_and(|r| (r.height - h).abs() > 0.01)
                        .then(|| {
                            (
                                Box::new(move |mm: &mut MapDoc| {
                                    if let Some(x) = mm.plan.room_mut(id) {
                                        x.height = h;
                                    }
                                }) as Box<dyn FnOnce(&mut MapDoc)>,
                                "Room height",
                            )
                        })
                }
                _ => {
                    let d = pointer
                        .ray
                        .and_then(|r| ray_plane(r, wd.y))
                        .map_or(wd.d, |q| (Vec2::new(q.x, q.z) - wd.at).dot(wd.out));
                    let d = if free {
                        d
                    } else {
                        (d / tool.snap.max(0.25)).round() * tool.snap.max(0.25)
                    };
                    if (d - wd.d).abs() < 1e-3 {
                        None
                    } else {
                        wd.d = d;
                        let (id, shape) = (wd.room, push_wall(&wd.shape, wd.at, wd.out, d));
                        (wd.mode == WallMode::Push).then(|| {
                            (
                                Box::new(move |mm: &mut MapDoc| {
                                    if let Some(x) = mm.plan.room_mut(id) {
                                        x.set_shape(shape);
                                    }
                                }) as Box<dyn FnOnce(&mut MapDoc)>,
                                "Move wall",
                            )
                        })
                    }
                }
            };
            if let Some((f, label)) = changed {
                let before = Arc::new(m.clone());
                if tool.live.is_none() {
                    tool.live = Some((label.into(), before));
                }
                if let Some(mm) = map.0.as_mut() {
                    f(mm);
                    meta.dirty = true;
                }
            }
            tool.wall_drag = Some(wd);
        } else {
            // let go: an alcove is made as deep as it was pushed
            if wd.mode == WallMode::Alcove
                && wd.d > 0.3
                && let Some(r) = m.plan.room(wd.room)
            {
                let alcove = alcove_room(r, wd.at, wd.out, wd.d);
                tool.requests.push(Request::Map(
                    "Push out an alcove".into(),
                    Box::new(move |m| {
                        m.plan.add(alcove);
                    }),
                ));
            }
            tool.wall_drag = None;
        }
        return;
    }
    match draw {
        Draw::Select => {
            // the way out: dragged along its floor (its frame follows: `portal::way_out_follows`)
            let exit = super::portal::exit_at(m, &store, gameplay.player_start.as_ref());
            if let Some(off) = tool.moving_exit {
                if buttons.pressed(MouseButton::Left)
                    && let Some((foot, yaw)) = exit
                    && let Some(q) = pointer.ray.and_then(|r| ray_plane(r, foot.y))
                {
                    let mut to = q + off;
                    if !free {
                        let s = snap(Vec2::new(to.x, to.z), tool.snap);
                        to = Vec3::new(s.x, to.y, s.y);
                    }
                    if to.distance(foot) > 0.01 {
                        if tool.live.is_none() {
                            tool.live = Some(("Move the way out".into(), Arc::new(m.clone())));
                        }
                        if let Some(mm) = map.0.as_mut() {
                            mm.way_out.at = Some(to.to_array());
                            mm.way_out.yaw = yaw;
                            meta.dirty = true;
                        }
                    }
                }
                if released || !buttons.pressed(MouseButton::Left) {
                    tool.moving_exit = None;
                }
                return;
            }
            // picked up through its sheet, or by its foot on the floor (seen from above)
            if pressed
                && let Some((foot, _)) = exit
                && let Some(ray) = pointer.ray
                && let Some(q) = ray_plane(ray, foot.y)
                && (portals.placed.iter().any(|w| {
                    w.site == super::portal::Site::Exit
                        && super::portal::ray_through(w, ray, pointer.hit)
                }) || Vec2::new(q.x - foot.x, q.z - foot.z).length() < 1.6)
            {
                tool.moving_exit = Some(foot - q);
                tool.selected_room = None;
                tool.selected_piece = None;
                return;
            }
            // a wall under the cursor: push or pull it (Alt: an alcove); Shift: the room's height
            if pressed
                && let Some(hit) = pointer.hit
                && let Some(n) = pointer.cave_normal
                && let Some(screen) = pointer.screen
            {
                let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                let wall = (n.y.abs() < 0.5)
                    .then(|| wall_at(m, level, hit, n))
                    .flatten();
                let height_of = if shift {
                    m.plan
                        .room_at(level, Vec2::new(hit.x, hit.z) + Vec2::new(n.x, n.z) * 0.3)
                } else {
                    None
                };
                if let Some(id) = height_of.or(wall.as_ref().filter(|_| shift).map(|w| w.0))
                    && let Some(r) = m.plan.room(id)
                {
                    tool.selected_room = Some(id);
                    tool.wall_drag = Some(WallDrag {
                        room: id,
                        mode: WallMode::Height,
                        at: Vec2::new(hit.x, hit.z),
                        y: hit.y,
                        out: Vec2::ZERO,
                        shape: r.shape.clone(),
                        height: r.height,
                        screen,
                        d: 0.0,
                    });
                    return;
                }
                if let Some((id, at, out)) = wall
                    && let Some(r) = m.plan.room(id)
                {
                    tool.selected_room = Some(id);
                    tool.selected_piece = None;
                    let alt = keys.pressed(KeyCode::AltLeft) || keys.pressed(KeyCode::AltRight);
                    let shape = r
                        .irregular
                        .as_ref()
                        .map_or_else(|| r.shape.clone(), |i| i.base.clone());
                    tool.wall_drag = Some(WallDrag {
                        room: id,
                        mode: if alt {
                            WallMode::Alcove
                        } else {
                            WallMode::Push
                        },
                        at,
                        y: hit.y,
                        out,
                        shape,
                        height: r.height,
                        screen,
                        d: 0.0,
                    });
                    return;
                }
            }
            if pressed {
                // a piece first (they stand in the rooms), then the room
                if let Some(id) = piece_near(m, pointer.ray)
                    && let Some(pc) = m.pieces.iter().find(|x| x.id == id)
                    && let Some(ray) = pointer.ray
                    && let Some(q) = ray_plane(ray, pc.pos[1])
                {
                    tool.selected_piece = Some(id);
                    tool.selected_room = None;
                    tool.moving_piece = Some((id, pc.position() - q));
                } else {
                    tool.selected_piece = None;
                    let id = m.plan.room_at(level, p);
                    tool.selected_room = id;
                    if let Some(id) = id
                        && let Some(r) = m.plan.room(id)
                    {
                        tool.moving = Some((id, p, r.shape.clone()));
                    }
                }
            }
            if let Some((id, off)) = tool.moving_piece {
                if buttons.pressed(MouseButton::Left)
                    && let Some(pc) = m.pieces.iter().find(|x| x.id == id)
                    && let Some(ray) = pointer.ray
                    && let Some(q) = ray_plane(ray, pc.pos[1])
                {
                    let mut to = q + off;
                    if !free {
                        let s = snap(Vec2::new(to.x, to.z), tool.snap);
                        to = Vec3::new(s.x, to.y, s.y);
                    }
                    if let Some(moved) =
                        drag_piece(pc, to, pointer.hit, pointer.cave_normal, &world.doorways)
                        && moved != *pc
                    {
                        let before = Arc::new(m.clone());
                        if tool.live.is_none() {
                            tool.live = Some(("Move piece".into(), before));
                        }
                        if let Some(mm) = map.0.as_mut()
                            && let Some(x) = mm.pieces.iter_mut().find(|x| x.id == id)
                        {
                            *x = moved;
                            meta.dirty = true;
                        }
                    }
                }
                if released || !buttons.pressed(MouseButton::Left) {
                    tool.moving_piece = None;
                }
                return;
            }
            if let Some((id, from, shape)) = tool.moving.clone() {
                if buttons.pressed(MouseButton::Left) {
                    let d = p - from;
                    if d.length() > 0.01 {
                        let before = Arc::new(m.clone());
                        if tool.live.is_none() {
                            tool.live = Some(("Move room".into(), before));
                        }
                        if let Some(mm) = map.0.as_mut()
                            && let Some(r) = mm.plan.room_mut(id)
                        {
                            let mut s = shape.clone();
                            s.translate(d);
                            if r.shape != s {
                                // (an irregular room's regular shape goes along)
                                r.translate(s.centre() - r.shape.centre());
                                r.shape = s;
                                meta.dirty = true;
                            }
                        }
                    }
                }
                if released || !buttons.pressed(MouseButton::Left) {
                    tool.moving = None;
                }
            }
        }
        Draw::WallDoor | Draw::WallSecret => {
            if pressed {
                let kind = if draw == Draw::WallSecret {
                    super::pieces::PieceKind::SecretDoor
                } else {
                    super::pieces::PieceKind::Door
                };
                // a doorway already there: the door goes in it
                if let Some(hit) = pointer.hit
                    && let Ok(pc) = place_piece(kind, hit, None, hit.y, &world.doorways)
                {
                    tool.requests.push(Request::Map(
                        format!("Add {}", kind.name().to_lowercase()),
                        Box::new(move |m| {
                            let mut pc = pc;
                            pc.id = m.alloc_piece();
                            m.pieces.push(pc);
                        }),
                    ));
                    return;
                }
                let wall = pointer
                    .hit
                    .zip(pointer.cave_normal)
                    .filter(|(_, n)| n.y.abs() < 0.5)
                    .and_then(|(h, n)| wall_at(m, level, h, n));
                match wall
                    .and_then(|(id, at, out)| super::presets::door_opening(&m.plan, id, at, out))
                {
                    Some((room, at)) => {
                        tool.requests.push(Request::Map(
                            format!("Cut a doorway for a {}", kind.name().to_lowercase()),
                            Box::new(move |m| {
                                m.plan.add(room);
                            }),
                        ));
                        tool.pending_doors.push((kind, at, world.want));
                    }
                    None => tool.requests.push(Request::Toast("Click on a wall".into())),
                }
            }
        }
        Draw::WallTorch | Draw::WallLantern => {
            if pressed {
                match pointer
                    .hit
                    .zip(pointer.cave_normal)
                    .filter(|(_, n)| n.y.abs() < 0.5)
                {
                    Some((at, normal)) => {
                        let kind = if draw == Draw::WallTorch {
                            LightKind::Torch
                        } else {
                            LightKind::Lantern
                        };
                        tool.requests
                            .push(Request::WallFixture { kind, at, normal });
                    }
                    None => tool.requests.push(Request::Toast("Click on a wall".into())),
                }
            }
        }
        Draw::Window | Draw::Alcove => {
            if pressed {
                let window = draw == Draw::Window;
                match super::presets::wall_opening(&m.plan, level, p, window) {
                    Some(r) => tool.requests.push(Request::Map(
                        format!("Add {}", if window { "window" } else { "alcove" }),
                        Box::new(move |m| {
                            m.plan.add(r);
                        }),
                    )),
                    None => tool
                        .requests
                        .push(Request::Toast("Click on a wall (within 3 m of one)".into())),
                }
            }
        }
        Draw::Erase => {
            if pressed && let Some(id) = m.plan.room_at(level, p) {
                tool.requests.push(Request::Map(
                    "Remove room".into(),
                    Box::new(move |m| {
                        m.plan.remove(id);
                        m.lights.retain(|l| !(l.auto && l.room == Some(id)));
                    }),
                ));
            }
        }
        d if d.by_points() => {
            if pressed {
                let now = time.elapsed_secs();
                let double = now - *last_click < 0.35
                    && tool.path.last().is_some_and(|q| q.distance(p) < 1.0);
                *last_click = now;
                if double {
                    finish_path(&mut tool, level);
                } else if tool.path.last().is_none_or(|q| q.distance(p) > 0.4) {
                    tool.path.push(p);
                    // stairs and bridges: two ends
                    if matches!(d, Draw::Stairs | Draw::Bridge) && tool.path.len() == 2 {
                        finish_path(&mut tool, level);
                    }
                }
            }
        }
        _ => {
            if pressed {
                tool.drag_from = Some(p);
            }
            if released && let Some(a) = tool.drag_from.take() {
                if draw == Draw::Gallery {
                    let (lo, hi) = (a.min(p), a.max(p));
                    if (hi - lo).min_element() >= 1.0 {
                        tool.requests.push(Request::Map(
                            "Add galleried hall".into(),
                            Box::new(move |m| {
                                let ceiling =
                                    super::styles::get(&m.style).ceiling_for(RoomKind::Hall);
                                super::presets::gallery_hall(
                                    &mut m.plan,
                                    level,
                                    (lo + hi) * 0.5,
                                    hi.x - lo.x,
                                    hi.y - lo.y,
                                    ceiling,
                                );
                            }),
                        ));
                    }
                } else if let Some(mut r) = dragged_room(draw, level, a, p) {
                    if r.kind == RoomKind::Spiral {
                        // a tower 2.4–6 m across the steps, to the level above (or below)
                        if let Shape::Round { r: rad, .. } = &mut r.shape {
                            *rad = rad.clamp(2.4, 6.0);
                        }
                        r.to_level = Some(if tool.stairs_up { level + 1 } else { level - 1 });
                    }
                    tool.requests.push(Request::Map(
                        format!("Add {}", draw.name().to_lowercase()),
                        Box::new(move |m| {
                            let mut r = r;
                            if r.kind == RoomKind::Balcony {
                                balcony_height(&m.plan, &mut r);
                            } else if r.kind != RoomKind::Cavern && r.ceiling != Ceiling::Open {
                                r.ceiling = super::styles::get(&m.style).ceiling_for(r.kind);
                            }
                            m.plan.add(r);
                        }),
                    ));
                }
            }
        }
    }
    let _ = meta;
}

/// A balcony drawn on `level`: at the floor of the level above when the room it's in reaches
/// it (and then on that level, so the cut-away keeps it upstairs), else half way up the room.
fn balcony_height(plan: &Plan, r: &mut Room) {
    let c = r.shape.centre();
    let y = plan.level_y(r.level);
    let host = plan
        .rooms
        .iter()
        .filter(|o| {
            o.level == r.level && o.kind.is_space() && !o.kind.is_pit() && o.shape.sdf(c) > 0.0
        })
        .max_by(|a, b| a.height.total_cmp(&b.height));
    let up = plan
        .levels
        .iter()
        .filter(|l| l.y > y + 0.5)
        .map(|l| (l.id, l.y))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    match (host, up) {
        (Some(h), Some((lid, ly))) if y + h.floor + h.height >= ly + 3.0 => {
            r.level = lid;
            r.floor = 0.0;
        }
        (Some(h), _) => r.floor = h.floor + (h.height * 0.5).clamp(3.0, 12.0),
        _ => r.floor = 4.0,
    }
}

/// The room a drag from `a` to `b` draws (none if too small).
fn dragged_room(draw: Draw, level: i32, a: Vec2, b: Vec2) -> Option<Room> {
    let round = matches!(draw, Draw::Round | Draw::Octagon | Draw::Pit | Draw::Spiral);
    let shape = if round {
        let r = a.distance(b);
        if r < 1.0 {
            return None;
        }
        match draw {
            Draw::Octagon => Shape::Ngon {
                c: a.to_array(),
                r,
                sides: 8,
                yaw: 0.0,
            },
            _ => Shape::Round { c: a.to_array(), r },
        }
    } else {
        let (lo, hi) = (a.min(b), a.max(b));
        if (hi - lo).min_element() < 1.0 {
            return None;
        }
        Shape::rect((lo + hi) * 0.5, (hi - lo) * 0.5)
    };
    let kind = match draw {
        Draw::Hall => RoomKind::Hall,
        Draw::Pit => RoomKind::Pit,
        Draw::Pool => RoomKind::Pool,
        Draw::Dais => RoomKind::Dais,
        Draw::Balcony => RoomKind::Balcony,
        Draw::Spiral => RoomKind::Spiral,
        _ => RoomKind::Room,
    };
    let mut r = Room::new(0, kind, level, shape);
    if kind == RoomKind::Hall {
        r.pillars = Pillars::Rows;
    }
    Some(r)
}

/// Turns the clicked points into a corridor, cavern, stairs or bridge.
fn finish_path(tool: &mut DungeonTool, level: i32) {
    let pts = std::mem::take(&mut tool.path);
    let draw = tool.draw;
    let up = tool.stairs_up;
    let room = match draw {
        Draw::Corridor if pts.len() >= 2 => Some(Room::new(
            0,
            RoomKind::Corridor,
            level,
            Shape::path(&pts, 4.0),
        )),
        Draw::Bridge if pts.len() >= 2 => Some(Room::new(
            0,
            RoomKind::Bridge,
            level,
            Shape::path(&pts[..2], 3.0),
        )),
        Draw::Chasm if pts.len() >= 2 => {
            Some(Room::new(0, RoomKind::Chasm, level, Shape::path(&pts, 9.0)))
        }
        Draw::Stairs if pts.len() >= 2 => {
            let mut r = Room::new(0, RoomKind::Stairs, level, Shape::path(&pts[..2], 3.6));
            r.to_level = Some(if up { level + 1 } else { level - 1 });
            Some(r)
        }
        Draw::Cavern if pts.len() >= 3 => {
            let mut r = Room::new(
                0,
                RoomKind::Cavern,
                level,
                Shape::Poly {
                    pts: pts.iter().map(|p| p.to_array()).collect(),
                },
            );
            r.height = 10.0;
            Some(r)
        }
        _ => None,
    };
    if let Some(r) = room {
        tool.requests.push(Request::Map(
            format!("Add {}", draw.name().to_lowercase()),
            Box::new(move |m| {
                let mut r = r;
                if r.kind != RoomKind::Cavern {
                    r.ceiling = super::styles::get(&m.style).ceiling_for(r.kind);
                }
                m.plan.add(r);
            }),
        ));
    }
}

/// The room whose wall a hit is on: the room just in front of the wall (`normal` faces into it),
/// the wall's nearest point (xz) and its outward direction.
fn wall_at(m: &MapDoc, level: i32, hit: Vec3, normal: Vec3) -> Option<(u32, Vec2, Vec2)> {
    let n = Vec2::new(normal.x, normal.z).normalize_or_zero();
    let h = Vec2::new(hit.x, hit.z);
    m.plan
        .rooms
        .iter()
        .filter(|r| {
            r.level == level
                && r.kind.is_space()
                && !r.kind.is_pit()
                && r.shape.sdf(h + n * 0.3) > 0.0
        })
        .filter_map(|r| {
            let pts = r.shape.outline();
            let k = pts.len();
            (0..k)
                .map(|i| {
                    let (a, b) = (pts[i], pts[(i + 1) % k]);
                    let ab = b - a;
                    let t = ((h - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    let q = a + ab * t;
                    (q.distance(h), q)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .filter(|(d, _)| *d < 1.0)
                .map(|(d, q)| (d, r.id, q))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id, q)| (id, q, -n))
}

/// A room's shape with the wall at `at` (facing `out`) moved `d` metres out (− in): a rect's side,
/// a round room's edge (its far side stays), a passage's width, a drawn outline's edge.
pub(super) fn push_wall(shape: &Shape, at: Vec2, out: Vec2, d: f32) -> Shape {
    let mut s = shape.clone();
    match &mut s {
        Shape::Rect { c, half, yaw } => {
            let ax = [
                super::geom::rot(Vec2::X, *yaw),
                super::geom::rot(Vec2::Y, *yaw),
            ];
            let i = if out.dot(ax[0]).abs() >= out.dot(ax[1]).abs() {
                0
            } else {
                1
            };
            let sign = out.dot(ax[i]).signum();
            let nh = (half[i] + d * 0.5).max(1.0);
            let shift = ax[i] * sign * (nh - half[i]);
            half[i] = nh;
            *c = (Vec2::from_array(*c) + shift).to_array();
        }
        Shape::Round { c, r } | Shape::Ngon { c, r, .. } => {
            let nr = (*r + d * 0.5).max(1.0);
            *c = (Vec2::from_array(*c) + out * (nr - *r)).to_array();
            *r = nr;
        }
        Shape::Path { width, .. } => *width = (*width + d).clamp(1.2, 30.0),
        Shape::Poly { pts } => {
            // the edge nearest the grab moves out
            let k = pts.len();
            if let Some(i) = (0..k).min_by(|&i, &j| {
                let e = |i: usize| {
                    super::geom::seg_dist2(
                        at,
                        Vec2::from_array(pts[i]),
                        Vec2::from_array(pts[(i + 1) % k]),
                    )
                };
                e(i).total_cmp(&e(j))
            }) {
                for v in [i, (i + 1) % k] {
                    pts[v] = (Vec2::from_array(pts[v]) + out * d).to_array();
                }
            }
        }
    }
    s
}

/// An alcove `depth` metres deep pushed out of room `r`'s wall at `at` (facing `out`): a small
/// vaulted room of its own that joins it.
pub(super) fn alcove_room(
    r: &super::plan::Room,
    at: Vec2,
    out: Vec2,
    depth: f32,
) -> super::plan::Room {
    let along = out.perp();
    let c = at + out * (depth * 0.5 - 0.2);
    let mut o = super::plan::Room::new(
        0,
        RoomKind::Room,
        r.level,
        Shape::Rect {
            c: c.to_array(),
            half: [1.3, depth * 0.5 + 0.2],
            yaw: along.y.atan2(along.x),
        },
    );
    o.floor = r.floor;
    o.height = r.height.min(3.2);
    o.ceiling = super::plan::Ceiling::Vault;
    o.style = r.style.clone();
    o.name = "Alcove".into();
    o
}

/// A light placed by hand where the cursor hit (a wall, the floor, or under the ceiling).
fn place_light(kind: LightKind, hit: Vec3, n: Vec3) -> Request {
    Request::Map(
        format!("Add {}", kind.name().to_lowercase()),
        Box::new(move |m| {
            let id = m.alloc_light();
            let wall = n.y.abs() < 0.5;
            let pos = match kind.mount() {
                Mount::Wall if wall => {
                    hit + Vec3::new(n.x, 0.0, n.z).normalize_or_zero() * 0.35 + Vec3::Y * 0.5
                }
                Mount::Wall => hit + Vec3::Y * 2.4,
                Mount::Floor => hit + Vec3::Y * 1.2,
                Mount::Ceiling => hit + Vec3::Y * 5.0,
                Mount::Free => hit + Vec3::Y * 2.0,
            };
            let mut l = LightDef::new(id, kind, pos);
            if kind.is_spot() {
                // (pointing back at where it was clicked from above)
                l.dir = [0.0, 1.2];
            }
            m.lights.push(l);
        }),
    )
}

/// Outlines of the rooms of the edited level, what's being drawn, the lights.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    mut gizmos: Gizmos<DungeonGizmos>,
    map: Res<DungeonMap>,
    tool: Res<DungeonTool>,
    view: Res<DungeonView>,
    ed: Res<crate::editor::Editor>,
    walk: Res<crate::walk::WalkState>,
    portals: Res<super::portal::Portals>,
) {
    if walk.active {
        return;
    }
    // ways in being picked (world): nothing to draw yet but the portals
    let Some(m) = map.0.as_ref().filter(|m| m.kind == MapKind::Interior) else {
        if ed.tab == Tab::Dungeons {
            for w in &portals.placed {
                let c = w.tf.transform_point(Vec3::new(0.0, w.h * 0.5, 0.0));
                gizmos.circle(
                    Isometry3d::new(c, w.tf.rotation),
                    w.w * 0.6,
                    Color::srgba(0.5, 0.7, 1.0, 0.5),
                );
            }
        }
        return;
    };
    if !view.show_lights {
        return;
    }
    let level_y = m.plan.level_y(view.level);
    let on_tab = matches!(ed.tab, Tab::Dungeons | Tab::DungeonLights);
    if on_tab {
        for r in m.plan.rooms.iter().filter(|r| r.level == view.level) {
            let sel = tool.selected_room == Some(r.id);
            let col = if sel {
                Color::srgb(1.0, 0.82, 0.3)
            } else {
                match r.kind {
                    RoomKind::Corridor | RoomKind::Stairs => Color::srgba(0.55, 0.75, 1.0, 0.7),
                    RoomKind::Pit | RoomKind::Pool => Color::srgba(0.3, 0.6, 1.0, 0.8),
                    RoomKind::Dais | RoomKind::Bridge => Color::srgba(0.9, 0.7, 0.4, 0.8),
                    _ => Color::srgba(0.85, 0.88, 0.95, 0.55),
                }
            };
            let y = level_y + r.floor.max(0.0) + 0.06;
            let lp = r.shape.outline();
            let pts: Vec<Vec3> = lp
                .iter()
                .chain(lp.first())
                .map(|p| Vec3::new(p.x, y, p.y))
                .collect();
            gizmos.linestrip(pts, col);
            if let Some(line) = r.shape.centreline()
                && r.kind == RoomKind::Stairs
            {
                let pts: Vec<Vec3> = line.iter().map(|p| Vec3::new(p.x, y + 0.1, p.y)).collect();
                gizmos.arrow(pts[0], *pts.last().unwrap(), col);
            }
        }
        // what's being drawn
        if let Some(h) = tool.hover {
            let c = Vec3::new(h.x, level_y + 0.05, h.y);
            gizmos.line(c - Vec3::X * 0.6, c + Vec3::X * 0.6, Color::WHITE);
            gizmos.line(c - Vec3::Z * 0.6, c + Vec3::Z * 0.6, Color::WHITE);
            if let Some(a) = tool.drag_from
                && let Some(r) = dragged_room(tool.draw, view.level, a, h)
            {
                let lp = r.shape.outline();
                let pts: Vec<Vec3> = lp
                    .iter()
                    .chain(lp.first())
                    .map(|p| Vec3::new(p.x, level_y + 0.08, p.y))
                    .collect();
                gizmos.linestrip(pts, Color::srgb(0.4, 1.0, 0.6));
            }
            if !tool.path.is_empty() {
                let mut pts: Vec<Vec3> = tool
                    .path
                    .iter()
                    .map(|p| Vec3::new(p.x, level_y + 0.08, p.y))
                    .collect();
                pts.push(c);
                gizmos.linestrip(pts, Color::srgb(0.4, 1.0, 0.6));
            }
        }
    }
    // the pieces: markers (nothing else shows them), what levers and plates work, the picked one
    for pc in &m.pieces {
        use super::pieces::PieceKind;
        let p = pc.position() + Vec3::Y * 0.08;
        let flat = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let sel = tool.selected_piece == Some(pc.id) && on_tab;
        let fwd = pc.rotation() * Vec3::Z;
        match pc.kind {
            PieceKind::Arena => {
                gizmos.circle(
                    Isometry3d::new(p, flat),
                    pc.width * 0.5,
                    Color::srgba(1.0, 0.3, 0.25, 0.8),
                );
                gizmos.circle(
                    Isometry3d::new(p, flat),
                    pc.width * 0.5 - 0.3,
                    Color::srgba(1.0, 0.3, 0.25, 0.4),
                );
            }
            PieceKind::Checkpoint => {
                gizmos.circle(
                    Isometry3d::new(p, flat),
                    pc.width * 0.5,
                    Color::srgb(0.35, 1.0, 0.55),
                );
                gizmos.line(p, p + Vec3::Y * 2.2, Color::srgb(0.35, 1.0, 0.55));
            }
            PieceKind::Loot => {
                gizmos.cube(
                    Transform::from_translation(p + Vec3::Y * 0.3)
                        .with_rotation(pc.rotation())
                        .with_scale(Vec3::new(0.9, 0.6, 0.6)),
                    Color::srgb(1.0, 0.8, 0.25),
                );
            }
            PieceKind::Spawn => {
                gizmos.circle(
                    Isometry3d::new(p, flat),
                    pc.width * 0.5,
                    Color::srgb(0.4, 0.8, 1.0),
                );
                gizmos.arrow(
                    p,
                    p + fwd * (pc.width * 0.5 + 0.6),
                    Color::srgb(0.4, 0.8, 1.0),
                );
            }
            _ => {}
        }
        for l in &pc.links {
            if let Some(q) = m.pieces.iter().find(|x| x.id == *l) {
                let a = pc.position() + Vec3::Y * (pc.height * 0.5).min(1.2);
                let b = q.position() + Vec3::Y * (q.height * 0.5).min(1.2);
                gizmos.line(
                    a,
                    b,
                    Color::srgba(
                        1.0,
                        0.62,
                        0.2,
                        if sel || tool.selected_piece == Some(q.id) {
                            0.95
                        } else {
                            0.4
                        },
                    ),
                );
            }
        }
        if sel {
            let h = pc.height.max(0.6);
            let depth = if pc.kind.in_doorway() {
                0.4
            } else {
                pc.width.max(0.4)
            };
            gizmos.cube(
                Transform::from_translation(pc.position() + Vec3::Y * h * 0.5)
                    .with_rotation(pc.rotation())
                    .with_scale(Vec3::new(pc.width.max(0.4) + 0.1, h + 0.1, depth + 0.1)),
                Color::WHITE,
            );
        }
    }
    // the lights
    for l in &m.lights {
        let p = l.position();
        let sel = tool.selected_light == Some(l.id);
        let c = Color::srgb(l.color[0], l.color[1], l.color[2]);
        gizmos.sphere(
            Isometry3d::from_translation(p),
            if sel { 0.45 } else { 0.25 },
            if sel { Color::WHITE } else { c },
        );
        if l.kind.is_spot() || l.cone > 0.0 {
            gizmos.arrow(p, p + l.direction() * 2.5, c);
        }
        if sel {
            gizmos.circle(
                Isometry3d::new(p, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
                l.range,
                c.with_alpha(0.35),
            );
        }
    }
}

/// Records the live edits (sliders, drags) as one undo step once the mouse lets go.
pub(super) fn commit_live(
    mut tool: ResMut<DungeonTool>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut history: ResMut<crate::history::History>,
    map: Res<DungeonMap>,
    list: Res<Dungeons>,
    store: Res<crate::objects::ObjectStore>,
) {
    if buttons.pressed(MouseButton::Left) {
        return;
    }
    if let Some((label, before)) = tool.live.take()
        && let Some(m) = map.0.as_ref()
        && *m != *before
    {
        history.push_ext(label, edit::UNDO_MAP, before, Arc::new(m.clone()));
    }
    if let Some((label, before)) = tool.live_list.take()
        && *list != *before
    {
        history.push_ext(label, edit::UNDO_LIST, before, Arc::new(list.clone()));
    }
    let _ = store;
}

/// Does what the panels and the view asked for, with the world.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_requests(
    mut d: WorldData,
    mut tool: ResMut<DungeonTool>,
    mut session: ResMut<Session>,
    mut view: ResMut<DungeonView>,
    mut walk: ResMut<crate::walk::WalkState>,
    dworld: Res<DungeonWorld>,
    mut actions: MessageWriter<crate::ui::UiAction>,
) {
    // a dungeon waiting for its new world to be saved
    if let Some(id) = session.open_after_save
        && d.meta.path.is_some()
        && d.dungeon_map.0.is_none()
    {
        session.open_after_save = None;
        tool.requests.push(Request::Open(id));
    }
    if tool.requests.is_empty() {
        return;
    }
    let later: Vec<Request> = Vec::new();
    for r in std::mem::take(&mut tool.requests) {
        let res: Result<Option<String>, String> = match r {
            Request::Create(n) => edit::create(&mut d, &n).map(|id| {
                d.dungeons.selected = Some(id);
                Some(format!(
                    "Created {}: press Open builder to build it",
                    d.dungeons
                        .get(id)
                        .map(|x| x.name.clone())
                        .unwrap_or_default()
                ))
            }),
            Request::AddEntrance(id, e) => {
                edit::add_entrance(&mut d, id, &e).map(|_| Some("Way in added".into()))
            }
            Request::RemoveEntrance(id, e) => edit::remove_entrance(&mut d, id, e).map(|_| None),
            Request::Remove(id) => edit::remove(&mut d, id).map(|_| {
                Some("Dungeon removed (its map is kept aside in the world's folder)".into())
            }),
            Request::Def(id, label, f) => edit::update_def(&mut d, id, &label, f).map(|_| None),
            Request::Entrance(id, e, label, f) => {
                edit::update_entrance(&mut d, id, e, &label, f).map(|_| None)
            }
            Request::SetFrame(id, e, asset) => edit::set_frame(&mut d, id, e, &asset).map(|_| None),
            Request::Open(id) => match session::enter(&mut d, &mut session, id) {
                Ok(()) => {
                    // walking through a way in: arrive where players do
                    if walk.active
                        && let Some((p, yaw)) = session::dungeon_arrival(&d)
                    {
                        walk.autopilot = None;
                        walk.teleport(p, Some(yaw));
                    }
                    view.level = 0;
                    tool.selected_room = None;
                    tool.selected_light = None;
                    tool.draw = Draw::Select;
                    d.ed.tab = Tab::Dungeons;
                    let name = d
                        .dungeon_map
                        .0
                        .as_ref()
                        .map(|m| m.name.clone())
                        .unwrap_or_default();
                    Ok(Some(format!("Building {name}: Back to world when done")))
                }
                Err(e) if e == session::NEEDS_SAVE => {
                    session.open_after_save = Some(id);
                    actions.write(crate::ui::UiAction::SaveAs);
                    Ok(Some(format!(
                        "{e} — pick where to save it; the builder opens right after"
                    )))
                }
                Err(e) => Err(e),
            },
            Request::Back => {
                let id = d.dungeon_map.0.as_ref().map(|m| m.id);
                session::back(&mut d, &mut session).map(|_| {
                    // walking out: come out of the way in, in the world
                    if walk.active
                        && let Some(id) = id
                        && let Some((p, yaw)) = session::world_arrival(
                            &d,
                            id,
                            session.via.filter(|v| v.0 == id).map(|v| v.1),
                        )
                    {
                        walk.autopilot = None;
                        walk.teleport(p, Some(yaw));
                    }
                    d.ed.tab = Tab::Dungeons;
                    Some(format!("Back in {}", d.meta.name))
                })
            }
            Request::WalkIn => {
                let start = d
                    .gameplay
                    .player_start
                    .map(|p| (Vec3::from_array(p.pos), p.yaw));
                match start {
                    Some((pos, facing)) => {
                        walk.request = Some(crate::walk::WalkStart {
                            pos: Some(pos + Vec3::Y * 0.1),
                            yaw: Some(facing - std::f32::consts::FRAC_PI_2),
                            mode: None,
                            playtest: false,
                        });
                        Ok(None)
                    }
                    None => Err("Set a player start first (Gameplay)".into()),
                }
            }
            Request::WayOut(label, f) => edit::way_out(&mut d, &label, f).map(|_| None),
            Request::Map(label, f) => edit::map(&mut d, &label, |m| {
                f(m);
                Ok(())
            })
            .map(|_| None),
            Request::Layout {
                layout,
                size,
                room_size,
                irregular,
                seed,
            } => edit::relayout(&mut d, &layout, size, room_size, irregular, seed).map(|_| {
                if let Some(m) = d.dungeon_map.0.as_ref() {
                    let name = m.name.clone();
                    let plan = m.plan.clone();
                    let ent = plan
                        .rooms
                        .iter()
                        .find(|r| r.role == super::plan::Role::Entrance)
                        .or(plan.rooms.first());
                    if let Some(r) = ent {
                        let c = r.shape.centre();
                        d.gameplay.player_start = Some(crate::gameplay::PlayerStart {
                            pos: [c.x, plan.level_y(r.level) + r.floor + 0.05, c.y],
                            yaw: std::f32::consts::FRAC_PI_2,
                        });
                        d.gameplay.dirty = true;
                    }
                    view.level = view.level.clamp(
                        plan.levels.iter().map(|l| l.id).min().unwrap_or(0),
                        plan.levels.iter().map(|l| l.id).max().unwrap_or(0),
                    );
                    Some(format!("{name} laid out: {} rooms", plan.rooms.len()))
                } else {
                    None
                }
            }),
            Request::AutoLight { density } => auto_light(&mut d, &dworld, density, None),
            Request::AutoLightRoom(id) => auto_light(&mut d, &dworld, 1.0, Some(id)),
            Request::DressRoom(id) => super::dress::dress_rooms(&mut d, &dworld, Some(id)),
            Request::WallFixture { kind, at, normal } => {
                let inward = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
                // (as Auto-light: the fixture's back on the wall, the flame a little out from it)
                fixture_light(
                    &mut d,
                    kind,
                    at + inward * 0.25 + Vec3::Y * 0.5,
                    at + inward * lights::WALL_MOUNT - Vec3::Y * 0.5,
                    inward.x.atan2(inward.z),
                )
            }
            Request::FloorFixture { kind, at } => {
                let lift = d
                    .dungeon_map
                    .0
                    .as_ref()
                    .and_then(|m| kind.fixture(super::styles::get(&m.style).pack))
                    .map_or(1.0, |f| f.1);
                fixture_light(&mut d, kind, at + Vec3::Y * lift, at, 0.0)
            }
            Request::ClearAutoLights => clear_auto_lights(&mut d).map(|_| None),
            Request::Dress => super::dress::dress(&mut d, &dworld),
            Request::ClearDressing => {
                let ids = d
                    .dungeon_map
                    .0
                    .as_ref()
                    .map(|m| m.props.clone())
                    .unwrap_or_default();
                edit::remove_props(&mut d, &ids);
                let _ = edit::map(&mut d, "Clear dressing", |m| {
                    m.props.clear();
                    Ok(())
                });
                Ok(None)
            }
            Request::GoTo(p, _) => {
                if let Some(m) = d.dungeon_map.0.as_ref() {
                    let lv = m
                        .plan
                        .levels
                        .iter()
                        .filter(|l| l.y <= p.y + 1.0)
                        .max_by(|a, b| a.y.total_cmp(&b.y))
                        .map(|l| l.id);
                    if let Some(l) = lv {
                        view.level = l;
                    }
                }
                let yaw = d.orbit.yaw;
                d.orbit.set_view(p, yaw, 0.85, 28.0);
                Ok(None)
            }
            Request::TopView => {
                let (f, yaw, dist) = (d.orbit.focus, d.orbit.yaw, d.orbit.distance.max(70.0));
                d.orbit.set_view(f, yaw, 1.5, dist);
                Ok(None)
            }
            Request::Toast(t) => Ok(Some(t)),
            Request::AgeWalls { density } => age_walls(&mut d, &dworld, density),
            Request::Survey => survey(&d, &dworld),
            Request::ClearAged => edit::map(&mut d, "Clear aging", |m| {
                let n = m.decals.iter().filter(|x| x.auto).count();
                m.decals.retain(|x| !x.auto);
                Ok(Some(format!(
                    "{n} decals cleared (the ones you placed stay)"
                )))
            }),
        };
        match res {
            Ok(Some(msg)) => d.ed.toast(msg),
            Ok(None) => {}
            Err(e) => d.ed.toast(e),
        }
    }
    tool.requests.extend(later);
}

/// "Age the walls": replaces the aged decals (yours stay), one undo step.
/// Minimaps saved with the dungeon, and the walk check (from where players arrive, doors open).
fn survey(d: &WorldData, dw: &DungeonWorld) -> Result<Option<String>, String> {
    let Some(c) = dw.compiled.clone() else {
        return Err("Lay the dungeon out first".into());
    };
    let m = d.dungeon_map.0.as_ref().ok_or("No dungeon is open")?;
    let dir = d.meta.path.clone().ok_or("Save the world first")?;
    let exit = super::portal::exit_want(m, d.gameplay.player_start.as_ref(), |id| {
        d.store.items.get(&id).cloned()
    })
    .map(|w| w.tf.translation);
    let mut n = 0;
    for lv in super::survey::levels(m) {
        if let Some(mm) = super::survey::minimap(m, &c, lv, exit) {
            std::fs::write(
                dir.join(format!("minimap_L{lv}.png")),
                super::survey::png(&mm),
            )
            .map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    let from = d
        .gameplay
        .player_start
        .as_ref()
        .map(|p| Vec3::from_array(p.pos))
        .or_else(|| super::survey::entrance(m, &c))
        .ok_or("No rooms")?;
    let open: std::collections::HashMap<u32, bool> =
        m.pieces.iter().map(|p| (p.id, true)).collect();
    let w = super::survey::walk(m, &c, from, &super::pieces::blockers(&m.pieces, &open));
    let out: Vec<&str> = w
        .rooms
        .iter()
        .filter(|r| r.3 < 0.05)
        .map(|r| r.1.as_str())
        .collect();
    let walk = if out.is_empty() {
        "every room can be walked to".to_string()
    } else {
        format!("out of reach: {}", out.join(", "))
    };
    Ok(Some(format!(
        "{n} minimap{} saved with the dungeon · {walk}",
        if n == 1 { "" } else { "s" }
    )))
}

pub(super) fn age_walls(
    d: &mut WorldData,
    dw: &DungeonWorld,
    density: f32,
) -> Result<Option<String>, String> {
    let Some(c) = dw.compiled.clone() else {
        return Err("Lay the dungeon out first".into());
    };
    edit::map(d, "Age the walls", |m| {
        let mut made = super::decals::age(m, &c, density);
        m.decals.retain(|x| !x.auto);
        for x in &mut made {
            x.id = m.alloc_decal();
        }
        let n = made.len();
        m.decals.extend(made);
        Ok(Some(format!(
            "{n} cracks, stains, drips, soot and runes (each can be moved or removed)"
        )))
    })
}

/// A door asked for on a plain wall (Layout › On the walls, MCP `dungeons_wall`) goes into its
/// new doorway once the dungeon has been rebuilt with the opening.
pub(super) fn pending_door(mut tool: ResMut<DungeonTool>, world: Res<DungeonWorld>) {
    if tool.pending_doors.is_empty() || world.busy() {
        return;
    }
    let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut tool.pending_doors)
        .into_iter()
        .partition(|(_, _, want)| world.want != *want);
    tool.pending_doors = waiting;
    for (kind, at, _) in ready {
        match place_piece(kind, at, None, at.y, &world.doorways) {
            Ok(pc) => tool.requests.push(Request::Map(
                format!("Add {}", kind.name().to_lowercase()),
                Box::new(move |m| {
                    let mut pc = pc;
                    pc.id = m.alloc_piece();
                    m.pieces.push(pc);
                }),
            )),
            Err(why) => tool.requests.push(Request::Toast(why.into())),
        }
    }
}

/// While the walking build bar is open: what the walker looks at (the dungeon's surface along the
/// middle of the view, within reach) and the room the walker stands in.
#[allow(clippy::too_many_arguments)]
pub(super) fn walk_look(
    mut tool: ResMut<DungeonTool>,
    mut walk: ResMut<crate::walk::WalkState>,
    mut cursor: Query<&mut bevy::window::CursorOptions, With<bevy::window::PrimaryWindow>>,
    map: Res<DungeonMap>,
    caves: Res<crate::caves::render::CaveWorld>,
    terrain: Res<crate::terrain::Terrain>,
    holes: Option<Res<crate::terrain::holes::TerrainHoles>>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
) {
    // the walker stands still and the mouse is free while the bar is open
    let open = walk.active && tool.build_bar;
    if walk.input_paused != open {
        walk.input_paused = open;
        if let Ok(mut cur) = cursor.single_mut()
            && walk.active
            && std::env::var("WB_SHOT").is_err()
        {
            cur.grab_mode = if open {
                bevy::window::CursorGrabMode::None
            } else {
                bevy::window::CursorGrabMode::Locked
            };
            cur.visible = open;
        }
    }
    if !open {
        if tool.look.is_some() {
            tool.look = None;
        }
        return;
    }
    let (Some(m), Ok(gt)) = (map.0.as_ref(), cam.single()) else {
        return;
    };
    let level_at = |y: f32| {
        m.plan
            .levels
            .iter()
            .filter(|l| l.y <= y + 0.6)
            .max_by(|a, b| a.y.total_cmp(&b.y))
            .map_or(0, |l| l.id)
    };
    let feet = walk.position;
    let stand = m.plan.room_at(level_at(feet.y), Vec2::new(feet.x, feet.z));
    let ray = Ray3d::new(gt.translation(), gt.forward());
    tool.look = crate::camera::pick(&terrain, Some(&caves), holes.as_deref(), ray)
        .filter(|(p, _)| p.distance(ray.origin) < 25.0)
        .map(|(p, n)| (p, n.unwrap_or(Vec3::Y), stand));
}

/// Words typed in the walking build bar go to the area agent, on the room being walked (its
/// outline is the selection) with where the walker stands and looks; its progress shows over
/// the walk view.
#[allow(clippy::too_many_arguments)]
pub(super) fn walk_prompt(
    mut tool: ResMut<DungeonTool>,
    map: Res<DungeonMap>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<crate::terrain::Terrain>,
    mut sel: ResMut<crate::prompt::Selection>,
    mut session: ResMut<crate::prompt::PromptSession>,
    time: Res<Time<Real>>,
    mut ours: Local<(bool, f32)>,
) {
    let now = time.elapsed_secs();
    if ours.0 {
        if session.running() {
            tool.walk_status = format!("Building… {} steps", session.calls);
        } else {
            ours.0 = false;
            ours.1 = now;
            tool.walk_status = if session.summary.is_empty() {
                "Done.".into()
            } else {
                session.summary.clone()
            };
        }
    } else if !tool.walk_status.is_empty() && now - ours.1 > 9.0 {
        tool.walk_status.clear();
    }
    let Some(text) = tool.walk_prompt.take() else {
        return;
    };
    let Some(m) = map.0.as_ref().filter(|_| walk.active && map.interior()) else {
        return;
    };
    if session.running() {
        tool.walk_status = "Another edit is still running.".into();
        ours.1 = now;
        return;
    }
    let feet = walk.position;
    let level = m
        .plan
        .levels
        .iter()
        .filter(|l| l.y <= feet.y + 0.6)
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .map_or(0, |l| l.id);
    // the room walked in (else the ground round the walker)
    let poly = match m
        .plan
        .room_at(level, Vec2::new(feet.x, feet.z))
        .and_then(|id| m.plan.room(id))
    {
        Some(r) => r.shape.outline(),
        None => (0..24)
            .map(|k| {
                Vec2::new(feet.x, feet.z)
                    + Vec2::from_angle(k as f32 / 24.0 * std::f32::consts::TAU) * 10.0
            })
            .collect(),
    };
    if let Err(e) = sel.set(&terrain, &poly) {
        tool.walk_status = format!("Can't build here: {e}");
        ours.1 = now;
        return;
    }
    let looking = match tool.look {
        Some((p, n, _)) => format!(
            " and looking at [{:.1}, {:.1}, {:.1}] on the {}",
            p.x,
            p.y,
            p.z,
            if n.y.abs() < 0.5 {
                "wall"
            } else if n.y > 0.0 {
                "floor"
            } else {
                "ceiling"
            }
        ),
        None => String::new(),
    };
    let prompt = format!(
        "{text}\n\n(The user is walking through the dungeon, standing at [{:.1}, {:.1}, {:.1}]{looking}; the selection is the room they're in. \"Here\" means where they look.)",
        feet.x, feet.y, feet.z
    );
    session.requests.push(crate::prompt::Request::Send {
        prompt,
        refine: false,
    });
    ours.0 = true;
    tool.walk_status = "Building…".into();
}

/// A light and the object that holds it (a torch's sconce, a crystal) move together: drag the
/// light's marker (Lights tab) and the object follows; move the object (object tools, MCP) and
/// the light follows; undo either and the other comes back with it. (Neither move is recorded
/// twice: the follower's move isn't an undo step of its own.)
pub(super) fn fixtures_follow(
    mut map: ResMut<DungeonMap>,
    mut store: ResMut<crate::objects::ObjectStore>,
    mut last: Local<std::collections::HashMap<u32, (Vec3, Vec3)>>,
) {
    let Some(m) = map.0.as_ref() else {
        last.clear();
        return;
    };
    let mut light_moves: Vec<(u32, Vec3)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for l in &m.lights {
        let Some(o) = l.fixture.and_then(|f| store.items.get(&f)) else {
            continue;
        };
        seen.insert(l.id);
        let (lp, op) = (l.position(), Vec3::from_array(o.pos));
        if let Some(&(pl, po)) = last.get(&l.id) {
            let (dl, dobj) = (lp - pl, op - po);
            if dl.length_squared() > 1e-8 && dobj.length_squared() < 1e-8 {
                // the light moved: its object follows
                let mut p = o.clone();
                p.pos = (op + dl).to_array();
                store.move_to(p);
                last.insert(l.id, (lp, op + dl));
                continue;
            }
            if dobj.length_squared() > 1e-8 && dl.length_squared() < 1e-8 {
                // the object moved: the light follows
                light_moves.push((l.id, dobj));
                last.insert(l.id, (lp + dobj, op));
                continue;
            }
        }
        last.insert(l.id, (lp, op));
    }
    last.retain(|id, _| seen.contains(id));
    if !light_moves.is_empty()
        && let Some(mm) = map.0.as_mut()
    {
        for (id, d) in light_moves {
            if let Some(l) = mm.lights.iter_mut().find(|l| l.id == id) {
                l.pos = (l.position() + d).to_array();
            }
        }
    }
}

/// A light placed by hand with the object that holds it (a torch's sconce on a wall, a brazier on
/// the floor): `at` the light, `fixture` the object's origin, turned `yaw`. One undo step.
fn fixture_light(
    d: &mut WorldData,
    kind: LightKind,
    at: Vec3,
    fixture: Vec3,
    yaw: f32,
) -> Result<Option<String>, String> {
    let Some(m) = d.dungeon_map.0.clone() else {
        return Err("No dungeon's map is open".into());
    };
    let before = Arc::new(m.clone());
    let mut out = m.clone();
    let mut l = LightDef::new(out.alloc_light(), kind, at);
    l.shadows = kind != LightKind::Candles;
    d.history.begin_group(&d.store);
    if let Some((asset, _)) = kind.fixture(super::styles::get(&m.style).pack)
        && let Some(e) = d.lib.as_deref().and_then(|lib| lib.get(asset))
    {
        let oid = d.store.new_id();
        d.history.touch_object(&d.store, oid);
        let mut o = crate::objects::Placed {
            id: oid,
            asset: e.id.clone(),
            variant: 0,
            pos: fixture.to_array(),
            rot: Quat::from_rotation_y(yaw).to_array(),
            scale: 1.0,
            stretch: None,
            group: 0,
            locked: false,
        };
        o.set_scale3(Vec3::splat(e.default_scale));
        d.store.insert(o);
        l.fixture = Some(oid);
    }
    out.lights.push(l);
    let label = format!("Add {}", kind.name().to_lowercase());
    d.dungeon_map.0 = Some(out.clone());
    d.history
        .push_ext(&label, edit::UNDO_MAP, before, Arc::new(out));
    d.history.end_group(&d.store, &label);
    d.meta.dirty = true;
    Ok(None)
}

/// Auto-light: replaces the auto lights (and their fixtures) with new ones (all of them, or
/// room `only`'s), one undo step.
fn auto_light(
    d: &mut WorldData,
    dw: &DungeonWorld,
    density: f32,
    only: Option<u32>,
) -> Result<Option<String>, String> {
    let Some(m) = d.dungeon_map.0.clone() else {
        return Err("No dungeon's map is open".into());
    };
    let Some(c) = dw.compiled.clone() else {
        return Err("Lay the dungeon out first".into());
    };
    let idx = super::render::style_index(&m.plan, &m.style);
    let mut planned = lights::auto_light(
        &m.plan,
        &c,
        &|r| &super::styles::all()[idx.get(&r.id).copied().unwrap_or(0)],
        density,
    );
    let mine = |room: Option<u32>| only.is_none() || room == only;
    planned.retain(|p| mine(p.light.room));
    let before = Arc::new(m.clone());
    d.history.begin_group(&d.store);
    // the old auto lights' fixtures go
    for l in m.lights.iter().filter(|l| l.auto && mine(l.room)) {
        if let Some(o) = l.fixture
            && d.store.items.contains_key(&o)
        {
            d.history.touch_object(&d.store, o);
            d.store.remove(o);
        }
    }
    let mut out = m.clone();
    out.lights.retain(|l| !(l.auto && mine(l.room)));
    let auto_fixtures: std::collections::HashSet<u64> = m
        .lights
        .iter()
        .filter(|l| l.auto && mine(l.room))
        .filter_map(|l| l.fixture)
        .collect();
    out.props.retain(|p| !auto_fixtures.contains(p));
    let mut n = 0;
    for p in planned {
        let mut l = p.light;
        l.id = out.alloc_light();
        if let Some((asset, at, yaw)) = &p.fixture
            && let Some(e) = d.lib.as_deref().and_then(|lib| lib.get(asset))
        {
            let oid = d.store.new_id();
            d.history.touch_object(&d.store, oid);
            let mut o = crate::objects::Placed {
                id: oid,
                asset: e.id.clone(),
                variant: 0,
                pos: at.to_array(),
                rot: Quat::from_rotation_y(*yaw).to_array(),
                scale: 1.0,
                stretch: None,
                group: 0,
                locked: false,
            };
            o.set_scale3(Vec3::splat(e.default_scale));
            d.store.insert(o);
            l.fixture = Some(oid);
            out.props.push(oid);
        }
        out.lights.push(l);
        n += 1;
    }
    d.dungeon_map.0 = Some(out.clone());
    d.history
        .push_ext("Auto-light", edit::UNDO_MAP, before, Arc::new(out));
    d.history.end_group(&d.store, "Auto-light");
    d.meta.dirty = true;
    Ok(Some(format!(
        "{n} lights hung (each can be changed: click its marker)"
    )))
}

fn clear_auto_lights(d: &mut WorldData) -> Result<(), String> {
    let Some(m) = d.dungeon_map.0.clone() else {
        return Err("No dungeon's map is open".into());
    };
    let before = Arc::new(m.clone());
    d.history.begin_group(&d.store);
    let mut out = m.clone();
    for l in m.lights.iter().filter(|l| l.auto) {
        if let Some(o) = l.fixture
            && d.store.items.contains_key(&o)
        {
            d.history.touch_object(&d.store, o);
            d.store.remove(o);
            out.props.retain(|p| *p != o);
        }
    }
    out.lights.retain(|l| !l.auto);
    d.dungeon_map.0 = Some(out.clone());
    d.history
        .push_ext("Remove auto lights", edit::UNDO_MAP, before, Arc::new(out));
    d.history.end_group(&d.store, "Remove auto lights");
    d.meta.dirty = true;
    Ok(())
}
