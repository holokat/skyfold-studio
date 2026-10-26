//! Textured surfaces for built objects (posts, crates, huts, ruins, bridges…): photoscanned CC0
//! materials from `assets/textures/materials/` (wood, metal, stone, fabric, rope…) mapped
//! triplanar in object space, so procedural meshes need no UVs and textures stay put on rotated
//! or scaled objects. Each vertex picks a material slot (vertex colour alpha × 255; 255 keeps the
//! plain vertex colour) and tints it (rgb, ≈1 = the material's own colour). Parts are assigned
//! from the object kind and the part's colour (`classify`), so every builder gets textures
//! without being rewritten.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use bevy::shader::{ShaderDefVal, ShaderRef};
use serde::{Deserialize, Serialize};

use crate::procgen::meshes::AssetKind;

/// Material slots built objects use. The library entry for each is picked by category and tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Planks,
    Beam,
    DarkWood,
    Iron,
    RustyIron,
    Canvas,
    Rope,
    StoneBlocks,
    Cobble,
    Plaster,
    RoofTiles,
    Thatch,
}

impl Slot {
    pub const ALL: [Slot; 12] = [
        Slot::Planks,
        Slot::Beam,
        Slot::DarkWood,
        Slot::Iron,
        Slot::RustyIron,
        Slot::Canvas,
        Slot::Rope,
        Slot::StoneBlocks,
        Slot::Cobble,
        Slot::Plaster,
        Slot::RoofTiles,
        Slot::Thatch,
    ];
    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as u8
    }
    pub fn name(self) -> &'static str {
        match self {
            Slot::Planks => "Planks",
            Slot::Beam => "Beams & posts",
            Slot::DarkWood => "Dark wood",
            Slot::Iron => "Iron",
            Slot::RustyIron => "Rusty iron",
            Slot::Canvas => "Canvas",
            Slot::Rope => "Rope",
            Slot::StoneBlocks => "Stone blocks",
            Slot::Cobble => "Paving",
            Slot::Plaster => "Plaster",
            Slot::RoofTiles => "Roof tiles",
            Slot::Thatch => "Thatch",
        }
    }
    /// Library categories offered for this slot in the editor.
    pub fn categories(self) -> &'static [&'static str] {
        match self {
            Slot::Planks | Slot::Beam | Slot::DarkWood => &["wood"],
            Slot::Iron | Slot::RustyIron => &["metal", "ore", "tech"],
            Slot::Canvas => &["fabric", "leather", "plastic"],
            Slot::Rope => &["rope", "fabric"],
            Slot::StoneBlocks => &["stone", "brick", "ore", "concrete"],
            Slot::Cobble => &["stone", "ceramic", "brick", "concrete"],
            Slot::Plaster => &["plaster", "brick", "stone", "concrete", "facade"],
            Slot::RoofTiles | Slot::Thatch => &["roof", "wood", "metal"],
        }
    }
    /// Library ids in preference order (the bundled CC0 library's best match for the slot).
    fn ids(self) -> &'static [&'static str] {
        match self {
            Slot::Planks => &["wood_planks_weathered", "wood_planks_brown"],
            Slot::Beam => &["wood_oak", "wood_rough_sawn_dark", "wood_pine"],
            Slot::DarkWood => &["wood_planks_dark", "wood_walnut"],
            Slot::Iron => &["metal_iron_wrought", "metal_iron_cast"],
            Slot::RustyIron => &["metal_rust_medium", "metal_rust_heavy"],
            Slot::Canvas => &["fabric_canvas_tent", "fabric_canvas_natural"],
            Slot::Rope => &["rope_hemp"],
            Slot::StoneBlocks => &["stone_blocks_grey", "stone_castle_wall"],
            Slot::Cobble => &["stone_flagstone_floor", "stone_slate_floor"],
            Slot::Plaster => &["plaster_white_rough", "plaster_clay_daub"],
            Slot::RoofTiles => &["roof_clay_tiles", "roof_slate"],
            Slot::Thatch => &["roof_thatch", "roof_thatch_straw"],
        }
    }
    /// Fallback for other libraries: (category, tags in preference order).
    fn wants(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Slot::Planks => ("wood", &["planks", "plank", "weathered", "boards"]),
            Slot::Beam => ("wood", &["beam", "post", "log", "weathered", "rough"]),
            Slot::DarkWood => ("wood", &["dark", "walnut", "painted", "old"]),
            Slot::Iron => ("metal", &["iron", "wrought", "cast", "steel"]),
            Slot::RustyIron => ("metal", &["rust", "rusty", "rusted", "corroded"]),
            Slot::Canvas => ("fabric", &["canvas", "tent", "burlap", "hessian", "linen"]),
            Slot::Rope => ("rope", &["rope", "cord", "twine"]),
            Slot::StoneBlocks => ("stone", &["block", "blocks", "wall", "castle", "cut"]),
            Slot::Cobble => ("stone", &["cobble", "cobblestone", "paving", "flagstone"]),
            Slot::Plaster => ("plaster", &["plaster", "stucco", "render"]),
            Slot::RoofTiles => ("roof", &["tiles", "roof", "shingles", "slate"]),
            Slot::Thatch => ("roof", &["thatch", "straw", "reed"]),
        }
    }
}

/// Which slot a part of a built object gets, from the object kind and the part's colour
/// (linear rgb). `None` keeps the plain vertex colour (small coloured details, glass, fire…).
pub fn classify(kind: AssetKind, c: [f32; 3]) -> Option<Slot> {
    use AssetKind::*;
    let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let sat = if max > 1e-4 { (max - min) / max } else { 0.0 };
    // warm brown: red ≥ green ≥ blue with some saturation
    let brown = c[0] >= c[1] && c[1] >= c[2] * 0.9 && sat > 0.25;
    let grey = sat < 0.18;
    match kind {
        Crate | Barrel | Fence | Signpost | WoodenBridge | Dock | Watchtower | Banner
        | TarpShelter | Campfire | Tent | Hut => {
            if brown {
                Some(if lum < 0.05 {
                    Slot::DarkWood
                } else if matches!(
                    kind,
                    Signpost | Fence | Watchtower | Banner | TarpShelter | Dock
                ) {
                    Slot::Beam
                } else {
                    Slot::Planks
                })
            } else if grey && lum < 0.06 {
                Some(Slot::Iron)
            } else if grey && lum >= 0.06 {
                Some(if matches!(kind, Hut) {
                    Slot::Plaster
                } else {
                    Slot::StoneBlocks
                })
            } else if matches!(kind, Tent | TarpShelter) && lum > 0.15 {
                Some(Slot::Canvas)
            } else if matches!(kind, Hut) && c[1] > c[2] && sat > 0.15 && lum > 0.12 {
                Some(Slot::Thatch)
            } else {
                None
            }
        }
        StoneBridge | RuinPillar | RuinWall | Well => {
            if grey || (sat < 0.3 && lum > 0.12) {
                Some(if matches!(kind, StoneBridge) {
                    Slot::Cobble
                } else {
                    Slot::StoneBlocks
                })
            } else if brown {
                Some(Slot::Beam)
            } else {
                None
            }
        }
        _ => None,
    }
}

pub type SurfaceMaterial = ExtendedMaterial<StandardMaterial, SurfaceExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct SurfaceExt {
    /// metres per texture repeat (u, v) per slot, two slots per vec4
    #[uniform(100)]
    pub tiles: [Vec4; 8],
    #[texture(101, dimension = "2d_array")]
    pub albedo: Handle<Image>,
    #[texture(102, dimension = "2d_array")]
    pub normal: Handle<Image>,
    #[texture(103, dimension = "2d_array")]
    #[sampler(104)]
    pub orm: Handle<Image>,
}

impl MaterialExtension for SurfaceExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/surface.wesl".into()
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // the fragment shader needs the instance's transform for object-space mapping
        let def = ShaderDefVal::Bool("VERTEX_OUTPUT_INSTANCE_INDEX".into(), true);
        descriptor.vertex.shader_defs.push(def.clone());
        if let Some(f) = descriptor.fragment.as_mut() {
            f.shader_defs.push(def);
        }
        Ok(())
    }
}

#[derive(Deserialize, Clone)]
pub struct LibraryEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    tags: Vec<String>,
    /// metres per repeat along u and v
    #[serde(default)]
    tile_size_m: Option<[f32; 2]>,
    #[serde(default = "yes")]
    pub tiling: bool,
    #[serde(default)]
    pub average_color_srgb: Option<[f32; 3]>,
}

fn yes() -> bool {
    true
}

/// The bundled material library (for the editor's pickers).
#[derive(Resource, Default)]
pub struct SurfaceLibrary {
    pub entries: Vec<LibraryEntry>,
}

/// Which library material fills each building slot, by slot index ("" = the default). Saved
/// with the world.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SurfaceChoices {
    pub ids: Vec<String>,
}

impl SurfaceChoices {
    pub fn get(&self, slot: Slot) -> &str {
        self.ids
            .get(slot.index() as usize)
            .map(|s| s.as_str())
            .unwrap_or("")
    }
    pub fn set(&mut self, slot: Slot, id: &str) {
        let i = slot.index() as usize;
        if self.ids.len() <= i {
            self.ids.resize(Slot::ALL.len(), String::new());
        }
        self.ids[i] = id.to_string();
    }
}

/// Loaded surface slots: the shared material and each slot's mean colour (for tints).
#[derive(Resource)]
pub struct Surfaces {
    pub material: Handle<SurfaceMaterial>,
    /// linear mean albedo per slot (None: slot has no texture)
    pub mean: Vec<Option<[f32; 3]>>,
    /// CPU copies of the arrays, so one slot can be swapped without reloading the rest
    alb: Vec<u8>,
    nor: Vec<u8>,
    orm: Vec<u8>,
    tiles: [Vec4; 8],
    applied: SurfaceChoices,
    /// the surface style (`art_style::surfaces`) the arrays hold
    art: crate::art_style::ArtStyle,
    /// the content packs installed when they were read (`content::revision`)
    content: u32,
}

impl Surfaces {
    /// Vertex colour for a part: slot in alpha, tint relative to the slot's own colour in rgb.
    pub fn encode(&self, slot: Slot, c: [f32; 3]) -> [f32; 4] {
        match self.mean.get(slot.index() as usize).copied().flatten() {
            Some(m) => {
                let t = |i: usize| (c[i] / m[i].max(0.02)).clamp(0.55, 1.6);
                // keep the hue shift gentle: blend the tint toward neutral
                let k = 0.6;
                [
                    1.0 + (t(0) - 1.0) * k,
                    1.0 + (t(1) - 1.0) * k,
                    1.0 + (t(2) - 1.0) * k,
                    slot.index() as f32 / 255.0,
                ]
            }
            None => [c[0], c[1], c[2], 1.0],
        }
    }
}

const SIZE: u32 = 512;

fn library_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::assets_path()).join("textures/materials")
}

/// One file of a material ("albedo.png"): the app's, or a downloaded content pack's
/// (`crate::content`; the app keeps every material's swatch).
pub fn item_file(id: &str, name: &str) -> std::path::PathBuf {
    crate::paths::asset_file(&format!("textures/materials/{id}/{name}"))
}

/// Whether a material's files are on this computer (false for a content pack's material that
/// hasn't been downloaded yet: automatic picks pass over it).
pub fn available(id: &str) -> bool {
    crate::content::available(&format!("textures/materials/{id}"))
}

fn read_library() -> Vec<LibraryEntry> {
    std::fs::read_to_string(library_dir().join("library.json"))
        .ok()
        .and_then(|s| {
            // either a list or {"materials": [...]}
            serde_json::from_str::<Vec<LibraryEntry>>(&s)
                .ok()
                .or_else(|| {
                    #[derive(Deserialize)]
                    struct Wrap {
                        materials: Vec<LibraryEntry>,
                    }
                    serde_json::from_str::<Wrap>(&s).ok().map(|w| w.materials)
                })
        })
        .unwrap_or_default()
}

fn layer_bytes() -> usize {
    (0..)
        .take_while(|m| SIZE >> m > 0)
        .map(|m| ((SIZE >> m) * (SIZE >> m) * 4) as usize)
        .sum()
}

/// The library entry for a slot: the world's choice, else the slot's preferred ids, else the
/// first entry matching its category and tags.
fn pick<'a>(
    slot: Slot,
    entries: &'a [LibraryEntry],
    choices: &SurfaceChoices,
) -> Option<&'a LibraryEntry> {
    let chosen = choices.get(slot);
    if !chosen.is_empty()
        && let Some(e) = entries.iter().find(|e| e.id == chosen)
        && available(&e.id)
    {
        return Some(e);
    }
    // (a material still to download is never picked automatically)
    let (cat, tags) = slot.wants();
    slot.ids()
        .iter()
        .find_map(|id| entries.iter().filter(|e| e.tiling && available(&e.id)).find(|e| e.id == *id))
        .or_else(|| {
            tags.iter().find_map(|tag| {
                entries.iter().filter(|e| e.tiling && available(&e.id)).find(|e| {
                    (e.category.eq_ignore_ascii_case(cat)
                        || cat == "plaster" && e.category.eq_ignore_ascii_case("stone"))
                        && (e.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
                            || e.id.to_ascii_lowercase().contains(tag))
                })
            })
        })
}

/// One slot's layer data (albedo, normal, orm with mips), tile size and mean colour, in the
/// given art style (a painted material in a hand-painted world, where there is one).
fn load_layer(
    e: &LibraryEntry,
    lut: &[f32; 256],
    art: crate::art_style::ArtStyle,
) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>, [f32; 2], [f32; 3])> {
    let f = |name: &str| item_file(&e.id, name);
    let files = crate::art_style::textures(
        [f("albedo.png"), f("normal.png"), f("orm.png")],
        art,
    );
    let read = |f: &std::path::Path| {
        image::open(f).ok().map(|i| {
            image::imageops::resize(
                &i.to_rgba8(),
                SIZE,
                SIZE,
                image::imageops::FilterType::Triangle,
            )
        })
    };
    let (a, n, o) = (read(&files[0])?, read(&files[1])?, read(&files[2])?);
    let m = a.pixels().fold([0.0f32; 3], |acc, p| {
        [
            acc[0] + lut[p[0] as usize],
            acc[1] + lut[p[1] as usize],
            acc[2] + lut[p[2] as usize],
        ]
    });
    let k = 1.0 / (SIZE * SIZE) as f32;
    let mips = |img: image::RgbaImage, srgb: bool| {
        crate::terrain::textures::build_mips(&img.into_raw(), SIZE, srgb, lut)
    };
    Some((
        mips(a, true),
        mips(n, false),
        mips(o, false),
        e.tile_size_m.unwrap_or([1.0, 1.0]),
        [m[0] * k, m[1] * k, m[2] * k],
    ))
}

fn set_tile(tiles: &mut [Vec4; 8], si: usize, t: [f32; 2]) {
    let (a0, a1) = if si % 2 == 0 { (0, 1) } else { (2, 3) };
    tiles[si / 2][a0] = t[0].max(0.05);
    tiles[si / 2][a1] = t[1].max(0.05);
}

fn array_image(data: Vec<u8>, format: TextureFormat) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: Slot::ALL.len() as u32,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = SIZE.trailing_zeros() + 1;
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    img
}

/// Builds the slot arrays from the library (missing slots stay untextured). Called once while
/// the object library is built; the world's choices are applied by `apply_choices`.
pub fn load(
    images: &mut Assets<Image>,
    mats: &mut Assets<SurfaceMaterial>,
) -> (Surfaces, SurfaceLibrary) {
    let t0 = std::time::Instant::now();
    let entries = read_library();
    let lut = crate::terrain::textures::srgb_to_lin_lut();
    let n = Slot::ALL.len();
    let per = layer_bytes();
    let none = SurfaceChoices::default();
    // (the library is built before a world opens: realistic; `apply_choices` repaints)
    let art = crate::art_style::ArtStyle::Realistic;
    // decode the slots in parallel
    let layers: Vec<Option<(Vec<u8>, Vec<u8>, Vec<u8>, [f32; 2], [f32; 3])>> =
        std::thread::scope(|sc| {
            let hs: Vec<_> = Slot::ALL
                .iter()
                .map(|slot| {
                    let (entries, lut, none) = (&entries, &lut, &none);
                    sc.spawn(move || {
                        pick(*slot, entries, none).and_then(|e| load_layer(e, lut, art))
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
    let (mut alb, mut nor, mut orm) = (
        Vec::with_capacity(per * n),
        Vec::with_capacity(per * n),
        Vec::with_capacity(per * n),
    );
    let mut tiles = [Vec4::ONE; 8];
    let mut mean = vec![None; n];
    for (si, layer) in layers.into_iter().enumerate() {
        match layer {
            Some((a, nm, o, t, m)) => {
                alb.extend(a);
                nor.extend(nm);
                orm.extend(o);
                set_tile(&mut tiles, si, t);
                mean[si] = Some(m);
            }
            None => {
                // flat placeholders (never sampled: such parts keep their vertex colour)
                alb.extend(std::iter::repeat_n([128u8, 128, 128, 255], per / 4).flatten());
                nor.extend(std::iter::repeat_n([128u8, 128, 255, 255], per / 4).flatten());
                orm.extend(std::iter::repeat_n([255u8, 200, 0, 255], per / 4).flatten());
            }
        }
    }
    let material = mats.add(SurfaceMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.8,
            reflectance: 0.3,
            ..default()
        },
        extension: SurfaceExt {
            tiles,
            albedo: images.add(array_image(alb.clone(), TextureFormat::Rgba8UnormSrgb)),
            normal: images.add(array_image(nor.clone(), TextureFormat::Rgba8Unorm)),
            orm: images.add(array_image(orm.clone(), TextureFormat::Rgba8Unorm)),
        },
    });
    let found = mean.iter().filter(|m| m.is_some()).count();
    info!(
        "surface materials: {found} of {n} slots textured from {} library entries in {:?}",
        entries.len(),
        t0.elapsed()
    );
    (
        Surfaces {
            material,
            mean,
            alb,
            nor,
            orm,
            tiles,
            applied: none,
            art,
            content: crate::content::revision(),
        },
        SurfaceLibrary { entries },
    )
}

pub struct SurfacesPlugin;

impl Plugin for SurfacesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SurfaceChoices>()
            .add_systems(Update, apply_choices);
    }
}

/// Swaps the slots whose chosen material changed (the editor's building-material pickers, a
/// loaded world), or every slot when the world's art style changed.
fn apply_choices(
    choices: Res<SurfaceChoices>,
    surfaces: Option<ResMut<Surfaces>>,
    library: Option<Res<SurfaceLibrary>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<SurfaceMaterial>>,
    meta: Res<crate::world::WorldMeta>,
) {
    let (Some(mut sf), Some(library)) = (surfaces, library) else {
        return;
    };
    // the world's art style changed: every slot is read again in the new style (the slots' mean
    // colours, which existing parts' tints are relative to, stay)
    let art = crate::art_style::surfaces(meta.art_style);
    // (a content pack that landed counts as a restyle: every slot is read again)
    let rev = crate::content::revision();
    let restyle = art != sf.art || rev != sf.content;
    if *choices == sf.applied && !restyle {
        return;
    }
    let lut = crate::terrain::textures::srgb_to_lin_lut();
    let per = layer_bytes();
    let mut changed = false;
    for (si, slot) in Slot::ALL.iter().enumerate() {
        let chosen = choices.get(*slot) != sf.applied.get(*slot);
        if !chosen && !restyle {
            continue;
        }
        let Some((a, n, o, t, m)) =
            pick(*slot, &library.entries, &choices).and_then(|e| load_layer(e, &lut, art))
        else {
            continue;
        };
        let r = si * per..(si + 1) * per;
        sf.alb[r.clone()].copy_from_slice(&a);
        sf.nor[r.clone()].copy_from_slice(&n);
        sf.orm[r].copy_from_slice(&o);
        let mut tiles = sf.tiles;
        set_tile(&mut tiles, si, t);
        sf.tiles = tiles;
        if chosen || sf.mean[si].is_none() {
            sf.mean[si] = Some(m);
        }
        changed = true;
    }
    sf.applied = choices.clone();
    sf.art = art;
    sf.content = rev;
    if !changed {
        return;
    }
    let (alb, nor, orm, tiles) = (
        array_image(sf.alb.clone(), TextureFormat::Rgba8UnormSrgb),
        array_image(sf.nor.clone(), TextureFormat::Rgba8Unorm),
        array_image(sf.orm.clone(), TextureFormat::Rgba8Unorm),
        sf.tiles,
    );
    if let Some(mut m) = mats.get_mut(&sf.material) {
        m.extension.tiles = tiles;
        m.extension.albedo = images.add(alb);
        m.extension.normal = images.add(nor);
        m.extension.orm = images.add(orm);
    }
}
