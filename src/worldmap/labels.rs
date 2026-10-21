//! Points of interest and lettering: one consistent icon set for the POI kinds, collision-free
//! label placement (regions first, then water, then places, then contour heights), and the map
//! data a game reads (`map.json`: transform, label anchors, icon positions, zone polygons).

use bevy::math::Vec2;
use serde_json::json;
use tiny_skia::{Path, PathBuilder};

use super::MapStyle;
use super::canvas::{
    Align, Bx, Canvas, Face, Fonts, Rgba, TextStyle, label_box, measure, poly_path, rgb, with_alpha,
};
use super::decor::Deco;
use super::features::{Drawn, ZoneLine, zone_polygons};
use super::render::Ctx;
use crate::gameplay::PoiKind;

/// Occupied areas: labels never overlap each other, icons or decorations.
pub struct Placer {
    boxes: Vec<Bx>,
    bounds: Bx,
}

impl Placer {
    pub fn free(&self, b: &Bx) -> bool {
        b.x0 >= self.bounds.x0
            && b.y0 >= self.bounds.y0
            && b.x1 <= self.bounds.x1
            && b.y1 <= self.bounds.y1
            && !self.boxes.iter().any(|o| o.hits(b))
    }
    pub fn take(&mut self, b: Bx) {
        self.boxes.push(b);
    }
}

/// A placed label (for map.json).
pub struct LabelOut {
    pub kind: &'static str,
    pub text: String,
    pub at: Vec2,
    pub size: f32,
    pub bx: Bx,
    /// Glyph centre line for curved labels (rivers, contours).
    pub path: Option<Vec<Vec2>>,
}

pub struct IconOut {
    pub id: u32,
    pub kind: Option<PoiKind>,
    pub name: String,
    pub at: Vec2,
    pub world: Vec2,
    pub radius: f32,
}

pub struct Placed {
    pub labels: Vec<LabelOut>,
    pub icons: Vec<IconOut>,
    pub zone_anchor: Vec<(u8, Vec2)>,
}

// ------------------------------------------------------------------ icons

/// Badge colour per kind (atlas, minimal, satellite).
pub fn kind_color(k: Option<PoiKind>) -> Rgba {
    match k {
        None => rgb(46, 150, 84),
        Some(k) => match k {
            PoiKind::Town => rgb(196, 64, 52),
            PoiKind::QuestHub => rgb(222, 158, 28),
            PoiKind::Dungeon => rgb(110, 64, 150),
            PoiKind::Landmark => rgb(58, 108, 168),
            PoiKind::Camp => rgb(172, 108, 52),
            PoiKind::Vendor => rgb(36, 146, 112),
            PoiKind::FlightPoint => rgb(48, 146, 198),
            PoiKind::Graveyard => rgb(96, 104, 112),
            PoiKind::Portal => rgb(146, 70, 200),
            PoiKind::BossArena => rgb(164, 28, 40),
            PoiKind::Shrine => rgb(196, 146, 58),
            PoiKind::Vista => rgb(66, 146, 88),
            PoiKind::Treasure => rgb(204, 154, 36),
            PoiKind::Custom => rgb(92, 92, 104),
        },
    }
}

/// Builds glyph shapes in a unit box (−1..1, y down) around `c`, scaled by `s`.
struct Glyph {
    c: Vec2,
    s: f32,
    /// (shape, knockout: drawn in the badge colour)
    shapes: Vec<(Path, bool)>,
}

impl Glyph {
    fn q(&self, x: f32, y: f32) -> Vec2 {
        self.c + Vec2::new(x, y) * self.s
    }
    fn poly(&mut self, pts: &[(f32, f32)], knock: bool) {
        let v: Vec<Vec2> = pts.iter().map(|(x, y)| self.q(*x, *y)).collect();
        if let Some(p) = poly_path(&v, true) {
            self.shapes.push((p, knock));
        }
    }
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, knock: bool) {
        self.poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], knock);
    }
    fn circle(&mut self, x: f32, y: f32, r: f32, knock: bool) {
        let p = self.q(x, y);
        if let Some(path) = PathBuilder::from_circle(p.x, p.y, r * self.s) {
            self.shapes.push((path, knock));
        }
    }
    /// A thick line as a quad.
    fn bar(&mut self, a: (f32, f32), b: (f32, f32), w: f32, knock: bool) {
        let (a, b) = (Vec2::new(a.0, a.1), Vec2::new(b.0, b.1));
        let n = (b - a).normalize_or(Vec2::X).perp() * (w * 0.5);
        self.poly(
            &[
                ((a + n).x, (a + n).y),
                ((b + n).x, (b + n).y),
                ((b - n).x, (b - n).y),
                ((a - n).x, (a - n).y),
            ],
            knock,
        );
    }
    /// An arch: a rectangle with a half-disc on top.
    fn arch(&mut self, x0: f32, x1: f32, y_spring: f32, y_bottom: f32, knock: bool) {
        let r = (x1 - x0) * 0.5;
        let cx = (x0 + x1) * 0.5;
        let mut pts = vec![(x0, y_bottom)];
        for k in 0..=12 {
            let a = std::f32::consts::PI + k as f32 / 12.0 * std::f32::consts::PI;
            pts.push((cx + a.cos() * r, y_spring + a.sin() * r));
        }
        pts.push((x1, y_bottom));
        self.poly(&pts, knock);
    }
}

fn glyph(kind: Option<PoiKind>, c: Vec2, s: f32) -> Glyph {
    let mut g = Glyph {
        c,
        s,
        shapes: Vec::new(),
    };
    match kind {
        None => {
            // player start: a flag
            g.bar((-0.45, -0.85), (-0.45, 0.85), 0.16, false);
            g.poly(&[(-0.42, -0.85), (0.75, -0.5), (-0.42, -0.12)], false);
        }
        Some(k) => match k {
            PoiKind::Town => {
                g.poly(
                    &[
                        (-0.85, 0.8),
                        (-0.85, -0.78),
                        (-0.71, -0.78),
                        (-0.71, -0.62),
                        (-0.59, -0.62),
                        (-0.59, -0.78),
                        (-0.45, -0.78),
                        (-0.45, -0.22),
                        (-0.27, -0.22),
                        (-0.27, -0.38),
                        (-0.09, -0.38),
                        (-0.09, -0.22),
                        (0.09, -0.22),
                        (0.09, -0.38),
                        (0.27, -0.38),
                        (0.27, -0.22),
                        (0.45, -0.22),
                        (0.45, -0.78),
                        (0.59, -0.78),
                        (0.59, -0.62),
                        (0.71, -0.62),
                        (0.71, -0.78),
                        (0.85, -0.78),
                        (0.85, 0.8),
                    ],
                    false,
                );
                g.arch(-0.2, 0.2, 0.4, 0.8, true);
            }
            PoiKind::QuestHub => {
                g.rect(-0.14, -0.85, 0.14, 0.32, false);
                g.circle(0.0, 0.66, 0.17, false);
            }
            PoiKind::Dungeon => {
                g.arch(-0.8, 0.8, -0.05, 0.82, false);
                g.arch(-0.4, 0.4, 0.2, 0.82, true);
                g.bar((-0.95, 0.82), (0.95, 0.82), 0.14, false);
            }
            PoiKind::Landmark => {
                g.poly(
                    &[
                        (-0.3, 0.68),
                        (-0.2, -0.48),
                        (0.0, -0.88),
                        (0.2, -0.48),
                        (0.3, 0.68),
                    ],
                    false,
                );
                g.rect(-0.55, 0.62, 0.55, 0.86, false);
            }
            PoiKind::Camp => {
                g.poly(&[(-0.9, 0.78), (0.0, -0.82), (0.9, 0.78)], false);
                g.poly(&[(-0.26, 0.78), (0.0, 0.12), (0.26, 0.78)], true);
            }
            PoiKind::Vendor => {
                g.circle(0.0, 0.22, 0.6, false);
                g.poly(
                    &[(-0.3, -0.36), (-0.42, -0.72), (0.42, -0.72), (0.3, -0.36)],
                    false,
                );
                g.bar((-0.36, -0.36), (0.36, -0.36), 0.12, true);
                g.circle(0.0, 0.24, 0.2, true);
            }
            PoiKind::FlightPoint => {
                g.poly(
                    &[
                        (-0.95, -0.02),
                        (-0.48, -0.52),
                        (0.0, 0.02),
                        (0.48, -0.52),
                        (0.95, -0.02),
                        (0.48, -0.24),
                        (0.0, 0.42),
                        (-0.48, -0.24),
                    ],
                    false,
                );
            }
            PoiKind::Graveyard => {
                g.arch(-0.55, 0.55, -0.25, 0.85, false);
                g.rect(-0.08, -0.48, 0.08, 0.42, true);
                g.rect(-0.3, -0.24, 0.3, -0.08, true);
            }
            PoiKind::Portal => {
                g.circle(0.0, 0.0, 0.85, false);
                g.circle(0.0, 0.0, 0.62, true);
                g.circle(0.0, 0.0, 0.42, false);
                g.circle(0.0, 0.0, 0.24, true);
            }
            PoiKind::BossArena => {
                for sx in [-1.0f32, 1.0] {
                    g.bar((-0.72 * sx, -0.75), (0.5 * sx, 0.5), 0.17, false);
                    g.bar((0.22 * sx, 0.62), (0.62 * sx, 0.22), 0.16, false);
                    g.circle(0.72 * sx, 0.72, 0.13, false);
                }
            }
            PoiKind::Shrine => {
                g.poly(&[(-0.85, -0.32), (0.0, -0.88), (0.85, -0.32)], false);
                g.rect(-0.9, 0.62, 0.9, 0.84, false);
                for x in [-0.56f32, 0.0, 0.56] {
                    g.rect(x - 0.09, -0.26, x + 0.09, 0.6, false);
                }
            }
            PoiKind::Vista => {
                let mut pts = vec![];
                for k in 0..=16 {
                    let t = k as f32 / 16.0;
                    pts.push((-0.95 + 1.9 * t, -0.62 * (std::f32::consts::PI * t).sin()));
                }
                for k in (0..=16).rev() {
                    let t = k as f32 / 16.0;
                    pts.push((-0.95 + 1.9 * t, 0.62 * (std::f32::consts::PI * t).sin()));
                }
                g.poly(&pts, false);
                g.circle(0.0, 0.0, 0.36, true);
                g.circle(0.0, 0.0, 0.18, false);
            }
            PoiKind::Treasure => {
                g.rect(-0.82, -0.08, 0.82, 0.72, false);
                g.arch(-0.82, 0.82, -0.2, -0.1, false);
                g.bar((-0.82, -0.08), (0.82, -0.08), 0.1, true);
                g.rect(-0.12, -0.2, 0.12, 0.16, true);
            }
            PoiKind::Custom => {
                g.poly(&[(0.0, -0.85), (0.7, 0.0), (0.0, 0.85), (-0.7, 0.0)], false);
            }
        },
    }
    g
}

/// Badge radius (px) for a kind at line-weight unit `u`.
pub fn icon_radius(kind: Option<PoiKind>, u: f32) -> f32 {
    match kind {
        Some(PoiKind::Town) => 10.0 * u,
        None | Some(PoiKind::Dungeon | PoiKind::BossArena | PoiKind::Portal) => 8.5 * u,
        _ => 7.5 * u,
    }
}

pub fn draw_icon(cv: &mut Canvas, style: MapStyle, kind: Option<PoiKind>, at: Vec2, r: f32) {
    let u = cv.u;
    let (disc, ring, ink) = match style {
        MapStyle::Parchment => (rgb(242, 232, 206), rgb(60, 44, 32), rgb(60, 44, 32)),
        _ => (kind_color(kind), rgb(255, 255, 255), rgb(255, 255, 255)),
    };
    if style != MapStyle::Parchment {
        cv.circle(at + Vec2::new(0.0, 0.9 * u), r + 1.6 * u, [0, 0, 0, 70]);
    }
    cv.circle(at, r, disc);
    cv.ring(
        at,
        r,
        if style == MapStyle::Parchment {
            1.1 * u
        } else {
            1.4 * u
        },
        ring,
    );
    if style == MapStyle::Parchment {
        cv.ring(at, r - 2.0 * u, 0.5 * u, with_alpha(ring, 0.6));
    }
    let g = glyph(
        kind,
        at,
        r * if style == MapStyle::Parchment {
            0.52
        } else {
            0.58
        },
    );
    for (p, knock) in &g.shapes {
        cv.fill_evenodd(p, if *knock { disc } else { ink });
    }
}

// ------------------------------------------------------------------ lettering

struct Styles {
    zone: TextStyle,
    zone_sub: TextStyle,
    poi: TextStyle,
    town: TextStyle,
    sea: TextStyle,
    lake: TextStyle,
    river: TextStyle,
    contour: TextStyle,
}

fn styles(c: &Ctx) -> Styles {
    let u = c.w as f32 / 2048.0;
    let k = &c.kit;
    let halo = |s: TextStyle, w: f32| s.halo(k.halo, w * u);
    if k.serif {
        Styles {
            zone: halo(
                TextStyle::new(Face::DisplayBold, 28.0 * u, k.region_label)
                    .tracking(0.14)
                    .upper(),
                5.0,
            ),
            zone_sub: halo(
                TextStyle::new(Face::Serif, 13.0 * u, k.region_sub).italic(),
                3.0,
            ),
            poi: halo(TextStyle::new(Face::SerifBold, 13.0 * u, k.label), 3.2),
            town: halo(
                TextStyle::new(Face::DisplayBold, 13.5 * u, k.label).tracking(0.04),
                3.4,
            ),
            sea: halo(
                TextStyle::new(Face::Serif, 24.0 * u, k.water_label)
                    .italic()
                    .upper()
                    .tracking(0.32),
                3.0,
            ),
            lake: halo(
                TextStyle::new(Face::Serif, 14.0 * u, k.water_label)
                    .italic()
                    .tracking(0.04),
                2.6,
            ),
            river: halo(
                TextStyle::new(Face::Serif, 12.5 * u, k.water_label)
                    .italic()
                    .tracking(0.08),
                2.6,
            ),
            contour: halo(TextStyle::new(Face::Sans, 8.5 * u, k.ink), 2.0),
        }
    } else {
        let contour_ink = rgb(150, 104, 66);
        Styles {
            zone: halo(
                TextStyle::new(Face::SansBold, 20.0 * u, k.region_label)
                    .tracking(0.24)
                    .upper(),
                3.6,
            ),
            zone_sub: halo(TextStyle::new(Face::Sans, 11.0 * u, k.region_sub), 2.8),
            poi: halo(TextStyle::new(Face::Sans, 11.5 * u, k.label), 2.8),
            town: halo(TextStyle::new(Face::SansBold, 12.5 * u, k.label), 3.0),
            sea: halo(
                TextStyle::new(Face::Sans, 18.0 * u, k.water_label)
                    .italic()
                    .upper()
                    .tracking(0.4),
                2.6,
            ),
            lake: halo(
                TextStyle::new(Face::Sans, 12.0 * u, k.water_label).italic(),
                2.6,
            ),
            river: halo(
                TextStyle::new(Face::Sans, 10.5 * u, k.water_label)
                    .italic()
                    .tracking(0.06),
                2.4,
            ),
            contour: TextStyle::new(Face::Sans, 8.0 * u, contour_ink)
                .halo(super::canvas::rgba(250, 246, 236, 190), 2.2 * u),
        }
    }
}

/// Places every icon and label (avoiding the decorations), draws them, and returns where.
pub fn place_and_draw(
    cv: &mut Canvas,
    c: &Ctx,
    fonts: &Fonts,
    deco: &Deco,
    drawn: &Drawn,
) -> Placed {
    let u = cv.u;
    let w = c.w as f32;
    let inset = deco.frame_inset + 4.0 * u;
    let mut pl = Placer {
        boxes: deco.reserved.clone(),
        bounds: Bx::new(inset, inset, w - inset, w - inset),
    };
    let st = styles(c);
    let mut out = Placed {
        labels: Vec::new(),
        icons: Vec::new(),
        zone_anchor: Vec::new(),
    };
    let layers = &c.settings.layers;

    // icons first: they mark exact places and must never move
    if layers.pois {
        let mut items: Vec<(Option<PoiKind>, u32, String, Vec2)> = c
            .inp
            .pois
            .iter()
            .map(|p| (Some(p.kind), p.id, p.name.clone(), p.pos))
            .collect();
        if let Some(s) = c.inp.start {
            items.push((None, 0, "Start".into(), s));
        }
        for (kind, id, name, world) in items {
            let at = c.world_to_px(world);
            let r = icon_radius(kind, u);
            draw_icon(cv, c.kit.style, kind, at, r);
            pl.take(Bx::around(at, Vec2::splat(r + 1.0 * u)));
            out.icons.push(IconOut {
                id,
                kind,
                name,
                at,
                world,
                radius: r,
            });
        }
    }
    if !layers.labels {
        return out;
    }

    // regions: at the point deepest inside each zone (away from borders and the sea)
    if layers.zones {
        let n = c.inp.n();
        for z in &c.inp.zones {
            let mut cells = 0usize;
            let mut cands: Vec<(f32, Vec2)> = Vec::new();
            let step = (n / 384).max(1);
            for j in (0..n).step_by(step) {
                for i in (0..n).step_by(step) {
                    if c.inp.zone_ids[j * n + i] != z.id {
                        continue;
                    }
                    cells += 1;
                    let p = Vec2::new(i as f32, j as f32) * c.k;
                    let to_sea = -c.coast_d(p);
                    if to_sea <= 0.0 {
                        continue;
                    }
                    let d = c.zone_d.v[j * n + i].min(to_sea * 1.4);
                    cands.push((d, p));
                }
            }
            if cands.is_empty() {
                continue;
            }
            cands.sort_by(|a, b| b.0.total_cmp(&a.0));
            let area = cells as f32 * (step as f32 * c.k).powi(2);
            let mut size = (area.sqrt() * 0.07).clamp(17.0 * u, 34.0 * u);
            let name = z.name.clone();
            let mut placed = false;
            'sizes: for _ in 0..4 {
                let mut zs = st.zone.clone();
                zs.size = size;
                let mut sub = st.zone_sub.clone();
                sub.size = (size * 0.46).max(10.0 * u);
                let mut tried: Vec<Vec2> = Vec::new();
                for (d, p) in cands.iter().take(400) {
                    if tried.iter().any(|q| q.distance(*p) < 24.0 * u) {
                        continue;
                    }
                    tried.push(*p);
                    let wname = measure(fonts, &zs, &name);
                    if wname * 0.5 > d * 1.6 && size > 17.5 * u {
                        continue;
                    }
                    let b1 = label_box(fonts, &zs, &name, *p, Align::Centre);
                    let sub_at = *p + Vec2::new(0.0, size * 0.62 + sub.size * 0.55);
                    let b2 = label_box(fonts, &sub, &z.level, sub_at, Align::Centre);
                    let all = b1.union(&b2);
                    if pl.free(&all) {
                        cv.text(fonts, &zs, &name, *p, Align::Centre);
                        cv.text(fonts, &sub, &z.level, sub_at, Align::Centre);
                        pl.take(all.grow(4.0 * u));
                        out.labels.push(LabelOut {
                            kind: "zone",
                            text: name.clone(),
                            at: *p,
                            size,
                            bx: b1,
                            path: None,
                        });
                        out.labels.push(LabelOut {
                            kind: "zone_level",
                            text: z.level.clone(),
                            at: sub_at,
                            size: sub.size,
                            bx: b2,
                            path: None,
                        });
                        out.zone_anchor.push((z.id, *p));
                        placed = true;
                        break 'sizes;
                    }
                }
                size *= 0.82;
            }
            if !placed {
                bevy::log::info!("map: no room for the {} label", z.name);
            }
        }
    }

    // the sea: far out where it's widest
    let sea_name = c.settings.sea_name.trim();
    if !sea_name.is_empty() {
        let mut cands: Vec<(f32, Vec2)> = Vec::new();
        let step = 24.0 * u;
        let mut y = step;
        while y < w - step {
            let mut x = step;
            while x < w - step {
                let p = Vec2::new(x, y);
                let d = c.coast_d(p);
                if d > 0.0 {
                    // keep away from the frame too
                    let edge = x.min(y).min(w - x).min(w - y);
                    cands.push((d.min(edge * 0.9), p));
                }
                x += step;
            }
            y += step;
        }
        cands.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, p) in cands.iter().take(200) {
            let b = label_box(fonts, &st.sea, sea_name, *p, Align::Centre);
            if pl.free(&b) {
                cv.text(fonts, &st.sea, sea_name, *p, Align::Centre);
                pl.take(b.grow(6.0 * u));
                out.labels.push(LabelOut {
                    kind: "sea",
                    text: sea_name.into(),
                    at: *p,
                    size: st.sea.size,
                    bx: b,
                    path: None,
                });
                break;
            }
        }
    }

    // lakes: centred on the water when they fit, else beside it
    for l in &c.inp.lakes {
        if l.name.trim().is_empty() {
            continue;
        }
        let p = c.world_to_px(l.seed);
        let mut done = false;
        for at in [
            p,
            p + Vec2::new(0.0, -14.0 * u),
            p + Vec2::new(0.0, 14.0 * u),
        ] {
            let b = label_box(fonts, &st.lake, &l.name, at, Align::Centre);
            let inside = [
                Vec2::new(b.x0, b.y0),
                Vec2::new(b.x1, b.y0),
                Vec2::new(b.x0, b.y1),
                Vec2::new(b.x1, b.y1),
            ]
            .iter()
            .all(|q| c.inland_water(*q));
            if inside && pl.free(&b) {
                cv.text(fonts, &st.lake, &l.name, at, Align::Centre);
                pl.take(b);
                out.labels.push(LabelOut {
                    kind: "lake",
                    text: l.name.clone(),
                    at,
                    size: st.lake.size,
                    bx: b,
                    path: None,
                });
                done = true;
                break;
            }
        }
        if !done {
            // beside the lake: walk outwards until the shore is passed
            'dirs: for dir in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
                let mut q = p;
                for _ in 0..200 {
                    if !c.inland_water(q) {
                        break;
                    }
                    q += dir * 2.0 * u;
                }
                let align = if dir.x > 0.5 {
                    Align::Left
                } else if dir.x < -0.5 {
                    Align::Right
                } else {
                    Align::Centre
                };
                let at = q
                    + dir * 6.0 * u
                    + if dir.y.abs() > 0.5 {
                        dir * 6.0 * u
                    } else {
                        Vec2::ZERO
                    };
                let b = label_box(fonts, &st.lake, &l.name, at, align);
                if pl.free(&b) {
                    cv.text(fonts, &st.lake, &l.name, at, align);
                    pl.take(b);
                    out.labels.push(LabelOut {
                        kind: "lake",
                        text: l.name.clone(),
                        at,
                        size: st.lake.size,
                        bx: b,
                        path: None,
                    });
                    break 'dirs;
                }
            }
        }
    }

    // rivers: along the straightest stretch, beside the water
    for (ri, line) in &drawn.rivers {
        let r = &c.inp.rivers[*ri];
        if r.name.trim().is_empty() || line.len() < 4 {
            continue;
        }
        let tw = measure(fonts, &st.river, &r.name);
        let span = tw * 1.15;
        let total: f32 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
        if span > total * 0.8 {
            continue;
        }
        let pts = super::field::resample(line, 2.0 * u);
        let seg = (span / (2.0 * u)) as usize;
        let mut wins: Vec<(f32, usize)> = Vec::new();
        let lo = pts.len() / 8;
        let hi = pts.len().saturating_sub(seg + pts.len() / 8);
        let mut s = lo;
        while s < hi {
            let win = &pts[s..=s + seg];
            let mut turn = 0.0;
            for k in 1..win.len() - 1 {
                let a = (win[k] - win[k - 1]).normalize_or(Vec2::X);
                let b = (win[k + 1] - win[k]).normalize_or(Vec2::X);
                turn += a.angle_to(b).abs();
            }
            // prefer gentle, more horizontal stretches
            let d = (win[win.len() - 1] - win[0]).normalize_or(Vec2::X);
            wins.push((turn + d.y.abs() * 0.6, s));
            s += (seg / 4).max(1);
        }
        wins.sort_by(|a, b| a.0.total_cmp(&b.0));
        let off = r.width * 0.5 * c.ppm() + st.river.size * 0.5 + 2.5 * u;
        'win: for (_, s) in wins.iter().take(12) {
            let win = &pts[*s..=*s + seg];
            for side in [1.0f32, -1.0] {
                let shifted: Vec<Vec2> = (0..win.len())
                    .map(|k| {
                        let d = (win[(k + 1).min(win.len() - 1)] - win[k.saturating_sub(1)])
                            .normalize_or(Vec2::X);
                        win[k] + d.perp() * off * side
                    })
                    .collect();
                let shifted = super::field::chaikin(&shifted, false, 2);
                let Some(boxes) = cv.text_along(fonts, &st.river, &r.name, &shifted, true) else {
                    continue;
                };
                if boxes.iter().all(|b| pl.free(b)) {
                    cv.text_along(fonts, &st.river, &r.name, &shifted, false);
                    let all = boxes.iter().skip(1).fold(boxes[0], |a, b| a.union(b));
                    for b in boxes {
                        pl.take(b);
                    }
                    out.labels.push(LabelOut {
                        kind: "river",
                        text: r.name.clone(),
                        at: all.centre(),
                        size: st.river.size,
                        bx: all,
                        path: Some(shifted),
                    });
                    break 'win;
                }
            }
        }
    }

    // places: beside their icon, trying eight positions
    for ic in out.icons.iter() {
        if ic.name.trim().is_empty() || ic.kind.is_none() {
            continue;
        }
        let ts = if ic.kind == Some(PoiKind::Town) {
            &st.town
        } else {
            &st.poi
        };
        // clear of the icon's own box once the halo is counted
        let gap = ic.radius + 2.5 * u + ts.halo.map(|h| h.1 * 0.5).unwrap_or(0.0);
        let cap = super::canvas::cap_height(ts);
        let d = gap * 0.75;
        let cands = [
            (Vec2::new(gap, 0.0), Align::Left),
            (Vec2::new(-gap, 0.0), Align::Right),
            (Vec2::new(0.0, -gap - cap * 0.6), Align::Centre),
            (Vec2::new(0.0, gap + cap * 0.7), Align::Centre),
            (Vec2::new(d, -d - cap * 0.3), Align::Left),
            (Vec2::new(d, d + cap * 0.3), Align::Left),
            (Vec2::new(-d, -d - cap * 0.3), Align::Right),
            (Vec2::new(-d, d + cap * 0.3), Align::Right),
        ];
        for (off, align) in cands {
            let at = ic.at + off;
            let b = label_box(fonts, ts, &ic.name, at, align);
            if pl.free(&b) {
                cv.text(fonts, ts, &ic.name, at, align);
                pl.take(b);
                out.labels.push(LabelOut {
                    kind: "poi",
                    text: ic.name.clone(),
                    at,
                    size: ts.size,
                    bx: b,
                    path: None,
                });
                break;
            }
        }
    }

    // contour heights (atlas): a few, on the longest index lines
    let mut idx: Vec<&(f32, Vec<Vec2>)> = drawn.index_contours.iter().collect();
    idx.sort_by_key(|c| std::cmp::Reverse(c.1.len()));
    for (level, line) in idx.into_iter().take(40) {
        let text = format!("{}", level.round() as i32);
        let tw = measure(fonts, &st.contour, &text);
        let pts = super::field::resample(line, 1.5 * u);
        if pts.len() < 8 {
            continue;
        }
        let seg = ((tw * 1.2) / (1.5 * u)) as usize + 1;
        for frac in [0.5f32, 0.25, 0.75] {
            let s0 = ((pts.len() as f32 * frac) as usize).min(pts.len().saturating_sub(seg + 1));
            let win = &pts[s0..(s0 + seg).min(pts.len())];
            if win.len() < 2 {
                continue;
            }
            let Some(boxes) = cv.text_along(fonts, &st.contour, &text, win, true) else {
                continue;
            };
            if boxes.iter().all(|b| pl.free(b)) {
                cv.text_along(fonts, &st.contour, &text, win, false);
                for b in &boxes {
                    pl.take(b.grow(10.0 * u));
                }
                break;
            }
        }
    }
    out
}

/// map.json: how to use the map in a game.
pub fn meta(
    c: &Ctx,
    placed: &Placed,
    borders: &[ZoneLine],
    drawn: &Drawn,
    fog_size: u32,
) -> serde_json::Value {
    let ppm = c.ppm();
    let half = c.inp.half();
    let r1 = |v: f32| (v * 10.0).round() / 10.0;
    let pt = |p: Vec2| json!([r1(p.x), r1(p.y)]);
    let zones: Vec<serde_json::Value> = c
        .inp
        .zones
        .iter()
        .map(|z| {
            let polys: Vec<Vec<serde_json::Value>> = zone_polygons(c, z.id).into_iter().map(|poly| poly.into_iter().map(pt).collect()).collect();
            json!({
                "id": z.id,
                "name": z.name,
                "levels": z.level,
                "type": format!("{:?}", z.kind),
                "color_srgb": z.color,
                "label_px": placed.zone_anchor.iter().find(|(id, _)| *id == z.id).map(|(_, p)| pt(*p)),
                "polygons_px": polys,
            })
        })
        .collect();
    let pois: Vec<serde_json::Value> = placed
        .icons
        .iter()
        .map(|i| {
            json!({
                "id": i.id,
                "kind": i.kind.map(|k| format!("{k:?}")).unwrap_or_else(|| "PlayerStart".into()),
                "name": i.name,
                "world_xz": [r1(i.world.x), r1(i.world.y)],
                "px": pt(i.at),
                "icon_radius_px": r1(i.radius),
            })
        })
        .collect();
    let labels: Vec<serde_json::Value> = placed
        .labels
        .iter()
        .map(|l| {
            json!({
                "kind": l.kind,
                "text": l.text,
                "anchor_px": pt(l.at),
                "size_px": r1(l.size),
                "box_px": [r1(l.bx.x0), r1(l.bx.y0), r1(l.bx.x1), r1(l.bx.y1)],
                "path_px": l.path.as_ref().map(|p| p.iter().step_by(4).map(|q| pt(*q)).collect::<Vec<_>>()),
            })
        })
        .collect();
    let roads: Vec<serde_json::Value> = c
        .inp
        .roads
        .iter()
        .map(|r| {
            let px: Vec<Vec2> = r.line.iter().map(|p| c.world_to_px(*p)).collect();
            json!({"name": r.name, "kind": format!("{:?}", r.kind), "px": super::field::simplify(&px, 1.0).into_iter().map(pt).collect::<Vec<_>>()})
        })
        .collect();
    let rivers: Vec<serde_json::Value> = drawn
        .rivers
        .iter()
        .map(|(i, line)| {
            let r = &c.inp.rivers[*i];
            json!({"name": r.name, "width_m": r.width, "px": super::field::simplify(line, 1.0).into_iter().map(pt).collect::<Vec<_>>()})
        })
        .collect();
    let borders: Vec<serde_json::Value> = borders
        .iter()
        .map(|b| json!({"zones": [b.a, b.b], "px": super::field::simplify(&b.pts, 1.0).into_iter().map(pt).collect::<Vec<_>>()}))
        .collect();
    json!({
        "format": "world-builder-map/1",
        "world": c.inp.name,
        "style": c.settings.style.id(),
        "size_px": c.w,
        "transform": {
            "world_min_xz": [-half, -half],
            "world_size_m": c.inp.size_m(),
            "px_per_m": ppm,
            "world_to_map": "px = (x - world_min_x) * px_per_m, py = (z - world_min_z) * px_per_m",
            "map_to_world": "x = px / px_per_m + world_min_x, z = py / px_per_m + world_min_z",
            "orientation": "north (−Z) is up, east (+X) is right; pixel (0,0) is the north-west corner",
        },
        "layers": c.settings.layers,
        "zones": zones,
        "zone_borders": borders,
        "pois": pois,
        "labels": labels,
        "roads": roads,
        "rivers": rivers,
        "fog_of_war": {
            "mask": "fog_mask.png",
            "size_px": fog_size,
            "encoding": "L8: 255 hidden, 0 revealed; same orientation as the map (scale by size_px / map size)",
            "reveal": "clear the mask around each player as they explore (radius ~150 m); zones can be revealed whole with their polygons",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_a_glyph() {
        for k in PoiKind::ALL {
            assert!(
                !glyph(Some(k), Vec2::splat(20.0), 6.0).shapes.is_empty(),
                "{k:?}"
            );
        }
        assert!(!glyph(None, Vec2::splat(20.0), 6.0).shapes.is_empty());
    }
}
