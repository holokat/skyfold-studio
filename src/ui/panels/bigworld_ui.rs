//! Erosion (Sculpt tab), the continent / coastline generator (Land tab and the New World
//! dialog) and world sizes with their memory and performance hints.

use bevy_egui::egui::{self, FontId, RichText, vec2};
use egui_phosphor::regular as icon;

use super::super::theme::*;
use super::super::widgets::*;
use super::super::{UiAction, UiOut, UiState};
use crate::bigworld::{NewShape, Request, SIZES, memory_gb, perf_hint};
use crate::terrain::continent::ContinentPreset;
use crate::terrain::erosion::ErosionPreset;
use crate::terrain::worldgen::WorldPreset;

fn shape_icon(s: NewShape) -> &'static str {
    match s {
        NewShape::Continent(p) => match p {
            ContinentPreset::Continent => icon::GLOBE_HEMISPHERE_WEST,
            ContinentPreset::Archipelago => icon::DOTS_NINE,
            ContinentPreset::TwinIslands => icon::UNITE,
            ContinentPreset::Peninsula => icon::MAP_TRIFOLD,
            ContinentPreset::RingAtoll => icon::CIRCLE_DASHED,
            ContinentPreset::FjordCoast => icon::WAVE_TRIANGLE,
            ContinentPreset::MountainIsland => icon::ISLAND,
            ContinentPreset::InlandSea => icon::CIRCLE_NOTCH,
        },
        NewShape::Blank(WorldPreset::Plains) => icon::TREE,
        NewShape::Blank(_) => icon::WAVES,
    }
}

fn shape_name(s: NewShape) -> &'static str {
    match s {
        NewShape::Continent(p) => p.name(),
        NewShape::Blank(WorldPreset::Plains) => "Flat Plains",
        NewShape::Blank(_) => "Empty Ocean",
    }
}

/// The continent presets, then the blank canvases (empty ocean, flat plains).
fn shapes() -> impl Iterator<Item = NewShape> {
    let blank = WorldPreset::ALL
        .into_iter()
        .filter(|p| matches!(p, WorldPreset::Ocean | WorldPreset::Plains));
    ContinentPreset::ALL
        .into_iter()
        .map(NewShape::Continent)
        .chain(blank.map(NewShape::Blank))
}

fn erosion_central(p: ErosionPreset) -> &'static str {
    match p {
        ErosionPreset::LightRain => "RainyLight",
        ErosionPreset::HeavyRain => "Rainy",
        ErosionPreset::AncientMountains => "FormPyramide",
        ErosionPreset::Glacial => "SnowFlakes",
        ErosionPreset::DesertWind => "Wind",
    }
}

/// Sculpt tab: the boxed Erosion section (presets for the Erode brush and the whole world).
pub fn erosion_section(ui: &mut egui::Ui, s: &mut UiState, out: &mut UiOut) {
    section_box(ui, "sculpt_erosion", "Erosion", |ui| {
        hint_small(
            ui,
            "How the Erode brush and the whole-world button wear the land down.",
        );
        let presets = ErosionPreset::ALL;
        let cur = presets.iter().position(|p| *p == s.bigworld.erosion_preset);
        if let Some(i) = card_grid(ui, 3, presets.len(), |ui, i, w| {
            let p = presets[i];
            icon_card(ui, erosion_central(p), p.name(), cur == Some(i), w).on_hover_text(p.hint())
        }) {
            s.bigworld.erosion_preset = presets[i];
        }
        hint_small(ui, s.bigworld.erosion_preset.hint());
        slider_row(
            ui,
            "Strength",
            &mut s.bigworld.erosion_strength,
            0.25..=2.0,
            |v| format!("×{v:.2}"),
        );
        let n = s.terrain.n() as f32;
        let secs = (3.5 * (n * n / 1.05e6).powf(0.8)).max(1.0);
        if wide_icon_button(
            ui,
            erosion_central(s.bigworld.erosion_preset),
            "Erode entire world",
        )
        .on_hover_text(format!(
            "Runs {} over the whole world (about {secs:.0} s; Cancel any time, one undo step)",
            s.bigworld.erosion_preset.name()
        ))
        .clicked()
        {
            out.actions.write(UiAction::ErodeWorld);
        }
        if wide_icon_button(ui, "Blur", "Soften entire world")
            .on_hover_text("Smooths the whole world a little (one undo step)")
            .clicked()
        {
            out.actions.write(UiAction::SmoothWorld);
        }
    });
}

/// The shape tiles shared by the Land tab and the New World dialog.
fn shape_tiles(ui: &mut egui::Ui, s: &mut UiState, size: egui::Vec2) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for shape in shapes() {
            let tip = match shape {
                NewShape::Continent(p) => p.hint(),
                NewShape::Blank(WorldPreset::Plains) => {
                    "Gentle plains everywhere: a blank canvas to sculpt"
                }
                NewShape::Blank(_) => "Open ocean: a blank canvas for painting continents",
            };
            if tool_tile(
                ui,
                shape_icon(shape),
                shape_name(shape),
                s.bigworld.new_shape == shape,
                size,
            )
            .on_hover_text(tip)
            .clicked()
                && s.bigworld.new_shape != shape
            {
                s.bigworld.new_shape = shape;
                match shape {
                    NewShape::Continent(p) => s.bigworld.continent = p.params(s.ed.new_world_seed),
                    NewShape::Blank(b) => s.ed.new_world_preset = b,
                }
            }
        }
    });
}

/// Land, coast, ranges, relief and erosion of the selected continent preset.
fn continent_settings(ui: &mut egui::Ui, s: &mut UiState) {
    let NewShape::Continent(p) = s.bigworld.new_shape else {
        return;
    };
    dim(ui, p.hint());
    let c = &mut s.bigworld.continent;
    slider_row(ui, "Land", &mut c.land, 0.05..=0.9, |v| {
        format!("{:.0}% of the map", v * 100.0)
    })
    .on_hover_text("How much of the map is land; the rest is sea");
    slider_row(ui, "Coast roughness", &mut c.roughness, 0.0..=1.0, |v| {
        format!("{:.0}%", v * 100.0)
    })
    .on_hover_text("Smooth shores … deep bays, rocky headlands and scattered islets");
    let mut ranges = c.ranges as f32;
    if slider_row(ui, "Mountain ranges", &mut ranges, 0.0..=8.0, |v| {
        format!("{v:.0}")
    })
    .changed()
    {
        c.ranges = ranges.round() as u32;
    }
    if c.ranges > 0 {
        let mut fixed = c.range_dir.is_some();
        ui.horizontal(|ui| {
            toggle(ui, &mut fixed, "Set range direction")
                .on_hover_text("Off: each seed picks its own direction");
        });
        if fixed != c.range_dir.is_some() {
            c.range_dir = fixed.then_some(0.0);
        }
        if let Some(d) = &mut c.range_dir {
            slider_row(ui, "Direction", d, 0.0..=180.0, |v| {
                let names = [
                    "east–west",
                    "ESE–WNW",
                    "SE–NW",
                    "SSE–NNW",
                    "north–south",
                    "SSW–NNE",
                    "SW–NE",
                    "WSW–ENE",
                    "east–west",
                ];
                format!("{v:.0}° {}", names[((v / 22.5).round() as usize).min(8)])
            });
        }
        slider_row(ui, "Mountain height", &mut c.relief, 0.2..=2.0, |v| {
            format!("×{v:.2}")
        });
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Erosion").color(TEXT));
        let label = c.erosion.map_or("None", |e| e.name());
        egui::ComboBox::from_id_salt("continent_erosion")
            .selected_text(label)
            .width(170.0)
            .dropdown_ui(ui, |ui| {
                menu_option(ui, &mut c.erosion, None, "None (raw shapes)");
                for e in ErosionPreset::ALL {
                    menu_option(ui, &mut c.erosion, Some(e), e.name()).on_hover_text(e.hint());
                }
            });
    });
}

fn seed_row(ui: &mut egui::Ui, s: &mut UiState) {
    ui.horizontal(|ui| {
        ui.label("Seed");
        ui.add(egui::DragValue::new(&mut s.ed.new_world_seed).range(0..=999_999));
        if ui
            .button(icon::DICE_FIVE)
            .on_hover_text("Random seed")
            .clicked()
        {
            s.ed.new_world_seed = fastrand::u32(0..999_999);
        }
    });
    s.bigworld.continent.seed = s.ed.new_world_seed;
}

/// The card picture of a shape (onboarding pictures; ring atoll and the blanks are painted).
fn shape_picture(shape: NewShape) -> &'static str {
    match shape {
        NewShape::Continent(p) => match p {
            ContinentPreset::Continent => "land_continent",
            ContinentPreset::Archipelago => "land_archipelago",
            ContinentPreset::TwinIslands => "land_twin_islands",
            ContinentPreset::Peninsula => "land_peninsula",
            ContinentPreset::RingAtoll => "gen:ring_atoll",
            ContinentPreset::FjordCoast => "land_fjord_coast",
            ContinentPreset::MountainIsland => "land_mountain_island",
            ContinentPreset::InlandSea => "land_inland_sea",
        },
        NewShape::Blank(WorldPreset::Plains) => "gen:flat_plains",
        NewShape::Blank(_) => "gen:empty_ocean",
    }
}

fn shape_tip(shape: NewShape) -> &'static str {
    match shape {
        NewShape::Continent(p) => p.hint(),
        NewShape::Blank(WorldPreset::Plains) => {
            "Gentle plains everywhere: a blank canvas to sculpt"
        }
        NewShape::Blank(_) => "Open ocean: a blank canvas for painting continents",
    }
}

/// Range direction presets (degrees: 0 = east–west, 90 = north–south); None = each seed picks.
const DIRECTIONS: [(Option<f32>, &str); 5] = [
    (Some(90.0), "N–S"),
    (Some(135.0), "NE–SW"),
    (Some(0.0), "E–W"),
    (Some(45.0), "NW–SE"),
    (None, "Random"),
];

fn direction_label(d: Option<f32>) -> String {
    match d {
        None => "Random".into(),
        Some(v) => DIRECTIONS
            .iter()
            .find(|(x, _)| x.is_some_and(|x| (x - v).abs() < 0.5))
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| format!("{v:.0}°")),
    }
}

/// Land tab: the boxed "Generate a world" section — shape cards, climate, land, ranges, erosion,
/// Generate; the rest (coast, relief, exact direction, seed, nature, ore) under More options.
pub fn generate_section(ui: &mut egui::Ui, s: &mut UiState, out: &mut UiOut) {
    section_box(ui, "land_generate", "Generate a world", |ui| {
        let all: Vec<NewShape> = shapes().collect();
        let ctx = ui.ctx().clone();
        let cur = s.bigworld.new_shape;
        if let Some(i) = card_grid(ui, 3, all.len(), |ui, i, w| {
            let sh = all[i];
            photo_card(
                ui,
                crate::ui::pictures::get(&ctx, shape_picture(sh)),
                shape_name(sh),
                sh == cur,
                w,
            )
            .on_hover_text(shape_tip(sh))
        }) {
            let shape = all[i];
            if shape != s.bigworld.new_shape {
                s.bigworld.new_shape = shape;
                match shape {
                    NewShape::Continent(p) => s.bigworld.continent = p.params(s.ed.new_world_seed),
                    NewShape::Blank(b) => s.ed.new_world_preset = b,
                }
            }
        }
        // climate (None = no biomes)
        let climate = if s.ed.new_world_biomes {
            s.ed.new_world_climate.name()
        } else {
            "None"
        };
        dropdown_row(ui, "Climate", "gen_climate", climate, |ui| {
            if menu_check(ui, !s.ed.new_world_biomes, "None (no biomes)").on_hover_text("Bare land: no biomes, ground, forests or plants").clicked() {
                s.ed.new_world_biomes = false;
            }
            for c in crate::biomegen::Climate::ALL {
                if menu_check(ui, s.ed.new_world_biomes && s.ed.new_world_climate == c, c.name()).clicked() {
                    s.ed.new_world_biomes = true;
                    s.ed.new_world_climate = c;
                }
            }
        })
        .on_hover_text("Fill the land with climate-driven biomes (their ground, grass, forests, plants, rocks, weather and ambience). No zones are created");
        if let NewShape::Continent(_) = s.bigworld.new_shape {
            let c = &mut s.bigworld.continent;
            slider_row(ui, "Land", &mut c.land, 0.05..=0.9, |v| {
                format!("{:.0}%", v * 100.0)
            })
            .on_hover_text("How much of the map is land; the rest is sea");
            let mut ranges = c.ranges as f32;
            if slider_row(ui, "Mountain ranges", &mut ranges, 0.0..=8.0, |v| {
                format!("{v:.0}")
            })
            .changed()
            {
                c.ranges = ranges.round() as u32;
            }
            if c.ranges > 0 {
                dropdown_row(
                    ui,
                    "Range direction",
                    "gen_dir",
                    direction_label(c.range_dir),
                    |ui| {
                        for (d, l) in DIRECTIONS {
                            menu_option(ui, &mut c.range_dir, d, l);
                        }
                    },
                )
                .on_hover_text("Random: each seed picks its own direction");
            }
            let label = c.erosion.map_or("None", |e| e.name());
            dropdown_row(ui, "Erosion", "continent_erosion", label, |ui| {
                menu_option(ui, &mut c.erosion, None, "None (raw shapes)");
                for e in ErosionPreset::ALL {
                    menu_option(ui, &mut c.erosion, Some(e), e.name()).on_hover_text(e.hint());
                }
            });
        }
        let shape = s.bigworld.new_shape;
        if primary_icon_button(ui, "MagicWand", &format!("Generate {}", shape_name(shape)))
            .clicked()
        {
            let biomes = s.ed.new_world_biomes.then_some(s.ed.new_world_climate);
            match shape {
                NewShape::Continent(_) => s.bigworld.requests.push(Request::Generate {
                    new_world: None,
                    biomes,
                    art_style: None,
                    appearance: None,
                }),
                NewShape::Blank(_) => {
                    out.actions.write(UiAction::Regenerate {
                        preset: s.ed.new_world_preset,
                        seed: s.ed.new_world_seed,
                        biomes,
                    });
                }
            }
        }
        hint_small(
            ui,
            "Replaces the terrain with a fresh landmass. One undo step.",
        );
        section_fold(ui, "gen_more", "More options", "", |ui| {
            if let NewShape::Continent(_) = s.bigworld.new_shape {
                let c = &mut s.bigworld.continent;
                slider_row(ui, "Coast roughness", &mut c.roughness, 0.0..=1.0, |v| {
                    format!("{:.0}%", v * 100.0)
                })
                .on_hover_text("Smooth shores … deep bays, rocky headlands and scattered islets");
                if c.ranges > 0 {
                    slider_row(ui, "Mountain height", &mut c.relief, 0.2..=2.0, |v| {
                        format!("×{v:.2}")
                    });
                    if let Some(d) = &mut c.range_dir {
                        slider_row(ui, "Exact direction", d, 0.0..=180.0, |v| {
                            let names = [
                                "east–west",
                                "ESE–WNW",
                                "SE–NW",
                                "SSE–NNW",
                                "north–south",
                                "SSW–NNE",
                                "SW–NE",
                                "WSW–ENE",
                                "east–west",
                            ];
                            format!("{v:.0}° {}", names[((v / 22.5).round() as usize).min(8)])
                        });
                    }
                }
            }
            seed_row(ui, s);
            if s.ed.new_world_biomes {
                super::nature_dials(ui, s);
            }
            ore_dials(ui, s);
        });
    });
}

/// "Spawn ore deposits" for generated worlds, with the world's ore mix folded underneath.
fn ore_dials(ui: &mut egui::Ui, s: &mut UiState) {
    let mut st = s.gameplay.ore_settings.clone();
    toggle(ui, &mut st.on_generate, "Spawn ore deposits").on_hover_text("Scatter mineable ore over the new world where each ore forms (Gameplay › Ore changes it later)");
    if st.on_generate {
        egui::CollapsingHeader::new(RichText::new("Ore settings").color(TEXT))
            .id_salt("gen_ore")
            .show(ui, |ui| {
                super::gameplay_tab::ore_ui::ore_settings_ui(ui, &mut st, "gen");
            });
    }
    if st != s.gameplay.ore_settings {
        s.gameplay.ore_settings = st;
        s.meta.dirty = true;
    }
}

/// The New World dialog's contents. Returns true when a world is being created.
pub fn new_world_contents(ui: &mut egui::Ui, s: &mut UiState, out: &mut UiOut) -> bool {
    // the guided setup: a playable world from a few questions (crate::setup)
    if crate::setup::wizard::entry_card(ui) {
        crate::setup::wizard::request_open();
        return true;
    }
    ui.add_space(8.0);
    section(ui, "Or start from a blank world");
    dim(
        ui,
        "Choose a starting point. You can paint continents and sculpt everything afterwards.",
    );
    ui.add_space(6.0);
    shape_tiles(ui, s, vec2(80.0, 66.0));
    ui.add_space(8.0);
    ui.add_space(5.0);
    ui.label(
        RichText::new("Art style")
            .font(FontId::new(13.0, semibold()))
            .color(TEXT),
    );
    dim(
        ui,
        "Sets the new world's surfaces and matching builder assets. Change it later in Atmosphere.",
    );
    segmented(
        ui,
        &mut s.ed.new_world_art_style,
        &[
            (crate::art_style::ArtStyle::Realistic, "Photorealistic"),
            (crate::art_style::ArtStyle::HandPainted, "Hand-painted"),
        ],
    );
    if s.ed.new_world_art_style.painted() && !crate::art_style::painted_usable() {
        hint_small(
            ui,
            "Hand-painted surfaces need the Hand-painted Fantasy pack. Until it is unlocked, the new world uses realistic surfaces.",
        );
    }
    ui.add_space(8.0);
    section(ui, "World size");
    let bw = &mut s.bigworld;
    ui.horizontal(|ui| {
        for (km, cells) in SIZES {
            if ui
                .selectable_label(bw.size_km == km, format!("{km} km"))
                .clicked()
            {
                bw.size_km = km;
                if !cells.contains(&bw.cell) {
                    bw.cell = if cells.contains(&4.0) { 4.0 } else { cells[0] };
                }
            }
        }
    });
    let cells = SIZES
        .iter()
        .find(|s| s.0 == bw.size_km)
        .map(|s| s.1)
        .unwrap_or(&[4.0]);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ground detail").color(TEXT_DIM));
        for &c in cells {
            let label = match c as u32 {
                2 => "Fine · 2 m",
                4 => "Standard · 4 m",
                _ => "Coarse · 8 m",
            };
            if ui.selectable_label(bw.cell == c, label).clicked() {
                bw.cell = c;
            }
        }
    });
    let res = bw.new_res();
    s.ed.new_world_size = res;
    let gb = memory_gb(res, s.bigworld.cell, s.ed.new_world_biomes);
    dim(
        ui,
        &format!("{res} × {res} cells · the editor will use about {gb:.1} GB of memory"),
    );
    dim(ui, perf_hint(res));
    ui.add_space(6.0);
    // a world traced from a picture: coast, mountains, water and biomes read from it
    let pic = ui
        .add(egui::Button::new(RichText::new(format!("{}  From a picture…", icon::IMAGE)).font(FontId::new(14.0, semibold()))).min_size(vec2(ui.available_width(), 36.0)))
        .on_hover_text("Pick a map, sketch, satellite view or painting: the coast, mountains, rivers, lakes and biomes are traced from it into a new world of the size chosen below. Fine-tune it afterwards in Land › Trace a Picture.");
    if pic.clicked() {
        crate::dialogs::pick_file(
            "new_world_picture",
            rfd::AsyncFileDialog::new()
                .set_title("Start a world from a picture")
                .add_filter("Pictures", &["png", "jpg", "jpeg"]),
        );
    }
    // (the dialog answers on a later frame)
    if let Some(path) = crate::dialogs::take_file("new_world_picture") {
        out.actions.write(UiAction::NewWorldFromPicture {
            path,
            size_m: s.bigworld.size_km as f32 * 1024.0,
            art_style: s.ed.new_world_art_style,
            appearance: s.ed.new_world_appearance,
        });
        return true;
    }
    ui.add_space(6.0);

    seed_row(ui, s);
    if matches!(s.bigworld.new_shape, NewShape::Continent(_)) {
        egui::CollapsingHeader::new("Shape settings")
            .default_open(false)
            .show(ui, |ui| continent_settings(ui, s));
    }
    super::biome_options(ui, s);
    section_fold(
        ui,
        "new_world_nature_appearance",
        "Forest and grass",
        "saved in the new world",
        |ui| {
            let appearance = &mut s.ed.new_world_appearance;
            toggle(ui, &mut appearance.forest.enabled, "Rich forest diversity");
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
            slider_row(
                ui,
                "Grass lushness",
                &mut appearance.grass_lushness,
                0.0..=1.0,
                |v| format!("{:.0}%", v * 100.0),
            )
            .on_hover_text("Leaves grass exactly as it is at 0%.");
        },
    );
    toggle(ui, &mut s.ed.new_world_fields, "Farm fields").on_hover_text("Patchworks of fields (wheat, corn, vineyards…) on the flat lowland, crops by biome. Add or remove them later in Nature › Fields");
    ore_dials(ui, s);
    ui.add_space(10.0);
    if primary_button(ui, "Create world").clicked() {
        if s.ed.new_world_fields {
            s.fields.auto_pending = Some(s.terrain.generation);
        }
        let biomes = s.ed.new_world_biomes.then_some(s.ed.new_world_climate);
        let cell = s.bigworld.cell;
        match s.bigworld.new_shape {
            NewShape::Continent(_) => s.bigworld.requests.push(Request::Generate {
                new_world: Some((s.ed.new_world_size, cell)),
                biomes,
                art_style: Some(s.ed.new_world_art_style),
                appearance: Some(s.ed.new_world_appearance),
            }),
            NewShape::Blank(_) => {
                out.actions.write(UiAction::NewWorld {
                    preset: s.ed.new_world_preset,
                    res: s.ed.new_world_size,
                    cell,
                    seed: s.ed.new_world_seed,
                    biomes,
                    art_style: s.ed.new_world_art_style,
                    appearance: s.ed.new_world_appearance,
                });
            }
        }
        return true;
    }
    dim(
        ui,
        "Creating a new world replaces the current one (save first!).",
    );
    false
}
