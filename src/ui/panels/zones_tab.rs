//! Gameplay › Zones in the Halo layout (HANDOFF › Gameplay › Zones): Add Zone, the zone list with
//! colour dots, the paint switch, display toggles and biomes in the panel; the zone's settings in
//! the inspector (only while one is selected); its brush in the dock.

use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, Sense, Ui, pos2, vec2};

use super::super::theme::*;
use super::super::widgets::*;
use super::super::{UiOut, UiState};
use super::{meters, zone_changed, zone_lighting};
use crate::biomes::BiomePreset;
use crate::editor::{Tab, ZoneTool};
use crate::haze::HazeMode;
use crate::world::{Biome, Soundscape, Weather, Zone, ZoneType};

pub(super) fn zones_left(ui: &mut Ui, s: &mut UiState) {
    zone_actions(ui, s);
    // the zones
    let mut select = None;
    let mut toggle_vis = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for (i, z) in s.zones.list.iter().enumerate() {
            let (row, eye) = zone_row(ui, i, z, s.zones.selected == Some(i));
            if eye {
                toggle_vis = Some(i);
            } else if row {
                select = Some(i);
            }
        }
    });
    if let Some(i) = toggle_vis {
        s.zones.list[i].visible = !s.zones.list[i].visible;
    }
    if let Some(i) = select {
        s.zones.selected = Some(i);
    }
    if s.zones.list.is_empty() {
        dim(
            ui,
            "No zones yet. Zones are your gameplay regions: names, level ranges, weather, music and rules. Add one, then paint its area.",
        );
    }
    section_fold(ui, "zones_paint", "Paint", "", |ui| {
        // (nothing lit while a biome brush is picked)
        let mut tool = (s.ed.zone_tool != ZoneTool::Biome).then_some(s.ed.zone_tool);
        if segmented_icons(
            ui,
            &mut tool,
            &[
                (Some(ZoneTool::Paint), "PaintBrush", "Paint Area"),
                (Some(ZoneTool::Erase), "Eraser", "Erase"),
            ],
            true,
        ) && let Some(t) = tool
        {
            s.ed.zone_tool = t;
        }
        if matches!(s.ed.zone_tool, ZoneTool::Paint | ZoneTool::Erase) {
            slider_row_log(
                ui,
                "Brush size",
                &mut s.ed.zone_brush.size,
                10.0..=1500.0,
                meters,
            );
        }
    });
    section_fold(ui, "zones_display", "Zone display", "", |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        toggle(ui, &mut s.ed.show_zone_boundaries, "Show zone boundaries");
        toggle(ui, &mut s.ed.show_zone_labels, "Show zone labels");
        toggle(ui, &mut s.ed.show_level_ranges, "Show level ranges");
        toggle(ui, &mut s.ed.show_zone_fill, "Show zone colors");
    });
    section_fold(ui, "zones_biomes", "Biomes", "", |ui| biomes(ui, s));
}

fn add_zone(s: &mut UiState) {
    if let Some(id) = s.zones.next_id() {
        s.history.begin("Add zone");
        s.history.touch_zones(&s.zones);
        let names = [
            "Starfall Basin",
            "Sunken Steps",
            "Bridgehaven",
            "Bloomwood",
            "Broken Reach",
            "Mirrorwater",
            "Upturned Wilds",
            "Fallen Spire",
            "Ashen Vale",
            "Frostmere",
            "Gloomfen",
            "Highwatch",
        ];
        let name = names
            .get(s.zones.list.len())
            .map(|n| n.to_string())
            .unwrap_or(format!("Zone {id}"));
        let mut z = Zone::new(id, name);
        let n = s.zones.list.len() as u32;
        z.level_min = 1 + n * 5;
        z.level_max = 5 + n * 5;
        s.zones.list.push(z);
        s.zones.selected = Some(s.zones.list.len() - 1);
        s.ed.zone_tool = ZoneTool::Paint;
        s.history.commit(&s.store);
    } else {
        s.ed.toast("Zone limit reached (63)");
    }
}

/// The AI fill only adds zones to land that has not already been zoned. It owns its own queued
/// lifecycle, so this panel only requests Start, Cancel or Undo and reflects that state.
fn zone_actions(ui: &mut Ui, s: &mut UiState) {
    let connection_id = ui.make_persistent_id("auto_zones_connection");
    let mut connection_open = ui.data_mut(|d| *d.get_persisted_mut_or(connection_id, false));
    let prerequisite = crate::zones::auto::prerequisite_error(&s.prompt.session, &s.gameplay);
    let needs_cap = s
        .gameplay
        .level_cap
        .is_none_or(|cap| !(1..=200).contains(&cap));
    let busy = s.auto_zones.busy();
    let can_undo = s.auto_zones.can_undo(&s.history, s.terrain.generation);
    let mut request = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // Reserve the icon-only undo button, then give Add zone most of the row.
        let action_width = (ui.available_width() - 40.0 - 12.0).max(0.0);
        let auto_width = (action_width * 0.25).max(64.0);
        let add_width = (action_width - auto_width).max(0.0);
        if zone_action_button(ui, Some("PlusMedium"), "Add zone", add_width, true).clicked() {
            add_zone(s);
        }
        if busy {
            auto_zone_loading_button(ui, auto_width);
        } else if ui
            .add_enabled_ui(prerequisite.is_none(), |ui| {
                zone_action_button(ui, Some("MagicWand"), "Auto", auto_width, true)
            })
            .inner
            .on_hover_text("Fill only land that has no zone; existing zones stay as they are")
            .clicked()
        {
            request = Some(crate::zones::auto::AutoZoneRequest::Start);
        }
        if ui
            .add_enabled_ui(!busy && can_undo, |ui| auto_zone_undo_button(ui))
            .inner
            .clicked()
        {
            request = Some(crate::zones::auto::AutoZoneRequest::Undo);
        }
    });
    if let Some(request) = request {
        s.auto_zones.requests.push(request);
    }

    if busy {
        ui.add(
            egui::Label::new(
                RichText::new("Zone generation is running. Progress and Cancel stay visible over the viewport.")
                    .size(12.0)
                    .color(TEXT_DIM),
            )
            .wrap(),
        );
    } else if let Some(error) = &s.auto_zones.error {
        ui.add(egui::Label::new(RichText::new(error).size(12.0).color(WARN)).wrap());
    } else if let Some(reason) = &prerequisite {
        hint_small(ui, &reason);
    } else {
        hint_small(
            ui,
            "Fills only unzoned land. Existing zones stay as they are; generated settings can be edited afterward.",
        );
    }
    if !busy && prerequisite.is_some() {
        if crate::prompt::ai_ready(&s.prompt.session).is_none() {
            if small_icon_button(ui, "SparklesTwo", "Connect AI")
                .on_hover_text("Choose and connect an AI agent")
                .clicked()
            {
                connection_open = true;
            }
        } else if needs_cap
            && small_icon_button(ui, "Gauge", "Set level cap")
                .on_hover_text("Open Gameplay › World progression")
                .clicked()
        {
            s.ed.tab = Tab::Gameplay;
        } else if !needs_cap && s.gameplay.player_start.is_none() {
            if small_icon_button(ui, "MapPin", "Set starting point")
                .on_hover_text("Open Gameplay › Player start")
                .clicked()
            {
                s.ed.tab = Tab::Gameplay;
                s.gp_ed.tool = crate::gameplay::GameplayTool::PlayerStart;
            }
        }
    }
    if crate::prompt::ai_ready(&s.prompt.session).is_some()
        && small_icon_button(ui, "SparklesTwo", "AI settings")
            .on_hover_text("Change the connected AI agent, model or API key")
            .clicked()
    {
        connection_open = true;
    }
    if connection_open {
        egui::Window::new("AI connection")
            .id(ui.id().with("auto_zones_connection_window"))
            .open(&mut connection_open)
            .default_pos(ui.ctx().content_rect().center() - vec2(190.0, 110.0))
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.set_width(360.0);
                ui.push_id("auto_zones_connection_controls", |ui| {
                    crate::prompt::connection_controls(ui, &mut s.prompt.session);
                });
            });
    }
    ui.data_mut(|d| d.insert_persisted(connection_id, connection_open));
}

/// The action keeps its familiar Auto label while a request is underway. Cancellation is in the
/// persistent viewport card, so the row does not change shape when the panel is collapsed.
fn auto_zone_loading_button(ui: &mut Ui, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_INPUT), SOFT);
        let label =
            ui.painter()
                .layout_no_wrap("Auto".to_owned(), FontId::new(13.0, medium()), TEXT_DIM);
        let content_width = 18.0 + 6.0 + label.size().x;
        let left = rect.center().x - content_width * 0.5;
        egui::Spinner::new().size(18.0).color(TEXT_DIM).paint_at(
            ui,
            egui::Rect::from_min_size(pos2(left, rect.center().y - 9.0), vec2(18.0, 18.0)),
        );
        ui.painter().galley(
            pos2(left + 24.0, rect.center().y - label.size().y * 0.5),
            label,
            TEXT_DIM,
        );
    }
    response.on_hover_text("Generating zones")
}

fn zone_action_button(
    ui: &mut Ui,
    icon: Option<&str>,
    label: &str,
    width: f32,
    primary: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if ui.is_rect_visible(rect) {
        let enabled = ui.is_enabled();
        let hot = response.hovered() && enabled;
        let fill = if primary && enabled {
            if hot { ACCENT_HOVER } else { ACCENT }
        } else if hot {
            SOFT_HOVER
        } else {
            SOFT
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_INPUT), fill);
        let foreground = if primary && enabled {
            ON_ACCENT
        } else {
            TEXT_DIM
        };
        let text =
            ui.painter()
                .layout_no_wrap(label.to_string(), FontId::new(13.0, medium()), foreground);
        let icon_width = if icon.is_some() { 22.0 } else { 0.0 };
        let left = rect.center().x - (icon_width + text.size().x) * 0.5;
        if let Some(icon) = icon {
            paint_icon(
                ui,
                pos2(left + 8.0, rect.center().y),
                icon,
                16.0,
                foreground,
            );
        }
        ui.painter().galley(
            pos2(left + icon_width, rect.center().y - text.size().y * 0.5),
            text,
            foreground,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn auto_zone_undo_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Undo auto zones")
    });
    if ui.is_rect_visible(rect) {
        let hot = response.hovered() && ui.is_enabled();
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(R_INPUT),
            if hot { SOFT_HOVER } else { SOFT },
        );
        paint_icon(
            ui,
            rect.center(),
            "ArrowUndoUp",
            17.0,
            if ui.is_enabled() { TEXT_DIM } else { TEXT_MUTE },
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Undo auto zones")
}

/// A zone in the list: its colour, number and name, level range, and the eye (shown / hidden).
/// Returns (row clicked, eye clicked).
fn zone_row(ui: &mut Ui, i: usize, z: &Zone, sel: bool) -> (bool, bool) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
    let eye =
        egui::Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(24.0, 24.0));
    let eye_resp = ui.interact(eye, ui.id().with(("zone_eye", i)), Sense::click());
    let p = ui.painter();
    if sel {
        p.rect_filled(rect, CornerRadius::same(R_INPUT), SOFT_HOVER);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(R_INPUT), SOFT);
    }
    p.rect_filled(
        egui::Rect::from_center_size(pos2(rect.left() + 12.0, rect.center().y), vec2(10.0, 10.0)),
        CornerRadius::same(3),
        color32(z.color),
    );
    let col = if z.visible { TEXT } else { TEXT_DIM };
    let lv = p.layout_no_wrap(z.level_label(), FontId::new(11.0, mono()), TEXT_MUTE);
    let lw = lv.size().x;
    p.galley(
        pos2(eye.left() - 6.0 - lw, rect.center().y - lv.size().y * 0.5),
        lv,
        TEXT_MUTE,
    );
    let g = p.layout_no_wrap(
        format!("{}. {}", i + 1, z.name),
        FontId::new(13.0, medium()),
        col,
    );
    p.galley(
        pos2(rect.left() + 26.0, rect.center().y - g.size().y * 0.5),
        g,
        col,
    );
    if eye_resp.hovered() {
        p.rect_filled(eye, CornerRadius::same(7), SOFT);
    }
    paint_icon(
        ui,
        eye.center(),
        if z.visible { "EyeOpen" } else { "EyeSlash" },
        14.0,
        if eye_resp.hovered() { TEXT } else { TEXT_MUTE },
    );
    let eye_resp = eye_resp.on_hover_text(if z.visible {
        "Hide this zone on the map"
    } else {
        "Show this zone on the map"
    });
    (
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked(),
        eye_resp.clicked(),
    )
}

fn biomes(ui: &mut Ui, s: &mut UiState) {
    hint_small(
        ui,
        "Paint the land as a biome: its ground, grass, plants, ambience and weather follow (shown in colour while painting). Biomes never create zones.",
    );
    let cols = 2;
    let w = ((ui.available_width() - 5.0) / cols as f32).floor();
    for row in BiomePreset::ALL.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            for p in row {
                let sel = s.ed.zone_tool == ZoneTool::Biome && s.ed.biome_preset == *p;
                if biome_cell(ui, color32(p.def().color), p.short(), sel, w)
                    .on_hover_text(format!("{}\n{}", p.def().name, p.def().blurb))
                    .clicked()
                {
                    s.ed.zone_tool = ZoneTool::Biome;
                    s.ed.biome_preset = *p;
                }
            }
        });
    }
    if s.ed.zone_tool == ZoneTool::Biome {
        let d = s.ed.biome_preset.def();
        ui.label(
            RichText::new(d.name)
                .font(FontId::new(13.5, semibold()))
                .color(TEXT),
        );
        hint_small(ui, d.blurb);
        segmented(
            ui,
            &mut s.ed.biome_outline,
            &[(false, "Paint"), (true, "Outline")],
        );
        if s.ed.biome_outline {
            hint_small(ui, "Hold and draw around the area; release to fill it.");
        } else {
            slider_row_log(
                ui,
                "Brush size",
                &mut s.ed.zone_brush.size,
                10.0..=1500.0,
                meters,
            );
            hint_small(ui, "Paint over the area; plants grow when you release.");
        }
    }
}

/// A biome brush: colour square + name, white when picked.
fn biome_cell(ui: &mut Ui, colour: Color32, label: &str, selected: bool, w: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            Color32::WHITE
        } else if resp.hovered() {
            SOFT_HOVER
        } else {
            SOFT
        };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(R_TILE), fill);
        p.rect_filled(
            egui::Rect::from_center_size(
                pos2(rect.left() + 15.0, rect.center().y),
                vec2(12.0, 12.0),
            ),
            CornerRadius::same(4),
            colour,
        );
        let fg = if selected { ON_ACCENT } else { TEXT };
        let g = p.layout_no_wrap(label.to_owned(), FontId::proportional(13.0), fg);
        p.galley(
            pos2(rect.left() + 29.0, rect.center().y - g.size().y * 0.5),
            g,
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ the inspector

pub(super) fn zones_inspector(s: &UiState) -> Option<(String, bool)> {
    s.zones
        .selected
        .filter(|i| *i < s.zones.list.len())
        .map(|_| ("Zone settings".to_string(), true))
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(TEXT_DIM));
}

pub(super) fn zones_right(ui: &mut Ui, s: &mut UiState, out: &mut UiOut) {
    let Some(i) = s.zones.selected.filter(|i| *i < s.zones.list.len()) else {
        return;
    };
    let before = s.zones.list.clone();
    let mut delete = false;
    let mut preview = None;
    let mut focus = None;
    {
        let z = &mut s.zones.list[i];
        field_label(ui, "Name");
        ui.add(egui::TextEdit::singleline(&mut z.name).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            field_label(ui, "Color");
            ui.color_edit_button_rgb(&mut z.color);
        });
        dropdown_row(ui, "Type", "zone_type", z.zone_type.name(), |ui| {
            for t in ZoneType::ALL {
                menu_option(ui, &mut z.zone_type, t, t.name());
            }
        });
        let mut lo = z.level_min as f32;
        if slider_row(ui, "Level from", &mut lo, 1.0..=200.0, |v| {
            format!("Lv {v:.0}")
        })
        .changed()
        {
            z.level_min = lo.round() as u32;
        }
        let mut hi = z.level_max as f32;
        if slider_row(ui, "Level to", &mut hi, 1.0..=200.0, |v| {
            format!("Lv {v:.0}")
        })
        .changed()
        {
            z.level_max = hi.round() as u32;
        }
        if z.level_max < z.level_min {
            z.level_max = z.level_min;
        }
        field_label(ui, "Description");
        ui.add(
            egui::TextEdit::multiline(&mut z.description)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .hint_text("What players find here…"),
        );
        if let Some(a) = s.stats.area_km2.get(&z.id) {
            hint_small(ui, &format!("Area: {a:.2} km²"));
        } else {
            warn_line(ui, "Not painted yet — paint its area on the map.");
        }
        section_fold(ui, "zone_haze", "Haze", "", |ui| {
            dropdown_row(ui, "Setting", "zone_haze_mode", z.haze.mode.name(), |ui| {
                for mode in HazeMode::ALL {
                    menu_option(ui, &mut z.haze.mode, mode, mode.name());
                }
            });
            if z.haze.mode == HazeMode::On {
                slider_row(ui, "Haze amount", &mut z.haze.amount, 0.0..=1.0, |v| {
                    format!("{:.0}%", v * 100.0)
                });
            }
            hint_small(
                ui,
                "Follows the viewer into this painted zone and uses the current style's haze settings.",
            );
        });
        section_fold(ui, "zone_rules", "Zone rules", "", |ui| {
            toggle(ui, &mut z.weather_enabled, "Set weather");
            if z.weather_enabled {
                dropdown_row(ui, "Weather", "zone_weather", z.weather.name(), |ui| {
                    for w in Weather::ALL {
                        menu_option(ui, &mut z.weather, w, w.name());
                    }
                });
                slider_row(ui, "Intensity", &mut z.weather_intensity, 0.1..=1.0, |v| {
                    format!("{:.0}%", v * 100.0)
                });
            }
            toggle(ui, &mut z.audio_enabled, "Set ambient audio");
            if z.audio_enabled {
                ui.horizontal(|ui| {
                    let w = ui.available_width() - 34.0;
                    ui.allocate_ui(vec2(w, 28.0), |ui| {
                        egui::ComboBox::from_id_salt("zone_audio")
                            .width(ui.available_width())
                            .selected_text(z.soundscape.name())
                            .dropdown_ui(ui, |ui| {
                                for a in Soundscape::ALL {
                                    menu_option(ui, &mut z.soundscape, a, a.name());
                                }
                            });
                    });
                    if icon_button(ui, "Play", "Preview this ambience").clicked() {
                        preview = Some((z.soundscape, z.audio_volume));
                    }
                });
                slider_row(ui, "Volume", &mut z.audio_volume, 0.0..=1.0, |v| {
                    format!("{:.0}%", v * 100.0)
                });
            }
            // one click: the whole zone takes a biome's ground, plants, weather, ambience and light
            let cur = z.biome_preset.map(|p| p.def().name).unwrap_or("Choose…");
            dropdown_row(ui, "Biome", "zone_biome_preset", cur, |ui| {
                for p in BiomePreset::ALL {
                    if menu_check(ui, z.biome_preset == Some(p), p.def().name)
                        .on_hover_text(p.def().blurb)
                        .clicked()
                    {
                        out.biome.write(crate::biomes::ApplyBiomeToZone {
                            zone: z.id,
                            preset: p,
                        });
                    }
                }
            });
            hint_small(
                ui,
                "Applies the biome to the whole zone: ground, plants, weather, sound, mood and water (undoable).",
            );
            toggle(ui, &mut z.biome_enabled, "Set biome filter");
            if z.biome_enabled {
                dropdown_row(ui, "Filter", "zone_biome", z.biome.name(), |ui| {
                    for b in Biome::ALL {
                        menu_option(ui, &mut z.biome, b, b.name());
                    }
                });
                hint_small(
                    ui,
                    "Nature brushes only place species that belong to this biome here.",
                );
            }
            section_fold(
                ui,
                "zone_forest_diversity",
                "Forest diversity",
                "independent of lighting",
                |ui| {
                    let mut mode = z.forest;
                    ui.horizontal_wrapped(|ui| {
                        for (value, label) in [
                            (None, "Inherit world"),
                            (Some(false), "Off"),
                            (Some(true), "On"),
                        ] {
                            let selected = mode.map(|f| f.enabled) == value;
                            if ui.selectable_label(selected, label).clicked() {
                                mode = value.map(|enabled| crate::nature::ForestDiversity {
                                    enabled,
                                    layout: z.forest.map(|f| f.layout).unwrap_or_default(),
                                });
                            }
                        }
                    });
                    z.forest = mode;
                    if let Some(forest) = &mut z.forest
                        && forest.enabled
                    {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("Layout").color(TEXT_DIM));
                            for layout in crate::nature::ForestLayout::ALL {
                                if ui
                                    .selectable_label(forest.layout == layout, layout.name())
                                    .clicked()
                                {
                                    forest.layout = layout;
                                }
                            }
                        });
                    }
                },
            );
            toggle(ui, &mut z.wind_enabled, "Set wind");
            if z.wind_enabled {
                slider_row(ui, "Wind", &mut z.wind_speed, 0.0..=25.0, |v| {
                    format!("{v:.1} m/s · {}", crate::wind::describe(v))
                });
            }
            toggle(ui, &mut z.clouds_enabled, "Set cloud cover");
            if z.clouds_enabled {
                slider_row(ui, "Cloud cover", &mut z.cloud_cover, 0.0..=1.0, |v| {
                    format!("{:.0}%", v * 100.0)
                });
            }
            slider_row(ui, "Temperature", &mut z.temperature, -40.0..=55.0, |v| {
                format!("{v:.1} °C")
            });
            toggle(ui, &mut z.limit_placement, "Limit object placement");
        });
        section_fold(ui, "zone_gameplay", "Gameplay", "", |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            toggle(ui, &mut z.allow_mounts, "Mounts allowed");
            toggle(ui, &mut z.allow_flying, "Flying allowed");
            toggle(ui, &mut z.rested_area, "Rested area (inns, safe camps)");
        });
        let viewer = s.sky.viewer_zone;
        let go = section_fold(ui, "zone_light", "Lighting & style", "", |ui| {
            zone_lighting(ui, z, viewer)
        })
        .unwrap_or(false);
        if go {
            focus = Some(z.id);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = ((ui.available_width() - 6.0) * 0.5).floor();
            if icon_text_button(ui, "Target", "Focus", Some(w))
                .on_hover_text("Fly the camera to it")
                .clicked()
            {
                focus = Some(z.id);
            }
            let r = icon_text_button(ui, "TrashCan", "Delete Zone", Some(w));
            if r.hovered() {
                ui.painter().rect_stroke(
                    r.rect,
                    CornerRadius::same(R_BUTTON),
                    egui::Stroke::new(1.0, DANGER.gamma_multiply(0.6)),
                    egui::StrokeKind::Inside,
                );
            }
            if r.clicked() {
                delete = true;
            }
        });
    }
    if let Some(p) = preview {
        s.ambience.preview = if s.ambience.preview.is_some() {
            None
        } else {
            Some(p)
        };
    }
    if let Some(id) = focus
        && let Some(a) = s.stats.anchor.get(&id)
    {
        let d = s.orbit.distance.max(600.0);
        let (yaw, pitch) = (s.orbit.yaw, s.orbit.pitch);
        s.orbit.set_view(*a, yaw, pitch, d.min(2500.0));
    }
    // record definition edits in history (coalesced per frame)
    if delete {
        s.history.begin("Delete zone");
        s.history.touch_zones(&s.zones);
        let id = s.zones.list[i].id;
        // clear its painted area
        let n = s.terrain.n();
        let all = crate::terrain::GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        };
        s.history.touch_terrain(&s.terrain, all);
        for v in s.terrain.zones.iter_mut() {
            if *v == id {
                *v = 0;
            }
        }
        s.terrain.mark_zone_dirty(all);
        s.zones.list.remove(i);
        s.zones.selected = None;
        s.history.commit(&s.store);
    } else if before.len() == s.zones.list.len() && zone_changed(&before[i], &s.zones.list[i]) {
        let old_zones = crate::world::Zones {
            list: before,
            selected: s.zones.selected,
        };
        s.history.begin("Edit zone");
        s.history.touch_zones(&old_zones);
        s.history.commit(&s.store);
        s.meta.dirty = true;
    }
}

// ------------------------------------------------------------------ the dock

pub(super) fn zones_dock(ui: &mut Ui, s: &mut UiState) -> bool {
    match s.ed.zone_tool {
        ZoneTool::Biome => {
            let d = s.ed.biome_preset.def();
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 28.0), Sense::hover());
            ui.painter().rect_filled(
                egui::Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
                CornerRadius::same(4),
                color32(d.color),
            );
            ui.label(
                RichText::new(format!("Biome · {}", d.name))
                    .font(FontId::new(13.0, semibold()))
                    .color(TEXT),
            );
            dock_sep(ui);
            if s.ed.biome_outline {
                dock_text(ui, "Hold and draw around the area; release to fill it.");
                dock_sep(ui);
                dock_hints(ui, &[("Drag", "Outline")]);
            } else {
                slider_mini_log(
                    ui,
                    "Brush size",
                    &mut s.ed.zone_brush.size,
                    10.0..=1500.0,
                    meters,
                );
                dock_sep(ui);
                dock_hints(ui, &[("Click", "Paint"), ("⌥", "Erase"), ("[ ]", "Size")]);
            }
        }
        tool => {
            let zone = s.zones.selected.and_then(|i| s.zones.list.get(i));
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 28.0), Sense::hover());
            if let Some(z) = zone {
                ui.painter().rect_filled(
                    egui::Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
                    CornerRadius::same(4),
                    color32(z.color),
                );
            }
            let what = if tool == ZoneTool::Erase {
                "Erase"
            } else {
                "Paint"
            };
            let name = zone
                .map(|z| format!("{what} · {}", z.name))
                .unwrap_or_else(|| "Select a zone".into());
            ui.label(
                RichText::new(name)
                    .font(FontId::new(13.0, semibold()))
                    .color(TEXT),
            );
            dock_sep(ui);
            slider_mini_log(
                ui,
                "Brush size",
                &mut s.ed.zone_brush.size,
                10.0..=1500.0,
                meters,
            );
            dock_sep(ui);
            dock_hints(
                ui,
                &[
                    (
                        "Click",
                        if tool == ZoneTool::Erase {
                            "Erase"
                        } else {
                            "Paint"
                        },
                    ),
                    (
                        "⌥",
                        if tool == ZoneTool::Erase {
                            "Paint"
                        } else {
                            "Erase"
                        },
                    ),
                    ("[ ]", "Size"),
                ],
            );
        }
    }
    true
}
