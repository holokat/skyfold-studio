//! Realm shapes in the Dungeons tab: the shape, size and seed a realm's land is made from
//! (`terrain::realm_land`), the shape and size each realm starts from, the create flow's picker
//! (a little map per shape, the size, and a live preview of the land that Shuffle rolls again;
//! previews are made on a worker thread and cached, so the panel never waits), making the land
//! when the realm is first opened, and the `dungeons_realm_preview` MCP tool.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, FontId, RichText};
use egui_phosphor::regular as icon;
use serde_json::{Value, json};

use super::edit::NewDungeon;
use super::model::{DungeonDef, MapKind};
use super::presets::RealmPreset;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};
use crate::terrain::realm_land::{self, Detail, Palette};
pub use crate::terrain::realm_land::{RealmShape, RealmSize};
use crate::ui::theme::*;
use crate::ui::widgets::*;

/// A realm's land as chosen in the create flow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choice {
    pub shape: RealmShape,
    pub size: RealmSize,
    pub seed: u32,
}

impl Default for Choice {
    fn default() -> Self {
        let (shape, size) = defaults("");
        Choice {
            shape,
            size,
            seed: random_seed(),
        }
    }
}

impl Choice {
    /// A realm's own shape and size (the seed stays).
    pub fn for_realm(realm: &str, seed: u32) -> Choice {
        let (shape, size) = defaults(realm);
        Choice { shape, size, seed }
    }
}

/// The shape and size a realm starts from (the create flow and MCP can change them).
pub fn defaults(realm: &str) -> (RealmShape, RealmSize) {
    match realm {
        "desert_realm" => (RealmShape::CoastalShelf, RealmSize::Medium),
        "frozen_realm" => (RealmShape::CrescentBay, RealmSize::Medium),
        "jungle_realm" => (RealmShape::IslandChain, RealmSize::Medium),
        "ember_realm" => (RealmShape::EnclosedValley, RealmSize::Medium),
        "twilight_realm" => (RealmShape::Lakeland, RealmSize::Medium),
        "wild_realm" => (RealmShape::RoundIsle, RealmSize::Large),
        super::compound::LAYOUT => (RealmShape::EnclosedValley, RealmSize::Small),
        super::sheets::LAYOUT => (RealmShape::EnclosedValley, RealmSize::Cozy),
        _ => (RealmShape::RoundIsle, RealmSize::Medium),
    }
}

/// The colours a realm's preview is painted in.
pub fn palette(realm: &str) -> Palette {
    match realm {
        "desert_realm" => Palette::Sand,
        "frozen_realm" => Palette::Snow,
        "jungle_realm" => Palette::Jungle,
        "ember_realm" => Palette::Ember,
        "twilight_realm" => Palette::Dusk,
        super::compound::LAYOUT => Palette::Jungle,
        _ => Palette::Green,
    }
}

/// How far a desert realm's sand fills its land's basins (`biomegen::generate_realm`): a walled
/// realm or a plateau keeps its shape under its dunes.
pub fn dune_fill(def: &DungeonDef) -> f32 {
    match def.shape {
        Some(s) if s.walled() || s == RealmShape::HighlandPlateau => 0.0,
        _ => 1.0,
    }
}

/// The water a realm's lake takes (`water::lakes::SHAPES`).
fn lake_style(realm: &str) -> &'static str {
    match realm {
        "desert_realm" => "oasis",
        "frozen_realm" => "frozen_lake",
        "jungle_realm" => "tropical_lagoon",
        "ember_realm" => "volcanic_lake",
        "twilight_realm" => "mirror_lake",
        _ => "lake",
    }
}

/// A new seed (short enough to read out).
pub fn random_seed() -> u32 {
    static N: AtomicU32 = AtomicU32::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    crate::noise::hash_u32((t ^ (t >> 32)) as u32 ^ N.fetch_add(0x9e37_79b9, Ordering::Relaxed))
        % 999_983
        + 1
}

/// The seed a realm made without one gets (from its id, like realms before seeds).
pub fn seed_for(id: u32) -> u32 {
    (id.wrapping_mul(0x9e37_79b9) ^ 0x5eed) % 999_983 + 1
}

/// The last seed a realm was made with: the picker rolls a new one for the next realm.
static USED: AtomicU32 = AtomicU32::new(0);

/// A new dungeon's realm land as it is kept: (shape, size, seed); an underground one has none.
pub fn resolve(n: &NewDungeon, id: u32) -> (Option<RealmShape>, RealmSize, Option<u32>) {
    if n.kind != MapKind::Realm {
        return (None, RealmSize::default(), None);
    }
    let (shape, size) = defaults(&n.layout);
    let seed = n.seed.unwrap_or_else(|| seed_for(id));
    USED.store(seed, Ordering::Relaxed);
    (
        Some(n.shape.unwrap_or(shape)),
        n.realm_size.unwrap_or(size),
        Some(seed),
    )
}

// ------------------------------------------------------------------ previews (worker threads)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    shape: RealmShape,
    size: RealmSize,
    seed: u32,
    palette: Palette,
    /// image side (px) and the grid it's generated on (cells per side)
    px: u16,
    res: u16,
}

#[derive(Default)]
struct Cache {
    /// None: being made
    images: HashMap<Key, Option<Arc<Vec<u8>>>>,
    order: VecDeque<Key>,
    textures: HashMap<Key, egui::TextureHandle>,
    /// the preview shown last (shown dimmed while the next one is made)
    last: Option<Key>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Cache) -> R) -> R {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Cache::default))
}

/// The preview (RGBA) once it's made; the first ask starts making it on a worker thread.
fn image(key: Key) -> Option<Arc<Vec<u8>>> {
    let (have, start) = with(|c| match c.images.get(&key) {
        Some(i) => (i.clone(), false),
        None => {
            c.images.insert(key, None);
            c.order.push_back(key);
            // (the oldest go; thumbnails are asked for every frame, so they stay young enough)
            while c.order.len() > 64 {
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
            let land = realm_land::generate(
                key.shape,
                key.size,
                key.seed,
                Detail::Preview(key.res as usize),
            );
            let img = Arc::new(realm_land::render(&land, key.px as usize, key.palette));
            with(|c| {
                c.images.insert(key, Some(img));
            });
        });
    }
    have
}

fn texture(ctx: &egui::Context, key: Key) -> Option<egui::TextureHandle> {
    if let Some(t) = with(|c| c.textures.get(&key).cloned()) {
        return Some(t);
    }
    let img = image(key)?;
    let ci = egui::ColorImage::from_rgba_unmultiplied([key.px as usize, key.px as usize], &img);
    let t = ctx.load_texture(
        format!(
            "realm_{}_{}_{}_{:?}_{}",
            key.shape.id(),
            key.size.id(),
            key.seed,
            key.palette,
            key.px
        ),
        ci,
        egui::TextureOptions::LINEAR,
    );
    with(|c| {
        c.textures.insert(key, t.clone());
        c.order.retain(|k| *k != key);
        c.order.push_back(key);
    });
    Some(t)
}

/// The little map on a shape's tile: one fixed roll of each shape.
fn thumb(shape: RealmShape, palette: Palette) -> Key {
    let seed = match shape {
        RealmShape::RoundIsle => 2,
        RealmShape::LongIsle => 2,
        RealmShape::CrescentBay => 1,
        RealmShape::TwinIsles => 1,
        RealmShape::IslandChain => 2,
        RealmShape::CoastalShelf => 2,
        RealmShape::EnclosedValley => 3,
        RealmShape::HighlandPlateau => 3,
        RealmShape::Lakeland => 1,
        RealmShape::Canyon => 4,
        RealmShape::Glade => 1,
    };
    Key {
        shape,
        size: RealmSize::Medium,
        seed,
        palette,
        px: 96,
        res: 72,
    }
}

fn preview_key(c: &Choice, palette: Palette) -> Key {
    Key {
        shape: c.shape,
        size: c.size,
        seed: c.seed,
        palette,
        px: 224,
        res: 160,
    }
}

// ------------------------------------------------------------------ the create flow's picker

/// Create a dungeon › Realm: the land's shape (a little map each), its size, and a live preview of
/// the land (Shuffle rolls another). `realm`: the realm picked (its colours). Returns true when
/// anything changed.
pub fn picker(ui: &mut egui::Ui, c: &mut Choice, realm: &str) -> bool {
    // (the seed made a realm already: the next realm gets land of its own)
    if c.seed == USED.load(Ordering::Relaxed) {
        c.seed = random_seed();
    }
    let ctx = ui.ctx().clone();
    let pal = palette(realm);
    let mut changed = false;
    let mut waiting = false;
    ui.add_space(6.0);
    ui.label(
        RichText::new("Land")
            .font(FontId::new(13.0, semibold()))
            .color(TEXT),
    );
    let cols = 4;
    let gap = 6.0;
    let w = ((ui.available_width() - gap * (cols - 1) as f32) / cols as f32).floor();
    egui::Grid::new("realm_shapes")
        .spacing([gap, gap])
        .show(ui, |ui| {
            for (i, sh) in RealmShape::ALL.iter().enumerate() {
                let tex = texture(&ctx, thumb(*sh, pal));
                waiting |= tex.is_none();
                if shape_tile(
                    ui,
                    tex.as_ref(),
                    sh.short(),
                    c.shape == *sh,
                    egui::vec2(w, w * 0.72 + 18.0),
                )
                .on_hover_text(sh.hint())
                .clicked()
                    && c.shape != *sh
                {
                    c.shape = *sh;
                    changed = true;
                }
                if i % cols == cols - 1 {
                    ui.end_row();
                }
            }
        });
    ui.add_space(6.0);
    // (how far across each is shows on the preview: ½, 1, 2 and 4 km)
    let mut sz = c.size;
    let opts: Vec<(RealmSize, &str)> = RealmSize::ALL.iter().map(|s| (*s, s.name())).collect();
    if segmented(ui, &mut sz, &opts) {
        c.size = sz;
        changed = true;
    }
    ui.add_space(6.0);
    // the preview: the land these make (the last one, dimmed, while the next is drawn)
    let key = preview_key(c, pal);
    let fresh = texture(&ctx, key);
    let shown = fresh.clone().map(|t| (t, true)).or_else(|| {
        with(|cc| cc.last.and_then(|k| cc.textures.get(&k).cloned())).map(|t| (t, false))
    });
    if fresh.is_some() {
        with(|cc| cc.last = Some(key));
    } else {
        waiting = true;
    }
    let side = ui.available_width().min(236.0);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), side), egui::Sense::click());
    let img = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
    let p = ui.painter().clone();
    p.rect_filled(img, egui::CornerRadius::same(10), CARD_BG);
    match &shown {
        Some((t, ready)) => {
            let tint = if *ready {
                Color32::WHITE
            } else {
                Color32::from_gray(120)
            };
            egui::Image::new((t.id(), img.size()))
                .corner_radius(10)
                .tint(tint)
                .paint_at(ui, img);
        }
        None => {
            p.text(
                img.center(),
                egui::Align2::CENTER_CENTER,
                "Drawing the land…",
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
    }
    // what it is, how big
    let caption = p.layout_no_wrap(
        format!("{} · {} across", c.shape.name(), c.size.across()),
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
    if resp.on_hover_text("The land this realm gets (a quick look: the real one is finer, with rivers worn into it). The white ring is where players arrive. Click for another roll").clicked() {
        c.seed = random_seed();
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
        if r.on_hover_text("Another land of this shape and size")
            .clicked()
        {
            c.seed = random_seed();
            changed = true;
        }
        ui.label(
            RichText::new(format!("seed {}", c.seed))
                .small()
                .color(TEXT_DIM),
        );
    });
    dim(ui, c.shape.hint());
    if waiting {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(50));
    }
    changed
}

/// A shape's tile: its little map and its name.
fn shape_tile(
    ui: &mut egui::Ui,
    tex: Option<&egui::TextureHandle>,
    label: &str,
    selected: bool,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter().clone();
    p.rect_filled(
        rect,
        egui::CornerRadius::same(9),
        if selected {
            ACCENT_SOFT
        } else if resp.hovered() {
            CARD_HOVER
        } else {
            CARD_BG
        },
    );
    let pic = egui::Rect::from_min_max(
        rect.min + egui::vec2(4.0, 4.0),
        egui::pos2(rect.max.x - 4.0, rect.max.y - 18.0),
    );
    match tex {
        Some(t) => {
            // (the map is square: show its middle)
            let a = pic.height() / pic.width();
            let uv = egui::Rect::from_min_max(
                egui::pos2(0.0, 0.5 - a * 0.5),
                egui::pos2(1.0, 0.5 + a * 0.5),
            );
            egui::Image::new((t.id(), pic.size()))
                .uv(uv)
                .corner_radius(6)
                .paint_at(ui, pic);
        }
        None => {
            p.rect_filled(
                pic,
                egui::CornerRadius::same(6),
                Color32::from_rgba_premultiplied(30, 60, 90, 120),
            );
        }
    }
    if selected {
        p.rect_stroke(
            rect,
            egui::CornerRadius::same(9),
            egui::Stroke::new(1.5, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        egui::pos2(rect.center().x, rect.bottom() - 9.0),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if selected { Color32::WHITE } else { TEXT_DIM },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ------------------------------------------------------------------ making the land

/// What `make_land` made: where players arrive, and what waits for the biomes.
pub struct Made {
    pub arrival: Vec2,
    lake: Option<Vec<Vec2>>,
    clearing: Vec<f32>,
}

/// A new realm's land (the world is replaced), from the shape, size and seed it was made with; a
/// realm made before shapes gets the classic island it always got.
pub fn make_land(d: &mut WorldData, def: &DungeonDef, rp: &RealmPreset, seed: u32) -> Made {
    let Some(shape) = def.shape else {
        crate::io::new_world(d, rp.world, 512, 4.0, seed);
        return Made {
            arrival: Vec2::ZERO,
            lake: None,
            clearing: vec![],
        };
    };
    let t0 = std::time::Instant::now();
    let land = realm_land::generate(shape, def.realm_size, seed, Detail::Full);
    info!(
        "realm land: {} {} seed {seed}: {:.0}% land, arrive at {:?}, in {:.2} s",
        shape.id(),
        def.realm_size.id(),
        land.land_share * 100.0,
        land.arrival,
        t0.elapsed().as_secs_f32()
    );
    let realm_land::Land {
        res,
        cell,
        heights,
        arrival,
        lake,
        clearing,
        ..
    } = land;
    crate::io::new_world_from(d, heights, res, cell, seed);
    Made {
        arrival,
        lake,
        clearing,
    }
}

/// After the biomes: the lake filled with the realm's own water, a glade's clearing kept clear of
/// trees (grass and flowers stay) and the hills round it thickly wooded.
pub fn finish_land(d: &mut WorldData, made: &Made, rp: &RealmPreset) {
    if let Some(outline) = &made.lake {
        let WorldData {
            terrain,
            history,
            store,
            water,
            lib,
            ..
        } = &mut *d;
        if let Err(e) = crate::water::create_drawn_lake(
            terrain,
            history,
            store,
            lib.as_deref(),
            water,
            outline,
            crate::water::lakes::shape_by_id(lake_style(rp.id)),
            1.0,
            1.0,
        ) {
            warn!("realm lake: {e}");
        }
    }
    if !made.clearing.is_empty() {
        let WorldData {
            terrain, nature, ..
        } = &mut *d;
        let (n, half, cell) = (terrain.n(), terrain.half(), terrain.cell);
        if made.clearing.len() == n * n {
            let clearing = &made.clearing;
            nature.set_texels(Vec2::splat(-half), Vec2::splat(half), |p| {
                let g = (p + Vec2::splat(half)) / cell;
                let (i, j) = (
                    (g.x.round().max(0.0) as usize).min(n - 1),
                    (g.y.round().max(0.0) as usize).min(n - 1),
                );
                let c = clearing[j * n + i];
                // trees, rocks, plants, debris (`nature::rules::CATS`): the clearing open, the
                // hills round it thickly wooded
                if c > 0.02 {
                    Some(([1.0 - c, 1.0 - c * 0.6, 1.0, 1.0 - c * 0.5], 0))
                } else if c < -0.02 {
                    Some(([1.0 - c * 1.6, 1.0, 1.0 - c * 0.3, 1.0 - c * 0.5], 0))
                } else {
                    None
                }
            });
        }
    }
}

// ------------------------------------------------------------------ MCP

/// What `dungeons_catalog` lists about realm land.
pub fn catalog_json() -> (Value, Value) {
    (
        Value::Array(RealmShape::ALL.iter().map(|s| json!({ "id": s.id(), "name": s.name(), "about": s.hint(), "walled": s.walled() })).collect()),
        Value::Array(RealmSize::ALL.iter().map(|s| json!({ "id": s.id(), "name": s.name(), "km": s.km(), "cells": s.grid().0, "cell_m": s.grid().1 })).collect()),
    )
}

/// `WB_REALM_PICKER=<realm id>[,<shape>[,<size>[,<seed>]]]`: opens Create a dungeon › Realm on
/// that realm (automated screenshots of the picker).
fn test_hook(
    mut frame: Local<u32>,
    mut tool: ResMut<super::ui::DungeonTool>,
    mut ed: ResMut<crate::editor::Editor>,
) {
    *frame += 1;
    if *frame != 240 {
        return;
    }
    let Ok(spec) = std::env::var("WB_REALM_PICKER") else {
        return;
    };
    let v: Vec<&str> = spec.split(',').map(str::trim).collect();
    let realm = v.first().copied().unwrap_or("desert_realm");
    let mut c = Choice::for_realm(realm, v.get(3).and_then(|s| s.parse().ok()).unwrap_or(7));
    if let Some(sh) = v.get(1).and_then(|s| RealmShape::from_id(s)) {
        c.shape = sh;
    }
    if let Some(sz) = v.get(2).and_then(|s| RealmSize::from_id(s)) {
        c.size = sz;
    }
    ed.tab = crate::editor::Tab::Dungeons;
    let t = &mut *tool;
    t.creating = true;
    t.pick = None;
    t.describe.page = false;
    t.new_realm = true;
    t.new_layout = realm.into();
    t.new_land = c;
}

pub(super) fn register(app: &mut App) {
    app.add_systems(Update, test_hook);
    app.mcp_tool(McpTool {
        name: "dungeons_realm_preview",
        description: "A preview map (PNG) of a realm's land before it is made: `shape` (dungeons_catalog realm_shapes), `realm_size` (cozy ½ km, small 1 km, medium 2 km, large 4 km) and `seed` give the same land dungeons_create makes from them (coarser here, before erosion). Sea by depth, land by height with hill shading, a white ring where players arrive. `realm` paints it in that realm's colours (dungeons_catalog realms); `px` is its size (64..768, default 384); `save_path` also writes the PNG there. Try a few seeds, then pass the one you like to dungeons_create.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "shape": { "type": "string", "enum": RealmShape::ALL.map(|s| s.id()) },
                "realm_size": { "type": "string", "enum": RealmSize::ALL.map(|s| s.id()) },
                "seed": { "type": "integer", "minimum": 0 },
                "realm": { "type": "string", "description": "a realm id, for its colours" },
                "px": { "type": "integer", "minimum": 64, "maximum": 768 },
                "save_path": { "type": "string" },
            }, "required": ["shape"] })
        },
        run: |_, a| preview_tool(&a),
    });
}

fn preview_tool(a: &Value) -> Result<Vec<McpContent>, String> {
    let shape_s: String =
        opt_arg(a, "shape")?.ok_or("give a `shape` (dungeons_catalog realm_shapes)")?;
    let shape = RealmShape::from_id(&shape_s)
        .ok_or_else(|| format!("unknown shape `{shape_s}` (dungeons_catalog realm_shapes)"))?;
    let size = match opt_arg::<String>(a, "realm_size")? {
        Some(s) => RealmSize::from_id(&s)
            .ok_or_else(|| format!("unknown realm_size `{s}`: cozy, small, medium or large"))?,
        None => RealmSize::Medium,
    };
    let seed: u32 = opt_arg(a, "seed")?.unwrap_or(1);
    let realm: String = opt_arg(a, "realm")?.unwrap_or_default();
    let px = opt_arg::<u32>(a, "px")?.unwrap_or(384).clamp(64, 768) as usize;
    let t0 = std::time::Instant::now();
    let land = realm_land::generate(
        shape,
        size,
        seed,
        Detail::Preview((px * 3 / 4).clamp(96, 320)),
    );
    let rgba = realm_land::render(&land, px, palette(&realm));
    let mut png = Vec::new();
    {
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgba, px as u32, px as u32, image::ExtendedColorType::Rgba8)
            .map_err(|e| e.to_string())?;
    }
    let saved = match opt_arg::<String>(a, "save_path")? {
        Some(p) if !p.trim().is_empty() => {
            std::fs::write(p.trim(), &png).map_err(|e| format!("couldn't write {p}: {e}"))?;
            Some(p.trim().to_string())
        }
        _ => None,
    };
    let half = land.size() * 0.5;
    Ok(vec![
        McpContent::Png(png),
        McpContent::Json(json!({
            "shape": shape.id(),
            "realm_size": size.id(),
            "km": size.km(),
            "seed": seed,
            "land_share": (land.land_share * 100.0).round() / 100.0,
            "arrival": [land.arrival.x.round(), land.arrival.y.round()],
            "map": { "from": [-half, -half], "to": [half, half] },
            "saved": saved,
            "ms": t0.elapsed().as_millis() as u64,
        })),
    ])
}
