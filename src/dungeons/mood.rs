//! A dungeon's mood: the fill light in the dark, how open the eye is, the fog and haze, dust in
//! the air, the tint of its fires and the sound of the place (a bed, a layer, and the random
//! sounds of its theme from the dungeon sound library, `sound`). Presets set everything at once;
//! every value can then be tweaked.

use serde::{Deserialize, Serialize};

/// Light, air and sound of an underground map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mood {
    /// the preset it started from (`preset`)
    pub preset: String,
    /// the soft fill everywhere (srgb) and how strong (0 = pitch black away from lights, 1 =
    /// everything readable)
    pub fill: [f32; 3],
    pub fill_strength: f32,
    /// exposure: + brighter, − darker (stops)
    pub exposure: f32,
    /// fog colour (srgb) and thickness (0..1)
    pub fog: [f32; 3],
    pub fog_density: f32,
    /// glowing haze lit by the lights (shafts, halos round fires; 0..1, costly)
    pub haze: f32,
    /// dust motes or embers drifting in the light (0..1)
    pub motes: f32,
    /// the colour all fires and lamps are pulled towards (srgb), and how far (0..1)
    pub tint: [f32; 3],
    pub tint_amount: f32,
    /// the sound theme (`sound::Library::themes`: its random sounds and footsteps)
    pub theme: String,
    /// the ambience bed (a sound id, e.g. `amb_dng_crypt`) and a layer under it (a loop id)
    pub sound: String,
    pub sound2: String,
    pub volume: f32,
}

impl Default for Mood {
    fn default() -> Self {
        preset("torchlit").unwrap()
    }
}

/// (id, name, description)
pub const PRESETS: [(&str, &str, &str); 22] = [
    (
        "torchlit",
        "Torchlit",
        "Warm fires in the dark, a little smoke",
    ),
    (
        "candlelit",
        "Candlelit crypt",
        "Dim candles, deep shadows, dust",
    ),
    (
        "catacombs",
        "Catacombs",
        "Close dry air, bone dust, distant skittering",
    ),
    ("prison", "Prison", "Cold cells, chains and far-off moans"),
    (
        "moonlit",
        "Cold moonlight",
        "Blue light falling from high windows",
    ),
    (
        "golden",
        "Golden temple",
        "Warm gold light and incense haze",
    ),
    ("forge", "Forge heat", "Orange glow, smoke and embers"),
    ("damp", "Damp sewers", "Green-grey murk, running water"),
    ("frost", "Frozen halls", "Pale icy light, cold mist"),
    (
        "overgrown",
        "Overgrown",
        "Green shafts through broken roofs, damp moss",
    ),
    ("industrial", "Works", "Hard white lamps, steam, machinery"),
    ("void", "Void-touched", "Violet gloom, an unnatural hum"),
    ("infernal", "Infernal", "Red light, ash and embers"),
    ("cave", "Natural cave", "Dark rock, drips, cool air"),
    ("mine", "Old mine", "Lantern light, creaking timbers"),
    ("city", "Undercity", "A vast cavern alive with distant work"),
    ("spider", "Spider den", "Violet murk, webs and chittering"),
    ("tomb", "Desert tomb", "Still, dry, golden dust"),
    (
        "arcane",
        "Arcane sanctum",
        "Blue-violet magic humming in the stone",
    ),
    (
        "sunken",
        "Sunken ruins",
        "Teal water-light, lapping and bubbling",
    ),
    ("haunted", "Haunted", "Thick pale fog, whispers"),
    (
        "readable",
        "Readable",
        "Bright enough to see every corner (for play)",
    ),
];

/// A preset's name ("Custom" for a mood of its own).
pub fn name(id: &str) -> &'static str {
    PRESETS
        .iter()
        .find(|p| p.0 == id)
        .map(|p| p.1)
        .unwrap_or("Custom")
}

/// A preset's mood.
pub fn preset(id: &str) -> Option<Mood> {
    #[allow(clippy::too_many_arguments)]
    let m = |fill: [f32; 3],
             fs: f32,
             ex: f32,
             fog: [f32; 3],
             fd: f32,
             haze: f32,
             motes: f32,
             tint: [f32; 3],
             ta: f32,
             theme: &str,
             s2: &str| Mood {
        preset: id.to_string(),
        fill,
        fill_strength: fs,
        exposure: ex,
        fog,
        fog_density: fd,
        haze,
        motes,
        tint,
        tint_amount: ta,
        theme: theme.into(),
        sound: format!("amb_dng_{theme}"),
        sound2: s2.into(),
        volume: 0.8,
    };
    Some(match id {
        "torchlit" => m(
            [0.42, 0.36, 0.32],
            0.22,
            0.0,
            [0.09, 0.07, 0.06],
            0.22,
            0.2,
            0.25,
            [1.0, 0.6, 0.3],
            0.0,
            "keep",
            "loop_drips",
        ),
        "candlelit" => m(
            [0.38, 0.34, 0.33],
            0.12,
            -0.3,
            [0.05, 0.045, 0.045],
            0.3,
            0.15,
            0.45,
            [1.0, 0.68, 0.4],
            0.2,
            "crypt",
            "",
        ),
        "catacombs" => m(
            [0.4, 0.36, 0.32],
            0.14,
            -0.2,
            [0.08, 0.07, 0.06],
            0.32,
            0.15,
            0.5,
            [1.0, 0.66, 0.38],
            0.15,
            "catacombs",
            "",
        ),
        "prison" => m(
            [0.36, 0.38, 0.42],
            0.18,
            -0.1,
            [0.07, 0.075, 0.08],
            0.3,
            0.2,
            0.2,
            [1.0, 0.62, 0.32],
            0.0,
            "prison",
            "loop_chains",
        ),
        "moonlit" => m(
            [0.36, 0.44, 0.62],
            0.4,
            0.2,
            [0.12, 0.15, 0.22],
            0.35,
            0.45,
            0.35,
            [0.75, 0.85, 1.0],
            0.35,
            "cathedral",
            "loop_wind_crack",
        ),
        "golden" => m(
            [0.62, 0.5, 0.3],
            0.42,
            0.3,
            [0.3, 0.24, 0.14],
            0.28,
            0.4,
            0.4,
            [1.0, 0.72, 0.35],
            0.2,
            "temple",
            "",
        ),
        "forge" => m(
            [0.6, 0.36, 0.2],
            0.3,
            0.1,
            [0.22, 0.12, 0.07],
            0.35,
            0.45,
            0.7,
            [1.0, 0.5, 0.2],
            0.3,
            "forge",
            "loop_furnace",
        ),
        "damp" => m(
            [0.34, 0.4, 0.36],
            0.22,
            -0.1,
            [0.1, 0.12, 0.1],
            0.45,
            0.2,
            0.15,
            [0.95, 0.85, 0.6],
            0.1,
            "sewer",
            "loop_stream",
        ),
        "frost" => m(
            [0.5, 0.62, 0.78],
            0.45,
            0.3,
            [0.4, 0.48, 0.58],
            0.38,
            0.3,
            0.35,
            [0.7, 0.85, 1.0],
            0.4,
            "ice",
            "",
        ),
        "overgrown" => m(
            [0.4, 0.5, 0.34],
            0.38,
            0.2,
            [0.16, 0.2, 0.13],
            0.35,
            0.55,
            0.45,
            [0.9, 1.0, 0.75],
            0.2,
            "fungal",
            "loop_drips",
        ),
        "industrial" => m(
            [0.42, 0.44, 0.46],
            0.35,
            0.2,
            [0.18, 0.19, 0.2],
            0.3,
            0.4,
            0.2,
            [0.95, 0.97, 1.0],
            0.0,
            "works",
            "loop_gears",
        ),
        "void" => m(
            [0.4, 0.3, 0.6],
            0.3,
            0.0,
            [0.12, 0.07, 0.18],
            0.4,
            0.35,
            0.4,
            [0.7, 0.45, 1.0],
            0.4,
            "void",
            "",
        ),
        "infernal" => m(
            [0.6, 0.24, 0.16],
            0.3,
            0.0,
            [0.25, 0.07, 0.04],
            0.4,
            0.4,
            0.75,
            [1.0, 0.35, 0.15],
            0.4,
            "infernal",
            "loop_lava",
        ),
        "cave" => m(
            [0.36, 0.36, 0.38],
            0.2,
            0.0,
            [0.06, 0.065, 0.07],
            0.25,
            0.1,
            0.1,
            [1.0, 0.75, 0.5],
            0.0,
            "cavern",
            "loop_drips",
        ),
        "mine" => m(
            [0.42, 0.38, 0.32],
            0.2,
            0.0,
            [0.08, 0.07, 0.06],
            0.3,
            0.2,
            0.3,
            [1.0, 0.68, 0.36],
            0.1,
            "mine",
            "",
        ),
        "city" => m(
            [0.5, 0.44, 0.36],
            0.38,
            0.3,
            [0.16, 0.13, 0.1],
            0.3,
            0.35,
            0.3,
            [1.0, 0.74, 0.42],
            0.1,
            "city",
            "loop_fountain",
        ),
        "spider" => m(
            [0.38, 0.3, 0.52],
            0.24,
            -0.1,
            [0.1, 0.07, 0.14],
            0.42,
            0.3,
            0.35,
            [0.62, 0.5, 1.0],
            0.35,
            "spider",
            "loop_cave_insects",
        ),
        "tomb" => m(
            [0.62, 0.52, 0.36],
            0.32,
            0.2,
            [0.28, 0.22, 0.14],
            0.3,
            0.35,
            0.55,
            [1.0, 0.76, 0.42],
            0.2,
            "tomb",
            "loop_sand_trickle",
        ),
        "arcane" => m(
            [0.42, 0.42, 0.7],
            0.32,
            0.1,
            [0.1, 0.1, 0.2],
            0.32,
            0.45,
            0.45,
            [0.6, 0.7, 1.0],
            0.45,
            "arcane",
            "loop_crystal_hum",
        ),
        "sunken" => m(
            [0.3, 0.5, 0.52],
            0.32,
            0.0,
            [0.05, 0.14, 0.15],
            0.48,
            0.35,
            0.3,
            [0.6, 0.95, 0.9],
            0.35,
            "sunken",
            "loop_drips",
        ),
        "haunted" => m(
            [0.4, 0.5, 0.5],
            0.3,
            0.0,
            [0.3, 0.36, 0.36],
            0.7,
            0.35,
            0.3,
            [0.75, 0.95, 0.9],
            0.4,
            "haunted",
            "loop_whispers",
        ),
        "readable" => m(
            [0.75, 0.72, 0.68],
            0.95,
            0.6,
            [0.2, 0.19, 0.18],
            0.05,
            0.0,
            0.1,
            [1.0, 0.8, 0.6],
            0.0,
            "keep",
            "",
        ),
        _ => return None,
    })
}

impl Mood {
    /// A fire's or lamp's colour after the mood's tint.
    pub fn tinted(&self, c: [f32; 3]) -> [f32; 3] {
        let t = self.tint_amount.clamp(0.0, 1.0);
        [
            c[0] + (self.tint[0] - c[0]) * t,
            c[1] + (self.tint[1] - c[1]) * t,
            c[2] + (self.tint[2] - c[2]) * t,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_exists_and_its_sounds_are_in_the_library() {
        let lib = crate::dungeons::sound::Library::load();
        assert!(
            lib.sounds.len() > 100,
            "the dungeon sound library loads ({} sounds)",
            lib.sounds.len()
        );
        for (id, _, _) in PRESETS {
            let m = preset(id).unwrap_or_else(|| panic!("{id}"));
            assert!(lib.sounds.contains_key(&m.sound), "{id}: bed {}", m.sound);
            if !m.sound2.is_empty() {
                assert!(
                    lib.sounds.contains_key(&m.sound2),
                    "{id}: layer {}",
                    m.sound2
                );
            }
            assert!(lib.themes.contains_key(&m.theme), "{id}: theme {}", m.theme);
        }
    }
}
