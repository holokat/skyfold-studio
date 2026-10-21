//! Objects › Scenes: the library of saved scenes (tiles), Save a scene (draw a loop), Import, the
//! picked scene's card (place, export, list on the marketplace, rename, delete) and the dialogs.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, Frame, Margin, RichText, Stroke, vec2};
use egui_phosphor::regular as icon;

use super::{Mode, file};
use crate::editor::{ObjectTool, Tab};
use crate::thumbs::Subject;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

const GOLD: Color32 = Color32::from_rgb(255, 214, 120);
const GREEN: Color32 = Color32::from_rgb(74, 196, 120);

/// Leaves whatever else the Place tool was doing (an asset, a set piece, a town).
fn take_place_tool(s: &mut UiState) {
    s.ed.tab = Tab::Objects;
    s.ed.object_tool = ObjectTool::Place;
    s.place.selected = None;
    s.sets.selected = None;
    s.towns.selected = None;
}

/// The left panel's Scenes view.
pub fn browser(ui: &mut egui::Ui, s: &mut UiState) {
    if !s.scenes.loaded {
        s.scenes.reload();
    }
    // `WB_SCENE_PLACE=<id>` (screenshot tests): start placing that scene once it's in the list
    static HOOK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Ok(id) = std::env::var("WB_SCENE_PLACE")
        && s.scenes.get(&id).is_some()
        && !HOOK.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        take_place_tool(s);
        s.scenes.mode = Mode::Place(id);
    }
    crate::ui::tips::card(
        ui,
        "scenes",
        "Scenes",
        &[
            "Decorate a spot — a shrine, a camp, a temple inside and out — then save it as a scene: Save a scene, and draw a loop round it.",
            "Click a scene, then the ground, to place it again here or in any other world, exactly as it was laid out. It lands as one set you can move, turn and resize.",
            "Export a scene to share it, or as one model for Unreal, Unity and Godot; List on marketplace sells or gives it away on skyfoldstudio.com.",
            "Ready-made scenes from the asset packs (whole monster-encounter sites…) are below yours: click one, then the ground, to place it whole.",
        ],
    );
    let capturing = s.scenes.capturing() && s.ed.object_tool == ObjectTool::Place;
    if capturing {
        Frame::new().fill(Color32::from_rgba_unmultiplied(255, 214, 120, 18)).stroke(Stroke::new(1.0, GOLD.gamma_multiply(0.5))).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{}  Draw a loop round the objects", icon::SCRIBBLE_LOOP)).strong().color(GOLD));
            dim(ui, "Hold the mouse and draw round everything that belongs in the scene, like drawing a lake. Every placed object inside is saved where it stands.");
            if ui.button("Cancel").clicked() {
                s.scenes.mode = Mode::Off;
            }
        });
    } else if primary_button(ui, &format!("{}  Save a scene", icon::PLUS))
        .on_hover_text(
            "Draw a loop round placed objects to save them, laid out as they are, for reuse",
        )
        .clicked()
    {
        take_place_tool(s);
        s.store.selected = None;
        s.scenes.mode = Mode::Capture;
    }
    ui.add_space(4.0);
    if ui
        .add(
            egui::Button::new(format!("{}  Open a scene file…", icon::FOLDER_OPEN))
                .min_size(vec2(ui.available_width(), 26.0)),
        )
        .on_hover_text("A scene .zip exported from Skyfold Studio")
        .clicked()
    {
        crate::dialogs::pick_file(
            "scene_open",
            rfd::AsyncFileDialog::new()
                .set_title("Open a Skyfold scene")
                .add_filter("Skyfold scene", &["zip"]),
        );
    }
    // (file dialogs answer on a later frame)
    if let Some(f) = crate::dialogs::take_file("scene_open") {
        match file::import_zip(&f) {
            Ok(id) => {
                s.scenes.reload();
                let name = s.scenes.get(&id).map(|x| x.name.clone()).unwrap_or(id);
                s.ed.toast(format!(
                    "{name} is in your scenes — click it, then the ground, to place it"
                ));
            }
            Err(e) => s.ed.toast(format!("Couldn't open that scene: {e}")),
        }
    }
    ui.add_space(8.0);
    if s.scenes.list.is_empty() {
        dim(
            ui,
            "No scenes yet. Decorate a spot, then Save a scene and draw a loop round it.",
        );
        return;
    }
    section(ui, &format!("Your scenes · {}", s.scenes.list.len()));
    let w = ((ui.available_width() - 7.0) / 2.0).floor();
    let list: Vec<(String, String)> = s
        .scenes
        .list
        .iter()
        .map(|x| (x.id.clone(), x.name.clone()))
        .collect();
    let placing = s.scenes.placing().map(str::to_string);
    for row in list.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(7.0, 7.0);
            for (id, name) in row {
                let on = placing.as_deref() == Some(id.as_str())
                    && s.ed.object_tool == ObjectTool::Place;
                let r = crate::thumbs::ui::tile(
                    ui,
                    &mut s.previews,
                    &Subject::Scene(id.clone()),
                    icon::MAP_TRIFOLD,
                    name,
                    on,
                    w,
                );
                if r.on_hover_text("Click, then click the ground to place it")
                    .clicked()
                {
                    if on {
                        s.scenes.mode = Mode::Off;
                    } else {
                        take_place_tool(s);
                        s.scenes.mode = Mode::Place(id.clone());
                        s.scenes.yaw = 0.0;
                    }
                }
            }
        });
        ui.add_space(4.0);
    }
}

/// "18 × 22 m · 42 pieces".
fn summary(sc: &file::SceneFile) -> String {
    let sz = sc.size();
    format!(
        "{} pieces · {:.0} × {:.0} m",
        sc.pieces.len(),
        sz.x.max(1.0),
        sz.y.max(1.0)
    )
}

/// What a scene needs, by source: (label, colour) lines such as "12 pieces · Skyfold Studio
/// Collection — $49.99 (owned)".
pub fn requirements(s: &UiState, sc: &file::SceneFile) -> Vec<(String, Color32)> {
    let m = crate::market::snapshot();
    let lib = s.lib.as_deref();
    let mut by: Vec<(String, usize, Color32)> = vec![];
    let bundled: std::collections::HashSet<&str> =
        sc.models.iter().map(|b| b.asset.as_str()).collect();
    for (asset, n) in sc.assets() {
        let (label, col) = if bundled.contains(asset.as_str()) {
            ("your own models (inside the scene)".to_string(), TEXT_DIM)
        } else if let Some(p) = m.pack_of(&asset) {
            match m.state(p) {
                crate::market::PackState::Free => (format!("{} (free)", p.name), GREEN),
                crate::market::PackState::Owned => {
                    (format!("{} — {} (yours)", p.name, p.price_label()), GREEN)
                }
                crate::market::PackState::Locked if m.usable(&asset) => {
                    (format!("{} — free samples", p.name), GREEN)
                }
                crate::market::PackState::Locked => (
                    format!("{} — {} (not owned)", p.name, p.price_label()),
                    GOLD,
                ),
            }
        } else if lib.is_some_and(|l| l.get(&asset).is_some()) {
            ("built-in models (free)".to_string(), GREEN)
        } else {
            ("models missing from this project".to_string(), GOLD)
        };
        match by.iter_mut().find(|b| b.0 == label) {
            Some(b) => b.1 += n,
            None => by.push((label, n, col)),
        }
    }
    by.into_iter()
        .map(|(l, n, c)| {
            (
                format!("{n} piece{} · {l}", if n == 1 { "" } else { "s" }),
                c,
            )
        })
        .collect()
}

/// The right panel while a scene is picked: its card and what can be done with it. True when it
/// showed (the Placement settings don't apply).
pub fn right_card(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let Some(id) = s.scenes.placing().map(str::to_string) else {
        return false;
    };
    if s.ed.object_tool != ObjectTool::Place {
        return false;
    }
    let Some(sc) = s.scenes.get(&id).cloned() else {
        s.scenes.mode = Mode::Off;
        return false;
    };
    let mut lines = vec![summary(&sc)];
    if !sc.description.is_empty() {
        lines.push(sc.description.clone());
    }
    crate::thumbs::ui::card(
        ui,
        &mut s.previews,
        &Subject::Scene(id.clone()),
        &sc.name,
        &lines,
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new(format!(
            "{}  Click the ground to place it",
            icon::CURSOR_CLICK
        ))
        .font(FontId::new(13.5, semibold()))
        .color(Color32::WHITE),
    );
    dim(
        ui,
        &format!(
            "Z / X turn it ({:.0}°) · Esc stops. It lands as one set: drag it to move it, the knob turns it, Ungroup frees the pieces.",
            s.scenes.yaw.to_degrees().rem_euclid(360.0)
        ),
    );
    ui.add_space(4.0);
    for (l, c) in requirements(s, &sc) {
        ui.label(RichText::new(l).small().color(c));
    }
    separator(ui);
    section(ui, "Share");
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(format!("{}  Export…", icon::EXPORT))
            .on_hover_text("A scene file (.zip) that opens in any Skyfold Studio")
            .clicked()
        {
            crate::dialogs::save_file(
                "scene_export_zip",
                rfd::AsyncFileDialog::new()
                    .set_title("Export scene")
                    .set_file_name(format!("{}.zip", sc.name))
                    .add_filter("Skyfold scene", &["zip"]),
            );
        }
        if ui
            .button(format!("{}  For game engines…", icon::CUBE))
            .on_hover_text(
                "One .glb model with every piece in place, for Unreal, Unity, Godot or Blender",
            )
            .clicked()
        {
            crate::dialogs::save_file(
                "scene_export_glb",
                rfd::AsyncFileDialog::new()
                    .set_title("Export scene as a model")
                    .set_file_name(format!("{}.glb", sc.name))
                    .add_filter("glTF binary", &["glb"]),
            );
        }
    });
    if let Some(to) = crate::dialogs::take_file("scene_export_zip") {
        match file::export_zip(&id, &to) {
            Ok(bytes) => s.ed.toast(format!(
                "Exported {} ({:.1} MB)",
                sc.name,
                bytes as f64 / 1e6
            )),
            Err(e) => s.ed.toast(format!("Couldn't export: {e}")),
        }
    }
    if let Some(to) = crate::dialogs::take_file("scene_export_glb") {
        s.scenes.export_glb = Some((id.clone(), to));
        s.ed.toast("Exporting…");
    }
    if ui
        .add(
            egui::Button::new(format!("{}  List on marketplace…", icon::STOREFRONT))
                .min_size(vec2(ui.available_width(), 26.0)),
        )
        .clicked()
    {
        s.scenes.sell = Some(id.clone());
        s.scenes.sell_ok = false;
    }
    separator(ui);
    ui.horizontal(|ui| {
        if ui
            .small_button(format!("{}  Rename", icon::PENCIL_SIMPLE))
            .clicked()
        {
            s.scenes.rename = Some((id.clone(), sc.name.clone()));
        }
        let armed = s.scenes.confirm_delete.as_deref() == Some(id.as_str());
        let label = if armed {
            format!("{}  Really delete?", icon::TRASH)
        } else {
            format!("{}  Delete", icon::TRASH)
        };
        if ui
            .add(egui::Button::new(
                RichText::new(label).color(DANGER).small(),
            ))
            .on_hover_text("Removes it from your scenes (placed copies stay)")
            .clicked()
        {
            if armed {
                match file::delete(&id) {
                    Ok(()) => {
                        s.scenes.mode = Mode::Off;
                        s.scenes.reload();
                        s.ed.toast(format!("Deleted {}", sc.name));
                    }
                    Err(e) => s.ed.toast(e),
                }
                s.scenes.confirm_delete = None;
            } else {
                s.scenes.confirm_delete = Some(id.clone());
            }
        }
    });
    separator(ui);
    true
}

/// The Save, Rename and List on marketplace dialogs (every frame).
pub fn windows(ctx: &egui::Context, s: &mut UiState) {
    save_dialog(ctx, s);
    rename_dialog(ctx, s);
    sell_dialog(ctx, s);
}

fn save_dialog(ctx: &egui::Context, s: &mut UiState) {
    let Some(mut draft) = s.scenes.save.take() else {
        return;
    };
    draft.ids.retain(|id| s.store.items.contains_key(id));
    if draft.ids.is_empty() {
        return;
    }
    // what's in it: the most common pieces
    let mut counts: Vec<(String, usize)> = vec![];
    for id in &draft.ids {
        let Some(p) = s.store.items.get(id) else {
            continue;
        };
        let name = s
            .lib
            .as_deref()
            .and_then(|l| l.get(&p.asset))
            .map(|e| e.name.clone())
            .unwrap_or_else(|| p.asset.clone());
        match counts.iter_mut().find(|c| c.0 == name) {
            Some(c) => c.1 += 1,
            None => counts.push((name, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let (mut save, mut cancel) = (false, false);
    let mut open = true;
    egui::Window::new(format!("{}  Save as a scene", icon::FLOPPY_DISK)).id(egui::Id::new("scene_save")).open(&mut open).collapsible(false).resizable(false).default_width(340.0).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0)).show(ctx, |ui| {
        ui.label(RichText::new(format!("{} objects, kept exactly where they stand", draft.ids.len())).strong());
        let list: Vec<String> = counts.iter().take(6).map(|(n, c)| format!("{c}× {n}")).collect();
        dim(ui, &format!("{}{}", list.join(" · "), if counts.len() > 6 { " · …" } else { "" }));
        ui.add_space(6.0);
        ui.label(RichText::new("Name").small().color(TEXT_DIM));
        let r = ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(f32::INFINITY));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            save = true;
        }
        ui.label(RichText::new("Description (optional)").small().color(TEXT_DIM));
        ui.add(egui::TextEdit::multiline(&mut draft.description).desired_rows(2).desired_width(f32::INFINITY).hint_text("A moonlit forest shrine with lanterns and offerings"));
        ui.add_space(6.0);
        dim(ui, "Your own imported models are copied into the scene. Models from asset packs are named, not copied: whoever places it needs those packs.");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            save |= ui.add(egui::Button::new(RichText::new(format!("{}  Save scene", icon::FLOPPY_DISK)).color(ON_ACCENT)).fill(ACCENT)).clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if !open || cancel {
        return;
    }
    if !save {
        s.scenes.save = Some(draft);
        return;
    }
    let Some(lib) = s.lib.as_deref() else { return };
    let r = file::capture(
        &s.store,
        &s.terrain,
        &draft.ids,
        draft.outline.as_deref(),
        &draft.name,
        &draft.description,
    )
    .and_then(|sc| file::save_new(sc, lib));
    match r {
        Ok(id) => {
            s.scenes.reload();
            let name = s
                .scenes
                .get(&id)
                .map(|x| x.name.clone())
                .unwrap_or_default();
            info!("SCENE saved {id}: {} objects", draft.ids.len());
            s.ed.toast(format!("Saved {name} — find it in Objects › Scenes"));
            s.market.browse = crate::market::ui::Browse::Scenes;
        }
        Err(e) => s.ed.toast(format!("Couldn't save the scene: {e}")),
    }
}

fn rename_dialog(ctx: &egui::Context, s: &mut UiState) {
    let Some((id, mut name)) = s.scenes.rename.take() else {
        return;
    };
    let (mut ok, mut open) = (false, true);
    egui::Window::new("Rename scene")
        .id(egui::Id::new("scene_rename"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0))
        .show(ctx, |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0));
            r.request_focus();
            ok = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                || ui.button("Rename").clicked();
        });
    if !open {
        return;
    }
    if !ok || name.trim().is_empty() {
        s.scenes.rename = Some((id, name));
        return;
    }
    if let Some(mut sc) = file::read(&id) {
        sc.name = name.trim().to_string();
        match file::write(&sc) {
            Ok(()) => s.scenes.reload(),
            Err(e) => s.ed.toast(format!("Couldn't rename it: {e}")),
        }
    }
}

/// List on marketplace: what a buyer needs, the rights question for the user's own models, then
/// the files to upload and the website's Creator studio.
fn sell_dialog(ctx: &egui::Context, s: &mut UiState) {
    let Some(id) = s.scenes.sell.clone() else {
        return;
    };
    let Some(sc) = s.scenes.get(&id).cloned() else {
        s.scenes.sell = None;
        return;
    };
    let req = requirements(s, &sc);
    let paid_packs: Vec<String> = req
        .iter()
        .filter(|(l, _)| l.contains(" — $") || l.contains("(not owned)") || l.contains("(yours)"))
        .map(|(l, _)| l.clone())
        .collect();
    let own = !sc.models.is_empty();
    let mut open = true;
    let mut package = false;
    egui::Window::new(format!("{}  List {} on the marketplace", icon::STOREFRONT, sc.name)).id(egui::Id::new("scene_sell")).open(&mut open).collapsible(false).resizable(false).default_width(380.0).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, -40.0)).show(ctx, |ui| {
        dim(ui, "Scenes are listed on skyfoldstudio.com as packs: free or paid, your choice there. Buyers get the scene in their Skyfold Studio (Objects › Scenes), placed exactly as you laid it out.");
        ui.add_space(6.0);
        ui.label(RichText::new("What's in it").strong());
        for (l, c) in &req {
            ui.label(RichText::new(l).small().color(*c));
        }
        if !paid_packs.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("{}  Buyers need those paid packs: say so in the listing. Only your own models are inside the file.", icon::INFO)).small().color(GOLD));
        }
        ui.add_space(6.0);
        if own {
            toggle(ui, &mut s.scenes.sell_ok, "I made the imported models in this scene, or have the right to share or sell them");
        } else {
            s.scenes.sell_ok = true;
            dim(ui, "Every model in it comes from Skyfold Studio or its free libraries: the layout is yours to list.");
        }
        ui.add_space(8.0);
        package = ui.add_enabled(s.scenes.sell_ok, egui::Button::new(RichText::new(format!("{}  Prepare the listing", icon::PACKAGE)).color(ON_ACCENT)).fill(ACCENT)).on_hover_text("Saves the scene file and a cover picture, then opens your Creator studio").clicked();
        dim(ui, "Then, in the Creator studio: New listing › Pack, upload the .zip, add the cover picture, set a price (or free) and publish.");
    });
    if !open {
        s.scenes.sell = None;
        return;
    }
    if package {
        crate::dialogs::pick_folder(
            "scene_listing",
            rfd::AsyncFileDialog::new().set_title("Where to save the listing files"),
        );
    }
    // (the dialog answers on a later frame)
    let Some(folder) = crate::dialogs::take_file("scene_listing") else {
        return;
    };
    let safe: String = sc
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let zip = folder.join(format!("{} (Skyfold scene).zip", safe.trim()));
    match file::export_zip(&id, &zip) {
        Ok(_) => {
            let thumb = file::folder(&id).join("thumb.png");
            let cover = folder.join(format!("{} cover.png", safe.trim()));
            let have_cover = std::fs::copy(&thumb, &cover).is_ok();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(&zip)
                .spawn();
            crate::market::ui::open_url(&format!("{}/studio.html", crate::market::site::base()));
            s.ed.toast(if have_cover { format!("Saved {} and its cover — upload them in your Creator studio (opened in your browser)", zip.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()) } else { "Saved the scene file — upload it in your Creator studio (opened in your browser)".to_string() });
            s.scenes.sell = None;
        }
        Err(e) => s.ed.toast(format!("Couldn't package it: {e}")),
    }
}
