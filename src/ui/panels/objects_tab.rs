//! Build › Objects in the Halo layout (HANDOFF › Build): an Assets / Packs / Scenes switch with the
//! Gallery, one search, set pieces first, the library (style, price / pack, category chips, asset
//! cards), Import; Place / Select & Move with the asset and its scale in the dock; the inspector for
//! what's being placed or the selected object (exact values, materials, ground, actions).

use bevy::prelude::*;
use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Id, Margin, RichText, Sense, Stroke, Ui, vec2,
};

use super::super::theme::*;
use super::super::widgets::*;
use super::super::{UiAction, UiOut, UiState};
use super::{asset_grid, in_style};
use crate::editor::ObjectTool;
use crate::market::ui::Browse;
use crate::objects::Category;

// ------------------------------------------------------------------ the panel

pub(super) fn objects_left(ui: &mut Ui, s: &mut UiState, out: &mut UiOut) {
    // Assets / Packs / Scenes · Gallery · ?
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ui.available_width() - 2.0 * (28.0 + 6.0);
        let mut b = s.market.browse;
        ui.allocate_ui(vec2(w, 26.0), |ui| {
            if segmented_icons(
                ui,
                &mut b,
                &[
                    (Browse::Assets, "LayoutGrid1", "Assets"),
                    (Browse::Packs, "Package", "Packs"),
                    (Browse::Scenes, "Stage", "Scenes"),
                ],
                false,
            ) {
                s.market.browse = b;
                if b != Browse::Scenes {
                    s.scenes.mode = crate::scenes::Mode::Off;
                }
            }
        });
        if icon_button(
            ui,
            "FullScreen",
            "Gallery: every asset of every style pack in a big window",
        )
        .clicked()
        {
            s.gallery.open = true;
        }
        crate::ui::tips::help_button(ui, "object_handles");
    });
    crate::ui::tips::card(
        ui,
        "object_handles",
        "Move, turn and resize by hand",
        &[
            "Click any placed object to select it (in Place too; clicking open ground still places).",
            "Drag it to move it. Drag the round knob on its ring to turn it (Shift: 15° steps).",
            "Drag a corner to resize it, always in proportion (Shift: ×0.25 steps). Drag the arrow on top, or Alt-drag it, to lift or lower it.",
            "Buildings level the ground under them and keep grass off their floors. ⌘D duplicates, Delete removes, Esc deselects.",
        ],
    );
    // picking an asset, a set piece or a town leaves the Scenes tab's capture / placing
    if s.scenes.mode != crate::scenes::Mode::Off
        && (s.place.selected.is_some() || s.sets.selected.is_some() || s.towns.selected.is_some())
    {
        s.scenes.mode = crate::scenes::Mode::Off;
    }
    match s.market.browse {
        Browse::Packs => {
            hint_small(
                ui,
                "Asset packs by style: what you own, what's free, prices. Skyfold paid packs give 10 assets free to try.",
            );
            crate::market::ui::packs_browser(ui, s);
            if primary_icon_button(ui, "Store1", "Open the Marketplace")
                .on_hover_text(
                    "Every pack and creator listing with pictures, what you own, what's free, prices",
                )
                .clicked()
            {
                s.market.open = true;
            }
            return;
        }
        Browse::Scenes => {
            crate::scenes::ui::browser(ui, s);
            ready_scenes(ui, s);
            return;
        }
        Browse::Assets => {}
    }
    let n = s.lib.as_ref().map_or(0, |l| {
        l.entries
            .iter()
            .filter(|e| {
                !e.whole_scene
                    && matches!(
                        e.category,
                        Category::Structures | Category::Props | Category::Imported
                    )
            })
            .count()
    });
    search_field(ui, &mut s.ed.asset_search, &format!("Search {n} objects…"));
    let searching = !s.ed.asset_search.trim().is_empty();
    if !searching {
        if s.place.style != crate::objects::PHOTOREAL {
            section_fold(
                ui,
                "obj_sets",
                "Set pieces",
                "draw a loop on the ground",
                |ui| set_pieces(ui, s),
            );
        } else {
            crate::towns::ui::photoreal_section(ui, s);
        }
    }
    section_fold(ui, "obj_library", "Library", "", |ui| library(ui, s));
    if wide_icon_button(ui, "Import", "Import 3D Model…")
        .on_hover_text("glTF / GLB models (bridges, buildings, props, trees…)")
        .clicked()
    {
        out.actions.write(UiAction::ImportModel);
    }
    hint_small(
        ui,
        "Imported models are copied into the project and get footstep sounds automatically from their names.",
    );
}

/// The library: style, price and pack, the category chips and the asset cards.
fn library(ui: &mut Ui, s: &mut UiState) {
    style_row(ui, s);
    price_chips(ui, s);
    crate::market::ui::pack_chips(ui, s);
    let search = s.ed.asset_search.clone();
    let all_id = Id::new("objects_all_cats");
    let mut all = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(all_id))
        .unwrap_or(false);
    if search.trim().is_empty() {
        category_chips(ui, s, &mut all);
        scenes_pointer(ui, s);
    } else {
        hint_small(ui, "Searching structures, props and imported models.");
    }
    ui.ctx().data_mut(|d| d.insert_temp(all_id, all));
    let every = [Category::Structures, Category::Props, Category::Imported];
    let category = [s.place.category];
    let group = s.place.group.clone();
    let style = s.place.style.clone();
    let (cats, group): (&[Category], &str) = if !search.trim().is_empty() || all {
        (&every, "")
    } else {
        (&category, &group)
    };
    let sel = if s.sets.selected.is_some() || s.towns.selected.is_some() {
        None
    } else {
        s.place.selected.clone()
    };
    if let Some(id) = asset_grid(ui, s, cats, group, &style, &search, false, &|id| {
        sel.as_deref() == Some(id)
    }) {
        s.place.selected = Some(id);
        s.sets.selected = None;
        s.towns.selected = None;
        s.ed.object_tool = ObjectTool::Place;
    }
}

/// This world's art style (Style tab › World art), or the mix of styles.
fn style_row(ui: &mut Ui, s: &mut UiState) {
    let Some(lib) = &s.lib else { return };
    let world = s.meta.art_style;
    if !s.place.mix_styles {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 24.0), Sense::hover());
            paint_icon(ui, r.center(), "PaintBrush", 13.0, TEXT_DIM);
            ui.label(RichText::new(format!("{} assets", world.name())).size(12.5).color(TEXT_DIM))
                .on_hover_text("This world's art style (Style tab › World art). Only its assets are listed; imported models and the procedural trees fit every style.");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if small_button(ui, "Show other styles").on_hover_text("Mix in assets of the other styles (for this session)").clicked() {
                    s.place.mix_styles = true;
                    s.place.style.clear();
                    s.place.group.clear();
                }
            });
        });
        return;
    }
    let mut packs: Vec<String> = vec![];
    for e in &lib.entries {
        if !packs.contains(&e.pack) {
            packs.push(e.pack.clone());
        }
    }
    if packs.len() < 2 {
        return;
    }
    packs.sort_by_key(|p| (p != crate::objects::PHOTOREAL, p.clone()));
    let labels: Vec<String> = std::iter::once("All styles".to_string())
        .chain(packs.iter().map(|p| crate::objects::pack_label(p)))
        .collect();
    let options: Vec<(usize, &str)> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| (i, l.as_str()))
        .collect();
    let mut cur = packs
        .iter()
        .position(|p| *p == s.place.style)
        .map(|i| i + 1)
        .unwrap_or(0);
    if segmented(ui, &mut cur, &options) {
        s.place.style = if cur == 0 {
            String::new()
        } else {
            packs[cur - 1].clone()
        };
        s.place.group.clear();
    }
    ui.horizontal(|ui| {
        hint_small(ui, "Asset packs in different game styles: photoscanned realism, or chunky hand-painted fantasy.");
    });
    if small_button(ui, &format!("Only {}", world.name().to_lowercase()))
        .on_hover_text("Back to this world's art style only")
        .clicked()
    {
        s.place.mix_styles = false;
        s.place.style = world.id().to_string();
        s.place.group.clear();
    }
}

/// All / Free / Paid.
fn price_chips(ui: &mut Ui, s: &mut UiState) {
    use crate::market::ui::PriceFilter;
    let mut f = s.market.price;
    let r = chips(
        ui,
        &mut f,
        &[
            (PriceFilter::All, "All", None),
            (PriceFilter::Free, "Free", None),
            (PriceFilter::Paid, "Paid", None),
        ],
    );
    if r {
        s.market.price = f;
    }
}

/// Category chips (All · Structures · Props · Imported) with counts; the open category's groups under them.
fn category_chips(ui: &mut Ui, s: &mut UiState, all: &mut bool) {
    let Some(lib) = &s.lib else { return };
    let market = crate::market::snapshot();
    let pack = crate::market::ui::filter(s, &market);
    let style = s.place.style.clone();
    let keep = |e: &crate::objects::LibraryEntry| {
        !e.whole_scene
            && in_style(e, &style)
            && pack.is_none_or(|p| {
                market.pack_index_of(&e.id) == Some(p) || e.category == Category::Imported
            })
    };
    let count = |c: Category| lib.of_category(c).filter(|(_, e)| keep(e)).count();
    let cats = [Category::Structures, Category::Props, Category::Imported];
    let total: usize = cats.iter().map(|c| count(*c)).sum();
    // (an empty "All" key: every category)
    let mut key: Option<Category> = if *all { None } else { Some(s.place.category) };
    let opts: Vec<(Option<Category>, &str, Option<usize>)> =
        std::iter::once((None, "All", Some(total)))
            .chain(cats.iter().map(|c| (Some(*c), c.name(), Some(count(*c)))))
            .collect();
    if chips(ui, &mut key, &opts) {
        match key {
            None => *all = true,
            Some(c) => {
                *all = false;
                s.place.category = c;
                s.place.group.clear();
            }
        }
    }
    if *all {
        return;
    }
    // the groups inside the open category
    let mut groups: Vec<(String, usize)> = vec![];
    let mut in_cat = 0;
    for (_, e) in lib.of_category(s.place.category).filter(|(_, e)| keep(e)) {
        in_cat += 1;
        let g = if e.group.is_empty() {
            "-"
        } else {
            e.group.as_str()
        };
        match groups.iter_mut().find(|x| x.0 == g) {
            Some(x) => x.1 += 1,
            None => groups.push((g.to_string(), 1)),
        }
    }
    if !groups.iter().any(|g| g.0 == s.place.group) {
        s.place.group.clear();
    }
    if groups.iter().filter(|g| g.0 != "-").count() < 2 {
        s.place.group.clear();
        return;
    }
    groups.sort_by(|a, b| (a.0 == "-").cmp(&(b.0 == "-")).then(a.0.cmp(&b.0)));
    let labels: Vec<String> = groups
        .iter()
        .map(|(g, _)| {
            if g == "-" {
                "Other".to_string()
            } else {
                let mut c = g.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            }
        })
        .collect();
    let all_label = format!("All {}", s.place.category.name().to_lowercase());
    let cur = s.place.group.clone();
    let opts: Vec<(&str, &str, Option<usize>)> =
        std::iter::once(("", all_label.as_str(), Some(in_cat)))
            .chain(
                groups
                    .iter()
                    .zip(&labels)
                    .map(|((g, n), l)| (g.as_str(), l.as_str(), Some(*n))),
            )
            .collect();
    let mut gr: &str = cur.as_str();
    if chips(ui, &mut gr, &opts) {
        s.place.group = gr.to_string();
    }
}

/// Set pieces and whole villages / towns: draw a loop to fill an area.
fn set_pieces(ui: &mut Ui, s: &mut UiState) {
    use crate::setpieces::SetPieceKind;
    crate::ui::tips::card(
        ui,
        "set_pieces",
        "Fill an area with a graveyard or market",
        &[
            "Pick a set piece, then hold the mouse and draw a loop round an area on the ground, like drawing a lake.",
            "The set fills it, sized to fit: a fenced graveyard, a giant's boneyard, a village square or a market, its front facing you. Shuffle re-rolls it in place.",
            "Every piece is a normal object: click one to move, turn or resize it, or delete it. Undo takes the whole set back.",
        ],
    );
    let w = ((ui.available_width() - 5.0 * 2.0) / 3.0).floor();
    let market = crate::market::snapshot();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
        for k in SetPieceKind::ALL {
            let subject = crate::thumbs::Subject::Asset(k.icon_asset().into());
            let on = s.sets.selected == Some(k) && s.ed.object_tool == ObjectTool::Place;
            let r = crate::thumbs::ui::tile(
                ui,
                &mut s.previews,
                &subject,
                "LayoutGrid1",
                k.name(),
                on,
                w,
            );
            // (a set needs its whole pack, not just the free samples)
            if let Some(p) = market
                .pack_of(k.icon_asset())
                .filter(|p| market.state(p) == crate::market::PackState::Locked)
            {
                crate::market::ui::badge(ui, r.rect, crate::market::Access::Locked(p));
                if crate::market::ui::locked_hover(r, k.name(), p).clicked() {
                    s.market.card = Some(p.id.clone());
                }
                continue;
            }
            if r.on_hover_text(k.hint()).clicked() {
                s.sets.selected = if on { None } else { Some(k) };
                if s.sets.selected.is_some() {
                    s.place.selected = None;
                    s.towns.selected = None;
                    s.ed.object_tool = ObjectTool::Place;
                }
            }
        }
        // whole villages and towns (crate::towns)
        crate::towns::ui::tiles(ui, s, w);
    });
}

fn scenes_pointer(ui: &mut Ui, s: &mut UiState) {
    let style = s.place.style.clone();
    let n = s.lib.as_ref().map_or(0, |l| {
        l.entries
            .iter()
            .filter(|e| e.whole_scene && in_style(e, &style))
            .count()
    });
    if n == 0 {
        return;
    }
    let label = format!(
        "{n} complete scene{} — in Scenes",
        if n == 1 { "" } else { "s" }
    );
    if small_icon_button(ui, "Stage", &label)
        .on_hover_text(
            "Whole scenes (every piece in place) are in Objects › Scenes › Ready-made scenes",
        )
        .clicked()
    {
        s.market.browse = Browse::Scenes;
    }
}

fn ready_scenes(ui: &mut Ui, s: &mut UiState) {
    let n = s.lib.as_ref().map_or(0, |l| {
        l.entries
            .iter()
            .filter(|e| e.whole_scene && in_style(e, &s.place.style))
            .count()
    });
    let others = s
        .lib
        .as_ref()
        .map_or(0, |l| l.entries.iter().filter(|e| e.whole_scene).count())
        - n;
    if n == 0 {
        // (all of them are in another style than the world's)
        if others > 0 && !s.place.mix_styles {
            hint_small(
                ui,
                &format!(
                    "{others} ready-made scenes are in other art styles than this world's ({}).",
                    s.meta.art_style.name().to_lowercase()
                ),
            );
            if small_button(ui, "Show other styles").clicked() {
                s.place.mix_styles = true;
                s.place.style.clear();
            }
        }
        return;
    }
    section_fold(
        ui,
        "obj_ready_scenes",
        "Ready-made scenes",
        &n.to_string(),
        |ui| {
            hint_small(
                ui,
                "Complete scenes from the asset packs, every piece in place. Click one, then the ground, to place it whole. Their pieces are also single objects under Assets.",
            );
            search_field(ui, &mut s.ed.asset_search, "Search scenes…");
            let sel = s.place.selected.clone();
            let search = s.ed.asset_search.clone();
            let picking = s.sets.selected.is_some()
                || s.towns.selected.is_some()
                || s.scenes.mode != crate::scenes::Mode::Off;
            let style = s.place.style.clone();
            if let Some(id) = asset_grid(
                ui,
                s,
                &[Category::Structures, Category::Props],
                "",
                &style,
                &search,
                true,
                &|id| !picking && sel.as_deref() == Some(id),
            ) {
                s.place.selected = Some(id);
                s.sets.selected = None;
                s.towns.selected = None;
                s.scenes.mode = crate::scenes::Mode::Off;
                s.ed.object_tool = ObjectTool::Place;
            }
        },
    );
}

// ------------------------------------------------------------------ the inspector

/// The inspector's title: what's selected or being placed (None when there's nothing).
pub(super) fn objects_inspector(s: &UiState) -> Option<(String, bool)> {
    let place = s.ed.object_tool == ObjectTool::Place;
    let t = if let Some(id) = s.store.selected {
        if s.store.items.get(&id).is_some_and(|p| p.group != 0) {
            "Selected set"
        } else {
            "Selected object"
        }
    } else if s.nature_brush.selected.is_some() {
        "Procedural nature"
    } else if s.scenes.placing().is_some() || s.scenes.capturing() {
        "Scene"
    } else if place && s.towns.selected.is_some() {
        "Town"
    } else if place && s.sets.selected.is_some() {
        "Set piece"
    } else if place {
        "Placement"
    } else {
        return None;
    };
    Some((t.to_string(), true))
}

pub(super) fn objects_right(ui: &mut Ui, s: &mut UiState, out: &mut UiOut) {
    let _ = out;
    if let Some(id) = s.store.selected {
        if let Some(p) = s.store.items.get(&id).cloned() {
            if p.group != 0 {
                selected_set(ui, s, p.group);
            } else {
                selected_object(ui, s, id, p);
            }
        }
        return;
    }
    if let Some(i) = s.nature_brush.selected {
        procedural(ui, s, i);
        return;
    }
    // a saved scene to place: its card, export and marketplace (crate::scenes)
    if crate::scenes::ui::right_card(ui, s) {
        return;
    }
    if s.ed.object_tool != ObjectTool::Place {
        return;
    }
    // a town to place: its size, Shuffle and options (crate::towns)
    if s.towns.selected.is_some() {
        crate::towns::ui::card(ui, s);
        return;
    }
    // a set piece to place: its size and Shuffle
    if let Some(k) = s.sets.selected {
        crate::thumbs::ui::card(
            ui,
            &mut s.previews,
            &crate::thumbs::Subject::Asset(k.icon_asset().into()),
            k.name(),
            &[k.hint().to_string()],
        );
        ui.label(
            RichText::new("Draw its area on the ground")
                .font(FontId::new(13.5, semibold()))
                .color(TEXT),
        );
        hint_small(
            ui,
            "Hold the mouse and draw a loop round the area, like drawing a lake. The set fills it: a bigger area gets more graves, stalls or bones. Its front faces you.",
        );
        let last = s
            .sets
            .last
            .as_ref()
            .is_some_and(|l| l.ids.iter().any(|id| s.store.items.contains_key(id)));
        if wide_icon_button(ui, "Shuffle", "Shuffle")
            .on_hover_text(if last {
                "Re-roll the last one in the same spot (undoable)"
            } else {
                "A new arrangement for the next one"
            })
            .clicked()
        {
            s.sets.shuffle = true;
        }
        hint_small(
            ui,
            "Every piece is a normal object afterwards: click one to move, turn or resize it.",
        );
        return;
    }
    // what's being placed, turning live in the studio (crate::thumbs)
    if let Some(id) = s.place.selected.clone()
        && let Some((name, lines)) = s
            .lib
            .as_ref()
            .and_then(|l| l.get(&id))
            .map(|e| (e.name.clone(), placing_lines(e)))
    {
        crate::thumbs::ui::card(
            ui,
            &mut s.previews,
            &crate::thumbs::Subject::Asset(id),
            &name,
            &lines,
        );
    } else {
        hint_small(
            ui,
            "Pick an asset in the library, then click the ground to place it.",
        );
    }
    slider_row(ui, "Scale", &mut s.place.scale, 0.1..=5.0, |v| {
        format!("{v:.2}×")
    });
    toggle(ui, &mut s.place.random_yaw, "Random rotation");
    toggle(ui, &mut s.place.align, "Align to terrain slope");
    hint_small(
        ui,
        "Buildings fit the ground wherever they go: levelled with natural banks and a trodden apron.",
    );
    section_fold(
        ui,
        "obj_materials",
        "Building materials",
        "the whole world",
        |ui| building_materials(ui, s),
    );
}

fn placing_lines(e: &crate::objects::LibraryEntry) -> Vec<String> {
    let mut first = e.category.name().trim_end_matches('s').to_string();
    if e.category != Category::Imported {
        first += &format!(
            " · about {:.1} m across, {:.1} m tall",
            e.radius * 2.0 / 1.1,
            e.height
        );
    }
    vec![
        first,
        format!(
            "Footsteps: {}{}",
            e.surface.name(),
            if e.walkable { " · walkable" } else { "" }
        ),
    ]
}

/// The inspector's hero: a thumbnail (or icon), a title and up to two muted lines.
fn hero(ui: &mut Ui, image: Option<egui::TextureId>, icon: &str, title: &str, lines: &[String]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(vec2(46.0, 46.0), Sense::hover());
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
            ui.label(
                RichText::new(title)
                    .font(FontId::new(14.5, semibold()))
                    .color(TEXT),
            );
            for l in lines {
                ui.label(RichText::new(l).size(12.0).color(TEXT_MUTE));
            }
        });
    });
}

fn procedural(ui: &mut Ui, s: &mut UiState, i: crate::nature::Instance) {
    let id = crate::objects::builtin_id(i.kind);
    let thumb = s.previews.thumb(&crate::thumbs::Subject::Asset(id));
    hero(
        ui,
        thumb,
        "Tree",
        i.kind.name(),
        &[format!(
            "{:.0}, {:.0} · scale {:.2}",
            i.pos.x, i.pos.z, i.scale
        )],
    );
    hint_small(
        ui,
        "Grown by the rules and painted density. Pin it to move, turn or keep it by hand.",
    );
    ui.horizontal(|ui| {
        let w = ((ui.available_width() - 6.0) * 0.5).floor();
        ui.spacing_mut().item_spacing.x = 6.0;
        if icon_text_button(ui, "Thumbtack", "Pin as object", Some(w)).clicked() {
            let ids = crate::nature::brush::pin(&mut s.store, &mut s.history, &[i]);
            s.store.selected = ids.first().copied();
            s.nature_brush.selected = None;
            s.meta.dirty = true;
        }
        if danger_button(ui, "TrashCan", "Remove", w).clicked() {
            crate::nature::brush::remove(&mut s.nature, &mut s.history, &[i.id]);
            s.nature_brush.selected = None;
            s.meta.dirty = true;
        }
    });
}

/// A soft button that turns red on hover (Delete).
fn danger_button(ui: &mut Ui, icon: &str, text: &str, w: f32) -> egui::Response {
    let r = icon_text_button(ui, icon, text, Some(w));
    if r.hovered() {
        ui.painter().rect_stroke(
            r.rect,
            CornerRadius::same(R_BUTTON),
            Stroke::new(1.0, DANGER.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
    }
    r
}

fn locked_card(ui: &mut Ui, s: &mut UiState, id: u64, what: &str) -> bool {
    if !s.store.is_locked(id) {
        return false;
    }
    let mut unlock = false;
    egui::Frame::new().fill(Color32::from_rgba_unmultiplied(242, 193, 78, 18)).stroke(Stroke::new(1.0, WARN.gamma_multiply(0.5))).corner_radius(CornerRadius::same(R_TILE)).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::hover());
            paint_icon(ui, r.center(), "Lock", 13.0, WARN);
            ui.label(RichText::new("Locked").font(FontId::new(13.0, semibold())).color(WARN));
        });
        hint_small(ui, &format!("This {what} can't be moved, turned, resized or deleted until you unlock it. You can still copy it (⌘D)."));
        unlock = icon_text_button(ui, "Unlocked", "Unlock", None).clicked();
    });
    if unlock {
        crate::placement::lock::set_lock(&mut s.store, &mut s.history, id, false);
        s.meta.dirty = true;
    }
    true
}

/// An exact-value field: a dark well with a coloured axis letter (or icon) and a drag value.
fn xyz(
    ui: &mut Ui,
    label: &str,
    colour: Color32,
    w: f32,
    value: &mut f32,
    speed: f64,
    suffix: &str,
    range: Option<std::ops::RangeInclusive<f32>>,
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin {
            left: 8,
            right: 6,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(w - 14.0);
            ui.horizontal(|ui| {
                ui.set_height(27.0);
                if super::super::icons::has(label) {
                    let (r, _) = ui.allocate_exact_size(vec2(13.0, 27.0), Sense::hover());
                    paint_icon(ui, r.center(), label, 12.0, TEXT_MUTE);
                } else {
                    ui.label(
                        RichText::new(label)
                            .font(FontId::new(11.0, mono_semibold()))
                            .color(colour),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.scope(|ui| {
                        let v = &mut ui.visuals_mut().widgets;
                        v.inactive.weak_bg_fill = Color32::TRANSPARENT;
                        v.hovered.weak_bg_fill = Color32::TRANSPARENT;
                        v.active.weak_bg_fill = Color32::TRANSPARENT;
                        let mut dv = egui::DragValue::new(value)
                            .speed(speed)
                            .suffix(suffix.to_string())
                            .custom_formatter(|v, _| format!("{v:.1}"));
                        if let Some(r) = range.clone() {
                            dv = dv.range(r);
                        }
                        changed |= ui.add(dv).changed();
                    });
                });
            });
        });
    changed
}

fn selected_object(ui: &mut Ui, s: &mut UiState, id: u64, p: crate::objects::Placed) {
    let entry = s.lib.as_ref().and_then(|l| l.get(&p.asset)).map(|e| {
        (
            e.name.clone(),
            crate::objects::pack_label(&e.pack),
            e.category.name(),
            e.surface.name(),
            e.walkable,
        )
    });
    let name = entry
        .as_ref()
        .map(|e| e.0.clone())
        .unwrap_or(p.asset.clone());
    let mut lines = vec![];
    if let Some((_, pack, cat, surf, walk)) = &entry {
        lines.push(format!("{pack} · {cat}"));
        lines.push(format!(
            "Footsteps: {surf}{}",
            if *walk { " · walkable" } else { "" }
        ));
    }
    let thumb = s
        .previews
        .thumb(&crate::thumbs::Subject::Asset(p.asset.clone()));
    hero(ui, thumb, "3dBoxTop", &name, &lines);
    if locked_card(ui, s, id, "object") {
        return;
    }
    hint_small(
        ui,
        "Drag it to move · the knob turns it · corners resize · the arrow lifts.",
    );
    // exact values
    let mut np = p.clone();
    let mut yaw = p.yaw().to_degrees();
    let mut changed = false;
    section_fold(ui, "obj_exact", "Exact values", "", |ui| {
        let w3 = ((ui.available_width() - 12.0) / 3.0).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            changed |= xyz(
                ui,
                "X",
                Color32::from_rgb(255, 138, 128),
                w3,
                &mut np.pos[0],
                0.5,
                " m",
                None,
            );
            changed |= xyz(
                ui,
                "Y",
                Color32::from_rgb(155, 229, 154),
                w3,
                &mut np.pos[1],
                0.1,
                " m",
                None,
            );
            changed |= xyz(
                ui,
                "Z",
                Color32::from_rgb(156, 200, 255),
                w3,
                &mut np.pos[2],
                0.5,
                " m",
                None,
            );
        });
        let w2 = ((ui.available_width() - 6.0) / 2.0).floor();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if xyz(ui, "Rotate360Left", TEXT_MUTE, w2, &mut yaw, 1.0, "°", None) {
                np.rot = Quat::from_rotation_y(yaw.to_radians()).to_array();
                changed = true;
            }
            changed |= xyz(
                ui,
                "Expand",
                TEXT_MUTE,
                w2,
                &mut np.scale,
                0.01,
                "×",
                Some(0.05..=20.0),
            );
        });
        // proportions locked unless asked: per-axis stretch only behind the lock
        let stretched = np.stretch.is_some();
        let mut unlocked = s.ed.scale_unlocked || stretched;
        let mut locked = !unlocked;
        if toggle(ui, &mut locked, "Proportions locked").on_hover_text("Locked: every resize keeps the object's shape. Unlock to stretch it along one axis.").changed() {
            unlocked = !locked;
            s.ed.scale_unlocked = unlocked;
            if !unlocked && stretched {
                np.stretch = None;
                changed = true;
            }
        }
        if unlocked {
            let mut k = np.scale3();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (axis, v) in ["W", "H", "D"].iter().zip([&mut k.x, &mut k.y, &mut k.z]) {
                    changed |= xyz(ui, axis, TEXT_MUTE, w3, v, 0.01, "×", Some(0.05..=20.0));
                }
            });
            if k != np.scale3() {
                np.set_scale3(k);
            }
            if stretched && small_button(ui, "Back to its own proportions").clicked() {
                np.stretch = None;
                changed = true;
            }
        }
    });
    if changed {
        let lifted = np.pos[1] != p.pos[1]
            && (np.pos[0], np.pos[2], np.rot, np.scale) == (p.pos[0], p.pos[2], p.rot, p.scale);
        s.history.begin("Edit object");
        s.history.touch_object(&s.store, id);
        s.store.update(np);
        s.store.selected = Some(id);
        // a fitted building's levelled ground follows its new height
        if lifted && let Some(lib) = s.lib.as_deref() {
            let mut w = crate::placement::FitWorld {
                terrain: &mut s.terrain,
                history: &mut s.history,
                store: &mut s.store,
                lib,
                water: &s.water,
                found: &mut s.fit.found,
                bounds: &s.fit.bounds,
            };
            crate::placement::fit::follow_lift(&mut w, id);
        }
        s.history.commit(&s.store);
    }
    // a crystal's own colour and light
    if s
        .lib
        .as_deref()
        .and_then(|l| l.get(&p.asset))
        .is_some_and(crate::objects::glow::glows)
    {
        section_fold(ui, "obj_glow", "Glow", "this crystal", |ui| {
            glow_controls(ui, s, id, &p)
        });
    }
    section_fold(
        ui,
        "obj_materials",
        "Building materials",
        "the whole world",
        |ui| building_materials(ui, s),
    );
    // levelling the ground under it (buildings and other structures do by default)
    if let Some(lib) = s.lib.as_deref() {
        let mut on = crate::placement::fit::wants(&s.fit.found, lib, &s.fit.bounds, &p);
        let resp = toggle(ui, &mut on, "Level the ground under it").on_hover_text(
            "Levels the ground to its floor with natural banks into the slope, paints a packed-earth apron and keeps grass and plants off its footprint. Follows it when it moves; the old spot gets its ground back.",
        );
        if resp.changed() {
            s.history.begin(if on {
                "Level the ground"
            } else {
                "Release the ground"
            });
            let mut w = crate::placement::FitWorld {
                terrain: &mut s.terrain,
                history: &mut s.history,
                store: &mut s.store,
                lib,
                water: &s.water,
                found: &mut s.fit.found,
                bounds: &s.fit.bounds,
            };
            crate::placement::fit::fit_objects(&mut w, &[id], &[(id, Some(on))]);
            s.history.commit(&s.store);
            s.meta.dirty = true;
        }
        if let Some(pad) = s.fit.found.pads.get(&id) {
            let (cut, fill) = crate::placement::fit::cut_fill(&s.fit.found, &s.terrain, pad);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(15.0, 18.0), Sense::hover());
                paint_icon(ui, r.center(), "CheckCircle2", 14.0, OK);
                ui.label(
                    RichText::new(format!(
                        "On levelled ground · cut {cut:.1} m · fill {fill:.1} m"
                    ))
                    .size(12.5)
                    .color(OK),
                );
            });
        }
    }
    // the actions, 2 × 2
    let (mut duplicate, mut snap, mut lock, mut delete) = (false, false, false, false);
    let w = ((ui.available_width() - 6.0) * 0.5).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        duplicate = icon_text_button(ui, "SquareBehindSquare1", "Duplicate", Some(w))
            .on_hover_text("A copy beside it (⌘D)")
            .clicked();
        snap = icon_text_button(ui, "ArrowDown", "Snap to ground", Some(w)).clicked();
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        lock = icon_text_button(ui, "Lock", "Lock", Some(w))
            .on_hover_text(
                "Keep it from being moved or deleted until you unlock it (also: right-click it)",
            )
            .clicked();
        delete = danger_button(ui, "TrashCan", "Delete", w)
            .on_hover_text("Delete it (⌫)")
            .clicked();
    });
    if lock {
        crate::placement::lock::set_lock(&mut s.store, &mut s.history, id, true);
        s.meta.dirty = true;
        return;
    }
    if duplicate
        && let Some(copy) = crate::objects::duplicate(
            &mut s.store,
            s.lib.as_deref(),
            &s.terrain,
            &s.water,
            Some(&s.cave_world),
            id,
        )
    {
        s.history.begin("Duplicate object");
        s.history.touch_object(&s.store, copy.id);
        s.store.insert(copy.clone());
        s.store.selected = Some(copy.id);
        s.history.commit(&s.store);
        return;
    }
    if snap {
        let mut q = p.clone();
        q.pos[1] = s.terrain.height_at(q.pos[0], q.pos[2]) - 0.05;
        s.history.begin("Snap to ground");
        s.history.touch_object(&s.store, id);
        s.store.update(q);
        s.store.selected = Some(id);
        s.history.commit(&s.store);
        return;
    }
    if delete {
        s.history.begin("Delete object");
        s.history.touch_object(&s.store, id);
        s.store.remove(id);
        s.history.commit(&s.store);
    }
}

/// A crystal's colour (swatches, a picker) and light, each change one undo step.
fn glow_controls(ui: &mut Ui, s: &mut UiState, id: u64, p: &crate::objects::Placed) {
    use crate::dungeons::ui::kit::{color_dot, color_row};
    use crate::objects::glow::{COLORS, MAX_LIGHT};
    let cur = p.glow;
    let mut g = cur.unwrap_or_default();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (name, c) in COLORS {
            let on = cur.is_some() && g.color_name() == Some(*name);
            let dot = Color32::from_rgb(
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
            );
            let mut tip = name.to_string();
            tip[..1].make_ascii_uppercase();
            if color_dot(ui, dot, on, &tip).clicked() && !on {
                g.color = *c;
                changed = true;
            }
        }
    });
    // (the picker works in linear colour)
    let lin = Color::srgb(g.color[0], g.color[1], g.color[2]).to_linear();
    let mut rgb = [lin.red, lin.green, lin.blue];
    if color_row(ui, "Colour", &mut rgb).changed() {
        let c = Color::linear_rgb(rgb[0], rgb[1], rgb[2]).to_srgba();
        g.color = [c.red, c.green, c.blue];
        changed = true;
    }
    changed |= slider_row(ui, "Light", &mut g.light, 0.0..=MAX_LIGHT, |v| {
        if v < 0.01 {
            "dark".into()
        } else {
            format!("{v:.1}×")
        }
    })
    .on_hover_text(
        "How brightly it glows and lights what's round it: 0 dark, 1 a soft glow, 10 a beacon",
    )
    .changed();
    if cur.is_none() {
        hint_small(
            ui,
            "As made: a violet glow with no light of its own. Pick a colour or move Light to give it its own.",
        );
    } else if small_button(ui, "Back to its made look").clicked() {
        set_glow(s, id, None);
        return;
    }
    if changed {
        set_glow(s, id, Some(g.sanitized()));
    }
}

fn set_glow(s: &mut UiState, id: u64, g: Option<crate::objects::glow::Glow>) {
    let Some(mut q) = s.store.items.get(&id).cloned() else {
        return;
    };
    if q.glow == g {
        return;
    }
    q.glow = g;
    s.history.begin("Crystal glow");
    s.history.touch_object(&s.store, id);
    s.store.update(q);
    s.store.selected = Some(id);
    s.history.commit(&s.store);
    s.meta.dirty = true;
}

fn selected_set(ui: &mut Ui, s: &mut UiState, group: u64) {
    let members = s.store.group_members(group);
    // what it's made of: the most common pieces
    let mut counts: Vec<(String, usize)> = vec![];
    for id in &members {
        let Some(p) = s.store.items.get(id) else {
            continue;
        };
        let name = s
            .lib
            .as_ref()
            .and_then(|l| l.get(&p.asset))
            .map(|e| e.name.clone())
            .unwrap_or_else(|| p.asset.clone());
        match counts.iter_mut().find(|c| c.0 == name) {
            Some(c) => c.1 += 1,
            None => counts.push((name, 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let list: Vec<String> = counts
        .iter()
        .take(5)
        .map(|(n, c)| format!("{c}× {n}"))
        .collect();
    hero(
        ui,
        None,
        "LayoutGrid1",
        &format!("{} pieces, moved as one", members.len()),
        &[format!(
            "{}{}",
            list.join(" · "),
            if counts.len() > 5 { " · …" } else { "" }
        )],
    );
    if locked_card(ui, s, members[0], "set") {
        return;
    }
    hint_small(
        ui,
        "Drag it to move the whole set · the knob turns it · corners resize it · Z / X turn · ⌘D copies · Delete removes it.",
    );
    let (mut ungroup, mut delete, mut lock) = (false, false, false);
    let w = ((ui.available_width() - 6.0) * 0.5).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        lock = icon_text_button(ui, "Lock", "Lock", Some(w)).on_hover_text("Keep the set from being moved or deleted until you unlock it (also: right-click it)").clicked();
        ungroup = icon_text_button(ui, "LayoutGrid1", "Ungroup", Some(w)).on_hover_text("Free the pieces so each can be moved, turned or deleted on its own").clicked();
    });
    delete = danger_button(ui, "TrashCan", "Delete set", ui.available_width()).clicked() || delete;
    if lock {
        crate::placement::lock::set_lock(&mut s.store, &mut s.history, members[0], true);
        s.meta.dirty = true;
        return;
    }
    if ungroup {
        s.history.begin("Ungroup set");
        for id in &members {
            if let Some(mut p) = s.store.items.get(id).cloned() {
                s.history.touch_object(&s.store, *id);
                p.group = 0;
                s.store.items.insert(*id, p);
            }
        }
        s.store.revision += 1;
        s.history.commit(&s.store);
    } else if delete {
        s.history.begin("Delete set");
        for id in &members {
            s.history.touch_object(&s.store, *id);
            s.store.remove(*id);
        }
        s.history.commit(&s.store);
    }
}

/// What posts, crates, huts, ruins and bridges are made of (world-wide surface slots).
fn building_materials(ui: &mut Ui, s: &mut UiState) {
    let Some(lib) = s.surface_lib.as_deref() else {
        return;
    };
    if lib.entries.is_empty() {
        return;
    }
    let swatch = |ui: &mut Ui, c: Option<[f32; 3]>| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        let c = c.unwrap_or([0.5, 0.5, 0.5]);
        ui.painter().rect_filled(
            r,
            CornerRadius::same(5),
            Color32::from_rgb(
                (c[0] * 255.0) as u8,
                (c[1] * 255.0) as u8,
                (c[2] * 255.0) as u8,
            ),
        );
    };
    hint_small(
        ui,
        "What posts, crates, huts, ruins and bridges are made of: photoscanned CC0 wood, metal, stone, fabric and more.",
    );
    for slot in crate::surfaces::Slot::ALL {
        let cur = s.surface_choices.get(slot).to_string();
        let entry = lib.entries.iter().find(|e| e.id == cur);
        ui.horizontal(|ui| {
            swatch(ui, entry.and_then(|e| e.average_color_srgb));
            // (left-aligned in a fixed column, so the menus line up)
            ui.allocate_ui_with_layout(
                vec2(96.0, 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(96.0);
                    ui.add(egui::Label::new(RichText::new(slot.name()).color(TEXT_DIM)).truncate());
                },
            );
            let label = entry
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "Default".into());
            egui::ComboBox::from_id_salt(("surface_slot", slot.index()))
                .width(ui.available_width())
                .selected_text(label)
                .height(360.0)
                .dropdown_ui(ui, |ui| {
                    if menu_check(ui, cur.is_empty(), "Default").clicked() {
                        s.surface_choices.set(slot, "");
                    }
                    for e in lib
                        .entries
                        .iter()
                        .filter(|e| e.tiling && slot.categories().contains(&e.category.as_str()))
                    {
                        ui.horizontal(|ui| {
                            swatch(ui, e.average_color_srgb);
                            // (a content pack's material downloads when picked)
                            let name = if crate::surfaces::available(&e.id) {
                                e.name.clone()
                            } else {
                                format!("{} · download", e.name)
                            };
                            if menu_check(ui, cur == e.id, &name).clicked() {
                                s.surface_choices.set(slot, &e.id);
                            }
                        });
                    }
                });
        });
    }
    super::content_pack_note(
        ui,
        crate::content::CITY,
        "City & sci-fi materials download the first time you pick one",
    );
}

// ------------------------------------------------------------------ the dock

pub(super) fn objects_dock(ui: &mut Ui, s: &mut UiState) -> bool {
    ui.allocate_ui(vec2(232.0, 26.0), |ui| {
        segmented_icons(
            ui,
            &mut s.ed.object_tool,
            &[
                (ObjectTool::Place, "CursorClick", "Place"),
                (ObjectTool::Select, "Cursor1", "Select & Move"),
            ],
            true,
        );
    });
    dock_sep(ui);
    if s.ed.object_tool == ObjectTool::Place
        && s.store.selected.is_none()
        && let Some(id) = s.place.selected.clone()
        && let Some(name) = s
            .lib
            .as_ref()
            .and_then(|l| l.get(&id))
            .map(|e| e.name.clone())
    {
        let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
        if let Some(t) = s.previews.thumb(&crate::thumbs::Subject::Asset(id)) {
            egui::Image::new((t, r.size()))
                .corner_radius(CornerRadius::same(6))
                .paint_at(ui, r);
        }
        ui.label(
            RichText::new(name)
                .font(FontId::new(13.0, semibold()))
                .color(TEXT),
        );
    }
    slider_mini(ui, "Scale", &mut s.place.scale, 0.1..=5.0, |v| {
        format!("{v:.2}×")
    });
    dock_sep(ui);
    if s.store.selected.is_some() {
        dock_hints(
            ui,
            &[
                ("⌘D", "Duplicate"),
                ("⌫", "Delete"),
                ("Z / X", "Turn"),
                ("Esc", "Deselect"),
            ],
        );
    } else {
        let place = s.ed.object_tool == ObjectTool::Place;
        let lmb = if s.scenes.capturing() && place {
            "Drag a loop · save what's inside as a scene"
        } else if s.scenes.placing().is_some() && place {
            "Place the scene"
        } else if s.towns.selected.is_some() && place {
            "Place it · drag a loop to fill an area"
        } else if s.sets.selected.is_some() && place {
            "Drag a loop · fill the area"
        } else if place {
            "Place · drag to turn"
        } else {
            "Select"
        };
        let mut hints = vec![("Click", lmb)];
        if s.scenes.placing().is_some() && place {
            hints.push(("Z / X", "Turn it"));
        }
        hints.push(("⌘Z", "Undo"));
        dock_hints(ui, &hints);
    }
    true
}
