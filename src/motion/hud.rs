//! What the explorer sees on screen and what the designer uses in the editor:
//!
//! - walking: a chip with the current way of moving and its speed, depth under water and the
//!   breath meter (the walk readout and key hints are drawn by `ui::walk_hud`);
//! - playtesting: a compass strip with the points of interest, a minimap, the session clock /
//!   distance / discoveries and discovery toasts;
//! - in the editor: the Paths & Playtests window (camera path keys, preview and capture; sessions,
//!   their routes and heat map, export) and the summary after a session.

use bevy::prelude::*;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Order, RichText, Stroke, pos2, vec2,
};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use egui_phosphor::regular as icon;

use super::flythrough::{self, CaptureSettings, Ease, FlightSpeed, FlightStyle, Flythrough};
use super::movement::{self, InWater, MoveMode};
use super::playtest::{self, Playtest};
use super::{MotionAction, MotionData, PanelTab, edit};
use crate::camera::{MainCamera, OrbitCamera};
use crate::gameplay::Gameplay;
use crate::history::History;
use crate::terrain::Terrain;
use crate::ui::theme::{ACCENT, ON_ACCENT, TEXT, TEXT_DIM, semibold};
use crate::ui::tips;
use crate::ui::widgets::{
    DropdownExt, TrackSlider, dim, menu_check, menu_option, primary_button, section, segmented,
    separator, toggle, wide_button,
};
use crate::walk::WalkState;

/// The motion window's state.
#[derive(Resource)]
pub struct MotionUi {
    pub open: bool,
    pub tab: PanelTab,
    /// selected camera path and key
    pub path: Option<u32>,
    pub key: Option<usize>,
    pub capture: CaptureSettings,
    /// places being clicked for a new flight (or a flight's new places)
    pub picking: Option<super::pick::Picking>,
    pub(super) rename: String,
    /// a slider / drag in progress: its undo label and the data before it began
    dragging: Option<(String, MotionData)>,
}

impl Default for MotionUi {
    fn default() -> Self {
        Self {
            open: false,
            tab: PanelTab::Paths,
            path: None,
            key: None,
            capture: CaptureSettings::default(),
            rename: String::new(),
            dragging: None,
            picking: None,
        }
    }
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MotionUi>()
            .add_systems(Update, open_panel)
            .add_systems(
                Update,
                super::pick::pick_input
                    .after(crate::camera::CameraSystems)
                    .before(crate::tools::ToolSystems::Input),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (hud, editor_windows, super::pick::overlay),
            );
    }
}

fn open_panel(
    mut actions: MessageReader<MotionAction>,
    mut ui: ResMut<MotionUi>,
    data: Res<MotionData>,
) {
    for a in actions.read() {
        if let MotionAction::OpenPanel(tab) = a {
            ui.open = true;
            ui.tab = *tab;
            // show the latest path's keys straight away
            if ui
                .path
                .is_none_or(|id| !data.paths.iter().any(|p| p.id == id))
            {
                ui.path = data.paths.last().map(|p| p.id);
                ui.rename.clear();
            }
        }
    }
}

fn panel_rect(painter: &egui::Painter, r: egui::Rect) {
    painter.rect_filled(
        r,
        CornerRadius::same(9),
        Color32::from_rgba_unmultiplied(10, 14, 20, 190),
    );
}

const GOLD: Color32 = Color32::from_rgb(255, 214, 120);

#[allow(clippy::too_many_arguments)]
fn hud(
    mut contexts: EguiContexts,
    walk: Res<WalkState>,
    pt: Res<Playtest>,
    gameplay: Res<Gameplay>,
    terrain: Res<Terrain>,
    water: Res<crate::water::WaterBodies>,
    zones: Res<crate::world::Zones>,
    sky: Res<crate::sky::SkyState>,
    (ocean, waves, time): (
        Res<crate::water::WaterStyle>,
        Res<crate::ocean::OceanWaves>,
        Res<Time>,
    ),
) -> Result {
    if !walk.active {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?.clone();
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("motion_hud")));
    let c = screen.center();

    // mode chip (above the key hints) and the breath meter
    let (label, _) = movement::mode_label(&walk);
    let speed = if walk.mode == MoveMode::Fly || walk.water == InWater::Diving {
        walk.velocity.length()
    } else {
        walk.velocity.xz().length()
    };
    let mut chip = format!("{label}  ·  {speed:.1} m/s");
    if walk.water == InWater::Diving
        && let Some(l) = crate::motion::waves::surface_at(
            &terrain,
            &water,
            &ocean,
            &waves,
            walk.position.xz(),
            &time,
        )
    {
        chip += &format!(
            "  ·  depth {:.1} m",
            (l - walk.position.y - walk.eye_off).max(0.0)
        );
    }
    if walk.mode == MoveMode::Fly {
        chip += &format!("  ·  wheel {:.0} m/s", walk.fly_speed);
    }
    let g = painter.layout_no_wrap(chip, FontId::new(13.0, semibold()), TEXT);
    let r =
        egui::Rect::from_center_size(pos2(c.x, screen.max.y - 64.0), g.size() + vec2(26.0, 12.0));
    panel_rect(&painter, r);
    let dot = match (walk.mode, walk.water) {
        (MoveMode::Fly, _) => Color32::from_rgb(170, 140, 255),
        (MoveMode::Mount, _) => Color32::from_rgb(230, 160, 90),
        (_, InWater::Swimming | InWater::Diving) => Color32::from_rgb(90, 190, 240),
        _ => Color32::from_rgb(110, 210, 140),
    };
    painter.circle_filled(r.left_center() + vec2(9.0, 0.0), 3.5, dot);
    painter.galley(r.min + vec2(17.0, 6.0), g, TEXT);
    if walk.breath < 0.995 || walk.water == InWater::Diving {
        let w = 190.0;
        let bar = egui::Rect::from_center_size(pos2(c.x, screen.max.y - 96.0), vec2(w, 7.0));
        painter.rect_filled(
            bar.expand(3.0),
            CornerRadius::same(6),
            Color32::from_rgba_unmultiplied(10, 14, 20, 170),
        );
        let k = walk.breath.clamp(0.0, 1.0);
        let col = if k > 0.35 {
            Color32::from_rgb(120, 200, 255)
        } else {
            Color32::from_rgb(255, 120, 90)
        };
        painter.rect_filled(
            egui::Rect::from_min_size(bar.min, vec2(w * k, bar.height())),
            CornerRadius::same(4),
            col,
        );
        painter.text(
            bar.left_center() - vec2(10.0, 0.0),
            Align2::RIGHT_CENTER,
            icon::DROP,
            FontId::proportional(14.0),
            col,
        );
    }
    if !walk.playtest {
        return Ok(());
    }
    let Some(live) = &pt.live else { return Ok(()) };

    // session panel (top left)
    let zone = zones.by_id(sky.viewer_zone);
    let zone_line = zone
        .map(|z| format!("{}  {}", z.name, z.level_label()))
        .unwrap_or_else(|| "Wilderness".into());
    let found = live.found.len();
    let total = gameplay.pois.len();
    let r = egui::Rect::from_min_size(screen.min + vec2(18.0, 18.0), vec2(250.0, 108.0));
    panel_rect(&painter, r);
    let blink = (live.t * 2.0).fract() < 0.6;
    painter.circle_filled(
        r.min + vec2(18.0, 18.0),
        5.0,
        if blink {
            Color32::from_rgb(240, 70, 70)
        } else {
            Color32::from_rgb(120, 40, 40)
        },
    );
    painter.text(
        r.min + vec2(30.0, 18.0),
        Align2::LEFT_CENTER,
        "PLAYTEST",
        FontId::new(13.0, semibold()),
        Color32::WHITE,
    );
    painter.text(
        r.right_top() + vec2(-12.0, 18.0),
        Align2::RIGHT_CENTER,
        playtest::clock(live.t),
        FontId::new(17.0, semibold()),
        Color32::WHITE,
    );
    let rows = [
        (
            icon::PATH,
            format!("{:.2} km travelled", live.distance / 1000.0),
        ),
        (
            icon::FLAG,
            if total > 0 {
                format!("{found} / {total} discovered")
            } else {
                "No points of interest yet".into()
            },
        ),
        (icon::MAP_PIN, zone_line),
    ];
    for (k, (ic, text)) in rows.iter().enumerate() {
        let y = r.min.y + 44.0 + k as f32 * 21.0;
        painter.text(
            pos2(r.min.x + 14.0, y),
            Align2::LEFT_CENTER,
            *ic,
            FontId::proportional(14.0),
            TEXT_DIM,
        );
        painter.text(
            pos2(r.min.x + 36.0, y),
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(13.0),
            TEXT,
        );
    }

    // compass strip (top centre): ±70° of heading, cardinal letters, POI markers
    let heading = (-walk.yaw).to_degrees().rem_euclid(360.0);
    let cr = egui::Rect::from_center_size(pos2(c.x, screen.min.y + 34.0), vec2(560.0, 34.0));
    panel_rect(&painter, cr);
    let span = 70.0;
    let x_of = |deg: f32| -> Option<f32> {
        let rel = (deg - heading + 540.0).rem_euclid(360.0) - 180.0;
        (rel.abs() <= span).then(|| cr.center().x + rel / span * (cr.width() * 0.5 - 16.0))
    };
    for d in (0..360).step_by(15) {
        let Some(x) = x_of(d as f32) else { continue };
        let fade = 1.0 - ((x - cr.center().x).abs() / (cr.width() * 0.5)).powi(2);
        let a = (fade * 255.0) as u8;
        let name = match d {
            0 => Some("N"),
            45 => Some("NE"),
            90 => Some("E"),
            135 => Some("SE"),
            180 => Some("S"),
            225 => Some("SW"),
            270 => Some("W"),
            315 => Some("NW"),
            _ => None,
        };
        match name {
            Some(n) => {
                let col = if d == 0 {
                    Color32::from_rgba_unmultiplied(255, 120, 100, a)
                } else {
                    Color32::from_rgba_unmultiplied(235, 238, 245, a)
                };
                let size = if n.len() == 1 { 15.0 } else { 11.5 };
                painter.text(
                    pos2(x, cr.center().y),
                    Align2::CENTER_CENTER,
                    n,
                    FontId::new(size, semibold()),
                    col,
                );
            }
            None => {
                painter.line_segment(
                    [pos2(x, cr.max.y - 9.0), pos2(x, cr.max.y - 4.0)],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 205, 215, a / 2)),
                );
            }
        }
    }
    painter.line_segment(
        [
            pos2(cr.center().x, cr.min.y + 2.0),
            pos2(cr.center().x, cr.min.y + 7.0),
        ],
        Stroke::new(2.0, Color32::WHITE),
    );
    let me = walk.position.xz();
    for poi in &gameplay.pois {
        let d = Vec2::new(poi.pos[0], poi.pos[2]) - me;
        let bearing = d.x.atan2(-d.y).to_degrees();
        let Some(x) = x_of(bearing) else { continue };
        let done = live.found.iter().any(|f| f.poi == poi.id);
        let col = if done {
            GOLD
        } else {
            Color32::from_rgb(140, 200, 255)
        };
        let p = pos2(x, cr.max.y + 7.0);
        painter.add(egui::Shape::convex_polygon(
            vec![
                p + vec2(0.0, -5.0),
                p + vec2(5.0, 0.0),
                p + vec2(0.0, 5.0),
                p + vec2(-5.0, 0.0),
            ],
            col,
            Stroke::NONE,
        ));
        // the name of what's straight ahead
        if (x - cr.center().x).abs() < 40.0 {
            let dist = d.length();
            let txt = if dist > 950.0 {
                format!("{}  {:.1} km", poi.name, dist / 1000.0)
            } else {
                format!("{}  {:.0} m", poi.name, dist)
            };
            painter.text(
                p + vec2(0.0, 9.0),
                Align2::CENTER_TOP,
                txt,
                FontId::proportional(12.5),
                col,
            );
        }
    }

    // discovery toast
    if let Some((name, sub, age)) = &live.toast {
        let a = (1.0 - ((*age - 3.4) / 1.0).clamp(0.0, 1.0)) * (*age / 0.35).min(1.0);
        if a > 0.0 {
            let y = screen.min.y + screen.height() * 0.32;
            let al = (255.0 * a) as u8;
            painter.text(
                pos2(c.x, y - 26.0),
                Align2::CENTER_CENTER,
                icon::SEAL_CHECK,
                FontId::proportional(26.0),
                Color32::from_rgba_unmultiplied(255, 214, 120, al),
            );
            painter.text(
                pos2(c.x, y + 6.0),
                Align2::CENTER_CENTER,
                name,
                FontId::new(30.0, semibold()),
                Color32::from_rgba_unmultiplied(255, 236, 200, al),
            );
            painter.text(
                pos2(c.x, y + 36.0),
                Align2::CENTER_CENTER,
                sub.to_uppercase(),
                FontId::proportional(13.0),
                Color32::from_rgba_unmultiplied(230, 230, 240, (200.0 * a) as u8),
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn editor_windows(
    // native file dialogs (capture folder, playtest save) must open on the main thread on macOS
    _main: bevy::ecs::system::NonSendMarker,
    mut contexts: EguiContexts,
    mut mui: ResMut<MotionUi>,
    walk: Res<WalkState>,
    mut data: ResMut<MotionData>,
    mut history: ResMut<History>,
    mut pt: ResMut<Playtest>,
    mut fly: ResMut<Flythrough>,
    mut orbit: ResMut<OrbitCamera>,
    gameplay: Res<Gameplay>,
    terrain: Res<Terrain>,
    cam: Query<(&GlobalTransform, &Projection), With<MainCamera>>,
    mut actions: MessageWriter<MotionAction>,
    mut meta: ResMut<crate::world::WorldMeta>,
) -> Result {
    finish_exports();
    if let Some(d) = crate::dialogs::take_file("capture_folder") {
        mui.capture.dir = d;
    }
    if walk.active || walk.cinematic || (!mui.open && pt.summary.is_none()) {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?.clone();
    if let Some(s) = pt.summary.clone() {
        let mut open = true;
        egui::Window::new(RichText::new(format!("{}  Playtest summary", icon::FLAG_CHECKERED)).font(FontId::new(16.0, semibold())))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .fixed_size(vec2(380.0, 0.0))
            .open(&mut open)
            .show(&ctx, |ui| {
                let speed = s.distance / s.duration.max(1.0);
                egui::Grid::new("pt_summary").num_columns(2).spacing(vec2(18.0, 6.0)).show(ui, |ui| {
                    for (k, v) in [
                        ("Time", playtest::clock(s.duration)),
                        ("Distance", format!("{:.2} km  ({speed:.1} m/s average)", s.distance / 1000.0)),
                        ("Discoveries", format!("{} of {}", s.discoveries.len(), s.poi_count)),
                        ("Longest gap", format!("{} without a discovery", playtest::clock(s.longest_gap()))),
                    ] {
                        ui.label(RichText::new(k).color(TEXT_DIM));
                        ui.label(RichText::new(v).color(TEXT));
                        ui.end_row();
                    }
                });
                if !s.discoveries.is_empty() {
                    separator(ui);
                    for d in &s.discoveries {
                        ui.label(RichText::new(format!("{}  {}  {}", playtest::clock(d.t), icon::SEAL_CHECK, d.name)).color(TEXT));
                    }
                }
                if s.longest_gap() > 180.0 {
                    ui.add_space(4.0);
                    dim(ui, "Over three minutes passed with nothing to find: consider a point of interest along that stretch (the route is drawn on the map).");
                }
                separator(ui);
                ui.horizontal(|ui| {
                    if ui.button(format!("{} Show route", icon::PATH)).clicked() {
                        pt.trails = vec![s.id];
                        mui.open = true;
                        mui.tab = PanelTab::Playtests;
                        pt.summary = None;
                    }
                    if ui.button(format!("{} Export JSON", icon::EXPORT)).clicked() {
                        export_session(&s);
                    }
                });
            });
        if !open {
            pt.summary = None;
        }
    }
    // (out of the way while places are picked: the bar at the top says what to do)
    if !mui.open || mui.picking.is_some() {
        return Ok(());
    }
    let before = data.clone();
    let mut open = true;
    let cam_view = cam.single().ok().map(|(t, p)| {
        (
            *t,
            if let Projection::Perspective(p) = p {
                p.fov.to_degrees()
            } else {
                50.0
            },
        )
    });
    egui::Window::new(
        RichText::new(format!("{}  Paths & Playtests", icon::FILM_SLATE))
            .font(FontId::new(16.0, semibold())),
    )
    .id(Id::new("motion_window"))
    .collapsible(true)
    .resizable(true)
    // (beside the editor's right-hand panel, not over it)
    .default_pos(pos2(
        ctx.content_rect().max.x - crate::ui::RIGHT_W - 420.0,
        70.0,
    ))
    .default_width(380.0)
    .open(&mut open)
    .show(&ctx, |ui| {
        let mut tab = mui.tab;
        segmented(
            ui,
            &mut tab,
            &[
                (PanelTab::Paths, "Camera paths"),
                (PanelTab::Playtests, "Playtests"),
            ],
        );
        mui.tab = tab;
        ui.add_space(6.0);
        match mui.tab {
            PanelTab::Paths => paths_ui(
                ui,
                &mut mui,
                &mut data,
                &mut history,
                &mut fly,
                &mut orbit,
                &terrain,
                &gameplay,
                cam_view,
            ),
            PanelTab::Playtests => sessions_ui(
                ui,
                &mut pt,
                &mut data,
                &mut history,
                &gameplay,
                &mut actions,
            ),
        }
    });
    if !open {
        mui.open = false;
    }
    if !ctx.egui_is_using_pointer()
        && !ctx.egui_wants_keyboard_input()
        && let Some((label, before)) = mui.dragging.take()
    {
        super::commit(&data, &mut history, &label, before);
    }
    if *data != before {
        meta.dirty = true;
    }
    Ok(())
}

/// The playtest JSON waiting for its Save dialog's answer.
static PLAYTEST_JSON: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn export_session(s: &playtest::Session) {
    *PLAYTEST_JSON.lock().unwrap_or_else(|p| p.into_inner()) =
        Some(serde_json::to_string_pretty(&s.export_json()).unwrap_or_default());
    crate::dialogs::save_file(
        "playtest_json",
        rfd::AsyncFileDialog::new()
            .set_file_name(format!("playtest_{}.json", s.id))
            .add_filter("JSON", &["json"]),
    );
}

/// Writes a playtest export once its dialog answered.
fn finish_exports() {
    if let Some(path) = crate::dialogs::take_file("playtest_json")
        && let Some(json) = PLAYTEST_JSON
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    {
        let _ = std::fs::write(path, json);
    }
}

const TIP: &str = "flights";

#[allow(clippy::too_many_arguments)]
fn paths_ui(
    ui: &mut egui::Ui,
    mui: &mut MotionUi,
    data: &mut MotionData,
    history: &mut History,
    fly: &mut Flythrough,
    orbit: &mut OrbitCamera,
    terrain: &Terrain,
    gameplay: &Gameplay,
    cam: Option<(GlobalTransform, f32)>,
) {
    ui.horizontal(|ui| {
        dim(
            ui,
            "Camera flights over the world, for trailers and screenshots.",
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            tips::help_button(ui, TIP)
        });
    });
    tips::card(
        ui,
        TIP,
        "Make a flight in three steps",
        &[
            "Press Pick places, then click spots on the ground in the order to fly over them. One spot makes a circle around it.",
            "Press Enter (or Make the flight). The camera route is made for you, clear of the hills.",
            "Choose a style and speed, then Preview it or Capture a video.",
        ],
    );
    if mui.picking.is_some() {
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(255, 196, 64, 22))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!(
                        "{}  Picking places: click on the ground",
                        icon::MAP_PIN_PLUS
                    ))
                    .color(TEXT),
                );
                dim(
                    ui,
                    "Enter makes the flight, Backspace removes the last place, Esc cancels.",
                );
            });
    } else {
        if primary_button(
            ui,
            &format!("{}  Pick places to fly over", icon::MAP_PIN_PLUS),
        )
        .on_hover_text("Click spots on the ground; the camera flight is made through them")
        .clicked()
        {
            mui.picking = Some(super::pick::Picking::new(None, vec![]));
        }
        if !gameplay.pois.is_empty() {
            ui.add_space(2.0);
            if wide_button(ui, &format!("{}  Tour the points of interest ({})", icon::BINOCULARS, gameplay.pois.len()))
                .on_hover_text("A flight through every point of interest (Gameplay tab), nearest first from the player start")
                .clicked()
            {
                let places = super::pick::poi_tour(gameplay);
                if let Some(id) = super::pick::make_flight(data, history, terrain, &places, None, None, Some("Tour of the places".into()), None) {
                    mui.path = Some(id);
                    mui.key = None;
                    mui.rename.clear();
                }
            }
        }
    }
    if !data.paths.is_empty() {
        section(ui, "Your flights");
    }
    for p in &data.paths {
        let sel = mui.path == Some(p.id);
        let what = match &p.route {
            Some(r) if r.places.len() == 1 => "circle".to_string(),
            Some(r) => format!("{} places", r.places.len()),
            None => format!("{} keys", p.keys.len()),
        };
        let ic = if p.route.is_some() {
            icon::AIRPLANE_TILT
        } else {
            icon::FILM_STRIP
        };
        if ui
            .selectable_label(
                sel,
                format!(
                    "{ic}  {}   ·  {what}  ·  {}",
                    p.name,
                    playtest::clock(p.duration())
                ),
            )
            .clicked()
        {
            mui.path = Some(p.id);
            mui.key = None;
            mui.rename = p.name.clone();
        }
    }
    let Some(pid) = mui.path.filter(|id| data.paths.iter().any(|p| p.id == *id)) else {
        hand_made(ui, mui, data, history, cam);
        return;
    };
    separator(ui);
    let path = data.paths.iter().find(|p| p.id == pid).cloned().unwrap();
    if mui.rename.is_empty() {
        mui.rename = path.name.clone();
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut mui.rename).desired_width(200.0));
        if r.lost_focus() && mui.rename.trim() != path.name && !mui.rename.trim().is_empty() {
            let name = mui.rename.trim().to_string();
            edit(data, history, "Rename camera path", |d| {
                d.paths
                    .iter_mut()
                    .find(|p| p.id == pid)
                    .map(|p| p.name = name)
                    .is_some()
            });
        }
        if ui
            .button(RichText::new(icon::TRASH).color(crate::ui::theme::DANGER))
            .on_hover_text("Delete this flight")
            .clicked()
        {
            edit(data, history, "Delete camera path", |d| {
                d.paths.retain(|p| p.id != pid);
                true
            });
            mui.path = None;
            mui.rename.clear();
        }
    });
    if path.keys.is_empty() && path.route.is_none() {
        dim(ui, "No camera keys yet: open Fine-tune camera keys below.");
    }
    if let Some(route) = &path.route {
        let (mut style, mut speed) = (route.style, route.speed);
        section(ui, "Style");
        segmented(ui, &mut style, &FlightStyle::ALL.map(|s| (s, s.name())));
        dim(ui, style.hint());
        section(ui, "Speed");
        segmented(ui, &mut speed, &FlightSpeed::ALL.map(|s| (s, s.name())));
        if (style, speed) != (route.style, route.speed) {
            super::pick::restyle(data, history, terrain, pid, style, speed);
        }
        ui.add_space(2.0);
        let places = route.places.len();
        ui.horizontal(|ui| {
            dim(ui, &if places == 1 { "Circles around 1 place.".to_string() } else { format!("Flies through {places} places.") });
            if mui.picking.is_none() && ui.small_button(format!("{} Change places", icon::MAP_PIN)).on_hover_text("Pick the places again (they start as they are: click to add, Backspace removes the last)").clicked() {
                mui.picking = Some(super::pick::Picking::new(Some(pid), route.places.iter().map(|p| Vec2::from_array(*p)).collect()));
            }
        });
    }
    // the path as it is now (style changes above re-made it)
    let path = data.paths.iter().find(|p| p.id == pid).cloned().unwrap();
    let can_play = path.keys.len() >= 2;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                can_play,
                egui::Button::new(
                    RichText::new(format!(
                        "{}  Preview  ({})",
                        icon::PLAY,
                        playtest::clock(path.duration())
                    ))
                    .color(ON_ACCENT),
                )
                .fill(ACCENT)
                .min_size(vec2(0.0, 30.0)),
            )
            .on_hover_text("Plays full screen; Esc stops")
            .clicked()
        {
            fly.play(pid, None);
        }
        if ui
            .add_enabled(
                can_play,
                egui::Button::new(
                    RichText::new(format!("{}  Capture video", icon::RECORD)).color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(190, 60, 60))
                .min_size(vec2(0.0, 30.0)),
            )
            .on_hover_text(format!("Renders an MP4 into {}", mui.capture.dir.display()))
            .clicked()
        {
            fly.play(pid, Some(mui.capture.clone()));
        }
    });
    if let Some(st) = &fly.last {
        match (&st.output, &st.error) {
            (Some(o), _) => dim(ui, &format!("Last capture: {}", o.display())),
            (_, Some(e)) => dim(ui, &format!("Last capture failed: {e}")),
            _ => {}
        }
    }
    let mut change: Option<(String, Box<dyn FnOnce(&mut flythrough::CameraPath)>)> = None;
    // drags change the path live and become one undo step when released
    let mut live: Option<(String, Box<dyn FnOnce(&mut flythrough::CameraPath)>)> = None;
    ui.add_space(4.0);
    egui::CollapsingHeader::new(
        RichText::new(format!("{}  Playback & video", icon::SLIDERS)).color(TEXT),
    )
    .id_salt("flight_video")
    .show(ui, |ui| {
        let mut hours = path.timelapse_hours;
        ui.horizontal(|ui| {
            ui.label("Time-lapse");
            if ui
                .add(
                    TrackSlider::new(&mut hours, 0.0..=24.0)
                        .step_by(0.5)
                        .suffix(" h"),
                )
                .on_hover_text("Hours the day advances over the flight (0 = time stands still)")
                .changed()
            {
                live = Some((
                    "Time-lapse".into(),
                    Box::new(move |p| p.timelapse_hours = hours),
                ));
            }
        });
        let mut looped = path.looped;
        if toggle(ui, &mut looped, "Loop the preview").changed() {
            change = Some(("Loop path".into(), Box::new(move |p| p.looped = looped)));
        }
        capture_settings(ui, mui);
    });
    egui::CollapsingHeader::new(RichText::new(format!("{}  Fine-tune camera keys", icon::FILM_STRIP)).color(TEXT)).id_salt(("flight_keys", path.route.is_none())).default_open(path.route.is_none()).show(ui, |ui| {
        if path.route.is_some() {
            dim(ui, "Changing keys by hand turns this into a hand-made path: Style and Speed stop applying.");
        }
        section(ui, "Keys");
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            let n = path.keys.len();
            for (i, k) in path.keys.iter().enumerate() {
                ui.horizontal(|ui| {
                    let sel = mui.key == Some(i);
                    if ui.selectable_label(sel, format!("#{}", i + 1)).on_hover_text(format!("({:.0}, {:.0}, {:.0})", k.pos[0], k.pos[1], k.pos[2])).clicked() {
                        mui.key = Some(i);
                    }
                    if i + 1 < n {
                        let mut dur = k.dur;
                        if ui.add(egui::DragValue::new(&mut dur).range(0.2..=120.0).speed(0.1).suffix(" s")).on_hover_text("Seconds to the next key").changed() {
                            live = Some(("Key timing".into(), Box::new(move |p| p.keys[i].dur = dur)));
                        }
                        let mut ease = k.ease;
                        egui::ComboBox::from_id_salt(("ease", pid, i)).width(92.0).selected_text(ease.name()).dropdown_ui(ui, |ui| {
                            for e in Ease::ALL {
                                menu_option(ui, &mut ease, e, e.name());
                            }
                        });
                        if ease != k.ease {
                            change = Some(("Key easing".into(), Box::new(move |p| p.keys[i].ease = ease)));
                        }
                    } else {
                        ui.label(RichText::new("end").color(TEXT_DIM));
                    }
                    if ui.small_button(icon::EYE).on_hover_text("Look from this key").clicked() {
                        look_from(orbit, terrain, k);
                        mui.key = Some(i);
                    }
                    if i > 0 && ui.small_button(icon::ARROW_UP).on_hover_text("Move up").clicked() {
                        change = Some(("Reorder keys".into(), Box::new(move |p| p.keys.swap(i, i - 1))));
                    }
                    if i + 1 < n && ui.small_button(icon::ARROW_DOWN).on_hover_text("Move down").clicked() {
                        change = Some(("Reorder keys".into(), Box::new(move |p| p.keys.swap(i, i + 1))));
                    }
                    if ui.small_button(icon::X).on_hover_text("Delete key").clicked() {
                        change = Some(("Delete key".into(), Box::new(move |p| {
                            p.keys.remove(i);
                        })));
                    }
                });
            }
        });
        ui.horizontal(|ui| {
            if let Some((t, fov)) = cam {
                if ui.button(format!("{} Add key here", icon::CAMERA_PLUS)).on_hover_text("Add the current view after the selected key").clicked() {
                    let at = mui.key.map(|k| k + 1).unwrap_or(path.keys.len()).min(path.keys.len());
                    let mut key = flythrough::key_from_view(&t, fov, 4.0);
                    if let Some(prev) = at.checked_sub(1).and_then(|i| path.keys.get(i)) {
                        key.dur = flythrough::auto_duration(prev, key.pos);
                    }
                    key.dur = key.dur.max(0.2);
                    change = Some(("Add camera key".into(), Box::new(move |p| {
                        if at > 0 && at <= p.keys.len() {
                            p.keys[at - 1].dur = key.dur;
                        }
                        p.keys.insert(at.min(p.keys.len()), key);
                    })));
                    mui.key = Some(at);
                }
                if let Some(i) = mui.key.filter(|i| *i < path.keys.len())
                    && ui.button(format!("{} Update #{}", icon::ARROWS_CLOCKWISE, i + 1)).on_hover_text("Replace the selected key with the current view").clicked()
                {
                    let mut key = flythrough::key_from_view(&t, fov, path.keys[i].dur);
                    key.ease = path.keys[i].ease;
                    change = Some(("Update camera key".into(), Box::new(move |p| p.keys[i] = key)));
                }
            }
        });
    });
    hand_made(ui, mui, data, history, cam);
    if let Some((label, f)) = live {
        if mui.dragging.is_none() {
            mui.dragging = Some((label, data.clone()));
        }
        if let Some(p) = data.paths.iter_mut().find(|p| p.id == pid) {
            let keys = p.keys.clone();
            f(p);
            if p.keys != keys {
                p.route = None;
            }
        }
    }
    if let Some((label, f)) = change {
        edit(data, history, &label, |d| {
            match d.paths.iter_mut().find(|p| p.id == pid) {
                Some(p) => {
                    let keys = p.keys.clone();
                    f(p);
                    if p.keys != keys {
                        p.route = None;
                    }
                    true
                }
                None => false,
            }
        });
    }
}

/// The small "build one by hand" button: an empty path starting at the current view.
fn hand_made(
    ui: &mut egui::Ui,
    mui: &mut MotionUi,
    data: &mut MotionData,
    history: &mut History,
    cam: Option<(GlobalTransform, f32)>,
) {
    ui.add_space(6.0);
    if ui
        .add(
            egui::Button::new(
                RichText::new(format!("{} Hand-made path from camera views", icon::PLUS))
                    .small()
                    .color(TEXT_DIM),
            )
            .frame(false),
        )
        .on_hover_text("For full control: fly the camera to a view, add it as a key, repeat")
        .clicked()
    {
        let mut new_id = 0;
        edit(data, history, "New camera path", |d| {
            let id = d.alloc_id();
            let n = d.paths.len() + 1;
            let mut keys = vec![];
            if let Some((t, fov)) = cam {
                keys.push(flythrough::key_from_view(&t, fov, 4.0));
            }
            d.paths.push(flythrough::CameraPath {
                id,
                name: format!("Path {n}"),
                keys,
                timelapse_hours: 0.0,
                looped: false,
                route: None,
            });
            new_id = id;
            true
        });
        mui.path = Some(new_id);
        mui.key = Some(0);
        mui.rename.clear();
    }
}

fn capture_settings(ui: &mut egui::Ui, mui: &mut MotionUi) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("cap_res")
            .width(110.0)
            .selected_text(format!("{}×{}", mui.capture.width, mui.capture.height))
            .dropdown_ui(ui, |ui| {
                for (w, h) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)] {
                    if menu_check(
                        ui,
                        mui.capture.width == w && mui.capture.height == h,
                        format!("{w}×{h}"),
                    )
                    .clicked()
                    {
                        mui.capture.width = w;
                        mui.capture.height = h;
                    }
                }
            });
        egui::ComboBox::from_id_salt("cap_fps")
            .width(70.0)
            .selected_text(format!("{} fps", mui.capture.fps))
            .dropdown_ui(ui, |ui| {
                for f in [24, 30, 60] {
                    menu_option(ui, &mut mui.capture.fps, f, format!("{f} fps"));
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(mui.capture.dir.display().to_string())
                .small()
                .color(TEXT_DIM),
        );
        if ui
            .small_button(format!("{} Folder…", icon::FOLDER_OPEN))
            .clicked()
        {
            crate::dialogs::pick_folder(
                "capture_folder",
                rfd::AsyncFileDialog::new().set_directory(&mui.capture.dir),
            );
        }
    });
    toggle(ui, &mut mui.capture.keep_frames, "Also keep the PNG frames");
    match flythrough::ffmpeg_path() {
        Some(_) => dim(ui, "Encodes an H.264 MP4 with ffmpeg."),
        None => dim(
            ui,
            "ffmpeg isn't installed: frames are saved as PNG only (brew install ffmpeg for MP4).",
        ),
    }
}

/// Put the editor camera at a key's view (as near as an orbit camera can).
fn look_from(orbit: &mut OrbitCamera, terrain: &Terrain, k: &flythrough::CameraKey) {
    let pos = Vec3::from_array(k.pos);
    let fwd = Quat::from_euler(EulerRot::YXZ, k.yaw, k.pitch, 0.0) * Vec3::NEG_Z;
    let hit = terrain
        .raycast(pos, fwd, 4000.0)
        .unwrap_or(pos + fwd * 80.0);
    let dist = hit.distance(pos).max(5.0);
    orbit.set_view(hit, k.yaw, (-k.pitch).clamp(0.03, 1.5), dist);
}

fn sessions_ui(
    ui: &mut egui::Ui,
    pt: &mut Playtest,
    data: &mut MotionData,
    history: &mut History,
    gameplay: &Gameplay,
    actions: &mut MessageWriter<MotionAction>,
) {
    dim(
        ui,
        "Play the world as a player: start at the player start with a compass, minimap and discoveries. Every session's route is kept (the last ten).",
    );
    let start = match &gameplay.player_start {
        Some(_) => "Starts at the player start.".to_string(),
        None => "No player start set (Gameplay tab): starts where the camera looks.".to_string(),
    };
    if primary_button(ui, &format!("{}  Start playtest", icon::PLAY)).clicked() {
        actions.write(MotionAction::Playtest);
    }
    dim(ui, &start);
    separator(ui);
    if data.sessions.is_empty() {
        dim(ui, "No sessions yet.");
        return;
    }
    toggle(ui, &mut pt.heat, "Heat map of the shown routes");
    let mut delete = None;
    for s in data.sessions.iter().rev() {
        ui.horizontal(|ui| {
            let mut shown = pt.trails.contains(&s.id);
            if toggle(ui, &mut shown, "")
                .on_hover_text("Show the route")
                .changed()
            {
                if shown {
                    pt.trails.push(s.id);
                } else {
                    pt.trails.retain(|x| *x != s.id);
                }
            }
            ui.label(RichText::new(format!("#{}  {}", s.id, s.label())).color(TEXT));
            if ui
                .small_button(icon::INFO)
                .on_hover_text("Summary")
                .clicked()
            {
                pt.summary = Some(s.clone());
            }
            if ui
                .small_button(icon::EXPORT)
                .on_hover_text("Export JSON")
                .clicked()
            {
                export_session(s);
            }
            if ui.small_button(icon::X).on_hover_text("Delete").clicked() {
                delete = Some(s.id);
            }
        });
    }
    if let Some(id) = delete {
        edit(data, history, "Delete playtest session", |d| {
            d.sessions.retain(|s| s.id != id);
            true
        });
        pt.trails.retain(|x| *x != id);
    }
}
