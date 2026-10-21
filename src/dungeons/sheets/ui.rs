//! The Dungeons tab's "Design sheet" section in a realm made from a sheet: the sheet itself (a
//! thumbnail; click for the full sheet), the build's progress, and the markers by key letter —
//! how many there are, how many are filled, a Fill button (the best-fitting installed model for
//! all of them), and the toggles that show them in the viewport and in walk mode. Selecting a
//! marker in the viewport shows its card here with its suggestions.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::egui;

use super::markers::Marker;
use crate::ui::UiState;
use crate::ui::widgets::*;

/// The section's state, the markers' view, and what the panel asked for.
#[derive(Resource)]
pub struct SheetUi {
    /// markers shown in the viewport / in walk mode
    pub on: bool,
    pub walk: bool,
    pub hovered: Option<u32>,
    pub selected: Option<u32>,
    /// the full sheet open in a window
    pub big: bool,
    tex: Option<(PathBuf, egui::TextureHandle)>,
    /// fills asked for: (marker ids, asset or None for the best fit)
    pub requests: Vec<(Vec<u32>, Option<String>)>,
}

impl Default for SheetUi {
    fn default() -> Self {
        Self {
            on: true,
            walk: false,
            hovered: None,
            selected: None,
            big: false,
            tex: None,
            requests: vec![],
        }
    }
}

fn texture(ctx: &egui::Context, st: &mut SheetUi, path: &PathBuf) -> Option<egui::TextureId> {
    if st.tex.as_ref().is_some_and(|(p, _)| p == path) {
        return st.tex.as_ref().map(|t| t.1.id());
    }
    let img = image::open(path).ok()?;
    let img = if img.width() > 1600 {
        img.resize(1600, 1600, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    );
    let h = ctx.load_texture("dungeon_sheet", ci, egui::TextureOptions::LINEAR);
    let id = h.id();
    st.tex = Some((path.clone(), h));
    Some(id)
}

/// The section (in a realm's builder panel; nothing when the realm isn't a sheet's).
pub fn design_section(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(doc) = s.dungeon_map.0.as_ref() else {
        return;
    };
    let dir = s.meta.path.clone().map(|p| p.join("sheet"));
    let pic = dir.as_ref().and_then(|d| super::library::picture(d));
    if pic.is_none() && doc.markers.is_empty() {
        return;
    }
    let markers: Vec<Marker> = doc.markers.clone();
    let st = &mut *s.sheet_ui;
    section_box(ui, "sheet_design", "Design sheet", |ui| {
        if let Some((done, total, last, finished)) = super::progress() {
            if !finished {
                let frac = done as f32 / total.max(1) as f32;
                bar_stat(
                    ui,
                    "Building from the sheet",
                    &format!("{}%", (frac * 100.0).round()),
                    frac,
                    crate::ui::theme::OK,
                );
                hint_small(ui, &last);
            }
        }
        if let Some(p) = &pic {
            let ctx = ui.ctx().clone();
            if let Some(tex) = texture(&ctx, st, p) {
                let w = ui.available_width();
                let r = image_card(ui, Some(tex), "", "", false, egui::vec2(w, w * 2.0 / 3.0));
                if r.on_hover_text("Open the full sheet").clicked() {
                    st.big = true;
                }
            }
        }
        let open = markers.iter().filter(|m| m.filled.is_none()).count();
        stat_row(
            ui,
            "Markers",
            &format!("{} to fill · {} filled", open, markers.len() - open),
        );
        toggle(ui, &mut st.on, "Show markers");
        toggle(ui, &mut st.walk, "Show them when walking");
        hint_small(
            ui,
            "Each marker stands where the sheet puts something, at its size: fill it with a model, or build your own there.",
        );
    });
    // the markers by key letter
    let mut letters: Vec<(String, String, usize, usize, Vec<u32>)> = vec![];
    for m in &markers {
        match letters.iter_mut().find(|l| l.0 == m.letter) {
            Some(l) => {
                l.2 += 1;
                l.3 += m.filled.is_some() as usize;
                l.4.push(m.id);
            }
            None => letters.push((
                m.letter.clone(),
                m.name.clone(),
                1,
                m.filled.is_some() as usize,
                vec![m.id],
            )),
        }
    }
    letters.sort_by(|a, b| a.0.cmp(&b.0));
    if !letters.is_empty() {
        section_fold(
            ui,
            "sheet_markers",
            "Object key",
            &letters.len().to_string(),
            |ui| {
                for (letter, name, n, filled, ids) in &letters {
                    let sub = format!("{n} on the map · {filled} filled");
                    ui.horizontal(|ui| {
                        let w = ui.available_width() - 56.0;
                        ui.allocate_ui(egui::vec2(w, 42.0), |ui| {
                            list_row(
                                ui,
                                Lead::None,
                                &format!("{letter}  {name}"),
                                &sub,
                                "",
                                false,
                            )
                        });
                        let fits = markers
                            .iter()
                            .find(|m| m.letter == *letter)
                            .is_some_and(|m| !super::markers::suggest(m, 1).is_empty());
                        if filled < n
                            && fits
                            && small_button(ui, "Fill")
                                .on_hover_text("Put the best-fitting installed model on every one")
                                .clicked()
                        {
                            st.requests.push((ids.clone(), None));
                        }
                    });
                }
            },
        );
    }
    // the selected marker's card
    if let Some(m) = st
        .selected
        .and_then(|id| markers.iter().find(|m| m.id == id))
    {
        let m = m.clone();
        section_box(
            ui,
            "sheet_marker",
            &format!("{} · {}", m.letter, m.name),
            |ui| {
                stat_row(
                    ui,
                    "Size",
                    &format!("{:.0} × {:.0} × {:.0} m", m.size[0], m.size[1], m.size[2]),
                );
                stat_row(
                    ui,
                    "Zone",
                    &if m.zone > 0 {
                        format!("{:02}", m.zone)
                    } else {
                        "between zones".into()
                    },
                );
                if let Some(a) = &m.filled {
                    stat_row(ui, "Filled with", a.trim_start_matches("hp:"));
                }
                let sug = super::markers::suggest(&m, 4);
                if sug.is_empty() {
                    hint_small(
                        ui,
                        "No installed model fits its name and size: build one here, or place any object from the Objects tab.",
                    );
                }
                for (asset, scale) in sug {
                    if list_row(
                        ui,
                        Lead::Icon("Box"),
                        asset.trim_start_matches("hp:"),
                        &format!("scale {scale:.2}"),
                        "",
                        false,
                    )
                    .clicked()
                    {
                        st.requests.push((vec![m.id], Some(asset)));
                    }
                }
                if wide_icon_button(ui, "CrossSmall", "Deselect").clicked() {
                    st.selected = None;
                }
            },
        );
    }
    // the full sheet
    if st.big {
        let tex = st.tex.as_ref().map(|t| t.1.id());
        let mut open = true;
        egui::Window::new("Design sheet")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(1100.0)
            .show(ui.ctx(), |ui| {
                if let Some(t) = tex {
                    let w = ui.available_width();
                    ui.add(egui::Image::new((t, egui::vec2(w, w * 2.0 / 3.0))));
                }
            });
        st.big = open;
    }
}

/// Runs the fills the panel asked for (needs the world: they place objects).
pub fn run_requests(world: &mut World) {
    let reqs = std::mem::take(&mut world.resource_mut::<SheetUi>().requests);
    for (ids, asset) in reqs {
        let scale = None;
        match super::mcp::fill_markers(world, ids, asset, scale) {
            Ok(_) => {}
            Err(e) => warn!("dungeon sheet fill: {e}"),
        }
    }
}

// ------------------------------------------------------------------ the Create flow's picker

/// Thumbnails of the library's sheets (made a couple a frame, the map part only).
static THUMBS: std::sync::Mutex<Vec<(String, egui::TextureHandle)>> =
    std::sync::Mutex::new(Vec::new());

fn thumb(ctx: &egui::Context, id: &str, budget: &mut u32) -> Option<egui::TextureId> {
    let mut cache = THUMBS.lock().ok()?;
    if let Some((_, h)) = cache.iter().find(|(k, _)| k == id) {
        return Some(h.id());
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let img = image::open(super::library::dir().join(id).join("sheet.jpg")).ok()?;
    let (w, h) = (img.width(), img.height());
    let img = img
        .crop_imm(0, 0, (w as f32 * 0.71) as u32, h)
        .thumbnail(300, 300)
        .to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    let t = ctx.load_texture(
        format!("sheet_thumb_{id}"),
        ci,
        egui::TextureOptions::LINEAR,
    );
    let tid = t.id();
    cache.push((id.to_string(), t));
    Some(tid)
}

/// Create a dungeon › a realm from a design sheet: the library by setting, a card per sheet.
pub fn picker(ui: &mut egui::Ui, t: &mut crate::dungeons::ui::DungeonTool) {
    let lib = super::library::index();
    if lib.is_empty() {
        hint_small(
            ui,
            "The design-sheet library isn't installed (assets/dungeon_sheets).",
        );
        return;
    }
    hint_small(
        ui,
        "Pick a sheet: its zones, paths, stairs, water and boss rings are shaped into the land, with markers where its objects go.",
    );
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        if chip(ui, "All", None, t.sheet_family.is_empty()).clicked() {
            t.sheet_family.clear();
        }
        for f in super::family::FAMILIES {
            let mut name = f.to_string();
            name[..1].make_ascii_uppercase();
            if chip(ui, &name, None, t.sheet_family == f).clicked() {
                t.sheet_family = f.to_string();
            }
        }
    });
    let shown: Vec<&super::library::Entry> = lib
        .iter()
        .filter(|e| t.sheet_family.is_empty() || e.family == t.sheet_family)
        .collect();
    let mut budget = 2;
    let w = ((ui.available_width() - 6.0) / 2.0).max(80.0);
    let mut pick = None;
    for row in shown.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for e in row {
                let tex = thumb(ui.ctx(), &e.id, &mut budget);
                let sel = t.new_sheet.as_ref().is_some_and(|r| r.id == e.id);
                let kind = if e.kind == super::blueprint::Kind::Raid {
                    "raid"
                } else {
                    "party"
                };
                let sub = format!(
                    "{} · {} · {} zones",
                    e.layout_type.replace('_', " "),
                    kind,
                    e.zones
                );
                if image_card(ui, tex, &e.name, &sub, sel, egui::vec2(w, w * 0.92)).clicked() {
                    pick = Some((*e).clone());
                }
            }
        });
    }
    if budget == 0 {
        ui.ctx().request_repaint();
    }
    if let Some(e) = pick {
        t.new_sheet = Some(super::library::SheetRef {
            id: e.id.clone(),
            custom: false,
            family: e.family.clone(),
        });
        t.new_name = e.name.clone();
        t.new_instancing = if e.kind == super::blueprint::Kind::Raid {
            crate::caves::model::Instancing::Raid
        } else {
            crate::caves::model::Instancing::Party
        };
    }
    if t.new_sheet.is_none() {
        warn_line(ui, "Pick a sheet to build from.");
    }
}
