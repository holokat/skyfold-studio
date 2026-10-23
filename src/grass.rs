//! Dense meadow grass around the camera, after tidewater's GrassField.
//!
//! Geometric blades (no textures): one shared patch mesh per 8 m cell, instanced at three levels of
//! detail that contain the *same* blades (distant levels just drop the upper blade tiers and blade
//! segments), so switching a cell's level never moves a blade. The vertex shader places each clump
//! on the terrain height map, culls it by the grass-density map (built here from the final ground
//! materials), thins distant blades while widening the survivors (constant coverage), and bends
//! blades with travelling wind gusts. Beyond ~75 m the photographic terrain texture carries on.

use bevy::asset::uuid_handle;
use bevy::asset::{RenderAssetUsages, UntypedAssetId};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::extract_resource::ExtractResource;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, encase,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use crate::camera::MainCamera;
use crate::noise::Rng;
use crate::terrain::gpu::{PendingTexWrites, TexWrite};
use crate::terrain::layers::Layer;
use crate::terrain::material::TerrainGpu;
use crate::terrain::{GridRect, Terrain};
use crate::water::WaterBodies;

/// Cell size (m): one patch instance each.
pub const CELL: f32 = 8.0;
/// Clumps per cell (6 per m²) and the original blades per clump. Realistic grass adds a dense
/// fine layer in the shader mesh recipe; the original seven remain intact for hand-painted
/// worlds.
const SLOTS: usize = 384;
const BLADES: usize = 7;
/// Blade tiers: distant levels keep only the low tiers.
const TIER: [u32; BLADES] = [0, 0, 1, 1, 1, 2, 2];
/// Fine realistic-only blades. Most are near-only basal leaves; a smaller low-tier set keeps
/// the distant silhouette textured without depending on very broad surviving ribbons.
const REALISTIC_TIER: [u32; 21] = [
    0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
];
/// Level-of-detail ranges (m): near, mid, far.
pub const RANGES: [f32; 3] = [16.0, 40.0, 75.0];
/// A wide editor overview cannot resolve individual blades, so it does not allocate a preview
/// ring of sub-pixel instances.
const ORBIT_PREVIEW_MAX_DISTANCE: f32 = 600.0;

/// The level-of-detail ranges scaled by the graphics detail setting (Close draws grass to ~50 m,
/// Far to ~110 m).
fn ranges(g: &crate::streaming::Graphics) -> [f32; 3] {
    let k = g.detail.clamp(0.65, 1.5);
    [
        RANGES[0] * k.min(1.0),
        RANGES[1] * k.min(1.2),
        RANGES[2] * k,
    ]
}

/// Keep an incomplete or hand-edited saved appearance from creating an invalid shader parameter.
/// Zero is deliberately the old meadow recipe.
fn lushness(value: f32) -> f32 {
    value
        .is_finite()
        .then_some(value.clamp(0.0, 1.0))
        .unwrap_or(0.0)
}

/// The dense mesh already exists for realistic grass. A lush painted meadow borrows its extra
/// blade slots, but the shader leaves them absent at zero so the old painted mesh is exact there.
fn mesh_style(painted: bool, grass_lushness: f32) -> usize {
    usize::from(!painted || grass_lushness > 0.001)
}

/// Bounds for vertex-shader-displaced blades. At full lushness the tallest blade may combine the
/// 1.25 m legacy mesh tip, the 1.5× biome height, a dry-turf multiplier and the 2.08× rich
/// accent multiplier. Wind rotates that length without increasing it. Six metres therefore
/// covers the algebraic tip envelope plus width and interpolation slack.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GrassBounds {
    aabb_top: f32,
    horizontal_margin: f32,
    lod_top: f32,
    scan_extra: f32,
}

fn grass_bounds(grass_lushness: f32) -> GrassBounds {
    let l = lushness(grass_lushness);
    GrassBounds {
        // Preserve legacy culling exactly at zero, then linearly reserve the rich-tip envelope.
        aabb_top: 1.6 + 4.4 * l,
        horizontal_margin: 0.6 + 5.4 * l,
        lod_top: 1.0 + 5.0 * l,
        scan_extra: 5.4 * l,
    }
}

pub type GrassMaterial = ExtendedMaterial<StandardMaterial, GrassExt>;

/// Grass mesh/material pairs that can be warmed before any grass cell enters the camera frustum.
/// The six static LOD recipes all share the one grass material.
#[derive(Resource, Clone, Default, ExtractResource)]
#[extract_app(RenderApp)]
pub(crate) struct GrassPipelineCandidates {
    pub pairs: Vec<(UntypedAssetId, UntypedAssetId)>,
}

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct GrassParams {
    /// x: terrain half size, y: cell size, z: vertices per side, w: sea level
    pub grid: Vec4,
    /// xy: wind direction, z: wind speed (m/s), w: time
    pub wind: Vec4,
    /// xy: gust field offset (integrated wind travel), zw: camera xz
    pub gust: Vec4,
    /// x: camera y, y: far fade start, z: far fade end, w: blade height scale
    pub view: Vec4,
    /// rgb: blade base colour (linear), w: unused
    pub base: Vec4,
    /// rgb: lush tip colour (linear)
    pub tip: Vec4,
    /// rgb: dry tip colour (linear)
    pub dry: Vec4,
    /// rgb: tip colour of cold (boreal / alpine) grass
    pub cold: Vec4,
    /// rgb: tip colour of warm (tropical) grass
    pub warm: Vec4,
    /// x: grass amount (density multiplier), y: realistic art treatment, z: lushness (0..1),
    /// w: editor orbit preview uses focus-centred horizontal LOD instead of eye distance
    pub amount: Vec4,
}

/// The grass parameters change every frame (wind, time, camera): they live in a storage buffer
/// so the material asset never changes (that would re-specialize every grass cell each frame).
pub const GRASS_PARAMS: Handle<ShaderBuffer> = uuid_handle!("0b7d4f9e-3c21-4a8e-9f5d-7e2a1c6b8d40");

/// How a biome's grass grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrassStyle {
    /// multiplies the density from the ground layers (0 = no grass, e.g. deserts)
    pub density: f32,
    /// added dryness (straw colour, taller thinner blades)
    pub dry: f32,
    /// 0 cold blue-green … 0.5 neutral … 1 warm yellow-green
    pub tint: f32,
    /// blade height multiplier
    pub height: f32,
}

impl GrassStyle {
    pub const DEFAULT: Self = Self {
        density: 1.0,
        dry: 0.0,
        tint: 0.5,
        height: 1.0,
    };
    pub const NONE: Self = Self {
        density: 0.0,
        dry: 0.0,
        tint: 0.5,
        height: 1.0,
    };
}

/// Grass style per biome-map value (0 = no biome: the default meadow grass).
fn biome_styles() -> [GrassStyle; 16] {
    let mut out = [GrassStyle::DEFAULT; 16];
    for p in crate::biomes::BiomePreset::ALL {
        out[(p.id() as usize).min(15)] = p.def().grass;
    }
    out
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct GrassExt {
    #[storage(100, read_only)]
    pub params: Handle<ShaderBuffer>,
    #[texture(101, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
    #[texture(102)]
    #[sampler(103)]
    pub density: Handle<Image>,
}

impl Default for GrassExt {
    fn default() -> Self {
        Self {
            params: GRASS_PARAMS,
            height: Handle::default(),
            density: Handle::default(),
        }
    }
}

impl MaterialExtension for GrassExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/grass.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/grass.wesl".into()
    }
    /// Blades are placed in the vertex shader: kept out of the depth prepass (drawn, and depth
    /// tested, in the main pass only). (In the prepass, ambient occlusion shades the dense blades
    /// against each other: meadows went dark and speckled.)
    fn enable_prepass() -> bool {
        false
    }
}

/// Per-vertex grass density (R), dryness (G), tint (B) and height (A), plus per-cell maxima for
/// culling.
#[derive(Resource)]
pub struct GrassField {
    image: Handle<Image>,
    material: Handle<GrassMaterial>,
    // [painted/realistic][near/mid/far]. Keeping the recipes separate avoids running the dense
    // realistic vertex layer at all in a hand-painted world.
    meshes: [[Handle<Mesh>; 3]; 2],
    data: Vec<[u8; 4]>,
    n: usize,
    generation: Option<u64>,
    /// The style used to build the current density map. World metadata changes often, so this
    /// records the actual art-style transition instead of invalidating the full map on any edit.
    density_painted: Option<bool>,
    /// The cached mesh recipe currently assigned to pool entities (0 painted, 1 realistic).
    mesh_style: Option<usize>,
    /// grass style per biome-map value
    styles: [GrassStyle; 16],
    /// max density and height range per grass cell
    cells: Vec<(u8, f32, f32)>,
    cells_per_side: usize,
    pools: [Vec<Entity>; 3],
    last_cam: Vec3,
    /// Switching between eye, focus-preview and overview-suppressed modes changes the pool even
    /// when their point happens to coincide.
    last_lod_mode: Option<GrassLodMode>,
    gust: Vec2,
    /// animation clock (s): stops while motion is paused
    clock: f32,
    /// grass amount is zero: no cells
    hidden: bool,
    dirty_cells: bool,
}

pub struct GrassPlugin;

impl Plugin for GrassPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<GrassMaterial>::default())
            .add_systems(Startup, setup)
            .add_systems(
                PostUpdate,
                (update_density, place_cells, update_params)
                    .chain()
                    .before(crate::terrain::TerrainSystems::Gpu),
            );
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<GrassMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    // the parameter buffer must exist before the material is prepared
    write_params(&mut buffers, &GrassParams::default());
    let patch = build_patch(2024);
    let meshes = [
        [
            meshes.add(patch_mesh(&patch, 0, false)),
            meshes.add(patch_mesh(&patch, 1, false)),
            meshes.add(patch_mesh(&patch, 2, false)),
        ],
        [
            meshes.add(patch_mesh(&patch, 0, true)),
            meshes.add(patch_mesh(&patch, 1, true)),
            meshes.add(patch_mesh(&patch, 2, true)),
        ],
    ];
    let image = images.add(density_image(2, vec![0; 2 * 2 * 4]));
    let material = mats.add(GrassMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.62,
            reflectance: 0.28,
            diffuse_transmission: 0.3,
            // both faces render with the same up-biased normal (no back-face flip → never black)
            cull_mode: None,
            double_sided: false,
            ..default()
        },
        extension: GrassExt {
            density: image.clone(),
            ..default()
        },
    });
    let material_id = material.id().untyped();
    commands.insert_resource(GrassPipelineCandidates {
        pairs: meshes
            .iter()
            .flatten()
            .map(|mesh| (mesh.id().untyped(), material_id))
            .collect(),
    });
    commands.insert_resource(GrassField {
        image,
        material,
        meshes,
        data: vec![],
        n: 0,
        generation: None,
        density_painted: None,
        mesh_style: None,
        styles: biome_styles(),
        cells: vec![],
        cells_per_side: 0,
        pools: [vec![], vec![], vec![]],
        last_cam: Vec3::splat(f32::MAX),
        last_lod_mode: None,
        gust: Vec2::ZERO,
        clock: 0.0,
        hidden: false,
        dirty_cells: true,
    });
}

fn density_image(n: usize, data: Vec<u8>) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

/// One blade vertex before placement.
struct BladeVert {
    /// centre-line point relative to the clump centre (unit height)
    pos: [f32; 3],
    hf: f32,
    across: f32,
}

struct Blade {
    slot: [f32; 2],
    slot_rand: f32,
    rnd: f32,
    tier: u32,
    realistic_only: bool,
    az: f32,
    /// rows of the centre line (hf, point)
    line: Vec<(f32, [f32; 3])>,
}

/// Blue-noise clump slots on the 8 m torus (best-candidate) and their blades.
fn build_patch(seed: u64) -> Vec<Blade> {
    let mut rng = Rng::new(seed);
    let mut slots: Vec<[f32; 2]> = vec![];
    for _ in 0..SLOTS {
        let mut best = [0.0, 0.0];
        let mut best_d = -1.0;
        for _ in 0..12 {
            let c = [rng.f32() * CELL, rng.f32() * CELL];
            let d = slots
                .iter()
                .map(|s| {
                    let dx = (s[0] - c[0]).abs().min(CELL - (s[0] - c[0]).abs());
                    let dz = (s[1] - c[1]).abs().min(CELL - (s[1] - c[1]).abs());
                    dx * dx + dz * dz
                })
                .fold(f32::MAX, f32::min);
            if d > best_d {
                best_d = d;
                best = c;
            }
        }
        slots.push(best);
    }
    let mut blades = vec![];
    let mut clumps = vec![];
    for s in slots {
        let slot_rand = rng.f32();
        let az0 = rng.f32() * std::f32::consts::TAU;
        for (k, tier) in TIER.iter().enumerate() {
            // golden-angle azimuths: any prefix of the blades spreads around the clump
            let az = az0 + k as f32 * 2.399_96;
            let rnd = rng.f32();
            let lean = if k < 2 {
                0.35 + 0.25 * rnd
            } else {
                0.12 + 0.4 * rnd
            };
            let curve = 0.15 + 0.35 * rng.f32();
            let h = 0.75 + 0.5 * rng.f32();
            let spread = 0.06 + 0.1 * rng.f32();
            let line = [0.0f32, 0.3, 0.62, 1.0]
                .iter()
                .map(|&t| {
                    let out = (lean.sin() * t + curve * t * t) * h;
                    let up = (lean.cos() * t - 0.35 * curve * t * t) * h;
                    (
                        t,
                        [az.cos() * (out + spread), up, az.sin() * (out + spread)],
                    )
                })
                .collect();
            blades.push(Blade {
                slot: s,
                slot_rand,
                rnd,
                tier: *tier,
                realistic_only: false,
                az,
                line,
            });
        }
        // Keep the original random stream and blade recipe above intact for the painted mesh.
        // Fine realistic blades are generated only after every original clump is complete.
        clumps.push((s, az0));
    }
    for (s, az0) in clumps {
        for (k, tier) in REALISTIC_TIER.iter().enumerate() {
            // The fine layer must not fan out from the seven legacy roots: distribute its roots
            // across the surrounding tuft so neighbouring tufts merge into ground cover.
            let root_angle = az0 + k as f32 * 2.399_96 + (rng.f32() - 0.5) * 0.8;
            let root_radius = 0.05 + 0.40 * rng.f32().sqrt();
            let slot = [
                (s[0] + root_angle.cos() * root_radius).rem_euclid(CELL),
                (s[1] + root_angle.sin() * root_radius).rem_euclid(CELL),
            ];
            let slot_rand = rng.f32();
            let az = root_angle + (rng.f32() - 0.5) * 1.4;
            let rnd = rng.f32();
            let lean = 0.08 + 0.36 * rnd;
            let curve = 0.12 + 0.30 * rng.f32();
            let h = 0.62 + 0.44 * rng.f32();
            let spread = 0.025 + 0.075 * rng.f32();
            let line = [0.0f32, 0.3, 0.62, 1.0]
                .iter()
                .map(|&t| {
                    let out = (lean.sin() * t + curve * t * t) * h;
                    let up = (lean.cos() * t - 0.35 * curve * t * t) * h;
                    (
                        t,
                        [az.cos() * (out + spread), up, az.sin() * (out + spread)],
                    )
                })
                .collect();
            blades.push(Blade {
                slot,
                slot_rand,
                rnd,
                tier: *tier,
                realistic_only: true,
                az,
                line,
            });
        }
    }
    blades
}

/// Patch mesh for a level of detail: 0 near (3 segments, all tiers), 1 mid (2 segments, tiers
/// 0–1), 2 far (one triangle, tier 0).
fn patch_mesh(blades: &[Blade], lod: usize, realistic: bool) -> Mesh {
    let rows: &[usize] = match lod {
        0 => &[0, 1, 2],
        1 => &[0, 2],
        _ => &[0],
    };
    let max_tier = 2 - lod as u32;
    let mut pos = vec![];
    let mut nor = vec![];
    let mut uv0 = vec![];
    let mut uv1 = vec![];
    let mut col = vec![];
    let mut idx: Vec<u32> = vec![];
    for b in blades
        .iter()
        .filter(|b| b.tier <= max_tier && (realistic || !b.realistic_only))
    {
        let base = pos.len() as u32;
        let mut verts: Vec<BladeVert> = vec![];
        for &r in rows {
            let (hf, p) = b.line[r];
            verts.push(BladeVert {
                pos: p,
                hf,
                across: -1.0,
            });
            verts.push(BladeVert {
                pos: p,
                hf,
                across: 1.0,
            });
        }
        let (hf, p) = b.line[3];
        verts.push(BladeVert {
            pos: p,
            hf,
            across: 0.0,
        });
        for v in &verts {
            pos.push(v.pos);
            nor.push([0.0, 1.0, 0.0]);
            uv0.push([v.hf, v.across]);
            uv1.push(b.slot);
            // Values below 1.5 are the unchanged hand-painted recipe. Values at or above it
            // identify the additional realistic-only blade layer in the vertex shader.
            let tier = b.tier as f32 * 0.5 + if b.realistic_only { 1.5 } else { 0.0 };
            col.push([
                b.rnd,
                tier,
                b.slot_rand,
                b.az.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU,
            ]);
        }
        let quads = rows.len() as u32 - 1;
        for q in 0..quads {
            let a = base + q * 2;
            idx.extend_from_slice(&[a, a + 1, a + 3, a, a + 3, a + 2]);
        }
        let last = base + quads * 2;
        idx.extend_from_slice(&[last, last + 1, last + 2]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv0);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

/// The three patch meshes (near, mid, far) of the meadow as the editor draws them, for the game
/// engine exports (Godot rebuilds the same blades with a port of `grass.wesl`).
pub(crate) fn export_patch_meshes(realistic: bool) -> [Mesh; 3] {
    let patch = build_patch(2024);
    [0, 1, 2].map(|lod| patch_mesh(&patch, lod, realistic))
}

/// Grass density, dryness, tint and height for one terrain vertex, from the final ground
/// materials and the biome map.
fn density_at(
    t: &Terrain,
    water: &WaterBodies,
    roads: &crate::roads::Roads,
    styles: &[GrassStyle; 16],
    realistic: bool,
    i: usize,
    j: usize,
) -> [u8; 4] {
    let h = t.h(i, j);
    if h < t.sea_level + 0.3 {
        return [0, 0, 128, 128];
    }
    if !water.level_map.is_empty() && water.level_res == t.res {
        let k = j * t.n() + i;
        if water.level_map.get(k).is_some_and(|l| *l > h - 0.05) {
            return [0, 0, 128, 128];
        }
    }
    let w = t.final_weights(i, j);
    let g = w[Layer::Grass as usize];
    let d = w[Layer::DryGrass as usize];
    let f = w[Layer::ForestFloor as usize];
    let dirt = w[Layer::Dirt as usize];
    // Roads already paint their core and shoulders into the terrain splat. Keep their historic
    // weak forest-floor contribution even in realistic mode, while natural forest floor can grow
    // a denser understory. Dirt intentionally remains sparse so painted bare paths stay legible.
    let road_covered = roads.ground.base(i, j).is_some();
    let forest_cover = if realistic && !road_covered {
        0.6
    } else {
        0.18
    };
    // the biome's style, blended across biome borders over their ecotone band
    let mut st = [0.0f32; 4];
    for (id, k) in crate::visuals::ecotone::biome_weights(t, t.grid_to_world(i, j), 0) {
        let s = styles[(id as usize).min(15)];
        st[0] += s.density * k;
        st[1] += s.dry * k;
        st[2] += s.tint * k;
        st[3] += s.height * k;
    }
    // (thinning out in clumps where the ground turns, not in a smooth ramp)
    // (lush grass painted into a grass-free biome, an oasis's shore, grows there all the same)
    let oasis = g * 0.8 * (1.0 - crate::noise::smoothstep(0.0, 0.3, st[0]));
    let density = crate::visuals::ecotone::patchy(
        ((g + d * 0.9 + f * forest_cover + dirt * 0.1) * st[0])
            .max(oasis)
            .clamp(0.0, 1.0),
        t.grid_to_world(i, j),
    );
    let dry = (if g + d > 0.01 { d / (g + d) } else { 0.0 } + st[1]).clamp(0.0, 1.0);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [q(density), q(dry), q(st[2]), q(st[3] * 0.5)]
}

impl GrassField {
    /// The per-vertex grass map (density, dryness, tint, height) and its vertices per side, once
    /// built for the current terrain (read by the engine exporters).
    pub fn density_map(&self) -> Option<(&[[u8; 4]], usize)> {
        (self.n > 0 && self.data.len() == self.n * self.n).then_some((&self.data, self.n))
    }
}

/// Recompute the density map where the terrain changed (whole map for a new world).
fn update_density(
    mut field: ResMut<GrassField>,
    mut terrain: ResMut<Terrain>,
    water: Res<WaterBodies>,
    mut images: ResMut<Assets<Image>>,
    mut writes: ResMut<PendingTexWrites>,
    stroke: Res<crate::tools::Stroke>,
    mut mats: ResMut<Assets<GrassMaterial>>,
    mask: Res<crate::placement::GrassMask>,
    holes: Res<crate::terrain::holes::TerrainHoles>,
    roads: Res<crate::roads::Roads>,
    meta: Res<crate::world::WorldMeta>,
) {
    let n = terrain.n();
    let fresh = field.generation != Some(terrain.generation) || field.n != n;
    if fresh {
        field.generation = Some(terrain.generation);
        field.n = n;
        field.data = vec![[0, 0, 128, 128]; n * n];
        terrain.dirty_grass = Some(GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        });
    }
    let painted = meta.art_style.painted();
    let style_changed = field.density_painted != Some(painted);
    field.density_painted = Some(painted);
    if (water.is_changed() || style_changed) && !fresh {
        terrain.dirty_grass = Some(GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        });
    }
    // during a stroke only small regions change; recompute them as we go
    let Some(rect) = terrain.dirty_grass else {
        return;
    };
    let area = rect.width() * rect.height();
    if stroke.active && area > 200 * 200 {
        return;
    }
    terrain.dirty_grass = None;
    let t = &*terrain;
    let zs: Vec<usize> = (rect.z0..=rect.z1).collect();
    let styles = &field.styles;
    let realistic = !painted;
    let rows: Vec<(usize, Vec<[u8; 4]>)> = std::thread::scope(|s| {
        let chunk = zs.len().div_ceil(8).max(1);
        let handles: Vec<_> = zs
            .chunks(chunk)
            .map(|c| {
                let water = &*water;
                let roads = &*roads;
                s.spawn(move || {
                    c.iter()
                        .map(|&j| {
                            (
                                j,
                                (rect.x0..=rect.x1)
                                    .map(|i| density_at(t, water, roads, styles, realistic, i, j))
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    for (j, row) in rows {
        let a = j * n + rect.x0;
        field.data[a..a + row.len()].copy_from_slice(&row);
    }
    // no grass under buildings' floors, nor in a cave mouth
    mask.clear(&mut field.data, n, rect);
    holes.clear_grass(&mut field.data, n, rect);
    if std::env::var("WB_GRASS_LOG").is_ok() {
        let land: Vec<&[u8; 4]> = field.data.iter().filter(|d| d[0] > 0).collect();
        let mean = |c: usize| {
            land.iter().map(|d| d[c] as f32).sum::<f32>() / land.len().max(1) as f32 / 255.0
        };
        info!(
            "grass map {}x{}: {} grassy vertices, mean density {:.2} dry {:.2} tint {:.2} height {:.2}",
            rect.width(),
            rect.height(),
            land.len(),
            mean(0),
            mean(1),
            mean(2),
            mean(3) * 2.0
        );
    }
    if fresh
        || field.image == Handle::default()
        || images
            .get(&field.image)
            .is_none_or(|i| i.width() as usize != n)
    {
        let bytes: Vec<u8> = field.data.iter().flatten().copied().collect();
        let h = images.add(density_image(n, bytes));
        field.image = h.clone();
        if let Some(mut m) = mats.get_mut(&field.material) {
            m.extension.density = h;
        }
    } else {
        // stream just the changed rows to the GPU
        let (w, h) = (rect.width(), rect.height());
        let mut bytes = Vec::with_capacity(w * h * 4);
        for j in rect.z0..=rect.z1 {
            for i in rect.x0..=rect.x1 {
                let d = field.data[j * n + i];
                bytes.extend_from_slice(&d);
            }
        }
        writes.0.push(TexWrite {
            image: field.image.id(),
            x: rect.x0 as u32,
            y: rect.z0 as u32,
            w: w as u32,
            h: h as u32,
            bytes_per_texel: 4,
            mip: 0,
            offset: 0,
            data: std::sync::Arc::new(bytes),
        });
    }
    rebuild_cells(&mut field, t, Some(rect));
}

/// Per grass cell: max density and terrain height range (for frustum culling).
fn rebuild_cells(field: &mut GrassField, t: &Terrain, rect: Option<GridRect>) {
    let per = (t.size() / CELL).ceil() as usize;
    if field.cells_per_side != per || field.cells.len() != per * per {
        field.cells_per_side = per;
        field.cells = vec![(0, 0.0, 0.0); per * per];
        return rebuild_cells(field, t, None);
    }
    let n = t.n();
    let vpc = CELL / t.cell; // vertices per grass cell
    let (c0x, c0z, c1x, c1z) = match rect {
        Some(r) => (
            (r.x0 as f32 / vpc) as usize,
            (r.z0 as f32 / vpc) as usize,
            ((r.x1 as f32 / vpc) as usize).min(per - 1),
            ((r.z1 as f32 / vpc) as usize).min(per - 1),
        ),
        None => (0, 0, per - 1, per - 1),
    };
    for cz in c0z..=c1z {
        for cx in c0x..=c1x {
            let i0 = (cx as f32 * vpc).floor() as usize;
            let j0 = (cz as f32 * vpc).floor() as usize;
            let i1 = (((cx + 1) as f32 * vpc).ceil() as usize).min(n - 1);
            let j1 = (((cz + 1) as f32 * vpc).ceil() as usize).min(n - 1);
            let (mut d, mut lo, mut hi) = (0u8, f32::MAX, f32::MIN);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    d = d.max(field.data[j * n + i][0]);
                    let h = t.h(i, j);
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
            field.cells[cz * per + cx] = (d, lo, hi);
        }
    }
    field.dirty_cells = true;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GrassLodMode {
    Eye,
    FocusPreview,
    Suppressed,
}

/// Close editor views and walking retain eye-centred 3D distance. Only an orbit whose eye has
/// risen beyond the grass range previews a bounded far-LOD ring at its inspected ground.
fn grass_lod_mode(
    eye: Vec3,
    orbit: &crate::camera::OrbitCamera,
    eye_ground: f32,
    far_range: f32,
) -> GrassLodMode {
    if !orbit.enabled || orbit.in_cave() || eye.y - eye_ground < far_range {
        GrassLodMode::Eye
    } else if eye.distance(orbit.focus) >= ORBIT_PREVIEW_MAX_DISTANCE {
        GrassLodMode::Suppressed
    } else {
        GrassLodMode::FocusPreview
    }
}

fn grass_lod_origin(eye: Vec3, orbit: &crate::camera::OrbitCamera, mode: GrassLodMode) -> Vec3 {
    if mode == GrassLodMode::FocusPreview {
        orbit.focus
    } else {
        eye
    }
}

/// Puts patch instances on the cells around the active grass LOD origin.
fn place_cells(
    mut commands: Commands,
    mut field: ResMut<GrassField>,
    terrain: Res<Terrain>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    orbit: Res<crate::camera::OrbitCamera>,
    mut q: Query<(&mut Transform, &mut Aabb, &mut Visibility, &mut Mesh3d)>,
    amounts: Res<crate::objects::NatureAmounts>,
    graphics: Res<crate::streaming::Graphics>,
    meta: Res<crate::world::WorldMeta>,
    appearance: Res<crate::nature::NatureAppearance>,
) {
    let Ok(cam) = cam.single() else { return };
    if std::env::var("WB_PERF").is_ok_and(|p| p.contains("nograss")) {
        return;
    }
    let grass_lushness = lushness(appearance.grass_lushness);
    // Lushness changes vertical and wind-displaced reach even when the mesh recipe itself stays
    // the same, so refresh pooled cell bounds before the stationary-camera early return.
    if appearance.is_changed() {
        field.dirty_cells = true;
    }
    let mesh_style = mesh_style(meta.art_style.painted(), grass_lushness);
    if field.mesh_style != Some(mesh_style) {
        field.mesh_style = Some(mesh_style);
        field.dirty_cells = true;
    }
    // no grass at all: no cells either
    let none = amounts.grass <= 0.01;
    if none != field.hidden {
        field.hidden = none;
        field.dirty_cells = true;
    }
    let eye = cam.translation();
    let rg = ranges(&graphics);
    let bounds = grass_bounds(grass_lushness);
    let lod_mode = grass_lod_mode(eye, &orbit, terrain.height_at(eye.x, eye.z), rg[2]);
    let p = grass_lod_origin(eye, &orbit, lod_mode);
    if graphics.is_changed() {
        field.dirty_cells = true;
    }
    if field.last_lod_mode != Some(lod_mode) {
        field.dirty_cells = true;
    }
    if !field.dirty_cells && p.distance(field.last_cam) < 1.0 {
        return;
    }
    field.last_cam = p;
    field.last_lod_mode = Some(lod_mode);
    field.dirty_cells = false;
    let per = field.cells_per_side;
    if per == 0 {
        return;
    }
    let half = terrain.half();
    let mut wanted: [Vec<(Vec2, f32, f32)>; 3] = [vec![], vec![], vec![]];
    let reach = rg[2] + CELL + bounds.scan_extra;
    let cx0 = (((p.x - reach + half) / CELL).floor().max(0.0)) as usize;
    let cz0 = (((p.z - reach + half) / CELL).floor().max(0.0)) as usize;
    let cx1 = ((((p.x + reach + half) / CELL).ceil()) as usize).min(per - 1);
    let cz1 = ((((p.z + reach + half) / CELL).ceil()) as usize).min(per - 1);
    // An orbit eye can be hundreds of metres above the focus. Keep the fixed-size grass ring
    // around that inspected ground; the renderer's ordinary frustum culling still rejects cells
    // outside the actual view. Walking keeps the old vertical range guard.
    let within_vertical_range = lod_mode == GrassLodMode::FocusPreview
        || (p.y - terrain.height_at(p.x, p.z)) < rg[2] + bounds.scan_extra;
    if lod_mode != GrassLodMode::Suppressed && within_vertical_range && !field.hidden {
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                let (d, lo, hi) = field.cells[cz * per + cx];
                if d < 8 {
                    continue;
                }
                let o = Vec2::new(cx as f32 * CELL - half, cz as f32 * CELL - half);
                // The preview ring is intentionally measured across the inspected ground. In
                // walk mode use the previous nearest 3D distance, including terrain height.
                let near = Vec2::new(p.x.clamp(o.x, o.x + CELL), p.z.clamp(o.y, o.y + CELL));
                let horizontal = near.distance(p.xz());
                let dist = if lod_mode == GrassLodMode::FocusPreview {
                    horizontal
                } else {
                    let dy = (p.y - p.y.clamp(lo, hi + bounds.lod_top)).abs();
                    (horizontal.powi(2) + dy * dy).sqrt()
                };
                // A lush blade can lean into the visible shell even when its cell origin is just
                // beyond it. The extra outer ring uses the existing far recipe only.
                let lod = if lod_mode == GrassLodMode::FocusPreview {
                    (dist < rg[2] + bounds.scan_extra).then_some(2)
                } else {
                    rg.iter().position(|r| dist < *r + bounds.scan_extra)
                };
                if let Some(l) = lod {
                    wanted[l].push((o, lo, hi));
                }
            }
        }
    }
    if std::env::var("WB_GRASS_LOG").is_ok() {
        info!(
            "grass cells near/mid/far: {} {} {} (pools {} {} {})",
            wanted[0].len(),
            wanted[1].len(),
            wanted[2].len(),
            field.pools[0].len(),
            field.pools[1].len(),
            field.pools[2].len()
        );
    }
    for lod in 0..3 {
        let need = wanted[lod].len();
        if field.pools[lod].len() < need {
            // new entities only exist next frame: place them then
            field.dirty_cells = true;
        }
        while field.pools[lod].len() < need {
            // (`NoAutoAabb`: the blades are placed in the vertex shader, so the bounds Bevy works
            // out from the patch mesh lie at y = 0, and it writes them over ours when an entity is
            // spawned: the cell stays culled until the camera moves or the ground changes)
            let e = commands
                .spawn((
                    Mesh3d(field.meshes[mesh_style][lod].clone()),
                    MeshMaterial3d(field.material.clone()),
                    Transform::IDENTITY,
                    Aabb::default(),
                    Visibility::Hidden,
                    NotShadowCaster,
                    NoAutoAabb,
                ))
                .id();
            field.pools[lod].push(e);
        }
        for (k, e) in field.pools[lod].clone().into_iter().enumerate() {
            let Ok((mut tf, mut aabb, mut vis, mut mesh)) = q.get_mut(e) else {
                continue;
            };
            match wanted[lod].get(k) {
                Some((o, lo, hi)) => {
                    if mesh.0 != field.meshes[mesh_style][lod] {
                        mesh.0 = field.meshes[mesh_style][lod].clone();
                    }
                    tf.translation = Vec3::new(o.x, 0.0, o.y);
                    let top = hi + bounds.aabb_top;
                    *aabb = Aabb::from_min_max(
                        Vec3::new(
                            -bounds.horizontal_margin,
                            lo - 0.2,
                            -bounds.horizontal_margin,
                        ),
                        Vec3::new(
                            CELL + bounds.horizontal_margin,
                            top,
                            CELL + bounds.horizontal_margin,
                        ),
                    );
                    if *vis != Visibility::Inherited {
                        *vis = Visibility::Inherited;
                    }
                }
                None => {
                    if *vis != Visibility::Hidden {
                        *vis = Visibility::Hidden;
                    }
                }
            }
        }
    }
}

fn update_params(
    mut field: ResMut<GrassField>,
    mut mats: ResMut<Assets<GrassMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    terrain: Res<Terrain>,
    gpu: Option<Res<TerrainGpu>>,
    wind: Res<crate::wind::WindState>,
    motion: Res<crate::wind::Motion>,
    amounts: Res<crate::objects::NatureAmounts>,
    graphics: Res<crate::streaming::Graphics>,
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    orbit: Res<crate::camera::OrbitCamera>,
    meta: Res<crate::world::WorldMeta>,
    appearance: Res<crate::nature::NatureAppearance>,
) {
    let Some(gpu) = gpu else { return };
    // while editing the grass holds still: its clock stops and the wind drops
    if motion.live {
        field.clock += time.delta_secs();
    }
    let (dir, speed) = (wind.dir, if motion.live { wind.speed } else { 0.0 });
    // the same travelling gust field as the trees
    field.gust = wind.gust_offset;
    let eye = cam.single().map(|c| c.translation()).unwrap_or_default();
    let lod_mode = grass_lod_mode(
        eye,
        &orbit,
        terrain.height_at(eye.x, eye.z),
        ranges(&graphics)[2],
    );
    let p = grass_lod_origin(eye, &orbit, lod_mode);
    // only a new height texture (new world) touches the material itself
    if mats
        .get(&field.material)
        .is_some_and(|m| m.extension.height != gpu.height)
        && let Some(mut m) = mats.get_mut(&field.material)
    {
        m.extension.height = gpu.height.clone();
    }
    let lin = |c: [f32; 3]| Vec4::new(c[0], c[1], c[2], 1.0);
    // (base, lush tip, dry tip, cold tip, warm tip); a hand-painted world's grass is stylised
    // like its painted lawns: light, saturated roots close to the tips (a flat painted sward
    // rather than deep photographic shade between blades), sunny yellow-green tips
    let [base, tip, dry, cold, warm] = if meta.art_style.painted() {
        [
            [0.12, 0.2, 0.04],
            [0.38, 0.54, 0.1],
            [0.66, 0.5, 0.16],
            [0.2, 0.36, 0.15],
            [0.44, 0.56, 0.08],
        ]
    } else {
        [
            [0.085, 0.115, 0.03],
            [0.3, 0.42, 0.1],
            [0.55, 0.45, 0.2],
            [0.16, 0.27, 0.12],
            [0.36, 0.5, 0.07],
        ]
    };
    let params = GrassParams {
        grid: Vec4::new(
            terrain.half(),
            terrain.cell,
            terrain.n() as f32,
            terrain.sea_level,
        ),
        wind: Vec4::new(dir.x, dir.y, speed, field.clock),
        // zw is the same LOD origin used by `place_cells`: focus in the editor, eye while
        // walking. The shader selects horizontal distance only for the editor preview.
        gust: Vec4::new(field.gust.x, field.gust.y, p.x, p.z),
        view: Vec4::new(
            eye.y,
            ranges(&graphics)[2] - 20.0,
            ranges(&graphics)[2] - 2.0,
            1.0,
        ),
        base: lin(base),
        tip: lin(tip),
        dry: lin(dry),
        cold: lin(cold),
        warm: lin(warm),
        // y selects the realistic leaf treatment. z is a separate, persisted appearance dial:
        // zero preserves the current recipe; one asks the shader for the full layered meadow.
        amount: Vec4::new(
            amounts.grass,
            (!meta.art_style.painted()) as u32 as f32,
            lushness(appearance.grass_lushness),
            (lod_mode == GrassLodMode::FocusPreview) as u32 as f32,
        ),
    };
    write_params(&mut buffers, &params);
}

fn write_params(buffers: &mut Assets<ShaderBuffer>, params: &GrassParams) {
    let mut bytes = encase::StorageBuffer::new(Vec::<u8>::new());
    if bytes.write(params).is_ok() {
        let _ = buffers.insert(
            &GRASS_PARAMS,
            ShaderBuffer::new(
                bytes.into_inner(),
                RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn painted_recipe_is_the_original_prefix_and_excludes_fine_blades() {
        let blades = build_patch(2024);
        let split = SLOTS * BLADES;
        assert_eq!(
            blades[..split].iter().filter(|b| b.realistic_only).count(),
            0
        );
        assert!(blades[split..].iter().all(|b| b.realistic_only));

        for lod in 0..3 {
            let max_tier = 2 - lod as u32;
            let painted = blades
                .iter()
                .filter(|b| b.tier <= max_tier && !b.realistic_only)
                .count();
            let realistic = blades.iter().filter(|b| b.tier <= max_tier).count();
            assert!(
                realistic > painted,
                "realistic LOD {lod} must carry its own fine layer"
            );
        }
    }

    #[test]
    fn grass_lushness_is_safe_and_only_adds_the_dense_painted_mesh_above_zero() {
        assert_eq!(lushness(-1.0), 0.0);
        assert_eq!(lushness(0.4), 0.4);
        assert_eq!(lushness(2.0), 1.0);
        assert_eq!(lushness(f32::NAN), 0.0);

        assert_eq!(mesh_style(true, 0.0), 0);
        assert_eq!(mesh_style(false, 0.0), 1);
        assert_eq!(mesh_style(true, 1.0), 1);
    }

    #[test]
    fn lush_grass_bounds_cover_displacement_without_changing_the_legacy_envelope() {
        assert_eq!(
            grass_bounds(0.0),
            GrassBounds {
                aabb_top: 1.6,
                horizontal_margin: 0.6,
                lod_top: 1.0,
                scan_extra: 0.0,
            }
        );
        assert_eq!(
            grass_bounds(1.0),
            GrassBounds {
                aabb_top: 6.0,
                horizontal_margin: 6.0,
                lod_top: 6.0,
                scan_extra: 5.4,
            }
        );
    }

    #[test]
    fn high_orbit_preview_is_focus_centred_far_only_and_wide_overview_is_suppressed() {
        let eye = Vec3::new(12.0, 228.0, -18.0);
        let mut orbit = crate::camera::OrbitCamera::default();
        orbit.focus = Vec3::new(12.0, 0.0, -18.0);

        let mode = grass_lod_mode(eye, &orbit, 0.0, 112.0);
        assert_eq!(mode, GrassLodMode::FocusPreview);
        assert_eq!(grass_lod_origin(eye, &orbit, mode), orbit.focus);

        // A close orbit preserves the existing eye-centred 3D culling and LOD recipe.
        assert_eq!(
            grass_lod_mode(Vec3::new(12.0, 80.0, -18.0), &orbit, 0.0, 112.0),
            GrassLodMode::Eye
        );

        orbit.focus = Vec3::new(12.0, 0.0, 590.0);
        assert_eq!(
            grass_lod_mode(eye, &orbit, 0.0, 112.0),
            GrassLodMode::Suppressed
        );
        orbit.enabled = false;
        assert_eq!(grass_lod_mode(eye, &orbit, 0.0, 112.0), GrassLodMode::Eye);
    }
}
