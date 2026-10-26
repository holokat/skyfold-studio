//! Solid objects: what a walker bumps into, slides along and stands on.
//!
//! Every library asset variant keeps a small collision mesh in its own space (the middle level of
//! detail of rocks, logs and built objects, a tree's bark below head height, a glTF model's
//! meshes), copied once at startup or when a model first loads. Near the player each placed
//! object is turned lazily into a **column grid**: its triangles rasterised into a world-aligned
//! grid of small cells (≈0.12–0.5 m), each holding the lowest and highest point of the object over
//! that cell. That one shape answers everything a character needs:
//!
//! - a cell whose top is within a step of the feet is ground to stand on (boulder tops, logs,
//!   stairs, a bridge deck);
//! - a cell rising higher that starts below the head blocks the capsule (trunks, rock faces,
//!   walls, a table top); the capsule is pushed out of the deepest overlap first and slides along;
//! - a cell high above the head (a branch, an arch) lets the walker pass underneath.
//!
//! Walk-in buildings (`LibraryEntry::walk_in`) keep every solid span of a column instead (floor
//! boards, a ceiling, the next floor, the roof), with finer cells, so rooms, doorways, stairs and
//! upper floors have room to stand in. Their colliders are built off the main thread.
//!
//! Trees (and cacti) collide by their trunk only: a prism fitted to the bark between knee and
//! chest height (its centre and median radius), so low branches and leaves never snag anyone.
//!
//! Footsteps sound like what is underfoot: each glTF triangle carries the surface of its material
//! (`material_surface`: an explicit `surface` in the material's extras, else its name — planks and
//! beams are wood, flagstones stone, a carpet runner carpet, arena sand sand), each column keeps
//! the surface of its top (and each walk-in span of its own top), and `floor` reports it; the
//! object's own surface stands in where a material says nothing.
//!
//! Grass, flowers, ferns, reeds, bushes, pebbles, twig piles and anything lower than a few
//! centimetres stay walk-through. Colliders are cached per object and rebuilt only when the
//! object moves. `Colliders` is public so other systems (walkability analysis) can ask the same
//! questions: `ensure` an area, then `blocked`, `floor` or `footprint`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::gltf::{Gltf, GltfMaterial, GltfMesh, GltfNode};
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use crate::objects::{Category, Library, LibraryEntry, ObjectStore, Placed};
use crate::procgen::meshes::AssetKind;
use crate::terrain::layers::Surface;

/// Objects whose collider can reach this far (m) from their origin: the spatial query margin.
const REACH: f32 = 24.0;
/// Tree bark above this height (m, local) is not looked at for the trunk.
const TREE_CUT: f32 = 4.6;
/// How tall a trunk collider stands (m): nobody stands on a tree.
const TRUNK_TOP: f32 = 6.0;
/// Cells per grid side at most.
const MAX_CELLS: f32 = 56.0;
/// Cells per grid side at most for walk-in buildings (doorways need cells well under a body's width).
const WALK_IN_CELLS: f32 = 96.0;
/// Solid spans of a walk-in column closer than this (m) are one span.
const SPAN_GAP: f32 = 0.03;

/// A triangle whose material says nothing about its surface (the object's own surface applies).
const NO_SURF: u8 = u8::MAX;

/// The footstep surface of a model material, from an explicit `"surface"` in its glTF extras or
/// its name (the style packs' materials: planks, beam, gstone, cobble, canvas, sand, iron…; and
/// the usual words in other models' material names). None: the object's own surface.
pub fn material_surface(name: Option<&str>, extras: Option<&str>) -> Option<Surface> {
    if let Some(v) = extras.and_then(|x| serde_json::from_str::<serde_json::Value>(x).ok())
        && let Some(n) = v.get("surface").and_then(|x| x.as_str())
        && let Some(s) = Surface::ALL.into_iter().find(|s| {
            format!("{s:?}").eq_ignore_ascii_case(n)
                || s.name().eq_ignore_ascii_case(n)
                || s.sound_stem().eq_ignore_ascii_case(n)
        })
    {
        return Some(s);
    }
    let n = name?.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    // most specific first: "cobblestone" is cobble, "grass_ground" grass, "sandstone" stone
    // decals and lights say nothing (moss on a step is still a stone step)
    Some(
        if has(&[
            "ember", "flame", "glow", "water", "glass", "dark", "moss", "decal", "emblem",
        ]) {
            return None;
        } else if has(&[
            "carpet", "rug", "canvas", "cloth", "fabric", "felt", "cushion", "hide", "leather",
            "banner",
        ]) {
            Surface::Carpet
        } else if has(&["cobble"]) {
            Surface::Cobble
        } else if has(&["marble"]) {
            Surface::Marble
        } else if has(&["sandstone"]) {
            Surface::Stone
        } else if has(&["sand"]) {
            Surface::Sand
        } else if has(&["snow"]) {
            Surface::Snow
        } else if has(&["gravel"]) {
            Surface::Gravel
        } else if has(&["mud"]) {
            Surface::Mud
        } else if has(&["grass", "turf"]) {
            Surface::Grass
        } else if has(&["dirt", "soil", "earth"]) {
            Surface::Dirt
        } else if has(&["bone", "skull", "ivory", "tusk"]) {
            Surface::Bone
        } else if has(&["rubber", "plastic", "foam", "vinyl"]) {
            Surface::Rubber
        } else if has(&["grate", "grating", "grill", "mesh", "perforated", "walkway"]) {
            Surface::Grate
        } else if has(&[
            "iron", "steel", "metal", "copper", "bronze", "brass", "gold", "silver", "chain",
        ]) {
            Surface::Metal
        } else if has(&["asphalt", "tarmac"]) {
            Surface::Asphalt
        } else if has(&["concrete", "cement", "pavement", "sidewalk"]) {
            Surface::Concrete
        } else if has(&["ceramic", "terrazzo", "porcelain", "tiles_", "tile_floor", "floor_tile"]) {
            Surface::Tile
        } else if has(&[
            "stone", "rock", "brick", "slate", "granite", "tile", "plaster", "flag", "ashlar",
        ]) {
            Surface::Stone
        } else if has(&[
            "wood", "plank", "beam", "log", "bark", "timber", "board", "deck",
        ]) {
            Surface::Wood
        } else {
            return None;
        },
    )
}

/// Is a model material something you walk through rather than on: a water surface (by name, or
/// `"solid": false` in its glTF extras)? Its triangles stay out of the collider, so a river insert
/// in a model is waded through on its bed instead of walked on like a floor.
pub fn material_passable(name: Option<&str>, extras: Option<&str>) -> bool {
    if let Some(v) = extras.and_then(|x| serde_json::from_str::<serde_json::Value>(x).ok())
        && let Some(b) = v.get("solid").and_then(|x| x.as_bool())
    {
        return !b;
    }
    name.is_some_and(|n| n.to_ascii_lowercase().contains("water"))
}

fn surf_code(s: Option<Surface>) -> u8 {
    s.and_then(|s| Surface::ALL.iter().position(|x| *x == s))
        .map(|i| i as u8)
        .unwrap_or(NO_SURF)
}

/// Triangles of one asset variant in its local space.
pub struct ShapeMesh {
    pub pos: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    /// per triangle: its footstep surface (`Surface::ALL` index; NO_SURF: the object's own)
    pub surf: Vec<u8>,
    pub min: Vec3,
    pub max: Vec3,
}

impl ShapeMesh {
    /// Copies a mesh's triangles; `cut` drops triangles lying wholly above that height.
    fn from_mesh(m: &Mesh, tf: Mat4, cut: Option<f32>, surf: u8, out: &mut ShapeMesh) {
        let Ok(Some(VertexAttributeValues::Float32x3(p))) =
            m.try_attribute_option(Mesh::ATTRIBUTE_POSITION)
        else {
            return;
        };
        let base = out.pos.len() as u32;
        out.pos
            .extend(p.iter().map(|q| tf.transform_point3(Vec3::from_array(*q))));
        let idx: Vec<u32> = match m.try_indices() {
            Ok(Indices::U32(i)) => i.clone(),
            Ok(Indices::U16(i)) => i.iter().map(|x| *x as u32).collect(),
            Err(_) => (0..p.len() as u32).collect(),
        };
        for t in idx.chunks_exact(3) {
            let t = [t[0] + base, t[1] + base, t[2] + base];
            if let Some(c) = cut
                && t.iter().all(|&i| out.pos[i as usize].y > c)
            {
                continue;
            }
            out.tris.push(t);
            out.surf.push(surf);
        }
    }
    fn finish(mut self) -> Option<Self> {
        if self.tris.is_empty() {
            return None;
        }
        // keep only the vertices the triangles use
        let mut remap = vec![u32::MAX; self.pos.len()];
        let mut pos = vec![];
        for t in &mut self.tris {
            for i in t.iter_mut() {
                if remap[*i as usize] == u32::MAX {
                    remap[*i as usize] = pos.len() as u32;
                    pos.push(self.pos[*i as usize]);
                }
                *i = remap[*i as usize];
            }
        }
        self.pos = pos;
        self.min = self
            .pos
            .iter()
            .fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
        self.max = self
            .pos
            .iter()
            .fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
        Some(self)
    }
    fn empty() -> Self {
        ShapeMesh {
            pos: vec![],
            tris: vec![],
            surf: vec![],
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        }
    }

    /// The trunk of a tree's bark when its skeleton isn't known (palms, cacti): a prism through
    /// the bark between 0.3 and 1.4 m near the root's axis (centre and median radius of those
    /// vertices; drooping branches and fronds further out are ignored).
    fn trunk(&self) -> Option<Self> {
        let near_axis = |p: &Vec3| p.xz().length() < 1.2;
        let mut band: Vec<Vec3> = self
            .pos
            .iter()
            .copied()
            .filter(|p| p.y > 0.3 && p.y < 1.4 && near_axis(p))
            .collect();
        if band.len() < 8 {
            band = self
                .pos
                .iter()
                .copied()
                .filter(|p| p.y < 3.0 && near_axis(p))
                .collect();
        }
        if band.len() < 3 {
            return None;
        }
        let mid = |mut v: Vec<f32>| {
            v.sort_by(|a, b| a.total_cmp(b));
            v[v.len() / 2]
        };
        let c = Vec2::new(
            mid(band.iter().map(|p| p.x).collect()),
            mid(band.iter().map(|p| p.z).collect()),
        );
        let r = mid(band.iter().map(|p| p.xz().distance(c)).collect()).clamp(0.06, 0.9);
        Self::prism(c, r)
    }

    /// A trunk collider: a 12-sided prism of radius `r` around `c` (local xz), standing from
    /// below the ground to `TRUNK_TOP`.
    fn prism(c: Vec2, r: f32) -> Option<Self> {
        let n = 12;
        let mut pos = vec![];
        for k in 0..n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            let q = c + Vec2::new(a.cos(), a.sin()) * r;
            pos.push(Vec3::new(q.x, -0.5, q.y));
            pos.push(Vec3::new(q.x, TRUNK_TOP, q.y));
        }
        let mut tris = vec![];
        for k in 0..n as u32 {
            let (a, b) = (k * 2, (k + 1) % n as u32 * 2);
            tris.push([a, b, a + 1]);
            tris.push([b, b + 1, a + 1]);
        }
        let surf = vec![NO_SURF; tris.len()];
        ShapeMesh {
            pos,
            tris,
            surf,
            min: Vec3::ZERO,
            max: Vec3::ZERO,
        }
        .finish()
    }
}

/// Is this asset solid at all? (Plants, pebbles, twig piles and lily pads are walk-through.)
pub fn is_solid(e: &LibraryEntry) -> bool {
    use AssetKind::*;
    match e.kind {
        Some(Cactus) => true,
        Some(Pebbles | Branches | LilyPads | Reeds) => false,
        Some(k) => !matches!(crate::objects::kind_category(k), Category::Plants),
        None => true,
    }
}

/// A tree variant's skeleton-derived trunk radius is already measured in world metres. Keep its
/// full finite value: mature giant trees can legitimately exceed the former 1.2 m player-scale
/// ceiling.
pub(crate) fn known_trunk_radius(radius: f32) -> Option<f32> {
    radius.is_finite().then_some(radius.max(0.05))
}

/// Which mesh of a library variant the collider is made from.
fn variant_meshes(e: &LibraryEntry, v: usize) -> Option<(Handle<Mesh>, Option<f32>)> {
    if let Some(t) = &e.tree {
        let var = t.variants.get(v.min(t.variants.len().saturating_sub(1)))?;
        // Bamboo collision keeps its individual culms, so use LOD 0 rather than the decimated
        // middle bark. Other trees use the established middle level.
        let lod = if e.kind == Some(AssetKind::Bamboo) {
            var.lods.first()?
        } else {
            var.lods.get(1.min(var.lods.len().saturating_sub(1)))?
        };
        let tree = matches!(e.category, Category::Trees) || e.kind == Some(AssetKind::Cactus);
        return Some((lod.0.clone(), tree.then_some(TREE_CUT)));
    }
    let (m0, _) = e.meshes.get(v.min(e.meshes.len().saturating_sub(1)))?;
    Some((m0.clone(), None))
}

/// Bamboo has several discrete culms. Its bark below the usual tree cut is already a compact,
/// accurate walk collider; reducing it to one trunk prism would fill the walkable gaps.
fn bamboo_collision_shape(mesh: &Mesh) -> Option<ShapeMesh> {
    let mut shape = ShapeMesh::empty();
    ShapeMesh::from_mesh(mesh, Mat4::IDENTITY, Some(TREE_CUT), NO_SURF, &mut shape);
    shape.finish()
}

/// Collision meshes of every library asset variant ((asset id, variant) → mesh), plus glTF
/// imports while they load.
#[derive(Resource, Default)]
pub struct ColliderMeshes {
    meshes: HashMap<(String, u32), Arc<ShapeMesh>>,
    loading: HashMap<String, Handle<Gltf>>,
    /// assets without a usable mesh (never retried)
    missing: HashSet<(String, u32)>,
}

impl ColliderMeshes {
    pub fn get(&self, asset: &str, variant: u32) -> Option<&Arc<ShapeMesh>> {
        self.meshes
            .get(&(asset.to_string(), variant))
            .or_else(|| self.meshes.get(&(asset.to_string(), 0)))
    }
}

/// At startup every procedural, ez-tree and photoscanned mesh still has its vertices on the CPU
/// (render-only meshes give them up once uploaded): copy what the colliders need.
pub fn collect_library_meshes(
    lib: Option<Res<Library>>,
    meshes: Res<Assets<Mesh>>,
    mut out: ResMut<ColliderMeshes>,
) {
    let Some(lib) = lib else { return };
    let t0 = std::time::Instant::now();
    let mut tris = 0;
    let mut trunks: Vec<f32> = vec![];
    for e in lib
        .entries
        .iter()
        .filter(|e| is_solid(e) && e.import_path.is_none())
    {
        let n = e
            .tree
            .as_ref()
            .map(|t| t.variants.len())
            .unwrap_or(e.meshes.len());
        for v in 0..n {
            let Some((h, cut)) = variant_meshes(e, v) else {
                continue;
            };
            let Some(m) = meshes.get(&h) else { continue };
            let bamboo = e.kind == Some(AssetKind::Bamboo);
            let trunk_only =
                matches!(e.category, Category::Trees) || e.kind == Some(AssetKind::Cactus);
            // ez-tree trees know their trunk exactly (from the skeleton): only it blocks
            let known = (!bamboo && trunk_only)
                .then(|| {
                    e.tree
                        .as_ref()
                        .and_then(|t| t.variants.get(v))
                        .and_then(|x| x.trunk)
                })
                .flatten();
            let shape = if bamboo {
                bamboo_collision_shape(m)
            } else {
                match known.and_then(|(r, c)| known_trunk_radius(r).map(|r| (r, c))) {
                    Some((r, c)) => {
                        trunks.push(r);
                        ShapeMesh::prism(Vec2::new(c[0], c[1]), r)
                    }
                    None => {
                        let mut s = ShapeMesh::empty();
                        ShapeMesh::from_mesh(m, Mat4::IDENTITY, cut, NO_SURF, &mut s);
                        s.finish()
                            .and_then(|s| if trunk_only { s.trunk() } else { Some(s) })
                    }
                }
            };
            match shape {
                Some(s) => {
                    tris += s.tris.len();
                    out.meshes.insert((e.id.clone(), v as u32), Arc::new(s));
                }
                None => {
                    out.missing.insert((e.id.clone(), v as u32));
                }
            }
        }
    }
    info!(
        "colliders: {} asset meshes ({} triangles) in {:?}",
        out.meshes.len(),
        tris,
        t0.elapsed()
    );
    if !trunks.is_empty() {
        let (lo, hi) = trunks
            .iter()
            .fold((f32::MAX, 0.0f32), |(a, b), r| (a.min(*r), b.max(*r)));
        info!(
            "colliders: {} tree trunks from their skeletons, radius {lo:.2}–{hi:.2} m",
            trunks.len()
        );
    }
}

/// glTF models: their collision mesh is built from the model's node tree once it has loaded, or
/// from its own collision mesh when it carries one (a node named `collision` outside the scene).
pub fn load_gltf_meshes(
    mut out: ResMut<ColliderMeshes>,
    gltfs: Res<Assets<Gltf>>,
    nodes: Res<Assets<GltfNode>>,
    gmeshes: Res<Assets<GltfMesh>>,
    meshes: Res<Assets<Mesh>>,
    server: Res<AssetServer>,
) {
    if out.loading.is_empty() {
        return;
    }
    let pending: Vec<(String, Handle<Gltf>)> = out
        .loading
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (id, h) in pending {
        if let bevy::asset::LoadState::Failed(_) = server.load_state(&h) {
            out.loading.remove(&id);
            out.missing.insert((id, 0));
            continue;
        }
        let Some(g) = gltfs.get(&h) else { continue };
        // roots: nodes nobody lists as a child
        let roots: Vec<_> = match g.named_nodes.get("collision") {
            Some(n) => vec![n.clone()],
            None => {
                let children: HashSet<_> = g
                    .nodes
                    .iter()
                    .filter_map(|n| nodes.get(n))
                    .flat_map(|n| n.children.iter().map(|c| c.id()))
                    .collect();
                g.nodes
                    .iter()
                    .filter(|n| !children.contains(&n.id()))
                    .cloned()
                    .collect()
            }
        };
        let mut s = ShapeMesh::empty();
        // material names, for each primitive's footstep surface
        let mat_names: HashMap<AssetId<GltfMaterial>, &str> = g
            .named_materials
            .iter()
            .map(|(n, h)| (h.id(), n.as_ref()))
            .collect();
        let mut stack: Vec<(Handle<GltfNode>, Mat4)> =
            roots.into_iter().map(|n| (n, Mat4::IDENTITY)).collect();
        let mut ready = true;
        while let Some((nh, parent)) = stack.pop() {
            let Some(n) = nodes.get(&nh) else {
                ready = false;
                break;
            };
            // hinged doors swing open as the explorer walks up: never solid (the wall opening
            // behind one is the way in); nor are chest lids (and what hangs from them), opened or shut,
            // nor lift cars and their doors (`model_render::lift` carries the explorer on them)
            if n.extras.as_ref().is_some_and(|x| {
                crate::model_render::door::parse_door(&x.value).is_some()
                    || crate::model_render::lid::parse_lid(&x.value).is_some()
                    || crate::model_render::lift::not_solid(&x.value)
            }) {
                continue;
            }
            let tf = parent * n.transform.to_matrix();
            if let Some(mh) = &n.mesh {
                let Some(gm) = gmeshes.get(mh) else {
                    ready = false;
                    break;
                };
                for p in &gm.primitives {
                    let name = p
                        .material
                        .as_ref()
                        .and_then(|h| mat_names.get(&h.id()).copied());
                    let extras = p.material_extras.as_ref().map(|x| x.value.as_str());
                    if material_passable(name, extras) {
                        continue;
                    }
                    let surf = surf_code(material_surface(name, extras));
                    match meshes.get(&p.mesh) {
                        Some(m) => ShapeMesh::from_mesh(m, tf, None, surf, &mut s),
                        None => ready = false,
                    }
                }
            }
            stack.extend(n.children.iter().map(|c| (c.clone(), tf)));
        }
        if !ready {
            continue;
        }
        out.loading.remove(&id);
        match s.finish() {
            Some(s) => {
                out.meshes.insert((id, 0), Arc::new(s));
            }
            None => {
                out.missing.insert((id, 0));
            }
        }
    }
}

/// What a collider was built from: asset, variant, position, rotation and scale (bit patterns).
type PlacedKey = (String, u32, [u32; 8], Option<[u32; 3]>);

fn key_of(p: &Placed) -> PlacedKey {
    let f = |x: f32| x.to_bits();
    (
        p.asset.clone(),
        p.variant,
        [
            f(p.pos[0]),
            f(p.pos[1]),
            f(p.pos[2]),
            f(p.rot[0]),
            f(p.rot[1]),
            f(p.rot[2]),
            f(p.rot[3]),
            f(p.scale),
        ],
        p.stretch.map(|k| k.map(f)),
    )
}

/// One placed object's shape: a world-aligned grid of (bottom, top) height columns.
pub struct ObjCollider {
    pub id: u64,
    pub asset: String,
    pub surface: Surface,
    /// a walk-in building (its stairs are meant to be climbed)
    pub walk_in: bool,
    origin: Vec2,
    cell: f32,
    w: usize,
    h: usize,
    /// per cell: lowest and highest point; empty cells have bottom > top
    cols: Vec<(f32, f32)>,
    /// per cell: the footstep surface of its top (NO_SURF: the object's own)
    top_surf: Vec<u8>,
    /// walk-in buildings: each cell's solid (bottom, top) spans, `spans.1[spans.0[c]..spans.0[c + 1]]`,
    /// and the surface of each span's top
    spans: Option<(Vec<u32>, Vec<(f32, f32)>, Vec<u8>)>,
    pub min: Vec3,
    pub max: Vec3,
}

impl ObjCollider {
    fn build(p: &Placed, surface: Surface, walk_in: bool, shape: &ShapeMesh) -> Option<Self> {
        let t0 = std::time::Instant::now();
        let tf = Mat4::from_scale_rotation_translation(p.scale3(), p.rotation(), p.translation());
        let world: Vec<Vec3> = shape.pos.iter().map(|q| tf.transform_point3(*q)).collect();
        let min = world.iter().fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
        let max = world.iter().fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
        // too low to matter (a plate, a pebble): walk over it
        if max.y - min.y < 0.06 {
            return None;
        }
        let ext = (max.x - min.x).max(max.z - min.z);
        let cell = if walk_in {
            (ext / WALK_IN_CELLS).clamp(0.12, 0.3)
        } else {
            (ext / MAX_CELLS).clamp(0.12, 0.6)
        };
        let origin = Vec2::new(min.x, min.z) - Vec2::splat(cell);
        let w = (((max.x - min.x) / cell).ceil() as usize + 2).max(1);
        let h = (((max.z - min.z) / cell).ceil() as usize + 2).max(1);
        let mut c = ObjCollider {
            id: p.id,
            asset: p.asset.clone(),
            surface,
            walk_in,
            origin,
            cell,
            w,
            h,
            cols: vec![(f32::MAX, f32::MIN); w * h],
            top_surf: vec![NO_SURF; w * h],
            spans: None,
            min,
            max,
        };
        let mut frags = walk_in.then(Vec::new);
        for (ti, t) in shape.tris.iter().enumerate() {
            let s = shape.surf.get(ti).copied().unwrap_or(NO_SURF);
            c.raster(
                [
                    world[t[0] as usize],
                    world[t[1] as usize],
                    world[t[2] as usize],
                ],
                s,
                0,
                &mut frags,
            );
        }
        if let Some(mut f) = frags {
            // per cell, overlapping (or nearly touching) pieces become one solid span
            f.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            let mut start = vec![0u32; w * h + 1];
            let mut spans: Vec<(f32, f32)> = Vec::with_capacity(f.len() / 4);
            let mut ssurf: Vec<u8> = Vec::with_capacity(f.len() / 4);
            let mut last = usize::MAX;
            for (k, lo, hi, su) in f {
                let k = k as usize;
                match spans.last_mut() {
                    Some(s) if k == last && lo <= s.1 + SPAN_GAP => {
                        // the span's top is the piece that reaches highest
                        if hi >= s.1 && (su != NO_SURF || hi > s.1 + 0.05) {
                            *ssurf.last_mut().unwrap() = su;
                        }
                        s.1 = s.1.max(hi);
                    }
                    _ => {
                        spans.push((lo, hi));
                        ssurf.push(su);
                        start[k + 1] += 1;
                    }
                }
                last = k;
            }
            for k in 0..w * h {
                start[k + 1] += start[k];
            }
            info!(
                "colliders: walk-in {} ({} triangles) as {}×{} columns of {} spans in {:?}",
                p.asset,
                shape.tris.len(),
                w,
                h,
                spans.len(),
                t0.elapsed()
            );
            c.spans = Some((start, spans, ssurf));
        }
        Some(c)
    }

    /// Marks every cell a triangle covers with its height range (and, for walk-in buildings, notes
    /// the piece in `frags`); triangles wider than a cell are split first (so a sloping face records
    /// its true height per cell, a wall its full height).
    fn raster(
        &mut self,
        t: [Vec3; 3],
        surf: u8,
        depth: u32,
        frags: &mut Option<Vec<(u32, f32, f32, u8)>>,
    ) {
        let lo = t[0].min(t[1]).min(t[2]);
        let hi = t[0].max(t[1]).max(t[2]);
        if (hi.x - lo.x).max(hi.z - lo.z) > self.cell && depth < 14 {
            // split the longest edge (in plan)
            let e = [(0, 1), (1, 2), (2, 0)];
            let (a, b) = e
                .into_iter()
                .max_by(|x, y| {
                    t[x.0]
                        .xz()
                        .distance_squared(t[x.1].xz())
                        .total_cmp(&t[y.0].xz().distance_squared(t[y.1].xz()))
                })
                .unwrap();
            let c = 3 - a - b;
            let m = (t[a] + t[b]) * 0.5;
            self.raster([t[a], m, t[c]], surf, depth + 1, frags);
            self.raster([m, t[b], t[c]], surf, depth + 1, frags);
            return;
        }
        let i0 = (((lo.x - self.origin.x) / self.cell).floor() as isize)
            .clamp(0, self.w as isize - 1) as usize;
        let i1 = (((hi.x - self.origin.x) / self.cell).floor() as isize)
            .clamp(0, self.w as isize - 1) as usize;
        let j0 = (((lo.z - self.origin.y) / self.cell).floor() as isize)
            .clamp(0, self.h as isize - 1) as usize;
        let j1 = (((hi.z - self.origin.y) / self.cell).floor() as isize)
            .clamp(0, self.h as isize - 1) as usize;
        for j in j0..=j1 {
            for i in i0..=i1 {
                let k = j * self.w + i;
                let c = &mut self.cols[k];
                // a top without a surface of its own (a decal) only counts when it stands clear
                if hi.y >= c.1 && (surf != NO_SURF || hi.y > c.1 + 0.05) {
                    self.top_surf[k] = surf;
                }
                c.0 = c.0.min(lo.y);
                c.1 = c.1.max(hi.y);
                if let Some(f) = frags {
                    f.push((k as u32, lo.y, hi.y, surf));
                }
            }
        }
    }

    /// Solid pieces of the cells overlapping a disc, with their squares: (min, max, bottom, top);
    /// one per cell, or each span of a walk-in building's cell.
    fn cells_in(&self, c: Vec2, r: f32) -> impl Iterator<Item = (Vec2, Vec2, f32, f32)> + '_ {
        self.cells_in_s(c, r)
            .map(|(lo, hi, b, t, _)| (lo, hi, b, t))
    }

    /// Like cells_in, with the surface of each piece's top (NO_SURF: the object's own).
    fn cells_in_s(&self, c: Vec2, r: f32) -> impl Iterator<Item = (Vec2, Vec2, f32, f32, u8)> + '_ {
        let i0 = (((c.x - r - self.origin.x) / self.cell).floor() as isize).max(0) as usize;
        let i1 =
            (((c.x + r - self.origin.x) / self.cell).floor() as isize).min(self.w as isize - 1);
        let j0 = (((c.y - r - self.origin.y) / self.cell).floor() as isize).max(0) as usize;
        let j1 =
            (((c.y + r - self.origin.y) / self.cell).floor() as isize).min(self.h as isize - 1);
        let (i1, j1) = (i1.max(-1), j1.max(-1));
        (j0 as isize..=j1)
            .flat_map(move |j| (i0 as isize..=i1).map(move |i| (i as usize, j as usize)))
            .flat_map(move |(i, j)| {
                let k = j * self.w + i;
                let lo = self.origin + Vec2::new(i as f32, j as f32) * self.cell;
                let hi = lo + Vec2::splat(self.cell);
                // only cells the disc actually touches
                let q = c.clamp(lo, hi);
                let touched = q.distance_squared(c) <= r * r && self.cols[k].0 <= self.cols[k].1;
                let (pieces, surfs): (&[(f32, f32)], &[u8]) = match &self.spans {
                    _ if !touched => (&[], &[]),
                    Some((start, spans, ss)) => {
                        let r = start[k] as usize..start[k + 1] as usize;
                        (&spans[r.clone()], &ss[r])
                    }
                    None => (
                        std::slice::from_ref(&self.cols[k]),
                        std::slice::from_ref(&self.top_surf[k]),
                    ),
                };
                pieces
                    .iter()
                    .zip(surfs)
                    .map(move |(&(b, t), &s)| (lo, hi, b, t, s))
            })
    }

    fn overlaps(&self, c: Vec2, r: f32, y0: f32, y1: f32) -> bool {
        c.x + r >= self.min.x
            && c.x - r <= self.max.x
            && c.y + r >= self.min.z
            && c.y - r <= self.max.z
            && y1 >= self.min.y
            && y0 <= self.max.y
    }

    /// Every solid piece of the collider: `f(cell min, cell max, bottom, top, surface of its top)`,
    /// one per cell, or each span of a walk-in building's cell (the bounce light rasterises them
    /// into the occupancy its rays march through, `crate::bounce_light`).
    pub fn pieces(&self, mut f: impl FnMut(Vec2, Vec2, f32, f32, Surface)) {
        let surf = |s: u8| {
            Surface::ALL
                .get(s as usize)
                .copied()
                .unwrap_or(self.surface)
        };
        for j in 0..self.h {
            for i in 0..self.w {
                let k = j * self.w + i;
                if self.cols[k].0 > self.cols[k].1 {
                    continue;
                }
                let lo = self.origin + Vec2::new(i as f32, j as f32) * self.cell;
                let hi = lo + Vec2::splat(self.cell);
                match &self.spans {
                    Some((start, spans, ss)) => {
                        for r in start[k] as usize..start[k + 1] as usize {
                            f(lo, hi, spans[r].0, spans[r].1, surf(ss[r]));
                        }
                    }
                    None => f(
                        lo,
                        hi,
                        self.cols[k].0,
                        self.cols[k].1,
                        surf(self.top_surf[k]),
                    ),
                }
            }
        }
    }
}

/// A character's body for collision: a vertical capsule standing on its feet.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub radius: f32,
    pub height: f32,
    /// highest ledge it walks up without jumping
    pub step: f32,
}

impl Body {
    pub const WALKER: Body = Body {
        radius: 0.32,
        height: 1.8,
        step: 0.45,
    };
    pub const CROUCHED: Body = Body {
        radius: 0.32,
        height: 1.15,
        step: 0.4,
    };
    pub const RIDER: Body = Body {
        radius: 0.6,
        height: 2.5,
        step: 0.6,
    };
}

/// What happened while resolving a move (for footsteps, logs and tests).
#[derive(Clone, Debug, Default)]
pub struct Contact {
    /// pushed out of this object (id, asset)
    pub blocked_by: Option<(u64, String)>,
    /// the push direction (plan)
    pub normal: Vec2,
}

/// Cached colliders of the objects near whoever asked.
#[derive(Resource, Default)]
pub struct Colliders {
    /// per object: the placement it was looked at for, and its collider (None: walk-through)
    objects: HashMap<u64, (PlacedKey, Option<Arc<ObjCollider>>)>,
    /// the same for procedural trees, rocks and logs near whoever asked (`crate::nature`)
    procedural: HashMap<u64, (PlacedKey, Option<Arc<ObjCollider>>)>,
    /// walk-in buildings' colliders being built on the async compute pool
    pending: HashMap<u64, (PlacedKey, Task<Option<ObjCollider>>)>,
    /// how many were built (stats / tests)
    pub built: usize,
    /// objects bigger than `REACH` (a landmark's plaza, a kilometre-high tower): the object grid
    /// finds things by their origin, which can be far further than `REACH` from where you stand on
    /// them, so these are always looked at
    big: HashSet<u64>,
}

impl Colliders {
    /// Builds (or refreshes) the colliders of every solid object within `r` of `c`; at most
    /// `budget` new ones (the rest follow next frame). Returns how many are still missing.
    pub fn ensure(
        &mut self,
        c: Vec2,
        r: f32,
        store: &ObjectStore,
        lib: &Library,
        meshes: &mut ColliderMeshes,
        server: &AssetServer,
        budget: usize,
    ) -> usize {
        self.ensure_with(c, r, store, lib, meshes, server, budget, false)
    }

    /// `ensure`, with every collider built off the main thread when `background` (at most
    /// `budget` started per call): for callers that look further out than a walker's next step
    /// (the bounce light's ring round the walker, `crate::bounce_light`).
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_with(
        &mut self,
        c: Vec2,
        r: f32,
        store: &ObjectStore,
        lib: &Library,
        meshes: &mut ColliderMeshes,
        server: &AssetServer,
        budget: usize,
        background: bool,
    ) -> usize {
        let mut left = budget;
        let mut missing = 0;
        let objects = &mut self.objects;
        self.pending
            .retain(|id, (pk, task)| match block_on(future::poll_once(task)) {
                Some(o) => {
                    objects.insert(*id, (pk.clone(), o.map(Arc::new)));
                    false
                }
                None => true,
            });
        let mut ids = store.query(c, r + REACH);
        let extra: Vec<u64> = self.big.iter().copied().filter(|id| !ids.contains(id)).collect();
        ids.extend(extra);
        for id in ids {
            let Some(p) = store.items.get(&id) else {
                continue;
            };
            let pk = key_of(p);
            if self.objects.get(&id).is_some_and(|(k, _)| *k == pk) {
                continue;
            }
            let Some(e) = lib.get(&p.asset) else { continue };
            if e.radius * p.scale > REACH {
                self.big.insert(id);
            }
            if !is_solid(e) {
                self.objects.insert(id, (pk, None));
                continue;
            }
            // is it near enough to need it now? (big objects reach further)
            if Vec2::new(p.pos[0], p.pos[2]).distance(c) > r + e.radius * p.scale * 2.0 + 2.0 {
                continue;
            }
            let key = (p.asset.clone(), p.variant);
            let shape = match meshes.get(&p.asset, p.variant) {
                Some(s) => s.clone(),
                None => {
                    if meshes.missing.contains(&key)
                        || meshes.missing.contains(&(p.asset.clone(), 0))
                    {
                        self.objects.insert(id, (pk, None));
                    } else if let Some(path) = &e.import_path {
                        meshes.loading.entry(p.asset.clone()).or_insert_with(|| {
                            server.load(path.split('#').next().unwrap_or(path).to_string())
                        });
                        missing += 1;
                    } else {
                        self.objects.insert(id, (pk, None));
                    }
                    continue;
                }
            };
            let walk_in = e.walk_in();
            if walk_in || background {
                // tens of thousands of columns: built in the background (no hitch walking up)
                if !self.pending.get(&id).is_some_and(|(k, _)| *k == pk) {
                    if !walk_in {
                        if left == 0 {
                            missing += 1;
                            continue;
                        }
                        left -= 1;
                    }
                    let (p, surface) = (p.clone(), e.surface);
                    self.built += 1;
                    self.pending.insert(
                        id,
                        (
                            pk,
                            AsyncComputeTaskPool::get().spawn(async move {
                                ObjCollider::build(&p, surface, walk_in, &shape)
                            }),
                        ),
                    );
                }
                missing += 1;
                continue;
            }
            if left == 0 {
                missing += 1;
                continue;
            }
            left -= 1;
            self.built += 1;
            self.objects.insert(
                id,
                (
                    pk,
                    ObjCollider::build(p, e.surface, false, &shape).map(Arc::new),
                ),
            );
        }
        // forget far ones now and then
        if self.objects.len() > 4000 {
            self.objects.retain(|id, _| {
                store
                    .items
                    .get(id)
                    .is_some_and(|p| Vec2::new(p.pos[0], p.pos[2]).distance(c) < 300.0)
            });
        }
        missing
    }

    /// Builds (or refreshes) the colliders of the procedural instances given (those near `c`,
    /// as placed objects), like `ensure`; forgets the ones no longer given.
    pub fn ensure_procedural(
        &mut self,
        c: Vec2,
        r: f32,
        near: &[Placed],
        lib: &Library,
        meshes: &ColliderMeshes,
        budget: usize,
    ) {
        let mut left = budget;
        for p in near {
            let pk = key_of(p);
            if self.procedural.get(&p.id).is_some_and(|(k, _)| *k == pk) {
                continue;
            }
            let Some(e) = lib.get(&p.asset) else { continue };
            if !is_solid(e) {
                self.procedural.insert(p.id, (pk, None));
                continue;
            }
            if Vec2::new(p.pos[0], p.pos[2]).distance(c) > r + e.radius * p.scale * 2.0 + 2.0
                || left == 0
            {
                continue;
            }
            let Some(shape) = meshes.get(&p.asset, p.variant) else {
                self.procedural.insert(p.id, (pk, None));
                continue;
            };
            left -= 1;
            self.built += 1;
            self.procedural.insert(
                p.id,
                (
                    pk,
                    ObjCollider::build(p, e.surface, e.walk_in(), shape).map(Arc::new),
                ),
            );
        }
        if self.procedural.len() > near.len() + 64 {
            let keep: HashSet<u64> = near.iter().map(|p| p.id).collect();
            self.procedural.retain(|id, _| keep.contains(id));
        }
    }

    /// Ready colliders that could touch a disc of radius `r` around `c`.
    pub fn near(&self, store: &ObjectStore, c: Vec2, r: f32) -> Vec<Arc<ObjCollider>> {
        let mut ids = store.query(c, r + REACH);
        let extra: Vec<u64> = self.big.iter().copied().filter(|id| !ids.contains(id)).collect();
        ids.extend(extra);
        ids.into_iter()
            .filter_map(|id| self.objects.get(&id).and_then(|(_, o)| o.clone()))
            .chain(self.procedural.values().filter_map(|(_, o)| o.clone()))
            .filter(|o| o.overlaps(c, r, f32::MIN, f32::MAX))
            .collect()
    }

    /// The top of every solid piece of the ready colliders over a square (centre `c`, half size
    /// `r`): `f(min, max, top)` per cell piece (rain splashes land on whatever is on top).
    pub fn tops(&self, store: &ObjectStore, c: Vec2, r: f32, mut f: impl FnMut(Vec2, Vec2, f32)) {
        let reach = r * std::f32::consts::SQRT_2;
        for o in self.near(store, c, reach) {
            for (lo, hi, _, top) in o.cells_in(c, reach) {
                f(lo, hi, top);
            }
        }
    }

    /// Is this object (with a ready collider) a walk-in building?
    pub fn walk_in(&self, id: u64) -> bool {
        self.objects
            .get(&id)
            .and_then(|(_, o)| o.as_ref())
            .is_some_and(|o| o.walk_in)
    }

    /// The highest object surface under a disc (feet at `feet`) that a body can stand on: tops
    /// no higher than `feet + reach`. Returns (height, surface, object id).
    pub fn floor(
        &self,
        store: &ObjectStore,
        feet: Vec3,
        r: f32,
        reach: f32,
    ) -> Option<(f32, Surface, u64)> {
        let mut best: Option<(f32, Surface, u64)> = None;
        let c = feet.xz();
        for o in self.near(store, c, r) {
            for (_, _, _, top, su) in o.cells_in_s(c, r) {
                if top <= feet.y + reach && best.is_none_or(|b| top > b.0) {
                    let surface = Surface::ALL.get(su as usize).copied().unwrap_or(o.surface);
                    best = Some((top, surface, o.id));
                }
            }
        }
        best
    }

    /// Is there something solid overhead at `p` (a roof or ceiling within `up` metres above, over
    /// a disc of radius `r`)?
    pub fn covered(&self, store: &ObjectStore, p: Vec3, r: f32, up: f32) -> bool {
        let c = p.xz();
        self.near(store, c, r).iter().any(|o| {
            o.cells_in(c, r)
                .any(|(_, _, b, _)| b > p.y + 0.05 && b < p.y + up)
        })
    }

    /// How far a ball of radius `r` gets from `from` toward `to` before it touches something big
    /// and solid (a building's walls, floors and ceilings, a boulder; not trunks, posts or props
    /// under `min_size` across): 0..=1 of the way. Starting inside something, it counts from where
    /// it first comes clear. The game camera stays inside rooms with it
    /// (`crate::characters::play_camera`).
    pub fn sweep(&self, store: &ObjectStore, from: Vec3, to: Vec3, r: f32, min_size: f32) -> f32 {
        let d = to - from;
        let len = d.length();
        if len < 1e-3 {
            return 1.0;
        }
        let mid = (from + to) * 0.5;
        let near: Vec<_> = self
            .near(store, mid.xz(), len * 0.5 + r)
            .into_iter()
            .filter(|o| (o.max.x - o.min.x).max(o.max.z - o.min.z) >= min_size)
            .collect();
        if near.is_empty() {
            return 1.0;
        }
        let steps = (len / 0.08).ceil().max(1.0) as usize;
        let mut clear = None;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = from + d * t;
            let c = p.xz();
            let hit = near.iter().any(|o| {
                o.overlaps(c, r, p.y - r, p.y + r)
                    && o.cells_in(c, r).any(|(_, _, b, top)| b < p.y + r && top > p.y - r)
            });
            match (hit, clear) {
                (true, Some(last)) => return last,
                (false, _) => clear = Some(t),
                _ => {}
            }
        }
        1.0
    }

    /// The bounds (min, max) of what roofs `p` (a building's upper floor or roof, a bridge): a
    /// solid piece over a disc of radius `r` whose underside is `from`..`up` metres above it.
    pub fn roof_over(
        &self,
        store: &ObjectStore,
        p: Vec3,
        r: f32,
        from: f32,
        up: f32,
    ) -> Option<(Vec3, Vec3)> {
        let c = p.xz();
        self.near(store, c, r)
            .into_iter()
            .find(|o| {
                o.cells_in(c, r)
                    .any(|(_, _, b, _)| b > p.y + from && b < p.y + up)
            })
            .map(|o| (o.min, o.max))
    }

    /// Would a body standing at `feet` overlap something solid (beyond what it steps onto)?
    pub fn blocked(&self, store: &ObjectStore, feet: Vec3, body: Body) -> bool {
        let c = feet.xz();
        self.near(store, c, body.radius).iter().any(|o| {
            o.cells_in(c, body.radius)
                .any(|(_, _, b, t)| t > feet.y + body.step && b < feet.y + body.height)
        })
    }

    /// Where a body dropped at `feet` into something solid should stand: the lowest surface above
    /// its feet with room for it (a boulder's top; inside a building, the floor it was dropped
    /// onto, not the roof).
    pub fn lift(&self, store: &ObjectStore, feet: Vec3, body: Body) -> Option<f32> {
        let c = feet.xz();
        let mut tops: Vec<f32> = self
            .near(store, c, body.radius)
            .iter()
            .flat_map(|o| o.cells_in(c, body.radius).map(|p| p.3).collect::<Vec<_>>())
            .filter(|t| *t >= feet.y)
            .collect();
        tops.sort_by(f32::total_cmp);
        tops.into_iter()
            .find(|&t| !self.blocked(store, Vec3::new(feet.x, t, feet.z), body))
    }

    /// Pushes a body out of whatever it overlaps (deepest overlap first) so it slides along
    /// walls, trunks and rock faces.
    pub fn resolve(&self, store: &ObjectStore, feet: Vec3, body: Body) -> (Vec3, Contact) {
        let mut p = feet;
        let mut contact = Contact::default();
        let near = self.near(store, p.xz(), body.radius + 1.0);
        if near.is_empty() {
            return (p, contact);
        }
        for _ in 0..6 {
            let c = p.xz();
            let mut deepest: Option<(f32, Vec2, &ObjCollider)> = None;
            for o in &near {
                if !o.overlaps(c, body.radius, p.y, p.y + body.height) {
                    continue;
                }
                for (lo, hi, b, t) in o.cells_in(c, body.radius) {
                    if t <= p.y + body.step || b >= p.y + body.height {
                        continue;
                    }
                    let q = c.clamp(lo, hi);
                    let d = c - q;
                    let dist = d.length();
                    let (n, pen) = if dist > 1e-4 {
                        (d / dist, body.radius - dist)
                    } else {
                        // centre inside the cell: out through the nearest side
                        let mid = (lo + hi) * 0.5;
                        let dir = (c - mid).normalize_or(Vec2::X);
                        (dir, body.radius + (hi.x - lo.x) * 0.5)
                    };
                    if pen > 0.0 && deepest.is_none_or(|x| pen > x.0) {
                        deepest = Some((pen, n, o));
                    }
                }
            }
            let Some((pen, n, o)) = deepest else { break };
            p += Vec3::new(n.x, 0.0, n.y) * (pen + 1e-3);
            if contact.blocked_by.is_none() {
                contact.blocked_by = Some((o.id, o.asset.clone()));
            }
            contact.normal = (contact.normal + n).normalize_or(n);
        }
        (p, contact)
    }

    /// A cheap plan-view footprint of a solid object (centre, radius, height) from its collider,
    /// or from the library's spacing radius when it has none yet: for world-wide walkability.
    pub fn footprint(
        &self,
        store: &ObjectStore,
        lib: &Library,
        id: u64,
    ) -> Option<(Vec2, f32, f32)> {
        let p = store.items.get(&id)?;
        let e = lib.get(&p.asset)?;
        if !is_solid(e) {
            return None;
        }
        if let Some((_, Some(o))) = self.objects.get(&id) {
            let c = (o.min.xz() + o.max.xz()) * 0.5;
            // the column cells at foot level give the true base (a tree's trunk, not its crown)
            let base = p.pos[1] + 1.0;
            let mut r: f32 = 0.0;
            for j in 0..o.h {
                for i in 0..o.w {
                    let (b, t) = o.cols[j * o.w + i];
                    if b <= base && t > base - 0.6 {
                        let q = o.origin + (Vec2::new(i as f32, j as f32) + 0.5) * o.cell;
                        r = r.max(q.distance(c) + o.cell * 0.5);
                    }
                }
            }
            return Some((c, r, o.max.y - p.pos[1]));
        }
        let trunk = matches!(e.category, Category::Trees);
        Some((
            Vec2::new(p.pos[0], p.pos[2]),
            if trunk {
                0.4 * p.scale
            } else {
                e.radius * p.scale * 0.8
            },
            e.height * p.scale,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 × 4 m room: a floor slab (0–0.1 m), a ceiling slab (3.0–3.1 m) and one wall (x = 2).
    fn room() -> ShapeMesh {
        let mut s = ShapeMesh::empty();
        let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            let i = s.pos.len() as u32;
            s.pos.extend([a, b, c, d]);
            s.tris.extend([[i, i + 1, i + 2], [i, i + 2, i + 3]]);
            s.surf.extend([NO_SURF; 2]);
        };
        for y in [0.0, 0.1, 3.0, 3.1] {
            quad(
                Vec3::new(-2.0, y, -2.0),
                Vec3::new(2.0, y, -2.0),
                Vec3::new(2.0, y, 2.0),
                Vec3::new(-2.0, y, 2.0),
            );
        }
        quad(
            Vec3::new(2.0, 0.0, -2.0),
            Vec3::new(2.0, 3.1, -2.0),
            Vec3::new(2.0, 3.1, 2.0),
            Vec3::new(2.0, 0.0, 2.0),
        );
        s.finish().unwrap()
    }

    fn placed() -> Placed {
        Placed {
            group: 0,
            id: 1,
            asset: "room".into(),
            variant: 0,
            pos: [0.0; 3],
            rot: Quat::IDENTITY.to_array(),
            scale: 1.0,
            stretch: None,
            locked: false,
            glow: None,
        }
    }

    /// A walk-in building keeps the room between its floor and ceiling; anything else is a solid
    /// block from its lowest to its highest point.
    #[test]
    fn walk_in_columns_keep_rooms() {
        let shape = room();
        let walk_in = ObjCollider::build(&placed(), Surface::Wood, true, &shape).unwrap();
        let pieces: Vec<(f32, f32)> = walk_in
            .cells_in(Vec2::ZERO, 0.01)
            .map(|p| (p.2, p.3))
            .collect();
        assert!(
            !pieces.is_empty()
                && pieces
                    .iter()
                    .all(|p| p.1 <= 0.1 + 1e-4 || p.0 >= 3.0 - 1e-4),
            "{pieces:?}"
        );
        assert!(
            pieces.iter().any(|p| (p.1 - 0.1).abs() < 1e-4)
                && pieces.iter().any(|p| (p.0 - 3.0).abs() < 1e-4),
            "{pieces:?}"
        );
        // the wall's columns are solid floor to ceiling
        assert!(
            walk_in
                .cells_in(Vec2::new(2.0, 0.0), 0.01)
                .any(|p| p.2 <= 0.0 + 1e-4 && p.3 >= 3.1 - 1e-4)
        );
        let block = ObjCollider::build(&placed(), Surface::Wood, false, &shape).unwrap();
        let pieces: Vec<(f32, f32)> = block
            .cells_in(Vec2::ZERO, 0.01)
            .map(|p| (p.2, p.3))
            .collect();
        assert!(
            pieces.iter().all(|p| p.0 <= 1e-4 && p.1 >= 3.1 - 1e-4),
            "{pieces:?}"
        );
    }

    /// Footsteps: the surface of whatever is on top (a rug on a stone floor is carpet), the
    /// object's own surface where the material says nothing.
    #[test]
    fn floors_sound_like_their_material() {
        let mut s = ShapeMesh::empty();
        let mut quad = |y: f32, x0: f32, x1: f32, surf: u8| {
            let i = s.pos.len() as u32;
            s.pos.extend([
                Vec3::new(x0, y, -2.0),
                Vec3::new(x1, y, -2.0),
                Vec3::new(x1, y, 2.0),
                Vec3::new(x0, y, 2.0),
            ]);
            s.tris.extend([[i, i + 1, i + 2], [i, i + 2, i + 3]]);
            s.surf.extend([surf; 2]);
        };
        quad(0.1, -2.0, 2.0, surf_code(Some(Surface::Stone)));
        quad(0.12, -0.5, 0.5, surf_code(Some(Surface::Carpet)));
        quad(0.1, 2.0, 3.0, NO_SURF);
        // a moss decal 1 cm over the stone keeps it stone
        quad(0.11, -2.0, -1.0, NO_SURF);
        // and a post, so the object stands taller than the walk-over limit
        let i = s.pos.len() as u32;
        s.pos.extend([
            Vec3::new(3.0, 0.0, -0.1),
            Vec3::new(3.0, 1.0, -0.1),
            Vec3::new(3.0, 1.0, 0.1),
        ]);
        s.tris.push([i, i + 1, i + 2]);
        s.surf.push(NO_SURF);
        let shape = s.finish().unwrap();
        for walk_in in [false, true] {
            let o = ObjCollider::build(&placed(), Surface::Sand, walk_in, &shape).unwrap();
            let at = |x: f32| {
                o.cells_in_s(Vec2::new(x, 0.0), 0.01)
                    .map(|p| p.4)
                    .max()
                    .unwrap()
            };
            assert_eq!(Surface::ALL.get(at(-1.5) as usize), Some(&Surface::Stone));
            assert_eq!(Surface::ALL.get(at(0.0) as usize), Some(&Surface::Carpet));
            assert_eq!(at(2.6), NO_SURF);
        }
    }

    #[test]
    fn material_names_map_to_surfaces() {
        let m = |n: &str| material_surface(Some(n), None);
        assert_eq!(m("planks"), Some(Surface::Wood));
        assert_eq!(m("beam_light"), Some(Surface::Wood));
        assert_eq!(m("gstone"), Some(Surface::Stone));
        assert_eq!(m("stone_wall_blocks"), Some(Surface::Stone));
        assert_eq!(m("cobble"), Some(Surface::Cobble));
        assert_eq!(m("canvas_sway"), Some(Surface::Carpet));
        assert_eq!(m("sand"), Some(Surface::Sand));
        assert_eq!(m("siegebreaker_dirt"), Some(Surface::Dirt));
        assert_eq!(m("grass_ground"), Some(Surface::Grass));
        assert_eq!(m("iron_grey"), Some(Surface::Metal));
        assert_eq!(m("bone"), Some(Surface::Bone));
        assert_eq!(m("glow"), None);
        assert_eq!(m("moss"), None);
        assert_eq!(m("Material.001"), None);
        assert_eq!(
            material_surface(Some("planks"), Some(r#"{"surface": "carpet"}"#)),
            Some(Surface::Carpet)
        );
        // a model's water surface is waded through, not stood on
        assert!(material_passable(Some("water"), None));
        assert!(material_passable(Some("ferry_river_water"), None));
        assert!(material_passable(Some("mist"), Some(r#"{"solid": false}"#)));
        assert!(!material_passable(Some("planks"), None));
    }

    #[test]
    fn known_giant_tree_radius_is_not_clipped() {
        assert_eq!(known_trunk_radius(2.45), Some(2.45));
        assert_eq!(known_trunk_radius(0.01), Some(0.05));
        assert_eq!(known_trunk_radius(f32::NAN), None);
    }

    #[test]
    fn bamboo_bark_collision_blocks_culms_without_filling_the_gap() {
        let mut positions = vec![];
        let mut indices = vec![];
        for cx in [-1.0, 1.0] {
            let base = positions.len() as u32;
            for y in [-0.2, TREE_CUT + 1.0] {
                for (x, z) in [(-0.25, -0.25), (0.25, -0.25), (0.25, 0.25), (-0.25, 0.25)] {
                    positions.push([cx + x, y, z]);
                }
            }
            for side in 0..4u32 {
                let next = (side + 1) % 4;
                indices.extend_from_slice(&[
                    base + side,
                    base + 4 + side,
                    base + next,
                    base + next,
                    base + 4 + side,
                    base + 4 + next,
                ]);
            }
        }
        let mut mesh = Mesh::new(
            bevy::mesh::PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_indices(Indices::U32(indices));
        let shape = bamboo_collision_shape(&mesh).expect("culm bark collision");
        let collider = ObjCollider::build(&placed(), Surface::Wood, false, &shape)
            .expect("rasterized bamboo collision");

        assert!(
            collider.cells_in(Vec2::ZERO, 0.10).next().is_none(),
            "the open space between bamboo culms must remain walkable"
        );
        assert!(
            collider
                .cells_in(Vec2::new(-0.75, 0.0), 0.10)
                .next()
                .is_some(),
            "a bamboo culm must still block movement"
        );
    }
}
