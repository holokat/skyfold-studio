//! Reading a map picture ("Analyse"): water (sea, lakes, rivers), land regions by colour with a
//! suggested biome and height role, and what to ignore (legend panels, lettering). Pure image
//! processing on a working grid (the picture downscaled so its longer side is at most
//! [`MAX_SIDE`]): no engine types, so it runs on a worker thread.
//!
//! Pipeline:
//! 1. CIELab colours; k-means colour classes, each judged water (blue / cyan / teal), land or
//!    ambiguous (whites and greys: clouds, foam, snow, lettering, shadows). An ambiguous patch
//!    becomes whatever mostly surrounds it (clouds over the sea are sea, snow among rock is land).
//! 2. Thin line art and lettering (strokes no thicker than a few px, far longer than wide) is not
//!    land; framed rectangular panels (legends) and the user's ignore areas take the class of
//!    what surrounds them.
//! 3. Open water reaching the border (or very large) is sea, enclosed open water is a lake; long
//!    thin water is a river, skeletonised into centrelines with widths. Short thin water joins the
//!    body it touches (inlets, straits).
//! 4. Land regions: k-means over each spot's regional average colour and texture, smoothed, small
//!    fragments merged into neighbours (islands into what they face across the water), then the
//!    most similar neighbours merged down to the target count.
//! 5. The user's edits (labels, merges, splits, deletions) are stored by picture position and
//!    replayed on top, so they survive re-analysis.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Working-grid size: the picture is downscaled so its longer side is at most this.
pub const MAX_SIDE: usize = 1024;

pub const SEA: u8 = 0;
pub const LAKE: u8 = 1;
pub const RIVER: u8 = 2;
pub const LAND: u8 = 3;
/// No region (water).
pub const NONE: u32 = u32::MAX;

/// How a region's ground rises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Peaks,
    Mountains,
    Hills,
    #[default]
    Plains,
    Lowland,
    Wetland,
}

impl Role {
    pub const ALL: [Role; 6] = [
        Role::Peaks,
        Role::Mountains,
        Role::Hills,
        Role::Plains,
        Role::Lowland,
        Role::Wetland,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Role::Peaks => "Peaks",
            Role::Mountains => "Mountains",
            Role::Hills => "Hills",
            Role::Plains => "Plains",
            Role::Lowland => "Lowland",
            Role::Wetland => "Wetland",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Role::Peaks => "peaks",
            Role::Mountains => "mountains",
            Role::Hills => "hills",
            Role::Plains => "plains",
            Role::Lowland => "lowland",
            Role::Wetland => "wetland",
        }
    }
    pub fn from_id(s: &str) -> Option<Role> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|r| r.id().eq_ignore_ascii_case(s))
    }
}

/// Analysis settings (saved with the world).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Target number of land regions (0: automatic, 8–20 by sensitivity).
    pub regions: u32,
    /// 0..1: higher keeps smaller features (islets, short rivers, small regions).
    pub sensitivity: f32,
    /// Find framed legend panels by themselves.
    pub auto_ignore: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            regions: 0,
            sensitivity: 0.5,
            auto_ignore: true,
        }
    }
}

/// A user edit of the regions, stored by picture position (uv, 0..1 from the top-left) so it
/// survives re-analysis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Rename / re-biome / re-role the region at `at`.
    Label {
        at: [f32; 2],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// `BiomePreset` name (e.g. "Desert").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        biome: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<Role>,
    },
    /// The region at `b` joins the region at `a`.
    Merge { a: [f32; 2], b: [f32; 2] },
    /// Cut the region the line crosses in two.
    Split { line: Vec<[f32; 2]> },
    /// The region at `at` isn't a region of its own: it becomes what surrounds it (an islet the
    /// sea, an inland patch its neighbours).
    Delete { at: [f32; 2] },
}

/// A river centreline in grid cells (x right, y down) with its width (cells) at each point.
#[derive(Clone, Debug)]
pub struct RiverLine {
    pub pts: Vec<[f32; 2]>,
    pub width: Vec<f32>,
    /// Whether each end meets the sea or a lake.
    pub mouth: [bool; 2],
}

impl RiverLine {
    pub fn length(&self) -> f32 {
        self.pts
            .windows(2)
            .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
            .sum()
    }
}

/// The expensive part of an analysis (everything before the user's edits).
pub struct Base {
    pub w: usize,
    pub h: usize,
    /// Working-grid colours (sRGB).
    pub rgb: Vec<[u8; 3]>,
    pub lab: Vec<[f32; 3]>,
    pub class: Vec<u8>,
    /// Automatic land regions (index per land cell, `NONE` on water).
    pub seg: Vec<u32>,
    /// Sea shallowness: 0 deep … 1 shallow (light turquoise water).
    pub shallow: Vec<f32>,
    /// Picture shading detail on land, about −1..1 (bright = up).
    pub relief: Vec<f32>,
    /// Local contrast of the picture (rocky / mountainous texture), about 0..1.
    pub contrast: Vec<f32>,
    /// How riddled with water the land around is (share of narrow water nearby), 0..1.
    pub wet: Vec<f32>,
    /// Vegetation density read from the picture, 0 (bare) … 1 (dense canopy): dark, green,
    /// textured land is forest; light green is open grass; desert, rock and snow are bare.
    pub density: Vec<f32>,
    pub rivers: Vec<RiverLine>,
    /// Cells taken from their surroundings (ignore areas and auto-detected panels).
    pub ignored: Vec<bool>,
    /// Auto-detected panels, uv rects [u0, v0, u1, v1].
    pub panels: Vec<[f32; 4]>,
    pub millis: f32,
}

/// One land region after the user's edits.
#[derive(Clone, Debug)]
pub struct Region {
    /// 1.. by size (largest first).
    pub id: u32,
    pub name: String,
    /// Mean picture colour.
    pub color: [u8; 3],
    pub cells: usize,
    /// Share of all land.
    pub frac: f32,
    /// uv centre of mass.
    pub centroid: [f32; 2],
    /// uv interior point farthest from the region's edge (for labels and edits).
    pub pole: [f32; 2],
    /// `BiomePreset` name.
    pub biome: String,
    pub role: Role,
    /// Whether the user set the name / biome / role.
    pub named: bool,
    pub suggested_biome: String,
    pub suggested_role: Role,
    /// Outline loops in uv (outer edges and holes), simplified.
    pub outline: Vec<Vec<[f32; 2]>>,
}

/// An enclosed water body.
#[derive(Clone, Debug)]
pub struct LakeArea {
    /// Outer shoreline in uv.
    pub outline: Vec<[f32; 2]>,
    pub cells: usize,
}

/// A finished analysis: the base plus the user's edits.
pub struct Analysis {
    pub w: usize,
    pub h: usize,
    pub class: Vec<u8>,
    /// Index into `regions` per land cell (`NONE` on water).
    pub region: Vec<u32>,
    pub regions: Vec<Region>,
    pub lakes: Vec<LakeArea>,
    pub rivers: Vec<RiverLine>,
    /// Edits that no longer found their region (e.g. after the image moved under them).
    pub stale_ops: usize,
}

impl Analysis {
    pub fn cell_at(&self, uv: [f32; 2]) -> Option<usize> {
        let x = (uv[0] * self.w as f32).floor();
        let y = (uv[1] * self.h as f32).floor();
        (x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h)
            .then(|| y as usize * self.w + x as usize)
    }
    /// Region index at a uv position (or the nearest land within a few cells).
    pub fn region_at(&self, uv: [f32; 2]) -> Option<usize> {
        let k = self.cell_at(uv)?;
        region_near(&self.region, self.w, self.h, k, 4).map(|r| r as usize)
    }
}

// ------------------------------------------------------------------ entry points

/// Analyses an RGB8 picture (`iw × ih`). `ignore`: polygons in uv the user marked as not part of
/// the map.
pub fn analyse(
    px: &[u8],
    iw: usize,
    ih: usize,
    settings: &Settings,
    ignore: &[Vec<[f32; 2]>],
) -> Base {
    let t0 = std::time::Instant::now();
    let (w, h, rgb) = downscale(px, iw, ih, MAX_SIDE);
    let n = w * h;
    let sens = settings.sensitivity.clamp(0.0, 1.0);
    let lut = srgb_lut();
    let lab: Vec<[f32; 3]> = rgb.par_iter().map(|c| srgb_to_lab(*c, &lut)).collect();
    let side = w.max(h) as f32;

    // --- what to ignore
    let mut ignored = vec![false; n];
    for poly in ignore {
        raster_polygon(poly, w, h, |k| ignored[k] = true);
    }
    let mut panels = vec![];
    if settings.auto_ignore {
        for r in find_panels(&lab, w, h) {
            panels.push([
                r[0] as f32 / w as f32,
                r[1] as f32 / h as f32,
                (r[2] + 1) as f32 / w as f32,
                (r[3] + 1) as f32 / h as f32,
            ]);
            for y in r[1]..=r[3] {
                for x in r[0]..=r[2] {
                    ignored[y * w + x] = true;
                }
            }
        }
    }

    // --- water / land by colour classes, rivers as lines found at up to twice the grid's detail
    let soft = blur3(&lab, w, h, 1);
    let lines = {
        let (lw, lh, lrgb) = downscale(px, iw, ih, MAX_SIDE * 2);
        let llab: Vec<[f32; 3]> = lrgb.par_iter().map(|c| srgb_to_lab(*c, &lut)).collect();
        let fine = water_lines(&llab, lw, lh);
        let mut on = vec![false; n];
        for (k, f) in fine.iter().enumerate() {
            if *f {
                on[((k / lw) * h / lh) * w + (k % lw) * w / lw] = true;
            }
        }
        on
    };
    let kind = classify(&lab, &soft, &lines, &ignored, w, h);
    let mut water: Vec<bool> = kind.iter().map(|k| *k == Kind::Water).collect();
    let known: Vec<bool> = kind.iter().map(|k| *k != Kind::Unknown).collect();
    fill_unknown_bool(&mut water, &known, w, h);
    clean_land(&mut water, w, h, sens);

    // --- sea, lakes, rivers
    let (class, rivers) = split_water(&water, w, h, sens);

    // --- hints
    let shallow = sea_shallowness(&soft, &class, w, h);
    let lum: Vec<f32> = lab.iter().map(|c| c[0]).collect();
    let landw: Vec<f32> = class
        .iter()
        .map(|c| if *c == LAND { 1.0 } else { 0.0 })
        .collect();
    let sq: Vec<f32> = lum.iter().map(|v| v * v).collect();
    let r_c = (side / 256.0).max(2.0) as usize;
    let m1 = blur(&lum, Some(&landw), w, h, r_c);
    let m2 = blur(&sq, Some(&landw), w, h, r_c);
    let contrast: Vec<f32> = (0..n)
        .map(|k| ((m2[k] - m1[k] * m1[k]).max(0.0).sqrt() / 30.0).min(1.5))
        .collect();
    // relief hint: the picture's mid-scale light and shade (a grey opening first drops bright
    // strokes such as lettering; the band-pass drops canopy texture and the regions' own tones)
    let r_o = (2.0 * side / 1024.0).round().max(1.0) as usize;
    let opened = min_max(&min_max(&lum, w, h, r_o, false), w, h, r_o, true);
    let fine = blur(
        &opened,
        Some(&landw),
        w,
        h,
        (side / 340.0).max(1.0) as usize,
    );
    let coarse = blur(&opened, Some(&landw), w, h, (side / 70.0).max(4.0) as usize);
    let hp: Vec<f32> = (0..n)
        .map(|k| {
            if class[k] == LAND {
                fine[k] - coarse[k]
            } else {
                0.0
            }
        })
        .collect();
    let land_n = class.iter().filter(|c| **c == LAND).count().max(1);
    let sd = (hp.iter().map(|v| v * v).sum::<f32>() / land_n as f32)
        .sqrt()
        .max(1.0);
    let relief: Vec<f32> = hp
        .iter()
        .map(|v| (v / (2.5 * sd)).clamp(-1.0, 1.0))
        .collect();

    // --- land regions
    let (seg, wet) = segment(&lab, &contrast, &class, &ignored, w, h, settings, sens);
    let density = vegetation(&lab, &contrast, &class, w, h);

    Base {
        w,
        h,
        rgb,
        lab,
        class,
        seg,
        shallow,
        relief,
        contrast,
        wet,
        density,
        rivers,
        ignored,
        panels,
        millis: t0.elapsed().as_secs_f32() * 1000.0,
    }
}

/// Replays the user's edits on the automatic result and describes the regions.
pub fn finish(base: &Base, ops: &[Op]) -> Analysis {
    let (w, h) = (base.w, base.h);
    let n = w * h;
    let mut class = base.class.clone();
    let mut reg = base.seg.clone();
    let mut count = reg
        .iter()
        .filter(|r| **r != NONE)
        .map(|r| *r as usize + 1)
        .max()
        .unwrap_or(0);
    // per region: (name, biome, role) set by the user
    let mut labels: Vec<(Option<String>, Option<String>, Option<Role>)> =
        vec![(None, None, None); count];
    let mut stale = 0;
    let at = |uv: [f32; 2]| -> Option<usize> {
        let x = (uv[0] * w as f32).floor();
        let y = (uv[1] * h as f32).floor();
        (x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h)
            .then(|| y as usize * w + x as usize)
    };
    for op in ops {
        match op {
            Op::Label {
                at: p,
                name,
                biome,
                role,
            } => {
                let Some(r) = at(*p).and_then(|k| region_near(&reg, w, h, k, 4)) else {
                    stale += 1;
                    continue;
                };
                let l = &mut labels[r as usize];
                if name.is_some() {
                    l.0 = name.clone();
                }
                if biome.is_some() {
                    l.1 = biome.clone();
                }
                if role.is_some() {
                    l.2 = *role;
                }
            }
            Op::Merge { a, b } => {
                let ra = at(*a).and_then(|k| region_near(&reg, w, h, k, 4));
                let rb = at(*b).and_then(|k| region_near(&reg, w, h, k, 4));
                match (ra, rb) {
                    (Some(ra), Some(rb)) if ra != rb => {
                        reg.iter_mut().filter(|r| **r == rb).for_each(|r| *r = ra);
                        let lb = labels[rb as usize].clone();
                        let la = &mut labels[ra as usize];
                        la.0 = la.0.take().or(lb.0);
                        la.1 = la.1.take().or(lb.1);
                        la.2 = la.2.or(lb.2);
                    }
                    _ => stale += 1,
                }
            }
            Op::Split { line } => {
                if split_region(&mut reg, w, h, line, count as u32) {
                    count += 1;
                    labels.push((None, None, None));
                } else {
                    stale += 1;
                }
            }
            Op::Delete { at: p } => {
                let Some(r) = at(*p).and_then(|k| region_near(&reg, w, h, k, 4)) else {
                    stale += 1;
                    continue;
                };
                let mut known = vec![true; n];
                for k in 0..n {
                    if reg[k] == r {
                        known[k] = false;
                    }
                }
                // the cells become what surrounds them (class and region)
                let src = nearest_known(&known, w, h);
                for k in 0..n {
                    if !known[k] {
                        let s = src[k] as usize;
                        class[k] = class[s];
                        reg[k] = reg[s];
                    }
                }
            }
        }
    }

    // --- describe, largest first
    let mut cells = vec![0usize; count];
    for r in &reg {
        if *r != NONE {
            cells[*r as usize] += 1;
        }
    }
    let mut order: Vec<usize> = (0..count).filter(|r| cells[*r] > 0).collect();
    order.sort_by(|a, b| cells[*b].cmp(&cells[*a]).then(a.cmp(b)));
    let mut remap = vec![NONE; count];
    for (i, r) in order.iter().enumerate() {
        remap[*r] = i as u32;
    }
    for r in reg.iter_mut() {
        if *r != NONE {
            *r = remap[*r as usize];
        }
    }
    let nreg = order.len();
    let land: usize = cells.iter().sum::<usize>().max(1);
    // per region sums
    let mut acc = vec![[0f64; 8]; nreg]; // r g b x y contrast wet purple
    let mut lab_acc = vec![[0f64; 3]; nreg];
    for k in 0..n {
        let r = reg[k];
        if r == NONE {
            continue;
        }
        let (x, y) = ((k % w) as f64 + 0.5, (k / w) as f64 + 0.5);
        let a = &mut acc[r as usize];
        let c = base.rgb[k];
        a[0] += c[0] as f64;
        a[1] += c[1] as f64;
        a[2] += c[2] as f64;
        a[3] += x;
        a[4] += y;
        a[5] += base.contrast[k] as f64;
        a[6] += base.wet[k] as f64;
        let (ch, hue) = chroma_hue(base.lab[k]);
        a[7] += (ch >= 15.0 && (275.0..345.0).contains(&hue)) as u8 as f64;
        let l = &mut lab_acc[r as usize];
        for d in 0..3 {
            l[d] += base.lab[k][d] as f64;
        }
    }
    // poles: the cell of each region farthest from its edge
    let edge: Vec<bool> = (0..n)
        .map(|k| {
            let r = reg[k];
            if r == NONE {
                return true;
            }
            let (x, y) = (k % w, k / w);
            x == 0
                || y == 0
                || x == w - 1
                || y == h - 1
                || reg[k - 1] != r
                || reg[k + 1] != r
                || reg[k - w] != r
                || reg[k + w] != r
        })
        .collect();
    let inner = edt(&edge, w, h);
    let mut pole = vec![(0usize, -1.0f32); nreg];
    for k in 0..n {
        let r = reg[k];
        if r != NONE && inner[k] > pole[r as usize].1 {
            pole[r as usize] = (k, inner[k]);
        }
    }
    let mut regions = vec![];
    let mut name_count: std::collections::HashMap<String, usize> = Default::default();
    let all_contrast: Vec<f32> = (0..nreg)
        .map(|i| (acc[i][5] / cells[order[i]] as f64) as f32)
        .collect();
    let mut sorted_c = all_contrast.clone();
    sorted_c.sort_by(|a, b| a.total_cmp(b));
    let median_c = sorted_c.get(sorted_c.len() / 2).copied().unwrap_or(0.3);
    // wetness of the land as a whole (shoreline density per land cell)
    let land_wet = (0..n)
        .filter(|k| reg[*k] != NONE)
        .map(|k| base.wet[k] as f64)
        .sum::<f64>()
        / land as f64;
    for i in 0..nreg {
        let c = cells[order[i]] as f64;
        let a = &acc[i];
        let (mx, my) = (a[3] / c, a[4] / c);
        let lab = [
            (lab_acc[i][0] / c) as f32,
            (lab_acc[i][1] / c) as f32,
            (lab_acc[i][2] / c) as f32,
        ];
        let contrast = all_contrast[i];
        let looks = Looks {
            lab,
            contrast,
            median_contrast: median_c,
            wet: (a[6] / c / land_wet.max(1e-6)) as f32,
            purple: (a[7] / c) as f32,
        };
        let (biome, role) = suggest(&looks);
        let (uname, ubiome, urole) = labels[order[i]].clone();
        let biome_now = ubiome.clone().unwrap_or_else(|| biome.to_string());
        let auto_name = {
            let base_name = auto_name(&biome_now, urole.unwrap_or(role));
            let k = name_count.entry(base_name.to_string()).or_insert(0);
            *k += 1;
            if *k == 1 {
                base_name.to_string()
            } else {
                format!("{base_name} {k}")
            }
        };
        let pk = pole[i].0;
        let r_idx = i as u32;
        let outline = trace_loops(w, h, |k| reg[k] == r_idx)
            .into_iter()
            .filter(|l| polygon_area(l).abs() >= 3.0)
            .map(|l| {
                simplify_closed(&l, 0.75)
                    .into_iter()
                    .map(|p| [p[0] / w as f32, p[1] / h as f32])
                    .collect()
            })
            .collect();
        regions.push(Region {
            id: i as u32 + 1,
            named: uname.is_some() || ubiome.is_some() || urole.is_some(),
            name: uname.unwrap_or(auto_name),
            color: [(a[0] / c) as u8, (a[1] / c) as u8, (a[2] / c) as u8],
            cells: c as usize,
            frac: c as f32 / land as f32,
            centroid: [(mx / w as f64) as f32, (my / h as f64) as f32],
            pole: [
                ((pk % w) as f32 + 0.5) / w as f32,
                ((pk / w) as f32 + 0.5) / h as f32,
            ],
            biome: biome_now,
            role: urole.unwrap_or(role),
            suggested_biome: biome.to_string(),
            suggested_role: role,
            outline,
        });
    }

    // --- lakes
    let lake_key: Vec<u32> = class
        .iter()
        .map(|c| if *c == LAKE { 0 } else { NONE })
        .collect();
    let (comp, sizes) = components(&lake_key, w, h, true);
    let mut lakes = vec![];
    for (ci, sz) in sizes.iter().enumerate() {
        let ci = ci as u32;
        let loops = trace_loops(w, h, |k| comp[k] == ci);
        let Some(outer) = loops
            .into_iter()
            .max_by(|a, b| polygon_area(a).abs().total_cmp(&polygon_area(b).abs()))
        else {
            continue;
        };
        let outline = simplify_closed(&outer, 0.6)
            .into_iter()
            .map(|p| [p[0] / w as f32, p[1] / h as f32])
            .collect();
        lakes.push(LakeArea {
            outline,
            cells: *sz as usize,
        });
    }

    Analysis {
        w,
        h,
        class,
        region: reg,
        regions,
        lakes,
        rivers: base.rivers.clone(),
        stale_ops: stale,
    }
}

// ------------------------------------------------------------------ colour

fn srgb_lut() -> [f32; 256] {
    std::array::from_fn(|i| {
        let v = i as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    })
}

fn srgb_to_lab(c: [u8; 3], lut: &[f32; 256]) -> [f32; 3] {
    let (r, g, b) = (lut[c[0] as usize], lut[c[1] as usize], lut[c[2] as usize]);
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn chroma_hue(c: [f32; 3]) -> (f32, f32) {
    let ch = (c[1] * c[1] + c[2] * c[2]).sqrt();
    let hue = c[2].atan2(c[1]).to_degrees().rem_euclid(360.0);
    (ch, hue)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Water,
    Land,
    /// whites and light greys: clouds, foam, snow, lettering
    Light,
    /// dark greys: shadows, black rock
    Dark,
    Unknown,
}

/// Water, land or ambiguous for a colour class centre.
fn judge(c: [f32; 3]) -> Kind {
    let (ch, hue) = chroma_hue(c);
    let l = c[0];
    let neutral = ch < if l > 62.0 { 10.0 } else { 8.0 };
    if neutral {
        return if l >= 52.0 { Kind::Light } else { Kind::Dark };
    }
    // blue, cyan and teal water; purple (a* high) is land (enchanted isles, heather)
    if (165.0..=295.0).contains(&hue) && c[1] < 16.0 {
        Kind::Water
    } else {
        Kind::Land
    }
}

/// Per-cell kind from k-means colour classes. Illustrated maps paint the land in detail and the
/// water smooth: broad patches of detailed blue (shadowed snow and rock, enchanted isles) are
/// land, while thin blue lines (rivers, detailed only by their banks) stay water. Ambiguous
/// patches are settled by their surroundings.
fn classify(
    lab: &[[f32; 3]],
    soft: &[[f32; 3]],
    lines: &[bool],
    ignored: &[bool],
    w: usize,
    h: usize,
) -> Vec<Kind> {
    let n = w * h;
    let step = (n / 60_000).max(1);
    let samples: Vec<[f32; 3]> = (0..n)
        .step_by(step)
        .filter(|k| !ignored[*k])
        .map(|k| soft[k])
        .collect();
    let cents = kmeans(&samples, 20, 14, 0x5eed_c0de);
    let kinds: Vec<Kind> = cents.iter().map(|c| judge(*c)).collect();
    let mut kind: Vec<Kind> = (0..n)
        .into_par_iter()
        .map(|k| {
            if ignored[k] {
                Kind::Unknown
            } else {
                kinds[nearest(&cents, &soft[k])]
            }
        })
        .collect();

    // texture: mean absolute detail of the brightness
    let lum: Vec<f32> = lab.iter().map(|c| c[0]).collect();
    let near = blur(&lum, None, w, h, 1);
    let detail: Vec<f32> = lum.iter().zip(&near).map(|(a, b)| (a - b).abs()).collect();
    let tex = blur(&detail, None, w, h, 2);
    // how detailed typical water and typical land are
    let median = |want: Kind| -> Option<f32> {
        let mut v: Vec<f32> = (0..n)
            .step_by(step)
            .filter(|k| kind[*k] == want)
            .map(|k| tex[k])
            .collect();
        if v.len() < 50 {
            return None;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        Some(v[v.len() / 2])
    };
    if let (Some(tw), Some(tl)) = (median(Kind::Water), median(Kind::Land))
        && tl > tw * 1.8
    {
        let detailed_above = ((tw + tl) * 0.5).clamp(tw * 1.4, tl * 0.9);
        // broad patches of greyish or violet blue (the water's opening) that are detailed; the
        // sea's own blues stay water even where surf makes them busy
        let greyish = |k: usize| {
            let (ch, hue) = chroma_hue(soft[k]);
            ch < 13.0 || hue > 262.0
        };
        let not_water: Vec<bool> = kind.iter().map(|k| *k != Kind::Water).collect();
        let dw = edt(&not_water, w, h);
        let r = 2.0;
        let core: Vec<bool> = dw.iter().map(|d| *d > r).collect();
        let dcore = edt(&core, w, h);
        let sus: Vec<u32> = (0..n)
            .map(|k| {
                if kind[k] == Kind::Water
                    && dcore[k] <= r + 0.5
                    && tex[k] > detailed_above
                    && greyish(k)
                {
                    0
                } else {
                    NONE
                }
            })
            .collect();
        let (comp, sizes) = components(&sus, w, h, true);
        // big whites (snowfields, not lettering) count as land around a patch
        let light: Vec<u32> = kind
            .iter()
            .map(|k| if *k == Kind::Light { 0 } else { NONE })
            .collect();
        let (lcomp, lsizes) = components(&light, w, h, true);
        let big_light = 0.0005 * n as f32;
        let min = 12.0 * (w.max(h) as f32 / 1024.0).powi(2);
        let mut done = vec![false; sizes.len()];
        // a patch must lean on land (lettering and compass roses out at sea don't); patches next
        // to converted ones are looked at again
        for _ in 0..4 {
            let mut landish = vec![0u32; sizes.len()];
            let mut wet = vec![0u32; sizes.len()];
            for k in 0..n {
                let c = comp[k];
                if c == NONE || done[c as usize] {
                    continue;
                }
                let (x, y) = (k % w, k / w);
                for (dx, dy) in NB8 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let kk = ny as usize * w + nx as usize;
                    if comp[kk] == c {
                        continue;
                    }
                    match kind[kk] {
                        Kind::Land | Kind::Dark => landish[c as usize] += 1,
                        Kind::Light if lsizes[lcomp[kk] as usize] as f32 >= big_light => {
                            landish[c as usize] += 1
                        }
                        Kind::Water => wet[c as usize] += 1,
                        _ => {}
                    }
                }
            }
            let take: Vec<bool> = (0..sizes.len())
                .map(|c| {
                    !done[c]
                        && sizes[c] as f32 >= min
                        && landish[c] as f32 >= 0.25 * (landish[c] + wet[c]) as f32
                })
                .collect();
            if !take.iter().any(|t| *t) {
                break;
            }
            let big: Vec<bool> = comp
                .iter()
                .map(|c| *c != NONE && take[*c as usize])
                .collect();
            // the patch and the water cells hugging it become land
            let dbig = edt(&big, w, h);
            for k in 0..n {
                if kind[k] == Kind::Water && dbig[k] <= r + 0.5 {
                    kind[k] = Kind::Land;
                }
            }
            for (c, t) in take.iter().enumerate() {
                done[c] |= *t;
            }
        }
    }

    // ambiguous patches: what surrounds them
    for amb in [Kind::Light, Kind::Dark] {
        let key: Vec<u32> = kind
            .iter()
            .map(|k| if *k == amb { 0 } else { NONE })
            .collect();
        let (comp, sizes) = components(&key, w, h, true);
        let mut wc = vec![0u32; sizes.len()];
        let mut lc = vec![0u32; sizes.len()];
        let mut bc = vec![0u32; sizes.len()];
        for k in 0..n {
            let c = comp[k];
            if c == NONE {
                continue;
            }
            let (x, y) = (k % w, k / w);
            if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                bc[c as usize] += 1;
            }
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                match kind[ny as usize * w + nx as usize] {
                    Kind::Water => wc[c as usize] += 1,
                    Kind::Land => lc[c as usize] += 1,
                    _ => {}
                }
            }
        }
        let verdict: Vec<Kind> = (0..sizes.len())
            .map(|c| {
                let (wv, lv, bv) = (wc[c] as f32, lc[c] as f32, bc[c] as f32);
                let water = match amb {
                    // clouds over the sea, foam, lettering on water
                    Kind::Light => wv + 2.0 * bv > lv,
                    // shadows on the water only when nearly all around is water
                    _ => wv + bv > 3.0 * lv + 4.0,
                };
                if water { Kind::Water } else { Kind::Land }
            })
            .collect();
        for k in 0..n {
            if comp[k] != NONE {
                kind[k] = verdict[comp[k] as usize];
            }
        }
    }
    // rivers: thin lines bluer than their banks (they blur into the banks in colour classes)
    for k in 0..n {
        if lines[k] && !ignored[k] {
            kind[k] = Kind::Water;
        }
    }
    kind
}

// ------------------------------------------------------------------ water

/// Thin lines bluer than what's beside them (rivers and streams): Frangi "vesselness" (ridges of
/// blueness that are long, not blobs) at two scales, with hysteresis (clearly line-like stretches
/// extended along fainter ones), on ground that isn't itself bluish (shadow streaks on snow and
/// rock) or red (lava). Strokes shorter than a few widths are dropped.
fn water_lines(lab: &[[f32; 3]], w: usize, h: usize) -> Vec<bool> {
    let n = w * h;
    // tuned on a 1536 px map
    let unit = w.max(h) as f32 / 1536.0;
    let q: Vec<f32> = lab.iter().map(|c| -c[2] - 0.3 * c[1]).collect();
    let mut v = vec![0f32; n];
    for scale in [1.0f32, 2.0] {
        let r = (scale * unit).round().max(1.0) as usize;
        let g = blur(
            &blur(&blur(&q, None, w, h, r), None, w, h, r),
            None,
            w,
            h,
            r,
        );
        let s2 = (r * (r + 1)) as f32;
        v.par_iter_mut().enumerate().for_each(|(k, out)| {
            let (x, y) = (k % w, k / w);
            if x == 0 || y == 0 || x + 1 >= w || y + 1 >= h {
                return;
            }
            let at = |dx: i32, dy: i32| g[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize];
            let c = g[k];
            let dxx = (at(1, 0) - 2.0 * c + at(-1, 0)) * s2;
            let dyy = (at(0, 1) - 2.0 * c + at(0, -1)) * s2;
            let dxy = (at(1, 1) - at(1, -1) - at(-1, 1) + at(-1, -1)) * 0.25 * s2;
            let t = ((dxx - dyy).powi(2) + 4.0 * dxy * dxy).sqrt();
            let (m1, m2) = ((dxx + dyy + t) * 0.5, (dxx + dyy - t) * 0.5);
            let (l1, l2) = if m1.abs() > m2.abs() {
                (m2, m1)
            } else {
                (m1, m2)
            };
            // a ridge: strongly curved across, flat along
            if l2 >= 0.0 {
                return;
            }
            let rb = l1.abs() / l2.abs().max(1e-6);
            let st = l1 * l1 + l2 * l2;
            *out = out.max((-2.0 * rb * rb).exp() * (1.0 - (-st / 200.0).exp()));
        });
    }
    let r = (4.0 * unit).round().max(2.0) as usize;
    let around = min_max(&min_max(&q, w, h, r, false), w, h, r, true);
    let gate = |k: usize| q[k] > -15.0 && lab[k][0] < 72.0 && lab[k][1] < 12.0 && around[k] < -5.0;
    let mut on = vec![false; n];
    let mut stack: Vec<usize> = (0..n).filter(|k| v[*k] > 0.3 && gate(*k)).collect();
    for &k in &stack {
        on[k] = true;
    }
    while let Some(k) = stack.pop() {
        let (x, y) = ((k % w) as i32, (k / w) as i32);
        for (dx, dy) in NB8 {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let kk = ny as usize * w + nx as usize;
            if !on[kk] && v[kk] > 0.08 && gate(kk) {
                on[kk] = true;
                stack.push(kk);
            }
        }
    }
    let line: Vec<u32> = on.iter().map(|o| if *o { 0 } else { NONE }).collect();
    let (comp, sizes) = components(&line, w, h, true);
    let mut ext = vec![[usize::MAX, usize::MAX, 0usize, 0usize]; sizes.len()];
    for k in 0..n {
        if comp[k] != NONE {
            let e = &mut ext[comp[k] as usize];
            let (x, y) = (k % w, k / w);
            e[0] = e[0].min(x);
            e[1] = e[1].min(y);
            e[2] = e[2].max(x);
            e[3] = e[3].max(y);
        }
    }
    let keep: Vec<bool> = ext
        .iter()
        .map(|e| ((e[2] - e[0]) as f32).hypot((e[3] - e[1]) as f32) >= 30.0 * unit)
        .collect();
    (0..n)
        .map(|k| comp[k] != NONE && keep[comp[k] as usize])
        .collect()
}

/// Separable square min (`max` false) or max filter of radius `r`.
fn min_max(v: &[f32], w: usize, h: usize, r: usize, max: bool) -> Vec<f32> {
    let pick = |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let init = if max { f32::MIN } else { f32::MAX };
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut m = init;
            for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                m = pick(m, v[y * w + xx]);
            }
            *o = m;
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut m = init;
            for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                m = pick(m, tmp[yy * w + x]);
            }
            *o = m;
        }
    });
    out
}

/// Drops land specks and line art / lettering (thin strokes far longer than wide), and fills
/// water pinholes in land.
fn clean_land(water: &mut [bool], w: usize, h: usize, sens: f32) {
    let n = w * h;
    let side = w.max(h) as f32;
    let land_key: Vec<u32> = water.iter().map(|w| if *w { NONE } else { 0 }).collect();
    let (comp, sizes) = components(&land_key, w, h, true);
    let dt = edt(water, w, h);
    let mut thick = vec![0f32; sizes.len()];
    for k in 0..n {
        if comp[k] != NONE {
            let t = &mut thick[comp[k] as usize];
            *t = t.max(dt[k]);
        }
    }
    let min_island = (lerp(16.0, 4.0, sens) * (side / 1024.0).powi(2)).max(3.0);
    let unit = side / 1024.0;
    let drop: Vec<bool> = (0..sizes.len())
        .map(|c| {
            let (a, t) = (sizes[c] as f32, thick[c]);
            // how much longer than wide: 1 for a round islet, far more for strokes
            let thinness = a / (std::f32::consts::PI * t * t).max(1.0);
            // a speck; hairlines (frames, scale bars); lettering and compass roses
            a < min_island
                || (t <= 1.6 * unit && thinness > 2.5)
                || (a < 0.003 * n as f32 && t <= 4.0 * unit && thinness > 6.0)
        })
        .collect();
    for k in 0..n {
        if comp[k] != NONE && drop[comp[k] as usize] {
            water[k] = true;
        }
    }
    // water pinholes inside land (dark specks)
    let wkey: Vec<u32> = water.iter().map(|w| if *w { 0 } else { NONE }).collect();
    let (wcomp, wsizes) = components(&wkey, w, h, false);
    let mut border = vec![false; wsizes.len()];
    for k in 0..n {
        let (x, y) = (k % w, k / w);
        if wcomp[k] != NONE && (x == 0 || y == 0 || x == w - 1 || y == h - 1) {
            border[wcomp[k] as usize] = true;
        }
    }
    let min_pond = 5.0 * (side / 1024.0).powi(2);
    for k in 0..n {
        let c = wcomp[k];
        if c != NONE && !border[c as usize] && (wsizes[c as usize] as f32) < min_pond {
            water[k] = false;
        }
    }
}

/// Sea, lakes and rivers from the water mask: (class per cell, river centrelines).
fn split_water(water: &[bool], w: usize, h: usize, sens: f32) -> (Vec<u8>, Vec<RiverLine>) {
    let n = w * h;
    let side = w.max(h) as f32;
    let land: Vec<bool> = water.iter().map(|w| !w).collect();
    // distance to land (in water) and the morphological opening of the water
    let dw = edt(&land, w, h);
    let r_open = (side / 300.0).max(2.0);
    let core: Vec<bool> = dw.iter().map(|d| *d > r_open).collect();
    let dcore = edt(&core, w, h);
    let open: Vec<bool> = (0..n)
        .map(|k| water[k] && dcore[k] <= r_open + 0.5)
        .collect();

    let mut class = vec![LAND; n];
    // open water: sea when it reaches the border or is very large, else a lake
    let okey: Vec<u32> = open.iter().map(|o| if *o { 0 } else { NONE }).collect();
    let (ocomp, osizes) = components(&okey, w, h, true);
    let mut obord = vec![false; osizes.len()];
    for k in 0..n {
        let (x, y) = (k % w, k / w);
        if ocomp[k] != NONE && (x == 0 || y == 0 || x == w - 1 || y == h - 1) {
            obord[ocomp[k] as usize] = true;
        }
    }
    let min_lake = lerp(90.0, 30.0, sens) * (side / 1024.0).powi(2);
    let mut thin = vec![false; n];
    for k in 0..n {
        if !water[k] {
            continue;
        }
        let c = ocomp[k];
        if c == NONE {
            thin[k] = true;
            continue;
        }
        let sz = osizes[c as usize] as f32;
        class[k] = if obord[c as usize] || sz > 0.02 * n as f32 {
            SEA
        } else if sz >= min_lake {
            LAKE
        } else {
            thin[k] = true;
            LAND
        };
    }

    // thin water: rivers, or part of what it touches
    let tkey: Vec<u32> = thin.iter().map(|t| if *t { 0 } else { NONE }).collect();
    let (tcomp, tsizes) = components(&tkey, w, h, true);
    let mut members: Vec<Vec<usize>> = vec![vec![]; tsizes.len()];
    for k in 0..n {
        if tcomp[k] != NONE {
            members[tcomp[k] as usize].push(k);
        }
    }
    let min_river = lerp(0.045, 0.012, sens) * side;
    let mut rivers = vec![];
    let mut skel = vec![false; n];
    for cells in members.iter() {
        // what it touches
        let (mut sea_touch, mut lake_touch) = (vec![], false);
        for &k in cells {
            let (x, y) = (k % w, k / w);
            for (dx, dy) in NB8 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                match class[ny as usize * w + nx as usize] {
                    SEA if water[ny as usize * w + nx as usize] => {
                        sea_touch.push(k);
                        break;
                    }
                    LAKE => lake_touch = true,
                    _ => {}
                }
            }
        }
        // skeleton and its length / width
        for &k in cells {
            skel[k] = true;
        }
        thin_mask(&mut skel, w, h, cells);
        let sk: Vec<usize> = cells.iter().copied().filter(|k| skel[*k]).collect();
        for &k in cells {
            skel[k] = false;
        }
        let len = sk.len() as f32;
        let width = if sk.is_empty() {
            1.0
        } else {
            2.0 * sk.iter().map(|k| dw[*k]).sum::<f32>() / sk.len() as f32
        };
        // straits: thin water joining the sea in several places
        let sea_contacts = contact_groups(&sea_touch, w);
        let river = len >= min_river
            && len >= 5.0 * width
            && !(sea_contacts >= 2 && !lake_touch && len < 12.0 * width);
        if river {
            for &k in cells {
                class[k] = RIVER;
            }
            let mut on = vec![false; 0];
            on.resize(n, false);
            for &k in &sk {
                on[k] = true;
            }
            for path in longest_paths(&mut on, &sk, w, h, min_river) {
                let pts: Vec<[f32; 2]> = path
                    .iter()
                    .map(|k| [(k % w) as f32 + 0.5, (k / w) as f32 + 0.5])
                    .collect();
                let wid: Vec<f32> = path.iter().map(|k| (2.0 * dw[*k]).max(1.0)).collect();
                rivers.push(RiverLine {
                    pts,
                    width: wid,
                    mouth: [false; 2],
                });
            }
        } else {
            let to = if !sea_touch.is_empty() {
                SEA
            } else if lake_touch {
                LAKE
            } else if cells.len() as f32 >= min_lake {
                LAKE
            } else {
                LAND
            };
            for &k in cells {
                class[k] = to;
            }
        }
    }

    // river ends meeting the sea or a lake
    let body: Vec<bool> = class.iter().map(|c| *c == SEA || *c == LAKE).collect();
    let dbody = edt(&body, w, h);
    for r in rivers.iter_mut() {
        for (e, idx) in [(0usize, 0usize), (1, r.pts.len() - 1)] {
            let p = r.pts[idx];
            let k = (p[1] as usize).min(h - 1) * w + (p[0] as usize).min(w - 1);
            r.mouth[e] = dbody[k] <= r.width[idx] * 0.5 + 2.5;
        }
        let simplified = simplify_open(&r.pts, 0.6);
        let widths = simplified
            .iter()
            .map(|p| {
                let i = r.pts.iter().position(|q| q == p).unwrap_or(0);
                r.width[i]
            })
            .collect();
        r.pts = simplified;
        r.width = widths;
    }
    rivers.retain(|r| r.pts.len() >= 2);
    (class, rivers)
}

/// Number of separate places (8-connected groups) among contact cells.
fn contact_groups(cells: &[usize], w: usize) -> usize {
    if cells.is_empty() {
        return 0;
    }
    let set: std::collections::HashSet<usize> = cells.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let mut groups = 0;
    for &c in cells {
        if !seen.insert(c) {
            continue;
        }
        groups += 1;
        let mut stack = vec![c];
        while let Some(k) = stack.pop() {
            let (x, y) = ((k % w) as i64, (k / w) as i64);
            for (dx, dy) in NB8 {
                let (nx, ny) = (x + dx as i64, y + dy as i64);
                if nx < 0 || ny < 0 || nx >= w as i64 {
                    continue;
                }
                let kk = ny as usize * w + nx as usize;
                if set.contains(&kk) && seen.insert(kk) {
                    stack.push(kk);
                }
            }
        }
    }
    groups
}

/// Sea shallowness from brightness: light turquoise water is shallow.
fn sea_shallowness(lab: &[[f32; 3]], class: &[u8], w: usize, h: usize) -> Vec<f32> {
    let n = w * h;
    let mut ls: Vec<f32> = (0..n)
        .filter(|k| class[*k] == SEA)
        .map(|k| lab[k][0])
        .collect();
    if ls.len() < 16 {
        return vec![0.0; n];
    }
    ls.sort_by(|a, b| a.total_cmp(b));
    let med = ls[ls.len() / 2];
    let hi = ls[(ls.len() as f32 * 0.93) as usize].max(med + 8.0);
    // only turquoise and cyan water counts (light clouds, foam and lettering don't)
    let raw: Vec<f32> = (0..n)
        .map(|k| {
            if class[k] != SEA {
                return 0.0;
            }
            let (ch, hue) = chroma_hue(lab[k]);
            let cyan = smoothstep01((ch - 10.0) / 14.0)
                * if (160.0..245.0).contains(&hue) {
                    1.0
                } else {
                    0.0
                };
            ((lab[k][0] - med) / (hi - med)).clamp(0.0, 1.0) * cyan
        })
        .collect();
    let wt: Vec<f32> = class
        .iter()
        .map(|c| if *c == SEA { 1.0 } else { 0.0 })
        .collect();
    blur(&raw, Some(&wt), w, h, (w.max(h) / 200).max(3))
}

fn smoothstep01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

// ------------------------------------------------------------------ vegetation

/// Vegetation density per cell from the picture's colour and texture: green and dark and
/// textured (canopy) is dense forest, light smooth green is open grassland, warm hues (desert,
/// badlands) carry sparse shrubs, neutral greys and whites (rock, snow) are bare.
fn vegetation(lab: &[[f32; 3]], contrast: &[f32], class: &[u8], w: usize, h: usize) -> Vec<f32> {
    let n = w * h;
    let raw: Vec<f32> = (0..n)
        .map(|k| {
            if class[k] != LAND {
                return 0.0;
            }
            let (ch, hue) = chroma_hue(lab[k]);
            let l = lab[k][0];
            let colour = smoothstep01((ch - 4.0) / 18.0);
            let green = if (80.0..175.0).contains(&hue) {
                1.0
            } else if (60.0..80.0).contains(&hue) || (175.0..220.0).contains(&hue) {
                0.45
            } else if (20.0..60.0).contains(&hue) {
                0.08
            } else {
                0.25
            };
            let dark = smoothstep01((72.0 - l) / 45.0);
            let canopy = smoothstep01((contrast[k] - 0.12) / 0.4);
            (green * colour.max(0.35 * dark) * (0.2 + 0.8 * dark) * (0.45 + 0.55 * canopy))
                .clamp(0.0, 1.0)
        })
        .collect();
    let wt: Vec<f32> = class
        .iter()
        .map(|c| if *c == LAND { 1.0 } else { 0.0 })
        .collect();
    let r = (w.max(h) / 300).max(2);
    blur(&raw, Some(&wt), w, h, r)
        .iter()
        .zip(&wt)
        .map(|(v, m)| v * m)
        .collect()
}

// ------------------------------------------------------------------ regions

/// Region features: regional lightness, a*, b*, texture and wetness.
const F: usize = 5;

/// Automatic land regions. Returns the region index per cell (`NONE` on water) and the wetness
/// map.
#[allow(clippy::too_many_arguments)]
fn segment(
    lab: &[[f32; 3]],
    contrast: &[f32],
    class: &[u8],
    ignored: &[bool],
    w: usize,
    h: usize,
    settings: &Settings,
    sens: f32,
) -> (Vec<u32>, Vec<f32>) {
    let n = w * h;
    let side = w.max(h) as f32;
    let is_land: Vec<bool> = class.iter().map(|c| *c == LAND).collect();
    let land_n = is_land.iter().filter(|l| **l).count();
    if land_n == 0 {
        return (vec![NONE; n], vec![0.0; n]);
    }
    // regional average colour and texture (land only, ignored cells left out)
    let wt: Vec<f32> = (0..n)
        .map(|k| if is_land[k] && !ignored[k] { 1.0 } else { 0.0 })
        .collect();
    let r = (side / 90.0).max(3.0) as usize;
    let chan = |f: &dyn Fn(usize) -> f32| -> Vec<f32> {
        let v: Vec<f32> = (0..n).map(f).collect();
        let once = blur(&v, Some(&wt), w, h, r);
        blur(&once, Some(&wt), w, h, r)
    };
    let fl = chan(&|k| lab[k][0] * 0.7);
    let fa = chan(&|k| lab[k][1]);
    let fb = chan(&|k| lab[k][2]);
    let fc = chan(&|k| contrast[k] * 18.0);
    // shoreline density: swampy, fragmented land has many shores close together
    let shore: Vec<f32> = (0..n)
        .map(|k| {
            let (x, y) = (k % w, k / w);
            let wet = |kk: usize| class[kk] != LAND;
            if class[k] == LAND
                && ((x > 0 && wet(k - 1))
                    || (x + 1 < w && wet(k + 1))
                    || (y > 0 && wet(k - w))
                    || (y + 1 < h && wet(k + w)))
            {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let fs = blur(&shore, None, w, h, (side / 40.0).max(4.0) as usize);
    // wetness (for biome suggestions): how much narrow water (channels, pools, creeks: within a
    // few cells of land) there is around; a swamp is riddled with it, an ordinary coast has a band
    let landm: Vec<bool> = class.iter().map(|c| *c == LAND).collect();
    let dl = edt(&landm, w, h);
    let narrow: Vec<f32> = (0..n)
        .map(|k| {
            if !landm[k] && dl[k] <= 3.0 * side / 1024.0 {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let fw = blur(&narrow, None, w, h, (side / 30.0).max(4.0) as usize);
    let feat: Vec<[f32; F]> = (0..n)
        .map(|k| [fl[k], fa[k], fb[k], fc[k], fs[k] * 250.0])
        .collect();

    let target = if settings.regions > 0 {
        settings.regions as usize
    } else {
        (8.0 + 12.0 * sens).round() as usize
    }
    .clamp(1, 40);
    let land_cells: Vec<usize> = (0..n).filter(|k| is_land[*k] && !ignored[*k]).collect();
    let step = (land_cells.len() / 50_000).max(1);
    let samples: Vec<[f32; F]> = land_cells.iter().step_by(step).map(|k| feat[*k]).collect();
    let k_classes = (target * 2).clamp(8, 48).min(samples.len().max(1));
    let cents = kmeans(&samples, k_classes, 16, 0xa11e_c0de);
    let mut lbl: Vec<u32> = (0..n)
        .into_par_iter()
        .map(|k| {
            if is_land[k] {
                nearest(&cents, &feat[k]) as u32
            } else {
                NONE
            }
        })
        .collect();

    // smooth: majority in a 5×5 window, a few rounds
    for _ in 0..3 {
        lbl = mode_filter(&lbl, w, h, 2, k_classes);
    }
    // ignored land takes the region around it
    let known: Vec<bool> = (0..n).map(|k| is_land[k] && !ignored[k]).collect();
    if known.iter().any(|k| *k) {
        let src = nearest_known(&known, w, h);
        for k in 0..n {
            if is_land[k] && ignored[k] {
                lbl[k] = lbl[src[k] as usize];
            }
        }
    }
    let (comp, sizes) = components(&lbl, w, h, false);
    let nc = sizes.len();

    // region graph: shared edges on land, and "near" edges across up to a little water
    let mut sum = vec![[0f64; F]; nc];
    for k in 0..n {
        let c = comp[k];
        if c != NONE {
            for d in 0..F {
                sum[c as usize][d] += feat[k][d] as f64;
            }
        }
    }
    let mut mean: Vec<[f32; F]> = (0..nc)
        .map(|c| std::array::from_fn(|d| (sum[c][d] / sizes[c].max(1) as f64) as f32))
        .collect();
    let mut size: Vec<f64> = sizes.iter().map(|s| *s as f64).collect();
    let mut edges: std::collections::HashMap<(u32, u32), (f32, bool)> = Default::default();
    let add_edge = |edges: &mut std::collections::HashMap<(u32, u32), (f32, bool)>,
                    a: u32,
                    b: u32,
                    near: bool| {
        if a == b || a == NONE || b == NONE {
            return;
        }
        let key = (a.min(b), a.max(b));
        let e = edges.entry(key).or_insert((0.0, near));
        e.0 += 1.0;
        e.1 &= near;
    };
    for y in 0..h {
        for x in 0..w {
            let k = y * w + x;
            if x + 1 < w {
                add_edge(&mut edges, comp[k], comp[k + 1], false);
            }
            if y + 1 < h {
                add_edge(&mut edges, comp[k], comp[k + w], false);
            }
        }
    }
    // reach across water to what an island faces
    let reach = (side * lerp(0.03, 0.05, sens)).max(4.0);
    let land_mask: Vec<bool> = comp.iter().map(|c| *c != NONE).collect();
    let (near_src, near_d) = nearest_known_dist(&land_mask, w, h);
    for y in 0..h {
        for x in 0..w {
            let k = y * w + x;
            if land_mask[k] || near_d[k] > reach {
                continue;
            }
            for kk in [
                if x + 1 < w { k + 1 } else { k },
                if y + 1 < h { k + w } else { k },
            ] {
                if !land_mask[kk] && near_d[kk] <= reach {
                    add_edge(
                        &mut edges,
                        comp[near_src[k] as usize],
                        comp[near_src[kk] as usize],
                        true,
                    );
                }
            }
        }
    }

    // union–find merging
    let mut parent: Vec<u32> = (0..nc as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let dist = |a: &[f32; F], b: &[f32; F]| -> f32 {
        (0..F).map(|d| (a[d] - b[d]).powi(2)).sum::<f32>().sqrt()
    };
    let mut alive = nc;
    let merge =
        |parent: &mut Vec<u32>, mean: &mut Vec<[f32; F]>, size: &mut Vec<f64>, a: u32, b: u32| {
            // b joins a
            let (sa, sb) = (size[a as usize], size[b as usize]);
            let m: [f32; F] = std::array::from_fn(|d| {
                ((mean[a as usize][d] as f64 * sa + mean[b as usize][d] as f64 * sb) / (sa + sb))
                    as f32
            });
            mean[a as usize] = m;
            size[a as usize] = sa + sb;
            parent[b as usize] = a;
        };
    // current graph between roots
    let roots_graph =
        |parent: &mut Vec<u32>, edges: &std::collections::HashMap<(u32, u32), (f32, bool)>| {
            let mut g: std::collections::HashMap<(u32, u32), (f32, bool)> = Default::default();
            for (&(a, b), &(len, near)) in edges {
                let (ra, rb) = (find(parent, a), find(parent, b));
                if ra != rb {
                    let e = g.entry((ra.min(rb), ra.max(rb))).or_insert((0.0, true));
                    e.0 += len;
                    e.1 &= near;
                }
            }
            g
        };
    // 1. fragments: into the neighbour they share most with (similar colour preferred)
    let min_frag = (land_n as f32 * lerp(0.02, 0.006, sens)).max(12.0) as f64;
    loop {
        let g = roots_graph(&mut parent, &edges);
        let mut best: Option<(f64, u32, u32)> = None; // (size, small, into)
        let mut nbrs: std::collections::HashMap<u32, Vec<(u32, f32, bool)>> = Default::default();
        for (&(a, b), &(len, near)) in &g {
            nbrs.entry(a).or_default().push((b, len, near));
            nbrs.entry(b).or_default().push((a, len, near));
        }
        for (&r, list) in &nbrs {
            if size[r as usize] >= min_frag {
                continue;
            }
            let into = list
                .iter()
                .max_by(|x, y| {
                    let s = |e: &(u32, f32, bool)| {
                        e.1 * if e.2 { 0.3 } else { 1.0 }
                            / (1.0 + dist(&mean[r as usize], &mean[e.0 as usize]) / 10.0)
                    };
                    s(x).total_cmp(&s(y))
                })
                .map(|e| e.0);
            if let Some(into) = into
                && best.is_none_or(|b| size[r as usize] < b.0)
            {
                best = Some((size[r as usize], r, into));
            }
        }
        let Some((_, small, into)) = best else { break };
        merge(&mut parent, &mut mean, &mut size, into, small);
        alive -= 1;
    }
    // 2. most similar neighbours (Ward cost; across water costs more) down to the target; in
    // automatic mode also on while some neighbours are nearly alike
    let alike = if settings.regions == 0 {
        lerp(9.0, 3.0, sens)
    } else {
        0.0
    };
    loop {
        let g = roots_graph(&mut parent, &edges);
        let best = g
            .iter()
            .map(|(&(a, b), &(_, near))| {
                let (sa, sb) = (size[a as usize], size[b as usize]);
                let d = dist(&mean[a as usize], &mean[b as usize]) as f64;
                let cost = sa * sb / (sa + sb) * d * d * if near { 2.5 } else { 1.0 };
                (cost, a, b, d as f32)
            })
            .min_by(|x, y| x.0.total_cmp(&y.0));
        let Some((_, a, b, _)) = best else { break };
        let closest = g
            .iter()
            .filter(|(_, e)| !e.1)
            .map(|(&(a, b), _)| dist(&mean[a as usize], &mean[b as usize]))
            .fold(f32::MAX, f32::min);
        if alive <= target && closest >= alike {
            break;
        }
        let (a, b) = if alive > target {
            (a, b)
        } else {
            // the most alike neighbours on land
            g.iter()
                .filter(|(_, e)| !e.1)
                .map(|(&(a, b), _)| (dist(&mean[a as usize], &mean[b as usize]), a, b))
                .min_by(|x, y| x.0.total_cmp(&y.0))
                .map(|x| (x.1, x.2))
                .unwrap_or((a, b))
        };
        let (a, b) = if size[a as usize] >= size[b as usize] {
            (a, b)
        } else {
            (b, a)
        };
        merge(&mut parent, &mut mean, &mut size, a, b);
        alive -= 1;
    }
    // compact
    let mut idx = vec![NONE; nc];
    let mut next = 0u32;
    let mut out = vec![NONE; n];
    for k in 0..n {
        let c = comp[k];
        if c == NONE {
            continue;
        }
        let r = find(&mut parent, c);
        if idx[r as usize] == NONE {
            idx[r as usize] = next;
            next += 1;
        }
        out[k] = idx[r as usize];
    }
    (out, fw)
}

/// What a region looks like in the picture.
struct Looks {
    /// Mean CIELab colour.
    lab: [f32; 3],
    contrast: f32,
    median_contrast: f32,
    /// Shoreline density relative to the land's average (1 = typical).
    wet: f32,
    /// Share of strongly purple cells.
    purple: f32,
}

/// Biome and height role suggested by a region's look.
fn suggest(lk: &Looks) -> (&'static str, Role) {
    let (ch, hue) = chroma_hue(lk.lab);
    let (l, a) = (lk.lab[0], lk.lab[1]);
    let rocky = lk.contrast > lk.median_contrast * 1.2 && lk.contrast > 0.3;
    let warm = (20.0..80.0).contains(&hue);
    let neutral = ch < 6.0;
    let cool = (150.0..280.0).contains(&hue);
    let biome = if lk.purple > 0.08 || (ch >= 8.0 && (275.0..345.0).contains(&hue) && a > 6.0) {
        "HauntedWoods"
    } else if l >= 70.0 && ch < 10.0 {
        "Tundra"
    } else if l >= 50.0 && ch < 9.0 {
        "Alpine"
    } else if lk.wet > 1.3 && l < 48.0 && lk.contrast < lk.median_contrast {
        // dark, flat and riddled with water
        "Swamp"
    } else if lk.wet > 1.4 && l >= 52.0 {
        "TropicalCoast"
    } else if neutral {
        if l >= 38.0 {
            "Alpine"
        } else if cool {
            "Taiga"
        } else {
            "Volcanic"
        }
    } else if warm {
        if l < 40.0 || (ch < 12.0 && l < 50.0) {
            "Volcanic"
        } else if l >= 70.0 && ch < 30.0 {
            "TropicalCoast"
        } else if ch >= 40.0 && l < 58.0 && hue < 45.0 {
            "AutumnWoods"
        } else {
            "Desert"
        }
    } else if (80.0..100.0).contains(&hue) {
        if l >= 58.0 && ch < 22.0 {
            "TropicalCoast"
        } else if l < 36.0 {
            "TemperateForest"
        } else {
            "Meadow"
        }
    } else if (100.0..175.0).contains(&hue) {
        if l < 46.0 {
            if hue > 145.0 {
                "Taiga"
            } else {
                "TemperateForest"
            }
        } else {
            "Meadow"
        }
    } else if cool {
        if l < 45.0 { "Taiga" } else { "Alpine" }
    } else if l < 40.0 {
        "Volcanic"
    } else {
        "AutumnWoods"
    };
    let role = match biome {
        "Tundra" => Role::Peaks,
        "Alpine" => {
            if l > 64.0 || rocky {
                Role::Peaks
            } else {
                Role::Mountains
            }
        }
        "Volcanic" => {
            if l < 33.0 || lk.contrast >= lk.median_contrast * 1.1 {
                Role::Mountains
            } else {
                Role::Hills
            }
        }
        "Desert" => {
            if rocky {
                Role::Hills
            } else {
                Role::Plains
            }
        }
        "Meadow" => {
            if rocky {
                Role::Hills
            } else {
                Role::Plains
            }
        }
        "Swamp" => Role::Wetland,
        "TropicalCoast" => Role::Lowland,
        _ => {
            if rocky {
                Role::Mountains
            } else {
                Role::Hills
            }
        }
    };
    (biome, role)
}

/// Default region name from its biome and role.
pub fn auto_name(biome: &str, role: Role) -> &'static str {
    match (biome, role) {
        ("Tundra", _) | ("Alpine", Role::Peaks) => "Snowy Peaks",
        ("Alpine", _) => "Highlands",
        ("Volcanic", Role::Peaks | Role::Mountains) => "Ash Mountains",
        ("Volcanic", _) => "Badlands",
        ("Desert", Role::Hills | Role::Mountains | Role::Peaks) => "Rocky Desert",
        ("Desert", _) => "Desert",
        ("TropicalCoast", _) => "Coast",
        ("AutumnWoods", _) => "Autumn Woods",
        ("Swamp", _) => "Swamp",
        ("Taiga", _) => "Pine Forest",
        ("TemperateForest", _) => "Forest",
        ("Jungle", _) => "Jungle",
        ("HauntedWoods", _) => "Haunted Isles",
        ("Meadow", Role::Hills | Role::Mountains) => "Green Hills",
        ("Meadow", _) => "Grasslands",
        _ => "Region",
    }
}

/// Splits the region the line crosses most into the cells on either side. True on success.
fn split_region(reg: &mut [u32], w: usize, h: usize, line: &[[f32; 2]], new_id: u32) -> bool {
    if line.len() < 2 {
        return false;
    }
    let pts: Vec<[f32; 2]> = line
        .iter()
        .map(|p| [p[0] * w as f32, p[1] * h as f32])
        .collect();
    // the region crossed most
    let mut hits: std::collections::HashMap<u32, usize> = Default::default();
    for s in pts.windows(2) {
        let len = ((s[1][0] - s[0][0]).powi(2) + (s[1][1] - s[0][1]).powi(2)).sqrt();
        let steps = (len.ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (
                s[0][0] + (s[1][0] - s[0][0]) * t,
                s[0][1] + (s[1][1] - s[0][1]) * t,
            );
            if x >= 0.0 && y >= 0.0 && (x as usize) < w && (y as usize) < h {
                let r = reg[y as usize * w + x as usize];
                if r != NONE {
                    *hits.entry(r).or_default() += 1;
                }
            }
        }
    }
    let Some((&r, _)) = hits.iter().max_by_key(|e| (*e.1, std::cmp::Reverse(*e.0))) else {
        return false;
    };
    // side of the line (nearest segment) for each of the region's cells
    let mut side_b = vec![];
    let mut count = [0usize; 2];
    for k in 0..w * h {
        if reg[k] != r {
            continue;
        }
        let p = [(k % w) as f32 + 0.5, (k / w) as f32 + 0.5];
        let mut best = (f32::MAX, 0.0f32);
        for s in pts.windows(2) {
            let (ax, ay, bx, by) = (s[0][0], s[0][1], s[1][0], s[1][1]);
            let (dx, dy) = (bx - ax, by - ay);
            let l2 = (dx * dx + dy * dy).max(1e-6);
            let t = (((p[0] - ax) * dx + (p[1] - ay) * dy) / l2).clamp(0.0, 1.0);
            let d = (p[0] - ax - dx * t).powi(2) + (p[1] - ay - dy * t).powi(2);
            if d < best.0 {
                best = (d, dx * (p[1] - ay) - dy * (p[0] - ax));
            }
        }
        let b = best.1 > 0.0;
        count[b as usize] += 1;
        if b {
            side_b.push(k);
        }
    }
    if count[0] < 4 || count[1] < 4 {
        return false;
    }
    for k in side_b {
        reg[k] = new_id;
    }
    true
}

// ------------------------------------------------------------------ panels

/// Framed rectangular panels (legends, cartouches): thin straight frame lines of an even colour
/// closing into a box (the picture's border can stand in for a side). Returns cell rects
/// [x0, y0, x1, y1] (inclusive, with a small margin).
fn find_panels(lab: &[[f32; 3]], w: usize, h: usize) -> Vec<[usize; 4]> {
    if w < 48 || h < 48 {
        return vec![];
    }
    let l = |x: usize, y: usize| lab[y * w + x][0];
    let thr = 6.0;
    // a thin line (1–2 cells) brighter or darker than both sides, across rows / columns
    let line = |a: f32, m1: f32, m2: f32, c: f32, d: f32| -> f32 {
        // one-cell line `m1` between `a` and `c`, or a two-cell line `m1 m2` between `a` and `d`
        let one = (m1 - a.max(c)).max(0.0) - (a.min(c) - m1).max(0.0);
        let mid = (m1 + m2) * 0.5;
        let two = (mid - a.max(d)).max(0.0) - (a.min(d) - mid).max(0.0);
        let v = if one.abs() > two.abs() { one } else { two };
        if v > thr {
            1.0
        } else if v < -thr {
            -1.0
        } else {
            0.0
        }
    };
    let line_h = |x: usize, y: usize| -> f32 {
        line(
            l(x, y - 2),
            l(x, y),
            l(x, y + 1),
            l(x, y + 2),
            l(x, (y + 3).min(h - 1)),
        )
    };
    let line_v = |x: usize, y: usize| -> f32 {
        line(
            l(x - 2, y),
            l(x, y),
            l(x + 1, y),
            l(x + 2, y),
            l((x + 3).min(w - 1), y),
        )
    };
    let min_len = (w.max(h) as f32 * 0.06) as usize;
    // runs of line cells along one row (column), allowing 1-cell gaps, of an even colour
    let runs = |len_a: usize,
                len_b: usize,
                f: &dyn Fn(usize, usize) -> f32,
                val: &dyn Fn(usize, usize) -> f32|
     -> Vec<(usize, usize, usize)> {
        let mut out = vec![];
        for b in 2..len_b - 2 {
            for sign in [1.0, -1.0] {
                let mut start: Option<usize> = None;
                let mut gap = 0;
                for a in 0..=len_a {
                    let on = a < len_a && f(a, b) == sign;
                    if on {
                        start.get_or_insert(a);
                        gap = 0;
                        continue;
                    }
                    let Some(s) = start else { continue };
                    gap += 1;
                    if gap > 2 || a == len_a {
                        let e = a - gap;
                        if e + 1 - s >= min_len {
                            let vals: Vec<f32> = (s..=e).map(|i| val(i, b)).collect();
                            let mean = vals.iter().sum::<f32>() / vals.len() as f32;
                            let sd = (vals.iter().map(|v| (v - mean).powi(2)).sum::<f32>()
                                / vals.len() as f32)
                                .sqrt();
                            if sd < 6.0 {
                                out.push((b, s, e));
                            }
                        }
                        start = None;
                    }
                }
            }
        }
        out
    };
    let mut hr = runs(w, h, &|x, y| line_h(x, y), &|x, y| l(x, y));
    let mut vr = runs(h, w, &|y, x| line_v(x, y), &|y, x| l(x, y));
    // the picture's border can close a box
    hr.extend([(0, 0, w - 1), (h - 1, 0, w - 1)]);
    vr.extend([(0, 0, h - 1), (w - 1, 0, h - 1)]);
    // share of [from, to] covered by the runs lying within 3 cells of `at`
    let cover = |rs: &[(usize, usize, usize)], at: usize, from: usize, to: usize| -> f32 {
        let mut iv: Vec<(usize, usize)> = rs
            .iter()
            .filter(|r| r.0.abs_diff(at) <= 3)
            .map(|r| (r.1.max(from), r.2.min(to)))
            .filter(|r| r.1 > r.0)
            .collect();
        iv.sort();
        let (mut tot, mut end) = (0usize, from);
        for (a, b) in iv {
            let a = a.max(end);
            if b > a {
                tot += b - a;
                end = b;
            }
        }
        tot as f32 / (to - from).max(1) as f32
    };
    let area = (w * h) as f32;
    let mut out: Vec<[usize; 4]> = vec![];
    for top in &hr {
        for bot in &hr {
            if bot.0 < top.0 + min_len / 2 || top.2.min(bot.2) < top.1.max(bot.1) {
                continue;
            }
            let (x0, x1, y0, y1) = (top.1.min(bot.1), top.2.max(bot.2), top.0, bot.0);
            let ra = ((x1 - x0) * (y1 - y0)) as f32;
            if x1 < x0 + min_len || ra < area * 0.004 || ra > area * 0.3 {
                continue;
            }
            // at least three real frame lines (the picture's border may close the fourth side)
            let real = [y0 > 0, y1 < h - 1, x0 > 0, x1 < w - 1]
                .iter()
                .filter(|b| **b)
                .count();
            if real < 3
                || cover(&hr, y0, x0, x1) < 0.7
                || cover(&hr, y1, x0, x1) < 0.7
                || cover(&vr, x0, y0, y1) < 0.6
                || cover(&vr, x1, y0, y1) < 0.6
            {
                continue;
            }
            let r = [
                x0.saturating_sub(3),
                y0.saturating_sub(3),
                (x1 + 3).min(w - 1),
                (y1 + 3).min(h - 1),
            ];
            if out.iter().any(|o| {
                o[0] <= r[0] + 4 && o[1] <= r[1] + 4 && o[2] + 4 >= r[2] && o[3] + 4 >= r[3]
            }) {
                continue;
            }
            out.retain(|o| {
                !(r[0] <= o[0] + 4 && r[1] <= o[1] + 4 && r[2] + 4 >= o[2] && r[3] + 4 >= o[3])
            });
            out.push(r);
        }
    }
    out
}

// ------------------------------------------------------------------ grid helpers

const NB8: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Area-average downscale of RGB8 to at most `max_side` px on the longer side.
fn downscale(px: &[u8], iw: usize, ih: usize, max_side: usize) -> (usize, usize, Vec<[u8; 3]>) {
    let s = (max_side as f32 / iw.max(ih) as f32).min(1.0);
    let w = ((iw as f32 * s).round() as usize).max(1);
    let h = ((ih as f32 * s).round() as usize).max(1);
    let out = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let y0 = y * ih / h;
            let y1 = ((y + 1) * ih / h).max(y0 + 1);
            (0..w).map(move |x| {
                let x0 = x * iw / w;
                let x1 = ((x + 1) * iw / w).max(x0 + 1);
                let mut acc = [0u32; 3];
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let k = (yy * iw + xx) * 3;
                        acc[0] += px[k] as u32;
                        acc[1] += px[k + 1] as u32;
                        acc[2] += px[k + 2] as u32;
                    }
                }
                let c = ((y1 - y0) * (x1 - x0)) as u32;
                [(acc[0] / c) as u8, (acc[1] / c) as u8, (acc[2] / c) as u8]
            })
        })
        .collect();
    (w, h, out)
}

/// Sum over the (2r+1)² window (clamped at the edges).
fn box_sum(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        let mut pre = vec![0f64; w + 1];
        for x in 0..w {
            pre[x + 1] = pre[x] + src[x] as f64;
        }
        for (x, o) in row.iter_mut().enumerate() {
            *o = (pre[(x + r + 1).min(w)] - pre[x.saturating_sub(r)]) as f32;
        }
    });
    let mut pre = vec![0f64; (h + 1) * w];
    for y in 0..h {
        for x in 0..w {
            pre[(y + 1) * w + x] = pre[y * w + x] + tmp[y * w + x] as f64;
        }
    }
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for (x, o) in row.iter_mut().enumerate() {
            *o = (pre[b * w + x] - pre[a * w + x]) as f32;
        }
    });
    out
}

/// Box blur, optionally weighted (normalised convolution: zero-weight cells don't bleed in; a
/// cell with no weight around keeps its value).
pub fn blur(v: &[f32], wt: Option<&[f32]>, w: usize, h: usize, r: usize) -> Vec<f32> {
    if r == 0 {
        return v.to_vec();
    }
    match wt {
        Some(wt) => {
            let num: Vec<f32> = v.iter().zip(wt).map(|(a, b)| a * b).collect();
            let s = box_sum(&num, w, h, r);
            let d = box_sum(wt, w, h, r);
            (0..w * h)
                .map(|k| if d[k] > 1e-4 { s[k] / d[k] } else { v[k] })
                .collect()
        }
        None => {
            let ones = vec![1f32; w * h];
            let s = box_sum(v, w, h, r);
            let d = box_sum(&ones, w, h, r);
            (0..w * h).map(|k| s[k] / d[k]).collect()
        }
    }
}

fn blur3(v: &[[f32; 3]], w: usize, h: usize, r: usize) -> Vec<[f32; 3]> {
    let ch: Vec<Vec<f32>> = (0..3)
        .map(|d| blur(&v.iter().map(|c| c[d]).collect::<Vec<_>>(), None, w, h, r))
        .collect();
    (0..w * h).map(|k| [ch[0][k], ch[1][k], ch[2][k]]).collect()
}

fn dist2<const D: usize>(a: &[f32; D], b: &[f32; D]) -> f32 {
    let mut s = 0.0;
    for d in 0..D {
        s += (a[d] - b[d]).powi(2);
    }
    s
}

fn nearest<const D: usize>(cents: &[[f32; D]], p: &[f32; D]) -> usize {
    let mut best = (f32::MAX, 0);
    for (i, c) in cents.iter().enumerate() {
        let d = dist2(c, p);
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

/// k-means (k-means++ seeding, deterministic).
fn kmeans<const D: usize>(pts: &[[f32; D]], k: usize, iters: usize, seed: u64) -> Vec<[f32; D]> {
    if pts.is_empty() {
        return vec![[0.0; D]];
    }
    let mut s = seed | 1;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let k = k.clamp(1, pts.len());
    let mut cents = vec![pts[(rnd() % pts.len() as u64) as usize]];
    let mut d2: Vec<f32> = pts.par_iter().map(|p| dist2(p, &cents[0])).collect();
    while cents.len() < k {
        let total: f64 = d2.iter().map(|v| *v as f64).sum();
        if total <= 1e-9 {
            break;
        }
        let mut t = (rnd() >> 11) as f64 / (1u64 << 53) as f64 * total;
        let mut pick = pts.len() - 1;
        for (i, v) in d2.iter().enumerate() {
            t -= *v as f64;
            if t <= 0.0 {
                pick = i;
                break;
            }
        }
        let c = pts[pick];
        cents.push(c);
        d2.par_iter_mut()
            .zip(pts.par_iter())
            .for_each(|(d, p)| *d = d.min(dist2(p, &c)));
    }
    for _ in 0..iters {
        let assign: Vec<usize> = pts.par_iter().map(|p| nearest(&cents, p)).collect();
        let mut sum = vec![[0f64; D]; cents.len()];
        let mut cnt = vec![0usize; cents.len()];
        for (p, a) in pts.iter().zip(&assign) {
            for d in 0..D {
                sum[*a][d] += p[d] as f64;
            }
            cnt[*a] += 1;
        }
        let mut moved = 0.0;
        for c in 0..cents.len() {
            if cnt[c] > 0 {
                let m: [f32; D] = std::array::from_fn(|d| (sum[c][d] / cnt[c] as f64) as f32);
                moved += dist2(&m, &cents[c]);
                cents[c] = m;
            }
        }
        if moved < 1e-3 {
            break;
        }
    }
    cents
}

/// Connected components of equal keys (`NONE` cells skipped). Returns (component per cell or
/// `NONE`, size per component).
pub fn components(key: &[u32], w: usize, h: usize, diag: bool) -> (Vec<u32>, Vec<u32>) {
    let n = w * h;
    let mut comp = vec![NONE; n];
    let mut sizes = vec![];
    let mut stack = vec![];
    for start in 0..n {
        if key[start] == NONE || comp[start] != NONE {
            continue;
        }
        let id = sizes.len() as u32;
        let kv = key[start];
        comp[start] = id;
        stack.push(start);
        let mut size = 0u32;
        while let Some(k) = stack.pop() {
            size += 1;
            let (x, y) = ((k % w) as i32, (k / w) as i32);
            for (dx, dy) in NB8 {
                if !diag && dx != 0 && dy != 0 {
                    continue;
                }
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let kk = ny as usize * w + nx as usize;
                if comp[kk] == NONE && key[kk] == kv {
                    comp[kk] = id;
                    stack.push(kk);
                }
            }
        }
        sizes.push(size);
    }
    (comp, sizes)
}

/// Exact Euclidean distance (cells) to the nearest `feature` cell (Felzenszwalb–Huttenlocher).
pub fn edt(feature: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f64 = 1e20;
    fn dt1(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
        let n = f.len();
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        for q in 1..n {
            let mut s;
            loop {
                let p = v[k];
                s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64))
                    / (2.0 * (q as f64 - p as f64));
                if s <= z[k] && k > 0 {
                    k -= 1;
                } else {
                    break;
                }
            }
            if s <= z[k] {
                v[k] = q;
                z[k + 1] = INF;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
            }
        }
        k = 0;
        for (q, out) in d.iter_mut().enumerate().take(n) {
            while z[k + 1] < q as f64 {
                k += 1;
            }
            let p = v[k];
            *out = (q as f64 - p as f64).powi(2) + f[p];
        }
    }
    let mut g: Vec<f64> = feature.iter().map(|b| if *b { 0.0 } else { INF }).collect();
    // columns
    let cols: Vec<Vec<f64>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let f: Vec<f64> = (0..h).map(|y| g[y * w + x]).collect();
            let mut d = vec![0f64; h];
            let mut v = vec![0usize; h];
            let mut z = vec![0f64; h + 1];
            dt1(&f, &mut d, &mut v, &mut z);
            d
        })
        .collect();
    for (x, c) in cols.iter().enumerate() {
        for y in 0..h {
            g[y * w + x] = c[y];
        }
    }
    // rows
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let f = &g[y * w..(y + 1) * w];
        let mut d = vec![0f64; w];
        let mut v = vec![0usize; w];
        let mut z = vec![0f64; w + 1];
        dt1(f, &mut d, &mut v, &mut z);
        for x in 0..w {
            row[x] = d[x].min(INF).sqrt() as f32;
        }
    });
    out
}

/// For each cell, the nearest `known` cell (multi-source BFS, 8-connected).
pub fn nearest_known(known: &[bool], w: usize, h: usize) -> Vec<u32> {
    nearest_known_dist(known, w, h).0
}

/// Nearest known cell and the (chamfer) distance to it.
fn nearest_known_dist(known: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<f32>) {
    let n = w * h;
    let mut src = vec![NONE; n];
    let mut dist = vec![f32::MAX; n];
    let mut q = std::collections::VecDeque::new();
    for k in 0..n {
        if known[k] {
            src[k] = k as u32;
            dist[k] = 0.0;
            q.push_back(k);
        }
    }
    if q.is_empty() {
        return ((0..n as u32).collect(), vec![0.0; n]);
    }
    while let Some(k) = q.pop_front() {
        let (x, y) = ((k % w) as i32, (k / w) as i32);
        for (dx, dy) in NB8 {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let kk = ny as usize * w + nx as usize;
            let step = if dx != 0 && dy != 0 {
                std::f32::consts::SQRT_2
            } else {
                1.0
            };
            if dist[k] + step < dist[kk] - 1e-4 {
                dist[kk] = dist[k] + step;
                src[kk] = src[k];
                q.push_back(kk);
            }
        }
    }
    (src, dist)
}

fn fill_unknown_bool(v: &mut [bool], known: &[bool], w: usize, h: usize) {
    if known.iter().all(|k| *k) {
        return;
    }
    let src = nearest_known(known, w, h);
    for k in 0..w * h {
        if !known[k] {
            v[k] = v[src[k] as usize];
        }
    }
}

/// The region at cell `k`, or the nearest one within `r` cells.
fn region_near(reg: &[u32], w: usize, h: usize, k: usize, r: i32) -> Option<u32> {
    if reg[k] != NONE {
        return Some(reg[k]);
    }
    let (x, y) = ((k % w) as i32, (k / w) as i32);
    let mut best = (i32::MAX, NONE);
    for dy in -r..=r {
        for dx in -r..=r {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let v = reg[ny as usize * w + nx as usize];
            let d = dx * dx + dy * dy;
            if v != NONE && d < best.0 {
                best = (d, v);
            }
        }
    }
    (best.1 != NONE).then_some(best.1)
}

/// Majority label in a (2r+1)² window (land only).
fn mode_filter(lbl: &[u32], w: usize, h: usize, r: usize, k: usize) -> Vec<u32> {
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let c = lbl[i];
            if c == NONE {
                return NONE;
            }
            let (x, y) = (i % w, i / w);
            let mut counts = [0u16; 64];
            for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                    let v = lbl[yy * w + xx];
                    if v != NONE {
                        counts[v as usize] += 1;
                    }
                }
            }
            let mut best = c;
            for (j, n) in counts.iter().enumerate().take(k) {
                if *n > counts[best as usize] {
                    best = j as u32;
                }
            }
            best
        })
        .collect()
}

/// Rasterises a uv polygon onto the grid (cell centres inside).
fn raster_polygon(poly: &[[f32; 2]], w: usize, h: usize, mut f: impl FnMut(usize)) {
    if poly.len() < 3 {
        return;
    }
    let pts: Vec<[f32; 2]> = poly
        .iter()
        .map(|p| [p[0] * w as f32, p[1] * h as f32])
        .collect();
    let (mut y0, mut y1) = (f32::MAX, f32::MIN);
    for p in &pts {
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    let ys = (y0.floor().max(0.0) as usize).min(h);
    let ye = (y1.ceil().max(0.0) as usize).min(h);
    for y in ys..ye {
        let cy = y as f32 + 0.5;
        let mut xs = vec![];
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if (a[1] > cy) != (b[1] > cy) {
                xs.push(a[0] + (cy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                continue;
            }
            let xa = (pair[0] - 0.5).ceil().max(0.0) as usize;
            let xb = ((pair[1] - 0.5).floor().max(-1.0) + 1.0) as usize;
            for x in xa..xb.min(w) {
                f(y * w + x);
            }
        }
    }
}

// ------------------------------------------------------------------ skeletons

/// Zhang–Suen thinning of `mask` restricted to `cells`.
fn thin_mask(mask: &mut [bool], w: usize, h: usize, cells: &[usize]) {
    let at = |m: &[bool], x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < w as i32 && y < h as i32 && m[y as usize * w + x as usize]
    };
    let mut active: Vec<usize> = cells.to_vec();
    loop {
        let mut changed = false;
        for pass in 0..2 {
            let mut del = vec![];
            for &k in &active {
                if !mask[k] {
                    continue;
                }
                let (x, y) = ((k % w) as i32, (k / w) as i32);
                let p = [
                    at(mask, x, y - 1),
                    at(mask, x + 1, y - 1),
                    at(mask, x + 1, y),
                    at(mask, x + 1, y + 1),
                    at(mask, x, y + 1),
                    at(mask, x - 1, y + 1),
                    at(mask, x - 1, y),
                    at(mask, x - 1, y - 1),
                ];
                let b = p.iter().filter(|v| **v).count();
                if !(2..=6).contains(&b) {
                    continue;
                }
                let a = (0..8).filter(|i| !p[*i] && p[(i + 1) % 8]).count();
                if a != 1 {
                    continue;
                }
                let (p2, p4, p6, p8) = (p[0], p[2], p[4], p[6]);
                let ok = if pass == 0 {
                    !(p2 && p4 && p6) && !(p4 && p6 && p8)
                } else {
                    !(p2 && p4 && p8) && !(p2 && p6 && p8)
                };
                if ok {
                    del.push(k);
                }
            }
            if !del.is_empty() {
                changed = true;
                for k in del {
                    mask[k] = false;
                }
            }
        }
        if !changed {
            break;
        }
        active.retain(|k| mask[*k]);
    }
}

/// Repeatedly takes the longest path through the skeleton cells (geodesic double sweep) while it
/// is at least `min_len` cells long; removes it (and the cells next to it) each time.
fn longest_paths(
    on: &mut [bool],
    cells: &[usize],
    w: usize,
    h: usize,
    min_len: f32,
) -> Vec<Vec<usize>> {
    let mut out = vec![];
    for _ in 0..6 {
        let Some(&start) = cells.iter().find(|k| on[**k]) else {
            break;
        };
        // the farthest from any cell of the largest remaining piece is found by two sweeps from
        // each piece; pieces are handled in turn
        let mut best: Option<(f32, Vec<usize>)> = None;
        let mut visited: std::collections::HashSet<usize> = Default::default();
        for &s in cells {
            if !on[s] || visited.contains(&s) {
                continue;
            }
            let (far, _, _, seen) = sweep(on, s, w, h);
            visited.extend(seen.iter().copied());
            let (end, len, parent, _) = sweep(on, far, w, h);
            if best.as_ref().is_none_or(|b| len > b.0) {
                let mut path = vec![end];
                let mut c = end;
                while let Some(&p) = parent.get(&c) {
                    path.push(p);
                    c = p;
                }
                best = Some((len, path));
            }
        }
        let _ = start;
        let Some((len, path)) = best else { break };
        if len < min_len {
            break;
        }
        for &k in &path {
            on[k] = false;
            let (x, y) = ((k % w) as i32, (k / w) as i32);
            for (dx, dy) in NB8 {
                let (nx, ny) = (x + dx, y + dy);
                if nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32 {
                    on[ny as usize * w + nx as usize] = false;
                }
            }
        }
        out.push(path);
    }
    out
}

/// Dijkstra over 8-connected `on` cells from `s`: (farthest cell, its distance, parents, seen).
fn sweep(
    on: &[bool],
    s: usize,
    w: usize,
    h: usize,
) -> (
    usize,
    f32,
    std::collections::HashMap<usize, usize>,
    Vec<usize>,
) {
    use std::cmp::Ordering;
    #[derive(PartialEq)]
    struct Item(f32, usize);
    impl Eq for Item {}
    impl PartialOrd for Item {
        fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Item {
        fn cmp(&self, o: &Self) -> Ordering {
            o.0.total_cmp(&self.0)
        }
    }
    let mut dist: std::collections::HashMap<usize, f32> = Default::default();
    let mut parent = std::collections::HashMap::new();
    let mut heap = std::collections::BinaryHeap::new();
    dist.insert(s, 0.0);
    heap.push(Item(0.0, s));
    let mut far = (s, 0.0f32);
    while let Some(Item(d, k)) = heap.pop() {
        if d > *dist.get(&k).unwrap_or(&f32::MAX) {
            continue;
        }
        if d > far.1 {
            far = (k, d);
        }
        let (x, y) = ((k % w) as i32, (k / w) as i32);
        for (dx, dy) in NB8 {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let kk = ny as usize * w + nx as usize;
            if !on[kk] {
                continue;
            }
            let nd = d + if dx != 0 && dy != 0 {
                std::f32::consts::SQRT_2
            } else {
                1.0
            };
            if nd < *dist.get(&kk).unwrap_or(&f32::MAX) {
                dist.insert(kk, nd);
                parent.insert(kk, k);
                heap.push(Item(nd, kk));
            }
        }
    }
    let seen = dist.keys().copied().collect();
    (far.0, far.1, parent, seen)
}

// ------------------------------------------------------------------ outlines

/// Closed boundary loops (cell-corner coordinates, x right / y down) around the cells where
/// `inside` holds; collinear corners dropped.
pub fn trace_loops(w: usize, h: usize, inside: impl Fn(usize) -> bool) -> Vec<Vec<[f32; 2]>> {
    let vid = |x: usize, y: usize| y * (w + 1) + x;
    // directed boundary edges, clockwise on screen around inside cells
    let mut from: std::collections::HashMap<usize, Vec<usize>> = Default::default();
    let mut edges: Vec<(usize, usize)> = vec![];
    let ins = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && inside(y as usize * w + x as usize)
    };
    for y in 0..h {
        for x in 0..w {
            if !inside(y * w + x) {
                continue;
            }
            let (xi, yi) = (x as i64, y as i64);
            if !ins(xi, yi - 1) {
                edges.push((vid(x, y), vid(x + 1, y)));
            }
            if !ins(xi + 1, yi) {
                edges.push((vid(x + 1, y), vid(x + 1, y + 1)));
            }
            if !ins(xi, yi + 1) {
                edges.push((vid(x + 1, y + 1), vid(x, y + 1)));
            }
            if !ins(xi - 1, yi) {
                edges.push((vid(x, y + 1), vid(x, y)));
            }
        }
    }
    for (i, e) in edges.iter().enumerate() {
        from.entry(e.0).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops = vec![];
    for i in 0..edges.len() {
        if used[i] {
            continue;
        }
        let mut lp = vec![];
        let mut e = i;
        loop {
            used[e] = true;
            let v = edges[e].0;
            lp.push([(v % (w + 1)) as f32, (v / (w + 1)) as f32]);
            let next = edges[e].1;
            let Some(cands) = from.get(&next) else { break };
            let Some(&ne) = cands.iter().find(|c| !used[**c]) else {
                break;
            };
            e = ne;
        }
        // drop collinear corners
        let m = lp.len();
        if m < 3 {
            continue;
        }
        let pruned: Vec<[f32; 2]> = (0..m)
            .filter(|&j| {
                let (a, b, c) = (lp[(j + m - 1) % m], lp[j], lp[(j + 1) % m]);
                (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) != 0.0
            })
            .map(|j| lp[j])
            .collect();
        if pruned.len() >= 3 {
            loops.push(pruned);
        }
    }
    loops
}

/// Shoelace area (positive for clockwise on screen with y down).
pub fn polygon_area(p: &[[f32; 2]]) -> f32 {
    let mut s = 0.0;
    for i in 0..p.len() {
        let (a, b) = (p[i], p[(i + 1) % p.len()]);
        s += a[0] * b[1] - b[0] * a[1];
    }
    s * 0.5
}

/// Douglas–Peucker on an open polyline.
pub fn simplify_open(p: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (p[a], p[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let mut best = (0.0, a);
        for (i, q) in p.iter().enumerate().take(b).skip(a + 1) {
            let d = ((q[0] - pa[0]) * dy - (q[1] - pa[1]) * dx).abs() / len;
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tol {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    p.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(q, _)| *q)
        .collect()
}

/// Douglas–Peucker on a closed loop.
pub fn simplify_closed(p: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    if p.len() < 8 {
        return p.to_vec();
    }
    // split at the point farthest from the first
    let far = (1..p.len())
        .max_by(|a, b| {
            let d = |i: usize| (p[i][0] - p[0][0]).powi(2) + (p[i][1] - p[0][1]).powi(2);
            d(*a).total_cmp(&d(*b))
        })
        .unwrap_or(p.len() / 2);
    let mut a = simplify_open(&p[..=far], tol);
    let mut tail: Vec<[f32; 2]> = p[far..].to_vec();
    tail.push(p[0]);
    let b = simplify_open(&tail, tol);
    a.pop();
    a.extend_from_slice(&b[..b.len() - 1]);
    a
}
