//! Lighting › Sky bodies: quick-pick skies, then one fold per body (where it hangs, how big,
//! its ring and air; colours and surface folded away).

use bevy_egui::egui::{self, RichText};

use super::{BodyKind, MAX_BODIES, SkyBodies, SkyBody, SkyPreset};
use crate::ui::theme::*;
use crate::ui::widgets::*;

fn deg(v: f32) -> String {
    format!("{v:.0}°")
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [f32; 3]) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.color_edit_button_rgb(c);
        });
    });
}

/// The section's contents. `view_bearing`: where the camera looks (new bodies appear there).
/// Returns true when anything changed.
pub fn sky_bodies(ui: &mut egui::Ui, set: &mut SkyBodies, view_bearing: f32) -> bool {
    let before = set.clone();
    dim(
        ui,
        "Planets, moons, distant suns and black holes far above the world, lit by its sun and seen through its air.",
    );
    // quick picks
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for p in SkyPreset::ALL.into_iter().skip(1) {
            if chip(ui, p.name(), None, false).clicked() {
                set.bodies = p.bodies(view_bearing);
            }
        }
    });
    toggle(ui, &mut set.hide_moon, "Hide the world's moon");

    let mut remove = None;
    for (i, b) in set.bodies.iter_mut().enumerate() {
        let note = if b.enabled { b.kind.name() } else { "off" };
        let title = format!("{} {}", i + 1, b.name);
        section_fold(ui, ("sky_body", i), &title, note, |ui| {
            ui.horizontal(|ui| {
                toggle(ui, &mut b.enabled, "Shown");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "TrashCan", "Remove").clicked() {
                        remove = Some(i);
                    }
                    if icon_button(ui, "Target", "Move to where the camera looks").clicked() {
                        b.bearing = view_bearing;
                    }
                    if icon_button(ui, "Shuffle", "Another surface").clicked() {
                        b.seed = fastrand::u32(1..100_000);
                    }
                });
            });
            let mut kind = b.kind;
            dropdown_row(ui, "Kind", ("sky_body_kind", i), kind.name(), |ui| {
                for k in BodyKind::ALL {
                    menu_option(ui, &mut kind, k, k.name());
                }
            });
            if kind != b.kind {
                // a new kind starts as designed, where the old one hung
                let keep = (b.bearing, b.elevation, b.enabled);
                *b = SkyBody::preset(kind);
                (b.bearing, b.elevation, b.enabled) = keep;
            }
            slider_row(ui, "Direction", &mut b.bearing, 0.0..=359.9, |v| {
                format!("{v:.0}° {}", compass(v))
            });
            slider_row(ui, "Height in the sky", &mut b.elevation, -30.0..=90.0, deg);
            slider_row_log(ui, "Size", &mut b.size, 0.1..=80.0, deg);
            if !b.kind.emissive() {
                slider_row(ui, "Atmosphere", &mut b.atmosphere, 0.0..=1.0, pct);
            }
            slider_row(ui, "Brightness", &mut b.brightness, 0.0..=4.0, |v| format!("{v:.2}"));
            if b.kind == BodyKind::BlackHole {
                slider_row(ui, "Disc size", &mut b.ring_outer, 1.1..=5.0, |v| format!("{v:.1}"));
                slider_row(ui, "Disc opening", &mut b.ring_open, 0.0..=90.0, deg);
            } else if !b.kind.emissive() {
                slider_row(ui, "Ring", &mut b.ring, 0.0..=1.0, pct);
                if b.ring > 0.0 {
                    slider_row(ui, "Ring opening", &mut b.ring_open, 0.0..=90.0, deg);
                    slider_row(ui, "Ring inner edge", &mut b.ring_inner, 1.05..=4.0, |v| {
                        format!("{v:.2}×")
                    });
                    slider_row(ui, "Ring outer edge", &mut b.ring_outer, 1.1..=5.0, |v| {
                        format!("{v:.2}×")
                    });
                }
            }
            section_fold(ui, ("sky_body_more", i), "Surface & colours", "", |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name").color(TEXT_DIM));
                    ui.text_edit_singleline(&mut b.name);
                });
                slider_row(ui, "Axis tilt", &mut b.tilt, -90.0..=90.0, deg);
                slider_row(ui, "Turn", &mut b.spin, 0.0..=359.9, deg);
                color_row(ui, "Colour", &mut b.color);
                color_row(ui, "Second colour", &mut b.color2);
                if !b.kind.emissive() {
                    color_row(ui, "Atmosphere colour", &mut b.atmosphere_color);
                    color_row(ui, "Ring colour", &mut b.ring_color);
                }
                toggle(ui, &mut b.follow_sky, "Circle the sky over the day");
            });
            b.clamp();
        });
    }
    if let Some(i) = remove {
        set.bodies.remove(i);
    }
    if set.bodies.len() < MAX_BODIES {
        let mut add = None;
        dropdown_row(ui, "Add", "sky_body_add", "Planet, moon, star…", |ui| {
            for k in BodyKind::ALL {
                if menu_item(ui, k.name(), "", false).clicked() {
                    add = Some(k);
                }
            }
        });
        if let Some(k) = add {
            set.bodies.push(SkyBody {
                bearing: view_bearing,
                ..SkyBody::preset(k)
            });
        }
    }
    *set != before
}

fn compass(b: f32) -> &'static str {
    const N: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    N[((b / 45.0 + 0.5).floor() as usize) % 8]
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
