//! MCP tools for image to world: `reference_load`, `reference_analyse`, `reference_label`,
//! `reference_merge`, `reference_split`, `reference_delete`, `reference_build`,
//! `reference_overlay`. Positions are world `[x, z]` metres (x east, z south) unless a tool says
//! picture pixels.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::analyse::{self, Op, Role};
use super::build::{self, BuildRequest};
use super::paint;
use super::{Placement, RefMap, RefView, biome_id};
use crate::biomes::BiomePreset;
use crate::history::History;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};

type R = Result<Vec<McpContent>, String>;

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "reference_load",
        description: "Image to world, step 1: load a map picture (PNG or JPG: concept art, a sketch, a real map; north up) as the world's reference \
map, laid over the terrain like tracing paper (Land tab › Trace a Picture). By default it is fitted to the world keeping its aspect ratio \
(`margin`: share of the world kept as open sea on each side, default 0.03); or give `placement`. Returns the picture size and its \
placement (world corners, metres per pixel). Next: reference_analyse, then reference_build (≈4 calls for a whole draft). Saved with the world.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "path": {"type": "string", "description": "absolute path of a .png / .jpg"},
                "fit": {"type": "boolean", "default": true, "description": "fit to the open world"},
                "margin": {"type": "number", "minimum": 0, "maximum": 0.4, "description": "sea kept clear on each side when fitting (share of the world size)"},
                "placement": placement_schema()
            }, "required": ["path"]})
        },
        run: load,
    })
    .mcp_tool(McpTool {
        name: "reference_analyse",
        description: "Image to world, step 2: read the reference picture. Finds the sea (water reaching the border, clouds over it included), lakes, rivers \
(centrelines with widths) and land regions by colour (target `regions`, default automatic ≈14 for a busy fantasy map), each with a suggested \
biome and height role (peaks, mountains, hills, plains, lowland, wetland). Legend panels are found by themselves; lettering, compass roses \
and scale bars are not land. Mark anything else that isn't map with `ignore` (polygons in world [x, z] m, replacing earlier ones) or \
`ignore_px` (polygons in picture pixels). Returns the regions (id, name, colour, area, centroid, biome, role), a water summary and a \
labelled preview PNG. Ids are by size and change when the regions change: re-read them after edits.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "regions": {"type": "integer", "minimum": 0, "maximum": 40, "description": "target number of land regions (0: automatic)"},
                "sensitivity": {"type": "number", "minimum": 0, "maximum": 1, "default": 0.5, "description": "higher keeps smaller islets, rivers and regions"},
                "auto_ignore": {"type": "boolean", "default": true, "description": "ignore framed legend panels"},
                "ignore": {"type": "array", "items": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}}, "description": "areas that aren't map: polygons of world [x, z] points (replaces the current list)"},
                "ignore_px": {"type": "array", "items": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}}, "description": "the same in picture pixels [px, py] from the top-left"},
                "clear_edits": {"type": "boolean", "default": false, "description": "drop all region edits (labels, merges, splits, deletions)"},
                "preview_px": {"type": "integer", "minimum": 256, "maximum": 2048, "default": 1024, "description": "preview size; 0 for none"}
            }})
        },
        run: analyse_tool,
    })
    .mcp_tool(McpTool {
        name: "reference_label",
        description: "Set a region's name, biome and/or height role (one region, or several with `labels`: [{id, name?, biome?, height_role?}, …]). \
Biomes: temperate_forest, meadow, taiga, autumn_woods, jungle, tropical_coast, desert, alpine, tundra, swamp, volcanic, haunted_woods. \
Height roles: peaks, mountains, hills, plains, lowland, wetland. Edits are kept by position and survive re-analysis; undoable. Returns the regions.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "id": {"type": "integer"},
                "name": {"type": "string"},
                "biome": biome_schema(),
                "height_role": role_schema(),
                "labels": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "integer"}, "name": {"type": "string"}, "biome": biome_schema(), "height_role": role_schema()}, "required": ["id"]}}
            }})
        },
        run: label,
    })
    .mcp_tool(McpTool {
        name: "reference_merge",
        description: "Merge regions: the others join the first (`ids`: [keep, other, …]); the merged region keeps the first one's name, biome and role. Undoable. Returns the regions.",
        input_schema: || json!({"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "integer"}, "minItems": 2}}, "required": ["ids"]}),
        run: merge,
    })
    .mcp_tool(McpTool {
        name: "reference_split",
        description: "Cut a region in two along a line drawn across it (`line`: world [x, z] points, or `line_px`: picture pixels); the region the line crosses most is split by which side of the line its parts lie. Undoable. Returns the regions.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "line": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "minItems": 2},
                "line_px": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "minItems": 2}
            }})
        },
        run: split,
    })
    .mcp_tool(McpTool {
        name: "reference_delete",
        description: "Delete regions (`ids`): each becomes what surrounds it (an islet the sea, an inland patch its neighbours), e.g. a misread legend swatch or label. Undoable. Returns the regions.",
        input_schema: || json!({"type": "object", "properties": {"ids": {"type": "array", "items": {"type": "integer"}, "minItems": 1}}, "required": ["ids"]}),
        run: delete,
    })
    .mcp_tool(McpTool {
        name: "reference_build",
        description: "Image to world, step 3: build the draft world from the analysed picture as ONE undo step: coastline and islands, heights (land rising \
from the coast; ridged ranges along each massif for peaks / mountains, rolling hills, gentle plains, wetlands just above the water; the picture's \
shading as relief; erosion), each region's biome (ground, vegetation, water styles) with organic borders, lakes and downhill rivers, and \
zones only with `zones: true`. `world_size`: \"current\" (default; uses the current placement) or a new world (\"small\" 2 km, \"medium\" 4 km, \
\"large\" 8 km, or metres) with the picture refitted (can't be undone past the new world). Returns a summary and a screenshot.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "world_size": {"oneOf": [{"type": "string", "enum": ["current", "small", "medium", "large"]}, {"type": "number", "minimum": 1024, "maximum": 16384}], "description": "the open world, a new small (2 km), medium (4 km) or large (8 km) world, or a new world of this many metres"},
                "margin": {"type": "number", "minimum": 0, "maximum": 0.4, "description": "new world: sea kept clear around the picture"},
                "peak_height": {"type": "number", "minimum": 0, "maximum": 2000, "description": "height of the highest peaks above the sea (m; 0 = from the world size)"},
                "erosion": {"type": "number", "minimum": 0, "maximum": 1, "default": 0.6},
                "shading": {"type": "boolean", "default": true, "description": "use the picture's hill shading as extra relief"},
                "roughness": {"type": "number", "minimum": 0, "maximum": 1, "default": 0.5, "description": "coastline raggedness"},
                "vegetation": {"type": "boolean", "default": true},
                "zones": {"type": "boolean", "default": false, "description": "also create a zone per region"},
                "seed": {"type": "integer"},
                "screenshot": {"type": "boolean", "default": true}
            }})
        },
        run: build_tool,
    })
    .mcp_tool(McpTool {
        name: "reference_overlay",
        description: "Show or hide the reference picture on the terrain and set its opacity, what it shows (\"picture\" or \"analysis\": regions, water, rivers) and its placement (`placement`, or `fit` to the world). Placement changes are undoable.",
        input_schema: || {
            json!({"type": "object", "properties": {
                "visible": {"type": "boolean"},
                "opacity": {"type": "number", "minimum": 0, "maximum": 1},
                "view": {"type": "string", "enum": ["picture", "analysis"]},
                "fit": {"type": "boolean", "description": "fit the picture to the world (with `margin`)"},
                "margin": {"type": "number", "minimum": 0, "maximum": 0.4},
                "placement": placement_schema()
            }})
        },
        run: overlay_tool,
    });
}

fn biome_schema() -> Value {
    json!({"type": "string", "enum": BiomePreset::ALL.iter().map(|b| biome_id(&format!("{b:?}"))).collect::<Vec<_>>()})
}

fn role_schema() -> Value {
    json!({"type": "string", "enum": Role::ALL.iter().map(|r| r.id()).collect::<Vec<_>>()})
}

fn placement_schema() -> Value {
    json!({"type": "object", "description": "where the picture lies", "properties": {
        "center": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "world [x, z] of the picture's centre (m)"},
        "width": {"type": "number", "description": "width of the picture on the ground (m); the height follows the aspect ratio"},
        "rotation_deg": {"type": "number", "description": "where the picture's top points, compass degrees clockwise from above (0: north)"}
    }})
}

fn point(v: &Value) -> Result<[f32; 2], String> {
    let a = v
        .as_array()
        .filter(|a| a.len() >= 2)
        .ok_or("a point must be [x, z]")?;
    let f = |v: &Value| {
        v.as_f64()
            .map(|f| f as f32)
            .filter(|f| f.is_finite())
            .ok_or("a point must be numbers")
    };
    Ok([f(&a[0])?, f(&a[a.len() - 1])?])
}

fn polygons(v: &Value, key: &str) -> Result<Option<Vec<Vec<[f32; 2]>>>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(list)) => list
            .iter()
            .map(|p| {
                p.as_array()
                    .ok_or(format!("`{key}`: each area is a list of points"))
                    .and_then(|pts| pts.iter().map(point).collect::<Result<Vec<_>, _>>())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("`{key}` must be a list of polygons")),
    }
}

fn line(v: &Value, key: &str) -> Result<Option<Vec<[f32; 2]>>, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(pts)) => pts
            .iter()
            .map(point)
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(format!("`{key}` must be a list of points")),
    }
}

fn apply_placement(p: &mut Placement, v: &Value) -> Result<(), String> {
    if let Some(c) = v.get("center").filter(|c| !c.is_null()) {
        p.center = point(c)?;
    }
    if let Some(w) = opt_arg::<f32>(v, "width")? {
        if !(10.0..=1.0e6).contains(&w) {
            return Err("`width` must be 10 m or more".into());
        }
        p.width = w;
    }
    if let Some(r) = opt_arg::<f32>(v, "rotation_deg")? {
        p.rotation = r.rem_euclid(360.0);
    }
    Ok(())
}

fn placement_json(r: &RefMap) -> Value {
    let p = &r.doc.placement;
    let a = r.aspect();
    let px = r.picture.as_ref().map(|q| [q.w, q.h]).unwrap_or([0, 0]);
    let round = |v: f32| (v * 10.0).round() / 10.0;
    json!({
        "center": [round(p.center[0]), round(p.center[1])],
        "width_m": round(p.width),
        "height_m": round(p.width * a),
        "rotation_deg": round(p.rotation),
        "m_per_px": if px[0] > 0 { round(p.width / px[0] as f32 * 100.0) / 100.0 } else { 0.0 },
        "corners": p.corners(a).iter().map(|c| [round(c[0]), round(c[1])]).collect::<Vec<_>>(),
        "corner_order": "top-left, top-right, bottom-right, bottom-left of the picture"
    })
}

fn world_size(world: &World) -> f32 {
    world.resource::<crate::terrain::Terrain>().size()
}

fn load(world: &mut World, args: Value) -> R {
    let path: String = crate::mcp_tools::arg(&args, "path")?;
    let p = std::path::PathBuf::from(&path);
    if !p.is_absolute() {
        return Err("`path` must be an absolute path".into());
    }
    let size = world_size(world);
    let margin = opt_arg::<f32>(&args, "margin")?;
    world.resource_scope(|world, mut r: Mut<RefMap>| -> Result<(), String> {
        let before = r.snapshot();
        let old_placement = r.doc.placement;
        if let Some(m) = margin {
            r.doc.build.margin = m.clamp(0.0, 0.4);
        }
        r.load(&p, size)?;
        if let Some(pl) = args.get("placement").filter(|v| v.is_object()) {
            apply_placement(&mut r.doc.placement, pl)?;
        } else if opt_arg::<bool>(&args, "fit")? == Some(false) {
            r.doc.placement.center = old_placement.center;
            r.doc.placement.width = old_placement.width;
        }
        r.record(
            &mut world.resource_mut::<History>(),
            "Load reference picture",
            before,
        );
        Ok(())
    })?;
    let mut ed = world.resource_mut::<crate::editor::Editor>();
    ed.tab = crate::editor::Tab::Land;
    ed.land_tool = crate::editor::LandTool::Reference;
    let r = world.resource::<RefMap>();
    let pic = r.picture.as_ref().ok_or("no picture")?;
    Ok(vec![McpContent::Json(json!({
        "picture": { "path": path, "width_px": pic.w, "height_px": pic.h },
        "world_size_m": size,
        "placement": placement_json(r),
        "next": "reference_analyse (regions, water, preview), then reference_build",
    }))])
}

/// The regions, water and placement as JSON.
fn summary(r: &RefMap, an: &analyse::Analysis) -> Value {
    let aspect = r.aspect();
    let place = r.doc.placement;
    let cell_km2 = (place.width / an.w as f32).powi(2) / 1.0e6;
    let to_w = |uv: [f32; 2]| {
        let p = place.to_world(uv, aspect);
        [p[0].round(), p[1].round()]
    };
    let regions: Vec<Value> = an
        .regions
        .iter()
        .enumerate()
        .map(|(i, rg)| {
            let c = paint::region_color(i);
            let mut v = json!({
                "id": rg.id,
                "name": rg.name,
                "biome": biome_id(&rg.biome),
                "height_role": rg.role.id(),
                "overlay_color": format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]),
                "picture_color": format!("#{:02x}{:02x}{:02x}", rg.color[0], rg.color[1], rg.color[2]),
                "area_pct_of_land": (rg.frac * 1000.0).round() / 10.0,
                "area_km2": (rg.cells as f32 * cell_km2 * 100.0).round() / 100.0,
                "centroid": to_w(rg.centroid),
                "label_point": to_w(rg.pole),
            });
            if rg.named {
                v["edited"] = json!(true);
            }
            if rg.biome != rg.suggested_biome || rg.role != rg.suggested_role {
                v["suggested"] = json!({ "biome": biome_id(&rg.suggested_biome), "height_role": rg.suggested_role.id() });
            }
            v
        })
        .collect();
    let n = an.class.len().max(1) as f32;
    let count = |c: u8| an.class.iter().filter(|x| **x == c).count() as f32 / n * 100.0;
    let river_km: f32 = an
        .rivers
        .iter()
        .map(|rv| rv.length() * place.width / an.w as f32)
        .sum::<f32>()
        / 1000.0;
    let mut v = json!({
        "regions": regions,
        "water": {
            "sea_pct": (count(analyse::SEA) * 10.0).round() / 10.0,
            "land_pct": (count(analyse::LAND) * 10.0).round() / 10.0,
            "lakes": an.lakes.len(),
            "lake_areas_m2": an.lakes.iter().map(|l| (l.cells as f32 * cell_km2 * 1.0e6 / 100.0).round() * 100.0).collect::<Vec<_>>(),
            "rivers": an.rivers.len(),
            "river_length_km": (river_km * 10.0).round() / 10.0,
        },
        "ignore_areas": r.doc.ignore.len(),
        "auto_ignored_panels": r.base.as_ref().map(|b| b.panels.iter().map(|p| [to_w([p[0], p[1]]), to_w([p[2], p[3]])]).collect::<Vec<_>>()).unwrap_or_default(),
        "edits": r.doc.ops.len(),
        "analyse_ms": r.base.as_ref().map(|b| b.millis.round()),
        "placement": placement_json(r),
    });
    if an.stale_ops > 0 {
        v["stale_edits"] = json!(an.stale_ops);
    }
    v
}

fn analyse_tool(world: &mut World, args: Value) -> R {
    let regions = opt_arg::<u32>(&args, "regions")?;
    let sens = opt_arg::<f32>(&args, "sensitivity")?;
    let auto = opt_arg::<bool>(&args, "auto_ignore")?;
    let clear = opt_arg::<bool>(&args, "clear_edits")?.unwrap_or(false);
    let ignore_w = polygons(&args, "ignore")?;
    let ignore_px = polygons(&args, "ignore_px")?;
    let preview = opt_arg::<u32>(&args, "preview_px")?.unwrap_or(1024);
    world.resource_scope(|world, mut r: Mut<RefMap>| -> R {
        let Some(pic) = r.picture.clone() else {
            return Err("no reference picture: load one first (reference_load)".into());
        };
        let (aspect, place) = (r.aspect(), r.doc.placement);
        let mut ignore: Option<Vec<Vec<[f32; 2]>>> = None;
        if let Some(polys) = ignore_w {
            ignore = Some(
                polys
                    .iter()
                    .map(|p| p.iter().map(|q| place.to_uv(*q, aspect)).collect())
                    .collect(),
            );
        }
        if let Some(polys) = ignore_px {
            let mut list = ignore.take().unwrap_or_default();
            list.extend(polys.iter().map(|p| {
                p.iter()
                    .map(|q| [q[0] / pic.w as f32, q[1] / pic.h as f32])
                    .collect::<Vec<_>>()
            }));
            ignore = Some(list);
        }
        if ignore
            .as_ref()
            .is_some_and(|l| l.iter().any(|p| p.len() < 3))
        {
            return Err("an ignore area needs at least 3 points".into());
        }
        let mut history = world.resource_mut::<History>();
        r.edit(&mut history, "Reference analysis", |d| {
            if let Some(n) = regions {
                d.settings.regions = n.min(40);
            }
            if let Some(s) = sens {
                d.settings.sensitivity = s.clamp(0.0, 1.0);
            }
            if let Some(a) = auto {
                d.settings.auto_ignore = a;
            }
            if let Some(i) = ignore.clone() {
                d.ignore = i;
            }
            if clear {
                d.ops.clear();
            }
            true
        });
        let an = r.analyse_now()?;
        let mut out = vec![McpContent::Json(summary(&r, &an))];
        if preview > 0 {
            let ignored = r
                .base
                .as_ref()
                .map(|b| b.ignored.clone())
                .unwrap_or_default();
            out.push(McpContent::Png(paint::preview_png(
                &an,
                &pic,
                &ignored,
                preview.clamp(256, 2048),
            )?));
        }
        Ok(out)
    })
}

/// Runs `f` with the current analysis and the region index for `id`; records the op it returns.
fn region_edit(
    world: &mut World,
    label: &str,
    edits: impl FnOnce(
        &analyse::Analysis,
        &dyn Fn(u32) -> Result<usize, String>,
    ) -> Result<Vec<Op>, String>,
) -> R {
    world.resource_scope(|world, mut r: Mut<RefMap>| -> R {
        let an = r.analyse_now()?;
        let find = |id: u32| -> Result<usize, String> {
            an.regions.iter().position(|g| g.id == id).ok_or(format!(
                "no region {id} (ids 1..={}; re-read them with reference_analyse)",
                an.regions.len()
            ))
        };
        let ops = edits(&an, &find)?;
        let mut history = world.resource_mut::<History>();
        r.edit(&mut history, label, |d| {
            d.ops.extend(ops);
            true
        });
        let an = r.analyse_now()?;
        Ok(vec![McpContent::Json(summary(&r, &an))])
    })
}

fn label(world: &mut World, args: Value) -> R {
    let mut list: Vec<Value> = match args.get("labels") {
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };
    if args.get("id").is_some() {
        list.push(args.clone());
    }
    if list.is_empty() {
        return Err("give `id` (with name / biome / height_role) or `labels`".into());
    }
    region_edit(world, "Label regions", |an, find| {
        let mut ops = vec![];
        for v in &list {
            let id: u32 = crate::mcp_tools::arg(v, "id")?;
            let i = find(id)?;
            let biome = match opt_arg::<String>(v, "biome")? {
                Some(b) => {
                    let norm = |s: &str| {
                        s.chars()
                            .filter(|c| c.is_alphanumeric())
                            .flat_map(|c| c.to_lowercase())
                            .collect::<String>()
                    };
                    let found = BiomePreset::ALL.into_iter().find(|p| {
                        norm(&format!("{p:?}")) == norm(&b)
                            || norm(p.def().name) == norm(&b)
                            || norm(p.short()) == norm(&b)
                    });
                    Some(format!(
                        "{:?}",
                        found.ok_or(format!(
                            "unknown biome `{b}`; one of: {}",
                            BiomePreset::ALL
                                .iter()
                                .map(|p| biome_id(&format!("{p:?}")))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))?
                    ))
                }
                None => None,
            };
            let role = match opt_arg::<String>(v, "height_role")? {
                Some(s) => Some(Role::from_id(&s).ok_or(format!(
                    "unknown height role `{s}` (peaks, mountains, hills, plains, lowland, wetland)"
                ))?),
                None => None,
            };
            let name = opt_arg::<String>(v, "name")?
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty());
            ops.push(Op::Label {
                at: an.regions[i].pole,
                name,
                biome,
                role,
            });
        }
        Ok(ops)
    })
}

fn merge(world: &mut World, args: Value) -> R {
    let ids: Vec<u32> = crate::mcp_tools::arg(&args, "ids")?;
    if ids.len() < 2 {
        return Err("`ids` needs at least two regions".into());
    }
    region_edit(world, "Merge regions", |an, find| {
        let keep = an.regions[find(ids[0])?].pole;
        ids[1..]
            .iter()
            .map(|id| {
                Ok(Op::Merge {
                    a: keep,
                    b: an.regions[find(*id)?].pole,
                })
            })
            .collect()
    })
}

fn delete(world: &mut World, args: Value) -> R {
    let ids: Vec<u32> = crate::mcp_tools::arg(&args, "ids")?;
    region_edit(world, "Delete regions", |an, find| {
        ids.iter()
            .map(|id| {
                Ok(Op::Delete {
                    at: an.regions[find(*id)?].pole,
                })
            })
            .collect()
    })
}

fn split(world: &mut World, args: Value) -> R {
    let (aspect, place, px) = {
        let r = world.resource::<RefMap>();
        (
            r.aspect(),
            r.doc.placement,
            r.picture.as_ref().map(|p| (p.w, p.h)),
        )
    };
    let (w, h) = px.ok_or("no reference picture: load one first (reference_load)")?;
    let uv: Vec<[f32; 2]> = if let Some(l) = line(&args, "line")? {
        l.iter().map(|q| place.to_uv(*q, aspect)).collect()
    } else if let Some(l) = line(&args, "line_px")? {
        l.iter()
            .map(|q| [q[0] / w as f32, q[1] / h as f32])
            .collect()
    } else {
        return Err("give `line` (world [x, z] points) or `line_px` (picture pixels)".into());
    };
    if uv.len() < 2 {
        return Err("a split line needs at least 2 points".into());
    }
    region_edit(world, "Split region", |_, _| {
        Ok(vec![Op::Split { line: uv }])
    })
}

fn build_tool(world: &mut World, args: Value) -> R {
    let mut settings = world.resource::<RefMap>().doc.build.clone();
    if let Some(m) = opt_arg::<f32>(&args, "margin")? {
        settings.margin = m.clamp(0.0, 0.4);
    }
    if let Some(v) = opt_arg::<f32>(&args, "peak_height")? {
        settings.relief.peak_height = v.clamp(0.0, 2000.0);
    }
    if let Some(v) = opt_arg::<f32>(&args, "erosion")? {
        settings.relief.erosion = v.clamp(0.0, 1.0);
    }
    if let Some(v) = opt_arg::<bool>(&args, "shading")? {
        settings.relief.shading = v;
    }
    if let Some(v) = opt_arg::<f32>(&args, "roughness")? {
        settings.relief.roughness = v.clamp(0.0, 1.0);
    }
    if let Some(v) = opt_arg::<bool>(&args, "vegetation")? {
        settings.vegetation = v;
    }
    settings.zones = opt_arg::<bool>(&args, "zones")?.unwrap_or(false);
    let new_size =
        match args.get("world_size") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => match s.to_lowercase().as_str() {
                "current" | "" => None,
                "small" => Some(2048.0),
                "medium" => Some(4096.0),
                "large" => Some(8192.0),
                _ => return Err(
                    "`world_size` must be \"current\", \"small\", \"medium\", \"large\" or metres"
                        .into(),
                ),
            },
            Some(v) => {
                let m = v.as_f64().ok_or(
                    "`world_size` must be \"current\", \"small\", \"medium\", \"large\" or metres",
                )? as f32;
                if !(1024.0..=16384.0).contains(&m) {
                    return Err("`world_size` in metres must be 1024–16384".into());
                }
                Some(m)
            }
        };
    let seed = opt_arg::<u32>(&args, "seed")?
        .unwrap_or_else(|| world.resource::<crate::world::WorldMeta>().seed);
    settings.world_size = new_size.unwrap_or(0.0);
    world.resource_mut::<RefMap>().doc.build = settings.clone();
    let mut summary = build::build(
        world,
        BuildRequest {
            new_size,
            settings,
            seed,
            art_style: None,
            appearance: None,
        },
    )?;
    // the picture's vegetation density, for the procedural nature's density maps
    if let Some(d) = super::image_density(world) {
        let land: Vec<f32> = d.values.iter().copied().filter(|v| *v > 0.0).collect();
        summary["picture_vegetation_density"] = json!({ "grid": [d.w, d.h], "mean_on_land": (land.iter().sum::<f32>() / land.len().max(1) as f32 * 100.0).round() / 100.0 });
    }
    let mut out = vec![McpContent::Json(summary)];
    if opt_arg::<bool>(&args, "screenshot")?.unwrap_or(true) {
        let shot = world
            .resource::<crate::mcp_tools::McpTools>()
            .tools
            .iter()
            .find(|t| t.name == "screenshot")
            .cloned();
        if let Some(t) = shot {
            out.extend((t.run)(
                world,
                json!({ "max_size": 1024, "settle_frames": 90 }),
            )?);
        }
    }
    Ok(out)
}

fn overlay_tool(world: &mut World, args: Value) -> R {
    let size = world_size(world);
    world.resource_scope(|world, mut r: Mut<RefMap>| -> R {
        if !r.has_picture() {
            return Err("no reference picture: load one first (reference_load)".into());
        }
        if let Some(v) = opt_arg::<bool>(&args, "visible")? {
            r.doc.visible = v;
        }
        if let Some(o) = opt_arg::<f32>(&args, "opacity")? {
            r.doc.opacity = o.clamp(0.0, 1.0);
        }
        if let Some(v) = opt_arg::<String>(&args, "view")? {
            r.doc.view = match v.as_str() {
                "picture" => RefView::Picture,
                "analysis" => RefView::Analysis,
                _ => return Err("`view` must be picture or analysis".into()),
            };
        }
        let margin = opt_arg::<f32>(&args, "margin")?;
        let fit = opt_arg::<bool>(&args, "fit")?.unwrap_or(false);
        let pl = args.get("placement").filter(|v| v.is_object()).cloned();
        if fit || pl.is_some() {
            let aspect = r.aspect();
            let mut history = world.resource_mut::<History>();
            let mut err = None;
            r.edit(&mut history, "Place reference picture", |d| {
                if let Some(m) = margin {
                    d.build.margin = m.clamp(0.0, 0.4);
                }
                if fit {
                    d.placement = Placement::fit(size, aspect, d.build.margin);
                }
                if let Some(pl) = &pl
                    && let Err(e) = apply_placement(&mut d.placement, pl)
                {
                    err = Some(e);
                }
                true
            });
            if let Some(e) = err {
                return Err(e);
            }
        }
        Ok(vec![McpContent::Json(json!({
            "visible": r.doc.visible,
            "opacity": r.doc.opacity,
            "view": if r.doc.view == RefView::Picture { "picture" } else { "analysis" },
            "placement": placement_json(&r),
            "hint": "the overlay shows on the Land, Sculpt, Paint, Water and Zones tabs",
        }))])
    })
}
