//! Shaping a compound into its realm's land: the open edge (the valley's cliffs behind the
//! entrance lowered to wooded slopes running down to the sea), low ground round the lakes, the
//! hub levelled, the plaza and the courts raised as platforms with steep stone banks and stairs
//! cut toward their trails, the processional way graded straight up to the final court's
//! terrace; then (once the biomes have grown) the lakes filled, the island raised in its lake,
//! each court's floor painted its own colour, the paving laid, the nature kept off the courts
//! and grown thick in the tree clumps between them.

use bevy::math::{Vec2, Vec3};
use bevy::prelude::warn;

use super::layout::{Court, CourtKind, Floor, Grid, Plan, side};
use super::theme::Theme;
use crate::io::WorldData;
use crate::noise::{fbm, smoothstep};
use crate::terrain::layers::Layer;
use crate::terrain::{GridRect, Terrain};

/// Stone banks: metres of rise per metre out from a platform's edge.
const BANK: f32 = 1.3;
/// Stairs: metres along per metre of rise.
pub const STAIR_RUN: f32 = 2.4;

/// Runs `f(k, p)` over every vertex in the box `lo`..`hi`.
pub(crate) fn each(n: usize, cell: f32, lo: Vec2, hi: Vec2, mut f: impl FnMut(usize, Vec2)) {
    let half = (n - 1) as f32 * cell * 0.5;
    let i0 = (((lo.x + half) / cell).floor().max(0.0) as usize).min(n - 1);
    let i1 = (((hi.x + half) / cell).ceil().max(0.0) as usize).min(n - 1);
    let j0 = (((lo.y + half) / cell).floor().max(0.0) as usize).min(n - 1);
    let j1 = (((hi.y + half) / cell).ceil().max(0.0) as usize).min(n - 1);
    for j in j0..=j1 {
        for i in i0..=i1 {
            f(
                j * n + i,
                Vec2::new(i as f32 * cell - half, j as f32 * cell - half),
            );
        }
    }
}

/// A box blur of radius `r` vertices over the grid (rows, then columns).
pub(crate) fn blur(v: &mut [f32], n: usize, r: usize) {
    let mut tmp = vec![0.0f32; n * n];
    // rows
    for j in 0..n {
        let row = &v[j * n..(j + 1) * n];
        let mut acc = 0.0;
        let mut cnt = 0usize;
        for i in 0..(r + 1).min(n) {
            acc += row[i];
            cnt += 1;
        }
        for i in 0..n {
            tmp[j * n + i] = acc / cnt as f32;
            if i + r + 1 < n {
                acc += row[i + r + 1];
                cnt += 1;
            }
            if i >= r {
                acc -= row[i - r];
                cnt -= 1;
            }
        }
    }
    // columns
    for i in 0..n {
        let mut acc = 0.0;
        let mut cnt = 0usize;
        for j in 0..(r + 1).min(n) {
            acc += tmp[j * n + i];
            cnt += 1;
        }
        for j in 0..n {
            v[j * n + i] = acc / cnt as f32;
            if j + r + 1 < n {
                acc += tmp[(j + r + 1) * n + i];
                cnt += 1;
            }
            if j >= r {
                acc -= tmp[(j - r) * n + i];
                cnt -= 1;
            }
        }
    }
}

/// The height a ramp from `head` (at `top`) down to `foot` (at `low`), `w` wide, gives at `p`
/// (with banks down its sides), or None off it.
fn ramp(p: Vec2, head: Vec2, foot: Vec2, w: f32, top: f32, low: f32) -> Option<f32> {
    let l = head.distance(foot).max(0.5);
    let d = (foot - head) / l;
    let u = (p - head).dot(d);
    let v = (p - head).dot(side(d)).abs();
    if u < -2.0 || u > l + 2.0 {
        return None;
    }
    let y = top + (low - top) * (u / l).clamp(0.0, 1.0);
    let y = y - (v - w * 0.5).max(0.0) * BANK;
    (y > low - 0.5).then_some(y)
}

/// The land's heights shaped for the plan (before the biomes grow on it).
pub fn carve(h: &mut [f32], n: usize, cell: f32, plan: &Plan) {
    let orig = h.to_vec();
    let g0 = Grid { n, cell, h: &orig };
    let Some(floor) = Floor::find(&g0) else {
        return;
    };
    let half = (n - 1) as f32 * cell * 0.5;
    let level = plan.floor;
    let seed = plan.seed & 0xffff;

    // --- the floor smoothed (the erosion's straight rills across a level floor go; the walls'
    // feet stay as they are)
    {
        let mut sm = h.to_vec();
        let r = ((9.0 / cell).round() as usize).max(1);
        for _ in 0..3 {
            blur(&mut sm, n, r);
        }
        for k in 0..n * n {
            let p = Vec2::new((k % n) as f32 * cell - half, (k / n) as f32 * cell - half);
            let near_e = 1.0
                - smoothstep(
                    plan.entrance.r + 60.0,
                    plan.entrance.r + 180.0,
                    p.distance(plan.entrance.c),
                );
            let w = smoothstep(0.5, 14.0, floor.room(p)).max(near_e);
            if w > 0.0 {
                h[k] += (sm[k] - h[k]) * w;
            }
        }
    }
    // --- the open edge: behind the entrance the cliffs give way to low wooded slopes (worked
    // out as a change and blurred, so it leaves no creases)
    {
        let outside = floor.outside();
        let e = plan.entrance.c;
        let back = plan.open;
        let reach = plan
            .entrance
            .wall
            .iter()
            .map(|p| p.distance(e))
            .fold(plan.entrance.r * 4.0, f32::max)
            + 40.0;
        let mut delta = vec![0.0f32; n * n];
        for k in 0..n * n {
            let p = Vec2::new((k % n) as f32 * cell - half, (k / n) as f32 * cell - half);
            let d = p - e;
            let dist = d.length();
            if dist > reach + 200.0 || floor.room(p) > 0.5 {
                continue;
            }
            let wa = smoothstep(
                (105f32).to_radians().cos(),
                (70f32).to_radians().cos(),
                d.normalize_or(back).dot(back),
            );
            let wr = 1.0 - smoothstep(reach, reach + 200.0, dist);
            let w = wa * wr;
            if w <= 0.0 {
                continue;
            }
            let off = outside[k];
            let hills = fbm(p.x / 90.0, p.y / 90.0, 4, 2.0, 0.5, seed + 5) * 0.5 + 0.5;
            let target = level
                + 1.5
                + 9.0 * smoothstep(0.0, 160.0, off)
                + 7.0 * hills * smoothstep(10.0, 80.0, off);
            delta[k] = -(h[k] - target).max(0.0) * w;
        }
        let r = ((10.0 / cell).round() as usize).max(2);
        blur(&mut delta, n, r);
        blur(&mut delta, n, r);
        for k in 0..n * n {
            // (never raised, never cut below the floor)
            if delta[k] < 0.0 {
                h[k] = (h[k] + delta[k]).max(h[k].min(level - 0.3));
            }
        }
    }
    // --- the low ground the lakes lie in
    for l in &plan.lakes {
        let r = l.r + 34.0;
        each(
            n,
            cell,
            l.c - Vec2::splat(r),
            l.c + Vec2::splat(r),
            |k, p| {
                let w = 1.0 - smoothstep(l.r * 0.8, l.r + 34.0, p.distance(l.c));
                h[k] -= 1.4 * w;
            },
        );
    }
    // --- the hub, levelled
    {
        let hub = &plan.hub;
        let r = hub.r + 16.0;
        each(
            n,
            cell,
            hub.c - Vec2::splat(r),
            hub.c + Vec2::splat(r),
            |k, p| {
                let w = 1.0 - smoothstep(hub.r, hub.r + 16.0, p.distance(hub.c));
                h[k] += (hub.top - h[k]) * w;
            },
        );
    }
    // --- the processional way: straight and evenly graded, the round plaza level on it
    {
        let a = &plan.axis;
        let lo = a.gate.min(a.end) - Vec2::splat(a.hw + 14.0);
        let hi = a.gate.max(a.end) + Vec2::splat(a.hw + 14.0);
        each(n, cell, lo, hi, |k, p| {
            let w = 1.0 - smoothstep(a.hw + 2.0, a.hw + 12.0, a.dist(p));
            if w > 0.0 {
                h[k] += (a.height(p) - h[k]) * w;
            }
        });
        if a.plaza_r > 0.0 {
            let r = a.plaza_r + 12.0;
            let y = a.height(a.plaza);
            each(
                n,
                cell,
                a.plaza - Vec2::splat(r),
                a.plaza + Vec2::splat(r),
                |k, p| {
                    let w =
                        1.0 - smoothstep(a.plaza_r + 2.0, a.plaza_r + 10.0, p.distance(a.plaza));
                    h[k] += (y - h[k]) * w;
                },
            );
        }
    }
    // --- the entrance plaza: a raised round terrace, stairs down toward the hub
    {
        let en = &plan.entrance;
        let reach = en.r + 12.0;
        let low = g0.at(en.stair_foot);
        let head = en.c + en.f * en.r;
        each(
            n,
            cell,
            en.c - Vec2::splat(reach),
            en.c + Vec2::splat(reach),
            |k, p| {
                let d = en.r - p.distance(en.c);
                let y = if d >= 0.0 { en.top } else { en.top + d * BANK };
                let y = match ramp(p, head, en.stair_foot, 10.0, en.top, low) {
                    Some(r) => y.max(r),
                    None => y,
                };
                if d >= 0.0 {
                    h[k] = en.top;
                } else {
                    h[k] = h[k].max(y);
                }
            },
        );
    }
    // --- the courts' platforms and their stairs
    for c in plan.all_courts() {
        if c.kind == CourtKind::IslandTemple {
            continue;
        }
        platform(h, n, cell, c, g0.at(c.stair_foot));
    }
}

/// A court's platform (and the final court's upper tier) with its stairs.
fn platform(h: &mut [f32], n: usize, cell: f32, c: &Court, foot_h: f32) {
    let reach = c.reach() + (c.top - foot_h).max(0.0) / BANK + 6.0;
    let fore = c.fz > 0.0;
    // (the stairs down to the ground come off the forecourt when there is one)
    let low_top = if fore { c.ftop } else { c.top };
    each(
        n,
        cell,
        c.c - Vec2::splat(reach),
        c.c + Vec2::splat(reach),
        |k, p| {
            let dm = c.sdf_main(p);
            let df = c.sdf_fore(p);
            let mut y = if dm >= 0.0 { c.top } else { c.top + dm * BANK };
            if fore {
                let yf = if df >= 0.0 {
                    c.ftop
                } else {
                    c.ftop + df * BANK
                };
                if dm < 0.0 {
                    y = y.max(yf);
                }
                if let Some(r) = ramp(
                    p,
                    c.inner_head - c.f * 1.0,
                    c.inner_foot,
                    c.stair_w,
                    c.top,
                    c.ftop,
                ) {
                    y = y.max(r);
                }
            }
            if let Some(r) = ramp(
                p,
                c.stair_head - c.f * 1.0,
                c.stair_foot,
                c.stair_w,
                low_top,
                foot_h,
            ) {
                y = y.max(r);
            }
            if c.hx2 > 0.0 {
                let d2 = c.sdf2(p);
                let y2 = if d2 >= 0.0 {
                    c.top2
                } else {
                    c.top2 + d2 * BANK
                };
                let head2 = c.c + c.f * (c.hz2 - c.back2);
                let foot2 = head2 + c.f * ((c.top2 - c.top) * STAIR_RUN);
                let mut y2 = y2;
                if let Some(r) = ramp(p, head2 - c.f, foot2, c.stair_w, c.top2, c.top) {
                    y2 = y2.max(r);
                }
                if dm >= 0.0 {
                    y = y.max(y2);
                }
            }
            if dm >= 0.0 || df >= 0.0 {
                h[k] = y;
            } else {
                h[k] = h[k].max(y);
            }
        },
    );
}

// ------------------------------------------------------------------ after the biomes

/// What the compound needs of the map once the biomes have grown: the paving materials, the
/// lakes and the island, the paint, the nature kept off and grown thick, where players arrive.
pub fn finish(d: &mut WorldData, plan: &mut Plan) {
    let theme = plan.theme;
    for (layer, id) in theme.layer_choices() {
        d.layer_choices.set(*layer as usize, id);
    }
    // the lakes (their own water), then the island raised in its lake
    let shape = crate::water::lakes::shape_by_id(theme.lake());
    for li in 0..plan.lakes.len() {
        let outline = plan.lakes[li].outline.clone();
        let res = {
            let WorldData {
                terrain,
                history,
                store,
                water,
                lib,
                ..
            } = &mut *d;
            crate::water::create_drawn_lake(
                terrain,
                history,
                store,
                lib.as_deref(),
                water,
                &outline,
                shape,
                1.0,
                if plan.lakes[li].island.is_some() {
                    0.6
                } else {
                    1.0
                },
            )
        };
        match res {
            Ok(id) => {
                let name = plan.lakes[li].name.clone();
                if let Some(l) = d.water.lakes.iter_mut().find(|l| l.id == id) {
                    l.name = name;
                }
                let level = d.water.lakes.iter().find(|l| l.id == id).map(|l| l.level);
                if let (Some(ci), Some(level)) = (plan.lakes[li].island, level) {
                    island(&mut d.terrain, &mut plan.courts[ci], level);
                }
            }
            Err(e) => warn!("compound lake: {e}"),
        }
    }
    paint(&mut d.terrain, plan);
    nature(d, plan);
    // players arrive on the entrance plaza facing into the compound (the way out behind them)
    let s = plan.entrance.start;
    let y = d.terrain.height_at(s.x, s.y);
    let f = plan.entrance.f;
    // (a player start's yaw faces along (cos, −sin))
    let yaw = (-f.y).atan2(f.x);
    d.gameplay.player_start = Some(crate::gameplay::PlayerStart {
        pos: [s.x, y, s.y],
        yaw,
    });
    d.gameplay.dirty = true;
    // the view: over the entrance, looking up the compound toward the hub
    let mid = plan.entrance.c.lerp(plan.hub.c, 0.55);
    let look = plan.hub.c - plan.entrance.c;
    d.orbit.set_view(
        Vec3::new(mid.x, plan.floor, mid.y),
        look.x.atan2(look.y) + std::f32::consts::PI,
        0.75,
        (plan.hub.c.distance(plan.entrance.c) * 1.6).max(220.0),
    );
    d.orbit.snap();
}

/// The island in its lake: ground a little over the water, the temple's platform on it.
fn island(t: &mut Terrain, c: &mut Court, level: f32) {
    let (n, cell) = (t.n(), t.cell);
    c.top = level + c.kind.rise();
    let ground = level + 0.8;
    // (its stairs come down onto the island's shore)
    let shore =
        c.hx.max(c.stair_head.distance(c.c) + c.stair_head.distance(c.stair_foot)) + 2.5;
    let foot_h = ground;
    let reach = shore + 12.0;
    let mut hs = std::mem::take(&mut t.heights);
    each(
        n,
        cell,
        c.c - Vec2::splat(reach),
        c.c + Vec2::splat(reach),
        |k, p| {
            let r = p.distance(c.c);
            let isle = if r <= shore {
                ground
            } else {
                ground - (r - shore) * 0.45
            };
            hs[k] = hs[k].max(isle);
        },
    );
    t.heights = hs;
    let cc = c.clone();
    let mut hs = std::mem::take(&mut t.heights);
    platform(&mut hs, n, cell, &cc, foot_h);
    t.heights = hs;
    if let Some(r) = t.rect_for_circle(c.c, reach) {
        t.mark_heights_dirty(r);
    }
}

/// Paints a layer at full strength (`w` 0..1 blends it over what is there).
pub(crate) fn put(t: &mut Terrain, k: usize, layer: Layer, w: f32) {
    if w <= 0.0 {
        return;
    }
    let w = w.min(1.0);
    let s = &mut t.splat[k];
    for (i, v) in s.iter_mut().enumerate() {
        let target = if i == layer as usize { 255.0 } else { 0.0 };
        *v = (*v as f32 + (target - *v as f32) * w)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

/// Each court's floor its own colour, its banks stone, its stairs paved; the plaza, the axis and
/// the final terrace paved; the hub's ring.
fn paint(t: &mut Terrain, plan: &Plan) {
    let (n, cell) = (t.n(), t.cell);
    let pave = plan.theme.paving();
    let half = t.half();
    let mut dirty: Option<GridRect> = None;
    let mut mark = |t: &Terrain, c: Vec2, r: f32| {
        if let Some(rc) = t.rect_for_circle(c, r) {
            dirty = Some(match dirty {
                Some(d) => d.union(rc),
                None => rc,
            });
        }
    };
    // the plaza
    {
        let en = &plan.entrance;
        let reach = en.r + 16.0;
        let head = en.c + en.f * en.r;
        let mut ks = vec![];
        each(
            n,
            cell,
            en.c - Vec2::splat(reach),
            en.c + Vec2::splat(reach),
            |k, p| ks.push((k, p)),
        );
        for (k, p) in ks {
            let d = en.r - p.distance(en.c);
            put(t, k, pave, smoothstep(-1.5, 0.5, d));
            if geom_band(p, head, en.stair_foot) < 5.0 {
                put(t, k, pave, 1.0);
            }
            if d < 0.0 && d > -en.r * 0.1 - 2.5 {
                put(t, k, Layer::Rock, 0.8);
            }
        }
        mark(t, en.c, reach);
    }
    // the hub: a paved plaza, a ring of beaten earth round it
    {
        let hub = &plan.hub;
        let mut ks = vec![];
        each(
            n,
            cell,
            hub.c - Vec2::splat(hub.r + 6.0),
            hub.c + Vec2::splat(hub.r + 6.0),
            |k, p| ks.push((k, p)),
        );
        for (k, p) in ks {
            let r = p.distance(hub.c);
            put(
                t,
                k,
                Layer::Dirt,
                1.0 - smoothstep(hub.r * 0.95, hub.r + 5.0, r),
            );
            put(t, k, pave, 1.0 - smoothstep(hub.r * 0.88, hub.r * 0.93, r));
        }
        mark(t, hub.c, hub.r + 6.0);
    }
    // the axis and its round plaza
    {
        let a = &plan.axis;
        let lo = a.gate.min(a.end) - Vec2::splat(a.hw + 4.0);
        let hi = a.gate.max(a.end) + Vec2::splat(a.hw + 4.0);
        let mut ks = vec![];
        each(n, cell, lo, hi, |k, p| ks.push((k, p)));
        for (k, p) in ks {
            put(
                t,
                k,
                pave,
                1.0 - smoothstep(a.hw + 0.5, a.hw + 2.5, a.dist(p)),
            );
            if a.plaza_r > 0.0 {
                put(
                    t,
                    k,
                    pave,
                    1.0 - smoothstep(a.plaza_r - 0.5, a.plaza_r + 1.5, p.distance(a.plaza)),
                );
            }
        }
        let mid = (a.gate + a.end) * 0.5;
        mark(t, mid, a.len() * 0.5 + a.hw + 6.0);
    }
    // the courts
    for c in plan.all_courts() {
        let reach = c.reach() + 12.0 + c.stair_head.distance(c.stair_foot);
        let floor_layer = if c.kind == CourtKind::ThroneTerrace {
            pave
        } else {
            c.floor
        };
        let mut ks = vec![];
        each(
            n,
            cell,
            c.c - Vec2::splat(reach),
            c.c + Vec2::splat(reach),
            |k, p| ks.push((k, p)),
        );
        for (k, p) in ks {
            let d = c.sdf(p);
            let dm = c.sdf_main(p);
            let bank = ((c.top - t.heights[k]).max(0.0) / BANK + 1.5).min(9.0);
            if dm >= -1.0 {
                put(t, k, floor_layer, smoothstep(-1.0, 0.5, dm));
            } else if d >= -1.0 {
                // the forecourt paved
                put(t, k, pave, smoothstep(-1.0, 0.5, d));
            } else if d > -bank {
                put(t, k, Layer::Rock, 1.0);
            }
            if c.fz > 0.0
                && dm < -1.0
                && d >= -1.0
                && c.sdf_main(p) > -bank
                && t.heights[k] > c.ftop + 0.5
            {
                // (the main terrace's bank above the forecourt: stone)
                put(t, k, Layer::Rock, 1.0);
            }
            if c.fz > 0.0 && geom_band(p, c.inner_head, c.inner_foot) < c.stair_w * 0.5 + 0.5 {
                put(t, k, pave, 1.0);
            }
            if c.hx2 > 0.0 && c.sdf2(p) > -1.0 {
                put(
                    t,
                    k,
                    if plan.theme == Theme::Frozen {
                        Layer::Rock
                    } else {
                        Layer::Ice
                    },
                    smoothstep(-1.0, 0.5, c.sdf2(p)),
                );
            }
            if geom_band(p, c.stair_head, c.stair_foot) < c.stair_w * 0.5 + 0.5 {
                put(t, k, pave, 1.0);
            }
        }
        mark(t, c.c, reach);
    }
    let _ = half;
    if let Some(r) = dirty {
        t.mark_splat_dirty(r);
    }
}

/// Distance from `p` to the segment `a`–`b`, only alongside it (f32::MAX before or past it).
pub(crate) fn geom_band(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared().max(1e-6);
    let u = (p - a).dot(ab) / l2;
    if !(-0.02..=1.02).contains(&u) {
        return f32::MAX;
    }
    (a + ab * u.clamp(0.0, 1.0)).distance(p)
}

/// Trees, rocks and plants kept off the courts, the plaza, the hub and the axis; grown thick in
/// the clumps between them.
fn nature(d: &mut WorldData, plan: &Plan) {
    let half = d.terrain.half();
    let courts: Vec<Court> = plan.all_courts().cloned().collect();
    let en = plan.entrance.clone();
    let hub = plan.hub.clone();
    let axis = plan.axis.clone();
    let clumps = plan.clumps.clone();
    let wall = plan.entrance.wall.clone();
    let lakes: Vec<(Vec2, f32)> = plan.lakes.iter().map(|l| (l.c, l.r)).collect();
    let camps: Vec<(Vec2, f32)> = plan.camps.iter().map(|c| (c.c, c.r)).collect();
    let bounds: Vec<(Vec2, f32)> = courts.iter().map(|c| c.bound()).collect();
    let district = plan.district.clone();
    let room = plan.room.clone();
    d.nature
        .set_texels(Vec2::splat(-half), Vec2::splat(half), move |p| {
            // how cleared (1: bare) and how thick (> 1)
            let mut clear = 0.0f32;
            for c in &courts {
                clear = clear.max(smoothstep(-9.0, -3.0, c.sdf(p)));
                clear = clear.max(
                    1.0 - smoothstep(
                        c.stair_w * 0.5 + 1.0,
                        c.stair_w * 0.5 + 4.0,
                        geom_band(p, c.stair_head, c.stair_foot),
                    ),
                );
            }
            clear = clear.max(1.0 - smoothstep(en.r + 3.0, en.r + 9.0, p.distance(en.c)));
            clear = clear.max(1.0 - smoothstep(hub.r + 2.0, hub.r + 8.0, p.distance(hub.c)));
            clear = clear.max(1.0 - smoothstep(axis.hw + 3.0, axis.hw + 9.0, axis.dist(p)));
            if axis.plaza_r > 0.0 {
                clear = clear.max(
                    1.0 - smoothstep(
                        axis.plaza_r + 2.0,
                        axis.plaza_r + 8.0,
                        p.distance(axis.plaza),
                    ),
                );
            }
            if wall.len() >= 2 {
                clear = clear
                    .max(1.0 - smoothstep(5.0, 9.0, super::super::geom::polyline_dist(&wall, p).0));
            }
            for (c, r) in &camps {
                clear = clear.max(1.0 - smoothstep(r + 1.0, r + 5.0, p.distance(*c)));
            }
            for (c, r) in &lakes {
                clear = clear.max(0.6 * (1.0 - smoothstep(r * 0.9, r + 2.0, p.distance(*c))));
            }
            let mut thick = 0.0f32;
            for (c, r) in &clumps {
                thick = thick.max(1.0 - smoothstep(r * 0.6, *r, p.distance(*c)));
            }
            let keep = 1.0 - clear;
            // open ground: grass clearings round the courts and along the axis (the trees go, the
            // grass and flowers stay), the floor's trees halved, the district's thinned, the jungle
            // thick in its clumps and close under the walls
            let mut glade = 0.0f32;
            for (c, r) in &bounds {
                glade = glade.max(1.0 - smoothstep(r + 6.0, r + 22.0, p.distance(*c)));
            }
            glade = glade.max(1.0 - smoothstep(axis.hw + 8.0, axis.hw + 26.0, axis.dist(p)));
            glade = glade.max(1.0 - smoothstep(hub.r + 6.0, hub.r + 22.0, p.distance(hub.c)));
            for (c, r) in &district {
                glade = glade.max(0.6 * (1.0 - smoothstep(r * 0.7, *r, p.distance(*c))));
            }
            let wall = 1.0 - smoothstep(8.0, 55.0, room.at(p));
            let base = 0.5 + 1.1 * wall;
            let trees = keep * (base * (1.0 - glade) + 2.6 * thick).max(0.0);
            Some((
                [
                    trees,
                    keep * (1.0 + 0.3 * thick),
                    keep * (1.0 + 0.8 * thick),
                    keep,
                ],
                0,
            ))
        });
}
