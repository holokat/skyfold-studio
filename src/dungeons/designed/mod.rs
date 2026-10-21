//! Designed layouts: outdoor realms laid out by level-design principles, as a graph of spaces
//! with roles (entrance, approach, hub, arenas and set pieces, vistas, secrets, the shortcut back,
//! the finale) placed on the land by a plan. Every plan's layout passes the same checks
//! (`graph::Graph::check`): pacing (tight connectors between open reveals, building to the
//! finale), a sightline to the finale from the entrance or the first vista, hierarchy (a few big
//! set pieces, smaller connectors), a loop or an optional branch, a one-way shortcut that opens
//! back near the start, purposeful placement (camps by water or along the route, lookouts on high
//! ground over the paths, ruins along the old roads) and readable routes with gates between
//! sections.
//!
//! The plans: the temple city (`compound`, the first; its own layout, kept as it was), the
//! mountain ascent (`mountain`), the canyon descent (`canyon`) and the fortress siege
//! (`fortress`). Each picks its realm's land and shapes it (`Op`), places its graph, and says
//! what to build; `steps` turns that into the editor's own tools, so everything made is ordinary
//! content.

use bevy::math::Vec2;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub mod canyon;
pub mod fortress;
pub mod graph;
pub mod mountain;

use super::compound::furnish::{self, call, obj, r2, Step};
use super::compound::layout::{side, yaw_facing, Grid};
use super::compound::theme::{boss_name, fill, Kit, Theme, A};
use super::geom;
use crate::noise::{fbm, smoothstep, Rng};
use crate::terrain::layers::Layer;
use crate::terrain::realm_land::{RealmShape, RealmSize};
use graph::{Graph, LinkKind, Role};

/// The plans a designed layout can follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanKind {
    /// courts round a hub in a walled valley, a processional way to the final court
    #[default]
    TempleCity,
    /// terraces up a mountain on switchback trails, the finale on its summit
    MountainAscent,
    /// down into a gorge whose chambers widen into arenas, bridges over its stream
    CanyonDescent,
    /// up through outer defences and courtyards to the keep
    FortressSiege,
}

impl PlanKind {
    pub const ALL: [PlanKind; 4] = [PlanKind::TempleCity, PlanKind::MountainAscent, PlanKind::CanyonDescent, PlanKind::FortressSiege];
    pub fn id(self) -> &'static str {
        match self {
            PlanKind::TempleCity => "temple_city",
            PlanKind::MountainAscent => "mountain_ascent",
            PlanKind::CanyonDescent => "canyon_descent",
            PlanKind::FortressSiege => "fortress_siege",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            PlanKind::TempleCity => "Temple city",
            PlanKind::MountainAscent => "Mountain ascent",
            PlanKind::CanyonDescent => "Canyon descent",
            PlanKind::FortressSiege => "Fortress siege",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            PlanKind::TempleCity => "A walled valley: a hub, raised boss courts round it, lakes and an island temple, a straight processional way to the final court",
            PlanKind::MountainAscent => "Switchback trails up a mountain, terraces as arenas, a lookout with the summit in view, the finale on the summit; a steep stair back down",
            PlanKind::CanyonDescent => "Down from the rim into a gorge whose chambers widen into arenas, bridges over its stream, secrets up the side canyons, a climb back out",
            PlanKind::FortressSiege => "Past a siege camp and the outer defences, through courtyards behind gatehouses to the keep; flanking paths and a sally port back out",
        }
    }
    pub fn icon(self) -> &'static str {
        use egui_phosphor::regular as icon;
        match self {
            PlanKind::TempleCity => icon::BANK,
            PlanKind::MountainAscent => icon::MOUNTAINS,
            PlanKind::CanyonDescent => icon::WAVES,
            PlanKind::FortressSiege => icon::CASTLE_TURRET,
        }
    }
    pub fn from_id(s: &str) -> Option<PlanKind> {
        let k = s.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        Self::ALL.into_iter().find(|p| p.id() == k || p.name().to_ascii_lowercase().replace(' ', "_") == k).or(match k.as_str() {
            "temple" | "temple_compound" | "compound" | "city" => Some(PlanKind::TempleCity),
            "mountain" | "ascent" | "summit" | "peak" => Some(PlanKind::MountainAscent),
            "canyon" | "gorge" | "descent" | "ravine" => Some(PlanKind::CanyonDescent),
            "fortress" | "siege" | "castle" | "citadel" | "keep" => Some(PlanKind::FortressSiege),
            _ => None,
        })
    }
    /// The land its realm is made of.
    pub fn land(self) -> RealmShape {
        match self {
            PlanKind::TempleCity => RealmShape::EnclosedValley,
            PlanKind::MountainAscent | PlanKind::FortressSiege => RealmShape::RoundIsle,
            PlanKind::CanyonDescent => RealmShape::Canyon,
        }
    }
    pub fn size(self) -> RealmSize {
        RealmSize::Small
    }
    /// What its arenas are called in its options ("Boss courts", "Terraces"…).
    pub fn arenas(self) -> &'static str {
        match self {
            PlanKind::TempleCity => "Boss courts",
            PlanKind::MountainAscent => "Terraces",
            PlanKind::CanyonDescent => "Chambers",
            PlanKind::FortressSiege => "Courtyards",
        }
    }
}

// ------------------------------------------------------------------ what a plan makes

/// A change to the land's heights.
#[derive(Clone, Debug)]
pub enum Op {
    /// a mountain or a mound raised round `c` (radius, height; `flat` > 0 flattens its top to
    /// that share of its radius)
    Massif { c: Vec2, r: f32, h: f32, flat: f32, seed: u32 },
    /// a level pad at `y`: cut into the slope above (`cut` m per m out), filled below (`fill`)
    Pad { c: Vec2, r: f32, y: f32, cut: f32, fill: f32 },
    /// a bench along a route, level across, at the heights `ys` along it
    Bench { pts: Vec<Vec2>, ys: Vec<f32>, w: f32 },
}

/// A designed layout made by one of the plans (not the temple city: `compound::layout::Plan`).
#[derive(Clone, Debug)]
pub struct Design {
    pub plan: PlanKind,
    pub theme: Theme,
    pub seed: u32,
    pub graph: Graph,
    /// where players arrive and which way they face
    pub start: Vec2,
    pub start_f: Vec2,
    pub ops: Vec<Op>,
    pub lakes: Vec<(String, Vec<Vec2>)>,
    /// rivers: name, course, width
    pub rivers: Vec<(String, Vec<Vec2>, f32)>,
    /// ground painted: a disc and its layer
    pub paint: Vec<(Vec2, f32, Layer)>,
    /// roads: name, kind, control points, width
    pub roads: Vec<(String, &'static str, Vec<Vec2>, f32)>,
    /// objects to place, in batches (label, items)
    pub objects: Vec<(String, Vec<Value>)>,
    /// the game's content: boss arenas, places, camps, creatures
    pub gameplay: Vec<(String, &'static str, Value)>,
    /// nature kept off (centre, radius) and grown thick
    pub clear: Vec<(Vec2, f32)>,
    pub thick: Vec<(Vec2, f32)>,
    /// how much bigger than a small realm's it is laid out
    pub grand: f32,
}

/// What a plan is asked for.
#[derive(Clone, Copy, Debug)]
pub struct Ask {
    pub plan: PlanKind,
    pub theme: Theme,
    /// arenas besides the finale (3-6)
    pub arenas: usize,
    pub seed: u32,
    pub levels: [u8; 2],
}

/// Lays out a design of `ask.plan` on land `g` (the realm's own, as made).
pub fn lay_out(g: &Grid, ask: &Ask) -> Result<Design, String> {
    match ask.plan {
        PlanKind::MountainAscent => mountain::lay_out(g, ask),
        PlanKind::CanyonDescent => canyon::lay_out(g, ask),
        PlanKind::FortressSiege => fortress::lay_out(g, ask),
        PlanKind::TempleCity => Err("the temple city is laid out by `compound`".into()),
    }
}

// ------------------------------------------------------------------ shaping the land

/// The heights shaped by the design's ops, in order.
pub fn apply(h: &mut [f32], n: usize, cell: f32, ops: &[Op]) {
    let half = (n - 1) as f32 * cell * 0.5;
    for op in ops {
        match op {
            Op::Massif { c, r, h: top, flat, seed } => {
                let (c, r, top, flat, seed) = (*c, *r, *top, *flat, *seed);
                super::compound::shape::each(n, cell, c - Vec2::splat(r), c + Vec2::splat(r), |k, p| {
                    let d = p.distance(c) / r;
                    if d >= 1.0 {
                        return;
                    }
                    let a = (p - c).y.atan2((p - c).x);
                    // spurs and gullies round it, crags near the top
                    let spurs = fbm(a.cos() * 2.2 + 7.0, a.sin() * 2.2 + d * 1.5, 3, 2.0, 0.5, seed + 3);
                    let crag = fbm(p.x / 35.0, p.y / 35.0, 3, 2.0, 0.5, seed + 9);
                    let prof = if flat > 0.0 { smoothstep(1.0, flat, d) } else { (1.0 - d.powf(1.4)).max(0.0).powf(1.6) };
                    let add = top * prof * (1.0 + 0.22 * spurs * (1.0 - d)) + 3.0 * crag * prof * (1.0 - flat);
                    h[k] += add * (1.0 - smoothstep(0.85, 1.0, d));
                });
            }
            Op::Pad { c, r, y, cut, fill } => {
                let reach = r + 60.0;
                super::compound::shape::each(n, cell, *c - Vec2::splat(reach), *c + Vec2::splat(reach), |k, p| {
                    let d = (p.distance(*c) - r).max(0.0);
                    h[k] = h[k].clamp(y - d * fill, y + d * cut);
                });
            }
            Op::Bench { pts, ys, w } => {
                if pts.len() < 2 {
                    continue;
                }
                let lo = pts.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)) - Vec2::splat(w + 40.0);
                let hi = pts.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)) + Vec2::splat(w + 40.0);
                let cum: Vec<f32> = std::iter::once(0.0).chain(pts.windows(2).scan(0.0, |s, q| {
                    *s += q[0].distance(q[1]);
                    Some(*s)
                })).collect();
                let y_at = |s: f32| {
                    let i = cum.partition_point(|c| *c <= s).clamp(1, cum.len() - 1);
                    let t = ((s - cum[i - 1]) / (cum[i] - cum[i - 1]).max(1e-4)).clamp(0.0, 1.0);
                    ys[i - 1] + (ys[i] - ys[i - 1]) * t
                };
                super::compound::shape::each(n, cell, lo, hi, |k, p| {
                    let (dist, s) = geom::polyline_dist(pts, p);
                    let d = (dist - w * 0.5).max(0.0);
                    if d > 40.0 {
                        return;
                    }
                    let y = y_at(s);
                    h[k] = h[k].clamp(y - d * 1.0, y + d * 1.3);
                });
            }
        }
    }
    let _ = half;
}

/// A route of `w` from `a` to `b` whose ground rises from `ya` to `yb`: straight-ish when it is
/// long enough for `grade`, else zigzagging across the slope (switchbacks); and the heights
/// along it.
pub fn switchback(a: Vec2, ya: f32, b: Vec2, yb: f32, grade: f32, swing: f32, rng: &mut Rng) -> (Vec<Vec2>, Vec<f32>) {
    let d = a.distance(b).max(1.0);
    let u = (b - a) / d;
    let s = side(u) * if rng.f32() < 0.5 { 1.0 } else { -1.0 };
    let need = (yb - ya).abs() / grade.max(0.02);
    let mut pts = vec![a];
    if need <= d * 1.08 {
        // a lazy bend is enough
        let amp = (d * 0.08).min(swing) * if rng.f32() < 0.5 { 1.0 } else { -1.0 };
        let m = ((d / 8.0).ceil() as usize).max(2);
        for i in 1..m {
            let t = i as f32 / m as f32;
            pts.push(a + u * d * t + side(u) * amp * (std::f32::consts::PI * t).sin());
        }
    } else {
        let extra = (need * need - d * d).sqrt();
        let m = ((extra / (2.0 * swing)).ceil() as usize).max(2);
        let w = extra / (2.0 * m as f32);
        for k in 1..m {
            let sign = if k % 2 == 1 { 1.0 } else { -1.0 };
            pts.push(a + u * (d * k as f32 / m as f32) + s * w * sign);
        }
    }
    pts.push(b);
    // round the turns
    for _ in 0..2 {
        let mut out = vec![pts[0]];
        for w2 in pts.windows(2) {
            out.push(w2[0].lerp(w2[1], 0.25));
            out.push(w2[0].lerp(w2[1], 0.75));
        }
        out.push(*pts.last().unwrap());
        out.dedup_by(|x, y| x.distance(*y) < 0.5);
        pts = out;
    }
    let len = geom::polyline_len(&pts).max(1.0);
    let mut acc = 0.0;
    let mut ys = vec![ya];
    for w2 in pts.windows(2) {
        acc += w2[0].distance(w2[1]);
        ys.push(ya + (yb - ya) * acc / len);
    }
    (pts, ys)
}

// ------------------------------------------------------------------ what plans share

/// Names a plan's spaces after the theme's totems.
pub struct Namer {
    totems: Vec<&'static str>,
    next: usize,
}

impl Namer {
    pub fn new(kit: &Kit, rng: &mut Rng) -> Namer {
        let mut totems = kit.totems.to_vec();
        super::compound::layout::shuffle(&mut totems, rng);
        Namer { totems, next: 0 }
    }
    pub fn totem(&mut self) -> &'static str {
        let t = self.totems[self.next % self.totems.len()];
        self.next += 1;
        t
    }
    pub fn name(&mut self, pattern: &str) -> String {
        let t = self.totem();
        fill(pattern, t)
    }
}

/// Ruins along a route (the old roads): broken walls, pillar stumps, statues, rubble.
pub fn ruins_along(pts: &[Vec2], width: f32, kit: &Kit, rng: &mut Rng, ok: &dyn Fn(Vec2) -> bool, out: &mut Vec<Value>, at: &mut Vec<Vec2>) {
    let len = geom::polyline_len(pts);
    let rubble: &'static str = kit.ruins.iter().map(|(a, _)| a.0).find(|a| a.contains("rubble")).unwrap_or("stonefist_rubble_pile");
    let mut s = rng.range(10.0, 24.0);
    while s < len - 10.0 {
        let (p, d) = geom::polyline_at(pts, s);
        let sd = side(d) * if rng.f32() < 0.5 { 1.0 } else { -1.0 };
        let q = p + sd * (width * 0.5 + rng.range(2.5, 6.0));
        if rng.f32() < 0.8 && ok(q) {
            match rng.below(4) {
                0 => {
                    for j in 0..2 + rng.below(3) as usize {
                        if rng.f32() < 0.25 {
                            continue;
                        }
                        let mut o = obj(("stone_wall", 1.3), q + d * (j as f32 * 3.1), super::compound::layout::yaw_along(d) + rng.range(-5.0, 5.0), 1.0);
                        if rng.f32() < 0.4 {
                            o["scale"] = json!([1.3, r2(1.3 * rng.range(0.4, 0.8)), 1.3]);
                        }
                        out.push(o);
                    }
                }
                1 => {
                    for j in 0..2 + rng.below(2) as usize {
                        let mut o = obj(kit.pillar, q + d * (j as f32 * 3.4), rng.range(0.0, 360.0), 1.0);
                        let s3 = kit.pillar.1;
                        o["scale"] = json!([r2(s3), r2(s3 * rng.range(0.22, 0.6)), r2(s3)]);
                        out.push(o);
                    }
                }
                2 => out.push(obj(kit.statue, q, yaw_facing(-sd), 1.0)),
                _ => out.push(obj((rubble, 1.3), q, rng.range(0.0, 360.0), rng.range(0.8, 1.3))),
            }
            at.push(q);
        }
        s += rng.range(18.0, 32.0);
    }
}

/// A camp of the theme's: its big piece at the back, a fire, the rest round them.
pub fn camp(kit: &Kit, c: Vec2, f: Vec2, r: f32) -> Vec<Value> {
    let mut v = vec![obj(kit.camp[0], c - f * r * 0.55, yaw_facing(f), 1.0)];
    for (i, a) in kit.camp.iter().skip(1).enumerate() {
        let ang = (i as f32 + 0.5) / (kit.camp.len() - 1) as f32 * std::f32::consts::PI * 1.4 - std::f32::consts::PI * 0.7;
        let d = geom::rot(f, ang);
        let rr = if i == 0 { 0.0 } else { r * 0.62 };
        v.push(obj(*a, c + d * rr, yaw_facing(-d), 1.0));
    }
    v
}

/// An arena's dressing: a floor in its middle, a ring of pillars and fire, statues by its way in,
/// a backdrop piece behind.
pub fn arena(kit: &Kit, c: Vec2, f: Vec2, r: f32, y: f32, floor: &'static str, back: A, g: f32) -> Vec<Value> {
    let mut v = vec![obj((floor, 1.0), c, yaw_facing(f), g), obj(back, c - f * (r - 6.0 * g), yaw_facing(f), g)];
    for i in 0..10 {
        let a = (i as f32 + 0.5) / 10.0 * std::f32::consts::TAU;
        let d = geom::rot(f, a);
        if d.dot(f) > 0.85 || d.dot(f) < -0.85 {
            continue;
        }
        v.push(furnish::obj_at(if i % 2 == 0 { kit.pillar } else { kit.brazier }, c + d * (r - 2.5), y, yaw_facing(-d), 1.0));
    }
    for sg in [-1.0f32, 1.0] {
        v.push(obj(kit.statue, c + f * (r - 2.0) + side(f) * sg * 6.0, yaw_facing(f), 1.0));
    }
    v
}

/// The gameplay a design's graph wants: its creatures, a boss and an arena per big space, its
/// named places, a camp's spawns, archers at the lookouts.
pub fn gameplay(design: &mut Design, kit: &Kit, levels: [u8; 2], rng: &mut Rng) {
    let gr = design.graph.clone();
    let [lo, hi] = [levels[0].min(levels[1]), levels[0].max(levels[1])];
    let path = gr.main_path();
    let bigs: Vec<usize> = path.iter().copied().filter(|i| gr.spaces[*i].role.big()).chain((0..gr.spaces.len()).filter(|i| gr.spaces[*i].role.big() && !path.contains(i))).collect();
    let n = bigs.len().max(1);
    let gp = &mut design.gameplay;
    for (name, family) in kit.trash {
        gp.push((format!("Creature: {name}"), "gameplay_add_creature", json!({ "name": name, "family": family, "level": [lo, hi], "aggression": "Hostile" })));
    }
    for (k, i) in bigs.iter().enumerate() {
        let s = &gr.spaces[*i];
        let level = if s.role == Role::Finale { hi } else { (lo as f32 + (hi - lo) as f32 * (k + 1) as f32 / (n + 1) as f32).round() as u8 };
        let title = kit.boss_titles[rng.below(kit.boss_titles.len() as u32) as usize];
        let boss = format!("{} {}", boss_name(rng), if s.role == Role::Finale { kit.final_title.to_string() } else { fill(title, kit.totems[k % kit.totems.len()]) });
        gp.push((format!("Boss: {boss}"), "gameplay_add_creature", json!({ "name": boss, "family": kit.boss_family, "level": [level, level], "aggression": "Hostile", "elite": true, "notes": format!("The boss of {}", s.name) })));
        gp.push((format!("Boss arena: {}", s.name), "gameplay_add_poi", json!({ "kind": "BossArena", "position": [r2(s.c.x), r2(s.c.y)], "name": s.name, "radius": r2(s.r + 6.0), "level": [level.saturating_sub(1).max(1), level], "notes": if s.role == Role::Finale { "The finale".to_string() } else { format!("Met {} of {}", k + 1, n) } })));
        gp.push((format!("Boss spawn: {boss}"), "gameplay_add_spawn_region", json!({ "center": [r2(s.c.x), r2(s.c.y)], "radius": 9.0, "creatures": [boss], "name": boss, "level": [level, level], "behaviour": "Static", "pack": [1, 1], "max_alive": 1, "respawn_secs": 0 })));
    }
    for s in &gr.spaces {
        let kind = match s.role {
            Role::Vista => "Vista",
            Role::Secret => "Treasure",
            Role::Hub => "Landmark",
            _ => continue,
        };
        gp.push((format!("{kind}: {}", s.name), "gameplay_add_poi", json!({ "kind": kind, "position": [r2(s.c.x), r2(s.c.y)], "name": s.name, "radius": r2(s.r + 4.0) })));
    }
    for l in gr.links.iter().filter(|l| l.kind == LinkKind::Shortcut) {
        if let Some(gpos) = l.gate {
            gp.push(("The shortcut's gate".into(), "gameplay_add_poi", json!({ "kind": "Landmark", "position": [r2(gpos.x), r2(gpos.y)], "name": "The Postern", "radius": 10.0, "notes": "A shortcut back to the start: it opens from the far side" })));
        }
    }
    for (i, c) in gr.camps.iter().enumerate() {
        let a = kit.trash[i % kit.trash.len()].0;
        let b = kit.trash[(i + 1) % kit.trash.len()].0;
        let lv = (lo as f32 + (hi - lo) as f32 * (i as f32 + 0.5) / (gr.camps.len() as f32 + 1.0)).round() as u8;
        gp.push((format!("Camp {}", i + 1), "gameplay_add_spawn_region", json!({ "center": [r2(c.x), r2(c.y)], "radius": 9.5, "creatures": [a, b], "weights": [2, 1], "name": format!("Camp {}", i + 1), "level": [lv, lv.saturating_add(1)], "behaviour": "Static", "pack": [2, 4], "max_alive": 5, "respawn_secs": 600 })));
    }
    for (i, o) in gr.lookouts.iter().enumerate() {
        let a = kit.trash[(i + 2) % kit.trash.len()].0;
        gp.push((format!("Lookout {}", i + 1), "gameplay_add_spawn_region", json!({ "center": [r2(o.x), r2(o.y)], "radius": 8.5, "creatures": [a], "name": format!("Lookout {}", i + 1), "level": [hi.saturating_sub(2).max(lo), hi.saturating_sub(1).max(lo)], "behaviour": "Static", "pack": [1, 2], "max_alive": 2, "respawn_secs": 600 })));
    }
}

/// Every step that builds a design, in order: rivers first (the roads bridge them), the roads,
/// the objects, the game's content.
pub fn steps(d: &Design) -> Vec<Step> {
    let mut out = vec![Step::Wait(12)];
    for (name, pts, w) in &d.rivers {
        out.push(call(name.clone(), "water_add_river", json!({ "points": pts.iter().map(|p| [r2(p.x), r2(p.y)]).collect::<Vec<_>>(), "width": r2(*w), "depth": 1.4, "name": name })));
    }
    if !d.rivers.is_empty() {
        out.push(Step::Wait(14));
    }
    for (name, kind, pts, w) in &d.roads {
        let every = if pts.len() > 2 { 10.0 } else { 1000.0 };
        let pts = if pts.len() > 2 { furnish::controls(pts, every) } else { pts.iter().map(|p| [r2(p.x), r2(p.y)]).collect() };
        out.push(call(name.clone(), "roads_create", json!({ "kind": kind, "points": pts, "width": r2(*w), "name": name, "options": { "props": false } })));
    }
    out.push(Step::Wait(4));
    for (label, items) in &d.objects {
        out.extend(furnish::place(label.clone(), items.clone()));
    }
    for (label, tool, args) in &d.gameplay {
        out.push(call(label.clone(), tool, args.clone()));
    }
    out
}

impl Design {
    pub fn summary(&self) -> Value {
        let mut s = self.graph.summary();
        s["plan"] = json!(self.plan.id());
        s["theme"] = json!(self.theme.id());
        s["seed"] = json!(self.seed);
        s["players_arrive"] = json!([r2(self.start.x), r2(self.start.y)]);
        s["lakes"] = json!(self.lakes.iter().map(|(n, o)| json!({ "name": n, "points": o.len() })).collect::<Vec<_>>());
        s["rivers"] = json!(self.rivers.iter().map(|(n, p, w)| json!({ "name": n, "length_m": r2(geom::polyline_len(p)), "width_m": r2(*w) })).collect::<Vec<_>>());
        s["roads"] = json!(self.roads.len());
        s["objects"] = json!(self.objects.iter().map(|(_, v)| v.len()).sum::<usize>());
        s
    }
}

// ------------------------------------------------------------------ after the biomes

/// The design's lakes, paint, nature and where players arrive (the realm's biomes have grown).
pub fn finish(d: &mut crate::io::WorldData, design: &Design) {
    use bevy::math::Vec3;
    for (layer, id) in design.theme.layer_choices() {
        d.layer_choices.set(*layer as usize, id);
    }
    let shape = crate::water::lakes::shape_by_id(design.theme.lake());
    for (name, outline) in &design.lakes {
        let res = {
            let crate::io::WorldData { terrain, history, store, water, lib, .. } = &mut *d;
            crate::water::create_drawn_lake(terrain, history, store, lib.as_deref(), water, outline, shape, 1.0, 1.0)
        };
        match res {
            Ok(id) => {
                if let Some(l) = d.water.lakes.iter_mut().find(|l| l.id == id) {
                    l.name = name.clone();
                }
            }
            Err(e) => bevy::log::warn!("designed layout lake: {e}"),
        }
    }
    {
        let t = &mut *d.terrain;
        let (n, cell) = (t.n(), t.cell);
        let mut ks = vec![];
        for (c, r, layer) in &design.paint {
            super::compound::shape::each(n, cell, *c - Vec2::splat(r + 2.0), *c + Vec2::splat(r + 2.0), |k, p| ks.push((k, 1.0 - smoothstep(r - 1.0, r + 1.5, p.distance(*c)), *layer)));
        }
        for (k, w, layer) in ks {
            super::compound::shape::put(t, k, layer, w);
        }
        t.mark_all_dirty();
    }
    let half = d.terrain.half();
    let clear = design.clear.clone();
    let thick = design.thick.clone();
    d.nature.set_texels(Vec2::splat(-half), Vec2::splat(half), move |p| {
        let mut c = 0.0f32;
        for (q, r) in &clear {
            c = c.max(1.0 - smoothstep(*r, r + 8.0, p.distance(*q)));
        }
        let mut t = 0.0f32;
        for (q, r) in &thick {
            t = t.max(1.0 - smoothstep(r * 0.6, *r, p.distance(*q)));
        }
        if c <= 0.01 && t <= 0.01 {
            return None;
        }
        let keep = 1.0 - c;
        Some(([keep * (1.0 + 2.4 * t), keep, keep * (1.0 + 0.8 * t), keep], 0))
    });
    let s = design.start;
    let y = d.terrain.height_at(s.x, s.y);
    let f = design.start_f;
    d.gameplay.player_start = Some(crate::gameplay::PlayerStart { pos: [s.x, y, s.y], yaw: (-f.y).atan2(f.x) });
    d.gameplay.dirty = true;
    let fin = design.graph.spaces.iter().find(|x| x.role == Role::Finale).map(|x| x.c).unwrap_or(Vec2::ZERO);
    let mid = s.lerp(fin, 0.45);
    let look = fin - s;
    d.orbit.set_view(Vec3::new(mid.x, y, mid.y), look.x.atan2(look.y) + std::f32::consts::PI, 0.7, (fin.distance(s) * 1.3).max(220.0));
    d.orbit.snap();
}

/// Paints the land with the design for a preview: spaces by role, links, water, lookouts.
pub fn overlay(img: &mut [u8], px: usize, size: f32, d: &Design) {
    let mpp = size / px as f32;
    let half = size * 0.5;
    let put = |img: &mut [u8], p: Vec2, col: [u8; 3], a: f32| {
        let q = (p + Vec2::splat(half)) / mpp;
        let (x, y) = (q.x as i32, q.y as i32);
        if x >= 0 && y >= 0 && (x as usize) < px && (y as usize) < px {
            let o = (y as usize * px + x as usize) * 4;
            for i in 0..3 {
                img[o + i] = (img[o + i] as f32 * (1.0 - a) + col[i] as f32 * a) as u8;
            }
        }
    };
    let g = &d.graph;
    for (_, pts, w) in &d.rivers {
        line(img, px, mpp, half, pts, w * 0.5, [62, 128, 168], &put);
    }
    for (_, o) in &d.lakes {
        let mut lp = o.clone();
        lp.push(o[0]);
        let c = o.iter().copied().sum::<Vec2>() / o.len() as f32;
        let r = o.iter().map(|p| p.distance(c)).fold(0.0f32, f32::max);
        disc(img, px, mpp, half, c, r * 0.9, [62, 128, 168], &put);
    }
    for l in &g.links {
        let col = match l.kind {
            LinkKind::Shortcut => [190, 70, 60],
            LinkKind::Secret => [150, 110, 170],
            LinkKind::Stairs => [200, 190, 170],
            _ => [176, 136, 92],
        };
        line(img, px, mpp, half, &l.pts, (l.width * 0.5).max(mpp), col, &put);
    }
    for s in &g.spaces {
        let col = match s.role {
            Role::Finale => [236, 198, 96],
            Role::Arena | Role::SetPiece => [200, 190, 172],
            Role::Vista => [120, 190, 220],
            Role::Secret => [170, 120, 200],
            Role::Entrance => [245, 245, 245],
            _ => [214, 204, 180],
        };
        disc(img, px, mpp, half, s.c, s.r, col, &put);
    }
    for o in &g.lookouts {
        disc(img, px, mpp, half, *o, 4.0f32.max(mpp * 1.5), [120, 40, 40], &put);
    }
    for c in &g.camps {
        disc(img, px, mpp, half, *c, 4.0f32.max(mpp * 1.5), [230, 120, 40], &put);
    }
}

type Put<'a> = dyn Fn(&mut [u8], Vec2, [u8; 3], f32) + 'a;

fn disc(img: &mut [u8], _px: usize, mpp: f32, _half: f32, c: Vec2, r: f32, col: [u8; 3], put: &Put) {
    let step = mpp * 0.7;
    let n = (r / step).ceil() as i32;
    for i in -n..=n {
        for j in -n..=n {
            let p = c + Vec2::new(i as f32, j as f32) * step;
            if p.distance(c) <= r {
                put(img, p, col, 0.9);
            }
        }
    }
}

fn line(img: &mut [u8], px: usize, mpp: f32, half: f32, pts: &[Vec2], w: f32, col: [u8; 3], put: &Put) {
    let len = geom::polyline_len(pts);
    let n = (len / (mpp * 0.6)).ceil() as usize;
    for i in 0..=n {
        let (p, _) = geom::polyline_at(pts, len * i as f32 / n.max(1) as f32);
        disc(img, px, mpp, half, p, w.max(mpp * 0.6), col, put);
    }
}

/// Island land: how far each vertex is from the sea (m; 0 at sea).
pub fn coast(g: &Grid) -> Vec<f32> {
    let sea: Vec<bool> = g.h.iter().map(|h| *h <= 1.0).collect();
    super::compound::layout::chamfer(&sea, g.n, g.cell)
}

pub fn sample(v: &[f32], n: usize, cell: f32, p: Vec2) -> f32 {
    let half = (n - 1) as f32 * cell * 0.5;
    let q = ((p + Vec2::splat(half)) / cell).round();
    let (i, j) = ((q.x.max(0.0) as usize).min(n - 1), (q.y.max(0.0) as usize).min(n - 1));
    v[j * n + i]
}
