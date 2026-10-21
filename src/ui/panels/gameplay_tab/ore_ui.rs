//! The Ore tool: the 25 ores in a grid by tier (rendered previews of each deposit, rarity
//! colours, hover hints with tier, rarity and typical use), how to place the picked ore (one
//! deposit per click, or an outlined area that fills itself; the switch is also in the dock) and a
//! one-click scatter over the whole world by zone level and biome; plus the inspectors of a
//! deposit area (ore table, density, respawn, arrangement) and of a single deposit.

use super::*;
use crate::gameplay::markers::{ORE_ICON, ore_color, rarity_color};
use crate::gameplay::ores::{self, Ore, OreEntry, Rarity, Site};
use crate::gameplay::{OreMode, ResourceNode};
use crate::thumbs::Subject;

/// An ore's card: its deposit's picture (an ore-coloured disc until it's rendered) in an image
/// well, its name below (shrunk to fit); a white ring when picked.
fn ore_tile(
    ui: &mut egui::Ui,
    previews: &mut crate::thumbs::Previews,
    ore: Ore,
    selected: bool,
    w: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, w + 15.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let img = egui::Rect::from_min_size(rect.min, vec2(w, w));
    card_well(ui, img, resp.hovered());
    let s = Subject::Ore(ore);
    match previews.thumb(&s) {
        Some(t) => {
            egui::Image::new((t, img.size()))
                .corner_radius(CornerRadius::same(8))
                .paint_at(ui, img.shrink(1.0));
        }
        None => {
            ui.painter()
                .circle_filled(img.center(), img.width() * 0.22, ore_color(ore));
            if previews.pending(&s) {
                egui::Spinner::new().size(12.0).color(TEXT_DIM).paint_at(
                    ui,
                    egui::Rect::from_center_size(img.center(), vec2(12.0, 12.0)),
                );
            }
        }
    }
    let p = ui.painter();
    if selected {
        p.rect_stroke(
            img,
            CornerRadius::same(8),
            Stroke::new(2.0, Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    } else if resp.hovered() {
        p.rect_stroke(
            img,
            CornerRadius::same(8),
            Stroke::new(1.0, Color32::from_white_alpha(90)),
            egui::StrokeKind::Outside,
        );
    }
    let col = if selected { TEXT } else { TEXT_DIM };
    let avail = w;
    let mut size = 11.0;
    let mut g = p.layout_no_wrap(ore.name().to_string(), FontId::proportional(size), col);
    if g.size().x > avail {
        size = (size * avail / g.size().x).max(7.5);
        g = p.layout_no_wrap(ore.name().to_string(), FontId::proportional(size), col);
    }
    p.galley(
        pos2(
            rect.center().x - g.size().x * 0.5,
            rect.bottom() - 13.0 + (11.0 - g.size().y) * 0.5,
        ),
        g,
        col,
    );
    resp.on_hover_ui(|ui| ore_hint(ui, ore))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Name, tier and rarity, use and habitat.
fn ore_hint(ui: &mut egui::Ui, ore: Ore) {
    ui.label(
        RichText::new(ore.name())
            .font(FontId::new(14.0, semibold()))
            .color(Color32::WHITE),
    );
    ui.label(
        RichText::new(format!("Tier {} · {}", ore.tier(), ore.rarity().name()))
            .color(rarity_color(ore)),
    );
    ui.label(RichText::new(ore.uses()).color(TEXT));
    ui.label(
        RichText::new(format!("Found in {}", ore.habitat().about))
            .small()
            .color(TEXT_DIM),
    );
}

/// The ore grid: a row of five per tier under a heading with its rarity. Returns the clicked ore.
fn ore_grid(ui: &mut egui::Ui, previews: &mut crate::thumbs::Previews, cur: Ore) -> Option<Ore> {
    let w = ((ui.available_width() - 5.0 * 4.0) / 5.0).floor();
    let mut out = None;
    for tier in 1..=5u8 {
        let r = Rarity::of_tier(tier);
        let c = r.color();
        let c = Color32::from_rgb(c[0], c[1], c[2]);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(format!("Tier {tier}"))
                    .size(11.5)
                    .color(TEXT_DIM),
            );
            let (dot, _) = ui.allocate_exact_size(vec2(6.0, 12.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.0, c);
            ui.label(RichText::new(r.name()).size(11.5).color(c));
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            for o in Ore::of_tier(tier) {
                if ore_tile(ui, previews, o, o == cur, w).clicked() {
                    out = Some(o);
                }
            }
        });
    }
    out
}

/// Finishes the deposit area being drawn (one undo step).
pub(super) fn finish_area(s: &mut UiState) {
    let pts: Vec<Vec2> = s.gp_ed.draft.iter().map(|p| p.xz()).collect();
    let o = s.gp_ed.ore;
    let g = Ground {
        terrain: &s.terrain,
        zones: &s.zones,
        water: &s.water,
        nature: None,
    };
    let site = Site {
        g: &g,
        store: Some(&*s.store),
        lib: s.lib.as_deref(),
        roads: Some(&*s.roads),
    };
    s.history.begin("Draw deposit area");
    if crate::gameplay::finish_area(&mut s.gameplay, &site, &mut s.history, &mut s.ed, o, &pts) {
        s.gp_ed.draft.clear();
        s.meta.dirty = true;
    }
    s.history.commit(&s.store);
}

pub(super) fn ore_tool(ui: &mut egui::Ui, s: &mut UiState) {
    crate::ui::tips::card(
        ui,
        "ores",
        "How ore deposits work",
        &[
            "Pick an ore below. Hover one to see its tier, rarity and what it's used for.",
            "Click the ground to place one deposit, or choose Draw an area and outline a field on the ground: it fills itself with deposits on suitable ground.",
            "Click an area to change its ores, density and respawn time. Fill the world scatters ore everywhere by zone level and biome.",
        ],
    );
    section_fold(ui, "gp_ore", "Ore", &Ore::ALL.len().to_string(), |ui| {
        // how clicks place: right at the top, so drawing areas is never below the fold
        segmented_solid(
            ui,
            &mut s.gp_ed.ore_mode,
            &[
                (OreMode::Single, "One deposit"),
                (OreMode::Area, "Draw an area"),
            ],
        );
        let o = s.gp_ed.ore;
        ui.horizontal(|ui| {
            let text = match s.gp_ed.ore_mode {
                OreMode::Single => format!("Click the ground to place a {} deposit; drag it to move it.", o.name()),
                OreMode::Area if s.gp_ed.draft.is_empty() => {
                    let mix = ores::default_table(o).iter().map(|e| e.ore.name()).collect::<Vec<_>>().join(" and ");
                    format!("Click corners around the area (or hold and draw a loop); a double-click, Enter or the first corner finishes. It fills with {mix} deposits.")
                }
                OreMode::Area => format!("Drawing an area: {} corners. Finish in the dock or with Enter; Esc cancels.", s.gp_ed.draft.len()),
            };
            ui.allocate_ui(vec2(ui.available_width() - 26.0, 0.0), |ui| hint_small(ui, &text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| crate::ui::tips::help_button(ui, "ores"));
        });
        // the picked ore, named in its rarity colour
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(o.name())
                    .font(FontId::new(13.5, semibold()))
                    .color(rarity_color(o)),
            );
            ui.label(
                RichText::new(format!("Tier {} · {}", o.tier(), o.rarity().name()))
                    .size(11.5)
                    .color(TEXT_MUTE),
            );
        });
        hint_small(
            ui,
            &format!("{} · found in {}.", o.uses(), o.habitat().about),
        );
        if let Some(o) = ore_grid(ui, &mut s.previews, s.gp_ed.ore) {
            s.gp_ed.ore = o;
        }
    });

    let mut scatter = false;
    section_fold(ui, "gp_ore_fill", "Fill the world", "optional", |ui| {
        hint_small(
            ui,
            "Scatter ore deposits over the whole world where each ore forms (iron and coal in the hills, mithril on peaks, obsidian by volcanoes). Zones use their level range; the wilderness uses the rarity mix below.",
        );
        let mut st = s.gameplay.ore_settings.clone();
        toggle(ui, &mut st.on_generate, "Also when generating a world")
            .on_hover_text("New and regenerated worlds get ore with these settings");
        if ore_settings_ui(ui, &mut st, "ore_tool") || st != s.gameplay.ore_settings {
            s.gameplay.ore_settings = st;
            s.meta.dirty = true;
        }
        let auto = s
            .gameplay
            .nodes
            .iter()
            .filter(|n| n.auto && n.ore().is_some())
            .count();
        ui.horizontal(|ui| {
            if icon_text_button(ui, "Shuffle", "Scatter ore", None)
                .on_hover_text("Replaces earlier scattered ore; deposits you placed and areas stay")
                .clicked()
            {
                scatter = true;
            }
            if auto > 0 {
                ui.label(
                    RichText::new(format!("{auto} scattered"))
                        .size(11.5)
                        .color(TEXT_MUTE),
                );
            }
        });
    });
    if scatter {
        scatter_ore(s);
    }
    if !s.gameplay.ore_areas.is_empty() {
        let mut pick = None;
        section_fold(
            ui,
            "gp_ore_areas",
            "Deposit areas",
            &s.gameplay.ore_areas.len().to_string(),
            |ui| {
                for a in &s.gameplay.ore_areas {
                    let n = s
                        .gameplay
                        .nodes
                        .iter()
                        .filter(|n| n.area == Some(a.id))
                        .count();
                    if item_row(
                        ui,
                        AREA_ICON,
                        area_color(a),
                        &a.name,
                        &format!("{n}"),
                        s.gameplay.selected == Some(GameplaySel::OreArea(a.id)),
                    )
                    .on_hover_text(a.mix_label())
                    .clicked()
                    {
                        pick = Some(GameplaySel::OreArea(a.id));
                    }
                }
            },
        );
        if let Some(sel) = pick {
            s.gameplay.selected = Some(sel);
            focus(s, sel);
        }
    }
}

/// The world's ore mix: abundance, mountains only, the rarity mix by tier and (folded) every ore
/// on/off with its weight. Returns true when something changed.
pub fn ore_settings_ui(ui: &mut egui::Ui, st: &mut ores::OreSettings, salt: &str) -> bool {
    let before = st.clone();
    dropdown_row(
        ui,
        "Amount",
        ("ore_amount", salt),
        st.abundance.name(),
        |ui| {
            for a in ores::Abundance::ALL {
                menu_option(ui, &mut st.abundance, a, a.name());
            }
        },
    );
    toggle(ui, &mut st.mountains_only, "Mountains only").on_hover_text(
        "Ore only on mountainous ground: high up, on steep high slopes, or in alpine country",
    );
    sub_fold(
        ui,
        &format!("ore_mix_{salt}"),
        "Rarity mix",
        "",
        false,
        TEXT,
        |ui| {
            hint_small(
                ui,
                "How the wilderness's deposits split by tier (zones follow their level range). Rarer tiers lean towards high, steep and harsh ground.",
            );
            let mix = st.mix();
            for t in 0..5usize {
                let r = Rarity::of_tier(t as u8 + 1);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(82.0, 18.0),
                        egui::Label::new(RichText::new(r.name()).size(12.5).color(rarity_color(
                            Ore::of_tier(t as u8 + 1).next().unwrap_or(Ore::Copper),
                        ))),
                    );
                    ui.spacing_mut().slider_width = (ui.available_width() - 44.0).max(60.0);
                    ui.add(TrackSlider::new(&mut st.tier_mix[t], 0.0..=100.0).show_value(false));
                    ui.label(
                        RichText::new(format!("{:.0}%", mix[t] * 100.0))
                            .font(FontId::new(11.0, mono()))
                            .color(TEXT_DIM),
                    );
                });
            }
            if st.tier_mix != ores::DEFAULT_MIX
                && small_button(ui, "Reset to 60 · 25 · 10 · 4 · 1").clicked()
            {
                st.tier_mix = ores::DEFAULT_MIX;
            }
        },
    );
    sub_fold(
        ui,
        &format!("ore_each_{salt}"),
        "Individual ores",
        "",
        false,
        TEXT,
        |ui| {
            hint_small(
                ui,
                "Switch ores off, or make one more or less common within its tier.",
            );
            for t in 1..=5u8 {
                ui.label(
                    RichText::new(format!("Tier {t} · {}", Rarity::of_tier(t).name()))
                        .size(11.5)
                        .color(TEXT_MUTE),
                );
                for o in Ore::of_tier(t) {
                    ui.horizontal(|ui| {
                        let mut on = st.enabled(o);
                        let col = if on { rarity_color(o) } else { TEXT_DIM };
                        if toggle(ui, &mut on, RichText::new(o.name()).color(col))
                            .on_hover_ui(|ui| ore_hint(ui, o))
                            .changed()
                        {
                            st.set_enabled(o, on);
                        }
                        if on {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let mut w = st.weight(o);
                                    if ui
                                        .add(
                                            egui::DragValue::new(&mut w)
                                                .range(0.0..=10.0)
                                                .speed(0.02)
                                                .max_decimals(2)
                                                .prefix("×"),
                                        )
                                        .on_hover_text("How common within its tier (×1 normal)")
                                        .changed()
                                    {
                                        st.set_weight(o, w);
                                    }
                                },
                            );
                        }
                    });
                }
            }
            if (!st.off.is_empty() || !st.weights.is_empty())
                && small_button(ui, "All on, ×1").clicked()
            {
                st.off.clear();
                st.weights.clear();
            }
        },
    );
    *st != before
}

/// Scatter for ore only (one undo step).
fn scatter_ore(s: &mut UiState) {
    let params = scatter::ScatterParams {
        kinds: vec![ResourceKind::Ore],
        ore: s.gameplay.ore_settings.clone(),
        ..s.gp_ed.scatter.clone()
    };
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
    s.history.begin("Scatter ore");
    s.history.touch_gameplay(&s.gameplay);
    let (added, _) = scatter::apply(&mut s.gameplay, &params, &g, &s.store, s.lib.as_deref());
    s.history.commit(&s.store);
    s.meta.dirty = true;
    let mut by: Vec<(Ore, usize)> = Ore::ALL
        .iter()
        .map(|o| {
            (
                *o,
                s.gameplay
                    .nodes
                    .iter()
                    .filter(|n| n.auto && n.ore == Some(*o))
                    .count(),
            )
        })
        .filter(|x| x.1 > 0)
        .collect();
    by.sort_by_key(|x| std::cmp::Reverse(x.1));
    let top: Vec<String> = by
        .iter()
        .take(4)
        .map(|(o, n)| format!("{n} {}", o.name().to_lowercase()))
        .collect();
    s.ed.toast(format!(
        "Scattered {added} ore deposits: {}{}",
        top.join(", "),
        if by.len() > 4 { "…" } else { "" }
    ));
}

/// A combo of every ore by tier (names in their rarity colour).
fn ore_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    cur: &mut Ore,
    width: f32,
) {
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(RichText::new(cur.name()).color(rarity_color(*cur)))
        .dropdown_ui(ui, |ui| {
            for tier in 1..=5u8 {
                ui.label(
                    RichText::new(format!("Tier {tier} · {}", Rarity::of_tier(tier).name()))
                        .small()
                        .color(TEXT_DIM),
                );
                for o in Ore::of_tier(tier) {
                    menu_option(ui, cur, o, RichText::new(o.name()).color(rarity_color(o)))
                        .on_hover_ui(|ui| ore_hint(ui, o));
                }
            }
        });
}

pub(super) fn area_inspector(
    ui: &mut egui::Ui,
    s: &mut UiState,
    id: u32,
    _action: &mut Option<Action>,
) {
    let g = Ground {
        terrain: &s.terrain,
        zones: &s.zones,
        water: &s.water,
        nature: None,
    };
    let gp = &mut *s.gameplay;
    let Some(idx) = gp.ore_areas.iter().position(|a| a.id == id) else {
        return;
    };
    let count = gp.nodes.iter().filter(|n| n.area == Some(id)).count();
    let info = g.region_info(&gp.ore_areas[idx].polygon);
    let before = gp.ore_areas[idx].clone();
    let a = &mut gp.ore_areas[idx];
    let zone = info
        .zone
        .and_then(|z| g.zones.by_id(z))
        .map(|z| format!("{} ({})", z.name, level_label(levels_of(z))))
        .unwrap_or("no zone".into());
    hero(
        ui,
        AREA_ICON,
        area_color(a),
        &a.name.clone(),
        &format!("{:.2} ha · {count} deposits", a.area() / 1.0e4),
        &zone,
    );
    name_field(ui, &mut a.name);

    section_fold(
        ui,
        "gp_area_ores",
        "Ores",
        &a.table.len().to_string(),
        |ui| {
            let chances = a.chances();
            let mut remove = None;
            for (i, e) in a.table.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    let (dr, _) = ui.allocate_exact_size(vec2(10.0, 20.0), Sense::hover());
                    ui.painter()
                        .circle_filled(dr.center(), 4.5, ore_color(e.ore));
                    ore_combo(ui, ("gp_area_ore", i), &mut e.ore, 108.0);
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
                                .max_decimals(2),
                        )
                        .on_hover_text("Weight: how often it appears relative to the others");
                    });
                });
            }
            if let Some(i) = remove {
                a.table.remove(i);
            }
            let mut add = None;
            egui::ComboBox::from_id_salt("gp_area_add")
                .width(ui.available_width())
                .selected_text("Add an ore…")
                .dropdown_ui(ui, |ui| {
                    for tier in 1..=5u8 {
                        ui.label(
                            RichText::new(format!(
                                "Tier {tier} · {}",
                                Rarity::of_tier(tier).name()
                            ))
                            .small()
                            .color(TEXT_DIM),
                        );
                        for o in Ore::of_tier(tier).filter(|o| !a.table.iter().any(|e| e.ore == *o))
                        {
                            if menu_check(ui, false, RichText::new(o.name()).color(rarity_color(o)))
                                .on_hover_ui(|ui| ore_hint(ui, o))
                                .clicked()
                            {
                                add = Some(o);
                            }
                        }
                    }
                });
            if let Some(o) = add {
                a.table.push(OreEntry {
                    ore: o,
                    weight: 0.5,
                });
            }
        },
    );

    section_fold(ui, "gp_area_deposits", "Deposits", "", |ui| {
        slider_row_log(ui, "Density", &mut a.density, 2.0..=200.0, |v| {
            format!("{v:.0} per hectare")
        });
        slider_row_log(
            ui,
            "Respawn",
            &mut a.respawn_secs,
            30.0..=7200.0,
            secs_label,
        );
        num_row(ui, "Arrangement", |ui| {
            if small_icon_button(ui, "Dice5", "Shuffle")
                .on_hover_text("Another arrangement of the deposits")
                .clicked()
            {
                a.seed = fastrand::u32(1..999_999);
            }
        });
        if count == 0 && !a.table.is_empty() {
            icon_line(
                ui,
                "ExclamationTriangle",
                WARN,
                "No deposit fits: the ground inside is under water, too steep, paved or built over.",
            );
        }
    });
    hint_small(
        ui,
        "Drag the white corners in the world to reshape it; drag inside it to move it. The deposits follow.",
    );
    notes_field(ui, &mut a.notes);
    // the deposits follow every change of the area (respawn: just their timers)
    let a = &gp.ore_areas[idx];
    if a.table != before.table || a.density != before.density || a.seed != before.seed {
        let site = Site {
            g: &g,
            store: Some(&*s.store),
            lib: s.lib.as_deref(),
            roads: Some(&*s.roads),
        };
        ores::rescatter(gp, id, &site);
    } else if a.respawn_secs != before.respawn_secs {
        let r = a.respawn_secs;
        for n in gp.nodes.iter_mut().filter(|n| n.area == Some(id)) {
            n.respawn_secs = r;
        }
    }
}

/// The fields of a node that yields an ore: the ore (its tier and an automatic name follow),
/// respawn, where it came from.
pub(super) fn deposit_fields(
    ui: &mut egui::Ui,
    n: &mut ResourceNode,
    ore: Ore,
    area: Option<&str>,
) {
    let mut o = ore;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ore").color(TEXT_DIM));
        ore_combo(ui, "gp_node_ore", &mut o, ui.available_width() - 8.0);
    });
    // (a node saved before ores existed keeps its implied ore until one is picked)
    if o != ore {
        if n.variant == ore.deposit_name() {
            n.variant = o.deposit_name();
        }
        (n.ore, n.kind, n.tier) = (Some(o), o.kind(), o.tier());
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        crate::ui::icons::icon(ui, ORE_ICON, 13.0, rarity_color(o));
        ui.label(
            RichText::new(format!(
                "{} · Tier {} {} · {}",
                o.name(),
                o.tier(),
                o.rarity().name(),
                o.uses()
            ))
            .size(11.5)
            .color(rarity_color(o)),
        );
    });
    text_field(ui, "Name", &mut n.variant, "");
    slider_row_log(
        ui,
        "Respawn",
        &mut n.respawn_secs,
        10.0..=7200.0,
        secs_label,
    );
    match area {
        Some(name) => hint_small(
            ui,
            &format!(
                "Part of the deposit area {name}: changing the area scatters its deposits again."
            ),
        ),
        None if n.auto => hint_small(
            ui,
            "Placed by Scatter. Editing it keeps it when you scatter again.",
        ),
        None => {}
    }
}
