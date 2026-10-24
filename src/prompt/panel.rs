//! The floating prompt panel next to the selection (prompt box, suggestion chips, backend and
//! model, the live step feed, Keep / Undo / Refine) and the top bar's sparkle button.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Id, Order, Pos2, RichText, Stroke, vec2,
};
use egui_phosphor::regular as icon;

use super::agent::Backend;
use super::selection::{CameraQuery, Selection, area, area_text};
use super::{DUNGEON_SUGGESTIONS, PromptSession, Request, RunState, SUGGESTIONS};
use crate::camera::Pointer;
use crate::terrain::Terrain;
use crate::ui::theme::*;
use crate::ui::widgets::*;

const W: f32 = 380.0;
const GREEN: Color32 = Color32::from_rgb(74, 210, 120);
const AMBER: Color32 = Color32::from_rgb(232, 180, 90);

/// What the top bar needs (part of the UI's system state).
#[derive(SystemParam)]
pub struct PromptUi<'w> {
    pub sel: ResMut<'w, Selection>,
    pub session: ResMut<'w, PromptSession>,
}

const TIP: &str = "prompt_area";

/// The "Prompt an area" action: (active, drawing). Active while drawing or while the agent runs.
pub fn button_state(p: &PromptUi) -> (bool, bool) {
    (p.sel.drawing || p.session.running(), p.sel.drawing)
}

/// A click on "Prompt an area": start a selection, or show the panel of the current one.
pub fn button_clicked(p: &mut PromptUi) {
    crate::ui::tips::mark(TIP, true);
    if p.sel.drawing {
        p.sel.cancel_drawing();
    } else if p.sel.polygon().is_some() && !p.session.panel_open {
        p.session.panel_open = true;
    } else {
        p.sel.start_drawing();
    }
}

/// Until the button has been used once, a callout under it (`at`) says what it does.
pub fn first_use_callout(ctx: &egui::Context, p: &mut PromptUi, at: egui::Rect, active: bool) {
    if crate::ui::tips::seen(TIP) || active {
        return;
    }
    let screen = ctx.content_rect();
    let pos = egui::pos2(at.right().min(screen.max.x - 12.0), at.bottom() + 8.0);
    egui::Area::new(Id::new("prompt_callout")).order(Order::Foreground).pivot(egui::Align2::RIGHT_TOP).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::new()
            .fill(GLASS_SOLID)
            .stroke(Stroke::new(1.0, LINE_STRONG))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_max_width(260.0);
                ui.label(RichText::new("Change any area by describing it").font(FontId::new(13.5, semibold())).color(TEXT));
                ui.label(RichText::new("Press this (or G), draw around a spot on the ground, then type what you want there.").size(12.5).color(TEXT_DIM));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if crate::ui::widgets::primary_button_fit(ui, "Try it").clicked() {
                        crate::ui::tips::mark(TIP, true);
                        p.sel.start_drawing();
                    }
                    if crate::ui::widgets::button(ui, "Dismiss").clicked() {
                        crate::ui::tips::mark(TIP, true);
                    }
                });
            });
    });
}

fn clock(s: f32) -> String {
    format!("{}:{:02}", (s / 60.0) as u32, (s % 60.0) as u32)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn panel_ui(
    // native file dialogs (Add a reference image) must open on the main thread on macOS
    _main: bevy::ecs::system::NonSendMarker,
    mut contexts: EguiContexts,
    mut sel: ResMut<Selection>,
    mut s: ResMut<PromptSession>,
    terrain: Res<Terrain>,
    pointer: Res<Pointer>,
    camera: CameraQuery,
    time: Res<Time<Real>>,
    walk: Res<crate::walk::WalkState>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    if walk.active {
        return Ok(());
    }
    let now = time.elapsed_secs();
    let cam = camera.single().ok();
    if let Some((c, ct)) = cam {
        super::selection::draw_outline(&ctx, &sel, &terrain, &pointer, c, ct, now);
    }
    if sel.drawing {
        drawing_bar(&ctx, &mut sel, &terrain);
    }
    // `WB_PROMPT_PANEL=1`: open the panel for any selection (screenshot tests)
    if sel.just_selected
        || (sel.polygon().is_some() && !s.panel_open && std::env::var("WB_PROMPT_PANEL").is_ok())
    {
        sel.just_selected = false;
        s.panel_open = true;
        if !s.running() {
            s.state = RunState::Idle;
            s.error = None;
        }
    }
    let Some(poly) = sel.polygon().map(|p| p.to_vec()) else {
        if !s.running() {
            s.panel_open = false;
        }
        return Ok(());
    };
    if !s.panel_open && !s.running() {
        return Ok(());
    }
    // beside the selection on screen
    let screen = ctx.content_rect();
    let mut pos = Pos2::new(
        screen.max.x - crate::ui::RIGHT_W - W - 24.0,
        crate::ui::TOP_H + 16.0,
    );
    if let Some((c, ct)) = cam {
        let pts: Vec<Pos2> = poly
            .iter()
            .step_by(poly.len().div_ceil(32).max(1))
            .filter_map(|p| {
                c.world_to_viewport(
                    ct,
                    Vec3::new(p.x, terrain.height_at(p.x, p.y).max(terrain.sea_level), p.y),
                )
                .ok()
            })
            .map(|v| Pos2::new(v.x, v.y))
            .collect();
        if !pts.is_empty() {
            let (min_y, max_x) = pts
                .iter()
                .fold((f32::MAX, f32::MIN), |(y, x), p| (y.min(p.y), x.max(p.x)));
            pos = Pos2::new(max_x + 28.0, min_y);
        }
    }
    let lo_x = crate::ui::RAIL_W + crate::ui::LEFT_W + 16.0;
    // (clear of the inspector on the right: its width plus its margins)
    let hi_x = (screen.max.x - crate::ui::RIGHT_W - W - 48.0).max(lo_x);
    pos.x = pos.x.clamp(lo_x, hi_x);
    // (in a dungeon: below its map bar at the top of the view)
    let top = ctx
        .memory(|m| m.area_rect(Id::new("dungeon_map_bar")))
        .filter(|_| crate::dungeons::in_dungeon())
        .map_or(crate::ui::TOP_H + 12.0, |r| r.max.y + 10.0);
    pos.y = pos.y.clamp(top, (screen.max.y - 520.0).max(top));
    let size = area_text(area(&poly));
    // Esc closes (a focused text box takes the first Esc to let go of the keyboard)
    if !s.running()
        && !sel.drawing
        && !ctx.egui_wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        close(&mut sel, &mut s);
        return Ok(());
    }
    egui::Area::new(Id::new("prompt_panel"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(&ctx, |ui| {
            panel_frame().show(ui, |ui| {
                ui.set_width(W);
                content(ui, &mut sel, &mut s, &size);
            });
        });
    if s.running() {
        ctx.request_repaint();
    }
    Ok(())
}

/// What to do while drawing the selection, at the top of the viewport.
fn drawing_bar(ctx: &egui::Context, sel: &mut Selection, terrain: &Terrain) {
    let n = sel.draft_len();
    let head = if n == 0 {
        "Draw around the area you want to change".to_string()
    } else {
        format!("{n} point{} drawn", if n == 1 { "" } else { "s" })
    };
    let sub = if n == 0 {
        "Hold the mouse and drag a loop around it, or click corners one by one."
    } else {
        "Click the first corner, double-click or press Enter to finish. Backspace undoes a corner."
    };
    let mut cancel = false;
    let mut whole = false;
    egui::Area::new(Id::new("prompt_draw_bar")).order(Order::Foreground).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 64.0)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(GLASS_SOLID)
            .stroke(Stroke::new(1.0, LINE_STRONG))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SPARKLE).size(22.0).color(ACCENT));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(head).font(FontId::new(15.0, semibold())).color(Color32::WHITE));
                        ui.label(RichText::new(sub).small().color(TEXT_DIM));
                    });
                    ui.add_space(10.0);
                    if sel.dungeon_floor.is_none() && ui.button(RichText::new(format!("{} Whole world", icon::GLOBE_HEMISPHERE_WEST)).color(TEXT)).on_hover_text("Edit the whole world instead of an area (e.g. recreate a map picture)").clicked() {
                        whole = true;
                    }
                    if ui.button(RichText::new(format!("{} Cancel  (Esc)", icon::X)).color(TEXT)).clicked() {
                        cancel = true;
                    }
                });
            });
    });
    if cancel {
        sel.cancel_drawing();
    }
    if whole {
        sel.cancel_drawing();
        select_whole_world(sel, terrain);
    }
}

/// Selects the whole world (a hair inside its edge).
pub fn select_whole_world(sel: &mut Selection, terrain: &Terrain) {
    let h = terrain.half() * 0.995;
    let poly = [
        Vec2::new(-h, -h),
        Vec2::new(h, -h),
        Vec2::new(h, h),
        Vec2::new(-h, h),
    ];
    if sel.set(terrain, &poly).is_ok() {
        sel.just_selected = true;
    }
}

/// Close the panel and drop the selection (a lingering selection would keep masking edits).
fn close(sel: &mut Selection, s: &mut PromptSession) {
    sel.clear();
    s.images.clear();
    s.panel_open = false;
    s.refining = false;
    if s.state != RunState::Running {
        s.whole_dungeon = false;
        s.state = RunState::Idle;
        s.error = None;
        s.feed.clear();
    }
}

fn content(ui: &mut egui::Ui, sel: &mut Selection, s: &mut PromptSession, size: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::SPARKLE).size(18.0).color(ACCENT));
        if s.whole_dungeon {
            ui.label(RichText::new("Shaping the dungeon").font(FontId::new(16.5, semibold())).color(Color32::WHITE));
        } else {
            ui.label(RichText::new("Edit this area").font(FontId::new(16.5, semibold())).color(Color32::WHITE));
            ui.label(RichText::new(size).small().color(TEXT_DIM));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !s.running() && icon_button(ui, icon::X, "Close (Esc): clears the selection; anything the agent built stays (⌘Z undoes it)").clicked() {
                close(sel, s);
            }
        });
    });
    ui.add_space(4.0);
    match s.state {
        RunState::Running => running(ui, s),
        RunState::Done => done(ui, s),
        RunState::Idle | RunState::Failed => compose(ui, sel, s),
    }
}

/// The conversation so far: what you asked and what the agent did, oldest first (the latest
/// answer is shown below it).
fn thread(ui: &mut egui::Ui, s: &PromptSession) {
    let n = s.turns.len();
    if n < 2 {
        if let Some(t) = s.turns.last() {
            ui.label(
                RichText::new(format!("{}  {}", icon::USER, t.prompt))
                    .small()
                    .color(TEXT_DIM),
            );
            ui.add_space(2.0);
        }
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("prompt_thread")
        .max_height(180.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for (i, t) in s.turns.iter().enumerate() {
                ui.label(
                    RichText::new(format!("{}  {}", icon::USER, t.prompt))
                        .small()
                        .color(TEXT),
                );
                if i + 1 < n {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}  {}", icon::SPARKLE, t.summary))
                                .small()
                                .color(TEXT_DIM),
                        )
                        .wrap(),
                    );
                    ui.add_space(4.0);
                }
            }
        });
    ui.add_space(2.0);
}

/// Reference image thumbnails (decoded once per file).
pub fn image_thumb(ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureId> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<std::path::PathBuf, Option<egui::TextureHandle>>>> =
        Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new)
        .entry(path.to_path_buf())
        .or_insert_with(|| {
            let img = image::open(path).ok()?.thumbnail(160, 160).to_rgba8();
            let c = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            Some(ctx.load_texture(
                format!("ref_{}", path.display()),
                c,
                egui::TextureOptions::LINEAR,
            ))
        })
        .as_ref()
        .map(|t| t.id())
}

/// (PNG and JPEG: the image decoder is built without WebP)
pub const IMAGE_EXTS: [&str; 3] = ["png", "jpg", "jpeg"];

/// The attached reference images and the button to add one.
fn images_row(ui: &mut egui::Ui, s: &mut PromptSession) {
    let ctx = ui.ctx().clone();
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (i, p) in s.images.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(64.0, 64.0), egui::Sense::click());
            match image_thumb(&ctx, p) {
                Some(t) => {
                    egui::Image::new((t, r.size())).corner_radius(CornerRadius::same(6)).paint_at(ui, r);
                }
                None => {
                    ui.painter().rect_filled(r, CornerRadius::same(6), CARD_BG);
                }
            }
            let x = egui::Rect::from_center_size(r.right_top() + vec2(-9.0, 9.0), vec2(16.0, 16.0));
            ui.painter().circle_filled(x.center(), 8.0, Color32::from_black_alpha(190));
            ui.painter().text(x.center(), egui::Align2::CENTER_CENTER, icon::X, FontId::proportional(10.0), Color32::WHITE);
            if resp.on_hover_text(format!("{}\nClick to remove", p.display())).clicked() {
                remove = Some(i);
            }
        }
        let add = ui.add(egui::Button::new(RichText::new(format!("{}  {}", icon::IMAGE, if s.images.is_empty() { "Add a reference image" } else { "Add" })).color(TEXT)).corner_radius(CornerRadius::same(8)));
        if add.on_hover_text("A picture of the place you want: a map (followed as a layout) or a photo / painting (interpreted). You can also drop an image on the window.").clicked() {
            crate::dialogs::pick_files("prompt_reference", rfd::AsyncFileDialog::new().add_filter("Images", &IMAGE_EXTS).set_title("Reference image"));
        }
    });
    // (the dialog answers on a later frame)
    if let Some(files) = crate::dialogs::take_files("prompt_reference") {
        for f in files {
            if !s.images.contains(&f) && s.images.len() < 4 {
                s.images.push(f);
            }
        }
    }
    if let Some(i) = remove {
        s.images.remove(i);
    }
}

fn compose(ui: &mut egui::Ui, sel: &mut Selection, s: &mut PromptSession) {
    if let Some(e) = &s.error {
        ui.label(RichText::new(format!("{}  {e}", icon::WARNING_CIRCLE)).color(AMBER));
        ui.add_space(4.0);
    }
    let hint = if sel.dungeon_floor.is_some() {
        "Describe what you want here… e.g. Turn this into a flooded crypt"
    } else {
        "Describe what you want here… e.g. Turn this into an area with volcanoes"
    };
    let edit = egui::TextEdit::multiline(&mut s.input)
        .hint_text(hint)
        .desired_rows(3)
        .desired_width(f32::INFINITY);
    let r = ui.add(edit);
    let send_key =
        r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    ui.add_space(4.0);
    images_row(ui, s);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
        for (chip, text) in if sel.dungeon_floor.is_some() {
            DUNGEON_SUGGESTIONS
        } else {
            SUGGESTIONS
        } {
            let b = egui::Button::new(RichText::new(*chip).size(12.0).color(TEXT))
                .fill(CARD_BG)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(12));
            if ui.add(b).on_hover_text(*text).clicked() {
                s.input = text.to_string();
            }
        }
    });
    separator(ui);
    backend_row(ui, s);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let mut on = sel.mask_on;
        if toggle(ui, &mut on, RichText::new("Mask edits to selection").color(TEXT)).on_hover_text("Every edit (brushes, tools, agents) only changes the inside of the selection, fading out over a soft edge. Always on while an agent edits the area.").changed() {
            sel.mask_on = on;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = s.ready(s.backend);
            let can = ready && !s.input.trim().is_empty();
            let send = ui.add_enabled(can, egui::Button::new(RichText::new(format!("{}  Send", icon::PAPER_PLANE_RIGHT)).color(ON_ACCENT)).fill(ACCENT).corner_radius(CornerRadius::same(8)).min_size(vec2(84.0, 30.0)));
            if (send.on_hover_text("Send (⌘↩)").clicked() || (send_key && can)) && can {
                let prompt = s.input.clone();
                s.requests.push(Request::Send { prompt, refine: false });
            }
            if icon_button(ui, icon::SELECTION_SLASH, "Clear the selection").clicked() {
                sel.clear();
                s.panel_open = false;
            }
        });
    });
}

/// Shared agent configuration for the area-prompt panel and focused entry points such as Auto
/// zones. It edits the same saved backend, model, effort and API-key state in both places.
pub(crate) fn connection_controls(ui: &mut egui::Ui, s: &mut PromptSession) {
    backend_row(ui, s);
}

fn backend_row(ui: &mut egui::Ui, s: &mut PromptSession) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Agent").color(TEXT_DIM));
        let cur = s.backend;
        let label = |b: Backend, s: &PromptSession| {
            let dot = match s.status(b) {
                Some(st) if st.ready => "●",
                Some(_) => "○",
                None => "…",
            };
            format!("{dot} {}", b.name())
        };
        egui::ComboBox::from_id_salt("prompt_backend")
            .selected_text(label(cur, s))
            .width(150.0)
            .dropdown_ui(ui, |ui| {
                for b in Backend::ALL {
                    let text = label(b, s);
                    if menu_check(ui, b == cur, text).clicked() && b != cur {
                        s.set_backend(b);
                    }
                }
            });
        if icon_button(
            ui,
            icon::ARROW_CLOCKWISE,
            "Check again which agents are installed and logged in (and their models)",
        )
        .clicked()
        {
            s.detect();
        }
    });
    if s.backend == Backend::Api {
        api_key_row(ui, s, 200.0);
        api_model_row(ui, s);
    } else {
        model_row(ui, s);
    }
    let signin = s.backend == Backend::ClaudeCode
        && s.status(Backend::ClaudeCode).is_some_and(|st| !st.ready && st.path.is_some());
    if signin || s.login.is_some() {
        claude_signin_row(ui, s);
    } else if let Some(st) = s.status(s.backend) {
        let col = if st.ready { GREEN } else { AMBER };
        ui.add(egui::Label::new(RichText::new(&st.note).small().color(col)).wrap());
    }
}

/// The model and how hard it thinks (Codex, Claude Code: their own lists).
fn model_row(ui: &mut egui::Ui, s: &mut PromptSession) {
    let opts = s.model_options();
    let model = s.model();
    let cur = opts.iter().find(|o| o.slug == model).cloned();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Model").color(TEXT_DIM));
        let name = cur.as_ref().map(|o| o.name.clone()).unwrap_or(model.clone());
        egui::ComboBox::from_id_salt("prompt_model").selected_text(name).width(150.0).height(420.0).dropdown_ui(ui, |ui| {
            ui.set_min_width(300.0);
            for o in &opts {
                let r = menu_check(ui, o.slug == model, RichText::new(&o.name).color(TEXT));
                if !o.description.is_empty() {
                    ui.label(RichText::new(&o.description).small().color(TEXT_DIM));
                }
                if r.clicked() {
                    s.set_model(&o.slug);
                }
            }
        });
        if let (Some(o), Some(effort)) = (&cur, s.effort()) {
            egui::ComboBox::from_id_salt("prompt_effort")
                .selected_text(agent_effort_label(&effort))
                .width(110.0)
                .dropdown_ui(ui, |ui| {
                    for e in &o.efforts {
                        if menu_check(ui, *e == effort, agent_effort_label(e)).clicked() {
                            s.set_effort(e);
                        }
                    }
                })
                .response
                .on_hover_text("Reasoning effort: higher thinks longer and builds more carefully, but takes longer");
        }
    });
}

/// The API backend's provider (OpenRouter, OpenAI, … or any OpenAI-compatible URL) and its key.
/// Shared by the prompt panel and the setup wizard.
pub(crate) fn api_key_row(ui: &mut egui::Ui, s: &mut PromptSession, key_width: f32) {
    let named = super::agent::provider(&s.api_base);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Provider").color(TEXT_DIM));
        egui::ComboBox::from_id_salt("api_provider")
            .selected_text(named.map_or("Custom URL", |p| p.name))
            .width(150.0)
            .dropdown_ui(ui, |ui| {
                for p in &super::agent::PROVIDERS {
                    if menu_check(ui, named.is_some_and(|n| n.base == p.base), p.name).clicked() {
                        s.set_api_base(p.base);
                    }
                }
                if menu_check(ui, named.is_none(), "Custom URL…").clicked() && named.is_some() {
                    s.api_base = "https://".into();
                }
            })
            .response
            .on_hover_text("Any service that speaks the OpenAI Chat Completions API works: OpenRouter reaches Claude, GPT, Gemini and many more with one key");
    });
    if named.is_none() {
        ui.horizontal(|ui| {
            ui.label(RichText::new("URL").color(TEXT_DIM));
            let r = ui.add(
                egui::TextEdit::singleline(&mut s.api_base)
                    .hint_text("https://…/v1")
                    .desired_width(key_width + 60.0),
            );
            if r.lost_focus() {
                let b = s.api_base.clone();
                s.set_api_base(&b);
            }
        });
        if !super::agent::valid_base(&s.api_base) {
            ui.label(RichText::new("The base URL, ending in /v1 (the part before /chat/completions)").small().color(AMBER));
        }
    }
    let needs_key = named.is_none_or(|p| !p.keys_at.is_empty());
    if needs_key {
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::KEY).color(TEXT_DIM));
            let r = ui.add(
                egui::TextEdit::singleline(&mut s.api_key)
                    .password(true)
                    .hint_text(format!("{} API key", named.map_or("sk-…", |p| p.key_hint)))
                    .desired_width(key_width),
            );
            let mut remember = s.remember_key;
            let c = toggle(ui, &mut remember, RichText::new("Remember").small().color(TEXT))
                .on_hover_text("Keep the key on this Mac (a file only you can read, outside the project)");
            if c.changed() || (r.lost_focus() && s.remember_key) {
                s.remember_key = remember;
                s.store_key();
            }
            // (a new key: read the provider's models with it)
            if r.lost_focus() {
                s.detect();
            }
        });
        if let Some(p) = named
            && s.api_key.trim().is_empty()
        {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Get a key at").small().color(TEXT_DIM));
                ui.hyperlink_to(
                    RichText::new(p.keys_at).small(),
                    format!("https://{}", p.keys_at),
                );
            });
        }
    }
}

/// The API backend's model: any id the provider has, with its catalog as suggestions.
pub(crate) fn api_model_row(ui: &mut egui::Ui, s: &mut PromptSession) {
    let opts = s.model_options();
    let mut model = s.models.get(&Backend::Api).cloned().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Model").color(TEXT_DIM));
        let hint = opts.first().map_or("model id, e.g. anthropic/…".to_string(), |o| o.slug.clone());
        let r = ui.add(egui::TextEdit::singleline(&mut model).hint_text(hint).desired_width(170.0));
        if r.changed() {
            s.set_model(&model);
        }
        if !opts.is_empty() {
            let typed = model.trim().to_lowercase();
            egui::ComboBox::from_id_salt("api_model_pick")
                .selected_text(format!("{}", opts.len()))
                .width(56.0)
                .height(420.0)
                .dropdown_ui(ui, |ui| {
                    ui.set_min_width(320.0);
                    let cur = s.model();
                    for o in opts
                        .iter()
                        .filter(|o| typed.is_empty() || o.slug.to_lowercase().contains(&typed) || o.name.to_lowercase().contains(&typed) || o.slug == cur)
                        .take(40)
                    {
                        let label = if o.name == o.slug { o.slug.clone() } else { format!("{}  ·  {}", o.name, o.slug) };
                        if menu_check(ui, o.slug == cur, RichText::new(label).color(TEXT)).clicked() {
                            s.set_model(&o.slug);
                        }
                    }
                })
                .response
                .on_hover_text("The provider's models that can use tools (type to narrow the list)");
        }
    });
}

/// Sign in with Claude: one click runs Claude Code's own sign-in (the browser opens); a code to
/// paste in case the browser can't hand it back. Shared by the prompt panel and the wizard.
pub(crate) fn claude_signin_row(ui: &mut egui::Ui, s: &mut PromptSession) {
    let Some(url) = s.login.as_ref().map(|l| l.url.clone()) else {
        let b = egui::Button::new(RichText::new(format!("{}  Sign in with Claude", icon::SIGN_IN)).color(ON_ACCENT))
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(170.0, 30.0));
        if ui.add(b).on_hover_text("Opens claude.ai in your browser to sign in with your Claude account").clicked() {
            s.start_claude_login();
        }
        if let Some(e) = &s.login_error {
            ui.add(egui::Label::new(RichText::new(format!("Sign-in didn't finish: {e}")).small().color(AMBER)).wrap());
        }
        return;
    };
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(14.0).color(ACCENT));
        ui.label(RichText::new("Finish signing in in your browser…").color(TEXT));
    });
    ui.horizontal(|ui| {
        if let Some(u) = &url
            && ui.link(RichText::new(format!("{}  Open the sign-in page again", icon::ARROW_SQUARE_OUT)).small()).clicked()
        {
            ui.ctx().open_url(egui::OpenUrl::new_tab(u));
        }
        // (dropping it stops the sign-in)
        if ui.button(RichText::new("Cancel").small()).clicked() {
            s.login = None;
        }
    });
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut s.login_code)
                .hint_text("Code from the page, if it shows one")
                .desired_width(210.0),
        );
        let go = !s.login_code.trim().is_empty()
            && (ui.button("Use code").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))));
        if go && let Some(l) = s.login.as_mut() {
            l.submit_code(&s.login_code);
            s.login_code.clear();
        }
    });
}

fn agent_effort_label(e: &str) -> String {
    super::agent::effort_name(e)
}

fn running(ui: &mut egui::Ui, s: &mut PromptSession) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
        ui.label(
            RichText::new(format!(
                "Working… {}  ·  {} tool call{}",
                clock(s.elapsed),
                s.calls,
                if s.calls == 1 { "" } else { "s" }
            ))
            .color(TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{}  Stop", icon::STOP)).color(ON_ACCENT),
                    )
                    .fill(DANGER)
                    .corner_radius(CornerRadius::same(8)),
                )
                .on_hover_text("Stop the agent (what it already did stays, as one undo step)")
                .clicked()
            {
                s.requests.push(Request::Stop);
            }
        });
    });
    ui.label(
        RichText::new(format!("{} · {}", s.backend.name(), s.model_label()))
            .small()
            .color(TEXT_DIM),
    );
    ui.add_space(4.0);
    feed(ui, s, 260.0);
}

fn feed(ui: &mut egui::Ui, s: &PromptSession, max_h: f32) {
    egui::ScrollArea::vertical()
        .max_height(max_h)
        .stick_to_bottom(true)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for f in &s.feed {
                ui.horizontal_top(|ui| {
                    let (ic, col) = match (&f.tool, f.ok) {
                        (None, _) => (icon::CHAT_CIRCLE_DOTS, TEXT_DIM),
                        (Some(_), None) => (icon::CIRCLE_NOTCH, ACCENT),
                        (Some(_), Some(true)) => (icon::CHECK_CIRCLE, GREEN),
                        (Some(_), Some(false)) => (icon::WARNING_CIRCLE, AMBER),
                    };
                    ui.label(RichText::new(ic).color(col));
                    let text = if f.tool.is_some() {
                        RichText::new(&f.text).color(TEXT)
                    } else {
                        RichText::new(&f.text).italics().color(TEXT_DIM)
                    };
                    let r = ui.add(egui::Label::new(text).wrap());
                    let when = clock(f.at);
                    r.on_hover_text(match &f.detail {
                        Some(d) => format!("{when}  {d}"),
                        None => when,
                    });
                });
            }
        });
}

fn done(ui: &mut egui::Ui, s: &mut PromptSession) {
    if let Some(e) = &s.error {
        ui.label(RichText::new(format!("{}  {e}", icon::WARNING_CIRCLE)).color(AMBER));
    }
    thread(ui, s);
    let summary = if s.summary.is_empty() {
        "Done.".to_string()
    } else {
        s.summary.clone()
    };
    ui.add(egui::Label::new(RichText::new(summary).color(TEXT)).wrap());
    ui.add_space(2.0);
    let failed = s.feed.iter().filter(|f| f.ok == Some(false)).count();
    ui.label(
        RichText::new(format!(
            "{} · {} tool calls{}",
            clock(s.elapsed),
            s.calls,
            if failed > 0 {
                format!(" ({failed} failed)")
            } else {
                String::new()
            }
        ))
        .small()
        .color(TEXT_DIM),
    );
    egui::CollapsingHeader::new(RichText::new("Steps").small().color(TEXT_DIM))
        .id_salt("prompt_steps")
        .show(ui, |ui| feed(ui, s, 200.0));
    separator(ui);
    if s.refining {
        ui.add(
            egui::TextEdit::multiline(&mut s.refine_input)
                .hint_text("What should change? e.g. Make the volcano taller and add more ash")
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                s.refining = false;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = !s.refine_input.trim().is_empty();
                if ui
                    .add_enabled(
                        can,
                        egui::Button::new(
                            RichText::new(format!("{}  Refine", icon::PAPER_PLANE_RIGHT))
                                .color(ON_ACCENT),
                        )
                        .fill(ACCENT)
                        .corner_radius(CornerRadius::same(8)),
                    )
                    .clicked()
                {
                    let prompt = std::mem::take(&mut s.refine_input);
                    s.requests.push(Request::Send {
                        prompt,
                        refine: true,
                    });
                }
            });
        });
        return;
    }
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(
                    RichText::new(format!("{}  Keep & close", icon::CHECK)).color(ON_ACCENT),
                )
                .fill(ACCENT)
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(110.0, 30.0)),
            )
            .on_hover_text("Keep the edit and close (⌘Z still undoes it)")
            .clicked()
        {
            s.requests.push(Request::Keep);
        }
        let can_undo = s.step.is_some();
        if ui
            .add_enabled(
                can_undo,
                egui::Button::new(format!("{}  Undo", icon::ARROW_U_UP_LEFT))
                    .min_size(vec2(80.0, 30.0)),
            )
            .on_hover_text("Undo the whole edit (one step)")
            .clicked()
        {
            s.requests.push(Request::Undo);
        }
        if ui
            .add(
                egui::Button::new(format!("{}  Refine", icon::MAGIC_WAND))
                    .min_size(vec2(84.0, 30.0)),
            )
            .on_hover_text("Ask for changes; the agent keeps the conversation")
            .clicked()
        {
            s.refining = true;
        }
    });
}
