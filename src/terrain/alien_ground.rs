//! Ground palettes: which CC0 photoscan fills each paint layer, plus the automatic ground style
//! that goes with it. `Fantasy` is what every world starts with (green meadows, grey rock);
//! `SciFi` turns the whole ground into an alien desert canyonland: red dust and ochre sand on the
//! flats, red-brown layered sandstone on slopes and cliffs, basalt and dark grit as accents, and
//! only sparse patches of (crimson-recoloured, `nature::alien`) grass on red soil.
//!
//! It works by layer *slots*, not by painting: every rule, biome or brush that puts "grass" or
//! "forest floor" somewhere gets the palette's material there, so no green can show through
//! whatever the land or its biomes do later. Everything it sets is an ordinary world setting
//! (Paint tab › layer textures, Terrain › ground style, Nature › grass amount), saved with the
//! world and editable afterwards.
//!
//! MCP: `terrain_ground_palette`. Rust: [`ground`] (Sci-Fi) / [`apply`].

use bevy::prelude::*;

use super::layers::{LAYER_COUNT, Layer, Surface};
use super::textures::TerrainLayerChoices;
use super::{GridRect, Terrain};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundPalette {
    /// The default ground of a new world (green lawn, built-in layers).
    Fantasy,
    /// Alien desert canyonland: no green anywhere.
    SciFi,
}

impl GroundPalette {
    pub const ALL: [GroundPalette; 2] = [GroundPalette::Fantasy, GroundPalette::SciFi];

    pub fn id(self) -> &'static str {
        match self {
            GroundPalette::Fantasy => "fantasy",
            GroundPalette::SciFi => "scifi",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
            "fantasy" | "default" | "meadow" => Some(GroundPalette::Fantasy),
            "scifi" | "alien" | "desert" | "canyon" => Some(GroundPalette::SciFi),
            _ => None,
        }
    }
}

/// The Sci-Fi palette: library material per paint layer ("" keeps the built-in one). Snow, ice
/// and cobbles stay as they are (painted on purpose, they should still look like snow / roads).
pub const SCIFI_LAYERS: [(Layer, &str); 9] = [
    // where the auto rules or a biome put lush grass: ochre gravelly dust (the sparse crimson
    // grass grows here; a dark soil here made biome-painted worlds read murky)
    (Layer::Grass, "gravelly_desert_sand"),
    // the dry flats, most of the land: red desert dust
    (Layer::DryGrass, "red_desert_sand"),
    // under woods / forest biomes: red soil
    (Layer::ForestFloor, "red_dirt"),
    // moderate slopes: stony dry ground
    (Layer::Dirt, "dry_ground_rocks"),
    // damp hollows: a cracked dry clay pan
    (Layer::Mud, "cracked_clay"),
    // stream beds, sea floor: dark basalt grit
    (Layer::Gravel, "dark_gravel"),
    // cliffs and steep slopes: warm red-brown layered sandstone
    (Layer::Rock, "layered_sandstone_cliff"),
    // volcanic paint / biomes: basalt
    (Layer::DarkRock, "basalt"),
    // beaches and dunes: red sand
    (Layer::Sand, "red_desert_sand"),
];

/// Ground style of the Sci-Fi palette.
mod scifi_style {
    /// dry flats (red dust); the red-soil layer only where the dry noise dips
    pub const DRYNESS: f32 = 1.0;
    /// no damp, mossy gullies on a desert world
    pub const WET_GULLIES: f32 = 0.0;
    /// a little more bare rock on the slopes than a meadow world
    pub const ROCK_SLOPE: f32 = 0.3;
    /// no automatic snow caps, even on the alien canyonlands' ~285 m plateaus
    pub const SNOW_LINE: f32 = 2000.0;
    /// sparse grass world-wide (it grows in clumps: `visuals::ecotone::patchy`)
    pub const GRASS: f32 = 0.3;
    /// few photo grass tufts among it
    pub const TUFTS: f32 = 0.3;
}

/// Puts the Sci-Fi ground on the open world (see the module docs). Returns one line describing
/// what changed. Call after the land exists (any order with the canyons is fine: it sets slots
/// and rules, it paints nothing).
pub fn ground(world: &mut World) -> String {
    apply(world, GroundPalette::SciFi)
}

/// Puts a ground palette on the open world. `Fantasy` puts back exactly what a new world has.
pub fn apply(world: &mut World, palette: GroundPalette) -> String {
    // the slots
    let lib: Vec<String> = world
        .get_resource::<super::textures::TerrainLibrary>()
        .map(|l| l.materials.iter().map(|m| m.id.clone()).collect())
        .unwrap_or_default();
    let mut ids = TerrainLayerChoices::default();
    let mut missing = Vec::new();
    if palette == GroundPalette::SciFi {
        for (layer, id) in SCIFI_LAYERS {
            // a library without the material (stripped install) keeps the built-in layer
            if lib.is_empty() || lib.iter().any(|m| m == id) {
                ids.set(layer as usize, id);
            } else {
                missing.push(id);
            }
        }
    }
    let surfaces = surfaces_for(world, &ids);
    {
        let mut choices = world.resource_mut::<TerrainLayerChoices>();
        if *choices != ids {
            *choices = ids;
        }
    }
    if let Some(mut ed) = world.get_resource_mut::<crate::editor::Editor>() {
        ed.layer_surface = surfaces;
    }
    // the automatic ground style
    {
        let d = super::StyleRules::default();
        let mut t = world.resource_mut::<Terrain>();
        let st = &mut t.style;
        match palette {
            GroundPalette::SciFi => {
                st.dryness = scifi_style::DRYNESS;
                st.wet_gullies = scifi_style::WET_GULLIES;
                st.rock_slope = scifi_style::ROCK_SLOPE;
                st.snow_line = scifi_style::SNOW_LINE;
            }
            GroundPalette::Fantasy => {
                st.dryness = d.dryness;
                st.wet_gullies = d.wet_gullies;
                st.rock_slope = d.rock_slope;
                st.snow_line = d.snow_line;
            }
        }
        // grass density follows the final ground weights: rebuild it everywhere
        let n = t.n();
        t.dirty_grass = Some(GridRect {
            x0: 0,
            z0: 0,
            x1: n - 1,
            z1: n - 1,
        });
    }
    // grass amount and photo tufts
    let (grass, tufts) = match palette {
        GroundPalette::SciFi => (scifi_style::GRASS, scifi_style::TUFTS),
        GroundPalette::Fantasy => (1.0, crate::nature::appearance::GroundCover::default().tufts),
    };
    if let Some(mut a) = world.get_resource_mut::<crate::objects::NatureAmounts>() {
        a.grass = grass;
    }
    if let Some(mut a) = world.get_resource_mut::<crate::nature::NatureAppearance>() {
        a.ground.tufts = tufts;
    }
    world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    let mut line = match palette {
        GroundPalette::SciFi => {
            "alien desert ground: red dust and ochre sand, red-brown layered rock, basalt grit, sparse grass"
                .to_string()
        }
        GroundPalette::Fantasy => "the default meadow ground".to_string(),
    };
    if !missing.is_empty() {
        line.push_str(&format!(" (not in the library, kept built-in: {})", missing.join(", ")));
    }
    line
}

/// Footstep surfaces for the slots: a library material's own, else the layer's default.
fn surfaces_for(world: &World, ids: &TerrainLayerChoices) -> [Surface; LAYER_COUNT] {
    let lib = world.get_resource::<super::textures::TerrainLibrary>();
    std::array::from_fn(|i| {
        let id = ids.get(i);
        lib.and_then(|l| l.materials.iter().find(|m| !id.is_empty() && m.id == id))
            .and_then(|m| Surface::ALL.iter().find(|s| format!("{s:?}") == m.surface))
            .copied()
            .unwrap_or_else(|| Layer::from_index(i).default_surface())
    })
}

/// Which palette the world's slots match (None: a custom mix).
pub fn current(choices: &TerrainLayerChoices) -> Option<GroundPalette> {
    if *choices == TerrainLayerChoices::default() {
        return Some(GroundPalette::Fantasy);
    }
    SCIFI_LAYERS
        .iter()
        .all(|(l, id)| choices.get(*l as usize) == *id)
        .then_some(GroundPalette::SciFi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scifi_palette_has_no_green_and_exists_in_the_library() {
        let lib = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/textures/terrain_library/library.json"),
        )
        .unwrap();
        for (_, id) in SCIFI_LAYERS {
            assert!(lib.contains(&format!("\"{id}\"")), "{id} not in the library");
            assert!(
                !["lush_green_lawn", "green_meadow", "lush_grass_soil", "meadow_grass_twigs", "moss", "mossy_grass"]
                    .contains(&id),
                "{id} is green"
            );
        }
        // the lush default grass is gone from every slot
        let mut c = TerrainLayerChoices::default();
        for (l, id) in SCIFI_LAYERS {
            c.set(l as usize, id);
        }
        assert_eq!(current(&c), Some(GroundPalette::SciFi));
        assert!(!c.ids.iter().any(|s| s == super::super::textures::LUSH_GRASS));
        assert_eq!(current(&TerrainLayerChoices::default()), Some(GroundPalette::Fantasy));
    }

    #[test]
    fn palette_ids_round_trip() {
        for p in GroundPalette::ALL {
            assert_eq!(GroundPalette::from_id(p.id()), Some(p));
        }
        assert_eq!(GroundPalette::from_id("Sci-Fi"), Some(GroundPalette::SciFi));
    }
}
