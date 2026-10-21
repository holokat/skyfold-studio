//! Shaping a realm's land to a blueprint: the ground between the zones rolling at the zones'
//! heights (higher and wooded away from the route), a level floor for every zone with steep
//! banks down (or cut walls up) round it, every link graded along its length with its stairs as
//! short steep ramps, the chasms cut to their floors, the water's beds dug, and cliffs all round
//! the map closing it. Then (after the biomes) the floors, rings, paths, stairs and banks painted
//! and the nature kept off the walkable ground.
//!
//! Everything here is pure grid work over the realm's heights, so it is testable without the
//! engine. The sheet's frame (x east, y north) maps to the world as (x, −y).

use bevy::math::Vec2;

use super::blueprint::{Blueprint, LinkKind, Role, Shape};
use crate::dungeons::compound::shape::{blur, each, geom_band, put};
use crate::dungeons::geom;
use crate::noise::{fbm, smoothstep};
use crate::terrain::layers::Layer;
use crate::terrain::{GridRect, Terrain};

/// Metres along per metre of rise on a stair (the sheet's "4 m over 12 m").
pub const STAIR_RUN: f32 = 3.0;
/// Bank slopes: a terrace's edge drops this steeply (m per m) and a cut wall rises this steeply.
const DROP: f32 = 2.2;
const CUT: f32 = 1.6;
/// The cliffs round the map: how far beyond its edge they reach their top.
const RIM_REACH: f32 = 22.0;

/// The sheet's frame → the world's ground plane (x, z).
pub fn w(p: [f32; 2]) -> Vec2 {
    Vec2::new(p[0], -p[1])
}

/// A zone in the world.
#[derive(Clone, Debug)]
pub struct ZoneW {
    pub num: u8,
    pub role: Role,
    pub c: Vec2,
    /// half its width and depth
    pub h: Vec2,
    /// its turn in the world (radians, counter-clockwise seen from above on x/z)
    pub ang: f32,
    pub round: bool,
    pub outline: Vec<Vec2>,
    /// its floor (world y)
    pub top: f32,
    pub ring: Option<(Vec2, f32)>,
}

impl ZoneW {
    /// Signed distance to its floor's edge: positive inside.
    pub fn inside(&self, p: Vec2) -> f32 {
        if !self.outline.is_empty() {
            return geom::poly_sdf(&self.outline, p);
        }
        let d = p - self.c;
        if self.round {
            return self.h.x.min(self.h.y) - d.length();
        }
        let l = geom::rot(d, -self.ang).abs() - self.h;
        let outside = l.max(Vec2::ZERO).length();
        let inside = l.x.max(l.y).min(0.0);
        -(outside + inside)
    }
    pub fn reach(&self) -> f32 {
        if self.outline.is_empty() {
            self.h.length()
        } else {
            self.outline
                .iter()
                .map(|q| q.distance(self.c))
                .fold(0.0, f32::max)
        }
    }
}

/// A link in the world: its centreline and the height along it.
#[derive(Clone, Debug)]
pub struct LinkW {
    pub pts: Vec<Vec2>,
    pub half_w: f32,
    pub kind: LinkKind,
    /// (from, to) along it in metres, and the height there: level runs and stair ramps
    pub profile: Vec<(f32, f32, f32, f32)>,
    /// its stairs (from, to along it), for the paint and the rails
    pub stairs: Vec<(f32, f32)>,
    pub len: f32,
}

/// A link's span is over an actual sheet hazard.  Bridges and paths get a raised deck there;
/// causeways remain ordinary grounded lanes.  Keeping this predicate here makes the land and
/// bridge compiler agree about where a lane must not fill or paint a water/chasm body.
pub fn over_hazard(site: &Site, l: &LinkW, s: f32) -> bool {
    if l.kind == LinkKind::Causeway {
        return false;
    }
    let p = geom::polyline_at(&l.pts, s).0;
    // A zone is a solid, intentionally walkable platform even if a broad sheet chasm or water
    // polygon overlaps it. Only the unsupported route between those platforms needs a deck.
    if site.zones.iter().any(|z| z.inside(p) >= 0.0) {
        return false;
    }
    site.chasms.iter().any(|(o, _)| geom::poly_sdf(o, p) >= 0.0)
        || site.water.iter().any(|(o, _)| geom::poly_sdf(o, p) >= 0.0)
        || site
            .water_mask
            .as_ref()
            .is_some_and(|m| site.sea.is_some() && m.at(p) > 0.5)
}

fn keeps_ground(site: &Site, p: Vec2) -> f32 {
    site.links
        .iter()
        .filter_map(|l| {
            let (d, s) = l.near(p);
            (!over_hazard(site, l, s)).then_some((d - l.half_w).max(0.0))
        })
        .fold(f32::MAX, f32::min)
}

impl LinkW {
    pub fn height(&self, s: f32) -> f32 {
        for &(a, b, ya, yb) in &self.profile {
            if s <= b {
                return if s <= a {
                    ya
                } else {
                    ya + (yb - ya) * ((s - a) / (b - a).max(0.01))
                };
            }
        }
        self.profile.last().map_or(0.0, |p| p.3)
    }
    /// Distance to the centreline and how far along it the nearest point is.
    pub fn near(&self, p: Vec2) -> (f32, f32) {
        geom::polyline_dist(&self.pts, p)
    }
    pub fn on_stairs(&self, s: f32) -> bool {
        self.stairs
            .iter()
            .any(|&(a, b)| s >= a - 0.5 && s <= b + 0.5)
    }
}

/// The blueprint laid on a realm: in the world, at its floor height.
pub struct Site {
    pub zones: Vec<ZoneW>,
    pub links: Vec<LinkW>,
    /// (outline, floor) in the world
    pub chasms: Vec<(Vec<Vec2>, f32)>,
    /// (outline, surface level) in the world
    pub water: Vec<(Vec<Vec2>, f32)>,
    /// half the map's width and depth
    pub half: Vec2,
    /// the world height of the Arrival's floor (elevation 0)
    pub y0: f32,
    pub seed: u32,
    /// the realm's sea level when the sheet's water is a sea round islands (it is the realm's
    /// own ocean, and the map's edge is open water instead of cliffs)
    pub sea: Option<f32>,
    /// where the sheet's picture shows water (a sea's shape is read from it, not an outline)
    pub water_mask: Option<WaterMask>,
}

/// Where a sheet's picture shows water, on a grid over the map: 0 dry … 1 water, smoothed (so
/// labels, boats and markers on the water don't make islands).
pub struct WaterMask {
    /// the map's south-west corner (sheet metres) and the cell size
    pub lo: Vec2,
    pub cell: f32,
    pub nx: usize,
    pub ny: usize,
    pub v: Vec<f32>,
}

impl WaterMask {
    /// From the picture: every cell's pixel judged water (blue to turquoise, saturated) or not.
    pub fn from_picture(
        img: &image::RgbaImage,
        pf: &super::blueprint::PictureFrame,
        size: [f32; 2],
    ) -> Option<WaterMask> {
        if pf.m_per_px <= 0.0 {
            return None;
        }
        let cell = 2.0;
        let (nx, ny) = (
            (size[0] / cell).ceil() as usize + 1,
            (size[1] / cell).ceil() as usize + 1,
        );
        let lo = Vec2::new(-size[0] * 0.5, -size[1] * 0.5);
        let (w, h) = (img.width() as i32, img.height() as i32);
        let water = |px: i32, py: i32| -> f32 {
            if px < 0 || py < 0 || px >= w || py >= h {
                return 0.0;
            }
            let c = img.get_pixel(px as u32, py as u32).0;
            let (r, g, b) = (
                c[0] as f32 / 255.0,
                c[1] as f32 / 255.0,
                c[2] as f32 / 255.0,
            );
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
            if max < 0.18 || sat < 0.25 || max - min < 0.08 {
                return 0.0;
            }
            // hue in degrees
            let hue = if max == r {
                60.0 * (((g - b) / (max - min)) % 6.0)
            } else if max == g {
                60.0 * ((b - r) / (max - min) + 2.0)
            } else {
                60.0 * ((r - g) / (max - min) + 4.0)
            };
            let hue = if hue < 0.0 { hue + 360.0 } else { hue };
            if (165.0..=235.0).contains(&hue) {
                1.0
            } else {
                0.0
            }
        };
        let mut v = vec![0.0f32; nx * ny];
        for j in 0..ny {
            for i in 0..nx {
                let p = lo + Vec2::new(i as f32, j as f32) * cell;
                let px = pf.centre_px[0] + p.x / pf.m_per_px;
                let py = pf.centre_px[1] - p.y / pf.m_per_px;
                let (px, py) = (px.round() as i32, py.round() as i32);
                let mut s = 0.0;
                for (dx, dy) in [(0, 0), (-2, 0), (2, 0), (0, -2), (0, 2)] {
                    s += water(px + dx, py + dy);
                }
                v[j * nx + i] = s / 5.0;
            }
        }
        // (smoothed twice over ~6 m: labels and boats on the water don't stay islands)
        for _ in 0..2 {
            let src = v.clone();
            for j in 0..ny {
                for i in 0..nx {
                    let mut s = 0.0;
                    let mut n = 0.0;
                    for dj in -3i32..=3 {
                        for di in -3i32..=3 {
                            let (a, b) = (i as i32 + di, j as i32 + dj);
                            if a >= 0 && b >= 0 && (a as usize) < nx && (b as usize) < ny {
                                s += src[b as usize * nx + a as usize];
                                n += 1.0;
                            }
                        }
                    }
                    v[j * nx + i] = s / n;
                }
            }
        }
        let share = v.iter().filter(|x| **x > 0.5).count() as f32 / v.len() as f32;
        (share > 0.03).then_some(WaterMask {
            lo,
            cell,
            nx,
            ny,
            v,
        })
    }
    /// How much water at a world point (0..1; 0 off the map).
    pub fn at(&self, world: Vec2) -> f32 {
        let p = Vec2::new(world.x, -world.y) - self.lo;
        let (fx, fy) = (p.x / self.cell, p.y / self.cell);
        if fx < 0.0 || fy < 0.0 || fx > (self.nx - 1) as f32 || fy > (self.ny - 1) as f32 {
            return 0.0;
        }
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (i1, j1) = ((i + 1).min(self.nx - 1), (j + 1).min(self.ny - 1));
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let a = self.v[j * self.nx + i] + (self.v[j * self.nx + i1] - self.v[j * self.nx + i]) * tx;
        let b =
            self.v[j1 * self.nx + i] + (self.v[j1 * self.nx + i1] - self.v[j1 * self.nx + i]) * tx;
        a + (b - a) * ty
    }
}

/// Whether a water body is a sea: it covers much of the map or runs along its edge (island maps),
/// or is named one.
pub fn is_sea(bp: &Blueprint, wt: &super::blueprint::Water) -> bool {
    if wt.outline.len() < 3 {
        return false;
    }
    let n = wt.name.to_ascii_lowercase();
    // (rivers, lava and pools are never the sea, even where they run off the map's edge)
    if [
        "river", "stream", "creek", "brook", "canal", "lava", "magma", "fissure", "falls", "pool",
        "pond", "spring", "moat", "well", "cistern", "ice", "frozen", "floe",
    ]
    .iter()
    .any(|k| n.contains(k))
    {
        return false;
    }
    if [
        "sea",
        "ocean",
        "deep water",
        "open water",
        "bay",
        "lagoon",
        "reef",
    ]
    .iter()
    .any(|k| n.contains(k))
    {
        return true;
    }
    let pts: Vec<Vec2> = wt.outline.iter().map(|p| Vec2::new(p[0], p[1])).collect();
    let area = geom::signed_area(&pts).abs();
    let [hx, hy] = bp.half();
    let near_edge = pts
        .iter()
        .filter(|p| p.x.abs() > hx - 12.0 || p.y.abs() > hy - 12.0)
        .count();
    area > 0.3 * bp.size[0] * bp.size[1] && near_edge * 4 >= pts.len()
}

impl Site {
    pub fn new(bp: &Blueprint, y0: f32, seed: u32) -> Site {
        let zones: Vec<ZoneW> = bp
            .zones
            .iter()
            .map(|z| ZoneW {
                num: z.num,
                role: z.role,
                c: w(z.c),
                h: Vec2::new(z.size[0], z.size[1]) * 0.5,
                // (counter-clockwise on the sheet is clockwise on x/z seen from above)
                ang: -z.turn.to_radians(),
                round: z.shape == Shape::Round,
                outline: if z.shape == Shape::Poly && z.outline.len() >= 3 {
                    z.outline.iter().map(|p| w(*p)).collect()
                } else {
                    vec![]
                },
                top: y0 + z.elevation,
                ring: z.boss.as_ref().map(|r| (w(r.c), r.d * 0.5)),
            })
            .collect();
        let links = bp.links.iter().filter_map(|l| link_w(&zones, l)).collect();
        Site {
            zones,
            links,
            chasms: bp
                .chasms
                .iter()
                .filter(|c| c.outline.len() >= 3)
                .map(|c| (c.outline.iter().map(|p| w(*p)).collect(), y0 + c.floor))
                .collect(),
            water: bp
                .water
                .iter()
                .filter(|c| c.outline.len() >= 3)
                .map(|c| (c.outline.iter().map(|p| w(*p)).collect(), y0 + c.level))
                .collect(),
            half: Vec2::new(bp.size[0], bp.size[1]) * 0.5,
            y0,
            seed,
            sea: bp
                .water
                .iter()
                .filter(|wt| is_sea(bp, wt))
                .map(|wt| y0 + wt.level)
                .reduce(f32::min),
            water_mask: None,
        }
    }
    pub fn zone(&self, num: u8) -> Option<&ZoneW> {
        self.zones.iter().find(|z| z.num == num)
    }
    /// The highest floor (world y).
    pub fn top(&self) -> f32 {
        self.zones.iter().map(|z| z.top).fold(self.y0, f32::max)
    }
    /// Signed distance outside the map's rectangle (negative inside).
    pub fn out(&self, p: Vec2) -> f32 {
        let q = p.abs() - self.half;
        q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0)
    }
    /// Signed distance outside the map's edge as the land draws it: the rectangle's sides
    /// wandering in and out (tens of metres) so the cliffs round it read as natural.
    pub fn edge(&self, p: Vec2) -> f32 {
        let out = self.out(p);
        if out < -60.0 {
            return out;
        }
        let s = (self.seed & 0xffff) + 11;
        let wob = 42.0 * fbm(p.x / 90.0, p.y / 90.0, 3, 2.0, 0.5, s)
            + 10.0 * fbm(p.x / 28.0, p.y / 28.0, 2, 2.0, 0.5, s + 1);
        // (the cliffs come in only where there's room: by the route they stand a little further
        // out and wander less, so they never close in on it)
        let k = smoothstep(8.0, 32.0, self.route_dist(p));
        out + wob * k + (1.0 - k) * (0.35 * wob - 12.0)
    }
    /// How far `p` is from the walkable route (zones' floors and links' lanes).
    pub fn route_dist(&self, p: Vec2) -> f32 {
        let mut d = f32::MAX;
        for z in &self.zones {
            d = d.min((-z.inside(p)).max(0.0));
        }
        for l in &self.links {
            d = d.min((l.near(p).0 - l.half_w).max(0.0));
        }
        d
    }
}

/// A link in the world with its height profile: level from its `from` zone's floor, each stair a
/// ramp of `STAIR_RUN` m per m of rise, placed where the sheet put it but outside both zones.
fn link_w(zones: &[ZoneW], l: &super::blueprint::Link) -> Option<LinkW> {
    let a = zones.iter().find(|z| z.num == l.from)?;
    let b = zones.iter().find(|z| z.num == l.to)?;
    let pts: Vec<Vec2> = l.pts.iter().map(|p| w(*p)).collect();
    if pts.len() < 2 {
        return None;
    }
    let len = geom::polyline_len(&pts);
    // where it leaves `from` and reaches `to`
    let steps = (len / 1.0).ceil().max(2.0) as usize;
    let at = |s: f32| geom::polyline_at(&pts, s).0;
    let mut s_out = 0.0;
    for i in 0..=steps {
        let s = len * i as f32 / steps as f32;
        if a.inside(at(s)) < 0.0 {
            s_out = s;
            break;
        }
    }
    let mut s_in = len;
    for i in (0..=steps).rev() {
        let s = len * i as f32 / steps as f32;
        if b.inside(at(s)) < 0.0 {
            s_in = s;
            break;
        }
    }
    if s_in <= s_out {
        (s_out, s_in) = (len * 0.25, len * 0.75);
    }
    let room = s_in - s_out;
    let mut stairs: Vec<(f32, f32, f32)> = vec![];
    let total_run: f32 = l.stairs.iter().map(|s| s.rise.abs() * STAIR_RUN).sum();
    // (squeezed steeper when the sheet's link is too short for its stairs: 1.2 m per m at most)
    let squeeze = if total_run > room * 0.9 {
        (room * 0.9 / total_run.max(0.01)).max(1.2 / STAIR_RUN)
    } else {
        1.0
    };
    let mut cursor = s_out;
    let mut ss: Vec<_> = l.stairs.iter().filter(|s| s.rise.abs() > 0.05).collect();
    ss.sort_by(|x, y| x.at.total_cmp(&y.at));
    for s in ss {
        let run = s.rise.abs() * STAIR_RUN * squeeze;
        let c = (s.at * len).clamp(
            cursor + run * 0.5,
            (s_in - run * 0.5).max(cursor + run * 0.5),
        );
        let s0 = (c - run * 0.5).max(cursor);
        stairs.push((s0, s0 + run, s.rise));
        cursor = s0 + run + 1.0;
    }
    let mut profile = vec![];
    let mut y = a.top;
    let mut s = 0.0;
    for &(s0, s1, rise) in &stairs {
        profile.push((s, s0, y, y));
        profile.push((s0, s1, y, y + rise));
        y += rise;
        s = s1;
    }
    // (a link whose stairs don't make up the difference ends with a ramp onto its zone's floor)
    if (y - b.top).abs() > 0.3 {
        let run = ((y - b.top).abs() * STAIR_RUN).min((s_in - s).max(2.0));
        let s0 = (s_in - run).max(s);
        profile.push((s, s0, y, y));
        profile.push((s0, s_in.max(s0 + 0.5), y, b.top));
        stairs.push((s0, s_in.max(s0 + 0.5), b.top - y));
        s = s_in.max(s0 + 0.5);
        y = b.top;
    }
    profile.push((s, len.max(s + 0.01), y, y));
    let half_w = (if l.width > 0.5 {
        l.width
    } else if l.kind == LinkKind::Return {
        6.0
    } else {
        9.0
    })
    .clamp(3.0, 20.0)
        * 0.5;
    Some(LinkW {
        pts,
        half_w,
        kind: l.kind,
        profile,
        stairs: stairs.iter().map(|&(a, b, _)| (a, b)).collect(),
        len,
    })
}

/// The floor height (world y) for a sheet whose water is a sea at `level` (m above the Arrival):
/// that sea lies at the realm's own sea level (and the Arrival never below it).
pub fn floor_for_sea(level: f32, sea: f32) -> f32 {
    sea - level + if level > -0.6 { 0.6 } else { 0.0 }
}

/// The floor height (world y) a realm's land gives the sheet: the middle of the land under the
/// map, never down at the sea.
pub fn floor_level(h: &[f32], n: usize, cell: f32, half: Vec2, sea: f32) -> f32 {
    let mut v = vec![];
    each(n, cell, -half, half, |k, _| v.push(h[k]));
    if v.is_empty() {
        return sea + 6.0;
    }
    v.sort_by(f32::total_cmp);
    v[v.len() / 2].max(sea + 6.0)
}

/// Shapes the heights to the site (before the biomes).
pub fn carve(h: &mut [f32], n: usize, cell: f32, site: &Site) {
    let seed = site.seed & 0xffff;
    let half_grid = (n - 1) as f32 * cell * 0.5;
    let rim_top = site.top() + 12.0;
    let base = h.to_vec();
    // --- the ground: inside the map, rolling at the nearby zones' heights, rising into wooded
    // hills away from the route; outside it, cliffs up to the rim and mountains beyond
    let mut ground = vec![0.0f32; n * n];
    for k in 0..n * n {
        let p = Vec2::new(
            (k % n) as f32 * cell - half_grid,
            (k / n) as f32 * cell - half_grid,
        );
        let out = site.edge(p);
        if let (Some(sea), true) = (site.sea, out > 0.0) {
            // an island map: open water beyond its edge, a few rocks breaking it
            let rocks = (fbm(p.x / 30.0, p.y / 30.0, 3, 2.0, 0.5, seed + 7) - 0.35).max(0.0) * 30.0;
            ground[k] = (sea - 4.0 - 8.0 * smoothstep(0.0, 40.0, out) + rocks).min(interp(site, p));
            continue;
        }
        if out > 0.0 {
            // cliffs up to a rim of rolling heights, rising into ridges and peaks beyond
            let ridge = (1.0 - fbm(p.x / 60.0, p.y / 60.0, 4, 2.0, 0.5, seed + 3).abs()).powf(2.2);
            let cliff = rim_top
                + 9.0 * fbm(p.x / 45.0, p.y / 45.0, 3, 2.0, 0.5, seed + 4)
                + 3.0 * fbm(p.x / 12.0, p.y / 12.0, 2, 2.0, 0.5, seed + 5);
            let up = smoothstep(0.0, RIM_REACH, out);
            let inner = interp(site, p);
            let rim = inner
                + (cliff - inner) * up
                + ridge * 55.0 * smoothstep(RIM_REACH * 0.6, 110.0, out);
            ground[k] = rim.max(base[k].min(rim + 30.0));
            continue;
        }
        let g = interp(site, p);
        let away = site.route_dist(p);
        // knolls and rock outcrops between the zones, low by the route and higher away from it
        let hills = (fbm(p.x / 40.0, p.y / 40.0, 4, 2.0, 0.5, seed + 1) * 0.5 + 0.5)
            * 9.0
            * smoothstep(6.0, 30.0, away);
        let crags = (fbm(p.x / 16.0, p.y / 16.0, 3, 2.0, 0.5, seed + 6) - 0.15).max(0.0)
            * 9.0
            * smoothstep(10.0, 24.0, away);
        let bumps =
            fbm(p.x / 13.0, p.y / 13.0, 3, 2.0, 0.5, seed + 2) * 0.9 * smoothstep(4.0, 16.0, away);
        // (the map's edge rises a little into the cliffs; an island map's runs down to the sea)
        let edge = if site.sea.is_some() {
            0.0
        } else {
            5.0 * smoothstep(-26.0, 0.0, out)
        };
        ground[k] = g + hills + crags + bumps + edge;
    }
    // (the rolling ground smoothed; the cliffs stay sharp)
    {
        let mut sm = ground.clone();
        blur(&mut sm, n, ((4.0 / cell).round() as usize).max(1));
        for k in 0..n * n {
            let p = Vec2::new(
                (k % n) as f32 * cell - half_grid,
                (k / n) as f32 * cell - half_grid,
            );
            let wgt = 1.0 - smoothstep(-6.0, 2.0, site.edge(p));
            ground[k] += (sm[k] - ground[k]) * wgt;
        }
    }
    h.copy_from_slice(&ground);
    // --- the chasms, cut to their floors with steep walls
    for (outline, floor) in &site.chasms {
        let (lo, hi) = bounds(outline, 4.0);
        each(n, cell, lo, hi, |k, p| {
            let d = geom::poly_sdf(outline, p);
            if d > -1.0 {
                let wall = h[k] - (d + 1.0) * 2.6;
                h[k] = h[k].min(wall.max(*floor));
            }
        });
    }
    // --- the water's beds (the lakes are filled after the biomes); a low lip round them. The
    // zones keep a shore round them and the links a causeway, so islands stay islands
    if let (Some(sea), Some(mask)) = (site.sea, site.water_mask.as_ref()) {
        each(n, cell, -site.half, site.half, |k, p| {
            let wv = mask.at(p);
            if wv <= 0.35 {
                return;
            }
            let zone_d = site
                .zones
                .iter()
                .map(|z| (-z.inside(p)).max(0.0))
                .fold(f32::MAX, f32::min);
            let link_d = keeps_ground(site, p);
            let keep = smoothstep(10.0, 3.0, zone_d).max(smoothstep(3.0, 0.5, link_d));
            let bed = sea - 0.5 - 4.0 * smoothstep(0.45, 0.95, wv);
            let bed = bed + (h[k].max(sea + 0.8) - bed) * keep;
            h[k] = h[k].min(bed);
        });
    }
    for (outline, level) in &site.water {
        if site.water_mask.is_some() && site.sea.is_some_and(|s| (s - level).abs() < 0.3) {
            continue;
        }
        let (lo, hi) = bounds(outline, 10.0);
        each(n, cell, lo, hi, |k, p| {
            let d = geom::poly_sdf(outline, p);
            if d >= 0.0 {
                let zone_d = site
                    .zones
                    .iter()
                    .map(|z| (-z.inside(p)).max(0.0))
                    .fold(f32::MAX, f32::min);
                let link_d = keeps_ground(site, p);
                let keep = smoothstep(14.0, 5.0, zone_d).max(smoothstep(4.0, 1.0, link_d));
                let bed = level - 0.6 - 2.2 * smoothstep(0.0, 10.0, d);
                let bed = bed + (h[k].max(level + 0.8) - bed) * keep;
                h[k] = h[k].min(bed);
            } else if d > -8.0 && h[k] < level + 0.3 {
                h[k] = level + 0.3 * smoothstep(0.0, -3.0, d) + (h[k] - level).max(0.0);
            }
        });
    }
    // --- banks: every link's and zone's ground clamped into the cone round it (fill up below a
    // terrace, cut down above it)
    for l in &site.links {
        let (lo, hi) = bounds(&l.pts, l.half_w + 40.0);
        each(n, cell, lo, hi, |k, p| {
            let (d, s) = l.near(p);
            if over_hazard(site, l, s) {
                return;
            }
            let y = l.height(s);
            let off = (d - l.half_w).max(0.0);
            h[k] = h[k].clamp(y - off * DROP, y + off * CUT);
        });
    }
    for z in &site.zones {
        let r = z.reach() + 40.0;
        each(
            n,
            cell,
            z.c - Vec2::splat(r),
            z.c + Vec2::splat(r),
            |k, p| {
                let off = (-z.inside(p)).max(0.0);
                h[k] = h[k].clamp(z.top - off * DROP, z.top + off * CUT);
            },
        );
    }
    // --- the walkable ground set exactly: the links' lanes (outside the zones), then the floors
    for l in &site.links {
        let (lo, hi) = bounds(&l.pts, l.half_w + 2.0);
        each(n, cell, lo, hi, |k, p| {
            if site.zones.iter().any(|z| z.inside(p) > 0.0) {
                return;
            }
            let (d, s) = l.near(p);
            if over_hazard(site, l, s) {
                return;
            }
            if d <= l.half_w {
                h[k] = l.height(s);
            }
        });
    }
    for z in &site.zones {
        let r = z.reach() + 2.0;
        each(
            n,
            cell,
            z.c - Vec2::splat(r),
            z.c + Vec2::splat(r),
            |k, p| {
                if z.inside(p) >= 0.0 {
                    h[k] = z.top;
                }
            },
        );
    }
}

/// The ground's height between the zones: their floors blended by nearness to their edges.
fn interp(site: &Site, p: Vec2) -> f32 {
    let mut num = 0.0;
    let mut den = 0.0;
    for z in &site.zones {
        let d = (-z.inside(p)).max(0.0) + 6.0;
        let wgt = 1.0 / (d * d);
        num += z.top * wgt;
        den += wgt;
    }
    if den > 0.0 { num / den } else { site.y0 }
}

fn bounds(pts: &[Vec2], pad: f32) -> (Vec2, Vec2) {
    let mut lo = Vec2::splat(f32::MAX);
    let mut hi = Vec2::splat(f32::MIN);
    for p in pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    (lo - Vec2::splat(pad), hi + Vec2::splat(pad))
}

// ------------------------------------------------------------------ after the biomes

/// The layers a site paints with (the family's materials are set on them by `family`).
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    /// zone floors
    pub floor: Layer,
    /// boss rings and stairs
    pub accent: Layer,
    /// links' lanes
    pub path: Layer,
}

/// Paints the floors, rings, lanes, stairs, banks and chasm walls.
pub fn paint(t: &mut Terrain, site: &Site, pal: Paint) {
    let (n, cell) = (t.n(), t.cell);
    let mut dirty: Option<GridRect> = None;
    let mut mark = |t: &Terrain, c: Vec2, r: f32| {
        if let Some(rc) = t.rect_for_circle(c, r) {
            dirty = Some(match dirty {
                Some(d) => d.union(rc),
                None => rc,
            });
        }
    };
    // the chasms' walls dark rock, their floors gravel
    for (outline, floor) in &site.chasms {
        let (lo, hi) = bounds(outline, 3.0);
        let mut ks = vec![];
        each(n, cell, lo, hi, |k, p| ks.push((k, p)));
        for (k, p) in ks {
            let d = geom::poly_sdf(outline, p);
            if d > -2.0 {
                let on_floor = t.heights[k] < floor + 0.8;
                put(
                    t,
                    k,
                    if on_floor {
                        Layer::Gravel
                    } else {
                        Layer::DarkRock
                    },
                    1.0,
                );
            }
        }
        let c = (lo + hi) * 0.5;
        mark(t, c, (hi - lo).length() * 0.5);
    }
    // the links: their lanes, their stairs paved, their banks rock where steep
    for l in &site.links {
        let (lo, hi) = bounds(&l.pts, l.half_w + 12.0);
        let mut ks = vec![];
        each(n, cell, lo, hi, |k, p| ks.push((k, p)));
        for (k, p) in ks {
            let (d, s) = l.near(p);
            if over_hazard(site, l, s) {
                continue;
            }
            if d <= l.half_w + 0.6 {
                let layer = if l.on_stairs(s) { pal.accent } else { pal.path };
                put(t, k, layer, smoothstep(l.half_w + 0.6, l.half_w - 0.6, d));
            } else if d < l.half_w + 10.0 && (t.heights[k] - l.height(s)).abs() > 1.2 {
                put(t, k, Layer::Rock, 1.0);
            }
        }
        mark(t, (lo + hi) * 0.5, (hi - lo).length() * 0.5);
    }
    // the zones: their floors paved, their rings in the accent, their banks rock
    for z in &site.zones {
        let r = z.reach() + 14.0;
        let mut ks = vec![];
        each(
            n,
            cell,
            z.c - Vec2::splat(r),
            z.c + Vec2::splat(r),
            |k, p| ks.push((k, p)),
        );
        let floor = if z.role == Role::Side {
            Layer::ForestFloor
        } else {
            pal.floor
        };
        for (k, p) in ks {
            let d = z.inside(p);
            if d >= -0.6 {
                put(t, k, floor, smoothstep(-0.6, 0.6, d));
                if let Some((c, rr)) = z.ring {
                    let e = (p.distance(c) - rr).abs();
                    // (a band round the ring and a lighter disc inside it)
                    if e < 1.2 {
                        put(t, k, pal.accent, 1.0);
                    } else if p.distance(c) < rr {
                        put(t, k, pal.accent, 0.35);
                    }
                }
            } else if d > -12.0 && (t.heights[k] - z.top).abs() > 1.0 {
                put(t, k, Layer::Rock, 1.0);
            }
        }
        mark(t, z.c, r);
    }
    if let Some(r) = dirty {
        t.mark_splat_dirty(r);
    }
}

/// How cleared of trees and plants a spot is (1: bare) and how thick the woods are (0..1).
pub fn clearing(site: &Site, p: Vec2) -> (f32, f32) {
    let mut clear = 0.0f32;
    for z in &site.zones {
        clear = clear.max(smoothstep(-5.0, 0.0, z.inside(p)));
    }
    for l in &site.links {
        let (d, _) = l.near(p);
        clear = clear.max(1.0 - smoothstep(l.half_w + 0.5, l.half_w + 3.0, d));
    }
    for (o, _) in &site.chasms {
        clear = clear.max(smoothstep(-2.0, 1.0, geom::poly_sdf(o, p)));
    }
    match &site.water_mask {
        Some(m) if site.sea.is_some() => clear = clear.max(smoothstep(0.3, 0.6, m.at(p))),
        _ => {
            for (o, _) in &site.water {
                clear = clear.max(0.7 * smoothstep(-1.0, 1.0, geom::poly_sdf(o, p)));
            }
        }
    }
    let thick = smoothstep(5.0, 26.0, site.route_dist(p))
        * (1.0 - smoothstep(-8.0, 0.0, site.edge(p)) * 0.4);
    (clear, thick)
}

/// Whether `p` is on a stair (for the rails) — (link index, along).
pub fn stair_at(site: &Site, p: Vec2) -> Option<(usize, f32)> {
    for (i, l) in site.links.iter().enumerate() {
        let (d, s) = l.near(p);
        if d <= l.half_w && l.on_stairs(s) {
            return Some((i, s));
        }
    }
    None
}

/// The stairs' head and foot points (world x/z) and their widths, for the rails.
pub fn stair_ends(site: &Site) -> Vec<(Vec2, Vec2, f32)> {
    let mut v = vec![];
    for l in &site.links {
        for &(a, b) in &l.stairs {
            if over_hazard(site, l, (a + b) * 0.5) {
                continue;
            }
            let pa = geom::polyline_at(&l.pts, a).0;
            let pb = geom::polyline_at(&l.pts, b).0;
            // (the head is the higher end)
            if l.height(a) >= l.height(b) {
                v.push((pa, pb, l.half_w * 2.0));
            } else {
                v.push((pb, pa, l.half_w * 2.0));
            }
        }
    }
    v
}

/// Distance from `p` to a stair's centre segment (f32::MAX off it).
pub fn stair_band(p: Vec2, head: Vec2, foot: Vec2) -> f32 {
    geom_band(p, head, foot)
}

/// A small four-zone blueprint for tests.
#[cfg(test)]
pub(crate) fn tests_sample() -> Blueprint {
    tests::sample()
}

#[cfg(test)]
mod tests {
    use super::super::blueprint::*;
    use super::*;

    pub(crate) fn sample() -> Blueprint {
        let z =
            |num: u8, name: &str, role: Role, c: P, size: [f32; 2], e: f32, boss: Option<f32>| {
                Zone {
                    num,
                    name: name.into(),
                    role,
                    c,
                    size,
                    elevation: e,
                    boss: boss.map(|d| Ring {
                        c,
                        d,
                        label: format!("Boss {num}"),
                    }),
                    ..Default::default()
                }
            };
        let link = |from: u8, to: u8, pts: Vec<P>, rise: f32| Link {
            from,
            to,
            pts,
            width: 9.0,
            kind: LinkKind::Path,
            stairs: if rise != 0.0 {
                vec![Stair {
                    at: 0.5,
                    rise,
                    tag: "D".into(),
                }]
            } else {
                vec![]
            },
        };
        Blueprint {
            version: VERSION,
            id: "test".into(),
            name: "Test".into(),
            family: "temperate".into(),
            size: [300.0, 200.0],
            zones: vec![
                z(
                    1,
                    "Arrival",
                    Role::Arrival,
                    [-110.0, -60.0],
                    [40.0, 30.0],
                    0.0,
                    None,
                ),
                z(
                    2,
                    "Court",
                    Role::Boss,
                    [-90.0, 50.0],
                    [60.0, 50.0],
                    4.0,
                    Some(28.0),
                ),
                z(
                    3,
                    "Garden",
                    Role::Area,
                    [20.0, 60.0],
                    [50.0, 40.0],
                    4.0,
                    None,
                ),
                z(
                    4,
                    "Arena",
                    Role::Final,
                    [100.0, -40.0],
                    [60.0, 60.0],
                    12.0,
                    Some(40.0),
                ),
            ],
            links: vec![
                link(
                    1,
                    2,
                    vec![[-110.0, -60.0], [-105.0, 0.0], [-90.0, 50.0]],
                    4.0,
                ),
                link(2, 3, vec![[-90.0, 50.0], [20.0, 60.0]], 0.0),
                link(3, 4, vec![[20.0, 60.0], [70.0, 20.0], [100.0, -40.0]], 8.0),
            ],
            chasms: vec![Chasm {
                name: "Ravine".into(),
                outline: vec![[-40.0, -40.0], [30.0, -40.0], [30.0, 10.0], [-40.0, 10.0]],
                floor: -10.0,
            }],
            ..Default::default()
        }
    }

    fn grid() -> (usize, f32, Vec<f32>) {
        let n = 257;
        (n, 2.0, vec![20.0; n * n])
    }

    fn at(h: &[f32], n: usize, cell: f32, p: Vec2) -> f32 {
        let half = (n - 1) as f32 * cell * 0.5;
        let i = ((p.x + half) / cell).round() as usize;
        let j = ((p.y + half) / cell).round() as usize;
        h[j * n + i]
    }

    #[test]
    fn floors_sit_at_their_elevations_and_stairs_climb() {
        let bp = sample();
        assert!(bp.problems().is_empty(), "{:?}", bp.problems());
        let (n, cell, mut h) = grid();
        let y0 = floor_level(&h, n, cell, Vec2::new(150.0, 100.0), 0.0);
        let site = Site::new(&bp, y0, 7);
        carve(&mut h, n, cell, &site);
        for z in &site.zones {
            assert!(
                (at(&h, n, cell, z.c) - z.top).abs() < 0.01,
                "zone {} floor",
                z.num
            );
        }
        // the link 3 → 4 climbs 8 m: low near 3, high near 4, in between on the way
        let l = &site.links[2];
        assert!((l.height(0.0) - (y0 + 4.0)).abs() < 0.01);
        assert!((l.height(l.len) - (y0 + 12.0)).abs() < 0.01);
        let mid = geom::polyline_at(&l.pts, l.len * 0.5).0;
        let hm = at(&h, n, cell, mid);
        assert!(
            hm >= y0 + 4.0 - 0.01 && hm <= y0 + 12.0 + 0.01,
            "mid-link {hm}"
        );
        // the chasm is cut deep, the rim stands high outside the map
        assert!(at(&h, n, cell, w([-5.0, -15.0])) < y0 - 5.0, "chasm");
        assert!(at(&h, n, cell, w([0.0, 140.0])) > site.top() + 5.0, "rim");
    }

    #[test]
    fn a_link_without_its_stairs_still_reaches_its_zone() {
        let mut bp = sample();
        bp.links[2].stairs.clear();
        let site = Site::new(&bp, 10.0, 1);
        let l = &site.links[2];
        assert!((l.height(l.len) - site.zone(4).unwrap().top).abs() < 0.01);
        assert!(!l.stairs.is_empty(), "a ramp was added");
    }

    #[test]
    fn zone_platforms_interrupt_a_wide_chasm_bridge_span() {
        let mut bp = sample();
        // A hand-read chasm can include the raised destination plaza. The centre route remains
        // unsupported, but neither solid zone should acquire a bridge deck.
        bp.chasms[0].outline = vec![
            [-135.0, -75.0],
            [-65.0, -75.0],
            [-65.0, 75.0],
            [-135.0, 75.0],
        ];
        let site = Site::new(&bp, 10.0, 1);
        let link = &site.links[0];
        let samples: Vec<_> = (0..=100).map(|i| link.len * i as f32 / 100.0).collect();
        let on_platform = samples
            .iter()
            .copied()
            .find(|s| {
                let p = geom::polyline_at(&link.pts, *s).0;
                site.zones.iter().any(|z| z.inside(p) >= 0.0)
            })
            .expect("link enters a zone");
        let over_gap = samples
            .iter()
            .copied()
            .find(|s| {
                let p = geom::polyline_at(&link.pts, *s).0;
                site.zones.iter().all(|z| z.inside(p) < 0.0)
            })
            .expect("link crosses a gap");
        assert!(!over_hazard(&site, link, on_platform));
        assert!(over_hazard(&site, link, over_gap));
    }
}
