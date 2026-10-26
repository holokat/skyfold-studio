//! Where an object stands: its model's bounds (from the bundled libraries' `models.json`, the
//! procedural meshes, or the footprint radius as a last resort) and the oriented rectangle it
//! covers on the ground once placed (`Footprint`).

use std::collections::HashMap;

use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use serde::Deserialize;

use crate::objects::{Library, LibraryEntry, Placed};

/// Local bounds (min, max) of every library model that has known ones.
#[derive(Resource, Default)]
pub struct ModelBounds(pub HashMap<String, (Vec3, Vec3)>);

impl ModelBounds {
    /// The model's local bounds: known ones, else a square of the footprint radius from the
    /// ground up to its height.
    pub fn of(&self, e: &LibraryEntry) -> (Vec3, Vec3) {
        if let Some(b) = self.0.get(&e.id) {
            return *b;
        }
        // (the library's radius is 0.55 × the larger side)
        let h = (e.radius / 1.1).max(0.05);
        (Vec3::new(-h, 0.0, -h), Vec3::new(h, e.height.max(0.1), h))
    }
}

/// Reads the bundled libraries' bounds and measures the procedural meshes (startup, once the
/// library is built).
pub(super) fn collect(
    lib: Option<Res<Library>>,
    meshes: Res<Assets<Mesh>>,
    mut out: ResMut<ModelBounds>,
) {
    #[derive(Deserialize)]
    struct Model {
        id: String,
        #[serde(default)]
        aabb_min: Option<[f32; 3]>,
        #[serde(default)]
        aabb_max: Option<[f32; 3]>,
    }
    #[derive(Deserialize)]
    struct Models {
        models: Vec<Model>,
    }
    let dir = std::path::PathBuf::from(crate::assets_path()).join("models");
    for lib in crate::objects::model_libraries(&dir) {
        let Some(doc) = std::fs::read_to_string(dir.join(&lib.folder).join("models.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<Models>(&s).ok())
        else {
            continue;
        };
        for m in doc.models {
            if let (Some(a), Some(b)) = (m.aabb_min, m.aabb_max) {
                out.0.insert(
                    format!("{}:{}", lib.prefix, m.id),
                    (Vec3::from_array(a), Vec3::from_array(b)),
                );
            }
        }
    }
    let Some(lib) = lib else { return };
    for e in lib
        .entries
        .iter()
        .filter(|e| e.kind.is_some() && e.tree.is_none())
    {
        let Some(m) = e.meshes.first().and_then(|(m, _)| meshes.get(m)) else {
            continue;
        };
        let Ok(Some(VertexAttributeValues::Float32x3(p))) =
            m.try_attribute_option(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for q in p {
            lo = lo.min(Vec3::from_array(*q));
            hi = hi.max(Vec3::from_array(*q));
        }
        if lo.x <= hi.x {
            out.0.insert(e.id.clone(), (lo, hi));
        }
    }
}

/// An oriented rectangle on the ground (xz): centre, the direction of the model's local x axis,
/// and half sizes along its local x and z.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub center: Vec2,
    pub ax: Vec2,
    pub half: Vec2,
}

impl Footprint {
    /// What a placed object covers (its bounds × size, turned by its heading; tilt ignored).
    pub fn of(p: &Placed, bounds: (Vec3, Vec3)) -> Footprint {
        let s = p.scale3();
        let (lo, hi) = (bounds.0 * s, bounds.1 * s);
        let rot = p.rotation();
        let x = rot * Vec3::X;
        let ax = Vec2::new(x.x, x.z).try_normalize().unwrap_or(Vec2::X);
        let az = Vec2::new(-ax.y, ax.x);
        let c = (lo + hi) * 0.5;
        Footprint {
            center: Vec2::new(p.pos[0], p.pos[2]) + ax * c.x + az * c.z,
            ax,
            half: Vec2::new((hi.x - lo.x) * 0.5, (hi.z - lo.z) * 0.5).max(Vec2::splat(0.05)),
        }
    }

    /// The model's local z axis in the world (x, z).
    pub fn az(&self) -> Vec2 {
        Vec2::new(-self.ax.y, self.ax.x)
    }

    /// A point in the rectangle's own frame.
    pub fn local(&self, p: Vec2) -> Vec2 {
        let d = p - self.center;
        Vec2::new(d.dot(self.ax), d.dot(self.az()))
    }

    /// Signed distance to the rectangle's edge (negative inside).
    pub fn sdist(&self, p: Vec2) -> f32 {
        let q = self.local(p).abs() - self.half;
        q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0)
    }

    pub fn contains(&self, p: Vec2, margin: f32) -> bool {
        let q = self.local(p).abs();
        q.x <= self.half.x + margin && q.y <= self.half.y + margin
    }

    /// Corners, counter-clockwise from (−x, −z).
    pub fn corners(&self) -> [Vec2; 4] {
        let (a, b) = (self.ax * self.half.x, self.az() * self.half.y);
        [
            self.center - a - b,
            self.center + a - b,
            self.center + a + b,
            self.center - a + b,
        ]
    }

    /// World-axis bounds (min, max), grown by `margin`.
    pub fn aabb(&self, margin: f32) -> (Vec2, Vec2) {
        let e = Vec2::new(
            (self.ax.x * self.half.x).abs() + (self.az().x * self.half.y).abs(),
            (self.ax.y * self.half.x).abs() + (self.az().y * self.half.y).abs(),
        ) + Vec2::splat(margin);
        (self.center - e, self.center + e)
    }

    /// Whether it overlaps the world-axis rectangle `lo`..`hi`, grown by `margin`
    /// (separating axes).
    pub fn overlaps_rect(&self, lo: Vec2, hi: Vec2, margin: f32) -> bool {
        let half = self.half + Vec2::splat(margin);
        let (rc, re) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
        let d = rc - self.center;
        let (ax, az) = (self.ax, self.az());
        // world x and z
        let ex = (ax.x * half.x).abs() + (az.x * half.y).abs();
        let ez = (ax.y * half.x).abs() + (az.y * half.y).abs();
        if d.x.abs() > ex + re.x || d.y.abs() > ez + re.y {
            return false;
        }
        // the rectangle's own axes
        let px = (ax.x * re.x).abs() + (ax.y * re.y).abs();
        let pz = (az.x * re.x).abs() + (az.y * re.y).abs();
        d.dot(ax).abs() <= half.x + px && d.dot(az).abs() <= half.y + pz
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(x: f32, z: f32, yaw: f32, scale: f32) -> Placed {
        Placed {
            group: 0,
            id: 1,
            asset: "a".into(),
            variant: 0,
            pos: [x, 0.0, z],
            rot: Quat::from_rotation_y(yaw).to_array(),
            scale,
            stretch: None,
            locked: false,
            glow: None,
        }
    }

    #[test]
    fn footprint_follows_heading_and_scale() {
        let b = (Vec3::new(-2.0, 0.0, -4.0), Vec3::new(2.0, 5.0, 4.0));
        let f = Footprint::of(&placed(10.0, 20.0, 0.0, 1.0), b);
        assert!((f.center - Vec2::new(10.0, 20.0)).length() < 1e-4);
        assert!((f.half - Vec2::new(2.0, 4.0)).length() < 1e-4);
        // a quarter turn: the long side now runs along world x
        let f = Footprint::of(&placed(0.0, 0.0, std::f32::consts::FRAC_PI_2, 1.5), b);
        let (lo, hi) = f.aabb(0.0);
        assert!(
            (hi - lo - Vec2::new(12.0, 6.0)).length() < 1e-3,
            "{lo} {hi}"
        );
        assert!(f.contains(Vec2::new(5.9, 0.0), 0.0) && !f.contains(Vec2::new(0.0, 5.9), 0.0));
    }

    #[test]
    fn offset_bounds_move_the_centre() {
        let b = (Vec3::new(0.0, 0.0, -1.0), Vec3::new(4.0, 1.0, 1.0));
        let f = Footprint::of(&placed(0.0, 0.0, std::f32::consts::PI, 1.0), b);
        // turned half way round, the model's +x side is on the world's −x side
        assert!(
            (f.center - Vec2::new(-2.0, 0.0)).length() < 1e-4,
            "{}",
            f.center
        );
    }

    #[test]
    fn distances_and_overlaps() {
        let f = Footprint {
            center: Vec2::ZERO,
            ax: Vec2::new(1.0, 1.0).normalize(),
            half: Vec2::new(2.0, 1.0),
        };
        assert!(f.sdist(Vec2::ZERO) < 0.0);
        assert!((f.sdist(f.ax * 5.0) - 3.0).abs() < 1e-4);
        assert!(f.overlaps_rect(Vec2::new(1.0, 1.0), Vec2::new(2.0, 2.0), 0.0));
        // inside the turned rectangle's world bounds but off its corner
        assert!(!f.overlaps_rect(Vec2::new(-2.2, 1.2), Vec2::new(-1.8, 1.6), 0.0));
        assert!(f.overlaps_rect(Vec2::new(-2.2, 1.2), Vec2::new(-1.8, 1.6), 1.5));
        for c in f.corners() {
            assert!(f.sdist(c).abs() < 1e-4);
        }
    }
}
