//! Distance LODs for library and imported glTF models (Poly Haven, Skyborn buildings, user
//! imports), which arrive as one detailed mesh. Once a placed model's scene has spawned, every
//! mesh of at least `MIN_TRIS` triangles gets up to two simplified levels, built once on the async
//! compute pool (`simplify`) and cached in the user's data folder (keyed like the thumbnail
//! cache: the mesh, its source file's size and date, the model size and `VERSION`). A level keeps
//! the source vertices, so it is the source mesh with fewer triangles: its error is bounded in
//! metres (from the model's size), and it takes over where that error is about a pixel
//! (`PX_K` × error; a hard switch: a pixel's change never shows). The near level keeps vertices
//! whose normals their neighbours can't stand in for, so displaced bumps shade as before; the far
//! level lets texture seams, hard edges and bumps go, gets normals from its own faces and drops
//! loose bits smaller than its error (nails, fittings, fur strands). Collision is not affected
//! (`motion::collide` reads the glTF itself).
//!
//! `WB_NOLOD=1` turns LODs off (A/B tests); `WB_LODLOG=1` logs, every 2 s, how many meshes have
//! LODs, how many of their triangles are drawn and how many textures got mips (`super::mips`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::VisibilityRange;
use bevy::mesh::{Indices, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bevy::world_serialization::WorldInstanceReady;

use super::simplify::simplify;
use crate::objects::{Library, ObjectStore, PlacedObject};

/// Bump when the simplifier or the level choice changes: every cached LOD is rebuilt.
const VERSION: u64 = 7;
/// Meshes with fewer triangles keep their single level.
const MIN_TRIS: usize = 3000;
/// A level shows from `PX_K` × its error (m) on: about 1.3 px on a 2000 px tall 60° view.
const PX_K: f32 = 1500.0;
/// A level must drop at least this share of the previous level's triangles to be kept.
const MIN_GAIN: f32 = 0.25;

/// A mesh entity of a placed model that has its LODs (the source level).
#[derive(Component)]
pub struct LodSource;

/// A simplified level spawned next to its source mesh entity.
#[derive(Component)]
pub struct LodLevel;

/// One built level: the mesh, its error (m, model space), triangle count and bounds.
struct Level {
    mesh: Mesh,
    error: f32,
    tris: usize,
    aabb: Aabb,
}

enum State {
    Building(Task<Vec<Level>>),
    /// (mesh, error, bounds) per level, nearest first
    Ready(Arc<Vec<(Handle<Mesh>, f32, Aabb)>>),
}

#[derive(Resource, Default)]
pub struct ModelLods {
    sets: HashMap<AssetId<Mesh>, State>,
    /// mesh entities of placed models waiting for their mesh's levels: (entity, mesh, model size,
    /// placed scale)
    waiting: Vec<(Entity, Handle<Mesh>, f32, f32)>,
    /// triangles per mesh (sources and levels), for stats
    tris: HashMap<AssetId<Mesh>, usize>,
}

/// A placed model's scene is in: its meshes queue for LODs.
pub fn on_scene_ready(
    ev: On<WorldInstanceReady>,
    placed: Query<&PlacedObject>,
    children: Query<&Children>,
    meshes: Query<&Mesh3d, (Without<LodSource>, Without<LodLevel>)>,
    store: Res<ObjectStore>,
    lib: Option<Res<Library>>,
    mut lods: ResMut<ModelLods>,
) {
    if std::env::var("WB_NOLOD").is_ok() {
        return;
    }
    let root = ev.event().entity;
    let Ok(po) = placed.get(root) else { return };
    let Some(p) = store.items.get(&po.id) else {
        return;
    };
    let Some(e) = lib.as_ref().and_then(|l| l.get(&p.asset)) else {
        return;
    };
    if e.import_path.is_none() {
        return;
    }
    let size = (e.radius / 0.55).max(e.height).max(0.05);
    for c in children.iter_descendants(root) {
        if let Ok(m) = meshes.get(c) {
            lods.waiting.push((c, m.0.clone(), size, p.scale));
        }
    }
}

/// Starts level builds, finishes them and gives waiting mesh entities their levels.
pub fn update(
    mut commands: Commands,
    mut lods: ResMut<ModelLods>,
    mut meshes: ResMut<Assets<Mesh>>,
    server: Res<AssetServer>,
    info: Query<(&ChildOf, &Transform, &MeshMaterial3d<StandardMaterial>), Without<LodLevel>>,
) {
    if lods.waiting.is_empty() && lods.sets.values().all(|s| matches!(s, State::Ready(_))) {
        return;
    }
    // finished builds
    let ModelLods { sets, tris, .. } = &mut *lods;
    for (id, s) in sets.iter_mut() {
        let State::Building(task) = s else { continue };
        let Some(levels) = block_on(future::poll_once(task)) else {
            continue;
        };
        let ready: Vec<(Handle<Mesh>, f32, Aabb)> = levels
            .into_iter()
            .map(|l| {
                let h = meshes.add(l.mesh);
                tris.insert(h.id(), l.tris);
                (h, l.error, l.aabb)
            })
            .collect();
        if !ready.is_empty() {
            info!(
                "lods: {} levels for {:?} ({} → {:?} triangles)",
                ready.len(),
                server
                    .get_path(*id)
                    .map(|p| p.to_string())
                    .unwrap_or_default(),
                tris.get(id).copied().unwrap_or(0),
                ready.iter().map(|r| tris[&r.0.id()]).collect::<Vec<_>>()
            );
        }
        *s = State::Ready(Arc::new(ready));
    }
    // waiting entities
    let waiting = std::mem::take(&mut lods.waiting);
    for (e, h, size, scale) in waiting {
        let Ok((parent, tf, mat)) = info.get(e) else {
            continue;
        };
        match lods.sets.get(&h.id()) {
            None => {
                let Some(src) = meshes.get(&h).and_then(Source::of) else {
                    lods.sets.insert(h.id(), State::Ready(Arc::new(vec![])));
                    continue;
                };
                lods.tris.insert(h.id(), src.tris.len());
                let key = server.get_path(h.id()).map(|p| cache_key(&p, size, &src));
                lods.sets.insert(
                    h.id(),
                    State::Building(
                        AsyncComputeTaskPool::get().spawn(async move { build(src, size, key) }),
                    ),
                );
                lods.waiting.push((e, h, size, scale));
            }
            Some(State::Building(_)) => lods.waiting.push((e, h, size, scale)),
            Some(State::Ready(levels)) => {
                if levels.is_empty() {
                    continue;
                }
                let at = |err: f32| quantize(err * PX_K * scale);
                // hard switches: a dithered cross-fade shows as a checkerboard without TAA
                let band = |a: f32, b: f32| VisibilityRange {
                    start_margin: a..a,
                    end_margin: b..b,
                    use_aabb: false,
                };
                let first = at(levels[0].1);
                commands.entity(e).insert((LodSource, band(0.0, first)));
                for (i, (lh, err, aabb)) in levels.iter().enumerate() {
                    let start = at(*err);
                    let end = levels.get(i + 1).map_or(1.0e9, |n| at(n.1));
                    commands.entity(parent.parent()).with_child((
                        Mesh3d(lh.clone()),
                        mat.clone(),
                        *tf,
                        *aabb,
                        band(start, end),
                        LodLevel,
                    ));
                }
            }
        }
    }
}

/// Distances on a quarter-octave grid: Bevy keeps one GPU entry per distinct range.
fn quantize(d: f32) -> f32 {
    (d.max(1.0).log2() * 4.0)
        .round()
        .exp2()
        .powf(0.25)
        .round()
        .max(1.0)
}

/// What a level build needs from the source mesh (all of it copied: the build runs elsewhere).
struct Source {
    attrs: Vec<(MeshVertexAttribute, VertexAttributeValues)>,
    tris: Vec<[u32; 3]>,
}

impl Source {
    fn of(m: &Mesh) -> Option<Source> {
        if m.primitive_topology() != PrimitiveTopology::TriangleList || m.has_morph_targets() {
            return None;
        }
        let attrs: Vec<(MeshVertexAttribute, VertexAttributeValues)> = m
            .try_attributes()
            .ok()?
            .map(|(a, v)| (a.clone(), v.clone()))
            .collect();
        if !attrs.iter().all(|(_, v)| {
            matches!(
                v,
                VertexAttributeValues::Float32x2(_)
                    | VertexAttributeValues::Float32x3(_)
                    | VertexAttributeValues::Float32x4(_)
            )
        }) {
            return None;
        }
        let n = attrs
            .iter()
            .find(|(a, _)| a.id == Mesh::ATTRIBUTE_POSITION.id)?
            .1
            .len() as u32;
        let idx: Vec<u32> = match m.try_indices_option().ok()? {
            Some(Indices::U16(i)) => i.iter().map(|&x| x as u32).collect(),
            Some(Indices::U32(i)) => i.clone(),
            None => (0..n).collect(),
        };
        let tris: Vec<[u32; 3]> = idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
        (tris.len() >= MIN_TRIS).then_some(Source { attrs, tris })
    }

    fn positions(&self) -> &[[f32; 3]] {
        match self
            .attrs
            .iter()
            .find(|(a, _)| a.id == Mesh::ATTRIBUTE_POSITION.id)
        {
            Some((_, VertexAttributeValues::Float32x3(p))) => p,
            _ => &[],
        }
    }

    fn normals(&self) -> &[[f32; 3]] {
        match self
            .attrs
            .iter()
            .find(|(a, _)| a.id == Mesh::ATTRIBUTE_NORMAL.id)
        {
            Some((_, VertexAttributeValues::Float32x3(n))) => n,
            _ => &[],
        }
    }

    /// Per vertex, a hash of everything but its position (what makes two corners one wedge).
    fn wedges(&self) -> Vec<u64> {
        let n = self.positions().len();
        let mut h = vec![0xcbf2_9ce4_8422_2325u64; n];
        let mut mix = |i: usize, v: &[f32]| {
            for x in v {
                h[i] = (h[i] ^ x.to_bits() as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for (a, v) in &self.attrs {
            if a.id == Mesh::ATTRIBUTE_POSITION.id {
                continue;
            }
            match v {
                VertexAttributeValues::Float32x2(v) => {
                    v.iter().enumerate().for_each(|(i, x)| mix(i, x))
                }
                VertexAttributeValues::Float32x3(v) => {
                    v.iter().enumerate().for_each(|(i, x)| mix(i, x))
                }
                VertexAttributeValues::Float32x4(v) => {
                    v.iter().enumerate().for_each(|(i, x)| mix(i, x))
                }
                _ => {}
            }
        }
        h
    }

    /// A level: the source's vertices that `s` keeps. The near level (`own_normals`) keeps the
    /// source normals (it never merged across a turn of them); the far level, which lets seams and
    /// creases go, gets normals from its own faces (smooth across faces within 50° of each other,
    /// split at sharper creases), or the source's bumps would streak across its big faces.
    fn level(&self, tris: &[[u32; 3]], error: f32, own_normals: bool) -> Level {
        let pos = self.positions();
        let fnorm: Vec<Vec3> = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| Vec3::from_array(pos[i as usize]));
                (b - a).cross(c - a)
            })
            .collect();
        let mut around: HashMap<[u32; 3], Vec<u32>> = HashMap::new();
        if !own_normals {
            for (ti, t) in tris.iter().enumerate() {
                for &i in t {
                    around
                        .entry(pos[i as usize].map(f32::to_bits))
                        .or_default()
                        .push(ti as u32);
                }
            }
        }
        // per corner: (source vertex, its new normal) -> new vertex
        let mut key: HashMap<(u32, [i32; 3]), u32> = HashMap::new();
        let mut used: Vec<(u32, [f32; 3])> = vec![];
        let mut idx: Vec<u32> = Vec::with_capacity(tris.len() * 3);
        for (ti, t) in tris.iter().enumerate() {
            let own = fnorm[ti].normalize_or_zero();
            for &i in t {
                if own_normals {
                    let v = *key.entry((i, [0; 3])).or_insert_with(|| {
                        used.push((i, [0.0; 3]));
                        (used.len() - 1) as u32
                    });
                    idx.push(v);
                    continue;
                }
                let mut n = Vec3::ZERO;
                for &tj in &around[&pos[i as usize].map(f32::to_bits)] {
                    let f = fnorm[tj as usize];
                    if own.dot(f.normalize_or_zero()) > 0.64 {
                        n += f;
                    }
                }
                let n = n.normalize_or(own).to_array();
                let q = n.map(|x| (x * 1024.0).round() as i32);
                let v = *key.entry((i, q)).or_insert_with(|| {
                    used.push((i, n));
                    (used.len() - 1) as u32
                });
                idx.push(v);
            }
        }
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (a, v) in &self.attrs {
            let picked = match v {
                _ if a.id == Mesh::ATTRIBUTE_NORMAL.id && !own_normals => {
                    VertexAttributeValues::Float32x3(used.iter().map(|u| u.1).collect())
                }
                VertexAttributeValues::Float32x2(v) => {
                    VertexAttributeValues::Float32x2(used.iter().map(|u| v[u.0 as usize]).collect())
                }
                VertexAttributeValues::Float32x3(v) => {
                    let p: Vec<[f32; 3]> = used.iter().map(|u| v[u.0 as usize]).collect();
                    if a.id == Mesh::ATTRIBUTE_POSITION.id {
                        for q in &p {
                            lo = lo.min(Vec3::from_array(*q));
                            hi = hi.max(Vec3::from_array(*q));
                        }
                    }
                    VertexAttributeValues::Float32x3(p)
                }
                VertexAttributeValues::Float32x4(v) => {
                    VertexAttributeValues::Float32x4(used.iter().map(|u| v[u.0 as usize]).collect())
                }
                _ => continue,
            };
            mesh.insert_attribute(a.clone(), picked);
        }
        mesh.insert_indices(if used.len() <= u16::MAX as usize {
            Indices::U16(idx.iter().map(|&i| i as u16).collect())
        } else {
            Indices::U32(idx)
        });
        Level {
            mesh,
            error,
            tris: tris.len(),
            aabb: Aabb::from_min_max(lo, hi),
        }
    }
}

fn cache_dir() -> PathBuf {
    crate::paths::cache_dir("lods")
}

/// FNV-1a over the mesh's asset path, its source file's size and date, the model size, the mesh's
/// counts and `VERSION`.
fn cache_key(path: &bevy::asset::AssetPath, size: f32, src: &Source) -> u64 {
    let file = crate::asset_file(&path.to_string());
    let stamp = std::fs::metadata(&file)
        .ok()
        .map(|m| {
            (
                m.len(),
                m.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs()),
            )
        })
        .unwrap_or((0, 0));
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let name = path.to_string();
    let parts: [&[u8]; 6] = [
        name.as_bytes(),
        &stamp.0.to_le_bytes(),
        &stamp.1.to_le_bytes(),
        &size.to_bits().to_le_bytes(),
        &(src.tris.len() as u64).to_le_bytes(),
        &VERSION.to_le_bytes(),
    ];
    for p in parts {
        for b in p {
            h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The levels' errors (m): a near one about 0.4 % of the model's size, a far one 6× that.
fn errors(size: f32) -> [f32; 2] {
    let e = (0.004 * size).clamp(0.003, 0.06);
    [e, e * 6.0]
}

/// Builds (or reads from the cache) the levels of one mesh.
fn build(src: Source, size: f32, key: Option<u64>) -> Vec<Level> {
    let t0 = std::time::Instant::now();
    let file = key.map(|k| cache_dir().join(format!("{k:016x}.bin")));
    if let Some(levels) = file.as_ref().and_then(|f| read_cache(f, &src, size)) {
        return levels;
    }
    let pos = src.positions().to_vec();
    let wedge = src.wedges();
    let [e1, e2] = errors(size);
    // the far level ignores texture seams and hard edges (sub-pixel there) and drops loose bits
    // under its error
    let shape_only = vec![0u64; pos.len()];
    // the near level keeps every vertex whose normal its neighbours can't stand in for within 10°
    // (displaced bumps shade the same; smooth bevels still go)
    let normals = src.normals();
    let near_normals = (!normals.is_empty()).then_some((normals, 10f32.to_radians().cos()));
    let mut out: Vec<(f32, Vec<[u32; 3]>)> = vec![];
    let mut prev = src.tris.len();
    for (err, prune, keys, n) in [(e1, 0.0, &wedge, near_normals), (e2, e2, &shape_only, None)] {
        let t = simplify(&pos, keys, &src.tris, err, prune, n);
        if (t.len() as f32) < prev as f32 * (1.0 - MIN_GAIN) && !t.is_empty() {
            prev = t.len();
            out.push((err, t));
        }
    }
    if let Some(f) = &file {
        write_cache(f, &out);
    }
    debug!(
        "lods: {} → {:?} triangles in {:?}",
        src.tris.len(),
        out.iter().map(|o| o.1.len()).collect::<Vec<_>>(),
        t0.elapsed()
    );
    out.iter()
        .map(|(e, t)| src.level(t, *e, *e <= e1))
        .collect()
}

/// `WBLOD1`, level count, then per level: error (f32), triangle count (u32), indices (u32 × 3n).
fn write_cache(f: &std::path::Path, levels: &[(f32, Vec<[u32; 3]>)]) {
    let mut b: Vec<u8> = b"WBLOD1".to_vec();
    b.extend_from_slice(&(levels.len() as u32).to_le_bytes());
    for (e, t) in levels {
        b.extend_from_slice(&e.to_le_bytes());
        b.extend_from_slice(&(t.len() as u32).to_le_bytes());
        for i in t.iter().flatten() {
            b.extend_from_slice(&i.to_le_bytes());
        }
    }
    let _ = std::fs::create_dir_all(cache_dir());
    let tmp = f.with_extension("tmp");
    if std::fs::write(&tmp, b).is_ok() {
        let _ = std::fs::rename(&tmp, f);
    }
}

fn read_cache(f: &std::path::Path, src: &Source, size: f32) -> Option<Vec<Level>> {
    let b = std::fs::read(f).ok()?;
    let rd = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    if b.get(..6)? != b"WBLOD1" {
        return None;
    }
    let n = src.positions().len() as u32;
    let mut o = 10;
    let mut out = vec![];
    for _ in 0..rd(6)? {
        let err = f32::from_bits(rd(o)?);
        let count = rd(o + 4)? as usize;
        o += 8;
        let mut tris = Vec::with_capacity(count);
        for k in 0..count {
            let tri = [rd(o + k * 12)?, rd(o + k * 12 + 4)?, rd(o + k * 12 + 8)?];
            if tri.iter().any(|&i| i >= n) {
                return None;
            }
            tris.push(tri);
        }
        o += count * 12;
        out.push(src.level(&tris, err, err <= errors(size)[0]));
    }
    Some(out)
}

/// `WB_LODLOG=1`: meshes with LODs and the triangles of theirs drawn now (by level).
pub fn log(
    time: Res<Time>,
    mut next: Local<f32>,
    lods: Res<ModelLods>,
    mips: Res<super::mips::MipJobs>,
    src: Query<(&Mesh3d, &ViewVisibility), With<LodSource>>,
    lvl: Query<(&Mesh3d, &ViewVisibility), With<LodLevel>>,
) {
    if std::env::var("WB_LODLOG").is_err() || time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 2.0;
    let sum = |it: &mut dyn Iterator<Item = (&Mesh3d, &ViewVisibility)>| {
        it.filter(|(_, v)| v.get())
            .map(|(m, _)| lods.tris.get(&m.0.id()).copied().unwrap_or(0))
            .sum::<usize>()
    };
    let (s, l) = (sum(&mut src.iter()), sum(&mut lvl.iter()));
    let all: usize = src
        .iter()
        .map(|(m, _)| lods.tris.get(&m.0.id()).copied().unwrap_or(0))
        .sum();
    let building = lods
        .sets
        .values()
        .filter(|s| matches!(s, State::Building(_)))
        .count();
    info!(
        "LODLOG meshes with levels {} (building {building}), sources {}, levels {}; triangles drawn: full {s} + simplified {l} = {} (all at full detail: {all})",
        lods.sets.len(),
        src.iter().count(),
        lvl.iter().count(),
        s + l
    );
    let (n, bytes, secs) = mips.stats;
    info!(
        "LODLOG mips: {n} textures, +{:.1} MB, {secs:.2} s of worker time",
        bytes as f64 / 1e6
    );
}
