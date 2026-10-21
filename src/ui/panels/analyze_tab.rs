//! Analyze tab: six views of the world as players will experience it (walkability, travel time,
//! content gaps, level flow, reachability, the World Doctor). The left panel picks the view and
//! its rules; the right panel shows the legend, the numbers and the lists with Go to / Fix.
//! The computation lives in `crate::analysis`.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, Sense, Stroke, pos2, vec2};

use super::super::UiState;
use super::super::theme::*;
use super::super::widgets::*;
use crate::analysis::doctor::{self, Check, Fix, Severity};
use crate::analysis::paint::{self, Rgba};
use crate::analysis::travel::fmt_time;
use crate::analysis::{SteepRule, TravelSource, View, flow};

fn view_icon(v: View) -> &'static str {
    match v {
        View::Walkability => "Footsteps",
        View::Travel => "Stopwatch",
        View::Gaps => "Target",
        View::Flow => "ChartWaterfall",
        View::Reach => "Earth",
        View::Doctor => "Stethoscope",
    }
}

fn view_blurb(v: View) -> &'static str {
    match v {
        View::Walkability => {
            "Where players can walk, wade, swim, and where slopes, cliffs and objects stop them."
        }
        View::Travel => {
            "How long it takes to get everywhere on foot from one place. Lines every 30 seconds."
        }
        View::Gaps => {
            "How far every spot is from something to do. Red ground is a dead zone: a long walk with nothing in it."
        }
        View::Flow => {
            "Zones that touch across walkable ground, and where the level jump between them is too big."
        }
        View::Reach => {
            "Everything players can reach on foot from the start, pockets they can't, and pits they can't leave."
        }
        View::Doctor => {
            "Finds common world-building mistakes and fixes the safe ones for you (undoable)."
        }
    }
}

fn col(c: Rgba) -> Color32 {
    if c[3] == 0 {
        Color32::TRANSPARENT
    } else {
        Color32::from_rgb(c[0], c[1], c[2])
    }
}

fn degrees(v: f32) -> String {
    format!("{v:.0}°")
}

fn stat(ui: &mut egui::Ui, label: &str, value: String) {
    stat_row(ui, label, &value);
}

/// A legend row; a fully transparent colour draws an empty box ("no tint").
fn swatch(ui: &mut egui::Ui, c: Color32, label: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
        let b = egui::Rect::from_center_size(r.center(), vec2(10.0, 10.0));
        ui.painter().rect_filled(b, CornerRadius::same(3), c);
        ui.painter().rect_stroke(
            b,
            CornerRadius::same(3),
            Stroke::new(1.0, Color32::from_white_alpha(40)),
            egui::StrokeKind::Inside,
        );
        ui.label(RichText::new(label).size(12.0).color(TEXT_DIM));
    });
}

/// An icon + text line in a colour.
fn icon_line(ui: &mut egui::Ui, ic: &str, c: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        crate::ui::icons::icon(ui, ic, 15.0, c);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(c)).wrap());
    });
}

/// What a list row asked for.
#[derive(Default)]
struct RowOut {
    hovered: bool,
    goto: bool,
    fix: bool,
}

/// A list item: coloured dot, title and subtitle, Go to and (optionally) Fix buttons; hovering it
/// highlights the spot on the map.
fn row(ui: &mut egui::Ui, dot: Color32, title: &str, sub: &str, fix: Option<&str>) -> RowOut {
    let mut out = RowOut::default();
    let bg = ui.painter().add(egui::Shape::Noop);
    let resp = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(7, 5))
        .corner_radius(CornerRadius::same(R_INPUT))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
                ui.painter().rect_filled(
                    egui::Rect::from_center_size(r.center(), vec2(10.0, 10.0)),
                    CornerRadius::same(3),
                    dot,
                );
                let bw = if fix.is_some() { 76.0 } else { 34.0 };
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.set_width((ui.available_width() - bw).max(60.0));
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .font(FontId::new(13.0, medium()))
                                .color(TEXT),
                        )
                        .wrap(),
                    );
                    if !sub.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(sub).size(11.5).color(TEXT_MUTE)).wrap(),
                        );
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if let Some(label) = fix
                        && small_button(ui, "Fix").on_hover_text(label).clicked()
                    {
                        out.fix = true;
                    }
                    if icon_button(ui, "Target", "Go to").clicked() {
                        out.goto = true;
                    }
                });
            });
        });
    out.hovered = resp.response.hovered() || ui.rect_contains_pointer(resp.response.rect);
    if out.hovered {
        ui.painter().set(
            bg,
            egui::epaint::RectShape::filled(resp.response.rect, CornerRadius::same(R_INPUT), SOFT),
        );
    }
    out
}

fn goto(s: &mut UiState, p: Vec3, dist: f32) {
    let yaw = s.orbit.yaw;
    s.orbit.set_view(p, yaw, 0.95, dist);
}

fn no_start_note(ui: &mut egui::Ui, s: &mut UiState) {
    if s.gameplay.player_start.is_some() {
        return;
    }
    hint_small(
        ui,
        "This world has no player start yet (Gameplay tab). Measuring from a chosen point: click the ground in the Reachability view to move it.",
    );
    if wide_icon_button(ui, "Target", "Use the camera focus").clicked() {
        s.analysis.fallback_origin = Some(s.orbit.focus.xz());
    }
}

/// One of the six views as a list row: icon and name; the picked one also shows what it does.
fn view_row(ui: &mut egui::Ui, v: View, selected: bool) -> egui::Response {
    let w = ui.available_width();
    let p = ui.painter();
    let desc = selected.then(|| {
        p.layout(
            view_blurb(v).to_string(),
            FontId::proportional(11.5),
            TEXT_MUTE,
            w - 44.0,
        )
    });
    let h = 34.0 + desc.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, CornerRadius::same(R_TILE), SOFT_HOVER);
        } else if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(R_TILE), SOFT);
        }
        let top = rect.top() + 17.0;
        paint_icon(
            ui,
            pos2(rect.left() + 18.0, top),
            view_icon(v),
            15.0,
            if selected || resp.hovered() {
                TEXT
            } else {
                TEXT_DIM
            },
        );
        let p = ui.painter();
        let g = p.layout_no_wrap(v.name().to_string(), FontId::new(13.0, medium()), TEXT);
        p.galley(pos2(rect.left() + 36.0, top - g.size().y * 0.5), g, TEXT);
        if let Some(d) = desc {
            p.galley(pos2(rect.left() + 36.0, top + 9.0), d, TEXT_MUTE);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ left panel

pub fn analyze_left(ui: &mut egui::Ui, s: &mut UiState) {
    dim(ui, "See your world the way players move through it.");
    if let Some((p, stage)) = s.analysis.progress() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            ui.add(egui::Spinner::new().size(13.0).color(TEXT_DIM));
            ui.label(
                RichText::new(format!("{stage}… {:.0}%", p * 100.0))
                    .size(12.0)
                    .color(TEXT_DIM),
            );
        });
    }
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for v in View::ALL {
            let sel = s.analysis.view == v;
            if view_row(ui, v, sel).clicked() && !sel {
                s.analysis.view = v;
                if v == View::Doctor && s.analysis.report().is_none() {
                    s.analysis.want_doctor = true;
                }
            }
        }
    });
    let before = s.analysis.settings.clone();
    section_fold(ui, "analyze_rules", "Rules", "", |ui| {
        match s.analysis.view {
            View::Walkability => walk_rules(ui, s),
            View::Travel => travel_rules(ui, s),
            View::Gaps => gap_rules(ui, s),
            View::Flow => flow_rules(ui, s),
            View::Reach => reach_rules(ui, s),
            View::Doctor => doctor_rules(ui, s),
        }
    });
    if s.analysis.settings != before {
        s.analysis.settings_changed = true;
    }
}

fn walk_rules(ui: &mut egui::Ui, s: &mut UiState) {
    let st = &mut s.analysis.settings;
    slider_row(
        ui,
        "Steepest walkable slope",
        &mut st.max_walk_slope,
        20.0..=60.0,
        degrees,
    )
    .on_hover_text("Players walk up slopes up to this angle.");
    let lo = st.max_walk_slope + 2.0;
    st.cliff_slope = st.cliff_slope.max(lo);
    slider_row(ui, "Cliffs from", &mut st.cliff_slope, lo..=85.0, degrees)
        .on_hover_text("Steeper than this nobody passes, up or down.");
    let steep = match st.steep {
        SteepRule::SlideDown => "Slide down",
        SteepRule::Climb => "Climb slowly",
        SteepRule::Block => "Can't pass",
    };
    dropdown_row(ui, "Between the two", "analysis_steep", steep, |ui| {
        menu_option(ui, &mut st.steep, SteepRule::SlideDown, "Slide down");
        menu_option(ui, &mut st.steep, SteepRule::Climb, "Climb slowly");
        menu_option(ui, &mut st.steep, SteepRule::Block, "Can't pass");
    })
    .on_hover_text("What players do on slopes between the walk limit and the cliffs");
    slider_row(
        ui,
        "Swim in water deeper than",
        &mut st.swim_depth,
        0.5..=3.0,
        |v| format!("{v:.1} m"),
    );
    toggle(ui, &mut st.allow_swim, "Players can swim")
        .on_hover_text("Off: deep water blocks like a wall.");
    toggle(
        ui,
        &mut st.objects_block,
        "Trees, rocks and buildings block",
    )
    .on_hover_text("Tree trunks, boulders, huts, tents and walls; plants and pebbles don't.");
    ui.add_space(2.0);
    let ready =
        s.analysis.grid().is_some() && s.analysis.running.is_none() && s.analysis.export.is_none();
    let label = if s.analysis.export.is_some() {
        "Writing navmesh…"
    } else {
        "Export navmesh…"
    };
    if ui
        .add_enabled_ui(ready, |ui| wide_icon_button(ui, "ArrowOutOfBox", label))
        .inner
        .on_disabled_hover_text("Analysing…")
        .clicked()
    {
        let default = crate::export::exports_dir();
        let _ = std::fs::create_dir_all(&default);
        crate::dialogs::pick_folder(
            "navmesh_export",
            rfd::AsyncFileDialog::new()
                .set_title("Export navmesh to folder")
                .set_directory(&default),
        );
    }
    hint_small(
        ui,
        "Navmesh for your server: walkable polygons with areas (walk, road, swim), costs and neighbours, as OBJ + JSON, plus a 16-bit walkable heightfield PNG.",
    );
    // (the dialog answers on a later frame)
    if let Some(dir) = crate::dialogs::take_file("navmesh_export")
        && let Some(g) = s.analysis.parts.grid.as_ref().map(|g| g.1.clone())
    {
        // build and write on a worker thread; the toast comes when it's done
        let (speed, d) = (s.analysis.settings.run_speed, dir.clone());
        if let Ok(h) = std::thread::Builder::new()
            .name("navmesh".into())
            .spawn(move || crate::analysis::navmesh::export(&g, speed, &d))
        {
            s.analysis.export = Some((dir, h));
        }
    }
}

fn travel_rules(ui: &mut egui::Ui, s: &mut UiState) {
    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        Start,
        Poi,
        Click,
    }
    let mode = match s.analysis.settings.source {
        TravelSource::PlayerStart => Mode::Start,
        TravelSource::Poi(_) => Mode::Poi,
        TravelSource::Point(_) => Mode::Click,
    };
    let label = match mode {
        Mode::Start => "Player start",
        Mode::Poi => "A POI",
        Mode::Click => "Click map",
    };
    let mut pick = mode;
    dropdown_row(ui, "Measure from", "analysis_from", label, |ui| {
        menu_option(ui, &mut pick, Mode::Start, "Player start");
        menu_option(ui, &mut pick, Mode::Poi, "A POI");
        menu_option(ui, &mut pick, Mode::Click, "Click map");
    });
    if pick != mode {
        s.analysis.settings.source = match pick {
            Mode::Start => TravelSource::PlayerStart,
            Mode::Poi => {
                let sel = match s.gameplay.selected {
                    Some(crate::gameplay::GameplaySel::Poi(id)) => Some(id),
                    _ => s.gameplay.pois.first().map(|p| p.id),
                };
                sel.map(TravelSource::Poi)
                    .unwrap_or(TravelSource::PlayerStart)
            }
            Mode::Click => TravelSource::Point(s.orbit.focus.xz().to_array()),
        };
        if pick == Mode::Poi && s.gameplay.pois.is_empty() {
            s.ed.toast("No POIs yet: add them in the Gameplay tab");
        }
    }
    match s.analysis.settings.source {
        TravelSource::Poi(id) => {
            let name = s
                .gameplay
                .pois
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let mut pick = id;
            egui::ComboBox::from_id_salt("analysis_poi")
                .width(ui.available_width())
                .height(320.0)
                .selected_text(if name.is_empty() {
                    "(unnamed)".to_string()
                } else {
                    name
                })
                .dropdown_ui(ui, |ui| {
                    for p in &s.gameplay.pois {
                        let label = if p.name.is_empty() {
                            format!("({})", p.kind.name())
                        } else {
                            p.name.clone()
                        };
                        menu_option(ui, &mut pick, p.id, label);
                    }
                });
            s.analysis.settings.source = TravelSource::Poi(pick);
        }
        TravelSource::Point(p) => hint_small(
            ui,
            &format!(
                "Click the ground to measure from there (now at {:.0}, {:.0}).",
                p[0], p[1]
            ),
        ),
        TravelSource::PlayerStart => no_start_note(ui, s),
    }
    let st = &mut s.analysis.settings;
    slider_row(ui, "Running", &mut st.run_speed, 3.0..=12.0, |v| {
        format!("{v:.1} m/s")
    })
    .on_hover_text("Walk mode jogs at 6.5 m/s.");
    slider_row(ui, "Swimming", &mut st.swim_speed, 0.5..=6.0, |v| {
        format!("{v:.1} m/s")
    });
    toggle(
        ui,
        &mut st.mounted,
        "On a mount (where the zone allows mounts)",
    );
    if st.mounted {
        slider_row(ui, "Mount", &mut st.mount_speed, 6.0..=30.0, |v| {
            format!("{v:.1} m/s")
        });
    }
    hint_small(
        ui,
        "Uphill is slower (about 30% on a 17° climb), wading is slower, roads are faster (×1.05 trail to ×1.35 highway).",
    );
}

fn gap_rules(ui: &mut egui::Ui, s: &mut UiState) {
    let st = &mut s.analysis.settings;
    slider_row_log(
        ui,
        "Dead after",
        &mut st.gap_threshold,
        15.0..=300.0,
        fmt_time,
    )
    .on_hover_text("Ground farther than this (on foot) from any content is a dead zone.");
    toggle(ui, &mut st.gap_fish, "Fish schools count as content");
    toggle(ui, &mut st.gap_roads, "Roads count as content");
    let gp = &s.gameplay;
    let n = gp.pois.len()
        + gp.npcs.len()
        + gp.spawns.len()
        + gp.nodes.len()
        + if st.gap_fish { s.water.fish.len() } else { 0 };
    hint_small(
        ui,
        &format!(
            "Measuring from {n} content point{}: POIs, NPCs, spawn regions, resource nodes{}.",
            if n == 1 { "" } else { "s" },
            if st.gap_fish { " and fish schools" } else { "" }
        ),
    );
    if n == 0 {
        hint_small(ui, "Add content in the Gameplay tab and the gaps fill in.");
    }
}

fn flow_rules(ui: &mut egui::Ui, s: &mut UiState) {
    let mut v = s.analysis.settings.level_jump as f32;
    slider_row(ui, "Flag jumps bigger than", &mut v, 1.0..=20.0, |v| {
        format!("{v:.0} levels")
    });
    s.analysis.settings.level_jump = v.round() as u32;
    hint_small(
        ui,
        "A jump is the difference between two neighbouring zones' average levels. Borders closed by cliffs or deep water don't count.",
    );
    if s.zones.list.is_empty() {
        hint_small(
            ui,
            "No zones yet: paint zones with level ranges in the Zones tab.",
        );
    }
}

fn reach_rules(ui: &mut egui::Ui, s: &mut UiState) {
    if s.gameplay.player_start.is_some() {
        hint_small(
            ui,
            "Flood fill on foot from the player start, using the walkability rules (Walkability view).",
        );
    }
    no_start_note(ui, s);
    hint_small(
        ui,
        "Pink: walkable ground nobody can reach. Orange: pits players slide into and can't climb out of.",
    );
}

/// A severity count as a pill with its colour dot.
fn severity_pill(ui: &mut egui::Ui, c: Color32, text: &str) {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(12.0), TEXT);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 28.0, 24.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(255), c.gamma_multiply(0.16));
    p.rect_stroke(
        rect,
        CornerRadius::same(255),
        Stroke::new(1.0, c.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    p.circle_filled(rect.left_center() + vec2(11.0, 0.0), 3.5, c);
    p.galley(rect.left_center() + vec2(20.0, -g.size().y * 0.5), g, TEXT);
}

fn doctor_rules(ui: &mut egui::Ui, s: &mut UiState) {
    let label = if s.analysis.report().is_some() {
        "Re-run checks"
    } else {
        "Run checks"
    };
    if primary_icon_button(ui, "Stethoscope", label).clicked() {
        s.analysis.want_doctor = true;
        s.analysis.parts.doctor = None;
    }
    if let Some(rep) = s.analysis.parts.doctor.as_ref().map(|d| d.1.clone()) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for sev in Severity::ALL {
                let n = rep
                    .issues
                    .iter()
                    .filter(|i| i.severity == sev && !s.analysis.fixed.contains(&i.id))
                    .count();
                severity_pill(
                    ui,
                    crate::analysis::severity_colour(sev),
                    &format!("{n} {}", sev.name().to_lowercase()),
                );
            }
        });
        hint_small(
            ui,
            &format!(
                "Checked {} objects in {:.0} ms. The list updates as you edit.",
                rep.objects, rep.ms
            ),
        );
    } else if s.analysis.want_doctor {
        hint_small(ui, "Checking…");
    }
    let mut b = s.analysis.settings.chunk_budget as f32;
    slider_row_log(
        ui,
        "Objects per 128 m chunk",
        &mut b,
        300.0..=20000.0,
        |v| format!("{:.0}", (v / 100.0).round() * 100.0),
    )
    .on_hover_text(
        "Trees, rocks, plants and props in one 128 × 128 m chunk before it counts as a hotspot.",
    );
    s.analysis.settings.chunk_budget = ((b / 100.0).round() * 100.0) as u32;
}

// ------------------------------------------------------------------ right panel

pub fn analyze_right(ui: &mut egui::Ui, s: &mut UiState) {
    let view = s.analysis.view;
    hint_small(ui, view_blurb(view));
    s.analysis.highlight = None;
    if s.analysis.grid().is_none() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            ui.add(egui::Spinner::new().size(14.0).color(TEXT_DIM));
            ui.label(
                RichText::new(
                    s.analysis
                        .progress()
                        .map(|p| format!("{}…", p.1))
                        .unwrap_or("Analysing…".into()),
                )
                .color(TEXT_DIM),
            );
        });
    } else {
        match view {
            View::Walkability => walk_results(ui, s),
            View::Travel => travel_results(ui, s),
            View::Gaps => gap_results(ui, s),
            View::Flow => flow_results(ui, s),
            View::Reach => reach_results(ui, s),
            View::Doctor => doctor_results(ui, s),
        }
    }
    ui.add_space(2.0);
    section_fold(ui, "analyze_legend", "Legend", "", |ui| legend(ui, s));
    if view != View::Doctor {
        slider_row(
            ui,
            "Overlay strength",
            &mut s.analysis.opacity,
            0.2..=1.0,
            |v| format!("{:.0}%", v * 100.0),
        );
    }
    if let Some((_, total)) = s.analysis.timings.iter().find(|t| t.0 == "total")
        && let Some(g) = s.analysis.grid()
    {
        let took = if *total >= 1000.0 {
            format!("{:.1} s", total / 1000.0)
        } else {
            format!("{total:.0} ms")
        };
        hint_small(
            ui,
            &format!(
                "Analysed {:.1} M cells of {:.1} m · updated in {took}",
                g.len() as f32 / 1.0e6,
                g.cell
            ),
        );
    }
}

fn legend(ui: &mut egui::Ui, s: &mut UiState) {
    let view = s.analysis.view;
    ui.spacing_mut().item_spacing.y = 3.0;
    if view == View::Travel {
        // the time ramp as equal-width steps between its stops
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        let n = paint::RAMP.len() - 1;
        let steps = 60;
        for k in 0..steps {
            let f = k as f32 / steps as f32 * n as f32;
            let (i, t) = (f.floor() as usize, f.fract());
            let sec = paint::RAMP[i].0 + (paint::RAMP[i + 1].0 - paint::RAMP[i].0) * t;
            let c = paint::ramp(sec);
            let x0 = r.left() + r.width() * k as f32 / steps as f32;
            let x1 = r.left() + r.width() * (k + 1) as f32 / steps as f32 + 0.5;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(pos2(x0, r.top()), pos2(x1, r.bottom())),
                0.0,
                Color32::from_rgb(c[0], c[1], c[2]),
            );
        }
        let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
        for (i, label) in ["0", "1 min", "2", "4", "8", "15+"].iter().enumerate() {
            let x = lr.left() + lr.width() * i as f32 / n as f32;
            let align = if i == 0 {
                egui::Align2::LEFT_TOP
            } else if i == n {
                egui::Align2::RIGHT_TOP
            } else {
                egui::Align2::CENTER_TOP
            };
            ui.painter().text(
                pos2(x, lr.top()),
                align,
                *label,
                FontId::new(10.5, mono()),
                TEXT_MUTE,
            );
        }
    }
    for (c, label) in paint::legend(view) {
        swatch(ui, col(c), label);
    }
    if view == View::Gaps {
        hint_small(
            ui,
            &format!(
                "Limit: {} on foot from the nearest content.",
                fmt_time(s.analysis.settings.gap_threshold)
            ),
        );
    }
    if view == View::Doctor {
        hint_small(
            ui,
            "Pins mark each issue; faint colours on the ground show walkability.",
        );
        for sev in Severity::ALL {
            swatch(
                ui,
                crate::analysis::severity_colour(sev),
                match sev {
                    Severity::Error => "Error: players will notice",
                    Severity::Warning => "Warning: likely a mistake",
                    Severity::Info => "Note: worth a look",
                },
            );
        }
    }
}

/// A big headline number (dead zones, reachability).
fn headline(ui: &mut egui::Ui, text: &str, c: Color32) {
    ui.label(
        RichText::new(text)
            .font(FontId::new(22.0, semibold()))
            .color(c),
    );
}

fn walk_results(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(g) = s.analysis.parts.grid.as_ref().map(|g| g.1.clone()) else {
        return;
    };
    let c = g.counts();
    let km2 = g.cell_area_km2();
    let land: usize = c[..5].iter().sum();
    let frac = |k: usize| c[k] as f32 / land.max(1) as f32;
    let pct = |k: usize| format!("{:.0}% · {:.2} km²", frac(k) * 100.0, c[k] as f64 * km2);
    ui.spacing_mut().item_spacing.y = 9.0;
    bar_stat(ui, "Walkable", &pct(0), frac(0), col(paint::WALK));
    bar_stat(ui, "Wading", &pct(1), frac(1), col(paint::SHALLOW));
    bar_stat(ui, "Steep", &pct(2), frac(2), col(paint::STEEP));
    bar_stat(ui, "Cliffs", &pct(3), frac(3), col(paint::CLIFF));
    bar_stat(
        ui,
        "Blocked by objects",
        &pct(4),
        frac(4),
        col(paint::OBSTACLE),
    );
    let roads = g
        .road
        .iter()
        .zip(&g.class)
        .filter(|(r, c)| **r > 0 && c.land())
        .count();
    if roads > 0 {
        bar_stat(
            ui,
            "Roads",
            &format!("{:.2} km²", roads as f64 * km2),
            roads as f32 / land.max(1) as f32,
            Color32::WHITE,
        );
    }
    stat(
        ui,
        "Swimmable water",
        format!("{:.1} km²", c[5] as f64 * km2),
    );
    if c[6] > 0 {
        stat(ui, "Open water", format!("{:.1} km²", c[6] as f64 * km2));
    }
    hint_small(
        ui,
        "Shares of the land (everything but deep water). Hover the map for slope and surface.",
    );
}

/// A small heading inside the results.
fn subhead(ui: &mut egui::Ui, text: &str) {
    ui.add_space(3.0);
    ui.label(
        RichText::new(text)
            .font(FontId::new(12.5, semibold()))
            .color(TEXT),
    );
}

fn travel_results(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(t) = s.analysis.parts.travel.as_ref().map(|t| t.1.clone()) else {
        return;
    };
    if t.source.is_none() {
        hint_small(
            ui,
            "Nothing to measure from yet: set a player start in the Gameplay tab, pick a POI or click the map.",
        );
        return;
    }
    subhead(ui, &format!("From {}", t.label));
    stat(ui, "Half the land within", fmt_time(t.median));
    stat(ui, "90% within", fmt_time(t.p90));
    stat(ui, "Reachable on foot", format!("{:.0}%", t.reachable_pct));
    for (label, v) in [
        ("Within 1 minute", t.within[0]),
        ("Within 2 minutes", t.within[1]),
        ("Within 5 minutes", t.within[2]),
    ] {
        stat(ui, label, format!("{v:.0}%"));
    }
    ui.add_space(2.0);
    if let Some(f) = t.farthest {
        let r = row(
            ui,
            Color32::from_rgb(150, 44, 170),
            &format!("Farthest point: {}", fmt_time(t.max)),
            "The longest walk from here",
            None,
        );
        if r.hovered {
            s.analysis.highlight = Some(f);
        }
        if r.goto {
            goto(s, f, 500.0);
        }
    }
    if let Some(src) = t.source {
        let r = row(ui, Color32::from_rgb(34, 140, 90), "Start", &t.label, None);
        if r.hovered {
            s.analysis.highlight = Some(src);
        }
        if r.goto {
            goto(s, src, 500.0);
        }
    }
}

fn gap_results(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(g) = s.analysis.parts.gaps.as_ref().map(|g| g.1.clone()) else {
        return;
    };
    if g.sources == 0 {
        subhead(ui, "No content yet");
        hint_small(
            ui,
            "Every spot is a dead zone until the world has POIs, NPCs, spawn regions, resource nodes or fish. Add them in the Gameplay tab.",
        );
        return;
    }
    subhead(ui, "Dead zones");
    headline(
        ui,
        &format!("{:.0}% of walkable ground", g.dead_pct),
        if g.dead_pct > 25.0 { DANGER } else { TEXT },
    );
    stat(
        ui,
        "Dead area",
        format!("{:.2} of {:.2} km²", g.dead_km2, g.walk_km2),
    );
    stat(ui, "Content points", g.sources.to_string());
    if g.regions.is_empty() {
        hint_small(ui, "No dead zones: content is never far away.");
        return;
    }
    subhead(ui, "Biggest gaps");
    for (i, r) in g.regions.iter().enumerate().take(10) {
        let worst = if r.max_s.is_finite() {
            format!("up to {} away", fmt_time(r.max_s))
        } else {
            "no route to content".into()
        };
        let o = row(
            ui,
            col(paint::DEAD),
            &format!("{}. {:.2} km²", i + 1, r.area_km2),
            &worst,
            None,
        );
        if o.hovered {
            s.analysis.highlight = Some(r.pos);
        }
        if o.goto {
            goto(s, r.pos, (r.area_km2.sqrt() * 1500.0).clamp(300.0, 1600.0));
        }
    }
    hint_small(
        ui,
        "Add a camp, a resource node, a creature spawn or a landmark near the worst spot.",
    );
}

fn flow_results(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(f) = s.analysis.parts.flow.as_ref().map(|f| f.1.clone()) else {
        return;
    };
    let zones = crate::analysis::job::zone_infos(&s.zones);
    if zones.is_empty() {
        hint_small(
            ui,
            "No zones yet. Paint zones and give them level ranges in the Zones tab.",
        );
        return;
    }
    if f.borders.is_empty() {
        hint_small(ui, "No zones touch each other across walkable ground.");
        return;
    }
    let name = |id: u8| {
        zones
            .iter()
            .find(|z| z.id == id)
            .map(|z| format!("{} (Lv {}–{})", z.name, z.level_min, z.level_max))
            .unwrap_or(format!("Zone {id}"))
    };
    let flagged = f.borders.iter().filter(|b| b.flagged).count();
    subhead(
        ui,
        &format!(
            "{flagged} border{} with a big jump",
            if flagged == 1 { "" } else { "s" }
        ),
    );
    for b in f.borders.iter().filter(|b| b.flagged) {
        let title = format!("{} → {}", name(b.low), name(b.high));
        let sub = format!(
            "+{:.0} levels · {:.0} m open · {:.0} m closed\n{}",
            b.jump,
            b.open_m,
            b.closed_m,
            flow::suggestion(b, &zones, f.threshold)
        );
        let o = row(ui, col(paint::BORDER_JUMP), &title, &sub, None);
        if let Some(seg) = b.segments.first() {
            if o.hovered {
                s.analysis.highlight = Some(seg.pos);
            }
            if o.goto {
                goto(s, seg.pos, 700.0);
            }
        }
    }
    let ok: Vec<_> = f.borders.iter().filter(|b| !b.flagged).collect();
    if !ok.is_empty() {
        subhead(ui, "Smooth borders");
        for b in ok {
            let sub = if b.open_m > 0.0 {
                format!("{:.0} levels apart · {:.0} m open", b.jump, b.open_m)
            } else {
                format!("{:.0} levels apart · closed by cliffs or water", b.jump)
            };
            row(
                ui,
                col(paint::BORDER_OPEN),
                &format!("{} ↔ {}", name(b.low), name(b.high)),
                &sub,
                None,
            );
        }
    }
}

fn reach_results(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(r) = s.analysis.parts.reach.as_ref().map(|r| r.1.clone()) else {
        return;
    };
    if r.origin.is_none() {
        hint_small(ui, "Click the ground to choose where players start.");
        return;
    }
    subhead(ui, &format!("From {}", r.origin_label));
    headline(
        ui,
        &format!(
            "{:.0}% reachable",
            r.reachable_km2 * 100.0 / r.land_km2.max(1e-6)
        ),
        TEXT,
    );
    stat(
        ui,
        "Unreachable walkable ground",
        format!("{:.2} km²", r.unreachable_km2),
    );
    stat(ui, "Traps", format!("{:.0} m²", r.trap_km2 * 1.0e6));
    if !r.items.is_empty() {
        subhead(ui, "Can't be reached");
        for it in &r.items {
            let o = row(
                ui,
                Color32::from_rgb(220, 48, 48),
                &format!(
                    "{} '{}'",
                    it.kind,
                    if it.name.is_empty() {
                        "(unnamed)"
                    } else {
                        &it.name
                    }
                ),
                it.why,
                None,
            );
            if o.hovered {
                s.analysis.highlight = Some(it.pos);
            }
            if o.goto {
                goto(s, it.pos, 300.0);
            }
        }
    }
    if !r.traps.is_empty() {
        subhead(ui, "Traps");
        for t in r.traps.iter().take(6) {
            let o = row(
                ui,
                col(paint::TRAP),
                &format!("{:.0} m² pit", t.area_km2 * 1.0e6),
                "Players slide in and can't walk out",
                None,
            );
            if o.hovered {
                s.analysis.highlight = Some(t.pos);
            }
            if o.goto {
                goto(s, t.pos, 250.0);
            }
        }
    }
    if !r.pockets.is_empty() {
        subhead(ui, "Unreachable areas");
        for (i, p) in r.pockets.iter().enumerate().take(8) {
            let o = row(
                ui,
                col(paint::POCKET),
                &format!("{}. {:.3} km²", i + 1, p.area_km2),
                "Walkable, but no way in on foot",
                None,
            );
            if o.hovered {
                s.analysis.highlight = Some(p.pos);
            }
            if o.goto {
                goto(s, p.pos, 400.0);
            }
        }
    }
}

fn doctor_results(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(rep) = s.analysis.parts.doctor.as_ref().map(|d| d.1.clone()) else {
        if s.analysis.want_doctor {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.add(egui::Spinner::new().size(14.0).color(TEXT_DIM));
                ui.label(RichText::new("Checking the world…").color(TEXT_DIM));
            });
        } else {
            hint_small(ui, "Press Run checks to examine the world.");
        }
        return;
    };
    let live: Vec<&doctor::Issue> = rep
        .issues
        .iter()
        .filter(|i| !s.analysis.fixed.contains(&i.id))
        .collect();
    if live.is_empty() {
        icon_line(ui, "CheckCircle2", OK, "All clear: no problems found.");
        return;
    }
    let fixable: Vec<&doctor::Issue> = live.iter().copied().filter(|i| i.fix.is_some()).collect();
    let mut apply: Vec<&doctor::Issue> = Vec::new();
    if !fixable.is_empty()
        && primary_icon_button(
            ui,
            "MagicWand",
            &format!("Fix all {} fixable issues", fixable.len()),
        )
        .on_hover_text("Applies every safe fix below as one undo step.")
        .clicked()
    {
        apply = fixable.clone();
    }
    for sev in Severity::ALL {
        let of: Vec<&doctor::Issue> = live.iter().copied().filter(|i| i.severity == sev).collect();
        if of.is_empty() {
            continue;
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
            ui.painter()
                .circle_filled(r.center(), 4.0, crate::analysis::severity_colour(sev));
            ui.label(
                RichText::new(sev.name())
                    .font(FontId::new(13.0, semibold()))
                    .color(TEXT),
            );
            ui.label(
                RichText::new(of.len().to_string())
                    .font(FontId::new(11.0, mono()))
                    .color(TEXT_MUTE),
            );
        });
        for check in Check::ALL {
            let items: Vec<&doctor::Issue> =
                of.iter().copied().filter(|i| i.check == check).collect();
            if items.is_empty() {
                continue;
            }
            let id = ui.make_persistent_id(("doctor_group", check.id(), sev.id()));
            let can_fix = items.iter().filter(|i| i.fix.is_some()).count();
            let (open, fix_all) = group_header(
                ui,
                id,
                check.name(),
                items.len(),
                items.len() <= 3,
                can_fix > 1,
                check.hint(),
            );
            if fix_all {
                apply.extend(items.iter().copied().filter(|i| i.fix.is_some()));
            }
            if open {
                hint_small(ui, check.hint());
                for it in items.iter().take(40) {
                    let o = row(
                        ui,
                        crate::analysis::severity_colour(it.severity),
                        &it.title,
                        &it.detail,
                        it.fix.as_ref().map(|f| f.label()),
                    );
                    if o.hovered {
                        s.analysis.highlight = Some(it.pos);
                    }
                    if o.goto {
                        let dist = if it.check == Check::Hotspot {
                            380.0
                        } else {
                            120.0
                        };
                        goto(s, it.pos, dist);
                    }
                    if o.fix {
                        apply.push(it);
                    }
                }
                if items.len() > 40 {
                    hint_small(
                        ui,
                        &format!("+{} more (Fix all handles them all)", items.len() - 40),
                    );
                }
            }
        }
    }
    if !apply.is_empty() {
        let fixes: Vec<&Fix> = apply.iter().filter_map(|i| i.fix.as_ref()).collect();
        let msg = doctor::apply(
            &fixes,
            &s.terrain,
            &mut s.store,
            s.lib.as_deref(),
            &mut s.zones,
            &mut s.history,
        );
        s.analysis.fixed.extend(apply.iter().map(|i| i.id));
        s.meta.dirty = true;
        if let Some(m) = msg {
            s.ed.toast(m);
        }
    }
}

/// A check's collapsible group header: chevron, name, count, and "Fix all" when it has several
/// fixable issues. Returns (open, Fix all clicked).
fn group_header(
    ui: &mut egui::Ui,
    id: egui::Id,
    name: &str,
    n: usize,
    default_open: bool,
    fix_all: bool,
    hint: &str,
) -> (bool, bool) {
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let mut fix = false;
    ui.horizontal(|ui| {
        let w = ui.available_width() - if fix_all { 64.0 } else { 0.0 };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
        if resp.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_persisted(id, open));
        }
        let c = if resp.hovered() { TEXT } else { TEXT_DIM };
        let p = ui.painter();
        chevron(
            p,
            pos2(rect.left() + 5.0, rect.center().y),
            3.2,
            if open { 0.0 } else { -90.0 },
            TEXT_DIM,
        );
        let g = p.layout_no_wrap(name.to_string(), FontId::new(12.5, medium()), c);
        let gw = g.size().x;
        p.galley(
            pos2(rect.left() + 16.0, rect.center().y - g.size().y * 0.5),
            g,
            c,
        );
        let ng = p.layout_no_wrap(n.to_string(), FontId::new(11.0, mono()), TEXT_MUTE);
        p.galley(
            pos2(rect.left() + 22.0 + gw, rect.center().y - ng.size().y * 0.5),
            ng,
            TEXT_MUTE,
        );
        resp.on_hover_text(hint)
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if fix_all && small_button(ui, "Fix all").on_hover_text(hint).clicked() {
            fix = true;
        }
    });
    (open, fix)
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The results inspector: always there while the tab is open, titled by the view.
pub(super) fn analyze_inspector(s: &UiState) -> Option<(String, bool)> {
    Some((s.analysis.view.name().to_string(), true))
}

/// The dock: the view and what clicking the map does in it.
pub(super) fn analyze_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let v = s.analysis.view;
    dock_title(ui, view_icon(v), v.name());
    dock_sep(ui);
    let click = match v {
        View::Travel if matches!(s.analysis.settings.source, TravelSource::Point(_)) => {
            Some("Measure from here")
        }
        View::Reach if s.gameplay.player_start.is_none() => Some("Move the start"),
        _ => None,
    };
    let mut hints: Vec<(&str, &str)> = vec![];
    if let Some(c) = click {
        hints.push(("Click", c));
    }
    hints.push(("Hover", "Read a spot"));
    hints.push(("RMB", "Orbit"));
    hints.push(("Scroll", "Zoom"));
    dock_hints(ui, &hints);
    true
}
