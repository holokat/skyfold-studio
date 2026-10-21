//! Terrain meshes: a quadtree of square patches, each 65×65 vertices, whose vertex spacing
//! doubles with every level. Near the camera patches sample every vertex of the heightfield;
//! further out one patch covers 4, 16, 64… times the ground, so any world size (2–16 km) draws
//! as roughly a hundred patches. Skirts hide the cracks between levels. Patches rebuild when the
//! heights under them change, and new ones are built on all cores.

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::holes::TerrainHoles;
use super::material::TerrainGpu;
use super::{GridRect, Terrain};
use crate::camera::MainCamera;

/// Cells per patch side (at the patch's own spacing).
pub const PATCH: usize = 64;
/// A patch splits into four finer ones while the camera is closer than this × its size.
const SPLIT: f32 = 1.05;

/// Quadtree node: level (0 = full detail) and position in patches of that level.
type Key = (u32, u32, u32);

/// Marks terrain patch entities (named `Terrain patch level:x,z`).
#[derive(Component)]
pub struct TerrainPatch;

#[derive(Resource, Default)]
pub struct TerrainPatches {
    patches: HashMap<Key, Entity>,
    /// nodes that were split last frame (merge hysteresis)
    split: HashSet<Key>,
    apron: Option<Entity>,
    generation: Option<u64>,
    /// min / max height per node, per level (level 0 first), for distances and bounds
    bounds: Vec<Vec<(f32, f32)>>,
    last_cam: Vec3,
}

pub struct TerrainMeshPlugin;

impl Plugin for TerrainMeshPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TerrainPatches>()
            .init_resource::<TerrainHoles>()
            .add_systems(
                PostUpdate,
                update_patches.in_set(super::TerrainSystems::Chunks),
            );
    }
}

/// Number of quadtree levels above full detail for a world of `res` cells.
fn top_level(res: usize) -> u32 {
    let mut l = 0;
    while PATCH << l < res {
        l += 1;
    }
    l
}

/// Patches per side at `level`.
fn per_side(res: usize, level: u32) -> usize {
    res.div_ceil(PATCH << level).max(1)
}

impl TerrainPatches {
    /// Rebuilds the min / max pyramid (all of it, or the level-0 nodes touching `rect`).
    fn update_bounds(&mut self, t: &Terrain, rect: Option<GridRect>) {
        let top = top_level(t.res);
        let n0 = per_side(t.res, 0);
        if self.bounds.len() != top as usize + 1 || self.bounds[0].len() != n0 * n0 {
            self.bounds = (0..=top)
                .map(|l| vec![(0.0, 0.0); per_side(t.res, l).pow(2)])
                .collect();
            return self.update_bounds(t, None);
        }
        // (a rect can outlive the terrain it was made on: opening a dungeon swaps in a much smaller
        // one while an edit of the world's terrain is still pending)
        let (x0, z0, x1, z1) = match rect {
            Some(r) => (
                (r.x0 / PATCH).min(n0 - 1),
                (r.z0 / PATCH).min(n0 - 1),
                (r.x1 / PATCH).min(n0 - 1),
                (r.z1 / PATCH).min(n0 - 1),
            ),
            None => (0, 0, n0 - 1, n0 - 1),
        };
        if x0 > x1 || z0 > z1 {
            return;
        }
        // a vertex on a patch border belongs to both patches
        let (x0, z0) = (x0.saturating_sub(1), z0.saturating_sub(1));
        let rows: Vec<(usize, Vec<(f32, f32)>)> = std::thread::scope(|s| {
            let hs: Vec<_> = (z0..=z1)
                .map(|pz| {
                    s.spawn(move || {
                        let row = (x0..=x1)
                            .map(|px| {
                                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                                for j in pz * PATCH..=((pz + 1) * PATCH).min(t.res) {
                                    for i in px * PATCH..=((px + 1) * PATCH).min(t.res) {
                                        let h = t.h(i, j);
                                        lo = lo.min(h);
                                        hi = hi.max(h);
                                    }
                                }
                                (lo, hi)
                            })
                            .collect();
                        (pz, row)
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (pz, row) in rows {
            self.bounds[0][pz * n0 + x0..=pz * n0 + x1].copy_from_slice(&row);
        }
        for l in 1..=top as usize {
            let (n, nc) = (per_side(t.res, l as u32), per_side(t.res, l as u32 - 1));
            for z in 0..n {
                for x in 0..n {
                    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                    for (cx, cz) in [
                        (2 * x, 2 * z),
                        (2 * x + 1, 2 * z),
                        (2 * x, 2 * z + 1),
                        (2 * x + 1, 2 * z + 1),
                    ] {
                        if cx < nc && cz < nc {
                            let b = self.bounds[l - 1][cz * nc + cx];
                            lo = lo.min(b.0);
                            hi = hi.max(b.1);
                        }
                    }
                    self.bounds[l][z * n + x] = (lo, hi);
                }
            }
        }
    }

    /// The patches to draw from this camera position (coarse far away, full detail near).
    fn select(&self, t: &Terrain, holes: &TerrainHoles, cam: Vec3) -> (Vec<Key>, HashSet<Key>) {
        let mut out = vec![];
        let mut split = HashSet::new();
        let top = top_level(t.res);
        self.visit(t, holes, cam, (top, 0, 0), &mut out, &mut split);
        (out, split)
    }

    fn visit(
        &self,
        t: &Terrain,
        holes: &TerrainHoles,
        cam: Vec3,
        key: Key,
        out: &mut Vec<Key>,
        split: &mut HashSet<Key>,
    ) {
        let (level, x, z) = key;
        let cells = PATCH << level;
        let size = cells as f32 * t.cell;
        let (x0, z0) = (
            -t.half() + (x as usize * cells) as f32 * t.cell,
            -t.half() + (z as usize * cells) as f32 * t.cell,
        );
        let n = per_side(t.res, level);
        let (lo, hi) = self.bounds[level as usize][z as usize * n + x as usize];
        let dx = (x0 - cam.x).max(cam.x - (x0 + size)).max(0.0);
        let dz = (z0 - cam.z).max(cam.z - (z0 + size)).max(0.0);
        let dy = (lo - cam.y).max(cam.y - hi).max(0.0);
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        // a node that was split stays split a little longer (no flicker at the threshold)
        let k = if self.split.contains(&key) {
            SPLIT * 1.12
        } else {
            SPLIT
        };
        // a patch holding a hole (a cave mouth) always draws at full detail
        let holed = level > 0
            && !holes.patches.is_empty()
            && holes.in_patches(x << level, z << level, (x + 1) << level, (z + 1) << level);
        if level == 0 || (d >= k * size && !holed) {
            out.push(key);
            return;
        }
        split.insert(key);
        let nc = per_side(t.res, level - 1) as u32;
        for (cx, cz) in [
            (2 * x, 2 * z),
            (2 * x + 1, 2 * z),
            (2 * x, 2 * z + 1),
            (2 * x + 1, 2 * z + 1),
        ] {
            if cx < nc && cz < nc {
                self.visit(t, holes, cam, (level - 1, cx, cz), out, split);
            }
        }
    }
}

/// Vertex data of one patch, built off the main thread.
struct PatchData {
    key: Key,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    indices: Vec<u32>,
    aabb: Aabb,
}

impl PatchData {
    fn mesh(self) -> (Mesh, Aabb) {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        mesh.insert_indices(Indices::U32(self.indices));
        (mesh, self.aabb)
    }
}

/// Builds many patches on all cores.
fn build_all(t: &Terrain, holes: &TerrainHoles, keys: &[Key]) -> Vec<PatchData> {
    if keys.len() < 4 {
        return keys.iter().map(|k| build_patch(t, holes, *k)).collect();
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(12);
    std::thread::scope(|s| {
        let hs: Vec<_> = keys
            .chunks(keys.len().div_ceil(threads))
            .map(|ks| {
                s.spawn(move || {
                    ks.iter()
                        .map(|k| build_patch(t, holes, *k))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

/// One patch: 65×65 vertices `2^level` cells apart, plus skirts.
fn build_patch(t: &Terrain, holes: &TerrainHoles, key: Key) -> PatchData {
    let (level, px, pz) = key;
    let step = 1usize << level;
    let verts = PATCH + 1;
    let (i0, j0) = (px as usize * PATCH * step, pz as usize * PATCH * step);
    let skirt = step as f32 * t.cell * 3.0 + 6.0;

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(verts * verts + verts * 4);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(positions.capacity());
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for vj in 0..verts {
        for vi in 0..verts {
            let i = (i0 + vi * step).min(t.res);
            let j = (j0 + vj * step).min(t.res);
            let p = t.grid_to_world(i, j);
            let h = t.h(i, j);
            min_y = min_y.min(h);
            max_y = max_y.max(h);
            positions.push([p.x, h, p.y]);
            normals.push(t.normal_at_grid(i, j).to_array());
        }
    }
    let mut indices: Vec<u32> = Vec::with_capacity((verts - 1) * (verts - 1) * 6 + verts * 24);
    for vj in 0..verts - 1 {
        for vi in 0..verts - 1 {
            // (a hole: a cave mouth draws the ground there itself)
            if level == 0 && holes.contains(i0 + vi, j0 + vj) {
                continue;
            }
            let a = (vj * verts + vi) as u32;
            let b = a + 1;
            let c = a + verts as u32;
            let d = c + 1;
            // alternate the diagonal for a more isotropic triangulation
            if (vi + vj) % 2 == 0 {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            } else {
                indices.extend_from_slice(&[a, c, d, a, d, b]);
            }
        }
    }
    // skirts: each edge vertex duplicated lower, stitched to the edge — only as deep as the ground
    // along that edge dips under it (the most a neighbouring patch's edge can lie lower: no
    // crack shows), at most `skirt`; over a cave barely at all, so none hangs into a chamber
    let depth = |e: u32| -> f32 {
        let (vi, vj) = (e as usize % verts, e as usize / verts);
        let (i, j) = ((i0 + vi * step).min(t.res), (j0 + vj * step).min(t.res));
        let h = t.h(i, j);
        let r = 2 * step;
        let along_i = vj == 0 || vj == verts - 1;
        let mut lo = h;
        if along_i {
            for a in i.saturating_sub(r)..=(i + r).min(t.res) {
                lo = lo.min(t.h(a, j));
            }
        } else {
            for b in j.saturating_sub(r)..=(j + r).min(t.res) {
                lo = lo.min(t.h(i, b));
            }
        }
        let d = (h - lo + 0.6).min(skirt);
        if holes.over_cave(i, j) { d.min(0.6) } else { d }
    };
    let v = verts as u32;
    let edges: [(Vec<u32>, bool); 4] = [
        ((0..v).collect(), true),
        ((0..v).map(|i| (v - 1) * v + i).collect(), false),
        ((0..v).map(|j| j * v).collect(), false),
        ((0..v).map(|j| j * v + v - 1).collect(), true),
    ];
    for (edge, flip) in edges {
        let base = positions.len() as u32;
        for &e in &edge {
            let p = positions[e as usize];
            positions.push([p[0], p[1] - depth(e), p[2]]);
            normals.push(normals[e as usize]);
        }
        for k in 0..edge.len() - 1 {
            let (a, b) = (edge[k], edge[k + 1]);
            let (c, d) = (base + k as u32, base + k as u32 + 1);
            if flip {
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            } else {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }
    let x0 = -t.half() + i0 as f32 * t.cell;
    let z0 = -t.half() + j0 as f32 * t.cell;
    let size = ((PATCH * step).min(t.res - i0.min(t.res))) as f32 * t.cell;
    let sizez = ((PATCH * step).min(t.res - j0.min(t.res))) as f32 * t.cell;
    let aabb = Aabb::from_min_max(
        Vec3::new(x0, min_y - skirt, z0),
        Vec3::new(x0 + size, max_y, z0 + sizez),
    );
    PatchData {
        key,
        positions,
        normals,
        indices,
        aabb,
    }
}

#[allow(clippy::too_many_arguments)]
fn update_patches(
    mut commands: Commands,
    mut terrain: ResMut<Terrain>,
    mut st: ResMut<TerrainPatches>,
    gpu: Option<Res<TerrainGpu>>,
    mut meshes: ResMut<Assets<Mesh>>,
    camera: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(&mut Mesh3d, &mut Aabb), With<TerrainPatch>>,
    holes: Res<TerrainHoles>,
) {
    let Some(gpu) = gpu else { return };
    let Ok(cam) = camera.single() else { return };
    let cam = cam.translation();
    let fresh = st.generation != Some(terrain.generation);
    if fresh {
        for (_, e) in st.patches.drain() {
            commands.entity(e).despawn();
        }
        if let Some(e) = st.apron.take() {
            commands.entity(e).despawn();
        }
        st.split.clear();
        st.bounds.clear();
        st.update_bounds(&terrain, None);
        terrain.dirty_mesh = None;
        // seabed apron beyond the world edge so the ocean floor continues to the horizon
        let apron = commands
            .spawn((
                Name::new("Seabed apron"),
                Mesh3d(meshes.add(build_apron(&terrain))),
                MeshMaterial3d(gpu.material.clone()),
                Transform::IDENTITY,
                bevy::light::NotShadowCaster,
            ))
            .id();
        st.apron = Some(apron);
        st.generation = Some(terrain.generation);
    }
    // heights changed: refresh bounds and rebuild the patches drawn over the change
    let dirty = terrain.dirty_mesh.take();
    let mut rebuild: Vec<Key> = vec![];
    if let Some(r) = dirty {
        st.update_bounds(&terrain, Some(r));
        for &(l, x, z) in st.patches.keys() {
            let cells = PATCH << l;
            let (a0, b0) = (x as usize * cells, z as usize * cells);
            if a0 <= r.x1 && a0 + cells >= r.x0 && b0 <= r.z1 && b0 + cells >= r.z0 {
                rebuild.push((l, x, z));
            }
        }
    }
    if !fresh && dirty.is_none() && cam.distance(st.last_cam) < 2.0 {
        return;
    }
    st.last_cam = cam;
    let (want, split) = st.select(&terrain, &holes, cam);
    st.split = split;
    let wanted: HashSet<Key> = want.iter().copied().collect();
    rebuild.retain(|k| wanted.contains(k));
    let new: Vec<Key> = want
        .iter()
        .copied()
        .filter(|k| !st.patches.contains_key(k))
        .collect();
    if new.is_empty() && rebuild.is_empty() && st.patches.len() == wanted.len() {
        return;
    }
    let jobs: Vec<Key> = new.iter().chain(rebuild.iter()).copied().collect();
    for data in build_all(&terrain, &holes, &jobs) {
        let key = data.key;
        let (mesh, aabb) = data.mesh();
        let handle = meshes.add(mesh);
        if let Some(&e) = st.patches.get(&key)
            && let Ok((mut m, mut a)) = q.get_mut(e)
        {
            m.0 = handle;
            *a = aabb;
            continue;
        }
        let e = commands
            .spawn((
                Name::new(format!("Terrain patch {}:{},{}", key.0, key.1, key.2)),
                TerrainPatch,
                Mesh3d(handle),
                MeshMaterial3d(gpu.material.clone()),
                Transform::IDENTITY,
                aabb,
            ))
            .id();
        st.patches.insert(key, e);
    }
    // patches no longer wanted (their replacements were built above, so no holes open up)
    let gone: Vec<Key> = st
        .patches
        .keys()
        .copied()
        .filter(|k| !wanted.contains(k))
        .collect();
    for k in gone {
        if let Some(e) = st.patches.remove(&k) {
            commands.entity(e).despawn();
        }
    }
}

/// A ring from the terrain's border out to 40 km, sloping down to the deep sea floor.
fn build_apron(t: &Terrain) -> Mesh {
    let step = (t.res / 64).max(1);
    // perimeter vertex indices, counter-clockwise seen from above
    let mut perim: Vec<(usize, usize)> = vec![];
    for i in (0..t.res).step_by(step) {
        perim.push((i, 0));
    }
    for j in (0..t.res).step_by(step) {
        perim.push((t.res, j));
    }
    for i in (1..=t.res).rev().step_by(step) {
        perim.push((i, t.res));
    }
    for j in (1..=t.res).rev().step_by(step) {
        perim.push((0, j));
    }
    let far = 40_000.0 + t.size();
    let mut pos = vec![];
    let mut idx = vec![];
    for (k, &(i, j)) in perim.iter().enumerate() {
        let p = t.grid_to_world(i, j);
        let h = t.h(i, j).min(t.sea_level - 2.0);
        let dir = p.normalize_or_zero();
        let mid = p + dir * 600.0;
        let out = dir * far;
        pos.push([p.x, h, p.y]);
        pos.push([mid.x, (h - 25.0).min(-70.0 + t.sea_level), mid.y]);
        pos.push([out.x, -90.0 + t.sea_level, out.y]);
        let a = (k * 3) as u32;
        let b = (((k + 1) % perim.len()) * 3) as u32;
        // inner ring -> middle ring -> outer ring
        idx.extend_from_slice(&[
            a,
            b,
            a + 1,
            b,
            b + 1,
            a + 1,
            a + 1,
            b + 1,
            a + 2,
            b + 1,
            b + 2,
            a + 2,
        ]);
    }
    let nor = vec![[0.0, 1.0, 0.0]; pos.len()];
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_indices(Indices::U32(idx));
    m
}
