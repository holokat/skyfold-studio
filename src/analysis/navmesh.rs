//! Navmesh from the walkability grid, for custom game servers: walkable cells of one area type
//! (walk, road, swim) are merged greedily into rectangles that stay within a height tolerance of
//! the ground, neighbours' corners are inserted along shared edges (no T-junctions, so every
//! polygon edge has at most one neighbour), and polygons get their adjacency.
//!
//! Files: `navmesh.obj` (polygons grouped by area), `navmesh.json` (vertices, polygons, areas,
//! costs, adjacency), `walkable_heightfield.png` (16-bit heights of walkable cells, 0 = blocked)
//! and `walkable_areas.png` (8-bit area codes).

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::Path;

use bevy::prelude::*;
use serde_json::json;

use super::SteepRule;
use super::grid::{Class, WalkGrid};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Walk,
    Road,
    Swim,
    /// Only with the "climb" steep-slope rule.
    Steep,
}

impl Area {
    pub const ALL: [Area; 4] = [Area::Walk, Area::Road, Area::Swim, Area::Steep];
    pub fn name(self) -> &'static str {
        match self {
            Area::Walk => "walk",
            Area::Road => "road",
            Area::Swim => "swim",
            Area::Steep => "steep",
        }
    }
    /// Code in `walkable_areas.png`.
    pub fn code(self) -> u8 {
        match self {
            Area::Walk => 1,
            Area::Road => 2,
            Area::Swim => 3,
            Area::Steep => 4,
        }
    }
}

pub struct NavPoly {
    /// Vertex indices, counter-clockwise seen from above (+y normal).
    pub verts: Vec<u32>,
    pub area: Area,
    /// Travel cost per metre relative to running on open ground (roads < 1, water > 1).
    pub cost: f32,
    /// Per edge (`verts[i]` → `verts[i + 1]`): the polygon across it, or −1.
    pub neighbours: Vec<i32>,
}

pub struct NavMesh {
    pub verts: Vec<Vec3>,
    pub polys: Vec<NavPoly>,
}

pub struct NavStats {
    pub polygons: usize,
    pub vertices: usize,
    pub components: usize,
    /// Share of polygons in the largest connected component.
    pub largest_pct: f32,
    pub per_area: Vec<(Area, usize)>,
}

fn area_of(g: &WalkGrid, k: usize) -> Option<Area> {
    if !g.passable(k) {
        return None;
    }
    match g.class[k] {
        Class::Walk | Class::Shallow if g.road[k] > 0 => Some(Area::Road),
        Class::Walk | Class::Shallow => Some(Area::Walk),
        Class::Swim => Some(Area::Swim),
        Class::Steep if g.steep == SteepRule::Climb => Some(Area::Steep),
        _ => None,
    }
}

/// Builds the navmesh. `max_cells` caps a rectangle's side, `tolerance` (m) is how far the ground
/// inside a polygon may stray from it.
pub fn build(g: &WalkGrid, run_speed: f32, max_cells: usize, tolerance: f32) -> NavMesh {
    let n = g.n;
    let area: Vec<Option<Area>> = (0..g.len()).map(|k| area_of(g, k)).collect();
    // corner heights: mean surface of the walkable cells around each grid corner
    let nc = n + 1;
    let mut corner = vec![0.0f32; nc * nc];
    for cj in 0..nc {
        for ci in 0..nc {
            let (mut s, mut c, mut any) = (0.0, 0, 0.0);
            for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (i, j) = (ci as i32 - di, cj as i32 - dj);
                if i < 0 || j < 0 || i >= n as i32 || j >= n as i32 {
                    continue;
                }
                let k = j as usize * n + i as usize;
                any = g.surface[k];
                if area[k].is_some() {
                    s += g.surface[k];
                    c += 1;
                }
            }
            corner[cj * nc + ci] = if c > 0 { s / c as f32 } else { any };
        }
    }
    let flat = |i0: usize, j0: usize, w: usize, h: usize| -> bool {
        let c00 = corner[j0 * nc + i0];
        let c10 = corner[j0 * nc + i0 + w];
        let c01 = corner[(j0 + h) * nc + i0];
        let c11 = corner[(j0 + h) * nc + i0 + w];
        for j in j0..j0 + h {
            let v = (j - j0) as f32 + 0.5;
            let fz = v / h as f32;
            for i in i0..i0 + w {
                let fx = ((i - i0) as f32 + 0.5) / w as f32;
                let a = c00 + (c10 - c00) * fx;
                let b = c01 + (c11 - c01) * fx;
                if (g.surface[j * n + i] - (a + (b - a) * fz)).abs() > tolerance {
                    return false;
                }
            }
        }
        true
    };

    // greedy rectangles inside max_cells² tiles (tiles stop one long sliver from forcing the
    // next, like Recast's tiles); each rectangle grows row-first or column-first, whichever
    // covers more
    let mut used = vec![false; g.len()];
    let mut rects: Vec<(usize, usize, usize, usize, Area)> = Vec::new();
    for tj in (0..n).step_by(max_cells) {
        for ti in (0..n).step_by(max_cells) {
            let (ti1, tj1) = ((ti + max_cells).min(n), (tj + max_cells).min(n));
            for j in tj..tj1 {
                for i in ti..ti1 {
                    let k = j * n + i;
                    let Some(a) = area[k] else { continue };
                    if used[k] {
                        continue;
                    }
                    let free = |x: usize, y: usize| !used[y * n + x] && area[y * n + x] == Some(a);
                    let grow = |rows_first: bool| {
                        let (mut w, mut h) = (1, 1);
                        for pass in 0..2 {
                            if (pass == 0) == rows_first {
                                while i + w < ti1
                                    && (j..j + h).all(|y| free(i + w, y))
                                    && flat(i, j, w + 1, h)
                                {
                                    w += 1;
                                }
                            } else {
                                while j + h < tj1
                                    && (i..i + w).all(|x| free(x, j + h))
                                    && flat(i, j, w, h + 1)
                                {
                                    h += 1;
                                }
                            }
                        }
                        (w, h)
                    };
                    let (a1, a2) = (grow(true), grow(false));
                    let (w, h) = if a1.0 * a1.1 >= a2.0 * a2.1 { a1 } else { a2 };
                    for y in j..j + h {
                        for x in i..i + w {
                            used[y * n + x] = true;
                        }
                    }
                    rects.push((i, j, w, h, a));
                }
            }
        }
    }

    // vertices at rectangle corners
    let mut vid = vec![u32::MAX; nc * nc];
    let mut verts: Vec<Vec3> = Vec::new();
    let mut vertex = |ci: usize, cj: usize, vid: &mut Vec<u32>| -> u32 {
        let c = cj * nc + ci;
        if vid[c] == u32::MAX {
            vid[c] = verts.len() as u32;
            let p = g.origin + Vec2::new(ci as f32, cj as f32) * g.cell;
            verts.push(Vec3::new(p.x, corner[c], p.y));
        }
        vid[c]
    };
    for &(i, j, w, h, _) in &rects {
        for (ci, cj) in [(i, j), (i + w, j), (i, j + h), (i + w, j + h)] {
            vertex(ci, cj, &mut vid);
        }
    }
    // polygons: walk each rectangle's boundary, picking up the corners of its neighbours
    let mut polys: Vec<NavPoly> = Vec::with_capacity(rects.len());
    for &(i, j, w, h, a) in &rects {
        let mut v: Vec<u32> = Vec::with_capacity(8);
        let mut take = |ci: usize, cj: usize| {
            let id = vid[cj * nc + ci];
            if id != u32::MAX {
                v.push(id);
            }
        };
        let (x0, z0, x1, z1) = (i, j, i + w, j + h);
        for z in z0..z1 {
            take(x0, z);
        }
        for x in x0..x1 {
            take(x, z1);
        }
        for z in (z0 + 1..=z1).rev() {
            take(x1, z);
        }
        for x in (x0 + 1..=x1).rev() {
            take(x, z0);
        }
        let mut inv = 0.0;
        for y in j..j + h {
            for x in i..i + w {
                inv += 1.0 / g.speed[y * n + x].max(0.05);
            }
        }
        let cost = run_speed * inv / (w * h) as f32;
        polys.push(NavPoly {
            neighbours: vec![-1; v.len()],
            verts: v,
            area: a,
            cost,
        });
    }
    // adjacency through shared edges
    let mut edges: HashMap<(u32, u32), u32> = HashMap::with_capacity(polys.len() * 6);
    for (pi, p) in polys.iter().enumerate() {
        for e in 0..p.verts.len() {
            edges.insert((p.verts[e], p.verts[(e + 1) % p.verts.len()]), pi as u32);
        }
    }
    for p in polys.iter_mut() {
        let m = p.verts.len();
        for e in 0..m {
            if let Some(q) = edges.get(&(p.verts[(e + 1) % m], p.verts[e])) {
                p.neighbours[e] = *q as i32;
            }
        }
    }
    NavMesh { verts, polys }
}

impl NavMesh {
    /// Polygon counts and connectivity (connected components over the adjacency).
    pub fn stats(&self) -> NavStats {
        let mut comp = vec![u32::MAX; self.polys.len()];
        let mut sizes = Vec::new();
        let mut q = VecDeque::new();
        for s in 0..self.polys.len() {
            if comp[s] != u32::MAX {
                continue;
            }
            let c = sizes.len() as u32;
            comp[s] = c;
            q.push_back(s);
            let mut size = 0;
            while let Some(p) = q.pop_front() {
                size += 1;
                for &nb in &self.polys[p].neighbours {
                    if nb >= 0 && comp[nb as usize] == u32::MAX {
                        comp[nb as usize] = c;
                        q.push_back(nb as usize);
                    }
                }
            }
            sizes.push(size);
        }
        let largest = sizes.iter().copied().max().unwrap_or(0);
        NavStats {
            polygons: self.polys.len(),
            vertices: self.verts.len(),
            components: sizes.len(),
            largest_pct: if self.polys.is_empty() {
                0.0
            } else {
                largest as f32 * 100.0 / self.polys.len() as f32
            },
            per_area: Area::ALL
                .iter()
                .map(|a| (*a, self.polys.iter().filter(|p| p.area == *a).count()))
                .filter(|(_, c)| *c > 0)
                .collect(),
        }
    }

    pub fn write_obj(&self, path: &Path) -> std::io::Result<()> {
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(
            f,
            "# Skyfold Studio navmesh: {} vertices, {} polygons (y up, metres)",
            self.verts.len(),
            self.polys.len()
        )?;
        for v in &self.verts {
            writeln!(f, "v {:.2} {:.2} {:.2}", v.x, v.y, v.z)?;
        }
        for a in Area::ALL {
            if !self.polys.iter().any(|p| p.area == a) {
                continue;
            }
            writeln!(f, "g {}", a.name())?;
            for p in self.polys.iter().filter(|p| p.area == a) {
                write!(f, "f")?;
                for v in &p.verts {
                    write!(f, " {}", v + 1)?;
                }
                writeln!(f)?;
            }
        }
        f.flush()
    }

    pub fn write_json(
        &self,
        path: &Path,
        g: &WalkGrid,
        extra: serde_json::Value,
    ) -> std::io::Result<()> {
        let r2 = |v: f32| (v as f64 * 100.0).round() / 100.0;
        let verts: Vec<[f64; 3]> = self
            .verts
            .iter()
            .map(|v| [r2(v.x), r2(v.y), r2(v.z)])
            .collect();
        let polys: Vec<serde_json::Value> = self.polys.iter().map(|p| json!({ "v": p.verts, "area": p.area.name(), "cost": r2(p.cost), "n": p.neighbours })).collect();
        let st = self.stats();
        let doc = json!({
            "format": "world-builder-navmesh",
            "version": 1,
            "up": "y",
            "units": "metres",
            "cell_size": g.cell,
            "origin": [g.origin.x, g.origin.y],
            "grid": g.n,
            "winding": "counter-clockwise seen from above (+y normal)",
            "areas": Area::ALL.iter().map(|a| (a.name().to_string(), json!(a.code()))).collect::<serde_json::Map<_, _>>(),
            "stats": { "polygons": st.polygons, "vertices": st.vertices, "components": st.components, "largest_component_pct": r2(st.largest_pct) },
            "vertices": verts,
            "polygons": polys,
            "files": extra,
        });
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        serde_json::to_writer(&mut f, &doc).map_err(std::io::Error::other)?;
        f.flush()
    }
}

/// `walkable_heightfield.png` (16-bit: 0 = not walkable, else min + (v − 1) / 65534 · (max − min))
/// and `walkable_areas.png` (8-bit area codes). Row 0 is the −z edge, column 0 the −x edge.
/// Returns (min, max) height.
pub fn write_pngs(g: &WalkGrid, dir: &Path) -> Result<(f32, f32), String> {
    let n = g.n;
    let walk: Vec<Option<Area>> = (0..g.len()).map(|k| area_of(g, k)).collect();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for k in 0..g.len() {
        if walk[k].is_some() {
            lo = lo.min(g.surface[k]);
            hi = hi.max(g.surface[k]);
        }
    }
    if lo > hi {
        (lo, hi) = (0.0, 1.0);
    }
    let span = (hi - lo).max(1e-3);
    let hf: Vec<u16> = (0..g.len())
        .map(|k| {
            if walk[k].is_some() {
                1 + ((g.surface[k] - lo) / span * 65534.0).round() as u16
            } else {
                0
            }
        })
        .collect();
    image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(n as u32, n as u32, hf)
        .ok_or("heightfield size")?
        .save(dir.join("walkable_heightfield.png"))
        .map_err(|e| e.to_string())?;
    let areas: Vec<u8> = walk
        .iter()
        .map(|a| a.map(|a| a.code()).unwrap_or(0))
        .collect();
    image::GrayImage::from_raw(n as u32, n as u32, areas)
        .ok_or("areas size")?
        .save(dir.join("walkable_areas.png"))
        .map_err(|e| e.to_string())?;
    Ok((lo, hi))
}

/// Builds and writes everything into `dir`. Returns the stats.
pub fn export(g: &WalkGrid, run_speed: f32, dir: &Path) -> Result<NavStats, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let nav = build(g, run_speed, 24, 0.6);
    nav.write_obj(&dir.join("navmesh.obj"))
        .map_err(|e| e.to_string())?;
    let (lo, hi) = write_pngs(g, dir)?;
    let extra = json!({
        "obj": "navmesh.obj",
        "heightfield": { "file": "walkable_heightfield.png", "bits": 16, "min_height": lo, "max_height": hi, "decode": "0 = not walkable; else min_height + (v - 1) / 65534 * (max_height - min_height)", "row0": "-z edge", "col0": "-x edge" },
        "areas": { "file": "walkable_areas.png", "codes": { "0": "blocked", "1": "walk", "2": "road", "3": "swim", "4": "steep" } },
    });
    nav.write_json(&dir.join("navmesh.json"), g, extra)
        .map_err(|e| e.to_string())?;
    Ok(nav.stats())
}
