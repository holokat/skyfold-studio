//! ⌘K: a palette that finds every tab, tool, asset, look and the main actions (HANDOFF › ⌘K
//! palette). ↑↓ moves, ↵ runs, Esc closes; with no match, ↵ opens Prompt an area with the words.

use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Id, Key, Margin, Modifiers, Order, Rect, RichText, Sense,
    Stroke, TextureId, pos2, vec2,
};

use super::theme::*;
use super::widgets::*;
use super::{UiAction, UiOut, UiState};
use crate::editor::{
    LandTool, ObjectTool, PaintTool, SculptTool, Tab, TabGroup, WaterTool, ZoneTool,
};
use crate::tools::EditorAction;

#[derive(Clone, Default)]
struct Pal {
    open: bool,
    query: String,
    sel: usize,
    focus: bool,
}

fn pal_id() -> Id {
    Id::new("halo_palette")
}

fn state(ctx: &egui::Context) -> Pal {
    ctx.data(|d| d.get_temp(pal_id())).unwrap_or_default()
}

fn store(ctx: &egui::Context, p: Pal) {
    ctx.data_mut(|d| d.insert_temp(pal_id(), p));
}

/// Open the palette (the search island, ⌘K).
pub fn open(ctx: &egui::Context) {
    store(
        ctx,
        Pal {
            open: true,
            focus: true,
            ..Default::default()
        },
    );
}

#[derive(Clone)]
enum Act {
    Tab(Tab),
    Fields,
    Land(LandTool),
    Sculpt(SculptTool),
    Paint(PaintTool),
    Water(WaterTool),
    Objects(ObjectTool),
    Zone(ZoneTool),
    Asset(String, crate::objects::Category),
    Look(crate::look::LookPreset),
    Ui(UiAction),
    Edit(EditorAction),
    NewWorld,
    Playtest,
    Paths,
    Prompt,
    Help,
    Mcp,
    Gallery,
    Mute,
    Panels,
}

struct Item {
    kind: &'static str,
    name: String,
    sub: String,
    icon: &'static str,
    key: &'static str,
    thumb: Option<crate::thumbs::Subject>,
    act: Act,
}

fn item(
    kind: &'static str,
    name: impl Into<String>,
    icon: &'static str,
    key: &'static str,
    act: Act,
) -> Item {
    Item {
        kind,
        name: name.into(),
        sub: String::new(),
        icon,
        key,
        thumb: None,
        act,
    }
}

fn items(s: &UiState) -> Vec<Item> {
    let mut v = vec![];
    for (gi, g) in TabGroup::visible().iter().enumerate() {
        let tabs = g.tabs();
        for t in tabs {
            let name = if tabs.len() > 1 {
                format!("{} › {}", g.name(), TabGroup::sub_name(*t))
            } else {
                g.name().to_string()
            };
            let key = if tabs.len() > 1 {
                ""
            } else {
                ["1", "2", "3", "4", "5", "6", "7", "8", "9"][gi.min(8)]
            };
            v.push(item(
                "Go to",
                name,
                super::shell::group_icon(*g),
                key,
                Act::Tab(*t),
            ));
        }
        if *g == TabGroup::Nature {
            v.push(item("Go to", "Nature › Fields", "Grass", "", Act::Fields));
        }
    }
    for (t, n, i) in [
        (LandTool::PaintLand, "Paint Land", "PlusMedium"),
        (LandTool::PaintSea, "Paint Sea", "MinusMedium"),
        (LandTool::DrawCoast, "Draw Coastline", "SelectLasso"),
        (LandTool::Reference, "Trace a Picture", "Images1"),
    ] {
        v.push(item("Tool", n, i, "", Act::Land(t)));
    }
    for t in SculptTool::ALL {
        v.push(item("Tool", t.name(), "Hammer", "", Act::Sculpt(t)));
    }
    v.push(item(
        "Tool",
        "Paint ground",
        "PaintBrush",
        "",
        Act::Paint(PaintTool::Paint),
    ));
    v.push(item(
        "Tool",
        "Erase paint to Auto",
        "Eraser",
        "",
        Act::Paint(PaintTool::Erase),
    ));
    for (t, n, i) in [
        (WaterTool::River, "Draw River", "AnimatePath"),
        (WaterTool::Lake, "Fill a Lake", "Drop"),
        (WaterTool::DrawLake, "Draw Lake", "SelectLasso"),
        (WaterTool::ShapeLake, "Shape Shore", "Draw"),
        (WaterTool::Waterfall, "Waterfall", "ArrowDown"),
        (WaterTool::Fish, "Add Fish", "Anchor"),
    ] {
        v.push(item("Tool", n, i, "", Act::Water(t)));
    }
    v.push(item(
        "Tool",
        "Place objects",
        "CursorClick",
        "",
        Act::Objects(ObjectTool::Place),
    ));
    v.push(item(
        "Tool",
        "Select & Move",
        "Cursor1",
        "",
        Act::Objects(ObjectTool::Select),
    ));
    v.push(item(
        "Tool",
        "Paint a zone",
        "Map",
        "",
        Act::Zone(ZoneTool::Paint),
    ));
    v.push(item(
        "Tool",
        "Erase a zone",
        "Eraser",
        "",
        Act::Zone(ZoneTool::Erase),
    ));
    v.push(item(
        "Tool",
        "Paint a biome",
        "Tree",
        "",
        Act::Zone(ZoneTool::Biome),
    ));
    if let Some(lib) = &s.lib {
        for e in &lib.entries {
            v.push(Item {
                kind: "Place",
                name: e.name.clone(),
                sub: e.category.name().to_string(),
                icon: "3dBoxTop",
                key: "",
                thumb: Some(crate::thumbs::Subject::Asset(e.id.clone())),
                act: Act::Asset(e.id.clone(), e.category),
            });
        }
    }
    let acts: [(&str, &'static str, &'static str, Act); 16] = [
        ("New world…", "PageAdd", "⌘N", Act::NewWorld),
        ("Open…", "FolderOpen", "⌘O", Act::Ui(UiAction::Open)),
        ("Save", "FloppyDisk1", "⌘S", Act::Ui(UiAction::Save)),
        ("Save as…", "FloppyDisk1", "⇧⌘S", Act::Ui(UiAction::SaveAs)),
        (
            "Export to Godot / Unity / Unreal…",
            "ArrowOutOfBox",
            "⌘E",
            Act::Ui(UiAction::Export),
        ),
        (
            "Import a 3D model…",
            "Import",
            "",
            Act::Ui(UiAction::ImportModel),
        ),
        (
            "Walk the World",
            "Footsteps",
            "",
            Act::Ui(UiAction::ToggleWalk),
        ),
        ("Playtest", "Gamepad", "", Act::Playtest),
        ("Camera paths & flythrough…", "Video", "", Act::Paths),
        ("Prompt an area", "SparklesTwo", "G", Act::Prompt),
        ("Help & shortcuts", "QuestionmarkCircle", "H", Act::Help),
        ("Connect an agent (MCP)", "Agent", "", Act::Mcp),
        ("Asset gallery", "LayoutGrid1", "", Act::Gallery),
        ("Undo", "ArrowUndoUp", "⌘Z", Act::Edit(EditorAction::Undo)),
        (
            "Redo",
            "ArrowRedoDown",
            "⇧⌘Z",
            Act::Edit(EditorAction::Redo),
        ),
        ("Mute / unmute sound", "VolumeFull", "", Act::Mute),
    ];
    for (n, i, k, a) in acts {
        v.push(item("Action", n, i, k, a));
    }
    v.push(item(
        "Action",
        "Hide / show panels",
        "SidebarSimpleLeftWide",
        "Tab",
        Act::Panels,
    ));
    v.push(item(
        "Action",
        "Auto-populate nature",
        "Tree",
        "",
        Act::Ui(UiAction::PopulateWorld),
    ));
    v.push(item(
        "Action",
        "Erode the whole world",
        "Rainy",
        "",
        Act::Ui(UiAction::ErodeWorld),
    ));
    for p in crate::look::LookPreset::ALL {
        let mut it = item("Look", p.name(), "ColorPalette", "", Act::Look(p));
        it.sub = p.inspired_by().to_string();
        v.push(it);
    }
    v
}

fn matches(all: Vec<Item>, q: &str) -> Vec<Item> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        let mut out: Vec<Item> = vec![];
        let mut take = |kind: &str, n: usize, all: &mut Vec<Item>| {
            let mut k = 0;
            let mut i = 0;
            while i < all.len() && k < n {
                if all[i].kind == kind {
                    out.push(all.remove(i));
                    k += 1;
                } else {
                    i += 1;
                }
            }
        };
        let mut all = all;
        take("Action", 5, &mut all);
        take("Tool", 3, &mut all);
        take("Go to", 3, &mut all);
        return out;
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(i32, Item)> = all
        .into_iter()
        .filter_map(|it| {
            let hay = format!("{} {} {}", it.name, it.sub, it.kind).to_lowercase();
            if !words.iter().all(|w| hay.contains(w)) {
                return None;
            }
            let n = it.name.to_lowercase();
            let score = if n == q {
                0
            } else if n.starts_with(&q) {
                1
            } else if n
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(words[0]))
            {
                2
            } else {
                3
            };
            let kind = match it.kind {
                "Go to" => 0,
                "Tool" => 1,
                "Action" => 2,
                "Look" => 3,
                _ => 4,
            };
            Some((score * 10 + kind, it))
        })
        .collect();
    hits.sort_by_key(|h| h.0);
    hits.into_iter().take(10).map(|h| h.1).collect()
}

fn run(act: Act, s: &mut UiState, out: &mut UiOut, ctx: &egui::Context) {
    let go = |s: &mut UiState, t: Tab| {
        s.ed.tab = t;
    };
    match act {
        Act::Tab(t) => {
            go(s, t);
            if t == Tab::Nature {
                s.field_tool.on = false;
            }
        }
        Act::Fields => {
            go(s, Tab::Nature);
            s.field_tool.on = true;
        }
        Act::Land(t) => {
            go(s, Tab::Land);
            s.ed.land_tool = t;
        }
        Act::Sculpt(t) => {
            go(s, Tab::Sculpt);
            s.ed.sculpt_tool = t;
        }
        Act::Paint(t) => {
            go(s, Tab::Paint);
            s.ed.paint_tool = t;
        }
        Act::Water(t) => {
            go(s, Tab::Water);
            s.ed.water_tool = t;
        }
        Act::Objects(t) => {
            go(s, Tab::Objects);
            s.ed.object_tool = t;
        }
        Act::Zone(t) => {
            go(s, Tab::Zones);
            s.ed.zone_tool = t;
        }
        Act::Asset(id, cat) => {
            go(s, Tab::Objects);
            s.ed.object_tool = ObjectTool::Place;
            s.place.category = cat;
            s.place.group.clear();
            s.place.selected = Some(id);
        }
        Act::Look(p) => s.lighting.look.select(p),
        Act::Ui(a) => {
            out.actions.write(a);
        }
        Act::Edit(a) => {
            out.editor_actions.write(a);
        }
        Act::NewWorld => s.ed.show_new_world = true,
        Act::Playtest => {
            out.motion.write(crate::motion::MotionAction::Playtest);
        }
        Act::Paths => {
            out.motion.write(crate::motion::MotionAction::OpenPanel(
                crate::motion::PanelTab::Paths,
            ));
        }
        Act::Prompt => crate::prompt::button_clicked(&mut s.prompt),
        Act::Help => s.ed.show_help = true,
        Act::Mcp => s.mcp.show_panel = true,
        Act::Gallery => s.gallery.open = true,
        Act::Mute => s.audio.muted = !s.audio.muted,
        Act::Panels => {
            let mut sh = super::shell::get(ctx);
            let open = !(sh.panel_open || sh.insp_open);
            sh.panel_open = open;
            sh.insp_open = open;
            super::shell::set(ctx, sh);
        }
    }
}

/// ⌘K anywhere opens it; draws it when open.
pub fn show(ctx: &egui::Context, s: &mut UiState, out: &mut UiOut) {
    if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
        let p = state(ctx);
        if p.open {
            store(ctx, Pal::default());
        } else {
            open(ctx);
        }
    }
    let mut p = state(ctx);
    if !p.open {
        return;
    }
    let screen = ctx.content_rect();
    // the scrim (a click closes)
    let scrim = egui::Area::new(Id::new("halo_palette_scrim"))
        .order(Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(r, 0.0, Color32::from_rgba_unmultiplied_const(4, 6, 9, 128));
            resp.clicked()
        });
    let found = matches(items(s), &p.query);
    p.sel = p.sel.min(found.len().saturating_sub(1));
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if down && !found.is_empty() {
        p.sel = (p.sel + 1).min(found.len() - 1);
    }
    if up {
        p.sel = p.sel.saturating_sub(1);
    }
    let mut chosen: Option<usize> = enter.then_some(p.sel).filter(|_| !found.is_empty());
    let prompt_words = enter && found.is_empty() && !p.query.trim().is_empty();
    let w = 560.0;
    egui::Area::new(Id::new("halo_palette"))
        .order(Order::Foreground)
        .fixed_pos(pos2(screen.center().x - w * 0.5, screen.top() + 110.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied_const(18, 21, 27, 230))
                .stroke(Stroke::new(1.0, LINE_STRONG))
                .corner_radius(CornerRadius::same(16))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 30],
                    blur: 80,
                    spread: 0,
                    color: Color32::from_black_alpha(170),
                })
                .show(ui, |ui| {
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // the search line
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(14, 0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.set_height(46.0);
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(16.0, 46.0), Sense::hover());
                                paint_icon(ui, r.center(), "MagnifyingGlass", 15.0, TEXT_MUTE);
                                let te = egui::TextEdit::singleline(&mut p.query)
                                    .frame(egui::Frame::NONE)
                                    .font(FontId::proportional(16.0))
                                    .hint_text(
                                        RichText::new("Search tools, assets, actions, looks…")
                                            .size(16.0)
                                            .color(TEXT_MUTE),
                                    )
                                    .desired_width(ui.available_width() - 40.0);
                                let r = ui.add(te);
                                if p.focus {
                                    r.request_focus();
                                    p.focus = false;
                                }
                                if r.changed() {
                                    p.sel = 0;
                                }
                                kbd(ui, "esc");
                            });
                        });
                    let (lr, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
                    ui.painter().rect_filled(lr, 0.0, LINE);
                    // results
                    egui::Frame::new()
                        .inner_margin(Margin::same(6))
                        .show(ui, |ui| {
                            ui.set_width(w - 12.0);
                            if found.is_empty() {
                                egui::Frame::new()
                                    .inner_margin(Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                RichText::new("No match. Press")
                                                    .size(12.5)
                                                    .color(TEXT_DIM),
                                            );
                                            kbd(ui, "↵");
                                            ui.label(
                                                RichText::new(format!(
                                                    "to prompt an area with “{}”.",
                                                    p.query.trim()
                                                ))
                                                .size(12.5)
                                                .color(TEXT_DIM),
                                            );
                                        });
                                    });
                            }
                            for (i, it) in found.iter().enumerate() {
                                let resp =
                                    ui.allocate_response(vec2(w - 12.0, 36.0), Sense::click());
                                let r = resp.rect;
                                let on = i == p.sel;
                                if resp.hovered()
                                    && ui.input(|inp| inp.pointer.delta().length() > 0.0)
                                {
                                    p.sel = i;
                                }
                                let pnt = ui.painter();
                                if on {
                                    pnt.rect_filled(r, CornerRadius::same(11), SOFT_HOVER);
                                }
                                let ib = Rect::from_min_size(
                                    pos2(r.left() + 9.0, r.center().y - 12.0),
                                    vec2(24.0, 24.0),
                                );
                                let thumb: Option<TextureId> =
                                    it.thumb.as_ref().and_then(|t| s.previews.thumb(t));
                                match thumb {
                                    Some(t) => {
                                        egui::Image::new((t, ib.size()))
                                            .corner_radius(CornerRadius::same(6))
                                            .paint_at(ui, ib);
                                    }
                                    None => {
                                        ui.painter().rect_filled(
                                            ib,
                                            CornerRadius::same(8),
                                            if on { Color32::WHITE } else { SOFT },
                                        );
                                        paint_icon(
                                            ui,
                                            ib.center(),
                                            it.icon,
                                            14.0,
                                            if on { ON_ACCENT } else { TEXT_DIM },
                                        );
                                    }
                                }
                                let pnt = ui.painter();
                                let g = pnt.layout_no_wrap(
                                    it.name.clone(),
                                    FontId::proportional(13.0),
                                    TEXT,
                                );
                                let x = ib.right() + 10.0;
                                let gw = g.size().x;
                                pnt.galley(pos2(x, r.center().y - g.size().y * 0.5), g, TEXT);
                                if !it.sub.is_empty() {
                                    let sg = pnt.layout_no_wrap(
                                        it.sub.clone(),
                                        FontId::proportional(12.5),
                                        TEXT_MUTE,
                                    );
                                    pnt.galley(
                                        pos2(x + gw + 8.0, r.center().y - sg.size().y * 0.5),
                                        sg,
                                        TEXT_MUTE,
                                    );
                                }
                                let mut right = r.right() - 9.0;
                                if !it.key.is_empty() {
                                    let kg = pnt.layout_no_wrap(
                                        it.key.to_string(),
                                        FontId::new(10.5, mono()),
                                        TEXT_DIM,
                                    );
                                    let kr = Rect::from_min_size(
                                        pos2(
                                            right - kg.size().x - 8.0,
                                            r.center().y - kg.size().y * 0.5 - 2.0,
                                        ),
                                        kg.size() + vec2(8.0, 4.0),
                                    );
                                    pnt.rect_stroke(
                                        kr,
                                        CornerRadius::same(4),
                                        Stroke::new(1.0, LINE_STRONG),
                                        egui::StrokeKind::Inside,
                                    );
                                    pnt.galley(kr.min + vec2(4.0, 2.0), kg, TEXT_DIM);
                                    right = kr.left() - 10.0;
                                }
                                let kind = pnt.layout_no_wrap(
                                    it.kind.to_string(),
                                    FontId::proportional(12.5),
                                    TEXT_MUTE,
                                );
                                pnt.galley(
                                    pos2(right - kind.size().x, r.center().y - kind.size().y * 0.5),
                                    kind,
                                    TEXT_MUTE,
                                );
                                if resp
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    chosen = Some(i);
                                }
                            }
                        });
                    let (lr, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
                    ui.painter().rect_filled(lr, 0.0, LINE);
                    egui::Frame::new()
                        .inner_margin(Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                kbd(ui, "↑↓");
                                ui.label(RichText::new("move").size(12.0).color(TEXT_MUTE));
                                ui.add_space(10.0);
                                kbd(ui, "↵");
                                ui.label(RichText::new("run").size(12.0).color(TEXT_MUTE));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new("describe something to prompt the AI")
                                                .size(12.0)
                                                .color(TEXT_MUTE),
                                        );
                                        let (r, _) = ui
                                            .allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                                        paint_icon(ui, r.center(), "SparklesTwo", 12.0, TEXT_MUTE);
                                    },
                                );
                            });
                        });
                });
        });
    let mut close = esc || scrim.inner;
    if let Some(i) = chosen
        && let Some(it) = found.into_iter().nth(i)
    {
        run(it.act, s, out, ctx);
        close = true;
    }
    if prompt_words {
        // Prompt an area with the words typed
        s.prompt.session.input = p.query.trim().to_string();
        crate::prompt::button_clicked(&mut s.prompt);
        close = true;
    }
    if close {
        store(ctx, Pal::default());
    } else {
        store(ctx, p);
    }
}
