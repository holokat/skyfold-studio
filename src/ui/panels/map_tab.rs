//! Map tab: generate and export the in-game world map and minimap. Left: style, resolution,
//! layers, title and Generate. Right: the preview (click for the large zoomable viewer), the
//! minimap as players will see it, and Export.

use bevy::math::Vec2;
use bevy_egui::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Stroke, pos2, vec2};

use super::super::theme::*;
use super::super::widgets::*;
use super::super::{TOP_H, UiState};
use crate::worldmap::minimap::{self, View};
use crate::worldmap::{MapStyle, SIZES};

/// A small picture of this world in a map style, painted from the heightmap (cached; repainted
/// every few seconds while the tab is open, so it follows the terrain).
fn style_preview(ctx: &egui::Context, s: &UiState, style: MapStyle) -> egui::TextureId {
    let k = MapStyle::ALL.iter().position(|m| *m == style).unwrap_or(0);
    let id = egui::Id::new(("map_style_preview", k));
    let key = (
        s.terrain.res as u32,
        s.terrain.size().to_bits(),
        (s.real_time.elapsed_secs() / 8.0) as u32,
    );
    if let Some((kk, h)) = ctx.data(|d| d.get_temp::<((u32, u32, u32), egui::TextureHandle)>(id))
        && kk == key
    {
        return h.id();
    }
    let (w, h) = (160usize, 100usize);
    let t = &*s.terrain;
    let half = t.half();
    let sea = t.sea_level;
    // a 16:10 window over the middle of the world
    let span_x = t.size();
    let span_z = span_x * h as f32 / w as f32;
    let at = |i: usize, j: usize| {
        let x = -half + (i as f32 + 0.5) / w as f32 * span_x;
        let z = -span_z * 0.5 + (j as f32 + 0.5) / h as f32 * span_z;
        t.height_at(x, z) - sea
    };
    let hs: Vec<f32> = (0..w * h).map(|n| at(n % w, n / w)).collect();
    let hmax = hs.iter().copied().fold(1.0f32, f32::max);
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let px: Vec<Color32> = (0..w * h)
        .map(|n| {
            let (i, j) = (n % w, n / w);
            let v = hs[n];
            let dx = hs[j * w + (i + 1).min(w - 1)] - hs[j * w + i.saturating_sub(1)];
            let dz = hs[(j + 1).min(h - 1) * w + i] - hs[j.saturating_sub(1) * w + i];
            let shade = (1.0 - (dx + dz) * 4.0 / (span_x / w as f32)).clamp(0.55, 1.35);
            let e = (v / hmax).clamp(0.0, 1.0);
            let c: [f32; 3] = match style {
                MapStyle::Parchment => {
                    if v < 0.0 {
                        mix(
                            [196.0, 184.0, 150.0],
                            [170.0, 160.0, 128.0],
                            (-v / 30.0).clamp(0.0, 1.0),
                        )
                    } else {
                        let c = mix([226.0, 206.0, 160.0], [170.0, 138.0, 92.0], e.powf(0.7));
                        [c[0] * shade, c[1] * shade, c[2] * shade]
                    }
                }
                MapStyle::Satellite => {
                    if v < 0.0 {
                        mix(
                            [46.0, 104.0, 120.0],
                            [14.0, 40.0, 66.0],
                            (-v / 25.0).clamp(0.0, 1.0),
                        )
                    } else {
                        let c = if e < 0.04 {
                            [196.0, 182.0, 140.0]
                        } else if e < 0.55 {
                            mix([78.0, 110.0, 54.0], [96.0, 92.0, 70.0], e / 0.55)
                        } else {
                            mix(
                                [110.0, 104.0, 96.0],
                                [236.0, 238.0, 242.0],
                                (e - 0.55) / 0.45,
                            )
                        };
                        [c[0] * shade, c[1] * shade, c[2] * shade]
                    }
                }
                MapStyle::Atlas => {
                    if v < 0.0 {
                        mix(
                            [170.0, 210.0, 230.0],
                            [110.0, 160.0, 200.0],
                            (-v / 30.0).clamp(0.0, 1.0),
                        )
                    } else {
                        let c = if e < 0.35 {
                            mix([120.0, 176.0, 110.0], [222.0, 214.0, 140.0], e / 0.35)
                        } else if e < 0.75 {
                            mix(
                                [222.0, 214.0, 140.0],
                                [168.0, 120.0, 80.0],
                                (e - 0.35) / 0.4,
                            )
                        } else {
                            mix(
                                [168.0, 120.0, 80.0],
                                [245.0, 245.0, 245.0],
                                (e - 0.75) / 0.25,
                            )
                        };
                        let contour = ((v / (hmax / 10.0).max(1.0)).fract() < 0.08) as u8 as f32;
                        let s2 = shade * (1.0 - 0.25 * contour);
                        [c[0] * s2, c[1] * s2, c[2] * s2]
                    }
                }
                MapStyle::Minimal => {
                    if v < 0.0 {
                        [58.0, 76.0, 96.0]
                    } else {
                        mix(
                            [96.0, 108.0, 92.0],
                            [150.0, 154.0, 146.0],
                            (e * 3.0).floor() / 3.0,
                        )
                    }
                }
            };
            Color32::from_rgb(
                c[0].clamp(0.0, 255.0) as u8,
                c[1].clamp(0.0, 255.0) as u8,
                c[2].clamp(0.0, 255.0) as u8,
            )
        })
        .collect();
    let tex = ctx.load_texture(
        format!("map_style_preview_{k}"),
        egui::ColorImage::new([w, h], px),
        egui::TextureOptions::LINEAR,
    );
    let tid = tex.id();
    ctx.data_mut(|d| d.insert_temp(id, (key, tex)));
    tid
}

/// A labelled one-line text field (label above, dark well).
fn text_field(ui: &mut egui::Ui, label: &str, text: &mut String, hint: &str) {
    ui.label(RichText::new(label).size(12.5).color(TEXT_DIM));
    egui::Frame::new()
        .fill(WELL)
        .corner_radius(CornerRadius::same(R_INPUT))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new(hint).color(TEXT_MUTE))
                    .desired_width(f32::INFINITY),
            );
        });
}

pub fn map_left(ui: &mut egui::Ui, s: &mut UiState) {
    let ctx = ui.ctx().clone();
    s.map.ensure_textures(&ctx);
    dim(
        ui,
        "Turn your world into a map for the game, the minimap and your store page.",
    );
    let busy = s.map.busy();
    ui.add_enabled_ui(!busy, |ui| {
        section_fold(ui, "map_style", "Style", "", |ui| {
            let cur = s.map.settings.style;
            let w = ((ui.available_width() - 6.0) / 2.0).floor();
            let pics: Vec<egui::TextureId> = MapStyle::ALL
                .iter()
                .map(|m| style_preview(&ctx, s, *m))
                .collect();
            let mut pick = None;
            for (r, row) in MapStyle::ALL.chunks(2).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    for (c, m) in row.iter().enumerate() {
                        if image_card(
                            ui,
                            Some(pics[r * 2 + c]),
                            m.name(),
                            "",
                            *m == cur,
                            vec2(w, (w * 0.62).round()),
                        )
                        .on_hover_text(m.blurb())
                        .clicked()
                        {
                            pick = Some(*m);
                        }
                    }
                });
            }
            if let Some(m) = pick {
                s.map.settings.style = m;
            }
            hint_small(ui, s.map.settings.style.blurb());
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Resolution").color(TEXT_DIM));
            let mut size = s.map.settings.size;
            segmented(
                ui,
                &mut size,
                &SIZES.map(|v| {
                    (
                        v,
                        match v {
                            2048 => "2048",
                            4096 => "4096",
                            _ => "8192",
                        },
                    )
                }),
            );
            s.map.settings.size = size;
        });
        let km = s.terrain.size() / 1000.0;
        hint_small(
            ui,
            &format!(
                "{:.2} m per pixel over {km:.1} km",
                s.terrain.size() / s.map.settings.size as f32
            ),
        );
        section_fold(ui, "map_show", "Show", "", |ui| {
            let l = &mut s.map.settings.layers;
            toggle(ui, &mut l.labels, "Names");
            toggle(ui, &mut l.zones, "Zones");
            toggle(ui, &mut l.roads, "Roads");
            toggle(ui, &mut l.pois, "Places");
            toggle(ui, &mut l.compass, "Compass & scale");
            toggle(ui, &mut l.grid, "Grid");
        });
        toggle(ui, &mut s.map.settings.show_title, "Title on the map");
        if s.map.settings.show_title {
            let hint = s.meta.name.clone();
            text_field(ui, "Title", &mut s.map.settings.title, &hint);
        }
        text_field(
            ui,
            "Sea name",
            &mut s.map.settings.sea_name,
            "e.g. The Sundering Sea",
        );
    });
    ui.add_space(4.0);
    if let Some((f, stage)) = s.map.progress() {
        ui.add(
            egui::ProgressBar::new(f)
                .text(RichText::new(stage).size(12.0).color(ON_ACCENT))
                .desired_height(22.0)
                .corner_radius(CornerRadius::same(R_INPUT))
                .fill(Color32::WHITE),
        );
        if s.map.capturing() {
            hint_small(
                ui,
                "Rendering the world from above, tile by tile. The view follows the tiles until it is done.",
            );
        }
        if wide_icon_button(ui, "CrossMedium", "Cancel").clicked() {
            s.map.cancel();
        }
    } else if primary_icon_button(ui, "Map", "Generate map").clicked() {
        s.map.generate_requested = true;
    }
    if s.walk.active {
        hint_small(ui, "Leave walk mode to generate.");
    }
    if let Some(msg) = &s.map.status {
        hint_small(ui, msg);
    }
}

pub fn map_right(ui: &mut egui::Ui, s: &mut UiState) {
    let ctx = ui.ctx().clone();
    s.map.ensure_textures(&ctx);
    let Some(r) = s.map.result.clone() else {
        hint_small(
            ui,
            "Choose a style and press Generate. The map is drawn from your terrain, water, forests, zones, roads and places.",
        );
        return;
    };
    let w = ui.available_width();
    if let Some(t) = &s.map.textures {
        let (rect, resp) = ui.allocate_exact_size(vec2(w, w), egui::Sense::click());
        let rad = CornerRadius::same(13);
        egui::Image::new((t.thumb.id(), rect.size()))
            .corner_radius(rad)
            .paint_at(ui, rect);
        let p = ui.painter();
        // caption over a bottom fade: the title, size and time
        let fade = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 46.0), rect.max);
        let mut m = egui::Mesh::default();
        let (c0, c1) = (Color32::TRANSPARENT, Color32::from_black_alpha(190));
        m.colored_vertex(fade.left_top(), c0);
        m.colored_vertex(fade.right_top(), c0);
        m.colored_vertex(fade.right_bottom(), c1);
        m.colored_vertex(fade.left_bottom(), c1);
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        p.add(egui::Shape::mesh(m));
        let title = if r.settings.title.is_empty() {
            s.meta.name.clone()
        } else {
            r.settings.title.clone()
        };
        p.text(
            pos2(rect.left() + 10.0, rect.bottom() - 25.0),
            egui::Align2::LEFT_BOTTOM,
            title,
            FontId::new(12.5, semibold()),
            Color32::WHITE,
        );
        p.text(
            pos2(rect.left() + 10.0, rect.bottom() - 9.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{} · {} px · {:.1} s",
                r.settings.style.name(),
                r.size,
                r.millis as f32 / 1000.0
            ),
            FontId::proportional(10.5),
            Color32::from_white_alpha(190),
        );
        p.rect_stroke(
            rect,
            rad,
            Stroke::new(
                if resp.hovered() { 1.5 } else { 1.0 },
                if resp.hovered() {
                    Color32::from_white_alpha(120)
                } else {
                    LINE
                },
            ),
            egui::StrokeKind::Inside,
        );
        if resp
            .on_hover_text("Open the large view")
            .on_hover_cursor(egui::CursorIcon::ZoomIn)
            .clicked()
        {
            s.map.viewer.open = true;
            s.map.viewer.fit = true;
        }
    }
    if s.map.stale {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            crate::ui::icons::icon(ui, "ExclamationTriangle", 14.0, WARN);
            ui.label(
                RichText::new("The world changed since this map was made")
                    .size(12.0)
                    .color(WARN),
            );
        });
    }
    if wide_icon_button(ui, "FullScreen", "Open large view")
        .on_hover_text("Scroll to zoom · drag to pan · double-click to fit · Esc closes")
        .clicked()
    {
        s.map.viewer.open = true;
        s.map.viewer.fit = true;
    }
    let exporting = s.map.exporting();
    if ui
        .add_enabled_ui(!exporting && !s.map.busy(), |ui| {
            primary_icon_button(ui, "ArrowOutOfBox", "Export for the game…")
        })
        .inner
        .clicked()
    {
        let default = crate::export::exports_dir();
        let _ = std::fs::create_dir_all(&default);
        crate::dialogs::pick_folder(
            "map_export",
            rfd::AsyncFileDialog::new()
                .set_title("Export map to folder")
                .set_directory(&default),
        );
    }
    if let Some(dir) = crate::dialogs::take_file("map_export") {
        s.map.export_request = Some(dir.join("map"));
    }
    hint_small(
        ui,
        "PNGs, minimap tiles (256 px, every zoom), fog-of-war mask and map.json (transform, labels, icons, zone outlines).",
    );
    section_fold(ui, "map_minimap", "Minimap", "", |ui| {
        if let Some(src) = s.map.minimap_source() {
            let (rect, _) = ui.allocate_exact_size(vec2(w, 150.0), egui::Sense::hover());
            let start = s
                .gameplay
                .player_start
                .map(|p| Vec2::new(p.pos[0], p.pos[2]))
                .unwrap_or(Vec2::new(s.orbit.focus.x, s.orbit.focus.z));
            let t = s.real_time.elapsed_secs();
            let markers = minimap::markers(&s.map, &s.gameplay);
            minimap::draw(
                ui.painter(),
                rect.center(),
                66.0,
                &src,
                &View {
                    player: start,
                    yaw: t * 0.25,
                    radius_m: 260.0,
                    rotate: true,
                },
                &markers,
            );
            ui.ctx().request_repaint();
        }
        hint_small(
            ui,
            "How players see it in walk mode (turns with them). Game code: worldmap::minimap::draw.",
        );
    });
    viewer(&ctx, s);
}

/// The large zoomable preview over the viewport (scroll to zoom, drag to pan, Esc to close).
fn viewer(ctx: &egui::Context, s: &mut UiState) {
    if !s.map.viewer.open {
        return;
    }
    let Some(r) = s.map.result.clone() else {
        return;
    };
    let screen = ctx.content_rect();
    let area = Rect::from_min_max(pos2(screen.min.x, screen.min.y + TOP_H), screen.max);
    let size = r.size as f32;
    let fit = (area.width().min(area.height()) - 60.0) / size;
    if s.map.viewer.fit {
        s.map.viewer.zoom = fit;
        s.map.viewer.centre = Vec2::splat(size * 0.5);
        s.map.viewer.fit = false;
    }
    let mut close = false;
    egui::Area::new(egui::Id::new("world_map_viewer"))
        .order(egui::Order::Foreground)
        .fixed_pos(area.min)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(area.size(), egui::Sense::click_and_drag());
            let p = ui.painter_at(rect);
            p.rect_filled(
                rect,
                CornerRadius::ZERO,
                Color32::from_rgba_unmultiplied(8, 10, 14, 236),
            );
            let v = &mut s.map.viewer;
            // zoom about the cursor
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
                if scroll.abs() > 0.0
                    && let Some(m) = ui.input(|i| i.pointer.hover_pos())
                {
                    let before =
                        v.centre + Vec2::new(m.x - rect.center().x, m.y - rect.center().y) / v.zoom;
                    v.zoom = (v.zoom * (scroll * 0.0025).exp()).clamp(fit * 0.5, 4.0);
                    v.centre =
                        before - Vec2::new(m.x - rect.center().x, m.y - rect.center().y) / v.zoom;
                }
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                v.centre -= Vec2::new(d.x, d.y) / v.zoom;
            }
            if resp.double_clicked() {
                v.fit = true;
            }
            let (zoom, centre) = (v.zoom, v.centre);
            let origin = rect.center() - vec2(centre.x, centre.y) * zoom;
            let map_rect = Rect::from_min_size(origin, vec2(size, size) * zoom);
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            // the overview when zoomed out, full-resolution tiles up close
            let tiles = (r.size / 2048).max(1) as usize;
            let shown = size * zoom;
            if tiles == 1 || shown < 2048.0 * 1.1 {
                if let Some(t) = &s.map.textures {
                    // the smallest copy that still has a texel per screen pixel
                    let tex = if shown <= 1100.0 { &t.fit } else { &t.overview };
                    p.image(tex.id(), map_rect, uv, Color32::WHITE);
                }
            } else {
                let tile = 2048.0 * zoom;
                for ty in 0..tiles {
                    for tx in 0..tiles {
                        let tr = Rect::from_min_size(
                            origin + vec2(tx as f32, ty as f32) * tile,
                            vec2(tile, tile),
                        );
                        if !tr.intersects(rect) {
                            continue;
                        }
                        if let Some(id) = s.map.tile_texture(ctx, ty * tiles + tx) {
                            p.image(id, tr, uv, Color32::WHITE);
                        }
                    }
                }
            }
            p.rect_stroke(
                map_rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, Color32::from_white_alpha(40)),
                egui::StrokeKind::Outside,
            );
            // toolbar
            let bar = Rect::from_min_size(
                pos2(rect.max.x - 300.0, rect.min.y + 14.0),
                vec2(286.0, 38.0),
            );
            p.rect_filled(bar, CornerRadius::same(R_MENU), GLASS_SOLID);
            p.rect_stroke(
                bar,
                CornerRadius::same(R_MENU),
                Stroke::new(1.0, LINE_STRONG),
                egui::StrokeKind::Inside,
            );
            let info = format!("{} · {:.0}%", r.settings.style.name(), zoom * 100.0);
            p.text(
                pos2(bar.min.x + 14.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                info,
                FontId::new(13.0, semibold()),
                TEXT,
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(
                pos2(bar.max.x - 104.0, bar.min.y + 5.0),
                vec2(100.0, 28.0),
            )));
            child.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, "Expand", "Fit (double-click)").clicked() {
                    s.map.viewer.fit = true;
                }
                if icon_button(ui, "ZoomIn", "Actual pixels").clicked() {
                    s.map.viewer.zoom = 1.0;
                }
                if icon_button(ui, "CrossMedium", "Close (Esc)").clicked() {
                    close = true;
                }
            });
            p.text(
                pos2(rect.center().x, rect.max.y - 22.0),
                egui::Align2::CENTER_CENTER,
                "Scroll to zoom · drag to pan · double-click to fit · Esc to close",
                FontId::proportional(12.5),
                TEXT_DIM,
            );
        });
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        s.map.viewer.open = false;
    }
}

// ------------------------------------------------------------------ Halo hooks (inspector, dock)

/// The preview inspector: while there's a map to preview.
pub(super) fn map_inspector(s: &UiState) -> Option<(String, bool)> {
    s.map.result.as_ref().map(|_| ("Preview".to_string(), true))
}

/// The dock: the map's style and size; Generate is in the panel.
pub(super) fn map_dock(ui: &mut egui::Ui, s: &mut UiState) -> bool {
    dock_title(ui, "CompassSquare", "World Map");
    dock_sep(ui);
    let st = &s.map.settings;
    dock_text(ui, &format!("{} · {} px", st.style.name(), st.size));
    dock_sep(ui);
    match s.map.progress() {
        Some((f, stage)) => dock_text(ui, &format!("{stage} {:.0}%", f * 100.0)),
        None => dock_hints(ui, &[("RMB", "Orbit"), ("Scroll", "Zoom")]),
    }
    true
}
