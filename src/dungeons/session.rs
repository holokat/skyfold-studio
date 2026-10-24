//! Going into a dungeon's map and back out: the world is saved, the dungeon's map (a world folder
//! inside the world's: `<world>.world/dungeons/<folder>/`) is opened — made first if it doesn't
//! exist yet, from the dungeon's layout or realm preset — and "Back to world" saves the map and
//! opens the world again. An untitled world goes to the recovery slot meanwhile (and its dungeons
//! move with it when it is saved somewhere: `carry_maps`).

use std::path::{Path, PathBuf};

use bevy::prelude::*;

use super::model::*;
use super::{mood, presets, render::ROOF};
use crate::io::WorldData;

/// The open session: which world to go back to.
#[derive(Resource, Default)]
pub struct Session {
    /// the world's folder while one of its dungeons' maps is open
    pub world: Option<PathBuf>,
    /// the world had no folder (it waits in the recovery slot)
    pub world_untitled: bool,
    pub world_name: String,
    /// where a walker comes out after going through a portal (position, heading)
    pub arrive: Option<(Vec3, f32)>,
    /// open this dungeon once the (new, unsaved) world has been saved
    pub open_after_save: Option<u32>,
    /// the way in a walker came through (dungeon, entrance): where they come back out
    pub via: Option<(u32, u32)>,
    /// the open map was just made: light and dress it once it's laid out
    pub fresh: bool,
}

/// Where a walker comes out of dungeon `id`'s way in, in the world: in front of the way in they
/// came through (else its first), looking away from it. (position, walker yaw)
pub fn world_arrival(d: &WorldData, id: u32, via: Option<u32>) -> Option<(Vec3, f32)> {
    let def = d.dungeons.get(id)?;
    let e = via
        .and_then(|v| def.entrances.iter().find(|e| e.id == v))
        .or(def.entrances.first())?;
    let f = Vec3::new(e.yaw.sin(), 0.0, e.yaw.cos());
    let p = Vec3::from_array(e.pos) + f * 2.6;
    // (a walker's yaw looks along (−sin, −cos))
    Some((
        Vec3::new(p.x, e.pos[1] + 0.2, p.z),
        e.yaw + std::f32::consts::PI,
    ))
}

/// Where a walker arrives in a dungeon's map: its player start.
pub fn dungeon_arrival(d: &WorldData) -> Option<(Vec3, f32)> {
    let ps = d.gameplay.player_start?;
    Some((
        Vec3::from_array(ps.pos) + Vec3::Y * 0.1,
        ps.yaw - std::f32::consts::FRAC_PI_2,
    ))
}

/// Caves whose dungeon is switched on in their own settings (Terrain › Caves) get a dungeon here,
/// with the cave's end as its way in; a cave whose dungeon is switched off loses that way in.
pub(super) fn link_caves(
    mut list: ResMut<Dungeons>,
    caves: Res<crate::caves::Caves>,
    map: Res<DungeonMap>,
) {
    if !caves.is_changed() || map.0.is_some() {
        return;
    }
    for c in &caves.list {
        let Some(dg) = &c.dungeon else { continue };
        if list.by_cave(c.id).is_some() {
            continue;
        }
        let id = list.alloc_id();
        let eid = list.alloc_entrance();
        let name = if dg.name.is_empty() {
            format!("{} Depths", c.name)
        } else {
            dg.name.clone()
        };
        let folder = list.folder_for(id, &name);
        list.list.push(DungeonDef {
            id,
            name,
            kind: MapKind::Interior,
            instancing: dg.instancing,
            levels: [10, 15],
            players: 5,
            entrances: vec![Entrance {
                id: eid,
                kind: EntranceKind::Cave,
                pos: dg.door,
                yaw: dg.door_yaw,
                object: None,
                cave: Some(c.id),
                color: PortalColor::Arcane,
                label: c.name.clone(),
            }],
            folder,
            layout: "great_caverns".into(),
            style: String::new(),
            size: super::presets::Size::Medium,
            room_size: Default::default(),
            irregularity: 0.0,
            art: super::styles::Art::HandPainted,
            mood: String::new(),
            notes: String::new(),
            shape: None,
            realm_size: Default::default(),
            seed: None,
            compound: None,
            sheet: None,
        });
    }
    // switched off: the cave's way in goes (the dungeon and its map stay)
    for d in &mut list.list {
        d.entrances.retain(|e| {
            e.kind != EntranceKind::Cave
                || e.cave
                    .and_then(|c| caves.get(c))
                    .is_some_and(|c| c.dungeon.is_some())
        });
    }
}

/// What `enter` says when the world has never been saved (its dungeons need its folder).
pub const NEEDS_SAVE: &str = "Save the world first: its dungeons are kept in its folder";

/// The world's folder (the recovery slot for an untitled world).
pub fn world_dir(d: &WorldData) -> PathBuf {
    d.meta.path.clone().unwrap_or_else(crate::io::recovery_path)
}

/// A dungeon's map folder inside its world's.
pub fn map_dir(world: &Path, def: &DungeonDef) -> PathBuf {
    world.join("dungeons").join(&def.folder)
}

/// The world a dungeon map belongs to (its folder's grandparent), when it is one.
pub fn world_of(map_dir: &Path) -> Option<PathBuf> {
    let parent = map_dir.parent()?;
    (parent.file_name()? == "dungeons")
        .then(|| parent.parent().map(Path::to_path_buf))
        .flatten()
}

/// Opens dungeon `id`'s map (making it the first time). The world is saved first.
pub fn enter(d: &mut WorldData, s: &mut Session, id: u32) -> Result<(), String> {
    if d.dungeon_map.0.is_some() {
        return Err("A dungeon's map is open: go back to the world first".into());
    }
    let def = d
        .dungeons
        .get(id)
        .cloned()
        .ok_or_else(|| format!("no dungeon {id}"))?;
    // (a world that was never saved has no folder of its own to keep its dungeons in: the
    // recovery slot is no home, autosave clears it)
    if d.meta.path.is_none() || d.meta.path.as_deref() == Some(crate::io::recovery_path().as_path())
    {
        return Err(NEEDS_SAVE.into());
    }
    let world = world_dir(d);
    let untitled = false;
    let name = d.meta.name.clone();
    // (the way back out looks like the way in players came through: read while the world is open)
    let way_in =
        super::portal::way_in_look(&def, s.via.filter(|v| v.0 == id).map(|v| v.1), &d.store);
    crate::io::save_world(&world, d)?;
    let dir = map_dir(&world, &def);
    if dir.join("world.json").is_file() {
        crate::io::load_world(&dir, d)?;
        // (renamed in the world meanwhile)
        if let Some(m) = &mut d.dungeon_map.0 {
            m.id = def.id;
            m.name = def.name.clone();
        }
    } else {
        create_map(d, &def, &dir)?;
        crate::io::save_world(&dir, d)?;
        s.fresh = def.kind == MapKind::Interior;
    }
    let way_out = super::portal::open_way_out(d, way_in);
    d.meta.path = Some(dir);
    d.meta.name = def.name.clone();
    // Loading an older sheet can add its missing bridge decks. Keep that unsaved repair even
    // when the portal itself was already present.
    d.meta.dirty |= way_out;
    s.world = Some(world);
    s.world_untitled = untitled;
    s.world_name = name;
    Ok(())
}

/// A new underground map opens ready to explore: Auto-light and Dress run once on its first
/// layout (each its own undo step, so either can be taken back).
pub(super) fn setup_fresh(
    mut s: ResMut<Session>,
    map: Res<DungeonMap>,
    world: Res<super::render::DungeonWorld>,
    mut tool: ResMut<super::ui::DungeonTool>,
) {
    if !s.fresh {
        return;
    }
    if !map.interior() {
        s.fresh = false;
        return;
    }
    if world.compiled.is_some() {
        s.fresh = false;
        let density = tool.light_density;
        tool.requests
            .push(super::ui::Request::AutoLight { density });
        tool.requests.push(super::ui::Request::Dress);
    }
}

/// Saves the dungeon's map and opens its world again (the dungeon keeps its new name).
pub fn back(d: &mut WorldData, s: &mut Session) -> Result<(), String> {
    let map = d.dungeon_map.0.clone().ok_or("No dungeon's map is open")?;
    let here = d
        .meta
        .path
        .clone()
        .ok_or("The dungeon's map has no folder")?;
    let world = s
        .world
        .clone()
        .or_else(|| world_of(&here))
        .ok_or("Couldn't tell which world this dungeon belongs to")?;
    crate::io::save_world(&here, d)?;
    crate::io::load_world(&world, d)?;
    if s.world_untitled || world == crate::io::recovery_path() {
        d.meta.path = None;
        d.meta.dirty = true;
        if !s.world_name.is_empty() {
            d.meta.name = s.world_name.clone();
        }
    }
    if let Some(def) = d.dungeons.get_mut(map.id)
        && def.name != map.name
    {
        def.name = map.name.clone();
        d.meta.dirty = true;
    }
    d.dungeons.selected = Some(map.id);
    s.world = None;
    s.fresh = false;
    s.world_untitled = false;
    Ok(())
}

/// Makes a dungeon's map for the first time: an underground one (a flat hidden ground far above,
/// no sea, no nature; the layout's plan, style and mood) or a realm (a new world of the realm's
/// kind, biomes and look).
fn create_map(d: &mut WorldData, def: &DungeonDef, dir: &Path) -> Result<(), String> {
    let seed = (def.id.wrapping_mul(0x9e37_79b9) ^ 0x5eed) & 0x7fff_ffff;
    match def.kind {
        MapKind::Interior => {
            let (res, cell) = (256usize, 4.0f32);
            crate::io::new_world_from(d, vec![ROOF; (res + 1) * (res + 1)], res, cell, seed);
            d.terrain.sea_level = -2000.0;
            *d.amounts = crate::objects::NatureAmounts {
                grass: 0.0,
                grass_height: 1.0,
                plants: 0.0,
                flowers: 0.0,
                mushrooms: 0.0,
                berries: 0.0,
                trees: 0.0,
                rocks: 0.0,
                debris: 0.0,
                applied_plants: 0.0,
            };
            d.lighting.cloud_coverage = 0.0;
            d.lighting.precipitation = false;
            d.lighting.stars = false;
            d.lighting.weather_override = None;
            let preset = presets::get(&def.layout).unwrap_or(&presets::all()[0]);
            let style = if def.style.is_empty() {
                preset.style_for(def.art).to_string()
            } else {
                def.style.clone()
            };
            let mut doc = MapDoc::new(def.id, &def.name, MapKind::Interior);
            doc.art = def.art;
            doc.plan = presets::generate(
                preset,
                def.size,
                def.room_size,
                def.irregularity,
                seed,
                &style,
            );
            doc.style = style;
            doc.mood = mood::preset(if def.mood.is_empty() {
                preset.mood
            } else {
                &def.mood
            })
            .unwrap_or_default();
            // the demo track that suits it (Look › Mood › Music changes it)
            doc.music.demo = super::sound::demo_for(preset.id, &doc.mood.preset);
            doc.sky = preset.sky();
            // a template's doors and gates in its doorways (a dungeon drawn from scratch has none)
            if preset.id != "empty" {
                super::pieces::place_for_layout(&mut doc);
            }
            frame_entrance(d, &doc);
            d.dungeon_map.0 = Some(doc);
        }
        MapKind::Realm => {
            // (a temple compound takes its theme's sky, biomes and music)
            let rp = super::compound::realm_preset(def)
                .or_else(|| super::sheets::realm_preset(def))
                .or_else(|| presets::realm(&def.layout))
                .unwrap_or(&presets::realms()[0]);
            // its land: the shape, size and seed it was made with (`realm_shapes`)
            let seed = def.seed.unwrap_or(seed);
            let land = super::realm_shapes::make_land(d, def, rp, seed);
            // a temple compound: laid out on the land, the land shaped for it
            let compound = super::compound::begin(d, def, seed);
            // a design sheet: its blueprint laid on the land, the land shaped to it
            let sheet = super::sheets::begin(d, def, seed, dir);
            d.history.begin("Generate biomes");
            let (climate, only) = super::compound::biomes(def)
                .or_else(|| super::sheets::biomes(def))
                .unwrap_or((rp.climate, rp.only));
            let g = crate::biomegen::generate_realm(
                &mut d.terrain,
                &mut d.zones,
                &mut d.history,
                &mut d.water,
                seed,
                climate,
                only,
                super::realm_shapes::dune_fill(def),
            );
            crate::biomegen::thin_nature_on_sand(&mut d.nature, &d.terrain, &g.sand);
            // the desert's one green place: an oasis pool ringed by palms and grass (a compound
            // has lakes of its own)
            if let Some(outline) = (!g.sand.is_empty() && def.compound.is_none() && sheet.is_none())
                .then(|| crate::biomegen::oasis_outline(&d.terrain, seed))
                .flatten()
            {
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
                    &outline,
                    crate::water::lakes::shape_by_id("oasis"),
                    1.0,
                    1.4,
                ) {
                    warn!("desert realm oasis: {e}");
                }
            }
            // its lake, a glade's clearing
            // (a sheet's land is its own: no lake or clearing of the realm's)
            if sheet.is_none() {
                super::realm_shapes::finish_land(d, &land, rp);
            }
            // the compound's lakes, island, paint and nature; where players arrive; its build
            // queued for the next frames
            let compound = super::compound::end(d, def, compound);
            let sheet = super::sheets::end(d, def, sheet);
            d.history.commit(&d.store);
            d.history.clear();
            d.lighting.look.select(rp.look);
            d.lighting.hour = rp.hour;
            d.gameplay.ore_fill_pending = true;
            let mut doc = MapDoc::new(def.id, &def.name, MapKind::Realm);
            doc.music.demo = super::sound::demo_for(rp.id, "");
            // `sheets::end` installed its bridge roads after water settled.  Carry the source
            // ids into the map document before the first save, so reload never adds duplicates.
            doc.sheet_bridges = super::sheets::take_pending_state();
            d.dungeon_map.0 = Some(doc);
            if compound || sheet {
                // (they arrive on the compound's entrance plaza, at the sheet's start)
                return finish_map(d, def, dir);
            }
            // players arrive on dry land near the middle (the way back out stands behind them):
            // the level spot the land was made with, else the nearest round it
            let t = &d.terrain;
            let mut at = land.arrival;
            'search: for ring in 0..60 {
                let r = ring as f32 * 12.0;
                for k in 0..(ring * 6).max(1) {
                    let a = std::f32::consts::TAU * k as f32 / (ring * 6).max(1) as f32;
                    let p = land.arrival + Vec2::from_angle(a) * r;
                    let h = t.height_at(p.x, p.y);
                    // level for 4 m round, and behind (−x) where the way back out stands
                    let flat = [
                        (4.0, 0.0),
                        (-4.0, 0.0),
                        (0.0, 4.0),
                        (0.0, -4.0),
                        (-7.0, 0.0),
                        (-7.0, 2.0),
                        (-7.0, -2.0),
                    ]
                    .iter()
                    .all(|(dx, dz)| (t.height_at(p.x + dx, p.y + dz) - h).abs() < 0.7);
                    if h > t.sea_level + 2.0 && flat {
                        at = p;
                        break 'search;
                    }
                }
            }
            let y = t.height_at(at.x, at.y);
            d.gameplay.player_start = Some(crate::gameplay::PlayerStart {
                pos: [at.x, y, at.y],
                yaw: std::f32::consts::FRAC_PI_2,
            });
            d.gameplay.dirty = true;
            d.orbit.set_view(Vec3::new(at.x, y, at.y), 0.35, 0.6, 140.0);
            d.orbit.snap();
        }
    }
    finish_map(d, def, dir)
}

/// A new map's name, folder and art.
fn finish_map(d: &mut WorldData, def: &DungeonDef, dir: &Path) -> Result<(), String> {
    d.meta.name = def.name.clone();
    d.meta.path = Some(dir.to_path_buf());
    // the map is painted in the dungeon's art (`crate::art_style`)
    d.meta.art_style = if def.art == super::styles::Art::HandPainted {
        crate::art_style::ArtStyle::HandPainted
    } else {
        crate::art_style::ArtStyle::Realistic
    };
    Ok(())
}

/// The player start in the entrance room, and the camera over it.
fn frame_entrance(d: &mut WorldData, doc: &MapDoc) {
    let plan = &doc.plan;
    let ent = plan
        .rooms
        .iter()
        .find(|r| r.role == super::plan::Role::Entrance)
        .or(plan.rooms.first());
    let (c, y) = ent
        .map(|r| (r.shape.centre(), plan.level_y(r.level) + r.floor))
        .unwrap_or((Vec2::ZERO, 0.0));
    d.gameplay.player_start = Some(crate::gameplay::PlayerStart {
        pos: [c.x, y + 0.05, c.y],
        yaw: std::f32::consts::FRAC_PI_2,
    });
    d.gameplay.dirty = true;
    let (lo, hi) = plan
        .bounds2()
        .unwrap_or((Vec2::splat(-20.0), Vec2::splat(20.0)));
    let size = (hi - lo).max_element().max(40.0);
    let mid = (lo + hi) * 0.5;
    d.orbit
        .set_view(Vec3::new(mid.x, y, mid.y), 0.35, 1.05, size * 1.05);
    d.orbit.snap();
}

/// A world saved somewhere new takes its dungeons' maps along (from its old folder, or from the
/// recovery slot it waited in).
pub fn carry_maps(from: &Path, to: &Path) {
    let src = from.join("dungeons");
    let dst = to.join("dungeons");
    if from == to || !src.is_dir() || dst.exists() {
        return;
    }
    if let Err(e) = copy_dir(&src, &dst) {
        warn!(
            "couldn't copy the dungeons' maps from {} to {}: {e}",
            src.display(),
            dst.display()
        );
    }
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}

/// Ways in follow what they stand on: a portal's frame or a linked object moved by hand, the end
/// of a cave laid out again.
pub(super) fn sync_entrances(
    mut list: ResMut<Dungeons>,
    store: Res<crate::objects::ObjectStore>,
    caves: Res<crate::caves::Caves>,
) {
    if !store.is_changed() && !caves.is_changed() {
        return;
    }
    let mut updates = Vec::new();
    for d in &list.list {
        for e in &d.entrances {
            let at = match e.kind {
                EntranceKind::Portal | EntranceKind::Object => e
                    .object
                    .and_then(|o| store.items.get(&o))
                    .map(|p| (p.pos, Quat::from_array(p.rot).to_euler(EulerRot::YXZ).0)),
                EntranceKind::Cave => e
                    .cave
                    .and_then(|c| caves.get(c))
                    .and_then(|c| c.dungeon.as_ref())
                    .map(|dg| (dg.door, dg.door_yaw)),
                EntranceKind::Point => None,
            };
            if let Some((p, yaw)) = at
                && (Vec3::from_array(p).distance(Vec3::from_array(e.pos)) > 0.01
                    || (yaw - e.yaw).abs() > 1e-3)
            {
                updates.push((d.id, e.id, p, yaw));
            }
        }
    }
    for (d, e, p, yaw) in updates {
        if let Some(x) = list
            .get_mut(d)
            .and_then(|x| x.entrances.iter_mut().find(|x| x.id == e))
        {
            x.pos = p;
            x.yaw = yaw;
        }
    }
}

/// Keeps the session in step with what is open: a dungeon map opened some other way (startup,
/// File › Open) knows its world from its folder; a world opened instead ends the session.
pub(super) fn follow(
    map: Res<DungeonMap>,
    meta: Res<crate::world::WorldMeta>,
    mut s: ResMut<Session>,
) {
    if !map.is_changed() && !meta.is_changed() {
        return;
    }
    match (&map.0, &meta.path) {
        (Some(_), Some(p)) if s.world.is_none() => {
            if let Some(w) = world_of(p) {
                s.world_untitled = w == crate::io::recovery_path();
                s.world = Some(w);
            }
        }
        (None, _) if s.world.is_some() => {
            s.world = None;
        }
        _ => {}
    }
}
