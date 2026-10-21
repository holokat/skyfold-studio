//! The shore map: for every spot of the terrain (at up to 1025² texels), the surface level of the
//! nearest water (sea, lake, river or lava), how far away its edge is and what kind of water it
//! is. The terrain shader reads it (`shore_tex`) for wet sand, swash and surf lines on sea beaches,
//! damp lapped shores on lakes, wet soil and gravel on river banks, and scorched, glowing ground at
//! the rim of lava. Rebuilt whenever heights, the sea level or any water body change (debounced
//! while editing).

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::terrain::Terrain;
use crate::terrain::material::{TerrainGpu, TerrainMaterial};
use crate::water::{WaterBehaviour, WaterBodies};

/// Kinds written to the map's blue channel (mirrored in terrain.wesl).
const SEA: f32 = 1.0;
const LAKE: f32 = 2.0;
const RIVER: f32 = 3.0;
const LAVA: f32 = 4.0;
/// Shores further away than this (m) are not tracked.
const REACH: f32 = 160.0;
/// Largest map side (texels): 8 m texels on an 8 km world.
const MAX_SIDE: usize = 1025;

#[derive(Resource, Default)]
pub struct ShoreMap {
    /// (terrain generation, height revision, sea level bits) the map was built for
    built: Option<(u64, u64, u32)>,
    water_stale: bool,
    /// seconds since the last change that is still waiting to be built
    wait: f32,
}

/// Texels per side and grid vertices per texel for a terrain.
fn layout(t: &Terrain) -> (usize, usize) {
    let n = t.n();
    let step = (n - 1).div_ceil(MAX_SIDE - 1).max(1);
    ((n - 1) / step + 1, step)
}

/// Builds the map: (level, distance to the water's edge in metres, kind, 1) per texel.
pub fn build(t: &Terrain, water: &WaterBodies) -> (usize, Vec<f32>) {
    let (m, step) = layout(t);
    let n = t.n();
    let texel = t.cell * step as f32;
    let has_levels = water.level_res == t.res && water.level_map.len() == n * n;
    // rivers (and lava rivers) by their courses: texels within their half width
    let mut river_kind = vec![0.0f32; m * m];
    for r in &water.rivers {
        let kind = if r.style.behaviour == WaterBehaviour::Molten {
            LAVA
        } else {
            RIVER
        };
        let reach = r.width * 0.5 + texel * 1.5;
        for w in r.points.windows(2) {
            let (a, b) = (Vec2::new(w[0][0], w[0][2]), Vec2::new(w[1][0], w[1][2]));
            let (lo, hi) = (a.min(b) - Vec2::splat(reach), a.max(b) + Vec2::splat(reach));
            let to_i = |v: f32| ((v + t.half()) / texel).clamp(0.0, (m - 1) as f32);
            for j in to_i(lo.y).floor() as usize..=to_i(hi.y).ceil() as usize {
                for i in to_i(lo.x).floor() as usize..=to_i(hi.x).ceil() as usize {
                    let p = Vec2::new(i as f32 * texel - t.half(), j as f32 * texel - t.half());
                    let ab = b - a;
                    let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    if p.distance(a + ab * u) <= reach {
                        river_kind[j * m + i] = kind;
                    }
                }
            }
        }
    }
    // the water's own texels: level and kind
    let mut level = vec![f32::NAN; m * m];
    let mut kind = vec![0.0f32; m * m];
    for j in 0..m {
        for i in 0..m {
            let (gi, gj) = ((i * step).min(n - 1), (j * step).min(n - 1));
            let h = t.h(gi, gj);
            let k = j * m + i;
            let lv = if has_levels {
                water.level_map[gj * n + gi]
            } else {
                f32::NEG_INFINITY
            };
            if lv.is_finite() && lv > h {
                level[k] = lv;
                kind[k] = if river_kind[k] > 0.0 {
                    river_kind[k]
                } else {
                    match water.lakes.iter().find(|l| (l.level - lv).abs() < 0.05) {
                        Some(l) if l.style.behaviour == WaterBehaviour::Molten => LAVA,
                        _ => LAKE,
                    }
                };
            } else if h < t.sea_level {
                level[k] = t.sea_level;
                kind[k] = SEA;
            }
        }
    }
    // nearest water for every dry texel: two-pass dead-reckoning distance transform (each texel
    // keeps the coordinates of its nearest wet texel)
    let mut src: Vec<u32> = (0..m * m)
        .map(|k| {
            if level[k].is_nan() {
                u32::MAX
            } else {
                k as u32
            }
        })
        .collect();
    let dist2 = |k: usize, s: u32| -> f32 {
        if s == u32::MAX {
            return f32::MAX;
        }
        let (dx, dz) = (
            (k % m) as f32 - (s as usize % m) as f32,
            (k / m) as f32 - (s as usize / m) as f32,
        );
        dx * dx + dz * dz
    };
    let mut best: Vec<f32> = (0..m * m).map(|k| dist2(k, src[k])).collect();
    let relax = |k: usize, nb: usize, src: &mut Vec<u32>, best: &mut Vec<f32>| {
        let s = src[nb];
        let d = dist2(k, s);
        if d < best[k] {
            best[k] = d;
            src[k] = s;
        }
    };
    for j in 0..m {
        for i in 0..m {
            let k = j * m + i;
            if i > 0 {
                relax(k, k - 1, &mut src, &mut best);
            }
            if j > 0 {
                relax(k, k - m, &mut src, &mut best);
                if i > 0 {
                    relax(k, k - m - 1, &mut src, &mut best);
                }
                if i + 1 < m {
                    relax(k, k - m + 1, &mut src, &mut best);
                }
            }
        }
    }
    for j in (0..m).rev() {
        for i in (0..m).rev() {
            let k = j * m + i;
            if i + 1 < m {
                relax(k, k + 1, &mut src, &mut best);
            }
            if j + 1 < m {
                relax(k, k + m, &mut src, &mut best);
                if i + 1 < m {
                    relax(k, k + m + 1, &mut src, &mut best);
                }
                if i > 0 {
                    relax(k, k + m - 1, &mut src, &mut best);
                }
            }
        }
    }
    let mut out = vec![0.0f32; m * m * 4];
    for k in 0..m * m {
        let d = best[k].sqrt() * texel;
        let o = &mut out[k * 4..k * 4 + 4];
        if src[k] == u32::MAX || d > REACH {
            // far from any water: no shore effects (the level is never read)
            o.copy_from_slice(&[t.sea_level, REACH, 0.0, 1.0]);
        } else {
            let s = src[k] as usize;
            o.copy_from_slice(&[level[s], d, kind[s], 1.0]);
        }
    }
    (m, out)
}

fn image(m: usize, data: Vec<f32>) -> Image {
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    Image::new(
        Extent3d {
            width: m as u32,
            height: m as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bytes,
        TextureFormat::Rgba32Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
}

/// Rebuilds the map when the land or the water changed (once a brush stroke ends, after a short
/// pause) and hands it to the terrain material.
#[allow(clippy::too_many_arguments)]
pub fn update_shore_map(
    mut map: ResMut<ShoreMap>,
    terrain: Res<Terrain>,
    water: Res<WaterBodies>,
    gpu: Option<Res<TerrainGpu>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<TerrainMaterial>>,
    time: Res<Time>,
    stroke: Res<crate::tools::Stroke>,
) {
    let Some(gpu) = gpu else { return };
    let key = (
        terrain.generation,
        terrain.height_revision,
        terrain.sea_level.to_bits(),
    );
    if water.is_changed() {
        map.water_stale = true;
    }
    let stale = map.built != Some(key) || map.water_stale;
    if !stale {
        return;
    }
    // (a new world builds at once; edits wait until they pause briefly)
    let fresh = map.built.is_none_or(|b| b.0 != key.0);
    map.wait += time.delta_secs();
    if !fresh && (map.wait < 0.4 || stroke.active) {
        return;
    }
    // the water's level map follows its own rebuild: wait for it
    if water.dirty {
        return;
    }
    map.wait = 0.0;
    map.built = Some(key);
    map.water_stale = false;
    let t0 = std::time::Instant::now();
    let (m, data) = build(&terrain, &water);
    let handle = images.add(image(m, data));
    if let Some(mut mat) = mats.get_mut(&gpu.material) {
        mat.extension.shore = handle;
    }
    debug!("shore map {m}² in {:?}", t0.elapsed());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A coast running north-south: the sea's texels are sea, and the land knows how far the water
    /// is (within a texel) and how high it stands.
    #[test]
    fn coast_distance() {
        let mut t = Terrain::new(256, 4.0, 0.0);
        let n = t.n();
        for j in 0..n {
            for i in 0..n {
                let k = t.idx(i, j);
                t.heights[k] = if i < 100 { -5.0 } else { 3.0 };
            }
        }
        let (m, data) = build(&t, &WaterBodies::default());
        let at = |i: usize, j: usize| &data[(j * m + i) * 4..(j * m + i) * 4 + 4];
        assert_eq!(at(50, 50)[2], SEA);
        assert_eq!(at(50, 50)[1], 0.0);
        // 20 cells (80 m) inland of the first dry column
        let d = at(120, 60);
        assert_eq!(d[2], SEA);
        assert!((d[1] - 84.0).abs() <= 4.5, "distance {}", d[1]);
        assert_eq!(d[0], 0.0);
    }
}
