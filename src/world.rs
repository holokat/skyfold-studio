//! Serializable world-level definitions: zones and their rules (weather, ambience, biome,
//! level range), plus the enums shared by lighting, audio and placement.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Weather {
    #[default]
    Clear,
    Cloudy,
    Rain,
    Storm,
    Snow,
    Fog,
    Sandstorm,
}

impl Weather {
    pub const ALL: [Weather; 7] = [
        Weather::Clear,
        Weather::Cloudy,
        Weather::Rain,
        Weather::Storm,
        Weather::Snow,
        Weather::Fog,
        Weather::Sandstorm,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Weather::Clear => "Clear",
            Weather::Cloudy => "Cloudy",
            Weather::Rain => "Rain",
            Weather::Storm => "Thunderstorm",
            Weather::Snow => "Snowfall",
            Weather::Fog => "Fog",
            Weather::Sandstorm => "Sandstorm",
        }
    }
    /// (cloud coverage, fog density, wetness, rain, snow, sun dimming, dust)
    pub fn params(self) -> WeatherParams {
        match self {
            Weather::Clear => WeatherParams {
                clouds: 0.22,
                fog: 0.0,
                wet: 0.0,
                rain: 0.0,
                snow: 0.0,
                dim: 0.0,
                dust: 0.0,
            },
            Weather::Cloudy => WeatherParams {
                clouds: 0.62,
                fog: 0.15,
                wet: 0.0,
                rain: 0.0,
                snow: 0.0,
                dim: 0.35,
                dust: 0.0,
            },
            Weather::Rain => WeatherParams {
                clouds: 0.85,
                fog: 0.35,
                wet: 0.85,
                rain: 0.7,
                snow: 0.0,
                dim: 0.62,
                dust: 0.0,
            },
            Weather::Storm => WeatherParams {
                clouds: 0.97,
                fog: 0.5,
                wet: 1.0,
                rain: 1.0,
                snow: 0.0,
                dim: 0.8,
                dust: 0.0,
            },
            Weather::Snow => WeatherParams {
                clouds: 0.8,
                fog: 0.45,
                wet: 0.0,
                rain: 0.0,
                snow: 0.8,
                dim: 0.5,
                dust: 0.0,
            },
            Weather::Fog => WeatherParams {
                clouds: 0.5,
                fog: 1.0,
                wet: 0.25,
                rain: 0.0,
                snow: 0.0,
                dim: 0.45,
                dust: 0.0,
            },
            Weather::Sandstorm => WeatherParams {
                clouds: 0.3,
                fog: 0.8,
                wet: 0.0,
                rain: 0.0,
                snow: 0.0,
                dim: 0.5,
                dust: 1.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherParams {
    pub clouds: f32,
    pub fog: f32,
    pub wet: f32,
    pub rain: f32,
    pub snow: f32,
    pub dim: f32,
    pub dust: f32,
}

impl WeatherParams {
    pub fn lerp(self, o: WeatherParams, t: f32) -> WeatherParams {
        let l = |a: f32, b: f32| a + (b - a) * t;
        WeatherParams {
            clouds: l(self.clouds, o.clouds),
            fog: l(self.fog, o.fog),
            wet: l(self.wet, o.wet),
            rain: l(self.rain, o.rain),
            snow: l(self.snow, o.snow),
            dim: l(self.dim, o.dim),
            dust: l(self.dust, o.dust),
        }
    }
    pub fn scaled(self, intensity: f32) -> WeatherParams {
        Weather::Clear
            .params()
            .lerp(self, intensity.clamp(0.0, 1.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Soundscape {
    None,
    #[default]
    Forest,
    Meadow,
    Mountain,
    Coast,
    Marsh,
    Desert,
    Cave,
    Frost,
    Town,
    Ruins,
    Volcanic,
    Eerie,
    Sunken,
    Infernal,
}

impl Soundscape {
    pub const ALL: [Soundscape; 15] = [
        Soundscape::None,
        Soundscape::Forest,
        Soundscape::Meadow,
        Soundscape::Mountain,
        Soundscape::Coast,
        Soundscape::Marsh,
        Soundscape::Desert,
        Soundscape::Cave,
        Soundscape::Frost,
        Soundscape::Town,
        Soundscape::Ruins,
        Soundscape::Volcanic,
        Soundscape::Eerie,
        Soundscape::Sunken,
        Soundscape::Infernal,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Soundscape::None => "Silence",
            Soundscape::Forest => "Forest",
            Soundscape::Meadow => "Meadow & Plains",
            Soundscape::Mountain => "Mountain Wind",
            Soundscape::Coast => "Coast & Sea",
            Soundscape::Marsh => "Marsh",
            Soundscape::Desert => "Desert Wind",
            Soundscape::Cave => "Cave",
            Soundscape::Frost => "Frozen Wastes",
            Soundscape::Town => "Town",
            Soundscape::Ruins => "Haunted Ruins",
            Soundscape::Volcanic => "Volcanic",
            Soundscape::Eerie => "Void / Eerie",
            Soundscape::Sunken => "Sunken",
            Soundscape::Infernal => "Infernal",
        }
    }
    /// Layered loops (file stem in assets/audio/ambience, relative volume) for day and night. The
    /// wind isn't one of them: it follows the wind actually blowing, see `wind_exposure`.
    pub fn layers(self, night: bool) -> Vec<(&'static str, f32)> {
        match self {
            Soundscape::None | Soundscape::Mountain | Soundscape::Desert => vec![],
            Soundscape::Forest if night => vec![
                ("amb_forest_0", 0.55),
                ("amb_near_crickets_0", 0.5),
                ("amb_near_owl_0", 0.35),
            ],
            Soundscape::Forest => vec![("amb_forest_0", 0.8), ("amb_near_songbirds_0", 0.45)],
            Soundscape::Meadow if night => vec![("amb_near_crickets_0", 0.6)],
            Soundscape::Meadow => vec![("amb_near_songbirds_0", 0.5), ("amb_near_bees_0", 0.25)],
            Soundscape::Coast => vec![
                ("amb_near_open_sea_0", 0.85),
                ("amb_near_gulls_0", if night { 0.0 } else { 0.4 }),
            ],
            Soundscape::Marsh => vec![
                ("amb_near_marsh_0", 0.75),
                ("amb_near_pond_0", 0.45),
                ("amb_near_flies_0", 0.2),
            ],
            Soundscape::Cave => vec![("amb_cave_0", 0.85), ("amb_near_drips_0", 0.45)],
            Soundscape::Frost => vec![("amb_frost_0", 0.85)],
            Soundscape::Town => vec![
                ("amb_town_0", 0.7),
                ("amb_near_market_0", if night { 0.0 } else { 0.4 }),
            ],
            Soundscape::Ruins => vec![("amb_veil_0", 0.7), ("amb_near_crows_0", 0.35)],
            Soundscape::Volcanic => vec![("amb_ashen_0", 0.7), ("amb_fire_0", 0.35)],
            Soundscape::Eerie => vec![("amb_void_0", 0.8)],
            Soundscape::Sunken => vec![("amb_sunken_0", 0.75), ("amb_water_0", 0.35)],
            Soundscape::Infernal => vec![("amb_hell_0", 0.8)],
        }
    }
    /// How much of the wind is heard here (audio.rs plays the wind and gust loops from the wind
    /// actually blowing): a factor on the wind speed — ridges and dunes roar in a breeze that only
    /// whispers across a meadow, woods and streets are sheltered, caves hear none — and how much
    /// of it comes in gusts even when it's light.
    pub fn wind_exposure(self) -> (f32, f32) {
        match self {
            Soundscape::None | Soundscape::Cave => (0.0, 0.0),
            Soundscape::Mountain => (2.4, 0.6),
            Soundscape::Desert => (1.5, 0.6),
            Soundscape::Frost => (1.0, 0.8),
            Soundscape::Meadow => (1.1, 0.0),
            Soundscape::Coast | Soundscape::Ruins | Soundscape::Marsh | Soundscape::Volcanic => {
                (0.7, 0.0)
            }
            Soundscape::Town | Soundscape::Eerie => (0.5, 0.0),
            Soundscape::Forest => (0.45, 0.0),
            Soundscape::Sunken | Soundscape::Infernal => (0.3, 0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Biome {
    #[default]
    Temperate,
    Boreal,
    Autumn,
    Tropical,
    Desert,
    Alpine,
    Tundra,
    Swamp,
    Volcanic,
}

impl Biome {
    pub const ALL: [Biome; 9] = [
        Biome::Temperate,
        Biome::Boreal,
        Biome::Autumn,
        Biome::Tropical,
        Biome::Desert,
        Biome::Alpine,
        Biome::Tundra,
        Biome::Swamp,
        Biome::Volcanic,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Biome::Temperate => "Temperate",
            Biome::Boreal => "Boreal / Taiga",
            Biome::Autumn => "Autumn Woods",
            Biome::Tropical => "Tropical",
            Biome::Desert => "Desert",
            Biome::Alpine => "Alpine",
            Biome::Tundra => "Tundra",
            Biome::Swamp => "Swamp",
            Biome::Volcanic => "Volcanic",
        }
    }
}

/// A zone's lighting mood: a preset for its [`ZoneLight`] (biomes use them too).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Mood {
    #[default]
    Natural,
    Warm,
    Cool,
    Vibrant,
    Gloomy,
    Eerie,
    Infernal,
    Bright,
    Misty,
    Dark,
}

impl Mood {
    pub const ALL: [Mood; 10] = [
        Mood::Natural,
        Mood::Bright,
        Mood::Vibrant,
        Mood::Warm,
        Mood::Cool,
        Mood::Misty,
        Mood::Gloomy,
        Mood::Dark,
        Mood::Eerie,
        Mood::Infernal,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Mood::Natural => "Natural",
            Mood::Bright => "Bright & Airy",
            Mood::Vibrant => "Vibrant & Alive",
            Mood::Warm => "Warm & Golden",
            Mood::Cool => "Cool & Crisp",
            Mood::Misty => "Misty",
            Mood::Gloomy => "Gloomy",
            Mood::Dark => "Dark & Foreboding",
            Mood::Eerie => "Eerie Green",
            Mood::Infernal => "Infernal Red",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Mood::Natural => "The world's own light.",
            Mood::Bright => "Lighter and softer: open, safe, welcoming (towns, sanctuaries).",
            Mood::Vibrant => {
                "Richer colour, a little glow and a stronger sun: the land feels alive."
            }
            Mood::Warm => "Golden light and warm air.",
            Mood::Cool => "Cooler, crisper light.",
            Mood::Misty => "Soft, pale haze hangs in the air; distances fade.",
            Mood::Gloomy => "Grey, drained colour, a weaker sun and a little murk.",
            Mood::Dark => "Much darker, deep shadows and dim corners: a place to fear.",
            Mood::Eerie => "Sickly green light and haze.",
            Mood::Infernal => "Red-hot light, smoky air.",
        }
    }
    /// The light the mood gives a zone.
    pub fn light(self) -> ZoneLight {
        let n = ZoneLight::default();
        match self {
            Mood::Natural => n,
            Mood::Bright => ZoneLight {
                brightness: 0.35,
                contrast: 0.96,
                saturation: 1.06,
                warmth: 0.08,
                shadow_fill: 1.3,
                glow: 0.08,
                fog_tint: [1.04, 1.04, 1.04],
                ..n
            },
            Mood::Vibrant => ZoneLight {
                brightness: 0.1,
                contrast: 1.06,
                saturation: 1.25,
                warmth: 0.1,
                sun: 1.1,
                glow: 0.1,
                ..n
            },
            Mood::Warm => ZoneLight {
                warmth: 0.45,
                tint: 0.05,
                saturation: 1.08,
                contrast: 1.02,
                sun: 1.05,
                fog_tint: [1.1, 0.98, 0.85],
                ..n
            },
            Mood::Cool => ZoneLight {
                warmth: -0.35,
                saturation: 0.95,
                contrast: 1.03,
                fog_tint: [0.96, 0.99, 1.04],
                ..n
            },
            Mood::Misty => ZoneLight {
                brightness: 0.05,
                contrast: 0.9,
                saturation: 0.85,
                warmth: -0.1,
                sun: 0.85,
                shadow_fill: 1.2,
                fog: 0.45,
                fog_tint: [1.02, 1.03, 1.05],
                glow: 0.12,
                ..n
            },
            Mood::Gloomy => ZoneLight {
                brightness: -0.25,
                warmth: -0.25,
                saturation: 0.65,
                contrast: 0.94,
                sun: 0.75,
                fog: 0.12,
                fog_tint: [0.8, 0.82, 0.85],
                ..n
            },
            Mood::Dark => ZoneLight {
                brightness: -0.9,
                contrast: 1.12,
                saturation: 0.8,
                warmth: -0.15,
                sun: 0.6,
                shadow_fill: 0.6,
                fog: 0.1,
                fog_tint: [0.55, 0.6, 0.7],
                vignette: 0.35,
                ..n
            },
            Mood::Eerie => ZoneLight {
                brightness: -0.2,
                warmth: -0.2,
                tint: -0.9,
                saturation: 0.75,
                contrast: 1.05,
                fog: 0.15,
                fog_tint: [0.7, 0.95, 0.75],
                vignette: 0.2,
                ..n
            },
            Mood::Infernal => ZoneLight {
                brightness: -0.15,
                warmth: 1.0,
                tint: 0.3,
                saturation: 1.1,
                contrast: 1.1,
                fog: 0.1,
                fog_tint: [1.3, 0.6, 0.45],
                glow: 0.15,
                vignette: 0.25,
                ..n
            },
        }
    }
}

/// How a zone changes the picture while the viewer is inside it. It stacks on the world's art
/// style (offsets add, multipliers multiply); the default changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneLight {
    /// Exposure offset in stops: − darker, + brighter.
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// Warm (+) / cool (−) along the daylight curve, −1..1 (see `sky::white_balance`).
    pub warmth: f32,
    /// Magenta (+) / green (−), −1..1.
    pub tint: f32,
    /// Sunlight multiplier.
    pub sun: f32,
    /// Sky (ambient) light multiplier: < 1 deepens the shadows, > 1 softens them.
    pub shadow_fill: f32,
    /// Extra haze and fog, 0..1 (1 is about as thick as Fog weather).
    pub fog: f32,
    /// Fog colour multiplier (1 = the sky's own colour).
    pub fog_tint: [f32; 3],
    /// Added glow (bloom), 0..1.
    pub glow: f32,
    /// Darkened corners, 0..1 (the stronger of this and the art style's).
    pub vignette: f32,
}

impl Default for ZoneLight {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            warmth: 0.0,
            tint: 0.0,
            sun: 1.0,
            shadow_fill: 1.0,
            fog: 0.0,
            fog_tint: [1.0; 3],
            glow: 0.0,
            vignette: 0.0,
        }
    }
}

impl ZoneLight {
    pub fn lerp(&self, o: &ZoneLight, t: f32) -> ZoneLight {
        let l = |a: f32, b: f32| a + (b - a) * t;
        ZoneLight {
            brightness: l(self.brightness, o.brightness),
            contrast: l(self.contrast, o.contrast),
            saturation: l(self.saturation, o.saturation),
            warmth: l(self.warmth, o.warmth),
            tint: l(self.tint, o.tint),
            sun: l(self.sun, o.sun),
            shadow_fill: l(self.shadow_fill, o.shadow_fill),
            fog: l(self.fog, o.fog),
            fog_tint: [
                l(self.fog_tint[0], o.fog_tint[0]),
                l(self.fog_tint[1], o.fog_tint[1]),
                l(self.fog_tint[2], o.fog_tint[2]),
            ],
            glow: l(self.glow, o.glow),
            vignette: l(self.vignette, o.vignette),
        }
    }
    /// Within the ranges the editor's sliders allow.
    pub fn clamped(mut self) -> ZoneLight {
        self.brightness = self.brightness.clamp(-3.0, 2.0);
        self.contrast = self.contrast.clamp(0.6, 1.5);
        self.saturation = self.saturation.clamp(0.0, 2.0);
        self.warmth = self.warmth.clamp(-1.0, 1.0);
        self.tint = self.tint.clamp(-1.0, 1.0);
        self.sun = self.sun.clamp(0.0, 2.0);
        self.shadow_fill = self.shadow_fill.clamp(0.2, 2.5);
        self.fog = self.fog.clamp(0.0, 1.0);
        self.fog_tint = self.fog_tint.map(|c| c.clamp(0.0, 1.5));
        self.glow = self.glow.clamp(0.0, 1.0);
        self.vignette = self.vignette.clamp(0.0, 1.0);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ZoneType {
    #[default]
    OpenWorld,
    Sanctuary,
    Contested,
    City,
    Dungeon,
    Raid,
}

impl ZoneType {
    pub const ALL: [ZoneType; 6] = [
        ZoneType::OpenWorld,
        ZoneType::Sanctuary,
        ZoneType::Contested,
        ZoneType::City,
        ZoneType::Dungeon,
        ZoneType::Raid,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ZoneType::OpenWorld => "Open World (PvE)",
            ZoneType::Sanctuary => "Sanctuary (no combat)",
            ZoneType::Contested => "Contested (PvP)",
            ZoneType::City => "Capital City",
            ZoneType::Dungeon => "Dungeon Entrance",
            ZoneType::Raid => "Raid Area",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    /// Value stored in the terrain zone map (1..=63).
    pub id: u8,
    pub name: String,
    /// sRGB 0..1
    pub color: [f32; 3],
    pub level_min: u32,
    pub level_max: u32,
    pub description: String,
    pub zone_type: ZoneType,
    pub visible: bool,
    pub weather_enabled: bool,
    pub weather: Weather,
    pub weather_intensity: f32,
    pub audio_enabled: bool,
    pub soundscape: Soundscape,
    pub audio_volume: f32,
    pub biome_enabled: bool,
    pub biome: Biome,
    /// The zone has its own lighting & style (mood, art style, fine-tuned light).
    pub mood_enabled: bool,
    /// Forest diversity is independent from this zone's lighting mood. `None` inherits world.
    #[serde(default)]
    pub forest: Option<crate::nature::ForestDiversity>,
    pub mood: Mood,
    /// Objects can't be placed inside this zone by brushes.
    pub limit_placement: bool,
    pub allow_mounts: bool,
    pub allow_flying: bool,
    pub rested_area: bool,
    /// The biome this zone was painted as (biome brush), if any.
    #[serde(default)]
    pub biome_preset: Option<crate::biomes::BiomePreset>,
    /// Override the world's wind speed inside this zone.
    #[serde(default)]
    pub wind_enabled: bool,
    /// m/s
    #[serde(default = "default_zone_wind")]
    pub wind_speed: f32,
    /// Typical temperature (°C) for gameplay: shown in the editor and exported.
    #[serde(default = "default_zone_temperature")]
    pub temperature: f32,
    /// Made by world generation (replaced when the world is generated again).
    #[serde(default)]
    pub generated: bool,
    /// Override the world's cloud cover inside this zone.
    #[serde(default)]
    pub clouds_enabled: bool,
    /// 0..1
    #[serde(default = "default_zone_clouds")]
    pub cloud_cover: f32,
    /// Outdoor haze override, independent of this zone's lighting mood and art style.
    #[serde(default)]
    pub haze: crate::haze::ZoneHaze,
    /// Fine-tuned light (`None`: the mood as designed).
    #[serde(default)]
    pub light: Option<ZoneLight>,
    /// The art style the zone switches to (`None`: the world's own).
    #[serde(default)]
    pub style: Option<crate::look::LookPreset>,
    /// How fully the style replaces the world's, 0..1.
    #[serde(default = "default_zone_style_strength")]
    pub style_strength: f32,
}

fn default_zone_style_strength() -> f32 {
    1.0
}

fn default_zone_clouds() -> f32 {
    0.5
}

fn default_zone_wind() -> f32 {
    3.0
}

fn default_zone_temperature() -> f32 {
    16.0
}

pub const ZONE_PALETTE: [[f32; 3]; 12] = [
    [0.98, 0.78, 0.18],
    [0.36, 0.72, 0.36],
    [0.25, 0.55, 0.95],
    [0.45, 0.80, 0.32],
    [0.95, 0.55, 0.20],
    [0.30, 0.82, 0.90],
    [0.52, 0.42, 0.92],
    [0.90, 0.30, 0.28],
    [0.92, 0.45, 0.70],
    [0.65, 0.85, 0.30],
    [0.80, 0.65, 0.45],
    [0.55, 0.65, 0.75],
];

impl Zone {
    pub fn new(id: u8, name: String) -> Self {
        Self {
            id,
            name,
            color: ZONE_PALETTE[(id as usize - 1) % ZONE_PALETTE.len()],
            level_min: 1,
            level_max: 5,
            description: String::new(),
            zone_type: ZoneType::OpenWorld,
            visible: true,
            weather_enabled: false,
            weather: Weather::Clear,
            weather_intensity: 1.0,
            audio_enabled: true,
            soundscape: Soundscape::Forest,
            audio_volume: 0.8,
            biome_enabled: false,
            biome: Biome::Temperate,
            mood_enabled: false,
            forest: None,
            mood: Mood::Natural,
            limit_placement: false,
            allow_mounts: true,
            allow_flying: false,
            rested_area: false,
            biome_preset: None,
            wind_enabled: false,
            wind_speed: 3.0,
            temperature: 16.0,
            generated: false,
            clouds_enabled: false,
            cloud_cover: 0.5,
            haze: Default::default(),
            light: None,
            style: None,
            style_strength: 1.0,
        }
    }
    /// The zone's light (`None`: it leaves the world's light alone).
    pub fn own_light(&self) -> Option<ZoneLight> {
        self.mood_enabled
            .then(|| self.light.unwrap_or_else(|| self.mood.light()))
    }
    /// The art style the zone switches to, and how fully.
    pub fn own_style(&self) -> Option<(crate::look::LookPreset, f32)> {
        self.style
            .filter(|_| self.mood_enabled)
            .map(|p| (p, self.style_strength.clamp(0.0, 1.0)))
    }
    pub fn level_label(&self) -> String {
        if self.level_max >= 60 && self.level_min >= self.level_max {
            format!("Lv. {}+", self.level_min)
        } else if self.level_min == self.level_max {
            format!("Lv. {}", self.level_min)
        } else {
            format!("Lv. {}–{}", self.level_min, self.level_max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_zones_default_to_haze_inheritance() {
        let mut saved = serde_json::to_value(Zone::new(1, "Old zone".into())).unwrap();
        saved.as_object_mut().unwrap().remove("haze");
        let loaded: Zone = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.haze, crate::haze::ZoneHaze::default());
    }

    #[test]
    fn older_zones_inherit_forest_diversity() {
        let mut saved = serde_json::to_value(Zone::new(1, "Old zone".into())).unwrap();
        saved.as_object_mut().unwrap().remove("forest");
        let loaded: Zone = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.forest, None);
    }
}

#[derive(Resource, Default, Clone)]
pub struct Zones {
    pub list: Vec<Zone>,
    pub selected: Option<usize>,
}

impl Zones {
    pub fn by_id(&self, id: u8) -> Option<&Zone> {
        if id == 0 {
            return None;
        }
        self.list.iter().find(|z| z.id == id)
    }
    pub fn next_id(&self) -> Option<u8> {
        (1u8..=63).find(|id| !self.list.iter().any(|z| z.id == *id))
    }
}

/// Metadata about the open world document.
#[derive(Resource, Clone)]
pub struct WorldMeta {
    pub name: String,
    pub path: Option<std::path::PathBuf>,
    pub seed: u32,
    pub dirty: bool,
    /// Frames with edits so far (`io::track_revision`): a background save that started at an
    /// older revision leaves the world dirty.
    pub revision: u64,
    /// Realistic or hand-painted: which surfaces the engine paints with and which assets the
    /// builder offers (`crate::art_style`; saved in world.json, Realistic by default).
    pub art_style: crate::art_style::ArtStyle,
    /// Fantasy or Sci-Fi (`crate::world_kind`; saved in world.json, Fantasy by default).
    pub kind: crate::world_kind::WorldKind,
}

impl Default for WorldMeta {
    fn default() -> Self {
        Self {
            name: "Untitled World".into(),
            path: None,
            seed: 7,
            dirty: false,
            revision: 0,
            art_style: Default::default(),
            kind: Default::default(),
        }
    }
}
