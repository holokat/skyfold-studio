//! Small flora that needs a real, style-specific material rather than a vertex-colour stand-in.
//!
//! The module is deliberately parallel to `photoscan`: a compact atlas, three stable LODs per
//! deterministic variant, ordinary foliage attributes, and `TreeParts` as the library-facing
//! shape.  The realistic atlas is reserved for documented photographic source material; the
//! hand-painted atlas is a separate authored texture set.  They share a cell layout so geometry
//! never depends on which art style is active.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::art_style::ArtStyle;
use crate::fields::plants::{FieldFlora, FloraGeometry, Regions, flora_geometry};
use crate::objects::{FoliageExt, FoliageMaterial};
use crate::procgen::meshes::AssetKind;
use crate::terrain::textures::{build_mips, srgb_to_lin_lut};
use crate::trees::{TreeParts, TreeVariant};

/// The flora atlas contract.  Both art styles provide this same 2048² layout, with an 8 px
/// painted bleed around each cell.  UV rectangles point inside that gutter.
pub const ATLAS_SIZE: u32 = 2048;
pub const ATLAS_GUTTER: u32 = 8;
pub const ATLAS_ALBEDO: &str = "flora_albedo.png";
pub const ATLAS_NORMAL: &str = "flora_normal.png";

/// A rectangle in the shared flora atlas.
#[derive(Clone, Copy, Debug)]
pub struct Region(pub [f32; 4]);

impl Region {
    const fn cell(x: u32, y: u32) -> Self {
        let inset = ATLAS_GUTTER as f32 / ATLAS_SIZE as f32;
        let u0 = x as f32 * 0.25 + inset;
        let v0 = y as f32 * 0.25 + inset;
        let u1 = (x + 1) as f32 * 0.25 - inset;
        let v1 = (y + 1) as f32 * 0.25 - inset;
        Self([u0, v0, u1, v1])
    }

    fn uv(self, u: f32, v: f32) -> [f32; 2] {
        [
            self.0[0] + (self.0[2] - self.0[0]) * u,
            self.0[1] + (self.0[3] - self.0[1]) * v,
        ]
    }
}

/// Atlas cells, row-major.  This is the source-of-truth for source art and mesh UVs.
#[derive(Clone, Copy, Debug)]
pub struct FloraRegions {
    // row 0
    pub mushroom_cap: Region,
    pub mushroom_gills: Region,
    pub mushroom_stem: Region,
    pub wildflower_petal: Region,
    // row 1
    pub wildflower_leaf: Region,
    pub lavender_spike: Region,
    pub lavender_leaf: Region,
    pub strawberry_leaf: Region,
    // row 2
    pub strawberry_berry: Region,
    pub strawberry_flower: Region,
    pub blueberry_leaf: Region,
    pub blueberry_berry: Region,
    // row 3
    pub blackberry_leaf: Region,
    pub blackberry_berry: Region,
    pub leaf_clump: Region,
    /// The unused leaf-clump cell is a red-cap photograph in the realistic atlas.  It stays
    /// aliased so field crops can continue to name their own leaf-clump surface.
    pub redcap: Region,
    pub spare_detail: Region,
}

pub const REGIONS: FloraRegions = FloraRegions {
    mushroom_cap: Region::cell(0, 0),
    mushroom_gills: Region::cell(1, 0),
    mushroom_stem: Region::cell(2, 0),
    wildflower_petal: Region::cell(3, 0),
    wildflower_leaf: Region::cell(0, 1),
    lavender_spike: Region::cell(1, 1),
    lavender_leaf: Region::cell(2, 1),
    strawberry_leaf: Region::cell(3, 1),
    strawberry_berry: Region::cell(0, 2),
    strawberry_flower: Region::cell(1, 2),
    blueberry_leaf: Region::cell(2, 2),
    blueberry_berry: Region::cell(3, 2),
    blackberry_leaf: Region::cell(0, 3),
    blackberry_berry: Region::cell(1, 3),
    leaf_clump: Region::cell(2, 3),
    redcap: Region::cell(2, 3),
    spare_detail: Region::cell(3, 3),
};

/// The yellow heart inside the strawberry-flower cell.  Wildflower heads use this compact crop
/// rather than mapping the unrelated green spare-detail cell over their centres.
const WILDFLOWER_CENTRE: Region = Region([0.3375, 0.5875, 0.4125, 0.6625]);

/// Atlas paths are intentionally explicit: no painted texture falls back to a photographic one.
pub fn atlas_dir(art: ArtStyle) -> PathBuf {
    let style = if art.painted() {
        "handpainted"
    } else {
        "realistic"
    };
    PathBuf::from(crate::assets_path())
        .join("models")
        .join("flora")
        .join(style)
}

pub fn atlas_paths(art: ArtStyle) -> [PathBuf; 2] {
    let dir = atlas_dir(art);
    [dir.join(ATLAS_ALBEDO), dir.join(ATLAS_NORMAL)]
}

/// Kinds that are supplied by this module in a given art style.
pub fn kinds(_art: ArtStyle) -> &'static [AssetKind] {
    use AssetKind::*;
    &[
        Flowers,
        Mushrooms,
        Lavender,
        WildStrawberry,
        Blueberry,
        Blackberry,
    ]
}

/// The mesh family and atlas change at a style switch.  The library must rebuild its
/// `TreeParts` (not merely swap a texture) whenever this returns true.
pub fn needs_library_rebuild(before: ArtStyle, after: ArtStyle) -> bool {
    before != after
}

/// Adds flora `TreeParts` to the object build map.  It is called after `photoscan::build` so the
/// compact, rooted wildflower family can replace the hard-to-see single-LOD scan in both styles.
pub fn build_library(
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    parts: &mut HashMap<AssetKind, TreeParts>,
    art: ArtStyle,
) {
    let Some([albedo, normal]) = load_atlas(art) else {
        let [a, n] = atlas_paths(art);
        warn!(
            "flora {} atlas unavailable (expected {} and {}); flora kinds were not installed",
            art.id(),
            a.display(),
            n.display()
        );
        return;
    };
    let material = foliage.add(FoliageMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(images.add(albedo)),
            normal_map_texture: Some(images.add(normal)),
            perceptual_roughness: if art.painted() { 0.78 } else { 0.7 },
            reflectance: if art.painted() { 0.22 } else { 0.28 },
            alpha_mode: AlphaMode::Mask(0.5),
            cull_mode: None,
            double_sided: true,
            diffuse_transmission: if art.painted() { 0.18 } else { 0.3 },
            ..default()
        },
        extension: FoliageExt::default(),
    });

    for &kind in kinds(art) {
        let variants = (0..variant_count(kind))
            .map(|variant| {
                let lods = (0..3)
                    .map(|lod| meshes.add(mesh(kind, variant, lod, art)))
                    .map(|mesh| (mesh, None))
                    .collect();
                let (height, width) = dimensions(kind, variant);
                TreeVariant {
                    lods,
                    cell: None,
                    height,
                    width,
                    min_y: 0.0,
                    trunk: None,
                }
            })
            .collect();
        parts.insert(
            kind,
            TreeParts {
                variants,
                bark: material.clone(),
                leaves: material.clone(),
                ranges: ranges(kind),
            },
        );
    }
}

/// The two small-flora libraries retained as cheap handle copies.  Meshes and images are built
/// once during startup; changing art style swaps only the affected `LibraryEntry::tree` values.
/// The source scans stay loaded by `photoscan`; the active Flowers entry uses the more legible,
/// rooted atlas geometry so both styles have equivalent LODs and placement behaviour.
#[derive(Resource)]
pub struct FloraStyles {
    active: ArtStyle,
    realistic: HashMap<AssetKind, TreeParts>,
    handpainted: HashMap<AssetKind, TreeParts>,
}

/// Builds both style-specific flora libraries and installs the requested style in `parts`.
/// Call this once, immediately after `photoscan::build`, then store the returned resource with
/// `commands.insert_resource(styles)`.
pub fn build_styles(
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    parts: &mut HashMap<AssetKind, TreeParts>,
    active: ArtStyle,
) -> FloraStyles {
    let mut realistic = HashMap::new();
    // This captures the scanned Flowers entry that photoscan installed.  No source is invented
    // if it is unavailable; normal object-library handling remains responsible for that case.
    for &kind in flora_kinds() {
        if let Some(part) = parts.get(&kind) {
            realistic.insert(kind, copy_tree_parts(part));
        }
    }
    build_library(meshes, foliage, images, &mut realistic, ArtStyle::Realistic);

    let mut handpainted = HashMap::new();
    build_library(
        meshes,
        foliage,
        images,
        &mut handpainted,
        ArtStyle::HandPainted,
    );

    let styles = FloraStyles {
        active,
        realistic,
        handpainted,
    };
    styles.install(parts, active);
    styles
}

impl FloraStyles {
    fn selected(&self, art: ArtStyle) -> &HashMap<AssetKind, TreeParts> {
        if art.painted() {
            &self.handpainted
        } else {
            &self.realistic
        }
    }

    fn install(&self, parts: &mut HashMap<AssetKind, TreeParts>, art: ArtStyle) {
        for (&kind, tree) in self.selected(art) {
            parts.insert(kind, copy_tree_parts(tree));
        }
    }

    fn swap_into(&mut self, library: &mut crate::objects::Library, art: ArtStyle) -> bool {
        if !needs_library_rebuild(self.active, art) {
            return false;
        }
        let selected = self.selected(art);
        for entry in &mut library.entries {
            let Some(kind) = entry.kind else { continue };
            let Some(tree) = selected.get(&kind) else {
                continue;
            };
            entry.height = tree
                .variants
                .first()
                .map(|v| v.height)
                .unwrap_or(entry.height);
            entry.meshes = tree
                .variants
                .iter()
                .map(|v| {
                    (
                        v.lods[0].0.clone(),
                        v.lods[v.lods.len().min(2) - 1].0.clone(),
                    )
                })
                .collect();
            entry.tree = Some(copy_tree_parts(tree));
        }
        self.active = art;
        true
    }
}

/// Watches `WorldMeta::art_style` and swaps only the cached small-flora entries.  Re-touching
/// nature makes the near renderer rebuild its plant cells; placed flora are respawned through
/// the ordinary object sync rather than rebuilding scans or the complete library.
pub struct FloraPlugin;

impl Plugin for FloraPlugin {
    fn build(&self, app: &mut App) {
        // `Rules` snapshots `kind_variants`; update the entry trees first so it sees the
        // painted flower count instead of a realistic scan's variants (and vice versa).
        app.add_systems(Update, sync_style.before(crate::nature::init_rules));
    }
}

fn sync_style(
    meta: Res<crate::world::WorldMeta>,
    styles: Option<ResMut<FloraStyles>>,
    library: Option<ResMut<crate::objects::Library>>,
    mut nature: Option<ResMut<crate::nature::NatureField>>,
    mut near: Option<ResMut<crate::nature::render::Near>>,
    mut store: Option<ResMut<crate::objects::ObjectStore>>,
) {
    let (Some(mut styles), Some(mut library)) = (styles, library) else {
        return;
    };
    if !styles.swap_into(&mut library, meta.art_style) {
        return;
    }
    if let Some(field) = &mut nature {
        field.touch(Rect::from_center_half_size(Vec2::ZERO, Vec2::splat(1.0e6)));
    }
    if let Some(near) = &mut near {
        // Asset handles are intentionally absent from the instance content hash.  Invalidate the
        // plant category explicitly so unchanged placements still receive the other mesh family.
        near.invalidate_assets(2);
    }
    if let Some(store) = &mut store {
        refresh_placed_flora(store);
    }
}

fn refresh_placed_flora(store: &mut crate::objects::ObjectStore) {
    let plants: Vec<_> = store
        .items
        .values()
        .filter(|placed| is_flora_asset(&placed.asset))
        .cloned()
        .collect();
    // Use the same replacement transaction as an ordinary object edit so spatial indexing,
    // streaming, render queues and selection stay in sync while the library changes styles.
    for plant in plants {
        store.update(plant);
    }
}

fn flora_kinds() -> &'static [AssetKind] {
    use AssetKind::*;
    &[
        Flowers,
        Mushrooms,
        Lavender,
        WildStrawberry,
        Blueberry,
        Blackberry,
    ]
}

fn is_flora_asset(id: &str) -> bool {
    flora_kinds()
        .iter()
        .any(|&kind| crate::objects::builtin_id(kind) == id)
}

fn copy_tree_parts(source: &TreeParts) -> TreeParts {
    TreeParts {
        variants: source
            .variants
            .iter()
            .map(|variant| TreeVariant {
                lods: variant.lods.clone(),
                cell: variant.cell,
                height: variant.height,
                width: variant.width,
                min_y: variant.min_y,
                trunk: variant.trunk,
            })
            .collect(),
        bark: source.bark.clone(),
        leaves: source.leaves.clone(),
        ranges: source.ranges,
    }
}

/// CPU-side form used by exporters.  Root integration can concatenate this with
/// `photoscan::export_props`: unlike the scanner source, all data comes from this module's atlas
/// and deterministic mesh builder.
pub fn export_props(kinds: &[AssetKind], art: ArtStyle) -> Vec<crate::photoscan::PropExport> {
    let Some((albedo, normal)) = load_rgba_pair(art) else {
        return vec![];
    };
    kinds
        .iter()
        .copied()
        .filter(|k| kinds_for_export(art).contains(k))
        .map(|kind| crate::photoscan::PropExport {
            variants: (0..variant_count(kind))
                .map(|variant| (0..3).map(|lod| mesh(kind, variant, lod, art)).collect())
                .collect(),
            albedo: (albedo.0, albedo.1, albedo.2.clone()),
            normal: Some((normal.0, normal.1, normal.2.clone())),
            tiling: false,
        })
        .collect()
}

fn kinds_for_export(art: ArtStyle) -> &'static [AssetKind] {
    kinds(art)
}

fn variant_count(kind: AssetKind) -> u32 {
    match kind {
        AssetKind::Mushrooms => 4,
        AssetKind::Flowers => 4,
        AssetKind::Lavender | AssetKind::WildStrawberry => 3,
        AssetKind::Blueberry | AssetKind::Blackberry => 4,
        _ => 1,
    }
}

fn ranges(kind: AssetKind) -> [f32; 4] {
    match kind {
        AssetKind::Mushrooms => [18.0, 44.0, 78.0, 78.0],
        AssetKind::Flowers | AssetKind::Lavender => [20.0, 48.0, 78.0, 78.0],
        AssetKind::WildStrawberry | AssetKind::Blueberry | AssetKind::Blackberry => {
            [16.0, 40.0, 68.0, 68.0]
        }
        _ => [20.0, 48.0, 78.0, 78.0],
    }
}

fn dimensions(kind: AssetKind, variant: u32) -> (f32, f32) {
    let v = (variant % 4) as f32;
    match kind {
        AssetKind::Mushrooms => (0.32 + v * 0.035, 0.5 + v * 0.08),
        AssetKind::Flowers => (0.72, 0.58),
        AssetKind::Lavender => (0.78, 0.48),
        AssetKind::WildStrawberry => (0.26, 0.42),
        AssetKind::Blueberry => (0.85, 0.78),
        AssetKind::Blackberry => (1.05, 1.0),
        _ => (0.5, 0.5),
    }
}

fn field_regions() -> Regions {
    let mut r = Regions::new();
    for (name, region) in [
        ("lavender_spike", REGIONS.lavender_spike),
        ("lavender_leaf", REGIONS.lavender_leaf),
        ("strawberry_leaf", REGIONS.strawberry_leaf),
        ("strawberry_berry", REGIONS.strawberry_berry),
        ("strawberry_calyx", REGIONS.wildflower_leaf),
        ("strawberry_flower", REGIONS.strawberry_flower),
        ("green_stem", REGIONS.spare_detail),
    ] {
        r.insert(name.to_string(), region.0);
    }
    r
}

fn mesh(kind: AssetKind, variant: u32, lod: u32, art: ArtStyle) -> Mesh {
    let lod = lod.min(2) as u8;
    let geometry = geometry(kind, variant, lod, art);
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, geometry.pos);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, geometry.nor);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, geometry.uv);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        geometry
            .color
            .iter()
            .zip(&geometry.sway)
            .map(|(color, sway)| [color[0], color[1], color[2], *sway])
            .collect::<Vec<_>>(),
    );
    mesh.insert_indices(Indices::U32(geometry.idx));
    if let Err(e) = mesh.generate_tangents() {
        warn!("flora tangents {kind:?} v{variant} LOD{lod}: {e}");
    }
    mesh
}

fn geometry(kind: AssetKind, variant: u32, lod: u8, art: ArtStyle) -> Geometry {
    match kind {
        AssetKind::Mushrooms => mushrooms(variant, lod, art),
        AssetKind::Flowers => wildflowers(variant, lod, art),
        AssetKind::Lavender => lavender(variant, lod),
        AssetKind::WildStrawberry => wild_strawberry(variant, lod),
        AssetKind::Blueberry => berry_bush(variant, lod, false),
        AssetKind::Blackberry => berry_bush(variant, lod, true),
        _ => Geometry::default(),
    }
}

fn field_geometry(kind: FieldFlora, variant: u32, lod: u8) -> Geometry {
    let seed = 0x6A09_E667_F3BC_C909u64 ^ (variant as u64 * 0x9E37_79B9);
    let FloraGeometry {
        pos,
        nor,
        uv,
        sway,
        idx,
        ..
    } = flora_geometry(kind, seed, lod, &field_regions());
    let mut geometry = Geometry {
        color: vec![[1.0, 1.0, 1.0]; pos.len()],
        pos,
        nor,
        uv,
        sway,
        idx,
        tint: Vec3::ONE,
    };
    geometry.normalize_copied_triangles();
    geometry
}

/// The field recipe supplies the close-packed trifoliate rosette.  Its fruit sits deliberately
/// under that canopy for a crop view, so add a few longer, side-hung fruiting stems here for the
/// library plant.  They keep wild strawberries low while leaving ripe fruit visible from a
/// walking camera.
fn wild_strawberry(variant: u32, lod: u8) -> Geometry {
    let mut g = field_geometry(FieldFlora::WildStrawberry, variant, lod);
    let fruit_count = [3usize, 2, 1][lod as usize];
    let phase = variant as f32 * 1.731;
    for i in 0..fruit_count {
        let a = phase + i as f32 / fruit_count as f32 * std::f32::consts::TAU;
        let outward = Vec3::new(a.cos(), 0.0, a.sin());
        let peak = outward * 0.055 + Vec3::Y * (0.105 + 0.012 * (i % 2) as f32);
        let fruit = outward * (0.145 + 0.014 * (i % 2) as f32)
            + Vec3::Y * (0.070 + 0.008 * ((i + variant as usize) % 2) as f32);
        let old = g.set_tint(Vec3::new(0.72, 0.84, 0.56));
        tube(
            &mut g,
            Vec3::Y * 0.012,
            peak,
            0.0025,
            0.0018,
            [5, 4, 3][lod as usize],
            REGIONS.spare_detail,
            0.10,
            0.48,
        );
        tube(
            &mut g,
            peak,
            fruit + Vec3::Y * 0.018,
            0.0018,
            0.0012,
            [5, 4, 3][lod as usize],
            REGIONS.spare_detail,
            0.48,
            0.66,
        );
        g.set_tint(Vec3::new(1.0, 0.92 + 0.03 * (i % 2) as f32, 0.92));
        sphere(
            &mut g,
            fruit,
            Vec3::new(0.022, 0.030, 0.022),
            [8, 6, 4][lod as usize],
            [5, 4, 3][lod as usize],
            REGIONS.strawberry_berry,
            0.72,
        );
        g.set_tint(Vec3::new(0.70, 0.83, 0.48));
        for calyx in 0..3 {
            let turn = a + calyx as f32 / 3.0 * std::f32::consts::TAU;
            leaf_card(
                &mut g,
                fruit + Vec3::Y * 0.020,
                Vec3::new(turn.cos(), 0.28, turn.sin()),
                0.024,
                0.010,
                REGIONS.wildflower_leaf,
                0.66,
            );
        }
        g.set_tint(old);
    }
    g
}

/// A wild lavender clump.  The crop recipe's lavender spike is deliberately not reused here:
/// its crossed ribbons were designed for an alpha atlas, while the small-flora atlas has an
/// opaque macro-floret surface.  It therefore uses spaced whorls of small floret lobes instead
/// of a continuous cone, preserving the gaps and ruffled outline of a real lavender spike.
fn lavender(variant: u32, lod: u8) -> Geometry {
    let mut g = Geometry::default();
    let leaf_count = [13usize, 7, 3][lod as usize];
    let spike_count = [9usize, 5, 3][lod as usize];
    let phase = variant as f32 * 1.721;

    let old = g.set_tint(Vec3::new(0.72, 0.82, 0.56));
    for i in 0..leaf_count {
        let a = phase + i as f32 * 2.399_96;
        let out = (i as f32 / leaf_count as f32).sqrt();
        let base = Vec3::new(a.cos(), 0.0, a.sin()) * out * 0.045;
        let dir = Vec3::new(
            a.cos() * (0.65 + out * 0.2),
            0.28,
            a.sin() * (0.65 + out * 0.2),
        );
        leaf_card(
            &mut g,
            base,
            dir,
            0.17 + 0.08 * (1.0 - out),
            0.028,
            REGIONS.lavender_leaf,
            0.54,
        );
    }
    g.set_tint(old);

    let old = g.set_tint(Vec3::new(0.56, 0.70, 0.36));
    for i in 0..spike_count {
        let a = phase + i as f32 * 2.399_96;
        let out = (i as f32 / spike_count as f32).sqrt();
        let base = Vec3::new(a.cos(), 0.0, a.sin()) * out * 0.075;
        let height = 0.47 + 0.11 * ((i + variant as usize) % 3) as f32;
        let bend = 0.06 + 0.10 * out;
        let axis = Vec3::new(a.cos() * bend, 1.0, a.sin() * bend).normalize();
        let flower_base = base + axis * (height - 0.16);
        let flower_tip = base + axis * height;
        tube(
            &mut g,
            base,
            flower_tip - axis * 0.006,
            0.008,
            0.0025,
            [5, 4, 3][lod as usize],
            REGIONS.spare_detail,
            0.0,
            0.68,
        );
        let violet = [
            Vec3::new(0.79, 0.62, 0.96),
            Vec3::new(0.67, 0.50, 0.88),
            Vec3::new(0.86, 0.67, 0.98),
        ][variant as usize % 3];
        g.set_tint(violet);
        lavender_spike(
            &mut g,
            flower_base,
            flower_tip,
            0.027,
            REGIONS.lavender_spike,
            lod,
            0.68,
            0.92,
        );
        g.set_tint(old);
    }
    g.set_tint(Vec3::ONE);
    g
}

struct Geometry {
    pos: Vec<[f32; 3]>,
    nor: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    color: Vec<[f32; 3]>,
    sway: Vec<f32>,
    idx: Vec<u32>,
    tint: Vec3,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            pos: vec![],
            nor: vec![],
            uv: vec![],
            color: vec![],
            sway: vec![],
            idx: vec![],
            tint: Vec3::ONE,
        }
    }
}

impl Geometry {
    fn vert(&mut self, p: Vec3, n: Vec3, uv: [f32; 2], sway: f32) -> u32 {
        self.pos.push(p.to_array());
        self.nor.push(n.normalize_or(Vec3::Y).to_array());
        self.uv.push(uv);
        self.color.push(self.tint.to_array());
        self.sway.push(sway.clamp(0.0, 1.0));
        (self.pos.len() - 1) as u32
    }

    fn tri(&mut self, a: u32, b: u32, c: u32) {
        // Small-flora builders mix radial surfaces, upright cards and downward-facing gills.
        // Keep their declared shading normals and their front-face winding in agreement rather
        // than relying on a renderer's double-sided backface handling to repair dark lighting.
        let pa = Vec3::from_array(self.pos[a as usize]);
        let pb = Vec3::from_array(self.pos[b as usize]);
        let pc = Vec3::from_array(self.pos[c as usize]);
        let na = Vec3::from_array(self.nor[a as usize]);
        let nb = Vec3::from_array(self.nor[b as usize]);
        let nc = Vec3::from_array(self.nor[c as usize]);
        if (pb - pa).cross(pc - pa).dot(na + nb + nc) < 0.0 {
            self.idx.extend_from_slice(&[a, c, b]);
        } else {
            self.idx.extend_from_slice(&[a, b, c]);
        }
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    /// Field recipes hand us an index stream built for their own renderer. Re-emit it through
    /// this module's winding rule, dropping only exact zero-area triangles from collapsed field
    /// details. Very small but positive faces remain part of the flora mesh.
    fn normalize_copied_triangles(&mut self) {
        let copied = std::mem::take(&mut self.idx);
        debug_assert_eq!(copied.len() % 3, 0);
        for face in copied.chunks_exact(3) {
            let [a, b, c] = [face[0], face[1], face[2]];
            let pa = Vec3::from_array(self.pos[a as usize]);
            let pb = Vec3::from_array(self.pos[b as usize]);
            let pc = Vec3::from_array(self.pos[c as usize]);
            if (pb - pa).cross(pc - pa).length_squared() > 0.0 {
                self.tri(a, b, c);
            }
        }
    }

    fn set_tint(&mut self, tint: Vec3) -> Vec3 {
        std::mem::replace(&mut self.tint, tint)
    }

    fn append(&mut self, other: Geometry) {
        let base = self.pos.len() as u32;
        self.pos.extend(other.pos);
        self.nor.extend(other.nor);
        self.uv.extend(other.uv);
        self.color.extend(other.color);
        self.sway.extend(other.sway);
        self.idx.extend(other.idx.into_iter().map(|i| i + base));
    }
}

fn tube(
    g: &mut Geometry,
    a: Vec3,
    b: Vec3,
    r0: f32,
    r1: f32,
    sides: u32,
    tex: Region,
    sway0: f32,
    sway1: f32,
) {
    let axis = (b - a).normalize_or(Vec3::Y);
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u).normalize_or(Vec3::Z);
    let mut rings = [Vec::new(), Vec::new()];
    for (ri, (p, radius, sway)) in [(a, r0, sway0), (b, r1, sway1)].into_iter().enumerate() {
        for k in 0..=sides {
            let f = k as f32 / sides as f32;
            let angle = f * std::f32::consts::TAU;
            let radial = (u * angle.cos() + v * angle.sin()).normalize();
            let mut point = p + radial * radius;
            // A leaning stem has a horizontal ground cut rather than a tilted base ring
            // penetrating below its root origin.
            point.y = point.y.max(0.0);
            rings[ri].push(g.vert(point, radial, tex.uv(f, ri as f32), sway));
        }
    }
    for k in 0..sides as usize {
        g.quad(rings[0][k], rings[1][k], rings[1][k + 1], rings[0][k + 1]);
    }
}

fn lavender_spike(
    g: &mut Geometry,
    base: Vec3,
    tip: Vec3,
    base_radius: f32,
    tex: Region,
    lod: u8,
    sway0: f32,
    sway1: f32,
) {
    let axis = (tip - base).normalize_or(Vec3::Y);
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u).normalize_or(Vec3::Z);
    let tiers = [5usize, 3, 2][lod as usize];
    for tier in 0..tiers {
        let t = (tier as f32 + 0.45) / (tiers as f32 + 0.20);
        let lobes = match lod {
            0 => [5usize, 4, 4, 3, 2][tier],
            1 => [4, 3, 2][tier],
            _ => [3, 2][tier],
        };
        let centre = base.lerp(tip, t);
        let ring_radius = base_radius * (1.0 - t).powf(0.72) * 0.82 + 0.003;
        let lobe_radius = [0.018, 0.016, 0.014][lod as usize] * (1.08 - t * 0.22);
        for lobe in 0..lobes {
            // Offset adjacent tiers so the cluster does not read as stacked regular beads.
            let angle = (lobe as f32 + if tier % 2 == 0 { 0.14 } else { 0.57 }) / lobes as f32
                * std::f32::consts::TAU;
            let radial = (u * angle.cos() + v * angle.sin()).normalize();
            let at = centre + radial * ring_radius + axis * (lobe_radius * 0.12);
            let stretch = 1.05 + 0.18 * ((lobe + tier) % 2) as f32;
            sphere(
                g,
                at,
                Vec3::new(
                    lobe_radius * 0.88,
                    lobe_radius * stretch,
                    lobe_radius * 0.88,
                ),
                [6, 5, 4][lod as usize],
                [4, 3, 2][lod as usize],
                tex,
                sway0.lerp(sway1, t),
            );
        }
    }
}

fn leaf_card(
    g: &mut Geometry,
    base: Vec3,
    dir: Vec3,
    length: f32,
    width: f32,
    tex: Region,
    sway: f32,
) {
    let forward = dir.normalize_or(Vec3::X);
    let side = forward.cross(Vec3::Y).normalize_or(Vec3::Z);
    let normal = side.cross(forward).normalize_or(Vec3::Y);
    let shoulder = base + forward * length * 0.48 + Vec3::Y * length * 0.025;
    let tip = base + forward * length;
    let arch = Vec3::Y * length * 0.075;
    // Atlas leaves are cutouts with the petiole at the image bottom.  Give their envelope a
    // narrow attachment, a broad middle and a pointed tip, while mapping base -> v=1 and tip
    // -> v=0.  The old one-quad card inverted those source leaves and pinched them all the way.
    let a = g.vert(
        base - side * width * 0.12,
        normal,
        tex.uv(0.42, 0.96),
        sway * 0.4,
    );
    let b = g.vert(
        base + side * width * 0.12,
        normal,
        tex.uv(0.58, 0.96),
        sway * 0.4,
    );
    let c = g.vert(
        shoulder + side * width * 0.5,
        normal,
        tex.uv(0.94, 0.52),
        sway * 0.68,
    );
    let d = g.vert(
        shoulder - side * width * 0.5,
        normal,
        tex.uv(0.06, 0.52),
        sway * 0.68,
    );
    let e = g.vert(
        tip + side * width * 0.04 + arch,
        normal,
        tex.uv(0.54, 0.04),
        sway,
    );
    let f = g.vert(
        tip - side * width * 0.04 + arch,
        normal,
        tex.uv(0.46, 0.04),
        sway,
    );
    g.quad(a, b, c, d);
    g.quad(d, c, e, f);
}

fn sphere(
    g: &mut Geometry,
    centre: Vec3,
    radius: Vec3,
    lon: u32,
    lat: u32,
    tex: Region,
    sway: f32,
) {
    debug_assert!(lon >= 3 && lat >= 2);
    let top = g.vert(centre + Vec3::Y * radius.y, Vec3::Y, tex.uv(0.5, 0.0), sway);
    let mut rings = Vec::with_capacity((lat - 1) as usize);
    for y in 1..lat {
        let theta = y as f32 / lat as f32 * std::f32::consts::PI;
        let mut ring = Vec::with_capacity((lon + 1) as usize);
        for x in 0..=lon {
            let phi = x as f32 / lon as f32 * std::f32::consts::TAU;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            ring.push(g.vert(
                centre + n * radius,
                n,
                tex.uv(x as f32 / lon as f32, y as f32 / lat as f32),
                sway,
            ));
        }
        rings.push(ring);
    }
    for x in 0..lon as usize {
        g.tri(top, rings[0][x], rings[0][x + 1]);
    }
    for y in 0..rings.len() - 1 {
        for x in 0..lon {
            let x = x as usize;
            g.quad(
                rings[y][x],
                rings[y + 1][x],
                rings[y + 1][x + 1],
                rings[y][x + 1],
            );
        }
    }
    let bottom = g.vert(
        centre - Vec3::Y * radius.y,
        -Vec3::Y,
        tex.uv(0.5, 1.0),
        sway,
    );
    let last = rings.last().expect("lat >= 2");
    for x in 0..lon as usize {
        g.tri(bottom, last[x + 1], last[x]);
    }
}

fn mushrooms(variant: u32, lod: u8, art: ArtStyle) -> Geometry {
    let mut g = Geometry::default();
    let count = [4usize, 3, 5, 4][variant as usize % 4];
    let (cap_tint, cap_surface) = if art.painted() {
        // The painted cap is already a redcap.  The warm variants share that artwork while
        // retaining enough brown and ochre to keep a patch from reading as one species.
        (
            [
                Vec3::new(0.98, 0.42, 0.32),
                Vec3::new(0.68, 0.42, 0.26),
                Vec3::new(0.94, 0.64, 0.22),
                Vec3::new(0.90, 0.76, 0.52),
            ][variant as usize % 4],
            REGIONS.mushroom_cap,
        )
    } else {
        // Variants zero and two use the documented red-cap photograph, supplied by the atlas
        // packer in cell 14.  Brown and ochre retain the photographed portobello surface.
        (
            [
                Vec3::new(1.0, 0.92, 0.92),
                Vec3::new(0.66, 0.44, 0.28),
                Vec3::new(0.96, 0.78, 0.72),
                Vec3::new(0.92, 0.76, 0.42),
            ][variant as usize % 4],
            if matches!(variant % 4, 0 | 2) {
                REGIONS.redcap
            } else {
                REGIONS.mushroom_cap
            },
        )
    };
    for i in 0..count {
        let phase = variant as f32 * 1.71 + i as f32 * 2.399_96;
        let radius = if i == 0 {
            0.1
        } else {
            0.1 + 0.05 * (i as f32).sqrt()
        };
        let at = Vec3::new(phase.cos() * radius, 0.0, phase.sin() * radius * 0.72);
        let h =
            (0.11 + 0.045 * ((i + variant as usize) % 3) as f32) * (1.0 + variant as f32 * 0.05);
        let cap = h * (0.7 + 0.16 * ((i + 1) % 2) as f32);
        let lean = Vec3::new(phase.cos() * h * 0.10, 0.0, phase.sin() * h * 0.10);
        let top = at + lean + Vec3::Y * h;
        let sides = [12, 8, 5][lod as usize];
        let old = g.set_tint(Vec3::new(0.86, 0.78, 0.61));
        tube(
            &mut g,
            at,
            top,
            cap * 0.22,
            cap * 0.15,
            sides,
            REGIONS.mushroom_stem,
            0.0,
            0.34,
        );
        g.set_tint(cap_tint);
        mushroom_cap(
            &mut g,
            top,
            cap,
            sides,
            lod,
            phase,
            cap_surface,
            0.42 + i as f32 * 0.04,
        );
        g.set_tint(old);
    }
    g
}

fn mushroom_cap(
    g: &mut Geometry,
    centre: Vec3,
    radius: f32,
    sides: u32,
    lod: u8,
    phase: f32,
    cap_surface: Region,
    sway: f32,
) {
    let rings = [4, 2, 1][lod as usize];
    let dome = radius * (0.34 + 0.06 * phase.sin().abs());
    let top = g.vert(
        centre + Vec3::Y * dome,
        Vec3::Y,
        cap_surface.uv(0.5, 0.5),
        sway,
    );
    let mut previous = vec![];
    for ring in 1..=rings {
        let r = ring as f32 / rings as f32;
        let mut current = vec![];
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            let wobble = 1.0 + 0.06 * (a * 3.0 + phase).sin() * r;
            let p = centre
                + Vec3::new(
                    a.cos() * radius * r * wobble,
                    dome * (1.0 - r * r),
                    a.sin() * radius * r * wobble,
                );
            let n = Vec3::new(a.cos() * dome / radius, 1.0, a.sin() * dome / radius);
            current.push(g.vert(
                p,
                n,
                cap_surface.uv(0.5 + 0.5 * r * a.cos(), 0.5 + 0.5 * r * a.sin()),
                sway,
            ));
        }
        if previous.is_empty() {
            for k in 0..sides as usize {
                g.tri(top, current[k], current[k + 1]);
            }
        } else {
            for k in 0..sides as usize {
                g.quad(previous[k], current[k], current[k + 1], previous[k + 1]);
            }
        }
        previous = current;
    }

    // The underside is separately modelled and mapped to the gill cell.  Near and mid LODs
    // retain a small concavity and actual radial gill ridges, rather than relying on a flat cap.
    if lod < 2 {
        let old = g.set_tint(Vec3::new(0.73, 0.60, 0.46));
        let inner = g.vert(
            centre - Vec3::Y * radius * 0.10,
            -Vec3::Y,
            REGIONS.mushroom_gills.uv(0.5, 0.5),
            sway,
        );
        let mut rim = vec![];
        for k in 0..=sides {
            let a = k as f32 / sides as f32 * std::f32::consts::TAU;
            let p = centre + Vec3::new(a.cos() * radius, -radius * 0.018, a.sin() * radius);
            rim.push(
                g.vert(
                    p,
                    -Vec3::Y,
                    REGIONS
                        .mushroom_gills
                        .uv(0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()),
                    sway,
                ),
            );
        }
        for k in 0..sides as usize {
            g.tri(inner, rim[k + 1], rim[k]);
        }
        if lod == 0 {
            let gills = sides.min(12);
            for k in 0..gills {
                let a = (k as f32 + 0.31) / gills as f32 * std::f32::consts::TAU + phase * 0.07;
                let side = Vec3::new(-a.sin(), 0.0, a.cos());
                let start = centre + Vec3::new(a.cos(), -0.095, a.sin()) * radius * 0.16;
                let end = centre + Vec3::new(a.cos(), -0.02, a.sin()) * radius * 0.96;
                let w = radius * 0.018;
                let a0 = g.vert(
                    start - side * w,
                    -Vec3::Y,
                    REGIONS.mushroom_gills.uv(0.45, 0.15),
                    sway,
                );
                let a1 = g.vert(
                    start + side * w,
                    -Vec3::Y,
                    REGIONS.mushroom_gills.uv(0.55, 0.15),
                    sway,
                );
                let b1 = g.vert(
                    end + side * w,
                    -Vec3::Y,
                    REGIONS.mushroom_gills.uv(0.55, 0.96),
                    sway,
                );
                let b0 = g.vert(
                    end - side * w,
                    -Vec3::Y,
                    REGIONS.mushroom_gills.uv(0.45, 0.96),
                    sway,
                );
                g.quad(a0, a1, b1, b0);
            }
        }
        g.set_tint(old);
    }
}

fn wildflowers(variant: u32, lod: u8, art: ArtStyle) -> Geometry {
    let mut g = Geometry::default();
    let blossom = if art.painted() {
        [
            Vec3::new(0.94, 0.49, 0.68),
            Vec3::new(0.62, 0.45, 0.93),
            Vec3::new(0.98, 0.74, 0.20),
            Vec3::new(0.98, 0.91, 0.74),
        ][variant as usize % 4]
    } else {
        // Both photographed blossom cells are white.  Gentle tinting creates a meadow mix
        // without fabricating a second flower photograph or turning a pale bloom fluorescent.
        [
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 0.79, 0.87),
            Vec3::new(1.0, 0.95, 0.72),
            Vec3::new(1.0, 0.91, 0.86),
        ][variant as usize % 4]
    };
    let stems = [7usize, 4, 2][lod as usize];
    for i in 0..stems {
        let a = variant as f32 * 0.87 + i as f32 * 2.399_96;
        let base = Vec3::new(
            a.cos() * 0.045 * (i as f32).sqrt(),
            0.0,
            a.sin() * 0.045 * (i as f32).sqrt(),
        );
        // Ordinary meadow grass can reach roughly 0.3 m.  These mixed stems clear it
        // without becoming garden flowers: roughly 0.46..0.71 m across every variant.
        let h = [0.48, 0.55, 0.46, 0.52][variant as usize % 4]
            + [0.0, 0.08, 0.15][(i + variant as usize) % 3];
        let tip = base + Vec3::new(a.cos() * h * 0.16, h, a.sin() * h * 0.16);
        let old = g.set_tint(Vec3::new(0.47, 0.75, 0.33));
        tube(
            &mut g,
            base,
            tip,
            0.006,
            0.003,
            [5, 4, 3][lod as usize],
            REGIONS.spare_detail,
            0.0,
            0.75,
        );
        if lod < 2 {
            leaf_card(
                &mut g,
                base + (tip - base) * 0.36,
                Vec3::new(a.cos(), 0.22, a.sin()),
                0.12,
                0.06,
                REGIONS.wildflower_leaf,
                0.8,
            );
        }
        g.set_tint(blossom);
        if art.painted() {
            painted_flower_head(&mut g, tip, a + 0.6, lod, 0.92);
        } else {
            let blossom_surface = if (i + variant as usize) % 2 == 0 {
                REGIONS.wildflower_petal
            } else {
                REGIONS.strawberry_flower
            };
            flower_head(&mut g, tip, a + 0.6, lod, blossom_surface, 0.92);
        }
        g.set_tint(old);
    }
    g
}

fn flower_head(
    g: &mut Geometry,
    centre: Vec3,
    phase: f32,
    lod: u8,
    blossom_surface: Region,
    sway: f32,
) {
    // The atlas tile is a complete photographed flower, rather than a seamless petal texture.
    // Map it once over a shallow radial corolla: this retains its true outline and creates a
    // real raised flower surface without the repeated-cutout pinwheel the old cards produced.
    let segments = [10usize, 8, 6][lod as usize];
    let r = [0.084, 0.078, 0.070][lod as usize];
    let centre_vertex = g.vert(
        centre + Vec3::Y * r * 0.12,
        Vec3::Y,
        blossom_surface.uv(0.5, 0.5),
        sway,
    );
    let mut rim = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let a = phase + i as f32 / segments as f32 * std::f32::consts::TAU;
        let radial = Vec3::new(a.cos(), 0.0, a.sin());
        let p = centre + radial * r;
        let normal = (Vec3::Y * 0.82 + radial * 0.20).normalize();
        rim.push(g.vert(
            p,
            normal,
            blossom_surface.uv(0.5 + 0.5 * a.cos(), 0.5 + 0.5 * a.sin()),
            sway,
        ));
    }
    for i in 0..segments {
        g.tri(centre_vertex, rim[i], rim[i + 1]);
    }
    sphere(
        g,
        centre + Vec3::Y * (r * 0.11),
        Vec3::splat(r * 0.26),
        [8, 6, 4][lod as usize],
        [5, 4, 3][lod as usize],
        WILDFLOWER_CENTRE,
        sway,
    );
}

/// The hand-painted cell 3 is one petal, unlike the photographed complete blossoms in cells 3
/// and 9.  Build the painted crown from those individual petals, preserving its intended
/// illustrated silhouette rather than stretching one petal over an entire real-flower corolla.
fn painted_flower_head(g: &mut Geometry, centre: Vec3, phase: f32, lod: u8, sway: f32) {
    let petals = [7usize, 6, 4][lod as usize];
    let r = [0.074, 0.070, 0.064][lod as usize];
    for i in 0..petals {
        let a = phase + i as f32 / petals as f32 * std::f32::consts::TAU;
        let dir = Vec3::new(a.cos(), 0.10, a.sin()).normalize();
        leaf_card(g, centre, dir, r, r * 0.66, REGIONS.wildflower_petal, sway);
    }
    sphere(
        g,
        centre + Vec3::Y * 0.003,
        Vec3::splat(r * 0.26),
        [8, 6, 4][lod as usize],
        [5, 4, 3][lod as usize],
        WILDFLOWER_CENTRE,
        sway,
    );
}

fn berry_bush(variant: u32, lod: u8, blackberry: bool) -> Geometry {
    let mut g = Geometry::default();
    let (leaf, berry) = if blackberry {
        (REGIONS.blackberry_leaf, REGIONS.blackberry_berry)
    } else {
        (REGIONS.blueberry_leaf, REGIONS.blueberry_berry)
    };
    let canes = [5usize, 4, 2][lod as usize];
    let main_leaves = [4usize, 2, 1][lod as usize];
    let side_branches = [2usize, 2, 1][lod as usize];
    let side_leaves = [2usize, 1, 1][lod as usize];
    let old = g.set_tint(if blackberry {
        Vec3::new(0.48, 0.64, 0.30)
    } else {
        Vec3::new(0.55, 0.72, 0.38)
    });
    for i in 0..canes {
        let a = variant as f32 * 1.31 + i as f32 / canes as f32 * std::f32::consts::TAU;
        let height =
            (if blackberry { 0.84 } else { 0.64 }) + 0.08 * ((i + variant as usize) % 3) as f32;
        let base = Vec3::new(a.cos(), 0.0, a.sin()) * (0.025 + i as f32 * 0.008);
        let horizontal = Vec3::new(a.cos(), 0.0, a.sin());
        let elbow = base
            + horizontal * height * (if blackberry { 0.14 } else { 0.10 })
            + Vec3::Y * height * 0.46;
        let tip = elbow
            + horizontal * height * (if blackberry { 0.28 } else { 0.22 })
            + Vec3::Y * height * 0.54;
        // Two connected sections give every cane a modest, natural bow instead of a row of
        // bare straight sticks.  They share the green detail cell used by living stems.
        tube(
            &mut g,
            base,
            elbow,
            0.012,
            0.007,
            [6, 5, 3][lod as usize],
            REGIONS.spare_detail,
            0.0,
            0.45,
        );
        tube(
            &mut g,
            elbow,
            tip,
            0.007,
            0.0035,
            [6, 5, 3][lod as usize],
            REGIONS.spare_detail,
            0.45,
            0.78,
        );
        g.set_tint(Vec3::ONE);

        for j in 0..main_leaves {
            let t = (j + 1) as f32 / (main_leaves + 1) as f32;
            let at = if t < 0.5 {
                base.lerp(elbow, t * 2.0)
            } else {
                elbow.lerp(tip, (t - 0.5) * 2.0)
            };
            let turn = a + if j % 2 == 0 { 1.45 } else { -1.35 };
            leaf_card(
                &mut g,
                at,
                Vec3::new(turn.cos(), 0.18, turn.sin()),
                if blackberry { 0.19 } else { 0.16 },
                if blackberry { 0.105 } else { 0.09 },
                leaf,
                0.72,
            );
        }

        for branch in 0..side_branches {
            let t = 0.34 + branch as f32 * 0.30;
            let at = if t < 0.5 {
                base.lerp(elbow, t * 2.0)
            } else {
                elbow.lerp(tip, (t - 0.5) * 2.0)
            };
            let turn = a + if branch % 2 == 0 { 1.15 } else { -1.10 };
            let reach = if blackberry { 0.24 } else { 0.19 } * (1.0 - branch as f32 * 0.12);
            let branch_tip = at + Vec3::new(turn.cos() * reach, reach * 0.30, turn.sin() * reach);
            g.set_tint(if blackberry {
                Vec3::new(0.48, 0.64, 0.30)
            } else {
                Vec3::new(0.55, 0.72, 0.38)
            });
            tube(
                &mut g,
                at,
                branch_tip,
                0.0055,
                0.0025,
                [5, 4, 3][lod as usize],
                REGIONS.spare_detail,
                0.58,
                0.82,
            );
            g.set_tint(Vec3::ONE);
            for leaf_index in 0..side_leaves {
                let leaf_at = at.lerp(branch_tip, 0.28 + leaf_index as f32 * 0.40);
                let leaf_turn = turn + if leaf_index % 2 == 0 { 0.65 } else { -0.72 };
                leaf_card(
                    &mut g,
                    leaf_at,
                    Vec3::new(leaf_turn.cos(), 0.14, leaf_turn.sin()),
                    if blackberry { 0.17 } else { 0.145 },
                    if blackberry { 0.095 } else { 0.08 },
                    leaf,
                    0.82,
                );
            }
            berries(
                &mut g,
                branch_tip,
                [2usize, 1, 1][lod as usize],
                turn,
                berry,
                blackberry,
                lod,
            );
        }
        berries(
            &mut g,
            tip,
            [3usize, 2, 1][lod as usize],
            a,
            berry,
            blackberry,
            lod,
        );
        g.set_tint(old);
    }
    g
}

fn berries(
    g: &mut Geometry,
    stem_tip: Vec3,
    count: usize,
    phase: f32,
    tex: Region,
    blackberry: bool,
    lod: u8,
) {
    for fruit in 0..count {
        let f = fruit as f32 / count.max(1) as f32;
        let p = stem_tip
            + Vec3::new(
                (phase + fruit as f32 * 2.1).cos() * 0.030,
                -0.030 - f * 0.042,
                (phase + fruit as f32 * 2.1).sin() * 0.030,
            );
        if blackberry {
            // A single blackberry reads as a cluster of drupelets close up, then as one dark
            // berry in the distance.  Side-tip clusters make the fruit part of the shrub form.
            for k in 0..[5usize, 3, 1][lod as usize] {
                let q = p + Vec3::new(
                    (k as f32 * 2.4).cos(),
                    (k as f32 * 1.9).sin() * 0.45,
                    (k as f32 * 2.4).sin(),
                ) * 0.012;
                sphere(
                    g,
                    q,
                    Vec3::splat(0.019),
                    [6, 5, 4][lod as usize],
                    [4, 3, 2][lod as usize],
                    tex,
                    0.88,
                );
            }
        } else {
            sphere(
                g,
                p,
                Vec3::splat(0.027),
                [8, 6, 4][lod as usize],
                [5, 4, 3][lod as usize],
                tex,
                0.88,
            );
        }
    }
}

fn load_atlas(art: ArtStyle) -> Option<[Image; 2]> {
    let [a, n] = atlas_paths(art);
    Some([load_png(&a, true)?, load_png(&n, false)?])
}

fn load_rgba_pair(art: ArtStyle) -> Option<((u32, u32, Vec<u8>), (u32, u32, Vec<u8>))> {
    let [a, n] = atlas_paths(art);
    let read = |p: &Path| {
        let img = image::open(p).ok()?.to_rgba8();
        let (w, h) = img.dimensions();
        Some((w, h, img.into_raw()))
    };
    let (aw, ah, albedo) = read(&a)?;
    let (nw, nh, normal_rgba) = read(&n)?;
    let normal = normal_rgba
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1]])
        .collect();
    Some(((aw, ah, albedo), (nw, nh, normal)))
}

fn load_png(path: &Path, srgb: bool) -> Option<Image> {
    let img = image::open(path)
        .map_err(|e| warn!("{}: {e}", path.display()))
        .ok()?
        .to_rgba8();
    let (w, h) = img.dimensions();
    if w != ATLAS_SIZE || h != ATLAS_SIZE || !w.is_power_of_two() {
        warn!(
            "{}: expected {ATLAS_SIZE}×{ATLAS_SIZE} PNG flora atlas",
            path.display()
        );
        return None;
    }
    let base = img.into_raw();
    let lut = srgb_to_lin_lut();
    let mut out = Image::new_uninit(
        Extent3d {
            width: w,
            height: h,
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
    out.data = Some(build_mips(&base, w, srgb, &lut));
    out.texture_descriptor.mip_level_count = w.trailing_zeros() + 1;
    out.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flora_geometry_is_finite_rooted_and_reduces_with_lod() {
        for kind in flora_kinds() {
            for variant in 0..variant_count(*kind) {
                let mut previous_vertices = usize::MAX;
                let mut previous_triangles = usize::MAX;
                for lod in 0..3 {
                    let mesh = geometry(*kind, variant, lod, ArtStyle::HandPainted);
                    let repeat = geometry(*kind, variant, lod, ArtStyle::HandPainted);
                    assert_eq!(
                        mesh.pos, repeat.pos,
                        "{kind:?} v{variant} lod {lod}: position drift"
                    );
                    assert_eq!(
                        mesh.nor, repeat.nor,
                        "{kind:?} v{variant} lod {lod}: normal drift"
                    );
                    assert_eq!(
                        mesh.uv, repeat.uv,
                        "{kind:?} v{variant} lod {lod}: UV drift"
                    );
                    assert_eq!(
                        mesh.color, repeat.color,
                        "{kind:?} v{variant} lod {lod}: colour drift"
                    );
                    assert_eq!(
                        mesh.sway, repeat.sway,
                        "{kind:?} v{variant} lod {lod}: sway drift"
                    );
                    assert_eq!(
                        mesh.idx, repeat.idx,
                        "{kind:?} v{variant} lod {lod}: index drift"
                    );
                    assert!(!mesh.pos.is_empty(), "{kind:?} v{variant} lod {lod}: empty");
                    assert_eq!(mesh.pos.len(), mesh.nor.len());
                    assert_eq!(mesh.pos.len(), mesh.uv.len());
                    assert_eq!(mesh.pos.len(), mesh.color.len());
                    assert_eq!(mesh.pos.len(), mesh.sway.len());
                    assert_eq!(mesh.idx.len() % 3, 0);
                    assert!(mesh.idx.iter().all(|&i| (i as usize) < mesh.pos.len()));
                    assert!(
                        mesh.pos
                            .iter()
                            .flatten()
                            .chain(mesh.nor.iter().flatten())
                            .chain(mesh.uv.iter().flatten())
                            .chain(mesh.color.iter().flatten())
                            .chain(mesh.sway.iter())
                            .all(|v| v.is_finite())
                    );
                    assert!(
                        mesh.pos.iter().all(|p| p[1] >= -0.001),
                        "{kind:?} v{variant} lod {lod}: root below ground"
                    );
                    assert!(
                        mesh.idx.len() >= 3,
                        "{kind:?} v{variant} lod {lod}: no faces"
                    );
                    assert!(
                        mesh.idx.len() / 3 <= 20_000,
                        "{kind:?} v{variant} lod {lod}: over 20k triangles"
                    );
                    assert!(mesh.pos.len() <= previous_vertices);
                    assert!(mesh.idx.len() / 3 <= previous_triangles);
                    previous_vertices = mesh.pos.len();
                    previous_triangles = mesh.idx.len() / 3;
                }
            }
        }
    }

    #[test]
    fn flora_faces_follow_their_declared_normals_without_degenerates() {
        for art in ArtStyle::ALL {
            for kind in flora_kinds() {
                for variant in 0..variant_count(*kind) {
                    for lod in 0..3 {
                        let mesh = geometry(*kind, variant, lod, art);
                        for face in mesh.idx.chunks_exact(3) {
                            let [a, b, c] = [face[0] as usize, face[1] as usize, face[2] as usize];
                            let pa = Vec3::from_array(mesh.pos[a]);
                            let pb = Vec3::from_array(mesh.pos[b]);
                            let pc = Vec3::from_array(mesh.pos[c]);
                            let cross = (pb - pa).cross(pc - pa);
                            assert!(
                                cross.length_squared() > 0.0,
                                "{kind:?} v{variant} lod {lod}: degenerate triangle"
                            );
                            let normal = Vec3::from_array(mesh.nor[a])
                                + Vec3::from_array(mesh.nor[b])
                                + Vec3::from_array(mesh.nor[c]);
                            assert!(
                                cross.dot(normal) > 0.0,
                                "{kind:?} v{variant} lod {lod}: winding opposes shading normal"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn photoreal_wildflowers_keep_raised_photo_corollas_at_every_lod() {
        for variant in 0..variant_count(AssetKind::Flowers) {
            let near = geometry(AssetKind::Flowers, variant, 0, ArtStyle::Realistic);
            assert!(
                near.pos.iter().any(|p| p[1] > 0.50),
                "Flowers v{variant}: blossoms did not clear meadow height"
            );
            // The two documented blossom cells alternate through each clump.  A complete source
            // cutout is mapped once per raised corolla, rather than stamped onto every petal.
            for surface in [REGIONS.wildflower_petal, REGIONS.strawberry_flower] {
                assert!(
                    near.uv.iter().any(|uv| {
                        uv[0] >= surface.0[0]
                            && uv[0] <= surface.0[2]
                            && uv[1] >= surface.0[1]
                            && uv[1] <= surface.0[3]
                    }),
                    "Flowers v{variant}: missing photographed blossom surface"
                );
            }
            for lod in 0..3 {
                let mesh = geometry(AssetKind::Flowers, variant, lod, ArtStyle::Realistic);
                assert!(
                    mesh.pos.iter().any(|p| p[1] > 0.44),
                    "Flowers v{variant} lod {lod}: no visible crown"
                );
            }
        }
    }

    #[test]
    fn painted_wildflowers_keep_individual_petal_cards() {
        for lod in 0..3 {
            let mesh = geometry(AssetKind::Flowers, 0, lod, ArtStyle::HandPainted);
            let petiole_uv = REGIONS.wildflower_petal.uv(0.42, 0.96);
            let petal_starts = mesh.uv.iter().filter(|&&uv| uv == petiole_uv).count();
            assert!(
                petal_starts >= [7, 6, 4][lod as usize],
                "painted Flowers lod {lod}: individual cell-3 petals were replaced"
            );
        }
    }

    #[test]
    fn wild_strawberries_keep_exposed_photo_fruit_through_far_lod() {
        for lod in 0..3 {
            let mesh = geometry(AssetKind::WildStrawberry, 0, lod, ArtStyle::Realistic);
            let exposed_fruit = mesh
                .pos
                .iter()
                .zip(&mesh.uv)
                .filter(|(p, uv)| {
                    p[1] > 0.035
                        && Vec2::new(p[0], p[2]).length() > 0.12
                        && uv[0] >= REGIONS.strawberry_berry.0[0]
                        && uv[0] <= REGIONS.strawberry_berry.0[2]
                        && uv[1] >= REGIONS.strawberry_berry.0[1]
                        && uv[1] <= REGIONS.strawberry_berry.0[3]
                })
                .count();
            assert!(
                exposed_fruit >= [36, 18, 7][lod as usize],
                "Wild strawberry lod {lod}: ripe fruit is hidden in the leaf mound"
            );
        }
    }
}
