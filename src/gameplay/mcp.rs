//! MCP tools for the gameplay layer. They go through the same edit functions as the Gameplay
//! tab and record one undo step each. Ores: `gameplay_list_ores`, `gameplay_add_ore_deposit`,
//! `gameplay_add_ore_area` (updated and removed with `gameplay_update` / `gameplay_remove`).

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use serde_json::{Value, json};

use super::edit::{self, Ground};
use super::ores::{self, Ore, OreEntry, Site};
use super::{
    Aggression, Gameplay, GameplaySel, NpcRole, PoiKind, ResourceKind, SpawnBehaviour, SpawnEntry,
    scatter,
};
use crate::history::History;
use crate::mcp_tools::{
    McpAppExt, McpContent, McpTool, arg, bearing_schema, facing_bearing, facing_yaw, opt_arg,
    position_arg, position_schema, xz_aliases,
};
use crate::objects::{Library, ObjectStore};
use crate::roads::Roads;
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::{WorldMeta, Zones};

type EditState<'w> = (
    ResMut<'w, Gameplay>,
    ResMut<'w, History>,
    Res<'w, Terrain>,
    Res<'w, Zones>,
    Res<'w, WaterBodies>,
    Res<'w, ObjectStore>,
    Option<Res<'w, Library>>,
    ResMut<'w, WorldMeta>,
    Option<Res<'w, Roads>>,
);

/// Procedural nature (scatter puts wood on its trees, ore and stone by its rocks).
type NatureState<'w> = (
    Res<'w, crate::nature::NatureField>,
    Option<Res<'w, crate::nature::Rules>>,
    Res<'w, crate::nature::Masks>,
    Res<'w, crate::objects::NatureAmounts>,
    Res<'w, crate::nature::NatureAppearance>,
);

type ReadState<'w> = (
    Res<'w, Gameplay>,
    Res<'w, Terrain>,
    Res<'w, Zones>,
    Res<'w, WaterBodies>,
    Res<'w, WorldMeta>,
);

/// Runs an edit as one undo step (nothing changes when it fails).
fn edit<R>(
    world: &mut World,
    label: &str,
    f: impl FnOnce(&mut Gameplay, &Ground, &ObjectStore, Option<&Library>) -> Result<R, String>,
) -> Result<R, String> {
    edit_site(world, label, |gp, site, store| {
        f(gp, site.g, store, site.lib)
    })
}

/// `edit` with the whole site (roads too) for ore deposit areas.
fn edit_site<R>(
    world: &mut World,
    label: &str,
    f: impl FnOnce(&mut Gameplay, &Site, &ObjectStore) -> Result<R, String>,
) -> Result<R, String> {
    let mut state: SystemState<(EditState, NatureState)> = SystemState::new(world);
    let (
        (mut gp, mut history, terrain, zones, water, store, lib, mut meta, roads),
        (field, rules, masks, amounts, appearance),
    ) = state.get_mut(world).map_err(|e| e.to_string())?;
    let nature = match (lib.as_deref(), rules.as_deref()) {
        (Some(lib), Some(rules)) => Some(crate::nature::Ctx {
            terrain: &terrain,
            water: &water,
            zones: &zones,
            lib,
            field: &field,
            rules,
            masks: &masks,
            amounts: crate::nature::amounts(&amounts),
            flora_amounts: amounts.flora_amounts(),
            appearance: *appearance,
            seed: meta.seed,
        }),
        _ => None,
    };
    let g = Ground {
        terrain: &terrain,
        zones: &zones,
        water: &water,
        nature,
    };
    let site = Site {
        g: &g,
        store: Some(&store),
        lib: lib.as_deref(),
        roads: roads.as_deref(),
    };
    let before = gp.clone();
    match f(&mut gp, &site, &store) {
        Ok(r) => {
            history.begin(label);
            history.touch_gameplay(&before);
            history.commit(&store);
            meta.dirty = true;
            gp.dirty = true;
            Ok(r)
        }
        Err(e) => {
            *gp = before;
            Err(e)
        }
    }
}

/// After a world is generated: scatter ore with the world's ore settings (one undo step).
pub(super) fn auto_fill_ore(world: &mut World) {
    let st = {
        let mut gp = world.resource_mut::<Gameplay>();
        if !gp.ore_fill_pending {
            return;
        }
        gp.ore_fill_pending = false;
        gp.ore_settings.clone()
    };
    if !st.on_generate {
        return;
    }
    let seed = world.resource::<crate::world::WorldMeta>().seed;
    let p = scatter::ScatterParams {
        kinds: vec![ResourceKind::Ore],
        ore: st,
        seed,
        ..default()
    };
    if let Ok(n) = edit(world, "Scatter ore", |gp, g, store, lib| {
        Ok(scatter::apply(gp, &p, g, store, lib).0)
    }) {
        world.resource_mut::<crate::editor::Editor>().toast(format!(
            "Added {n} ore deposits (Gameplay › Ore to change the mix)"
        ));
    }
}

/// Reads the world without changing it.
fn read<R>(
    world: &mut World,
    f: impl FnOnce(&Gameplay, &Ground, &WorldMeta) -> R,
) -> Result<R, String> {
    let mut state: SystemState<ReadState> = SystemState::new(world);
    let (gp, terrain, zones, water, meta) = state.get(world).map_err(|e| e.to_string())?;
    let g = Ground {
        terrain: &terrain,
        zones: &zones,
        water: &water,
        nature: None,
    };
    Ok(f(&gp, &g, &meta))
}

/// Lower-case letters and digits only ("Quest Hub", "quest_hub" and "QuestHub" all match).
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn parse_enum<T: Copy + std::fmt::Debug>(
    all: &[T],
    name: impl Fn(T) -> &'static str,
    v: &str,
    what: &str,
) -> Result<T, String> {
    let n = norm(v);
    all.iter()
        .copied()
        .find(|x| {
            norm(&format!("{x:?}")) == n
                || norm(name(*x)) == n
                || norm(name(*x)).starts_with(&n) && n.len() >= 4
        })
        .ok_or_else(|| {
            format!(
                "unknown {what} `{v}` (one of: {})",
                all.iter()
                    .map(|x| format!("{x:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn poi_kind(v: &str) -> Result<PoiKind, String> {
    parse_enum(&PoiKind::ALL, PoiKind::name, v, "POI kind")
}
fn npc_role(v: &str) -> Result<NpcRole, String> {
    parse_enum(&NpcRole::ALL, NpcRole::name, v, "NPC role")
}
fn resource_kind(v: &str) -> Result<ResourceKind, String> {
    parse_enum(&ResourceKind::ALL, ResourceKind::name, v, "resource kind")
}
fn aggression(v: &str) -> Result<Aggression, String> {
    parse_enum(&Aggression::ALL, Aggression::name, v, "aggression")
}
fn behaviour(v: &str) -> Result<SpawnBehaviour, String> {
    parse_enum(&SpawnBehaviour::ALL, SpawnBehaviour::name, v, "behaviour")
}
fn ore(v: &str) -> Result<Ore, String> {
    Ore::parse(v).ok_or_else(|| {
        format!(
            "unknown ore `{v}` (one of: {})",
            Ore::ALL.map(|o| o.id()).join(", ")
        )
    })
}

fn level_cap_arg(args: &Value) -> Result<Option<u32>, String> {
    match args.get("level_cap") {
        Some(Value::Null) => Ok(None),
        Some(v) => {
            let cap = serde_json::from_value::<u32>(v.clone())
                .map_err(|e| format!("argument `level_cap`: {e}"))?;
            if !(1..=200).contains(&cap) {
                return Err("`level_cap` must be an integer from 1 through 200".into());
            }
            Ok(Some(cap))
        }
        None => Err("missing argument `level_cap`".into()),
    }
}

/// An ore table from `ores` (ids or names) and optional `weights`.
fn ore_table(args: &Value) -> Result<Option<Vec<OreEntry>>, String> {
    let Some(list) = opt_arg::<Vec<String>>(args, "ores")? else {
        return Ok(None);
    };
    let weights: Vec<f32> = opt_arg(args, "weights")?.unwrap_or_default();
    list.iter()
        .enumerate()
        .map(|(i, o)| {
            Ok(OreEntry {
                ore: ore(o)?,
                weight: weights.get(i).copied().unwrap_or(1.0).max(0.0),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}

/// Deposit-area fields shared by add and update (the area is re-scattered after).
fn apply_area_fields(a: &mut ores::OreArea, args: &Value) -> Result<(), String> {
    if let Some(d) = opt_arg::<f32>(args, "density")? {
        a.density = d.clamp(0.1, 2000.0);
    }
    if let Some(r) = opt_arg::<f32>(args, "respawn_secs")? {
        a.respawn_secs = r.max(1.0);
    }
    if let Some(s) = opt_arg::<u32>(args, "seed")? {
        a.seed = s;
    }
    if let Some(n) = opt_arg::<String>(args, "notes")? {
        a.notes = n;
    }
    Ok(())
}

fn ore_json(o: Ore) -> Value {
    let h = o.habitat();
    let l = o.look();
    json!({
        "id": o.id(), "name": o.name(), "tier": o.tier(), "rarity": o.rarity().name(), "typical_use": o.uses(),
        "found_in": h.about, "biomes": h.biomes.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>(),
        "height_band": [h.alt.0, h.alt.1], "forms_in_cliffs": h.cliffs,
        "gathered_as": format!("{:?}", o.kind()), "deposit_name": o.deposit_name(), "respawn_secs": o.default_respawn(),
        "companion": o.companion().map(|c| c.id()), "glows": l.glow_nits > 0.0, "crystal_shards": l.shards,
    })
}

fn levels(v: [u32; 2]) -> [u8; 2] {
    let a = v[0].clamp(1, 255) as u8;
    [a, (v[1].clamp(1, 255) as u8).max(a)]
}

/// A creature by id or by name.
fn creature_id(gp: &Gameplay, v: &Value) -> Result<u32, String> {
    match v {
        Value::Number(n) => n
            .as_u64()
            .map(|id| id as u32)
            .filter(|id| gp.creature(*id).is_some())
            .ok_or_else(|| format!("no creature with id {n}")),
        Value::String(s) => gp
            .creatures
            .iter()
            .find(|c| norm(&c.name) == norm(s))
            .map(|c| c.id)
            .ok_or_else(|| format!("no creature named `{s}` (add it with gameplay_add_creature)")),
        _ => Err("creatures are ids or names".into()),
    }
}

/// An encounter table from `creatures` (ids or names) and optional `weights`.
fn entries(gp: &Gameplay, args: &Value) -> Result<Option<Vec<SpawnEntry>>, String> {
    let Some(list) = opt_arg::<Vec<Value>>(args, "creatures")? else {
        return Ok(None);
    };
    let weights: Vec<f32> = opt_arg(args, "weights")?.unwrap_or_default();
    list.iter()
        .enumerate()
        .map(|(i, v)| {
            Ok(SpawnEntry {
                creature: creature_id(gp, v)?,
                weight: weights.get(i).copied().unwrap_or(1.0).max(0.0),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}

/// A polygon from `polygon` ([[x,z],…]) or `center` + `radius` (a rough circle).
fn polygon(args: &Value) -> Result<Option<Vec<[f32; 2]>>, String> {
    if let Some(p) = opt_arg::<Vec<[f32; 2]>>(args, "polygon")? {
        return Ok(Some(p));
    }
    match (
        opt_arg::<[f32; 2]>(args, "center")?,
        opt_arg::<f32>(args, "radius")?,
    ) {
        (Some(c), Some(r)) => Ok(Some(
            (0..16)
                .map(|k| {
                    let a = k as f32 / 16.0 * std::f32::consts::TAU;
                    [
                        c[0] + a.cos() * r * (1.0 + 0.08 * (a * 3.0).sin()),
                        c[1] + a.sin() * r * (1.0 + 0.08 * (a * 2.0).cos()),
                    ]
                })
                .collect(),
        )),
        _ => Ok(None),
    }
}

/// `yaw_deg`: a compass bearing (the MCP convention) → the gameplay facing.
fn yaw_arg(args: &Value) -> Result<Option<f32>, String> {
    Ok(opt_arg::<f32>(args, "yaw_deg")?.map(facing_yaw))
}

/// `position` [x, z] (old form: separate `x` and `z`).
fn pos_arg(args: &Value) -> Result<Option<Vec2>, String> {
    Ok(position_arg(args, "position")?.map(|(p, _)| p))
}

fn need_pos(args: &Value) -> Result<Vec2, String> {
    pos_arg(args)?.ok_or_else(|| "missing `position` [x, z]".into())
}

/// Facings in the MCP convention: `yaw_deg` (compass bearing, from the exact facing) instead of
/// the server data's rounded `yaw_rad` / `facing`.
fn bearings(v: &mut Value, gp: &Gameplay) {
    match v {
        Value::Object(o) => {
            if let Some(y) = o.remove("yaw_rad").and_then(|y| y.as_f64()) {
                o.remove("facing");
                // NPCs have ids; the player start has none
                let exact = match o.get("id").and_then(|i| i.as_u64()) {
                    Some(id) => gp.npcs.iter().find(|n| n.id as u64 == id).map(|n| n.yaw),
                    None => gp.player_start.map(|s| s.yaw),
                };
                o.insert(
                    "yaw_deg".into(),
                    json!(facing_bearing(exact.unwrap_or(y as f32))),
                );
            }
            o.values_mut().for_each(|c| bearings(c, gp));
        }
        Value::Array(a) => a.iter_mut().for_each(|c| bearings(c, gp)),
        _ => {}
    }
}

fn item_json(gp: &Gameplay, g: &Ground, sel: GameplaySel) -> Value {
    let data = super::export::server_data(gp, g, "");
    let (list, id) = match sel {
        GameplaySel::PlayerStart => {
            let mut v = data["player_start"].clone();
            bearings(&mut v, gp);
            return v;
        }
        GameplaySel::Poi(id) => ("pois", id),
        GameplaySel::Npc(id) => ("npcs", id),
        GameplaySel::Spawn(id) => ("spawn_regions", id),
        GameplaySel::Node(id) => ("resource_nodes", id),
        GameplaySel::OreArea(id) => ("deposit_areas", id),
    };
    let mut v = data[list]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == json!(id)).cloned())
        .unwrap_or(Value::Null);
    bearings(&mut v, gp);
    let hints: Vec<Value> = edit::issues(gp, g, sel)
        .into_iter()
        .map(|i| json!({"warning": i.warn, "text": i.text}))
        .collect();
    if let Some(o) = v.as_object_mut() {
        o.insert("hints".into(), json!(hints));
    }
    v
}

fn ok(v: Value) -> Result<Vec<McpContent>, String> {
    Ok(vec![McpContent::Json(v)])
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

/// A schema with a `position` (and the old `x` / `z` as quiet aliases).
fn schema_at(mut props: Value, required: &[&str]) -> Value {
    props["position"] = position_schema(XZ);
    let mut s = schema(props, required);
    s["x-aliases"] = xz_aliases();
    s
}

const XZ: &str =
    "World position (x east, z south; 0,0 = map centre); the height follows the ground";

pub(crate) fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "gameplay_list",
        description: "List the gameplay layer: player start, points of interest, NPCs, spawn regions (with resolved zone, levels and expected creature counts), the creature library, resource nodes and ore deposits (counts unless `nodes` is true), ore deposit areas, fishing spots, and every placement warning. Use it to find ids for gameplay_update / gameplay_remove.",
        input_schema: || schema(json!({"nodes": {"type": "boolean", "description": "List every resource node and ore deposit (default: counts per kind and ore)"}}), &[]),
        run: |world, args| {
            let full: bool = opt_arg(&args, "nodes")?.unwrap_or(false);
            let v = read(world, |gp, g, meta| {
                let mut v = super::export::server_data(gp, g, &meta.name);
                v["level_cap"] = json!(gp.level_cap);
                if !full {
                    let mut counts = serde_json::Map::new();
                    for k in ResourceKind::ALL {
                        let n = gp.nodes.iter().filter(|n| n.kind == k).count();
                        if n > 0 {
                            counts.insert(format!("{k:?}"), json!({"count": n, "scattered": gp.nodes.iter().filter(|x| x.kind == k && x.auto).count()}));
                        }
                    }
                    v["resource_nodes"] = Value::Object(counts);
                    let mut by_ore = serde_json::Map::new();
                    for o in Ore::ALL {
                        let n = gp.nodes.iter().filter(|n| n.ore() == Some(o)).count();
                        if n > 0 {
                            by_ore.insert(o.id().into(), json!(n));
                        }
                    }
                    v["ore_deposits"] = Value::Object(by_ore);
                    if let Some(o) = v.as_object_mut() {
                        o.remove("ores");
                    }
                    for a in v["deposit_areas"].as_array_mut().into_iter().flatten() {
                        if let Some(o) = a.as_object_mut() {
                            o.remove("deposit_ids");
                        }
                    }
                }
                bearings(&mut v, gp);
                v["warnings"] = json!(edit::all_warnings(gp, g)
                    .into_iter()
                    .map(|(s, t)| {
                        let id = match s {
                            GameplaySel::PlayerStart => json!("player_start"),
                            GameplaySel::Poi(id) | GameplaySel::Npc(id) | GameplaySel::Spawn(id) | GameplaySel::Node(id) | GameplaySel::OreArea(id) => json!(id),
                        };
                        json!({"id": id, "name": edit::sel_label(gp, s), "text": t})
                    })
                    .collect::<Vec<_>>());
                v
            })?;
            ok(v)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_set_player_start",
        description: "Set where new players enter the world (`position` [x, z]) and which way they face (`yaw_deg`, a compass bearing).",
        input_schema: || schema_at(json!({"yaw_deg": bearing_schema("Which way new players face (default: keep, else north)")}), &[]),
        run: |world, args| {
            let p = need_pos(&args)?;
            let yaw = yaw_arg(&args)?;
            edit(world, "Set player start", |gp, g, _, _| {
                let yaw = yaw.or(gp.player_start.map(|s| s.yaw)).unwrap_or(facing_yaw(0.0));
                edit::set_player_start(gp, g, p, yaw);
                Ok(item_json(gp, g, GameplaySel::PlayerStart))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_set_level_cap",
        description: "Set the world's intended progression ceiling (`level_cap`, 1 through 200), or clear it with null. Auto zones requires an explicit cap.",
        input_schema: || schema(json!({
            "level_cap": {
                "type": ["integer", "null"],
                "minimum": 1,
                "maximum": 200,
                "description": "Highest intended player level, or null to clear it"
            }
        }), &["level_cap"]),
        run: |world, args| {
            let level_cap = level_cap_arg(&args)?;
            edit(world, "Set level cap", move |gp, _, _, _| {
                gp.level_cap = level_cap;
                Ok(json!({"level_cap": gp.level_cap}))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_poi",
        description: "Add a point of interest at `position` [x, z] (`kind`: town, quest hub, dungeon (an entrance), landmark, camp, vendor, flight point, graveyard, portal, boss arena, shrine, vista, treasure, custom). Names default to the kind (\"Dungeon Entrance 2\"); the discovery radius defaults by kind; the level range defaults to the zone's. NPCs within 60 m that belong nowhere yet join it.",
        input_schema: || schema_at(json!({
            "kind": {"type": "string", "enum": PoiKind::ALL.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>()},
            "name": {"type": "string"},
            "radius": {"type": "number", "description": "Discovery radius (m)"},
            "level": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2, "description": "Recommended [min, max] level (default: the zone's)"},
            "notes": {"type": "string"},
        }), &["kind"]),
        run: |world, args| {
            let kind = poi_kind(&arg::<String>(&args, "kind")?)?;
            let at = need_pos(&args)?;
            let name: Option<String> = opt_arg(&args, "name")?;
            let radius: Option<f32> = opt_arg(&args, "radius")?;
            let level: Option<[u32; 2]> = opt_arg(&args, "level")?;
            let notes: Option<String> = opt_arg(&args, "notes")?;
            edit(world, &format!("Place {}", kind.name()), |gp, g, _, _| {
                let id = edit::add_poi(gp, g, kind, at, name);
                if let Some(q) = gp.pois.iter_mut().find(|q| q.id == id) {
                    q.radius = radius.unwrap_or(q.radius).max(1.0);
                    q.level = level.map(levels);
                    q.notes = notes.unwrap_or_default();
                }
                Ok(item_json(gp, g, GameplaySel::Poi(id)))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_npc",
        description: "Add an NPC at `position` [x, z] facing `yaw_deg` (`role`: quest giver, vendor, trainer, guard, innkeeper, flight master, banker, ambient, custom). It joins the nearest point of interest within 60 m unless `poi` is given (an id, or null for none).",
        input_schema: || schema_at(json!({
            "role": {"type": "string", "enum": NpcRole::ALL.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>()},
            "name": {"type": "string"},
            "poi": {"type": ["integer", "null"], "description": "POI id to belong to (null: none; omitted: nearest within 60 m)"},
            "yaw_deg": bearing_schema("Which way the NPC faces (default north)"),
            "notes": {"type": "string"},
        }), &["role"]),
        run: |world, args| {
            let role = npc_role(&arg::<String>(&args, "role")?)?;
            let at = need_pos(&args)?;
            let name: Option<String> = opt_arg(&args, "name")?;
            let poi: Option<Option<u32>> = if args.get("poi").is_some() { Some(opt_arg(&args, "poi")?) } else { None };
            let yaw = yaw_arg(&args)?.unwrap_or(facing_yaw(0.0));
            let notes: Option<String> = opt_arg(&args, "notes")?;
            edit(world, &format!("Place {}", role.name()), |gp, g, _, _| {
                if let Some(Some(pid)) = poi
                    && !gp.pois.iter().any(|q| q.id == pid)
                {
                    return Err(format!("no point of interest with id {pid}"));
                }
                let id = edit::add_npc(gp, g, role, at, yaw, name, poi);
                if let Some(n) = gp.npcs.iter_mut().find(|n| n.id == id) {
                    n.notes = notes.unwrap_or_default();
                }
                Ok(item_json(gp, g, GameplaySel::Npc(id)))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_creature",
        description: "Add a creature type to the library that spawn regions draw from. With `starter: true` (and no name) adds a starter fantasy bestiary of 14 creatures (levels 1–30) instead.",
        input_schema: || schema(json!({
            "name": {"type": "string"},
            "family": {"type": "string", "description": "Model / family group, e.g. wolf, bandit, spider"},
            "level": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2},
            "aggression": {"type": "string", "enum": ["Passive", "Neutral", "Hostile"]},
            "elite": {"type": "boolean"},
            "notes": {"type": "string"},
            "starter": {"type": "boolean"},
        }), &[]),
        run: |world, args| {
            if opt_arg::<bool>(&args, "starter")?.unwrap_or(false) && args.get("name").is_none() {
                let n = edit(world, "Add starter creatures", |gp, _, _, _| Ok(edit::add_starter_creatures(gp)))?;
                return ok(read(world, |gp, _, _| json!({"added": n, "creatures": gp.creatures}))?);
            }
            let name: String = arg(&args, "name")?;
            let family: String = opt_arg(&args, "family")?.unwrap_or_default();
            let level = levels(opt_arg(&args, "level")?.unwrap_or([1, 5]));
            let aggr = aggression(&opt_arg::<String>(&args, "aggression")?.unwrap_or_else(|| "Hostile".into()))?;
            let elite: bool = opt_arg(&args, "elite")?.unwrap_or(false);
            let notes: String = opt_arg(&args, "notes")?.unwrap_or_default();
            edit(world, "Add creature", |gp, _, _, _| {
                let id = edit::add_creature(gp, &name, &family, level, aggr, elite);
                let c = gp.creatures.iter_mut().find(|c| c.id == id).ok_or("creature not added")?;
                c.notes = notes;
                Ok(json!(c))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_spawn_region",
        description: "Add a creature spawn region: a ground polygon (or a centre + radius) with an encounter table of library creatures (ids or names, optional weights). Levels default to the zone the region lies in; density is creatures alive at once per km².",
        input_schema: || schema(json!({
            "polygon": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "Outline [[x,z],…] in world metres (≥3 corners)"},
            "center": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "Instead of polygon: centre [x,z]…"},
            "radius": {"type": "number", "description": "…and radius (m)"},
            "creatures": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "Creature ids or names from the library"},
            "weights": {"type": "array", "items": {"type": "number"}, "description": "Relative weight per creature (default 1)"},
            "name": {"type": "string"},
            "level": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2},
            "density": {"type": "number", "description": "Creatures alive at once per km² (default 300)"},
            "max_alive": {"type": "integer", "description": "Hard cap (0 = density only)"},
            "respawn_secs": {"type": "number"},
            "pack": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2, "description": "Creatures per group [min, max]"},
            "behaviour": {"type": "string", "enum": ["Roam", "Patrol", "Static", "Ambush"]},
            "notes": {"type": "string"},
        }), &[]),
        run: |world, args| {
            let poly = polygon(&args)?.ok_or("give `polygon` or `center` + `radius`")?;
            edit(world, "Draw spawn region", |gp, g, _, _| {
                let table = entries(gp, &args)?.unwrap_or_default();
                let ids: Vec<u32> = table.iter().map(|e| e.creature).collect();
                let id = edit::add_spawn(gp, g, poly, &ids, opt_arg(&args, "name")?)?;
                let r = gp.spawns.iter_mut().find(|s| s.id == id).ok_or("region not added")?;
                r.entries = table;
                apply_spawn_fields(r, &args)?;
                Ok(item_json(gp, g, GameplaySel::Spawn(id)))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_resource_node",
        description: "Add a gathering node by hand at `position` [x, z] (`kind`: Ore, Herb, Wood, Stone, Crystal, Custom). Tier defaults to the zone's level range (tier 1 = levels 1–10, 2 = 11–20, …); the variant defaults by kind, tier and biome (e.g. \"Silverleaf\"). An Ore node gets an ore that suits the tier and ground and shows as a deposit rock; to pick the ore, use gameplay_add_ore_deposit.",
        input_schema: || schema_at(json!({
            "kind": {"type": "string", "enum": ResourceKind::ALL.iter().map(|k| format!("{k:?}")).collect::<Vec<_>>()},
            "variant": {"type": "string"},
            "tier": {"type": "integer", "minimum": 1, "maximum": 8},
            "respawn_secs": {"type": "number"},
        }), &["kind"]),
        run: |world, args| {
            let kind = resource_kind(&arg::<String>(&args, "kind")?)?;
            let at = need_pos(&args)?;
            let variant: Option<String> = opt_arg(&args, "variant")?;
            let tier: Option<u8> = opt_arg(&args, "tier")?;
            let respawn: Option<f32> = opt_arg(&args, "respawn_secs")?;
            edit(world, &format!("Place {} node", kind.name()), |gp, g, _, _| {
                let id = edit::add_node(gp, g, kind, at, variant, tier);
                if let (Some(r), Some(n)) = (respawn, gp.nodes.iter_mut().find(|n| n.id == id)) {
                    n.respawn_secs = r.max(1.0);
                }
                Ok(item_json(gp, g, GameplaySel::Node(id)))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_scatter_resources",
        description: "Automatically place gathering nodes where they make sense: ore on rocky, steep, high ground and by cliff rocks; herbs on grassland and forest floors; wood at grown trees; stone at boulders and rocky ground; crystals on high peaks and volcanic rock. Never under water or on unreachable cliffs; tier follows the zone's levels. Ore nodes become deposit rocks of an ore of the zone's tier that suits the ground (iron and coal in hills, mithril on alpine peaks, obsidian and dragonsteel in volcanic land, voidstone in haunted woods). Deterministic per seed. Replaces earlier scattered nodes of the chosen kinds (hand-placed nodes and deposit areas stay). `clear: true` only removes scattered nodes.",
        input_schema: || schema(json!({
            "density": {"type": "number", "description": "× typical MMO density (default 1, 0.1–3)"},
            "seed": {"type": "integer"},
            "kinds": {"type": "array", "items": {"type": "string", "enum": ["Ore", "Herb", "Wood", "Stone", "Crystal"]}, "description": "Default Ore, Herb, Wood, Stone"},
            "clear": {"type": "boolean"},
        }), &[]),
        run: |world, args| {
            if opt_arg::<bool>(&args, "clear")?.unwrap_or(false) {
                let n = edit(world, "Clear scattered resources", |gp, _, _, _| Ok(scatter::clear(gp)))?;
                return ok(json!({"removed": n}));
            }
            let mut p = scatter::ScatterParams::default();
            if let Some(d) = opt_arg::<f32>(&args, "density")? {
                p.density = d.clamp(0.0, 5.0);
            }
            if let Some(s) = opt_arg::<u32>(&args, "seed")? {
                p.seed = s;
            }
            if let Some(k) = opt_arg::<Vec<String>>(&args, "kinds")? {
                p.kinds = k.iter().map(|k| resource_kind(k)).collect::<Result<_, _>>()?;
            }
            let t0 = std::time::Instant::now();
            let (added, removed, by_kind) = edit(world, "Scatter resources", |gp, g, store, lib| {
                let p = scatter::ScatterParams { ore: gp.ore_settings.clone(), ..p.clone() };
                let (a, r) = scatter::apply(gp, &p, g, store, lib);
                let by: serde_json::Map<String, Value> = p.kinds.iter().map(|k| (format!("{k:?}"), json!(gp.nodes.iter().filter(|n| n.auto && n.kind == *k).count()))).collect();
                Ok((a, r, by))
            })?;
            ok(json!({"added": added, "removed": removed, "by_kind": by_kind, "seed": p.seed, "density": p.density, "ms": t0.elapsed().as_millis()}))
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_update",
        description: "Change fields of an item by id (or id \"player_start\"), creatures included. Only the given fields change. Common: name, notes, position (move), yaw_deg (compass bearing). POI: kind, radius, level (or null for the zone's), tags. NPC: role, poi (id or null). Spawn region: polygon or center+radius, creatures (+weights), level, density, max_alive, respawn_secs, pack, behaviour. Node: kind, variant, tier, respawn_secs, ore (an ore id: the node becomes that ore's deposit). Ore deposit area: polygon or center+radius, ores (+weights), density (per hectare), respawn_secs, seed; its deposits are re-scattered. Creature: family, level, aggression, elite.",
        input_schema: || schema_at(json!({
            "id": {"description": "Item or creature id, or \"player_start\"", "oneOf": [{"type": "integer"}, {"type": "string", "enum": ["player_start"]}]},
            "name": {"type": "string"}, "notes": {"type": "string"},
            "yaw_deg": bearing_schema("Which way it faces (player start, NPCs)"),
            "kind": {"type": "string"}, "role": {"type": "string"}, "radius": {"type": "number"},
            "level": {"type": ["array", "null"], "items": {"type": "integer"}},
            "tags": {"type": "array", "items": {"type": "string"}},
            "poi": {"type": ["integer", "null"]},
            "polygon": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "Spawn region outline [[x,z],…]"},
            "center": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "Spawn region: centre [x,z] (with radius)"},
            "creatures": {"type": "array", "items": {"type": ["integer", "string"]}, "description": "Creature ids or names"},
            "weights": {"type": "array", "items": {"type": "number"}},
            "density": {"type": "number"}, "max_alive": {"type": "integer"}, "respawn_secs": {"type": "number"},
            "pack": {"type": "array", "items": {"type": "integer"}, "minItems": 2, "maxItems": 2}, "behaviour": {"type": "string", "enum": ["Roam", "Patrol", "Static", "Ambush"]},
            "variant": {"type": "string"}, "tier": {"type": "integer"},
            "ore": {"type": "string", "description": "Node: ore id (see gameplay_list_ores)"},
            "ores": {"type": "array", "items": {"type": "string"}, "description": "Deposit area: ore ids or names"},
            "seed": {"type": "integer"},
            "family": {"type": "string"}, "aggression": {"type": "string"}, "elite": {"type": "boolean"},
        }), &["id"]),
        run: |world, args| {
            let id_v: Value = arg(&args, "id")?;
            let sel = match &id_v {
                Value::String(s) if norm(s) == "playerstart" => Some(GameplaySel::PlayerStart),
                _ => None,
            };
            let id = id_v.as_u64().map(|v| v as u32);
            edit_site(world, "Edit gameplay", |gp, site, _| update(gp, site, sel, id, &args)).and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_remove",
        description: "Remove an item or creature by id (or \"player_start\"). Removing a POI unlinks its NPCs; removing a creature takes it out of every encounter table; removing an ore deposit area removes its deposits.",
        input_schema: || schema(json!({"id": {"description": "Item or creature id, or \"player_start\"", "oneOf": [{"type": "integer"}, {"type": "string", "enum": ["player_start"]}]}}), &["id"]),
        run: |world, args| {
            let id_v: Value = arg(&args, "id")?;
            let what = edit(world, "Remove gameplay item", |gp, _, _, _| match &id_v {
                Value::String(s) if norm(s) == "playerstart" => edit::remove_sel(gp, GameplaySel::PlayerStart).ok_or_else(|| "there is no player start".to_string()),
                v => {
                    let id = v.as_u64().ok_or("`id` must be a number or \"player_start\"")? as u32;
                    edit::remove(gp, id).ok_or_else(|| format!("nothing with id {id}"))
                }
            })?;
            Ok(vec![McpContent::Text(format!("Removed {what}"))])
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_list_ores",
        description: "The 25 mineable ores in five tiers (1 Common … 5 Legendary): id, name, tier, rarity, typical use, where they form (biomes, height band 0 = sea level … 1 = highest peak, cliffs), deposit name, default respawn, usual companion ore. Ids work in every ore argument.",
        input_schema: || schema(json!({"tier": {"type": "integer", "minimum": 1, "maximum": 5, "description": "Only this tier"}}), &[]),
        run: |_, args| {
            let tier: Option<u8> = opt_arg(&args, "tier")?;
            ok(json!(Ore::ALL.into_iter().filter(|o| tier.is_none_or(|t| o.tier() == t)).map(ore_json).collect::<Vec<_>>()))
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_ore_deposit",
        description: "Place one mineable ore deposit (a photoscanned rock with the ore in it) at `position` [x, z]. `ore`: an id or name from gameplay_list_ores (copper, iron, mithril, dark_iron, dragonsteel…).",
        input_schema: || schema_at(json!({
            "ore": {"type": "string", "enum": Ore::ALL.map(|o| o.id())},
            "name": {"type": "string", "description": "Default e.g. \"Iron Vein\""},
            "respawn_secs": {"type": "number", "description": "Default by tier (300 s … 1800 s)"},
        }), &["ore"]),
        run: |world, args| {
            let o = ore(&arg::<String>(&args, "ore")?)?;
            let at = need_pos(&args)?;
            let name: Option<String> = opt_arg(&args, "name")?;
            let respawn: Option<f32> = opt_arg(&args, "respawn_secs")?;
            edit(world, &format!("Place {} deposit", o.name()), |gp, g, _, _| {
                let id = ores::add_deposit(gp, g, o, at);
                if let Some(n) = gp.nodes.iter_mut().find(|n| n.id == id) {
                    if let Some(v) = name.filter(|v| !v.trim().is_empty()) {
                        n.variant = v;
                    }
                    if let Some(r) = respawn {
                        n.respawn_secs = r.max(1.0);
                    }
                }
                Ok(item_json(gp, g, GameplaySel::Node(id)))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_ore_settings",
        description: "Read or change how ore fills the world (saved with it): `on_generate` (scatter ore whenever a world is generated, default true), `abundance` (sparse / normal / rich), `mountains_only` (only on high, steep or alpine ground), `tier_mix` ([common, uncommon, rare, epic, legendary] shares in %, default [60, 25, 10, 4, 1]; applies outside zones — zones follow their level range), `ores_off` (ore ids that never appear), `ore_weights` ({ore id: ×weight within its tier}). Without arguments it returns the current settings. Then run gameplay_scatter_resources with kinds [\"Ore\"] to apply them.",
        input_schema: || schema(json!({
            "on_generate": {"type": "boolean"},
            "abundance": {"type": "string", "enum": ["sparse", "normal", "rich"]},
            "mountains_only": {"type": "boolean"},
            "tier_mix": {"type": "array", "items": {"type": "number", "minimum": 0}, "minItems": 5, "maxItems": 5},
            "ores_off": {"type": "array", "items": {"type": "string"}},
            "ore_weights": {"type": "object", "additionalProperties": {"type": "number", "minimum": 0}},
        }), &[]),
        run: |world, args| {
            let mut st = world.resource::<Gameplay>().ore_settings.clone();
            if let Some(v) = opt_arg::<bool>(&args, "on_generate")? {
                st.on_generate = v;
            }
            if let Some(a) = opt_arg::<String>(&args, "abundance")? {
                st.abundance = ores::Abundance::ALL.into_iter().find(|x| x.id() == a.to_lowercase()).ok_or("abundance: sparse, normal or rich")?;
            }
            if let Some(v) = opt_arg::<bool>(&args, "mountains_only")? {
                st.mountains_only = v;
            }
            if let Some(m) = opt_arg::<Vec<f32>>(&args, "tier_mix")? {
                if m.len() != 5 || m.iter().any(|v| !v.is_finite() || *v < 0.0) || m.iter().sum::<f32>() <= 0.0 {
                    return Err("tier_mix: five shares ≥ 0, not all 0".into());
                }
                st.tier_mix = [m[0], m[1], m[2], m[3], m[4]];
            }
            if let Some(off) = opt_arg::<Vec<String>>(&args, "ores_off")? {
                st.off = off.iter().map(|o| ore(o)).collect::<Result<_, _>>()?;
            }
            if let Some(w) = opt_arg::<std::collections::BTreeMap<String, f32>>(&args, "ore_weights")? {
                for (k, v) in w {
                    st.set_weight(ore(&k)?, v);
                }
            }
            let changed = st != world.resource::<Gameplay>().ore_settings;
            if changed {
                world.resource_mut::<Gameplay>().ore_settings = st.clone();
                world.resource_mut::<crate::world::WorldMeta>().dirty = true;
            }
            let mix = st.mix();
            ok(json!({
                "on_generate": st.on_generate, "abundance": st.abundance.id(), "mountains_only": st.mountains_only,
                "tier_mix_percent": mix.map(|v| (v * 1000.0).round() / 10.0),
                "ores_off": st.off.iter().map(|o| o.id()).collect::<Vec<_>>(),
                "ore_weights": st.weights.iter().map(|(o, w)| (o.id().to_string(), json!(w))).collect::<serde_json::Map<String, Value>>(),
                "changed": changed,
            }))
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_add_ore_area",
        description: "Add an ore deposit area: a ground polygon (or centre + radius) with an ore table (`ores`: ids or names, optional `weights`). Deposits are scattered inside on suitable ground (not in water, on roads, paving or by buildings, not on steep cliffs unless the ore forms in them), deterministically per `seed`, and re-scattered whenever the area changes (gameplay_update). Default table: the first ore plus its usual companion at a third of the weight when only one ore is given; density in deposits per hectare defaults by tier (60 common … 20 legendary).",
        input_schema: || schema(json!({
            "polygon": {"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "description": "Outline [[x,z],…] in world metres (≥3 corners)"},
            "center": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "Instead of polygon: centre [x,z]…"},
            "radius": {"type": "number", "description": "…and radius (m)"},
            "ores": {"type": "array", "items": {"type": "string"}, "minItems": 1, "description": "Ore ids or names"},
            "weights": {"type": "array", "items": {"type": "number"}, "description": "Relative weight per ore (default 1)"},
            "companion": {"type": "boolean", "description": "With a single ore: mix in its usual companion (default true)"},
            "density": {"type": "number", "description": "Deposits per hectare (10 000 m²)"},
            "respawn_secs": {"type": "number"},
            "seed": {"type": "integer"},
            "name": {"type": "string"},
            "notes": {"type": "string"},
        }), &["ores"]),
        run: |world, args| {
            let poly = polygon(&args)?.ok_or("give `polygon` or `center` + `radius`")?;
            let mut table = ore_table(&args)?.filter(|t| !t.is_empty()).ok_or("give at least one ore in `ores`")?;
            if table.len() == 1 && opt_arg::<bool>(&args, "companion")?.unwrap_or(true) {
                table = ores::default_table(table[0].ore);
            }
            let name: Option<String> = opt_arg(&args, "name")?;
            edit_site(world, "Draw deposit area", |gp, site, _| {
                let id = ores::add_area(gp, site, poly, table, name)?;
                if let Some(a) = gp.ore_areas.iter_mut().find(|a| a.id == id) {
                    apply_area_fields(a, &args)?;
                }
                ores::rescatter(gp, id, site);
                Ok(item_json(gp, site.g, GameplaySel::OreArea(id)))
            })
            .and_then(ok)
        },
    })
    .mcp_tool(McpTool {
        name: "gameplay_export_server_data",
        description: "Write gameplay.json (player start, POIs, NPCs, spawn regions with resolved zones / levels / expected counts, creature library, resource nodes, ore deposits and deposit areas, the ore catalogue, fishing spots; format in docs/server-data.md) into a folder. Default folder: exports/<world name>/.",
        input_schema: || schema(json!({"dir": {"type": "string", "description": "Output folder (created if missing)"}}), &[]),
        run: |world, args| {
            let dir: Option<String> = opt_arg(&args, "dir")?;
            let mut state: SystemState<ReadState> = SystemState::new(world);
            let (gp, terrain, zones, water, meta) = state.get(world).map_err(|e| e.to_string())?;
            let dir = dir.map(std::path::PathBuf::from).unwrap_or_else(|| {
                let name: String = meta.name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
                crate::export::exports_dir().join(name)
            });
            let path = super::write_server_data(&dir, &gp, &terrain, &zones, &water, &meta.name)?;
            ok(json!({"path": path.display().to_string(), "pois": gp.pois.len(), "npcs": gp.npcs.len(), "spawn_regions": gp.spawns.len(), "creatures": gp.creatures.len(), "resource_nodes": gp.nodes.len(), "fishing_spots": water.fish.len()}))
        },
    });
}

/// Spawn-region fields shared by add and update.
fn apply_spawn_fields(r: &mut super::SpawnRegion, args: &Value) -> Result<(), String> {
    if let Some(l) = opt_arg::<[u32; 2]>(args, "level")? {
        r.level = levels(l);
    }
    if let Some(d) = opt_arg::<f32>(args, "density")? {
        r.density = d.max(0.0);
    }
    if let Some(m) = opt_arg::<u32>(args, "max_alive")? {
        r.max_alive = m;
    }
    if let Some(s) = opt_arg::<f32>(args, "respawn_secs")? {
        r.respawn_secs = s.max(1.0);
    }
    if let Some(p) = opt_arg::<[u32; 2]>(args, "pack")? {
        let a = p[0].clamp(1, 50) as u8;
        r.pack = [a, (p[1].clamp(1, 50) as u8).max(a)];
    }
    if let Some(b) = opt_arg::<String>(args, "behaviour")? {
        r.behaviour = behaviour(&b)?;
    }
    if let Some(n) = opt_arg::<String>(args, "notes")? {
        r.notes = n;
    }
    Ok(())
}

fn update(
    gp: &mut Gameplay,
    site: &Site,
    sel: Option<GameplaySel>,
    id: Option<u32>,
    args: &Value,
) -> Result<Value, String> {
    let g = site.g;
    let name: Option<String> = opt_arg(args, "name")?;
    let notes: Option<String> = opt_arg(args, "notes")?;
    let yaw = yaw_arg(args)?;
    let xz = pos_arg(args)?;
    // creatures live outside the selection kinds
    if sel.is_none()
        && let Some(id) = id
        && let Some(i) = gp.creatures.iter().position(|c| c.id == id)
    {
        let c = &mut gp.creatures[i];
        if let Some(n) = name {
            c.name = n;
        }
        if let Some(n) = notes {
            c.notes = n;
        }
        if let Some(f) = opt_arg::<String>(args, "family")? {
            c.family = f;
        }
        if let Some(l) = opt_arg::<[u32; 2]>(args, "level")? {
            c.level = levels(l);
        }
        if let Some(a) = opt_arg::<String>(args, "aggression")? {
            c.aggression = aggression(&a)?;
        }
        if let Some(e) = opt_arg::<bool>(args, "elite")? {
            c.elite = e;
        }
        return Ok(json!(c));
    }
    let sel = sel
        .or_else(|| id.and_then(|id| gp.find(id)))
        .ok_or_else(|| {
            format!(
                "nothing with id {}",
                id.map(|i| i.to_string()).unwrap_or_default()
            )
        })?;
    if let Some(p) = xz {
        if sel == GameplaySel::PlayerStart && gp.player_start.is_none() {
            return Err("there is no player start (use gameplay_set_player_start)".into());
        }
        edit::move_to(gp, g, sel, p);
    }
    match sel {
        GameplaySel::PlayerStart => {
            let s = gp.player_start.as_mut().ok_or("there is no player start")?;
            if let Some(y) = yaw {
                s.yaw = y;
            }
        }
        GameplaySel::Poi(id) => {
            let level_given = args.get("level").is_some();
            let level: Option<[u32; 2]> = opt_arg(args, "level")?;
            let kind = opt_arg::<String>(args, "kind")?
                .map(|k| poi_kind(&k))
                .transpose()?;
            let q = gp
                .pois
                .iter_mut()
                .find(|q| q.id == id)
                .ok_or("POI not found")?;
            if let Some(n) = name {
                q.name = n;
            }
            if let Some(n) = notes {
                q.notes = n;
            }
            if let Some(k) = kind {
                q.kind = k;
            }
            if let Some(r) = opt_arg::<f32>(args, "radius")? {
                q.radius = r.max(1.0);
            }
            if level_given {
                q.level = level.map(levels);
            }
            if let Some(t) = opt_arg::<Vec<String>>(args, "tags")? {
                q.tags = t;
            }
        }
        GameplaySel::Npc(id) => {
            let poi: Option<Option<u32>> = if args.get("poi").is_some() {
                Some(opt_arg(args, "poi")?)
            } else {
                None
            };
            if let Some(Some(pid)) = poi
                && !gp.pois.iter().any(|q| q.id == pid)
            {
                return Err(format!("no point of interest with id {pid}"));
            }
            let role = opt_arg::<String>(args, "role")?
                .map(|r| npc_role(&r))
                .transpose()?;
            let n = gp
                .npcs
                .iter_mut()
                .find(|q| q.id == id)
                .ok_or("NPC not found")?;
            if let Some(v) = name {
                n.name = v;
            }
            if let Some(v) = notes {
                n.notes = v;
            }
            if let Some(r) = role {
                n.role = r;
            }
            if let Some(p) = poi {
                n.poi = p;
            }
            if let Some(y) = yaw {
                n.yaw = y;
            }
        }
        GameplaySel::Spawn(id) => {
            let table = entries(gp, args)?;
            let poly = polygon(args)?;
            if poly
                .as_ref()
                .is_some_and(|p| p.len() < 3 || edit::polygon_area(p) < 150.0)
            {
                return Err("the polygon needs ≥3 corners and ~150 m²".into());
            }
            let r = gp
                .spawns
                .iter_mut()
                .find(|q| q.id == id)
                .ok_or("spawn region not found")?;
            if let Some(n) = name {
                r.name = n;
            }
            if let Some(t) = table {
                r.entries = t;
            }
            if let Some(p) = poly {
                r.polygon = p;
            }
            apply_spawn_fields(r, args)?;
        }
        GameplaySel::Node(id) => {
            let kind = opt_arg::<String>(args, "kind")?
                .map(|k| resource_kind(&k))
                .transpose()?;
            let new_ore = opt_arg::<String>(args, "ore")?
                .map(|o| ore(&o))
                .transpose()?;
            let n = gp
                .nodes
                .iter_mut()
                .find(|q| q.id == id)
                .ok_or("node not found")?;
            if let Some(k) = kind {
                n.kind = k;
            }
            if let Some(o) = new_ore {
                // an automatic name follows the ore
                if n.ore().is_none_or(|old| n.variant == old.deposit_name()) {
                    n.variant = o.deposit_name();
                }
                (n.ore, n.kind, n.tier, n.area) = (Some(o), o.kind(), o.tier(), None);
            }
            if let Some(v) = opt_arg::<String>(args, "variant")?.or(name) {
                n.variant = v;
            }
            if let Some(t) = opt_arg::<u8>(args, "tier")? {
                n.tier = t.clamp(1, 8);
            }
            if let Some(r) = opt_arg::<f32>(args, "respawn_secs")? {
                n.respawn_secs = r.max(1.0);
            }
            n.auto = false;
        }
        GameplaySel::OreArea(id) => {
            let table = ore_table(args)?;
            let poly = polygon(args)?;
            if poly
                .as_ref()
                .is_some_and(|p| p.len() < 3 || edit::polygon_area(p) < 100.0)
            {
                return Err("the polygon needs ≥3 corners and ~100 m²".into());
            }
            let a = gp
                .ore_areas
                .iter_mut()
                .find(|q| q.id == id)
                .ok_or("deposit area not found")?;
            if let Some(n) = name {
                a.name = n;
            }
            if let Some(t) = table {
                a.table = t;
            }
            if let Some(p) = poly {
                a.polygon = p;
            }
            apply_area_fields(a, args)?;
            ores::rescatter(gp, id, site);
        }
    }
    gp.dirty = true;
    Ok(item_json(gp, g, sel))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::level_cap_arg;

    #[test]
    fn level_cap_argument_requires_an_integer_in_range() {
        assert_eq!(level_cap_arg(&json!({"level_cap": 1})).unwrap(), Some(1));
        assert_eq!(
            level_cap_arg(&json!({"level_cap": 200})).unwrap(),
            Some(200)
        );
        assert_eq!(level_cap_arg(&json!({"level_cap": null})).unwrap(), None);
        for value in [json!(0), json!(201), json!(1.5), json!("60")] {
            assert!(level_cap_arg(&json!({"level_cap": value})).is_err());
        }
    }
}
