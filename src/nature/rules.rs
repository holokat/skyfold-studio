//! The rules that decide what grows where: the vegetation presets (`objects::presets`) as
//! per-category layer tables, and which preset a spot of land gets.
//!
//! A spot's preset comes, in order, from a painted preset (the Nature tab's ecosystem brushes),
//! its biome (the biome's vegetation list, picked by the same large-scale patch noise as its
//! ground, so woods stand on forest floor and clearings on grass), or — on land without a biome,
//! once the world has been populated — the land itself: slope, altitude, moisture and forest
//! noise (the classic auto-populate rules, zone biomes included).

use bevy::prelude::*;

use super::{ForestDiversity, ForestLayout};
use crate::biomes::BiomePreset;
use crate::objects::{Category, Library, kind_category, presets};
use crate::procgen::meshes::AssetKind;
use crate::world::{Biome, Zones};

/// The broad ecological context allowed to replace a legacy automatic tree layer.  This stays
/// deliberately small: the legacy layer still decides density and where trees can grow; it only
/// answers whether a colourful replacement belongs at that spot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForestEcology {
    Temperate,
    Autumn,
    Jungle,
    Cold,
    Excluded,
}

/// The procedural categories, in layer order.
/// Independent abundance family beneath the stable Plants density channel.
/// None means a non-flora category, whose amount is already applied by Ctx::amounts.
pub fn flora_group(kind: AssetKind) -> Option<usize> {
    use AssetKind::*;
    match kind {
        Flowers | FlowerBush | Lavender | Tulip => Some(1),
        Mushrooms => Some(2),
        WildStrawberry | Blueberry | Blackberry => Some(3),
        _ if kind_category(kind) == Category::Plants => Some(0),
        _ => None,
    }
}

pub const CATS: [Category; 4] = [
    Category::Trees,
    Category::Rocks,
    Category::Plants,
    Category::Debris,
];

pub fn cat_index(c: Category) -> Option<usize> {
    CATS.iter().position(|x| *x == c)
}

/// The old object scatter grew each preset in overlapping 70 m dabs with a soft edge: on average
/// a spot got about this multiple of a layer's nominal density (dab overlap × falloff).
const DAB_COVERAGE: f32 = 1.12;
/// … and the populate tools scaled every dab by this.
pub const POPULATE: f32 = 0.85;

/// One layer of a preset: a kind and how it grows.
#[derive(Clone, Debug)]
pub struct Layer {
    pub kind: AssetKind,
    /// library entry index
    pub entry: usize,
    /// instances per m² at amount 1
    pub density: f32,
    pub scale: (f32, f32),
    pub max_slope: f32,
    /// altitude above sea (m)
    pub alt: (f32, f32),
}

/// A preset split by category.
pub struct PresetRules {
    pub name: &'static str,
    pub cats: [Vec<Layer>; 4],
    /// summed layer density per category (per m²)
    pub total: [f32; 4],
}

/// Rule tables built once the asset library exists.
#[derive(Resource)]
pub struct Rules {
    pub presets: Vec<PresetRules>,
    /// per biome-map value: (vegetation preset indices, densest first; density factor)
    biome_veg: Vec<(Vec<u8>, f32)>,
    /// library entry per asset kind (indexed by `AssetKind as usize`)
    pub kind_entry: Vec<Option<usize>>,
    /// variants per asset kind
    pub kind_variants: Vec<u8>,
    fallback: Fallback,
    /// the world art style the tables were built for (`art_style::kind_grows`)
    pub art: crate::art_style::ArtStyle,
}

/// Preset indices used on land without a biome.
struct Fallback {
    mixed: u8,
    pine: u8,
    autumn: u8,
    sparse: u8,
    meadow: u8,
    alpine: u8,
    jungle: u8,
    beach: u8,
    desert: u8,
    swamp: u8,
    rocky: u8,
    tundra: u8,
    volcanic: u8,
}

/// No preset grows here.
pub const NONE: u8 = 255;

/// Bamboo was a hidden placeholder when the original Jungle preset shipped. Keeping that layer
/// out of the rule table preserves old worlds exactly while the visible species comes through
/// the explicit Bamboo Forest brush or rich-forest replacement of the legacy palm layer.
fn grows_in_rule_table(
    preset_name: &str,
    kind: AssetKind,
    art: crate::art_style::ArtStyle,
) -> bool {
    !(preset_name == "Jungle" && kind == AssetKind::Bamboo)
        && crate::art_style::kind_grows(kind, art)
}

/// A deterministic weighted draw in [0, 1). Keeping legacy green species heavier makes the
/// optional colour trees read as woodland variety rather than a uniformly ornamental orchard.
fn palette_pick(palette: &[(AssetKind, u16)], sample: f32) -> AssetKind {
    let total: u32 = palette.iter().map(|(_, weight)| *weight as u32).sum();
    let mut pick = (sample.clamp(0.0, 0.999_999) * total as f32) as u32;
    for &(kind, weight) in palette {
        if pick < weight as u32 {
            return kind;
        }
        pick -= weight as u32;
    }
    palette.last().expect("non-empty forest palette").0
}

fn forest_sample(p: Vec2, seed: u32, layout: ForestLayout) -> f32 {
    match layout {
        // Hundreds-of-metres value noise forms related stands rather than a checkerboard.
        ForestLayout::NaturalGroves => {
            crate::noise::value2(p.x / 260.0, p.y / 260.0, seed.wrapping_add(0x5A4B_9E31))
        }
        // Slots need an actually uniform value: smooth value noise at a small scale left some
        // palette intervals underrepresented and could make species disappear from a region.
        ForestLayout::Mixed => {
            let h = crate::noise::hash_u32(
                p.x.to_bits() ^ p.y.to_bits().rotate_left(13) ^ seed.wrapping_mul(0x5A4B_9E31),
            );
            h as f32 / (u32::MAX as f32 + 1.0)
        }
    }
}

impl Rules {
    /// A deterministic replacement for automatic tree layers. It never changes layer density,
    /// painted species, or the disabled legacy path.
    pub fn resolve_tree(
        &self,
        base: AssetKind,
        p: Vec2,
        seed: u32,
        forest: ForestDiversity,
        ecology: ForestEcology,
    ) -> AssetKind {
        if !forest.enabled || matches!(ecology, ForestEcology::Cold | ForestEcology::Excluded) {
            return base;
        }
        use AssetKind::*;
        let palette: &[(AssetKind, u16)] = match base {
            Oak | Birch if matches!(ecology, ForestEcology::Temperate | ForestEcology::Autumn) => {
                &[
                    (Oak, 30),
                    (Birch, 20),
                    (Maple, 10),
                    (GoldenMaple, 8),
                    (SakuraPink, 10),
                    (SakuraWhite, 6),
                    (Ginkgo, 8),
                ]
            }
            Maple if matches!(ecology, ForestEcology::Temperate | ForestEcology::Autumn) => &[
                (Maple, 30),
                (GoldenMaple, 20),
                (Ginkgo, 16),
                (SakuraPink, 18),
                (SakuraWhite, 16),
            ],
            Pine | Fir | Spruce | Cedar => {
                // Giant sequoias are rare individual anchors, even inside a conifer grove: their
                // large crown must not inherit ordinary pine spacing.
                if matches!(ecology, ForestEcology::Temperate | ForestEcology::Autumn) {
                    let h = crate::noise::hash_u32(
                        p.x.to_bits()
                            ^ p.y.to_bits().rotate_left(13)
                            ^ seed.wrapping_mul(0x72F1_12A9),
                    );
                    // A hash is uniform while value noise is spatially smooth, so this really is
                    // a 3.5% per-slot anchor chance rather than an accidental contour threshold.
                    if (h as f64 / (u32::MAX as f64 + 1.0)) < 0.035 {
                        return Sequoia;
                    }
                }
                &[(Pine, 30), (Fir, 25), (Spruce, 25), (Cedar, 20)]
            }
            Palm | Bamboo if ecology == ForestEcology::Jungle => &[(Bamboo, 55), (Palm, 45)],
            _ => return base,
        };
        palette_pick(palette, forest_sample(p, seed, forest.layout))
    }
    /// The tables for a world of art style `art`: in a hand-painted world the presets (biome
    /// vegetation, ecosystem brushes) leave out the kinds that don't fit it (photoscanned rocks,
    /// shrubs, ground plants, dead wood: `art_style::kind_grows`). Species painted on purpose
    /// (Nature › a species, with Show other styles) still grow.
    pub fn build(lib: &Library, art: crate::art_style::ArtStyle) -> Self {
        let ps = presets();
        let mut kind_entry = vec![None; AssetKind::ALL.len()];
        let mut kind_variants = vec![0u8; AssetKind::ALL.len()];
        for k in AssetKind::ALL {
            let i = *k as usize;
            if let Some(e) = lib.index_of(&crate::objects::builtin_id(*k)) {
                kind_entry[i] = Some(e);
                let le = &lib.entries[e];
                let n = le
                    .tree
                    .as_ref()
                    .map(|t| t.variants.len())
                    .unwrap_or(le.meshes.len());
                kind_variants[i] = n.clamp(1, 255) as u8;
            }
        }
        let presets: Vec<PresetRules> = ps
            .iter()
            .map(|p| {
                let mut cats: [Vec<Layer>; 4] = Default::default();
                for l in p
                    .layers
                    .iter()
                    .filter(|l| grows_in_rule_table(p.name, l.kind, art))
                {
                    let (Some(entry), Some(ci)) = (
                        kind_entry[l.kind as usize],
                        cat_index(kind_category(l.kind)),
                    ) else {
                        continue;
                    };
                    cats[ci].push(Layer {
                        kind: l.kind,
                        entry,
                        density: l.density / 1000.0,
                        scale: l.scale,
                        max_slope: l.max_slope,
                        alt: l.alt,
                    });
                }
                let total = std::array::from_fn(|c| cats[c].iter().map(|l| l.density).sum());
                PresetRules {
                    name: p.name,
                    cats,
                    total,
                }
            })
            .collect();
        let find = |name: &str| ps.iter().position(|p| p.name == name).unwrap_or(0) as u8;
        let mut biome_veg = vec![(vec![], 0.0); 16];
        for b in BiomePreset::ALL {
            let def = b.def();
            let list = def.vegetation.iter().map(|n| find(n)).collect();
            biome_veg[(b.id() as usize).min(15)] = (list, def.density);
        }
        let fallback = Fallback {
            mixed: find("Mixed Forest"),
            pine: find("Pine Forest"),
            autumn: find("Autumn Woods"),
            sparse: find("Sparse Woodland"),
            meadow: find("Flower Meadow"),
            alpine: find("Alpine"),
            jungle: find("Jungle"),
            beach: find("Palm Beach"),
            desert: find("Desert"),
            swamp: find("Swamp"),
            rocky: find("Rocky Hills"),
            tundra: find("Tundra"),
            volcanic: find("Volcanic"),
        };
        Self {
            presets,
            biome_veg,
            kind_entry,
            kind_variants,
            fallback,
            art,
        }
    }

    pub fn preset_index(&self, name: &str) -> Option<u8> {
        let n = name.replace(['_', ' ', '-'], "").to_lowercase();
        self.presets
            .iter()
            .position(|p| p.name.replace([' ', '-'], "").to_lowercase() == n)
            .map(|i| i as u8)
    }

    /// The preset growing at `p` and its density factor (`NONE` when nothing grows there).
    /// `painted` is the Nature brush's preset for this spot (0 = none, else index + 1) and
    /// `fallback` whether land without a biome grows nature (a populated world).
    #[allow(clippy::too_many_arguments)]
    pub fn preset_at(
        &self,
        t: &crate::terrain::Terrain,
        zones: &Zones,
        p: Vec2,
        seed: u32,
        painted: u8,
        fallback: bool,
    ) -> (u8, f32) {
        if painted > 0 && (painted as usize) <= self.presets.len() {
            return (painted - 1, DAB_COVERAGE);
        }
        let (biome, claim) = ecotone_biome(t, p, seed);
        if biome > 0 {
            let (list, density) = &self.biome_veg[(biome as usize).min(15)];
            if list.is_empty() {
                return (NONE, 0.0);
            }
            let n = (crate::biomes::patch_noise(p, seed) * 0.5 + 0.5).clamp(0.0, 0.999);
            let pick = list[((1.0 - n) * list.len() as f32) as usize];
            return (pick, density * POPULATE * DAB_COVERAGE * claim);
        }
        if !fallback {
            return (NONE, 0.0);
        }
        // the classic auto-populate rules
        let h = t.height_at(p.x, p.y);
        let alt = h - t.sea_level;
        if alt < 0.5 {
            return (NONE, 0.0);
        }
        let f = &self.fallback;
        let slope = t.normal_at(p.x, p.y).y.clamp(-1.0, 1.0).acos().to_degrees();
        let forest =
            crate::noise::fbm(p.x * 0.0022, p.y * 0.0022, 4, 2.0, 0.5, seed + 101) * 0.5 + 0.5;
        let wet =
            crate::noise::fbm(p.x * 0.0015, p.y * 0.0015, 3, 2.0, 0.5, seed + 202) * 0.5 + 0.5;
        let zone_biome = zones
            .by_id(t.zone_at(p.x, p.y))
            .filter(|z| z.biome_enabled)
            .map(|z| z.biome);
        let snow = t.style.snow_line;
        let preset = match zone_biome {
            Some(Biome::Tropical) => {
                if alt < 8.0 {
                    f.beach
                } else {
                    f.jungle
                }
            }
            Some(Biome::Desert) => f.desert,
            Some(Biome::Swamp) => f.swamp,
            Some(Biome::Alpine) => f.alpine,
            Some(Biome::Tundra) => f.tundra,
            Some(Biome::Boreal) => f.pine,
            Some(Biome::Autumn) => f.autumn,
            Some(Biome::Volcanic) => f.volcanic,
            _ => {
                if slope > 30.0 {
                    f.rocky
                } else if alt > snow * 0.7 {
                    f.alpine
                } else if alt < 5.0 {
                    if wet > 0.62 { f.swamp } else { f.beach }
                } else if forest > 0.55 {
                    if alt > snow * 0.4 {
                        f.pine
                    } else if wet > 0.6 {
                        f.autumn
                    } else {
                        f.mixed
                    }
                } else if forest > 0.45 {
                    f.sparse
                } else {
                    f.meadow
                }
            }
        };
        // beach palms only in warm-looking (low, sandy) spots; temperate coasts stay clean
        if preset == f.beach && zone_biome.is_none() && forest < 0.6 {
            return (NONE, 0.0);
        }
        let k = if preset == f.meadow { 0.5 } else { 1.0 };
        (preset, POPULATE * DAB_COVERAGE * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rules() -> Rules {
        Rules {
            presets: vec![],
            biome_veg: vec![],
            kind_entry: vec![],
            kind_variants: vec![],
            fallback: Fallback {
                mixed: 0,
                pine: 0,
                autumn: 0,
                sparse: 0,
                meadow: 0,
                alpine: 0,
                jungle: 0,
                beach: 0,
                desert: 0,
                swamp: 0,
                rocky: 0,
                tundra: 0,
                volcanic: 0,
            },
            art: Default::default(),
        }
    }

    #[test]
    fn legacy_jungle_omits_hidden_bamboo_without_changing_tree_total() {
        let jungle = presets()
            .into_iter()
            .find(|p| p.name == "Jungle")
            .expect("legacy Jungle preset");
        let trees: Vec<_> = jungle
            .layers
            .iter()
            .filter(|l| kind_category(l.kind) == Category::Trees)
            .filter(|l| {
                grows_in_rule_table(jungle.name, l.kind, crate::art_style::ArtStyle::Realistic)
            })
            .collect();
        assert!(!trees.iter().any(|l| l.kind == AssetKind::Bamboo));
        assert_eq!(trees.iter().map(|l| l.density).sum::<f32>(), 3.5);
    }

    #[test]
    fn every_temperate_broadleaf_is_reachable_in_each_layout() {
        let rules = test_rules();
        let expected = [
            AssetKind::Oak,
            AssetKind::Birch,
            AssetKind::Maple,
            AssetKind::GoldenMaple,
            AssetKind::SakuraPink,
            AssetKind::SakuraWhite,
            AssetKind::Ginkgo,
        ];
        for layout in ForestLayout::ALL {
            let mut seen = vec![];
            for z in 0..128 {
                for x in 0..128 {
                    let kind = rules.resolve_tree(
                        AssetKind::Oak,
                        Vec2::new(x as f32 * 17.0, z as f32 * 19.0),
                        7,
                        ForestDiversity {
                            enabled: true,
                            layout,
                        },
                        ForestEcology::Temperate,
                    );
                    if !seen.contains(&kind) {
                        seen.push(kind);
                    }
                }
            }
            for kind in expected {
                assert!(seen.contains(&kind), "{layout:?} never selected {kind:?}");
            }
        }
    }
}

/// The biome whose vegetation grows at texel `p`, and its density factor there (1 well inside
/// it). Near a border both sides claim the spot (`visuals::ecotone::biome_weights`, the same
/// wandering band the grass blends across): each texel picks one side by its claim, broken into
/// clumps and tongues (`ecotone::patchy`), and grows it as densely as
/// `ecotone::vegetation_density` says for that claim, so forests fray and intermingle into the
/// neighbouring biome instead of ending on the biome map's cell edges.
pub fn ecotone_biome(t: &crate::terrain::Terrain, p: Vec2, seed: u32) -> (u8, f32) {
    let (b, w) = ecotone_pick(t, p, seed);
    if w >= 1.0 {
        return (b, 1.0);
    }
    // picked with probability ~w: grows vegetation_density / w, so the expected density is
    // what vegetation_density says
    (
        b,
        crate::visuals::ecotone::vegetation_density(t, p, b, 0) / w.max(0.05),
    )
}

fn ecotone_pick(t: &crate::terrain::Terrain, p: Vec2, seed: u32) -> (u8, f32) {
    let ws = crate::visuals::ecotone::biome_weights(t, p, 0);
    if ws[1].1 <= 0.0 {
        return (ws[0].0, 1.0);
    }
    let h = crate::noise::hash_u32(
        p.x.to_bits() ^ p.y.to_bits().rotate_left(13) ^ seed.wrapping_mul(0x9E37_79B9),
    );
    let u = (h & 0xffff) as f32 / 65536.0;
    let lead = crate::visuals::ecotone::patchy(ws[0].1, p);
    if u < lead {
        return ws[0];
    }
    let rest: f32 = ws[1..].iter().map(|e| e.1).sum();
    let mut x = (u - lead) / (1.0 - lead).max(1e-6) * rest;
    for e in &ws[1..] {
        if x < e.1 {
            return *e;
        }
        x -= e.1;
    }
    ws[0]
}
