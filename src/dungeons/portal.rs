//! The liquid portals: a sheet of swirling light (`shaders/dungeon_portal.wesl`) standing in
//! every way into a dungeon that is a portal — in its stone frame wherever it was put in the world
//! (it follows the frame when you move, turn or resize it), filling the doorway at the end of a
//! cave, and in a dungeon's map the way back out. Each glows with a light of its colour.
//!
//! The way back out looks like the way in players came through (its frame, an object of the
//! map: `MapDoc::exit`, and its colour) unless it is given its own (`MapDoc::way_out`), and stands
//! behind where players arrive unless it is put somewhere (dragged in the builder, its frame moved
//! like any object, or MCP `dungeons_way_out`).

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use super::edit::{FRAMES, Sheet, frame};
use super::model::*;

pub type PortalMaterial = ExtendedMaterial<StandardMaterial, PortalExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default, PartialEq)]
pub struct PortalParams {
    /// core colour (linear), w: strength
    pub core: Vec4,
    /// rim colour (linear), w: opacity
    pub rim: Vec4,
    /// x: swirl speed, y: seed, z: aspect (height / width), w: time offset
    pub look: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct PortalExt {
    #[uniform(100)]
    pub params: PortalParams,
}

impl MaterialExtension for PortalExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/dungeon_portal.wesl".into()
    }
}

/// Which way in a sheet belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Site {
    /// (dungeon, entrance)
    Entrance(u32, u32),
    /// a cave's end (its dungeon not made yet, or a cave way in)
    Cave(u32),
    /// the way back out of a dungeon's map
    Exit,
}

#[derive(Component)]
pub struct PortalSheet {
    pub site: Site,
}

/// A sheet wanted this frame: where, its size and shape, colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub site: Site,
    pub tf: Transform,
    pub w: f32,
    pub h: f32,
    pub shape: Sheet,
    pub color: PortalColor,
}

#[derive(Resource, Default)]
pub struct Portals {
    sheets: HashMap<Site, (Entity, Entity, (u32, u32, Sheet, PortalColor))>,
    meshes: HashMap<(u32, u32, Sheet), Handle<Mesh>>,
    mats: HashMap<(PortalColor, u32), Handle<PortalMaterial>>,
    /// the sheets as last placed (walk mode's portal crossing reads them)
    pub placed: Vec<Want>,
}

/// A sheet `w` wide and `h` tall standing on its bottom edge, facing +z (round-topped, a circle,
/// or flat-topped); its edge fades out (vertex alpha).
pub fn sheet_mesh(w: f32, h: f32, shape: Sheet) -> Mesh {
    let r = w * 0.5;
    // the outline, counter-clockwise seen from the front
    let mut outline: Vec<Vec2> = Vec::new();
    match shape {
        Sheet::Round => {
            for k in 0..48 {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * k as f32 / 48.0;
                outline.push(Vec2::new(r * a.cos(), r + r * a.sin()));
            }
        }
        Sheet::Arch | Sheet::Square => {
            let straight = if shape == Sheet::Arch {
                (h - r).max(0.0)
            } else {
                h
            };
            let side = 6;
            for k in 0..=side {
                outline.push(Vec2::new(r, straight * k as f32 / side as f32));
            }
            if shape == Sheet::Arch {
                let arc = 24;
                for k in 1..arc {
                    let a = std::f32::consts::PI * k as f32 / arc as f32;
                    outline.push(Vec2::new(r * a.cos(), straight + r * a.sin()));
                }
            } else {
                for k in 1..8 {
                    outline.push(Vec2::new(r - w * k as f32 / 8.0, h));
                }
            }
            for k in 0..=side {
                outline.push(Vec2::new(-r, straight * (side - k) as f32 / side as f32));
            }
            for k in 1..8 {
                outline.push(Vec2::new(-r + w * k as f32 / 8.0, 0.0));
            }
        }
    }
    let centre = Vec2::new(0.0, if shape == Sheet::Round { r } else { h * 0.45 });
    let rings = 7;
    let n = outline.len();
    let mut pos = vec![];
    let mut uv = vec![];
    let mut col = vec![];
    pos.push([centre.x, centre.y, 0.0]);
    uv.push([0.5, 1.0 - centre.y / h]);
    col.push([1.0, 1.0, 1.0, 1.0f32]);
    for ring in 1..=rings {
        let t = ring as f32 / rings as f32;
        for p in &outline {
            let q = centre.lerp(*p, t);
            pos.push([q.x, q.y, 0.0]);
            uv.push([q.x / w + 0.5, 1.0 - q.y / h]);
            // (the last ring is the edge: see-through)
            let a = (1.0 - t.powf(6.0)).clamp(0.0, 1.0);
            col.push([1.0, 1.0, 1.0, a]);
        }
    }
    let mut idx: Vec<u32> = vec![];
    for k in 0..n {
        idx.extend_from_slice(&[0, 1 + k as u32, 1 + ((k + 1) % n) as u32]);
    }
    for ring in 1..rings {
        let a0 = 1 + (ring - 1) * n;
        let b0 = 1 + ring * n;
        for k in 0..n {
            let k1 = (k + 1) % n;
            let (a, b, c, d) = (
                (a0 + k) as u32,
                (a0 + k1) as u32,
                (b0 + k1) as u32,
                (b0 + k) as u32,
            );
            idx.extend_from_slice(&[a, d, c, a, c, b]);
        }
    }
    let nrm = vec![[0.0, 0.0, 1.0f32]; pos.len()];
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

fn lin(c: [f32; 3]) -> Vec3 {
    let l = Color::srgb(c[0], c[1], c[2]).to_linear();
    Vec3::new(l.red, l.green, l.blue)
}

/// Every sheet that should stand this frame.
pub fn wanted(
    list: &Dungeons,
    map: &DungeonMap,
    caves: &crate::caves::Caves,
    store: &crate::objects::ObjectStore,
    gameplay: &crate::gameplay::Gameplay,
) -> Vec<Want> {
    let mut out = Vec::new();
    let mut cave_done = std::collections::HashSet::new();
    for d in &list.list {
        for e in &d.entrances {
            match e.kind {
                EntranceKind::Portal => {
                    let (tf, (w, h, bottom, shape)) =
                        match e.object.and_then(|o| store.items.get(&o)) {
                            Some(p) => {
                                let s = frame(&p.asset)
                                    .map(|f| (f.w, f.h, f.bottom, f.sheet))
                                    .unwrap_or((3.2, 4.4, 0.3, Sheet::Arch));
                                (
                                    Transform {
                                        translation: Vec3::from_array(p.pos),
                                        rotation: Quat::from_array(p.rot),
                                        scale: p.scale3(),
                                    },
                                    s,
                                )
                            }
                            None => (
                                Transform::from_translation(Vec3::from_array(e.pos))
                                    .with_rotation(Quat::from_rotation_y(e.yaw)),
                                (3.2, 4.4, 0.05, Sheet::Arch),
                            ),
                        };
                    let tf = tf * Transform::from_xyz(0.0, bottom, 0.0);
                    out.push(Want {
                        site: Site::Entrance(d.id, e.id),
                        tf,
                        w,
                        h,
                        shape,
                        color: e.color,
                    });
                }
                EntranceKind::Cave => {
                    if let Some(c) = e.cave {
                        cave_done.insert(c);
                        if let Some(dg) = caves.get(c).and_then(|c| c.dungeon.as_ref()) {
                            out.push(cave_sheet(Site::Entrance(d.id, e.id), dg, e.color));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // caves with a dungeon that has no entry here yet still show their portal
    for c in &caves.list {
        if let Some(dg) = &c.dungeon
            && !cave_done.contains(&c.id)
        {
            out.push(cave_sheet(Site::Cave(c.id), dg, PortalColor::Arcane));
        }
    }
    // the way back out of a dungeon's map: in its frame, else where it should stand
    if let Some(m) = map.0.as_ref()
        && let Some(w) = exit_want(m, gameplay.player_start.as_ref(), |id| {
            store.items.get(&id).cloned()
        })
    {
        out.push(w);
    }
    out
}

// ------------------------------------------------------------------ the way back out

/// The frame the way back out stands in (asset id; "" the liquid alone): its own, else the way
/// in's, else the first style.
pub fn exit_frame(m: &MapDoc) -> String {
    m.way_out
        .frame
        .clone()
        .or_else(|| m.way_out.way_in.as_ref().map(|w| w.0.clone()))
        .unwrap_or_else(|| FRAMES[0].asset.to_string())
}

/// The way back out's colour: its own, else the way in's.
pub fn exit_color(m: &MapDoc) -> PortalColor {
    m.way_out
        .color
        .or_else(|| m.way_out.way_in.as_ref().map(|w| w.1))
        .unwrap_or_default()
}

/// A placed object's facing (radians about the upright, 0 = +z).
pub fn yaw_of(rot: [f32; 4]) -> f32 {
    Quat::from_array(rot).to_euler(EulerRot::YXZ).0
}

/// Where the way back out's foot stands and which way it faces: where it was put, else behind
/// where players arrive (underground: against the wall, its frame clear of it).
pub fn exit_spot(m: &MapDoc, ps: Option<&crate::gameplay::PlayerStart>) -> Option<(Vec3, f32)> {
    if let Some(at) = m.way_out.at {
        return Some((Vec3::from_array(at), m.way_out.yaw));
    }
    let ps = ps?;
    Some(match m.kind {
        MapKind::Realm => realm_exit(ps),
        MapKind::Interior => {
            let (at, yaw) = interior_exit(m, ps);
            // (a deep frame stands out from the wall, not into it)
            let clear = frame(&exit_frame(m)).map_or(0.0, |f| (f.depth * 0.5 - 0.6).max(0.0));
            (at + Vec3::new(yaw.sin(), 0.0, yaw.cos()) * clear, yaw)
        }
    })
}

/// Where the way back out stands now (its foot) and which way it faces: its frame's, else its spot.
pub fn exit_at(
    m: &MapDoc,
    store: &crate::objects::ObjectStore,
    ps: Option<&crate::gameplay::PlayerStart>,
) -> Option<(Vec3, f32)> {
    match m.exit.and_then(|o| store.items.get(&o)) {
        Some(p) => Some((Vec3::from_array(p.pos), yaw_of(p.rot))),
        None => exit_spot(m, ps),
    }
}

/// Whether `ray` goes through the way back out's sheet `w` before it meets what it hits (`hit`).
pub fn ray_through(w: &Want, ray: Ray3d, hit: Option<Vec3>) -> bool {
    let n = w.tf.rotation * Vec3::Z;
    let d = ray.direction.dot(n);
    if d.abs() < 1e-4 {
        return false;
    }
    let t = (w.tf.translation - ray.origin).dot(n) / d;
    if t <= 0.0 || hit.is_some_and(|h| t > h.distance(ray.origin) + 0.3) {
        return false;
    }
    let local =
        w.tf.compute_affine()
            .inverse()
            .transform_point3(ray.origin + *ray.direction * t);
    local.x.abs() < w.w * 0.5 && local.y > 0.0 && local.y < w.h
}

/// The way back out's sheet: in its frame (`MapDoc::exit`, looked up with `object`) when it has
/// one, else at its spot.
pub fn exit_want(
    m: &MapDoc,
    ps: Option<&crate::gameplay::PlayerStart>,
    object: impl Fn(u64) -> Option<crate::objects::Placed>,
) -> Option<Want> {
    let color = exit_color(m);
    if let Some(p) = m.exit.and_then(object)
        && let Some(f) = frame(&p.asset)
    {
        let tf = Transform {
            translation: Vec3::from_array(p.pos),
            rotation: Quat::from_array(p.rot),
            scale: p.scale3(),
        } * Transform::from_xyz(0.0, f.bottom, 0.0);
        return Some(Want {
            site: Site::Exit,
            tf,
            w: f.w,
            h: f.h,
            shape: f.sheet,
            color,
        });
    }
    let (at, yaw) = exit_spot(m, ps)?;
    let (w, h) = if m.kind == MapKind::Realm {
        (3.2, 4.4)
    } else {
        (3.0, 4.0)
    };
    Some(Want {
        site: Site::Exit,
        tf: Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
        w,
        h,
        shape: Sheet::Arch,
        color,
    })
}

/// The look of the way into `def` players came through (`via`) when it is a portal, else of its
/// first portal: (frame asset, "" for the liquid alone; colour). Without a portal way in: the
/// first frame style in the colour of the way in.
pub fn way_in_look(
    def: &DungeonDef,
    via: Option<u32>,
    store: &crate::objects::ObjectStore,
) -> (String, PortalColor) {
    let came = via.and_then(|v| def.entrances.iter().find(|e| e.id == v));
    let portal = came.filter(|e| e.kind == EntranceKind::Portal).or_else(|| {
        def.entrances
            .iter()
            .find(|e| e.kind == EntranceKind::Portal)
    });
    match portal {
        Some(e) => (
            e.object
                .and_then(|o| store.items.get(&o))
                .map(|p| p.asset.clone())
                .filter(|a| frame(a).is_some())
                .unwrap_or_default(),
            e.color,
        ),
        None => (
            FRAMES[0].asset.to_string(),
            came.or(def.entrances.first())
                .map(|e| e.color)
                .unwrap_or_default(),
        ),
    }
}

/// Puts the way back out's frame in step with `MapDoc::way_out`: the frame it should have stands
/// at its spot (put in, swapped for another, moved or taken out). `record`: inside the open undo
/// step; else quietly (opening the map). Whether anything changed.
pub fn place_exit_frame(d: &mut crate::io::WorldData, record: bool) -> bool {
    let Some(m) = d.dungeon_map.0.as_ref() else {
        return false;
    };
    let want = exit_frame(m);
    let entry = d
        .lib
        .as_deref()
        .and_then(|l| l.get(&want))
        .filter(|_| frame(&want).is_some())
        .map(|e| (e.id.clone(), e.default_scale));
    let cur = m.exit.and_then(|o| d.store.items.get(&o)).cloned();
    // where it stands: where it was put, else where its frame stands now, else behind the arrival
    let spot = match (m.way_out.at, &cur) {
        (Some(at), _) => Some((Vec3::from_array(at), m.way_out.yaw)),
        (None, Some(p)) if p.asset == want => Some((Vec3::from_array(p.pos), yaw_of(p.rot))),
        _ => exit_spot(m, d.gameplay.player_start.as_ref()),
    };
    let Some((at, yaw)) = spot else { return false };
    let rot = Quat::from_rotation_y(yaw).to_array();
    let had = m.exit.is_some();
    match (cur, entry) {
        (Some(p), Some((asset, _))) if p.asset == asset => {
            if Vec3::from_array(p.pos).distance(at) < 1e-3
                && Quat::from_array(p.rot).angle_between(Quat::from_array(rot)) < 1e-3
            {
                return false;
            }
            if record {
                d.history.touch_object(&d.store, p.id);
            }
            d.store.move_to(crate::objects::Placed {
                pos: at.to_array(),
                rot,
                ..p
            });
            true
        }
        (cur, entry) => {
            if cur.is_none() && entry.is_none() && !had {
                return false;
            }
            if let Some(p) = cur {
                if record {
                    d.history.touch_object(&d.store, p.id);
                }
                d.store.remove(p.id);
            }
            let mut exit = None;
            if let Some((asset, scale)) = entry {
                // (underground, a frame taller than the room is made to fit under its ceiling)
                let scale = scale
                    * d.dungeon_map
                        .0
                        .as_ref()
                        .map_or(1.0, |m| fit_under(m, at, &asset));
                let oid = d.store.new_id();
                if record {
                    d.history.touch_object(&d.store, oid);
                }
                let mut p = crate::objects::Placed {
                    id: oid,
                    asset,
                    variant: 0,
                    pos: at.to_array(),
                    rot,
                    scale: 1.0,
                    stretch: None,
                    group: 0,
                    locked: false,
                    glow: None,
                };
                p.set_scale3(Vec3::splat(scale));
                d.store.insert(p);
                exit = Some(oid);
            }
            if let Some(m) = d.dungeon_map.0.as_mut() {
                m.exit = exit;
            }
            true
        }
    }
}

/// How much a frame (`asset`) standing at `at` must shrink to fit under the ceiling of the room
/// it stands in (1: it fits; a realm has no ceiling).
fn fit_under(m: &MapDoc, at: Vec3, asset: &str) -> f32 {
    let Some(f) = frame(asset) else { return 1.0 };
    if m.kind != MapKind::Interior {
        return 1.0;
    }
    let p = Vec2::new(at.x, at.z);
    let plan = &m.plan;
    let room = plan
        .rooms
        .iter()
        .filter(|r| {
            r.kind.is_space()
                && r.shape.sdf(p) > -0.5
                && (plan.level_y(r.level) + r.floor - at.y).abs() < 1.0
        })
        .min_by(|a, b| a.shape.inradius().total_cmp(&b.shape.inradius()));
    room.map_or(1.0, |r| ((r.height - 0.3) / f.tall).clamp(0.4, 1.0))
}

/// Opening a dungeon's map from its world: the way back out takes the look of the way in players
/// came through (`look`: `way_in_look`) and its frame stands. Whether the map changed.
pub fn open_way_out(d: &mut crate::io::WorldData, look: (String, PortalColor)) -> bool {
    let Some(m) = d.dungeon_map.0.as_mut() else {
        return false;
    };
    let changed = m.way_out.way_in.as_ref() != Some(&look);
    m.way_out.way_in = Some(look);
    place_exit_frame(d, false) || changed
}

/// The way back out and its frame move together: drag it in the builder (or set it by MCP) and
/// the frame follows; move the frame like any object and the way out stays where it was put. (As
/// `viewport::fixtures_follow`: the follower's move isn't an undo step of its own.)
pub(super) fn way_out_follows(
    mut map: ResMut<DungeonMap>,
    mut store: ResMut<crate::objects::ObjectStore>,
    gameplay: Res<crate::gameplay::Gameplay>,
    mut last: Local<Option<((Vec3, f32), (Vec3, f32))>>,
) {
    let Some(m) = map.0.as_ref() else {
        *last = None;
        return;
    };
    let Some(o) = m.exit.and_then(|o| store.items.get(&o)) else {
        *last = None;
        return;
    };
    let Some(spot) = exit_spot(m, gameplay.player_start.as_ref()) else {
        return;
    };
    let obj = (Vec3::from_array(o.pos), yaw_of(o.rot));
    let same = |a: (Vec3, f32), b: (Vec3, f32)| {
        a.0.distance_squared(b.0) < 1e-6
            && Quat::from_rotation_y(a.1).angle_between(Quat::from_rotation_y(b.1)) < 1e-3
    };
    let prev = *last;
    match prev {
        // the spot moved (dragged, set, the players' arrival moved): the frame follows
        Some((ls, lo)) if !same(spot, ls) && same(obj, lo) => {
            let p = crate::objects::Placed {
                pos: spot.0.to_array(),
                rot: Quat::from_rotation_y(spot.1).to_array(),
                ..o.clone()
            };
            store.move_to(p);
            *last = Some((spot, spot));
        }
        // the frame was moved (object tools): the way out stays where it was put
        Some((ls, lo)) if same(spot, ls) && !same(obj, lo) => {
            if let Some(mm) = map.0.as_mut() {
                mm.way_out.at = Some(obj.0.to_array());
                mm.way_out.yaw = obj.1;
            }
            *last = Some((obj, obj));
        }
        _ => *last = Some((spot, obj)),
    }
}

/// Where a realm's way back out stands: 4 m behind where players arrive, facing them.
/// (position of its foot, facing yaw)
pub fn realm_exit(ps: &crate::gameplay::PlayerStart) -> (Vec3, f32) {
    let fwd = Vec3::new(ps.yaw.cos(), 0.0, -ps.yaw.sin());
    (Vec3::from_array(ps.pos) - fwd * 4.0, fwd.x.atan2(fwd.z))
}

/// Where an underground map's way back out stands: against the wall behind where players
/// arrive, facing them; when that wall is the way on (a doorway), against a side wall, else the
/// one ahead. (position of its foot, facing yaw)
pub fn interior_exit(m: &MapDoc, ps: &crate::gameplay::PlayerStart) -> (Vec3, f32) {
    let fwd = Vec3::new(ps.yaw.cos(), 0.0, -ps.yaw.sin());
    let start = Vec3::from_array(ps.pos);
    let s2 = Vec2::new(start.x, start.z);
    let back = Vec2::new(-fwd.x, -fwd.z);
    let room = m
        .plan
        .rooms
        .iter()
        .filter(|r| r.kind.is_space() && r.shape.sdf(s2) > 0.0)
        .min_by(|a, b| a.shape.inradius().total_cmp(&b.shape.inradius()));
    let Some(r) = room else {
        return (start - fwd * 2.5, fwd.x.atan2(fwd.z));
    };
    // how far the wall that way is (the foot stands just off it), and whether another room opens
    // there (a doorway: the portal would stand in the way on)
    let wall = |dir: Vec2| -> (f32, bool) {
        let mut d = 2.5;
        while d < 60.0 && r.shape.sdf(s2 + dir * (d + 0.5)) > 0.4 {
            d += 0.25;
        }
        let beyond = s2 + dir * (d + 1.6);
        let open = [-1.8f32, 0.0, 1.8].iter().any(|o| {
            m.plan.rooms.iter().any(|x| {
                x.id != r.id
                    && x.level == r.level
                    && x.kind.is_space()
                    && x.shape.sdf(beyond + dir.perp() * *o) > 0.0
            })
        });
        (d, open)
    };
    let dir = [back, back.perp(), -back.perp(), -back]
        .into_iter()
        .find(|d| !wall(*d).1)
        .unwrap_or(back);
    let d = wall(dir).0;
    (
        start + Vec3::new(dir.x, 0.0, dir.y) * d,
        (-dir.x).atan2(-dir.y),
    )
}

fn cave_sheet(site: Site, dg: &crate::caves::Dungeon, color: PortalColor) -> Want {
    // the doorway is 3.6 m wide and 4 m tall: the sheet fills it, a hand's breadth inside
    let tf = Transform::from_translation(Vec3::from_array(dg.door) + Vec3::Y * 0.02)
        .with_rotation(Quat::from_rotation_y(dg.door_yaw));
    Want {
        site,
        tf,
        w: 3.2,
        h: 3.7,
        shape: Sheet::Arch,
        color,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sync(
    mut commands: Commands,
    mut portals: ResMut<Portals>,
    list: Res<Dungeons>,
    map: Res<DungeonMap>,
    caves: Res<crate::caves::Caves>,
    store: Res<crate::objects::ObjectStore>,
    gameplay: Res<crate::gameplay::Gameplay>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<PortalMaterial>>,
    mut tfs: Query<&mut Transform, With<PortalSheet>>,
    mut lights: Query<(&mut Transform, &mut PointLight), Without<PortalSheet>>,
) {
    let want = wanted(&list, &map, &caves, &store, &gameplay);
    // gone
    let keep: std::collections::HashSet<Site> = want.iter().map(|w| w.site).collect();
    let stale: Vec<Site> = portals
        .sheets
        .keys()
        .filter(|s| !keep.contains(s))
        .copied()
        .collect();
    for s in stale {
        if let Some((a, b, _)) = portals.sheets.remove(&s) {
            commands.entity(a).despawn();
            commands.entity(b).despawn();
        }
    }
    for w in &want {
        let key = (
            (w.w * 10.0).round() as u32,
            (w.h * 10.0).round() as u32,
            w.shape,
        );
        let look = (key.0, key.1, w.shape, w.color);
        let (core, rim) = w.color.colors();
        let light_at = w.tf * Transform::from_xyz(0.0, w.h * 0.45, 0.9);
        match portals.sheets.get(&w.site) {
            Some((sheet, light, old)) if *old == look => {
                if let Ok(mut t) = tfs.get_mut(*sheet)
                    && *t != w.tf
                {
                    *t = w.tf;
                }
                if let Ok((mut t, _)) = lights.get_mut(*light) {
                    t.translation = light_at.translation;
                }
            }
            _ => {
                if let Some((a, b, _)) = portals.sheets.remove(&w.site) {
                    commands.entity(a).despawn();
                    commands.entity(b).despawn();
                }
                let mesh = portals
                    .meshes
                    .entry(key)
                    .or_insert_with(|| meshes.add(sheet_mesh(w.w, w.h, w.shape)))
                    .clone();
                let aspect = w.h / w.w.max(0.1);
                let mat = portals
                    .mats
                    .entry((w.color, (aspect * 20.0).round() as u32))
                    .or_insert_with(|| {
                        mats.add(ExtendedMaterial {
                            base: StandardMaterial {
                                alpha_mode: AlphaMode::Blend,
                                double_sided: true,
                                cull_mode: None,
                                perceptual_roughness: 0.3,
                                ..default()
                            },
                            extension: PortalExt {
                                params: PortalParams {
                                    core: lin(core).extend(1.5),
                                    rim: lin(rim).extend(0.96),
                                    look: Vec4::new(
                                        1.0,
                                        (w.color as u32 * 17 + 3) as f32,
                                        aspect,
                                        w.color as u32 as f32 * 1.7,
                                    ),
                                },
                            },
                        })
                    })
                    .clone();
                let sheet = commands
                    .spawn((
                        Name::new("Dungeon portal"),
                        PortalSheet { site: w.site },
                        Mesh3d(mesh),
                        MeshMaterial3d(mat),
                        w.tf,
                        NotShadowCaster,
                    ))
                    .id();
                let rl = lin(rim);
                let light = commands
                    .spawn((
                        Name::new("Dungeon portal light"),
                        PointLight {
                            color: Color::linear_rgb(rl.x, rl.y, rl.z),
                            intensity: 350_000.0,
                            range: 12.0,
                            radius: 0.6,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        light_at,
                    ))
                    .id();
                portals.sheets.insert(w.site, (sheet, light, look));
            }
        }
    }
    portals.placed = want;
}

/// Walking through a portal: into its dungeon (a way in, in the world), or back out (the way
/// out of a dungeon's map). The walker arrives on the other side and keeps walking.
#[allow(clippy::too_many_arguments)]
pub(super) fn crossing(
    walk: Res<crate::walk::WalkState>,
    portals: Res<Portals>,
    list: Res<Dungeons>,
    mut tool: ResMut<super::ui::DungeonTool>,
    mut session: ResMut<super::session::Session>,
    time: Res<Time<Real>>,
    mut last: Local<HashMap<Site, f32>>,
    mut cooldown: Local<f32>,
    mut ed: ResMut<crate::editor::Editor>,
    store: Res<crate::objects::ObjectStore>,
    lib: Option<Res<crate::objects::Library>>,
    bounds: Res<crate::placement::footprint::ModelBounds>,
    map: Res<DungeonMap>,
) {
    if !walk.active {
        last.clear();
        return;
    }
    let now = time.elapsed_secs();
    let feet = walk.position + Vec3::Y * 0.9;
    let mut went = None;
    // an object or a spot that is a way in: walking onto it (a cellar door, the foot of a stairway)
    if map.0.is_none() && now > *cooldown {
        let p = walk.position;
        'find: for d in &list.list {
            for e in &d.entrances {
                let q = Vec2::new(p.x, p.z);
                let touching = match e.kind {
                    // within an arm's length of the object's edge (a cellar door, a well: players
                    // walk up to it, they can't always step onto it), level with it
                    EntranceKind::Object => match e.object.and_then(|o| store.items.get(&o)) {
                        Some(o) => {
                            let (lo, hi) = lib
                                .as_deref()
                                .and_then(|l| l.get(&o.asset))
                                .map(|x| bounds.of(x))
                                .unwrap_or((Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)));
                            let fp = crate::placement::footprint::Footprint::of(o, (lo, hi));
                            let dy = p.y - o.pos[1];
                            fp.sdist(q) < 0.9 && dy > -3.0 && dy < hi.y * o.scale3().y + 1.0
                        }
                        None => continue,
                    },
                    EntranceKind::Point => {
                        q.distance(Vec2::new(e.pos[0], e.pos[2])) < 1.0
                            && (p.y - e.pos[1]).abs() < 3.0
                    }
                    _ => continue,
                };
                if touching {
                    went = Some(Site::Entrance(d.id, e.id));
                    break 'find;
                }
            }
        }
    }
    for w in &portals.placed {
        let local = w.tf.compute_affine().inverse().transform_point3(feet);
        let inside = local.x.abs() < w.w * 0.5 && local.y > -0.5 && local.y < w.h;
        let prev = last.insert(w.site, local.z);
        // (from its front, through it)
        if inside && now > *cooldown && prev.is_some_and(|p| p > 0.0) && local.z <= 0.0 {
            went = Some(w.site);
        }
    }
    let Some(site) = went else { return };
    *cooldown = now + 3.0;
    last.clear();
    match site {
        Site::Entrance(d, e) => {
            session.via = Some((d, e));
            let name = list.get(d).map(|x| x.name.clone()).unwrap_or_default();
            ed.toast(format!("Entering {name}…"));
            tool.requests.push(super::ui::Request::Open(d));
        }
        Site::Exit => {
            ed.toast("Back to the world…");
            tool.requests.push(super::ui::Request::Back);
        }
        Site::Cave(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sheet_is_an_arch_with_a_soft_edge() {
        let m = sheet_mesh(3.0, 4.0, Sheet::Arch);
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(p)) =
            m.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!()
        };
        let top = p.iter().map(|v| v[1]).fold(f32::MIN, f32::max);
        let side = p.iter().map(|v| v[0]).fold(f32::MIN, f32::max);
        assert!(
            (top - 4.0).abs() < 0.05 && (side - 1.5).abs() < 0.01,
            "{top} {side}"
        );
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(c)) =
            m.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!()
        };
        assert!(c.iter().any(|c| c[3] < 0.05) && c[0][3] > 0.99);
    }

    #[test]
    fn the_way_out_looks_like_the_way_in_unless_given_its_own() {
        let def: DungeonDef = serde_json::from_value(
            serde_json::json!({ "id": 1, "name": "Crypt", "folder": "001_crypt", "entrances": [
            { "id": 1, "kind": "cave", "pos": [0, 0, 0], "color": "fel" },
            { "id": 2, "kind": "portal", "pos": [9, 0, 0], "object": 7, "color": "void" },
        ] }),
        )
        .unwrap();
        let mut store = crate::objects::ObjectStore::default();
        store.insert(serde_json::from_value(serde_json::json!({ "id": 7, "asset": "hp:portal_gate", "variant": 0, "pos": [9, 0, 0], "rot": [0, 0, 0, 1], "scale": 1.0 })).unwrap());
        // came through the cave: the dungeon's portal is the look; through the portal: its own
        assert_eq!(
            way_in_look(&def, Some(1), &store),
            ("hp:portal_gate".to_string(), PortalColor::Void)
        );
        assert_eq!(
            way_in_look(&def, None, &store),
            ("hp:portal_gate".to_string(), PortalColor::Void)
        );
        // no portal at all: the first frame in the colour of the way in
        let caves = DungeonDef {
            entrances: def.entrances[..1].to_vec(),
            ..def.clone()
        };
        assert_eq!(
            way_in_look(&caves, Some(1), &store),
            (FRAMES[0].asset.to_string(), PortalColor::Fel)
        );
        let mut m = MapDoc::new(1, "Crypt", MapKind::Interior);
        m.way_out.way_in = Some(("hp:portal_gate".into(), PortalColor::Void));
        assert_eq!(
            (exit_frame(&m), exit_color(&m)),
            ("hp:portal_gate".to_string(), PortalColor::Void)
        );
        m.way_out.frame = Some(String::new());
        m.way_out.color = Some(PortalColor::Frost);
        assert_eq!(
            (exit_frame(&m), exit_color(&m)),
            (String::new(), PortalColor::Frost)
        );
        // put somewhere: it stands there, facing the way it was turned
        m.way_out.at = Some([3.0, 0.0, 4.0]);
        m.way_out.yaw = 1.0;
        assert_eq!(exit_spot(&m, None), Some((Vec3::new(3.0, 0.0, 4.0), 1.0)));
        let w = exit_want(&m, None, |_| None).unwrap();
        assert_eq!((w.site, w.color), (Site::Exit, PortalColor::Frost));
    }
}
