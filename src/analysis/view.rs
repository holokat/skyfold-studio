//! What the Analyze tab draws in the viewport besides the ground overlay: map markers (travel
//! start, farthest point, numbered gaps, level-jump flags with arrows, unreachable items, doctor
//! issues), a ring around the list row under the cursor, and the hover read-out next to the
//! cursor ("1 m 42 s from the player start · walkable · 12°").

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, Stroke, pos2, vec2,
};

use super::doctor::Severity;
use super::grid::Class;
use super::travel::fmt_time;
use super::{Analysis, View};
use crate::camera::{MainCamera, Pointer};
use crate::editor::{Editor, Tab};
use crate::ui::theme::{DANGER, TEXT, TEXT_DIM, WARN, semibold};

pub fn severity_colour(s: Severity) -> Color32 {
    match s {
        Severity::Error => DANGER,
        Severity::Warning => WARN,
        Severity::Info => TEXT,
    }
}

struct Painter<'a> {
    p: egui::Painter,
    cam: &'a Camera,
    cam_t: &'a GlobalTransform,
    screen: egui::Rect,
    /// The editor's panels: markers under them are skipped.
    panels: Vec<egui::Rect>,
}

impl Painter<'_> {
    fn project(&self, w: Vec3) -> Option<egui::Pos2> {
        let v = self.cam.world_to_viewport(self.cam_t, w).ok()?;
        let p = pos2(v.x, v.y);
        (self.screen.expand(40.0).contains(p)
            && !self.panels.iter().any(|r| r.expand(6.0).contains(p)))
        .then_some(p)
    }
    /// A round badge with an icon or number, and an optional caption to its right.
    fn badge(&self, at: egui::Pos2, fill: Color32, text: &str, caption: Option<&str>) {
        // a filled colour disc with a white rim; the icon (a Central icon name, else text) in white
        self.p
            .circle_filled(at + vec2(0.0, 2.0), 12.5, Color32::from_black_alpha(90));
        self.p.circle_filled(at, 11.0, fill);
        self.p
            .circle_stroke(at, 11.0, Stroke::new(1.5, Color32::WHITE));
        if crate::ui::icons::has(text) {
            crate::ui::icons::paint(&self.p, at, text, 13.0, Color32::WHITE);
        } else {
            self.p.text(
                at,
                Align2::CENTER_CENTER,
                text,
                FontId::new(12.5, semibold()),
                Color32::WHITE,
            );
        }
        if let Some(c) = caption {
            // a dark glass pill with a 1 px light rim (as the gameplay labels)
            let g =
                self.p
                    .layout_no_wrap(c.to_string(), FontId::new(12.5, semibold()), Color32::WHITE);
            let r = egui::Rect::from_min_size(
                at + vec2(17.0, -g.size().y * 0.5 - 4.5),
                g.size() + vec2(20.0, 9.0),
            );
            self.p.rect_filled(
                r,
                CornerRadius::same(9),
                Color32::from_rgba_unmultiplied(12, 15, 21, 204),
            );
            self.p.rect_stroke(
                r,
                CornerRadius::same(9),
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 31)),
                egui::StrokeKind::Inside,
            );
            self.p.galley(r.min + vec2(10.0, 4.5), g, Color32::WHITE);
        }
    }
    fn dot(&self, at: egui::Pos2, fill: Color32, r: f32) {
        self.p
            .circle_filled(at, r + 1.5, Color32::from_black_alpha(120));
        self.p.circle_filled(at, r, fill);
    }
    fn arrow(&self, a: egui::Pos2, b: egui::Pos2, col: Color32) {
        let d = b - a;
        if d.length() < 6.0 {
            return;
        }
        let dir = d.normalized();
        let side = vec2(-dir.y, dir.x);
        self.p.line_segment(
            [a, b - dir * 8.0],
            Stroke::new(5.0, Color32::from_black_alpha(110)),
        );
        self.p
            .line_segment([a, b - dir * 8.0], Stroke::new(3.0, col));
        self.p.add(egui::Shape::convex_polygon(
            vec![b, b - dir * 13.0 + side * 7.0, b - dir * 13.0 - side * 7.0],
            col,
            Stroke::new(1.0, Color32::from_black_alpha(140)),
        ));
    }
}

#[allow(clippy::too_many_arguments)]
pub fn viewport(
    mut contexts: EguiContexts,
    a: Res<Analysis>,
    ed: Res<Editor>,
    pointer: Res<Pointer>,
    walk: Res<crate::walk::WalkState>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    gameplay: Res<crate::gameplay::Gameplay>,
    zones: Res<crate::world::Zones>,
    stats: Res<crate::zones::ZoneStats>,
    time: Res<Time<Real>>,
) -> Result {
    if ed.tab != Tab::Analyze || walk.active {
        return Ok(());
    }
    let Ok((cam, cam_t)) = camera.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?.clone();
    let screen = ctx.content_rect();
    let panels = ["left_panel", "right_panel", "top_bar", "rail", "hints"]
        .iter()
        .filter_map(|id| ctx.memory(|m| m.area_rect(Id::new(*id))))
        .collect();
    let pt = Painter {
        p: ctx.layer_painter(LayerId::new(Order::Background, Id::new("analysis_markers"))),
        cam,
        cam_t,
        screen,
        panels,
    };
    let zone_name = |id: u8| {
        zones
            .by_id(id)
            .map(|z| format!("{} ({})", z.name, z.level_label()))
    };

    match a.view {
        View::Travel => {
            if let Some((_, t)) = &a.parts.travel {
                if let Some(f) = t.farthest.and_then(|p| pt.project(p)) {
                    pt.badge(
                        f,
                        Color32::from_rgb(150, 44, 170),
                        "Hourglass",
                        Some(&format!("Farthest: {}", fmt_time(t.max))),
                    );
                }
                if let Some(s) = t.source.and_then(|p| pt.project(p)) {
                    pt.badge(
                        s,
                        Color32::from_rgb(34, 140, 90),
                        "Flag1",
                        Some(&format!("Start: {}", t.label)),
                    );
                }
            }
        }
        View::Gaps => {
            if let Some((_, f)) = &a.parts.gapfield {
                if f.sources.len() <= 3000 {
                    for (p, _) in &f.sources {
                        let w = Vec3::new(
                            p.x,
                            a.grid()
                                .and_then(|g| g.cell_at(*p).map(|k| g.surface[k]))
                                .unwrap_or(0.0),
                            p.y,
                        );
                        if let Some(s) = pt.project(w) {
                            pt.dot(s, Color32::WHITE, 3.0);
                        }
                    }
                }
            }
            if let Some((_, g)) = &a.parts.gaps {
                for (i, r) in g.regions.iter().enumerate().take(8) {
                    if let Some(s) = pt.project(r.pos) {
                        let worst = if r.max_s.is_finite() {
                            fmt_time(r.max_s)
                        } else {
                            "no route".into()
                        };
                        pt.badge(
                            s,
                            Color32::from_rgb(200, 50, 50),
                            &(i + 1).to_string(),
                            Some(&format!("{:.2} km² · up to {worst}", r.area_km2)),
                        );
                    }
                }
            }
        }
        View::Flow => {
            // zone names and levels
            for z in &zones.list {
                let Some(p) = stats
                    .anchor
                    .get(&z.id)
                    .and_then(|w| pt.project(*w + Vec3::Y * 20.0))
                else {
                    continue;
                };
                let g1 = pt.p.layout_no_wrap(
                    z.name.clone(),
                    FontId::new(13.5, semibold()),
                    Color32::WHITE,
                );
                let g2 = pt.p.layout_no_wrap(
                    z.level_label(),
                    FontId::proportional(12.0),
                    Color32::from_rgb(220, 225, 235),
                );
                let r = egui::Rect::from_center_size(
                    p,
                    vec2(g1.size().x.max(g2.size().x) + 22.0, 42.0),
                );
                pt.p.rect_filled(
                    r,
                    CornerRadius::same(7),
                    Color32::from_rgba_unmultiplied(12, 14, 18, 200),
                );
                pt.p.rect_stroke(
                    r,
                    CornerRadius::same(7),
                    Stroke::new(1.5, crate::ui::widgets::color32(z.color)),
                    egui::StrokeKind::Inside,
                );
                pt.p.galley(
                    egui::pos2(r.center().x - g1.size().x * 0.5, r.top() + 4.0),
                    g1,
                    Color32::WHITE,
                );
                pt.p.galley(
                    egui::pos2(r.center().x - g2.size().x * 0.5, r.top() + 23.0),
                    g2,
                    Color32::WHITE,
                );
            }
            if let Some((_, f)) = &a.parts.flow {
                for b in f.borders.iter().filter(|b| b.flagged) {
                    for s in b.segments.iter().take(3).filter(|s| s.len_m >= 16.0) {
                        let d = Vec3::new(s.dir.x, 0.0, s.dir.y) * 90.0;
                        let (Some(p0), Some(p1), Some(c)) = (
                            pt.project(s.pos - d),
                            pt.project(s.pos + d),
                            pt.project(s.pos),
                        ) else {
                            continue;
                        };
                        pt.arrow(p0, p1, Color32::from_rgb(255, 70, 60));
                        pt.badge(
                            c,
                            Color32::from_rgb(220, 48, 48),
                            "ExclamationTriangle",
                            Some(&format!("+{:.0} levels · {:.0} m open", b.jump, s.len_m)),
                        );
                    }
                }
            }
        }
        View::Reach => {
            if let Some((_, r)) = &a.parts.reach {
                if let Some(o) = r.origin.and_then(|p| pt.project(p)) {
                    pt.badge(o, Color32::from_rgb(34, 140, 90), "Flag1", Some("Start"));
                }
                for (i, p) in r.pockets.iter().enumerate().take(6) {
                    if let Some(s) = pt.project(p.pos) {
                        pt.badge(
                            s,
                            Color32::from_rgb(220, 50, 150),
                            &(i + 1).to_string(),
                            Some(&format!("{:.3} km² unreachable", p.area_km2)),
                        );
                    }
                }
                for t in r.traps.iter().take(4) {
                    if let Some(s) = pt.project(t.pos) {
                        pt.badge(
                            s,
                            Color32::from_rgb(240, 140, 40),
                            "ArrowDown",
                            Some(&format!("Trap · {:.0} m²", t.area_km2 * 1.0e6)),
                        );
                    }
                }
                // unreachable items on top (they often sit inside a pocket)
                for it in &r.items {
                    if let Some(s) = pt.project(it.pos) {
                        pt.badge(
                            s + vec2(0.0, -26.0),
                            Color32::from_rgb(220, 48, 48),
                            "CrossSmall",
                            Some(&format!("{} '{}' can't be reached", it.kind, it.name)),
                        );
                    }
                }
            }
        }
        View::Doctor => {
            if let Some(rep) = a.report() {
                // pins for every issue; captions only near the cursor to keep the map readable
                let cursor = pointer.screen.map(|c| pos2(c.x, c.y));
                // errors drawn last, on top
                for i in rep.issues.iter().rev().filter(|i| !a.fixed.contains(&i.id)) {
                    let Some(s) = pt.project(i.pos) else { continue };
                    let near = cursor.is_some_and(|c| c.distance(s) < 16.0);
                    if near {
                        pt.badge(s, severity_colour(i.severity), "!", Some(&i.title));
                    } else {
                        pt.dot(s, severity_colour(i.severity), 5.0);
                    }
                }
            }
        }
        View::Walkability => {}
    }
    if let Some(h) = a.highlight
        && let Some(s) = pt.project(h)
    {
        let r = 20.0 + 4.0 * (time.elapsed_secs() * 4.0).sin();
        pt.p.circle_stroke(s, r, Stroke::new(3.0, Color32::from_black_alpha(120)));
        pt.p.circle_stroke(s, r, Stroke::new(2.0, Color32::WHITE));
    }

    // ---- progress chip (top centre) while the worker runs
    if let Some((p, stage)) = a.progress() {
        let painter = ctx.layer_painter(LayerId::new(
            Order::Foreground,
            Id::new("analysis_progress"),
        ));
        let c = pos2(
            screen.center().x + 20.0,
            screen.min.y + crate::ui::TOP_H + 26.0,
        );
        let r = egui::Rect::from_center_size(c, vec2(250.0, 30.0));
        painter.rect_filled(r, CornerRadius::same(15), crate::ui::theme::GLASS_SOLID);
        painter.rect_stroke(
            r,
            CornerRadius::same(15),
            Stroke::new(1.0, crate::ui::theme::LINE_STRONG),
            egui::StrokeKind::Inside,
        );
        let bar = egui::Rect::from_min_size(
            r.left_bottom() + vec2(10.0, -7.0),
            vec2((r.width() - 20.0) * p, 3.0),
        );
        painter.rect_filled(bar, CornerRadius::same(2), crate::ui::theme::ACCENT);
        painter.text(
            r.center() - vec2(0.0, 3.0),
            Align2::CENTER_CENTER,
            format!("Analysing: {stage}…  {:.0}%", p * 100.0),
            FontId::proportional(12.5),
            TEXT,
        );
    }

    // ---- hover read-out
    let (Some(hit), Some(cur), Some(g)) = (pointer.hit, pointer.screen, a.grid()) else {
        return Ok(());
    };
    if pointer.over_ui {
        return Ok(());
    }
    let Some(k) = g.cell_at(hit.xz()) else {
        return Ok(());
    };
    let class = g.class[k];
    let mut what = class.name().to_string();
    if class.land() && g.road[k] > 0 {
        what = format!(
            "on a {}",
            crate::roads::RoadKind::ALL[g.road[k] as usize - 1]
                .name()
                .to_lowercase()
        );
    }
    let ground = if matches!(class, Class::Swim | Class::OpenSea) {
        what.clone()
    } else {
        format!("{what} · {}° slope", g.slope[k])
    };
    let mut lines: Vec<(String, bool)> = Vec::new();
    match a.view {
        View::Walkability | View::Doctor => {
            lines.push((ground.clone(), true));
            if class == Class::Steep {
                lines.push((
                    match a.settings.steep {
                        super::SteepRule::SlideDown => {
                            "Players slide down here and can't walk up".into()
                        }
                        super::SteepRule::Climb => {
                            "Players climb here at a third of the speed".into()
                        }
                        super::SteepRule::Block => "Too steep: blocked".into(),
                    },
                    false,
                ));
            }
        }
        View::Travel => {
            if let Some((_, t)) = &a.parts.travel {
                let v = t.times[k];
                lines.push((
                    if v.is_finite() {
                        format!("{} from {}", fmt_time(v), t.label)
                    } else {
                        "Can't get here on foot".into()
                    },
                    true,
                ));
                lines.push((ground.clone(), false));
            }
        }
        View::Gaps => {
            if let (Some((_, f)), Some((_, gp))) = (&a.parts.gapfield, &a.parts.gaps) {
                let v = f.times[k];
                if gp.sources == 0 {
                    lines.push(("No content yet".into(), true));
                } else if !v.is_finite() {
                    lines.push(("No route to any content".into(), true));
                } else if v > gp.threshold {
                    lines.push((
                        format!("Dead zone: {} to the nearest content", fmt_time(v)),
                        true,
                    ));
                } else {
                    lines.push((format!("{} to the nearest content", fmt_time(v)), true));
                }
                lines.push((ground.clone(), false));
            }
        }
        View::Flow => {
            let z = g.zone[k];
            lines.push((zone_name(z).unwrap_or("No zone".into()), true));
            lines.push((ground.clone(), false));
        }
        View::Reach => {
            if let Some((_, r)) = &a.parts.reach {
                let msg = if r.origin.is_none() {
                    "Click the ground to choose a start point".to_string()
                } else if !class.land() {
                    ground.clone()
                } else {
                    match r.flags[k] {
                        3 => format!("Reachable from {}", r.origin_label),
                        1 => "Trap: players get in but can't walk out".into(),
                        _ => format!("Unreachable from {}", r.origin_label),
                    }
                };
                lines.push((msg, true));
                if r.origin.is_some() && class.land() {
                    lines.push((ground.clone(), false));
                }
            }
        }
    }
    if gameplay.player_start.is_none()
        && matches!(a.view, View::Travel | View::Reach)
        && a.parts.reach.as_ref().is_some_and(|r| r.1.origin.is_some())
    {
        lines.push((
            "No player start yet: measuring from your chosen point".into(),
            false,
        ));
    }
    if lines.is_empty() {
        return Ok(());
    }
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("analysis_hover")));
    let galleys: Vec<_> = lines
        .iter()
        .map(|(t, main)| {
            painter.layout_no_wrap(
                t.clone(),
                if *main {
                    FontId::new(13.0, semibold())
                } else {
                    FontId::proportional(11.5)
                },
                if *main { Color32::WHITE } else { TEXT_DIM },
            )
        })
        .collect();
    let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 18.0;
    let h = galleys.iter().map(|g| g.size().y + 2.0).sum::<f32>() + 12.0;
    let mut at = pos2(cur.x + 18.0, cur.y + 16.0);
    if at.x + w > screen.max.x - 8.0 {
        at.x = cur.x - w - 12.0;
    }
    let r = egui::Rect::from_min_size(at, vec2(w, h));
    painter.rect_filled(
        r,
        CornerRadius::same(8),
        Color32::from_rgba_unmultiplied(12, 15, 21, 225),
    );
    painter.rect_stroke(
        r,
        CornerRadius::same(8),
        Stroke::new(1.0, Color32::from_white_alpha(28)),
        egui::StrokeKind::Inside,
    );
    let mut y = r.min.y + 6.0;
    for g in galleys {
        let hgt = g.size().y;
        painter.galley(pos2(r.min.x + 9.0, y), g, Color32::WHITE);
        y += hgt + 2.0;
    }
    Ok(())
}
