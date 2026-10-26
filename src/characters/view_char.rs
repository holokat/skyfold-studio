//! The codex inspector for a model (a library character): Use as player / Add as NPC, and its
//! Rig, Motion, Animations, Moves and Export.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bevy_egui::egui;
use egui::{CornerRadius, vec2};

use super::anims::Category;
use super::moves::{self, Move};
use super::view::{CharTab, Mode, Placing, Sel, ViewData};
use super::{AnimLibrary, CharacterMeta, Entry, RigKind};
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// The picture, name (editable) and facts at the top of an inspector.
pub(super) fn header(
    ui: &mut egui::Ui,
    d: &mut ViewData,
    character: Option<&str>,
    name: &mut String,
    editable: bool,
    sub: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let tex = character
            .and_then(|c| d.chars.get(c))
            .and_then(|e| d.thumbs.get(&e.rig_key()));
        let (r, _) = ui.allocate_exact_size(vec2(58.0, 58.0), egui::Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(12), WELL);
        if let Some(t) = tex {
            ui.painter().image(
                t,
                r.shrink(1.0),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            paint_icon(ui, r.center(), "User", 22.0, TEXT_DIM);
        }
        ui.vertical(|ui| {
            if editable {
                let resp = ui.add(
                    egui::TextEdit::singleline(name)
                        .font(egui::FontId::proportional(17.0))
                        .frame(egui::Frame::NONE)
                        .desired_width(ui.available_width()),
                );
                changed = resp.changed();
            } else {
                ui.label(egui::RichText::new(name.as_str()).size(17.0).color(TEXT).strong());
            }
            hint_small(ui, sub);
        });
    });
    changed
}

pub(super) fn facts(e: &Entry) -> String {
    let mut f = vec![];
    if e.meta.triangles > 0 {
        f.push(format!("{} triangles", super::view::group(e.meta.triangles)));
    }
    if e.meta.height > 0.0 {
        f.push(format!("{:.2} m tall", e.meta.height));
    }
    f.push(if e.builtin.is_some() {
        "Quaternius, CC0".to_string()
    } else {
        e.meta.source.clone()
    });
    f.join(" · ")
}

pub(super) fn inspector(ui: &mut egui::Ui, d: &mut ViewData, id: &str, now: f64) {
    let Some(e) = d.chars.get(id).cloned() else {
        return;
    };
    let mut name = e.meta.name.clone();
    let renamed = header(ui, d, Some(id), &mut name, e.builtin.is_none(), &facts(&e));
    if renamed && !name.trim().is_empty() {
        let mut m = e.meta.clone();
        m.name = name.trim().to_string();
        d.chars.save_meta(&m);
    }
    for n in &e.meta.notes {
        dim(ui, n);
    }
    ui.add_space(8.0);
    // what it's for
    let player = d.world.player.as_deref() == Some(id);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 6.0) * 0.5;
        ui.allocate_ui(vec2(w, 32.0), |ui| {
            if player {
                if wide_icon_button(ui, "CheckCircle2", "The player").on_hover_text("Walk mode plays as it. Click to stop.").clicked() {
                    d.world.player = None;
                    d.meta.dirty = true;
                }
            } else if primary_icon_button(ui, "Gamepad", "Use as player").clicked() {
                if e.rigged() {
                    d.world.player = Some(id.to_string());
                    d.meta.dirty = true;
                    d.v.note(format!("Walk mode now plays as {}", e.meta.name), false, now);
                } else {
                    d.v.note("Rig it first", true, now);
                }
            }
        });
        ui.allocate_ui(vec2(w, 32.0), |ui| {
            if wide_icon_button(ui, "MapPin", "Add as NPC").clicked() {
                if e.rigged() {
                    d.v.placing = Some(Placing {
                        character: id.to_string(),
                    });
                } else {
                    d.v.note("Rig it first", true, now);
                }
            }
        });
    });
    // creatures that look like it
    let looks: Vec<String> = d
        .gp
        .creatures
        .iter()
        .filter(|c| c.character.as_deref() == Some(id))
        .map(|c| c.name.clone())
        .collect();
    let npcs = d.gp.npcs.iter().filter(|n| n.life.character.as_deref() == Some(id)).count();
    let mut uses = vec![];
    if npcs > 0 {
        uses.push(format!("{npcs} NPC{}", if npcs == 1 { "" } else { "s" }));
    }
    if !looks.is_empty() {
        uses.push(format!("creature{}: {}", if looks.len() == 1 { "" } else { "s" }, looks.join(", ")));
    }
    if !uses.is_empty() {
        hint_small(ui, &format!("Used by {}", uses.join(" · ")));
    }
    ui.add_space(8.0);
    let mut tab = d.v.char_tab;
    if segmented(
        ui,
        &mut tab,
        &[
            (CharTab::Rig, "Rig"),
            (CharTab::Motion, "Motion"),
            (CharTab::Animations, "Anims"),
            (CharTab::Moves, "Moves"),
            (CharTab::Export, "Export"),
        ],
    ) {
        d.v.char_tab = tab;
    }
    ui.add_space(8.0);
    match d.v.char_tab {
        CharTab::Rig => rig_tab(ui, d, &e, now),
        CharTab::Motion => super::view_motion::motion_tab(ui, d, &e, now),
        CharTab::Animations => animations_tab(ui, d, &e, now),
        CharTab::Moves => {
            let mut own = e.meta.moves.clone();
            if moves_editor(ui, d, &mut own, None, "char_moves") {
                let mut m = e.meta.clone();
                m.moves = own;
                d.chars.save_meta(&m);
            }
        }
        CharTab::Export => export_tab(ui, d, &e),
    }
}

fn rig_tab(ui: &mut egui::Ui, d: &mut ViewData, e: &Entry, now: f64) {
    let id = e.meta.id.as_str();
    let busy = d.chars.busy(id).map(|j| j.status.lock().unwrap().clone());
    section_box(ui, "char_model_rig", "Model & rig", |ui| {
        stat_row(ui, "File", &e.meta.source);
        stat_row(ui, "Skeleton", "Humanoid (Unreal 5 names)");
        let status = match (&busy, e.meta.rig) {
            (Some(s), _) => s.clone(),
            _ if e.builtin.is_some() => "Rigged by hand".into(),
            (_, RigKind::Auto) => "Rigged automatically".into(),
            (_, RigKind::Imported) => "Its own rig".into(),
            (_, RigKind::None) => "Not rigged".into(),
        };
        stat_row(ui, "Rig", &status);
        if busy.is_some() {
            ui.ctx().request_repaint();
            return;
        }
        if e.builtin.is_some() {
            return;
        }
        ui.add_space(4.0);
        if small_icon_button(ui, "FolderOpen", "Show its files").clicked() {
            let _ = std::process::Command::new("/usr/bin/open").arg(e.dir()).spawn();
        }
    });
    if busy.is_some() || e.builtin.is_some() {
        return;
    }
    ui.add_space(8.0);
    match (d.v.mode, e.meta.rig) {
        (Mode::Markers, _) => {
            dim(ui, "Drag the dots onto the chin, wrists, elbows, groin and knees.");
            let mut both = !d.v.one_side;
            if toggle(ui, &mut both, "Move both sides together").changed() {
                d.v.one_side = !both;
            }
            ui.add_space(4.0);
            if primary_icon_button(ui, "MagicWand", "Rig with these markers").clicked() {
                let m = d.v.markers.as_ref().map(|(_, m)| *m);
                match d.chars.rig(id, m) {
                    Ok(()) => d.v.mode = Mode::Preview,
                    Err(err) => d.v.note(err, true, now),
                }
            }
            ui.horizontal(|ui| {
                if small_button(ui, "Guess again").clicked()
                    && let Some(model) = d.chars.prepared_model(id)
                {
                    d.v.markers = Some((id.to_string(), autorig::markers::guess(&model)));
                }
                if small_button(ui, "Cancel").clicked() {
                    d.v.mode = Mode::Preview;
                    d.v.markers = None;
                }
            });
        }
        (_, rig) => {
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) * 0.5;
                ui.allocate_ui(vec2(w, 32.0), |ui| {
                    let label = if rig == RigKind::None { "Auto-rig" } else { "Auto-rig again" };
                    if wide_icon_button(ui, "MagicWand", label).clicked()
                        && let Err(err) = d.chars.rig(id, None)
                    {
                        d.v.note(err, true, now);
                    }
                });
                ui.allocate_ui(vec2(w, 32.0), |ui| {
                    if wide_icon_button(ui, "Target", "Edit rig").clicked() {
                        d.v.mode = Mode::Markers;
                        if rig == RigKind::Imported {
                            d.v.markers = None;
                        }
                    }
                });
            });
            hint_small(
                ui,
                "Arms or legs bending wrong? Edit rig: drag the body markers, then rig again.",
            );
        }
    }
    ui.add_space(14.0);
    if small_icon_button(ui, "TrashCan", "Delete this character").clicked() {
        match d.chars.delete(id) {
            Ok(()) => {
                if d.world.player.as_deref() == Some(id) {
                    d.world.player = None;
                }
                d.v.sel = None;
            }
            Err(err) => d.v.note(err, true, now),
        }
    }
}

fn animations_tab(ui: &mut egui::Ui, d: &mut ViewData, e: &Entry, now: f64) {
    if !d.lib.ensure() {
        dim(ui, "Loading the library…");
        ui.ctx().request_repaint();
        return;
    }
    // its set first: the clips it exports with
    let mut meta: CharacterMeta = e.meta.clone();
    if !meta.export_anims.is_empty() {
        hint_small(ui, "Its animations");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for k in meta.export_anims.clone() {
                let Some(a) = d.lib.find(&k) else { continue };
                let on = d.v.mode == Mode::Preview && d.stage.anim.as_deref() == Some(k.as_str());
                if chip(ui, &a.label, None, on).clicked() {
                    d.stage.anim = Some(k.clone());
                    d.stage.playing = true;
                    d.stage.seek = Some(0.0);
                }
            }
        });
        ui.add_space(6.0);
    }
    search_field(ui, &mut d.v.search, "Search animations");
    ui.add_space(4.0);
    let mut cat = d.v.category;
    let mut opts: Vec<(Option<Category>, &str, Option<usize>)> = vec![(None, "All", None)];
    for c in Category::ALL {
        let n = d.lib.anims.iter().filter(|a| a.category == c).count();
        if n > 0 {
            opts.push((Some(c), c.name(), Some(n)));
        }
    }
    if chips(ui, &mut cat, &opts) {
        d.v.category = cat;
    }
    ui.add_space(4.0);
    let q = d.v.search.to_lowercase();
    let rows: Vec<(String, String, f32)> = d
        .lib
        .anims
        .iter()
        .filter(|a| d.v.category.is_none_or(|c| a.category == c))
        .filter(|a| q.is_empty() || a.label.to_lowercase().contains(&q))
        .map(|a| (a.key.clone(), a.label.clone(), a.seconds))
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("char_anim_list")
        .max_height(260.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (key, label, secs) in &rows {
                let sel = d.stage.anim.as_deref() == Some(key.as_str());
                let mine = meta.export_anims.contains(key);
                let m = format!("{}{secs:.1}s", if mine { "● " } else { "" });
                let lead = if sel { Lead::Icon("Play") } else { Lead::None };
                if list_row(ui, lead, label, "", &m, sel).clicked() {
                    d.stage.anim = Some(key.clone());
                    d.stage.playing = true;
                    d.stage.seek = Some(0.0);
                    d.v.mode = Mode::Preview;
                }
            }
        });
    if let Some(k) = d.stage.anim.clone()
        && d.lib.find(&k).is_some()
    {
        ui.add_space(4.0);
        let mut on = meta.export_anims.contains(&k);
        if toggle(ui, &mut on, "In its animations (exported with it)").changed() {
            if on {
                meta.export_anims.push(k.clone());
            } else {
                meta.export_anims.retain(|x| *x != k);
            }
            d.chars.save_meta(&meta);
        }
    }
    ui.add_space(6.0);
    if let Some(slot) = &d.v.anim_import {
        let done = slot.lock().unwrap().take();
        match done {
            Some(Ok(keys)) => {
                d.v.anim_import = None;
                d.lib.reload();
                d.v.category = Some(Category::Mine);
                d.v.note(format!("Added {} animations", keys.len()), false, now);
            }
            Some(Err(err)) => {
                d.v.anim_import = None;
                d.v.note(err, true, now);
            }
            None => {
                dim(ui, "Importing animations…");
                ui.ctx().request_repaint();
            }
        }
    } else if wide_icon_button(ui, "Import", "Import my animations…").clicked()
        && let Some(paths) = rfd::FileDialog::new()
            .set_title("Import animations (FBX or glTF, any humanoid rig)")
            .add_filter("Animations", &["fbx", "glb", "gltf"])
            .pick_files()
    {
        let slot = Arc::new(Mutex::new(None));
        let out = slot.clone();
        let paths: Vec<PathBuf> = paths;
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(AnimLibrary::import_files(&paths));
        });
        d.v.anim_import = Some(slot);
    }
    hint_small(ui, "Mixamo downloads or clips you own: they stay on this Mac.");
}

/// The clip per move: a row per move with a menu of the library. `inherit` names what an
/// unset move falls back to ("the character's"); `None`: the library default. Returns whether
/// anything changed.
pub(super) fn moves_editor(
    ui: &mut egui::Ui,
    d: &mut ViewData,
    own: &mut std::collections::BTreeMap<String, String>,
    inherit: Option<&std::collections::BTreeMap<String, String>>,
    salt: &str,
) -> bool {
    if !d.lib.ensure() {
        dim(ui, "Loading the library…");
        ui.ctx().request_repaint();
        return false;
    }
    let mut changed = false;
    for group in ["Moving", "Fighting", "Living"] {
        section_fold(ui, (salt, group), group, "", |ui| {
            for m in Move::ALL.into_iter().filter(|m| m.group() == group) {
                let fallback = match inherit {
                    Some(c) => moves::anim_for(c, m),
                    None => m.default_anim().to_string(),
                };
                let cur = own.get(m.id()).cloned();
                let shown = cur.clone().unwrap_or(fallback.clone());
                let label = d
                    .lib
                    .find(&shown)
                    .map(|a| a.label.clone())
                    .unwrap_or_else(|| shown.clone());
                let text = if cur.is_some() { label } else { format!("{label} (default)") };
                ui.horizontal(|ui| {
                    if small_icon_button(ui, "Play", "").on_hover_text("Preview").clicked() {
                        d.stage.anim = Some(shown.clone());
                        d.stage.playing = true;
                        d.stage.seek = Some(0.0);
                    }
                    let mut pick: Option<Option<String>> = None;
                    dropdown_row(ui, m.label(), (salt, m.id()), text, |ui| {
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            if menu_item(ui, "Default", "", cur.is_none()).clicked() {
                                pick = Some(None);
                            }
                            for a in &d.lib.anims {
                                if menu_item(ui, a.label.as_str(), a.category.name(), cur.as_deref() == Some(a.key.as_str()))
                                    .clicked()
                                {
                                    pick = Some(Some(a.key.clone()));
                                }
                            }
                        });
                    });
                    if let Some(p) = pick {
                        match p {
                            Some(k) => {
                                own.insert(m.id().to_string(), k);
                            }
                            None => {
                                own.remove(m.id());
                            }
                        }
                        changed = true;
                    }
                });
            }
        });
    }
    changed
}

fn export_tab(ui: &mut egui::Ui, d: &mut ViewData, e: &Entry) {
    let mut meta = e.meta.clone();
    let mut ex = meta.export;
    if toggle(ui, &mut ex, "Include in exports").changed() {
        meta.export = ex;
        d.chars.save_meta(&meta);
    }
    hint_small(
        ui,
        "Exports write it as a GLB with its skeleton (Unreal 5 bone names), skin and animations, plus characters.json. NPCs and the player are always included.",
    );
    ui.add_space(6.0);
    hint_small(ui, &format!("{} animations go with it:", meta.export_anims.len()));
    let mut remove = None;
    for k in &meta.export_anims {
        let label = d.lib.find(k).map(|a| a.label.clone()).unwrap_or_else(|| k.clone());
        ui.horizontal(|ui| {
            dim(ui, &label);
            if small_icon_button(ui, "CrossSmall", "").on_hover_text("Leave out").clicked() {
                remove = Some(k.clone());
            }
        });
    }
    if let Some(k) = remove {
        meta.export_anims.retain(|x| *x != k);
        d.chars.save_meta(&meta);
    }
    let _ = Sel::Player;
}
