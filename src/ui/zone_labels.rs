//! Readable map labels: selected regions take priority and nearby labels never overlap.

use bevy::prelude::{Camera, GlobalTransform, Vec3};
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Order, Pos2, Rect, Stroke, Vec2, vec2,
};

use super::{UiState, project, shell, theme::semibold, widgets::color32};

/// Keep labels near their true ground anchor. If a close zoom cannot fit every label, retain
/// the selected/larger regions rather than drawing illegible text over another label.
fn place(anchor: Pos2, size: Vec2, viewport: Rect, occupied: &[Rect]) -> Option<Rect> {
    let directions = [
        vec2(0.0, 0.0),
        vec2(0.0, -1.0),
        vec2(0.0, 1.0),
        vec2(-1.0, 0.0),
        vec2(1.0, 0.0),
        vec2(-0.8, -0.6),
        vec2(0.8, -0.6),
        vec2(-0.8, 0.6),
        vec2(0.8, 0.6),
    ];
    for radius in [0.0, 32.0, 56.0, 88.0, 120.0] {
        for direction in directions {
            if radius > 0.0 && direction == Vec2::ZERO {
                continue;
            }
            let rect = Rect::from_center_size(anchor + direction * radius, size);
            if viewport.contains_rect(rect)
                && occupied
                    .iter()
                    .all(|other| !other.expand(5.0).intersects(rect))
            {
                return Some(rect);
            }
        }
    }
    None
}

pub(super) fn draw(
    ctx: &egui::Context,
    s: &UiState,
    cam: &Camera,
    cam_t: &GlobalTransform,
    inspector: bool,
) {
    if !s.ed.show_zone_labels
        || !matches!(
            s.ed.tab,
            crate::editor::Tab::Zones | crate::editor::Tab::Lighting
        )
    {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        Order::Background,
        Id::new("zone_labels"),
    ));
    let mut viewport = ctx.content_rect().shrink(10.0);
    viewport.min.y += shell::TOP;
    viewport.max.y -= 54.0;
    let shell = shell::get(ctx);
    viewport.min.x += if shell.panel_open {
        shell::PANEL_X + shell::PANEL_W
    } else {
        48.0
    };
    if inspector && shell.insp_open {
        viewport.max.x -= shell::INSP_W + shell::M;
    }

    let mut order: Vec<usize> = (0..s.zones.list.len()).collect();
    order.sort_by(|a, b| {
        (s.zones.selected == Some(*b))
            .cmp(&(s.zones.selected == Some(*a)))
            .then_with(|| {
                let area = |i: usize| {
                    s.stats
                        .area_km2
                        .get(&s.zones.list[i].id)
                        .copied()
                        .unwrap_or(0.0)
                };
                area(*b).total_cmp(&area(*a))
            })
            .then(a.cmp(b))
    });
    let mut occupied = Vec::with_capacity(order.len());
    for i in order {
        let zone = &s.zones.list[i];
        if !zone.visible {
            continue;
        }
        let Some(anchor) = s.stats.anchor.get(&zone.id) else {
            continue;
        };
        let Some(point) = project(cam, cam_t, *anchor + Vec3::Y * 20.0) else {
            continue;
        };
        if !ctx.content_rect().contains(point) {
            continue;
        }
        let title = format!("{}. {}", i + 1, zone.name);
        let sub = zone.level_label();
        let title_font = FontId::new(14.0, semibold());
        let level_font = FontId::proportional(12.0);
        let title_size = painter
            .layout_no_wrap(title.clone(), title_font.clone(), Color32::WHITE)
            .size();
        let level_size = painter
            .layout_no_wrap(sub.clone(), level_font.clone(), Color32::WHITE)
            .size();
        let size = vec2(
            title_size.x.max(level_size.x) + 22.0,
            if s.ed.show_level_ranges { 44.0 } else { 28.0 },
        );
        let Some(rect) = place(point, size, viewport, &occupied) else {
            continue;
        };
        occupied.push(rect);
        let color = color32(zone.color);
        if rect.center().distance(point) > 8.0 {
            painter.line_segment(
                [point, rect.clamp(point)],
                Stroke::new(1.0, color.gamma_multiply(0.75)),
            );
            painter.circle_filled(point, 2.5, color);
        }
        painter.rect_filled(
            rect,
            CornerRadius::same(7),
            Color32::from_rgba_unmultiplied(12, 14, 18, 215),
        );
        painter.rect_stroke(
            rect,
            CornerRadius::same(7),
            Stroke::new(1.5, color.gamma_multiply(0.85)),
            egui::StrokeKind::Inside,
        );
        if s.ed.show_level_ranges {
            painter.text(
                rect.center() - vec2(0.0, 8.0),
                Align2::CENTER_CENTER,
                title,
                title_font,
                Color32::WHITE,
            );
            painter.text(
                rect.center() + vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                sub,
                level_font,
                Color32::from_rgb(220, 225, 235),
            );
        } else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                title,
                title_font,
                Color32::WHITE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crowded_labels_keep_a_gap_and_stay_in_the_viewport() {
        let view = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(600.0, 400.0));
        let mut occupied = vec![];
        for _ in 0..12 {
            if let Some(rect) = place(egui::pos2(300.0, 200.0), vec2(160.0, 44.0), view, &occupied)
            {
                assert!(view.contains_rect(rect));
                assert!(
                    occupied
                        .iter()
                        .all(|r: &Rect| !r.expand(5.0).intersects(rect))
                );
                occupied.push(rect);
            }
        }
        assert!(occupied.len() >= 3);
        assert!(occupied.len() < 12);
    }
    #[test]
    fn isolated_label_keeps_its_exact_anchor() {
        let view = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(600.0, 400.0));
        let anchor = egui::pos2(300.0, 200.0);
        assert_eq!(
            place(anchor, vec2(160.0, 44.0), view, &[])
                .unwrap()
                .center(),
            anchor
        );
    }
}
