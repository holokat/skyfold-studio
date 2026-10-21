//! The draft's ground from an analysed picture: the coastline (sub-cell, lightly roughened),
//! land rising with distance from the coast, each region's height role (ridged ranges along the
//! massif's long axis, rolling hills, gentle plains, wetlands just above the water), the picture's
//! shading as a relief hint, a seabed shallow where the picture's water is light, valleys for
//! the rivers and level ground around the lakes, then rain erosion. Pure data in, heights out (no
//! engine types).

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::analyse::{self, Analysis, Base, NONE, Role, SEA};
use crate::noise::{fbm, perlin2, smoothstep};

/// Where the picture lies on the world: its centre (m), its width (m) along its own x axis and
/// its rotation (degrees clockwise seen from above with north up). The aspect ratio is kept.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub center: [f32; 2],
    pub width: f32,
    pub rotation: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            width: 1000.0,
            rotation: 0.0,
        }
    }
}

impl Placement {
    /// uv (0..1 from the picture's top-left) → world [x, z]. `aspect` is height / width.
    pub fn to_world(&self, uv: [f32; 2], aspect: f32) -> [f32; 2] {
        let (lx, lz) = (
            (uv[0] - 0.5) * self.width,
            (uv[1] - 0.5) * self.width * aspect,
        );
        let (s, c) = self.rotation.to_radians().sin_cos();
        [
            self.center[0] + lx * c - lz * s,
            self.center[1] + lx * s + lz * c,
        ]
    }
    /// World [x, z] → uv.
    pub fn to_uv(&self, p: [f32; 2], aspect: f32) -> [f32; 2] {
        let (dx, dz) = (p[0] - self.center[0], p[1] - self.center[1]);
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (lx, lz) = (dx * c + dz * s, -dx * s + dz * c);
        [lx / self.width + 0.5, lz / (self.width * aspect) + 0.5]
    }
    /// The picture fitted into a world of `size` m, `margin` (share of the size) kept clear on
    /// every side.
    pub fn fit(size: f32, aspect: f32, margin: f32) -> Self {
        let avail = size * (1.0 - 2.0 * margin.clamp(0.0, 0.4));
        let width = if aspect <= 1.0 { avail } else { avail / aspect };
        Placement {
            center: [0.0, 0.0],
            width,
            rotation: 0.0,
        }
    }
    /// The four corners (top-left, top-right, bottom-right, bottom-left) in world space.
    pub fn corners(&self, aspect: f32) -> [[f32; 2]; 4] {
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|uv| self.to_world(uv, aspect))
    }
}

/// The world's height grid.
#[derive(Clone, Copy, Debug)]
pub struct Grid {
    /// Cells per side (vertices: `res + 1`).
    pub res: usize,
    pub cell: f32,
    pub sea: f32,
}

impl Grid {
    fn n(&self) -> usize {
        self.res + 1
    }
    fn size(&self) -> f32 {
        self.res as f32 * self.cell
    }
    fn world(&self, i: usize, j: usize) -> [f32; 2] {
        let half = self.size() * 0.5;
        [i as f32 * self.cell - half, j as f32 * self.cell - half]
    }
}

/// How the draft's ground is shaped (saved with the world).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReliefParams {
    /// Height of the highest peaks above the sea (m); 0 picks one from the world size.
    pub peak_height: f32,
    /// Read the picture's hill shading as extra relief in hills and mountains.
    pub shading: bool,
    /// Rain erosion, 0 (none) … 1 (strong).
    pub erosion: f32,
    /// Coastline raggedness beyond what the picture shows, 0..1.
    pub roughness: f32,
}

impl Default for ReliefParams {
    fn default() -> Self {
        Self {
            peak_height: 0.0,
            shading: true,
            erosion: 0.6,
            roughness: 0.5,
        }
    }
}

impl ReliefParams {
    /// The peak height for a world of `size` m.
    pub fn peak_for(&self, size: f32) -> f32 {
        if self.peak_height > 0.0 {
            self.peak_height
        } else {
            (size * 0.075).clamp(220.0, 900.0)
        }
    }
}

/// A lake to carve: shoreline (world) and the region it lies in (its level follows the levelled
/// ground around it).
pub struct DraftLake {
    pub outline: Vec<[f32; 2]>,
    pub region: u32,
}

/// A river course from source to mouth (world) and its width (m).
pub struct DraftRiver {
    pub pts: Vec<[f32; 2]>,
    pub width: f32,
}

pub struct Draft {
    pub heights: Vec<f32>,
    /// Region index per vertex (`NONE` at sea), with organic borders.
    pub region: Vec<u32>,
    pub lakes: Vec<DraftLake>,
    pub rivers: Vec<DraftRiver>,
}

/// Per-role shape: (base lift, rolling-hill amplitude, mountain amplitude, wetness), as
/// shares of the peak height (wetness 0..1).
fn role_shape(r: Role) -> [f32; 4] {
    match r {
        Role::Peaks => [0.08, 0.05, 1.0, 0.0],
        Role::Mountains => [0.05, 0.06, 0.62, 0.0],
        Role::Hills => [0.025, 0.14, 0.0, 0.0],
        Role::Plains => [0.008, 0.04, 0.0, 0.0],
        Role::Lowland => [0.0, 0.012, 0.0, 0.35],
        Role::Wetland => [0.0, 0.004, 0.0, 1.0],
    }
}

/// Bilinear sample of a grid field at uv (clamped to the edge).
fn sample(f: &[f32], w: usize, h: usize, uv: [f32; 2]) -> f32 {
    let x = (uv[0] * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (uv[1] * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let a = f[y0 * w + x0] + (f[y0 * w + x1] - f[y0 * w + x0]) * fx;
    let b = f[y1 * w + x0] + (f[y1 * w + x1] - f[y1 * w + x0]) * fx;
    a + (b - a) * fy
}

fn inside01(uv: [f32; 2]) -> bool {
    (0.0..1.0).contains(&uv[0]) && (0.0..1.0).contains(&uv[1])
}

/// Ridged noise along `axis` (radians, world): crests run along the axis.
fn ridges(p: [f32; 2], axis: f32, spacing: f32, seed: u32) -> f32 {
    let (s, c) = axis.sin_cos();
    let along = (p[0] * c + p[1] * s) / spacing;
    let across = (-p[0] * s + p[1] * c) / spacing;
    let (mut sum, mut amp, mut norm, mut weight) = (0.0, 0.5, 0.0, 1.0f32);
    let (mut x, mut y) = (along * 0.38, across);
    for o in 0..6u32 {
        let n = perlin2(x, y, seed.wrapping_add(o * 7919));
        // soft absolute value: rounded crests, no knife edges
        let r = (1.0 - (n * n + 0.004).sqrt()).max(0.0);
        let r = r * r * weight;
        weight = (r * 1.6).clamp(0.0, 1.0);
        sum += r * amp;
        norm += amp;
        amp *= 0.5;
        // later octaves turn a little so small ridges branch off the main crests
        let (ss, cc) = (0.55f32).sin_cos();
        let (nx, ny) = (x * cc - y * ss, x * ss + y * cc);
        x = nx * 2.03;
        y = ny * 2.03;
    }
    (sum / norm / 0.75).clamp(0.0, 1.2)
}

/// Shapes the ground of a `g` world from the analysis.
pub fn relief(
    an: &Analysis,
    base: &Base,
    place: &Placement,
    g: &Grid,
    p: &ReliefParams,
    seed: u32,
) -> Draft {
    let (aw, ah) = (an.w, an.h);
    let an_n = aw * ah;
    let aspect = ah as f32 / aw as f32;
    let n = g.n();
    let nn = n * n;
    let peak = p.peak_for(g.size());
    // metres per analysis cell
    let acell = place.width / aw as f32;

    // --- analysis-grid fields
    let solid: Vec<f32> = an
        .class
        .iter()
        .map(|c| if *c != SEA { 1.0 } else { 0.0 })
        .collect();
    let solid_soft = analyse::blur(&solid, None, aw, ah, 1);
    // the region of the nearest land cell (so smoothed coastlines always find one)
    let land_known: Vec<bool> = an.region.iter().map(|r| *r != NONE).collect();
    let near_region: Vec<u32> = {
        let src = nearest_cell(&land_known, aw, ah);
        (0..an_n)
            .map(|k| {
                if src[k] == NONE {
                    NONE
                } else {
                    an.region[src[k] as usize]
                }
            })
            .collect()
    };
    let roles: Vec<[f32; 4]> = an.regions.iter().map(|r| role_shape(r.role)).collect();
    // mountain mass: distance into the union of mountain regions, per massif normalised to 1 at
    // its spine; each massif's long axis orients its ridges
    let mountain: Vec<bool> = (0..an_n)
        .map(|k| {
            near_region[k] != NONE && an.class[k] != SEA && roles[near_region[k] as usize][2] > 0.0
        })
        .collect();
    let not_mountain: Vec<bool> = mountain.iter().map(|m| !m).collect();
    let into = analyse::edt(&not_mountain, aw, ah);
    let mkey: Vec<u32> = mountain.iter().map(|m| if *m { 0 } else { NONE }).collect();
    let (mcomp, msizes) = analyse::components(&mkey, aw, ah, true);
    let mut mmax = vec![1e-3f32; msizes.len()];
    let mut mom = vec![[0f64; 6]; msizes.len()];
    for k in 0..an_n {
        if mcomp[k] != NONE {
            let c = mcomp[k] as usize;
            mmax[c] = mmax[c].max(into[k]);
            let (x, y) = ((k % aw) as f64, (k / aw) as f64);
            let m = &mut mom[c];
            m[0] += 1.0;
            m[1] += x;
            m[2] += y;
            m[3] += x * x;
            m[4] += y * y;
            m[5] += x * y;
        }
    }
    // massif axes in world space (picture axis + the placement's rotation)
    let axes: Vec<f32> = mom
        .iter()
        .map(|m| {
            let (mx, my) = (m[1] / m[0].max(1.0), m[2] / m[0].max(1.0));
            let (sxx, syy, sxy) = (
                m[3] / m[0].max(1.0) - mx * mx,
                m[4] / m[0].max(1.0) - my * my,
                m[5] / m[0].max(1.0) - mx * my,
            );
            (0.5 * (2.0 * sxy).atan2(sxx - syy)) as f32 + place.rotation.to_radians()
        })
        .collect();
    let spacing: Vec<f32> = msizes
        .iter()
        .map(|s| ((*s as f32).sqrt() * acell * 0.11).clamp(160.0, 900.0))
        .collect();
    // a massif can't stand much taller than it is wide (no needles on islets)
    let cap: Vec<f32> = mmax.iter().map(|m| m * acell * 1.1).collect();
    let massif: Vec<f32> = (0..an_n)
        .map(|k| {
            if mcomp[k] == NONE {
                0.0
            } else {
                (into[k] / mmax[mcomp[k] as usize]).min(1.0)
            }
        })
        .collect();
    let massif = analyse::blur(&massif, None, aw, ah, 2);
    // which massif a cell belongs to (also just outside it, for the blurred edge)
    let mnear: Vec<u32> = {
        let src = nearest_cell(&mountain, aw, ah);
        (0..an_n)
            .map(|k| {
                if src[k] == NONE {
                    NONE
                } else {
                    mcomp[src[k] as usize]
                }
            })
            .collect()
    };

    // --- per vertex: picture position, land, region
    let warp_m = acell * 1.6;
    let f_warp = 1.0 / (acell * 7.0);
    let f_coast = 1.0 / (acell * 2.5);
    let rough = 0.08 + 0.22 * p.roughness.clamp(0.0, 1.0);
    let rows: Vec<(Vec<f32>, Vec<u32>)> = (0..n)
        .into_par_iter()
        .map(|j| {
            let mut land = Vec::with_capacity(n);
            let mut reg = Vec::with_capacity(n);
            for i in 0..n {
                let w = g.world(i, j);
                let uv = place.to_uv(w, aspect);
                if !inside01(uv) {
                    land.push(-1.0);
                    reg.push(NONE);
                    continue;
                }
                let s = sample(&solid_soft, aw, ah, uv) - 0.5
                    + fbm(w[0] * f_coast, w[1] * f_coast, 4, 2.0, 0.55, seed + 31) * rough;
                land.push(s);
                // organic region borders: look the region up at a wandering position
                let d = [
                    fbm(w[0] * f_warp, w[1] * f_warp, 3, 2.0, 0.5, seed + 41) * warp_m,
                    fbm(
                        w[0] * f_warp + 7.3,
                        w[1] * f_warp - 2.1,
                        3,
                        2.0,
                        0.5,
                        seed + 43,
                    ) * warp_m,
                ];
                let uv2 = place.to_uv([w[0] + d[0], w[1] + d[1]], aspect);
                let x = ((uv2[0] * aw as f32) as isize).clamp(0, aw as isize - 1) as usize;
                let y = ((uv2[1] * ah as f32) as isize).clamp(0, ah as isize - 1) as usize;
                reg.push(near_region[y * aw + x]);
            }
            (land, reg)
        })
        .collect();
    let landness: Vec<f32> = rows.iter().flat_map(|r| r.0.iter().copied()).collect();
    let mut region: Vec<u32> = rows.into_iter().flat_map(|r| r.1).collect();
    let is_land: Vec<bool> = landness.iter().map(|s| *s > 0.0).collect();
    for k in 0..nn {
        if !is_land[k] {
            region[k] = NONE;
        }
    }
    let is_sea: Vec<bool> = is_land.iter().map(|l| !l).collect();
    let d_coast = analyse::edt(&is_sea, n, n);
    let d_sea = analyse::edt(&is_land, n, n);

    // role fields per vertex, blended across region borders over ~200 m
    // (the fifth: sand dunes in low-lying deserts)
    let dunes: Vec<f32> = an
        .regions
        .iter()
        .map(|r| {
            if r.biome == "Desert" && matches!(r.role, Role::Plains | Role::Lowland) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let mut fields: Vec<Vec<f32>> = vec![vec![0.0; nn]; 5];
    for k in 0..nn {
        if region[k] != NONE {
            let ri = region[k] as usize;
            for (f, v) in fields
                .iter_mut()
                .zip(roles[ri].into_iter().chain([dunes[ri]]))
            {
                f[k] = v;
            }
        }
    }
    let blend_r = ((200.0 / g.cell) as usize).max(2);
    let weight: Vec<f32> = region
        .iter()
        .map(|r| if *r != NONE { 1.0 } else { 0.0 })
        .collect();
    let fields: Vec<Vec<f32>> = fields
        .into_iter()
        .map(|f| {
            let once = analyse::blur(&f, Some(&weight), n, n, blend_r);
            analyse::blur(&once, Some(&weight), n, n, blend_r)
        })
        .collect();

    // --- heights
    let size = g.size();
    let coast_len = (size * 0.06).clamp(150.0, 900.0);
    let lift = peak * 0.1;
    let f_hill = 1.0 / 520.0;
    let shading = p.shading;
    let mut heights: Vec<f32> = (0..nn)
        .into_par_iter()
        .map(|k| {
            let (i, j) = (k % n, k / n);
            let w = g.world(i, j);
            let uv = place.to_uv(w, aspect);
            if !is_land[k] {
                // seabed: shelf near the coast, shallower where the picture's water is light
                let d = d_sea[k] * g.cell;
                let shallow = if inside01(uv) {
                    sample(&base.shallow, aw, ah, uv)
                } else {
                    0.0
                };
                let depth = 1.5
                    + 36.0 * (1.0 - (-d / 260.0).exp()) * (1.0 - 0.8 * shallow)
                    + 6.0 * smoothstep(0.0, 1.0, d / 900.0);
                return g.sea - depth
                    + fbm(w[0] * 0.004, w[1] * 0.004, 3, 2.0, 0.5, seed + 51) * 1.5;
            }
            let d = d_coast[k] * g.cell;
            let [base_lift, hill_amp, mount_amp, wet, dune] = [
                fields[0][k],
                fields[1][k],
                fields[2][k],
                fields[3][k],
                fields[4][k],
            ];
            // land rises gently with distance from the coast
            let rise = 0.8 + lift * (1.0 - (-d / coast_len).exp()) * (1.0 - wet);
            let inland = smoothstep(0.0, 160.0, d);
            // rolling hills (and the faint swell of plains)
            let hn = fbm(w[0] * f_hill, w[1] * f_hill, 5, 2.0, 0.5, seed + 61) * 0.5 + 0.5;
            let hills = hill_amp * peak * hn.powf(1.3) * inland;
            // mountains: ranges along the massif's axis, highest along its spine
            let mut mount = 0.0;
            if mount_amp > 0.002 {
                let m = sample(&massif, aw, ah, uv).clamp(0.0, 1.0);
                let x = ((uv[0] * aw as f32) as usize).min(aw - 1);
                let y = ((uv[1] * ah as f32) as usize).min(ah - 1);
                let c = mnear[y * aw + x];
                let (axis, sp, top) = if c == NONE {
                    (0.0, 400.0, f32::MAX)
                } else {
                    (axes[c as usize], spacing[c as usize], cap[c as usize])
                };
                let r = ridges(w, axis, sp, seed + 71);
                // a broad massif rising to its spine, broken into peaks and saddles by the ridges
                let body = 0.6 * smoothstep(0.0, 0.65, m) + 0.4 * m;
                mount = (mount_amp * peak).min(top) * body * (0.35 + 0.85 * r);
            }
            // the picture's own hill shading as fine relief
            let mut hint = 0.0;
            if shading {
                // fades in with the relief (no seams where plains meet hills)
                let rugged = smoothstep(0.04, 0.12, hill_amp + mount_amp);
                hint = sample(&base.relief, aw, ah, uv)
                    * (mount_amp * 0.16 + hill_amp * 0.5)
                    * peak
                    * inland
                    * rugged;
            }
            // dune ridges across the wind (crests north–south), a few metres high
            let sand = if dune > 0.01 {
                dune * inland
                    * peak
                    * 0.014
                    * ridges(w, std::f32::consts::FRAC_PI_2, 150.0, seed + 91)
            } else {
                0.0
            };
            let ground = g.sea + rise + base_lift * peak * inland + hills + mount + hint + sand;
            // wetlands sit just above the water
            let marsh = g.sea
                + 0.6
                + 0.7 * (fbm(w[0] * 0.01, w[1] * 0.01, 3, 2.0, 0.5, seed + 81) * 0.5 + 0.5);
            ground + (marsh - ground) * wet.clamp(0.0, 1.0)
        })
        .collect();

    // --- rivers: valleys falling all the way to their mouths
    let to_world = |pt: [f32; 2]| place.to_world([pt[0] / aw as f32, pt[1] / ah as f32], aspect);
    let h_at = |hs: &[f32], w: [f32; 2]| -> f32 {
        let half = size * 0.5;
        let x = ((w[0] + half) / g.cell).clamp(0.0, g.res as f32);
        let z = ((w[1] + half) / g.cell).clamp(0.0, g.res as f32);
        hs[(z.round() as usize) * n + x.round() as usize]
    };
    let mut rivers = vec![];
    for r in &an.rivers {
        let mut pts: Vec<[f32; 2]> = r.pts.iter().map(|q| to_world(*q)).collect();
        let width =
            (r.width.iter().sum::<f32>() / r.width.len().max(1) as f32 * acell).clamp(5.0, 45.0);
        // downstream: the end at the sea or a lake, else the lower end
        let down_last = match r.mouth {
            [true, false] => false,
            [false, true] => true,
            _ => h_at(&heights, pts[0]) >= h_at(&heights, *pts.last().unwrap()),
        };
        if !down_last {
            pts.reverse();
        }
        let pts = densify(&pts, g.cell * 2.0);
        if pts.len() < 3 {
            continue;
        }
        // the ground where it ends: the coast, or a lake's shore
        let mouth = (h_at(&heights, *pts.last().unwrap()) - 1.0).max(g.sea);
        carve_valley(&mut heights, g, &pts, width, mouth);
        rivers.push(DraftRiver { pts, width });
    }

    // --- lakes: level ground around a basin
    let mut lakes = vec![];
    for l in &an.lakes {
        let outline: Vec<[f32; 2]> = l
            .outline
            .iter()
            .map(|uv| place.to_world(*uv, aspect))
            .collect();
        if outline.len() < 3 {
            continue;
        }
        let mut ring: Vec<f32> = outline.iter().map(|q| h_at(&heights, *q)).collect();
        ring.sort_by(|a, b| a.total_cmp(b));
        let level = ring[ring.len() / 3].max(g.sea + 1.5);
        let reg = {
            let c = centroid(&outline);
            let uv = place.to_uv(c, aspect);
            let x = ((uv[0] * aw as f32) as usize).min(aw - 1);
            let y = ((uv[1] * ah as f32) as usize).min(ah - 1);
            near_region[y * aw + x]
        };
        level_lake(&mut heights, g, &outline, level);
        lakes.push(DraftLake {
            outline,
            region: reg,
        });
    }

    // --- rain, then the coastline exactly where the picture has it
    erode_draft(&mut heights, g.res, g.cell, p.erosion, seed);
    for k in 0..nn {
        if is_land[k] {
            heights[k] = heights[k].max(g.sea + 0.35);
        } else {
            heights[k] = heights[k].min(g.sea - 0.6);
        }
    }
    Draft {
        heights,
        region,
        lakes,
        rivers,
    }
}

/// Rain erosion of the draft (one place to swap in a better erosion).
pub fn erode_draft(heights: &mut [f32], res: usize, cell: f32, strength: f32, seed: u32) {
    if strength <= 0.0 {
        return;
    }
    let droplets = (res * res) as f32 / 14.0 * strength.clamp(0.0, 1.0);
    crate::terrain::worldgen::erode(
        heights,
        res,
        cell,
        droplets as usize,
        seed as u64,
        None,
        0.8,
    );
    crate::terrain::worldgen::smooth_all(heights, res, 0.45);
}

/// For each cell, the index of the nearest `known` cell (`NONE` if there is none).
fn nearest_cell(known: &[bool], w: usize, h: usize) -> Vec<u32> {
    if !known.iter().any(|k| *k) {
        return vec![NONE; w * h];
    }
    analyse::nearest_known(known, w, h)
}

fn centroid(p: &[[f32; 2]]) -> [f32; 2] {
    let n = p.len().max(1) as f32;
    [
        p.iter().map(|q| q[0]).sum::<f32>() / n,
        p.iter().map(|q| q[1]).sum::<f32>() / n,
    ]
}

fn densify(p: &[[f32; 2]], step: f32) -> Vec<[f32; 2]> {
    let mut out = vec![];
    for w in p.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        let k = (d / step).ceil().max(1.0) as usize;
        for s in 0..k {
            let t = s as f32 / k as f32;
            out.push([
                w[0][0] + (w[1][0] - w[0][0]) * t,
                w[0][1] + (w[1][1] - w[0][1]) * t,
            ]);
        }
    }
    if let Some(l) = p.last() {
        out.push(*l);
    }
    out
}

/// Grid rect (inclusive vertex range) around a world-space box, padded.
fn rect(g: &Grid, lo: [f32; 2], hi: [f32; 2], pad: f32) -> (usize, usize, usize, usize) {
    let half = g.size() * 0.5;
    let f = |v: f32| ((v + half) / g.cell).clamp(0.0, g.res as f32);
    (
        f(lo[0] - pad).floor() as usize,
        f(lo[1] - pad).floor() as usize,
        f(hi[0] + pad).ceil() as usize,
        f(hi[1] + pad).ceil() as usize,
    )
}

/// A V-shaped valley along a river (source → mouth) whose floor falls steadily to `mouth`.
fn carve_valley(hs: &mut [f32], g: &Grid, pts: &[[f32; 2]], width: f32, mouth: f32) {
    let n = g.n();
    let half = g.size() * 0.5;
    let at = |hs: &[f32], q: [f32; 2]| {
        let x = ((q[0] + half) / g.cell).round().clamp(0.0, g.res as f32) as usize;
        let z = ((q[1] + half) / g.cell).round().clamp(0.0, g.res as f32) as usize;
        hs[z * n + x]
    };
    // floor: follows the ground down, never rising, ending at the mouth
    let m = pts.len();
    let mut floor = vec![0f32; m];
    let mut cur = at(hs, pts[0]) - 2.0;
    let total: f32 = pts
        .windows(2)
        .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
        .sum::<f32>()
        .max(1.0);
    let mut run = 0.0;
    for i in 0..m {
        if i > 0 {
            run +=
                ((pts[i][0] - pts[i - 1][0]).powi(2) + (pts[i][1] - pts[i - 1][1]).powi(2)).sqrt();
        }
        let ground = at(hs, pts[i]) - 1.5;
        // at least a gentle fall towards the mouth level
        let towards = mouth + (cur - mouth) * (1.0 - 0.15 / m as f32);
        cur = cur
            .min(ground)
            .min(towards)
            .max(mouth + (1.0 - run / total) * 0.5);
        floor[i] = cur;
    }
    let reach = (width * 6.0).max(70.0);
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in pts {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    let (x0, z0, x1, z1) = rect(g, lo, hi, reach);
    let updates: Vec<(usize, f32)> = (z0..=z1)
        .into_par_iter()
        .flat_map_iter(|j| {
            let mut out = vec![];
            for i in x0..=x1 {
                let w = g.world(i, j);
                let mut best = (f32::MAX, 0.0);
                for s in 0..m - 1 {
                    let (a, b) = (pts[s], pts[s + 1]);
                    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
                    let t = (((w[0] - a[0]) * dx + (w[1] - a[1]) * dz)
                        / (dx * dx + dz * dz).max(1e-6))
                    .clamp(0.0, 1.0);
                    let d =
                        ((w[0] - a[0] - dx * t).powi(2) + (w[1] - a[1] - dz * t).powi(2)).sqrt();
                    if d < best.0 {
                        best = (d, floor[s] + (floor[s + 1] - floor[s]) * t);
                    }
                }
                let (d, f) = best;
                if d < reach {
                    let k = j * n + i;
                    let x = (d - width * 0.5).max(0.0) / (reach - width * 0.5);
                    out.push((k, f + (hs[k] - f).max(0.0) * x.powf(0.85)));
                }
            }
            out
        })
        .collect();
    for (k, v) in updates {
        hs[k] = hs[k].min(v);
    }
}

/// Levels the ground in and around a lake to just above its water level.
fn level_lake(hs: &mut [f32], g: &Grid, outline: &[[f32; 2]], level: f32) {
    let n = g.n();
    let reach = 90.0;
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for q in outline {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    let (x0, z0, x1, z1) = rect(g, lo, hi, reach);
    for j in z0..=z1 {
        for i in x0..=x1 {
            let w = g.world(i, j);
            let inside = point_in(outline, w);
            let d = if inside {
                0.0
            } else {
                dist_to_loop(outline, w)
            };
            if d >= reach {
                continue;
            }
            let k = j * n + i;
            let target = level + 1.2 + d * 0.04;
            let t = smoothstep(0.0, reach, d);
            hs[k] = target + (hs[k] - target) * t;
        }
    }
}

fn point_in(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            c = !c;
        }
    }
    c
}

fn dist_to_loop(poly: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let n = poly.len();
    let mut best = f32::MAX;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
        let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / (dx * dx + dz * dz).max(1e-6))
            .clamp(0.0, 1.0);
        best = best.min(((p[0] - a[0] - dx * t).powi(2) + (p[1] - a[1] - dz * t).powi(2)).sqrt());
    }
    best
}
