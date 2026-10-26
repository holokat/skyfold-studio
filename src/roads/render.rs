//! Drawing the roads: the surface material (`shaders/road.wesl`) with its photoscanned textures
//! (loaded in the background the first time a kind is used), the ribbon chunks, the bridges and
//! the walkable bridge decks. Visuals follow the road data, the ground under it and the water:
//! they are rebuilt when a road changes (edit, undo, load), when the terrain under a road changes
//! (sculpting) or when the water changes (a new river under a road gets a bridge).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::plan::RoadPlan;
use super::{RoadKind, Roads};
use crate::terrain::Terrain;
use crate::terrain::layers::Surface;
use crate::water::WaterBodies;

pub type RoadMaterial = ExtendedMaterial<StandardMaterial, RoadExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default, PartialEq)]
pub struct RoadParams {
    /// x: texture repeat (m), y: edge softness (m), z: edge breakup (m), w: cart ruts
    pub surface: Vec4,
    /// x: normal strength, y: texture softening (mip bias), z: rain wetness, w: depth pull per m
    pub look: Vec4,
    /// xyz: direction towards the sun, w: cloud shadow strength
    pub sun: Vec4,
    /// x: cloud coverage, y: cloud scale, zw: cloud offset
    pub clouds: Vec4,
    /// x: sea level, y: anti-tiling
    pub misc: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct RoadExt {
    #[uniform(100)]
    pub params: RoadParams,
    #[texture(101)]
    #[sampler(102)]
    pub albedo: Handle<Image>,
    #[texture(103)]
    pub detail: Handle<Image>,
}

impl MaterialExtension for RoadExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/road.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/road.wesl".into()
    }
}

/// One material per road kind (textures arrive in the background).
#[derive(Resource, Default)]
pub struct RoadMaterials {
    pub kinds: Vec<Handle<RoadMaterial>>,
    /// texture state per kind: 0 not requested, 1 loading, 2 ready, 3 failed
    state: [u8; 6],
    tiles: [f32; 6],
    /// kinds whose textures should start loading
    queue: Vec<usize>,
    /// the surface style (`art_style::surfaces`) the textures are loaded in
    art: crate::art_style::ArtStyle,
}

/// Texture loads in flight: (kind, the style they load in, task).
#[derive(Resource, Default)]
struct TextureTasks(
    Vec<(
        usize,
        crate::art_style::ArtStyle,
        Task<Result<(Image, Image), String>>,
    )>,
);

/// A built road's entities and what they were built from.
struct Vis {
    key: u64,
    heights: u64,
    ribbons: Vec<Entity>,
    bridges: Vec<Entity>,
}

#[derive(Resource, Default)]
pub struct RoadVisuals {
    roads: HashMap<u32, Vis>,
    water_sig: u64,
    height_rev: u64,
    ground_rev: u64,
    /// The latest plans (rank order), for the inspector and the bridges.
    pub plans: Vec<Arc<RoadPlan>>,
}

/// Marks the road surface and bridge entities.
#[derive(Component)]
pub struct RoadMesh;

/// A walkable bridge deck: a path (point, height, direction) and its half width.
#[derive(Clone, Debug)]
pub struct Deck {
    pub pts: Vec<(Vec2, f32)>,
    pub hw: f32,
    pub lo: Vec2,
    pub hi: Vec2,
    pub surface: Surface,
}

/// A road's centreline for footstep sounds.
#[derive(Clone, Debug)]
pub struct SurfacePath {
    pub pts: Vec<Vec2>,
    pub hw: f32,
    pub lo: Vec2,
    pub hi: Vec2,
    pub surface: Surface,
}

/// Floors walk mode can stand on besides the terrain (bridge decks), and the road surfaces
/// (footsteps).
#[derive(Resource, Default)]
pub struct RoadFloors {
    pub decks: Vec<Deck>,
    /// rank order: later ones win where roads overlap
    pub roads: Vec<SurfacePath>,
}

impl RoadFloors {
    /// The deck under someone standing at `p` (within a step up or a short drop), if any.
    pub fn floor_at(&self, p: Vec3) -> Option<(f32, Surface)> {
        let q = p.xz();
        // Adjacent authored approach/span strips share endpoints. Pick the closest eligible
        // segment globally rather than returning the first strip whose AABB happens to contain
        // the point, otherwise a bridge endpoint can shadow its higher/lower approach.
        let mut hit: Option<(f32, f32, Surface)> = None;
        for d in &self.decks {
            if q.x < d.lo.x || q.y < d.lo.y || q.x > d.hi.x || q.y > d.hi.y {
                continue;
            }
            let mut best: Option<(f32, f32)> = None;
            for w in d.pts.windows(2) {
                let (a, b) = (w[0].0, w[1].0);
                let ab = b - a;
                let u = ((q - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let dist = q.distance(a + ab * u);
                if dist <= d.hw && best.is_none_or(|x| dist < x.0) {
                    best = Some((dist, w[0].1 + (w[1].1 - w[0].1) * u));
                }
            }
            if let Some((dist, y)) = best
                && y <= p.y + 1.4
                && y >= p.y - 1.8
            {
                if hit.is_none_or(|current| dist < current.0) {
                    hit = Some((dist, y, d.surface));
                }
            }
        }
        hit.map(|(_, y, surface)| (y, surface))
    }

    /// The surface of the road at `p` (footsteps), if it is on one.
    pub fn surface_at(&self, p: Vec3) -> Option<Surface> {
        let q = p.xz();
        self.roads.iter().rev().find_map(|r| {
            if q.x < r.lo.x || q.y < r.lo.y || q.x > r.hi.x || q.y > r.hi.y {
                return None;
            }
            r.pts
                .windows(2)
                .any(|w| {
                    let ab = w[1] - w[0];
                    let u = ((q - w[0]).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                    q.distance(w[0] + ab * u) <= r.hw
                })
                .then_some(r.surface)
        })
    }
}

pub fn build(app: &mut App) {
    app.add_plugins(MaterialPlugin::<RoadMaterial>::default())
        .init_resource::<RoadMaterials>()
        .init_resource::<TextureTasks>()
        .init_resource::<RoadVisuals>()
        .init_resource::<RoadFloors>()
        .add_systems(Startup, init_materials)
        .add_systems(
            Update,
            (finish_textures, sync_visuals, update_params)
                .chain()
                .after(crate::tools::ToolSystems::Commit),
        )
        .add_systems(PostUpdate, lamp_ranges);
    super::bridge::build(app);
}

fn placeholder(srgb: bool) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if srgb {
            vec![120, 110, 100, 128]
        } else {
            vec![128, 128, 230, 255]
        },
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(repeat_sampler());
    img
}

fn repeat_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    }
}

fn init_materials(
    mut mats: ResMut<RoadMaterials>,
    mut materials: ResMut<Assets<RoadMaterial>>,
    mut images: ResMut<Assets<Image>>,
    library: Res<crate::terrain::textures::TerrainLibrary>,
) {
    let (a, d) = (
        images.add(placeholder(true)),
        images.add(placeholder(false)),
    );
    for (k, kind) in RoadKind::ALL.iter().enumerate() {
        let st = kind.style();
        let tile = library
            .materials
            .iter()
            .find(|m| m.id == st.texture)
            .map(|m| m.tile_m)
            .unwrap_or(2.0);
        mats.tiles[k] = tile;
        let ext = RoadExt {
            params: RoadParams {
                surface: Vec4::new(tile, st.edge, st.breakup, st.ruts),
                look: Vec4::new(1.0, 0.0, 0.0, 0.0022),
                sun: Vec4::new(0.3, 0.8, 0.2, 0.0),
                clouds: Vec4::new(0.3, 1.0, 0.0, 0.0),
                misc: Vec4::new(0.0, 0.6, 0.0, 0.0),
            },
            albedo: a.clone(),
            detail: d.clone(),
        };
        let base = StandardMaterial {
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.8,
            reflectance: 0.4,
            ..default()
        };
        mats.kinds.push(materials.add(ExtendedMaterial {
            base,
            extension: ext,
        }));
    }
}

/// Loads one kind's surface (albedo + height, normal + roughness + AO) with full mip chains, in
/// the world's art style (the painted library material in a hand-painted world).
fn load_surface(id: &str, art: crate::art_style::ArtStyle) -> Result<(Image, Image), String> {
    let f = |name: &str| crate::terrain::textures::library_file(id, name);
    let [albedo, normal] = crate::art_style::textures([f("albedo.png"), f("normal.png")], art);
    let a = image::open(albedo)
        .map_err(|e| format!("{id}/albedo.png: {e}"))?
        .to_rgba8();
    let d = image::open(normal)
        .map_err(|e| format!("{id}/normal.png: {e}"))?
        .to_rgba8();
    let size = a.width();
    if a.height() != size || !size.is_power_of_two() || d.width() != size || d.height() != size {
        return Err(format!("{id}: textures must be power-of-two squares"));
    }
    let lut = crate::terrain::textures::srgb_to_lin_lut();
    let make = |raw: &[u8], srgb: bool| {
        let data = crate::terrain::textures::build_mips(raw, size, srgb, &lut);
        let mut img = Image::new_uninit(
            Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            if srgb {
                TextureFormat::Rgba8UnormSrgb
            } else {
                TextureFormat::Rgba8Unorm
            },
            RenderAssetUsages::RENDER_WORLD,
        );
        img.data = Some(data);
        img.texture_descriptor.mip_level_count = size.ilog2() + 1;
        img.sampler = ImageSampler::Descriptor(repeat_sampler());
        img
    };
    Ok((make(a.as_raw(), true), make(d.as_raw(), false)))
}

impl RoadMaterials {
    /// The material of a kind, starting its texture load the first time.
    pub fn get(&mut self, kind: RoadKind) -> Handle<RoadMaterial> {
        let k = kind.index();
        if self.state[k] == 0 {
            self.state[k] = 1;
            self.queue.push(k);
        }
        self.kinds[k].clone()
    }
}

fn finish_textures(
    mut tasks: ResMut<TextureTasks>,
    mut mats: ResMut<RoadMaterials>,
    mut materials: ResMut<Assets<RoadMaterial>>,
    mut images: ResMut<Assets<Image>>,
    meta: Res<crate::world::WorldMeta>,
) {
    // the world's art style changed: the kinds already shown load again in the new style
    let art = crate::art_style::surfaces(meta.art_style);
    if art != mats.art {
        mats.art = art;
        for k in 0..mats.state.len() {
            if mats.state[k] >= 1 && !mats.queue.contains(&k) {
                mats.queue.push(k);
            }
        }
    }
    for k in std::mem::take(&mut mats.queue) {
        let id = RoadKind::ALL[k].style().texture;
        tasks.0.push((
            k,
            art,
            AsyncComputeTaskPool::get().spawn(async move { load_surface(id, art) }),
        ));
    }
    let mut i = 0;
    while i < tasks.0.len() {
        let Some(result) = block_on(future::poll_once(&mut tasks.0[i].2)) else {
            i += 1;
            continue;
        };
        let (k, loaded_art, _) = tasks.0.remove(i);
        if loaded_art != mats.art {
            // (an older style's load: the new one is queued)
            continue;
        }
        match result {
            Ok((a, d)) => {
                let (a, d) = (images.add(a), images.add(d));
                if let Some(mut m) = materials.get_mut(&mats.kinds[k]) {
                    m.extension.albedo = a;
                    m.extension.detail = d;
                }
                mats.state[k] = 2;
                info!("ROADS surface textures ready: {}", RoadKind::ALL[k].name());
            }
            Err(e) => {
                warn!(
                    "ROADS surface textures for {} unavailable: {e}",
                    RoadKind::ALL[k].name()
                );
                mats.state[k] = 3;
            }
        }
    }
}

/// Lighting-related parameters follow the terrain material (sun, clouds, rain, art style).
fn update_params(
    gpu: Option<Res<crate::terrain::material::TerrainGpu>>,
    terrain_mats: Res<Assets<crate::terrain::material::TerrainMaterial>>,
    mats: Res<RoadMaterials>,
    mut materials: ResMut<Assets<RoadMaterial>>,
    terrain: Res<Terrain>,
) {
    let Some(gpu) = gpu else { return };
    let Some(tm) = terrain_mats.get(&gpu.material) else {
        return;
    };
    let tp = &tm.extension.params;
    // puddles: the rain's (the terrain's `overlay_image.y`, see `rain`), or in a world styled
    // wet the standing water its ruts always held
    let style_wet = terrain.style.wetness;
    let puddles = tp.overlay_image.y.max(crate::rain::quantize(
        style_wet * crate::noise::smoothstep(0.5, 0.75, style_wet) * 0.6,
    ));
    for (k, h) in mats.kinds.iter().enumerate() {
        let st = RoadKind::ALL[k].style();
        let want = RoadParams {
            surface: Vec4::new(mats.tiles[k], st.edge, st.breakup, st.ruts),
            look: Vec4::new(tp.tex.z * 0.9, tp.tex.w, tp.style2.y, 0.0022),
            sun: tp.sun,
            clouds: tp.clouds,
            misc: Vec4::new(
                terrain.sea_level,
                if st.stone_bridge { 0.35 } else { 0.7 },
                puddles,
                tp.overlay_image.z,
            ),
        };
        let same = materials.get(h).is_some_and(|m| m.extension.params == want);
        if !same && let Some(mut m) = materials.get_mut(h) {
            m.extension.params = want;
        }
    }
}

/// A cheap signature of the water bodies (rebuild bridges when water appears under a road).
fn water_signature(w: &WaterBodies) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    w.level_map.len().hash(&mut h);
    for r in &w.rivers {
        (r.id, r.points.len(), r.width.to_bits()).hash(&mut h);
        if let Some(p) = r.points.first() {
            p[1].to_bits().hash(&mut h);
        }
    }
    for l in &w.lakes {
        (l.id, l.level.to_bits(), l.outline.len()).hash(&mut h);
    }
    h.finish()
}

/// Hash of terrain heights that can change a rendered ribbon or authored deck landing.
fn height_hash(plan: &RoadPlan, t: &Terrain) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (i, s) in plan.samples.iter().enumerate().step_by(2) {
        if plan.authored || plan.span_of(i).is_none() {
            ((super::mesh::surface_height(t, s.p.x, s.p.y) * 200.0).round() as i32).hash(&mut h);
        }
    }
    h.finish()
}

/// Walkable floor strips built from a road plan. Authored routes register only their visible
/// bridge spans and raised landings, while ordinary roads only register bridge decks.
pub(crate) fn floor_decks(plan: &RoadPlan) -> Vec<Deck> {
    let add_deck = |pts: Vec<(Vec2, f32)>, hw: f32, surface: Surface| {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for (p, _) in &pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        Deck {
            pts,
            hw,
            lo: lo - Vec2::splat(hw),
            hi: hi + Vec2::splat(hw),
            surface,
        }
    };
    if plan.authored {
        let range = |i0: usize, i1: usize, hw: f32, surface: Surface| {
            add_deck(
                (i0..=i1).map(|i| (plan.samples[i].p, plan.h[i])).collect(),
                hw,
                surface,
            )
        };
        let tag = |i: usize| {
            plan.span_of(i)
                .map(|span| if span.stone { 1 } else { 2 })
                .unwrap_or(0)
        };
        let surface = |tag| match tag {
            1 => Surface::Cobble,
            2 => Surface::Wood,
            _ => plan.kind.style().surface,
        };
        let mut decks = Vec::with_capacity(plan.spans.len() * 3);
        for (first, last) in plan.authored_deck_ranges() {
            let mut start = first;
            let mut current = tag(first);
            for i in first + 1..=last {
                let next = tag(i);
                if next != current {
                    if start < i {
                        decks.push(range(
                            start,
                            i,
                            plan.hw + if current == 2 { 0.1 } else { 0.0 },
                            surface(current),
                        ));
                    }
                    start = i;
                    current = next;
                }
            }
            if start < last {
                decks.push(range(
                    start,
                    last,
                    plan.hw + if current == 2 { 0.1 } else { 0.0 },
                    surface(current),
                ));
            }
        }
        return decks;
    }
    plan.spans
        .iter()
        .map(|sp| {
            add_deck(
                (sp.i0..=sp.i1)
                    .map(|i| (plan.samples[i].p, plan.h[i]))
                    .collect(),
                plan.hw + if sp.stone { 0.0 } else { 0.1 },
                if sp.stone {
                    Surface::Cobble
                } else {
                    Surface::Wood
                },
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn sync_visuals(
    mut commands: Commands,
    mut roads: ResMut<Roads>,
    terrain: Res<Terrain>,
    water: Res<WaterBodies>,
    stroke: Res<crate::tools::Stroke>,
    mut vis: ResMut<RoadVisuals>,
    mut mats: ResMut<RoadMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut floors: ResMut<RoadFloors>,
    bridge_mats: Res<super::bridge::BridgeMaterials>,
    mut tool: ResMut<super::RoadTool>,
) {
    let wsig = water_signature(&water);
    let height_changed = terrain.height_revision != vis.height_rev && !stroke.active;
    let replan = roads.dirty
        || wsig != vis.water_sig
        || roads.ground.revision != vis.ground_rev
        || (height_changed
            && (vis.plans.is_empty() || vis.plans.iter().any(|plan| plan.authored))
            && !roads.roads.is_empty());
    if !replan && !height_changed {
        return;
    }
    let t0 = std::time::Instant::now();
    let mut rebuilt = 0;
    roads.dirty = false;
    vis.water_sig = wsig;
    vis.ground_rev = roads.ground.revision;
    if !stroke.active {
        vis.height_rev = terrain.height_revision;
    }
    if replan {
        let base = super::ground::Base {
            t: &terrain,
            g: &roads.ground,
        };
        vis.plans = super::plan::plan_all(&roads, &base, &water)
            .into_iter()
            .map(Arc::new)
            .collect();
        tool.update_stats(&vis.plans);
        floors.roads = vis
            .plans
            .iter()
            .filter(|plan| !plan.authored)
            .map(|p| SurfacePath {
                pts: p.samples.iter().step_by(2).map(|s| s.p).collect(),
                hw: p.hw,
                lo: p.lo,
                hi: p.hi,
                surface: p.kind.style().surface,
            })
            .collect();
    }
    // drop roads that are gone
    let live: std::collections::HashSet<u32> = roads.roads.iter().map(|r| r.id).collect();
    let gone: Vec<u32> = vis
        .roads
        .keys()
        .filter(|id| !live.contains(id))
        .copied()
        .collect();
    for id in gone {
        if let Some(v) = vis.roads.remove(&id) {
            for e in v.ribbons.into_iter().chain(v.bridges) {
                commands.entity(e).despawn();
            }
        }
    }
    let mut decks = vec![];
    let plans = vis.plans.clone();
    for plan in &plans {
        let Some(road) = roads.roads.iter().find(|r| r.id == plan.id) else {
            continue;
        };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            road.def_hash().hash(&mut h);
            for sp in &plan.spans {
                (sp.i0, sp.i1, sp.deck.to_bits()).hash(&mut h);
            }
            plan.h.len().hash(&mut h);
            for v in plan.h.iter().step_by(4) {
                ((v * 100.0).round() as i32).hash(&mut h);
            }
            h.finish()
        };
        let heights = height_hash(plan, &terrain);
        decks.extend(floor_decks(plan));
        let entry = vis.roads.get(&plan.id);
        let rebuild_all = entry.is_none_or(|v| v.key != key);
        let rebuild_ribbon = rebuild_all || entry.is_some_and(|v| v.heights != heights);
        if !rebuild_ribbon {
            continue;
        }
        let material = mats.get(road.kind);
        let mut v = vis.roads.remove(&plan.id).unwrap_or(Vis {
            key: 0,
            heights: 0,
            ribbons: vec![],
            bridges: vec![],
        });
        for e in v.ribbons.drain(..) {
            commands.entity(e).despawn();
        }
        for (mesh, aabb) in super::mesh::ribbon_chunks(plan, &terrain, road.id) {
            let e = commands
                .spawn((
                    Name::new(format!("Road {} surface", road.id)),
                    RoadMesh,
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                    aabb,
                    NotShadowCaster,
                ))
                .id();
            v.ribbons.push(e);
        }
        if rebuild_all {
            for e in v.bridges.drain(..) {
                commands.entity(e).despawn();
            }
            for sp in &plan.spans {
                for (mesh, slot) in super::bridge::bridge_meshes(plan, sp, &terrain) {
                    let Some(aabb) = bevy::camera::primitives::MeshAabb::get_aabb(&mesh) else {
                        continue;
                    };
                    let e = commands
                        .spawn((
                            Name::new(format!("Road {} bridge", road.id)),
                            RoadMesh,
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(bridge_mats.get(slot)),
                            Transform::IDENTITY,
                            aabb,
                        ))
                        .id();
                    v.bridges.push(e);
                }
            }
        }
        v.key = key;
        v.heights = heights;
        vis.roads.insert(plan.id, v);
        rebuilt += 1;
    }
    floors.decks = decks;
    if rebuilt > 0 {
        info!(
            "ROADS visuals: {} roads planned, {rebuilt} rebuilt in {:?}",
            vis.plans.len(),
            t0.elapsed()
        );
    }
}

/// Lamp posts are detailed photoscans without LODs and a road can carry dozens: their meshes are
/// drawn (and cast shadows) only within ~200 m of the viewer.
fn lamp_ranges(
    mut commands: Commands,
    added: Query<Entity, Added<Mesh3d>>,
    parents: Query<&ChildOf>,
    placed: Query<&crate::objects::PlacedObject>,
    store: Res<crate::objects::ObjectStore>,
) {
    for e in &added {
        let mut cur = e;
        for _ in 0..8 {
            if let Ok(p) = placed.get(cur) {
                if store
                    .items
                    .get(&p.id)
                    .is_some_and(|o| o.asset == super::build::LAMP)
                {
                    commands
                        .entity(e)
                        .insert(bevy::camera::visibility::VisibilityRange {
                            start_margin: 0.0..0.0,
                            end_margin: 170.0..210.0,
                            use_aabb: false,
                        });
                }
                break;
            }
            match parents.get(cur) {
                Ok(c) => cur = c.parent(),
                Err(_) => break,
            }
        }
    }
}
