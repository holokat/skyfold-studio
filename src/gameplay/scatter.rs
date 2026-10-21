//! "Scatter resources": deterministic automatic placement of gathering nodes where they make
//! sense. Ore on rocky, steep and high ground and next to cliff rocks; herbs on grassland and
//! forest floors (by biome); wood at grown trees; stone at boulders and on rocky ground;
//! crystals on high peaks and volcanic rock. Never under water or on cliffs too steep to reach.
//! The tier follows the level range of the zone below (tier 1 outside zones). Mining nodes get
//! an ore of that tier which suits the ground (`ores::pick_for_spot`): iron and coal in the
//! hills, mithril on alpine peaks, obsidian and dragonsteel in volcanic land…
//!
//! Candidates come from a jittered grid hashed from the seed, so the same seed and density
//! always give the same nodes; a minimum spacing per kind keeps them apart.

use std::collections::HashMap;

use bevy::prelude::*;

use super::edit::{Ground, default_variant, levels_of, tier_for_levels, wood_variant};
use super::{Gameplay, ResourceKind, ResourceNode, ores};
use crate::biomes::BiomePreset;
use crate::noise::{Rng, hash2, smoothstep};
use crate::objects::{Category, Library, ObjectStore};
use crate::procgen::meshes::AssetKind;
use crate::terrain::layers::Layer;

#[derive(Clone, Debug)]
pub struct ScatterParams {
    /// × a typical MMO zone's node density.
    pub density: f32,
    pub seed: u32,
    /// Kinds to scatter (Ore, Herb, Wood, Stone, Crystal).
    pub kinds: Vec<ResourceKind>,
    /// The world's ore mix (abundance, mountains only, tier shares, ores off).
    pub ore: ores::OreSettings,
}

impl Default for ScatterParams {
    fn default() -> Self {
        Self {
            density: 1.0,
            seed: 1,
            kinds: vec![
                ResourceKind::Ore,
                ResourceKind::Herb,
                ResourceKind::Wood,
                ResourceKind::Stone,
            ],
            ore: ores::OreSettings::default(),
        }
    }
}

pub const SCATTER_KINDS: [ResourceKind; 5] = [
    ResourceKind::Ore,
    ResourceKind::Herb,
    ResourceKind::Wood,
    ResourceKind::Stone,
    ResourceKind::Crystal,
];

/// Nodes per km² on ideal ground at density 1, and the minimum spacing (m).
fn rate(kind: ResourceKind) -> (f32, f32) {
    match kind {
        ResourceKind::Ore => (30.0, 32.0),
        ResourceKind::Herb => (30.0, 28.0),
        ResourceKind::Wood => (18.0, 30.0),
        ResourceKind::Stone => (14.0, 40.0),
        ResourceKind::Crystal => (3.0, 90.0),
        ResourceKind::Custom => (0.0, 50.0),
    }
}

/// Rocks next to a point, by how strongly they suggest each resource.
#[derive(Default)]
struct NearRocks {
    /// cliff rocks and spires
    cliff: f32,
    /// boulders, rock clusters, mossy rocks
    boulder: f32,
}

fn near_rocks(
    store: &ObjectStore,
    lib: Option<&Library>,
    g: &Ground,
    p: Vec2,
    r: f32,
) -> NearRocks {
    let mut out = NearRocks::default();
    let Some(lib) = lib else { return out };
    let mut kinds: Vec<Option<AssetKind>> = vec![];
    for id in store.query(p, r) {
        let Some(o) = store.items.get(&id) else {
            continue;
        };
        let Some(e) = lib.get(&o.asset) else { continue };
        if e.category == Category::Rocks {
            kinds.push(e.kind);
        }
    }
    // procedural rocks
    if let Some(ctx) = &g.nature {
        kinds.extend(
            crate::nature::place::place_rect(ctx, p - r, p + r, &[1])
                .iter()
                .filter(|i| i.pos.xz().distance(p) <= r)
                .map(|i| Some(i.kind)),
        );
    }
    for k in kinds {
        match k {
            Some(AssetKind::CliffRock | AssetKind::RockSpire) => out.cliff = 1.0,
            Some(AssetKind::Pebbles) => {}
            _ => out.boulder = 1.0,
        }
    }
    out
}

/// The nearest grown tree within `r` (position, species name).
fn nearest_tree(
    store: &ObjectStore,
    lib: Option<&Library>,
    g: &Ground,
    p: Vec2,
    r: f32,
) -> Option<(Vec3, String)> {
    let lib = lib?;
    let mut trees: Vec<(u64, Vec3, String)> = store
        .query(p, r)
        .into_iter()
        .filter_map(|id| {
            let o = store.items.get(&id)?;
            let e = lib.get(&o.asset)?;
            (e.category == Category::Trees).then(|| (id, o.translation(), e.name.clone()))
        })
        .collect();
    // procedural trees
    if let Some(ctx) = &g.nature {
        trees.extend(
            crate::nature::place::place_rect(ctx, p - r, p + r, &[0])
                .iter()
                .filter(|i| i.pos.xz().distance(p) <= r)
                .map(|i| (i.id, i.pos, lib.entries[i.entry as usize].name.clone())),
        );
    }
    trees
        .into_iter()
        .min_by(|a, b| {
            a.1.xz()
                .distance(p)
                .total_cmp(&b.1.xz().distance(p))
                .then(a.0.cmp(&b.0))
        })
        .map(|(_, t, name)| (t, name))
}

/// How well a spot suits a resource (0..1), from the ground's material, slope, altitude, biome
/// and nearby rocks.
fn suitability(
    kind: ResourceKind,
    g: &Ground,
    store: &ObjectStore,
    lib: Option<&Library>,
    p: Vec2,
    top: f32,
) -> f32 {
    let t = g.terrain;
    let h = t.height_at(p.x, p.y);
    if g.water_depth(p) > 0.05 || h < t.sea_level + 0.6 {
        return 0.0;
    }
    let slope = g.slope_deg(p);
    let gi = t.world_to_grid(p.x, p.y).round();
    let (i, j) = (
        (gi.x.max(0.0) as usize).min(t.res),
        (gi.y.max(0.0) as usize).min(t.res),
    );
    let w = t.final_weights(i, j);
    let l = |x: Layer| w[x as usize];
    let rocky = (l(Layer::Rock) + l(Layer::DarkRock)) * 1.3 + l(Layer::Gravel) * 0.5;
    let grassy =
        l(Layer::Grass) + l(Layer::DryGrass) * 0.8 + l(Layer::ForestFloor) + l(Layer::Mud) * 0.4;
    let barren = l(Layer::Sand) + l(Layer::Snow) + l(Layer::Ice);
    let alt = ((h - t.sea_level) / (top - t.sea_level).max(1.0)).clamp(0.0, 1.0);
    let biome = BiomePreset::from_id(t.biome_at(p.x, p.y));
    let volcanic = (biome == Some(BiomePreset::Volcanic)) as u8 as f32;
    match kind {
        ResourceKind::Ore => {
            if slope > 52.0 {
                return 0.0;
            }
            let rocks = near_rocks(store, lib, g, p, 18.0);
            let steep = smoothstep(10.0, 34.0, slope);
            let s = 0.45 * rocky.min(1.0)
                + 0.3 * steep
                + 0.25 * smoothstep(0.12, 0.55, alt)
                + 0.55 * rocks.cliff
                + 0.25 * rocks.boulder
                + 0.3 * volcanic;
            let rich = ((s - 0.25 - 0.8 * l(Layer::Sand) - 0.3 * l(Layer::Snow)) / 0.75)
                .clamp(0.0, 1.0)
                .powf(1.4);
            // a little ore anywhere on dry land (a grassy world still has some), most where it's rocky
            let floor = 0.3
                * (1.0 - l(Layer::Sand) - 0.6 * l(Layer::Snow)).clamp(0.0, 1.0)
                * (1.0 - smoothstep(36.0, 50.0, slope));
            rich.max(floor)
        }
        ResourceKind::Herb => {
            if slope > 26.0 {
                return 0.0;
            }
            let fit = match biome {
                Some(
                    BiomePreset::TemperateForest
                    | BiomePreset::Meadow
                    | BiomePreset::AutumnWoods
                    | BiomePreset::Jungle,
                ) => 1.0,
                Some(BiomePreset::Swamp | BiomePreset::Taiga) => 0.85,
                Some(BiomePreset::HauntedWoods | BiomePreset::TropicalCoast) => 0.7,
                Some(BiomePreset::Alpine) => 0.45,
                Some(BiomePreset::Tundra) => 0.3,
                Some(BiomePreset::Desert) => 0.2,
                Some(BiomePreset::Volcanic) => 0.1,
                None => 0.85,
            };
            let flat = 1.0 - smoothstep(10.0, 26.0, slope);
            let forest_edge = nearest_tree(store, lib, g, p, 14.0).is_some() as u8 as f32;
            (flat * (grassy.min(1.0) * fit + 0.2 * forest_edge)
                - 0.6 * barren
                - 0.4 * rocky.min(1.0))
            .clamp(0.0, 1.0)
        }
        ResourceKind::Stone => {
            if slope > 40.0 {
                return 0.0;
            }
            let rocks = near_rocks(store, lib, g, p, 15.0);
            let s =
                rocks.boulder.max(rocks.cliff * 0.6) + 0.55 * rocky.min(1.0) - 0.5 * l(Layer::Sand);
            (s * (1.0 - smoothstep(28.0, 40.0, slope))).clamp(0.0, 1.0)
        }
        ResourceKind::Crystal => {
            if slope > 50.0 {
                return 0.0;
            }
            let rocks = near_rocks(store, lib, g, p, 20.0);
            (smoothstep(0.45, 0.8, alt) * rocky.min(1.0)
                + 0.6 * volcanic
                + 0.4 * rocks.cliff * smoothstep(0.3, 0.6, alt))
            .clamp(0.0, 1.0)
        }
        ResourceKind::Wood => {
            if slope > 40.0 {
                return 0.0;
            }
            nearest_tree(store, lib, g, p, 12.0).is_some() as u8 as f32
        }
        ResourceKind::Custom => 0.0,
    }
}

/// Where the nodes of `params` go (no ids yet). Deterministic for the same world, seed and
/// density. `keep` are positions of nodes that stay (new ones keep their distance).
pub fn plan(
    params: &ScatterParams,
    g: &Ground,
    store: &ObjectStore,
    lib: Option<&Library>,
    keep: &[(ResourceKind, Vec2)],
) -> Vec<ResourceNode> {
    let t = g.terrain;
    let top = t.heights.iter().copied().fold(t.sea_level + 1.0, f32::max);
    let half = t.half();
    let cell = 12.0;
    let n = (t.size() / cell).floor() as i32;
    let mut out = vec![];
    for kind in SCATTER_KINDS {
        if !params.kinds.contains(&kind) {
            continue;
        }
        let (per_km2, spacing) = rate(kind);
        let abundance = if kind == ResourceKind::Ore {
            params.ore.abundance.factor()
        } else {
            1.0
        };
        let p_max = (per_km2 * params.density.max(0.0) * abundance * cell * cell / 1.0e6).min(1.0);
        if p_max <= 0.0 {
            continue;
        }
        // spacing grid over the accepted nodes (and the kept ones of this kind)
        let mut grid: HashMap<(i32, i32), Vec<Vec2>> = HashMap::new();
        let key = |p: Vec2| {
            (
                (p.x / spacing).floor() as i32,
                (p.y / spacing).floor() as i32,
            )
        };
        let free = |grid: &HashMap<(i32, i32), Vec<Vec2>>, p: Vec2| {
            let (kx, kz) = key(p);
            (-1..=1).all(|dz| {
                (-1..=1).all(|dx| {
                    grid.get(&(kx + dx, kz + dz))
                        .is_none_or(|v| v.iter().all(|q| q.distance(p) >= spacing))
                })
            })
        };
        for (k, p) in keep {
            if *k == kind {
                grid.entry(key(*p)).or_default().push(*p);
            }
        }
        let salt =
            params.seed.wrapping_mul(0x9E37_79B9) ^ (kind as u32 + 1).wrapping_mul(0x85EB_CA6B);
        for j in 0..n {
            for i in 0..n {
                let mut rng = Rng::new(hash2(i, j, salt) as u64 | ((salt as u64) << 32));
                let roll = rng.f32();
                if roll >= p_max {
                    continue;
                }
                let p = Vec2::new(
                    -half + (i as f32 + rng.f32()) * cell,
                    -half + (j as f32 + rng.f32()) * cell,
                );
                let s = suitability(kind, g, store, lib, p, top);
                if roll >= p_max * s {
                    continue;
                }
                // wood grows on a tree: the node is the tree
                let (p, species) = if kind == ResourceKind::Wood {
                    match nearest_tree(store, lib, g, p, 12.0) {
                        Some((tp, name)) => (tp.xz(), Some(name)),
                        None => continue,
                    }
                } else {
                    (p, None)
                };
                if !free(&grid, p) {
                    continue;
                }
                grid.entry(key(p)).or_default().push(p);
                let zone_tier = g.zone_levels(p).map(tier_for_levels);
                let tier = zone_tier.unwrap_or(1);
                let biome = BiomePreset::from_id(t.biome_at(p.x, p.y));
                if kind == ResourceKind::Ore {
                    let (alt, slope) = (ores::altitude_with(t, p, top), g.slope_deg(p));
                    if params.ore.mountains_only && !ores::mountainous(biome, alt, slope) {
                        continue;
                    }
                    let Some(ore) = ores::pick_with(
                        &params.ore,
                        zone_tier.map(|z| z.min(5)),
                        biome,
                        alt,
                        slope,
                        rng.f32(),
                    ) else {
                        continue;
                    };
                    out.push(ResourceNode {
                        auto: true,
                        ..ores::deposit_node(0, ore, g.at(p), ore.default_respawn(), None)
                    });
                    continue;
                }
                let variant = match species {
                    Some(sp) => wood_variant(&sp, tier),
                    None => default_variant(kind, tier, biome),
                };
                out.push(ResourceNode {
                    id: 0,
                    kind,
                    variant,
                    tier,
                    pos: g.at(p).to_array(),
                    respawn_secs: kind.default_respawn(),
                    auto: true,
                    ore: None,
                    area: None,
                });
            }
        }
    }
    out
}

/// Replaces the scattered nodes of the chosen kinds with a new scatter (hand-placed nodes and
/// other kinds stay). Returns (added, removed).
pub fn apply(
    gp: &mut Gameplay,
    params: &ScatterParams,
    g: &Ground,
    store: &ObjectStore,
    lib: Option<&Library>,
) -> (usize, usize) {
    let before = gp.nodes.len();
    gp.nodes
        .retain(|n| !(n.auto && params.kinds.contains(&n.kind)));
    let removed = before - gp.nodes.len();
    let keep: Vec<(ResourceKind, Vec2)> = gp
        .nodes
        .iter()
        .map(|n| (n.kind, Vec2::new(n.pos[0], n.pos[2])))
        .collect();
    let planned = plan(params, g, store, lib, &keep);
    let added = planned.len();
    for mut n in planned {
        n.id = gp.alloc_id();
        gp.nodes.push(n);
    }
    if gp.selected.is_some_and(|s| !gp.exists(s)) {
        gp.selected = None;
    }
    gp.dirty = true;
    (added, removed)
}

/// Removes every scattered node (hand-placed ones stay). Returns how many.
pub fn clear(gp: &mut Gameplay) -> usize {
    let before = gp.nodes.len();
    gp.nodes.retain(|n| !n.auto);
    if gp.selected.is_some_and(|s| !gp.exists(s)) {
        gp.selected = None;
    }
    gp.dirty = true;
    before - gp.nodes.len()
}

/// The tier a node at `p` gets automatically.
pub fn zone_tier(g: &Ground, p: Vec2) -> u8 {
    g.zone(p)
        .map(|z| tier_for_levels(levels_of(z)))
        .unwrap_or(1)
}
