//! What a cave is, as saved with the world: its recipe (kind, size, levels, theme, features,
//! seed), where its entrance is and the chambers and tunnels laid out for it. Everything drawn —
//! the rock, the hole in the ground, the stalactites, the lights — is derived from this.

use serde::{Deserialize, Serialize};

/// What sort of hole in the world it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaveKind {
    /// A natural cave: a mouth in a hillside (a pit on flat ground) leading to chambers.
    #[default]
    Cave,
    /// A mine: a timbered adit into the hill (a shaft on flat ground), straight drifts, rooms.
    Mine,
    /// A collapsed sinkhole: a wide pit open to the sky with a scree ramp down into the cave.
    Pit,
    /// A passage right through a hill or ridge, open at both ends.
    Passage,
    /// A cavern sealed under the ground (no entrance): dig into it, or keep it for later.
    Cavern,
    /// A natural rock arch standing over the ground.
    Arch,
}

impl CaveKind {
    pub const ALL: [CaveKind; 6] = [
        CaveKind::Cave,
        CaveKind::Mine,
        CaveKind::Pit,
        CaveKind::Passage,
        CaveKind::Cavern,
        CaveKind::Arch,
    ];
    pub fn name(self) -> &'static str {
        match self {
            CaveKind::Cave => "Cave",
            CaveKind::Mine => "Mine",
            CaveKind::Pit => "Sinkhole",
            CaveKind::Passage => "Passage",
            CaveKind::Cavern => "Hidden cavern",
            CaveKind::Arch => "Rock arch",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            CaveKind::Cave => "cave",
            CaveKind::Mine => "mine",
            CaveKind::Pit => "pit",
            CaveKind::Passage => "passage",
            CaveKind::Cavern => "cavern",
            CaveKind::Arch => "arch",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    pub fn hint(self) -> &'static str {
        match self {
            CaveKind::Cave => {
                "Click a hillside: a cave mouth opens there and chambers run into the hill (on flat ground it starts as a pit)."
            }
            CaveKind::Mine => {
                "Click a hillside for a timbered mine entrance (flat ground: a shaft with a ramp). Straight drifts and rooms."
            }
            CaveKind::Pit => {
                "Click anywhere: the ground collapses into a wide pit with a scree slope down into the cave."
            }
            CaveKind::Passage => {
                "Click the foot of a hill or ridge: a passage runs right through it and opens on the far side."
            }
            CaveKind::Cavern => {
                "Click the ground above where it should be: chambers deep underground with no way in (dig one, or keep it for a dungeon)."
            }
            CaveKind::Arch => {
                "Click where the arch should stand (drag to set its span): a natural rock arch over the ground."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaveSize {
    Small,
    #[default]
    Medium,
    Large,
    Huge,
    /// a great hall (60–80 m across, ~30 m high) with chambers round it
    Giant,
    /// two vast halls (130–180 m across, ~55 m high): a whole underground world
    Colossal,
}

impl CaveSize {
    pub const ALL: [CaveSize; 6] = [
        CaveSize::Small,
        CaveSize::Medium,
        CaveSize::Large,
        CaveSize::Huge,
        CaveSize::Giant,
        CaveSize::Colossal,
    ];
    pub fn name(self) -> &'static str {
        match self {
            CaveSize::Small => "S",
            CaveSize::Medium => "M",
            CaveSize::Large => "L",
            CaveSize::Huge => "XL",
            CaveSize::Giant => "XXL",
            CaveSize::Colossal => "XXXL",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            CaveSize::Small => "small",
            CaveSize::Medium => "medium",
            CaveSize::Large => "large",
            CaveSize::Huge => "huge",
            CaveSize::Giant => "giant",
            CaveSize::Colossal => "colossal",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// Chambers along the main way (before branches).
    pub fn chambers(self) -> usize {
        match self {
            CaveSize::Small => 2,
            CaveSize::Medium => 4,
            CaveSize::Large => 7,
            CaveSize::Huge => 11,
            CaveSize::Giant => 5,
            CaveSize::Colossal => 7,
        }
    }
    /// Side branches (each a tunnel and one or two chambers).
    pub fn branches(self) -> usize {
        match self {
            CaveSize::Small => 0,
            CaveSize::Medium => 1,
            CaveSize::Large => 3,
            CaveSize::Huge => 5,
            CaveSize::Giant => 2,
            CaveSize::Colossal => 3,
        }
    }
    /// Scale of chambers and tunnels: bigger caves are bigger inside, not just longer.
    pub fn scale(self) -> f32 {
        match self {
            CaveSize::Small => 0.8,
            CaveSize::Medium => 1.0,
            CaveSize::Large => 1.4,
            CaveSize::Huge => 1.8,
            CaveSize::Giant => 2.2,
            CaveSize::Colossal => 2.6,
        }
    }
    /// Great halls along the main way (giant sizes).
    pub fn halls(self) -> usize {
        match self {
            CaveSize::Giant => 1,
            CaveSize::Colossal => 2,
            _ => 0,
        }
    }
    /// The giant sizes: built round great halls, meshed coarser (their rock is vast).
    pub fn giant(self) -> bool {
        self.halls() > 0
    }
}

/// The look: rock, floor, what grows or forms on it, the light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// From the biome at the entrance.
    #[default]
    Auto,
    /// Grey limestone: bedding-plane ledges, flowstone, stalactites.
    Limestone,
    /// Ice cave: frozen walls, icicles, frost at the mouth.
    Ice,
    /// Mossy and damp: green moss, roots and dripping water near the light.
    Mossy,
    /// Layered sandstone: warm, dry, wind-cut.
    Sandstone,
    /// Basalt lava tubes: dark, round tubes, embers.
    Volcanic,
    /// Marble and glowing crystals.
    Crystal,
    /// Ore-streaked rock and dirt floors (mines).
    Mine,
}

impl Theme {
    pub const ALL: [Theme; 8] = [
        Theme::Auto,
        Theme::Limestone,
        Theme::Ice,
        Theme::Mossy,
        Theme::Sandstone,
        Theme::Volcanic,
        Theme::Crystal,
        Theme::Mine,
    ];
    /// The concrete themes (index = the shader's theme slot).
    pub const CONCRETE: [Theme; 7] = [
        Theme::Limestone,
        Theme::Ice,
        Theme::Mossy,
        Theme::Sandstone,
        Theme::Volcanic,
        Theme::Crystal,
        Theme::Mine,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Theme::Auto => "Auto",
            Theme::Limestone => "Limestone",
            Theme::Ice => "Ice",
            Theme::Mossy => "Mossy",
            Theme::Sandstone => "Sandstone",
            Theme::Volcanic => "Volcanic",
            Theme::Crystal => "Crystal",
            Theme::Mine => "Mine",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Limestone => "limestone",
            Theme::Ice => "ice",
            Theme::Mossy => "mossy",
            Theme::Sandstone => "sandstone",
            Theme::Volcanic => "volcanic",
            Theme::Crystal => "crystal",
            Theme::Mine => "mine",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// Slot in the shader's theme table (Auto must be resolved first).
    pub fn slot(self) -> usize {
        Self::CONCRETE.iter().position(|t| *t == self).unwrap_or(0)
    }
    /// The theme a biome suggests.
    pub fn for_biome(b: Option<crate::biomes::BiomePreset>, kind: CaveKind) -> Theme {
        use crate::biomes::BiomePreset as B;
        if kind == CaveKind::Mine {
            return Theme::Mine;
        }
        match b {
            Some(B::Taiga | B::Alpine | B::Tundra) => Theme::Ice,
            Some(B::Jungle | B::TropicalCoast | B::Swamp | B::HauntedWoods) => Theme::Mossy,
            Some(B::Desert) => Theme::Sandstone,
            Some(B::Volcanic) => Theme::Volcanic,
            _ => Theme::Limestone,
        }
    }
}

/// Optional things the cave grows (all on by default except crystals outside crystal caves).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    /// stalactites, stalagmites and columns (icicles and ice columns in ice caves)
    pub formations: bool,
    /// still pools in low chambers
    pub pools: bool,
    /// glowing crystal clusters (they light the chambers)
    pub crystals: bool,
    /// fallen rock and scree on the floors
    pub rubble: bool,
    /// shafts of daylight through holes in the roof of big chambers
    pub skylights: bool,
    /// lanterns or torches along the way (mines: timber props and lanterns)
    pub lights: bool,
    /// ramps, ledges and rock bridges in tall chambers (the way on climbs or drops inside them),
    /// natural arches over the way
    pub bridges: bool,
    /// waterfalls pouring from high in the walls of tall chambers into pools
    pub waterfalls: bool,
    /// cobwebs in the corners and across side passages
    pub cobwebs: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            formations: true,
            pools: true,
            crystals: false,
            rubble: true,
            skylights: false,
            lights: false,
            bridges: true,
            waterfalls: false,
            cobwebs: false,
        }
    }
}

/// How dark it is inside: real darkness, or a soft fill so a game stays readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Darkness {
    /// pitch black away from light sources
    Dark,
    /// a faint fill: shapes read, colours don't
    #[default]
    Dim,
    /// game-readable: you can see the whole chamber
    Bright,
}

impl Darkness {
    pub const ALL: [Darkness; 3] = [Darkness::Dark, Darkness::Dim, Darkness::Bright];
    pub fn name(self) -> &'static str {
        match self {
            Darkness::Dark => "Dark",
            Darkness::Dim => "Dim",
            Darkness::Bright => "Readable",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Darkness::Dark => "dark",
            Darkness::Dim => "dim",
            Darkness::Bright => "readable",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| {
            k.id() == s
                || k.name().eq_ignore_ascii_case(s)
                || (s == "bright" && *k == Darkness::Bright)
        })
    }
    /// Fill light strength (0..1) for the shader.
    pub fn fill(self) -> f32 {
        match self {
            Darkness::Dark => 0.0,
            Darkness::Dim => 0.55,
            Darkness::Bright => 1.0,
        }
    }
}

/// How the way in looks (chosen from the ground at the click and the kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mouth {
    /// a rocky opening in a hillside
    #[default]
    Hillside,
    /// a wide collapsed pit with a scree ramp
    Pit,
    /// a timbered mine entrance in a hillside
    Adit,
    /// a mine shaft with a ramp beside it on flat ground
    Shaft,
    /// none (sealed cavern, arch)
    None,
}

/// A chamber: its floor centre, half width (x), height, half length (z), turned by `yaw`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Chamber {
    pub id: u16,
    pub pos: [f32; 3],
    /// half width, height, half length (m)
    pub size: [f32; 3],
    pub yaw: f32,
    #[serde(default)]
    pub level: u8,
    /// kept by Shuffle
    #[serde(default)]
    pub locked: bool,
    /// a still pool in this chamber
    #[serde(default)]
    pub pool: bool,
    /// a daylight hole in its roof
    #[serde(default)]
    pub skylight: bool,
    /// a great hall (giant caves): uneven floor, giant formations, a lake
    #[serde(default)]
    pub hall: bool,
    /// the lip round its pool (Auto: picked from the cave's seed)
    #[serde(default)]
    pub rim: RimStyle,
}

/// The lip of rock round a pool that holds its water in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RimStyle {
    /// one of the others, picked for each pool from the cave's seed
    #[default]
    Auto,
    /// a low rounded dam of flowstone
    Rimstone,
    /// lobed dams curling round it (gours)
    Scalloped,
    /// steps of travertine round it, each a little higher
    Terraces,
    /// a ring of rounded boulders
    Boulders,
    /// a wide, flat apron of flowstone
    Shelf,
    /// a crust of sharp crystals
    Crust,
}

impl RimStyle {
    pub const ALL: [RimStyle; 7] = [
        RimStyle::Auto,
        RimStyle::Rimstone,
        RimStyle::Scalloped,
        RimStyle::Terraces,
        RimStyle::Boulders,
        RimStyle::Shelf,
        RimStyle::Crust,
    ];
    /// The concrete styles (what Auto picks among).
    pub const CONCRETE: [RimStyle; 6] = [
        RimStyle::Rimstone,
        RimStyle::Scalloped,
        RimStyle::Terraces,
        RimStyle::Boulders,
        RimStyle::Shelf,
        RimStyle::Crust,
    ];
    pub fn name(self) -> &'static str {
        match self {
            RimStyle::Auto => "Auto",
            RimStyle::Rimstone => "Rimstone",
            RimStyle::Scalloped => "Scalloped",
            RimStyle::Terraces => "Terraces",
            RimStyle::Boulders => "Boulders",
            RimStyle::Shelf => "Shelf",
            RimStyle::Crust => "Crystal crust",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            RimStyle::Auto => "auto",
            RimStyle::Rimstone => "rimstone",
            RimStyle::Scalloped => "scalloped",
            RimStyle::Terraces => "terraces",
            RimStyle::Boulders => "boulders",
            RimStyle::Shelf => "shelf",
            RimStyle::Crust => "crust",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    /// The next (or previous) one round the carousel, Auto included.
    pub fn step(self, by: i32) -> RimStyle {
        let n = Self::ALL.len() as i32;
        let i = Self::ALL.iter().position(|r| *r == self).unwrap_or(0) as i32;
        Self::ALL[(i + by).rem_euclid(n) as usize]
    }
}

impl Chamber {
    pub fn center(&self) -> bevy::math::Vec3 {
        bevy::math::Vec3::from_array(self.pos)
    }
    pub fn radius(&self) -> f32 {
        self.size[0].max(self.size[2])
    }
    /// How far its floor reaches from the middle towards `dir` (x, z; a unit vector): the foot
    /// of its domed ellipse (about 0.79 of the ellipse's radius that way).
    pub fn reach(&self, dir: bevy::math::Vec2) -> f32 {
        floor_reach(self.size, self.yaw, dir)
    }
}

/// The floor's reach of a chamber of `size` turned by `yaw` towards `dir` (see `Chamber::reach`).
pub fn floor_reach(size: [f32; 3], yaw: f32, dir: bevy::math::Vec2) -> f32 {
    let (s, c) = yaw.sin_cos();
    let (lx, lz) = (dir.x * c - dir.y * s, dir.x * s + dir.y * c);
    0.79 / ((lx / size[0]).powi(2) + (lz / size[2]).powi(2))
        .sqrt()
        .max(1e-4)
}

/// Cross-section of a tunnel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// an oval tube with a flat floor
    #[default]
    Tube,
    /// tall and narrow
    Canyon,
    /// a tube on top of a narrow slot (limestone)
    Keyhole,
    /// wide and low (a bedding plane)
    Low,
    /// square-ish with a flat roof (mines)
    Drift,
}

/// A tunnel between two chambers (`a` = `ENTRANCE` for the way in): its floor centreline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tunnel {
    pub a: u16,
    pub b: u16,
    pub pts: Vec<[f32; 3]>,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub profile: Profile,
}

/// `Tunnel::a` / `b` of an end that opens outside (the way in, the far end of a passage).
pub const OUTSIDE: u16 = u16::MAX;

/// A dungeon behind a door at the end of the cave (a separate map in the game: a loading door).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dungeon {
    pub name: String,
    /// the chamber the door stands in
    pub chamber: u16,
    /// where the door is and which way it faces (into the chamber)
    pub door: [f32; 3],
    pub door_yaw: f32,
    /// how the game runs it: one shared copy, a private copy per party, or a raid
    #[serde(default)]
    pub instancing: Instancing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Instancing {
    Shared,
    #[default]
    Party,
    Raid,
}

impl Instancing {
    pub const ALL: [Instancing; 3] = [Instancing::Shared, Instancing::Party, Instancing::Raid];
    pub fn name(self) -> &'static str {
        match self {
            Instancing::Shared => "Shared",
            Instancing::Party => "Party instance",
            Instancing::Raid => "Raid",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Instancing::Shared => "shared",
            Instancing::Party => "party",
            Instancing::Raid => "raid",
        }
    }
}

/// A natural arch: two feet on the ground and the span between them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArchSpec {
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// clearance under the middle (m)
    pub height: f32,
    /// thickness of the rock band (m)
    pub thickness: f32,
}

/// One cave (or arch). The recipe plus the laid-out chambers and tunnels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cave {
    pub id: u32,
    pub name: String,
    pub kind: CaveKind,
    #[serde(default)]
    pub size: CaveSize,
    #[serde(default = "one")]
    pub levels: u8,
    /// the levels climb into the hill instead of going down
    #[serde(default)]
    pub up: bool,
    /// as chosen (Auto follows the biome)
    #[serde(default)]
    pub theme: Theme,
    /// what Auto resolved to when the cave was laid out
    #[serde(default)]
    pub resolved: Theme,
    #[serde(default)]
    pub features: Features,
    #[serde(default)]
    pub darkness: Darkness,
    pub seed: u32,
    /// where it was clicked (on the ground) and the heading into the cave (radians, 0 = +z)
    pub entrance: [f32; 3],
    pub heading: f32,
    #[serde(default)]
    pub mouth: Mouth,
    #[serde(default)]
    pub chambers: Vec<Chamber>,
    #[serde(default)]
    pub tunnels: Vec<Tunnel>,
    #[serde(default)]
    pub dungeon: Option<Dungeon>,
    #[serde(default)]
    pub arch: Option<ArchSpec>,
    /// the objects furnishing it (`dress`): replaced when it is laid out again
    #[serde(default)]
    pub props: Vec<u64>,
}

fn one() -> u8 {
    1
}

impl Cave {
    /// The concrete theme (Auto resolved).
    pub fn theme(&self) -> Theme {
        if self.theme == Theme::Auto {
            if self.resolved == Theme::Auto {
                Theme::Limestone
            } else {
                self.resolved
            }
        } else {
            self.theme
        }
    }
    pub fn chamber(&self, id: u16) -> Option<&Chamber> {
        self.chambers.iter().find(|c| c.id == id)
    }
    /// The lip round chamber `c`'s pool (Auto resolved: the same for the same seed).
    pub fn rim_of(&self, c: &Chamber) -> RimStyle {
        if c.rim != RimStyle::Auto {
            return c.rim;
        }
        let mut h = (self.seed as u64 ^ 0x7121_5eed).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (c.id as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        h ^= h >> 29;
        RimStyle::CONCRETE[(h % RimStyle::CONCRETE.len() as u64) as usize]
    }
    /// The chamber farthest from the way in (along the tunnels): where a dungeon door goes.
    pub fn deepest(&self) -> Option<u16> {
        let dist = self.path_lengths();
        dist.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|d| d.0)
    }
    /// Distance from the entrance along the tunnels to each chamber.
    pub fn path_lengths(&self) -> Vec<(u16, f32)> {
        let len = |t: &Tunnel| {
            t.pts
                .windows(2)
                .map(|w| {
                    bevy::math::Vec3::from_array(w[0]).distance(bevy::math::Vec3::from_array(w[1]))
                })
                .sum::<f32>()
        };
        let mut dist: Vec<(u16, f32)> = self
            .chambers
            .iter()
            .map(|c| (c.id, f32::INFINITY))
            .collect();
        let mut frontier: Vec<(u16, f32)> = self
            .tunnels
            .iter()
            .filter(|t| t.a == OUTSIDE)
            .map(|t| (t.b, len(t)))
            .collect();
        if frontier.is_empty()
            && let Some(c) = self.chambers.first()
        {
            frontier.push((c.id, 0.0));
        }
        while let Some((id, d)) = frontier.pop() {
            let Some(slot) = dist.iter_mut().find(|x| x.0 == id) else {
                continue;
            };
            if d >= slot.1 {
                continue;
            }
            slot.1 = d;
            for t in &self.tunnels {
                if t.a == id && t.b != OUTSIDE {
                    frontier.push((t.b, d + len(t)));
                } else if t.b == id && t.a != OUTSIDE {
                    frontier.push((t.a, d + len(t)));
                }
            }
        }
        dist.into_iter().filter(|d| d.1.is_finite()).collect()
    }
    /// Bounds of everything carved or added (x, y, z), without noise margins.
    pub fn bounds(&self) -> (bevy::math::Vec3, bevy::math::Vec3) {
        use bevy::math::Vec3;
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for c in &self.chambers {
            let r = c.radius();
            let p = c.center();
            lo = lo.min(p - Vec3::new(r, 0.5, r));
            hi = hi.max(p + Vec3::new(r, c.size[1], r));
        }
        for t in &self.tunnels {
            for q in &t.pts {
                let p = Vec3::from_array(*q);
                lo = lo.min(p - Vec3::new(t.width, 0.5, t.width));
                hi = hi.max(p + Vec3::new(t.width, t.height, t.width));
            }
        }
        if let Some(a) = &self.arch {
            for f in [a.a, a.b] {
                let p = Vec3::new(f[0], self.entrance[1], f[1]);
                lo = lo.min(p - Vec3::splat(a.thickness * 2.0));
                hi = hi.max(
                    p + Vec3::new(
                        a.thickness * 2.0,
                        a.height + a.thickness * 2.0,
                        a.thickness * 2.0,
                    ),
                );
            }
        }
        if lo.x > hi.x {
            let e = Vec3::from_array(self.entrance);
            return (e - Vec3::splat(4.0), e + Vec3::splat(4.0));
        }
        (lo, hi)
    }
}

/// A freehand dig stroke (Caves › Dig / Fill): spheres swept along the path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DigStroke {
    pub pts: Vec<[f32; 3]>,
    pub radius: f32,
    /// true: fill rock back in
    #[serde(default)]
    pub fill: bool,
}

/// Everything the Caves tab made in this world.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, bevy::prelude::Resource)]
pub struct Caves {
    pub list: Vec<Cave>,
    #[serde(default)]
    pub strokes: Vec<DigStroke>,
    #[serde(default)]
    pub next_id: u32,
}

impl Caves {
    pub fn alloc_id(&mut self) -> u32 {
        self.next_id = self
            .next_id
            .max(self.list.iter().map(|c| c.id).max().unwrap_or(0))
            .max(0)
            + 1;
        self.next_id
    }
    pub fn get(&self, id: u32) -> Option<&Cave> {
        self.list.iter().find(|c| c.id == id)
    }
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Cave> {
        self.list.iter_mut().find(|c| c.id == id)
    }
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.strokes.is_empty()
    }
}
