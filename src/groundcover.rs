//! Ground cover: the knee-high layer between the meadow blades (`grass`) and the plants nature
//! places one by one (`nature`). Two parts, both photoscans (`tools/process_photoscan.py`):
//!
//! - forest undergrowth under the crowns: ferns (the library's fern scan) and broad-leaved herbs
//!   (`herbs`: weed rosettes, nettles, leafy stalks), in single-species patches;
//! - photo grass tufts among the meadow blades in the open (`meadow_tuft`).
//!
//! Nature's plants are capped at 0.25 a square metre (its blue-noise slots), far too sparse for a
//! woodland floor, so the cover is its own layer: placed deterministically per 8 m tile around the
//! viewer (out to ~55 m; the forest-floor terrain texture carries on beyond), up to a clump a
//! square metre, in clumps from patch noise. Where it grows comes from the land (the grass density
//! map: no water, rock, sand or roads), placed objects and roads (nature's masks), and the crowns
//! above (`nature::shade`'s cover: undergrowth under them, tufts in the open, while the grass
//! shader thins the meadow under them). Each plant is one GPU-culled entity per level of detail its
//! tile's distance can show, sharing meshes and one material per set, so they draw as instanced
//! batches like nature's plants. Settings: `NatureAppearance::ground` (saved with the world; the
//! Land tab's nature dials, MCP `ground_cover`). Realistic art style only: the scans are photos.

use std::collections::HashMap;

use bevy::camera::visibility::{NoCpuCulling, VisibilityRange};
use bevy::image::CompressedImageFormatSupport;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;

use crate::camera::MainCamera;
use crate::grass::GrassField;
use crate::nature::shade::CanopyShade;
use crate::nature::{GroundCover, Masks, NatureAppearance};
use crate::objects::{FoliageMaterial, Library, builtin_id};
use crate::procgen::meshes::AssetKind;
use crate::streaming::Graphics;
use crate::terrain::Terrain;
use crate::trees::TreeParts;
use crate::world::WorldMeta;

const TILE: f32 = 8.0;
/// How far the cover reaches at detail 1 (m, 3D distance).
const REACH: f32 = 55.0;
/// Tiles placed per frame at most (nearest first), so walking never stalls.
const BUDGET: usize = 24;
/// Candidate spacing (m): undergrowth clumps are ~1.3 m across, tufts ~0.5 m.
const HERB_STEP: f32 = 0.8;
const TUFT_STEP: f32 = 0.6;
/// Steepest ground anything grows on (normal's y).
const MIN_UP: f32 = 0.77;

pub struct GroundCoverPlugin;

impl Plugin for GroundCoverPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GroundTiles>()
            .add_systems(Startup, load_sets)
            .add_systems(
                PostUpdate,
                update_tiles
                    .after(crate::nature::shade::update_canopy_shade)
                    .before(crate::objects::ObjectSync),
            );
    }
}

/// Which photoscan set a plant comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Set {
    Herbs,
    Ferns,
    Tufts,
}

/// The loaded sets (none until loaded, or when their files are missing).
#[derive(Resource)]
pub struct GroundSets {
    herbs: Option<TreeParts>,
    tufts: Option<TreeParts>,
}

fn load_sets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut foliage: ResMut<Assets<FoliageMaterial>>,
    mut images: ResMut<Assets<Image>>,
    formats: Option<Res<CompressedImageFormatSupport>>,
) {
    // LOD 0 near, LOD 1 to the middle, the light LOD 2 out to the reach (scaled by detail)
    let mut load = |file, ranges| {
        crate::photoscan::load_set(
            file,
            ranges,
            &mut meshes,
            &mut foliage,
            &mut images,
            formats.as_deref(),
        )
    };
    let herbs = load("herbs", [12.0, 28.0, REACH, REACH]);
    let tufts = load("meadow_tuft", [9.0, 22.0, REACH * 0.8, REACH * 0.8]);
    commands.insert_resource(GroundSets { herbs, tufts });
}

/// One plant of the cover.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Plant {
    set: Set,
    variant: u8,
    pos: Vec3,
    yaw: f32,
    scale: f32,
}

struct Tile {
    entities: Vec<Entity>,
    plants: Vec<Plant>,
    /// the levels of detail spawned (bit per level)
    lods: u8,
    /// what the plants were placed from (shade revision, settings), to re-place when it changes
    source: u64,
}

/// The tiles alive around the viewer.
#[derive(Resource, Default)]
pub struct GroundTiles {
    tiles: HashMap<IVec2, Tile>,
    /// settings, art style and land the tiles were placed for
    key: u64,
}

impl GroundTiles {

    /// Plants alive near the viewer: (undergrowth herbs, ferns, grass tufts).
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for p in self.tiles.values().flat_map(|t| &t.plants) {
            match p.set {
                Set::Herbs => c.0 += 1,
                Set::Ferns => c.1 += 1,
                Set::Tufts => c.2 += 1,
            }
        }
        c
    }
}

#[derive(Component)]
pub struct GroundCoverEntity;

/// Everything placement reads, borrowed for one frame.
struct PlaceCtx<'a> {
    terrain: &'a Terrain,
    density: Option<(&'a [[u8; 4]], usize)>,
    shade: &'a CanopyShade,
    masks: &'a Masks,
    g: GroundCover,
    seed: u32,
    herb_variants: usize,
    fern_variants: usize,
    tuft_variants: usize,
}

#[allow(clippy::too_many_arguments)]
fn update_tiles(
    mut commands: Commands,
    mut tiles: ResMut<GroundTiles>,
    sets: Option<Res<GroundSets>>,
    lib: Option<Res<Library>>,
    shade: Res<CanopyShade>,
    appearance: Res<NatureAppearance>,
    terrain: Res<Terrain>,
    grass: Option<Res<GrassField>>,
    masks: Res<Masks>,
    meta: Res<WorldMeta>,
    graphics: Res<Graphics>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
) {
    let (Some(sets), Some(lib), Ok(cam)) = (sets, lib, cam.single()) else {
        return;
    };
    let g = appearance.sanitized().ground;
    let ferns = lib
        .get(&builtin_id(AssetKind::Fern))
        .and_then(|e| e.tree.as_ref());
    let on = !meta.art_style.painted()
        && (g.undergrowth > 0.0 || g.tufts > 0.0)
        && (sets.herbs.is_some() || sets.tufts.is_some());
    // a new world, terrain, style or setting: place everything again
    let key = [
        g.undergrowth.to_bits(),
        g.herbs.to_bits(),
        g.tufts.to_bits(),
        g.clumping.to_bits(),
        on as u32,
        meta.seed,
        terrain.generation as u32,
        terrain.height_revision as u32,
    ]
    .iter()
    .fold(0xcbf2_9ce4_8422_2325u64, |h, v| {
        (h ^ *v as u64).wrapping_mul(0x100_0000_01b3)
    });
    if key != tiles.key {
        tiles.key = key;
        for (_, t) in tiles.tiles.drain() {
            for e in t.entities {
                commands.entity(e).despawn();
            }
        }
    }
    if !on {
        return;
    }
    let eye = cam.translation();
    let detail = graphics.detail.clamp(0.5, 2.0);
    let reach = REACH * detail;
    let ground = terrain.height_at(eye.x, eye.z);
    let up = (eye.y - ground).max(0.0);
    // too high above the land for any of it to show
    if up > reach {
        for (_, t) in tiles.tiles.drain() {
            for e in t.entities {
                commands.entity(e).despawn();
            }
        }
        return;
    }
    let flat = (reach * reach - up * up).max(0.0).sqrt();
    // drop tiles out of reach
    tiles.tiles.retain(|c, t| {
        let keep = tile_dist(*c, eye.xz(), up) <= reach + TILE;
        if !keep {
            for e in t.entities.drain(..) {
                commands.entity(e).despawn();
            }
        }
        keep
    });
    // tiles to (re)place, nearest first: new ones, or ones whose crowns changed
    let source = shade.revision;
    let c0 = ((eye.xz() - flat - TILE) / TILE).floor().as_ivec2();
    let c1 = ((eye.xz() + flat + TILE) / TILE).floor().as_ivec2();
    let mut todo: Vec<(f32, IVec2)> = vec![];
    for z in c0.y..=c1.y {
        for x in c0.x..=c1.x {
            let c = IVec2::new(x, z);
            let d = tile_dist(c, eye.xz(), up);
            if d > reach + TILE * 0.5 {
                continue;
            }
            let lods = lod_mask(&sets, d, detail);
            match tiles.tiles.get(&c) {
                Some(t) if t.source == source && t.lods == lods => {}
                _ => todo.push((d, c)),
            }
        }
    }
    todo.sort_by(|a, b| a.0.total_cmp(&b.0));
    let ctx = PlaceCtx {
        terrain: &terrain,
        density: grass.as_deref().and_then(|f| f.density_map()),
        shade: &shade,
        masks: &masks,
        g,
        seed: meta.seed,
        herb_variants: sets.herbs.as_ref().map_or(0, |p| p.variants.len()),
        fern_variants: ferns.map_or(0, |p| p.variants.len()),
        tuft_variants: sets.tufts.as_ref().map_or(0, |p| p.variants.len()),
    };
    for (d, c) in todo.into_iter().take(BUDGET) {
        let lods = lod_mask(&sets, d, detail);
        // placing is cheap and deterministic: a tile whose plants and levels stayed keeps its
        // entities (the crowns' map is rebuilt as the viewer moves on, mostly to the same cover)
        let plants = place_tile(c, &ctx);
        if let Some(t) = tiles.tiles.remove(&c) {
            if t.plants == plants && t.lods == lods {
                tiles.tiles.insert(c, Tile { source, ..t });
                continue;
            }
            for e in t.entities {
                commands.entity(e).despawn();
            }
        }
        let window = (d, d + TILE * 1.42);
        let mut entities = vec![];
        for p in &plants {
            let (parts, ranges) = match p.set {
                Set::Herbs => (sets.herbs.as_ref(), None),
                Set::Tufts => (sets.tufts.as_ref(), None),
                // the library's fern has one level: it carries the undergrowth's whole reach
                Set::Ferns => (ferns, Some([reach; 3])),
            };
            if let Some(parts) = parts {
                let r = ranges.unwrap_or_else(|| {
                    [parts.ranges[0], parts.ranges[1], parts.ranges[2]].map(|v| v * detail)
                });
                spawn_plant(&mut commands, parts, p, r, window, &mut entities);
            }
        }
        tiles.tiles.insert(
            c,
            Tile {
                entities,
                plants,
                lods,
                source,
            },
        );
    }
}

/// 3D distance from the viewer to the nearest point of a tile (ground level).
fn tile_dist(c: IVec2, eye: Vec2, up: f32) -> f32 {
    let lo = c.as_vec2() * TILE;
    let near = eye.clamp(lo, lo + TILE);
    (near.distance(eye).powi(2) + up * up).sqrt()
}

/// The levels of detail a tile at distance `d` can show: a bit per level of each set (herbs
/// 0..2, tufts 3..5), so a tile is respawned only when what it shows changes.
fn lod_mask(sets: &GroundSets, d: f32, detail: f32) -> u8 {
    let (lo, hi) = (d, d + TILE * 1.42);
    let mut mask = 0;
    for (k, parts) in [&sets.herbs, &sets.tufts].into_iter().enumerate() {
        let Some(parts) = parts else { continue };
        let mut start = 0.0;
        for l in 0..3 {
            let end = parts.ranges[l] * detail;
            if hi >= start && lo <= end {
                mask |= 1 << (l + k * 3);
            }
            start = end;
        }
    }
    mask
}

/// The entities of one plant: its levels of detail (ending at `ranges`) that the tile's distance
/// `window` can show, each GPU-culled by its visibility range.
fn spawn_plant(
    commands: &mut Commands,
    parts: &TreeParts,
    p: &Plant,
    ranges: [f32; 3],
    window: (f32, f32),
    out: &mut Vec<Entity>,
) {
    let v = &parts.variants[(p.variant as usize).min(parts.variants.len() - 1)];
    let tf = Transform {
        translation: p.pos,
        rotation: Quat::from_rotation_y(p.yaw),
        scale: Vec3::splat(p.scale),
    };
    let mut lo = 0.0;
    for (l, (mesh, _)) in v.lods.iter().take(3).enumerate() {
        // a set with fewer levels stretches its last one to the end
        let hi = if l + 1 == v.lods.len().min(3) {
            ranges[2]
        } else {
            ranges[l]
        };
        let (start, end) = (lo, hi);
        lo = hi;
        if end <= start || window.1 < start || window.0 > end {
            continue;
        }
        let e = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(parts.leaves.clone()),
                tf,
                // hard switches, like nature's (a dithered fade leaves holes with the prepass)
                VisibilityRange {
                    start_margin: start..start,
                    end_margin: end..end,
                    use_aabb: false,
                },
                NoCpuCulling,
                NotShadowCaster,
                GroundCoverEntity,
            ))
            .id();
        out.push(e);
    }
}

/// Grass suitability at `p` (0..1): the grass density map, which knows water, rock, sand, roads
/// and the biomes.
fn suitability(ctx: &PlaceCtx, p: Vec2) -> f32 {
    let Some((map, n)) = ctx.density else {
        return 1.0;
    };
    let g = ctx.terrain.world_to_grid(p.x, p.y);
    let (i, j) = (g.x.floor(), g.y.floor());
    if i < 0.0 || j < 0.0 || i >= (n - 1) as f32 || j >= (n - 1) as f32 {
        return 0.0;
    }
    let (fx, fy) = (g.x - i, g.y - j);
    let (i, j) = (i as usize, j as usize);
    let r = |i: usize, j: usize| map[j * n + i][0] as f32 / 255.0;
    let a = r(i, j) + (r(i + 1, j) - r(i, j)) * fx;
    let b = r(i, j + 1) + (r(i + 1, j + 1) - r(i, j + 1)) * fx;
    a + (b - a) * fy
}

fn unit(h: u32) -> f32 {
    (h & 0xff_ffff) as f32 / 16_777_216.0
}

/// Patch noise (0..1) at `p`: dense clumps with gaps between them, `clumping` 0 even .. 1 patchy.
fn clumps(p: Vec2, size: f32, seed: u32, clumping: f32) -> f32 {
    use crate::noise::{smoothstep, value2};
    let n = value2(p.x / size, p.y / size, seed) * 0.7
        + value2(p.x / (size * 0.37), p.y / (size * 0.37), seed ^ 0x55) * 0.3;
    let patchy = smoothstep(0.42, 0.62, n) * 1.7;
    1.0 + (patchy - 1.0) * clumping
}

/// What grows in one tile (deterministic: same land, crowns and settings, same plants).
fn place_tile(c: IVec2, ctx: &PlaceCtx) -> Vec<Plant> {
    use crate::noise::{hash2, smoothstep, value2};
    let mut out = vec![];
    let origin = c.as_vec2() * TILE;
    let g = &ctx.g;
    let seed = ctx.seed ^ 0x9e37_79b9;
    let grow = |step: f32, salt: u32, out: &mut Vec<Plant>| {
        let k = (TILE / step).round() as i32;
        for j in 0..k {
            for i in 0..k {
                let gx = c.x * k + i;
                let gz = c.y * k + j;
                let h = hash2(gx, gz, seed ^ salt);
                let jitter = Vec2::new(unit(h), unit(h >> 8 ^ h.rotate_left(13)));
                let p = origin + (Vec2::new(i as f32, j as f32) + jitter) * step;
                let cover = smoothstep(0.08, 0.45, ctx.shade.cover_at(p));
                let roll = unit(hash2(gx, gz, seed ^ salt ^ 0xabcd));
                let herb = salt == 1;
                let want = if herb {
                    g.undergrowth * 1.1 * cover * clumps(p, 7.0, seed ^ 3, g.clumping)
                } else {
                    g.tufts * 0.3 * (1.0 - cover) * clumps(p, 4.5, seed ^ 5, g.clumping)
                };
                if want <= 0.0 || roll >= want {
                    continue;
                }
                // the land: grass-bearing ground, not too steep (the forest floor and soil under the
                // crowns carry little meadow but full undergrowth)
                let suit = suitability(ctx, p);
                let (s0, s1) = if herb { (0.003, 0.05) } else { (0.03, 0.35) };
                if roll >= want * smoothstep(s0, s1, suit) {
                    continue;
                }
                if ctx.terrain.normal_at(p.x, p.y).y < MIN_UP {
                    continue;
                }
                let r = if herb { 0.5 } else { 0.25 };
                if ctx.masks.blocked(crate::nature::place::cell_of(p), p, r) {
                    continue;
                }
                let h2 = hash2(gx, gz, seed ^ salt ^ 0x1234);
                let (set, variants) = if herb {
                    // single-species patches: ferns where the patch noise is low, herbs high
                    let m = value2(p.x / 11.0, p.y / 11.0, seed ^ 9);
                    let herbs = m + (unit(h2) - 0.5) * 0.12 < g.herbs;
                    if herbs && ctx.herb_variants > 0 || ctx.fern_variants == 0 {
                        (Set::Herbs, ctx.herb_variants)
                    } else {
                        (Set::Ferns, ctx.fern_variants)
                    }
                } else {
                    (Set::Tufts, ctx.tuft_variants)
                };
                if variants == 0 {
                    continue;
                }
                let y = ctx.terrain.height_at(p.x, p.y);
                out.push(Plant {
                    set,
                    variant: (h2 >> 8) as u8 % variants as u8,
                    pos: Vec3::new(p.x, y, p.y),
                    yaw: unit(h2 >> 3) * std::f32::consts::TAU,
                    scale: match set {
                        Set::Ferns => 0.7 + unit(h2 >> 5) * 0.5,
                        _ => 0.8 + unit(h2 >> 5) * 0.45,
                    },
                });
            }
        }
    };
    if g.undergrowth > 0.0 {
        grow(HERB_STEP, 1, &mut out);
    }
    if g.tufts > 0.0 {
        grow(TUFT_STEP, 2, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clumping_zero_is_even_and_one_leaves_gaps() {
        let even: Vec<f32> = (0..400)
            .map(|i| clumps(Vec2::new(i as f32 * 0.7, i as f32 * 1.3), 7.0, 3, 0.0))
            .collect();
        assert!(even.iter().all(|v| (*v - 1.0).abs() < 1e-6));
        let patchy: Vec<f32> = (0..400)
            .map(|i| clumps(Vec2::new(i as f32 * 0.7, i as f32 * 1.3), 7.0, 3, 1.0))
            .collect();
        assert!(patchy.iter().any(|v| *v < 0.05), "bare gaps between clumps");
        assert!(patchy.iter().any(|v| *v > 1.0), "denser inside the clumps");
    }

    #[test]
    fn near_tiles_show_the_first_level_and_far_ones_only_the_last() {
        let ranges = [12.0, 28.0, REACH, REACH];
        let set = || TreeParts {
            variants: vec![],
            bark: Handle::default(),
            leaves: Handle::default(),
            ranges,
        };
        let sets = GroundSets {
            herbs: Some(set()),
            tufts: None,
        };
        assert_eq!(lod_mask(&sets, 0.0, 1.0), 0b001);
        assert_eq!(lod_mask(&sets, 15.0, 1.0), 0b010);
        assert_eq!(lod_mask(&sets, 40.0, 1.0), 0b100);
        // a tile straddling a switch carries both levels
        assert_eq!(lod_mask(&sets, 6.0, 1.0), 0b011);
    }
}
