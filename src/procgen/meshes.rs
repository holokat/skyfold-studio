//! Procedural "stylized-realistic" low/mid-poly nature and prop meshes.
//!
//! Pure `std` Rust, zero dependencies. Every asset is generated deterministically from
//! `(kind, variant, lod)`; `lod` only changes the amount of detail, never the macro shape
//! (height, crown size, lean, placement), so LOD0 and LOD1 of the same variant line up.
//!
//! Conventions
//! * Y up, meters, right-handed. Origin = base center, resting on the ground at y = 0.
//!   Trunks, posts and rocks sink a little below 0 so they never float on slopes.
//! * Triangle list, counter-clockwise front faces. Thin sheets (leaves, fronds, blades,
//!   canvas) are emitted twice (both windings) because the renderer back-face culls.
//! * `colors` are LINEAR RGB (designed in sRGB, converted with [`srgb`]).
//!   Alpha is the wind-sway weight: 0 = rigid (trunk base, rocks, structures),
//!   rising towards 1 at leaf / frond / blade tips.
//! * Bridges and the dock span the X axis; fallen log and driftwood lie along X;
//!   the fence section runs along X from -2 to +2.

#![allow(dead_code)]

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

// =====================================================================================
// Public API
// =====================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    // Trees (height ~8-22 m)
    Pine,
    Fir,
    Spruce,
    SnowyPine,
    Cedar,
    Oak,
    Birch,
    Maple,
    Willow,
    Palm,
    Bamboo,
    DeadTree,
    // Rocks
    Boulder,
    RockCluster,
    CliffRock,
    Pebbles,
    MossyRock,
    RockSpire,
    // Plants (small, 0.3-2.5 m)
    Bush,
    FlowerBush,
    Fern,
    Flowers,
    TallGrass,
    Reeds,
    Cactus,
    Mushrooms,
    // Debris
    FallenLog,
    Stump,
    Branches,
    Driftwood,
    // Structures / props
    WoodenBridge,
    StoneBridge,
    Hut,
    Tent,
    Fence,
    RuinPillar,
    RuinWall,
    Watchtower,
    Campfire,
    Crate,
    Barrel,
    Signpost,
    Banner,
    TarpShelter,
    Well,
    Dock,
    DesertShrub,
    LilyPads,
    /// Exposed root tangle (photoscan; debris)
    Roots,
    /// Tall formations and crystals (`visuals::formations`)
    Hoodoo,
    SeaStack,
    Crystals,
    // Appended: previous values are serialized in nature.bin. Never reorder them.
    GoldenMaple,
    SakuraPink,
    SakuraWhite,
    Ginkgo,
    Sequoia,
    Lavender,
    WildStrawberry,
    Blueberry,
    Blackberry,
    /// A single tulip (`flora`; variants = colours, `fields::plants::TULIP_COLOUR_NAMES`)
    Tulip,
    /// A rock hanging in the air: floating island chunks and asteroid boulders
    /// (`visuals::floating`; placed at a height above the ground)
    FloatingRock,
    /// Standing slabs, obelisks, menhirs and basalt columns (`visuals::monoliths`)
    Monolith,
    /// The quiver tree (photoscan; alien flora of the sci-fi template)
    QuiverTree,
    /// Karoo succulent clumps and ice-plant rosettes (photoscan; alien flora)
    Succulent,
}

/// Broad grouping of [`AssetKind`]s (triangle budgets, sway conventions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetCategory {
    Tree,
    Rock,
    Plant,
    Debris,
    Structure,
}

impl AssetCategory {
    /// Maximum triangle count for the given LOD (0 = full, >= 1 = far).
    pub fn triangle_budget(self, lod: u32) -> usize {
        let far = lod > 0;
        match self {
            AssetCategory::Tree => {
                if far {
                    220
                } else {
                    2000
                }
            }
            AssetCategory::Rock => {
                if far {
                    120
                } else {
                    800
                }
            }
            AssetCategory::Plant => {
                if far {
                    60
                } else {
                    500
                }
            }
            AssetCategory::Debris => {
                if far {
                    80
                } else {
                    600
                }
            }
            AssetCategory::Structure => {
                if far {
                    500
                } else {
                    4000
                }
            }
        }
    }
}

impl AssetKind {
    pub const ALL: &'static [AssetKind] = &[
        AssetKind::Pine,
        AssetKind::Fir,
        AssetKind::Spruce,
        AssetKind::SnowyPine,
        AssetKind::Cedar,
        AssetKind::Oak,
        AssetKind::Birch,
        AssetKind::Maple,
        AssetKind::Willow,
        AssetKind::Palm,
        AssetKind::Bamboo,
        AssetKind::DeadTree,
        AssetKind::Boulder,
        AssetKind::RockCluster,
        AssetKind::CliffRock,
        AssetKind::Pebbles,
        AssetKind::MossyRock,
        AssetKind::RockSpire,
        AssetKind::Bush,
        AssetKind::FlowerBush,
        AssetKind::Fern,
        AssetKind::Flowers,
        AssetKind::TallGrass,
        AssetKind::Reeds,
        AssetKind::Cactus,
        AssetKind::Mushrooms,
        AssetKind::FallenLog,
        AssetKind::Stump,
        AssetKind::Branches,
        AssetKind::Driftwood,
        AssetKind::WoodenBridge,
        AssetKind::StoneBridge,
        AssetKind::Hut,
        AssetKind::Tent,
        AssetKind::Fence,
        AssetKind::RuinPillar,
        AssetKind::RuinWall,
        AssetKind::Watchtower,
        AssetKind::Campfire,
        AssetKind::Crate,
        AssetKind::Barrel,
        AssetKind::Signpost,
        AssetKind::Banner,
        AssetKind::TarpShelter,
        AssetKind::DesertShrub,
        AssetKind::LilyPads,
        AssetKind::Well,
        AssetKind::Dock,
        AssetKind::Roots,
        AssetKind::Hoodoo,
        AssetKind::SeaStack,
        AssetKind::Crystals,
        AssetKind::GoldenMaple,
        AssetKind::SakuraPink,
        AssetKind::SakuraWhite,
        AssetKind::Ginkgo,
        AssetKind::Sequoia,
        AssetKind::Lavender,
        AssetKind::WildStrawberry,
        AssetKind::Blueberry,
        AssetKind::Blackberry,
        AssetKind::Tulip,
        AssetKind::FloatingRock,
        AssetKind::Monolith,
        AssetKind::QuiverTree,
        AssetKind::Succulent,
    ];

    /// Human readable label.
    pub fn name(self) -> &'static str {
        use AssetKind::*;
        match self {
            Pine => "Pine",
            Fir => "Fir",
            Spruce => "Spruce",
            SnowyPine => "Snowy Pine",
            Cedar => "Cedar",
            Oak => "Oak",
            Birch => "Birch",
            Maple => "Maple",
            Willow => "Willow",
            Palm => "Palm",
            Bamboo => "Bamboo",
            DeadTree => "Dead Tree",
            Boulder => "Boulder",
            RockCluster => "Rock Cluster",
            CliffRock => "Cliff Rock",
            Pebbles => "Pebbles",
            MossyRock => "Mossy Rock",
            RockSpire => "Rock Spire",
            Bush => "Bush",
            FlowerBush => "Flower Bush",
            Fern => "Fern",
            Flowers => "Flowers",
            TallGrass => "Tall Grass",
            Reeds => "Reeds",
            Cactus => "Cactus",
            Mushrooms => "Mushrooms",
            FallenLog => "Fallen Log",
            Stump => "Stump",
            Branches => "Branches",
            Driftwood => "Driftwood",
            WoodenBridge => "Wooden Bridge",
            StoneBridge => "Stone Bridge",
            Hut => "Hut",
            Tent => "Tent",
            Fence => "Fence",
            RuinPillar => "Ruin Pillar",
            RuinWall => "Ruin Wall",
            Watchtower => "Watchtower",
            Campfire => "Campfire",
            Crate => "Crate",
            Barrel => "Barrel",
            Signpost => "Signpost",
            Banner => "Banner",
            TarpShelter => "Tarp Shelter",
            DesertShrub => "Desert Shrub",

            LilyPads => "Lily Pads",
            Well => "Well",
            Dock => "Dock",
            Roots => "Roots",
            Hoodoo => "Hoodoo",
            SeaStack => "Sea Stack",
            Crystals => "Crystals",
            GoldenMaple => "Golden Maple",
            SakuraPink => "Pink Sakura",
            SakuraWhite => "White Sakura",
            Ginkgo => "Ginkgo",
            Sequoia => "Giant Sequoia",
            Lavender => "Lavender",
            WildStrawberry => "Wild strawberry",
            Blueberry => "Blueberry",
            Blackberry => "Blackberry",
            Tulip => "Tulip",
            FloatingRock => "Floating Rock",
            Monolith => "Monolith",
            QuiverTree => "Quiver Tree",
            Succulent => "Succulents",
        }
    }

    pub fn category(self) -> AssetCategory {
        use AssetKind::*;
        match self {
            Pine | Fir | Spruce | SnowyPine | Cedar | Oak | Birch | Maple | GoldenMaple
            | SakuraPink | SakuraWhite | Ginkgo | Sequoia | Willow | Palm | Bamboo | DeadTree
            | QuiverTree => {
                AssetCategory::Tree
            }
            Boulder | RockCluster | CliffRock | Pebbles | MossyRock | RockSpire | Hoodoo
            | SeaStack | Crystals | FloatingRock | Monolith => AssetCategory::Rock,
            Bush | FlowerBush | Fern | Flowers | TallGrass | Reeds | Cactus | Mushrooms
            | Lavender | WildStrawberry | Blueberry | Blackberry | Tulip | Succulent => {
                AssetCategory::Plant
            }
            FallenLog | Stump | Branches | Driftwood | Roots => AssetCategory::Debris,
            _ => AssetCategory::Structure,
        }
    }
}

/// Plain mesh buffers, ready to be copied into an engine mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    /// Unit length.
    pub normals: Vec<[f32; 3]>,
    /// LINEAR-space RGB; A = wind sway weight.
    pub colors: Vec<[f32; 4]>,
    /// Triangle list, counter-clockwise front faces.
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis aligned bounds `(min, max)`; zeros for an empty mesh.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        if self.positions.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for p in &self.positions {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
        (mn, mx)
    }
}

/// Build one asset. `variant` seeds the random variation (callers use 0, 1, 2, ...),
/// `lod` 0 = full detail, 1 (or more) = far LOD. Deterministic for a given input.
pub fn build_asset(kind: AssetKind, variant: u32, lod: u32) -> MeshData {
    use AssetKind::*;
    let lod = lod.min(1);
    let seed = (kind as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (variant as u64 + 0x51).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let mut rng = Rng::new(seed);
    let m = match kind {
        Pine | Fir | Spruce | SnowyPine => conifer(kind, &mut rng, lod),
        Cedar => cedar(&mut rng, lod),
        Oak | Birch | Maple | GoldenMaple | SakuraPink | SakuraWhite | Ginkgo => {
            broadleaf(kind, &mut rng, lod)
        }
        Sequoia => conifer(AssetKind::Cedar, &mut rng, lod),
        Willow => willow(&mut rng, lod),
        Palm => palm(&mut rng, lod),
        Bamboo => bamboo(&mut rng, lod),
        DeadTree => dead_tree(&mut rng, lod),
        Boulder | MossyRock => boulder(kind == MossyRock, &mut rng, lod),
        RockCluster => rock_cluster(&mut rng, lod),
        CliffRock => cliff_rock(&mut rng, lod),
        Pebbles => pebbles(&mut rng, lod),
        RockSpire | Hoodoo | SeaStack | Crystals => {
            return crate::visuals::formations::formation(kind, variant, lod);
        }
        FloatingRock => return crate::visuals::floating::floating_rock(variant, lod),
        Monolith => return crate::visuals::monoliths::monolith(variant, lod),
        Bush => bush(false, &mut rng, lod),
        FlowerBush => bush(true, &mut rng, lod),
        Fern => fern(&mut rng, lod),
        Flowers => flowers(&mut rng, lod),
        TallGrass => tall_grass(&mut rng, lod),
        Reeds => reeds(&mut rng, lod),
        Cactus => cactus(&mut rng, lod),
        Mushrooms => mushrooms(&mut rng, lod),
        Lavender | WildStrawberry | Blueberry | Blackberry | Tulip => Mesh::new(),
        FallenLog => fallen_log(&mut rng, lod),
        Stump => stump(&mut rng, lod),
        Branches => branches(&mut rng, lod),
        Driftwood => driftwood(&mut rng, lod),
        WoodenBridge => wooden_bridge(&mut rng, lod),
        StoneBridge => stone_bridge(&mut rng, lod),
        Hut => hut(&mut rng, lod),
        Tent => tent(&mut rng, lod),
        Fence => fence(&mut rng, lod),
        RuinPillar => ruin_pillar(&mut rng, lod),
        RuinWall => ruin_wall(&mut rng, lod),
        Watchtower => watchtower(&mut rng, lod),
        Campfire => campfire(&mut rng, lod),
        Crate => crate_box(&mut rng, lod),
        Barrel => barrel(&mut rng, lod),
        Signpost => signpost(&mut rng, lod),
        Banner => banner_pole(&mut rng, lod),
        TarpShelter => tarp_shelter(&mut rng, lod),
        // grown in trees.rs / plants.rs; these placeholders are never used
        DesertShrub | LilyPads => bush(false, &mut rng, lod),
        // photoscanned (photoscan.rs); a rooted stump stands in if its files are missing
        Roots => stump(&mut rng, lod),
        // photoscanned alien flora; stand-ins if their files are missing
        QuiverTree => dead_tree(&mut rng, lod),
        Succulent => cactus(&mut rng, lod),
        Well => well(&mut rng, lod),
        Dock => dock(&mut rng, lod),
    };
    m.into_data()
}

/// sRGB (0..1, as picked in a color picker) to linear.
pub fn srgb(r: f32, g: f32, b: f32) -> [f32; 3] {
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]
}

fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// =====================================================================================
// Math
// =====================================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}

const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    const ZERO: V3 = v3(0.0, 0.0, 0.0);
    const X: V3 = v3(1.0, 0.0, 0.0);
    const Y: V3 = v3(0.0, 1.0, 0.0);
    const Z: V3 = v3(0.0, 0.0, 1.0);

    fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    fn cross(self, o: V3) -> V3 {
        v3(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-12 && l.is_finite() {
            self * (1.0 / l)
        } else {
            V3::Y
        }
    }
    fn lerp(self, o: V3, t: f32) -> V3 {
        self + (o - self) * t
    }
    fn cmul(self, o: V3) -> V3 {
        v3(self.x * o.x, self.y * o.y, self.z * o.z)
    }
    fn horiz(self) -> V3 {
        v3(self.x, 0.0, self.z)
    }
    fn with_y(self, y: f32) -> V3 {
        v3(self.x, y, self.z)
    }
    fn arr(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    /// Unit vector on the XZ plane at angle `a` (same convention as the ring builders).
    fn polar(a: f32) -> V3 {
        v3(a.cos(), 0.0, -a.sin())
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for V3 {
    type Output = V3;
    fn mul(self, s: f32) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}
impl Div<f32> for V3 {
    type Output = V3;
    fn div(self, s: f32) -> V3 {
        v3(self.x / s, self.y / s, self.z / s)
    }
}
impl Neg for V3 {
    type Output = V3;
    fn neg(self) -> V3 {
        v3(-self.x, -self.y, -self.z)
    }
}
impl AddAssign for V3 {
    fn add_assign(&mut self, o: V3) {
        *self = *self + o;
    }
}
impl SubAssign for V3 {
    fn sub_assign(&mut self, o: V3) {
        *self = *self - o;
    }
}

/// 3x3 matrix stored as columns (images of the X, Y, Z axes). Used for rotations.
#[derive(Clone, Copy, Debug)]
struct M3 {
    x: V3,
    y: V3,
    z: V3,
}

impl M3 {
    const I: M3 = M3 {
        x: V3::X,
        y: V3::Y,
        z: V3::Z,
    };

    fn apply(&self, v: V3) -> V3 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }
    /// `self` first, then `o`.
    fn then(&self, o: &M3) -> M3 {
        M3 {
            x: o.apply(self.x),
            y: o.apply(self.y),
            z: o.apply(self.z),
        }
    }
    fn rot_x(a: f32) -> M3 {
        let (s, c) = a.sin_cos();
        M3 {
            x: V3::X,
            y: v3(0.0, c, s),
            z: v3(0.0, -s, c),
        }
    }
    fn rot_y(a: f32) -> M3 {
        let (s, c) = a.sin_cos();
        M3 {
            x: v3(c, 0.0, -s),
            y: V3::Y,
            z: v3(s, 0.0, c),
        }
    }
    fn rot_z(a: f32) -> M3 {
        let (s, c) = a.sin_cos();
        M3 {
            x: v3(c, s, 0.0),
            y: v3(-s, c, 0.0),
            z: V3::Z,
        }
    }
    fn axis_angle(axis: V3, a: f32) -> M3 {
        let k = axis.norm();
        let (s, c) = a.sin_cos();
        let r = |v: V3| v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c));
        M3 {
            x: r(V3::X),
            y: r(V3::Y),
            z: r(V3::Z),
        }
    }
    /// Rotation taking unit vector `a` onto unit vector `b`.
    fn from_to(a: V3, b: V3) -> M3 {
        let a = a.norm();
        let b = b.norm();
        let c = a.dot(b).clamp(-1.0, 1.0);
        let axis = a.cross(b);
        if axis.len() < 1e-6 {
            if c > 0.0 {
                return M3::I;
            }
            let perp = if a.x.abs() < 0.9 { V3::X } else { V3::Z };
            return M3::axis_angle(a.cross(perp), PI);
        }
        M3::axis_angle(axis, c.acos())
    }
    /// Small random tilt away from vertical.
    fn tilt(rng: &mut Rng, max: f32) -> M3 {
        let a = rng.angle();
        M3::axis_angle(V3::polar(a), rng.range(0.0, max))
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

// =====================================================================================
// Random numbers & noise
// =====================================================================================

/// SplitMix64 generator: tiny, fast, good enough for content generation.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        let mut r = Rng(seed ^ 0x6A09_E667_F3BC_C909);
        r.next();
        r
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 * (1.0 / 16_777_216.0)
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
    fn sym(&mut self, a: f32) -> f32 {
        self.range(-a, a)
    }
    /// Inclusive integer range.
    fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % ((hi - lo + 1) as u64)) as i32
    }
    fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }
    fn angle(&mut self) -> f32 {
        self.f() * TAU
    }
    fn seed(&mut self) -> u32 {
        (self.next() >> 32) as u32
    }
    fn fork(&mut self) -> Rng {
        Rng::new(self.next())
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[(self.next() % xs.len() as u64) as usize]
    }
    fn unit(&mut self) -> V3 {
        loop {
            let v = v3(self.sym(1.0), self.sym(1.0), self.sym(1.0));
            let l = v.len();
            if l > 0.05 && l <= 1.0 {
                return v / l;
            }
        }
    }
}

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B1)
        ^ (x as u32).wrapping_mul(0x85EB_CA77)
        ^ (y as u32).wrapping_mul(0xC2B2_AE3D)
        ^ (z as u32).wrapping_mul(0x27D4_EB2F);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Deterministic hash to [0, 1).
fn hashf(i: u32, seed: u32) -> f32 {
    (hash3(i as i32, 17, 91, seed) >> 8) as f32 * (1.0 / 16_777_216.0)
}

fn grad3(h: u32, x: f32, y: f32, z: f32) -> f32 {
    match h & 15 {
        0 | 12 => x + y,
        1 | 13 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 | 14 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

/// 3D gradient noise, roughly in [-1, 1].
fn noise3(p: V3, seed: u32) -> f32 {
    let (fx, fy, fz) = (p.x.floor(), p.y.floor(), p.z.floor());
    let (ix, iy, iz) = (fx as i32, fy as i32, fz as i32);
    let (x, y, z) = (p.x - fx, p.y - fy, p.z - fz);
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v, w) = (fade(x), fade(y), fade(z));
    let g = |dx: i32, dy: i32, dz: i32| {
        grad3(
            hash3(ix + dx, iy + dy, iz + dz, seed),
            x - dx as f32,
            y - dy as f32,
            z - dz as f32,
        )
    };
    let x00 = lerp(g(0, 0, 0), g(1, 0, 0), u);
    let x10 = lerp(g(0, 1, 0), g(1, 1, 0), u);
    let x01 = lerp(g(0, 0, 1), g(1, 0, 1), u);
    let x11 = lerp(g(0, 1, 1), g(1, 1, 1), u);
    (lerp(lerp(x00, x10, v), lerp(x01, x11, v), w) * 0.95).clamp(-1.0, 1.0)
}

/// Fractal sum of `noise3`, normalized to roughly [-1, 1].
fn fbm(p: V3, octaves: u32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut q = p;
    for o in 0..octaves {
        sum += amp * noise3(q, seed.wrapping_add(o * 1013));
        norm += amp;
        amp *= 0.5;
        q = q * 2.03 + v3(17.1, 3.7, 9.2);
    }
    sum / norm
}

// =====================================================================================
// Colors
// =====================================================================================

type C3 = [f32; 3];

fn hex(h: u32) -> C3 {
    srgb(
        ((h >> 16) & 255) as f32 / 255.0,
        ((h >> 8) & 255) as f32 / 255.0,
        (h & 255) as f32 / 255.0,
    )
}

fn cmix(a: C3, b: C3, t: f32) -> C3 {
    let t = clamp01(t);
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

fn cscale(a: C3, s: f32) -> C3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Warm / cool hue shift; `n` in [-1, 1].
fn ctint(c: C3, n: f32, amt: f32) -> C3 {
    let k = n * amt;
    [c[0] * (1.0 + k), c[1] * (1.0 + 0.35 * k), c[2] * (1.0 - k)]
}

fn rgba(c: C3, a: f32) -> [f32; 4] {
    [clamp01(c[0]), clamp01(c[1]), clamp01(c[2]), clamp01(a)]
}

// =====================================================================================
// Mesh builder
// =====================================================================================

#[derive(Clone, Default)]
struct Mesh {
    p: Vec<V3>,
    n: Vec<V3>,
    c: Vec<[f32; 4]>,
    /// Builder-specific parameterisation (ring angle / length, strip across / along...).
    uv: Vec<[f32; 2]>,
    i: Vec<u32>,
}

#[derive(Clone, Copy)]
enum Ring {
    Pole(u32),
    Loop(u32),
}

impl Mesh {
    fn new() -> Mesh {
        Mesh::default()
    }
    fn vn(&self) -> u32 {
        self.p.len() as u32
    }
    fn tris(&self) -> usize {
        self.i.len() / 3
    }
    fn vert(&mut self, p: V3, n: V3, uv: [f32; 2]) -> u32 {
        let k = self.vn();
        self.p.push(p);
        self.n.push(n);
        self.c.push([1.0, 1.0, 1.0, 0.0]);
        self.uv.push(uv);
        k
    }
    fn tri(&mut self, a: u32, b: u32, c: u32) {
        self.i.extend_from_slice(&[a, b, c]);
    }
    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }
    fn append(&mut self, o: &Mesh) {
        let base = self.vn();
        self.p.extend_from_slice(&o.p);
        self.n.extend_from_slice(&o.n);
        self.c.extend_from_slice(&o.c);
        self.uv.extend_from_slice(&o.uv);
        self.i.extend(o.i.iter().map(|k| k + base));
    }
    /// Rigid transform (rotation `m`, then translation `t`).
    fn transform(&mut self, m: &M3, t: V3) {
        for p in &mut self.p {
            *p = m.apply(*p) + t;
        }
        for n in &mut self.n {
            *n = m.apply(*n).norm();
        }
    }
    fn translate(&mut self, t: V3) {
        for p in &mut self.p {
            *p += t;
        }
    }
    fn xf(mut self, m: &M3, t: V3) -> Mesh {
        self.transform(m, t);
        self
    }
    fn at(mut self, t: V3) -> Mesh {
        self.translate(t);
        self
    }
    /// Non-uniform scale with correct normal transform.
    fn scale3(&mut self, s: V3) {
        for p in &mut self.p {
            *p = p.cmul(s);
        }
        for n in &mut self.n {
            *n = v3(n.x / s.x, n.y / s.y, n.z / s.z).norm();
        }
    }
    fn face_n(&self, t: usize) -> V3 {
        let a = self.p[self.i[t * 3] as usize];
        let b = self.p[self.i[t * 3 + 1] as usize];
        let c = self.p[self.i[t * 3 + 2] as usize];
        (b - a).cross(c - a)
    }
    /// Area weighted smooth normals over shared vertices.
    fn smooth_normals(&mut self) {
        let mut acc = vec![V3::ZERO; self.p.len()];
        for t in 0..self.tris() {
            let f = self.face_n(t);
            for k in 0..3 {
                acc[self.i[t * 3 + k] as usize] += f;
            }
        }
        for (n, a) in self.n.iter_mut().zip(acc) {
            if a.len() > 1e-14 {
                *n = a.norm();
            }
        }
    }
    /// Split every triangle into its own vertices (faceted look). `smooth` in 0..1
    /// blends the face normal towards the previous (smooth) vertex normal.
    fn unweld(&mut self, smooth: f32) {
        let mut out = Mesh::new();
        for t in 0..self.tris() {
            let f = self.face_n(t).norm();
            for k in 0..3 {
                let v = self.i[t * 3 + k] as usize;
                let n = if smooth > 0.0 {
                    (f * (1.0 - smooth) + self.n[v] * smooth).norm()
                } else {
                    f
                };
                out.p.push(self.p[v]);
                out.n.push(n);
                out.c.push(self.c[v]);
                out.uv.push(self.uv[v]);
                out.i.push(out.i.len() as u32);
            }
        }
        *self = out;
    }
    /// Append a mirrored copy (reversed winding, negated normals) so a sheet is visible
    /// from both sides. `back_dim` darkens the back copy's color.
    fn double_sided(&mut self, back_dim: f32) {
        let base = self.vn();
        let nv = self.p.len();
        for k in 0..nv {
            let c = self.c[k];
            self.p.push(self.p[k]);
            self.n.push(-self.n[k]);
            self.c
                .push([c[0] * back_dim, c[1] * back_dim, c[2] * back_dim, c[3]]);
            self.uv.push(self.uv[k]);
        }
        let nt = self.tris();
        for t in 0..nt {
            let (a, b, c) = (self.i[t * 3], self.i[t * 3 + 1], self.i[t * 3 + 2]);
            self.i.extend_from_slice(&[a + base, c + base, b + base]);
        }
    }
    /// Blend normals towards `dir(p)` by `k` (soft volumetric foliage lighting).
    /// The result never leans more than ~75 degrees away from the original normal, so
    /// lighting stays on the correct side of each face.
    fn bend_normals(&mut self, k: f32, dir: impl Fn(V3) -> V3) {
        const COS_LIMIT: f32 = 0.26;
        for (n, p) in self.n.iter_mut().zip(&self.p) {
            let n0 = *n;
            let d = dir(*p).norm();
            let mut b = (n0 * (1.0 - k) + d * k).norm();
            if b.dot(n0) < COS_LIMIT {
                let t = d - n0 * d.dot(n0);
                let t = if t.len() > 1e-6 { t.norm() } else { b };
                b = (n0 * COS_LIMIT + t * (1.0 - COS_LIMIT * COS_LIMIT).sqrt()).norm();
            }
            *n = b;
        }
    }
    fn paint(&mut self, f: impl Fn(V3, V3, [f32; 2]) -> [f32; 4]) {
        for k in 0..self.p.len() {
            self.c[k] = f(self.p[k], self.n[k], self.uv[k]);
        }
    }
    /// Multiply rgb by `f(p)` (ambient occlusion style darkening).
    fn shade(&mut self, f: impl Fn(V3) -> f32) {
        for (c, p) in self.c.iter_mut().zip(&self.p) {
            let s = f(*p);
            c[0] *= s;
            c[1] *= s;
            c[2] *= s;
        }
    }
    fn set_alpha(&mut self, f: impl Fn(V3, [f32; 2]) -> f32) {
        for k in 0..self.p.len() {
            self.c[k][3] = f(self.p[k], self.uv[k]);
        }
    }
    fn bounds(&self) -> (V3, V3) {
        let mut mn = v3(f32::MAX, f32::MAX, f32::MAX);
        let mut mx = v3(f32::MIN, f32::MIN, f32::MIN);
        for p in &self.p {
            mn = v3(mn.x.min(p.x), mn.y.min(p.y), mn.z.min(p.z));
            mx = v3(mx.x.max(p.x), mx.y.max(p.y), mx.z.max(p.z));
        }
        (mn, mx)
    }
    /// Darken vertices close to the ground (contact ambient occlusion).
    fn ground_ao(&mut self, height: f32, strength: f32) {
        self.shade(|p| 1.0 - strength * (1.0 - smoothstep(-0.15 * height, height, p.y)));
    }
    fn into_data(self) -> MeshData {
        let fix = |v: V3| {
            if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() {
                v
            } else {
                V3::ZERO
            }
        };
        MeshData {
            positions: self.p.iter().map(|p| fix(*p).arr()).collect(),
            normals: self.n.iter().map(|n| fix(*n).norm().arr()).collect(),
            colors: self
                .c
                .iter()
                .map(|c| {
                    let g = |x: f32| if x.is_finite() { clamp01(x) } else { 0.5 };
                    [g(c[0]), g(c[1]), g(c[2]), g(c[3])]
                })
                .collect(),
            indices: self.i,
        }
    }
}

// =====================================================================================
// Primitives
// =====================================================================================

/// Connect two consecutive rings. Ring vertices go around with increasing angle in the
/// `V3::polar` convention relative to the progression direction, which yields outward
/// facing triangles.
fn connect(m: &mut Mesh, a: Ring, b: Ring, sides: u32) {
    for j in 0..sides {
        let j1 = (j + 1) % sides;
        match (a, b) {
            (Ring::Loop(a0), Ring::Loop(b0)) => {
                m.tri(a0 + j, a0 + j1, b0 + j1);
                m.tri(a0 + j, b0 + j1, b0 + j);
            }
            (Ring::Loop(a0), Ring::Pole(p)) => m.tri(a0 + j, a0 + j1, p),
            (Ring::Pole(p), Ring::Loop(b0)) => m.tri(p, b0 + j1, b0 + j),
            _ => {}
        }
    }
}

/// Build a surface from successive rings (a ring of length 1 is a pole). Smooth normals.
/// uv = (index around / sides, ring index / (rings - 1)).
fn rings_mesh(rings: &[Vec<V3>]) -> Mesh {
    let mut m = Mesh::new();
    let sides = rings.iter().map(|r| r.len()).max().unwrap_or(1) as u32;
    let last = (rings.len().max(2) - 1) as f32;
    let mut handles = Vec::with_capacity(rings.len());
    for (k, ring) in rings.iter().enumerate() {
        let v = k as f32 / last;
        if ring.len() == 1 {
            handles.push(Ring::Pole(m.vert(ring[0], V3::Y, [0.5, v])));
        } else {
            let s = m.vn();
            for (j, p) in ring.iter().enumerate() {
                m.vert(*p, V3::Y, [j as f32 / ring.len() as f32, v]);
            }
            handles.push(Ring::Loop(s));
        }
    }
    for k in 0..rings.len().saturating_sub(1) {
        connect(&mut m, handles[k], handles[k + 1], sides);
    }
    m.smooth_normals();
    m
}

/// Surface of revolution around +Y. Walking along the profile (r, y) the surface faces
/// to the right, i.e. bottom-center -> outward -> up -> top-center gives a closed solid.
fn lathe(profile: &[(f32, f32)], sides: u32, phase: f32) -> Mesh {
    let rings: Vec<Vec<V3>> = profile
        .iter()
        .map(|&(r, y)| {
            if r < 1e-5 {
                vec![v3(0.0, y, 0.0)]
            } else {
                (0..sides)
                    .map(|j| V3::polar(phase + j as f32 * TAU / sides as f32) * r + V3::Y * y)
                    .collect()
            }
        })
        .collect();
    rings_mesh(&rings)
}

#[derive(Clone, Copy, PartialEq)]
enum Cap {
    Open,
    Flat,
}

/// Rotation-minimising frames along a polyline: (tangent, normal, binormal).
fn path_frames(path: &[V3]) -> Vec<(V3, V3, V3)> {
    let n = path.len();
    let mut out = Vec::with_capacity(n);
    let tan = |i: usize| -> V3 {
        let a = path[i.saturating_sub(1)];
        let b = path[(i + 1).min(n - 1)];
        (b - a).norm()
    };
    let t0 = tan(0);
    let refv = if t0.y.abs() < 0.9 { V3::Y } else { V3::X };
    let mut nrm = (refv - t0 * refv.dot(t0)).norm();
    if refv.x > 0.5 {
        // vertical start: keep the lathe convention (normal = +X)
        nrm = (V3::X - t0 * t0.x).norm();
    }
    for i in 0..n {
        let t = tan(i);
        nrm = (nrm - t * nrm.dot(t)).norm();
        let b = t.cross(nrm);
        out.push((t, nrm, b));
    }
    out
}

/// Sweep a circle-ish cross-section along `path`; `radius(ring, angle)` may vary per
/// vertex. A ring of radius ~0 becomes a pole (pointed end). uv = (angle/TAU, length t),
/// cap vertices get uv.y = -1 (start) or 2 (end) and uv.x = radial fraction.
fn tube_fn(
    path: &[V3],
    sides: u32,
    cap0: Cap,
    cap1: Cap,
    radius: impl Fn(usize, f32) -> f32,
) -> Mesh {
    let frames = path_frames(path);
    let mut rings = Vec::with_capacity(path.len());
    for (i, &c) in path.iter().enumerate() {
        let (_, nn, bb) = frames[i];
        if radius(i, 0.0) < 1e-4 {
            rings.push(vec![c]);
            continue;
        }
        let ring: Vec<V3> = (0..sides)
            .map(|j| {
                let a = j as f32 * TAU / sides as f32;
                c + (nn * a.cos() + bb * a.sin()) * radius(i, a)
            })
            .collect();
        rings.push(ring);
    }
    let mut m = rings_mesh(&rings);
    // uv.y = normalised length along the path
    let mut acc = vec![0.0f32; path.len()];
    for i in 1..path.len() {
        acc[i] = acc[i - 1] + (path[i] - path[i - 1]).len();
    }
    let total = acc[path.len() - 1].max(1e-6);
    {
        let mut k = 0usize;
        for (i, r) in rings.iter().enumerate() {
            for _ in 0..r.len() {
                m.uv[k][1] = acc[i] / total;
                k += 1;
            }
        }
    }
    for (ci, cap) in [(0usize, cap0), (path.len() - 1, cap1)] {
        if cap != Cap::Flat || rings[ci].len() < 3 {
            continue;
        }
        let (t, _, _) = frames[ci];
        let nrm = if ci == 0 { -t } else { t };
        let tag = if ci == 0 { -1.0 } else { 2.0 };
        let mut cm = Mesh::new();
        let center = cm.vert(path[ci], nrm, [0.0, tag]);
        let s = cm.vn();
        for p in &rings[ci] {
            cm.vert(*p, nrm, [1.0, tag]);
        }
        for j in 0..sides {
            let j1 = (j + 1) % sides;
            if ci == 0 {
                cm.tri(center, s + j1, s + j);
            } else {
                cm.tri(center, s + j, s + j1);
            }
        }
        m.append(&cm);
    }
    m
}

fn tube(path: &[V3], radii: &[f32], sides: u32, cap0: Cap, cap1: Cap) -> Mesh {
    tube_fn(path, sides, cap0, cap1, |i, _| radii[i])
}

/// Quadratic Bezier sampled into `n + 1` points.
fn bezier(a: V3, ctrl: V3, b: V3, n: usize) -> Vec<V3> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            a * (u * u) + ctrl * (2.0 * u * t) + b * (t * t)
        })
        .collect()
}

/// Unit icosphere with shared vertices (level 0 = 20 tris, 1 = 80, 2 = 320).
fn icosphere(level: u32) -> Mesh {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut verts: Vec<V3> = [
        v3(-1.0, t, 0.0),
        v3(1.0, t, 0.0),
        v3(-1.0, -t, 0.0),
        v3(1.0, -t, 0.0),
        v3(0.0, -1.0, t),
        v3(0.0, 1.0, t),
        v3(0.0, -1.0, -t),
        v3(0.0, 1.0, -t),
        v3(t, 0.0, -1.0),
        v3(t, 0.0, 1.0),
        v3(-t, 0.0, -1.0),
        v3(-t, 0.0, 1.0),
    ]
    .iter()
    .map(|v| v.norm())
    .collect();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    fn mid(cache: &mut HashMap<(u32, u32), u32>, verts: &mut Vec<V3>, a: u32, b: u32) -> u32 {
        let key = if a < b { (a, b) } else { (b, a) };
        if let Some(&k) = cache.get(&key) {
            return k;
        }
        verts.push(((verts[a as usize] + verts[b as usize]) * 0.5).norm());
        let k = verts.len() as u32 - 1;
        cache.insert(key, k);
        k
    }
    for _ in 0..level {
        let mut cache = HashMap::new();
        let mut nf = Vec::with_capacity(faces.len() * 4);
        for f in &faces {
            let ab = mid(&mut cache, &mut verts, f[0], f[1]);
            let bc = mid(&mut cache, &mut verts, f[1], f[2]);
            let ca = mid(&mut cache, &mut verts, f[2], f[0]);
            nf.push([f[0], ab, ca]);
            nf.push([f[1], bc, ab]);
            nf.push([f[2], ca, bc]);
            nf.push([ab, bc, ca]);
        }
        faces = nf;
    }
    let mut m = Mesh::new();
    for v in &verts {
        m.vert(*v, *v, [0.0, 0.0]);
    }
    for f in &faces {
        m.tri(f[0], f[1], f[2]);
    }
    m
}

/// Unit octahedron (8 tris) with smooth normals: tiny flowers, embers, berries.
fn octa() -> Mesh {
    let mut m = Mesh::new();
    let v = [V3::X, -V3::X, V3::Y, -V3::Y, V3::Z, -V3::Z];
    for p in v {
        m.vert(p, p, [0.0, 0.0]);
    }
    for sx in [0u32, 1] {
        for sy in [2u32, 3] {
            for sz in [4u32, 5] {
                let sign = (if sx == 0 { 1 } else { -1 })
                    * (if sy == 2 { 1 } else { -1 })
                    * (if sz == 4 { 1 } else { -1 });
                if sign > 0 {
                    m.tri(sx, sy, sz)
                } else {
                    m.tri(sx, sz, sy)
                }
            }
        }
    }
    m
}

/// Noise-displaced ellipsoid with smooth normals (foliage clumps, bushes).
fn blob(level: u32, radii: V3, amp: f32, freq: f32, seed: u32, flat_bottom: f32) -> Mesh {
    let mut m = icosphere(level);
    let off = v3(
        hashf(1, seed) * 50.0,
        hashf(2, seed) * 50.0,
        hashf(3, seed) * 50.0,
    );
    for p in &mut m.p {
        let d = *p;
        let r = 1.0
            + amp * fbm(d * freq + off, 3, seed)
            + amp * 0.35 * noise3(d * freq * 3.1 + off, seed ^ 7);
        let mut q = d * r;
        if q.y < 0.0 {
            q.y *= flat_bottom;
        }
        *p = q.cmul(radii);
    }
    m.smooth_normals();
    m
}

/// Faceted rock: noise displaced icosphere with planar cuts (chiselled faces).
/// `shard`: prismatic column (vertical side cuts + tilted top) instead of a boulder.
/// uv.x stores the relative radius after displacement (low = crevice), uv.y the height.
fn rock_mesh(
    level: u32,
    radii: V3,
    rough: f32,
    cuts: usize,
    smooth: f32,
    seed: u32,
    shard: bool,
) -> Mesh {
    let mut m = icosphere(level);
    let mut rng = Rng::new(seed as u64 ^ 0xC0FFEE);
    let off = v3(
        rng.range(0.0, 50.0),
        rng.range(0.0, 50.0),
        rng.range(0.0, 50.0),
    );
    let mut planes: Vec<(V3, f32)> = Vec::new();
    if shard {
        let a0 = rng.angle();
        for k in 0..cuts {
            let a = a0 + k as f32 * TAU / cuts as f32 + rng.sym(0.3);
            planes.push((
                (V3::polar(a) + V3::Y * rng.sym(0.15)).norm(),
                rng.range(0.6, 0.78),
            ));
        }
        let top = (V3::Y + V3::polar(rng.angle()) * rng.range(0.25, 0.7)).norm();
        planes.push((top, rng.range(0.7, 0.85)));
        planes.push((
            (V3::Y + V3::polar(rng.angle()) * 1.4).norm(),
            rng.range(0.7, 0.82),
        ));
    } else {
        for _ in 0..cuts {
            let mut n = rng.unit();
            n.y = n.y * 0.6 + 0.25; // favour flat-ish tops and steep sides
            planes.push((n.norm(), rng.range(0.62, 0.86)));
        }
    }
    planes.push((-V3::Y, 0.6));
    for k in 0..m.p.len() {
        let d = m.p[k];
        let r = 1.0
            + rough * fbm(d * 1.25 + off, 3, seed)
            + rough * 0.3 * noise3(d * 3.7 + off, seed ^ 11);
        let mut q = d * r;
        for (n, o) in &planes {
            let s = q.dot(*n) - o;
            if s > 0.0 {
                q -= *n * (s * 0.92);
            }
        }
        m.uv[k] = [q.len(), d.y];
        m.p[k] = q.cmul(radii);
    }
    m.smooth_normals();
    m.unweld(smooth);
    m
}

/// Axis aligned box centered at the origin with flat normals (24 verts, 12 tris).
fn cuboid(h: V3) -> Mesh {
    let mut m = Mesh::new();
    let faces = [
        (V3::X, V3::Y, V3::Z),
        (-V3::X, V3::Z, V3::Y),
        (V3::Y, V3::Z, V3::X),
        (-V3::Y, V3::X, V3::Z),
        (V3::Z, V3::X, V3::Y),
        (-V3::Z, V3::Y, V3::X),
    ];
    for (n, u, v) in faces {
        let c = n.cmul(h);
        let (uu, vv) = (u.cmul(h), v.cmul(h));
        let a = m.vert(c - uu - vv, n, [0.0, 0.0]);
        let b = m.vert(c + uu - vv, n, [1.0, 0.0]);
        let cc = m.vert(c + uu + vv, n, [1.0, 1.0]);
        let d = m.vert(c - uu + vv, n, [0.0, 1.0]);
        m.quad(a, b, cc, d);
    }
    m
}

fn box_at(center: V3, half: V3, rot: &M3) -> Mesh {
    cuboid(half).xf(rot, center)
}

/// Box spanning from `a` to `b` (its long axis), cross-section `w` (sideways) x `h` (up).
fn box_between(a: V3, b: V3, w: f32, h: f32, up: V3) -> Mesh {
    let d = b - a;
    let len = d.len().max(1e-4);
    let x = d / len;
    let mut z = x.cross(up);
    if z.len() < 1e-4 {
        z = x.cross(V3::X);
    }
    let z = z.norm();
    let y = z.cross(x);
    let rot = M3 { x, y, z };
    box_at((a + b) * 0.5, v3(len * 0.5, h * 0.5, w * 0.5), &rot)
}

/// Hexahedron from 8 corners indexed by bits (x = 1, y = 2, z = 4) of a right-handed
/// deformed cube. Flat shaded.
fn hexa(c: [V3; 8]) -> Mesh {
    let faces = [
        [0, 4, 6, 2],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 2, 3, 1],
        [4, 5, 7, 6],
    ];
    let mut m = Mesh::new();
    for f in faces {
        let s = m.vn();
        for (k, &ix) in f.iter().enumerate() {
            m.vert(
                c[ix],
                V3::Y,
                [(k == 1 || k == 2) as u32 as f32, (k >= 2) as u32 as f32],
            );
        }
        m.quad(s, s + 1, s + 2, s + 3);
    }
    m.unweld(0.0);
    m
}

/// Annular sector block (well stones, arch voussoirs laid flat): angles a0 < a1.
fn ring_block(r0: f32, r1: f32, a0: f32, a1: f32, y0: f32, y1: f32) -> Mesh {
    let p = |a: f32, y: f32, r: f32| V3::polar(a) * r + V3::Y * y;
    let mut c = [V3::ZERO; 8];
    for k in 0..8 {
        let a = if k & 1 != 0 { a1 } else { a0 };
        let y = if k & 2 != 0 { y1 } else { y0 };
        let r = if k & 4 != 0 { r1 } else { r0 };
        c[k] = p(a, y, r);
    }
    hexa(c)
}

/// Convex polygon (CCW in the XY plane) extruded along Z by `depth` (centered).
fn prism(poly: &[(f32, f32)], depth: f32) -> Mesh {
    let mut m = Mesh::new();
    let d = depth * 0.5;
    let n = poly.len() as u32;
    let f = m.vn();
    for &(x, y) in poly {
        m.vert(v3(x, y, d), V3::Z, [0.0, 0.0]);
    }
    for k in 1..n - 1 {
        m.tri(f, f + k, f + k + 1);
    }
    let b = m.vn();
    for &(x, y) in poly {
        m.vert(v3(x, y, -d), -V3::Z, [0.0, 0.0]);
    }
    for k in 1..n - 1 {
        m.tri(b, b + k + 1, b + k);
    }
    for k in 0..n as usize {
        let (ax, ay) = poly[k];
        let (bx, by) = poly[(k + 1) % n as usize];
        let nn = v3(by - ay, -(bx - ax), 0.0).norm();
        let s = m.vn();
        m.vert(v3(ax, ay, -d), nn, [0.0, 0.0]);
        m.vert(v3(bx, by, -d), nn, [1.0, 0.0]);
        m.vert(v3(bx, by, d), nn, [1.0, 1.0]);
        m.vert(v3(ax, ay, d), nn, [0.0, 1.0]);
        m.quad(s, s + 1, s + 2, s + 3);
    }
    m
}

/// Strip along `spine`. At station i the strip spans `side[i] * +-w[i]`; `fold[i]` is added
/// to the two edges (V-shaped cross-section when `center` is true). A station with w ~ 0
/// collapses to a single vertex (pointed tip / base). Front face normal ~ side x along.
/// uv = (across 0..1, along 0..1). Single sided; call `double_sided` for leaves.
fn ribbon(spine: &[V3], side: &[V3], w: &[f32], fold: &[V3], center: bool) -> Mesh {
    let mut m = Mesh::new();
    let n = spine.len();
    let mut st: Vec<Vec<u32>> = Vec::with_capacity(n);
    for i in 0..n {
        let v = i as f32 / (n - 1).max(1) as f32;
        if w[i] < 1e-4 {
            st.push(vec![m.vert(spine[i], V3::Y, [0.5, v])]);
            continue;
        }
        let l = spine[i] - side[i] * w[i] + fold[i];
        let r = spine[i] + side[i] * w[i] + fold[i];
        let mut s = vec![m.vert(l, V3::Y, [0.0, v])];
        if center {
            s.push(m.vert(spine[i], V3::Y, [0.5, v]));
        }
        s.push(m.vert(r, V3::Y, [1.0, v]));
        st.push(s);
    }
    for i in 0..n - 1 {
        let (a, b) = (&st[i], &st[i + 1]);
        match (a.len(), b.len()) {
            (1, 1) => {}
            (1, _) => {
                for k in 0..b.len() - 1 {
                    m.tri(a[0], b[k + 1], b[k]);
                }
            }
            (_, 1) => {
                for k in 0..a.len() - 1 {
                    m.tri(a[k], a[k + 1], b[0]);
                }
            }
            _ => {
                for k in 0..a.len() - 1 {
                    m.quad(a[k], a[k + 1], b[k + 1], b[k]);
                }
            }
        }
    }
    m.smooth_normals();
    m
}

/// Simple leaf / blade: spine from `base` along a gently curved path to `tip`.
/// `widths` sampled along the stations (0 for pointed ends).
fn leaf_strip(pts: &[V3], widths: &[f32], side: V3, fold: f32, center: bool) -> Mesh {
    let n = pts.len();
    let sides: Vec<V3> = (0..n).map(|_| side).collect();
    let mut folds = Vec::with_capacity(n);
    for i in 0..n {
        let along = (pts[(i + 1).min(n - 1)] - pts[i.saturating_sub(1)]).norm();
        let up = side.cross(along).norm();
        folds.push(-up * (widths[i] * fold));
    }
    ribbon(pts, &sides, widths, &folds, center)
}

// =====================================================================================
// Shared painters
// =====================================================================================

/// Trunk / branch bark: vertical streaks, darker base, slight sway near the top.
fn paint_bark(m: &mut Mesh, base: C3, dark: C3, seed: u32, tree_h: f32) {
    m.paint(|p, _, _| {
        let s = noise3(v3(p.x * 3.0, p.y * 0.45, p.z * 3.0), seed);
        let f = fbm(p * 0.9, 2, seed ^ 0x55);
        let mut c = cmix(base, dark, 0.35 + 0.45 * s);
        c = cscale(c, 1.0 + 0.12 * f);
        let base_dark = 0.72 + 0.28 * smoothstep(0.0, 1.2, p.y);
        c = cscale(c, base_dark);
        let a = 0.22 * clamp01(p.y / tree_h).powi(2);
        rgba(c, a)
    });
}

/// Foliage clump paint: palette color with noise hue jitter, height gradient and
/// occlusion by neighbouring clumps.
struct Canopy {
    clumps: Vec<(V3, f32)>,
    center: V3,
    radii: V3,
}

impl Canopy {
    fn occlusion(&self, p: V3, own: usize) -> f32 {
        let mut occ = 0.0;
        for (j, &(c, r)) in self.clumps.iter().enumerate() {
            if j != own {
                occ += (1.0 - (p - c).len() / (r * 1.15)).max(0.0);
            }
        }
        let rel = (p - self.center).cmul(v3(
            1.0 / self.radii.x,
            1.0 / self.radii.y,
            1.0 / self.radii.z,
        ));
        let interior = 1.0 - smoothstep(0.35, 0.95, rel.len());
        (occ * 0.8 + interior * 0.6).min(1.0)
    }
    fn paint(
        &self,
        m: &mut Mesh,
        own: usize,
        col: C3,
        tip: C3,
        seed: u32,
        trunk_axis: V3,
        sway_r: f32,
    ) {
        let bottom = self.center.y - self.radii.y;
        let span = self.radii.y * 2.0;
        m.paint(|p, n, _| {
            let g = clamp01((p.y - bottom) / span);
            let nz = fbm(p * 0.8, 2, seed);
            let hi = clamp01(n.y * 0.5 + 0.5 * g + 0.3 * nz);
            let mut c = cmix(col, tip, hi * hi * 0.45);
            c = ctint(c, noise3(p * 0.35, seed ^ 9), 0.18);
            let occ = self.occlusion(p, own);
            c = cscale(c, (0.62 + 0.48 * g) * (1.0 - 0.5 * occ) * (1.0 + 0.12 * nz));
            let rad = (p - trunk_axis).horiz().len() / sway_r;
            let a = 0.25 + 0.5 * clamp01(rad) + 0.25 * g;
            rgba(c, a)
        });
    }
}

// =====================================================================================
// Trees
// =====================================================================================

struct TierSpec {
    r: f32,
    h: f32,
    n: usize,
    droop: f32,
    notch: f32,
    rot: f32,
    lift: f32,
    jitter: f32,
}

/// One conifer tier: a star-shaped, slightly drooping "umbrella" with a concave underside.
/// Local space: rim at y ~ 0, apex at y = h. uv.y: 0 bottom pole, < 0.5 underside,
/// 0.5 rim, > 0.5 top surface, 1 apex.
fn tier_mesh(t: &TierSpec, rng: &mut Rng, full: bool) -> Mesh {
    let n2 = t.n * 2;
    let mut rim = Vec::with_capacity(n2);
    let mut mid = Vec::with_capacity(n2);
    let mut under = Vec::with_capacity(n2);
    let step = PI / t.n as f32;
    for k in 0..n2 {
        let tip = k % 2 == 0;
        let a = t.rot + k as f32 * step + rng.sym(0.22 * step);
        let dir = V3::polar(a);
        let r = if tip {
            t.r * (1.0 + rng.sym(t.jitter))
        } else {
            t.r * (1.0 - t.notch) * (1.0 + rng.sym(t.jitter))
        };
        let y = if tip {
            -t.droop * rng.range(0.7, 1.3)
        } else {
            -t.droop * 0.2 + t.h * 0.05
        };
        rim.push(dir * r + V3::Y * y);
        let ym = lerp(y, t.h, 0.45) + t.lift * t.h * 0.14;
        mid.push(dir * (r * 0.56) + V3::Y * ym);
        under.push(dir * (r * 0.34) + V3::Y * (y * 0.3 + t.h * 0.08));
    }
    let bottom = v3(0.0, t.h * 0.24, 0.0);
    let apex = v3(rng.sym(0.04) * t.r, t.h, rng.sym(0.04) * t.r);
    if full {
        rings_mesh(&[vec![bottom], under, rim, mid, vec![apex]])
    } else {
        // far LOD: crease at the rim so the top and underside keep distinct normals
        let mut m = rings_mesh(&[vec![bottom], rim.clone()]);
        for uv in m.uv.iter_mut() {
            uv[1] *= 0.5;
        }
        let mut top = rings_mesh(&[rim, vec![apex]]);
        for uv in top.uv.iter_mut() {
            uv[1] = 0.5 + 0.5 * uv[1];
        }
        m.append(&top);
        m
    }
}

fn paint_tier(
    m: &mut Mesh,
    leaf: C3,
    tip: C3,
    snow: bool,
    seed: u32,
    axis: V3,
    r: f32,
    tree_h: f32,
) {
    let snow_c = srgb(0.93, 0.95, 0.98);
    let snow_shadow = srgb(0.78, 0.83, 0.92);
    m.paint(|p, n, uv| {
        let radial = clamp01((p - axis).horiz().len() / r.max(0.01));
        let nz = noise3(p * 0.9, seed);
        let mut c = cmix(leaf, tip, radial * radial * 0.75 + 0.15 * nz);
        let side = uv[1];
        let shade = if side < 0.2 {
            0.42
        } else if side < 0.45 {
            0.58
        } else if side < 0.55 {
            0.9
        } else {
            1.0
        };
        c = ctint(c, noise3(p * 0.25, seed ^ 5), 0.15);
        c = cscale(
            c,
            shade * (0.82 + 0.3 * clamp01(p.y / tree_h)) * (1.0 + 0.1 * nz),
        );
        if snow {
            let s = smoothstep(
                0.38,
                0.7,
                n.y + 0.4 * noise3(p * 1.4, seed ^ 3) + (side - 0.5) * 0.9,
            );
            let sc = cmix(snow_shadow, snow_c, clamp01(n.y));
            c = cmix(c, sc, s);
        }
        let a = 0.12 + 0.68 * radial + 0.2 * clamp01(p.y / tree_h);
        rgba(c, a)
    });
}

/// Trunk along a gently wandering path from below ground to `top`, with root flare.
fn trunk_mesh(
    rng: &mut Rng,
    base: V3,
    top: V3,
    r0: f32,
    r1: f32,
    sides: u32,
    rings: usize,
    flare: f32,
    wobble: f32,
) -> Mesh {
    let seed = rng.seed();
    let h = top.y - base.y;
    let path: Vec<V3> = (0..=rings)
        .map(|i| {
            let t = i as f32 / rings as f32;
            let mut p = base.lerp(top, t);
            let w = wobble * (t * (1.0 - t) * 4.0).min(1.0);
            p.x += noise3(v3(t * 2.5, 0.0, 0.0), seed) * w;
            p.z += noise3(v3(t * 2.5, 5.0, 0.0), seed) * w;
            p
        })
        .collect();
    let lobes = rng.int(3, 5) as f32;
    let lphase = rng.angle();
    tube_fn(&path, sides, Cap::Open, Cap::Open, |i, a| {
        let t = i as f32 / rings as f32;
        let y = path[i].y;
        let mut r = lerp(r0, r1, t.powf(0.8));
        let f = (1.0 - (y.max(0.0) / (0.12 * h).max(0.5))).max(0.0);
        if flare > 0.0 && f > 0.0 {
            let lobe = 0.5 + 0.5 * (lobes * a + lphase).sin();
            r *= 1.0 + flare * f * f * (0.6 + 0.8 * lobe);
        }
        r * (1.0 + 0.05 * noise3(v3(a.cos() * 2.0, y * 0.7, a.sin() * 2.0), seed ^ 3))
    })
}

fn conifer(kind: AssetKind, rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let mut dr = rng.fork();
    // (height range, crown base, radius frac, tiers, branches, droop, notch, gap, wobble)
    struct S {
        h: (f32, f32),
        base: f32,
        rad: f32,
        tiers: (i32, i32),
        n: usize,
        droop: f32,
        notch: f32,
        overlap: f32,
        offset: f32,
        leaf: C3,
        tip: C3,
        bark: C3,
        bark_dark: C3,
        snow: bool,
    }
    let s = match kind {
        AssetKind::Pine => S {
            h: (15.0, 21.0),
            base: 0.44,
            rad: 0.2,
            tiers: (6, 7),
            n: 7,
            droop: 0.14,
            notch: 0.45,
            overlap: 1.1,
            offset: 0.16,
            leaf: srgb(0.19, 0.31, 0.14),
            tip: srgb(0.34, 0.45, 0.19),
            bark: srgb(0.56, 0.36, 0.22),
            bark_dark: srgb(0.30, 0.20, 0.14),
            snow: false,
        },
        AssetKind::Fir => S {
            h: (10.0, 15.0),
            base: 0.07,
            rad: 0.27,
            tiers: (8, 9),
            n: 8,
            droop: 0.22,
            notch: 0.3,
            overlap: 1.7,
            offset: 0.03,
            leaf: srgb(0.13, 0.30, 0.15),
            tip: srgb(0.26, 0.46, 0.22),
            bark: srgb(0.36, 0.27, 0.21),
            bark_dark: srgb(0.20, 0.15, 0.12),
            snow: false,
        },
        AssetKind::Spruce => S {
            h: (15.0, 21.0),
            base: 0.05,
            rad: 0.17,
            tiers: (10, 12),
            n: 8,
            droop: 0.42,
            notch: 0.3,
            overlap: 1.6,
            offset: 0.04,
            leaf: srgb(0.10, 0.22, 0.19),
            tip: srgb(0.22, 0.36, 0.32),
            bark: srgb(0.32, 0.24, 0.19),
            bark_dark: srgb(0.18, 0.13, 0.11),
            snow: false,
        },
        _ => S {
            h: (11.0, 17.0),
            base: 0.06,
            rad: 0.2,
            tiers: (9, 10),
            n: 8,
            droop: 0.35,
            notch: 0.3,
            overlap: 1.55,
            offset: 0.04,
            leaf: srgb(0.11, 0.24, 0.19),
            tip: srgb(0.22, 0.36, 0.30),
            bark: srgb(0.32, 0.24, 0.19),
            bark_dark: srgb(0.18, 0.13, 0.11),
            snow: true,
        },
    };
    let h = mr.range(s.h.0, s.h.1);
    let n_tiers = mr.int(s.tiers.0, s.tiers.1) as usize;
    let rbase = s.rad * h * mr.range(0.9, 1.1);
    let y0 = s.base * h * mr.range(0.9, 1.1);
    let lean = V3::polar(mr.angle()) * (h * mr.range(0.0, 0.02));
    let seed = mr.seed();
    let pine = kind == AssetKind::Pine;
    let envelope = |t: f32| -> f32 {
        if pine {
            let up = smoothstep(0.0, 0.3, t);
            (0.55 + 0.45 * up) * (1.0 - t).powf(0.75) * 0.95 + 0.12
        } else {
            0.12 + 0.88 * (1.0 - t).powf(if kind == AssetKind::Fir { 0.95 } else { 1.1 })
        }
    };
    // tier layout (macro)
    let spacing = (h * 0.97 - y0) / n_tiers as f32;
    let mut tiers = Vec::new();
    for i in 0..n_tiers {
        let t = i as f32 / (n_tiers - 1) as f32;
        let r = rbase * envelope(t) * mr.range(0.88, 1.08);
        let th = spacing * s.overlap + r * 0.42;
        let rim_y = if i + 1 == n_tiers {
            h - th
        } else {
            y0 + spacing * i as f32 * 0.98
        };
        let off = V3::polar(mr.angle()) * (r * s.offset * mr.f());
        let axis = lean * (rim_y / h) + off;
        tiers.push((axis.with_y(rim_y), r, th, mr.angle(), mr.seed()));
    }
    let mut m = Mesh::new();
    // trunk
    let trunk_top = v3(lean.x, h * 0.93, lean.z);
    let r0 = h * if pine { 0.021 } else { 0.024 };
    let mut trunk = if full {
        trunk_mesh(
            &mut dr,
            v3(0.0, -0.4, 0.0),
            trunk_top,
            r0,
            r0 * 0.15,
            7,
            8,
            0.6,
            if pine { 0.25 } else { 0.08 },
        )
    } else {
        tube(
            &[
                v3(0.0, -0.4, 0.0),
                v3(lean.x * 0.5, h * 0.4, lean.z * 0.5),
                trunk_top,
            ],
            &[r0 * 1.15, r0 * 0.7, r0 * 0.15],
            4,
            Cap::Open,
            Cap::Open,
        )
    };
    let (bark, bark_dark) = (s.bark, s.bark_dark);
    paint_bark(&mut trunk, bark, bark_dark, seed, h);
    if pine {
        // Scots-pine style: orange upper bark
        let warm = srgb(0.70, 0.42, 0.24);
        trunk.shade(|_| 1.0);
        for k in 0..trunk.p.len() {
            let t = smoothstep(h * 0.25, h * 0.55, trunk.p[k].y);
            let c = trunk.c[k];
            let mixed = cmix(
                [c[0], c[1], c[2]],
                cscale(warm, 0.85 + 0.25 * (c[0] / (bark[0] + 1e-3) - 1.0)),
                t * 0.75,
            );
            trunk.c[k] = rgba(mixed, c[3]);
        }
    }
    m.append(&trunk);
    // pine: a few branches reaching the offset tiers + dead stubs on the bare trunk
    if pine && full {
        for &(axis, r, _, _, _) in &tiers {
            if axis.horiz().len() > 0.15 {
                let a = v3(lean.x * axis.y / h, axis.y - r * 0.15, lean.z * axis.y / h);
                let b = axis + V3::Y * (r * 0.05);
                let mut br = tube(
                    &[a, a.lerp(b, 0.5) + V3::Y * 0.2, b],
                    &[r0 * 0.35, r0 * 0.25, r0 * 0.12],
                    4,
                    Cap::Open,
                    Cap::Open,
                );
                paint_bark(&mut br, srgb(0.55, 0.36, 0.22), bark_dark, seed, h);
                m.append(&br);
            }
        }
        for k in 0..3 {
            let y = h * (0.22 + 0.08 * k as f32) + dr.sym(0.5);
            let d = V3::polar(dr.angle());
            let a = v3(lean.x * y / h, y, lean.z * y / h);
            let mut st = tube(
                &[a, a + d * 0.9 + V3::Y * 0.25],
                &[r0 * 0.22, 0.0],
                4,
                Cap::Open,
                Cap::Open,
            );
            paint_bark(&mut st, bark_dark, bark_dark, seed, h);
            m.append(&st);
        }
    }
    // tiers (far LOD: pairs of tiers merged into one, keeping the wider footprint)
    let groups: Vec<Vec<usize>> = if full {
        (0..n_tiers).map(|i| vec![i]).collect()
    } else {
        (0..n_tiers)
            .step_by(2)
            .map(|i| (i..(i + 2).min(n_tiers)).collect())
            .collect()
    };
    for g in &groups {
        let i = g[0];
        let (mut axis, mut r, mut th, rot, ts) = tiers[i];
        let mut trng = Rng::new(ts as u64);
        if g.len() > 1 {
            let (a2, r2, th2, _, _) = tiers[g[1]];
            let apex = (a2.y + th2).max(axis.y + th);
            if r2 > r {
                axis = v3(a2.x, axis.y, a2.z);
            }
            r = r.max(r2) * 1.03;
            th = apex - axis.y;
        }
        let droop = (s.droop * r).min((axis.y - 0.25).max(0.05) / 1.3);
        let spec = TierSpec {
            r,
            h: th,
            n: if full { s.n } else { 5 },
            droop,
            notch: s.notch,
            rot,
            lift: if pine { 0.6 } else { 0.25 },
            jitter: if pine { 0.18 } else { 0.1 },
        };
        let mut tm = tier_mesh(&spec, &mut trng, full);
        let c0 = v3(0.0, th * 0.3, 0.0);
        tm.bend_normals(0.42, |p| p - c0);
        let tilt = M3::tilt(&mut trng, if pine { 0.12 } else { 0.05 });
        tm.transform(&tilt, axis);
        let tint = trng.sym(1.0);
        let leaf = ctint(s.leaf, tint, 0.12);
        paint_tier(&mut tm, leaf, s.tip, s.snow, seed ^ i as u32, axis, r, h);
        m.append(&tm);
    }
    m.ground_ao(1.0, 0.35);
    m
}

/// Cedar: thick trunk with broad flat layered plates.
fn cedar(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let mut dr = rng.fork();
    let h = mr.range(13.0, 18.0);
    let seed = mr.seed();
    let lean = V3::polar(mr.angle()) * (h * 0.03);
    let r0 = h * 0.035;
    let layers = mr.int(4, 5) as usize;
    let leaf = srgb(0.16, 0.29, 0.21);
    let tip = srgb(0.30, 0.43, 0.30);
    let mut pads: Vec<(V3, f32, f32, f32, u32)> = Vec::new(); // center, radius, thickness, rot, seed
    for l in 0..layers {
        let t = l as f32 / (layers - 1) as f32;
        let y = lerp(h * 0.3, h * 0.84, t.powf(0.9));
        let rl = lerp(h * 0.4, h * 0.15, t);
        let count = if l + 1 == layers {
            1
        } else if l == 0 {
            3
        } else {
            mr.int(2, 3) as usize
        };
        let a0 = mr.angle();
        for k in 0..count {
            let a = a0 + k as f32 * TAU / count as f32 + mr.sym(0.5);
            let d = if count == 1 {
                0.0
            } else {
                rl * mr.range(0.38, 0.55)
            };
            let rr = if count == 1 {
                rl * 0.95
            } else {
                rl * mr.range(0.62, 0.8)
            };
            let c = lean * (y / h) + V3::polar(a) * d + V3::Y * (y + mr.sym(0.5));
            pads.push((c, rr, rr * mr.range(0.36, 0.48), mr.angle(), mr.seed()));
        }
    }
    pads.truncate(13);
    let mut m = Mesh::new();
    let top = lean.with_y(h * 0.9);
    let mut trunk = if full {
        trunk_mesh(
            &mut dr,
            v3(0.0, -0.4, 0.0),
            top,
            r0,
            r0 * 0.25,
            8,
            8,
            0.7,
            0.3,
        )
    } else {
        tube(
            &[
                v3(0.0, -0.4, 0.0),
                lean.with_y(h * 0.45) * 0.5 + V3::Y * (h * 0.225),
                top,
            ],
            &[r0 * 1.2, r0 * 0.7, r0 * 0.2],
            4,
            Cap::Open,
            Cap::Open,
        )
    };
    let bark = srgb(0.42, 0.30, 0.24);
    let bark_dark = srgb(0.22, 0.16, 0.13);
    paint_bark(&mut trunk, bark, bark_dark, seed, h);
    m.append(&trunk);
    let pad_list: Vec<&(V3, f32, f32, f32, u32)> = if full {
        pads.iter().collect()
    } else {
        // far LOD: one plate per layer (the biggest), enlarged
        let mut v: Vec<&(V3, f32, f32, f32, u32)> = Vec::new();
        let mut i = 0;
        while i < pads.len() {
            let y = pads[i].0.y;
            let group: Vec<&(V3, f32, f32, f32, u32)> = pads[i..]
                .iter()
                .take_while(|p| (p.0.y - y).abs() < 1.2)
                .collect();
            i += group.len().max(1);
            v.push(group[0]);
        }
        v
    };
    for p in pad_list.iter() {
        let &(c, r, th, rot, ps) = *p;
        let mut prng = Rng::new(ps as u64);
        let center_trunk = lean * (c.y / h);
        let (c_used, r_used) = if full {
            (c, r)
        } else {
            (
                center_trunk.with_y(c.y),
                r + (c - center_trunk).horiz().len() * 0.9,
            )
        };
        if full && (c - center_trunk.with_y(c.y)).len() > 0.3 {
            let a = center_trunk.with_y(c.y - th * 1.5);
            let mut br = tube(
                &[a, a.lerp(c, 0.5) + V3::Y * 0.1, c],
                &[r0 * 0.4, r0 * 0.28, r0 * 0.15],
                5,
                Cap::Open,
                Cap::Open,
            );
            paint_bark(&mut br, bark, bark_dark, seed, h);
            m.append(&br);
        }
        let spec = TierSpec {
            r: r_used,
            h: th,
            n: if full { 8 } else { 5 },
            droop: th * 0.3,
            notch: 0.2,
            rot,
            lift: 1.0,
            jitter: 0.14,
        };
        let mut tm = tier_mesh(&spec, &mut prng, full);
        let c0 = v3(0.0, -th * 0.5, 0.0);
        tm.bend_normals(0.35, |q| q - c0);
        tm.transform(&M3::tilt(&mut prng, 0.06), c_used);
        paint_tier(
            &mut tm,
            ctint(leaf, prng.sym(1.0), 0.1),
            tip,
            false,
            seed ^ ps,
            c_used,
            r_used,
            h,
        );
        m.append(&tm);
    }
    m.ground_ao(1.0, 0.35);
    m
}

fn broadleaf(kind: AssetKind, rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let mut dr = rng.fork();
    struct S {
        h: (f32, f32),
        fork: f32,
        trunk_r: f32,
        cr: f32,
        cry: f32,
        clumps: (i32, i32),
        pal: [C3; 3],
        tip: C3,
        bark: C3,
        bark_dark: C3,
    }
    let birch = kind == AssetKind::Birch;
    let s = match kind {
        AssetKind::Oak => S {
            h: (10.0, 14.0),
            fork: 0.3,
            trunk_r: 0.045,
            cr: 0.44,
            cry: 0.3,
            clumps: (10, 11),
            pal: [
                srgb(0.20, 0.35, 0.12),
                srgb(0.25, 0.40, 0.14),
                srgb(0.18, 0.31, 0.12),
            ],
            tip: srgb(0.42, 0.55, 0.20),
            bark: srgb(0.38, 0.30, 0.23),
            bark_dark: srgb(0.20, 0.16, 0.13),
        },
        AssetKind::Maple => S {
            h: (10.0, 13.0),
            fork: 0.32,
            trunk_r: 0.035,
            cr: 0.38,
            cry: 0.34,
            clumps: (10, 11),
            pal: [
                srgb(0.74, 0.18, 0.06),
                srgb(0.86, 0.40, 0.09),
                srgb(0.62, 0.12, 0.07),
            ],
            tip: srgb(0.96, 0.64, 0.16),
            bark: srgb(0.36, 0.31, 0.27),
            bark_dark: srgb(0.19, 0.16, 0.14),
        },
        _ => S {
            h: (12.0, 17.0),
            fork: 0.3,
            trunk_r: 0.017,
            cr: 0.23,
            cry: 0.36,
            clumps: (14, 15),
            pal: [
                srgb(0.36, 0.50, 0.17),
                srgb(0.43, 0.55, 0.19),
                srgb(0.31, 0.45, 0.15),
            ],
            tip: srgb(0.58, 0.66, 0.27),
            bark: srgb(0.88, 0.86, 0.80),
            bark_dark: srgb(0.70, 0.68, 0.64),
        },
    };
    let h = mr.range(s.h.0, s.h.1);
    let seed = mr.seed();
    let lean = V3::polar(mr.angle()) * (h * mr.range(0.01, 0.05));
    let fork_y = h * s.fork * mr.range(0.9, 1.1);
    let tr = h * s.trunk_r;
    let cr = h * s.cr * mr.range(0.9, 1.08);
    let cry = h * s.cry;
    let cc = lean + V3::Y * (h - cry * 1.05);
    let n_clumps = mr.int(s.clumps.0, s.clumps.1) as usize;
    // clump layout (macro): a crown clump plus clumps scattered over an ellipsoidal shell
    let mut clumps: Vec<(V3, f32)> = vec![(
        cc + V3::Y * (cry * 0.4),
        cr * if birch { 0.42 } else { 0.5 },
    )];
    let a0 = mr.angle();
    let ring = n_clumps - 1;
    for i in 0..ring {
        let t = (i as f32 + 0.5) / ring as f32;
        let yf = lerp(-0.72, 0.62, t) + mr.sym(0.08);
        let a = a0 + i as f32 * 2.39996 + mr.sym(0.3);
        let d = cr
            * (1.0 - yf * yf).max(0.05).sqrt()
            * if birch {
                mr.range(0.32, 0.55)
            } else {
                mr.range(0.5, 0.72)
            };
        let r = cr
            * if birch {
                mr.range(0.42, 0.52)
            } else {
                mr.range(0.34, 0.46)
            };
        clumps.push((
            cc.horiz() + V3::polar(a) * d + V3::Y * (cc.y + yf * cry * 0.85),
            r,
        ));
    }
    let canopy = Canopy {
        clumps: clumps.clone(),
        center: cc,
        radii: v3(cr, cry, cr),
    };
    let mut m = Mesh::new();
    // trunk + branches
    let trunk_top = cc + V3::Y * (cry * 0.2);
    let mut wood = Mesh::new();
    if full {
        let rings = if birch { 16 } else { 8 };
        wood.append(&trunk_mesh(
            &mut dr,
            v3(0.0, -0.4, 0.0),
            trunk_top,
            tr,
            tr * 0.3,
            8,
            rings,
            if birch { 0.35 } else { 0.7 },
            h * 0.02,
        ));
        for (i, &(c, r)) in clumps.iter().enumerate().skip(1) {
            if c.y > cc.y + cry * 0.15 || i > 8 {
                continue;
            }
            let start =
                lean * (fork_y / h) + V3::Y * (fork_y + dr.range(-0.3, 1.2) * (i % 3) as f32 * 0.5);
            let end = c + (start - c).norm() * (r * 0.35);
            let ctrl = start.lerp(end, 0.5) + V3::Y * (r * 0.3);
            let path = bezier(start, ctrl, end, 3);
            wood.append(&tube(
                &path,
                &[tr * 0.5, tr * 0.36, tr * 0.24, tr * 0.12],
                5,
                Cap::Open,
                Cap::Open,
            ));
        }
    } else {
        wood.append(&tube(
            &[
                v3(0.0, -0.4, 0.0),
                lean * 0.5 + V3::Y * (fork_y * 0.6),
                trunk_top,
            ],
            &[tr * 1.1, tr * 0.8, tr * 0.3],
            5,
            Cap::Open,
            Cap::Open,
        ));
    }
    if birch {
        let dark = srgb(0.12, 0.11, 0.10);
        let white = s.bark;
        let grey = s.bark_dark;
        wood.paint(|p, _, _| {
            let band = noise3(v3(p.x * 3.0, p.y * 4.5, p.z * 3.0), seed ^ 77);
            let streak = noise3(v3(p.x * 6.0, p.y * 0.8, p.z * 6.0), seed ^ 78);
            let mut c = cmix(white, grey, 0.5 + 0.5 * streak);
            let mark = smoothstep(0.22, 0.36, band);
            c = cmix(c, dark, mark * 0.9);
            let base = 1.0 - smoothstep(0.2, 1.6, p.y);
            c = cmix(c, cscale(dark, 1.6), base * 0.85);
            rgba(c, 0.22 * clamp01(p.y / h).powi(2))
        });
    } else {
        paint_bark(&mut wood, s.bark, s.bark_dark, seed, h);
    }
    m.append(&wood);
    // canopy
    if full {
        for (i, &(c, r)) in clumps.iter().enumerate() {
            let level = if i == 0 { 2 } else { 1 };
            let radii = v3(
                r * mr.range(0.95, 1.1),
                r * mr.range(0.78, 0.92),
                r * mr.range(0.95, 1.1),
            );
            let mut b = blob(level, radii, 0.24, 1.9, seed ^ (i as u32 * 31), 0.75);
            b.translate(c);
            let cc2 = cc;
            b.bend_normals(0.55, |p| (p - c).norm() * 0.45 + (p - cc2).norm() * 0.55);
            let col = s.pal[i % 3];
            canopy.paint(&mut b, i, col, s.tip, seed ^ i as u32, lean, cr * 1.2);
            m.append(&b);
        }
    } else {
        // one big blob for the canopy body (fitted to the clumps) + up to three outer lumps
        let mut lo = v3(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = v3(f32::MIN, f32::MIN, f32::MIN);
        for &(c, r) in &clumps {
            lo = v3(
                lo.x.min(c.x - r),
                lo.y.min(c.y - r * 0.85),
                lo.z.min(c.z - r),
            );
            hi = v3(
                hi.x.max(c.x + r),
                hi.y.max(c.y + r * 0.85),
                hi.z.max(c.z + r),
            );
        }
        let bc = (lo + hi) * 0.5;
        let br = (hi - lo) * 0.5;
        let mut b = blob(
            1,
            v3(br.x * 0.85, br.y * 0.86, br.z * 0.85),
            0.12,
            1.4,
            seed,
            0.85,
        );
        b.translate(bc);
        b.bend_normals(0.5, |p| p - cc);
        let avg = cmix(cmix(s.pal[0], s.pal[1], 0.5), s.pal[2], 0.33);
        let lod_canopy = Canopy {
            clumps: vec![(cc, cr)],
            center: cc,
            radii: v3(cr, cry, cr),
        };
        lod_canopy.paint(&mut b, 0, avg, s.tip, seed, lean, cr * 1.2);
        m.append(&b);
        let mut outer: Vec<(usize, f32)> = clumps
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, (c, r))| (i, (*c - cc).len() + r))
            .collect();
        outer.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        for &(i, _) in outer.iter().take(3) {
            let (c, r) = clumps[i];
            let mut lb = blob(0, v3(r, r * 0.85, r), 0.1, 1.4, seed ^ i as u32, 0.8);
            let push = (c - cc).norm() * (r * 0.15);
            lb.translate(c + push);
            lb.bend_normals(0.5, |p| p - cc);
            lod_canopy.paint(
                &mut lb,
                1,
                s.pal[i % 3],
                s.tip,
                seed ^ i as u32,
                lean,
                cr * 1.2,
            );
            m.append(&lb);
        }
    }
    m.ground_ao(1.0, 0.35);
    m
}

fn willow(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let mut dr = rng.fork();
    let h = mr.range(9.0, 12.5);
    let seed = mr.seed();
    let lean = V3::polar(mr.angle()) * (h * 0.04);
    let tr = h * 0.05;
    let cr = h * mr.range(0.42, 0.48);
    let cry = h * 0.2;
    let cc = lean + V3::Y * (h - cry * 1.1);
    let leaf = srgb(0.33, 0.47, 0.16);
    let leaf2 = srgb(0.42, 0.54, 0.19);
    let tip = srgb(0.58, 0.66, 0.29);
    let mut clumps: Vec<(V3, f32)> = vec![(cc + V3::Y * (cry * 0.3), cr * 0.55)];
    let a0 = mr.angle();
    for i in 0..5 {
        let a = a0 + i as f32 * TAU / 5.0 + mr.sym(0.3);
        clumps.push((
            cc.horiz()
                + V3::polar(a) * (cr * mr.range(0.45, 0.6))
                + V3::Y * (cc.y + mr.sym(cry * 0.2)),
            cr * mr.range(0.4, 0.5),
        ));
    }
    // frond anchors (macro, so LOD1 skirt matches)
    let n_fronds = 34;
    let mut fronds = Vec::new();
    for i in 0..n_fronds {
        let a = i as f32 * TAU / n_fronds as f32 + mr.sym(0.12);
        let d = cr * mr.range(0.55, 1.0);
        let top = cc.horiz() + V3::polar(a) * d + V3::Y * (cc.y - cry * mr.range(0.0, 0.45));
        let len = (top.y - mr.range(0.6, 2.6)).max(1.0);
        fronds.push((a, top, len, mr.range(0.28, 0.42), mr.seed()));
    }
    let canopy = Canopy {
        clumps: clumps.clone(),
        center: cc,
        radii: v3(cr, cry, cr),
    };
    let mut m = Mesh::new();
    // trunk & limbs
    let mut wood = Mesh::new();
    let fork = lean * 0.4 + V3::Y * (h * 0.32);
    if full {
        wood.append(&trunk_mesh(
            &mut dr,
            v3(0.0, -0.4, 0.0),
            fork + V3::Y * 0.5,
            tr,
            tr * 0.75,
            8,
            5,
            0.6,
            0.3,
        ));
        for i in 0..4 {
            let a = a0 + i as f32 * TAU / 4.0 + 0.4;
            let end = cc.horiz() + V3::polar(a) * (cr * 0.45) + V3::Y * (cc.y + cry * 0.1);
            let path = bezier(fork, fork.lerp(end, 0.5) + V3::polar(a) * 0.5, end, 4);
            wood.append(&tube(
                &path,
                &[tr * 0.6, tr * 0.5, tr * 0.38, tr * 0.25, tr * 0.12],
                6,
                Cap::Open,
                Cap::Open,
            ));
        }
    } else {
        wood.append(&tube(
            &[v3(0.0, -0.4, 0.0), fork, cc],
            &[tr * 1.1, tr * 0.8, tr * 0.3],
            5,
            Cap::Open,
            Cap::Open,
        ));
    }
    paint_bark(
        &mut wood,
        srgb(0.38, 0.32, 0.25),
        srgb(0.20, 0.17, 0.14),
        seed,
        h,
    );
    m.append(&wood);
    if full {
        for (i, &(c, r)) in clumps.iter().enumerate() {
            let mut b = blob(
                if i == 0 { 2 } else { 1 },
                v3(r * 1.1, r * 0.7, r * 1.1),
                0.2,
                1.6,
                seed ^ (i as u32 * 7),
                0.7,
            );
            b.translate(c);
            let cc2 = cc;
            b.bend_normals(0.55, |p| (p - c).norm() * 0.4 + (p - cc2).norm() * 0.6);
            canopy.paint(
                &mut b,
                i,
                if i % 2 == 0 { leaf } else { leaf2 },
                tip,
                seed ^ i as u32,
                lean,
                cr,
            );
            m.append(&b);
        }
        for &(a, top, len, w, fs) in &fronds {
            let mut fr = Rng::new(fs as u64);
            let out = V3::polar(a);
            let segs = 5;
            let mut pts = Vec::new();
            let mut ws = Vec::new();
            let sway = V3::polar(a + fr.sym(0.6)) * fr.range(0.2, 0.6);
            for k in 0..=segs {
                let t = k as f32 / segs as f32;
                let p = top + out * (0.5 * (t * 3.0).min(1.0) + 0.2 * t) + sway * (t * t)
                    - V3::Y * (len * t);
                pts.push(p);
                ws.push(if k == segs { 0.0 } else { w * (1.0 - 0.6 * t) });
            }
            let side = (-V3::Y).cross(out).norm();
            let mut f = leaf_strip(&pts, &ws, side, 0.0, false);
            f.double_sided(1.0);
            let cc2 = cc;
            f.bend_normals(0.5, |p| (p - cc2).horiz() + V3::Y * 0.3);
            let top_y = top.y;
            f.paint(|p, _, uv| {
                let t = uv[1];
                let nz = noise3(p * 0.6, seed ^ 41);
                let mut c = cmix(leaf2, tip, t * 0.8 + 0.2 * nz);
                c = ctint(c, nz, 0.15);
                c = cscale(c, 0.65 + 0.4 * t * (p.y / top_y).min(1.0) + 0.1 * nz);
                rgba(c, 0.35 + 0.65 * t)
            });
            m.append(&f);
        }
    } else {
        let mut b = blob(
            1,
            v3(cr * 1.05, cry * 1.05, cr * 1.05),
            0.12,
            1.4,
            seed,
            0.6,
        );
        b.translate(cc);
        b.bend_normals(0.5, |p| p - cc);
        let lc = Canopy {
            clumps: vec![(cc, cr)],
            center: cc,
            radii: v3(cr, cry, cr),
        };
        lc.paint(&mut b, 0, leaf, tip, seed, lean, cr);
        m.append(&b);
        // hanging skirt: jagged truncated cone, double sided
        let n = 16;
        let mut rings_top = Vec::new();
        let mut rings_bot = Vec::new();
        for k in 0..n {
            let a = k as f32 * TAU / n as f32;
            rings_top.push(cc.horiz() + V3::polar(a) * (cr * 0.85) + V3::Y * (cc.y - cry * 0.2));
            let drop = if k % 2 == 0 {
                0.8 + 0.6 * hashf(k as u32, seed)
            } else {
                3.4 + 0.8 * hashf(k as u32, seed)
            };
            let rr = if k % 2 == 0 { 0.98 } else { 0.86 };
            rings_bot.push(cc.horiz() + V3::polar(a + 0.1) * (cr * rr) + V3::Y * drop);
        }
        let mut sk = rings_mesh(&[rings_bot, rings_top]);
        sk.double_sided(0.55);
        sk.bend_normals(0.5, |p| (p - cc).horiz() + V3::Y * 0.3);
        sk.paint(|p, n, uv| {
            let t = 1.0 - uv[1];
            let c = cmix(leaf, leaf2, 0.4 + 0.4 * t);
            let back = if n.dot((p - cc).horiz()) < 0.0 {
                0.55
            } else {
                1.0
            };
            rgba(
                cscale(
                    c,
                    (0.62 + 0.3 * t) * back * (1.0 + 0.1 * noise3(p * 0.7, seed)),
                ),
                0.4 + 0.6 * t,
            )
        });
        m.append(&sk);
    }
    m.ground_ao(1.0, 0.35);
    m
}

fn palm(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let h = mr.range(8.0, 12.0);
    let seed = mr.seed();
    let bend_dir = V3::polar(mr.angle());
    let bend = h * mr.range(0.1, 0.2);
    let segs = if full { 11 } else { 3 };
    let r_base = mr.range(0.24, 0.3);
    let path_at = |t: f32| bend_dir * (bend * t * t) + V3::Y * (lerp(-0.4, h, t));
    let mut m = Mesh::new();
    // trunk with segment rings (sawtooth radius)
    let mut path = Vec::new();
    let mut radii = Vec::new();
    let mut band = Vec::new();
    for s in 0..segs {
        let t0 = s as f32 / segs as f32;
        let t1 = (s as f32 + 0.8) / segs as f32;
        let rr = |t: f32| lerp(r_base, r_base * 0.68, t) * (1.0 + 0.6 * (1.0 - t * 6.0).max(0.0));
        if full {
            path.push(path_at(t0));
            radii.push(rr(t0) * 0.93);
            band.push(1.0);
            path.push(path_at(t1));
            radii.push(rr(t1) * 1.06);
            band.push(0.0);
        } else {
            path.push(path_at(t0));
            radii.push(rr(t0));
            band.push(0.5);
        }
    }
    let top = path_at(1.0);
    path.push(top);
    radii.push(r_base * 0.62);
    band.push(1.0);
    let sides = if full { 7 } else { 5 };
    let mut trunk = tube(&path, &radii, sides, Cap::Open, Cap::Flat);
    {
        let light = srgb(0.62, 0.52, 0.38);
        let dark = srgb(0.34, 0.27, 0.20);
        // ring index -> band intensity via uv.y matching
        let ys: Vec<f32> = path.iter().map(|p| p.y).collect();
        trunk.paint(|p, _, _| {
            // nearest ring
            let mut best = 0;
            for (k, y) in ys.iter().enumerate() {
                if (y - p.y).abs() < (ys[best] - p.y).abs() {
                    best = k;
                }
            }
            let nz = noise3(p * 1.5, seed);
            let c = cmix(light, dark, band[best] * 0.7 + 0.15 * nz);
            rgba(cscale(c, 1.0 + 0.08 * nz), 0.2 * clamp01(p.y / h).powi(2))
        });
    }
    m.append(&trunk);
    // crown bulb
    let mut bulb = blob(
        if full { 1 } else { 0 },
        v3(0.42, 0.55, 0.42),
        0.1,
        2.0,
        seed,
        1.0,
    );
    bulb.translate(top + V3::Y * 0.1);
    let olive = srgb(0.36, 0.38, 0.17);
    bulb.paint(|p, n, _| {
        rgba(
            cscale(olive, 0.75 + 0.25 * n.y + 0.1 * noise3(p * 3.0, seed)),
            0.3,
        )
    });
    m.append(&bulb);
    let nuts = mr.int(3, 4);
    let nut_a = mr.sym(0.3);
    if full {
        for k in 0..nuts {
            let a = k as f32 * 2.1 + nut_a;
            let mut nut = icosphere(1);
            nut.scale3(v3(0.14, 0.16, 0.14));
            nut.translate(top + V3::polar(a) * 0.32 - V3::Y * (0.15 + 0.08 * k as f32));
            let nc = srgb(0.42, 0.36, 0.14);
            nut.paint(|p, n, _| {
                rgba(
                    cscale(nc, 0.7 + 0.3 * n.y + 0.1 * noise3(p * 9.0, seed)),
                    0.25,
                )
            });
            m.append(&nut);
        }
    }
    // fronds
    let n_fr = mr.int(7, 9) as usize;
    let a0 = mr.angle();
    let leaf = srgb(0.26, 0.45, 0.14);
    let leaf_tip = srgb(0.52, 0.60, 0.22);
    let rachis = srgb(0.55, 0.58, 0.30);
    for i in 0..n_fr {
        let a = a0 + i as f32 * TAU / n_fr as f32 + mr.sym(0.25);
        let elev = mr.range(0.15, 0.85);
        let len = mr.range(3.6, 5.0);
        let droop = mr.range(0.55, 0.9);
        let wmax = mr.range(0.5, 0.7);
        let dir = V3::polar(a);
        let n = if full { 16 } else { 3 };
        let mut pts = Vec::new();
        let mut ws = Vec::new();
        let mut sides_v = Vec::new();
        let mut folds = Vec::new();
        let side = V3::Y.cross(dir).norm();
        for k in 0..=n {
            let s = k as f32 / n as f32;
            let p = top
                + V3::Y * 0.25
                + dir * (len * s * elev.cos())
                + V3::Y * (len * s * elev.sin() - droop * len * s * s);
            pts.push(p);
            let env = (PI * (s * 0.92 + 0.04)).sin().max(0.0).powf(0.6);
            let serr = if full && k % 2 == 1 { 0.62 } else { 1.0 };
            let w = if k == 0 || k == n {
                0.0
            } else {
                wmax * env * serr
            };
            ws.push(w);
            sides_v.push(side);
        }
        for k in 0..=n {
            let along = (pts[(k + 1).min(n)] - pts[k.saturating_sub(1)]).norm();
            let up = side.cross(along).norm();
            folds.push(-up * (ws[k] * 0.45));
        }
        let mut f = ribbon(&pts, &sides_v, &ws, &folds, true);
        f.double_sided(0.85);
        let center = top;
        f.bend_normals(0.35, |p| (p - center) + V3::Y * 1.5);
        f.paint(|p, _, uv| {
            let s = uv[1];
            let mid = 1.0 - (uv[0] - 0.5).abs() * 2.0;
            let nz = noise3(p * 0.8, seed ^ 13);
            let mut c = cmix(leaf, leaf_tip, s * 0.7 + 0.15 * nz);
            c = cmix(c, rachis, mid * 0.35);
            c = cscale(c, 0.7 + 0.35 * s.min(0.6) + 0.08 * nz);
            rgba(c, 0.2 + 0.8 * s)
        });
        m.append(&f);
    }
    m.ground_ao(0.8, 0.3);
    m
}

fn bamboo(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let n = mr.int(7, 8) as usize;
    let seed = mr.seed();
    let green = srgb(0.42, 0.55, 0.19);
    let old = srgb(0.62, 0.60, 0.28);
    let node_c = srgb(0.26, 0.33, 0.13);
    let leaf = srgb(0.27, 0.45, 0.14);
    let leaf_tip = srgb(0.46, 0.58, 0.20);
    let mut m = Mesh::new();
    for c in 0..n {
        let a = c as f32 * 2.39996 + mr.sym(0.3);
        let d = if c == 0 { 0.05 } else { mr.range(0.25, 0.8) };
        let base = V3::polar(a) * d;
        let hh = mr.range(8.5, 12.0) * (1.0 - 0.2 * d);
        let r = mr.range(0.055, 0.08);
        let lean = V3::polar(a + mr.sym(0.35)) * (hh * mr.range(0.1, 0.2));
        let cs = mr.seed();
        let age = mr.f();
        let mut cr = Rng::new(cs as u64);
        let p_at = |t: f32| base + lean * (t * t) + V3::Y * lerp(-0.3, hh, t);
        let col = cmix(green, old, age * 0.6);
        // culm
        let k = if full { 5 } else { 2 };
        let path: Vec<V3> = (0..=k).map(|i| p_at(i as f32 / k as f32)).collect();
        let radii: Vec<f32> = (0..=k)
            .map(|i| {
                if i == k {
                    0.0
                } else {
                    r * (1.0 - 0.45 * i as f32 / k as f32)
                }
            })
            .collect();
        let mut culm = tube(
            &path,
            &radii,
            if full { 5 } else { 3 },
            Cap::Open,
            Cap::Open,
        );
        culm.paint(|p, _, _| {
            let nz = noise3(p * 2.0, seed ^ cs);
            rgba(
                cscale(col, 0.9 + 0.12 * nz),
                0.35 * clamp01(p.y / hh).powi(2),
            )
        });
        m.append(&culm);
        if full {
            // node collars
            let spacing = cr.range(1.05, 1.3);
            let mut y = spacing * 0.6;
            while y < hh * 0.9 {
                let t = (y + 0.3) / (hh + 0.3);
                let c0 = p_at(t - 0.005);
                let c1 = p_at(t + 0.005);
                let rr = r * (1.0 - 0.45 * t) * 1.14;
                let mut col_m = tube(&[c0, c1], &[rr, rr], 5, Cap::Open, Cap::Open);
                col_m.paint(|_, _, _| rgba(node_c, 0.35 * t * t));
                m.append(&col_m);
                y += spacing;
            }
            // fan shaped leaf sprays in the upper half, pointing away from the clump
            let sprays = 5;
            for sidx in 0..sprays {
                let t = 0.5 + 0.46 * (sidx as f32 + cr.f() * 0.6) / sprays as f32;
                let at = p_at(t);
                let out = (at.horiz() + V3::polar(cr.angle()) * 0.5).norm();
                for l in 0..5 {
                    let spread = (l as f32 - 2.0) * 0.5 + cr.sym(0.15);
                    let dir = (M3::rot_y(spread).apply(out) + V3::Y * cr.range(-0.35, 0.15)).norm();
                    let len = cr.range(0.85, 1.25);
                    let tip = at + dir * len - V3::Y * (len * cr.range(0.25, 0.45));
                    let mid = at.lerp(tip, 0.45) + V3::Y * (len * 0.1);
                    let side = dir.cross(V3::Y).norm();
                    let mut lf = leaf_strip(
                        &[at, mid, tip],
                        &[0.0, cr.range(0.075, 0.1), 0.0],
                        side,
                        0.0,
                        false,
                    );
                    lf.double_sided(0.85);
                    lf.bend_normals(0.45, |_| V3::Y);
                    let ls = seed ^ (l as u32 + sidx * 7);
                    lf.paint(|p, _, uv| {
                        let nz = noise3(p * 1.3, ls);
                        let c = cmix(leaf, leaf_tip, uv[1] * 0.6 + 0.25 * nz);
                        rgba(cscale(c, 0.85 + 0.15 * nz + 0.15 * t), 0.5 + 0.5 * uv[1])
                    });
                    m.append(&lf);
                }
            }
        } else if c < 6 {
            // far: one elongated leaf mass per culm
            let mut b = blob(0, v3(0.85, 1.7, 0.85), 0.12, 1.5, seed ^ cs, 0.9);
            b.translate(p_at(0.74) + lean.norm() * 0.35);
            b.bend_normals(0.4, |_| V3::Y);
            b.paint(|p, n, _| {
                let c = cmix(leaf, leaf_tip, 0.15 + 0.25 * n.y.max(0.0));
                rgba(cscale(c, 0.75 + 0.1 * noise3(p, seed)), 0.7)
            });
            m.append(&b);
        }
    }
    m.ground_ao(0.6, 0.35);
    m
}

fn dead_tree(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let h = mr.range(8.0, 12.0);
    let seed = mr.seed();
    let r0 = h * 0.04;
    let lean = V3::polar(mr.angle()) * (h * 0.06);
    let wob = mr.seed();
    let trunk_at = |t: f32| -> V3 {
        let w = h * 0.05 * (t * 3.0).min(1.0);
        lean * t
            + v3(
                noise3(v3(t * 2.0, 0.0, 0.0), wob) * w,
                lerp(-0.4, h * 0.85, t),
                noise3(v3(t * 2.0, 3.0, 0.0), wob) * w,
            )
    };
    let rk = if full { 9 } else { 3 };
    let path: Vec<V3> = (0..=rk).map(|i| trunk_at(i as f32 / rk as f32)).collect();
    let radii: Vec<f32> = (0..=rk)
        .map(|i| {
            let t = i as f32 / rk as f32;
            if i == rk {
                0.0
            } else {
                r0 * (1.0 - 0.85 * t) * (1.0 + 0.6 * (1.0 - t * 8.0).max(0.0))
            }
        })
        .collect();
    let mut wood = tube(
        &path,
        &radii,
        if full { 8 } else { 5 },
        Cap::Open,
        Cap::Open,
    );
    // branches (macro): recursive two levels
    let nb = mr.int(4, 6) as usize;
    let mut specs = Vec::new();
    for i in 0..nb {
        let t = mr.range(0.35, 0.8);
        let a = i as f32 * TAU / nb as f32 + mr.sym(0.6);
        let elev = mr.range(0.2, 0.9);
        let len = h * mr.range(0.25, 0.42) * (1.1 - t * 0.5);
        specs.push((t, a, elev, len, mr.seed()));
    }
    for &(t, a, elev, len, bs) in &specs {
        let mut br = Rng::new(bs as u64);
        let start = trunk_at(t);
        let dir = (V3::polar(a) * elev.cos() + V3::Y * elev.sin()).norm();
        let k = if full { 4 } else { 1 };
        let mut pts = vec![start];
        let mut p = start;
        let mut d = dir;
        for _ in 0..k {
            d = (d + br.unit() * 0.35 + V3::Y * 0.1).norm();
            p += d * (len / k as f32);
            pts.push(p);
        }
        let rb = r0 * (1.0 - 0.85 * t) * 0.6;
        let rr: Vec<f32> = (0..=k)
            .map(|i| {
                if i == k {
                    0.0
                } else {
                    rb * (1.0 - i as f32 / (k as f32 + 0.5))
                }
            })
            .collect();
        wood.append(&tube(
            &pts,
            &rr,
            if full { 5 } else { 3 },
            Cap::Open,
            Cap::Open,
        ));
        // sub branches
        let subs = if full { br.int(1, 3) } else { 0 };
        for _ in 0..subs {
            let j = br.int(1, (k - 1).max(1) as i32) as usize;
            let s0 = pts[j];
            let sd = (d + br.unit() * 0.9 + V3::Y * 0.4).norm();
            let sl = len * br.range(0.3, 0.5);
            let s1 = s0 + sd * (sl * 0.5) + br.unit() * (sl * 0.1);
            let s2 = s1 + (sd + br.unit() * 0.4).norm() * (sl * 0.5);
            wood.append(&tube(
                &[s0, s1, s2],
                &[rb * 0.45, rb * 0.25, 0.0],
                4,
                Cap::Open,
                Cap::Open,
            ));
        }
    }
    let base = srgb(0.46, 0.42, 0.37);
    let dark = srgb(0.26, 0.23, 0.20);
    let pale = srgb(0.66, 0.63, 0.58);
    wood.paint(|p, n, _| {
        let s = noise3(v3(p.x * 3.0, p.y * 0.5, p.z * 3.0), seed);
        let mut c = cmix(base, dark, 0.3 + 0.4 * s);
        let up = smoothstep(h * 0.3, h * 0.9, p.y);
        c = cmix(c, pale, up * 0.6 + 0.2 * n.y.max(0.0));
        c = cscale(c, 0.7 + 0.3 * smoothstep(0.0, 1.5, p.y));
        let reach = clamp01((p - lean * clamp01(p.y / h)).horiz().len() / (h * 0.2));
        rgba(c, 0.12 * clamp01(p.y / h).powi(2) + 0.6 * reach * reach)
    });
    let mut m = wood;
    m.ground_ao(1.0, 0.3);
    m
}

// =====================================================================================
// Rocks
// =====================================================================================

fn rock_palette(rng: &mut Rng) -> (C3, C3) {
    let pals = [
        (srgb(0.52, 0.50, 0.47), srgb(0.36, 0.34, 0.32)),
        (srgb(0.55, 0.49, 0.42), srgb(0.38, 0.33, 0.28)),
        (srgb(0.47, 0.49, 0.51), srgb(0.32, 0.33, 0.35)),
        (srgb(0.58, 0.52, 0.44), srgb(0.42, 0.36, 0.30)),
    ];
    rng.pick(&pals)
}

/// Rock paint: strata bands, crevice darkening (uv.x from `rock_mesh`), lighter weathered
/// tops, optional moss on upward faces.
fn paint_rock(
    m: &mut Mesh,
    base: C3,
    dark: C3,
    seed: u32,
    center: V3,
    scale: f32,
    moss: f32,
    strata: f32,
) {
    let moss_c = srgb(0.21, 0.30, 0.10);
    let moss_c2 = srgb(0.31, 0.38, 0.13);
    let dust = srgb(0.64, 0.61, 0.55);
    m.paint(|p, n, uv| {
        let q = (p - center) / scale;
        let nz = fbm(q * 1.6, 3, seed);
        let band = (p.y * 5.0 / scale.max(0.5) + 2.0 * noise3(q * 0.9, seed ^ 3)).sin();
        let mut c = cmix(base, dark, 0.4 + 0.35 * nz + strata * 0.3 * band);
        c = ctint(c, noise3(q * 0.7, seed ^ 8), 0.12);
        let crev = smoothstep(0.78, 1.04, uv[0]);
        c = cscale(c, 0.68 + 0.32 * crev);
        c = cmix(c, dust, clamp01(n.y) * 0.22);
        if moss > 0.0 {
            let patch = fbm(q * 2.3, 3, seed ^ 21);
            let mm = smoothstep(
                0.42,
                0.62,
                n.y * 0.8 + 0.45 * patch + (moss - 1.0) * 0.3 + 0.1,
            );
            let mc = cmix(moss_c, moss_c2, 0.5 + 0.6 * noise3(q * 3.0, seed ^ 4));
            c = cmix(c, mc, mm);
        }
        rgba(c, 0.0)
    });
}

/// A rock resting on the ground: generated, rotated and sunk so min y = -sink.
fn placed_rock(
    level: u32,
    radii: V3,
    rough: f32,
    cuts: usize,
    smooth: f32,
    seed: u32,
    pos: V3,
    yaw: f32,
    tilt: M3,
    sink: f32,
    shard: bool,
) -> Mesh {
    let mut r = rock_mesh(level, radii, rough, cuts, smooth, seed, shard);
    r.transform(&M3::rot_y(yaw).then(&tilt), V3::ZERO);
    let (mn, _) = r.bounds();
    r.translate(v3(pos.x, pos.y - mn.y - sink, pos.z));
    r
}

fn boulder(mossy: bool, rng: &mut Rng, lod: u32) -> Mesh {
    let mut mr = rng.fork();
    let (base, dark) = rock_palette(&mut mr);
    let size = mr.range(1.0, 1.45);
    let radii = v3(
        size * mr.range(1.0, 1.2),
        size * mr.range(0.62, 0.8),
        size * mr.range(0.85, 1.05),
    );
    let seed = mr.seed();
    let yaw = mr.angle();
    let level = if lod == 0 { 2 } else { 1 };
    let cuts = if mossy { 5 } else { 6 };
    let mut m = placed_rock(
        level,
        radii,
        if mossy { 0.16 } else { 0.18 },
        cuts,
        if mossy { 0.2 } else { 0.12 },
        seed,
        V3::ZERO,
        yaw,
        M3::I,
        0.3,
        false,
    );
    let c = v3(0.0, radii.y * 0.5, 0.0);
    paint_rock(
        &mut m,
        base,
        dark,
        seed,
        c,
        size,
        if mossy { 1.0 } else { 0.0 },
        0.6,
    );
    m.ground_ao(0.5, 0.35);
    m
}

fn rock_cluster(rng: &mut Rng, lod: u32) -> Mesh {
    let mut mr = rng.fork();
    let (base, dark) = rock_palette(&mut mr);
    let n = mr.int(4, 5) as usize;
    let mut m = Mesh::new();
    let a0 = mr.angle();
    let mut rocks = Vec::new();
    for i in 0..n {
        let big = i == 0;
        let s = if big {
            mr.range(0.9, 1.15)
        } else if i == 1 {
            mr.range(0.55, 0.75)
        } else {
            mr.range(0.25, 0.5)
        };
        let a = a0 + i as f32 * TAU / (n - 1).max(1) as f32 + mr.sym(0.4);
        let d = if big {
            0.0
        } else {
            mr.range(0.9, 1.35) + s * 0.6
        };
        let radii = v3(
            s * mr.range(0.95, 1.25),
            s * mr.range(0.55, 0.8),
            s * mr.range(0.8, 1.05),
        );
        rocks.push((
            V3::polar(a) * d,
            radii,
            mr.seed(),
            mr.angle(),
            M3::tilt(&mut mr, 0.3),
            s,
        ));
    }
    for (i, (pos, radii, seed, yaw, tilt, s)) in rocks.into_iter().enumerate() {
        let level = match (lod, i) {
            (0, 0) => 2,
            (0, _) => 1,
            _ => 0,
        };
        let mut r = placed_rock(
            level,
            radii,
            0.18,
            7,
            0.1,
            seed,
            pos,
            yaw,
            tilt,
            0.12 + 0.12 * s,
            false,
        );
        paint_rock(
            &mut r,
            base,
            dark,
            seed,
            pos + V3::Y * (radii.y * 0.5),
            s,
            0.0,
            0.5,
        );
        m.append(&r);
    }
    m.ground_ao(0.4, 0.35);
    m
}

fn cliff_rock(rng: &mut Rng, lod: u32) -> Mesh {
    let mut mr = rng.fork();
    let (base, dark) = rock_palette(&mut mr);
    let height = mr.range(6.5, 9.5);
    let n = mr.int(4, 5) as usize;
    let mut shards = Vec::new();
    for i in 0..n {
        let main = i == 0;
        let hh = if main {
            height
        } else {
            height * mr.range(0.4, 0.78)
        };
        let side = if i % 2 == 0 { 1.0 } else { -1.0 };
        let x = if main {
            0.0
        } else {
            side * (mr.range(1.1, 1.7) * ((i + 1) / 2) as f32)
        };
        let w = mr.range(1.2, 1.7) * if main { 1.2 } else { 1.0 };
        let radii = v3(w, hh * 0.56, w * mr.range(0.75, 1.0));
        let tilt = M3::rot_z(mr.sym(0.08) - x * 0.035).then(&M3::rot_x(mr.sym(0.1)));
        shards.push((v3(x, 0.0, mr.sym(0.7)), radii, mr.seed(), mr.sym(0.6), tilt));
    }
    let mut m = Mesh::new();
    let seed0 = shards[0].2;
    for (i, (pos, radii, seed, yaw, tilt)) in shards.into_iter().enumerate() {
        let level = match (lod, i) {
            (0, 0) => 2,
            (0, _) => 1,
            (_, 0) if n <= 2 => 1,
            _ => 0,
        };
        let mut r = placed_rock(level, radii, 0.14, 5, 0.06, seed, pos, yaw, tilt, 0.4, true);
        paint_rock(
            &mut r,
            base,
            dark,
            seed0,
            v3(0.0, height * 0.5, 0.0),
            height * 0.35,
            0.0,
            1.0,
        );
        m.append(&r);
    }
    m.ground_ao(1.0, 0.35);
    m
}

fn pebbles(rng: &mut Rng, lod: u32) -> Mesh {
    let mut mr = rng.fork();
    let n = mr.int(11, 14) as usize;
    let cols = [
        (srgb(0.58, 0.56, 0.52), srgb(0.42, 0.40, 0.38)),
        (srgb(0.62, 0.55, 0.46), srgb(0.46, 0.40, 0.33)),
        (srgb(0.46, 0.47, 0.48), srgb(0.32, 0.33, 0.34)),
        (srgb(0.70, 0.66, 0.60), srgb(0.52, 0.49, 0.45)),
        (srgb(0.50, 0.44, 0.40), srgb(0.36, 0.31, 0.28)),
    ];
    let mut stones: Vec<(V3, V3, u32, f32, (C3, C3), f32)> = Vec::new();
    let mut tries = 0;
    while stones.len() < n && tries < 400 {
        tries += 1;
        let i = stones.len();
        let s = if i < 3 {
            mr.range(0.12, 0.17)
        } else {
            mr.range(0.05, 0.11)
        };
        let a = mr.angle();
        let d = 0.72 * mr.f().sqrt();
        let pos = V3::polar(a) * d;
        if stones
            .iter()
            .any(|st| (st.0 - pos).len() < (st.5 + s) * 1.05)
        {
            continue;
        }
        let radii = v3(
            s * mr.range(1.0, 1.35),
            s * mr.range(0.38, 0.55),
            s * mr.range(0.75, 1.0),
        );
        stones.push((pos, radii, mr.seed(), mr.angle(), mr.pick(&cols), s));
    }
    let mut far: Vec<usize> = (0..stones.len()).collect();
    far.sort_by(|&a, &b| {
        (stones[b].0.len() + stones[b].5)
            .partial_cmp(&(stones[a].0.len() + stones[a].5))
            .unwrap()
    });
    let far_keep: Vec<usize> = (0..3.min(stones.len()))
        .chain(far.into_iter().filter(|&i| i >= 3).take(3))
        .collect();
    let mut m = Mesh::new();
    for (i, (pos, radii, seed, yaw, (c0, c1), s)) in stones.into_iter().enumerate() {
        if lod > 0 && !far_keep.contains(&i) {
            continue;
        }
        let level = if lod == 0 && i < 6 { 1 } else { 0 };
        let mut r = placed_rock(
            level,
            radii,
            0.1,
            2,
            0.6,
            seed,
            pos,
            yaw,
            M3::I,
            s * 0.2,
            false,
        );
        paint_rock(
            &mut r,
            c0,
            c1,
            seed,
            pos + V3::Y * (radii.y * 0.5),
            s * 2.0,
            0.0,
            0.15,
        );
        m.append(&r);
    }
    m.ground_ao(0.1, 0.3);
    m
}

// =====================================================================================
// Plants
// =====================================================================================

fn bush(flowering: bool, rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let size = mr.range(0.62, 0.8);
    let h = size * mr.range(1.6, 1.95);
    let seed = mr.seed();
    let pals = [
        [
            srgb(0.20, 0.36, 0.12),
            srgb(0.25, 0.40, 0.14),
            srgb(0.18, 0.32, 0.12),
        ],
        [
            srgb(0.26, 0.38, 0.13),
            srgb(0.31, 0.43, 0.15),
            srgb(0.22, 0.34, 0.12),
        ],
        [
            srgb(0.17, 0.32, 0.16),
            srgb(0.22, 0.37, 0.18),
            srgb(0.15, 0.29, 0.14),
        ],
    ];
    let pal = mr.pick(&pals);
    let tip = srgb(0.42, 0.55, 0.22);
    let flower_cols = [
        srgb(0.95, 0.55, 0.70),
        srgb(0.97, 0.95, 0.90),
        srgb(0.75, 0.45, 0.85),
        srgb(0.98, 0.80, 0.25),
        srgb(0.92, 0.30, 0.30),
    ];
    let fcol = mr.pick(&flower_cols);
    let fcol2 = cmix(fcol, srgb(1.0, 1.0, 1.0), 0.35);
    let n = if flowering { 4 } else { 6 };
    let cc = v3(0.0, h * 0.5, 0.0);
    let mut clumps: Vec<(V3, f32)> = vec![(v3(0.0, h - size * 0.95, 0.0), size)];
    let a0 = mr.angle();
    for i in 1..n {
        let a = a0 + i as f32 * TAU / (n - 1) as f32 + mr.sym(0.4);
        let r = size * mr.range(0.6, 0.78);
        clumps.push((
            V3::polar(a) * (size * mr.range(0.55, 0.8)) + V3::Y * (r * mr.range(0.42, 0.55)),
            r,
        ));
    }
    let canopy = Canopy {
        clumps: clumps.clone(),
        center: cc,
        radii: v3(size * 1.3, h * 0.5, size * 1.3),
    };
    let mut m = Mesh::new();
    if full {
        for (i, &(c, r)) in clumps.iter().enumerate() {
            let radii = v3(
                r * mr.range(0.95, 1.1),
                r * mr.range(0.8, 0.95),
                r * mr.range(0.95, 1.1),
            );
            let mut b = blob(1, radii, 0.3, 2.3, seed ^ (i as u32 * 13), 0.6);
            b.translate(c);
            b.bend_normals(0.55, |p| (p - c).norm() * 0.4 + (p - cc).norm() * 0.6);
            canopy.paint(
                &mut b,
                i,
                pal[i % 3],
                tip,
                seed ^ i as u32,
                V3::ZERO,
                size * 1.6,
            );
            b.set_alpha(|p, _| {
                0.1 + 0.5 * clamp01(p.y / h) + 0.15 * clamp01(p.horiz().len() / size - 0.5)
            });
            m.append(&b);
        }
        if flowering {
            for k in 0..20 {
                let (c, r) = clumps[k % n];
                let mut d = mr.unit();
                d.y = d.y.abs() * 0.8 + 0.15;
                let d = d.norm();
                let pos = c + d * (r * 1.0);
                if pos.y < h * 0.25 {
                    continue;
                }
                let mut fl = octa();
                fl.scale3(v3(0.1, 0.05, 0.1));
                fl.transform(&M3::from_to(V3::Y, d), pos);
                let col = if k % 3 == 0 { fcol2 } else { fcol };
                fl.paint(|p, n, _| {
                    rgba(
                        cscale(col, 0.85 + 0.15 * n.y + 0.05 * noise3(p * 9.0, seed)),
                        0.6,
                    )
                });
                m.append(&fl);
            }
        }
    } else {
        for (i, &(c, r)) in clumps.iter().take(3).enumerate() {
            let rr = if i == 0 { r * 1.12 } else { r * 1.0 };
            let mut b = blob(0, v3(rr, rr * 0.85, rr), 0.12, 1.5, seed ^ i as u32, 0.7);
            b.translate(c);
            b.bend_normals(0.5, |p| p - cc);
            canopy.paint(
                &mut b,
                i,
                pal[i % 3],
                tip,
                seed ^ i as u32,
                V3::ZERO,
                size * 1.6,
            );
            if flowering {
                for k in 0..b.c.len() {
                    if b.p[k].y > h * 0.45 && hashf(k as u32 * 7 + i as u32, seed) > 0.65 {
                        let c0 = b.c[k];
                        b.c[k] = rgba(cmix([c0[0], c0[1], c0[2]], fcol, 0.28), c0[3]);
                    }
                }
            }
            b.set_alpha(|p, _| 0.1 + 0.5 * clamp01(p.y / h));
            m.append(&b);
        }
    }
    // rest on (slightly in) the ground
    let (mn, _) = m.bounds();
    m.translate(V3::Y * (-mn.y - 0.06));
    m.ground_ao(0.35, 0.4);
    m
}

fn fern(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let n = mr.int(10, 11) as usize;
    let seed = mr.seed();
    let scale = mr.range(0.85, 1.15);
    // far LOD keeps five well spread fronds
    let far_keep: Vec<usize> = (0..7).map(|q| q * n / 7).collect();
    let dark = srgb(0.16, 0.32, 0.10);
    let leaf = srgb(0.27, 0.47, 0.14);
    let tip = srgb(0.46, 0.62, 0.22);
    let a0 = mr.angle();
    let mut m = Mesh::new();
    for i in 0..n {
        let a = a0 + i as f32 * TAU / n as f32 + mr.sym(0.25);
        let elev = mr.range(0.95, 1.3);
        let len = mr.range(0.8, 1.15) * scale;
        let droop = mr.range(0.5, 0.8);
        let wmax = mr.range(0.11, 0.15) * scale;
        if !full && !far_keep.contains(&i) {
            continue;
        }
        let wmax = if full { wmax } else { wmax * 1.3 };
        let dir = V3::polar(a);
        let side = V3::Y.cross(dir).norm();
        let k = if full { 10 } else { 3 };
        let base = dir * 0.04;
        let mut pts = Vec::new();
        let mut ws = Vec::new();
        let peak = (0.5 / (droop * 1.1)).clamp(0.3, 0.85);
        for j in 0..=k {
            let s = if full {
                j as f32 / k as f32
            } else {
                [0.0, peak, (peak + 1.0) * 0.5, 1.0][j]
            };
            pts.push(
                base + dir * (len * s * elev.cos())
                    + V3::Y
                        * (len * s * elev.sin() - droop * len * s * s * elev.sin() * 1.1 - 0.03),
            );
            let env = (PI * (0.15 + 0.85 * s)).sin().max(0.0).powf(0.7);
            let serr = if full && j % 2 == 1 { 0.45 } else { 1.0 };
            ws.push(if j == 0 || j == k {
                0.0
            } else {
                wmax * env * serr
            });
        }
        let mut f = leaf_strip(&pts, &ws, side, 0.0, false);
        // curl the leaflets a little: lower the edges
        for q in 0..f.p.len() {
            let across = (f.uv[q][0] - 0.5).abs() * 2.0;
            f.p[q].y -= across * 0.03 * scale;
        }
        f.smooth_normals();
        if !full {
            f.unweld(0.3);
        }
        f.double_sided(0.8);
        f.bend_normals(0.5, |p| p.horiz() * 0.5 + V3::Y);
        f.paint(|p, _, uv| {
            let s = uv[1];
            let nz = noise3(p * 4.0, seed);
            let mut c = cmix(dark, leaf, smoothstep(0.0, 0.35, s));
            c = cmix(c, tip, smoothstep(0.55, 1.0, s) * 0.7 + 0.1 * nz);
            rgba(cscale(c, 0.9 + 0.12 * nz), 0.1 + 0.9 * s)
        });
        m.append(&f);
    }
    m.ground_ao(0.25, 0.35);
    m
}

fn flowers(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let palette = [
        srgb(0.98, 0.82, 0.18),
        srgb(0.62, 0.36, 0.86),
        srgb(0.97, 0.96, 0.92),
        srgb(0.90, 0.20, 0.16),
        srgb(0.98, 0.55, 0.72),
        srgb(0.38, 0.55, 0.95),
        srgb(0.98, 0.58, 0.16),
    ];
    let stem_c = srgb(0.28, 0.45, 0.15);
    let leaf_c = srgb(0.26, 0.46, 0.14);
    let center_cols = [srgb(0.95, 0.75, 0.15), srgb(0.35, 0.22, 0.10)];
    let n = mr.int(6, 8) as usize;
    let mut specs = Vec::new();
    for i in 0..n {
        let a = mr.angle();
        let d = 0.28 * mr.f().sqrt();
        let hh = mr.range(0.3, 0.6);
        let col = palette[(i + mr.int(0, 6) as usize) % palette.len()];
        specs.push((V3::polar(a) * d, hh, col, mr.pick(&center_cols), mr.seed()));
    }
    let mut m = Mesh::new();
    for &(base, hh, col, cen, fs) in specs.iter() {
        let mut fr = Rng::new(fs as u64);
        let lean = V3::polar(fr.angle()) * (hh * fr.range(0.05, 0.2));
        let top = base + lean + V3::Y * hh;
        let mid = base + lean * 0.3 + V3::Y * (hh * 0.5);
        if full {
            let mut stem = tube(
                &[base - V3::Y * 0.02, mid, top],
                &[0.009, 0.007, 0.0],
                3,
                Cap::Open,
                Cap::Open,
            );
            stem.paint(|p, _, _| {
                rgba(
                    cscale(stem_c, 0.85 + 0.2 * p.y / hh),
                    0.7 * clamp01(p.y / hh),
                )
            });
            m.append(&stem);
            // two leaves
            for l in 0..2 {
                let la = fr.angle();
                let ld = V3::polar(la);
                let at = base.lerp(mid, 0.4 + 0.3 * l as f32);
                let tip = at + ld * fr.range(0.1, 0.16) + V3::Y * fr.range(0.04, 0.1);
                let lm = at.lerp(tip, 0.5) + V3::Y * 0.02;
                let mut lf = leaf_strip(
                    &[at, lm, tip],
                    &[0.0, 0.025, 0.0],
                    ld.cross(V3::Y).norm(),
                    0.0,
                    false,
                );
                lf.double_sided(0.85);
                lf.bend_normals(0.4, |_| V3::Y);
                lf.paint(|p, _, uv| {
                    rgba(
                        cscale(leaf_c, 0.85 + 0.2 * uv[1]),
                        0.5 * clamp01(p.y / hh) + 0.2,
                    )
                });
                m.append(&lf);
            }
            // petals
            let k = fr.int(5, 6) as usize;
            let up = (V3::Y * 2.0 + lean.norm() * 0.6).norm();
            let rot = M3::from_to(V3::Y, up);
            let plen = fr.range(0.06, 0.09);
            let cup = fr.range(0.25, 0.6);
            let pa0 = fr.angle();
            let mut head = Mesh::new();
            for q in 0..k {
                let pa = pa0 + q as f32 * TAU / k as f32;
                let d = (V3::polar(pa) * cup.cos() + V3::Y * cup.sin()).norm();
                let p0 = V3::ZERO;
                let p2 = d * plen;
                let p1 = d * (plen * 0.55) + V3::Y * 0.006;
                let side = V3::polar(pa + PI * 0.5);
                let mut pm = leaf_strip(&[p0, p1, p2], &[0.0, plen * 0.38, 0.0], side, 0.0, false);
                pm.double_sided(0.8);
                pm.bend_normals(0.5, |_| V3::Y);
                head.append(&pm);
            }
            head.paint(|_, _, uv| rgba(cscale(col, 0.75 + 0.25 * uv[1]), 0.75));
            let mut center = tube(
                &[v3(0.0, -0.004, 0.0), v3(0.0, 0.012, 0.0)],
                &[plen * 0.28, 0.0],
                5,
                Cap::Flat,
                Cap::Open,
            );
            center.paint(|_, n, _| rgba(cscale(cen, 0.8 + 0.2 * n.y), 0.75));
            head.append(&center);
            head.transform(&rot, top);
            m.append(&head);
        } else {
            // far: crossed stem card + one colored diamond
            let side = V3::polar(fr.angle());
            let mut st = Mesh::new();
            let a = st.vert(base - side * 0.012, V3::Y, [0.0, 0.0]);
            let b = st.vert(base + side * 0.012, V3::Y, [1.0, 0.0]);
            let c = st.vert(top, V3::Y, [0.5, 1.0]);
            st.tri(a, b, c);
            st.smooth_normals();
            st.double_sided(0.9);
            st.paint(|_, _, uv| rgba(stem_c, 0.6 * uv[1]));
            m.append(&st);
            let d0 = V3::polar(fr.angle()) * 0.07;
            let d1 = V3::Y.cross(d0);
            let mut hd = Mesh::new();
            let i0 = hd.vert(top - d0, V3::Y, [0.0, 0.0]);
            let i1 = hd.vert(top + d1 + V3::Y * 0.02, V3::Y, [0.0, 0.0]);
            let i2 = hd.vert(top + d0, V3::Y, [0.0, 0.0]);
            let i3 = hd.vert(top - d1 + V3::Y * 0.02, V3::Y, [0.0, 0.0]);
            hd.quad(i0, i3, i2, i1);
            hd.smooth_normals();
            hd.double_sided(0.85);
            hd.paint(|_, _, _| rgba(col, 0.7));
            m.append(&hd);
        }
    }
    // a few grass blades at the base
    let nb = if full { 6 } else { 0 };
    for b in 0..nb {
        let a = mr.angle();
        let out = V3::polar(a);
        let base = out * mr.range(0.0, 0.2);
        let l = mr.range(0.18, 0.32);
        let pts = [
            base,
            base + out * (l * 0.25) + V3::Y * (l * 0.55),
            base + out * (l * 0.6) + V3::Y * l,
        ];
        let mut bl = leaf_strip(
            &pts,
            &[0.012, 0.009, 0.0],
            V3::polar(a + PI * 0.5),
            0.0,
            false,
        );
        bl.double_sided(0.85);
        bl.bend_normals(0.5, |_| V3::Y);
        let bs = seed ^ b;
        bl.paint(|p, _, uv| {
            rgba(
                cscale(
                    cmix(leaf_c, srgb(0.5, 0.6, 0.25), uv[1]),
                    0.85 + 0.1 * noise3(p * 9.0, bs),
                ),
                uv[1],
            )
        });
        m.append(&bl);
    }
    m.ground_ao(0.12, 0.35);
    m
}

/// Grass blade from `base` leaning outward along `out`.
fn grass_blade(
    base: V3,
    out: V3,
    len: f32,
    w: f32,
    tilt: f32,
    bend: f32,
    twist: f32,
    segs: usize,
) -> Mesh {
    let mut pts = Vec::new();
    let mut ws = Vec::new();
    for k in 0..=segs {
        let t = k as f32 / segs as f32;
        let horiz = len * (tilt.sin() * t + bend * t * t);
        let vert = len * (tilt.cos() * t - bend * 0.35 * t * t);
        pts.push(base + out * horiz + V3::Y * vert);
        ws.push(if k == segs { 0.0 } else { w * (1.0 - 0.55 * t) });
    }
    let side = M3::rot_y(twist).apply(V3::Y.cross(out).norm());
    leaf_strip(&pts, &ws, side, 0.0, false)
}

fn tall_grass(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let n = mr.int(24, 28) as usize;
    let base_c = srgb(0.18, 0.30, 0.09);
    let mid_c = srgb(0.34, 0.50, 0.15);
    let tip_c = srgb(0.62, 0.66, 0.30);
    let dry = srgb(0.72, 0.64, 0.38);
    let height = mr.range(0.75, 0.95);
    let mut m = Mesh::new();
    let mut blades = Vec::new();
    for i in 0..n {
        let a = i as f32 * 2.39996 + mr.sym(0.3);
        let out = V3::polar(a);
        let r = 0.14 * mr.f().sqrt();
        let len = height * mr.range(0.6, 1.0);
        let tilt = mr.range(0.08, 0.5) * (0.5 + r / 0.14);
        let (w, bend, twist) = (mr.range(0.022, 0.034), mr.range(0.1, 0.35), mr.sym(0.6));
        let is_dry = mr.chance(0.15);
        blades.push((i, a, out, r, len, tilt, w, bend, twist, is_dry));
    }
    let keep: Vec<usize> = if full {
        (0..n).collect()
    } else {
        let sectors = 10;
        let mut best: Vec<Option<(usize, f32)>> = vec![None; sectors];
        for b in &blades {
            let reach = b.3 + b.4 * (b.5.sin() + b.7);
            let sct = ((b.1.rem_euclid(TAU)) / TAU * sectors as f32) as usize % sectors;
            if best[sct].map(|(_, r)| reach > r).unwrap_or(true) {
                best[sct] = Some((b.0, reach));
            }
        }
        best.into_iter().flatten().map(|(i, _)| i).collect()
    };
    for &(i, _a, out, r, len, tilt, w, bend, twist, is_dry) in
        blades.iter().filter(|b| keep.contains(&b.0))
    {
        let w = if full { w } else { w * 1.6 };
        let mut b = grass_blade(
            out * r,
            out,
            len,
            w,
            tilt,
            bend,
            twist,
            if full { 3 } else { 2 },
        );
        b.double_sided(0.85);
        b.bend_normals(0.55, |p| p.horiz() * 0.6 + V3::Y);
        let bs = seed ^ i as u32;
        b.paint(|p, _, uv| {
            let t = uv[1];
            let mut c = cmix(base_c, mid_c, smoothstep(0.0, 0.45, t));
            c = cmix(c, tip_c, smoothstep(0.5, 1.0, t) * 0.8);
            if is_dry {
                c = cmix(c, dry, 0.3 + 0.6 * t);
            }
            rgba(cscale(c, 0.92 + 0.12 * noise3(p * 6.0, bs)), t.powf(1.3))
        });
        m.append(&b);
    }
    m.ground_ao(0.15, 0.3);
    m
}

fn reeds(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let stem_c = srgb(0.40, 0.48, 0.20);
    let head_c = srgb(0.36, 0.22, 0.11);
    let leaf_c = srgb(0.33, 0.49, 0.16);
    let leaf_tip = srgb(0.62, 0.62, 0.30);
    // macro layout shared by both LODs
    let n_stems = mr.int(9, 12) as usize;
    let mut stems = Vec::new();
    for _ in 0..n_stems {
        let a = mr.angle();
        let base = V3::polar(a) * (0.32 * mr.f().sqrt());
        let hh = mr.range(1.45, 2.0);
        let lean = V3::polar(a + mr.sym(0.5)) * (hh * mr.range(0.02, 0.1));
        let y0 = hh * mr.range(0.72, 0.8);
        let y1 = y0 + mr.range(0.16, 0.24);
        stems.push((base, hh, lean, y0, y1));
    }
    let n_leaves = mr.int(9, 11) as usize;
    let mut leaves = Vec::new();
    for _ in 0..n_leaves {
        let a = mr.angle();
        let out = V3::polar(a);
        leaves.push((
            out * (0.3 * mr.f().sqrt()),
            out,
            mr.range(0.9, 1.5),
            mr.range(0.022, 0.03),
            mr.range(0.1, 0.35),
            mr.range(0.15, 0.4),
            mr.sym(0.5),
        ));
    }
    let mut m = Mesh::new();
    for (i, &(base, hh, lean, head_y0, head_y1)) in stems.iter().enumerate() {
        let has_head = i % 2 == 0;
        if !full && i > 1 {
            continue;
        }
        let top = base + lean + V3::Y * hh;
        let at = |y: f32| base + lean * (y / hh) * (y / hh) + V3::Y * y;
        let mut stem = if !full {
            let mut s = tube(
                &[
                    base - V3::Y * 0.05,
                    at(head_y0),
                    at(head_y0 + 0.02),
                    at(head_y1),
                    top,
                ],
                &[0.012, 0.01, 0.03, 0.03, 0.0],
                3,
                Cap::Open,
                Cap::Open,
            );
            s.paint(|p, _, _| {
                let inside = p.y > head_y0 - 0.01 && p.y < head_y1 + 0.01;
                rgba(
                    if inside { head_c } else { stem_c },
                    0.8 * clamp01(p.y / hh),
                )
            });
            s
        } else if has_head {
            let mut s = tube(
                &[base - V3::Y * 0.05, at(head_y0 * 0.5), at(head_y0)],
                &[0.011, 0.009, 0.008],
                3,
                Cap::Open,
                Cap::Open,
            );
            s.append(&tube(
                &[at(head_y1), top],
                &[0.006, 0.0],
                3,
                Cap::Open,
                Cap::Open,
            ));
            s.paint(|p, _, _| rgba(stem_c, 0.8 * clamp01(p.y / hh)));
            let mut hd = tube(
                &[
                    at(head_y0),
                    at(head_y0 + 0.02),
                    at(head_y1 - 0.02),
                    at(head_y1),
                ],
                &[0.008, 0.026, 0.027, 0.012],
                5,
                Cap::Open,
                Cap::Flat,
            );
            hd.paint(|p, _, _| rgba(cscale(head_c, 0.85 + 0.15 * noise3(p * 20.0, seed)), 0.8));
            s.append(&hd);
            s
        } else {
            let mut s = tube(
                &[base - V3::Y * 0.05, at(hh * 0.5), top],
                &[0.01, 0.008, 0.0],
                3,
                Cap::Open,
                Cap::Open,
            );
            s.paint(|p, _, _| rgba(stem_c, 0.8 * clamp01(p.y / hh)));
            s
        };
        stem.bend_normals(0.2, |_| V3::Y);
        m.append(&stem);
    }
    // far LOD keeps the widest leaves so the silhouette matches
    let mut order: Vec<usize> = (0..leaves.len()).collect();
    order.sort_by(|&x, &y| {
        let reach = |l: &(V3, V3, f32, f32, f32, f32, f32)| l.0.len() + l.2 * (l.4.sin() + l.5);
        reach(&leaves[y]).partial_cmp(&reach(&leaves[x])).unwrap()
    });
    let picks: Vec<usize> = if full {
        order.clone()
    } else {
        let mut v = Vec::new();
        for sct in 0..3 {
            if let Some(&i) = order.iter().find(|&&i| {
                let a = leaves[i].1.z.atan2(leaves[i].1.x).rem_euclid(TAU);
                (a / TAU * 3.0) as usize % 3 == sct
            }) {
                v.push(i);
            }
        }
        v
    };
    for &i in picks.iter() {
        let (base, out, len, w, tilt, bend, twist) = leaves[i];
        let mut b = grass_blade(
            base,
            out,
            len,
            w,
            tilt,
            bend,
            twist,
            if full { 3 } else { 2 },
        );
        b.double_sided(0.85);
        b.bend_normals(0.5, |p| p.horiz() * 0.5 + V3::Y);
        let bs = seed ^ (i as u32 + 50);
        b.paint(|p, _, uv| {
            let t = uv[1];
            let c = cmix(leaf_c, leaf_tip, smoothstep(0.4, 1.0, t) * 0.8);
            rgba(
                cscale(c, 0.8 + 0.2 * t + 0.08 * noise3(p * 5.0, bs)),
                t.powf(1.2),
            )
        });
        m.append(&b);
    }
    m.ground_ao(0.2, 0.3);
    m
}

fn cactus(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let h = mr.range(1.9, 2.4);
    let r = mr.range(0.19, 0.25);
    let n_arms = mr.int(1, 2) as usize;
    let green = srgb(0.30, 0.45, 0.22);
    let ridge = srgb(0.44, 0.56, 0.30);
    let groove = srgb(0.18, 0.30, 0.15);
    let dry = srgb(0.50, 0.46, 0.32);
    let ribs = 6.0;
    let paint = |m: &mut Mesh, ribs: f32| {
        m.paint(|p, _, uv| {
            let rib = (uv[0] * TAU * ribs).cos();
            let mut c = cmix(groove, green, smoothstep(-1.0, 0.0, rib));
            c = cmix(c, ridge, smoothstep(0.3, 1.0, rib) * 0.7);
            c = cmix(c, dry, (1.0 - smoothstep(0.0, 0.35, p.y)) * 0.5);
            rgba(cscale(c, 0.92 + 0.1 * noise3(p * 5.0, seed)), 0.0)
        });
    };
    // column with a domed top
    let column = |base: Vec<V3>, r: f32, sides: u32, ribs: f32, dome: usize| -> Mesh {
        let mut path = base.clone();
        let mut rads: Vec<f32> = vec![r; base.len()];
        let last = *base.last().unwrap();
        let dir = (last - base[base.len() - 2]).norm();
        for k in 1..=dome {
            let phi = k as f32 / (dome + 1) as f32 * PI * 0.5;
            path.push(last + dir * (r * phi.sin()));
            rads.push(r * phi.cos());
        }
        path.push(last + dir * r);
        rads.push(0.0);
        let rr = rads.clone();
        tube_fn(&path, sides, Cap::Open, Cap::Open, move |i, a| {
            rr[i] * (1.0 + 0.09 * (ribs * a).cos())
        })
    };
    let mut m = Mesh::new();
    let rings = if full { 6 } else { 2 };
    let main_path: Vec<V3> = (0..=rings)
        .map(|i| V3::Y * lerp(-0.2, h - r, i as f32 / rings as f32))
        .collect();
    let mut main = column(
        main_path,
        r,
        if full { 12 } else { 5 },
        ribs,
        if full { 2 } else { 0 },
    );
    paint(&mut main, if full { ribs } else { 2.5 });
    m.append(&main);
    let a0 = mr.angle();
    for k in 0..n_arms {
        let a = a0 + k as f32 * PI + mr.sym(0.4);
        let out = V3::polar(a);
        let y = h * mr.range(0.35, 0.55);
        let ra = r * mr.range(0.6, 0.72);
        let up = mr.range(0.45, 0.8);
        let reach = r + ra + mr.range(0.12, 0.25);
        let base = V3::Y * y;
        let path = if full {
            vec![
                base,
                base + out * (reach * 0.55) + V3::Y * 0.02,
                base + out * (reach * 0.85) + V3::Y * (ra * 0.4),
                base + out * reach + V3::Y * (ra * 1.3),
                base + out * reach + V3::Y * (ra * 1.3 + up * 0.5),
                base + out * (reach * 0.98) + V3::Y * (ra * 1.3 + up),
            ]
        } else {
            vec![
                base,
                base + out * reach + V3::Y * (ra * 0.8),
                base + out * reach + V3::Y * (ra * 1.3 + up),
            ]
        };
        let mut arm = column(
            path,
            ra,
            if full { 8 } else { 3 },
            4.0,
            if full { 2 } else { 0 },
        );
        paint(&mut arm, if full { 4.0 } else { 1.5 });
        m.append(&arm);
    }
    if full {
        // a few blossoms on top
        let fc = srgb(0.95, 0.85, 0.55);
        for k in 0..3 {
            let a = a0 + k as f32 * 2.1;
            let mut f = octa();
            f.scale3(v3(0.05, 0.035, 0.05));
            f.translate(V3::Y * (h - r * 0.25) + V3::polar(a) * (r * 0.45));
            f.paint(|_, n, _| rgba(cscale(fc, 0.85 + 0.15 * n.y), 0.0));
            m.append(&f);
        }
    }
    m.ground_ao(0.3, 0.35);
    m
}

fn mushrooms(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let styles = [
        (srgb(0.80, 0.12, 0.07), true),
        (srgb(0.52, 0.33, 0.19), false),
        (srgb(0.82, 0.50, 0.20), false),
    ];
    let (cap_c, spots) = mr.pick(&styles);
    let gill = srgb(0.86, 0.80, 0.68);
    let stem_c = srgb(0.90, 0.87, 0.80);
    let white = srgb(0.97, 0.96, 0.92);
    let n = mr.int(3, 5) as usize;
    let mut specs = Vec::new();
    for i in 0..n {
        let hs = if i == 0 {
            mr.range(0.28, 0.4)
        } else {
            mr.range(0.1, 0.26)
        };
        let a = mr.angle();
        let d = if i == 0 { 0.0 } else { mr.range(0.12, 0.3) };
        specs.push((
            V3::polar(a) * d,
            hs,
            hs * mr.range(0.45, 0.6),
            M3::tilt(&mut mr, 0.25),
            mr.seed(),
        ));
    }
    let mut m = Mesh::new();
    for (i, &(pos, hs, rc, tilt, ms)) in specs.iter().enumerate() {
        if !full && i >= 2 {
            continue;
        }
        let rs = rc * 0.22;
        let sides = if full { 8 } else { 5 };
        let mut stem = if full {
            tube(
                &[
                    v3(0.0, -0.03, 0.0),
                    v3(0.0, hs * 0.5, 0.0),
                    v3(0.0, hs + rc * 0.05, 0.0),
                ],
                &[rs * 1.3, rs * 1.0, rs * 0.9],
                6,
                Cap::Open,
                Cap::Open,
            )
        } else {
            tube(
                &[v3(0.0, -0.03, 0.0), v3(0.0, hs, 0.0)],
                &[rs * 1.2, rs],
                4,
                Cap::Open,
                Cap::Open,
            )
        };
        stem.paint(|p, _, _| {
            rgba(
                cscale(
                    stem_c,
                    0.78 + 0.2 * clamp01(p.y / hs) + 0.05 * noise3(p * 40.0, seed),
                ),
                0.0,
            )
        });
        let yb = hs;
        let prof: Vec<(f32, f32)> = if full {
            vec![
                (rs, yb + 0.03 * rc),
                (rc * 0.95, yb - 0.02 * rc),
                (rc, yb + 0.1 * rc),
                (rc * 0.85, yb + 0.45 * rc),
                (rc * 0.5, yb + 0.72 * rc),
                (0.0, yb + 0.8 * rc),
            ]
        } else {
            vec![
                (0.0, yb + 0.05 * rc),
                (rc * 0.95, yb),
                (rc * 0.6, yb + 0.6 * rc),
                (0.0, yb + 0.8 * rc),
            ]
        };
        let mut cap = lathe(&prof, sides, hashf(i as u32, ms) * TAU);
        let nprof = prof.len() as f32 - 1.0;
        let under_ring = if full { 1 } else { 0 };
        cap.paint(|p, n, uv| {
            let ring = (uv[1] * nprof).round() as i32;
            let under = ring <= under_ring && n.y < 0.2;
            if under {
                return rgba(gill, 0.0);
            }
            let mut c = cscale(
                cap_c,
                0.85 + 0.2 * n.y.max(0.0) + 0.06 * noise3(p * 30.0, ms),
            );
            if spots && ring >= 3 && hashf((uv[0] * 64.0) as u32 + ring as u32 * 97, ms) > 0.55 {
                c = white;
            }
            rgba(c, 0.0)
        });
        let mut one = stem;
        one.append(&cap);
        one.transform(&tilt, pos);
        m.append(&one);
    }
    m.ground_ao(0.08, 0.35);
    m
}

// =====================================================================================
// Debris
// =====================================================================================

fn bark_and_wood(
    m: &mut Mesh,
    bark: C3,
    bark_dark: C3,
    wood: C3,
    wood_dark: C3,
    moss: f32,
    seed: u32,
    grain_axis: V3,
) {
    let moss_c = srgb(0.25, 0.35, 0.12);
    m.paint(|p, n, uv| {
        if uv[1] < -0.5 || uv[1] > 1.5 {
            // cut end: light wood, darker towards the bark, growth ring hint
            let r = uv[0];
            let ringy = (r * 9.0 + noise3(p * 3.0, seed) * 1.5).sin() * 0.5 + 0.5;
            let c = cmix(wood, wood_dark, r * 0.8 + ringy * 0.15);
            return rgba(c, 0.0);
        }
        let along = p.dot(grain_axis);
        let across = p - grain_axis * along;
        let s = noise3(grain_axis * (along * 0.6) + across * 7.0, seed);
        let mut c = cmix(bark, bark_dark, 0.35 + 0.45 * s);
        c = cscale(c, 0.9 + 0.12 * noise3(p * 1.3, seed ^ 5));
        if moss > 0.0 {
            let mm = smoothstep(
                0.55,
                0.85,
                n.y * 0.85 + 0.5 * fbm(p * 1.6, 3, seed ^ 9) + (moss - 1.0) * 0.4,
            );
            c = cmix(c, moss_c, mm * 0.85);
        }
        rgba(c, 0.0)
    });
}

fn fallen_log(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let len = mr.range(5.0, 6.8);
    let r = mr.range(0.32, 0.44);
    let bendz = mr.sym(0.35);
    let k = if full { 12 } else { 3 };
    let path: Vec<V3> = (0..=k)
        .map(|i| {
            let t = i as f32 / k as f32;
            let x = lerp(-len * 0.5, len * 0.5, t);
            let rr = r * lerp(1.0, 0.82, t);
            v3(
                x,
                rr * 0.72 + 0.04 * noise3(v3(t * 3.0, 0.0, 0.0), seed),
                bendz * (t * PI).sin() + 0.08 * noise3(v3(t * 3.0, 4.0, 0.0), seed),
            )
        })
        .collect();
    let mut log = tube_fn(
        &path,
        if full { 12 } else { 5 },
        Cap::Flat,
        Cap::Flat,
        |i, a| {
            let t = i as f32 / k as f32;
            let bump = if full {
                1.0 + 0.06 * noise3(v3(a.cos() * 2.0, t * 8.0, a.sin() * 2.0), seed)
            } else {
                1.0
            };
            r * lerp(1.0, 0.82, t) * bump
        },
    );
    bark_and_wood(
        &mut log,
        srgb(0.36, 0.27, 0.19),
        srgb(0.20, 0.15, 0.11),
        srgb(0.78, 0.62, 0.42),
        srgb(0.55, 0.38, 0.22),
        0.7,
        seed,
        V3::X,
    );
    let mut m = log;
    {
        // broken branch stubs (the first one also on the far LOD)
        for b in 0..mr.int(2, 3) {
            let t = mr.range(0.2, 0.85);
            let i = (t * k as f32) as usize;
            let c = path[i];
            let a = mr.range(0.3, 2.6) * if b % 2 == 0 { 1.0 } else { -1.0 };
            let dir = (v3(mr.sym(0.4), a.sin().abs() + 0.2, a.cos())).norm();
            let rb = r * mr.range(0.22, 0.32);
            let start = c + dir * (r * 0.5);
            let end = c + dir * (r + mr.range(0.25, 0.6));
            if !full && b > 0 {
                continue;
            }
            let mut st = if full {
                tube(
                    &[start, start.lerp(end, 0.5), end],
                    &[rb, rb * 0.9, rb * 0.8],
                    5,
                    Cap::Open,
                    Cap::Flat,
                )
            } else {
                tube(&[start, end], &[rb, rb * 0.8], 3, Cap::Open, Cap::Flat)
            };
            bark_and_wood(
                &mut st,
                srgb(0.36, 0.27, 0.19),
                srgb(0.20, 0.15, 0.11),
                srgb(0.72, 0.58, 0.40),
                srgb(0.5, 0.36, 0.22),
                0.4,
                seed,
                dir,
            );
            m.append(&st);
        }
    }
    m.ground_ao(0.3, 0.35);
    m
}

fn stump(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let r = mr.range(0.38, 0.52);
    let hs = mr.range(0.5, 0.85);
    let slope = mr.sym(0.25);
    let k = if full { 5 } else { 1 };
    let path: Vec<V3> = (0..=k)
        .map(|i| V3::Y * lerp(-0.3, hs, i as f32 / k as f32))
        .collect();
    let lob = mr.angle();
    let mut st = tube_fn(
        &path,
        if full { 11 } else { 5 },
        Cap::Open,
        Cap::Flat,
        |i, a| {
            let y = path[i].y;
            let flare = 1.0
                + 0.45
                    * (1.0 - (y + 0.1) / 0.45).clamp(0.0, 1.0).powi(2)
                    * (0.7 + 0.5 * (4.0 * a + lob).sin().max(0.0));
            r * flare * (1.0 + 0.04 * noise3(v3(a.cos(), y * 3.0, a.sin()), seed))
        },
    );
    // slanted, jagged cut
    let sides_f = if full { 11.0 } else { 5.0 };
    for q in 0..st.p.len() {
        if st.uv[q][1] > 0.99 {
            let p = st.p[q];
            let is_center = st.uv[q][1] > 1.5 && st.uv[q][0] < 0.5;
            let j =
                ((-p.z).atan2(p.x).rem_euclid(TAU) / TAU * sides_f).round() as u32 % sides_f as u32;
            let jag = if full && !is_center {
                0.07 * hashf(j, seed)
            } else {
                0.0
            };
            st.p[q].y += p.x * slope + jag;
        }
    }
    st.smooth_normals();
    // keep the cap flat shaded
    for q in 0..st.p.len() {
        if st.uv[q][1] > 1.5 {
            st.n[q] = v3(-slope, 1.0, 0.0).norm();
        }
    }
    let bark = srgb(0.36, 0.27, 0.20);
    let bark_dark = srgb(0.20, 0.15, 0.12);
    bark_and_wood(
        &mut st,
        bark,
        bark_dark,
        srgb(0.80, 0.64, 0.44),
        srgb(0.56, 0.40, 0.24),
        0.7,
        seed,
        V3::Y,
    );
    let mut m = st;
    let nr = mr.int(5, 6) as usize;
    let a0 = mr.angle();
    for j in 0..nr {
        let a = a0 + j as f32 * TAU / nr as f32 + mr.sym(0.3);
        let out = V3::polar(a);
        let len = r * mr.range(1.3, 2.0);
        let rr = r * mr.range(0.28, 0.36);
        if !full && j >= 4 {
            continue;
        }
        let path = if full {
            vec![
                out * (r * 0.4) + V3::Y * 0.3,
                out * (r * 0.9) + V3::Y * 0.14,
                out * (r * 0.95 + len * 0.45) + V3::Y * 0.02,
                out * (r * 0.95 + len) - V3::Y * 0.1,
            ]
        } else {
            vec![
                out * (r * 0.5) + V3::Y * 0.22,
                out * (r + len * 0.4) + V3::Y * 0.03,
                out * (r + len) - V3::Y * 0.1,
            ]
        };
        let rads: Vec<f32> = if full {
            vec![rr * 1.25, rr, rr * 0.55, 0.0]
        } else {
            vec![rr * 1.1, rr * 0.7, 0.0]
        };
        let mut root = tube(&path, &rads, if full { 6 } else { 3 }, Cap::Open, Cap::Open);
        root.scale3(v3(1.0, 0.75, 1.0));
        bark_and_wood(
            &mut root,
            bark,
            bark_dark,
            bark,
            bark,
            0.5,
            seed ^ j as u32,
            out,
        );
        m.append(&root);
    }
    m.ground_ao(0.3, 0.35);
    m
}

fn branches(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let n = mr.int(10, 13) as usize;
    let cols = [
        srgb(0.42, 0.34, 0.26),
        srgb(0.50, 0.42, 0.33),
        srgb(0.35, 0.27, 0.20),
        srgb(0.46, 0.40, 0.34),
    ];
    let mut sticks = Vec::new();
    for i in 0..n {
        let yaw = mr.angle();
        let len = mr.range(0.9, 2.0);
        let r = mr.range(0.025, 0.06);
        let c = V3::polar(mr.angle()) * (0.45 * mr.f().sqrt());
        let lift = r + (i as f32 / n as f32) * 0.22 + mr.range(0.0, 0.05);
        let pitch = mr.sym(0.12);
        let dir = M3::rot_y(yaw).apply(v3(pitch.cos(), pitch.sin(), 0.0));
        let a = c - dir * (len * 0.5) + V3::Y * lift;
        let b = c + dir * (len * 0.5) + V3::Y * lift;
        let a = v3(a.x, a.y.max(r * 0.6), a.z);
        let b = v3(b.x, b.y.max(r * 0.6), b.z);
        let bow =
            M3::rot_y(yaw).apply(V3::Z) * mr.sym(len * 0.06) + V3::Y * mr.range(0.0, len * 0.03);
        let col = mr.pick(&cols);
        let twig = (
            mr.chance(0.6),
            mr.range(0.3, 0.7),
            mr.sym(1.2),
            mr.range(0.1, 0.5),
            mr.range(0.25, 0.5),
        );
        sticks.push((i, len + c.len(), r, dir, a, b, bow, col, twig));
    }
    // far LOD: lowest two, highest two and the two longest remaining sticks
    let keep: Vec<usize> = if full {
        (0..n).collect()
    } else {
        let mut k = vec![0, 1, n - 2, n - 1];
        let mut order: Vec<usize> = (2..n - 2).collect();
        order.sort_by(|&x, &y| sticks[y].1.partial_cmp(&sticks[x].1).unwrap());
        k.extend(order.into_iter().take(2));
        k
    };
    let mut m = Mesh::new();
    for &(i, _, r, dir, a, b, bow, col, twig) in sticks.iter().filter(|st| keep.contains(&st.0)) {
        let path = if full {
            vec![a, a.lerp(b, 0.5) + bow, b]
        } else {
            vec![a, b]
        };
        let rads = if full {
            vec![r, r * 0.85, r * 0.7]
        } else {
            vec![r * 1.2, r]
        };
        let mut st = tube(&path, &rads, if full { 5 } else { 3 }, Cap::Flat, Cap::Flat);
        let s = seed ^ i as u32;
        st.paint(|p, nn, uv| {
            if uv[1] < -0.5 || uv[1] > 1.5 {
                return rgba(srgb(0.70, 0.58, 0.42), 0.0);
            }
            let c = cscale(col, 0.85 + 0.15 * nn.y + 0.12 * noise3(p * 6.0, s));
            rgba(c, 0.0)
        });
        m.append(&st);
        if full && twig.0 {
            let t = twig.1;
            let at = a.lerp(b, t) + bow * (1.0 - (2.0 * t - 1.0).powi(2));
            let tw = (M3::rot_y(twig.2).apply(dir) + V3::Y * twig.3).norm();
            let tl = twig.4;
            let mut twig = tube(
                &[at, at + tw * tl],
                &[r * 0.55, 0.0],
                4,
                Cap::Open,
                Cap::Open,
            );
            twig.paint(|p, _, _| rgba(cscale(col, 0.9 + 0.1 * noise3(p * 6.0, s)), 0.0));
            m.append(&twig);
        }
    }
    m.ground_ao(0.15, 0.35);
    m
}

fn driftwood(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let len = mr.range(3.2, 4.8);
    let r = mr.range(0.18, 0.26);
    let k = if full { 12 } else { 3 };
    let path: Vec<V3> = (0..=k)
        .map(|i| {
            let t = i as f32 / k as f32;
            let x = lerp(-len * 0.5, len * 0.5, t);
            v3(x, 0.0, 0.25 * noise3(v3(t * 2.5, 1.0, 0.0), seed))
        })
        .collect();
    let rad = |t: f32| -> f32 {
        let body = 0.45 + 0.55 * (PI * (0.08 + 0.84 * t)).sin().powf(0.6);
        let root = 1.0 + 0.9 * (1.0 - t * 6.0).max(0.0).powi(2);
        r * body * root
    };
    // rest on the ground: lift each ring by its radius
    let path: Vec<V3> = path
        .iter()
        .enumerate()
        .map(|(i, p)| p.with_y(rad(i as f32 / k as f32) * 0.8))
        .collect();
    let tw = mr.range(2.0, 4.0);
    let mut w = tube_fn(
        &path,
        if full { 9 } else { 5 },
        Cap::Flat,
        Cap::Flat,
        |i, a| {
            let t = i as f32 / k as f32;
            let lobes = if full {
                1.0 + 0.18 * ((3.0 * a + t * tw).sin()) * (1.0 - t * 5.0).max(0.0)
            } else {
                1.0
            };
            let bump = if full {
                1.0 + 0.08 * noise3(v3(a.cos() * 2.0, t * 6.0, a.sin() * 2.0), seed)
            } else {
                1.0
            };
            rad(t) * lobes * bump
        },
    );
    {
        // root stubs at the -X end, broken branch stub on top
        let sides = if full { 5 } else { 3 };
        for j in 0..3 {
            let a = j as f32 * 2.2 + mr.sym(0.4);
            let d = v3(-0.4, a.sin() * 0.8 + 0.3, a.cos()).norm();
            let base = path[0] + V3::X * 0.15;
            let rr = r * mr.range(0.35, 0.5);
            let l2 = mr.range(0.3, 0.6);
            w.append(&tube(
                &[
                    base,
                    base + d * (r * 1.6),
                    base + d * (r * 1.6 + l2) - V3::Y * 0.1,
                ],
                &[rr, rr * 0.7, 0.0],
                sides,
                Cap::Open,
                Cap::Open,
            ));
        }
        let t = mr.range(0.45, 0.7);
        let c = path[(t * k as f32) as usize];
        let d = v3(mr.sym(0.5), 1.0, mr.sym(0.6)).norm();
        let rb = r * 0.35;
        w.append(&tube(
            &[c, c + d * (r + 0.35)],
            &[rb, rb * 0.8],
            sides,
            Cap::Open,
            Cap::Flat,
        ));
    }
    let silver = srgb(0.52, 0.50, 0.47);
    let groove = srgb(0.30, 0.28, 0.26);
    let warm = srgb(0.52, 0.45, 0.38);
    w.paint(|p, n, uv| {
        if uv[1] < -0.5 || uv[1] > 1.5 {
            return rgba(cmix(warm, groove, uv[0] * 0.5), 0.0);
        }
        let g = noise3(v3(p.x * 0.7, p.y * 9.0, p.z * 9.0), seed);
        let mut c = cmix(silver, groove, smoothstep(-0.1, 0.5, g) * 0.85);
        c = cmix(c, warm, 0.25 * (0.5 + 0.5 * noise3(p * 0.8, seed ^ 3)));
        c = cscale(c, 0.8 + 0.25 * n.y.max(0.0));
        rgba(c, 0.0)
    });
    w.ground_ao(0.2, 0.35);
    w
}

// =====================================================================================
// Structures / props
// =====================================================================================

/// Non-wrapping grid of rows (columns increase in the `V3::polar` angle direction,
/// rows progress along the profile) with smooth normals.
fn grid_mesh(rows: &[Vec<V3>]) -> Mesh {
    let mut m = Mesh::new();
    let cols = rows[0].len();
    let nr = rows.len();
    for (k, row) in rows.iter().enumerate() {
        for (j, p) in row.iter().enumerate() {
            m.vert(
                *p,
                V3::Y,
                [
                    j as f32 / (cols - 1).max(1) as f32,
                    k as f32 / (nr - 1).max(1) as f32,
                ],
            );
        }
    }
    for k in 0..nr - 1 {
        let a0 = (k * cols) as u32;
        let b0 = ((k + 1) * cols) as u32;
        for j in 0..cols as u32 - 1 {
            m.tri(a0 + j, a0 + j + 1, b0 + j + 1);
            m.tri(a0 + j, b0 + j + 1, b0 + j);
        }
    }
    m.smooth_normals();
    m
}

/// Closed slab between z0 < z1 whose top / bottom follow `top(x)` / `bot(x)` sampled at
/// `xs`. Faceted per segment; uv = (segment index, face id: 0 top, 1 bottom, 2 side, 3 end).
fn slab(xs: &[f32], top: &dyn Fn(f32) -> f32, bot: &dyn Fn(f32) -> f32, z0: f32, z1: f32) -> Mesh {
    let mut m = Mesh::new();
    let n = xs.len();
    let q = |m: &mut Mesh, pts: [V3; 4], seg: usize, face: f32| {
        let s = m.vn();
        for p in pts {
            m.vert(p, V3::Y, [seg as f32, face]);
        }
        m.quad(s, s + 1, s + 2, s + 3);
    };
    for k in 0..n - 1 {
        let (xa, xb) = (xs[k], xs[k + 1]);
        let (ta, tb, ba, bb) = (top(xa), top(xb), bot(xa), bot(xb));
        q(
            &mut m,
            [
                v3(xa, ta, z0),
                v3(xa, ta, z1),
                v3(xb, tb, z1),
                v3(xb, tb, z0),
            ],
            k,
            0.0,
        );
        q(
            &mut m,
            [
                v3(xa, ba, z0),
                v3(xb, bb, z0),
                v3(xb, bb, z1),
                v3(xa, ba, z1),
            ],
            k,
            1.0,
        );
        q(
            &mut m,
            [
                v3(xa, ba, z1),
                v3(xb, bb, z1),
                v3(xb, tb, z1),
                v3(xa, ta, z1),
            ],
            k,
            2.0,
        );
        q(
            &mut m,
            [
                v3(xa, ba, z0),
                v3(xa, ta, z0),
                v3(xb, tb, z0),
                v3(xb, bb, z0),
            ],
            k,
            2.0,
        );
    }
    let (x0, x1) = (xs[0], xs[n - 1]);
    q(
        &mut m,
        [
            v3(x0, bot(x0), z0),
            v3(x0, bot(x0), z1),
            v3(x0, top(x0), z1),
            v3(x0, top(x0), z0),
        ],
        0,
        3.0,
    );
    q(
        &mut m,
        [
            v3(x1, bot(x1), z0),
            v3(x1, top(x1), z0),
            v3(x1, top(x1), z1),
            v3(x1, bot(x1), z1),
        ],
        n - 2,
        3.0,
    );
    m.smooth_normals();
    m
}

/// Box whose corners are jittered (weathered stone), flat shaded.
fn rough_box(center: V3, half: V3, rot: &M3, amt: f32, seed: u32) -> Mesh {
    let mut c = [V3::ZERO; 8];
    for (k, cc) in c.iter_mut().enumerate() {
        let s = v3(
            if k & 1 != 0 { 1.0 } else { -1.0 },
            if k & 2 != 0 { 1.0 } else { -1.0 },
            if k & 4 != 0 { 1.0 } else { -1.0 },
        );
        let j = v3(
            hashf(k as u32 * 3, seed) - 0.5,
            hashf(k as u32 * 3 + 1, seed) - 0.5,
            hashf(k as u32 * 3 + 2, seed) - 0.5,
        ) * (2.0 * amt);
        *cc = rot.apply(s.cmul(half) + j) + center;
    }
    hexa(c)
}

/// Accumulates painted parts for props: per-part color jitter + soft per-vertex noise.
struct Kit {
    m: Mesh,
    rng: Rng,
    seed: u32,
}

impl Kit {
    fn new(rng: &mut Rng) -> Kit {
        let mut r = rng.fork();
        let seed = r.seed();
        Kit {
            m: Mesh::new(),
            rng: r,
            seed,
        }
    }
    fn add(&mut self, mut part: Mesh, col: C3, var: f32) {
        let k = 1.0 + self.rng.sym(var);
        let t = self.rng.sym(1.0);
        let c = cscale(ctint(col, t, var * 0.6), k);
        let s = self.seed;
        part.paint(|p, n, _| {
            let nz = noise3(p * 2.3, s);
            rgba(cscale(c, (1.0 + 0.07 * nz) * (0.93 + 0.07 * n.y)), 0.0)
        });
        self.m.append(&part);
    }
    fn finish(mut self, ao_h: f32, ao: f32) -> Mesh {
        self.m.ground_ao(ao_h, ao);
        self.m
    }
}

fn wood_col(rng: &mut Rng) -> (C3, C3, C3) {
    // (planks, beams, posts)
    let opts = [
        (
            srgb(0.56, 0.42, 0.28),
            srgb(0.40, 0.29, 0.19),
            srgb(0.33, 0.24, 0.17),
        ),
        (
            srgb(0.55, 0.50, 0.43),
            srgb(0.42, 0.37, 0.31),
            srgb(0.33, 0.29, 0.25),
        ),
        (
            srgb(0.60, 0.45, 0.30),
            srgb(0.44, 0.31, 0.20),
            srgb(0.36, 0.26, 0.18),
        ),
    ];
    rng.pick(&opts)
}

/// Darken + green tint below the water line / near wet ground.
fn wet_below(m: &mut Mesh, y: f32) {
    let algae = srgb(0.22, 0.27, 0.16);
    for (c, p) in m.c.iter_mut().zip(&m.p) {
        let t = 1.0 - smoothstep(y - 0.6, y + 0.15, p.y);
        if t > 0.0 {
            let cc = cmix([c[0], c[1], c[2]], cscale(algae, 0.7), t * 0.6);
            *c = rgba(cscale(cc, 1.0 - 0.35 * t), c[3]);
        }
    }
}

fn wooden_bridge(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let (plank, beam, post) = wood_col(&mut mr);
    let mut k = Kit::new(&mut mr);
    let half = 12.0f32;
    let hw = 2.0f32;
    let deck = |x: f32| 0.3 + 1.5 * (1.0 - (x / half).powi(2));
    let slope = |x: f32| -3.0 * x / (half * half);
    let n_seg = if full { 12 } else { 6 };
    let xs: Vec<f32> = (0..=n_seg)
        .map(|i| lerp(-half, half, i as f32 / n_seg as f32))
        .collect();
    if full {
        // planks across Z
        let mut x = -half;
        while x < half - 0.05 {
            let dx = 0.33 / (1.0 + slope(x).powi(2)).sqrt();
            let xc = (x + dx * 0.5).min(half - 0.15);
            let ang = slope(xc).atan();
            let rot = M3::rot_y(mr.sym(0.012)).then(&M3::rot_z(ang));
            let c = v3(xc, deck(xc) - 0.04, mr.sym(0.05));
            k.add(
                box_at(c, v3(0.15, 0.04, hw + mr.sym(0.06)), &rot),
                plank,
                0.14,
            );
            x += dx;
        }
        // stringers
        for z in [-1.5f32, 0.0, 1.5] {
            for w in xs.windows(2) {
                let a = v3(w[0], deck(w[0]) - 0.23, z);
                let b = v3(w[1], deck(w[1]) - 0.23, z);
                k.add(box_between(a, b, 0.22, 0.3, V3::Y), beam, 0.06);
            }
        }
    } else {
        for w in xs.windows(2) {
            let a = v3(w[0], deck(w[0]) - 0.1, 0.0);
            let b = v3(w[1], deck(w[1]) - 0.1, 0.0);
            k.add(box_between(a, b, hw * 2.0, 0.2, V3::Y), plank, 0.05);
        }
    }
    // railings
    let step = if full { 2.0 } else { 6.0 };
    let n_posts = (2.0 * half / step).round() as usize;
    for side in [-1.0f32, 1.0] {
        let z = side * (hw - 0.08);
        for i in 0..=n_posts {
            let x = (-half + i as f32 * step).clamp(-half + 0.1, half - 0.1);
            let y = deck(x);
            k.add(
                box_at(
                    v3(x, y + 0.35, z),
                    v3(0.075, 0.72, 0.075),
                    &M3::rot_z(mr.sym(0.02)),
                ),
                post,
                0.08,
            );
        }
        let rails: &[f32] = if full { &[1.02, 0.55] } else { &[1.02] };
        for &h in rails {
            for i in 0..n_posts {
                let xa = (-half + i as f32 * step).clamp(-half + 0.1, half - 0.1);
                let xb = (-half + (i + 1) as f32 * step).clamp(-half + 0.1, half - 0.1);
                let a = v3(xa - 0.05, deck(xa) + h, z);
                let b = v3(xb + 0.05, deck(xb) + h, z);
                let (w, hh) = if h > 1.0 { (0.11, 0.1) } else { (0.07, 0.08) };
                k.add(box_between(a, b, w, hh, V3::Y), beam, 0.08);
            }
        }
    }
    // support bents reaching into the riverbed
    for xb in [-6.0f32, 0.0, 6.0] {
        let cap_y = deck(xb) - 0.38 - 0.13;
        k.add(
            box_at(v3(xb, cap_y, 0.0), v3(0.15, 0.13, hw + 0.15), &M3::I),
            beam,
            0.06,
        );
        for z in [-1.55f32, 1.55] {
            let pile = tube(
                &[v3(xb, -4.0, z), v3(xb, cap_y - 0.1, z)],
                &[0.16, 0.15],
                if full { 7 } else { 4 },
                Cap::Open,
                Cap::Flat,
            );
            k.add(pile, post, 0.08);
        }
        if full {
            let lo = -0.8;
            let hi = cap_y - 0.25;
            k.add(
                box_between(
                    v3(xb + 0.18, lo, -1.55),
                    v3(xb + 0.18, hi, 1.55),
                    0.16,
                    0.06,
                    V3::X,
                ),
                beam,
                0.08,
            );
            k.add(
                box_between(
                    v3(xb - 0.18, hi, -1.55),
                    v3(xb - 0.18, lo, 1.55),
                    0.16,
                    0.06,
                    V3::X,
                ),
                beam,
                0.08,
            );
        }
    }
    // sill logs where the bridge rests on the banks
    for side in [-1.0f32, 1.0] {
        let x = side * (half - 0.35);
        let sill = tube(
            &[v3(x, 0.08, -hw - 0.3), v3(x, 0.08, hw + 0.3)],
            &[0.2, 0.2],
            if full { 7 } else { 4 },
            Cap::Flat,
            Cap::Flat,
        );
        k.add(sill, post, 0.06);
    }
    let mut m = k.finish(0.4, 0.3);
    wet_below(&mut m, -0.3);
    m
}

fn stone_bridge(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let pals = [
        (srgb(0.54, 0.52, 0.48), srgb(0.40, 0.38, 0.35)),
        (srgb(0.58, 0.52, 0.44), srgb(0.44, 0.38, 0.32)),
        (srgb(0.50, 0.51, 0.52), srgb(0.37, 0.38, 0.40)),
    ];
    let (stone, stone_dark) = mr.pick(&pals);
    let mortar = srgb(0.30, 0.29, 0.27);
    let seed = mr.seed();
    let half = 15.0f32;
    let hw = 2.5f32;
    let a = 7.0f32; // half span of the arch
    let ys = -1.2f32; // springing line
    let rise = 3.5f32;
    let rr = (a * a + rise * rise) / (2.0 * rise);
    let yc = ys + rise - rr;
    let deck = |x: f32| 0.3 + 3.25 * (1.0 - (x / half).powi(2)).max(0.0).powf(1.25);
    let intrados = |x: f32| {
        if x.abs() < a {
            yc + (rr * rr - x * x).max(0.0).sqrt()
        } else {
            ys
        }
    };
    let ground = -2.5f32;
    let mut m = Mesh::new();
    let mut rng_c = mr.fork();
    let block_col = |rng: &mut Rng| {
        cscale(
            ctint(cmix(stone, stone_dark, rng.f() * 0.7), rng.sym(1.0), 0.12),
            rng.range(0.88, 1.1),
        )
    };
    // ---------------------------------------------------------------- body
    let arch_n = if full { 22 } else { 10 };
    let abut_n = if full { 7 } else { 2 };
    let left: Vec<f32> = (0..=abut_n)
        .map(|i| lerp(-half, -a, i as f32 / abut_n as f32))
        .collect();
    let right: Vec<f32> = (0..=abut_n)
        .map(|i| lerp(a, half, i as f32 / abut_n as f32))
        .collect();
    let mid: Vec<f32> = (0..=arch_n)
        .map(|i| a * (-(PI * 0.5) + PI * i as f32 / arch_n as f32).sin())
        .collect();
    let gnd = move |_x: f32| ground;
    let inset = if full { 0.03 } else { 0.0 };
    for (xs, bot) in [
        (&left, &gnd as &dyn Fn(f32) -> f32),
        (&right, &gnd as &dyn Fn(f32) -> f32),
        (&mid, &intrados as &dyn Fn(f32) -> f32),
    ] {
        let mut b = slab(xs, &deck, bot, -hw + inset, hw - inset);
        let cols: Vec<C3> = (0..xs.len()).map(|_| block_col(&mut rng_c)).collect();
        let is_mid = xs[0] > -a - 0.01 && xs[0] < -a + 0.01;
        b.paint(|p, _, uv| {
            let face = uv[1] as i32;
            let seg = uv[0] as usize;
            let c = match face {
                0 => cscale(
                    cmix(stone_dark, stone, 0.5 + 0.4 * noise3(p * 1.7, seed)),
                    0.92,
                ),
                1 if is_mid => cols[seg],
                2 if full => mortar,
                _ => cmix(stone, stone_dark, 0.5 + 0.5 * noise3(p * 0.9, seed ^ 4)),
            };
            rgba(c, 0.0)
        });
        m.append(&b);
    }
    if full {
        // vault courses: split the arch underside into staggered stones
        for k in 0..mid.len() - 1 {
            let (xa, xb) = (mid[k], mid[k + 1]);
            let mut z = -hw + inset + if k % 2 == 0 { 0.0 } else { -0.6 };
            while z < hw - inset {
                let z0 = z.max(-hw + inset);
                let z1 = (z + mr.range(1.0, 1.6)).min(hw - inset);
                z = z1;
                if z1 - z0 < 0.2 {
                    continue;
                }
                let e = 0.012;
                let pa = |x: f32, zz: f32| v3(x, intrados(x) - e, zz);
                let mut q = Mesh::new();
                let s = q.vn();
                q.vert(pa(xa, z0 + 0.02), V3::Y, [0.0; 2]);
                q.vert(pa(xb, z0 + 0.02), V3::Y, [0.0; 2]);
                q.vert(pa(xb, z1 - 0.02), V3::Y, [0.0; 2]);
                q.vert(pa(xa, z1 - 0.02), V3::Y, [0.0; 2]);
                q.quad(s, s + 1, s + 2, s + 3);
                q.smooth_normals();
                let c = block_col(&mut rng_c);
                q.paint(|_, _, _| rgba(cscale(c, 0.9), 0.0));
                m.append(&q);
            }
        }
        // ashlar block faces on both sides of the body
        let vd = 0.85f32;
        for side in [-1.0f32, 1.0] {
            let zf = side * (hw - inset + 0.012);
            let mut y = -0.75f32;
            let mut row = 0;
            while y < 3.6 {
                let h = mr.range(0.42, 0.55);
                let (y0, y1) = (y, y + h);
                let mut x = -half - if row % 2 == 0 { 0.0 } else { 0.55 };
                while x < half {
                    let bl = mr.range(0.8, 1.35);
                    let (x0, x1) = (x.max(-half), (x + bl).min(half));
                    x += bl;
                    if x1 - x0 < 0.25 {
                        continue;
                    }
                    let g = 0.03;
                    let (bx0, bx1) = (x0 + g, x1 - g);
                    // skip the voussoir ring
                    let near_arch = |xx: f32, yy: f32| {
                        ((xx * xx + (yy - yc).powi(2)).sqrt() < rr + vd + 0.05) && yy > ys - 0.3
                    };
                    if near_arch(bx0, y0)
                        || near_arch(bx1, y0)
                        || near_arch(bx0, y1)
                        || near_arch(bx1, y1)
                        || near_arch((bx0 + bx1) * 0.5, (y0 + y1) * 0.5)
                    {
                        continue;
                    }
                    let clampy = |xx: f32, yy: f32| -> f32 {
                        let lo = if xx.abs() < a { intrados(xx) } else { ground };
                        yy.clamp(lo + g, deck(xx) - g)
                    };
                    let (l0, l1) = (clampy(bx0, y0 + g), clampy(bx0, y1 - g));
                    let (r0, r1) = (clampy(bx1, y0 + g), clampy(bx1, y1 - g));
                    if (l1 - l0) < 0.1 && (r1 - r0) < 0.1 {
                        continue;
                    }
                    let mut q = Mesh::new();
                    let s = q.vn();
                    let pts = [
                        v3(bx0, l0, zf),
                        v3(bx1, r0, zf),
                        v3(bx1, r1, zf),
                        v3(bx0, l1, zf),
                    ];
                    for p in pts {
                        q.vert(p, V3::Z * side, [0.0; 2]);
                    }
                    if side > 0.0 {
                        q.quad(s, s + 1, s + 2, s + 3)
                    } else {
                        q.quad(s, s + 3, s + 2, s + 1)
                    }
                    let c = block_col(&mut rng_c);
                    q.paint(|p, _, _| rgba(cscale(c, 1.0 + 0.05 * noise3(p * 3.0, seed)), 0.0));
                    m.append(&q);
                }
                y = y1;
                row += 1;
            }
            // voussoirs (arch ring stones), odd count so a keystone sits on top
            let nv = 21;
            let phi0 = (a / rr).asin();
            for v in 0..nv {
                let f0 = -phi0 + 2.0 * phi0 * v as f32 / nv as f32;
                let f1 = -phi0 + 2.0 * phi0 * (v + 1) as f32 / nv as f32;
                let key = v == nv / 2;
                let outer = rr
                    + vd
                    + if key {
                        0.15
                    } else if v % 2 == 0 {
                        0.05
                    } else {
                        -0.05
                    };
                let (za, zb) = if side > 0.0 {
                    (hw - 0.3, hw + 0.05)
                } else {
                    (-hw - 0.05, -hw + 0.3)
                };
                let mut c = [V3::ZERO; 8];
                for (q, cc) in c.iter_mut().enumerate() {
                    let f = if q & 1 != 0 { f1 - 0.006 } else { f0 + 0.006 };
                    let r = if q & 2 != 0 { outer } else { rr - 0.015 };
                    let z = if q & 4 != 0 { zb } else { za };
                    *cc = v3(r * f.sin(), yc + r * f.cos(), z);
                }
                let col = cscale(block_col(&mut rng_c), 1.05);
                let mut vb = hexa(c);
                vb.paint(|p, _, _| rgba(cscale(col, 1.0 + 0.05 * noise3(p * 3.0, seed)), 0.0));
                m.append(&vb);
            }
        }
    }
    // ---------------------------------------------------------------- parapets
    let pn = if full { 26 } else { 8 };
    let pxs: Vec<f32> = (0..=pn)
        .map(|i| lerp(-half + 0.35, half - 0.35, i as f32 / pn as f32))
        .collect();
    for side in [-1.0f32, 1.0] {
        let (z0, z1) = if side > 0.0 {
            (hw - 0.42, hw + 0.05)
        } else {
            (-hw - 0.05, -hw + 0.42)
        };
        let ptop = |x: f32| deck(x) + 0.85;
        let pbot = |x: f32| deck(x) - 0.12;
        let mut p = slab(&pxs, &ptop, &pbot, z0, z1);
        let cols: Vec<C3> = (0..pxs.len()).map(|_| block_col(&mut rng_c)).collect();
        p.paint(|q, _, uv| {
            rgba(
                cscale(cols[uv[0] as usize], 1.0 + 0.04 * noise3(q * 2.0, seed)),
                0.0,
            )
        });
        m.append(&p);
        if full {
            let ctop = |x: f32| deck(x) + 1.0;
            let cbot = |x: f32| deck(x) + 0.85;
            let (c0, c1) = if side > 0.0 {
                (hw - 0.5, hw + 0.1)
            } else {
                (-hw - 0.1, -hw + 0.5)
            };
            let mut cp = slab(&pxs, &ctop, &cbot, c0, c1);
            let cols2: Vec<C3> = (0..pxs.len())
                .map(|_| cscale(block_col(&mut rng_c), 1.1))
                .collect();
            cp.paint(|_, _, uv| rgba(cols2[uv[0] as usize], 0.0));
            m.append(&cp);
        }
        // end posts
        for ex in [-1.0f32, 1.0] {
            let x = ex * (half - 0.35);
            let c = v3(x, deck(x) + 0.55, side * (hw - 0.2));
            let mut b = if full {
                rough_box(
                    c,
                    v3(0.35, 0.8, 0.35),
                    &M3::I,
                    0.03,
                    seed ^ (x as i32 as u32),
                )
            } else {
                box_at(c, v3(0.35, 0.8, 0.35), &M3::I)
            };
            let col = block_col(&mut rng_c);
            b.paint(|_, _, _| rgba(col, 0.0));
            m.append(&b);
        }
    }
    // damp, mossy lower stones
    let moss = srgb(0.30, 0.36, 0.18);
    for (c, p) in m.c.iter_mut().zip(&m.p) {
        let t = 1.0 - smoothstep(-1.0, 0.8, p.y);
        let cc = cmix(
            [c[0], c[1], c[2]],
            moss,
            t * 0.45 * (0.6 + 0.4 * noise3(*p * 0.8, seed)),
        );
        *c = rgba(cscale(cc, 1.0 - 0.25 * t), 0.0);
    }
    m
}

fn hut(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let hx = mr.range(2.8, 3.2);
    let hz = mr.range(2.3, 2.6);
    let wh = mr.range(2.5, 2.8);
    let pitch = mr.range(0.68, 0.78);
    let ridge = wh + (hz + 0.1) * pitch.tan();
    let seed = mr.seed();
    let (_, timber, _) = wood_col(&mut mr);
    let timber = cscale(timber, 0.75);
    let plaster = mr.pick(&[
        srgb(0.86, 0.81, 0.70),
        srgb(0.80, 0.76, 0.68),
        srgb(0.88, 0.84, 0.76),
    ]);
    let stone = srgb(0.56, 0.54, 0.50);
    let roofs = [
        srgb(0.70, 0.58, 0.34),
        srgb(0.46, 0.30, 0.22),
        srgb(0.38, 0.40, 0.44),
    ];
    let roof_c = mr.pick(&roofs);
    let shutter_c = mr.pick(&[
        srgb(0.30, 0.42, 0.36),
        srgb(0.55, 0.30, 0.22),
        srgb(0.34, 0.40, 0.52),
    ]);
    let door_c = srgb(0.40, 0.27, 0.17);
    let glass = srgb(0.10, 0.12, 0.14);
    let mut k = Kit::new(&mut mr);
    let base_top = 0.45;
    // stone plinth
    if full {
        for (axis, len, off) in [
            (0usize, hx + 0.15, hz + 0.05),
            (0, hx + 0.15, -(hz + 0.05)),
            (1, hz - 0.2, hx + 0.05),
            (1, hz - 0.2, -(hx + 0.05)),
        ] {
            let mut t = -len;
            while t < len - 0.05 {
                let bl = mr.range(0.5, 0.9).min(len - t);
                let c = t + bl * 0.5;
                let (cx, cz) = if axis == 0 { (c, off) } else { (off, c) };
                let half = if axis == 0 {
                    v3(bl * 0.5 - 0.02, 0.36, 0.24)
                } else {
                    v3(0.24, 0.36, bl * 0.5 - 0.02)
                };
                let s = seed ^ ((cx * 31.0 + cz * 7.0) as i32 as u32);
                k.add(
                    rough_box(
                        v3(cx, base_top - 0.36, cz),
                        half,
                        &M3::rot_y(mr.sym(0.03)),
                        0.025,
                        s,
                    ),
                    stone,
                    0.14,
                );
                t += bl;
            }
        }
    } else {
        k.add(
            box_at(
                v3(0.0, base_top - 0.36, 0.0),
                v3(hx + 0.15, 0.36, hz + 0.3),
                &M3::I,
            ),
            stone,
            0.0,
        );
    }
    // walls
    let wy = (base_top + wh) * 0.5;
    let whh = (wh - base_top) * 0.5;
    k.add(
        box_at(v3(0.0, wy, 0.0), v3(hx - 0.05, whh, hz - 0.05), &M3::I),
        plaster,
        0.03,
    );
    // timber frame (protrudes 4 cm)
    let tb = 0.11;
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        k.add(
            box_at(
                v3(sx * (hx - 0.05), wy, sz * (hz - 0.05)),
                v3(tb + 0.02, whh, tb + 0.02),
                &M3::I,
            ),
            timber,
            0.06,
        );
    }
    for sz in [-1.0f32, 1.0] {
        for y in [base_top + 0.06, wh - 0.06] {
            k.add(
                box_at(v3(0.0, y, sz * (hz - 0.01)), v3(hx, 0.07, 0.06), &M3::I),
                timber,
                0.06,
            );
        }
        if full {
            k.add(
                box_at(v3(0.0, 1.55, sz * (hz - 0.01)), v3(hx, 0.05, 0.05), &M3::I),
                timber,
                0.06,
            );
        }
    }
    for sx in [-1.0f32, 1.0] {
        for y in [base_top + 0.06, wh - 0.06] {
            k.add(
                box_at(v3(sx * (hx - 0.01), y, 0.0), v3(0.06, 0.07, hz), &M3::I),
                timber,
                0.06,
            );
        }
    }
    if full {
        // diagonal braces on the back wall
        for sx in [-1.0f32, 1.0] {
            let a = v3(sx * (hx - 0.25), base_top + 0.1, -(hz + 0.0));
            let b = v3(sx * (hx - 1.2), 1.5, -(hz + 0.0));
            k.add(box_between(a, b, 0.12, 0.08, V3::Z), timber, 0.06);
        }
    }
    // door (front, +Z)
    let dx = -hx * 0.35;
    let fz = hz - 0.05;
    if full {
        for i in 0..3 {
            let x = dx - 0.3 + 0.3 * i as f32;
            k.add(
                box_at(
                    v3(x, base_top + 0.95, fz + 0.03),
                    v3(0.145, 0.95, 0.04),
                    &M3::I,
                ),
                door_c,
                0.1,
            );
        }
        k.add(
            box_at(
                v3(dx, base_top + 0.5, fz + 0.08),
                v3(0.42, 0.04, 0.02),
                &M3::I,
            ),
            cscale(door_c, 0.7),
            0.0,
        );
        k.add(
            box_at(
                v3(dx, base_top + 1.4, fz + 0.08),
                v3(0.42, 0.04, 0.02),
                &M3::I,
            ),
            cscale(door_c, 0.7),
            0.0,
        );
        for sx in [-1.0f32, 1.0] {
            k.add(
                box_at(
                    v3(dx + sx * 0.55, base_top + 1.0, fz + 0.04),
                    v3(0.07, 1.0, 0.07),
                    &M3::I,
                ),
                timber,
                0.05,
            );
        }
        k.add(
            box_at(
                v3(dx, base_top + 2.03, fz + 0.04),
                v3(0.65, 0.07, 0.08),
                &M3::I,
            ),
            timber,
            0.05,
        );
        k.add(
            rough_box(
                v3(dx, 0.12, fz + 0.55),
                v3(0.55, 0.14, 0.3),
                &M3::I,
                0.03,
                seed ^ 5,
            ),
            stone,
            0.1,
        );
    } else {
        k.add(
            box_at(
                v3(dx, base_top + 0.95, fz + 0.03),
                v3(0.5, 0.95, 0.04),
                &M3::I,
            ),
            door_c,
            0.0,
        );
    }
    // windows: (center, outward normal)
    let wins = [
        (v3(hx * 0.4, 1.55, fz), V3::Z),
        (v3(0.0, 1.55, -fz), -V3::Z),
        (v3(hx - 0.05, 1.55, 0.0), V3::X),
        (v3(-(hx - 0.05), 1.55, 0.0), -V3::X),
    ];
    for (c, n) in wins {
        let side = V3::Y.cross(n); // along the wall
        let rot = M3 {
            x: side,
            y: V3::Y,
            z: n,
        };
        let at = |u: f32, v: f32, w: f32| c + side * u + V3::Y * v + n * w;
        k.add(
            box_at(at(0.0, 0.0, 0.02), v3(0.36, 0.42, 0.03), &rot),
            glass,
            0.0,
        );
        if full {
            for s in [-1.0f32, 1.0] {
                k.add(
                    box_at(at(s * 0.42, 0.0, 0.05), v3(0.06, 0.5, 0.05), &rot),
                    timber,
                    0.05,
                );
                k.add(
                    box_at(at(0.0, s * 0.48, 0.05), v3(0.48, 0.06, 0.05), &rot),
                    timber,
                    0.05,
                );
                k.add(
                    box_at(at(s * 0.78, 0.0, 0.06), v3(0.3, 0.46, 0.03), &rot),
                    shutter_c,
                    0.08,
                );
            }
            k.add(
                box_at(at(0.0, 0.0, 0.05), v3(0.025, 0.42, 0.025), &rot),
                timber,
                0.0,
            );
            k.add(
                box_at(at(0.0, 0.0, 0.05), v3(0.36, 0.025, 0.025), &rot),
                timber,
                0.0,
            );
            k.add(
                box_at(at(0.0, -0.56, 0.1), v3(0.55, 0.04, 0.1), &rot),
                timber,
                0.05,
            );
        }
    }
    // gables
    for sx in [-1.0f32, 1.0] {
        let poly = [(-hz + 0.05, wh), (hz - 0.05, wh), (0.0, ridge - 0.08)];
        let g = prism(&poly, 0.22).xf(&M3::rot_y(PI * 0.5), v3(sx * (hx - 0.05), 0.0, 0.0));
        k.add(g, plaster, 0.03);
        if full {
            k.add(
                box_at(
                    v3(sx * (hx + 0.0), (wh + ridge) * 0.5 - 0.05, 0.0),
                    v3(0.07, (ridge - wh) * 0.5 - 0.05, 0.07),
                    &M3::I,
                ),
                timber,
                0.05,
            );
        }
    }
    // roof: deck slab + overlapping shingle / thatch rows
    let over = 0.45;
    let len = hx + 0.45;
    for sz in [-1.0f32, 1.0] {
        let top = v3(0.0, ridge + 0.05, 0.0);
        let dir = v3(0.0, -pitch.sin(), sz * pitch.cos());
        let run = (hz + over) / pitch.cos();
        let eave = top + dir * run;
        k.add(
            box_between(top, eave, len * 2.0, 0.12, V3::Y),
            cscale(roof_c, 0.75),
            0.03,
        );
        let rows = if full { 6 } else { 0 };
        for r in 0..rows {
            let t0 = r as f32 / rows as f32;
            let t1 = (r as f32 + 1.25) / rows as f32;
            let a = top + dir * (run * t0) + V3::Y * 0.08;
            let b = top + dir * (run * t1.min(1.04)) + V3::Y * 0.02;
            let thick = if roof_c[0] > 0.4 { 0.16 } else { 0.07 };
            k.add(
                box_between(a, b, len * 2.0 + mr.sym(0.06), thick, V3::Y),
                roof_c,
                0.1,
            );
        }
    }
    k.add(
        box_at(
            v3(0.0, ridge + 0.12, 0.0),
            v3(len + 0.02, 0.1, 0.14),
            &M3::rot_x(PI * 0.25),
        ),
        cscale(roof_c, 0.8),
        0.05,
    );
    // chimney
    let cxp = hx * 0.45;
    let czp = -hz * 0.4;
    let ctop = ridge + 0.7;
    if full {
        k.add(
            rough_box(
                v3(cxp, (base_top + ctop) * 0.5 - 0.3, czp),
                v3(0.36, (ctop - base_top) * 0.5 - 0.3, 0.36),
                &M3::I,
                0.02,
                seed ^ 9,
            ),
            stone,
            0.08,
        );
        k.add(
            rough_box(
                v3(cxp, ctop - 0.3, czp),
                v3(0.4, 0.3, 0.4),
                &M3::I,
                0.02,
                seed ^ 10,
            ),
            cscale(stone, 0.9),
            0.08,
        );
        k.add(
            box_at(v3(cxp, ctop + 0.05, czp), v3(0.45, 0.06, 0.45), &M3::I),
            cscale(stone, 0.8),
            0.05,
        );
        // firewood stack along the left wall
        let wx = -hx - 0.45;
        for i in 0..6 {
            let row = (i / 3) as f32;
            let zc = -0.5 + (i % 3) as f32 * 0.32 + row * 0.16;
            let y = 0.16 + row * 0.27;
            let mut log = tube(
                &[v3(wx, y, zc) - V3::X * 0.35, v3(wx, y, zc) + V3::X * 0.35],
                &[0.14, 0.14],
                6,
                Cap::Flat,
                Cap::Flat,
            );
            log.paint(|_, _, uv| {
                if uv[1] < -0.5 || uv[1] > 1.5 {
                    rgba(srgb(0.72, 0.58, 0.40), 0.0)
                } else {
                    rgba(srgb(0.38, 0.28, 0.19), 0.0)
                }
            });
            k.m.append(&log);
        }
    } else {
        k.add(
            box_at(
                v3(cxp, (base_top + ctop) * 0.5, czp),
                v3(0.38, (ctop - base_top) * 0.5, 0.38),
                &M3::I,
            ),
            stone,
            0.0,
        );
    }
    k.finish(0.6, 0.3)
}

fn tent(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let l = mr.range(2.8, 3.3) * 0.5;
    let w = mr.range(1.25, 1.45);
    let h = mr.range(1.7, 1.95);
    let seed = mr.seed();
    let canvas = mr.pick(&[
        srgb(0.84, 0.78, 0.62),
        srgb(0.50, 0.52, 0.38),
        srgb(0.55, 0.60, 0.64),
        srgb(0.72, 0.46, 0.34),
    ]);
    let dirt = srgb(0.40, 0.33, 0.24);
    let pole_c = srgb(0.45, 0.33, 0.22);
    let rope_c = srgb(0.70, 0.62, 0.45);
    let mut m = Mesh::new();
    let paint_canvas = |mm: &mut Mesh| {
        mm.paint(|p, n, _| {
            let d = 1.0 - smoothstep(0.0, 0.45, p.y);
            let seam = if ((p.x + l) / (2.0 * l) * 6.0).fract() < 0.06 {
                0.85
            } else {
                1.0
            };
            let c = cmix(canvas, dirt, d * 0.55);
            rgba(
                cscale(
                    c,
                    seam * (0.93 + 0.08 * noise3(p * 2.5, seed)) * (0.9 + 0.1 * n.y.abs()),
                ),
                0.0,
            )
        });
    };
    // two sloped panels
    let nx = if full { 8 } else { 2 };
    let ny = if full { 3 } else { 1 };
    for side in [-1.0f32, 1.0] {
        let mut rows = Vec::new();
        for j in 0..=ny {
            let s = j as f32 / ny as f32;
            let mut row = Vec::new();
            for i in 0..=nx {
                let t = i as f32 / nx as f32;
                let x = lerp(-l - 0.06, l + 0.06, t);
                let sag_r = 0.07 * (PI * t).sin();
                let ry = h - sag_r;
                let mut p = v3(x, ry * (1.0 - s) + 0.01 * s, side * (w * s + 0.1 * s * s));
                let nrm = v3(0.0, w, side * h).norm();
                p -= nrm * (0.06 * (PI * s).sin() * (PI * t).sin());
                row.push(p);
            }
            rows.push(row);
        }
        if side < 0.0 {
            for r in rows.iter_mut() {
                r.reverse();
            }
        }
        let mut pm = grid_mesh(&rows);
        paint_canvas(&mut pm);
        pm.double_sided(0.45);
        m.append(&pm);
    }
    // back wall
    {
        let mut b = Mesh::new();
        let p0 = b.vert(v3(-l, 0.0, -w), V3::Y, [0.0; 2]);
        let p1 = b.vert(v3(-l, 0.0, w), V3::Y, [0.0; 2]);
        let p2 = b.vert(v3(-l, h - 0.02, 0.0), V3::Y, [0.0; 2]);
        let p3 = b.vert(v3(-l, 0.0, 0.0), V3::Y, [0.0; 2]);
        b.tri(p0, p3, p2);
        b.tri(p3, p1, p2);
        b.smooth_normals();
        paint_canvas(&mut b);
        b.double_sided(0.4);
        m.append(&b);
    }
    // front: open door flaps folded outward
    for side in [-1.0f32, 1.0] {
        let mut f = Mesh::new();
        let apex = f.vert(v3(l, h - 0.03, 0.0), V3::Y, [0.0; 2]);
        let outer = f.vert(v3(l, 0.0, side * w), V3::Y, [0.0; 2]);
        let fold = f.vert(v3(l + 0.5, 0.05, side * w * 0.55), V3::Y, [0.0; 2]);
        if side > 0.0 {
            f.tri(apex, fold, outer)
        } else {
            f.tri(apex, outer, fold)
        }
        f.smooth_normals();
        paint_canvas(&mut f);
        f.double_sided(0.5);
        m.append(&f);
    }
    // ground sheet + bedroll seen through the door
    let mut k = Kit::new(&mut mr);
    k.add(
        box_at(v3(0.0, 0.01, 0.0), v3(l - 0.02, 0.01, w - 0.05), &M3::I),
        srgb(0.30, 0.27, 0.22),
        0.0,
    );
    if full {
        let roll = tube(
            &[v3(-l + 0.3, 0.14, -0.35), v3(l - 0.4, 0.14, -0.35)],
            &[0.14, 0.14],
            7,
            Cap::Flat,
            Cap::Flat,
        );
        k.add(
            roll,
            mr.pick(&[srgb(0.55, 0.18, 0.15), srgb(0.25, 0.32, 0.50)]),
            0.05,
        );
    }
    // poles
    let ps = if full { 5 } else { 3 };
    for ex in [-1.0f32, 1.0] {
        k.add(
            tube(
                &[v3(ex * l, -0.25, 0.0), v3(ex * l, h + 0.14, 0.0)],
                &[0.03, 0.03],
                ps,
                Cap::Open,
                Cap::Flat,
            ),
            pole_c,
            0.05,
        );
    }
    k.add(
        tube(
            &[v3(-l - 0.12, h + 0.01, 0.0), v3(l + 0.12, h + 0.01, 0.0)],
            &[0.028, 0.028],
            ps,
            Cap::Flat,
            Cap::Flat,
        ),
        pole_c,
        0.05,
    );
    {
        // guy ropes and pegs
        let ropes = [
            (v3(l, h + 0.08, 0.0), v3(l + 1.2, 0.0, 0.0)),
            (v3(-l, h + 0.08, 0.0), v3(-l - 1.2, 0.0, 0.0)),
            (v3(l * 0.6, 0.35, w * 0.75), v3(l * 0.7, 0.0, w + 0.75)),
            (v3(-l * 0.6, 0.35, w * 0.75), v3(-l * 0.7, 0.0, w + 0.75)),
            (v3(l * 0.6, 0.35, -w * 0.75), v3(l * 0.7, 0.0, -w - 0.75)),
            (v3(-l * 0.6, 0.35, -w * 0.75), v3(-l * 0.7, 0.0, -w - 0.75)),
        ];
        for (a, b) in ropes {
            k.add(
                box_between(a, b + V3::Y * 0.06, 0.015, 0.015, V3::Y),
                rope_c,
                0.0,
            );
            if full {
                k.add(
                    box_at(b + V3::Y * 0.04, v3(0.025, 0.1, 0.025), &M3::I),
                    pole_c,
                    0.05,
                );
            }
        }
    }
    let props = k.finish(0.2, 0.25);
    m.append(&props);
    m.ground_ao(0.3, 0.25);
    m
}

fn fence(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let (_, beam, post) = wood_col(&mut mr);
    let rails = if mr.chance(0.35) { 3 } else { 2 };
    let mut k = Kit::new(&mut mr);
    let ph = 1.15;
    for (i, x) in [-2.0f32, 0.0, 2.0].iter().enumerate() {
        // end posts are exact so neighbouring sections overlap cleanly
        let tilt = if i == 1 {
            M3::rot_z(mr.sym(0.04)).then(&M3::rot_x(mr.sym(0.04)))
        } else {
            M3::I
        };
        let c = v3(*x, (ph - 0.35) * 0.5, 0.0);
        k.add(
            box_at(c, v3(0.075, (ph + 0.35) * 0.5, 0.075), &tilt),
            post,
            0.1,
        );
        if full {
            let cap = lathe(&[(0.106, ph), (0.0, ph + 0.09)], 4, PI * 0.25).xf(&tilt, V3::ZERO);
            k.add(cap, post, 0.1);
        }
    }
    for r in 0..rails {
        let y = if rails == 2 {
            [0.45, 0.88][r]
        } else {
            [0.3, 0.62, 0.94][r]
        };
        for (x0, x1) in [(-2.06f32, 0.06f32), (-0.06, 2.06)] {
            let sag = mr.range(0.0, 0.03);
            let a = v3(x0, y + mr.sym(0.03), 0.0);
            let b = v3(x1, y + mr.sym(0.03), 0.0);
            let rot = mr.sym(0.25);
            let mut bx = box_between(a, b, 0.07, 0.11, (V3::Y + V3::Z * rot).norm());
            for p in bx.p.iter_mut() {
                let t = ((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0);
                p.y -= sag * (PI * t).sin();
            }
            k.add(bx, beam, 0.12);
        }
    }
    k.finish(0.25, 0.3)
}

fn ruin_pillar(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let stone = mr.pick(&[
        srgb(0.80, 0.77, 0.70),
        srgb(0.80, 0.70, 0.56),
        srgb(0.74, 0.74, 0.73),
    ]);
    let dark = cscale(stone, 0.7);
    let r0 = mr.range(0.42, 0.5);
    let hs = mr.range(2.6, 3.6);
    let drums = mr.int(3, 4) as usize;
    let mut m = Mesh::new();
    let mut k = Kit::new(&mut mr);
    // stepped plinth
    let plinth = |k: &mut Kit, full: bool| {
        if full {
            k.add(
                rough_box(
                    v3(0.0, 0.08, 0.0),
                    v3(0.95, 0.33, 0.95),
                    &M3::rot_y(0.05),
                    0.04,
                    seed,
                ),
                cscale(stone, 0.92),
                0.05,
            );
            k.add(
                rough_box(
                    v3(0.0, 0.53, 0.0),
                    v3(0.75, 0.13, 0.75),
                    &M3::rot_y(-0.03),
                    0.03,
                    seed ^ 1,
                ),
                stone,
                0.05,
            );
        } else {
            k.add(
                box_at(v3(0.0, 0.08, 0.0), v3(0.95, 0.33, 0.95), &M3::I),
                cscale(stone, 0.92),
                0.0,
            );
            k.add(
                box_at(v3(0.0, 0.53, 0.0), v3(0.75, 0.13, 0.75), &M3::I),
                stone,
                0.0,
            );
        }
    };
    plinth(&mut k, full);
    m.append(&k.m);
    let y0 = 0.66;
    let sides = if full { 20 } else { 8 };
    if full {
        // base moulding
        let mut mo = lathe(
            &[
                (0.0, y0),
                (r0 * 1.32, y0),
                (r0 * 1.36, y0 + 0.1),
                (r0 * 1.2, y0 + 0.2),
                (r0 * 1.04, y0 + 0.26),
                (0.0, y0 + 0.26),
            ],
            sides,
            0.0,
        );
        mo.paint(|p, _, _| rgba(cscale(stone, 0.95 + 0.05 * noise3(p * 3.0, seed)), 0.0));
        m.append(&mo);
    }
    // fluted drums
    let yb = y0 + if full { 0.26 } else { 0.0 };
    let rad = |y: f32| r0 * (1.0 - 0.1 * ((y - yb) / hs).clamp(0.0, 1.0));
    let n = if full { drums } else { 1 };
    let offsets: Vec<V3> = (0..drums)
        .map(|_| V3::polar(mr.angle()) * mr.range(0.0, 0.025))
        .collect();
    let mut top_ring = Vec::new();
    for d in 0..n {
        let ya = yb + hs * d as f32 / n as f32;
        let ybb = yb + hs * (d + 1) as f32 / n as f32;
        let top = d + 1 == n;
        let mut prof = if full {
            vec![(0.0, ya), (rad(ya) * 0.96, ya), (rad(ya), ya + 0.03)]
        } else {
            vec![(rad(ya), ya)]
        };
        if top {
            // open top ring, jagged below and closed by a broken cap
            prof.push((rad(ybb), ybb));
        } else {
            prof.extend_from_slice(&[(rad(ybb), ybb - 0.03), (rad(ybb) * 0.96, ybb), (0.0, ybb)]);
        }
        let mut dm = lathe(&prof, sides, d as f32 * 0.1);
        let off = offsets[d];
        for q in 0..dm.p.len() {
            let j = (dm.uv[q][0] * sides as f32).round() as u32;
            if full && j % 2 == 1 {
                let p = dm.p[q];
                dm.p[q] = v3(p.x * 0.93, p.y, p.z * 0.93);
            }
            dm.p[q] += off * (d as f32);
        }
        if top {
            for q in 0..dm.p.len() {
                if dm.uv[q][1] > 0.99 {
                    let a = dm.uv[q][0] * TAU;
                    let drop =
                        0.35 * (0.5 + 0.5 * (a + seed as f32).sin()) + 0.15 * hashf(q as u32, seed);
                    dm.p[q].y -= drop;
                }
            }
            top_ring = (0..dm.p.len())
                .filter(|&q| dm.uv[q][1] > 0.99)
                .map(|q| dm.p[q])
                .collect();
        }
        dm.smooth_normals();
        dm.paint(|p, _, _| {
            let streak = noise3(v3(p.x * 4.0, p.y * 0.6, p.z * 4.0), seed ^ 7);
            let c = cmix(
                stone,
                dark,
                0.15 + 0.35 * smoothstep(0.0, 0.8, streak)
                    + 0.3 * (1.0 - smoothstep(0.6, 1.6, p.y)),
            );
            rgba(c, 0.0)
        });
        m.append(&dm);
    }
    // broken top surface
    if !top_ring.is_empty() {
        let c =
            top_ring.iter().fold(V3::ZERO, |a, b| a + *b) / top_ring.len() as f32 - V3::Y * 0.08;
        let mut cap = Mesh::new();
        let ci = cap.vert(c, V3::Y, [0.0; 2]);
        let s = cap.vn();
        for p in &top_ring {
            cap.vert(*p, V3::Y, [0.0; 2]);
        }
        let nn = top_ring.len() as u32;
        for j in 0..nn {
            cap.tri(ci, s + j, s + (j + 1) % nn);
        }
        cap.unweld(0.0);
        let fresh = cmix(stone, srgb(0.9, 0.88, 0.84), 0.3);
        cap.paint(|p, _, _| rgba(cscale(fresh, 0.92 + 0.08 * noise3(p * 6.0, seed)), 0.0));
        m.append(&cap);
    }
    // fallen drum and rubble
    let fa = mr.angle();
    let fd = V3::polar(fa) * mr.range(1.4, 1.8);
    let fl = mr.range(0.45, 0.7);
    let mut fallen = lathe(
        &[
            (0.0, -fl * 0.5),
            (r0 * 0.95, -fl * 0.5),
            (r0 * 0.95, fl * 0.5),
            (0.0, fl * 0.5),
        ],
        if full { 14 } else { 6 },
        0.0,
    );
    fallen.transform(
        &M3::rot_z(PI * 0.5).then(&M3::rot_y(fa + 0.6)),
        fd + V3::Y * (r0 * 0.95 - 0.06),
    );
    fallen.unweld(0.6);
    fallen.paint(|p, _, _| rgba(cscale(stone, 0.85 + 0.1 * noise3(p * 3.0, seed)), 0.0));
    m.append(&fallen);
    if full {
        for j in 0..3 {
            let a = fa + 1.5 + j as f32 * 1.4;
            let s = mr.range(0.15, 0.3);
            let mut r = placed_rock(
                1,
                v3(s * 1.2, s * 0.7, s),
                0.2,
                5,
                0.1,
                seed ^ (j + 3),
                V3::polar(a) * mr.range(1.0, 1.6),
                a,
                M3::I,
                0.05,
                false,
            );
            paint_rock(&mut r, stone, dark, seed, V3::ZERO, 1.0, 0.0, 0.0);
            m.append(&r);
        }
    }
    // moss on upward faces near the ground
    let moss = srgb(0.32, 0.40, 0.17);
    for k in 0..m.p.len() {
        let p = m.p[k];
        let n = m.n[k];
        let t = smoothstep(0.5, 0.9, n.y + 0.4 * noise3(p * 2.0, seed ^ 11))
            * (1.0 - smoothstep(0.6, 1.4, p.y));
        let c = m.c[k];
        m.c[k] = rgba(cmix([c[0], c[1], c[2]], moss, t * 0.8), 0.0);
    }
    m.ground_ao(0.5, 0.35);
    m
}

fn ruin_wall(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let stone = mr.pick(&[
        srgb(0.62, 0.60, 0.56),
        srgb(0.66, 0.60, 0.50),
        srgb(0.58, 0.58, 0.60),
    ]);
    let dark = cscale(stone, 0.72);
    let half = 3.0f32;
    let th = mr.range(0.36, 0.42);
    let hmax = mr.range(3.3, 3.9);
    let cr_side = if mr.chance(0.5) { 1.0 } else { -1.0 };
    let tseed = mr.seed();
    let top = move |x: f32| {
        hmax - hmax * 0.5 * smoothstep(-1.5, 3.2, x * cr_side)
            + 0.3 * noise3(v3(x * 0.9, 0.0, 0.0), tseed)
    };
    // courses
    let mut courses = Vec::new();
    let mut y = -0.25f32;
    while y < hmax {
        let h = mr.range(0.36, 0.46);
        courses.push((y, y + h));
        y += h;
    }
    let xw = mr.sym(0.5);
    let ww = mr.range(1.15, 1.35);
    let sill_i = courses.iter().position(|c| c.1 > 1.05).unwrap_or(2);
    let sill = courses[sill_i].1;
    let spring = sill + mr.range(0.9, 1.05);
    let ar = ww * 0.5;
    let vd = 0.36;
    let blocked = |y0: f32, y1: f32| -> f32 {
        if y1 <= sill + 0.01 {
            0.0
        } else if y0 < spring {
            if y1 > spring { ar + vd } else { ar }
        } else {
            let dy = y0 - spring;
            if dy < ar + vd {
                ((ar + vd).powi(2) - dy * dy).sqrt()
            } else {
                0.0
            }
        }
    };
    let mut pieces: Vec<(f32, f32, f32, f32, f32, f32, u32)> = Vec::new(); // x0 x1 y0 y1 zoff yaw seed
    let mut runs: Vec<(f32, f32, f32, f32)> = Vec::new();
    for (ci, &(y0, y1)) in courses.iter().enumerate() {
        let frac = y0 / hmax;
        let xl = -half + mr.range(0.0, 0.5) * frac;
        let xr = half - mr.range(0.0, 0.5) * frac;
        let mut x = xl - if ci % 2 == 1 { mr.range(0.2, 0.4) } else { 0.0 };
        let hb = blocked(y0, y1);
        let mut run: Option<(f32, f32)> = None;
        while x < xr {
            let bl = mr.range(0.5, 0.95);
            let (bx0, bx1) = (x.max(xl), (x + bl).min(xr));
            x += bl;
            let zoff = mr.sym(0.03);
            let yaw = mr.sym(0.025);
            let bs = mr.seed();
            let skip_top = y1 > top((bx0 + bx1) * 0.5)
                || (y1 > top((bx0 + bx1) * 0.5) - 0.45 && mr.chance(0.3));
            if bx1 - bx0 < 0.12 || skip_top {
                if let Some(r) = run.take() {
                    runs.push((r.0, r.1, y0, y1));
                }
                continue;
            }
            let mut parts = vec![(bx0, bx1)];
            if hb > 0.0 {
                let (w0, w1) = (xw - hb, xw + hb);
                parts = parts
                    .into_iter()
                    .flat_map(|(a, b)| {
                        let mut v = Vec::new();
                        if a < w0 {
                            v.push((a, b.min(w0)));
                        }
                        if b > w1 {
                            v.push((a.max(w1), b));
                        }
                        v
                    })
                    .filter(|(a, b)| b - a > 0.12)
                    .collect();
            }
            for (a, b) in parts {
                pieces.push((a, b, y0, y1, zoff, yaw, bs));
                match run {
                    Some((r0, r1)) if a - r1 < 0.05 => run = Some((r0, b)),
                    Some(r) => {
                        runs.push((r.0, r.1, y0, y1));
                        run = Some((a, b));
                    }
                    None => run = Some((a, b)),
                }
            }
        }
        if let Some(r) = run.take() {
            runs.push((r.0, r.1, y0, y1));
        }
    }
    let mut k = Kit::new(&mut mr);
    if full {
        for &(x0, x1, y0, y1, zoff, yaw, bs) in &pieces {
            let g = 0.018;
            let c = v3((x0 + x1) * 0.5, (y0 + y1) * 0.5, zoff);
            let hh = v3(
                (x1 - x0) * 0.5 - g,
                (y1 - y0) * 0.5 - g,
                th + hashf(1, bs) * 0.03,
            );
            k.add(
                rough_box(c, hh, &M3::rot_y(yaw), 0.02, bs),
                cmix(stone, dark, hashf(2, bs) * 0.5),
                0.08,
            );
        }
        // mortar core so gaps read dark, not see-through
        for &(x0, x1, y0, y1) in &runs {
            k.add(
                box_at(
                    v3((x0 + x1) * 0.5, (y0 + y1) * 0.5, 0.0),
                    v3((x1 - x0) * 0.5 - 0.02, (y1 - y0) * 0.5, th - 0.04),
                    &M3::I,
                ),
                srgb(0.24, 0.23, 0.21),
                0.0,
            );
        }
    } else {
        for &(x0, x1, y0, y1) in &runs {
            k.add(
                box_at(
                    v3((x0 + x1) * 0.5, (y0 + y1) * 0.5, 0.0),
                    v3((x1 - x0) * 0.5, (y1 - y0) * 0.5, th),
                    &M3::I,
                ),
                cmix(stone, dark, 0.25),
                0.06,
            );
        }
    }
    // arch voussoirs (missing where the wall has crumbled)
    let nv = if full { 9 } else { 5 };
    for v in 0..nv {
        let t0 = PI * v as f32 / nv as f32;
        let t1 = PI * (v + 1) as f32 / nv as f32;
        let mid = (t0 + t1) * 0.5;
        let outer_top = spring + (ar + vd) * mid.sin();
        if outer_top > top(xw + (ar + vd) * mid.cos()) + 0.15 {
            continue;
        }
        let mut c = [V3::ZERO; 8];
        for (q, cc) in c.iter_mut().enumerate() {
            let r = if q & 1 != 0 { ar + vd } else { ar };
            let t = if q & 2 != 0 { t1 - 0.012 } else { t0 + 0.012 };
            let z = if q & 4 != 0 { th + 0.02 } else { -th - 0.02 };
            *cc = v3(xw + r * t.cos(), spring + r * t.sin(), z);
        }
        k.add(hexa(c), cmix(stone, dark, 0.15), 0.08);
    }
    // sill stone
    k.add(
        box_at(
            v3(xw, sill + 0.05, 0.0),
            v3(ar + 0.12, 0.06, th + 0.06),
            &M3::I,
        ),
        cscale(stone, 1.05),
        0.05,
    );
    // fallen blocks
    let nf = if full { 6 } else { 2 };
    for f in 0..nf {
        let side = if f % 2 == 0 { 1.0 } else { -1.0 };
        let x = mr.sym(half * 0.9);
        let z = side * mr.range(0.7, 1.6);
        let s = v3(
            mr.range(0.2, 0.4),
            mr.range(0.16, 0.22),
            mr.range(0.25, 0.4),
        );
        let rot = M3::rot_y(mr.angle()).then(&M3::rot_z(mr.sym(0.3)));
        let b = if full {
            rough_box(v3(x, s.y * 0.6, z), s, &rot, 0.03, seed ^ f)
        } else {
            box_at(v3(x, s.y * 0.6, z), s, &rot)
        };
        k.add(b, cmix(stone, dark, 0.3), 0.1);
    }
    let mut m = k.finish(0.6, 0.35);
    // moss on top faces + damp base
    let moss = srgb(0.30, 0.38, 0.16);
    for q in 0..m.p.len() {
        let p = m.p[q];
        let n = m.n[q];
        let up =
            smoothstep(0.6, 0.9, n.y) * smoothstep(0.2, 0.9, 0.5 + 0.5 * noise3(p * 1.5, seed ^ 3));
        let low = 1.0 - smoothstep(-0.2, 0.9, p.y);
        let c = m.c[q];
        let cc = cmix([c[0], c[1], c[2]], moss, up * 0.75 + low * 0.3);
        m.c[q] = rgba(cc, 0.0);
    }
    m
}

fn watchtower(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let (plank, beam, post) = wood_col(&mut mr);
    let roof_c = mr.pick(&[
        srgb(0.40, 0.28, 0.20),
        srgb(0.52, 0.26, 0.18),
        srgb(0.36, 0.36, 0.38),
    ]);
    let hp = mr.range(6.8, 7.4);
    let bs = 1.7f32;
    let ts = 1.25f32;
    let ps = 1.55f32;
    let ye = hp + 2.25;
    let mut k = Kit::new(&mut mr);
    let leg = |sx: f32, sz: f32, y: f32| {
        let t = (y + 0.5) / (hp + 0.4);
        v3(sx * lerp(bs, ts, t), y, sz * lerp(bs, ts, t))
    };
    let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for &(sx, sz) in &corners {
        k.add(
            tube(
                &[leg(sx, sz, -0.5), leg(sx, sz, hp - 0.1)],
                &[0.16, 0.13],
                if full { 6 } else { 4 },
                Cap::Open,
                Cap::Open,
            ),
            post,
            0.08,
        );
    }
    let levels: &[f32] = if full {
        &[0.35, 2.4, 4.6, hp - 0.35]
    } else {
        &[2.4]
    };
    for (li, &y) in levels.iter().enumerate() {
        for c in 0..4 {
            let (ax, az) = corners[c];
            let (bx, bz) = corners[(c + 1) % 4];
            let a = leg(ax, az, y);
            let b = leg(bx, bz, y);
            let out = v3(ax + bx, 0.0, az + bz).norm();
            k.add(
                box_between(a + out * 0.1, b + out * 0.1, 0.12, 0.16, V3::Y),
                beam,
                0.08,
            );
            if full && li + 1 < levels.len() {
                let y2 = levels[li + 1];
                let a2 = leg(ax, az, y2);
                let b2 = leg(bx, bz, y2);
                k.add(
                    box_between(a + out * 0.14, b2 + out * 0.14, 0.12, 0.06, out),
                    beam,
                    0.1,
                );
                k.add(
                    box_between(b + out * 0.16, a2 + out * 0.16, 0.12, 0.06, out),
                    beam,
                    0.1,
                );
            }
        }
    }
    // platform
    if full {
        for s in [-1.0f32, 1.0] {
            k.add(
                box_at(
                    v3(0.0, hp - 0.18, s * (ps - 0.1)),
                    v3(ps, 0.1, 0.08),
                    &M3::I,
                ),
                beam,
                0.05,
            );
            k.add(
                box_at(
                    v3(s * (ps - 0.1), hp - 0.18, 0.0),
                    v3(0.08, 0.1, ps),
                    &M3::I,
                ),
                beam,
                0.05,
            );
        }
        let n = 10;
        for i in 0..n {
            let z = -ps + (i as f32 + 0.5) * 2.0 * ps / n as f32;
            k.add(
                box_at(
                    v3(0.0, hp - 0.04, z),
                    v3(ps + 0.04, 0.04, ps / n as f32 - 0.015),
                    &M3::rot_y(mr.sym(0.01)),
                ),
                plank,
                0.12,
            );
        }
    } else {
        k.add(
            box_at(v3(0.0, hp - 0.1, 0.0), v3(ps, 0.1, ps), &M3::I),
            plank,
            0.0,
        );
    }
    // roof posts + railing (gap on the ladder side, +Z)
    for &(sx, sz) in &corners {
        k.add(
            box_at(
                v3(sx * (ps - 0.1), (hp + ye) * 0.5, sz * (ps - 0.1)),
                v3(0.08, (ye - hp) * 0.5, 0.08),
                &M3::I,
            ),
            post,
            0.06,
        );
    }
    let rails: &[f32] = if full { &[1.0, 0.5] } else { &[1.0] };
    for &ry in rails {
        for c in 0..4 {
            let (ax, az) = corners[c];
            let (bx, bz) = corners[(c + 1) % 4];
            let a = v3(ax * (ps - 0.1), hp + ry, az * (ps - 0.1));
            let b = v3(bx * (ps - 0.1), hp + ry, bz * (ps - 0.1));
            if az > 0.0 && bz > 0.0 {
                // ladder side: two short stubs leaving an opening
                let m1 = a.lerp(b, 0.3);
                let m2 = a.lerp(b, 0.7);
                k.add(box_between(a, m1, 0.08, 0.1, V3::Y), beam, 0.06);
                k.add(box_between(m2, b, 0.08, 0.1, V3::Y), beam, 0.06);
            } else {
                k.add(box_between(a, b, 0.08, 0.1, V3::Y), beam, 0.06);
            }
        }
    }
    // layered pyramid roof
    let bands: &[(f32, f32, f32, f32)] = if full {
        &[
            (2.05, 1.5, 0.0, 0.55),
            (1.65, 1.05, 0.45, 1.0),
            (1.2, 0.6, 0.9, 1.45),
            (0.7, 0.05, 1.35, 1.9),
        ]
    } else {
        &[(2.05, 1.05, 0.0, 1.0), (1.2, 0.05, 0.9, 1.9)]
    };
    for &(sb, st, y0, y1) in bands {
        let mut c = [V3::ZERO; 8];
        for (q, cc) in c.iter_mut().enumerate() {
            let s = if q & 2 != 0 { st } else { sb };
            let sx = if q & 1 != 0 { 1.0 } else { -1.0 };
            let sz = if q & 4 != 0 { 1.0 } else { -1.0 };
            *cc = v3(sx * s, ye + if q & 2 != 0 { y1 } else { y0 }, sz * s);
        }
        k.add(hexa(c), roof_c, 0.08);
    }
    if full {
        k.add(
            box_at(v3(0.0, ye + 2.05, 0.0), v3(0.04, 0.25, 0.04), &M3::I),
            post,
            0.0,
        );
        // ladder
        let lz0 = bs + 1.0;
        for s in [-1.0f32, 1.0] {
            k.add(
                box_between(
                    v3(s * 0.28, -0.2, lz0),
                    v3(s * 0.28, hp + 0.9, ps + 0.05),
                    0.08,
                    0.07,
                    V3::X,
                ),
                beam,
                0.06,
            );
        }
        let n = 18;
        for i in 1..n {
            let t = i as f32 / n as f32;
            let p = v3(0.0, -0.2, lz0).lerp(v3(0.0, hp + 0.9, ps + 0.05), t);
            if p.y > hp - 0.1 {
                break;
            }
            k.add(box_at(p, v3(0.3, 0.025, 0.035), &M3::I), plank, 0.08);
        }
    } else {
        k.add(
            box_between(
                v3(0.0, -0.2, bs + 1.0),
                v3(0.0, hp, ps + 0.05),
                0.6,
                0.06,
                V3::X,
            ),
            beam,
            0.0,
        );
    }
    k.finish(0.6, 0.3)
}

fn campfire(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let mut m = Mesh::new();
    let ring_r = mr.range(0.58, 0.68);
    let ns = mr.int(9, 10) as usize;
    let stone = srgb(0.52, 0.50, 0.47);
    let soot = srgb(0.16, 0.15, 0.14);
    let a0 = mr.angle();
    for i in 0..ns {
        let a = a0 + i as f32 * TAU / ns as f32 + mr.sym(0.12);
        let s = mr.range(0.16, 0.22);
        let radii = v3(s * 1.15, s * mr.range(0.6, 0.8), s * 0.9);
        let pos = V3::polar(a) * (ring_r + mr.sym(0.04));
        let rs = mr.seed();
        let mut r = placed_rock(
            if full { 1 } else { 0 },
            radii,
            0.18,
            4,
            0.15,
            rs,
            pos,
            a + PI * 0.5,
            M3::I,
            0.05,
            false,
        );
        paint_rock(&mut r, stone, cscale(stone, 0.7), rs, pos, s, 0.0, 0.0);
        for q in 0..r.p.len() {
            let inward = (-r.p[q]).horiz().norm().dot(r.n[q]);
            let t = smoothstep(-0.2, 0.7, inward) * 0.75;
            let c = r.c[q];
            r.c[q] = rgba(cmix([c[0], c[1], c[2]], soot, t), 0.0);
        }
        m.append(&r);
    }
    // ash bed
    let mut ash = lathe(
        &[(ring_r * 0.95, 0.0), (ring_r * 0.55, 0.035), (0.0, 0.05)],
        if full { 12 } else { 6 },
        0.0,
    );
    let ash_c = srgb(0.24, 0.23, 0.22);
    let ash_l = srgb(0.48, 0.46, 0.43);
    let glow = srgb(1.0, 0.42, 0.08);
    ash.paint(|p, _, _| {
        let r = p.horiz().len();
        let c = cmix(ash_c, ash_l, 0.5 + 0.5 * noise3(p * 6.0, seed));
        rgba(cmix(c, glow, (1.0 - smoothstep(0.05, 0.3, r)) * 0.9), 0.0)
    });
    m.append(&ash);
    // logs in a teepee
    let nl = mr.int(4, 5) as usize;
    let bark = srgb(0.36, 0.26, 0.18);
    let char_c = srgb(0.08, 0.07, 0.07);
    let ember = srgb(1.0, 0.45, 0.10);
    let la0 = mr.angle();
    for i in 0..nl {
        let a = la0 + i as f32 * TAU / nl as f32 + mr.sym(0.2);
        let r = mr.range(0.055, 0.075);
        let b = V3::polar(a) * (ring_r * 0.8) + V3::Y * 0.05;
        let t = V3::polar(a + 0.3) * 0.07 + V3::Y * mr.range(0.42, 0.55);
        let mut lg = if full {
            tube(
                &[b, b.lerp(t, 0.5), t],
                &[r, r, r * 0.85],
                6,
                Cap::Flat,
                Cap::Flat,
            )
        } else {
            tube(&[b, t], &[r, r * 0.85], 4, Cap::Open, Cap::Flat)
        };
        lg.paint(|p, _, uv| {
            if uv[1] > 1.5 {
                return rgba(ember, 0.0);
            }
            if uv[1] < -0.5 {
                return rgba(srgb(0.70, 0.56, 0.38), 0.0);
            }
            let burn = smoothstep(0.35, 0.9, uv[1]);
            let c = cmix(
                cscale(bark, 0.85 + 0.15 * noise3(p * 12.0, seed)),
                char_c,
                burn,
            );
            rgba(cmix(c, ember, smoothstep(0.85, 1.0, uv[1]) * 0.8), 0.0)
        });
        m.append(&lg);
    }
    let mut er = mr.fork();
    if full {
        for e in 0..6 {
            let mut o = octa();
            let s = er.range(0.035, 0.06);
            o.scale3(v3(s, s * 0.6, s));
            o.translate(V3::polar(er.angle()) * er.range(0.05, 0.25) + V3::Y * 0.06);
            let c = if e % 2 == 0 {
                srgb(1.0, 0.55, 0.12)
            } else {
                srgb(0.95, 0.25, 0.06)
            };
            o.paint(|_, _, _| rgba(c, 0.0));
            m.append(&o);
        }
    }
    // flames: twisted tongues, bright and saturated so they read as emissive
    let core = srgb(1.0, 0.92, 0.55);
    let mid = srgb(1.0, 0.62, 0.12);
    let tip = srgb(0.95, 0.30, 0.04);
    let nf = if full { 3 } else { 2 };
    for f in 0..nf {
        let a = mr.angle();
        let off = V3::polar(a) * if f == 0 { 0.0 } else { mr.range(0.08, 0.13) };
        let hh = if f == 0 {
            mr.range(0.75, 0.95)
        } else {
            mr.range(0.45, 0.65)
        };
        let ph = mr.angle();
        let rings = if full { 5 } else { 2 };
        let path: Vec<V3> = (0..=rings)
            .map(|i| {
                let t = i as f32 / rings as f32;
                off * (1.0 - t * 0.6)
                    + V3::polar(ph + t * 4.0) * (0.06 * t)
                    + V3::Y * (0.06 + hh * t)
            })
            .collect();
        let base_r = if f == 0 { 0.17 } else { 0.11 };
        let mut fl = tube_fn(
            &path,
            if full { 6 } else { 4 },
            Cap::Open,
            Cap::Open,
            |i, ang| {
                let t = i as f32 / rings as f32;
                if i == rings {
                    return 0.0;
                }
                base_r
                    * (1.0 - t).powf(0.7)
                    * (1.0 + 0.4 * t)
                    * (1.0 + 0.15 * (3.0 * ang + t * 6.0).sin())
            },
        );
        fl.bend_normals(0.6, |_| V3::Y);
        fl.paint(|_, _, uv| {
            let t = uv[1];
            let c = if t < 0.4 {
                cmix(core, mid, t / 0.4)
            } else {
                cmix(mid, tip, (t - 0.4) / 0.6)
            };
            rgba(c, 0.35 * t)
        });
        m.append(&fl);
    }
    m.ground_ao(0.15, 0.3);
    m
}

fn crate_box(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let s = mr.range(0.85, 1.1);
    let hs = s * 0.5;
    let pine = mr.pick(&[
        srgb(0.72, 0.56, 0.36),
        srgb(0.62, 0.48, 0.32),
        srgb(0.66, 0.58, 0.46),
    ]);
    let frame = cscale(pine, 0.7);
    let mut k = Kit::new(&mut mr);
    let f = 0.075;
    let c = v3(0.0, hs, 0.0);
    if full {
        k.add(
            box_at(c, v3(hs - 0.03, hs - 0.03, hs - 0.03), &M3::I),
            srgb(0.20, 0.14, 0.09),
            0.0,
        );
        // planks on 4 sides + top
        for face in 0..5 {
            let (n, u) = match face {
                0 => (V3::X, V3::Z),
                1 => (-V3::X, V3::Z),
                2 => (V3::Z, V3::X),
                3 => (-V3::Z, V3::X),
                _ => (V3::Y, V3::X),
            };
            let vdir = n.cross(u);
            for p in 0..3 {
                let t = -hs + f + (p as f32 + 0.5) * (s - 2.0 * f) / 3.0;
                let center = c + n * (hs - 0.025) + vdir * t;
                let half = u * (hs - f) + vdir * ((s - 2.0 * f) / 6.0 - 0.012) + n * 0.014;
                let half = v3(half.x.abs(), half.y.abs(), half.z.abs());
                k.add(box_at(center, half, &M3::I), pine, 0.12);
            }
            if face < 4 {
                // diagonal brace
                let a = c + n * (hs - 0.005) - u * (hs - f) - V3::Y * (hs - f);
                let b = c + n * (hs - 0.005) + u * (hs - f) + V3::Y * (hs - f);
                let (a, b) = if face % 2 == 0 {
                    (a, b)
                } else {
                    (a + u * (s - 2.0 * f), b - u * (s - 2.0 * f))
                };
                k.add(box_between(a, b, 0.09, 0.022, n), frame, 0.06);
            }
        }
        // frame boards along the 12 edges
        for sx in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                k.add(
                    box_at(
                        c + v3(sx * (hs - f * 0.5), 0.0, sz * (hs - f * 0.5)),
                        v3(f * 0.5 + 0.004, hs, f * 0.5 + 0.004),
                        &M3::I,
                    ),
                    frame,
                    0.08,
                );
            }
        }
        for sy in [-1.0f32, 1.0] {
            for s2 in [-1.0f32, 1.0] {
                k.add(
                    box_at(
                        c + v3(0.0, sy * (hs - f * 0.5), s2 * (hs - f * 0.5)),
                        v3(hs - f, f * 0.5, f * 0.5 + 0.002),
                        &M3::I,
                    ),
                    frame,
                    0.08,
                );
                k.add(
                    box_at(
                        c + v3(s2 * (hs - f * 0.5), sy * (hs - f * 0.5), 0.0),
                        v3(f * 0.5 + 0.002, f * 0.5, hs - f),
                        &M3::I,
                    ),
                    frame,
                    0.08,
                );
            }
        }
    } else {
        k.add(
            box_at(c, v3(hs - 0.01, hs - 0.01, hs - 0.01), &M3::I),
            pine,
            0.0,
        );
        for sx in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                k.add(
                    box_at(
                        c + v3(sx * (hs - f * 0.5), 0.0, sz * (hs - f * 0.5)),
                        v3(f * 0.5, hs, f * 0.5),
                        &M3::I,
                    ),
                    frame,
                    0.0,
                );
            }
        }
    }
    k.finish(0.15, 0.25)
}

fn barrel(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let h = mr.range(0.95, 1.1);
    let re = mr.range(0.29, 0.33);
    let rm = re + mr.range(0.06, 0.08);
    let seed = mr.seed();
    let oak = mr.pick(&[
        srgb(0.55, 0.38, 0.22),
        srgb(0.48, 0.34, 0.22),
        srgb(0.60, 0.45, 0.30),
    ]);
    let iron = srgb(0.22, 0.21, 0.20);
    let rust = srgb(0.38, 0.24, 0.16);
    let rad = |y: f32| re + (rm - re) * (PI * y / h).sin();
    let mut m = Mesh::new();
    let mut cr = mr.fork();
    if full {
        let n = 14;
        let rows = 7;
        for j in 0..n {
            let a0 = j as f32 * TAU / n as f32;
            let a1 = (j + 1) as f32 * TAU / n as f32;
            let grid: Vec<Vec<V3>> = (0..=rows)
                .map(|r| {
                    let y = h * r as f32 / rows as f32;
                    vec![
                        V3::polar(a0) * rad(y) + V3::Y * y,
                        V3::polar(a1) * rad(y) + V3::Y * y,
                    ]
                })
                .collect();
            let mut st = grid_mesh(&grid);
            let c = cscale(ctint(oak, cr.sym(1.0), 0.1), cr.range(0.85, 1.12));
            st.paint(|p, _, _| {
                rgba(
                    cscale(
                        c,
                        0.95 + 0.08 * noise3(v3(p.x * 8.0, p.y * 1.5, p.z * 8.0), seed),
                    ),
                    0.0,
                )
            });
            m.append(&st);
        }
        for &t in &[0.1f32, 0.28, 0.72, 0.9] {
            let y0 = h * t - 0.035;
            let y1 = h * t + 0.035;
            let (ra, rb) = (rad(y0) + 0.002, rad(y1) + 0.002);
            let mut hp = lathe(
                &[(ra, y0), (ra + 0.014, y0), (rb + 0.014, y1), (rb, y1)],
                14,
                0.0,
            );
            hp.paint(|p, _, _| rgba(cmix(iron, rust, 0.3 + 0.4 * noise3(p * 12.0, seed)), 0.0));
            m.append(&hp);
        }
        let mut top = lathe(
            &[
                (re, h),
                (re - 0.035, h),
                (re - 0.035, h - 0.05),
                (0.0, h - 0.05),
            ],
            14,
            0.0,
        );
        top.paint(|p, _, uv| {
            let lid = uv[1] > 0.6;
            let stripe = hashf(((p.x + 1.0) * 9.0) as u32, seed);
            let c = if lid {
                cscale(oak, 0.75 + 0.15 * stripe)
            } else {
                cscale(oak, 0.9)
            };
            rgba(c, 0.0)
        });
        m.append(&top);
    } else {
        let ys = [0.0, 0.1, 0.28, 0.5, 0.72, 0.9, 1.0];
        let mut prof: Vec<(f32, f32)> = ys.iter().map(|&t| (rad(h * t), h * t)).collect();
        prof.push((0.0, h - 0.04));
        let mut b = lathe(&prof, 8, 0.0);
        b.paint(|p, _, uv| {
            let ring = (uv[1] * 7.0).round() as i32;
            let c = if (ring == 1 || ring == 2 || ring == 4 || ring == 5) && p.y < h - 0.01 {
                cmix(oak, iron, 0.55)
            } else {
                oak
            };
            rgba(c, 0.0)
        });
        m.append(&b);
    }
    m.ground_ao(0.15, 0.3);
    m
}

fn signpost(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let (plank, _, post) = wood_col(&mut mr);
    let ink = srgb(0.14, 0.10, 0.07);
    let ph = mr.range(2.1, 2.4);
    let lean = M3::rot_z(mr.sym(0.04)).then(&M3::rot_x(mr.sym(0.04)));
    let mut k = Kit::new(&mut mr);
    k.add(
        box_at(
            V3::Y * ((ph - 0.3) * 0.5),
            v3(0.07, (ph + 0.3) * 0.5, 0.07),
            &M3::I,
        )
        .xf(&lean, V3::ZERO),
        post,
        0.06,
    );
    if full {
        k.add(
            lathe(&[(0.1, ph), (0.0, ph + 0.1)], 4, PI * 0.25).xf(&lean, V3::ZERO),
            post,
            0.06,
        );
    }
    let nb = mr.int(2, 3) as usize;
    let mut yaw = mr.angle();
    for b in 0..nb {
        let y = ph - 0.3 - b as f32 * 0.32;
        let len = mr.range(0.8, 1.0);
        let hh = 0.11;
        let flip = mr.chance(0.5);
        let poly = [
            (-0.12, -hh),
            (len - 0.15, -hh),
            (len, 0.0),
            (len - 0.15, hh),
            (-0.12, hh),
        ];
        let rot = M3::rot_z(mr.sym(0.05)).then(&M3::rot_y(yaw + if flip { PI } else { 0.0 }));
        let board = prism(&poly, 0.035).xf(&rot, V3::Y * y + V3::ZERO);
        k.add(board.xf(&lean, V3::ZERO), plank, 0.12);
        let mut lr = mr.fork();
        if full {
            // painted lettering strokes on both faces
            let strokes = lr.int(3, 5);
            let mut x = 0.0;
            for _ in 0..strokes {
                let w = lr.range(0.06, 0.16);
                if x + w > len - 0.2 {
                    break;
                }
                let dy = lr.sym(0.02);
                for side in [-1.0f32, 1.0] {
                    let c = v3(x + w * 0.5, dy, side * 0.019);
                    let st = box_at(c, v3(w * 0.5, 0.022, 0.003), &M3::I)
                        .xf(&rot, V3::Y * y)
                        .xf(&lean, V3::ZERO);
                    k.add(st, ink, 0.0);
                }
                x += w + 0.04;
            }
        }
        yaw += mr.range(1.2, 2.4);
    }
    // stones at the foot
    let nst = if full { 4 } else { 2 };
    let mut m = k.finish(0.3, 0.3);
    for i in 0..nst {
        let a = i as f32 * TAU / nst as f32 + 0.4;
        let s = mr.range(0.1, 0.16);
        let pos = V3::polar(a) * 0.17;
        let mut r = placed_rock(
            if full { 1 } else { 0 },
            v3(s * 1.2, s * 0.8, s),
            0.2,
            4,
            0.15,
            seed ^ i as u32,
            pos,
            a,
            M3::I,
            0.04,
            false,
        );
        paint_rock(
            &mut r,
            srgb(0.55, 0.53, 0.50),
            srgb(0.40, 0.38, 0.36),
            seed,
            pos,
            s,
            0.0,
            0.0,
        );
        m.append(&r);
    }
    m
}

/// Banner pole: the cloth itself is simulated (effects.rs) and hangs from the arm.
pub const BANNER_POLE_H: f32 = 4.0;
pub const BANNER_ARM: (f32, f32) = (0.12, 1.42);

fn banner_pole(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let (_, beam, post) = wood_col(&mut mr);
    let h = BANNER_POLE_H;
    let mut k = Kit::new(&mut mr);
    k.add(
        box_at(V3::Y * (h * 0.5), v3(0.065, h * 0.5, 0.065), &M3::I),
        post,
        0.06,
    );
    k.add(
        box_at(
            v3((BANNER_ARM.0 + BANNER_ARM.1) * 0.5, h - 0.25, 0.0),
            v3((BANNER_ARM.1 - BANNER_ARM.0) * 0.5 + 0.06, 0.035, 0.035),
            &M3::I,
        ),
        beam,
        0.06,
    );
    if full {
        k.add(
            lathe(&[(0.1, h), (0.0, h + 0.16)], 6, PI * 0.25),
            beam,
            0.06,
        );
        // brace under the arm
        k.add(
            box_at(
                v3(0.3, h - 0.5, 0.0),
                v3(0.03, 0.3, 0.03),
                &M3::rot_z(-0.75),
            ),
            beam,
            0.06,
        );
    }
    k.finish(0.3, 0.3)
}

/// Tarp shelter frame: four posts, high at the front (+z), low at the back. The tarp itself is
/// simulated cloth (effects.rs) pinned to the post tops.
pub const TARP_HALF: f32 = 1.6;
pub const TARP_POSTS: (f32, f32) = (2.3, 1.55);

fn tarp_shelter(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let (_, _, post) = wood_col(&mut mr);
    let mut k = Kit::new(&mut mr);
    for (x, z) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let h = if z > 0.0 { TARP_POSTS.0 } else { TARP_POSTS.1 };
        let p = v3(x * TARP_HALF, 0.0, z * TARP_HALF);
        k.add(
            box_at(p + V3::Y * (h * 0.5), v3(0.05, h * 0.5, 0.05), &M3::I),
            post,
            0.06,
        );
        if full {
            k.add(
                lathe(&[(0.07, h), (0.0, h + 0.08)], 5, 0.0).xf(&M3::I, p),
                post,
                0.06,
            );
        }
    }
    k.finish(0.3, 0.3)
}

fn well(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let seed = mr.seed();
    let ri = 0.62f32;
    let ro = 0.95f32;
    let stone = mr.pick(&[srgb(0.58, 0.56, 0.52), srgb(0.62, 0.56, 0.48)]);
    let (plank, beam, post) = wood_col(&mut mr);
    let roof_c = mr.pick(&[
        srgb(0.46, 0.30, 0.22),
        srgb(0.40, 0.40, 0.42),
        srgb(0.68, 0.56, 0.34),
    ]);
    let mut m = Mesh::new();
    let mut sc = mr.fork();
    let stone_part = |m: &mut Mesh, mut p: Mesh, sc: &mut Rng| {
        let c = cscale(ctint(stone, sc.sym(1.0), 0.1), sc.range(0.82, 1.12));
        p.paint(|q, _, _| rgba(cscale(c, 1.0 + 0.05 * noise3(q * 4.0, seed)), 0.0));
        m.append(&p);
    };
    if full {
        let courses = [(-0.25f32, 0.12f32), (0.12, 0.48), (0.48, 0.8)];
        let nb = 10;
        for (ci, &(y0, y1)) in courses.iter().enumerate() {
            let off = if ci % 2 == 1 { 0.5 } else { 0.0 };
            for b in 0..nb {
                let a0 = (b as f32 + off) * TAU / nb as f32 + 0.012;
                let a1 = (b as f32 + 1.0 + off) * TAU / nb as f32 - 0.012;
                let j = sc.sym(0.02);
                let blk = ring_block(ri + j, ro + j, a0, a1, y0 + 0.01, y1 - 0.01);
                stone_part(&mut m, blk, &mut sc);
            }
        }
        for b in 0..nb {
            let a0 = (b as f32 + 0.25) * TAU / nb as f32 + 0.01;
            let a1 = (b as f32 + 1.25) * TAU / nb as f32 - 0.01;
            let blk = ring_block(ri - 0.04, ro + 0.05, a0, a1, 0.8, 0.92);
            stone_part(&mut m, blk, &mut sc);
        }
        // mortar core between the blocks
        let mut core = lathe(&[(ro - 0.03, -0.2), (ro - 0.03, 0.8)], 10, 0.0);
        core.append(&lathe(&[(ri + 0.03, 0.8), (ri + 0.03, -0.2)], 10, 0.0));
        core.paint(|_, _, _| rgba(srgb(0.25, 0.24, 0.22), 0.0));
        m.append(&core);
    } else {
        let ring = lathe(&[(ro, -0.2), (ro, 0.92), (ri, 0.92), (ri, 0.1)], 10, 0.0);
        stone_part(&mut m, ring, &mut sc);
    }
    let mut water = lathe(&[(ri + 0.03, 0.15), (0.0, 0.15)], 10, 0.0);
    water.paint(|p, _, _| {
        rgba(
            cmix(
                srgb(0.08, 0.16, 0.18),
                srgb(0.16, 0.28, 0.30),
                0.5 + 0.5 * noise3(p * 3.0, seed),
            ),
            0.0,
        )
    });
    m.append(&water);
    let mut k = Kit::new(&mut mr);
    let px = ro + 0.12;
    for s in [-1.0f32, 1.0] {
        k.add(
            box_at(v3(s * px, 1.0, 0.0), v3(0.08, 1.3, 0.08), &M3::I),
            post,
            0.06,
        );
    }
    let ry = 1.55;
    k.add(
        tube(
            &[v3(-px, ry, 0.0), v3(px + 0.12, ry, 0.0)],
            &[0.07, 0.07],
            if full { 8 } else { 4 },
            Cap::Flat,
            Cap::Flat,
        ),
        beam,
        0.05,
    );
    let ridge = 2.55;
    let eave_y = 2.0;
    let eave_z = 0.85;
    for s in [-1.0f32, 1.0] {
        let a = v3(0.0, ridge, 0.0);
        let b = v3(0.0, eave_y, s * eave_z);
        k.add(
            box_between(a, b, 2.6, 0.06, V3::Y),
            cscale(roof_c, 0.8),
            0.04,
        );
        if full {
            for r in 0..3 {
                let t0 = r as f32 / 3.0;
                let t1 = (r as f32 + 1.3) / 3.0;
                let lift = V3::Y * 0.05;
                k.add(
                    box_between(
                        a.lerp(b, t0) + lift,
                        a.lerp(b, t1.min(1.05)) + lift * 0.4,
                        2.65,
                        0.05,
                        V3::Y,
                    ),
                    roof_c,
                    0.1,
                );
            }
        }
    }
    k.add(
        box_at(v3(0.0, ridge + 0.04, 0.0), v3(1.36, 0.06, 0.08), &M3::I),
        cscale(roof_c, 0.7),
        0.04,
    );
    for s in [-1.0f32, 1.0] {
        k.add(
            box_between(
                v3(s * px, 2.3, 0.0),
                v3(s * px, ridge, 0.0),
                0.1,
                0.1,
                V3::Z,
            ),
            post,
            0.04,
        );
    }
    if full {
        // rope on the roller, crank, bucket
        k.add(
            tube(
                &[v3(-0.25, ry, 0.0), v3(0.25, ry, 0.0)],
                &[0.1, 0.1],
                8,
                Cap::Flat,
                Cap::Flat,
            ),
            srgb(0.66, 0.56, 0.38),
            0.03,
        );
        k.add(
            box_between(
                v3(px + 0.12, ry, 0.0),
                v3(px + 0.12, ry - 0.32, 0.0),
                0.05,
                0.05,
                V3::Z,
            ),
            post,
            0.0,
        );
        k.add(
            box_between(
                v3(px + 0.12, ry - 0.32, 0.0),
                v3(px + 0.32, ry - 0.32, 0.0),
                0.05,
                0.05,
                V3::Y,
            ),
            post,
            0.0,
        );
        let by = 1.0;
        k.add(
            box_between(
                v3(0.0, ry - 0.08, 0.0),
                v3(0.0, by + 0.27, 0.0),
                0.02,
                0.02,
                V3::Z,
            ),
            srgb(0.66, 0.56, 0.38),
            0.0,
        );
        let bucket = lathe(
            &[
                (0.0, by),
                (0.13, by),
                (0.16, by + 0.27),
                (0.145, by + 0.27),
                (0.115, by + 0.04),
                (0.0, by + 0.04),
            ],
            10,
            0.0,
        );
        k.add(bucket, plank, 0.05);
        let hoop = lathe(
            &[
                (0.145, by + 0.04),
                (0.152, by + 0.04),
                (0.16, by + 0.1),
                (0.153, by + 0.1),
            ],
            10,
            0.0,
        );
        k.add(hoop, srgb(0.22, 0.21, 0.2), 0.0);
    }
    let parts = k.finish(0.0, 0.0);
    m.append(&parts);
    m.ground_ao(0.45, 0.35);
    m
}

fn dock(rng: &mut Rng, lod: u32) -> Mesh {
    let full = lod == 0;
    let mut mr = rng.fork();
    let (plank, beam, post) = wood_col(&mut mr);
    let half = 6.0f32;
    let hw = 1.3f32;
    let top = 0.55f32;
    let missing = mr.int(5, 35);
    let mut k = Kit::new(&mut mr);
    for i in 0..=6 {
        let x = -half + i as f32 * 2.0;
        let x = x.clamp(-half + 0.12, half - 0.12);
        for s in [-1.0f32, 1.0] {
            let bollard = (i == 6 || i == 3) && s > 0.0;
            let yt = if bollard { top + 0.75 } else { top - 0.2 };
            let r = 0.14 + 0.01 * (i % 2) as f32;
            k.add(
                tube(
                    &[v3(x, -3.0, s * (hw - 0.12)), v3(x, yt, s * (hw - 0.12))],
                    &[r, r * 0.95],
                    if full { 7 } else { 4 },
                    Cap::Open,
                    Cap::Flat,
                ),
                post,
                0.08,
            );
        }
        k.add(
            box_at(v3(x, top - 0.3, 0.0), v3(0.1, 0.1, hw + 0.08), &M3::I),
            beam,
            0.06,
        );
    }
    if full {
        for z in [-1.0f32, 0.0, 1.0] {
            k.add(
                box_at(v3(0.0, top - 0.14, z), v3(half + 0.05, 0.06, 0.07), &M3::I),
                beam,
                0.05,
            );
        }
        let n = 43;
        for i in 0..n {
            if i as i32 == missing {
                continue;
            }
            let x = -half + (i as f32 + 0.5) * 2.0 * half / n as f32;
            let rot = M3::rot_y(mr.sym(0.012));
            k.add(
                box_at(
                    v3(x, top - 0.035, mr.sym(0.03)),
                    v3(half / n as f32 - 0.015, 0.035, hw + mr.sym(0.05)),
                    &rot,
                ),
                plank,
                0.14,
            );
        }
        // ladder at the water end
        for s in [-1.0f32, 1.0] {
            k.add(
                box_at(
                    v3(half + 0.05, top - 1.0, -0.6 + s * 0.25),
                    v3(0.04, 1.1, 0.04),
                    &M3::I,
                ),
                beam,
                0.05,
            );
        }
        for r in 0..5 {
            k.add(
                box_at(
                    v3(half + 0.05, top - 0.25 - r as f32 * 0.35, -0.6),
                    v3(0.03, 0.025, 0.27),
                    &M3::I,
                ),
                plank,
                0.05,
            );
        }
        // coiled rope
        let cpos = v3(half - 0.9, top + 0.035, 0.55);
        let path: Vec<V3> = (0..=10)
            .map(|i| cpos + V3::polar(i as f32 * TAU / 10.0) * 0.18)
            .collect();
        let coil = tube(&path, &[0.035; 11], 5, Cap::Open, Cap::Open);
        k.add(coil, srgb(0.68, 0.58, 0.40), 0.03);
        let coil2: Vec<V3> = path
            .iter()
            .map(|p| (*p - cpos) * 0.7 + cpos + V3::Y * 0.05)
            .collect();
        k.add(
            tube(&coil2, &[0.03; 11], 5, Cap::Open, Cap::Open),
            srgb(0.64, 0.54, 0.37),
            0.03,
        );
    } else {
        k.add(
            box_at(v3(0.0, top - 0.09, 0.0), v3(half, 0.09, hw), &M3::I),
            plank,
            0.0,
        );
    }
    let mut m = k.finish(0.0, 0.0);
    // a small crate on the deck
    let mut cr = Rng::new(missing as u64 + 7);
    let mut c = crate_box(&mut cr, lod);
    c.transform(&M3::rot_y(0.3), v3(half - 2.2, top, -0.45));
    let (mn, mx) = c.bounds();
    let s = 0.6 / (mx.y - mn.y);
    for p in c.p.iter_mut() {
        *p = v3(half - 2.2, top, -0.45) + (*p - v3(half - 2.2, top, -0.45)) * s;
    }
    m.append(&c);
    wet_below(&mut m, 0.1);
    m
}
