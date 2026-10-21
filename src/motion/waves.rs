//! Where the water surface is for a body in it: lakes and rivers at their level, the sea at its
//! level plus the tide and the FFT waves (read from the same displacement cascades the water
//! shader draws, so a swimmer rises and falls with the swell they see).

use bevy::prelude::*;
use half::f16;

use crate::ocean::{CASCADE_L, N, OceanWaves};
use crate::terrain::Terrain;
use crate::water::{WaterBodies, WaterStyle};

/// The shader's per-cascade shallow-water attenuation (water.wesl `cascade_atten`).
fn cascade_atten(depth: f32) -> [f32; 3] {
    let ss = crate::noise::smoothstep;
    let d0 = [40.0, 12.1, 2.2];
    let floor = [0.0, 0.05, 0.25];
    std::array::from_fn(|c| {
        let a = ss(0.0, d0[c], depth);
        let f = floor[c] * ss(0.0, 0.6, depth);
        f + (1.0 - f) * a
    })
}

/// Bilinear sample of one cascade's height (mip 0, wrapping) at world uv.
fn sample_height(data: &[u8], uv: Vec2) -> f32 {
    if data.len() < N * N * 8 {
        return 0.0;
    }
    let x = uv.x * N as f32 - 0.5;
    let y = uv.y * N as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |i: i32, j: i32| {
        let i = i.rem_euclid(N as i32) as usize;
        let j = j.rem_euclid(N as i32) as usize;
        // texel = 4 × f16 (choppy x, height, choppy z, Jacobian)
        let o = (j * N + i) * 8 + 2;
        f16::from_bits(u16::from_le_bytes([data[o], data[o + 1]])).to_f32()
    };
    let (i, j) = (x0 as i32, y0 as i32);
    let a = at(i, j) + (at(i + 1, j) - at(i, j)) * fx;
    let b = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * fx;
    a + (b - a) * fy
}

/// Height of the sea surface at `xz` (sea level + tide + waves), as the water shader lifts it:
/// the tide on the water materials' clock (`Time::elapsed_secs`), the current's drift on the
/// shader's (`globals.time`, wrapped), the waves straight from the simulation's latest frame.
pub fn sea_surface(
    terrain: &Terrain,
    style: &WaterStyle,
    waves: &OceanWaves,
    xz: Vec2,
    time: &Time,
) -> f32 {
    let tide = crate::water::tide_offset(style, time.elapsed_secs());
    let sea = terrain.sea_level + tide;
    let Some(disp) = &waves.disp else { return sea };
    let att = cascade_atten(sea - terrain.height_at(xz.x, xz.y));
    let a = style.flow_dir.to_radians();
    let q = xz - Vec2::new(a.cos(), a.sin()) * (style.flow * 1.6 * time.elapsed_secs_wrapped());
    let mut h = 0.0;
    for c in 0..3 {
        h += sample_height(&disp[c], q / CASCADE_L[c]) * att[c];
    }
    sea + h
}

/// The water surface above (or around) a point: the highest of the sea and any lake or river
/// covering it; `None` where no water could reach (ground well above every level).
pub fn surface_at(
    terrain: &Terrain,
    water: &WaterBodies,
    style: &WaterStyle,
    waves: &OceanWaves,
    xz: Vec2,
    time: &Time,
) -> Option<f32> {
    let ground = terrain.height_at(xz.x, xz.y);
    let lake = water.water_level(Vec3::new(xz.x, ground, xz.y));
    let sea =
        (ground < terrain.sea_level + 3.0).then(|| sea_surface(terrain, style, waves, xz, time));
    match (lake, sea) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}
