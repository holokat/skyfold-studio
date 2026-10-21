//! Create a dungeon › Realm › Temple compound: its theme, how many boss courts, their order, the
//! realm's size, and a live preview of the layout on its land (made on a worker thread and
//! cached, so the panel never waits; Shuffle rolls another).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use bevy::math::Vec2;
use bevy_egui::egui::{self, Color32, FontId, RichText};
use egui_phosphor::regular as icon;

use super::Options;
use super::layout::{self, CourtKind, Grid, Plan, TrailKind};
use crate::dungeons::realm_shapes::{self, Choice, RealmShape, RealmSize};
use crate::terrain::realm_land::{self, Detail};
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// A preview: the land of `size` and `seed` with the compound of `o` drawn on it (RGBA, `px`²),
/// and its layout. The land is the realm's own (made in about half a second), so the layout is
/// exactly the one the realm gets.
pub fn render_preview(
    o: Options,
    size: RealmSize,
    seed: u32,
    px: usize,
) -> (Vec<u8>, Result<Plan, String>) {
    let mut land = realm_land::generate(RealmShape::EnclosedValley, size, seed, Detail::Full);
    let n = land.n();
    let ask = layout::Ask {
        theme: o.theme,
        courts: o.courts as usize,
        order: o.order,
        seed,
        levels: [10, 15],
    };
    let plan = {
        let g = Grid {
            n,
            cell: land.cell,
            h: &land.heights,
        };
        layout::lay_out(&g, &ask)
    };
    let pal = realm_shapes::palette(o.theme.realm());
    let Ok(plan) = plan else {
        return (realm_land::render(&land, px, pal), plan);
    };
    super::shape::carve(&mut land.heights, n, land.cell, &plan);
    // the lakes as water
    let half = land.size() * 0.5;
    land.wet = (0..n * n)
        .map(|k| {
            let p = Vec2::new(
                (k % n) as f32 * land.cell - half,
                (k / n) as f32 * land.cell - half,
            );
            plan.lakes.iter().any(|l| {
                p.distance(l.c) < l.r * 1.6 && crate::dungeons::geom::poly_sdf(&l.outline, p) > 0.0
            }) && !plan
                .courts
                .iter()
                .any(|c| c.island.is_some() && p.distance(c.c) < c.hx + 6.0)
        })
        .collect();
    land.arrival = plan.entrance.start;
    let mut img = realm_land::render(&land, px, pal);
    overlay(&mut img, px, land.size(), &plan);
    // the ruins and the district's broken walls: small dark marks
    let mpp = land.size() / px as f32;
    for r in &plan.ruins {
        let q = (r.p + Vec2::splat(half)) / mpp;
        let (x, y) = (q.x as i32, q.y as i32);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = (x + dx, y + dy);
            if x >= 0 && y >= 0 && (x as usize) < px && (y as usize) < px {
                let o = (y as usize * px + x as usize) * 4;
                img[o..o + 3].copy_from_slice(&[84, 76, 66]);
            }
        }
    }
    (img, Ok(plan))
}

/// The compound drawn over the land: paving, platforms, trails, the wall, the courts' order.
fn overlay(img: &mut [u8], px: usize, size: f32, plan: &Plan) {
    let mpp = size / px as f32;
    let half = size * 0.5;
    let to_world = |x: usize, y: usize| {
        Vec2::new((x as f32 + 0.5) * mpp - half, (y as f32 + 0.5) * mpp - half)
    };
    let blend = |img: &mut [u8], o: usize, c: [u8; 3], a: f32| {
        for i in 0..3 {
            img[o + i] = (img[o + i] as f32 * (1.0 - a) + c[i] as f32 * a).round() as u8;
        }
    };
    let pave = [214u8, 204, 180];
    let line = (mpp * 0.9).max(1.2);
    let bbox = |pts: &[Vec2], m: f32| {
        pts.iter()
            .fold(
                (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
                |(a, b), p| (a.min(*p), b.max(*p)),
            )
            .into_box(m)
    };
    let trail_boxes: Vec<(Vec2, Vec2)> = plan
        .trails
        .iter()
        .map(|t| bbox(&t.pts, t.width + line * 2.0))
        .collect();
    let wall_box = bbox(&plan.entrance.wall, line * 3.0);
    let court_boxes: Vec<(Vec2, Vec2)> = plan
        .all_courts()
        .map(|c| {
            (
                c.c - Vec2::splat(c.reach() + 2.0),
                c.c + Vec2::splat(c.reach() + 2.0),
            )
        })
        .collect();
    let inb =
        |b: &(Vec2, Vec2), p: Vec2| p.x >= b.0.x && p.y >= b.0.y && p.x <= b.1.x && p.y <= b.1.y;
    for y in 0..px {
        for x in 0..px {
            let p = to_world(x, y);
            let o = (y * px + x) * 4;
            // trails and the way
            for (t, b) in plan.trails.iter().zip(&trail_boxes) {
                if !inb(b, p) {
                    continue;
                }
                let (d, _) = crate::dungeons::geom::polyline_dist(&t.pts, p);
                let w = (t.width * 0.5).max(line);
                if d < w {
                    let c = match t.kind {
                        TrailKind::Axis => pave,
                        TrailKind::Bridge => [150, 112, 72],
                        _ => [168, 132, 88],
                    };
                    blend(img, o, c, 0.9);
                }
            }
            // the wall
            if plan.entrance.wall.len() >= 2
                && inb(&wall_box, p)
                && crate::dungeons::geom::polyline_dist(&plan.entrance.wall, p).0 < line * 1.6
            {
                blend(img, o, [70, 66, 62], 0.95);
            }
            // the plaza, the hub, the round plaza
            let en = &plan.entrance;
            if p.distance(en.c) < en.r {
                blend(img, o, pave, 0.9);
            }
            let hd = p.distance(plan.hub.c);
            if hd < plan.hub.r && hd > plan.hub.r - line * 1.8 {
                blend(img, o, pave, 0.9);
            }
            if plan.axis.plaza_r > 0.0 && p.distance(plan.axis.plaza) < plan.axis.plaza_r {
                blend(img, o, pave, 0.9);
            }
            if hd < plan.hub.r * 0.9 {
                blend(img, o, pave, 0.55);
            }
            // the courts
            for (c, b) in plan.all_courts().zip(&court_boxes) {
                if !inb(b, p) {
                    continue;
                }
                let d = c.sdf(p);
                if d > 0.0 {
                    let col = match c.kind {
                        CourtKind::ThroneTerrace => [232, 196, 104],
                        _ if c.sdf_main(p) <= 0.0 => pave,
                        _ => layer_rgb(c.floor),
                    };
                    blend(img, o, col, 0.92);
                    if d < line * 1.2 || c.sdf_main(p).abs() < line * 0.8 {
                        blend(img, o, [60, 56, 52], 0.8);
                    }
                }
                if c.sdf2(p) > 0.0 {
                    blend(img, o, [246, 214, 120], 0.9);
                }
            }
        }
    }
}

trait IntoBox {
    fn into_box(self, m: f32) -> (Vec2, Vec2);
}

impl IntoBox for (Vec2, Vec2) {
    fn into_box(self, m: f32) -> (Vec2, Vec2) {
        (self.0 - Vec2::splat(m), self.1 + Vec2::splat(m))
    }
}

fn layer_rgb(l: crate::terrain::layers::Layer) -> [u8; 3] {
    use crate::terrain::layers::Layer;
    match l {
        Layer::Cobblestone => [196, 186, 168],
        Layer::Ice => [170, 186, 196],
        Layer::Gravel => [176, 170, 160],
        Layer::Rock => [150, 144, 136],
        Layer::DarkRock => [96, 90, 92],
        Layer::Dirt => [176, 136, 96],
        Layer::Snow => [236, 240, 246],
        _ => [200, 190, 170],
    }
}

// ------------------------------------------------------------------ the cache

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    o: Options,
    size: RealmSize,
    seed: u32,
    px: u16,
}

type Made = (Arc<Vec<u8>>, Option<String>);

#[derive(Default)]
struct Cache {
    images: HashMap<Key, Option<Made>>,
    order: VecDeque<Key>,
    textures: HashMap<Key, egui::TextureHandle>,
    last: Option<Key>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Cache::default))
}

fn image(key: Key) -> Option<Made> {
    let (have, start) = with(|c| match c.images.get(&key) {
        Some(i) => (i.clone(), false),
        None => {
            c.images.insert(key, None);
            c.order.push_back(key);
            while c.order.len() > 24 {
                if let Some(old) = c.order.pop_front()
                    && c.images.get(&old).is_some_and(|i| i.is_some())
                {
                    c.images.remove(&old);
                    c.textures.remove(&old);
                }
            }
            (None, true)
        }
    });
    if start {
        std::thread::spawn(move || {
            let (img, plan) = render_preview(key.o, key.size, key.seed, key.px as usize);
            let note = match plan {
                Ok(p) => Some(format!(
                    "{} courts{}, {} lakes, axis {:.0} m",
                    p.courts.len(),
                    if p.dropped > 0 {
                        format!(" ({} didn't fit)", p.dropped)
                    } else {
                        String::new()
                    },
                    p.lakes.len(),
                    p.axis.len()
                )),
                Err(e) => Some(e),
            };
            with(|c| {
                c.images.insert(key, Some((Arc::new(img), note)));
            });
        });
    }
    have
}

fn texture(ctx: &egui::Context, key: Key) -> Option<(egui::TextureHandle, Option<String>)> {
    let made = image(key)?;
    if let Some(t) = with(|c| c.textures.get(&key).cloned()) {
        return Some((t, made.1));
    }
    let ci = egui::ColorImage::from_rgba_unmultiplied([key.px as usize, key.px as usize], &made.0);
    let t = ctx.load_texture(
        format!(
            "compound_{}_{}_{}_{:?}_{}",
            key.o.theme.id(),
            key.o.courts,
            key.seed,
            key.size,
            key.px
        ),
        ci,
        egui::TextureOptions::LINEAR,
    );
    with(|c| {
        c.textures.insert(key, t.clone());
    });
    Some((t, made.1))
}

// ------------------------------------------------------------------ the panel

/// The compound's options and the preview of its layout. `land`: the realm's size and seed (its
/// shape is the enclosed valley). Returns true when anything changed.
pub fn picker(ui: &mut egui::Ui, o: &mut Options, land: &mut Choice) -> bool {
    let mut changed = false;
    land.shape = RealmShape::EnclosedValley;
    ui.add_space(6.0);
    ui.label(
        RichText::new("Theme")
            .font(FontId::new(13.0, semibold()))
            .color(TEXT),
    );
    let w = (ui.available_width() - 4.0 * 6.0) / 5.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for t in super::Theme::ALL {
            if tool_tile(ui, t.icon(), t.short(), o.theme == t, egui::vec2(w, 52.0))
                .on_hover_text(t.hint())
                .clicked()
                && o.theme != t
            {
                o.theme = t;
                changed = true;
            }
        }
    });
    dim(ui, o.theme.name());
    ui.add_space(4.0);
    let mut courts = o.courts as f32;
    if slider_row(ui, "Boss courts", &mut courts, 3.0..=6.0, |x| format!("{:.0} + the final court", x)).on_hover_text("How many boss courts stand round the hub (the final court at the end of the axis comes on top)").changed() {
        o.courts = courts.round() as u8;
        changed = true;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Order").color(TEXT)).on_hover_text("Hub: trails from a central hub out to every court, in any order, the final court's gate opens once they have all fallen. Linear: one court after another along one trail");
        let mut ord = o.order;
        let opts: Vec<(super::Order, &str)> = super::Order::ALL.iter().map(|x| (*x, x.name())).collect();
        if segmented(ui, &mut ord, &opts) {
            o.order = ord;
            changed = true;
        }
    });
    let mut sz = land.size;
    let opts: Vec<(RealmSize, &str)> = RealmSize::ALL.iter().map(|s| (*s, s.name())).collect();
    if segmented(ui, &mut sz, &opts) {
        land.size = sz;
        changed = true;
    }
    ui.add_space(6.0);
    // the preview
    let ctx = ui.ctx().clone();
    let key = Key {
        o: *o,
        size: land.size,
        seed: land.seed,
        px: 256,
    };
    let fresh = texture(&ctx, key);
    let shown = fresh.clone().map(|t| (t, true)).or_else(|| {
        with(|c| c.last.and_then(|k| c.textures.get(&k).cloned())).map(|t| ((t, None), false))
    });
    if fresh.is_some() {
        with(|c| c.last = Some(key));
    }
    let side = ui.available_width().min(256.0);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), side), egui::Sense::click());
    let img = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
    let p = ui.painter().clone();
    p.rect_filled(img, egui::CornerRadius::same(10), CARD_BG);
    let mut note = None;
    match &shown {
        Some(((t, n), ready)) => {
            egui::Image::new((t.id(), img.size()))
                .corner_radius(10)
                .tint(if *ready {
                    Color32::WHITE
                } else {
                    Color32::from_gray(120)
                })
                .paint_at(ui, img);
            if *ready {
                note = n.clone();
            }
        }
        None => {
            p.text(
                img.center(),
                egui::Align2::CENTER_CENTER,
                "Laying the compound out…",
                FontId::proportional(12.0),
                TEXT_DIM,
            );
        }
    }
    if fresh.is_none() {
        ui.put(
            egui::Rect::from_center_size(img.center(), egui::vec2(28.0, 28.0)),
            egui::Spinner::new().size(24.0),
        );
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    let caption = p.layout_no_wrap(
        format!("Temple compound · {} across", land.size.across()),
        FontId::new(12.0, semibold()),
        Color32::WHITE,
    );
    let at = img.left_top() + egui::vec2(10.0, 9.0);
    p.rect_filled(
        egui::Rect::from_min_size(
            at - egui::vec2(5.0, 3.0),
            caption.size() + egui::vec2(10.0, 6.0),
        ),
        egui::CornerRadius::same(6),
        Color32::from_black_alpha(140),
    );
    p.galley(at, caption, Color32::WHITE);
    if resp.on_hover_text("The compound laid out on its land, just as the realm will get it. The white ring is where players arrive; the gold terrace is the final court. Click for another roll").clicked() {
        land.seed = realm_shapes::random_seed();
        changed = true;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::Button::new(
                RichText::new(format!("{}  Shuffle", icon::SHUFFLE)).color(ON_ACCENT),
            )
            .fill(ACCENT)
            .min_size(egui::vec2(110.0, 28.0)),
        );
        if r.on_hover_text("Another compound of this theme and size")
            .clicked()
        {
            land.seed = realm_shapes::random_seed();
            changed = true;
        }
        ui.label(
            RichText::new(format!("seed {}", land.seed))
                .small()
                .color(TEXT_DIM),
        );
    });
    if let Some(n) = note {
        dim(ui, &n);
    }
    dim(
        ui,
        "An enclosed valley: the way in at the open end behind a wall, a hub with trails out to raised boss courts, lakes and an island temple between them, a straight processional way to the final court.",
    );
    changed
}
