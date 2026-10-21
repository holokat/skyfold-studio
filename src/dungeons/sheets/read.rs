//! Reading a design sheet into a blueprint. A vision model reads the sheet (`READ_PROMPT`) into
//! pixel-space JSON (`SheetRead`); `normalize` measures that into metres and `repair` makes it
//! buildable.
//!
//! Image models get a sheet's layout, topology and heights right but its metric scale wrong: on
//! real sheets the dimension lines, the scale bar and the zones' captioned sizes disagree by
//! 10–45%. So sizes and elevations come from the text (captions, ring diameters, the key),
//! positions and shapes from the picture, and one scale (metres per pixel) is fitted to every size
//! the text gives — robustly, so one wild caption or a stretched frame can't skew the map. The
//! model's pixel positions drift too, so the zone markers (solid red discs) are found in the image
//! itself (`find_markers`) and the reading's markers snapped to them.

use bevy::math::Vec2;
use image::RgbaImage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::blueprint::*;

/// What the vision model is asked (the sheet image goes with it).
pub const READ_PROMPT: &str = include_str!("read_prompt.md");

/// A point in the image's pixels: x from the left edge, y from the top.
pub type Px = [f32; 2];

/// The model's reading of a sheet, in the image's pixels, as `read_prompt.md` asks for it. Every
/// field is optional and forgiving: numbers may come as strings ("+4 m", "↓ 6 m"), zone numbers
/// as "02", points as [x, y] or {x, y}; anything missing or unreadable is left empty.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SheetRead {
    #[serde(deserialize_with = "de::text")]
    pub name: String,
    /// the painted map's border: [x0, y0, x1, y1]
    #[serde(deserialize_with = "de::rect")]
    pub frame_px: Option<[f32; 4]>,
    /// the dimension lines: [width, depth] m
    #[serde(deserialize_with = "de::pair")]
    pub frame_m: Option<[f32; 2]>,
    #[serde(deserialize_with = "de::list")]
    pub zones: Vec<ZoneRead>,
    #[serde(deserialize_with = "de::list")]
    pub links: Vec<LinkRead>,
    #[serde(deserialize_with = "de::list")]
    pub water: Vec<WaterRead>,
    #[serde(deserialize_with = "de::list")]
    pub chasms: Vec<ChasmRead>,
    #[serde(deserialize_with = "de::list")]
    pub objects: Vec<ObjectRead>,
    #[serde(deserialize_with = "de::pt")]
    pub start_px: Option<Px>,
    #[serde(deserialize_with = "de::pt")]
    pub start_toward_px: Option<Px>,
    #[serde(deserialize_with = "de::ending")]
    pub ending: Option<EndingRead>,
    #[serde(deserialize_with = "de::segment")]
    pub sightline_px: Option<[Px; 2]>,
    #[serde(deserialize_with = "de::art")]
    pub art: ArtRead,
    #[serde(deserialize_with = "de::texts")]
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ZoneRead {
    #[serde(deserialize_with = "de::byte")]
    pub num: u8,
    #[serde(deserialize_with = "de::text")]
    pub name: String,
    /// from its caption, m
    #[serde(deserialize_with = "de::pair")]
    pub size_m: Option<[f32; 2]>,
    /// from its caption, m (on the sheet's own datum; the Arrival is usually +0)
    #[serde(deserialize_with = "de::opt_num")]
    pub elevation_m: Option<f32>,
    /// the red disc's centre
    #[serde(deserialize_with = "de::pt")]
    pub marker_px: Option<Px>,
    /// its floor as drawn: [x0, y0, x1, y1]
    #[serde(deserialize_with = "de::rect")]
    pub box_px: Option<[f32; 4]>,
    #[serde(deserialize_with = "de::flag")]
    pub round: bool,
    #[serde(deserialize_with = "de::obj")]
    pub boss: Option<BossRead>,
    #[serde(deserialize_with = "de::flag")]
    pub side: bool,
    #[serde(rename = "final", deserialize_with = "de::flag")]
    pub finale: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BossRead {
    #[serde(deserialize_with = "de::text")]
    pub label: String,
    #[serde(deserialize_with = "de::opt_num")]
    pub diameter_m: Option<f32>,
    #[serde(deserialize_with = "de::pt")]
    pub center_px: Option<Px>,
    #[serde(deserialize_with = "de::opt_num")]
    pub radius_px: Option<f32>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkRead {
    #[serde(deserialize_with = "de::byte")]
    pub from: u8,
    #[serde(deserialize_with = "de::byte")]
    pub to: u8,
    #[serde(deserialize_with = "de::pts")]
    pub points_px: Vec<Px>,
    /// "path", "bridge", "causeway" or "return"
    #[serde(deserialize_with = "de::text")]
    pub kind: String,
    #[serde(deserialize_with = "de::opt_num")]
    pub width_m: Option<f32>,
    #[serde(deserialize_with = "de::list")]
    pub stairs: Vec<StairRead>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StairRead {
    #[serde(deserialize_with = "de::pt")]
    pub at_px: Option<Px>,
    /// m up (negative: down) in walking direction
    #[serde(deserialize_with = "de::num")]
    pub rise_m: f32,
    #[serde(deserialize_with = "de::text")]
    pub tag: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterRead {
    #[serde(deserialize_with = "de::text")]
    pub name: String,
    #[serde(deserialize_with = "de::opt_num")]
    pub level_m: Option<f32>,
    #[serde(deserialize_with = "de::pts")]
    pub outline_px: Vec<Px>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChasmRead {
    #[serde(deserialize_with = "de::text")]
    pub name: String,
    #[serde(deserialize_with = "de::opt_num")]
    pub floor_m: Option<f32>,
    #[serde(deserialize_with = "de::pts")]
    pub outline_px: Vec<Px>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ObjectRead {
    #[serde(deserialize_with = "de::text")]
    pub letter: String,
    #[serde(deserialize_with = "de::text")]
    pub name: String,
    /// "structure" or "prop"
    #[serde(deserialize_with = "de::text")]
    pub group: String,
    /// [w, d, h] m
    #[serde(deserialize_with = "de::nums")]
    pub size_m: Vec<f32>,
    #[serde(deserialize_with = "de::count")]
    pub count: u32,
    #[serde(deserialize_with = "de::pts")]
    pub at_px: Vec<Px>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EndingRead {
    /// "exit", "shortcut" or "return"
    #[serde(deserialize_with = "de::text")]
    pub kind: String,
    #[serde(deserialize_with = "de::pt")]
    pub at_px: Option<Px>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArtRead {
    #[serde(deserialize_with = "de::text")]
    pub feel: String,
    #[serde(deserialize_with = "de::text")]
    pub materials: String,
    #[serde(deserialize_with = "de::text")]
    pub light: String,
    #[serde(deserialize_with = "de::text")]
    pub vfx: String,
    #[serde(deserialize_with = "de::text")]
    pub post: String,
}

/// The model's reply → its reading. Forgiving: a ```json fence or prose round the object, a
/// typographic minus (−4), a leading plus (+4) or a trailing comma outside strings are all fine.
pub fn parse(text: &str) -> Result<SheetRead, String> {
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else {
        return Err("no JSON object in the reply".into());
    };
    if b < a {
        return Err("no JSON object in the reply".into());
    }
    let body = &text[a..=b];
    serde_json::from_str(body)
        .or_else(|_| serde_json::from_str(&tidy_json(body)))
        .map_err(|e| format!("the reading isn't valid JSON: {e}"))
}

/// Fixes what models write into JSON numbers outside strings: "−4" → "-4", "+4" → "4", and drops
/// trailing commas.
fn tidy_json(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let (mut in_str, mut esc) = (false, false);
    for (i, &c) in cs.iter().enumerate() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        let next = cs[i + 1..].iter().find(|c| !c.is_whitespace()).copied();
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '−' | '–' => out.push('-'),
            '+' if next.is_some_and(|n| n.is_ascii_digit() || n == '.') => {}
            ',' if matches!(next, Some('}') | Some(']')) => {}
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The zone markers in the image

/// The centres (px) of the solid pure-red zone-marker discs in the sheet's map (its left ~72%; the
/// sidebar has red legend glyphs). A window about a disc's size slides over a red mask; windows
/// mostly red are discs (a dashed route line or a ring's dashes fill a small part of one), one per
/// 25 px, each refined to the centroid of the red round it. Sizes scale with the image (tuned on
/// 1536-wide sheets).
pub fn find_markers(img: &RgbaImage) -> Vec<[f32; 2]> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let k = w as f32 / 1536.0;
    let win = ((22.0 * k).round() as usize).max(5);
    let xmax = ((w as f32 * 0.72) as usize).min(w);
    if xmax < win * 2 || h < win * 2 {
        return vec![];
    }
    // an integral image of the red mask over the map part
    let raw = img.as_raw();
    let iw = xmax + 1;
    let mut ii = vec![0u32; iw * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..xmax {
            let p = &raw[(y * w + x) * 4..(y * w + x) * 4 + 3];
            if p[0] > 170 && p[1] < 80 && p[2] < 80 {
                row += 1;
            }
            ii[(y + 1) * iw + x + 1] = ii[y * iw + x + 1] + row;
        }
    }
    // red pixels in [x0, x1) × [y0, y1), clipped to the map part
    let red = |x0: i64, y0: i64, x1: i64, y1: i64| -> u32 {
        let cx = |x: i64| x.clamp(0, xmax as i64) as usize;
        let cy = |y: i64| y.clamp(0, h as i64) as usize;
        let (x0, x1, y0, y1) = (cx(x0), cx(x1), cy(y0), cy(y1));
        ii[y1 * iw + x1] + ii[y0 * iw + x0] - ii[y0 * iw + x1] - ii[y1 * iw + x0]
    };
    let need = (0.55 * (win * win) as f32) as u32;
    let mut found: Vec<(u32, usize, usize)> = vec![];
    for y in 0..=h - win {
        for x in 0..=xmax - win {
            let n = red(x as i64, y as i64, (x + win) as i64, (y + win) as i64);
            if n > need {
                found.push((n, x, y));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
    let apart = 25.0 * k;
    let mut kept: Vec<[f32; 2]> = vec![];
    for (_, x, y) in found {
        let c = [x as f32 + win as f32 * 0.5, y as f32 + win as f32 * 0.5];
        if kept.iter().all(|q| dist(*q, c) > apart) {
            kept.push(c);
        }
    }
    let mut discs: Vec<[f32; 2]> = vec![];
    let r = win as i64;
    for c in kept {
        let (x0, y0) = (c[0] as i64 - r, c[1] as i64 - r);
        // a big red area (a banner, a roof) fills the box round a candidate; a disc doesn't
        if red(x0, y0, x0 + 2 * r, y0 + 2 * r) as f32 > 0.65 * (4 * r * r) as f32 {
            continue;
        }
        // the centroid of the red within a disc-sized circle, a few times over (so the route's
        // dashes running into the disc barely pull it)
        let mut c = c;
        let reach = 0.75 * win as f32;
        for _ in 0..4 {
            let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
            let (cx, cy) = (c[0] as i64, c[1] as i64);
            let rr = reach.ceil() as i64;
            for y in (cy - rr).max(0)..(cy + rr + 1).min(h as i64) {
                for x in (cx - rr).max(0)..(cx + rr + 1).min(xmax as i64) {
                    let (dx, dy) = (x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]);
                    if dx * dx + dy * dy > reach * reach {
                        continue;
                    }
                    let p = &raw
                        [(y as usize * w + x as usize) * 4..(y as usize * w + x as usize) * 4 + 3];
                    if p[0] > 170 && p[1] < 80 && p[2] < 80 {
                        sx += x as f64;
                        sy += y as f64;
                        n += 1.0;
                    }
                }
            }
            if n > 0.0 {
                c = [(sx / n) as f32 + 0.5, (sy / n) as f32 + 0.5];
            }
        }
        if discs.iter().all(|q| dist(*q, c) > apart * 0.5) {
            discs.push(c);
        }
    }
    discs
}

// ---------------------------------------------------------------------------------------------
// Reading → blueprint

/// Pixels → metres on one sheet: one scale about the frame's centre, y flipped (the image's y
/// runs down, the sheet's north is up).
#[derive(Clone, Copy)]
struct ToMetres {
    c: Vec2,
    s: f32,
}

impl ToMetres {
    fn p(&self, q: Px) -> P {
        [(q[0] - self.c.x) * self.s, (self.c.y - q[1]) * self.s]
    }
}

/// The reading → a blueprint in metres (and notes on what was measured, guessed or fixed). With
/// the sheet's image the zone markers are snapped to the red discs found in it; with the brief the
/// sheet was made from, its id, name, family, kind and layout type are kept.
pub fn normalize(
    read: &SheetRead,
    img: Option<&RgbaImage>,
    brief: Option<&Value>,
) -> Result<(Blueprint, Vec<String>), String> {
    let mut notes: Vec<String> = vec![];
    if read.zones.is_empty() {
        return Err("the reading has no zones".into());
    }
    let frame = frame_of(read, img, &mut notes);
    let markers = snap_markers(read, img, &mut notes);
    let s = fit_scale(read, frame, &mut notes);
    let (fw, fd) = (frame[2] - frame[0], frame[3] - frame[1]);
    let to_m = ToMetres {
        c: Vec2::new((frame[0] + frame[2]) * 0.5, (frame[1] + frame[3]) * 0.5),
        s,
    };
    let mut bp = Blueprint {
        version: VERSION,
        size: [round10(fw * s), round10(fd * s)],
        ..Default::default()
    };
    bp.picture = Some(super::blueprint::PictureFrame {
        centre_px: [to_m.c.x, to_m.c.y],
        m_per_px: s,
        size_px: img.map_or([0, 0], |i| [i.width(), i.height()]),
    });

    // zone numbers (a missing one is its place in the list)
    let nums: Vec<u8> = read
        .zones
        .iter()
        .enumerate()
        .map(|(i, z)| {
            if z.num > 0 {
                z.num
            } else {
                (i + 1).min(255) as u8
            }
        })
        .collect();
    for (i, z) in read.zones.iter().enumerate() {
        if z.num == 0 {
            notes.push(format!(
                "zone \"{}\" had no number: {:02} by its place",
                z.name, nums[i]
            ));
        }
    }
    let arrival = if nums.contains(&1) {
        1
    } else {
        *nums.iter().min().unwrap()
    };
    let finale = read
        .zones
        .iter()
        .zip(&nums)
        .filter(|(z, n)| z.finale && **n != arrival)
        .map(|(_, n)| *n)
        .last()
        .or_else(|| {
            read.zones
                .iter()
                .zip(&nums)
                .filter(|(z, n)| z.boss.is_some() && **n != arrival)
                .map(|(_, n)| *n)
                .max()
        });
    let heights = zone_heights(read, &nums, &mut notes);

    for (i, zr) in read.zones.iter().enumerate() {
        let num = nums[i];
        let label = format!("zone {:02} {}", num, zr.name);
        let Some(centre) = zr
            .box_px
            .map(|b| [(b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5])
            .or(markers[i])
        else {
            notes.push(format!(
                "{label} has neither a floor nor a marker on the sheet: left out"
            ));
            continue;
        };
        let drawn = zr.box_px.map(|b| [(b[2] - b[0]) * s, (b[3] - b[1]) * s]);
        let mut size = match (zr.size_m.filter(|q| q[0] > 0.0 && q[1] > 0.0), drawn) {
            (Some(q), _) => q,
            (None, Some(d)) => {
                notes.push(format!(
                    "{label} has no captioned size: {:.0} × {:.0} m as drawn",
                    d[0], d[1]
                ));
                d
            }
            (None, None) => {
                notes.push(format!("{label} has no size: 30 × 30 m assumed"));
                [30.0, 30.0]
            }
        };
        if let Some(d) = drawn.filter(|d| d[0] > 0.0 && d[1] > 0.0) {
            // a caption reads "W × D", but which way round is the drawing's call
            let aspect = (d[0] / d[1]).ln();
            let (kept, swapped) = (
                ((size[0] / size[1]).ln() - aspect).abs(),
                ((size[1] / size[0]).ln() - aspect).abs(),
            );
            if aspect.abs() > 0.1 && swapped + 0.05 < kept {
                size = [size[1], size[0]];
            }
            let off = ((size[0] * size[1]) / (d[0] * d[1])).sqrt();
            if !(0.67..=1.5).contains(&off) {
                notes.push(format!(
                    "{label} is captioned {:.0} × {:.0} m but drawn {:.0} × {:.0} m at the fitted scale (the caption wins)",
                    size[0], size[1], d[0], d[1]
                ));
            }
        }
        if let (Some(m), Some(b)) = (markers[i], zr.box_px) {
            if m[0] < b[0] - 10.0 || m[0] > b[2] + 10.0 || m[1] < b[1] - 10.0 || m[1] > b[3] + 10.0
            {
                notes.push(format!("{label}'s marker lies outside its floor"));
            }
        }
        let c = to_m.p(centre);
        let boss = zr.boss.as_ref().map(|b| Ring {
            c: b.center_px.map(|q| to_m.p(q)).unwrap_or(c),
            d: b.diameter_m
                .filter(|d| *d > 0.0)
                .or(b.radius_px.filter(|r| *r > 0.0).map(|r| 2.0 * r * s))
                .unwrap_or(0.45 * size[0].min(size[1])),
            label: if !b.label.is_empty() {
                b.label.clone()
            } else if Some(num) == finale {
                "Final boss".into()
            } else {
                "Boss".into()
            },
        });
        let role = if num == arrival {
            Role::Arrival
        } else if Some(num) == finale {
            Role::Final
        } else if boss.is_some() {
            Role::Boss
        } else if zr.side {
            Role::Side
        } else {
            Role::Area
        };
        let shape = if zr.round { Shape::Round } else { Shape::Rect };
        bp.zones.push(Zone {
            num,
            name: zr.name.clone(),
            role,
            c,
            size,
            turn: 0.0,
            shape,
            outline: vec![],
            elevation: heights[i],
            boss,
        });
    }
    if bp.zones.is_empty() {
        return Err("no zone could be placed".into());
    }

    // heights above the Arrival
    let base = bp.zone(arrival).map(|z| z.elevation).unwrap_or(0.0);
    if base != 0.0 {
        notes.push(format!(
            "elevations made relative to the Arrival ({base:+} m on the sheet)"
        ));
        for z in &mut bp.zones {
            z.elevation -= base;
        }
    }

    // route order: by number, side areas right after the zone they branch from, the finale last
    bp.zones.sort_by_key(|z| z.num);
    let sides: Vec<u8> = bp
        .zones
        .iter()
        .filter(|z| z.role == Role::Side)
        .map(|z| z.num)
        .collect();
    for side in sides {
        let parent = read
            .links
            .iter()
            .find(|l| l.to == side && l.from != side)
            .map(|l| l.from)
            .or_else(|| {
                read.links
                    .iter()
                    .find(|l| l.from == side && l.to != side)
                    .map(|l| l.to)
            });
        let Some(parent) = parent else { continue };
        let Some(i) = bp.zones.iter().position(|z| z.num == side) else {
            continue;
        };
        let z = bp.zones.remove(i);
        let at = bp
            .zones
            .iter()
            .position(|z| z.num == parent)
            .map(|p| p + 1)
            .unwrap_or(i);
        bp.zones.insert(at.min(bp.zones.len()), z);
    }
    if let Some(i) = bp.zones.iter().position(|z| z.role == Role::Final) {
        let z = bp.zones.remove(i);
        bp.zones.push(z);
    }

    for lr in &read.links {
        if bp.zone(lr.from).is_none() || bp.zone(lr.to).is_none() || lr.from == lr.to {
            notes.push(format!(
                "a link {:02} → {:02} joins unknown zones: left out",
                lr.from, lr.to
            ));
            continue;
        }
        let kind = link_kind(&lr.kind);
        let pts: Vec<P> = lr.points_px.iter().map(|q| to_m.p(*q)).collect();
        let width = lr
            .width_m
            .filter(|w| (2.0..=40.0).contains(w))
            .unwrap_or(if kind == LinkKind::Return { 6.0 } else { 9.0 });
        let n = lr.stairs.len();
        let mut stairs: Vec<Stair> = lr
            .stairs
            .iter()
            .enumerate()
            .map(|(k, st)| Stair {
                at: st
                    .at_px
                    .filter(|_| pts.len() >= 2)
                    .map(|q| along(&pts, to_m.p(q)))
                    .unwrap_or((k + 1) as f32 / (n + 1) as f32),
                rise: st.rise_m,
                tag: key_letter(&st.tag),
            })
            .collect();
        stairs.sort_by(|a, b| a.at.total_cmp(&b.at));
        bp.links.push(Link {
            from: lr.from,
            to: lr.to,
            pts,
            width,
            kind,
            stairs,
        });
    }

    for w in &read.water {
        if w.outline_px.len() < 3 {
            notes.push(format!("water \"{}\" has no outline: left out", w.name));
            continue;
        }
        let level = w.level_m.map(|l| l - base).unwrap_or_else(|| {
            notes.push(format!("water \"{}\" has no level: −2 m assumed", w.name));
            -2.0
        });
        bp.water.push(Water {
            name: w.name.clone(),
            outline: w.outline_px.iter().map(|q| to_m.p(*q)).collect(),
            level,
        });
    }
    for c in &read.chasms {
        if c.outline_px.len() < 3 {
            notes.push(format!("chasm \"{}\" has no outline: left out", c.name));
            continue;
        }
        let floor = c.floor_m.map(|l| l - base).unwrap_or_else(|| {
            notes.push(format!("chasm \"{}\" has no depth: −10 m assumed", c.name));
            -10.0
        });
        bp.chasms.push(Chasm {
            name: c.name.clone(),
            outline: c.outline_px.iter().map(|q| to_m.p(*q)).collect(),
            floor,
        });
    }

    for o in &read.objects {
        let letter = key_letter(&o.letter);
        if letter.is_empty() {
            notes.push(format!("key line \"{}\" has no letter: left out", o.name));
            continue;
        }
        let v = &o.size_m;
        let size = match v.len() {
            0 => {
                notes.push(format!(
                    "key {letter} {} has no size: 2 × 2 × 2 m assumed",
                    o.name
                ));
                [2.0, 2.0, 2.0]
            }
            1 => [v[0]; 3],
            2 => [v[0], v[1], v[0].min(v[1])],
            _ => [v[0], v[1], v[2]],
        };
        let group = match o.group.to_lowercase() {
            g if g.contains("prop") => Group::Prop,
            g if g.contains("struct") => Group::Structure,
            _ if size.iter().cloned().fold(0.0, f32::max) >= 6.0 => Group::Structure,
            _ => Group::Prop,
        };
        bp.objects.push(ObjectKind {
            letter: letter.clone(),
            name: o.name.clone(),
            group,
            size,
            count: o.count.max(o.at_px.len() as u32).max(1),
        });
        for q in &o.at_px {
            bp.placements.push(Placement {
                letter: letter.clone(),
                at: to_m.p(*q),
                facing: None,
            });
        }
    }

    let first = bp.zones.first().map(|z| z.c).unwrap_or_default();
    bp.start = read.start_px.map(|q| to_m.p(q)).unwrap_or_else(|| {
        notes.push("no player start: the Arrival's middle".into());
        bp.zone(arrival).map(|z| z.c).unwrap_or(first)
    });
    let toward = read
        .start_toward_px
        .map(|q| to_m.p(q))
        .filter(|t| dist(*t, bp.start) > 0.5)
        .or_else(|| {
            bp.zone(2)
                .or(bp.zones.iter().find(|z| z.num != arrival))
                .map(|z| z.c)
        });
    bp.start_dir = toward
        .map(|t| unit(Vec2::from(t) - Vec2::from(bp.start)))
        .unwrap_or([0.0, 1.0]);

    bp.ending = Ending {
        kind: match read
            .ending
            .as_ref()
            .map(|e| e.kind.to_lowercase())
            .unwrap_or_default()
        {
            k if k.contains("shortcut") => EndKind::Shortcut,
            k if k.contains("return") || k.contains("gate") => EndKind::Return,
            k if k.contains("exit") || k.contains("portal") => EndKind::Exit,
            _ if bp.links.iter().any(|l| l.kind == LinkKind::Return) => EndKind::Return,
            _ => EndKind::Exit,
        },
        at: read
            .ending
            .as_ref()
            .and_then(|e| e.at_px)
            .map(|q| to_m.p(q)),
    };
    bp.sightline = read.sightline_px.map(|[a, b]| [to_m.p(a), to_m.p(b)]);
    let a = &read.art;
    bp.art = Art {
        feel: a.feel.clone(),
        materials: a.materials.clone(),
        light: a.light.clone(),
        vfx: a.vfx.clone(),
        post: a.post.clone(),
    };

    bp.name = if read.name.is_empty() {
        "Unnamed sheet".into()
    } else {
        read.name.clone()
    };
    bp.id = slug(&bp.name);
    if let Some(b) = brief {
        let field = |k: &str| b.get(k).map(text_of).unwrap_or_default();
        for (k, slot) in [
            ("id", &mut bp.id),
            ("name", &mut bp.name),
            ("family", &mut bp.family),
            ("type", &mut bp.layout_type),
        ] {
            let v = field(k);
            if !v.is_empty() {
                *slot = v;
            }
        }
        bp.kind = if field("kind").eq_ignore_ascii_case("raid") {
            Kind::Raid
        } else {
            Kind::Party
        };
        bp.brief = Some(b.clone());
    }

    notes.extend(repair(&mut bp));
    bp.notes = read.notes.iter().chain(&notes).cloned().collect();
    Ok((bp, notes))
}

/// The map's frame in px: as read, else the extent of everything read (padded).
fn frame_of(read: &SheetRead, img: Option<&RgbaImage>, notes: &mut Vec<String>) -> [f32; 4] {
    if let Some(f) = read
        .frame_px
        .filter(|f| f[2] - f[0] >= 50.0 && f[3] - f[1] >= 50.0)
    {
        return f;
    }
    let mut pts: Vec<Px> = vec![];
    for z in &read.zones {
        pts.extend(z.marker_px);
        if let Some(b) = z.box_px {
            pts.extend([[b[0], b[1]], [b[2], b[3]]]);
        }
    }
    for l in &read.links {
        pts.extend(&l.points_px);
    }
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in pts {
        lo = lo.min(Vec2::from(p));
        hi = hi.max(Vec2::from(p));
    }
    notes.push("no map frame read: the extent of the zones and paths is taken as the map".into());
    if lo.x >= hi.x || lo.y >= hi.y {
        let (w, h) = img
            .map(|i| (i.width() as f32, i.height() as f32))
            .unwrap_or((1536.0, 1024.0));
        return [0.03 * w, 0.1 * h, 0.7 * w, 0.9 * h];
    }
    let pad = (hi - lo) * 0.08 + Vec2::splat(10.0);
    [lo.x - pad.x, lo.y - pad.y, hi.x + pad.x, hi.y + pad.y]
}

/// The zones' markers, snapped to the red discs found in the image (nearest first, each disc
/// used once, within 60 px of a 1536-wide sheet).
fn snap_markers(
    read: &SheetRead,
    img: Option<&RgbaImage>,
    notes: &mut Vec<String>,
) -> Vec<Option<Px>> {
    let mut out: Vec<Option<Px>> = read.zones.iter().map(|z| z.marker_px).collect();
    let Some(img) = img else { return out };
    let discs = find_markers(img);
    let reach = 60.0 * img.width() as f32 / 1536.0;
    let mut pairs: Vec<(f32, usize, usize)> = vec![];
    for (i, z) in read.zones.iter().enumerate() {
        let Some(m) = z.marker_px else { continue };
        for (j, d) in discs.iter().enumerate() {
            let e = dist(m, *d);
            if e <= reach {
                pairs.push((e, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut zone_done, mut disc_done) = (vec![false; read.zones.len()], vec![false; discs.len()]);
    let (mut snapped, mut moved) = (0, 0.0f32);
    for (e, i, j) in pairs {
        if zone_done[i] || disc_done[j] {
            continue;
        }
        out[i] = Some(discs[j]);
        zone_done[i] = true;
        disc_done[j] = true;
        snapped += 1;
        moved = moved.max(e);
    }
    notes.push(format!(
        "{snapped} of {} zone markers snapped to the {} red discs found (moved up to {moved:.0} px)",
        read.zones.len(),
        discs.len()
    ));
    for (i, z) in read.zones.iter().enumerate() {
        if !zone_done[i] {
            notes.push(format!(
                "zone {:02} {}: no red disc near its marker",
                z.num, z.name
            ));
        }
    }
    for (j, d) in discs.iter().enumerate() {
        if !disc_done[j] {
            notes.push(format!(
                "a red disc at ({:.0}, {:.0}) px is no zone's marker",
                d[0], d[1]
            ));
        }
    }
    out
}

/// Metres per pixel, fitted to every size the text gives: each zone's caption against its drawn
/// floor (longer side to longer side), each boss ring's diameter against its drawn ring, and the
/// dimension lines against the frame (at half weight — they are often the most stretched). The
/// weighted median, then the median again of the cues within 35% of it.
fn fit_scale(read: &SheetRead, frame: [f32; 4], notes: &mut Vec<String>) -> f32 {
    let mut cues: Vec<(f32, f32, String)> = vec![];
    for z in &read.zones {
        if let (Some([w, d]), Some(b)) = (z.size_m, z.box_px) {
            let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
            if w > 0.0 && d > 0.0 && bw > 2.0 && bh > 2.0 {
                cues.push((
                    w.max(d) / bw.max(bh),
                    1.0,
                    format!("zone {:02} length", z.num),
                ));
                cues.push((
                    w.min(d) / bw.min(bh),
                    1.0,
                    format!("zone {:02} breadth", z.num),
                ));
            }
        }
        if let Some(b) = &z.boss {
            if let (Some(d), Some(r)) = (b.diameter_m, b.radius_px) {
                if d > 0.0 && r > 2.0 {
                    cues.push((d / (2.0 * r), 1.0, format!("zone {:02} ring", z.num)));
                }
            }
        }
    }
    let (fw, fd) = (frame[2] - frame[0], frame[3] - frame[1]);
    if let Some([mw, md]) = read.frame_m {
        if mw > 0.0 {
            cues.push((mw / fw, 0.5, "frame width".into()));
        }
        if md > 0.0 {
            cues.push((md / fd, 0.5, "frame depth".into()));
        }
    }
    cues.retain(|c| c.0.is_finite() && c.0 > 0.0);
    if cues.is_empty() {
        notes.push("no sizes to measure the sheet by: the map is taken as 280 m wide".into());
        return 280.0 / fw;
    }
    let all: Vec<(f32, f32)> = cues.iter().map(|c| (c.0, c.1)).collect();
    let m0 = weighted_median(&all);
    let near: Vec<(f32, f32)> = all
        .iter()
        .copied()
        .filter(|c| (c.0 / m0 - 1.0).abs() <= 0.35)
        .collect();
    let s = if near.is_empty() {
        m0
    } else {
        weighted_median(&near)
    };
    let (lo, hi) = near.iter().fold((f32::MAX, f32::MIN), |(lo, hi), c| {
        (lo.min(c.0), hi.max(c.0))
    });
    let mut note = format!(
        "scale {s:.3} m/px from {} of {} size cues (spread {:+.0}% … {:+.0}%)",
        near.len(),
        cues.len(),
        (lo / s - 1.0) * 100.0,
        (hi / s - 1.0) * 100.0
    );
    let off: Vec<String> = cues
        .iter()
        .filter(|c| (c.0 / m0 - 1.0).abs() > 0.35)
        .map(|c| format!("{} {:+.0}%", c.2, (c.0 / s - 1.0) * 100.0))
        .collect();
    if !off.is_empty() {
        note += &format!("; set aside: {}", off.join(", "));
    }
    notes.push(note);
    s
}

fn weighted_median(v: &[(f32, f32)]) -> f32 {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = v.iter().map(|c| c.1).sum::<f32>() * 0.5;
    let mut acc = 0.0;
    for (i, (x, w)) in v.iter().enumerate() {
        acc += w;
        if (acc - half).abs() < 1e-4 {
            return v.get(i + 1).map(|n| (x + n.0) * 0.5).unwrap_or(*x);
        }
        if acc > half {
            return *x;
        }
    }
    v.last().map(|c| c.0).unwrap_or(1.0)
}

/// Each zone's elevation: its caption's, else carried along the links' stairs from a neighbour
/// whose is known, else +0 m.
fn zone_heights(read: &SheetRead, nums: &[u8], notes: &mut Vec<String>) -> Vec<f32> {
    let mut known: Vec<Option<f32>> = read.zones.iter().map(|z| z.elevation_m).collect();
    let idx = |n: u8| nums.iter().position(|m| *m == n);
    let mut grew = true;
    while grew {
        grew = false;
        for l in read
            .links
            .iter()
            .filter(|l| link_kind(&l.kind) != LinkKind::Return)
        {
            let (Some(a), Some(b)) = (idx(l.from), idx(l.to)) else {
                continue;
            };
            let rise: f32 = l.stairs.iter().map(|s| s.rise_m).sum();
            let (fill, h) = match (known[a], known[b]) {
                (Some(h), None) => (b, h + rise),
                (None, Some(h)) => (a, h - rise),
                _ => continue,
            };
            known[fill] = Some(h);
            notes.push(format!(
                "zone {:02} has no elevation: {h:+} m from its stairs",
                nums[fill]
            ));
            grew = true;
        }
    }
    known
        .iter()
        .enumerate()
        .map(|(i, h)| {
            h.unwrap_or_else(|| {
                notes.push(format!(
                    "zone {:02} has no elevation: +0 m assumed",
                    nums[i]
                ));
                0.0
            })
        })
        .collect()
}

fn link_kind(s: &str) -> LinkKind {
    let s = s.to_lowercase();
    if s.contains("return") || s.contains("shortcut") {
        LinkKind::Return
    } else if s.contains("bridge") || s.contains("deck") {
        LinkKind::Bridge
    } else if s.contains("causeway") {
        LinkKind::Causeway
    } else {
        LinkKind::Path
    }
}

/// "d", " D ", "D." → "D".
fn key_letter(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_uppercase()
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

fn round10(x: f32) -> f32 {
    (x / 10.0).round() * 10.0
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    Vec2::from(a).distance(Vec2::from(b))
}

fn unit(v: Vec2) -> P {
    let u = v.normalize_or_zero();
    if u == Vec2::ZERO {
        [0.0, 1.0]
    } else {
        u.to_array()
    }
}

/// How far along a polyline (0..1 of its length) the point nearest `p` is.
fn along(pts: &[P], p: P) -> f32 {
    let p = Vec2::from(p);
    let total: f32 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    if total <= 1e-3 {
        return 0.5;
    }
    let (mut best, mut best_d, mut run) = (0.0, f32::MAX, 0.0);
    for w in pts.windows(2) {
        let (a, b) = (Vec2::from(w[0]), Vec2::from(w[1]));
        let len = a.distance(b);
        let t = if len > 1e-4 {
            ((p - a).dot(b - a) / (len * len)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = p.distance(a + (b - a) * t);
        if d < best_d {
            best_d = d;
            best = run + t * len;
        }
        run += len;
    }
    (best / total).clamp(0.0, 1.0)
}

fn length(pts: &[P]) -> f32 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

// ---------------------------------------------------------------------------------------------
// Repair

/// Half a zone's footprint along x and y (its turned rectangle's bounds).
fn half_extent(z: &Zone) -> Vec2 {
    let h = Vec2::new(z.size[0], z.size[1]) * 0.5;
    if z.shape == Shape::Round {
        return h;
    }
    let (s, c) = z.turn.to_radians().sin_cos();
    Vec2::new(c.abs() * h.x + s.abs() * h.y, s.abs() * h.x + c.abs() * h.y)
}

fn floor_area(z: &Zone) -> f32 {
    let a = z.size[0] * z.size[1];
    if z.shape == Shape::Round {
        a * std::f32::consts::FRAC_PI_4
    } else {
        a
    }
}

/// A point in the zone's own frame (x along its width).
fn local(z: &Zone, p: Vec2) -> Vec2 {
    let d = p - Vec2::from(z.c);
    let (s, c) = (-z.turn.to_radians()).sin_cos();
    Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y)
}

fn world(z: &Zone, l: Vec2) -> Vec2 {
    let (s, c) = z.turn.to_radians().sin_cos();
    Vec2::from(z.c) + Vec2::new(c * l.x - s * l.y, s * l.x + c * l.y)
}

/// Whether `p` is on the zone's floor (or within `tol` m of it).
fn on_floor(z: &Zone, p: Vec2, tol: f32) -> bool {
    let l = local(z, p);
    let h = Vec2::new(z.size[0], z.size[1]) * 0.5 + Vec2::splat(tol);
    match z.shape {
        Shape::Round => (l.x / h.x).powi(2) + (l.y / h.y).powi(2) <= 1.0,
        _ => l.x.abs() <= h.x && l.y.abs() <= h.y,
    }
}

/// How much two zones' footprints overlap (m²) with `b` shifted by `shift`.
fn overlap(a: &Zone, b: &Zone, shift: Vec2) -> f32 {
    let d = (Vec2::from(b.c) + shift - Vec2::from(a.c)).abs();
    let o = (half_extent(a) + half_extent(b) - d).max(Vec2::ZERO);
    o.x * o.y
}

/// Moves a zone and what stands on it: its ring and outline, the ends of its links that were on
/// it, the objects, the start and the ending on it.
fn move_zone(bp: &mut Blueprint, i: usize, delta: Vec2) {
    if delta.length_squared() < 1e-6 {
        return;
    }
    let before = bp.zones[i].clone();
    let shift = move |p: &mut P| {
        p[0] += delta.x;
        p[1] += delta.y;
    };
    let on = |p: &P, tol: f32| on_floor(&before, Vec2::from(*p), tol);
    let z = &mut bp.zones[i];
    shift(&mut z.c);
    z.outline.iter_mut().for_each(shift);
    if let Some(r) = &mut z.boss {
        shift(&mut r.c);
    }
    for l in &mut bp.links {
        if l.from == before.num {
            if let Some(p) = l.pts.first_mut().filter(|p| on(&**p, 3.0)) {
                shift(p);
            }
        }
        if l.to == before.num {
            if let Some(p) = l.pts.last_mut().filter(|p| on(&**p, 3.0)) {
                shift(p);
            }
        }
    }
    for pl in bp.placements.iter_mut().filter(|pl| on(&pl.at, 4.0)) {
        shift(&mut pl.at);
    }
    if on(&bp.start, 10.0) {
        shift(&mut bp.start);
    }
    if let Some(p) = bp.ending.at.as_mut().filter(|p| on(&**p, 4.0)) {
        shift(p);
    }
}

/// Keeps a zone inside the map with a 6 m margin (shrinking it if it is bigger than the map).
fn clamp_zone(bp: &mut Blueprint, i: usize, notes: &mut Vec<String>) -> bool {
    let [hx, hy] = bp.half();
    let room = Vec2::new(hx - 6.0, hy - 6.0).max(Vec2::splat(4.0));
    let mut e = half_extent(&bp.zones[i]);
    if e.x > room.x || e.y > room.y {
        let k = (room.x / e.x).min(room.y / e.y);
        let z = &mut bp.zones[i];
        z.size = [z.size[0] * k, z.size[1] * k];
        notes.push(format!(
            "zone {:02} {} was bigger than the map: shrunk to {:.0} × {:.0} m",
            z.num, z.name, z.size[0], z.size[1]
        ));
        e = half_extent(z);
    }
    let c = Vec2::from(bp.zones[i].c);
    let lim = (room - e).max(Vec2::ZERO);
    let to = if c.is_finite() {
        c.clamp(-lim, lim)
    } else {
        Vec2::ZERO
    };
    if to.distance(c) > 0.01 || !c.is_finite() {
        let z = &bp.zones[i];
        notes.push(format!(
            "moved zone {:02} {} {:.0} m to keep it inside the map",
            z.num,
            z.name,
            to.distance(if c.is_finite() { c } else { to })
        ));
        if !c.is_finite() {
            bp.zones[i].c = [0.0, 0.0];
        } else {
            move_zone(bp, i, to - c);
        }
        return true;
    }
    false
}

/// The zones reached from the Arrival through the links (return links one way), as `problems`
/// walks them.
fn reached(bp: &Blueprint) -> Vec<u8> {
    let Some(start) = bp.zones.iter().find(|z| z.role == Role::Arrival) else {
        return vec![];
    };
    let mut seen = vec![start.num];
    let mut grew = true;
    while grew {
        grew = false;
        for l in &bp.links {
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
    seen
}

/// Makes a blueprint buildable (what `Blueprint::problems` asks), changing as little as it can,
/// and says what it changed: the map's size in range; zones big enough, in range of height,
/// numbered once, inside the map and not piled on each other; one Arrival (first) and one Final
/// (last); every link's ends on its zones; every zone reached from the Arrival; each link's
/// stairs making up the height between its zones; boss rings inside their zones; placements only
/// of key letters.
pub fn repair(bp: &mut Blueprint) -> Vec<String> {
    let mut notes: Vec<String> = vec![];
    // the map
    let size = if bp.size.iter().all(|s| s.is_finite()) {
        bp.size
    } else {
        [240.0, 180.0]
    };
    let size = [size[0].clamp(120.0, 640.0), size[1].clamp(90.0, 520.0)];
    if size != bp.size {
        notes.push(format!(
            "map size {:?} m brought into range: {:?} m",
            bp.size, size
        ));
        bp.size = size;
    }

    // the zones themselves
    let mut seen: Vec<u8> = vec![];
    bp.zones.retain(|z| {
        let fresh = !seen.contains(&z.num);
        seen.push(z.num);
        if !fresh {
            notes.push(format!(
                "zone number {:02} was used twice: the second ({}) left out",
                z.num, z.name
            ));
        }
        fresh
    });
    if bp.zones.is_empty() {
        return notes;
    }
    for z in &mut bp.zones {
        if !(z.size[0] >= 6.0 && z.size[1] >= 6.0) {
            let s = [
                if z.size[0] >= 6.0 { z.size[0] } else { 8.0 },
                if z.size[1] >= 6.0 { z.size[1] } else { 8.0 },
            ];
            notes.push(format!(
                "zone {:02} {} was too small ({:?} m): {:?} m",
                z.num, z.name, z.size, s
            ));
            z.size = s;
        }
        if z.shape == Shape::Poly && z.outline.len() < 3 {
            z.shape = Shape::Rect;
        }
    }

    // one Arrival, first; one Final, last
    let arrivals: Vec<usize> = (0..bp.zones.len())
        .filter(|&i| bp.zones[i].role == Role::Arrival)
        .collect();
    let keep = arrivals
        .iter()
        .copied()
        .min_by_key(|&i| bp.zones[i].num)
        .unwrap_or_else(|| {
            let i = (0..bp.zones.len())
                .min_by_key(|&i| bp.zones[i].num)
                .unwrap();
            notes.push(format!(
                "no Arrival: zone {:02} {} made it",
                bp.zones[i].num, bp.zones[i].name
            ));
            i
        });
    for (i, z) in bp.zones.iter_mut().enumerate() {
        if i == keep {
            z.role = Role::Arrival;
        } else if z.role == Role::Arrival {
            z.role = if z.boss.is_some() {
                Role::Boss
            } else {
                Role::Area
            };
            notes.push(format!("zone {:02} {} was a second Arrival", z.num, z.name));
        }
    }
    let z = bp.zones.remove(keep);
    bp.zones.insert(0, z);
    let finals: Vec<usize> = (1..bp.zones.len())
        .filter(|&i| bp.zones[i].role == Role::Final)
        .collect();
    let keep = match finals.last() {
        Some(&i) => Some(i),
        None => {
            let pick = (1..bp.zones.len())
                .rev()
                .find(|&i| bp.zones[i].role != Role::Side)
                .or((bp.zones.len() > 1).then(|| bp.zones.len() - 1));
            if let Some(i) = pick {
                notes.push(format!(
                    "no Final zone: zone {:02} {} (the last on the route) made it",
                    bp.zones[i].num, bp.zones[i].name
                ));
            }
            pick
        }
    };
    if let Some(keep) = keep {
        for (i, z) in bp.zones.iter_mut().enumerate().skip(1) {
            if i == keep {
                z.role = Role::Final;
            } else if z.role == Role::Final {
                z.role = if z.boss.is_some() {
                    Role::Boss
                } else {
                    Role::Area
                };
                notes.push(format!("zone {:02} {} was a second Final", z.num, z.name));
            }
        }
        let z = bp.zones.remove(keep);
        bp.zones.push(z);
    }

    // heights above the Arrival, in range
    let base = bp.zones[0].elevation;
    if base != 0.0 {
        notes.push(format!(
            "heights made relative to the Arrival (it stood at {base:+} m)"
        ));
        bp.zones.iter_mut().for_each(|z| z.elevation -= base);
        bp.water.iter_mut().for_each(|w| w.level -= base);
        bp.chasms.iter_mut().for_each(|c| c.floor -= base);
    }
    for z in &mut bp.zones {
        let e = if z.elevation.is_finite() {
            z.elevation.clamp(-40.0, 80.0)
        } else {
            0.0
        };
        if e != z.elevation {
            notes.push(format!(
                "zone {:02} {} elevation {} m brought into range: {e} m",
                z.num, z.name, z.elevation
            ));
            z.elevation = e;
        }
    }

    // links between known zones
    let before = bp.links.len();
    let nums: Vec<u8> = bp.zones.iter().map(|z| z.num).collect();
    bp.links
        .retain(|l| l.from != l.to && nums.contains(&l.from) && nums.contains(&l.to));
    if bp.links.len() < before {
        notes.push(format!(
            "{} links joining unknown zones (or a zone to itself) left out",
            before - bp.links.len()
        ));
    }

    spread(bp, &mut notes);
    link_ends(bp, &mut notes);

    // every zone reached from the Arrival: a straight path from the nearest reached zone
    loop {
        let seen = reached(bp);
        let Some(u) = bp.zones.iter().find(|z| !seen.contains(&z.num)) else {
            break;
        };
        let Some(from) = bp
            .zones
            .iter()
            .filter(|z| seen.contains(&z.num))
            .min_by(|a, b| dist(a.c, u.c).total_cmp(&dist(b.c, u.c)))
        else {
            break;
        };
        notes.push(format!(
            "zone {:02} {} was not reached: added a path from zone {:02}",
            u.num, u.name, from.num
        ));
        bp.links.push(Link {
            from: from.num,
            to: u.num,
            pts: vec![from.c, u.c],
            width: 9.0,
            kind: LinkKind::Path,
            stairs: vec![],
        });
    }

    fix_stairs(bp, &mut notes);
    fit_rings(bp, &mut notes);

    // placements of key letters only
    bp.objects.retain(|o| !o.letter.trim().is_empty());
    let mut dropped: Vec<String> = vec![];
    let objects = bp.objects.clone();
    bp.placements.retain(|p| {
        let known = objects
            .iter()
            .any(|o| o.letter.eq_ignore_ascii_case(&p.letter));
        if !known && !dropped.contains(&p.letter) {
            dropped.push(p.letter.clone());
        }
        known
    });
    if !dropped.is_empty() {
        notes.push(format!(
            "placements of letters not in the key left out: {}",
            dropped.join(", ")
        ));
    }

    // the start on the map, facing somewhere
    let [hx, hy] = bp.half();
    let s = Vec2::from(bp.start);
    let to = if s.is_finite() {
        s.clamp(
            Vec2::new(-hx + 2.0, -hy + 2.0),
            Vec2::new(hx - 2.0, hy - 2.0),
        )
    } else {
        Vec2::from(bp.zones[0].c)
    };
    if to != s {
        notes.push("the player start was off the map: moved onto it".into());
        bp.start = to.to_array();
    }
    let d = Vec2::from(bp.start_dir);
    if !d.is_finite() || d.length() < 0.01 {
        let toward = bp.zones.get(1).map(|z| z.c).unwrap_or([0.0, 0.0]);
        bp.start_dir = unit(Vec2::from(toward) - Vec2::from(bp.start));
    } else {
        bp.start_dir = unit(d);
    }
    notes
}

/// Pushes apart zones whose floors overlap by more than 20% of the smaller one (the later one
/// moves away along the line between their middles) and keeps them inside the map.
fn spread(bp: &mut Blueprint, notes: &mut Vec<String>) {
    for _ in 0..8 {
        let mut moved = false;
        for i in 0..bp.zones.len() {
            moved |= clamp_zone(bp, i, notes);
        }
        for j in 1..bp.zones.len() {
            for i in 0..j {
                let (a, b) = (&bp.zones[i], &bp.zones[j]);
                let limit = 0.2 * floor_area(a).min(floor_area(b));
                let o = overlap(a, b, Vec2::ZERO);
                if o <= limit + 0.01 {
                    continue;
                }
                let mut dir = (Vec2::from(b.c) - Vec2::from(a.c)).normalize_or_zero();
                if dir == Vec2::ZERO {
                    dir = Vec2::from(b.c).normalize_or_zero();
                }
                if dir == Vec2::ZERO {
                    dir = Vec2::X;
                }
                // the least push that brings the overlap under the limit (it shrinks as b moves)
                let (mut lo, mut hi) = (
                    0.0f32,
                    (half_extent(a) + half_extent(b)).length() * 2.0 + 1.0,
                );
                for _ in 0..24 {
                    let mid = (lo + hi) * 0.5;
                    if overlap(a, b, dir * mid) > limit * 0.9 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                notes.push(format!(
                    "pushed zone {:02} {} {hi:.0} m away from zone {:02} (their floors overlapped by {:.0}% of the smaller)",
                    b.num,
                    b.name,
                    a.num,
                    100.0 * o / floor_area(a).min(floor_area(b))
                ));
                move_zone(bp, j, dir * hi);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for i in 0..bp.zones.len() {
        clamp_zone(bp, i, notes);
    }
}

/// Every link runs from its `from` zone to its `to` zone: a path, the right way round, its first
/// point on (or within 3 m of) the first zone and its last on the second (the zones' middles are
/// added where they aren't).
fn link_ends(bp: &mut Blueprint, notes: &mut Vec<String>) {
    let zones = bp.zones.clone();
    let zone = |n: u8| zones.iter().find(|z| z.num == n).unwrap();
    for l in &mut bp.links {
        let (a, b) = (zone(l.from), zone(l.to));
        let name = format!("{:02} → {:02}", l.from, l.to);
        if l.pts.len() < 2 {
            notes.push(format!("the link {name} had no path: a straight one"));
            l.pts = vec![a.c, b.c];
            continue;
        }
        let (p0, pn) = (l.pts[0], *l.pts.last().unwrap());
        if dist(p0, b.c) + dist(pn, a.c) < 0.6 * (dist(p0, a.c) + dist(pn, b.c)) {
            notes.push(format!("the link {name} was drawn backwards: turned round"));
            l.pts.reverse();
            for s in &mut l.stairs {
                s.at = 1.0 - s.at;
            }
            l.stairs.reverse();
        }
        let old = length(&l.pts);
        let (mut front, mut back) = (0.0, 0.0);
        if !on_floor(a, Vec2::from(l.pts[0]), 3.0) {
            front = dist(a.c, l.pts[0]);
            l.pts.insert(0, a.c);
            notes.push(format!(
                "the link {name} didn't start on zone {:02}: begun at its middle",
                l.from
            ));
        }
        if !on_floor(b, Vec2::from(*l.pts.last().unwrap()), 3.0) {
            back = dist(b.c, *l.pts.last().unwrap());
            l.pts.push(b.c);
            notes.push(format!(
                "the link {name} didn't end on zone {:02}: ended at its middle",
                l.to
            ));
        }
        let new = old + front + back;
        if new > 1e-3 && (front > 0.0 || back > 0.0) {
            for s in &mut l.stairs {
                s.at = (s.at * old + front) / new;
            }
        }
    }
}

/// Each link's stairs make up the height between its zones: stairs added where there are none
/// (4 m each, round the link's middle), rescaled where they don't add up, removed between zones at
/// the same height. Return links are one-way drops the builder handles; their stairs are only
/// rescaled.
fn fix_stairs(bp: &mut Blueprint, notes: &mut Vec<String>) {
    let height = |n: u8| {
        bp.zones
            .iter()
            .find(|z| z.num == n)
            .map(|z| z.elevation)
            .unwrap_or(0.0)
    };
    let deltas: Vec<f32> = bp
        .links
        .iter()
        .map(|l| height(l.to) - height(l.from))
        .collect();
    for (l, &delta) in bp.links.iter_mut().zip(&deltas) {
        let name = format!("{:02} → {:02}", l.from, l.to);
        let sum: f32 = l.stairs.iter().map(|s| s.rise).sum();
        if (sum - delta).abs() <= 0.6 {
            continue;
        }
        let ret = l.kind == LinkKind::Return;
        if l.stairs.is_empty() {
            if ret {
                continue;
            }
            let n = (delta.abs() / 4.0).ceil().max(1.0) as usize;
            l.stairs = (0..n)
                .map(|k| Stair {
                    at: if n == 1 {
                        0.5
                    } else {
                        0.35 + 0.3 * k as f32 / (n - 1) as f32
                    },
                    rise: delta / n as f32,
                    tag: String::new(),
                })
                .collect();
            notes.push(format!(
                "the link {name} climbs {delta:+} m with no stairs: added {n} of {:+} m",
                delta / n as f32
            ));
        } else if delta.abs() <= 0.6 {
            if ret {
                continue;
            }
            notes.push(format!(
                "the link {name} joins zones at the same height: its {} stairs ({sum:+} m) removed",
                l.stairs.len()
            ));
            l.stairs.clear();
        } else if sum.abs() < 0.1 || l.stairs.iter().any(|s| s.rise.abs() < 0.1) {
            let each = delta / l.stairs.len() as f32;
            l.stairs.iter_mut().for_each(|s| s.rise = each);
            notes.push(format!(
                "the link {name}'s stairs had no rises: {each:+} m each"
            ));
        } else {
            let k = delta / sum;
            l.stairs.iter_mut().for_each(|s| s.rise *= k);
            notes.push(format!("the link {name}'s stairs rose {sum:+} m but its zones differ by {delta:+} m: rescaled"));
        }
    }
}

/// Each boss ring fits its zone's floor: no wider than it, its middle where the whole ring is on
/// the floor.
fn fit_rings(bp: &mut Blueprint, notes: &mut Vec<String>) {
    for z in &mut bp.zones {
        let (w, d) = (z.size[0], z.size[1]);
        let zc = z.clone();
        let Some(r) = &mut z.boss else { continue };
        let most = w.min(d);
        if !(r.d > 0.0) {
            r.d = 0.45 * most;
        }
        if r.d > most {
            notes.push(format!(
                "zone {:02}'s boss ring (Ø{:.0} m) was wider than its floor: Ø{most:.0} m",
                zc.num, r.d
            ));
            r.d = most;
        }
        let room = (Vec2::new(w, d) * 0.5 - Vec2::splat(r.d * 0.5)).max(Vec2::ZERO);
        let l = local(&zc, Vec2::from(r.c));
        let l2 = if !l.is_finite() {
            Vec2::ZERO
        } else if zc.shape == Shape::Round {
            l.clamp_length_max(room.x.min(room.y))
        } else {
            l.clamp(-room, room)
        };
        if l2.distance(l) > 0.5 || !l.is_finite() {
            notes.push(format!(
                "zone {:02}'s boss ring moved {:.0} m to lie on its floor",
                zc.num,
                l2.distance(if l.is_finite() { l } else { l2 })
            ));
        }
        r.c = world(&zc, l2).to_array();
    }
}

// ---------------------------------------------------------------------------------------------
// Forgiving JSON fields

/// A string's numbers: "72 × 56 m" → [72, 56], "−10 m" → [−10], "Ø28" → [28]. A minus counts
/// only before a digit and not right after one ("8–10" is a range, not 8 and −10).
fn numbers_in(s: &str) -> Vec<f32> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = vec![];
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let digit_next = cs
            .get(i + 1)
            .is_some_and(|n| n.is_ascii_digit() || *n == '.');
        let sign = (c == '-' || c == '−') && digit_next && !(i > 0 && cs[i - 1].is_ascii_digit());
        if c.is_ascii_digit() || sign || (c == '.' && digit_next) {
            let mut buf = String::new();
            if sign {
                buf.push('-');
                i += 1;
            }
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                buf.push(cs[i]);
                i += 1;
            }
            if let Ok(x) = buf.trim_end_matches('.').parse::<f32>() {
                out.push(x);
            }
            continue;
        }
        i += 1;
    }
    out
}

fn num_of(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|x| x as f32).filter(|x| x.is_finite()),
        Value::String(s) => {
            let x = *numbers_in(s).first()?;
            Some(if s.contains('↓') && x > 0.0 { -x } else { x })
        }
        _ => None,
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Array(a) => a
            .iter()
            .map(text_of)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" • "),
        _ => String::new(),
    }
}

fn pt_of(v: &Value) -> Option<Px> {
    match v {
        Value::Array(a) if a.len() >= 2 => Some([num_of(&a[0])?, num_of(&a[1])?]),
        Value::Object(o) => Some([num_of(o.get("x")?)?, num_of(o.get("y")?)?]),
        _ => None,
    }
}

fn pts_of(v: &Value) -> Vec<Px> {
    let Value::Array(a) = v else { return vec![] };
    if a.iter().all(|e| e.is_number()) {
        return a
            .chunks_exact(2)
            .filter_map(|c| Some([num_of(&c[0])?, num_of(&c[1])?]))
            .collect();
    }
    a.iter().filter_map(pt_of).collect()
}

fn nums_of(v: &Value) -> Vec<f32> {
    match v {
        Value::Array(a) => a.iter().filter_map(num_of).collect(),
        Value::String(s) => numbers_in(s),
        Value::Number(_) => num_of(v).into_iter().collect(),
        _ => vec![],
    }
}

mod de {
    use super::*;

    fn value<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
        Value::deserialize(d)
    }
    pub fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(text_of(&value(d)?))
    }
    pub fn texts<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        Ok(match value(d)? {
            Value::Array(a) => a.iter().map(text_of).filter(|s| !s.is_empty()).collect(),
            v => Some(text_of(&v))
                .filter(|s| !s.is_empty())
                .into_iter()
                .collect(),
        })
    }
    pub fn num<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
        Ok(num_of(&value(d)?).unwrap_or(0.0))
    }
    pub fn opt_num<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f32>, D::Error> {
        Ok(num_of(&value(d)?))
    }
    pub fn byte<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
        Ok(num_of(&value(d)?)
            .map(|x| x.round().clamp(0.0, 255.0) as u8)
            .unwrap_or(0))
    }
    pub fn count<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        Ok(num_of(&value(d)?)
            .map(|x| x.round().max(0.0) as u32)
            .unwrap_or(0))
    }
    pub fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
        Ok(match value(d)? {
            Value::Bool(b) => b,
            Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
            Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1"),
            _ => false,
        })
    }
    pub fn pt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Px>, D::Error> {
        Ok(pt_of(&value(d)?))
    }
    pub fn pts<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Px>, D::Error> {
        Ok(pts_of(&value(d)?))
    }
    pub fn nums<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
        Ok(nums_of(&value(d)?))
    }
    /// [a, b] (numbers or a "72 × 56 m" string)
    pub fn pair<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[f32; 2]>, D::Error> {
        let v = nums_of(&value(d)?);
        Ok((v.len() >= 2).then(|| [v[0], v[1]]))
    }
    /// [x0, y0, x1, y1] or [[x0, y0], [x1, y1]], put in order
    pub fn rect<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[f32; 4]>, D::Error> {
        let v = value(d)?;
        let mut n = nums_of(&v);
        if n.len() < 4 {
            n = pts_of(&v).into_iter().flatten().collect();
        }
        if n.len() < 4 {
            return Ok(None);
        }
        let r = [
            n[0].min(n[2]),
            n[1].min(n[3]),
            n[0].max(n[2]),
            n[1].max(n[3]),
        ];
        Ok((r[2] - r[0] > 0.5 && r[3] - r[1] > 0.5).then_some(r))
    }
    pub fn segment<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[Px; 2]>, D::Error> {
        let p = pts_of(&value(d)?);
        Ok((p.len() >= 2).then(|| [p[0], p[1]]))
    }
    /// A list of objects; the ones that aren't readable are skipped.
    pub fn list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
        Ok(match value(d)? {
            Value::Array(a) => a
                .into_iter()
                .filter_map(|e| serde_json::from_value(e).ok())
                .collect(),
            _ => vec![],
        })
    }
    /// An object, or None (null, or not an object).
    pub fn obj<'de, D: Deserializer<'de>, T: DeserializeOwned>(
        d: D,
    ) -> Result<Option<T>, D::Error> {
        Ok(match value(d)? {
            v @ Value::Object(_) => serde_json::from_value(v).ok(),
            _ => None,
        })
    }
    /// {kind, at_px}, or just the kind ("exit").
    pub fn ending<'de, D: Deserializer<'de>>(d: D) -> Result<Option<EndingRead>, D::Error> {
        Ok(match value(d)? {
            v @ Value::Object(_) => serde_json::from_value(v).ok(),
            Value::String(s) => Some(EndingRead {
                kind: s,
                at_px: None,
            }),
            _ => None,
        })
    }
    pub fn art<'de, D: Deserializer<'de>>(d: D) -> Result<ArtRead, D::Error> {
        Ok(match value(d)? {
            v @ Value::Object(_) => serde_json::from_value(v).unwrap_or_default(),
            _ => ArtRead::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const PARCHMENT: Rgba<u8> = Rgba([232, 216, 184, 255]);
    const RED: Rgba<u8> = Rgba([214, 28, 36, 255]);
    const WHITE: Rgba<u8> = Rgba([250, 250, 250, 255]);

    fn disc(img: &mut RgbaImage, c: [f32; 2], r: f32, col: Rgba<u8>) {
        for y in (c[1] - r - 1.0) as u32..=(c[1] + r + 1.0) as u32 {
            for x in (c[0] - r - 1.0) as u32..=(c[0] + r + 1.0) as u32 {
                let (dx, dy) = (x as f32 + 0.5 - c[0], y as f32 + 0.5 - c[1]);
                if dx * dx + dy * dy <= r * r {
                    img.put_pixel(x, y, col);
                }
            }
        }
    }

    fn fill(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, col: Rgba<u8>) {
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, col);
            }
        }
    }

    /// A marker disc with a white two-digit-ish number inside.
    fn marker(img: &mut RgbaImage, c: [f32; 2]) {
        disc(img, c, 15.0, RED);
        let (x, y) = (c[0] as u32, c[1] as u32);
        fill(img, x - 7, y - 6, x - 2, y + 6, WHITE);
        fill(img, x + 1, y - 6, x + 6, y + 6, WHITE);
    }

    /// A red dashed line (the route), 4 px thick.
    fn dashes(img: &mut RgbaImage, a: [f32; 2], b: [f32; 2]) {
        let len = dist(a, b);
        let n = len as usize;
        for i in 0..n {
            if (i % 22) >= 14 {
                continue;
            }
            let t = i as f32 / len;
            let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            fill(
                img,
                p[0] as u32 - 2,
                p[1] as u32 - 2,
                p[0] as u32 + 2,
                p[1] as u32 + 2,
                RED,
            );
        }
    }

    #[test]
    fn finds_the_marker_discs() {
        let mut img = RgbaImage::from_pixel(1536, 1024, PARCHMENT);
        let want = [[300.0, 400.0], [700.0, 250.0], [900.0, 700.0]];
        marker(&mut img, want[0]);
        marker(&mut img, want[1]);
        disc(&mut img, want[2], 15.0, RED);
        // the route through them, a dashed boss ring, a legend disc in the sidebar
        dashes(&mut img, [100.0, 900.0], [1000.0, 850.0]);
        dashes(&mut img, [300.0, 420.0], [690.0, 260.0]);
        for k in 0..48 {
            if k % 3 == 2 {
                continue;
            }
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            disc(
                &mut img,
                [500.0 + 60.0 * a.cos(), 650.0 + 60.0 * a.sin()],
                2.0,
                RED,
            );
        }
        marker(&mut img, [1300.0, 300.0]);
        let found = find_markers(&img);
        assert_eq!(found.len(), 3, "found {found:?}");
        for w in want {
            assert!(
                found.iter().any(|f| dist(*f, w) < 2.0),
                "no disc found at {w:?}: {found:?}"
            );
        }
    }

    /// A made-up sheet read at 0.3 m/px (its dimension lines say ~9% more), sloppily typed.
    const READING: &str = r#"Here is my reading of the sheet.
```json
{
  "name": "Test Hollow",
  "frame_px": [40, 100, 1080, 900],
  "frame_m": [340, 260],
  "zones": [
    {"num": 1, "name": "Gate yard", "size_m": [48, 36], "elevation_m": 2, "marker_px": [180, 760], "box_px": [100, 700, 260, 820], "round": false, "boss": null},
    {"num": "02", "name": "Lower court", "size_m": "60 × 72 m", "elevation_m": "+6 m", "marker_px": [240, 300], "box_px": [120, 200, 360, 400],
     "boss": {"label": "Boss 1", "diameter_m": 27, "center_px": [240, 300], "radius_px": 45}},
    {"num": 3, "name": "Bridge deck", "size_m": [40, 12], "elevation_m": 6, "marker_px": [600, 250]},
    {"num": 4, "name": "Side garden", "size_m": [45, 45], "elevation_m": 2, "marker_px": [550, 650], "box_px": [500, 600, 600, 700], "side": true},
    {"num": 5, "name": "Crown arena", "size_m": [72, 72], "elevation_m": 14, "marker_px": [920, 560], "box_px": [800, 450, 1040, 690], "round": true,
     "boss": {"label": "Final boss", "diameter_m": 36, "center_px": [920, 570], "radius_px": 60}, "final": true}
  ],
  "links": [
    {"from": 1, "to": 2, "points_px": [[180, 740], [180, 600], [200, 500], [240, 380]], "kind": "path", "stairs": [{"at_px": [180, 620], "rise_m": "↑ 4 m", "tag": "D"}]},
    {"from": 2, "to": 3, "points_px": [[340, 300], [450, 260], [590, 250]], "kind": "path", "stairs": []},
    {"from": 3, "to": 5, "points_px": [[610, 260], [750, 400], [880, 520]], "kind": "bridge", "width_m": 10},
    {"from": 5, "to": 1, "points_px": [[860, 650], [500, 800], [250, 780]], "kind": "return", "stairs": [{"at_px": [500, 800], "rise_m": "↓ 12 m"}]}
  ],
  "water": [{"name": "Moat", "level_m": 0, "outline_px": [[400, 450], [700, 450], [700, 560], [400, 560]]}],
  "chasms": [{"name": "Rift", "floor_m": -8, "outline_px": [[650, 700], [780, 700], [780, 860], [650, 860]]}],
  "objects": [
    {"letter": "A", "name": "Gatehouse", "group": "structure", "size_m": [12, 8, 10], "count": 1, "at_px": [[180, 820]]},
    {"letter": "b", "name": "Brazier", "group": "prop", "size_m": "Ø1.5 × 2h m", "count": "2", "at_px": [[150, 250], [330, 250]]}
  ],
  "start_px": [180, 840],
  "start_toward_px": [180, 700],
  "ending": {"kind": "exit", "at_px": [960, 600]},
  "sightline_px": [[300, 700], [920, 570]],
  "art": {"feel": "Quiet", "materials": "stone", "light": "dusk", "vfx": "dust", "post": "bloom"},
  "notes": ["zone 03 has no floor drawn"],
}
```
That's all."#;

    #[test]
    fn parses_a_fenced_reply() {
        let r = parse(READING).unwrap();
        assert_eq!(r.name, "Test Hollow");
        assert_eq!(r.zones.len(), 5);
        assert_eq!(r.zones[1].num, 2);
        assert_eq!(r.zones[1].size_m, Some([60.0, 72.0]));
        assert_eq!(r.zones[1].elevation_m, Some(6.0));
        assert!(r.zones[4].finale && r.zones[4].round && r.zones[3].side);
        assert!(r.zones[0].boss.is_none() && r.zones[1].boss.is_some());
        assert_eq!(r.links[0].stairs[0].rise_m, 4.0);
        assert_eq!(r.links[3].stairs[0].rise_m, -12.0);
        assert_eq!(r.objects[1].count, 2);
        assert_eq!(r.objects[1].size_m, vec![1.5, 2.0]);
        assert_eq!(r.chasms[0].floor_m, Some(-8.0));
        // what models write into JSON numbers
        let r = parse("{\"zones\": [{\"num\": 1, \"elevation_m\": +4}, {\"num\": 2, \"elevation_m\": −4,},], \"name\": \"a − b\"}").unwrap();
        assert_eq!(r.zones[0].elevation_m, Some(4.0));
        assert_eq!(r.zones[1].elevation_m, Some(-4.0));
        assert_eq!(r.name, "a − b");
        assert!(parse("no json here").is_err());
    }

    #[test]
    fn normalizes_a_reading() {
        let read = parse(READING).unwrap();
        // the sheet's discs, zone 03's 13 px from where the model put it, zone 04's missing
        let mut img = RgbaImage::from_pixel(1536, 1024, PARCHMENT);
        for c in [
            [180.0, 760.0],
            [240.0, 300.0],
            [612.0, 244.0],
            [920.0, 560.0],
        ] {
            marker(&mut img, c);
        }
        let (bp, notes) = normalize(&read, Some(&img), None).unwrap();
        for n in &notes {
            println!("note: {n}");
        }
        assert!(bp.problems().is_empty(), "{:?}", bp.problems());
        // 0.3 m/px: the frame is 1040 × 800 px
        assert!(
            (bp.size[0] - 310.0).abs() <= 10.0 && (bp.size[1] - 240.0).abs() <= 10.0,
            "size {:?}",
            bp.size
        );
        let z = |n: u8| bp.zone(n).unwrap();
        assert_eq!(z(1).role, Role::Arrival);
        assert_eq!(z(1).elevation, 0.0);
        assert_eq!(z(2).elevation, 4.0);
        assert_eq!(z(5).elevation, 12.0);
        assert_eq!(z(2).role, Role::Boss);
        assert_eq!(z(4).role, Role::Side);
        assert_eq!(bp.finale().unwrap().num, 5);
        assert_eq!(bp.zones[0].num, 1);
        // y up: the court near the top of the image is north, the gate yard south
        assert!(
            z(2).c[1] > 50.0 && z(1).c[1] < -50.0,
            "{:?} {:?}",
            z(2).c,
            z(1).c
        );
        assert!(z(1).c[0] < 0.0 && z(5).c[0] > 0.0);
        // the caption's W × D turned to match the drawing
        assert_eq!(z(2).size, [72.0, 60.0]);
        // zone 03 (no floor drawn) stands on its disc: (612 − 560, 500 − 244) px × 0.3
        assert!(dist(z(3).c, [15.6, 76.8]) < 2.0, "zone 03 at {:?}", z(3).c);
        assert!(
            notes
                .iter()
                .any(|n| n.contains("zone 04") && n.contains("no red disc"))
        );
        // stairs: as read, then added where a climb had none
        let link = |a: u8, b: u8| bp.links.iter().find(|l| l.from == a && l.to == b).unwrap();
        assert_eq!(link(1, 2).stairs.len(), 1);
        assert_eq!(link(1, 2).stairs[0].tag, "D");
        // 36 m along a 110.5 m link
        assert!(
            (link(1, 2).stairs[0].at - 0.326).abs() < 0.02,
            "at {}",
            link(1, 2).stairs[0].at
        );
        assert_eq!(link(3, 5).stairs.iter().map(|s| s.rise).sum::<f32>(), 8.0);
        assert_eq!(link(3, 5).kind, LinkKind::Bridge);
        assert_eq!(link(5, 1).kind, LinkKind::Return);
        assert_eq!(link(5, 1).width, 6.0);
        // the side garden had no link: one was added
        assert!(bp.links.iter().any(|l| l.to == 4));
        assert_eq!(bp.water[0].level, -2.0);
        assert_eq!(bp.chasms[0].floor, -10.0);
        assert_eq!(bp.placements.len(), 3);
        assert_eq!(bp.object("B").unwrap().group, Group::Prop);
        assert!(bp.start_dir[1] > 0.99);
        assert_eq!(bp.zone(5).unwrap().boss.as_ref().unwrap().d, 36.0);
        assert_eq!(bp.id, "test_hollow");
    }

    #[test]
    fn fills_in_from_the_brief() {
        let read = parse(READING).unwrap();
        let brief = serde_json::json!({"id": "042_test_hollow", "name": "The Test Hollow", "family": "cold", "kind": "raid", "type": "gauntlet"});
        let (bp, _) = normalize(&read, None, Some(&brief)).unwrap();
        assert_eq!(bp.id, "042_test_hollow");
        assert_eq!(bp.name, "The Test Hollow");
        assert_eq!(bp.family, "cold");
        assert_eq!(bp.kind, Kind::Raid);
        assert_eq!(bp.layout_type, "gauntlet");
        assert!(bp.brief.is_some());
        assert!(bp.problems().is_empty(), "{:?}", bp.problems());
    }

    fn zone(num: u8, role: Role, c: P, size: [f32; 2], elevation: f32) -> Zone {
        Zone {
            num,
            name: format!("Z{num}"),
            role,
            c,
            size,
            elevation,
            ..Default::default()
        }
    }

    #[test]
    fn repairs_a_blueprint() {
        let mut bp = Blueprint {
            size: [200.0, 150.0],
            zones: vec![
                zone(1, Role::Arrival, [-60.0, -40.0], [30.0, 30.0], 0.0),
                zone(2, Role::Area, [0.0, 0.0], [40.0, 30.0], 4.0),
                zone(4, Role::Area, [-55.0, -35.0], [20.0, 20.0], 0.0),
                zone(3, Role::Boss, [95.0, 30.0], [30.0, 30.0], 12.0),
            ],
            links: vec![
                Link {
                    from: 1,
                    to: 2,
                    pts: vec![[-60.0, -40.0], [0.0, 0.0]],
                    width: 9.0,
                    ..Default::default()
                },
                Link {
                    from: 2,
                    to: 3,
                    pts: vec![[0.0, 0.0], [50.0, 30.0]],
                    width: 9.0,
                    stairs: vec![Stair {
                        at: 0.5,
                        rise: 4.0,
                        tag: "D".into(),
                    }],
                    ..Default::default()
                },
                Link {
                    from: 3,
                    to: 9,
                    pts: vec![[0.0, 0.0], [50.0, 30.0]],
                    ..Default::default()
                },
            ],
            objects: vec![ObjectKind {
                letter: "A".into(),
                name: "Arch".into(),
                size: [8.0, 4.0, 10.0],
                count: 1,
                ..Default::default()
            }],
            placements: vec![
                Placement {
                    letter: "A".into(),
                    at: [-60.0, -50.0],
                    facing: None,
                },
                Placement {
                    letter: "Z".into(),
                    at: [0.0, 0.0],
                    facing: None,
                },
            ],
            start_dir: [0.0, 0.0],
            ..Default::default()
        };
        assert!(!bp.problems().is_empty());
        let notes = repair(&mut bp);
        for n in &notes {
            println!("note: {n}");
        }
        assert!(bp.problems().is_empty(), "{:?}", bp.problems());
        // the last zone on the route became the Final, and stands inside the map
        assert_eq!(bp.finale().unwrap().num, 3);
        assert!(bp.zone(3).unwrap().c[0] <= 100.0 - 6.0 - 15.0 + 0.01);
        // a climb without stairs got one; stairs that didn't add up were rescaled
        let link = |a: u8, b: u8| bp.links.iter().find(|l| l.from == a && l.to == b).unwrap();
        assert_eq!(link(1, 2).stairs.len(), 1);
        assert_eq!(link(1, 2).stairs[0].rise, 4.0);
        assert_eq!(link(2, 3).stairs.iter().map(|s| s.rise).sum::<f32>(), 8.0);
        // the link to nowhere is gone; the unreached zone 04 got a path
        assert!(bp.links.iter().all(|l| l.to != 9));
        assert!(bp.links.iter().any(|l| l.to == 4 || l.from == 4));
        // zone 04 was piled on the Arrival: pushed off it
        let (a, b) = (bp.zone(1).unwrap(), bp.zone(4).unwrap());
        assert!(overlap(a, b, Vec2::ZERO) <= 0.2 * floor_area(b) + 0.01);
        assert_eq!(bp.placements.len(), 1);
        assert!(Vec2::from(bp.start_dir).length() > 0.99);
    }

    #[test]
    fn adds_stairs_round_the_middle() {
        let mut bp = Blueprint {
            size: [300.0, 200.0],
            zones: vec![
                zone(1, Role::Arrival, [-100.0, 0.0], [30.0, 30.0], 0.0),
                zone(2, Role::Area, [0.0, 0.0], [30.0, 30.0], 10.0),
                zone(3, Role::Final, [100.0, 0.0], [40.0, 40.0], 10.0),
            ],
            links: vec![
                Link {
                    from: 1,
                    to: 2,
                    pts: vec![[-100.0, 0.0], [0.0, 0.0]],
                    ..Default::default()
                },
                Link {
                    from: 2,
                    to: 3,
                    pts: vec![[0.0, 0.0], [100.0, 0.0]],
                    stairs: vec![Stair {
                        at: 0.5,
                        rise: 4.0,
                        tag: String::new(),
                    }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        repair(&mut bp);
        assert!(bp.problems().is_empty(), "{:?}", bp.problems());
        let s = &bp.links[0].stairs;
        assert_eq!(s.len(), 3);
        assert!((s[0].at - 0.35).abs() < 1e-4 && (s[2].at - 0.65).abs() < 1e-4);
        assert!(s.iter().all(|s| (s.rise - 10.0 / 3.0).abs() < 1e-4));
        assert!(
            bp.links[1].stairs.is_empty(),
            "stairs between zones at the same height are removed"
        );
    }

    /// `WB_SHEET_READ=<reading.json>,<image.png>,<out blueprint.json>[,<brief.json>]` reads a real
    /// sheet's reading (the image may be left empty), prints the notes and problems and writes
    /// the blueprint.
    #[test]
    fn real_sheet() {
        let Ok(spec) = std::env::var("WB_SHEET_READ") else {
            return;
        };
        let parts: Vec<&str> = spec.split(',').map(str::trim).collect();
        let text = std::fs::read_to_string(parts[0]).expect("the reading");
        let read = parse(&text).expect("a readable reading");
        let img = parts
            .get(1)
            .filter(|p| !p.is_empty())
            .map(|p| image::open(p).expect("the sheet (png or jpeg)").to_rgba8());
        if let Some(img) = &img {
            let discs: Vec<String> = find_markers(img)
                .iter()
                .map(|d| format!("({:.1}, {:.1})", d[0], d[1]))
                .collect();
            println!("red discs: {}", discs.join(" "));
        }
        let brief = parts.get(3).map(|p| {
            serde_json::from_str::<Value>(&std::fs::read_to_string(p).expect("the brief"))
                .expect("brief JSON")
        });
        let (bp, notes) = normalize(&read, img.as_ref(), brief.as_ref()).expect("a blueprint");
        for n in &read.notes {
            println!("reader: {n}");
        }
        for n in &notes {
            println!("note: {n}");
        }
        println!("{}", serde_json::to_string_pretty(&bp.summary()).unwrap());
        for l in &bp.links {
            let stairs: Vec<String> = l
                .stairs
                .iter()
                .map(|s| {
                    format!(
                        "{:+} m @{:.2}{}",
                        s.rise,
                        s.at,
                        if s.tag.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", s.tag)
                        }
                    )
                })
                .collect();
            println!(
                "link {:02} → {:02} {:?} {:.0} m wide, {:.0} m long, stairs [{}]",
                l.from,
                l.to,
                l.kind,
                l.width,
                length(&l.pts),
                stairs.join(", ")
            );
        }
        let problems = bp.problems();
        println!("problems: {problems:?}");
        if let Some(out) = parts.get(2).filter(|p| !p.is_empty()) {
            std::fs::write(out, serde_json::to_string_pretty(&bp).unwrap())
                .expect("writing the blueprint");
            println!("wrote {out}");
        }
        assert!(problems.is_empty());
    }
}
