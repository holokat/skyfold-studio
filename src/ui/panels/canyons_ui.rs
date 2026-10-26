//! Sculpt › Strata and Sculpt › Canyonlands: the canyonlands section in the tool panel (looks,
//! lakes, the whole-world button, fine-tune sliders folded away) and both tools' docks.

use bevy_egui::egui::Ui;

use super::super::UiState;
use super::super::widgets::*;
use super::meters;
use crate::terrain::canyons::CanyonParams;

/// The two looks: (name, tip, params).
fn looks() -> [(&'static str, &'static str, CanyonParams); 2] {
    [
        (
            "Alien canyonland",
            "Vast and deep: many floor tiers, crisp strata, mesas and lakes",
            CanyonParams::alien(),
        ),
        (
            "Desert canyons",
            "Gentler canyons with fewer tiers and no lakes",
            CanyonParams::default(),
        ),
    ]
}

/// Canyonlands section (shown while the Canyonlands tool is picked).
pub(super) fn canyons_section(ui: &mut Ui, s: &mut UiState) {
    section_box(ui, "sculpt_canyons", "Canyonlands", |ui| {
        dim(
            ui,
            "Click the ground to raise canyonlands over the brush circle: plateaus, terraced canyons, mesas and buttes.",
        );
        let looks = looks();
        let cur = looks.iter().position(|(_, _, p)| {
            p.relief == s.ed.canyons.relief && p.tiers == s.ed.canyons.tiers
        });
        if let Some(i) = card_grid(ui, 2, looks.len(), |ui, i, w| {
            let icon = if i == 0 { "Map" } else { "Sunset" };
            icon_card(ui, icon, looks[i].0, cur == Some(i), w).on_hover_text(looks[i].1)
        }) {
            s.ed.canyons = looks[i].2;
        }
        let mut lakes = s.ed.canyons.lakes > 0.0;
        if toggle(ui, &mut lakes, "Lakes on the canyon floors")
            .on_hover_text("Each floor tier gets a few lakes at its own height")
            .changed()
        {
            s.ed.canyons.lakes = if lakes { 0.6 } else { 0.0 };
        }
        if wide_icon_button(ui, "Earth", "Canyonlands over the entire world")
            .on_hover_text("Replaces the whole world's heights with canyonlands (one undo step)")
            .clicked()
        {
            whole_world(s);
        }
        // fine-tune starts folded away
        let fine = ui.make_persistent_id(("section_fold", "canyons_fine"));
        ui.data_mut(|d| {
            d.get_persisted_mut_or(fine, false);
        });
        section_fold(ui, "canyons_fine", "Fine-tune", "", |ui| {
            let c = &mut s.ed.canyons;
            slider_row(ui, "Plateau height", &mut c.relief, 40.0..=600.0, meters);
            slider_row(ui, "Canyon spacing", &mut c.scale, 300.0..=4000.0, meters);
            slider_row(ui, "Canyon width", &mut c.canyon_width, 0.4..=2.0, |v| {
                format!("×{v:.2}")
            });
            slider_row(ui, "Side canyons", &mut c.branching, 0.0..=1.0, super::pct);
            slider_row(ui, "Mesas & buttes", &mut c.mesas, 0.0..=1.0, super::pct);
            let mut tiers = c.tiers as f32;
            if slider_row(ui, "Floor tiers", &mut tiers, 1.0..=5.0, |v| {
                format!("{v:.0}")
            })
            .changed()
            {
                c.tiers = tiers.round() as u32;
            }
            slider_row(ui, "Plateau steps", &mut c.plateau_steps, 0.0..=1.0, super::pct);
            slider_row(ui, "Rock layer height", &mut c.strata.step, 4.0..=60.0, meters);
            slider_row(ui, "Ledge sharpness", &mut c.strata.sharpness, 0.0..=1.0, super::pct);
            slider_row(ui, "Irregularity", &mut c.strata.irregularity, 0.0..=1.0, super::pct);
            slider_row(
                ui,
                "Banded cliff rock",
                &mut s.terrain.style.strata_bands,
                0.0..=1.0,
                super::pct,
            )
            .on_hover_text("Rock layers of different tones up every cliff in this world");
        });
    });
}

fn whole_world(s: &mut UiState) {
    let p = s.ed.canyons;
    let seed = s.meta.seed + 19;
    let made = crate::terrain::canyon_ops::canyonlands(
        &mut s.terrain,
        &mut s.history,
        &mut s.store,
        s.lib.as_deref(),
        &mut s.water,
        &p,
        seed,
        None,
        p.lakes > 0.0,
    );
    if let Some((r, n)) = made {
        s.meta.dirty = true;
        s.ed.toast(format!(
            "The world is canyonlands now: plateaus up to {:.0} m, {} floor tiers{} (undo to go back)",
            r.top - r.floor,
            r.tier_heights.len(),
            if n > 0 { format!(", {n} lakes") } else { String::new() }
        ));
    }
}

/// Dock rows for Strata: bed height, sharpness, irregularity.
pub(super) fn strata_dock(ui: &mut Ui, s: &mut UiState) {
    let sp = &mut s.ed.strata;
    slider_mini(ui, "Layer height", &mut sp.step, 2.0..=60.0, meters);
    slider_mini(ui, "Sharpness", &mut sp.sharpness, 0.0..=1.0, |v| {
        format!("{v:.2}")
    });
    slider_mini(ui, "Irregular", &mut sp.irregularity, 0.0..=1.0, |v| {
        format!("{v:.2}")
    });
}

/// Dock rows for Canyonlands: the area (the brush size) and the plateau height.
pub(super) fn canyons_dock(ui: &mut Ui, s: &mut UiState) {
    if let Some(b) = s.ed.active_brush_mut() {
        slider_mini_log(ui, "Area", &mut b.size, 100.0..=1500.0, meters);
    }
    slider_mini(ui, "Plateau", &mut s.ed.canyons.relief, 40.0..=600.0, meters);
}
