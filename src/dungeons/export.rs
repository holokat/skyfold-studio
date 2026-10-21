//! The dungeons for the engine exports (`export::dungeons` writes them in each engine's terms):
//! every dungeon's map read from its folder (the world is the map that's loaded), built the way
//! the editor builds it, with its lights, mood and placed objects, and how it links to the world:
//! the ways in (triggers in the world, where players come back out) and the way back out of the
//! map. Plain data in world metres and Bevy's axes (Y up, right-handed); no engine formats here.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde_json::{Value, json};

use super::mesh::{self, Doorway, Layer, MeshData};
use super::model::*;
use super::plan::{Liquid, RoomKind};
use super::styles::{self, Slot, Style};
use crate::objects::Placed;

/// One dungeon of the world.
pub struct DungeonExport {
    pub id: u32,
    pub name: String,
    /// its folder in the world (`001_the_sunken_crypt`); also its name in the export
    pub slug: String,
    pub kind: MapKind,
    pub def: DungeonDef,
    /// its underground map as last saved (None: a realm, or a map never opened)
    pub map: Option<MapDoc>,
    /// everything placed in the map (light fixtures, dressing, the user's own)
    pub objects: Vec<Placed>,
    /// where players arrive in the map: position and the way they look
    pub start: Option<(Vec3, Vec3)>,
    /// the way back out, in the map
    pub exit: Option<Trigger>,
    /// the ways in, in the world
    pub ways_in: Vec<WayIn>,
    /// its fires (its fixtures' and objects' flames, its fire lights' own: `crate::flames`)
    pub fires: Vec<crate::flames::Fire>,
    /// its folder on disk (its music track lives there)
    pub folder: Option<std::path::PathBuf>,
}

/// A box that takes whoever walks into it somewhere: its middle, turn and half size (x across,
/// y up, z through), and the portal sheet standing in it (width, height; none for doors, hatches
/// and spots).
#[derive(Clone, Debug)]
pub struct Trigger {
    pub center: Vec3,
    pub rot: Quat,
    pub half: Vec3,
    pub sheet: Option<(f32, f32)>,
    /// linear RGB of the portal's liquid
    pub color: [f32; 3],
}

/// A way into a dungeon from the world.
#[derive(Clone, Debug)]
pub struct WayIn {
    pub entrance: u32,
    pub kind: EntranceKind,
    pub label: String,
    pub trigger: Trigger,
    /// where players come back out of the dungeon in the world: position and the way they look
    pub arrive: (Vec3, Vec3),
}

/// The built geometry of an underground map.
pub struct Geometry {
    pub parts: Vec<Part>,
    pub liquids: Vec<(i32, Liquid, MeshData)>,
    pub doorways: Vec<Doorway>,
    pub triangles: usize,
}

/// One piece of the architecture: a level's walls and floors (or its ceilings) in one material.
pub struct Part {
    pub level: i32,
    /// ceilings, vaults and the shafts of tall pillars (hidden in the editor's cut-away view)
    pub ceiling: bool,
    pub style: &'static Style,
    pub slot: Slot,
    pub mesh: MeshData,
}

#[derive(serde::Deserialize)]
struct MapFile {
    #[serde(default)]
    dungeon_map: Option<MapDoc>,
    #[serde(default)]
    objects: Vec<Placed>,
    #[serde(default)]
    objects_file: Option<String>,
    #[serde(default)]
    gameplay: crate::gameplay::Gameplay,
}

/// The world's dungeons as saved (`Err` while a dungeon's map is open: its world isn't the one
/// loaded, so there's nothing to export it with).
pub fn gather(world: &World) -> Result<Vec<DungeonExport>, String> {
    if super::in_dungeon() {
        return Err("A dungeon's map is open: go back to its world (Back to world) to export; the dungeons export with it".into());
    }
    let Some(list) = world.get_resource::<Dungeons>() else {
        return Ok(vec![]);
    };
    if list.list.is_empty() {
        return Ok(vec![]);
    }
    let dir: Option<PathBuf> = world.resource::<crate::world::WorldMeta>().path.clone();
    let store = world.resource::<crate::objects::ObjectStore>();
    let lib = world.get_resource::<crate::objects::Library>();
    let bounds = world.get_resource::<crate::placement::footprint::ModelBounds>();
    let caves = world
        .get_resource::<crate::caves::Caves>()
        .cloned()
        .unwrap_or_default();
    // the portal sheets as the editor stands them (frames, caves' ends)
    let sheets = super::portal::wanted(
        list,
        &DungeonMap(None),
        &caves,
        store,
        &crate::gameplay::Gameplay::default(),
    );
    let mut out = vec![];
    for def in &list.list {
        let file = dir
            .as_deref()
            .map(|d| d.join("dungeons").join(&def.folder))
            .and_then(|p| read_map(&p).transpose())
            .transpose()?;
        let (map, objects, gameplay) = match file {
            Some(f) => (f.dungeon_map, f.objects, f.gameplay),
            None => (None, vec![], Default::default()),
        };
        let start = gameplay.player_start.map(|p| {
            (
                Vec3::from_array(p.pos),
                Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin()),
            )
        });
        // (in its frame, in its colour, where it was put: as the editor stands it)
        let exit = match (
            map.as_ref().and_then(|m| {
                super::portal::exit_want(m, gameplay.player_start.as_ref(), |id| {
                    objects.iter().find(|o| o.id == id).cloned()
                })
            }),
            gameplay.player_start.as_ref(),
        ) {
            (Some(w), _) => Some(sheet_trigger(w.tf, w.w, w.h, w.color)),
            (None, Some(ps)) if def.kind == MapKind::Realm => {
                let (at, yaw) = super::portal::realm_exit(ps);
                Some(sheet_trigger(
                    Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
                    3.2,
                    4.4,
                    PortalColor::Arcane,
                ))
            }
            _ => None,
        };
        let mut ways_in = vec![];
        for e in &def.entrances {
            let sheet = sheets
                .iter()
                .find(|w| w.site == super::portal::Site::Entrance(def.id, e.id));
            let trigger = match e.kind {
                EntranceKind::Portal | EntranceKind::Cave => match sheet {
                    Some(w) => sheet_trigger(w.tf, w.w, w.h, e.color),
                    None => continue,
                },
                EntranceKind::Object => {
                    let Some(o) = e.object.and_then(|o| store.items.get(&o)) else {
                        continue;
                    };
                    let b = lib
                        .and_then(|l| l.get(&o.asset))
                        .zip(bounds)
                        .map(|(x, b)| b.of(x))
                        .unwrap_or((Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)));
                    object_trigger(o, b)
                }
                EntranceKind::Point => Trigger {
                    center: Vec3::from_array(e.pos) + Vec3::Y * 1.2,
                    rot: Quat::IDENTITY,
                    half: Vec3::new(1.0, 1.4, 1.0),
                    sheet: None,
                    color: [1.0; 3],
                },
            };
            // (as `session::world_arrival`: in front of it, looking away)
            let f = Vec3::new(e.yaw.sin(), 0.0, e.yaw.cos());
            let at = Vec3::from_array(e.pos) + f * 2.6;
            ways_in.push(WayIn {
                entrance: e.id,
                kind: e.kind,
                label: e.label.clone(),
                trigger,
                arrive: (Vec3::new(at.x, e.pos[1] + 0.2, at.z), f),
            });
        }
        let fires = if map.as_ref().is_some_and(|m| m.kind == MapKind::Interior) {
            crate::flames::export_fires(world, &objects, map.as_ref())
        } else {
            vec![]
        };
        let folder = dir.as_deref().map(|d| d.join("dungeons").join(&def.folder));
        out.push(DungeonExport {
            id: def.id,
            name: def.name.clone(),
            slug: def.folder.clone(),
            kind: def.kind,
            def: def.clone(),
            map,
            objects,
            start,
            exit,
            ways_in,
            fires,
            folder,
        });
    }
    Ok(out)
}

fn read_map(dir: &Path) -> Result<Option<MapFile>, String> {
    let Ok(text) = std::fs::read_to_string(dir.join("world.json")) else {
        return Ok(None);
    };
    let mut f: MapFile =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Some(name) = f.objects_file.clone() {
        f.objects = crate::bigworld::decode_objects(
            &std::fs::read(dir.join(name)).map_err(|e| format!("{}: {e}", dir.display()))?,
        )?;
    }
    Ok(Some(f))
}

/// The box round a portal sheet (`portal::Want`): as wide and tall as the sheet, a metre and a
/// bit deep.
fn sheet_trigger(tf: Transform, w: f32, h: f32, color: PortalColor) -> Trigger {
    let s = tf.scale.abs().max(Vec3::splat(0.01));
    let (w, h) = (w * s.x, h * s.y);
    let core = color.colors().0;
    Trigger {
        center: tf.translation + tf.rotation * Vec3::new(0.0, h * 0.5, 0.0),
        rot: tf.rotation,
        half: Vec3::new(w * 0.5, h * 0.5, 0.6),
        sheet: Some((w, h)),
        color: core,
    }
}

/// The box round a way-in object (a cellar door, a well), an arm's length bigger all round:
/// players walk up to it (the editor's trigger: `portal::crossing`).
fn object_trigger(o: &Placed, b: (Vec3, Vec3)) -> Trigger {
    let fp = crate::placement::footprint::Footprint::of(o, b);
    let s = o.scale3();
    let (lo, hi) = (o.pos[1] + b.0.y * s.y, o.pos[1] + b.1.y * s.y);
    let yaw = fp.ax.y.atan2(fp.ax.x);
    Trigger {
        center: Vec3::new(fp.center.x, (lo + hi) * 0.5 + 0.4, fp.center.y),
        rot: Quat::from_rotation_y(-yaw),
        half: Vec3::new(fp.half.x + 0.9, (hi - lo) * 0.5 + 1.0, fp.half.y + 0.9),
        sheet: None,
        color: [1.0; 3],
    }
}

/// The map's architecture, built as the editor builds it (no wall caps: they're only for the
/// cut-away view).
pub fn geometry(map: &MapDoc) -> Option<Geometry> {
    if map.plan.rooms.is_empty() {
        return None;
    }
    let c = super::render::compile(&map.plan, &map.style);
    let idx = super::render::style_index(&map.plan, &map.style);
    let built = mesh::build_with(
        &c,
        &|r| idx.get(&r.id).copied().unwrap_or(0),
        map.weathering.rubble,
    );
    let triangles = built.triangles();
    let all = styles::all();
    let mut keys: Vec<_> = built
        .parts
        .keys()
        .copied()
        .filter(|k| k.layer != Layer::Cap)
        .collect();
    keys.sort();
    let mut parts_in = built.parts;
    let parts = keys
        .into_iter()
        .filter_map(|k| {
            let m = parts_in.remove(&k)?;
            (!m.is_empty()).then(|| Part {
                level: k.level,
                ceiling: k.layer == Layer::Ceiling,
                style: &all[(k.style as usize).min(all.len() - 1)],
                slot: k.slot,
                mesh: m,
            })
        })
        .collect();
    let liquids = built
        .liquids
        .into_iter()
        .filter(|l| !l.mesh.is_empty())
        .map(|l| (l.level, l.liquid, l.mesh))
        .collect();
    Some(Geometry {
        parts,
        liquids,
        doorways: built.doorways,
        triangles,
    })
}

/// The map's light sources: what engines need to place them (linear colour after the mood's
/// tint; `intensity` for engines without light units, `lumens` for an eye adapted to the dark).
pub fn lights_json(map: &MapDoc) -> Vec<Value> {
    map.lights
        .iter()
        .map(|l| {
            let c = Color::srgb_from_array(map.mood.tinted(l.color)).to_linear();
            let spot = l.cone > 0.0;
            json!({
                "id": l.id,
                "kind": l.kind.id(),
                "position": l.pos,
                "color_linear": [c.red, c.green, c.blue],
                "lumens_editor": l.lumens,
                "lumens": l.lumens / 100.0,
                "intensity": ((l.lumens / 300_000.0).sqrt() * 1.5).clamp(0.4, 8.0),
                "range": l.range,
                "spot": spot,
                "cone_deg": l.cone,
                "direction": if spot { l.direction().to_array() } else { [0.0, -1.0, 0.0] },
                "flicker": l.flicker,
                "shadows": l.shadows,
                "haze": l.haze,
            })
        })
        .collect()
}

/// The map's mood for the engines: ambient fill, fog, exposure, haze and dust, ambience.
pub fn mood_json(map: &MapDoc) -> Value {
    let m = &map.mood;
    let lin = |c: [f32; 3]| {
        let l = Color::srgb_from_array(c).to_linear();
        [l.red, l.green, l.blue]
    };
    json!({
        "preset": m.preset,
        "name": super::mood::name(&m.preset),
        "ambient_linear": lin(m.fill),
        "ambient_strength": m.fill_strength,
        "exposure_stops": m.exposure,
        "fog_linear": lin(m.fog),
        "fog_density": 0.003 + m.fog_density.clamp(0.0, 1.0).powi(2) * 0.045,
        "haze": m.haze,
        "dust": m.motes,
        "embers": matches!(m.theme.as_str(), "forge" | "infernal"),
        "ambience": m.sound,
        "ambience_layer": m.sound2,
        "volume": m.volume,
    })
}

/// The sky over the map's open-ceilinged rooms: none (darkness), or day / dusk / night with the
/// direction its sun (or moon) shines along and a light strength for engines without units.
pub fn sky_json(map: &MapDoc) -> Value {
    use super::model::SkyMode;
    let (elev, intensity, color) = match map.sky {
        SkyMode::None => return json!({ "mode": "none" }),
        SkyMode::Day => (55.0f32, 1.6, [1.0, 0.96, 0.9]),
        SkyMode::Dusk => (9.0, 0.9, [1.0, 0.62, 0.38]),
        SkyMode::Night => (42.0, 0.25, [0.62, 0.72, 1.0]),
    };
    let (e, a) = (elev.to_radians(), 35f32.to_radians());
    // the way the light travels (down from the sun)
    let dir = -Vec3::new(a.sin() * e.cos(), e.sin(), a.cos() * e.cos());
    json!({ "mode": map.sky.id(), "light_direction": dir.to_array(), "intensity": intensity, "color": color, "hour": map.sky.hour() })
}

/// The rooms (outlines, floors, heights, roles): for minimaps, encounter design and the game's
/// own logic.
pub fn rooms_json(map: &MapDoc) -> Vec<Value> {
    map.plan
        .rooms
        .iter()
        .map(|r| {
            let outline: Vec<[f32; 2]> = r.shape.outline().iter().map(|p| [p.x, p.y]).collect();
            json!({
                "id": r.id,
                "name": r.name,
                "kind": r.kind.id(),
                "role": r.role.id(),
                "level": r.level,
                "floor": map.plan.level_y(r.level) + r.floor,
                "height": r.height,
                "outline": outline,
                "liquid": if matches!(r.kind, RoomKind::Pool) { r.liquid.id() } else { "none" },
                "to_level": r.to_level,
            })
        })
        .collect()
}

/// Doorways (for doors, gates and the game's own logic): ends, floor, top, the way into the room.
pub fn doorways_json(g: &Geometry) -> Vec<Value> {
    g.doorways.iter().map(|d| json!({ "room": d.room, "a": [d.a.x, d.a.y], "b": [d.b.x, d.b.y], "floor": d.floor, "top": d.top, "inward": [d.inward.x, d.inward.y] })).collect()
}

/// The baked fires the export carries (`flames/<fire>.png`): their grids and paces.
pub fn flame_sheets_json() -> Value {
    let mut m = serde_json::Map::new();
    for k in crate::flames::FireKind::ALL {
        let s = k.sheet();
        m.insert(k.id().into(), json!({ "file": format!("{}.png", k.id()), "columns": s.columns, "rows": s.rows, "frames": s.frames, "fps": s.fps, "frame_m": [s.width_m, s.height_m], "foot_v": s.foot_v, "premultiplied": true }));
    }
    Value::Object(m)
}

pub fn trigger_json(t: &Trigger) -> Value {
    json!({
        "center": t.center.to_array(),
        "rotation": t.rot.to_array(),
        "half_size": t.half.to_array(),
        "sheet": t.sheet.map(|(w, h)| json!({ "width": w, "height": h })),
        "color_linear": t.color,
    })
}

/// A dungeon's description for every engine (`dungeon.json`; its geometry, objects and scenes
/// are written next to it).
pub fn describe(d: &DungeonExport, g: Option<&Geometry>) -> Value {
    let look = |(p, f): (Vec3, Vec3)| json!({ "position": p.to_array(), "look": f.to_array() });
    json!({
        "id": d.id,
        "name": d.name,
        "slug": d.slug,
        "kind": match d.kind { MapKind::Interior => "underground", MapKind::Realm => "realm" },
        "instancing": d.def.instancing.id(),
        "levels": d.def.levels,
        "players": d.def.players,
        "notes": d.def.notes,
        "art": d.map.as_ref().map(|m| m.art.id()),
        "style": d.map.as_ref().map(|m| m.style.clone()),
        "player_start": d.start.map(look),
        "exit": d.exit.as_ref().map(trigger_json),
        "ways_in": d.ways_in.iter().map(|w| json!({
            "entrance": w.entrance,
            "kind": w.kind.id(),
            "label": w.label,
            "trigger": trigger_json(&w.trigger),
            "arrive": look(w.arrive),
        })).collect::<Vec<_>>(),
        "mood": d.map.as_ref().map(mood_json),
        "sky": d.map.as_ref().map(sky_json),
        "lights": d.map.as_ref().map(lights_json).unwrap_or_default(),
        "rooms": d.map.as_ref().map(rooms_json).unwrap_or_default(),
        "doorways": g.map(doorways_json).unwrap_or_default(),
        "decals": d.map.as_ref().map(|m| m.decals.iter().map(|x| json!({
            "kind": x.kind.id(), "position": x.pos, "normal": x.normal, "size": x.size, "spin_deg": x.spin.to_degrees(),
            "color": x.color, "glow": x.glow,
        })).collect::<Vec<_>>()).unwrap_or_default(),
        "fires": crate::flames::fires_json(&d.fires),
        "triangles": g.map(|g| g.triangles).unwrap_or(0),
        "map_built": d.map.is_some(),
    })
}
