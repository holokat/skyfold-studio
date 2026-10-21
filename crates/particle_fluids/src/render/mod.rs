//! CPU equivalents of the reference's GPU render passes: grids the host
//! uploads as 3D textures and ray-marches in its own shaders.
//!
//! Both fields are sampled with linear filtering and clamp-to-edge wrapping at
//! `uvw = (p − min) / extent` (voxel centres at `(i + ½) / n`). The shading
//! below is the reference's fragment shaders, for porting to WGSL.
//!
//! # Smoke ([`smoke_volume::SmokeVolume`], `GasVolumeRenderer.ts`)
//!
//! A box over the volume drawn with back faces, depth test and depth write
//! off, normal alpha blending. Per pixel, with `dir` the view ray:
//!
//! ```text
//! inv = 1 / (dir + 1e-6);  near = (min − cam)·inv;  far = (max − cam)·inv
//! t0 = max(max3(min(near, far)), 0);  t1 = min(min3(max(near, far)), sceneDistance)
//! step = max(t1 − t0, 0) / STEPS                 // STEPS = 56 (80 in Simulation)
//! t = t0 + step · fract(sin(dot(fragCoord.xy, (12.9898, 78.233))) · 43758.5453)
//! T = 1;  L = 0
//! repeat STEPS:
//!     (σ, light) = volume(cam + dir·t)           // R = extinction 1/m, G = light 0–1
//!     if σ > 0.005:
//!         shade = mix(shadowColor, color, light)
//!         α = 1 − exp(−σ · step)
//!         L += shade · α · T;  T *= 1 − α
//!     if T < 0.015: break
//!     t += step
//! out = vec4(L / max(1 − T, 1e-4), 1 − T)
//! ```
//!
//! `color` = `0xd8dfe6`, `shadowColor` = `0x3b4758` (sRGB, linearize first).
//! The opacity (`density`, 0.7 in `Simulation`) and the light direction
//! `(−0.35, 0.8, 0.4)` are already baked into the grid; `light` is
//! `0.7 e^(−2.5τ) + 0.3 e^(−0.35τ)` with `τ` the optical depth over six taps
//! toward the light (0.03 … 0.58 m).
//!
//! # Liquid ([`liquid_surface::LiquidSurfaceField`], `FluidSurfaceRenderer.ts`)
//!
//! A box over the field drawn with back faces, depth test off, depth write on
//! (the hit's clip depth), drawn before other transparent objects.
//! `d(p) = field(p).r · voxel` is a signed distance in metres.
//!
//! ```text
//! // Sphere trace [t0, min(t1, sceneDistance)] (box slab test as above):
//! repeat 128: d = d(cam + dir·t); if d < 0: hit, break
//!             prev = t; t += max(0.8·d, 0.3·voxel); if t > end: break
//! two secant steps between prev (outside) and t (inside); discard on a miss.
//! n = normalize(∇d) by central differences one voxel apart
//! n = normalize(n + v · max(−dot(n, v), 0) · 1.02)       // v = −dir
//! cosV = clamp(dot(n, v), 1e-4, 1)
//! // Transmission: refract and integrate liquid along the bent ray.
//! tr = refract(dir, n, 1/ior)
//! optical = 0; s = 0.5·voxel
//! repeat 32: f = field(hit + tr·s); optical += clamp(f.g, 0, 1) · 1.5·voxel; s += 1.5·voxel
//!            if f.r > 1.5 or s > box exit: break
//! behind = scene colour where the bent ray meets the depth buffer
//!          (two depth re-projections from reach = min(clamp(sceneDist − tHit, 0, 1.5), s + 2·voxel))
//! transmittance = pow(max(color, 1e-3), optical / attenuationDistance)
//! ambient = env(n, rough = 1) + sun · (0.5·dot(n, l) + 0.5) · 0.3
//! scattered = ambient · pow(color, 0.5·optical/attenuationDistance) · scattering
//!             · (1 − dot(transmittance, 1/3))
//! body = behind · transmittance + scattered
//! // Reflection and the key light (GGX).
//! f0 = ((ior − 1)/(ior + 1))²;  F = f0 + (1 − f0)(1 − cosV)⁵
//! reflected = env(reflect(dir, n), roughness) · envIntensity   // + optional SSR
//! a = roughness²;  a2 = a²;  h = normalize(l + v)
//! D = a2 / (π (nh²(a2 − 1) + 1)²)
//! V = 0.5 / max(nl·sqrt(cosV²(1 − a2) + a2) + cosV·sqrt(nl²(1 − a2) + a2), 1e-5)
//! spec = min(sun · D · V · nl, 64)
//! dielectric = mix(body, reflected, F) + spec · F
//! metal = (reflected + spec) · (metalColor + (1 − metalColor)(1 − cosV)⁵)
//! out = vec4(mix(dielectric, metal, metalness), 1)
//! ```
//!
//! Defaults: `color 0x3a9fcf` (`Simulation`; `0x2a8fb0` for a bare renderer),
//! `attenuationDistance 0.6` m, `scattering 0.08`, `ior 1.333`, `roughness
//! 0.04` (clamped 0.02–1), `envIntensity 1`, `metalness 0`, `metalColor
//! 0xc8d2da`. Without an environment map, `env(d) = mix((0.05, 0.06, 0.08),
//! (0.5, 0.55, 0.6), 0.5·d.y + 0.5)`. `sun` is the key light's colour ×
//! intensity and `l` the direction toward it. Screen-space reflections march
//! the reflected ray from `t = voxel`, growing ×1.3 for 24 steps, accept a
//! crossing `0 < depth error < 0.35·t + 0.05`, bisect 4 times, and blend by
//! `found · edge fade · clamp(1 − 2.5·roughness, 0, 1)`.

pub mod liquid_surface;
pub mod smoke_volume;

use rayon::prelude::*;

/// Parallel, deterministic scatter into a z-sliced grid. Every item touches
/// slices `z` and `z + 1` only (trilinear / cloud-in-cell splats). Items are
/// bucketed by `z`; even buckets run in parallel, then odd ones, so no two
/// tasks write the same slice and each voxel's sum order is fixed.
pub(crate) fn splat_two_slices<T, F>(
    grid: &mut [T],
    slice_len: usize,
    slices: usize,
    items: &[(u32, u32)],
    f: F,
) where
    T: Send,
    F: Fn(u32, &mut [T], &mut [T]) + Sync + Send,
{
    if slices < 2 || items.is_empty() {
        return;
    }
    // Counting sort of items by slice.
    let mut start = vec![0u32; slices + 1];
    for &(z, _) in items {
        start[z as usize + 1] += 1;
    }
    for z in 0..slices {
        start[z + 1] += start[z];
    }
    let mut cursor = start.clone();
    let mut sorted = vec![0u32; items.len()];
    for &(z, item) in items {
        sorted[cursor[z as usize] as usize] = item;
        cursor[z as usize] += 1;
    }
    for parity in 0..2 {
        // Pairs of slices (z, z + 1) with z ≡ parity (mod 2), as disjoint chunks.
        let offset = parity * slice_len;
        let tail = &mut grid[offset..];
        tail.par_chunks_mut(2 * slice_len)
            .enumerate()
            .for_each(|(k, chunk)| {
                let z = 2 * k + parity;
                if z + 1 >= slices || chunk.len() < 2 * slice_len {
                    return;
                }
                let (lo, hi) = chunk.split_at_mut(slice_len);
                for &item in &sorted[start[z] as usize..start[z + 1] as usize] {
                    f(item, lo, hi);
                }
            });
    }
}
