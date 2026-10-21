//! Per-tab tool panels (left) and inspectors (right).

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, ScrollArea, pos2, vec2};
use egui_phosphor::regular as icon;

use super::theme::*;
use super::widgets::*;
use super::{UiOut, UiState, UiTextures, category_icon};
use crate::audio::SoundEvent;
use crate::editor::{LandTool, ObjectTool, PaintTool, ScatterMode, Tab};
use crate::look::{LookParams, LookPreset, ToneCurve};
use crate::objects::Category;
use crate::sky::ShadowQuality;
use crate::terrain::layers::{LAYER_COUNT, Layer, Surface};
use crate::world::{Mood, Soundscape, Weather, Zone};

mod analyze_tab;
pub(super) mod bigworld_ui;
pub(super) mod gallery;
mod gameplay_tab;
mod map_tab;
mod roads_tab;

mod nature_tab;
mod objects_tab;
mod terrain_tab;
mod zones_tab;

use super::shell::{INSP_W, PANEL_W, PANEL_X, TOP};
use crate::editor::TabGroup;

/// A sub-tab in the tool panel header: a tab of the group, or Nature's Wild nature / Fields.
#[derive(Clone, Copy, PartialEq)]
enum Sub {
    Tab(Tab),
    Fields(bool),
}

fn sub_tab_list(s: &UiState) -> Vec<(Sub, &'static str)> {
    let g = s.ed.tab.group();
    if g == TabGroup::Nature {
        return vec![
            (Sub::Fields(false), "Wild nature"),
            (Sub::Fields(true), "Fields"),
        ];
    }
    let tabs = g.tabs();
    if tabs.len() < 2 {
        return vec![];
    }
    tabs.iter()
        .map(|t| (Sub::Tab(*t), TabGroup::sub_name(*t)))
        .collect()
}

fn current_sub(s: &UiState) -> Sub {
    if s.ed.tab.group() == TabGroup::Nature {
        Sub::Fields(s.field_tool.on)
    } else {
        Sub::Tab(s.ed.tab)
    }
}

fn sub_tabs(ui: &mut egui::Ui, s: &mut UiState, subs: &[(Sub, &str)]) {
    let mut cur = current_sub(s);
    if segmented(ui, &mut cur, subs) {
        match cur {
            Sub::Tab(t) => s.ed.tab = t,
            Sub::Fields(on) => s.field_tool.on = on,
        }
    }
}

/// A panel header: the title (hover: what's in it), the sub-tabs (same row when short, their
/// own row when the labels run long) and the collapse button. True when collapse was clicked.
fn panel_header(
    ui: &mut egui::Ui,
    s: &mut UiState,
    title: &str,
    hint: &str,
    size: f32,
    hide_icon: &str,
    hide_tip: &str,
    with_subs: bool,
) -> bool {
    let mut hide = false;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 13,
            right: 9,
            top: 11,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let subs = if with_subs { sub_tab_list(s) } else { vec![] };
            let chars: usize = subs.iter().map(|x| x.1.chars().count()).sum();
            let wrap = chars > 14;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let t = ui.label(
                    RichText::new(title)
                        .font(FontId::new(size, semibold()))
                        .color(TEXT),
                );
                if !hint.is_empty() {
                    t.on_hover_text(hint);
                }
                let avail = (ui.available_width() - 28.0 - 8.0).max(0.0);
                if !subs.is_empty() && !wrap {
                    ui.allocate_ui(vec2(avail, 26.0), |ui| sub_tabs(ui, s, &subs));
                } else {
                    ui.add_space(avail);
                }
                if icon_button(ui, hide_icon, hide_tip).clicked() {
                    hide = true;
                }
            });
            if !subs.is_empty() && wrap {
                ui.add_space(2.0);
                sub_tabs(ui, s, &subs);
            }
        });
    hide
}

/// A collapsed panel or inspector: a small glass pill with its icon and title. True when clicked.
fn reopen_pill(
    ctx: &egui::Context,
    id: &str,
    pos: egui::Pos2,
    pivot: egui::Align2,
    icon: &str,
    title: &str,
) -> bool {
    let mut clicked = false;
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .pivot(pivot)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let r = glass(
                ui,
                glass_frame()
                    .corner_radius(CornerRadius::same(11))
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 12,
                        top: 0,
                        bottom: 0,
                    }),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.set_height(34.0);
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 34.0), egui::Sense::hover());
                        paint_icon(ui, r.center(), icon, 15.0, TEXT);
                        ui.label(
                            RichText::new(title)
                                .font(FontId::new(13.0, semibold()))
                                .color(TEXT),
                        );
                    });
                },
            );
            let resp = ui
                .interact(
                    r.response.rect,
                    egui::Id::new(id).with("click"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            clicked = resp.clicked();
        });
    clicked
}

pub fn left_panel(
    ctx: &egui::Context,
    s: &mut UiState,
    out: &mut UiOut,
    tex: &UiTextures,
    screen: egui::Rect,
) {
    let mut sh = super::shell::get(ctx);
    let g = s.ed.tab.group();
    if !sh.panel_open {
        if reopen_pill(
            ctx,
            "left_panel_pill",
            pos2(screen.left() + 72.0, screen.top() + 64.0),
            egui::Align2::LEFT_TOP,
            "SidebarSimpleLeftWide",
            g.name(),
        ) {
            sh.panel_open = true;
            super::shell::set(ctx, sh);
        }
        return;
    }
    let pos = pos2(screen.left() + PANEL_X, screen.top() + TOP);
    let max_h = (screen.height() - 112.0).max(240.0);
    egui::Area::new(egui::Id::new("left_panel"))
        .order(egui::Order::Middle)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            glass(ui, glass_frame().inner_margin(egui::Margin::ZERO), |ui| {
                ui.set_width(PANEL_W - 2.0);
                ui.set_max_height(max_h);
                let hint = format!("{}\n{}", g.hint(), s.ed.tab.hint());
                if panel_header(
                    ui,
                    s,
                    g.name(),
                    &hint,
                    16.0,
                    "SidebarSimpleLeftWide",
                    "Hide panel (Tab)",
                    true,
                ) {
                    sh.panel_open = false;
                    super::shell::set(ui.ctx(), sh);
                }
                ScrollArea::vertical()
                    .id_salt(("left_body", s.ed.tab))
                    .max_height(ui.available_height())
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: 12,
                                right: 12,
                                top: 2,
                                bottom: 13,
                            })
                            .show(ui, |ui| {
                                ui.set_width(PANEL_W - 2.0 - 24.0);
                                left_tab(ui, s, out, tex);
                            });
                    });
            });
        });
}

fn left_tab(ui: &mut egui::Ui, s: &mut UiState, out: &mut UiOut, tex: &UiTextures) {
    match s.ed.tab {
        Tab::Land => terrain_tab::land_left(ui, s, out),
        Tab::Sculpt => terrain_tab::sculpt_left(ui, s, out),
        Tab::Paint => terrain_tab::paint_left(ui, s, out, tex),
        Tab::Caves => crate::caves::ui::caves_left(ui, s),
        Tab::Water => water_left(ui, s),
        Tab::Nature if s.field_tool.on => crate::fields::ui::panel(ui, s),
        Tab::Nature => nature_tab::nature_left(ui, s, out),
        Tab::Objects => objects_tab::objects_left(ui, s, out),
        Tab::Zones => zones_tab::zones_left(ui, s),
        Tab::Lighting => lighting_left(ui, s),
        Tab::Weather => weather_left(ui, s),
        Tab::Style => style_left(ui, s),
        Tab::Audio => audio_left(ui, s, out),
        Tab::Roads => roads_tab::roads_left(ui, s),
        Tab::Gameplay => gameplay_tab::gameplay_left(ui, s),
        Tab::Analyze => analyze_tab::analyze_left(ui, s),
        Tab::Map => map_tab::map_left(ui, s),
        Tab::Dungeons => crate::dungeons::ui::dungeons_left(ui, s),
        Tab::DungeonLook => crate::dungeons::ui::look_left(ui, s),
        Tab::DungeonLights => crate::dungeons::ui::lights_left(ui, s),
    }
}

/// The inspector island (right): only when the tab has something to inspect. `header` false keeps
/// the tab's own title (tabs not yet moved over) with the collapse button over its corner.
fn inspector(
    ctx: &egui::Context,
    screen: egui::Rect,
    s: &mut UiState,
    title: &str,
    header: bool,
    add: impl FnOnce(&mut egui::Ui, &mut UiState),
) {
    let mut sh = super::shell::get(ctx);
    if !sh.insp_open {
        if reopen_pill(
            ctx,
            "right_panel_pill",
            pos2(screen.right() - 12.0, screen.top() + 64.0),
            egui::Align2::RIGHT_TOP,
            "SidebarSimpleRightWide",
            title,
        ) {
            sh.insp_open = true;
            super::shell::set(ctx, sh);
        }
        return;
    }
    let inspector_width = INSP_W;
    let pos = pos2(screen.right() - 12.0 - inspector_width, screen.top() + TOP);
    let max_h = (screen.height() - 112.0).max(240.0);
    egui::Area::new(egui::Id::new("right_panel"))
        .order(egui::Order::Middle)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let r = glass(ui, glass_frame().inner_margin(egui::Margin::ZERO), |ui| {
                ui.set_width(inspector_width - 2.0);
                ui.set_max_height(max_h);
                if header
                    && panel_header(
                        ui,
                        s,
                        title,
                        "",
                        14.5,
                        "SidebarSimpleRightWide",
                        "Hide inspector (Tab)",
                        false,
                    )
                {
                    sh.insp_open = false;
                    super::shell::set(ui.ctx(), sh);
                }
                ScrollArea::vertical()
                    .id_salt(("right_body", s.ed.tab))
                    .max_height(ui.available_height())
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: 12,
                                right: 12,
                                top: if header { 2 } else { 12 },
                                bottom: 13,
                            })
                            .show(ui, |ui| {
                                ui.set_width(inspector_width - 2.0 - 24.0);
                                add(ui, s);
                            });
                    });
            });
            if !header {
                let b = egui::Rect::from_min_size(
                    r.response.rect.right_top() + vec2(-9.0 - 28.0, 9.0),
                    vec2(28.0, 28.0),
                );
                let resp = ui.interact(b, egui::Id::new("right_panel_hide"), egui::Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(b, CornerRadius::same(8), SOFT);
                }
                paint_icon(
                    ui,
                    b.center(),
                    "SidebarSimpleRightWide",
                    15.0,
                    if resp.hovered() { TEXT } else { TEXT_DIM },
                );
                if tip(resp, "Hide inspector", "", "Tab", TipSide::Below)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    sh.insp_open = false;
                    super::shell::set(ui.ctx(), sh);
                }
            }
        });
}

/// The right-hand inspector for the open tab, if it has one. Returns whether one is shown.
/// Each tab says when there's something to inspect (`*_inspector` → title, Halo header).
pub fn right_panel(
    ctx: &egui::Context,
    s: &mut UiState,
    out: &mut UiOut,
    _tex: &UiTextures,
    screen: egui::Rect,
) -> bool {
    let fixed = |t: &str| Some((t.to_string(), false));
    let hook: Option<(String, bool)> = match s.ed.tab {
        Tab::Land if s.ed.land_tool == LandTool::Reference => fixed("Reference picture"),
        Tab::Land | Tab::Sculpt | Tab::Paint => None,
        Tab::Caves => crate::caves::ui::caves_inspector(s),
        Tab::Nature if s.field_tool.on => crate::fields::ui::fields_inspector(s),
        Tab::Nature => Some(("Brush & wind".into(), true)),
        Tab::Objects => objects_tab::objects_inspector(s),
        Tab::Zones => zones_tab::zones_inspector(s),
        Tab::Lighting => Some(("Atmosphere & camera".into(), true)),
        Tab::Weather => Some(("Wind & quality".into(), true)),
        Tab::Style => Some(("Fine-tune".into(), true)),
        Tab::Audio => Some(("Ambience previews".into(), true)),
        Tab::Water => water_tab::water_inspector(s),
        Tab::Roads => roads_tab::roads_inspector(s),
        Tab::Gameplay => gameplay_tab::gameplay_inspector(s),
        Tab::Analyze => analyze_tab::analyze_inspector(s),
        Tab::Map => map_tab::map_inspector(s),
        Tab::Dungeons | Tab::DungeonLook | Tab::DungeonLights => {
            crate::dungeons::ui::dungeons_inspector(s)
        }
    };
    let Some((title, header)) = hook else {
        return false;
    };
    inspector(ctx, screen, s, &title, header, |ui, s| match s.ed.tab {
        Tab::Land | Tab::Sculpt | Tab::Paint => crate::refmap::ui::reference_right(ui, s),
        Tab::Caves => crate::caves::ui::caves_right(ui, s),
        Tab::Nature => nature_tab::nature_right(ui, s),
        Tab::Objects => objects_tab::objects_right(ui, s, out),
        Tab::Zones => zones_tab::zones_right(ui, s, out),
        Tab::Lighting => lighting_right(ui, s),
        Tab::Weather => weather_right(ui, s),
        Tab::Style => style_right(ui, s),
        Tab::Audio => audio_right(ui, s, out),
        Tab::Water => water_right(ui, s),
        Tab::Roads => roads_tab::roads_right(ui, s),
        Tab::Gameplay => gameplay_tab::gameplay_right(ui, s),
        Tab::Analyze => analyze_tab::analyze_right(ui, s),
        Tab::Map => map_tab::map_right(ui, s),
        Tab::Dungeons | Tab::DungeonLook | Tab::DungeonLights => {
            crate::dungeons::ui::dungeons_right(ui, s)
        }
    });
    true
}

/// The tool dock (bottom, between the panel and the inspector): the active tool, its settings
/// and its key hints.
pub fn dock(
    ctx: &egui::Context,
    s: &mut UiState,
    tex: &UiTextures,
    screen: egui::Rect,
    inspector: bool,
) {
    let cx = super::shell::dock_center_x(ctx, screen, inspector);
    egui::Area::new(egui::Id::new("halo_dock"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(cx, screen.bottom() - 12.0))
        .show(ctx, |ui| {
            glass(
                ui,
                glass_frame()
                    .corner_radius(CornerRadius::same(15))
                    .inner_margin(egui::Margin::symmetric(10, 6)),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.set_min_height(33.0);
                        let done = terrain_tab::dock(ui, s, tex)
                            || match s.ed.tab {
                                Tab::Water => water_tab::water_dock(ui, s),
                                Tab::Roads => roads_tab::roads_dock(ui, s),
                                Tab::Gameplay => gameplay_tab::gameplay_dock(ui, s),
                                Tab::Analyze => analyze_tab::analyze_dock(ui, s),
                                Tab::Map => map_tab::map_dock(ui, s),
                                Tab::Caves => crate::caves::ui::caves_dock(ui, s),
                                Tab::Nature if s.field_tool.on => {
                                    crate::fields::ui::fields_dock(ui, s)
                                }
                                Tab::Nature => nature_tab::nature_dock(ui, s),
                                Tab::Objects => objects_tab::objects_dock(ui, s),
                                Tab::Zones => zones_tab::zones_dock(ui, s),
                                Tab::Dungeons | Tab::DungeonLook | Tab::DungeonLights => {
                                    crate::dungeons::ui::dungeons_dock(ui, s)
                                }
                                _ => false,
                            };
                        if !done {
                            legacy_dock(ui, s);
                        }
                    });
                },
            );
        });
}

/// Tabs not moved over yet: their mouse hints in the dock (the settings stay in their panels).
fn legacy_dock(ui: &mut egui::Ui, s: &UiState) {
    use crate::editor::ObjectTool as OT;
    let place = s.ed.object_tool == OT::Place;
    let (lmb, alt): (String, &str) = match s.ed.tab {
        Tab::Caves => (crate::caves::ui::lmb_hint(&s.cave_tool).to_string(), ""),
        Tab::Water => ("Draw".into(), ""),
        Tab::Nature if s.field_tool.on => (
            "Drag a loop · plant a field   Click · select a field".into(),
            "",
        ),
        Tab::Nature => ("Paint".into(), "Thin"),
        Tab::Objects if s.scenes.capturing() && place => {
            ("Drag a loop · save what's inside as a scene".into(), "")
        }
        Tab::Objects if s.scenes.placing().is_some() && place => {
            ("Click · place the scene   Z / X · turn it".into(), "")
        }
        Tab::Objects if s.towns.selected.is_some() && place => {
            ("Click · place it   Drag a loop · fill an area".into(), "")
        }
        Tab::Objects if place && (s.sets.selected.is_some() || s.towns.selected.is_some()) => {
            ("Drag a loop · fill the area".into(), "")
        }
        Tab::Objects => ("Place / Select".into(), ""),
        Tab::Zones => ("Paint".into(), "Erase"),
        Tab::Gameplay => ("Place / Select".into(), ""),
        _ => (String::new(), ""),
    };
    let name = match s.ed.tab {
        Tab::Caves
        | Tab::Water
        | Tab::Roads
        | Tab::Gameplay
        | Tab::Zones
        | Tab::Analyze
        | Tab::Map
        | Tab::Dungeons
        | Tab::DungeonLook
        | Tab::DungeonLights => TabGroup::sub_name(s.ed.tab),
        Tab::Nature if s.field_tool.on => "Fields",
        t => t.name(),
    };
    dock_title(ui, super::shell::group_icon(s.ed.tab.group()), name);
    dock_sep(ui);
    let mut hints: Vec<(&str, &str)> = vec![];
    if !lmb.is_empty() {
        hints.push(("Click", &lmb));
    }
    if !alt.is_empty() {
        hints.push(("⌥", alt));
    }
    if s.orbit.in_cave() {
        hints.push(("RMB", "Look around"));
        hints.push(("WASD", "Walk the floor"));
        hints.push((
            "B",
            if s.work_light.on {
                "Light off"
            } else {
                "Work light"
            },
        ));
    }
    if s.ed.active_brush().is_some() {
        hints.push(("[ ]", "Size"));
    }
    if hints.is_empty() {
        hints.push(("RMB", "Orbit"));
        hints.push(("Scroll", "Zoom"));
    }
    dock_hints(ui, &hints);
}

/// File › Export server data… (the folder dialog answers on a later frame: see [`poll`]).
pub fn export_server_data(s: &mut UiState) {
    gameplay_tab::export_dialog(s);
}

/// Every frame, whatever tab is open: finish dialogs started from the menus.
pub fn poll(s: &mut UiState) {
    gameplay_tab::finish_export(s);
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
fn meters(v: f32) -> String {
    format!("{v:.0} m")
}

// ------------------------------------------------------------------ Land

/// The world-wide nature dials (instant).
pub fn nature_dials(ui: &mut egui::Ui, s: &mut UiState) {
    let before_appearance = *s.appearance;
    let mut appearance = before_appearance;
    toggle(ui, &mut appearance.forest.enabled, "Rich forest diversity")
        .on_hover_text("Adds biome-appropriate species to automatic forests. Painted and pinned trees keep their selected species.");
    if appearance.forest.enabled {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Forest layout").color(TEXT_DIM));
            for layout in crate::nature::ForestLayout::ALL {
                if ui
                    .selectable_label(appearance.forest.layout == layout, layout.name())
                    .clicked()
                {
                    appearance.forest.layout = layout;
                }
            }
        });
    }
    ui.add_space(6.0);
    slider_row(ui, "Grass lushness", &mut appearance.grass_lushness, 0.0..=1.0, pct)
        .on_hover_text("Controls grass height, fullness and the mix of fine blades and taller meadow tufts. 0% keeps the shorter grass.");
    if appearance != before_appearance {
        crate::nature::set_appearance(&mut s.appearance, &mut s.history, appearance);
        s.meta.dirty = true;
    }
    slider_row(ui, "Grass amount", &mut s.amounts.grass, 0.0..=2.0, pct).on_hover_text(
        "Controls ground coverage wherever grass can grow. 0% removes it; 200% fills the meadow more densely.",
    );
    toggle(ui, &mut s.motion.preview, "Preview wind while editing")
        .on_hover_text("Animates grass, leaves and the rest of the world while editing. Walking always enables motion. Wind speed is in the Wind controls.");
    let lush = s.layer_choices.get(0) == crate::terrain::textures::LUSH_GRASS;
    let mut pick = lush;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Grass ground").color(TEXT_DIM));
        segmented(
            ui,
            &mut pick,
            &[(true, "Lush lawn"), (false, "Natural soil")],
        );
    });
    if pick != lush {
        s.layer_choices.set(
            0,
            if pick {
                crate::terrain::textures::LUSH_GRASS
            } else {
                ""
            },
        );
    }
    ui.add_space(6.0);
    let a = &mut *s.amounts;
    for (v, label, hint) in [
        (
            &mut a.trees,
            "Trees",
            "How many trees the biomes and painted density grow, world-wide (instant).",
        ),
        (
            &mut a.rocks,
            "Rocks",
            "Boulders, rock clusters and pebbles, world-wide (instant).",
        ),
        (
            &mut a.plants,
            "Plants",
            "Ferns, bushes, grass tufts and reeds, world-wide (instant).",
        ),
        (
            &mut a.flowers,
            "Flowers",
            "Flower patches and flowering shrubs, world-wide (instant).",
        ),
        (
            &mut a.mushrooms,
            "Mushrooms",
            "Mushrooms and other fungi, world-wide (instant).",
        ),
        (
            &mut a.berries,
            "Berries",
            "Berry-bearing shrubs and ground cover, world-wide (instant).",
        ),
        (
            &mut a.debris,
            "Debris",
            "Fallen logs, stumps, roots and branches, world-wide (instant).",
        ),
    ] {
        slider_row(ui, label, v, 0.0..=2.0, pct).on_hover_text(hint);
    }
}

/// Climate and "living world" options for world generation.
pub fn biome_options(ui: &mut egui::Ui, s: &mut UiState) {
    toggle(ui, &mut s.ed.new_world_biomes, "Biomes and nature")
        .on_hover_text("Fill the land with climate-driven biomes (their ground, grass, forests, plants, rocks, weather and ambience). No zones are created");
    if s.ed.new_world_biomes {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Climate").color(TEXT_DIM));
            for c in crate::biomegen::Climate::ALL {
                if ui
                    .selectable_label(s.ed.new_world_climate == c, c.name())
                    .clicked()
                {
                    s.ed.new_world_climate = c;
                }
            }
        });
    }
}

// ------------------------------------------------------------------ Sculpt

// ------------------------------------------------------------------ Paint

/// A ground material in the Paint grid: a library scan (by id) or one of the built-in layer
/// textures (which can only live in their own slot).
#[derive(Clone, PartialEq)]
enum GroundMat {
    Library(String),
    BuiltIn(usize),
}

/// Round previews of the materials, decoded once (library `swatch.png`, built-in `<name>_swatch.png`).
fn mat_swatch(ctx: &egui::Context, m: &GroundMat) -> Option<egui::TextureId> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<String, Option<egui::TextureHandle>>>> = Mutex::new(None);
    let (key, path) = match m {
        GroundMat::Library(id) => (
            id.clone(),
            crate::terrain::textures::library_dir()
                .join(id)
                .join("swatch.png"),
        ),
        GroundMat::BuiltIn(i) => (
            format!("builtin:{i}"),
            crate::terrain::textures::texture_dir().join(format!(
                "{}_swatch.png",
                crate::terrain::textures::LAYER_FILES[*i]
            )),
        ),
    };
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.entry(key.clone())
        .or_insert_with(|| {
            let img = image::open(&path).ok()?.to_rgba8();
            let c = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            Some(ctx.load_texture(format!("mat_{key}"), c, egui::TextureOptions::LINEAR))
        })
        .as_ref()
        .map(|t| t.id())
}

/// How much of the world each slot covers (fractions of the land, painted or automatic).
fn layer_usage(t: &crate::terrain::Terrain) -> [f32; LAYER_COUNT] {
    let mut sum = [0.0f32; LAYER_COUNT];
    let n = t.n();
    let step = (n / 256).max(1);
    let mut count: f32 = 0.0;
    for j in (0..n).step_by(step) {
        for i in (0..n).step_by(step) {
            let w = t.final_weights(i, j);
            for k in 0..LAYER_COUNT {
                sum[k] += w[k];
            }
            count += 1.0;
        }
    }
    sum.map(|v| v / count.max(1.0))
}

/// Slots the automatic style is least likely to need, first: a free one of these takes a new
/// material.
const FREE_ORDER: [Layer; LAYER_COUNT] = [
    Layer::Ice,
    Layer::Cobblestone,
    Layer::DarkRock,
    Layer::Mud,
    Layer::DryGrass,
    Layer::ForestFloor,
    Layer::Gravel,
    Layer::Dirt,
    Layer::Snow,
    Layer::Sand,
    Layer::Rock,
    Layer::Grass,
];

/// Paint with a material: its slot if it's already in the world, else a free slot (one the world
/// doesn't use at all), else ask which one to replace.
fn pick_material(s: &mut UiState, m: &GroundMat) {
    s.ed.paint_tool = PaintTool::Paint;
    let slot_of = |s: &UiState| -> Option<usize> {
        match m {
            GroundMat::Library(id) => (0..LAYER_COUNT).find(|i| s.layer_choices.get(*i) == id),
            GroundMat::BuiltIn(i) => (s.layer_choices.get(*i).is_empty()).then_some(*i),
        }
    };
    if let Some(i) = slot_of(s) {
        s.ed.paint_layer = Layer::from_index(i);
        s.ed.paint_replace = None;
        return;
    }
    match m {
        GroundMat::BuiltIn(i) => {
            // a built-in texture only fits its own slot
            set_slot(s, *i, "");
            s.ed.toast(format!(
                "{} is back to its classic texture",
                Layer::from_index(*i).name()
            ));
        }
        GroundMat::Library(id) => {
            let usage = layer_usage(&s.terrain);
            match FREE_ORDER
                .iter()
                .map(|l| *l as usize)
                .find(|i| usage[*i] < 1e-4)
            {
                Some(i) => set_slot(s, i, id),
                None => s.ed.paint_replace = Some(id.clone()),
            }
        }
    }
}

/// Puts a material into a slot (it repaints wherever that slot is used) and paints with it.
fn set_slot(s: &mut UiState, i: usize, id: &str) {
    if let Some(m) = s.terrain_lib.materials.iter().find(|m| m.id == id)
        && let Some(surf) = crate::terrain::layers::Surface::ALL
            .iter()
            .find(|x| format!("{x:?}") == m.surface)
    {
        s.ed.layer_surface[i] = *surf;
    }
    s.layer_choices.set(i, id);
    s.ed.paint_layer = Layer::from_index(i);
    s.ed.paint_tool = PaintTool::Paint;
    s.ed.paint_replace = None;
    s.meta.dirty = true;
}

fn mat_name(s: &UiState, i: usize) -> String {
    let id = s.layer_choices.get(i);
    s.terrain_lib
        .materials
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| Layer::from_index(i).name().to_string())
}

// ------------------------------------------------------------------ Water

// (the Water tab lives in panels/water_tab.rs)
mod water_tab;
use water_tab::{water_left, water_right};

// ------------------------------------------------------------------ Nature

/// Whether a library entry matches a search: every word in its name, id, group or category.
fn asset_matches(e: &crate::objects::LibraryEntry, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let hay = format!("{} {} {} {}", e.name, e.id, e.group, e.category.name()).to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

/// The asset tiles (rendered thumbnails, crate::thumbs) of `categories`, narrowed to a group
/// (`"-"`: entries without one), a style pack (empty: every style; imported models always
/// show) and a search. `scenes`: the complete scenes in one model (Objects › Scenes › Ready-made
/// scenes) instead of single objects.
#[allow(clippy::too_many_arguments)]
fn asset_grid(
    ui: &mut egui::Ui,
    s: &mut UiState,
    categories: &[Category],
    group: &str,
    style: &str,
    search: &str,
    scenes: bool,
    is_selected: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let mut clicked = None;
    let Some(lib) = &s.lib else {
        ui.spinner();
        return None;
    };
    let words: Vec<String> = search
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    // (assets in packs this account hasn't got show a lock; style "" is the Nature tab: no packs)
    let market = crate::market::snapshot();
    // (the Nature tab has no packs: no pack or price filters there; ready-made scenes show
    // from every pack, owned or not)
    let shop = s.ed.tab == crate::editor::Tab::Objects && !scenes;
    let pack = if shop {
        crate::market::ui::filter(s, &market)
    } else {
        None
    };
    let price = if shop {
        s.market.price
    } else {
        crate::market::ui::PriceFilter::All
    };
    let items: Vec<(String, String, String, Category, crate::market::Access)> = lib
        .entries
        .iter()
        .filter(|e| {
            e.whole_scene == scenes
                && categories.contains(&e.category)
                && (group.is_empty() || e.group == group || (group == "-" && e.group.is_empty()))
                && in_style(e, style)
                && asset_matches(e, &words)
        })
        .filter(|e| {
            pack.is_none_or(|p| {
                market.pack_index_of(&e.id) == Some(p) || e.category == Category::Imported
            })
        })
        .filter(|e| price.keeps_asset(&market, &e.id))
        .map(|e| {
            (
                e.id.clone(),
                e.name.clone(),
                format!("{}\nFootsteps: {}", e.name, e.surface.name()),
                e.category,
                market.access(&e.id),
            )
        })
        .collect();
    if items.is_empty() {
        let msg = if scenes {
            if words.is_empty() {
                "No ready-made scenes yet.".to_string()
            } else {
                format!("No ready-made scene matches “{}”.", search.trim())
            }
        } else if !words.is_empty() {
            format!("Nothing matches “{}”.", search.trim())
        } else if categories == [Category::Imported] {
            "No imported models yet.".into()
        } else if price != crate::market::ui::PriceFilter::All {
            format!(
                "No {} {} here.",
                if price == crate::market::ui::PriceFilter::Free {
                    "free"
                } else {
                    "paid"
                },
                categories[0].name().to_lowercase()
            )
        } else if !style.is_empty() {
            format!(
                "No {} {} here.",
                crate::objects::pack_label(style).to_lowercase(),
                categories[0].name().to_lowercase()
            )
        } else {
            "Nothing here yet.".into()
        };
        dim(ui, &msg);
        return None;
    }
    // three to a row; tiles out of view cost nothing (and render their thumbnails last)
    let w = ((ui.available_width() - 14.0) / 3.0).floor();
    egui::Grid::new(("asset_grid", categories[0] as u8, scenes))
        .num_columns(3)
        .spacing(vec2(7.0, 7.0))
        .show(ui, |ui| {
            for (n, (id, name, hover, cat, access)) in items.iter().enumerate() {
                let subject = crate::thumbs::Subject::Asset(id.clone());
                let r = crate::thumbs::ui::tile(
                    ui,
                    &mut s.previews,
                    &subject,
                    category_icon(*cat),
                    name,
                    is_selected(id),
                    w,
                );
                crate::market::ui::badge(ui, r.rect, *access);
                match access {
                    // window shopping: a locked tile opens its pack's card instead of placing
                    crate::market::Access::Locked(p) => {
                        if crate::market::ui::locked_hover(r, name, p).clicked() {
                            s.market.card = Some(p.id.clone());
                        }
                    }
                    _ => {
                        if r.on_hover_text(hover).clicked() {
                            clicked = Some(id.clone());
                        }
                    }
                }
                if n % 3 == 2 {
                    ui.end_row();
                }
            }
        });
    clicked
}

// ------------------------------------------------------------------ Objects

/// Whether a library entry belongs to a style pack filter (empty: every style). Imported models
/// and the procedural plants whose textures follow the world's style show under every style.
fn in_style(e: &crate::objects::LibraryEntry, style: &str) -> bool {
    crate::art_style::fits(e, style)
}

// ------------------------------------------------------------------ Zones

fn zone_changed(a: &Zone, b: &Zone) -> bool {
    a != b
}

/// The zone's own lighting & style: a mood, an art style and its light fine-tuned. Returns
/// true when "go there" was clicked (to see it, the camera has to be inside the zone).
fn zone_lighting(ui: &mut egui::Ui, z: &mut Zone, viewer_zone: u8) -> bool {
    let mut go = false;
    crate::ui::tips::help_button(ui, "zone_lighting");
    crate::ui::tips::card(
        ui,
        "zone_lighting",
        "Give this zone its own feel",
        &[
            "Tick “Own lighting & style”, then pick a mood: brighter, more alive, misty, darker…",
            "Optionally switch the zone to another art style, and fine-tune its brightness, contrast, colour and fog.",
            "It blends in over a few seconds as the camera or player enters the zone, and fades back out as they leave.",
        ],
    );
    toggle(ui, &mut z.mood_enabled, "Own lighting & style");
    if !z.mood_enabled {
        dim(
            ui,
            "Off: the world's light and art style (or the biome's mood) apply here.",
        );
        return false;
    }
    ui.add_space(2.0);
    ui.label(RichText::new("Mood").color(TEXT));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for m in Mood::ALL {
            if chip(ui, m.name(), None, z.mood == m)
                .on_hover_text(m.blurb())
                .clicked()
            {
                z.mood = m;
                z.light = None;
            }
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} Art style", icon::PALETTE)).color(TEXT));
        let same = "Same as the world";
        egui::ComboBox::from_id_salt("zone_style")
            .width(180.0)
            .selected_text(z.style.map_or(same, |p| p.name()))
            .dropdown_ui(ui, |ui| {
                if menu_check(ui, z.style.is_none(), same).clicked() {
                    z.style = None;
                }
                for p in LookPreset::ALL {
                    ui.horizontal(|ui| {
                        palette_strip(ui, p, vec2(28.0, 12.0));
                        if menu_check(ui, z.style == Some(p), p.name())
                            .on_hover_text(p.blurb())
                            .clicked()
                        {
                            z.style = Some(p);
                        }
                    });
                }
            });
    });
    if z.style.is_some() {
        let mut v = z.style_strength * 100.0;
        if slider_row(ui, "Style strength", &mut v, 0.0..=100.0, |v| {
            format!("{v:.0}%")
        })
        .changed()
        {
            z.style_strength = v / 100.0;
        }
        dim(
            ui,
            "The zone takes on this style's colours, light and haze; the time of day stays the world's.",
        );
    }
    ui.add_space(4.0);
    let mut l = z.light.unwrap_or_else(|| z.mood.light());
    let mut changed = false;
    let x = |v: f32| format!("{v:.2}×");
    let signed = |v: f32| format!("{v:+.2}");
    changed |= slider_row(
        ui,
        "Brightness (darker ↔ brighter)",
        &mut l.brightness,
        -3.0..=2.0,
        |v| format!("{v:+.1} EV"),
    )
    .changed();
    changed |= slider_row(ui, "Contrast", &mut l.contrast, 0.6..=1.5, x).changed();
    changed |= slider_row(
        ui,
        "Colour (muted ↔ vivid)",
        &mut l.saturation,
        0.0..=2.0,
        x,
    )
    .changed();
    changed |= slider_row(
        ui,
        "Warmth (cool ↔ warm)",
        &mut l.warmth,
        -1.0..=1.0,
        signed,
    )
    .changed();
    if z.haze.mode != crate::haze::HazeMode::Off {
        changed |= slider_row(ui, "Haze & fog", &mut l.fog, 0.0..=1.0, pct).changed();
    } else {
        hint_small(ui, "Zone haze is off; its fog settings are kept for later.");
    }
    egui::CollapsingHeader::new(
        RichText::new(format!("{} More light controls", icon::SLIDERS_HORIZONTAL)).color(TEXT_DIM),
    )
    .id_salt("zone_light_more")
    .default_open(false)
    .show(ui, |ui| {
        changed |= slider_row(ui, "Sunlight", &mut l.sun, 0.0..=2.0, x).changed();
        changed |= slider_row(
            ui,
            "Shadow fill (deep ↔ soft)",
            &mut l.shadow_fill,
            0.2..=2.5,
            x,
        )
        .changed();
        changed |= slider_row(
            ui,
            "Tint (green ↔ magenta)",
            &mut l.tint,
            -1.0..=1.0,
            signed,
        )
        .changed();
        if z.haze.mode != crate::haze::HazeMode::Off {
            changed |= tint_row(ui, "Fog colour", &mut l.fog_tint);
        }
        changed |= slider_row(ui, "Glow", &mut l.glow, 0.0..=1.0, pct).changed();
        changed |= slider_row(
            ui,
            "Vignette (dark corners)",
            &mut l.vignette,
            0.0..=1.0,
            pct,
        )
        .changed();
    });
    if changed {
        z.light = Some(l.clamped());
    }
    if z.light.is_some() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} · fine-tuned", z.mood.name()))
                    .small()
                    .color(ACCENT),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(format!("{} Reset to mood", icon::ARROW_COUNTER_CLOCKWISE))
                    .clicked()
                {
                    z.light = None;
                }
            });
        });
    }
    ui.add_space(2.0);
    if viewer_zone == z.id {
        dim(ui, "You're in this zone: changes show live.");
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("The camera is outside this zone.")
                    .small()
                    .color(Color32::from_rgb(240, 190, 90)),
            );
            if ui
                .small_button(format!("{} Go there to preview", icon::NAVIGATION_ARROW))
                .clicked()
            {
                go = true;
            }
        });
    }
    go
}

// ------------------------------------------------------------------ Lighting

fn lighting_left(ui: &mut egui::Ui, s: &mut UiState) {
    // time presets (the environment pill above plays the day)
    let presets: [(&str, f32, &str); 5] = [
        ("Dawn", 6.2, "Sunrise"),
        ("Morning", 9.0, "SunLow"),
        ("Noon", 12.5, "SunHigh"),
        ("Dusk", 18.6, "Sunset"),
        ("Night", 23.0, "Moon"),
    ];
    let items: Vec<(&str, &str, &str)> = presets.iter().map(|(n, _, i)| (*i, *n, "")).collect();
    let near = presets
        .iter()
        .position(|(_, h, _)| (s.lighting.hour - h).abs() < 0.05);
    if let Some(i) = tool_grid(ui, 5, &items, near) {
        s.lighting.hour = presets[i].1;
    }
    slider_row(ui, "Time of day", &mut s.lighting.hour, 0.0..=23.99, |h| {
        format!("{:02}:{:02}", h as u32, (h.fract() * 60.0) as u32)
    });
    slider_row(
        ui,
        "Day length (preview)",
        &mut s.lighting.day_minutes,
        0.5..=60.0,
        |v| format!("{v:.1} min"),
    );
    section_fold(ui, "light_sun", "Sun & moon", "", |ui| {
        slider_row(
            ui,
            "Sun path tilt",
            &mut s.lighting.sun_tilt,
            5.0..=75.0,
            |v| format!("{:.0}° noon", 90.0 - v),
        );
        slider_row(
            ui,
            "Sunrise direction",
            &mut s.lighting.sun_azimuth,
            -180.0..=180.0,
            |v| format!("{v:.0}°"),
        );
        slider_row(
            ui,
            "Sun strength",
            &mut s.lighting.sun_strength,
            0.2..=2.0,
            |v| format!("{v:.2}"),
        );
        slider_row(
            ui,
            "Moonlight",
            &mut s.lighting.moon_strength,
            0.0..=3.0,
            |v| format!("{v:.2}"),
        );
        slider_row(
            ui,
            "Moon phase",
            &mut s.lighting.moon_phase,
            0.0..=1.0,
            |v| {
                let names = [
                    "New moon",
                    "Waxing crescent",
                    "First quarter",
                    "Waxing gibbous",
                    "Full moon",
                    "Waning gibbous",
                    "Last quarter",
                    "Waning crescent",
                ];
                names[((v * 8.0 + 0.5).floor() as usize) % 8].to_string()
            },
        );
        slider_row(
            ui,
            "Night brightness",
            &mut s.lighting.night_brightness,
            0.0..=3.0,
            |v| format!("{v:.2}"),
        );
        slider_row(
            ui,
            "Exposure",
            &mut s.lighting.exposure_comp,
            -3.0..=3.0,
            |v| format!("{v:+.1} EV"),
        );
        toggle(ui, &mut s.lighting.stars, "Stars at night");
    });
}

/// World wind: speed, direction, gustiness (zones can override the speed).
fn wind_controls(ui: &mut egui::Ui, s: &mut UiState) {
    slider_row(ui, "Wind speed", &mut s.wind.speed, 0.0..=25.0, |v| {
        format!("{v:.1} m/s · {}", crate::wind::describe(v))
    });
    let mut ang = s.wind.dir.y.atan2(s.wind.dir.x).to_degrees();
    if slider_row(ui, "Wind direction", &mut ang, -180.0..=180.0, |v| {
        format!("{v:.0}°")
    })
    .changed()
    {
        let a = ang.to_radians();
        s.wind.dir = Vec2::new(a.cos(), a.sin());
    }
    slider_row(ui, "Gustiness", &mut s.wind.gustiness, 0.0..=1.0, pct);
    dim(
        ui,
        "Trees, grass, banners, smoke and the sea follow the wind. Zones can set their own wind speed.",
    );
}

fn lighting_right(ui: &mut egui::Ui, s: &mut UiState) {
    toggle(ui, &mut s.lighting.haze_enabled, "Haze").on_hover_text(
        "Outdoor distance fog, atmospheric colour haze, lit air, mist and sun rays. Turning it off keeps these settings ready to restore; zones may override this setting. Dungeon and underwater fog stay independent.",
    );
    if s.lighting.haze_enabled {
        haze_controls(ui, s);
    }
    hint_small(
        ui,
        "Painted zones can use this world setting or override haze on or off.",
    );
    hint_small(ui, "Clouds, wind, rain and snow are in the Weather tab.");
    section_fold(ui, "light_shadows", "Shadows", "", |ui| {
        segmented(
            ui,
            &mut s.lighting.shadow_quality,
            &[
                (ShadowQuality::Low, "Low"),
                (ShadowQuality::Medium, "Medium"),
                (ShadowQuality::High, "High"),
            ],
        );
        toggle(ui, &mut s.lighting.contact_shadows, "Contact shadows (fine detail)")
            .on_hover_text("Small, sharp shadows where things touch the ground, seen up close. Costs about a tenth of the frame rate.");
        let opts: Vec<(crate::camera::AmbientOcclusion, &str)> =
            crate::camera::AmbientOcclusion::ALL
                .iter()
                .map(|a| (*a, a.name()))
                .collect();
        dropdown_row(ui, "Ambient occlusion", "light_ao", s.lighting.ambient_occlusion.name(), |ui| {
            for (a, n) in &opts {
                menu_option(ui, &mut s.lighting.ambient_occlusion, *a, *n);
            }
        })
        .on_hover_text("Soft shade in creases, doorways and on the ground round buildings, rocks and trees, so they sit in the world instead of floating on it. Higher is smoother and costs more; how strong it is belongs to the art style (Style › Fine-tune › Ambient occlusion).");
        toggle(ui, &mut s.lighting.bounce_light, "Bounce light").on_hover_text(
            "While walking: sunlit walls, streets and squares throw warm light into the shade round them, and narrow lanes, arcades and rooms get darker where buildings hide the sky. \
Worked out in the background round you as you go; open ground looks the same either way. The editor view keeps the plain sky light.",
        );
        hint_small(
            ui,
            "Cascaded sun shadows refit to the camera distance automatically: crisp up close, wide from above.",
        );
    });
    section_fold(ui, "light_camera", "Camera", "", |ui| {
        camera_controls(ui, s)
    });
    section_fold(ui, "light_perf", "Performance", "", |ui| {
        performance_controls(ui, s)
    });
}

/// Haze has one master switch in Lighting; these values remain stored while that switch is off.
fn haze_controls(ui: &mut egui::Ui, s: &mut UiState) {
    slider_row(ui, "Haze amount", &mut s.lighting.haze, 0.0..=1.0, pct);
    let look = &mut s.lighting.look;
    let before = look.base();
    let mut p = before;
    let x = |v: f32| format!("{v:.2}×");
    slider_row(ui, "Haze density", &mut p.haze, 0.0..=4.0, x);
    slider_row(ui, "Colour haze", &mut p.color_haze, 0.0..=1.0, pct);
    color_row(ui, "Haze colour", &mut p.haze_color);
    slider_row_log(
        ui,
        "Haze distance",
        &mut p.haze_distance,
        150.0..=6000.0,
        |v| format!("{v:.0} m"),
    );
    slider_row(ui, "Sun rays", &mut p.sunbeams, 0.0..=1.5, pct);
    slider_row(ui, "Hazy air", &mut p.air, 0.0..=3.0, x).on_hover_text(
        "How much haze hangs in the lit air: what sunbeams, the glow round the sun and the halos round lamps are made of. Needs Lighting › Volumetric light.",
    );
    slider_row(ui, "Ground mist", &mut p.mist, 0.0..=1.0, pct).on_hover_text(
        "Mist lying a few metres over the ground, thickest in hollows. Weather fog and wet ground add their own.",
    );
    if !p.approx_eq(&before) {
        look.custom = Some(p);
    }
}

/// Lighting › Camera: the eye and the lens (`camera_fx`).
fn camera_controls(ui: &mut egui::Ui, s: &mut UiState) {
    ui.label("Eye adaptation").on_hover_text(
        "The exposure follows what you look at, the way eyes adjust: step into a shaded lane or a building and it opens up over a couple of seconds; back out in the sun it dazzles for a moment. \
Walking: while walking and in camera paths (the editor view keeps a fixed exposure, so you can judge the lighting). Always: the editor view too, gently.",
    );
    let opts: Vec<(crate::camera_fx::EyeAdaptation, &str)> = crate::camera_fx::EyeAdaptation::ALL
        .iter()
        .map(|a| (*a, a.name()))
        .collect();
    segmented(ui, &mut s.lighting.eye_adaptation, &opts);
    ui.add_space(4.0);
    ui.label("Depth of field").on_hover_text(
        "Cinematic focus blur, like a real lens: only in camera paths (and with Always the editor view, for stills), never while walking. Costs frame rate while it shows.",
    );
    let opts: Vec<(crate::camera_fx::DepthOfFieldMode, &str)> =
        crate::camera_fx::DepthOfFieldMode::ALL
            .iter()
            .map(|m| (*m, m.name()))
            .collect();
    segmented(ui, &mut s.lighting.depth_of_field.mode, &opts);
    if s.lighting.depth_of_field.mode != crate::camera_fx::DepthOfFieldMode::Off {
        let d = &mut s.lighting.depth_of_field;
        slider_row_log(ui, "Aperture", &mut d.f_stops, 0.7..=22.0, |v| {
            format!("f/{v:.1}")
        })
        .on_hover_text(
            "Lower: shallower focus, more blur. Close shots and a narrow field of view blur most.",
        );
        let mut auto = d.focus <= 0.0;
        if toggle(ui, &mut auto, "Auto focus (what's at the centre)").changed() {
            d.focus = if auto { 0.0 } else { 20.0 };
        }
        if !auto {
            slider_row_log(ui, "Focus distance", &mut d.focus, 0.5..=5000.0, |v| {
                format!("{v:.1} m")
            });
        }
    }
}

fn weather_left(ui: &mut egui::Ui, s: &mut UiState) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), egui::Sense::hover());
        let here = weather_here(s);
        let ic = if here.to_lowercase().contains("snow") {
            "SnowFlakes"
        } else if here.to_lowercase().contains("rain") || here.to_lowercase().contains("storm") {
            "Rainy"
        } else {
            "CloudySun"
        };
        paint_icon(ui, r.center(), ic, 15.0, TEXT);
        ui.add(egui::Label::new(RichText::new(here).size(12.5).color(TEXT)).wrap());
    });
    toggle(ui, &mut s.lighting.precipitation, "Rain and snow").on_hover_text("Off: nothing falls anywhere and the ground stays dry. Zones and biomes keep their clouds, fog and mood.");
    let cur = s
        .lighting
        .weather_override
        .map(|w| w.name())
        .unwrap_or("Follow zones & biomes");
    dropdown_row(ui, "Weather", "weather_override", cur, |ui| {
        menu_option(
            ui,
            &mut s.lighting.weather_override,
            None,
            "Follow zones & biomes",
        );
        for w in Weather::ALL {
            menu_option(ui, &mut s.lighting.weather_override, Some(w), w.name());
        }
    });
    hint_small(
        ui,
        "Zones set their own weather; elsewhere each biome brings its typical weather (rain in jungles, fog in swamps, snow on tundra).",
    );
    section_fold(ui, "weather_rain", "Rain", "", |ui| {
        slider_row(ui, "Puddles", &mut s.lighting.rain.puddles, 0.0..=1.0, pct)
            .on_hover_text("How much standing water the rain leaves on flat earth, mud, gravel, cobbles and roads. Puddles fill as it keeps raining and linger after it stops.");
        slider_row(
            ui,
            "Lightning",
            &mut s.lighting.rain.lightning,
            0.0..=3.0,
            |v| {
                if v <= 0.0 {
                    "Never".into()
                } else {
                    format!("{v:.1}×")
                }
            },
        )
        .on_hover_text(
            "How often lightning strikes in thunderstorms (now and then in heavy rain too).",
        );
        toggle(ui, &mut s.lighting.rain.thunder, "Thunder").on_hover_text(
            "Thunder rolls in after each strike: later and softer the further away it struck.",
        );
        hint_small(
            ui,
            "Ground, roads and buildings get wet in a minute of rain and dry over a few minutes after.",
        );
    });
    section_fold(ui, "weather_air", "Air", "", |ui| {
        crate::visuals::air::air_controls(ui, &mut s.lighting.air)
    });
    section_fold(ui, "weather_clouds", "Clouds", "", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for p in crate::clouds::CloudPreset::ALL {
                if chip(ui, p.name(), None, false).clicked() {
                    p.apply(&mut s.lighting, fastrand::u64(..));
                }
            }
        });
        if wide_icon_button(ui, "Shuffle", "Randomize sky")
            .on_hover_text("A random pleasant sky: cover, cloud type, density, cirrus and pattern")
            .clicked()
        {
            crate::clouds::randomize_clouds(&mut s.lighting, fastrand::u64(..));
        }
        slider_row(ui, "Cover", &mut s.lighting.cloud_coverage, 0.0..=1.0, pct);
        slider_row(
            ui,
            "Cloud type",
            &mut s.lighting.cloud_type,
            0.0..=1.0,
            |v| {
                (if v < 0.3 {
                    "Stratus"
                } else if v < 0.65 {
                    "Stratocumulus"
                } else {
                    "Cumulus"
                })
                .to_string()
            },
        );
        slider_row(
            ui,
            "Thickness",
            &mut s.lighting.cloud_density,
            0.3..=2.0,
            |v| format!("{v:.2}"),
        );
        slider_row(ui, "Cirrus", &mut s.lighting.cirrus, 0.0..=1.0, pct);
        slider_row(ui, "Speed", &mut s.lighting.cloud_speed, 0.0..=5.0, |v| {
            format!("{v:.1}×")
        });
        slider_row(ui, "Shadows", &mut s.lighting.cloud_shadows, 0.0..=1.0, pct);
        hint_small(ui, "Zones can set their own cloud cover.");
    });
}

/// The weather where the camera is, and why (the same order as `sky::update_weather`: the
/// weather everywhere, else the zone's own, else the biome's typical weather).
fn weather_here(s: &UiState) -> String {
    let doing = |w: Weather| match w {
        Weather::Rain => "Raining here",
        Weather::Storm => "Storming here",
        Weather::Snow => "Snowing here",
        Weather::Fog => "Foggy here",
        Weather::Sandstorm => "A sandstorm here",
        Weather::Cloudy => "Cloudy here",
        Weather::Clear => "Clear here",
    };
    let falls = |w: Weather| matches!(w, Weather::Rain | Weather::Storm | Weather::Snow);
    let off = |w: Weather| {
        if falls(w) && !s.lighting.precipitation {
            " (Rain and snow are off: nothing falls)"
        } else {
            ""
        }
    };
    if let Some(w) = s.lighting.weather_override {
        return format!(
            "{} everywhere: chosen above{}",
            doing(w).trim_end_matches(" here"),
            off(w)
        );
    }
    if let Some(z) = s
        .zones
        .by_id(s.sky.viewer_zone)
        .filter(|z| z.weather_enabled)
    {
        return format!(
            "{} because: the {} zone{}",
            doing(z.weather),
            z.name,
            off(z.weather)
        );
    }
    let p = s.sky.viewer_pos;
    match crate::biomes::BiomePreset::from_id(s.terrain.biome_at(p.x, p.z)).map(|b| b.def()) {
        Some(d) if d.weather == Weather::Clear => format!("Clear here ({} biome)", d.name),
        Some(d) => format!(
            "{} because: {} biome{}",
            doing(d.weather),
            d.name,
            off(d.weather)
        ),
        None => "Clear here".into(),
    }
}

fn weather_right(ui: &mut egui::Ui, s: &mut UiState) {
    wind_controls(ui, s);
    section_fold(ui, "weather_perf", "Performance", "", |ui| {
        let mut q = s.lighting.cloud_quality;
        let opts: Vec<(crate::clouds::CloudQuality, &str)> = crate::clouds::CloudQuality::ALL
            .iter()
            .map(|q| (*q, q.name()))
            .collect();
        ui.label(RichText::new("Cloud quality").color(TEXT_DIM));
        if segmented(ui, &mut q, &opts) {
            s.lighting.cloud_quality = q;
        }
        hint_small(
            ui,
            "How much the volumetric clouds cost: Low is cheapest on weak GPUs.",
        );
    });
}

fn performance_controls(ui: &mut egui::Ui, s: &mut UiState) {
    slider_row(ui, "Draw distance", &mut s.graphics.draw_distance, 500.0..=8000.0, |v| format!("{:.1} km", v / 1000.0))
        .on_hover_text("How far anything is drawn: tree billboards, big rocks, buildings. Lower it for weaker GPUs.");
    slider_row(
        ui,
        "Detail distance",
        &mut s.graphics.detail,
        0.5..=2.0,
        |v| format!("{v:.2}×"),
    )
    .on_hover_text(
        "How far full-detail trees, plants and rocks reach before simpler versions take over.",
    );
    ui.add_space(4.0);
    let opts: Vec<(crate::render_scale::RenderResolution, &str)> =
        crate::render_scale::RenderResolution::ALL
            .iter()
            .map(|r| (*r, r.label()))
            .collect();
    let cur = s.render_scale.setting.label();
    dropdown_row(ui, "Render resolution", "render_resolution", cur, |ui| {
        for (r, l) in &opts {
            menu_option(ui, &mut s.render_scale.setting, *r, *l);
        }
    })
    .on_hover_text("How many pixels the 3D view draws before it is scaled up to the window. Lower is faster; the panels always stay sharp. Saved for this computer, not with the world.");
    let px = s.render_scale.pixels;
    dim(
        ui,
        &format!(
            "{} × {} pixels ({:.0}%). Auto uses 75% on Retina and 5K displays.",
            px.x,
            px.y,
            s.render_scale.active * 100.0
        ),
    );
    ui.add_space(4.0);
    ui.label("Anti-aliasing").on_hover_text("Sharp smooths edges within each frame: crisp, but leaves, grass and thin lines shimmer as the camera moves. Smooth blends frames over time: steady foliage and softer shading noise, a touch less crisp.");
    let opts: Vec<(crate::camera::AntiAliasing, &str)> = crate::camera::AntiAliasing::ALL
        .iter()
        .map(|a| (*a, a.name()))
        .collect();
    segmented(ui, &mut s.lighting.anti_aliasing, &opts);
    ui.add_space(4.0);
    toggle(
        ui,
        &mut s.lighting.forest_contours.enabled,
        "Soften distant forests",
    )
    .on_hover_text(
        "Blends dense tree contours in the distance while keeping foreground trees crisp.",
    );
    if s.lighting.forest_contours.enabled {
        hint_small(
            ui,
            "Fades dense tree contours into the distance; foreground trees stay crisp.",
        );
        slider_row(
            ui,
            "Softness",
            &mut s.lighting.forest_contours.strength,
            0.0..=1.0,
            pct,
        );
        slider_row(
            ui,
            "Starts at",
            &mut s.lighting.forest_contours.start_distance,
            crate::forest_contours::ForestContours::MIN_START_DISTANCE
                ..=crate::forest_contours::ForestContours::MAX_START_DISTANCE,
            |v| format!("{v:.0} m"),
        );
    }
    ui.add_space(4.0);
    ui.label("Volumetric light").on_hover_text(
        "Lit air: sunbeams through trees, buildings and windows, the glow round the sun, ground mist and fog the light shines into, halos round lamps. \
Off draws plain distance fog. Higher draws finer shafts for a little more GPU time (it doesn't grow with the window's pixels).",
    );
    let opts: Vec<(crate::volumetrics::VolumetricQuality, &str)> =
        crate::volumetrics::VolumetricQuality::ALL
            .iter()
            .map(|q| (*q, q.name()))
            .collect();
    segmented(ui, &mut s.lighting.volumetrics, &opts);
    toggle(ui, &mut s.background.0, "Save power in the background").on_hover_text(
        "While another app is in front, the editor updates 10 times a second instead of 60, so it barely uses the CPU or GPU. \
An agent working through MCP or a flythrough capture keeps it at full speed. Saved for this computer.",
    );
    toggle(ui, &mut s.motion.preview, "Preview motion while editing");
    dim(
        ui,
        &format!(
            "Only objects near the camera are streamed in ({} of {} now); distant trees are merged billboards. Both settings are saved and exported for the game.",
            s.store.entities.len(),
            s.store.count()
        ),
    );
}

// ------------------------------------------------------------------ Style

/// Strip of the look's four signature colours.
pub fn palette_strip(ui: &mut egui::Ui, p: LookPreset, size: egui::Vec2) {
    let (r, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let w = r.width() / 4.0;
    for (i, c) in p.palette().iter().enumerate() {
        let cr = egui::Rect::from_min_size(r.min + vec2(i as f32 * w, 0.0), vec2(w, r.height()));
        let round = match i {
            0 => CornerRadius {
                nw: 6,
                sw: 6,
                ne: 0,
                se: 0,
            },
            3 => CornerRadius {
                nw: 0,
                sw: 0,
                ne: 6,
                se: 6,
            },
            _ => CornerRadius::ZERO,
        };
        ui.painter()
            .rect_filled(cr, round, Color32::from_rgb(c[0], c[1], c[2]));
    }
}

fn look_picture(p: LookPreset) -> &'static str {
    match p.name() {
        "Nordic Mist" => "look_nordic",
        "Cinematic" => "look_cinematic",
        "Epic Sunbeams" => "look_epic_sunbeams",
        "Overcast" => "look_overcast",
        "Lavender Dream" => "look_lavender",
        "Golden Vintage" => "look_golden_vintage",
        "Rose Haze" => "look_rose_haze",
        "Hellfire Dusk" => "look_hellfire",
        "Enchanted Grove" => "look_enchanted",
        "Golden Hour" => "look_golden_hour",
        _ => "look_natural",
    }
}
fn style_left(ui: &mut egui::Ui, s: &mut UiState) {
    section_fold(ui, "style_art", "World art", "", |ui| world_art(ui, s));
    section_fold(
        ui,
        "style_look",
        "Look",
        &LookPreset::ALL.len().to_string(),
        |ui| {
            hint_small(
                ui,
                "Switch the whole world's look in one click. Styles hold through the day/night cycle and weather; zone moods still tint their zones on top.",
            );
            let ctx = ui.ctx().clone();
            let all = LookPreset::ALL;
            let w = ((ui.available_width() - 5.0) / 2.0).floor();
            let cur = s.lighting.look.preset;
            let edited = s.lighting.look.custom.is_some();
            let mut pick = None;
            for row in all.chunks(2) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    for p in row {
                        let sel = cur == *p;
                        let sub = if sel && edited {
                            format!("{} · edited", p.inspired_by())
                        } else {
                            p.inspired_by().to_string()
                        };
                        let r = image_card(
                            ui,
                            crate::ui::pictures::get(&ctx, look_picture(*p)),
                            p.name(),
                            &sub,
                            sel,
                            vec2(w, w * 0.625),
                        )
                        .on_hover_text(p.blurb());
                        if r.clicked() && !(sel && !edited) {
                            pick = Some(*p);
                        }
                    }
                });
            }
            if let Some(p) = pick {
                s.lighting.look.select(p);
                // scene templates bring their time of day, sky and haze along
                if let Some((hour, clouds, haze)) = p.scene() {
                    s.lighting.hour = hour;
                    clouds.apply(&mut s.lighting, fastrand::u64(..));
                    s.lighting.haze = haze;
                }
            }
            let mut pct_v = s.lighting.look.strength * 100.0;
            if slider_row(ui, "Strength", &mut pct_v, 0.0..=100.0, |v| {
                format!("{v:.0}%")
            })
            .changed()
            {
                s.lighting.look.strength = pct_v / 100.0;
            }
            hint_small(ui, "Hold \\ to compare with Natural · L / ⇧L cycles styles");
        },
    );
    section_fold(ui, "style_mine", "My styles", "", |ui| my_styles(ui, s));
}

/// Style › World art: Realistic or Hand-painted (`crate::art_style`): what the ground, rock,
/// trees and buildings are painted with and which assets the builder offers. One undo step.
fn world_art(ui: &mut egui::Ui, s: &mut UiState) {
    use crate::art_style::ArtStyle;
    hint_small(
        ui,
        "What the ground, rocks, trees and buildings are painted with. Objects and Nature then offer only this style's assets.",
    );
    let ctx = ui.ctx().clone();
    let w = ((ui.available_width() - 5.0) / 2.0).floor();
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        for a in ArtStyle::ALL {
            let selected = s.meta.art_style == a;
            let (pic, sub) = match a {
                ArtStyle::Realistic => ("art_photoreal", "Photoscanned ground, rock, bark"),
                ArtStyle::HandPainted => ("art_handpainted", "Painted surfaces, classic MMO"),
            };
            if image_card(
                ui,
                crate::ui::pictures::get(&ctx, pic),
                a.name(),
                sub,
                selected,
                vec2(w, w * 0.625),
            )
            .on_hover_text(a.blurb())
            .clicked()
                && !selected
            {
                pick = Some(a);
            }
        }
    });
    if let Some(a) = pick
        && crate::art_style::set_style(&mut s.meta, &mut s.lighting, &mut s.history, a)
    {
        s.ed.toast(format!(
            "{} world: the surfaces repaint and the builder offers {} assets (⌘Z undoes)",
            a.name(),
            a.name().to_lowercase()
        ));
    }
    if s.meta.art_style.painted() && !crate::art_style::painted_usable() {
        hint_small(
            ui,
            "The painted surfaces come with the Hand-painted Fantasy pack: until it's unlocked (Objects › Asset packs) the ground and materials stay realistic.",
        );
    }
}

/// Style › Fine-tune › Shuffle: new values for every fine-tune setting at each press, round the
/// style in use (`look_shuffle`; the style itself stays); Back returns to the values before; Save
/// keeps them in My styles.
fn fine_tune_shuffle(ui: &mut egui::Ui, s: &mut UiState) {
    use crate::look_styles::{self as styles, SavedStyle};
    let hist_id = egui::Id::new("style_shuffle_history");
    let info_id = egui::Id::new("style_shuffle_info");
    let time_id = egui::Id::new("style_shuffle_time");
    let preset = s.lighting.look.preset;
    let mut with_time: bool = ui.data(|d| d.get_temp(time_id)).unwrap_or(false);
    let mut hist: Vec<(crate::look::Look, f32)> =
        ui.data(|d| d.get_temp(hist_id)).unwrap_or_default();
    // Back stays within the style in use: picking another style starts afresh
    if hist.last().is_some_and(|(l, _)| l.preset != preset) {
        hist.clear();
    }
    // (mood, the shuffled values: the line shows while the look is still that one)
    let mut info: Option<(String, LookParams)> = ui.data(|d| d.get_temp(info_id));
    if info
        .as_ref()
        .is_some_and(|i| s.lighting.look.custom != Some(i.1))
    {
        info = None;
    }
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !hist.is_empty()
                && icon_button(ui, "ArrowRotateCounterClockwise", "Back to the values before").clicked()
                && let Some((look, hour)) = hist.pop()
            {
                s.lighting.look = look;
                s.lighting.hour = hour;
                info = None;
            }
            let tip = format!("New values for every setting below, round {} (the style stays). Press again for another.", preset.name());
            if primary_icon_button(ui, "Shuffle", "Shuffle settings").on_hover_text(tip).clicked() {
                hist.push((s.lighting.look.clone(), s.lighting.hour));
                if hist.len() > 30 {
                    hist.remove(0);
                }
                let sh = crate::look_shuffle::shuffle(fastrand::u64(..), preset, None);
                s.lighting.look.custom = Some(sh.params);
                s.lighting.look.saved = None;
                if with_time {
                    s.lighting.hour = sh.hour;
                }
                info = Some((sh.mood.to_string(), sh.params));
            }
        });
    });
    toggle(ui, &mut with_time, "Shuffle the time of day too");
    if let Some((mood, _)) = &info {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{mood} take on {}", preset.name()))
                    .size(12.5)
                    .color(TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_icon_button(ui, "FloppyDisk1", "Save this style")
                    .on_hover_text(
                        "Keep it in My styles (on the left) to use on any world or share",
                    )
                    .clicked()
                {
                    let name = (1..)
                        .map(|k| {
                            if k == 1 {
                                mood.clone()
                            } else {
                                format!("{mood} {k}")
                            }
                        })
                        .find(|n| styles::find(n).is_none())
                        .unwrap_or_else(styles::next_name);
                    let st = SavedStyle::from_look(&name, &s.lighting.look);
                    match styles::save(&st) {
                        Ok(_) => {
                            st.apply(&mut s.lighting.look);
                            s.ed.toast(format!("Saved the style \"{}\"", st.name));
                        }
                        Err(e) => s.ed.toast(e),
                    }
                }
            });
        });
    }
    ui.data_mut(|d| {
        d.insert_temp(time_id, with_time);
        d.insert_temp(hist_id, hist);
        if let Some(i) = info {
            d.insert_temp(info_id, i);
        } else {
            d.remove::<(String, LookParams)>(info_id);
        }
    });
}

/// Style › My styles: the user's own saved styles (`look_styles`): click one to use it, share it
/// as a .skystyle file or delete it; save the look as it is now under a name; import a shared one.
fn my_styles(ui: &mut egui::Ui, s: &mut UiState) {
    use crate::look_styles::{self as styles, SavedStyle};
    section(ui, "My styles");
    // answers of the file dialogs opened below
    let export_id = egui::Id::new("style_export_name");
    if let Some(path) = crate::dialogs::take_file("style_import") {
        match styles::import(&path) {
            Ok(st) => {
                st.apply(&mut s.lighting.look);
                s.ed.toast(format!("Imported the style \"{}\"", st.name));
            }
            Err(e) => s.ed.toast(e),
        }
    }
    if let Some(path) = crate::dialogs::take_file("style_export") {
        let name: String = ui.data(|d| d.get_temp(export_id)).unwrap_or_default();
        match styles::export(&name, &path) {
            Ok(p) => s.ed.toast(format!("Saved \"{name}\" to {}", p.display())),
            Err(e) => s.ed.toast(e),
        }
    }

    let list = styles::list();
    if list.is_empty() {
        dim(
            ui,
            "Fine-tune a style on the right (colours, light, haze, ambient occlusion, sun rays), then save it here to use it in any world or share it.",
        );
    }
    let confirm_id = egui::Id::new("style_delete_confirm");
    let confirm: Option<String> = ui.data(|d| d.get_temp(confirm_id));
    for st in &list {
        let active = s.lighting.look.saved.as_deref() == Some(st.name.as_str());
        let edited = active
            && !s
                .lighting
                .look
                .custom
                .is_some_and(|c| c.approx_eq(&st.params));
        ui.horizontal(|ui| {
            palette_strip(ui, st.based_on, vec2(28.0, 16.0));
            let label = if edited { format!("{}  (edited)", st.name) } else { st.name.clone() };
            if ui.selectable_label(active, label).on_hover_text(format!("Made from {}. Click to use it on this world.", st.based_on.name())).clicked() {
                st.apply(&mut s.lighting.look);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if confirm.as_deref() == Some(st.name.as_str()) {
                    if ui.small_button("Keep").clicked() {
                        ui.data_mut(|d| d.remove::<String>(confirm_id));
                    }
                    if ui.small_button(RichText::new("Delete").color(Color32::from_rgb(235, 110, 100))).clicked() {
                        match styles::delete(&st.name) {
                            Ok(()) => s.ed.toast(format!("Deleted the style \"{}\"", st.name)),
                            Err(e) => s.ed.toast(e),
                        }
                        ui.data_mut(|d| d.remove::<String>(confirm_id));
                    }
                } else {
                    if ui.small_button(icon::TRASH).on_hover_text("Delete this style from this computer (worlds using it keep their look)").clicked() {
                        ui.data_mut(|d| d.insert_temp(confirm_id, st.name.clone()));
                    }
                    if ui.small_button(icon::EXPORT).on_hover_text("Share: save it as a .skystyle file anyone can import").clicked() {
                        ui.data_mut(|d| d.insert_temp(export_id, st.name.clone()));
                        crate::dialogs::save_file("style_export", rfd::AsyncFileDialog::new().set_file_name(styles::file_name(&st.name)).add_filter("Skyfold style", &[styles::EXT]));
                    }
                }
            });
        });
    }

    // the name field follows the style in use: Save updates it, a new name saves a new style
    ui.add_space(4.0);
    let name_id = egui::Id::new("style_save_name");
    let seen_id = egui::Id::new("style_save_seen");
    let seen: Option<Option<String>> = ui.data(|d| d.get_temp(seen_id));
    let mut name: String = match ui.data(|d| d.get_temp::<String>(name_id)) {
        Some(n) if seen.as_ref() == Some(&s.lighting.look.saved) => n,
        _ => s
            .lighting
            .look
            .saved
            .clone()
            .unwrap_or_else(styles::next_name),
    };
    ui.horizontal(|ui| {
        let replacing = styles::find(&name).is_some();
        let label = if replacing { "Update" } else { "Save style" };
        let w = (ui.available_width() - 110.0).max(80.0);
        let field = ui.add(egui::TextEdit::singleline(&mut name).hint_text("Style name").desired_width(w));
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let tip = if replacing { "Replace this saved style with the look as it is now" } else { "Save the look as it is now (colours, light, haze, lens, ambient occlusion, sun rays) under this name" };
        if ui.button(format!("{} {label}", icon::FLOPPY_DISK)).on_hover_text(tip).clicked() || enter {
            let st = SavedStyle::from_look(&name, &s.lighting.look);
            match styles::save(&st) {
                Ok(_) => {
                    st.apply(&mut s.lighting.look);
                    s.ed.toast(format!("Saved the style \"{}\"", st.name));
                    name = st.name.clone();
                }
                Err(e) => s.ed.toast(e),
            }
        }
    });
    ui.data_mut(|d| {
        d.insert_temp(name_id, name);
        d.insert_temp(seen_id, s.lighting.look.saved.clone());
    });
    if ui
        .button(format!("{} Import style…", icon::DOWNLOAD_SIMPLE))
        .on_hover_text("Add a .skystyle file someone shared")
        .clicked()
    {
        crate::dialogs::pick_file(
            "style_import",
            rfd::AsyncFileDialog::new().add_filter("Skyfold style", &[styles::EXT]),
        );
    }
}

/// Tint multiplier (around 1.0) edited as a colour swatch: neutral shows as 80 % grey.
fn tint_row(ui: &mut egui::Ui, label: &str, tint: &mut [f32; 3]) -> bool {
    let mut c = [
        (tint[0] * 0.8).clamp(0.0, 1.0),
        (tint[1] * 0.8).clamp(0.0, 1.0),
        (tint[2] * 0.8).clamp(0.0, 1.0),
    ];
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("neutral").clicked() {
                c = [0.8; 3];
                changed = true;
            }
            changed |= ui.color_edit_button_rgb(&mut c).changed();
        });
    });
    if changed {
        *tint = [c[0] / 0.8, c[1] / 0.8, c[2] / 0.8];
    }
    changed
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [f32; 3]) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.color_edit_button_rgb(c);
        });
    });
}

fn style_right(ui: &mut egui::Ui, s: &mut UiState) {
    let look = &mut s.lighting.look;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} · {}",
                look.preset.name(),
                look.preset.inspired_by()
            ))
            .small()
            .color(TEXT_DIM),
        );
        if look.custom.is_some() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("Reset")
                    .on_hover_text("Back to the style as designed")
                    .clicked()
                {
                    look.custom = None;
                    look.saved = None;
                }
            });
        }
    });
    fine_tune_shuffle(ui, s);
    let look = &mut s.lighting.look;
    let before = look.base();
    let mut p: LookParams = before;
    let signed = |v: f32| format!("{v:+.2}");
    let x = |v: f32| format!("{v:.2}×");

    section_fold(ui, "ft_colour", "Colour", "", |ui| {
        slider_row(ui, "Exposure", &mut p.exposure, -1.5..=1.5, |v| {
            format!("{v:+.2} EV")
        });
        slider_row(
            ui,
            "Temperature (cool ↔ warm)",
            &mut p.temperature,
            -1.0..=1.0,
            signed,
        );
        slider_row(
            ui,
            "Tint (green ↔ magenta)",
            &mut p.tint,
            -1.0..=1.0,
            signed,
        );
        slider_row(ui, "Saturation", &mut p.saturation, 0.0..=2.0, x);
        slider_row(ui, "Vibrance", &mut p.vibrance, -0.5..=1.0, signed);
        slider_row(ui, "Contrast", &mut p.contrast, 0.6..=1.5, x);
        slider_row(ui, "Black level", &mut p.lift, -0.1..=0.15, signed);
        slider_row(
            ui,
            "Greens (yellow ↔ teal)",
            &mut p.foliage_hue,
            -1.0..=1.0,
            signed,
        );
        tint_row(ui, "Shadow colour", &mut p.shadow_tint);
        tint_row(ui, "Highlight colour", &mut p.highlight_tint);
        ui.label(RichText::new("Tone curve").color(TEXT));
        segmented(ui, &mut p.tone, &ToneCurve::ALL.map(|t| (t, t.name())));
    });

    section_fold(ui, "ft_light", "Light", "", |ui| {
        slider_row(ui, "Sun strength", &mut p.sun, 0.3..=2.0, x);
        slider_row(ui, "Sun warmth", &mut p.sun_warmth, -1.0..=1.0, signed);
        slider_row(ui, "Sky light (shadow fill)", &mut p.sky_fill, 0.3..=2.5, x);
        slider_row(ui, "Ambient occlusion", &mut p.occlusion, 0.0..=2.5, x).on_hover_text(
        "Soft shade in creases and on the ground round buildings, rocks and trees: how far it reaches. 0 turns it off. Its quality is in Lighting › Shadows.",
    );
        slider_row(ui, "Night brightness", &mut p.night_lift, 0.0..=2.5, |v| {
            format!("+{v:.1} EV")
        });
    });

    section_fold(ui, "ft_atmosphere", "Atmosphere", "", |ui| {
        slider_row(ui, "Sky tint", &mut p.sky_tint_amount, 0.0..=1.0, pct);
        color_row(ui, "Sky colour", &mut p.sky_tint);
    });

    section_fold(ui, "ft_lens", "Lens", "", |ui| {
        slider_row(ui, "Bloom / glow", &mut p.bloom, 0.0..=1.0, |v| {
            format!("{v:.2}")
        });
        slider_row(ui, "Vignette", &mut p.vignette, 0.0..=1.0, pct);
        slider_row(ui, "Film grain", &mut p.grain, 0.0..=0.1, |v| {
            format!("{:.0}%", v * 1000.0)
        });
        slider_row(ui, "Sharpen", &mut p.sharpen, 0.0..=1.0, pct);
        slider_row(
            ui,
            "Chromatic aberration",
            &mut p.aberration,
            0.0..=0.03,
            |v| format!("{:.1}", v * 1000.0),
        );
        slider_row(ui, "Sun glare", &mut p.glare, 0.0..=1.5, pct)
        .on_hover_text("A soft glare and a faint starburst round a bright sun in view; it lifts the shadows of a backlit frame a little, like a real eye or lens.");
        slider_row(ui, "Lens flare", &mut p.flare, 0.0..=1.5, pct).on_hover_text("Faint reflections of the sun inside the lens (hexagons and rings) along the line from the sun through the centre of the view.");
    });

    section_fold(ui, "ft_stylise", "Stylise", "", |ui| {
        slider_row(ui, "Painterly brushwork", &mut p.painterly, 0.0..=1.0, pct);
        slider_row(ui, "Ink outlines", &mut p.outline, 0.0..=1.0, pct);
        slider_row(
            ui,
            "Hand-painted textures",
            &mut p.texture_soften,
            0.0..=4.0,
            |v| format!("{v:.1}"),
        );
        slider_row(ui, "Surface detail", &mut p.detail, 0.0..=2.0, x);
    });

    if !p.approx_eq(&before) {
        look.custom = Some(p);
    }
}

// ------------------------------------------------------------------ Audio

fn audio_left(ui: &mut egui::Ui, s: &mut UiState, out: &mut UiOut) {
    section_fold(ui, "audio_sound", "Sound", "", |ui| {
        slider_row(ui, "Master", &mut s.audio.master, 0.0..=1.0, pct);
        slider_row(ui, "Ambience", &mut s.audio.ambience, 0.0..=1.0, pct);
        slider_row(ui, "Footsteps", &mut s.audio.footsteps, 0.0..=1.0, pct);
        toggle(
            ui,
            &mut s.audio.preview_in_editor,
            "Play ambience while editing",
        );
        if let Some(sc) = s.ambience.soundscape {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), egui::Sense::hover());
                paint_icon(ui, r.center(), "Headphones", 14.0, TEXT_DIM);
                ui.label(
                    RichText::new(format!(
                        "Now playing: {}{}",
                        sc.name(),
                        if s.ambience.auto { " (automatic)" } else { "" }
                    ))
                    .size(12.5)
                    .color(TEXT_DIM),
                );
            });
        }
        if s.ambience.preview.is_some()
            && icon_text_button(ui, "Stop", "Stop preview", None).clicked()
        {
            s.ambience.preview = None;
        }
    });
    section_fold(ui, "audio_steps", "Footsteps by ground", "", |ui| {
        hint_small(
            ui,
            "Every footstep is classified automatically from the ground under the player: painted materials, automatic snow/rock/sand, shallow water and walkable objects like bridges. The map shows what each spot sounds like.",
        );
        ui.spacing_mut().item_spacing.y = 2.0;
        for i in 0..LAYER_COUNT {
            let l = Layer::from_index(i);
            let sw = l.swatch();
            ui.horizontal(|ui| {
                ui.set_height(30.0);
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::hover());
                ui.painter().circle_filled(
                    r.center(),
                    11.0,
                    Color32::from_rgb(sw[0], sw[1], sw[2]),
                );
                let c = s.ed.layer_surface[i].overlay_color();
                ui.painter().circle_stroke(
                    r.center(),
                    12.5,
                    egui::Stroke::new(1.5, Color32::from_rgb(c[0], c[1], c[2])),
                );
                let name = mat_name(s, i);
                ui.add_sized(
                    vec2(ui.available_width() - 110.0 - 34.0 - 8.0, 22.0),
                    egui::Label::new(RichText::new(name).color(TEXT)).truncate(),
                );
                let mut cur = s.ed.layer_surface[i];
                ui.allocate_ui(vec2(110.0, 28.0), |ui| {
                    egui::ComboBox::from_id_salt(("ls", i))
                        .width(110.0)
                        .selected_text(cur.name())
                        .dropdown_ui(ui, |ui| {
                            for sf in Surface::ALL {
                                menu_option(ui, &mut cur, sf, sf.name());
                            }
                        });
                });
                s.ed.layer_surface[i] = cur;
                if icon_button(ui, "Play", "Hear it").clicked() {
                    out.sounds.write(SoundEvent::Footstep(cur));
                }
            });
        }
    });
}

fn audio_right(ui: &mut egui::Ui, s: &mut UiState, out: &mut UiOut) {
    ui.spacing_mut().item_spacing.y = 2.0;
    for sc in Soundscape::ALL.iter().skip(1) {
        let playing = s.ambience.preview.map(|p| p.0) == Some(*sc);
        if list_row(
            ui,
            Lead::Icon(if playing { "Stop" } else { "Play" }),
            sc.name(),
            "",
            if playing { "playing" } else { "" },
            playing,
        )
        .clicked()
        {
            s.ambience.preview = if playing { None } else { Some((*sc, 1.0)) };
        }
    }
    hint_small(
        ui,
        "Click one to hear it. Zones pick their own ambience in Zone rules.",
    );
    section_fold(ui, "audio_legend", "Surface legend", "", |ui| {
        for sf in Surface::ALL {
            let c = sf.overlay_color();
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    r,
                    CornerRadius::same(4),
                    Color32::from_rgb(c[0], c[1], c[2]),
                );
                ui.label(sf.name());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, "Play", "Hear it").clicked() {
                        out.sounds.write(SoundEvent::Footstep(sf));
                    }
                });
            });
        }
    });
}
