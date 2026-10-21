//! Small in-editor tutorials: a card with numbered steps shown the first times a feature is
//! opened, until "Got it" (remembered in the prefs under `tips_seen`); a "?" button brings it
//! back.

use std::collections::BTreeSet;
use std::sync::Mutex;

use bevy_egui::egui::{self, Color32, CornerRadius, FontId, Frame, Margin, RichText, Stroke, Ui};
use egui_phosphor::regular as icon;

use super::theme::*;

const KEY: &str = "tips_seen";

static SEEN: Mutex<Option<BTreeSet<String>>> = Mutex::new(None);

fn with_seen<R>(f: impl FnOnce(&mut BTreeSet<String>) -> R) -> R {
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let set = g.get_or_insert_with(|| {
        crate::prefs::get(KEY)
            .and_then(|v| serde_json::from_value::<BTreeSet<String>>(v).ok())
            .unwrap_or_default()
    });
    f(set)
}

/// The tutorial `id` was dismissed (test runs: or listed in `WB_TIPS_SEEN=id,id…`).
pub fn seen(id: &str) -> bool {
    std::env::var("WB_TIPS_SEEN").is_ok_and(|v| v.split(',').any(|x| x == id))
        || with_seen(|s| s.contains(id))
}

/// Dismiss (`true`) or bring back (`false`) the tutorial `id`.
pub fn mark(id: &str, seen: bool) {
    let all = with_seen(|s| {
        if seen {
            s.insert(id.to_string());
        } else {
            s.remove(id);
        }
        s.clone()
    });
    crate::prefs::set(KEY, serde_json::json!(all));
}

/// The tutorial card (unless dismissed): a title and numbered steps.
pub fn card(ui: &mut Ui, id: &str, title: &str, steps: &[&str]) {
    if seen(id) {
        return;
    }
    Frame::new()
        .fill(SOFT)
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::LIGHTBULB).color(Color32::from_rgb(255, 214, 120)));
                ui.label(
                    RichText::new(title)
                        .font(FontId::new(14.0, semibold()))
                        .color(Color32::WHITE),
                );
            });
            ui.add_space(4.0);
            for (i, s) in steps.iter().enumerate() {
                ui.horizontal_top(|ui| {
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 9.0, ACCENT);
                    ui.painter().text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("{}", i + 1),
                        FontId::new(11.5, semibold()),
                        ON_ACCENT,
                    );
                    ui.add(egui::Label::new(RichText::new(*s).color(TEXT)).wrap());
                });
                ui.add_space(2.0);
            }
            ui.add_space(2.0);
            if ui.button(format!("{} Got it", icon::CHECK)).clicked() {
                mark(id, true);
            }
        });
    ui.add_space(6.0);
}

/// A small "?" that shows the tutorial `id` again (only while it's dismissed).
pub fn help_button(ui: &mut Ui, id: &str) {
    if seen(id)
        && ui
            .add(egui::Button::new(RichText::new(icon::QUESTION).color(TEXT_DIM)).frame(false))
            .on_hover_text("How does this work?")
            .clicked()
    {
        mark(id, false);
    }
}
