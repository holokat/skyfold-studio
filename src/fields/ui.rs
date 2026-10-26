//! Nature › Fields: pick a crop, draw a loop on the ground and it's planted; click a field to
//! select it (change its crop, turn its rows, remove it); Plant fields round the world lays out
//! farmland automatically. In the Halo layout: the crop cards, the loop tool and the fields list
//! in the tool panel, the selected field's card in the inspector, the tool and hints in the dock.

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, RichText, pos2, vec2};

use super::{Crop, PlantWorld};
use crate::editor::Tab;
use crate::io::WorldData;
use crate::ui::UiState;
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// The Nature tab's Fields mode.
#[derive(Resource)]
pub struct FieldTool {
    /// Nature tab: fields (true) or wild nature (false)
    pub on: bool,
    pub crop: Crop,
    /// the selected field
    pub selected: Option<u64>,
    /// "Plant fields round the world" was pressed
    pub auto: bool,
}

impl Default for FieldTool {
    fn default() -> Self {
        // (`WB_FIELDS=1` opens Nature › Fields, for screenshot tests)
        Self {
            on: std::env::var("WB_FIELDS").is_ok(),
            crop: Crop::Wheat,
            selected: None,
            auto: false,
        }
    }
}

impl FieldTool {
    pub fn drawing(&self, ed: &crate::editor::Editor) -> bool {
        self.on && ed.tab == Tab::Nature
    }
}

/// The editor knows when Fields is open (the nature brush stands down).
pub(super) fn sync_mode(tool: Res<FieldTool>, mut ed: ResMut<crate::editor::Editor>) {
    if ed.nature_fields != tool.on {
        ed.nature_fields = tool.on;
    }
}

/// A drawn loop plants the crop; a click on a field selects it (or nothing).
pub(super) fn draw_field(
    stroke: Res<crate::tools::Stroke>,
    mut tool: ResMut<FieldTool>,
    mut d: WorldData,
) {
    if std::mem::take(&mut tool.auto) {
        d.history.begin("Plant fields");
        let opts = super::auto::Auto {
            clusters: ((d.terrain.size() / 900.0).powi(2) as usize).clamp(3, 24),
            area: None,
            crops: vec![],
            seed: d.terrain.generation as u32 ^ 0x5eed,
        };
        let WorldData {
            fields,
            terrain,
            history,
            nature,
            water,
            store,
            ..
        } = &mut d;
        let r = super::auto::auto(
            &mut PlantWorld {
                fields,
                terrain,
                history,
                nature,
                water,
            },
            store,
            &opts,
        );
        d.history.commit(&d.store);
        match r {
            Ok(ids) => d.ed.toast(format!(
                "Planted {} fields in {} patchworks — undo takes them away",
                ids.len(),
                opts.clusters
            )),
            Err(e) => d.ed.toast(e),
        }
        d.meta.dirty = true;
        return;
    }
    if !tool.drawing(&d.ed) || !stroke.just_ended {
        return;
    }
    let raw: Vec<Vec2> = stroke.path.iter().map(|p| p.xz()).collect();
    let (lo, hi) = super::bounds(&raw);
    // a click: select the field under it
    if raw.len() < 4 || (hi - lo).max_element() < 4.0 {
        tool.selected = stroke
            .start_hit
            .and_then(|h| d.fields.at(h.xz()).map(|f| f.id));
        return;
    }
    let Some(outline) = crate::setpieces::clean_outline(&raw) else {
        d.ed.toast("Hold the mouse and draw a loop round the field, like drawing a lake");
        return;
    };
    let crop = tool.crop;
    let label = format!("Plant {}", crop.name().to_lowercase());
    let WorldData {
        fields,
        terrain,
        history,
        nature,
        water,
        ..
    } = &mut d;
    match super::add(
        &mut PlantWorld {
            fields,
            terrain,
            history,
            nature,
            water,
        },
        crop,
        &outline,
        None,
        &label,
    ) {
        Ok(id) => {
            tool.selected = Some(id);
            let area = super::poly_area(&outline);
            d.ed.toast(format!(
                "{} planted: {:.1} ha — click it to change the crop or turn the rows",
                crop.name(),
                area / 10_000.0
            ));
            d.meta.dirty = true;
        }
        Err(e) => d.ed.toast(e),
    }
}

/// A ring at the cursor while a field waits for its loop; the selected field outlined.
pub(super) fn cursor(
    tool: Res<FieldTool>,
    ed: Res<crate::editor::Editor>,
    fields: Res<super::Fields>,
    pointer: Res<crate::camera::Pointer>,
    stroke: Res<crate::tools::Stroke>,
    terrain: Res<crate::terrain::Terrain>,
    mut gizmos: Gizmos,
) {
    if !tool.drawing(&ed) {
        return;
    }
    let on_ground = |q: Vec2, up: f32| Vec3::new(q.x, terrain.height_at(q.x, q.y) + up, q.y);
    if let Some(f) = tool.selected.and_then(|id| fields.get(id)) {
        let poly = f.poly();
        let mut pts = vec![];
        for i in 0..=poly.len() {
            let (a, b) = (poly[i % poly.len()], poly[(i + 1) % poly.len()]);
            let n = ((a.distance(b) / 3.0).ceil() as usize).max(1);
            for k in 0..n {
                pts.push(on_ground(a.lerp(b, k as f32 / n as f32), 0.4));
            }
        }
        gizmos.linestrip(pts, Color::srgba(1.0, 0.85, 0.4, 0.95));
        // the rows' direction through the middle
        let c = f.centre();
        let d = (Quat::from_rotation_y(f.rows) * Vec3::X).xz() * 6.0;
        gizmos.arrow(
            on_ground(c - d, 0.6),
            on_ground(c + d, 0.6),
            Color::srgba(1.0, 0.85, 0.4, 0.95),
        );
    }
    if pointer.over_ui || stroke.active {
        return;
    }
    let Some(hit) = pointer.hit else { return };
    let ring: Vec<Vec3> = (0..=32)
        .map(|i| {
            on_ground(
                hit.xz() + Vec2::from_angle(std::f32::consts::TAU * i as f32 / 32.0) * 1.4,
                0.25,
            )
        })
        .collect();
    let [r, g, b] = tool.crop.swatch();
    gizmos.linestrip(ring, Color::srgb_u8(r, g, b));
}

// ------------------------------------------------------------------ small Halo bits (private)

/// The Central icon on a crop's card.
fn crop_icon(c: Crop) -> &'static str {
    use Crop::*;
    match c {
        Wheat | Barley | Rye => "Cereals",
        Corn | Hay | Hops => "Grass",
        Sunflowers => "Sun",
        Potatoes => "Bean",
        Cabbages => "Garlic",
        Carrots | Pumpkins => "Orange",
        Strawberries => "Strawberry",
        Tomatoes | Vineyard => "Cherry",
        Lavender => "Blossom",
        Tulips => "Rose",
        Cotton => "Cloud",
        Rice => "Drop",
        Tea => "Tea",
        Plowed => "Shovel",
    }
}

/// One line of text, cut with an ellipsis at `max_w`.
fn one_line(
    p: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_w.max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    p.layout_job(job)
}

/// A crop's well: its colour glowing from the top middle, its icon on it.
fn paint_crop_well(
    p: &egui::Painter,
    r: egui::Rect,
    c: Crop,
    hovered: bool,
    icon_size: f32,
    ui: &egui::Ui,
) {
    let [cr, cg, cb] = c.swatch();
    p.rect_filled(
        r,
        CornerRadius::same(8),
        Color32::from_rgba_unmultiplied(cr, cg, cb, if hovered { 66 } else { 51 }),
    );
    // radial glow (fades to nothing before the rounded corners)
    let centre = pos2(r.center().x, r.top() + r.height() * 0.35);
    let rad = r.width() * 0.6;
    let n = 40;
    let mut m = egui::Mesh::default();
    m.colored_vertex(centre, Color32::from_rgba_unmultiplied(cr, cg, cb, 204));
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        m.colored_vertex(
            centre + vec2(a.cos(), a.sin()) * rad,
            Color32::from_rgba_unmultiplied(cr, cg, cb, 0),
        );
        if k > 0 {
            m.add_triangle(0, k, k + 1);
        }
    }
    p.with_clip_rect(r.intersect(p.clip_rect()))
        .add(egui::Shape::mesh(m));
    // the icon reads on light crops (cotton, wheat) and dark ones (vineyard, soil) alike
    let lum = 0.299 * cr as f32 + 0.587 * cg as f32 + 0.114 * cb as f32;
    let col = if lum > 175.0 {
        Color32::from_rgba_unmultiplied(10, 12, 16, 200)
    } else {
        Color32::WHITE
    };
    paint_icon(ui, r.center(), crop_icon(c), icon_size, col);
}

/// A crop card: the glowing colour well with its icon, the name below; a white ring when picked.
fn crop_card(ui: &mut egui::Ui, c: Crop, selected: bool, w: f32) -> egui::Response {
    let well = w - 6.0;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(w, 4.0 + well + 4.0 + 14.0 + 6.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let wr = egui::Rect::from_min_size(rect.min + vec2(3.0, 4.0), egui::Vec2::splat(well));
        paint_crop_well(ui.painter(), wr, c, resp.hovered(), 20.0, ui);
        let p = ui.painter();
        if selected {
            p.rect_stroke(
                wr,
                CornerRadius::same(8),
                egui::Stroke::new(2.0, Color32::WHITE),
                egui::StrokeKind::Outside,
            );
        } else if resp.hovered() {
            p.rect_stroke(
                wr,
                CornerRadius::same(8),
                egui::Stroke::new(1.0, Color32::from_white_alpha(90)),
                egui::StrokeKind::Outside,
            );
        }
        let col = if selected { TEXT } else { TEXT_DIM };
        let g = one_line(p, c.name(), FontId::proportional(11.5), col, w - 2.0);
        p.galley(
            pos2(rect.center().x - g.size().x * 0.5, wr.bottom() + 4.0),
            g,
            col,
        );
    }
    resp.on_hover_text(c.name())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A full-width soft button in the danger colour (Remove …).
fn danger_button(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 30.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() {
            Color32::from_white_alpha(46)
        } else if resp.hovered() {
            SOFT_HOVER
        } else {
            SOFT
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_BUTTON), fill);
        let g = ui
            .painter()
            .layout_no_wrap(text.to_owned(), FontId::new(13.0, semibold()), DANGER);
        let iw = 20.0;
        let x = rect.center().x - (g.size().x + iw) * 0.5;
        paint_icon(ui, pos2(x + 7.0, rect.center().y), icon, 14.0, DANGER);
        ui.painter()
            .galley(pos2(x + iw, rect.center().y - g.size().y * 0.5), g, DANGER);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ tool panel

/// The tool panel in Fields mode (the panel header's sub-tabs switch Wild nature / Fields).
pub fn panel(ui: &mut egui::Ui, s: &mut UiState) {
    crate::ui::tips::card(
        ui,
        "fields",
        "Fields",
        &[
            "Pick a crop, then hold the mouse and draw a loop round the field on the ground, like drawing a lake. It's planted in rows along its long side.",
            "Click a field to select it: change its crop, turn its rows, or remove it. Undo takes back any of it.",
            "Plant fields round the world lays out patchworks of farmland on the flat lowland, near villages first, with crops that suit each biome.",
        ],
    );
    section_fold(
        ui,
        "field_crops",
        "Crop",
        &Crop::ALL.len().to_string(),
        |ui| {
            let cur = Crop::ALL.iter().position(|c| *c == s.field_tool.crop);
            if let Some(i) = card_grid(ui, 4, Crop::ALL.len(), |ui, i, w| {
                crop_card(ui, Crop::ALL[i], cur == Some(i), w)
            }) {
                s.field_tool.crop = Crop::ALL[i];
            }
        },
    );
    section_fold(ui, "field_plant", "Plant", "", |ui| {
        // the one tool: drawing a loop plants the picked crop
        tool_grid(
            ui,
            1,
            &[(
                "SelectLasso",
                "Draw a loop round the field",
                "Hold the mouse and draw a loop round the field on the ground",
            )],
            Some(0),
        );
        hint_small(
            ui,
            "Its ground is tilled, wild trees and bushes cleared, and the crop grows in rows that sway in the wind.",
        );
        ui.add_space(2.0);
        if wide_icon_button(ui, "SparklesTwo", "Plant fields round the world").on_hover_text("Patchworks of fields on the flat lowland, near villages first, crops by biome (undoable)").clicked() {
            s.field_tool.auto = true;
            s.ed.toast("Planting fields…");
        }
        hint_small(
            ui,
            "Patchworks of fields on the flat lowland, near villages first, crops by biome.",
        );
    });
    let n = s.fields.list.len();
    let total: f32 = s.fields.list.iter().map(|f| f.area()).sum();
    let note = if n == 0 {
        String::new()
    } else {
        format!("{n} · {:.1} ha", total / 10_000.0)
    };
    section_fold(ui, "field_list", "Fields", &note, |ui| {
        if n == 0 {
            hint_small(ui, "No fields yet.");
            return;
        }
        ui.spacing_mut().item_spacing.y = 2.0;
        let list: Vec<(u64, Crop, f32)> = s
            .fields
            .list
            .iter()
            .map(|f| (f.id, f.crop, f.area()))
            .collect();
        for (id, crop, area) in list.into_iter().rev().take(60) {
            let on = s.field_tool.selected == Some(id);
            if list_row(
                ui,
                Lead::Icon(crop_icon(crop)),
                crop.name(),
                "",
                &format!("{:.2} ha", area / 10_000.0),
                on,
            )
            .clicked()
            {
                s.field_tool.selected = if on { None } else { Some(id) };
                if let Some(f) = s.fields.get(id) {
                    let c = f.centre();
                    // fly to it (same angle, close enough to see the rows)
                    let (yaw, pitch) = (s.orbit.yaw, s.orbit.pitch.max(0.35));
                    let dist = (f.area().sqrt() * 1.6).clamp(60.0, 600.0);
                    s.orbit.set_view(
                        Vec3::new(c.x, s.terrain.height_at(c.x, c.y), c.y),
                        yaw,
                        pitch,
                        dist,
                    );
                }
            }
        }
    });
}

// ------------------------------------------------------------------ inspector (the selected field)

/// The inspector in Fields mode: the selected field.
pub fn right(ui: &mut egui::Ui, s: &mut UiState) {
    let Some(f) = s
        .field_tool
        .selected
        .and_then(|id| s.fields.get(id))
        .cloned()
    else {
        dim(
            ui,
            "Click a field to select it, or draw a loop to plant a new one.",
        );
        return;
    };
    // the hero: the crop's well, its name and area
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let (r, _) = ui.allocate_exact_size(vec2(54.0, 54.0), egui::Sense::hover());
        paint_crop_well(ui.painter(), r, f.crop, false, 22.0, ui);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(8.0);
            ui.label(
                RichText::new(f.crop.name())
                    .font(FontId::new(14.0, semibold()))
                    .color(TEXT),
            );
            ui.label(
                RichText::new(format!(
                    "{:.2} ha ({:.0} m²)",
                    f.area() / 10_000.0,
                    f.area()
                ))
                .size(12.0)
                .color(TEXT_DIM),
            );
        });
    });
    ui.add_space(4.0);
    let mut crop = f.crop;
    dropdown_row(ui, "Crop", "field_crop", crop.name(), |ui| {
        for c in Crop::ALL {
            if menu_row(ui, crop_icon(c), c.name(), "", crop == c).clicked() {
                crop = c;
            }
        }
    });
    if crop != f.crop {
        let _ = super::edit(&mut s.fields, &mut s.history, f.id, Some(crop), None);
        s.meta.dirty = true;
    }
    if f.crop == Crop::Tulips {
        let names = super::plants::TULIP_COLOUR_NAMES;
        let name = |c: Option<u8>| {
            c.and_then(|c| names.get(c as usize))
                .map_or("Mixed bands".to_string(), |n| {
                    let mut s = n.to_string();
                    s[..1].make_ascii_uppercase();
                    s
                })
        };
        let mut colour = f.colour;
        dropdown_row(ui, "Colour", "field_colour", name(colour), |ui| {
            for c in std::iter::once(None).chain((0..names.len() as u8).map(Some)) {
                if menu_row(ui, crop_icon(Crop::Tulips), name(c), "", colour == c).clicked() {
                    colour = c;
                }
            }
        });
        if colour != f.colour {
            let _ = super::set_colour(&mut s.fields, &mut s.history, f.id, colour);
            s.meta.dirty = true;
        }
    }
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = ui.available_width();
        let big = ((w - 6.0) * 0.66).floor();
        if icon_text_button(ui, "Rotate360Left", "Turn rows 90°", Some(big))
            .on_hover_text("Turn the rows by 90°")
            .clicked()
        {
            let _ = super::edit(
                &mut s.fields,
                &mut s.history,
                f.id,
                None,
                Some(f.rows + std::f32::consts::FRAC_PI_2),
            );
            s.meta.dirty = true;
        }
        if icon_text_button(
            ui,
            "ArrowRotateClockwise",
            "15°",
            Some(ui.available_width()),
        )
        .on_hover_text("Turn the rows by 15°")
        .clicked()
        {
            let _ = super::edit(
                &mut s.fields,
                &mut s.history,
                f.id,
                None,
                Some(f.rows + 15f32.to_radians()),
            );
            s.meta.dirty = true;
        }
    });
    ui.add_space(4.0);
    if danger_button(ui, "TrashCan", "Remove field")
        .on_hover_text("The crop goes; its tilled ground stays (paint over it, or undo)")
        .clicked()
    {
        let _ = super::remove(&mut s.fields, &mut s.history, f.id);
        s.field_tool.selected = None;
        s.meta.dirty = true;
    }
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The inspector shows the selected field; nothing selected, no inspector.
pub fn fields_inspector(s: &UiState) -> Option<(String, bool)> {
    s.field_tool
        .selected
        .and_then(|id| s.fields.get(id))
        .map(|_| ("Field".to_string(), true))
}

/// The tool dock: drawing a field, the crop it plants, the key hints.
pub fn fields_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    dock_title(ui, "SelectLasso", "Draw a field");
    dock_sep(ui);
    dock_text(
        ui,
        &format!("{} in rows along the long side", s.field_tool.crop.name()),
    );
    dock_sep(ui);
    dock_hints(ui, &[("Drag", "Draw a loop"), ("Click", "Pick a field")]);
    true
}
