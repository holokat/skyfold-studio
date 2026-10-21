//! Building a compound out of the editor's own tools, a step at a time: its trails, bridges and
//! the processional way (`roads_create`), every court's floor, temple, pillars, braziers,
//! statues and trophies, the perimeter wall and its towers, the gates, the ruins and the camps
//! (`objects_place`, the hand-painted pack), and its creatures, camps, bosses and boss arenas
//! (`gameplay_*`). Everything is ordinary content: move it, change it, delete it.

use bevy::math::Vec2;
use serde_json::{Value, json};

use super::super::geom;
use super::layout::{ARM, Court, CourtKind, Plan, TrailKind, side, yaw_along, yaw_facing};
use super::theme::{A, Kit};

/// One step of the build.
pub enum Step {
    Call {
        label: String,
        tool: &'static str,
        args: Value,
    },
    /// let the world settle (water levels, road beds) for a few frames
    Wait(u32),
}

pub(crate) fn call(label: impl Into<String>, tool: &'static str, args: Value) -> Step {
    Step::Call {
        label: label.into(),
        tool,
        args,
    }
}

/// An object on the ground at `p`, its front (+Z) facing `yaw_deg`.
pub(crate) fn obj(a: A, p: Vec2, yaw_deg: f32, scale: f32) -> Value {
    json!({ "asset": format!("hp:{}", a.0), "position": [r2(p.x), r2(p.y)], "yaw_deg": r2(yaw_deg), "scale": r2(a.1 * scale) })
}

/// An object at an exact height (it doesn't level the ground).
pub(crate) fn obj_at(a: A, p: Vec2, y: f32, yaw_deg: f32, scale: f32) -> Value {
    json!({ "asset": format!("hp:{}", a.0), "position": [r2(p.x), r2(y), r2(p.y)], "yaw_deg": r2(yaw_deg), "scale": r2(a.1 * scale), "fit_ground": false })
}

pub(crate) fn r2(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

pub(crate) fn place(label: impl Into<String>, items: Vec<Value>) -> Option<Step> {
    (!items.is_empty()).then(|| call(label, "objects_place", json!({ "items": items })))
}

/// Control points for a road along a trail (one every ~`every` m, both ends kept).
pub(crate) fn controls(pts: &[Vec2], every: f32) -> Vec<[f32; 2]> {
    let len = geom::polyline_len(pts);
    let n = ((len / every).round() as usize).max(1);
    (0..=n)
        .map(|i| geom::polyline_at(pts, len * i as f32 / n as f32).0)
        .map(|p| [r2(p.x), r2(p.y)])
        .collect()
}

/// Every step that builds the plan, in order.
pub fn steps(plan: &Plan) -> Vec<Step> {
    let kit = plan.theme.kit();
    let mut out = vec![Step::Wait(12)];
    // --- the trails, the bridges, the processional way
    for t in &plan.trails {
        let (main, spoke) = plan.theme.trail_kinds();
        let (kind, every) = match t.kind {
            TrailKind::Main => (main, 14.0),
            TrailKind::Spoke => (spoke, 12.0),
            TrailKind::Bridge => ("cobblestone", 100.0),
            TrailKind::Axis => ("paved", 1000.0),
        };
        let pts = if matches!(t.kind, TrailKind::Bridge | TrailKind::Axis) {
            t.pts.iter().map(|p| [r2(p.x), r2(p.y)]).collect()
        } else {
            controls(&t.pts, every)
        };
        // (built wider than the layout's own measure: 4-6 m trails read from afar)
        let width = match t.kind {
            TrailKind::Main => (t.width * 1.25).clamp(5.0, 7.0),
            TrailKind::Spoke => (t.width * 1.3).clamp(4.0, 6.0),
            TrailKind::Bridge => (t.width * 1.1).clamp(4.5, 6.0),
            TrailKind::Axis => t.width,
        };
        out.push(call(t.name.clone(), "roads_create", json!({ "kind": kind, "points": pts, "width": r2(width), "name": t.name, "options": { "props": false } })));
    }
    out.push(Step::Wait(4));
    // --- the entrance: the perimeter wall and its towers, the gatehouse behind the way out, the
    // plaza's braziers and the arch over its stairs
    out.extend(place(
        "The perimeter wall along the open edge",
        wall(plan, kit),
    ));
    out.extend(place("The entrance plaza", entrance(plan, kit)));
    out.extend(place(format!("{}", plan.hub.name), hub(plan, kit)));
    out.extend(place(
        "The gate, the processional way and the round plaza",
        axis(plan, kit),
    ));
    for c in &plan.courts {
        out.extend(place(
            format!("{} ({})", c.name, c.kind.name().to_lowercase()),
            court(c, kit, plan.grand),
        ));
    }
    out.extend(place(
        plan.final_court.name.clone(),
        final_court(&plan.final_court, kit, plan.grand),
    ));
    let ruins: Vec<Value> = plan
        .ruins
        .iter()
        .map(|r| {
            let mut o = obj((r.asset, r.scale), r.p, r.yaw_deg, 1.0);
            if let Some(s3) = r.scale3 {
                o["scale"] = json!([r2(s3[0]), r2(s3[1]), r2(s3[2])]);
            }
            o
        })
        .collect();
    out.extend(place(
        "Ruins along the trails and the outer temple district",
        ruins,
    ));
    out.extend(place("Camps along the trails", camps(plan, kit)));
    // --- who lives there
    out.extend(gameplay(plan, kit));
    out
}

/// The perimeter wall: sections along the open edge, towers at every few joints and at its ends.
fn wall(plan: &Plan, kit: &Kit) -> Vec<Value> {
    let w = &plan.entrance.wall;
    let len = geom::polyline_len(w);
    if w.len() < 2 || len < 30.0 {
        return vec![];
    }
    let seg = 12.0 * kit.wall.1;
    let n = ((len / seg).round() as usize).max(1);
    let step = len / n as f32;
    let en = &plan.entrance;
    // (the gatehouse stands in the wall right behind the way out)
    let gate_at = geom::polyline_dist(w, en.c - en.f * en.r).1;
    let mut items = vec![];
    let (gp, gd) = geom::polyline_at(w, gate_at);
    // its front faces into the compound
    let into = if side(gd).dot(en.f) > 0.0 {
        side(gd)
    } else {
        -side(gd)
    };
    items.push(obj(kit.outer_gate, gp, yaw_facing(into), 1.0));
    for i in 0..n {
        let (a, _) = geom::polyline_at(w, step * i as f32);
        let (b, _) = geom::polyline_at(w, step * (i + 1) as f32);
        let m = (a + b) * 0.5;
        if (step * (i as f32 + 0.5) - gate_at).abs() < 10.0 {
            continue;
        }
        let d = (b - a).normalize_or(gd);
        items.push(json!({ "asset": format!("hp:{}", kit.wall.0), "position": [r2(m.x), r2(m.y)], "yaw_deg": r2(yaw_along(d)), "scale": [r2(a.distance(b) / 12.0 * 1.02), 1.0, 1.0] }));
        if i == 0 || (i % 4 == 0 && (step * i as f32 - gate_at).abs() > 14.0) {
            items.push(obj(kit.tower, a, yaw_facing(into), 1.0));
        }
    }
    let (end, _) = geom::polyline_at(w, len);
    items.push(obj(kit.tower, end, yaw_facing(into), 1.0));
    items
}

fn entrance(plan: &Plan, kit: &Kit) -> Vec<Value> {
    let en = &plan.entrance;
    let s = side(en.f);
    let mut v = vec![];
    let head = en.c + en.f * en.r;
    v.push(obj(
        kit.entrance_arch,
        head - en.f * 1.5,
        yaw_facing(en.f),
        1.0,
    ));
    v.extend(stair_rails(head, en.stair_foot, 10.0, kit));
    for sg in [-1.0f32, 1.0] {
        for a in [50f32, 120.0] {
            let d = geom::rot(en.f, sg * a.to_radians());
            v.push(obj_at(
                kit.brazier,
                en.c + d * (en.r - 2.0),
                en.top,
                yaw_facing(-d),
                1.0,
            ));
        }
        v.push(obj(
            kit.statue,
            en.stair_foot + s * sg * 7.0 - en.f * 1.0,
            yaw_facing(en.f),
            1.0,
        ));
    }
    v
}

/// The hub: a paved plaza with its monument, a low wall round it broken where the trails and the
/// gate come in, braziers and statues.
fn hub(plan: &Plan, kit: &Kit) -> Vec<Value> {
    let h = &plan.hub;
    let mut v = vec![obj(
        kit.hub_center,
        h.c,
        yaw_facing(plan.axis.dir),
        plan.grand * 1.5,
    )];
    let r = h.r * 0.9;
    // where the trails and the processional way come in
    let mut ways: Vec<(Vec2, f32)> = plan
        .trails
        .iter()
        .flat_map(|t| [t.pts[0], t.pts[t.pts.len() - 1]].map(|p| (p, t.width)))
        .filter(|(p, _)| (p.distance(h.c) - h.r * 0.92).abs() < 4.0)
        .map(|(p, w)| ((p - h.c).normalize_or(Vec2::X), w))
        .collect();
    ways.push((plan.axis.dir, plan.axis.hw * 2.0));
    let seg = 2.4 * RIM.1;
    let k = ((std::f32::consts::TAU * r) / seg).round() as usize;
    for i in 0..k {
        let a0 = i as f32 / k as f32 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / k as f32 * std::f32::consts::TAU;
        let (p0, p1) = (
            h.c + Vec2::from_angle(a0) * r,
            h.c + Vec2::from_angle(a1) * r,
        );
        let m = (p0 + p1) * 0.5;
        let d = (m - h.c) / r;
        if ways
            .iter()
            .any(|(w, width)| d.dot(*w) > ((width * 0.5 + 3.0) / r).cos())
        {
            continue;
        }
        v.push(json!({ "asset": format!("hp:{}", RIM.0), "position": [r2(m.x), r2(h.top), r2(m.y)], "yaw_deg": r2(yaw_along(p1 - p0)), "scale": [r2(RIM.1 * p0.distance(p1) / seg * 1.04), r2(RIM.1 * 1.2), r2(RIM.1)], "fit_ground": false }));
    }
    for i in 0..8 {
        let d = Vec2::from_angle(i as f32 / 8.0 * std::f32::consts::TAU + 0.2);
        if ways.iter().any(|(w, _)| d.dot(*w) > 0.93) {
            continue;
        }
        v.push(obj(
            if i % 2 == 0 {
                kit.tall_brazier
            } else {
                kit.statue
            },
            h.c + d * (r - 3.0),
            yaw_facing(-d),
            1.0,
        ));
        v.push(obj(kit.brazier, h.c + d * h.r * 0.42, yaw_facing(-d), 1.0));
    }
    v
}

fn axis(plan: &Plan, kit: &Kit) -> Vec<Value> {
    let a = &plan.axis;
    let s = side(a.dir);
    let mut v = vec![obj(
        kit.gate,
        a.gate + a.dir * 2.0,
        yaw_facing(a.dir),
        plan.grand,
    )];
    let len = a.len();
    let mut t = 14.0;
    let mut k = 0;
    // (grand: tall pillars two by two up the way, fire between them, statues at the plaza)
    while t < len - 6.0 {
        let p = a.gate + a.dir * t;
        if a.plaza_r <= 0.0 || p.distance(a.plaza) > a.plaza_r + 5.0 {
            for sg in [-1.0f32, 1.0] {
                let q = p + s * sg * (a.hw + 1.8);
                if k % 2 == 1 {
                    v.push(obj(kit.tall_brazier, q, yaw_facing(-s * sg), 1.0));
                } else {
                    v.push(obj(kit.pillar, q, yaw_facing(-s * sg), 1.5));
                }
            }
            k += 1;
        }
        t += 12.0;
    }
    if a.plaza_r > 0.0 {
        v.push(obj(
            kit.plaza_center,
            a.plaza,
            yaw_facing(a.dir),
            plan.grand,
        ));
        for sg in [-1.0f32, 1.0] {
            for e in [-1.0f32, 1.0] {
                v.push(obj(
                    kit.statue,
                    a.plaza + a.dir * e * (a.plaza_r + 2.5) + s * sg * (a.hw + 1.8),
                    yaw_facing(s * -sg),
                    1.3,
                ));
            }
        }
        for i in 0..8 {
            let ang = i as f32 / 8.0 * std::f32::consts::TAU + std::f32::consts::FRAC_PI_8;
            let d = geom::rot(a.dir, ang);
            // (the way runs through: none on it)
            if d.dot(a.dir).abs() > 0.9 {
                continue;
            }
            v.push(obj(
                kit.pillar,
                a.plaza + d * (a.plaza_r - 1.2),
                yaw_facing(-d),
                1.0,
            ));
            if i % 2 == 0 {
                v.push(obj(
                    kit.brazier,
                    a.plaza + d * (a.plaza_r - 4.5),
                    yaw_facing(-d),
                    1.0,
                ));
            }
        }
    }
    v
}

/// The low wall round a platform's rim (not across its stairs).
pub(crate) const RIM: A = ("stone_wall", 1.25);

/// A court's outline in its own frame (x across, z toward its stairs), corner by corner.
fn outline(c: &Court) -> Vec<Vec2> {
    let (hx, hz) = (c.hx, c.hz);
    match c.kind {
        CourtKind::RoundShrine | CourtKind::IslandTemple => (0..36)
            .map(|i| Vec2::from_angle(i as f32 / 36.0 * std::f32::consts::TAU) * hx)
            .collect(),
        CourtKind::Octagon => {
            let r = hx / (std::f32::consts::PI / 8.0).cos();
            (0..8)
                .map(|i| Vec2::from_angle((i as f32 + 0.5) / 8.0 * std::f32::consts::TAU) * r)
                .collect()
        }
        CourtKind::CrossYard => {
            let a = hx * ARM;
            vec![
                Vec2::new(a, hz),
                Vec2::new(a, a),
                Vec2::new(hx, a),
                Vec2::new(hx, -a),
                Vec2::new(a, -a),
                Vec2::new(a, -hz),
                Vec2::new(-a, -hz),
                Vec2::new(-a, -a),
                Vec2::new(-hx, -a),
                Vec2::new(-hx, a),
                Vec2::new(-a, a),
                Vec2::new(-a, hz),
            ]
        }
        _ => vec![
            Vec2::new(hx, hz),
            Vec2::new(hx, -hz),
            Vec2::new(-hx, -hz),
            Vec2::new(-hx, hz),
        ],
    }
}

/// Low wall sections round the platform's edge, `inset` m in, leaving the stairs open.
/// Wall sections round an outline in a court's frame, `inset` m in: none across the stairs (on the
/// edge at `front` along the facing, `gap` either side), none on the edges `skip_edge` says, and a
/// few broken out (the same each time: by where they stand).
fn rim(
    c: &Court,
    corners: &[Vec2],
    y: f32,
    inset: f32,
    front: f32,
    gap: f32,
    skip_edge: &dyn Fn(Vec2, Vec2) -> bool,
) -> Vec<Value> {
    let mut v = vec![];
    let seg = 2.4 * RIM.1;
    let n = corners.len();
    let centroid = corners.iter().copied().sum::<Vec2>() / n as f32;
    for i in 0..n {
        let (a, b) = (corners[i], corners[(i + 1) % n]);
        if skip_edge(a, b) {
            continue;
        }
        let len = a.distance(b);
        let k = (len / seg).floor().max(1.0) as usize;
        let d = (b - a) / len;
        for j in 0..k {
            let m = a + d * (len * (j as f32 + 0.5) / k as f32);
            // (in toward the middle)
            let nrm = side(d);
            let inward = if nrm.dot(centroid - m) > 0.0 {
                nrm
            } else {
                -nrm
            };
            let q = m + inward * inset;
            if q.y > front - 3.0 && q.x.abs() < gap {
                continue;
            }
            let w = c.at(q.x, q.y);
            // (broken: one in nine gone, the ends of a run kept)
            let hsh = crate::noise::hash_u32(
                (w.x * 7.0) as i32 as u32 ^ ((w.y * 13.0) as i32 as u32).rotate_left(16),
            );
            if j > 0 && j + 1 < k && hsh % 9 == 0 {
                continue;
            }
            let dir = side(c.f) * d.x + c.f * d.y;
            let tall = if hsh % 5 == 1 { 1.0 } else { 1.6 };
            v.push(json!({ "asset": format!("hp:{}", RIM.0), "position": [r2(w.x), r2(y), r2(w.y)], "yaw_deg": r2(yaw_along(dir)), "scale": [r2(RIM.1 * len / k as f32 / seg * 1.03), r2(RIM.1 * tall), r2(RIM.1 * 1.2)], "fit_ground": false }));
        }
    }
    v
}

/// Low walls down both sides of a stair (tilted with it), pillars at its foot.
pub(crate) fn stair_rails(head: Vec2, foot: Vec2, w: f32, kit: &Kit) -> Vec<Value> {
    let len = head.distance(foot);
    if len < 2.0 {
        return vec![];
    }
    let d = (foot - head) / len;
    let s = side(d);
    let seg = 2.4 * RIM.1;
    let k = (len / seg).round().max(1.0) as usize;
    let mut v = vec![];
    for sg in [-1.0f32, 1.0] {
        for j in 0..k {
            let p = head + d * (len * (j as f32 + 0.5) / k as f32) + s * sg * (w * 0.5 + 0.35);
            v.push(json!({ "asset": format!("hp:{}", RIM.0), "position": [r2(p.x), r2(p.y)], "yaw_deg": r2(yaw_along(d)), "scale": [r2(RIM.1 * len / k as f32 / seg * 1.04), r2(RIM.1), r2(RIM.1)], "align": true, "fit_ground": false }));
        }
        v.push(obj(
            kit.pillar,
            foot + d * 0.6 + s * sg * (w * 0.5 + 0.6),
            yaw_facing(d),
            1.0,
        ));
    }
    v
}

/// A court's floor, centrepiece, rim wall, pillars, braziers and trophies, by its shape. `g`:
/// how much bigger its floor and temple are made.
fn court(c: &Court, kit: &Kit, g: f32) -> Vec<Value> {
    let f = yaw_facing(c.f);
    let back = yaw_facing(-c.f);
    let mut v = vec![];
    let at = |x: f32, z: f32| c.at(x, z);
    // the enclosure: walls round the main terrace (open at its stairs, a few broken out), tall
    // braziers either side of its stair head
    let gap = c.stair_w * 0.5 + 1.0;
    let no_edge = |_: Vec2, _: Vec2| false;
    v.extend(rim(c, &outline(c), c.top, 0.8, c.edge(), gap, &no_edge));
    let flank = c.stair_w * 0.5 + 1.8;
    for sg in [-1.0f32, 1.0] {
        v.push(obj_at(
            kit.tall_brazier,
            at(sg * flank, c.edge() - 1.6),
            c.top,
            f,
            1.0,
        ));
    }
    v.extend(stair_rails(c.inner_head, c.inner_foot, c.stair_w, kit));
    if c.fz > 0.0 {
        // the forecourt: walled on three sides (its back is the main terrace's bank), its own
        // stairs down to the trail, pavilions on its front corners, braziers and statues
        let (fz0, fz1) = (c.fore_z() - c.fz, c.fore_z() + c.fz);
        let fore = [
            Vec2::new(c.fx, fz1),
            Vec2::new(c.fx, fz0),
            Vec2::new(-c.fx, fz0),
            Vec2::new(-c.fx, fz1),
        ];
        let back_edge = |a: Vec2, b: Vec2| (a.y - fz0).abs() < 0.1 && (b.y - fz0).abs() < 0.1;
        v.extend(rim(c, &fore, c.ftop, 0.8, fz1, gap, &back_edge));
        v.extend(stair_rails(c.stair_head, c.stair_foot, c.stair_w, kit));
        let pr = 5.0 * kit.pavilion.1;
        for sg in [-1.0f32, 1.0] {
            v.push(obj(
                kit.pavilion,
                at(sg * (c.fx - pr - 1.5), fz1 - pr - 1.5),
                f,
                1.0,
            ));
            v.push(obj_at(
                kit.tall_brazier,
                at(sg * flank, fz1 - 1.6),
                c.ftop,
                f,
                1.0,
            ));
            v.push(obj_at(
                kit.statue,
                at(sg * (c.fx - 2.5), fz0 + 3.0),
                c.ftop,
                yaw_facing(-side(c.f) * sg),
                1.1,
            ));
            v.push(obj_at(
                kit.brazier,
                at(sg * (c.stair_w * 0.5 + 6.0), c.fore_z()),
                c.ftop,
                f,
                1.0,
            ));
        }
    } else {
        v.extend(stair_rails(c.stair_head, c.stair_foot, c.stair_w, kit));
    }
    match c.kind {
        CourtKind::CrossYard => {
            v.push(obj((kit.cross_floor, 1.0), c.c, f, g));
            v.push(obj(kit.cross_back, at(0.0, -(c.hz - 5.5 * g)), f, g));
            let arm = c.hx * ARM;
            for (x, z) in [
                (arm - 1.8, -(c.hz - 1.8)),
                (-(arm - 1.8), -(c.hz - 1.8)),
                (c.hx - 1.8, arm - 1.8),
                (c.hx - 1.8, -(arm - 1.8)),
                (-(c.hx - 1.8), arm - 1.8),
                (-(c.hx - 1.8), -(arm - 1.8)),
                (arm - 1.8, arm + 2.0),
                (-(arm - 1.8), arm + 2.0),
            ] {
                v.push(obj_at(kit.pillar, at(x, z), c.top, f, 1.0));
            }
            for sg in [-1.0f32, 1.0] {
                v.push(obj_at(
                    kit.brazier,
                    at(sg * (c.hx - 3.5), 0.0),
                    c.top,
                    yaw_facing(-side(c.f) * sg),
                    1.0,
                ));
                v.push(obj_at(
                    kit.statue,
                    at(sg * (arm - 3.0), -(c.hz - 3.0)),
                    c.top,
                    f,
                    1.0,
                ));
            }
            v.push(obj_at(
                kit.trophy,
                at(c.hx - 6.0, 0.0),
                c.top,
                yaw_facing(-side(c.f)),
                0.8,
            ));
        }
        CourtKind::PillaredHall => {
            v.push(obj((kit.hall_floor, 1.0), at(0.0, 3.0), f, g));
            let mut z = -c.hz + 3.0;
            while z < c.hz - 3.0 {
                for sg in [-1.0f32, 1.0] {
                    v.push(obj_at(kit.pillar, at(sg * (c.hx - 2.2), z), c.top, f, 1.0));
                }
                z += 5.5;
            }
            v.push(obj_at(kit.trophy, at(0.0, -(c.hz - 4.5)), c.top, f, 1.0));
            for sg in [-1.0f32, 1.0] {
                v.push(obj_at(
                    kit.statue,
                    at(sg * (c.hx - 6.0), -(c.hz - 3.0)),
                    c.top,
                    f,
                    1.0,
                ));
                v.push(obj_at(
                    kit.brazier,
                    at(sg * 6.0, -(c.hz - 3.5)),
                    c.top,
                    f,
                    1.0,
                ));
            }
        }
        CourtKind::SteppedPyramid => {
            v.push(obj(kit.pyramid, at(0.0, -(c.hz - 13.5 * g)), f, g * 1.3));
            v.push(obj(
                (kit.pyramid_floor, 1.0),
                at(0.0, c.hz - 14.0 * g),
                f,
                g,
            ));
            for sg in [-1.0f32, 1.0] {
                for z in [-(c.hz - 2.0), -(c.hz * 0.35), c.hz * 0.1] {
                    v.push(obj_at(kit.pillar, at(sg * (c.hx - 2.0), z), c.top, f, 1.0));
                }
                v.push(obj_at(
                    kit.statue,
                    at(sg * (c.hx - 3.0), c.hz - 6.0),
                    c.top,
                    f,
                    1.0,
                ));
                v.push(obj_at(
                    kit.brazier,
                    at(sg * 13.0 * g, -(c.hz - 24.0 * g)),
                    c.top,
                    f,
                    1.0,
                ));
            }
        }
        CourtKind::RoundShrine | CourtKind::Octagon => {
            let round = c.kind == CourtKind::RoundShrine;
            v.push(obj(
                (
                    (if round {
                        kit.round_floor
                    } else {
                        kit.oct_floor
                    }),
                    1.0,
                ),
                c.c,
                f,
                g,
            ));
            v.push(obj(
                if round { kit.round_back } else { kit.oct_back },
                at(0.0, -(c.hx - 6.0 * g)),
                f,
                g,
            ));
            let n = 12;
            for i in 0..n {
                let a = (i as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
                let d = geom::rot(c.f, a);
                // (none on the stairs, nor behind the shrine)
                if d.dot(c.f) > 0.86 || d.dot(c.f) < -0.9 {
                    continue;
                }
                v.push(obj_at(
                    if i % 2 == 0 { kit.pillar } else { kit.brazier },
                    c.c + d * (c.hx - 2.4),
                    c.top,
                    yaw_facing(-d),
                    1.0,
                ));
            }
            if !round {
                v.push(obj_at(
                    kit.trophy,
                    at(c.hx - 7.0, -(c.hx - 8.0)),
                    c.top,
                    back,
                    0.8,
                ));
            }
        }
        CourtKind::IslandTemple => {
            v.push(obj(kit.island_temple, at(0.0, -1.0), f, g * 1.1));
            for i in 0..10 {
                let a = (i as f32 + 0.5) / 10.0 * std::f32::consts::TAU;
                let d = geom::rot(c.f, a);
                if d.dot(c.f) > 0.82 {
                    continue;
                }
                v.push(obj_at(
                    if i % 2 == 0 { kit.pillar } else { kit.brazier },
                    c.c + d * (c.hx - 2.0),
                    c.top,
                    yaw_facing(-d),
                    1.0,
                ));
            }
        }
        CourtKind::ThroneTerrace => {}
    }
    v
}

/// The final court: pillars down both sides of its fighting floor, the throne at the head of
/// the upper tier's stairs, what towers behind it, braziers and statues.
fn final_court(c: &Court, kit: &Kit, g: f32) -> Vec<Value> {
    let f = yaw_facing(c.f);
    let mut v = vec![];
    let at = |x: f32, z: f32| c.at(x, z);
    let up_front = c.hz2 - c.back2;
    // the rim walls: round the lower tier (open at its stairs) and along the upper tier's front
    let no_edge = |_: Vec2, _: Vec2| false;
    v.extend(rim(
        c,
        &outline(c),
        c.top,
        0.8,
        c.edge(),
        c.stair_w * 0.5 + 1.0,
        &no_edge,
    ));
    let upper = Court {
        hx: c.hx2,
        hz: c.hz2,
        c: c.at(0.0, -c.back2),
        stair_head: c.at(0.0, up_front),
        ..c.clone()
    };
    v.extend(rim(
        &upper,
        &outline(&upper),
        c.top2,
        0.8,
        upper.edge(),
        c.stair_w * 0.5 + 1.0,
        &no_edge,
    ));

    v.extend(stair_rails(c.stair_head, c.stair_foot, c.stair_w, kit));
    let head2 = c.at(0.0, up_front);
    v.extend(stair_rails(
        head2,
        head2 + c.f * ((c.top2 - c.top) * super::shape::STAIR_RUN),
        c.stair_w,
        kit,
    ));
    // the lower tier: pillar rows and braziers
    let mut z = up_front + 4.0;
    let mut k = 0;
    while z < c.hz - 3.0 {
        for sg in [-1.0f32, 1.0] {
            v.push(obj_at(
                if k % 2 == 0 {
                    kit.pillar
                } else {
                    kit.tall_brazier
                },
                at(sg * (c.hx - 1.8), z),
                c.top,
                f,
                1.0,
            ));
        }
        z += 6.5;
        k += 1;
    }
    for sg in [-1.0f32, 1.0] {
        v.push(obj_at(
            kit.tall_brazier,
            at(sg * (c.stair_w * 0.5 + 1.6), c.hz - 1.4),
            c.top,
            f,
            1.0,
        ));
        v.push(obj_at(
            kit.statue,
            at(sg * (c.hx2 + 3.0), up_front + 2.0),
            c.top,
            f,
            1.2,
        ));
    }
    // the upper tier: the throne at the head of its stairs, the temple (or keep, or spire)
    // behind, braziers either side
    v.push(obj(kit.throne, at(0.0, up_front - 6.0), f, 1.25 * g));
    v.push(obj(
        kit.final_back,
        at(0.0, -c.back2 - c.hz2 + 12.5 * g),
        f,
        g,
    ));
    for sg in [-1.0f32, 1.0] {
        v.push(obj_at(
            kit.tall_brazier,
            at(sg * 8.0, up_front - 2.0),
            c.top2,
            f,
            1.0,
        ));
        v.push(obj_at(
            kit.pillar,
            at(sg * (c.hx2 - 1.5), up_front - 1.5),
            c.top2,
            f,
            1.0,
        ));
        v.push(obj_at(
            kit.pillar,
            at(sg * (c.hx2 - 1.5), -c.back2 - c.hz2 + 2.0),
            c.top2,
            f,
            1.0,
        ));
    }
    v
}

fn camps(plan: &Plan, kit: &Kit) -> Vec<Value> {
    let mut v = vec![];
    for c in &plan.camps {
        let sd = side(c.f);
        // the big piece at the back, the fire in the middle, the rest round them
        v.push(obj(
            kit.camp[0],
            c.c - c.f * c.r * 0.55,
            yaw_facing(c.f),
            1.0,
        ));
        for (i, a) in kit.camp.iter().skip(1).enumerate() {
            let ang = (i as f32 + 0.5) / (kit.camp.len() - 1) as f32 * std::f32::consts::PI * 1.4
                - std::f32::consts::PI * 0.7;
            let d = geom::rot(c.f, ang);
            let r = if i == 0 { 0.0 } else { c.r * 0.62 };
            let p = c.c + d * r + sd * 0.0;
            v.push(obj(*a, p, yaw_facing(-d), 1.0));
        }
    }
    v
}

/// The creatures (trash, a boss per court, the final boss), a camp's spawns, the boss arenas and
/// the places worth naming.
fn gameplay(plan: &Plan, kit: &Kit) -> Vec<Step> {
    let mut out = vec![];
    let lo = plan
        .courts
        .iter()
        .map(|c| c.level)
        .min()
        .unwrap_or(plan.final_court.level)
        .saturating_sub(2)
        .max(1);
    let hi = plan.final_court.level;
    for (name, family) in kit.trash {
        out.push(call(
            format!("Creature: {name}"),
            "gameplay_add_creature",
            json!({ "name": name, "family": family, "level": [lo, hi], "aggression": "Hostile" }),
        ));
    }
    for c in plan.all_courts() {
        out.push(call(format!("Boss: {}", c.boss), "gameplay_add_creature", json!({ "name": c.boss, "family": kit.boss_family, "level": [c.level, c.level], "aggression": "Hostile", "elite": true, "notes": format!("The boss of {}", c.name) })));
    }
    for (i, cp) in plan.camps.iter().enumerate() {
        let a = kit.trash[i % kit.trash.len()].0;
        let b = kit.trash[(i + 1) % kit.trash.len()].0;
        out.push(call(
            cp.name.clone(),
            "gameplay_add_spawn_region",
            json!({ "center": [r2(cp.c.x), r2(cp.c.y)], "radius": r2(cp.r), "creatures": [a, b], "weights": [2, 1], "name": cp.name, "level": cp.level, "behaviour": "Static", "pack": [2, 4], "max_alive": 5, "respawn_secs": 600 }),
        ));
    }
    let n = plan.courts.len();
    for c in plan.all_courts() {
        let notes = if c.kind == CourtKind::ThroneTerrace {
            if plan.axis.gated {
                format!("The final court. Its gate opens once the {n} courts' bosses have fallen.")
            } else {
                "The final court, at the end of the processional way.".to_string()
            }
        } else {
            format!(
                "{} {} (met {} of {}). Raised {:.0} m, stairs toward its trail.",
                c.kind.name(),
                if c.island.is_some() {
                    "on an island, reached by a bridge"
                } else {
                    ""
                },
                c.order,
                n + 1,
                c.top - plan.floor
            )
        };
        out.push(call(
            format!("Boss arena: {}", c.name),
            "gameplay_add_poi",
            json!({ "kind": "BossArena", "position": [r2(c.c.x), r2(c.c.y)], "name": c.name, "radius": r2(c.reach() + 8.0), "level": [c.level.saturating_sub(1).max(1), c.level], "notes": notes }),
        ));
        out.push(call(
            format!("Boss spawn: {}", c.boss),
            "gameplay_add_spawn_region",
            json!({ "center": [r2(c.c.x), r2(c.c.y)], "radius": 9.0, "creatures": [c.boss], "name": c.boss, "level": [c.level, c.level], "behaviour": "Static", "pack": [1, 1], "max_alive": 1, "respawn_secs": 0 }),
        ));
    }
    let h = &plan.hub;
    out.push(call("The hub", "gameplay_add_poi", json!({ "kind": "Landmark", "position": [r2(h.c.x), r2(h.c.y)], "name": h.name, "radius": r2(h.r + 10.0) })));
    let a = &plan.axis;
    let gate_note = if a.gated {
        format!("Sealed until the {n} courts' bosses have fallen")
    } else {
        "The way to the final court".to_string()
    };
    out.push(call("The gate", "gameplay_add_poi", json!({ "kind": "Landmark", "position": [r2(a.gate.x), r2(a.gate.y)], "name": "The Processional Gate", "radius": 14.0, "notes": gate_note })));
    for l in &plan.lakes {
        out.push(call(format!("Vista: {}", l.name), "gameplay_add_poi", json!({ "kind": "Vista", "position": [r2(l.c.x), r2(l.c.y)], "name": l.name, "radius": r2(l.r) })));
    }
    out
}
