//! Procedural world generation presets and hydraulic erosion.

use crate::noise::{Rng, fbm, perlin2, smoothstep};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldPreset {
    /// Open ocean: a blank canvas for drawing continents.
    Ocean,
    Island,
    Continent,
    Archipelago,
    /// Gentle plains everywhere above sea level.
    Plains,
}

impl WorldPreset {
    pub const ALL: [WorldPreset; 5] = [
        WorldPreset::Ocean,
        WorldPreset::Island,
        WorldPreset::Continent,
        WorldPreset::Archipelago,
        WorldPreset::Plains,
    ];
    pub fn name(self) -> &'static str {
        match self {
            WorldPreset::Ocean => "Empty Ocean",
            WorldPreset::Island => "Island",
            WorldPreset::Continent => "Continent",
            WorldPreset::Archipelago => "Archipelago",
            WorldPreset::Plains => "Flat Plains",
        }
    }
}

/// Height profile shared by generation and the continent brush: maps a "landness" value
/// (negative = sea, positive = land) to meters relative to sea level.
pub fn land_profile(land: f32) -> f32 {
    if land < 0.0 {
        // continental shelf then deeper ocean
        let shelf = (land * 40.0).max(-8.0);
        let deep = (land + 0.12).min(0.0) * 140.0;
        shelf + deep
    } else {
        land * 42.0 + land * land * 70.0
    }
}

/// Generates heights (meters, absolute) for a world of `res` cells of size `cell`.
pub fn generate(preset: WorldPreset, res: usize, cell: f32, sea: f32, seed: u32) -> Vec<f32> {
    let n = res + 1;
    let mut heights = vec![0.0f32; n * n];
    let size = res as f32 * cell;
    // feature scale is relative to a 4 km world so presets look similar at any size
    let scale = size / 4096.0;
    heights.chunks_mut(n).enumerate().for_each(|(j, row)| {
        for (i, out) in row.iter_mut().enumerate() {
            let u = i as f32 / res as f32 * 2.0 - 1.0;
            let v = j as f32 / res as f32 * 2.0 - 1.0;
            *out = sea + sample_preset(preset, u, v, scale, seed);
        }
    });
    heights
}

fn sample_preset(preset: WorldPreset, u: f32, v: f32, scale: f32, seed: u32) -> f32 {
    // domain warp for organic coastlines
    let wx = fbm(u * 1.3 + 3.1, v * 1.3 + 7.7, 4, 2.0, 0.5, seed + 1);
    let wy = fbm(u * 1.3 - 5.2, v * 1.3 + 1.3, 4, 2.0, 0.5, seed + 2);
    let qx = u + wx * 0.28;
    let qy = v + wy * 0.28;
    let d = (qx * qx + qy * qy).sqrt();
    let f = 1.0 + 0.6 * (scale - 1.0).max(0.0); // more detail on bigger maps
    let base = fbm(qx * 2.2 * f, qy * 2.2 * f, 6, 2.0, 0.5, seed + 3);

    let land = match preset {
        WorldPreset::Ocean => -0.25 + base * 0.05,
        WorldPreset::Island => base * 0.55 + (1.0 - smoothstep(0.15, 0.9, d)) - 0.45,
        WorldPreset::Continent => {
            let edge = 1.0 - smoothstep(0.55, 1.02, qx.abs().max(qy.abs()) * 0.6 + d * 0.45);
            base * 0.6 + edge * 0.9 - 0.3
        }
        WorldPreset::Archipelago => {
            let isles = fbm(qx * 4.5 * f, qy * 4.5 * f, 5, 2.0, 0.5, seed + 9);
            isles * 0.9 + base * 0.2 + (1.0 - smoothstep(0.3, 1.0, d)) * 0.25 - 0.18
        }
        WorldPreset::Plains => 0.12 + base * 0.08,
    };

    let mut h = land_profile(land);
    if land > 0.0 && preset != WorldPreset::Ocean {
        // rolling hills
        let hills = fbm(qx * 7.0 * f + 11.0, qy * 7.0 * f, 5, 2.0, 0.5, seed + 5);
        h += hills * 18.0 * smoothstep(0.0, 0.15, land);
        if preset != WorldPreset::Plains {
            // rolling mountain ranges away from the coast: rounded ridges blended with smooth
            // massifs, so peaks read as soft shoulders instead of knife edges
            let ridge = soft_ridged(qx * 2.4 * f + 10.0, qy * 2.4 * f - 4.0, 5, seed + 4);
            let massif = fbm(
                qx * 1.9 * f - 3.0,
                qy * 1.9 * f + 8.0,
                4,
                2.0,
                0.45,
                seed + 7,
            ) * 0.5
                + 0.5;
            let range_mask = smoothstep(0.08, 0.5, land)
                * smoothstep(
                    -0.15,
                    0.35,
                    fbm(qx * 1.2, qy * 1.2, 3, 2.0, 0.5, seed + 6) + 0.1,
                );
            let m = ridge * 0.55 + massif * massif * 0.45;
            h += m * m * 470.0 * range_mask;
        }
    }
    h
}

/// Ridged noise with rounded crests: a soft absolute value (`sqrt(n² + k²)`) keeps ridgelines
/// from forming knife edges, and fewer octaves keep the slopes rolling.
fn soft_ridged(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm, mut weight) = (0.0, 0.5, 1.0, 0.0, 1.0f32);
    for o in 0..octaves {
        let (s, c) = (0.7f32 * o as f32).sin_cos();
        let (rx, ry) = (x * c - y * s, x * s + y * c);
        let n = perlin2(rx * freq, ry * freq, seed.wrapping_add(o * 7919));
        let r = (1.0 - (n * n + 0.03).sqrt()).max(0.0);
        let r = r * r * weight;
        weight = (r * 1.5).clamp(0.0, 1.0);
        sum += r * amp;
        norm += amp;
        amp *= 0.48;
        freq *= 2.0;
    }
    sum / norm / 0.8
}

/// A finished world: generated, gently eroded (gullies without raw droplet scars) and softened.
pub fn build_world(preset: WorldPreset, res: usize, cell: f32, sea: f32, seed: u32) -> Vec<f32> {
    let mut h = generate(preset, res, cell, sea, seed);
    if preset != WorldPreset::Ocean && preset != WorldPreset::Plains {
        erode(&mut h, res, cell, res * res / 14, seed as u64, None, 0.75);
    }
    for _ in 0..2 {
        smooth_all(&mut h, res, 0.5);
    }
    h
}

/// Droplet-based hydraulic erosion (after Hans Theobald Beyer / Sebastian Lague), operating
/// on grid units. If `region` is set (center grid x, z, radius in cells) droplets only start inside it.
pub fn erode(
    heights: &mut [f32],
    res: usize,
    cell: f32,
    droplets: usize,
    seed: u64,
    region: Option<(f32, f32, f32)>,
    strength: f32,
) {
    let n = res + 1;
    let radius: i32 = 3;
    // precompute erosion brush
    let mut brush: Vec<(i32, i32, f32)> = Vec::new();
    let mut wsum = 0.0;
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let d = ((dx * dx + dz * dz) as f32).sqrt();
            if d <= radius as f32 {
                let w = 1.0 - d / radius as f32;
                brush.push((dx, dz, w));
                wsum += w;
            }
        }
    }
    for b in &mut brush {
        b.2 /= wsum;
    }

    let inertia = 0.05;
    let capacity_factor = 4.0;
    let min_capacity = 0.01;
    let erode_speed = 0.3 * strength;
    let deposit_speed = 0.3;
    let evaporate = 0.015;
    let gravity = 4.0;
    let max_steps = 64;
    // heights are meters; gradients are taken per cell, normalised by cell size so the
    // algorithm behaves similarly regardless of resolution.
    let inv_cell = 1.0 / cell;

    let mut rng = Rng::new(seed);
    let maxf = (res - 1) as f32;

    let height_grad = |h: &[f32], x: f32, z: f32| -> (f32, f32, f32) {
        let ix = x as usize;
        let iz = z as usize;
        let fx = x - ix as f32;
        let fz = z - iz as f32;
        let i = iz * n + ix;
        let h00 = h[i];
        let h10 = h[i + 1];
        let h01 = h[i + n];
        let h11 = h[i + n + 1];
        let gx = ((h10 - h00) * (1.0 - fz) + (h11 - h01) * fz) * inv_cell;
        let gz = ((h01 - h00) * (1.0 - fx) + (h11 - h10) * fx) * inv_cell;
        let height = h00 * (1.0 - fx) * (1.0 - fz)
            + h10 * fx * (1.0 - fz)
            + h01 * (1.0 - fx) * fz
            + h11 * fx * fz;
        (height, gx, gz)
    };

    for _ in 0..droplets {
        let (mut x, mut z) = match region {
            Some((cx, cz, r)) => {
                let a = rng.f32() * std::f32::consts::TAU;
                let rr = r * rng.f32().sqrt();
                (cx + a.cos() * rr, cz + a.sin() * rr)
            }
            None => (rng.f32() * maxf, rng.f32() * maxf),
        };
        if x < 1.0 || z < 1.0 || x >= maxf || z >= maxf {
            continue;
        }
        let (mut dx, mut dz) = (0.0f32, 0.0f32);
        let mut speed = 1.0f32;
        let mut water = 1.0f32;
        let mut sediment = 0.0f32;

        for _ in 0..max_steps {
            let ix = x as usize;
            let iz = z as usize;
            let fx = x - ix as f32;
            let fz = z - iz as f32;
            let (h, gx, gz) = height_grad(heights, x, z);
            dx = dx * inertia - gx * (1.0 - inertia);
            dz = dz * inertia - gz * (1.0 - inertia);
            let len = (dx * dx + dz * dz).sqrt();
            if len < 1e-6 {
                break;
            }
            dx /= len;
            dz /= len;
            let nx = x + dx;
            let nz = z + dz;
            if nx < 1.0 || nz < 1.0 || nx >= maxf || nz >= maxf {
                break;
            }
            let (nh, _, _) = height_grad(heights, nx, nz);
            let dh = nh - h;
            let capacity =
                (-dh * inv_cell).max(min_capacity) * speed * water * capacity_factor * cell;
            if sediment > capacity || dh > 0.0 {
                // deposit
                let amount = if dh > 0.0 {
                    dh.min(sediment)
                } else {
                    (sediment - capacity) * deposit_speed
                };
                sediment -= amount;
                let i = iz * n + ix;
                heights[i] += amount * (1.0 - fx) * (1.0 - fz);
                heights[i + 1] += amount * fx * (1.0 - fz);
                heights[i + n] += amount * (1.0 - fx) * fz;
                heights[i + n + 1] += amount * fx * fz;
            } else {
                // erode with a brush
                let amount = ((capacity - sediment) * erode_speed).min(-dh);
                for &(bx, bz, w) in &brush {
                    let px = ix as i32 + bx;
                    let pz = iz as i32 + bz;
                    if px < 0 || pz < 0 || px >= n as i32 || pz >= n as i32 {
                        continue;
                    }
                    let k = pz as usize * n + px as usize;
                    let take = amount * w;
                    heights[k] -= take;
                    sediment += take;
                }
            }
            speed = (speed * speed + dh.abs() * gravity * inv_cell)
                .max(0.0)
                .sqrt();
            water *= 1.0 - evaporate;
            x = nx;
            z = nz;
        }
    }
}

/// Simple full-map smoothing pass (box blur blended by `amount`).
pub fn smooth_all(heights: &mut [f32], res: usize, amount: f32) {
    let n = res + 1;
    let src = heights.to_vec();
    for j in 1..res {
        for i in 1..res {
            let k = j * n + i;
            let avg = (src[k - 1] + src[k + 1] + src[k - n] + src[k + n] + src[k] * 4.0) / 8.0;
            heights[k] = src[k] + (avg - src[k]) * amount;
        }
    }
}
