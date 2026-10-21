//! Laying a cave out: from the entrance (the click), the heading into the hill and the recipe
//! (kind, size, levels, seed), the chambers and the tunnels between them.
//!
//! - The way in depends on the ground: a hillside gets a mouth with a short entry tunnel running
//!   level into the hill, flat ground a pit (caves) or a sloping decline (mines); a passage runs
//!   right through to where the ground falls away again; a hidden cavern starts deep down.
//! - The main way wanders on from chamber to chamber, going down a level at a time: a level
//!   change turns back under the level above (stacked levels), its tunnel no steeper than a
//!   walkable ramp. Side branches end in dead-end chambers; bigger caves get a loop.
//! - Every chamber keeps rock above it (`COVER` under the ground surface), so nothing pokes out of
//!   the hill by accident; chambers never overlap.
//! - Locked chambers survive Shuffle: the new layout grows around them and joins them up.
//!
//! Pure and deterministic for a given terrain, recipe and seed.

use bevy::math::{Vec2, Vec3};

use super::model::*;
use crate::noise::Rng;
use crate::terrain::Terrain;

/// Rock kept between a chamber's roof and the ground above (m).
pub const COVER: f32 = 4.5;
/// Steepest a ramp is laid out (about 27°): comfortably walkable both ways.
pub const RAMP_SLOPE: f32 = 0.5;

/// How far from a chamber's middle its floor runs level towards `dir` before a tunnel ramps
/// away: to just short of its wall, so no ramp hangs inside it (a lobe reaching further gets a
/// short bank of scree under the ramp, `sdf::walk_supports`).
fn level_reach(size: [f32; 3], yaw: f32, dir: Vec2) -> f32 {
    (floor_reach(size, yaw, dir) - 0.6).max(size[0].min(size[2]) * 0.55)
}

/// Horizontal unit vector of a heading (0 = +z, turning towards +x).
pub fn dir(heading: f32) -> Vec2 {
    Vec2::new(heading.sin(), heading.cos())
}

/// The heading into the hill at a point: straight uphill (any heading on flat ground: away from
/// `toward`, the camera, so the cave runs away from the viewer).
pub fn uphill(t: &Terrain, p: Vec2, toward: Vec2) -> (f32, f32) {
    let n = t.normal_at(p.x, p.y);
    let up = -Vec2::new(n.x, n.z);
    let slope = (1.0 - n.y * n.y).max(0.0).sqrt() / n.y.max(0.05);
    let d = if up.length() > 0.08 {
        up.normalize()
    } else {
        (p - toward).normalize_or(Vec2::Y)
    };
    (d.x.atan2(d.y), slope)
}

/// Is angle `a` within `lo`..`hi` (radians, going round from `lo`)?
fn angle_in(a: f32, lo: f32, hi: f32) -> bool {
    let tau = std::f32::consts::TAU;
    let span = (hi - lo).rem_euclid(tau);
    (a - lo).rem_euclid(tau) <= span || hi - lo >= tau
}

/// Lowest ground over a disc (a few samples): the roof of a chamber there must stay under it.
fn ground_min(t: &Terrain, c: Vec2, r: f32) -> f32 {
    let mut lo = t.height_at(c.x, c.y);
    for k in 0..12 {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        for f in [0.55, 1.0] {
            let q = c + Vec2::from_angle(a) * r * f;
            lo = lo.min(t.height_at(q.x, q.y));
        }
    }
    lo
}

struct Builder<'a> {
    t: &'a Terrain,
    rng: Rng,
    cave: Cave,
    next: u16,
    scale: f32,
    mine: bool,
    /// ledges and bridges: a level change happens inside a tall chamber (`Features::bridges`)
    bridges: bool,
    /// the levels climb into the hill
    up: bool,
    /// wall spans a ledge runs along (chamber, from angle, to angle): no other way leaves there
    spans: Vec<(u16, f32, f32)>,
}

/// What a step along the way makes.
#[derive(Clone, Copy, PartialEq)]
enum Room {
    Normal,
    Big,
    Hall,
}

impl Builder<'_> {
    fn new<'a>(t: &'a Terrain, cave: Cave, seed: u64, next: u16) -> Builder<'a> {
        let (scale, mine, bridges, up) = (
            cave.size.scale(),
            cave.kind == CaveKind::Mine,
            cave.features.bridges,
            cave.up,
        );
        Builder {
            t,
            rng: Rng::new(seed),
            cave,
            next,
            scale,
            mine,
            bridges,
            up,
            spans: vec![],
        }
    }

    /// A great hall (giant caves): vast, domed.
    fn hall_size(&mut self) -> [f32; 3] {
        let r = &mut self.rng;
        match self.cave.size {
            CaveSize::Colossal => [
                r.range(55.0, 68.0),
                r.range(40.0, 52.0),
                r.range(62.0, 80.0),
            ],
            _ => [
                r.range(30.0, 38.0),
                r.range(24.0, 32.0),
                r.range(34.0, 44.0),
            ],
        }
    }

    /// How much of a level change a chamber of this size takes up inside it, on a ledge round
    /// its wall (0: too low or small, or no ledges wanted).
    fn climb_in(&self, size: [f32; 3], hall: bool) -> f32 {
        // (wide enough that a ledge round the wall stays well clear of the middle, where the
        // ways through the chamber meet)
        if !self.bridges
            || self.mine
            || size[1] < 8.5
            || size[0].min(size[2]) * 0.79 < 9.5 + 2.5 * self.scale
        {
            return 0.0;
        }
        size[1] * if hall { 0.55 } else { 0.5 }
    }

    fn chamber_size(&mut self, big: bool) -> [f32; 3] {
        let s = self.scale;
        let r = &mut self.rng;
        if self.mine {
            // rooms (stopes): boxy, lower
            return [
                r.range(4.0, 6.5) * s,
                r.range(4.2, 5.6) * s,
                r.range(5.0, 9.0) * s,
            ];
        }
        if big {
            [
                r.range(10.0, 14.0) * s,
                r.range(9.0, 13.0) * s,
                r.range(12.0, 18.0) * s,
            ]
        } else {
            [
                r.range(5.0, 8.5) * s,
                r.range(5.0, 8.0) * s,
                r.range(6.0, 11.0) * s,
            ]
        }
    }

    /// Floor height for a chamber at `c` (wanted `y`): pushed down to keep `COVER` under the
    /// ground (and never above `y`).
    fn covered_floor(&self, c: Vec2, size: [f32; 3], y: f32) -> f32 {
        let roof = ground_min(self.t, c, size[0].max(size[2]) * 1.1) - COVER;
        y.min(roof - size[1])
    }

    fn overlaps(&self, c: Vec3, size: [f32; 3], skip: Option<u16>) -> bool {
        let r = size[0].max(size[2]);
        self.cave.chambers.iter().any(|o| {
            if Some(o.id) == skip {
                return false;
            }
            let p = o.center();
            let dy = (p.y - c.y).abs();
            // levels apart (one roof well below the other's floor) may overlap in plan
            let stacked = if p.y > c.y {
                c.y + size[1] + 2.5 < p.y
            } else {
                p.y + o.size[1] + 2.5 < c.y
            };
            !stacked && dy < 14.0 && Vec2::new(p.x - c.x, p.z - c.z).length() < r + o.radius() + 3.0
        })
    }

    /// A locked chamber in the way of a new one at `c` (the new layout joins it instead).
    fn locked_at(&self, c: Vec3, size: [f32; 3]) -> Option<u16> {
        let r = size[0].max(size[2]);
        self.cave
            .chambers
            .iter()
            .find(|o| {
                o.locked
                    && (o.center().y - c.y).abs() < 9.0
                    && Vec2::new(o.pos[0] - c.x, o.pos[2] - c.z).length() < r + o.radius() + 3.0
            })
            .map(|o| o.id)
    }

    /// Does a tunnel already leave chamber `id` within ~26° of `dir` (they would share a way),
    /// or a ledge run along its wall there?
    fn crowded(&self, id: u16, dir: Vec2) -> bool {
        let Some(c) = self.cave.chamber(id) else {
            return false;
        };
        let mid = Vec2::new(c.pos[0], c.pos[2]);
        let a = dir.y.atan2(dir.x);
        if self
            .spans
            .iter()
            .any(|(sid, a0, a1)| *sid == id && angle_in(a, *a0 - 0.3, *a1 + 0.3))
        {
            return true;
        }
        self.cave.tunnels.iter().any(|t| {
            let pts: Vec<Vec2> = if t.a == id {
                t.pts.iter().map(|p| Vec2::new(p[0], p[2])).collect()
            } else if t.b == id {
                t.pts.iter().rev().map(|p| Vec2::new(p[0], p[2])).collect()
            } else {
                return false;
            };
            pts.iter()
                .find(|q| q.distance(mid) > 1.5)
                .is_some_and(|q| (*q - mid).normalize_or_zero().dot(dir) > 0.9)
        })
    }

    fn linked(&self, a: u16, b: u16) -> bool {
        self.cave
            .tunnels
            .iter()
            .any(|t| (t.a == a && t.b == b) || (t.a == b && t.b == a))
    }

    /// The first chamber (a locked one standing there is kept instead): its id and floor centre.
    fn first(&mut self, pos: Vec3, size: [f32; 3], yaw: f32) -> (u16, Vec3) {
        match self.locked_at(pos, size) {
            Some(l) => (l, self.cave.chamber(l).unwrap().center()),
            None => (self.add_chamber(pos, size, yaw, 0), pos),
        }
    }

    fn add_chamber(&mut self, pos: Vec3, size: [f32; 3], yaw: f32, level: u8) -> u16 {
        let id = self.next;
        self.next += 1;
        self.cave.chambers.push(Chamber {
            id,
            pos: pos.to_array(),
            size,
            yaw,
            level,
            locked: false,
            pool: false,
            skylight: false,
            hall: false,
            rim: RimStyle::Auto,
        });
        id
    }

    /// Width and height of a tunnel (never too tight to walk: the volume also keeps a clear
    /// walking corridor through every tunnel, `sdf::WALK_W` × `sdf::WALK_H`).
    fn tunnel_dims(&mut self, profile: Profile) -> (f32, f32) {
        let s = self.scale;
        let r = &mut self.rng;
        let (w, h) = match profile {
            Profile::Drift => (r.range(3.2, 3.8), r.range(3.2, 3.6)),
            Profile::Canyon => (r.range(3.3, 4.0) * s, r.range(5.5, 8.0) * s),
            Profile::Keyhole => (r.range(4.4, 5.4) * s, r.range(5.0, 6.5) * s),
            Profile::Low => (r.range(6.0, 8.0) * s, r.range(3.1, 3.6) * s),
            Profile::Tube => (r.range(3.9, 5.2) * s, r.range(3.6, 5.0) * s),
        };
        (w.max(3.3), h.max(3.1))
    }

    fn pick_profile(&mut self) -> Profile {
        if self.mine {
            return Profile::Drift;
        }
        let x = self.rng.f32();
        match self.cave.theme() {
            Theme::Limestone | Theme::Crystal => {
                if x < 0.35 {
                    Profile::Keyhole
                } else if x < 0.55 {
                    Profile::Canyon
                } else if x < 0.7 {
                    Profile::Low
                } else {
                    Profile::Tube
                }
            }
            Theme::Sandstone => {
                if x < 0.55 {
                    Profile::Canyon
                } else {
                    Profile::Tube
                }
            }
            Theme::Volcanic => Profile::Tube,
            _ => {
                if x < 0.2 {
                    Profile::Canyon
                } else if x < 0.3 {
                    Profile::Low
                } else {
                    Profile::Tube
                }
            }
        }
    }

    /// A wandering floor line from `a` to `b` (both floor points), a point every ~6 m.
    fn wander(&mut self, a: Vec3, b: Vec3, amount: f32) -> Vec<[f32; 3]> {
        let span = Vec2::new(b.x - a.x, b.z - a.z);
        let len = span.length().max(0.1);
        let side = Vec2::new(-span.y, span.x) / len;
        let n = ((len / 6.0).ceil() as usize).max(1);
        let ph = self.rng.f32() * 10.0;
        let (f1, f2) = (self.rng.range(0.6, 1.4), self.rng.range(1.6, 2.6));
        let mut pts = vec![a.to_array()];
        for k in 1..n {
            let u = k as f32 / n as f32;
            // (fades out at both ends so the tunnel meets each chamber square on)
            let env = (u * std::f32::consts::PI).sin();
            let off = ((u * f1 * 6.28 + ph).sin() * 0.7 + (u * f2 * 6.28 + ph * 1.7).sin() * 0.3)
                * amount
                * env
                * (len / 25.0).min(1.0);
            let p = a.lerp(b, u)
                + Vec3::new(
                    side.x * off,
                    (self.rng.f32() - 0.5) * 0.4 * env,
                    side.y * off,
                );
            pts.push(p.to_array());
        }
        pts.push(b.to_array());
        pts
    }

    fn connect(&mut self, a: u16, b: u16) {
        let (ca, cb) = (
            self.cave.chamber(a).unwrap().clone(),
            self.cave.chamber(b).unwrap().clone(),
        );
        let (pa, pb) = (ca.center(), cb.center());
        // the lower chamber takes up what it can of the climb on a ledge round its wall
        let dy = pb.y - pa.y;
        let low_is_a = dy > 0.0;
        let low = if low_is_a { &ca } else { &cb };
        let inside = if dy.abs() > 2.5 {
            dy.abs().min(self.climb_in(low.size, low.hall))
        } else {
            0.0
        };
        // a ramp between levels is a wide tube (never a narrow slot) that wanders less
        let ramp = dy.abs() - inside > 3.0;
        let profile = if ramp && !self.mine {
            Profile::Tube
        } else {
            self.pick_profile()
        };
        let (mut w, h) = self.tunnel_dims(profile);
        if ramp {
            w = w.max(4.4 * self.scale.min(1.0));
        }
        let amt = if self.mine {
            0.3
        } else if ramp {
            1.2
        } else {
            2.2
        };
        // level across each chamber's floor to its wall (or round the wall on a ledge), any
        // climb or descent left between the walls
        let d = Vec2::new(pb.x - pa.x, pb.z - pa.z);
        let len = d.length();
        let u = d / len.max(1e-3);
        let (ra, rb) = (
            level_reach(ca.size, ca.yaw, u),
            level_reach(cb.size, cb.yaw, -u),
        );
        // (chambers close together: their level floors shorten to leave the way between)
        let f = ((len - 3.0) / (ra + rb)).clamp(0.2, 1.0);
        let start = if low_is_a && inside > 0.5 {
            self.ledge(&ca, u, inside, w)
        } else {
            vec![
                pa,
                Vec3::new(pa.x + u.x * ra * f, pa.y, pa.z + u.y * ra * f),
            ]
        };
        let end = if !low_is_a && inside > 0.5 {
            let mut l = self.ledge(&cb, -u, inside, w);
            l.reverse();
            l
        } else {
            vec![
                Vec3::new(pb.x - u.x * rb * f, pb.y, pb.z - u.y * rb * f),
                pb,
            ]
        };
        let mut pts: Vec<[f32; 3]> = start.iter().map(|p| p.to_array()).collect();
        let mid = self.wander(*start.last().unwrap(), end[0], amt);
        pts.extend(&mid[1..mid.len() - 1]);
        pts.extend(end.iter().map(|p| p.to_array()));
        self.cave.tunnels.push(Tunnel {
            a,
            b,
            pts,
            width: w,
            height: h,
            profile,
        });
    }

    /// A ledge climbing round chamber `c`'s wall from its floor up `climb` metres to where a
    /// tunnel leaves towards `toward` (its floor line, from the chamber's middle out to the
    /// wall): it sweeps round the side no other way leaves by, no steeper than a walkable ramp.
    /// (`sdf::walk_supports` builds the rock under it: a bank low down, a ledge leaning on the
    /// wall or a bridge higher up.)
    fn ledge(&mut self, c: &Chamber, toward: Vec2, climb: f32, w: f32) -> Vec<Vec3> {
        let mid = c.center();
        let a0 = toward.y.atan2(toward.x);
        let ring =
            |a: f32| (floor_reach(c.size, c.yaw, Vec2::from_angle(a)) - w * 0.5 - 1.2).max(2.5);
        // the other ways out of the chamber (their directions)
        let ways: Vec<f32> = self
            .cave
            .tunnels
            .iter()
            .filter_map(|t| {
                let pts: Vec<Vec2> = if t.a == c.id {
                    t.pts.iter().map(|p| Vec2::new(p[0], p[2])).collect()
                } else if t.b == c.id {
                    t.pts.iter().rev().map(|p| Vec2::new(p[0], p[2])).collect()
                } else {
                    return None;
                };
                let m = Vec2::new(mid.x, mid.z);
                pts.iter()
                    .find(|q| q.distance(m) > 1.5)
                    .map(|q| (q.y - m.y).atan2(q.x - m.x))
            })
            .collect();
        let need = climb / (RAMP_SLOPE * 0.9) + 2.0;
        // round the wall one way or the other, stopping short of any other way out (or another
        // ledge): the longer of the two
        let blocked = |a: f32| {
            ways.iter().any(|w| angle_in(a, *w - 0.4, *w + 0.4))
                || self
                    .spans
                    .iter()
                    .any(|(id, x0, x1)| *id == c.id && angle_in(a, *x0 - 0.2, *x1 + 0.2))
        };
        let sweep = |sign: f32| {
            let (mut a, mut len) = (a0, 0.0f32);
            while len < need && (a - a0).abs() < std::f32::consts::PI * 1.6 {
                let na = a + sign * 0.05;
                if (na - a0).abs() > 0.45 && blocked(na) {
                    break;
                }
                len +=
                    Vec2::from_angle(a).lerp(Vec2::from_angle(na), 0.5).length() * 0.05 * ring(a);
                a = na;
            }
            (a, len)
        };
        let (pos, neg) = (sweep(1.0), sweep(-1.0));
        let pick_pos =
            pos.1 > neg.1 + 0.5 || ((pos.1 - neg.1).abs() <= 0.5 && self.rng.f32() < 0.5);
        let (a1, len) = if pick_pos { pos } else { neg };
        let climb = climb.min(((len - 2.0) * RAMP_SLOPE * 0.9).max(0.0));
        self.spans.push((c.id, a0.min(a1), a0.max(a1)));
        // from the middle across the floor to the ledge's foot, round and up to the wall
        let n = ((len / 4.0).ceil() as usize).max(2);
        let mut out = vec![mid];
        for k in 0..=n {
            let u = k as f32 / n as f32;
            let a = a1 + (a0 - a1) * u;
            let q = Vec2::from_angle(a) * ring(a);
            // (the last two metres level, into the tunnel's mouth)
            let y = mid.y + climb * (u * len / (len - 2.0).max(0.1)).min(1.0);
            out.push(Vec3::new(mid.x + q.x, y, mid.z + q.y));
        }
        let wall = Vec2::from_angle(a0) * (floor_reach(c.size, c.yaw, Vec2::from_angle(a0)) + 0.5);
        out.push(Vec3::new(mid.x + wall.x, mid.y + climb, mid.z + wall.y));
        out
    }

    /// Next chamber along the way from `from`, heading roughly `heading`, on `level`.
    fn step(
        &mut self,
        from: u16,
        heading: f32,
        level: u8,
        base: f32,
        room: Room,
    ) -> Option<(u16, f32)> {
        let prev = self.cave.chamber(from).unwrap().clone();
        let p0 = prev.center();
        let level_change = level > prev.level;
        for attempt in 0..10 {
            let turn = if level_change {
                // go back under the level above
                let s = if self.rng.f32() < 0.5 { -1.0 } else { 1.0 };
                s * self.rng.range(1.6, 2.6)
            } else if self.mine {
                [0.0, 0.0, 0.0, 1.5708, -1.5708][self.rng.below(5) as usize]
                    + self.rng.range(-0.08, 0.08)
            } else {
                self.rng.range(-0.75, 0.75) * (1.0 + attempt as f32 * 0.3)
            };
            let hd = heading + turn;
            let hall = room == Room::Hall;
            let size = if hall {
                self.hall_size()
            } else {
                self.chamber_size(room == Room::Big && attempt < 6)
            };
            let drop = if level_change {
                11.0 * self.scale.max(1.0) + size[1] * 0.3
            } else {
                0.0
            };
            let mut dist =
                prev.radius() + size[0].max(size[2]) + self.rng.range(7.0, 14.0) * self.scale;
            // the ramp runs between where the two floors level off: never steeper than walkable
            // (a tall chamber at either end takes up part of the climb on a ledge inside it)
            let flat =
                level_reach(prev.size, prev.yaw, dir(hd)) + (size[0].min(size[2]) * 0.79 - 0.6);
            let absorb = self
                .climb_in(prev.size, prev.hall)
                .max(self.climb_in(size, hall));
            if level_change {
                dist = dist.max((drop - absorb).max(0.0) / RAMP_SLOPE + flat + 2.0);
            }
            let c2 = Vec2::new(p0.x, p0.z) + dir(hd) * dist;
            if !self.t.in_bounds(c2.x, c2.y) || (self.crowded(from, dir(hd)) && attempt < 9) {
                continue;
            }
            let rise = if self.up { 1.0 } else { -1.0 };
            let want =
                base + rise * level as f32 * 11.0 * self.scale.max(1.0) + self.rng.range(-1.2, 1.2);
            let want = if self.up {
                want.max(p0.y + drop - 1.0)
            } else {
                want.min(p0.y - drop + 1.0)
            };
            let y = self.covered_floor(c2, size, want);
            // never steeper than a walkable ramp (a deeper ground cover pushes it down: try again)
            if ((p0.y - y).abs() - absorb).max(0.0) / (dist - flat).max(1.0) > RAMP_SLOPE
                && attempt < 8
            {
                continue;
            }
            let pos = Vec3::new(c2.x, y, c2.y);
            if let Some(l) = self.locked_at(pos, size).filter(|l| *l != from) {
                if !self.linked(from, l) {
                    self.connect(from, l);
                }
                return Some((l, hd));
            }
            if self.overlaps(pos, size, Some(from)) && attempt < 9 {
                continue;
            }
            let yaw = hd + self.rng.range(-0.4, 0.4);
            let id = self.add_chamber(pos, size, yaw, level);
            if hall && let Some(c) = self.cave.chambers.iter_mut().find(|c| c.id == id) {
                c.hall = true;
            }
            self.connect(from, id);
            return Some((id, hd));
        }
        None
    }
}

fn work_seed(seed: u32) -> u64 {
    seed as u64 * 0x9e37_79b9 + 17
}

/// Lays out `cave` from its recipe (`kind`, `size`, `levels`, `seed`, `entrance`, `heading`):
/// replaces its mouth, chambers and tunnels, keeping locked chambers (Shuffle). The dungeon door
/// moves to the new deepest chamber.
pub fn layout(cave: &mut Cave, t: &Terrain) {
    if cave.kind == CaveKind::Arch {
        return;
    }
    let locked: Vec<Chamber> = cave.chambers.iter().filter(|c| c.locked).cloned().collect();
    // pools' chosen lips stay with their chambers (by id) through a new layout
    let rims: Vec<(u16, RimStyle)> = cave
        .chambers
        .iter()
        .filter(|c| c.rim != RimStyle::Auto)
        .map(|c| (c.id, c.rim))
        .collect();
    let locked_tunnels: Vec<Tunnel> = cave
        .tunnels
        .iter()
        .filter(|tn| locked.iter().any(|c| c.id == tn.a) && locked.iter().any(|c| c.id == tn.b))
        .cloned()
        .collect();
    let mut work = cave.clone();
    work.chambers = locked.clone();
    work.tunnels = locked_tunnels;
    if work.theme == Theme::Auto {
        let e = Vec3::from_array(work.entrance);
        work.resolved = Theme::for_biome(
            crate::biomes::BiomePreset::from_id(t.biome_at(e.x, e.z)),
            work.kind,
        );
    }
    let mine = work.kind == CaveKind::Mine;
    let next = locked.iter().map(|c| c.id + 1).max().unwrap_or(0).max(1);
    let mut b = Builder::new(t, work, work_seed(cave.seed), next);
    let e = Vec3::from_array(b.cave.entrance);
    let e2 = Vec2::new(e.x, e.z);
    let (_, slope) = uphill(t, e2, e2);
    let heading = b.cave.heading;
    let ground = t.height_at(e.x, e.z);

    // ---- the way in, and the first chamber
    let (first, base) = match b.cave.kind {
        CaveKind::Cavern => {
            b.cave.mouth = Mouth::None;
            let size = b.chamber_size(true);
            let y = b.covered_floor(e2, size, ground - 14.0 - size[1]);
            let (first, at) = b.first(Vec3::new(e.x, y, e.z), size, heading);
            (first, at.y)
        }
        CaveKind::Pit => {
            b.cave.mouth = Mouth::Pit;
            pit_entry(&mut b, e2, ground, heading)
        }
        _ if slope < 0.22 => {
            // flat ground: a pit for caves, a sloping decline for mines and passages
            if mine {
                b.cave.mouth = Mouth::Shaft;
                decline_entry(&mut b, e2, ground, heading)
            } else {
                b.cave.mouth = Mouth::Pit;
                pit_entry(&mut b, e2, ground, heading)
            }
        }
        _ => {
            b.cave.mouth = if mine { Mouth::Adit } else { Mouth::Hillside };
            hillside_entry(&mut b, e2, ground, heading)
        }
    };

    // ---- a passage runs on through the hill to where the ground falls away
    if b.cave.kind == CaveKind::Passage {
        passage_exit(&mut b, first, heading);
    } else {
        // ---- the main way, going down a level at a time
        let n = cave.size.chambers();
        let levels = cave.levels.clamp(1, 4);
        let mut at = first;
        let mut hd = heading;
        let big_at = if n >= 3 {
            1 + b.rng.below((n - 1) as u32) as usize
        } else {
            usize::MAX
        };
        // giant caves: great halls along the way (the second chamber, and further on)
        let halls: &[usize] = match cave.size.halls() {
            0 => &[],
            1 => &[1],
            _ => &[1, 4],
        };
        for i in 1..n {
            let level = ((i * levels as usize) / n).min(levels as usize - 1) as u8;
            let room = if halls.contains(&i) {
                Room::Hall
            } else if i == big_at {
                Room::Big
            } else {
                Room::Normal
            };
            match b.step(at, hd, level, base, room) {
                Some((id, h2)) => {
                    at = id;
                    hd = h2;
                }
                None => break,
            }
        }
        // ---- side branches: dead ends off the main way
        for _ in 0..cave.size.branches() {
            let ids: Vec<u16> = b.cave.chambers.iter().map(|c| c.id).collect();
            if ids.len() < 2 {
                break;
            }
            let from = ids[1 + b.rng.below(ids.len() as u32 - 1) as usize];
            let c = b.cave.chamber(from).unwrap().clone();
            let hd = b.rng.f32() * std::f32::consts::TAU;
            let level = c.level;
            let base_here = c.pos[1] + level as f32 * 11.0 * b.scale.max(1.0);
            if let Some((id, h2)) = b.step(from, hd, level, base_here, Room::Normal)
                && b.rng.f32() < 0.4
            {
                b.step(id, h2, level, base_here, Room::Normal);
            }
        }
        // ---- a loop in bigger caves: join two near chambers on a level
        if cave.size as u8 >= CaveSize::Large as u8 {
            let cs = b.cave.chambers.clone();
            'outer: for x in &cs {
                for y in &cs {
                    if x.id >= y.id || x.level != y.level {
                        continue;
                    }
                    let linked = b
                        .cave
                        .tunnels
                        .iter()
                        .any(|t| (t.a == x.id && t.b == y.id) || (t.a == y.id && t.b == x.id));
                    let d = x.center().distance(y.center());
                    let u = Vec2::new(y.pos[0] - x.pos[0], y.pos[2] - x.pos[2]).normalize_or_zero();
                    if !linked
                        && d < 34.0 * b.scale
                        && d > x.radius() + y.radius() + 4.0
                        && !b.crowded(x.id, u)
                        && !b.crowded(y.id, -u)
                    {
                        b.connect(x.id, y.id);
                        break 'outer;
                    }
                }
            }
        }
    }

    // ---- locked chambers the new layout didn't run into: joined to the nearest reachable one
    for _ in 0..locked.len() {
        let reach: Vec<u16> = b.cave.path_lengths().iter().map(|d| d.0).collect();
        let Some(l) = b
            .cave
            .chambers
            .iter()
            .find(|c| !reach.contains(&c.id))
            .cloned()
        else {
            break;
        };
        let near = b
            .cave
            .chambers
            .iter()
            .filter(|c| reach.contains(&c.id))
            .min_by(|x, y| {
                let k = |c: &Chamber| {
                    c.center().distance(l.center()) + (c.level as f32 - l.level as f32).abs() * 20.0
                };
                k(x).total_cmp(&k(y))
            })
            .map(|c| c.id);
        match near {
            Some(n) => b.connect(n, l.id),
            None => break,
        }
    }

    // ---- extras: pools in low chambers, skylights in big ones near the surface
    let feats = b.cave.features;
    let ids: Vec<u16> = b.cave.chambers.iter().map(|c| c.id).collect();
    for id in ids {
        let roll = b.rng.f32();
        let roll2 = b.rng.f32();
        let c = b.cave.chambers.iter_mut().find(|c| c.id == id).unwrap();
        if c.locked {
            continue;
        }
        c.pool = feats.pools
            && roll < if c.hall { 0.8 } else { 0.35 }
            && c.size[0] > 5.0
            && b.cave.kind != CaveKind::Mine;
        let roof = c.pos[1] + c.size[1];
        let above = t.height_at(c.pos[0], c.pos[2]) - roof;
        c.skylight = feats.skylights && roll2 < 0.6 && c.size[0] > 6.0 && above < 26.0;
    }

    // ---- the dungeon door moves to the deepest chamber
    let mut out = b.cave;
    for c in &mut out.chambers {
        if let Some((_, r)) = rims.iter().find(|(id, _)| *id == c.id) {
            c.rim = *r;
        }
    }
    if out.dungeon.is_some() {
        place_door(&mut out);
    }
    *cave = out;
}

/// Puts the dungeon door against the far wall of the deepest chamber, facing back in.
pub fn place_door(cave: &mut Cave) {
    let Some(deep) = cave.deepest() else { return };
    let Some(c) = cave.chamber(deep).cloned() else {
        return;
    };
    // the far wall: away from the tunnel that comes in
    let p = c.center();
    let into = cave
        .tunnels
        .iter()
        .find(|t| t.b == deep || t.a == deep)
        .and_then(|t| {
            let pts = if t.b == deep {
                t.pts.clone()
            } else {
                t.pts.iter().rev().copied().collect()
            };
            let before = pts.get(pts.len().saturating_sub(2)).copied()?;
            let d = p - Vec3::from_array(before);
            Some(Vec2::new(d.x, d.z).normalize_or(Vec2::Y))
        })
        .unwrap_or(Vec2::Y);
    // the chamber's ellipse radius along that direction (in its own frame)
    let (s, co) = c.yaw.sin_cos();
    let local = Vec2::new(into.x * co - into.y * s, into.x * s + into.y * co);
    let r = 1.0 / ((local.x / c.size[0]).powi(2) + (local.y / c.size[2]).powi(2)).sqrt();
    // the door stands at the end of a short passage cut into the far wall (`DOORWAY` m past the
    // chamber's edge), on the chamber's floor
    let at = Vec2::new(p.x, p.z) + into * (r + DOORWAY);
    if let Some(dg) = &mut cave.dungeon {
        dg.chamber = deep;
        dg.door = [at.x, p.y, at.y];
        // faces back into the chamber
        dg.door_yaw = (-into.x).atan2(-into.y);
    }
}

/// How far past the deepest chamber's edge the dungeon door stands (its doorway's length).
pub const DOORWAY: f32 = 2.5;

/// Hillside: a short level entry tunnel from just outside the mouth into the first chamber.
fn hillside_entry(b: &mut Builder, e: Vec2, ground: f32, heading: f32) -> (u16, f32) {
    let d = dir(heading);
    let size = b.chamber_size(false);
    let floor0 = ground - 0.25;
    let mut len = b.rng.range(9.0, 15.0) * b.scale + size[2] * 0.6;
    let fall = b.rng.range(0.03, 0.12);
    // the entry descends gently; the chamber keeps its rock cover — where the cover pushes it
    // deep, it moves on into the hill rather than the way in getting steep
    let (mut c, mut y) = (e, floor0);
    for _ in 0..14 {
        c = e + d * len;
        y = b.covered_floor(c, size, floor0 - len * fall);
        if (floor0 - y) / (len - level_reach(size, heading, -d)).max(1.0) <= RAMP_SLOPE {
            break;
        }
        len += 4.0;
    }
    let (first, at) = b.first(Vec3::new(c.x, y, c.y), size, heading);
    let profile = if b.mine {
        Profile::Drift
    } else {
        Profile::Tube
    };
    let (w, h) = b.tunnel_dims(profile);
    // the approach: over the hillside's own surface up to the mouth (no trench cut into the
    // slope, no rock built out over it)
    let (out, near) = (e - d * 4.0, e - d * 2.0);
    let on = |q: Vec2| (b.t.height_at(q.x, q.y) + 0.1).min(floor0 + 0.35);
    let mut pts = vec![
        [out.x, on(out), out.y],
        [near.x, on(near), near.y],
        [e.x, floor0, e.y],
    ];
    // down to where the chamber's floor levels off, then on across it to its middle
    let toward = Vec2::new(at.x - e.x, at.z - e.y);
    let reach = b
        .cave
        .chamber(first)
        .map(|c| level_reach(c.size, c.yaw, -toward.normalize_or(d)))
        .unwrap_or(0.0);
    let ea = if toward.length() > reach + 2.0 {
        {
            let o = toward.normalize() * reach;
            at - Vec3::new(o.x, 0.0, o.y)
        }
    } else {
        at
    };
    let mut rest = b.wander(
        Vec3::new(e.x, floor0, e.y),
        ea,
        if b.mine { 0.2 } else { 1.0 },
    );
    rest.remove(0);
    pts.extend(rest);
    if ea != at {
        pts.push(at.to_array());
    }
    b.cave.tunnels.push(Tunnel {
        a: OUTSIDE,
        b: first,
        pts,
        width: w * 1.15,
        height: h * 1.1,
        profile,
    });
    (first, at.y)
}

/// A pit: a wide hole down to the first chamber with a scree slope (drawn by the volume).
fn pit_entry(b: &mut Builder, e: Vec2, ground: f32, heading: f32) -> (u16, f32) {
    let size = b.chamber_size(true);
    // the pit's floor is the first chamber's floor
    let depth = b.rng.range(11.0, 15.0) * b.scale;
    let (first, at) = b.first(Vec3::new(e.x, ground - depth, e.y), size, heading);
    (first, at.y)
}

/// A mine's decline on flat ground: a straight sloping drift down to the first room.
fn decline_entry(b: &mut Builder, e: Vec2, ground: f32, heading: f32) -> (u16, f32) {
    let d = dir(heading);
    let size = b.chamber_size(false);
    let drop = 12.0;
    let len = drop / 0.32;
    let c = e + d * (len + size[2]);
    let y = b.covered_floor(c, size, ground - drop);
    let (first, at) = b.first(Vec3::new(c.x, y, c.y), size, heading);
    let (y, c) = (at.y, Vec2::new(at.x, at.z));
    let (w, h) = b.tunnel_dims(Profile::Drift);
    let out = e - d * 4.0;
    let mut pts = vec![[out.x, ground + 0.2, out.y]];
    for k in 0..=6 {
        let u = k as f32 / 6.0;
        let q = e + d * (len * u);
        pts.push([q.x, ground - 0.2 - (ground - 0.2 - y) * u, q.y]);
    }
    pts.push([c.x, y, c.y]);
    b.cave.tunnels.push(Tunnel {
        a: OUTSIDE,
        b: first,
        pts,
        width: w * 1.1,
        height: h,
        profile: Profile::Drift,
    });
    (first, y)
}

/// A passage: on through the hill from the first chamber until the ground falls away under the
/// floor, then out.
fn passage_exit(b: &mut Builder, first: u16, heading: f32) {
    let c = b.cave.chamber(first).unwrap().clone();
    let d = dir(heading);
    let mut y = c.pos[1];
    let start = Vec2::new(c.pos[0], c.pos[2]) + d * c.radius();
    let mut exit = None;
    let mut s = 8.0;
    while s < 260.0 {
        let q = start + d * s;
        if !b.t.in_bounds(q.x, q.y) {
            break;
        }
        let g = b.t.height_at(q.x, q.y);
        // follow the ground down a little where the hill drops, never climbing
        y = y.min(g - 0.3).max(y - 0.35 * 2.0);
        if g < y + 1.2 {
            exit = Some((q, y));
            break;
        }
        s += 2.0;
    }
    let Some((x, yx)) = exit else {
        // no far side within reach: an ordinary cave instead
        let base = c.pos[1];
        let mut at = first;
        let mut hd = heading;
        for _ in 1..b.cave.size.chambers() {
            match b.step(at, hd, 0, base, Room::Normal) {
                Some((id, h2)) => {
                    at = id;
                    hd = h2;
                }
                None => break,
            }
        }
        return;
    };
    let profile = b.pick_profile();
    let (w, h) = b.tunnel_dims(profile);
    let ea = c.center() + Vec3::new(d.x, 0.0, d.y) * level_reach(c.size, c.yaw, d);
    let mut pts = vec![c.center().to_array()];
    pts.extend(b.wander(ea, Vec3::new(x.x, yx, x.y), 2.0));
    // out and down to the ground beyond, no steeper than a walkable ramp
    let mut on = 4.0;
    while on < 18.0 {
        let q = x + d * on;
        if !b.t.in_bounds(q.x, q.y) || b.t.height_at(q.x, q.y) >= yx - on * RAMP_SLOPE - 0.1 {
            break;
        }
        on += 1.0;
    }
    let out = x + d * on;
    let out_y = (yx - on * RAMP_SLOPE)
        .max(b.t.height_at(out.x, out.y) - 0.1)
        .min(yx);
    pts.push([out.x, out_y, out.y]);
    b.cave.tunnels.push(Tunnel {
        a: first,
        b: OUTSIDE,
        pts,
        width: w,
        height: h,
        profile,
    });
}

/// Grows a new tunnel and chamber from chamber `from` towards `heading` (the "+" handle).
/// `down`: the new chamber is a level lower. Returns its id.
pub fn extend(
    cave: &mut Cave,
    t: &Terrain,
    from: u16,
    heading: f32,
    down: bool,
) -> Result<u16, String> {
    let c = cave.chamber(from).ok_or("no such chamber")?.clone();
    let level = if down { c.level + 1 } else { c.level };
    // (`down`: a level further on — lower, or higher in a cave whose levels climb)
    let rise = if cave.up { 1.0 } else { -1.0 };
    let base = c.pos[1] - rise * c.level as f32 * 11.0 * cave.size.scale().max(1.0);
    let next = cave.chambers.iter().map(|c| c.id + 1).max().unwrap_or(1);
    let seed =
        cave.seed ^ (next as u32).wrapping_mul(0x2545_f491) ^ ((heading * 1000.0) as i32 as u32);
    let mut b = Builder::new(t, cave.clone(), seed as u64, next);
    // (the heading is what was asked: no random turn)
    let prev = b.cave.chamber(from).unwrap().clone();
    let p0 = prev.center();
    let size = b.chamber_size(false);
    let drop = if down { 11.0 * b.scale.max(1.0) } else { 0.0 };
    let mut dist = prev.radius() + size[0].max(size[2]) + 10.0 * b.scale;
    let absorb = b
        .climb_in(prev.size, prev.hall)
        .max(b.climb_in(size, false));
    if down {
        dist = dist.max(
            (drop - absorb).max(0.0) / RAMP_SLOPE
                + level_reach(prev.size, prev.yaw, dir(heading))
                + size[0].min(size[2]) * 0.79
                + 1.4,
        );
    }
    for attempt in 0..8 {
        let hd = heading + (attempt as f32 * 0.35) * if attempt % 2 == 0 { 1.0 } else { -1.0 };
        let c2 = Vec2::new(p0.x, p0.z) + dir(hd) * (dist + attempt as f32 * 2.0);
        if !t.in_bounds(c2.x, c2.y) {
            continue;
        }
        let want = base + rise * level as f32 * 11.0 * b.scale.max(1.0);
        let want = if cave.up {
            want.max(p0.y + drop - 0.5)
        } else {
            want.min(p0.y - drop + 0.5)
        };
        let y = b.covered_floor(c2, size, want);
        let pos = Vec3::new(c2.x, y, c2.y);
        if b.overlaps(pos, size, Some(from)) {
            continue;
        }
        let id = b.add_chamber(pos, size, hd, level);
        b.connect(from, id);
        b.cave.levels = b.cave.levels.max(level + 1);
        *cave = b.cave;
        return Ok(id);
    }
    Err("no room there: other chambers are in the way (try another direction)".into())
}

/// The test hill (for other modules' tests).
#[cfg(test)]
pub fn tests_hill() -> Terrain {
    tests::hill()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn hill() -> Terrain {
        // a 1 km world with a big round hill in the middle
        let mut t = Terrain::new(256, 4.0, 0.0);
        let n = t.n();
        for j in 0..n {
            for i in 0..n {
                let p = t.grid_to_world(i, j);
                let r = p.length();
                t.heights[j * n + i] = 10.0 + 70.0 * (-(r / 180.0).powi(2)).exp();
            }
        }
        t
    }

    fn cave(kind: CaveKind, size: CaveSize, levels: u8, seed: u32, t: &Terrain) -> Cave {
        let e = Vec2::new(150.0, 0.0);
        let (heading, _) = uphill(t, e, Vec2::ZERO);
        let mut c = Cave {
            id: 1,
            name: "Test".into(),
            kind,
            size,
            levels,
            up: false,
            theme: Theme::Auto,
            resolved: Theme::Auto,
            features: Features::default(),
            darkness: Darkness::Dim,
            seed,
            entrance: [e.x, t.height_at(e.x, e.y), e.y],
            heading,
            mouth: Mouth::Hillside,
            chambers: vec![],
            tunnels: vec![],
            dungeon: None,
            arch: None,
            props: vec![],
        };
        layout(&mut c, t);
        c
    }

    #[test]
    fn layouts_are_deterministic_and_covered() {
        let t = hill();
        for seed in 0..12 {
            for size in CaveSize::ALL {
                let a = cave(CaveKind::Cave, size, 2, seed, &t);
                let b = cave(CaveKind::Cave, size, 2, seed, &t);
                assert_eq!(a, b);
                assert!(!a.chambers.is_empty());
                assert!(a.tunnels.iter().any(|t| t.a == OUTSIDE), "has a way in");
                for c in &a.chambers {
                    let roof = c.pos[1] + c.size[1];
                    let g = ground_min(&t, Vec2::new(c.pos[0], c.pos[2]), c.radius());
                    assert!(
                        roof <= g - COVER + 0.01,
                        "chamber {} roof {roof} under ground {g}",
                        c.id
                    );
                }
                // every chamber reachable from the way in
                assert_eq!(
                    a.path_lengths().len(),
                    a.chambers.len(),
                    "seed {seed} {size:?}"
                );
            }
        }
    }

    #[test]
    fn levels_go_down_and_tunnels_stay_walkable() {
        let t = hill();
        for seed in 0..10 {
            let c = cave(CaveKind::Cave, CaveSize::Large, 3, seed, &t);
            let max_level = c.chambers.iter().map(|c| c.level).max().unwrap();
            assert!(max_level >= 1, "seed {seed}: {max_level}");
            for tn in &c.tunnels {
                for w in tn.pts.windows(2) {
                    let (a, b) = (Vec3::from_array(w[0]), Vec3::from_array(w[1]));
                    let run = Vec2::new(b.x - a.x, b.z - a.z).length();
                    if run > 1.0 {
                        assert!(
                            (b.y - a.y).abs() / run < 1.3,
                            "seed {seed}: steep step {} over {run}",
                            (b.y - a.y).abs()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn locked_chambers_survive_shuffle() {
        let t = hill();
        let mut c = cave(CaveKind::Cave, CaveSize::Medium, 1, 3, &t);
        let keep = c.chambers[1].clone();
        c.chambers[1].locked = true;
        c.seed = 99;
        layout(&mut c, &t);
        assert!(
            c.chambers
                .iter()
                .any(|x| x.id == keep.id && x.pos == keep.pos && x.locked)
        );
        assert_eq!(c.path_lengths().len(), c.chambers.len());
    }

    #[test]
    fn extend_adds_a_reachable_chamber() {
        let t = hill();
        let mut c = cave(CaveKind::Cave, CaveSize::Small, 1, 5, &t);
        let from = c.chambers.last().unwrap().id;
        let n = c.chambers.len();
        let hd = c.heading + 0.5;
        let id = extend(&mut c, &t, from, hd, true).unwrap();
        assert_eq!(c.chambers.len(), n + 1);
        assert_eq!(
            c.chamber(id).unwrap().level,
            c.chamber(from).unwrap().level + 1
        );
        assert_eq!(c.path_lengths().len(), c.chambers.len());
    }

    #[test]
    fn giant_caves_have_great_halls() {
        let t = hill();
        for (size, halls, across) in [(CaveSize::Giant, 1, 60.0), (CaveSize::Colossal, 2, 110.0)] {
            for seed in 0..4 {
                let c = cave(CaveKind::Cave, size, 2, seed, &t);
                let hs: Vec<&Chamber> = c.chambers.iter().filter(|c| c.hall).collect();
                assert!(
                    !hs.is_empty() && hs.len() <= halls,
                    "{size:?} seed {seed}: {} halls",
                    hs.len()
                );
                for h in hs {
                    assert!(
                        h.size[0].min(h.size[2]) * 2.0 >= across,
                        "{size:?} seed {seed}: hall only {:?}",
                        h.size
                    );
                }
                assert_eq!(
                    c.path_lengths().len(),
                    c.chambers.len(),
                    "{size:?} seed {seed}: unreachable chambers"
                );
            }
        }
    }

    #[test]
    fn dungeon_door_goes_deepest() {
        let t = hill();
        let mut c = cave(CaveKind::Cave, CaveSize::Medium, 2, 8, &t);
        c.dungeon = Some(Dungeon {
            name: "Crypt".into(),
            chamber: 0,
            door: [0.0; 3],
            door_yaw: 0.0,
            instancing: Instancing::Party,
        });
        place_door(&mut c);
        let deep = c.deepest().unwrap();
        assert_eq!(c.dungeon.as_ref().unwrap().chamber, deep);
    }
}
