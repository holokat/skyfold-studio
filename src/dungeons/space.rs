//! The dungeon's space for everything that moves or points: the walker's floor and walls, the
//! camera, the cursor. The same questions the caves answer (`caves::physics`), asked of the
//! compiled plan; `CaveWorld` passes them on, so walking, placing objects, the camera and the
//! cursor work in a dungeon exactly as in a cave.

use bevy::math::{Vec2, Vec3};
use std::sync::Arc;

use super::plan::Compiled;

/// The built dungeon, as the physics sees it.
#[derive(Clone, Debug)]
pub struct Space {
    pub plan: Arc<Compiled>,
    /// editing view: everything above this height is cut away (the cursor passes through it)
    pub cut: Option<f32>,
    /// an underground map (nothing but this space is solid)
    pub interior: bool,
    /// the map's style (rooms without their own)
    pub style: &'static str,
    /// closed doors, gates and bars in the way
    pub blockers: std::sync::Arc<Vec<super::pieces::Blocker>>,
}

impl Space {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        (self.plan.lo, self.plan.hi)
    }
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.plan.lo).all() && p.cmple(self.plan.hi).all()
    }
    /// What the floor under the feet at `p` sounds like: a bridge's deck is wood, a pool's liquid
    /// splashes, the rest is its room's style.
    pub fn surface_at(&self, p: Vec3) -> Option<crate::terrain::layers::Surface> {
        use super::plan::{Liquid, RoomKind};
        use crate::terrain::layers::Surface;
        if !self.contains(p) {
            return None;
        }
        let q = Vec2::new(p.x, p.z);
        let floor = self.plan.floor_under(p + Vec3::Y * 0.3, 1.5)?;
        let r = self
            .plan
            .rooms_near(q)
            .filter(|r| r.inside(q) > -0.3)
            .min_by(|a, b| {
                (a.floor_at(q) - floor)
                    .abs()
                    .total_cmp(&(b.floor_at(q) - floor).abs())
            })?;
        Some(match r.kind {
            RoomKind::Bridge => Surface::Wood,
            RoomKind::Pool if r.liquid != Liquid::None && floor < r.level_y - 0.1 => Surface::Water,
            k => super::styles::footstep(
                r.style.as_deref().unwrap_or(self.style),
                k == RoomKind::Cavern,
            ),
        })
    }
    /// In the dungeon's air.
    pub fn in_space(&self, p: Vec3) -> bool {
        self.contains(p) && self.plan.air(p) && self.blockers.iter().all(|b| b.sdf(p) > 0.0)
    }
    /// Distance-like value: positive in the air, negative in the rock.
    pub fn field(&self, p: Vec3) -> f32 {
        let d = self.plan.field(p);
        self.blockers.iter().fold(d, |d, b| d.min(b.sdf(p)))
    }
    pub fn floor_under(&self, p: Vec3, reach: f32) -> Option<f32> {
        if !self.contains(p) {
            return None;
        }
        self.plan.floor_under(p, reach)
    }
    pub fn roof_over(&self, p: Vec3, reach: f32) -> Option<f32> {
        if !self.contains(p) {
            return None;
        }
        self.plan.roof_over(p, reach)
    }

    /// Air as the cursor sees it: with the cut, the levels above it are rock it looks through.
    fn air_cut(&self, p: Vec3) -> bool {
        self.cut.is_none_or(|c| p.y <= c) && self.plan.air(p)
    }

    /// Where a ray first hits a floor, wall or roof from inside the dungeon (a ray that starts in
    /// the rock — a camera outside, or over the cut — first travels to the air, then on to the
    /// next surface): (hit point, distance).
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<(Vec3, f32)> {
        let dir = dir.normalize_or_zero();
        let (lo, hi) = self.bounds();
        let (mut t0, mut t1) = (0.0f32, max);
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
        let at = |t: f32| origin + dir * t;
        // first: into the air (through rock, ceilings and the cut levels)
        let mut t = t0;
        if !self.air_cut(at(t)) {
            loop {
                let p = at(t);
                let d = self.field(p);
                let step = if self.cut.is_some_and(|c| p.y > c) {
                    0.25
                } else {
                    (-d * 0.6).clamp(0.12, 2.0)
                };
                t += step;
                if t > t1 {
                    return None;
                }
                if self.air_cut(at(t)) {
                    break;
                }
            }
        }
        // then: on to the next surface
        let mut prev = t;
        while t < t1 {
            let d = self.field(at(t)).max(0.0);
            let nt = t + (d * 0.7).clamp(0.04, 1.0);
            if !self.air_cut(at(nt)) {
                let (mut a, mut b) = (prev.max(t), nt);
                for _ in 0..16 {
                    let m = 0.5 * (a + b);
                    if self.air_cut(at(m)) {
                        a = m;
                    } else {
                        b = m;
                    }
                }
                return Some((at(b), b));
            }
            prev = t;
            t = nt;
        }
        None
    }

    /// The surface normal at a hit point (pointing into the air).
    pub fn normal_at(&self, p: Vec3) -> Vec3 {
        let e = 0.06;
        let f = |q: Vec3| self.plan.field(q);
        let g = Vec3::new(
            f(p + Vec3::X * e) - f(p - Vec3::X * e),
            f(p + Vec3::Y * e) - f(p - Vec3::Y * e),
            f(p + Vec3::Z * e) - f(p - Vec3::Z * e),
        );
        // floors and roofs are flat: snap near-vertical normals
        let n = g.normalize_or(Vec3::Y);
        if n.y.abs() > 0.92 {
            Vec3::Y * n.y.signum()
        } else {
            n
        }
    }

    /// Pushes a standing body out of the walls (like `CaveWorld::push_out`).
    pub fn push_out(&self, feet: Vec3, radius: f32, height: f32, step: f32) -> Option<Vec2> {
        if !self.contains(feet + Vec3::Y * (height * 0.5)) {
            return None;
        }
        let mut p = feet;
        let mut moved = false;
        for _ in 0..3 {
            let mut push = Vec2::ZERO;
            let n = 4;
            for k in 0..n {
                let y = p.y + step + (height - step) * (k as f32 + 0.5) / n as f32;
                let q = Vec3::new(p.x, y, p.z);
                let d = self.field(q);
                if d < radius {
                    let e = 0.08;
                    let gx = self.field(q + Vec3::X * e) - self.field(q - Vec3::X * e);
                    let gz = self.field(q + Vec3::Z * e) - self.field(q - Vec3::Z * e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dungeons::plan::*;

    fn space() -> Space {
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(8.0, 8.0)),
        ));
        let mut up = Room::new(
            0,
            RoomKind::Room,
            1,
            Shape::rect(Vec2::ZERO, Vec2::new(8.0, 8.0)),
        );
        up.name = "above".into();
        plan.add(up);
        Space {
            plan: Arc::new(Compiled::new(&plan, &|_| (0.6, false))),
            cut: None,
            interior: true,
            style: "crypt_stone",
            blockers: Default::default(),
        }
    }

    #[test]
    fn a_ray_from_above_finds_the_top_floor_then_with_the_cut_the_one_below() {
        let mut s = space();
        let (hit, _) = s
            .raycast(Vec3::new(0.0, 80.0, 0.5), Vec3::NEG_Y, 200.0)
            .expect("a hit");
        assert!((hit.y - LEVEL_STEP).abs() < 0.05, "top floor: {hit}");
        s.cut = Some(LEVEL_STEP - 1.0);
        let (hit, _) = s
            .raycast(Vec3::new(0.0, 80.0, 0.5), Vec3::NEG_Y, 200.0)
            .expect("a hit");
        assert!(
            hit.y.abs() < 0.05,
            "through the cut to the ground floor: {hit}"
        );
        assert!(s.normal_at(hit).y > 0.9);
    }

    #[test]
    fn footsteps_sound_like_the_rooms_floor() {
        use crate::terrain::layers::Surface;
        let mut plan = Plan::default();
        plan.add(Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::ZERO, Vec2::new(8.0, 8.0)),
        ));
        let mut works = Room::new(
            0,
            RoomKind::Room,
            0,
            Shape::rect(Vec2::new(40.0, 0.0), Vec2::new(8.0, 8.0)),
        );
        works.style = Some("clockwork_hp".into());
        plan.add(works);
        let s = Space {
            plan: Arc::new(Compiled::new(&plan, &|_| (0.6, false))),
            cut: None,
            interior: true,
            style: "crypt_stone",
            blockers: Default::default(),
        };
        assert_eq!(
            s.surface_at(Vec3::new(1.0, 0.0, 1.0)),
            Some(Surface::Cobble)
        );
        assert_eq!(
            s.surface_at(Vec3::new(41.0, 0.0, 1.0)),
            Some(Surface::Metal)
        );
        assert_eq!(
            s.surface_at(Vec3::new(20.0, 0.0, 1.0)),
            None,
            "in the rock between"
        );
    }

    #[test]
    fn walls_push_out() {
        let s = space();
        let q = s
            .push_out(Vec3::new(7.9, 0.0, 0.0), 0.35, 1.8, 0.4)
            .expect("pushed");
        assert!(q.x < 7.7, "{q}");
        assert!(
            s.push_out(Vec3::new(0.0, 0.0, 0.0), 0.35, 1.8, 0.4)
                .is_none()
        );
    }
}
