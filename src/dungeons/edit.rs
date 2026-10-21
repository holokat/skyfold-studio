//! Every change to dungeons (from the panels and MCP alike), one undo step each: in the world,
//! creating dungeons and their ways in (portals, objects, spots, cave ends); in a dungeon's map,
//! its plan, style, lights, mood and dressing.

use std::sync::Arc;

use bevy::prelude::*;

use super::model::*;
use super::plan::{Plan, Room, RoomKind, Shape};
use super::presets::Size;
use crate::io::WorldData;
use crate::objects::Placed;

/// History keys of the two snapshots (`History::push_ext`).
pub const UNDO_LIST: &str = "dungeons";
pub const UNDO_MAP: &str = "dungeon_map";

/// The shape of a portal's liquid sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sheet {
    /// straight sides and a round top
    Arch,
    /// a full circle
    Round,
    /// a flat top
    Square,
}

/// A portal frame style: the model (a stone frame holding the liquid) and its opening.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub asset: &'static str,
    pub name: &'static str,
    pub sheet: Sheet,
    /// the opening: width, height, its bottom above the model's origin
    pub w: f32,
    pub h: f32,
    pub bottom: f32,
    /// how deep the model is, front to back, and how tall (m)
    pub depth: f32,
    pub tall: f32,
}

/// The portal styles to choose from.
pub const FRAMES: [Frame; 6] = [
    Frame {
        asset: "hp:portal_arch",
        name: "Stone arch",
        sheet: Sheet::Arch,
        w: 3.2,
        h: 4.4,
        bottom: 0.32,
        depth: 1.96,
        tall: 6.06,
    },
    Frame {
        asset: "hp:portal_ruin",
        name: "Ruined arch",
        sheet: Sheet::Arch,
        w: 3.0,
        h: 4.2,
        bottom: 0.25,
        depth: 3.05,
        tall: 5.35,
    },
    Frame {
        asset: "hp:portal_ring",
        name: "Rune ring",
        sheet: Sheet::Round,
        w: 4.0,
        h: 4.0,
        bottom: 0.9,
        depth: 7.55,
        tall: 6.09,
    },
    Frame {
        asset: "hp:portal_gate",
        name: "Temple gate",
        sheet: Sheet::Arch,
        w: 4.6,
        h: 6.0,
        bottom: 0.5,
        depth: 3.01,
        tall: 8.11,
    },
    Frame {
        asset: "hp:portal_dwarven",
        name: "Dwarven door",
        sheet: Sheet::Square,
        w: 3.6,
        h: 4.6,
        bottom: 0.4,
        depth: 3.01,
        tall: 6.99,
    },
    Frame {
        asset: "hp:portal_spires",
        name: "Crystal spires",
        sheet: Sheet::Arch,
        w: 3.4,
        h: 4.6,
        bottom: 0.6,
        depth: 2.59,
        tall: 6.29,
    },
];

pub fn frame(asset: &str) -> Option<&'static Frame> {
    FRAMES.iter().find(|f| f.asset == asset)
}

/// A frame by its asset id or its name ("Stone arch", "stone_arch").
pub fn frame_named(s: &str) -> Option<&'static Frame> {
    let k = s.trim().to_lowercase().replace([' ', '-'], "_");
    FRAMES.iter().find(|f| {
        f.asset == s.trim()
            || f.asset.trim_start_matches("hp:") == k
            || f.asset.trim_start_matches("hp:portal_") == k
            || f.name.to_lowercase().replace(' ', "_") == k
    })
}

/// The frame a new portal gets: the first one the library has.
pub fn default_frame(lib: Option<&crate::objects::Library>) -> Option<&'static str> {
    let lib = lib?;
    FRAMES
        .iter()
        .map(|f| f.asset)
        .find(|id| lib.get(id).is_some())
}

/// What a new dungeon is.
#[derive(Clone, Debug)]
pub struct NewDungeon {
    pub name: String,
    pub kind: MapKind,
    /// a layout (`presets::all`) or a realm (`presets::realms`)
    pub layout: String,
    pub size: Size,
    pub room_size: super::presets::RoomSize,
    /// 0..1: how uneven the rooms are made
    pub irregularity: f32,
    /// hand-painted or photoreal
    pub art: super::styles::Art,
    /// "" for the layout's own
    pub style: String,
    pub mood: String,
    pub instancing: Instancing,
    pub way_in: Option<NewEntrance>,
    /// a realm's land: its shape, size and seed (None: the realm's own shape and size, a seed from
    /// its id; `realm_shapes`)
    pub shape: Option<super::realm_shapes::RealmShape>,
    pub realm_size: Option<super::realm_shapes::RealmSize>,
    pub seed: Option<u32>,
    /// a temple compound in the realm (`compound`; its layout is `compound::LAYOUT`)
    pub compound: Option<super::compound::Options>,
    /// a design sheet the realm is built from (`sheets`; its layout is `sheets::LAYOUT`)
    pub sheet: Option<super::sheets::SheetRef>,
}

impl Default for NewDungeon {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: MapKind::Interior,
            layout: "crypt".into(),
            size: Size::Medium,
            room_size: Default::default(),
            irregularity: 0.0,
            art: super::styles::Art::HandPainted,
            style: String::new(),
            mood: String::new(),
            instancing: Instancing::Party,
            way_in: None,
            shape: None,
            realm_size: None,
            seed: None,
            compound: None,
            sheet: None,
        }
    }
}

/// A way in to add.
#[derive(Clone, Debug)]
pub struct NewEntrance {
    pub kind: EntranceKind,
    pub at: Vec3,
    /// facing (radians, 0 = +z): a portal faces this way (you walk in against it)
    pub yaw: f32,
    pub object: Option<u64>,
    pub cave: Option<u32>,
    pub color: PortalColor,
    /// a portal's frame (`FRAMES`); None: the first the library has
    pub frame: Option<String>,
    /// an object way in placed new where it was put (`ENTRANCE_PROPS`) instead of a picked one
    pub prop: Option<String>,
    pub label: String,
}

/// Ready-made ways in (Hand-painted › Entrances) the way-in step can put down in one click.
pub const ENTRANCE_PROPS: [&str; 3] = ["hp:cellar_door", "hp:trapdoor", "hp:well_hatch"];

impl Default for NewEntrance {
    fn default() -> Self {
        Self {
            kind: EntranceKind::Portal,
            at: Vec3::ZERO,
            yaw: 0.0,
            object: None,
            cave: None,
            color: PortalColor::Arcane,
            frame: None,
            prop: None,
            label: String::new(),
        }
    }
}

fn finish_list(d: &mut WorldData, label: &str, before: Arc<Dungeons>) {
    let after = Arc::new((*d.dungeons).clone());
    if *after != *before {
        d.history
            .push_ext(label.to_string(), UNDO_LIST, before, after);
        d.meta.dirty = true;
    }
    d.history.commit(&d.store);
}

/// A name for a new dungeon (deterministic per id and layout).
pub fn auto_name(layout: &str, id: u32) -> String {
    let mut r = crate::noise::Rng::new(id as u64 * 7919 + layout.len() as u64);
    let adj = [
        "Sunken",
        "Forgotten",
        "Shadowed",
        "Weeping",
        "Iron",
        "Ashen",
        "Hollow",
        "Silent",
        "Crimson",
        "Blackstone",
        "Gloom",
        "Ember",
        "Frost",
        "Bone",
        "Whispering",
    ];
    let noun = match layout {
        "crypt" => "Crypt",
        "catacombs" => "Catacombs",
        "prison_keep" => "Stockade",
        "sun_temple" => "Temple",
        "cathedral" => "Cathedral",
        "dwarven_halls" => "Halls",
        "underground_city" => "Undercity",
        "great_caverns" => "Caverns",
        "goblin_cave" => "Warren",
        "spider_den" => "Nest",
        "sewers" => "Sewers",
        "mine" => "Mine",
        "clockwork" => "Works",
        "frost_halls" => "Halls",
        "infernal" => "Pits",
        l if l.ends_with("_realm") => "Realm",
        super::compound::LAYOUT => "Temple City",
        super::sheets::LAYOUT => "Ruins",
        _ => "Depths",
    };
    format!("The {} {}", adj[r.below(adj.len() as u32) as usize], noun)
}

/// Creates a dungeon (and its way in). Its map is made when it is first opened. Returns its id.
pub fn create(d: &mut WorldData, n: &NewDungeon) -> Result<u32, String> {
    if d.dungeon_map.0.is_some() {
        return Err("Dungeons are made in the world (go back to the world first)".into());
    }
    if !n.art.ready() {
        return Err(format!(
            "{} dungeons are coming soon: make it hand-painted for now",
            n.art.name()
        ));
    }
    let before = Arc::new((*d.dungeons).clone());
    // (one step, even when a cave lays itself out again for its portal)
    d.history.begin_group(&d.store);
    let id = d.dungeons.alloc_id();
    let name = if n.name.trim().is_empty() {
        auto_name(&n.layout, id)
    } else {
        n.name.trim().to_string()
    };
    let folder = d.dungeons.folder_for(id, &name);
    let (shape, realm_size, seed) = super::realm_shapes::resolve(n, id);
    d.dungeons.list.push(DungeonDef {
        id,
        name,
        kind: n.kind,
        instancing: n.instancing,
        levels: [10, 15],
        players: if n.instancing == Instancing::Raid {
            20
        } else {
            5
        },
        entrances: vec![],
        folder,
        layout: n.layout.clone(),
        style: n.style.clone(),
        size: n.size,
        room_size: n.room_size,
        irregularity: n.irregularity,
        art: n.art,
        mood: n.mood.clone(),
        notes: String::new(),
        shape,
        realm_size,
        seed,
        compound: (n.kind == MapKind::Realm
            && (n.layout == super::compound::LAYOUT || n.compound.is_some()))
        .then(|| n.compound.unwrap_or_default().clamped()),
        sheet: n.sheet.clone().filter(|_| n.kind == MapKind::Realm),
    });
    d.dungeons.selected = Some(id);
    let r = n
        .way_in
        .as_ref()
        .map(|e| entrance_into(d, id, e))
        .transpose();
    finish_list(d, "Create dungeon", before);
    d.history.end_group(&d.store, "Create dungeon");
    r?;
    Ok(id)
}

/// Adds a way in to dungeon `id` (one undo step). Returns its id.
pub fn add_entrance(d: &mut WorldData, id: u32, e: &NewEntrance) -> Result<u32, String> {
    let before = Arc::new((*d.dungeons).clone());
    d.history.begin_group(&d.store);
    let r = entrance_into(d, id, e);
    finish_list(d, "Add a way in", before);
    d.history.end_group(&d.store, "Add a way in");
    r
}

/// Adds the way in inside the open step (a portal's frame object goes in too).
fn entrance_into(d: &mut WorldData, id: u32, e: &NewEntrance) -> Result<u32, String> {
    if d.dungeons.get(id).is_none() {
        return Err(format!("no dungeon {id}"));
    }
    let mut ent = Entrance {
        id: 0,
        kind: e.kind,
        pos: e.at.to_array(),
        yaw: e.yaw,
        object: e.object,
        cave: e.cave,
        color: e.color,
        label: e.label.clone(),
    };
    match e.kind {
        EntranceKind::Portal => {
            // (Some(""): the liquid alone)
            let frame = e
                .frame
                .clone()
                .or_else(|| default_frame(d.lib.as_deref()).map(str::to_string))
                .filter(|f| !f.is_empty());
            if let Some(asset) = frame
                && let Some(entry) = d.lib.as_deref().and_then(|l| l.get(&asset))
            {
                let oid = d.store.new_id();
                d.history.touch_object(&d.store, oid);
                let mut p = Placed {
                    id: oid,
                    asset: entry.id.clone(),
                    variant: 0,
                    pos: e.at.to_array(),
                    rot: Quat::from_rotation_y(e.yaw).to_array(),
                    scale: 1.0,
                    stretch: None,
                    group: 0,
                    locked: false,
                };
                p.set_scale3(Vec3::splat(entry.default_scale));
                d.store.insert(p);
                ent.object = Some(oid);
            }
            // (no frame model in the library: the liquid stands on its own where it was put)
        }
        EntranceKind::Object => {
            // a ready-made way in put down where it was clicked
            let placed = match (e.object, e.prop.as_deref()) {
                (None, Some(asset)) => {
                    let entry = d
                        .lib
                        .as_deref()
                        .and_then(|l| l.get(asset))
                        .ok_or_else(|| format!("no model {asset} in the library"))?;
                    let oid = d.store.new_id();
                    d.history.touch_object(&d.store, oid);
                    let mut p = Placed {
                        id: oid,
                        asset: entry.id.clone(),
                        variant: 0,
                        pos: e.at.to_array(),
                        rot: Quat::from_rotation_y(e.yaw).to_array(),
                        scale: 1.0,
                        stretch: None,
                        group: 0,
                        locked: false,
                    };
                    p.set_scale3(Vec3::splat(entry.default_scale));
                    d.store.insert(p);
                    Some(oid)
                }
                _ => None,
            };
            let oid = placed
                .or(e.object)
                .ok_or("Pick the object that is the way in")?;
            let p = d
                .store
                .items
                .get(&oid)
                .ok_or_else(|| format!("no object {oid}"))?;
            if let Some((other, _)) = d.dungeons.by_object(oid) {
                return Err(format!(
                    "That object is already the way into dungeon {other}"
                ));
            }
            ent.object = Some(oid);
            ent.pos = p.pos;
            ent.yaw = Quat::from_array(p.rot).to_euler(EulerRot::YXZ).0;
            if ent.label.is_empty() {
                ent.label = d
                    .lib
                    .as_deref()
                    .and_then(|l| l.get(&p.asset))
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
            }
        }
        EntranceKind::Cave => {
            let cid = e.cave.ok_or("Pick the cave whose end is the way in")?;
            if d.dungeons.by_cave(cid).is_some() {
                return Err("That cave already leads to a dungeon".into());
            }
            let name = d
                .dungeons
                .get(id)
                .map(|x| x.name.clone())
                .unwrap_or_default();
            if d.caves.get(cid).is_none() {
                return Err(format!("no cave {cid}"));
            }
            // (the cave lays itself out again: the portal goes to its deepest chamber)
            crate::caves::edit::update(d, cid, "Cave dungeon", move |c| {
                if c.dungeon.is_some() {
                    return false;
                }
                c.dungeon = Some(crate::caves::Dungeon {
                    name,
                    chamber: 0,
                    door: [0.0; 3],
                    door_yaw: 0.0,
                    instancing: Instancing::Party,
                });
                true
            })?;
            if let Some(dg) = d.caves.get(cid).and_then(|c| c.dungeon.as_ref()) {
                ent.pos = dg.door;
                ent.yaw = dg.door_yaw;
            }
        }
        EntranceKind::Point => {}
    }
    ent.id = d.dungeons.alloc_entrance();
    let eid = ent.id;
    d.dungeons.get_mut(id).unwrap().entrances.push(ent);
    Ok(eid)
}

/// Gives a portal another frame style (`asset`; "" = the liquid alone), where it stands now.
pub fn set_frame(d: &mut WorldData, id: u32, eid: u32, asset: &str) -> Result<(), String> {
    let before = Arc::new((*d.dungeons).clone());
    let e = d
        .dungeons
        .get(id)
        .and_then(|x| x.entrances.iter().find(|e| e.id == eid))
        .cloned()
        .ok_or("no such way in")?;
    if e.kind != EntranceKind::Portal {
        return Err("Only a portal has a frame".into());
    }
    d.history.begin("Portal style");
    // where it stands: its frame's spot, else its own
    let (pos, rot) = match e.object.and_then(|o| d.store.items.get(&o)) {
        Some(p) => (p.pos, p.rot),
        None => (e.pos, Quat::from_rotation_y(e.yaw).to_array()),
    };
    if let Some(o) = e.object
        && d.store.items.contains_key(&o)
    {
        d.history.touch_object(&d.store, o);
        d.store.remove(o);
    }
    let mut object = None;
    if !asset.is_empty() {
        let entry = d
            .lib
            .as_deref()
            .and_then(|l| l.get(asset))
            .ok_or_else(|| format!("{asset} isn't in the library yet"))?;
        let oid = d.store.new_id();
        d.history.touch_object(&d.store, oid);
        let mut p = Placed {
            id: oid,
            asset: entry.id.clone(),
            variant: 0,
            pos,
            rot,
            scale: 1.0,
            stretch: None,
            group: 0,
            locked: false,
        };
        p.set_scale3(Vec3::splat(entry.default_scale));
        d.store.insert(p);
        object = Some(oid);
    }
    if let Some(x) = d
        .dungeons
        .get_mut(id)
        .and_then(|x| x.entrances.iter_mut().find(|e| e.id == eid))
    {
        x.object = object;
        x.pos = pos;
        x.yaw = Quat::from_array(rot).to_euler(EulerRot::YXZ).0;
    }
    finish_list(d, "Portal style", before);
    Ok(())
}

/// Removes a way in (a portal's frame goes too; a linked object or cave stays, unlinked).
pub fn remove_entrance(d: &mut WorldData, id: u32, eid: u32) -> Result<(), String> {
    let before = Arc::new((*d.dungeons).clone());
    d.history.begin("Remove a way in");
    let def = d
        .dungeons
        .get_mut(id)
        .ok_or_else(|| format!("no dungeon {id}"))?;
    let i = def
        .entrances
        .iter()
        .position(|e| e.id == eid)
        .ok_or_else(|| format!("no way in {eid}"))?;
    let e = def.entrances.remove(i);
    if e.kind == EntranceKind::Portal
        && let Some(o) = e.object
        && d.store.items.contains_key(&o)
    {
        d.history.touch_object(&d.store, o);
        d.store.remove(o);
    }
    finish_list(d, "Remove a way in", before);
    Ok(())
}

/// Removes a dungeon: its ways in, its portals (its map's folder is kept aside in
/// `dungeons/_removed/`).
pub fn remove(d: &mut WorldData, id: u32) -> Result<(), String> {
    let before = Arc::new((*d.dungeons).clone());
    d.history.begin("Remove dungeon");
    let def = d
        .dungeons
        .get(id)
        .cloned()
        .ok_or_else(|| format!("no dungeon {id}"))?;
    for e in &def.entrances {
        if e.kind == EntranceKind::Portal
            && let Some(o) = e.object
            && d.store.items.contains_key(&o)
        {
            d.history.touch_object(&d.store, o);
            d.store.remove(o);
        }
    }
    d.dungeons.list.retain(|x| x.id != id);
    if d.dungeons.selected == Some(id) {
        d.dungeons.selected = None;
    }
    finish_list(d, "Remove dungeon", before);
    if let Some(world) = d.meta.path.clone() {
        let dir = super::session::map_dir(&world, &def);
        if dir.is_dir() {
            let aside = world.join("dungeons").join("_removed").join(format!(
                "{}_{}",
                def.folder,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|t| t.as_secs())
                    .unwrap_or(0)
            ));
            let _ = std::fs::create_dir_all(aside.parent().unwrap());
            let _ = std::fs::rename(&dir, &aside);
        }
    }
    Ok(())
}

/// Changes a dungeon's details (name, instancing, levels, notes…) as one step.
pub fn update_def(
    d: &mut WorldData,
    id: u32,
    label: &str,
    f: impl FnOnce(&mut DungeonDef),
) -> Result<(), String> {
    let before = Arc::new((*d.dungeons).clone());
    d.history.begin(label);
    let def = d
        .dungeons
        .get_mut(id)
        .ok_or_else(|| format!("no dungeon {id}"))?;
    f(def);
    // a cave's way in carries the name
    let (name, caves): (String, Vec<u32>) = (
        def.name.clone(),
        def.entrances.iter().filter_map(|e| e.cave).collect(),
    );
    for c in caves {
        if let Some(dg) = d.caves.get_mut(c).and_then(|c| c.dungeon.as_mut()) {
            dg.name = name.clone();
        }
    }
    finish_list(d, label, before);
    Ok(())
}

/// Changes an entrance (colour, label…) as one step.
pub fn update_entrance(
    d: &mut WorldData,
    id: u32,
    eid: u32,
    label: &str,
    f: impl FnOnce(&mut Entrance),
) -> Result<(), String> {
    let before = Arc::new((*d.dungeons).clone());
    d.history.begin(label);
    let e = d
        .dungeons
        .get_mut(id)
        .and_then(|x| x.entrances.iter_mut().find(|e| e.id == eid))
        .ok_or("no such way in")?;
    f(e);
    finish_list(d, label, before);
    Ok(())
}

// ------------------------------------------------------------------ the map

/// Changes the open dungeon map (plan, style, lights, mood…) as one undo step.
pub fn map<T>(
    d: &mut WorldData,
    label: &str,
    f: impl FnOnce(&mut MapDoc) -> Result<T, String>,
) -> Result<T, String> {
    let Some(m) = d.dungeon_map.0.as_mut() else {
        return Err("No dungeon's map is open".into());
    };
    let before = Arc::new(m.clone());
    let out = f(m)?;
    let after = Arc::new(m.clone());
    if *after != *before {
        d.history.begin(label);
        d.history
            .push_ext(label.to_string(), UNDO_MAP, before, after);
        d.history.commit(&d.store);
        d.meta.dirty = true;
    }
    Ok(out)
}

/// Changes the way back out (its frame, colour, where it stands) as one undo step: its frame
/// object goes in, is swapped for another or taken out, and stands where it should.
pub fn way_out(d: &mut WorldData, label: &str, f: impl FnOnce(&mut WayOut)) -> Result<(), String> {
    let Some(m) = d.dungeon_map.0.as_mut() else {
        return Err("No dungeon's map is open".into());
    };
    let before = Arc::new(m.clone());
    f(&mut m.way_out);
    d.history.begin_group(&d.store);
    super::portal::place_exit_frame(d, true);
    let after = Arc::new(d.dungeon_map.0.clone().unwrap_or_else(|| (*before).clone()));
    if *after != *before {
        d.history
            .push_ext(label.to_string(), UNDO_MAP, before, after);
        d.meta.dirty = true;
    }
    d.history.end_group(&d.store, label);
    Ok(())
}

/// Adds a room to the plan (its level made if missing). Returns its id.
pub fn add_room(d: &mut WorldData, r: Room) -> Result<u32, String> {
    let label = format!("Add {}", r.kind.name().to_lowercase());
    map(d, &label, |m| {
        let mut r = r;
        if r.kind != RoomKind::Cavern && r.ceiling != super::plan::Ceiling::Open {
            r.ceiling =
                super::styles::get(r.style.as_deref().unwrap_or(&m.style)).ceiling_for(r.kind);
        }
        Ok(m.plan.add(r))
    })
}

pub fn remove_room(d: &mut WorldData, id: u32) -> Result<(), String> {
    map(d, "Remove room", |m| {
        if !m.plan.remove(id) {
            return Err(format!("no room {id}"));
        }
        m.lights.retain(|l| !(l.auto && l.room == Some(id)));
        Ok(())
    })
}

pub fn update_room(
    d: &mut WorldData,
    id: u32,
    label: &str,
    f: impl FnOnce(&mut Room),
) -> Result<(), String> {
    map(d, label, |m| {
        let r = m.plan.room_mut(id).ok_or_else(|| format!("no room {id}"))?;
        f(r);
        if let Some(t) = r.to_level {
            m.plan.ensure_level(t);
        }
        Ok(())
    })
}

/// Lays the whole plan out again from a layout (locked rooms stay); auto lights and dressing
/// are dropped (Auto-light and Dress make them again).
pub fn relayout(
    d: &mut WorldData,
    layout: &str,
    size: Size,
    rooms: super::presets::RoomSize,
    irregular: f32,
    seed: u32,
) -> Result<(), String> {
    let p = super::presets::get(layout).ok_or_else(|| format!("no layout {layout}"))?;
    let ids = d
        .dungeon_map
        .0
        .as_ref()
        .map(|m| m.props.clone())
        .unwrap_or_default();
    let r = map(d, &format!("Layout: {}", p.name), |m| {
        m.plan = super::presets::shuffle(&m.plan, p, size, rooms, irregular, seed, &m.style);
        // (a layout with rooms open to the sky brings its sky; others keep the map's)
        if p.sky() != super::model::SkyMode::None {
            m.sky = p.sky();
        }
        if p.id == "empty" {
            m.pieces.retain(|x| !x.auto);
        } else {
            super::pieces::place_for_layout(m);
        }
        m.lights.retain(|l| !l.auto);
        m.props.clear();
        Ok(())
    });
    if r.is_ok() {
        remove_props(d, &ids);
    }
    r
}

/// Removes the builder's objects (dressing, fixtures) in their own undo step.
pub fn remove_props(d: &mut WorldData, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    d.history.begin("Clear dressing");
    for id in ids {
        if d.store.items.contains_key(id) {
            d.history.touch_object(&d.store, *id);
            d.store.remove(*id);
        }
    }
    d.history.commit(&d.store);
}

/// A new plan for an empty map: a room to start from.
pub fn starter_plan() -> Plan {
    let mut p = Plan::default();
    let id = p.add(Room::new(
        0,
        RoomKind::Room,
        0,
        Shape::rect(Vec2::ZERO, Vec2::new(8.0, 8.0)),
    ));
    if let Some(r) = p.room_mut(id) {
        r.role = super::plan::Role::Entrance;
        r.name = "Entrance".into();
    }
    p
}

/// Undo / redo: restore the snapshots the history hands back.
pub(super) fn apply_undo(
    mut history: ResMut<crate::history::History>,
    mut list: ResMut<Dungeons>,
    mut map: ResMut<DungeonMap>,
) {
    if history.ext_apply.is_empty() {
        return;
    }
    let mut keep = vec![];
    for (key, snap) in std::mem::take(&mut history.ext_apply) {
        match key {
            UNDO_LIST => {
                if let Some(c) = snap.downcast_ref::<Dungeons>() {
                    let sel = list.selected;
                    *list = c.clone();
                    list.selected = sel.filter(|s| list.get(*s).is_some());
                }
            }
            UNDO_MAP => {
                if let Some(c) = snap.downcast_ref::<MapDoc>() {
                    map.0 = Some(c.clone());
                }
            }
            _ => keep.push((key, snap)),
        }
    }
    history.ext_apply = keep;
}
