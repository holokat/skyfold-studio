//! Terrain › Land, Sculpt and Paint in the Halo layout (HANDOFF › Per-tab mapping): tools and
//! options in the tool panel, the brush with its key hints in the dock.

use bevy_egui::egui::{self, Color32, CornerRadius, FontId, Id, RichText, Sense, Ui, vec2};

use super::super::theme::*;
use super::super::widgets::*;
use super::super::{UiAction, UiOut, UiState, UiTextures};
use super::{
    GroundMat, bigworld_ui, layer_usage, mat_name, mat_swatch, meters, pct, pick_material, set_slot,
};
use crate::editor::{LandTool, PaintTool, SculptTool, Tab};
use crate::terrain::layers::{LAYER_COUNT, Layer};

const LAND_TOOLS: [(LandTool, &str, &str); 4] = [
    (LandTool::PaintLand, "PlusMedium", "Paint Land"),
    (LandTool::PaintSea, "MinusMedium", "Paint Sea"),
    (LandTool::DrawCoast, "SelectLasso", "Draw Coastline"),
    (LandTool::Reference, "Images1", "Trace a Picture"),
];

fn land_hint(t: LandTool) -> &'static str {
    match t {
        LandTool::PaintLand => {
            "Brush land up from the sea. Gentle hills and natural coastlines are added for you."
        }
        LandTool::PaintSea => "Sink land below the waves to carve bays, straits and inland seas.",
        LandTool::DrawCoast => {
            "Hold and draw the outline of a continent. Release to raise it. Hold ⌥ while drawing to carve sea instead."
        }
        LandTool::Reference => {
            "Build a world from a map picture: the editor traces the coast, water and regions for you."
        }
    }
}

pub(super) fn land_left(ui: &mut Ui, s: &mut UiState, out: &mut UiOut) {
    section_fold(ui, "land_tools", "Continents", "", |ui| {
        let items: Vec<(&str, &str, &str)> =
            LAND_TOOLS.iter().map(|(_, i, n)| (*i, *n, "")).collect();
        let sel = LAND_TOOLS.iter().position(|t| t.0 == s.ed.land_tool);
        if let Some(i) = tool_grid(ui, 2, &items, sel) {
            s.ed.land_tool = LAND_TOOLS[i].0;
        }
        dim(ui, land_hint(s.ed.land_tool));
    });
    if s.ed.land_tool == LandTool::Reference {
        crate::refmap::ui::reference_left(ui, s);
        return;
    }
    section_fold(ui, "land_shape", "Land shape", "", |ui| {
        slider_row(ui, "Land height", &mut s.ed.land_height, 2.0..=60.0, meters);
        slider_row(
            ui,
            "Coast roughness",
            &mut s.ed.coast_roughness,
            0.0..=1.0,
            pct,
        );
        slider_row(
            ui,
            "Sea level",
            &mut s.terrain.sea_level,
            -50.0..=50.0,
            |v| format!("{v:.1} m"),
        );
    });
    bigworld_ui::generate_section(ui, s, out);
}

fn sculpt_icon(t: SculptTool) -> &'static str {
    match t {
        SculptTool::Raise => "ArrowUp",
        SculptTool::Lower => "ArrowDown",
        SculptTool::Smooth => "Blur",
        SculptTool::Flatten => "LineThickness",
        SculptTool::Mountain => "FormPyramide",
        SculptTool::Hills => "Sunrise",
        SculptTool::Terrace => "ChartWaterfall",
        SculptTool::Erode => "Rainy",
    }
}

pub(super) fn sculpt_left(ui: &mut Ui, s: &mut UiState, out: &mut UiOut) {
    section_fold(ui, "sculpt_tools", "Sculpt", "", |ui| {
        let items: Vec<(&str, &str, &str)> = SculptTool::ALL
            .iter()
            .map(|t| (sculpt_icon(*t), t.name(), t.hint()))
            .collect();
        let sel = SculptTool::ALL.iter().position(|t| *t == s.ed.sculpt_tool);
        if let Some(i) = tool_grid(ui, 4, &items, sel) {
            s.ed.sculpt_tool = SculptTool::ALL[i];
        }
        dim(
            ui,
            "Shape mountains, valleys and cliffs. Snow and rock follow automatically.",
        );
        hint_small(ui, s.ed.sculpt_tool.hint());
    });
    bigworld_ui::erosion_section(ui, s, out);
}

// ------------------------------------------------------------------ Paint

/// The material categories as chips (library `category` ids), then the classic built-in textures.
const CATS: [(&str, &str); 10] = [
    ("", "All"),
    ("grass", "Grass"),
    ("forest", "Forest floor"),
    ("ground", "Soil & ground"),
    ("gravel", "Gravel"),
    ("sand", "Sand"),
    ("rock", "Rock"),
    ("snow", "Snow & ice"),
    ("road", "Paving & roads"),
    ("classic", "Classic"),
];

/// How much of the world each slot covers, sampled at most every 1.5 s.
fn usage(ctx: &egui::Context, s: &UiState) -> [f32; LAYER_COUNT] {
    let id = Id::new("paint_usage");
    let now = s.real_time.elapsed_secs();
    if let Some((t, u)) = ctx.data(|d| d.get_temp::<(f32, [f32; LAYER_COUNT])>(id))
        && now - t < 1.5
    {
        return u;
    }
    let u = layer_usage(&s.terrain);
    ctx.data_mut(|d| d.insert_temp(id, (now, u)));
    u
}

pub(super) fn paint_left(ui: &mut Ui, s: &mut UiState, out: &mut UiOut, tex: &UiTextures) {
    let ctx = ui.ctx().clone();
    let mut tool = s.ed.paint_tool;
    if segmented_icons(
        ui,
        &mut tool,
        &[
            (PaintTool::Paint, "PaintBrush", "Paint"),
            (PaintTool::Erase, "Eraser", "Erase to Auto"),
        ],
        true,
    ) {
        s.ed.paint_tool = tool;
    }
    search_field(ui, &mut s.ed.material_search, "Search materials…");
    // category chips with counts
    let cat_id = Id::new("paint_cat");
    let mut cat: usize = ctx.data(|d| d.get_temp(cat_id)).unwrap_or(0);
    let q = s.ed.material_search.trim().to_lowercase();
    let matches = |m: &crate::terrain::textures::LibraryMaterial| {
        q.is_empty() || m.name.to_lowercase().contains(&q) || m.category.contains(&q)
    };
    let count = |c: &str| -> usize {
        match c {
            "" => {
                s.terrain_lib
                    .materials
                    .iter()
                    .filter(|m| matches(m))
                    .count()
                    + LAYER_COUNT
            }
            "classic" => LAYER_COUNT,
            c => s
                .terrain_lib
                .materials
                .iter()
                .filter(|m| m.category == c && matches(m))
                .count(),
        }
    };
    let opts: Vec<(usize, &str, Option<usize>)> = CATS
        .iter()
        .enumerate()
        .map(|(i, (c, l))| (i, *l, Some(count(c))))
        .collect();
    if chips(ui, &mut cat, &opts) {
        ctx.data_mut(|d| d.insert_temp(cat_id, cat));
    }
    // the swatches
    let painting = s.ed.paint_tool == PaintTool::Paint;
    let cur = s.ed.paint_layer as usize;
    let selected_id = s.layer_choices.get(cur).to_string();
    let filter = CATS[cat.min(CATS.len() - 1)].0;
    let mut items: Vec<(Option<egui::TextureId>, Color32, String, bool, GroundMat)> = vec![];
    if filter != "classic" {
        for m in s
            .terrain_lib
            .materials
            .iter()
            .filter(|m| (filter.is_empty() || m.category == filter) && matches(m))
        {
            let gm = GroundMat::Library(m.id.clone());
            items.push((
                mat_swatch(&ctx, &gm),
                TEXT_DIM,
                m.name.clone(),
                painting && m.id == selected_id,
                gm,
            ));
        }
    }
    if filter.is_empty() || filter == "classic" {
        for i in (0..LAYER_COUNT).filter(|i| {
            q.is_empty()
                || Layer::from_index(*i).name().to_lowercase().contains(&q)
                || filter == "classic"
        }) {
            let sw = Layer::from_index(i).swatch();
            let m = GroundMat::BuiltIn(i);
            let sel = painting && cur == i && selected_id.is_empty();
            items.push((
                mat_swatch(&ctx, &m),
                Color32::from_rgb(sw[0], sw[1], sw[2]),
                Layer::from_index(i).name().to_string(),
                sel,
                m,
            ));
        }
    }
    let cols = 4;
    let d = ((ui.available_width() - 8.0 * (cols as f32 - 1.0)) / cols as f32 - 14.0).floor();
    let mut picked = None;
    for row in items.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for (t, c, label, sel, m) in row {
                if circle_swatch(ui, *t, *c, label, *sel, d).clicked() {
                    picked = Some(m.clone());
                }
            }
        });
    }
    if items.is_empty() {
        hint_small(ui, "No material matches.");
    }
    if let Some(m) = picked {
        pick_material(s, &m);
    }
    replace_card(ui, s);
    section_fold(ui, "paint_grass", "Grass blades", "", |ui| {
        super::grass_dials(ui, s);
        hint_small(
            ui,
            "Lush green grounds (Lush Green Lawn, Green Meadow…) keep their blades; other materials paint them away.",
        );
    });
    // the world's 12 slots, most used first
    let u = usage(&ctx, s);
    let used = u.iter().filter(|v| **v > 1e-4).count();
    section_fold(
        ui,
        "paint_world",
        "In this world",
        &format!("{used} of {LAYER_COUNT}"),
        |ui| {
            hint_small(
                ui,
                "A world blends up to 12 ground materials, managed for you. Click one to paint with it.",
            );
            let mut order: Vec<usize> = (0..LAYER_COUNT).collect();
            order.sort_by(|a, b| u[*b].total_cmp(&u[*a]));
            ui.spacing_mut().item_spacing.y = 2.0;
            for i in order {
                let m = match s.layer_choices.get(i) {
                    "" => GroundMat::BuiltIn(i),
                    id => GroundMat::Library(id.to_string()),
                };
                let t = tex
                    .layers
                    .get(i)
                    .map(|t| t.id())
                    .or_else(|| mat_swatch(&ctx, &m));
                let lead = t.map_or(
                    Lead::Dot({
                        let sw = Layer::from_index(i).swatch();
                        Color32::from_rgb(sw[0], sw[1], sw[2])
                    }),
                    Lead::Swatch,
                );
                let meta = if u[i] >= 0.005 {
                    format!("{:.0}%", u[i] * 100.0)
                } else if u[i] > 1e-4 {
                    "<1%".into()
                } else {
                    "unused".into()
                };
                if list_row(ui, lead, &mat_name(s, i), "", &meta, painting && cur == i).clicked() {
                    pick_material(s, &m);
                }
            }
            ui.add_space(4.0);
            if wide_icon_button(ui, "Eraser", "Clear all paint")
                .on_hover_text("Back to the automatic style everywhere (undoable)")
                .clicked()
            {
                out.actions.write(UiAction::ClearPaint);
            }
        },
    );
    section_fold(ui, "paint_auto", "Automatic style", "", |ui| {
        hint_small(
            ui,
            "Unpainted ground is styled by these rules: rock on steep slopes, beaches by the sea, snow up high.",
        );
        let st = &mut s.terrain.style;
        slider_row(ui, "Snow line", &mut st.snow_line, 20.0..=800.0, meters);
        slider_row(ui, "Snow blend", &mut st.snow_blend, 2.0..=150.0, meters);
        slider_row(ui, "Rock on slopes", &mut st.rock_slope, 0.15..=0.9, |v| {
            format!("{:.0}°", (1.0 - v).acos().to_degrees())
        });
        slider_row(ui, "Beach height", &mut st.beach_height, 0.0..=12.0, |v| {
            format!("{v:.1} m")
        });
        slider_row(ui, "Dry grass", &mut st.dryness, 0.0..=1.0, pct);
        slider_row(ui, "Wet gullies", &mut st.wet_gullies, 0.0..=1.0, pct).on_hover_text("Damp soil, mud and stream gravel where rain gathers: gullies, stream beds, valley floors");
        toggle(ui, &mut s.ed.surface_overlay, "Show footstep surfaces");
    });
}

/// A 13th material: which slot makes room for it.
fn replace_card(ui: &mut Ui, s: &mut UiState) {
    let Some(id) = s.ed.paint_replace.clone() else {
        return;
    };
    let name = s
        .terrain_lib
        .materials
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.clone())
        .unwrap_or(id.clone());
    let usage = layer_usage(&s.terrain);
    egui::Frame::new()
        .fill(SOFT)
        .stroke(egui::Stroke::new(1.0, LINE_STRONG))
        .corner_radius(CornerRadius::same(R_TILE))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(format!(
                    "A world blends up to 12 ground materials. Which one should {name} replace?"
                ))
                .color(TEXT),
            );
            hint_small(ui, "It takes that one's place everywhere it's used.");
            let mut order: Vec<usize> = (0..LAYER_COUNT).collect();
            order.sort_by(|a, b| usage[*a].total_cmp(&usage[*b]));
            ui.spacing_mut().item_spacing.y = 2.0;
            for i in order {
                if list_row(
                    ui,
                    Lead::None,
                    &mat_name(s, i),
                    "",
                    &format!("{:.1}%", usage[i] * 100.0),
                    false,
                )
                .clicked()
                {
                    set_slot(s, i, &id);
                    s.ed.toast(format!("{name} replaced {}", Layer::from_index(i).name()));
                    return;
                }
            }
            if button(ui, "Cancel").clicked() {
                s.ed.paint_replace = None;
            }
        });
}

// ------------------------------------------------------------------ the dock

/// The dock's contents for the Terrain tabs; false when the tab isn't one of them.
pub(super) fn dock(ui: &mut Ui, s: &mut UiState, tex: &UiTextures) -> bool {
    match s.ed.tab {
        Tab::Land => {
            let (_, icon, name) = LAND_TOOLS
                .iter()
                .find(|t| t.0 == s.ed.land_tool)
                .copied()
                .unwrap_or(LAND_TOOLS[0]);
            dock_title(ui, icon, name);
            dock_sep(ui);
            match s.ed.land_tool {
                LandTool::DrawCoast => {
                    dock_text(
                        ui,
                        "The coastline tool doesn't use a brush: just draw the outline.",
                    );
                    dock_sep(ui);
                    dock_hints(ui, &[("Drag", "Draw the coast"), ("⌥", "Carve sea")]);
                }
                LandTool::Reference => {
                    dock_text(
                        ui,
                        "Line the picture up, then read and build it in the panel.",
                    );
                    dock_sep(ui);
                    dock_hints(ui, &[("Drag", "Draw"), ("⌥", "Un-ignore")]);
                }
                _ => {
                    brush_sliders(ui, s);
                    altitude(ui, s);
                    dock_sep(ui);
                    dock_hints(
                        ui,
                        &[
                            ("Click", "Paint"),
                            (
                                "⌥",
                                if s.ed.land_tool == LandTool::PaintLand {
                                    "Sink"
                                } else {
                                    "Raise"
                                },
                            ),
                            ("[ ]", "Size"),
                        ],
                    );
                }
            }
            true
        }
        Tab::Sculpt => {
            let t = s.ed.sculpt_tool;
            dock_title(ui, sculpt_icon(t), t.name());
            dock_sep(ui);
            brush_sliders(ui, s);
            if t == SculptTool::Terrace {
                slider_mini(
                    ui,
                    "Terrace height",
                    &mut s.ed.terrace_step,
                    2.0..=60.0,
                    meters,
                );
            }
            altitude(ui, s);
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Sculpt"), ("⌥", "Invert"), ("[ ]", "Size")]);
            true
        }
        Tab::Paint => {
            let l = s.ed.paint_layer;
            if s.ed.paint_tool == PaintTool::Erase {
                dock_title(ui, "Eraser", "Erase to Auto");
            } else {
                let i = l as usize;
                let m = match s.layer_choices.get(i) {
                    "" => GroundMat::BuiltIn(i),
                    id => GroundMat::Library(id.to_string()),
                };
                let t = tex
                    .layers
                    .get(i)
                    .map(|t| t.id())
                    .or_else(|| mat_swatch(ui.ctx(), &m));
                let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                match t {
                    Some(t) => {
                        egui::Image::new((t, r.size()))
                            .corner_radius(CornerRadius::same(255))
                            .paint_at(ui, r);
                    }
                    None => {
                        let sw = l.swatch();
                        ui.painter().circle_filled(
                            r.center(),
                            12.0,
                            Color32::from_rgb(sw[0], sw[1], sw[2]),
                        );
                    }
                }
                ui.label(
                    RichText::new(mat_name(s, i))
                        .font(FontId::new(13.0, semibold()))
                        .color(TEXT),
                );
            }
            dock_sep(ui);
            brush_sliders(ui, s);
            dock_sep(ui);
            let surf = s.ed.layer_surface[l as usize];
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 28.0), Sense::hover());
            paint_icon(ui, r.center(), "Footsteps", 13.0, TEXT_DIM);
            ui.label(
                RichText::new(format!("Footsteps: {}", surf.name()))
                    .size(12.5)
                    .color(TEXT_DIM),
            );
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Paint"), ("⌥", "Erase"), ("[ ]", "Size")]);
            true
        }
        _ => false,
    }
}

/// Size / Strength / Falloff of the active brush as mini sliders.
pub(super) fn brush_sliders(ui: &mut Ui, s: &mut UiState) {
    let Some(b) = s.ed.active_brush_mut() else {
        return;
    };
    slider_mini_log(ui, "Size", &mut b.size, 2.0..=1500.0, meters);
    slider_mini(ui, "Strength", &mut b.strength, 0.02..=1.0, |v| {
        format!("{v:.2}")
    });
    slider_mini(ui, "Falloff", &mut b.falloff, 0.0..=1.0, |v| {
        format!("{v:.2}")
    });
}

/// The altitude under the cursor (hover: the terrain's grid).
fn altitude(ui: &mut Ui, s: &UiState) {
    let text = match s.pointer.hit {
        Some(h) => format!("alt {:.1} m", h.y - s.terrain.sea_level),
        None => "alt –".into(),
    };
    ui.label(
        RichText::new(text)
            .font(FontId::new(11.0, mono()))
            .color(TEXT_DIM),
    )
    .on_hover_text(format!(
        "Altitude under the cursor\n{} × {} cells · {:.0} m per cell",
        s.terrain.res, s.terrain.res, s.terrain.cell
    ));
}
