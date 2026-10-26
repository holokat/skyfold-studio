//! The Roads tab tools:
//!
//! - Draw Road: click control points (a live surface preview follows the ground and the cursor);
//!   double-click or Enter finishes, Backspace removes the last point, Esc cancels.
//! - Auto Route: click A then B; the A* router finds a natural road in the background.
//! - Edit: click a road to select it, drag its control points, click on it again to insert one,
//!   Alt-click a point (or select it and press Delete) to delete it.
//! - Remove: click a road.
//!
//! Points snap to the ends and middles of existing roads so networks join cleanly. The overlay
//! (control points, snapping, hints) is drawn with egui over the viewport.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bevy_egui::EguiContexts;
use bevy_egui::egui;

use super::build::{Edit, RoadWorld};
use super::plan::RoadPlan;
use super::render::{RoadMaterials, RoadVisuals};
use super::{Road, RoadKind, RoadOptions, Roads};
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, Tab};
use crate::terrain::Terrain;
use crate::tools::Stroke;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RoadToolMode {
    #[default]
    Draw,
    Route,
    Edit,
    Remove,
}

impl RoadToolMode {
    pub fn hint(self) -> &'static str {
        match self {
            RoadToolMode::Draw => {
                "Click to place points along the way. Double-click or Enter to finish, Backspace removes the last point, Esc cancels. Points snap to other roads."
            }
            RoadToolMode::Route => {
                "Click a start and an end: the road finds its own way, easing up slopes with switchbacks and crossing rivers where they are narrow."
            }
            RoadToolMode::Edit => {
                "Click a road to select it, then drag its points to reshape it. Click on the selected road to add a point; Alt-click a point (or select it and press Delete) to remove it."
            }
            RoadToolMode::Remove => {
                "Click a road to remove it. The ground, plants and trees under it come back."
            }
        }
    }
}

/// A requested change from the inspector, applied by the tool system.
pub enum Request {
    Update(Road),
    Rebuild(u32),
    Remove(u32),
}

/// Inspector numbers for one road.
#[derive(Clone, Debug, Default)]
pub struct RoadStats {
    pub length: f32,
    pub max_grade: f32,
    pub avg_grade: f32,
    pub climb: f32,
    pub descent: f32,
    pub bridges: usize,
    pub bridge_len: f32,
    /// (arc length, road height, ground height), ~160 points
    pub profile: Vec<[f32; 3]>,
}

/// The cursor point, snapped onto a road end or centreline when close.
#[derive(Clone, Copy, Debug)]
pub struct Snap {
    pub p: Vec3,
    pub road: Option<u32>,
    pub end: bool,
}

struct Drag {
    road: u32,
    index: usize,
    pos: Vec3,
    inserted: bool,
}

#[derive(Resource)]
pub struct RoadTool {
    pub mode: RoadToolMode,
    pub kind: RoadKind,
    pub width: f32,
    pub options: RoadOptions,
    /// control points of the road being drawn
    pub draft: Vec<Vec3>,
    pub cursor: Option<Snap>,
    /// auto route start and the search running in the background
    pub route_from: Option<Vec3>,
    route_task: Option<(Task<Option<Vec<Vec2>>>, Road)>,
    drag: Option<Drag>,
    /// hovered road and control point; the selected control point (Edit)
    pub hover: Option<(u32, Option<usize>)>,
    pub sel_point: Option<(u32, usize)>,
    /// inspector width slider being dragged (road, width): applied when let go
    pub width_edit: Option<(u32, f32)>,
    pub requests: Vec<Request>,
    pub stats: HashMap<u32, RoadStats>,
    /// 128 px surface previews (RGBA) per kind and their egui textures
    pub swatches: Vec<Option<(u32, u32, Vec<u8>)>>,
    pub swatch_tex: Vec<egui::TextureHandle>,
    last_click: Option<(f32, Vec2)>,
    preview_key: u64,
}

impl Default for RoadTool {
    fn default() -> Self {
        let swatches = RoadKind::ALL
            .iter()
            .map(|k| {
                let p = crate::terrain::textures::library_file(k.style().texture, "swatch.png");
                image::open(p).ok().map(|i| {
                    let i = i.to_rgba8();
                    (i.width(), i.height(), i.into_raw())
                })
            })
            .collect();
        Self {
            mode: RoadToolMode::Draw,
            kind: RoadKind::DirtRoad,
            width: RoadKind::DirtRoad.default_width(),
            options: RoadOptions::default(),
            draft: vec![],
            cursor: None,
            route_from: None,
            route_task: None,
            drag: None,
            hover: None,
            sel_point: None,
            width_edit: None,
            requests: vec![],
            stats: HashMap::new(),
            swatches,
            swatch_tex: vec![],
            last_click: None,
            preview_key: 0,
        }
    }
}

impl RoadTool {
    pub fn routing(&self) -> bool {
        self.route_task.is_some()
    }
    pub fn update_stats(&mut self, plans: &[Arc<RoadPlan>]) {
        self.stats.clear();
        for p in plans {
            let (max_grade, avg_grade) = p.grades();
            let (climb, descent) = p.climb();
            let n = p.samples.len();
            let step = (n / 160).max(1);
            let profile = (0..n)
                .step_by(step)
                .chain(std::iter::once(n.saturating_sub(1)))
                .map(|i| [p.samples[i].s, p.h[i], p.ground[i]])
                .collect();
            let bridge_len = p
                .spans
                .iter()
                .map(|sp| p.samples[sp.i1].s - p.samples[sp.i0].s)
                .sum();
            self.stats.insert(
                p.id,
                RoadStats {
                    length: p.length(),
                    max_grade,
                    avg_grade,
                    climb,
                    descent,
                    bridges: p.spans.len(),
                    bridge_len,
                    profile,
                },
            );
        }
    }
}

#[derive(Component)]
struct RoadPreview;

pub fn build(app: &mut App) {
    app.init_resource::<RoadTool>()
        .add_systems(
            Update,
            (tool_input, finish_route, super::build::regrade)
                .chain()
                .in_set(crate::tools::ToolSystems::Apply),
        )
        .add_systems(Update, preview.after(crate::tools::ToolSystems::Commit))
        .add_systems(bevy_egui::EguiPrimaryContextPass, overlay);
}

/// Snaps a ground point to the end of a road (within `radius`) or onto its centreline.
fn snap(
    roads: &Roads,
    plans: &[Arc<RoadPlan>],
    t: &Terrain,
    p: Vec3,
    radius: f32,
    exclude: Option<u32>,
) -> Snap {
    let mut best: Option<(f32, Vec3, u32)> = None;
    for r in roads.roads.iter().filter(|r| Some(r.id) != exclude) {
        for e in [r.points.first(), r.points.last()].into_iter().flatten() {
            let d = Vec2::new(e[0], e[2]).distance(p.xz());
            if d < radius && best.is_none_or(|b| d < b.0) {
                let y = if r.authored.is_some() {
                    e[1]
                } else {
                    t.height_at(e[0], e[2])
                };
                best = Some((d, Vec3::new(e[0], y, e[2]), r.id));
            }
        }
    }
    if let Some((_, q, id)) = best {
        return Snap {
            p: q,
            road: Some(id),
            end: true,
        };
    }
    for pl in plans.iter().filter(|pl| Some(pl.id) != exclude) {
        if let Some((_, s)) = pl.nearest(p.xz(), pl.hw + radius * 0.6) {
            let q = pl.point_at(s);
            return Snap {
                p: Vec3::new(
                    q.x,
                    if pl.authored {
                        pl.height_at(s)
                    } else {
                        t.height_at(q.x, q.y)
                    },
                    q.y,
                ),
                road: Some(pl.id),
                end: false,
            };
        }
    }
    Snap {
        p,
        road: None,
        end: false,
    }
}

/// The road (and control point) under the cursor.
fn pick(
    roads: &Roads,
    plans: &[Arc<RoadPlan>],
    cam: (&Camera, &GlobalTransform),
    screen: Vec2,
    hit: Option<Vec3>,
    prefer: Option<u32>,
) -> Option<(u32, Option<usize>)> {
    let mut best: Option<(f32, u32, usize)> = None;
    for r in &roads.roads {
        for (i, q) in r.points.iter().enumerate() {
            let Ok(v) = cam.0.world_to_viewport(cam.1, Vec3::from_array(*q)) else {
                continue;
            };
            let d = v.distance(screen) - if Some(r.id) == prefer { 4.0 } else { 0.0 };
            if d < 14.0 && best.is_none_or(|b| d < b.0) {
                best = Some((d, r.id, i));
            }
        }
    }
    if let Some((_, id, i)) = best {
        return Some((id, Some(i)));
    }
    let hit = hit?;
    plans
        .iter()
        .filter_map(|pl| pl.nearest(hit.xz(), pl.hw + 1.5).map(|(d, _)| (d, pl.id)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| (id, None))
}

/// Where a new point on the road goes in its control-point list (the segment the arc length
/// `s` falls in).
fn insert_index(road: &Road, pl: &RoadPlan, s: f32) -> usize {
    let mut best = 1;
    for (i, q) in road.points.iter().enumerate() {
        if let Some((_, sq)) = pl.nearest(Vec2::new(q[0], q[2]), 2.0) {
            if sq < s {
                best = i + 1;
            }
        }
    }
    best.clamp(1, road.points.len())
}

#[allow(clippy::too_many_arguments)]
fn tool_input(
    mut tool: ResMut<RoadTool>,
    mut rw: RoadWorld,
    ed: Res<Editor>,
    stroke: Res<Stroke>,
    pointer: Res<Pointer>,
    keys: Res<ButtonInput<KeyCode>>,
    walk: Res<crate::walk::WalkState>,
    vis: Res<RoadVisuals>,
    time: Res<Time<Real>>,
    cam: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    // inspector requests first (they work from any tab)
    for req in std::mem::take(&mut tool.requests) {
        let r = match req {
            Request::Update(road) => rw.apply(Edit::Update(road), "Edit road"),
            Request::Rebuild(id) => rw.apply(Edit::Rebuild(id), "Rebuild road"),
            Request::Remove(id) => rw.apply(Edit::Remove(id), "Remove road"),
        };
        if let Err(e) = r {
            warn!("ROADS edit failed: {e}");
        }
    }
    if ed.tab != Tab::Roads || walk.active {
        tool.cursor = None;
        tool.hover = None;
        return;
    }
    let Ok((camera, cam_t)) = cam.single() else {
        return;
    };
    let plans = &vis.plans;
    let radius = pointer
        .hit
        .map(|h| (h.distance(cam_t.translation()) * 0.018).clamp(2.5, 40.0))
        .unwrap_or(4.0);
    let exclude = tool.drag.as_ref().map(|d| d.road);
    tool.cursor = pointer
        .hit
        .filter(|_| !pointer.over_ui)
        .map(|h| snap(&rw.roads, plans, &rw.terrain, h, radius, exclude));
    tool.hover = match (pointer.screen, pointer.over_ui) {
        (Some(sc), false) if tool.drag.is_none() => pick(
            &rw.roads,
            plans,
            (camera, cam_t),
            sc,
            pointer.hit,
            rw.roads.selected,
        ),
        _ => None,
    };
    let keys_ok = !pointer.keyboard_captured;
    let now = time.elapsed_secs();
    match tool.mode {
        RoadToolMode::Draw => {
            if keys_ok && keys.just_pressed(KeyCode::Escape) {
                tool.draft.clear();
            }
            if keys_ok && keys.just_pressed(KeyCode::Backspace) {
                tool.draft.pop();
            }
            let mut finish = keys_ok
                && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter));
            if stroke.just_started
                && let (Some(c), Some(sc)) = (tool.cursor, pointer.screen)
            {
                let double = tool
                    .last_click
                    .is_some_and(|(t0, p0)| now - t0 < 0.38 && p0.distance(sc) < 8.0);
                tool.last_click = Some((now, sc));
                if double && tool.draft.len() >= 2 {
                    finish = true;
                } else if tool
                    .draft
                    .last()
                    .is_none_or(|l| l.xz().distance(c.p.xz()) > 0.8)
                {
                    tool.draft.push(c.p);
                }
            }
            if finish && tool.draft.len() >= 2 {
                let pts: Vec<[f32; 3]> = tool.draft.iter().map(|p| p.to_array()).collect();
                let road = Road::new(0, tool.kind, pts, tool.width, tool.options);
                match rw.apply(Edit::Create(road), "Draw road") {
                    Ok(id) => info!("ROADS drew road {id}"),
                    Err(e) => warn!("ROADS draw failed: {e}"),
                }
                tool.draft.clear();
                tool.last_click = None;
            }
        }
        RoadToolMode::Route => {
            if keys_ok && keys.just_pressed(KeyCode::Escape) {
                tool.route_from = None;
            }
            if stroke.just_started
                && !tool.routing()
                && let Some(c) = tool.cursor
            {
                match tool.route_from.take() {
                    None => tool.route_from = Some(c.p),
                    Some(a) => {
                        let road = Road::new(
                            0,
                            tool.kind,
                            vec![a.to_array(), c.p.to_array()],
                            tool.width,
                            tool.options,
                        );
                        start_route(&mut tool, &rw, road);
                    }
                }
            }
        }
        RoadToolMode::Edit => {
            if keys_ok && keys.just_pressed(KeyCode::Escape) {
                rw.roads.selected = None;
                tool.sel_point = None;
            }
            if stroke.just_started
                && let Some((id, pt)) = tool.hover
            {
                let was_selected = rw.roads.selected == Some(id);
                rw.roads.selected = Some(id);
                match pt {
                    Some(i) if stroke.inverse => {
                        delete_point(&mut rw, id, i);
                        tool.sel_point = None;
                    }
                    Some(i) => {
                        let pos = rw
                            .roads
                            .get(id)
                            .map(|r| Vec3::from_array(r.points[i]))
                            .unwrap_or_default();
                        tool.sel_point = Some((id, i));
                        tool.drag = Some(Drag {
                            road: id,
                            index: i,
                            pos,
                            inserted: false,
                        });
                    }
                    // the first click selects a road; on a selected road a click adds a point
                    None if !was_selected => tool.sel_point = None,
                    None => {
                        // a new point there, dragged right away
                        if let (Some(hit), Some(pl), Some(road)) = (
                            pointer.hit,
                            plans.iter().find(|p| p.id == id),
                            rw.roads.get(id),
                        ) && let Some((_, s)) = pl.nearest(hit.xz(), pl.hw + 2.0)
                        {
                            let i = insert_index(road, pl, s);
                            let q = pl.point_at(s);
                            let pos = Vec3::new(
                                q.x,
                                if road.authored.is_some() {
                                    pl.height_at(s)
                                } else {
                                    rw.terrain.height_at(q.x, q.y)
                                },
                                q.y,
                            );
                            tool.sel_point = Some((id, i));
                            tool.drag = Some(Drag {
                                road: id,
                                index: i,
                                pos,
                                inserted: true,
                            });
                        }
                    }
                }
            } else if stroke.just_started {
                rw.roads.selected = None;
                tool.sel_point = None;
            }
            if let Some(c) = tool.cursor
                && stroke.active
                && let Some(d) = tool.drag.as_mut()
                && pointer.hit.is_some()
            {
                d.pos = c.p;
                if !d.inserted
                    && rw
                        .roads
                        .get(d.road)
                        .is_some_and(|road| road.authored.is_some())
                {
                    // Dragging an authored bridge edits its horizontal alignment. Its explicit
                    // profile remains the imported deck unless a 3-D API update supplies a y.
                    d.pos.y = rw
                        .roads
                        .get(d.road)
                        .and_then(|road| road.points.get(d.index))
                        .map(|point| point[1])
                        .unwrap_or(d.pos.y);
                }
            }
            if !stroke.active
                && let Some(d) = tool.drag.take()
                && let Some(mut road) = rw.roads.get(d.road).cloned()
            {
                let moved = d.inserted
                    || road
                        .points
                        .get(d.index)
                        .is_some_and(|p| Vec3::from_array(*p).distance(d.pos) > 0.05);
                if moved {
                    if d.inserted {
                        road.points.insert(d.index, d.pos.to_array());
                    } else {
                        road.points[d.index] = d.pos.to_array();
                    }
                    let label = if d.inserted {
                        "Add road point"
                    } else {
                        "Move road point"
                    };
                    if let Err(e) = rw.apply(Edit::Update(road), label) {
                        warn!("ROADS edit failed: {e}");
                    }
                }
            }
            if keys_ok
                && (keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace))
                && let Some((id, i)) = tool.sel_point.take()
            {
                delete_point(&mut rw, id, i);
            }
        }
        RoadToolMode::Remove => {
            if stroke.just_started
                && let Some((id, _)) = tool.hover
            {
                if let Err(e) = rw.apply(Edit::Remove(id), "Remove road") {
                    warn!("ROADS remove failed: {e}");
                }
                tool.hover = None;
            }
        }
    }
}

fn delete_point(rw: &mut RoadWorld, id: u32, i: usize) {
    let Some(mut road) = rw.roads.get(id).cloned() else {
        return;
    };
    let r = if road.points.len() <= 2 {
        rw.apply(Edit::Remove(id), "Remove road")
    } else {
        road.points.remove(i.min(road.points.len() - 1));
        rw.apply(Edit::Update(road), "Delete road point")
    };
    if let Err(e) = r {
        warn!("ROADS edit failed: {e}");
    }
}

fn start_route(tool: &mut RoadTool, rw: &RoadWorld, road: Road) {
    let (a, b) = (
        Vec3::from_array(road.points[0]).xz(),
        Vec3::from_array(road.points[1]).xz(),
    );
    let base = super::ground::Base {
        t: &rw.terrain,
        g: &rw.roads.ground,
    };
    let input = super::route::prepare(&base, &rw.water, a, b, road.kind, road.width);
    let task = AsyncComputeTaskPool::get().spawn(async move { super::route::solve(&input) });
    tool.route_task = Some((task, road));
}

/// Creates the routed road once the background search is done.
fn finish_route(mut tool: ResMut<RoadTool>, mut rw: RoadWorld, mut ed: ResMut<Editor>) {
    let Some((task, _)) = tool.route_task.as_mut() else {
        return;
    };
    let Some(result) = block_on(future::poll_once(task)) else {
        return;
    };
    let (_, mut road) = tool.route_task.take().unwrap();
    match result {
        Some(pts) => {
            road.points = pts
                .iter()
                .map(|p| [p.x, rw.terrain.height_at(p.x, p.y), p.y])
                .collect();
            match rw.apply(Edit::Create(road), "Auto route") {
                Ok(id) => info!("ROADS routed road {id}"),
                Err(e) => warn!("ROADS route failed: {e}"),
            }
        }
        None => ed.toast("No route found between those points"),
    }
}

/// Live surface preview of the road being drawn (or the point being dragged).
#[allow(clippy::too_many_arguments)]
fn preview(
    mut commands: Commands,
    mut tool: ResMut<RoadTool>,
    roads: Res<Roads>,
    terrain: Res<Terrain>,
    water: Res<crate::water::WaterBodies>,
    ed: Res<Editor>,
    mut mats: ResMut<RoadMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    old: Query<Entity, With<RoadPreview>>,
) {
    let mut pts: Vec<Vec3> = vec![];
    let (mut kind, mut width, mut options) = (tool.kind, tool.width, tool.options);
    if ed.tab == Tab::Roads {
        match tool.mode {
            RoadToolMode::Draw if !tool.draft.is_empty() => {
                pts = tool.draft.clone();
                if let Some(c) = tool.cursor {
                    pts.push(c.p);
                }
            }
            RoadToolMode::Edit => {
                if let Some(d) = &tool.drag
                    && let Some(r) = roads.get(d.road)
                {
                    pts = r.points.iter().map(|p| Vec3::from_array(*p)).collect();
                    if d.inserted {
                        pts.insert(d.index.min(pts.len()), d.pos);
                    } else if d.index < pts.len() {
                        pts[d.index] = d.pos;
                    }
                    (kind, width, options) = (r.kind, r.width, r.options);
                }
            }
            _ => {}
        }
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for p in &pts {
            ((p.x * 4.0) as i32, (p.z * 4.0) as i32).hash(&mut h);
        }
        (kind, (width * 10.0) as i32).hash(&mut h);
        h.finish()
    };
    if key == tool.preview_key {
        return;
    }
    tool.preview_key = key;
    for e in &old {
        commands.entity(e).despawn();
    }
    pts.dedup_by(|a, b| a.xz().distance(b.xz()) < 0.5);
    if pts.len() < 2 {
        return;
    }
    let road = Road::new(
        u32::MAX,
        kind,
        pts.iter().map(|p| p.to_array()).collect(),
        width,
        RoadOptions {
            bed: false,
            ..options
        },
    );
    let base = super::ground::Base {
        t: &terrain,
        g: &roads.ground,
    };
    let plan = super::plan::plan_road(&road, &base, &water, &[]);
    let material = mats.get(kind);
    for (mesh, aabb) in super::mesh::ribbon_chunks(&plan, &terrain, road.id) {
        commands.spawn((
            Name::new("Road preview"),
            RoadPreview,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material.clone()),
            Transform::IDENTITY,
            aabb,
            bevy::light::NotShadowCaster,
        ));
    }
}

/// Control points, snapping and hints over the viewport.
#[allow(clippy::too_many_arguments)]
fn overlay(
    mut contexts: EguiContexts,
    tool: Res<RoadTool>,
    roads: Res<Roads>,
    vis: Res<RoadVisuals>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    pointer: Res<Pointer>,
    terrain: Res<Terrain>,
    cam: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) -> Result {
    if ed.tab != Tab::Roads || walk.active {
        return Ok(());
    }
    let Ok((camera, cam_t)) = cam.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("road_overlay"),
    ));
    let proj = |p: Vec3| {
        camera
            .world_to_viewport(cam_t, p)
            .ok()
            .map(|v| egui::pos2(v.x, v.y))
    };
    let accent = egui::Color32::from_rgb(255, 196, 92);
    let white = egui::Color32::WHITE;
    let shadow = egui::Color32::from_black_alpha(150);
    let handle = |p: egui::Pos2, r: f32, fill: egui::Color32| {
        painter.circle_filled(p, r + 1.5, shadow);
        painter.circle_filled(p, r, fill);
        painter.circle_stroke(
            p,
            r,
            egui::Stroke::new(1.5, egui::Color32::from_black_alpha(200)),
        );
    };
    // a road's centreline draped on the ground
    let line = |road_pts: &[Vec3], col: egui::Color32, w: f32| {
        if road_pts.len() < 2 {
            return;
        }
        let tmp = Road::new(
            0,
            RoadKind::Trail,
            road_pts.iter().map(|p| p.to_array()).collect(),
            1.0,
            RoadOptions::default(),
        );
        let pts: Vec<egui::Pos2> = tmp
            .centreline(2.0)
            .iter()
            .filter_map(|p| proj(Vec3::new(p.x, terrain.height_at(p.x, p.z) + 0.3, p.z)))
            .collect();
        painter.add(egui::Shape::dashed_line(
            &pts,
            egui::Stroke::new(w, col),
            10.0,
            6.0,
        ));
    };

    // selected / hovered roads: their control points
    let sel = roads.selected;
    let hover_road = tool.hover.map(|h| h.0);
    for r in &roads.roads {
        let show = matches!(tool.mode, RoadToolMode::Edit)
            && (Some(r.id) == sel || Some(r.id) == hover_road);
        if show {
            let pts: Vec<Vec3> = r.points.iter().map(|p| Vec3::from_array(*p)).collect();
            line(&pts, egui::Color32::from_white_alpha(140), 1.5);
            for (i, p) in pts.iter().enumerate() {
                if let Some(q) = proj(*p) {
                    let hot =
                        tool.hover == Some((r.id, Some(i))) || tool.sel_point == Some((r.id, i));
                    handle(
                        q,
                        if hot { 7.0 } else { 5.5 },
                        if hot { accent } else { white },
                    );
                }
            }
        }
        if tool.mode == RoadToolMode::Remove
            && Some(r.id) == hover_road
            && let Some(pl) = vis.plans.iter().find(|p| p.id == r.id)
        {
            for side in [-1.0f32, 1.0] {
                let pts: Vec<egui::Pos2> = pl
                    .samples
                    .iter()
                    .step_by(2)
                    .filter_map(|s| {
                        let q = s.p + s.dir.perp() * side * pl.hw;
                        proj(Vec3::new(q.x, terrain.height_at(q.x, q.y) + 0.3, q.y))
                    })
                    .collect();
                painter.add(egui::Shape::line(
                    pts,
                    egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 90, 80)),
                ));
            }
        }
    }
    // the road being drawn
    if tool.mode == RoadToolMode::Draw && !tool.draft.is_empty() {
        let mut pts = tool.draft.clone();
        if let Some(c) = tool.cursor {
            pts.push(c.p);
        }
        line(&pts, egui::Color32::from_white_alpha(170), 1.5);
        for p in &tool.draft {
            if let Some(q) = proj(*p) {
                handle(q, 5.5, white);
            }
        }
    }
    if tool.mode == RoadToolMode::Route
        && let Some(a) = tool.route_from
        && let Some(q) = proj(a)
    {
        handle(q, 7.0, accent);
        painter.text(
            q + egui::vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            "A",
            egui::FontId::proportional(14.0),
            white,
        );
        if let Some(c) = tool.cursor
            && let Some(b) = proj(c.p)
            && !tool.routing()
        {
            painter.add(egui::Shape::dashed_line(
                &[q, b],
                egui::Stroke::new(1.5, egui::Color32::from_white_alpha(150)),
                8.0,
                6.0,
            ));
        }
    }
    // snapping indicator and a hint by the cursor
    if let Some(c) = tool.cursor
        && let Some(q) = proj(c.p)
        && matches!(tool.mode, RoadToolMode::Draw | RoadToolMode::Route)
    {
        if c.road.is_some() {
            // joins a road's end (filled) or its side (ring)
            painter.circle_stroke(q, 11.0, egui::Stroke::new(2.0, accent));
            painter.circle_filled(q, if c.end { 6.0 } else { 3.0 }, accent);
        } else {
            painter.circle_stroke(q, 6.0, egui::Stroke::new(1.5, white));
        }
        let hint = match tool.mode {
            RoadToolMode::Draw if tool.draft.is_empty() => {
                format!("{} · click to start", tool.kind.name())
            }
            RoadToolMode::Draw => format!(
                "{} point{} · double-click or Enter to finish",
                tool.draft.len(),
                if tool.draft.len() == 1 { "" } else { "s" }
            ),
            RoadToolMode::Route if tool.routing() => "Finding a route…".to_string(),
            RoadToolMode::Route if tool.route_from.is_some() => "Click the destination".to_string(),
            RoadToolMode::Route => "Click the start".to_string(),
            _ => String::new(),
        };
        let hint = if c.road.is_some() {
            format!("{hint} · joins road")
        } else {
            hint
        };
        if let Some(sc) = pointer.screen {
            let pos = egui::pos2(sc.x + 18.0, sc.y + 14.0);
            let galley = painter.layout_no_wrap(hint, egui::FontId::proportional(12.5), white);
            let rect = egui::Rect::from_min_size(pos, galley.size() + egui::vec2(14.0, 8.0));
            painter.rect_filled(
                rect,
                egui::CornerRadius::same(6),
                egui::Color32::from_rgba_unmultiplied(14, 16, 20, 210),
            );
            painter.galley(pos + egui::vec2(7.0, 4.0), galley, white);
        }
    }
    Ok(())
}
