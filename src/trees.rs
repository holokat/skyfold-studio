//! Trees and bushes grown with ez-tree (the Rust port in `crates/ez_tree`).
//!
//! Each species uses ez-tree presets at three sizes, scaled to real heights, with photoscanned
//! CC0 bark (albedo + normal map, mapped at its real size) and alpha-cut leaf twig cards built
//! from photographed leaves (see assets/textures/CREDITS.md). A tree renders as the three ez-tree
//! levels of detail; beyond them a crossed-quad billboard ("impostor") shows a picture of the
//! same tree, captured once at startup into a shared atlas (one material, one draw batch for all
//! distant trees). Leaves and branches sway with the wind using ez-tree's hierarchy weights.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget, ScalingMode};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::gpu_readback::{ReadbackComplete, ReadbackOnce};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use ez_tree::{Detail, TreeOptions};

use crate::objects::{FoliageExt, FoliageMaterial, Library};
use crate::procgen::meshes::AssetKind;
use crate::streaming::{BillboardExt, BillboardMaterial};
use crate::terrain::textures::{build_mips, lin_to_srgb, srgb_to_lin_lut};

#[path = "trees/diverse.rs"]
mod diverse;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BarkTex {
    Oak,
    Ash,
    Birch,
    Pine,
    Palm,
    Cactus,
    Bamboo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeafTex {
    Oak,
    Ash,
    Aspen,
    Pine,
    MapleAutumn,
    Bush,
    PalmFrond,
    Reeds,
    LilyPads,
    BlossomPink,
    BlossomWhite,
    GinkgoFan,
    Bamboo,
}

impl BarkTex {
    const ALL: [BarkTex; 7] = [
        BarkTex::Oak,
        BarkTex::Ash,
        BarkTex::Birch,
        BarkTex::Pine,
        BarkTex::Palm,
        BarkTex::Cactus,
        BarkTex::Bamboo,
    ];
    fn file(self) -> &'static str {
        match self {
            BarkTex::Oak => "bark_oak",
            BarkTex::Ash => "bark_ash",
            BarkTex::Birch => "bark_birch",
            BarkTex::Pine => "bark_pine",
            BarkTex::Palm => "bark_palm",
            BarkTex::Cactus => "cactus_skin",
            BarkTex::Bamboo => "bark_bamboo",
        }
    }
    /// Real-world size (m) of one texture repeat: (around, along) the branch.
    fn tile(self) -> Vec2 {
        match self {
            BarkTex::Oak => Vec2::new(0.5, 1.0), // ambientCG Bark001 (1:2 scan)
            BarkTex::Ash => Vec2::new(1.0, 1.0), // Poly Haven tree_bark_03
            BarkTex::Birch => Vec2::new(0.8, 0.8), // ambientCG Bark009 (pale smooth bark)
            BarkTex::Pine => Vec2::new(2.0, 2.0), // Poly Haven pine_bark
            BarkTex::Palm => Vec2::new(1.86, 1.86), // Poly Haven palm_bark (mapped in plants.rs)
            BarkTex::Cactus => Vec2::new(1.0, 1.0),
            // Bamboo culms author their own per-internode UVs in `trees::diverse`.
            BarkTex::Bamboo => Vec2::ZERO,
        }
    }

    fn roughness(self, art: crate::art_style::ArtStyle) -> f32 {
        match self {
            // Moso bamboo's smooth green culms retain a restrained waxy highlight. The painted
            // counterpart carries its lighting in the artwork, so it stays visibly calmer.
            BarkTex::Bamboo if art.painted() => 0.72,
            BarkTex::Bamboo => 0.46,
            BarkTex::Cactus => 0.55,
            _ => 0.92,
        }
    }

    fn reflectance(self) -> f32 {
        match self {
            BarkTex::Bamboo | BarkTex::Cactus => 0.35,
            _ => 0.18,
        }
    }
}

impl LeafTex {
    const ALL: [LeafTex; 13] = [
        LeafTex::Oak,
        LeafTex::Ash,
        LeafTex::Aspen,
        LeafTex::Pine,
        LeafTex::MapleAutumn,
        LeafTex::Bush,
        LeafTex::PalmFrond,
        LeafTex::Reeds,
        LeafTex::LilyPads,
        LeafTex::BlossomPink,
        LeafTex::BlossomWhite,
        LeafTex::GinkgoFan,
        LeafTex::Bamboo,
    ];
    fn file(self) -> &'static str {
        match self {
            LeafTex::Oak => "leaf_oak",
            LeafTex::Ash => "leaf_ash",
            LeafTex::Aspen => "leaf_aspen",
            LeafTex::Pine => "leaf_pine",
            LeafTex::MapleAutumn => "leaf_maple_autumn",
            LeafTex::Bush => "leaf_bush",
            LeafTex::PalmFrond => "leaf_palm_frond",
            LeafTex::Reeds => "reeds_cattails",
            LeafTex::LilyPads => "lily_pads",
            LeafTex::BlossomPink => "procedural_blossom_pink",
            LeafTex::BlossomWhite => "procedural_blossom_white",
            LeafTex::GinkgoFan => "procedural_ginkgo_fan",
            LeafTex::Bamboo => "leaf_bamboo",
        }
    }

    fn blossom(self) -> Option<bool> {
        match self {
            LeafTex::BlossomPink => Some(true),
            LeafTex::BlossomWhite => Some(false),
            _ => None,
        }
    }

    fn ginkgo(self) -> bool {
        self == LeafTex::GinkgoFan
    }

    /// The hand-painted foliage atlas already owns the leaf shapes and painted value variation.
    /// These restrained linear tints only keep its hue families readable under the world's
    /// cinematic exposure, without flattening the card texture or turning leaves emissive.
    fn appearance(self, art: crate::art_style::ArtStyle) -> LeafAppearance {
        let plain = LeafAppearance {
            tint: [1.0; 3],
            roughness: 0.72,
            reflectance: 0.30,
            transmission: 0.35,
            fill: LinearRgba::BLACK,
        };
        if !art.painted() {
            return plain;
        }
        let (tint, fill) = match self {
            // Keep the real green bamboo asset exactly as authored in both styles.
            LeafTex::Bamboo | LeafTex::Reeds | LeafTex::LilyPads => ([1.0; 3], LinearRgba::BLACK),
            // The painted leaf maps retain their brushwork; a stronger green channel and lower
            // red/blue keep their canopy reads verdant rather than yellowed or fluorescent.
            LeafTex::Oak | LeafTex::Ash | LeafTex::Aspen | LeafTex::Bush | LeafTex::PalmFrond => {
                ([0.82, 1.48, 0.58], LinearRgba::rgb(0.018, 0.052, 0.009))
            }
            LeafTex::Pine => ([0.78, 1.35, 0.65], LinearRgba::rgb(0.012, 0.040, 0.010)),
            // Autumn crowns and ginkgo share a warm, golden-orange family, but their card
            // textures retain their distinct veins and silhouette.
            LeafTex::MapleAutumn | LeafTex::GinkgoFan => {
                ([1.50, 1.10, 0.36], LinearRgba::rgb(0.060, 0.030, 0.003))
            }
            LeafTex::BlossomPink => ([1.35, 0.82, 1.08], LinearRgba::rgb(0.060, 0.012, 0.028)),
            LeafTex::BlossomWhite => ([1.22, 1.14, 0.95], LinearRgba::rgb(0.042, 0.035, 0.020)),
        };
        LeafAppearance {
            tint,
            // Painted foliage reads as opaque pigment with a little leaf translucency. This
            // leaves directional canopy shadows intact while preventing a dark, flat mass.
            roughness: 0.82,
            reflectance: 0.24,
            transmission: 0.43,
            // This is a tiny, hue-matched indirect canopy fill, not an unlit foliage shader.
            // Direct light, shadows, texture variation and rounded leaf normals remain dominant.
            fill,
        }
    }
}

#[derive(Clone, Copy)]
struct LeafAppearance {
    /// Linear multiplier shared by the runtime material and baked export texture.
    tint: [f32; 3],
    roughness: f32,
    reflectance: f32,
    transmission: f32,
    fill: LinearRgba,
}

impl LeafAppearance {
    fn base_color(self) -> Color {
        Color::linear_rgb(self.tint[0], self.tint[1], self.tint[2])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tweak {
    None,
    /// Branches droop towards the ground.
    Willow,
    /// Bare branches.
    Dead,
    /// A low, spreading deciduous scaffold carrying blossom-cluster cards.
    Sakura,
    /// A spreading ash-like scaffold carrying compact fan-leaf sprays.
    Ginkgo,
    /// A high-density evergreen with a materially heavier lower trunk.
    Sequoia,
}

/// How a species is grown.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gen {
    /// ez-tree presets (`sizes` names the presets)
    Ez,
    Palm,
    Cactus,
    /// crossed photo cards: (aspect, planes per clump, clumps, spread m)
    Cards(f32, u32, u32, f32),
    LilyPads,
    Bamboo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// LODs + far billboard
    Tree,
    /// LODs, near only
    Shrub,
    /// one photo-card mesh with the leaf material, near only
    Card,
}

/// One tree species: presets / variants per size with target heights (m), textures and tints.
struct Species {
    kind: AssetKind,
    grow: Gen,
    class: Class,
    sizes: &'static [(&'static str, f32)],
    bark: BarkTex,
    leaf: LeafTex,
    /// Added to the preset seeds, so species sharing presets grow differently.
    seed: i64,
    tweak: Tweak,
    /// sRGB multipliers applied through vertex colours.
    bark_tint: [f32; 3],
    leaf_tint: [f32; 3],
}

fn species() -> Vec<Species> {
    use AssetKind::*;
    let s = |kind, sizes, bark, leaf, seed, tweak, bark_tint, leaf_tint| Species {
        kind,
        grow: Gen::Ez,
        class: if kind == Bush {
            Class::Shrub
        } else {
            Class::Tree
        },
        sizes,
        bark,
        leaf,
        seed,
        tweak,
        bark_tint,
        leaf_tint,
    };
    let g = |kind, grow, class, sizes, bark, leaf, seed, leaf_tint| Species {
        kind,
        grow,
        class,
        sizes,
        bark,
        leaf,
        seed,
        tweak: Tweak::None,
        bark_tint: [1.0; 3],
        leaf_tint,
    };
    vec![
        g(
            Palm,
            Gen::Palm,
            Class::Tree,
            &[("palm_a", 8.0), ("palm_b", 10.5), ("palm_c", 13.0)],
            BarkTex::Palm,
            LeafTex::PalmFrond,
            101,
            [1.0; 3],
        ),
        g(
            Cactus,
            Gen::Cactus,
            Class::Tree,
            &[("cactus_a", 3.4), ("cactus_b", 4.8), ("cactus_c", 6.2)],
            BarkTex::Cactus,
            LeafTex::Bush,
            202,
            [1.0; 3],
        ),
        g(
            Reeds,
            Gen::Cards(1.0, 3, 3, 0.35),
            Class::Card,
            &[("reeds_a", 1.6), ("reeds_b", 1.95)],
            BarkTex::Oak,
            LeafTex::Reeds,
            505,
            [1.0; 3],
        ),
        g(
            LilyPads,
            Gen::LilyPads,
            Class::Card,
            &[("lily_a", 2.4), ("lily_b", 3.2)],
            BarkTex::Oak,
            LeafTex::LilyPads,
            606,
            [1.0; 3],
        ),
        s(
            Oak,
            &[
                ("oak_small", 9.0),
                ("oak_medium", 13.0),
                ("oak_large", 18.0),
            ],
            BarkTex::Oak,
            LeafTex::Oak,
            0,
            Tweak::None,
            [1.0; 3],
            [1.0; 3],
        ),
        s(
            Birch,
            &[
                ("aspen_small", 9.0),
                ("aspen_medium", 14.0),
                ("aspen_large", 19.0),
            ],
            BarkTex::Birch,
            LeafTex::Aspen,
            0,
            Tweak::None,
            [1.0; 3],
            [1.0; 3],
        ),
        s(
            Maple,
            &[
                ("ash_small", 9.0),
                ("ash_medium", 13.0),
                ("ash_large", 16.0),
            ],
            BarkTex::Ash,
            LeafTex::MapleAutumn,
            11,
            Tweak::None,
            [0.9, 0.88, 0.85],
            [1.0; 3],
        ),
        s(
            Willow,
            &[("ash_medium", 11.0), ("ash_large", 14.0)],
            BarkTex::Ash,
            LeafTex::Ash,
            23,
            Tweak::Willow,
            [0.85, 0.85, 0.8],
            [0.92, 1.0, 0.8],
        ),
        // These use the same skeleton, rounded foliage normals and shared detail LODs as the
        // established broadleaf/evergreen trees. Only the crown recipe and foliage material are
        // species-authored; they are not presented as scans.
        s(
            SakuraPink,
            &[
                ("oak_small", 7.0),
                ("oak_medium", 10.0),
                ("oak_large", 13.0),
            ],
            BarkTex::Ash,
            LeafTex::BlossomPink,
            53,
            Tweak::Sakura,
            [0.82, 0.65, 0.48],
            [1.0; 3],
        ),
        s(
            SakuraWhite,
            &[
                ("oak_small", 7.0),
                ("oak_medium", 10.0),
                ("oak_large", 13.0),
            ],
            BarkTex::Ash,
            LeafTex::BlossomWhite,
            59,
            Tweak::Sakura,
            [0.82, 0.65, 0.48],
            [1.0; 3],
        ),
        s(
            GoldenMaple,
            &[
                ("ash_small", 9.0),
                ("ash_medium", 13.0),
                ("ash_large", 16.0),
            ],
            BarkTex::Ash,
            LeafTex::MapleAutumn,
            47,
            Tweak::None,
            [0.95, 0.88, 0.72],
            [1.35, 1.12, 0.35],
        ),
        s(
            Ginkgo,
            &[
                ("ash_small", 9.0),
                ("ash_medium", 13.0),
                ("ash_large", 17.0),
            ],
            BarkTex::Ash,
            LeafTex::GinkgoFan,
            61,
            Tweak::Ginkgo,
            [0.90, 0.80, 0.60],
            [1.0; 3],
        ),
        s(
            Sequoia,
            &[
                ("pine_medium", 28.0),
                ("pine_large", 42.0),
                ("pine_large", 55.0),
            ],
            BarkTex::Pine,
            LeafTex::Pine,
            71,
            Tweak::Sequoia,
            [0.68, 0.43, 0.25],
            [0.72, 0.9, 0.56],
        ),
        g(
            Bamboo,
            Gen::Bamboo,
            Class::Tree,
            &[("bamboo_a", 12.0), ("bamboo_b", 17.0), ("bamboo_c", 22.0)],
            BarkTex::Bamboo,
            LeafTex::Bamboo,
            73,
            [1.0; 3],
        ),
        s(
            Pine,
            &[
                ("pine_small", 11.0),
                ("pine_medium", 16.0),
                ("pine_large", 23.0),
            ],
            BarkTex::Pine,
            LeafTex::Pine,
            0,
            Tweak::None,
            [1.0; 3],
            [1.0; 3],
        ),
        s(
            Spruce,
            &[("pine_medium", 14.0), ("pine_large", 20.0)],
            BarkTex::Pine,
            LeafTex::Pine,
            7,
            Tweak::None,
            [0.8, 0.75, 0.72],
            [0.72, 0.84, 0.8],
        ),
        s(
            Fir,
            &[("pine_small", 10.0), ("pine_medium", 15.0)],
            BarkTex::Pine,
            LeafTex::Pine,
            13,
            Tweak::None,
            [0.85, 0.8, 0.8],
            [0.8, 0.92, 0.95],
        ),
        s(
            SnowyPine,
            &[("pine_small", 10.0), ("pine_medium", 15.0)],
            BarkTex::Pine,
            LeafTex::Pine,
            17,
            Tweak::None,
            [0.9, 0.9, 0.9],
            [1.35, 1.45, 1.55],
        ),
        s(
            Cedar,
            &[("pine_large", 18.0)],
            BarkTex::Oak,
            LeafTex::Pine,
            21,
            Tweak::None,
            [0.8, 0.72, 0.65],
            [0.9, 1.0, 0.82],
        ),
        s(
            DeadTree,
            &[("oak_medium", 10.0), ("ash_medium", 12.0)],
            BarkTex::Oak,
            LeafTex::Oak,
            31,
            Tweak::Dead,
            [0.72, 0.7, 0.68],
            [1.0; 3],
        ),
        s(
            Bush,
            &[("bush_1", 1.6), ("bush_2", 2.1)],
            BarkTex::Oak,
            LeafTex::Bush,
            0,
            Tweak::None,
            [1.0; 3],
            [1.0; 3],
        ),
    ]
}

/// Meshes of one tree variant.
pub struct TreeVariant {
    /// (bark, leaves) per level of detail; leaves are absent on bare trees
    pub lods: Vec<(Handle<Mesh>, Option<Handle<Mesh>>)>,
    /// billboard atlas cell (trees only; distant trees are merged billboards, see streaming.rs)
    pub cell: Option<u32>,
    pub height: f32,
    pub width: f32,
    /// Lowest point (m) relative to the root; drooping branches can reach below it.
    pub min_y: f32,
    /// The trunk at hip height: (radius m, centre xz relative to the root), from the tree's
    /// skeleton. Walk-mode colliders block on this alone, never on branches or leaves.
    pub trunk: Option<(f32, [f32; 2])>,
}

/// What a library entry needs to render ez-tree trees.
pub struct TreeParts {
    pub variants: Vec<TreeVariant>,
    pub bark: Handle<FoliageMaterial>,
    pub leaves: Handle<FoliageMaterial>,
    /// Distances (m) where LOD 0→1, 1→2 and 2→impostor switch, and where the tree disappears.
    pub ranges: [f32; 4],
}

const ATLAS: u32 = 2048;
const CELL: u32 = 256;
const CELLS: u32 = ATLAS / CELL;
const CAPTURE_LAYER: usize = 8;

/// Shared billboard atlas for distant trees.
#[derive(Resource)]
pub struct ImpostorAtlas {
    pub material: Handle<BillboardMaterial>,
    /// (entry id, variant, cell) to capture
    jobs: Vec<(String, usize, u32)>,
    /// one small render target per cell (cameras sharing a target can't keep it transparent)
    targets: Vec<Handle<Image>>,
    /// read-back pixels per cell
    cells: HashMap<u32, Vec<u8>>,
    state: CaptureState,
    /// every species' bark and leaf materials (the world's art style swaps their textures)
    bark_mats: Vec<(BarkTex, Handle<FoliageMaterial>)>,
    leaf_mats: Vec<(LeafTex, Handle<FoliageMaterial>)>,
    /// the surface style (`art_style::surfaces`) the textures are in
    art: crate::art_style::ArtStyle,
    /// the textures changed since the pictures were taken: take them again
    recapture: bool,
}

impl ImpostorAtlas {
    /// Takes the distant-tree pictures again (the plants' colours changed: `nature::alien`).
    pub fn retake(&mut self) {
        self.recapture = true;
    }
}

#[derive(Default, PartialEq)]
enum CaptureState {
    #[default]
    Waiting,
    Rendering(u32),
    ReadingBack(u32),
    Done,
}

struct BuiltVariant {
    lods: Vec<(ez_tree::Buffers, Option<ez_tree::Buffers>)>,
    scale: f32,
    height: f32,
    width: f32,
    min_y: f32,
    trunk: Option<(f32, [f32; 2])>,
}

/// Height (m above the root) the trunk collider is measured at.
const TRUNK_AT: f64 = 1.0;

/// The trunk's radius and centre at `TRUNK_AT` metres, from the skeleton (tree units × `scale`).
fn trunk_at_hip(skel: &ez_tree::Skeleton, scale: f32) -> Option<(f32, [f32; 2])> {
    let b = skel.branches.iter().find(|b| b.level == 0)?;
    let s = &b.sections;
    if s.len() < 2 {
        return None;
    }
    let want = TRUNK_AT / scale.max(1e-4) as f64;
    let i = s
        .iter()
        .position(|x| x.origin.y >= want)
        .unwrap_or(s.len() - 1)
        .max(1);
    let (a, c) = (&s[i - 1], &s[i]);
    let t = ((want - a.origin.y) / (c.origin.y - a.origin.y).max(1e-9)).clamp(0.0, 1.0);
    let r = a.radius + (c.radius - a.radius) * t;
    let x = a.origin.x + (c.origin.x - a.origin.x) * t;
    let z = a.origin.z + (c.origin.z - a.origin.z) * t;
    Some((r as f32 * scale, [x as f32 * scale, z as f32 * scale]))
}

fn options_for(sp: &Species, preset: &str) -> Option<TreeOptions> {
    let mut o = TreeOptions::preset(preset)?;
    o.seed += sp.seed;
    match sp.tweak {
        Tweak::Willow => {
            o.branch.force.direction = ez_tree::XYZ {
                x: 0.0,
                y: -1.0,
                z: 0.0,
            };
            o.branch.force.strength = 0.025;
            o.leaves.count = (o.leaves.count as f64 * 1.3) as usize;
        }
        Tweak::Sakura => {
            // A low, spreading, gently gnarled scaffold, while preserving ez-tree's dense
            // terminal anchors, rounded normals, deterministic seeds and shared LOD details.
            o.branch.angle[1] = 66.0;
            o.branch.angle[2] = 48.0;
            o.branch.angle[3] = 34.0;
            o.branch.children[0] = 8;
            o.branch.children[1] = 5;
            o.branch.children[2] = 4;
            o.branch.start[1] = 0.34;
            o.branch.start[2] = 0.10;
            o.branch.length[1] *= 1.24;
            o.branch.length[2] *= 0.92;
            o.branch.gnarliness[0] = 0.12;
            o.branch.gnarliness[1] = 0.30;
            o.branch.gnarliness[2] = 0.25;
            o.branch.force.strength = 0.008;
            // The blossom cluster card itself is dense. Fewer terminal anchors make the crown
            // read as airy, layered flowers instead of a solid pink or white volume.
            o.leaves.count = 21;
            o.leaves.size = 2.05;
            o.leaves.size_variance = 0.38;
            o.leaves.start = 0.03;
        }
        Tweak::Ginkgo => {
            // Ash already produces the strong, wide-spreading branch hierarchy ginkgo needs.
            // More terminal anchors and compact fan-spray cards make a dense golden crown.
            o.branch.angle[1] = 58.0;
            o.branch.angle[2] = 62.0;
            o.branch.children[0] = 8;
            o.branch.children[1] = 5;
            o.branch.children[2] = 4;
            o.branch.length[1] *= 1.12;
            o.branch.length[2] *= 0.90;
            o.branch.gnarliness[1] = 0.22;
            // The alpha mask holds several modest fans, so give the shared terminal anchors
            // enough overlapping card area to read as a continuous crown at a distance.
            o.leaves.count = 24;
            o.leaves.size = 2.12;
            o.leaves.size_variance = 0.26;
            o.leaves.start = 0.02;
        }
        Tweak::Sequoia => {
            // Keep the proven dense evergreen hierarchy, but make the lower trunk proportionate
            // to a giant sequoia and pull the tier mass slightly inward around that trunk.
            o.branch.radius[0] *= 2.45;
            o.branch.radius[1] *= 1.16;
            o.branch.length[1] *= 0.88;
            o.branch.children[0] = o.branch.children[0].max(110);
            o.leaves.count = 26;
            o.leaves.size *= 1.08;
            o.leaves.size_variance = 0.28;
        }
        Tweak::None | Tweak::Dead => {}
    }
    Some(o)
}

/// Bounds of the first LOD (both meshes).
fn lod_bounds(lods: &crate::plants::Lods) -> ([f32; 3], [f32; 3]) {
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    let (b, l) = &lods[0];
    for p in b
        .positions
        .iter()
        .chain(l.iter().flat_map(|l| l.positions.iter()))
    {
        for i in 0..3 {
            mn[i] = mn[i].min(p[i]);
            mx[i] = mx[i].max(p[i]);
        }
    }
    (mn, mx)
}

/// Bamboo retains a central trunk proxy as compatibility metadata; walk-mode uses its actual
/// individual culm bark in `motion::collide`. Every other tree follows its ez-tree skeleton
/// through `trunk_at_hip`, including Sakura, ginkgo and sequoia.
fn custom_trunk(grow: Gen, height: f32) -> Option<(f32, [f32; 2])> {
    let radius = match grow {
        // `diverse::bamboo` places the outer culm centre at 11.2% of the target height and its
        // base extends another 0.8%, so this is its measured clump footprint.
        Gen::Bamboo => height * 0.120,
        _ => return None,
    };
    Some((radius.max(0.12), [0.0, 0.0]))
}

fn grow_variant(sp: &Species, preset: &str) -> Option<BuiltVariant> {
    if sp.grow != Gen::Ez {
        let (vi, target) = sp
            .sizes
            .iter()
            .enumerate()
            .find(|(_, (p, _))| *p == preset)
            .map(|(i, s)| (i, s.1))?;
        let seed = sp.seed as u64 * 977 + vi as u64 * 131;
        let lods = match sp.grow {
            Gen::Palm => crate::plants::palm(seed, target),
            Gen::Cactus => crate::plants::cactus(seed, target),
            Gen::Cards(aspect, planes, clumps, spread) => {
                crate::plants::cards(seed, target, aspect, planes, clumps, spread)
            }
            Gen::LilyPads => crate::plants::lily_pads(target),
            Gen::Bamboo => diverse::bamboo(seed, target),
            Gen::Ez => unreachable!(),
        };
        let (mn, mx) = lod_bounds(&lods);
        let width = (mx[0] - mn[0])
            .max(mx[2] - mn[2])
            .max(mn[0].abs().max(mx[0].abs()) * 2.0)
            .max(mn[2].abs().max(mx[2].abs()) * 2.0);
        return Some(BuiltVariant {
            lods,
            scale: 1.0,
            height: (mx[1] - mn[1]).max(0.05),
            width,
            min_y: mn[1],
            trunk: custom_trunk(sp.grow, target),
        });
    }
    let o = options_for(sp, preset)?;
    let target = sp
        .sizes
        .iter()
        .find(|(p, _)| *p == preset)
        .map(|s| s.1)
        .unwrap_or(10.0);
    let skel = ez_tree::grow(&o);
    let mut lods = vec![];
    let mut bounds = None;
    // the three ez-tree levels, plus the far one (`FAR`) for the last stretch before the
    // billboards (procedural nature draws it; exports carry it for engines without impostors)
    for level in 0..=Detail::LOD_COUNT {
        let detail = if level < Detail::LOD_COUNT {
            Detail::lod(level)
        } else {
            FAR
        };
        let m = ez_tree::mesh(&o, &skel, &detail);
        if level == 0 {
            bounds = Some((m.min, m.max));
        }
        let leaves =
            (sp.tweak != Tweak::Dead && !m.leaves.positions.is_empty()).then_some(m.leaves);
        lods.push((m.branches, leaves));
    }
    let (mn, mx) = bounds?;
    let h = (mx[1] - mn[1]).max(0.01);
    let scale = target / h;
    let width = (mx[0] - mn[0])
        .max(mx[2] - mn[2])
        .max(mn[0].abs().max(mx[0].abs()) * 2.0)
        .max(mn[2].abs().max(mx[2].abs()) * 2.0)
        * scale;
    let trunk = trunk_at_hip(&skel, scale);
    Some(BuiltVariant {
        lods,
        scale,
        height: target,
        width,
        min_y: mn[1] * scale,
        trunk,
    })
}

fn srgb_tint(c: [f32; 3]) -> [f32; 3] {
    let l = Color::srgb(c[0].min(1.0), c[1].min(1.0), c[2].min(1.0)).to_linear();
    // allow brightening tints above 1 (snow on needles)
    let k = |v: f32, t: f32| if v > 1.0 { t * v } else { t };
    [k(c[0], l.red), k(c[1], l.green), k(c[2], l.blue)]
}

fn to_mesh(
    b: &ez_tree::Buffers,
    scale: f32,
    tint: [f32; 3],
    leaves: bool,
    bark_tile: Vec2,
) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        b.positions
            .iter()
            .map(|p| [p[0] * scale, p[1] * scale, p[2] * scale])
            .collect::<Vec<_>>(),
    );
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, b.normals.clone());
    let uvs: Vec<[f32; 2]> = if leaves {
        // three.js textures are flipped vertically compared to wgpu's top-left origin
        b.uvs.iter().map(|u| [u[0], 1.0 - u[1]]).collect()
    } else if bark_tile == Vec2::ZERO {
        // Species with a zero tile size own their bark UV contract. Bamboo needs this so every
        // culm internode receives exactly one texture repeat in both renderer and GLB export.
        b.uvs.clone()
    } else if b.bark.len() == b.uvs.len() {
        // photoscanned bark at its real size: whole repeats around the branch base, the scan's
        // aspect kept (thin twigs get finer bark), image up pointing up the branch
        b.bark
            .iter()
            .map(|&[around, along, girth]| {
                let wraps = (girth * scale / bark_tile.x).round().max(1.0);
                let rep = girth * scale / wraps;
                [
                    around * wraps,
                    -along * scale * bark_tile.x / (bark_tile.y * rep),
                ]
            })
            .collect()
    } else {
        b.uvs.clone()
    };
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    m.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        b.sway
            .iter()
            .map(|s| [tint[0], tint[1], tint[2], *s])
            .collect::<Vec<_>>(),
    );
    m.insert_indices(Indices::U32(b.indices.clone()));
    if !leaves && let Err(e) = m.generate_tangents() {
        warn!("tree tangents: {e}");
    }
    m
}

/// Two crossed quads showing atlas cell `cell` (the captured picture of the tree), transformed
/// by `tf` and appended to a merged billboard mesh.
#[allow(clippy::too_many_arguments)]
pub fn billboard_quads(
    pos: &mut Vec<[f32; 3]>,
    nor: &mut Vec<[f32; 3]>,
    uv: &mut Vec<[f32; 2]>,
    idx: &mut Vec<u32>,
    width: f32,
    min_y: f32,
    height: f32,
    cell: u32,
    tf: &Transform,
) {
    let (cx, cy) = (cell % CELLS, cell / CELLS);
    let pad = 0.5 / CELL as f32;
    let u0 = cx as f32 / CELLS as f32 + pad;
    let u1 = (cx + 1) as f32 / CELLS as f32 - pad;
    let v0 = cy as f32 / CELLS as f32 + pad;
    let v1 = (cy + 1) as f32 / CELLS as f32 - pad;
    let (w, y0, y1) = (width * 0.5, min_y, min_y + height);
    for (axis, n) in [(Vec3::X, Vec3::Z), (Vec3::Z, -Vec3::X)] {
        // lit mostly from above so both quads shade alike whatever the sun direction
        let nn = tf.rotation * (n * 0.35 + Vec3::Y).normalize();
        let base = pos.len() as u32;
        for (s, t) in [(-1.0, y1), (-1.0, y0), (1.0, y0), (1.0, y1)] {
            let p = tf.transform_point(axis * (s * w) + Vec3::Y * t);
            pos.push(p.to_array());
            nor.push(nn.to_array());
            uv.push([if s < 0.0 { u0 } else { u1 }, if t == y1 { v0 } else { v1 }]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

fn foliage_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::assets_path()).join("textures/foliage")
}

/// RGBA8 image with a full CPU-built mip chain. Cut-outs keep their alpha-test coverage per mip.
pub(crate) fn load_texture(
    path: &std::path::Path,
    srgb: bool,
    cutout: bool,
    repeat: bool,
) -> Option<Image> {
    let img = image::open(path)
        .map_err(|e| warn!("{}: {e}", path.display()))
        .ok()?
        .to_rgba8();
    let (w, h) = img.dimensions();
    if w != h || !w.is_power_of_two() {
        warn!("{}: expected a square power-of-two image", path.display());
        return None;
    }
    let lut = srgb_to_lin_lut();
    let base = img.into_raw();
    let data = if cutout {
        cutout_mips(&base, w, &lut)
    } else {
        build_mips(&base, w, srgb, &lut)
    };
    let mips = w.trailing_zeros() + 1;
    let mut out = Image::new_uninit(
        Extent3d {
            width: w,
            height: w,
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
    out.data = Some(data);
    out.texture_descriptor.mip_level_count = mips;
    let mode = if repeat {
        ImageAddressMode::Repeat
    } else {
        ImageAddressMode::ClampToEdge
    };
    out.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode,
        address_mode_v: mode,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    Some(out)
}

/// Mip chain for alpha-tested textures: RGB averaged in linear light weighted by alpha, and each
/// level's alpha rescaled so the share of texels passing the 0.5 test matches the base level —
/// otherwise leaves thin out into nothing in the distance.
fn cutout_mips(base: &[u8], size: u32, lut: &[f32; 256]) -> Vec<u8> {
    let coverage = |px: &[u8], s: f32| {
        px.chunks_exact(4)
            .filter(|p| p[3] as f32 * s > 127.5)
            .count() as f32
            / (px.len() / 4) as f32
    };
    let target = coverage(base, 1.0);
    let mut out = base.to_vec();
    let mut prev = base.to_vec();
    let mut s = size;
    while s > 1 {
        let n = s / 2;
        let mut next = vec![0u8; (n * n * 4) as usize];
        for y in 0..n {
            for x in 0..n {
                let mut rgb = [0.0f32; 3];
                let mut wsum = 0.0;
                let mut a = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (((2 * y + dy) * s + 2 * x + dx) * 4) as usize;
                    let w = prev[i + 3] as f32 / 255.0 + 1e-3;
                    for c in 0..3 {
                        rgb[c] += lut[prev[i + c] as usize] * w;
                    }
                    wsum += w;
                    a += prev[i + 3] as f32;
                }
                let o = ((y * n + x) * 4) as usize;
                for c in 0..3 {
                    next[o + c] = lin_to_srgb(rgb[c] / wsum);
                }
                next[o + 3] = (a / 4.0 + 0.5) as u8;
            }
        }
        // rescale alpha to preserve coverage (binary search on the scale)
        if target > 0.0 && n >= 4 {
            let (mut lo, mut hi) = (0.5f32, 4.0f32);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                if coverage(&next, mid) < target {
                    lo = mid
                } else {
                    hi = mid
                }
            }
            let k = (lo + hi) * 0.5;
            for p in next.chunks_exact_mut(4) {
                p[3] = (p[3] as f32 * k).min(255.0) as u8;
            }
        }
        out.extend_from_slice(&next);
        prev = next;
        s = n;
    }
    out
}

/// Bark (albedo, normal) per `BarkTex::ALL` and leaf cards per `LeafTex::ALL`, decoded and
/// mipmapped in parallel, in the given art style (painted twins in a hand-painted world).
type FoliageTextures = (Vec<(Option<Image>, Option<Image>)>, Vec<Option<Image>>);

/// Base pixels for foliage made in code rather than loaded from a file. Exporters use this before
/// trying the asset path, keeping their alpha silhouette byte-for-byte aligned with the renderer.
pub fn generated_leaf_rgba(
    file: &str,
    art: crate::art_style::ArtStyle,
) -> Option<image::RgbaImage> {
    const N: u32 = 128;
    let mut base = vec![0u8; (N * N * 4) as usize];
    let painted = matches!(art, crate::art_style::ArtStyle::HandPainted);
    match file {
        "procedural_blossom_pink" | "procedural_blossom_white" => {
            let pink = file == "procedural_blossom_pink";
            let (inner, outer) = match (pink, painted) {
                (true, false) => ([246, 166, 190], [255, 218, 228]),
                (false, false) => ([245, 238, 224], [255, 252, 244]),
                (true, true) => ([230, 116, 158], [255, 192, 207]),
                (false, true) => ([242, 229, 191], [255, 247, 221]),
            };
            // Every terminal card is a dense spray of 56 small five-petal blossoms. The opaque
            // petal interiors survive the material's 0.5 alpha cut-off; only their edges feather.
            // At tree scale these are centimetre-scale flowers, not one giant bloom per card.
            for y in 0..N {
                for x in 0..N {
                    let p = Vec2::new(
                        (x as f32 + 0.5) / N as f32 * 2.0 - 1.0,
                        (y as f32 + 0.5) / N as f32 * 2.0 - 1.0,
                    );
                    let mut cover = 0.0;
                    let mut flower_t = 0.0;
                    for gy in 0..7 {
                        for gx in 0..8 {
                            let k = gy * 8 + gx;
                            let jitter = Vec2::new(
                                ((k * 37 % 11) as f32 - 5.0) * 0.010,
                                ((k * 17 % 13) as f32 - 6.0) * 0.010,
                            );
                            let centre = Vec2::new(
                                -0.875 + (gx as f32 + 0.5) * 0.25,
                                -0.855 + (gy as f32 + 0.5) * (1.71 / 7.0),
                            ) + jitter;
                            let scale = 0.150 + (k % 5) as f32 * 0.006;
                            let q = (p - centre) / scale;
                            if q.length_squared() > 1.4 {
                                continue;
                            }
                            let mut petal_cover = (-q.length_squared() / 0.060).exp() * 0.94;
                            let twist = k as f32 * 0.73;
                            for petal in 0..5 {
                                let a = twist + petal as f32 * std::f32::consts::TAU / 5.0;
                                let axis = Vec2::new(a.cos(), a.sin());
                                let side = Vec2::new(-axis.y, axis.x);
                                let r = q - axis * 0.37;
                                let d = (r.dot(axis) / 0.42).powi(2) + (r.dot(side) / 0.25).powi(2);
                                petal_cover = petal_cover.max((1.0 - d).max(0.0));
                            }
                            if petal_cover > cover {
                                cover = petal_cover;
                                flower_t = (1.0 - q.length().clamp(0.0, 1.0)).powf(0.72);
                            }
                        }
                    }
                    let i = ((y * N + x) * 4) as usize;
                    for c in 0..3 {
                        base[i + c] =
                            (inner[c] as f32 * (1.0 - flower_t) + outer[c] as f32 * flower_t) as u8;
                    }
                    base[i + 3] = (crate::noise::smoothstep(0.08, 0.40, cover) * 255.0) as u8;
                }
            }
        }
        "procedural_ginkgo_fan" => {
            let (dark, light) = if painted {
                ([188, 137, 39], [247, 204, 77])
            } else {
                ([142, 112, 24], [230, 191, 45])
            };
            // Several modest fan leaves share every terminal card. This produces the layered
            // spur-leaf silhouette of a ginkgo crown instead of a sparse field of giant fans.
            const FANS: [(f32, f32, f32, f32); 13] = [
                (-0.47, -0.45, 0.34, -0.35),
                (0.03, -0.48, 0.31, 0.18),
                (0.47, -0.37, 0.30, 0.62),
                (-0.32, 0.02, 0.32, -0.62),
                (0.20, 0.03, 0.36, 0.30),
                (-0.43, 0.47, 0.27, -0.10),
                (0.30, 0.47, 0.31, 0.48),
                (-0.10, -0.20, 0.24, 0.55),
                (0.56, 0.08, 0.22, -0.28),
                (-0.58, 0.12, 0.22, 0.41),
                (0.00, 0.40, 0.25, -0.46),
                (-0.27, -0.67, 0.22, 0.12),
                (0.37, -0.65, 0.22, -0.44),
            ];
            for y in 0..N {
                for x in 0..N {
                    let p = Vec2::new(
                        (x as f32 + 0.5) / N as f32 * 2.0 - 1.0,
                        (y as f32 + 0.5) / N as f32 * 2.0 - 1.0,
                    );
                    let mut fan = false;
                    let mut shade: f32 = 0.0;
                    for (cx, cy, scale, turn) in FANS {
                        let rel = (p - Vec2::new(cx, cy)) / scale;
                        let q = Vec2::new(
                            rel.x * turn.cos() + rel.y * turn.sin(),
                            -rel.x * turn.sin() + rel.y * turn.cos(),
                        );
                        let radial = Vec2::new(q.x, q.y + 0.54);
                        let notch = !(q.y > 0.34 && q.x.abs() < 0.10);
                        let crown = q.y > -0.58 && radial.length() < 1.03 && q.y < 0.82 && notch;
                        let stem = q.y < -0.42 && q.y > -0.92 && q.x.abs() < 0.075;
                        if crown || stem {
                            fan = true;
                            let vein = (q.x * 18.0).sin().abs() * 0.06;
                            shade = shade.max(
                                (0.42 + (q.y + 0.8).clamp(0.0, 1.4) * 0.35 - vein).clamp(0.0, 1.0),
                            );
                        }
                    }
                    let i = ((y * N + x) * 4) as usize;
                    for c in 0..3 {
                        base[i + c] =
                            (dark[c] as f32 * (1.0 - shade) + light[c] as f32 * shade) as u8;
                    }
                    base[i + 3] = if fan { 255 } else { 0 };
                }
            }
        }
        _ => return None,
    }
    image::RgbaImage::from_raw(N, N, base)
}

/// A compact alpha-cut blossom cluster shared by every sakura card. It is generated rather than
/// pretending a green photographed leaf is a flower; the hand-painted palette remains distinct
/// when the normal restyle path asks for its texture set.
fn blossom_texture(pink: bool, art: crate::art_style::ArtStyle) -> Image {
    const N: u32 = 128;
    let file = if pink {
        "procedural_blossom_pink"
    } else {
        "procedural_blossom_white"
    };
    let base = generated_leaf_rgba(file, art)
        .expect("known blossom generator")
        .into_raw();
    let lut = srgb_to_lin_lut();
    // (with its mip chain: `Image::new` checks the data is the base level only)
    let mut out = Image::new_uninit(
        Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.data = Some(cutout_mips(&base, N, &lut));
    out.texture_descriptor.mip_level_count = N.trailing_zeros() + 1;
    out.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    out
}

/// A fan-shaped, alpha-cut ginkgo leaf. The mesh supplies fine pleats while this alpha mask
/// preserves the notched fan silhouette in both foliage styles.
fn ginkgo_texture(art: crate::art_style::ArtStyle) -> Image {
    const N: u32 = 128;
    let base = generated_leaf_rgba("procedural_ginkgo_fan", art)
        .expect("known ginkgo generator")
        .into_raw();
    let lut = srgb_to_lin_lut();
    // (with its mip chain: `Image::new` checks the data is the base level only)
    let mut out = Image::new_uninit(
        Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.data = Some(cutout_mips(&base, N, &lut));
    out.texture_descriptor.mip_level_count = N.trailing_zeros() + 1;
    out.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    out
}

fn load_foliage(art: crate::art_style::ArtStyle) -> FoliageTextures {
    let dir = foliage_dir();
    std::thread::scope(|s| {
        let b: Vec<_> = BarkTex::ALL
            .iter()
            .map(|b| {
                let d = dir.clone();
                s.spawn(move || {
                    let [albedo, normal] = crate::art_style::textures(
                        [
                            d.join(format!("{}_albedo.png", b.file())),
                            d.join(format!("{}_normal.png", b.file())),
                        ],
                        art,
                    );
                    (
                        load_texture(&albedo, true, false, true),
                        load_texture(&normal, false, false, true),
                    )
                })
            })
            .collect();
        let l: Vec<_> = LeafTex::ALL
            .iter()
            .map(|l| {
                let d = dir.clone();
                s.spawn(move || {
                    l.blossom()
                        .map(|pink| blossom_texture(pink, art))
                        .or_else(|| l.ginkgo().then(|| ginkgo_texture(art)))
                        .or_else(|| {
                            load_texture(
                                &crate::art_style::texture(
                                    &d.join(format!("{}.png", l.file())),
                                    art,
                                ),
                                true,
                                true,
                                false,
                            )
                        })
                })
            })
            .collect();
        (
            b.into_iter().map(|h| h.join().unwrap()).collect(),
            l.into_iter().map(|h| h.join().unwrap()).collect(),
        )
    })
}

/// Builds every tree species (ez-tree meshes, materials, textures) into the library.
/// Called from `objects::build_library`; returns the parts per asset kind. (Realistic textures:
/// no world is open yet; `restyle` repaints them for a hand-painted world.)
pub fn build(
    meshes: &mut Assets<Mesh>,
    foliage: &mut Assets<FoliageMaterial>,
    images: &mut Assets<Image>,
    billboards: &mut Assets<BillboardMaterial>,
) -> (HashMap<AssetKind, TreeParts>, ImpostorAtlas) {
    let t0 = std::time::Instant::now();
    let art = crate::art_style::ArtStyle::Realistic;
    let (barks, leaves) = load_foliage(art);
    // (block-compressed in the background where the GPU samples BC: `model_render::mips`)
    let mut add = |i: Image, normal_map: bool| {
        let h = images.add(i);
        crate::model_render::mips::compress_later(h.id(), normal_map);
        h
    };
    let mut bark_mats = HashMap::new();
    for (b, (albedo, normal)) in BarkTex::ALL.iter().zip(barks) {
        let mat = foliage.add(FoliageMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: albedo.map(|i| add(i, false)),
                normal_map_texture: normal.map(|i| add(i, true)),
                perceptual_roughness: b.roughness(art),
                reflectance: b.reflectance(),
                ..default()
            },
            extension: FoliageExt::default(),
        });
        bark_mats.insert(*b, mat);
    }
    let mut leaf_mats = HashMap::new();
    for (l, tex) in LeafTex::ALL.iter().zip(leaves) {
        let appearance = l.appearance(art);
        let mat = foliage.add(FoliageMaterial {
            base: StandardMaterial {
                base_color: appearance.base_color(),
                base_color_texture: tex.map(|i| add(i, false)),
                alpha_mode: AlphaMode::Mask(0.5),
                // both faces render; normals are ez-tree's rounded canopy normals, never flipped
                cull_mode: None,
                double_sided: false,
                perceptual_roughness: appearance.roughness,
                reflectance: appearance.reflectance,
                diffuse_transmission: appearance.transmission,
                emissive: appearance.fill,
                ..default()
            },
            extension: FoliageExt::default(),
        });
        leaf_mats.insert(*l, mat);
    }

    // grow every variant in parallel
    let all = species();
    let jobs: Vec<(usize, &str)> = all
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.sizes.iter().map(move |(p, _)| (i, *p)))
        .collect();
    let built: Vec<(usize, Option<BuiltVariant>)> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(8).max(1))
            .map(|chunk| {
                let all = &all;
                s.spawn(move || {
                    chunk
                        .iter()
                        .map(|&(i, p)| (i, grow_variant(&all[i], p)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });

    // impostor atlas material: gets its texture once the captures are read back
    let atlas_mat = billboards.add(BillboardMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Mask(0.5),
            cull_mode: None,
            double_sided: false,
            perceptual_roughness: 0.95,
            reflectance: 0.12,
            diffuse_transmission: 0.2,
            ..default()
        },
        extension: BillboardExt::default(),
    });

    let mut parts: HashMap<AssetKind, TreeParts> = HashMap::new();
    let mut jobs_out = vec![];
    let mut cell = 0u32;
    let mut tris = 0usize;
    for (i, sp) in all.iter().enumerate() {
        let bark_tint = srgb_tint(sp.bark_tint);
        let leaf_tint = srgb_tint(sp.leaf_tint);
        let small = sp.class != Class::Tree;
        let card = sp.class == Class::Card;
        let mut variants = vec![];
        for (vi, b) in built
            .iter()
            .filter(|(k, _)| *k == i)
            .filter_map(|(_, b)| b.as_ref())
            .enumerate()
        {
            let lods = b
                .lods
                .iter()
                .map(|(br, lv)| {
                    tris +=
                        br.triangle_count() + lv.as_ref().map(|l| l.triangle_count()).unwrap_or(0);
                    // photo cards use the cut-out leaf texture (flipped v like leaves)
                    let tile = sp.bark.tile();
                    let first = if card {
                        to_mesh(br, b.scale, leaf_tint, true, tile)
                    } else {
                        to_mesh(br, b.scale, bark_tint, false, tile)
                    };
                    (
                        meshes.add(first),
                        lv.as_ref()
                            .map(|l| meshes.add(to_mesh(l, b.scale, leaf_tint, true, tile))),
                    )
                })
                .collect();
            let billboard = (!small && cell < CELLS * CELLS).then(|| {
                let c = cell;
                cell += 1;
                jobs_out.push((format!("{:?}", sp.kind), vi, c));
                c
            });
            variants.push(TreeVariant {
                lods,
                cell: billboard,
                height: b.height,
                width: b.width,
                min_y: b.min_y,
                trunk: b.trunk,
            });
        }
        if variants.is_empty() {
            continue;
        }
        let ranges = match sp.class {
            Class::Tree => [40.0, 110.0, 320.0, 6000.0],
            Class::Shrub => [22.0, 55.0, 110.0, 110.0],
            Class::Card => [75.0, 75.0, 75.0, 75.0],
        };
        let bark = if card {
            leaf_mats[&sp.leaf].clone()
        } else {
            bark_mats[&sp.bark].clone()
        };
        parts.insert(
            sp.kind,
            TreeParts {
                variants,
                bark,
                leaves: leaf_mats[&sp.leaf].clone(),
                ranges,
            },
        );
    }
    assert!(
        cell <= CELLS * CELLS,
        "tree impostor atlas overflow: {cell} requested, {} available",
        CELLS * CELLS
    );
    info!(
        "grew {} tree variants ({} triangles over all LODs) in {:?}",
        jobs_out.len(),
        tris,
        t0.elapsed()
    );
    let targets = capture_targets(jobs_out.len(), images);
    let bark_mats = BarkTex::ALL
        .iter()
        .filter_map(|b| bark_mats.get(b).map(|m| (*b, m.clone())))
        .collect();
    let leaf_mats = LeafTex::ALL
        .iter()
        .filter_map(|l| leaf_mats.get(l).map(|m| (*l, m.clone())))
        .collect();
    (
        parts,
        ImpostorAtlas {
            material: atlas_mat,
            jobs: jobs_out,
            targets,
            cells: HashMap::new(),
            state: CaptureState::Waiting,
            bark_mats,
            leaf_mats,
            art,
            recapture: false,
        },
    )
}

/// The capture targets of every atlas cell (one small render target each).
fn capture_targets(n: usize, images: &mut Assets<Image>) -> Vec<Handle<Image>> {
    (0..n)
        .map(|_| {
            let mut t = Image::new_target_texture(
                CELL,
                CELL,
                TextureFormat::Rgba8Unorm,
                Some(TextureFormat::Rgba8UnormSrgb),
            );
            t.texture_descriptor.usage |= TextureUsages::COPY_SRC;
            images.add(t)
        })
        .collect()
}

/// Bark and leaf textures loading in another art style: (the style, the textures).
#[derive(Resource)]
struct Restyle(bevy::tasks::Task<(crate::art_style::ArtStyle, FoliageTextures)>);

/// The world's art style changed: the bark and leaf textures load in the new style in the
/// background, then every species' materials take them (same materials, new textures) and the
/// distant-tree pictures are taken again.
fn restyle(
    mut commands: Commands,
    atlas: Option<ResMut<ImpostorAtlas>>,
    task: Option<ResMut<Restyle>>,
    meta: Res<crate::world::WorldMeta>,
    mut foliage: ResMut<Assets<FoliageMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut atlas) = atlas else { return };
    let art = crate::art_style::surfaces(meta.art_style);
    let Some(mut task) = task else {
        if art != atlas.art {
            commands.insert_resource(Restyle(
                bevy::tasks::AsyncComputeTaskPool::get()
                    .spawn(async move { (art, load_foliage(art)) }),
            ));
        }
        return;
    };
    let Some((loaded, (barks, leaves))) =
        bevy::tasks::block_on(bevy::tasks::futures_lite::future::poll_once(&mut task.0))
    else {
        return;
    };
    commands.remove_resource::<Restyle>();
    if loaded != art {
        // (changed again meanwhile: the next frame loads the newest)
        return;
    }
    let mut add = |i: Image, normal_map: bool| {
        let h = images.add(i);
        crate::model_render::mips::compress_later(h.id(), normal_map);
        h
    };
    for ((b, (albedo, normal)), (b2, mat)) in
        BarkTex::ALL.iter().zip(barks).zip(atlas.bark_mats.clone())
    {
        debug_assert_eq!(*b, b2);
        if let Some(mut m) = foliage.get_mut(&mat) {
            m.base.perceptual_roughness = b.roughness(art);
            m.base.reflectance = b.reflectance();
            if let Some(a) = albedo {
                m.base.base_color_texture = Some(add(a, false));
            }
            if let Some(n) = normal {
                m.base.normal_map_texture = Some(add(n, true));
            }
        }
    }
    for ((l, tex), (l2, mat)) in LeafTex::ALL.iter().zip(leaves).zip(atlas.leaf_mats.clone()) {
        debug_assert_eq!(*l, l2);
        if let Some(mut m) = foliage.get_mut(&mat) {
            let appearance = l.appearance(art);
            m.base.base_color = appearance.base_color();
            m.base.perceptual_roughness = appearance.roughness;
            m.base.reflectance = appearance.reflectance;
            m.base.diffuse_transmission = appearance.transmission;
            m.base.emissive = appearance.fill;
            if let Some(t) = tex {
                m.base.base_color_texture = Some(add(t, false));
            }
        }
    }
    atlas.art = art;
    atlas.recapture = true;
    info!("trees repainted: {} bark and leaf textures", art.name());
}

/// One species as plain CPU data for the engine exporters (`crate::export`). Uploaded meshes keep
/// no main-world copy, so the species is grown again (same presets and seeds: same trees).
pub struct SpeciesExport {
    /// `textures/foliage/<bark>_albedo.png` + `_normal.png`
    pub bark: &'static str,
    /// `textures/foliage/<leaf>.png` (alpha cut-out)
    pub leaf: &'static str,
    /// Photo-card plants: the first mesh of every level of detail wears the leaf texture.
    pub card: bool,
    /// Linear colour multipliers the editor applies through vertex colours (above 1 brightens).
    pub bark_tint: [f32; 3],
    pub leaf_tint: [f32; 3],
    /// Variants in library order: (bark, leaves) per level of detail.
    pub variants: Vec<Vec<(Mesh, Option<Mesh>)>>,
}

/// An extra, much lighter fourth level of detail (about half of LOD 2: every sixth leaf, enlarged
/// to keep the crown's coverage, and the coarsest branches): procedural nature draws it for the
/// last stretch before the billboards, and engines without the editor's impostors draw distant
/// trees with it.
const FAR: Detail = Detail {
    section_stride: 12,
    segment_factor: 0.25,
    leaf_stride: 6,
    leaf_scale: 2.25,
    billboard: Some(ez_tree::Billboard::Single),
};

/// Grows the given species on the CPU for export (in parallel, like `build`). ez-tree species
/// get the far level of detail (`FAR`) as a fourth LOD.
pub fn export_species(kinds: &[AssetKind]) -> Vec<SpeciesExport> {
    let all: Vec<Species> = species()
        .into_iter()
        .filter(|s| kinds.contains(&s.kind))
        .collect();
    let jobs: Vec<(usize, &str)> = all
        .iter()
        .enumerate()
        .flat_map(|(i, s)| s.sizes.iter().map(move |(p, _)| (i, *p)))
        .collect();
    let built: Vec<(usize, Option<BuiltVariant>)> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(i, p)| {
                let sp = &all[i];
                (i, s.spawn(move || grow_variant(sp, p)))
            })
            .collect();
        handles
            .into_iter()
            .map(|(i, h)| (i, h.join().ok().flatten()))
            .collect()
    });
    all.iter()
        .enumerate()
        .map(|(i, sp)| {
            let card = sp.class == Class::Card;
            let bark_tint = srgb_tint(sp.bark_tint);
            // Export bakes the material tint into the leaf texture (rather than retaining the
            // editor's runtime material), so preserve the selected art style's foliage palette.
            let style_tint = sp
                .leaf
                .appearance(crate::art_style::surfaces(crate::art_style::current()))
                .tint;
            let authored_tint = srgb_tint(sp.leaf_tint);
            let leaf_tint = [
                authored_tint[0] * style_tint[0],
                authored_tint[1] * style_tint[1],
                authored_tint[2] * style_tint[2],
            ];
            let tile = sp.bark.tile();
            let variants = built
                .iter()
                .filter(|(k, _)| *k == i)
                .filter_map(|(_, b)| b.as_ref())
                .map(|b| {
                    b.lods
                        .iter()
                        .map(|(br, lv)| {
                            let first = if card {
                                to_mesh(br, b.scale, leaf_tint, true, tile)
                            } else {
                                to_mesh(br, b.scale, bark_tint, false, tile)
                            };
                            (
                                first,
                                lv.as_ref()
                                    .map(|l| to_mesh(l, b.scale, leaf_tint, true, tile)),
                            )
                        })
                        .collect()
                })
                .collect();
            SpeciesExport {
                bark: sp.bark.file(),
                leaf: sp.leaf.file(),
                card,
                bark_tint,
                leaf_tint,
                variants,
            }
        })
        .collect()
}

pub struct TreesPlugin;

impl Plugin for TreesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (restyle, capture_impostors).chain());
    }
}

#[derive(Component)]
struct CaptureRig;

/// Renders a front picture of every tree variant (transparent background, soft frontal light),
/// reads them back once, packs them into the atlas and swaps in a mipmapped, coverage-preserving
/// texture for the billboard material.
fn capture_impostors(
    mut commands: Commands,
    atlas: Option<ResMut<ImpostorAtlas>>,
    lib: Option<Res<Library>>,
    rigs: Query<Entity, With<CaptureRig>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<BillboardMaterial>>,
) {
    let (Some(mut atlas), Some(lib)) = (atlas, lib) else {
        return;
    };
    match atlas.state {
        // new textures (another art style): take the pictures again
        CaptureState::Done if atlas.recapture => {
            atlas.recapture = false;
            let n = atlas.jobs.len();
            atlas.targets = capture_targets(n, &mut images);
            atlas.state = CaptureState::Waiting;
        }
        CaptureState::Done => {}
        CaptureState::Waiting => {
            let layer = RenderLayers::layer(CAPTURE_LAYER);
            commands.spawn((
                CaptureRig,
                DirectionalLight {
                    illuminance: 2.6,
                    shadow_maps_enabled: false,
                    ..default()
                },
                Transform::from_xyz(0.3, 0.8, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
                layer.clone(),
            ));
            for (n, (id, vi, _cell)) in atlas.jobs.iter().enumerate() {
                let Some(tree) = lib.get(id).and_then(|e| e.tree.as_ref()) else {
                    continue;
                };
                let Some(v) = tree.variants.get(*vi) else {
                    continue;
                };
                let origin = Vec3::new(n as f32 * 300.0, -20_000.0, 0.0);
                let (bark, leaves) = &v.lods[0];
                commands.spawn((
                    CaptureRig,
                    Mesh3d(bark.clone()),
                    MeshMaterial3d(tree.bark.clone()),
                    Transform::from_translation(origin),
                    layer.clone(),
                    NotShadowCaster,
                ));
                if let Some(l) = leaves {
                    commands.spawn((
                        CaptureRig,
                        Mesh3d(l.clone()),
                        MeshMaterial3d(tree.leaves.clone()),
                        Transform::from_translation(origin),
                        layer.clone(),
                        NotShadowCaster,
                    ));
                }
                let centre = origin + Vec3::Y * (v.min_y + v.height * 0.5);
                commands.spawn((
                    CaptureRig,
                    Camera3d::default(),
                    Camera {
                        order: -200 - n as isize,
                        clear_color: ClearColorConfig::Custom(Color::NONE),
                        ..default()
                    },
                    RenderTarget::Image(atlas.targets[n].clone().into()),
                    Projection::Orthographic(OrthographicProjection {
                        scaling_mode: ScalingMode::Fixed {
                            width: v.width,
                            height: v.height,
                        },
                        near: 0.0,
                        far: 400.0,
                        ..OrthographicProjection::default_3d()
                    }),
                    Transform::from_translation(centre + Vec3::Z * 200.0)
                        .looking_at(centre, Vec3::Y),
                    bevy::core_pipeline::tonemapping::Tonemapping::None,
                    bevy::core_pipeline::tonemapping::DebandDither::Disabled,
                    bevy::camera::Exposure { ev100: 0.0 },
                    AmbientLight {
                        color: Color::WHITE,
                        brightness: 0.3,
                        affects_lightmapped_meshes: false,
                    },
                    Msaa::Sample4,
                    layer.clone(),
                ));
            }
            atlas.state = CaptureState::Rendering(0);
        }
        CaptureState::Rendering(f) => {
            // give pipelines time to compile before reading back
            if f >= 45 {
                atlas.cells.clear();
                for (n, (_, _, cell)) in atlas.jobs.clone().into_iter().enumerate() {
                    let target = atlas.targets[n].clone();
                    commands
                        .spawn((CaptureRig, ReadbackOnce::texture(target)))
                        .observe(
                            move |ev: On<ReadbackComplete>, mut atlas: ResMut<ImpostorAtlas>| {
                                atlas.cells.insert(cell, ev.data.clone());
                            },
                        );
                }
                atlas.state = CaptureState::ReadingBack(0);
            } else {
                atlas.state = CaptureState::Rendering(f + 1);
            }
        }
        CaptureState::ReadingBack(f) => {
            if atlas.cells.len() >= atlas.jobs.len() {
                let cell_bytes = (CELL * CELL * 4) as usize;
                let opaque: usize = atlas
                    .cells
                    .values()
                    .map(|d| {
                        d.chunks_exact(4)
                            .take(cell_bytes / 4)
                            .filter(|p| p[3] > 127)
                            .count()
                    })
                    .sum();
                if opaque < 2000 && f < 600 {
                    // pipelines weren't ready yet: render a little longer and try again
                    atlas.state = CaptureState::Rendering(30);
                    return;
                }
                let mut px = vec![0u8; (ATLAS * ATLAS * 4) as usize];
                for (cell, data) in &atlas.cells {
                    if data.len() < cell_bytes {
                        continue;
                    }
                    let (cx, cy) = (cell % CELLS, cell / CELLS);
                    for row in 0..CELL {
                        let src = (row * CELL * 4) as usize;
                        let dst = (((cy * CELL + row) * ATLAS + cx * CELL) * 4) as usize;
                        px[dst..dst + (CELL * 4) as usize]
                            .copy_from_slice(&data[src..src + (CELL * 4) as usize]);
                    }
                }
                let handle = images.add(atlas_image(px));
                if let Some(mut m) = mats.get_mut(&atlas.material) {
                    m.base.base_color_texture = Some(handle);
                }
                for e in &rigs {
                    commands.entity(e).despawn();
                }
                let targets = std::mem::take(&mut atlas.targets);
                for t in targets {
                    images.remove(&t);
                }
                atlas.state = CaptureState::Done;
                info!(
                    "tree impostor atlas captured: {} cells, {opaque} opaque texels",
                    atlas.cells.len()
                );
                atlas.cells.clear();
            } else if f > 600 {
                warn!("tree impostor readback timed out");
                for e in &rigs {
                    commands.entity(e).despawn();
                }
                atlas.state = CaptureState::Done;
            } else {
                atlas.state = CaptureState::ReadingBack(f + 1);
            }
        }
    }
}

/// Mipmapped atlas texture (mips stop at 32 px cells so neighbouring trees don't mix).
fn atlas_image(mut px: Vec<u8>) -> Image {
    let lut = srgb_to_lin_lut();
    bleed_rgb(&mut px, ATLAS);
    let full = cutout_mips(&px, ATLAS, &lut);
    let levels = CELL.trailing_zeros() - 4;
    let mut len = 0usize;
    let mut s = ATLAS;
    for _ in 0..levels {
        len += (s * s * 4) as usize;
        s /= 2;
    }
    let mut img = Image::new_uninit(
        Extent3d {
            width: ATLAS,
            height: ATLAS,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(full[..len].to_vec());
    img.texture_descriptor.mip_level_count = levels;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 4,
        ..default()
    });
    img
}

/// Fills transparent texels with the nearest opaque colour (a few dilation passes).
fn bleed_rgb(px: &mut [u8], size: u32) {
    let s = size as i32;
    let mut known: Vec<bool> = px.chunks_exact(4).map(|p| p[3] > 8).collect();
    for _ in 0..6 {
        let mut next = known.clone();
        let src = px.to_vec();
        for y in 0..s {
            for x in 0..s {
                let i = (y * s + x) as usize;
                if known[i] {
                    continue;
                }
                let mut acc = [0u32; 3];
                let mut n = 0;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= s || ny >= s {
                        continue;
                    }
                    let j = (ny * s + nx) as usize;
                    if known[j] {
                        for c in 0..3 {
                            acc[c] += src[j * 4 + c] as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    for c in 0..3 {
                        px[i * 4 + c] = (acc[c] / n) as u8;
                    }
                    next[i] = true;
                }
            }
        }
        known = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_leaf_sources_are_alpha_cut_and_style_specific() {
        let pink = generated_leaf_rgba(
            "procedural_blossom_pink",
            crate::art_style::ArtStyle::Realistic,
        )
        .expect("pink blossom");
        let painted = generated_leaf_rgba(
            "procedural_blossom_pink",
            crate::art_style::ArtStyle::HandPainted,
        )
        .expect("painted pink blossom");
        let fan = generated_leaf_rgba(
            "procedural_ginkgo_fan",
            crate::art_style::ArtStyle::Realistic,
        )
        .expect("ginkgo fan");
        assert_eq!(pink.dimensions(), (128, 128));
        assert_eq!(fan.dimensions(), (128, 128));
        assert!(pink.pixels().any(|p| p[3] == 0));
        assert!(pink.pixels().any(|p| p[3] > 127));
        let blossom_coverage = pink.pixels().filter(|p| p[3] >= 128).count() as f32
            / (pink.width() * pink.height()) as f32;
        assert!(
            (0.35..=0.55).contains(&blossom_coverage),
            "blossom alpha coverage must sustain a full canopy: {blossom_coverage:.3}"
        );
        let fan_coverage = fan.pixels().filter(|p| p[3] >= 128).count() as f32
            / (fan.width() * fan.height()) as f32;
        assert!(
            (0.35..=0.45).contains(&fan_coverage),
            "ginkgo cluster alpha coverage must sustain a full canopy: {fan_coverage:.3}"
        );
        assert_ne!(pink.as_raw(), painted.as_raw());
        assert!(generated_leaf_rgba("leaf_oak", crate::art_style::ArtStyle::Realistic).is_none());
    }

    #[test]
    fn painted_leaf_appearance_is_vivid_without_changing_realistic_or_bamboo() {
        use crate::art_style::ArtStyle::{HandPainted, Realistic};

        let realistic = LeafTex::GinkgoFan.appearance(Realistic);
        assert_eq!(realistic.tint, [1.0; 3]);
        assert_eq!(realistic.roughness, 0.72);
        assert_eq!(realistic.transmission, 0.35);

        let ginkgo = LeafTex::GinkgoFan.appearance(HandPainted);
        assert!(ginkgo.tint[0] > 1.0 && ginkgo.tint[1] > 1.0);
        assert!(ginkgo.tint[2] < 1.0);
        assert!(ginkgo.transmission > realistic.transmission);

        let bamboo = LeafTex::Bamboo.appearance(HandPainted);
        assert_eq!(bamboo.tint, [1.0; 3]);
        assert_eq!(bamboo.fill, LinearRgba::BLACK);
    }

    #[test]
    fn blossom_and_ginkgo_crowns_are_thinned_by_a_quarter() {
        let all = species();
        let count = |kind| {
            let sp = all.iter().find(|sp| sp.kind == kind).expect("tree species");
            options_for(sp, sp.sizes[1].0).expect("preset").leaves.count
        };
        assert_eq!(count(AssetKind::SakuraPink), 21);
        assert_eq!(count(AssetKind::SakuraWhite), 21);
        assert_eq!(count(AssetKind::Ginkgo), 24);
    }

    #[test]
    fn special_tree_crowns_use_the_shared_ez_tree_lod_pipeline() {
        use crate::procgen::meshes::AssetKind::{Ginkgo, SakuraPink, SakuraWhite, Sequoia};

        for kind in [SakuraPink, SakuraWhite, Ginkgo, Sequoia] {
            let sp = species()
                .into_iter()
                .find(|sp| sp.kind == kind)
                .expect("special tree species");
            assert_eq!(sp.grow, Gen::Ez, "{kind:?} must share ez-tree growth");
            let variant = grow_variant(&sp, sp.sizes[1].0).expect("grown variant");
            assert_eq!(variant.lods.len(), Detail::LOD_COUNT + 1);
            assert!(variant.trunk.is_some(), "{kind:?} collider");
            let leaves: Vec<_> = variant
                .lods
                .iter()
                .map(|(_, l)| l.as_ref().expect("foliage").triangle_count())
                .collect();
            assert!(leaves[0] > 1_000, "{kind:?} dense LOD 0 canopy");
            assert!(leaves[0] > leaves[1] && leaves[1] >= leaves[2]);
        }
    }

    #[test]
    fn bamboo_materials_keep_the_authored_texture_contract() {
        let sp = species()
            .into_iter()
            .find(|sp| sp.kind == AssetKind::Bamboo)
            .expect("bamboo species");
        assert_eq!(
            sp.sizes,
            &[("bamboo_a", 12.0), ("bamboo_b", 17.0), ("bamboo_c", 22.0)]
        );
        assert_eq!(sp.bark, BarkTex::Bamboo);
        assert_eq!(sp.leaf, LeafTex::Bamboo);
        assert_eq!(sp.bark.file(), "bark_bamboo");
        assert_eq!(sp.leaf.file(), "leaf_bamboo");
        assert_eq!(sp.bark.tile(), Vec2::ZERO);
        assert_eq!(
            sp.bark.roughness(crate::art_style::ArtStyle::Realistic),
            0.46
        );
        assert_eq!(
            sp.bark.roughness(crate::art_style::ArtStyle::HandPainted),
            0.72
        );
    }

    #[test]
    fn authored_bamboo_bark_uvs_survive_mesh_building() {
        let mut bark = ez_tree::Buffers::default();
        bark.positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        bark.normals = vec![[0.0, 0.0, 1.0]; 3];
        bark.uvs = vec![[0.0, 3.0], [0.5, 3.0], [0.0, 4.0]];
        bark.sway = vec![0.0; 3];
        bark.bark = vec![[0.0, 0.0, 1.0]; 3];
        bark.indices = vec![0, 1, 2];

        let mesh = to_mesh(&bark, 1.0, [1.0; 3], false, BarkTex::Bamboo.tile());
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uvs)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("bamboo mesh must retain UV0");
        };
        assert_eq!(uvs, &bark.uvs);
    }

    #[test]
    fn sequoia_hip_collider_tracks_the_generated_trunk_without_a_player_scale_cap() {
        let sp = species()
            .into_iter()
            .find(|sp| sp.kind == AssetKind::Sequoia)
            .expect("sequoia species");
        let variant = grow_variant(&sp, sp.sizes[1].0).expect("grown sequoia");
        let (radius, _) = variant.trunk.expect("sequoia trunk collider");
        let bark = &variant.lods[0].0;
        let (_, provenance) = bark
            .positions
            .iter()
            .zip(&bark.bark)
            .min_by(|(a, _), (b, _)| {
                (a[1] * variant.scale - TRUNK_AT as f32)
                    .abs()
                    .total_cmp(&(b[1] * variant.scale - TRUNK_AT as f32).abs())
            })
            .expect("sequoia bark at hip height");
        let visible_radius = provenance[2] * variant.scale / std::f32::consts::TAU;
        assert!(
            radius > 1.2,
            "giant sequoia must exceed the old 1.2 m cap: {radius}"
        );
        assert!(
            radius >= visible_radius * 0.7 && radius <= visible_radius * 1.35,
            "skeleton radius {radius} must follow generated hip bark {visible_radius}"
        );
        assert_eq!(
            crate::motion::collide::known_trunk_radius(radius),
            Some(radius),
            "walk-mode collision must not cap the measured sequoia radius"
        );
    }
}
