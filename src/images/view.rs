//! The Images window (one big window right of the rail, like the Characters lab):
//!
//! - Left: the Generate box (prompt, model, count, aspect, reference pictures), running jobs, and
//!   Image models (which backends are set up, their keys, extra model ids).
//! - Middle: the library as cards, with search, Favourites / Generated / Imported and tag chips.
//! - Right: the inspector for the picked image: big preview, name, favourite, notes, tags, its
//!   prompt and model, Use as reference / Vary / Edit with prompt, duplicate, reveal, delete.
//!
//! Pictures dropped on the window while it's open are imported.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use egui::{ColorImage, CornerRadius, Rect, RichText, Sense, TextureHandle, TextureId, pos2, vec2};

use super::backends::{self, BackendKind, SizeStyle};
use super::jobs::Stage;
use super::library::{self, EXTS, ImageMeta};
use super::{GenerateArgs, Images};
use crate::editor::{Editor, Tab};
use crate::ui::theme::*;
use crate::ui::widgets::*;

const TIP: &str = "images_intro";

#[derive(Clone, Debug, PartialEq, Eq, Default, Copy)]
pub enum Filter {
    #[default]
    All,
    Favourites,
    Generated,
    Imported,
}

#[derive(Resource, Default)]
pub struct ImageView {
    pub selected: Option<String>,
    pub search: String,
    pub filter: Filter,
    pub tag: Option<String>,
    /// the Generate box
    pub prompt: String,
    pub refs: Vec<String>,
    /// "Edit with prompt" / "Vary": the picture the next ones come from
    pub parent: Option<String>,
    /// edit buffers of the selected picture (`buf_id`: whose they are; None reloads them)
    pub buf_id: Option<String>,
    name: String,
    notes: String,
    tag_input: String,
    /// notes typed, saved this long after the last key
    notes_dirty: Option<f64>,
    confirm_delete: Option<String>,
    /// Image models inputs (keys are cleared once stored)
    cf_account: Option<String>,
    cf_token: String,
    bfl_key: String,
    custom_backend: Option<BackendKind>,
    custom_id: String,
    /// the Image models setup: shown only when asked for, one service at a time
    pub models_open: bool,
    pub models_service: Option<BackendKind>,
    more_models: bool,
    focus_prompt: bool,
    /// (message, error?, until)
    note: Option<(String, bool, f64)>,
    tex: Textures,
}

impl ImageView {
    pub fn select(&mut self, id: Option<String>) {
        if self.selected != id {
            self.selected = id;
            self.buf_id = None;
            self.confirm_delete = None;
        }
    }
    fn say(&mut self, msg: impl Into<String>, err: bool, now: f64) {
        self.note = Some((msg.into(), err, now + if err { 8.0 } else { 4.0 }));
    }
}

pub struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            view.after(crate::ui::editor_ui).run_if(crate::mcp::ui_visible),
        );
    }
}

// ------------------------------------------------------------------ textures

enum Slot {
    Waiting,
    Loading,
    Ready(TextureHandle, [usize; 2]),
    Missing,
}

/// Card and preview textures, decoded off the frame thread (a few at a time) and cached by
/// file and size.
#[derive(Default)]
struct Textures {
    slots: HashMap<String, Slot>,
    queue: Vec<(String, PathBuf, u32)>,
    loading: usize,
    news: Arc<Mutex<Vec<(String, Option<ColorImage>)>>>,
}

impl Textures {
    fn key(path: &Path, max: u32) -> String {
        format!("{}#{max}", path.display())
    }

    /// The picture at `path` at most `max` px a side (None while it loads).
    fn get(&mut self, path: &Path, max: u32) -> Option<(TextureId, [usize; 2])> {
        let key = Self::key(path, max);
        match self.slots.get(&key) {
            Some(Slot::Ready(t, s)) => return Some((t.id(), *s)),
            Some(_) => return None,
            None => {}
        }
        self.slots.insert(key.clone(), Slot::Waiting);
        self.queue.push((key, path.to_path_buf(), max));
        None
    }

    /// Takes decoded pictures and starts the next decodes.
    fn pump(&mut self, ctx: &egui::Context) {
        let done = std::mem::take(&mut *self.news.lock().unwrap_or_else(|e| e.into_inner()));
        for (k, img) in done {
            self.loading = self.loading.saturating_sub(1);
            let slot = match img {
                Some(img) => {
                    let size = img.size;
                    Slot::Ready(ctx.load_texture(format!("img_{k}"), img, egui::TextureOptions::LINEAR), size)
                }
                None => Slot::Missing,
            };
            self.slots.insert(k, slot);
            ctx.request_repaint();
        }
        // newest requests first (what's on screen now)
        while self.loading < 4 {
            let Some((k, path, max)) = self.queue.pop() else { break };
            self.loading += 1;
            self.slots.insert(k.clone(), Slot::Loading);
            let news = self.news.clone();
            std::thread::spawn(move || {
                let img = image::open(&path).ok().map(|im| {
                    let im = if im.width() > max || im.height() > max {
                        im.resize(max, max, image::imageops::FilterType::Triangle)
                    } else {
                        im
                    };
                    let rgba = im.to_rgba8();
                    ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())
                });
                news.lock().unwrap_or_else(|e| e.into_inner()).push((k, img));
            });
        }
    }

    /// Forgets a file's textures (it was deleted or replaced).
    fn forget(&mut self, path: &Path) {
        let p = format!("{}#", path.display());
        self.slots.retain(|k, _| !k.starts_with(&p));
    }
}

// ------------------------------------------------------------------ the window

fn window_rect(screen: Rect) -> Rect {
    Rect::from_min_max(
        pos2(screen.left() + crate::ui::shell::PANEL_X, screen.top() + crate::ui::shell::TOP),
        pos2(screen.right() - 12.0, screen.bottom() - 12.0),
    )
}

fn view(
    _main: bevy::ecs::system::NonSendMarker,
    mut contexts: EguiContexts,
    mut ed: ResMut<Editor>,
    mut images: ResMut<Images>,
    mut v: ResMut<ImageView>,
    walk: Res<crate::walk::WalkState>,
    time: Res<Time<Real>>,
) -> Result {
    if ed.tab != Tab::Images || walk.active {
        return Ok(());
    }
    let now = time.elapsed_secs_f64();
    let ctx = contexts.ctx_mut()?.clone();
    images.ensure();
    v.tex.pump(&ctx);
    take_dialogs(&mut images, &mut v, now);
    // the selection stays valid; the edit buffers follow it
    if v.selected.as_ref().is_some_and(|id| images.lib.get(id).is_none()) {
        v.select(None);
    }
    if v.buf_id != v.selected {
        save_notes(&mut images, &mut v);
        v.buf_id = v.selected.clone();
        if let Some(m) = v.selected.as_ref().and_then(|id| images.lib.get(id)) {
            v.name = m.name.clone();
            v.notes = m.notes.clone();
        }
        v.tag_input.clear();
        v.notes_dirty = None;
    }
    if v.notes_dirty.is_some_and(|t| now - t > 0.8) {
        save_notes(&mut images, &mut v);
    }
    if v.note.as_ref().is_some_and(|n| now > n.2) {
        v.note = None;
    }
    let rect = window_rect(ctx.content_rect());
    let left_w = 320.0;
    let right_w = 340.0;
    egui::Area::new(egui::Id::new("images_view"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(&ctx, |ui| {
            let frame = glass_solid_frame()
                .corner_radius(CornerRadius::same(16))
                .inner_margin(egui::Margin::same(12))
                .shadow(egui::epaint::Shadow::NONE);
            glass(ui, frame, |ui| {
                let inner = rect.size() - vec2(26.0, 26.0);
                ui.set_min_size(inner);
                ui.set_max_size(inner);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    column(ui, left_w, inner.y, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("images_left")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(left_w - 14.0);
                                left_column(ui, &mut ed, &mut images, &mut v, now);
                            });
                    });
                    let mid_w = (inner.x - left_w - right_w - 24.0).max(240.0);
                    column(ui, mid_w, inner.y, |ui| library_column(ui, &mut images, &mut v, now));
                    column(ui, right_w, inner.y, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("images_inspector")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(right_w - 14.0);
                                match v.selected.clone() {
                                    Some(id) => inspector(ui, &mut images, &mut v, &id, now),
                                    None => empty_inspector(ui, &images),
                                }
                            });
                    });
                });
            });
        });
    Ok(())
}

fn column(ui: &mut egui::Ui, w: f32, h: f32, body: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.expand(2.0).intersect(ui.clip_rect()));
    child.set_width(w);
    body(&mut child);
}

/// Answers of the file dialogs: imports, and reference pictures (imported, then referenced).
fn take_dialogs(images: &mut Images, v: &mut ImageView, now: f64) {
    if let Some(paths) = crate::dialogs::take_files("images_import") {
        import(images, v, &paths, now);
    }
    if let Some(paths) = crate::dialogs::take_files("images_ref") {
        match images.lib.import(&paths) {
            Ok(ms) => {
                for m in ms {
                    if !v.refs.contains(&m.id) {
                        v.refs.push(m.id);
                    }
                }
            }
            Err(e) => v.say(e, true, now),
        }
    }
}

fn import(images: &mut Images, v: &mut ImageView, paths: &[PathBuf], now: f64) {
    match images.lib.import(paths) {
        Ok(ms) => {
            if let Some(m) = ms.last() {
                v.select(Some(m.id.clone()));
            }
            let n = ms.len();
            v.say(format!("{n} picture{} imported", if n == 1 { "" } else { "s" }), false, now);
        }
        Err(e) => v.say(e, true, now),
    }
}

/// Pictures dropped on the window while Images is open are imported.
pub fn dropped_files(
    mut drops: MessageReader<bevy::window::FileDragAndDrop>,
    ed: Res<Editor>,
    mut images: ResMut<Images>,
    mut v: ResMut<ImageView>,
    time: Res<Time<Real>>,
) {
    let mut paths = vec![];
    for d in drops.read() {
        let bevy::window::FileDragAndDrop::DroppedFile { path_buf, .. } = d else { continue };
        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if ed.tab == Tab::Images && EXTS.contains(&ext.as_str()) {
            paths.push(path_buf.clone());
        }
    }
    if !paths.is_empty() {
        images.lib.ensure();
        import(&mut images, &mut v, &paths, time.elapsed_secs_f64());
    }
}

fn save_notes(images: &mut Images, v: &mut ImageView) {
    v.notes_dirty = None;
    let Some(id) = v.buf_id.clone() else { return };
    if images.lib.get(&id).is_some_and(|m| m.notes != v.notes) {
        let notes = v.notes.clone();
        let _ = images.lib.update(&id, |m| m.notes = notes);
    }
}

// ------------------------------------------------------------------ left: generate, jobs, models

fn left_column(ui: &mut egui::Ui, ed: &mut Editor, images: &mut Images, v: &mut ImageView, now: f64) {
    ui.horizontal(|ui| {
        title(ui, "Images");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::ui::tips::help_button(ui, TIP);
        });
    });
    hint_small(ui, "Concept art, references and mood boards, kept with your notes");
    ui.add_space(6.0);
    crate::ui::tips::card(
        ui,
        TIP,
        "Make pictures here",
        &[
            "Describe a picture in Generate and press Generate. It paints in the background.",
            "Pick a model: ChatGPT images works with your ChatGPT sign-in; FLUX runs on your Cloudflare or Black Forest Labs key (Image models, below).",
            "Pick a picture to add notes and tags, or to vary it, edit it with a prompt or use it as a reference.",
            "Drop picture files on the window to keep them here too.",
        ],
    );
    generate_box(ui, images, v, now);
    jobs_list(ui, images);
    ui.add_space(6.0);
    if v.models_open {
        models_section(ui, ed, images, v, now);
    } else {
        // (folded away: one line saying what's set up)
        let ready = BackendKind::ALL.iter().filter(|b| images.ready(**b)).count();
        ui.horizontal(|ui| {
            hint_small(ui, &format!("Image models · {ready} of 3 set up"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_button(ui, "Set up").clicked() {
                    v.models_open = true;
                }
            });
        });
    }
}

fn generate_box(ui: &mut egui::Ui, images: &mut Images, v: &mut ImageView, now: f64) {
    section_box(ui, "images_generate", "Generate", |ui| {
        if let Some(pid) = v.parent.clone() {
            let name = images.lib.get(&pid).map(|m| m.name.clone());
            match name {
                Some(n) => {
                    ui.horizontal(|ui| {
                        dim(ui, &format!("From “{n}”"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, "CrossSmall", "Start from scratch instead").clicked() {
                                v.parent = None;
                            }
                        });
                    });
                }
                None => v.parent = None,
            }
        }
        let r = ui.add(
            egui::TextEdit::multiline(&mut v.prompt)
                .hint_text(if v.parent.is_some() {
                    "Describe the change (e.g. “make it dusk, add lanterns”)"
                } else {
                    "Describe the picture…"
                })
                .desired_rows(4)
                .desired_width(ui.available_width()),
        );
        if std::mem::take(&mut v.focus_prompt) {
            r.request_focus();
        }
        ui.add_space(4.0);
        // the model
        let models = images.models();
        let cur = images.current_model();
        let cur_ready = images.ready(cur.backend);
        let mut pick: Option<String> = None;
        dropdown_row(ui, "Model", "images_model", cur.name.clone(), |ui| {
            ui.set_min_width(300.0);
            for b in BackendKind::ALL {
                let ready = images.ready(b);
                ui.label(
                    RichText::new(if ready { b.name().to_string() } else { format!("{} · not set up", b.name()) })
                        .size(11.0)
                        .color(TEXT_MUTE),
                );
                for m in models.iter().filter(|m| m.backend == b) {
                    if menu_item(ui, RichText::new(&m.name).color(if ready { TEXT } else { TEXT_DIM }), &m.note, m.key() == cur.key())
                        .clicked()
                    {
                        pick = Some(m.key());
                    }
                }
            }
        });
        if let Some(k) = pick {
            images.settings.model = k;
            images.settings.save();
        }
        if !cur_ready
            && let Some(st) = images.status.get(&cur.backend)
        {
            warn_line(ui, &format!("{}: {}", cur.backend.name(), st.note));
            // (straight to that service's setup)
            if small_button(ui, &format!("Set up {}", cur.backend.name())).clicked() {
                v.models_open = true;
                v.models_service = Some(cur.backend);
            }
        }
        // how many, what shape
        let mut count = images.settings.count.clamp(1, 4);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Count").color(TEXT_DIM));
            if segmented(ui, &mut count, &[(1, "1"), (2, "2"), (3, "3"), (4, "4")]) {
                images.settings.count = count;
                images.settings.save();
            }
        });
        if cur.size == SizeStyle::Square {
            hint_small(ui, "This model paints square pictures");
        } else {
            let mut aspect = backends::ASPECTS
                .iter()
                .position(|a| *a == images.settings.aspect)
                .unwrap_or(0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Shape").color(TEXT_DIM));
                let opts: Vec<(usize, &str)> = backends::ASPECTS.iter().copied().enumerate().collect();
                if segmented(ui, &mut aspect, &opts) {
                    images.settings.aspect = backends::ASPECTS[aspect].into();
                    images.settings.save();
                }
            });
        }
        // reference pictures
        ui.add_space(2.0);
        v.refs.retain(|id| images.lib.get(id).is_some());
        let max = cur.max_refs;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            let mut remove = None;
            for (i, id) in v.refs.iter().enumerate() {
                let file = images.lib.file(id);
                let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::click());
                let tex = file.and_then(|f| v.tex.get(&f, 360));
                match tex {
                    Some((t, s)) => {
                        let uv = cover_uv(vec2(1.0, 1.0), vec2(s[0] as f32, s[1] as f32));
                        egui::Image::new((t, rect.size()))
                            .uv(uv)
                            .corner_radius(CornerRadius::same(7))
                            .paint_at(ui, rect);
                    }
                    None => {
                        ui.painter().rect_filled(rect, CornerRadius::same(7), SOFT);
                    }
                }
                let dim_it = i >= max;
                if dim_it {
                    ui.painter().rect_filled(rect, CornerRadius::same(7), egui::Color32::from_black_alpha(150));
                }
                if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(7), egui::Color32::from_black_alpha(110));
                    paint_icon(ui, rect.center(), "CrossSmall", 16.0, TEXT);
                }
                if resp.on_hover_text("Remove this reference").clicked() {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                v.refs.remove(i);
            }
            if max > 0
                && v.refs.len() < max
                && icon_button(ui, "PlusSmall", "Add a reference picture from a file").clicked()
            {
                crate::dialogs::pick_files(
                    "images_ref",
                    rfd::AsyncFileDialog::new().add_filter("Pictures", &EXTS),
                );
            }
        });
        if max == 0 && !v.refs.is_empty() {
            hint_small(ui, "This model takes no references; they're left out");
        } else if v.refs.len() > max {
            hint_small(ui, &format!("{} takes {max} reference{}; the rest are left out", cur.name, if max == 1 { "" } else { "s" }));
        } else if v.refs.is_empty() && max > 0 {
            hint_small(ui, "References: pick a picture › Use as reference, or press +");
        }
        ui.add_space(6.0);
        let go = ui
            .add_enabled_ui(!v.prompt.trim().is_empty(), |ui| primary_icon_button(ui, "Sparkle", "Generate"))
            .inner;
        if go.clicked() {
            let args = GenerateArgs {
                prompt: v.prompt.clone(),
                model: None,
                count: images.settings.count,
                aspect: images.settings.aspect.clone(),
                seed: None,
                refs: v.refs.clone(),
                ref_files: vec![],
                parent: v.parent.clone(),
            };
            match images.generate(args) {
                Ok(_) => v.say("Painting… the pictures land in the library", false, now),
                Err(e) => v.say(e, true, now),
            }
        }
        if let Some((msg, err, _)) = &v.note {
            if *err {
                warn_line(ui, msg);
            } else {
                hint_small(ui, msg);
            }
        }
    });
}

fn jobs_list(ui: &mut egui::Ui, images: &mut Images) {
    if images.jobs.list.is_empty() {
        return;
    }
    ui.add_space(6.0);
    section(ui, "Jobs");
    let mut stop = None;
    let mut clear = false;
    for j in images.jobs.list.iter().rev() {
        let (icon, meta) = match j.stage {
            Stage::Running => ("LoadingCircle", "".to_string()),
            Stage::Done => ("CheckCircle2", format!("{}", j.images.len())),
            Stage::Failed => ("ExclamationTriangle", String::new()),
            Stage::Stopped => ("StopCircle", String::new()),
        };
        let title_text: String = j.prompt.chars().take(60).collect();
        let r = list_row(ui, Lead::Icon(icon), &title_text, &format!("{} · {}", j.model_name, j.note), &meta, false)
            .on_hover_text(&j.note);
        if j.stage == Stage::Running {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            if r.clicked() {
                stop = Some(j.id);
            }
        }
    }
    if images.jobs.running() > 0 {
        hint_small(ui, "Click a running job to stop it");
    } else if small_button(ui, "Clear").clicked() {
        clear = true;
    }
    if let Some(id) = stop
        && let Some(j) = images.jobs.get(id)
    {
        j.stop();
    }
    if clear {
        images.jobs.list.retain(|j| j.stage == Stage::Running);
    }
}

fn models_section(ui: &mut egui::Ui, ed: &mut Editor, images: &mut Images, v: &mut ImageView, now: f64) {
    section_box(ui, "images_models", "Image models", |ui| {
        ui.horizontal(|ui| {
            hint_small(ui, "Your own accounts; keys stay in your Mac's Keychain");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_button(ui, "Done").clicked() {
                    v.models_open = false;
                }
                if images.checking() {
                    ui.add(egui::Spinner::new().size(14.0));
                } else if icon_button(ui, "ArrowRotateClockwise", "Check the backends again").clicked() {
                    images.check_backends();
                }
            });
        });
        ui.add_space(4.0);
        // one service at a time
        let mut svc = v.models_service.unwrap_or(BackendKind::Cloudflare);
        segmented(
            ui,
            &mut svc,
            &[(BackendKind::Codex, "ChatGPT"), (BackendKind::Cloudflare, "Cloudflare"), (BackendKind::Bfl, "BFL")],
        );
        v.models_service = Some(svc);
        ui.add_space(4.0);
        backend_header(ui, images, svc);
        match svc {
            BackendKind::Codex => {
                hint_small(ui, "Uses the Codex CLI that comes with the ChatGPT app; no key needed.");
            }
            BackendKind::Cloudflare => {
                let account = v.cf_account.get_or_insert_with(|| images.settings.cloudflare_account.clone());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Account").color(TEXT_DIM));
                    let r = ui.add(
                        egui::TextEdit::singleline(account)
                            .hint_text("account id (32 characters)")
                            .desired_width(ui.available_width()),
                    );
                    if r.lost_focus() && *account != images.settings.cloudflare_account {
                        images.settings.cloudflare_account = account.trim().to_string();
                        images.settings.save();
                        images.check_backends();
                    }
                });
                key_row(ui, images, v, BackendKind::Cloudflare, now, ed);
            }
            BackendKind::Bfl => {
                let base = images.settings.bfl_base.clone();
                let label = backends::bfl::BASES
                    .iter()
                    .find(|(u, _)| *u == base)
                    .map_or("Global", |(_, l)| l);
                let mut pick = None;
                dropdown_row(ui, "Region", "images_bfl_region", label, |ui| {
                    for (u, l) in backends::bfl::BASES {
                        if menu_check(ui, u == base, l).clicked() {
                            pick = Some(u.to_string());
                        }
                    }
                });
                if let Some(u) = pick {
                    images.settings.bfl_base = u;
                    images.settings.save();
                }
                key_row(ui, images, v, BackendKind::Bfl, now, ed);
            }
        }
        ui.add_space(6.0);
        if !v.more_models {
            if small_button(ui, "Add a model id…").clicked() {
                v.more_models = true;
            }
            return;
        }
        // model ids the user adds (a new FLUX the day it ships)
        ui.add_space(6.0);
        section(ui, "More models");
        hint_small(ui, "Add any model id the service has, e.g. a new FLUX release");
        let cb = *v.custom_backend.get_or_insert(BackendKind::Cloudflare);
        let mut nb = cb;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("images_custom_backend")
                .selected_text(if cb == BackendKind::Cloudflare { "Cloudflare" } else { "BFL" })
                .width(96.0)
                .dropdown_ui(ui, |ui| {
                    menu_option(ui, &mut nb, BackendKind::Cloudflare, "Cloudflare");
                    menu_option(ui, &mut nb, BackendKind::Bfl, "BFL");
                });
            let hint = if cb == BackendKind::Cloudflare { "@cf/black-forest-labs/…" } else { "flux-…" };
            let r = ui.add(egui::TextEdit::singleline(&mut v.custom_id).hint_text(hint).desired_width(ui.available_width() - 34.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (icon_button(ui, "PlusSmall", "Add this model").clicked() || enter) && !v.custom_id.trim().is_empty() {
                let id = v.custom_id.trim().to_string();
                if backends::valid_model_id(&id) {
                    let key = format!("{}:{id}", cb.id());
                    if !images.settings.custom.contains(&key) {
                        images.settings.custom.push(key.clone());
                    }
                    images.settings.model = key;
                    images.settings.save();
                    v.custom_id.clear();
                } else {
                    v.say("A model id has no spaces or quotes", true, now);
                }
            }
        });
        v.custom_backend = Some(nb);
        let mut remove = None;
        for (i, key) in images.settings.custom.iter().enumerate() {
            ui.horizontal(|ui| {
                hint_small(ui, key);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "CrossSmall", "Remove this model").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            images.settings.custom.remove(i);
            images.settings.save();
        }
    });
}

fn backend_header(ui: &mut egui::Ui, images: &Images, b: BackendKind) {
    let st = images.status.get(&b);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
        let col = match st {
            Some(s) if s.ready => OK,
            Some(_) => WARN,
            None => TEXT_MUTE,
        };
        ui.painter().circle_filled(r.center(), 3.5, col);
        ui.label(RichText::new(b.name()).color(TEXT).strong());
    });
    if let Some(s) = st
        && !s.ready
    {
        hint_small(ui, &s.note);
    }
}

fn key_row(ui: &mut egui::Ui, images: &mut Images, v: &mut ImageView, b: BackendKind, now: f64, ed: &mut Editor) {
    let has = images.status.get(&b).is_some_and(|s| s.has_key);
    let buf = match b {
        BackendKind::Cloudflare => &mut v.cf_token,
        _ => &mut v.bfl_key,
    };
    let mut result = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Key").color(TEXT_DIM));
        let hint = match (b, has) {
            (_, true) => "stored · type to replace",
            (BackendKind::Cloudflare, false) => "API token",
            _ => "API key",
        };
        let r = ui.add(
            egui::TextEdit::singleline(buf)
                .password(true)
                .hint_text(hint)
                .desired_width(ui.available_width() - 64.0),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (small_button(ui, "Save").clicked() || enter) && !buf.trim().is_empty() {
            let key = std::mem::take(buf);
            result = Some(images.set_key(b, &key));
        }
    });
    match result {
        Some(Ok(())) => ed.toast(format!("{}: key stored in the Keychain", b.name())),
        Some(Err(e)) => v.say(e, true, now),
        None => {}
    }
    ui.horizontal(|ui| {
        hint_small(ui, "Get one at");
        ui.hyperlink_to(RichText::new(b.keys_at()).size(11.5), format!("https://{}", b.keys_at()));
        if has {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_button(ui, "Forget").on_hover_text("Remove the key from the Keychain").clicked() {
                    images.forget_key(b);
                }
            });
        }
    });
}

// ------------------------------------------------------------------ middle: the library

fn library_column(ui: &mut egui::Ui, images: &mut Images, v: &mut ImageView, now: f64) {
    let lib = &images.lib;
    let fav = lib.entries.iter().filter(|e| e.favourite).count();
    let gen_n = lib.entries.iter().filter(|e| e.generated()).count();
    let total = lib.entries.len();
    ui.horizontal(|ui| {
        ui.allocate_ui(vec2((ui.available_width() - 110.0).min(320.0), 30.0), |ui| {
            search_field(ui, &mut v.search, "Search names, prompts, notes, tags");
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_text_button(ui, "Import", "Import", None)
                .on_hover_text("Add picture files to the library (or drop them on the window)")
                .clicked()
            {
                crate::dialogs::pick_files("images_import", rfd::AsyncFileDialog::new().add_filter("Pictures", &EXTS));
            }
        });
    });
    ui.add_space(6.0);
    chips(
        ui,
        &mut v.filter,
        &[
            (Filter::All, "All", Some(total)),
            (Filter::Favourites, "Favourites", Some(fav)),
            (Filter::Generated, "Generated", Some(gen_n)),
            (Filter::Imported, "Imported", Some(total - gen_n)),
        ],
    );
    let tags = lib.tags();
    if !tags.is_empty() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            paint_tag_icon(ui);
            for (t, n) in tags.iter().take(24) {
                let on = v.tag.as_deref() == Some(t.as_str());
                if chip(ui, t, Some(*n), on).clicked() {
                    v.tag = if on { None } else { Some(t.clone()) };
                }
            }
        });
    }
    ui.add_space(8.0);
    let shown: Vec<&ImageMeta> = lib
        .entries
        .iter()
        .filter(|e| match v.filter {
            Filter::All => true,
            Filter::Favourites => e.favourite,
            Filter::Generated => e.generated(),
            Filter::Imported => !e.generated(),
        })
        .filter(|e| v.tag.as_ref().is_none_or(|t| e.tags.contains(t)))
        .filter(|e| e.matches(&v.search))
        .collect();
    // painting now: a placeholder card per picture on the way
    let painting: Vec<(String, String)> = images
        .jobs
        .list
        .iter()
        .filter(|j| j.stage == Stage::Running)
        .flat_map(|j| {
            let left = j.count.saturating_sub(j.images.len() as u32);
            (0..left).map(move |_| (j.prompt.clone(), j.note.clone()))
        })
        .collect();
    if shown.is_empty() && painting.is_empty() {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            paint_icon(ui, ui.cursor().center_top() + vec2(0.0, 14.0), "Images1", 28.0, TEXT_MUTE);
            ui.add_space(34.0);
            if total == 0 {
                dim(ui, "No pictures yet");
                hint_small(ui, "Describe one in Generate, or drop picture files here");
            } else {
                dim(ui, "Nothing matches");
            }
        });
        return;
    }
    let ids: Vec<(String, String, String, PathBuf, bool)> = shown
        .iter()
        .map(|e| {
            let sub = if e.generated() {
                model_name(&e.backend, &e.model)
            } else {
                "Imported".into()
            };
            (e.id.clone(), e.name.clone(), sub, library::Library::dir(&e.id).join(&e.file), e.favourite)
        })
        .collect();
    let np = painting.len();
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .id_salt("images_grid")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 6.0);
            let cols = ((ui.available_width() / 190.0).floor() as usize).clamp(2, 8);
            let tex = &mut v.tex;
            let sel = v.selected.clone();
            if let Some(i) = card_grid(ui, cols, np + ids.len(), |ui, i, w| {
                let size = vec2(w, (w * 0.8).round());
                if i < np {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                    let (_, note) = &painting[i];
                    return image_card(ui, None, "Painting…", note, false, size);
                }
                let (id, name, sub, path, fav) = &ids[i - np];
                let r = Rect::from_min_size(ui.cursor().min, size);
                let t = if ui.is_rect_visible(r) { tex.get(path, 360).map(|t| t.0) } else { None };
                let sub = if *fav { format!("Favourite · {sub}") } else { sub.clone() };
                image_card(ui, t, name, &sub, sel.as_deref() == Some(id.as_str()), size)
            }) && i >= np
            {
                clicked = Some(ids[i - np].0.clone());
            }
            ui.add_space(8.0);
        });
    if let Some(id) = clicked {
        save_notes(images, v);
        v.select(Some(id));
    }
    let _ = now;
}

fn paint_tag_icon(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(18.0, 24.0), Sense::hover());
    paint_icon(ui, r.center(), "Tag", 14.0, TEXT_MUTE);
}

/// A model's display name ("FLUX.2 [pro]"), or its id.
pub fn model_name(backend: &str, model: &str) -> String {
    BackendKind::from_id(backend)
        .and_then(|b| b.models().iter().find(|m| m.id == model))
        .map(|m| m.name.to_string())
        .unwrap_or_else(|| model.rsplit('/').next().unwrap_or(model).to_string())
}

fn cover_uv(card: egui::Vec2, img: egui::Vec2) -> Rect {
    let (ca, ia) = (card.x / card.y.max(1.0), img.x / img.y.max(1.0));
    if ia > ca {
        Rect::from_center_size(pos2(0.5, 0.5), vec2(ca / ia, 1.0))
    } else {
        Rect::from_center_size(pos2(0.5, 0.5), vec2(1.0, ia / ca))
    }
}

// ------------------------------------------------------------------ right: the inspector

fn empty_inspector(ui: &mut egui::Ui, images: &Images) {
    ui.label(RichText::new("No picture picked").size(16.0).color(TEXT).strong());
    ui.add_space(6.0);
    let steps: &[&str] = if images.lib.entries.is_empty() {
        &[
            "Describe a picture in Generate (left) and press Generate.",
            "Or drop picture files on the window to keep them here.",
        ]
    } else {
        &["Pick a picture to see it large, with its prompt, notes and tags."]
    };
    for (i, s) in steps.iter().enumerate() {
        dim(ui, &format!("{}. {s}", i + 1));
    }
}

fn inspector(ui: &mut egui::Ui, images: &mut Images, v: &mut ImageView, id: &str, now: f64) {
    let Some(m) = images.lib.get(id).cloned() else { return };
    let path = library::Library::dir(&m.id).join(&m.file);
    // the picture, fitted
    let w = ui.available_width();
    let tex = v.tex.get(&path, 1024);
    let aspect = match tex {
        Some((_, s)) => s[0] as f32 / s[1].max(1) as f32,
        None if m.height > 0 => m.width as f32 / m.height as f32,
        None => 1.0,
    };
    let h = (w / aspect).min(320.0);
    let pw = (h * aspect).min(w);
    let (row, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let rect = Rect::from_center_size(row.center(), vec2(pw, h));
    match tex {
        Some((t, _)) => {
            egui::Image::new((t, rect.size()))
                .corner_radius(CornerRadius::same(10))
                .paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, CornerRadius::same(10), SOFT);
        }
    }
    if resp.on_hover_text("Open in Preview").clicked() {
        let _ = std::process::Command::new("/usr/bin/open").arg(&path).spawn();
    }
    ui.add_space(8.0);
    // name and the quick actions
    let r = ui.add(
        egui::TextEdit::singleline(&mut v.name)
            .font(egui::FontId::new(15.0, semibold()))
            .desired_width(w),
    );
    if r.lost_focus() && v.name.trim() != m.name && !v.name.trim().is_empty() {
        let n = v.name.trim().to_string();
        let _ = images.lib.update(id, |e| e.name = n);
    }
    ui.horizontal(|ui| {
        if icon_toggle(ui, "Star", if m.favourite { "Favourite (click to unmark)" } else { "Mark as favourite" }, m.favourite).clicked() {
            let _ = images.lib.update(id, |e| e.favourite = !e.favourite);
        }
        if icon_button(ui, "FolderOpen", "Show in Finder").clicked() {
            images.lib.reveal(id);
        }
        if icon_button(ui, "SquareBehindSquare1", "Duplicate").clicked() {
            match images.lib.duplicate(id) {
                Ok(d) => v.select(Some(d.id)),
                Err(e) => v.say(e, true, now),
            }
        }
        if icon_button(ui, "TrashCan", "Delete (moves it to the Trash)").clicked() {
            v.confirm_delete = Some(id.to_string());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            hint_small(ui, &library::date(m.created));
        });
    });
    if v.confirm_delete.as_deref() == Some(id) {
        ui.horizontal(|ui| {
            warn_line(ui, "Move this picture to the Trash?");
            if small_button(ui, "Delete").clicked() {
                v.tex.forget(&path);
                match images.lib.delete(id) {
                    Ok(()) => v.select(None),
                    Err(e) => v.say(e, true, now),
                }
                v.confirm_delete = None;
            }
            if small_button(ui, "Cancel").clicked() {
                v.confirm_delete = None;
            }
        });
        if v.selected.is_none() {
            return;
        }
    }
    ui.add_space(6.0);
    // make more from it
    if wide_icon_button(ui, "Images1", "Use as reference")
        .on_hover_text("Hand this picture to the next generation (style, subject or a base to edit)")
        .clicked()
        && !v.refs.iter().any(|r| r == id)
    {
        v.refs.push(id.to_string());
        v.say("Added as a reference in Generate", false, now);
    }
    ui.horizontal(|ui| {
        let half = ((ui.available_width() - ui.spacing().item_spacing.x) * 0.5).floor();
        let can_vary = m.generated() && !m.prompt.is_empty();
        let vary = ui
            .add_enabled_ui(can_vary, |ui| icon_text_button(ui, "ArrowsRepeat", "Vary", Some(half)))
            .inner
            .on_hover_text("New takes on the same prompt and model (new seeds)")
            .on_disabled_hover_text("Only for generated pictures");
        if vary.clicked() {
            let args = GenerateArgs {
                prompt: m.prompt.clone(),
                model: Some(format!("{}:{}", m.backend, m.model)),
                count: images.settings.count,
                aspect: if m.aspect.is_empty() { images.settings.aspect.clone() } else { m.aspect.clone() },
                seed: None,
                refs: m.references.iter().filter(|r| images.lib.get(r).is_some()).cloned().collect(),
                ref_files: vec![],
                parent: Some(m.id.clone()),
            };
            match images.generate(args) {
                Ok(_) => v.say("Painting variations…", false, now),
                Err(e) => v.say(e, true, now),
            }
        }
        if icon_text_button(ui, "Pencil", "Edit with prompt", Some(half))
            .on_hover_text("Describe a change; the picture goes to the model as the base")
            .clicked()
        {
            v.parent = Some(m.id.clone());
            v.refs = vec![m.id.clone()];
            v.prompt.clear();
            v.focus_prompt = true;
            if images.current_model().max_refs == 0 {
                v.say("Pick a model that takes references (FLUX.2, Kontext or ChatGPT images)", true, now);
            }
        }
    });
    if let Some((msg, err, _)) = &v.note {
        ui.add_space(2.0);
        if *err {
            warn_line(ui, msg);
        } else {
            hint_small(ui, msg);
        }
    }
    // notes
    ui.add_space(6.0);
    section(ui, "Notes");
    let r = ui.add(
        egui::TextEdit::multiline(&mut v.notes)
            .hint_text("What it's for, what to change, where it's used…")
            .desired_rows(4)
            .desired_width(w),
    );
    if r.changed() {
        v.notes_dirty = Some(now);
    }
    if r.lost_focus() {
        save_notes(images, v);
    }
    // tags
    section(ui, "Tags");
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for t in &m.tags {
            if chip(ui, t, None, false).on_hover_text("Click to remove").clicked() {
                remove = Some(t.clone());
            }
        }
        let r = ui.add(
            egui::TextEdit::singleline(&mut v.tag_input)
                .hint_text("add a tag")
                .desired_width(110.0),
        );
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !v.tag_input.trim().is_empty() {
            let new: Vec<String> = v.tag_input.split(',').map(|s| s.to_string()).collect();
            let _ = images.lib.update(id, |e| e.tags.extend(new));
            v.tag_input.clear();
            r.request_focus();
        }
    });
    if let Some(t) = remove {
        let _ = images.lib.update(id, |e| e.tags.retain(|x| *x != t));
    }
    // where it came from
    ui.add_space(4.0);
    section_fold(ui, "images_details", "Details", "", |ui| {
        if !m.prompt.is_empty() {
            ui.add(egui::Label::new(RichText::new(&m.prompt).size(12.5).color(TEXT)).wrap().selectable(true));
            ui.horizontal(|ui| {
                if small_button(ui, "Use this prompt").on_hover_text("Put it in the Generate box").clicked() {
                    v.prompt = m.prompt.clone();
                }
                if small_button(ui, "Copy").clicked() {
                    ui.ctx().copy_text(m.prompt.clone());
                }
            });
            ui.add_space(4.0);
        }
        if m.generated() {
            stat_row(ui, "Model", &model_name(&m.backend, &m.model));
            if let Some(b) = BackendKind::from_id(&m.backend) {
                stat_row(ui, "Backend", b.name());
            }
            stat_row(ui, "Seed", &m.seed.map_or("—".into(), |s| s.to_string()));
        }
        if m.width > 0 {
            stat_row(ui, "Size", &format!("{} × {}", m.width, m.height));
        }
        stat_row(ui, "Source", &m.provenance);
        if !m.original.is_empty() {
            stat_row(ui, "File", &m.original);
        }
        if let Some(p) = &m.parent
            && let Some(pm) = images.lib.get(p)
            && stat_row(ui, "From", &pm.name).on_hover_text("Open it").clicked()
        {
            v.select(Some(p.clone()));
        }
        if !m.references.is_empty() {
            stat_row(ui, "References", &m.references.len().to_string());
        }
    });
}
