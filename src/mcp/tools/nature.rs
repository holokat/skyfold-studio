//! Nature tools. Trees, rocks, plants and debris are procedural (`crate::nature`): rules plus
//! painted density, placed deterministically. Agents paint density in an area (grow / thin /
//! remove an ecosystem, a category or species), set the world dials, query what grows where
//! (`nature_query`), see the rules (`nature_rules`), and pin or remove single instances.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpTool};
use crate::nature::field::Target;
use crate::nature::place::{Instance, is_procedural};
use crate::nature::rules::{CATS, cat_index};
use crate::nature::{Masks, Rules};
use crate::objects::{Category, kind_category, presets};
use crate::procgen::meshes::AssetKind;
use crate::ui::UiAction;

fn preset_ids() -> Vec<String> {
    presets()
        .iter()
        .map(|p| snake(&p.name.replace([' ', '-'], "")))
        .collect()
}

const NATURE_CATS: [Category; 4] = CATS;

pub fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "nature_scatter",
        description: "Paint procedural nature in an `area` (like the Nature brush; nature is density, not objects). Mode \"add\" grows a biome `preset` (a whole ecosystem: mixed_forest, pine_forest, flower_meadow, palm_beach, desert, swamp, rocky_hills, alpine…), \
more of some `assets` (species ids from objects_library, categories trees/rocks/plants/debris) or of a whole `category`, at `density` (1 = natural). \"thin\" keeps `1 − amount` of what grows there (`amount` default 0.5); \"remove\" clears it. \
Without preset, assets or category, thin / remove act on all nature. Placement follows the land (slope, altitude, water, roads, placed objects, zone rules). One undo step; reports what grows in the area before and after.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "area": area_schema("Where."),
                    "mode": { "type": "string", "enum": ["add", "thin", "remove"], "default": "add" },
                    "preset": enum_schema(preset_ids(), "ecosystem preset"),
                    "assets": { "type": "array", "items": { "type": "string" }, "description": "nature species (library asset ids) instead of a preset" },
                    "category": { "type": "string", "enum": ["trees", "rocks", "plants", "debris"], "description": "a whole category instead of a preset or species" },
                    "density": { "type": "number", "minimum": 0.05, "maximum": 3.9, "default": 1, "description": "add: how dense (1 = natural)" },
                    "amount": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.5, "description": "thin: share to take away" }
                },
                "required": ["area"]
            })
        },
        run: |w, a| run(w, scatter, a),
    });
    app.mcp_tool(McpTool {
        name: "nature_auto",
        description: "Whole-world nature (Nature tab buttons): \"populate\" makes nature grow everywhere it suits the land and its biomes (forests, meadows, alpine trees, palms on beaches, rocks on slopes; also on land without a biome), resetting painted edits; \
\"clear\" removes placed nature and turns every procedural nature dial to 0. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "action": { "type": "string", "enum": ["populate", "clear"] } }, "required": ["action"] }),
        run: |w, a| {
            let action = match opt_str(&a, "action")? {
                Some("populate") => UiAction::PopulateWorld,
                Some("clear") => UiAction::ClearNature,
                _ => return Err("`action` must be populate or clear".into()),
            };
            super::world::ui_action(w, action, |_| "Done (one undo step).".into())
        },
    });
    app.mcp_tool(McpTool {
        name: "nature_set_amounts",
        description: "World-wide nature dials, instant and saved with the world: `grass` (meadow grass thickness), `grass_height` (blade length ×0.3..2.5, 1 = as grown), `trees`, `rocks`, `plants` (ferns, bushes, reeds), `flowers`, `mushrooms`, `berries`, and `debris` (logs, stumps, branches). Each is 0..2 (`trees` 0..5); 1 follows the rules.",
        input_schema: || {
            json!({ "type": "object", "properties": {
                "grass": { "type": "number", "minimum": 0, "maximum": 2 },
                "grass_height": { "type": "number", "minimum": 0.3, "maximum": 2.5 },
                "trees": { "type": "number", "minimum": 0, "maximum": 5 },
                "rocks": { "type": "number", "minimum": 0, "maximum": 2 },
                "plants": { "type": "number", "minimum": 0, "maximum": 2 },
                "flowers": { "type": "number", "minimum": 0, "maximum": 2 },
                "mushrooms": { "type": "number", "minimum": 0, "maximum": 2 },
                "berries": { "type": "number", "minimum": 0, "maximum": 2 },
                "debris": { "type": "number", "minimum": 0, "maximum": 2 }
            } })
        },
        run: |w, a| run(w, amounts, a),
    });
    app.mcp_tool(McpTool {
        name: "nature_appearance",
        description: "Read or update the saved global nature appearance. Forest diversity changes only automatic rule-grown forests; explicit painted and pinned trees retain their species. `enabled: false` exactly keeps the legacy tree selection. `layout` is natural_groves or mixed. `grass_lushness` is 0..1 and adds lushness independently from the grass amount dial. `canopy_shade` is 0..1 (default 1): how much sky light tree crowns hold back from the forest floor and the insides of crowns (0 lights them like open ground).",
        input_schema: || json!({ "type": "object", "properties": {
            "enabled": { "type": "boolean", "description": "forest diversity on/off" },
            "layout": { "type": "string", "enum": ["natural_groves", "mixed"] },
            "grass_lushness": { "type": "number", "minimum": 0, "maximum": 1 },
            "canopy_shade": { "type": "number", "minimum": 0, "maximum": 1 }
        }}),
        run: |w, a| run(w, appearance, a),
    });
    app.mcp_tool(McpTool {
        name: "ground_cover",
        description: "Read or set the ground cover, saved with the world: the dense knee-high layer round the viewer (realistic art style). \
`undergrowth` 0..2 (1 = a full woodland floor): ferns and broad-leaved herbs (photoscanned weed rosettes, nettles, leafy stalks) growing in clumps under tree crowns; \
`herbs` 0..1: the undergrowth's mix, 0 all ferns .. 1 all broad-leaved herbs, in single-species patches; \
`tufts` 0..2: photoscanned grass tufts among the meadow blades in the open; \
`grass_under_trees` 0..1: how much meadow grass still grows under the crowns (the undergrowth takes over); \
`clumping` 0..1: 0 an even spread .. 1 dense patches with bare ground between. One undo step. With no arguments, reports the settings and how many plants of each kind grow near the viewer.",
        input_schema: || json!({ "type": "object", "properties": {
            "undergrowth": { "type": "number", "minimum": 0, "maximum": 2 },
            "herbs": { "type": "number", "minimum": 0, "maximum": 1 },
            "tufts": { "type": "number", "minimum": 0, "maximum": 2 },
            "grass_under_trees": { "type": "number", "minimum": 0, "maximum": 1 },
            "clumping": { "type": "number", "minimum": 0, "maximum": 1 }
        }}),
        run: |w, a| run(w, ground_cover, a),
    });
    app.mcp_tool(McpTool {
        name: "nature_query",
        description: "What grows in an `area` (default: around the world centre, 500 m): procedural trees, rocks, plants and debris plus placed nature objects, optionally only a `category` or some `assets`. \
Returns the total, counts per asset and up to `limit` instances nearest the area's centre (id, asset, position [x, y, z], yaw_deg (clockwise turn), scale, procedural). Procedural ids are strings (\"n…\"): pin or remove them with nature_pin / nature_remove.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "area": area_schema("Search area."),
                    "category": { "type": "string", "enum": ["trees", "rocks", "plants", "debris"] },
                    "assets": { "type": "array", "items": { "type": "string" } },
                    "limit": { "type": "integer", "minimum": 0, "maximum": 2000, "default": 50 }
                }
            })
        },
        run: |w, a| run(w, query, a),
    });
    app.mcp_tool(McpTool {
        name: "nature_rules",
        description: "The rules procedural nature grows by: each ecosystem preset's layers per category (asset, density per 1000 m², scale range, max slope, altitude range) and each biome's presets (densest first). Painted density and the world dials scale them.",
        input_schema: || json!({ "type": "object", "properties": {} }),
        run: |w, a| run(w, rules_info, a),
    });
    app.mcp_tool(McpTool {
        name: "nature_pin",
        description: "Turn procedural instances into placed objects (to move, rotate or keep them whatever the density does): `ids` from nature_query, or every instance of some `assets` in an `area` (at most 500). Returns the new object ids. One undo step.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "string" }, "description": "procedural ids (\"n…\") from nature_query" },
                    "area": area_schema("Pin everything of `assets` in here."),
                    "assets": { "type": "array", "items": { "type": "string" } }
                }
            })
        },
        run: |w, a| run(w, pin, a),
    });
    app.mcp_tool(McpTool {
        name: "nature_remove",
        description: "Remove single procedural instances by hand (`ids` from nature_query); they stay gone whatever the density. To clear an area, use nature_scatter in its remove mode. One undo step.",
        input_schema: || json!({ "type": "object", "properties": { "ids": { "type": "array", "items": { "type": "string" } } }, "required": ["ids"] }),
        run: |w, a| run(w, remove, a),
    });
}

/// Procedural ids are strings: they don't fit a JSON number.
fn id_str(id: u64) -> String {
    format!("n{id:x}")
}

fn parse_id(s: &str) -> Result<u64, String> {
    let id = s
        .strip_prefix('n')
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .ok_or_else(|| format!("`{s}` is not a procedural id (\"n…\" from nature_query)"))?;
    if !is_procedural(id) {
        return Err(format!("`{s}` is not a procedural id"));
    }
    Ok(id)
}

fn cat_arg(args: &Value) -> Result<Option<usize>, String> {
    Ok(match opt_str(args, "category")? {
        Some(c) => Some(
            NATURE_CATS
                .iter()
                .position(|x| x.name().eq_ignore_ascii_case(c))
                .ok_or_else(|| format!("unknown category `{c}`"))?,
        ),
        None => None,
    })
}

fn kinds_arg(args: &Value, lib: &crate::objects::Library) -> Result<Vec<AssetKind>, String> {
    let assets: Vec<String> = crate::mcp_tools::opt_arg(args, "assets")?.unwrap_or_default();
    assets
        .iter()
        .map(|a| {
            let e = lib
                .get(a)
                .ok_or_else(|| format!("unknown asset `{a}` (see objects_library)"))?;
            crate::market::check(a)?;
            match (e.kind, cat_index(e.category)) {
                (Some(k), Some(_)) => Ok(k),
                _ => Err(format!(
                    "`{a}` is a {}: place it with objects_place",
                    e.category.name().to_lowercase()
                )),
            }
        })
        .collect()
}

fn amounts(In(args): In<Value>, mut d: WorldData) -> R {
    let keys = [
        "grass",
        "grass_height",
        "trees",
        "rocks",
        "plants",
        "flowers",
        "mushrooms",
        "berries",
        "debris",
    ];
    let vals: Vec<Option<f32>> = keys
        .iter()
        .map(|k| opt_num(&args, k))
        .collect::<Result<_, _>>()?;
    if vals.iter().all(Option::is_none) {
        return Err("give at least one of grass, grass_height (0.3..2.5), trees (0..5), rocks, plants, flowers, mushrooms, berries, debris (0..2)".into());
    }
    let a = &mut *d.amounts;
    for (k, v) in keys.iter().zip(vals) {
        let Some(v) = v else { continue };
        let v = match *k {
            "grass_height" => v.clamp(0.3, 2.5),
            "trees" => v.clamp(0.0, 5.0),
            _ => v.clamp(0.0, 2.0),
        };
        match *k {
            "grass" => a.grass = v,
            "grass_height" => a.grass_height = v,
            "trees" => a.trees = v,
            "rocks" => a.rocks = v,
            "plants" => a.plants = v,
            "flowers" => a.flowers = v,
            "mushrooms" => a.mushrooms = v,
            "berries" => a.berries = v,
            _ => a.debris = v,
        }
    }
    d.meta.dirty = true;
    text(format!(
        "Grass {:.0}%, trees {:.0}%, rocks {:.0}%, plants {:.0}%, flowers {:.0}%, mushrooms {:.0}%, berries {:.0}%, debris {:.0}%.",
        a.grass * 100.0,
        a.trees * 100.0,
        a.rocks * 100.0,
        a.plants * 100.0,
        a.flowers * 100.0,
        a.mushrooms * 100.0,
        a.berries * 100.0,
        a.debris * 100.0
    ))
}

fn appearance(In(args): In<Value>, mut d: WorldData) -> R {
    let layout = match opt_str(&args, "layout")? {
        Some("natural_groves") => Some(crate::nature::ForestLayout::NaturalGroves),
        Some("mixed") => Some(crate::nature::ForestLayout::Mixed),
        Some(_) => return Err("`layout` must be natural_groves or mixed".into()),
        None => None,
    };
    let enabled = opt_bool(&args, "enabled")?;
    let lushness = opt_num(&args, "grass_lushness")?;
    let shade = opt_num(&args, "canopy_shade")?;
    if enabled.is_none() && layout.is_none() && lushness.is_none() && shade.is_none() {
        let a = d.appearance.sanitized();
        return json_out(
            json!({ "forest": { "enabled": a.forest.enabled, "layout": match a.forest.layout { crate::nature::ForestLayout::NaturalGroves => "natural_groves", crate::nature::ForestLayout::Mixed => "mixed" } }, "grass_lushness": a.grass_lushness, "canopy_shade": a.canopy_shade }),
        );
    }
    let mut after = d.appearance.sanitized();
    if let Some(on) = enabled {
        after.forest.enabled = on;
    }
    if let Some(layout) = layout {
        after.forest.layout = layout;
    }
    if let Some(v) = lushness {
        after.grass_lushness = if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    if let Some(v) = shade {
        after.canopy_shade = v;
    }
    crate::nature::set_appearance(&mut d.appearance, &mut d.history, after);
    d.meta.dirty = true;
    text(format!(
        "Forest diversity {} ({}) · grass lushness {:.0}% · shade under trees {:.0}%.",
        if after.forest.enabled { "on" } else { "off" },
        after.forest.layout.name(),
        after.grass_lushness * 100.0,
        after.sanitized().canopy_shade * 100.0
    ))
}

fn ground_cover(
    In(args): In<Value>,
    mut d: WorldData,
    tiles: Option<Res<crate::groundcover::GroundTiles>>,
    shade: Res<crate::nature::shade::CanopyShade>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    frames: Option<Res<crate::frame_stats::FrameStats>>,
) -> R {
    let mut after = d.appearance.sanitized();
    let mut any = false;
    for (key, v) in [
        ("undergrowth", &mut after.ground.undergrowth),
        ("herbs", &mut after.ground.herbs),
        ("tufts", &mut after.ground.tufts),
        ("grass_under_trees", &mut after.ground.grass_under_trees),
        ("clumping", &mut after.ground.clumping),
    ] {
        if let Some(x) = opt_num(&args, key)? {
            *v = x;
            any = true;
        }
    }
    if any {
        crate::nature::set_appearance(&mut d.appearance, &mut d.history, after);
        d.meta.dirty = true;
    }
    let g = d.appearance.sanitized().ground;
    let (herbs, ferns, tufts) = tiles.map(|t| t.counts()).unwrap_or_default();
    json_out(json!({
        "undergrowth": g.undergrowth,
        "herbs": g.herbs,
        "tufts": g.tufts,
        "grass_under_trees": g.grass_under_trees,
        "clumping": g.clumping,
        "near_viewer": { "herb_clumps": herbs, "ferns": ferns, "grass_tufts": tufts },
        "canopy_cover_at_viewer": cam.single().map(|c| shade.cover_at(c.translation().xz())).unwrap_or(0.0),
        "frame_ms_median": frames.map(|f| f.median_ms),
        "note": if d.meta.art_style.painted() { "the hand-painted style has no ground cover (photoscans)" } else { "counts follow the viewer and update over a few frames" },
    }))
}

/// Counts per asset of what grows in an area (procedural plus placed nature objects).
fn census(
    d: &WorldData,
    rules: &Rules,
    masks: &Masks,
    area: &Area,
    cats: &[usize],
    kinds: &[AssetKind],
) -> Option<(BTreeMap<String, usize>, Vec<Instance>, Vec<u64>)> {
    let lib = d.lib.as_deref()?;
    let ctx = crate::nature::Ctx {
        terrain: &d.terrain,
        water: &d.water,
        zones: &d.zones,
        lib,
        field: &d.nature,
        rules,
        masks,
        amounts: crate::nature::amounts(&d.amounts),
        flora_amounts: d.amounts.flora_amounts(),
        appearance: *d.appearance,
        seed: d.meta.seed,
    };
    let (lo, hi) = area.bounds();
    let mut found = crate::nature::place::place_rect(&ctx, lo, hi, cats);
    found.retain(|i| area.contains(i.pos.xz()) && (kinds.is_empty() || kinds.contains(&i.kind)));
    let (c, r) = area.circle();
    let placed: Vec<u64> = d
        .store
        .query(c, r)
        .into_iter()
        .filter(|id| {
            let o = &d.store.items[id];
            lib.get(&o.asset).and_then(|e| e.kind).is_some_and(|k| {
                cat_index(kind_category(k)).is_some_and(|ci| cats.contains(&ci))
                    && (kinds.is_empty() || kinds.contains(&k))
            }) && area.contains(Vec2::new(o.pos[0], o.pos[2]))
        })
        .collect();
    let mut per = BTreeMap::new();
    for i in &found {
        *per.entry(crate::objects::builtin_id(i.kind))
            .or_insert(0usize) += 1;
    }
    for id in &placed {
        *per.entry(d.store.items[id].asset.clone()).or_insert(0usize) += 1;
    }
    Some((per, found, placed))
}

/// Areas bigger than this are refused by the per-instance tools (the counts would take long).
const MAX_AREA_M2: f32 = 64.0e6;

fn check_area(area: &Area) -> Result<(), String> {
    let (lo, hi) = area.bounds();
    if (hi - lo).element_product() > MAX_AREA_M2 {
        return Err("that area is larger than 8 × 8 km: query or paint it in parts".into());
    }
    Ok(())
}

fn summary(per: &BTreeMap<String, usize>) -> String {
    let total: usize = per.values().sum();
    if total == 0 {
        return "nothing".into();
    }
    let mut v: Vec<(&String, &usize)> = per.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    let list: Vec<String> = v.iter().take(8).map(|(k, n)| format!("{n} {k}")).collect();
    format!(
        "{total} ({}{})",
        list.join(", "),
        if v.len() > 8 { ", …" } else { "" }
    )
}

fn scatter(
    In(args): In<Value>,
    mut d: WorldData,
    rules: Option<Res<Rules>>,
    masks: Res<Masks>,
) -> R {
    let area = Area::parse(args.get("area").ok_or("missing `area`")?)?;
    check_area(&area)?;
    let rules = rules.ok_or("the asset library is still loading; try again in a moment")?;
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading; try again in a moment")?;
    let mode = opt_str(&args, "mode")?.unwrap_or("add");
    let preset =
        match opt_str(&args, "preset")? {
            Some(p) => Some(rules.preset_index(p).ok_or_else(|| {
                format!("unknown preset `{p}` (one of {})", preset_ids().join(", "))
            })?),
            None => None,
        };
    let kinds = kinds_arg(&args, lib)?;
    // a hand-painted world's own nature has no photoscans (`art_style::kind_grows`): painting
    // them on purpose mixes styles, which the result points out
    let art = d.meta.art_style;
    let off: Vec<String> = kinds
        .iter()
        .filter(|k| !crate::art_style::kind_grows(**k, art))
        .map(|k| crate::objects::builtin_id(*k))
        .collect();
    let mixed = (mode == "add" && !off.is_empty()).then(|| {
        format!(
            " Note: {} {} photoscanned, in a hand-painted world (its own nature grows only the procedural trees, bushes, palms, cacti, reeds and lily pads with painted textures).",
            off.join(", "),
            if off.len() == 1 { "is" } else { "are" }
        )
    });
    let cat = cat_arg(&args)?;
    let density = num_in(&args, "density", 1.0, 0.05, 3.9)?;
    let amount = num_in(&args, "amount", 0.5, 0.0, 1.0)?;
    let target = match (preset, kinds.is_empty(), cat) {
        (Some(p), _, _) => Target::Preset(p),
        (None, false, _) => Target::Species(kinds.clone()),
        (None, true, Some(c)) => Target::Category(c),
        (None, true, None) if mode != "add" => Target::All,
        _ => return Err("give a `preset`, `assets` or a `category` to add".into()),
    };
    let (before, _, _) = census(&d, &rules, &masks, &area, &[0, 1, 2, 3], &[])
        .ok_or("the asset library is still loading")?;
    let (lo, hi) = area.bounds();
    let op = match mode {
        "add" => crate::nature::AreaOp::Add(density),
        "thin" => crate::nature::AreaOp::Keep(1.0 - amount),
        "remove" => crate::nature::AreaOp::Keep(0.0),
        m => return Err(format!("unknown mode `{m}` (add, thin or remove)")),
    };
    crate::nature::paint_region(
        &mut d.nature,
        &mut d.history,
        "Nature brush",
        lo,
        hi,
        |p| area.contains(p),
        &target,
        op,
    );
    d.meta.dirty = true;
    let (after, _, _) = census(&d, &rules, &masks, &area, &[0, 1, 2, 3], &[])
        .ok_or("the asset library is still loading")?;
    text(format!(
        "Before: {}. Now: {}. One undo step.{}",
        summary(&before),
        summary(&after),
        mixed.unwrap_or_default()
    ))
}

fn query(In(args): In<Value>, d: WorldData, rules: Option<Res<Rules>>, masks: Res<Masks>) -> R {
    let area = match args.get("area").filter(|a| !a.is_null()) {
        Some(a) => Area::parse(a)?,
        None => Area::Circle(Vec2::ZERO, 500.0),
    };
    check_area(&area)?;
    let rules = rules.ok_or("the asset library is still loading; try again in a moment")?;
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading")?;
    let cats: Vec<usize> = match cat_arg(&args)? {
        Some(c) => vec![c],
        None => vec![0, 1, 2, 3],
    };
    let kinds = kinds_arg(&args, lib)?;
    let limit = num(&args, "limit", 50.0)?.clamp(0.0, 2000.0) as usize;
    let (per, mut found, placed) = census(&d, &rules, &masks, &area, &cats, &kinds)
        .ok_or("the asset library is still loading")?;
    let centre = area.circle().0;
    let mut list: Vec<(f32, Value)> = vec![];
    found.sort_by(|a, b| {
        a.pos
            .xz()
            .distance_squared(centre)
            .total_cmp(&b.pos.xz().distance_squared(centre))
    });
    for i in found.iter().take(limit) {
        let yaw = super::objects::yaw_of(&i.placed());
        list.push((
            i.pos.xz().distance(centre),
            json!({ "id": id_str(i.id), "asset": crate::objects::builtin_id(i.kind), "position": i.pos.to_array().map(super::terrain::round1), "yaw_deg": yaw, "scale": (i.scale * 100.0).round() / 100.0, "procedural": true }),
        ));
    }
    for id in &placed {
        let o = &d.store.items[id];
        let yaw = super::objects::yaw_of(o);
        list.push((Vec2::new(o.pos[0], o.pos[2]).distance(centre), json!({ "id": o.id, "asset": o.asset, "position": o.pos.map(super::terrain::round1), "yaw_deg": yaw, "scale": super::objects::scale_of(o), "procedural": false })));
    }
    list.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: usize = per.values().sum();
    json_out(
        json!({ "total": total, "per_asset": per, "instances": list.into_iter().take(limit).map(|x| x.1).collect::<Vec<_>>() }),
    )
}

fn rules_info(
    In(_): In<Value>,
    rules: Option<Res<Rules>>,
    lib: Option<Res<crate::objects::Library>>,
    appearance: Res<crate::nature::NatureAppearance>,
) -> R {
    let (Some(rules), Some(lib)) = (rules, lib) else {
        return Err("the asset library is still loading; try again in a moment".into());
    };
    let presets: Vec<Value> = rules
        .presets
        .iter()
        .map(|p| {
            let cats: serde_json::Map<String, Value> = p
                .cats
                .iter()
                .enumerate()
                .filter(|(_, ls)| !ls.is_empty())
                .map(|(c, ls)| {
                    let layers: Vec<Value> = ls
                        .iter()
                        .map(|l| json!({ "asset": lib.entries[l.entry].id, "per_1000_m2": (l.density * 1000.0 * 100.0).round() / 100.0, "scale": [l.scale.0, l.scale.1], "max_slope_deg": l.max_slope, "altitude_m": [l.alt.0, l.alt.1.min(9999.0)] }))
                        .collect();
                    (NATURE_CATS[c].name().to_lowercase(), json!(layers))
                })
                .collect();
            json!({ "preset": snake(&p.name.replace([' ', '-'], "")), "layers": cats })
        })
        .collect();
    let biomes: Vec<Value> = crate::biomes::BiomePreset::ALL
        .iter()
        .map(|b| {
            let d = b.def();
            json!({ "biome": snake(&format!("{b:?}")), "presets": d.vegetation.iter().map(|v| snake(&v.replace([' ', '-'], ""))).collect::<Vec<_>>(), "density": d.density })
        })
        .collect();
    let appearance = appearance.sanitized();
    json_out(json!({
        "presets": presets,
        "biomes": biomes,
        "appearance": {
            "forest": {
                "enabled": appearance.forest.enabled,
                "layout": match appearance.forest.layout {
                    crate::nature::ForestLayout::NaturalGroves => "natural_groves",
                    crate::nature::ForestLayout::Mixed => "mixed",
                },
            },
            "grass_lushness": appearance.grass_lushness,
            "canopy_shade": appearance.canopy_shade,
        },
        "note": "A spot grows its painted preset, else its biome's (picked by large-scale patch noise), else — once the world is populated — the classic land rules (slope, altitude, moisture, forest noise). Rich forest diversity only replaces automatic tree layers: temperate and autumn forests can add maples, ginkgo and sakura, sequoias are rare temperate conifer anchors, and bamboo is restricted to jungle ecology. Explicit painted species stay exact.",
    }))
}

fn pin(In(args): In<Value>, mut d: WorldData, rules: Option<Res<Rules>>, masks: Res<Masks>) -> R {
    let rules = rules.ok_or("the asset library is still loading; try again in a moment")?;
    let lib = d
        .lib
        .as_deref()
        .ok_or("the asset library is still loading")?;
    let ids: Vec<String> = crate::mcp_tools::opt_arg(&args, "ids")?.unwrap_or_default();
    let mut found: Vec<Instance> = vec![];
    if !ids.is_empty() {
        let ctx = crate::nature::Ctx {
            terrain: &d.terrain,
            water: &d.water,
            zones: &d.zones,
            lib,
            field: &d.nature,
            rules: &rules,
            masks: &masks,
            amounts: crate::nature::amounts(&d.amounts),
            flora_amounts: d.amounts.flora_amounts(),
            appearance: *d.appearance,
            seed: d.meta.seed,
        };
        for s in &ids {
            let id = parse_id(s)?;
            found.push(
                crate::nature::place::find(&ctx, id)
                    .ok_or_else(|| format!("`{s}` doesn't grow (any more)"))?,
            );
        }
    } else if let Some(a) = args.get("area") {
        let area = Area::parse(a)?;
        check_area(&area)?;
        let kinds = kinds_arg(&args, lib)?;
        if kinds.is_empty() {
            return Err("pinning an area needs `assets`".into());
        }
        let (_, f, _) = census(&d, &rules, &masks, &area, &[0, 1, 2, 3], &kinds)
            .ok_or("the asset library is still loading")?;
        if f.len() > 500 {
            return Err(format!(
                "{} instances: too many to pin at once (500); use a smaller area",
                f.len()
            ));
        }
        found = f;
    } else {
        return Err("give `ids`, or an `area` with `assets`".into());
    }
    let WorldData {
        store,
        history,
        meta,
        ..
    } = &mut d;
    let new = crate::nature::brush::pin(store, history, &found);
    meta.dirty = true;
    json_out(json!({ "pinned": new.len(), "object_ids": new }))
}

fn remove(In(args): In<Value>, mut d: WorldData) -> R {
    let ids: Vec<String> = crate::mcp_tools::arg(&args, "ids")?;
    let ids: Vec<u64> = ids.iter().map(|s| parse_id(s)).collect::<Result<_, _>>()?;
    let WorldData {
        nature,
        history,
        meta,
        ..
    } = &mut d;
    crate::nature::brush::remove(nature, history, &ids);
    meta.dirty = true;
    text(format!("Removed {} instance(s). One undo step.", ids.len()))
}
