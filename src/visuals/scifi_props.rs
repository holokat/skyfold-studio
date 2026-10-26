//! Alien-world props: floating rocks (`floating`), monoliths (`monoliths`) and giant rock spires,
//! the rock material that keeps them all at their real grain (`rock_material`), scattering them
//! over a region for the Sci-Fi world template (`scatter_scifi_props`), and their MCP tools:
//!
//! * `scifi_props_scatter`: floating rocks, monoliths and giant spires over a disc.
//! * `objects_hover`: how high objects hang above the ground, and how floating rocks drift.
//!
//! Single objects place through `objects_place` like any asset (FloatingRock, Monolith); the
//! Nature tab's sci-fi presets (Alien Spires, Floating Isles: `objects::Preset::scifi`) grow them as
//! nature. None of it is in a fantasy world's biomes or presets.

use bevy::prelude::*;
use serde_json::{Value, json};

use super::floating::{self, FloatMotion};
use super::rock_material::{self, RockMaterial};
use crate::io::WorldData;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool};
use crate::noise::Rng;
use crate::objects::{Placed, builtin_id};
use crate::procgen::meshes::AssetKind;

pub struct ScifiPropsPlugin;

impl Plugin for ScifiPropsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<RockMaterial>::default())
            .init_resource::<FloatMotion>()
            .add_systems(Startup, rock_material::build_rock_materials)
            .add_systems(
                PostUpdate,
                (floating::mark_drifting, floating::drift)
                    .chain()
                    .after(crate::objects::ObjectSync)
                    .before(bevy::transform::TransformSystems::Propagate),
            );
        register(app);
    }
}

/// How much of each prop `scatter_scifi_props` lays out (× its natural density; 0 leaves it out).
#[derive(Clone, Copy, Debug)]
pub struct ScifiMix {
    pub floating: f32,
    pub monoliths: f32,
    pub spires: f32,
}

impl Default for ScifiMix {
    fn default() -> Self {
        Self {
            floating: 1.0,
            monoliths: 1.0,
            spires: 1.0,
        }
    }
}

/// Per km² at density 1: floating rocks (mostly pebbles and boulders, a few islands), monoliths,
/// giant rock spires (90–300 m tall).
const PER_KM2: [f32; 3] = [60.0, 22.0, 5.0];
const MAX_PROPS: usize = 2000;

#[derive(Clone, Copy)]
struct Scatter {
    center: Vec2,
    radius: f32,
    density: f32,
    seed: u64,
    mix: ScifiMix,
}

/// Scatters floating rocks, monoliths and giant rock spires over the disc `center` (x, z) ±
/// `radius` m, `density` 1 = natural (≈ 60 floating rocks, 22 monoliths and 5 spires per km²),
/// as placed objects: one undo step. Floating rocks hang 4–80 m up (the big ones higher), bob and
/// turn; monoliths and spires stand on dry ground that isn't too steep. Same `seed`, same layout.
/// Returns the new objects' ids.
#[allow(dead_code)] // (called by the Sci-Fi world template)
pub fn scatter_scifi_props(
    world: &mut World,
    center: Vec2,
    radius: f32,
    density: f32,
    seed: u64,
) -> Vec<u64> {
    scatter_scifi_mix(world, center, radius, density, seed, ScifiMix::default())
}

/// `scatter_scifi_props` with its own mix of props.
pub fn scatter_scifi_mix(
    world: &mut World,
    center: Vec2,
    radius: f32,
    density: f32,
    seed: u64,
    mix: ScifiMix,
) -> Vec<u64> {
    let a = Scatter {
        center,
        radius,
        density,
        seed,
        mix,
    };
    world.run_system_cached_with(scatter, a).unwrap_or_default()
}

fn scatter(In(a): In<Scatter>, mut d: WorldData) -> Vec<u64> {
    let mut rng = Rng::new(a.seed ^ 0x5c1f_1a7e);
    let km2 = std::f32::consts::PI * a.radius * a.radius / 1e6;
    let count = |k: usize, m: f32| {
        ((PER_KM2[k] * km2 * a.density.max(0.0) * m.max(0.0)) as usize).min(MAX_PROPS)
    };
    let want = [
        count(0, a.mix.floating),
        count(1, a.mix.monoliths),
        count(2, a.mix.spires),
    ];
    // (spot, footprint radius) of what's laid out so far
    let mut taken: Vec<(Vec2, f32)> = vec![];
    let mut out: Vec<Placed> = vec![];
    let sea = d.terrain.sea_level;
    // the biggest first, so the small fill in around them
    for (k, kind) in [
        (2, AssetKind::RockSpire),
        (1, AssetKind::Monolith),
        (0, AssetKind::FloatingRock),
    ] {
        let mut placed = 0;
        let mut tries = 0;
        while placed < want[k] && tries < want[k] * 30 + 30 {
            tries += 1;
            let r = a.radius * rng.f32().sqrt();
            let ang = rng.f32() * std::f32::consts::TAU;
            let q = a.center + Vec2::new(ang.cos(), ang.sin()) * r;
            if !d.terrain.in_bounds(q.x, q.y) {
                continue;
            }
            let ground = d.terrain.height_at(q.x, q.y);
            let slope = d
                .terrain
                .normal_at(q.x, q.y)
                .y
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            let (variant, scale, y, foot) = match kind {
                AssetKind::RockSpire => {
                    if ground < sea + 0.5 || slope > 35.0 {
                        continue;
                    }
                    let s = rng.range(6.0, 20.0);
                    (rng.below(3), s, ground - 0.15 * s, 4.0 * s * 0.8)
                }
                AssetKind::Monolith => {
                    if ground < sea + 0.3 || slope > 28.0 {
                        continue;
                    }
                    let s = rng.range(0.7, 1.9);
                    (rng.below(floor_variants(kind)), s, ground - 0.1, 2.5 * s)
                }
                _ => {
                    // many pebbles and boulders, a few islands
                    let s = 0.15 * (5.5f32 / 0.15).powf(rng.f32().powf(2.2));
                    let v = if s < 0.7 && rng.f32() < 0.75 {
                        3 + rng.below(3)
                    } else {
                        rng.below(3)
                    };
                    let gap = floating::hover_for(Some(kind), s) * rng.range(0.6, 2.4);
                    (v, s, ground.max(sea) + gap, 6.0 * s)
                }
            };
            if taken.iter().any(|(p, f)| p.distance(q) < (f + foot) * 0.8) {
                continue;
            }
            // (and clear of what's already there: towns, buildings, the user's own objects —
            // floating rocks hang over them)
            if kind != AssetKind::FloatingRock && !d.store.query(q, foot).is_empty() {
                continue;
            }
            taken.push((q, foot));
            let yaw = rng.f32() * std::f32::consts::TAU;
            out.push(Placed {
                id: 0,
                asset: builtin_id(kind),
                variant,
                pos: [q.x, y, q.y],
                rot: Quat::from_rotation_y(yaw).to_array(),
                scale,
                stretch: None,
                group: 0,
                locked: false,
                glow: None,
            });
            placed += 1;
        }
    }
    if out.is_empty() {
        return vec![];
    }
    d.history.begin("Scatter sci-fi props");
    let mut ids = vec![];
    for mut p in out {
        p.id = d.store.new_id();
        d.history.touch_object(&d.store, p.id);
        ids.push(p.id);
        d.store.insert(p);
    }
    d.history.commit(&d.store);
    d.meta.dirty = true;
    ids
}

/// Variants of the kinds laid out on the ground.
fn floor_variants(kind: AssetKind) -> u32 {
    match kind {
        AssetKind::Monolith => super::monoliths::VARIANTS,
        _ => 3,
    }
}

// ---------------------------------------------------------------------------------------------
// MCP

fn register(app: &mut App) {
    app.mcp_tool(McpTool {
        name: "scifi_props_scatter",
        description: "Scatter alien-world props over a disc (for sci-fi worlds; fantasy worlds never get them on their own): floating rocks hanging 4–80 m up (mostly pebbles and asteroid boulders, a few floating islands up to ~60 m across; they bob and turn slowly), monoliths (carved slabs, obelisks, standing stones, basalt columns) and giant rock spires 90–300 m tall, as placed objects. \
`center` [x, z], `radius` m, `density` (1 = natural: about 60 floating rocks, 22 monoliths and 5 spires per km²), optional `floating`, `monoliths`, `spires` (× each; 0 leaves it out) and `seed` (same seed, same layout). Monoliths and spires keep to dry ground that isn't steep. One undo step; returns the counts and ids. Single ones place with objects_place (FloatingRock, Monolith, RockSpire); objects_hover changes how high they hang.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "center": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "[x, z]" },
                    "radius": { "type": "number", "minimum": 10, "maximum": 20000 },
                    "density": { "type": "number", "minimum": 0, "maximum": 10, "default": 1 },
                    "floating": { "type": "number", "minimum": 0, "maximum": 10, "default": 1 },
                    "monoliths": { "type": "number", "minimum": 0, "maximum": 10, "default": 1 },
                    "spires": { "type": "number", "minimum": 0, "maximum": 10, "default": 1 },
                    "seed": { "type": "integer", "minimum": 0, "default": 1 }
                },
                "required": ["center", "radius"]
            })
        },
        run: scatter_tool,
    });
    app.mcp_tool(McpTool {
        name: "objects_hover",
        description: "How high objects hang in the air, and how floating rocks drift. `ids` (default: every FloatingRock) with `hover_m`: set each one's gap above the ground (m from the ground — or the sea, over water — to the object's origin, which for a floating rock is its lowest point; terrain edits keep the gap). Without `hover_m` it reports each one's gap. \
`motion` {bob, spin}: the floating rocks' bob height and turning speed × their natural amount (1; 0 holds them still), for this session. One undo step when anything moves; returns each object's id, asset and hover_m.",
        input_schema: || {
            json!({
                "type": "object",
                "properties": {
                    "ids": { "type": "array", "items": { "type": "integer" } },
                    "hover_m": { "type": "number", "minimum": -50, "maximum": 5000 },
                    "motion": {
                        "type": "object",
                        "properties": {
                            "bob": { "type": "number", "minimum": 0, "maximum": 5 },
                            "spin": { "type": "number", "minimum": 0, "maximum": 10 }
                        }
                    }
                }
            })
        },
        run: hover_tool,
    });
}

fn f32_arg(args: &Value, key: &str) -> Result<Option<f32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|f| Some(f as f32))
            .filter(|f| f.is_some_and(f32::is_finite))
            .ok_or_else(|| format!("`{key}` must be a number")),
    }
}

fn scatter_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let c = args
        .get("center")
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 2)
        .and_then(|a| Some(Vec2::new(a[0].as_f64()? as f32, a[1].as_f64()? as f32)))
        .ok_or("`center` must be [x, z]")?;
    let radius = f32_arg(&args, "radius")?
        .ok_or("missing `radius`")?
        .clamp(10.0, 20_000.0);
    let density = f32_arg(&args, "density")?.unwrap_or(1.0).clamp(0.0, 10.0);
    let mix = ScifiMix {
        floating: f32_arg(&args, "floating")?.unwrap_or(1.0).clamp(0.0, 10.0),
        monoliths: f32_arg(&args, "monoliths")?.unwrap_or(1.0).clamp(0.0, 10.0),
        spires: f32_arg(&args, "spires")?.unwrap_or(1.0).clamp(0.0, 10.0),
    };
    let seed = args.get("seed").and_then(|v| v.as_u64()).unwrap_or(1);
    let ids = scatter_scifi_mix(world, c, radius, density, seed, mix);
    let store = world.resource::<crate::objects::ObjectStore>();
    let n = |k: AssetKind| {
        ids.iter()
            .filter(|id| {
                store
                    .items
                    .get(id)
                    .is_some_and(|p| p.asset == builtin_id(k))
            })
            .count()
    };
    Ok(vec![McpContent::Json(json!({
        "placed": ids.len(),
        "floating_rocks": n(AssetKind::FloatingRock),
        "monoliths": n(AssetKind::Monolith),
        "spires": n(AssetKind::RockSpire),
        "ids": ids,
        "note": if ids.is_empty() { "nothing fitted: the area is outside the world, under water or too steep" } else { "one undo step" }
    }))])
}

fn hover_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    if let Some(m) = args.get("motion").filter(|m| !m.is_null()) {
        let mut motion = world.resource_mut::<FloatMotion>();
        if let Some(b) = f32_arg(m, "bob")? {
            motion.bob = b.clamp(0.0, 5.0);
        }
        if let Some(s) = f32_arg(m, "spin")? {
            motion.spin = s.clamp(0.0, 10.0);
        }
    }
    let hover = f32_arg(&args, "hover_m")?.map(|h| h.clamp(-50.0, 5000.0));
    let ids: Option<Vec<u64>> = match args.get("ids") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_array()
                .ok_or("`ids` must be a list of ids")?
                .iter()
                .map(|x| x.as_u64().ok_or("`ids` must be integers"))
                .collect::<Result<_, _>>()?,
        ),
    };
    world
        .run_system_cached_with(hover_system, (ids, hover))
        .map_err(|e| format!("{e}"))?
        .map(|(rows, motion)| {
            vec![McpContent::Json(json!({
                "objects": rows,
                "motion": { "bob": motion.bob, "spin": motion.spin },
            }))]
        })
}

type HoverRows = (Vec<Value>, FloatMotion);

fn hover_system(
    In((ids, hover)): In<(Option<Vec<u64>>, Option<f32>)>,
    mut d: WorldData,
    motion: Res<FloatMotion>,
) -> Result<HoverRows, String> {
    let floating = builtin_id(AssetKind::FloatingRock);
    let mut ids = ids.unwrap_or_else(|| {
        d.store
            .items
            .values()
            .filter(|p| p.asset == floating)
            .map(|p| p.id)
            .collect()
    });
    ids.sort_unstable();
    let sea = d.terrain.sea_level;
    let base = |d: &WorldData, p: &Placed| d.terrain.height_at(p.pos[0], p.pos[2]).max(sea);
    for id in &ids {
        if !d.store.items.contains_key(id) {
            return Err(format!("no object with id {id}"));
        }
        if hover.is_some() && d.store.is_locked(*id) {
            return Err(format!(
                "object {id} is locked (objects_lock with locked: false frees it, if the user wants it changed)"
            ));
        }
    }
    if let Some(h) = hover
        && !ids.is_empty()
    {
        d.history.begin("Set hover height");
        for id in &ids {
            let Some(mut p) = d.store.items.get(id).cloned() else {
                continue;
            };
            p.pos[1] = base(&d, &p) + h;
            d.history.touch_object(&d.store, *id);
            d.store.move_to(p);
        }
        d.history.commit(&d.store);
        d.meta.dirty = true;
    }
    let rows = ids
        .iter()
        .filter_map(|id| d.store.items.get(id))
        .take(2000)
        .map(|p| json!({ "id": p.id, "asset": p.asset, "hover_m": ((p.pos[1] - base(&d, p)) as f64 * 100.0).round() / 100.0 }))
        .collect();
    Ok((rows, *motion))
}
