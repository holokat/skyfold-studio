//! Nature › Wild nature in the Halo layout (HANDOFF › Nature): Auto-populate / Clear, the world
//! dials, the asset browser and the biome brushes in the panel; the Grow / Place one / Thin / Erase
//! switch and the brush in the dock; the selected asset and the wind in the inspector.

use bevy_egui::egui::{self, CornerRadius, FontId, RichText, Ui, vec2};

use super::super::theme::*;
use super::super::widgets::*;
use super::super::{UiAction, UiOut, UiState};
use super::{asset_grid, meters, nature_dials, wind_controls};
use crate::editor::ScatterMode;
use crate::objects::{Category, presets};
use crate::world::Biome;

fn brush_icon(name: &str, biome: Biome) -> &'static str {
    match name {
        "Flower Meadow" => "Blossom",
        "Alpine" | "Rocky Hills" => "FormPyramide",
        "Jungle" => "Grass",
        "Palm Beach" => "Sunrise",
        "Desert" => "SunHigh",
        "Swamp" => "Frog",
        "Haunted Woods" => "Ghost",
        "Volcanic" => "Fire1",
        "Crystal Grove" => "Diamond",
        "Tundra" => "SnowFlakes",
        _ => match biome {
            Biome::Desert => "SunHigh",
            Biome::Alpine => "FormPyramide",
            Biome::Swamp => "Frog",
            Biome::Tropical => "Grass",
            _ => "Tree",
        },
    }
}

pub(super) fn nature_left(ui: &mut Ui, s: &mut UiState, out: &mut UiOut) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ((ui.available_width() - 6.0) * 0.5).floor();
        ui.allocate_ui(vec2(w, 30.0), |ui| {
            if primary_icon_button(ui, "SparklesTwo", "Auto-populate")
                .on_hover_text("Grow forests, meadows and rocks wherever they suit the land and its biomes — also on land without a biome. Resets painted edits (undoable)")
                .clicked()
            {
                out.actions.write(UiAction::PopulateWorld);
            }
        });
        ui.allocate_ui(vec2(w, 30.0), |ui| {
            if wide_icon_button(ui, "TrashCan", "Clear nature").on_hover_text("Turns the nature dials to 0% (undoable)").clicked() {
                out.actions.write(UiAction::ClearNature);
            }
        });
    });
    hint_small(
        ui,
        "Nature grows from the biomes: paint more, thin or erase it, by ecosystem, category or species.",
    );
    section_fold(
        ui,
        "nature_dials",
        "World dials",
        "instant, world-wide",
        |ui| nature_dials(ui, s),
    );
    section_fold(ui, "nature_assets", "Assets", "", |ui| assets(ui, s));
    section_fold(ui, "nature_brushes", "Biome brushes", "", |ui| {
        hint_small(
            ui,
            "Paint a complete ecosystem: trees, undergrowth, rocks and fallen wood.",
        );
        let ps = presets();
        // (the alien-world presets are offered in sci-fi worlds only)
        let scifi = s.meta.kind == crate::world_kind::WorldKind::SciFi;
        let shown: Vec<usize> = (0..ps.len()).filter(|&i| scifi || !ps[i].scifi).collect();
        let items: Vec<(&str, &str, &str)> = shown
            .iter()
            .map(|&i| (brush_icon(ps[i].name, ps[i].biome), ps[i].name, ""))
            .collect();
        let at = s
            .scatter
            .preset
            .and_then(|p| shown.iter().position(|&i| i == p));
        if let Some(k) = tool_grid(ui, 2, &items, at) {
            s.scatter.preset = Some(shown[k]);
        }
    });
}

fn assets(ui: &mut Ui, s: &mut UiState) {
    let mut cat = s.scatter.category;
    if segmented(
        ui,
        &mut cat,
        &[
            (Category::Trees, "Trees"),
            (Category::Rocks, "Rocks"),
            (Category::Plants, "Plants"),
            (Category::Debris, "Debris"),
        ],
    ) {
        s.scatter.category = cat;
    }
    search_field(ui, &mut s.ed.asset_search, "Search assets…");
    let selected = s.scatter.selected.clone();
    let preset_on = s.scatter.preset.is_some();
    let search = s.ed.asset_search.clone();
    let category = s.scatter.category;
    // (the world's art style, like the Objects tab: a hand-painted world lists no photoscans)
    let style = s.place.style.clone();
    if s.meta.art_style.painted() && !s.place.mix_styles {
        let what = match category {
            Category::Trees => "Trees and bushes wear painted bark and leaves here.",
            Category::Plants => {
                "Plants, flowers, mushrooms and berries keep their separate world dials in every art style."
            }
            Category::Rocks => {
                "No hand-painted rocks yet: photoscanned rocks don't grow in a hand-painted world (the ground's painted rock layer shows on slopes)."
            }
            _ => "Photoscanned logs, stumps and branches don't grow in a hand-painted world.",
        };
        hint_small(ui, what);
        if category != Category::Trees
            && small_button(ui, "Show other styles")
                .on_hover_text(
                    "List the photoscans too, to paint some on purpose (for this session)",
                )
                .clicked()
        {
            s.place.mix_styles = true;
            s.place.style.clear();
        }
    }
    if let Some(id) = asset_grid(ui, s, &[category], "", &style, &search, false, &|id| {
        !preset_on && selected.iter().any(|x| x == id)
    }) {
        let multi = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        if s.scatter.preset.is_some() || !multi {
            s.scatter.selected.clear();
        }
        s.scatter.preset = None;
        // picking something to paint means painting it, not erasing or thinning
        if matches!(s.ed.scatter_mode, ScatterMode::Remove | ScatterMode::Thin) {
            s.ed.scatter_mode = ScatterMode::Add;
            s.ed.toast("Brush set to Grow");
        }
        if let Some(pos) = s.scatter.selected.iter().position(|x| *x == id) {
            s.scatter.selected.remove(pos);
        } else {
            s.scatter.selected.push(id);
        }
    }
    hint_small(
        ui,
        "Tip: ⇧-click to paint a mix of several assets. None picked paints the whole category.",
    );
}

/// The inspector: what the brush paints, then the wind.
pub(super) fn nature_right(ui: &mut Ui, s: &mut UiState) {
    if s.field_tool.on {
        crate::fields::ui::right(ui, s);
        return;
    }
    if let Some(i) = s.scatter.preset {
        let ps = presets();
        if let Some(p) = ps.get(i) {
            hero(ui, None, brush_icon(p.name, p.biome), p.name, "Biome brush");
            // (what grows in this world's art style)
            let art = s.meta.art_style;
            let names: Vec<String> = p
                .layers
                .iter()
                .filter(|l| crate::art_style::kind_grows(l.kind, art))
                .map(|l| l.kind.name().to_string())
                .collect();
            hint_small(ui, &names.join(" · "));
        }
    } else if let Some(id) = s.scatter.selected.first().cloned()
        && let Some(lib) = &s.lib
        && let Some(e) = lib.get(&id)
    {
        let thumb = s.previews.thumb(&crate::thumbs::Subject::Asset(id.clone()));
        let more = if s.scatter.selected.len() > 1 {
            format!("Selected asset · + {} more", s.scatter.selected.len() - 1)
        } else {
            "Selected asset".to_string()
        };
        let name = e.name.clone();
        let biomes: Vec<&str> = e.biomes.iter().take(4).map(|b| b.name()).collect();
        let biomes = biomes.join(", ");
        hero(ui, thumb, "Tree", &name, &more);
        if !biomes.is_empty() {
            hint_small(ui, &biomes);
        }
    } else {
        hero(
            ui,
            None,
            "Tree",
            &format!("All {}", s.scatter.category.name().to_lowercase()),
            "Paints the whole category",
        );
        hint_small(
            ui,
            "Paints the whole category, whatever species the biome grows.",
        );
    }
    hint_small(
        ui,
        "To hand-place a single tree or rock: Build › Objects › Select & Move, click it, then Pin.",
    );
    section_fold(ui, "nature_wind", "Wind", "", |ui| wind_controls(ui, s));
}

/// The inspector's hero row: a picture (or icon) with a title and a muted line.
fn hero(ui: &mut Ui, image: Option<egui::TextureId>, icon: &str, title: &str, sub: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(vec2(46.0, 46.0), egui::Sense::hover());
        ui.painter().rect_filled(r, CornerRadius::same(11), SOFT);
        match image {
            Some(t) => {
                egui::Image::new((t, r.size() - vec2(6.0, 6.0)))
                    .corner_radius(CornerRadius::same(9))
                    .paint_at(ui, r.shrink(3.0));
            }
            None => paint_icon(ui, r.center(), icon, 20.0, TEXT),
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.add_space(6.0);
            ui.label(
                RichText::new(title)
                    .font(FontId::new(14.5, semibold()))
                    .color(TEXT),
            );
            ui.label(RichText::new(sub).size(12.0).color(TEXT_MUTE));
        });
    });
}

/// The dock: Grow / Place one / Thin / Erase, then the brush.
pub(super) fn nature_dock(ui: &mut Ui, s: &mut UiState) -> bool {
    if s.field_tool.on {
        return false;
    }
    ui.allocate_ui(vec2(280.0, 26.0), |ui| {
        segmented_solid(
            ui,
            &mut s.ed.scatter_mode,
            &[
                (ScatterMode::Add, "Grow"),
                (ScatterMode::Place, "Place one"),
                (ScatterMode::Thin, "Thin"),
                (ScatterMode::Remove, "Erase"),
            ],
        );
    });
    dock_sep(ui);
    let b = &mut s.ed.nature_brush;
    slider_mini_log(ui, "Size", &mut b.size, 3.0..=600.0, meters);
    slider_mini(ui, "Amount", &mut s.scatter.density, 0.05..=2.0, |v| {
        format!("{v:.2}")
    });
    let b = &mut s.ed.nature_brush;
    slider_mini(ui, "Paint speed", &mut b.strength, 0.05..=1.0, |v| {
        format!("{v:.2}")
    });
    slider_mini(ui, "Falloff", &mut b.falloff, 0.0..=1.0, |v| {
        format!("{v:.2}")
    });
    dock_sep(ui);
    if s.ed.scatter_mode == ScatterMode::Place {
        dock_hints(ui, &[("Click", "Place one")]);
        info(
            ui,
            "Click to place one of the picked species (a random one of them, turned at random). Drag it later in Objects › Select & Move.",
        );
    } else {
        dock_hints(
            ui,
            &[
                ("Click", "Paint"),
                ("⌥", "Thins while growing"),
                ("[ ]", "Size"),
            ],
        );
        info(
            ui,
            "Slopes, water, roads and buildings stay clear by themselves.",
        );
    }
    true
}

/// A small (i) with an explanation on hover.
fn info(ui: &mut Ui, text: &str) {
    let (r, resp) = ui.allocate_exact_size(vec2(16.0, 28.0), egui::Sense::hover());
    paint_icon(
        ui,
        r.center(),
        "CircleInfo",
        13.0,
        if resp.hovered() { TEXT } else { TEXT_MUTE },
    );
    resp.on_hover_text(text);
}
