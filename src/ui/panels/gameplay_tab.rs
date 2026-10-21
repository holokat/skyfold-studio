//! Gameplay tab in the Halo layout (HANDOFF › Per-tab mapping): on the left the 9 tools (select,
//! player start, points of interest, NPCs, spawn regions, resource nodes, ore, scatter, remove)
//! with their options, "In this world" (counts, warnings, clickable lists), what shows in the
//! viewport and the server-data export; the inspector appears for the selected pin, region or
//! node (or the creature library), and the dock carries the tool, its mode and its key hints.
//! Inspector edits are recorded as one undo step per widget interaction. The Ore tool and the
//! deposit-area inspector live in `ore_ui`.

pub(super) mod ore_ui;

use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, Sense, Stroke, pos2, vec2};

use super::super::UiState;
use super::super::theme::*;
use super::super::widgets::*;
use super::meters;
use crate::gameplay::edit::{self, Ground, level_label, levels_of};
use crate::gameplay::markers::{
    AREA_ICON, NPC_COLOR, POI_COLOR, SPAWN_ICON, START_COLOR, START_ICON, area_color, node_color,
    node_icon, npc_icon, poi_icon, spawn_color,
};
use crate::gameplay::scatter::{self, SCATTER_KINDS};
use crate::gameplay::{
    Aggression, Gameplay, GameplaySel, GameplayTool, NpcRole, OreMode, PoiKind, ResourceKind,
    SpawnBehaviour, SpawnEntry,
};

/// The creature library is open in the inspector (the "Creature library" buttons; the Spawns
/// tool shows it anyway while nothing is selected).
static LIBRARY: AtomicBool = AtomicBool::new(false);

fn library_flag() -> bool {
    LIBRARY.load(Ordering::Relaxed)
}

fn set_library(on: bool) {
    LIBRARY.store(on, Ordering::Relaxed);
}

/// The library shows in the inspector while nothing is selected.
fn library_shown(s: &UiState) -> bool {
    library_flag() || s.gp_ed.tool == GameplayTool::Spawn
}

fn aggression_color(a: Aggression) -> Color32 {
    spawn_color(Some(a))
}

const TOOLS: [GameplayTool; 9] = [
    GameplayTool::Select,
    GameplayTool::PlayerStart,
    GameplayTool::Poi,
    GameplayTool::Npc,
    GameplayTool::Spawn,
    GameplayTool::Node,
    GameplayTool::Ore,
    GameplayTool::Scatter,
    GameplayTool::Remove,
];

fn tool_icon(t: GameplayTool) -> &'static str {
    match t {
        GameplayTool::Select => "Cursor1",
        GameplayTool::PlayerStart => START_ICON,
        GameplayTool::Poi => "MapPin",
        GameplayTool::Npc => "User",
        GameplayTool::Spawn => SPAWN_ICON,
        GameplayTool::Node => "Grass",
        GameplayTool::Ore => "Diamond",
        GameplayTool::Scatter => "Shuffle",
        GameplayTool::Remove => "Eraser",
    }
}

fn tool_name(t: GameplayTool) -> &'static str {
    match t {
        GameplayTool::Select => "Select",
        GameplayTool::PlayerStart => "Start",
        GameplayTool::Poi => "Places",
        GameplayTool::Npc => "NPCs",
        GameplayTool::Spawn => "Spawns",
        GameplayTool::Node => "Nodes",
        GameplayTool::Ore => "Ore",
        GameplayTool::Scatter => "Scatter",
        GameplayTool::Remove => "Remove",
    }
}

fn tool_hint(t: GameplayTool) -> &'static str {
    match t {
        GameplayTool::Select => {
            "Click a pin or a spawn region to edit it; drag to move. Delete removes, Ctrl/⌘ D duplicates."
        }
        GameplayTool::PlayerStart => {
            "Click where new players enter the world; keep the button down and drag to turn them."
        }
        GameplayTool::Poi => "Pick a kind, then click to place it. Drag a pin to move it.",
        GameplayTool::Npc => {
            "Pick a role and click to place; drag to turn them. NPCs join the nearest point of interest within 60 m."
        }
        GameplayTool::Spawn => {
            "Creatures live in spawn regions: pick who lives there, then outline the area on the ground."
        }
        GameplayTool::Node => {
            "Click to place a gathering node. The tier follows the zone's levels unless you set one."
        }
        GameplayTool::Ore => {
            "Mineable ore deposits: pick an ore, then click the ground for one deposit or draw an area that fills itself."
        }
        GameplayTool::Scatter => {
            "Fill the world with gathering nodes where they make sense, in one click."
        }
        GameplayTool::Remove => "Click a pin or a spawn region to remove it.",
    }
}

// ------------------------------------------------------------------ small Halo pieces (private)

/// A pill with a colour dot (creature and resource pickers); white when on.
fn dot_chip(ui: &mut egui::Ui, label: &str, col: Color32, on: bool) -> egui::Response {
    let fg = if on { ON_ACCENT } else { TEXT_DIM };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), FontId::proportional(12.0), fg);
    let size = vec2(galley.size().x + 30.0, 24.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if on {
            p.rect_filled(rect, CornerRadius::same(255), Color32::WHITE);
        } else {
            p.rect_filled(
                rect,
                CornerRadius::same(255),
                if resp.hovered() {
                    SOFT
                } else {
                    Color32::TRANSPARENT
                },
            );
            p.rect_stroke(
                rect,
                CornerRadius::same(255),
                Stroke::new(1.0, LINE),
                egui::StrokeKind::Inside,
            );
        }
        p.circle_filled(rect.left_center() + vec2(12.0, 0.0), 4.0, col);
        let fg = if !on && resp.hovered() { TEXT } else { fg };
        p.galley(
            rect.left_center() + vec2(22.0, -galley.size().y * 0.5),
            galley,
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn secs_label(v: f32) -> String {
    if v < 90.0 {
        format!("{v:.0} s")
    } else if v < 5400.0 {
        format!("{:.0} min", v / 60.0)
    } else {
        format!("{:.1} h", v / 3600.0)
    }
}

fn compass(deg: f32) -> &'static str {
    // 0° = east, counter-clockwise (90° = north)
    let dirs = ["E", "NE", "N", "NW", "W", "SW", "S", "SE"];
    dirs[(((deg.rem_euclid(360.0)) + 22.5) / 45.0) as usize % 8]
}

/// Moves the camera to look at an item.
fn focus(s: &mut UiState, sel: GameplaySel) {
    let Some(a) = s.gameplay.anchor(sel) else {
        return;
    };
    let d = match sel {
        GameplaySel::Spawn(_) | GameplaySel::OreArea(_) => {
            crate::gameplay::markers::region_outline(&s.gameplay, sel)
                .map(|poly| {
                    let (lo, hi) = crate::gameplay::SpawnRegion::bounds_of(poly);
                    ((hi - lo).max_element() * 1.7).max(if matches!(sel, GameplaySel::OreArea(_)) {
                        90.0
                    } else {
                        220.0
                    })
                })
                .unwrap_or(400.0)
        }
        _ => s.orbit.distance.clamp(90.0, 420.0),
    };
    let y = s.terrain.height_at(a.x, a.z);
    let (yaw, pitch) = (s.orbit.yaw, s.orbit.pitch.max(0.45));
    s.orbit.set_view(Vec3::new(a.x, y, a.z), yaw, pitch, d);
}

fn sel_icon(gp: &Gameplay, sel: GameplaySel) -> (&'static str, Color32) {
    match sel {
        GameplaySel::PlayerStart => (START_ICON, START_COLOR),
        GameplaySel::Poi(id) => (
            gp.pois
                .iter()
                .find(|q| q.id == id)
                .map(|q| poi_icon(q.kind))
                .unwrap_or("MapPin"),
            POI_COLOR,
        ),
        GameplaySel::Npc(id) => (
            gp.npcs
                .iter()
                .find(|q| q.id == id)
                .map(|q| npc_icon(q.role))
                .unwrap_or("User"),
            NPC_COLOR,
        ),
        GameplaySel::Spawn(id) => (
            SPAWN_ICON,
            gp.spawns
                .iter()
                .find(|q| q.id == id)
                .map(|r| spawn_color(r.aggression(gp)))
                .unwrap_or(TEXT),
        ),
        GameplaySel::Node(id) => gp
            .nodes
            .iter()
            .find(|q| q.id == id)
            .map(|q| {
                (
                    if q.ore().is_some() {
                        crate::gameplay::markers::ORE_ICON
                    } else {
                        node_icon(q.kind)
                    },
                    q.ore()
                        .map(crate::gameplay::markers::rarity_color)
                        .unwrap_or(node_color(q.kind)),
                )
            })
            .unwrap_or(("Diamond", TEXT)),
        GameplaySel::OreArea(id) => (
            AREA_ICON,
            gp.ore_areas
                .iter()
                .find(|q| q.id == id)
                .map(area_color)
                .unwrap_or(TEXT),
        ),
    }
}

/// The inspector's title for a selection.
fn sel_title(gp: &Gameplay, sel: GameplaySel) -> &'static str {
    match sel {
        GameplaySel::PlayerStart => "Player Start",
        GameplaySel::Poi(_) => "Place",
        GameplaySel::Npc(_) => "NPC",
        GameplaySel::Spawn(_) => "Spawn region",
        GameplaySel::OreArea(_) => "Deposit area",
        GameplaySel::Node(id) => {
            if gp
                .nodes
                .iter()
                .find(|n| n.id == id)
                .is_some_and(|n| n.ore().is_some())
            {
                "Ore deposit"
            } else {
                "Resource node"
            }
        }
    }
}

/// A clickable list row: the icon in the item's colour, name, mono detail on the right.
fn item_row(
    ui: &mut egui::Ui,
    ic: &str,
    col: Color32,
    name: &str,
    detail: &str,
    selected: bool,
) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, CornerRadius::same(R_INPUT), SOFT_HOVER);
        } else if resp.hovered() {
            p.rect_filled(rect, CornerRadius::same(R_INPUT), SOFT);
        }
        paint_icon(ui, rect.left_center() + vec2(15.0, 0.0), ic, 15.0, col);
        let p = ui.painter();
        let dg = p.layout_no_wrap(detail.to_string(), FontId::new(11.0, mono()), TEXT_MUTE);
        let name_w = (w - 32.0 - 10.0 - dg.size().x).max(40.0);
        let mut job = egui::text::LayoutJob::simple_singleline(
            name.to_string(),
            FontId::new(13.0, medium()),
            TEXT,
        );
        job.wrap.max_width = name_w;
        job.wrap.max_rows = 1;
        let ng = p.layout_job(job);
        p.galley(
            pos2(rect.left() + 30.0, rect.center().y - ng.size().y * 0.5),
            ng,
            TEXT,
        );
        p.galley(
            pos2(
                rect.right() - 8.0 - dg.size().x,
                rect.center().y - dg.size().y * 0.5,
            ),
            dg,
            TEXT_MUTE,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small collapsible group inside a section (chevron, title, muted count).
fn sub_fold(
    ui: &mut egui::Ui,
    id: &str,
    title: &str,
    note: &str,
    default_open: bool,
    title_col: Color32,
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(("gp_sub", id));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    let col = if resp.hovered() { TEXT } else { title_col };
    let p = ui.painter();
    chevron(
        p,
        pos2(rect.left() + 5.0, rect.center().y),
        3.2,
        if open { 0.0 } else { -90.0 },
        TEXT_DIM,
    );
    let tg = p.layout_no_wrap(title.to_string(), FontId::new(12.5, medium()), col);
    let tw = tg.size().x;
    p.galley(
        pos2(rect.left() + 16.0, rect.center().y - tg.size().y * 0.5),
        tg,
        col,
    );
    if !note.is_empty() {
        let ng = p.layout_no_wrap(note.to_string(), FontId::new(11.0, mono()), TEXT_MUTE);
        p.galley(
            pos2(rect.left() + 22.0 + tw, rect.center().y - ng.size().y * 0.5),
            ng,
            TEXT_MUTE,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if open {
        body(ui);
    }
}

/// An icon + text line in a colour (ok / warning lines).
fn icon_line(ui: &mut egui::Ui, ic: &str, col: Color32, text: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        crate::ui::icons::icon(ui, ic, 15.0, col);
        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(col)).wrap());
    })
    .response
}

/// The dark input well of fields.
fn well<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(R_INPUT))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A labelled one-line text field (label above, dark well).
fn text_field(ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str) -> egui::Response {
    if !label.is_empty() {
        ui.label(RichText::new(label).size(12.5).color(TEXT_DIM));
    }
    well(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(text)
                .frame(egui::Frame::NONE)
                .hint_text(RichText::new(hint).color(TEXT_MUTE))
                .desired_width(f32::INFINITY),
        )
    })
}

fn name_field(ui: &mut egui::Ui, name: &mut String) {
    text_field(ui, "Name", name, "");
}

fn notes_field(ui: &mut egui::Ui, notes: &mut String) {
    ui.label(RichText::new("Notes").size(12.5).color(TEXT_DIM));
    well(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(notes)
                .frame(egui::Frame::NONE)
                .hint_text(RichText::new("Lore, quest ideas, server notes…").color(TEXT_MUTE))
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        )
    });
}

/// Label left (dim), the widgets right-aligned.
fn num_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
}

/// A [min, max] level range (kept in order).
fn level_range(ui: &mut egui::Ui, label: &str, l: &mut [u8; 2]) {
    let before = *l;
    num_row(ui, label, |ui| {
        ui.add(egui::DragValue::new(&mut l[1]).range(1..=255).speed(0.1));
        ui.label(RichText::new("to").color(TEXT_MUTE));
        ui.add(egui::DragValue::new(&mut l[0]).range(1..=255).speed(0.1));
    });
    if l[1] < l[0] {
        if l[0] != before[0] {
            l[1] = l[0]
        } else {
            l[0] = l[1]
        }
    }
}

fn zone_text(g: &Ground, p: Vec2) -> String {
    match g.zone(p) {
        Some(z) => format!("{} · {}", z.name, level_label(levels_of(z))),
        None => "No zone (wilderness)".into(),
    }
}

/// A beacon-style badge: dark glass disc, colour ring, the icon in the colour.
fn badge(ui: &mut egui::Ui, ic: &str, col: Color32, d: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(d, d), Sense::hover());
    let p = ui.painter();
    p.circle_filled(
        r.center(),
        d * 0.5,
        Color32::from_rgba_unmultiplied(12, 15, 21, 204),
    );
    p.circle_stroke(r.center(), d * 0.5 - 0.8, Stroke::new(1.6, col));
    p.circle_stroke(
        r.center(),
        d * 0.5 - 3.0,
        Stroke::new(1.0, Color32::from_white_alpha(20)),
    );
    let f = |v: u8| (v as f32 + (255.0 - v as f32) * 0.25) as u8;
    paint_icon(
        ui,
        r.center(),
        ic,
        d * 0.46,
        Color32::from_rgb(f(col.r()), f(col.g()), f(col.b())),
    );
}

/// The inspector's hero: badge, name and two muted lines.
fn hero(ui: &mut egui::Ui, ic: &str, col: Color32, name: &str, line1: &str, line2: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        badge(ui, ic, col, 46.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add(
                egui::Label::new(
                    RichText::new(name)
                        .font(FontId::new(15.0, semibold()))
                        .color(TEXT),
                )
                .truncate(),
            );
            if !line1.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(line1).size(12.0).color(TEXT_MUTE)).truncate(),
                );
            }
            if !line2.is_empty() {
                ui.add(
                    egui::Label::new(RichText::new(line2).size(12.0).color(TEXT_MUTE)).truncate(),
                );
            }
        });
    });
    ui.add_space(2.0);
}

/// A dropdown with icons (kinds, roles, resources).
fn icon_dropdown<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    label: &str,
    id: &str,
    cur: &mut T,
    all: &[T],
    look: impl Fn(T) -> (&'static str, &'static str, &'static str),
) {
    let (_, name, _) = look(*cur);
    dropdown_row(ui, label, id, name, |ui| {
        for k in all {
            let (ic, n, tipt) = look(*k);
            if menu_row(ui, ic, n, "", *cur == *k)
                .on_hover_text(tipt)
                .clicked()
            {
                *cur = *k;
            }
        }
    });
}

// ------------------------------------------------------------------ left panel

pub fn gameplay_left(ui: &mut egui::Ui, s: &mut UiState) {
    let items: Vec<(&str, &str, &str)> = TOOLS
        .iter()
        .map(|t| (tool_icon(*t), tool_name(*t), ""))
        .collect();
    let sel = TOOLS.iter().position(|t| *t == s.gp_ed.tool);
    if let Some(i) = tool_grid(ui, 3, &items, sel) {
        s.gp_ed.tool = TOOLS[i];
    }
    world_progression(ui, s);
    dim(ui, tool_hint(s.gp_ed.tool));
    match s.gp_ed.tool {
        GameplayTool::Select | GameplayTool::Remove => {}
        GameplayTool::PlayerStart => {
            section_fold(ui, "gp_start", "Player start", "", |ui| {
                match s.gameplay.player_start {
                    Some(p) => {
                        let deg = p.yaw.to_degrees();
                        stat_row(
                            ui,
                            "Position",
                            &format!("{:.0}, {:.0} · facing {}", p.pos[0], p.pos[2], compass(deg)),
                        );
                        if small_icon_button(ui, "Target", "Show")
                            .on_hover_text("Select it and look at it")
                            .clicked()
                        {
                            s.gameplay.selected = Some(GameplaySel::PlayerStart);
                            focus(s, GameplaySel::PlayerStart);
                        }
                    }
                    None => dim(
                        ui,
                        "Not set yet. Every world needs one: it's where new characters appear.",
                    ),
                }
            });
        }
        GameplayTool::Poi => {
            section_fold(
                ui,
                "gp_kind",
                "Kind",
                &PoiKind::ALL.len().to_string(),
                |ui| {
                    let items: Vec<(&str, &str, &str)> = PoiKind::ALL
                        .iter()
                        .map(|k| (poi_icon(*k), k.name(), k.hint()))
                        .collect();
                    let sel = PoiKind::ALL.iter().position(|k| *k == s.gp_ed.poi_kind);
                    if let Some(i) = tool_grid(ui, 2, &items, sel) {
                        s.gp_ed.poi_kind = PoiKind::ALL[i];
                    }
                    hint_small(
                        ui,
                        &format!(
                            "{}: {}. Discovery radius {:.0} m.",
                            s.gp_ed.poi_kind.name(),
                            s.gp_ed.poi_kind.hint(),
                            s.gp_ed.poi_kind.default_radius()
                        ),
                    );
                },
            );
        }
        GameplayTool::Npc => {
            section_fold(
                ui,
                "gp_role",
                "Role",
                &NpcRole::ALL.len().to_string(),
                |ui| {
                    let items: Vec<(&str, &str, &str)> = NpcRole::ALL
                        .iter()
                        .map(|r| (npc_icon(*r), r.name(), r.hint()))
                        .collect();
                    let sel = NpcRole::ALL.iter().position(|r| *r == s.gp_ed.npc_role);
                    if let Some(i) = tool_grid(ui, 2, &items, sel) {
                        s.gp_ed.npc_role = NpcRole::ALL[i];
                    }
                },
            );
        }
        GameplayTool::Spawn => spawn_tool(ui, s),
        GameplayTool::Node => {
            section_fold(ui, "gp_resource", "Resource", "", |ui| {
                let items: Vec<(&str, &str, &str)> = ResourceKind::ALL
                    .iter()
                    .map(|k| (node_icon(*k), k.name(), k.hint()))
                    .collect();
                let sel = ResourceKind::ALL
                    .iter()
                    .position(|k| *k == s.gp_ed.node_kind);
                if let Some(i) = tool_grid(ui, 3, &items, sel) {
                    s.gp_ed.node_kind = ResourceKind::ALL[i];
                    s.gp_ed.node_variant.clear();
                }
                let mut tier = s.gp_ed.node_tier as f32;
                slider_row(ui, "Tier", &mut tier, 0.0..=8.0, |v| {
                    if v < 0.5 {
                        "from the zone".into()
                    } else {
                        format!("{v:.0}")
                    }
                });
                s.gp_ed.node_tier = tier.round() as u8;
                text_field(ui, "Variant", &mut s.gp_ed.node_variant, "automatic");
                hint_small(
                    ui,
                    "Automatic variants follow the tier and biome (Copper Vein, Silverleaf, Old Oak…).",
                );
            });
        }
        GameplayTool::Ore => ore_ui::ore_tool(ui, s),
        GameplayTool::Scatter => scatter_tool(ui, s),
    }
    in_this_world(ui, s);
    section_fold(ui, "gp_show", "Show", "", |ui| {
        let v = &mut s.gp_ed.view;
        egui::Grid::new("gp_show")
            .num_columns(2)
            .spacing(vec2(14.0, 0.0))
            .show(ui, |ui| {
                toggle(ui, &mut v.pois, "Places & start");
                toggle(ui, &mut v.npcs, "NPCs");
                ui.end_row();
                toggle(ui, &mut v.spawns, "Spawn regions");
                toggle(ui, &mut v.nodes, "Resources");
                ui.end_row();
            });
        toggle(ui, &mut v.labels, "Names");
        toggle(ui, &mut v.all_tabs, "Pins in every tab").on_hover_text(
            "Keep the gameplay pins visible while sculpting, painting or placing objects",
        );
        toggle(ui, &mut v.walk, "Pins in walk mode");
    });
    ui.add_space(2.0);
    if wide_icon_button(ui, "Server", "Export server data…")
        .on_hover_text("gameplay.json for your game server: start, places, NPCs, spawns with levels and counts, creatures, resources and fishing spots. (File › Export writes it too.)")
        .clicked()
    {
        export_dialog(s);
    }
}

/// The world-level settings which guide progression, kept together with the player start rather
/// than buried in an individual zone. A missing cap intentionally remains missing for legacy
/// worlds until the designer changes the field.
fn world_progression(ui: &mut egui::Ui, s: &mut UiState) {
    section_fold(ui, "gp_progression", "World progression", "", |ui| {
        let start = if s.gameplay.player_start.is_some() {
            "Set"
        } else {
            "Not set"
        };
        stat_row(ui, "Starting point", start);
        if small_icon_button(ui, START_ICON, "Set starting point")
            .on_hover_text("Choose the Player start tool and click the ground")
            .clicked()
        {
            s.gp_ed.tool = GameplayTool::PlayerStart;
        }

        if let Some(current) = s.gameplay.level_cap {
            let mut cap = current;
            let mut changed = false;
            num_row(ui, "Level cap", |ui| {
                changed = ui
                    .add(
                        egui::DragValue::new(&mut cap)
                            .range(1..=200)
                            .speed(0.25)
                            .suffix(" levels"),
                    )
                    .changed();
            });
            if changed {
                if !s.gp_ed.editing {
                    let before = s.gameplay.clone();
                    s.history.begin("Set level cap");
                    s.history.touch_gameplay(&before);
                    s.gp_ed.editing = true;
                }
                s.gameplay.level_cap = Some(cap);
                s.gameplay.dirty = true;
                s.meta.dirty = true;
            }
            if s.gp_ed.editing
                && !ui.ctx().egui_is_using_pointer()
                && !ui.ctx().egui_wants_keyboard_input()
            {
                s.history.commit(&s.store);
                s.gp_ed.editing = false;
            }
        } else {
            let id = ui.make_persistent_id("gp_level_cap_draft");
            let mut cap = ui.data_mut(|d| *d.get_persisted_mut_or(id, 60_u32));
            let mut set = false;
            num_row(ui, "Level cap", |ui| {
                set = small_button(ui, "Set level cap").clicked();
                ui.add(egui::DragValue::new(&mut cap).range(1..=200).speed(0.25));
            });
            ui.data_mut(|d| d.insert_persisted(id, cap));
            hint_small(
                ui,
                "Not set. Choose the highest planned level before generating zones automatically.",
            );
            if set {
                let before = s.gameplay.clone();
                s.history.begin("Set level cap");
                s.history.touch_gameplay(&before);
                s.gameplay.level_cap = Some(cap);
                s.gameplay.dirty = true;
                s.meta.dirty = true;
                s.history.commit(&s.store);
            }
        }
    });
}

pub(super) fn export_dialog(s: &mut UiState) {
    let default = crate::export::exports_dir();
    let _ = std::fs::create_dir_all(&default);
    crate::dialogs::pick_folder(
        "server_export",
        rfd::AsyncFileDialog::new()
            .set_title("Export server data to folder")
            .set_directory(&default),
    );
    let _ = s;
}

/// Writes the server data once the folder dialog answered.
pub(super) fn finish_export(s: &mut UiState) {
    let Some(dir) = crate::dialogs::take_file("server_export") else {
        return;
    };
    match crate::gameplay::write_server_data(
        &dir,
        &s.gameplay,
        &s.terrain,
        &s.zones,
        &s.water,
        &s.meta.name,
    ) {
        Ok(p) => s.ed.toast(format!("Wrote {}", p.display())),
        Err(e) => s.ed.toast(format!("Export failed: {e}")),
    }
}

/// "In this world": the counts and the layer's warnings (click one to go there); with the Select
/// tool also everything placed, by kind (click to select and look at it).
fn in_this_world(ui: &mut egui::Ui, s: &mut UiState) {
    // the layer's warnings, refreshed twice a second
    let now = s.real_time.elapsed_secs();
    if now - s.gp_ed.warnings_at > 0.5 {
        let g = Ground {
            terrain: &s.terrain,
            zones: &s.zones,
            water: &s.water,
            nature: None,
        };
        s.gp_ed.warnings = edit::all_warnings(&s.gameplay, &g);
        s.gp_ed.warnings_at = now;
    }
    let mut pick = None;
    let mut library = false;
    let selecting = s.gp_ed.tool == GameplayTool::Select;
    section_fold(ui, "gp_world", "In this world", "", |ui| {
        let gp = &s.gameplay;
        let empty = gp.player_start.is_none()
            && gp.pois.is_empty()
            && gp.npcs.is_empty()
            && gp.spawns.is_empty()
            && gp.nodes.is_empty()
            && gp.ore_areas.is_empty();
        let start = match gp.player_start {
            Some(p) => format!(
                "{:.0}, {:.0} · facing {}",
                p.pos[0],
                p.pos[2],
                compass(p.yaw.to_degrees())
            ),
            None => "not set".into(),
        };
        stat_row(ui, "Player start", &start);
        stat_row(ui, "Points of interest", &gp.pois.len().to_string());
        stat_row(ui, "NPCs", &gp.npcs.len().to_string());
        stat_row(ui, "Spawn regions", &gp.spawns.len().to_string());
        stat_row(ui, "Creatures", &gp.creatures.len().to_string());
        let deposits = gp.nodes.iter().filter(|n| n.ore().is_some()).count();
        if !gp.ore_areas.is_empty() {
            let in_areas = gp.nodes.iter().filter(|n| n.area.is_some()).count();
            stat_row(
                ui,
                "Ore deposit areas",
                &format!("{} · {in_areas} deposits", gp.ore_areas.len()),
            );
        }
        stat_row(
            ui,
            "Resource nodes",
            &if deposits > 0 {
                format!("{} · {deposits} ore", gp.nodes.len())
            } else {
                gp.nodes.len().to_string()
            },
        );
        if !gp.nodes.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(5.0, 2.0);
                for k in ResourceKind::ALL {
                    let n = gp.nodes.iter().filter(|x| x.kind == k).count();
                    if n > 0 {
                        crate::ui::icons::icon(ui, node_icon(k), 13.0, node_color(k));
                        ui.label(
                            RichText::new(format!("{} {n}", k.name()))
                                .size(11.5)
                                .color(TEXT_DIM),
                        );
                        ui.add_space(4.0);
                    }
                }
            });
        }
        // warnings
        if gp.player_start.is_none() && (!gp.pois.is_empty() || !gp.spawns.is_empty()) {
            icon_line(ui, "ExclamationTriangle", WARN, "No player start yet");
        }
        let n = s.gp_ed.warnings.len();
        if n == 0 {
            if !gp.pois.is_empty() || !gp.spawns.is_empty() || !gp.nodes.is_empty() {
                icon_line(ui, "CheckCircle2", OK, "No placement problems");
            }
        } else {
            sub_fold(
                ui,
                "warnings",
                &format!("{n} warning{}", if n == 1 { "" } else { "s" }),
                "",
                n <= 4,
                WARN,
                |ui| {
                    for (sel, text) in s.gp_ed.warnings.iter().take(40) {
                        let (ic, col) = sel_icon(gp, *sel);
                        let r = ui
                            .horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                crate::ui::icons::icon(ui, ic, 13.0, col);
                                ui.label(
                                    RichText::new(format!("{}: {text}", edit::sel_label(gp, *sel)))
                                        .size(12.0)
                                        .color(TEXT),
                                );
                            })
                            .response
                            .interact(Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clicked() {
                            pick = Some(*sel);
                        }
                    }
                },
            );
        }
        if !selecting {
            return;
        }
        // everything placed, by kind
        ui.add_space(2.0);
        if empty {
            hint_small(
                ui,
                "Nothing yet. Start with the player start, then the first quest hub, a few spawn regions and Scatter for resources.",
            );
        }
        if gp.player_start.is_some()
            && item_row(
                ui,
                START_ICON,
                START_COLOR,
                "Player Start",
                "",
                gp.selected == Some(GameplaySel::PlayerStart),
            )
            .clicked()
        {
            pick = Some(GameplaySel::PlayerStart);
        }
        let g = Ground {
            terrain: &s.terrain,
            zones: &s.zones,
            water: &s.water,
            nature: None,
        };
        if !gp.pois.is_empty() {
            sub_fold(
                ui,
                "pois",
                "Points of interest",
                &gp.pois.len().to_string(),
                true,
                TEXT,
                |ui| {
                    for q in &gp.pois {
                        let lv = q
                            .level
                            .or_else(|| g.zone_levels(Vec2::new(q.pos[0], q.pos[2])))
                            .map(level_label)
                            .unwrap_or_default();
                        if item_row(
                            ui,
                            poi_icon(q.kind),
                            POI_COLOR,
                            &q.name,
                            &lv,
                            gp.selected == Some(GameplaySel::Poi(q.id)),
                        )
                        .clicked()
                        {
                            pick = Some(GameplaySel::Poi(q.id));
                        }
                    }
                },
            );
        }
        if !gp.npcs.is_empty() {
            sub_fold(
                ui,
                "npcs",
                "NPCs",
                &gp.npcs.len().to_string(),
                gp.npcs.len() <= 8,
                TEXT,
                |ui| {
                    for q in &gp.npcs {
                        if item_row(
                            ui,
                            npc_icon(q.role),
                            NPC_COLOR,
                            &q.name,
                            q.role.name(),
                            gp.selected == Some(GameplaySel::Npc(q.id)),
                        )
                        .clicked()
                        {
                            pick = Some(GameplaySel::Npc(q.id));
                        }
                    }
                },
            );
        }
        if !gp.spawns.is_empty() {
            sub_fold(
                ui,
                "spawns",
                "Spawn regions",
                &gp.spawns.len().to_string(),
                true,
                TEXT,
                |ui| {
                    for r in &gp.spawns {
                        if item_row(
                            ui,
                            SPAWN_ICON,
                            spawn_color(r.aggression(gp)),
                            &r.name,
                            &level_label(r.level),
                            gp.selected == Some(GameplaySel::Spawn(r.id)),
                        )
                        .clicked()
                        {
                            pick = Some(GameplaySel::Spawn(r.id));
                        }
                    }
                },
            );
        }
        if !gp.ore_areas.is_empty() {
            sub_fold(
                ui,
                "areas",
                "Ore deposit areas",
                &gp.ore_areas.len().to_string(),
                true,
                TEXT,
                |ui| {
                    for a in &gp.ore_areas {
                        let n = gp.nodes.iter().filter(|n| n.area == Some(a.id)).count();
                        if item_row(
                            ui,
                            AREA_ICON,
                            area_color(a),
                            &a.name,
                            &format!("{n} deposits"),
                            gp.selected == Some(GameplaySel::OreArea(a.id)),
                        )
                        .on_hover_text(a.mix_label())
                        .clicked()
                        {
                            pick = Some(GameplaySel::OreArea(a.id));
                        }
                    }
                },
            );
        }
        let open = library_flag() && gp.selected.is_none();
        if item_row(
            ui,
            "Library",
            TEXT_DIM,
            "Creature library",
            &gp.creatures.len().to_string(),
            open,
        )
        .on_hover_text("Creature types: open them in the inspector")
        .clicked()
        {
            library = true;
        }
    });
    if library {
        let on = !(library_flag() && s.gameplay.selected.is_none());
        set_library(on);
        if on {
            s.gameplay.selected = None;
        }
    }
    if let Some(sel) = pick {
        s.gameplay.selected = Some(sel);
        focus(s, sel);
    }
}

/// Finishes the spawn region being drawn (one undo step).
fn finish_spawn(s: &mut UiState) {
    let pts: Vec<Vec2> = s.gp_ed.draft.iter().map(|p| p.xz()).collect();
    let creatures = s.gp_ed.spawn_creatures.clone();
    let g = Ground {
        terrain: &s.terrain,
        zones: &s.zones,
        water: &s.water,
        nature: None,
    };
    s.history.begin("Draw spawn region");
    if crate::gameplay::finish_region(
        &mut s.gameplay,
        &g,
        &mut s.history,
        &mut s.ed,
        &creatures,
        &pts,
    ) {
        s.gp_ed.draft.clear();
        s.meta.dirty = true;
    }
    s.history.commit(&s.store);
}

fn spawn_tool(ui: &mut egui::Ui, s: &mut UiState) {
    if !s.gp_ed.draft.is_empty() {
        icon_line(
            ui,
            SPAWN_ICON,
            TEXT,
            &format!(
                "Drawing a spawn region: {} corners. Finish in the dock or with Enter; Esc cancels.",
                s.gp_ed.draft.len()
            ),
        );
    } else {
        crate::ui::tips::card(
            ui,
            "spawns",
            "How spawn regions work",
            &[
                "Pick the creatures that live there (below).",
                "Click corners around the area on the ground; Enter or a double-click finishes.",
                "They now spawn inside it. Click the region to set levels, numbers and respawn time.",
            ],
        );
    }
    let picked = s.gp_ed.spawn_creatures.len();
    section_fold(
        ui,
        "gp_spawn_pick",
        "1 · Pick creatures",
        &if picked > 0 {
            format!("{picked} picked")
        } else {
            String::new()
        },
        |ui| {
            ui.horizontal(|ui| {
                hint_small(ui, "What lives in the next region you draw.");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    crate::ui::tips::help_button(ui, "spawns")
                });
            });
            if s.gameplay.creatures.is_empty() {
                dim(
                    ui,
                    "Your creature library is empty. Start from a fantasy bestiary (you can edit it in the inspector) or create your own.",
                );
                if primary_icon_button(ui, SPAWN_ICON, "Add starter creatures").clicked() {
                    s.history.begin("Add starter creatures");
                    s.history.touch_gameplay(&s.gameplay);
                    edit::add_starter_creatures(&mut s.gameplay);
                    s.history.commit(&s.store);
                    s.meta.dirty = true;
                }
                return;
            }
            s.gp_ed
                .spawn_creatures
                .retain(|id| s.gameplay.creatures.iter().any(|c| c.id == *id));
            let mut flip = None;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for c in &s.gameplay.creatures {
                    let on = s.gp_ed.spawn_creatures.contains(&c.id);
                    let tip = format!(
                        "{} · {} · {}{}",
                        c.family,
                        level_label(c.level),
                        c.aggression.name(),
                        if c.elite { " · elite" } else { "" }
                    );
                    if dot_chip(ui, &c.name, aggression_color(c.aggression), on)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        flip = Some(c.id);
                    }
                }
            });
            if let Some(id) = flip {
                if let Some(i) = s.gp_ed.spawn_creatures.iter().position(|c| *c == id) {
                    s.gp_ed.spawn_creatures.remove(i);
                } else {
                    s.gp_ed.spawn_creatures.push(id);
                }
            }
            if s.gp_ed.spawn_creatures.is_empty() {
                hint_small(
                    ui,
                    "None picked: the region starts empty. Weights and levels can be changed afterwards.",
                );
            } else {
                hint_small(
                    ui,
                    "Weights and levels can be changed afterwards. Edit the creatures in the library (inspector).",
                );
            }
        },
    );
    if s.gp_ed.draft.is_empty() {
        section_fold(
            ui,
            "gp_spawn_outline",
            "2 · Outline it on the ground",
            "",
            |ui| {
                hint_small(
                    ui,
                    "Click corners around the area (or hold and draw a loop). Double-click, Enter or click the first corner to finish; Backspace drops a corner, Esc cancels.",
                );
            },
        );
    }
}

fn scatter_tool(ui: &mut egui::Ui, s: &mut UiState) {
    let mut run = false;
    let mut clear = false;
    let auto = s.gameplay.nodes.iter().filter(|n| n.auto).count();
    let hand = s.gameplay.nodes.len() - auto;
    section_fold(ui, "gp_scatter", "Scatter resources", "", |ui| {
        hint_small(
            ui,
            "Ore deposits on rocky slopes, peaks and by cliffs (ores of the zone's level that suit the ground) · herbs in meadows and forests · wood at grown trees · stone at boulders · crystals on high peaks. Never under water; tiers follow the zones' levels.",
        );
        let p = &mut s.gp_ed.scatter;
        egui::Grid::new("gp_scatter_kinds")
            .num_columns(2)
            .spacing(vec2(14.0, 0.0))
            .show(ui, |ui| {
                for (i, k) in SCATTER_KINDS.into_iter().enumerate() {
                    let mut on = p.kinds.contains(&k);
                    if toggle(ui, &mut on, k.name())
                        .on_hover_text(k.hint())
                        .changed()
                    {
                        if on {
                            p.kinds.push(k);
                        } else {
                            p.kinds.retain(|x| *x != k);
                        }
                    }
                    if i % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        slider_row(ui, "Density", &mut p.density, 0.1..=3.0, |v| {
            if (v - 1.0).abs() < 0.05 {
                "typical".into()
            } else {
                format!("{v:.1}×")
            }
        });
        num_row(ui, "Seed", |ui| {
            if icon_button(ui, "Dice5", "Another arrangement").clicked() {
                p.seed = fastrand::u32(0..999_999);
            }
            ui.add(egui::DragValue::new(&mut p.seed).range(0..=999_999));
        });
        let can = !p.kinds.is_empty();
        if ui
            .add_enabled_ui(can, |ui| {
                primary_icon_button(ui, "Shuffle", "Scatter resources")
            })
            .inner
            .on_disabled_hover_text("Switch on at least one kind")
            .clicked()
        {
            run = true;
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{auto} scattered · {hand} by hand"))
                    .size(11.5)
                    .color(TEXT_MUTE),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled_ui(auto > 0, |ui| small_icon_button(ui, "TrashCan", "Clear"))
                    .inner
                    .on_hover_text("Remove the scattered nodes (hand-placed ones stay)")
                    .clicked()
                {
                    clear = true;
                }
            });
        });
        hint_small(
            ui,
            "Scattering again replaces the scattered nodes of the chosen kinds; nodes you placed or edited stay.",
        );
    });
    if run {
        let t0 = std::time::Instant::now();
        let params = s.gp_ed.scatter.clone();
        let nature = match (s.lib.as_deref(), s.nature_rules.as_deref()) {
            (Some(lib), Some(rules)) => Some(crate::nature::Ctx {
                terrain: &s.terrain,
                water: &s.water,
                zones: &s.zones,
                lib,
                field: &s.nature,
                rules,
                masks: &s.nature_masks,
                amounts: crate::nature::amounts(&s.amounts),
                flora_amounts: s.amounts.flora_amounts(),
                appearance: *s.appearance,
                seed: s.meta.seed,
            }),
            _ => None,
        };
        let g = Ground {
            terrain: &s.terrain,
            zones: &s.zones,
            water: &s.water,
            nature,
        };
        s.history.begin("Scatter resources");
        s.history.touch_gameplay(&s.gameplay);
        let params = scatter::ScatterParams {
            ore: s.gameplay.ore_settings.clone(),
            ..params
        };
        let (added, removed) =
            scatter::apply(&mut s.gameplay, &params, &g, &s.store, s.lib.as_deref());
        s.history.commit(&s.store);
        s.meta.dirty = true;
        let per: Vec<String> = params
            .kinds
            .iter()
            .map(|k| {
                format!(
                    "{} {}",
                    s.gameplay
                        .nodes
                        .iter()
                        .filter(|n| n.auto && n.kind == *k)
                        .count(),
                    k.name().to_lowercase()
                )
            })
            .collect();
        info!(
            "scattered {added} resource nodes (replaced {removed}) in {:?}",
            t0.elapsed()
        );
        s.ed.toast(format!("Scattered {added} nodes: {}", per.join(", ")));
    }
    if clear {
        s.history.begin("Clear scattered resources");
        s.history.touch_gameplay(&s.gameplay);
        let n = scatter::clear(&mut s.gameplay);
        s.history.commit(&s.store);
        s.meta.dirty = true;
        s.ed.toast(format!("Removed {n} scattered nodes"));
    }
}

// ------------------------------------------------------------------ inspector

/// Buttons that change the layer beyond field edits (each its own undo step).
enum Action {
    Remove(GameplaySel),
    Duplicate(GameplaySel),
    Focus(GameplaySel),
    StarterCreatures,
    NewCreature,
    RemoveCreature(u32),
    /// Switch to the Spawns tool, with these creatures picked for the next region.
    DrawSpawn(Vec<u32>),
    /// Put a creature in a spawn region's encounter table: (region, creature).
    AddToRegion(u32, u32),
}

pub fn gameplay_right(ui: &mut egui::Ui, s: &mut UiState) {
    if s.gameplay
        .selected
        .is_some_and(|sel| !s.gameplay.exists(sel))
    {
        s.gameplay.selected = None;
    }
    s.gp_ed.inspector_drawn = true;
    let before = s.gameplay.clone();
    let mut action = None;
    // The creature library belongs with spawns: it shows with nothing selected (Spawns tool or the
    // library button) or under a spawn region (never under a place, an NPC or a node).
    match s.gameplay.selected {
        None => {
            if library_shown(s) {
                creature_library(ui, s, &mut action);
            }
        }
        Some(sel) => {
            inspector(ui, s, sel, &mut action);
            if matches!(sel, GameplaySel::Spawn(_)) {
                ui.add_space(4.0);
                let open = library_flag();
                let n = s.gameplay.creatures.len();
                if wide_icon_button(
                    ui,
                    "Library",
                    &if open {
                        "Hide the creature library".to_string()
                    } else {
                        format!("Creature library ({n})")
                    },
                )
                .clicked()
                {
                    set_library(!open);
                }
                if open {
                    section_fold(
                        ui,
                        "gp_lib_region",
                        &format!("Creature library ({n})"),
                        "",
                        |ui| creature_library(ui, s, &mut action),
                    );
                }
            }
        }
    }
    record_edits(ui, s, &before);
    if let Some(a) = action {
        perform(s, a);
    }
}

fn content_differs(a: &Gameplay, b: &Gameplay) -> bool {
    a.player_start != b.player_start
        || a.level_cap != b.level_cap
        || a.pois != b.pois
        || a.npcs != b.npcs
        || a.spawns != b.spawns
        || a.nodes != b.nodes
        || a.creatures != b.creatures
        || a.ore_areas != b.ore_areas
}

/// Field edits become one undo step per widget interaction (a drag, a typing session, a pick).
fn record_edits(ui: &egui::Ui, s: &mut UiState, before: &Gameplay) {
    let changed = content_differs(before, &s.gameplay);
    if changed {
        if !s.gp_ed.editing {
            let label = match before.selected {
                Some(sel) if before.creatures == s.gameplay.creatures => {
                    format!("Edit {}", edit::sel_label(before, sel))
                }
                _ => "Edit creature library".to_string(),
            };
            s.history.begin(label);
            s.history.touch_gameplay(before);
            s.gp_ed.editing = true;
        }
        s.gameplay.dirty = true;
        s.meta.dirty = true;
    }
    if s.gp_ed.editing && !ui.ctx().egui_is_using_pointer() && !ui.ctx().egui_wants_keyboard_input()
    {
        s.history.commit(&s.store);
        s.gp_ed.editing = false;
    }
}

fn perform(s: &mut UiState, a: Action) {
    if s.gp_ed.editing {
        s.history.commit(&s.store);
        s.gp_ed.editing = false;
    }
    let a = match a {
        Action::Focus(sel) => return focus(s, sel),
        Action::DrawSpawn(ids) => {
            s.gp_ed.tool = GameplayTool::Spawn;
            s.gp_ed.draft.clear();
            if !ids.is_empty() {
                s.gp_ed.spawn_creatures = ids;
            }
            s.gameplay.selected = None;
            s.ed.toast(
                "Outline the spawn region: click corners on the ground, Enter to finish"
                    .to_string(),
            );
            return;
        }
        a => a,
    };
    let g = Ground {
        terrain: &s.terrain,
        zones: &s.zones,
        water: &s.water,
        nature: None,
    };
    match a {
        Action::Focus(_) | Action::DrawSpawn(_) => {}
        Action::AddToRegion(rid, cid) => {
            let name = s
                .gameplay
                .creature(cid)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            s.history.begin(format!(
                "Add {name} to {}",
                edit::sel_label(&s.gameplay, GameplaySel::Spawn(rid))
            ));
            s.history.touch_gameplay(&s.gameplay);
            if let Some(r) = s.gameplay.spawns.iter_mut().find(|r| r.id == rid)
                && !r.entries.iter().any(|e| e.creature == cid)
            {
                r.entries.push(SpawnEntry {
                    creature: cid,
                    weight: 1.0,
                });
            }
            s.history.commit(&s.store);
        }
        Action::Remove(sel) => {
            s.history
                .begin(format!("Remove {}", edit::sel_label(&s.gameplay, sel)));
            s.history.touch_gameplay(&s.gameplay);
            if let Some(what) = edit::remove_sel(&mut s.gameplay, sel) {
                s.ed.toast(format!("Removed {what}"));
            }
            s.history.commit(&s.store);
        }
        Action::Duplicate(sel) => {
            s.history
                .begin(format!("Duplicate {}", edit::sel_label(&s.gameplay, sel)));
            s.history.touch_gameplay(&s.gameplay);
            if let Some(copy) = edit::duplicate(&mut s.gameplay, &g, sel) {
                s.gameplay.selected = Some(copy);
            }
            s.history.commit(&s.store);
        }
        Action::StarterCreatures => {
            s.history.begin("Add starter creatures");
            s.history.touch_gameplay(&s.gameplay);
            let n = edit::add_starter_creatures(&mut s.gameplay);
            s.history.commit(&s.store);
            s.ed.toast(format!("Added {n} creatures to the library"));
        }
        Action::NewCreature => {
            s.history.begin("New creature");
            s.history.touch_gameplay(&s.gameplay);
            let id = edit::add_creature(
                &mut s.gameplay,
                "New Creature",
                "",
                [1, 5],
                Aggression::Hostile,
                false,
            );
            s.gp_ed.creature = Some(id);
            s.history.commit(&s.store);
        }
        Action::RemoveCreature(id) => {
            s.history.begin("Remove creature");
            s.history.touch_gameplay(&s.gameplay);
            if let Some(what) = edit::remove(&mut s.gameplay, id) {
                s.ed.toast(format!("Removed {what} (and from every encounter table)"));
            }
            s.gp_ed.creature = None;
            s.history.commit(&s.store);
        }
    }
    s.gameplay.dirty = true;
    s.meta.dirty = true;
}

/// Look at it · Duplicate · Remove, at the foot of the inspector.
fn actions_row(ui: &mut egui::Ui, sel: GameplaySel, action: &mut Option<Action>) {
    ui.add_space(4.0);
    let n = if sel == GameplaySel::PlayerStart {
        2.0
    } else {
        3.0
    };
    let w = ((ui.available_width() - 6.0 * (n - 1.0)) / n).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if icon_text_button(ui, "Target", "Look at it", Some(w)).clicked() {
            *action = Some(Action::Focus(sel));
        }
        if sel != GameplaySel::PlayerStart
            && icon_text_button(ui, "SquareBehindSquare1", "Duplicate", Some(w))
                .on_hover_text("Duplicate (Ctrl/⌘ D)")
                .clicked()
        {
            *action = Some(Action::Duplicate(sel));
        }
        if icon_text_button(ui, "TrashCan", "Remove", Some(w))
            .on_hover_text("Remove (Delete)")
            .clicked()
        {
            *action = Some(Action::Remove(sel));
        }
    });
}

fn issues_ui(ui: &mut egui::Ui, list: &[edit::Issue]) {
    for i in list {
        if i.warn {
            icon_line(ui, "ExclamationTriangle", WARN, &i.text);
        } else {
            icon_line(ui, "CircleInfo", TEXT_DIM, &i.text);
        }
    }
}

fn facing_slider(ui: &mut egui::Ui, yaw: &mut f32) {
    let mut deg = yaw.to_degrees();
    deg = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if slider_row(ui, "Facing", &mut deg, -180.0..=180.0, |v| {
        format!("{v:.0}° {}", compass(v))
    })
    .changed()
    {
        *yaw = deg.to_radians();
    }
}

fn place_line(g: &Ground, p: Vec2) -> String {
    format!(
        "{:.0}, {:.0} · {:.0} m above sea",
        p.x,
        p.y,
        g.terrain.height_at(p.x, p.y) - g.terrain.sea_level
    )
}

fn inspector(ui: &mut egui::Ui, s: &mut UiState, sel: GameplaySel, action: &mut Option<Action>) {
    let g = Ground {
        terrain: &s.terrain,
        zones: &s.zones,
        water: &s.water,
        nature: None,
    };
    // The zone line already says when there's no zone.
    let issues: Vec<edit::Issue> = edit::issues(&s.gameplay, &g, sel)
        .into_iter()
        .filter(|i| i.warn || !i.text.starts_with("Not inside a zone"))
        .collect();
    let gp = &mut *s.gameplay;
    match sel {
        GameplaySel::PlayerStart => {
            let Some(st) = gp.player_start.as_mut() else {
                return;
            };
            let p = Vec2::new(st.pos[0], st.pos[2]);
            hero(
                ui,
                START_ICON,
                START_COLOR,
                "Player Start",
                &place_line(&g, p),
                &zone_text(&g, p),
            );
            hint_small(ui, "Where new characters enter the world.");
            facing_slider(ui, &mut st.yaw);
        }
        GameplaySel::Poi(id) => {
            let npcs: Vec<(u32, String, NpcRole)> = gp
                .npcs
                .iter()
                .filter(|n| n.poi == Some(id))
                .map(|n| (n.id, n.name.clone(), n.role))
                .collect();
            let Some(q) = gp.pois.iter_mut().find(|q| q.id == id) else {
                return;
            };
            let p = Vec2::new(q.pos[0], q.pos[2]);
            hero(
                ui,
                poi_icon(q.kind),
                POI_COLOR,
                &q.name.clone(),
                &format!("{} · {}", q.kind.name(), place_line(&g, p)),
                &zone_text(&g, p),
            );
            name_field(ui, &mut q.name);
            icon_dropdown(ui, "Kind", "gp_poi_kind", &mut q.kind, &PoiKind::ALL, |k| {
                (poi_icon(k), k.name(), k.hint())
            });
            slider_row_log(ui, "Discovery radius", &mut q.radius, 3.0..=600.0, meters);
            let zone_lv = g.zone_levels(p);
            let mut custom = q.level.is_some();
            let label = match zone_lv {
                Some(l) => format!("Own level range (zone: {})", level_label(l)),
                None => "Recommended level range".to_string(),
            };
            if toggle(ui, &mut custom, label).changed() {
                q.level = if custom {
                    Some(zone_lv.unwrap_or([1, 5]))
                } else {
                    None
                };
            }
            if let Some(l) = &mut q.level {
                level_range(ui, "Levels", l);
            }
            section_fold(
                ui,
                "gp_poi_npcs",
                "NPCs here",
                &npcs.len().to_string(),
                |ui| {
                    if npcs.is_empty() {
                        hint_small(ui, "None yet: NPCs placed within 60 m join this place.");
                    }
                    for (nid, name, role) in &npcs {
                        if item_row(ui, npc_icon(*role), NPC_COLOR, name, role.name(), false)
                            .clicked()
                        {
                            gp.selected = Some(GameplaySel::Npc(*nid));
                        }
                    }
                },
            );
            if let Some(q) = gp.pois.iter_mut().find(|q| q.id == id) {
                notes_field(ui, &mut q.notes);
            }
        }
        GameplaySel::Npc(id) => {
            let mut pois: Vec<(u32, String, f32)> = vec![];
            if let Some(n) = gp.npcs.iter().find(|n| n.id == id) {
                let p = Vec2::new(n.pos[0], n.pos[2]);
                pois = gp
                    .pois
                    .iter()
                    .map(|q| {
                        (
                            q.id,
                            q.name.clone(),
                            Vec2::new(q.pos[0], q.pos[2]).distance(p),
                        )
                    })
                    .collect();
                pois.sort_by(|a, b| a.2.total_cmp(&b.2));
            }
            let nearest = gp
                .npcs
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| edit::nearest_poi(gp, Vec2::new(n.pos[0], n.pos[2])));
            let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) else {
                return;
            };
            let p = Vec2::new(n.pos[0], n.pos[2]);
            let home = n
                .poi
                .and_then(|pid| pois.iter().find(|q| q.0 == pid))
                .map(|q| format!("Belongs to {}", q.1));
            hero(
                ui,
                npc_icon(n.role),
                NPC_COLOR,
                &n.name.clone(),
                &format!("{} · {}", n.role.name(), place_line(&g, p)),
                &home.unwrap_or_else(|| zone_text(&g, p)),
            );
            name_field(ui, &mut n.name);
            icon_dropdown(ui, "Role", "gp_npc_role", &mut n.role, &NpcRole::ALL, |r| {
                (npc_icon(r), r.name(), r.hint())
            });
            hint_small(ui, &zone_text(&g, p));
            facing_slider(ui, &mut n.yaw);
            let cur = n
                .poi
                .and_then(|pid| pois.iter().find(|q| q.0 == pid))
                .map(|q| q.1.clone())
                .unwrap_or_else(|| "No point of interest".into());
            dropdown_row(ui, "Belongs to", "gp_npc_poi", cur, |ui| {
                menu_option(ui, &mut n.poi, None, "No point of interest");
                for (pid, name, d) in &pois {
                    if menu_item(ui, name.as_str(), &meters(*d), n.poi == Some(*pid)).clicked() {
                        n.poi = Some(*pid);
                    }
                }
            });
            if let Some(nid) = nearest
                && n.poi != Some(nid)
                && small_icon_button(ui, "MapPin", "Join the nearest place").clicked()
            {
                n.poi = Some(nid);
            }
            notes_field(ui, &mut n.notes);
        }
        GameplaySel::Spawn(id) => spawn_inspector(ui, s, id, action),
        GameplaySel::OreArea(id) => ore_ui::area_inspector(ui, s, id, action),
        GameplaySel::Node(id) => {
            let area = gp
                .nodes
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| n.area)
                .and_then(|a| gp.ore_areas.iter().find(|x| x.id == a))
                .map(|a| a.name.clone());
            let Some(n) = gp.nodes.iter_mut().find(|n| n.id == id) else {
                return;
            };
            let before = n.clone();
            let p = Vec2::new(n.pos[0], n.pos[2]);
            if let Some(o) = n.ore() {
                let title = if n.variant.is_empty() {
                    format!("{} Deposit", o.name())
                } else {
                    n.variant.clone()
                };
                hero(
                    ui,
                    crate::gameplay::markers::ORE_ICON,
                    crate::gameplay::markers::rarity_color(o),
                    &title,
                    &place_line(&g, p),
                    &zone_text(&g, p),
                );
                ore_ui::deposit_fields(ui, n, o, area.as_deref());
            } else {
                let title = if n.variant.is_empty() {
                    format!("{} Node", n.kind.name())
                } else {
                    n.variant.clone()
                };
                hero(
                    ui,
                    node_icon(n.kind),
                    node_color(n.kind),
                    &title,
                    &format!(
                        "{} · tier {} · {}",
                        n.kind.name(),
                        n.tier,
                        place_line(&g, p)
                    ),
                    &zone_text(&g, p),
                );
                icon_dropdown(
                    ui,
                    "Kind",
                    "gp_node_kind",
                    &mut n.kind,
                    &ResourceKind::ALL,
                    |k| (node_icon(k), k.name(), k.hint()),
                );
                ui.label(RichText::new("Variant").size(12.5).color(TEXT_DIM));
                ui.horizontal(|ui| {
                    let w = ui.available_width() - 34.0;
                    ui.allocate_ui(vec2(w, 30.0), |ui| {
                        well(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut n.variant)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(f32::INFINITY),
                            )
                        })
                    });
                    if icon_button(
                        ui,
                        "ArrowRotateCounterClockwise",
                        "Automatic name for the kind, tier and biome",
                    )
                    .clicked()
                    {
                        n.variant = edit::default_variant(
                            n.kind,
                            n.tier,
                            crate::biomes::BiomePreset::from_id(g.terrain.biome_at(p.x, p.y)),
                        );
                    }
                });
                let mut tier = n.tier as f32;
                slider_row(ui, "Tier", &mut tier, 1.0..=8.0, |v| format!("{v:.0}"));
                n.tier = tier.round() as u8;
                hint_small(
                    ui,
                    &format!("The zone here suggests tier {}.", scatter::zone_tier(&g, p)),
                );
                slider_row_log(
                    ui,
                    "Respawn",
                    &mut n.respawn_secs,
                    10.0..=7200.0,
                    secs_label,
                );
                if n.auto {
                    hint_small(
                        ui,
                        "Placed by Scatter. Editing it keeps it when you scatter again.",
                    );
                }
            }
            if *n != before {
                n.auto = false;
            }
        }
    }
    issues_ui(ui, &issues);
    actions_row(ui, sel, action);
}

fn spawn_inspector(ui: &mut egui::Ui, s: &mut UiState, id: u32, action: &mut Option<Action>) {
    let g = Ground {
        terrain: &s.terrain,
        zones: &s.zones,
        water: &s.water,
        nature: None,
    };
    let gp = &mut *s.gameplay;
    let Some(idx) = gp.spawns.iter().position(|r| r.id == id) else {
        return;
    };
    let info = g.region_info(&gp.spawns[idx].polygon);
    let col = spawn_color(gp.spawns[idx].aggression(gp));
    let creatures = gp.creatures.clone();
    let r = &mut gp.spawns[idx];
    let zone = info.zone.and_then(|z| g.zones.by_id(z));
    let line = format!(
        "{:.2} km² · ~{} alive · {}",
        r.area() / 1.0e6,
        r.expected_alive(),
        zone.map(|z| z.name.clone()).unwrap_or("no zone".into())
    );
    hero(
        ui,
        SPAWN_ICON,
        col,
        &r.name.clone(),
        &line,
        &level_label(r.level),
    );
    name_field(ui, &mut r.name);

    let mut starter = false;
    section_fold(
        ui,
        "gp_spawn_table",
        "Encounter table",
        &r.entries.len().to_string(),
        |ui| {
            let chances = r.chances();
            let mut remove = None;
            for (i, e) in r.entries.iter_mut().enumerate() {
                let c = creatures.iter().find(|c| c.id == e.creature);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let dot = c
                        .map(|c| aggression_color(c.aggression))
                        .unwrap_or(TEXT_DIM);
                    let (dr, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(
                        egui::Rect::from_center_size(dr.center(), vec2(9.0, 9.0)),
                        CornerRadius::same(3),
                        dot,
                    );
                    egui::ComboBox::from_id_salt(("gp_entry", i))
                        .width(112.0)
                        .selected_text(c.map(|c| c.name.clone()).unwrap_or("(missing)".into()))
                        .dropdown_ui(ui, |ui| {
                            for c in &creatures {
                                menu_option(
                                    ui,
                                    &mut e.creature,
                                    c.id,
                                    format!("{}  {}", c.name, level_label(c.level)),
                                );
                            }
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, "CrossSmall", "Take out of the table").clicked() {
                            remove = Some(i);
                        }
                        ui.label(
                            RichText::new(format!(
                                "{:.0}%",
                                chances.get(i).copied().unwrap_or(0.0) * 100.0
                            ))
                            .font(FontId::new(11.0, mono()))
                            .color(TEXT_MUTE),
                        );
                        ui.add(
                            egui::DragValue::new(&mut e.weight)
                                .range(0.0..=100.0)
                                .speed(0.05)
                                .max_decimals(1),
                        )
                        .on_hover_text("Weight: how often it spawns relative to the others");
                    });
                });
            }
            if let Some(i) = remove {
                r.entries.remove(i);
            }
            let missing: Vec<&crate::gameplay::CreatureType> = creatures
                .iter()
                .filter(|c| !r.entries.iter().any(|e| e.creature == c.id))
                .collect();
            if creatures.is_empty() {
                hint_small(ui, "The creature library is empty.");
                if small_icon_button(ui, SPAWN_ICON, "Add starter creatures").clicked() {
                    starter = true;
                }
            } else if !missing.is_empty() {
                egui::ComboBox::from_id_salt("gp_entry_add")
                    .width(ui.available_width())
                    .selected_text("Add a creature…")
                    .dropdown_ui(ui, |ui| {
                        for c in missing {
                            let label = format!(
                                "{}  {} · {}{}",
                                c.name,
                                level_label(c.level),
                                c.aggression.name(),
                                if c.elite { " · elite" } else { "" }
                            );
                            if menu_check(
                                ui,
                                false,
                                RichText::new(label)
                                    .color(aggression_color(c.aggression).gamma_multiply(1.1)),
                            )
                            .clicked()
                            {
                                r.entries.push(SpawnEntry {
                                    creature: c.id,
                                    weight: 1.0,
                                });
                            }
                        }
                    });
            }
        },
    );
    if starter {
        *action = Some(Action::StarterCreatures);
    }

    section_fold(ui, "gp_spawn_levels", "Levels", "", |ui| {
        level_range(ui, "Level", &mut r.level);
        if let Some(z) = zone
            && r.level != levels_of(z)
            && small_button(
                ui,
                &format!("Match {} ({})", z.name, level_label(levels_of(z))),
            )
            .clicked()
        {
            r.level = levels_of(z);
        }
    });

    section_fold(ui, "gp_spawn_pop", "Population", "", |ui| {
        slider_row_log(ui, "Density", &mut r.density, 5.0..=1500.0, |v| {
            format!("{v:.0} per km²")
        });
        num_row(ui, "Max alive", |ui| {
            if r.max_alive == 0 {
                ui.label(RichText::new("no cap").size(11.5).color(TEXT_MUTE));
            }
            ui.add(
                egui::DragValue::new(&mut r.max_alive)
                    .range(0..=2000)
                    .speed(0.2),
            )
            .on_hover_text("Hard cap on creatures alive at once (0 = from density only)");
        });
        num_row(ui, "Pack size", |ui| {
            ui.add(
                egui::DragValue::new(&mut r.pack[1])
                    .range(1..=50)
                    .speed(0.05),
            );
            ui.label(RichText::new("to").color(TEXT_MUTE));
            ui.add(
                egui::DragValue::new(&mut r.pack[0])
                    .range(1..=50)
                    .speed(0.05),
            );
        });
        r.pack[1] = r.pack[1].max(r.pack[0]);
        slider_row_log(ui, "Respawn", &mut r.respawn_secs, 5.0..=3600.0, secs_label);
        dropdown_row(
            ui,
            "Behaviour",
            "gp_spawn_behaviour",
            r.behaviour.name(),
            |ui| {
                for b in SpawnBehaviour::ALL {
                    menu_option(ui, &mut r.behaviour, b, b.name()).on_hover_text(b.hint());
                }
            },
        );
        hint_small(ui, r.behaviour.hint());
    });
    hint_small(
        ui,
        "Drag the white corners in the world to reshape it; drag inside it to move it.",
    );
    notes_field(ui, &mut r.notes);
}

/// The open creature's fields (below its row in the library).
/// `region`: the selected spawn region (id, name, whether this creature is already in it).
fn creature_editor(
    ui: &mut egui::Ui,
    c: &mut crate::gameplay::CreatureType,
    used: usize,
    region: Option<(u32, &str, bool)>,
    drawing: bool,
    action: &mut Option<Action>,
) {
    egui::Frame::new().fill(SOFT).corner_radius(CornerRadius::same(R_TILE)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        name_field(ui, &mut c.name);
        text_field(ui, "Family", &mut c.family, "wolf, bandit, spider…");
        level_range(ui, "Level", &mut c.level);
        segmented(ui, &mut c.aggression, &Aggression::ALL.map(|a| (a, a.name())));
        toggle(ui, &mut c.elite, "Elite (tougher, rarer, better loot)");
        well(ui, |ui| ui.add(egui::TextEdit::multiline(&mut c.notes).frame(egui::Frame::NONE).hint_text(RichText::new("Notes…").color(TEXT_MUTE)).desired_rows(1).desired_width(f32::INFINITY)));
        match region {
            Some((_, name, true)) => {
                icon_line(ui, "Checkmark1Small", TEXT_DIM, &format!("In {name}"));
            }
            Some((rid, name, false)) => {
                if small_icon_button(ui, "PlusMedium", &format!("Add to {name}")).on_hover_text("Put it in the selected region's encounter table").clicked() {
                    *action = Some(Action::AddToRegion(rid, c.id));
                }
            }
            None => {}
        }
        if small_icon_button(ui, SPAWN_ICON, "Draw a spawn region for it")
            .on_hover_text(if drawing { "Make it the only creature picked for the next region you draw" } else { "Switch to the Spawns tool with it picked, then outline where it lives on the ground" })
            .clicked()
        {
            *action = Some(Action::DrawSpawn(vec![c.id]));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("In {used} spawn region{}", if used == 1 { "" } else { "s" })).size(11.5).color(TEXT_MUTE));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_icon_button(ui, "TrashCan", "Remove").on_hover_text("Remove the creature (and from every encounter table)").clicked() {
                    *action = Some(Action::RemoveCreature(c.id));
                }
            });
        });
    });
    ui.add_space(4.0);
}

fn creature_library(ui: &mut egui::Ui, s: &mut UiState, action: &mut Option<Action>) {
    let gp = &mut *s.gameplay;
    let drawing = s.gp_ed.tool == GameplayTool::Spawn;
    let region = match gp.selected {
        Some(GameplaySel::Spawn(id)) => gp.spawns.iter().find(|r| r.id == id).map(|r| {
            (
                r.id,
                r.name.clone(),
                r.entries.iter().map(|e| e.creature).collect::<Vec<_>>(),
            )
        }),
        _ => None,
    };
    hint_small(
        ui,
        "Creature types. They appear in the world through spawn regions: areas you outline on the ground with the Spawns tool.",
    );
    if !drawing
        && !gp.creatures.is_empty()
        && region.is_none()
        && primary_icon_button(ui, SPAWN_ICON, "Add a spawn region")
            .on_hover_text(
                "Switch to the Spawns tool: pick creatures, then outline the area on the ground",
            )
            .clicked()
    {
        *action = Some(Action::DrawSpawn(s.gp_ed.creature.into_iter().collect()));
    }
    if gp.creatures.is_empty()
        && primary_icon_button(ui, SPAWN_ICON, "Add starter creatures")
            .on_hover_text("14 fantasy creatures from level 1 to 30: hares, boars, wolves, spiders, bandits, bears, goblins, harpies, trolls, treants…")
            .clicked()
    {
        *action = Some(Action::StarterCreatures);
    }
    let used: Vec<usize> = gp
        .creatures
        .iter()
        .map(|c| {
            gp.spawns
                .iter()
                .filter(|r| r.entries.iter().any(|e| e.creature == c.id))
                .count()
        })
        .collect();
    ui.spacing_mut().item_spacing.y = 2.0;
    for (i, c) in gp.creatures.iter_mut().enumerate() {
        let detail = format!(
            "{}{}",
            level_label(c.level),
            if c.elite { " · elite" } else { "" }
        );
        let open = s.gp_ed.creature == Some(c.id);
        if creature_row(ui, aggression_color(c.aggression), &c.name, &detail, open).clicked() {
            s.gp_ed.creature = if open { None } else { Some(c.id) };
        }
        if open {
            let r = region
                .as_ref()
                .map(|(id, name, list)| (*id, name.as_str(), list.contains(&c.id)));
            creature_editor(ui, c, used[i], r, drawing, action);
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if icon_text_button(ui, "PlusMedium", "New creature", None).clicked() {
            *action = Some(Action::NewCreature);
        }
        if !gp.creatures.is_empty()
            && icon_text_button(ui, SPAWN_ICON, "Starter set", None)
                .on_hover_text("Add the starter creatures that aren't in the library yet")
                .clicked()
        {
            *action = Some(Action::StarterCreatures);
        }
    });
}

/// A library row: the aggression colour dot, name, level on the right.
fn creature_row(
    ui: &mut egui::Ui,
    col: Color32,
    name: &str,
    detail: &str,
    open: bool,
) -> egui::Response {
    list_row(ui, Lead::Dot(col), name, "", detail, open)
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The inspector's title when the Gameplay tab has something to inspect (None hides it): the
/// selected pin, region or node, else the creature library while it's open.
pub(super) fn gameplay_inspector(s: &UiState) -> Option<(String, bool)> {
    match s.gameplay.selected.filter(|sel| s.gameplay.exists(*sel)) {
        Some(sel) => Some((sel_title(&s.gameplay, sel).to_string(), true)),
        None if library_shown(s) => Some((
            format!("Creature library ({})", s.gameplay.creatures.len()),
            true,
        )),
        None => None,
    }
}

/// Finish / Cancel for an outline being drawn (spawn region or deposit area).
fn draft_buttons(ui: &mut egui::Ui, s: &mut UiState, finish: fn(&mut UiState)) {
    let n = s.gp_ed.draft.len();
    dock_text(ui, &format!("{n} corner{}", if n == 1 { "" } else { "s" }));
    if ui
        .add_enabled_ui(n >= 3, |ui| {
            small_icon_button(ui, "Checkmark1Small", "Finish")
        })
        .inner
        .on_hover_text("Finish (Enter)")
        .clicked()
    {
        finish(s);
    }
    if small_icon_button(ui, "CrossSmall", "Cancel")
        .on_hover_text("Cancel (Esc)")
        .clicked()
    {
        s.gp_ed.draft.clear();
    }
    dock_sep(ui);
}

/// The tool dock: the tool (with the picked kind), its mode and its key hints.
pub(super) fn gameplay_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let t = s.gp_ed.tool;
    match t {
        GameplayTool::Select => {
            dock_title(ui, tool_icon(t), "Select");
            dock_sep(ui);
            dock_hints(
                ui,
                &[
                    ("Click", "Select a pin"),
                    ("Drag", "Move"),
                    ("⌫", "Remove"),
                    ("⌘D", "Duplicate"),
                ],
            );
        }
        GameplayTool::PlayerStart => {
            dock_title(ui, START_ICON, "Player start");
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Put it here"), ("Drag", "Turn them")]);
        }
        GameplayTool::Poi => {
            let k = s.gp_ed.poi_kind;
            dock_title(ui, poi_icon(k), k.short());
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Put it here"), ("Drag", "Move a pin")]);
        }
        GameplayTool::Npc => {
            let r = s.gp_ed.npc_role;
            dock_title(ui, npc_icon(r), r.name());
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Put them here"), ("Drag", "Turn them")]);
        }
        GameplayTool::Spawn => {
            dock_title(ui, SPAWN_ICON, "Spawn region");
            dock_sep(ui);
            if !s.gp_ed.draft.is_empty() {
                draft_buttons(ui, s, finish_spawn);
            }
            dock_hints(
                ui,
                &[
                    ("Click", "Corner"),
                    ("↵", "Finish"),
                    ("⌫", "Drop a corner"),
                    ("Esc", "Cancel"),
                ],
            );
        }
        GameplayTool::Node => {
            let k = s.gp_ed.node_kind;
            dock_title(ui, node_icon(k), &format!("{} node", k.name()));
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Put it here"), ("Drag", "Move a pin")]);
        }
        GameplayTool::Ore => {
            dock_title(ui, "Diamond", &format!("{} deposit", s.gp_ed.ore.name()));
            dock_sep(ui);
            ui.allocate_ui(vec2(210.0, 28.0), |ui| {
                segmented_solid(
                    ui,
                    &mut s.gp_ed.ore_mode,
                    &[
                        (OreMode::Single, "One deposit"),
                        (OreMode::Area, "Draw an area"),
                    ],
                );
            });
            dock_sep(ui);
            if s.gp_ed.ore_mode == OreMode::Area {
                if !s.gp_ed.draft.is_empty() {
                    draft_buttons(ui, s, ore_ui::finish_area);
                }
                dock_hints(
                    ui,
                    &[("Click", "Corner"), ("↵", "Finish"), ("Esc", "Cancel")],
                );
            } else {
                dock_hints(ui, &[("Click", "Put it here"), ("Drag", "Move it")]);
            }
        }
        GameplayTool::Scatter => {
            dock_title(ui, "Shuffle", "Scatter");
            dock_sep(ui);
            dock_text(
                ui,
                "Fills the whole world in one click: pick the kinds in the panel.",
            );
        }
        GameplayTool::Remove => {
            dock_title(ui, "Eraser", "Remove");
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Remove a pin or region")]);
        }
    }
    true
}
