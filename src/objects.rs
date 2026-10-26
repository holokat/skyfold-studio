//! Placeable objects: the asset library (procedural nature + structures + imported glTF models),
//! placed instances with a spatial index, LOD + wind-swayed foliage, the vegetation presets (the
//! rules procedural nature grows by, `crate::nature`) and the Objects (place & select) tool.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::asset::uuid_handle;
use bevy::camera::visibility::VisibilityRange;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, MeshTag, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use serde::{Deserialize, Serialize};

use crate::camera::Pointer;
use crate::editor::{Editor, ObjectTool, Tab};
use crate::history::History;
use crate::procgen::meshes::{AssetKind, MeshData, build_asset};
use crate::terrain::Terrain;
use crate::terrain::layers::Surface;
use crate::tools::Stroke;
use crate::world::Biome;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Trees,
    Rocks,
    Plants,
    Debris,
    Structures,
    /// small photoscanned props: food, tableware, baskets…
    Props,
    Imported,
}

impl Category {
    pub const NATURE: [Category; 4] = [
        Category::Trees,
        Category::Rocks,
        Category::Plants,
        Category::Debris,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Category::Trees => "Trees",
            Category::Rocks => "Rocks",
            Category::Plants => "Plants",
            Category::Debris => "Debris",
            Category::Structures => "Structures",
            Category::Props => "Props",
            Category::Imported => "Imported",
        }
    }
    /// Default instances per 1000 m² at density 1.
    pub(crate) fn base_density(self) -> f32 {
        match self {
            Category::Trees => 7.0,
            Category::Rocks => 2.5,
            Category::Plants => 30.0,
            Category::Debris => 2.0,
            Category::Structures | Category::Props | Category::Imported => 0.5,
        }
    }
}

pub fn kind_category(k: AssetKind) -> Category {
    use AssetKind::*;
    match k {
        Pine | Fir | Spruce | SnowyPine | Cedar | Oak | Birch | Maple | Willow | Palm | Bamboo
        | DeadTree | GoldenMaple | SakuraPink | SakuraWhite | Ginkgo | Sequoia | QuiverTree => {
            Category::Trees
        }
        Boulder | RockCluster | CliffRock | Pebbles | MossyRock | RockSpire | Hoodoo | SeaStack
        | Crystals | FloatingRock | Monolith => Category::Rocks,
        Bush | FlowerBush | Fern | Flowers | TallGrass | Reeds | Cactus | Mushrooms
        | DesertShrub | LilyPads | Lavender | WildStrawberry | Blueberry | Blackberry | Tulip
        | Succulent => {
            Category::Plants
        }
        FallenLog | Stump | Branches | Driftwood | Roots => Category::Debris,
        _ => Category::Structures,
    }
}

/// Procedural low-poly placeholders: kept out of the builder (library, brushes, presets,
/// generation) until a photoscanned version replaces them.
pub fn kind_hidden(k: AssetKind) -> bool {
    use AssetKind::*;
    matches!(
        k,
        FlowerBush
            | WoodenBridge
            | StoneBridge
            | Hut
            | Fence
            | RuinPillar
            | RuinWall
            | Watchtower
            | Campfire
            | Signpost
            | Well
            | Dock
    )
}

fn kind_biomes(k: AssetKind) -> Vec<Biome> {
    use AssetKind::*;
    use Biome::*;
    match k {
        Pine => vec![Boreal, Alpine, Temperate],
        Fir => vec![Boreal, Alpine],
        Spruce => vec![Boreal, Alpine, Tundra],
        SnowyPine => vec![Alpine, Tundra],
        Cedar => vec![Temperate, Alpine],
        Oak => vec![Temperate],
        Birch => vec![Temperate, Boreal],
        Maple => vec![Autumn, Temperate],
        GoldenMaple | Ginkgo => vec![Autumn, Temperate],
        SakuraPink | SakuraWhite | Sequoia => vec![Temperate],
        Willow => vec![Swamp, Temperate],
        Palm => vec![Tropical, Desert],
        Bamboo => vec![Tropical],
        DeadTree => vec![Swamp, Volcanic, Desert, Tundra, Autumn, Temperate],
        MossyRock => vec![Temperate, Boreal, Swamp, Tropical, Autumn, Alpine],
        RockSpire => vec![Desert, Alpine, Volcanic, Tundra],
        Hoodoo => vec![Desert, Volcanic],
        SeaStack => vec![Tropical, Temperate, Boreal, Desert, Volcanic],
        Crystals => vec![Volcanic, Tundra, Alpine, Swamp],
        Bush => vec![Temperate, Autumn, Boreal, Tropical],
        FlowerBush => vec![Temperate, Tropical],
        Fern => vec![Temperate, Boreal, Tropical, Swamp],
        Flowers => vec![Temperate, Boreal, Alpine, Autumn],
        Lavender | Tulip => vec![Temperate, Autumn],
        WildStrawberry | Blackberry => vec![Temperate, Boreal, Autumn],
        Blueberry => vec![Temperate, Boreal, Alpine, Autumn],
        TallGrass => vec![Temperate, Autumn, Swamp, Tundra, Boreal],
        Reeds => vec![Swamp, Temperate],
        Cactus => vec![Desert],
        Mushrooms => vec![Temperate, Boreal, Swamp, Autumn, Tropical],
        FallenLog | Stump | Roots => vec![Temperate, Boreal, Autumn, Swamp, Tropical],
        Driftwood => vec![Tropical, Temperate, Desert, Boreal],
        DesertShrub => vec![Desert, Volcanic, Alpine],
        LilyPads => vec![Swamp, Temperate, Tropical, Autumn],
        // alien-world rocks belong to no biome: they're only placed on purpose (or by the sci-fi
        // presets and `visuals::scifi_props`), never grown into a fantasy world's nature
        FloatingRock | Monolith => vec![],
        // alien flora (sci-fi template): only placed on purpose or by the sci-fi scatter
        QuiverTree | Succulent => vec![],
        _ => Biome::ALL.to_vec(),
    }
}

/// What a walker hears when stepping on / through this object.
fn kind_surface(k: AssetKind) -> Surface {
    use AssetKind::*;
    match k {
        StoneBridge | RuinPillar | RuinWall | Well | Boulder | RockCluster | CliffRock
        | MossyRock | RockSpire | Hoodoo | SeaStack | Crystals | FloatingRock | Monolith => {
            Surface::Stone
        }
        Pebbles => Surface::Gravel,
        Bush | FlowerBush | Fern | Flowers | TallGrass | Reeds | Cactus | DesertShrub
        | Lavender | WildStrawberry | Blueberry | Blackberry | Tulip | Succulent => Surface::Grass,
        LilyPads => Surface::Water,
        Mushrooms | Branches => Surface::Leaves,
        Tent => Surface::Carpet,
        Campfire => Surface::Ash,
        _ => Surface::Wood,
    }
}

/// Footprint radius (m) used for spacing and picking at scale 1.
fn kind_radius(k: AssetKind) -> f32 {
    use AssetKind::*;
    match k {
        Pine | Fir | Spruce | SnowyPine => 3.2,
        Cedar | Oak | Maple | GoldenMaple | SakuraPink | SakuraWhite | Ginkgo | Willow => 5.0,
        Sequoia => 7.5,
        Birch | Palm => 3.0,
        Bamboo => 2.2,
        DeadTree => 2.6,
        Boulder | MossyRock => 1.8,
        RockCluster => 3.0,
        CliffRock => 5.0,
        RockSpire => 4.0,
        Hoodoo => 2.5,
        SeaStack => 10.0,
        Crystals => 1.8,
        Pebbles => 1.2,
        Bush | FlowerBush => 1.2,
        Fern | Flowers | TallGrass | Mushrooms | Lavender | WildStrawberry => 0.6,
        Tulip => 0.15,
        Blueberry | Blackberry => 0.9,
        Reeds => 0.8,
        Cactus => 1.2,
        FallenLog => 3.0,
        Stump | Branches => 1.0,
        Driftwood => 2.5,
        WoodenBridge => 12.0,
        StoneBridge => 15.0,
        Hut => 5.0,
        Tent => 2.5,
        Fence => 2.0,
        RuinPillar => 1.5,
        RuinWall => 3.5,
        Watchtower => 3.5,
        Campfire => 1.2,
        Crate | Barrel => 0.7,
        Signpost => 0.6,
        Banner => 0.9,
        TarpShelter => 2.2,
        Well => 1.8,
        Dock => 6.0,
        DesertShrub => 1.0,
        LilyPads => 1.4,
        Roots => 2.2,
        FloatingRock => 6.0,
        Monolith => 2.2,
        QuiverTree => 2.4,
        Succulent => 0.7,
    }
}

/// The Objects tab group of a built-in structure.
fn kind_group(k: AssetKind) -> &'static str {
    use AssetKind::*;
    match k {
        Tent | TarpShelter => "camp",
        Crate | Barrel => "containers",
        Banner => "decor",
        _ => "",
    }
}

fn kind_align(k: AssetKind) -> bool {
    // (floating rocks hang level in the air, monoliths stand upright)
    (matches!(kind_category(k), Category::Rocks | Category::Debris)
        || matches!(k, AssetKind::Fern | AssetKind::Mushrooms))
        && !matches!(k, AssetKind::FloatingRock | AssetKind::Monolith)
}

fn kind_walkable(k: AssetKind) -> bool {
    matches!(
        k,
        AssetKind::WoodenBridge | AssetKind::StoneBridge | AssetKind::Dock
    )
}

/// (LOD swap distance, cull distance)
/// (distance where the far LOD takes over, distance where the object disappears) in metres;
/// 0 for the second means there is no far LOD. Objects only exist as render entities within the
/// second distance (see streaming.rs), so small things are cheap however many there are.
fn kind_lod(k: AssetKind) -> (f32, f32) {
    use AssetKind::*;
    match k {
        Pebbles | MossyRock | RockCluster => (60.0, 260.0),
        Boulder => (110.0, 900.0),
        CliffRock | RockSpire | Hoodoo => (160.0, 2500.0),
        SeaStack => (220.0, 4000.0),
        Crystals => (60.0, 600.0),
        FloatingRock => (120.0, 4000.0),
        Monolith => (90.0, 3000.0),
        _ => match kind_category(k) {
            // procedural trees (ez-tree trees use their own ranges and billboards)
            Category::Trees => (170.0, 1500.0),
            // small plants only render up close (no far LOD): from afar they're sub-pixel noise
            Category::Plants => (90.0, 0.0),
            Category::Debris => (80.0, 300.0),
            _ => (260.0, 8000.0),
        },
    }
}

/// How far (m, 3D) from the viewer an object is drawn at all: it exists as a render entity only
/// inside this distance. Trees stop at their last mesh LOD (billboard chunks carry on beyond).
/// `size`: the object's `size_band`.
pub fn stream_dist(e: &LibraryEntry, g: &crate::streaming::Graphics, size: f32) -> f32 {
    if let Some(t) = &e.tree {
        return sized(t.ranges[2] * g.detail, size);
    }
    match e.kind {
        Some(k) => {
            let (lod_d, cull) = kind_lod(k);
            if cull > lod_d {
                sized((cull * g.detail.max(1.0)).min(g.draw_distance), size)
            } else {
                lod_d * 1.15 * g.detail
            }
        }
        // props draw while they're a few pixels big: food and tableware only up close, crates
        // and headstones from across a field, a giant's ribcage as far as anything
        None if e.category == Category::Props => {
            ((e.radius * 350.0).max(90.0) * g.detail).min(g.draw_distance.max(90.0))
        }
        None => g.draw_distance,
    }
}

/// Giant rocks (a 300 m rock spire, a 60 m floating island) hold each level of detail and stay
/// drawn proportionally further: their distance bands × this. 1 at the sizes fantasy worlds use
/// (under 2×); powers of two above, so Bevy's table of distinct ranges stays small.
pub fn size_band(e: &LibraryEntry, scale: f32) -> f32 {
    use AssetKind::*;
    let grows = matches!(
        e.kind,
        Some(RockSpire | Hoodoo | SeaStack | Crystals | CliffRock | FloatingRock | Monolith)
    );
    if !grows || scale < 2.0 {
        return 1.0;
    }
    2f32.powi((scale.log2().floor() as i32).min(5))
}

/// A distance band `d` (m) of an object in size band `size`: further for giants, up to 12 km.
fn sized(d: f32, size: f32) -> f32 {
    (d * size).min(d.max(12_000.0))
}

/// The bundled CC0 food & tableware models (`assets/models/food/food.json`) and model libraries.
fn prop_entries() -> Vec<LibraryEntry> {
    #[derive(Deserialize)]
    struct Item {
        id: String,
        name: String,
        size_m: Vec<f32>,
        path: String,
        entry: String,
    }
    #[derive(Deserialize)]
    struct Doc {
        items: Vec<Item>,
    }
    let dir = std::path::PathBuf::from(crate::assets_path()).join("models");
    let food = std::fs::read_to_string(dir.join("food/food.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Doc>(&s).ok())
        .map(|d| d.items)
        .unwrap_or_default();
    // the photoscanned Poly Haven structures and props, the Skyborn Building Studio's walk-in
    // buildings and village props, and the style packs
    let mut out: Vec<LibraryEntry> = model_libraries(&dir)
        .iter()
        .flat_map(model_library)
        .collect();
    out.extend(food.into_iter().map(|it| {
        let (w, h, d) = (
            it.size_m.first().copied().unwrap_or(0.2),
            it.size_m.get(1).copied().unwrap_or(0.2),
            it.size_m.get(2).copied().unwrap_or(0.2),
        );
        LibraryEntry {
            id: format!("props:{}", it.id),
            name: it.name,
            category: Category::Props,
            group: "food".into(),
            kind: None,
            import_path: Some(format!("models/{}{}", it.path, it.entry)),
            surface: Surface::Wood,
            radius: (w.max(d) * 0.6).max(0.05),
            align: false,
            walkable: false,
            whole_scene: false,
            biomes: Biome::ALL.to_vec(),
            sway: false,
            default_scale: 1.0,
            meshes: vec![],
            scene: None,
            height: h,
            tree: None,
            textured: false,
            pack: PHOTOREAL.into(),
        }
    }));
    out
}

/// The style pack of everything photoscanned or built procedurally (Poly Haven, Skyborn,
/// photoscans, nature, imported models).
pub const PHOTOREAL: &str = "photoreal";

/// A bundled model library: `models/<folder>/models.json`, its entries' id prefix and style pack.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelLibrary {
    pub folder: String,
    pub prefix: String,
    /// its style (the look: `PHOTOREAL`, "handpainted"…)
    pub pack: String,
    /// the marketplace pack its models are sold in (`crate::market`)
    pub pack_id: String,
    /// where its `models.json` is on disk: `<dir>/<folder>`, or a bought pack's install
    /// (`crate::paths::packs_dir()`, read through the same `models/<folder>` asset paths)
    pub dir: std::path::PathBuf,
    /// a shipped preview of a paid style pack (`PREVIEWS`): every model listed with its picture,
    /// but only the free samples' files are there
    pub preview: bool,
}

/// Where the shipped app keeps previews of paid style packs it doesn't include:
/// `models/stylepack_previews/<style>/` holds the pack's whole `models.json`, a picture of every
/// model (`thumbs/<id>.png`, the thumbnail studio's own, `crate::thumbs`) and the files of its
/// free samples and hero only (`tools/release/package_macos.py` makes it). It is listed only
/// while neither the bundle nor a bought install (`crate::market::install`) has that style's
/// library, so buying the pack replaces it whole; the bundle's `models/stylepacks/` paths, which
/// the towns, set pieces and dungeons check for the pack, stay absent.
pub const PREVIEWS: &str = "stylepack_previews";

/// The shipped picture of a preview library's model (`PREVIEWS`), as an assets path.
pub fn preview_thumb(e: &LibraryEntry) -> Option<String> {
    let rest = e.import_path.as_deref()?.strip_prefix("models/")?;
    let rest = rest.strip_prefix(PREVIEWS)?.strip_prefix('/')?;
    let style = rest.split('/').next().filter(|s| !s.is_empty())?;
    let (_, local) = e.id.split_once(':')?;
    Some(format!("models/{PREVIEWS}/{style}/thumbs/{local}.png"))
}

/// The bundled model libraries under `dir` (`assets/models`): Poly Haven (`ph:`) and Skyborn
/// (`sb:`), both photoreal, then every style pack `stylepacks/<pack>/models.json` (ids
/// `<pack>:<id>`; the hand-painted pack is `hp:`), in name order. A manifest may name its
/// `"style"` and marketplace `"pack"` id; without `"style"` its `"pack"` is the style (the first
/// manifests) and the pack id comes from `default_pack_id`. Style packs bought and installed
/// (`crate::market::install`) are listed too when the bundle lacks them.
pub fn model_libraries(dir: &std::path::Path) -> Vec<ModelLibrary> {
    model_libraries_in(dir, &crate::paths::packs_dir().join("models"))
}

pub(crate) fn model_libraries_in(
    dir: &std::path::Path,
    bought: &std::path::Path,
) -> Vec<ModelLibrary> {
    #[derive(Deserialize)]
    struct Head {
        #[serde(default)]
        style: Option<String>,
        #[serde(default)]
        pack: Option<String>,
    }
    let lib = |root: &std::path::Path, folder: &str, prefix: &str, style: &str, preview: bool| {
        let head = std::fs::read_to_string(root.join(folder).join("models.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Head>(&s).ok());
        let (style, pack_id) = match head {
            Some(Head {
                style: Some(s),
                pack,
            }) => (s, pack.unwrap_or_else(|| default_pack_id(folder))),
            _ => (style.to_string(), default_pack_id(folder)),
        };
        ModelLibrary {
            folder: folder.into(),
            prefix: prefix.into(),
            pack: style,
            pack_id,
            dir: root.join(folder),
            preview,
        }
    };
    let mut out = vec![
        lib(dir, "polyhaven", "ph", PHOTOREAL, false),
        lib(dir, "skyborn", "sb", PHOTOREAL, false),
    ];
    let styles_in = |root: &std::path::Path, base: &str| -> Vec<String> {
        std::fs::read_dir(root.join(base))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().join("models.json").is_file())
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    // (style, where, base folder): the bundle's first, then bought ones it doesn't have, then
    // shipped previews of the rest
    let mut packs: Vec<(String, &std::path::Path, &str)> = styles_in(dir, "stylepacks")
        .into_iter()
        .map(|p| (p, dir, "stylepacks"))
        .collect();
    for (root, base) in [(bought, "stylepacks"), (dir, PREVIEWS)] {
        for p in styles_in(root, base) {
            if !packs.iter().any(|(q, ..)| *q == p) {
                packs.push((p, root, base));
            }
        }
    }
    packs.sort();
    for (p, root, base) in packs {
        out.push(lib(
            root,
            &format!("{base}/{p}"),
            pack_prefix(&p),
            &p,
            base == PREVIEWS,
        ));
    }
    out
}

/// The id prefix of a style pack's models.
pub fn pack_prefix(pack: &str) -> &str {
    match pack {
        "handpainted" => "hp",
        p => p,
    }
}

/// The marketplace pack a bundled library's models belong to when its manifest doesn't say.
fn default_pack_id(folder: &str) -> String {
    // (a shipped preview is of the pack its full library is sold in)
    let full = folder
        .strip_prefix(PREVIEWS)
        .map(|style| format!("stylepacks{style}"));
    match full.as_deref().unwrap_or(folder) {
        "polyhaven" => "polyhaven_scans".into(),
        "skyborn" => "skyborn_village".into(),
        "stylepacks/handpainted" => "skyfold_studio".into(),
        f => f.rsplit('/').next().unwrap_or(f).to_string(),
    }
}

/// Styles (the look of a set of assets) and their names for people. Styles are open strings:
/// one missing here reads as its id, capitalised.
pub const STYLES: &[(&str, &str)] = &[
    (PHOTOREAL, "Photoreal"),
    ("handpainted", "Hand-painted"),
    ("celshaded", "Cel-shaded"),
    ("gothic", "Gothic"),
    ("lowpoly", "Low-poly"),
];

/// A style pack's name for people ("Hand-painted").
pub fn pack_label(pack: &str) -> String {
    match STYLES.iter().find(|s| s.0 == pack) {
        Some(s) => s.1.into(),
        None => {
            let mut c = pack.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    }
}

/// A model library, `models/<folder>/models.json` (one schema for all of them; bundled or a
/// bought pack's install): its models become library entries `<prefix>:<id>` of its style pack.
pub fn model_library(lib: &ModelLibrary) -> Vec<LibraryEntry> {
    let (folder, prefix) = (lib.folder.as_str(), lib.prefix.as_str());
    #[derive(Deserialize)]
    struct Model {
        id: String,
        name: String,
        category: String,
        size_m: Vec<f32>,
        gltf: String,
        /// fortifications, ships, containers, lighting, furniture, tools, weapons, decor, village,
        /// camp, food or buildings (the Objects tab's group chips)
        #[serde(default)]
        group: String,
        /// what it sounds like underfoot (default: guessed from its id and group)
        #[serde(default)]
        surface: Option<Surface>,
        /// can be walked on (default: everything but props)
        #[serde(default)]
        walkable: Option<bool>,
        /// a complete scene (default: models whose pieces are instanced, `unique_tris`)
        #[serde(default)]
        scene: Option<bool>,
        #[serde(default)]
        unique_tris: Option<u64>,
    }
    #[derive(Deserialize)]
    struct Models {
        models: Vec<Model>,
    }
    let models = std::fs::read_to_string(lib.dir.join("models.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Models>(&s).ok())
        .map(|d| d.models)
        .unwrap_or_default();
    models
        .into_iter()
        .map(|m| {
            let (w, h, d) = (
                m.size_m.first().copied().unwrap_or(1.0),
                m.size_m.get(1).copied().unwrap_or(1.0),
                m.size_m.get(2).copied().unwrap_or(1.0),
            );
            let surface = m
                .surface
                .unwrap_or_else(|| ph_surface(&m.id, &m.category, &m.group));
            LibraryEntry {
                id: format!("{prefix}:{}", m.id),
                name: m.name,
                category: if m.category == "props" {
                    Category::Props
                } else {
                    Category::Structures
                },
                walkable: m.walkable.unwrap_or(m.category != "props"),
                whole_scene: m.scene.unwrap_or(m.unique_tris.is_some()),
                group: m.group,
                kind: None,
                import_path: Some(format!("models/{folder}/{}", m.gltf)),
                surface,
                radius: (w.max(d) * 0.55).max(0.05),
                align: false,
                biomes: Biome::ALL.to_vec(),
                sway: false,
                default_scale: 1.0,
                meshes: vec![],
                scene: None,
                height: h,
                tree: None,
                textured: false,
                pack: lib.pack.clone(),
            }
        })
        .collect()
}

pub struct LibraryEntry {
    pub id: String,
    pub name: String,
    pub category: Category,
    /// finer grouping within the category ("furniture", "camp"…; empty: none)
    pub group: String,
    pub kind: Option<AssetKind>,
    /// assets-relative path for imported models
    pub import_path: Option<String>,
    pub surface: Surface,
    pub radius: f32,
    pub align: bool,
    pub walkable: bool,
    /// A complete scene in one model (an encounter site with every piece in place): listed under
    /// Objects › Scenes › Ready-made scenes, not among single objects.
    pub whole_scene: bool,
    pub biomes: Vec<Biome>,
    pub sway: bool,
    pub default_scale: f32,
    pub meshes: Vec<(Handle<Mesh>, Handle<Mesh>)>,
    pub scene: Option<Handle<WorldAsset>>,
    pub height: f32,
    /// ez-tree trees: bark/leaf meshes per LOD and the billboard impostor.
    pub tree: Option<crate::trees::TreeParts>,
    /// Parts carry surface slots in their vertex colours: render with `Library::surface`.
    pub textured: bool,
    /// The style pack it belongs to (`PHOTOREAL`, or a style pack's name like "handpainted"):
    /// the Objects tab's Style filter.
    pub pack: String,
}

impl LibraryEntry {
    /// Walk-in buildings (rooms behind open doors, stairs, upper floors) and walkable encounter
    /// sets (an arena with a gate hall to walk through and a grandstand to climb): their colliders
    /// keep the air between floors, walls and ceilings (see `motion::collide`).
    pub fn walk_in(&self) -> bool {
        self.walkable
            && matches!(
                self.group.as_str(),
                "buildings" | "encounters" | "portals" | "entrances" | "monuments"
            )
    }

    /// Ships, boats and pier pieces sit on the water (their y = 0 is the waterline), not on
    /// the sea bed.
    pub fn floats(&self) -> bool {
        self.group == "ships" && self.category != Category::Props
    }

    /// Ships and boats ride the waves (piers and docks float in place but stay put).
    pub fn bobs(&self) -> bool {
        self.floats() && !self.id.contains("pier")
    }

    /// Where it stands at ground point `ground`: on the water surface when it floats and
    /// there's water there, else on the ground.
    pub fn seat(&self, ground: Vec3, water: &crate::water::WaterBodies, sea: f32) -> Vec3 {
        if self.floats() {
            let w = water.surface_at(ground, sea);
            if w > ground.y {
                return Vec3::new(ground.x, w, ground.z);
            }
        }
        ground
    }
}

/// What a Poly Haven model sounds like underfoot, from its group and id.
fn ph_surface(id: &str, category: &str, group: &str) -> Surface {
    let has = |w: &[&str]| w.iter().any(|x| id.to_lowercase().contains(x));
    if has(&[
        "iron", "brass", "cannon", "metal", "crowbar", "vice", "anvil", "pan_", "pot_0",
    ]) {
        Surface::Metal
    } else if has(&["marble", "bust"]) {
        Surface::Marble
    } else if category == "ruins"
        || has(&[
            "statue",
            "stone",
            "lion_head",
            "ceramic",
            "vase",
            "fort_01_tower",
            "fort_01_wall",
        ])
        || (group == "fortifications" && !has(&["door", "gate"]))
    {
        Surface::Stone
    } else {
        Surface::Wood
    }
}

pub type FoliageMaterial = ExtendedMaterial<StandardMaterial, FoliageExt>;

/// The wind shared by every foliage material: (dir.x, dir.z, speed m/s, gustiness), (gust offset
/// x, z, 0, 0). It lives in one small storage buffer rewritten each frame, so the materials
/// themselves never change (a changed material re-specializes every mesh that uses it — with
/// ~100k tree and plant meshes that alone cost several ms a frame).
pub const FOLIAGE_WIND: Handle<ShaderBuffer> = uuid_handle!("6f1c3a52-9d0e-4b7a-8a51-2f4f0f6c9e11");

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct FoliageExt {
    #[storage(100, read_only)]
    pub wind: Handle<ShaderBuffer>,
    /// The shade under the trees (`nature::shade`), shared like the wind.
    #[storage(120, read_only)]
    pub canopy: Handle<ShaderBuffer>,
}

impl Default for FoliageExt {
    fn default() -> Self {
        Self {
            wind: FOLIAGE_WIND,
            canopy: crate::nature::shade::CANOPY_SHADE,
        }
    }
}

impl MaterialExtension for FoliageExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/foliage.wesl".into()
    }
    /// Standard PBR, with the sky light the crowns above hold back.
    fn fragment_shader() -> ShaderRef {
        "shaders/foliage.wesl".into()
    }
    /// The depth prepass and shadows use the same wind (alpha-tested leaves then shade once per
    /// pixel instead of once per overlapping leaf).
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/foliage_prepass.wesl".into()
    }
    fn prepass_fragment_shader() -> ShaderRef {
        "shaders/forest_prepass.wesl".into()
    }
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        crate::nature::shade::shader_def(descriptor);
        equal_depth_main_pass(descriptor, key.mesh_key);
        Ok(())
    }
}

/// Alpha-tested foliage after a depth prepass: the prepass already kept exactly the leaf pixels
/// in front (same vertex wind, same cut-out), so the main pass draws only where its depth equals
/// the prepass's and needs no discard of its own. A shader that may discard turns off the GPU's
/// hidden-surface removal (Apple's tile renderers shade every overlapping leaf); without it each
/// pixel is shaded once. Identical pixels; ~9 % faster in a dense forest (`WB_NOEQUALDEPTH=1`
/// turns it off, for comparisons).
pub fn equal_depth_main_pass(
    descriptor: &mut RenderPipelineDescriptor,
    mesh_key: bevy::pbr::MeshPipelineKey,
) {
    use bevy::pbr::MeshPipelineKey;
    use bevy::render::render_resource::CompareFunction;
    use bevy::shader::ShaderDefVal;
    if std::env::var("WB_NOEQUALDEPTH").is_ok() {
        return;
    }
    let Some(frag) = descriptor.fragment.as_mut() else {
        return;
    };
    let is_prepass = frag
        .shader_defs
        .iter()
        .any(|d| matches!(d, ShaderDefVal::Bool(n, true) if n == "PREPASS_PIPELINE"));
    if is_prepass
        || !mesh_key.contains(MeshPipelineKey::DEPTH_PREPASS)
        || !mesh_key.contains(MeshPipelineKey::MAY_DISCARD)
    {
        return;
    }
    let Some(ds) = descriptor.depth_stencil.as_mut() else {
        return;
    };
    frag.shader_defs
        .retain(|d| !matches!(d, ShaderDefVal::Bool(n, _) if n == "MAY_DISCARD"));
    ds.depth_compare = Some(CompareFunction::Equal);
    ds.depth_write_enabled = Some(false);
}

#[derive(Resource)]
pub struct Library {
    pub entries: Vec<LibraryEntry>,
    pub foliage: Handle<FoliageMaterial>,
    pub solid: Handle<StandardMaterial>,
    /// photoscanned triplanar materials for built objects (surfaces.rs)
    pub surface: Handle<crate::surfaces::SurfaceMaterial>,
    index: HashMap<String, usize>,
}

impl Library {
    pub fn get(&self, id: &str) -> Option<&LibraryEntry> {
        self.index.get(id).map(|&i| &self.entries[i])
    }
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
    pub fn add(&mut self, e: LibraryEntry) -> usize {
        if let Some(&i) = self.index.get(&e.id) {
            self.entries[i] = e;
            return i;
        }
        self.index.insert(e.id.clone(), self.entries.len());
        self.entries.push(e);
        self.entries.len() - 1
    }
    pub fn of_category(&self, c: Category) -> impl Iterator<Item = (usize, &LibraryEntry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(move |(_, e)| e.category == c)
    }
    /// An empty library without materials (unit tests).
    #[cfg(test)]
    pub fn empty_for_tests() -> Self {
        Library {
            entries: vec![],
            foliage: Handle::default(),
            solid: Handle::default(),
            surface: Handle::default(),
            index: HashMap::new(),
        }
    }
}

pub fn builtin_id(k: AssetKind) -> String {
    format!("{k:?}")
}

fn to_mesh(m: MeshData) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, m.colors);
    mesh.insert_indices(Indices::U32(m.indices));
    mesh
}

/// Number of random variants generated per builtin kind.
fn kind_variants(k: AssetKind) -> u32 {
    match kind_category(k) {
        Category::Structures => 1,
        _ => 3,
    }
}

pub mod glow;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Placed {
    pub id: u64,
    pub asset: String,
    pub variant: u32,
    pub pos: [f32; 3],
    pub rot: [f32; 4],
    /// Uniform size (× the model); with `stretch`, the largest axis.
    pub scale: f32,
    /// Non-uniform size: per-axis factors (x, y, z in the model's frame) on top of `scale`;
    /// `None` for uniform objects (nearly all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch: Option<[f32; 3]>,
    /// A set piece's pieces share a group id (0: on its own): clicking one selects the set, and
    /// the handles move, turn and resize the set as one (`placement::group`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub group: u64,
    /// Locked (right-click › Lock): it can't be moved, turned, resized or deleted until it's
    /// unlocked; it still selects, copies and rides terrain edits (`placement::lock`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// A glowing object's own colour and light (crystals: `glow`); `None`: as made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glow: Option<glow::Glow>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl Placed {
    /// Size along the model's x, y and z axes.
    pub fn scale3(&self) -> Vec3 {
        Vec3::from_array(self.stretch.unwrap_or([1.0; 3])) * self.scale
    }
    /// Sets the size: a number (uniform) or per-axis `[x, y, z]` (stored as `scale` = the largest
    /// axis and the rest as `stretch`).
    pub fn set_scale3(&mut self, s: Vec3) {
        let m = s.max_element().max(1e-4);
        self.scale = m;
        let k = s / m;
        self.stretch = ((k - Vec3::ONE).abs().max_element() > 1e-4).then_some(k.to_array());
    }
    pub fn translation(&self) -> Vec3 {
        Vec3::from_array(self.pos)
    }
    pub fn rotation(&self) -> Quat {
        Quat::from_array(self.rot)
    }
    pub fn yaw(&self) -> f32 {
        self.rotation().to_euler(EulerRot::YXZ).0
    }
}

pub const GRID: f32 = 16.0;
/// Billboard chunk size (m): distant trees are merged per chunk.
pub const CHUNK: f32 = 128.0;

#[derive(Resource, Default)]
pub struct ObjectStore {
    pub items: HashMap<u64, Placed>,
    next_id: u64,
    grid: HashMap<(i32, i32), Vec<u64>>,
    /// render entities of the objects currently streamed in
    pub entities: HashMap<u64, Entity>,
    pub to_spawn: Vec<u64>,
    pub to_despawn: Vec<Entity>,
    /// billboard chunks whose objects changed
    pub changed_chunks: std::collections::HashSet<(i32, i32)>,
    pub selected: Option<u64>,
    pub revision: u64,
    /// render entities moved in place (`move_to`), applied by `apply_moves`
    pub to_move: Vec<(Entity, Transform)>,
}

fn cell_of(p: Vec3) -> (i32, i32) {
    ((p.x / GRID).floor() as i32, (p.z / GRID).floor() as i32)
}

pub fn chunk_of(x: f32, z: f32) -> (i32, i32) {
    ((x / CHUNK).floor() as i32, (z / CHUNK).floor() as i32)
}

impl ObjectStore {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    pub fn insert(&mut self, p: Placed) {
        // group ids come from the same counter (`placement::group`)
        self.next_id = self.next_id.max(p.id).max(p.group);
        let id = p.id;
        self.grid
            .entry(cell_of(p.translation()))
            .or_default()
            .push(id);
        self.changed_chunks.insert(chunk_of(p.pos[0], p.pos[2]));
        self.items.insert(id, p);
        self.to_spawn.push(id);
        self.revision += 1;
    }
    pub fn remove(&mut self, id: u64) -> Option<Placed> {
        let p = self.items.remove(&id)?;
        if let Some(v) = self.grid.get_mut(&cell_of(p.translation())) {
            v.retain(|x| *x != id);
        }
        self.changed_chunks.insert(chunk_of(p.pos[0], p.pos[2]));
        if let Some(e) = self.entities.remove(&id) {
            self.to_despawn.push(e);
        }
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.revision += 1;
        Some(p)
    }
    /// Moves an object in place (set pieces dragged as a group): its render entity lives on and
    /// only takes the new transform (`apply_moves`), where `update` would respawn it.
    pub fn move_to(&mut self, p: Placed) {
        let Some(old) = self.items.get(&p.id) else {
            return;
        };
        let (oc, nc) = (cell_of(old.translation()), cell_of(p.translation()));
        if oc != nc {
            if let Some(v) = self.grid.get_mut(&oc) {
                v.retain(|x| *x != p.id);
            }
            self.grid.entry(nc).or_default().push(p.id);
        }
        self.changed_chunks.insert(chunk_of(old.pos[0], old.pos[2]));
        self.changed_chunks.insert(chunk_of(p.pos[0], p.pos[2]));
        if let Some(&e) = self.entities.get(&p.id) {
            self.to_move.push((
                e,
                Transform {
                    translation: p.translation(),
                    rotation: p.rotation(),
                    scale: p.scale3(),
                },
            ));
        }
        self.items.insert(p.id, p);
        self.revision += 1;
    }
    /// The objects of group `g` (a set piece), by id.
    pub fn group_members(&self, g: u64) -> Vec<u64> {
        if g == 0 {
            return vec![];
        }
        let mut v: Vec<u64> = self
            .items
            .values()
            .filter(|p| p.group == g)
            .map(|p| p.id)
            .collect();
        v.sort_unstable();
        v
    }
    /// Locks or unlocks objects in place (no respawn); record them in the open undo step first.
    pub fn set_locked(&mut self, ids: &[u64], locked: bool) {
        for id in ids {
            if let Some(p) = self.items.get_mut(id)
                && p.locked != locked
            {
                p.locked = locked;
                self.revision += 1;
            }
        }
    }
    /// Is this object locked (or, for a set piece, any of its set)?
    pub fn is_locked(&self, id: u64) -> bool {
        self.items.get(&id).is_some_and(|p| {
            p.locked
                || (p.group != 0 && self.items.values().any(|q| q.group == p.group && q.locked))
        })
    }
    /// Replaces an object (it stays selected if it was).
    pub fn update(&mut self, p: Placed) {
        let selected = self.selected;
        self.remove(p.id);
        self.insert(p);
        self.selected = selected;
    }
    pub fn clear(&mut self) {
        let ids: Vec<u64> = self.items.keys().copied().collect();
        for id in ids {
            self.remove(id);
        }
        self.next_id = 0;
    }
    pub fn query(&self, c: Vec2, r: f32) -> Vec<u64> {
        let mut out = vec![];
        let x0 = ((c.x - r) / GRID).floor() as i32;
        let x1 = ((c.x + r) / GRID).floor() as i32;
        let z0 = ((c.y - r) / GRID).floor() as i32;
        let z1 = ((c.y + r) / GRID).floor() as i32;
        for z in z0..=z1 {
            for x in x0..=x1 {
                if let Some(v) = self.grid.get(&(x, z)) {
                    for id in v {
                        if let Some(p) = self.items.get(id)
                            && Vec2::new(p.pos[0], p.pos[2]).distance(c) <= r
                        {
                            out.push(*id);
                        }
                    }
                }
            }
        }
        out
    }
    pub fn count(&self) -> usize {
        self.items.len()
    }
    /// The objects in one grid cell (GRID m).
    pub fn cell(&self, c: (i32, i32)) -> Option<&Vec<u64>> {
        self.grid.get(&c)
    }
    /// Non-empty grid cells (GRID m) and their objects.
    pub fn cells(&self) -> impl Iterator<Item = (&(i32, i32), &Vec<u64>)> {
        self.grid.iter()
    }
    /// Objects inside a billboard chunk.
    pub fn in_chunk(&self, c: (i32, i32)) -> Vec<u64> {
        let per = (CHUNK / GRID) as i32;
        let mut out = vec![];
        for z in c.1 * per..(c.1 + 1) * per {
            for x in c.0 * per..(c.0 + 1) * per {
                if let Some(v) = self.grid.get(&(x, z)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out
    }
}

#[derive(Component)]
pub struct PlacedObject {
    #[allow(dead_code)]
    pub id: u64,
}

/// On walkable objects' render entities (bridges, docks, structures), for tools that ray-cast
/// them; walking itself stands on `motion::collide`'s colliders.
#[derive(Component)]
pub struct Walkable(#[allow(dead_code)] pub Surface);

/// One layer of a scatter preset.
pub struct PresetLayer {
    pub kind: AssetKind,
    /// instances per 1000 m²
    pub density: f32,
    pub scale: (f32, f32),
    pub max_slope: f32,
    /// altitude above sea (m)
    pub alt: (f32, f32),
}

pub struct Preset {
    pub name: &'static str,
    pub biome: Biome,
    pub layers: Vec<PresetLayer>,
    /// An alien-world preset (Alien Spires, Floating Isles): only offered in sci-fi worlds, never
    /// grown by a biome. Appended after the fantasy presets, so their indices never move.
    pub scifi: bool,
}

fn pl(
    kind: AssetKind,
    density: f32,
    scale: (f32, f32),
    max_slope: f32,
    alt: (f32, f32),
) -> PresetLayer {
    PresetLayer {
        kind,
        density,
        scale,
        max_slope,
        alt,
    }
}

pub fn presets() -> Vec<Preset> {
    use AssetKind::*;
    let any = (1.5, 2000.0);
    // Grass provides the base cover; these layers grow recognizable ecological plant patches.
    vec![
        Preset {
            name: "Mixed Forest",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(Blackberry, 5.0, (0.8, 1.2), 28.0, any),
                pl(Blueberry, 8.0, (0.7, 1.2), 28.0, any),
                pl(WildStrawberry, 12.0, (0.8, 1.3), 28.0, any),
                pl(Oak, 2.5, (0.8, 1.3), 30.0, any),
                pl(Birch, 1.8, (0.8, 1.2), 30.0, any),
                pl(Pine, 1.6, (0.85, 1.25), 34.0, any),
                pl(Bush, 6.0, (0.7, 1.3), 35.0, any),
                pl(Fern, 10.0, (0.7, 1.3), 35.0, any),
                pl(TallGrass, 4.0, (0.7, 1.2), 30.0, any),
                pl(Flowers, 22.0, (0.6, 1.0), 25.0, any),
                pl(FallenLog, 0.6, (0.8, 1.2), 25.0, any),
                pl(Stump, 0.5, (0.8, 1.2), 25.0, any),
                pl(Branches, 0.8, (0.8, 1.2), 30.0, any),
                pl(MossyRock, 0.7, (0.6, 1.4), 40.0, any),
                pl(Mushrooms, 14.0, (0.8, 1.3), 25.0, any),
                pl(Roots, 0.15, (0.8, 1.2), 25.0, any),
            ],
        },
        Preset {
            name: "Pine Forest",
            biome: Biome::Boreal,
            scifi: false,
            layers: vec![
                pl(Blueberry, 14.0, (0.7, 1.2), 28.0, any),
                // A conifer understorey is blueberry-led, with scattered strawberries and
                // wildflowers in brighter gaps. Brambles stay a lower-elevation accent.
                pl(WildStrawberry, 7.0, (0.7, 1.1), 25.0, (1.5, 180.0)),
                pl(Blackberry, 2.0, (0.7, 1.1), 25.0, (1.5, 120.0)),
                pl(Flowers, 28.0, (0.85, 1.25), 32.0, any),
                pl(Pine, 3.0, (0.85, 1.35), 36.0, any),
                pl(Spruce, 2.5, (0.8, 1.3), 36.0, any),
                pl(Fir, 2.0, (0.8, 1.3), 36.0, any),
                pl(Fern, 8.0, (0.7, 1.2), 35.0, any),
                pl(Branches, 1.2, (0.8, 1.2), 30.0, any),
                pl(FallenLog, 0.5, (0.8, 1.2), 25.0, any),
                pl(Boulder, 0.5, (0.5, 1.3), 45.0, any),
                pl(MossyRock, 0.6, (0.6, 1.3), 40.0, any),
                pl(Stump, 0.4, (0.8, 1.2), 25.0, any),
                pl(Mushrooms, 20.0, (0.7, 1.1), 25.0, any),
                pl(Roots, 0.15, (0.8, 1.2), 25.0, any),
            ],
        },
        Preset {
            name: "Autumn Woods",
            biome: Biome::Autumn,
            scifi: false,
            layers: vec![
                pl(Blackberry, 8.0, (0.8, 1.2), 28.0, any),
                pl(WildStrawberry, 10.0, (0.8, 1.2), 28.0, any),
                pl(Flowers, 18.0, (0.7, 1.2), 25.0, any),
                pl(Maple, 3.5, (0.8, 1.3), 30.0, any),
                pl(Oak, 1.5, (0.8, 1.2), 30.0, any),
                pl(Birch, 1.2, (0.8, 1.2), 30.0, any),
                pl(Bush, 6.0, (0.7, 1.2), 35.0, any),
                pl(Mushrooms, 18.0, (0.8, 1.3), 25.0, any),
                pl(FallenLog, 0.6, (0.8, 1.2), 25.0, any),
                pl(Stump, 0.4, (0.8, 1.2), 25.0, any),
                pl(Branches, 1.0, (0.8, 1.2), 30.0, any),
                pl(MossyRock, 0.3, (0.6, 1.2), 40.0, any),
                pl(TallGrass, 3.0, (0.7, 1.1), 30.0, any),
            ],
        },
        Preset {
            name: "Sparse Woodland",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(Blackberry, 6.0, (0.7, 1.2), 28.0, any),
                pl(WildStrawberry, 14.0, (0.8, 1.3), 28.0, any),
                pl(Lavender, 12.0, (0.8, 1.3), 25.0, any),
                pl(Oak, 0.8, (0.9, 1.4), 28.0, any),
                pl(Birch, 0.6, (0.8, 1.2), 28.0, any),
                pl(Bush, 3.0, (0.7, 1.3), 35.0, any),
                pl(Flowers, 35.0, (0.7, 1.2), 25.0, any),
                pl(TallGrass, 5.0, (0.7, 1.3), 30.0, any),
                pl(Boulder, 0.3, (0.5, 1.2), 40.0, any),
                pl(Stump, 0.2, (0.8, 1.2), 25.0, any),
                pl(FallenLog, 0.15, (0.8, 1.2), 25.0, any),
                pl(Mushrooms, 7.0, (0.7, 1.1), 25.0, any),
            ],
        },
        Preset {
            name: "Flower Meadow",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(Mushrooms, 4.0, (0.7, 1.2), 25.0, any),
                pl(WildStrawberry, 16.0, (0.8, 1.3), 28.0, any),
                pl(Lavender, 22.0, (0.8, 1.3), 25.0, any),
                pl(Flowers, 68.0, (0.7, 1.3), 25.0, any),
                pl(TallGrass, 8.0, (0.7, 1.3), 28.0, any),
                pl(FlowerBush, 2.5, (0.7, 1.2), 28.0, any),
                pl(Bush, 1.0, (0.7, 1.2), 30.0, any),
                pl(Boulder, 0.15, (0.5, 1.1), 40.0, any),
                pl(Pebbles, 0.5, (0.7, 1.2), 35.0, any),
            ],
        },
        Preset {
            name: "Alpine",
            biome: Biome::Alpine,
            scifi: false,
            layers: vec![
                pl(Blueberry, 7.0, (0.6, 1.0), 25.0, (1.5, 300.0)),
                pl(SnowyPine, 2.0, (0.7, 1.2), 40.0, (120.0, 2000.0)),
                pl(Spruce, 2.0, (0.7, 1.2), 38.0, (1.5, 400.0)),
                pl(Boulder, 1.2, (0.6, 1.6), 50.0, any),
                pl(RockCluster, 0.7, (0.6, 1.3), 50.0, any),
                pl(Pebbles, 2.0, (0.8, 1.4), 45.0, any),
                pl(MossyRock, 0.4, (0.6, 1.2), 45.0, (1.5, 300.0)),
                pl(Flowers, 15.0, (0.6, 1.0), 25.0, (1.5, 300.0)),
            ],
        },
        Preset {
            name: "Jungle",
            biome: Biome::Tropical,
            scifi: false,
            layers: vec![
                pl(Palm, 2.5, (0.8, 1.3), 32.0, any),
                pl(Bamboo, 2.0, (0.8, 1.3), 30.0, any),
                pl(Oak, 1.0, (1.0, 1.5), 30.0, any),
                pl(Fern, 18.0, (0.9, 1.6), 38.0, any),
                pl(FlowerBush, 5.0, (0.8, 1.4), 35.0, any),
                pl(Bush, 8.0, (0.9, 1.5), 35.0, any),
                pl(MossyRock, 0.8, (0.6, 1.3), 40.0, any),
                pl(FallenLog, 0.6, (0.8, 1.3), 25.0, any),
                pl(Mushrooms, 18.0, (0.8, 1.3), 25.0, any),
                pl(Stump, 0.3, (0.8, 1.2), 25.0, any),
            ],
        },
        Preset {
            name: "Palm Beach",
            biome: Biome::Tropical,
            scifi: false,
            layers: vec![
                pl(Palm, 1.4, (0.8, 1.25), 25.0, (0.5, 40.0)),
                pl(Driftwood, 0.4, (0.8, 1.2), 20.0, (0.2, 6.0)),
                pl(Pebbles, 1.0, (0.6, 1.2), 30.0, (0.2, 20.0)),
                pl(Bush, 1.0, (0.7, 1.1), 28.0, (3.0, 60.0)),
                pl(Boulder, 0.2, (0.5, 1.2), 40.0, (0.2, 40.0)),
            ],
        },
        Preset {
            name: "Desert",
            biome: Biome::Desert,
            scifi: false,
            layers: vec![
                // (no procedural saguaros until a photoscanned cactus exists: the scanned
                // scrub carries the desert)
                pl(DesertShrub, 5.0, (0.6, 1.3), 30.0, any),
                pl(RockSpire, 0.08, (0.6, 1.3), 40.0, any),
                pl(Hoodoo, 0.1, (0.7, 1.3), 30.0, any),
                pl(Boulder, 0.6, (0.5, 1.4), 45.0, any),
                pl(RockCluster, 0.3, (0.5, 1.2), 45.0, any),
                pl(DeadTree, 0.2, (0.7, 1.1), 25.0, any),
                pl(Driftwood, 0.15, (0.6, 1.0), 25.0, any),
                pl(Pebbles, 1.2, (0.7, 1.2), 30.0, any),
            ],
        },
        Preset {
            name: "Swamp",
            biome: Biome::Swamp,
            scifi: false,
            layers: vec![
                pl(Willow, 1.6, (0.8, 1.3), 25.0, (0.0, 80.0)),
                pl(DeadTree, 0.8, (0.8, 1.2), 25.0, any),
                pl(Reeds, 22.0, (0.7, 1.3), 20.0, (-0.5, 15.0)),
                pl(TallGrass, 6.0, (0.7, 1.2), 25.0, any),
                pl(Mushrooms, 24.0, (0.8, 1.2), 25.0, any),
                pl(FallenLog, 0.8, (0.8, 1.2), 20.0, any),
                pl(Stump, 0.5, (0.8, 1.2), 20.0, any),
                pl(MossyRock, 0.4, (0.6, 1.2), 30.0, any),
                pl(Roots, 0.2, (0.8, 1.2), 20.0, any),
            ],
        },
        Preset {
            name: "Rocky Hills",
            biome: Biome::Alpine,
            scifi: false,
            layers: vec![
                pl(Boulder, 2.0, (0.5, 1.8), 55.0, any),
                pl(RockCluster, 1.2, (0.6, 1.4), 55.0, any),
                pl(CliffRock, 0.25, (0.6, 1.3), 60.0, any),
                pl(Pebbles, 3.0, (0.7, 1.4), 50.0, any),
                pl(Bush, 1.5, (0.6, 1.1), 35.0, any),
            ],
        },
        Preset {
            name: "Tundra",
            biome: Biome::Tundra,
            scifi: false,
            layers: vec![
                pl(SnowyPine, 0.7, (0.6, 1.1), 35.0, any),
                pl(Boulder, 0.8, (0.5, 1.5), 50.0, any),
                pl(RockCluster, 0.4, (0.5, 1.2), 50.0, any),
                pl(Pebbles, 1.5, (0.7, 1.3), 45.0, any),
                pl(DeadTree, 0.15, (0.6, 1.0), 30.0, any),
                pl(Branches, 0.3, (0.7, 1.1), 30.0, any),
            ],
        },
        Preset {
            name: "Haunted Woods",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(DeadTree, 2.2, (0.8, 1.4), 32.0, any),
                pl(Pine, 1.2, (0.9, 1.4), 34.0, any),
                pl(Willow, 0.4, (0.8, 1.2), 25.0, any),
                pl(Mushrooms, 3.0, (0.8, 1.4), 25.0, any),
                pl(Branches, 1.6, (0.8, 1.2), 30.0, any),
                pl(Stump, 0.8, (0.8, 1.2), 25.0, any),
                pl(FallenLog, 0.7, (0.8, 1.2), 25.0, any),
                pl(MossyRock, 0.5, (0.6, 1.3), 40.0, any),
                pl(Fern, 3.0, (0.6, 1.0), 35.0, any),
                pl(Crystals, 0.12, (0.6, 1.4), 35.0, any),
            ],
        },
        Preset {
            name: "Crystal Grove",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(Crystals, 1.4, (0.5, 1.8), 45.0, any),
                pl(MossyRock, 1.0, (0.6, 1.4), 45.0, any),
                pl(RockSpire, 0.12, (0.5, 1.0), 50.0, any),
                pl(Fern, 4.0, (0.6, 1.1), 35.0, any),
                pl(DeadTree, 0.3, (0.7, 1.1), 30.0, any),
                pl(Pebbles, 1.5, (0.7, 1.3), 40.0, any),
            ],
        },
        Preset {
            name: "Volcanic",
            biome: Biome::Volcanic,
            scifi: false,
            layers: vec![
                // a strewn, rocky waste: the densest rock field of the ecosystems
                pl(CliffRock, 0.7, (0.6, 1.4), 65.0, any),
                pl(RockSpire, 0.3, (0.6, 1.4), 55.0, any),
                pl(Boulder, 3.0, (0.5, 1.6), 60.0, any),
                pl(RockCluster, 2.2, (0.6, 1.3), 60.0, any),
                pl(DeadTree, 0.25, (0.6, 1.0), 30.0, any),
                pl(Pebbles, 5.0, (0.7, 1.3), 55.0, any),
            ],
        },
        Preset {
            name: "Sakura Grove",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(SakuraPink, 2.5, (0.8, 1.2), 28.0, any),
                pl(SakuraWhite, 1.7, (0.8, 1.2), 28.0, any),
                pl(Ginkgo, 0.7, (0.8, 1.2), 28.0, any),
                pl(Flowers, 5.0, (0.7, 1.2), 25.0, any),
                pl(Fern, 5.0, (0.7, 1.2), 30.0, any),
            ],
        },
        Preset {
            name: "Bamboo Forest",
            biome: Biome::Tropical,
            scifi: false,
            layers: vec![
                pl(Bamboo, 4.0, (0.8, 1.25), 28.0, any),
                pl(Palm, 0.5, (0.8, 1.15), 28.0, any),
                pl(Fern, 14.0, (0.8, 1.4), 32.0, any),
                pl(MossyRock, 0.5, (0.7, 1.2), 38.0, any),
            ],
        },
        Preset {
            name: "Sequoia Grove",
            biome: Biome::Temperate,
            scifi: false,
            layers: vec![
                pl(Sequoia, 0.55, (0.75, 1.05), 28.0, any),
                pl(Cedar, 1.8, (0.8, 1.2), 30.0, any),
                pl(Fern, 10.0, (0.8, 1.35), 32.0, any),
                pl(FallenLog, 0.5, (0.8, 1.2), 28.0, any),
                pl(MossyRock, 0.8, (0.7, 1.3), 40.0, any),
            ],
        },
        // alien worlds (`scifi`): towering spires, monoliths and glowing crystals on the ground,
        // rocks hanging in the air over it (`visuals::floating`)
        Preset {
            name: "Alien Spires",
            biome: Biome::Desert,
            scifi: true,
            layers: vec![
                pl(RockSpire, 0.08, (1.5, 4.0), 55.0, any),
                pl(Monolith, 0.12, (0.6, 1.6), 30.0, any),
                pl(Crystals, 0.5, (0.8, 2.2), 45.0, any),
                pl(Boulder, 1.2, (0.6, 1.8), 50.0, any),
                pl(Pebbles, 2.0, (0.7, 1.3), 45.0, any),
            ],
        },
        Preset {
            name: "Floating Isles",
            biome: Biome::Desert,
            scifi: true,
            layers: vec![
                pl(FloatingRock, 0.2, (0.15, 1.6), 90.0, any),
                pl(Monolith, 0.05, (0.6, 1.4), 30.0, any),
                pl(Boulder, 0.8, (0.6, 1.5), 50.0, any),
            ],
        },
        // alien flora (photoscans, crimson with alien flora on): quiver trees standing apart over
        // knobbly succulent clumps and dry scrub on the red ground
        Preset {
            name: "Alien Scrub",
            biome: Biome::Desert,
            scifi: true,
            layers: vec![
                pl(QuiverTree, 0.35, (0.7, 1.35), 28.0, any),
                pl(Succulent, 6.0, (0.7, 1.4), 32.0, any),
                pl(DesertShrub, 1.5, (0.6, 1.2), 30.0, any),
                pl(Pebbles, 1.0, (0.7, 1.3), 40.0, any),
            ],
        },
    ]
}

/// Nature brush settings.
#[derive(Resource)]
pub struct ScatterSettings {
    pub category: Category,
    pub selected: Vec<String>,
    pub preset: Option<usize>,
    /// how strongly a dab paints
    pub density: f32,
}

impl Default for ScatterSettings {
    fn default() -> Self {
        Self {
            category: Category::Trees,
            selected: vec![builtin_id(AssetKind::Pine)],
            preset: Some(0),
            density: 0.7,
        }
    }
}

/// Objects tab settings.
#[derive(Resource)]
pub struct PlaceSettings {
    pub category: Category,
    /// group filter within the category (empty: all)
    pub group: String,
    /// style pack filter (empty: every style): the world's art style unless `mix_styles`
    /// (`art_style::sync_place_style`)
    pub style: String,
    /// "Show other styles": the Style switch is out and `style` is the person's pick (kept for the
    /// session; another world style resets it)
    pub mix_styles: bool,
    /// the world style `style` was last set to
    pub world_style: String,
    pub selected: Option<String>,
    pub scale: f32,
    pub random_yaw: bool,
    pub align: bool,
}

impl Default for PlaceSettings {
    fn default() -> Self {
        Self {
            category: Category::Structures,
            group: String::new(),
            style: String::new(),
            mix_styles: false,
            world_style: String::new(),
            selected: Some(builtin_id(AssetKind::WoodenBridge)),
            scale: 1.0,
            random_yaw: false,
            align: false,
        }
    }
}

/// The world's wind (saved with the world). Zones can override the speed; see `wind::WindState`
/// for the wind actually blowing where the viewer is.
#[derive(Resource, Clone)]
pub struct Wind {
    pub dir: Vec2,
    /// m/s; generated worlds start with a light breeze
    pub speed: f32,
    /// 0 steady … 1 very gusty
    pub gustiness: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            dir: Vec2::new(0.8, 0.6),
            speed: 2.5,
            gustiness: 0.5,
        }
    }
}

pub struct ObjectsPlugin;

impl Plugin for ObjectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<FoliageMaterial>::default(),
            MaterialPlugin::<crate::surfaces::SurfaceMaterial>::default(),
        ))
        .init_resource::<ObjectStore>()
        .init_resource::<ScatterSettings>()
        .init_resource::<PlaceSettings>()
        .init_resource::<Wind>()
        .init_resource::<GroundSnapshot>()
        .init_resource::<NatureAmounts>()
        .add_systems(PreStartup, (init_wind_buffer, build_library).chain())
        .add_systems(
            Update,
            (place_tool, select_tool).in_set(crate::tools::ToolSystems::Apply),
        )
        .add_systems(
            PostUpdate,
            apply_moves.before(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            (
                (follow_ground, sync_entities.in_set(ObjectSync))
                    .chain()
                    .before(bevy::transform::TransformSystems::Propagate)
                    .before(bevy::camera::visibility::VisibilitySystems::CalculateBounds),
                update_wind,
            ),
        );
    }
}

fn build_library(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut foliage: ResMut<Assets<FoliageMaterial>>,
    mut solids: ResMut<Assets<StandardMaterial>>,
    mut billboards: ResMut<Assets<crate::streaming::BillboardMaterial>>,
    mut surface_mats: ResMut<Assets<crate::surfaces::SurfaceMaterial>>,
    mut images: ResMut<Assets<Image>>,
    formats: Option<Res<bevy::image::CompressedImageFormatSupport>>,
) {
    let t0 = std::time::Instant::now();
    let (surfaces, surface_library) = crate::surfaces::load(&mut images, &mut surface_mats);
    let (mut trees, atlas) =
        crate::trees::build(&mut meshes, &mut foliage, &mut images, &mut billboards);
    commands.insert_resource(atlas);
    // photoscanned rocks, debris and small plants (crate::photoscan), as tree-style LOD entries
    crate::photoscan::build(
        &mut meshes,
        &mut foliage,
        &mut images,
        &mut trees,
        formats.as_deref(),
    );
    let flora_styles = crate::flora::build_styles(
        &mut meshes,
        &mut foliage,
        &mut images,
        &mut trees,
        crate::art_style::current(),
    );
    commands.insert_resource(flora_styles);
    let glow_mats =
        crate::visuals::formations::build_crystals(&mut meshes, &mut foliage, &mut images, &mut trees);
    commands.insert_resource(glow_mats);
    let foliage = foliage.add(FoliageMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.82,
            reflectance: 0.25,
            diffuse_transmission: 0.25,
            ..default()
        },
        extension: FoliageExt::default(),
    });
    let solid = solids.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.85,
        reflectance: 0.3,
        ..default()
    });
    let mut lib = Library {
        entries: vec![],
        foliage,
        solid,
        surface: surfaces.material.clone(),
        index: HashMap::new(),
    };
    // build meshes in parallel: they're pure CPU work
    let jobs: Vec<(AssetKind, u32)> = AssetKind::ALL
        .iter()
        .filter(|k| !trees.contains_key(k) && !kind_hidden(**k))
        .flat_map(|&k| (0..kind_variants(k)).map(move |v| (k, v)))
        .collect();
    let built: Vec<(AssetKind, u32, MeshData, MeshData)> = std::thread::scope(|s| {
        let chunks: Vec<_> = jobs
            .chunks(jobs.len().div_ceil(8))
            .map(|c| {
                s.spawn(move || {
                    c.iter()
                        .map(|&(k, v)| (k, v, build_asset(k, v, 0), build_asset(k, v, 1)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        chunks.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    for &k in AssetKind::ALL {
        if kind_hidden(k) {
            continue;
        }
        let tree = trees.remove(&k);
        let variants: Vec<_> = built.iter().filter(|b| b.0 == k).collect();
        // built objects: wood, iron, stone and canvas parts take photoscanned surfaces
        let mut textured = false;
        let mut surfaced = |md: &MeshData| {
            let mut md = clone_mesh(md);
            if kind_category(k) == Category::Structures {
                for c in md.colors.iter_mut() {
                    let rgb = [c[0], c[1], c[2]];
                    if let Some(slot) = crate::surfaces::classify(k, rgb)
                        .filter(|s| surfaces.mean[s.index() as usize].is_some())
                    {
                        *c = surfaces.encode(slot, rgb);
                        textured = true;
                    } else {
                        c[3] = 1.0;
                    }
                }
            }
            md
        };
        let (height, ms) = match &tree {
            Some(t) => (
                t.variants[0].height,
                t.variants
                    .iter()
                    .map(|v| {
                        (
                            v.lods[0].0.clone(),
                            v.lods[v.lods.len().min(2) - 1].0.clone(),
                        )
                    })
                    .collect(),
            ),
            None => (
                variants.first().map(|b| b.2.bounds().1[1]).unwrap_or(1.0),
                variants
                    .iter()
                    .map(|b| {
                        (
                            meshes.add(to_mesh(surfaced(&b.2))),
                            meshes.add(to_mesh(surfaced(&b.3))),
                        )
                    })
                    .collect(),
            ),
        };
        let cat = kind_category(k);
        lib.add(LibraryEntry {
            id: builtin_id(k),
            name: k.name().to_string(),
            category: cat,
            group: kind_group(k).into(),
            kind: Some(k),
            import_path: None,
            surface: kind_surface(k),
            radius: kind_radius(k),
            align: kind_align(k),
            walkable: kind_walkable(k),
            whole_scene: false,
            biomes: kind_biomes(k),
            sway: matches!(cat, Category::Trees | Category::Plants),
            default_scale: 1.0,
            meshes: ms,
            scene: None,
            height,
            tree,
            textured,
            pack: PHOTOREAL.into(),
        });
    }
    for e in prop_entries() {
        lib.add(e);
    }
    info!(
        "built {} library assets in {:?}",
        lib.entries.len(),
        t0.elapsed()
    );
    commands.insert_resource(lib);
    commands.insert_resource(surfaces);
    commands.insert_resource(surface_library);
}

fn clone_mesh(m: &MeshData) -> MeshData {
    MeshData {
        positions: m.positions.clone(),
        normals: m.normals.clone(),
        colors: m.colors.clone(),
        indices: m.indices.clone(),
    }
}

/// The shared wind buffer must exist before any foliage material is prepared.
fn init_wind_buffer(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let _ = buffers.insert(
        &FOLIAGE_WIND,
        ShaderBuffer::new(
            vec![0.0f32; 8],
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        ),
    );
}

/// Every foliage material (procedural plants, ez-tree bark and leaves) follows the wind.
fn update_wind(
    wind: Res<crate::wind::WindState>,
    motion: Res<crate::wind::Motion>,
    appearance: Res<crate::nature::NatureAppearance>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    mut last: Local<Option<[f32; 8]>>,
) {
    // still while editing (speed 0: no sway at all)
    let speed = if motion.live { wind.speed } else { 0.0 };
    // (the last two: the alien flora's colour, `nature::alien`; 0 = off)
    let alien = appearance.alien.pack();
    let data = [
        wind.dir.x,
        wind.dir.y,
        speed,
        wind.gustiness,
        wind.gust_offset.x,
        wind.gust_offset.y,
        alien.x,
        alien.y,
    ];
    if *last == Some(data) {
        return;
    }
    *last = Some(data);
    let data = data.to_vec();
    let buf = ShaderBuffer::new(
        data,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let _ = buffers.insert(&FOLIAGE_WIND, buf);
}

/// Where render entities are spawned (streaming runs before it).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectSync;

/// Spawns / despawns render entities to match the store: changed objects are (re)spawned if
/// they're within their draw distance of the viewer (streaming.rs streams the rest in and out as
/// the viewer moves). Spawning is spread over frames so a new world never stalls.
fn sync_entities(
    mut commands: Commands,
    mut store: ResMut<ObjectStore>,
    lib: Option<Res<Library>>,
    asset_server: Res<AssetServer>,
    graphics: Res<crate::streaming::Graphics>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    rocks: Option<Res<crate::visuals::rock_material::RockMaterials>>,
    mut foliage: ResMut<Assets<FoliageMaterial>>,
    mut glow_mats: Option<ResMut<glow::GlowMaterials>>,
) {
    let Some(lib) = lib else { return };
    for e in std::mem::take(&mut store.to_despawn) {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.despawn();
        }
    }
    let cam = cam
        .single()
        .map(|c| c.translation())
        .unwrap_or(Vec3::splat(f32::MAX));
    let ids = std::mem::take(&mut store.to_spawn);
    let mut budget = 3000;
    let mut later = vec![];
    for id in ids {
        if budget == 0 {
            later.push(id);
            continue;
        }
        if let Some(old) = store.entities.remove(&id) {
            commands.entity(old).despawn();
        }
        let Some(p) = store.items.get(&id) else {
            continue;
        };
        let Some(entry) = lib.get(&p.asset) else {
            continue;
        };
        if p.translation().distance(cam)
            > stream_dist(entry, &graphics, size_band(entry, p.scale))
                + crate::streaming::SPAWN_MARGIN
        {
            continue;
        }
        // a crystal given its own colour and light: its material
        let glow = match (&p.glow, glow_mats.as_deref_mut()) {
            (Some(g), Some(m)) if glow::glows(entry) => Some(m.material(g, &mut foliage)),
            _ => None,
        };
        if let Some(e) = spawn_object(
            &mut commands,
            &lib,
            entry,
            p,
            &asset_server,
            &graphics,
            rocks.as_deref(),
            glow,
        ) {
            store.entities.insert(id, e);
            budget -= 1;
        }
    }
    store.to_spawn = later;
}

/// The distance bands (`VisibilityRange`) of an object's LOD children, in the order
/// `spawn_object` gives them out (a tree: its three mesh LODs; a library model: near, and far
/// when it has one). None for imported models (their LODs don't follow the graphics settings).
/// `streaming` retunes the children with these when the settings change.
pub fn lod_ranges(
    entry: &LibraryEntry,
    g: &crate::streaming::Graphics,
    size: f32,
) -> Option<Vec<VisibilityRange>> {
    if let Some(tree) = &entry.tree {
        let r = tree.ranges.map(|d| sized(d * g.detail, size));
        // hard LOD switches: the camera's depth prepass can't dither opaque meshes between levels
        let band = |a: f32, b: f32| VisibilityRange {
            start_margin: a..a,
            end_margin: b..b,
            use_aabb: false,
        };
        let first = VisibilityRange {
            start_margin: 0.0..0.0,
            end_margin: r[0]..r[0],
            use_aabb: false,
        };
        return Some(vec![first, band(r[0], r[1]), band(r[1], r[2])]);
    }
    let (lod_d, cull) = kind_lod(entry.kind?);
    let lod_d = sized(lod_d * g.detail, size);
    let cull = sized((cull * g.detail.max(1.0)).min(g.draw_distance), size);
    let near = VisibilityRange {
        start_margin: 0.0..0.0,
        end_margin: lod_d..lod_d,
        use_aabb: false,
    };
    let far = VisibilityRange {
        start_margin: lod_d..lod_d,
        end_margin: cull..cull,
        use_aabb: false,
    };
    Some(if cull > lod_d {
        vec![near, far]
    } else {
        vec![near]
    })
}

/// The render entity (with LOD children) for one placed object.
fn spawn_object(
    commands: &mut Commands,
    lib: &Library,
    entry: &LibraryEntry,
    p: &Placed,
    asset_server: &AssetServer,
    g: &crate::streaming::Graphics,
    rocks: Option<&crate::visuals::rock_material::RockMaterials>,
    glow: Option<Handle<FoliageMaterial>>,
) -> Option<Entity> {
    let id = p.id;
    let tf = Transform {
        translation: p.translation(),
        rotation: p.rotation(),
        scale: p.scale3(),
    };
    if let Some(tree) = &entry.tree {
        let v = &tree.variants[(p.variant as usize).min(tree.variants.len() - 1)];
        let ranges = lod_ranges(entry, g, size_band(entry, p.scale)).unwrap_or_default();
        // (rocks that keep their grain at any size: `visuals::rock_material`)
        let rock = rocks.and_then(|r| r.for_object(entry.kind, p.scale));
        let mut ec = commands.spawn((
            Name::new(entry.name.clone()),
            PlacedObject { id },
            tf,
            Visibility::default(),
        ));
        ec.with_children(|c| {
            for (lod, ((bark, leaves), range)) in v.lods.iter().zip(ranges).enumerate() {
                // the farthest LOD skips shadows: in the wide cascades it would be sub-texel.
                // Small photoscanned plants never cast them, like the procedural ones.
                let shadows = lod < 2
                    && !matches!(
                        entry.kind,
                        Some(
                            AssetKind::Fern
                                | AssetKind::Flowers
                                | AssetKind::TallGrass
                                | AssetKind::Mushrooms
                                | AssetKind::Lavender
                                | AssetKind::WildStrawberry
                                | AssetKind::Tulip
                        )
                    );
                let mut b = match &rock {
                    Some(m) => c.spawn((
                        Mesh3d(bark.clone()),
                        MeshMaterial3d(m.clone()),
                        range.clone(),
                    )),
                    None => c.spawn((
                        Mesh3d(bark.clone()),
                        MeshMaterial3d(glow.clone().unwrap_or_else(|| tree.bark.clone())),
                        range.clone(),
                    )),
                };
                if entry.category == Category::Trees {
                    b.insert(MeshTag::new(crate::forest_contours::FOREST_TAG));
                }
                // Tree-atlas recapture uses more cameras than CPU VisibilityRange's 32-view
                // table can hold. Like scattered plants, placed plants keep their distance
                // and frustum culling on the GPU so those temporary views cannot hide them.
                if entry.category == Category::Plants {
                    b.insert(bevy::camera::visibility::NoCpuCulling);
                }
                if !shadows {
                    b.insert(NotShadowCaster);
                }
                if let Some(l) = leaves {
                    let mut e = c.spawn((
                        Mesh3d(l.clone()),
                        MeshMaterial3d(glow.clone().unwrap_or_else(|| tree.leaves.clone())),
                        range,
                    ));
                    if entry.category == Category::Trees {
                        e.insert(MeshTag::new(crate::forest_contours::FOREST_TAG));
                    }
                    if entry.category == Category::Plants {
                        e.insert(bevy::camera::visibility::NoCpuCulling);
                    }
                    if !shadows {
                        e.insert(NotShadowCaster);
                    }
                }
            }
            // beyond the last LOD the tree is part of a billboard chunk (streaming.rs)
            if glow.is_some()
                && let Some(g) = &p.glow
            {
                glow::spawn_light(c, p, g, v.height);
            }
        });
        Some(ec.id())
    } else if entry.kind.is_some() {
        // ranges are per asset type (not per instance): Bevy keeps a GPU table of distinct ranges
        let v = (p.variant as usize).min(entry.meshes.len().saturating_sub(1));
        let (m0, m1) = entry.meshes[v].clone();
        let bands = lod_ranges(entry, g, size_band(entry, p.scale)).unwrap_or_default();
        let (near, far) = (bands[0].clone(), bands.get(1).cloned().unwrap_or_default());
        let has_far = bands.len() > 1;
        let mut ec = commands.spawn((
            Name::new(entry.name.clone()),
            PlacedObject { id },
            tf,
            Visibility::default(),
        ));
        if entry.walkable {
            ec.insert(Walkable(entry.surface));
        }
        let small = matches!(entry.category, Category::Plants);
        if entry.sway {
            ec.with_children(|c| {
                let mut a = c.spawn((Mesh3d(m0), MeshMaterial3d(lib.foliage.clone()), near));
                if small {
                    a.insert(NotShadowCaster);
                }
                if has_far {
                    c.spawn((
                        Mesh3d(m1),
                        MeshMaterial3d(lib.foliage.clone()),
                        far,
                        NotShadowCaster,
                    ));
                }
            });
        } else {
            ec.with_children(|c| {
                // far LODs don't cast shadows: in the wide cascades they'd be sub-texel blobs
                // but would be re-rendered into every cascade
                let far_shadows = entry.category == Category::Structures && !small;
                if entry.textured {
                    c.spawn((Mesh3d(m0), MeshMaterial3d(lib.surface.clone()), near));
                    if has_far {
                        let mut b = c.spawn((Mesh3d(m1), MeshMaterial3d(lib.surface.clone()), far));
                        if !far_shadows {
                            b.insert(NotShadowCaster);
                        }
                    }
                } else {
                    c.spawn((Mesh3d(m0), MeshMaterial3d(lib.solid.clone()), near));
                    if has_far {
                        let mut b = c.spawn((Mesh3d(m1), MeshMaterial3d(lib.solid.clone()), far));
                        if !far_shadows {
                            b.insert(NotShadowCaster);
                        }
                    }
                }
            });
        }
        Some(ec.id())
    } else if let Some(path) = &entry.import_path {
        let scene = entry.scene.clone().unwrap_or_else(|| {
            asset_server.load(GltfAssetLabel::Scene(0).from_asset(path.clone()))
        });
        let mut ec = commands.spawn((
            Name::new(entry.name.clone()),
            PlacedObject { id },
            tf,
            WorldAssetRoot(scene),
        ));
        if entry.walkable {
            ec.insert(Walkable(entry.surface));
        }
        Some(ec.id())
    } else {
        None
    }
}

/// World-wide nature dials saved with the world: meadow grass and independent plants, flowers,
/// mushrooms and berries wherever biome rules allow them.
#[derive(Resource, Clone, Debug, PartialEq, Serialize)]
pub struct NatureAmounts {
    pub grass: f32,
    /// Blade length multiplier world-wide (1 = as grown; 0.5 short turf … 2.5 tall meadow).
    pub grass_height: f32,
    pub plants: f32,
    /// World-wide flora family amounts. Older worlds use their saved `plants` value for each.
    pub flowers: f32,
    pub mushrooms: f32,
    pub berries: f32,
    /// Procedural trees, rocks and debris world-wide.
    pub trees: f32,
    pub rocks: f32,
    pub debris: f32,
    /// plants amount the world's current plants were grown with
    #[serde(skip)]
    pub applied_plants: f32,
}

impl Default for NatureAmounts {
    fn default() -> Self {
        Self {
            grass: 1.0,
            grass_height: 1.0,
            plants: 1.0,
            flowers: 1.0,
            mushrooms: 1.0,
            berries: 1.0,
            trees: 1.0,
            rocks: 1.0,
            debris: 1.0,
            applied_plants: 1.0,
        }
    }
}

impl NatureAmounts {
    /// Per-family flora amounts: plants, flowers, mushrooms, berries.
    pub fn flora_amounts(&self) -> [f32; 4] {
        [self.plants, self.flowers, self.mushrooms, self.berries]
    }
}

impl<'de> Deserialize<'de> for NatureAmounts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Saved {
            grass: Option<f32>,
            grass_height: Option<f32>,
            plants: Option<f32>,
            flowers: Option<f32>,
            mushrooms: Option<f32>,
            berries: Option<f32>,
            trees: Option<f32>,
            rocks: Option<f32>,
            debris: Option<f32>,
        }

        let saved = Saved::deserialize(deserializer)?;
        let defaults = Self::default();
        let plants = saved.plants.unwrap_or(defaults.plants);
        Ok(Self {
            grass: saved.grass.unwrap_or(defaults.grass),
            grass_height: saved.grass_height.unwrap_or(defaults.grass_height),
            plants,
            // Worlds saved before the separate flora dials retain their exact plants value,
            // including zero, for every family.
            flowers: saved.flowers.unwrap_or(plants),
            mushrooms: saved.mushrooms.unwrap_or(plants),
            berries: saved.berries.unwrap_or(plants),
            trees: saved.trees.unwrap_or(defaults.trees),
            rocks: saved.rocks.unwrap_or(defaults.rocks),
            debris: saved.debris.unwrap_or(defaults.debris),
            applied_plants: defaults.applied_plants,
        })
    }
}

#[cfg(test)]
mod nature_amount_tests {
    use super::NatureAmounts;

    #[test]
    fn old_flora_amounts_expand_without_losing_zero_or_precision() {
        for plants in [0.0, 1.0, 1.97] {
            let saved = format!(r#"{{"plants":{plants}}}"#);
            let amounts: NatureAmounts = serde_json::from_str(&saved).unwrap();
            assert_eq!(amounts.flora_amounts(), [plants; 4]);
        }
    }

    #[test]
    fn new_flora_amounts_round_trip() {
        let amounts = NatureAmounts {
            plants: 0.25,
            flowers: 0.5,
            mushrooms: 1.25,
            berries: 1.75,
            ..NatureAmounts::default()
        };
        let restored: NatureAmounts =
            serde_json::from_str(&serde_json::to_string(&amounts).unwrap()).unwrap();
        assert_eq!(restored.flora_amounts(), amounts.flora_amounts());
        assert_eq!(restored.grass, amounts.grass);
        assert_eq!(restored.trees, amounts.trees);
        assert_eq!(restored.rocks, amounts.rocks);
        assert_eq!(restored.debris, amounts.debris);
    }
}

#[allow(clippy::too_many_arguments)]
fn place_tool(
    stroke: Res<Stroke>,
    ed: Res<Editor>,
    settings: Res<PlaceSettings>,
    lib: Option<Res<Library>>,
    terrain: Res<Terrain>,
    water: Res<crate::water::WaterBodies>,
    handles: Res<crate::placement::Handles>,
    pointer: Res<crate::camera::Pointer>,
    caves: Res<crate::caves::render::CaveWorld>,
    mut store: ResMut<ObjectStore>,
    mut history: ResMut<History>,
    // (the new object, and the ground's normal it was tilted to)
    mut placing: Local<Option<(u64, Vec3)>>,
) {
    // (a press on an object or a handle selects / drags it instead: `placement::handles`)
    if ed.tab != Tab::Objects || ed.object_tool != ObjectTool::Place || handles.took_press {
        *placing = None;
        return;
    }
    let Some(lib) = lib else { return };
    if stroke.just_started
        && let (Some(hit), Some(sel)) = (stroke.hit, settings.selected.as_ref())
        && let Some(e) = lib.get(sel)
        && crate::market::allow(&e.id)
    {
        let yaw = if settings.random_yaw {
            fastrand::f32() * std::f32::consts::TAU
        } else {
            0.0
        };
        let align = settings.align || e.align;
        // on a cave's rock: its floor, or a wall or the roof — stuck to it when the object follows
        // the slope, else stood on the floor in front of it
        let (at, n) = match pointer.cave_normal.filter(|_| pointer.hit == Some(hit)) {
            Some(n) if n.y < 0.5 && !align => {
                let out = Vec3::new(n.x, 0.0, n.z).normalize_or_zero()
                    * (e.radius * settings.scale * e.default_scale + 0.2);
                let q = hit + n * 0.3 + out;
                (
                    caves
                        .floor_under(q, 40.0)
                        .map_or(hit, |f| Vec3::new(q.x, f, q.z)),
                    Vec3::Y,
                )
            }
            Some(n) => (hit, n),
            None => (hit, terrain.normal_at(hit.x, hit.z)),
        };
        let rot = if align {
            Quat::from_rotation_arc(Vec3::Y, n) * Quat::from_rotation_y(yaw)
        } else {
            Quat::from_rotation_y(yaw)
        };
        let id = store.new_id();
        history.touch_object(&store, id);
        store.insert(Placed {
            group: 0,
            id,
            asset: e.id.clone(),
            variant: 0,
            // (sunk a little into what it stands on; stuck to a cave wall or roof: into the rock)
            pos: (e.seat(
                    at - (if align && pointer.cave_normal.is_some() {
                        n
                    } else {
                        Vec3::Y
                    }) * 0.05,
                    &water,
                    terrain.sea_level,
                )
                // (floating rocks hang in the air above the spot)
                + Vec3::Y
                    * crate::visuals::floating::hover_for(e.kind, settings.scale * e.default_scale))
            .to_array(),
            rot: rot.to_array(),
            scale: settings.scale * e.default_scale,
            stretch: None,
            locked: false,
            glow: None,
        });
        store.selected = Some(id);
        *placing = Some((id, n));
    } else if stroke.active
        && let (Some((id, n)), Some(hit)) = (*placing, stroke.hit)
        && let Some(p) = store.items.get(&id).cloned()
    {
        // drag after clicking to rotate the new object toward the cursor
        let d = Vec2::new(hit.x - p.pos[0], hit.z - p.pos[2]);
        if d.length() > 1.5 {
            let yaw = (-d.y).atan2(d.x);
            let mut np = p.clone();
            let e_align = lib.get(&p.asset).map(|e| e.align).unwrap_or(false) || settings.align;
            let base = if e_align {
                Quat::from_rotation_arc(Vec3::Y, n)
            } else {
                Quat::IDENTITY
            };
            np.rot = (base * Quat::from_rotation_y(yaw)).to_array();
            store.update(np);
        }
    }
    if stroke.just_ended {
        *placing = None;
    }
}

/// Keys for the selection (Objects tab, either tool): Delete removes it, Z / X turn it by 15°,
/// Ctrl/⌘ D duplicates it beside itself, Esc deselects. Picking and dragging (move, turn, resize,
/// lift) are the viewport handles (`placement::handles`).
#[allow(clippy::too_many_arguments)]
fn select_tool(
    mut ed: ResMut<Editor>,
    pointer: Res<Pointer>,
    lib: Option<Res<Library>>,
    terrain: Res<Terrain>,
    water: Res<crate::water::WaterBodies>,
    keys: Res<ButtonInput<KeyCode>>,
    bounds: Res<crate::placement::ModelBounds>,
    mut store: ResMut<ObjectStore>,
    mut history: ResMut<History>,
    caves: Res<crate::caves::render::CaveWorld>,
) {
    if ed.tab != Tab::Objects || pointer.keyboard_captured {
        return;
    }
    let Some(id) = store.selected else { return };
    // locked (with its set): Delete and Z / X leave it (Esc and ⌘D still work)
    if store.is_locked(id) {
        let cmd = keys.pressed(KeyCode::SuperLeft)
            || keys.pressed(KeyCode::SuperRight)
            || keys.pressed(KeyCode::ControlLeft)
            || keys.pressed(KeyCode::ControlRight);
        let editing = keys.just_pressed(KeyCode::Delete)
            || keys.just_pressed(KeyCode::Backspace)
            || (!cmd && (keys.just_pressed(KeyCode::KeyZ) || keys.just_pressed(KeyCode::KeyX)));
        if editing {
            ed.toast("It's locked — right-click it › Unlock to change it");
            return;
        }
    }
    // a set piece's piece: Delete, ⌘D and Z / X act on the whole set
    let group = store.items.get(&id).map(|p| p.group).unwrap_or(0);
    if group != 0 {
        let cmd = keys.pressed(KeyCode::SuperLeft)
            || keys.pressed(KeyCode::SuperRight)
            || keys.pressed(KeyCode::ControlLeft)
            || keys.pressed(KeyCode::ControlRight);
        if keys.just_pressed(KeyCode::Escape) {
            store.selected = None;
        } else if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace) {
            history.begin("Delete set");
            for m in store.group_members(group) {
                history.touch_object(&store, m);
                store.remove(m);
            }
            history.commit(&store);
        } else if cmd && keys.just_pressed(KeyCode::KeyD) {
            if let Some(lib) = lib.as_deref() {
                let copy =
                    crate::placement::group::duplicate(&mut store, lib, &bounds, &terrain, group);
                if !copy.is_empty() {
                    history.begin("Duplicate set");
                    let first = copy[0].id;
                    for p in copy {
                        history.touch_object(&store, p.id);
                        store.insert(p);
                    }
                    store.selected = Some(first);
                    history.commit(&store);
                }
            }
        } else if !cmd && (keys.just_pressed(KeyCode::KeyZ) || keys.just_pressed(KeyCode::KeyX)) {
            let turn = if keys.just_pressed(KeyCode::KeyZ) {
                15f32.to_radians()
            } else {
                -15f32.to_radians()
            };
            if let Some(f) = lib
                .as_deref()
                .and_then(|l| crate::placement::group::frame(&store, l, &bounds, group))
            {
                history.begin("Rotate set");
                let starts = crate::placement::group::snapshot(&store, group);
                let ch = crate::placement::group::Change {
                    yaw: turn,
                    ..crate::placement::group::Change::none(f.translation())
                };
                crate::placement::group::transform(
                    &mut store,
                    &mut history,
                    &terrain,
                    group,
                    &starts,
                    ch,
                );
                store.selected = Some(id);
                history.commit(&store);
            }
        }
        return;
    }
    let cmd = keys.pressed(KeyCode::SuperLeft)
        || keys.pressed(KeyCode::SuperRight)
        || keys.pressed(KeyCode::ControlLeft)
        || keys.pressed(KeyCode::ControlRight);
    if keys.just_pressed(KeyCode::Escape) {
        store.selected = None;
    } else if keys.just_pressed(KeyCode::Delete) || keys.just_pressed(KeyCode::Backspace) {
        history.begin("Delete object");
        history.touch_object(&store, id);
        store.remove(id);
        history.commit(&store);
    } else if cmd && keys.just_pressed(KeyCode::KeyD) {
        if let Some(copy) = duplicate(
            &mut store,
            lib.as_deref(),
            &terrain,
            &water,
            Some(&caves),
            id,
        ) {
            history.begin("Duplicate object");
            history.touch_object(&store, copy.id);
            store.insert(copy.clone());
            store.selected = Some(copy.id);
            history.commit(&store);
        }
    } else if !cmd {
        let mut turn = 0.0;
        if keys.just_pressed(KeyCode::KeyZ) {
            turn = 15f32.to_radians();
        }
        if keys.just_pressed(KeyCode::KeyX) {
            turn = -15f32.to_radians();
        }
        if turn != 0.0
            && let Some(p) = store.items.get(&id).cloned()
        {
            history.begin("Rotate object");
            history.touch_object(&store, id);
            let mut np = p;
            np.rot = (Quat::from_rotation_y(turn) * np.rotation()).to_array();
            store.update(np);
            history.commit(&store);
        }
    }
}

/// Objects moved in place (`ObjectStore::move_to`) take their new transforms.
fn apply_moves(mut store: ResMut<ObjectStore>, mut q: Query<&mut Transform>) {
    for (e, t) in std::mem::take(&mut store.to_move) {
        if let Ok(mut tf) = q.get_mut(e) {
            *tf = t;
        }
    }
}

/// A copy of object `id` (new id, not yet inserted) standing beside it on the ground (in a cave:
/// on its floor), one footprint over along its own x axis.
pub fn duplicate(
    store: &mut ObjectStore,
    lib: Option<&Library>,
    terrain: &Terrain,
    water: &crate::water::WaterBodies,
    caves: Option<&crate::caves::render::CaveWorld>,
    id: u64,
) -> Option<Placed> {
    let p = store.items.get(&id)?.clone();
    // (an asset from a pack this account hasn't got stays where it is, but isn't copied)
    if !crate::market::allow(&p.asset) {
        return None;
    }
    let e = lib.and_then(|l| l.get(&p.asset));
    let step = e.map(|e| e.radius * 2.0 / 1.1).unwrap_or(1.0) * p.scale3().max_element() + 1.0;
    let side = p.rotation() * Vec3::X;
    let at = p.translation() + Vec3::new(side.x, 0.0, side.z).normalize_or(Vec3::X) * step;
    // on the ground as far below it as the original stood (a lifted lamp stays lifted)
    let cave_floor = |q: Vec3| caves.and_then(|c| c.floor_under(q + Vec3::Y * 1.2, 6.0));
    let ground = match (cave_floor(p.translation()), cave_floor(at)) {
        (Some(f0), Some(f1)) => Vec3::new(at.x, f1 + p.pos[1] - f0, at.z),
        _ => {
            let lift = p.pos[1] - terrain.height_at(p.pos[0], p.pos[2]);
            Vec3::new(at.x, terrain.height_at(at.x, at.z) + lift, at.z)
        }
    };
    let pos = e
        .map(|e| e.seat(ground, water, terrain.sea_level))
        .unwrap_or(ground);
    Some(Placed {
        id: store.new_id(),
        pos: pos.to_array(),
        locked: false,
        glow: None,
        ..p
    })
}

/// After terrain edits, re-seat objects onto the ground (once per stroke end).
/// Ground heights at the moment objects were last fitted to the terrain.
#[derive(Resource, Default)]
pub struct GroundSnapshot {
    heights: Vec<f32>,
    res: usize,
    generation: u64,
    revision: u64,
}

impl GroundSnapshot {
    pub fn sync(&mut self, t: &Terrain) {
        self.heights.clone_from(&t.heights);
        self.res = t.res;
        self.generation = t.generation;
        self.revision = t.height_revision;
    }

    /// Bilinear height of the snapshot (same grid mapping as `Terrain::height_at`).
    fn height_at(&self, t: &Terrain, x: f32, z: f32) -> f32 {
        let g = t.world_to_grid(x, z);
        let max = self.res as f32;
        let (gx, gz) = (g.x.clamp(0.0, max), g.y.clamp(0.0, max));
        let i0 = (gx.floor() as usize).min(self.res - 1);
        let j0 = (gz.floor() as usize).min(self.res - 1);
        let (fx, fz) = (gx - i0 as f32, gz - j0 as f32);
        let n = self.res + 1;
        let h = |i: usize, j: usize| self.heights[j.min(self.res) * n + i.min(self.res)];
        let a = h(i0, j0) + (h(i0 + 1, j0) - h(i0, j0)) * fx;
        let b = h(i0, j0 + 1) + (h(i0 + 1, j0 + 1) - h(i0, j0 + 1)) * fx;
        a + (b - a) * fz
    }
}

/// Objects ride the terrain: after any height change (brush stroke, erode/smooth world, generate,
/// undo/redo) each object moves by the change in ground height beneath it. Deliberate offsets
/// (a bridge above a river, a dock on posts) are kept, and objects placed explicitly by the same
/// change (history entries) are left where they were put.
fn follow_ground(
    stroke: Res<Stroke>,
    terrain: Res<Terrain>,
    history: Res<History>,
    mut snap: ResMut<GroundSnapshot>,
    mut store: ResMut<ObjectStore>,
) {
    if stroke.active {
        return;
    }
    if snap.generation != terrain.generation
        || snap.res != terrain.res
        || snap.heights.len() != terrain.heights.len()
    {
        // a new or loaded world: objects already sit on this terrain
        snap.sync(&terrain);
        return;
    }
    if snap.revision == terrain.height_revision {
        return;
    }
    let moved: Vec<Placed> = store
        .items
        .values()
        .filter(|p| !history.recent_objects.contains(&p.id))
        // (things well under the ground were put there on purpose: in a cave, they stay put)
        .filter(|p| p.pos[1] > snap.height_at(&terrain, p.pos[0], p.pos[2]) - 2.0)
        .filter_map(|p| {
            let d = terrain.height_at(p.pos[0], p.pos[2])
                - snap.height_at(&terrain, p.pos[0], p.pos[2]);
            (d.abs() > 0.02).then(|| {
                let mut np = p.clone();
                np.pos[1] += d;
                np
            })
        })
        .collect();
    for p in moved {
        store.update(p);
    }
    snap.sync(&terrain);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shipped preview of a paid pack (`PREVIEWS`) is listed only until the pack itself is
    /// there (bundled or bought), and its entries point at the shipped pictures.
    #[test]
    fn a_shipped_preview_gives_way_to_the_full_pack() {
        let root = std::env::temp_dir().join(format!("wb-previews-{}", fastrand::u64(..)));
        let (bundle, bought) = (root.join("bundle/models"), root.join("packs/models"));
        let write = |dir: &std::path::Path, body: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("models.json"), body).unwrap();
        };
        let model = |id: &str, gltf: &str| {
            format!(
                r#"{{"id":"{id}","name":"{id}","category":"props","size_m":[1,1,1],"gltf":"{gltf}"}}"#
            )
        };
        write(
            &bundle.join(PREVIEWS).join("handpainted"),
            &format!(
                r#"{{"style":"handpainted","pack":"skyfold_studio","preview":true,"models":[{},{}]}}"#,
                model("well", "well/well.gltf"),
                model("inn", "inn/inn.gltf")
            ),
        );
        let hp = |libs: &[ModelLibrary]| {
            let v: Vec<ModelLibrary> = libs.iter().filter(|l| l.prefix == "hp").cloned().collect();
            assert_eq!(v.len(), 1, "one hand-painted library: {v:?}");
            v.into_iter().next().unwrap()
        };
        // not bought: the preview, under its own folder (not models/stylepacks/, which the town
        // and set-piece builders take as "the pack is here")
        let lib = hp(&model_libraries_in(&bundle, &bought));
        assert!(lib.preview);
        assert_eq!(
            (lib.folder.as_str(), lib.pack.as_str(), lib.pack_id.as_str()),
            (
                "stylepack_previews/handpainted",
                "handpainted",
                "skyfold_studio"
            )
        );
        let entries = model_library(&lib);
        assert_eq!(entries.len(), 2, "every model is listed");
        let well = entries.iter().find(|e| e.id == "hp:well").unwrap();
        assert_eq!(
            well.import_path.as_deref(),
            Some("models/stylepack_previews/handpainted/well/well.gltf")
        );
        assert_eq!(
            preview_thumb(well).as_deref(),
            Some("models/stylepack_previews/handpainted/thumbs/well.png")
        );
        assert_eq!(well.pack, "handpainted");
        // (a manifest without its own pack id is still the pack its full library is sold in)
        assert_eq!(
            default_pack_id("stylepack_previews/handpainted"),
            "skyfold_studio"
        );
        // bought: the installed full library replaces the preview whole
        write(
            &bought.join("stylepacks/handpainted"),
            &format!(
                r#"{{"pack":"handpainted","models":[{},{},{}]}}"#,
                model("well", "well/well.gltf"),
                model("inn", "inn/inn.gltf"),
                model("bank", "bank/bank.gltf")
            ),
        );
        let lib = hp(&model_libraries_in(&bundle, &bought));
        assert!(!lib.preview);
        assert_eq!(lib.dir, bought.join("stylepacks/handpainted"));
        let entries = model_library(&lib);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|e| preview_thumb(e).is_none()));
        assert_eq!(
            entries[0].import_path.as_deref(),
            Some("models/stylepacks/handpainted/well/well.gltf")
        );
        // a bundle with the full pack (the developer checkout) wins over both
        write(
            &bundle.join("stylepacks/handpainted"),
            &format!(
                r#"{{"pack":"handpainted","models":[{}]}}"#,
                model("well", "well/well.gltf")
            ),
        );
        let lib = hp(&model_libraries_in(&bundle, &bought));
        assert!(!lib.preview && lib.dir == bundle.join("stylepacks/handpainted"));
        // pictures only for preview entries
        let mut e = model_library(&lib).remove(0);
        assert!(preview_thumb(&e).is_none());
        e.import_path = None;
        assert!(preview_thumb(&e).is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn style_packs_load_with_their_pack() {
        let dir = std::path::PathBuf::from(crate::assets_path()).join("models");
        let libs = model_libraries(&dir);
        assert_eq!(
            (libs[0].prefix.as_str(), libs[0].pack.as_str()),
            ("ph", PHOTOREAL)
        );
        assert_eq!(
            (libs[1].prefix.as_str(), libs[1].pack.as_str()),
            ("sb", PHOTOREAL)
        );
        // the marketplace packs they're sold in (crate::market)
        let ids: Vec<&str> = [&libs[0], &libs[1]]
            .iter()
            .map(|l| l.pack_id.as_str())
            .collect();
        assert_eq!(ids, ["polyhaven_scans", "skyborn_village"]);
        // The public build deliberately omits this paid pack. If it is installed, retain the
        // full manifest and glTF integration audit.
        if let Some(hp) = libs.iter().find(|l| l.pack == "handpainted") {
            assert_eq!(
                (hp.folder.as_str(), hp.prefix.as_str()),
                ("stylepacks/handpainted", "hp")
            );
            assert_eq!(hp.pack_id, "skyfold_studio");
            let entries = model_library(hp);
            assert!(entries.len() >= 70, "{} hand-painted models", entries.len());
            assert!(
                entries
                    .iter()
                    .all(|e| e.id.starts_with("hp:") && e.pack == "handpainted")
            );
            let inn = entries.iter().find(|e| e.id == "hp:inn").expect("the inn");
            assert_eq!(
                (inn.category, inn.group.as_str()),
                (Category::Structures, "buildings")
            );
            // the inn has rooms to walk into (room colliders); a closed building (the open market
            // stall is walkable=false) isn't walk-in
            assert!(inn.walk_in());
            assert!(
                !entries
                    .iter()
                    .find(|e| e.id == "hp:market_stall")
                    .expect("the market stall")
                    .walk_in()
            );
            assert!(entries.iter().any(|e| e.id == "hp:iron_gate"
                && e.category == Category::Props
                && e.group == "graveyard"));
            // every model and the textures it references are there
            let assets = dir.parent().unwrap();
            for e in &entries {
                let path = assets.join(e.import_path.as_ref().unwrap());
                let doc: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                for img in doc["images"].as_array().into_iter().flatten() {
                    let uri = img["uri"].as_str().unwrap();
                    assert!(
                        path.parent().unwrap().join(uri).is_file(),
                        "{}: missing {uri}",
                        e.id
                    );
                }
            }
        }
        let ph = model_library(&libs[0]);
        assert!(
            !ph.is_empty()
                && ph
                    .iter()
                    .all(|e| e.pack == PHOTOREAL && e.id.starts_with("ph:"))
        );
        assert_eq!(pack_label("handpainted"), "Hand-painted");
        assert_eq!(pack_label(PHOTOREAL), "Photoreal");
        assert_eq!(pack_prefix("toon"), "toon");
    }
}
