//! Biomes: paint (or outline) an area and it becomes that biome in one go. The area turns into a
//! zone carrying the biome's weather, ambience, lighting mood and placement rules; its ground is
//! painted to match (steep slopes and snow stay automatic); its old vegetation is cleared and the
//! biome's plants grow back; lakes and rivers inside take the biome's water type. Footstep sounds
//! follow automatically from the painted ground.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Tab, ZoneTool};
use crate::grass::GrassStyle;
use crate::history::History;
use crate::noise::{fbm, smoothstep};
use crate::objects::ObjectStore;
use crate::terrain::brush::for_each_in_circle;
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::terrain::{GridRect, Terrain};
use crate::tools::Stroke;
use crate::water::{WaterBodies, WaterType};
use crate::world::{Biome, Mood, Soundscape, Weather, WorldMeta, Zone, Zones};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BiomePreset {
    #[default]
    TemperateForest,
    Meadow,
    Taiga,
    AutumnWoods,
    Jungle,
    TropicalCoast,
    Desert,
    Alpine,
    Tundra,
    Swamp,
    Volcanic,
    HauntedWoods,
}

/// Everything a biome brings with it.
pub struct BiomeDef {
    pub name: &'static str,
    pub blurb: &'static str,
    /// Zone colour (sRGB).
    pub color: [f32; 3],
    pub biome: Biome,
    pub weather: Weather,
    pub weather_intensity: f32,
    pub soundscape: Soundscape,
    pub mood: Mood,
    /// Ground layers: the first two alternate in large patches (forest floor under woods, grass in
    /// clearings…), the third is sprinkled in small patches with the given share.
    pub ground: [Layer; 3],
    pub accent_share: f32,
    /// Vegetation presets from densest to most open; large-scale noise picks between them, in step
    /// with the ground patches.
    pub vegetation: &'static [&'static str],
    pub density: f32,
    /// Water type for lakes and rivers inside the biome.
    pub water: Option<WaterType>,
    /// How the meadow grass grows here (none in deserts: shrubs and cacti instead).
    pub grass: GrassStyle,
    /// Typical wind (m/s), offered when the zone overrides the world's wind.
    pub wind: f32,
    /// Typical temperature (°C).
    pub temperature: f32,
}

impl BiomePreset {
    pub const ALL: [BiomePreset; 12] = [
        BiomePreset::TemperateForest,
        BiomePreset::Meadow,
        BiomePreset::Taiga,
        BiomePreset::AutumnWoods,
        BiomePreset::Jungle,
        BiomePreset::TropicalCoast,
        BiomePreset::Desert,
        BiomePreset::Alpine,
        BiomePreset::Tundra,
        BiomePreset::Swamp,
        BiomePreset::Volcanic,
        BiomePreset::HauntedWoods,
    ];

    /// Value stored in the terrain biome map (0 means no biome).
    pub fn id(self) -> u8 {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0) as u8 + 1
    }
    pub fn from_id(id: u8) -> Option<Self> {
        (id > 0)
            .then(|| Self::ALL.get(id as usize - 1).copied())
            .flatten()
    }

    /// Short label for tool tiles.
    pub fn short(self) -> &'static str {
        match self {
            BiomePreset::TemperateForest => "Forest",
            BiomePreset::Meadow => "Meadow",
            BiomePreset::Taiga => "Taiga",
            BiomePreset::AutumnWoods => "Autumn",
            BiomePreset::Jungle => "Jungle",
            BiomePreset::TropicalCoast => "Coast",
            BiomePreset::Desert => "Desert",
            BiomePreset::Alpine => "Alpine",
            BiomePreset::Tundra => "Tundra",
            BiomePreset::Swamp => "Swamp",
            BiomePreset::Volcanic => "Volcanic",
            BiomePreset::HauntedWoods => "Haunted",
        }
    }

    /// Whether rain gathering in gullies shows as damp, mossy ground (`Terrain::gully_wet`): not in
    /// the dry biomes, whose washes stay sand and ash.
    pub fn damp_gullies(self) -> bool {
        !matches!(self, BiomePreset::Desert | BiomePreset::Volcanic)
    }

    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as i;
        match self {
            BiomePreset::TemperateForest => i::TREE,
            BiomePreset::Meadow => i::FLOWER,
            BiomePreset::Taiga => i::TREE_EVERGREEN,
            BiomePreset::AutumnWoods => i::LEAF,
            BiomePreset::Jungle => i::PLANT,
            BiomePreset::TropicalCoast => i::TREE_PALM,
            BiomePreset::Desert => i::CACTUS,
            BiomePreset::Alpine => i::MOUNTAINS,
            BiomePreset::Tundra => i::SNOWFLAKE,
            BiomePreset::Swamp => i::DROP,
            BiomePreset::Volcanic => i::FIRE,
            BiomePreset::HauntedWoods => i::GHOST,
        }
    }

    pub fn def(self) -> BiomeDef {
        use Layer::*;
        match self {
            BiomePreset::TemperateForest => BiomeDef {
                name: "Temperate Forest",
                blurb: "Oak, birch and pine woods with grassy clearings, birdsong, natural light.",
                color: [0.36, 0.72, 0.36],
                biome: Biome::Temperate,
                weather: Weather::Clear,
                weather_intensity: 1.0,
                soundscape: Soundscape::Forest,
                mood: Mood::Natural,
                ground: [ForestFloor, Grass, Dirt],
                accent_share: 0.12,
                vegetation: &[
                    "Mixed Forest",
                    "Mixed Forest",
                    "Sparse Woodland",
                    "Flower Meadow",
                ],
                density: 0.9,
                water: Some(WaterType::ClearLake),
                grass: GrassStyle::DEFAULT,
                wind: 2.5,
                temperature: 15.0,
            },
            BiomePreset::Meadow => BiomeDef {
                name: "Flower Meadow",
                blurb: "Open grassland with wildflowers and lone trees, warm and bright.",
                color: [0.65, 0.85, 0.3],
                biome: Biome::Temperate,
                weather: Weather::Clear,
                weather_intensity: 1.0,
                soundscape: Soundscape::Meadow,
                mood: Mood::Vibrant,
                ground: [Grass, DryGrass, Dirt],
                accent_share: 0.06,
                vegetation: &["Flower Meadow", "Flower Meadow", "Sparse Woodland"],
                density: 0.8,
                water: Some(WaterType::ClearLake),
                grass: GrassStyle {
                    density: 1.0,
                    dry: 0.05,
                    tint: 0.6,
                    height: 1.15,
                },
                wind: 4.0,
                temperature: 18.0,
            },
            BiomePreset::Taiga => BiomeDef {
                name: "Pine Taiga",
                blurb: "Dense conifer forest, cool light, needles underfoot.",
                color: [0.3, 0.55, 0.45],
                biome: Biome::Boreal,
                weather: Weather::Cloudy,
                weather_intensity: 0.6,
                soundscape: Soundscape::Forest,
                mood: Mood::Cool,
                ground: [ForestFloor, Grass, Gravel],
                accent_share: 0.1,
                vegetation: &["Pine Forest", "Pine Forest", "Sparse Woodland"],
                density: 1.0,
                water: Some(WaterType::GlacialLake),
                grass: GrassStyle {
                    density: 0.75,
                    dry: 0.12,
                    tint: 0.08,
                    height: 0.75,
                },
                wind: 3.0,
                temperature: 4.0,
            },
            BiomePreset::AutumnWoods => BiomeDef {
                name: "Autumn Woods",
                blurb: "Red and gold maples over leaf litter in warm low sun.",
                color: [0.95, 0.55, 0.2],
                biome: Biome::Autumn,
                weather: Weather::Clear,
                weather_intensity: 1.0,
                soundscape: Soundscape::Forest,
                mood: Mood::Warm,
                ground: [ForestFloor, DryGrass, Dirt],
                accent_share: 0.12,
                vegetation: &["Autumn Woods", "Autumn Woods", "Sparse Woodland"],
                density: 0.9,
                water: Some(WaterType::ClearLake),
                grass: GrassStyle {
                    density: 0.9,
                    dry: 0.45,
                    tint: 0.75,
                    height: 0.95,
                },
                wind: 3.5,
                temperature: 10.0,
            },
            BiomePreset::Jungle => BiomeDef {
                name: "Tropical Jungle",
                blurb: "Lush jungle over muddy ground, warm rain, saturated greens.",
                color: [0.2, 0.7, 0.35],
                biome: Biome::Tropical,
                weather: Weather::Rain,
                weather_intensity: 0.45,
                soundscape: Soundscape::Marsh,
                mood: Mood::Vibrant,
                ground: [ForestFloor, Grass, Mud],
                accent_share: 0.15,
                vegetation: &["Jungle", "Jungle", "Jungle", "Sparse Woodland"],
                density: 1.1,
                water: Some(WaterType::MuddyRiver),
                grass: GrassStyle {
                    density: 1.0,
                    dry: 0.0,
                    tint: 0.9,
                    height: 1.35,
                },
                wind: 1.5,
                temperature: 28.0,
            },
            BiomePreset::TropicalCoast => BiomeDef {
                name: "Tropical Coast",
                blurb: "Palm-lined white sand and turquoise water, surf and sea breeze.",
                color: [0.3, 0.82, 0.9],
                biome: Biome::Tropical,
                weather: Weather::Clear,
                weather_intensity: 1.0,
                soundscape: Soundscape::Coast,
                mood: Mood::Warm,
                ground: [Sand, Grass, DryGrass],
                accent_share: 0.08,
                vegetation: &["Palm Beach", "Palm Beach", "Flower Meadow"],
                density: 0.8,
                water: Some(WaterType::TropicalSea),
                grass: GrassStyle {
                    density: 0.8,
                    dry: 0.1,
                    tint: 0.85,
                    height: 1.0,
                },
                wind: 6.0,
                temperature: 27.0,
            },
            BiomePreset::Desert => BiomeDef {
                name: "Desert",
                blurb: "Sand, gravel and cacti under a hard sun; dry wind.",
                color: [0.92, 0.75, 0.4],
                biome: Biome::Desert,
                weather: Weather::Clear,
                weather_intensity: 1.0,
                soundscape: Soundscape::Desert,
                mood: Mood::Warm,
                ground: [Sand, Gravel, DryGrass],
                accent_share: 0.08,
                vegetation: &["Desert"],
                density: 0.7,
                water: Some(WaterType::MuddyRiver),
                grass: GrassStyle::NONE,
                wind: 5.0,
                temperature: 34.0,
            },
            BiomePreset::Alpine => BiomeDef {
                name: "Alpine",
                blurb: "High meadows, scree and hardy pines; thin cool air and wind.",
                color: [0.55, 0.65, 0.75],
                biome: Biome::Alpine,
                weather: Weather::Cloudy,
                weather_intensity: 0.7,
                soundscape: Soundscape::Mountain,
                mood: Mood::Cool,
                ground: [Gravel, Grass, Rock],
                accent_share: 0.15,
                vegetation: &["Alpine", "Rocky Hills"],
                density: 0.7,
                water: Some(WaterType::GlacialLake),
                grass: GrassStyle {
                    density: 0.6,
                    dry: 0.12,
                    tint: 0.18,
                    height: 0.55,
                },
                wind: 7.0,
                temperature: 2.0,
            },
            BiomePreset::Tundra => BiomeDef {
                name: "Frozen Tundra",
                blurb: "Snowfields and ice, scattered frosted pines, falling snow.",
                color: [0.75, 0.88, 0.98],
                biome: Biome::Tundra,
                weather: Weather::Snow,
                weather_intensity: 0.7,
                soundscape: Soundscape::Frost,
                mood: Mood::Cool,
                ground: [Snow, Ice, Gravel],
                accent_share: 0.1,
                vegetation: &["Tundra"],
                density: 0.35,
                water: Some(WaterType::GlacialLake),
                grass: GrassStyle {
                    density: 0.3,
                    dry: 0.55,
                    tint: 0.1,
                    height: 0.45,
                },
                wind: 6.5,
                temperature: -8.0,
            },
            BiomePreset::Swamp => BiomeDef {
                name: "Swamp",
                blurb: "Mud, reeds and drowned trees in low fog; frogs and murky water.",
                color: [0.45, 0.55, 0.3],
                biome: Biome::Swamp,
                weather: Weather::Cloudy,
                weather_intensity: 0.42,
                soundscape: Soundscape::Marsh,
                mood: Mood::Gloomy,
                ground: [Mud, Grass, ForestFloor],
                accent_share: 0.15,
                vegetation: &["Swamp"],
                density: 0.9,
                water: Some(WaterType::Swamp),
                grass: GrassStyle {
                    density: 1.0,
                    dry: 0.08,
                    tint: 0.3,
                    height: 1.5,
                },
                wind: 1.5,
                temperature: 20.0,
            },
            BiomePreset::Volcanic => BiomeDef {
                name: "Volcanic Wastes",
                blurb: "Black rock and ash, smoky air, a red infernal glow.",
                color: [0.9, 0.3, 0.28],
                biome: Biome::Volcanic,
                weather: Weather::Cloudy,
                weather_intensity: 0.35,
                soundscape: Soundscape::Volcanic,
                mood: Mood::Infernal,
                ground: [DarkRock, Gravel, Dirt],
                accent_share: 0.12,
                vegetation: &["Volcanic", "Rocky Hills"],
                density: 0.35,
                water: Some(WaterType::MuddyRiver),
                grass: GrassStyle::NONE,
                wind: 4.0,
                temperature: 30.0,
            },
            BiomePreset::HauntedWoods => BiomeDef {
                name: "Haunted Woods",
                blurb: "Dark pines and dead trees in sickly green fog, eerie ambience.",
                color: [0.52, 0.42, 0.92],
                biome: Biome::Temperate,
                weather: Weather::Cloudy,
                weather_intensity: 0.55,
                soundscape: Soundscape::Eerie,
                mood: Mood::Eerie,
                ground: [ForestFloor, Mud, Dirt],
                accent_share: 0.15,
                vegetation: &["Haunted Woods", "Haunted Woods", "Swamp"],
                density: 0.85,
                water: Some(WaterType::Swamp),
                grass: GrassStyle {
                    density: 0.7,
                    dry: 0.4,
                    tint: 0.15,
                    height: 0.9,
                },
                wind: 2.0,
                temperature: 8.0,
            },
        }
    }
}

/// Large-scale patch noise shared by ground and vegetation, so woods sit on forest floor and
/// clearings on grass. Roughly −1..1.
pub(crate) fn patch_noise(p: Vec2, seed: u32) -> f32 {
    fbm(p.x * 0.004, p.y * 0.004, 4, 2.0, 0.5, seed + 701)
}

/// Splat target (0..255 per layer) for one spot of the biome; slopes, beaches and snow above the
/// snow line are left to the automatic styling.
pub(crate) fn ground_target(t: &Terrain, def: &BiomeDef, p: Vec2, seed: u32) -> [f32; LAYER_COUNT] {
    let mut out = [0.0; LAYER_COUNT];
    let h = t.height_at(p.x, p.y);
    let alt = h - t.sea_level;
    let slope = t.normal_at(p.x, p.y).y.clamp(-1.0, 1.0).acos().to_degrees();
    let steep = smoothstep(24.0, 38.0, slope);
    let mut keep = 1.0;
    let coastal = matches!(def.biome, Biome::Tropical | Biome::Swamp | Biome::Desert);
    if !coastal {
        keep *= smoothstep(0.5, t.style.beach_height + 1.0, alt);
    }
    if !matches!(def.biome, Biome::Tundra | Biome::Alpine) {
        keep *= 1.0 - smoothstep(t.style.snow_line - 30.0, t.style.snow_line + 20.0, alt);
    }
    let cover = keep * (1.0 - steep);
    // biomes without grass (deserts, volcanic wastes) never let the automatic ground (a green
    // meadow on gentle slopes) show through: their slopes are bare rock, their paint is whole
    let bare = def.grass.density <= 0.0;
    if bare {
        let rock = if def.ground.contains(&Layer::DarkRock) {
            Layer::DarkRock
        } else {
            Layer::Rock
        };
        out[rock as usize] += keep * steep * 255.0;
    }
    if cover <= 0.0 {
        return out;
    }
    let n = patch_noise(p, seed);
    let a = smoothstep(-0.18, 0.18, n);
    let accent = smoothstep(
        0.25,
        0.55,
        fbm(p.x * 0.03, p.y * 0.03, 3, 2.0, 0.5, seed + 707) * 0.5 + 0.5,
    ) * def.accent_share
        * 2.0;
    let accent = accent.min(0.6);
    let total = if bare { 1.0 } else { 0.92 } * cover * 255.0;
    out[def.ground[0] as usize] += a * (1.0 - accent) * total;
    out[def.ground[1] as usize] += (1.0 - a) * (1.0 - accent) * total;
    out[def.ground[2] as usize] += accent * total;
    out
}

/// Blend one splat vertex towards the target by `k` (0..1).
fn blend_splat(s: &mut [u8; LAYER_COUNT], target: &[f32; LAYER_COUNT], k: f32) {
    for (l, v) in s.iter_mut().enumerate() {
        let cur = *v as f32;
        let mut nv = cur + (target[l] - cur) * k;
        if (nv.round() - cur).abs() < 1.0 && (target[l] - cur).abs() >= 1.0 && k > 0.002 {
            nv = cur + (target[l] - cur).signum();
        }
        *v = nv.round().clamp(0.0, 255.0) as u8;
    }
    let sum: u32 = s.iter().map(|v| *v as u32).sum();
    if sum > 255 {
        let scale = 255.0 / sum as f32;
        for v in s.iter_mut() {
            *v = (*v as f32 * scale).floor() as u8;
        }
    }
}

/// Brush dab: paint the biome's ground with the brush falloff.
pub fn paint_ground_dab(
    t: &mut Terrain,
    h: &mut History,
    def: &BiomeDef,
    center: Vec2,
    radius: f32,
    falloff: f32,
    k: f32,
    seed: u32,
) {
    if let Some(r) = for_each_in_circle(t, h, center, radius, falloff, |t, idx, p, w| {
        let target = ground_target(t, def, p, seed);
        blend_splat(&mut t.splat[idx], &target, (k * w).min(1.0));
    }) {
        t.mark_splat_dirty(r);
    }
}

/// Paint the biome's ground over every vertex where `weight` > 0 (0..1, e.g. a feathered mask).
pub(crate) fn paint_ground_masked(
    t: &mut Terrain,
    h: &mut History,
    def: &BiomeDef,
    rect: GridRect,
    seed: u32,
    weight: impl Fn(&Terrain, usize, usize) -> f32,
) {
    h.touch_terrain(t, rect);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let w = weight(t, i, j);
            if w <= 0.0 {
                continue;
            }
            let p = t.grid_to_world(i, j);
            let target = ground_target(t, def, p, seed);
            let k = t.idx(i, j);
            blend_splat(&mut t.splat[k], &target, w);
        }
    }
    t.mark_splat_dirty(rect);
}

/// Zone fields that come from the biome.
pub fn apply_zone_props(z: &mut Zone, preset: BiomePreset) {
    let d = preset.def();
    z.biome_preset = Some(preset);
    z.biome_enabled = true;
    z.biome = d.biome;
    z.weather_enabled = true;
    z.weather = d.weather;
    z.weather_intensity = d.weather_intensity;
    z.audio_enabled = true;
    z.soundscape = d.soundscape;
    // the biome's mood replaces the zone's fine-tuned light; a chosen art style stays
    z.mood_enabled = d.mood != Mood::Natural || z.style.is_some();
    z.mood = d.mood;
    z.light = None;
    z.wind_speed = d.wind;
    z.temperature = d.temperature;
}

/// Lakes and rivers on the biome's ground take its water type (lava stays lava).
pub(crate) fn restyle_water(
    terrain: &Terrain,
    water: &mut WaterBodies,
    history: &mut History,
    def: &BiomeDef,
    biome: u8,
) -> usize {
    let Some(wt) = def.water else { return 0 };
    let in_zone = |x: f32, z: f32| terrain.biome_at(x, z) == biome;
    let lava = |s: &crate::water::WaterStyle| s.behaviour == crate::water::WaterBehaviour::Molten;
    let lakes: Vec<usize> = water
        .lakes
        .iter()
        .enumerate()
        .filter(|(_, l)| !lava(&l.style) && in_zone(l.seed[0], l.seed[1]))
        .map(|(i, _)| i)
        .collect();
    let rivers: Vec<usize> = water
        .rivers
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            !lava(&r.style)
                && !r.points.is_empty()
                && r.points.iter().filter(|p| in_zone(p[0], p[2])).count() * 2 > r.points.len()
        })
        .map(|(i, _)| i)
        .collect();
    if lakes.is_empty() && rivers.is_empty() {
        return 0;
    }
    history.touch_water(water);
    let style = wt.style();
    for i in &lakes {
        water.lakes[*i].style = style.clone();
    }
    let style = wt.river_style();
    for i in &rivers {
        let flow = water.rivers[*i].style.flow.max(style.flow);
        water.rivers[*i].style = style.clone();
        water.rivers[*i].style.flow = flow;
    }
    water.dirty = true;
    lakes.len() + rivers.len()
}

/// Request from the UI: make a whole existing zone a biome.
#[derive(Message, Clone, Debug)]
pub struct ApplyBiomeToZone {
    pub zone: u8,
    pub preset: BiomePreset,
}

pub struct BiomesPlugin;

impl Plugin for BiomesPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ApplyBiomeToZone>().add_systems(
            Update,
            (
                biome_tool.in_set(crate::tools::ToolSystems::Apply),
                apply_to_zone,
            ),
        );
    }
}

/// The biome brush (Zones tab): paints the biome map and its ground live, regrows vegetation on
/// release; in outline mode the drawn shape is filled on release. It never creates zones: those
/// are added by hand (a zone can take on a biome from its inspector).
#[allow(clippy::too_many_arguments)]
fn biome_tool(
    stroke: Res<Stroke>,
    mut ed: ResMut<Editor>,
    mut terrain: ResMut<Terrain>,
    mut history: ResMut<History>,
    mut water: ResMut<WaterBodies>,
    mut nature: ResMut<crate::nature::NatureField>,
    meta: Res<WorldMeta>,
    time: Res<Time>,
) {
    if ed.tab != Tab::Zones
        || ed.zone_tool != ZoneTool::Biome
        || !(stroke.active || stroke.just_ended)
    {
        return;
    }
    let preset = ed.biome_preset;
    let def = preset.def();
    let biome = preset.id();
    let seed = meta.seed;
    if stroke.just_started {
        history.begin(format!("Biome: {}", def.name));
    }

    if !ed.biome_outline && stroke.active {
        let Some(hit) = stroke.hit else { return };
        let b = ed.zone_brush;
        let from = stroke.last_hit.unwrap_or(hit);
        let seg = hit.xz().distance(from.xz());
        let dabs = ((seg / (b.size * 0.3).max(0.5)).ceil() as usize).clamp(1, 24);
        let dt = time.delta_secs().min(0.05);
        for d in 0..dabs {
            let c = from.xz().lerp(hit.xz(), (d + 1) as f32 / dabs as f32);
            if let Some(r) = for_each_in_circle(
                &mut terrain,
                &mut history,
                c,
                b.size,
                0.0,
                |t, idx, _p, w| {
                    if w > 0.0 {
                        t.biomes[idx] = biome;
                    }
                },
            ) {
                terrain.mark_biome_dirty(r);
            }
            paint_ground_dab(
                &mut terrain,
                &mut history,
                &def,
                c,
                b.size * 1.1,
                b.falloff.max(0.35),
                (dt / dabs as f32 * 7.0).min(1.0),
                seed,
            );
        }
    }

    if !stroke.just_ended {
        return;
    }
    let path: Vec<Vec2> = stroke.path.iter().map(|p| p.xz()).collect();
    if path.is_empty() {
        return;
    }
    if ed.biome_outline {
        if path.len() < 8 {
            ed.toast("Draw a closed outline around the area");
            return;
        }
        let step = (path.len() / 200).max(1);
        let poly: Vec<Vec2> = path.iter().step_by(step).copied().collect();
        // the drawn line becomes a natural, wandering border
        let shape = crate::visuals::ecotone::OrganicOutline::new(&poly, seed);
        let (lo, hi) = shape.bounds();
        let Some(rect) = terrain.rect_for_circle((lo + hi) * 0.5, ((hi - lo) * 0.5).max_element())
        else {
            return;
        };
        history.touch_terrain(&terrain, rect);
        for j in rect.z0..=rect.z1 {
            for i in rect.x0..=rect.x1 {
                if shape.inside(terrain.grid_to_world(i, j)) {
                    let k = terrain.idx(i, j);
                    terrain.biomes[k] = biome;
                }
            }
        }
        terrain.mark_biome_dirty(rect);
        // the ground feathers in across the border
        paint_ground_masked(&mut terrain, &mut history, &def, rect, seed, |t, i, j| {
            shape.ground_weight(t.grid_to_world(i, j))
        });
        // the biome's own nature grows here: painted nature edits inside are dropped
        crate::nature::regrow_area(
            &mut nature,
            &mut history,
            lo,
            hi,
            |p: Vec2| shape.inside(p),
            1.0,
        );
    } else {
        let r = ed.zone_brush.size;
        let (lo, hi) = path.iter().fold(
            (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)),
            |(a, b), p| (a.min(*p - r), b.max(*p + r)),
        );
        let samples: Vec<Vec2> = path
            .iter()
            .step_by((path.len() / 400).max(1))
            .copied()
            .collect();
        crate::nature::regrow_area(
            &mut nature,
            &mut history,
            lo,
            hi,
            |p: Vec2| samples.iter().any(|s| s.distance_squared(p) <= r * r),
            1.0,
        );
    }
    let restyled = restyle_water(&terrain, &mut water, &mut history, &def, biome);
    let mut msg = format!("{}: its forests, plants and rocks grow here", def.name);
    if restyled > 0 {
        msg += &format!(", {restyled} water bodies restyled");
    }
    ed.toast(msg);
    // the history entry is committed by tools::commit_history at the end of the stroke
}

/// "Make this whole zone a biome" from the zone inspector.
#[allow(clippy::too_many_arguments)]
fn apply_to_zone(
    mut msgs: MessageReader<ApplyBiomeToZone>,
    mut ed: ResMut<Editor>,
    mut terrain: ResMut<Terrain>,
    mut history: ResMut<History>,
    mut zones: ResMut<Zones>,
    store: Res<ObjectStore>,
    mut water: ResMut<WaterBodies>,
    mut nature: ResMut<crate::nature::NatureField>,
    mut meta: ResMut<WorldMeta>,
) {
    for m in msgs.read() {
        info!("apply biome {:?} to zone {}", m.preset, m.zone);
        let def = m.preset.def();
        let Some(zi) = zones.list.iter().position(|z| z.id == m.zone) else {
            continue;
        };
        history.begin(format!("Biome: {}", def.name));
        history.touch_zones(&zones);
        apply_zone_props(&mut zones.list[zi], m.preset);
        // the zone's extent
        let n = terrain.n();
        let (mut x0, mut z0, mut x1, mut z1) = (usize::MAX, usize::MAX, 0, 0);
        for j in 0..n {
            for i in 0..n {
                if terrain.zones[terrain.idx(i, j)] == m.zone {
                    x0 = x0.min(i);
                    z0 = z0.min(j);
                    x1 = x1.max(i);
                    z1 = z1.max(j);
                }
            }
        }
        if x0 == usize::MAX {
            history.commit(&store);
            ed.toast("Paint the zone's area first, then apply the biome");
            continue;
        }
        let rect = GridRect { x0, z0, x1, z1 };
        let zid = m.zone;
        let biome = m.preset.id();
        history.touch_terrain(&terrain, rect);
        for j in z0..=z1 {
            for i in x0..=x1 {
                let k = terrain.idx(i, j);
                if terrain.zones[k] == zid {
                    terrain.biomes[k] = biome;
                }
            }
        }
        terrain.mark_biome_dirty(rect);
        // feather the ground over ~3 cells inside the zone border
        paint_ground_masked(
            &mut terrain,
            &mut history,
            &def,
            rect,
            meta.seed,
            |t, i, j| {
                if t.zones[t.idx(i, j)] != zid {
                    return 0.0;
                }
                let mut edge = 4;
                for d in 1..=3usize {
                    let nb = [
                        (i.saturating_sub(d), j),
                        ((i + d).min(n), j),
                        (i, j.saturating_sub(d)),
                        (i, (j + d).min(n)),
                    ];
                    if nb.iter().any(|(a, b)| t.zones[t.idx(*a, *b)] != zid) {
                        edge = d;
                        break;
                    }
                }
                edge as f32 / 4.0
            },
        );
        let lo = terrain.grid_to_world(x0, z0);
        let hi = terrain.grid_to_world(x1, z1);
        let in_zone = |p: Vec2| terrain.zone_at(p.x, p.y) == zid;
        crate::nature::regrow_area(&mut nature, &mut history, lo, hi, in_zone, 1.0);
        let restyled = restyle_water(&terrain, &mut water, &mut history, &def, biome);
        history.commit(&store);
        meta.dirty = true;
        let name = zones.list[zi].name.clone();
        info!(
            "biome {} applied to zone {name}: {restyled} water bodies",
            def.name
        );
        let mut msg = format!(
            "{name} is now {}: its forests, plants and rocks grow there",
            def.name
        );
        if restyled > 0 {
            msg += &format!(", {restyled} water bodies restyled");
        }
        ed.toast(msg);
    }
}

pub(crate) fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(crate) fn distance_to_polyline(p: Vec2, poly: &[Vec2]) -> f32 {
    let mut best = f32::MAX;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        best = best.min(p.distance(a + ab * t));
    }
    best
}
