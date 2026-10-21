//! What furnishes a cave besides its rock:
//!
//! - **Objects** (ordinary placed objects, so they collide, export and can be moved or deleted
//!   like any other; the cave keeps their ids and replaces them when it is laid out again): the
//!   dungeon's door set into the far wall of the deepest chamber (`ph:large_castle_door`), glowing
//!   crystal clusters, lanterns (Lights), and in mines crates, barrels and a pickaxe left in the
//!   rooms. Everything is placed by probing the cave's own rock (floors, walls), deterministic
//!   per seed.
//! - **Cave meshes** (built with the rock): timber sets along mine drifts and a heavy timber portal
//!   at a mine's mouth, and still water in the pools.
//! - **Lights** follow the lanterns and crystals (`render::sync_lights`).

use bevy::prelude::*;

use super::model::*;
use super::sdf::{self, Volume};
use crate::io::WorldData;
use crate::noise::Rng;
use crate::objects::Placed;

pub const DOOR: &str = "ph:large_castle_door";
pub const LANTERN: &str = "ph:Lantern_01";
pub const CRYSTALS: &str = "Crystals";
const CRATES: [&str; 2] = ["ph:wooden_crate_01", "ph:wooden_barrels_01"];
const PICK: &str = "ph:picke_dirty_01";

/// A pool in a chamber: its centre (x, z) and radius (the same everywhere it is needed).
pub fn pool_of(cave: &Cave, c: &Chamber) -> Option<(Vec2, f32)> {
    let mut r = Rng::new((cave.seed as u64) << 20 ^ c.id as u64 ^ 0x9001);
    let mid = Vec2::new(c.pos[0], c.pos[2]);
    // the ways out of the chamber's middle (tunnels, the dungeon's doorway, a pit's scree): the
    // pool lies in the widest gap between them, clear of them, so no one has to wade it
    let mut ways: Vec<f32> = vec![];
    for t in &cave.tunnels {
        let pts: Vec<Vec2> = if t.a == c.id {
            t.pts.iter().map(|p| Vec2::new(p[0], p[2])).collect()
        } else if t.b == c.id {
            t.pts.iter().rev().map(|p| Vec2::new(p[0], p[2])).collect()
        } else {
            continue;
        };
        if let Some(q) = pts.iter().find(|q| q.distance(mid) > 1.5) {
            let d = *q - mid;
            ways.push(d.y.atan2(d.x));
        }
    }
    if let Some(dg) = cave.dungeon.as_ref().filter(|d| d.chamber == c.id) {
        let d = Vec2::new(dg.door[0], dg.door[2]) - mid;
        ways.push(d.y.atan2(d.x));
    }
    if cave.mouth == Mouth::Pit && cave.chambers.first().is_some_and(|f| f.id == c.id) {
        let d = -super::layout::dir(cave.heading);
        ways.push(d.y.atan2(d.x));
    }
    let tau = std::f32::consts::TAU;
    ways.sort_by(|a, b| a.total_cmp(b));
    let (dir, half_gap) = if ways.is_empty() {
        (r.f32() * tau, std::f32::consts::PI)
    } else {
        let mut best = (0.0f32, 0.0f32);
        for (i, a) in ways.iter().enumerate() {
            let b = if i + 1 < ways.len() {
                ways[i + 1]
            } else {
                ways[0] + tau
            };
            if b - a > best.1 * 2.0 {
                best = (a + (b - a) * 0.5, (b - a) * 0.5);
            }
        }
        best
    };
    let dir = dir + r.range(-0.15, 0.15) * half_gap.min(1.0);
    // the chamber's floor reaches about 0.79 of its radius that way (its dome's foot)
    let u = Vec2::from_angle(dir);
    let (s, co) = c.yaw.sin_cos();
    let local = Vec2::new(u.x * co - u.y * s, u.x * s + u.y * co);
    let wall = 0.79 / ((local.x / c.size[0]).powi(2) + (local.y / c.size[2]).powi(2)).sqrt();
    let off = wall * 0.55;
    let clear = super::sdf::WALK_W * 0.5 + 0.6;
    let radius = (off * half_gap.min(std::f32::consts::FRAC_PI_2).sin() - clear)
        .min(wall - off - 0.5)
        .min(c.size[0].min(c.size[2]) * r.range(0.45, 0.6));
    (radius >= 1.6).then(|| (mid + u * off, radius))
}

/// The still water's level in a chamber's pool: a hand under its floor (a rimstone lip round
/// the pool holds it in where the floor dips lower, `sdf`).
pub fn pool_level(c: &Chamber) -> f32 {
    c.pos[1] - 0.12
}

/// One object to place: asset, position, heading (radians about +y), size.
#[derive(Clone, Debug)]
pub struct Prop {
    pub asset: &'static str,
    pub pos: Vec3,
    pub yaw: f32,
    pub scale: f32,
}

/// Floor under a spot in a chamber (probing the rock from a metre over its floor).
fn floor_at(vol: &Volume, q: Vec2, around: f32) -> Option<f32> {
    let mut y = around + 1.2;
    if vol.at(Vec3::new(q.x, y, q.y)).d < 0.0 {
        return None;
    }
    while y > around - 3.0 {
        let ny = y - 0.1;
        if vol.at(Vec3::new(q.x, ny, q.y)).d <= 0.0 {
            return Some(ny + 0.02);
        }
        y = ny;
    }
    None
}

/// From `from` along `dir` at height `y`, how far to the rock (None: nothing within `max`).
fn to_wall(vol: &Volume, from: Vec2, dir: Vec2, y: f32, max: f32) -> Option<f32> {
    let mut s = 0.0;
    while s < max {
        let q = from + dir * s;
        if vol.at(Vec3::new(q.x, y, q.y)).d <= 0.0 {
            return Some(s);
        }
        s += 0.2;
    }
    None
}

/// The objects a cave is furnished with.
pub fn plan(cave: &Cave, vol: &Volume) -> Vec<Prop> {
    let mut out = vec![];
    let mut rng = Rng::new(cave.seed as u64 ^ 0xf0_0d_5eed);
    // (the dungeon's way in at the end of its doorway is a liquid portal: `crate::dungeons::portal`
    // draws it; no door)
    let feats = cave.features;
    let mine = cave.kind == CaveKind::Mine || cave.theme() == Theme::Mine;
    for c in &cave.chambers {
        let p = c.center();
        let (s, co) = c.yaw.sin_cos();
        let at = |u: f32, v: f32| {
            let l = Vec2::new(u * c.size[0], v * c.size[2]);
            Vec2::new(p.x + l.x * co + l.y * s, p.z - l.x * s + l.y * co)
        };
        // (glowing crystals are built with the rock: `crystals`)
        // a lantern or two (Lights): on the floor by the wall in chambers
        if feats.lights || mine {
            let a = rng.f32() * std::f32::consts::TAU;
            let dir = Vec2::from_angle(a);
            if let Some(w) = to_wall(vol, Vec2::new(p.x, p.z), dir, p.y + 0.6, c.radius() * 1.3) {
                let q = Vec2::new(p.x, p.z) + dir * (w - 0.7).max(0.4);
                if let Some(fy) = floor_at(vol, q, p.y) {
                    out.push(Prop {
                        asset: LANTERN,
                        pos: Vec3::new(q.x, fy, q.y),
                        yaw: rng.f32() * 6.28,
                        scale: 1.6,
                    });
                }
            }
        }
        // mines: crates, barrels and a pickaxe left in the rooms
        if mine {
            for _ in 0..1 + rng.below(3) {
                let q = at(rng.range(-0.5, 0.5), rng.range(-0.5, 0.5));
                let Some(fy) = floor_at(vol, q, p.y) else {
                    continue;
                };
                let asset = CRATES[rng.below(2) as usize];
                out.push(Prop {
                    asset,
                    pos: Vec3::new(q.x, fy, q.y),
                    yaw: rng.f32() * 6.28,
                    scale: 1.0,
                });
            }
            if rng.f32() < 0.5 {
                let q = at(rng.range(-0.4, 0.4), rng.range(-0.4, 0.4));
                if let Some(fy) = floor_at(vol, q, p.y) {
                    out.push(Prop {
                        asset: PICK,
                        pos: Vec3::new(q.x, fy, q.y),
                        yaw: rng.f32() * 6.28,
                        scale: 1.0,
                    });
                }
            }
        }
    }
    // a lantern by a mine's mouth
    if mine
        && let Some(t) = cave.tunnels.iter().find(|t| t.a == OUTSIDE)
        && t.pts.len() > 2
    {
        let q = Vec3::from_array(t.pts[2]);
        let d = (Vec3::from_array(t.pts[3.min(t.pts.len() - 1)]) - Vec3::from_array(t.pts[1]))
            .xz()
            .normalize_or(Vec2::Y);
        let side = Vec2::new(d.y, -d.x) * (t.width * 0.5 - 0.5);
        let q2 = Vec2::new(q.x, q.z) + side;
        if let Some(fy) = floor_at(vol, q2, q.y) {
            out.push(Prop {
                asset: LANTERN,
                pos: Vec3::new(q2.x, fy, q2.y),
                yaw: 0.0,
                scale: 1.6,
            });
        }
    }
    out
}

/// Replaces cave `id`'s objects with freshly planned ones (into the open undo step).
pub fn refurnish(d: &mut WorldData, id: u32) {
    let Some(cave) = d.caves.get(id).cloned() else {
        return;
    };
    for pid in &cave.props {
        if d.store.items.contains_key(pid) {
            d.history.touch_object(&d.store, *pid);
            d.store.remove(*pid);
        }
    }
    let mut ids = vec![];
    if cave.kind != CaveKind::Arch {
        // the rock of this cave alone (plus the dug strokes) is enough to place things on
        let one = Caves {
            list: vec![cave.clone()],
            strokes: d.caves.strokes.clone(),
            next_id: 0,
        };
        let vol = sdf::compile_probe(&one, &d.terrain);
        let lib = d.lib.as_deref();
        for pr in plan(&cave, &vol) {
            let Some(e) = lib.and_then(|l| l.get(pr.asset)) else {
                continue;
            };
            let id = d.store.new_id();
            d.history.touch_object(&d.store, id);
            let mut p = Placed {
                id,
                asset: e.id.clone(),
                variant: 0,
                pos: pr.pos.to_array(),
                rot: Quat::from_rotation_y(pr.yaw).to_array(),
                scale: 1.0,
                stretch: None,
                group: 0,
                locked: false,
            };
            p.set_scale3(Vec3::splat(pr.scale * e.default_scale));
            d.store.insert(p);
            ids.push(id);
        }
    }
    if let Some(c) = d.caves.get_mut(id) {
        c.props = ids;
    }
}

/// Removes a cave's objects (into the open undo step).
pub fn unfurnish(d: &mut WorldData, cave: &Cave) {
    for pid in &cave.props {
        if d.store.items.contains_key(pid) {
            d.history.touch_object(&d.store, *pid);
            d.store.remove(*pid);
        }
    }
}

// ------------------------------------------------------------------ meshes

/// Vertex data of the dressing built with the rock (`colors`: only where the material reads
/// them — water, crystals, webs; see `shaders/cave_fx.wesl`).
#[derive(Default, Clone, Debug)]
pub struct DressMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// A light the dressing gives off (a crystal cluster's glow).
#[derive(Clone, Copy, Debug)]
pub struct DressLight {
    pub pos: Vec3,
    /// linear rgb
    pub color: [f32; 3],
    pub lumens: f32,
    pub range: f32,
}

/// Everything built with a cave's rock besides the rock itself.
#[derive(Default, Clone, Debug)]
pub struct Dress {
    pub timbers: DressMesh,
    pub water: DressMesh,
    pub crystals: DressMesh,
    pub webs: DressMesh,
    pub lights: Vec<DressLight>,
    /// waterfalls, and the level of the pool each lands in
    pub falls: Vec<(crate::water::waterfall::Waterfall, Option<f32>)>,
}

/// The dressing of one cave (after its rock and daylight are built).
pub fn dress(cave: &Cave, vol: &Volume, light: &super::mesher::Light) -> Dress {
    if cave.kind == CaveKind::Arch {
        return Dress::default();
    }
    let (crystals, lights) = crystals(cave, vol);
    let water = water(cave, vol, light);
    let falls = falls(cave);
    Dress {
        timbers: timbers(cave, vol),
        water,
        crystals,
        webs: webs(cave, vol, light),
        lights,
        falls,
    }
}

impl DressMesh {
    /// A beam (box) from `a` to `b`, `w` wide and `h` deep, `side` the direction of its width;
    /// UVs in metres (u along the beam).
    fn beam(&mut self, a: Vec3, b: Vec3, w: f32, h: f32, side: Vec3) {
        let len_v = b - a;
        let len = len_v.length();
        if len < 1e-3 {
            return;
        }
        let ax = len_v / len;
        let s = (side - ax * side.dot(ax)).normalize_or(ax.any_orthonormal_vector());
        let u = ax.cross(s).normalize();
        let (hw, hh) = (w * 0.5, h * 0.5);
        // four long faces and two ends
        let faces: [(Vec3, Vec3, f32); 4] = [(s, u, hh), (u, -s, hw), (-s, -u, hh), (-u, s, hw)];
        for (n, t, half_t) in faces {
            let off = n * if n == s || n == -s { hw } else { hh };
            let base = self.positions.len() as u32;
            for (k, (p, along)) in [(a, 0.0), (b, len)].into_iter().enumerate() {
                for sgn in [-1.0f32, 1.0] {
                    let q = p + off + t * half_t * sgn;
                    self.positions.push(q.to_array());
                    self.normals.push(n.to_array());
                    self.uvs
                        .push([along, (sgn * 0.5 + 0.5) * half_t * 2.0 + k as f32 * 0.0]);
                }
            }
            self.indices.extend_from_slice(&[
                base,
                base + 2,
                base + 1,
                base + 1,
                base + 2,
                base + 3,
            ]);
        }
        for (p, n) in [(a, -ax), (b, ax)] {
            let base = self.positions.len() as u32;
            for (i, j) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let q = p + s * hw * i + u * hh * j;
                self.positions.push(q.to_array());
                self.normals.push(n.to_array());
                self.uvs.push([(i * 0.5 + 0.5) * w, (j * 0.5 + 0.5) * h]);
            }
            if n == ax {
                self.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base + 1,
                    base + 3,
                    base + 2,
                ]);
            } else {
                self.indices.extend_from_slice(&[
                    base,
                    base + 2,
                    base + 1,
                    base + 1,
                    base + 2,
                    base + 3,
                ]);
            }
        }
    }
}

/// Timber sets along a mine's drifts (every ~2.6 m: two posts and a cap, set to the floor and
/// roof there) and a heavier portal at its mouth.
pub fn timbers(cave: &Cave, vol: &Volume) -> DressMesh {
    let mut m = DressMesh::default();
    if !(cave.kind == CaveKind::Mine || cave.theme() == Theme::Mine) {
        return m;
    }
    let mut rng = Rng::new(cave.seed as u64 ^ 0x7177_ba5e);
    for t in &cave.tunnels {
        let pts: Vec<Vec3> = t.pts.iter().map(|p| Vec3::from_array(*p)).collect();
        let mut along = 0.0;
        let gap = 2.6;
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        // the way in: the portal where the rock roof begins (a cut in the slope before it)
        let mouth = if t.a == OUTSIDE {
            let mut s = 0.0;
            let mut found = None;
            while s < total.min(30.0) {
                let q = point_along(&pts, s);
                let floor = floor_at(vol, q.xz(), q.y).unwrap_or(q.y);
                let mut y = floor + 1.5;
                while y < floor + t.height * 1.3 && vol.at(Vec3::new(q.x, y, q.z)).d > 0.0 {
                    y += 0.15;
                }
                if y < floor + t.height * 1.3 && y < vol.ground.h(q.x, q.z) - 0.3 {
                    found = Some(s);
                    break;
                }
                s += 0.5;
            }
            found.unwrap_or(2.5)
        } else {
            0.0
        };
        let mut next = if t.a == OUTSIDE { mouth + 0.4 } else { 4.0 };
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let seg = a.distance(b);
            while next <= along + seg && next < total - 3.0 {
                let f = (next - along) / seg.max(1e-3);
                let c = a.lerp(b, f);
                let dir = (b - a).with_y(0.0).normalize_or(Vec3::Z);
                let side = Vec3::new(dir.z, 0.0, -dir.x);
                let portal = t.a == OUTSIDE && next < mouth + 1.0;
                let (post, cap) = if portal { (0.32, 0.38) } else { (0.22, 0.26) };
                // the drift's walls and roof here
                let floor = floor_at(vol, c.xz(), c.y).unwrap_or(c.y);
                let mid = Vec3::new(c.x, floor + 1.4, c.z);
                let wl = to_wall(vol, mid.xz(), side.xz(), mid.y, t.width).unwrap_or(t.width * 0.5);
                let wr =
                    to_wall(vol, mid.xz(), -side.xz(), mid.y, t.width).unwrap_or(t.width * 0.5);
                let roof = {
                    let mut y = mid.y;
                    while y < floor + t.height + 2.0 && vol.at(Vec3::new(c.x, y, c.z)).d > 0.0 {
                        y += 0.1;
                    }
                    y
                };
                let top = (roof - cap * 0.5).min(floor + t.height * 1.05);
                if top - floor < 1.9 || wl + wr < 1.6 {
                    next += gap;
                    continue;
                }
                let lean = rng.range(-0.04, 0.04);
                let pl = Vec3::new(c.x, 0.0, c.z) + side * (wl - post * 0.5 - 0.02);
                let pr = Vec3::new(c.x, 0.0, c.z) - side * (wr - post * 0.5 - 0.02);
                m.beam(
                    pl.with_y(floor - 0.1),
                    (pl + side * lean).with_y(top),
                    post,
                    post,
                    dir,
                );
                m.beam(
                    pr.with_y(floor - 0.1),
                    (pr - side * lean).with_y(top),
                    post,
                    post,
                    dir,
                );
                m.beam(
                    (pl + side * 0.25).with_y(top + cap * 0.3),
                    (pr - side * 0.25).with_y(top + cap * 0.3),
                    cap,
                    cap,
                    dir,
                );
                if portal {
                    // braces from the posts up to the cap
                    m.beam(
                        pl.with_y(top - 0.9),
                        (pl - side * 0.7).with_y(top),
                        0.16,
                        0.16,
                        dir,
                    );
                    m.beam(
                        pr.with_y(top - 0.9),
                        (pr + side * 0.7).with_y(top),
                        0.16,
                        0.16,
                        dir,
                    );
                }
                next += gap * rng.range(0.85, 1.15);
            }
            along += seg;
        }
    }
    m
}

/// The point `s` metres along a polyline.
fn point_along(pts: &[Vec3], s: f32) -> Vec3 {
    let mut left = s;
    for w in pts.windows(2) {
        let l = w[0].distance(w[1]);
        if left <= l {
            return w[0].lerp(w[1], left / l.max(1e-3));
        }
        left -= l;
    }
    *pts.last().unwrap()
}

/// The floor under `q` below `top` (sphere-stepping down through the air; None when `top`
/// itself is in rock or no floor within `reach`).
fn floor_below(vol: &Volume, q: Vec2, top: f32, reach: f32) -> Option<f32> {
    let mut y = top;
    let mut d = vol.at(Vec3::new(q.x, y, q.y)).d;
    if d <= 0.0 {
        return None;
    }
    while y > top - reach {
        if d < 0.02 {
            return Some(y);
        }
        y -= d.clamp(0.04, 1.0);
        d = vol.at(Vec3::new(q.x, y, q.y)).d;
    }
    None
}

/// Still water in the pools: a surface at the level the basin holds (a hand under its lowest
/// rim), over the basin's floor only (flooded out from its middle, so it never spills over the
/// chamber's floor), each vertex carrying the daylight over it and the water's depth under it.
pub fn water(cave: &Cave, vol: &Volume, light: &super::mesher::Light) -> DressMesh {
    let mut m = DressMesh::default();
    for c in cave.chambers.iter().filter(|c| c.pool) {
        let Some((centre, r)) = pool_of(cave, c) else {
            continue;
        };
        let level = pool_level(c);
        // the grid over the basin: depth at each vertex
        let step = if r > 8.0 { 0.8 } else { 0.4 };
        let half = r * 1.35;
        let n = ((half * 2.0 / step).ceil() as usize) + 1;
        let o = centre - Vec2::splat(half);
        let depth: Vec<f32> = (0..n * n)
            .map(|i| {
                let q = o + Vec2::new((i % n) as f32, (i / n) as f32) * step;
                floor_below(vol, q, level + 0.6, 5.0)
                    .map(|f| level - f)
                    .unwrap_or(-1.0)
            })
            .collect();
        // flood out from the middle over the wet vertices
        let mut wet = vec![false; n * n];
        let mid = (n / 2) * n + n / 2;
        let mut stack = if depth[mid] > 0.0 { vec![mid] } else { vec![] };
        while let Some(i) = stack.pop() {
            if wet[i] || depth[i] <= 0.0 {
                continue;
            }
            wet[i] = true;
            let (x, z) = (i % n, i / n);
            if x > 0 {
                stack.push(i - 1);
            }
            if x + 1 < n {
                stack.push(i + 1);
            }
            if z > 0 {
                stack.push(i - n);
            }
            if z + 1 < n {
                stack.push(i + n);
            }
        }
        // every grid square touching wet water (past the shore the surface runs on under the
        // rising floor, hidden by it)
        let mut index = vec![u32::MAX; n * n];
        let mut vertex = |i: usize, m: &mut DressMesh| -> u32 {
            if index[i] == u32::MAX {
                let q = o + Vec2::new((i % n) as f32, (i / n) as f32) * step;
                let p = Vec3::new(q.x, level, q.y);
                index[i] = m.positions.len() as u32;
                m.positions.push(p.to_array());
                m.normals.push([0.0, 1.0, 0.0]);
                m.uvs.push([q.x, q.y]);
                m.colors
                    .push([light.at(p + Vec3::Y * 0.6), depth[i].max(0.0), 0.0, 0.0]);
            }
            index[i]
        };
        for z in 0..n - 1 {
            for x in 0..n - 1 {
                let q = [
                    z * n + x,
                    z * n + x + 1,
                    (z + 1) * n + x + 1,
                    (z + 1) * n + x,
                ];
                if !q.iter().any(|i| wet[*i]) {
                    continue;
                }
                let [a, b, cc, d] = q.map(|i| vertex(i, &mut m));
                // (wound to face up)
                m.indices.extend_from_slice(&[a, d, b, b, d, cc]);
            }
        }
    }
    m
}

/// Where a waterfall pours into a tall chamber's pool (Features::waterfalls): the lip high in
/// the wall over the pool (a notch is carved there, `sdf`), the way it pours (into the chamber),
/// where it lands, its width. From the recipe alone, so the rock and the water agree.
#[derive(Clone, Copy, Debug)]
pub struct FallSite {
    pub lip: Vec3,
    pub out: Vec2,
    pub land: Vec3,
    pub width: f32,
    /// the level of the pool it lands in
    pub level: f32,
}

pub fn fall_sites(cave: &Cave) -> Vec<FallSite> {
    let mut out = vec![];
    if !cave.features.waterfalls || cave.kind == CaveKind::Mine {
        return out;
    }
    for c in cave.chambers.iter().filter(|c| c.pool && c.size[1] >= 9.0) {
        let Some((pc, pr)) = pool_of(cave, c) else {
            continue;
        };
        let mid = Vec2::new(c.pos[0], c.pos[2]);
        let u = (pc - mid).normalize_or(Vec2::X);
        // the dome's wall that way at the lip's height (the chamber's ellipsoid: widest at
        // 0.38 of its height)
        let (h, ry) = (c.size[1], c.size[1] * 0.62);
        let lip_h = h * if c.hall { 0.5 } else { 0.62 };
        let r_dir = super::model::floor_reach(c.size, c.yaw, u) / 0.79;
        let k = (lip_h - (h - ry)) / ry;
        let wall = r_dir * (1.0 - k * k).max(0.0).sqrt();
        let lip = mid + u * (wall - 0.6);
        let land = pc + u * pr * 0.2;
        let mut r = Rng::new(cave.seed as u64 ^ (c.id as u64) << 8 ^ 0xfa11);
        let width = r.range(1.2, 2.4)
            * if c.hall {
                2.2
            } else {
                cave.size.scale().max(1.0).sqrt()
            };
        out.push(FallSite {
            lip: Vec3::new(lip.x, c.pos[1] + lip_h, lip.y),
            out: -u,
            land: Vec3::new(land.x, c.pos[1], land.y),
            width,
            level: pool_level(c),
        });
    }
    out
}

/// The falling water of each site, as the water module draws waterfalls: the path arcs out of
/// the lip and lands in the pool (at the pool's level).
pub fn falls(cave: &Cave) -> Vec<(crate::water::waterfall::Waterfall, Option<f32>)> {
    fall_sites(cave)
        .into_iter()
        .enumerate()
        .map(|(i, f)| {
            let foot_y = f.level;
            let drop = (f.lip.y - foot_y).max(1.0);
            let horiz = Vec2::new(f.land.x - f.lip.x, f.land.z - f.lip.z)
                .length()
                .max(0.5);
            // launched over the lip fast enough to reach the pool
            let t_fall = (2.0 * drop / 9.81).sqrt();
            let v0 = horiz / t_fall;
            let n = 24;
            let path: Vec<[f32; 3]> = (0..n)
                .map(|k| {
                    let s = horiz * k as f32 / (n - 1) as f32;
                    let q = Vec2::new(f.lip.x, f.lip.z) + f.out * s;
                    let y = f.lip.y - 0.5 * 9.81 * (s / v0).powi(2);
                    [q.x, y.max(foot_y), q.y]
                })
                .collect();
            let mut style = crate::water::presets::preset_style("mountain_stream");
            // (white water catching what light there is: a cool glow that reads in the dark —
            // the fall's own shading only knows the sun and sky, which don't reach in here)
            style.glow = [0.55, 0.62, 0.68];
            style.glow_strength = 0.5 * cave.darkness.fill().max(0.3);
            let wf = crate::water::waterfall::Waterfall {
                id: 900_000 + cave.id * 16 + i as u32,
                name: format!("{} {}", cave.name, i + 1),
                path,
                width: f.width,
                flow: 1.2,
                mist: 0.55,
                style,
                pool: None,
            };
            (wf, Some(f.level))
        })
        .collect()
}

/// Glow colours by theme (linear rgb): a cave's clusters pick among them.
fn crystal_colours(theme: Theme) -> &'static [[f32; 3]] {
    match theme {
        Theme::Crystal => &[
            [0.2, 0.75, 1.0],
            [0.6, 0.3, 1.0],
            [1.0, 0.3, 0.75],
            [0.3, 1.0, 0.85],
        ],
        Theme::Ice => &[[0.35, 0.75, 1.0], [0.55, 0.9, 1.0]],
        Theme::Volcanic => &[[1.0, 0.42, 0.08], [1.0, 0.68, 0.18]],
        Theme::Mossy => &[[0.25, 1.0, 0.55], [0.2, 0.85, 0.85]],
        Theme::Sandstone => &[[1.0, 0.75, 0.3], [1.0, 0.55, 0.35]],
        _ => &[[0.3, 0.7, 1.0], [0.5, 0.35, 1.0], [0.25, 0.95, 0.8]],
    }
}

/// Glowing crystal clusters (Features::crystals): hexagonal prisms with pointed tips sprouting
/// in sheaves from the walls, the floor's edge and the roofs of the chambers — waist-high in
/// small chambers, towering in great halls — each cluster one colour, lighting its chamber.
/// (Vertex colour: glow colour and strength; uv: along the crystal, facet.)
pub fn crystals(cave: &Cave, vol: &Volume) -> (DressMesh, Vec<DressLight>) {
    let mut m = DressMesh::default();
    let mut lights = vec![];
    if !cave.features.crystals {
        return (m, lights);
    }
    let mut rng = Rng::new(cave.seed as u64 ^ 0xc4f5_7a15);
    let lanes = sdf::lanes(cave);
    let in_the_way = |q: Vec2, r: f32| {
        lanes
            .iter()
            .any(|(a, b, half)| sdf::seg_dist(q, *a, *b) < half + r + 0.5)
    };
    let palette = crystal_colours(cave.theme());
    let scale = cave.size.scale().max(1.0);
    for c in &cave.chambers {
        let p = c.center();
        let (rx, h, rz) = (c.size[0], c.size[1], c.size[2]);
        let cs = if c.hall {
            (rx.min(rz) / 12.0).clamp(2.5, 5.0)
        } else {
            scale
        };
        let n = if c.hall {
            ((rx * rz / 150.0) as usize).clamp(5, 14)
        } else {
            ((rx * rz / 45.0) as usize).clamp(2, 7)
        };
        for _ in 0..n {
            // a spot on the rock: along a ray from the chamber's middle (low to a wall, or up to
            // the roof)
            let a = rng.f32() * std::f32::consts::TAU;
            let up = rng.f32() < 0.3;
            let dir = if up {
                Vec3::new(a.cos() * 0.35, 1.0, a.sin() * 0.35).normalize()
            } else {
                Vec3::new(a.cos(), rng.range(-0.15, 0.25), a.sin()).normalize()
            };
            let from = p + Vec3::Y
                * if up {
                    h * 0.4
                } else {
                    rng.range(0.6, 1.6) * cs.sqrt()
                };
            let mut t = 0.0;
            let mut hit = None;
            while t < rx.max(rz) * 1.6 + h {
                let q = from + dir * t;
                let d = vol.at(q).d;
                if d < 0.05 {
                    hit = Some(q);
                    break;
                }
                t += d.clamp(0.1, 1.5);
            }
            let Some(base) = hit else { continue };
            let nrm = vol.normal(base, &vol.shapes_near(base, base, 1.0), 0.15);
            // (great halls: towering ones, the first two of them giants)
            let size = if c.hall {
                rng.range(1.0, 2.2) * cs * if lights.len() < 2 { 2.2 } else { 1.0 }
            } else {
                rng.range(0.6, 1.5) * cs
            };
            if in_the_way(base.xz(), size * 0.8) && base.y < p.y + 3.2 {
                continue;
            }
            let colour = palette[rng.below(palette.len() as u32) as usize];
            let alt = palette[rng.below(palette.len() as u32) as usize];
            // a sheaf: a big one and smaller ones round it, fanning out from the rock
            let count = if c.hall {
                6 + rng.below(9)
            } else {
                4 + rng.below(8)
            };
            for k in 0..count {
                let spread = if k == 0 { 0.12 } else { rng.range(0.25, 0.75) };
                let jitter = Vec3::new(
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                    rng.range(-1.0, 1.0),
                ) * spread;
                let axis = (nrm + jitter).normalize_or(nrm);
                let len = size
                    * if k == 0 {
                        rng.range(1.2, 1.7)
                    } else {
                        rng.range(0.35, 1.1)
                    };
                let rad = len * rng.range(0.08, 0.15);
                let off = if k == 0 {
                    Vec3::ZERO
                } else {
                    (jitter - nrm * jitter.dot(nrm)) * size * 0.6
                };
                let foot = base + off - axis * rad * 1.5;
                let tint = colour.map(|x| x);
                let mix = rng.range(0.0, 0.35);
                let col = [
                    tint[0] + (alt[0] - tint[0]) * mix,
                    tint[1] + (alt[1] - tint[1]) * mix,
                    tint[2] + (alt[2] - tint[2]) * mix,
                ];
                prism(
                    &mut m,
                    foot,
                    axis,
                    len,
                    rad,
                    rng.f32() * 6.28,
                    [col[0], col[1], col[2], rng.range(0.7, 1.15)],
                );
            }
            if lights.len() < if c.hall { 40 } else { 24 } {
                // (lumens for the eye adjusted to a cave, `sky::CAVE_EV`: enough to throw the
                // crystal's colour on the rock round it)
                lights.push(DressLight {
                    pos: base + nrm * size * 0.9,
                    color: colour,
                    lumens: (350_000.0 * size * size).clamp(300_000.0, 6_000_000.0),
                    range: 10.0 + size * 6.0,
                });
            }
        }
    }
    (m, lights)
}

/// One crystal: a six-sided prism from `foot` along `axis`, a pointed tip over its last fifth;
/// flat faces (each face its own vertices).
fn prism(m: &mut DressMesh, foot: Vec3, axis: Vec3, len: f32, rad: f32, turn: f32, glow: [f32; 4]) {
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    let ring = |k: usize, along: f32, r: f32| -> Vec3 {
        let a = turn + k as f32 / 6.0 * std::f32::consts::TAU;
        foot + axis * along + (u * a.cos() + v * a.sin()) * r
    };
    let body = len * 0.8;
    let tip = foot + axis * len;
    for k in 0..6 {
        let (k1, f) = ((k + 1) % 6, k as f32);
        // the side face
        let (a, b, c, d) = (
            ring(k, 0.0, rad),
            ring(k1, 0.0, rad),
            ring(k1, body, rad * 0.92),
            ring(k, body, rad * 0.92),
        );
        let nrm = (b - a).cross(d - a).normalize_or(axis);
        let base = m.positions.len() as u32;
        for (p, along) in [(a, 0.0), (b, 0.0), (c, 0.8), (d, 0.8)] {
            m.positions.push(p.to_array());
            m.normals.push(nrm.to_array());
            m.uvs.push([along, f]);
            m.colors.push(glow);
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        // the tip's facet
        let tn = (c - d).cross(tip - d).normalize_or(axis);
        let base = m.positions.len() as u32;
        for (p, along) in [(d, 0.8), (c, 0.8), (tip, 1.0)] {
            m.positions.push(p.to_array());
            m.normals.push(tn.to_array());
            m.uvs.push([along, f]);
            m.colors.push(glow);
        }
        m.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
}

/// Cobwebs (Features::cobwebs): sheets of web across the upper corners of the tunnels and the
/// chambers' wall heads, and right across the dead-end side passages. Quads in the texture's
/// space (`web_texture`); the rock hides what reaches past the corner.
pub fn webs(cave: &Cave, vol: &Volume, light: &super::mesher::Light) -> DressMesh {
    let mut m = DressMesh::default();
    if !cave.features.cobwebs {
        return m;
    }
    let mut rng = Rng::new(cave.seed as u64 ^ 0x0c0b_3eb5);
    let quad = |m: &mut DressMesh, c: Vec3, right: Vec3, up: Vec3, rot: f32| {
        let (s, co) = rot.sin_cos();
        let (r2, u2) = (right * co + up * s, up * co - right * s);
        let nrm = r2.cross(u2).normalize_or(Vec3::Y);
        let day = light.at(c);
        let base = m.positions.len() as u32;
        for (i, j) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            m.positions.push((c + r2 * i + u2 * j).to_array());
            m.normals.push(nrm.to_array());
            m.uvs.push([i * 0.5 + 0.5, 0.5 - j * 0.5]);
            m.colors.push([day, 0.0, 0.0, 0.0]);
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    // the distance to the rock from `p` along `d`
    let to_rock = |p: Vec3, d: Vec3, max: f32| -> Option<f32> {
        let mut t = 0.0;
        while t < max {
            let dd = vol.at(p + d * t).d;
            if dd < 0.03 {
                return Some(t);
            }
            t += dd.clamp(0.05, 1.0);
        }
        None
    };
    // a web across the corner where a wall meets the roof, seen from `p` (the middle of the
    // space at about head height) looking out along `side`, the way ahead `ahead`
    let corner =
        |m: &mut DressMesh, p: Vec3, side: Vec3, ahead: Vec3, reach: f32, rng: &mut Rng| {
            let diag = (side + Vec3::Y).normalize();
            let Some(t) = to_rock(p, diag, reach) else {
                return;
            };
            let at = p + diag * (t - rng.range(0.25, 0.6));
            let size = (t * rng.range(0.25, 0.4)).clamp(0.5, 2.2);
            // the sheet across the corner: square to the way ahead, tipped in from the corner
            let right = (side - Vec3::Y).normalize();
            let up = ahead.normalize().cross(right).normalize_or(Vec3::Y);
            let (r, u) = if up.y < 0.0 {
                (right, -up)
            } else {
                (right, up)
            };
            quad(m, at, r * size, u * size, rng.range(-0.5, 0.5));
        };
    for t in &cave.tunnels {
        let pts: Vec<Vec3> = t.pts.iter().map(|p| Vec3::from_array(*p)).collect();
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        // a dead end: a side passage to a chamber nothing else leads on from
        let dead = [t.a, t.b].iter().any(|id| {
            *id != OUTSIDE
                && cave
                    .tunnels
                    .iter()
                    .filter(|o| o.a == *id || o.b == *id)
                    .count()
                    == 1
        }) && cave
            .dungeon
            .as_ref()
            .is_none_or(|d| d.chamber != t.a && d.chamber != t.b);
        let mut s = rng.range(6.0, 12.0);
        let mut across_done = false;
        while s < total - 4.0 {
            // (never near the way in: the daylight end stays clear)
            if t.a == OUTSIDE && s < 18.0 {
                s += 6.0;
                continue;
            }
            let (q, ahead) = point_dir(&pts, s);
            let side = Vec3::new(-ahead.z, 0.0, ahead.x).normalize_or(Vec3::X);
            let Some(floor) = floor_below(vol, q.xz(), q.y + 1.2, 3.0) else {
                s += 4.0;
                continue;
            };
            let mid = Vec3::new(q.x, floor + t.height * 0.55, q.z);
            if vol.at(mid).d <= 0.0 {
                s += 4.0;
                continue;
            }
            for sgn in [1.0f32, -1.0] {
                if rng.f32() < 0.55 {
                    corner(&mut m, mid, side * sgn, ahead, t.width + t.height, &mut rng);
                }
            }
            // right across a dead-end passage, once
            if dead && !across_done && s > total * 0.4 {
                let (l, r, up) = (
                    to_rock(mid, -side, t.width),
                    to_rock(mid, side, t.width),
                    to_rock(mid, Vec3::Y, t.height * 2.0),
                );
                if let (Some(l), Some(r), Some(up)) = (l, r, up) {
                    let half_w = (l + r) * 0.5 + 0.3;
                    let half_h = (up + (mid.y - floor)) * 0.5 + 0.3;
                    let c = mid + side * (r - l) * 0.5 + Vec3::Y * (up - (mid.y - floor)) * 0.5;
                    quad(
                        &mut m,
                        c,
                        side * half_w,
                        Vec3::Y * half_h,
                        rng.range(-0.3, 0.3),
                    );
                    across_done = true;
                }
            }
            s += rng.range(7.0, 14.0);
        }
    }
    // the chambers' wall heads
    for c in &cave.chambers {
        let p = c.center();
        for _ in 0..2 + rng.below(4) {
            let a = rng.f32() * std::f32::consts::TAU;
            let side = Vec3::new(a.cos(), 0.0, a.sin());
            let mid = p + Vec3::Y * (c.size[1] * 0.5).min(4.0);
            corner(
                &mut m,
                mid,
                side,
                Vec3::new(-side.z, 0.0, side.x),
                c.radius() * 1.4,
                &mut rng,
            );
        }
    }
    m
}

/// The point `s` metres along a polyline and the way it runs there.
fn point_dir(pts: &[Vec3], s: f32) -> (Vec3, Vec3) {
    let mut left = s;
    for w in pts.windows(2) {
        let l = w[0].distance(w[1]);
        if left <= l {
            return (
                w[0].lerp(w[1], left / l.max(1e-3)),
                (w[1] - w[0]).normalize_or(Vec3::Z),
            );
        }
        left -= l;
    }
    let n = pts.len();
    (
        pts[n - 1],
        (pts[n - 1] - pts[n.saturating_sub(2)]).normalize_or(Vec3::Z),
    )
}

/// A cobweb (RGBA, `size`²): spokes from a hub, a capture spiral strung between them in
/// straight runs, a few broken and loose threads; fading out at the edge.
pub fn web_texture(size: u32, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let n = size as usize;
    let mut a = vec![0.0f32; n * n];
    let px = 1.0 / size as f32;
    let line = |a: &mut Vec<f32>, p: Vec2, q: Vec2, w: f32, k: f32| {
        let (lo, hi) = (
            p.min(q) - Vec2::splat(w * 3.0),
            p.max(q) + Vec2::splat(w * 3.0),
        );
        let (x0, x1) = (
            ((lo.x * size as f32) as i32).max(0),
            ((hi.x * size as f32) as i32 + 1).min(size as i32 - 1),
        );
        let (y0, y1) = (
            ((lo.y * size as f32) as i32).max(0),
            ((hi.y * size as f32) as i32 + 1).min(size as i32 - 1),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let c = Vec2::new((x as f32 + 0.5) * px, (y as f32 + 0.5) * px);
                let d = sdf::seg_dist(c, p, q);
                let v = (1.0 - d / w).clamp(0.0, 1.0) * k;
                let i = y as usize * n + x as usize;
                a[i] = a[i].max(v);
            }
        }
    };
    let hub = Vec2::new(0.5, 0.5) + Vec2::new(rng.range(-0.04, 0.04), rng.range(-0.04, 0.04));
    let spokes = 13 + rng.below(5) as usize;
    let mut ang: Vec<f32> = (0..spokes)
        .map(|k| (k as f32 + rng.range(-0.3, 0.3)) / spokes as f32 * std::f32::consts::TAU)
        .collect();
    ang.sort_by(|a, b| a.total_cmp(b));
    let reach: Vec<f32> = ang.iter().map(|_| rng.range(0.4, 0.49)).collect();
    let w = px * 1.1;
    for (k, a0) in ang.iter().enumerate() {
        let end = hub + Vec2::from_angle(*a0) * reach[k];
        line(&mut a, hub, end, w, 0.9);
    }
    // the capture spiral: straight threads between neighbouring spokes, round and out
    let turns = 18 + rng.below(8);
    let steps = turns as usize * spokes;
    for i in 0..steps {
        let (k, k1) = (i % spokes, (i + 1) % spokes);
        let f = |j: usize| 0.07 + (0.42 - 0.07) * j as f32 / steps as f32;
        let (r0, r1) = (f(i).min(reach[k] - 0.01), f(i + 1).min(reach[k1] - 0.01));
        let p = hub + Vec2::from_angle(ang[k]) * r0;
        let q = hub
            + Vec2::from_angle(ang[k1] + if k1 == 0 { std::f32::consts::TAU } else { 0.0 }) * r1;
        // a few broken threads
        if rng.f32() < 0.06 {
            continue;
        }
        line(&mut a, p, q, w * 0.85, 0.75);
    }
    // loose strands
    for _ in 0..4 {
        let p = Vec2::new(rng.range(0.1, 0.9), rng.range(0.1, 0.9));
        let q = p + Vec2::from_angle(rng.f32() * 6.28) * rng.range(0.1, 0.3);
        line(&mut a, p, q, w, 0.5);
    }
    let mut out = Vec::with_capacity(n * n * 4);
    for (i, v) in a.iter().enumerate() {
        let c = Vec2::new((i % n) as f32 + 0.5, (i / n) as f32 + 0.5) * px;
        let edge = 1.0 - ((c - hub).length() / 0.5 - 0.88).clamp(0.0, 0.12) / 0.12;
        let g = 200 + (v * 40.0) as u8 / 2;
        out.extend_from_slice(&[g, g, (g as f32 * 0.97) as u8, (v * edge * 255.0) as u8]);
    }
    out
}
