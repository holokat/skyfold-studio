//! Editor markers for the gameplay layer: colour-coded map pins with icons and names (player
//! start, POIs, NPCs, resource nodes), drawn as a screen-space egui layer so they stay crisp and
//! legible from the overview down to a few metres, plus ground rings, links and the player start's
//! facing arrow. Spawn regions are painted on the ground with the `TerrainOverlay` (translucent
//! hatched fills and outlines) while the Gameplay tab is open, labelled with their level and
//! creatures. Ore deposit areas are painted the same way in their ore's colour; their deposits
//! are rocks in the world (`deposits`), so they get no pins of their own.
//!
//! Cost stays flat with hundreds of markers: far nodes and NPCs collapse to dots, labels are
//! decluttered by priority, and occlusion is a short terrain height march per marker.

use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Id, LayerId, Order, Pos2, Rect, Shape, Stroke, pos2, vec2,
};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};

use super::edit::{Ground, level_label};
use super::ores::Ore;
use super::{
    Aggression, Gameplay, GameplayEditor, GameplaySel, GameplayTool, MarkerView, NpcRole, OreMode,
    PoiKind, ResourceKind,
};
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, Tab};
use crate::terrain::Terrain;
use crate::terrain::overlay::{TerrainOverlay, new_overlay_image};
use crate::water::WaterBodies;
use crate::world::Zones;

pub(super) fn build(app: &mut App) {
    app.add_systems(EguiPrimaryContextPass, draw_markers)
        .add_systems(
            PostUpdate,
            spawn_overlay.before(crate::terrain::TerrainSystems::Gpu),
        );
}

// ------------------------------------------------------------------ look

pub const POI_COLOR: Color32 = Color32::from_rgb(240, 176, 46);
pub const NPC_COLOR: Color32 = Color32::from_rgb(74, 158, 255);
pub const START_COLOR: Color32 = Color32::from_rgb(52, 199, 112);

// Marker icons are Central icon names (`assets/icons/central`, drawn with `crate::ui::icons::paint`).

pub fn poi_icon(k: PoiKind) -> &'static str {
    match k {
        PoiKind::Town => "Castle",
        PoiKind::QuestHub => "ExclamationCircle",
        PoiKind::Dungeon => "Door",
        PoiKind::Landmark => "PinFlag",
        PoiKind::Camp => "Fire1",
        PoiKind::Vendor => "Store1",
        PoiKind::FlightPoint => "Feather",
        PoiKind::Graveyard => "Ghost",
        PoiKind::Portal => "BlackHole",
        PoiKind::BossArena => "Trophy",
        PoiKind::Shrine => "Sparkle",
        PoiKind::Vista => "Telescope",
        PoiKind::Treasure => "Treasure",
        PoiKind::Custom => "MapPin",
    }
}

pub fn npc_icon(r: NpcRole) -> &'static str {
    match r {
        NpcRole::QuestGiver => "ExclamationCircle",
        NpcRole::Vendor => "Coin1",
        NpcRole::Trainer => "GraduateCap",
        NpcRole::Guard => "Shield",
        NpcRole::Innkeeper => "Beer",
        NpcRole::FlightMaster => "Feather",
        NpcRole::Banker => "Bank",
        NpcRole::Ambient => "User",
        NpcRole::Custom => "User",
    }
}

pub fn node_icon(k: ResourceKind) -> &'static str {
    match k {
        ResourceKind::Ore => "Diamond",
        ResourceKind::Herb => "Grass",
        ResourceKind::Wood => "Tree",
        ResourceKind::Stone => "FormOctagon",
        ResourceKind::Crystal => "Diamond",
        ResourceKind::Custom => "Package",
    }
}

pub fn node_color(k: ResourceKind) -> Color32 {
    match k {
        ResourceKind::Ore => Color32::from_rgb(240, 112, 44),
        ResourceKind::Herb => Color32::from_rgb(92, 196, 82),
        ResourceKind::Wood => Color32::from_rgb(192, 150, 100),
        ResourceKind::Stone => Color32::from_rgb(150, 156, 168),
        ResourceKind::Crystal => Color32::from_rgb(178, 112, 255),
        ResourceKind::Custom => Color32::from_rgb(220, 220, 220),
    }
}

pub const START_ICON: &str = "Flag1";
pub const SPAWN_ICON: &str = "Goblin";
pub const ORE_ICON: &str = "Diamond";
pub const AREA_ICON: &str = "FormPyramide";

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// The colour of an ore's rarity (tier headers, deposit pins).
pub fn rarity_color(o: Ore) -> Color32 {
    c32(o.rarity().color())
}

/// An ore's colour in the editor (outlines, swatches).
pub fn ore_color(o: Ore) -> Color32 {
    c32(o.color())
}

/// A deposit area's colour: its main ore's.
pub fn area_color(a: &super::OreArea) -> Color32 {
    a.main_ore()
        .map(ore_color)
        .unwrap_or(Color32::from_rgb(170, 170, 170))
}

/// A spawn region's colour, by its most hostile creature (grey-violet while it has none).
pub fn spawn_color(a: Option<Aggression>) -> Color32 {
    match a {
        Some(Aggression::Hostile) => Color32::from_rgb(232, 72, 64),
        Some(Aggression::Neutral) => Color32::from_rgb(236, 160, 48),
        Some(Aggression::Passive) => Color32::from_rgb(110, 196, 96),
        None => Color32::from_rgb(150, 140, 205),
    }
}

// ------------------------------------------------------------------ layout & picking

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinKind {
    Start,
    Poi,
    Npc,
    Node,
    Spawn,
    /// an ore deposit area's label
    Area,
}

impl PinKind {
    /// A region's label at its centre rather than a pin.
    pub fn region(self) -> bool {
        matches!(self, PinKind::Spawn | PinKind::Area)
    }
}

/// A marker on screen.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub sel: GameplaySel,
    pub kind: PinKind,
    /// Screen point of the ground anchor (the pin's tip).
    pub tip: Pos2,
    /// Centre of the icon head.
    pub head: Pos2,
    /// Head radius (px); 0 = a far-away dot.
    pub r: f32,
    /// Camera distance (m).
    pub dist: f32,
    pub occluded: bool,
}

fn project(cam: &Camera, cam_t: &GlobalTransform, p: Vec3) -> Option<Pos2> {
    let v = cam.world_to_viewport(cam_t, p).ok()?;
    Some(pos2(v.x, v.y))
}

/// Hidden behind the terrain (a short height march from the eye).
fn occluded(t: &Terrain, eye: Vec3, p: Vec3) -> bool {
    let d = p - eye;
    let len = d.length();
    if len < 40.0 {
        return false;
    }
    let steps = 24;
    (1..steps).any(|k| {
        let q = eye + d * (k as f32 / steps as f32) * 0.97;
        t.height_at(q.x, q.z) > q.y + 1.0
    })
}

impl MarkerView {
    pub fn shows(&self, k: PinKind) -> bool {
        match k {
            PinKind::Start | PinKind::Poi => self.pois,
            PinKind::Npc => self.npcs,
            PinKind::Node | PinKind::Area => self.nodes,
            PinKind::Spawn => self.spawns,
        }
    }
}

/// Every visible marker on screen. `spawns`: include the spawn-region labels.
pub fn layout(
    gp: &Gameplay,
    view: &MarkerView,
    g: &Ground,
    cam: &Camera,
    cam_t: &GlobalTransform,
    spawns: bool,
) -> Vec<Pin> {
    let eye = cam_t.translation();
    let screen = cam
        .logical_viewport_rect()
        .map(|r| Rect::from_min_max(pos2(r.min.x, r.min.y), pos2(r.max.x, r.max.y)).expand(40.0));
    let mut out = Vec::with_capacity(gp.nodes.len() + gp.pois.len() + gp.npcs.len() + 8);
    let mut add = |sel: GameplaySel, kind: PinKind, xz: Vec2| {
        if !view.shows(kind) || (kind.region() && !spawns) {
            return;
        }
        let p = Vec3::new(xz.x, g.surface(xz), xz.y);
        let Some(tip) = project(cam, cam_t, p) else {
            return;
        };
        if screen.is_some_and(|r| !r.contains(tip)) {
            return;
        }
        let dist = eye.distance(p);
        let scale = (1.15 - dist / 9000.0).clamp(0.85, 1.12);
        let r = match kind {
            PinKind::Start => 14.5 * scale,
            PinKind::Poi => 13.0 * scale,
            PinKind::Npc if dist < 1500.0 => 10.5 * scale,
            PinKind::Node if dist < 800.0 => 8.5 * scale,
            PinKind::Spawn | PinKind::Area => 15.0,
            _ => 0.0,
        };
        let head = if kind.region() || r == 0.0 {
            tip
        } else {
            tip - vec2(0.0, r * 2.25)
        };
        out.push(Pin {
            sel,
            kind,
            tip,
            head,
            r,
            dist,
            occluded: occluded(g.terrain, eye, p + Vec3::Y * 1.5),
        });
    };
    if let Some(s) = gp.player_start {
        add(
            GameplaySel::PlayerStart,
            PinKind::Start,
            Vec2::new(s.pos[0], s.pos[2]),
        );
    }
    for q in &gp.pois {
        add(
            GameplaySel::Poi(q.id),
            PinKind::Poi,
            Vec2::new(q.pos[0], q.pos[2]),
        );
    }
    for q in &gp.npcs {
        add(
            GameplaySel::Npc(q.id),
            PinKind::Npc,
            Vec2::new(q.pos[0], q.pos[2]),
        );
    }
    // (an area's deposits are picked through their area)
    for q in gp.nodes.iter().filter(|q| q.area.is_none()) {
        add(
            GameplaySel::Node(q.id),
            PinKind::Node,
            Vec2::new(q.pos[0], q.pos[2]),
        );
    }
    for s in &gp.spawns {
        if let Some(c) = s.centroid() {
            add(GameplaySel::Spawn(s.id), PinKind::Spawn, c);
        }
    }
    for a in &gp.ore_areas {
        if let Some(c) = a.centroid() {
            add(GameplaySel::OreArea(a.id), PinKind::Area, c);
        }
    }
    out
}

/// The marker under a screen point: the closest head (visible ones first).
pub fn pick_pin(pins: &[Pin], at: Pos2) -> Option<GameplaySel> {
    pins.iter()
        .filter_map(|p| {
            let reach = if p.r > 0.0 { p.r + 4.0 } else { 7.0 };
            let d = p.head.distance(at).min(if p.r > 0.0 {
                p.tip.distance(at) + 2.0
            } else {
                f32::MAX
            });
            (d <= reach).then_some((p, d))
        })
        .min_by(|a, b| {
            (a.0.occluded, a.0.kind.region(), a.1)
                .partial_cmp(&(b.0.occluded, b.0.kind.region(), b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(p, _)| p.sel)
}

/// The outline of a selectable region (spawn region or ore deposit area).
pub fn region_outline(gp: &Gameplay, sel: GameplaySel) -> Option<&[[f32; 2]]> {
    match sel {
        GameplaySel::Spawn(id) => gp
            .spawns
            .iter()
            .find(|s| s.id == id)
            .map(|r| r.polygon.as_slice()),
        GameplaySel::OreArea(id) => gp
            .ore_areas
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.polygon.as_slice()),
        _ => None,
    }
}

/// A corner handle of the selected spawn region or deposit area under a screen point.
pub fn pick_vertex(
    gp: &Gameplay,
    g: &Ground,
    cam: &Camera,
    cam_t: &GlobalTransform,
    at: Pos2,
) -> Option<(GameplaySel, usize)> {
    let sel = gp.selected?;
    region_outline(gp, sel)?
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let p = Vec2::new(v[0], v[1]);
            let s = project(cam, cam_t, Vec3::new(p.x, g.surface(p) + 0.3, p.y))?;
            let d = s.distance(at);
            (d <= 10.0).then_some((i, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| (sel, i))
}

// ------------------------------------------------------------------ drawing

/// A ground path through world xz points, draped on the surface and projected (split where it
/// leaves the view).
fn draped(
    cam: &Camera,
    cam_t: &GlobalTransform,
    g: &Ground,
    pts: &[Vec2],
    closed: bool,
    step: f32,
) -> Vec<Vec<Pos2>> {
    let mut lines = vec![];
    let mut cur: Vec<Pos2> = vec![];
    let n = pts.len();
    let segs = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segs {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let k = ((a.distance(b) / step).ceil() as usize).clamp(1, 64);
        for s in 0..=k {
            if s == k && i + 1 < segs {
                continue;
            }
            let p = a.lerp(b, s as f32 / k as f32);
            match project(cam, cam_t, Vec3::new(p.x, g.surface(p) + 0.4, p.y)) {
                Some(q) => cur.push(q),
                None => {
                    if cur.len() > 1 {
                        lines.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                }
            }
        }
    }
    if cur.len() > 1 {
        lines.push(cur);
    }
    lines
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        c.r(),
        c.g(),
        c.b(),
        (c.a() as f32 * a.clamp(0.0, 1.0)) as u8,
    )
}

/// A colour lifted toward white by `k` (icons on the dark glass heads).
fn lift(c: Color32, k: f32) -> Color32 {
    let f = |v: u8| (v as f32 + (255.0 - v as f32) * k) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// The dark glass of beacon heads and labels: rgba(12, 15, 21, 0.80).
const GLASS_DARK: Color32 = Color32::from_rgba_unmultiplied_const(12, 15, 21, 204);
/// Dark icon on a filled (selected) head.
const ON_COLOUR: Color32 = Color32::from_rgb(10, 12, 16);

/// A glass beacon (HANDOFF › Map markers): a ground halo at the tip, a stem fading toward the
/// ground, a dark glass head ringed in the colour with the icon in the lifted colour. Selected:
/// the head fills with the colour, a white ring and a pulsing halo. Far away: a glowing dot.
fn draw_pin(
    p: &egui::Painter,
    pin: &Pin,
    col: Color32,
    glyph: &str,
    selected: bool,
    hovered: bool,
    t: f32,
) {
    let a = if pin.occluded && !selected && !hovered {
        0.42
    } else {
        1.0
    };
    if pin.r <= 0.0 {
        p.circle_filled(pin.tip, 6.0, with_alpha(col, 0.22 * a));
        p.circle(
            pin.tip,
            3.0,
            with_alpha(col, a),
            Stroke::new(1.5, with_alpha(ON_COLOUR, 0.85 * a)),
        );
        return;
    }
    let r = pin.r * if hovered { 1.1 } else { 1.0 };
    let halo = vec2(pin.r * 0.95, pin.r * 0.34);
    if selected {
        // the halo pulses outward: scale 1 → 2.1, alpha 0.9 → 0 every 1.6 s
        let k = (t / 1.6).fract();
        p.add(Shape::ellipse_stroke(
            pin.tip,
            halo * (1.0 + 1.1 * k),
            Stroke::new(1.5, with_alpha(col, 0.9 * (1.0 - k) * a)),
        ));
    }
    p.add(Shape::ellipse_filled(
        pin.tip,
        halo,
        with_alpha(col, if selected { 0.32 } else { 0.2 } * a),
    ));
    p.add(Shape::ellipse_stroke(
        pin.tip,
        halo,
        Stroke::new(1.2, with_alpha(col, 0.9 * a)),
    ));
    p.circle_filled(pin.tip, 1.8, with_alpha(col, a));
    // the stem, 1.8 px, fading from 95 % under the head to 12 % at the ground
    let (top, bot) = (pin.head.y + r, pin.tip.y);
    if bot > top {
        let mut m = egui::Mesh::default();
        let (ct, cb) = (with_alpha(col, 0.95 * a), with_alpha(col, 0.12 * a));
        m.colored_vertex(pos2(pin.tip.x - 0.9, top), ct);
        m.colored_vertex(pos2(pin.tip.x + 0.9, top), ct);
        m.colored_vertex(pos2(pin.tip.x + 0.9, bot), cb);
        m.colored_vertex(pos2(pin.tip.x - 0.9, bot), cb);
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        p.add(Shape::mesh(m));
    }
    // the head
    p.circle_filled(
        pin.head + vec2(0.0, 2.0),
        r + 1.5,
        with_alpha(Color32::from_black_alpha(90), a),
    );
    if selected {
        p.circle_stroke(pin.head, r + 3.5, Stroke::new(1.5, Color32::WHITE));
    }
    let fill = if selected {
        with_alpha(col, a)
    } else {
        Color32::from_rgba_unmultiplied(12, 15, 21, (204.0 * a) as u8)
    };
    p.circle(
        pin.head,
        r,
        fill,
        Stroke::new(if hovered { 2.2 } else { 1.6 }, with_alpha(col, a)),
    );
    p.circle_stroke(
        pin.head,
        r - 2.0,
        Stroke::new(
            1.0,
            with_alpha(Color32::WHITE, if selected { 0.25 } else { 0.08 } * a),
        ),
    );
    let ic = if selected { ON_COLOUR } else { lift(col, 0.25) };
    crate::ui::icons::paint(p, pin.head, glyph, r * 1.2, with_alpha(ic, a));
}

struct Label {
    at: Pos2,
    /// Centre x of the pin a side label belongs to (the left-hand spot mirrors around it).
    anchor_x: f32,
    /// Left-aligned next to a pin head (else centred on `at`).
    side: bool,
    title: String,
    sub: String,
    col: Color32,
    glyph: Option<&'static str>,
    prio: u8,
    /// The selected marker's label (region labels get a white ring and a filled icon circle).
    selected: bool,
    /// Importance within the priority (towns before treasure chests).
    rank: u8,
    dist: f32,
}

/// How important a place's name is on the map (0 = most).
fn poi_rank(k: PoiKind) -> u8 {
    match k {
        PoiKind::Town => 0,
        PoiKind::QuestHub => 1,
        PoiKind::Dungeon => 2,
        PoiKind::BossArena => 3,
        PoiKind::FlightPoint => 4,
        PoiKind::Portal => 5,
        PoiKind::Graveyard => 6,
        PoiKind::Camp => 7,
        PoiKind::Landmark => 8,
        PoiKind::Vendor => 9,
        PoiKind::Shrine => 10,
        PoiKind::Vista => 11,
        PoiKind::Treasure => 12,
        PoiKind::Custom => 13,
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_markers(
    mut contexts: EguiContexts,
    gp: Res<Gameplay>,
    gp_ed: Res<GameplayEditor>,
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    terrain: Res<Terrain>,
    zones: Res<Zones>,
    water: Res<WaterBodies>,
    pointer: Res<Pointer>,
    stroke: Res<crate::tools::Stroke>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    time: Res<Time<Real>>,
    mut perf: Local<(f32, u32)>,
) -> Result {
    let t0 = std::time::Instant::now();
    if walk.cinematic {
        return Ok(());
    }
    let tab = ed.tab == Tab::Gameplay && !walk.active;
    let view = &gp_ed.view;
    if !(tab || (walk.active && view.walk) || (!walk.active && view.all_tabs)) {
        return Ok(());
    }
    let Ok((cam, cam_t)) = camera.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    // the editor's fonts (labels use its semibold face) are installed on the UI's first frame
    if !ctx.fonts(|f| {
        f.definitions()
            .families
            .contains_key(&crate::ui::theme::semibold())
    }) {
        return Ok(());
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("gameplay_markers")));
    let g = Ground {
        terrain: &terrain,
        zones: &zones,
        water: &water,
        nature: None,
    };
    let t = time.elapsed_secs();
    let pins = layout(&gp, view, &g, cam, cam_t, tab);
    let hovered = if tab && !pointer.over_ui {
        pointer.screen.and_then(|s| pick_pin(&pins, pos2(s.x, s.y)))
    } else {
        None
    };
    let sel = gp.selected;

    if tab {
        // ground decorations of the selection
        match sel {
            Some(GameplaySel::Poi(id)) => {
                if let Some(q) = gp.pois.iter().find(|q| q.id == id) {
                    let c = Vec2::new(q.pos[0], q.pos[2]);
                    discovery_ring(&painter, cam, cam_t, &g, c, q.radius, POI_COLOR, 2.0);
                    for n in gp.npcs.iter().filter(|n| n.poi == Some(id)) {
                        link(&painter, cam, cam_t, &g, Vec2::new(n.pos[0], n.pos[2]), c);
                    }
                }
            }
            Some(GameplaySel::Npc(id)) => {
                if let Some(n) = gp.npcs.iter().find(|q| q.id == id)
                    && let Some(q) = n.poi.and_then(|pid| gp.pois.iter().find(|q| q.id == pid))
                {
                    let c = Vec2::new(q.pos[0], q.pos[2]);
                    discovery_ring(
                        &painter,
                        cam,
                        cam_t,
                        &g,
                        c,
                        q.radius,
                        with_alpha(POI_COLOR, 0.5),
                        1.2,
                    );
                    link(&painter, cam, cam_t, &g, Vec2::new(n.pos[0], n.pos[2]), c);
                }
            }
            Some(sel @ (GameplaySel::Spawn(_) | GameplaySel::OreArea(_))) => {
                if let Some(poly) = region_outline(&gp, sel) {
                    let pts: Vec<Vec2> = poly.iter().map(|p| Vec2::new(p[0], p[1])).collect();
                    let col = match sel {
                        GameplaySel::OreArea(id) => gp
                            .ore_areas
                            .iter()
                            .find(|a| a.id == id)
                            .map(area_color)
                            .unwrap_or(Color32::GRAY),
                        GameplaySel::Spawn(id) => spawn_color(
                            gp.spawns
                                .iter()
                                .find(|s| s.id == id)
                                .and_then(|r| r.aggression(&gp)),
                        ),
                        _ => Color32::GRAY,
                    };
                    for l in draped(cam, cam_t, &g, &pts, true, 6.0) {
                        painter.add(Shape::line(
                            l.clone(),
                            Stroke::new(4.0, Color32::from_black_alpha(90)),
                        ));
                        painter.add(Shape::line(l, Stroke::new(2.0, col)));
                    }
                    for p in &pts {
                        if let Some(s) =
                            project(cam, cam_t, Vec3::new(p.x, g.surface(*p) + 0.3, p.y))
                        {
                            let near = pointer
                                .screen
                                .is_some_and(|c| s.distance(pos2(c.x, c.y)) <= 10.0);
                            painter.circle_filled(s, if near { 6.5 } else { 5.0 }, Color32::WHITE);
                            painter.circle_stroke(
                                s,
                                if near { 6.5 } else { 5.0 },
                                Stroke::new(1.5, col),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        // the spawn region or deposit area under the cursor (click to select it)
        if hovered.is_none()
            && !pointer.over_ui
            && let Some(h) = pointer.hit
        {
            let under: Option<(Vec<Vec2>, Color32)> = if let Some(a) =
                gp.ore_areas.iter().rev().find(|a| {
                    view.nodes && a.contains(h.xz()) && sel != Some(GameplaySel::OreArea(a.id))
                }) {
                Some((
                    a.polygon.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
                    area_color(a),
                ))
            } else {
                gp.spawns
                    .iter()
                    .rev()
                    .find(|r| {
                        view.spawns && r.contains(h.xz()) && sel != Some(GameplaySel::Spawn(r.id))
                    })
                    .map(|r| {
                        (
                            r.polygon.iter().map(|p| Vec2::new(p[0], p[1])).collect(),
                            spawn_color(r.aggression(&gp)),
                        )
                    })
            };
            if let Some((pts, col)) = under {
                for l in draped(cam, cam_t, &g, &pts, true, 8.0) {
                    painter.add(Shape::line(l, Stroke::new(1.5, with_alpha(col, 0.85))));
                }
            }
        }
        if let Some(GameplaySel::Poi(id)) = hovered
            && sel != hovered
            && let Some(q) = gp.pois.iter().find(|q| q.id == id)
        {
            discovery_ring(
                &painter,
                cam,
                cam_t,
                &g,
                Vec2::new(q.pos[0], q.pos[2]),
                q.radius,
                with_alpha(POI_COLOR, 0.45),
                1.2,
            );
        }
        // the spawn region or deposit area being drawn (a loop while lasso-dragging from the first
        // corner)
        let drawing = gp_ed.tool == GameplayTool::Spawn
            || (gp_ed.tool == GameplayTool::Ore && gp_ed.ore_mode == OreMode::Area);
        if drawing && !gp_ed.draft.is_empty() {
            let lasso = gp_ed.draft.len() == 1 && stroke.active && stroke.path.len() > 2;
            let mut pts: Vec<Vec2> = if lasso {
                stroke.path.iter().map(|p| p.xz()).collect()
            } else {
                gp_ed.draft.iter().map(|p| p.xz()).collect()
            };
            if !lasso
                && !pointer.over_ui
                && let Some(h) = pointer.hit
            {
                pts.push(h.xz());
            }
            let col = if gp_ed.tool == GameplayTool::Ore {
                ore_color(gp_ed.ore)
            } else {
                Color32::from_rgb(255, 214, 110)
            };
            for l in draped(cam, cam_t, &g, &pts, false, 6.0) {
                painter.add(Shape::line(
                    l.clone(),
                    Stroke::new(4.5, Color32::from_black_alpha(90)),
                ));
                painter.add(Shape::line(l, Stroke::new(2.5, col)));
            }
            if pts.len() >= 3 {
                for l in draped(cam, cam_t, &g, &[pts[pts.len() - 1], pts[0]], false, 6.0) {
                    painter.add(Shape::dashed_line(
                        &l,
                        Stroke::new(1.5, with_alpha(col, 0.7)),
                        8.0,
                        6.0,
                    ));
                }
            }
            for (i, p) in gp_ed.draft.iter().enumerate() {
                if let Some(s) = project(cam, cam_t, *p + Vec3::Y * 0.3) {
                    painter.circle_filled(s, if i == 0 { 6.0 } else { 4.5 }, col);
                    painter.circle_stroke(
                        s,
                        if i == 0 { 6.0 } else { 4.5 },
                        Stroke::new(1.5, Color32::from_black_alpha(160)),
                    );
                }
            }
        }
    }
    // the player start's facing
    if view.pois
        && let Some(s) = gp.player_start
    {
        let p = Vec2::new(s.pos[0], s.pos[2]);
        let d = cam_t
            .translation()
            .distance(Vec3::new(p.x, g.surface(p), p.y));
        facing_arrow(
            &painter,
            cam,
            cam_t,
            &g,
            p,
            s.yaw,
            (d * 0.06).clamp(8.0, 120.0),
            START_COLOR,
        );
    }
    if let Some(GameplaySel::Npc(id)) = sel
        && tab
        && let Some(n) = gp.npcs.iter().find(|q| q.id == id)
    {
        let p = Vec2::new(n.pos[0], n.pos[2]);
        let d = cam_t
            .translation()
            .distance(Vec3::new(p.x, g.surface(p), p.y));
        facing_arrow(
            &painter,
            cam,
            cam_t,
            &g,
            p,
            n.yaw,
            (d * 0.03).clamp(2.5, 40.0),
            NPC_COLOR,
        );
    }

    // pins, far ones first
    let mut order: Vec<&Pin> = pins.iter().filter(|p| !p.kind.region()).collect();
    order.sort_by(|a, b| b.dist.total_cmp(&a.dist));
    let mut labels: Vec<Label> = vec![];
    let lv = |l: Option<[u8; 2]>| l.map(level_label).unwrap_or_default();
    for pin in order {
        let is_sel = sel == Some(pin.sel);
        let is_hov = hovered == Some(pin.sel);
        let (col, glyph, title, sub, show_label, prio, rank) = match pin.sel {
            GameplaySel::PlayerStart => (
                START_COLOR,
                START_ICON,
                "Player Start".to_string(),
                String::new(),
                true,
                2,
                0,
            ),
            GameplaySel::Poi(id) => {
                let Some(q) = gp.pois.iter().find(|q| q.id == id) else {
                    continue;
                };
                let level = q
                    .level
                    .or_else(|| g.zone_levels(Vec2::new(q.pos[0], q.pos[2])));
                (
                    POI_COLOR,
                    poi_icon(q.kind),
                    q.name.clone(),
                    format!("{} {}", q.kind.name(), lv(level)),
                    pin.dist < 7000.0,
                    3,
                    poi_rank(q.kind),
                )
            }
            GameplaySel::Npc(id) => {
                let Some(q) = gp.npcs.iter().find(|q| q.id == id) else {
                    continue;
                };
                (
                    NPC_COLOR,
                    npc_icon(q.role),
                    q.name.clone(),
                    q.role.name().to_string(),
                    pin.dist < 500.0,
                    5,
                    0,
                )
            }
            GameplaySel::Node(id) => {
                let Some(q) = gp.nodes.iter().find(|q| q.id == id) else {
                    continue;
                };
                match q.ore() {
                    Some(o) => (
                        rarity_color(o),
                        ORE_ICON,
                        q.variant.clone(),
                        format!("{} · Tier {} {}", o.name(), o.tier(), o.rarity().name()),
                        pin.dist < 220.0,
                        6,
                        0,
                    ),
                    None => (
                        node_color(q.kind),
                        node_icon(q.kind),
                        q.variant.clone(),
                        format!("{} · tier {}", q.kind.name(), q.tier),
                        pin.dist < 220.0,
                        6,
                        0,
                    ),
                }
            }
            GameplaySel::Spawn(_) | GameplaySel::OreArea(_) => continue,
        };
        draw_pin(&painter, pin, col, glyph, is_sel, is_hov, t);
        if (view.labels && show_label && !pin.occluded && pin.r > 0.0) || is_sel || is_hov {
            // the subtitle: always for places and the start, else while hovered or selected
            let detail = is_sel || is_hov || matches!(pin.kind, PinKind::Poi | PinKind::Start);
            labels.push(Label {
                at: pin.head + vec2(pin.r.max(4.0) + 6.0, 0.0),
                anchor_x: pin.head.x,
                side: true,
                title,
                sub: if detail { sub } else { String::new() },
                col,
                glyph: None,
                prio: if is_sel {
                    0
                } else if is_hov {
                    1
                } else {
                    prio
                },
                selected: is_sel,
                rank,
                dist: pin.dist,
            });
        }
    }
    // spawn region labels
    if tab {
        for pin in pins.iter().filter(|p| p.kind == PinKind::Spawn) {
            let GameplaySel::Spawn(id) = pin.sel else {
                continue;
            };
            let Some(r) = gp.spawns.iter().find(|s| s.id == id) else {
                continue;
            };
            let names: Vec<&str> = r
                .entries
                .iter()
                .filter_map(|e| gp.creature(e.creature))
                .map(|c| c.name.as_str())
                .collect();
            let creatures = match names.len() {
                0 => "no creatures".to_string(),
                1 | 2 => names.join(", "),
                n => format!("{}, {} +{}", names[0], names[1], n - 2),
            };
            let alive = r.expected_alive();
            let is_sel = sel == Some(pin.sel);
            let is_hov = hovered == Some(pin.sel);
            if !view.labels && !is_sel && !is_hov {
                continue;
            }
            labels.push(Label {
                at: pin.head,
                anchor_x: pin.head.x,
                side: false,
                title: r.name.clone(),
                sub: format!(
                    "{} · {creatures}{}",
                    level_label(r.level),
                    if alive > 0 {
                        format!(" · ~{alive}")
                    } else {
                        String::new()
                    }
                ),
                col: spawn_color(r.aggression(&gp)),
                glyph: Some(SPAWN_ICON),
                prio: if is_sel {
                    0
                } else if is_hov {
                    1
                } else {
                    4
                },
                selected: is_sel,
                rank: 0,
                dist: pin.dist,
            });
        }
    }
    // deposit area labels
    if tab {
        for pin in pins.iter().filter(|p| p.kind == PinKind::Area) {
            let GameplaySel::OreArea(id) = pin.sel else {
                continue;
            };
            let Some(a) = gp.ore_areas.iter().find(|a| a.id == id) else {
                continue;
            };
            let is_sel = sel == Some(pin.sel);
            let is_hov = hovered == Some(pin.sel);
            if !view.labels && !is_sel && !is_hov {
                continue;
            }
            let n = gp.nodes.iter().filter(|n| n.area == Some(id)).count();
            labels.push(Label {
                at: pin.head,
                anchor_x: pin.head.x,
                side: false,
                title: a.name.clone(),
                sub: format!(
                    "{} · {n} deposit{}",
                    a.mix_label(),
                    if n == 1 { "" } else { "s" }
                ),
                col: area_color(a),
                glyph: Some(AREA_ICON),
                prio: if is_sel {
                    0
                } else if is_hov {
                    1
                } else {
                    4
                },
                selected: is_sel,
                rank: 0,
                dist: pin.dist,
            });
        }
    }
    draw_labels(&painter, labels);
    if hovered.is_some() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // WB_GP_PERF=1: the markers' CPU time (layout + painting), averaged over 120 frames
    if std::env::var("WB_GP_PERF").is_ok() {
        perf.0 += t0.elapsed().as_secs_f32() * 1000.0;
        perf.1 += 1;
        if perf.1 == 120 {
            info!(
                "GPPERF markers {:.2} ms/frame for {} pins",
                perf.0 / 120.0,
                pins.len()
            );
            *perf = (0.0, 0);
        }
    }
    Ok(())
}

/// Labels in priority order, skipping any that would overlap one already placed. Pin labels are
/// dark glass pills with a colour dot before the title; region labels (spawn regions, deposit
/// areas) are round pills with the icon in a colour circle.
fn draw_labels(p: &egui::Painter, mut labels: Vec<Label>) {
    labels.sort_by(|a, b| {
        (a.prio, a.rank)
            .cmp(&(b.prio, b.rank))
            .then(a.dist.total_cmp(&b.dist))
    });
    let mut placed: Vec<Rect> = vec![];
    let f1 = FontId::new(12.5, crate::ui::theme::semibold());
    let f2 = FontId::proportional(11.0);
    let sub_col = Color32::from_rgba_unmultiplied(255, 255, 255, 184);
    let rim = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 31));
    let shadow = egui::epaint::Shadow {
        offset: [0, 5],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    for l in labels {
        let g1 = p.layout_no_wrap(l.title.clone(), f1.clone(), Color32::WHITE);
        let g2 = (!l.sub.is_empty()).then(|| p.layout_no_wrap(l.sub.clone(), f2.clone(), sub_col));
        let region = l.glyph.is_some();
        // pin labels: padding 10 / 4–5, a 7 px dot + 7 px gap, the subtitle under the title text;
        // region labels: a 26 px icon circle at 4 px, 8 px gap, 12 px right padding
        let (lead, pad_r) = if region {
            (4.0 + 26.0 + 8.0, 12.0)
        } else {
            (10.0 + 14.0, 10.0)
        };
        let text_w = g1
            .size()
            .x
            .max(g2.as_ref().map(|g| g.size().x).unwrap_or(0.0));
        let text_h = g1.size().y + g2.as_ref().map(|g| g.size().y).unwrap_or(0.0);
        let w = lead + text_w + pad_r;
        let h = if region {
            (text_h + 8.0).max(34.0)
        } else {
            text_h + 9.0
        };
        // beside a pin: right, else left of it; region labels: centred, else above or below
        let spots = if l.side {
            [
                Rect::from_min_size(l.at - vec2(0.0, h * 0.5), vec2(w, h)),
                Rect::from_min_size(
                    l.at - vec2(w + 2.0 * (l.at.x - l.anchor_x), h * 0.5),
                    vec2(w, h),
                ),
                Rect::NOTHING,
            ]
        } else {
            [
                Rect::from_center_size(l.at, vec2(w, h)),
                Rect::from_center_size(l.at - vec2(0.0, h + 4.0), vec2(w, h)),
                Rect::from_center_size(l.at + vec2(0.0, h + 4.0), vec2(w, h)),
            ]
        };
        let free =
            |r: &Rect| r.is_positive() && !placed.iter().any(|q| q.expand(2.0).intersects(*r));
        let Some(r) = (if l.prio <= 1 {
            Some(spots[0])
        } else {
            spots.into_iter().find(free)
        }) else {
            continue;
        };
        placed.push(r);
        let rad = if region {
            CornerRadius::same(255)
        } else {
            CornerRadius::same(9)
        };
        p.add(shadow.as_shape(r, rad));
        p.rect_filled(r, rad, GLASS_DARK);
        p.rect_stroke(r, rad, rim, egui::StrokeKind::Inside);
        let y0 = r.center().y - text_h * 0.5;
        if let Some(gl) = l.glyph {
            let c = pos2(r.left() + 4.0 + 13.0, r.center().y);
            if l.selected {
                p.rect_stroke(
                    r.expand(1.5),
                    rad,
                    Stroke::new(1.5, Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
                p.circle_filled(c, 13.0, l.col);
                crate::ui::icons::paint(p, c, gl, 15.0, ON_COLOUR);
            } else {
                p.circle(c, 13.0, with_alpha(l.col, 0.22), Stroke::new(1.2, l.col));
                crate::ui::icons::paint(p, c, gl, 15.0, lift(l.col, 0.25));
            }
            let x = r.left() + lead;
            let h1 = g1.size().y;
            p.galley(pos2(x, y0), g1, Color32::WHITE);
            if let Some(g2) = g2 {
                p.galley(pos2(x, y0 + h1), g2, sub_col);
            }
        } else {
            let h1 = g1.size().y;
            p.circle_filled(pos2(r.left() + 10.0 + 3.5, y0 + h1 * 0.5), 3.5, l.col);
            let x = r.left() + lead;
            p.galley(pos2(x, y0), g1, Color32::WHITE);
            if let Some(g2) = g2 {
                p.galley(pos2(x, y0 + h1), g2, sub_col);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn discovery_ring(
    p: &egui::Painter,
    cam: &Camera,
    cam_t: &GlobalTransform,
    g: &Ground,
    c: Vec2,
    radius: f32,
    col: Color32,
    w: f32,
) {
    let n = 72;
    let pts: Vec<Vec2> = (0..n)
        .map(|k| c + Vec2::from_angle(k as f32 / n as f32 * std::f32::consts::TAU) * radius)
        .collect();
    for l in draped(cam, cam_t, g, &pts, true, radius.max(4.0)) {
        p.add(Shape::line(
            l.clone(),
            Stroke::new(w + 2.0, Color32::from_black_alpha(70)),
        ));
        p.add(Shape::dashed_line(&l, Stroke::new(w, col), 10.0, 5.0));
    }
}

fn link(p: &egui::Painter, cam: &Camera, cam_t: &GlobalTransform, g: &Ground, a: Vec2, b: Vec2) {
    for l in draped(cam, cam_t, g, &[a, b], false, 4.0) {
        p.add(Shape::dashed_line(
            &l,
            Stroke::new(1.6, with_alpha(NPC_COLOR, 0.9)),
            6.0,
            4.0,
        ));
    }
}

/// An arrow on the ground from `p` along `yaw` (0 = +x, counter-clockwise from above).
#[allow(clippy::too_many_arguments)]
fn facing_arrow(
    p: &egui::Painter,
    cam: &Camera,
    cam_t: &GlobalTransform,
    g: &Ground,
    at: Vec2,
    yaw: f32,
    len: f32,
    col: Color32,
) {
    let dir = Vec2::new(yaw.cos(), -yaw.sin());
    let side = dir.perp();
    let w = |q: Vec2| project(cam, cam_t, Vec3::new(q.x, g.surface(q) + 0.4, q.y));
    let (Some(a), Some(b)) = (w(at + dir * len * 0.25), w(at + dir * len * 0.68)) else {
        return;
    };
    let (Some(tip), Some(l), Some(r)) = (
        w(at + dir * len),
        w(at + dir * len * 0.6 + side * len * 0.2),
        w(at + dir * len * 0.6 - side * len * 0.2),
    ) else {
        return;
    };
    p.line_segment([a, b], Stroke::new(6.0, Color32::from_black_alpha(110)));
    p.line_segment([a, b], Stroke::new(3.5, col));
    p.add(Shape::convex_polygon(
        vec![tip, l, r],
        col,
        Stroke::new(1.5, Color32::WHITE),
    ));
}

// ------------------------------------------------------------------ spawn regions on the ground

#[derive(Default)]
struct OverlayCache {
    image: Option<Handle<Image>>,
    key: u64,
    res: u32,
    built: f32,
}

const OVERLAY_OWNER: &str = "gameplay";

/// Paints the spawn regions into the terrain overlay while the Gameplay tab is open (and
/// clears our overlay when leaving it). Rebuilt only when a region changes, at most ~8×/s
/// while one is being dragged.
#[allow(clippy::too_many_arguments)]
fn spawn_overlay(
    ed: Res<Editor>,
    walk: Res<crate::walk::WalkState>,
    gp: Res<Gameplay>,
    gp_ed: Res<GameplayEditor>,
    terrain: Res<Terrain>,
    mut overlay: ResMut<TerrainOverlay>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time<Real>>,
    mut cache: Local<OverlayCache>,
) {
    let show = ed.tab == Tab::Gameplay
        && !walk.active
        && !walk.cinematic
        && ((gp_ed.view.spawns && !gp.spawns.is_empty())
            || (gp_ed.view.nodes && !gp.ore_areas.is_empty()));
    if !show {
        overlay.clear(OVERLAY_OWNER);
        return;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    terrain.size().to_bits().hash(&mut h);
    for r in &gp.spawns {
        r.id.hash(&mut h);
        for p in &r.polygon {
            p[0].to_bits().hash(&mut h);
            p[1].to_bits().hash(&mut h);
        }
        spawn_color(r.aggression(&gp)).to_array().hash(&mut h);
        (gp.selected == Some(GameplaySel::Spawn(r.id))).hash(&mut h);
    }
    gp_ed.view.spawns.hash(&mut h);
    gp_ed.view.nodes.hash(&mut h);
    for a in &gp.ore_areas {
        a.id.hash(&mut h);
        for p in &a.polygon {
            p[0].to_bits().hash(&mut h);
            p[1].to_bits().hash(&mut h);
        }
        area_color(a).to_array().hash(&mut h);
        (gp.selected == Some(GameplaySel::OreArea(a.id))).hash(&mut h);
    }
    let key = h.finish();
    let now = time.elapsed_secs();
    let res = ((terrain.size() / 2.0) as u32).clamp(512, 2048);
    if key != cache.key && (now - cache.built > 0.12 || cache.image.is_none()) {
        let data = paint_regions(&gp, &terrain, res, gp_ed.view.spawns, gp_ed.view.nodes);
        let reuse = cache.res == res && cache.image.as_ref().is_some_and(|h| images.contains(h));
        if reuse {
            if let Some(mut img) = cache.image.as_ref().and_then(|h| images.get_mut(h)) {
                img.data = Some(data);
            }
        } else {
            let mut img = new_overlay_image(res, res);
            img.data = Some(data);
            cache.image = Some(images.add(img));
            cache.res = res;
        }
        cache.key = key;
        cache.built = now;
    }
    if let Some(img) = &cache.image
        && (!overlay.is_shown_by(OVERLAY_OWNER) || overlay.image.as_ref() != Some(img))
    {
        overlay.show(OVERLAY_OWNER, img.clone(), 1.0);
    }
}

/// RGBA texels (sRGB, row 0 at −z) with every region filled (diagonal hatching) and outlined:
/// spawn regions (`spawns`) and ore deposit areas (`areas`, hatched the other way).
fn paint_regions(gp: &Gameplay, t: &Terrain, res: u32, spawns: bool, areas: bool) -> Vec<u8> {
    let n = res as usize;
    let mut px = vec![0u8; n * n * 4];
    let half = t.half();
    let texel = t.size() / res as f32;
    let to_tex = |v: f32| (v + half) / texel;
    // (outline, colour, selected, hatching direction); the selection on top
    let mut order: Vec<(&[[f32; 2]], Color32, bool, bool)> = vec![];
    if spawns {
        order.extend(gp.spawns.iter().map(|r| {
            (
                r.polygon.as_slice(),
                spawn_color(r.aggression(gp)),
                gp.selected == Some(GameplaySel::Spawn(r.id)),
                false,
            )
        }));
    }
    if areas {
        order.extend(gp.ore_areas.iter().map(|a| {
            (
                a.polygon.as_slice(),
                area_color(a),
                gp.selected == Some(GameplaySel::OreArea(a.id)),
                true,
            )
        }));
    }
    order.retain(|r| r.0.len() >= 3);
    order.sort_by_key(|r| r.2);
    let over = |px: &mut [u8], i: usize, j: usize, c: Color32, a: f32| {
        let k = (j * n + i) * 4;
        let da = px[k + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        for (ch, s) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
            let d = px[k + ch] as f32;
            px[k + ch] = ((s as f32 * a + d * da * (1.0 - a)) / oa).round() as u8;
        }
        px[k + 3] = (oa * 255.0).round() as u8;
    };
    for (outline, col, selected, flip) in order {
        let poly: Vec<Vec2> = outline
            .iter()
            .map(|p| Vec2::new(to_tex(p[0]), to_tex(p[1])))
            .collect();
        let (lo, hi) = poly.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(a, b), p| (a.min(*p), b.max(*p)),
        );
        let (j0, j1) = (
            (lo.y.floor().max(0.0)) as usize,
            (hi.y.ceil().max(0.0) as usize).min(n - 1),
        );
        // (deposit areas lighter: their rocks are the content)
        let fill = if selected { 0.30 } else { 0.20 } * if flip { 0.55 } else { 1.0 };
        // scanline fill
        let mut xs: Vec<f32> = vec![];
        for j in j0..=j1 {
            let y = j as f32 + 0.5;
            xs.clear();
            for k in 0..poly.len() {
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                if (a.y > y) != (b.y > y) {
                    xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for s in xs.chunks_exact(2) {
                let (i0, i1) = (
                    (s[0] - 0.5).ceil().max(0.0) as usize,
                    ((s[1] - 0.5).floor().max(-1.0) as isize).min(n as isize - 1),
                );
                for i in i0 as isize..=i1 {
                    let i = i as usize;
                    // hatching: diagonal stripes ~8 m apart
                    let diag = if flip {
                        (i + n) as f32 - j as f32
                    } else {
                        (i + j) as f32
                    };
                    let stripe = ((diag * texel / 8.0).fract() < 0.32) as u8 as f32;
                    over(&mut px, i, j, col, fill + 0.12 * stripe);
                }
            }
        }
        // outline
        let w = 2.2_f32;
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            let (mn, mx) = (
                a.min(b) - Vec2::splat(w + 1.0),
                a.max(b) + Vec2::splat(w + 1.0),
            );
            let ab = b - a;
            let l2 = ab.length_squared().max(1e-6);
            for j in (mn.y.floor().max(0.0) as usize)..=(mx.y.ceil().max(0.0) as usize).min(n - 1) {
                for i in
                    (mn.x.floor().max(0.0) as usize)..=(mx.x.ceil().max(0.0) as usize).min(n - 1)
                {
                    let q = Vec2::new(i as f32 + 0.5, j as f32 + 0.5);
                    let tt = ((q - a).dot(ab) / l2).clamp(0.0, 1.0);
                    let d = q.distance(a + ab * tt);
                    if d < w {
                        let edge = 1.0 - ((d - (w - 1.0)).max(0.0));
                        let k4 = (j * n + i) * 4;
                        // edges of one region don't stack: keep the strongest
                        let a_now = px[k4 + 3] as f32 / 255.0;
                        let want = if flip { 0.6 } else { 0.92 } * edge;
                        if want > a_now {
                            let lift = |v: u8| (v as f32 * 0.8 + 255.0 * 0.2) as u8;
                            over(
                                &mut px,
                                i,
                                j,
                                Color32::from_rgb(lift(col.r()), lift(col.g()), lift(col.b())),
                                ((want - a_now) / (1.0 - a_now).max(1e-3)).clamp(0.0, 1.0),
                            );
                        }
                    }
                }
            }
        }
    }
    px
}
