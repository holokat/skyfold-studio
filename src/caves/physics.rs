//! Questions about the cave rock for everything that moves or points: is a spot in a cave's
//! space, where is the floor under it, where does a ray hit the rock, and how a walker stays
//! out of the walls. Answered from the latest built volume (`CaveWorld::volume`).

use bevy::prelude::*;

use super::render::CaveWorld;
use crate::terrain::Terrain;
use crate::terrain::holes::TerrainHoles;

impl CaveWorld {
    /// Is `p` in the air of a cave's space (carved out, or by rock the caves added) rather than
    /// plain outdoors? (There the cave decides what is solid, not the terrain.)
    pub fn in_space(&self, p: Vec3) -> bool {
        if self.dungeon.as_ref().is_some_and(|d| d.in_space(p)) {
            return true;
        }
        let Some(v) = &self.volume else { return false };
        if !v.near_any(p, 0.0) {
            return false;
        }
        let s = v.at(p);
        s.d > 0.0 && (s.d - s.ground).abs() > 0.05
    }

    /// The field at `p` (negative in rock), if a cave shapes the rock there.
    pub fn field(&self, p: Vec3) -> Option<f32> {
        if let Some(d) = self.dungeon.as_ref().filter(|d| d.contains(p)) {
            let dd = d.field(p);
            return Some(match self.volume.as_ref().filter(|v| v.near_any(p, 0.0)) {
                Some(v) if !d.interior => dd.max(v.at(p).d),
                _ => dd,
            });
        }
        let v = self.volume.as_ref()?;
        if !v.near_any(p, 0.0) {
            return None;
        }
        Some(v.at(p).d)
    }

    /// The floor under `p` when `p` is in a cave's space (within `reach` metres down).
    pub fn floor_under(&self, p: Vec3, reach: f32) -> Option<f32> {
        if let Some(f) = self.dungeon.as_ref().and_then(|d| d.floor_under(p, reach)) {
            return Some(f);
        }
        let v = self.volume.as_ref()?;
        if !v.near_any(p, 0.0) {
            return None;
        }
        let s = v.at(p);
        if s.d <= 0.0 || (s.d - s.ground).abs() <= 0.05 {
            return None;
        }
        // step down by the distance to the rock (never more than 0.5 m), then refine
        let mut y = p.y;
        let mut d = s.d;
        let bottom = p.y - reach;
        while y > bottom {
            let step = d.clamp(0.05, 0.5);
            let ny = y - step;
            let nd = v.at(Vec3::new(p.x, ny, p.z)).d;
            if nd <= 0.0 {
                let (mut a, mut b) = (ny, y);
                for _ in 0..12 {
                    let m = 0.5 * (a + b);
                    if v.at(Vec3::new(p.x, m, p.z)).d <= 0.0 {
                        a = m;
                    } else {
                        b = m;
                    }
                }
                return Some(b);
            }
            y = ny;
            d = nd;
        }
        None
    }

    /// The roof over `p` in a cave's space (within `reach` metres up): its height.
    pub fn roof_over(&self, p: Vec3, reach: f32) -> Option<f32> {
        if let Some(f) = self.dungeon.as_ref().and_then(|d| d.roof_over(p, reach)) {
            return Some(f);
        }
        let v = self.volume.as_ref()?;
        if !v.near_any(p, 0.0) {
            return None;
        }
        let mut y = p.y;
        let top = p.y + reach;
        while y < top {
            let d = v.at(Vec3::new(p.x, y, p.z)).d;
            if d <= 0.0 {
                return Some(y);
            }
            y += d.clamp(0.05, 0.5);
        }
        None
    }

    /// Where a ray first meets cave rock (or the ground inside a cave's bounds), sphere-traced
    /// through the volume: (hit point, distance along the ray).
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(Vec3, f32)> {
        let dungeon = self
            .dungeon
            .as_ref()
            .and_then(|d| d.raycast(origin, dir, max));
        match (dungeon, self.raycast_rock(origin, dir, max)) {
            (Some(a), Some(b)) => Some(if a.1 <= b.1 { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    /// `raycast` through the caves' rock alone.
    fn raycast_rock(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(Vec3, f32)> {
        let v = self.volume.as_ref()?;
        let (lo, hi) = v.bounds()?;
        let dir = dir.normalize_or_zero();
        // clip to the volume's box
        let mut t0 = 0.0f32;
        let mut t1 = max;
        for k in 0..3 {
            let (o, d, a, b) = (origin[k], dir[k], lo[k], hi[k]);
            if d.abs() < 1e-6 {
                if o < a || o > b {
                    return None;
                }
            } else {
                let (u, w) = ((a - o) / d, (b - o) / d);
                t0 = t0.max(u.min(w));
                t1 = t1.min(u.max(w));
            }
        }
        if t0 > t1 {
            return None;
        }
        let mut t = t0;
        let mut prev = v.at(origin + dir * t).d;
        if prev <= 0.0 {
            // starts in rock (a camera behind a wall): look for the first way out, then in again
            while t < t1 && prev <= 0.0 {
                t += 0.4;
                prev = v.at(origin + dir * t).d;
            }
        }
        while t < t1 {
            let step = (prev * 0.7).clamp(0.04, 2.0);
            let nt = t + step;
            let d = v.at(origin + dir * nt).d;
            if d <= 0.0 {
                let (mut a, mut b) = (t, nt);
                for _ in 0..14 {
                    let m = 0.5 * (a + b);
                    if v.at(origin + dir * m).d <= 0.0 {
                        b = m;
                    } else {
                        a = m;
                    }
                }
                return Some((origin + dir * b, b));
            }
            t = nt;
            prev = d;
        }
        None
    }

    /// What a ray (the cursor's) meets with the caves taken into account: the terrain where it
    /// is drawn — not in its holes at the cave mouths, and from an eye under the ground (in a
    /// cave) only once the ray is out of the caves — or the cave rock, whichever is nearer.
    /// (the point, and the rock's normal there when it is cave rock)
    pub fn pick(
        &self,
        terrain: &Terrain,
        holes: &TerrainHoles,
        origin: Vec3,
        dir: Vec3,
        max: f32,
    ) -> Option<(Vec3, Option<Vec3>)> {
        // a dungeon's underground map: only its space (the hidden ground far above is no surface)
        if let Some(d) = self.dungeon.as_ref().filter(|d| d.interior) {
            return d
                .raycast(origin, dir, max)
                .map(|(p, _)| (p, Some(d.normal_at(p))));
        }
        let dir = dir.normalize_or_zero();
        let under =
            self.volume.is_some() && origin.y < terrain.height_at(origin.x, origin.z) - 0.05;
        let t_hit = if under {
            self.box_exit(origin, dir)
                .and_then(|t0| terrain.raycast(origin + dir * t0, dir, max - t0))
        } else {
            terrain.raycast(origin, dir, max)
        };
        let t_hit = t_hit.filter(|h| !holes.at(terrain, h.x, h.z));
        let reach = t_hit.map_or(max.min(6000.0), |h| h.distance(origin) + 1.0);
        match (t_hit, self.raycast(origin, dir, reach)) {
            (Some(a), Some((_, tb))) if a.distance(origin) <= tb => Some((a, None)),
            (_, Some((b, _))) => Some((b, Some(self.normal_at(b).unwrap_or(Vec3::Y)))),
            (a, None) => a.map(|a| (a, None)),
        }
    }

    /// The rock's surface normal at `p` (pointing out into the open), where a cave shapes it.
    pub fn normal_at(&self, p: Vec3) -> Option<Vec3> {
        if let Some(d) = self
            .dungeon
            .as_ref()
            .filter(|d| d.contains(p) && d.field(p).abs() < 0.5)
        {
            return Some(d.normal_at(p));
        }
        let v = self.volume.as_ref()?;
        if !v.near_any(p, 0.5) {
            return None;
        }
        let e = 0.1;
        let g = Vec3::new(
            v.at(p + Vec3::X * e).d - v.at(p - Vec3::X * e).d,
            v.at(p + Vec3::Y * e).d - v.at(p - Vec3::Y * e).d,
            v.at(p + Vec3::Z * e).d - v.at(p - Vec3::Z * e).d,
        );
        Some(g.normalize_or(Vec3::Y))
    }

    /// How far a ray that starts inside the caves' box travels before it leaves it.
    pub fn box_exit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let (lo, hi) = self.volume.as_ref()?.bounds()?;
        if origin.cmplt(lo).any() || origin.cmpgt(hi).any() {
            return None;
        }
        let mut t = f32::MAX;
        for k in 0..3 {
            if dir[k].abs() > 1e-6 {
                t = t.min(((if dir[k] > 0.0 { hi[k] } else { lo[k] }) - origin[k]) / dir[k]);
            }
        }
        (t < f32::MAX).then_some(t)
    }

    /// The cave (index in `Caves::list` at the last build) whose space `p` is in.
    pub fn owner_at(&self, p: Vec3) -> Option<usize> {
        let v = self.volume.as_ref()?;
        if !self.in_space(p) {
            return None;
        }
        let o = v.at(p).owner;
        (o != super::sdf::NONE).then_some(o as usize)
    }

    /// Pushes a standing body (feet at `feet`, `radius`, `height`) out of the cave walls:
    /// the corrected horizontal position, if anything was in the way.
    pub fn push_out(&self, feet: Vec3, radius: f32, height: f32, step: f32) -> Option<Vec2> {
        if let Some(q) = self
            .dungeon
            .as_ref()
            .and_then(|d| d.push_out(feet, radius, height, step))
        {
            return Some(q);
        }
        let v = self.volume.as_ref()?;
        let c = feet + Vec3::Y * (height * 0.5);
        if !v.near_any(c, radius + height) {
            return None;
        }
        let mut p = feet;
        let mut moved = false;
        for _ in 0..3 {
            let mut push = Vec2::ZERO;
            // sample the body above the step height (lower bumps are stepped onto)
            let n = 4;
            for k in 0..n {
                let y = p.y + step + (height - step) * (k as f32 + 0.5) / n as f32;
                let q = Vec3::new(p.x, y, p.z);
                let d = v.at(q).d;
                if d < radius {
                    let e = 0.08;
                    let gx = v.at(q + Vec3::X * e).d - v.at(q - Vec3::X * e).d;
                    let gz = v.at(q + Vec3::Z * e).d - v.at(q - Vec3::Z * e).d;
                    let g = Vec2::new(gx, gz);
                    if g.length() > 1e-5 {
                        push += g.normalize() * (radius - d);
                    }
                }
            }
            if push.length() < 1e-4 {
                break;
            }
            p.x += push.x / n as f32 * 1.5;
            p.z += push.y / n as f32 * 1.5;
            moved = true;
        }
        moved.then_some(Vec2::new(p.x, p.z))
    }
}

/// What `walk_check` found.
#[derive(Debug, Default, serde::Serialize)]
pub struct WalkReport {
    /// chambers walked into (ids)
    pub reached: Vec<u16>,
    pub missed: Vec<u16>,
    /// where the walker got stuck: (position, the waypoint it was heading for)
    pub stuck: Vec<([f32; 3], [f32; 3])>,
    /// where it ended up out on the hillside above while it should have been inside
    pub popped_out: Vec<[f32; 3]>,
    /// metres walked and seconds taken
    pub walked: f32,
    pub seconds: f32,
}

/// Walks a cave on foot with the real walk physics (no rendering): from just outside the way in,
/// along every tunnel to every chamber and back (depth first), steering at the tunnel points.
pub fn walk_check(world: &mut World, cave_id: u32) -> Result<WalkReport, String> {
    use crate::motion::movement::{self, MoveInput, MoveWorld};
    use crate::walk::WalkState;
    let cave = world
        .resource::<super::model::Caves>()
        .get(cave_id)
        .cloned()
        .ok_or_else(|| format!("no cave {cave_id}"))?;
    let cw = world.resource::<CaveWorld>();
    if cw.busy() || cw.volume.is_none() {
        return Err("the caves are still being built: try again in a moment (caves_wait)".into());
    }
    let terrain = world.resource::<crate::terrain::Terrain>();
    let colliders = world.resource::<crate::motion::collide::Colliders>();
    let store = world.resource::<crate::objects::ObjectStore>();
    let surface = |_: Vec2| -> Option<f32> { None };
    let decks = |_: Vec3| -> Option<(f32, crate::terrain::layers::Surface)> { None };
    let w = MoveWorld {
        terrain,
        colliders,
        store,
        surface: &surface,
        decks: &decks,
        caves: Some(cw),
    };
    let mut s = WalkState::default();
    s.active = true;
    s.eye_off = movement::EYE_STAND;
    s.breath = 1.0;
    s.fly_speed = movement::FLY_DEFAULT;
    // the route: depth first through the tunnels from the way in (a pit: down its scree slope)
    let (mut route, first): (Vec<(Vec3, Option<u16>)>, u16) =
        if cave.mouth == super::model::Mouth::Pit {
            let c = cave.chambers.first().ok_or("the pit has no chamber")?;
            let p = c.center();
            let d = super::layout::dir(cave.heading);
            let r0 = c.size[0].min(c.size[2]) * 0.85;
            let r1 = r0 * 1.35 + 2.0;
            let top = Vec2::new(p.x, p.z) - d * (r1 * 1.15);
            let bot = Vec2::new(p.x, p.z) + d * (r0 * 0.55);
            (
                vec![
                    (
                        Vec3::new(top.x, terrain.height_at(top.x, top.y), top.y),
                        None,
                    ),
                    (Vec3::new(bot.x, p.y, bot.y), None),
                    (p, Some(c.id)),
                ],
                c.id,
            )
        } else {
            let entry = cave
                .tunnels
                .iter()
                .find(|t| t.a == super::model::OUTSIDE)
                .ok_or("this cave has no way in (a sealed cavern)")?;
            let mut r: Vec<(Vec3, Option<u16>)> = entry
                .pts
                .iter()
                .map(|p| (Vec3::from_array(*p), None))
                .collect();
            r.last_mut().unwrap().1 = Some(entry.b);
            (r, entry.b)
        };
    let mut seen = vec![first];
    fn visit(
        c: &super::model::Cave,
        at: u16,
        seen: &mut Vec<u16>,
        route: &mut Vec<(Vec3, Option<u16>)>,
    ) {
        for t in &c.tunnels {
            let (next, pts): (u16, Vec<[f32; 3]>) = if t.a == at && t.b != super::model::OUTSIDE {
                (t.b, t.pts.clone())
            } else if t.b == at && t.a != super::model::OUTSIDE {
                (t.a, t.pts.iter().rev().copied().collect())
            } else {
                continue;
            };
            if seen.contains(&next) {
                continue;
            }
            seen.push(next);
            let n = pts.len();
            for (k, p) in pts.iter().enumerate() {
                route.push((Vec3::from_array(*p), (k + 1 == n).then_some(next)));
            }
            visit(c, next, seen, route);
            // and back
            for p in pts.iter().rev() {
                route.push((Vec3::from_array(*p), None));
            }
        }
    }
    visit(&cave, first, &mut seen, &mut route);
    let start = route[0].0;
    s.position = Vec3::new(start.x, w.ground(start + Vec3::Y * 2.0, 1.0), start.z);
    let dt = 1.0 / 60.0;
    let mut rep = WalkReport::default();
    let mut last = s.position;
    let mut was_inside = false;
    for (target, chamber) in route.iter().skip(1) {
        let mut t = 0.0;
        let mut best = f32::MAX;
        let mut since_better = 0.0;
        loop {
            let d = Vec2::new(target.x - s.position.x, target.z - s.position.z);
            let dist = d.length();
            if dist < 1.2 {
                break;
            }
            if dist < best - 0.05 {
                best = dist;
                since_better = 0.0;
            } else {
                since_better += dt;
            }
            if since_better > 3.0 || t > 25.0 {
                rep.stuck.push((s.position.to_array(), target.to_array()));
                // carry on from the waypoint (on its floor)
                s.position = Vec3::new(target.x, w.ground(*target + Vec3::Y * 1.0, 0.5), target.z);
                s.velocity = Vec3::ZERO;
                break;
            }
            s.yaw = (-d.x).atan2(-d.y);
            let inp = MoveInput {
                fwd: 1.0,
                jump: since_better > 0.6 && s.grounded,
                ..Default::default()
            };
            movement::step(&mut s, &inp, &w, dt);
            t += dt;
            rep.seconds += dt;
            rep.walked += Vec2::new(s.position.x - last.x, s.position.z - last.z).length();
            last = s.position;
            // out on the hillside above the cave while heading somewhere inside it
            let ground = terrain.height_at(s.position.x, s.position.z);
            let inside = cw.in_space(s.position + Vec3::Y * 0.5);
            let at_mouth = Vec2::new(
                s.position.x - cave.entrance[0],
                s.position.z - cave.entrance[2],
            )
            .length()
                < 8.0;
            let popped = was_inside
                && !inside
                && !at_mouth
                && target.y < ground - 3.0
                && s.position.y > ground - 0.3;
            was_inside = inside;
            if popped {
                rep.popped_out.push(s.position.to_array());
                s.position = Vec3::new(target.x, w.ground(*target + Vec3::Y * 1.0, 0.5), target.z);
                s.velocity = Vec3::ZERO;
                break;
            }
        }
        if let Some(c) = chamber
            && !rep.reached.contains(c)
            && rep
                .stuck
                .last()
                .is_none_or(|st| Vec3::from_array(st.1) != *target)
        {
            rep.reached.push(*c);
        }
    }
    rep.missed = cave
        .chambers
        .iter()
        .map(|c| c.id)
        .filter(|id| !rep.reached.contains(id))
        .collect();
    Ok(rep)
}
