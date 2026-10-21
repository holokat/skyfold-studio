//! Dungeon styles: what walls, floors, ceilings, trims and pillars are made of, how the rooms are
//! roofed and trimmed, and which lights and mood suit them. Hand-painted styles use the
//! hand-painted pack's textures (`models/stylepacks/handpainted/textures`), photoreal ones the
//! photoscanned material and ground libraries (`textures/materials`, `textures/terrain_library`).

use super::lights::LightKind;
use super::plan::{Ceiling, RoomKind};

/// Where a texture comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tex {
    /// the hand-painted pack: `<name>.jpg` and `<name>_nor.jpg`
    Hp(&'static str),
    /// the photoscanned material library: `<name>/albedo.png`, `normal.png`, `orm.png`
    Scan(&'static str),
    /// the photoscanned ground library: `<name>/albedo.png`, `normal.png`
    Ground(&'static str),
}

impl Tex {
    /// (albedo, normal, occlusion-roughness-metal) asset paths.
    pub fn paths(self) -> (String, Option<String>, Option<String>) {
        match self {
            Tex::Hp(n) => (
                format!("models/stylepacks/handpainted/textures/{n}.jpg"),
                Some(format!(
                    "models/stylepacks/handpainted/textures/{n}_nor.jpg"
                )),
                None,
            ),
            Tex::Scan(n) => (
                format!("textures/materials/{n}/albedo.png"),
                Some(format!("textures/materials/{n}/normal.png")),
                Some(format!("textures/materials/{n}/orm.png")),
            ),
            Tex::Ground(n) => (
                format!("textures/terrain_library/{n}/albedo.png"),
                Some(format!("textures/terrain_library/{n}/normal.png")),
                None,
            ),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Tex::Hp(n) | Tex::Scan(n) | Tex::Ground(n) => n,
        }
    }
    /// Its depth map for parallax (white deep; `tools/dungeons/make_depth_maps.py`).
    pub fn depth_path(self) -> String {
        match self {
            Tex::Hp(n) => format!("models/stylepacks/handpainted/textures/{n}_depth.png"),
            Tex::Scan(n) => format!("textures/materials/{n}/depth.png"),
            Tex::Ground(n) => format!("textures/terrain_library/{n}/depth.png"),
        }
    }
}

/// The moss an art grows on its stone: the texture, and whether it carries its own coverage in
/// its alpha (the painted overlay) or covers where it's put (the scanned moss).
pub fn moss_texture(art: Art) -> (&'static str, bool) {
    match art {
        Art::HandPainted => (
            "models/stylepacks/handpainted/textures/moss_overlay.png",
            true,
        ),
        Art::Photoreal => ("textures/terrain_library/moss/albedo.png", false),
    }
}

/// A surface: its texture, how big one repeat is, a tint, how rough and metallic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat {
    pub tex: Tex,
    /// metres per texture repeat
    pub tile: f32,
    pub tint: [f32; 3],
    pub rough: f32,
    pub metal: f32,
}

const fn m(tex: Tex, tile: f32) -> Mat {
    Mat {
        tex,
        tile,
        tint: [1.0, 1.0, 1.0],
        rough: 0.9,
        metal: 0.0,
    }
}

const fn mt(tex: Tex, tile: f32, tint: [f32; 3]) -> Mat {
    Mat {
        tex,
        tile,
        tint,
        rough: 0.9,
        metal: 0.0,
    }
}

const fn metal(tex: Tex, tile: f32) -> Mat {
    Mat {
        tex,
        tile,
        tint: [1.0, 1.0, 1.0],
        rough: 0.5,
        metal: 0.6,
    }
}

/// The art a dungeon is made in: hand-painted (the hand-painted pack's textures and models) or
/// photoreal (photoscanned materials and models). Picked first; a dungeon's styles, fixtures and
/// dressing all come from its art.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Art {
    #[default]
    HandPainted,
    Photoreal,
}

impl Art {
    pub const ALL: [Art; 2] = [Art::HandPainted, Art::Photoreal];
    pub fn name(self) -> &'static str {
        match self {
            Art::HandPainted => "Hand-painted",
            Art::Photoreal => "Photoreal",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Art::HandPainted => "handpainted",
            Art::Photoreal => "photoreal",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| {
            a.id() == s
                || a.name().eq_ignore_ascii_case(s)
                || (s == "hand_painted" && *a == Art::HandPainted)
        })
    }
    /// Ready to build with (photoreal dungeons are coming: their photoscanned kit isn't done).
    pub fn ready(self) -> bool {
        self == Art::HandPainted
    }
}

/// Which surface of a style a mesh is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Wall,
    WallLower,
    Floor,
    Ceiling,
    Trim,
    Pillar,
    Beam,
    Rock,
    RockFloor,
    /// the dark section where the cut-away slices a wall
    Cap,
}

impl Slot {
    pub const ALL: [Slot; 10] = [
        Slot::Wall,
        Slot::WallLower,
        Slot::Floor,
        Slot::Ceiling,
        Slot::Trim,
        Slot::Pillar,
        Slot::Beam,
        Slot::Rock,
        Slot::RockFloor,
        Slot::Cap,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillarShape {
    Square,
    Round,
    Octagon,
}

/// A style.
#[derive(Clone, Debug)]
pub struct Style {
    pub id: &'static str,
    pub name: &'static str,
    /// "handpainted" or "photoreal"
    pub pack: &'static str,
    pub blurb: &'static str,
    pub wall: Mat,
    /// a different lower band of the walls (wainscot) and its height
    pub lower: Option<(Mat, f32)>,
    pub floor: Mat,
    pub ceiling: Mat,
    pub trim: Mat,
    pub pillar: Mat,
    pub beam: Mat,
    /// caverns
    pub rock: Mat,
    pub rock_floor: Mat,
    pub pillar_shape: PillarShape,
    /// pillar half width (m)
    pub pillar_r: f32,
    /// a skirting along the foot of the walls, a cornice along their top
    pub plinth: bool,
    pub cornice: bool,
    /// a pilaster every this many metres along straight walls (0: none)
    pub pilasters: f32,
    /// ceilings of rooms, halls and corridors made in this style
    pub ceilings: [Ceiling; 3],
    /// frames round doorways
    pub frames: bool,
    /// the light fixture Auto-light hangs on the walls, and the big one for halls
    pub wall_light: LightKind,
    pub hall_light: LightKind,
    /// the mood it suggests (`mood::preset`)
    pub mood: &'static str,
    /// a swatch colour for the picker
    pub swatch: [u8; 3],
}

impl Style {
    pub fn art(&self) -> Art {
        if self.pack == "handpainted" {
            Art::HandPainted
        } else {
            Art::Photoreal
        }
    }
    pub fn mat(&self, s: Slot) -> Mat {
        match s {
            Slot::Wall => self.wall,
            Slot::WallLower => self.lower.map(|l| l.0).unwrap_or(self.wall),
            Slot::Floor => self.floor,
            Slot::Ceiling => self.ceiling,
            Slot::Trim => self.trim,
            Slot::Pillar => self.pillar,
            Slot::Beam => self.beam,
            Slot::Rock => self.rock,
            Slot::RockFloor => self.rock_floor,
            Slot::Cap => Mat {
                tint: [0.32, 0.31, 0.3],
                ..self.trim
            },
        }
    }
    /// The ceiling a new room of `kind` gets.
    pub fn ceiling_for(&self, kind: RoomKind) -> Ceiling {
        match kind {
            RoomKind::Cavern => Ceiling::Rock,
            RoomKind::Hall => self.ceilings[1],
            RoomKind::Corridor | RoomKind::Stairs => self.ceilings[2],
            _ => self.ceilings[0],
        }
    }
}

use Tex::{Ground, Hp, Scan};

pub fn all() -> &'static [Style] {
    use std::sync::OnceLock;
    static ALL: OnceLock<Vec<Style>> = OnceLock::new();
    ALL.get_or_init(|| {
        vec![
            Style {
                id: "crypt_stone",
                name: "Stone Crypt",
                pack: "handpainted",
                blurb: "Hand-painted grey blocks, cobbled floors, round vaults, torches",
                wall: m(Hp("stone_wall_blocks"), 2.6),
                lower: Some((m(Hp("dark_stone"), 2.2), 1.1)),
                floor: m(Hp("cobblestone_path"), 2.2),
                ceiling: mt(Hp("stone_wall_blocks"), 2.6, [0.82, 0.8, 0.78]),
                trim: m(Hp("dark_stone"), 1.6),
                pillar: m(Hp("dark_stone"), 1.8),
                beam: m(Hp("wood_beam"), 1.3),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Square,
                pillar_r: 0.55,
                plinth: true,
                cornice: true,
                pilasters: 6.0,
                ceilings: [Ceiling::Vault, Ceiling::Vault, Ceiling::Vault],
                frames: true,
                wall_light: LightKind::Torch,
                hall_light: LightKind::Brazier,
                mood: "torchlit",
                swatch: [128, 124, 118],
            },
            Style {
                id: "keep_stone",
                name: "Keep Dungeon",
                pack: "handpainted",
                blurb: "Hand-painted castle stone, cobbles, timber-beamed cells and halls",
                wall: mt(Hp("stone_wall_blocks"), 2.4, [0.9, 0.9, 0.94]),
                lower: Some((m(Hp("dark_stone"), 2.0), 1.0)),
                floor: m(Hp("blooddrum_cobble"), 2.2),
                ceiling: m(Hp("wood_planks"), 1.8),
                trim: m(Hp("dark_stone"), 1.6),
                pillar: m(Hp("stone_wall_blocks"), 2.0),
                beam: m(Hp("wood_beam"), 1.3),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Square,
                pillar_r: 0.6,
                plinth: true,
                cornice: false,
                pilasters: 7.0,
                ceilings: [Ceiling::Beams, Ceiling::Beams, Ceiling::Vault],
                frames: true,
                wall_light: LightKind::Torch,
                hall_light: LightKind::Chandelier,
                mood: "prison",
                swatch: [122, 116, 108],
            },
            Style {
                id: "cathedral_hp",
                name: "Moonlit Cathedral",
                pack: "handpainted",
                blurb: "Pale hand-painted stone, marble floors and trims, soaring pointed vaults",
                wall: m(Hp("grave_stone"), 2.4),
                lower: Some((m(Hp("dark_stone"), 2.0), 1.4)),
                floor: mt(Hp("marble_white"), 3.0, [0.72, 0.74, 0.78]),
                ceiling: mt(Hp("grave_stone"), 2.6, [0.7, 0.72, 0.78]),
                trim: m(Hp("marble_white"), 1.8),
                pillar: m(Hp("grave_stone"), 2.0),
                beam: m(Hp("wood_beam"), 1.3),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Octagon,
                pillar_r: 0.8,
                plinth: true,
                cornice: true,
                pilasters: 5.0,
                ceilings: [Ceiling::Pointed, Ceiling::Pointed, Ceiling::Pointed],
                frames: true,
                wall_light: LightKind::Lantern,
                hall_light: LightKind::Chandelier,
                mood: "moonlit",
                swatch: [150, 150, 160],
            },
            Style {
                id: "sewer_hp",
                name: "Brick Sewers",
                pack: "handpainted",
                blurb: "Hand-painted red brick barrel vaults, dark footings, wet cobbles",
                wall: m(Hp("brick_red"), 1.8),
                lower: Some((mt(Hp("dark_stone"), 1.8, [0.7, 0.8, 0.7]), 1.0)),
                floor: Mat {
                    rough: 0.45,
                    ..mt(Hp("cobblestone_path"), 2.0, [0.7, 0.75, 0.7])
                },
                ceiling: m(Hp("brick_red"), 1.8),
                trim: m(Hp("dark_stone"), 1.6),
                pillar: m(Hp("brick_red"), 1.8),
                beam: metal(Hp("iron_trim"), 1.0),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Square,
                pillar_r: 0.65,
                plinth: false,
                cornice: false,
                pilasters: 8.0,
                ceilings: [Ceiling::Vault, Ceiling::Vault, Ceiling::Vault],
                frames: false,
                wall_light: LightKind::Lantern,
                hall_light: LightKind::Lantern,
                mood: "damp",
                swatch: [140, 76, 60],
            },
            Style {
                id: "cave_rock",
                name: "Rock Cave",
                pack: "handpainted",
                blurb: "Hand-painted rough rock and packed earth (for cave dungeons)",
                wall: m(Hp("rough_rock"), 3.2),
                lower: None,
                floor: m(Hp("dirt_ground"), 2.4),
                ceiling: m(Hp("rough_rock"), 3.2),
                trim: m(Hp("rough_rock"), 2.6),
                pillar: m(Hp("rough_rock"), 2.6),
                beam: m(Hp("log_bark"), 1.6),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Round,
                pillar_r: 1.1,
                plinth: false,
                cornice: false,
                pilasters: 0.0,
                ceilings: [Ceiling::Rock, Ceiling::Rock, Ceiling::Rock],
                frames: false,
                wall_light: LightKind::Torch,
                hall_light: LightKind::Crystal,
                mood: "cave",
                swatch: [112, 100, 88],
            },
            Style {
                id: "clockwork_hp",
                name: "Brass Works",
                pack: "handpainted",
                blurb: "Hand-painted iron plate, copper and brass, work lamps",
                wall: Mat {
                    rough: 0.55,
                    metal: 0.45,
                    ..m(Hp("iron_grey"), 2.2)
                },
                lower: Some((
                    Mat {
                        rough: 0.5,
                        metal: 0.5,
                        ..m(Hp("copper_metal"), 1.6)
                    },
                    1.2,
                )),
                floor: m(Hp("ferry_pavers"), 2.4),
                ceiling: Mat {
                    rough: 0.6,
                    metal: 0.4,
                    ..mt(Hp("iron_grey"), 2.4, [0.8, 0.8, 0.82])
                },
                trim: Mat {
                    rough: 0.4,
                    metal: 0.6,
                    ..m(Hp("bronze_trim"), 1.0)
                },
                pillar: Mat {
                    rough: 0.45,
                    metal: 0.55,
                    ..m(Hp("copper_metal"), 1.6)
                },
                beam: metal(Hp("iron_trim"), 1.0),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Round,
                pillar_r: 0.65,
                plinth: true,
                cornice: true,
                pilasters: 5.0,
                ceilings: [Ceiling::Beams, Ceiling::Beams, Ceiling::Flat],
                frames: true,
                wall_light: LightKind::WorkLamp,
                hall_light: LightKind::WorkLamp,
                mood: "industrial",
                swatch: [120, 104, 86],
            },
            Style {
                id: "nordic_hp",
                name: "Timber Hall",
                pack: "handpainted",
                blurb: "Hand-painted log walls on stone footings, plank floors, carved beams",
                wall: m(Hp("log_bark"), 2.0),
                lower: Some((m(Hp("stone_wall_blocks"), 2.2), 1.4)),
                floor: m(Hp("wood_planks"), 1.6),
                ceiling: m(Hp("wood_planks"), 1.6),
                trim: m(Hp("wood_beam"), 1.3),
                pillar: m(Hp("log_bark"), 1.8),
                beam: m(Hp("wood_beam"), 1.3),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Round,
                pillar_r: 0.5,
                plinth: false,
                cornice: false,
                pilasters: 4.0,
                ceilings: [Ceiling::Beams, Ceiling::Pointed, Ceiling::Beams],
                frames: true,
                wall_light: LightKind::Torch,
                hall_light: LightKind::HangingBrazier,
                mood: "torchlit",
                swatch: [130, 96, 66],
            },
            Style {
                id: "overgrown_hp",
                name: "Overgrown Ruins",
                pack: "handpainted",
                blurb: "Hand-painted mossy stone, earthen floors, shafts of green light",
                wall: mt(Hp("stone_wall_blocks"), 2.6, [0.82, 0.92, 0.78]),
                lower: Some((mt(Hp("rough_rock"), 2.6, [0.8, 0.95, 0.75]), 0.8)),
                floor: m(Hp("grass_ground"), 2.2),
                ceiling: mt(Hp("stone_wall_blocks"), 2.6, [0.75, 0.85, 0.72]),
                trim: m(Hp("grave_stone"), 1.8),
                pillar: mt(Hp("stone_wall_blocks"), 2.2, [0.82, 0.92, 0.78]),
                beam: m(Hp("log_bark"), 1.6),
                rock: m(Hp("rough_rock"), 3.2),
                rock_floor: m(Hp("dirt_ground"), 2.4),
                pillar_shape: PillarShape::Round,
                pillar_r: 0.75,
                plinth: true,
                cornice: false,
                pilasters: 0.0,
                ceilings: [Ceiling::Vault, Ceiling::Open, Ceiling::Vault],
                frames: true,
                wall_light: LightKind::Lantern,
                hall_light: LightKind::Shaft,
                mood: "overgrown",
                swatch: [104, 124, 90],
            },
            Style {
                id: "castle_keep",
                name: "Castle Dungeon (photoreal)",
                pack: "photoreal",
                blurb: "Photoscanned castle stone, flagstones, timber-beamed cells and halls",
                wall: m(Scan("stone_castle_wall"), 3.0),
                lower: Some((m(Scan("stone_rubble_wall"), 2.4), 0.9)),
                floor: m(Scan("stone_flagstone_floor"), 2.6),
                ceiling: m(Scan("stone_blocks_grey"), 2.6),
                trim: m(Scan("stone_ashlar_dark"), 2.0),
                pillar: m(Scan("stone_ashlar_dark"), 2.0),
                beam: m(Scan("wood_planks_dark"), 1.6),
                rock: m(Ground("dark_layered_rock"), 4.0),
                rock_floor: m(Ground("grey_scree"), 3.0),
                pillar_shape: PillarShape::Square,
                pillar_r: 0.8,
                plinth: true,
                cornice: false,
                pilasters: 7.0,
                ceilings: [Ceiling::Beams, Ceiling::Beams, Ceiling::Vault],
                frames: true,
                wall_light: LightKind::Torch,
                hall_light: LightKind::Chandelier,
                mood: "torchlit",
                swatch: [110, 104, 96],
            },
            Style {
                id: "gothic_cathedral",
                name: "Gothic Cathedral (photoreal)",
                pack: "photoreal",
                blurb: "Dark ashlar, slate floors, soaring pointed vaults, cold light",
                wall: m(Scan("stone_ashlar_dark"), 2.8),
                lower: Some((m(Scan("stone_blocks_grey"), 2.4), 1.4)),
                floor: m(Scan("stone_slate_floor"), 2.4),
                ceiling: m(Scan("stone_blocks_grey"), 3.0),
                trim: m(Scan("stone_marble_white"), 2.0),
                pillar: m(Scan("stone_ashlar_dark"), 2.2),
                beam: m(Scan("wood_planks_dark"), 1.6),
                rock: m(Ground("grey_granite"), 4.0),
                rock_floor: m(Ground("grey_scree"), 3.0),
                pillar_shape: PillarShape::Octagon,
                pillar_r: 0.9,
                plinth: true,
                cornice: true,
                pilasters: 5.0,
                ceilings: [Ceiling::Pointed, Ceiling::Pointed, Ceiling::Pointed],
                frames: true,
                wall_light: LightKind::Lantern,
                hall_light: LightKind::Chandelier,
                mood: "moonlit",
                swatch: [72, 74, 86],
            },
            Style {
                id: "sun_temple",
                name: "Sun Temple",
                pack: "handpainted",
                blurb: "Jungle-temple sandstone, mosaic floors, gold trims, stepped vaults",
                wall: mt(Hp("hourglass_sandblock"), 2.4, [0.86, 0.95, 0.92]),
                lower: Some((m(Hp("dragonbone_turquoise"), 1.8), 1.0)),
                floor: m(Hp("turtleshell_mosaic"), 3.0),
                ceiling: mt(Hp("hourglass_sandstone"), 2.6, [0.8, 0.86, 0.84]),
                trim: Mat {
                    rough: 0.5,
                    metal: 0.4,
                    ..m(Hp("gold_trim"), 1.6)
                },
                pillar: m(Hp("hourglass_sandblock"), 2.0),
                beam: m(Hp("hourglass_sandstone"), 2.0),
                rock: m(Hp("turtleshell_rock"), 3.2),
                rock_floor: m(Hp("turtleshell_sand"), 2.4),
                pillar_shape: PillarShape::Square,
                pillar_r: 1.1,
                plinth: true,
                cornice: true,
                pilasters: 5.0,
                ceilings: [Ceiling::Stepped, Ceiling::Stepped, Ceiling::Stepped],
                frames: true,
                wall_light: LightKind::Brazier,
                hall_light: LightKind::Brazier,
                mood: "golden",
                swatch: [196, 172, 112],
            },
            Style {
                id: "dwarven_forge",
                name: "Dwarven Forge",
                pack: "handpainted",
                blurb: "Dark carved stone, bronze bands, heavy octagonal pillars, forge glow",
                wall: m(Hp("dark_stone"), 2.4),
                lower: Some((m(Hp("iron_grey"), 1.6), 1.2)),
                floor: m(Hp("ferry_pavers"), 2.6),
                ceiling: mt(Hp("dark_stone"), 2.6, [0.75, 0.72, 0.7]),
                trim: Mat {
                    rough: 0.45,
                    metal: 0.5,
                    ..m(Hp("bronze_trim"), 1.0)
                },
                pillar: m(Hp("dark_stone"), 2.0),
                beam: metal(Hp("iron_trim"), 1.0),
                rock: m(Hp("dragonbone_rock"), 3.4),
                rock_floor: m(Hp("siegebreaker_dirt"), 2.4),
                pillar_shape: PillarShape::Octagon,
                pillar_r: 1.3,
                plinth: true,
                cornice: true,
                pilasters: 6.0,
                ceilings: [Ceiling::Beams, Ceiling::Flat, Ceiling::Stepped],
                frames: true,
                wall_light: LightKind::Brazier,
                hall_light: LightKind::HangingBrazier,
                mood: "forge",
                swatch: [88, 70, 58],
            },
            Style {
                id: "nordic_keep",
                name: "Nordic Keep (photoreal)",
                pack: "photoreal",
                blurb: "Dark log walls on rubble footings, plank floors, timber halls",
                wall: m(Scan("wood_log_wall_dark"), 2.6),
                lower: Some((m(Scan("stone_rubble_wall"), 2.4), 1.6)),
                floor: m(Scan("wood_planks_medieval"), 2.2),
                ceiling: m(Scan("wood_planks_dark"), 2.2),
                trim: m(Scan("wood_rough_sawn_dark"), 1.6),
                pillar: m(Scan("wood_bark_oak"), 2.0),
                beam: m(Scan("wood_rough_sawn_dark"), 1.6),
                rock: m(Ground("lichen_rock"), 4.0),
                rock_floor: m(Ground("dark_gravel"), 3.0),
                pillar_shape: PillarShape::Round,
                pillar_r: 0.55,
                plinth: false,
                cornice: false,
                pilasters: 4.0,
                ceilings: [Ceiling::Beams, Ceiling::Pointed, Ceiling::Beams],
                frames: true,
                wall_light: LightKind::Torch,
                hall_light: LightKind::HangingBrazier,
                mood: "torchlit",
                swatch: [92, 70, 52],
            },
            Style {
                id: "sewer_brick",
                name: "Old Sewers (photoreal)",
                pack: "photoreal",
                blurb: "Old brick barrel vaults, mossy footings, wet slate",
                wall: m(Scan("brick_castle_old"), 2.0),
                lower: Some((m(Scan("brick_mossy"), 2.0), 1.0)),
                floor: Mat {
                    rough: 0.4,
                    ..m(Scan("stone_slate_floor"), 2.4)
                },
                ceiling: m(Scan("brick_castle_old"), 2.0),
                trim: m(Scan("stone_blocks_grey"), 2.0),
                pillar: m(Scan("brick_castle_old"), 2.0),
                beam: metal(Scan("metal_rust_medium"), 1.6),
                rock: m(Ground("dark_layered_rock"), 4.0),
                rock_floor: m(Ground("forest_mud"), 3.0),
                pillar_shape: PillarShape::Square,
                pillar_r: 0.7,
                plinth: false,
                cornice: false,
                pilasters: 8.0,
                ceilings: [Ceiling::Vault, Ceiling::Vault, Ceiling::Vault],
                frames: false,
                wall_light: LightKind::Lantern,
                hall_light: LightKind::Lantern,
                mood: "damp",
                swatch: [120, 82, 66],
            },
            Style {
                id: "frost_halls",
                name: "Frost Halls",
                pack: "handpainted",
                blurb: "Pale marble glazed with frost, steel bands, cold crystal light",
                wall: mt(Hp("marble_white"), 2.6, [0.82, 0.9, 1.0]),
                lower: Some((mt(Hp("dark_stone"), 2.0, [0.7, 0.8, 0.95]), 0.8)),
                floor: Mat {
                    rough: 0.35,
                    ..mt(Hp("marble_white"), 3.0, [0.78, 0.86, 0.96])
                },
                ceiling: mt(Hp("marble_white"), 3.0, [0.7, 0.8, 0.95]),
                trim: Mat {
                    rough: 0.35,
                    metal: 0.5,
                    ..m(Hp("steel_trim"), 1.0)
                },
                pillar: mt(Hp("marble_white"), 2.2, [0.8, 0.88, 1.0]),
                beam: metal(Hp("steel_trim"), 1.0),
                rock: mt(Ground("frozen_crust"), 4.0, [0.85, 0.92, 1.0]),
                rock_floor: m(Ground("fresh_snow"), 3.0),
                pillar_shape: PillarShape::Octagon,
                pillar_r: 0.9,
                plinth: true,
                cornice: true,
                pilasters: 6.0,
                ceilings: [Ceiling::Vault, Ceiling::Pointed, Ceiling::Vault],
                frames: true,
                wall_light: LightKind::Crystal,
                hall_light: LightKind::Crystal,
                mood: "frost",
                swatch: [196, 214, 236],
            },
            Style {
                id: "overgrown_ruins",
                name: "Overgrown Ruins (photoreal)",
                pack: "photoreal",
                blurb: "Mossy stone, broken flagstones, shafts of green light",
                wall: m(Scan("stone_wall_mossy"), 3.0),
                lower: None,
                floor: m(Ground("grassy_flagstones"), 3.0),
                ceiling: m(Scan("stone_blocks_grey"), 3.0),
                trim: m(Scan("stone_blocks_grey"), 2.0),
                pillar: m(Scan("stone_wall_mossy"), 2.4),
                beam: m(Scan("wood_planks_rotten"), 1.6),
                rock: m(Ground("lichen_rock"), 4.0),
                rock_floor: m(Ground("forest_soil_debris"), 3.0),
                pillar_shape: PillarShape::Round,
                pillar_r: 0.8,
                plinth: true,
                cornice: false,
                pilasters: 0.0,
                ceilings: [Ceiling::Vault, Ceiling::Open, Ceiling::Vault],
                frames: true,
                wall_light: LightKind::Lantern,
                hall_light: LightKind::Shaft,
                mood: "overgrown",
                swatch: [96, 110, 80],
            },
            Style {
                id: "clockwork_works",
                name: "Clockwork Works (photoreal)",
                pack: "photoreal",
                blurb: "Riveted plate walls, diamond-plate floors, brass, work lamps",
                wall: Mat {
                    rough: 0.55,
                    metal: 0.55,
                    ..m(Scan("metal_plates_riveted"), 3.0)
                },
                lower: Some((
                    Mat {
                        rough: 0.7,
                        metal: 0.4,
                        ..m(Scan("metal_rust_medium"), 2.0)
                    },
                    1.2,
                )),
                floor: Mat {
                    rough: 0.5,
                    metal: 0.6,
                    ..m(Scan("metal_diamond_plate"), 2.0)
                },
                ceiling: Mat {
                    rough: 0.6,
                    metal: 0.5,
                    ..m(Scan("metal_corrugated_rusty"), 2.4)
                },
                trim: Mat {
                    rough: 0.4,
                    metal: 0.7,
                    ..m(Scan("metal_brass_tarnished"), 1.4)
                },
                pillar: Mat {
                    rough: 0.5,
                    metal: 0.6,
                    ..m(Scan("metal_iron_cast"), 2.0)
                },
                beam: Mat {
                    rough: 0.5,
                    metal: 0.6,
                    ..m(Scan("metal_iron_wrought"), 1.6)
                },
                rock: m(Ground("dark_layered_rock"), 4.0),
                rock_floor: m(Ground("dark_gravel"), 3.0),
                pillar_shape: PillarShape::Round,
                pillar_r: 0.7,
                plinth: true,
                cornice: true,
                pilasters: 5.0,
                ceilings: [Ceiling::Beams, Ceiling::Beams, Ceiling::Flat],
                frames: true,
                wall_light: LightKind::WorkLamp,
                hall_light: LightKind::WorkLamp,
                mood: "industrial",
                swatch: [96, 104, 112],
            },
            Style {
                id: "sandstone_tomb",
                name: "Sandstone Tomb",
                pack: "handpainted",
                blurb: "Desert sandstone, lapis and brass, sand-drifted floors",
                wall: m(Hp("hourglass_sandwall"), 2.6),
                lower: Some((m(Hp("hourglass_lapis"), 1.6), 0.7)),
                floor: m(Hp("hourglass_sandstone"), 2.6),
                ceiling: m(Hp("hourglass_sandstone"), 2.8),
                trim: Mat {
                    rough: 0.45,
                    metal: 0.5,
                    ..m(Hp("hourglass_brass"), 1.2)
                },
                pillar: m(Hp("hourglass_sandblock"), 2.2),
                beam: m(Hp("hourglass_sandstone"), 2.0),
                rock: m(Hp("dragonbone_sandstone"), 3.2),
                rock_floor: m(Hp("sand_pale"), 2.4),
                pillar_shape: PillarShape::Square,
                pillar_r: 0.9,
                plinth: true,
                cornice: true,
                pilasters: 5.0,
                ceilings: [Ceiling::Flat, Ceiling::Stepped, Ceiling::Flat],
                frames: true,
                wall_light: LightKind::Brazier,
                hall_light: LightKind::Brazier,
                mood: "golden",
                swatch: [206, 176, 128],
            },
            Style {
                id: "spider_den",
                name: "Spider Den",
                pack: "handpainted",
                blurb: "Dark chitin-brown stone, violet gloom, webs (Nerubian-like)",
                wall: mt(Hp("dragonbone_rock"), 3.0, [0.7, 0.62, 0.78]),
                lower: Some((mt(Hp("dark_stone"), 2.0, [0.62, 0.55, 0.72]), 1.4)),
                floor: mt(Hp("siegebreaker_dirt"), 2.6, [0.7, 0.62, 0.74]),
                ceiling: mt(Hp("dragonbone_rock"), 3.0, [0.55, 0.5, 0.66]),
                trim: Mat {
                    rough: 0.5,
                    metal: 0.4,
                    ..mt(Hp("bronze_trim"), 1.2, [0.8, 0.6, 0.5])
                },
                pillar: mt(Hp("dark_stone"), 2.0, [0.7, 0.6, 0.8]),
                beam: mt(Hp("bone"), 1.2, [0.85, 0.8, 0.85]),
                rock: mt(Hp("dragonbone_rock"), 3.4, [0.6, 0.55, 0.72]),
                rock_floor: mt(Hp("siegebreaker_dirt"), 2.4, [0.6, 0.55, 0.66]),
                pillar_shape: PillarShape::Octagon,
                pillar_r: 1.0,
                plinth: true,
                cornice: false,
                pilasters: 7.0,
                ceilings: [Ceiling::Pointed, Ceiling::Pointed, Ceiling::Pointed],
                frames: true,
                wall_light: LightKind::Crystal,
                hall_light: LightKind::Crystal,
                mood: "void",
                swatch: [84, 66, 104],
            },
            Style {
                id: "infernal_halls",
                name: "Infernal Halls",
                pack: "handpainted",
                blurb: "Scorched dark stone, ember-red light, iron and bone",
                wall: mt(Hp("dark_stone"), 2.4, [0.82, 0.62, 0.56]),
                lower: Some((mt(Hp("iron_grey"), 1.6, [0.7, 0.55, 0.5]), 1.0)),
                floor: mt(Hp("cindermaw_stone_light"), 2.6, [0.75, 0.6, 0.55]),
                ceiling: mt(Hp("dark_stone"), 2.6, [0.6, 0.48, 0.44]),
                trim: metal(Hp("iron_trim"), 1.0),
                pillar: mt(Hp("dark_stone"), 2.0, [0.75, 0.55, 0.5]),
                beam: m(Hp("bone"), 1.2),
                rock: m(Ground("dark_volcanic_rock"), 4.0),
                rock_floor: m(Ground("burned_ground"), 3.0),
                pillar_shape: PillarShape::Square,
                pillar_r: 1.0,
                plinth: true,
                cornice: true,
                pilasters: 6.0,
                ceilings: [Ceiling::Pointed, Ceiling::Open, Ceiling::Pointed],
                frames: true,
                wall_light: LightKind::Brazier,
                hall_light: LightKind::HangingBrazier,
                mood: "infernal",
                swatch: [120, 52, 40],
            },
            Style {
                id: "natural_cave",
                name: "Natural Cave (photoreal)",
                pack: "photoreal",
                blurb: "Photoscanned layered rock and scree (for cave dungeons)",
                wall: m(Ground("dark_layered_rock"), 4.0),
                lower: None,
                floor: m(Ground("grey_scree"), 3.0),
                ceiling: m(Ground("dark_layered_rock"), 4.0),
                trim: m(Ground("dark_layered_rock"), 3.0),
                pillar: m(Ground("dark_layered_rock"), 3.0),
                beam: m(Scan("wood_rough_sawn_dark"), 1.6),
                rock: m(Ground("dark_layered_rock"), 4.0),
                rock_floor: m(Ground("grey_scree"), 3.0),
                pillar_shape: PillarShape::Round,
                pillar_r: 1.2,
                plinth: false,
                cornice: false,
                pilasters: 0.0,
                ceilings: [Ceiling::Rock, Ceiling::Rock, Ceiling::Rock],
                frames: false,
                wall_light: LightKind::Torch,
                hall_light: LightKind::Crystal,
                mood: "cave",
                swatch: [70, 66, 60],
            },
        ]
    })
}

/// The styles of an art.
pub fn of(art: Art) -> impl Iterator<Item = &'static Style> {
    all().iter().filter(move |s| s.art() == art)
}

pub fn get(id: &str) -> &'static Style {
    all()
        .iter()
        .find(|s| s.id == id || s.name.eq_ignore_ascii_case(id))
        .unwrap_or(&all()[0])
}

/// What a style's floor sounds like underfoot (its rock floor in caverns).
pub fn footstep(id: &str, cavern: bool) -> crate::terrain::layers::Surface {
    use crate::terrain::layers::Surface as S;
    match (id, cavern) {
        ("infernal_halls", _) => S::Cinder,
        ("frost_halls", _) => S::Ice,
        ("spider_den", _) => S::Dirt,
        ("sewer_hp" | "sewer_brick", _) => S::WetStone,
        (_, true) => S::Gravel,
        ("clockwork_hp" | "clockwork_works", _) => S::Metal,
        ("cathedral_hp" | "gothic_cathedral", _) => S::Marble,
        ("crypt_stone" | "keep_stone" | "castle_keep", _) => S::Cobble,
        ("cave_rock" | "natural_cave", _) => S::Gravel,
        _ => S::Stone,
    }
}

impl Style {
    /// How much rubble lies at the foot of its walls (0 none … ~1 a ruin).
    pub fn rubble(&self) -> f32 {
        match self.id {
            "overgrown_hp" | "overgrown_ruins" => 1.0,
            "cave_rock" | "natural_cave" | "spider_den" => 0.8,
            "crypt_stone" | "keep_stone" | "castle_keep" | "sandstone_tomb" | "sun_temple" => 0.5,
            "sewer_hp" | "sewer_brick" | "nordic_hp" | "nordic_keep" | "infernal_halls" => 0.4,
            "dwarven_forge" | "frost_halls" => 0.3,
            "cathedral_hp" | "gothic_cathedral" | "clockwork_hp" | "clockwork_works" => 0.15,
            _ => 0.4,
        }
    }
    /// How weathered its places are: moss, grime, wet, puddles (0 none … ~1 a lot).
    pub fn weathering(&self) -> [f32; 4] {
        match self.id {
            "sewer_hp" | "sewer_brick" => [0.65, 0.75, 0.8, 0.9],
            "overgrown_hp" | "overgrown_ruins" => [1.0, 0.4, 0.35, 0.4],
            "cave_rock" | "natural_cave" => [0.45, 0.35, 0.5, 0.6],
            "sun_temple" => [0.6, 0.35, 0.2, 0.25],
            "crypt_stone" => [0.25, 0.6, 0.15, 0.15],
            "keep_stone" | "castle_keep" => [0.3, 0.5, 0.2, 0.3],
            "cathedral_hp" | "gothic_cathedral" => [0.15, 0.35, 0.1, 0.1],
            "nordic_hp" | "nordic_keep" => [0.35, 0.45, 0.2, 0.25],
            "clockwork_hp" | "clockwork_works" => [0.0, 0.85, 0.15, 0.35],
            "dwarven_forge" => [0.0, 0.8, 0.0, 0.0],
            "infernal_halls" => [0.0, 0.9, 0.0, 0.0],
            "frost_halls" => [0.0, 0.2, 0.55, 0.0],
            "sandstone_tomb" => [0.0, 0.45, 0.0, 0.0],
            "spider_den" => [0.2, 0.7, 0.2, 0.15],
            _ => [0.2, 0.45, 0.15, 0.2],
        }
    }
}

pub fn index(id: &str) -> usize {
    all()
        .iter()
        .position(|s| s.id == id || s.name.eq_ignore_ascii_case(id))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_texture_is_bundled() {
        let assets = std::path::PathBuf::from(crate::assets_path());
        for s in all() {
            for slot in Slot::ALL {
                let (a, n, o) = s.mat(slot).tex.paths();
                for p in std::iter::once(a).chain(n).chain(o) {
                    assert!(assets.join(&p).is_file(), "{}: {p} missing", s.id);
                }
            }
        }
    }
}
