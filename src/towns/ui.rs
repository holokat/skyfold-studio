//! Objects tab › Set pieces: the Village and Town tiles (with a one-time tips card), and the
//! town card on the right: the size picker (the one setting shown), Shuffle and the folded
//! options (roads, wall, style).

use bevy_egui::egui::{self, Color32, FontId, RichText, vec2};
use egui_phosphor::regular as icon;

use super::{Style, TownSize};
use crate::editor::ObjectTool;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// The style a town would be built in now (Auto: hand-painted when that pack is usable).
fn style_now(s: &UiState) -> Style {
    match s.towns.style {
        Style::Auto if crate::market::usable("hp:inn") => Style::Handpainted,
        Style::Auto => Style::Photoreal,
        st => st,
    }
}

/// The model whose thumbnail stands for a size in a style.
fn icon_asset(size: TownSize, style: Style) -> &'static str {
    match (size.is_town(), style) {
        (false, Style::Photoreal) => "sb:caretaker_hut",
        (true, Style::Photoreal) => "sb:two_storey_inn",
        (false, _) => "hp:farmhouse",
        (true, _) => "hp:town_hall",
    }
}

/// The Village and Town tiles (a row under the other set pieces; `w`: a tile's width).
pub fn tiles(ui: &mut egui::Ui, s: &mut UiState, w: f32) {
    // (the town follows the style being browsed)
    match s.place.style.as_str() {
        crate::objects::PHOTOREAL => s.towns.style = Style::Photoreal,
        "handpainted" => s.towns.style = Style::Handpainted,
        _ => {}
    }
    let style = style_now(s);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(7.0, 7.0);
        for (town, label, hint) in [(false, "Village", "A village: a green with a well by the road, the inn and chapel facing it, a smithy, homes with gardens along a lane, a farm and a windmill. Click the ground to place one."), (true, "Town", "A town round a cobbled square with a statue and stalls: town hall, inn, chapel and store facing it, homes spaced along lanes, farms outside. Click the ground to place one.")] {
            let on = s.towns.selected.is_some_and(|z| z.is_town() == town) && s.ed.object_tool == ObjectTool::Place;
            let size = s.towns.selected.filter(|z| z.is_town() == town).unwrap_or(if town { TownSize::Town } else { TownSize::Village });
            let subject = crate::thumbs::Subject::Asset(icon_asset(size, style).into());
            let r = crate::thumbs::ui::tile(ui, &mut s.previews, &subject, if town { icon::CASTLE_TURRET } else { icon::HOUSE }, label, on, w);
            if r.on_hover_text(hint).clicked() {
                s.towns.selected = if on { None } else { Some(size) };
                if s.towns.selected.is_some() {
                    s.sets.selected = None;
                    s.place.selected = None;
                    s.ed.object_tool = ObjectTool::Place;
                }
            }
        }
    });
    if s.towns.selected.is_some() {
        crate::ui::tips::card(
            ui,
            "towns",
            "A whole village or town in one click",
            &[
                "Click the ground: a settlement of the size picked on the right goes down there, its streets turned to follow the land. Or hold the mouse and draw a loop to fill an exact area.",
                "Streets are real roads (edit them in the Roads tab). Buildings stand well apart, facing the square or their lane, with gardens and trees between. Shuffle re-rolls it; Undo takes it all back.",
                "Want a particular mix (an inn, a smithy, twelve houses)? Ask Prompt an area: it builds towns the same way.",
            ],
        );
    }
}

/// Set pieces for the photoreal style: only the towns (the other set pieces are hand-painted).
pub fn photoreal_section(ui: &mut egui::Ui, s: &mut UiState) {
    egui::CollapsingHeader::new(
        RichText::new(format!("{}  Set pieces", icon::SQUARES_FOUR))
            .font(FontId::new(14.0, semibold()))
            .color(Color32::WHITE),
    )
    .id_salt("set_pieces")
    .default_open(true)
    .show(ui, |ui| {
        let w = ((ui.available_width() - 7.0 * 3.0) / 4.0).floor();
        tiles(ui, s, w);
    });
    ui.add_space(4.0);
}

/// The right panel while a town waits for its click: what it is, its size, Shuffle, options.
pub fn card(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(size) = s.towns.selected else { return };
    let style = style_now(s);
    crate::thumbs::ui::card(
        ui,
        &mut s.previews,
        &crate::thumbs::Subject::Asset(icon_asset(size, style).into()),
        size.name(),
        &[size.hint().to_string()],
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!(
            "{}  Click the ground to place it",
            icon::CURSOR_CLICK
        ))
        .font(FontId::new(13.5, semibold()))
        .color(Color32::WHITE),
    );
    dim(
        ui,
        "Sized for you and turned so its main street runs the flattest way (the ring shows it). Or hold the mouse and draw a loop to fill an exact area.",
    );
    ui.add_space(6.0);
    let mut sz = size;
    if segmented(ui, &mut sz, &TownSize::ALL.map(|z| (z, z.name()))) {
        s.towns.selected = Some(sz);
    }
    ui.add_space(6.0);
    let last = s
        .towns
        .last
        .as_ref()
        .is_some_and(|l| l.ids.iter().any(|id| s.store.items.contains_key(id)));
    if ui
        .add(
            egui::Button::new(format!("{}  Shuffle", icon::SHUFFLE))
                .min_size(vec2(ui.available_width(), 28.0)),
        )
        .on_hover_text(if last {
            "Re-roll the last one in the same place, with the options below (undoable)"
        } else {
            "A new arrangement for the next one"
        })
        .clicked()
    {
        s.towns.shuffle = true;
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(RichText::new("Options").color(TEXT_DIM)).id_salt("town_options").default_open(false).show(ui, |ui| {
        toggle(ui, &mut s.towns.roads, "Streets as real roads").on_hover_text("Cobbled and dirt roads with a carved bed and bridges, editable in the Roads tab. Off: the buildings still line open lanes.");
        if sz.is_town() {
            let mut walls = s.towns.walls_for(sz);
            if toggle(ui, &mut walls, "Town wall").on_hover_text("A log palisade round it with gates where the streets leave (hand-painted). Off by default: MMO towns lie open in the land").changed() {
                s.towns.walls = Some(walls);
            }
        }
        ui.add_space(4.0);
        ui.label(RichText::new("Style").small().color(TEXT_DIM));
        let mut st = s.towns.style;
        if segmented(ui, &mut st, &Style::ALL.map(|x| (x, x.name()))) {
            s.towns.style = st;
            // (the Objects tab's Style filter follows, so the grid shows the same pack)
            s.place.style = match st {
                Style::Photoreal => crate::objects::PHOTOREAL.to_string(),
                Style::Handpainted => "handpainted".to_string(),
                Style::Auto => String::new(),
            };
        }
    });
    ui.add_space(4.0);
    dim(
        ui,
        "Every building is a normal object afterwards and the streets are roads you can edit. Undo takes the whole town back.",
    );
    separator(ui);
}
