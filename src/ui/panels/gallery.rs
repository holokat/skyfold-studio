//! The asset gallery: every model, plant and rock of every style pack in one big window, to
//! look through quickly — big rendered tiles (crate::thumbs), filters by style, kind and group,
//! search, a slowly turning preview of the picked one with its size and pack, ←/→ to flip through,
//! and "Place it" to start placing it (Objects, or Nature › Place one for plants and rocks).
//! Assets in packs this account hasn't got show a lock: they can be looked at, and "Get pack"
//! shows their pack (crate::market).

use super::super::TOP_H;
use super::*;
use crate::thumbs::Subject;

/// The gallery's filters and pick (kept while the editor runs).
#[derive(Resource)]
pub struct Gallery {
    pub open: bool,
    pub style: String,
    /// None: everything
    pub category: Option<Category>,
    pub search: String,
    pub selected: Option<String>,
}

impl Default for Gallery {
    /// (`WB_GALLERY=<style>[:<asset id>]` opens it, for screenshot tests)
    fn default() -> Self {
        let env = std::env::var("WB_GALLERY").ok();
        let (style, sel) = env
            .as_deref()
            .map(|v| {
                v.split_once(':')
                    .map(|(a, b)| (a, Some(b.to_string())))
                    .unwrap_or((v, None))
            })
            .unwrap_or(("", None));
        Self {
            open: env.is_some(),
            style: style.to_string(),
            category: None,
            search: String::new(),
            selected: sel,
        }
    }
}

const KINDS: [(Option<Category>, &str); 8] = [
    (None, "All"),
    (Some(Category::Structures), "Structures"),
    (Some(Category::Props), "Props"),
    (Some(Category::Trees), "Trees"),
    (Some(Category::Rocks), "Rocks"),
    (Some(Category::Plants), "Plants"),
    (Some(Category::Debris), "Debris"),
    (Some(Category::Imported), "Imported"),
];

pub fn window(ctx: &egui::Context, s: &mut UiState) {
    let screen = ctx.content_rect();
    let size = vec2(
        (screen.width() - 120.0).min(1400.0),
        (screen.height() - TOP_H - 80.0).min(900.0),
    );
    let mut open = s.gallery.open;
    let Some(lib) = s.lib.as_deref() else { return };
    // the filtered list, in a stable order: style, kind, group, name
    let words: Vec<String> = s
        .gallery
        .search
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let market = crate::market::snapshot();
    let mut items: Vec<(
        String,
        String,
        String,
        Category,
        String,
        crate::market::Access,
    )> = lib
        .entries
        .iter()
        .filter(|e| {
            s.gallery.style.is_empty()
                || e.pack == s.gallery.style
                || e.category == Category::Imported
        })
        .filter(|e| s.gallery.category.is_none_or(|c| c == e.category))
        .filter(|e| {
            let hay = format!(
                "{} {} {} {} {}",
                e.name,
                e.id,
                e.group,
                e.category.name(),
                e.pack
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|e| {
            (
                e.id.clone(),
                e.name.clone(),
                e.group.clone(),
                e.category,
                e.pack.clone(),
                market.access(&e.id),
            )
        })
        .collect();
    items.sort_by(|a, b| (&a.4, a.3 as u8, &a.2, &a.1).cmp(&(&b.4, b.3 as u8, &b.2, &b.1)));
    let packs: Vec<String> = {
        let mut p: Vec<String> = lib.entries.iter().map(|e| e.pack.clone()).collect();
        p.sort();
        p.dedup();
        p
    };
    // keys: ←/→ flip through, Esc closes
    if !ctx.egui_wants_keyboard_input() {
        let step = ctx.input(|i| {
            (i.key_pressed(egui::Key::ArrowRight) as i32)
                - (i.key_pressed(egui::Key::ArrowLeft) as i32)
        });
        if step != 0 && !items.is_empty() {
            let cur = s
                .gallery
                .selected
                .as_ref()
                .and_then(|id| items.iter().position(|x| &x.0 == id));
            let n = items.len() as i32;
            let next = match cur {
                Some(i) => (i as i32 + step).rem_euclid(n),
                None => 0,
            };
            s.gallery.selected = Some(items[next as usize].0.clone());
        }
        // (a pack card over the gallery closes first)
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && s.market.card.is_none() {
            open = false;
        }
    }
    let mut place: Option<String> = None;
    let mut card: Option<String> = None;
    egui::Window::new(RichText::new(format!("{}  Asset gallery", icon::SQUARES_FOUR)).font(FontId::new(16.0, semibold())))
        .id(egui::Id::new("asset_gallery"))
        .collapsible(false)
        .resizable(false)
        .fixed_size(size)
        .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, TOP_H * 0.5))
        .open(&mut open)
        .show(ctx, |ui| {
            // filters
            ui.horizontal_wrapped(|ui| {
                let mut style = s.gallery.style.clone();
                let mut opts: Vec<(String, String)> = vec![(String::new(), "All styles".into())];
                opts.extend(packs.iter().map(|p| (p.clone(), crate::objects::pack_label(p))));
                for (v, l) in &opts {
                    if ui.selectable_label(style == *v, l).clicked() {
                        style = v.clone();
                    }
                }
                s.gallery.style = style;
                ui.separator();
                for (c, l) in KINDS {
                    if ui.selectable_label(s.gallery.category == c, l).clicked() {
                        s.gallery.category = c;
                    }
                }
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut s.gallery.search).hint_text(format!("{}  Search…", icon::MAGNIFYING_GLASS)).desired_width(200.0));
                ui.label(RichText::new(format!("{} assets", items.len())).small().color(TEXT_DIM));
            });
            ui.add_space(6.0);
            let side = 330.0;
            ui.horizontal_top(|ui| {
                // the grid
                let grid_w = ui.available_width() - side - 12.0;
                ui.allocate_ui_with_layout(vec2(grid_w, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                    egui::ScrollArea::vertical().id_salt("gallery_grid").auto_shrink([false, false]).max_width(grid_w).show(ui, |ui| {
                        ui.set_width(grid_w - 14.0);
                        let tile = 132.0;
                        let per_row = ((grid_w - 8.0) / (tile + 8.0)).floor().max(2.0) as usize;
                        let mut last_head = String::new();
                        let mut row: Vec<&(String, String, String, Category, String, crate::market::Access)> = vec![];
                        let mut flush = |ui: &mut egui::Ui, row: &mut Vec<&(String, String, String, Category, String, crate::market::Access)>, s: &mut UiState| {
                            if row.is_empty() {
                                return;
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                                for (id, name, _, cat, _, access) in row.iter() {
                                    let sel = s.gallery.selected.as_deref() == Some(id.as_str());
                                    let r = crate::thumbs::ui::tile(ui, &mut s.previews, &Subject::Asset(id.clone()), category_icon(*cat), name, sel, tile);
                                    crate::market::ui::badge(ui, r.rect, *access);
                                    if r.clicked() {
                                        s.gallery.selected = Some(id.clone());
                                    }
                                    if r.double_clicked() {
                                        match access {
                                            crate::market::Access::Locked(p) => card = Some(p.id.clone()),
                                            _ => place = Some(id.clone()),
                                        }
                                    }
                                }
                            });
                            ui.add_space(4.0);
                            row.clear();
                        };
                        for it in &items {
                            let head = format!("{} · {}{}", crate::objects::pack_label(&it.4), it.3.name(), if it.2.is_empty() { String::new() } else { format!(" · {}", nice_group(&it.2)) });
                            if head != last_head {
                                flush(ui, &mut row, s);
                                ui.add_space(4.0);
                                ui.label(RichText::new(&head).small().color(TEXT_DIM));
                                last_head = head;
                            }
                            row.push(it);
                            if row.len() == per_row {
                                flush(ui, &mut row, s);
                            }
                        }
                        flush(ui, &mut row, s);
                        if items.is_empty() {
                            dim(ui, "Nothing matches.");
                        }
                    });
                });
                ui.add_space(4.0);
                // the picked asset
                ui.allocate_ui_with_layout(vec2(side, ui.available_height()), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(side);
                    match s.gallery.selected.clone().and_then(|id| lib_entry_info(s, &id)) {
                        Some((id, name, lines)) => {
                            crate::thumbs::ui::card(ui, &mut s.previews, &Subject::Asset(id.clone()), &name, &lines);
                            ui.add_space(8.0);
                            match market.access(&id) {
                                crate::market::Access::Locked(p) => {
                                    if primary_button(ui, &format!("{}  Get pack — {}", icon::SHOPPING_BAG, p.price_label())).on_hover_text(format!("It's in the {} pack: see what's in it", p.name)).clicked() {
                                        card = Some(p.id.clone());
                                    }
                                }
                                _ => {
                                    if primary_button(ui, &format!("{}  Place it", icon::CURSOR_CLICK)).on_hover_text("Close the gallery and place it in the world (double-click a tile does the same)").clicked() {
                                        place = Some(id);
                                    }
                                }
                            }
                            dim(ui, "← → flip through · Esc closes");
                        }
                        None => dim(ui, "Click a tile to see it turn, with its size and pack. ← → flip through, double-click to place it."),
                    }
                });
            });
        });
    s.gallery.open = open;
    if card.is_some() {
        s.market.card = card;
    }
    if let Some(id) = place {
        start_placing(s, &id);
        s.gallery.open = false;
    }
}

fn nice_group(g: &str) -> String {
    let mut c = g.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// (id, name, info lines) of a library entry.
fn lib_entry_info(s: &UiState, id: &str) -> Option<(String, String, Vec<String>)> {
    let e = s.lib.as_deref()?.get(id)?;
    let mut lines = vec![format!(
        "{} · {}{}",
        crate::objects::pack_label(&e.pack),
        e.category.name(),
        if e.group.is_empty() {
            String::new()
        } else {
            format!(" · {}", nice_group(&e.group))
        }
    )];
    let market = crate::market::snapshot();
    match market.access(id) {
        crate::market::Access::Locked(p) => lines.push(format!(
            "{}  In the {} pack · {}",
            icon::LOCK_SIMPLE,
            p.name,
            p.price_label()
        )),
        crate::market::Access::Sample(p) => {
            lines.push(format!("Free to try from the {} pack", p.name))
        }
        _ => {
            if let Some(p) = market.pack_of(id) {
                lines.push(format!(
                    "{} pack{}",
                    p.name,
                    if p.is_free() { " · free" } else { " · yours" }
                ));
            }
        }
    }
    lines.push(format!(
        "About {:.1} m across, {:.1} m tall",
        e.radius * 2.0 / 1.1,
        e.height
    ));
    lines.push(format!(
        "Footsteps: {}{}",
        e.surface.name(),
        if e.walkable { " · walkable" } else { "" }
    ));
    Some((e.id.clone(), e.name.clone(), lines))
}

/// Starts placing an asset: Objects › Place for structures and props, Nature › Place one for
/// trees, rocks, plants and debris.
fn start_placing(s: &mut UiState, id: &str) {
    let Some(e) = s.lib.as_deref().and_then(|l| l.get(id)) else {
        return;
    };
    let (cat, pack) = (e.category, e.pack.clone());
    if Category::NATURE.contains(&cat) {
        s.ed.tab = Tab::Nature;
        s.scatter.category = cat;
        s.scatter.selected = vec![id.to_string()];
        s.scatter.preset = None;
        s.ed.scatter_mode = ScatterMode::Place;
        s.ed.toast("Click the ground to place it (Nature › Place one)");
    } else {
        s.ed.tab = Tab::Objects;
        s.ed.object_tool = ObjectTool::Place;
        s.place.category = cat;
        s.place.selected = Some(id.to_string());
        if s.place.style != pack && cat != Category::Imported {
            s.place.style = pack;
        }
        s.ed.toast("Click the ground to place it");
    }
}
