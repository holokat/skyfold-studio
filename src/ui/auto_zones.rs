//! A viewport-level progress card for automatic zone generation. It stays visible when the
//! zones panel is closed so starting a request always has a clear, persistent acknowledgement.

use std::time::Duration;

use bevy_egui::egui::{
    self, Align, Color32, CornerRadius, FontId, Frame, Id, Layout, Margin, Order, Rect, Sense,
    Stroke, Ui, pos2, vec2,
};

use super::UiState;
use super::theme::*;
use super::widgets::paint_icon;

const CARD_WIDTH: f32 = 336.0;

pub fn show(ctx: &egui::Context, s: &mut UiState, screen: Rect) {
    let Some(stage) = s.auto_zones.stage() else {
        return;
    };
    let elapsed = s.auto_zones.elapsed().unwrap_or_default();
    ctx.request_repaint_after(Duration::from_millis(16));

    egui::Area::new(Id::new("auto_zones_progress"))
        .order(Order::Foreground)
        .fixed_pos(pos2(
            screen.center().x - CARD_WIDTH * 0.5,
            screen.top() + 104.0,
        ))
        .show(ctx, |ui| {
            Frame::new()
                .fill(GLASS_SOLID)
                .stroke(Stroke::new(1.0, LINE_STRONG))
                .corner_radius(CornerRadius::same(R_PANEL))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| card(ui, stage, elapsed, s));
        });
}

fn card(ui: &mut Ui, stage: &str, elapsed: Duration, s: &mut UiState) {
    ui.set_width(CARD_WIDTH - 28.0);
    ui.horizontal(|ui| {
        let spinner = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover()).0;
        egui::Spinner::new()
            .size(18.0)
            .color(ACCENT)
            .paint_at(ui, spinner);
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(stage)
                .font(FontId::new(14.0, semibold()))
                .color(TEXT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format_elapsed(elapsed))
                    .font(FontId::new(12.0, mono()))
                    .color(TEXT_DIM),
            );
        });
    });
    ui.add_space(7.0);
    ui.label(
        egui::RichText::new(
            "Planning zones around your terrain and player start. Keep this world unchanged until it finishes.",
        )
        .size(12.0)
        .color(TEXT_DIM),
    );
    ui.add_space(11.0);
    indeterminate_progress(ui);
    ui.add_space(12.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if cancel_button(ui).clicked() {
            s.auto_zones
                .requests
                .push(crate::zones::auto::AutoZoneRequest::Cancel);
        }
    });
}

fn indeterminate_progress(ui: &mut Ui) {
    let (track, _) = ui.allocate_exact_size(vec2(ui.available_width(), 5.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(3), WELL);
    let segment = track.width() * 0.34;
    let phase = (ui.ctx().input(|input| input.time) as f32 * 0.7).fract();
    let x = track.left() - segment + phase * (track.width() + segment);
    let moving =
        Rect::from_min_size(pos2(x, track.top()), vec2(segment, track.height())).intersect(track);
    painter.rect_filled(moving, CornerRadius::same(3), ACCENT);
}

fn cancel_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(98.0, 40.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hot = response.hovered();
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(R_INPUT),
            if hot { SOFT_HOVER } else { SOFT },
        );
        paint_icon(
            ui,
            pos2(rect.left() + 18.0, rect.center().y),
            "CrossSmall",
            15.0,
            if hot { TEXT } else { TEXT_DIM },
        );
        let label = ui.painter().layout_no_wrap(
            "Cancel".to_owned(),
            FontId::new(13.0, medium()),
            if hot { TEXT } else { TEXT_DIM },
        );
        ui.painter().galley(
            pos2(rect.left() + 31.0, rect.center().y - label.size().y * 0.5),
            label,
            if hot { TEXT } else { TEXT_DIM },
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Stop generating zones")
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
