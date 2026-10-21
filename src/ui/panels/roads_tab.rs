//! Roads tab in the Halo layout (HANDOFF › Per-tab mapping): the road tools, road type tiles
//! (photoscanned surface swatches), width, build options and the list of roads in the panel; the
//! selected road's inspector (name, type, width, stats, grade profile, options, Rebuild / Delete);
//! the road type, width and the tool's key hints in the dock.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, Sense, Stroke, pos2, vec2};

use super::super::UiState;
use super::super::theme::*;
use super::super::widgets::*;
use super::meters;
use crate::roads::tool::{Request, RoadStats};
use crate::roads::{Road, RoadKind, RoadOptions, RoadToolMode};

const TOOLS: [(RoadToolMode, &str, &str); 4] = [
    (RoadToolMode::Draw, "Pencil2", "Draw Road"),
    (RoadToolMode::Route, "MagicWand", "Auto Route"),
    (RoadToolMode::Edit, "Bezier", "Edit"),
    (RoadToolMode::Remove, "Eraser", "Remove"),
];

/// The kinds' surface previews as egui textures (made once from the library swatches).
fn swatches(ui: &egui::Ui, s: &mut UiState) -> Vec<Option<egui::TextureId>> {
    let tool = &mut s.road_tool;
    if tool.swatch_tex.is_empty() {
        let tex: Vec<egui::TextureHandle> = tool
            .swatches
            .iter()
            .enumerate()
            .map(|(k, sw)| {
                let img = match sw {
                    Some((w, h, px)) => {
                        egui::ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], px)
                    }
                    None => egui::ColorImage::new([1, 1], vec![Color32::from_gray(90)]),
                };
                ui.ctx()
                    .load_texture(format!("road_kind_{k}"), img, egui::TextureOptions::LINEAR)
            })
            .collect();
        tool.swatch_tex = tex;
    }
    tool.swatch_tex.iter().map(|t| Some(t.id())).collect()
}

/// A small rounded strip of the road's surface (tiles, dock, list).
fn surface_strip(ui: &egui::Ui, rect: egui::Rect, tex: Option<egui::TextureId>) {
    match tex {
        Some(t) => {
            egui::Image::new((t, rect.size()))
                .uv(egui::Rect::from_min_max(pos2(0.0, 0.3), pos2(1.0, 0.7)))
                .corner_radius(CornerRadius::same(3))
                .paint_at(ui, rect);
        }
        None => {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(3), Color32::from_gray(90));
        }
    }
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, Color32::from_black_alpha(64)),
        egui::StrokeKind::Inside,
    );
}

/// One road type tile (two columns): its surface strip and name; white when picked.
fn road_tile(
    ui: &mut egui::Ui,
    tex: Option<egui::TextureId>,
    kind: RoadKind,
    selected: bool,
    w: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            Color32::WHITE
        } else if resp.hovered() {
            SOFT_HOVER
        } else {
            SOFT
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_TILE), fill);
        surface_strip(
            ui,
            egui::Rect::from_min_size(
                pos2(rect.left() + 10.0, rect.center().y - 6.0),
                vec2(26.0, 12.0),
            ),
            tex,
        );
        let fg = if selected { ON_ACCENT } else { TEXT };
        let avail = w - 46.0 - 8.0;
        let p = ui.painter();
        let mut g = p.layout_no_wrap(kind.name().to_string(), FontId::proportional(13.0), fg);
        if g.size().x > avail {
            g = p.layout_no_wrap(kind.short().to_string(), FontId::proportional(13.0), fg);
        }
        p.galley(
            pos2(rect.left() + 44.0, rect.center().y - g.size().y * 0.5),
            g,
            fg,
        );
    }
    resp.on_hover_text(format!("{}\n{}", kind.name(), kind.style().blurb))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Road type tiles, two per row. Returns the clicked kind.
fn kind_picker(
    ui: &mut egui::Ui,
    tex: &[Option<egui::TextureId>],
    current: RoadKind,
) -> Option<RoadKind> {
    let w = ((ui.available_width() - 6.0) / 2.0).floor();
    let mut picked = None;
    for row in RoadKind::ALL.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for k in row {
                let t = tex.get(k.index()).copied().flatten();
                if road_tile(ui, t, *k, *k == current, w).clicked() {
                    picked = Some(*k);
                }
            }
        });
    }
    picked
}

fn options_ui(ui: &mut egui::Ui, o: &mut RoadOptions, kind: RoadKind) -> bool {
    let mut changed = false;
    changed |= toggle(ui, &mut o.bed, "Terrain bed")
        .on_hover_text("Cut and fill the ground into a smooth road bed with soft shoulders")
        .changed();
    changed |= toggle(ui, &mut o.clear, "Clear vegetation")
        .on_hover_text(
            "Remove trees, plants and rocks in the way (they come back if the road goes)",
        )
        .changed();
    changed |= toggle(ui, &mut o.bridges, "Bridges")
        .on_hover_text(if kind.style().stone_bridge {
            "Stone arch bridges over rivers, lakes and straits"
        } else {
            "Timber bridges over rivers, lakes and straits"
        })
        .changed();
    if kind.style().lamps > 0.0 {
        changed |= toggle(ui, &mut o.props, "Lamp posts")
            .on_hover_text("Iron lamp posts along the road")
            .changed();
    }
    changed
}

pub fn roads_left(ui: &mut egui::Ui, s: &mut UiState) {
    section_fold(ui, "roads_tools", "Roads", "", |ui| {
        hint_small(
            ui,
            "Paths and roads follow the land, clear the way and bridge rivers.",
        );
        let items: Vec<(&str, &str, &str)> = TOOLS.iter().map(|(_, i, n)| (*i, *n, "")).collect();
        let sel = TOOLS.iter().position(|t| t.0 == s.road_tool.mode);
        if let Some(i) = tool_grid(ui, 2, &items, sel) {
            s.road_tool.mode = TOOLS[i].0;
            s.road_tool.draft.clear();
            s.road_tool.route_from = None;
        }
        dim(ui, s.road_tool.mode.hint());
    });

    let tex = swatches(ui, s);
    section_fold(ui, "roads_type", "Road type", "", |ui| {
        if let Some(k) = kind_picker(ui, &tex, s.road_tool.kind) {
            s.road_tool.kind = k;
            s.road_tool.width = k.default_width();
        }
        let k = s.road_tool.kind;
        hint_small(ui, &format!("{}: {}", k.name(), k.style().blurb));
        slider_row(ui, "Width", &mut s.road_tool.width, 1.0..=16.0, |v| {
            format!("{v:.1} m")
        });
    });
    section_fold(ui, "roads_build", "Build", "", |ui| {
        let k = s.road_tool.kind;
        options_ui(ui, &mut s.road_tool.options, k);
    });

    let mut select = None;
    section_fold(
        ui,
        "roads_list",
        "Roads",
        &s.roads.roads.len().to_string(),
        |ui| {
            if s.roads.roads.is_empty() {
                hint_small(
                    ui,
                    "No roads yet. Draw one, or let Auto Route find the way between two places.",
                );
            }
            ui.spacing_mut().item_spacing.y = 2.0;
            for r in &s.roads.roads {
                let sel = s.roads.selected == Some(r.id);
                let len = s
                    .road_tool
                    .stats
                    .get(&r.id)
                    .map(|st| st.length)
                    .unwrap_or(0.0);
                let lead = match tex.get(r.kind.index()).copied().flatten() {
                    Some(t) => Lead::Image(t),
                    None => Lead::Icon("AnimatePath"),
                };
                if list_row(
                    ui,
                    lead,
                    &r.name,
                    r.kind.name(),
                    &format!("{len:.0} m"),
                    sel,
                )
                .clicked()
                {
                    select = Some(r.id);
                }
            }
            if s.roads.selected.is_none() {
                ui.add_space(4.0);
                hint_small(
                    ui,
                    "Select a road (Edit tool, or this list) to see its length, grades and profile and change its type, width and options.",
                );
                hint_small(
                    ui,
                    "Tips: start and end roads on other roads to join them into a network. Auto Route climbs hills with switchbacks and bridges rivers where they are narrow.",
                );
            }
        },
    );
    if let Some(id) = select {
        s.roads.selected = Some(id);
        // fly to it
        if let Some(r) = s.roads.get(id) {
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in &r.points {
                lo = lo.min(Vec2::new(p[0], p[2]));
                hi = hi.max(Vec2::new(p[0], p[2]));
            }
            let c = (lo + hi) * 0.5;
            let dist = ((hi - lo).length() * 1.1).clamp(60.0, 3000.0);
            let (yaw, pitch) = (s.orbit.yaw, s.orbit.pitch.max(0.55));
            s.orbit.set_view(
                Vec3::new(c.x, s.terrain.height_at(c.x, c.y), c.y),
                yaw,
                pitch,
                dist,
            );
        }
    }
}

/// The road's elevation along its length: ground (a faint band) and road (a line coloured by
/// grade: white, amber over the design grade, red well over it), with the lowest and highest
/// heights underneath.
fn profile_chart(ui: &mut egui::Ui, st: &RoadStats, max_grade: f32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(R_INPUT), WELL);
    if st.profile.len() < 2 {
        return;
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for q in &st.profile {
        lo = lo.min(q[1].min(q[2]));
        hi = hi.max(q[1].max(q[2]));
    }
    let pad = ((hi - lo) * 0.12).max(1.0);
    let (lo, hi) = (lo - pad, hi + pad);
    let len = st.length.max(1.0);
    let inner = rect.shrink2(vec2(6.0, 7.0));
    let at = |s: f32, h: f32| {
        pos2(
            inner.left() + s / len * inner.width(),
            inner.bottom() - (h - lo) / (hi - lo) * inner.height(),
        )
    };
    // ground as a filled band, the road as a line coloured by grade
    let ground: Vec<egui::Pos2> = st.profile.iter().map(|q| at(q[0], q[2])).collect();
    let mut fill = egui::Mesh::default();
    for (i, g) in ground.iter().enumerate() {
        fill.colored_vertex(*g, Color32::from_white_alpha(34));
        fill.colored_vertex(pos2(g.x, inner.bottom()), Color32::from_white_alpha(6));
        if i > 0 {
            let b = (i * 2) as u32;
            fill.add_triangle(b - 2, b - 1, b);
            fill.add_triangle(b - 1, b + 1, b);
        }
    }
    p.add(egui::Shape::mesh(fill));
    for w2 in st.profile.windows(2) {
        let g = (w2[1][1] - w2[0][1]).abs() / (w2[1][0] - w2[0][0]).max(0.01);
        let col = if g > max_grade * 1.25 {
            DANGER
        } else if g > max_grade {
            WARN
        } else {
            Color32::WHITE
        };
        p.line_segment(
            [at(w2[0][0], w2[0][1]), at(w2[1][0], w2[1][1])],
            Stroke::new(1.6, col),
        );
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{lo:.0} m"))
                .size(11.5)
                .color(TEXT_MUTE),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{hi:.0} m"))
                    .size(11.5)
                    .color(TEXT_MUTE),
            );
        });
    });
}

/// A stat row whose value carries a colour (grades over the design limit).
fn stat_col(ui: &mut egui::Ui, label: &str, value: &str, col: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    let p = ui.painter();
    let vg = p.layout_no_wrap(value.to_owned(), FontId::new(12.0, mono()), col);
    let lg = p.layout_no_wrap(label.to_owned(), FontId::proportional(13.0), TEXT_DIM);
    p.galley(
        pos2(rect.left(), rect.center().y - lg.size().y * 0.5),
        lg,
        TEXT_DIM,
    );
    p.galley(
        pos2(
            rect.right() - vg.size().x,
            rect.center().y - vg.size().y * 0.5,
        ),
        vg,
        col,
    );
}

pub fn roads_right(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(id) = s.roads.selected else { return };
    let Some(road) = s.roads.get(id).cloned() else {
        s.roads.selected = None;
        return;
    };
    let tex = swatches(ui, s);
    // name (renaming needs no rebuild)
    ui.label(RichText::new("Name").size(12.5).color(TEXT_DIM));
    egui::Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(R_INPUT))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(rr) = s.roads.roads.iter_mut().find(|x| x.id == id)
                && ui
                    .add(
                        egui::TextEdit::singleline(&mut rr.name)
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .font(FontId::new(14.0, semibold())),
                    )
                    .changed()
            {
                s.meta.dirty = true;
            }
        });
    let mut new: Road = s.roads.get(id).cloned().unwrap_or(road.clone());
    let mut rebuild = false;
    section_fold(ui, "road_insp_type", "Type", "", |ui| {
        if let Some(k) = kind_picker(ui, &tex, road.kind)
            && k != road.kind
        {
            if (new.width - road.kind.default_width()).abs() < 0.05 {
                new.width = k.default_width();
            }
            new.kind = k;
            rebuild = true;
        }
        // width: previewed in the slider, applied when let go
        let mut w = s
            .road_tool
            .width_edit
            .filter(|e| e.0 == id)
            .map(|e| e.1)
            .unwrap_or(road.width);
        let resp = slider_row(ui, "Width", &mut w, 1.0..=16.0, |v| format!("{v:.1} m"));
        if resp.changed() {
            s.road_tool.width_edit = Some((id, w));
        }
        if (resp.drag_stopped() || (resp.changed() && !resp.dragged()))
            && (w - road.width).abs() > 0.01
        {
            new.width = w;
            s.road_tool.width_edit = None;
            rebuild = true;
        }
    });

    // stats
    let st = s.road_tool.stats.get(&id).cloned().unwrap_or_default();
    let design = road.kind.style().max_grade;
    section_fold(ui, "road_insp_stats", "Stats", "", |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        let grade_col = if st.max_grade > design * 1.25 {
            DANGER
        } else if st.max_grade > design {
            WARN
        } else {
            TEXT
        };
        stat_row(ui, "Type", road.kind.name());
        stat_row(ui, "Width", &format!("{:.1} m", road.width));
        stat_row(ui, "Length", &meters(st.length));
        stat_col(
            ui,
            "Steepest grade",
            &format!("{:.1} %", st.max_grade * 100.0),
            grade_col,
        );
        stat_row(
            ui,
            "Average grade",
            &format!("{:.1} %", st.avg_grade * 100.0),
        );
        stat_row(
            ui,
            "Climb / descent",
            &format!("+{:.0} / −{:.0} m", st.climb, st.descent),
        );
        stat_row(
            ui,
            "Travel speed",
            &format!("×{:.2}", road.kind.travel_factor()),
        );
        if st.bridges > 0 {
            stat_row(
                ui,
                "Bridges",
                &format!("{} ({:.0} m)", st.bridges, st.bridge_len),
            );
        }
        stat_col(
            ui,
            "Cleared",
            &format!("{} plants & trees", road.cleared.len()),
            TEXT_DIM,
        );
    });
    section_fold(ui, "road_insp_profile", "Profile", "", |ui| {
        profile_chart(ui, &st, design);
        if st.max_grade > design * 1.25 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                crate::ui::icons::icon(ui, "ExclamationTriangle", 15.0, WARN);
                ui.add(egui::Label::new(RichText::new(format!("Steeper than a {} should be ({:.0} %). Add points to wind it up the slope, or use Auto Route.", road.kind.name().to_lowercase(), design * 100.0)).size(12.5).color(WARN)).wrap());
            });
        }
    });
    section_fold(ui, "road_insp_options", "Options", "", |ui| {
        if options_ui(ui, &mut new.options, new.kind) {
            rebuild = true;
        }
    });
    if rebuild {
        s.road_tool.requests.push(Request::Update(new));
    }
    ui.add_space(4.0);
    let bw = ((ui.available_width() - 6.0) / 2.0).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if icon_text_button(ui, "ArrowsRepeat", "Rebuild", Some(bw)).on_hover_text("Re-carve the bed, clear the way and rebuild bridges and props (after sculpting, painting or adding water nearby)").clicked() {
            s.road_tool.requests.push(Request::Rebuild(id));
        }
        if icon_text_button(ui, "TrashCan", "Delete road", Some(bw)).clicked() {
            s.road_tool.requests.push(Request::Remove(id));
        }
    });
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The inspector shows while a road is selected.
pub(super) fn roads_inspector(s: &UiState) -> Option<(String, bool)> {
    s.roads
        .selected
        .filter(|id| s.roads.get(*id).is_some())
        .map(|_| ("Road".to_string(), true))
}

/// The dock: the road type and width, then the tool's key hints.
pub(super) fn roads_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let mode = s.road_tool.mode;
    let (_, icon, name) = TOOLS
        .iter()
        .find(|t| t.0 == mode)
        .copied()
        .unwrap_or(TOOLS[0]);
    match mode {
        RoadToolMode::Draw | RoadToolMode::Route => {
            let tex = swatches(ui, s);
            let k = s.road_tool.kind;
            let (r, _) = ui.allocate_exact_size(vec2(26.0, 28.0), Sense::hover());
            surface_strip(
                ui,
                egui::Rect::from_center_size(r.center(), vec2(26.0, 12.0)),
                tex.get(k.index()).copied().flatten(),
            );
            ui.label(
                RichText::new(k.name())
                    .font(FontId::new(13.0, semibold()))
                    .color(TEXT),
            )
            .on_hover_text(name);
            dock_sep(ui);
            slider_mini(ui, "Width", &mut s.road_tool.width, 1.0..=16.0, |v| {
                format!("{v:.1} m")
            });
            dock_sep(ui);
            if mode == RoadToolMode::Route {
                let hint = if s.road_tool.route_from.is_some() {
                    "To B"
                } else {
                    "From A"
                };
                dock_hints(ui, &[("Click", hint), ("Esc", "Cancel")]);
            } else {
                dock_hints(
                    ui,
                    &[
                        ("Click", "Add point"),
                        ("↵", "Finish road"),
                        ("⌫", "Last point"),
                        ("Esc", "Cancel"),
                    ],
                );
            }
        }
        RoadToolMode::Edit => {
            dock_title(ui, icon, name);
            dock_sep(ui);
            dock_hints(
                ui,
                &[
                    ("Click", "Select · add a point"),
                    ("Drag", "Move a point"),
                    ("⌥", "Click deletes a point"),
                    ("Esc", "Deselect"),
                ],
            );
        }
        RoadToolMode::Remove => {
            dock_title(ui, icon, name);
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Remove a road")]);
        }
    }
    true
}
