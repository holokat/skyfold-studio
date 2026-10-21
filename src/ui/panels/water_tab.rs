//! The Water tab in the Halo layout (HANDOFF › Per-tab mapping): an 8-tool grid with each tool's
//! options in the tool panel (rivers, drawn lakes and their shore brush, basins, waterfalls,
//! fish); the inspector designs the water of the selected body, of the open sea, or of what the
//! drawing tool makes next — presets by family as painted round swatches, the behaviour, and
//! every control (colours, transparency, visibility, waves, flow, foam, roughness, reflections,
//! metal, iridescence, bioluminescence, inner glow). The dock: the tool, the shore brush and the
//! key hints.

use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Stroke, pos2, vec2,
};

use super::super::UiState;
use super::super::theme::*;
use super::super::widgets::*;
use super::{meters, pct};
use crate::editor::WaterTool;
use crate::fish::FishSpecies;
use crate::thumbs::Subject;
use crate::water::lakes::{self, LakeShape, ShapeMode};
use crate::water::presets::{self, WaterPreset, in_category};
use crate::water::{
    BodyKind, ReflectionQuality, WaterBehaviour, WaterCategory, WaterSel, WaterStyle,
};

/// The tools in grid order: (tool, Central icon, name).
const WATER_TOOLS: [(WaterTool, &str, &str); 8] = [
    (WaterTool::River, "AnimatePath", "Draw River"),
    (WaterTool::DrawLake, "SelectLasso", "Draw Lake"),
    (WaterTool::ShapeLake, "Brush", "Shape Shore"),
    (WaterTool::Lake, "Bucket", "Fill Basin"),
    (WaterTool::Waterfall, "ArrowDown", "Waterfall"),
    (WaterTool::Fish, "Anchor", "Add Fish"),
    (WaterTool::Select, "Cursor1", "Select & Edit"),
    (WaterTool::Remove, "Eraser", "Remove"),
];

fn tool_hint(t: WaterTool) -> &'static str {
    match t {
        WaterTool::River => {
            "Drag a path across the land. The river flows downhill from the higher end, carves its bed, and forms rapids on steep drops."
        }
        WaterTool::DrawLake => {
            "Draw a loop around the lake. The outline becomes a smooth shore, the basin is carved to the lake type's depth and the shore blends into the land with sand, mud or pebbles and plants."
        }
        WaterTool::ShapeLake => {
            "Brush over a drawn lake's shore to push it out, pull it in or smooth it (⌥ swaps expand and shrink). The basin re-carves when you let go."
        }
        WaterTool::Lake => {
            "Click inside a valley or crater. Water fills the basin up to the fill depth above where you click."
        }
        WaterTool::Waterfall => {
            "Drag from a cliff top or a river's edge down to where the water lands. The water falls over the lip, mist rises at its foot and a plunge pool is carved there."
        }
        WaterTool::Fish => {
            "Click on any water (sea, lake or river) to release a school of fish. Each school is exported as a fishing spot."
        }
        WaterTool::Select => {
            "Click a river, lake, fish school or the open sea to design its water."
        }
        WaterTool::Remove => "Click a river, lake or fish school to remove it.",
    }
}

// ------------------------------------------------------------------ small Halo bits (private)

/// One line of text, cut with an ellipsis at `max_w`.
fn one_line(
    p: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_w.max(8.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    p.layout_job(job)
}

/// A [`section_fold`] that starts closed (until it's opened once).
fn fold_closed<R>(
    ui: &mut egui::Ui,
    salt: &str,
    title: &str,
    note: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let id = ui.make_persistent_id(("section_fold", salt));
    ui.data_mut(|d| {
        d.get_persisted_mut_or(id, false);
    });
    section_fold(ui, salt, title, note, body)
}

/// A labelled text field: dim label, then a dark well.
fn text_field(ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str) -> egui::Response {
    if !label.is_empty() {
        ui.label(RichText::new(label).size(12.5).color(TEXT_DIM));
    }
    egui::Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(R_INPUT))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new(hint).color(TEXT_MUTE))
                    .desired_width(ui.available_width()),
            )
        })
        .inner
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

/// A dim label then a widget filling the rest of the row.
fn labelled<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TEXT_DIM));
        ui.add_space(4.0);
        add(ui)
    })
    .inner
}

// ------------------------------------------------------------------ painting helpers

fn srgb(c: [f32; 3]) -> Color32 {
    color32([
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
    ])
}

/// How a colour reads on screen after the water's lighting (deep colours are albedos: brighten).
fn lit(c: [f32; 3], gain: f32) -> [f32; 3] {
    let to_lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    };
    [
        to_srgb(to_lin(c[0]) * gain),
        to_srgb(to_lin(c[1]) * gain),
        to_srgb(to_lin(c[2]) * gain),
    ]
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// A star-shaped blob around `c` (fan mesh, so wobbly concave outlines fill correctly), coloured
/// `centre` in the middle fading to `rim` at the edge.
fn blob(
    p: &egui::Painter,
    c: Pos2,
    radius: egui::Vec2,
    seed: f32,
    scale: f32,
    centre: Color32,
    rim: Color32,
) {
    let n = 48;
    let mut m = egui::Mesh::default();
    m.colored_vertex(c, centre);
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        let r = scale
            * (1.0
                + 0.11 * (2.0 * a + seed).sin()
                + 0.07 * (3.0 * a + seed * 1.7).sin()
                + 0.03 * (5.0 * a + seed * 2.3).sin());
        m.colored_vertex(
            c + vec2(a.cos() * radius.x * r, a.sin() * radius.y * r),
            rim,
        );
        if k > 0 {
            m.add_triangle(0, k as u32, k as u32 + 1);
        }
    }
    p.add(egui::Shape::mesh(m));
}

fn blob_point(c: Pos2, radius: egui::Vec2, seed: f32, scale: f32, a: f32) -> Pos2 {
    let r = scale
        * (1.0
            + 0.11 * (2.0 * a + seed).sin()
            + 0.07 * (3.0 * a + seed * 1.7).sin()
            + 0.03 * (5.0 * a + seed * 2.3).sin());
    c + vec2(a.cos() * radius.x * r, a.sin() * radius.y * r)
}

fn seed_of(id: &str, k: f32) -> f32 {
    id.bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) as f32
        * k
}

/// A little painted sea in a disc: depth colour fading to the water colour over a sand bar, wave
/// lines, and the preset's character (metal sheen, rainbow film, glow sparks, inner light).
fn paint_water_disc(p: &egui::Painter, c: Pos2, r: f32, s: &WaterStyle, seed: f32) {
    let deep = srgb(lit(s.deep, 2.6));
    let shallow = srgb(s.shallow);
    let mid = mix(deep, shallow, 0.45);
    let bottom = if s.transparency > 0.5 {
        mix(
            shallow,
            Color32::from_rgb(228, 212, 170),
            0.25 * s.transparency.min(1.0) * (s.clarity / 12.0).min(1.0),
        )
    } else {
        shallow
    };
    let emissive = srgb(s.emissive);
    let glows: Vec<(f32, f32)> = (0..3)
        .map(|k| {
            (
                0.2 + 0.3 * k as f32 + 0.05 * (seed + k as f32).sin(),
                0.35 + 0.2 * ((seed * 3.0 + k as f32).sin() * 0.5 + 0.5),
            )
        })
        .collect();
    // the colour at (u, v) of the swatch's square, 0..1
    let colour = |u: f32, v: f32| -> Color32 {
        let mut col = if v < 0.62 {
            mix(deep, mid, v / 0.62)
        } else {
            mix(mid, bottom, (v - 0.62) / 0.38)
        };
        if s.emissive_strength > 0.01 {
            let k = (s.emissive_strength * 2.2).min(1.0);
            for (gu, gv) in &glows {
                let d = ((u - gu).powi(2) + (v - gv).powi(2)).sqrt();
                col = mix(
                    col,
                    emissive,
                    (1.0 - d / 0.24).clamp(0.0, 1.0).powf(1.4) * 0.6 * k,
                );
            }
        }
        if s.metallic > 0.05 {
            // liquid metal: a bright diagonal sheen
            let t = (u - (0.25 - 0.6 * v)) / 0.22;
            if (0.0..1.0).contains(&t) {
                col = mix(col, Color32::WHITE, s.metallic * 0.47 * (1.0 - t));
            }
        }
        if s.iridescence > 0.05 && (0.15..0.42).contains(&v) {
            // thin-film rainbow
            col = mix(
                col,
                Color32::from(egui::ecolor::Hsva::new(u, 0.65, 1.0, 1.0)),
                s.iridescence * 0.35,
            );
        }
        col
    };
    let (rings, segs) = (7u32, 40u32);
    let mut m = egui::Mesh::default();
    m.colored_vertex(c, colour(0.5, 0.5));
    for k in 1..=rings {
        let f = k as f32 / rings as f32;
        for j in 0..segs {
            let a = j as f32 / segs as f32 * std::f32::consts::TAU;
            let (dx, dy) = (a.cos() * f, a.sin() * f);
            m.colored_vertex(c + vec2(dx, dy) * r, colour(0.5 + dx * 0.5, 0.5 + dy * 0.5));
        }
    }
    for j in 0..segs {
        m.add_triangle(0, 1 + j, 1 + (j + 1) % segs);
    }
    for k in 1..rings {
        let (a0, b0) = (1 + (k - 1) * segs, 1 + k * segs);
        for j in 0..segs {
            let j1 = (j + 1) % segs;
            m.add_triangle(a0 + j, b0 + j, a0 + j1);
            m.add_triangle(a0 + j1, b0 + j, b0 + j1);
        }
    }
    p.add(egui::Shape::mesh(m));
    // wave lines: taller and whiter with more waves and foam, inside the disc
    let amp = (0.6 + s.waves * 2.2).min(5.0) * (r / 28.0);
    let foam_a = (40.0 + s.foam * 120.0 + s.waves * 30.0).min(200.0) as u8;
    for (row, yk) in [(0, 0.3f32), (1, 0.55), (2, 0.8)] {
        let y = c.y - r + 2.0 * r * yk;
        let hw = (r * r - (y - c.y).powi(2)).max(0.0).sqrt() - 2.0 - amp;
        if hw < 3.0 {
            continue;
        }
        let pts: Vec<Pos2> = (0..=16)
            .map(|i| {
                let x = c.x - hw + 2.0 * hw * i as f32 / 16.0;
                let u = (x - (c.x - r)) / (2.0 * r) * 20.0;
                pos2(
                    x,
                    y + (u * 0.9 + seed + row as f32 * 1.7).sin() * amp * (0.5 + 0.5 * yk),
                )
            })
            .collect();
        p.add(egui::Shape::line(
            pts,
            Stroke::new(
                1.0 + 0.4 * row as f32,
                Color32::from_white_alpha(foam_a / (3 - row) as u8),
            ),
        ));
    }
    // bioluminescent sparks
    if s.glow_strength > 0.01 {
        let g = srgb(s.glow);
        for k in 0..9 {
            let fx = ((seed * 13.7 + k as f32 * 3.31).sin() * 0.5 + 0.5) * 0.9 + 0.05;
            let fy = ((seed * 7.1 + k as f32 * 1.93).cos() * 0.5 + 0.5) * 0.8 + 0.1;
            let q = c + vec2((fx - 0.5) * 2.0 * r, (fy - 0.5) * 2.0 * r);
            if q.distance(c) > r - 3.0 {
                continue;
            }
            p.circle_filled(
                q,
                3.2,
                Color32::from_rgba_unmultiplied(g.r(), g.g(), g.b(), 60),
            );
            p.circle_filled(q, 1.3, mix(g, Color32::WHITE, 0.4));
        }
    }
}

/// A water preset: its painted round swatch with the name underneath; a white ring with a dark
/// gap when picked.
fn preset_swatch(ui: &mut egui::Ui, pr: &WaterPreset, selected: bool, w: f32) -> egui::Response {
    let d = (w - 4.0).max(16.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, d + 6.0 + 16.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let c = pos2(rect.center().x, rect.top() + 3.0 + d * 0.5);
        paint_water_disc(ui.painter(), c, d * 0.5, &pr.style, seed_of(pr.id, 0.0137));
        let p = ui.painter();
        if selected {
            p.circle_stroke(
                c,
                d * 0.5 + 1.0,
                Stroke::new(2.0, Color32::from_rgba_unmultiplied_const(10, 12, 16, 230)),
            );
            p.circle_stroke(c, d * 0.5 + 3.0, Stroke::new(2.0, Color32::WHITE));
        } else if resp.hovered() {
            p.circle_stroke(
                c,
                d * 0.5 + 2.0,
                Stroke::new(1.5, Color32::from_white_alpha(110)),
            );
        }
        let col = if selected { TEXT } else { TEXT_DIM };
        let g = one_line(p, pr.name, FontId::proportional(11.0), col, w + 4.0);
        p.galley(
            pos2(rect.center().x - g.size().x * 0.5, c.y + d * 0.5 + 6.0),
            g,
            col,
        );
    }
    resp.on_hover_text(format!("{}\n{}", pr.name, pr.blurb))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Family chips (with counts) + the presets of the picked family as round swatches. Returns the
/// picked preset.
fn preset_picker(
    ui: &mut egui::Ui,
    tab: &mut WaterCategory,
    current: &str,
    cats: &[WaterCategory],
) -> Option<&'static WaterPreset> {
    if !cats.contains(tab) {
        *tab = cats[0];
    }
    if cats.len() > 1 {
        let opts: Vec<(WaterCategory, &str, Option<usize>)> = cats
            .iter()
            .map(|c| (*c, c.name(), Some(in_category(*c).count())))
            .collect();
        chips(ui, tab, &opts);
        ui.add_space(6.0);
    }
    let list: Vec<&'static WaterPreset> = in_category(*tab).collect();
    let cols = 4;
    let gap = 8.0;
    let w = ((ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32).floor();
    let mut picked = None;
    for row in list.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for pr in row {
                if preset_swatch(ui, pr, current == pr.id, w).clicked() {
                    picked = Some(*pr);
                }
            }
        });
        ui.add_space(4.0);
    }
    picked
}

/// How many presets the families hold.
fn preset_count(cats: &[WaterCategory]) -> usize {
    cats.iter().map(|c| in_category(*c).count()).sum()
}

fn layer_col(l: crate::terrain::layers::Layer) -> Color32 {
    let s = l.swatch();
    Color32::from_rgb(s[0], s[1], s[2])
}

/// A painted top-down picture of a lake type: the land around, the shore bands, the water
/// (deep in the middle, shallow at the edge) and its plants.
fn paint_lake_tile(p: &egui::Painter, r: Rect, s: &LakeShape, seed: f32) {
    let land = Color32::from_rgb(s.land[0], s.land[1], s.land[2]);
    p.rect_filled(r, CornerRadius::same(8), mix(land, Color32::BLACK, 0.04));
    p.rect_filled(
        Rect::from_min_max(r.min, pos2(r.right(), r.center().y)),
        CornerRadius {
            nw: 8,
            ne: 8,
            sw: 0,
            se: 0,
        },
        Color32::from_white_alpha(6),
    );
    let c = r.center() + vec2(0.0, 1.0);
    let base = vec2(r.width() * 0.25, r.height() * 0.25) * (0.75 + 0.25 * (s.depth / 8.0).min(1.0))
        + vec2(3.0, 3.0);
    let water = presets::preset_style(s.water);
    if s.rim > 0.0 {
        let rim = mix(layer_col(s.shore), Color32::BLACK, 0.35);
        blob(p, c, base, seed, 1.55, rim, mix(rim, land, 0.6));
    }
    if let Some(o) = s.outer {
        let oc = layer_col(o);
        blob(p, c, base, seed, 1.42, oc, mix(oc, land, 0.7));
    }
    let sh = layer_col(s.shore);
    blob(
        p,
        c,
        base,
        seed,
        1.0 + (s.shore_w / 22.0).clamp(0.12, 0.35),
        sh,
        mix(sh, land, 0.25),
    );
    let deep = srgb(lit(water.deep, 2.6));
    let shallow = srgb(water.shallow);
    let centre = mix(shallow, deep, (s.depth / 6.0).clamp(0.25, 1.0));
    blob(p, c, base, seed, 1.0, centre, mix(shallow, sh, 0.25));
    // a soft glint
    p.circle_filled(
        c + vec2(-base.x * 0.35, -base.y * 0.4),
        base.y * 0.35,
        Color32::from_white_alpha(18),
    );
    // islands
    if s.islands > 0.0 {
        for k in 0..3 {
            let a = seed + k as f32 * 2.1;
            let q = c + vec2(a.cos() * base.x * 0.45, a.sin() * base.y * 0.4);
            blob(
                p,
                q,
                vec2(base.x * 0.16, base.y * 0.13),
                seed + k as f32,
                1.0,
                mix(land, sh, 0.4),
                mix(land, sh, 0.6),
            );
        }
    }
    // plants
    for (asset, zone, density) in s.plants {
        let count = ((density.sqrt() * 7.0).round() as usize).clamp(2, 14);
        for k in 0..count {
            let a = seed * 3.0
                + k as f32 * (std::f32::consts::TAU / count as f32)
                + (k as f32 * 1.37).sin() * 0.3;
            let (scale, kind) = match zone {
                lakes::Zone::Edge => (1.0 + 0.06 * (k as f32 * 2.3).sin(), 0),
                lakes::Zone::Water => (0.72 + 0.12 * (k as f32 * 1.9).sin(), 1),
                lakes::Zone::Bank => (1.45 + 0.15 * (k as f32 * 3.1).sin(), 2),
                lakes::Zone::Island => (0.42, 0),
            };
            let q = blob_point(c, base, seed, scale, a);
            if !r.shrink(4.0).contains(q) {
                continue;
            }
            match (*asset, kind) {
                ("LilyPads", _) => {
                    p.circle_filled(q, 2.6, Color32::from_rgb(70, 132, 52));
                    p.circle_filled(q + vec2(0.8, -0.6), 0.9, Color32::from_rgb(232, 180, 205));
                }
                ("Reeds", _) => {
                    for dx in [-1.6f32, 0.0, 1.6] {
                        p.line_segment(
                            [q + vec2(dx, 2.5), q + vec2(dx * 1.4, -3.5)],
                            Stroke::new(1.1, Color32::from_rgb(70, 92, 40)),
                        );
                    }
                }
                ("Palm", _) => {
                    for k2 in 0..5 {
                        let aa = k2 as f32 * 1.2566 + seed;
                        p.line_segment(
                            [q, q + vec2(aa.cos(), aa.sin()) * 5.0],
                            Stroke::new(1.6, Color32::from_rgb(50, 104, 40)),
                        );
                    }
                }
                ("Boulder" | "Pebbles" | "RockCluster" | "RockSpire" | "MossyRock", _) => {
                    p.circle_filled(
                        q,
                        if *asset == "Pebbles" { 1.3 } else { 2.4 },
                        Color32::from_rgb(132, 128, 120),
                    );
                }
                ("Willow" | "SnowyPine" | "Pine" | "DeadTree", _) => {
                    p.circle_filled(q, 4.0, Color32::from_rgb(46, 78, 38));
                }
                _ => {
                    p.circle_filled(q, 1.6, mix(Color32::from_rgb(110, 150, 70), land, 0.3));
                }
            }
        }
    }
}

/// A lake-type card: the painted picture in a square well, the name below, a white ring when picked.
fn lake_card(ui: &mut egui::Ui, s: &LakeShape, selected: bool, w: f32) -> egui::Response {
    let well = w - 6.0;
    let (rect, resp) =
        ui.allocate_exact_size(vec2(w, 4.0 + well + 4.0 + 14.0 + 6.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let img = Rect::from_min_size(rect.min + vec2(3.0, 4.0), egui::Vec2::splat(well));
        let clip = img.intersect(ui.clip_rect());
        paint_lake_tile(
            &ui.painter().with_clip_rect(clip),
            img,
            s,
            seed_of(s.id, 0.0173),
        );
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
        let g = one_line(p, s.name, FontId::proportional(11.5), col, w - 4.0);
        p.galley(
            pos2(rect.center().x - g.size().x * 0.5, img.bottom() + 4.0),
            g,
            col,
        );
    }
    resp.on_hover_text(format!("{}\n{}", s.name, s.blurb))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The listed lake types as cards, four across (drawing tool / inspector). Returns the picked index
/// into `lakes::SHAPES`.
fn lake_picker(ui: &mut egui::Ui, current: &str) -> Option<usize> {
    let listed: Vec<usize> = lakes::SHAPES
        .iter()
        .enumerate()
        .filter(|(_, s)| s.listed)
        .map(|(i, _)| i)
        .collect();
    card_grid(ui, 4, listed.len(), |ui, k, w| {
        let s = &lakes::SHAPES[listed[k]];
        lake_card(ui, s, current == s.id, w)
    })
    .map(|k| listed[k])
}

fn behaviour_icon(b: WaterBehaviour) -> &'static str {
    match b {
        WaterBehaviour::Still => "MinusMedium",
        WaterBehaviour::GentleRipples => "Wave",
        WaterBehaviour::SlowFlow => "ArrowRight",
        WaterBehaviour::FastFlow => "FastForward",
        WaterBehaviour::OceanSwells => "WaveRound2",
        WaterBehaviour::Rapids => "Bolt",
        WaterBehaviour::Whirlpool => "Rotate",
        WaterBehaviour::Storm => "Thunder",
        WaterBehaviour::Tidal => "ArrowsRepeat",
        WaterBehaviour::Bubbling => "Bubble5",
        WaterBehaviour::Molten => "Fire1",
    }
}

/// A colour button that only writes back real edits (egui's HSV round trip nudges the value
/// every frame otherwise, which would turn every preset into "Custom").
fn color_button(ui: &mut egui::Ui, c: &mut [f32; 3]) {
    let mut tmp = *c;
    if ui.color_edit_button_rgb(&mut tmp).changed() {
        *c = tmp;
    }
}

/// A colour row: the swatch button, the label, the hex value on the right.
fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [f32; 3]) {
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size = vec2(26.0, 22.0);
        color_button(ui, c);
        ui.add_space(2.0);
        ui.label(RichText::new(label).color(TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            ui.label(
                RichText::new(format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2])))
                    .font(FontId::new(11.0, mono()))
                    .color(TEXT_MUTE),
            );
        });
    });
}

// ------------------------------------------------------------------ tool panel

pub fn water_left(ui: &mut egui::Ui, s: &mut UiState) {
    section_fold(ui, "water_tools", "Water", "", |ui| {
        let items: Vec<(&str, &str, &str)> =
            WATER_TOOLS.iter().map(|(_, i, n)| (*i, *n, "")).collect();
        let sel = WATER_TOOLS.iter().position(|t| t.0 == s.ed.water_tool);
        if let Some(i) = tool_grid(ui, 4, &items, sel) {
            s.ed.water_tool = WATER_TOOLS[i].0;
        }
        dim(ui, tool_hint(s.ed.water_tool));
    });
    match s.ed.water_tool {
        WaterTool::River => {
            section_fold(ui, "water_new_river", "New river", "", |ui| {
                slider_row(ui, "Width", &mut s.ed.river_width, 4.0..=80.0, meters);
                slider_row(ui, "Depth", &mut s.ed.river_depth, 0.5..=12.0, |v| {
                    format!("{v:.1} m")
                });
                hint_small(ui, "Its water is picked in the inspector on the right.");
            });
        }
        WaterTool::DrawLake => {
            section_fold(ui, "water_lake_type", "Lake type", "", |ui| {
                let cur =
                    lakes::SHAPES[s.ed.water_tools.lake_shape.min(lakes::SHAPES.len() - 1)].id;
                if let Some(i) = lake_picker(ui, cur) {
                    s.ed.water_tools.lake_shape = i;
                }
                let sh = &lakes::SHAPES[s.ed.water_tools.lake_shape.min(lakes::SHAPES.len() - 1)];
                ui.add_space(2.0);
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        sh.name,
                        presets::preset(sh.water).map(|p| p.name).unwrap_or("")
                    ))
                    .color(TEXT),
                );
                hint_small(ui, sh.blurb);
                let d = sh.depth;
                slider_row(
                    ui,
                    "Depth",
                    &mut s.ed.water_tools.lake_depth,
                    0.3..=2.5,
                    move |v| format!("{:.1} m", v * d),
                );
            });
            section_fold(ui, "water_lake_plants", "Shore plants", "", |ui| {
                slider_row(
                    ui,
                    "Shore plants",
                    &mut s.ed.water_tools.lake_plants,
                    0.0..=2.0,
                    |v| if v < 0.01 { "none".into() } else { pct(v) },
                );
                hint_small(
                    ui,
                    "The lake type picks the plants; this sets how many grow round the shore.",
                );
            });
        }
        WaterTool::ShapeLake => {
            section_fold(ui, "water_shore_brush", "Shore brush", "", |ui| {
                segmented(
                    ui,
                    &mut s.ed.water_tools.shape_mode,
                    &[
                        (ShapeMode::Expand, "Expand"),
                        (ShapeMode::Shrink, "Shrink"),
                        (ShapeMode::Smooth, "Smooth"),
                    ],
                );
                hint_small(
                    ui,
                    "Brush size and strength are in the dock below. Works on lakes made with Draw Lake. Undo restores the old shore and ground.",
                );
            });
        }
        WaterTool::Lake => {
            section_fold(ui, "water_new_basin", "New basin lake", "", |ui| {
                slider_row(ui, "Fill depth", &mut s.ed.lake_fill, 0.5..=40.0, |v| {
                    format!("{v:.1} m")
                });
                hint_small(ui, "Its water is picked in the inspector on the right.");
            });
        }
        WaterTool::Waterfall => {
            section_fold(ui, "water_new_fall", "New waterfall", "", |ui| {
                let w = &mut s.ed.water_tools;
                slider_row(ui, "Width", &mut w.fall_width, 1.5..=40.0, |v| {
                    format!("{v:.1} m")
                });
                slider_row(ui, "Flow", &mut w.fall_flow, 0.2..=3.0, |v| match v {
                    v if v < 0.6 => "trickle".into(),
                    v if v < 1.5 => "steady".into(),
                    v if v < 2.3 => "strong".into(),
                    _ => "torrent".into(),
                });
                slider_row(ui, "Mist", &mut w.fall_mist, 0.0..=1.0, pct);
                toggle(ui, &mut w.fall_pool, "Carve a plunge pool at the foot");
                hint_small(
                    ui,
                    "Spilling from a river, the fall takes the river's water.",
                );
            });
        }
        WaterTool::Fish => {
            // which water they live in: everything, or what suits a river / lake or the sea
            let filter_id = egui::Id::new("fish_water_filter");
            let mut filter: u8 = ui.data(|d| d.get_temp(filter_id)).unwrap_or(0);
            let list: Vec<FishSpecies> = FishSpecies::ALL
                .into_iter()
                .filter(|f| filter == 0 || f.info().water.suits(filter == 1))
                .collect();
            section_fold(
                ui,
                "water_fish_species",
                "Species",
                &list.len().to_string(),
                |ui| {
                    if segmented(
                        ui,
                        &mut filter,
                        &[(0, "All"), (1, "Fresh water"), (2, "Sea")],
                    ) {
                        ui.data_mut(|d| d.insert_temp(filter_id, filter));
                    }
                    ui.add_space(4.0);
                    let list: Vec<FishSpecies> = FishSpecies::ALL
                        .into_iter()
                        .filter(|f| filter == 0 || f.info().water.suits(filter == 1))
                        .collect();
                    let cur = s.ed.fish_species;
                    let previews = &mut s.previews;
                    if let Some(i) = card_grid(ui, 3, list.len(), |ui, i, w| {
                        let sp = list[i];
                        crate::thumbs::ui::tile(
                            ui,
                            previews,
                            &Subject::Fish(sp),
                            "Anchor",
                            sp.name(),
                            cur == sp,
                            w,
                        )
                        .on_hover_text(sp.about())
                    }) {
                        s.ed.fish_species = list[i];
                    }
                },
            );
            section_fold(ui, "water_fish_school", "School", "", |ui| {
                let mut c = s.ed.fish_count as f32;
                slider_row(ui, "School size", &mut c, 1.0..=60.0, |v| {
                    format!("{v:.0} fish")
                });
                s.ed.fish_count = c.round() as u32;
                slider_row(ui, "Swim area", &mut s.ed.fish_radius, 3.0..=80.0, meters);
            });
        }
        WaterTool::Select | WaterTool::Remove => {}
    }
    ui.add_space(2.0);
    hint_small(
        ui,
        &format!(
            "{} rivers · {} lakes · {} fish schools",
            s.water.rivers.len(),
            s.water.lakes.len(),
            s.water.fish.len()
        ),
    );
}

// ------------------------------------------------------------------ inspector (design)

/// The behaviour of a water style as a dropdown (Whirlpool is off on the open sea).
fn behaviour_row(ui: &mut egui::Ui, st: &mut WaterStyle, body: BodyKind) {
    let cur = st.behaviour;
    let mut pick = None;
    dropdown_row(
        ui,
        "Behaviour",
        ("water_behaviour", body as u8),
        cur.name(),
        |ui| {
            for b in WaterBehaviour::ALL {
                let off = b == WaterBehaviour::Whirlpool && body == BodyKind::Ocean;
                let r = ui
                    .add_enabled_ui(!off, |ui| {
                        menu_row(ui, behaviour_icon(b), b.name(), "", cur == b)
                    })
                    .inner;
                let r = if off {
                    r.on_disabled_hover_text("Whirlpools swirl in lakes and rivers")
                } else {
                    r.on_hover_text(b.hint())
                };
                if r.clicked() && !off && cur != b {
                    pick = Some(b);
                }
            }
        },
    )
    .on_hover_text(cur.hint());
    if let Some(b) = pick {
        st.set_behaviour(b, body);
    }
}

/// Every control of a water style, in sections. Returns true when anything changed.
fn style_controls(ui: &mut egui::Ui, st: &mut WaterStyle, body: BodyKind) -> bool {
    let before = st.clone();
    section_fold(ui, "water_style_colour", "Colour", "", |ui| {
        color_row(ui, "Water colour", &mut st.shallow);
        color_row(ui, "Depth colour", &mut st.deep);
        slider_row(ui, "Transparency", &mut st.transparency, 0.0..=1.0, pct);
        slider_row_log(
            ui,
            "Underwater visibility",
            &mut st.clarity,
            0.3..=30.0,
            |v| format!("{v:.1} m"),
        );
    });
    section_fold(ui, "water_style_waves", "Waves & flow", "", |ui| {
        behaviour_row(ui, st, body);
        let wave_label = if body == BodyKind::Ocean {
            let st2 = crate::ocean::SeaState::at((st.waves * 1.6).clamp(0.0, 4.0));
            format!("{:.1} m", st2.hs())
        } else {
            format!("{:.2}", st.waves)
        };
        slider_row(ui, "Wave height", &mut st.waves, 0.0..=2.5, move |_| {
            wave_label.clone()
        });
        slider_row(ui, "Wave speed", &mut st.wave_speed, 0.2..=2.5, |v| {
            format!("{v:.2}×")
        });
        if body == BodyKind::River {
            slider_row(ui, "Flow speed", &mut st.flow, 0.0..=4.0, |v| {
                format!("{:.1} m/s", v * 1.6)
            });
            hint_small(ui, "Rivers flow downhill along their course.");
        } else {
            slider_row(ui, "Flow speed", &mut st.flow, 0.0..=2.0, |v| {
                if v < 0.01 {
                    "none".into()
                } else {
                    format!("{:.1} m/s", v * 1.6)
                }
            });
            slider_row(
                ui,
                "Flow direction",
                &mut st.flow_dir,
                -180.0..=180.0,
                |v| {
                    let dirs = ["E", "SE", "S", "SW", "W", "NW", "N", "NE"];
                    let k = (((v.rem_euclid(360.0)) + 22.5) / 45.0) as usize % 8;
                    format!("{v:.0}° {}", dirs[k])
                },
            );
        }
    });
    section_fold(ui, "water_style_surface", "Surface", "", |ui| {
        slider_row(ui, "Surface roughness", &mut st.roughness, 0.0..=1.0, pct);
        labelled(ui, "Reflections", |ui| {
            segmented(
                ui,
                &mut st.reflections,
                &ReflectionQuality::ALL.map(|q| (q, q.name())),
            )
        });
        ui.add_space(2.0);
        slider_row(ui, "Metallic", &mut st.metallic, 0.0..=1.0, pct);
        slider_row(ui, "Iridescence", &mut st.iridescence, 0.0..=1.0, pct);
    });
    section_fold(ui, "water_style_foam", "Foam", "", |ui| {
        slider_row(ui, "Foam amount", &mut st.foam, 0.0..=1.0, pct);
        slider_row(ui, "Shoreline foam", &mut st.shore_foam, 0.0..=1.5, pct);
    });
    section_fold(ui, "water_style_glow", "Glow", "", |ui| {
        color_row(ui, "Bioluminescence colour", &mut st.glow);
        slider_row(
            ui,
            "Bioluminescence",
            &mut st.glow_strength,
            0.0..=2.0,
            |v| if v < 0.01 { "off".into() } else { pct(v) },
        );
        hint_small(ui, "Crests, foam and sparkles light up after dusk.");
        color_row(ui, "Inner glow colour", &mut st.emissive);
        slider_row(
            ui,
            "Inner glow",
            &mut st.emissive_strength,
            0.0..=1.5,
            |v| if v < 0.01 { "off".into() } else { pct(v) },
        );
    });

    let changed = *st != before;
    if changed && before.preset == st.preset && st.preset != "custom" {
        // edited away from the preset
        if presets::preset(&st.preset).is_none_or(|p| p.style != *st) {
            st.preset = "custom".into();
        }
    }
    changed
}

/// Apply a preset to a style, keeping what belongs to the body rather than the look.
fn apply_preset(st: &mut WaterStyle, p: &WaterPreset, body: BodyKind) {
    let flow_dir = st.flow_dir;
    let old_flow = st.flow;
    *st = p.style.clone();
    st.flow_dir = flow_dir;
    if body == BodyKind::River {
        // a river keeps moving even with a lake or sea look
        st.flow = if p.category == WaterCategory::River {
            p.style.flow
        } else {
            old_flow.max(0.5)
        };
    }
}

fn storm_weather(s: &mut UiState, before: WaterBehaviour, after: WaterBehaviour) {
    use crate::world::Weather;
    if before == after {
        return;
    }
    if after == WaterBehaviour::Storm && s.lighting.weather_override.is_none() {
        s.lighting.weather_override = Some(Weather::Storm);
        s.ed.toast("Storm: the weather turns stormy too (Weather tab to change it)");
    } else if before == WaterBehaviour::Storm && s.lighting.weather_override == Some(Weather::Storm)
    {
        s.lighting.weather_override = None;
    }
}

/// The inspector while a drawing tool is active: the water the next river, lake or fall gets.
fn new_water_right(ui: &mut egui::Ui, s: &mut UiState, tool: WaterTool) {
    use WaterCategory::*;
    let (blurb, cats, body): (&str, &[WaterCategory], BodyKind) = match tool {
        WaterTool::River => (
            "Rivers you draw next get this water.",
            &[River, Swamp, Fantasy],
            BodyKind::River,
        ),
        WaterTool::Waterfall => (
            "Falls you paint next get this water (a fall from a river takes the river's).",
            &[River, Fantasy],
            BodyKind::River,
        ),
        _ => (
            "Lakes you fill next get this water.",
            &[Lake, Swamp, Fantasy],
            BodyKind::Lake,
        ),
    };
    dim(
        ui,
        &format!("{blurb} To restyle existing water, use Select & Edit and click it."),
    );
    let mut tab = s.ed.water_tools.new_tab;
    if !cats.contains(&tab) {
        tab = cats[0];
    }
    let st = match tool {
        WaterTool::River => &mut s.ed.new_river_style,
        WaterTool::Waterfall => &mut s.ed.water_tools.fall_style,
        _ => &mut s.ed.new_lake_style,
    };
    stat_row(ui, "Water", st.preset_name());
    let cur = st.preset.clone();
    section_fold(
        ui,
        "water_presets",
        "Water presets",
        &preset_count(cats).to_string(),
        |ui| {
            if let Some(p) = preset_picker(ui, &mut tab, &cur, cats) {
                apply_preset(st, p, body);
            }
        },
    );
    style_controls(ui, st, body);
    s.ed.water_tools.new_tab = tab;
}

/// A fish species turning in the studio, with what it is and where it lives.
fn fish_card(ui: &mut egui::Ui, previews: &mut crate::thumbs::Previews, sp: FishSpecies) {
    let i = sp.info();
    let mut lines = vec![
        format!("{} · {}", i.look, i.rarity),
        format!("{} · about {:.1} m long", i.habitat, i.length),
    ];
    if let Some(e) = i.emberfall {
        lines.push(format!("From Emberfall: {e}"));
    }
    crate::thumbs::ui::card(ui, previews, &Subject::Fish(sp), i.name, &lines);
}

pub fn water_right(ui: &mut egui::Ui, s: &mut UiState) {
    let tool = s.ed.water_tool;
    if matches!(
        tool,
        WaterTool::River | WaterTool::Lake | WaterTool::Waterfall
    ) {
        new_water_right(ui, s, tool);
        return;
    }
    let sel = s.water.selected.unwrap_or(WaterSel::Ocean);
    if tool == WaterTool::Fish && !matches!(sel, WaterSel::Fish(_)) {
        // the species the next click releases
        fish_card(ui, &mut s.previews, s.ed.fish_species);
        ui.add_space(4.0);
        hint_small(
            ui,
            "Click on water to release a school. Fish swim at the depth they like, bottom dwellers along the floor; each school is exported as a fishing spot.",
        );
        return;
    }
    let mut remove = false;
    match sel {
        WaterSel::Fish(id) => {
            let Some(f) = s.water.fish.iter_mut().find(|f| f.id == id) else {
                s.water.selected = None;
                return;
            };
            fish_card(ui, &mut s.previews, f.species);
            ui.add_space(4.0);
            labelled(ui, "Species", |ui| {
                egui::ComboBox::from_id_salt("school_species")
                    .width(ui.available_width())
                    .selected_text(f.species.name())
                    .height(420.0)
                    .dropdown_ui(ui, |ui| {
                        for sp in FishSpecies::ALL {
                            menu_option(ui, &mut f.species, sp, sp.name())
                                .on_hover_text(sp.about());
                        }
                    });
            });
            let mut c = f.count as f32;
            slider_row(ui, "Fish", &mut c, 1.0..=60.0, |v| format!("{v:.0}"));
            f.count = c.round() as u32;
            slider_row(ui, "Swim area", &mut f.radius, 1.5..=80.0, meters);
            hint_small(
                ui,
                "Exported as a fishing spot (species, position, radius).",
            );
            ui.add_space(4.0);
            remove = danger_button(ui, "TrashCan", "Remove school").clicked();
        }
        WaterSel::Ocean => {
            hint_small(
                ui,
                &format!("The open sea · {}", s.water_style.preset_name()),
            );
            slider_row(
                ui,
                "Sea level",
                &mut s.terrain.sea_level,
                -50.0..=50.0,
                |v| format!("{v:.1} m"),
            );
            let level = (s.water_style.waves * 1.6).clamp(0.0, 4.0);
            let states = &crate::ocean::SEA_STATES;
            let cur = (0..states.len()).find(|i| (level - *i as f32).abs() < 0.5);
            let mut pick = None;
            dropdown_row(
                ui,
                "Sea state",
                "ocean_sea_state",
                cur.map_or("Custom", |i| states[i].0),
                |ui| {
                    for (i, (name, _)) in states.iter().enumerate() {
                        if menu_check(ui, cur == Some(i), *name).clicked() {
                            pick = Some(i);
                        }
                    }
                },
            );
            if let Some(i) = pick {
                s.water_style.waves = i as f32 / 1.6;
                s.meta.dirty = true;
            }
            let mut ang = s.wind.dir.y.atan2(s.wind.dir.x).to_degrees();
            if slider_row(ui, "Wind direction", &mut ang, -180.0..=180.0, |v| {
                format!("{v:.0}°")
            })
            .changed()
            {
                let a = ang.to_radians();
                s.wind.dir = Vec2::new(a.cos(), a.sin());
            }
            let cur = s.water_style.preset.clone();
            let mut tab = s.ed.water_tools.preset_tab;
            let picked = section_fold(
                ui,
                "water_presets",
                "Water presets",
                &preset_count(&WaterCategory::ALL).to_string(),
                |ui| preset_picker(ui, &mut tab, &cur, &WaterCategory::ALL),
            )
            .flatten();
            s.ed.water_tools.preset_tab = tab;
            let before = s.water_style.behaviour;
            if let Some(p) = picked {
                apply_preset(&mut s.water_style, p, BodyKind::Ocean);
                // whirlpools need a centre: the open sea swells instead
                if s.water_style.behaviour == WaterBehaviour::Whirlpool {
                    s.water_style.behaviour = WaterBehaviour::OceanSwells;
                }
                s.meta.dirty = true;
                s.ed.toast(format!("Ocean: {}", p.name));
            }
            let mut st = s.water_style.clone();
            if style_controls(ui, &mut st, BodyKind::Ocean) {
                *s.water_style = st;
                s.meta.dirty = true;
            }
            let after = s.water_style.behaviour;
            storm_weather(s, before, after);
            ui.add_space(2.0);
            hint_small(
                ui,
                "A Tessendorf FFT ocean: wind sea + swell in three cascades; wave height picks the sea state, wave speed runs its clock.",
            );
        }
        WaterSel::River(_) | WaterSel::Lake(_) | WaterSel::Waterfall(_) => {
            let body = match sel {
                WaterSel::River(_) | WaterSel::Waterfall(_) => BodyKind::River,
                _ => BodyKind::Lake,
            };
            // what it is, its name and the body's own controls
            match sel {
                WaterSel::River(id) => {
                    let Some(r) = s.water.rivers.iter_mut().find(|r| r.id == id) else {
                        s.water.selected = None;
                        return;
                    };
                    hint_small(
                        ui,
                        &format!(
                            "River · {:.0} m wide · {:.1} m deep · {}",
                            r.width,
                            r.depth,
                            r.style.preset_name()
                        ),
                    );
                    text_field(ui, "Name", &mut r.name, "River name");
                }
                WaterSel::Waterfall(id) => {
                    let Some(f) = s.water.waterfalls.iter_mut().find(|f| f.id == id) else {
                        s.water.selected = None;
                        return;
                    };
                    hint_small(
                        ui,
                        &format!(
                            "Waterfall · {:.0} m drop · {}",
                            f.drop(),
                            f.style.preset_name()
                        ),
                    );
                    text_field(ui, "Name", &mut f.name, "Waterfall name");
                    ui.add_space(4.0);
                    let (w0, m0) = (f.width, f.mist);
                    slider_row(ui, "Width", &mut f.width, 1.5..=40.0, |v| {
                        format!("{v:.1} m")
                    });
                    slider_row(ui, "Flow", &mut f.flow, 0.2..=3.0, |v| format!("{v:.1}"));
                    slider_row(ui, "Mist", &mut f.mist, 0.0..=1.0, pct);
                    if f.width != w0 || (f.mist - m0).abs() > 0.02 || (f.mist > 0.01) != (m0 > 0.01)
                    {
                        s.water.dirty = true;
                        s.meta.dirty = true;
                    }
                }
                WaterSel::Lake(id) => {
                    let Some(l) = s.water.lakes.iter_mut().find(|l| l.id == id) else {
                        s.water.selected = None;
                        return;
                    };
                    if l.is_drawn() {
                        let shape = lakes::shape_by_id(&l.shape);
                        hint_small(
                            ui,
                            &format!(
                                "{} · {:.1} m deep · {}",
                                shape.name,
                                l.depth,
                                l.style.preset_name()
                            ),
                        );
                    } else {
                        hint_small(ui, &format!("Basin lake · {}", l.style.preset_name()));
                    }
                    text_field(ui, "Name", &mut l.name, "Lake name");
                    if !l.is_drawn() {
                        ui.add_space(2.0);
                        let mut level = l.level;
                        labelled(ui, "Water level", |ui| {
                            ui.add(egui::DragValue::new(&mut level).speed(0.1).suffix(" m"))
                        });
                        if (level - l.level).abs() > 1e-4 {
                            l.level = level;
                            s.water.dirty = true;
                        }
                    }
                    if s.water.lakes.iter().any(|l| l.id == id && l.is_drawn()) {
                        lake_shape_controls(ui, s, id);
                    }
                }
                _ => {}
            }
            let cur = s
                .water
                .style_mut(sel)
                .map(|st| st.preset.clone())
                .unwrap_or_default();
            let mut tab = s.ed.water_tools.preset_tab;
            let picked = section_fold(
                ui,
                "water_presets",
                "Water presets",
                &preset_count(&WaterCategory::ALL).to_string(),
                |ui| preset_picker(ui, &mut tab, &cur, &WaterCategory::ALL),
            )
            .flatten();
            if let Some(p) = picked {
                s.history.begin(format!("Water: {}", p.name));
                s.history.touch_water(&s.water);
                if let Some(st) = s.water.style_mut(sel) {
                    apply_preset(st, p, body);
                }
                s.history.commit(&s.store);
                s.meta.dirty = true;
            }
            s.ed.water_tools.preset_tab = tab;
            if let Some(st) = s.water.style_mut(sel)
                && style_controls(ui, st, body)
            {
                s.meta.dirty = true;
            }
            ui.add_space(6.0);
            let what = match sel {
                WaterSel::River(_) => "river",
                WaterSel::Waterfall(_) => "waterfall",
                _ => "lake",
            };
            remove = danger_button(ui, "TrashCan", &format!("Remove {what}")).clicked();
        }
    }
    if remove {
        s.history.begin("Remove water");
        s.history.touch_water(&s.water);
        match sel {
            WaterSel::River(id) => s.water.rivers.retain(|r| r.id != id),
            WaterSel::Lake(id) => s.water.lakes.retain(|l| l.id != id),
            WaterSel::Fish(id) => s.water.fish.retain(|f| f.id != id),
            WaterSel::Waterfall(id) => s.water.waterfalls.retain(|f| f.id != id),
            WaterSel::Ocean => {}
        }
        s.water.selected = None;
        s.water.dirty = true;
        s.history.commit(&s.store);
    }
}

/// A drawn lake's type, depth and plants (re-carved when a slider is released).
fn lake_shape_controls(ui: &mut egui::Ui, s: &mut UiState, id: u32) {
    let Some(l) = s.water.lakes.iter().find(|l| l.id == id) else {
        return;
    };
    let prev = l.clone();
    let shape = lakes::shape_by_id(&l.shape);
    // slider values live in egui memory while dragging: the lake keeps what was carved until release
    let (kd, kp) = (
        egui::Id::new(("lake_depth_edit", id)),
        egui::Id::new(("lake_plants_edit", id)),
    );
    let mut depth = ui.data(|d| d.get_temp::<f32>(kd)).unwrap_or(l.depth);
    let mut plants = ui.data(|d| d.get_temp::<f32>(kp)).unwrap_or(l.plants);
    let mut new_shape = None;
    ui.add_space(4.0);
    fold_closed(
        ui,
        "water_lake_shape",
        "Lake type & shore",
        shape.name,
        |ui| {
            if let Some(i) = lake_picker(ui, shape.id) {
                new_shape = Some(i);
            }
            hint_small(
                ui,
                "Changing the type re-carves the basin and shore and switches to its water.",
            );
        },
    );
    let rd = slider_row(ui, "Depth", &mut depth, 0.3..=30.0, |v| format!("{v:.1} m"));
    let rp = slider_row(ui, "Shore plants", &mut plants, 0.0..=2.0, |v| {
        if v < 0.01 { "none".into() } else { pct(v) }
    });
    let done = |r: &egui::Response| r.drag_stopped() || (r.changed() && !r.dragged());
    let release = done(&rd) || done(&rp);
    if rd.dragged() {
        ui.data_mut(|d| d.insert_temp(kd, depth));
    }
    if rp.dragged() {
        ui.data_mut(|d| d.insert_temp(kp, plants));
    }
    if release || new_shape.is_some() {
        ui.data_mut(|d| {
            d.remove::<f32>(kd);
            d.remove::<f32>(kp);
        });
        s.history.begin("Reshape lake");
        s.history.touch_water(&s.water);
        if let Some(l) = s.water.lakes.iter_mut().find(|l| l.id == id) {
            l.depth = depth;
            l.plants = plants;
            if let Some(i) = new_shape {
                let sh = &lakes::SHAPES[i];
                l.shape = sh.id.to_string();
                l.depth = sh.depth;
                l.style = presets::preset_style(sh.water);
            }
        }
        let lib = s.lib.as_deref();
        crate::water::recarve_lake(
            &mut s.terrain,
            &mut s.history,
            &mut s.store,
            lib,
            &mut s.water,
            id,
            &prev,
        );
        s.history.commit(&s.store);
        s.meta.dirty = true;
    }
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The inspector's title: the Water tab always has something to design — the selected body, the
/// open sea, the next school, or the water the drawing tool makes next.
pub(super) fn water_inspector(s: &UiState) -> Option<(String, bool)> {
    let named = |name: Option<&String>, kind: &str| match name {
        Some(n) if !n.trim().is_empty() => Some(n.clone()),
        Some(_) => Some(kind.to_string()),
        None => None,
    };
    let title = match s.ed.water_tool {
        WaterTool::River => Some("New river water".to_string()),
        WaterTool::Waterfall => Some("New waterfall water".to_string()),
        WaterTool::Lake => Some("New basin lake water".to_string()),
        tool => match s.water.selected.unwrap_or(WaterSel::Ocean) {
            WaterSel::Fish(id) if s.water.fish.iter().any(|f| f.id == id) => {
                Some("Fish school".to_string())
            }
            _ if tool == WaterTool::Fish => Some("New school".to_string()),
            WaterSel::River(id) => named(
                s.water.rivers.iter().find(|r| r.id == id).map(|r| &r.name),
                "River",
            ),
            WaterSel::Lake(id) => named(
                s.water.lakes.iter().find(|l| l.id == id).map(|l| &l.name),
                "Lake",
            ),
            WaterSel::Waterfall(id) => named(
                s.water
                    .waterfalls
                    .iter()
                    .find(|f| f.id == id)
                    .map(|f| &f.name),
                "Waterfall",
            ),
            _ => None,
        },
    };
    Some((title.unwrap_or_else(|| "Ocean".to_string()), true))
}

/// The tool dock: the tool, the water it makes (or the shore brush), the key hints.
pub(super) fn water_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    let t = s.ed.water_tool;
    let (_, icon, name) = WATER_TOOLS
        .iter()
        .find(|w| w.0 == t)
        .copied()
        .unwrap_or(WATER_TOOLS[0]);
    dock_title(ui, icon, name);
    dock_sep(ui);
    match t {
        WaterTool::River => {
            dock_text(
                ui,
                &format!("Water: {}", s.ed.new_river_style.preset_name()),
            );
            dock_sep(ui);
            dock_hints(ui, &[("Drag", "Draw its course")]);
        }
        WaterTool::DrawLake => {
            let sh = &lakes::SHAPES[s.ed.water_tools.lake_shape.min(lakes::SHAPES.len() - 1)];
            dock_text(
                ui,
                &format!(
                    "{} · Water: {}",
                    sh.name,
                    presets::preset(sh.water).map(|p| p.name).unwrap_or("")
                ),
            );
            dock_sep(ui);
            dock_hints(ui, &[("Drag", "Draw its shore")]);
        }
        WaterTool::ShapeLake => {
            let b = &mut s.ed.water_tools.brush;
            slider_mini_log(ui, "Size", &mut b.size, 4.0..=200.0, meters);
            slider_mini(ui, "Strength", &mut b.strength, 0.05..=1.0, |v| {
                format!("{v:.2}")
            });
            dock_sep(ui);
            dock_hints(
                ui,
                &[
                    ("Click", "Brush"),
                    ("⌥", "Swap expand / shrink"),
                    ("[ ]", "Size"),
                ],
            );
        }
        WaterTool::Lake => {
            dock_text(ui, &format!("Water: {}", s.ed.new_lake_style.preset_name()));
            dock_sep(ui);
            dock_hints(ui, &[("Click", "A hollow")]);
        }
        WaterTool::Waterfall => {
            dock_text(
                ui,
                &format!("Water: {}", s.ed.water_tools.fall_style.preset_name()),
            );
            dock_sep(ui);
            dock_hints(ui, &[("Drag", "Down a slope")]);
        }
        WaterTool::Fish => {
            dock_text(
                ui,
                &format!("{} · {} fish", s.ed.fish_species.name(), s.ed.fish_count),
            );
            dock_sep(ui);
            dock_hints(ui, &[("Click", "Water")]);
        }
        WaterTool::Select => dock_hints(
            ui,
            &[("Click", "Water to design it"), ("Click sea", "The ocean")],
        ),
        WaterTool::Remove => dock_hints(ui, &[("Click", "Water to remove it")]),
    }
    true
}
