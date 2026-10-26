//! Collision for the engine exports: simplified proxies instead of render meshes, physics
//! surface hints and water volumes.
//!
//! Every model variant that collides gets a proxy node `<asset>_v<k>_COL` (a root of its own,
//! never drawn): small props, rocks and logs a convex hull of at most `HULL_DIRS` support points
//! (under Unity's 255-triangle limit for convex mesh colliders); buildings, bridges and other
//! walk-in or large pieces their opaque triangles simplified to within `MESH_TOL` (floors, stairs
//! and door frames stay within a few centimetres; loose bits smaller than `PRUNE` go); trees a
//! trunk prism (engines with capsules use `trunk_radius_m` instead). `physics` maps a footstep
//! surface to friction and bounciness; `water_volumes` gives lakes, river stretches and the
//! ocean as boxes for swim / buoyancy triggers.

use bevy::math::{Vec3, Vec3Swizzles};
use serde_json::{Value, json};

use super::core::{AssetInfo, Collider, Snapshot, VariantOut};
use super::glb::{self, FlatPart, Glb, Prim};
use crate::objects::Category;
use crate::terrain::layers::Surface;

/// Support directions for convex hulls (at most this many hull vertices).
const HULL_DIRS: usize = 96;
/// Deviation allowed for walk-in collision meshes (m).
const MESH_TOL: f32 = 0.04;
/// Loose pieces smaller than this (bounding box diagonal, m) leave walk-in collision meshes.
const PRUNE: f32 = 0.25;

/// What collides for a packed model (generated ones keep `core::model`'s choice): walk-in and
/// big pieces collide with their (simplified) shape, everything else with a convex hull.
pub fn kind(a: &AssetInfo, size: f32) -> Collider {
    match a.category {
        Category::Trees => Collider::Capsule,
        Category::Plants => Collider::None,
        Category::Structures => Collider::Mesh,
        Category::Imported if a.walkable || size >= 4.0 => Collider::Mesh,
        Category::Rocks | Category::Debris | Category::Props | Category::Imported => {
            Collider::Convex
        }
    }
}

/// Friction and bounciness for a footstep surface (engine physics materials).
pub fn physics(s: Surface) -> (f32, f32) {
    match s {
        Surface::Grass | Surface::Leaves => (0.6, 0.05),
        Surface::Loam | Surface::Dirt | Surface::Ash | Surface::Cinder => (0.65, 0.03),
        Surface::Mud => (0.5, 0.0),
        Surface::Gravel | Surface::Cobble | Surface::Stone => (0.7, 0.08),
        Surface::WetStone => (0.45, 0.08),
        Surface::Marble | Surface::Salt => (0.5, 0.1),
        Surface::Sand => (0.75, 0.0),
        Surface::Snow => (0.35, 0.0),
        Surface::Ice => (0.05, 0.05),
        Surface::Water => (0.1, 0.0),
        Surface::Wood | Surface::Bone => (0.55, 0.15),
        Surface::Metal => (0.4, 0.1),
        Surface::Crystal => (0.3, 0.2),
        Surface::Carpet => (0.8, 0.02),
        Surface::Concrete | Surface::Asphalt => (0.75, 0.06),
        Surface::Tile | Surface::Glass => (0.45, 0.1),
        Surface::Rubber => (0.9, 0.2),
        Surface::Grate => (0.6, 0.1),
    }
}

pub fn physics_json(s: Surface) -> Value {
    let (f, b) = physics(s);
    json!({ "surface": s.sound_stem(), "friction": f, "bounciness": b })
}

// ---------------------------------------------------------------------------------------------
// convex hulls

/// Directions spread evenly over the sphere (a Fibonacci lattice).
fn directions(n: usize) -> Vec<Vec3> {
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let a = golden * i as f32;
            Vec3::new(r * a.cos(), y, r * a.sin())
        })
        .collect()
}

/// The convex hull of a few points (incremental; fine for the ~100 support points it gets).
/// `None` when they're flat or too few.
pub fn hull(points: &[Vec3]) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    // unique points (millimetres)
    let mut pts: Vec<Vec3> = vec![];
    for p in points {
        if !pts.iter().any(|q| q.distance_squared(*p) < 1e-6) {
            pts.push(*p);
        }
    }
    if pts.len() < 4 {
        return None;
    }
    let scale = pts
        .iter()
        .fold(0f32, |m, p| m.max(p.abs().max_element()))
        .max(1e-3);
    let eps = scale * 1e-5;
    // a first tetrahedron from extreme points
    let a = (0..pts.len()).min_by(|&i, &j| pts[i].x.total_cmp(&pts[j].x))?;
    let b = (0..pts.len()).max_by(|&i, &j| {
        pts[i]
            .distance_squared(pts[a])
            .total_cmp(&pts[j].distance_squared(pts[a]))
    })?;
    let ab = pts[b] - pts[a];
    let c = (0..pts.len()).max_by(|&i, &j| {
        ab.cross(pts[i] - pts[a])
            .length_squared()
            .total_cmp(&ab.cross(pts[j] - pts[a]).length_squared())
    })?;
    let n = ab.cross(pts[c] - pts[a]);
    if n.length() < eps {
        return None;
    }
    let d = (0..pts.len()).max_by(|&i, &j| {
        n.dot(pts[i] - pts[a])
            .abs()
            .total_cmp(&n.dot(pts[j] - pts[a]).abs())
    })?;
    if n.normalize().dot(pts[d] - pts[a]).abs() < eps * 10.0 {
        return None;
    }
    let centre = (pts[a] + pts[b] + pts[c] + pts[d]) * 0.25;
    let plane = |f: &[u32; 3]| {
        let (p0, p1, p2) = (pts[f[0] as usize], pts[f[1] as usize], pts[f[2] as usize]);
        let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        (n, n.dot(p0))
    };
    let mut faces: Vec<[u32; 3]> = vec![];
    for f in [[a, b, c], [a, c, d], [a, d, b], [b, d, c]] {
        let mut f = f.map(|v| v as u32);
        let (n, w) = plane(&f);
        if n.dot(centre) - w > 0.0 {
            f.swap(1, 2);
        }
        faces.push(f);
    }
    for (i, p) in pts.iter().enumerate() {
        if [a, b, c, d].contains(&i) {
            continue;
        }
        let visible: Vec<bool> = faces
            .iter()
            .map(|f| {
                let (n, w) = plane(f);
                n.dot(*p) - w > eps
            })
            .collect();
        if !visible.iter().any(|v| *v) {
            continue;
        }
        // horizon: edges of visible faces whose twin belongs to a hidden face
        let mut edges: Vec<(u32, u32)> = vec![];
        for (f, _) in faces.iter().zip(&visible).filter(|(_, v)| **v) {
            for k in 0..3 {
                edges.push((f[k], f[(k + 1) % 3]));
            }
        }
        let horizon: Vec<(u32, u32)> = edges
            .iter()
            .filter(|(x, y)| !edges.contains(&(*y, *x)))
            .copied()
            .collect();
        let mut kept: Vec<[u32; 3]> = faces
            .iter()
            .zip(&visible)
            .filter(|(_, v)| !**v)
            .map(|(f, _)| *f)
            .collect();
        for (x, y) in horizon {
            kept.push([x, y, i as u32]);
        }
        faces = kept;
    }
    // only the points on the hull
    let mut remap = vec![u32::MAX; pts.len()];
    let mut out = vec![];
    for f in faces.iter_mut() {
        for v in f.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = out.len() as u32;
                out.push(pts[*v as usize]);
            }
            *v = remap[*v as usize];
        }
    }
    Some((out, faces))
}

/// The hull of a model's extreme points in `HULL_DIRS` directions (a box if they're flat).
pub fn support_hull(parts: &[FlatPart]) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let pts: Vec<Vec3> = parts
        .iter()
        .flat_map(|p| p.pos.iter().map(|q| Vec3::from_array(*q)))
        .collect();
    if pts.is_empty() {
        return None;
    }
    let support: Vec<Vec3> = directions(HULL_DIRS)
        .iter()
        .filter_map(|d| {
            pts.iter()
                .copied()
                .max_by(|a, b| a.dot(*d).total_cmp(&b.dot(*d)))
        })
        .collect();
    hull(&support).or_else(|| {
        // flat: a thin box round it
        let (lo, hi) = pts
            .iter()
            .fold((Vec3::MAX, Vec3::MIN), |(l, h), p| (l.min(*p), h.max(*p)));
        let (lo, hi) = (lo - Vec3::splat(0.01), hi + Vec3::splat(0.01));
        let corners: Vec<Vec3> = (0..8)
            .map(|k| {
                Vec3::new(
                    if k & 1 == 0 { lo.x } else { hi.x },
                    if k & 2 == 0 { lo.y } else { hi.y },
                    if k & 4 == 0 { lo.z } else { hi.z },
                )
            })
            .collect();
        hull(&corners)
    })
}

/// A walk-in model's collision mesh: its opaque triangles welded and simplified to within
/// `MESH_TOL`, loose bits under `PRUNE` dropped.
pub fn mesh_proxy(
    parts: &[FlatPart],
    opaque: impl Fn(Option<usize>) -> bool,
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let mut pos: Vec<[f32; 3]> = vec![];
    let mut at: std::collections::HashMap<[i32; 3], u32> = std::collections::HashMap::new();
    let mut tris: Vec<[u32; 3]> = vec![];
    for p in parts.iter().filter(|p| opaque(p.material)) {
        let ids: Vec<u32> = p
            .pos
            .iter()
            .map(|q| {
                let key = q.map(|v| (v * 2000.0).round() as i32);
                *at.entry(key).or_insert_with(|| {
                    pos.push(*q);
                    pos.len() as u32 - 1
                })
            })
            .collect();
        for t in p.idx.chunks_exact(3) {
            let t = [ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]];
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                tris.push(t);
            }
        }
    }
    if tris.is_empty() {
        return None;
    }
    let wedge = vec![0u64; pos.len()];
    let mut kept =
        crate::model_render::simplify::simplify(&pos, &wedge, &tris, MESH_TOL, PRUNE, None);
    // very busy models: a coarser pass rather than a collider engines choke on
    if kept.len() > 60_000 {
        kept = crate::model_render::simplify::simplify(
            &pos,
            &wedge,
            &tris,
            MESH_TOL * 2.5,
            PRUNE,
            None,
        );
    }
    let mut remap = vec![u32::MAX; pos.len()];
    let mut out = vec![];
    for t in kept.iter_mut() {
        for v in t.iter_mut() {
            if remap[*v as usize] == u32::MAX {
                remap[*v as usize] = out.len() as u32;
                out.push(Vec3::from_array(pos[*v as usize]));
            }
            *v = remap[*v as usize];
        }
    }
    (!kept.is_empty()).then_some((out, kept))
}

/// An eight-sided trunk prism standing on the origin.
pub fn trunk_prism(radius: f32, height: f32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut pts = vec![];
    for y in [0.0, height] {
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::TAU / 8.0;
            pts.push(Vec3::new(radius * a.cos(), y, radius * a.sin()));
        }
    }
    hull(&pts).unwrap_or_default()
}

/// Adds a collision proxy root `<variant>_COL` per variant to a model's GLB and names it in the
/// variant. Models without variant nodes (packed files that couldn't be reorganised) are left as
/// they are: engines fall back to their render meshes.
pub fn add_proxies(bytes: Vec<u8>, variants: &mut [VariantOut], kind: Collider) -> Vec<u8> {
    if kind == Collider::None
        || variants
            .iter()
            .any(|v| v.node.is_empty() || v.lods.is_empty())
    {
        return bytes;
    }
    let Some(doc) = glb::glb_json(&bytes) else {
        return bytes;
    };
    // see-through materials (cut-out leaves, glass, webs) don't block
    let opaque: Vec<bool> = doc
        .get("materials")
        .and_then(|m| m.as_array())
        .map(|m| {
            m.iter()
                .map(|m| {
                    m.get("alphaMode")
                        .and_then(|a| a.as_str())
                        .is_none_or(|a| a == "OPAQUE")
                })
                .collect()
        })
        .unwrap_or_default();
    let mut proxies = vec![];
    for v in variants.iter() {
        let proxy = match kind {
            Collider::Capsule => {
                let h = (v.height * 0.6).min(8.0).max(v.trunk * 2.0 + 0.1);
                Some(trunk_prism(v.trunk.max(0.05), h))
            }
            Collider::Convex => {
                glb::flat_parts(&bytes, Some(&v.lods[0])).and_then(|p| support_hull(&p))
            }
            Collider::Mesh => glb::flat_parts(&bytes, Some(&v.lods[0])).and_then(|p| {
                mesh_proxy(&p, |m| {
                    m.is_none_or(|m| opaque.get(m).copied().unwrap_or(true))
                })
            }),
            Collider::None => None,
        };
        proxies.push(proxy);
    }
    let scene = doc.get("scene").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
    let mut roots: Vec<usize> = doc
        .get("scenes")
        .and_then(|s| s.get(scene))
        .and_then(|s| s.get("nodes"))
        .and_then(|n| n.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().map(|v| v as usize))
                .collect()
        })
        .unwrap_or_default();
    let Some(mut g) = Glb::open(&bytes) else {
        return bytes;
    };
    for (v, proxy) in variants.iter_mut().zip(proxies) {
        let Some((pts, tris)) = proxy else { continue };
        let pos: Vec<[f32; 3]> = pts.iter().map(|p| p.to_array()).collect();
        let idx: Vec<u32> = tris.iter().flatten().copied().collect();
        let name = format!("{}_COL", v.node);
        if let Some(m) = g.mesh(
            &name,
            &[Prim {
                positions: &pos,
                indices: &idx,
                ..Default::default()
            }],
        ) {
            roots.push(g.node(&name, Some(m), vec![]));
            v.collision = name;
        }
    }
    g.finish(&roots)
}

// ---------------------------------------------------------------------------------------------
// water volumes

/// One box of water (world metres, Y up): centre, full size along its own x (length), y and z
/// (width), turned `yaw` radians about +Y (counter-clockwise seen from above), and its surface.
/// `kind` is lake, river, ocean or lava (a glowing lake: hurts, nobody swims in it).
pub struct WaterVolume {
    pub kind: &'static str,
    pub name: String,
    pub center: [f32; 3],
    pub size: [f32; 3],
    pub yaw: f32,
    pub surface: f32,
}

impl WaterVolume {
    pub fn json(&self) -> Value {
        let r = |v: f32| (v * 100.0).round() / 100.0;
        json!({ "kind": self.kind, "name": self.name, "center": self.center.map(r), "size": self.size.map(r), "yaw": (self.yaw * 1e4).round() / 1e4, "surface": r(self.surface) })
    }
}

/// Lakes (one box from the bed to the surface), rivers (a box per stretch of about 30 m) and the
/// ocean (below sea level all round the map).
pub fn water_volumes(s: &Snapshot) -> Vec<WaterVolume> {
    let t = &s.terrain;
    let mut out = vec![];
    for (kind, name, mesh, style) in crate::water::export_bodies(&s.water, t) {
        if kind != "lake" {
            continue;
        }
        let kind = if style.emissive_strength > 0.3 {
            "lava"
        } else {
            "lake"
        };
        let arr = super::core::arrays(&mesh);
        if arr.pos.is_empty() {
            continue;
        }
        let (lo, hi) = arr
            .pos
            .iter()
            .fold(([f32::MAX; 3], [f32::MIN; 3]), |(l, h), p| {
                (
                    [l[0].min(p[0]), l[1].min(p[1]), l[2].min(p[2])],
                    [h[0].max(p[0]), h[1].max(p[1]), h[2].max(p[2])],
                )
            });
        let surface = hi[1];
        // the deepest ground under it
        let mut bed = surface;
        let step = t.cell.max(1.0);
        let (nx, nz) = (
            ((hi[0] - lo[0]) / step).ceil() as usize + 1,
            ((hi[2] - lo[2]) / step).ceil() as usize + 1,
        );
        for j in 0..nz.min(2048) {
            for i in 0..nx.min(2048) {
                bed = bed.min(t.height_at(lo[0] + i as f32 * step, lo[2] + j as f32 * step));
            }
        }
        let bottom = bed - 0.5;
        out.push(WaterVolume {
            kind,
            name,
            center: [
                (lo[0] + hi[0]) * 0.5,
                (surface + bottom) * 0.5,
                (lo[2] + hi[2]) * 0.5,
            ],
            size: [hi[0] - lo[0], surface - bottom, hi[2] - lo[2]],
            yaw: 0.0,
            surface,
        });
    }
    for r in s.water.rivers.iter().filter(|r| r.points.len() >= 2) {
        let name = if r.name.is_empty() {
            format!("River {}", r.id)
        } else {
            r.name.clone()
        };
        let pts: Vec<Vec3> = r.points.iter().map(|p| Vec3::from_array(*p)).collect();
        // stretches of about 30 m along the centreline
        let mut start = 0;
        while start + 1 < pts.len() {
            let mut end = start + 1;
            while end + 1 < pts.len() && pts[start].xz().distance(pts[end].xz()) < 30.0 {
                end += 1;
            }
            let (a, b) = (pts[start], pts[end]);
            let d = (b - a).xz();
            let len = d.length();
            if len > 0.01 {
                let surface = (start..=end).map(|k| pts[k].y).fold(f32::MIN, f32::max);
                let low = (start..=end).map(|k| pts[k].y).fold(f32::MAX, f32::min);
                let bottom = low - r.depth.max(0.5) - 0.5;
                let mid = (a + b) * 0.5;
                // (overlapping a little so bends have no gaps)
                out.push(WaterVolume {
                    kind: "river",
                    name: name.clone(),
                    center: [mid.x, (surface + bottom) * 0.5, mid.z],
                    size: [len + r.width * 0.5, surface - bottom, r.width + 1.0],
                    yaw: (-d.y).atan2(d.x),
                    surface,
                });
            }
            start = end;
        }
    }
    if t.heights.iter().any(|h| *h < t.sea_level) {
        let size = t.size() * 4.0;
        out.push(WaterVolume {
            kind: "ocean",
            name: "Ocean".into(),
            center: [0.0, t.sea_level - 250.0, 0.0],
            size: [size, 500.0, size],
            yaw: 0.0,
            surface: t.sea_level,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hulls_are_closed_and_convex() {
        let pts: Vec<Vec3> = directions(200)
            .into_iter()
            .map(|d| d * Vec3::new(2.0, 1.0, 1.5))
            .collect();
        let (v, f) = hull(&pts).unwrap();
        assert!(v.len() <= 200 && f.len() >= 4);
        // every edge has exactly one twin (closed), every vertex is behind every face (convex)
        for t in &f {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                assert_eq!(
                    f.iter()
                        .filter(|u| (0..3).any(|j| u[j] == b && u[(j + 1) % 3] == a))
                        .count(),
                    1
                );
            }
            let (p0, p1, p2) = (v[t[0] as usize], v[t[1] as usize], v[t[2] as usize]);
            let n = (p1 - p0).cross(p2 - p0).normalize();
            assert!(v.iter().all(|q| n.dot(*q - p0) < 1e-3));
        }
        // support hulls stay under Unity's 255-triangle limit
        let part = FlatPart {
            pos: pts.iter().map(|p| p.to_array()).collect(),
            nrm: vec![],
            tan: vec![],
            uv: vec![],
            color: vec![],
            idx: vec![0, 1, 2],
            material: None,
        };
        let (_, f) = support_hull(&[part]).unwrap();
        assert!(f.len() < 255, "{}", f.len());
        assert!(hull(&[Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)]).is_none());
    }

    #[test]
    fn walk_in_proxies_keep_floors() {
        // a floor of 80 × 80 quads with 2 cm bumps: the proxy is far lighter and keeps the floor
        // within the tolerance
        let n = 80u32;
        let mut floor = FlatPart {
            pos: vec![],
            nrm: vec![],
            tan: vec![],
            uv: vec![],
            color: vec![],
            idx: vec![],
            material: Some(0),
        };
        for j in 0..=n {
            for i in 0..=n {
                floor.pos.push([
                    i as f32 * 0.1,
                    ((i * 7 + j * 3) % 5) as f32 * 0.005,
                    j as f32 * 0.1,
                ]);
            }
        }
        for j in 0..n {
            for i in 0..n {
                let r = n + 1;
                floor.idx.extend_from_slice(&[
                    j * r + i,
                    (j + 1) * r + i,
                    j * r + i + 1,
                    j * r + i + 1,
                    (j + 1) * r + i,
                    (j + 1) * r + i + 1,
                ]);
            }
        }
        let (v, f) = mesh_proxy(&[floor], |_| true).unwrap();
        // (the quadric sums the squared distances to every merged plane, so bumpy ground keeps
        // more triangles than flat floors, which collapse to a few)
        assert!(f.len() * 3 < 12_800, "{}", f.len());
        assert!(v.iter().all(|p| p.y.abs() < MESH_TOL));
    }
}
