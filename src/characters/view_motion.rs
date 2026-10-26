//! The Motion tab of a model's inspector: what runs on top of its clips — feet on the ground,
//! looking at you, its style (posture, arm swing, breathing) and its moving parts (capes, skirts,
//! hair, tails swinging under gravity and wind), plus fitting clothing models onto it.

use bevy_egui::egui;
use egui::vec2;

use super::Entry;
use super::rigging::{self, NewPart, Source};
use super::view::ViewData;
use crate::ui::theme::*;
use crate::ui::widgets::*;
use rigkit::spec::{PartKind, Preset, RigSpec};

pub(super) fn motion_tab(ui: &mut egui::Ui, d: &mut ViewData, e: &Entry, now: f64) {
    if !e.rigged() {
        dim(ui, "Rig it first: motion works on the skeleton.");
        return;
    }
    let id = e.meta.id.clone();
    let busy = d.chars.busy(&id).map(|j| j.status.lock().unwrap().clone());
    let mut spec: RigSpec = d.specs.get(e).clone();
    let before = spec.clone();

    section_box(ui, "motion_body", "Body", |ui| {
        toggle(ui, &mut spec.feet.on, "Feet on the ground");
        hint_small(ui, "Each foot finds the ground on slopes and steps; the hips drop to reach.");
        ui.add_space(4.0);
        toggle(ui, &mut spec.look.on, "Looks at you");
        if spec.look.on {
            slider_row(ui, "Turns its chest", &mut spec.look.body, 0.0..=1.0, |v| format!("{:.0} %", v * 100.0));
        }
        ui.add_space(4.0);
        slider_row(ui, "Posture", &mut spec.style.posture, -1.0..=1.0, |v| {
            match v {
                v if v < -0.05 => format!("Slouched {:.0} %", -v * 100.0),
                v if v > 0.05 => format!("Proud {:.0} %", v * 100.0),
                _ => "As animated".into(),
            }
        });
        slider_row(ui, "Arm swing", &mut spec.style.arm_swing, 0.0..=2.0, |v| format!("{v:.1}×"));
        slider_row(ui, "Breathing", &mut spec.style.breathing, 0.0..=1.0, |v| format!("{:.0} %", v * 100.0));
    });
    ui.add_space(8.0);

    section_box(ui, "motion_parts", "Moving parts", |ui| {
        if let Some(s) = &busy {
            dim(ui, s);
            ui.ctx().request_repaint();
        }
        if spec.parts.is_empty() {
            hint_small(ui, "Capes, skirts, hair and tails that swing as it moves, kept off its body. For a cape, fit a cape model under Clothing, then make it move.");
        }
        let mut remove = None;
        let open_id = ui.make_persistent_id(("motion_open_part", &id));
        let mut open: Option<String> = ui.data(|m| m.get_temp(open_id));
        for p in spec.parts.iter_mut() {
            let icon = match p.kind {
                PartKind::Sheet => "Flag1",
                PartKind::Chain => "Feather",
                PartKind::Jiggle => "Sparkle",
            };
            let off = if p.on { "" } else { " · off" };
            let sub = format!("{} · {} · {} bones{off}", p.kind.label(), p.preset.label(), p.bone_count());
            let is_open = open.as_deref() == Some(p.id.as_str());
            let row = list_row(ui, Lead::Icon(icon), &p.name, &sub, "", is_open);
            // (one click to take it off, right on its row)
            let can_remove = e.builtin.is_none() && busy.is_none();
            let what = if p.mesh.is_some() { "Take it off" } else { "Remove this part" };
            let bin = egui::Rect::from_center_size(egui::pos2(row.rect.right() - 20.0, row.rect.center().y), vec2(28.0, 28.0));
            if can_remove && ui.put(bin, |ui: &mut egui::Ui| icon_button(ui, "TrashCan", what)).clicked() {
                remove = Some(p.id.clone());
            } else if row.clicked() {
                open = if is_open { None } else { Some(p.id.clone()) };
            }
            if !is_open {
                continue;
            }
            let pid = p.id.clone();
            ui.indent(("motion_part", pid.clone()), |ui| {
                toggle(ui, &mut p.on, "Moving");
                let mut preset = p.preset;
                dropdown_row(ui, "Feels like", ("motion_preset", &pid), preset.label(), |ui| {
                    for q in Preset::ALL {
                        menu_option(ui, &mut preset, q, q.label());
                    }
                });
                if preset != p.preset {
                    p.apply_preset(preset);
                }
                let mut custom = false;
                custom |= slider_row(ui, "Holds its shape", &mut p.stiffness, 0.0..=1.0, pct).changed();
                custom |= slider_row(ui, "Settles", &mut p.damping, 0.0..=1.0, pct).changed();
                custom |= slider_row(ui, "Weight", &mut p.gravity, 0.0..=2.0, |v| format!("{v:.1}×")).changed();
                custom |= slider_row(ui, "Wind", &mut p.wind, 0.0..=2.0, |v| format!("{v:.1}×")).changed();
                if p.kind == PartKind::Sheet {
                    custom |= slider_row(ui, "Stretch", &mut p.stretch, 0.0..=0.15, pct).changed();
                }
                if custom {
                    p.preset = Preset::Custom;
                }
                toggle(ui, &mut p.collide, "Kept off the body");
                if can_remove && small_icon_button(ui, "TrashCan", what).clicked() {
                    remove = Some(pid.clone());
                }
            });
        }
        ui.data_mut(|m| m.insert_temp(open_id, open));
        if let Some(pid) = remove
            && let Err(err) = rigging::remove_part(&mut d.chars, &mut d.specs, &id, &pid)
        {
            d.v.note(err, true, now);
        }
        if e.builtin.is_some() {
            ui.add_space(6.0);
            hint_small(ui, "The built-in mannequins can't change. Copy one to give it moving parts or clothes.");
            if wide_icon_button(ui, "LayersTwo", "Make an editable copy").clicked() {
                match rigging::copy_builtin(&mut d.chars, &id) {
                    Ok(nid) => {
                        d.v.sel = Some(super::view::Sel::Character(nid.clone()));
                        d.v.note("Copied: give it moving parts in Motion", false, now);
                    }
                    Err(err) => d.v.note(err, true, now),
                }
            }
            return;
        }
        if busy.is_some() {
            return;
        }
        ui.add_space(6.0);
        found(ui, d, e, &spec, now);
    });
    ui.add_space(8.0);

    section_box(ui, "motion_clothes", "Clothing", |ui| {
        hint_small(
            ui,
            "A garment takes the skin weights of the body under it, so it moves exactly with the body. Loose parts of it can then swing.",
        );
        ui.add_space(4.0);
        if e.builtin.is_none()
            && busy.is_none()
            && wide_icon_button(ui, "Import", "Fit clothing…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_title("Fit clothing (a garment made for this character works best)")
                .add_filter("Models", &["fbx", "glb", "gltf", "obj", "zip"])
                .pick_file()
        {
            match rigging::fit_garment(&mut d.chars, &mut d.specs, &id, path) {
                Ok(()) => d.v.note("Fitting it…", false, now),
                Err(err) => d.v.note(err, true, now),
            }
        }
    });
    ui.add_space(8.0);

    section_fold(ui, ("motion_more", &id), "More", "", |ui| {
        slider_row(ui, "Wind on its parts", &mut spec.wind, 0.0..=2.0, |v| format!("{v:.1}×"));
        slider_row(ui, "Head turns up to", &mut spec.look.max_yaw_deg, 20.0..=110.0, |v| format!("{v:.0}°"));
        slider_row(ui, "Notices you within", &mut spec.look.range, 1.0..=30.0, |v| format!("{v:.0} m"));
        slider_row(ui, "Hips drop up to", &mut spec.feet.max_drop, 0.0..=0.8, |v| format!("{:.0} cm", v * 100.0));
        slider_row(ui, "Feet tilt to the slope", &mut spec.feet.align, 0.0..=1.0, pct);
    });

    if spec != before {
        d.specs.set(e, spec);
    }
}

fn pct(v: f32) -> String {
    format!("{:.0} %", v * 100.0)
}

/// What was found on the model: one click each to make it move, or any mesh by name.
fn found(ui: &mut egui::Ui, d: &mut ViewData, e: &Entry, spec: &RigSpec, now: f64) {
    let id = e.meta.id.clone();
    match d.specs.suggestions(e) {
        Ok(None) => {
            dim(ui, "Looking for loose parts…");
            ui.ctx().request_repaint();
        }
        Err(err) => warn_line(ui, &err),
        Ok(Some(f)) => {
            // (skip what already moves: a part of the same name)
            let fresh: Vec<(usize, &rigkit::author::Suggestion)> = f
                .suggestions
                .iter()
                .enumerate()
                .filter(|(_, s)| !spec.parts.iter().any(|p| p.name == s.name))
                .collect();
            if !fresh.is_empty() {
                hint_small(ui, "Found on this model");
                for (i, s) in fresh {
                    ui.horizontal(|ui| {
                        let w = ui.available_width();
                        ui.allocate_ui(vec2(w - 112.0, 30.0), |ui| {
                            ui.label(egui::RichText::new(&s.name).color(TEXT));
                            ui.label(
                                egui::RichText::new(format!("{} · {:.0} cm", s.kind.label(), s.size * 100.0))
                                    .color(TEXT_DIM)
                                    .size(11.0),
                            );
                        });
                        if small_icon_button(ui, "MagicWand", "Make it move").clicked() {
                            match rigging::add_part(&mut d.chars, &mut d.specs, &id, Source::Suggestion(i), NewPart::default()) {
                                Ok(()) => d.v.note(format!("Adding {}…", s.name), false, now),
                                Err(err) => d.v.note(err, true, now),
                            }
                        }
                    });
                }
            } else if spec.parts.is_empty() {
                hint_small(ui, "Nothing hangs loose on this model. Pick a mesh to make it move whole:");
            }
            if f.meshes.len() > 1 {
                ui.add_space(4.0);
                let mut pick: Option<String> = None;
                dropdown_row(ui, "Make a mesh move", ("motion_mesh", &id), "Pick…", |ui| {
                    for m in &f.meshes {
                        let label = if m.is_empty() { "(unnamed)" } else { m.as_str() };
                        if menu_item(ui, label, "", false).clicked() {
                            pick = Some(m.clone());
                        }
                    }
                });
                if let Some(m) = pick {
                    match rigging::add_part(&mut d.chars, &mut d.specs, &id, Source::Mesh(m.clone()), NewPart::default()) {
                        Ok(()) => d.v.note(format!("Adding {m}…"), false, now),
                        Err(err) => d.v.note(err, true, now),
                    }
                }
            }
        }
    }
}
