//! Drawn lakes: an outline drawn on the ground becomes a clean closed spline (resampled, Chaikin
//! smoothed, resampled again), a basin is carved with the lake type's depth profile, the shore is
//! blended into the land (sand, mud, pebbles, grass) and shore plants grow (reeds, lily pads,
//! palms…). The water surface is a mesh of the outline itself whose last metre fades out, so the
//! shoreline is the smooth spline and not the 4 m terrain grid. Expand / Shrink / Smooth brushes
//! push the outline and re-carve; the ground under the lake is remembered so a lake can shrink
//! back without leaving a pit.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::Lake;
use crate::history::History;
use crate::noise::{Rng, smoothstep as ss};
use crate::objects::{Category, Library, ObjectStore, Placed};
use crate::terrain::layers::{LAYER_COUNT, Layer};
use crate::terrain::{GridRect, Terrain};

/// Where a shore plant grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// along the waterline, in clumps (density: clumps per 10 m of shore)
    Edge,
    /// floating on shallow water (per 100 m² of water 0.2–1.8 m deep near the shore)
    Water,
    /// on the bank (per 100 m² of the band 3–14 m from the water)
    Bank,
    /// on marsh islands (per 100 m² of island)
    Island,
}

/// A kind of lake for the drawing tool: depth profile, shore and plants, plus the water preset.
pub struct LakeShape {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// water preset id
    pub water: &'static str,
    /// deepest point (m)
    pub depth: f32,
    /// 0: steep sides and a flat bottom … 1: a gentle bowl
    pub bowl: f32,
    /// width of the bank that blends into the land (m)
    pub bank: f32,
    /// rise of the ground per metre right at the waterline (beach slope)
    pub beach: f32,
    /// raised rim around the bank (craters), m
    pub rim: f32,
    /// 0..1: low islands breaking the surface (marshes)
    pub islands: f32,
    pub bed: Layer,
    pub shore: Layer,
    /// a second, outer band (None: just the shore layer)
    pub outer: Option<Layer>,
    /// shore band width (m)
    pub shore_w: f32,
    /// tile colours: surrounding land (sRGB)
    pub land: [u8; 3],
    pub plants: &'static [(&'static str, Zone, f32)],
    /// shown in the picker
    pub listed: bool,
}

const fn shape(
    id: &'static str,
    name: &'static str,
    blurb: &'static str,
    water: &'static str,
    (depth, bowl, bank, beach, rim, islands): (f32, f32, f32, f32, f32, f32),
    (bed, shore, outer, shore_w): (Layer, Layer, Option<Layer>, f32),
    land: [u8; 3],
    plants: &'static [(&'static str, Zone, f32)],
) -> LakeShape {
    LakeShape {
        id,
        name,
        blurb,
        water,
        depth,
        bowl,
        bank,
        beach,
        rim,
        islands,
        bed,
        shore,
        outer,
        shore_w,
        land,
        plants,
        listed: true,
    }
}

use Zone::*;

pub const SHAPES: &[LakeShape] = &[
    shape(
        "pond",
        "Pond",
        "Small and shallow, muddy edge, reeds and lily pads.",
        "forest_lake",
        (1.8, 0.9, 7.0, 0.12, 0.0, 0.0),
        (Layer::Mud, Layer::Mud, Some(Layer::Grass), 3.0),
        [92, 128, 58],
        &[
            ("Reeds", Edge, 1.4),
            ("LilyPads", Water, 1.6),
            ("TallGrass", Bank, 2.0),
            ("Flowers", Bank, 0.5),
        ],
    ),
    shape(
        "lake",
        "Lake",
        "Deeper open water with a sandy beach.",
        "clear_lake",
        (7.0, 0.5, 14.0, 0.07, 0.0, 0.0),
        (Layer::Sand, Layer::Sand, Some(Layer::Gravel), 5.0),
        [88, 124, 60],
        &[
            ("Reeds", Edge, 0.45),
            ("TallGrass", Bank, 0.8),
            ("Boulder", Bank, 0.04),
        ],
    ),
    shape(
        "marsh",
        "Marsh",
        "Very shallow, islands and dense reed beds.",
        "marsh_water",
        (0.7, 1.0, 8.0, 0.05, 0.0, 0.45),
        (Layer::Mud, Layer::Mud, Some(Layer::Grass), 8.0),
        [96, 118, 62],
        &[
            ("Reeds", Edge, 3.0),
            ("Reeds", Island, 5.0),
            ("TallGrass", Bank, 3.0),
            ("LilyPads", Water, 0.8),
        ],
    ),
    shape(
        "tarn",
        "Mountain Tarn",
        "Small, deep and cold under rocky slopes.",
        "alpine_lake",
        (10.0, 0.25, 5.0, 0.45, 0.0, 0.0),
        (Layer::Gravel, Layer::Gravel, Some(Layer::Rock), 3.0),
        [118, 116, 108],
        &[
            ("Boulder", Bank, 0.5),
            ("Pebbles", Bank, 1.0),
            ("RockCluster", Bank, 0.15),
        ],
    ),
    shape(
        "oasis",
        "Oasis",
        "Sheltered pool ringed by sand and palms.",
        "tropical_lagoon",
        (2.5, 0.8, 12.0, 0.06, 0.0, 0.0),
        (Layer::Sand, Layer::Sand, Some(Layer::Grass), 6.0),
        [214, 190, 140],
        &[
            ("Palm", Bank, 0.35),
            ("Reeds", Edge, 0.7),
            ("TallGrass", Bank, 1.0),
            ("DesertShrub", Bank, 0.3),
        ],
    ),
    shape(
        "alpine_lake",
        "Alpine Lake",
        "Clear blue-green lake with a pebble shore.",
        "alpine_lake",
        (9.0, 0.4, 10.0, 0.2, 0.0, 0.0),
        (Layer::Gravel, Layer::Gravel, Some(Layer::Grass), 4.0),
        [100, 128, 76],
        &[
            ("Pebbles", Bank, 0.8),
            ("Boulder", Bank, 0.12),
            ("Reeds", Edge, 0.2),
            ("Flowers", Bank, 0.5),
        ],
    ),
    shape(
        "mirror_lake",
        "Mirror Lake",
        "Still water with grassy banks: a perfect mirror.",
        "mirror_lake",
        (5.0, 0.6, 10.0, 0.15, 0.0, 0.0),
        (Layer::Mud, Layer::Grass, None, 3.0),
        [84, 122, 56],
        &[
            ("Reeds", Edge, 0.6),
            ("TallGrass", Bank, 1.0),
            ("Willow", Bank, 0.03),
        ],
    ),
    shape(
        "forest_lake",
        "Forest Lake",
        "Dark water with a soft forest-floor shore.",
        "forest_lake",
        (4.0, 0.6, 10.0, 0.12, 0.0, 0.0),
        (Layer::Mud, Layer::ForestFloor, None, 4.0),
        [70, 96, 50],
        &[
            ("Reeds", Edge, 1.0),
            ("LilyPads", Water, 0.6),
            ("Fern", Bank, 1.5),
            ("FallenLog", Bank, 0.04),
        ],
    ),
    shape(
        "tropical_lagoon",
        "Tropical Lagoon",
        "Turquoise shallows over a wide white beach.",
        "tropical_lagoon",
        (3.0, 0.85, 14.0, 0.05, 0.0, 0.0),
        (Layer::Sand, Layer::Sand, None, 8.0),
        [226, 210, 168],
        &[("Palm", Bank, 0.25), ("Bush", Bank, 0.3)],
    ),
    shape(
        "volcanic_lake",
        "Volcanic Lake",
        "Acid water on black rock, inside a low rim.",
        "volcanic_lake",
        (6.0, 0.4, 7.0, 0.3, 1.5, 0.0),
        (Layer::DarkRock, Layer::DarkRock, Some(Layer::Gravel), 4.0),
        [70, 64, 60],
        &[("RockCluster", Bank, 0.3), ("RockSpire", Bank, 0.04)],
    ),
    shape(
        "crater_lake",
        "Crater Lake",
        "Deep blue water inside a raised crater rim.",
        "crater_lake",
        (14.0, 0.3, 10.0, 0.5, 5.0, 0.0),
        (Layer::Rock, Layer::Rock, Some(Layer::Gravel), 4.0),
        [112, 108, 100],
        &[("Boulder", Bank, 0.3), ("Pebbles", Bank, 0.6)],
    ),
    shape(
        "swamp_pond",
        "Swamp Pond",
        "Murky pond choked with lily pads and reeds.",
        "swamp_pond",
        (1.2, 0.9, 7.0, 0.06, 0.0, 0.15),
        (Layer::Mud, Layer::Mud, None, 6.0),
        [74, 84, 46],
        &[
            ("Reeds", Edge, 2.5),
            ("LilyPads", Water, 3.0),
            ("Reeds", Island, 3.0),
            ("Mushrooms", Bank, 0.6),
            ("DeadTree", Bank, 0.04),
            ("Willow", Bank, 0.03),
        ],
    ),
    shape(
        "koi_pond",
        "Koi Pond",
        "Shallow garden pond, pebble bottom, stone edge.",
        "koi_pond",
        (1.2, 0.3, 3.0, 0.3, 0.0, 0.0),
        (Layer::Gravel, Layer::Cobblestone, Some(Layer::Grass), 2.5),
        [96, 132, 66],
        &[
            ("LilyPads", Water, 1.5),
            ("Reeds", Edge, 0.3),
            ("Flowers", Bank, 1.5),
            ("MossyRock", Edge, 0.3),
        ],
    ),
    shape(
        "frozen_lake",
        "Frozen Lake",
        "Iced over, between snowy banks.",
        "frozen_lake",
        (4.0, 0.6, 10.0, 0.1, 0.0, 0.0),
        (Layer::Ice, Layer::Snow, None, 5.0),
        [226, 232, 238],
        &[("Boulder", Bank, 0.1), ("SnowyPine", Bank, 0.03)],
    ),
    shape(
        "salt_lake",
        "Salt Lake",
        "Very shallow brine on a flat white shore.",
        "salt_lake",
        (0.8, 1.0, 16.0, 0.03, 0.0, 0.0),
        (Layer::Sand, Layer::Sand, Some(Layer::DryGrass), 10.0),
        [218, 210, 190],
        &[("DesertShrub", Bank, 0.1)],
    ),
    LakeShape {
        listed: false,
        ..shape(
            "plunge_pool",
            "Plunge Pool",
            "The pool at a waterfall's foot.",
            "waterfall",
            (2.5, 0.7, 4.0, 0.25, 0.0, 0.0),
            (Layer::Gravel, Layer::Gravel, None, 2.5),
            [100, 110, 90],
            &[("Pebbles", Bank, 0.6), ("Boulder", Bank, 0.1)],
        )
    },
];

pub fn shape_by_id(id: &str) -> &'static LakeShape {
    SHAPES.iter().find(|s| s.id == id).unwrap_or(&SHAPES[0])
}

/// The ground under a drawn lake before it was carved (millimetres, a rect of grid vertices).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GroundPatch {
    pub x0: usize,
    pub z0: usize,
    pub w: usize,
    pub h: usize,
    pub mm: Vec<i32>,
}

impl GroundPatch {
    fn get(&self, i: usize, j: usize) -> Option<f32> {
        if i < self.x0 || j < self.z0 || i >= self.x0 + self.w || j >= self.z0 + self.h {
            return None;
        }
        self.mm
            .get((j - self.z0) * self.w + (i - self.x0))
            .map(|v| *v as f32 * 0.001)
    }
    fn valid(&self, t: &Terrain) -> bool {
        self.x0 + self.w <= t.n() && self.z0 + self.h <= t.n() && self.mm.len() == self.w * self.h
    }
}

// ---------------------------------------------------------------- outline geometry

pub fn signed_area(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].perp_dot(p[(i + 1) % n])).sum::<f32>() * 0.5
}

pub fn perimeter(p: &[Vec2]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].distance(p[(i + 1) % n])).sum()
}

pub fn centroid(p: &[Vec2]) -> Vec2 {
    let n = p.len();
    let a = signed_area(p);
    if a.abs() < 1e-3 {
        return p.iter().copied().sum::<Vec2>() / n.max(1) as f32;
    }
    let mut c = Vec2::ZERO;
    for i in 0..n {
        let (u, v) = (p[i], p[(i + 1) % n]);
        c += (u + v) * u.perp_dot(v);
    }
    c / (6.0 * a)
}

/// Resample a closed polyline at even spacing.
pub fn resample_closed(p: &[Vec2], spacing: f32) -> Vec<Vec2> {
    let n = p.len();
    if n < 3 {
        return p.to_vec();
    }
    let total = perimeter(p);
    let count = ((total / spacing).round() as usize).max(8);
    let step = total / count as f32;
    let mut out = Vec::with_capacity(count);
    let mut seg = 0usize;
    let mut seg_start = 0.0f32;
    let mut seg_len = p[0].distance(p[1 % n]);
    for k in 0..count {
        let d = k as f32 * step;
        while d > seg_start + seg_len && seg < n - 1 {
            seg_start += seg_len;
            seg += 1;
            seg_len = p[seg].distance(p[(seg + 1) % n]);
        }
        let t = ((d - seg_start) / seg_len.max(1e-5)).clamp(0.0, 1.0);
        out.push(p[seg].lerp(p[(seg + 1) % n], t));
    }
    out
}

fn chaikin_closed(p: &[Vec2], iterations: usize) -> Vec<Vec2> {
    let mut p = p.to_vec();
    for _ in 0..iterations {
        let n = p.len();
        let mut q = Vec::with_capacity(n * 2);
        for i in 0..n {
            let (a, b) = (p[i], p[(i + 1) % n]);
            q.push(a.lerp(b, 0.25));
            q.push(a.lerp(b, 0.75));
        }
        p = q;
    }
    p
}

/// Outline point spacing for a lake of this size.
pub fn spacing_for(p: &[Vec2]) -> f32 {
    (perimeter(p) / 360.0).max(1.6)
}

/// A hand-drawn loop → a clean closed counter-clockwise spline (no jaggies, no wobble from the
/// mouse): coarse resampling drops the jitter, Chaikin rounds the corners, a final resampling
/// spaces the points evenly. None if the loop is too small.
pub fn smooth_outline(raw: &[Vec2]) -> Option<Vec<Vec2>> {
    if raw.len() < 6 {
        return None;
    }
    let total = perimeter(raw);
    if total < 20.0 {
        return None;
    }
    // jitter-free base: ~40 points around, at least 3 m apart
    let coarse = resample_closed(raw, (total / 40.0).max(3.0));
    let mut p = chaikin_closed(&coarse, 3);
    // a little extra relaxation evens out the remaining kinks
    for _ in 0..2 {
        let n = p.len();
        p = (0..n)
            .map(|i| p[i] * 0.5 + (p[(i + n - 1) % n] + p[(i + 1) % n]) * 0.25)
            .collect();
    }
    if signed_area(&p) < 0.0 {
        p.reverse();
    }
    if signed_area(&p) < 40.0 {
        return None;
    }
    let s = spacing_for(&p);
    Some(resample_closed(&p, s))
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let u = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    p.distance(a + ab * u)
}

/// Is `p` inside the closed polygon (crossing number)?
pub fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x > p.x {
                c = !c;
            }
        }
    }
    c
}

/// Signed distance to the outline, positive inside.
pub fn sdf(poly: &[Vec2], p: Vec2) -> f32 {
    let n = poly.len();
    let d = (0..n)
        .map(|i| seg_dist(p, poly[i], poly[(i + 1) % n]))
        .fold(f32::MAX, f32::min);
    if inside(poly, p) { d } else { -d }
}

/// Signed distances on a regular grid (origin `o`, spacing `g`, nx × nz points), exact within
/// `band` of the outline; elsewhere ±`band` (sign from a scanline fill).
fn sdf_grid(poly: &[Vec2], o: Vec2, g: f32, nx: usize, nz: usize, band: f32) -> Vec<f32> {
    let n = poly.len();
    let mut d = vec![band; nx * nz];
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let lo = (a.min(b) - Vec2::splat(band) - o) / g;
        let hi = (a.max(b) + Vec2::splat(band) - o) / g;
        let (x0, z0) = (
            lo.x.floor().max(0.0) as usize,
            lo.y.floor().max(0.0) as usize,
        );
        let (x1, z1) = (
            (hi.x.ceil() as usize).min(nx - 1),
            (hi.y.ceil() as usize).min(nz - 1),
        );
        if lo.x > (nx - 1) as f32 || lo.y > (nz - 1) as f32 || hi.x < 0.0 || hi.y < 0.0 {
            continue;
        }
        for z in z0..=z1 {
            for x in x0..=x1 {
                let p = o + Vec2::new(x as f32, z as f32) * g;
                let k = z * nx + x;
                d[k] = d[k].min(seg_dist(p, a, b));
            }
        }
    }
    // signs, row by row
    let mut xs = Vec::new();
    for z in 0..nz {
        let y = o.y + z as f32 * g;
        xs.clear();
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if (a.y > y) != (b.y > y) {
                xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for x in 0..nx {
            let px = o.x + x as f32 * g;
            let crossings = xs.iter().filter(|v| **v > px).count();
            if crossings % 2 == 0 {
                d[z * nx + x] = -d[z * nx + x];
            }
        }
    }
    d
}

// ---------------------------------------------------------------- the water surface

/// The lake surface: a grid over the outline, each cell clipped exactly where the shore fade ends
/// (marching squares on the signed distance), so the mesh — which writes depth, and so hides any
/// spray or mist behind it — ends on the smooth curve rather than in grid steps; vertex colour
/// alpha is the shore fade (0 just outside the spline, 1 a couple of metres in).
pub fn lake_mesh(outline: &[Vec2], level: f32) -> Mesh {
    let (mut mn, mut mx) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in outline {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let size = mx - mn;
    let g = (size.max_element() / 170.0).max(1.25);
    let o = mn - Vec2::splat(g * 2.0);
    let nx = ((size.x + g * 4.0) / g).ceil() as usize + 1;
    let nz = ((size.y + g * 4.0) / g).ceil() as usize + 1;
    let band = g * 3.0;
    let d = sdf_grid(outline, o, g, nx, nz, band);
    // fade over the first ~2 m inside the spline (wider for big lakes, whose cells are coarser);
    // the surface ends where the fade reaches 0
    let fade = (g * 1.6).max(2.0);
    let f = |k: usize| d[k] + 0.25;
    let mut index = vec![u32::MAX; nx * nz];
    // where the shore cuts a grid edge (keyed by the edge's lower vertex, ×2 + 0 along x / 1 along z)
    let mut cuts = std::collections::HashMap::<usize, u32>::new();
    let mut pos = vec![];
    let mut col = vec![];
    let mut uv = vec![];
    let mut idx = vec![];
    let add = |p: Vec2,
               a: f32,
               pos: &mut Vec<[f32; 3]>,
               col: &mut Vec<[f32; 4]>,
               uv: &mut Vec<[f32; 2]>|
     -> u32 {
        pos.push([p.x, level, p.y]);
        col.push([0.5, 0.5, 0.0, a]);
        uv.push([p.x * 0.05, p.y * 0.05]);
        pos.len() as u32 - 1
    };
    let at = |k: usize| o + Vec2::new((k % nx) as f32, (k / nx) as f32) * g;
    for z in 0..nz - 1 {
        for x in 0..nx - 1 {
            // the cell's corners counter-clockwise (seen from above: +x, then +z)
            let ks = [
                z * nx + x,
                z * nx + x + 1,
                (z + 1) * nx + x + 1,
                (z + 1) * nx + x,
            ];
            if ks.iter().all(|k| f(*k) < 0.0) {
                continue;
            }
            let mut poly: Vec<u32> = Vec::with_capacity(6);
            for e in 0..4 {
                let (ka, kb) = (ks[e], ks[(e + 1) % 4]);
                if f(ka) >= 0.0 {
                    if index[ka] == u32::MAX {
                        index[ka] = add(
                            at(ka),
                            (f(ka) / fade).clamp(0.0, 1.0),
                            &mut pos,
                            &mut col,
                            &mut uv,
                        );
                    }
                    poly.push(index[ka]);
                }
                if (f(ka) >= 0.0) != (f(kb) >= 0.0) {
                    let key = ka.min(kb) * 2 + usize::from(ka.abs_diff(kb) != 1);
                    let i = *cuts.entry(key).or_insert_with(|| {
                        let u = f(ka) / (f(ka) - f(kb));
                        add(at(ka).lerp(at(kb), u), 0.0, &mut pos, &mut col, &mut uv)
                    });
                    poly.push(i);
                }
            }
            // a fan over the clipped (convex enough) cell; wound to face up like the full cells
            for i in 1..poly.len().saturating_sub(1) {
                idx.extend_from_slice(&[poly[0], poly[i + 1], poly[i]]);
            }
        }
    }
    let nor = vec![[0.0, 1.0, 0.0]; pos.len()];
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Stamp a drawn lake into the level map (grid vertices inside the outline).
pub fn stamp_level(t: &Terrain, outline: &[Vec2], level: f32, map: &mut [f32]) {
    let n = t.n();
    let (mut mn, mut mx) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in outline {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let Some(r) = t.rect_for_circle((mn + mx) * 0.5, (mx - mn).max_element() * 0.5 + t.cell) else {
        return;
    };
    let mut xs = Vec::new();
    for j in r.z0..=r.z1 {
        let y = t.grid_to_world(0, j).y;
        xs.clear();
        let m = outline.len();
        for i in 0..m {
            let (a, b) = (outline[i], outline[(i + 1) % m]);
            if (a.y > y) != (b.y > y) {
                xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        for pair in xs.chunks(2) {
            if pair.len() < 2 {
                continue;
            }
            let g0 = t.world_to_grid(pair[0], y).x.ceil().max(0.0) as usize;
            let g1 = (t.world_to_grid(pair[1], y).x.floor().max(-1.0)) as isize;
            if g1 < 0 {
                continue;
            }
            for i in g0..=(g1 as usize).min(n - 1) {
                let k = j * n + i;
                map[k] = map[k].max(level);
            }
        }
    }
}

// ---------------------------------------------------------------- carving

fn outline_of(l: &Lake) -> Vec<Vec2> {
    l.outline.iter().map(|p| Vec2::from_array(*p)).collect()
}

fn bounds(p: &[Vec2]) -> (Vec2, Vec2) {
    let (mut mn, mut mx) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for q in p {
        mn = mn.min(*q);
        mx = mx.max(*q);
    }
    (mn, mx)
}

/// How far the carve reaches outside the outline (bank + rim + paint, and a contained lake's lip).
fn reach(t: &Terrain, lake: &Lake) -> f32 {
    let s = shape_by_id(&lake.shape);
    let r = (s.bank + s.rim * 2.0).max(s.shore_w * 2.2) + 6.0;
    if lake.contained {
        let (_, hold, face) = lip(t);
        r.max(hold + face + t.cell)
    } else {
        r
    }
}

/// A contained lake's shore, outward from the outline (m): a shelf just under the surface, wide
/// enough that every terrain cell the outline crosses lies below the water (so the visible
/// waterline is the outline's smooth fade, never the terrain grid's facets); then the lip, a band
/// at least one and a half cells wide standing above the water (so the grid can't let the water
/// through); then its outer face down to the land.
fn lip(t: &Terrain) -> (f32, f32, f32) {
    let shelf = t.cell * 0.8;
    (
        shelf,
        shelf + (t.cell * 1.8).max(3.0),
        (t.cell * 2.0).max(8.0),
    )
}

fn rect_for(t: &Terrain, outline: &[Vec2], pad: f32) -> Option<GridRect> {
    let (mn, mx) = bounds(outline);
    let g0 = t.world_to_grid(mn.x - pad, mn.y - pad);
    let g1 = t.world_to_grid(mx.x + pad, mx.y + pad);
    let max = t.res as f32;
    if g1.x < 0.0 || g1.y < 0.0 || g0.x > max || g0.y > max {
        return None;
    }
    Some(GridRect {
        x0: g0.x.floor().clamp(0.0, max) as usize,
        z0: g0.y.floor().clamp(0.0, max) as usize,
        x1: g1.x.ceil().clamp(0.0, max) as usize,
        z1: g1.y.ceil().clamp(0.0, max) as usize,
    })
}

/// Ground heights of a lake over `rect` given the uncarved ground `base` (rect-sized): the basin
/// inside, the bank blending back into the land outside. Also returns the signed distances.
fn carve_heights(t: &Terrain, rect: GridRect, base: &[f32], lake: &Lake) -> (Vec<f32>, Vec<f32>) {
    let s = shape_by_id(&lake.shape);
    let poly = outline_of(lake);
    let (w, h) = (rect.width(), rect.height());
    let seed = lake.id.wrapping_mul(7919);
    let mut sd = vec![0.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            let p = t.grid_to_world(rect.x0 + i, rect.z0 + j);
            sd[j * w + i] = sdf(&poly, p);
        }
    }
    let r_in = sd.iter().copied().fold(0.0f32, f32::max).max(1.0);
    let depth = if lake.depth > 0.05 {
        lake.depth
    } else {
        s.depth
    };
    let level = lake.level;
    let rn = (r_in * (0.35 + 0.65 * s.bowl)).max(2.0);
    let (shelf, hold, face) = lip(t);
    let mut out = vec![0.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            let k = j * w + i;
            let p = t.grid_to_world(rect.x0 + i, rect.z0 + j);
            let d = sd[k];
            let b0 = base[k];
            out[k] = if d > 0.0 {
                let sn = (d / rn).clamp(0.0, 1.0);
                let steep = 1.0 - (1.0 - sn).powi(3);
                let bowl = sn * sn * (3.0 - 2.0 * sn);
                let prof = steep + (bowl - steep) * s.bowl;
                let nz = crate::noise::fbm(p.x / 41.0, p.y / 41.0, 3, 2.0, 0.5, seed);
                let mut dd = (depth * prof * (1.0 + 0.25 * nz * sn)).max((d * 0.12).min(0.35));
                if lake.contained {
                    // (on from the shelf outside)
                    dd = dd.max((0.14 + d * 0.1).min(depth));
                }
                let target = level - dd;
                let mut v = b0.min(target);
                if lake.contained {
                    // land falling away under the lake is filled into the bowl
                    v = v.max(target - (0.3 * depth + 0.3) * ss(0.0, rn * 0.5, d));
                }
                if s.islands > 0.0 {
                    let n2 = crate::noise::fbm(p.x / 23.0, p.y / 23.0, 3, 2.0, 0.5, seed + 5) * 0.5
                        + 0.5;
                    let isl =
                        ss(0.62 - s.islands * 0.12, 0.70 - s.islands * 0.1, n2) * ss(3.0, 9.0, d);
                    let top = level + 0.25 + 0.45 * ss(0.68, 0.85, n2);
                    v += (top - v) * isl;
                }
                v
            } else if lake.contained && -d <= shelf {
                // the shelf: shoaling gently, never quite out of the water
                level - 0.02 - 0.12 * (1.0 + d / shelf)
            } else {
                // (a contained lake's bank starts beyond its shelf)
                let b = if lake.contained { -d - shelf } else { -d };
                let k_bank = ss(0.0, s.bank, b);
                let shore_h = level + 0.06 + b * s.beach;
                let mut v = shore_h + (b0 - shore_h) * k_bank;
                if s.rim > 0.0 {
                    let x = (b - s.bank * 0.45) / (s.bank * 0.35).max(1.0);
                    v += s.rim * (-x * x).exp();
                }
                if lake.contained {
                    // the lip: rounding up out of the shelf, a crest (rough with noise), then the
                    // outer face down to wherever the land fell away
                    let nz = crate::noise::fbm(p.x / 13.0, p.y / 13.0, 2, 2.0, 0.5, seed + 9);
                    let crest = level - 0.02 + (1.0 + 0.15 * t.cell) * (1.0 + 0.3 * nz);
                    let x = (b / (hold - shelf)).min(1.0);
                    let rim = level - 0.02
                        + (crest - level + 0.02) * (x * std::f32::consts::FRAC_PI_2).sin();
                    v = v.max(rim + (b0 - rim).min(0.0) * ss(hold - shelf, hold - shelf + face, b));
                }
                v
            };
        }
    }
    (out, sd)
}

/// Blend a splat vertex toward one layer (as the paint brush does).
fn paint_toward(s: &mut [u8; LAYER_COUNT], layer: usize, k: f32) {
    if k <= 0.003 {
        return;
    }
    for (l, v) in s.iter_mut().enumerate() {
        let target = if l == layer { 255.0 } else { 0.0 };
        let cur = *v as f32;
        *v = (cur + (target - cur) * k).round().clamp(0.0, 255.0) as u8;
    }
    let sum: u32 = s.iter().map(|v| *v as u32).sum();
    if sum > 255 {
        let scale = 255.0 / sum as f32;
        for v in s.iter_mut() {
            *v = (*v as f32 * scale).floor() as u8;
        }
    }
}

/// Carve (or re-carve) a drawn lake: basin, banks, shore paint and shore plants. `prev` is the
/// lake as it was before an edit (None for a new lake): ground changes made since the last carve
/// (sculpting near the shore) are kept, everything the old carve did is undone first.
pub fn carve(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    lake: &mut Lake,
    prev: Option<&Lake>,
) {
    let poly = outline_of(lake);
    if poly.len() < 3 {
        return;
    }
    let s = shape_by_id(&lake.shape);
    let pad = reach(t, lake);
    let Some(mut rect) = rect_for(t, &poly, pad) else {
        return;
    };
    if let Some(p) = prev
        && p.outline.len() >= 3
        && let Some(r) = rect_for(t, &outline_of(p), reach(t, p))
    {
        rect = rect.union(r);
    }
    if let Some(g) = &lake.ground
        && g.valid(t)
        && g.w > 0
        && g.h > 0
    {
        rect = rect.union(GridRect {
            x0: g.x0,
            z0: g.z0,
            x1: g.x0 + g.w - 1,
            z1: g.z0 + g.h - 1,
        });
    }
    let (w, hh) = (rect.width(), rect.height());
    // the uncarved ground: the remembered patch where we have it, else the current terrain
    let ground = lake.ground.take().filter(|g| g.valid(t));
    let mut base = vec![0.0f32; w * hh];
    for j in 0..hh {
        for i in 0..w {
            let (gi, gj) = (rect.x0 + i, rect.z0 + j);
            base[j * w + i] = ground
                .as_ref()
                .and_then(|g| g.get(gi, gj))
                .unwrap_or_else(|| t.h(gi, gj));
        }
    }
    // keep edits made since the previous carve: current − what that carve produced
    if let (Some(p), Some(_)) = (prev, ground.as_ref())
        && p.outline.len() >= 3
    {
        let (old, _) = carve_heights(t, rect, &base, p);
        for j in 0..hh {
            for i in 0..w {
                let k = j * w + i;
                let delta = t.h(rect.x0 + i, rect.z0 + j) - old[k];
                if delta.abs() > 0.02 {
                    base[k] += delta;
                }
            }
        }
    }
    lake.ground = Some(GroundPatch {
        x0: rect.x0,
        z0: rect.z0,
        w,
        h: hh,
        mm: base.iter().map(|v| (v * 1000.0).round() as i32).collect(),
    });
    let (new_h, sd) = carve_heights(t, rect, &base, lake);

    h.touch_terrain(t, rect);
    let seed = lake.id.wrapping_mul(131) + 17;
    let bed = s.bed as usize;
    let shore = s.shore as usize;
    for j in 0..hh {
        for i in 0..w {
            let k = j * w + i;
            let idx = t.idx(rect.x0 + i, rect.z0 + j);
            t.heights[idx] = new_h[k];
            let d = sd[k];
            let p = t.grid_to_world(rect.x0 + i, rect.z0 + j);
            // ragged band edges
            let e = crate::noise::fbm(p.x / 9.0, p.y / 9.0, 3, 2.0, 0.5, seed) * s.shore_w * 0.5;
            let sw = (s.shore_w + e).max(0.8);
            let sp = &mut t.splat[idx];
            if let Some(outer) = s.outer
                && d < 0.0
            {
                let x = -d;
                let k_out = ss(sw * 0.5, sw, x) * ss(sw * 2.4, sw * 1.4, x) * 0.7;
                paint_toward(sp, outer as usize, k_out);
            }
            if d <= 1.5 {
                paint_toward(sp, shore, 0.92 * ss(-sw, -sw * 0.35, d));
            }
            if d > -0.5 {
                paint_toward(sp, bed, 0.9 * ss(-0.5, 2.0 + e * 0.2, d));
            }
            if lake.contained && d < 0.0 {
                // a wall cut into the slope is bare ground, rock where it's deep; a raised lip is
                // bare only near the water (fill further out keeps the hillside's own ground)
                // (cut walls are beside the pool; a slight lowering far out is just smoothing)
                let cut = ((base[k] - new_h[k]).max(0.0) + e * 0.3) * ss(sw * 7.0, sw * 3.0, -d);
                let fill = (new_h[k] - base[k]).max(0.0);
                let near = ss(sw * 3.0, sw * 1.1, -d);
                paint_toward(
                    sp,
                    shore,
                    (0.85 * ss(0.6, 2.0, cut)).max(0.75 * near * ss(0.4, 1.5, fill)),
                );
                paint_toward(sp, Layer::Rock as usize, 0.8 * ss(2.0, 4.5, cut));
            }
        }
    }
    t.mark_heights_dirty(rect);
    t.mark_splat_dirty(rect);

    // plants: clear the lake's previous ones and anything now under water, then grow new ones
    for id in std::mem::take(&mut lake.plant_ids) {
        if store.items.contains_key(&id) {
            h.touch_object(store, id);
            store.remove(id);
        }
    }
    let (mn, mx) = bounds(&poly);
    let drowned: Vec<u64> = store
        .query((mn + mx) * 0.5, (mx - mn).length() * 0.5 + 2.0)
        .into_iter()
        .filter(|id| {
            let Some(o) = store.items.get(id) else {
                return false;
            };
            let structure = lib
                .and_then(|l| l.get(&o.asset))
                .is_some_and(|e| e.category == Category::Structures);
            !structure && sdf(&poly, Vec2::new(o.pos[0], o.pos[2])) > 0.8
        })
        .collect();
    for id in drowned {
        h.touch_object(store, id);
        store.remove(id);
    }
    if let Some(lib) = lib
        && lake.plants > 0.0
    {
        lake.plant_ids = grow_plants(t, h, store, lib, lake, &poly, s);
    }
}

/// Gives back the ground a drawn lake carved (keeping edits made on top since) and removes its
/// shore plants. False when there is nothing to give back (basin lakes carve nothing).
pub fn uncarve(t: &mut Terrain, h: &mut History, store: &mut ObjectStore, lake: &Lake) -> bool {
    for id in &lake.plant_ids {
        if store.items.contains_key(id) {
            h.touch_object(store, *id);
            store.remove(*id);
        }
    }
    let Some(g) = lake
        .ground
        .as_ref()
        .filter(|g| lake.is_drawn() && g.valid(t) && g.w > 0 && g.h > 0)
    else {
        return false;
    };
    let rect = GridRect {
        x0: g.x0,
        z0: g.z0,
        x1: g.x0 + g.w - 1,
        z1: g.z0 + g.h - 1,
    };
    let base: Vec<f32> = g.mm.iter().map(|v| *v as f32 * 0.001).collect();
    let (carved, _) = carve_heights(t, rect, &base, lake);
    h.touch_terrain(t, rect);
    for j in 0..g.h {
        for i in 0..g.w {
            let k = j * g.w + i;
            let idx = t.idx(g.x0 + i, g.z0 + j);
            let later = t.heights[idx] - carved[k];
            t.heights[idx] = if later.abs() > 0.02 {
                base[k] + later
            } else {
                base[k]
            };
        }
    }
    t.mark_heights_dirty(rect);
    true
}

fn grow_plants(
    t: &Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: &Library,
    lake: &Lake,
    poly: &[Vec2],
    s: &LakeShape,
) -> Vec<u64> {
    let mut rng = Rng::new(lake.id as u64 * 977 + 3);
    let mut ids = vec![];
    let amount = lake.plants;
    let (mn, mx) = bounds(poly);
    let area_box = (mx - mn + Vec2::splat(30.0)).element_product();
    let perim = perimeter(poly);
    let level = lake.level;
    let n = poly.len();
    let mut place = |store: &mut ObjectStore,
                     h: &mut History,
                     rng: &mut Rng,
                     asset: &str,
                     pos: Vec3,
                     align_n: Option<Vec3>| {
        let Some(e) = lib.get(asset) else { return };
        let scale = e.default_scale * rng.range(0.75, 1.25);
        let yaw = rng.f32() * std::f32::consts::TAU;
        let rot = match align_n {
            Some(nrm) if e.align => {
                Quat::from_rotation_arc(Vec3::Y, nrm) * Quat::from_rotation_y(yaw)
            }
            _ => Quat::from_rotation_y(yaw),
        };
        let id = store.new_id();
        h.touch_object(store, id);
        store.insert(Placed {
            group: 0,
            id,
            asset: asset.to_string(),
            variant: rng.below(e.meshes.len().max(1) as u32),
            pos: pos.to_array(),
            rot: rot.to_array(),
            scale,
            stretch: None,
            locked: false,
        });
        ids.push(id);
    };
    for &(asset, zone, density) in s.plants {
        let Some(entry) = lib.get(asset) else {
            continue;
        };
        let sink = if entry.category == Category::Trees {
            0.25
        } else {
            0.08
        };
        // (from the lowest ground under the trunk's footprint: no gap on the downhill side)
        let foot = if entry.category == Category::Trees {
            (entry.radius * 0.15).max(0.45) * entry.default_scale * 1.25
        } else {
            0.0
        };
        match zone {
            Zone::Edge => {
                // clumps along the waterline, gated by a slow noise so the shore has gaps
                let count = (perim / 10.0 * density * amount).round() as usize;
                for _ in 0..count {
                    let k = rng.below(n as u32) as usize;
                    let a = poly[k];
                    let b = poly[(k + 1) % n];
                    let along = a.lerp(b, rng.f32());
                    let tan = (b - a).normalize_or_zero();
                    let outward = Vec2::new(tan.y, -tan.x);
                    let gate = crate::noise::value2(along.x / 18.0, along.y / 18.0, lake.id + 11);
                    if gate < 0.35 {
                        continue;
                    }
                    let clump = 2 + rng.below(4);
                    for _ in 0..clump {
                        let q = along + outward * rng.range(-1.4, 1.6) + tan * rng.range(-1.8, 1.8);
                        if !t.in_bounds(q.x, q.y) {
                            continue;
                        }
                        let g = t.height_at(q.x, q.y);
                        let y = if asset == "LilyPads" {
                            level + 0.02
                        } else {
                            g.min(t.min_height_around(q.x, q.y, foot)) - sink
                        };
                        place(
                            store,
                            h,
                            &mut rng,
                            asset,
                            Vec3::new(q.x, y, q.y),
                            Some(t.normal_at(q.x, q.y)),
                        );
                    }
                }
            }
            Zone::Water | Zone::Bank | Zone::Island => {
                let tries = (area_box / 100.0 * density * amount * 1.6).round() as usize;
                for _ in 0..tries.min(6000) {
                    let q = Vec2::new(
                        rng.range(mn.x - 15.0, mx.x + 15.0),
                        rng.range(mn.y - 15.0, mx.y + 15.0),
                    );
                    if !t.in_bounds(q.x, q.y) {
                        continue;
                    }
                    let d = sdf(poly, q);
                    let g = t.height_at(q.x, q.y);
                    let ok = match zone {
                        Zone::Water => {
                            d > 1.2
                                && (0.2..1.8).contains(&(level - g))
                                && crate::noise::value2(q.x / 14.0, q.y / 14.0, lake.id + 23) > 0.4
                        }
                        Zone::Bank => {
                            (-14.0..-2.5).contains(&d)
                                && g > level + 0.2
                                && t.normal_at(q.x, q.y).y > 0.8
                        }
                        _ => d > 2.0 && g > level + 0.08,
                    };
                    if !ok {
                        continue;
                    }
                    // spacing between plants of the same kind
                    let r = entry.radius * 0.8;
                    let crowded = store.query(q, r + 2.0).into_iter().any(|id| {
                        store.items.get(&id).is_some_and(|o| {
                            o.asset == asset && Vec2::new(o.pos[0], o.pos[2]).distance(q) < r
                        })
                    });
                    if crowded {
                        continue;
                    }
                    let y = if zone == Zone::Water {
                        level + 0.02
                    } else {
                        t.min_height_around(q.x, q.y, foot) - sink
                    };
                    place(
                        store,
                        h,
                        &mut rng,
                        asset,
                        Vec3::new(q.x, y, q.y),
                        Some(t.normal_at(q.x, q.y)),
                    );
                }
            }
        }
    }
    ids
}

/// The lowest sensible water level for an outline: a low percentile of the ground along it (the
/// downhill side gets a natural bank), never below the sea.
pub fn level_for(t: &Terrain, outline: &[Vec2]) -> f32 {
    let mut hs: Vec<f32> = outline.iter().map(|p| t.height_at(p.x, p.y)).collect();
    hs.sort_by(|a, b| a.total_cmp(b));
    let l = hs[(hs.len() as f32 * 0.3) as usize];
    l.max(t.sea_level + 0.3)
}

/// A point well inside the outline (for selection and flood-free bookkeeping).
pub fn interior_point(outline: &[Vec2]) -> Vec2 {
    let c = centroid(outline);
    if inside(outline, c) {
        return c;
    }
    // concave shape: the deepest of a few candidates
    let (mn, mx) = bounds(outline);
    let mut best = (f32::MIN, c);
    for j in 0..12 {
        for i in 0..12 {
            let p = mn + (mx - mn) * Vec2::new((i as f32 + 0.5) / 12.0, (j as f32 + 0.5) / 12.0);
            let d = sdf(outline, p);
            if d > best.0 {
                best = (d, p);
            }
        }
    }
    best.1
}

// ---------------------------------------------------------------- shoreline brushes

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShapeMode {
    #[default]
    Expand,
    Shrink,
    Smooth,
}

/// Push the outline out (Expand), pull it in (Shrink) or relax it (Smooth) under a brush; the
/// result is resampled so it stays an even, smooth curve. `amount` is metres this frame.
pub fn deform(
    outline: &mut Vec<[f32; 2]>,
    center: Vec2,
    radius: f32,
    amount: f32,
    mode: ShapeMode,
) {
    let mut p: Vec<Vec2> = outline.iter().map(|q| Vec2::from_array(*q)).collect();
    let n = p.len();
    if n < 3 {
        return;
    }
    let weight = |q: Vec2| {
        let x = (q.distance(center) / radius).min(1.0);
        (1.0 - x * x).powi(2)
    };
    let before = p.clone();
    for i in 0..n {
        let w = weight(before[i]);
        if w <= 0.0 {
            continue;
        }
        let (a, b) = (before[(i + n - 1) % n], before[(i + 1) % n]);
        let tan = (b - a).normalize_or_zero();
        let outward = Vec2::new(tan.y, -tan.x);
        match mode {
            ShapeMode::Expand => p[i] += outward * amount * w,
            ShapeMode::Shrink => p[i] -= outward * amount * w,
            ShapeMode::Smooth => p[i] = p[i].lerp((a + b) * 0.5, (amount * 0.6 * w).min(0.9)),
        }
    }
    // even out how far neighbours moved so pushing never makes kinks (smoothing the movement,
    // not the positions: relaxing positions pulls a curved shore inward and undoes Expand)
    if mode != ShapeMode::Smooth {
        let moved: Vec<Vec2> = (0..n).map(|i| p[i] - before[i]).collect();
        for i in 0..n {
            let w = weight(before[i]);
            if w > 0.0 {
                let avg = (moved[(i + n - 1) % n] + moved[(i + 1) % n]) * 0.5;
                p[i] = before[i] + moved[i].lerp(avg, 0.5 * w);
            }
        }
    }
    if signed_area(&p) < 30.0 {
        return;
    }
    let s = spacing_for(&p);
    *outline = resample_closed(&p, s)
        .into_iter()
        .map(|q| q.to_array())
        .collect();
}

#[cfg(test)]
mod shore_brush_tests {
    use super::*;

    fn circle(r: f32, n: usize) -> Vec<[f32; 2]> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [r * a.cos(), r * a.sin()]
            })
            .collect()
    }

    fn area(o: &[[f32; 2]]) -> f32 {
        let p: Vec<Vec2> = o.iter().map(|q| Vec2::from_array(*q)).collect();
        signed_area(&p).abs()
    }

    /// One second of brushing at 60 fps over the shore actually moves it (the kink relax used to
    /// pull a round shore inward as fast as Expand pushed it out).
    #[test]
    fn expand_and_shrink_move_a_round_shore() {
        let start = circle(60.0, 48);
        let a0 = area(&start);
        let (mut grown, mut shrunk) = (start.clone(), start.clone());
        for _ in 0..60 {
            deform(&mut grown, Vec2::new(60.0, 0.0), 40.0, 0.6 * 14.0 / 60.0, ShapeMode::Expand);
            deform(&mut shrunk, Vec2::new(60.0, 0.0), 40.0, 0.6 * 14.0 / 60.0, ShapeMode::Shrink);
        }
        assert!(area(&grown) > a0 * 1.03, "expand: {a0} -> {}", area(&grown));
        assert!(area(&shrunk) < a0 * 0.97, "shrink: {a0} -> {}", area(&shrunk));
    }
}
