//! Terrain material layers and the footstep surfaces they map to.

use serde::{Deserialize, Serialize};

pub const LAYER_COUNT: usize = 12;

/// Paintable terrain layers. The index order is baked into the splat textures and the shader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Layer {
    Grass = 0,
    DryGrass = 1,
    ForestFloor = 2,
    Dirt = 3,
    Mud = 4,
    Gravel = 5,
    Rock = 6,
    DarkRock = 7,
    Sand = 8,
    Snow = 9,
    Cobblestone = 10,
    Ice = 11,
}

impl Layer {
    pub const ALL: [Layer; LAYER_COUNT] = [
        Layer::Grass,
        Layer::DryGrass,
        Layer::ForestFloor,
        Layer::Dirt,
        Layer::Mud,
        Layer::Gravel,
        Layer::Rock,
        Layer::DarkRock,
        Layer::Sand,
        Layer::Snow,
        Layer::Cobblestone,
        Layer::Ice,
    ];

    pub fn from_index(i: usize) -> Layer {
        Self::ALL[i.min(LAYER_COUNT - 1)]
    }

    pub fn name(self) -> &'static str {
        match self {
            Layer::Grass => "Grass",
            Layer::DryGrass => "Dry Grass",
            Layer::ForestFloor => "Forest Floor",
            Layer::Dirt => "Dirt",
            Layer::Mud => "Mud",
            Layer::Gravel => "Gravel",
            Layer::Rock => "Rock",
            Layer::DarkRock => "Volcanic Rock",
            Layer::Sand => "Sand",
            Layer::Snow => "Snow",
            Layer::Cobblestone => "Cobblestone",
            Layer::Ice => "Ice",
        }
    }

    /// Representative sRGB swatch color (UI + surface overlay). Matches the shader palette.
    pub fn swatch(self) -> [u8; 3] {
        match self {
            Layer::Grass => [92, 122, 52],
            Layer::DryGrass => [150, 140, 84],
            Layer::ForestFloor => [78, 64, 42],
            Layer::Dirt => [116, 88, 60],
            Layer::Mud => [70, 56, 42],
            Layer::Gravel => [128, 124, 116],
            Layer::Rock => [112, 108, 102],
            Layer::DarkRock => [48, 45, 46],
            Layer::Sand => [212, 192, 148],
            Layer::Snow => [236, 240, 248],
            Layer::Cobblestone => [120, 116, 110],
            Layer::Ice => [176, 208, 228],
        }
    }

    /// The footstep surface this layer sounds like by default.
    pub fn default_surface(self) -> Surface {
        match self {
            Layer::Grass | Layer::DryGrass => Surface::Grass,
            Layer::ForestFloor => Surface::Leaves,
            Layer::Dirt => Surface::Dirt,
            Layer::Mud => Surface::Mud,
            Layer::Gravel => Surface::Gravel,
            Layer::Rock => Surface::Stone,
            Layer::DarkRock => Surface::Cinder,
            Layer::Sand => Surface::Sand,
            Layer::Snow => Surface::Snow,
            Layer::Cobblestone => Surface::Cobble,
            Layer::Ice => Surface::Ice,
        }
    }
}

/// Footstep surface classes. Each maps to a family of recorded footstep sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Surface {
    Grass,
    Leaves,
    Loam,
    Dirt,
    Mud,
    Gravel,
    Stone,
    WetStone,
    Cobble,
    Marble,
    Sand,
    Salt,
    Snow,
    Ice,
    Water,
    Wood,
    Metal,
    Ash,
    Cinder,
    Crystal,
    Bone,
    Carpet,
}

impl Surface {
    pub const ALL: [Surface; 22] = [
        Surface::Grass,
        Surface::Leaves,
        Surface::Loam,
        Surface::Dirt,
        Surface::Mud,
        Surface::Gravel,
        Surface::Stone,
        Surface::WetStone,
        Surface::Cobble,
        Surface::Marble,
        Surface::Sand,
        Surface::Salt,
        Surface::Snow,
        Surface::Ice,
        Surface::Water,
        Surface::Wood,
        Surface::Metal,
        Surface::Ash,
        Surface::Cinder,
        Surface::Crystal,
        Surface::Bone,
        Surface::Carpet,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Surface::Grass => "Grass",
            Surface::Leaves => "Leaves",
            Surface::Loam => "Loam",
            Surface::Dirt => "Dirt",
            Surface::Mud => "Mud",
            Surface::Gravel => "Gravel",
            Surface::Stone => "Stone",
            Surface::WetStone => "Wet Stone",
            Surface::Cobble => "Cobblestone",
            Surface::Marble => "Marble",
            Surface::Sand => "Sand",
            Surface::Salt => "Salt Flat",
            Surface::Snow => "Snow",
            Surface::Ice => "Ice",
            Surface::Water => "Shallow Water",
            Surface::Wood => "Wood",
            Surface::Metal => "Metal",
            Surface::Ash => "Ash",
            Surface::Cinder => "Cinder",
            Surface::Crystal => "Crystal",
            Surface::Bone => "Bone",
            Surface::Carpet => "Carpet / Cloth",
        }
    }

    /// File stem of the footstep recordings (`assets/audio/steps/step_<stem>_<n>.mp3`).
    pub fn sound_stem(self) -> &'static str {
        match self {
            Surface::Grass => "grass",
            Surface::Leaves => "leaves",
            Surface::Loam => "loam",
            Surface::Dirt => "dirt",
            Surface::Mud => "mud",
            Surface::Gravel => "gravel",
            Surface::Stone => "stone",
            Surface::WetStone => "wet_stone",
            Surface::Cobble => "cobble",
            Surface::Marble => "marble",
            Surface::Sand => "sand",
            Surface::Salt => "salt",
            Surface::Snow => "snow",
            Surface::Ice => "ice",
            Surface::Water => "water",
            Surface::Wood => "wood",
            Surface::Metal => "metal",
            Surface::Ash => "ash",
            Surface::Cinder => "cinder",
            Surface::Crystal => "crystal",
            Surface::Bone => "bone",
            Surface::Carpet => "carpet",
        }
    }

    /// Footstep takes per surface (`tools/audio/gen_footsteps.py` makes six: three per foot). The
    /// audio loader counts the files on disk and only falls back to this.
    pub fn variant_count(self) -> u32 {
        6
    }

    /// Distinct color for the footstep-surface overlay (sRGB).
    pub fn overlay_color(self) -> [u8; 3] {
        match self {
            Surface::Grass => [86, 196, 74],
            Surface::Leaves => [196, 120, 40],
            Surface::Loam => [120, 84, 50],
            Surface::Dirt => [160, 110, 70],
            Surface::Mud => [96, 70, 50],
            Surface::Gravel => [170, 170, 160],
            Surface::Stone => [120, 128, 140],
            Surface::WetStone => [80, 110, 140],
            Surface::Cobble => [200, 180, 150],
            Surface::Marble => [235, 230, 220],
            Surface::Sand => [240, 214, 120],
            Surface::Salt => [250, 250, 250],
            Surface::Snow => [210, 236, 255],
            Surface::Ice => [120, 220, 255],
            Surface::Water => [40, 120, 255],
            Surface::Wood => [190, 120, 60],
            Surface::Metal => [150, 150, 190],
            Surface::Ash => [100, 100, 100],
            Surface::Cinder => [200, 60, 40],
            Surface::Crystal => [200, 120, 255],
            Surface::Bone => [230, 220, 190],
            Surface::Carpet => [180, 40, 90],
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Guess a surface from a model / material name (used for imported models).
    pub fn guess_from_name(name: &str) -> Surface {
        let n = name.to_lowercase();
        let has = |keys: &[&str]| keys.iter().any(|k| n.contains(k));
        if has(&[
            "metal", "iron", "steel", "bronze", "copper", "grate", "anvil", "chain",
        ]) {
            Surface::Metal
        } else if has(&["marble", "temple", "palace", "statue"]) {
            Surface::Marble
        } else if has(&["crystal", "gem", "glass"]) {
            Surface::Crystal
        } else if has(&["bone", "skull", "skeleton"]) {
            Surface::Bone
        } else if has(&["carpet", "rug", "cloth", "tent", "canvas", "bed"]) {
            Surface::Carpet
        } else if has(&[
            "wood", "wooden", "plank", "bridge", "log", "dock", "pier", "crate", "barrel", "hut",
            "cabin", "tower", "fence", "boat", "ship", "deck", "timber", "tree", "stump",
        ]) {
            Surface::Wood
        } else if has(&["cobble", "road", "street", "paving"]) {
            Surface::Cobble
        } else if has(&["ice", "frozen"]) {
            Surface::Ice
        } else if has(&["snow"]) {
            Surface::Snow
        } else if has(&["sand", "dune"]) {
            Surface::Sand
        } else if has(&["grass", "moss", "hay"]) {
            Surface::Grass
        } else if has(&["dirt", "soil", "earth"]) {
            Surface::Dirt
        } else {
            Surface::Stone
        }
    }
}
