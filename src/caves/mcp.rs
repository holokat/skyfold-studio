//! MCP tools for caves: the same edits as the Caves tab (`edit`), one undo step each.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::edit::{self, NewCave};
use super::layout;
use super::model::*;
use super::render::CaveWorld;
use crate::io::WorldData;
use crate::mcp_tools::{
    McpAppExt, McpContent, McpTool, bearing_dir, bearing_schema, dir_bearing, opt_arg,
    position_arg, position_schema, xz_aliases,
};

fn heading_of(bearing: f32) -> f32 {
    let v = bearing_dir(bearing);
    v.x.atan2(v.y)
}

fn bearing_of(heading: f32) -> f32 {
    dir_bearing(layout::dir(heading))
}

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// A cave as JSON (`detail`: its chambers and tunnels too).
pub fn cave_json(c: &Cave, detail: bool) -> Value {
    let (lo, hi) = c.bounds();
    let mut v = json!({
        "id": c.id,
        "name": c.name,
        "kind": c.kind.id(),
        "size": c.size.id(),
        "levels": c.levels,
        "levels_go": if c.up { "up" } else { "down" },
        "theme": c.theme.id(),
        "theme_in_use": c.theme().id(),
        "darkness": c.darkness.id(),
        "features": c.features,
        "seed": c.seed,
        "entrance": [r1(c.entrance[0]), r1(c.entrance[1]), r1(c.entrance[2])],
        "heading_deg": r1(bearing_of(c.heading)),
        "mouth": format!("{:?}", c.mouth).to_lowercase(),
        "chambers": c.chambers.len(),
        "tunnels": c.tunnels.len(),
        "deepest_floor": r1(c.chambers.iter().map(|c| c.pos[1]).fold(c.entrance[1], f32::min)),
        "bounds": { "min": [r1(lo.x), r1(lo.y), r1(lo.z)], "max": [r1(hi.x), r1(hi.y), r1(hi.z)] },
        "dungeon": c.dungeon.as_ref().map(|d| json!({ "name": d.name, "chamber": d.chamber, "door": [r1(d.door[0]), r1(d.door[1]), r1(d.door[2])], "door_heading_deg": r1(bearing_of(d.door_yaw)), "instancing": d.instancing.id() })),
    });
    if let Some(a) = &c.arch {
        v["arch"] = json!({ "foot_a": a.a, "foot_b": a.b, "height": r1(a.height), "thickness": r1(a.thickness) });
    }
    if detail {
        v["chamber_list"] = c
            .chambers
            .iter()
            .map(|ch| {
                let pool = if ch.pool { super::dress::pool_of(c, ch).map(|(p, r)| json!({ "centre": [r1(p.x), r1(p.y)], "radius": r1(r), "rim": ch.rim.id(), "rim_in_use": c.rim_of(ch).id() })) } else { None };
                json!({ "id": ch.id, "floor": [r1(ch.pos[0]), r1(ch.pos[1]), r1(ch.pos[2])], "size": [r1(ch.size[0] * 2.0), r1(ch.size[1]), r1(ch.size[2] * 2.0)], "level": ch.level, "locked": ch.locked, "pool": pool, "skylight": ch.skylight, "great_hall": ch.hall })
            })
            .collect();
        v["tunnel_list"] = c
            .tunnels
            .iter()
            .map(|t| json!({ "from": if t.a == OUTSIDE { json!("outside") } else { json!(t.a) }, "to": if t.b == OUTSIDE { json!("outside") } else { json!(t.b) }, "width": r1(t.width), "height": r1(t.height), "profile": format!("{:?}", t.profile).to_lowercase(), "floor_line": t.pts.iter().map(|p| [r1(p[0]), r1(p[1]), r1(p[2])]).collect::<Vec<_>>() }))
            .collect();
    }
    v
}

fn features_arg(args: &Value, base: Features) -> Result<Features, String> {
    let Some(f) = args.get("features").filter(|v| !v.is_null()) else {
        return Ok(base);
    };
    let o = f
        .as_object()
        .ok_or("`features` must be an object like {\"formations\": true, \"crystals\": false}")?;
    let mut out = base;
    for (k, v) in o {
        let b = v
            .as_bool()
            .ok_or_else(|| format!("feature `{k}` must be true or false"))?;
        match k.as_str() {
            "formations" | "stalactites" => out.formations = b,
            "pools" | "water" => out.pools = b,
            "crystals" => out.crystals = b,
            "rubble" => out.rubble = b,
            "skylights" => out.skylights = b,
            "lights" => out.lights = b,
            "bridges" | "ledges" | "arches" => out.bridges = b,
            "waterfalls" => out.waterfalls = b,
            "cobwebs" | "webs" => out.cobwebs = b,
            _ => {
                return Err(format!(
                    "unknown feature `{k}` (formations, pools, crystals, rubble, skylights, lights, bridges, waterfalls, cobwebs)"
                ));
            }
        }
    }
    Ok(out)
}

/// `levels_go`: "up" (true) or "down" (false).
fn levels_go(args: &Value) -> Result<Option<bool>, String> {
    match args.get("levels_go").and_then(|v| v.as_str()) {
        None => Ok(None),
        Some("up") => Ok(Some(true)),
        Some("down") => Ok(Some(false)),
        Some(s) => Err(format!("unknown levels_go `{s}` (down, up)")),
    }
}

fn enum_arg<T: Copy>(
    args: &Value,
    key: &str,
    parse: fn(&str) -> Option<T>,
    all: &str,
) -> Result<Option<T>, String> {
    match args.get(key).and_then(|v| v.as_str()) {
        None => Ok(None),
        Some(s) => parse(s)
            .map(Some)
            .ok_or_else(|| format!("unknown {key} `{s}` ({all})")),
    }
}

fn kinds() -> Vec<&'static str> {
    CaveKind::ALL.iter().map(|k| k.id()).collect()
}

fn recipe_schema(props: &mut serde_json::Map<String, Value>) {
    props.insert("kind".into(), json!({ "type": "string", "enum": kinds(), "description": "cave (mouth in a hillside; a pit on flat ground), mine (timbered adit / decline, straight drifts), pit (collapsed sinkhole with a scree ramp), passage (right through a hill, open at both ends), cavern (sealed underground, no entrance), arch (natural rock arch)" }));
    props.insert("size".into(), json!({ "type": "string", "enum": CaveSize::ALL.map(|s| s.id()), "description": "how big — inside as well as how many chambers: small (2 chambers), medium (4 + a branch), large (7, rooms 1.4× bigger, a loop), huge / XL (11, rooms 1.8×), giant / XXL (a great hall 60–80 m across and ~30 m high with chambers round it), colossal / XXXL (two vast halls 110–160 m across and 40–50 m high: an underground world)" }));
    props.insert("levels".into(), json!({ "type": "integer", "minimum": 1, "maximum": 4, "description": "levels stacked one over another, joined by walkable ramps (in tall chambers by ledges round the walls)" }));
    props.insert("levels_go".into(), json!({ "type": "string", "enum": ["down", "up"], "description": "down (default: each level deeper) or up (the levels climb into the hill, where it is high enough)" }));
    props.insert("theme".into(), json!({ "type": "string", "enum": Theme::ALL.map(|t| t.id()), "description": "auto follows the biome at the entrance (snow: ice, jungle / swamp: mossy, desert: sandstone, volcanic: volcanic; mines: mine)" }));
    props.insert("darkness".into(), json!({ "type": "string", "enum": Darkness::ALL.map(|d| d.id()), "description": "dark (pitch black away from light), dim (shapes read), readable (game-readable fill)" }));
    props.insert("features".into(), json!({ "type": "object", "description": "on / off: formations (stalactites, stalagmites, columns, sized to the room; icicles in ice caves), pools (still water with shelving shores; lakes in great halls), crystals (glowing crystal clusters that light the chambers; towering ones in great halls), rubble, skylights (daylight shafts), lights (lanterns), bridges (ledges climbing round tall chambers where the way changes level, rock bridges, natural arches over the way; on by default), waterfalls (pouring from high in the walls of tall chambers into pools), cobwebs (in corners and across dead-end passages)", "properties": { "formations": { "type": "boolean" }, "pools": { "type": "boolean" }, "crystals": { "type": "boolean" }, "rubble": { "type": "boolean" }, "skylights": { "type": "boolean" }, "lights": { "type": "boolean" }, "bridges": { "type": "boolean" }, "waterfalls": { "type": "boolean" }, "cobwebs": { "type": "boolean" } } }));
    props.insert("name".into(), json!({ "type": "string" }));
    props.insert("dungeon".into(), json!({ "description": "a dungeon behind a loading door in the deepest chamber: its name (\"\" for one made from the cave's name), or false to remove it", "type": ["string", "boolean"] }));
}

pub(super) fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "caves_create",
        description: "Add a cave where the ground was clicked, in one call — like Terrain › Caves › click. On a hillside a cave mouth opens in a rock face and chambers run into the hill (kept under at least 4.5 m of rock); on flat ground a cave starts as a sinkhole pit and a mine as a sloping decline. Levels stack one over another (each deeper, or climbing with levels_go up), joined by walkable ramps — in tall chambers by ledges climbing round the wall and rock bridges. Sizes go from small up to giant (XXL: a great hall 60–80 m across) and colossal (XXXL: two vast halls). The look follows the biome (theme auto). The ground round each opening is painted bare (rock / dirt / gravel) and plants are cleared. Arches: `position` is one foot, `span_to` the other (default: 16 m across the view). Returns the cave (id, entrance, bounds, chambers, dungeon door). The rock is built in the background (caves_wait). One undo step.",
        input_schema: || {
            let mut props = serde_json::Map::new();
            props.insert("position".into(), position_schema("where the entrance is (on the ground)"));
            recipe_schema(&mut props);
            props.insert("seed".into(), json!({ "type": "integer", "minimum": 0, "description": "layout (same seed and spot: same cave)" }));
            props.insert("heading_deg".into(), bearing_schema("which way the cave runs in from the entrance (default: straight uphill)"));
            props.insert("span_to".into(), json!({ "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "arches: the far foot [x, z]" }));
            json!({ "type": "object", "properties": props, "required": ["position"], "x-aliases": xz_aliases() })
        },
        run: |w, a| w.run_system_cached_with(mcp_create, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_pool_rim",
        description: "Choose the lip round a cave's pools — the rock that holds the water in: rimstone (a low rounded dam), scalloped (lobed gours), terraces (steps of travertine round it), boulders, shelf (a wide flat apron of flowstone), crust (sharp crystals), or auto (each pool its own, from the cave's seed). `chamber`: one pool (its chamber id, see caves_list detail); omitted: every pool of the cave. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" }, "chamber": { "type": "integer" }, "rim": { "type": "string", "enum": RimStyle::ALL.map(|r| r.id()) } }, "required": ["id", "rim"] }),
        run: |w, a| w.run_system_cached_with(mcp_pool_rim, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_list",
        description: "Every cave, mine, sinkhole, passage, cavern and arch: id, name, kind, size, levels, theme, entrance, bounds, dungeon door; with `detail` (or `id`) also each chamber (floor centre, size, level, locked) and tunnel.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer", "description": "just this cave (with detail)" }, "detail": { "type": "boolean" } } }),
        run: |w, a| {
            let caves = w.resource::<Caves>();
            let id: Option<u32> = opt_arg(&a, "id")?;
            let detail = a.get("detail").and_then(|v| v.as_bool()).unwrap_or(false) || id.is_some();
            let list: Vec<Value> = caves.list.iter().filter(|c| id.is_none_or(|i| c.id == i)).map(|c| cave_json(c, detail)).collect();
            if let Some(i) = id
                && list.is_empty()
            {
                return Err(format!("no cave {i}"));
            }
            Ok(vec![McpContent::Json(json!({ "caves": list, "dig_strokes": caves.strokes.len() }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "caves_set",
        description: "Change a cave's recipe — kind, size, levels, theme, darkness, features, name, dungeon (name, or false), instancing (shared / party / raid) — and lay it out again (same seed: it changes only where the recipe does; locked chambers stay). One undo step.",
        input_schema: || {
            let mut props = serde_json::Map::new();
            props.insert("id".into(), json!({ "type": "integer" }));
            recipe_schema(&mut props);
            props.insert("instancing".into(), json!({ "type": "string", "enum": Instancing::ALL.map(|i| i.id()), "description": "how the game runs the dungeon: one shared copy, a private copy per party, a raid" }));
            json!({ "type": "object", "properties": props, "required": ["id"] })
        },
        run: |w, a| w.run_system_cached_with(mcp_set, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_shuffle",
        description: "A new layout for a cave (the next seed, or `seed`), keeping locked chambers and joining the new layout up to them. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" }, "seed": { "type": "integer", "minimum": 0 } }, "required": ["id"] }),
        run: |w, a| w.run_system_cached_with(mcp_shuffle, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_extend",
        description: "Grow a cave: a new tunnel and chamber from chamber `from` (default: the deepest) towards `heading_deg` (default: on away from the entrance), a level lower with `down`. Fails if other chambers are in the way. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" }, "from": { "type": "integer", "description": "chamber id" }, "heading_deg": bearing_schema("which way it grows"), "down": { "type": "boolean", "description": "a level lower" } }, "required": ["id"] }),
        run: |w, a| w.run_system_cached_with(mcp_extend, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_lock",
        description: "Lock a chamber (Shuffle keeps it and lays the rest out round it) or unlock it.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" }, "chamber": { "type": "integer" }, "locked": { "type": "boolean", "default": true } }, "required": ["id", "chamber"] }),
        run: |w, a| w.run_system_cached_with(mcp_lock, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_remove",
        description: "Remove a cave (or arch). One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
        run: |w, a| w.run_system_cached_with(mcp_remove, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_dig",
        description: "Dig through rock (or `fill` it back) along a path of points [[x, y, z], …] with a sphere of `radius` m: tunnels by hand, arches cut through a ridge, an opening into a hidden cavern. Works on the terrain itself too (it opens where the dig breaks the surface). One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "points": { "type": "array", "minItems": 1, "items": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 } }, "radius": { "type": "number", "minimum": 0.5, "maximum": 12, "default": 2.5 }, "fill": { "type": "boolean" } }, "required": ["points"] }),
        run: |w, a| w.run_system_cached_with(mcp_dig, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_xray",
        description: "X-ray view (view only, not saved): cut away everything above `height` (m, world y) so you look down into the caves; null turns it off. With `id`: just under that cave's highest roof.",
        input_schema: || json!({ "type": "object", "properties": { "height": { "type": ["number", "null"] }, "id": { "type": "integer" } } }),
        run: |w, a| {
            let id: Option<u32> = opt_arg(&a, "id")?;
            let h: Option<f32> = match id {
                Some(i) => {
                    let c = w.resource::<Caves>().get(i).ok_or_else(|| format!("no cave {i}"))?;
                    Some(c.chambers.iter().map(|c| c.pos[1] + c.size[1]).fold(f32::MIN, f32::max) - 0.5)
                }
                None => opt_arg(&a, "height")?,
            };
            w.resource_mut::<CaveWorld>().xray = h;
            Ok(vec![McpContent::Json(json!({ "xray": h }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "caves_walk_check",
        description: "Check a cave can be walked: a walker with the real walk-mode physics (no rendering) starts outside the way in and follows every tunnel to every chamber and back. Reports the chambers reached and missed, where it got stuck and how often it ended up out on the hillside.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" } }, "required": ["id"] }),
        run: |w, a| {
            let id: u32 = crate::mcp_tools::arg(&a, "id")?;
            let r = super::physics::walk_check(w, id)?;
            Ok(vec![McpContent::Json(serde_json::to_value(&r).map_err(|e| e.to_string())?)])
        },
    });
    app.mcp_tool(McpTool {
        name: "caves_probe",
        description: "Ask the cave rock about points [[x, y, z], …]: whether each is in a cave's space (carved out or under added rock), the floor under it and the roof over it there, and the rock field (m to the nearest rock surface; negative inside rock).",
        input_schema: || json!({ "type": "object", "properties": { "points": { "type": "array", "minItems": 1, "maxItems": 200, "items": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 } } }, "required": ["points"] }),
        run: |w, a| {
            let pts: Vec<[f32; 3]> = crate::mcp_tools::arg(&a, "points")?;
            let cw = w.resource::<CaveWorld>();
            let out: Vec<Value> = pts
                .iter()
                .map(|p| {
                    let p = Vec3::from_array(*p);
                    json!({ "point": [p.x, p.y, p.z], "in_cave": cw.in_space(p), "floor": cw.floor_under(p, 12.0).map(r1), "roof": cw.roof_over(p, 30.0).map(r1), "field": cw.field(p).map(|d| (d * 100.0).round() / 100.0) })
                })
                .collect();
            Ok(vec![McpContent::Json(json!({ "points": out }))])
        },
    });
    app.mcp_tool(McpTool {
        name: "caves_view",
        description: "Put the editor camera in a cave, the way its Decorate inside / chamber buttons do: in chamber `chamber` (default: the first a visitor reaches), turning round a spot at eye height over its floor and seen from the side with the longest view across it — or `outside` in front of its way in. While the camera's focus is on a cave's floor it stays in the cave: moves slide along the walls, the camera is held in front of the rock, and things placed with objects_place at [x, y, z] stay where they are (no levelled ground). `decorate` also opens Objects › Place. Returns the cave, chamber, the focus on the floor, the bearing the camera looks along and its distance.",
        input_schema: || json!({ "type": "object", "properties": { "id": { "type": "integer" }, "chamber": { "type": "integer", "description": "chamber id (caves_list with `id` lists them)" }, "outside": { "type": "boolean", "description": "in front of the way in instead" }, "decorate": { "type": "boolean", "description": "also open Objects › Place" } }, "required": ["id"] }),
        run: |w, a| w.run_system_cached_with(mcp_view, a).map_err(|e| format!("{e}"))?,
    });
    app.mcp_tool(McpTool {
        name: "caves_wait",
        description: "Wait until the caves' rock has been rebuilt after edits (it builds in the background), then report chunks and triangles drawn.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |_w, _a| {
            let mut frames = 0;
            Ok(vec![McpContent::deferred(move |w| {
                frames += 1;
                let cw = w.resource::<CaveWorld>();
                if cw.busy() && frames < 1800 {
                    return None;
                }
                Some(Ok(vec![McpContent::Json(json!({ "ready": !cw.busy(), "chunks": cw.chunks.len(), "holes": w.resource::<crate::terrain::holes::TerrainHoles>().cells.len() }))]))
            })])
        },
    });
}

fn new_from_args(args: &Value, d: &WorldData) -> Result<NewCave, String> {
    let (at, _) =
        position_arg(args, "position")?.ok_or("give `position` [x, z]: where the entrance is")?;
    let o = &*d.orbit;
    let cam = o.eye().xz();
    let mut n = NewCave {
        at,
        toward: cam,
        ..default()
    };
    if let Some(k) = enum_arg(
        args,
        "kind",
        CaveKind::from_id,
        "cave, mine, pit, passage, cavern, arch",
    )? {
        n.kind = k;
    }
    if let Some(s) = enum_arg(
        args,
        "size",
        CaveSize::from_id,
        "small, medium, large, huge",
    )? {
        n.size = s;
    }
    if let Some(l) = opt_arg::<u8>(args, "levels")? {
        n.levels = l.clamp(1, 4);
    }
    if let Some(g) = levels_go(args)? {
        n.up = g;
    }
    if let Some(t) = enum_arg(
        args,
        "theme",
        Theme::from_id,
        "auto, limestone, ice, mossy, sandstone, volcanic, crystal, mine",
    )? {
        n.theme = t;
    }
    if let Some(dk) = enum_arg(args, "darkness", Darkness::from_id, "dark, dim, readable")? {
        n.darkness = dk;
    }
    let mut base = Features::default();
    if n.theme == Theme::Crystal {
        base.crystals = true;
    }
    n.features = features_arg(args, base)?;
    n.seed = opt_arg(args, "seed")?;
    n.heading = opt_arg::<f32>(args, "heading_deg")?.map(heading_of);
    n.name = opt_arg(args, "name")?;
    n.dungeon = match args.get("dungeon") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Bool(true)) => Some(String::new()),
        _ => None,
    };
    if let Some(v) = args.get("span_to").and_then(|v| v.as_array()) {
        let f: Vec<f32> = v
            .iter()
            .filter_map(|x| x.as_f64())
            .map(|x| x as f32)
            .collect();
        if f.len() == 2 {
            n.span_to = Some(Vec2::new(f[0], f[1]));
        }
    }
    Ok(n)
}

fn mcp_create(
    In(args): In<Value>,
    mut d: WorldData,
    mut tool: ResMut<super::ui::CaveTool>,
) -> Result<Vec<McpContent>, String> {
    let n = new_from_args(&args, &d)?;
    let id = edit::create(&mut d, &n)?;
    // (the new cave is the one the Caves tab shows)
    tool.selected = Some(id);
    let c = d.caves.get(id).unwrap();
    Ok(vec![McpContent::Json(
        json!({ "cave": cave_json(c, true), "note": "one undo step; the rock builds in the background (caves_wait)" }),
    )])
}

fn mcp_pool_rim(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    let chamber: Option<u16> = opt_arg(&args, "chamber")?;
    let rim = enum_arg(
        &args,
        "rim",
        RimStyle::from_id,
        "auto, rimstone, scalloped, terraces, boulders, shelf, crust",
    )?
    .ok_or("give `rim`")?;
    let cave = d.caves.get(id).ok_or_else(|| format!("no cave {id}"))?;
    if !cave
        .chambers
        .iter()
        .any(|c| c.pool && chamber.is_none_or(|ch| ch == c.id))
    {
        return Err(match chamber {
            Some(ch) => format!("chamber {ch} of cave {id} has no pool"),
            None => format!("cave {id} has no pools (turn on features.pools with caves_set)"),
        });
    }
    edit::update(&mut d, id, "Change pool rim", |c| {
        let mut changed = false;
        for x in c
            .chambers
            .iter_mut()
            .filter(|x| x.pool && chamber.is_none_or(|ch| ch == x.id))
        {
            changed |= std::mem::replace(&mut x.rim, rim) != rim;
        }
        changed
    })?;
    let c = d.caves.get(id).unwrap();
    let pools: Vec<Value> = c
        .chambers
        .iter()
        .filter(|x| x.pool)
        .map(|x| json!({ "chamber": x.id, "rim": x.rim.id(), "rim_in_use": c.rim_of(x).id() }))
        .collect();
    Ok(vec![McpContent::Json(
        json!({ "pools": pools, "note": "one undo step; the rock rebuilds in the background (caves_wait)" }),
    )])
}

fn mcp_set(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    let kind = enum_arg(
        &args,
        "kind",
        CaveKind::from_id,
        "cave, mine, pit, passage, cavern, arch",
    )?;
    let size = enum_arg(
        &args,
        "size",
        CaveSize::from_id,
        "small, medium, large, huge, giant, colossal",
    )?;
    let up = levels_go(&args)?;
    let theme = enum_arg(
        &args,
        "theme",
        Theme::from_id,
        "auto, limestone, ice, mossy, sandstone, volcanic, crystal, mine",
    )?;
    let dark = enum_arg(&args, "darkness", Darkness::from_id, "dark, dim, readable")?;
    let levels: Option<u8> = opt_arg(&args, "levels")?;
    let name: Option<String> = opt_arg(&args, "name")?;
    let inst = match args.get("instancing").and_then(|v| v.as_str()) {
        None => None,
        Some(s) => Some(
            Instancing::ALL
                .into_iter()
                .find(|i| i.id() == s)
                .ok_or_else(|| format!("unknown instancing `{s}` (shared, party, raid)"))?,
        ),
    };
    let base = d
        .caves
        .get(id)
        .ok_or_else(|| format!("no cave {id}"))?
        .features;
    let feats = features_arg(&args, base)?;
    let dungeon = args.get("dungeon").cloned();
    edit::update(&mut d, id, "Change cave", |c| {
        let before = c.clone();
        if let Some(k) = kind
            && k != CaveKind::Arch
            && c.kind != CaveKind::Arch
        {
            c.kind = k;
        }
        if let Some(s) = size {
            c.size = s;
        }
        if let Some(t) = theme {
            c.theme = t;
        }
        if let Some(dk) = dark {
            c.darkness = dk;
        }
        if let Some(l) = levels {
            c.levels = l.clamp(1, 4);
        }
        if let Some(u) = up {
            c.up = u;
        }
        if let Some(n) = &name {
            c.name = n.clone();
        }
        c.features = feats;
        match &dungeon {
            Some(Value::Bool(false)) | Some(Value::Null) => c.dungeon = None,
            Some(Value::String(s)) => {
                let nm = if s.is_empty() {
                    format!("{} Depths", c.name)
                } else {
                    s.clone()
                };
                match &mut c.dungeon {
                    Some(dg) => dg.name = nm,
                    None => {
                        c.dungeon = Some(Dungeon {
                            name: nm,
                            chamber: 0,
                            door: [0.0; 3],
                            door_yaw: 0.0,
                            instancing: Instancing::Party,
                        })
                    }
                }
            }
            Some(Value::Bool(true)) if c.dungeon.is_none() => {
                c.dungeon = Some(Dungeon {
                    name: format!("{} Depths", c.name),
                    chamber: 0,
                    door: [0.0; 3],
                    door_yaw: 0.0,
                    instancing: Instancing::Party,
                })
            }
            _ => {}
        }
        if let (Some(i), Some(dg)) = (inst, &mut c.dungeon) {
            dg.instancing = i;
        }
        *c != before
    })?;
    Ok(vec![McpContent::Json(
        json!({ "cave": cave_json(d.caves.get(id).unwrap(), true) }),
    )])
}

fn mcp_shuffle(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    let seed = edit::shuffle(&mut d, id, opt_arg(&args, "seed")?)?;
    Ok(vec![McpContent::Json(
        json!({ "seed": seed, "cave": cave_json(d.caves.get(id).unwrap(), true) }),
    )])
}

fn mcp_extend(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    let from: Option<u16> = opt_arg(&args, "from")?;
    let heading = opt_arg::<f32>(&args, "heading_deg")?.map(heading_of);
    let down = args.get("down").and_then(|v| v.as_bool()).unwrap_or(false);
    let new = edit::extend(&mut d, id, from, heading, down)?;
    Ok(vec![McpContent::Json(
        json!({ "chamber": new, "cave": cave_json(d.caves.get(id).unwrap(), true) }),
    )])
}

fn mcp_lock(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    let ch: u16 = crate::mcp_tools::arg(&args, "chamber")?;
    let on = args.get("locked").and_then(|v| v.as_bool()).unwrap_or(true);
    edit::lock(&mut d, id, ch, on)?;
    Ok(vec![McpContent::Json(
        json!({ "id": id, "chamber": ch, "locked": on }),
    )])
}

fn mcp_remove(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    edit::remove(&mut d, id)?;
    Ok(vec![McpContent::Json(json!({ "removed": id }))])
}

fn mcp_dig(In(args): In<Value>, mut d: WorldData) -> Result<Vec<McpContent>, String> {
    let pts: Vec<[f32; 3]> = crate::mcp_tools::arg(&args, "points")?;
    let radius: f32 = opt_arg(&args, "radius")?.unwrap_or(2.5);
    let fill = args.get("fill").and_then(|v| v.as_bool()).unwrap_or(false);
    edit::dig(
        &mut d,
        pts.into_iter().map(Vec3::from_array).collect(),
        radius,
        fill,
    )?;
    Ok(vec![McpContent::Json(
        json!({ "strokes": d.caves.strokes.len() }),
    )])
}

fn mcp_view(
    In(args): In<Value>,
    mut d: WorldData,
    mut cw: ResMut<CaveWorld>,
) -> Result<Vec<McpContent>, String> {
    let id: u32 = crate::mcp_tools::arg(&args, "id")?;
    let c = d
        .caves
        .get(id)
        .ok_or_else(|| format!("no cave {id}"))?
        .clone();
    let outside = args
        .get("outside")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let chamber = if outside {
        super::ui::view_outside(&mut d.orbit, &c);
        None
    } else {
        let ch = match opt_arg::<u16>(&args, "chamber")? {
            Some(ch) => c.chamber(ch).map(|x| x.id).ok_or_else(|| {
                format!(
                    "cave {id} has no chamber {ch} (chambers: {:?})",
                    c.chambers.iter().map(|x| x.id).collect::<Vec<_>>()
                )
            })?,
            None => super::ui::first_chamber(&c)
                .map(|x| x.id)
                .ok_or("this cave has no chambers")?,
        };
        cw.xray = None;
        super::ui::view_chamber(&mut d.orbit, &cw, &c, ch);
        Some(ch)
    };
    if args
        .get("decorate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        d.ed.tab = crate::editor::Tab::Objects;
        d.ed.object_tool = crate::editor::ObjectTool::Place;
    }
    let f = d.orbit.focus;
    let mut out = json!({ "cave": id, "chamber": chamber, "focus": [r1(f.x), r1(f.y), r1(f.z)], "yaw_deg": r1(crate::mcp_tools::view_bearing(d.orbit.yaw)), "distance": r1(d.orbit.distance) });
    if cw.busy() {
        out["note"] = json!(
            "the rock is still being built (caves_wait): the view was set from the cave's layout"
        );
    }
    Ok(vec![McpContent::Json(out)])
}
