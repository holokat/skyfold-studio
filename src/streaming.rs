//! Streams objects around the viewer. Only objects close enough to be drawn exist as render
//! entities: trees out to their last mesh LOD, plants ~100 m, rocks by size, buildings to the draw
//! distance (`objects::stream_dist`). Everything else costs nothing per frame. Distant trees are
//! billboards merged into one mesh per 128 m chunk: a forest on the horizon is a few hundred
//! entities instead of tens of thousands. A per-vertex distance test in the billboard shader
//! hands over from each tree's last mesh LOD with the same dithered cross-fade Bevy uses between
//! LODs, so the seam is invisible.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::VisibilityRange;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use crate::camera::MainCamera;
use crate::objects::{CHUNK, GRID, Library, ObjectStore, stream_dist};

/// Extra distance (m) before a streamed-out object comes back / after it leaves before it goes.
pub const SPAWN_MARGIN: f32 = 12.0;
const DESPAWN_MARGIN: f32 = 40.0;

/// How far the world is drawn: a per-machine quality setting, saved with the world and exported
/// so a game can expose the same knobs.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Graphics {
    /// How far (m) anything is drawn: tree billboards, big rocks, buildings.
    pub draw_distance: f32,
    /// Multiplies how far full-detail trees, plants and rocks reach (0.5–2).
    pub detail: f32,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            draw_distance: 3000.0,
            detail: 1.0,
        }
    }
}

impl Graphics {
    /// Quick presets (walk mode cycles them with V): name, draw distance, detail.
    pub const PRESETS: [(&'static str, f32, f32); 3] = [
        ("Close", 900.0, 0.6),
        ("Medium", 3000.0, 1.0),
        ("Far", 7000.0, 1.6),
    ];

    pub fn preset_name(&self) -> &'static str {
        Self::PRESETS
            .iter()
            .find(|p| (p.1 - self.draw_distance).abs() < 1.0 && (p.2 - self.detail).abs() < 0.01)
            .map(|p| p.0)
            .unwrap_or("Custom")
    }
    /// Next preset (from a custom setting: the nearest larger one).
    pub fn cycle(&mut self) {
        let next = match Self::PRESETS.iter().position(|p| {
            (p.1 - self.draw_distance).abs() < 1.0 && (p.2 - self.detail).abs() < 0.01
        }) {
            Some(i) => (i + 1) % Self::PRESETS.len(),
            None => Self::PRESETS
                .iter()
                .position(|p| p.1 > self.draw_distance)
                .unwrap_or(0),
        };
        (self.draw_distance, self.detail) = (Self::PRESETS[next].1, Self::PRESETS[next].2);
    }
}

pub type BillboardMaterial = ExtendedMaterial<StandardMaterial, BillboardExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct BillboardExt {
    /// x..y: fade in (m, after the trees' last mesh LOD), z..w: fade out at the draw distance
    #[uniform(100)]
    pub ranges: Vec4,
}

impl MaterialExtension for BillboardExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/billboard.wesl".into()
    }
    /// The prepass uses the same distance fade and collapse as the colour pass.
    fn enable_prepass() -> bool {
        true
    }
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/billboard_prepass.wesl".into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        "shaders/forest_prepass.wesl".into()
    }
}

pub struct StreamingPlugin;

impl Plugin for StreamingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<BillboardMaterial>::default())
            .init_resource::<Graphics>()
            .init_resource::<Streamer>()
            .init_resource::<Chunks>()
            .add_systems(
                PostUpdate,
                (stream, build_chunks).before(crate::objects::ObjectSync),
            );
    }
}

#[derive(Resource)]
struct Streamer {
    last_cam: Vec3,
    last_graphics: Graphics,
    timer: f32,
    /// Grid cells still to check for objects coming into range, farthest first (a pass is spread
    /// over a few frames: a 16 km world has millions of objects in reach of the draw distance).
    sweep: Vec<(i32, i32)>,
}

impl Default for Streamer {
    fn default() -> Self {
        Self {
            last_cam: Vec3::splat(f32::MAX),
            last_graphics: Graphics::default(),
            timer: 0.0,
            sweep: vec![],
        }
    }
}

/// Streams objects in and out as the viewer moves (a full pass every 8 m of movement; the
/// spawn checks run nearest cells first within a few milliseconds per frame).
#[allow(clippy::too_many_arguments)]
fn stream(
    mut st: ResMut<Streamer>,
    mut store: ResMut<ObjectStore>,
    lib: Option<Res<Library>>,
    graphics: Res<Graphics>,
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    children: Query<&Children>,
    mut bands: Query<&mut VisibilityRange>,
) {
    let (Some(lib), Ok(cam)) = (lib, cam.single()) else {
        return;
    };
    let p = cam.translation();
    st.timer += time.delta_secs();
    let moved = p.distance(st.last_cam) > 8.0;
    let settings = *graphics != st.last_graphics;
    if !moved && !settings && st.timer < 2.0 {
        spawn_slice(&mut st, &mut store, &lib, &graphics, p);
        return;
    }
    st.timer = 0.0;
    st.last_cam = p;
    let old = std::mem::replace(&mut st.last_graphics, graphics.clone());
    let g = &*graphics;
    if settings {
        retune(&mut store, &lib, &old, g, &children, &mut bands);
    }

    // out of range: despawn
    let gone: Vec<u64> = store
        .entities
        .keys()
        .copied()
        .filter(|id| {
            let Some(o) = store.items.get(id) else {
                return true;
            };
            let Some(e) = lib.get(&o.asset) else {
                return true;
            };
            o.translation().distance(p)
                > stream_dist(e, g, crate::objects::size_band(e, o.scale)) + DESPAWN_MARGIN
        })
        .collect();
    for id in gone {
        if let Some(e) = store.entities.remove(&id) {
            store.to_despawn.push(e);
        }
    }
    // in range: spawn (a new sweep over the cells in reach, nearest first)
    let reach = g.draw_distance.max(400.0) + SPAWN_MARGIN;
    let half_cell = GRID * 0.75;
    let mut cells: Vec<(f32, (i32, i32))> = store
        .cells()
        .filter_map(|(cell, _)| {
            let d = Vec2::new((cell.0 as f32 + 0.5) * GRID, (cell.1 as f32 + 0.5) * GRID)
                .distance(p.xz())
                - half_cell;
            (d <= reach).then_some((d, *cell))
        })
        .collect();
    cells.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
    st.sweep = cells.into_iter().map(|c| c.1).collect();
    spawn_slice(&mut st, &mut store, &lib, g, p);
}

/// New graphics settings (walk mode's V cycles them): the streamed objects' LOD bands follow in
/// place. (Despawning every streamed object and streaming it back in was a long hitch, with
/// gaps while thousands of objects came back.) An object that gains or loses its far LOD child
/// is respawned; objects now out of reach go, newly in reach come, as when moving.
fn retune(
    store: &mut ObjectStore,
    lib: &Library,
    old: &Graphics,
    new: &Graphics,
    children: &Query<&Children>,
    bands: &mut Query<&mut VisibilityRange>,
) {
    let mut respawn = vec![];
    for (id, e) in &store.entities {
        let Some((o, entry)) = store
            .items
            .get(id)
            .and_then(|o| Some((o, lib.get(&o.asset)?)))
        else {
            continue;
        };
        let size = crate::objects::size_band(entry, o.scale);
        let (Some(before), Some(after)) = (
            crate::objects::lod_ranges(entry, old, size),
            crate::objects::lod_ranges(entry, new, size),
        ) else {
            continue;
        };
        if before == after {
            continue;
        }
        if after.len() != before.len() {
            respawn.push(*id);
            continue;
        }
        for c in children.get(*e).into_iter().flatten() {
            if let Ok(mut r) = bands.get_mut(*c)
                && let Some(k) = before.iter().position(|b| *b == *r)
            {
                *r = after[k].clone();
            }
        }
    }
    store.to_spawn.extend(respawn);
}

/// Checks the next cells of the sweep (nearest first, ~3 ms) and queues their objects in range.
fn spawn_slice(st: &mut Streamer, store: &mut ObjectStore, lib: &Library, g: &Graphics, p: Vec3) {
    if st.sweep.is_empty() {
        return;
    }
    let t0 = std::time::Instant::now();
    let mut want: Vec<(f32, u64)> = vec![];
    let mut checked = 0;
    while let Some(cell) = st.sweep.pop() {
        for id in store.cell(cell).map(|v| v.as_slice()).unwrap_or_default() {
            if store.entities.contains_key(id) {
                continue;
            }
            let Some(o) = store.items.get(id) else {
                continue;
            };
            let Some(e) = lib.get(&o.asset) else { continue };
            let d = o.translation().distance(p);
            if d <= stream_dist(e, g, crate::objects::size_band(e, o.scale)) + SPAWN_MARGIN {
                want.push((d, *id));
            }
        }
        checked += 1;
        if checked % 64 == 0 && t0.elapsed().as_secs_f32() > 0.003 {
            break;
        }
    }
    want.sort_by(|a, b| a.0.total_cmp(&b.0));
    let queued: std::collections::HashSet<u64> = store.to_spawn.iter().copied().collect();
    for (_, id) in want {
        if !queued.contains(&id) {
            store.to_spawn.push(id);
        }
    }
}

/// Merged billboard meshes of the distant trees, one entity per chunk.
#[derive(Resource, Default)]
struct Chunks {
    map: HashMap<(i32, i32), Entity>,
    material: Option<Handle<BillboardMaterial>>,
    ranges: Vec4,
}

fn build_chunks(
    mut commands: Commands,
    mut chunks: ResMut<Chunks>,
    mut store: ResMut<ObjectStore>,
    lib: Option<Res<Library>>,
    atlas: Option<Res<crate::trees::ImpostorAtlas>>,
    graphics: Res<Graphics>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<BillboardMaterial>>,
) {
    let (Some(lib), Some(atlas)) = (lib, atlas) else {
        return;
    };
    chunks
        .material
        .get_or_insert_with(|| atlas.material.clone());
    // hand-over distances: after the trees' last mesh LOD, out to the draw distance
    let far_lod = lib
        .entries
        .iter()
        .filter_map(|e| e.tree.as_ref())
        .filter(|t| t.ranges[3] > t.ranges[2])
        .map(|t| t.ranges[2])
        .fold(0.0f32, f32::max)
        * graphics.detail;
    let dd = graphics.draw_distance;
    let ranges = Vec4::new(far_lod * 0.9, far_lod, dd * 0.9, dd);
    if ranges != chunks.ranges {
        chunks.ranges = ranges;
        if let Some(mut m) = mats.get_mut(&atlas.material) {
            m.extension.ranges = ranges;
        }
        // chunk entities carry a coarse range too (whole chunk beyond the draw distance)
        let all: Vec<(i32, i32)> = chunks.map.keys().copied().collect();
        store.changed_chunks.extend(all);
    }
    if store.changed_chunks.is_empty() {
        return;
    }
    // a few chunks per frame: a new world fills in over a second or two
    let todo: Vec<(i32, i32)> = store.changed_chunks.iter().copied().take(24).collect();
    for c in todo {
        store.changed_chunks.remove(&c);
        if let Some(e) = chunks.map.remove(&c) {
            commands.entity(e).despawn();
        }
        let centre = Vec3::new((c.0 as f32 + 0.5) * CHUNK, 0.0, (c.1 as f32 + 0.5) * CHUNK);
        let mut b = Batch::default();
        for id in store.in_chunk(c) {
            let Some(o) = store.items.get(&id) else {
                continue;
            };
            let Some(tree) = lib.get(&o.asset).and_then(|e| e.tree.as_ref()) else {
                continue;
            };
            if tree.ranges[3] <= tree.ranges[2] {
                continue;
            }
            let v = &tree.variants[(o.variant as usize).min(tree.variants.len() - 1)];
            let Some(cell) = v.cell else { continue };
            let tf = Transform {
                translation: o.translation() - centre,
                rotation: o.rotation(),
                scale: o.scale3(),
            };
            crate::trees::billboard_quads(
                &mut b.pos, &mut b.nor, &mut b.uv, &mut b.idx, v.width, v.min_y, v.height, cell,
                &tf,
            );
            let n = b.pos.len() - b.origin.len();
            let origin = o.translation();
            b.origin
                .extend(std::iter::repeat_n([origin.x, origin.z], n));
            b.origin_y
                .extend(std::iter::repeat_n([origin.y, 0.0, 0.0, 1.0], n));
        }
        if b.pos.is_empty() {
            continue;
        }
        let mut m = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        m.insert_attribute(Mesh::ATTRIBUTE_POSITION, b.pos);
        m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, b.nor);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uv);
        m.insert_attribute(Mesh::ATTRIBUTE_UV_1, b.origin);
        m.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.origin_y);
        m.insert_indices(Indices::U32(b.idx));
        // the chunk vanishes as a whole once its nearest corner is beyond the draw distance; the
        // shader fades individual trees before that
        let reach = dd + CHUNK * 0.75;
        let range = VisibilityRange {
            start_margin: 0.0..0.0,
            end_margin: reach..reach + 1.0,
            use_aabb: false,
        };
        let e = commands
            .spawn((
                Name::new("Tree billboards"),
                Mesh3d(meshes.add(m)),
                MeshMaterial3d(atlas.material.clone()),
                Transform::from_translation(centre),
                range,
                NotShadowCaster,
                MeshTag::new(crate::forest_contours::FOREST_TAG),
            ))
            .id();
        chunks.map.insert(c, e);
    }
}

#[derive(Default)]
struct Batch {
    pos: Vec<[f32; 3]>,
    nor: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    idx: Vec<u32>,
    origin: Vec<[f32; 2]>,
    origin_y: Vec<[f32; 4]>,
}
