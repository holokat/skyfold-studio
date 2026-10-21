//! Terrain brush operations. All brushes take a world-space centre (x, z), a radius and a
//! falloff, modify the CPU data, and mark the touched region dirty (GPU upload + mesh rebuild).

use bevy::prelude::*;

use super::layers::LAYER_COUNT;
use super::worldgen::land_profile;
use super::{GridRect, Terrain};
use crate::history::History;
use crate::noise::{fbm, ridged, smoothstep, value2};

/// Brush weight for a normalised distance `d` (0 centre .. 1 edge).
#[inline]
pub fn falloff_weight(d: f32, falloff: f32) -> f32 {
    if d >= 1.0 {
        return 0.0;
    }
    let inner = 1.0 - falloff.clamp(0.0, 1.0);
    if d <= inner {
        1.0
    } else {
        1.0 - smoothstep(inner, 1.0, d)
    }
}

/// Iterate vertices inside the brush circle: f(terrain, idx, world_xz, weight).
pub(crate) fn for_each_in_circle(
    t: &mut Terrain,
    history: &mut History,
    center: Vec2,
    radius: f32,
    falloff: f32,
    mut f: impl FnMut(&mut Terrain, usize, Vec2, f32),
) -> Option<GridRect> {
    let rect = t.rect_for_circle(center, radius)?;
    history.touch_terrain(t, rect);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let p = t.grid_to_world(i, j);
            let d = p.distance(center) / radius;
            let w = falloff_weight(d, falloff);
            if w > 0.0 {
                let k = t.idx(i, j);
                f(t, k, p, w);
            }
        }
    }
    Some(rect)
}

pub struct BrushArgs {
    pub center: Vec2,
    pub radius: f32,
    pub falloff: f32,
    pub strength: f32,
    pub dt: f32,
}

/// Raise (or lower with negative `dir`) at a rate proportional to brush size.
pub fn raise(t: &mut Terrain, h: &mut History, a: &BrushArgs, dir: f32) {
    let rate = dir * a.strength * (a.radius * 0.35 + 6.0) * a.dt;
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, k, _, w| {
        t.heights[k] += rate * w;
    }) {
        t.mark_heights_dirty(r);
    }
}

pub fn smooth(t: &mut Terrain, h: &mut History, a: &BrushArgs) {
    let Some(rect) = t.rect_for_circle(a.center, a.radius) else {
        return;
    };
    h.touch_terrain(t, rect);
    let n = t.n();
    let src = t.heights.clone();
    // radius of the blur kernel grows a little with brush size
    let kr = ((a.radius / t.cell) * 0.08).clamp(1.0, 4.0) as i32;
    let k = (a.strength * a.dt * 12.0).min(1.0);
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let p = t.grid_to_world(i, j);
            let w = falloff_weight(p.distance(a.center) / a.radius, a.falloff);
            if w <= 0.0 {
                continue;
            }
            let mut sum = 0.0;
            let mut cnt = 0.0;
            for dz in -kr..=kr {
                for dx in -kr..=kr {
                    let x = (i as i32 + dx).clamp(0, n as i32 - 1) as usize;
                    let z = (j as i32 + dz).clamp(0, n as i32 - 1) as usize;
                    sum += src[z * n + x];
                    cnt += 1.0;
                }
            }
            let idx = t.idx(i, j);
            t.heights[idx] += (sum / cnt - src[idx]) * w * k;
        }
    }
    t.mark_heights_dirty(rect);
}

pub fn flatten(t: &mut Terrain, h: &mut History, a: &BrushArgs, target: f32) {
    let k = (a.strength * a.dt * 6.0).min(1.0);
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, idx, _, w| {
        t.heights[idx] += (target - t.heights[idx]) * w * k;
    }) {
        t.mark_heights_dirty(r);
    }
}

/// Grow ridged mountains. The noise is anchored in world space so repeated dabs build
/// coherent ranges. `dir` < 0 carves canyons.
pub fn mountain(t: &mut Terrain, h: &mut History, a: &BrushArgs, dir: f32, seed: u32) {
    let freq = 1.6 / a.radius.max(10.0);
    let rate = a.strength * (a.radius * 0.6 + 10.0) * a.dt * dir;
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, k, p, w| {
        let n = ridged(p.x * freq, p.y * freq, 6, seed);
        t.heights[k] += rate * w * (n.powf(1.6) * 1.4 + 0.08);
    }) {
        t.mark_heights_dirty(r);
    }
}

pub fn hills(t: &mut Terrain, h: &mut History, a: &BrushArgs, dir: f32, seed: u32) {
    let freq = 3.0 / a.radius.max(10.0);
    let rate = a.strength * (a.radius * 0.25 + 5.0) * a.dt * dir;
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, k, p, w| {
        let n = fbm(p.x * freq, p.y * freq, 4, 2.0, 0.5, seed) * 0.5 + 0.5;
        t.heights[k] += rate * w * n;
    }) {
        t.mark_heights_dirty(r);
    }
}

pub fn terrace(t: &mut Terrain, h: &mut History, a: &BrushArgs, step: f32) {
    let k = (a.strength * a.dt * 4.0).min(1.0);
    let step = step.max(1.0);
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, idx, p, w| {
        let hh = t.heights[idx];
        // jitter the step heights a little so terraces look natural
        let jitter = (value2(p.x * 0.01, p.y * 0.01, 77) - 0.5) * step * 0.35;
        let x = (hh + jitter) / step;
        let f = x.fract();
        let target = (x.floor() + smoothstep(0.55, 0.85, f)) * step - jitter;
        t.heights[idx] += (target - hh) * w * k;
    }) {
        t.mark_heights_dirty(r);
    }
}

/// Erosion brush: rain (or ice, or wind, per the preset) works the ground under the brush;
/// hold it to deepen gullies and build fans (see `erosion`).
pub fn erode(t: &mut Terrain, h: &mut History, a: &BrushArgs, p: &super::erosion::ErosionParams) {
    let Some(rect) = t.rect_for_circle(a.center, a.radius * 1.25 + t.cell * 4.0) else {
        return;
    };
    h.touch_terrain(t, rect);
    let g = t.world_to_grid(a.center.x, a.center.y);
    // rounds of rain this frame is worth (a second of scrubbing ≈ 20 rounds at half strength)
    let amount = (a.strength * a.dt * 40.0).min(3.0);
    let (n, cell, sea) = (t.n(), t.cell, t.sea_level);
    if let Some(r) = super::erosion::erode_brush(
        &mut t.heights,
        n,
        cell,
        sea,
        (g.x, g.y),
        a.radius / cell,
        a.falloff,
        amount,
        p,
    ) {
        t.mark_heights_dirty(r);
    }
}

/// Paint land up out of the sea (or sink it with `dir` < 0). Coastlines get organic,
/// noise-driven edges and the land rises gently inland.
pub fn paint_land(
    t: &mut Terrain,
    h: &mut History,
    a: &BrushArgs,
    dir: f32,
    land_height: f32,
    roughness: f32,
    seed: u32,
) {
    let sea = t.sea_level;
    let k = (a.strength * a.dt * 3.0).min(1.0);
    let coast_freq = 3.5 / a.radius.max(20.0);
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius * 1.05, 0.0, |t, idx, p, _| {
        let d = p.distance(a.center) / (a.radius * 1.05);
        // noisy brush edge → ragged coastline
        let edge_noise = fbm(p.x * coast_freq, p.y * coast_freq, 4, 2.0, 0.55, seed) * roughness;
        let w = falloff_weight((d + edge_noise * 0.5).max(0.0), a.falloff.max(0.25));
        if w <= 0.0 {
            return;
        }
        let hills = fbm(p.x * 0.006, p.y * 0.006, 4, 2.0, 0.5, seed + 3) * 0.5 + 0.5;
        let cur = t.heights[idx];
        if dir > 0.0 {
            let target = sea + land_height * (0.6 + 0.8 * hills) * w.powf(0.6) + land_profile(0.02);
            if cur < target {
                t.heights[idx] += (target - cur) * k * w;
            }
        } else {
            let target = sea - 6.0 - 18.0 * w - hills * 6.0;
            if cur > target {
                t.heights[idx] += (target - cur) * k * w;
            }
        }
    }) {
        t.mark_heights_dirty(r);
    }
}

fn point_in_polygon(p: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if ((a.y > p.y) != (b.y > p.y)) && (p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist_to_polyline(p: Vec2, poly: &[Vec2]) -> f32 {
    let mut best = f32::MAX;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
        best = best.min(p.distance(a + ab * t));
    }
    best
}

/// Fill a hand-drawn coastline outline with land (or carve it into sea when `sink`).
pub fn fill_outline(
    t: &mut Terrain,
    h: &mut History,
    outline: &[Vec2],
    sink: bool,
    land_height: f32,
    roughness: f32,
    seed: u32,
) {
    if outline.len() < 3 {
        return;
    }
    // simplify the outline for speed
    let step = (outline.len() / 160).max(1);
    let poly: Vec<Vec2> = outline.iter().step_by(step).copied().collect();
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in &poly {
        min = min.min(*p);
        max = max.max(*p);
    }
    let extent = (max - min).max_element();
    let shore = (extent * 0.12).clamp(40.0, 400.0);
    let pad = shore * 0.6;
    let Some(rect) = t.rect_for_circle((min + max) * 0.5, (max - min).length() * 0.5 + pad) else {
        return;
    };
    h.touch_terrain(t, rect);
    let sea = t.sea_level;
    let coast_freq = 6.0 / extent.max(50.0);
    let rows: Vec<(usize, Vec<(usize, f32)>)> = {
        let tt = &*t;
        let poly = &poly;
        std::thread::scope(|s| {
            let handles: Vec<_> = (rect.z0..=rect.z1)
                .collect::<Vec<_>>()
                .chunks(((rect.height()) / 8).max(1))
                .map(|js| {
                    let js = js.to_vec();
                    s.spawn(move || {
                        js.into_iter()
                            .map(|j| {
                                let mut row = vec![];
                                for i in rect.x0..=rect.x1 {
                                    let p = tt.grid_to_world(i, j);
                                    if p.x < min.x - pad
                                        || p.x > max.x + pad
                                        || p.y < min.y - pad
                                        || p.y > max.y + pad
                                    {
                                        continue;
                                    }
                                    let inside = point_in_polygon(p, poly);
                                    let d = dist_to_polyline(p, poly);
                                    let noise =
                                        fbm(p.x * coast_freq, p.y * coast_freq, 5, 2.0, 0.55, seed)
                                            * roughness
                                            * shore
                                            * 0.9;
                                    let sd = if inside { d } else { -d } + noise;
                                    row.push((i, sd));
                                }
                                (j, row)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        })
    };
    for (j, row) in rows {
        for (i, sd) in row {
            let idx = t.idx(i, j);
            let cur = t.heights[idx];
            let p = t.grid_to_world(i, j);
            let hills = fbm(p.x * 0.005, p.y * 0.005, 4, 2.0, 0.5, seed + 3) * 0.5 + 0.5;
            if !sink {
                let landness = (sd / shore).clamp(-0.6, 1.0);
                if landness > -0.6 {
                    let target = sea
                        + land_profile(landness * 0.25)
                        + land_height * smoothstep(0.0, 1.0, landness) * (0.5 + hills);
                    if target > cur {
                        t.heights[idx] = target;
                    }
                }
            } else {
                let depth = (sd / shore).clamp(-0.6, 1.0);
                if depth > -0.6 {
                    let target = sea - 2.0 - 30.0 * smoothstep(-0.6, 1.0, depth);
                    if target < cur {
                        t.heights[idx] = target;
                    }
                }
            }
        }
    }
    t.mark_heights_dirty(rect);
}

/// Paint a material layer (`erase` blends back towards automatic styling).
pub fn paint_layer(t: &mut Terrain, h: &mut History, a: &BrushArgs, layer: usize, erase: bool) {
    let k = (a.strength * a.dt * 5.0).min(1.0);
    if let Some(r) = for_each_in_circle(t, h, a.center, a.radius, a.falloff, |t, idx, _, w| {
        let kk = k * w;
        let s = &mut t.splat[idx];
        for (l, v) in s.iter_mut().enumerate().take(LAYER_COUNT) {
            let target = if !erase && l == layer { 255.0 } else { 0.0 };
            let cur = *v as f32;
            let mut nv = cur + (target - cur) * kk;
            // make sure slow strokes still progress despite 8-bit quantisation
            if (nv.round() - cur).abs() < 1.0 && (target - cur).abs() >= 1.0 && kk > 0.002 {
                nv = cur + (target - cur).signum();
            }
            *v = nv.round().clamp(0.0, 255.0) as u8;
        }
        // keep the total ≤ 255
        let sum: u32 = s.iter().map(|v| *v as u32).sum();
        if sum > 255 {
            let scale = 255.0 / sum as f32;
            for v in s.iter_mut() {
                *v = (*v as f32 * scale).floor() as u8;
            }
        }
    }) {
        t.mark_splat_dirty(r);
    }
}

/// Paint zone ownership (hard-edged). `id` 0 erases.
pub fn paint_zone(
    t: &mut Terrain,
    h: &mut History,
    center: Vec2,
    radius: f32,
    id: u8,
    only_erase: Option<u8>,
) {
    if let Some(r) = for_each_in_circle(t, h, center, radius, 0.0, |t, idx, _, w| {
        if w > 0.5 {
            match only_erase {
                Some(z) if t.zones[idx] != z => {}
                _ => t.zones[idx] = id,
            }
        }
    }) {
        t.mark_zone_dirty(r);
    }
}
