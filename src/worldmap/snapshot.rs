//! A copy of everything the map draws, taken on the main thread in a few milliseconds so the
//! generator can run on a background thread while the crafter keeps editing.

use bevy::prelude::*;

use super::field::Grid;
use crate::gameplay::{Gameplay, PoiKind};
use crate::objects::{Category, Library, ObjectStore};
use crate::procgen::meshes::AssetKind;
use crate::roads::{RoadKind, Roads};
use crate::terrain::Terrain;
use crate::water::WaterBodies;
use crate::world::{WorldMeta, ZoneType, Zones};

pub struct ZoneInfo {
    pub id: u8,
    pub name: String,
    /// sRGB 0..1
    pub color: [f32; 3],
    pub level: String,
    pub kind: ZoneType,
}

pub struct RiverInfo {
    pub name: String,
    /// Centreline, world xz.
    pub pts: Vec<Vec2>,
    pub width: f32,
}

pub struct LakeInfo {
    pub name: String,
    /// Somewhere in the lake, world xz.
    pub seed: Vec2,
}

pub struct RoadInfo {
    pub name: String,
    pub kind: RoadKind,
    /// Centreline sampled every few metres, world xz.
    pub line: Vec<Vec2>,
}

pub struct PoiInfo {
    pub id: u32,
    pub kind: PoiKind,
    pub name: String,
    pub pos: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKind {
    Broadleaf,
    Conifer,
    Palm,
    Dead,
}

#[derive(Clone, Copy)]
pub struct TreeDot {
    pub pos: Vec2,
    pub kind: TreeKind,
}

/// A building footprint (structures, tents, huts…).
#[derive(Clone, Copy)]
pub struct Building {
    pub pos: Vec2,
    pub yaw: f32,
    pub radius: f32,
}

pub struct MapInput {
    pub name: String,
    pub res: usize,
    /// Metres per cell.
    pub cell: f32,
    pub sea: f32,
    /// Vertex heights ((res+1)², row-major from the −x/−z corner).
    pub heights: Grid,
    /// Lake / river water depth per vertex (m; negative where dry).
    pub water: Grid,
    pub biomes: Vec<u8>,
    pub zone_ids: Vec<u8>,
    pub snow_line: f32,
    pub zones: Vec<ZoneInfo>,
    pub rivers: Vec<RiverInfo>,
    pub lakes: Vec<LakeInfo>,
    pub roads: Vec<RoadInfo>,
    pub pois: Vec<PoiInfo>,
    pub start: Option<Vec2>,
    pub trees: Vec<TreeDot>,
    pub buildings: Vec<Building>,
}

impl MapInput {
    pub fn n(&self) -> usize {
        self.res + 1
    }
    pub fn size_m(&self) -> f32 {
        self.res as f32 * self.cell
    }
    pub fn half(&self) -> f32 {
        self.size_m() * 0.5
    }
}

fn tree_kind(k: Option<AssetKind>) -> TreeKind {
    use AssetKind::*;
    match k {
        Some(Pine | Fir | Spruce | SnowyPine | Cedar) => TreeKind::Conifer,
        Some(Palm | Bamboo) => TreeKind::Palm,
        Some(DeadTree) => TreeKind::Dead,
        _ => TreeKind::Broadleaf,
    }
}

/// The world state a map is made from.
pub struct Parts<'a> {
    pub terrain: &'a Terrain,
    pub water: &'a WaterBodies,
    pub zones: &'a Zones,
    pub store: &'a ObjectStore,
    pub lib: Option<&'a Library>,
    pub gameplay: &'a Gameplay,
    pub roads: &'a Roads,
    pub name: &'a str,
    /// procedural nature, for its trees
    pub nature: Option<crate::nature::Ctx<'a>>,
}

/// Copies the world's map-relevant state.
pub fn take(world: &World) -> MapInput {
    from_parts(Parts {
        terrain: world.resource::<Terrain>(),
        water: world.resource::<WaterBodies>(),
        zones: world.resource::<Zones>(),
        store: world.resource::<ObjectStore>(),
        lib: world.get_resource::<Library>(),
        gameplay: world.resource::<Gameplay>(),
        roads: world.resource::<Roads>(),
        name: &world.resource::<WorldMeta>().name,
        nature: crate::nature::world_ctx(world),
    })
}

pub fn from_parts(p: Parts) -> MapInput {
    let (t, water, zones) = (p.terrain, p.water, p.zones);
    let n = t.n();
    let mut depth = vec![-1.0f32; n * n];
    if water.level_map.len() == n * n {
        for (k, d) in depth.iter_mut().enumerate() {
            let l = water.level_map[k];
            if l.is_finite() {
                *d = l - t.heights[k];
            }
        }
    }
    let mut trees = Vec::new();
    let mut buildings = Vec::new();
    if let Some(lib) = p.lib {
        for o in p.store.items.values() {
            let Some(e) = lib.get(&o.asset) else { continue };
            let pos = Vec2::new(o.pos[0], o.pos[2]);
            match e.category {
                Category::Trees => trees.push(TreeDot {
                    pos,
                    kind: tree_kind(e.kind),
                }),
                Category::Structures | Category::Imported
                    if e.radius * o.scale >= 1.6 && !is_bridge(e) =>
                {
                    buildings.push(Building {
                        pos,
                        yaw: o.yaw(),
                        radius: e.radius * o.scale,
                    })
                }
                _ => {}
            }
        }
    }
    // procedural trees (`crate::nature`)
    if let Some(ctx) = &p.nature {
        trees.extend(
            crate::nature::place::place_world(ctx, &[0])
                .iter()
                .map(|i| TreeDot {
                    pos: i.pos.xz(),
                    kind: tree_kind(Some(i.kind)),
                }),
        );
    }
    let (gp, roads) = (p.gameplay, p.roads);
    MapInput {
        name: p.name.to_string(),
        res: t.res,
        cell: t.cell,
        sea: t.sea_level,
        heights: Grid::from_vec(n, n, t.heights.clone()),
        water: Grid::from_vec(n, n, depth),
        biomes: t.biomes.clone(),
        zone_ids: t.zones.clone(),
        snow_line: t.sea_level + t.style.snow_line,
        zones: zones
            .list
            .iter()
            .filter(|z| z.visible)
            .map(|z| ZoneInfo {
                id: z.id,
                name: z.name.clone(),
                color: z.color,
                level: z.level_label(),
                kind: z.zone_type,
            })
            .collect(),
        rivers: water
            .rivers
            .iter()
            .map(|r| RiverInfo {
                name: r.name.clone(),
                pts: r.points.iter().map(|p| Vec2::new(p[0], p[2])).collect(),
                width: r.width,
            })
            .collect(),
        lakes: water
            .lakes
            .iter()
            .map(|l| {
                let seed = if l.is_drawn() {
                    let poly: Vec<Vec2> = l.outline.iter().map(|p| Vec2::from_array(*p)).collect();
                    crate::water::lakes::centroid(&poly)
                } else {
                    Vec2::from_array(l.seed)
                };
                LakeInfo {
                    name: l.name.clone(),
                    seed,
                }
            })
            .collect(),
        roads: roads
            .roads
            .iter()
            .map(|r| RoadInfo {
                name: r.name.clone(),
                kind: r.kind,
                line: r.centreline(4.0).iter().map(|p| p.xz()).collect(),
            })
            .collect(),
        pois: gp
            .pois
            .iter()
            .map(|p| PoiInfo {
                id: p.id,
                kind: p.kind,
                name: p.name.clone(),
                pos: Vec2::new(p.pos[0], p.pos[2]),
            })
            .collect(),
        start: gp.player_start.map(|s| Vec2::new(s.pos[0], s.pos[2])),
        trees,
        buildings,
    }
}

/// Bridges and docks are drawn by the roads and rivers, not as buildings.
fn is_bridge(e: &crate::objects::LibraryEntry) -> bool {
    matches!(
        e.kind,
        Some(AssetKind::WoodenBridge | AssetKind::StoneBridge | AssetKind::Dock)
    )
}
