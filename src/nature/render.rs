//! Drawing procedural nature. Only what is near the viewer exists as render entities:
//!
//! - Near cells (32 m, per category) within each category's reach: one flat entity per instance
//!   and level of detail. They opt out of CPU culling (`NoCpuCulling`), so Bevy frustum-culls
//!   them and picks their LOD (`VisibilityRange`, dithered cross-fades) on the GPU and a static
//!   instance costs the CPU nothing per frame; same-mesh instances draw as one instanced batch.
//!   A cell only carries the LODs its distance band can show, and re-evaluated cells keep their
//!   entities when their instances didn't change.
//! - Billboard chunks (128 m) from the trees' last mesh LOD out to the draw distance: one merged
//!   mesh of crossed impostor quads per chunk (the shared impostor atlas), built from the same
//!   placements. Farther chunks keep fewer, larger trees (`THIN`), so their cost stays bounded
//!   however dense the forest; chunks are rebuilt when the viewer moves them into another band.
//! - Beyond that the forest canopy (`canopy.rs`).
//!
//! Evaluation runs in parallel within a few milliseconds per frame, nearest cells first; edits
//! re-evaluate only the cells their dirty rects touch.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoCpuCulling, VisibilityRange};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::prelude::*;
use rayon::prelude::*;

use super::NatureParams;
use super::place::{CELL, Instance, cell_of, place_cell};
use super::rules::CATS;
use crate::camera::MainCamera;
use crate::objects::{Category, Library};
use crate::procgen::meshes::AssetKind;
use crate::streaming::{BillboardMaterial, Graphics};

/// Billboard chunk size (m).
pub const BB_CHUNK: f32 = 128.0;
/// Billboard thinning by distance (m, × detail): (from, share of trees kept, their enlargement).
const THIN: [(f32, f32, f32); 4] = [
    (0.0, 1.0, 1.0),
    (900.0, 0.5, 1.35),
    (1600.0, 0.28, 1.7),
    (2400.0, 0.16, 2.0),
];

fn thin_band(d: f32, detail: f32) -> usize {
    THIN.iter().rposition(|t| d >= t.0 * detail).unwrap_or(0)
}
/// Extra distance before a cell or chunk is dropped after leaving its reach.
const HYSTERESIS: f32 = 48.0;
/// The distance band a cell's spawned LODs cover beyond its current one (plus a fifth of its
/// distance).
const LOD_SLACK: f32 = 30.0;

/// Densities (per m²) the default level-of-detail distances are made for: where nature grows
/// denser, a category's distances shrink with √(density) so about as many instances are drawn
/// at each level — the frame cost follows what is near, not how dense the world is.
const REF_DENSITY: [f32; 4] = [0.014, 0.004, 0.06, 0.006];
/// Never closer than this share of the default distances.
const MIN_DENSITY_DETAIL: f32 = 0.45;

/// How far (m, 3D) each category is drawn as meshes; `k` is its density detail (`Near::detail`).
pub fn reach(cat: usize, g: &Graphics, k: f32) -> f32 {
    let d = g.detail.clamp(0.4, 2.0) * k;
    match CATS[cat] {
        // the trees' last mesh LOD; billboards carry on beyond it
        Category::Trees => 320.0 * d,
        // big rocks read from afar, but past this they are sub-pixel or behind the canopy
        Category::Rocks => (700.0 * d).min(g.draw_distance),
        // small plants only up close (the photographic ground carries on)
        Category::Plants => 115.0 * d,
        _ => 260.0 * d,
    }
}

/// Render entities of the procedural instances near the viewer.
#[derive(Resource, Default)]
pub struct Near {
    cells: HashMap<(IVec2, u8), NearCell>,
    /// render entities alive (stats)
    pub entities: usize,
    last_detail: f32,
    log_timer: f32,
    /// where the last full pass looked from, and whether work was left over (cells to evaluate)
    last_pass: Option<Vec3>,
    pending: bool,
    /// render entities spawned / despawned and cells evaluated since the last log line
    churn: (usize, usize, usize),
    /// per category: the density detail in use (1 = the default distances)
    pub detail: [f32; 4],
    density_timer: f32,
}

struct NearCell {
    instances: Vec<Instance>,
    /// render entities per instance (same order as `instances`)
    entities: Vec<Vec<Entity>>,
    hash: u64,
    /// the 3D distance band (m) the spawned levels of detail cover
    window: (f32, f32),
    stale: bool,
}

impl Near {
    /// Mesh/material changes require respawning even when every placement is unchanged.
    pub fn invalidate_assets(&mut self, category: u8) {
        for ((_, cat), cell) in &mut self.cells {
            if *cat == category {
                cell.hash = 0;
                cell.stale = true;
            }
        }
    }

    /// Instances of the evaluated near cells within `r` of `c` (colliders, picking, queries).
    pub fn query(&self, c: Vec2, r: f32, out: &mut Vec<Instance>) {
        let c0 = cell_of(c - r);
        let c1 = cell_of(c + r);
        for z in c0.y..=c1.y {
            for x in c0.x..=c1.x {
                for cat in 0..4u8 {
                    if let Some(cell) = self.cells.get(&(IVec2::new(x, z), cat)) {
                        out.extend(
                            cell.instances
                                .iter()
                                .filter(|i| i.pos.xz().distance(c) <= r),
                        );
                    }
                }
            }
        }
    }

    /// Instances alive near the viewer.
    pub fn count(&self) -> usize {
        self.cells.values().map(|c| c.instances.len()).sum()
    }
}

fn cell_rect(c: IVec2) -> Rect {
    let o = c.as_vec2() * CELL;
    Rect::from_corners(o, o + CELL)
}

/// (nearest, farthest) 3D distance from the viewer to a cell.
fn cell_dist(c: IVec2, p: Vec3, ground: f32) -> (f32, f32) {
    let r = cell_rect(c);
    let q = p.xz().clamp(r.min, r.max);
    let dy = (p.y - ground).abs();
    let near = (q.distance(p.xz()).powi(2) + dy * dy).sqrt();
    let far_xz = (p.xz() - r.center()).abs() + CELL * 0.5;
    (near, (far_xz.length_squared() + dy * dy).sqrt())
}

fn content_hash(v: &[Instance]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for i in v {
        i.id.hash(&mut h);
        i.kind.hash(&mut h);
        i.entry.hash(&mut h);
        i.variant.hash(&mut h);
        i.pos.to_array().map(f32::to_bits).hash(&mut h);
        i.rot.to_array().map(f32::to_bits).hash(&mut h);
        i.scale.to_bits().hash(&mut h);
    }
    h.finish()
}

/// Marks a render entity as procedural nature: the category index of a near instance (0 trees,
/// 1 rocks, 2 plants, 3 debris), 4 billboards, 5 canopy (benchmarks hide them).
#[derive(Component)]
pub struct NatureEntity(pub u8);

#[allow(clippy::too_many_arguments)]
pub fn update_near(
    mut commands: Commands,
    mut near: ResMut<Near>,
    params: NatureParams,
    graphics: Res<Graphics>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    time: Res<Time>,
) {
    let Some(ctx) = params.ctx() else { return };
    let Ok(cam) = cam.single() else { return };
    let p = cam.translation();
    let field = &*params.field;
    if near.detail[0] == 0.0 {
        near.detail = [1.0; 4];
    }
    // a new detail setting changes every range
    if near.last_detail != graphics.detail {
        near.last_detail = graphics.detail;
        for c in near.cells.values_mut() {
            c.stale = true;
        }
    }
    // how dense nature is around the viewer, per category (once a second): denser → closer LODs
    near.density_timer -= time.delta_secs();
    if near.density_timer <= 0.0 {
        near.density_timer = 1.0;
        let probe = 140.0;
        let mut count = [0usize; 4];
        let mut cells = [0usize; 4];
        for ((c, cat), cell) in &near.cells {
            if !cell.stale && cell_rect(*c).center().distance(p.xz()) < probe {
                count[*cat as usize] += cell.instances.len();
                cells[*cat as usize] += 1;
            }
        }
        for cat in 0..4 {
            if cells[cat] < 8 {
                continue;
            }
            let rho = count[cat] as f32 / (cells[cat] as f32 * CELL * CELL);
            let want = (REF_DENSITY[cat] / rho.max(1e-6))
                .sqrt()
                .clamp(if cat == 2 { 0.85 } else { MIN_DENSITY_DETAIL }, 1.0);
            // only real changes (entering a dense forest), not every step
            if (want / near.detail[cat] - 1.0).abs() > 0.15 {
                near.detail[cat] = want;
                for ((_, k), cell) in near.cells.iter_mut() {
                    if *k as usize == cat {
                        cell.stale = true;
                    }
                }
            }
        }
    }
    for r in &field.dirty {
        for ((c, _), cell) in near.cells.iter_mut() {
            if cell_rect(*c).intersect(*r).is_empty() {
                continue;
            }
            cell.stale = true;
        }
    }
    // nothing to do while the viewer stays put and nothing changed (no per-frame scans)
    let stale = near.cells.values().any(|c| c.stale);
    if !stale && !near.pending && near.last_pass.is_some_and(|q| q.distance(p) < 2.0) {
        return;
    }
    near.last_pass = Some(p);
    let t = ctx.terrain;
    let ground = |c: IVec2| {
        let m = (c.as_vec2() + 0.5) * CELL;
        if t.in_bounds(m.x, m.y) {
            t.height_at(m.x, m.y)
        } else {
            t.sea_level
        }
    };
    let reaches: [f32; 4] = std::array::from_fn(|c| reach(c, &graphics, near.detail[c]));
    // out of reach: gone
    let mut gone = vec![];
    near.cells.retain(|(c, cat), cell| {
        let keep = cell_dist(*c, p, ground(*c)).0 <= reaches[*cat as usize] + HYSTERESIS;
        if !keep {
            gone.extend(cell.entities.drain(..).flatten());
        }
        keep
    });
    // in reach: missing, stale, or needing other levels of detail
    let mut todo: Vec<(f32, IVec2, u8)> = vec![];
    for (cat, &rr) in reaches.iter().enumerate() {
        let c0 = cell_of(p.xz() - rr);
        let c1 = cell_of(p.xz() + rr);
        for z in c0.y..=c1.y {
            for x in c0.x..=c1.x {
                let c = IVec2::new(x, z);
                let (dn, df) = cell_dist(c, p, ground(c));
                if dn > rr {
                    continue;
                }
                match near.cells.get(&(c, cat as u8)) {
                    Some(cell) if !cell.stale && dn >= cell.window.0 && df <= cell.window.1 => {}
                    _ => todo.push((dn, c, cat as u8)),
                }
            }
        }
    }
    let mut spawned = 0;
    let mut todo_left = todo.len();
    if !todo.is_empty() {
        todo.sort_by(|a, b| a.0.total_cmp(&b.0));
        let t0 = Instant::now();
        let mut at = 0;
        while at < todo.len() && t0.elapsed().as_secs_f32() < 0.004 && spawned < 12_000 {
            let batch = &todo[at..(at + 48).min(todo.len())];
            at += batch.len();
            near.churn.2 += batch.len();
            let results: Vec<(IVec2, u8, Vec<Instance>)> = batch
                .par_iter()
                .map(|(_, c, cat)| {
                    let mut v = vec![];
                    place_cell(&ctx, *c, &[*cat as usize], &mut v);
                    (*c, *cat, v)
                })
                .collect();
            for (c, cat, inst) in results {
                let (dn, df) = cell_dist(c, p, ground(c));
                // far cells move through their (wide) bands slowly: more slack, fewer rebuilds
                let slack = LOD_SLACK + dn * 0.2;
                let window = ((dn - slack).max(0.0), df + slack);
                let hash = content_hash(&inst);
                if let Some(cell) = near.cells.get_mut(&(c, cat))
                    && cell.hash == hash
                    && dn >= cell.window.0
                    && df <= cell.window.1
                {
                    cell.stale = false;
                    continue;
                }
                // keep the entities of instances that didn't change (painting grows a cell a
                // few instances at a time); a new distance window rebuilds them all
                let mut old: HashMap<u64, (Instance, Vec<Entity>)> = HashMap::new();
                let mut keep_window = None;
                if let Some(cell) = near.cells.remove(&(c, cat)) {
                    if dn >= cell.window.0 && df <= cell.window.1 {
                        keep_window = Some(cell.window);
                        old = cell
                            .instances
                            .into_iter()
                            .zip(cell.entities)
                            .map(|(i, e)| (i.id, (i, e)))
                            .collect();
                    } else {
                        gone.extend(cell.entities.into_iter().flatten());
                    }
                }
                let window = keep_window.unwrap_or(window);
                let mut entities = Vec::with_capacity(inst.len());
                for i in &inst {
                    match old.remove(&i.id) {
                        Some((o, e)) if o == *i => entities.push(e),
                        other => {
                            if let Some((_, e)) = other {
                                gone.extend(e);
                            }
                            let mut e = vec![];
                            spawn_instance(
                                &mut commands,
                                ctx.lib,
                                i,
                                window,
                                graphics.detail * near.detail[cat as usize],
                                reaches[cat as usize],
                                &mut e,
                            );
                            spawned += e.len();
                            entities.push(e);
                        }
                    }
                }
                gone.extend(old.into_values().flat_map(|(_, e)| e));
                near.cells.insert(
                    (c, cat),
                    NearCell {
                        instances: inst,
                        entities,
                        hash,
                        window,
                        stale: false,
                    },
                );
            }
        }
        todo_left = todo.len() - at;
    }
    near.pending = todo_left > 0;
    near.churn.0 += spawned;
    near.churn.1 += gone.len();
    for e in gone {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }
    near.entities = near
        .cells
        .values()
        .flat_map(|c| &c.entities)
        .map(Vec::len)
        .sum();
    // WB_NATURE_LOG=1: what is alive near the viewer, every two seconds
    if std::env::var("WB_NATURE_LOG").is_ok() && (near.log_timer + 2.0) < time.elapsed_secs() {
        near.log_timer = time.elapsed_secs();
        let per_cat: Vec<usize> = (0..4u8)
            .map(|c| {
                near.cells
                    .iter()
                    .filter(|((_, k), _)| *k == c)
                    .map(|(_, v)| v.instances.len())
                    .sum()
            })
            .collect();
        let (sp, de, ev) = std::mem::take(&mut near.churn);
        info!(
            "nature near: {} cells, instances trees {} rocks {} plants {} debris {}, {} entities, {} queued; last 2 s: {ev} cells evaluated, {sp} entities spawned, {de} despawned",
            near.cells.len(),
            per_cat[0],
            per_cat[1],
            per_cat[2],
            per_cat[3],
            near.entities,
            todo_left
        );
    }
}

/// Level-of-detail bands (m) of a library entry: (start, end) per level, ending before `cap`.
fn bands(e: &crate::objects::LibraryEntry, detail: f32, cap: f32) -> Vec<(f32, f32)> {
    let Some(t) = &e.tree else {
        return vec![(0.0, cap - CELL)];
    };
    let r = t.ranges.map(|d| d * detail);
    let end = r[2].min(cap - CELL * 0.5);
    let n = t.variants.first().map(|v| v.lods.len()).unwrap_or(1);
    // where each level hands over: the ranges, and a fourth (far) level takes the last 60% of
    // the third's stretch
    let switch = [r[0], r[1], r[1] + (r[2] - r[1]) * 0.4];
    let mut out = vec![];
    let mut lo = 0.0;
    for l in 0..n {
        let hi = if l + 1 >= n {
            end
        } else {
            switch[l.min(2)].min(end)
        };
        if hi > lo {
            out.push((lo, hi));
        }
        lo = hi;
    }
    out
}

/// The render entities of one instance: its levels of detail that the cell's distance window
/// can show, each GPU-culled with its visibility range.
fn spawn_instance(
    commands: &mut Commands,
    lib: &Library,
    i: &Instance,
    window: (f32, f32),
    detail: f32,
    cap: f32,
    out: &mut Vec<Entity>,
) {
    let e = &lib.entries[i.entry as usize];
    let tf = Transform {
        translation: i.pos,
        rotation: i.rot,
        scale: Vec3::splat(i.scale),
    };
    let bands = bands(e, detail, cap);
    let small = matches!(
        i.kind,
        AssetKind::Fern
            | AssetKind::Flowers
            | AssetKind::TallGrass
            | AssetKind::Mushrooms
            | AssetKind::Lavender
            | AssetKind::WildStrawberry
            | AssetKind::Reeds
            | AssetKind::LilyPads
            | AssetKind::Pebbles
    );
    for (l, (lo, hi)) in bands.iter().enumerate() {
        if *hi < window.0 || *lo > window.1 {
            continue;
        }
        // hard switches: with the camera's depth prepass, Bevy's dithered LOD cross-fade leaves
        // holes in GPU-culled meshes (their prepass draws both levels whole); the trees' last
        // band hands over to billboards that fade in just before it ends
        let range = VisibilityRange {
            start_margin: *lo..*lo,
            end_margin: *hi..*hi,
            use_aabb: false,
        };
        let shadows = l < 2 && !small && *lo < 160.0;
        match &e.tree {
            Some(t) => {
                let v = &t.variants[(i.variant as usize).min(t.variants.len() - 1)];
                let Some((bark, leaves)) = v.lods.get(l) else {
                    continue;
                };
                let mut b = commands.spawn((
                    Mesh3d(bark.clone()),
                    MeshMaterial3d(t.bark.clone()),
                    tf,
                    range.clone(),
                    NoCpuCulling,
                    NatureEntity(i.cat),
                ));
                if CATS[i.cat as usize] == Category::Trees {
                    b.insert(MeshTag::new(crate::forest_contours::FOREST_TAG));
                }
                if !shadows {
                    b.insert(NotShadowCaster);
                }
                out.push(b.id());
                if let Some(lv) = leaves {
                    let mut le = commands.spawn((
                        Mesh3d(lv.clone()),
                        MeshMaterial3d(t.leaves.clone()),
                        tf,
                        range,
                        NoCpuCulling,
                        NatureEntity(i.cat),
                    ));
                    if CATS[i.cat as usize] == Category::Trees {
                        le.insert(MeshTag::new(crate::forest_contours::FOREST_TAG));
                    }
                    if !shadows {
                        le.insert(NotShadowCaster);
                    }
                    out.push(le.id());
                }
            }
            None => {
                let Some((m0, _)) = e
                    .meshes
                    .get((i.variant as usize).min(e.meshes.len().saturating_sub(1)))
                else {
                    continue;
                };
                let mut b = if e.sway {
                    commands.spawn((
                        Mesh3d(m0.clone()),
                        MeshMaterial3d(lib.foliage.clone()),
                        tf,
                        range,
                        NoCpuCulling,
                        NatureEntity(i.cat),
                    ))
                } else {
                    commands.spawn((
                        Mesh3d(m0.clone()),
                        MeshMaterial3d(lib.solid.clone()),
                        tf,
                        range,
                        NoCpuCulling,
                        NatureEntity(i.cat),
                    ))
                };
                if !shadows {
                    b.insert(NotShadowCaster);
                }
                out.push(b.id());
            }
        }
    }
}

// ------------------------------------------------------------------ billboards

/// Merged billboard meshes of distant procedural trees, one entity per 128 m chunk.
#[derive(Resource, Default)]
pub struct Billboards {
    /// per chunk: entity, content hash, thinning band it was built for
    map: HashMap<IVec2, (Option<Entity>, u64, usize)>,
    /// where the last full pass looked from, and whether chunks were left to build
    last_pass: Option<Vec3>,
    pending: bool,
    stale: HashSet<IVec2>,
    material: Option<Handle<BillboardMaterial>>,
    ranges: Vec4,
}

fn chunk_rect(c: IVec2) -> Rect {
    let o = c.as_vec2() * BB_CHUNK;
    Rect::from_corners(o, o + BB_CHUNK)
}

/// How far billboards reach (m): the draw distance.
pub fn billboard_end(g: &Graphics) -> f32 {
    g.draw_distance
}

#[allow(clippy::too_many_arguments)]
pub fn update_billboards(
    mut commands: Commands,
    mut bb: ResMut<Billboards>,
    params: NatureParams,
    graphics: Res<Graphics>,
    atlas: Option<Res<crate::trees::ImpostorAtlas>>,
    mut mats: ResMut<Assets<BillboardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    near: Res<Near>,
    stroke: Res<crate::tools::Stroke>,
) {
    let (Some(ctx), Some(atlas), Ok(cam)) = (params.ctx(), atlas, cam.single()) else {
        return;
    };
    let p = cam.translation();
    // own copy of the impostor material: same atlas, but fading in where the trees' meshes end
    // and out where the canopy takes over
    // (the same distances as the trees' last band: `bands` ends it half a cell before the reach)
    let far_lod = reach(0, &graphics, near.detail[0].max(0.01)) - CELL * 0.5;
    let end = billboard_end(&graphics);
    let ranges = Vec4::new(far_lod * 0.9, far_lod, end * 0.85, end);
    let Some(src) = mats.get(&atlas.material).cloned() else {
        return;
    };
    let mat = match &bb.material {
        Some(h) => h.clone(),
        None => {
            let h = mats.add(src.clone());
            bb.material = Some(h.clone());
            h
        }
    };
    if let Some(m) = mats.get(&mat)
        && (m.base.base_color_texture != src.base.base_color_texture || bb.ranges != ranges)
    {
        bb.ranges = ranges;
        if let Some(mut m) = mats.get_mut(&mat) {
            m.base = src.base.clone();
            m.extension.ranges = ranges;
        }
    }
    for r in &params.field.dirty {
        let hit: Vec<IVec2> = bb
            .map
            .keys()
            .copied()
            .filter(|c| !chunk_rect(*c).intersect(*r).is_empty())
            .collect();
        bb.stale.extend(hit);
    }
    if (bb.stale.is_empty() || stroke.active)
        && !bb.pending
        && !graphics.is_changed()
        && bb.last_pass.is_some_and(|q| q.distance(p) < 8.0)
    {
        return;
    }
    bb.last_pass = Some(p);
    let t = ctx.terrain;
    let dist = |c: IVec2| {
        let r = chunk_rect(c);
        let q = p.xz().clamp(r.min, r.max);
        let g = if t.in_bounds(q.x, q.y) {
            t.height_at(q.x, q.y)
        } else {
            t.sea_level
        };
        (q.distance(p.xz()).powi(2) + (p.y - g).powi(2)).sqrt()
    };
    // out of reach
    let mut gone = vec![];
    bb.map.retain(|c, (e, _, _)| {
        let keep = dist(*c) <= end + HYSTERESIS;
        if !keep && let Some(e) = e.take() {
            gone.push(e);
        }
        keep
    });
    // chunks the viewer moved into another thinning band (with some slack against flicker)
    let detail = graphics.detail.clamp(0.5, 2.0);
    let moved: Vec<IVec2> = bb
        .map
        .iter()
        .filter(|(c, (_, _, band))| {
            let d = dist(**c);
            thin_band(d + 60.0, detail) < *band || thin_band(d - 60.0, detail) > *band
        })
        .map(|(c, _)| *c)
        .collect();
    bb.stale.extend(moved);
    // painting: distant trees catch up once the stroke ends (the near ones update live)
    // (chunks stay stale until rebuilt: a big change takes several frames' budgets)
    let Billboards { stale, map, .. } = &mut *bb;
    stale.retain(|c| map.contains_key(c));
    let stale: Vec<IVec2> = if stroke.active {
        vec![]
    } else {
        bb.stale.iter().copied().collect()
    };
    let mut todo: Vec<(f32, IVec2)> = stale.iter().map(|c| (dist(*c), *c)).collect();
    let c0 = (p.xz() / BB_CHUNK - end / BB_CHUNK).floor().as_ivec2();
    let c1 = (p.xz() / BB_CHUNK + end / BB_CHUNK).floor().as_ivec2();
    let half = t.half();
    for z in c0.y..=c1.y {
        for x in c0.x..=c1.x {
            let c = IVec2::new(x, z);
            let r = chunk_rect(c);
            if r.max.x < -half || r.max.y < -half || r.min.x > half || r.min.y > half {
                continue;
            }
            let d = dist(c);
            if d <= end && !bb.map.contains_key(&c) {
                todo.push((d, c));
            }
        }
    }
    if !todo.is_empty() {
        todo.sort_by(|a, b| a.0.total_cmp(&b.0));
        let t0 = Instant::now();
        let mut at = 0;
        while at < todo.len() && t0.elapsed().as_secs_f32() < 0.004 {
            let batch = &todo[at..(at + 16).min(todo.len())];
            at += batch.len();
            let built: Vec<(IVec2, Option<Mesh>, u64, usize)> = batch
                .par_iter()
                .map(|(d, c)| {
                    let band = thin_band(*d, detail);
                    let (c, m, h) = build_chunk(&ctx, *c, band);
                    (c, m, h, band)
                })
                .collect();
            for (c, mesh, hash, band) in built {
                bb.stale.remove(&c);
                if bb
                    .map
                    .get(&c)
                    .is_some_and(|(_, h, b)| *h == hash && *b == band)
                {
                    continue;
                }
                if let Some((Some(e), _, _)) = bb.map.remove(&c) {
                    gone.push(e);
                }
                let e = mesh.map(|m| {
                    let centre = (c.as_vec2() + 0.5) * BB_CHUNK;
                    let reach = end + BB_CHUNK * 0.75;
                    commands
                        .spawn((
                            Name::new("Nature billboards"),
                            Mesh3d(meshes.add(m)),
                            MeshMaterial3d(mat.clone()),
                            Transform::from_translation(Vec3::new(centre.x, 0.0, centre.y)),
                            VisibilityRange {
                                start_margin: 0.0..0.0,
                                end_margin: reach..reach + 1.0,
                                use_aabb: false,
                            },
                            NotShadowCaster,
                            NatureEntity(4),
                            MeshTag::new(crate::forest_contours::FOREST_TAG),
                        ))
                        .id()
                });
                bb.map.insert(c, (e, hash, band));
            }
        }
        bb.pending = at < todo.len();
    } else {
        bb.pending = false;
    }
    for e in gone {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }
}

/// The billboard mesh of one chunk (none when no tree grows there), thinned for its band.
fn build_chunk(ctx: &super::Ctx, c: IVec2, band: usize) -> (IVec2, Option<Mesh>, u64) {
    let per = (BB_CHUNK / CELL) as i32;
    let mut trees = vec![];
    for z in 0..per {
        for x in 0..per {
            place_cell(ctx, c * per + IVec2::new(x, z), &[0], &mut trees);
        }
    }
    let hash = content_hash(&trees);
    // farther bands keep a stable share of the trees (by id), drawn larger to keep the cover
    let (_, keep, grow) = THIN[band];
    if keep < 1.0 {
        trees.retain(|i| {
            ((i.id.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as f32 / (1u64 << 24) as f32) < keep
        });
    }
    let centre = (c.as_vec2() + 0.5) * BB_CHUNK;
    let centre = Vec3::new(centre.x, 0.0, centre.y);
    let (mut pos, mut nor, mut uv, mut idx, mut origin, mut origin_y) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    for i in &trees {
        let Some(tree) = &ctx.lib.entries[i.entry as usize].tree else {
            continue;
        };
        if tree.ranges[3] <= tree.ranges[2] {
            continue;
        }
        let v = &tree.variants[(i.variant as usize).min(tree.variants.len() - 1)];
        let Some(cell) = v.cell else { continue };
        let tf = Transform {
            translation: i.pos - centre,
            rotation: i.rot,
            scale: Vec3::splat(i.scale * grow),
        };
        crate::trees::billboard_quads(
            &mut pos, &mut nor, &mut uv, &mut idx, v.width, v.min_y, v.height, cell, &tf,
        );
        let k = pos.len() - origin.len();
        origin.extend(std::iter::repeat_n([i.pos.x, i.pos.z], k));
        origin_y.extend(std::iter::repeat_n([i.pos.y, 0.0, 0.0, 1.0], k));
    }
    if pos.is_empty() {
        return (c, None, hash);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, origin);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, origin_y);
    if idx.len() < u16::MAX as usize && idx.iter().all(|i| *i < u16::MAX as u32) {
        m.insert_indices(Indices::U16(idx.into_iter().map(|i| i as u16).collect()));
    } else {
        m.insert_indices(Indices::U32(idx));
    }
    (c, Some(m), hash)
}
