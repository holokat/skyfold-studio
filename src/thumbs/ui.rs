//! Widgets for the pictures: asset / fish tiles (thumbnail with the name under it) and the big
//! preview card with the live turntable.

use bevy_egui::egui::{self, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, vec2};

use super::{Previews, Subject};
use crate::ui::theme::*;

/// A tile: the subject's thumbnail (a spinner while it renders, `icon` if it can't be pictured)
/// with its name under it, up to two lines.
pub fn tile(
    ui: &mut egui::Ui,
    previews: &mut Previews,
    s: &Subject,
    icon: &str,
    label: &str,
    selected: bool,
    width: f32,
) -> egui::Response {
    let px = s.size();
    let img_w = width - 6.0;
    let img_h = img_w * px.y as f32 / px.x as f32;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(width, 4.0 + img_h + 4.0 + 14.0 + 6.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        // scrolled away: not drawn, and its thumbnail can wait
        return resp;
    }
    // the Halo asset card: a soft radial well, the picture, the name under it; a white ring when picked
    let img = Rect::from_min_size(rect.min + vec2(3.0, 4.0), vec2(img_w, img_h));
    crate::ui::widgets::card_well(ui, img, resp.hovered());
    match previews.thumb(s) {
        Some(t) => {
            let k = if resp.hovered() { 0.96 } else { 0.92 };
            egui::Image::new((t, img.size() * k))
                .corner_radius(CornerRadius::same(7))
                .paint_at(ui, Rect::from_center_size(img.center(), img.size() * k));
        }
        None if previews.pending(s) => {
            egui::Spinner::new()
                .size(16.0)
                .color(TEXT_DIM)
                .paint_at(ui, Rect::from_center_size(img.center(), vec2(16.0, 16.0)));
        }
        None => {
            crate::ui::widgets::paint_icon(ui, img.center(), icon, 20.0, TEXT_DIM);
        }
    }
    let p = ui.painter();
    if selected {
        p.rect_stroke(
            img,
            CornerRadius::same(8),
            Stroke::new(2.0, Color32::WHITE),
            StrokeKind::Outside,
        );
    }
    let colour = if selected { TEXT } else { TEXT_DIM };
    let mut job = egui::text::LayoutJob::simple_singleline(
        label.to_owned(),
        FontId::proportional(11.5),
        colour,
    );
    job.wrap.max_width = width - 4.0;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(rect.center().x - galley.size().x * 0.5, img.bottom() + 4.0),
        galley,
        colour,
    );
    resp.on_hover_text(label)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The big preview card: the subject turning live in the studio (its thumbnail until the first
/// frames are in), with a title and detail lines under it.
pub fn card(
    ui: &mut egui::Ui,
    previews: &mut Previews,
    s: &Subject,
    title: &str,
    lines: &[String],
) {
    let w = ui.available_width();
    let h = w * super::preview::SIZE.y as f32 / super::preview::SIZE.x as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), CARD_BG);
    let live = previews.turntable(s);
    match live.or_else(|| previews.thumb(s)) {
        Some(t) if live.is_some() => {
            egui::Image::new((t, rect.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, rect);
        }
        Some(t) => {
            // the still thumbnail, at its own aspect, until the turntable is running
            let px = s.size();
            let fit = (rect.width() / px.x as f32).min(rect.height() / px.y as f32);
            let r = Rect::from_center_size(rect.center(), vec2(px.x as f32, px.y as f32) * fit);
            egui::Image::new((t, r.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, r);
        }
        None => {
            egui::Spinner::new()
                .size(20.0)
                .color(TEXT_DIM)
                .paint_at(ui, Rect::from_center_size(rect.center(), vec2(20.0, 20.0)));
        }
    }
    // the turntable keeps moving: keep painting
    ui.ctx().request_repaint();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title)
            .font(FontId::new(15.0, semibold()))
            .color(Color32::WHITE),
    );
    for l in lines {
        ui.label(egui::RichText::new(l).small().color(TEXT_DIM));
    }
}
