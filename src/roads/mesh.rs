//! Road geometry other systems can reuse: the surface ribbon (`road_ribbon_mesh` for exporters,
//! `ribbon_chunks` for the editor) and a rasterised road mask (`road_mask`, for pacing analysis
//! and the world map).
//!
//! The ribbon drapes over the rendered terrain exactly: every vertex takes the height of the
//! full-detail terrain triangle under it (`surface_height`), with the terrain's smooth normal, so
//! the alpha-blended edges shade like the ground they fade into. Stone bridge decks carry the
//! ribbon at deck height; timber bridges get planks instead, with the ribbon fading out over the
//! first planks. Road ends get rounded caps. On tight bends the inner edge is pulled in so the
//! ribbon never folds over.

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::plan::RoadPlan;
use super::{RoadKind, Roads};
use crate::terrain::Terrain;
use crate::water::WaterBodies;

/// Height above the ground the ribbon is built at (the shader adds a view-dependent pull).
const LIFT: f32 = 0.025;
/// Ribbon chunk length (m): chunks are culled and sorted separately.
const CHUNK: usize = 96;
/// How far the ribbon runs onto a timber deck before it has faded out (m).
const DECK_FADE: f32 = 1.6;

/// Height of the rendered terrain (its full-detail triangles, alternating diagonals) at a point.
pub fn surface_height(t: &Terrain, x: f32, z: f32) -> f32 {
    let g = t.world_to_grid(x, z);
    let max = t.res as f32;
    let (gx, gz) = (g.x.clamp(0.0, max), g.y.clamp(0.0, max));
    let i = (gx.floor() as usize).min(t.res - 1);
    let j = (gz.floor() as usize).min(t.res - 1);
    let (fx, fz) = (gx - i as f32, gz - j as f32);
    let (h00, h10, h01, h11) = (t.h(i, j), t.h(i + 1, j), t.h(i, j + 1), t.h(i + 1, j + 1));
    if (i + j) % 2 == 0 {
        if fx + fz <= 1.0 {
            h00 + (h10 - h00) * fx + (h01 - h00) * fz
        } else {
            h11 + (h01 - h11) * (1.0 - fx) + (h10 - h11) * (1.0 - fz)
        }
    } else if fz >= fx {
        h00 + (h11 - h01) * fx + (h01 - h00) * fz
    } else {
        h00 + (h10 - h00) * fx + (h11 - h10) * fz
    }
}

/// The terrain's smooth normal at a point (bilinear between vertex normals, as the shader does).
pub fn surface_normal(t: &Terrain, x: f32, z: f32) -> Vec3 {
    let g = t.world_to_grid(x, z);
    let max = t.res as f32;
    let (gx, gz) = (g.x.clamp(0.0, max), g.y.clamp(0.0, max));
    let i = (gx.floor() as usize).min(t.res - 1);
    let j = (gz.floor() as usize).min(t.res - 1);
    let (fx, fz) = (gx - i as f32, gz - j as f32);
    let a = t.normal_at_grid(i, j).lerp(t.normal_at_grid(i + 1, j), fx);
    let b = t
        .normal_at_grid(i, j + 1)
        .lerp(t.normal_at_grid(i + 1, j + 1), fx);
    a.lerp(b, fz).normalize()
}

/// Which samples carry the ribbon: ordinary roads retain their terrain-following runs, while an
/// authored route carries only declared bridge spans and their short raised landings. Timber deck
/// middles are omitted for their individual planks. Returns (first, last).
fn pieces(plan: &RoadPlan) -> Vec<(usize, usize)> {
    let n = plan.samples.len();
    if n < 2 {
        return vec![];
    }
    let over = (DECK_FADE / super::plan::DS).ceil() as usize;
    let mut draw = if plan.authored {
        let mut draw = vec![false; n];
        for (first, last) in plan.authored_deck_ranges() {
            draw[first..=last].fill(true);
        }
        draw
    } else {
        vec![true; n]
    };
    for sp in plan.spans.iter().filter(|sp| !sp.stone) {
        let (a, b) = (sp.i0 + over + 1, sp.i1.saturating_sub(over));
        if a < b {
            draw[a..b].iter_mut().for_each(|d| *d = false);
        }
    }
    let mut out = vec![];
    let mut i = 0;
    while i < n {
        if !draw[i] {
            i += 1;
            continue;
        }
        let a = i;
        while i + 1 < n && draw[i + 1] {
            i += 1;
        }
        out.push((a, i));
        i += 1;
    }
    out
}

/// Vertex data of a ribbon.
struct Ribbon {
    pos: Vec<[f32; 3]>,
    nor: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    uv_b: Vec<[f32; 2]>,
    tan: Vec<[f32; 4]>,
    col: Vec<[f32; 4]>,
    idx: Vec<u32>,
}

/// One cross-section of the ribbon.
struct Row {
    p: Vec2,
    dir: Vec2,
    s: f32,
    /// half width the surface fades out at (narrower on the rounded end caps)
    hw: f32,
    /// end fade (timber deck edges)
    fade: f32,
    curvature: f32,
    /// on an authored or stone deck: its height and slope
    deck: Option<(f32, f32)>,
}

/// The ribbon over samples `i0..=i1` of the piece `piece`, with rounded caps where the road
/// itself starts / ends inside this range.
fn ribbon(
    plan: &RoadPlan,
    t: &Terrain,
    seed: f32,
    i0: usize,
    i1: usize,
    piece: (usize, usize),
) -> Ribbon {
    let st = plan.kind.style();
    let hw = plan.hw;
    let outer = hw + st.edge + st.breakup * 1.6;
    let cols = (2.0 * (outer / (hw / 3.0).clamp(0.35, 1.0)).ceil()) as usize + 1;
    let n = plan.samples.len();
    let (s_first, s_last) = (plan.samples[piece.0].s, plan.samples[piece.1].s);
    let mut rows: Vec<Row> = Vec::with_capacity(i1 - i0 + 16);
    // a rounded cap: rows past the end whose half width shrinks along a quarter ellipse
    let cap = |rows: &mut Vec<Row>, at: usize, sign: f32| {
        let sm = plan.samples[at];
        let len = hw + st.edge;
        let m = (len / 0.4).ceil().max(2.0) as usize;
        let ks: Vec<usize> = if sign < 0.0 {
            (1..=m).rev().collect()
        } else {
            (1..=m).collect()
        };
        for k in ks {
            let d = len * k as f32 / m as f32;
            let w = (1.0 - (d / len).powi(2)).max(0.0).sqrt();
            rows.push(Row {
                p: sm.p + sm.dir * d * sign,
                dir: sm.dir,
                s: sm.s + d * sign,
                hw: hw * w,
                fade: 1.0,
                curvature: 0.0,
                deck: plan.authored.then(|| {
                    let slope = if at + 1 < n {
                        (plan.h[at + 1] - plan.h[at])
                            / (plan.samples[at + 1].s - plan.samples[at].s).max(1e-3)
                    } else if at > 0 {
                        (plan.h[at] - plan.h[at - 1])
                            / (plan.samples[at].s - plan.samples[at - 1].s).max(1e-3)
                    } else {
                        0.0
                    };
                    (plan.h[at], slope)
                }),
            });
        }
    };
    if i0 == 0 && piece.0 == 0 {
        cap(&mut rows, 0, -1.0);
    }
    for i in i0..=i1 {
        let sm = plan.samples[i];
        // signed curvature, smoothed over a few metres
        let (a, b) = (
            plan.samples[i.saturating_sub(3)],
            plan.samples[(i + 3).min(n - 1)],
        );
        let curvature = a.dir.perp_dot(b.dir) / (b.s - a.s).max(1e-3);
        let mut fade = 1.0f32;
        if piece.0 > 0 {
            fade = fade.min((sm.s - s_first) / DECK_FADE);
        }
        if piece.1 < n - 1 {
            fade = fade.min((s_last - sm.s) / DECK_FADE);
        }
        let deck = (plan.authored || plan.span_of(i).is_some()).then(|| {
            let slope = if i + 1 < n {
                (plan.h[i + 1] - plan.h[i]) / (plan.samples[i + 1].s - sm.s).max(1e-3)
            } else {
                0.0
            };
            (plan.h[i], slope)
        });
        rows.push(Row {
            p: sm.p,
            dir: sm.dir,
            s: sm.s,
            hw,
            fade: fade.clamp(0.0, 1.0),
            curvature,
            deck,
        });
    }
    if i1 == n - 1 && piece.1 == n - 1 {
        cap(&mut rows, n - 1, 1.0);
    }

    let mut r = Ribbon {
        pos: vec![],
        nor: vec![],
        uv: vec![],
        uv_b: vec![],
        tan: vec![],
        col: vec![],
        idx: vec![],
    };
    for row in &rows {
        let lat = row.dir.perp();
        for c in 0..cols {
            let mut o = -outer + 2.0 * outer * c as f32 / (cols - 1) as f32;
            // inner side of a bend: never past the centre of curvature
            if o * row.curvature > 0.0 {
                o = o.signum() * o.abs().min(0.9 / row.curvature.abs());
            }
            let q = row.p + lat * o;
            let (y, nrm) = match row.deck {
                Some((h, slope)) => (
                    h + LIFT,
                    Vec3::new(-row.dir.x * slope, 1.0, -row.dir.y * slope).normalize(),
                ),
                None => (
                    surface_height(t, q.x, q.y) + LIFT,
                    surface_normal(t, q.x, q.y),
                ),
            };
            r.pos.push([q.x, y, q.y]);
            r.nor.push(nrm.to_array());
            r.uv.push([o, row.s]);
            r.uv_b.push([o / hw, row.fade]);
            let tl = Vec3::new(lat.x, 0.0, lat.y);
            let tl = (tl - nrm * tl.dot(nrm)).normalize_or(Vec3::X);
            let along = Vec3::new(row.dir.x, 0.0, row.dir.y);
            let w = if nrm.cross(tl).dot(along) >= 0.0 {
                1.0
            } else {
                -1.0
            };
            r.tan.push([tl.x, tl.y, tl.z, w]);
            r.col.push([
                seed,
                row.hw,
                if row.deck.is_some() { 1.0 } else { 0.0 },
                1.0,
            ]);
        }
    }
    let nrows = rows.len();
    // the ground can rise between the ribbon's vertices (a crease of the terrain grid, the edge
    // of a bank): lift each quad until the ground under it stays below
    let mut raise = vec![0.0f32; r.pos.len()];
    for row in 0..nrows - 1 {
        if rows[row].deck.is_some() || rows[row + 1].deck.is_some() {
            continue;
        }
        for c in 0..cols - 1 {
            let ids = [
                row * cols + c,
                row * cols + c + 1,
                (row + 1) * cols + c,
                (row + 1) * cols + c + 1,
            ];
            let q = ids.map(|k| Vec3::from_array(r.pos[k]));
            let mut excess = 0.0f32;
            for (u, v) in [
                (0.5, 0.5),
                (0.5, 0.0),
                (0.5, 1.0),
                (0.0, 0.5),
                (1.0, 0.5),
                (0.25, 0.25),
                (0.75, 0.75),
                (0.25, 0.75),
                (0.75, 0.25),
            ] {
                let x = q[0].lerp(q[1], u).lerp(q[2].lerp(q[3], u), v);
                excess = excess.max(surface_height(t, x.x, x.z) + LIFT - x.y);
            }
            if excess > 0.0 {
                for k in ids {
                    raise[k] = raise[k].max(excess);
                }
            }
        }
    }
    for (p, d) in r.pos.iter_mut().zip(raise) {
        p[1] += d;
    }
    for row in 0..nrows - 1 {
        for c in 0..cols - 1 {
            let a = (row * cols + c) as u32;
            let b = a + 1;
            let d = a + cols as u32;
            let e = d + 1;
            // counter-clockwise seen from above
            r.idx.extend_from_slice(&[a, b, d, b, e, d]);
        }
    }
    r
}

fn to_mesh(r: Ribbon) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, r.pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, r.nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, r.uv);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_1, r.uv_b);
    m.insert_attribute(Mesh::ATTRIBUTE_TANGENT, r.tan);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, r.col);
    m.insert_indices(Indices::U32(r.idx));
    m
}

fn seed_of(id: u32) -> f32 {
    (crate::noise::hash_u32(id.wrapping_mul(2654435761)) & 0xffff) as f32 / 65535.0
}

/// The ribbon in culling-sized chunks (≈ 96 m each), with their bounds.
pub fn ribbon_chunks(plan: &RoadPlan, t: &Terrain, id: u32) -> Vec<(Mesh, Aabb)> {
    let mut out = vec![];
    let seed = seed_of(id);
    for (a, b) in pieces(plan) {
        if b <= a {
            continue;
        }
        let mut i = a;
        while i < b {
            let j = (i + CHUNK).min(b);
            let r = ribbon(plan, t, seed, i, j, (a, b));
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in &r.pos {
                lo = lo.min(Vec3::from_array(*p));
                hi = hi.max(Vec3::from_array(*p));
            }
            out.push((
                to_mesh(r),
                Aabb::from_min_max(lo - Vec3::splat(0.5), hi + Vec3::splat(0.5)),
            ));
            i = j;
        }
    }
    out
}

/// One road's whole surface ribbon as a single mesh (positions, normals, UV0 = across / along in
/// metres, UV1 = across / half width and end fade, tangents, colours), e.g. for exporters. The
/// surface's texture is the kind's `KindStyle::texture` from the terrain library.
pub fn road_ribbon_mesh(roads: &Roads, id: u32, t: &Terrain, water: &WaterBodies) -> Option<Mesh> {
    let base = super::ground::Base {
        t,
        g: &roads.ground,
    };
    let plan = super::plan::plan_all(roads, &base, water)
        .into_iter()
        .find(|p| p.id == id)?;
    let seed = seed_of(id);
    let mut all = Ribbon {
        pos: vec![],
        nor: vec![],
        uv: vec![],
        uv_b: vec![],
        tan: vec![],
        col: vec![],
        idx: vec![],
    };
    for (a, b) in pieces(&plan) {
        if b <= a {
            continue;
        }
        let r = ribbon(&plan, t, seed, a, b, (a, b));
        let off = all.pos.len() as u32;
        all.idx.extend(r.idx.iter().map(|i| i + off));
        all.pos.extend(r.pos);
        all.nor.extend(r.nor);
        all.uv.extend(r.uv);
        all.uv_b.extend(r.uv_b);
        all.tan.extend(r.tan);
        all.col.extend(r.col);
    }
    if all.idx.is_empty() {
        return None;
    }
    Some(to_mesh(all))
}

/// Roads rasterised over the whole terrain: `size` × `size` pixels, row-major from the
/// (−x, −z) corner. `coverage` is 0..255 (antialiased), `kind` the `RoadKind` index + 1 of the
/// widest road at the pixel (0 = none).
pub struct RoadMask {
    pub size: usize,
    pub coverage: Vec<u8>,
    pub kind: Vec<u8>,
}

impl RoadMask {
    /// The road kind at a world position, if any.
    pub fn kind_at(&self, t: &Terrain, x: f32, z: f32) -> Option<RoadKind> {
        let u = ((x + t.half()) / t.size() * self.size as f32).floor();
        let v = ((z + t.half()) / t.size() * self.size as f32).floor();
        if u < 0.0 || v < 0.0 || u >= self.size as f32 || v >= self.size as f32 {
            return None;
        }
        let k = self.kind[v as usize * self.size + u as usize];
        if k == 0 {
            None
        } else {
            Some(RoadKind::ALL[(k - 1) as usize])
        }
    }
}

pub fn road_mask(roads: &Roads, t: &Terrain, size: usize) -> RoadMask {
    let size = size.max(1);
    let mut mask = RoadMask {
        size,
        coverage: vec![0; size * size],
        kind: vec![0; size * size],
    };
    let px = t.size() / size as f32;
    let mut order: Vec<_> = roads.roads.iter().collect();
    order.sort_by_key(|r| r.rank());
    for road in order {
        let hw = road.width * 0.5;
        let samples = super::plan::dense_samples(road, (px * 0.5).clamp(0.5, 4.0));
        for w in samples.windows(2) {
            let (a, b) = (w[0].p, w[1].p);
            let lo = (a.min(b) - Vec2::splat(hw + px) + Vec2::splat(t.half())) / px;
            let hi = (a.max(b) + Vec2::splat(hw + px) + Vec2::splat(t.half())) / px;
            let (x0, y0) = (
                lo.x.floor().max(0.0) as usize,
                lo.y.floor().max(0.0) as usize,
            );
            let (x1, y1) = (
                (hi.x.ceil() as usize).min(size - 1),
                (hi.y.ceil() as usize).min(size - 1),
            );
            if lo.x > (size - 1) as f32 || lo.y > (size - 1) as f32 || hi.x < 0.0 || hi.y < 0.0 {
                continue;
            }
            let ab = b - a;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let p = Vec2::new((x as f32 + 0.5) * px, (y as f32 + 0.5) * px)
                        - Vec2::splat(t.half());
                    let u = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                    let d = p.distance(a + ab * u);
                    let c = ((hw - d) / px + 0.5).clamp(0.0, 1.0);
                    if c <= 0.0 {
                        continue;
                    }
                    let k = y * size + x;
                    let v = (c * 255.0).round() as u8;
                    if v > mask.coverage[k] {
                        mask.coverage[k] = v;
                    }
                    if c >= 0.5 {
                        mask.kind[k] = road.kind.index() as u8 + 1;
                    }
                }
            }
        }
    }
    mask
}
