//! The Halo shell (HANDOFF › Layout): floating glass islands over the full-bleed world.
//! Top-left the world capsule (name, size, saved state, undo / redo, File ▾), top-centre the
//! environment pill (play the day, time + day speed, time slider, motion preview, Look ▾),
//! top-right the actions (⌘K search, Prompt an area, MCP, Walk ▾); the rail on the left
//! (groups 1–9, mute, help); the status line with scale and compass bottom-left; toasts.

use bevy::prelude::*;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Margin, Order, Rect, RectAlign, Response,
    Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::theme::*;
use super::widgets::*;
use super::{UiAction, UiOut, UiState};
use crate::audio::SoundEvent;
use crate::editor::TabGroup;
use crate::tools::EditorAction;

/// Screen margin round the islands.
pub const M: f32 = 12.0;
/// Island height (capsule, pill, actions).
pub const H: f32 = 38.0;
/// Where the tool panel and inspector start (under the top islands).
pub const TOP: f32 = 60.0;
/// The tool panel's left edge (right of the rail) and width; the inspector's width.
pub const PANEL_X: f32 = 62.0;
pub const PANEL_W: f32 = 316.0;
pub const INSP_W: f32 = 290.0;

/// What's open: the tool panel and the inspector (Tab or their buttons collapse them to pills).
#[derive(Clone, Copy)]
pub struct Shell {
    pub panel_open: bool,
    pub insp_open: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            panel_open: true,
            insp_open: true,
        }
    }
}

fn shell_id() -> Id {
    Id::new("halo_shell")
}

pub fn get(ctx: &egui::Context) -> Shell {
    ctx.data(|d| d.get_temp(shell_id())).unwrap_or_default()
}

pub fn set(ctx: &egui::Context, s: Shell) {
    ctx.data_mut(|d| d.insert_temp(shell_id(), s));
}

/// `Tab` (when no text field has the keys): hide both panels, or show them again.
pub fn tab_key(ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
        let mut sh = get(ctx);
        let open = !(sh.panel_open || sh.insp_open);
        sh.panel_open = open;
        sh.insp_open = open;
        set(ctx, sh);
    }
}

// ------------------------------------------------------------------ island pieces

fn island(
    ctx: &egui::Context,
    id: &str,
    anchor: Align2,
    offset: egui::Vec2,
    radius: u8,
    add: impl FnOnce(&mut Ui),
) -> Response {
    egui::Area::new(Id::new(id))
        .order(Order::Middle)
        .anchor(anchor, offset)
        .show(ctx, |ui| {
            let frame = glass_frame()
                .corner_radius(CornerRadius::same(radius))
                .inner_margin(Margin {
                    left: 5,
                    right: 5,
                    top: 5,
                    bottom: 5,
                });
            glass(ui, frame, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.set_height(28.0);
                    add(ui);
                });
            });
        })
        .response
}

/// A thin divider spanning the island's height.
fn sep(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, 28.0), Sense::hover());
    ui.painter().rect_filled(
        Rect::from_min_max(
            pos2(r.left(), r.top() - 5.0),
            pos2(r.right(), r.bottom() + 5.0),
        ),
        0.0,
        LINE,
    );
}

/// A 28 px icon button in an island, with its instant tip below.
fn ib(ui: &mut Ui, icon: &str, title: &str, key: &str, on: bool, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(
        vec2(28.0, 28.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        let hot = enabled && resp.hovered();
        if on {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), SOFT_HOVER);
        } else if hot {
            ui.painter().rect_filled(rect, CornerRadius::same(8), SOFT);
        }
        let col = if !enabled {
            TEXT_MUTE
        } else if hot || on {
            TEXT
        } else {
            TEXT_DIM
        };
        paint_icon(ui, rect.center(), icon, 15.0, col);
    }
    tip(resp, title, "", key, TipSide::Below).on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    })
}

/// A labelled island button (icon + text + optional chevron), soft when `fill`.
fn text_button(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    chevron: bool,
    fill: bool,
    open: bool,
) -> Response {
    let font = FontId::proportional(12.5);
    let g = ui.painter().layout_no_wrap(label.to_owned(), font, TEXT);
    let w = 9.0 + 14.0 + 5.0 + g.size().x + if chevron { 5.0 + 11.0 } else { 0.0 } + 9.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hot = resp.hovered() || open;
        let bg = match (fill, hot) {
            (true, true) => Some(SOFT_HOVER),
            (true, false) => Some(SOFT),
            (false, true) => Some(SOFT),
            _ => None,
        };
        if let Some(bg) = bg {
            p.rect_filled(rect, CornerRadius::same(8), bg);
        }
        let col = if fill || hot { TEXT } else { TEXT_DIM };
        let mut x = rect.left() + 9.0;
        paint_icon(ui, pos2(x + 7.0, rect.center().y), icon, 14.0, col);
        x += 19.0;
        let gw = g.size().x;
        ui.painter()
            .galley(pos2(x, rect.center().y - g.size().y * 0.5), g, col);
        x += gw + 5.0;
        if chevron {
            paint_icon(
                ui,
                pos2(x + 5.5, rect.center().y),
                "ChevronDownSmall",
                12.0,
                col.gamma_multiply(0.6),
            );
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The Halo glass menu frame.
pub fn menu_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(GLASS_SOLID)
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(R_MENU))
        .inner_margin(Margin::same(4))
        .shadow(egui::epaint::Shadow {
            offset: [0, 18],
            blur: 46,
            spread: 0,
            color: Color32::from_black_alpha(150),
        })
}

fn menu_hr(ui: &mut Ui) {
    ui.add_space(4.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter()
        .rect_filled(r.shrink2(vec2(6.0, 0.0)), 0.0, LINE);
    ui.add_space(4.0);
}

// ------------------------------------------------------------------ top islands

pub fn top(ctx: &egui::Context, s: &mut UiState, out: &mut UiOut, screen: Rect) {
    world_capsule(ctx, s, out);
    env_pill(ctx, s);
    actions(ctx, s, out);
    let _ = screen;
}

fn world_capsule(ctx: &egui::Context, s: &mut UiState, out: &mut UiOut) {
    island(ctx, "halo_world", Align2::LEFT_TOP, vec2(M, M), 13, |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
        let logo = super::logo_texture(ui.ctx());
        egui::Image::new((logo.id(), r.size()))
            .corner_radius(CornerRadius::same(8))
            .paint_at(ui, r);
        ui.label(
            egui::RichText::new(&s.meta.name)
                .font(FontId::new(13.5, semibold()))
                .color(TEXT),
        );
        let km = s.terrain.size() / 1000.0;
        ui.label(
            egui::RichText::new(format!("{km:.1} × {km:.1} km"))
                .font(FontId::new(11.5, mono()))
                .color(TEXT_MUTE),
        );
        saved_state(ui, s);
        sep(ui);
        let undo_tip = s
            .history
            .undo_label()
            .map(|l| format!("Undo {l}"))
            .unwrap_or("Nothing to undo".into());
        if ib(
            ui,
            "ArrowUndoUp",
            &undo_tip,
            "⌘Z",
            false,
            s.history.can_undo(),
        )
        .clicked()
        {
            out.editor_actions.write(EditorAction::Undo);
        }
        let redo_tip = s
            .history
            .redo_label()
            .map(|l| format!("Redo {l}"))
            .unwrap_or("Nothing to redo".into());
        if ib(
            ui,
            "ArrowRedoDown",
            &redo_tip,
            "⇧⌘Z",
            false,
            s.history.can_redo(),
        )
        .clicked()
        {
            out.editor_actions.write(EditorAction::Redo);
        }
        let open = egui::Popup::is_id_open(ui.ctx(), Id::new("halo_file_menu"));
        let file = text_button(ui, "FolderOpen", "File", true, false, open);
        egui::Popup::menu(&file).id(Id::new("halo_file_menu")).frame(menu_frame()).align(RectAlign::BOTTOM_START).gap(8.0).show(|ui| {
            ui.set_min_width(250.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            if menu_row(ui, "PageAdd", "New world…", "⌘N", false).clicked() {
                s.ed.open_new_world();
            }
            if menu_row(ui, "FolderOpen", "Open…", "⌘O", false).clicked() {
                out.actions.write(UiAction::Open);
            }
            if menu_row(ui, "FloppyDisk1", "Save", "⌘S", false).clicked() {
                out.actions.write(UiAction::Save);
            }
            if menu_row(ui, "FloppyDisk1", "Save as…", "⇧⌘S", false).clicked() {
                out.actions.write(UiAction::SaveAs);
            }
            menu_hr(ui);
            if menu_row(ui, "ArrowOutOfBox", "Export to Godot / Unity / Unreal…", "⌘E", false).clicked() {
                out.actions.write(UiAction::Export);
            }
            if menu_row(ui, "Server", "Export server data…", "", false).on_hover_text("gameplay.json for your game server: start, places, NPCs, spawns with levels and counts, creatures, resources and fishing spots").clicked() {
                super::panels::export_server_data(s);
            }
            menu_hr(ui);
            if menu_row(ui, "Agent", "Connect an agent (MCP)…", "", false).clicked() {
                s.mcp.show_panel = true;
            }
        });
    });
}

/// Saved dot (green saved, amber unsaved; a ring while autosave is off). Click: autosave on / off.
fn saved_state(ui: &mut Ui, s: &mut UiState) {
    let now = s.real_time.elapsed_secs();
    let dirty = s.meta.dirty;
    let label = if dirty { "Edited" } else { "Saved" };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), TEXT_DIM);
    let (rect, resp) = ui.allocate_exact_size(vec2(10.0 + g.size().x + 2.0, 28.0), Sense::click());
    let col = if dirty { WARN } else { OK };
    let c = pos2(rect.left() + 3.0, rect.center().y);
    if s.autosave.enabled {
        ui.painter().circle_filled(c, 3.0, col);
    } else {
        ui.painter().circle_stroke(c, 2.6, Stroke::new(1.2, col));
    }
    ui.painter().galley(
        pos2(rect.left() + 10.0, rect.center().y - g.size().y * 0.5),
        g,
        if resp.hovered() { TEXT } else { TEXT_DIM },
    );
    let state = if dirty {
        "Unsaved changes.".to_string()
    } else if let Some((t, true)) = s.autosave.last {
        let mins = ((now - t) / 60.0).floor() as i32;
        if mins < 1 {
            "Autosaved just now.".into()
        } else {
            format!("Autosaved {mins} min ago.")
        }
    } else {
        "All changes saved.".into()
    };
    let tip_text = if s.autosave.enabled {
        format!(
            "{state}\nAutosave is on: every 2 minutes with changes and when you close the app. Saved worlds save in place (only what changed is rewritten); untitled worlds keep one recovery copy in worlds/_recovery.world.\nClick to turn autosave off."
        )
    } else {
        format!("{state}\nAutosave is off. Click to turn it on.")
    };
    if resp
        .on_hover_text(tip_text)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        let on = !s.autosave.enabled;
        s.autosave.set_enabled(on);
    }
}

fn env_pill(ctx: &egui::Context, s: &mut UiState) {
    island(
        ctx,
        "halo_env",
        Align2::CENTER_TOP,
        vec2(0.0, M),
        13,
        |ui| {
            // play the day
            let h = s.lighting.hour;
            let night = !(5.0..20.5).contains(&h);
            let ic = if s.lighting.playing {
                "Pause"
            } else if night {
                "Moon"
            } else if !(7.5..17.5).contains(&h) {
                "Sunset"
            } else {
                "Sun"
            };
            let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
            let amber = Color32::from_rgb(255, 213, 138);
            ui.painter().rect_filled(
                r,
                CornerRadius::same(9),
                Color32::from_rgba_unmultiplied(
                    255,
                    213,
                    138,
                    if resp.hovered() { 46 } else { 31 },
                ),
            );
            paint_icon(ui, r.center(), ic, 15.0, amber);
            let mins = s.lighting.day_minutes.max(1.0);
            let play_tip = format!(
                "{} the 24-hour day · {mins:.0} min/day",
                if s.lighting.playing { "Pause" } else { "Play" }
            );
            if tip(resp, &play_tip, "", "", TipSide::Below)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                s.lighting.playing = !s.lighting.playing;
            }
            // time readout: click for the day speed
            let g = ui.painter().layout_no_wrap(
                format!("{:02}:{:02}", h as u32, (h.fract() * 60.0) as u32),
                FontId::new(14.0, mono_semibold()),
                TEXT,
            );
            let (r, resp) = ui.allocate_exact_size(vec2(54.0, 28.0), Sense::click());
            let open = egui::Popup::is_id_open(ui.ctx(), Id::new("halo_speed_menu"));
            if resp.hovered() || open {
                ui.painter().rect_filled(r, CornerRadius::same(8), SOFT);
            }
            ui.painter().galley(r.center() - g.size() * 0.5, g, TEXT);
            let resp = tip(
                resp,
                &format!("Day speed: 1 day = {mins:.0} min"),
                "",
                "",
                TipSide::Below,
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::Popup::menu(&resp)
                .id(Id::new("halo_speed_menu"))
                .frame(menu_frame())
                .align(RectAlign::BOTTOM_START)
                .gap(8.0)
                .show(|ui| {
                    ui.set_min_width(180.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for m in [1.0, 2.0, 5.0, 10.0, 24.0, 60.0] {
                        if menu_row(
                            ui,
                            "",
                            &format!("1 day = {m:.0} min"),
                            "",
                            s.lighting.day_minutes == m,
                        )
                        .clicked()
                        {
                            s.lighting.day_minutes = m;
                        }
                    }
                });
            // the day as a gradient slider
            day_slider(ui, &mut s.lighting.hour);
            // motion while editing (walking always animates)
            let on = s.motion.preview;
            let mtip = if on {
                "Motion preview on: wind, waves, smoke and cloth animate"
            } else {
                "The world holds still while editing. Click to preview motion."
            };
            if ib(ui, "Wind", mtip, "", on, true).clicked() {
                s.motion.preview = !on;
            }
            sep(ui);
            // Look ▾
            let cur = s.lighting.look.preset;
            let open = egui::Popup::is_id_open(ui.ctx(), Id::new("halo_look_menu"));
            let look = text_button(ui, "ColorPalette", cur.name(), true, true, open);
            let look = tip(
                look,
                "Art style",
                "L cycles · hold \\ to compare with Natural",
                "L",
                TipSide::Below,
            );
            egui::Popup::menu(&look)
                .id(Id::new("halo_look_menu"))
                .frame(menu_frame())
                .align(RectAlign::BOTTOM_END)
                .gap(8.0)
                .show(|ui| {
                    ui.set_min_width(300.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for p in crate::look::LookPreset::ALL {
                        if look_row(ui, p, cur == p).on_hover_text(p.blurb()).clicked() && cur != p
                        {
                            s.lighting.look.select(p);
                        }
                    }
                });
        },
    );
}

/// A Look menu row: the 4-colour palette strip, the name, "inspired by" at the right.
fn look_row(ui: &mut Ui, p: crate::look::LookPreset, on: bool) -> Response {
    let mut resp = ui.allocate_response(vec2(280.0, 30.0), Sense::click());
    if resp.clicked() {
        resp.mark_changed();
        ui.close();
    }
    let r = resp.rect;
    let pnt = ui.painter();
    if resp.hovered() {
        pnt.rect_filled(r, CornerRadius::same(8), SOFT_HOVER);
    } else if on {
        pnt.rect_filled(r, CornerRadius::same(8), SOFT);
    }
    let strip = Rect::from_min_size(pos2(r.left() + 9.0, r.center().y - 7.0), vec2(36.0, 14.0));
    let w = strip.width() / 4.0;
    for (i, c) in p.palette().iter().enumerate() {
        let cr = Rect::from_min_size(strip.min + vec2(i as f32 * w, 0.0), vec2(w, strip.height()));
        let round = match i {
            0 => CornerRadius {
                nw: 4,
                sw: 4,
                ne: 0,
                se: 0,
            },
            3 => CornerRadius {
                nw: 0,
                sw: 0,
                ne: 4,
                se: 4,
            },
            _ => CornerRadius::ZERO,
        };
        pnt.rect_filled(cr, round, Color32::from_rgb(c[0], c[1], c[2]));
    }
    let g = pnt.layout_no_wrap(p.name().to_owned(), FontId::proportional(13.0), TEXT);
    pnt.galley(
        pos2(strip.right() + 10.0, r.center().y - g.size().y * 0.5),
        g,
        TEXT,
    );
    let e = pnt.layout_no_wrap(
        p.inspired_by().to_owned(),
        FontId::proportional(12.0),
        TEXT_MUTE,
    );
    pnt.galley(
        pos2(
            r.right() - 9.0 - e.size().x,
            r.center().y - e.size().y * 0.5,
        ),
        e,
        TEXT_MUTE,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The time of day on a sky-coloured track (night blue → dawn → day → dusk → night).
fn day_slider(ui: &mut Ui, hour: &mut f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(120.0, 28.0), Sense::click_and_drag());
    let (x0, x1) = (rect.left() + 8.0, rect.right() - 8.0);
    if let Some(p) = resp.interact_pointer_pos() {
        *hour = ((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0) * 23.99;
    }
    let p = ui.painter();
    let cy = rect.center().y;
    let stops: [(f32, [u8; 3]); 8] = [
        (0.0, [28, 42, 82]),
        (0.20, [28, 42, 82]),
        (0.27, [242, 162, 92]),
        (0.40, [159, 208, 255]),
        (0.55, [214, 236, 255]),
        (0.74, [247, 178, 103]),
        (0.82, [196, 93, 93]),
        (0.90, [28, 42, 82]),
    ];
    let (t0, t1) = (cy - 3.0, cy + 3.0);
    let mut m = egui::Mesh::default();
    let mut add = |t: f32, c: [u8; 3]| {
        let x = egui::lerp(rect.left()..=rect.right(), t);
        let col = Color32::from_rgb(c[0], c[1], c[2]);
        m.colored_vertex(pos2(x, t0), col);
        m.colored_vertex(pos2(x, t1), col);
    };
    for (t, c) in stops {
        add(t, c);
    }
    add(1.0, [28, 42, 82]);
    let n = m.vertices.len() as u32 / 2;
    for i in 0..n - 1 {
        let (a, b) = (i * 2, i * 2 + 2);
        m.add_triangle(a, a + 1, b);
        m.add_triangle(b, a + 1, b + 1);
    }
    // round ends (the track ends are night blue)
    let night = Color32::from_rgb(28, 42, 82);
    p.circle_filled(pos2(rect.left(), cy), 3.0, night);
    p.circle_filled(pos2(rect.right(), cy), 3.0, night);
    p.add(Shape::mesh(m));
    let cx = egui::lerp(x0..=x1, *hour / 23.99);
    p.circle_filled(pos2(cx, cy), 11.0, Color32::from_black_alpha(77));
    p.circle_filled(
        pos2(cx, cy),
        if resp.hovered() || resp.dragged() {
            8.5
        } else {
            8.0
        },
        Color32::WHITE,
    );
    let _ = tip(resp, "Time of day", "", "", TipSide::Below);
}

fn actions(ctx: &egui::Context, s: &mut UiState, out: &mut UiOut) {
    egui::Area::new(Id::new("halo_actions")).order(Order::Middle).anchor(Align2::RIGHT_TOP, vec2(-M, M)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // ⌘K search
            let r = action_island(ui, "halo_find", false, |ui| {
                paint_slot_icon(ui, "MagnifyingGlass", TEXT_DIM);
                kbd(ui, "⌘K");
            });
            if tip(r, "Search tools, assets, actions", "", "", TipSide::Below).clicked() {
                super::palette::open(ui.ctx());
            }
            // Prompt an area
            let (active, drawing) = crate::prompt::button_state(&s.prompt);
            let r = action_island(ui, "halo_prompt", active, |ui| {
                let col = if active { ON_ACCENT } else { TEXT };
                paint_slot_icon(ui, "SparklesTwo", col);
                ui.label(egui::RichText::new(if drawing { "Drawing…" } else { "Prompt an area" }).font(FontId::new(13.0, semibold())).color(col));
                kbd(ui, "G");
            });
            let r = r.on_hover_text("Draw around part of the world and describe what you want there (\"turn this into a volcanic wasteland\"); an AI agent builds it while you watch.");
            crate::prompt::first_use_callout(ui.ctx(), &mut s.prompt, r.rect, active);
            if r.clicked() {
                crate::prompt::button_clicked(&mut s.prompt);
            }
            // MCP
            let now = s.real_time.elapsed_secs();
            let (dot, mcp_tip, listening) = crate::mcp::status_info(&s.mcp, now);
            let r = action_island(ui, "halo_mcp", false, |ui| {
                paint_slot_icon(ui, "Agent", if listening { TEXT } else { TEXT_DIM });
            });
            ui.painter().circle_filled(pos2(r.rect.right() - 9.0, r.rect.top() + 11.0), 5.0, dot.gamma_multiply(0.25));
            ui.painter().circle_filled(pos2(r.rect.right() - 9.0, r.rect.top() + 11.0), 3.0, dot);
            if r.on_hover_text(mcp_tip).clicked() {
                s.mcp.show_panel = !s.mcp.show_panel;
            }
            walk_split(ui, out);
        });
    });
}

/// A 38 px glass island used as one button (white when `on`).
fn action_island(ui: &mut Ui, id: &str, on: bool, add: impl FnOnce(&mut Ui)) -> Response {
    let frame = if on {
        egui::Frame::new()
            .fill(Color32::WHITE)
            .corner_radius(CornerRadius::same(13))
            .inner_margin(Margin::symmetric(10, 5))
    } else {
        glass_frame()
            .corner_radius(CornerRadius::same(13))
            .inner_margin(Margin::symmetric(10, 5))
    };
    let inner = if on {
        frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.set_height(28.0);
                add(ui);
            });
        })
    } else {
        glass(ui, frame, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.set_height(28.0);
                add(ui);
            });
        })
    };
    let r = ui
        .interact(inner.response.rect, Id::new(id), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if r.hovered() && !on {
        ui.painter().rect_filled(
            inner.response.rect,
            CornerRadius::same(13),
            Color32::from_white_alpha(10),
        );
    }
    r
}

fn paint_slot_icon(ui: &mut Ui, icon: &str, col: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(16.0, 28.0), Sense::hover());
    paint_icon(ui, r.center(), icon, 15.0, col);
}

/// Walk: white split button (Walk · ▾ with Playtest and camera paths).
fn walk_split(ui: &mut Ui, out: &mut UiOut) {
    let font = FontId::new(13.0, semibold());
    let g = ui.painter().layout_no_wrap("Walk".into(), font, ON_ACCENT);
    let main_w = 12.0 + 15.0 + 7.0 + g.size().x + 12.0;
    let (rect, _) = ui.allocate_exact_size(vec2(main_w + 1.0 + 30.0, H), Sense::hover());
    let main = Rect::from_min_size(rect.min, vec2(main_w, H));
    let caret = Rect::from_min_max(pos2(main.right() + 1.0, rect.top()), rect.max);
    let p = ui.painter();
    p.rect_filled(
        rect.translate(vec2(0.0, 10.0)).shrink(6.0),
        CornerRadius::same(13),
        Color32::from_white_alpha(18),
    );
    p.rect_filled(rect, CornerRadius::same(13), Color32::WHITE);
    let rm = ui
        .interact(main, Id::new("halo_walk"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let rc = ui
        .interact(caret, Id::new("halo_walk_more"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if rm.hovered() {
        p.rect_filled(
            main,
            CornerRadius {
                nw: 13,
                sw: 13,
                ne: 0,
                se: 0,
            },
            Color32::from_black_alpha(15),
        );
    }
    if rc.hovered() {
        p.rect_filled(
            caret,
            CornerRadius {
                nw: 0,
                sw: 0,
                ne: 13,
                se: 13,
            },
            Color32::from_black_alpha(15),
        );
    }
    p.rect_filled(
        Rect::from_min_size(pos2(main.right(), rect.top() + 8.0), vec2(1.0, H - 16.0)),
        0.0,
        Color32::from_black_alpha(31),
    );
    paint_icon(
        ui,
        pos2(main.left() + 12.0 + 7.5, main.center().y),
        "Footsteps",
        15.0,
        ON_ACCENT,
    );
    ui.painter().galley(
        pos2(main.left() + 34.0, main.center().y - g.size().y * 0.5),
        g,
        ON_ACCENT,
    );
    paint_icon(ui, caret.center(), "ChevronDownSmall", 14.0, ON_ACCENT);
    if rm.on_hover_text("Explore on foot, swim, ride or fly: footsteps, zone weather and ambience (Esc to return).").clicked() {
        out.actions.write(UiAction::ToggleWalk);
    }
    egui::Popup::menu(&rc).frame(menu_frame()).align(RectAlign::BOTTOM_END).gap(8.0).show(|ui| {
        ui.set_min_width(250.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        if menu_row(ui, "Footsteps", "Walk the World", "", false).on_hover_text("Explore from where the camera looks").clicked() {
            out.actions.write(UiAction::ToggleWalk);
        }
        if menu_row(ui, "Gamepad", "Playtest", "", false).on_hover_text("Play from the player start: compass, minimap, discoveries; the route is recorded").clicked() {
            out.motion.write(crate::motion::MotionAction::Playtest);
        }
        menu_hr(ui);
        if menu_row(ui, "Video", "Camera paths & flythrough…", "", false).on_hover_text("Keyframe a camera path, preview it and capture a video").clicked() {
            out.motion.write(crate::motion::MotionAction::OpenPanel(crate::motion::PanelTab::Paths));
        }
        if menu_row(ui, "AnimatePath", "Playtest sessions…", "", false).on_hover_text("Recorded routes, heat map, summaries and export").clicked() {
            out.motion.write(crate::motion::MotionAction::OpenPanel(crate::motion::PanelTab::Playtests));
        }
    });
}

// ------------------------------------------------------------------ the rail

pub fn group_icon(g: TabGroup) -> &'static str {
    match g {
        TabGroup::Terrain => "FormPyramide",
        TabGroup::Water => "Drop",
        TabGroup::Nature => "Tree",
        TabGroup::Build => "3dBoxTop",
        TabGroup::Dungeons => "Castle",
        TabGroup::Gameplay => "Sword",
        TabGroup::Atmosphere => "CloudySun",
        TabGroup::Review => "Stethoscope",
    }
}

pub fn rail(ctx: &egui::Context, s: &mut UiState, out: &mut UiOut, screen: Rect) {
    let cur = s.ed.tab.group();
    let open = s.ed.tab;
    s.ed.group_tab.insert(cur, open);
    let groups = TabGroup::visible();
    let h = 10.0 + groups.len() as f32 * 40.0 + 7.0 + 2.0 * 40.0 - 2.0;
    let y = (screen.center().y - h * 0.5).max(TOP + 8.0);
    egui::Area::new(Id::new("halo_rail"))
        .order(Order::Middle)
        .fixed_pos(pos2(screen.left() + M, y))
        .show(ctx, |ui| {
            let frame = glass_frame()
                .corner_radius(CornerRadius::same(R_RAIL))
                .inner_margin(Margin::same(5));
            glass(ui, frame, |ui| {
                ui.set_width(38.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, g) in groups.iter().enumerate() {
                    let sel = cur == *g;
                    let (r, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
                    let p = ui.painter();
                    if sel {
                        p.rect_filled(
                            r.translate(vec2(0.0, 6.0)).shrink(4.0),
                            CornerRadius::same(12),
                            Color32::from_white_alpha(20),
                        );
                        p.rect_filled(r, CornerRadius::same(12), Color32::WHITE);
                    } else if resp.hovered() {
                        p.rect_filled(r, CornerRadius::same(12), SOFT);
                    }
                    let col = if sel {
                        ON_ACCENT
                    } else if resp.hovered() {
                        TEXT
                    } else {
                        TEXT_DIM
                    };
                    paint_icon(ui, r.center(), group_icon(*g), 18.0, col);
                    let n = ui.painter().layout_no_wrap(
                        (i + 1).to_string(),
                        FontId::new(7.5, mono_semibold()),
                        col.gamma_multiply(if sel { 0.5 } else { 0.45 }),
                    );
                    ui.painter()
                        .galley(pos2(r.right() - 3.0 - n.size().x, r.top() + 3.0), n, col);
                    let tabs: Vec<&str> = g.tabs().iter().map(|t| TabGroup::sub_name(*t)).collect();
                    let sub = if tabs.len() > 1 {
                        tabs.join(" · ")
                    } else {
                        String::new()
                    };
                    let resp = tip(resp, g.name(), &sub, &(i + 1).to_string(), TipSide::Right)
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() && !sel {
                        s.ed.tab = s.ed.group_tab.get(g).copied().unwrap_or(g.tabs()[0]);
                        out.sounds.write(SoundEvent::Ui("ui_click_0"));
                    }
                }
                ui.add_space(1.0);
                let (r, _) = ui.allocate_exact_size(vec2(38.0, 1.0), Sense::hover());
                ui.painter()
                    .rect_filled(r.shrink2(vec2(7.0, 0.0)), 0.0, LINE);
                ui.add_space(1.0);
                let muted = s.audio.muted;
                if rail_button(
                    ui,
                    if muted { "VolumeOff" } else { "VolumeFull" },
                    if muted { "Unmute sound" } else { "Mute sound" },
                    "",
                )
                .clicked()
                {
                    s.audio.muted = !muted;
                }
                if rail_button(ui, "QuestionmarkCircle", "Help & shortcuts", "H").clicked() {
                    s.ed.show_help = !s.ed.show_help;
                }
            });
        });
}

fn rail_button(ui: &mut Ui, icon: &str, title: &str, key: &str) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(12), SOFT);
    }
    paint_icon(
        ui,
        r.center(),
        icon,
        16.0,
        if resp.hovered() { TEXT } else { TEXT_DIM },
    );
    tip(resp, title, "", key, TipSide::Right).on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ status, toast

/// Bottom-left: fps · objects · zones, the scale bar and the compass (under the tool panel).
pub fn status(
    ctx: &egui::Context,
    s: &UiState,
    cam: &Camera,
    cam_t: &GlobalTransform,
    screen: Rect,
) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        Order::Background,
        Id::new("halo_status"),
    ));
    let x0 = screen.left() + if get(ctx).panel_open { 76.0 } else { 16.0 };
    let y = screen.bottom() - 16.0 - 13.0;
    let text = format!(
        "{:.0} fps · {} objects · {} zones",
        s.frames.fps,
        group_digits(s.store.count()),
        s.zones.list.len()
    );
    let col = Color32::from_rgba_unmultiplied_const(255, 255, 255, 199);
    let font = FontId::new(11.0, mono());
    let g = painter.layout_no_wrap(text.clone(), font.clone(), col);
    let shadow = painter.layout_no_wrap(text, font, Color32::from_black_alpha(150));
    let gw = g.size().x;
    painter.galley(
        pos2(x0, y - g.size().y * 0.5 + 1.0),
        shadow,
        Color32::from_black_alpha(150),
    );
    painter.galley(pos2(x0, y - g.size().y * 0.5), g, col);
    let mut x = x0 + gw + 10.0;
    // scale bar: a nice distance close to 52 px at the focus point
    let project = |p: Vec3| cam.world_to_viewport(cam_t, p).ok().map(|v| pos2(v.x, v.y));
    let focus = s.orbit.focus;
    if let (Some(a), Some(b)) = (project(focus), project(focus + cam_t.right() * 100.0)) {
        let px_per_m = (b - a).length() / 100.0;
        if px_per_m > 0.0 {
            let target = 60.0 / px_per_m;
            let nice = [
                1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0,
                10000.0,
            ];
            let m = *nice.iter().rev().find(|n| **n <= target).unwrap_or(&1.0);
            let len = (m * px_per_m).clamp(8.0, 90.0);
            let label = if m >= 1000.0 {
                format!("{:.0} km", m / 1000.0)
            } else {
                format!("{m:.0} m")
            };
            let lg = painter.layout_no_wrap(label, FontId::new(10.5, mono()), col);
            let mid = x + len * 0.5;
            painter.galley(pos2(mid - lg.size().x * 0.5, y - 12.0), lg, col);
            let by = y + 5.0;
            let st = Stroke::new(1.5, Color32::WHITE);
            painter.line_segment([pos2(x, by - 4.0), pos2(x, by)], st);
            painter.line_segment([pos2(x, by), pos2(x + len, by)], st);
            painter.line_segment([pos2(x + len, by - 4.0), pos2(x + len, by)], st);
            x += len + 12.0;
        }
    }
    // compass: the needle points north
    let c = pos2(x + 13.0, y);
    let fwd = cam_t.forward();
    let ang = fwd.x.atan2(-fwd.z);
    let rot = |v: egui::Vec2| {
        let (sn, cs) = (-ang).sin_cos();
        vec2(v.x * cs - v.y * sn, v.x * sn + v.y * cs)
    };
    painter.circle(
        c,
        13.0,
        Color32::from_black_alpha(90),
        Stroke::new(1.0, Color32::from_white_alpha(128)),
    );
    let (tipp, tail, side) = (
        c + rot(vec2(0.0, -10.0)),
        c + rot(vec2(0.0, 10.0)),
        rot(vec2(3.5, 0.0)),
    );
    let mid = |k: f32| c + rot(vec2(0.0, k));
    painter.add(Shape::convex_polygon(
        vec![tipp, c + side, mid(2.0), c - side],
        Color32::WHITE,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        vec![tail, c - side, mid(-2.0), c + side],
        Color32::from_white_alpha(90),
        Stroke::NONE,
    ));
    let _ = screen;
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{202F}');
        }
        out.push(ch);
    }
    out
}

/// The toast: a glass pill top-centre under the islands.
pub fn toast(ctx: &egui::Context, s: &UiState, screen: Rect) {
    let Some((msg, age)) = &s.ed.toast else {
        return;
    };
    let a = (1.0 - ((*age - 5.2) / 0.8).clamp(0.0, 1.0)) * (*age / 0.15).min(1.0);
    if a <= 0.0 {
        return;
    }
    let mut painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("toast")));
    painter.set_opacity(a);
    let g = painter.layout_no_wrap(msg.clone(), FontId::proportional(13.0), TEXT);
    let size = vec2(14.0 + 14.0 + 10.0 + g.size().x + 14.0, 34.0);
    let drop = (1.0 - (*age / 0.25).min(1.0)) * 10.0;
    let r = Rect::from_min_size(
        pos2(screen.center().x - size.x * 0.5, screen.top() + TOP - drop),
        size,
    );
    painter.add(soft_shadow(r, 17.0, Color32::from_black_alpha(140)));
    painter.rect(
        r,
        CornerRadius::same(255),
        Color32::from_rgba_unmultiplied_const(18, 21, 27, 230),
        Stroke::new(1.0, LINE_STRONG),
        StrokeKind::Inside,
    );
    super::icons::paint(
        &painter,
        pos2(r.left() + 21.0, r.center().y),
        "CheckCircle2",
        14.0,
        TEXT,
    );
    painter.galley(
        pos2(r.left() + 38.0, r.center().y - g.size().y * 0.5),
        g,
        TEXT,
    );
}

/// Where the dock goes: centred between the tool panel and the inspector.
pub fn dock_center_x(ctx: &egui::Context, screen: Rect, inspector: bool) -> f32 {
    let sh = get(ctx);
    let lx = screen.left()
        + if sh.panel_open {
            PANEL_X + PANEL_W + 14.0
        } else {
            72.0
        };
    let rx = if inspector && sh.insp_open {
        screen.right() - M - INSP_W
    } else {
        screen.right()
    };
    (lx + rx) * 0.5
}
