//! The guided setup's pages (File › New world › Guided setup): a full-window card wizard over the
//! editor. Nine pages: connect an AI (optional), the MMOs the user loves, the kind of game, the
//! land (and their own words, read by the AI), towns and names, adventure and gathering, the look,
//! a review, and the build's progress. It only fills `Setup.plan` and sends `SetupRequest`s; the
//! world is made by `build`.
//!
//! Esc or ✕ closes it (the answers stay); Enter goes on to the next page. While a build runs with
//! the wizard closed, a small pill at the bottom shows its progress.
//!
//! Test hooks (screenshots): `WB_SETUP=1` opens it on start, `WB_SETUP_STEP=<page 1–9>` opens it
//! on that page, `WB_SETUP_DEMO=ai|building|done|failed|pill|unsaved|scifi` shows a sample AI answer, a
//! sample build (on the last page, or as the pill with the wizard closed) or the unsaved-changes
//! question (nothing is built).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Margin, Mesh, Pos2, Rect,
    Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;

use super::plan::{
    Art, Climate, GameKind, INSPIRATIONS, Inspiration, Land, Names, WorldPlan, WorldSize,
    inspiration,
};
use super::{BuildState, Setup, SetupRequest};
use crate::dungeons::presets::{self as dpresets, RoomSize};
use crate::look::LookPreset;
use crate::prompt::PromptSession;
use crate::prompt::agent::{self, Backend};
use crate::ui::UiAction;
use crate::ui::theme::*;
use crate::world::WorldMeta;

/// The panel's own (opaque) colour: picture cards mask their corners with it.
const PANEL: Color32 = Color32::from_rgb(18, 21, 27);
const LINE: Color32 = Color32::from_rgb(43, 46, 52);
const GREEN: Color32 = OK;
const AMBER: Color32 = WARN;
const ACCENT_LIGHT: Color32 = TEXT;
const GAP: f32 = 12.0;
const REVIEW: usize = 7;
const BUILDING: usize = 8;

struct Page {
    short: &'static str,
    title: &'static str,
    sub: &'static str,
}

const PAGES: [Page; 9] = [
    Page {
        short: "AI",
        title: "Connect your AI",
        sub: "Optional. An AI can read a description of your world in your own words and make up names for its places. Everything else works with cards.",
    },
    Page {
        short: "Inspiration",
        title: "Worlds you love",
        sub: "Pick up to three. Your first pick suggests the art, the look, the kind of game and the levels; you can change each one later.",
    },
    Page {
        short: "Game",
        title: "Your game",
        sub: "What kind of MMO is it? It decides how many dungeons, frontiers and resources the world gets.",
    },
    Page {
        short: "World",
        title: "Your world",
        sub: "Describe it in your own words, or pick the shape, size and climate below.",
    },
    Page {
        short: "Towns",
        title: "Towns & realms",
        sub: "Where people live, how they're joined up, and what the places are called.",
    },
    Page {
        short: "Adventure",
        title: "Adventure",
        sub: "Dungeons, caves and monsters, and what can be gathered out in the wilds.",
    },
    Page {
        short: "Look",
        title: "Look & feel",
        sub: "The art style, the colours and light, and the time of day the world opens at.",
    },
    Page {
        short: "Review",
        title: "Ready to build",
        sub: "Here's your world. Give it a name and build it: it takes seconds, and you can change everything afterwards.",
    },
    Page {
        short: "Build",
        title: "Building your world",
        sub: "",
    },
];

static OPEN: AtomicBool = AtomicBool::new(false);

/// Opens the guided setup: on its first page (earlier answers kept), or on the build's page while
/// one is running.
pub fn request_open() {
    OPEN.store(true, Ordering::Relaxed);
}

/// The wizard's own state (not part of the plan).
#[derive(Default)]
pub struct Wiz {
    init: bool,
    demo: Option<String>,
    /// pictures by key (`assets/onboarding/<key>.jpg`)
    textures: HashMap<String, egui::TextureHandle>,
    image_rx: Option<Mutex<Receiver<(String, Option<egui::ColorImage>)>>>,
    images_started: bool,
    /// "I'll answer with cards only": the description reader is off
    cards_only: bool,
    checking_since: Option<f64>,
    /// "unsaved changes: save first / discard / cancel" is showing
    confirm: bool,
    /// "Save first" was chosen: the build starts once the world is saved
    save_then_build: Option<f64>,
    build_asked_at: Option<f64>,
    flash: Option<(String, f64)>,
    shown_step: usize,
    step_at: f64,
    /// the done page's places list unfolded
    places_all: bool,
    /// the wizard was closed to look at the new world: a pill brings the summary back
    peek: bool,
}

struct Cx<'a> {
    setup: &'a mut Setup,
    prompt: &'a mut PromptSession,
    wiz: &'a mut Wiz,
    dirty: bool,
    /// a dungeon is open (the build needs the world)
    in_dungeon: bool,
    /// where the open world is saved
    world_path: Option<std::path::PathBuf>,
    /// what the finished build made where (name, kind), and whether it knows where players start
    places: Vec<(String, &'static str)>,
    has_start: bool,
    actions: Vec<UiAction>,
    now: f64,
}

pub fn wizard_ui(
    mut contexts: EguiContexts,
    mut setup: ResMut<Setup>,
    mut prompt: ResMut<PromptSession>,
    meta: Res<WorldMeta>,
    mut actions: MessageWriter<UiAction>,
    mut wiz: Local<Wiz>,
    dungeon: Res<crate::dungeons::DungeonMap>,
    run: Res<super::build::BuildRun>,
) -> Result {
    let ctx = contexts.ctx_mut()?.clone();
    // (the theme's fonts become usable a frame after the editor's UI installs them)
    if !ctx.fonts(|f| f.definitions().families.contains_key(&semibold())) {
        return Ok(());
    }
    let now = ctx.input(|i| i.time);
    if !wiz.init {
        init_hooks(&mut setup, &mut wiz);
    }
    if OPEN.swap(false, Ordering::Relaxed) {
        setup.open = true;
        setup.step = if setup.build.as_ref().is_some_and(|b| !b.done) {
            BUILDING
        } else {
            0
        };
        wiz.confirm = false;
        wiz.save_then_build = None;
        prompt.detect();
        wiz.checking_since = Some(now);
    }
    if !setup.open {
        let demo =
            (wiz.demo.as_deref() == Some("pill")).then(|| demo_build("building", &setup.plan));
        let pill = match (&demo, setup.build.as_ref()) {
            (Some(b), _) => building_pill(&ctx, b),
            (None, Some(b)) if !b.done || (wiz.peek && b.failed.is_none() && !b.cancelled) => {
                building_pill(&ctx, b)
            }
            _ => Pill::Nothing,
        };
        match pill {
            Pill::Open => {
                setup.open = true;
                setup.step = BUILDING;
                wiz.peek = false;
            }
            Pill::Dismiss => wiz.peek = false,
            Pill::Nothing => {}
        }
        return Ok(());
    }
    if !wiz.images_started {
        start_images(&mut wiz);
    }
    poll_images(&ctx, &mut wiz);
    let mut c = Cx {
        setup: &mut setup,
        prompt: &mut prompt,
        wiz: &mut wiz,
        dirty: meta.dirty,
        in_dungeon: dungeon.0.is_some(),
        world_path: meta.path.clone(),
        places: vec![],
        has_start: run.start.is_some(),
        actions: vec![],
        now,
    };
    if c.setup.step == BUILDING {
        c.places = if c.wiz.demo.as_deref() == Some("done") {
            DEMO_PLACES
                .iter()
                .map(|(n, k)| (n.to_string(), *k))
                .collect()
        } else {
            run.places.iter().map(|(n, k, _)| (n.clone(), *k)).collect()
        };
        c.has_start |= c.wiz.demo.as_deref() == Some("done");
    }
    // "Save first": build once the save has gone through
    if c.wiz.save_then_build.is_some() && !c.dirty {
        c.wiz.save_then_build = None;
        c.wiz.confirm = false;
        if !c.in_dungeon {
            start_build(&mut c);
        }
    }
    show(&ctx, &mut c);
    for a in c.actions {
        actions.write(a);
    }
    Ok(())
}

/// `WB_SETUP`, `WB_SETUP_STEP`, `WB_SETUP_DEMO`.
fn init_hooks(setup: &mut Setup, wiz: &mut Wiz) {
    wiz.init = true;
    if std::env::var("WB_SETUP").is_ok() {
        setup.open = true;
    }
    if let Some(n) = std::env::var("WB_SETUP_STEP")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        setup.open = true;
        setup.step = n.saturating_sub(1).min(BUILDING);
    }
    wiz.demo = std::env::var("WB_SETUP_DEMO").ok();
    match wiz.demo.as_deref() {
        // the build's pill (the wizard closed)
        Some("pill") => setup.open = false,
        // "unsaved changes: save first / discard / cancel"
        Some("unsaved") => wiz.confirm = true,
        // a Sci-Fi world picked (its World and Look pages)
        Some("scifi") => setup.plan.kind = crate::world_kind::WorldKind::SciFi,
        _ => {}
    }
    if wiz.demo.as_deref() == Some("ai") {
        let p = &mut setup.plan;
        p.description = "A cold northern continent with jungle islands in the south, three realms at war, and a dwarven capital in the mountains.".into();
        p.realms = 3;
        p.game = GameKind::RealmWar;
        p.names = Names {
            world: "Vaelmoor".into(),
            capital: "Khazad Orn".into(),
            towns: vec!["Frostgate".into(), "Emberhollow".into(), "Saltmere".into()],
            zones: vec!["The Rime Wastes".into(), "Verdant Shoals".into()],
            dungeons: vec!["The Sunken Forge".into()],
            realms: vec!["Hrimfold".into(), "Aurel".into(), "The Tidebound".into()],
        };
        setup.ai.understood = "A big, varied continent: snowy north, tropical islands in the south. Three realms fight over the middle, and the capital is a dwarven city high in the mountains.".into();
    }
}

/// Closes the wizard (the answers stay); a build still running keeps a pill on screen.
fn close(c: &mut Cx) {
    c.setup.open = false;
    if c.setup.build.as_ref().is_some_and(|b| !b.done) {
        c.wiz.peek = true;
    }
}

fn go(c: &mut Cx, step: usize) {
    c.setup.step = step.min(BUILDING);
}

fn show(ctx: &egui::Context, c: &mut Cx) {
    let screen = ctx.content_rect();
    let size = vec2(
        (screen.width() - 40.0).clamp(520.0, 980.0),
        (screen.height() - 40.0).clamp(440.0, 780.0),
    );
    let typing = ctx.egui_wants_keyboard_input();
    if !typing && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if c.wiz.confirm {
            c.wiz.confirm = false;
            c.wiz.save_then_build = None;
        } else {
            close(c);
            return;
        }
    }
    if !typing && c.setup.step < REVIEW && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
        go(c, c.setup.step + 1);
    }
    c.setup.step = c.setup.step.min(BUILDING);
    if c.wiz.shown_step != c.setup.step {
        c.wiz.shown_step = c.setup.step;
        c.wiz.step_at = c.now;
        // arriving at the review unnamed (and no AI name): start from a made-up one, not "New World"
        let plan = &mut c.setup.plan;
        if c.setup.step == REVIEW
            && plan.name.trim().is_empty()
            && plan.names.world.trim().is_empty()
        {
            roll_name(plan);
        }
    }
    let frame = egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(18))
        .shadow(egui::epaint::Shadow {
            offset: [0, 18],
            blur: 60,
            spread: 0,
            color: Color32::from_black_alpha(150),
        });
    egui::Modal::new(Id::new("guided_setup"))
        .backdrop_color(Color32::from_black_alpha(205))
        .frame(frame)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let head = Rect::from_min_size(rect.min, vec2(size.x, 98.0));
            let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 68.0), rect.max);
            let body = Rect::from_min_max(
                pos2(rect.left(), head.bottom()),
                pos2(rect.right(), foot.top()),
            );
            header(ui, head, c);
            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                let step = c.setup.step;
                let out = egui::ScrollArea::vertical()
                    .id_salt(("setup_page", step))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(Margin {
                                left: 30,
                                right: 30,
                                top: 16,
                                bottom: 18,
                            })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                // each page fades in
                                let t = ((c.now - c.wiz.step_at) / 0.22).clamp(0.0, 1.0) as f32;
                                if t < 1.0 {
                                    ui.ctx().request_repaint();
                                }
                                ui.multiply_opacity(1.0 - (1.0 - t).powi(2));
                                if step != BUILDING {
                                    page_title(ui, PAGES[step].title, PAGES[step].sub);
                                }
                                match step {
                                    0 => page_ai(ui, c),
                                    1 => page_inspirations(ui, c),
                                    2 => page_game(ui, c),
                                    3 => page_world(ui, c),
                                    4 => page_towns(ui, c),
                                    5 => page_adventure(ui, c),
                                    6 => page_look(ui, c),
                                    7 => page_review(ui, c),
                                    _ => page_build(ui, c),
                                }
                            });
                    });
                // more below: a fade at the bottom edge
                let below = out.content_size.y - out.state.offset.y - out.inner_rect.height();
                if below > 4.0 {
                    let r = out.inner_rect;
                    let fade = Rect::from_min_max(
                        pos2(r.left(), r.bottom() - 36.0),
                        pos2(r.right() - 12.0, r.bottom()),
                    );
                    let mut m = Mesh::default();
                    vgrad(&mut m, fade, Color32::TRANSPARENT, PANEL);
                    ui.painter().add(Shape::mesh(m));
                    ui.painter().text(
                        pos2(r.center().x, r.bottom() - 11.0),
                        Align2::CENTER_CENTER,
                        format!("{}  more below", icon::CARET_DOWN),
                        FontId::proportional(11.5),
                        TEXT_DIM,
                    );
                }
            });
            footer(ui, foot, c);
        });
    if c.wiz.confirm {
        confirm_unsaved(ctx, c);
    }
}

/// Before a build replaces a world with unsaved changes: save first, discard them, or cancel.
fn confirm_unsaved(ctx: &egui::Context, c: &mut Cx) {
    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(22, 25, 31))
        .stroke(Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(22))
        .shadow(egui::epaint::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(160),
        });
    egui::Modal::new(Id::new("guided_setup_unsaved")).backdrop_color(Color32::from_black_alpha(90)).frame(frame).show(ctx, |ui| {
        ui.set_width(430.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::WARNING).size(26.0).color(AMBER));
            ui.vertical(|ui| {
                ui.label(RichText::new("Your world has unsaved changes").font(FontId::new(17.0, semibold())).color(Color32::WHITE));
                ui.add(egui::Label::new(RichText::new("Building makes a new world in its place. Save the open one first to keep your changes.").color(TEXT_DIM)).wrap());
            });
        });
        ui.add_space(14.0);
        if c.wiz.save_then_build.is_some() {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
                let what = if c.world_path.is_some() { "Saving…" } else { "Choose where to save it…" };
                ui.label(RichText::new(format!("{what} the build starts right after.")).color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ghost(ui, "Cancel").clicked() {
                        c.wiz.save_then_build = None;
                        c.wiz.confirm = false;
                    }
                });
            });
            return;
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if primary(ui, &format!("{}  Save first", icon::FLOPPY_DISK)).on_hover_text("Save the open world, then build").clicked() {
                c.actions.push(UiAction::Save);
                c.wiz.save_then_build = Some(c.now);
            }
            let discard = ui.add(
                egui::Button::new(RichText::new("Discard changes").color(DANGER))
                    .fill(Color32::from_rgba_unmultiplied(239, 88, 80, 18))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(239, 88, 80, 120)))
                    .corner_radius(CornerRadius::same(10))
                    .min_size(vec2(0.0, 38.0)),
            );
            if discard.on_hover_text("Build now; the open world's unsaved changes are lost").clicked() {
                c.wiz.confirm = false;
                start_build(c);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ghost(ui, "Cancel").clicked() {
                    c.wiz.confirm = false;
                }
            });
        });
    });
}

// ---------------------------------------------------------------------------------------------
// frame: header (steps) and footer (Back / Next)

fn header(ui: &mut Ui, r: Rect, c: &mut Cx) {
    let step = c.setup.step;
    let p = ui.painter().clone();
    let x0 = r.left() + 30.0;
    let cap = p.text(
        pos2(x0, r.top() + 27.0),
        Align2::LEFT_CENTER,
        format!("{}  Guided setup", icon::SPARKLE),
        FontId::new(13.5, semibold()),
        ACCENT_LIGHT,
    );
    if step < BUILDING {
        p.text(
            pos2(cap.right() + 10.0, r.top() + 27.0),
            Align2::LEFT_CENTER,
            format!("·  Step {} of 8", step + 1),
            FontId::proportional(12.5),
            TEXT_DIM,
        );
    }
    // ✕
    let xr = Rect::from_center_size(pos2(r.right() - 30.0, r.top() + 27.0), vec2(30.0, 30.0));
    let cr = ui
        .interact(xr, Id::new("setup_close"), Sense::click())
        .on_hover_text("Close (Esc): your answers are kept");
    if cr.hovered() {
        p.circle_filled(xr.center(), 15.0, Color32::from_white_alpha(18));
    }
    p.text(
        xr.center(),
        Align2::CENTER_CENTER,
        icon::X,
        FontId::proportional(17.0),
        if cr.hovered() {
            Color32::WHITE
        } else {
            TEXT_DIM
        },
    );
    if cr.clicked() {
        close(c);
    }
    // the steps: dots on a line, labels underneath
    let n = PAGES.len();
    let (left, right) = (r.left() + 64.0, r.right() - 64.0);
    let dx = (right - left) / (n - 1) as f32;
    let y = r.top() + 60.0;
    let has_build = c.setup.build.is_some() || c.wiz.demo.as_deref().is_some_and(|d| d != "ai");
    p.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(2.0, LINE));
    if step > 0 {
        p.line_segment(
            [pos2(left, y), pos2(left + dx * step as f32, y)],
            Stroke::new(2.0, ACCENT),
        );
    }
    for (i, page) in PAGES.iter().enumerate() {
        let ctr = pos2(left + dx * i as f32, y);
        let hit = Rect::from_center_size(ctr + vec2(0.0, 9.0), vec2(dx.min(90.0), 36.0));
        let can = i != step && (i < BUILDING || has_build);
        let resp = ui.interact(
            hit,
            Id::new(("setup_dot", i)),
            if can { Sense::click() } else { Sense::hover() },
        );
        let hov = can && resp.hovered();
        if i < step {
            p.circle_filled(ctr, 8.0, ACCENT);
            p.text(
                ctr,
                Align2::CENTER_CENTER,
                icon::CHECK,
                FontId::proportional(10.0),
                ON_ACCENT,
            );
        } else if i == step {
            p.circle_filled(ctr, 13.0, Color32::from_white_alpha(28));
            p.circle_filled(ctr, 8.5, ACCENT);
            p.circle_filled(ctr, 3.5, ON_ACCENT);
        } else {
            p.circle_filled(ctr, 6.5, PANEL);
            p.circle_stroke(
                ctr,
                6.5,
                Stroke::new(
                    1.5,
                    if hov {
                        TEXT
                    } else {
                        Color32::from_rgb(80, 84, 90)
                    },
                ),
            );
        }
        let col = if i == step {
            Color32::WHITE
        } else if hov {
            Color32::WHITE
        } else if i < step {
            TEXT
        } else {
            TEXT_DIM
        };
        let font = if i == step {
            FontId::new(11.5, semibold())
        } else {
            FontId::proportional(11.5)
        };
        p.text(
            ctr + vec2(0.0, 20.0),
            Align2::CENTER_CENTER,
            page.short,
            font,
            col,
        );
        if can
            && resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
        {
            go(c, i);
        }
    }
    p.line_segment(
        [pos2(r.left(), r.bottom()), pos2(r.right(), r.bottom())],
        Stroke::new(1.0, LINE),
    );
}

fn footer(ui: &mut Ui, r: Rect, c: &mut Cx) {
    ui.painter().line_segment(
        [pos2(r.left(), r.top()), pos2(r.right(), r.top())],
        Stroke::new(1.0, LINE),
    );
    let step = c.setup.step;
    let inner = r.shrink2(vec2(30.0, 14.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if step == BUILDING {
            build_footer(ui, c);
            return;
        }
        if step > 0 && secondary(ui, &format!("{}  Back", icon::ARROW_LEFT)).clicked() {
            go(c, step - 1);
        }
        if (1..REVIEW).contains(&step)
            && ghost(ui, &format!("{}  Surprise me", icon::DICE_FIVE))
                .on_hover_text("Fill in every page with sensible random picks (your description and names stay)")
                .clicked()
        {
            surprise(&mut c.setup.plan);
            c.wiz.flash = Some(("Shuffled! Have a look round and change anything you like.".into(), c.now));
        }
        if let Some((msg, at)) = &c.wiz.flash {
            if c.now - at < 3.0 {
                ui.label(RichText::new(msg).small().color(ACCENT_LIGHT));
            } else {
                c.wiz.flash = None;
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if step == REVIEW {
                let b = ui
                    .add_enabled(
                        !c.in_dungeon,
                        egui::Button::new(RichText::new(format!("{}  Build my world", icon::ROCKET_LAUNCH)).font(FontId::new(15.5, semibold())).color(ON_ACCENT))
                            .fill(ACCENT)
                            .corner_radius(CornerRadius::same(10))
                            .min_size(vec2(200.0, 40.0)),
                    )
                    .on_disabled_hover_text("Go back to the world first: a dungeon is open");
                if b.clicked() {
                    if c.dirty {
                        c.wiz.confirm = true;
                    } else {
                        start_build(c);
                    }
                }
            } else {
                let next = if step == 0 && !ai_ready(c) { "Continue without AI" } else { "Next" };
                if primary(ui, &format!("{next}  {}", icon::ARROW_RIGHT)).on_hover_text("Enter").clicked() {
                    go(c, step + 1);
                }
                if step > 0 && ghost(ui, "Skip to review").on_hover_text("Keep the suggestions for the rest and go to the review").clicked() {
                    go(c, REVIEW);
                }
            }
        });
    });
}

fn start_build(c: &mut Cx) {
    let plan = c.setup.plan.normalized();
    c.setup.requests.push(SetupRequest::Build(plan));
    c.wiz.build_asked_at = Some(c.now);
    c.wiz.demo = None;
    c.wiz.peek = false;
    c.wiz.places_all = false;
    go(c, BUILDING);
}

// ---------------------------------------------------------------------------------------------
// page 1: AI

fn backend_ready(p: &PromptSession, b: Backend) -> bool {
    p.ready(b)
}

fn ai_ready(c: &Cx) -> bool {
    !c.wiz.cards_only && backend_ready(c.prompt, c.prompt.backend)
}

/// (label, colour, note) for a backend's card.
fn backend_state(p: &PromptSession, b: Backend) -> (&'static str, Color32, String) {
    let Some(st) = p.status(b) else {
        return ("Checking…", TEXT_DIM, String::new());
    };
    if b == Backend::Api {
        let name = agent::provider(&p.api_base).map_or("your provider", |p| p.name);
        return if p.ready(b) {
            ("Ready", GREEN, format!("{name}: {}", st.note))
        } else {
            (
                "Needs a key",
                AMBER,
                "Pick a provider and paste its key below.".into(),
            )
        };
    }
    if b == Backend::ClaudeCode && !st.ready && st.path.is_some() {
        return if p.login.is_some() {
            ("Signing in…", AMBER, String::new())
        } else {
            ("Not signed in", AMBER, String::new())
        };
    }
    if st.ready {
        ("Ready", GREEN, st.note.clone())
    } else if st.path.is_none() {
        ("Not installed", TEXT_DIM, st.note.clone())
    } else {
        ("Not signed in", AMBER, st.note.clone())
    }
}

fn page_ai(ui: &mut Ui, c: &mut Cx) {
    let w = ui.available_width();
    let cols = if w >= 640.0 { 3 } else { 1 };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    for row in Backend::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &b in row {
                let card = backend_card(ui, c, b, vec2(cw, 176.0));
                if card.clicked() {
                    c.prompt.set_backend(b);
                    c.wiz.cards_only = false;
                }
                // Claude Code installed but signed out: sign in right on its card
                let signed_out = c
                    .prompt
                    .status(b)
                    .is_some_and(|st| !st.ready && st.path.is_some());
                if b == Backend::ClaudeCode && signed_out && c.prompt.login.is_none() {
                    let r = Rect::from_min_size(
                        pos2(card.rect.left() + 16.0, card.rect.bottom() - 54.0),
                        vec2((card.rect.width() - 32.0).min(220.0), 38.0),
                    );
                    let btn = egui::Button::new(RichText::new(format!("{}  Sign in with Claude", icon::SIGN_IN)).font(FontId::new(14.0, semibold())).color(ON_ACCENT))
                        .fill(ACCENT)
                        .corner_radius(CornerRadius::same(10));
                    // (a child Ui: drawn over the card without moving the row on)
                    let mut over = ui.new_child(egui::UiBuilder::new().max_rect(r));
                    if over
                        .add_sized(r.size(), btn)
                        .on_hover_text("Opens claude.ai in your browser to sign in with your Claude account")
                        .clicked()
                    {
                        c.prompt.set_backend(b);
                        c.wiz.cards_only = false;
                        c.prompt.start_claude_login();
                    }
                }
            }
        });
    }
    ui.add_space(8.0);
    if c.prompt.backend == Backend::Api && !c.wiz.cards_only {
        crate::prompt::panel::api_key_row(ui, c.prompt, 320.0);
        crate::prompt::panel::api_model_row(ui, c.prompt);
        ui.add_space(6.0);
    }
    // (a sign-in in progress, or how it went)
    if c.prompt.login.is_some() || c.prompt.login_error.is_some() {
        crate::prompt::panel::claude_signin_row(ui, c.prompt);
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        let checking =
            c.prompt.statuses.is_none() || c.wiz.checking_since.is_some_and(|t| c.now - t < 1.2);
        if checking {
            ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
            ui.label(RichText::new("Checking what's installed…").color(TEXT_DIM));
        } else if secondary(ui, &format!("{}  Re-check", icon::ARROW_CLOCKWISE))
            .on_hover_text("Signed in just now? Check again")
            .clicked()
        {
            c.prompt.detect();
            c.wiz.checking_since = Some(c.now);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ghost(
                ui,
                &format!("Skip — I'll answer with cards only  {}", icon::ARROW_RIGHT),
            )
            .clicked()
            {
                c.wiz.cards_only = true;
                go(c, 1);
            }
        });
    });
    ui.add_space(10.0);
    let (col, text) = if c.wiz.cards_only {
        (
            TEXT_DIM,
            "Cards only: the description reader is off. Pick an AI above any time to turn it on."
                .to_string(),
        )
    } else if ai_ready(c) {
        (
            GREEN,
            format!(
                "{}  {} is ready: on page 4 you can describe your world in your own words.",
                icon::CHECK_CIRCLE,
                c.prompt.backend.name()
            ),
        )
    } else {
        (
            TEXT_DIM,
            format!(
                "{}  No AI yet? That's fine: every question has cards to pick from. The AI only reads your words and makes up names.",
                icon::INFO
            ),
        )
    };
    info_card(ui, &text, col);
    // what's ahead
    ui.add_space(18.0);
    section_title(ui, "How it works");
    let w = ui.available_width();
    let cw = (w - GAP * 2.0) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (i, (ic, title, text)) in [
            (
                icon::HEART,
                "Pick what you love",
                "MMOs you like, the kind of game, the land and its climate",
            ),
            (
                icon::MAP_TRIFOLD,
                "Fill it with life",
                "Towns and roads, zones, dungeons, caves, monsters and resources",
            ),
            (
                icon::ROCKET_LAUNCH,
                "Build and explore",
                "A playable world in seconds: fly through it or walk in",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let (rect, _) = ui.allocate_exact_size(vec2(cw, 74.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(12), Color32::from_white_alpha(6));
            let n = pos2(rect.left() + 26.0, rect.top() + 26.0);
            p.circle_filled(n, 14.0, Color32::from_white_alpha(30));
            p.text(
                n,
                Align2::CENTER_CENTER,
                ic,
                FontId::proportional(15.0),
                ACCENT_LIGHT,
            );
            p.text(
                pos2(rect.left() + 50.0, rect.top() + 18.0),
                Align2::LEFT_CENTER,
                format!("{}. {title}", i + 1),
                FontId::new(13.5, semibold()),
                Color32::WHITE,
            );
            let g = p.layout(
                text.to_string(),
                FontId::proportional(11.5),
                TEXT_DIM,
                rect.width() - 62.0,
            );
            p.galley(pos2(rect.left() + 50.0, rect.top() + 30.0), g, TEXT_DIM);
        }
    });
}

fn backend_card(ui: &mut Ui, c: &Cx, b: Backend, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let selected = c.prompt.backend == b && !c.wiz.cards_only;
    card_bg(ui, rect, selected, resp.hovered());
    let p = ui.painter();
    let (glyph, sub) = match b {
        Backend::Codex => (icon::CHAT_CIRCLE_DOTS, "Sign in with ChatGPT"),
        Backend::ClaudeCode => (icon::TERMINAL_WINDOW, "Use your Claude account"),
        Backend::Api => (icon::KEY, "OpenRouter, OpenAI and more"),
    };
    let ib = Rect::from_min_size(rect.min + vec2(16.0, 16.0), vec2(42.0, 42.0));
    p.rect_filled(
        ib,
        CornerRadius::same(11),
        if selected {
            ACCENT
        } else {
            Color32::from_white_alpha(14)
        },
    );
    p.text(
        ib.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(21.0),
        if selected { ON_ACCENT } else { Color32::WHITE },
    );
    p.text(
        pos2(ib.right() + 12.0, ib.top() + 11.0),
        Align2::LEFT_CENTER,
        b.name(),
        FontId::new(15.5, semibold()),
        Color32::WHITE,
    );
    p.text(
        pos2(ib.right() + 12.0, ib.top() + 31.0),
        Align2::LEFT_CENTER,
        sub,
        FontId::proportional(12.0),
        TEXT_DIM,
    );
    let (label, col, note) = backend_state(c.prompt, b);
    let pill_r = status_pill(p, pos2(rect.left() + 16.0, ib.bottom() + 14.0), label, col);
    let g = p.layout(
        note,
        FontId::proportional(11.5),
        TEXT_DIM,
        rect.width() - 32.0,
    );
    let room = rect.bottom() - 12.0 - (pill_r.bottom() + 10.0);
    if g.size().y <= room + 1.0 {
        p.galley(
            pos2(rect.left() + 16.0, pill_r.bottom() + 10.0),
            g,
            TEXT_DIM,
        );
    } else {
        // too long for the card: the first lines, the rest on hover
        p.with_clip_rect(Rect::from_min_max(
            pos2(rect.left(), pill_r.bottom() + 8.0),
            pos2(rect.right(), rect.bottom() - 10.0),
        ))
        .galley(
            pos2(rect.left() + 16.0, pill_r.bottom() + 10.0),
            g,
            TEXT_DIM,
        );
    }
    if selected {
        check_badge(p, pos2(rect.right() - 20.0, rect.top() + 20.0), icon::CHECK);
    }
    let note = backend_state(c.prompt, b).2;
    let resp = if note.is_empty() {
        resp
    } else {
        resp.on_hover_text(note)
    };
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn status_pill(p: &egui::Painter, at: Pos2, label: &str, col: Color32) -> Rect {
    let g = p.layout_no_wrap(label.to_string(), FontId::new(11.5, semibold()), col);
    let r = Rect::from_min_size(at, vec2(g.size().x + 26.0, 22.0));
    p.rect_filled(
        r,
        CornerRadius::same(11),
        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 26),
    );
    p.circle_filled(pos2(r.left() + 11.0, r.center().y), 3.5, col);
    p.galley(
        pos2(r.left() + 19.0, r.center().y - g.size().y / 2.0),
        g,
        col,
    );
    r
}

// ---------------------------------------------------------------------------------------------
// page 2: inspirations

fn page_inspirations(ui: &mut Ui, c: &mut Cx) {
    let w = ui.available_width();
    let cols = if w >= 760.0 {
        4
    } else if w >= 520.0 {
        3
    } else {
        2
    };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    let ch = (cw * 0.9).clamp(150.0, 196.0);
    let mut changed = false;
    for row in INSPIRATIONS.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for insp in row {
                let pick = c.setup.plan.inspirations.iter().position(|x| x == insp.id);
                if inspiration_card(ui, c.wiz, insp, pick, vec2(cw, ch)).clicked() {
                    let list = &mut c.setup.plan.inspirations;
                    match pick {
                        Some(i) => {
                            list.remove(i);
                            changed = true;
                        }
                        None if list.len() < 3 => {
                            list.push(insp.id.to_string());
                            changed = true;
                        }
                        None => {
                            c.wiz.flash = Some((
                                "Up to three: click a picked one to take it off.".into(),
                                c.now,
                            ))
                        }
                    }
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    if changed {
        let p = &mut c.setup.plan;
        p.apply_inspirations();
        if let Some((hour, ..)) = look_of(&p.look).scene() {
            p.hour = hour;
        }
    }
    ui.add_space(4.0);
    let plan = &c.setup.plan;
    if let Some(first) = plan.inspirations.first().and_then(|id| inspiration(id)) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}  {} suggests", icon::SPARKLE, first.name))
                    .color(ACCENT_LIGHT),
            );
            tag(ui, icon::PAINT_BRUSH, &format!("{} art", first.art.name()));
            tag(ui, icon::PALETTE, look_of(first.look).name());
            tag(ui, icon::GAME_CONTROLLER, first.game.name());
            tag(
                ui,
                icon::STAIRS,
                &format!("levels {}–{}", first.levels[0], first.levels[1]),
            );
        });
    } else {
        dim_line(
            ui,
            "Nothing picked yet, and that's fine: Next keeps the suggestions as they are.",
        );
    }
    ui.add_space(4.0);
    dim_line(
        ui,
        "Only for inspiration: the pictures are our own, and nothing is copied from these games.",
    );
}

fn inspiration_card(
    ui: &mut Ui,
    wiz: &Wiz,
    insp: &Inspiration,
    pick: Option<usize>,
    size: Vec2,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hov = resp.hovered();
    let p = ui.painter_at(rect.expand(10.0));
    let rad = 14u8;
    match wiz.textures.get(insp.id) {
        Some(t) => paint_cover(ui, t, rect, rad),
        None => paint_landscape(
            &p,
            rect,
            look_of(insp.look).palette(),
            seed_of(insp.id),
            insp.art == Art::HandPainted,
        ),
    }
    if hov {
        p.rect_filled(rect, CornerRadius::same(rad), Color32::from_white_alpha(14));
    }
    scrim(&p, rect, 0.32, 235);
    let pad = 13.0;
    let blurb = p.layout(
        insp.blurb.to_string(),
        FontId::proportional(11.5),
        Color32::from_rgb(208, 214, 224),
        rect.width() - pad * 2.0,
    );
    let by = rect.bottom() - pad - blurb.size().y;
    let name = p.layout(
        insp.name.to_string(),
        FontId::new(16.0, semibold()),
        Color32::WHITE,
        rect.width() - pad * 2.0,
    );
    let ny = by - 4.0 - name.size().y;
    p.text(
        pos2(rect.left() + pad, ny - 3.0),
        Align2::LEFT_BOTTOM,
        "INSPIRED BY",
        FontId::new(9.5, semibold()),
        Color32::from_white_alpha(150),
    );
    p.galley(pos2(rect.left() + pad, ny), name, Color32::WHITE);
    p.galley(pos2(rect.left() + pad, by), blurb, Color32::WHITE);
    corner_mask(&p, rect, rad);
    if let Some(i) = pick {
        p.rect_stroke(
            rect,
            CornerRadius::same(rad),
            Stroke::new(3.0, ACCENT),
            StrokeKind::Outside,
        );
        check_badge(
            &p,
            pos2(rect.right() - 20.0, rect.top() + 20.0),
            &(i + 1).to_string(),
        );
    } else if hov {
        p.rect_stroke(
            rect,
            CornerRadius::same(rad),
            Stroke::new(1.5, Color32::from_white_alpha(110)),
            StrokeKind::Outside,
        );
    }
    let tip = if pick.is_some() {
        "Click to take it off"
    } else {
        "Click to pick (up to three; the first one counts most)"
    };
    resp.on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ---------------------------------------------------------------------------------------------
// page 3: game

fn game_icon(g: GameKind) -> &'static str {
    match g {
        GameKind::Themepark => icon::FLAG_BANNER,
        GameKind::Sandbox => icon::HAMMER,
        GameKind::RealmWar => icon::SWORD,
        GameKind::Survival => icon::CAMPFIRE,
        GameKind::Story => icon::BOOK_OPEN,
    }
}

fn page_game(ui: &mut Ui, c: &mut Cx) {
    let w = ui.available_width();
    let cols = if w >= 760.0 { 5 } else { 3 };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    for row in GameKind::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &g in row {
                let pic = c.wiz.textures.get(&format!("game_{}", game_id(g)));
                let (rect, resp) = ui.allocate_exact_size(
                    vec2(cw, if pic.is_some() { 196.0 } else { 170.0 }),
                    Sense::click(),
                );
                let sel = c.setup.plan.game == g;
                card_bg(ui, rect, sel, resp.hovered());
                let p = ui.painter();
                let ny = match pic {
                    Some(t) => {
                        let r = Rect::from_min_size(
                            rect.min + vec2(6.0, 6.0),
                            vec2(rect.width() - 12.0, 92.0),
                        );
                        paint_cover(ui, t, r, 10);
                        r.bottom() + 10.0
                    }
                    None => {
                        let ic = pos2(rect.left() + 36.0, rect.top() + 36.0);
                        p.circle_filled(
                            ic,
                            21.0,
                            if sel {
                                ACCENT
                            } else {
                                Color32::from_white_alpha(14)
                            },
                        );
                        p.text(
                            ic,
                            Align2::CENTER_CENTER,
                            game_icon(g),
                            FontId::proportional(21.0),
                            if sel { ON_ACCENT } else { Color32::WHITE },
                        );
                        rect.top() + 70.0
                    }
                };
                let name = p.layout(
                    g.name().to_string(),
                    FontId::new(14.5, semibold()),
                    Color32::WHITE,
                    rect.width() - 28.0,
                );
                let nh = name.size().y;
                p.galley(pos2(rect.left() + 14.0, ny), name, Color32::WHITE);
                let blurb = p.layout(
                    g.blurb().to_string(),
                    FontId::proportional(11.5),
                    TEXT_DIM,
                    rect.width() - 28.0,
                );
                p.galley(pos2(rect.left() + 14.0, ny + nh + 5.0), blurb, TEXT_DIM);
                if sel {
                    check_badge(p, pos2(rect.right() - 18.0, rect.top() + 18.0), icon::CHECK);
                }
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                    && !sel
                {
                    let plan = &mut c.setup.plan;
                    plan.game = g;
                    if g == GameKind::RealmWar && plan.realms < 2 {
                        plan.realms = 3;
                    }
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    ui.add_space(10.0);
    let plan = &mut c.setup.plan;
    ui.horizontal(|ui| {
        section_title(ui, "Levels");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "Characters go from level {} to {}",
                    plan.levels[0], plan.levels[1]
                ))
                .color(TEXT),
            );
        });
    });
    level_range(ui, &mut plan.levels);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.label(RichText::new("Quick picks").small().color(TEXT_DIM));
        for lv in [[1u32, 20u32], [1, 50], [1, 60], [1, 80], [1, 99]] {
            if chip(ui, "", &format!("{}–{}", lv[0], lv[1]), plan.levels == lv).clicked() {
                plan.levels = lv;
            }
        }
    });
    ui.add_space(14.0);
    if plan.game == GameKind::RealmWar || plan.realms > 1 {
        section_title(ui, "Realms");
        stepper_row(
            ui,
            icon::FLAG_BANNER,
            "Realms",
            "Each realm gets a homeland of its own; they meet on a contested frontier",
            &mut plan.realms,
            1,
            3,
        );
        ui.horizontal(|ui| {
            for i in 0..plan.realms as usize {
                let col = REALM_COLOURS[i % 3];
                let name = plan
                    .names
                    .realms
                    .get(i)
                    .filter(|s| !s.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Realm {}", i + 1));
                ui.label(RichText::new(format!("{}  {name}", icon::FLAG_PENNANT)).color(col));
                ui.add_space(10.0);
            }
        });
    } else if ghost(
        ui,
        &format!(
            "{}  Add realms: factions with homelands of their own",
            icon::PLUS
        ),
    )
    .clicked()
    {
        plan.realms = 2;
    }
}

const REALM_COLOURS: [Color32; 3] = [
    Color32::from_rgb(232, 96, 86),
    Color32::from_rgb(96, 150, 240),
    Color32::from_rgb(110, 200, 120),
];

// ---------------------------------------------------------------------------------------------
// page 4: world

/// Fantasy or Sci-Fi: two picture cards (the Sci-Fi world gets the alien template after it is
/// built, and its look is picked from the Sci-Fi looks on the Look page).
fn world_kind_cards(ui: &mut Ui, c: &mut Cx) {
    use crate::world_kind::WorldKind;
    section_title(ui, "World type");
    let w = ui.available_width();
    let cw = (w - GAP) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for k in WorldKind::ALL {
            let (rect, resp) = ui.allocate_exact_size(vec2(cw, 96.0), Sense::click());
            let sel = c.setup.plan.kind == k;
            let p = ui.painter_at(rect.expand(8.0));
            let (pic, ic) = match k {
                WorldKind::Fantasy => ("look_natural".to_string(), icon::CASTLE_TURRET),
                WorldKind::SciFi => (
                    crate::world_kind::looks::ScifiLook::AlienDusk.picture(),
                    icon::PLANET,
                ),
            };
            match c.wiz.textures.get(&pic) {
                Some(t) => paint_cover(ui, t, rect, 14),
                None => paint_landscape(&p, rect, REAL, 11 + k as u32, false),
            }
            scrim(&p, rect, 0.2, 230);
            p.text(
                pos2(rect.left() + 14.0, rect.bottom() - 30.0),
                Align2::LEFT_BOTTOM,
                format!("{ic}  {}", k.name()),
                FontId::new(16.0, semibold()),
                Color32::WHITE,
            );
            p.text(
                pos2(rect.left() + 14.0, rect.bottom() - 12.0),
                Align2::LEFT_BOTTOM,
                k.blurb(),
                FontId::proportional(11.5),
                Color32::from_rgb(210, 216, 226),
            );
            corner_mask(&p, rect, 14);
            pick_border(&p, rect, 14, sel, resp.hovered());
            if sel {
                check_badge(&p, pos2(rect.right() - 20.0, rect.top() + 20.0), icon::CHECK);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !sel {
                c.setup.plan.kind = k;
                // a Sci-Fi world opens at its look's hour
                if k == WorldKind::SciFi
                    && let Some(l) = crate::world_kind::looks::ScifiLook::from_id(&c.setup.plan.scifi_look)
                {
                    c.setup.plan.hour = l.hour();
                }
            }
        }
    });
    ui.add_space(14.0);
}

fn page_world(ui: &mut Ui, c: &mut Cx) {
    world_kind_cards(ui, c);
    section_title(ui, "In your own words");
    egui::Frame::new()
        .fill(Color32::from_rgb(12, 14, 18))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut c.setup.plan.description)
                    .frame(egui::Frame::NONE)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(14.0))
                    .hint_text("Describe your world… e.g. a cold northern continent with jungle islands in the south and a dwarven capital in the mountains"),
            );
        });
    ui.add_space(6.0);
    let ready = ai_ready(c);
    let ai = c.setup.ai.clone();
    ui.horizontal(|ui| {
        let can = ready && !ai.running && !c.setup.plan.description.trim().is_empty();
        let b = ui.add_enabled(
            can,
            egui::Button::new(
                RichText::new(format!("{}  Read my description", icon::MAGIC_WAND))
                    .color(ON_ACCENT)
                    .font(FontId::new(13.5, semibold())),
            )
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(9))
            .min_size(vec2(0.0, 32.0)),
        );
        let b = if !ready {
            b.on_disabled_hover_text("Connect an AI on the first page to have your words read")
        } else {
            b.on_disabled_hover_text("Write a few words about your world first")
                .on_hover_text("The AI reads your words into the cards below and makes up names")
        };
        if b.clicked() {
            c.setup.requests.push(SetupRequest::ReadDescription);
            c.setup.ai.running = true;
            c.setup.ai.status =
                format!("Reading your description with {}…", c.prompt.backend.name());
            c.setup.ai.error = None;
        }
        if ai.running {
            ui.add(egui::Spinner::new().size(16.0).color(ACCENT));
            ui.label(
                RichText::new(if ai.status.is_empty() {
                    "Reading…"
                } else {
                    ai.status.as_str()
                })
                .color(TEXT_DIM),
            );
        } else if !ready {
            ui.label(
                RichText::new(
                    "Connect an AI to have your words read, or just pick from the cards below.",
                )
                .small()
                .color(TEXT_DIM),
            );
            if ui
                .add(
                    egui::Button::new(RichText::new("Connect an AI").small().color(ACCENT_LIGHT))
                        .frame(false),
                )
                .clicked()
            {
                go(c, 0);
            }
        }
    });
    if let Some(e) = &ai.error {
        ui.add_space(4.0);
        info_card(ui, &format!("{}  {e}", icon::WARNING_CIRCLE), DANGER);
    } else if !ai.understood.is_empty() {
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(Color32::from_white_alpha(14))
            .stroke(Stroke::new(1.0, LINE_STRONG))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(format!("{}  What I understood", icon::SPARKLE))
                        .font(FontId::new(13.0, semibold()))
                        .color(ACCENT_LIGHT),
                );
                ui.label(RichText::new(&ai.understood).color(TEXT));
                if names_any(&c.setup.plan.names) {
                    ui.add_space(4.0);
                    names_chips(ui, &c.setup.plan.names);
                }
            });
    }
    ui.add_space(14.0);
    // the land's shape
    section_title(ui, "Shape of the land");
    let w = ui.available_width();
    let cols = if w >= 760.0 { 7 } else { 4 };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    let mh = (cw * 0.62).round();
    for row in Land::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &l in row {
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, mh + 26.0), Sense::click());
                let map = Rect::from_min_size(rect.min, vec2(cw, mh));
                let sel = c.setup.plan.land == l;
                let p = ui.painter_at(rect.expand(8.0));
                match c.wiz.textures.get(&format!("land_{}", l.preset())) {
                    Some(t) => paint_cover(ui, t, map, 10),
                    None => paint_land_map(&p, map, l, c.setup.plan.rivers),
                }
                corner_mask(&p, map, 10);
                if sel {
                    p.rect_stroke(
                        map,
                        CornerRadius::same(10),
                        Stroke::new(2.5, ACCENT),
                        StrokeKind::Outside,
                    );
                } else if resp.hovered() {
                    p.rect_stroke(
                        map,
                        CornerRadius::same(10),
                        Stroke::new(1.5, Color32::from_white_alpha(110)),
                        StrokeKind::Outside,
                    );
                }
                let col = if sel || resp.hovered() {
                    Color32::WHITE
                } else {
                    TEXT_DIM
                };
                p.text(
                    pos2(rect.center().x, map.bottom() + 13.0),
                    Align2::CENTER_CENTER,
                    l.name(),
                    FontId::new(
                        12.0,
                        if sel {
                            semibold()
                        } else {
                            egui::FontFamily::Proportional
                        },
                    ),
                    col,
                );
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    c.setup.plan.land = l;
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    ui.add_space(8.0);
    // size
    let plan = &mut c.setup.plan;
    section_title(ui, "Size");
    let mut size = plan.size;
    let opts: Vec<(WorldSize, &str)> = WorldSize::ALL.iter().map(|s| (*s, s.name())).collect();
    if crate::ui::widgets::segmented(ui, &mut size, &opts) {
        plan.size = size;
        plan.fit_to_size();
    }
    dim_line(
        ui,
        &format!(
            "{} km across: room for about {} towns, {} villages and {} dungeons. Builds in {}.",
            plan.size.km(),
            plan.towns,
            plan.villages,
            plan.dungeons,
            build_time(plan.size)
        ),
    );
    ui.add_space(12.0);
    // climate
    section_title(ui, "Climate");
    let w = ui.available_width();
    let cols = if w >= 640.0 { 5 } else { 3 };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    for row in Climate::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &cl in row {
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, 72.0), Sense::click());
                let sel = plan.climate == cl;
                let p = ui.painter_at(rect.expand(8.0));
                match c.wiz.textures.get(&climate_key(cl, plan.art)) {
                    Some(t) => paint_cover(ui, t, rect, 12),
                    None => paint_climate(&p, rect, cl),
                }
                scrim(&p, rect, 0.35, 210);
                p.text(
                    pos2(rect.left() + 11.0, rect.bottom() - 11.0),
                    Align2::LEFT_BOTTOM,
                    format!("{}  {}", climate_icon(cl), cl.name()),
                    FontId::new(12.5, semibold()),
                    Color32::WHITE,
                );
                corner_mask(&p, rect, 12);
                pick_border(&p, rect, 12, sel, resp.hovered());
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    plan.climate = cl;
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    ui.add_space(8.0);
    toggle_row(
        ui,
        icon::WAVES,
        "Rivers and lakes",
        "Rivers run from the mountains to the sea, with lakes on the way",
        &mut plan.rivers,
    );
}

fn climate_icon(c: Climate) -> &'static str {
    match c {
        Climate::Varied => icon::GLOBE_HEMISPHERE_WEST,
        Climate::Temperate => icon::TREE,
        Climate::Tropical => icon::TREE_PALM,
        Climate::Arid => icon::CACTUS,
        Climate::Cold => icon::SNOWFLAKE,
    }
}

fn build_time(s: WorldSize) -> &'static str {
    match s {
        WorldSize::Small => "about 10 seconds",
        WorldSize::Medium => "about 20 seconds",
        WorldSize::Large => "about a minute",
    }
}

// ---------------------------------------------------------------------------------------------
// page 5: towns & names

fn page_towns(ui: &mut Ui, c: &mut Cx) {
    let plan = &mut c.setup.plan;
    let cap = match plan.size {
        WorldSize::Small => 1,
        WorldSize::Medium => 2,
        WorldSize::Large => 4,
    };
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        section_title(ui, "Places");
        toggle_row(
            ui,
            icon::CROWN,
            "A capital city",
            "The biggest city, at the heart of the land",
            &mut plan.capital,
        );
        stepper_row(
            ui,
            icon::BUILDINGS,
            "Towns",
            "Walled towns with a market and a quest hub",
            &mut plan.towns,
            0,
            16,
        );
        stepper_row(
            ui,
            icon::HOUSE_LINE,
            "Villages",
            "Small hamlets out in the countryside",
            &mut plan.villages,
            0,
            24,
        );
        if plan.towns > 4 * cap || plan.villages > 6 * cap {
            ui.label(
                RichText::new(format!(
                    "{}  A {} world holds up to {} towns and {} villages; the rest are left out.",
                    icon::INFO,
                    size_word(plan.size),
                    4 * cap,
                    6 * cap
                ))
                .small()
                .color(AMBER),
            );
        }
        toggle_row(
            ui,
            icon::PATH,
            "Roads",
            "Roads join the settlements up",
            &mut plan.roads,
        );
        toggle_row(
            ui,
            icon::GRAINS,
            "Farm fields",
            "Patchwork fields round the villages",
            &mut plan.farms,
        );
        toggle_row(
            ui,
            icon::MAP_TRIFOLD,
            "Zones",
            "Zones with names and level ranges: you can change them later",
            &mut plan.zones,
        );

        let ui = &mut cols[1];
        section_title(ui, "Names");
        dim_line(ui, "Leave a name empty and one is made up from the land.");
        ui.add_space(2.0);
        let names = &mut plan.names;
        if plan.capital {
            name_field(ui, "Capital", &mut names.capital, "made up for you");
        }
        let n = (plan.towns + plan.villages).min(40) as usize;
        for i in 0..n {
            let label = if i < plan.towns as usize {
                format!("Town {}", i + 1)
            } else {
                format!("Village {}", i + 1 - plan.towns as usize)
            };
            list_field(ui, &label, &mut names.towns, i);
        }
        if plan.realms > 1 {
            ui.add_space(6.0);
            for i in 0..plan.realms as usize {
                list_field(ui, &format!("Realm {}", i + 1), &mut names.realms, i);
            }
        }
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new("More names: zones and dungeons").color(TEXT))
            .id_salt("setup_more_names")
            .show(ui, |ui| {
                let z = (names.zones.len() + 1).clamp(3, 24);
                for i in 0..z {
                    list_field(ui, &format!("Zone {}", i + 1), &mut names.zones, i);
                }
                let d = ((plan.dungeons + plan.raids) as usize)
                    .max(names.dungeons.len())
                    .min(32);
                for i in 0..d {
                    list_field(ui, &format!("Dungeon {}", i + 1), &mut names.dungeons, i);
                }
            });
    });
}

fn size_word(s: WorldSize) -> &'static str {
    match s {
        WorldSize::Small => "small",
        WorldSize::Medium => "medium",
        WorldSize::Large => "large",
    }
}

fn name_field(ui: &mut Ui, label: &str, v: &mut String, hint: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(70.0, 24.0),
            egui::Label::new(RichText::new(label).color(TEXT_DIM)),
        );
        ui.add(
            egui::TextEdit::singleline(v)
                .hint_text(hint)
                .desired_width(ui.available_width()),
        );
    });
}

/// One entry of a list of names (the list only grows when the user types).
fn list_field(ui: &mut Ui, label: &str, list: &mut Vec<String>, i: usize) {
    let mut v = list.get(i).cloned().unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(70.0, 24.0),
            egui::Label::new(RichText::new(label).color(TEXT_DIM)),
        );
        if ui
            .add(
                egui::TextEdit::singleline(&mut v)
                    .hint_text("made up for you")
                    .desired_width(ui.available_width()),
            )
            .changed()
        {
            if list.len() <= i {
                list.resize(i + 1, String::new());
            }
            list[i] = v;
        }
    });
}

// ---------------------------------------------------------------------------------------------
// page 6: adventure

fn page_adventure(ui: &mut Ui, c: &mut Cx) {
    let plan = &mut c.setup.plan;
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        section_title(ui, "Dungeons and caves");
        stepper_row(
            ui,
            icon::SKULL,
            "Dungeons",
            "Underground dungeons for a party",
            &mut plan.dungeons,
            0,
            24,
        );
        stepper_row(
            ui,
            icon::CROWN_SIMPLE,
            "Raids",
            "Big raid dungeons with a boss at the end",
            &mut plan.raids,
            0,
            4,
        );
        stepper_row(
            ui,
            icon::MOUNTAINS,
            "Caves",
            "Caves in the hills to explore",
            &mut plan.caves,
            0,
            20,
        );
        toggle_row(
            ui,
            icon::TENT,
            "Monster camps",
            "Ready-made encounter sites out in the wilds",
            &mut plan.encounters,
        );
        toggle_row(
            ui,
            icon::PAW_PRINT,
            "Creatures",
            "Creatures roam every zone, tougher further out",
            &mut plan.creatures,
        );

        let ui = &mut cols[1];
        section_title(ui, "Gathering");
        dim_line(ui, "What players can gather out in the world.");
        ui.add_space(2.0);
        let g = &mut plan.gathering;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for (ic, label, v) in [
                (icon::LEAF, "Herbs", &mut g.herbs),
                (icon::TREE, "Wood", &mut g.wood),
                (icon::CUBE, "Stone", &mut g.stone),
                (icon::DIAMOND, "Ore", &mut g.ore),
                (icon::FISH, "Fish", &mut g.fish),
            ] {
                if chip(ui, ic, label, *v).clicked() {
                    *v = !*v;
                }
            }
        });
        ui.add_space(8.0);
        let any = g.herbs || g.wood || g.stone || g.ore || g.fish;
        ui.add_enabled_ui(any, |ui| {
            crate::ui::widgets::slider_row(ui, "How much", &mut g.density, 0.5..=2.0, |d| {
                if d < 0.8 {
                    "Sparse".into()
                } else if d < 1.3 {
                    "Normal".into()
                } else {
                    "Plenty".into()
                }
            });
        });
    });
    ui.add_space(12.0);
    section_title(ui, "Dungeon kinds");
    dim_line(
        ui,
        "Pick the kinds you'd like, or leave it to the land: a frozen dungeon in the snow, a temple in the jungle.",
    );
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if chip(
            ui,
            icon::GLOBE_HEMISPHERE_WEST,
            "Chosen by the land",
            plan.dungeon_layouts.is_empty(),
        )
        .clicked()
        {
            plan.dungeon_layouts.clear();
        }
        for d in dpresets::all().iter().filter(|d| d.id != "empty") {
            let on = plan.dungeon_layouts.iter().any(|x| x == d.id);
            if chip(ui, d.icon, d.short(), on)
                .on_hover_text(d.blurb)
                .clicked()
            {
                if on {
                    plan.dungeon_layouts.retain(|x| x != d.id);
                } else {
                    plan.dungeon_layouts.push(d.id.to_string());
                }
            }
        }
    });
    ui.add_space(8.0);
    egui::CollapsingHeader::new(RichText::new("Dungeon rooms").color(TEXT))
        .id_salt("setup_rooms")
        .show(ui, |ui| {
            let mut rs = RoomSize::ALL
                .into_iter()
                .find(|r| r.id() == plan.dungeon_rooms)
                .unwrap_or_default();
            ui.label(RichText::new("Room size").color(TEXT));
            let opts: Vec<(RoomSize, &str)> =
                RoomSize::ALL.iter().map(|r| (*r, r.name())).collect();
            if crate::ui::widgets::segmented(ui, &mut rs, &opts) {
                plan.dungeon_rooms = rs.id().to_string();
            }
            ui.add_space(4.0);
            crate::ui::widgets::slider_row(
                ui,
                "Irregular rooms",
                &mut plan.dungeon_irregularity,
                0.0..=1.0,
                |x| {
                    if x < 0.01 {
                        "Off".into()
                    } else {
                        format!("{:.0}%", x * 100.0)
                    }
                },
            )
            .on_hover_text("Uneven walls, alcoves and jogs: a little, or every room its own shape");
        });
}

// ---------------------------------------------------------------------------------------------
// page 7: look

fn look_id(p: LookPreset) -> String {
    snake(&format!("{p:?}"))
}

fn look_of(id: &str) -> LookPreset {
    LookPreset::ALL
        .into_iter()
        .find(|p| look_id(*p) == id)
        .unwrap_or_default()
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

const PAINTED: [[u8; 3]; 4] = [
    [62, 140, 58],
    [90, 170, 180],
    [96, 160, 236],
    [255, 238, 186],
];
const REAL: [[u8; 3]; 4] = [
    [70, 84, 58],
    [118, 124, 112],
    [146, 170, 196],
    [226, 224, 214],
];

fn page_look(ui: &mut Ui, c: &mut Cx) {
    let plan = &mut c.setup.plan;
    section_title(ui, "Art style");
    let w = ui.available_width();
    let cw = (w - GAP) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for a in Art::ALL {
            let (rect, resp) = ui.allocate_exact_size(vec2(cw, 112.0), Sense::click());
            let sel = plan.art == a;
            let p = ui.painter_at(rect.expand(8.0));
            let (pal, painted, blurb, ic) = match a {
                Art::HandPainted => (
                    PAINTED,
                    true,
                    "Bold colour and painted textures, like classic fantasy MMOs",
                    icon::PAINT_BRUSH,
                ),
                Art::Photoreal => (
                    REAL,
                    false,
                    "Photoscanned ground, rocks and plants under natural light",
                    icon::CAMERA,
                ),
            };
            match c.wiz.textures.get(&format!("art_{}", a.id())) {
                Some(t) => paint_cover(ui, t, rect, 14),
                None => paint_landscape(&p, rect, pal, 7 + a as u32, painted),
            }
            scrim(&p, rect, 0.2, 230);
            p.text(
                pos2(rect.left() + 14.0, rect.bottom() - 30.0),
                Align2::LEFT_BOTTOM,
                format!("{ic}  {}", a.name()),
                FontId::new(16.0, semibold()),
                Color32::WHITE,
            );
            p.text(
                pos2(rect.left() + 14.0, rect.bottom() - 12.0),
                Align2::LEFT_BOTTOM,
                blurb,
                FontId::proportional(11.5),
                Color32::from_rgb(210, 216, 226),
            );
            corner_mask(&p, rect, 14);
            pick_border(&p, rect, 14, sel, resp.hovered());
            if sel {
                check_badge(
                    &p,
                    pos2(rect.right() - 20.0, rect.top() + 20.0),
                    icon::CHECK,
                );
            }
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                plan.art = a;
            }
        }
    });
    ui.add_space(14.0);
    if plan.kind == crate::world_kind::WorldKind::SciFi {
        scifi_look_grid(ui, c);
        time_of_day(ui, &mut c.setup.plan);
        return;
    }
    section_title(ui, "Colour and light");
    let cur = look_of(&plan.look);
    let w = ui.available_width();
    let cols = if w >= 800.0 {
        6
    } else if w >= 560.0 {
        4
    } else {
        3
    };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    for row in LookPreset::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &lp in row {
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, 82.0), Sense::click());
                let sel = cur == lp;
                let p = ui.painter_at(rect.expand(8.0));
                match c.wiz.textures.get(&format!("look_{}", look_id(lp))) {
                    Some(t) => paint_cover(ui, t, rect, 11),
                    None => paint_landscape(
                        &p,
                        rect,
                        lp.palette(),
                        31 + lp as u32 * 7,
                        plan.art == Art::HandPainted,
                    ),
                }
                scrim(&p, rect, 0.4, 220);
                p.text(
                    pos2(rect.left() + 10.0, rect.bottom() - 9.0),
                    Align2::LEFT_BOTTOM,
                    lp.name(),
                    FontId::new(12.0, semibold()),
                    Color32::WHITE,
                );
                corner_mask(&p, rect, 11);
                pick_border(&p, rect, 11, sel, resp.hovered());
                let tip = if lp.inspired_by().starts_with("like") {
                    format!("{}\n({})", lp.blurb(), lp.inspired_by())
                } else {
                    lp.blurb().to_string()
                };
                if resp
                    .on_hover_text(tip)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    plan.look = look_id(lp);
                    if let Some((hour, ..)) = lp.scene() {
                        plan.hour = hour;
                    }
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    dim_line(ui, cur.blurb());
    time_of_day(ui, plan);
}

/// The Sci-Fi looks (`world_kind::looks`): the same cards as Style › Look › Sci-Fi.
fn scifi_look_grid(ui: &mut Ui, c: &mut Cx) {
    use crate::world_kind::looks::ScifiLook;
    section_title(ui, "Alien sky, light and plants");
    let plan = &mut c.setup.plan;
    let cur = ScifiLook::from_id(&plan.scifi_look).unwrap_or(ScifiLook::AlienDusk);
    let w = ui.available_width();
    let cols = if w >= 800.0 {
        5
    } else if w >= 560.0 {
        4
    } else {
        3
    };
    let cw = (w - GAP * (cols - 1) as f32) / cols as f32;
    for row in ScifiLook::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for &l in row {
                let (rect, resp) = ui.allocate_exact_size(vec2(cw, 82.0), Sense::click());
                let sel = cur == l;
                let p = ui.painter_at(rect.expand(8.0));
                match c.wiz.textures.get(&l.picture()) {
                    Some(t) => paint_cover(ui, t, rect, 11),
                    None => paint_landscape(&p, rect, REAL, 53 + l as u32 * 7, false),
                }
                scrim(&p, rect, 0.4, 220);
                p.text(
                    pos2(rect.left() + 10.0, rect.bottom() - 9.0),
                    Align2::LEFT_BOTTOM,
                    l.name(),
                    FontId::new(12.0, semibold()),
                    Color32::WHITE,
                );
                corner_mask(&p, rect, 11);
                pick_border(&p, rect, 11, sel, resp.hovered());
                if resp
                    .on_hover_text(l.blurb())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    plan.scifi_look = l.id().to_string();
                    plan.hour = l.hour();
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }
    dim_line(
        ui,
        &format!(
            "{}: {}. Sets the sky, the air and the plants; the land gets red rock ground, rock spires and floating rocks.",
            cur.name(),
            cur.blurb()
        ),
    );
}

fn time_of_day(ui: &mut Ui, plan: &mut WorldPlan) {
    ui.add_space(12.0);
    section_title(ui, "Time of day");
    crate::ui::widgets::slider_row(
        ui,
        &format!(
            "{}  The world opens at",
            if (6.0..19.5).contains(&plan.hour) {
                icon::SUN
            } else {
                icon::MOON
            }
        ),
        &mut plan.hour,
        0.0..=24.0,
        |h| {
            let hh = h.floor() as u32 % 24;
            let mm = ((h.fract() * 60.0) as u32 / 5) * 5;
            format!("{hh:02}:{mm:02} · {}", day_part(h))
        },
    );
}

fn day_part(h: f32) -> &'static str {
    match h {
        h if h < 5.0 => "night",
        h if h < 8.0 => "dawn",
        h if h < 11.5 => "morning",
        h if h < 14.0 => "midday",
        h if h < 17.5 => "afternoon",
        h if h < 20.0 => "evening",
        _ => "night",
    }
}

// ---------------------------------------------------------------------------------------------
// page 8: review

fn page_review(ui: &mut Ui, c: &mut Cx) {
    let plan = &mut c.setup.plan;
    section_title(ui, "World name");
    let hint = if plan.names.world.trim().is_empty() {
        "New World".to_string()
    } else {
        plan.names.world.clone()
    };
    egui::Frame::new()
        .fill(Color32::from_rgb(12, 14, 18))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let font = FontId::new(20.0, semibold());
                ui.add(
                    egui::TextEdit::singleline(&mut plan.name)
                        .frame(egui::Frame::NONE)
                        .font(font.clone())
                        .hint_text(RichText::new(hint).font(font).color(TEXT_MUTE))
                        .desired_width(ui.available_width() - 36.0),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if crate::ui::widgets::icon_button(ui, "Dice5", "Another name").clicked() {
                        roll_name(plan);
                    }
                });
            });
        });
    ui.add_space(12.0);
    let lines = plan.summary();
    let plan_view = plan.clone();
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        section_title(ui, "Your world");
        for (i, l) in lines.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(summary_icon(i, l))
                        .size(15.0)
                        .color(ACCENT_LIGHT),
                );
                ui.add(egui::Label::new(RichText::new(capitalised(l)).color(TEXT)).wrap());
            });
        }
        let ui = &mut cols[1];
        // a postcard of it
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 168.0), Sense::hover());
        let p = ui.painter_at(rect.expand(8.0));
        let lp = look_of(&plan_view.look);
        match c
            .wiz
            .textures
            .get(&climate_key(plan_view.climate, plan_view.art))
        {
            Some(t) => paint_cover(ui, t, rect, 14),
            None => paint_landscape(
                &p,
                rect,
                lp.palette(),
                seed_of(&plan_view.name) ^ 0x5a5a,
                plan_view.art == Art::HandPainted,
            ),
        }
        scrim(&p, rect, 0.3, 230);
        let shown = if plan_view.name.trim().is_empty() {
            if plan_view.names.world.trim().is_empty() {
                "New World".to_string()
            } else {
                plan_view.names.world.clone()
            }
        } else {
            plan_view.name.clone()
        };
        p.text(
            pos2(rect.left() + 16.0, rect.bottom() - 32.0),
            Align2::LEFT_BOTTOM,
            &shown,
            FontId::new(22.0, semibold()),
            Color32::WHITE,
        );
        let by: Vec<&str> = plan_view
            .inspirations
            .iter()
            .filter_map(|id| inspiration(id).map(|i| i.name))
            .collect();
        let sub = if by.is_empty() {
            format!("{} · {}", lp.name(), plan_view.art.name())
        } else {
            format!("Inspired by {}", by.join(", "))
        };
        p.text(
            pos2(rect.left() + 16.0, rect.bottom() - 13.0),
            Align2::LEFT_BOTTOM,
            sub,
            FontId::proportional(12.0),
            Color32::from_rgb(214, 220, 230),
        );
        corner_mask(&p, rect, 14);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::TIMER).size(15.0).color(ACCENT_LIGHT));
            ui.label(
                RichText::new(format!("Builds in {}", build_time(plan_view.size))).color(TEXT),
            );
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::PALETTE).size(15.0).color(ACCENT_LIGHT));
            ui.label(
                RichText::new(format!(
                    "{} look, opens at {:02}:{:02}",
                    if plan_view.kind == crate::world_kind::WorldKind::SciFi {
                        crate::world_kind::looks::ScifiLook::from_id(&plan_view.scifi_look)
                            .map(|l| l.name())
                            .unwrap_or("Alien Dusk")
                    } else {
                        lp.name()
                    },
                    plan_view.hour.floor() as u32 % 24,
                    ((plan_view.hour.fract() * 60.0) as u32 / 5) * 5
                ))
                .color(TEXT),
            );
        });
        if names_any(&plan_view.names) {
            ui.add_space(4.0);
            names_chips(ui, &plan_view.names);
        }
    });
    ui.add_space(12.0);
    if c.in_dungeon {
        info_card(
            ui,
            &format!(
                "{}  Go back to the world first: the build can't start while a dungeon is open.",
                icon::WARNING
            ),
            AMBER,
        );
    } else if c.dirty {
        info_card(
            ui,
            &format!(
                "{}  Your open world has unsaved changes: you'll be asked to save it before the new world is built.",
                icon::INFO
            ),
            AMBER,
        );
    } else {
        dim_line(
            ui,
            &format!(
                "{}  Your open world is saved. Building makes a new world (saved in worlds/), and the old one stays under File › Open.",
                icon::CHECK_CIRCLE
            ),
        );
    }
}

/// A fresh made-up name for the world: never the one it has, nor one a world on disk already has.
fn roll_name(plan: &mut WorldPlan) {
    let cur = plan.name.trim().to_lowercase();
    let dir = crate::io::worlds_dir();
    plan.name = super::plan::random_world_name(|n| {
        n.to_lowercase() == cur || dir.join(format!("{n}.world")).exists()
    });
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn summary_icon(i: usize, l: &str) -> &'static str {
    let l = l.to_lowercase();
    if i == 0 {
        icon::GLOBE_HEMISPHERE_WEST
    } else if i == 1 {
        icon::GAME_CONTROLLER
    } else if l.contains("realm") {
        icon::FLAG_BANNER
    } else if l.contains("dungeon")
        || l.contains("raid")
        || l.contains("cave")
        || l.contains("camp")
    {
        icon::SKULL
    } else if l.contains("creature") {
        icon::PAW_PRINT
    } else if l.contains("zone") {
        icon::MAP_TRIFOLD
    } else if l.contains("gathering") {
        icon::LEAF
    } else {
        icon::HOUSE_LINE
    }
}

// ---------------------------------------------------------------------------------------------
// page 9: building

fn shown_build(c: &Cx) -> Option<BuildState> {
    match c.wiz.demo.as_deref() {
        Some(d @ ("building" | "done" | "failed")) => Some(demo_build(d, &c.setup.plan)),
        _ => c.setup.build.clone(),
    }
}

fn fmt_secs(s: f32) -> String {
    let s = s.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

fn page_build(ui: &mut Ui, c: &mut Cx) {
    let Some(b) = shown_build(c) else {
        let asked = c.wiz.build_asked_at.is_some_and(|t| c.now - t < 10.0);
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            if asked {
                ui.add(egui::Spinner::new().size(28.0).color(ACCENT));
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Getting ready…")
                        .font(FontId::new(18.0, semibold()))
                        .color(Color32::WHITE),
                );
            } else {
                ui.label(
                    RichText::new(icon::ROCKET_LAUNCH)
                        .size(40.0)
                        .color(TEXT_DIM),
                );
                ui.label(
                    RichText::new("Nothing is building yet")
                        .font(FontId::new(18.0, semibold()))
                        .color(Color32::WHITE),
                );
                ui.label(RichText::new("Build your world from the review page.").color(TEXT_DIM));
            }
        });
        return;
    };
    let name = if b.plan.name.trim().is_empty() {
        "your world".to_string()
    } else {
        b.plan.name.clone()
    };
    let (title, col) = if let Some(_e) = &b.failed {
        ("Something went wrong".to_string(), DANGER)
    } else if b.cancelled {
        ("Stopped".to_string(), AMBER)
    } else if b.done {
        (
            format!("{name} is ready  {}", icon::SPARKLE),
            Color32::WHITE,
        )
    } else {
        (format!("Building {name}…"), Color32::WHITE)
    };
    ui.label(
        RichText::new(title)
            .font(FontId::new(23.0, semibold()))
            .color(col),
    );
    let stage = b
        .stages
        .get(b.current)
        .map(|s| s.0.clone())
        .unwrap_or_default();
    let sub = if let Some(e) = &b.failed {
        e.clone()
    } else if b.cancelled {
        "What was made so far stays in the world.".to_string()
    } else if b.done {
        "Here's what the checks found. Fly through it, walk it, or start shaping it yourself."
            .to_string()
    } else {
        format!("{stage}…")
    };
    ui.label(RichText::new(sub).color(if b.failed.is_some() { DANGER } else { TEXT_DIM }));
    ui.add_space(10.0);
    if b.done {
        // done: how long it took and where it's saved, instead of the bar
        ui.horizontal(|ui| {
            tag(
                ui,
                icon::CHECK_CIRCLE,
                &format!("Built in {}", fmt_secs(b.elapsed)),
            );
            if let Some(file) = c.world_path.as_ref().and_then(|p| p.file_name()) {
                tag(
                    ui,
                    icon::FLOPPY_DISK,
                    &format!("Saved as worlds/{}", file.to_string_lossy()),
                );
            }
            tag(ui, icon::STACK, &format!("{} stages done", b.stages.len()));
        });
        ui.add_space(10.0);
        ui.columns(2, |cols| {
            report_and_places(&mut cols[0], c, &b);
            log_box(&mut cols[1], &b, 9);
        });
        return;
    }
    // the overall bar
    let pr = b.progress().clamp(0.0, 1.0);
    let w = ui.available_width();
    let (bar, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    progress(
        ui,
        bar,
        pr,
        if b.failed.is_some() {
            DANGER
        } else if b.cancelled {
            AMBER
        } else {
            ACCENT
        },
        !b.done && b.failed.is_none() && !b.cancelled,
    );
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{:.0}%", pr * 100.0))
                .font(FontId::new(13.0, semibold()))
                .color(Color32::WHITE),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{}  {}", icon::TIMER, fmt_secs(b.elapsed))).color(TEXT_DIM),
            );
        });
    });
    ui.add_space(8.0);
    ui.columns(2, |cols| {
        stages_list(&mut cols[0], &b);
        log_box(&mut cols[1], &b, 11);
    });
}

fn stages_list(ui: &mut Ui, b: &BuildState) {
    section_title(ui, "Stages");
    let row = 25.0;
    for (i, (sname, f)) in b.stages.iter().enumerate() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row), Sense::hover());
        let p = ui.painter();
        let done = *f >= 0.999;
        let now = i == b.current && !b.done && b.failed.is_none() && !b.cancelled;
        let ic = pos2(rect.left() + 9.0, rect.center().y);
        if done {
            p.circle_filled(ic, 8.0, GREEN);
            p.text(
                ic,
                Align2::CENTER_CENTER,
                icon::CHECK,
                FontId::proportional(10.0),
                Color32::from_rgb(10, 30, 16),
            );
        } else if now {
            let t = ui.input(|i| i.time) as f32;
            p.circle_stroke(ic, 7.0, Stroke::new(2.0, Color32::from_white_alpha(30)));
            let a0 = t * 5.0;
            let pts: Vec<Pos2> = (0..=12)
                .map(|k| a0 + k as f32 * 0.35)
                .map(|a| ic + vec2(a.cos(), a.sin()) * 7.0)
                .collect();
            p.add(Shape::line(pts, Stroke::new(2.0, ACCENT)));
            ui.ctx().request_repaint();
        } else {
            p.circle_stroke(ic, 6.5, Stroke::new(1.5, Color32::from_rgb(80, 84, 90)));
        }
        let tc = if done || now {
            Color32::WHITE
        } else {
            TEXT_DIM
        };
        p.text(
            pos2(rect.left() + 26.0, rect.center().y),
            Align2::LEFT_CENTER,
            sname,
            FontId::new(
                13.0,
                if now {
                    semibold()
                } else {
                    egui::FontFamily::Proportional
                },
            ),
            tc,
        );
        if now || (!done && *f > 0.0) {
            let br = Rect::from_min_size(
                pos2(rect.right() - 90.0, rect.center().y - 3.0),
                vec2(90.0, 6.0),
            );
            progress(ui, br, *f, ACCENT, false);
        }
    }
    ui.add_space(8.0);
}

fn log_box(ui: &mut Ui, b: &BuildState, n: usize) {
    section_title(ui, if b.done { "Made" } else { "Made so far" });
    egui::Frame::new()
        .fill(Color32::from_rgb(12, 14, 18))
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let start = b.log.len().saturating_sub(n);
            if b.log.is_empty() {
                ui.label(RichText::new("…").color(TEXT_DIM));
            }
            for (k, l) in b.log[start..].iter().enumerate() {
                let last = start + k + 1 == b.log.len();
                ui.label(
                    RichText::new(l)
                        .font(FontId::proportional(12.0))
                        .color(if last { TEXT } else { TEXT_DIM }),
                );
            }
        });
}

const DEMO_PLACES: [(&str, &str); 12] = [
    ("Khazad Orn", "city"),
    ("Frostgate", "town"),
    ("Emberhollow", "town"),
    ("Saltmere", "town"),
    ("Brindle", "village"),
    ("Oakhurst", "village"),
    ("The Sunken Forge", "dungeon"),
    ("Spirevault", "dungeon"),
    ("The Drowned Throne", "raid"),
    ("Mossjaw Cave", "cave"),
    ("Ironbark Camp", "camp"),
    ("Redfang Camp", "camp"),
];

fn place_icon(kind: &str) -> &'static str {
    match kind {
        "city" => icon::CROWN,
        "town" => icon::BUILDINGS,
        "village" => icon::HOUSE_LINE,
        "dungeon" => icon::SKULL,
        "raid" => icon::CROWN_SIMPLE,
        "cave" => icon::MOUNTAINS,
        "camp" => icon::TENT,
        _ => icon::MAP_PIN,
    }
}

/// The done page's left side: what the checks found, then the places, each with "Show".
fn report_and_places(ui: &mut Ui, c: &mut Cx, b: &BuildState) {
    if !b.report.is_empty() {
        section_title(ui, "What the checks found");
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(74, 210, 120, 16))
            .stroke(Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(74, 210, 120, 70),
            ))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for l in &b.report {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::CHECK_CIRCLE).color(GREEN));
                        ui.add(egui::Label::new(RichText::new(l).color(TEXT)).wrap());
                    });
                }
            });
        ui.add_space(10.0);
    }
    let places = std::mem::take(&mut c.places);
    if places.is_empty() && !c.has_start {
        return;
    }
    section_title(ui, "Places");
    ui.spacing_mut().item_spacing.y = 2.0;
    let total = places.len() + usize::from(c.has_start);
    let cap = if c.wiz.places_all { usize::MAX } else { 8 };
    let mut rows = 0;
    let mut goto: Option<String> = None;
    if c.has_start && place_row(ui, icon::FLAG, "Where players start", "") {
        goto = Some("start".into());
    }
    rows += usize::from(c.has_start);
    let groups: [(&str, &[&str]); 3] = [
        ("Settlements", &["city", "capital", "town", "village"]),
        ("Dungeons & raids", &["dungeon", "raid"]),
        ("Caves & camps", &["cave", "camp"]),
    ];
    let known: Vec<&str> = groups.iter().flat_map(|g| g.1.iter().copied()).collect();
    let others: Vec<&(String, &str)> = places.iter().filter(|p| !known.contains(&p.1)).collect();
    for (title, kinds) in groups {
        let list: Vec<&(String, &str)> = places.iter().filter(|p| kinds.contains(&p.1)).collect();
        if list.is_empty() || rows >= cap {
            continue;
        }
        ui.add_space(4.0);
        ui.label(RichText::new(title).size(11.5).color(TEXT_DIM));
        for (name, kind) in list {
            if rows >= cap {
                break;
            }
            if place_row(ui, place_icon(kind), name, kind) {
                goto = Some(name.clone());
            }
            rows += 1;
        }
    }
    if !others.is_empty() && rows < cap {
        ui.add_space(2.0);
        ui.label(RichText::new("More").size(11.5).color(TEXT_DIM));
        for (name, kind) in others {
            if rows >= cap {
                break;
            }
            if place_row(ui, place_icon(kind), name, kind) {
                goto = Some(name.clone());
            }
            rows += 1;
        }
    }
    if total > 8 {
        let label = if c.wiz.places_all {
            "Show fewer".to_string()
        } else {
            format!("Show all {total} places")
        };
        if ui
            .add(egui::Button::new(RichText::new(label).small().color(ACCENT_LIGHT)).frame(false))
            .clicked()
        {
            c.wiz.places_all = !c.wiz.places_all;
        }
    }
    if let Some(g) = goto {
        c.setup.requests.push(SetupRequest::GoTo(g));
        c.setup.open = false;
        c.wiz.peek = true;
    }
    c.places = places;
}

/// One place: its icon, name and kind, and a "Show" link (true when clicked).
fn place_row(ui: &mut Ui, ic: &str, name: &str, kind: &str) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    let hov = resp.hovered();
    let p = ui.painter();
    if hov {
        p.rect_filled(rect, CornerRadius::same(6), Color32::from_white_alpha(10));
    }
    p.text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::CENTER_CENTER,
        ic,
        FontId::proportional(14.0),
        ACCENT_LIGHT,
    );
    let n = p.text(
        pos2(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(13.0),
        Color32::WHITE,
    );
    if !kind.is_empty() {
        p.text(
            pos2(n.right() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            kind,
            FontId::proportional(11.5),
            TEXT_DIM,
        );
    }
    p.text(
        pos2(rect.right() - 6.0, rect.center().y),
        Align2::RIGHT_CENTER,
        format!("Show  {}", icon::ARROW_RIGHT),
        FontId::proportional(12.0),
        if hov { Color32::WHITE } else { ACCENT_LIGHT },
    );
    resp.on_hover_text("Fly the camera there")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn build_footer(ui: &mut Ui, c: &mut Cx) {
    let Some(b) = shown_build(c) else {
        if secondary(ui, &format!("{}  Back to review", icon::ARROW_LEFT)).clicked() {
            go(c, REVIEW);
        }
        return;
    };
    if b.failed.is_some() || b.cancelled {
        if secondary(ui, &format!("{}  Back to review", icon::ARROW_LEFT)).clicked() {
            c.wiz.demo = None;
            go(c, REVIEW);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary(ui, "Close").clicked() {
                c.setup.open = false;
            }
        });
    } else if b.done {
        if secondary(ui, &format!("{}  Fly through it", icon::AIRPLANE_TILT))
            .on_hover_text("A camera flight over the new world")
            .clicked()
        {
            c.setup.requests.push(SetupRequest::Flythrough);
            c.setup.open = false;
            c.wiz.peek = true;
        }
        if secondary(ui, &format!("{}  Walk in", icon::PERSON_SIMPLE_WALK))
            .on_hover_text("Walk into the world where players start")
            .clicked()
        {
            c.setup.requests.push(SetupRequest::Walk);
            c.setup.open = false;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary(ui, &format!("Start exploring  {}", icon::ARROW_RIGHT)).clicked() {
                c.setup.open = false;
                c.wiz.peek = false;
            }
        });
    } else {
        let cancel = ui.add(
            egui::Button::new(RichText::new(format!("{}  Cancel", icon::STOP)).color(DANGER))
                .fill(Color32::from_rgba_unmultiplied(239, 88, 80, 18))
                .stroke(Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(239, 88, 80, 120),
                ))
                .corner_radius(CornerRadius::same(10))
                .min_size(vec2(100.0, 38.0)),
        );
        if cancel
            .on_hover_text("Stop building: what's made so far stays")
            .clicked()
        {
            c.setup.requests.push(SetupRequest::Cancel);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if secondary(ui, "Hide — keep building")
                .on_hover_text("Close this; a small bar shows the progress")
                .clicked()
            {
                c.setup.open = false;
                // (the bar stays once it's done, with the way back to the summary)
                c.wiz.peek = true;
            }
        });
    }
}

enum Pill {
    Nothing,
    Open,
    Dismiss,
}

/// With the wizard closed: a build's progress at the bottom of the screen (or, once it's done and
/// the user went to look at a place, a way back to the summary).
fn building_pill(ctx: &egui::Context, b: &BuildState) -> Pill {
    let pr = b.progress();
    let stage = b
        .stages
        .get(b.current)
        .map(|s| s.0.clone())
        .unwrap_or_default();
    let name = if b.plan.name.trim().is_empty() {
        "your world".to_string()
    } else {
        b.plan.name.clone()
    };
    let mut out = Pill::Nothing;
    egui::Area::new(Id::new("setup_building_pill"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -64.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(20, 23, 29))
                .stroke(Stroke::new(1.0, LINE_STRONG))
                .corner_radius(CornerRadius::same(22))
                .inner_margin(Margin::symmetric(16, 10))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(120),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if b.done {
                            ui.label(RichText::new(icon::CHECK_CIRCLE).size(16.0).color(GREEN));
                            ui.label(
                                RichText::new(format!("{name} is ready"))
                                    .font(FontId::new(13.0, semibold()))
                                    .color(Color32::WHITE),
                            );
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Places and checks")
                                            .small()
                                            .color(ACCENT_LIGHT),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                out = Pill::Open;
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(icon::X).size(13.0).color(TEXT_DIM),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Hide")
                                .clicked()
                            {
                                out = Pill::Dismiss;
                            }
                            return;
                        }
                        ui.add(egui::Spinner::new().size(15.0).color(ACCENT));
                        ui.label(
                            RichText::new(format!("Building {name}"))
                                .font(FontId::new(13.0, semibold()))
                                .color(Color32::WHITE),
                        );
                        ui.label(RichText::new(&stage).small().color(TEXT_DIM));
                        let (bar, _) = ui.allocate_exact_size(vec2(120.0, 6.0), Sense::hover());
                        progress(ui, bar, pr, ACCENT, false);
                        ui.label(
                            RichText::new(format!("{:.0}%", pr * 100.0))
                                .small()
                                .color(TEXT),
                        );
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Show").small().color(ACCENT_LIGHT),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            out = Pill::Open;
                        }
                    });
                });
        });
    out
}

fn demo_build(kind: &str, plan: &WorldPlan) -> BuildState {
    let names = super::build::STAGES;
    let cur = if kind == "done" { names.len() } else { 4 };
    let stages = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            (
                n.to_string(),
                if i < cur {
                    1.0
                } else if i == cur {
                    0.45
                } else {
                    0.0
                },
            )
        })
        .collect();
    let log = [
        "Shaped the land: one continent, 4 km across",
        "Painted the climates: snow in the north, desert in the south",
        "Rivers: 6, lakes: 3",
        "Grew forests, meadows and reeds",
        "Capital: Khazad Orn",
        "Towns: Frostgate, Emberhollow, Saltmere",
        "Villages: 4, roads: 21 km",
        "Zones: 7, levels 1–60",
        "Dungeon: The Sunken Forge (Dwarf Halls)",
    ];
    let mut p = plan.normalized();
    if p.name == "New World" {
        p.name = "Vaelmoor".into();
    }
    BuildState {
        plan: p,
        stages,
        current: cur.min(names.len() - 1),
        log: log.iter().map(|s| s.to_string()).collect(),
        done: kind == "done",
        cancelled: false,
        failed: (kind == "failed").then(|| {
            "The land couldn't be shaped: the generator ran out of memory. Try a smaller world."
                .to_string()
        }),
        report: if kind == "done" {
            vec![
                "Every town can be reached by road".into(),
                "Levels rise smoothly from the capital (1) to the far north (60)".into(),
                "Every zone has something to do: no empty zones".into(),
                "2 dungeons sit close together; that's fine for a raid wing".into(),
            ]
        } else {
            vec![]
        },
        elapsed: 83.0,
    }
}

// ---------------------------------------------------------------------------------------------
// surprise me

fn surprise(p: &mut WorldPlan) {
    let mut ids: Vec<&str> = INSPIRATIONS.iter().map(|i| i.id).collect();
    fastrand::shuffle(&mut ids);
    p.inspirations = ids[..fastrand::usize(1..=2)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    p.apply_inspirations();
    p.land = Land::ALL[fastrand::usize(..Land::ALL.len())];
    p.size = [
        WorldSize::Small,
        WorldSize::Medium,
        WorldSize::Medium,
        WorldSize::Large,
    ][fastrand::usize(..4)];
    p.climate = Climate::ALL[fastrand::usize(..Climate::ALL.len())];
    p.fit_to_size();
    p.rivers = fastrand::f32() < 0.85;
    p.capital = fastrand::f32() < 0.9;
    p.roads = true;
    p.farms = fastrand::f32() < 0.8;
    p.zones = true;
    p.encounters = true;
    p.creatures = true;
    p.dungeon_layouts.clear();
    if fastrand::bool() {
        let all: Vec<&str> = dpresets::all()
            .iter()
            .map(|d| d.id)
            .filter(|id| *id != "empty")
            .collect();
        for _ in 0..fastrand::usize(1..=3) {
            let id = all[fastrand::usize(..all.len())].to_string();
            if !p.dungeon_layouts.contains(&id) {
                p.dungeon_layouts.push(id);
            }
        }
    }
    p.dungeon_rooms = ["normal", "normal", "large", "extra_large"][fastrand::usize(..4)].into();
    p.dungeon_irregularity = 0.1 + fastrand::f32() * 0.5;
    let g = &mut p.gathering;
    (g.herbs, g.wood, g.stone, g.ore, g.fish) = (true, true, true, true, fastrand::f32() < 0.85);
    p.hour = look_of(&p.look)
        .scene()
        .map(|s| s.0)
        .unwrap_or(8.0 + fastrand::f32() * 10.0);
    p.seed = 0;
}

// ---------------------------------------------------------------------------------------------
// inspiration pictures (assets/onboarding/<id>.jpg), decoded off the frame

fn start_images(wiz: &mut Wiz) {
    wiz.images_started = true;
    let (tx, rx) = std::sync::mpsc::channel();
    let base = std::path::PathBuf::from(crate::assets_path()).join("onboarding");
    std::thread::spawn(move || {
        for key in picture_keys() {
            let img = image::open(base.join(format!("{key}.jpg"))).ok().map(|im| {
                let im = if im.width() > 720 {
                    im.resize(720, 720, image::imageops::FilterType::Triangle)
                } else {
                    im
                };
                let rgba = im.to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                )
            });
            if tx.send((key, img)).is_err() {
                return;
            }
        }
    });
    wiz.image_rx = Some(Mutex::new(rx));
}

/// Every picture the wizard can show (`assets/onboarding/<key>.jpg`); a missing one is drawn as a
/// painted sketch instead.
fn picture_keys() -> Vec<String> {
    let mut v: Vec<String> = INSPIRATIONS.iter().map(|i| i.id.to_string()).collect();
    v.extend(Land::ALL.map(|l| format!("land_{}", l.preset())));
    for art in ["hp", "real"] {
        v.extend(Climate::ALL.map(|c| format!("climate_{art}_{}", c.id())));
    }
    v.extend(GameKind::ALL.map(|g| format!("game_{}", game_id(g))));
    v.extend(Art::ALL.map(|a| format!("art_{}", a.id())));
    v.extend(LookPreset::ALL.map(|lp| format!("look_{}", look_id(lp))));
    v.extend(crate::world_kind::looks::ScifiLook::ALL.map(|l| l.picture()));
    v
}

fn game_id(g: GameKind) -> &'static str {
    match g {
        GameKind::Themepark => "themepark",
        GameKind::Sandbox => "sandbox",
        GameKind::RealmWar => "realm_war",
        GameKind::Survival => "survival",
        GameKind::Story => "story",
    }
}

fn climate_key(c: Climate, art: Art) -> String {
    format!(
        "climate_{}_{}",
        if art == Art::HandPainted {
            "hp"
        } else {
            "real"
        },
        c.id()
    )
}

fn poll_images(ctx: &egui::Context, wiz: &mut Wiz) {
    let Some(rx) = wiz.image_rx.as_ref() else {
        return;
    };
    let mut got = vec![];
    let mut finished = false;
    if let Ok(rx) = rx.lock() {
        loop {
            match rx.try_recv() {
                Ok(x) => got.push(x),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
    }
    for (id, img) in got {
        if let Some(img) = img {
            let t = ctx.load_texture(
                format!("onboarding_{id}"),
                img,
                egui::TextureOptions::LINEAR,
            );
            ctx.request_repaint();
            wiz.textures.insert(id, t);
        }
    }
    if finished {
        wiz.image_rx = None;
    }
}

/// A picture filling `rect` (cropped to its shape), rounded.
fn paint_cover(ui: &Ui, t: &egui::TextureHandle, rect: Rect, rad: u8) {
    let [w, h] = t.size();
    let (ia, ra) = (w as f32 / h.max(1) as f32, rect.width() / rect.height());
    let uv = if ia > ra {
        let k = ra / ia;
        Rect::from_min_max(pos2(0.5 - k / 2.0, 0.0), pos2(0.5 + k / 2.0, 1.0))
    } else {
        let k = ia / ra;
        Rect::from_min_max(pos2(0.0, 0.5 - k / 2.0), pos2(1.0, 0.5 + k / 2.0))
    };
    egui::Image::new((t.id(), rect.size()))
        .uv(uv)
        .corner_radius(CornerRadius::same(rad))
        .paint_at(ui, rect);
}

// ---------------------------------------------------------------------------------------------
// widgets

fn page_title(ui: &mut Ui, title: &str, sub: &str) {
    ui.label(
        RichText::new(title)
            .font(FontId::new(24.0, semibold()))
            .color(Color32::WHITE),
    );
    if !sub.is_empty() {
        ui.add(egui::Label::new(RichText::new(sub).size(13.5).color(TEXT_DIM)).wrap());
    }
    ui.add_space(14.0);
}

fn section_title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(FontId::new(14.5, semibold()))
            .color(Color32::WHITE),
    );
    ui.add_space(2.0);
}

fn dim_line(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(TEXT_DIM)).wrap());
}

fn info_card(ui: &mut Ui, text: &str, col: Color32) {
    egui::Frame::new()
        .fill(Color32::from_white_alpha(7))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).color(col)).wrap());
        });
}

fn primary(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(
            RichText::new(text)
                .font(FontId::new(14.0, semibold()))
                .color(ON_ACCENT),
        )
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(10))
        .min_size(vec2(120.0, 38.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn secondary(ui: &mut Ui, text: &str) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(TEXT))
            .fill(Color32::from_white_alpha(10))
            .stroke(Stroke::new(1.0, Color32::from_white_alpha(26)))
            .corner_radius(CornerRadius::same(10))
            .min_size(vec2(0.0, 38.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn ghost(ui: &mut Ui, text: &str) -> Response {
    let r = ui.add(
        egui::Button::new(RichText::new(text).color(TEXT_DIM))
            .frame(false)
            .min_size(vec2(0.0, 38.0)),
    );
    if r.hovered() {
        ui.painter()
            .rect_filled(r.rect, CornerRadius::same(9), Color32::from_white_alpha(8));
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small, non-interactive tag ("Hand-painted art").
fn tag(ui: &mut Ui, ic: &str, text: &str) {
    let g = ui
        .painter()
        .layout_no_wrap(format!("{ic}  {text}"), FontId::proportional(12.0), TEXT);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 24.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(12), Color32::from_white_alpha(12));
    p.galley(
        pos2(rect.left() + 10.0, rect.center().y - g.size().y / 2.0),
        g,
        TEXT,
    );
}

fn names_any(n: &Names) -> bool {
    !n.world.trim().is_empty()
        || !n.capital.trim().is_empty()
        || [&n.towns, &n.zones, &n.dungeons, &n.realms]
            .iter()
            .any(|l| l.iter().any(|s| !s.trim().is_empty()))
}

fn names_chips(ui: &mut Ui, n: &Names) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        let mut one = |ic: &str, s: &str| {
            if !s.trim().is_empty() {
                tag(ui, ic, s);
            }
        };
        one(icon::GLOBE_HEMISPHERE_WEST, &n.world);
        one(icon::CROWN, &n.capital);
        for s in &n.realms {
            one(icon::FLAG_BANNER, s);
        }
        for s in &n.towns {
            one(icon::HOUSE_LINE, s);
        }
        for s in &n.zones {
            one(icon::MAP_PIN, s);
        }
        for s in &n.dungeons {
            one(icon::SKULL, s);
        }
    });
}

/// A choice card's background (a tinted fill and an accent edge when picked).
fn card_bg(ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
    let p = ui.painter();
    let fill = if selected {
        Color32::from_white_alpha(22)
    } else if hovered {
        Color32::from_white_alpha(16)
    } else {
        Color32::from_white_alpha(8)
    };
    p.rect_filled(rect, CornerRadius::same(14), fill);
    let stroke = if selected {
        Stroke::new(2.0, ACCENT)
    } else if hovered {
        Stroke::new(1.0, Color32::from_white_alpha(60))
    } else {
        Stroke::new(1.0, Color32::from_white_alpha(14))
    };
    p.rect_stroke(rect, CornerRadius::same(14), stroke, StrokeKind::Inside);
}

fn pick_border(p: &egui::Painter, rect: Rect, rad: u8, selected: bool, hovered: bool) {
    if selected {
        p.rect_stroke(
            rect,
            CornerRadius::same(rad),
            Stroke::new(2.5, ACCENT),
            StrokeKind::Outside,
        );
    } else if hovered {
        p.rect_stroke(
            rect,
            CornerRadius::same(rad),
            Stroke::new(1.5, Color32::from_white_alpha(110)),
            StrokeKind::Outside,
        );
    }
}

fn check_badge(p: &egui::Painter, c: Pos2, text: &str) {
    p.circle_filled(c, 12.0, ACCENT);
    p.circle_stroke(c, 12.0, Stroke::new(2.0, Color32::from_black_alpha(90)));
    p.text(
        c,
        Align2::CENTER_CENTER,
        text,
        FontId::new(12.0, semibold()),
        ON_ACCENT,
    );
}

/// A rounded pill to toggle (icon + words).
fn chip(ui: &mut Ui, ic: &str, text: &str, selected: bool) -> Response {
    let label = if ic.is_empty() {
        text.to_string()
    } else {
        format!("{ic}  {text}")
    };
    let font = FontId::proportional(13.0);
    let w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), TEXT)
        .size()
        .x;
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 26.0, 32.0), Sense::click());
    let hov = resp.hovered();
    let (fill, stroke, col) = if selected {
        (Color32::WHITE, Stroke::NONE, ON_ACCENT)
    } else if hov {
        (
            Color32::from_white_alpha(16),
            Stroke::new(1.0, Color32::from_white_alpha(60)),
            Color32::WHITE,
        )
    } else {
        (
            Color32::from_white_alpha(8),
            Stroke::new(1.0, Color32::from_white_alpha(24)),
            TEXT,
        )
    };
    let p = ui.painter();
    p.rect(
        rect,
        CornerRadius::same(16),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    p.text(rect.center(), Align2::CENTER_CENTER, label, font, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row with an icon, a label and a hint; returns the row's response and rect.
fn option_row(
    ui: &mut Ui,
    ic: &str,
    label: &str,
    hint: &str,
    clickable: bool,
    right: f32,
) -> (Response, Rect) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(w, 54.0),
        if clickable {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let p = ui.painter();
    let hov = clickable && resp.hovered();
    p.rect_filled(
        rect,
        CornerRadius::same(11),
        if hov {
            Color32::from_white_alpha(15)
        } else {
            Color32::from_white_alpha(7)
        },
    );
    let ib = Rect::from_min_size(
        pos2(rect.left() + 10.0, rect.center().y - 17.0),
        vec2(34.0, 34.0),
    );
    p.rect_filled(ib, CornerRadius::same(9), Color32::from_white_alpha(12));
    p.text(
        ib.center(),
        Align2::CENTER_CENTER,
        ic,
        FontId::proportional(18.0),
        ACCENT_LIGHT,
    );
    let tx = ib.right() + 12.0;
    let tw = (rect.right() - right - 16.0 - tx).max(40.0);
    let lg = p.layout_no_wrap(
        label.to_string(),
        FontId::new(14.0, semibold()),
        Color32::WHITE,
    );
    let hg = p.layout(hint.to_string(), FontId::proportional(11.5), TEXT_DIM, tw);
    let one = hg.rows.len() <= 1;
    let total = lg.size().y + 2.0 + if one { hg.size().y } else { 14.0 };
    let y = rect.center().y - total / 2.0;
    p.galley(pos2(tx, y), lg.clone(), Color32::WHITE);
    if one {
        p.galley(pos2(tx, y + lg.size().y + 2.0), hg, TEXT_DIM);
    } else {
        // too long for one line: cut it, the whole hint on hover
        let cut = p.layout_no_wrap(
            ellipsize(hint, tw, &p, &FontId::proportional(11.5)),
            FontId::proportional(11.5),
            TEXT_DIM,
        );
        p.galley(pos2(tx, y + lg.size().y + 2.0), cut, TEXT_DIM);
    }
    let resp = if one { resp } else { resp.on_hover_text(hint) };
    ui.add_space(6.0 - ui.spacing().item_spacing.y);
    (resp, rect)
}

fn ellipsize(s: &str, w: f32, p: &egui::Painter, font: &FontId) -> String {
    let mut out = s.to_string();
    while !out.is_empty()
        && p.layout_no_wrap(format!("{out}…"), font.clone(), TEXT)
            .size()
            .x
            > w
    {
        out.pop();
    }
    format!("{}…", out.trim_end())
}

fn toggle_row(ui: &mut Ui, ic: &str, label: &str, hint: &str, v: &mut bool) -> bool {
    let (resp, rect) = option_row(ui, ic, label, hint, true, 54.0);
    let sw = Rect::from_min_size(
        pos2(rect.right() - 14.0 - 40.0, rect.center().y - 11.0),
        vec2(40.0, 22.0),
    );
    let t = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("sw"), *v, 0.12);
    let p = ui.painter();
    p.rect_filled(
        sw,
        CornerRadius::same(11),
        mix(Color32::from_rgb(64, 67, 72), ACCENT, t),
    );
    let x = egui::lerp(sw.left() + 11.0..=sw.right() - 11.0, t);
    p.circle_filled(
        pos2(x, sw.center().y),
        8.0,
        mix(Color32::WHITE, ON_ACCENT, t),
    );
    if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
    {
        *v = !*v;
        return true;
    }
    false
}

fn stepper_row(
    ui: &mut Ui,
    ic: &str,
    label: &str,
    hint: &str,
    v: &mut u32,
    min: u32,
    max: u32,
) -> bool {
    let (resp, rect) = option_row(ui, ic, label, hint, false, 112.0);
    let cy = rect.center().y;
    let plus = Rect::from_center_size(pos2(rect.right() - 14.0 - 15.0, cy), vec2(30.0, 30.0));
    let minus = Rect::from_center_size(pos2(plus.center().x - 30.0 - 40.0, cy), vec2(30.0, 30.0));
    let mut changed = false;
    for (r, glyph, delta) in [(minus, icon::MINUS, -1i32), (plus, icon::PLUS, 1)] {
        let can = if delta < 0 { *v > min } else { *v < max };
        let b = ui.interact(
            r,
            resp.id.with(glyph),
            if can { Sense::click() } else { Sense::hover() },
        );
        let p = ui.painter();
        let fill = if !can {
            Color32::from_white_alpha(5)
        } else if b.hovered() {
            Color32::from_white_alpha(34)
        } else {
            Color32::from_white_alpha(16)
        };
        p.circle_filled(r.center(), 15.0, fill);
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(14.0),
            if can {
                Color32::WHITE
            } else {
                Color32::from_rgb(80, 84, 90)
            },
        );
        if can && b.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            *v = (*v as i32 + delta).clamp(min as i32, max as i32) as u32;
            changed = true;
        }
    }
    ui.painter().text(
        pos2((minus.center().x + plus.center().x) / 2.0, cy),
        Align2::CENTER_CENTER,
        v.to_string(),
        FontId::new(17.0, semibold()),
        Color32::WHITE,
    );
    changed
}

/// The level range: one rail, two handles (1–100; at least 4 levels apart).
fn level_range(ui: &mut Ui, v: &mut [u32; 2]) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 40.0), Sense::click_and_drag());
    let rail = Rect::from_center_size(rect.center() - vec2(0.0, 4.0), vec2(w - 24.0, 6.0));
    let to_x = |l: u32| egui::remap_clamp(l as f32, 1.0..=100.0, rail.left()..=rail.right());
    let from_x =
        |x: f32| egui::remap_clamp(x, rail.left()..=rail.right(), 1.0..=100.0).round() as u32;
    let which_id = resp.id.with("which");
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.drag_started() || resp.clicked() {
            let (d0, d1) = ((pos.x - to_x(v[0])).abs(), (pos.x - to_x(v[1])).abs());
            let which = usize::from(!(d0 < d1 || (d0 == d1 && pos.x < to_x(v[0]))));
            ui.ctx().data_mut(|d| d.insert_temp(which_id, which));
        }
        if resp.dragged() || resp.clicked() {
            let which = ui
                .ctx()
                .data(|d| d.get_temp::<usize>(which_id))
                .unwrap_or(1);
            let x = from_x(pos.x);
            let mut n = *v;
            if which == 0 {
                n[0] = x.clamp(1, n[1].saturating_sub(4).max(1));
            } else {
                n[1] = x.clamp(n[0] + 4, 100);
            }
            if n != *v {
                *v = n;
                changed = true;
            }
        }
    }
    let p = ui.painter();
    p.rect_filled(rail, CornerRadius::same(3), Color32::from_white_alpha(26));
    let (a, b) = (to_x(v[0]), to_x(v[1]));
    p.rect_filled(
        Rect::from_min_max(pos2(a, rail.top()), pos2(b, rail.bottom())),
        CornerRadius::same(3),
        ACCENT,
    );
    for (x, l) in [(a, v[0]), (b, v[1])] {
        let c = pos2(x, rail.center().y);
        p.circle_filled(c, 10.0, Color32::WHITE);
        p.circle_stroke(
            c,
            10.0,
            Stroke::new(2.5, Color32::from_rgba_unmultiplied(10, 12, 16, 140)),
        );
        p.text(
            pos2(x, rail.bottom() + 14.0),
            Align2::CENTER_CENTER,
            l.to_string(),
            FontId::new(11.5, semibold()),
            TEXT,
        );
    }
    let _ = resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    changed
}

fn progress(ui: &Ui, r: Rect, f: f32, col: Color32, shimmer: bool) {
    let p = ui.painter();
    let rad = CornerRadius::same((r.height() / 2.0) as u8);
    p.rect_filled(r, rad, Color32::from_white_alpha(18));
    let f = f.clamp(0.0, 1.0);
    if f > 0.0 {
        let fr = Rect::from_min_size(r.min, vec2((r.width() * f).max(r.height()), r.height()));
        p.rect_filled(fr, rad, col);
        if shimmer {
            let t = ui.input(|i| i.time) as f32;
            let x = fr.left() + ((t * 0.6).fract() * 1.4 - 0.2) * fr.width();
            let band = Rect::from_center_size(
                pos2(x, fr.center().y),
                vec2(fr.width().min(80.0), fr.height()),
            )
            .intersect(fr);
            if band.width() > 0.0 {
                p.rect_filled(band, rad, Color32::from_white_alpha(30));
            }
            ui.ctx().request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// pictures: little painted landscapes and maps (placeholders for missing pictures, look swatches)

fn seed_of(s: &str) -> u32 {
    s.bytes()
        .fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

fn shade(c: Color32, f: f32) -> Color32 {
    let s = |x: u8| (x as f32 * f).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(s(c.r()), s(c.g()), s(c.b()))
}

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// A vertical gradient rectangle.
fn vgrad(mesh: &mut Mesh, r: Rect, top: Color32, bottom: Color32) {
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(r.left_top(), top);
    mesh.colored_vertex(r.right_top(), top);
    mesh.colored_vertex(r.left_bottom(), bottom);
    mesh.colored_vertex(r.right_bottom(), bottom);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i + 1, i + 2, i + 3);
}

/// A soft round glow (bright in the middle, gone at `r`).
fn glow(p: &egui::Painter, c: Pos2, r: f32, col: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(c, col);
    let n = 48u32;
    let half = mix(Color32::TRANSPARENT, col, 0.35);
    for ring in [(0.35, half), (1.0, Color32::TRANSPARENT)] {
        for k in 0..n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            m.colored_vertex(c + vec2(a.cos(), a.sin()) * r * ring.0, ring.1);
        }
    }
    for k in 0..n {
        let k1 = (k + 1) % n;
        m.add_triangle(0, 1 + k, 1 + k1);
        m.add_triangle(1 + k, 1 + k1, 1 + n + k);
        m.add_triangle(1 + k1, 1 + n + k, 1 + n + k1);
    }
    p.add(Shape::mesh(m));
}

/// Darkens the bottom of a picture so words on it read.
fn scrim(p: &egui::Painter, r: Rect, from: f32, a: u8) {
    let mut m = Mesh::default();
    let mid = r.top() + r.height() * from;
    vgrad(
        &mut m,
        Rect::from_min_max(pos2(r.left(), mid), r.max),
        Color32::TRANSPARENT,
        Color32::from_black_alpha(a),
    );
    p.add(Shape::mesh(m));
}

/// Paints the panel's colour over a picture's square corners (outside its rounded shape).
fn corner_mask(p: &egui::Painter, r: Rect, rad: u8) {
    p.rect_stroke(
        r,
        CornerRadius::same(rad),
        Stroke::new(rad as f32 * 0.5 + 1.5, PANEL),
        StrokeKind::Outside,
    );
}

/// A mountain line across `n` steps: y as a fraction of the height from the top.
fn ridge(
    rng: &mut fastrand::Rng,
    n: usize,
    base: f32,
    amp: f32,
    peaks: usize,
    sharp: bool,
) -> Vec<f32> {
    let cs: Vec<(f32, f32, f32)> = (0..peaks)
        .map(|_| {
            (
                rng.f32() * 1.2 - 0.1,
                0.1 + rng.f32() * 0.22,
                0.45 + rng.f32() * 0.55,
            )
        })
        .collect();
    let ph = rng.f32() * 6.28;
    (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32;
            let mut h: f32 = 0.0;
            for &(c, w, a) in &cs {
                let d = ((x - c).abs() / w).min(1.0);
                let v = if sharp {
                    1.0 - d
                } else {
                    (1.0 - d * d).max(0.0)
                };
                h = h.max(v * a);
            }
            h += 0.05 * (x * 19.0 + ph).sin() + 0.025 * (x * 47.0 + ph * 1.7).sin();
            base - amp * h
        })
        .collect()
}

fn band(p: &egui::Painter, r: Rect, ys: &[f32], top: Color32, bottom: Color32, outline: bool) {
    let n = ys.len() - 1;
    let mut m = Mesh::default();
    let mut line = vec![];
    for (i, y) in ys.iter().enumerate() {
        let x = r.left() + r.width() * i as f32 / n as f32;
        let y = r.top() + r.height() * y;
        let k = m.vertices.len() as u32;
        m.colored_vertex(pos2(x, y), top);
        m.colored_vertex(pos2(x, r.bottom()), bottom);
        if i > 0 {
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k, k + 1);
        }
        line.push(pos2(x, y));
    }
    p.add(Shape::mesh(m));
    if outline {
        p.add(Shape::line(line, Stroke::new(1.2, shade(top, 0.62))));
    }
}

fn trees(
    p: &egui::Painter,
    r: Rect,
    ys: &[f32],
    col: Color32,
    rng: &mut fastrand::Rng,
    count: usize,
    scale: f32,
) {
    let n = ys.len() - 1;
    for _ in 0..count {
        let i = rng.usize(..=n);
        let x = r.left() + r.width() * i as f32 / n as f32;
        let y = r.top() + r.height() * ys[i] + 1.5;
        let h = r.height() * 0.085 * scale * (0.7 + rng.f32() * 0.6);
        let w = h * 0.42;
        p.add(Shape::convex_polygon(
            vec![pos2(x - w, y), pos2(x + w, y), pos2(x, y - h)],
            col,
            Stroke::NONE,
        ));
        p.add(Shape::convex_polygon(
            vec![
                pos2(x - w * 0.8, y - h * 0.4),
                pos2(x + w * 0.8, y - h * 0.4),
                pos2(x, y - h * 1.25),
            ],
            col,
            Stroke::NONE,
        ));
    }
}

/// A little landscape in a palette (ground, hills, sky, light): sky, sun, far mountains, hills, trees.
fn paint_landscape(p: &egui::Painter, rect: Rect, pal: [[u8; 3]; 4], seed: u32, painted: bool) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let (ground, mid, sky, light) = (rgb(pal[0]), rgb(pal[1]), rgb(pal[2]), rgb(pal[3]));
    let mut rng = fastrand::Rng::with_seed(seed as u64 ^ 0x9e37_79b9);
    let mut m = Mesh::default();
    let horizon = rect.top() + rect.height() * 0.66;
    vgrad(
        &mut m,
        Rect::from_min_max(rect.min, pos2(rect.right(), horizon)),
        shade(sky, 0.8),
        mix(sky, light, 0.7),
    );
    vgrad(
        &mut m,
        Rect::from_min_max(pos2(rect.left(), horizon), rect.max),
        mix(sky, light, 0.7),
        mix(mid, ground, 0.5),
    );
    p.add(Shape::mesh(m));
    // the sun and its glow
    let sun = pos2(
        rect.left() + rect.width() * (0.18 + 0.64 * rng.f32()),
        rect.top() + rect.height() * (0.2 + 0.14 * rng.f32()),
    );
    glow(&p, sun, rect.height() * 0.6, alpha(light, 110));
    glow(
        &p,
        sun,
        rect.height() * 0.15,
        alpha(mix(light, Color32::WHITE, 0.4), 170),
    );
    p.circle_filled(sun, rect.height() * 0.035, mix(light, Color32::WHITE, 0.55));
    // ranges, far to near
    let far = ridge(&mut rng, 56, 0.6, 0.36, 4, true);
    let far_col = mix(mid, sky, 0.48);
    band(&p, rect, &far, far_col, mix(far_col, sky, 0.35), painted);
    if !painted {
        // haze over the distance
        let mut h = Mesh::default();
        vgrad(
            &mut h,
            Rect::from_min_max(
                pos2(rect.left(), rect.top() + rect.height() * 0.4),
                pos2(rect.right(), rect.top() + rect.height() * 0.75),
            ),
            Color32::TRANSPARENT,
            alpha(light, 70),
        );
        p.add(Shape::mesh(h));
    }
    let hills = ridge(&mut rng, 56, 0.77, 0.17, 5, false);
    let hill_col = mix(ground, mid, 0.42);
    band(&p, rect, &hills, hill_col, shade(hill_col, 0.82), painted);
    trees(&p, rect, &hills, shade(ground, 0.62), &mut rng, 8, 0.7);
    let near = ridge(&mut rng, 56, 0.9, 0.12, 3, false);
    band(&p, rect, &near, ground, shade(ground, 0.55), painted);
    trees(&p, rect, &near, shade(ground, 0.45), &mut rng, 5, 1.0);
}

fn climate_palette(c: Climate) -> [[u8; 3]; 4] {
    match c {
        Climate::Varied | Climate::Temperate => [
            [70, 124, 56],
            [112, 150, 92],
            [116, 168, 222],
            [242, 240, 220],
        ],
        Climate::Tropical => [
            [30, 112, 66],
            [48, 158, 136],
            [84, 196, 228],
            [250, 244, 196],
        ],
        Climate::Arid => [
            [196, 136, 74],
            [196, 104, 62],
            [148, 188, 228],
            [255, 230, 180],
        ],
        Climate::Cold => [
            [214, 226, 238],
            [124, 146, 170],
            [150, 182, 216],
            [246, 250, 255],
        ],
    }
}

/// A climate's card picture; "every climate" runs from snow through green to desert.
fn paint_climate(p: &egui::Painter, rect: Rect, c: Climate) {
    if c != Climate::Varied {
        paint_landscape(p, rect, climate_palette(c), 101 + c as u32, false);
        return;
    }
    let parts = [Climate::Cold, Climate::Temperate, Climate::Arid];
    let w = rect.width() / 3.0;
    for (i, cl) in parts.iter().enumerate() {
        let slice = Rect::from_min_size(
            rect.min + vec2(w * i as f32, 0.0),
            vec2(w + 0.5, rect.height()),
        );
        paint_landscape(
            &p.with_clip_rect(slice.intersect(p.clip_rect())),
            rect,
            climate_palette(*cl),
            101,
            false,
        );
    }
}

/// A top-down sketch of a shape of land.
fn paint_land_map(p: &egui::Painter, rect: Rect, land: Land, rivers: bool) {
    let p = p.with_clip_rect(rect.intersect(p.clip_rect()));
    let sea = Color32::from_rgb(30, 78, 118);
    let mut m = Mesh::default();
    vgrad(
        &mut m,
        rect,
        Color32::from_rgb(36, 90, 132),
        Color32::from_rgb(22, 56, 92),
    );
    p.add(Shape::mesh(m));
    let (sand, grass, dark) = (
        Color32::from_rgb(214, 198, 142),
        Color32::from_rgb(96, 146, 76),
        Color32::from_rgb(70, 116, 60),
    );
    let s = rect.height();
    let at = |u: f32, v: f32| {
        pos2(
            rect.left() + u * rect.width(),
            rect.top() + v * rect.height(),
        )
    };
    let blobs: &[(f32, f32, f32)] = match land {
        Land::Continent => &[
            (0.5, 0.5, 0.34),
            (0.36, 0.42, 0.26),
            (0.64, 0.58, 0.28),
            (0.42, 0.68, 0.2),
            (0.6, 0.32, 0.2),
            (0.27, 0.56, 0.16),
            (0.73, 0.42, 0.17),
        ],
        Land::TwinIslands => &[
            (0.27, 0.42, 0.25),
            (0.22, 0.63, 0.18),
            (0.35, 0.6, 0.15),
            (0.73, 0.58, 0.25),
            (0.79, 0.38, 0.17),
            (0.65, 0.4, 0.14),
        ],
        Land::Archipelago => &[
            (0.17, 0.3, 0.11),
            (0.32, 0.62, 0.14),
            (0.5, 0.33, 0.12),
            (0.63, 0.7, 0.1),
            (0.79, 0.4, 0.13),
            (0.87, 0.74, 0.08),
            (0.12, 0.76, 0.08),
            (0.46, 0.86, 0.07),
            (0.67, 0.14, 0.07),
        ],
        Land::Peninsula => &[
            (0.0, 0.18, 0.3),
            (0.05, 0.55, 0.36),
            (0.0, 0.92, 0.3),
            (0.18, 0.34, 0.18),
            (0.3, 0.5, 0.21),
            (0.42, 0.53, 0.19),
            (0.5, 0.42, 0.11),
            (0.53, 0.62, 0.17),
            (0.63, 0.7, 0.14),
            (0.73, 0.66, 0.08),
            (0.69, 0.82, 0.11),
            (0.6, 0.86, 0.07),
        ],
        Land::InlandSea => &[
            (0.5, 0.5, 0.44),
            (0.3, 0.46, 0.32),
            (0.7, 0.54, 0.32),
            (0.5, 0.2, 0.2),
            (0.5, 0.82, 0.2),
        ],
        Land::FjordCoast => &[
            (0.08, 0.2, 0.36),
            (0.08, 0.62, 0.4),
            (0.3, 0.38, 0.36),
            (0.3, 0.9, 0.3),
            (0.46, 0.14, 0.24),
            (0.5, 0.62, 0.28),
        ],
        Land::MountainIsland => &[(0.5, 0.52, 0.38), (0.42, 0.45, 0.3), (0.6, 0.6, 0.28)],
    };
    for &(u, v, r) in blobs {
        p.circle_filled(at(u, v), r * s + 1.8, sand);
    }
    for &(u, v, r) in blobs {
        p.circle_filled(at(u, v), r * s, grass);
    }
    // darker woods inside the bigger blobs
    for &(u, v, r) in blobs.iter().filter(|b| b.2 > 0.2) {
        p.circle_filled(at(u + 0.02, v + 0.03), r * s * 0.45, dark);
    }
    match land {
        Land::InlandSea => {
            p.circle_filled(at(0.5, 0.52), 0.22 * s + 1.8, sand);
            p.circle_filled(at(0.5, 0.52), 0.22 * s, sea);
        }
        Land::FjordCoast => {
            for (a, b) in [
                ((1.0, 0.24), (0.36, 0.3)),
                ((1.0, 0.52), (0.42, 0.5)),
                ((1.0, 0.8), (0.44, 0.84)),
                ((0.7, 0.05), (0.62, 0.3)),
            ] {
                p.line_segment([at(a.0, a.1), at(b.0, b.1)], Stroke::new(s * 0.06, sea));
            }
        }
        Land::MountainIsland => {
            p.circle_filled(at(0.5, 0.5), 0.16 * s, Color32::from_rgb(132, 128, 118));
            p.circle_filled(at(0.5, 0.48), 0.07 * s, Color32::from_rgb(240, 244, 248));
        }
        _ => {}
    }
    if rivers
        && matches!(
            land,
            Land::Continent | Land::Peninsula | Land::MountainIsland | Land::TwinIslands
        )
    {
        let pts: &[(f32, f32)] = match land {
            Land::Continent => &[(0.5, 0.42), (0.55, 0.55), (0.52, 0.66), (0.58, 0.8)],
            Land::Peninsula => &[(0.08, 0.3), (0.14, 0.45), (0.2, 0.6), (0.22, 0.78)],
            Land::MountainIsland => &[(0.5, 0.6), (0.44, 0.72), (0.4, 0.85)],
            _ => &[(0.26, 0.4), (0.3, 0.52), (0.27, 0.66)],
        };
        p.add(Shape::line(
            pts.iter().map(|&(u, v)| at(u, v)).collect(),
            Stroke::new(1.4, Color32::from_rgb(110, 170, 220)),
        ));
    }
}

// ---------------------------------------------------------------------------------------------
// the entry in File › New world

/// The New world dialog's "Guided setup" card. True when clicked (the wizard opens).
pub fn entry_card(ui: &mut Ui) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 66.0), Sense::click());
    let hov = resp.hovered();
    let p = ui.painter();
    p.rect_filled(
        rect,
        CornerRadius::same(12),
        if hov { ACCENT_HOVER } else { ACCENT },
    );
    p.text(
        pos2(rect.left() + 22.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon::SPARKLE,
        FontId::proportional(24.0),
        ON_ACCENT,
    );
    p.text(
        pos2(rect.left() + 44.0, rect.center().y - 10.0),
        Align2::LEFT_CENTER,
        "Guided setup",
        FontId::new(16.0, semibold()),
        ON_ACCENT,
    );
    p.text(
        pos2(rect.left() + 44.0, rect.center().y + 11.0),
        Align2::LEFT_CENTER,
        "Answer a few questions, get a playable world",
        FontId::proportional(12.5),
        ON_ACCENT.gamma_multiply(0.7),
    );
    p.text(
        pos2(rect.right() - 18.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon::ARROW_RIGHT,
        FontId::proportional(18.0),
        ON_ACCENT,
    );
    resp.on_hover_text("Pick the MMOs you love, the kind of game and the land: a world with towns, zones, dungeons and creatures is built for you")
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}
