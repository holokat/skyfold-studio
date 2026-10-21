//! Light sources of a dungeon: torches, braziers, chandeliers, candles, lanterns, crystals, work
//! lamps, shafts of light from above and plain glows or spots. Each has a colour, a strength, a
//! reach, a direction (spots and shafts), a flicker, and optional shadows and haze. Auto-light
//! hangs them for you by room and style (every one stays editable); any can be added by hand.
//!
//! In the editor and walk mode the nearest lights are real Bevy lights (`sync`): a budget of the
//! closest ones shine, the few nearest cast shadows, fires flicker.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::geom;
use super::plan::{CRoom, Compiled, Plan, Role, RoomKind};
use super::styles::Style;

/// What gives the light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    /// a torch in a wall sconce
    #[default]
    Torch,
    /// a fire bowl on a stand
    Brazier,
    /// a fire basket hanging on chains
    HangingBrazier,
    /// a ring of candles or lamps hanging high
    Chandelier,
    /// a cluster of candles
    Candles,
    /// a lantern on the wall
    Lantern,
    /// glowing crystals
    Crystal,
    /// a hard white work lamp (a spot)
    WorkLamp,
    /// a shaft of light from high above (a spot, hazy)
    Shaft,
    /// a plain glow, no fixture
    Glow,
    /// a plain spot, no fixture
    Spot,
}

impl LightKind {
    pub const ALL: [LightKind; 11] = [
        LightKind::Torch,
        LightKind::Brazier,
        LightKind::HangingBrazier,
        LightKind::Chandelier,
        LightKind::Candles,
        LightKind::Lantern,
        LightKind::Crystal,
        LightKind::WorkLamp,
        LightKind::Shaft,
        LightKind::Glow,
        LightKind::Spot,
    ];
    pub fn name(self) -> &'static str {
        match self {
            LightKind::Torch => "Torch",
            LightKind::Brazier => "Brazier",
            LightKind::HangingBrazier => "Hanging fire",
            LightKind::Chandelier => "Chandelier",
            LightKind::Candles => "Candles",
            LightKind::Lantern => "Lantern",
            LightKind::Crystal => "Crystal",
            LightKind::WorkLamp => "Work lamp",
            LightKind::Shaft => "Light shaft",
            LightKind::Glow => "Glow",
            LightKind::Spot => "Spotlight",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            LightKind::Torch => "torch",
            LightKind::Brazier => "brazier",
            LightKind::HangingBrazier => "hanging_brazier",
            LightKind::Chandelier => "chandelier",
            LightKind::Candles => "candles",
            LightKind::Lantern => "lantern",
            LightKind::Crystal => "crystal",
            LightKind::WorkLamp => "work_lamp",
            LightKind::Shaft => "shaft",
            LightKind::Glow => "glow",
            LightKind::Spot => "spot",
        }
    }
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.id() == s || k.name().eq_ignore_ascii_case(s))
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as icon;
        match self {
            LightKind::Torch => icon::FIRE,
            LightKind::Brazier => icon::FIRE_SIMPLE,
            LightKind::HangingBrazier => icon::FLAME,
            LightKind::Chandelier => icon::LAMP,
            LightKind::Candles => icon::SPARKLE,
            LightKind::Lantern => icon::LAMP_PENDANT,
            LightKind::Crystal => icon::DIAMOND,
            LightKind::WorkLamp => icon::FLASHLIGHT,
            LightKind::Shaft => icon::SUN_DIM,
            LightKind::Glow => icon::LIGHTBULB,
            LightKind::Spot => icon::CROSSHAIR,
        }
    }
    /// Defaults: colour (srgb), lumens, reach (m), flicker, a spot's cone (deg; 0 = all round).
    pub fn defaults(self) -> ([f32; 3], f32, f32, f32, f32) {
        match self {
            LightKind::Torch => ([1.0, 0.6, 0.3], 200_000.0, 12.0, 0.35, 0.0),
            LightKind::Brazier => ([1.0, 0.55, 0.24], 650_000.0, 19.0, 0.4, 0.0),
            LightKind::HangingBrazier => ([1.0, 0.58, 0.26], 900_000.0, 24.0, 0.35, 0.0),
            LightKind::Chandelier => ([1.0, 0.76, 0.48], 900_000.0, 24.0, 0.12, 0.0),
            LightKind::Candles => ([1.0, 0.7, 0.42], 110_000.0, 7.0, 0.25, 0.0),
            LightKind::Lantern => ([1.0, 0.78, 0.52], 220_000.0, 12.0, 0.06, 0.0),
            LightKind::Crystal => ([0.45, 0.62, 1.0], 320_000.0, 14.0, 0.0, 0.0),
            LightKind::WorkLamp => ([0.92, 0.96, 1.0], 1_600_000.0, 28.0, 0.02, 55.0),
            LightKind::Shaft => ([0.78, 0.86, 1.0], 3_500_000.0, 48.0, 0.0, 22.0),
            LightKind::Glow => ([1.0, 0.85, 0.65], 300_000.0, 14.0, 0.0, 0.0),
            LightKind::Spot => ([1.0, 0.95, 0.85], 1_200_000.0, 24.0, 0.0, 40.0),
        }
    }
    pub fn is_spot(self) -> bool {
        self.defaults().4 > 0.0
    }
    /// Mounted on a wall (faces out of it), standing on the floor, or hanging from above.
    pub fn mount(self) -> Mount {
        match self {
            LightKind::Torch | LightKind::Lantern => Mount::Wall,
            LightKind::Brazier | LightKind::Candles | LightKind::Crystal => Mount::Floor,
            LightKind::HangingBrazier
            | LightKind::Chandelier
            | LightKind::WorkLamp
            | LightKind::Shaft => Mount::Ceiling,
            LightKind::Glow | LightKind::Spot => Mount::Free,
        }
    }
    /// The object that holds it (by art pack), and where its flame is above the object's origin.
    pub fn fixture(self, pack: &str) -> Option<(&'static str, f32)> {
        let hp = pack == "handpainted";
        Some(match self {
            LightKind::Torch => ("hp:torch_sconce", 0.95),
            LightKind::Brazier => {
                if hp {
                    ("hp:brazier", 1.35)
                } else {
                    ("hp:orc_brazier", 1.5)
                }
            }
            LightKind::HangingBrazier => ("hp:dragonbone_hanging_lantern", -1.5),
            LightKind::Chandelier => {
                if hp {
                    ("hp:rivetjaw_hanging_lantern", -1.5)
                } else {
                    ("sb:chandelier", -0.35)
                }
            }
            LightKind::Candles => {
                if hp {
                    ("hp:candles", 0.4)
                } else {
                    ("ph:brass_candleholders", 0.7)
                }
            }
            LightKind::Lantern => {
                if hp {
                    ("hp:wall_lantern", 0.4)
                } else {
                    ("sb:lantern_interior", 0.35)
                }
            }
            LightKind::Crystal => ("Crystals", 0.9),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount {
    Wall,
    Floor,
    Ceiling,
    Free,
}

/// A light source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightDef {
    pub id: u32,
    pub kind: LightKind,
    /// where the light is (the flame, the crystal's heart)
    pub pos: [f32; 3],
    /// which way a spot or shaft points: heading (radians, 0 = +z) and tilt down (radians)
    #[serde(default)]
    pub dir: [f32; 2],
    /// colour (srgb)
    pub color: [f32; 3],
    /// lumens
    pub lumens: f32,
    /// reach (m)
    pub range: f32,
    /// spot cone (degrees; 0 = all round)
    #[serde(default)]
    pub cone: f32,
    /// 0 = steady, 1 = a wild fire
    #[serde(default)]
    pub flicker: f32,
    /// casts shadows (the nearest few do; costly)
    #[serde(default = "yes")]
    pub shadows: bool,
    /// lights the haze (shafts, big fires)
    #[serde(default)]
    pub haze: bool,
    /// made by Auto-light (replaced when it runs again)
    #[serde(default)]
    pub auto: bool,
    /// the object holding it (torch sconce, brazier…)
    #[serde(default)]
    pub fixture: Option<u64>,
    #[serde(default)]
    pub room: Option<u32>,
}

fn yes() -> bool {
    true
}

impl LightDef {
    pub fn new(id: u32, kind: LightKind, pos: Vec3) -> LightDef {
        let (color, lumens, range, flicker, cone) = kind.defaults();
        LightDef {
            id,
            kind,
            pos: pos.to_array(),
            dir: [0.0, if kind == LightKind::Shaft { 1.25 } else { 0.9 }],
            color,
            lumens,
            range,
            cone,
            flicker,
            shadows: true,
            haze: matches!(
                kind,
                LightKind::Shaft | LightKind::Brazier | LightKind::HangingBrazier
            ),
            auto: false,
            fixture: None,
            room: None,
        }
    }
    pub fn position(&self) -> Vec3 {
        Vec3::from_array(self.pos)
    }
    /// The direction a spot shines.
    pub fn direction(&self) -> Vec3 {
        let (yaw, tilt) = (self.dir[0], self.dir[1]);
        Vec3::new(yaw.sin() * tilt.cos(), -tilt.sin(), yaw.cos() * tilt.cos())
            .normalize_or(Vec3::NEG_Y)
    }
}

/// Lights › All lights: a master over every light's own values (1 = each as set), saved with the
/// map. The colour all fires and lamps lean towards is the mood's light tint.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AllLights {
    /// × every light's strength
    pub brightness: f32,
    /// × every light's reach
    pub reach: f32,
    /// × every light's flicker
    pub flicker: f32,
}

impl Default for AllLights {
    fn default() -> Self {
        Self {
            brightness: 1.0,
            reach: 1.0,
            flicker: 1.0,
        }
    }
}

impl AllLights {
    /// A light's strength (lumens), reach (m) and flicker with the master applied.
    pub fn apply(&self, l: &LightDef) -> (f32, f32, f32) {
        (
            l.lumens * self.brightness.max(0.0),
            l.range * self.reach.max(0.05),
            l.flicker * self.flicker.max(0.0),
        )
    }
}

/// How far a wall fixture's origin (its back plate) sits in front of the wall (m): its back
/// touches the wall (the torch sconce and the wall lantern reach 5–6 cm behind their origin).
pub const WALL_MOUNT: f32 = 0.05;

/// One light Auto-light wants, with the fixture to put up (asset id, origin, heading).
#[derive(Clone, Debug)]
pub struct Planned {
    pub light: LightDef,
    pub fixture: Option<(String, Vec3, f32)>,
}

/// Lights every room by its kind, size, role and style: torches along the walls (alternating
/// sides in corridors), braziers round big rooms and either side of a boss's dais, chandeliers or
/// hanging fires over halls, candles in crypts and shrines, crystals in caverns, shafts of light
/// where a room is open overhead. Deterministic for a plan.
pub fn auto_light(
    plan: &Plan,
    c: &Compiled,
    style_of: &dyn Fn(&CRoom) -> &'static Style,
    brightness: f32,
) -> Vec<Planned> {
    let mut out: Vec<Planned> = Vec::new();
    let mut rng = crate::noise::Rng::new(plan.seed as u64 ^ 0x11ce);
    let spacing = (9.0 / brightness.clamp(0.3, 3.0)).clamp(4.0, 24.0);
    for r in &c.rooms {
        if !r.kind.is_space() || r.kind.is_pit() || r.kind == RoomKind::Pool {
            continue;
        }
        let st = style_of(r);
        let room = plan.room(r.id);
        let role = room.map(|r| r.role).unwrap_or(Role::None);
        let wall_kind = match (r.kind, role) {
            (RoomKind::Cavern, _) => LightKind::Crystal,
            (_, Role::Crypt | Role::Shrine | Role::Ritual) => LightKind::Candles,
            _ => st.wall_light,
        };
        // wall lights along the walls, where the wall is whole for 3 m (not an opening); with
        // pilasters (every `st.pilasters` m from half that), halfway between two of them
        if matches!(wall_kind.mount(), Mount::Wall) {
            let lp = geom::resample_loop(&r.shape.outline(), 0.5);
            let n = lp.len();
            let mut along = 0.0f32;
            let (spacing, mut next) = if st.pilasters > 0.0 {
                let step = st.pilasters * (spacing / st.pilasters).round().max(1.0);
                (step, st.pilasters)
            } else {
                (spacing, spacing * 0.5)
            };
            let mut side = false;
            for k in 0..n {
                let (a, b) = (lp[k], lp[(k + 1) % n]);
                let len = a.distance(b);
                if along + len >= next {
                    let m = a.lerp(b, ((next - along) / len.max(1e-4)).clamp(0.0, 1.0));
                    let nrm = (b - a).perp().normalize_or_zero();
                    let inward = if r.inside(m + nrm * 0.2) > 0.0 {
                        nrm
                    } else {
                        -nrm
                    };
                    let floor = r.floor_at(m + inward * 0.5);
                    let h = 2.5f32.min(r.height * 0.6);
                    // a solid stretch of wall: rock just outside for 1.5 m both ways
                    let dir = (b - a).normalize_or_zero();
                    let solid = [-1.5f32, 0.0, 1.5].iter().all(|s| {
                        !c.air(Vec3::new(
                            m.x + dir.x * s - inward.x * 0.3,
                            floor + h,
                            m.y + dir.y * s - inward.y * 0.3,
                        ))
                    });
                    // corridors: alternate sides
                    let skip = r.kind == RoomKind::Corridor && side;
                    if solid && !skip {
                        // the wall is drawn on the outline; the fixture's origin is its back plate
                        // (about 5 cm in front of its back), the flame ~0.3 m out from the wall
                        let at = m + inward * WALL_MOUNT;
                        let pos = Vec3::new(at.x, floor + h, at.y);
                        let yaw = inward.x.atan2(inward.y);
                        let mut l = LightDef::new(
                            0,
                            wall_kind,
                            pos + Vec3::new(inward.x, 0.0, inward.y) * 0.25,
                        );
                        l.auto = true;
                        l.room = Some(r.id);
                        l.pos[1] = pos.y + 0.55;
                        l.shadows = false;
                        let fx = wall_kind.fixture(st.pack).map(|(id, _)| {
                            (id.to_string(), Vec3::new(at.x, floor + h - 0.45, at.y), yaw)
                        });
                        out.push(Planned {
                            light: l,
                            fixture: fx,
                        });
                    }
                    if r.kind == RoomKind::Corridor {
                        side = !side;
                    }
                    next += spacing;
                }
                along += len;
            }
        }
        // floor lights: braziers round big rooms, candles in crypts, crystals in caverns
        let big = r.inr > 5.0;
        let c0 = r.shape.centre();
        let floor_kind = match (r.kind, role) {
            (RoomKind::Cavern, _) => Some(LightKind::Crystal),
            (_, Role::Boss | Role::Throne | Role::Arena | Role::Forge) => Some(LightKind::Brazier),
            (_, Role::Crypt | Role::Shrine | Role::Ritual) => Some(LightKind::Candles),
            (RoomKind::Hall, _) | (RoomKind::Room, _) if big => {
                Some(if st.hall_light == LightKind::Brazier {
                    LightKind::Brazier
                } else {
                    LightKind::Brazier
                })
            }
            _ => None,
        };
        if let Some(k) = floor_kind {
            let ring = (r.inr * 0.72).max(1.5);
            let n = match r.kind {
                RoomKind::Cavern => ((r.inr * r.inr) / 40.0).round().clamp(2.0, 8.0) as usize,
                _ if big => 4,
                _ => 2,
            };
            for i in 0..n {
                let a = std::f32::consts::TAU * (i as f32 + 0.5) / n as f32 + rng.range(-0.2, 0.2);
                let (ax, hl, hw) = r.shape.axes();
                // along the room's own shape: a ring scaled to its axes
                let q = c0
                    + ax * a.cos() * (hl * 0.72).max(1.0)
                    + ax.perp() * a.sin() * (hw * 0.72).max(1.0);
                let q = if r.inside(q) > 1.2 {
                    q
                } else {
                    c0.lerp(q, 0.6)
                };
                if r.inside(q) < 1.0 || !c.air(Vec3::new(q.x, r.floor_at(q) + 1.0, q.y)) {
                    continue;
                }
                let _ = ring;
                let fy = c
                    .floor_under(Vec3::new(q.x, r.floor_at(q) + 1.5, q.y), 3.0)
                    .unwrap_or(r.floor_at(q));
                let (_, _, _, _, _) = k.defaults();
                let lift = k.fixture(st.pack).map(|f| f.1).unwrap_or(1.0);
                let mut l = LightDef::new(0, k, Vec3::new(q.x, fy + lift, q.y));
                l.auto = true;
                l.room = Some(r.id);
                l.shadows = k != LightKind::Candles;
                let yaw = (c0 - q).x.atan2((c0 - q).y);
                let fx = k
                    .fixture(st.pack)
                    .map(|(id, _)| (id.to_string(), Vec3::new(q.x, fy, q.y), yaw));
                out.push(Planned {
                    light: l,
                    fixture: fx,
                });
            }
        }
        // overhead: a chandelier or hanging fire in halls; a shaft where the roof is open
        if matches!(r.kind, RoomKind::Hall) || (r.kind == RoomKind::Room && r.inr > 7.0) {
            let k = st.hall_light;
            let k = if k.mount() == Mount::Ceiling {
                k
            } else {
                LightKind::Chandelier
            };
            let (ax, hl, _) = r.shape.axes();
            let n = ((hl * 2.0) / 16.0).round().clamp(1.0, 4.0) as usize;
            for i in 0..n {
                let t = if n == 1 {
                    0.0
                } else {
                    -hl * 0.55 + hl * 1.1 * i as f32 / (n - 1) as f32
                };
                let q = c0 + ax * t;
                if r.inside(q) < 1.0 {
                    continue;
                }
                let top = r.ceil_at(q);
                let hang = if k == LightKind::Shaft {
                    top - 0.5
                } else {
                    (top - 2.2).max(r.floor_at(q) + 4.0)
                };
                let mut l = LightDef::new(0, k, Vec3::new(q.x, hang, q.y));
                l.auto = true;
                l.room = Some(r.id);
                if k == LightKind::Shaft {
                    l.dir = [0.3, 1.3];
                }
                let fx = k
                    .fixture(st.pack)
                    .map(|(id, lift)| (id.to_string(), Vec3::new(q.x, hang - lift, q.y), 0.0));
                out.push(Planned {
                    light: l,
                    fixture: fx,
                });
            }
        }
    }
    // keep them apart (overlapping rooms light the same wall once)
    let mut kept: Vec<Planned> = Vec::new();
    for p in out {
        let pos = p.light.position();
        if kept
            .iter()
            .any(|k| k.light.kind == p.light.kind && k.light.position().distance(pos) < 2.5)
        {
            continue;
        }
        kept.push(p);
    }
    kept
}

// ------------------------------------------------------------------ runtime

/// A spawned light.
#[derive(Component)]
pub struct DungeonLight {
    pub id: u32,
}

/// How many lights shine at once, and how many cast shadows (the nearest).
pub const BUDGET: usize = 96;
pub const SHADOWED: usize = 4;

/// How many of the nearest lights cast shadows, by the graphics' detail (each is a cube of
/// shadow maps).
pub fn shadow_budget(detail: f32) -> usize {
    if detail < 1.0 {
        3
    } else if detail < 1.5 {
        6
    } else {
        10
    }
}

/// Flicker: a fire's brightness over time (around 1).
pub fn flicker(t: f32, seed: u32, amount: f32) -> f32 {
    if amount <= 0.0 {
        return 1.0;
    }
    let s = seed as f32 * 0.137;
    let n = (t * 9.1 + s).sin() * 0.35
        + (t * 15.7 + s * 2.3).sin() * 0.25
        + (t * 4.3 + s * 0.7).sin() * 0.4;
    (1.0 + n * 0.22 * amount).max(0.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeons::plan::*;

    #[test]
    fn auto_light_lights_rooms_and_corridors() {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(8.0, 6.0)),
        ));
        plan.add(Room::new(
            0,
            RoomKind::Corridor,
            0,
            Shape::path(&[Vec2::new(7.0, 0.0), Vec2::new(40.0, 0.0)], 3.0),
        ));
        let c = Compiled::new(&plan, &|_| (0.6, false));
        let st = crate::dungeons::styles::get("crypt_stone");
        let lights = auto_light(&plan, &c, &|_| st, 1.0);
        assert!(lights.len() >= 4, "{} lights", lights.len());
        // every light is in the air, and wall lights are close to a wall
        for p in &lights {
            assert!(
                c.air(p.light.position()),
                "{:?} in the rock",
                p.light.position()
            );
        }
        assert!(
            lights
                .iter()
                .any(|p| p.light.kind == LightKind::Torch && p.fixture.is_some())
        );
    }
}
