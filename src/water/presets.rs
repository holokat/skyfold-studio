//! The water design catalogue: one-click looks for the sea, rivers, lakes, swamps and fantasy
//! waters, the behaviours a body can have (still, ripples, flow, swells, rapids, whirlpool, storm,
//! tide, bubbling) and the reflection quality setting. A preset is a complete `WaterStyle`; the
//! controls in the Water tab edit the selected body's copy.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::{WaterStyle, WaterType};

/// How the surface moves. Each behaviour also suggests slider values when picked (see
/// `WaterStyle::set_behaviour`); the shader adds the behaviour's own effect on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum WaterBehaviour {
    /// a mirror
    Still,
    #[default]
    GentleRipples,
    SlowFlow,
    FastFlow,
    OceanSwells,
    /// white water torn along the current
    Rapids,
    /// a swirl around the body's centre with a dip in the middle
    Whirlpool,
    /// big heavy waves, whitecaps and rain on the water
    Storm,
    /// the level slowly rises and falls
    Tidal,
    /// bubbles rising and bursting in rings
    Bubbling,
    /// lava: a dark crust of plates drifting over glowing molten cracks
    Molten,
}

impl WaterBehaviour {
    pub const ALL: [WaterBehaviour; 11] = [
        WaterBehaviour::Still,
        WaterBehaviour::GentleRipples,
        WaterBehaviour::SlowFlow,
        WaterBehaviour::FastFlow,
        WaterBehaviour::OceanSwells,
        WaterBehaviour::Rapids,
        WaterBehaviour::Whirlpool,
        WaterBehaviour::Storm,
        WaterBehaviour::Tidal,
        WaterBehaviour::Bubbling,
        WaterBehaviour::Molten,
    ];
    pub fn name(self) -> &'static str {
        match self {
            WaterBehaviour::Still => "Still",
            WaterBehaviour::GentleRipples => "Gentle ripples",
            WaterBehaviour::SlowFlow => "Slow flow",
            WaterBehaviour::FastFlow => "Fast flow",
            WaterBehaviour::OceanSwells => "Ocean swells",
            WaterBehaviour::Rapids => "Rapids",
            WaterBehaviour::Whirlpool => "Whirlpool",
            WaterBehaviour::Storm => "Storm",
            WaterBehaviour::Tidal => "Tidal",
            WaterBehaviour::Bubbling => "Bubbling",
            WaterBehaviour::Molten => "Molten",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            WaterBehaviour::Still => "Glassy mirror: no waves at all",
            WaterBehaviour::GentleRipples => "Small wind ripples",
            WaterBehaviour::SlowFlow => "A lazy current",
            WaterBehaviour::FastFlow => "A quick current with more ripples",
            WaterBehaviour::OceanSwells => "Long rolling waves",
            WaterBehaviour::Rapids => "Fast white water torn along the current",
            WaterBehaviour::Whirlpool => {
                "Swirls around the centre of the lake and dips in the middle"
            }
            WaterBehaviour::Storm => {
                "Big heavy waves, whitecaps and rain (on the ocean it also turns the weather stormy)"
            }
            WaterBehaviour::Tidal => "The water level slowly rises and falls",
            WaterBehaviour::Bubbling => {
                "Bubbles rise and burst in rings (hot springs, poison, volcanic lakes)"
            }
            WaterBehaviour::Molten => {
                "Lava: a dark cooling crust drifts over glowing molten cracks; it lights its surroundings and smokes"
            }
        }
    }
    /// Index sent to the shader.
    pub fn id(self) -> f32 {
        Self::ALL.iter().position(|b| *b == self).unwrap_or(1) as f32
    }
}

/// Reflection quality: Low uses the inexpensive environment fallback, Medium uses a budgeted
/// planar capture, and High updates the full capture cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ReflectionQuality {
    Low,
    Medium,
    #[default]
    High,
}

impl ReflectionQuality {
    pub const ALL: [ReflectionQuality; 3] = [
        ReflectionQuality::Low,
        ReflectionQuality::Medium,
        ReflectionQuality::High,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ReflectionQuality::Low => "Low",
            ReflectionQuality::Medium => "Medium",
            ReflectionQuality::High => "High",
        }
    }
    pub fn id(self) -> f32 {
        match self {
            ReflectionQuality::Low => 0.0,
            ReflectionQuality::Medium => 1.0,
            ReflectionQuality::High => 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WaterCategory {
    Ocean,
    River,
    Lake,
    Swamp,
    Fantasy,
}

impl WaterCategory {
    pub const ALL: [WaterCategory; 5] = [
        WaterCategory::Ocean,
        WaterCategory::River,
        WaterCategory::Lake,
        WaterCategory::Swamp,
        WaterCategory::Fantasy,
    ];
    pub fn name(self) -> &'static str {
        match self {
            WaterCategory::Ocean => "Ocean",
            WaterCategory::River => "River",
            WaterCategory::Lake => "Lake",
            WaterCategory::Swamp => "Swamp",
            WaterCategory::Fantasy => "Fantasy",
        }
    }
}

pub struct WaterPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub category: WaterCategory,
    pub blurb: &'static str,
    pub style: WaterStyle,
}

// ---- style builder (chained setters used by the catalogue)

fn base(cat: WaterCategory, shallow: [f32; 3], deep: [f32; 3], clarity: f32) -> WaterStyle {
    let (waves, foam, shore, flow, rough, behaviour) = match cat {
        WaterCategory::Ocean => (0.6, 0.8, 0.9, 0.0, 0.0, WaterBehaviour::OceanSwells),
        WaterCategory::River => (0.6, 0.7, 0.6, 1.0, 0.04, WaterBehaviour::SlowFlow),
        WaterCategory::Lake => (0.3, 0.35, 0.4, 0.0, 0.02, WaterBehaviour::GentleRipples),
        WaterCategory::Swamp => (0.1, 0.15, 0.15, 0.03, 0.12, WaterBehaviour::Still),
        WaterCategory::Fantasy => (0.25, 0.4, 0.5, 0.0, 0.03, WaterBehaviour::GentleRipples),
    };
    let kind = match cat {
        WaterCategory::Ocean => WaterType::TemperateSea,
        WaterCategory::River => WaterType::MountainStream,
        WaterCategory::Lake => WaterType::ClearLake,
        WaterCategory::Swamp => WaterType::Swamp,
        WaterCategory::Fantasy => WaterType::Custom,
    };
    WaterStyle {
        kind,
        preset: String::new(),
        shallow,
        deep,
        clarity,
        waves,
        foam,
        flow,
        transparency: 1.0,
        wave_speed: 1.0,
        flow_dir: 30.0,
        shore_foam: shore,
        roughness: rough,
        reflections: ReflectionQuality::High,
        glow: [0.2, 0.8, 1.0],
        glow_strength: 0.0,
        emissive: [0.5, 0.4, 1.0],
        emissive_strength: 0.0,
        metallic: 0.0,
        iridescence: 0.0,
        behaviour,
    }
}

impl WaterStyle {
    fn wv(mut self, waves: f32, speed: f32) -> Self {
        self.waves = waves;
        self.wave_speed = speed;
        self
    }
    fn fl(mut self, flow: f32) -> Self {
        self.flow = flow;
        self
    }
    fn fo(mut self, foam: f32, shore: f32) -> Self {
        self.foam = foam;
        self.shore_foam = shore;
        self
    }
    fn ro(mut self, r: f32) -> Self {
        self.roughness = r;
        self
    }
    fn tr(mut self, t: f32) -> Self {
        self.transparency = t;
        self
    }
    fn gl(mut self, c: [f32; 3], s: f32) -> Self {
        self.glow = c;
        self.glow_strength = s;
        self
    }
    fn em(mut self, c: [f32; 3], s: f32) -> Self {
        self.emissive = c;
        self.emissive_strength = s;
        self
    }
    fn me(mut self, m: f32) -> Self {
        self.metallic = m;
        self
    }
    fn ir(mut self, i: f32) -> Self {
        self.iridescence = i;
        self
    }
    fn be(mut self, b: WaterBehaviour) -> Self {
        self.behaviour = b;
        self
    }
    fn k(mut self, kind: WaterType) -> Self {
        self.kind = kind;
        self
    }
}

fn build() -> Vec<WaterPreset> {
    use WaterBehaviour as B;
    use WaterCategory::*;
    let p = |id: &'static str,
             name: &'static str,
             category: WaterCategory,
             blurb: &'static str,
             style: WaterStyle| {
        let mut style = style;
        style.preset = id.to_string();
        WaterPreset {
            id,
            name,
            category,
            blurb,
            style,
        }
    };
    vec![
        // ---- ocean
        p(
            "tropical_water",
            "Tropical Water",
            Ocean,
            "Warm turquoise shallows over white sand, deep blue beyond the reef.",
            base(Ocean, [0.10, 0.70, 0.66], [0.01, 0.16, 0.32], 10.0)
                .wv(0.55, 1.0)
                .fo(0.8, 1.0)
                .k(WaterType::TropicalSea),
        ),
        p(
            "deep_ocean",
            "Deep Ocean",
            Ocean,
            "Open-water navy with a working swell and whitecaps.",
            base(Ocean, [0.06, 0.30, 0.42], [0.004, 0.035, 0.11], 6.0)
                .wv(1.15, 1.0)
                .fo(0.9, 0.9),
        ),
        p(
            "caribbean",
            "Caribbean Water",
            Ocean,
            "Glass-clear aquamarine you can see the bottom through.",
            base(Ocean, [0.20, 0.86, 0.80], [0.0, 0.24, 0.42], 16.0)
                .wv(0.45, 1.0)
                .fo(0.7, 1.0)
                .k(WaterType::TropicalSea),
        ),
        p(
            "mediterranean",
            "Mediterranean",
            Ocean,
            "Clear sapphire-blue sea under a bright sun.",
            base(Ocean, [0.12, 0.52, 0.70], [0.01, 0.10, 0.30], 14.0)
                .wv(0.5, 1.0)
                .fo(0.7, 0.9),
        ),
        p(
            "temperate_ocean",
            "Temperate Ocean",
            Ocean,
            "Green-grey northern sea, a little murky.",
            base(Ocean, [0.12, 0.42, 0.42], [0.01, 0.08, 0.16], 4.0)
                .wv(0.7, 1.0)
                .fo(0.8, 0.8),
        ),
        p(
            "arctic_ocean",
            "Arctic Ocean",
            Ocean,
            "Cold steel-blue water with a hard chop.",
            base(Ocean, [0.30, 0.47, 0.50], [0.025, 0.065, 0.09], 6.0)
                .wv(0.85, 0.9)
                .fo(0.9, 1.0)
                .ro(0.06)
                .k(WaterType::ArcticSea),
        ),
        p(
            "emerald_coast",
            "Emerald Coast",
            Ocean,
            "Emerald-green shallows fading into teal.",
            base(Ocean, [0.18, 0.72, 0.50], [0.0, 0.20, 0.20], 12.0)
                .wv(0.45, 1.0)
                .fo(0.7, 1.0),
        ),
        p(
            "blackwater_coast",
            "Blackwater Coast",
            Ocean,
            "Tannin-dark water off peat and forest shores.",
            base(Ocean, [0.28, 0.20, 0.10], [0.02, 0.015, 0.01], 1.2)
                .wv(0.7, 1.0)
                .fo(0.9, 1.0)
                .tr(0.85),
        ),
        p(
            "bioluminescent",
            "Bioluminescent Ocean",
            Ocean,
            "By night every breaking wave and foam line glows blue.",
            base(Ocean, [0.06, 0.30, 0.40], [0.005, 0.04, 0.10], 7.0)
                .wv(0.6, 1.0)
                .fo(0.9, 1.0)
                .gl([0.15, 0.75, 1.0], 1.0),
        ),
        p(
            "stormy_sea",
            "Stormy Sea",
            Ocean,
            "Heavy grey seas, streaked foam and driving rain.",
            base(Ocean, [0.20, 0.30, 0.30], [0.03, 0.06, 0.08], 2.5)
                .wv(2.2, 1.25)
                .fo(1.0, 1.0)
                .be(B::Storm)
                .k(WaterType::StormySea),
        ),
        // ---- rivers and streams
        p(
            "crystal_river",
            "Crystal River",
            River,
            "Clear spring-fed river; every pebble shows.",
            base(River, [0.30, 0.78, 0.72], [0.03, 0.28, 0.32], 14.0)
                .wv(0.35, 1.0)
                .fl(1.0)
                .fo(0.4, 0.4),
        ),
        p(
            "mountain_stream",
            "Mountain Stream",
            River,
            "Cold, quick and green over boulders.",
            base(River, [0.22, 0.52, 0.48], [0.04, 0.16, 0.20], 5.0)
                .wv(0.7, 1.1)
                .fl(1.6)
                .fo(1.0, 0.8)
                .be(B::FastFlow),
        ),
        p(
            "tropical_river",
            "Tropical River",
            River,
            "Warm jungle river, green and slow.",
            base(River, [0.22, 0.50, 0.36], [0.04, 0.16, 0.10], 2.5)
                .wv(0.4, 1.0)
                .fl(0.9)
                .fo(0.5, 0.5),
        ),
        p(
            "forest_creek",
            "Forest Creek",
            River,
            "Brown-green creek under the trees.",
            base(River, [0.30, 0.40, 0.24], [0.06, 0.10, 0.05], 2.0)
                .wv(0.45, 1.0)
                .fl(1.1)
                .fo(0.6, 0.5)
                .ro(0.05),
        ),
        p(
            "muddy_river",
            "Muddy River",
            River,
            "Silt-laden brown water, opaque.",
            base(River, [0.40, 0.32, 0.18], [0.14, 0.10, 0.05], 0.8)
                .wv(0.45, 1.0)
                .fl(0.8)
                .fo(0.6, 0.5)
                .tr(0.9)
                .k(WaterType::MuddyRiver),
        ),
        p(
            "glacial_river",
            "Glacial River",
            River,
            "Milky turquoise meltwater full of rock flour.",
            base(River, [0.42, 0.80, 0.82], [0.10, 0.42, 0.50], 3.0)
                .wv(0.7, 1.1)
                .fl(1.5)
                .fo(0.9, 0.8)
                .tr(0.7)
                .be(B::FastFlow),
        ),
        p(
            "rapids",
            "Rapids",
            River,
            "Fast white water tearing over rocks.",
            base(River, [0.26, 0.56, 0.52], [0.04, 0.18, 0.20], 4.0)
                .wv(1.3, 1.3)
                .fl(2.6)
                .fo(1.0, 1.0)
                .be(B::Rapids),
        ),
        p(
            "waterfall",
            "Waterfall",
            River,
            "Aerated falling water: use it for waterfalls and cascades.",
            base(River, [0.34, 0.70, 0.66], [0.06, 0.26, 0.28], 6.0)
                .wv(0.9, 1.2)
                .fl(2.2)
                .fo(1.0, 1.0)
                .be(B::Rapids),
        ),
        p(
            "desert_river",
            "Desert River",
            River,
            "Slow silty olive water through dry land.",
            base(River, [0.46, 0.46, 0.30], [0.12, 0.16, 0.10], 1.5)
                .wv(0.3, 0.9)
                .fl(0.6)
                .fo(0.4, 0.4),
        ),
        p(
            "underground_river",
            "Underground River",
            River,
            "Black cave water with faint glowing life.",
            base(River, [0.10, 0.24, 0.30], [0.0, 0.02, 0.05], 8.0)
                .wv(0.2, 0.8)
                .fl(0.7)
                .fo(0.3, 0.3)
                .gl([0.2, 0.65, 0.95], 0.4),
        ),
        // ---- lakes and ponds
        p(
            "alpine_lake",
            "Alpine Lake",
            Lake,
            "Cold, clear blue-green mountain lake.",
            base(Lake, [0.20, 0.62, 0.70], [0.01, 0.14, 0.26], 12.0)
                .wv(0.25, 0.9)
                .fo(0.3, 0.4),
        ),
        p(
            "mirror_lake",
            "Mirror Lake",
            Lake,
            "Perfectly still: the sky and shore mirrored.",
            base(Lake, [0.14, 0.40, 0.42], [0.01, 0.06, 0.10], 8.0)
                .wv(0.0, 0.6)
                .fo(0.1, 0.1)
                .ro(0.0)
                .be(B::Still),
        ),
        p(
            "forest_lake",
            "Forest Lake",
            Lake,
            "Dark green-brown water under the trees.",
            base(Lake, [0.22, 0.38, 0.26], [0.02, 0.07, 0.05], 3.5)
                .wv(0.25, 0.9)
                .fo(0.25, 0.3),
        ),
        p(
            "tropical_lagoon",
            "Tropical Lagoon",
            Lake,
            "Sheltered turquoise lagoon over pale sand.",
            base(Lake, [0.14, 0.78, 0.72], [0.0, 0.26, 0.40], 14.0)
                .wv(0.3, 1.0)
                .fo(0.5, 0.8),
        ),
        p(
            "volcanic_lake",
            "Volcanic Lake",
            Lake,
            "Acid yellow-green crater water, gently bubbling.",
            base(Lake, [0.40, 0.62, 0.30], [0.12, 0.24, 0.10], 1.5)
                .wv(0.15, 0.8)
                .fo(0.3, 0.3)
                .tr(0.7)
                .ro(0.08)
                .be(B::Bubbling),
        ),
        p(
            "crater_lake",
            "Crater Lake",
            Lake,
            "Deep, intensely blue water in an old caldera.",
            base(Lake, [0.06, 0.32, 0.66], [0.0, 0.04, 0.20], 20.0)
                .wv(0.2, 0.9)
                .fo(0.2, 0.3),
        ),
        p(
            "swamp_pond",
            "Swamp Pond",
            Lake,
            "Murky green pond with lily pads and reeds.",
            base(Lake, [0.26, 0.30, 0.14], [0.05, 0.07, 0.03], 0.9)
                .wv(0.05, 0.7)
                .fo(0.15, 0.15)
                .tr(0.7)
                .ro(0.12)
                .be(B::Still),
        ),
        p(
            "koi_pond",
            "Koi Pond",
            Lake,
            "Clear garden pond over pebbles.",
            base(Lake, [0.24, 0.50, 0.36], [0.03, 0.14, 0.10], 4.0)
                .wv(0.1, 0.7)
                .fo(0.1, 0.15)
                .be(B::Still),
        ),
        p(
            "frozen_lake",
            "Frozen Lake",
            Lake,
            "Ice-covered: pale, frosted and still.",
            base(Lake, [0.70, 0.86, 0.92], [0.36, 0.56, 0.66], 2.0)
                .wv(0.0, 0.5)
                .fo(0.0, 0.0)
                .tr(0.25)
                .ro(0.35)
                .be(B::Still),
        ),
        p(
            "salt_lake",
            "Salt Lake",
            Lake,
            "Shallow milky brine that mirrors the sky.",
            base(Lake, [0.50, 0.82, 0.80], [0.20, 0.50, 0.56], 3.0)
                .wv(0.05, 0.6)
                .fo(0.15, 0.6)
                .tr(0.8)
                .ro(0.04)
                .be(B::Still),
        ),
        p(
            "clear_lake",
            "Clear Lake",
            Lake,
            "A clear temperate lake (the classic default).",
            base(Lake, [0.16, 0.48, 0.46], [0.02, 0.12, 0.18], 6.0)
                .wv(0.35, 1.0)
                .fo(0.4, 0.4),
        ),
        p(
            "glacial_lake",
            "Glacial Lake",
            Lake,
            "Bright milky turquoise meltwater.",
            base(Lake, [0.30, 0.78, 0.80], [0.04, 0.32, 0.44], 10.0)
                .wv(0.3, 0.9)
                .fo(0.3, 0.3)
                .k(WaterType::GlacialLake),
        ),
        // ---- swamps
        p(
            "murky_swamp",
            "Murky Swamp",
            Swamp,
            "Opaque olive water with a sluggish surface.",
            base(Swamp, [0.22, 0.26, 0.12], [0.04, 0.06, 0.03], 1.0)
                .wv(0.12, 0.7)
                .fl(0.05)
                .fo(0.2, 0.2)
                .tr(0.75)
                .be(B::GentleRipples),
        ),
        p(
            "blackwater_swamp",
            "Blackwater Swamp",
            Swamp,
            "Tea-black glossy water that mirrors the trees.",
            base(Swamp, [0.24, 0.14, 0.06], [0.012, 0.008, 0.004], 1.2)
                .wv(0.03, 0.6)
                .fo(0.1, 0.1)
                .tr(0.9)
                .ro(0.02),
        ),
        p(
            "marsh_water",
            "Marsh Water",
            Swamp,
            "Shallow grassy water between reed beds.",
            base(Swamp, [0.30, 0.40, 0.26], [0.06, 0.10, 0.06], 1.5)
                .wv(0.15, 0.8)
                .fo(0.15, 0.2)
                .ro(0.06)
                .be(B::GentleRipples),
        ),
        p(
            "bog_water",
            "Bog Water",
            Swamp,
            "Peaty brown, thick and still.",
            base(Swamp, [0.36, 0.24, 0.10], [0.08, 0.04, 0.015], 0.6)
                .wv(0.02, 0.6)
                .fo(0.1, 0.1)
                .tr(0.6)
                .ro(0.2),
        ),
        p(
            "stagnant_pond",
            "Stagnant Pond",
            Swamp,
            "Algae-green scummy water that never moves.",
            base(Swamp, [0.30, 0.36, 0.10], [0.06, 0.10, 0.02], 0.5)
                .wv(0.0, 0.5)
                .fo(0.1, 0.1)
                .tr(0.45)
                .ro(0.3),
        ),
        p(
            "poisonous_swamp",
            "Poisonous Swamp",
            Swamp,
            "Toxic green ooze that glows faintly and bubbles.",
            base(Swamp, [0.40, 0.62, 0.12], [0.06, 0.16, 0.02], 0.8)
                .wv(0.05, 0.6)
                .fo(0.2, 0.2)
                .tr(0.55)
                .em([0.35, 0.9, 0.1], 0.25)
                .be(B::Bubbling),
        ),
        p(
            "mangrove_water",
            "Mangrove Water",
            Swamp,
            "Brackish tidal water among the roots.",
            base(Swamp, [0.30, 0.40, 0.28], [0.05, 0.10, 0.08], 1.8)
                .wv(0.2, 0.8)
                .fl(0.1)
                .fo(0.25, 0.3)
                .ro(0.05)
                .be(B::Tidal),
        ),
        p(
            "flooded_forest",
            "Flooded Forest",
            Swamp,
            "Dark still floodwater mirroring the canopy.",
            base(Swamp, [0.28, 0.28, 0.16], [0.05, 0.05, 0.02], 2.0)
                .wv(0.03, 0.6)
                .fo(0.1, 0.1)
                .tr(0.85)
                .ro(0.04),
        ),
        // ---- fantasy
        p(
            "arcane_water",
            "Arcane Water",
            Fantasy,
            "Violet-blue water veined with drifting light.",
            base(Fantasy, [0.36, 0.30, 0.86], [0.06, 0.03, 0.30], 6.0)
                .wv(0.3, 0.8)
                .em([0.45, 0.30, 1.0], 0.45)
                .gl([0.6, 0.4, 1.0], 0.6)
                .ir(0.15),
        ),
        p(
            "celestial_water",
            "Celestial Water",
            Fantasy,
            "Silver-blue sheen that sparkles like starlight.",
            base(Fantasy, [0.62, 0.74, 0.90], [0.16, 0.24, 0.44], 10.0)
                .wv(0.2, 0.7)
                .tr(0.85)
                .me(0.35)
                .ro(0.05)
                .gl([0.8, 0.9, 1.0], 0.7)
                .ir(0.1),
        ),
        p(
            "void_water",
            "Void Water",
            Fantasy,
            "Pitch-black, bottomless, a perfect dark mirror.",
            base(Fantasy, [0.02, 0.02, 0.03], [0.0, 0.0, 0.0], 0.3)
                .wv(0.08, 0.4)
                .fo(0.0, 0.0)
                .tr(0.0)
                .ro(0.0)
                .be(B::Still),
        ),
        p(
            "enchanted_spring",
            "Enchanted Spring",
            Fantasy,
            "Bright cyan spring water, glowing and bubbling.",
            base(Fantasy, [0.25, 0.95, 0.95], [0.02, 0.50, 0.62], 16.0)
                .wv(0.15, 0.8)
                .em([0.2, 0.9, 1.0], 0.35)
                .gl([0.4, 1.0, 1.0], 0.8)
                .be(B::Bubbling),
        ),
        p(
            "blood_lake",
            "Blood Lake",
            Fantasy,
            "Thick deep-crimson water.",
            base(Fantasy, [0.55, 0.04, 0.05], [0.16, 0.0, 0.01], 0.6)
                .wv(0.15, 0.6)
                .fo(0.25, 0.3)
                .tr(0.25)
                .ro(0.12),
        ),
        p(
            "liquid_gold",
            "Liquid Gold",
            Fantasy,
            "Molten metallic gold, slow and heavy.",
            base(Fantasy, [1.0, 0.76, 0.30], [0.50, 0.32, 0.06], 0.3)
                .wv(0.2, 0.45)
                .fo(0.0, 0.0)
                .tr(0.0)
                .me(1.0)
                .ro(0.22)
                .em([1.0, 0.62, 0.18], 0.2),
        ),
        p(
            "moonwater",
            "Moonwater",
            Fantasy,
            "Pale silver water that glows under the moon.",
            base(Fantasy, [0.80, 0.84, 0.90], [0.30, 0.34, 0.42], 8.0)
                .wv(0.1, 0.6)
                .tr(0.6)
                .me(0.5)
                .ro(0.04)
                .gl([0.85, 0.9, 1.0], 0.9)
                .be(B::Still),
        ),
        p(
            "spirit_water",
            "Spirit Water",
            Fantasy,
            "Translucent glowing turquoise, clear to the bottom.",
            base(Fantasy, [0.30, 0.92, 0.80], [0.04, 0.40, 0.36], 25.0)
                .wv(0.15, 0.6)
                .em([0.25, 1.0, 0.8], 0.25)
                .gl([0.4, 1.0, 0.85], 0.6),
        ),
        p(
            "corrupted_water",
            "Corrupted Water",
            Fantasy,
            "Purple-black tainted water, bubbling with malice.",
            base(Fantasy, [0.22, 0.06, 0.26], [0.03, 0.0, 0.05], 0.7)
                .wv(0.15, 0.7)
                .tr(0.3)
                .ro(0.15)
                .em([0.6, 0.1, 0.9], 0.3)
                .be(B::Bubbling),
        ),
        p(
            "healing_waters",
            "Healing Waters",
            Fantasy,
            "Emerald-cyan water with a soft inner glow.",
            base(Fantasy, [0.20, 0.88, 0.66], [0.02, 0.36, 0.32], 14.0)
                .wv(0.2, 0.8)
                .em([0.25, 1.0, 0.65], 0.3)
                .gl([0.5, 1.0, 0.7], 0.6)
                .ir(0.1),
        ),
        p(
            "dreamwater",
            "Dreamwater",
            Fantasy,
            "Pastel blue-violet with a shifting pearly sheen.",
            base(Fantasy, [0.66, 0.66, 0.96], [0.26, 0.24, 0.56], 10.0)
                .wv(0.2, 0.5)
                .tr(0.8)
                .ir(0.45)
                .em([0.6, 0.55, 1.0], 0.15)
                .gl([0.9, 0.7, 1.0], 0.5),
        ),
        p(
            "astral_ocean",
            "Astral Ocean",
            Fantasy,
            "Midnight-blue sea full of star-like sparks.",
            base(Fantasy, [0.08, 0.14, 0.40], [0.0, 0.01, 0.08], 8.0)
                .wv(0.6, 0.9)
                .fo(0.6, 0.7)
                .em([0.2, 0.3, 0.9], 0.12)
                .gl([0.6, 0.75, 1.0], 1.0)
                .be(B::OceanSwells),
        ),
        p(
            "liquid_crystal",
            "Liquid Crystal",
            Fantasy,
            "Iridescent blue, rainbow sheen on every ripple.",
            base(Fantasy, [0.50, 0.80, 1.0], [0.08, 0.30, 0.60], 14.0)
                .wv(0.15, 0.7)
                .me(0.25)
                .ro(0.02)
                .ir(1.0)
                .be(B::Still),
        ),
        p(
            "lava",
            "Lava",
            Fantasy,
            "Molten rock: glowing orange cracks under a dark drifting crust, lighting the ground around it.",
            base(Fantasy, [1.0, 0.36, 0.06], [0.42, 0.05, 0.0], 0.1)
                .wv(0.05, 0.3)
                .fo(0.0, 0.0)
                .tr(0.0)
                .ro(0.55)
                .em([1.0, 0.34, 0.05], 1.6)
                .gl([1.0, 0.45, 0.1], 0.6)
                .be(B::Molten),
        ),
        p(
            "abyssal_water",
            "Abyssal Water",
            Fantasy,
            "Deep indigo-black with faint lights far below.",
            base(Fantasy, [0.06, 0.05, 0.22], [0.0, 0.0, 0.03], 2.0)
                .wv(0.4, 0.8)
                .tr(0.4)
                .ro(0.02)
                .gl([0.25, 0.4, 1.0], 0.4),
        ),
    ]
}

/// Every preset, grouped by category in display order.
pub fn presets() -> &'static [WaterPreset] {
    static P: OnceLock<Vec<WaterPreset>> = OnceLock::new();
    P.get_or_init(build)
}

pub fn preset(id: &str) -> Option<&'static WaterPreset> {
    presets().iter().find(|p| p.id == id)
}

/// A preset's style (falls back to Tropical Water for unknown ids).
pub fn preset_style(id: &str) -> WaterStyle {
    preset(id).unwrap_or(&presets()[0]).style.clone()
}

pub fn in_category(cat: WaterCategory) -> impl Iterator<Item = &'static WaterPreset> {
    presets().iter().filter(move |p| p.category == cat)
}

impl WaterType {
    /// The catalogue preset a legacy water type became.
    pub fn preset_id(self) -> &'static str {
        match self {
            WaterType::TemperateSea | WaterType::Custom => "temperate_ocean",
            WaterType::TropicalSea => "tropical_water",
            WaterType::ArcticSea => "arctic_ocean",
            WaterType::StormySea => "stormy_sea",
            WaterType::ClearLake => "clear_lake",
            WaterType::GlacialLake => "glacial_lake",
            WaterType::MountainStream => "mountain_stream",
            WaterType::MuddyRiver => "muddy_river",
            WaterType::Swamp => "murky_swamp",
        }
    }
    /// The river preset that fits a biome whose lakes have this water type.
    pub fn river_preset_id(self) -> &'static str {
        match self {
            WaterType::TemperateSea | WaterType::Custom | WaterType::MountainStream => {
                "mountain_stream"
            }
            WaterType::TropicalSea => "tropical_river",
            WaterType::ArcticSea | WaterType::GlacialLake => "glacial_river",
            WaterType::StormySea => "rapids",
            WaterType::ClearLake => "crystal_river",
            WaterType::MuddyRiver => "muddy_river",
            WaterType::Swamp => "forest_creek",
        }
    }
}

/// What kind of body a style is on (decides which preset an old world's custom style is based on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Ocean,
    Lake,
    River,
}

impl WaterStyle {
    /// Worlds saved before the preset catalogue carry only colours, clarity, waves, foam and flow:
    /// keep those and take every newer setting from the preset their water type became. Returns
    /// true if the style was upgraded.
    pub fn upgrade(&mut self, body: BodyKind) -> bool {
        if !self.preset.is_empty() {
            return false;
        }
        let id = match (self.kind, body) {
            (WaterType::Custom, BodyKind::Ocean) => "temperate_ocean",
            (WaterType::Custom, BodyKind::Lake) => "clear_lake",
            (WaterType::Custom, BodyKind::River) => "mountain_stream",
            (k, _) => k.preset_id(),
        };
        let p = preset_style(id);
        let old = self.clone();
        *self = WaterStyle {
            shallow: old.shallow,
            deep: old.deep,
            clarity: old.clarity,
            waves: old.waves,
            foam: old.foam,
            flow: old.flow,
            kind: old.kind,
            ..p.clone()
        };
        // the old shader scaled the shore break by the foam amount
        self.shore_foam = old.foam;
        let same = old.shallow == p.shallow
            && old.deep == p.deep
            && old.clarity == p.clarity
            && old.waves == p.waves
            && old.foam == p.foam;
        self.preset = if same && old.kind != WaterType::Custom {
            id.to_string()
        } else {
            "custom".to_string()
        };
        true
    }

    /// Pick a behaviour and the slider values that go with it.
    pub fn set_behaviour(&mut self, b: WaterBehaviour, body: BodyKind) {
        self.behaviour = b;
        let ocean = body == BodyKind::Ocean;
        match b {
            WaterBehaviour::Still => {
                self.waves = 0.0;
                self.wave_speed = 0.7;
                if body != BodyKind::River {
                    self.flow = 0.0;
                }
            }
            WaterBehaviour::GentleRipples => {
                self.waves = if ocean { 0.3 } else { 0.3 };
                self.wave_speed = 0.85;
            }
            WaterBehaviour::SlowFlow => {
                self.flow = if body == BodyKind::River { 0.7 } else { 0.25 };
                self.waves = self.waves.clamp(0.25, 0.5);
            }
            WaterBehaviour::FastFlow => {
                self.flow = if body == BodyKind::River { 1.6 } else { 0.8 };
                self.waves = self.waves.max(0.6);
            }
            WaterBehaviour::OceanSwells => {
                self.waves = if ocean { 0.9 } else { 1.2 };
                self.wave_speed = 1.0;
            }
            WaterBehaviour::Rapids => {
                self.flow = if body == BodyKind::River { 2.6 } else { 1.4 };
                self.waves = 1.3;
                self.foam = self.foam.max(0.9);
                self.wave_speed = 1.3;
            }
            WaterBehaviour::Whirlpool => {
                self.flow = self.flow.max(1.2);
                self.waves = 0.5;
                self.foam = self.foam.max(0.6);
            }
            WaterBehaviour::Storm => {
                self.waves = if ocean { 2.3 } else { 2.0 };
                self.wave_speed = 1.3;
                self.foam = 1.0;
            }
            WaterBehaviour::Tidal => {
                self.waves = self.waves.clamp(0.3, 0.8);
            }
            WaterBehaviour::Bubbling => {
                self.waves = self.waves.min(0.2);
            }
            WaterBehaviour::Molten => {
                self.waves = self.waves.min(0.1);
                self.foam = 0.0;
                self.shore_foam = 0.0;
                self.transparency = 0.0;
                self.wave_speed = self.wave_speed.min(0.4);
            }
        }
    }

    pub fn preset_name(&self) -> &'static str {
        if self.preset == "custom" {
            return "Custom";
        }
        preset(&self.preset).map(|p| p.name).unwrap_or("Custom")
    }
}
