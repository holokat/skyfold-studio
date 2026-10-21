//! Making a flight by clicking places on the ground: "Pick places" in the Paths window (or "Pick
//! again" on a flight) turns clicks in the viewport into numbered pins, with a bar at the top of
//! the screen saying what to do. Enter, a double-click or "Make the flight" turns them into a
//! camera path (`flythrough::flight_keys`); Backspace drops the last pin, Esc cancels. The clicks
//! are taken before the tab's tools see them.
//!
//! The places of the selected flight show as numbered pins while the Paths window is open.

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, Pos2, RichText, Stroke, pos2,
    vec2,
};
use egui_phosphor::regular as icon;

use super::flythrough::{self, CameraPath, FlightSpeed, FlightStyle, Flythrough, Route};
use super::hud::MotionUi;
use super::{MotionData, PanelTab, edit, playtest};
use crate::camera::{MainCamera, Pointer};
use crate::history::History;
use crate::terrain::Terrain;
use crate::ui::theme::{ACCENT, ON_ACCENT, TEXT, TEXT_DIM, semibold};
use crate::walk::WalkState;

const PIN: Color32 = Color32::from_rgb(255, 196, 64);

/// Places being picked.
#[derive(Default)]
pub struct Picking {
    /// [x, z] on the ground, in order
    pub places: Vec<Vec2>,
    /// re-picking this flight's places (else a new flight)
    pub path: Option<u32>,
    /// "Make the flight" was pressed (handled with the clicks)
    pub finish: bool,
    press: Option<Vec2>,
    last_click: Option<(f32, Vec2)>,
}

impl Picking {
    pub fn new(path: Option<u32>, places: Vec<Vec2>) -> Self {
        Self {
            places,
            path,
            ..default()
        }
    }
}

/// Make a flight through `places` (or re-make flight `replace` through them), undoable. Style and
/// speed default to the replaced flight's (else bird's-eye, normal). Returns the path's id.
#[allow(clippy::too_many_arguments)]
pub fn make_flight(
    data: &mut MotionData,
    history: &mut History,
    terrain: &Terrain,
    places: &[Vec2],
    style: Option<FlightStyle>,
    speed: Option<FlightSpeed>,
    name: Option<String>,
    replace: Option<u32>,
) -> Option<u32> {
    if places.is_empty() {
        return None;
    }
    let replace = replace.filter(|id| data.paths.iter().any(|p| p.id == *id));
    let mut out = None;
    edit(
        data,
        history,
        if replace.is_some() {
            "Change flight places"
        } else {
            "New flight"
        },
        |d| {
            let old = replace
                .and_then(|id| d.paths.iter().find(|p| p.id == id))
                .and_then(|p| p.route.clone());
            let route = Route {
                places: places.iter().map(|p| p.to_array()).collect(),
                style: style
                    .or(old.as_ref().map(|r| r.style))
                    .unwrap_or(FlightStyle::Aerial),
                speed: speed.or(old.as_ref().map(|r| r.speed)).unwrap_or_default(),
            };
            let keys = flythrough::flight_keys(&route, terrain);
            match replace.and_then(|id| d.paths.iter_mut().find(|p| p.id == id)) {
                Some(p) => {
                    p.keys = keys;
                    p.route = Some(route);
                    if let Some(n) = name.clone() {
                        p.name = n;
                    }
                    out = Some(p.id);
                }
                None => {
                    let id = d.alloc_id();
                    let n = d.paths.iter().filter(|p| p.route.is_some()).count() + 1;
                    d.paths.push(CameraPath {
                        id,
                        name: name.clone().unwrap_or_else(|| format!("Flight {n}")),
                        keys,
                        timelapse_hours: 0.0,
                        looped: false,
                        route: Some(route),
                    });
                    out = Some(id);
                }
            }
            true
        },
    );
    out
}

/// Re-make a flight's keys after its style or speed changed (undoable).
pub fn restyle(
    data: &mut MotionData,
    history: &mut History,
    terrain: &Terrain,
    id: u32,
    style: FlightStyle,
    speed: FlightSpeed,
) {
    edit(data, history, "Flight style", |d| {
        let Some(p) = d.paths.iter_mut().find(|p| p.id == id) else {
            return false;
        };
        let Some(r) = p.route.as_mut() else {
            return false;
        };
        r.style = style;
        r.speed = speed;
        p.keys = flythrough::flight_keys(r, terrain);
        true
    });
}

/// Every point of interest, nearest first from the player start (or the first one).
pub fn poi_tour(gp: &crate::gameplay::Gameplay) -> Vec<Vec2> {
    let mut left: Vec<Vec2> = gp
        .pois
        .iter()
        .map(|q| Vec2::new(q.pos[0], q.pos[2]))
        .collect();
    let mut at = gp
        .player_start
        .map(|s| Vec2::new(s.pos[0], s.pos[2]))
        .or(left.first().copied())
        .unwrap_or_default();
    let mut out = vec![];
    while !left.is_empty() {
        let i = (0..left.len())
            .min_by(|a, b| left[*a].distance(at).total_cmp(&left[*b].distance(at)))
            .unwrap();
        at = left.swap_remove(i);
        out.push(at);
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(super) fn pick_input(
    mut mui: ResMut<MotionUi>,
    pointer: Res<Pointer>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time<Real>>,
    walk: Res<WalkState>,
    fly: Res<Flythrough>,
    terrain: Res<Terrain>,
    mut data: ResMut<MotionData>,
    mut history: ResMut<History>,
    mut ed: ResMut<crate::editor::Editor>,
    mut meta: ResMut<crate::world::WorldMeta>,
) {
    let Some(pk) = mui.picking.as_mut() else {
        return;
    };
    if walk.active || fly.playing.is_some() {
        mui.picking = None;
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        mui.picking = None;
        ed.toast("Flight cancelled");
        return;
    }
    let typing = pointer.keyboard_captured;
    let mut finish = std::mem::take(&mut pk.finish)
        || (!typing
            && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter)));
    if !typing && keys.just_pressed(KeyCode::Backspace) {
        pk.places.pop();
    }
    let screen = pointer.screen.unwrap_or_default();
    if buttons.just_pressed(MouseButton::Left) && !pointer.over_ui && !keys.pressed(KeyCode::Space)
    {
        // ours: no brush stroke or click for the tab's tool
        buttons.clear_just_pressed(MouseButton::Left);
        pk.press = Some(screen);
    }
    if let Some(start) = pk.press
        && !buttons.pressed(MouseButton::Left)
    {
        pk.press = None;
        if screen.distance(start) < 6.0 {
            let now = time.elapsed_secs();
            let double = pk
                .last_click
                .is_some_and(|(t, p)| now - t < 0.4 && p.distance(screen) < 8.0);
            pk.last_click = Some((now, screen));
            if double && !pk.places.is_empty() {
                finish = true;
            } else if let Some(h) = pointer.hit {
                pk.places.push(h.xz());
            }
        }
    }
    if !finish {
        return;
    }
    if pk.places.is_empty() {
        ed.toast("Click a place on the ground first (Esc cancels)");
        return;
    }
    let pk = mui.picking.take().unwrap();
    if let Some(id) = make_flight(
        &mut data,
        &mut history,
        &terrain,
        &pk.places,
        None,
        None,
        None,
        pk.path,
    ) {
        meta.dirty = true;
        let p = data.paths.iter().find(|p| p.id == id);
        let secs = p.map(|p| p.duration()).unwrap_or(0.0);
        let what = if pk.places.len() == 1 {
            "a circle around the place".to_string()
        } else {
            format!("{} places", pk.places.len())
        };
        ed.toast(format!(
            "Flight ready: {what}, {}. Press Preview to watch it.",
            playtest::clock(secs)
        ));
        mui.open = true;
        mui.tab = PanelTab::Paths;
        mui.path = Some(id);
        mui.key = None;
        mui.rename.clear();
    }
}

/// The pins being picked (with the bar saying what to do) or the selected flight's places.
pub(super) fn draw(
    ctx: &egui::Context,
    mui: &mut MotionUi,
    data: &MotionData,
    terrain: &Terrain,
    pointer: &Pointer,
    cam: Option<(&Camera, &GlobalTransform)>,
) {
    let Some((cam, cam_t)) = cam else { return };
    let project = |p: Vec2, lift: f32| -> Option<Pos2> {
        let y = terrain.height_at(p.x, p.y).max(terrain.sea_level) + lift;
        let v = cam.world_to_viewport(cam_t, Vec3::new(p.x, y, p.y)).ok()?;
        Some(pos2(v.x, v.y))
    };
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("flight_pins")));
    // a line over the ground through the places
    let route_line = |pts: &[Vec2], col: Color32| {
        for w in pts.windows(2) {
            let m = ((w[0].distance(w[1]) / (terrain.cell * 3.0).max(4.0)).ceil() as usize)
                .clamp(1, 96);
            let line: Vec<Option<Pos2>> = (0..=m)
                .map(|s| project(w[0].lerp(w[1], s as f32 / m as f32), 1.5))
                .collect();
            for (width, a) in [(6.0, 0.18), (2.0, 1.0)] {
                for seg in line.windows(2) {
                    if let (Some(p), Some(q)) = (seg[0], seg[1]) {
                        painter.line_segment([p, q], Stroke::new(width, col.gamma_multiply(a)));
                    }
                }
            }
        }
    };
    let pin = |p: Vec2, n: usize, alpha: f32, big: bool| {
        let (Some(base), Some(top)) = (project(p, 0.0), project(p, 14.0)) else {
            return;
        };
        let r = if big { 11.0 } else { 9.0 };
        let head = pos2(base.x, base.y.min(top.y.min(base.y - 18.0)));
        painter.line_segment(
            [base, head],
            Stroke::new(2.0, Color32::from_black_alpha((150.0 * alpha) as u8)),
        );
        painter.circle_filled(base, 3.0, PIN.gamma_multiply(alpha));
        painter.circle_filled(
            head,
            r + 1.5,
            Color32::from_black_alpha((120.0 * alpha) as u8),
        );
        painter.circle_filled(head, r, PIN.gamma_multiply(alpha));
        painter.text(
            head,
            Align2::CENTER_CENTER,
            format!("{n}"),
            FontId::new(if big { 12.5 } else { 11.0 }, semibold()),
            Color32::from_black_alpha((230.0 * alpha) as u8),
        );
    };

    if let Some(pk) = &mui.picking {
        let mut pts = pk.places.clone();
        let hover = pointer.hit.filter(|_| !pointer.over_ui).map(|h| h.xz());
        if let Some(h) = hover {
            pts.push(h);
        }
        if pts.len() >= 2 {
            route_line(&pk.places, PIN);
            if let (Some(h), Some(l)) = (hover, pk.places.last()) {
                route_line(&[*l, h], PIN.gamma_multiply(0.45));
            }
        }
        for (i, p) in pk.places.iter().enumerate() {
            pin(*p, i + 1, 1.0, true);
        }
        if let Some(h) = hover {
            pin(h, pk.places.len() + 1, 0.45, true);
        }
        bar(ctx, mui);
        return;
    }
    if !mui.open || mui.tab != PanelTab::Paths {
        return;
    }
    if let Some(r) = mui
        .path
        .and_then(|id| data.paths.iter().find(|p| p.id == id))
        .and_then(|p| p.route.as_ref())
    {
        let pts: Vec<Vec2> = r.places.iter().map(|p| Vec2::from_array(*p)).collect();
        for (i, p) in pts.iter().enumerate() {
            pin(*p, i + 1, 0.9, false);
        }
    }
}

/// The bar at the top of the viewport while picking: what to do next, Undo / Make / Cancel.
fn bar(ctx: &egui::Context, mui: &mut MotionUi) {
    let Some(pk) = mui.picking.as_mut() else {
        return;
    };
    let n = pk.places.len();
    let (head, sub) = match n {
        0 => ("Click a place on the ground to start".to_string(), "One place: the camera circles around it. More: it flies from one to the next, in the order you click.".to_string()),
        1 => ("1 place picked".to_string(), "Press Enter to circle around it, or click the next place to fly to.".to_string()),
        _ => {
            let len: f32 = pk.places.windows(2).map(|w| w[0].distance(w[1])).sum();
            let dist = if len >= 1000.0 { format!("{:.1} km", len / 1000.0) } else { format!("{len:.0} m") };
            (format!("{n} places · {dist}"), "Keep clicking to add places, or press Enter to make the flight.".to_string())
        }
    };
    let screen = ctx.content_rect();
    let mut cancel = false;
    egui::Area::new(Id::new("flight_pick_bar"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_TOP, vec2(0.0, 64.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(crate::ui::theme::GLASS_SOLID)
                .stroke(Stroke::new(1.0, PIN.gamma_multiply(0.6)))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_max_width((screen.width() - 40.0).min(560.0));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::MAP_PIN_PLUS).size(22.0).color(PIN));
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(head)
                                    .font(FontId::new(15.0, semibold()))
                                    .color(Color32::WHITE),
                            );
                            ui.label(RichText::new(sub).small().color(TEXT_DIM));
                        });
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let make = egui::Button::new(
                            RichText::new(format!("{}  Make the flight  (Enter)", icon::CHECK))
                                .color(ON_ACCENT),
                        )
                        .fill(ACCENT)
                        .corner_radius(CornerRadius::same(8));
                        if ui.add_enabled(n > 0, make).clicked() {
                            pk.finish = true;
                        }
                        if ui
                            .add_enabled(
                                n > 0,
                                egui::Button::new(
                                    RichText::new(format!("{} Undo", icon::ARROW_U_UP_LEFT))
                                        .color(TEXT),
                                ),
                            )
                            .on_hover_text("Remove the last place (Backspace)")
                            .clicked()
                        {
                            pk.places.pop();
                        }
                        if ui
                            .button(RichText::new(format!("{} Cancel  (Esc)", icon::X)).color(TEXT))
                            .clicked()
                        {
                            cancel = true;
                        }
                    });
                });
        });
    if cancel {
        mui.picking = None;
    }
}

/// The camera, for the overlay.
pub(super) type CamQuery<'w, 's> =
    Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<MainCamera>>;

/// Draws the overlay (its own system: the windows system has enough parameters).
pub(super) fn overlay(
    mut contexts: EguiContexts,
    mut mui: ResMut<MotionUi>,
    data: Res<MotionData>,
    terrain: Res<Terrain>,
    pointer: Res<Pointer>,
    walk: Res<WalkState>,
    fly: Res<Flythrough>,
    cam: CamQuery,
) -> Result {
    if walk.active
        || walk.cinematic
        || fly.playing.is_some()
        || (mui.picking.is_none() && !mui.open)
    {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?.clone();
    draw(&ctx, &mut mui, &data, &terrain, &pointer, cam.single().ok());
    Ok(())
}
