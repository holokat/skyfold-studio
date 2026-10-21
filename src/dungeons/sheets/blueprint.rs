//! A dungeon blueprint: what a design sheet says, in metres — the zones (where, how big, how
//! high, which hold boss rings), the links between them (paths, stairs, bridges), the water and
//! chasms, the object key and where its objects stand, where players arrive and how the dungeon
//! ends, and the sheet's art direction. The builder (`build`) makes a realm from it; the reader
//! (`read`) makes it from a sheet.
//!
//! Coordinates are the sheet's: metres from the map's centre, x east, y north (the world's
//! +X and −Z). Elevations are metres above the Arrival area.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A point on the sheet, metres from its centre (x east, y north).
pub type P = [f32; 2];

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Blueprint {
    pub version: u32,
    /// the library id ("001_sundered_aqueduct") or a generated one
    pub id: String,
    pub name: String,
    /// the setting family: temperate, arid, cold, tropical, volcanic, twilight
    pub family: String,
    pub kind: Kind,
    /// a layout type id (`layout_types.json`)
    pub layout_type: String,
    /// the map's width (east–west) and depth (north–south), m
    pub size: [f32; 2],
    /// in route order (the Arrival first, the final zone last; side areas where they branch)
    pub zones: Vec<Zone>,
    pub links: Vec<Link>,
    pub water: Vec<Water>,
    pub chasms: Vec<Chasm>,
    /// the object key
    pub objects: Vec<ObjectKind>,
    /// where the key's objects stand (every copy the sheet shows)
    pub placements: Vec<Placement>,
    /// the player start and the way it faces (a unit vector)
    pub start: P,
    pub start_dir: P,
    pub ending: Ending,
    /// from an early vantage point to the finale's landmark
    pub sightline: Option<[P; 2]>,
    pub art: Art,
    /// the brief the sheet was made from (library sheets and generated ones)
    pub brief: Option<Value>,
    /// what the reader wasn't sure of (shown to the user, kept for re-reads)
    pub notes: Vec<String>,
    /// how the sheet's picture lies over the map (its water is read from the pixels)
    pub picture: Option<PictureFrame>,
}

/// Where the map is in the sheet's picture: the map's centre in pixels and the metres per pixel.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PictureFrame {
    pub centre_px: [f32; 2],
    pub m_per_px: f32,
    pub size_px: [u32; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Party,
    Raid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Arrival,
    #[default]
    Area,
    Boss,
    Final,
    /// the optional side area
    Side,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Rect,
    Round,
    /// `outline` gives it
    Poly,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Zone {
    /// its number on the sheet (01 = the Arrival)
    pub num: u8,
    pub name: String,
    pub role: Role,
    pub c: P,
    /// width (along its own x) × depth, m
    pub size: [f32; 2],
    /// its turn, degrees counter-clockwise from east
    pub turn: f32,
    pub shape: Shape,
    /// its floor's outline when `shape` is Poly (counter-clockwise)
    pub outline: Vec<P>,
    /// m above the Arrival
    pub elevation: f32,
    pub boss: Option<Ring>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ring {
    pub c: P,
    /// diameter, m
    pub d: f32,
    /// "Boss 1", "Final boss"
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    #[default]
    Path,
    Bridge,
    Causeway,
    /// the return path or the shortcut back (one-way)
    Return,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    /// zone numbers
    pub from: u8,
    pub to: u8,
    /// its centreline, from `from` to `to` (both ends at or inside the zones)
    pub pts: Vec<P>,
    pub width: f32,
    pub kind: LinkKind,
    /// its stairs and ramps, in order along it
    pub stairs: Vec<Stair>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stair {
    /// where along the link it starts, 0..1 of its length
    pub at: f32,
    /// m up (negative: down) going from `from` to `to`
    pub rise: f32,
    /// the object key's letter on the sheet ("D")
    pub tag: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Water {
    pub name: String,
    pub outline: Vec<P>,
    /// its surface, m above the Arrival
    pub level: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Chasm {
    pub name: String,
    pub outline: Vec<P>,
    /// its floor, m above the Arrival (negative)
    pub floor: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    #[default]
    Structure,
    Prop,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectKind {
    pub letter: String,
    pub name: String,
    pub group: Group,
    /// width × depth × height, m
    pub size: [f32; 3],
    pub count: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub letter: String,
    pub at: P,
    /// degrees counter-clockwise from east the object's front faces (None: face the zone's middle)
    pub facing: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndKind {
    /// an exit portal in the final arena
    #[default]
    Exit,
    /// a one-way shortcut (stair, slide, lift) back toward the start
    Shortcut,
    /// a one-way return path through a gate
    Return,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ending {
    pub kind: EndKind,
    /// the exit portal, the shortcut's head or the return gate
    pub at: Option<P>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Art {
    pub feel: String,
    pub materials: String,
    pub light: String,
    pub vfx: String,
    pub post: String,
}

impl Blueprint {
    pub fn zone(&self, num: u8) -> Option<&Zone> {
        self.zones.iter().find(|z| z.num == num)
    }
    pub fn object(&self, letter: &str) -> Option<&ObjectKind> {
        self.objects
            .iter()
            .find(|o| o.letter.eq_ignore_ascii_case(letter))
    }
    pub fn half(&self) -> [f32; 2] {
        [self.size[0] * 0.5, self.size[1] * 0.5]
    }
    /// The final zone (the last Final, else the last zone).
    pub fn finale(&self) -> Option<&Zone> {
        self.zones
            .iter()
            .rev()
            .find(|z| z.role == Role::Final)
            .or(self.zones.last())
    }

    /// What is wrong with it (empty: buildable). The builder refuses a blueprint with problems;
    /// the reader repairs what it can first (`read::repair`).
    pub fn problems(&self) -> Vec<String> {
        let mut bad = vec![];
        if !(self.size[0] >= 120.0
            && self.size[0] <= 640.0
            && self.size[1] >= 90.0
            && self.size[1] <= 520.0)
        {
            bad.push(format!(
                "map size {:?} m is out of range (120–640 × 90–520)",
                self.size
            ));
        }
        if self.zones.len() < 3 {
            bad.push(format!("{} zones (at least 3)", self.zones.len()));
        }
        if !self.zones.iter().any(|z| z.role == Role::Arrival) {
            bad.push("no Arrival zone".into());
        }
        if !self.zones.iter().any(|z| z.role == Role::Final) {
            bad.push("no Final zone".into());
        }
        let [hx, hy] = self.half();
        for z in &self.zones {
            if z.c[0].abs() > hx + 1.0 || z.c[1].abs() > hy + 1.0 {
                bad.push(format!(
                    "zone {:02} {} is off the map at {:?}",
                    z.num, z.name, z.c
                ));
            }
            if z.size[0] < 6.0 || z.size[1] < 6.0 {
                bad.push(format!(
                    "zone {:02} {} is too small ({:?} m)",
                    z.num, z.name, z.size
                ));
            }
            if !(-40.0..=80.0).contains(&z.elevation) {
                bad.push(format!(
                    "zone {:02} {} elevation {} m is out of range",
                    z.num, z.name, z.elevation
                ));
            }
        }
        for (i, a) in self.zones.iter().enumerate() {
            if self.zones[i + 1..].iter().any(|b| b.num == a.num) {
                bad.push(format!("zone number {:02} is used twice", a.num));
            }
        }
        // every zone is reached from the Arrival through the links
        if let Some(start) = self.zones.iter().find(|z| z.role == Role::Arrival) {
            let mut seen = vec![start.num];
            let mut grew = true;
            while grew {
                grew = false;
                for l in &self.links {
                    for (a, b) in [(l.from, l.to), (l.to, l.from)] {
                        if l.kind == LinkKind::Return && a == l.to {
                            continue;
                        }
                        if seen.contains(&a) && !seen.contains(&b) {
                            seen.push(b);
                            grew = true;
                        }
                    }
                }
            }
            for z in &self.zones {
                if !seen.contains(&z.num) {
                    bad.push(format!(
                        "zone {:02} {} is not reached by any link",
                        z.num, z.name
                    ));
                }
            }
        }
        for l in &self.links {
            if self.zone(l.from).is_none() || self.zone(l.to).is_none() {
                bad.push(format!(
                    "a link joins unknown zones {:02} → {:02}",
                    l.from, l.to
                ));
                continue;
            }
            if l.pts.len() < 2 {
                bad.push(format!("the link {:02} → {:02} has no path", l.from, l.to));
            }
            if l.kind == LinkKind::Return {
                continue;
            }
            // its stairs make up the height between its zones
            let (a, b) = (self.zone(l.from).unwrap(), self.zone(l.to).unwrap());
            let rise: f32 = l.stairs.iter().map(|s| s.rise).sum();
            if (rise - (b.elevation - a.elevation)).abs() > 0.6 {
                bad.push(format!(
                    "the link {:02} → {:02} rises {rise} m but its zones differ by {} m",
                    l.from,
                    l.to,
                    b.elevation - a.elevation
                ));
            }
        }
        for p in &self.placements {
            if self.object(&p.letter).is_none() {
                bad.push(format!(
                    "a placement uses the unknown key letter {}",
                    p.letter
                ));
            }
        }
        bad
    }

    pub fn summary(&self) -> Value {
        serde_json::json!({
            "id": self.id, "name": self.name, "family": self.family, "kind": self.kind, "layout_type": self.layout_type,
            "size_m": self.size,
            "zones": self.zones.iter().map(|z| serde_json::json!({ "num": z.num, "name": z.name, "role": z.role, "at": z.c, "size": z.size, "elevation": z.elevation, "boss": z.boss.as_ref().map(|b| &b.label) })).collect::<Vec<_>>(),
            "links": self.links.len(), "water": self.water.len(), "chasms": self.chasms.len(),
            "objects": self.objects.iter().map(|o| format!("{} {} ×{}", o.letter, o.name, o.count)).collect::<Vec<_>>(),
            "ending": self.ending.kind,
        })
    }
}
