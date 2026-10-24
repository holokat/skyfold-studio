//! Reusable styled widgets (Halo): glass islands, uppercase section headers, white-thumb sliders,
//! segmented controls, toggles, dropdowns, tool tiles, cards, chips, list rows and keycaps.
//! White is the only accent: a selected tile, chip or primary button is a white fill with dark text.

use std::hash::Hash;
use std::ops::RangeInclusive;
use std::sync::Arc;

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Galley, Id, InnerResponse, Key, Margin,
    Mesh, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, TextStyle,
    TextureId, Ui, Vec2, WidgetInfo, WidgetText, WidgetType, emath, pos2, style::StyleModifier,
    style::WidgetVisuals, text::LayoutJob, vec2,
};

use super::theme::*;

// ------------------------------------------------------------------ text helpers

/// One line of text, cut with "…" when wider than `max_w`.
fn one_line(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_w.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    painter.layout_job(job)
}

/// Up to `rows` centred lines, the last cut with "…".
fn centred_rows(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
    rows: usize,
) -> Arc<Galley> {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, max_w.max(1.0));
    job.wrap.max_rows = rows;
    job.halign = egui::Align::Center;
    painter.layout_job(job)
}

/// Paint a galley from [`centred_rows`] centred on `cx`, its top at `top`.
fn paint_centred(p: &Painter, g: Arc<Galley>, cx: f32, top: f32, color: Color32) {
    p.galley(pos2(cx - g.rect.center().x, top - g.rect.min.y), g, color);
}

/// Draw an icon centred on `c`: a Central icon by name (`"FormPyramide"`), or a Phosphor glyph
/// while call sites move over to Central icons.
pub fn paint_icon(ui: &Ui, c: Pos2, icon: &str, size: f32, color: Color32) {
    if super::icons::has(icon) {
        super::icons::paint(ui.painter(), c, icon, size, color);
    } else {
        ui.painter().text(
            c,
            Align2::CENTER_CENTER,
            icon,
            FontId::proportional(size + 1.0),
            color,
        );
    }
}

/// Where an instant [`tip`] sits.
#[derive(Clone, Copy, PartialEq)]
pub enum TipSide {
    Right,
    Below,
    Above,
}

/// The chrome tooltip: shows at once on hover (fading in over 0.12 s) as a small dark-glass pill
/// beside the widget, with a muted second part and a keycap. Rail buttons put it to the right,
/// top-bar and panel buttons below, the dock above. Long explanations keep egui's delayed tooltips.
pub fn tip(resp: Response, title: &str, sub: &str, key: &str, side: TipSide) -> Response {
    let show = resp.hovered() && !resp.is_pointer_button_down_on() && !title.is_empty();
    let a = resp
        .ctx
        .animate_bool_with_time(resp.id.with("halo_tip"), show, 0.12);
    if a > 0.0 {
        paint_tip(&resp.ctx, resp.id, resp.rect, title, sub, key, side, a);
    }
    resp
}

#[allow(clippy::too_many_arguments)]
fn paint_tip(
    ctx: &egui::Context,
    id: Id,
    r: Rect,
    title: &str,
    sub: &str,
    key: &str,
    side: TipSide,
    a: f32,
) {
    let mut p = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        id.with("halo_tip_layer"),
    ));
    p.set_opacity(a);
    let mut job = LayoutJob::default();
    job.append(
        title,
        0.0,
        egui::TextFormat {
            font_id: FontId::proportional(12.0),
            color: TEXT,
            ..Default::default()
        },
    );
    if !sub.is_empty() {
        job.append(
            sub,
            8.0,
            egui::TextFormat {
                font_id: FontId::proportional(12.0),
                color: TEXT_MUTE,
                ..Default::default()
            },
        );
    }
    job.wrap.max_width = 300.0;
    let g = p.layout_job(job);
    let kg = (!key.is_empty()).then(|| kbd_galley(&p, key, 10.5, TEXT_DIM));
    let kw = kg
        .as_ref()
        .map_or(0.0, |k| (k.size().x + 8.0).max(k.size().y + 4.0) + 8.0);
    let size = vec2(8.0 + g.size().x + kw + 7.0, g.size().y.max(16.0) + 10.0);
    let min = match side {
        TipSide::Right => pos2(r.right() + 12.0, r.center().y - size.y * 0.5),
        TipSide::Below => pos2(r.center().x - size.x * 0.5, r.bottom() + 10.0),
        TipSide::Above => pos2(r.center().x - size.x * 0.5, r.top() - 10.0 - size.y),
    };
    let screen = ctx.content_rect().shrink(4.0);
    let mut rect = Rect::from_min_size(min, size);
    rect = rect.translate(vec2(
        (screen.right() - rect.right()).min(0.0) + (screen.left() - rect.left()).max(0.0),
        (screen.bottom() - rect.bottom()).min(0.0) + (screen.top() - rect.top()).max(0.0),
    ));
    p.rect(
        rect,
        CornerRadius::same(8),
        Color32::from_rgba_unmultiplied_const(14, 17, 22, 209),
        Stroke::new(1.0, LINE),
        StrokeKind::Inside,
    );
    p.galley(
        pos2(rect.left() + 8.0, rect.center().y - g.size().y * 0.5),
        g,
        TEXT,
    );
    if kg.is_some() {
        paint_kbd(&p, pos2(rect.right() - 7.0, rect.center().y), key, false);
    }
}

/// Split "Remove (Delete)" into ("Remove", "Delete") when the parenthesis holds a short key.
fn split_key(text: &str) -> (&str, &str) {
    if let Some(rest) = text.strip_suffix(')')
        && let Some(i) = rest.rfind(" (")
    {
        let key = &rest[i + 2..];
        let keyish = key.chars().count() <= 10
            && !key
                .split(|c: char| !c.is_alphabetic())
                .any(|w| w.chars().count() > 6);
        if keyish && !key.is_empty() {
            return (&text[..i], key);
        }
    }
    (text, "")
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

/// Native size of a texture egui manages (thumbnails loaded through `ctx.load_texture`).
fn texture_size(ctx: &egui::Context, id: TextureId) -> Option<Vec2> {
    ctx.tex_manager()
        .read()
        .meta(id)
        .map(|m| vec2(m.size[0] as f32, m.size[1] as f32))
}

// ------------------------------------------------------------------ glass

/// The glass island frame (panels, rail, top islands, dock): translucent fill, 1 px light rim.
/// Show it with [`glass`] to also get the soft shadow and the inner top highlight.
pub fn glass_frame() -> Frame {
    Frame::new()
        .fill(GLASS)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(R_PANEL))
        .inner_margin(Margin::symmetric(13, 12))
}

/// Pre-Halo name of [`glass_frame`].
pub fn panel_frame() -> Frame {
    glass_frame()
}

/// Menus, the ⌘K palette and windows: near-opaque glass with a stronger rim.
pub fn glass_solid_frame() -> Frame {
    Frame::new()
        .fill(GLASS_SOLID)
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(R_WINDOW))
        .inner_margin(Margin::symmetric(18, 15))
        .shadow(egui::epaint::Shadow {
            offset: [0, 26],
            blur: 70,
            spread: 0,
            color: Color32::from_black_alpha(140),
        })
}

/// Show `frame` as a floating glass island: a soft shadow outside it (so the translucent fill
/// isn't darkened from below), the frame, and a 1 px highlight inside its top edge.
pub fn glass<R>(
    ui: &mut Ui,
    frame: Frame,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    let shadow = ui.painter().add(Shape::Noop);
    let r = frame.show(ui, add_contents);
    let rect = r.response.rect;
    let radius = frame.corner_radius.nw as f32;
    ui.painter().set(
        shadow,
        soft_shadow(rect, radius, Color32::from_black_alpha(150)),
    );
    glass_highlight(ui.painter(), rect, radius);
    r
}

/// The glass highlight: a faint white line just inside the top edge.
pub fn glass_highlight(painter: &Painter, rect: Rect, radius: f32) {
    let y = rect.top() + 1.5;
    painter.line_segment(
        [
            pos2(rect.left() + radius * 0.75, y),
            pos2(rect.right() - radius * 0.75, y),
        ],
        Stroke::new(1.0, HIGHLIGHT),
    );
}

fn rounded_outline(r: Rect, radius: f32, seg: usize, out: &mut Vec<Pos2>) {
    let radius = radius.min(r.width() * 0.5).min(r.height() * 0.5);
    let corners = [
        (pos2(r.right() - radius, r.top() + radius), -90.0f32),
        (pos2(r.right() - radius, r.bottom() - radius), 0.0),
        (pos2(r.left() + radius, r.bottom() - radius), 90.0),
        (pos2(r.left() + radius, r.top() + radius), 180.0),
    ];
    for (c, start) in corners {
        for i in 0..=seg {
            let a = (start + 90.0 * i as f32 / seg as f32).to_radians();
            out.push(c + vec2(a.cos(), a.sin()) * radius);
        }
    }
}

/// A drop shadow that only exists outside `rect`: strongest under the bottom edge, faint at the
/// sides, almost none at the top (like CSS `0 24px 60px -24px`), as one gradient mesh.
pub fn soft_shadow(rect: Rect, radius: f32, color: Color32) -> Shape {
    const SEG: usize = 6;
    let (blur, drop) = (34.0, 16.0);
    let rings = [
        (rect, radius, 1.0),
        (
            rect.expand(blur * 0.35).translate(vec2(0.0, drop * 0.45)),
            radius + blur * 0.35,
            0.32,
        ),
        (
            rect.expand(blur).translate(vec2(0.0, drop)),
            radius + blur,
            0.0,
        ),
    ];
    let mut pts: Vec<Vec<Pos2>> = vec![];
    for (r, rad, _) in rings {
        let mut v = Vec::with_capacity(4 * (SEG + 1));
        rounded_outline(r, rad, SEG, &mut v);
        pts.push(v);
    }
    let n = pts[0].len();
    let mut m = Mesh::default();
    for (ring, (_, _, k)) in pts.iter().zip(rings) {
        for (i, p) in ring.iter().enumerate() {
            let t = ((pts[0][i].y - rect.top()) / rect.height().max(1.0)).clamp(0.0, 1.0);
            let side = 0.12 + 0.88 * t * t;
            m.colored_vertex(*p, color.gamma_multiply(k * side));
        }
    }
    for ring in 0..2u32 {
        let (a, b) = (ring * n as u32, (ring + 1) * n as u32);
        for i in 0..n as u32 {
            let j = (i + 1) % n as u32;
            m.add_triangle(a + i, b + i, a + j);
            m.add_triangle(a + j, b + i, b + j);
        }
    }
    Shape::mesh(m)
}

// ------------------------------------------------------------------ headings & text

pub fn title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(FontId::new(16.0, semibold()))
            .color(TEXT),
    );
}

/// Section header: 11 px uppercase, tracked, semibold, dim.
pub fn section(ui: &mut Ui, text: &str) {
    ui.add_space(5.0);
    ui.label(section_text(text));
}

fn section_text(text: &str) -> RichText {
    RichText::new(text.to_uppercase())
        .font(FontId::new(11.0, semibold()))
        .color(TEXT_DIM)
        .extra_letter_spacing(0.8)
}

/// Collapsible section: chevron + uppercase header (+ a muted note); open/closed is remembered per id.
/// Returns the body's result while open.
pub fn section_fold<R>(
    ui: &mut Ui,
    id_salt: impl Hash + std::fmt::Debug,
    title: &str,
    note: &str,
    body: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let id = ui.make_persistent_id(("section_fold", id_salt));
    let open = fold_header(ui, id, title, note);
    open.then(|| {
        ui.add_space(1.0);
        body(ui)
    })
}

/// The boxed variant (black 16 %, radius 13), used for "Generate a world" and "Erosion".
pub fn section_box<R>(
    ui: &mut Ui,
    id_salt: impl Hash + std::fmt::Debug,
    title: &str,
    body: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let id = ui.make_persistent_id(("section_box", id_salt));
    Frame::new()
        .fill(BOX)
        .corner_radius(CornerRadius::same(13))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let open = fold_header(ui, id, title, "");
            open.then(|| body(ui))
        })
        .inner
}

fn fold_header(ui: &mut Ui, id: Id, title: &str, note: &str) -> bool {
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, true));
    if !open && super::search::wants_open(ui, title) {
        open = true;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click());
    super::search::mark(ui, rect, title);
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    let p = ui.painter();
    let hover = resp.hovered();
    let col = if hover { TEXT } else { TEXT_DIM };
    let job = {
        let mut j = LayoutJob::default();
        j.append(
            &title.to_uppercase(),
            0.0,
            egui::TextFormat {
                font_id: FontId::new(11.0, semibold()),
                color: col,
                extra_letter_spacing: 0.8,
                ..Default::default()
            },
        );
        if !note.is_empty() {
            j.append(
                note,
                6.0,
                egui::TextFormat {
                    font_id: FontId::proportional(12.0),
                    color: TEXT_MUTE,
                    ..Default::default()
                },
            );
        }
        j.wrap.max_width = w - 22.0;
        j.wrap.max_rows = 1;
        j
    };
    let g = p.layout_job(job);
    p.galley(
        pos2(rect.left(), rect.center().y - g.size().y * 0.5),
        g,
        col,
    );
    let t = ui.ctx().animate_bool_responsive(id.with("chev"), open);
    chevron(
        p,
        pos2(rect.right() - 7.0, rect.center().y),
        3.5,
        egui::lerp(-90.0..=0.0, t),
        col.gamma_multiply(0.75),
    );
    open
}

/// A small "v" chevron, rotated by `deg` (0 = pointing down, -90 = pointing right).
pub fn chevron(painter: &Painter, c: Pos2, s: f32, deg: f32, color: Color32) {
    let rot = emath::Rot2::from_angle(deg.to_radians());
    let pts = [vec2(-s, -s * 0.5), vec2(0.0, s * 0.5), vec2(s, -s * 0.5)]
        .map(|v| c + rot * v)
        .to_vec();
    painter.add(Shape::line(pts, Stroke::new(1.4, color)));
}

/// A hint: 12.5 px, dim, wrapping.
pub fn dim(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(TEXT_DIM));
}

/// A small hint: 11.5 px, muted.
pub fn hint_small(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(TEXT_MUTE));
}

/// A warning line (amber).
pub fn warn_line(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(WARN));
}

/// A success line (green).
pub fn ok_line(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(OK));
}

pub fn separator(ui: &mut Ui) {
    ui.add_space(4.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, LINE);
    ui.add_space(4.0);
}

// ------------------------------------------------------------------ sliders

fn to_t(v: f64, lo: f64, hi: f64, log: bool) -> f64 {
    if hi <= lo {
        return 0.0;
    }
    let t = if log && lo > 0.0 {
        (v.max(lo).ln() - lo.ln()) / (hi.ln() - lo.ln())
    } else {
        (v - lo) / (hi - lo)
    };
    t.clamp(0.0, 1.0)
}

fn from_t(t: f64, lo: f64, hi: f64, log: bool) -> f64 {
    if log && lo > 0.0 {
        (lo.ln() + t * (hi.ln() - lo.ln())).exp()
    } else {
        lo + t * (hi - lo)
    }
}

const THUMB_R: f32 = 6.0;

/// The Halo slider track in `rect`: a 4 px rail, white up to the value and white 14 % after it,
/// a 12 px white thumb with a dark 3 px ring. Drag or click to set; arrow keys nudge it while
/// focused; values snap to "nice" numbers (and `step`) like egui's slider.
fn track_at(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    value: &mut f64,
    (lo, hi): (f64, f64),
    log: bool,
    step: Option<f64>,
) -> Response {
    let mut resp = ui.interact(rect, id, Sense::click_and_drag());
    let (x0, x1) = (rect.left() + THUMB_R, rect.right() - THUMB_R);
    let span = (x1 - x0).max(1.0);
    let snap = |v: f64| {
        let v = match step {
            Some(s) if s > 0.0 => lo + ((v - lo) / s).round() * s,
            _ => v,
        };
        v.clamp(lo.min(hi), hi.max(lo))
    };
    let mut new = None;
    if let Some(p) = resp.interact_pointer_pos() {
        let aim = ui.input(|i| i.aim_radius());
        let ta = (((p.x - aim) - x0) / span).clamp(0.0, 1.0) as f64;
        let tb = (((p.x + aim) - x0) / span).clamp(0.0, 1.0) as f64;
        let (a, b) = (from_t(ta, lo, hi, log), from_t(tb, lo, hi, log));
        new = Some(snap(emath::smart_aim::best_in_range_f64(
            a.min(b),
            a.max(b),
        )));
    }
    if resp.has_focus() {
        let (l, r) = ui.input(|i| {
            (
                i.num_presses(Key::ArrowLeft) + i.num_presses(Key::ArrowDown),
                i.num_presses(Key::ArrowRight) + i.num_presses(Key::ArrowUp),
            )
        });
        if l + r > 0 {
            let dt = (r as f64 - l as f64) * 0.01;
            let v = match step {
                Some(s) if s > 0.0 => *value + (r as f64 - l as f64) * s,
                _ => from_t(to_t(*value, lo, hi, log) + dt, lo, hi, log),
            };
            new = Some(snap(v));
        }
    }
    if let Some(v) = new
        && v != *value
    {
        *value = v;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let cy = rect.center().y;
        let cx = x0 + to_t(*value, lo, hi, log) as f32 * span;
        let rail = Rect::from_min_max(pos2(rect.left(), cy - 2.0), pos2(rect.right(), cy + 2.0));
        p.rect_filled(rail, CornerRadius::same(2), SOFT_HOVER);
        p.rect_filled(
            Rect::from_min_max(rail.min, pos2(cx, rail.max.y)),
            CornerRadius::same(2),
            Color32::WHITE,
        );
        let hot = resp.hovered() || resp.dragged();
        p.circle_filled(
            pos2(cx, cy),
            THUMB_R + 3.0,
            Color32::from_rgba_unmultiplied_const(10, 12, 16, 140),
        );
        p.circle_filled(
            pos2(cx, cy),
            if hot { THUMB_R + 0.5 } else { THUMB_R },
            Color32::WHITE,
        );
        if resp.has_focus() {
            p.circle_stroke(
                pos2(cx, cy),
                THUMB_R + 4.5,
                Stroke::new(1.0, Color32::from_white_alpha(120)),
            );
        }
    }
    resp.widget_info(|| WidgetInfo::slider(ui.is_enabled(), *value, ""));
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

struct RowLook {
    width: f32,
    label_size: f32,
    gap: f32,
}

fn slider_row_impl(
    ui: &mut Ui,
    look: RowLook,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    log: bool,
    fmt: &dyn Fn(f32) -> String,
) -> Response {
    let head_h = look.label_size + 4.0;
    let h = head_h + look.gap + 16.0;
    let (rect, row) = ui.allocate_exact_size(vec2(look.width, h), Sense::hover());
    super::search::mark(ui, rect, label);
    let track_rect = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 16.0), rect.max);
    let mut v = *value as f64;
    let track = track_at(
        ui,
        track_rect,
        row.id.with("track"),
        &mut v,
        (*range.start() as f64, *range.end() as f64),
        log,
        None,
    );
    if track.changed() {
        *value = v as f32;
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let vg = p.layout_no_wrap(
            fmt(*value),
            FontId::new(look.label_size - 1.0, mono()),
            TEXT,
        );
        let lg = one_line(
            p,
            label,
            FontId::proportional(look.label_size),
            TEXT_DIM,
            look.width - vg.size().x - 10.0,
        );
        let base = rect.top() + head_h * 0.5;
        p.galley(pos2(rect.left(), base - lg.size().y * 0.5), lg, TEXT_DIM);
        p.galley(
            pos2(rect.right() - vg.size().x, base - vg.size().y * 0.5),
            vg,
            TEXT,
        );
    }
    track.union(row)
}

/// Label left, mono value right, the Halo track underneath (full width).
pub fn slider_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    fmt: impl Fn(f32) -> String,
) -> Response {
    let w = ui.available_width();
    let r = slider_row_impl(
        ui,
        RowLook {
            width: w,
            label_size: 13.0,
            gap: 3.0,
        },
        label,
        value,
        range,
        false,
        &fmt,
    );
    ui.add_space(1.0);
    r
}

/// [`slider_row`] on a logarithmic scale (the range must start above zero).
pub fn slider_row_log(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    fmt: impl Fn(f32) -> String,
) -> Response {
    let w = ui.available_width();
    let r = slider_row_impl(
        ui,
        RowLook {
            width: w,
            label_size: 13.0,
            gap: 3.0,
        },
        label,
        value,
        range,
        true,
        &fmt,
    );
    ui.add_space(1.0);
    r
}

/// The dock's mini slider (116 px wide).
pub fn slider_mini(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    fmt: impl Fn(f32) -> String,
) -> Response {
    slider_row_impl(
        ui,
        RowLook {
            width: 116.0,
            label_size: 12.0,
            gap: 1.0,
        },
        label,
        value,
        range,
        false,
        &fmt,
    )
}

/// [`slider_mini`] on a logarithmic scale.
pub fn slider_mini_log(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    fmt: impl Fn(f32) -> String,
) -> Response {
    slider_row_impl(
        ui,
        RowLook {
            width: 116.0,
            label_size: 12.0,
            gap: 1.0,
        },
        label,
        value,
        range,
        true,
        &fmt,
    )
}

type Formatter<'a> = Box<dyn 'a + Fn(f64, RangeInclusive<usize>) -> String>;

/// Drop-in for `egui::Slider` in the Halo style: the track, then (unless hidden) the value as an
/// editable drag field and the label after it, as egui's slider lays them out.
pub struct TrackSlider<'a, T: emath::Numeric> {
    value: &'a mut T,
    range: RangeInclusive<T>,
    log: bool,
    step: Option<f64>,
    show_value: bool,
    text: Option<WidgetText>,
    suffix: String,
    formatter: Option<Formatter<'a>>,
}

impl<'a, T: emath::Numeric> TrackSlider<'a, T> {
    pub fn new(value: &'a mut T, range: RangeInclusive<T>) -> Self {
        Self {
            value,
            range,
            log: false,
            step: None,
            show_value: true,
            text: None,
            suffix: String::new(),
            formatter: None,
        }
    }
    pub fn step_by(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }
    pub fn logarithmic(mut self, log: bool) -> Self {
        self.log = log;
        self
    }
    pub fn show_value(mut self, show: bool) -> Self {
        self.show_value = show;
        self
    }
    pub fn text(mut self, text: impl Into<WidgetText>) -> Self {
        self.text = Some(text.into());
        self
    }
    pub fn suffix(mut self, suffix: impl ToString) -> Self {
        self.suffix = suffix.to_string();
        self
    }
    pub fn custom_formatter(
        mut self,
        f: impl 'a + Fn(f64, RangeInclusive<usize>) -> String,
    ) -> Self {
        self.formatter = Some(Box::new(f));
        self
    }
}

impl<T: emath::Numeric> egui::Widget for TrackSlider<'_, T> {
    fn ui(self, ui: &mut Ui) -> Response {
        let (lo, hi) = (self.range.start().to_f64(), self.range.end().to_f64());
        let step = self.step.or(T::INTEGRAL.then_some(1.0));
        let mut v = self.value.to_f64();
        let before = v;
        let inner = ui.horizontal(|ui| {
            let label_w = self.text.as_ref().map_or(0.0, |t| {
                ui.painter()
                    .layout_no_wrap(t.text().to_owned(), FontId::proportional(13.0), TEXT)
                    .size()
                    .x
                    + 8.0
            });
            let value_w = if self.show_value { 64.0 } else { 0.0 };
            let w = ui
                .spacing()
                .slider_width
                .min(ui.available_width() - label_w - value_w)
                .max(60.0);
            let (rect, base) =
                ui.allocate_exact_size(vec2(w, ui.spacing().interact_size.y), Sense::hover());
            let track_rect = Rect::from_center_size(rect.center(), vec2(w, 16.0));
            let mut r = track_at(
                ui,
                track_rect,
                base.id.with("track"),
                &mut v,
                (lo, hi),
                self.log,
                step,
            );
            if self.show_value {
                let speed = ((hi - lo).abs() / w.max(1.0) as f64).max(step.unwrap_or(0.0));
                let mut dv = egui::DragValue::new(&mut v)
                    .range(lo..=hi)
                    .speed(speed)
                    .suffix(self.suffix.clone());
                if let Some(f) = &self.formatter {
                    dv = dv.custom_formatter(|x, d| f(x, d));
                }
                r = r.union(ui.add(dv));
            }
            if let Some(t) = self.text {
                ui.label(t);
            }
            r
        });
        if v != before {
            *self.value = T::from_f64(v);
        }
        inner.inner
    }
}

// ------------------------------------------------------------------ segmented

fn segmented_impl<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
    solid: bool,
) -> bool {
    let opts: Vec<(T, &str, &str)> = options.iter().map(|(v, l)| (*v, "", *l)).collect();
    segmented_icons(ui, value, &opts, solid)
}

/// A segmented control whose items may carry an icon: `(value, icon or "", label)`.
pub fn segmented_icons<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str, &str)],
    solid: bool,
) -> bool {
    let mut changed = false;
    let w = ui.available_width();
    let n = options.len().max(1) as f32;
    let h = 26.0;
    let (rect, base) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(R_INPUT), WELL);
    let seg_w = (w - 4.0 - 2.0 * (n - 1.0)) / n;
    for (i, (v, icon, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(
            rect.min + Vec2::new(2.0 + i as f32 * (seg_w + 2.0), 2.0),
            Vec2::new(seg_w, h - 4.0),
        );
        let resp = ui
            .interact(r, base.id.with(i), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let selected = *value == *v;
        let p = ui.painter();
        if selected {
            if solid {
                p.rect_filled(r, CornerRadius::same(R_INNER), Color32::WHITE);
            } else {
                p.rect_filled(r, CornerRadius::same(R_INNER), SOFT_HOVER);
                p.rect_stroke(
                    r,
                    CornerRadius::same(R_INNER),
                    Stroke::new(1.0, Color32::from_white_alpha(15)),
                    StrokeKind::Inside,
                );
            }
        }
        let col = match (selected, solid) {
            (true, true) => ON_ACCENT,
            (true, false) => TEXT,
            _ if resp.hovered() => TEXT,
            _ => TEXT_DIM,
        };
        let iw = if icon.is_empty() { 0.0 } else { 18.0 };
        let g = one_line(p, label, FontId::proportional(12.0), col, seg_w - 8.0 - iw);
        let x = r.center().x - (g.size().x + iw) * 0.5;
        if !icon.is_empty() {
            paint_icon(ui, pos2(x + 6.5, r.center().y), icon, 13.0, col);
        }
        ui.painter()
            .galley(pos2(x + iw, r.center().y - g.size().y * 0.5), g, col);
        if resp.clicked() && !selected {
            *value = *v;
            changed = true;
        }
    }
    changed
}

/// Segmented control: a dark well; the "on" item is a soft white fill. Returns true if changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    segmented_impl(ui, value, options, false)
}

/// Tool-mode switch (Paint / Erase, Grow / Place one / Thin / Erase…): the "on" item is solid white.
pub fn segmented_solid<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    segmented_impl(ui, value, options, true)
}

// ------------------------------------------------------------------ toggle

/// A switch (replaces checkboxes): 26 × 15 track, white 16 % when off, white with a dark knob when on.
/// The label sits to the right; the whole row is clickable.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: impl Into<WidgetText>) -> Response {
    let label: WidgetText = label.into();
    let (tw, th, gap) = (26.0, 15.0, 8.0);
    let galley = (!label.is_empty()).then(|| {
        label.into_galley(
            ui,
            None,
            (ui.available_width() - tw - gap).max(20.0),
            TextStyle::Body,
        )
    });
    let text_h = galley.as_ref().map_or(0.0, |g| g.size().y);
    let size = vec2(
        tw + galley.as_ref().map_or(0.0, |g| gap + g.size().x),
        text_h.max(th) + 8.0,
    );
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let text = galley
        .as_ref()
        .map(|g| g.text().to_owned())
        .unwrap_or_default();
    super::search::mark(ui, rect, &text);
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, &text));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let track =
            Rect::from_min_size(pos2(rect.left(), rect.center().y - th * 0.5), vec2(tw, th));
        let p = ui.painter();
        let off = Color32::from_white_alpha(if resp.hovered() { 72 } else { 56 });
        p.rect_filled(track, CornerRadius::same(8), mix(off, Color32::WHITE, t));
        let kr = 5.5;
        let kx = egui::lerp((track.left() + 2.0 + kr)..=(track.right() - 2.0 - kr), t);
        p.circle_filled(
            pos2(kx, track.center().y),
            kr,
            mix(Color32::WHITE, ON_ACCENT, t),
        );
        if let Some(g) = galley {
            p.galley(
                pos2(track.right() + gap, rect.center().y - g.size().y * 0.5),
                g,
                TEXT,
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ dropdown

/// Halo dropdowns: build an `egui::ComboBox` as usual, then call `.dropdown_ui(ui, |ui| …)` instead of `.show_ui(..)`.
pub trait DropdownExt: Sized {
    /// Show this combo box as a dark well with an up/down chevron; the menu is a glass popup.
    fn dropdown_ui<R>(
        self,
        ui: &mut Ui,
        menu: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<Option<R>>;
}

impl DropdownExt for egui::ComboBox {
    fn dropdown_ui<R>(
        self,
        ui: &mut Ui,
        menu: impl FnOnce(&mut Ui) -> R,
    ) -> InnerResponse<Option<R>> {
        ui.scope(|ui| {
            well_style(ui);
            self.icon(paint_grabber)
                .popup_style(StyleModifier::new(menu_look))
                .show_ui(ui, menu)
        })
        .inner
    }
}

/// Dropdown fields: the dark well, white 10 % while hovered or open.
fn well_style(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.weak_bg_fill = WELL;
    for v in [&mut w.hovered, &mut w.active, &mut w.open] {
        v.weak_bg_fill = Color32::from_white_alpha(26);
    }
    for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        v.corner_radius = CornerRadius::same(8);
        v.bg_stroke = Stroke::NONE;
    }
    ui.spacing_mut().button_padding = vec2(9.0, 5.0);
}

/// Inside dropdown menus: frameless rows with a soft hover; the current item has a check mark
/// ([`menu_option`] / [`menu_check`]).
fn menu_look(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = vec2(9.0, 6.0);
    style.spacing.item_spacing.y = 0.0;
    style.visuals.selection.bg_fill = SOFT;
    let w = &mut style.visuals.widgets;
    w.hovered.weak_bg_fill = SOFT_HOVER;
    for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        v.corner_radius = CornerRadius::same(8);
    }
}

fn paint_grabber(ui: &Ui, rect: Rect, _v: &WidgetVisuals, _open: bool) {
    let c = rect.center();
    let col = TEXT_DIM.gamma_multiply(0.85);
    chevron(ui.painter(), c + vec2(0.0, -2.6), 3.0, 180.0, col);
    chevron(ui.painter(), c + vec2(0.0, 2.6), 3.0, 0.0, col);
}

/// A menu / dropdown item with a check mark on the current choice (sets `current` when clicked).
pub fn menu_option<T: PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    value: T,
    label: impl Into<WidgetText>,
) -> Response {
    let selected = *current == value;
    let resp = menu_item(ui, label, "", selected);
    if resp.clicked() && !selected {
        *current = value;
    }
    resp
}

/// A menu row with a check mark when `checked` (same arguments as `ui.selectable_label`).
pub fn menu_check(ui: &mut Ui, checked: bool, label: impl Into<WidgetText>) -> Response {
    menu_item(ui, label, "", checked)
}

/// A menu row: label, a right-aligned muted note or shortcut, a check mark when `checked`.
pub fn menu_item(ui: &mut Ui, label: impl Into<WidgetText>, note: &str, checked: bool) -> Response {
    menu_row(ui, "", label, note, checked)
}

/// A menu row with a leading icon (Central name or glyph; "" for none).
pub fn menu_row(
    ui: &mut Ui,
    icon: &str,
    label: impl Into<WidgetText>,
    note: &str,
    checked: bool,
) -> Response {
    let lead = if icon.is_empty() { 0.0 } else { 24.0 };
    let label: WidgetText = label.into();
    let galley = label.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        TextStyle::Body,
    );
    let note_g = (!note.is_empty()).then(|| {
        ui.painter()
            .layout_no_wrap(note.to_owned(), FontId::proportional(12.0), TEXT_MUTE)
    });
    let extra =
        note_g.as_ref().map_or(0.0, |g| g.size().x + 16.0) + if checked { 22.0 } else { 0.0 };
    // the natural width: menus are justified layouts, which stretch the row to the menu's width
    let mut resp = ui.allocate_response(
        vec2(lead + galley.size().x + 18.0 + extra, 28.0),
        Sense::click(),
    );
    let rect = resp.rect;
    if resp.clicked() {
        resp.mark_changed();
        ui.close();
    }
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), SOFT_HOVER);
    } else if checked {
        p.rect_filled(rect, CornerRadius::same(8), SOFT);
    }
    if lead > 0.0 {
        paint_icon(
            ui,
            pos2(rect.left() + 9.0 + 7.0, rect.center().y),
            icon,
            14.0,
            TEXT_DIM,
        );
    }
    let p = ui.painter();
    p.galley(
        pos2(
            rect.left() + 9.0 + lead,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        TEXT,
    );
    let mut right = rect.right() - 9.0;
    if checked {
        check_mark(p, pos2(right - 6.0, rect.center().y), 4.5, TEXT);
        right -= 22.0;
    }
    if let Some(g) = note_g {
        p.galley(
            pos2(right - g.size().x, rect.center().y - g.size().y * 0.5),
            g,
            TEXT_MUTE,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn check_mark(p: &Painter, c: Pos2, s: f32, color: Color32) {
    p.add(Shape::line(
        vec![
            c + vec2(-s, 0.1 * s),
            c + vec2(-0.3 * s, 0.8 * s),
            c + vec2(s, -0.8 * s),
        ],
        Stroke::new(1.6, color),
    ));
}

// ------------------------------------------------------------------ tool tiles

fn tile_fill(selected: bool, hovered: bool) -> Color32 {
    if selected {
        Color32::WHITE
    } else if hovered {
        SOFT_HOVER
    } else {
        SOFT
    }
}

/// Wide tool row: icon + name (one column), white when selected.
pub fn tool_row(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> Response {
    tool_row_key(ui, icon, label, "", selected)
}

/// [`tool_row`] with a keycap at the right.
pub fn tool_row_key(ui: &mut Ui, icon: &str, label: &str, key: &str, selected: bool) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(
            rect,
            CornerRadius::same(R_TILE),
            tile_fill(selected, resp.hovered()),
        );
        let (ic, fg) = if selected {
            (ON_ACCENT, ON_ACCENT)
        } else {
            (TEXT_DIM, TEXT)
        };
        paint_icon(
            ui,
            pos2(rect.left() + 18.0, rect.center().y),
            icon,
            15.0,
            ic,
        );
        let mut right = rect.right() - 8.0;
        if !key.is_empty() {
            right = paint_kbd(p, pos2(right, rect.center().y), key, selected) - 8.0;
        }
        let g = one_line(
            p,
            label,
            FontId::proportional(13.0),
            fg,
            right - rect.left() - 34.0,
        );
        p.galley(
            pos2(rect.left() + 34.0, rect.center().y - g.size().y * 0.5),
            g,
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Square tile with the icon over a caption (tool grids of 3–4), white when selected.
pub fn tool_tile(ui: &mut Ui, icon: &str, label: &str, selected: bool, size: Vec2) -> Response {
    tool_tile_key(ui, icon, label, "", selected, size)
}

/// [`tool_tile`] with its keycap in the corner (shown only on the selected tile).
pub fn tool_tile_key(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    key: &str,
    selected: bool,
    size: Vec2,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        p.rect_filled(
            rect,
            CornerRadius::same(R_TILE),
            tile_fill(selected, resp.hovered()),
        );
        let (ic, fg) = if selected {
            (ON_ACCENT, ON_ACCENT)
        } else {
            (TEXT_DIM, TEXT)
        };
        let g = centred_rows(p, label, FontId::proportional(11.5), fg, size.x - 8.0, 2);
        let gap = 5.0;
        let icon_h = 17.0;
        let top = rect.center().y - (icon_h + gap + g.size().y) * 0.5;
        paint_icon(
            ui,
            pos2(rect.center().x, top + icon_h * 0.5),
            icon,
            16.0,
            ic,
        );
        paint_centred(p, g, rect.center().x, top + icon_h + gap, fg);
        if selected && !key.is_empty() {
            paint_kbd_small(p, pos2(rect.right() - 5.0, rect.top() + 5.0), key);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Colour / picture swatch tile with a caption underneath; a 2 px white ring when picked.
pub fn swatch_tile(
    ui: &mut Ui,
    color: Color32,
    texture: Option<TextureId>,
    label: &str,
    selected: bool,
    size: Vec2,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let img = Rect::from_min_size(rect.min, Vec2::new(size.x, size.y - 18.0));
    if let Some(t) = texture {
        egui::Image::new((t, img.size()))
            .corner_radius(CornerRadius::same(8))
            .paint_at(ui, img);
    } else {
        ui.painter().rect_filled(img, CornerRadius::same(8), color);
    }
    let p = ui.painter();
    if selected {
        p.rect_stroke(
            img,
            CornerRadius::same(8),
            Stroke::new(2.0, Color32::WHITE),
            StrokeKind::Outside,
        );
    } else if resp.hovered() {
        p.rect_stroke(
            img,
            CornerRadius::same(8),
            Stroke::new(1.0, Color32::from_white_alpha(90)),
            StrokeKind::Outside,
        );
    }
    let col = if selected { TEXT } else { TEXT_DIM };
    let g = one_line(p, label, FontId::proportional(11.5), col, size.x);
    p.galley(
        pos2(
            rect.center().x - g.size().x * 0.5,
            rect.bottom() - 7.0 - g.size().y * 0.5,
        ),
        g,
        col,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A round material swatch with its name underneath (two lines at most); a white ring with a dark gap when picked.
pub fn circle_swatch(
    ui: &mut Ui,
    texture: Option<TextureId>,
    color: Color32,
    label: &str,
    selected: bool,
    d: f32,
) -> Response {
    let w = d + 14.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, d + 24.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let c = pos2(rect.center().x, rect.top() + 5.0 + d * 0.5);
    let disc = Rect::from_center_size(c, Vec2::splat(d));
    match texture {
        Some(t) => {
            egui::Image::new((t, disc.size()))
                .corner_radius(CornerRadius::same(255))
                .paint_at(ui, disc);
        }
        None => {
            ui.painter().circle_filled(c, d * 0.5, color);
        }
    }
    let p = ui.painter();
    if selected {
        p.circle_stroke(
            c,
            d * 0.5 + 1.0,
            Stroke::new(2.0, Color32::from_rgba_unmultiplied_const(10, 12, 16, 230)),
        );
        p.circle_stroke(c, d * 0.5 + 3.0, Stroke::new(2.0, Color32::WHITE));
    } else if resp.hovered() {
        p.circle_stroke(
            c,
            d * 0.5 + 2.0,
            Stroke::new(1.5, Color32::from_white_alpha(110)),
        );
    }
    let col = if selected { TEXT } else { TEXT_DIM };
    let g = one_line(p, label, FontId::proportional(11.0), col, w + 4.0);
    p.galley(
        pos2(rect.center().x - g.size().x * 0.5, disc.bottom() + 6.0),
        g,
        col,
    );
    resp.on_hover_text(label)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ cards

/// Cached gradient textures for cards (the radial image well and the caption fade).
fn gradient_texture(ctx: &egui::Context, radial: bool) -> TextureId {
    let id = Id::new(("halo_gradient", radial));
    if let Some(h) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return h.id();
    }
    let img = if radial {
        // white 10 % at (50 %, 60 %) fading to 3 % at the rim
        let n = 64;
        let px: Vec<Color32> = (0..n * n)
            .map(|i| {
                let (x, y) = (
                    (i % n) as f32 / (n - 1) as f32,
                    (i / n) as f32 / (n - 1) as f32,
                );
                let d = ((x - 0.5).powi(2) + (y - 0.6).powi(2)).sqrt() / 0.62;
                Color32::from_white_alpha(
                    (255.0 * egui::lerp(0.10..=0.03, d.clamp(0.0, 1.0))).round() as u8,
                )
            })
            .collect();
        egui::ColorImage::new([n, n], px)
    } else {
        // transparent → black 75 % (caption fade)
        let n = 48;
        let px: Vec<Color32> = (0..n)
            .map(|i| {
                Color32::from_black_alpha((191.0 * (i as f32 / (n - 1) as f32).powf(1.3)) as u8)
            })
            .collect();
        egui::ColorImage::new([1, n], px)
    };
    let h = ctx.load_texture(
        if radial { "halo_radial" } else { "halo_fade" },
        img,
        egui::TextureOptions::LINEAR,
    );
    let tid = h.id();
    ctx.data_mut(|d| d.insert_temp(id, h));
    tid
}

/// The radial image well of a card (white 10 % → 3 %), a touch brighter when hovered.
pub fn card_well(ui: &Ui, rect: Rect, hovered: bool) {
    let radial = gradient_texture(ui.ctx(), true);
    let tint = if hovered {
        Color32::from_white_alpha(255)
    } else {
        Color32::from_white_alpha(210)
    };
    egui::Image::new((radial, rect.size()))
        .tint(tint)
        .corner_radius(CornerRadius::same(8))
        .paint_at(ui, rect);
}

/// Asset card: a square image well (subtle radial glow), the name below. A 2 px white ring when
/// selected, a lock badge top-right, a white `FREE` tag top-left.
pub fn asset_card(
    ui: &mut Ui,
    image: Option<TextureId>,
    name: &str,
    selected: bool,
    width: f32,
    locked: bool,
    free: bool,
) -> Response {
    let well = width - 6.0;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(width, 4.0 + well + 4.0 + 14.0 + 6.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let wr = Rect::from_min_size(rect.min + vec2(3.0, 4.0), Vec2::splat(well));
    let radial = gradient_texture(ui.ctx(), true);
    egui::Image::new((radial, wr.size()))
        .corner_radius(CornerRadius::same(8))
        .paint_at(ui, wr);
    if let Some(t) = image {
        let ts = texture_size(ui.ctx(), t).unwrap_or(Vec2::splat(1.0));
        let scale = if resp.hovered() { 0.93 } else { 0.88 };
        let fit = wr.size() * scale;
        let k = (fit.x / ts.x.max(1.0)).min(fit.y / ts.y.max(1.0));
        egui::Image::new((t, ts * k)).paint_at(ui, Rect::from_center_size(wr.center(), ts * k));
    }
    let p = ui.painter();
    if selected {
        p.rect_stroke(
            wr,
            CornerRadius::same(8),
            Stroke::new(2.0, Color32::WHITE),
            StrokeKind::Outside,
        );
    }
    if locked {
        let b = Rect::from_min_size(pos2(wr.right() - 23.0, wr.top() + 5.0), Vec2::splat(18.0));
        p.rect_filled(b, CornerRadius::same(6), Color32::from_black_alpha(140));
        p.text(
            b.center(),
            Align2::CENTER_CENTER,
            egui_phosphor::regular::LOCK_SIMPLE,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
    if free {
        let g = p.layout_no_wrap("FREE".into(), FontId::new(9.0, mono_semibold()), ON_ACCENT);
        let b = Rect::from_min_size(
            pos2(wr.left() + 5.0, wr.top() + 5.0),
            g.size() + vec2(10.0, 2.0),
        );
        p.rect_filled(b, CornerRadius::same(5), Color32::WHITE);
        p.galley(b.center() - g.size() * 0.5, g, ON_ACCENT);
    }
    let col = if selected { TEXT } else { TEXT_DIM };
    let g = one_line(p, name, FontId::proportional(11.5), col, width - 4.0);
    p.galley(
        pos2(rect.center().x - g.size().x * 0.5, wr.bottom() + 4.0),
        g,
        col,
    );
    resp.on_hover_text(name)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Photo card (generate presets, looks, world art, map styles): the picture cropped to fill,
/// a bottom fade with the caption (+ a smaller second line); a white ring and check dot when picked.
pub fn image_card(
    ui: &mut Ui,
    image: Option<TextureId>,
    caption: &str,
    sub: &str,
    selected: bool,
    size: Vec2,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let rad = CornerRadius::same(11);
    match image {
        Some(t) => {
            // cover: crop the picture to the card's aspect (a touch more when hovered)
            let ts = texture_size(ui.ctx(), t).unwrap_or(size);
            let (ca, ia) = (size.x / size.y, ts.x / ts.y.max(1.0));
            let mut uv = if ia > ca {
                Rect::from_center_size(pos2(0.5, 0.5), vec2(ca / ia, 1.0))
            } else {
                Rect::from_center_size(pos2(0.5, 0.5), vec2(1.0, ia / ca))
            };
            if resp.hovered() {
                uv = uv.shrink2(uv.size() * 0.02);
            }
            egui::Image::new((t, size))
                .uv(uv)
                .corner_radius(rad)
                .paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, rad, SOFT);
        }
    }
    let p = ui.painter();
    let cg = one_line(
        p,
        caption,
        FontId::new(12.0, semibold()),
        Color32::WHITE,
        size.x - 16.0,
    );
    let sg = (!sub.is_empty()).then(|| {
        one_line(
            p,
            sub,
            FontId::proportional(10.5),
            Color32::from_white_alpha(190),
            size.x - 16.0,
        )
    });
    let text_h = cg.size().y + sg.as_ref().map_or(0.0, |g| g.size().y);
    let fade_h = (text_h + 24.0).min(size.y);
    let fade = Rect::from_min_max(pos2(rect.left(), rect.bottom() - fade_h), rect.max);
    let fade_tex = gradient_texture(ui.ctx(), false);
    egui::Image::new((fade_tex, fade.size()))
        .corner_radius(CornerRadius {
            nw: 0,
            ne: 0,
            sw: 11,
            se: 11,
        })
        .paint_at(ui, fade);
    let p = ui.painter();
    let mut y = rect.bottom() - 6.0 - text_h;
    let cy = cg.size().y;
    p.galley(pos2(rect.left() + 8.0, y), cg, Color32::WHITE);
    y += cy;
    if let Some(g) = sg {
        p.galley(pos2(rect.left() + 8.0, y), g, Color32::WHITE);
    }
    if selected {
        p.rect_stroke(
            rect,
            rad,
            Stroke::new(2.0, Color32::WHITE),
            StrokeKind::Outside,
        );
        let c = pos2(rect.right() - 15.0, rect.top() + 15.0);
        p.circle_filled(c, 8.0, Color32::WHITE);
        check_mark(p, c, 3.4, ON_ACCENT);
    } else if resp.hovered() {
        p.rect_stroke(
            rect,
            rad,
            Stroke::new(1.0, Color32::from_white_alpha(80)),
            StrokeKind::Outside,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A square card well (the image or icon area of a card) with its caption below.
fn card_frame(ui: &mut Ui, name: &str, selected: bool, width: f32) -> (Response, Rect) {
    let well = width - 6.0;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(width, 4.0 + well + 4.0 + 14.0 + 6.0), Sense::click());
    let wr = Rect::from_min_size(rect.min + vec2(3.0, 4.0), Vec2::splat(well));
    if ui.is_rect_visible(rect) {
        let col = if selected { TEXT } else { TEXT_DIM };
        let p = ui.painter();
        let g = one_line(p, name, FontId::proportional(11.5), col, width - 4.0);
        p.galley(
            pos2(rect.center().x - g.size().x * 0.5, wr.bottom() + 4.0),
            g,
            col,
        );
    }
    (resp, wr)
}

/// A card with a photo cropped to a square (generate shapes, crops): the name below, a white ring when picked.
pub fn photo_card(
    ui: &mut Ui,
    image: Option<TextureId>,
    name: &str,
    selected: bool,
    width: f32,
) -> Response {
    let (resp, wr) = card_frame(ui, name, selected, width);
    if ui.is_rect_visible(wr) {
        match image {
            Some(t) => {
                let ts = texture_size(ui.ctx(), t).unwrap_or(Vec2::splat(1.0));
                let a = ts.x / ts.y.max(1.0);
                let uv = if a > 1.0 {
                    Rect::from_center_size(pos2(0.5, 0.5), vec2(1.0 / a, 1.0))
                } else {
                    Rect::from_center_size(pos2(0.5, 0.5), vec2(1.0, a))
                };
                egui::Image::new((t, wr.size()))
                    .uv(uv)
                    .corner_radius(CornerRadius::same(8))
                    .paint_at(ui, wr);
            }
            None => {
                let radial = gradient_texture(ui.ctx(), true);
                egui::Image::new((radial, wr.size()))
                    .corner_radius(CornerRadius::same(8))
                    .paint_at(ui, wr);
            }
        }
        let p = ui.painter();
        if selected {
            p.rect_stroke(
                wr,
                CornerRadius::same(8),
                Stroke::new(2.0, Color32::WHITE),
                StrokeKind::Outside,
            );
        } else if resp.hovered() {
            p.rect_stroke(
                wr,
                CornerRadius::same(8),
                Stroke::new(1.0, Color32::from_white_alpha(90)),
                StrokeKind::Outside,
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A card with a big icon in the well (erosion presets, kinds): the name below, a white ring when picked.
pub fn icon_card(ui: &mut Ui, icon: &str, name: &str, selected: bool, width: f32) -> Response {
    let (resp, wr) = card_frame(ui, name, selected, width);
    if ui.is_rect_visible(wr) {
        let radial = gradient_texture(ui.ctx(), true);
        egui::Image::new((radial, wr.size()))
            .corner_radius(CornerRadius::same(8))
            .paint_at(ui, wr);
        paint_icon(
            ui,
            wr.center(),
            icon,
            22.0,
            if selected || resp.hovered() {
                TEXT
            } else {
                TEXT_DIM
            },
        );
        if selected {
            ui.painter().rect_stroke(
                wr,
                CornerRadius::same(8),
                Stroke::new(2.0, Color32::WHITE),
                StrokeKind::Outside,
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A grid of cards, `cols` across with a 5 px gap; `card(ui, i, width)` draws card `i`. Returns the clicked index.
pub fn card_grid(
    ui: &mut Ui,
    cols: usize,
    n: usize,
    mut card: impl FnMut(&mut Ui, usize, f32) -> Response,
) -> Option<usize> {
    let cols = cols.max(1);
    let w = ((ui.available_width() - 5.0 * (cols as f32 - 1.0)) / cols as f32).floor();
    let mut out = None;
    let mut i = 0;
    while i < n {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            for _ in 0..cols {
                if i >= n {
                    break;
                }
                if card(ui, i, w).clicked() {
                    out = Some(i);
                }
                i += 1;
            }
        });
    }
    out
}

/// A tool grid: two columns as icon + name rows, three or four columns as icons over captions.
/// `items`: (icon, name, tooltip). Returns the clicked index.
pub fn tool_grid(
    ui: &mut Ui,
    cols: usize,
    items: &[(&str, &str, &str)],
    selected: Option<usize>,
) -> Option<usize> {
    let cols = cols.max(1);
    let w = ((ui.available_width() - 5.0 * (cols as f32 - 1.0)) / cols as f32).floor();
    let mut out = None;
    for row in items.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            for (k, (icon, name, tipt)) in row.1.iter().enumerate() {
                let i = row.0 * cols + k;
                let sel = selected == Some(i);
                let r = if cols <= 2 {
                    tool_cell(ui, icon, name, sel, w)
                } else {
                    tool_tile(ui, icon, name, sel, vec2(w, 58.0))
                };
                let r = if tipt.is_empty() {
                    r
                } else {
                    r.on_hover_text(*tipt)
                };
                if r.clicked() {
                    out = Some(i);
                }
            }
        });
    }
    out
}

/// One cell of a two-column tool grid: icon + name, white when selected.
fn tool_cell(ui: &mut Ui, icon: &str, label: &str, selected: bool, w: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(R_TILE),
            tile_fill(selected, resp.hovered()),
        );
        let (ic, fg) = if selected {
            (ON_ACCENT, ON_ACCENT)
        } else {
            (TEXT_DIM, TEXT)
        };
        paint_icon(
            ui,
            pos2(rect.left() + 16.0, rect.center().y),
            icon,
            15.0,
            ic,
        );
        let g = one_line(
            ui.painter(),
            label,
            FontId::proportional(13.0),
            fg,
            w - 36.0,
        );
        ui.painter().galley(
            pos2(rect.left() + 31.0, rect.center().y - g.size().y * 0.5),
            g,
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Label (dim) + a full-width dropdown on one row.
pub fn dropdown_row<R>(
    ui: &mut Ui,
    label: &str,
    id_salt: impl Hash + std::fmt::Debug,
    selected: impl Into<WidgetText>,
    menu: impl FnOnce(&mut Ui) -> R,
) -> Response {
    let r = ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT_DIM));
        egui::ComboBox::from_id_salt(id_salt)
            .width(ui.available_width())
            .selected_text(selected)
            .dropdown_ui(ui, menu)
            .response
    });
    super::search::mark(ui, r.response.rect, label);
    r.inner
}

// ------------------------------------------------------------------ chips

/// A pill button with an optional mono count; white when on.
pub fn chip(ui: &mut Ui, label: &str, count: Option<usize>, on: bool) -> Response {
    let p = ui.painter();
    let (fg, cnt) = if on {
        (ON_ACCENT, Color32::from_black_alpha(128))
    } else {
        (TEXT_DIM, TEXT_MUTE)
    };
    let lg = p.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), fg);
    let ng = count.map(|n| p.layout_no_wrap(n.to_string(), FontId::new(11.0, mono()), cnt));
    let w = 16.0 + lg.size().x + ng.as_ref().map_or(0.0, |g| 6.0 + g.size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let fg = if !on && resp.hovered() { TEXT } else { fg };
        if on {
            p.rect_filled(rect, CornerRadius::same(255), Color32::WHITE);
        } else {
            p.rect_stroke(
                rect,
                CornerRadius::same(255),
                Stroke::new(1.0, LINE),
                StrokeKind::Inside,
            );
        }
        let x = rect.left() + 8.0;
        p.galley(pos2(x, rect.center().y - lg.size().y * 0.5), lg.clone(), fg);
        if let Some(g) = ng {
            p.galley(
                pos2(x + lg.size().x + 6.0, rect.center().y - g.size().y * 0.5),
                g,
                cnt,
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A wrapping row of [`chip`]s acting as one choice. Returns true if changed.
pub fn chips<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str, Option<usize>)],
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (v, label, n) in options {
            if chip(ui, label, *n, *value == *v).clicked() && *value != *v {
                *value = *v;
                changed = true;
            }
        }
    });
    changed
}

// ------------------------------------------------------------------ list rows & stats

/// What leads a [`list_row`].
pub enum Lead<'a> {
    None,
    Icon(&'a str),
    Dot(Color32),
    Image(TextureId),
    /// a round material swatch
    Swatch(TextureId),
}

/// A list row: icon / colour dot / thumbnail, title + muted subtitle, mono meta on the right.
pub fn list_row(
    ui: &mut Ui,
    lead: Lead,
    title: &str,
    subtitle: &str,
    meta: &str,
    selected: bool,
) -> Response {
    let w = ui.available_width();
    let h = if subtitle.is_empty() { 30.0 } else { 42.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_INPUT), SOFT_HOVER);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_INPUT), SOFT);
    }
    let cy = rect.center().y;
    let mut x = rect.left() + 7.0;
    match lead {
        Lead::None => {}
        Lead::Icon(ic) => {
            paint_icon(ui, pos2(x + 8.0, cy), ic, 15.0, TEXT_DIM);
            x += 24.0;
        }
        Lead::Dot(c) => {
            ui.painter().rect_filled(
                Rect::from_center_size(pos2(x + 5.0, cy), Vec2::splat(10.0)),
                CornerRadius::same(3),
                c,
            );
            x += 18.0;
        }
        Lead::Image(t) => {
            egui::Image::new((t, Vec2::splat(26.0))).paint_at(
                ui,
                Rect::from_center_size(pos2(x + 13.0, cy), Vec2::splat(26.0)),
            );
            x += 34.0;
        }
        Lead::Swatch(t) => {
            egui::Image::new((t, Vec2::splat(22.0)))
                .corner_radius(CornerRadius::same(255))
                .paint_at(
                    ui,
                    Rect::from_center_size(pos2(x + 11.0, cy), Vec2::splat(22.0)),
                );
            x += 30.0;
        }
    }
    let p = ui.painter();
    let mut right = rect.right() - 7.0;
    if !meta.is_empty() {
        let g = p.layout_no_wrap(meta.to_owned(), FontId::new(11.0, mono()), TEXT_MUTE);
        right -= g.size().x;
        p.galley(pos2(right, cy - g.size().y * 0.5), g, TEXT_MUTE);
        right -= 8.0;
    }
    let tg = one_line(p, title, FontId::new(13.0, medium()), TEXT, right - x);
    if subtitle.is_empty() {
        p.galley(pos2(x, cy - tg.size().y * 0.5), tg, TEXT);
    } else {
        let sg = one_line(
            p,
            subtitle,
            FontId::proportional(11.5),
            TEXT_MUTE,
            right - x,
        );
        let top = cy - (tg.size().y + sg.size().y) * 0.5;
        let th = tg.size().y;
        p.galley(pos2(x, top), tg, TEXT);
        p.galley(pos2(x, top + th), sg, TEXT_MUTE);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Label left (dim), mono value right.
pub fn stat_row(ui: &mut Ui, label: &str, value: &str) -> Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    super::search::mark(ui, rect, label);
    let p = ui.painter();
    let vg = p.layout_no_wrap(value.to_owned(), FontId::new(12.0, mono()), TEXT);
    let lg = one_line(
        p,
        label,
        FontId::proportional(13.0),
        TEXT_DIM,
        w - vg.size().x - 10.0,
    );
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
        TEXT,
    );
    resp
}

/// [`stat_row`] with a 6 px bar underneath (`frac` 0–1 of the bar in `color`).
pub fn bar_stat(ui: &mut Ui, label: &str, value: &str, frac: f32, color: Color32) -> Response {
    let r = ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        let r = stat_row(ui, label, value);
        let w = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(bar, CornerRadius::same(3), Color32::from_white_alpha(26));
        let f = frac.clamp(0.0, 1.0);
        if f > 0.0 {
            p.rect_filled(
                Rect::from_min_size(bar.min, vec2((w * f).max(6.0), 6.0)),
                CornerRadius::same(3),
                color,
            );
        }
        r
    });
    r.inner
}

// ------------------------------------------------------------------ buttons

#[derive(Clone, Copy, PartialEq)]
enum ButtonKind {
    Soft,
    Primary,
    Small,
}

fn button_impl(ui: &mut Ui, text: &str, kind: ButtonKind, width: Option<f32>) -> Response {
    button_icon_impl(ui, "", text, kind, width)
}

fn button_icon_impl(
    ui: &mut Ui,
    icon: &str,
    text: &str,
    kind: ButtonKind,
    width: Option<f32>,
) -> Response {
    let (size, h, pad, rad) = match kind {
        ButtonKind::Small => (12.0, 24.0, 8.0, R_SMALL),
        _ => (13.0, 30.0, 11.0, R_BUTTON),
    };
    let fg = if kind == ButtonKind::Primary {
        ON_ACCENT
    } else {
        TEXT
    };
    let font = FontId::new(size, semibold());
    let icon_w = if icon.is_empty() { 0.0 } else { size + 6.0 };
    let natural = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), fg)
        .size()
        .x
        + 2.0 * pad
        + icon_w;
    let w = width.unwrap_or(natural);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    super::search::mark(ui, rect, text);
    if ui.is_rect_visible(rect) {
        let down = resp.is_pointer_button_down_on();
        let fill = match (kind, resp.hovered(), down) {
            (ButtonKind::Primary, _, true) => Color32::from_rgb(214, 220, 228),
            (ButtonKind::Primary, true, _) => ACCENT_HOVER,
            (ButtonKind::Primary, ..) => Color32::WHITE,
            (_, _, true) => Color32::from_white_alpha(46),
            (_, true, _) => SOFT_HOVER,
            _ => SOFT,
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(rad), fill);
        let g = one_line(p, text, font, fg, w - 2.0 * pad - icon_w);
        let x = rect.center().x - (g.size().x + icon_w) * 0.5;
        if !icon.is_empty() {
            paint_icon(
                ui,
                pos2(x + size * 0.5, rect.center().y),
                icon,
                size + 1.0,
                fg,
            );
        }
        ui.painter()
            .galley(pos2(x + icon_w, rect.center().y - g.size().y * 0.5), g, fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The white call-to-action button with an icon, full width.
pub fn primary_icon_button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let w = ui.available_width();
    button_icon_impl(ui, icon, text, ButtonKind::Primary, Some(w))
}

/// A soft button with an icon, full width.
pub fn wide_icon_button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    let w = ui.available_width();
    button_icon_impl(ui, icon, text, ButtonKind::Soft, Some(w))
}

/// A soft button with an icon, sized to its text (`width` to fix it).
pub fn icon_text_button(ui: &mut Ui, icon: &str, text: &str, width: Option<f32>) -> Response {
    button_icon_impl(ui, icon, text, ButtonKind::Soft, width)
}

/// A small soft button with an icon.
pub fn small_icon_button(ui: &mut Ui, icon: &str, text: &str) -> Response {
    button_icon_impl(ui, icon, text, ButtonKind::Small, None)
}

/// A soft button sized to its text.
pub fn button(ui: &mut Ui, text: &str) -> Response {
    button_impl(ui, text, ButtonKind::Soft, None)
}

/// A small soft button (24 px).
pub fn small_button(ui: &mut Ui, text: &str) -> Response {
    button_impl(ui, text, ButtonKind::Small, None)
}

/// The white call-to-action button, full width.
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    let w = ui.available_width();
    button_impl(ui, text, ButtonKind::Primary, Some(w))
}

/// The white call-to-action button, sized to its text.
pub fn primary_button_fit(ui: &mut Ui, text: &str) -> Response {
    button_impl(ui, text, ButtonKind::Primary, None)
}

/// A soft button, full width.
pub fn wide_button(ui: &mut Ui, text: &str) -> Response {
    let w = ui.available_width();
    button_impl(ui, text, ButtonKind::Soft, Some(w))
}

/// A 28 px icon button: dim icon, soft fill and bright icon on hover.
pub fn icon_button(ui: &mut Ui, icon: &str, tip_text: &str) -> Response {
    icon_toggle(ui, icon, tip_text, false)
}

/// [`icon_button`] that shows an "on" state (white 14 % fill, bright icon).
pub fn icon_toggle(ui: &mut Ui, icon: &str, tip_text: &str, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hot = resp.hovered();
        if on {
            p.rect_filled(rect, CornerRadius::same(8), SOFT_HOVER);
        } else if hot {
            p.rect_filled(rect, CornerRadius::same(8), SOFT);
        }
        paint_icon(
            ui,
            rect.center(),
            icon,
            15.0,
            if hot || on { TEXT } else { TEXT_DIM },
        );
    }
    let (title, key) = split_key(tip_text);
    tip(resp, title, "", key, TipSide::Below).on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ keycaps & fields

fn kbd_galley(p: &Painter, key: &str, size: f32, color: Color32) -> Arc<Galley> {
    p.layout_no_wrap(key.to_owned(), FontId::new(size, mono()), color)
}

/// Paint a keycap whose right edge is at `right_center`; returns its left edge.
fn paint_kbd(p: &Painter, right_center: Pos2, key: &str, on_white: bool) -> f32 {
    let col = if on_white {
        Color32::from_black_alpha(140)
    } else {
        TEXT_DIM
    };
    let g = kbd_galley(p, key, 10.5, col);
    let size = vec2((g.size().x + 8.0).max(g.size().y + 4.0), g.size().y + 4.0);
    let r = Rect::from_min_size(
        pos2(right_center.x - size.x, right_center.y - size.y * 0.5),
        size,
    );
    if on_white {
        p.rect_stroke(
            r,
            CornerRadius::same(4),
            Stroke::new(1.0, Color32::from_black_alpha(46)),
            StrokeKind::Inside,
        );
    } else {
        p.rect_filled(r, CornerRadius::same(4), Color32::from_black_alpha(46));
        p.rect_stroke(
            r,
            CornerRadius::same(4),
            Stroke::new(1.0, LINE_STRONG),
            StrokeKind::Inside,
        );
    }
    p.galley(r.center() - g.size() * 0.5, g, col);
    r.left()
}

/// The small corner keycap of a selected grid tile (dark on white).
fn paint_kbd_small(p: &Painter, right_top: Pos2, key: &str) {
    let col = Color32::from_black_alpha(140);
    let g = kbd_galley(p, key, 9.5, col);
    let size = vec2(g.size().x + 6.0, g.size().y + 2.0);
    let r = Rect::from_min_size(pos2(right_top.x - size.x, right_top.y), size);
    p.rect_stroke(
        r,
        CornerRadius::same(4),
        Stroke::new(1.0, Color32::from_black_alpha(46)),
        StrokeKind::Inside,
    );
    p.galley(r.center() - g.size() * 0.5, g, col);
}

/// A keycap alone (10.5 px mono, 1 px rim, radius 4).
pub fn kbd(ui: &mut Ui, key: &str) -> Response {
    let g = kbd_galley(ui.painter(), key, 10.5, TEXT_DIM);
    let size = vec2((g.size().x + 8.0).max(g.size().y + 4.0), g.size().y + 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_kbd(
        ui.painter(),
        pos2(rect.right(), rect.center().y),
        key,
        false,
    );
    resp
}

/// Keycap + what it does (key hints).
pub fn keycap(ui: &mut Ui, key: &str, action: &str) {
    kbd(ui, key);
    ui.label(RichText::new(action).size(12.0).color(TEXT_DIM));
    ui.add_space(8.0);
}

/// A search field: dark well with a magnifier, no frame.
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str) -> Response {
    Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(R_INPUT))
        .inner_margin(Margin {
            left: 10,
            right: 8,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_height(30.0);
                let (icon_rect, _) = ui.allocate_exact_size(vec2(16.0, 30.0), Sense::hover());
                paint_icon(ui, icon_rect.center(), "MagnifyingGlass", 15.0, TEXT_MUTE);
                ui.add(
                    egui::TextEdit::singleline(text)
                        .frame(Frame::NONE)
                        .hint_text(RichText::new(hint).color(TEXT_MUTE))
                        .desired_width(ui.available_width()),
                )
            })
            .inner
        })
        .inner
}

pub fn color32(c: [f32; 3]) -> Color32 {
    Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    )
}

// ------------------------------------------------------------------ dock pieces

/// The dock's tool title: a white icon and the tool name.
pub fn dock_title(ui: &mut Ui, icon: &str, name: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(16.0, 28.0), Sense::hover());
    paint_icon(ui, r.center(), icon, 15.0, Color32::WHITE);
    ui.label(
        RichText::new(name)
            .font(FontId::new(13.0, semibold()))
            .color(TEXT),
    );
}

/// A divider between dock groups.
pub fn dock_sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 30.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, LINE);
}

/// A line of dim text in the dock.
pub fn dock_text(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(TEXT_DIM));
}

/// Key hints in the dock: keycap + what it does.
pub fn dock_hints(ui: &mut Ui, hints: &[(&str, &str)]) {
    for (k, l) in hints {
        kbd(ui, k);
        ui.label(RichText::new(*l).size(12.0).color(TEXT_DIM));
        ui.add_space(2.0);
    }
}

// ------------------------------------------------------------------ dev gallery

#[derive(Clone)]
struct GalleryState {
    v: [f32; 4],
    seg: u8,
    tool: u8,
    on: [bool; 3],
    pick: u8,
    chip: u8,
    row: u8,
    text: String,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            v: [0.42, 120.0, 0.6, 35.0],
            seg: 0,
            tool: 1,
            on: [true, false, true],
            pick: 1,
            chip: 0,
            row: 1,
            text: String::new(),
        }
    }
}

fn gradient_lead(
    ctx: &egui::Context,
) -> Option<(Lead<'static>, &'static str, &'static str, &'static str)> {
    Some((
        Lead::Image(gradient_texture(ctx, true)),
        "Thumbnail row",
        "an image lead",
        "×3",
    ))
}

/// `WB_WIDGETS=1`: every Halo widget on one glass panel, for comparing against the prototype.
pub fn widget_gallery(ctx: &egui::Context) {
    use egui_phosphor::regular as icon;
    let id = Id::new("halo_widget_gallery");
    let mut st = ctx
        .data_mut(|d| d.get_temp::<GalleryState>(id))
        .unwrap_or_default();
    let screen = ctx.content_rect();
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(screen.center().x - 330.0, screen.top() + 70.0))
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                // a rail sample and a menu sample
                ui.vertical(|ui| {
                    glass(
                        ui,
                        glass_frame()
                            .corner_radius(CornerRadius::same(R_RAIL))
                            .inner_margin(Margin::same(5)),
                        |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for (i, ic) in [
                                "FormPyramide",
                                "Drop",
                                "Tree",
                                "3dBoxTop",
                                "Castle",
                                "Sword",
                                "CloudySun",
                                "Stethoscope",
                            ]
                            .iter()
                            .enumerate()
                            {
                                let r = icon_toggle(ui, ic, "", i == 0);
                                if i == 0 {
                                    paint_tip(
                                        ui.ctx(),
                                        r.id,
                                        r.rect,
                                        "Terrain",
                                        "Land · Sculpt · Paint · Caves",
                                        "1",
                                        TipSide::Right,
                                        1.0,
                                    );
                                }
                            }
                        },
                    );
                    glass_solid_frame()
                        .corner_radius(CornerRadius::same(R_MENU))
                        .inner_margin(Margin::same(4))
                        .show(ui, |ui| {
                            ui.set_width(200.0);
                            menu_item(ui, "New world…", "⌘N", false);
                            menu_item(ui, "Save", "⌘S", false);
                            menu_item(ui, "Cinematic", "", true);
                        });
                });
                ui.vertical(|ui| {
                    glass(ui, glass_frame(), |ui| {
                        ui.set_width(290.0);
                        title(ui, "Terrain");
                        let mut tab = st.seg;
                        segmented(
                            ui,
                            &mut tab,
                            &[(0, "Land"), (1, "Sculpt"), (2, "Paint"), (3, "Caves")],
                        );
                        st.seg = tab;
                        section(ui, "Tools");
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            let w = (ui.available_width() - 15.0) / 4.0;
                            for (i, (ic, l, k)) in [
                                (icon::ARROW_FAT_UP, "Raise", "R"),
                                (icon::ARROW_FAT_DOWN, "Lower", "F"),
                                (icon::WAVE_SINE, "Smooth", "S"),
                                (icon::STAIRS, "Terraces", "T"),
                            ]
                            .iter()
                            .enumerate()
                            {
                                if tool_tile_key(ui, ic, l, k, st.tool == i as u8, vec2(w, 58.0))
                                    .clicked()
                                {
                                    st.tool = i as u8;
                                }
                            }
                        });
                        tool_row_key(ui, "PlusMedium", "Paint Land", "B", true);
                        tool_row(ui, icon::LASSO, "Draw Coastline (Phosphor glyph)", false);
                        dim(ui, "Drag on the world to raise land out of the sea.");
                        hint_small(ui, "Hold Shift to paint sea instead.");
                        section(ui, "Land shape");
                        slider_row(ui, "Strength", &mut st.v[0], 0.0..=1.0, |v| {
                            format!("{:.0}%", v * 100.0)
                        });
                        slider_row_log(ui, "Brush size", &mut st.v[1], 2.0..=1500.0, |v| {
                            format!("{v:.0} m")
                        });
                        ui.add(
                            TrackSlider::new(&mut st.v[3], 1.0..=100.0)
                                .logarithmic(true)
                                .suffix(" m")
                                .text("Raw slider"),
                        );
                        ui.horizontal(|ui| {
                            slider_mini(ui, "Size", &mut st.v[1], 2.0..=1500.0, |v| {
                                format!("{v:.0} m")
                            });
                            slider_mini_log(ui, "Falloff", &mut st.v[2], 0.05..=1.0, |v| {
                                format!("{v:.2}")
                            });
                        });
                        let mut seg2 = st.pick;
                        segmented_solid(ui, &mut seg2, &[(0, "Paint"), (1, "Erase to Auto")]);
                        st.pick = seg2;
                        section_fold(ui, "gal_fold", "Show", "3 on", |ui| {
                            toggle(ui, &mut st.on[0], "Roads");
                            toggle(ui, &mut st.on[1], "Zone borders");
                            toggle(ui, &mut st.on[2], "Water");
                        });
                        section_box(ui, "gal_box", "Generate a world", |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Climate").color(TEXT_DIM));
                                let dd = egui::ComboBox::from_id_salt("gal_dd")
                                    .width(ui.available_width())
                                    .selected_text(
                                        ["Temperate", "Arid", "Arctic"][st.chip as usize % 3],
                                    )
                                    .dropdown_ui(ui, |ui| {
                                        for (i, n) in
                                            ["Temperate", "Arid", "Arctic"].iter().enumerate()
                                        {
                                            menu_option(ui, &mut st.chip, i as u8, *n);
                                        }
                                    });
                                if std::env::var("WB_WIDGETS").is_ok_and(|v| v == "open") {
                                    egui::Popup::open_id(ui.ctx(), dd.response.id.with("popup"));
                                }
                            });
                            primary_button(ui, &format!("{}  Generate", icon::SPARKLE));
                        });
                        ui.horizontal(|ui| {
                            button(ui, "Clear");
                            primary_button_fit(ui, "Send");
                            small_button(ui, "Undo");
                            let r = icon_button(ui, icon::TRASH, "Delete");
                            paint_tip(
                                ui.ctx(),
                                r.id.with("above"),
                                r.rect,
                                "Delete",
                                "",
                                "⌫",
                                TipSide::Above,
                                1.0,
                            );
                            icon_toggle(ui, icon::WIND, "Motion", true);
                            keycap(ui, "⌘K", "Search");
                        });
                    })
                });
                ui.vertical(|ui| {
                    glass(ui, glass_frame(), |ui| {
                        ui.set_width(276.0);
                        ui.label(RichText::new("Inspector").font(FontId::new(14.5, semibold())));
                        ui.horizontal_wrapped(|ui| {
                            for n in [
                                "Earth",
                                "Hammer",
                                "PaintBrush",
                                "Shovel",
                                "AnimatePath",
                                "Map",
                                "Sun",
                                "Rainy",
                                "ColorPalette",
                                "VolumeFull",
                                "CompassSquare",
                                "Footsteps",
                                "Gamepad",
                                "Video",
                                "SparklesTwo",
                                "Agent",
                                "ArrowUndoUp",
                                "ArrowRedoDown",
                                "MagnifyingGlass",
                            ] {
                                super::icons::icon(ui, n, 15.0, TEXT_DIM).on_hover_text(n);
                            }
                        });
                        search_field(ui, &mut st.text, "Search assets");
                        let mut c = st.chip;
                        chips(
                            ui,
                            &mut c,
                            &[
                                (0, "All", Some(214)),
                                (1, "Trees", Some(38)),
                                (2, "Rocks", Some(52)),
                                (3, "Props", Some(91)),
                            ],
                        );
                        st.chip = c;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            let w = (ui.available_width() - 10.0) / 3.0;
                            asset_card(ui, None, "Oak", true, w, false, true);
                            asset_card(ui, None, "Boulder", false, w, true, false);
                            asset_card(ui, None, "Crate", false, w, false, false);
                        });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            let w = (ui.available_width() - 5.0) / 2.0;
                            image_card(
                                ui,
                                None,
                                "Archipelago",
                                "many islands",
                                true,
                                vec2(w, w * 0.625),
                            );
                            image_card(ui, None, "Continent", "", false, vec2(w, w * 0.625));
                        });
                        for (i, (lead, t, sub, m)) in [
                            (
                                Lead::Dot(Color32::from_rgb(232, 96, 86)),
                                "Ashen Wastes",
                                "Danger zone · level 12",
                                "1.2 km²",
                            ),
                            (Lead::Icon(icon::SHOVEL), "Cave 1", "3 chambers", "−40 m"),
                            (Lead::None, "Plain row", "", ""),
                        ]
                        .into_iter()
                        .chain(gradient_lead(ui.ctx()))
                        .enumerate()
                        {
                            if list_row(ui, lead, t, sub, m, st.row == i as u8).clicked() {
                                st.row = i as u8;
                            }
                        }
                        stat_row(ui, "Objects", "4 421");
                        bar_stat(ui, "Walkable", "72%", 0.72, OK);
                        bar_stat(ui, "Too steep", "9%", 0.09, WARN);
                        warn_line(ui, "Two zones overlap.");
                        ok_line(ui, "Everything is reachable.");
                        ui.horizontal(|ui| {
                            ui.add(egui::Button::new("egui button"));
                            ui.add(
                                egui::TextEdit::singleline(&mut st.text)
                                    .desired_width(110.0)
                                    .hint_text("text field"),
                            );
                        });
                        let mut c2 = st.chip;
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut c2, 0, "selectable");
                            ui.selectable_value(&mut c2, 1, "values");
                        });
                        st.chip = c2;
                    })
                });
            });
        });
    ctx.data_mut(|d| d.insert_temp(id, st));
}
