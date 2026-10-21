//! Drawing the caves: the rock material (`shaders/cave.wesl`), its photoscanned textures (the
//! rocks, floors and accents of the themes in use, loaded in the background into one texture
//! array), and the chunk entities, rebuilt off the main thread whenever a cave or the ground under
//! it changes (only the chunks whose inputs changed get new meshes). The rebuilt volume also opens
//! the terrain where the carving breaks its surface (`terrain::holes`) and serves walk mode, the
//! camera and the pointer (`physics`).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::shader::ShaderRef;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::mesher::{self, Built, ChunkMesh, Light};
use super::model::{Caves, Theme};
use super::sdf::{self, Volume};
use crate::terrain::Terrain;

pub type CaveMaterial = ExtendedMaterial<StandardMaterial, CaveExt>;

/// One theme's slot in the shader's table.
#[derive(Clone, Debug, ShaderType, Reflect, Default, PartialEq)]
pub struct ThemeGpu {
    /// layers (in the cave texture array) of wall A, wall B, floor, accent
    pub tex: Vec4,
    /// repeat size (m) of each
    pub tile: Vec4,
    /// rgb tint, a: dampness
    pub tint: Vec4,
    /// x: accent kind, y: accent amount, z: gloss, w: rock contrast
    pub fx: Vec4,
}

#[derive(Clone, Debug, ShaderType, Reflect, Default, PartialEq)]
pub struct CaveParams {
    pub sun: Vec4,
    pub clouds: Vec4,
    /// x: normal strength, y: texture softening, z: rain wetness
    pub look: Vec4,
    /// x: cut-away height, y: cut-away on, z: terrain textures ready, w: cave textures ready
    pub misc: Vec4,
    /// repeat (m) of the terrain's rock, dirt, gravel and grass layers
    pub ground_tile: Vec4,
    /// rgb: fill colour, a: time
    pub fill: Vec4,
    pub themes: [ThemeGpu; 7],
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CaveExt {
    #[uniform(100)]
    pub params: CaveParams,
    #[texture(101, dimension = "2d_array")]
    #[sampler(102)]
    pub albedo: Handle<Image>,
    #[texture(103, dimension = "2d_array")]
    pub detail: Handle<Image>,
    #[texture(104, dimension = "2d_array")]
    pub ground_albedo: Handle<Image>,
    #[texture(105, dimension = "2d_array")]
    pub ground_detail: Handle<Image>,
}

impl MaterialExtension for CaveExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/cave.wesl".into()
    }
}

/// The caves' effects material (`shaders/cave_fx.wesl`): pool water, glowing crystals, webs.
pub type CaveFxMaterial = ExtendedMaterial<StandardMaterial, CaveFxExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default, PartialEq)]
pub struct FxParams {
    /// x: kind (0 water, 1 crystal, 2 web), y: the cave's darkness fill, z: glow strength
    pub kind: Vec4,
    /// water: shallow colour; crystal: body tint; web: thread colour (linear rgb), a: opacity
    pub a: Vec4,
    /// water: deep colour
    pub b: Vec4,
    /// rgb: the cave's fill light colour
    pub fill: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CaveFxExt {
    #[uniform(100)]
    pub params: FxParams,
    #[texture(101)]
    #[sampler(102)]
    pub tex: Handle<Image>,
}

impl MaterialExtension for CaveFxExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/cave_fx.wesl".into()
    }
}

/// Water colours by theme (shallow, deep; linear rgb).
fn water_colours(t: Theme) -> ([f32; 3], [f32; 3]) {
    match t {
        Theme::Ice => ([0.06, 0.14, 0.18], [0.005, 0.02, 0.035]),
        Theme::Mossy => ([0.05, 0.1, 0.06], [0.006, 0.016, 0.01]),
        Theme::Volcanic => ([0.04, 0.05, 0.05], [0.004, 0.006, 0.007]),
        Theme::Crystal => ([0.04, 0.12, 0.14], [0.004, 0.02, 0.03]),
        Theme::Sandstone => ([0.09, 0.09, 0.07], [0.012, 0.014, 0.012]),
        _ => ([0.05, 0.09, 0.09], [0.005, 0.013, 0.016]),
    }
}

// ------------------------------------------------------------------ themes

/// How a theme looks: library textures (`assets/textures/terrain_library`) and shading.
pub struct Look {
    pub wall: [&'static str; 2],
    pub floor: &'static str,
    pub accent: &'static str,
    pub tint: [f32; 3],
    pub damp: f32,
    /// 0 moss, 1 frost, 2 sand, 3 embers, 4 crystal, 5 ore
    pub accent_kind: u32,
    pub accent_amount: f32,
    pub gloss: f32,
    pub contrast: f32,
    /// colour of the darkness fill
    pub fill: [f32; 3],
}

pub fn look(t: Theme) -> Look {
    match t {
        Theme::Ice => Look {
            wall: ["stratified_grey_cliff", "grey_granite"],
            floor: "trampled_snow",
            accent: "frozen_crust",
            tint: [0.88, 0.94, 1.0],
            damp: 0.35,
            accent_kind: 1,
            accent_amount: 0.85,
            gloss: 0.9,
            contrast: 0.35,
            fill: [0.75, 0.85, 1.0],
        },
        Theme::Mossy => Look {
            wall: ["mossy_rock", "stratified_grey_cliff"],
            floor: "muddy_mossy_floor",
            accent: "moss",
            tint: [0.95, 1.0, 0.92],
            damp: 0.9,
            accent_kind: 0,
            accent_amount: 1.0,
            gloss: 0.6,
            contrast: 0.6,
            fill: [0.8, 0.9, 0.8],
        },
        Theme::Sandstone => Look {
            wall: ["layered_sandstone_cliff", "cracked_sandstone"],
            floor: "gravelly_desert_sand",
            accent: "red_desert_sand",
            tint: [1.0, 0.98, 0.95],
            damp: 0.05,
            accent_kind: 2,
            accent_amount: 0.8,
            gloss: 0.1,
            contrast: 0.45,
            fill: [1.0, 0.88, 0.75],
        },
        Theme::Volcanic => Look {
            wall: ["basalt", "dark_volcanic_rock"],
            floor: "dark_gravel",
            accent: "burned_ground",
            tint: [0.95, 0.93, 0.92],
            damp: 0.12,
            accent_kind: 3,
            accent_amount: 0.8,
            gloss: 0.3,
            contrast: 0.5,
            fill: [1.0, 0.7, 0.55],
        },
        Theme::Crystal => Look {
            wall: ["veined_marble_rock", "white_marble_cliff"],
            floor: "slate_gravel",
            accent: "slate_gravel",
            tint: [0.9, 0.93, 1.0],
            damp: 0.4,
            accent_kind: 4,
            accent_amount: 1.0,
            gloss: 0.6,
            contrast: 0.4,
            fill: [0.75, 0.8, 1.0],
        },
        Theme::Mine => Look {
            wall: ["dark_layered_rock", "ochre_stratified_rock"],
            floor: "muddy_cart_track",
            accent: "pale_rubble",
            tint: [1.0, 0.97, 0.92],
            damp: 0.35,
            accent_kind: 5,
            accent_amount: 0.45,
            gloss: 0.3,
            contrast: 0.6,
            fill: [1.0, 0.85, 0.65],
        },
        Theme::Limestone | Theme::Auto => Look {
            wall: ["stratified_grey_cliff", "grey_granite"],
            floor: "grey_scree",
            accent: "moss",
            tint: [1.0, 0.97, 0.92],
            damp: 0.6,
            accent_kind: 0,
            accent_amount: 0.65,
            gloss: 0.5,
            contrast: 0.7,
            fill: [0.85, 0.88, 0.95],
        },
    }
}

/// The library textures the given themes need, in array order.
pub fn texture_list(themes: &[Theme]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = vec![];
    let mut sorted: Vec<Theme> = themes.to_vec();
    sorted.sort_by_key(|t| t.slot());
    sorted.dedup();
    for t in sorted {
        let l = look(t);
        for id in [l.wall[0], l.wall[1], l.floor, l.accent] {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

fn sampler() -> ImageSamplerDescriptor {
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

fn array_image(size: u32, layers: u32, mips: u32, data: Vec<u8>, format: TextureFormat) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = mips;
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    img.sampler = ImageSampler::Descriptor(sampler());
    img
}

/// 1×1 placeholder arrays (two layers: a single layer would be bound as a plain 2D view).
fn placeholder(srgb: bool) -> Image {
    array_image(
        1,
        2,
        1,
        vec![118, 112, 104, 128, 118, 112, 104, 128],
        if srgb {
            TextureFormat::Rgba8UnormSrgb
        } else {
            TextureFormat::Rgba8Unorm
        },
    )
}

/// Loads library materials into an albedo and a detail array (full mip chains; 1024², or 512²
/// when many themes are in use so the arrays stay under ~120 MB).
fn load_arrays(ids: &[&str], art: crate::art_style::ArtStyle) -> Result<(Image, Image), String> {
    let dir = crate::terrain::textures::library_dir();
    let lut = crate::terrain::textures::srgb_to_lin_lut();
    let size = if ids.len() <= 10 { 1024u32 } else { 512 };
    let mut albedo = vec![];
    let mut detail = vec![];
    for id in ids {
        let d = dir.join(id);
        // (the painted library material in a hand-painted world)
        let [ap, np] =
            crate::art_style::textures([d.join("albedo.png"), d.join("normal.png")], art);
        let a = image::open(ap)
            .map_err(|e| format!("{id}/albedo.png: {e}"))?
            .to_rgba8();
        let n = image::open(np)
            .map_err(|e| format!("{id}/normal.png: {e}"))?
            .to_rgba8();
        let fit = |img: image::RgbaImage| {
            if img.width() == size && img.height() == size {
                img
            } else {
                image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle)
            }
        };
        albedo.extend(crate::terrain::textures::build_mips(
            fit(a).as_raw(),
            size,
            true,
            &lut,
        ));
        detail.extend(crate::terrain::textures::build_mips(
            fit(n).as_raw(),
            size,
            false,
            &lut,
        ));
    }
    // (a single layer would bind as a plain 2D view: repeat it)
    let mut layers = ids.len() as u32;
    if layers == 1 {
        let (a, d) = (albedo.clone(), detail.clone());
        albedo.extend(a);
        detail.extend(d);
        layers = 2;
    }
    let mips = size.ilog2() + 1;
    Ok((
        array_image(size, layers, mips, albedo, TextureFormat::Rgba8UnormSrgb),
        array_image(size, layers, mips, detail, TextureFormat::Rgba8Unorm),
    ))
}

// ------------------------------------------------------------------ state

/// Marks a cave chunk entity.
#[derive(Component)]
pub struct CaveChunk(pub IVec4);

/// The drawn caves: chunk entities, the material, the latest volume (physics and the pointer
/// read it) and the background work.
#[derive(Resource, Default)]
pub struct CaveWorld {
    pub material: Handle<CaveMaterial>,
    /// the same, drawn from both sides (x-ray: cut rock shows as a dark section)
    pub material_xray: Handle<CaveMaterial>,
    pub chunks: HashMap<IVec4, (Entity, u64)>,
    /// the field the chunks were built from, and its daylight
    pub volume: Option<Arc<Volume>>,
    pub light: Option<Arc<Light>>,
    /// revision of `Caves` wanted and drawn
    pub want: u64,
    pub built: u64,
    /// a rebuild is running (`want` at its start)
    task: Option<(u64, Task<(Built, Arc<Volume>)>)>,
    /// fingerprint of the ground under the caves at the last build
    ground_sig: u64,
    height_rev: u64,
    since_check: f32,
    /// themes and textures in the current array
    textures: Vec<&'static str>,
    tex_task: Option<(Vec<&'static str>, Task<Result<(Image, Image), String>>)>,
    tex_ready: bool,
    /// the surface style (`art_style::surfaces`) the arrays are in
    tex_art: crate::art_style::ArtStyle,
    /// triangles drawn (for the HUD and logs)
    pub triangles: usize,
    /// x-ray view: cut away everything above this height
    pub xray: Option<f32>,
    /// holes the carving opens, and the cells over carved space (opened in x-ray)
    base_holes: std::collections::HashSet<IVec2>,
    footprint: std::collections::HashSet<IVec2>,
    /// what the terrain holes and shadows were last set for: (x-ray on, build)
    synced: Option<(bool, u64)>,
    /// x-ray: a dark floor under the caves (what shows through the opened ground)
    backdrop: Option<Entity>,
    /// timbers, pools, crystals and webs (`dress`), and the lights of lanterns and crystals
    dress: Vec<Entity>,
    dress_lights: Vec<super::dress::DressLight>,
    lights: Vec<Entity>,
    lights_for: (u64, u64),
    pub timber_mat: Handle<StandardMaterial>,
    /// the cobweb texture
    pub web_tex: Handle<Image>,
    /// what the dressing was last built as (the export reads the meshes back)
    pub dressed: Vec<super::dress::Dress>,
    /// a dungeon's built space (`crate::dungeons`): the questions below answer for it too, so
    /// walking, the camera and the cursor work in it as in a cave
    pub dungeon: Option<crate::dungeons::space::Space>,
}

impl CaveWorld {
    pub fn busy(&self) -> bool {
        self.task.is_some() || self.want != self.built
    }
    /// The terrain cells the caves open (without the x-ray view's).
    pub fn holes(&self) -> &std::collections::HashSet<IVec2> {
        &self.base_holes
    }
}

pub(super) fn init(
    mut world: ResMut<CaveWorld>,
    mut materials: ResMut<Assets<CaveMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    server: Res<AssetServer>,
) {
    // mine timbers: a photoscanned rough-sawn wood; pools: dark still water
    let wood = "textures/materials/wood_rough_sawn_dark";
    let repeat = |linear: bool| {
        move |s: &mut bevy::image::ImageLoaderSettings| {
            s.is_srgb = !linear;
            s.sampler = ImageSampler::Descriptor(sampler());
        }
    };
    let albedo = server
        .load_builder()
        .with_settings(repeat(false))
        .load(format!("{wood}/albedo.png"));
    let normal = server
        .load_builder()
        .with_settings(repeat(true))
        .load(format!("{wood}/normal.png"));
    let orm = server
        .load_builder()
        .with_settings(repeat(true))
        .load(format!("{wood}/orm.png"));
    world.timber_mat = std_mats.add(StandardMaterial {
        base_color_texture: Some(albedo),
        normal_map_texture: Some(normal),
        metallic_roughness_texture: Some(orm.clone()),
        occlusion_texture: Some(orm),
        perceptual_roughness: 1.0,
        metallic: 1.0,
        ..default()
    });
    // the cobweb texture (drawn once, with its mips, so the threads stay fine far off)
    let size = 512u32;
    let base = super::dress::web_texture(size, 0x5eb);
    let lut = crate::terrain::textures::srgb_to_lin_lut();
    let data = crate::terrain::textures::build_mips(&base, size, true, &lut);
    let mut web = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    web.data = Some(data);
    web.texture_descriptor.mip_level_count = size.ilog2() + 1;
    web.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..sampler()
    });
    world.web_tex = images.add(web);
    let (a, d) = (
        images.add(placeholder(true)),
        images.add(placeholder(false)),
    );
    let (ga, gd) = (
        images.add(placeholder(true)),
        images.add(placeholder(false)),
    );
    let mut params = CaveParams {
        look: Vec4::new(1.0, 0.0, 0.0, 0.0),
        misc: Vec4::new(1e9, 0.0, 0.0, 0.0),
        ground_tile: Vec4::new(4.5, 1.6, 1.6, 2.0),
        fill: Vec4::new(0.85, 0.88, 0.95, 0.0),
        ..default()
    };
    set_themes(&mut params, &[]);
    let base = StandardMaterial {
        perceptual_roughness: 0.85,
        reflectance: 0.35,
        ..default()
    };
    let ext = CaveExt {
        params,
        albedo: a,
        detail: d,
        ground_albedo: ga,
        ground_detail: gd,
    };
    world.material = materials.add(ExtendedMaterial {
        base: base.clone(),
        extension: ext.clone(),
    });
    world.material_xray = materials.add(ExtendedMaterial {
        base: StandardMaterial {
            cull_mode: None,
            double_sided: true,
            ..base
        },
        extension: ext,
    });
}

/// Fills the theme table for the textures in `list`.
fn set_themes(p: &mut CaveParams, list: &[&'static str]) {
    for t in Theme::CONCRETE {
        let l = look(t);
        let idx = |id: &str| list.iter().position(|x| *x == id).unwrap_or(0) as f32;
        let tile = |id: &str| tile_of(id);
        p.themes[t.slot()] = ThemeGpu {
            tex: Vec4::new(idx(l.wall[0]), idx(l.wall[1]), idx(l.floor), idx(l.accent)),
            tile: Vec4::new(
                tile(l.wall[0]) * 1.3,
                tile(l.wall[1]) * 1.3,
                tile(l.floor),
                tile(l.accent),
            ),
            tint: Vec4::new(l.tint[0], l.tint[1], l.tint[2], l.damp),
            fx: Vec4::new(l.accent_kind as f32, l.accent_amount, l.gloss, l.contrast),
        };
    }
}

/// Repeat size (m) of a library material (from `library.json`).
fn tile_of(id: &str) -> f32 {
    static TILES: std::sync::OnceLock<HashMap<String, f32>> = std::sync::OnceLock::new();
    let m = TILES.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Doc {
            materials: Vec<crate::terrain::textures::LibraryMaterial>,
        }
        std::fs::read_to_string(crate::terrain::textures::library_dir().join("library.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Doc>(&s).ok())
            .map(|d| d.materials.into_iter().map(|m| (m.id, m.tile_m)).collect())
            .unwrap_or_default()
    });
    m.get(id).copied().unwrap_or(2.0).max(0.4)
}

// ------------------------------------------------------------------ rebuilding

/// Wants a rebuild when the caves change, or (checked twice a second, not mid-stroke) when the
/// ground under them changes.
pub(super) fn track(
    caves: Res<Caves>,
    terrain: Res<Terrain>,
    stroke: Res<crate::tools::Stroke>,
    time: Res<Time<Real>>,
    mut world: ResMut<CaveWorld>,
) {
    if caves.is_changed() {
        world.want += 1;
        return;
    }
    if caves.is_empty() || terrain.height_revision == world.height_rev || stroke.active {
        return;
    }
    world.since_check += time.delta_secs();
    if world.since_check < 0.5 {
        return;
    }
    world.since_check = 0.0;
    world.height_rev = terrain.height_revision;
    let sig = ground_signature(&caves, &terrain);
    if sig != world.ground_sig {
        world.want += 1;
    }
}

fn ground_signature(caves: &Caves, t: &Terrain) -> u64 {
    let g = sdf::ground_for(caves, t);
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for c in &caves.list {
        let (a, b) = c.bounds();
        lo = lo.min(Vec2::new(a.x, a.z));
        hi = hi.max(Vec2::new(b.x, b.z));
    }
    if lo.x > hi.x {
        return 0;
    }
    g.hash_rect(
        g.cell_of(lo.x - 24.0, lo.y - 24.0),
        g.cell_of(hi.x + 24.0, hi.y + 24.0),
    )
}

/// Starts a rebuild in the background when one is wanted (one at a time).
pub(super) fn start(caves: Res<Caves>, terrain: Res<Terrain>, mut world: ResMut<CaveWorld>) {
    if world.task.is_some() || world.want == world.built {
        return;
    }
    let rev = world.want;
    let snapshot = caves.clone();
    let ground = sdf::ground_for(&caves, &terrain);
    world.ground_sig = ground_signature(&caves, &terrain);
    world.height_rev = terrain.height_revision;
    let old: HashMap<IVec4, u64> = world.chunks.iter().map(|(k, (_, h))| (*k, *h)).collect();
    let fills: Vec<f32> = snapshot.list.iter().map(|c| c.darkness.fill()).collect();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let t0 = std::time::Instant::now();
        let vol = Arc::new(sdf::compile_on(&snapshot, ground));
        let t1 = t0.elapsed();
        let mut built = mesher::build(&vol, &old, &fills);
        built.dress = snapshot.list.iter().map(|c| super::dress::dress(c, &vol, &built.light)).collect();
        let tris: usize = built.meshes.iter().map(|m| m.indices.len() / 3).sum();
        info!(
            "CAVES built {} caves: {} shapes, {} holes, {} chunks ({} remeshed, {tris} new triangles) in {:.0} ms (field {:.0} ms)",
            snapshot.list.len(),
            vol.shapes.len(),
            vol.holes.len(),
            built.all.len(),
            built.meshes.len(),
            t0.elapsed().as_secs_f32() * 1000.0,
            t1.as_secs_f32() * 1000.0
        );
        (built, vol)
    });
    world.task = Some((rev, task));
}

fn mesh_of(m: ChunkMesh) -> (Mesh, Aabb) {
    let aabb = Aabb::from_min_max(m.lo - Vec3::splat(0.05), m.hi + Vec3::splat(0.05));
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, m.colors);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs);
    mesh.insert_indices(Indices::U32(m.indices));
    (mesh, aabb)
}

/// Puts a finished rebuild in the world: new chunk meshes, gone chunks despawned, the volume for
/// physics (the terrain's holes follow in `sync_xray`).
pub(super) fn apply(
    mut commands: Commands,
    mut world: ResMut<CaveWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut fx_mats: ResMut<Assets<CaveFxMaterial>>,
    (mut fall_mats, ocean_tex): (
        ResMut<Assets<crate::water::waterfall::WaterfallMaterial>>,
        Option<Res<crate::ocean::OceanTextures>>,
    ),
    caves: Res<Caves>,
    mut q: Query<(&mut Mesh3d, &mut Aabb), With<CaveChunk>>,
) {
    let Some((rev, task)) = world.task.as_mut() else {
        return;
    };
    let Some((built, vol)) = block_on(future::poll_once(task)) else {
        return;
    };
    let rev = *rev;
    world.task = None;
    world.built = rev;
    let mat = if world.xray.is_some() {
        world.material_xray.clone()
    } else {
        world.material.clone()
    };
    let keep: std::collections::HashSet<IVec4> = built.all.iter().map(|(k, _)| *k).collect();
    for m in built.meshes {
        let (key, hash) = (m.key, m.hash);
        let (mesh, aabb) = mesh_of(m);
        let h = meshes.add(mesh);
        if let Some((e, old)) = world.chunks.get_mut(&key) {
            if let Ok((mut m3, mut a)) = q.get_mut(*e) {
                m3.0 = h;
                *a = aabb;
                *old = hash;
                continue;
            }
        }
        let e = commands
            .spawn((
                Name::new(format!(
                    "Cave chunk {},{},{}/{}",
                    key.x, key.y, key.z, key.w
                )),
                CaveChunk(key),
                Mesh3d(h),
                MeshMaterial3d(mat.clone()),
                Transform::IDENTITY,
                aabb,
            ))
            .id();
        world.chunks.insert(key, (e, hash));
    }
    let gone: Vec<IVec4> = world
        .chunks
        .keys()
        .filter(|k| !keep.contains(k))
        .copied()
        .collect();
    for k in gone {
        if let Some((e, _)) = world.chunks.remove(&k) {
            commands.entity(e).despawn();
        }
    }
    // hashes of chunks that kept their mesh
    for (k, h) in &built.all {
        if let Some(c) = world.chunks.get_mut(k) {
            c.1 = *h;
        }
    }
    // timbers, pools, crystals and webs (rebuilt whole: they are small)
    for e in std::mem::take(&mut world.dress) {
        commands.entity(e).despawn();
    }
    world.dress_lights.clear();
    for (i, d) in built.dress.iter().enumerate() {
        let cave = caves.list.get(i);
        let theme = cave.map(|c| c.theme()).unwrap_or(Theme::Limestone);
        let fill = cave.map(|c| c.darkness.fill()).unwrap_or(0.55);
        let lk = look(theme);
        let fill_col = Vec4::new(lk.fill[0], lk.fill[1], lk.fill[2], 0.0);
        let fx = |kind: f32, a: [f32; 4], b: [f32; 3], glow: f32| FxParams {
            kind: Vec4::new(kind, fill, glow, 0.0),
            a: Vec4::from_array(a),
            b: Vec4::new(b[0], b[1], b[2], 0.0),
            fill: fill_col,
        };
        let (shallow, deep) = water_colours(theme);
        let parts: [(
            &super::dress::DressMesh,
            &str,
            Option<(StandardMaterial, FxParams)>,
        ); 4] = [
            (&d.timbers, "Cave timbers", None),
            (
                &d.water,
                "Cave pools",
                Some((
                    StandardMaterial {
                        alpha_mode: AlphaMode::Blend,
                        ..default()
                    },
                    fx(0.0, [shallow[0], shallow[1], shallow[2], 1.0], deep, 0.0),
                )),
            ),
            (
                &d.crystals,
                "Cave crystals",
                Some((
                    StandardMaterial::default(),
                    fx(1.0, [1.0; 4], [0.0; 3], 1.1),
                )),
            ),
            (
                &d.webs,
                "Cave cobwebs",
                Some((
                    StandardMaterial {
                        alpha_mode: AlphaMode::Blend,
                        double_sided: true,
                        cull_mode: None,
                        ..default()
                    },
                    fx(2.0, [0.62, 0.62, 0.6, 0.85], [0.0; 3], 0.0),
                )),
            ),
        ];
        for (m, name, mat) in parts {
            if m.indices.is_empty() {
                continue;
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals.clone());
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs.clone());
            if !m.colors.is_empty() {
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, m.colors.clone());
            }
            mesh.insert_indices(Indices::U32(m.indices.clone()));
            if mat.is_none() {
                // (the timbers' wood has a normal map)
                let _ = mesh.generate_tangents();
            }
            let mut e = commands.spawn((
                Name::new(name),
                Mesh3d(meshes.add(mesh)),
                Transform::IDENTITY,
            ));
            match mat {
                None => {
                    e.insert(MeshMaterial3d(world.timber_mat.clone()));
                }
                Some((base, params)) => {
                    let h = fx_mats.add(ExtendedMaterial {
                        base,
                        extension: CaveFxExt {
                            params,
                            tex: world.web_tex.clone(),
                        },
                    });
                    e.insert((MeshMaterial3d(h), bevy::light::NotShadowCaster));
                }
            }
            let id = e.id();
            world.dress.push(id);
        }
        world.dress_lights.extend(d.lights.iter().copied());
        if let Some(tex) = ocean_tex.as_deref()
            && !d.falls.is_empty()
        {
            let es = crate::water::waterfall::spawn_free(
                &mut commands,
                &mut meshes,
                &mut fall_mats,
                tex,
                &d.falls,
                1.0,
            );
            info!(
                "CAVES {} waterfall(s) in cave {}: {:?}",
                d.falls.len(),
                i,
                d.falls
                    .iter()
                    .map(|f| (f.0.top().to_array(), f.0.bottom().to_array(), f.0.width))
                    .collect::<Vec<_>>()
            );
            world.dress.extend(es);
        }
    }
    world.dressed = built.dress;
    world.base_holes = built.holes;
    world.footprint = built.light.footprint.clone();
    world.synced = None;
    world.light = Some(Arc::new(built.light));
    world.volume = Some(vol);
}

/// The terrain's holes: where the carving opens it, plus everything over the caves in x-ray
/// (look down into them); x-ray also stops the caves casting shadows.
pub(super) fn sync_xray(
    mut commands: Commands,
    mut world: ResMut<CaveWorld>,
    mut terrain: ResMut<Terrain>,
    mut holes: ResMut<crate::terrain::holes::TerrainHoles>,
    mut chunks: Query<
        (
            Entity,
            &mut MeshMaterial3d<CaveMaterial>,
            Has<bevy::light::NotShadowCaster>,
        ),
        With<CaveChunk>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
) {
    let want = (world.xray.is_some(), world.built);
    let mat = if want.0 {
        world.material_xray.clone()
    } else {
        world.material.clone()
    };
    let ok = chunks.iter().all(|(_, m, off)| off == want.0 && m.0 == mat);
    if world.synced == Some(want) && ok {
        return;
    }
    world.synced = Some(want);
    let mut cells = world.base_holes.clone();
    if want.0 {
        cells.extend(world.footprint.iter().copied());
    }
    holes.set(cells, &mut terrain);
    // (the terrain's skirts stay out of the caves)
    holes.set_under(world.footprint.clone(), &mut terrain);
    // x-ray: a dark floor under it all, so the opened ground shows "rock" round the caves
    if let Some(e) = world.backdrop.take() {
        commands.entity(e).despawn();
    }
    if want.0
        && let Some(v) = world.volume.as_ref()
        && let Some((lo, hi)) = v.bounds()
    {
        let size = Vec2::new(hi.x - lo.x, hi.z - lo.z) + 40.0;
        let c = Vec3::new((lo.x + hi.x) * 0.5, lo.y - 6.0, (lo.z + hi.z) * 0.5);
        let e = commands
            .spawn((
                Name::new("Cave x-ray backdrop"),
                Mesh3d(meshes.add(Plane3d::new(Vec3::Y, size * 0.5).mesh())),
                MeshMaterial3d(std_mats.add(StandardMaterial {
                    base_color: Color::srgb(0.035, 0.033, 0.03),
                    unlit: true,
                    ..default()
                })),
                Transform::from_translation(c),
                bevy::light::NotShadowCaster,
            ))
            .id();
        world.backdrop = Some(e);
    }
    for (e, mut m, off) in &mut chunks {
        if m.0 != mat {
            m.0 = mat.clone();
        }
        if want.0 && !off {
            commands.entity(e).insert(bevy::light::NotShadowCaster);
        } else if !want.0 && off {
            commands.entity(e).remove::<bevy::light::NotShadowCaster>();
        }
    }
}

// ------------------------------------------------------------------ textures and parameters

/// Loads the textures of the themes in use (again when the set or the world's art style changes).
pub(super) fn textures(
    caves: Res<Caves>,
    mut world: ResMut<CaveWorld>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CaveMaterial>>,
    meta: Res<crate::world::WorldMeta>,
) {
    let art = crate::art_style::surfaces(meta.art_style);
    if let Some((list, task)) = world.tex_task.as_mut()
        && let Some(res) = block_on(future::poll_once(task))
    {
        let list = list.clone();
        world.tex_task = None;
        match res {
            Ok((a, d)) => {
                let (a, d) = (images.add(a), images.add(d));
                for h in [world.material.clone(), world.material_xray.clone()] {
                    if let Some(mut m) = materials.get_mut(&h) {
                        m.extension.albedo = a.clone();
                        m.extension.detail = d.clone();
                        set_themes(&mut m.extension.params, &list);
                        m.extension.params.misc.w = 1.0;
                    }
                }
                info!("CAVES textures ready: {}", list.join(", "));
                world.textures = list;
                world.tex_ready = true;
            }
            Err(e) => warn!("CAVES textures unavailable: {e}"),
        }
    }
    let restyle = art != world.tex_art && world.tex_ready;
    if world.tex_task.is_some()
        || (!caves.is_changed() && world.tex_ready && !restyle)
        || caves.list.is_empty()
    {
        return;
    }
    let themes: Vec<Theme> = caves.list.iter().map(|c| c.theme()).collect();
    let want = texture_list(&themes);
    // (a superset of what is needed is fine: keep it)
    if want.iter().all(|t| world.textures.contains(t)) && world.tex_ready && !restyle {
        return;
    }
    world.tex_art = art;
    let mut list = world.textures.clone();
    for t in want {
        if !list.contains(&t) {
            list.push(t);
        }
    }
    let ids = list.clone();
    world.tex_task = Some((
        list,
        AsyncComputeTaskPool::get().spawn(async move { load_arrays(&ids, art) }),
    ));
}

/// Lighting and look follow the terrain's material (sun, clouds, rain, detail, art style).
pub(super) fn params(
    world: Res<CaveWorld>,
    gpu: Option<Res<crate::terrain::material::TerrainGpu>>,
    terrain_mats: Res<Assets<crate::terrain::material::TerrainMaterial>>,
    tex: Res<crate::terrain::textures::TerrainTextures>,
    mut materials: ResMut<Assets<CaveMaterial>>,
    time: Res<Time>,
) {
    let Some(gpu) = gpu else { return };
    let Some(tm) = terrain_mats.get(&gpu.material) else {
        return;
    };
    let tp = &tm.extension.params;
    let Some(m) = materials.get(&world.material) else {
        return;
    };
    let mut p = m.extension.params.clone();
    p.sun = tp.sun;
    p.clouds = tp.clouds;
    p.look = Vec4::new(tp.tex.z, tp.tex.w, tp.style2.y, tp.tex.y);
    p.misc.x = world.xray.unwrap_or(1e9);
    p.misc.y = if world.xray.is_some() { 1.0 } else { 0.0 };
    p.misc.z = if tex.albedo.is_some() { 1.0 } else { 0.0 };
    let t = &tex.tiles;
    p.ground_tile = Vec4::new(t[6].max(0.3), t[3].max(0.3), t[5].max(0.3), t[0].max(0.3));
    p.fill.w = time.elapsed_secs();
    let ground = (tex.albedo.clone(), tex.detail.clone());
    let same_tex = matches!(&ground, (Some(a), Some(d)) if m.extension.ground_albedo == *a && m.extension.ground_detail == *d)
        || ground.0.is_none();
    if p == m.extension.params && same_tex {
        return;
    }
    // (time alone changes every frame: push it a few times a second, for the embers' flicker)
    let mut q = p.clone();
    q.fill.w = m.extension.params.fill.w;
    if q == m.extension.params && same_tex && (p.fill.w - m.extension.params.fill.w).abs() < 0.1 {
        return;
    }
    for h in [world.material.clone(), world.material_xray.clone()] {
        let Some(mut m) = materials.get_mut(&h) else {
            continue;
        };
        m.extension.params = p.clone();
        if let (Some(a), Some(d)) = &ground {
            if m.extension.ground_albedo != *a {
                m.extension.ground_albedo = a.clone();
            }
            if m.extension.ground_detail != *d {
                m.extension.ground_detail = d.clone();
            }
        }
    }
}

/// Lights of the caves' lanterns and crystals (they follow the objects).
pub(super) fn sync_lights(
    mut commands: Commands,
    caves: Res<Caves>,
    store: Res<crate::objects::ObjectStore>,
    mut world: ResMut<CaveWorld>,
) {
    let key = (world.built, store.revision);
    if world.lights_for == key {
        return;
    }
    world.lights_for = key;
    for e in std::mem::take(&mut world.lights) {
        commands.entity(e).despawn();
    }
    for c in &caves.list {
        for id in &c.props {
            let Some(p) = store.items.get(id) else {
                continue;
            };
            let pos = Vec3::from_array(p.pos);
            let (color, lm, range, lift) = match p.asset.as_str() {
                // (lumens for the eye adjusted to a cave, `sky::CAVE_EV`)
                super::dress::LANTERN => (Color::srgb(1.0, 0.72, 0.42), 400_000.0, 16.0, 0.35),
                super::dress::CRYSTALS => (Color::srgb(0.45, 0.62, 1.0), 400_000.0, 12.0, 0.9),
                _ => continue,
            };
            let e = commands
                .spawn((
                    Name::new("Cave light"),
                    PointLight {
                        color,
                        intensity: lm,
                        range,
                        radius: 0.1,
                        shadow_maps_enabled: false,
                        ..default()
                    },
                    Transform::from_translation(pos + Vec3::Y * lift),
                ))
                .id();
            world.lights.push(e);
        }
    }
    // the crystals' glow
    for l in world.dress_lights.clone() {
        let color = Color::linear_rgb(l.color[0], l.color[1], l.color[2]);
        let e = commands
            .spawn((
                Name::new("Cave crystal light"),
                PointLight {
                    color,
                    intensity: l.lumens,
                    range: l.range,
                    radius: 0.3,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_translation(l.pos),
            ))
            .id();
        world.lights.push(e);
    }
}
