//! Looking a dungeon over: its **minimaps** (a map of each level, drawn from the plan as the walls
//! stand: ink walls on parchment floors, pits and chasms dark, water blue, stairs hatched, doors
//! and gates, the game's markers, the way in and out) and a **walkability** check (from the way
//! in, which rooms a walker can reach and which they can't: a flood over every spot with a floor
//! and headroom, up steps and stairs, through doorways, round what blocks).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use bevy::prelude::*;
use image::{Rgba, RgbaImage};

use super::model::MapDoc;
use super::pieces::{Blocker, PieceKind};
use super::plan::{Compiled, RoomKind, Span};

/// Metres per minimap pixel (a big dungeon is drawn coarser to stay under `MAX_PX` a side).
const MPP: f32 = 0.2;
const MAX_PX: u32 = 1400;

/// A level's minimap: the picture, where its top-left corner is (x, z) and metres per pixel
/// (x grows east with the image's columns, z with its rows).
pub struct Minimap {
    pub level: i32,
    pub image: RgbaImage,
    pub origin: Vec2,
    pub mpp: f32,
}

/// The levels with rooms on them, lowest first.
pub fn levels(m: &MapDoc) -> Vec<i32> {
    let mut v: Vec<i32> = m.plan.rooms.iter().map(|r| r.level).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Draws level `level` of `m` (`c`: its compiled plan; `exit`: the way out's foot, if any).
pub fn minimap(m: &MapDoc, c: &Compiled, level: i32, exit: Option<Vec3>) -> Option<Minimap> {
    let rooms: Vec<&super::plan::CRoom> = c.rooms.iter().filter(|r| r.level == level).collect();
    if rooms.is_empty() {
        return None;
    }
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for r in &rooms {
        for p in r.shape.outline() {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    lo -= Vec2::splat(2.0);
    hi += Vec2::splat(2.0);
    let ext = hi - lo;
    let mpp = MPP.max(ext.max_element() / MAX_PX as f32);
    let (w, h) = ((ext.x / mpp).ceil() as u32, (ext.y / mpp).ceil() as u32);
    let ly = m.plan.level_y(level);
    // what each pixel is: 0 rock, 1 floor, 2 pit / chasm, 3 water, 4 stairs, 5 solid (pillar…)
    let mut what = vec![0u8; (w * h) as usize];
    let mut spans: Vec<Span> = Vec::new();
    let mut solids: Vec<(f32, f32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let p = lo + Vec2::new(x as f32 + 0.5, y as f32 + 0.5) * mpp;
            c.spans(p, &mut spans);
            let mine: Vec<&Span> = spans
                .iter()
                .filter(|s| c.room(s.room).is_some_and(|r| r.level == level))
                .collect();
            if mine.is_empty() {
                continue;
            }
            let kind = |s: &Span| c.room(s.room).map(|r| r.kind).unwrap_or(RoomKind::Room);
            let k = if mine.iter().any(|s| matches!(kind(s), RoomKind::Pool)) {
                3
            } else if mine.iter().all(|s| kind(s).is_pit()) {
                2
            } else if mine
                .iter()
                .any(|s| matches!(kind(s), RoomKind::Stairs | RoomKind::Spiral))
            {
                4
            } else {
                1
            };
            // pillars and the like standing on this level's floor
            c.solid_spans(p, &mut solids);
            let floor = mine.iter().map(|s| s.lo).fold(f32::MAX, f32::min);
            let pillar = solids
                .iter()
                .any(|(a, b)| *a <= floor + 0.2 && *b > floor + 1.6 && *b < ly + 40.0);
            what[(y * w + x) as usize] = if pillar && k == 1 { 5 } else { k };
        }
    }
    let at = |x: i32, y: i32| -> u8 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            0
        } else {
            what[(y as u32 * w + x as u32) as usize]
        }
    };
    let paper = |x: u32, y: u32| -> f32 {
        // (fbm is about −1..1)
        let n = crate::noise::fbm(x as f32 * 0.05, y as f32 * 0.05, 3, 2.0, 0.5, 7) * 0.5 + 0.5;
        0.92 + 0.08 * n.clamp(0.0, 1.0)
    };
    let mut img = RgbaImage::new(w, h);
    let ink = Rgba([52, 40, 30, 255]);
    let wall_px = (0.35 / mpp).ceil().max(1.0) as i32;
    for y in 0..h {
        for x in 0..w {
            let k = what[(y * w + x) as usize];
            let shade = paper(x, y);
            let px = match k {
                1 => {
                    let v = |c: f32| (c * shade) as u8;
                    Rgba([v(222.0), v(205.0), v(168.0), 255])
                }
                2 => Rgba([34, 28, 24, 255]),
                3 => Rgba([
                    (70.0 * shade) as u8,
                    (112.0 * shade) as u8,
                    (140.0 * shade) as u8,
                    255,
                ]),
                4 => {
                    // stairs: hatched across
                    let stripe = ((x as i32 + y as i32) / (0.4 / mpp).max(1.0) as i32) % 2 == 0;
                    if stripe {
                        Rgba([
                            (196.0 * shade) as u8,
                            (178.0 * shade) as u8,
                            (140.0 * shade) as u8,
                            255,
                        ])
                    } else {
                        Rgba([
                            (222.0 * shade) as u8,
                            (205.0 * shade) as u8,
                            (168.0 * shade) as u8,
                            255,
                        ])
                    }
                }
                5 => ink,
                _ => {
                    // rock: an ink wall along any floor, else nothing
                    let near = (-wall_px..=wall_px).any(|dy| {
                        (-wall_px..=wall_px).any(|dx| {
                            dx * dx + dy * dy <= wall_px * wall_px
                                && at(x as i32 + dx, y as i32 + dy) != 0
                        })
                    });
                    if near { ink } else { Rgba([0, 0, 0, 0]) }
                }
            };
            img.put_pixel(x, y, px);
        }
    }
    let to_px =
        |p: Vec2| -> (i32, i32) { (((p.x - lo.x) / mpp) as i32, ((p.y - lo.y) / mpp) as i32) };
    let disc = |img: &mut RgbaImage, p: Vec2, r_m: f32, col: Rgba<u8>, ring: bool| {
        let (cx, cy) = to_px(p);
        let r = (r_m / mpp).max(2.0);
        let ri = r.ceil() as i32;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                let on = if ring {
                    (d - r).abs() < (0.12 / mpp).max(1.0)
                } else {
                    d <= r
                };
                let (x, y) = (cx + dx, cy + dy);
                if on && x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    img.put_pixel(x as u32, y as u32, col);
                }
            }
        }
    };
    // doors and gates across their doorways; the game's markers
    for p in m
        .pieces
        .iter()
        .filter(|p| (p.pos[1] - ly).abs() < super::plan::LEVEL_STEP * 0.5)
    {
        let at = Vec2::new(p.pos[0], p.pos[2]);
        if p.kind.in_doorway() {
            let across = Vec2::from_angle(-p.yaw).rotate(Vec2::X);
            let col = match p.kind {
                PieceKind::Portcullis | PieceKind::Bars => Rgba([90, 96, 104, 255]),
                PieceKind::SecretDoor => Rgba([150, 60, 160, 255]),
                _ => Rgba([140, 74, 30, 255]),
            };
            let n = (p.width / mpp).ceil() as i32;
            for k in 0..=n {
                let q = at + across * (k as f32 / n.max(1) as f32 - 0.5) * p.width;
                disc(&mut img, q, 0.16, col, false);
            }
        } else if p.kind.is_marker() {
            let (col, r, ring) = match p.kind {
                PieceKind::Arena => (Rgba([170, 40, 30, 255]), p.width * 0.5, true),
                PieceKind::Checkpoint => (Rgba([40, 140, 70, 255]), 0.5, false),
                PieceKind::Loot => (Rgba([214, 168, 40, 255]), 0.45, false),
                _ => (Rgba([40, 110, 170, 255]), 0.45, false),
            };
            disc(&mut img, at, r, col, ring);
        } else if matches!(p.kind, PieceKind::SpikeTrap | PieceKind::PressurePlate) {
            disc(&mut img, at, 0.4, Rgba([120, 30, 30, 255]), true);
        }
    }
    if let Some(e) = exit.filter(|e| (e.y - ly).abs() < super::plan::LEVEL_STEP * 0.5) {
        disc(
            &mut img,
            Vec2::new(e.x, e.z),
            0.9,
            Rgba([80, 120, 220, 255]),
            false,
        );
        disc(
            &mut img,
            Vec2::new(e.x, e.z),
            1.3,
            Rgba([30, 50, 120, 255]),
            true,
        );
    }
    Some(Minimap {
        level,
        image: img,
        origin: lo,
        mpp,
    })
}

/// What a walker can reach.
#[derive(Debug, Default)]
pub struct Walk {
    /// spots with a floor and headroom (0.5 m apart)
    pub spots: usize,
    pub reached: usize,
    /// rooms (id, name, level, share of its floor reached)
    pub rooms: Vec<(u32, String, i32, f32)>,
}

/// How far apart the walk check's spots are, the highest step up, the headroom it needs.
const STEP: f32 = 0.5;
const CLIMB: f32 = 0.6;
const HEAD: f32 = 1.7;

/// From `from` (a foot), which spots and rooms a walker reaches: floors and the tops of daises,
/// decks, stairs and steps with headroom, joined where the step between them is small, round
/// pillars and rails and the closed pieces (`blockers`). Pools and pits are no floor.
pub fn walk(m: &MapDoc, c: &Compiled, from: Vec3, blockers: &[Blocker]) -> Walk {
    let (lo, hi) = (Vec2::new(c.lo.x, c.lo.z), Vec2::new(c.hi.x, c.hi.z));
    let (nx, nz) = (
        ((hi.x - lo.x) / STEP).ceil() as i32,
        ((hi.y - lo.y) / STEP).ceil() as i32,
    );
    let clear = |p: Vec3| -> bool {
        [0.35, 1.0, HEAD - 0.1]
            .iter()
            .all(|h| c.air(p + Vec3::Y * *h))
            && !blockers.iter().any(|b| b.sdf(p + Vec3::Y * 1.0) < 0.05)
    };
    // the standable heights at every spot (and the room each belongs to)
    let mut stands: HashMap<(i32, i32), Vec<(f32, u32)>> = HashMap::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut solids: Vec<(f32, f32)> = Vec::new();
    for j in 0..nz {
        for i in 0..nx {
            let q = lo + Vec2::new(i as f32 + 0.5, j as f32 + 0.5) * STEP;
            c.spans(q, &mut spans);
            if spans.is_empty() {
                continue;
            }
            c.solid_spans(q, &mut solids);
            let mut here: Vec<(f32, u32)> = Vec::new();
            for s in &spans {
                let kind = c.room(s.room).map(|r| r.kind).unwrap_or(RoomKind::Room);
                if !(kind.is_pit() || kind == RoomKind::Pool) {
                    here.push((s.lo, s.room));
                }
                for (_, top) in solids
                    .iter()
                    .filter(|(_, t)| *t > s.lo + 0.05 && *t < s.hi - 0.5)
                {
                    here.push((*top, s.room));
                }
            }
            here.retain(|(y, _)| clear(Vec3::new(q.x, *y, q.y)));
            here.sort_by(|a, b| a.0.total_cmp(&b.0));
            here.dedup_by(|a, b| (a.0 - b.0).abs() < 0.1);
            if !here.is_empty() {
                stands.insert((i, j), here);
            }
        }
    }
    let spots: usize = stands.values().map(|v| v.len()).sum();
    // flood from the spot nearest the start
    let start = {
        let (i, j) = (
            ((from.x - lo.x) / STEP) as i32,
            ((from.z - lo.y) / STEP) as i32,
        );
        let mut best: Option<((i32, i32, usize), f32)> = None;
        for dj in -4..=4 {
            for di in -4..=4 {
                if let Some(v) = stands.get(&(i + di, j + dj)) {
                    for (k, (y, _)) in v.iter().enumerate() {
                        let d = (di * di + dj * dj) as f32 * 0.25 + (y - from.y).abs();
                        if best.is_none_or(|b| d < b.1) {
                            best = Some(((i + di, j + dj, k), d));
                        }
                    }
                }
            }
        }
        best.map(|b| b.0)
    };
    let mut seen: std::collections::HashSet<(i32, i32, usize)> = Default::default();
    if let Some(s) = start {
        let mut q = VecDeque::from([s]);
        seen.insert(s);
        while let Some((i, j, k)) = q.pop_front() {
            let y = stands[&(i, j)][k].0;
            for (di, dj) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let n = (i + di, j + dj);
                let Some(v) = stands.get(&n) else { continue };
                for (kk, (yy, _)) in v.iter().enumerate() {
                    if (yy - y).abs() <= CLIMB && seen.insert((n.0, n.1, kk)) {
                        q.push_back((n.0, n.1, kk));
                    }
                }
            }
        }
    }
    let mut per: HashMap<u32, (usize, usize)> = HashMap::new();
    for (&(i, j), v) in &stands {
        for (k, (_, room)) in v.iter().enumerate() {
            let e = per.entry(*room).or_default();
            e.0 += 1;
            if seen.contains(&(i, j, k)) {
                e.1 += 1;
            }
        }
    }
    let mut rooms: Vec<(u32, String, i32, f32)> = per
        .into_iter()
        .filter_map(|(id, (all, got))| {
            let r = m.plan.room(id)?;
            Some((
                id,
                if r.name.is_empty() {
                    format!("{} {id}", r.kind.name())
                } else {
                    r.name.clone()
                },
                r.level,
                got as f32 / all.max(1) as f32,
            ))
        })
        .collect();
    rooms.sort_by_key(|r| r.0);
    Walk {
        spots,
        reached: seen.len(),
        rooms,
    }
}

/// Where a map's walkers start: its entrance room's middle (else its first room's), on its floor.
pub fn entrance(m: &MapDoc, c: &Compiled) -> Option<Vec3> {
    let r = m
        .plan
        .rooms
        .iter()
        .find(|r| r.role == super::plan::Role::Entrance)
        .or(m.plan.rooms.first())?;
    let p = r.shape.centre();
    let y = c
        .room(r.id)
        .map(|cr| cr.floor_at(p))
        .unwrap_or(m.plan.level_y(r.level));
    Some(Vec3::new(p.x, y, p.y))
}

/// PNG bytes of a minimap.
pub fn png(m: &Minimap) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = m.image.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

// ------------------------------------------------------------------ walk HUD

/// One level's minimap as the walk HUD shows it.
struct HudLevel {
    y: f32,
    tex: bevy_egui::egui::TextureHandle,
    origin: Vec2,
    size: f32,
}

type HudJob = std::thread::JoinHandle<Vec<(i32, f32, bevy_egui::egui::ColorImage, Vec2, f32)>>;

/// The levels' pictures for the dungeon as last drawn (`key`: its fingerprint), and a drawing
/// under way.
#[derive(Default)]
struct HudCache {
    key: u64,
    job: Option<HudJob>,
    levels: Vec<HudLevel>,
}

static HUD: LazyLock<Mutex<HudCache>> = LazyLock::new(Default::default);

/// A minimap padded to a square (rock round it), with the side in metres.
fn square(mm: &Minimap) -> (bevy_egui::egui::ColorImage, f32) {
    let (w, h) = mm.image.dimensions();
    let s = w.max(h);
    let rock = *mm.image.get_pixel(0, 0);
    let mut img = RgbaImage::from_pixel(s, s, rock);
    image::imageops::overlay(&mut img, &mm.image, 0, 0);
    (
        bevy_egui::egui::ColorImage::from_rgba_unmultiplied([s as usize, s as usize], img.as_raw()),
        s as f32 * mm.mpp,
    )
}

/// The walk-mode minimap inside an underground dungeon: its own map of the level the player is
/// on (the world's minimap draws terrain, which a dungeon hasn't got: it came out blank). The
/// levels are drawn on a helper thread whenever the dungeon changes. False when no underground
/// dungeon is open (the world's minimap shows instead).
pub fn hud(
    ctx: &bevy_egui::egui::Context,
    map: &super::DungeonMap,
    world: &super::render::DungeonWorld,
    player: Vec3,
    yaw: f32,
    screen: bevy_egui::egui::Rect,
) -> bool {
    use crate::worldmap::minimap;
    let Some(m) = map
        .0
        .as_ref()
        .filter(|m| m.kind == super::model::MapKind::Interior)
    else {
        return false;
    };
    let mut hud = HUD.lock().unwrap();
    if hud.job.is_none()
        && hud.key != world.want
        && let Some(c) = world.compiled.clone()
    {
        hud.key = world.want;
        let m = Arc::new(m.clone());
        hud.job = Some(std::thread::spawn(move || {
            levels(&m)
                .into_iter()
                .filter_map(|lv| {
                    let mm = minimap(&m, &c, lv, None)?;
                    let (img, size) = square(&mm);
                    Some((lv, m.plan.level_y(lv), img, mm.origin, size))
                })
                .collect()
        }));
    }
    if hud.job.as_ref().is_some_and(|j| j.is_finished())
        && let Some(Ok(done)) = hud.job.take().map(|j| j.join())
    {
        hud.levels = done
            .into_iter()
            .map(|(lv, y, img, origin, size)| HudLevel {
                y,
                tex: ctx.load_texture(
                    format!("dungeon_minimap_L{lv}"),
                    img,
                    bevy_egui::egui::TextureOptions::LINEAR,
                ),
                origin,
                size,
            })
            .collect();
    }
    // the level the player stands on: the highest floor at or below the feet
    let Some(lv) = hud
        .levels
        .iter()
        .filter(|l| l.y <= player.y + 0.5)
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .or(hud.levels.first())
    else {
        return true;
    };
    let (centre, radius) = minimap::hud_spot(screen);
    let painter = ctx.layer_painter(bevy_egui::egui::LayerId::new(
        bevy_egui::egui::Order::Foreground,
        bevy_egui::egui::Id::new("minimap"),
    ));
    let src = minimap::Source {
        texture: lv.tex.id(),
        world_min: lv.origin,
        world_size: lv.size,
    };
    // a dungeon is small: about 40 m round the player
    minimap::draw(
        &painter,
        centre,
        radius,
        &src,
        &minimap::View {
            player: Vec2::new(player.x, player.z),
            yaw,
            radius_m: 40.0,
            rotate: true,
        },
        &[],
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeons::model::MapKind;
    use crate::dungeons::presets;

    fn map(preset: &str) -> (MapDoc, Compiled) {
        let p = presets::get(preset).unwrap();
        let mut m = MapDoc::new(1, "Test", MapKind::Interior);
        m.plan = presets::generate(
            p,
            presets::Size::Small,
            presets::RoomSize::Normal,
            0.0,
            4,
            p.style,
        );
        m.style = p.style.to_string();
        let c = crate::dungeons::render::compile(&m.plan, &m.style);
        (m, c)
    }

    #[test]
    fn every_level_gets_a_minimap_with_floors_walls_and_doors() {
        let (mut m, c) = map("crypt");
        crate::dungeons::pieces::place_for_layout(&mut m);
        for lv in levels(&m) {
            let mm = minimap(&m, &c, lv, None).expect("a minimap");
            let (w, h) = mm.image.dimensions();
            assert!(
                w > 50 && h > 50 && w <= MAX_PX + 30 && h <= MAX_PX + 30,
                "{w}×{h}"
            );
            let floor = mm
                .image
                .pixels()
                .filter(|p| p[1] > 150 && p[3] == 255)
                .count();
            let ink = mm
                .image
                .pixels()
                .filter(|p| p[0] == 52 && p[1] == 40)
                .count();
            assert!(
                floor > 1000 && ink > 200,
                "level {lv}: {floor} floor, {ink} ink"
            );
        }
        assert!(!png(&minimap(&m, &c, levels(&m)[0], None).unwrap()).is_empty());
        // (to look at them)
        if let Ok(dir) = std::env::var("WB_MINIMAP_OUT") {
            for lv in levels(&m) {
                let _ = std::fs::write(
                    format!("{dir}/minimap_crypt_L{lv}.png"),
                    png(&minimap(&m, &c, lv, None).unwrap()),
                );
            }
        }
    }

    #[test]
    fn a_template_dungeon_is_walkable_from_its_entrance_and_a_shut_gate_cuts_it() {
        let (mut m, c) = map("crypt");
        let from = entrance(&m, &c).expect("an entrance");
        let open = walk(&m, &c, from, &[]);
        assert!(open.spots > 500, "{} spots", open.spots);
        let unreached: Vec<_> = open.rooms.iter().filter(|r| r.3 < 0.5).collect();
        assert!(unreached.is_empty(), "rooms out of reach: {unreached:?}");
        // the layout's doors and gates, all shut: fewer rooms within reach (or the same, if none
        // stand between the entrance and the rest)
        crate::dungeons::pieces::place_for_layout(&mut m);
        let shut = crate::dungeons::pieces::blockers(&m.pieces, &Default::default());
        let closed = walk(&m, &c, from, &shut);
        assert!(closed.reached <= open.reached);
        if !shut.is_empty() {
            assert!(
                closed.reached < open.reached,
                "shut doors cut nothing off ({} of {})",
                closed.reached,
                open.reached
            );
        }
    }
}
