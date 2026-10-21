//! The open sea: a Tessendorf FFT ocean in three cascades (a port of Emberfall's three.js sea,
//! `src/sea/scene/ocean/`). Each cascade is an N×N tile of the sea at a different scale
//! (800 m swell, 151 m wind sea, 27.7 m chop), each owning a band of wavenumbers so no wave is
//! drawn twice. Every frame, on a worker thread:
//!
//!   evolve    h(k,t) = h0(k) e^(-iwt) + conj(h0(-k)) e^(iwt) and the seven fields the surface needs
//!             (packed two real fields per complex one)
//!   IFFT      2D inverse FFTs (rustfft)
//!   assemble  disp = (choppy·Dx, h, choppy·Dz, J), norm = (slope.x, slope.z, slope², foam) with mips
//!             (slope² keeps the slope variance in the mips: the sun's glitter widens far off, LEAN);
//!             foam is built where the surface folds (Jacobian) and fades over seconds
//!
//! The results stream to six half-float textures the water shader reads, plus the caustics the
//! finest cascade focuses on the sea floor.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use half::f16;
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};

use crate::terrain::gpu::{PendingTexWrites, TexWrite};

pub const N: usize = 256;
pub const MIPS: u32 = 9; // 256 → 1
pub const CASCADE_L: [f32; 3] = [800.0, 151.0, 27.7];
const CASCADE_HI: [Option<i32>; 3] = [Some(31), Some(37), None];
pub const CHOP_C: [f32; 3] = [1.3, 1.0, 0.8];
const G: f32 = 9.81;
const REPEAT: f32 = 400.0;
const TAU: f32 = std::f32::consts::TAU;
const SPREAD_MAX: f32 = 400.0;
const LONG_N: i32 = 8;

/// One wave system (JONSWAP): significant height, peak period, peak sharpness, directional
/// spread (cos^2s exponent at the peak), short-wave tail lift, heading offset from the wind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveSystem {
    pub hs: f32,
    pub tp: f32,
    pub gamma: f32,
    pub spread: f32,
    pub tail: f32,
    pub off: f32,
}

/// A sea state: a wind sea + a swell, how choppy the crests are, when they foam, and the look.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeaState {
    pub windsea: WaveSystem,
    pub swell: WaveSystem,
    pub choppy: f32,
    pub foam_j: [f32; 3],
    pub foam_gain: f32,
    pub foam_fade: f32,
    pub foam: f32,
    pub whitecaps: f32,
    pub streaks: f32,
    pub chop: f32,
    pub wind: f32,
}

const fn sys(hs: f32, tp: f32, gamma: f32, spread: f32, tail: f32, off: f32) -> WaveSystem {
    WaveSystem {
        hs,
        tp,
        gamma,
        spread,
        tail,
        off,
    }
}

/// The named sea states, calm → storm (Emberfall's, plus a gentle "calm" for coasts).
pub const SEA_STATES: [(&str, SeaState); 5] = [
    (
        "Glassy",
        SeaState {
            windsea: sys(0.22, 2.6, 2.0, 6.0, 0.0, 0.0),
            swell: sys(1.0, 12.0, 5.0, 45.0, 0.0, 25.0),
            choppy: 0.55,
            foam_j: [-1.0, -1.0, -1.0],
            foam_gain: 0.0,
            foam_fade: 3.0,
            foam: 0.0,
            whitecaps: 0.0,
            streaks: 0.0,
            chop: 0.12,
            wind: 0.05,
        },
    ),
    (
        "Calm",
        SeaState {
            windsea: sys(0.7, 4.2, 3.3, 8.0, 0.2, 0.0),
            swell: sys(0.9, 10.5, 4.0, 35.0, 0.0, -28.0),
            choppy: 0.8,
            foam_j: [0.9, 0.9, 0.86],
            foam_gain: 3.0,
            foam_fade: 2.5,
            foam: 0.3,
            whitecaps: 0.2,
            streaks: 0.0,
            chop: 0.35,
            wind: 0.2,
        },
    ),
    (
        "Fair",
        SeaState {
            windsea: sys(2.2, 6.8, 3.3, 9.0, 0.35, 0.0),
            swell: sys(1.6, 11.0, 4.0, 30.0, 0.0, -32.0),
            choppy: 0.95,
            foam_j: [0.86, 0.845, 0.81],
            foam_gain: 3.5,
            foam_fade: 3.0,
            foam: 0.65,
            whitecaps: 0.6,
            streaks: 0.0,
            chop: 0.6,
            wind: 0.42,
        },
    ),
    (
        "Heavy",
        SeaState {
            windsea: sys(4.8, 8.6, 3.3, 8.0, 0.35, 0.0),
            swell: sys(2.4, 12.5, 4.0, 25.0, 0.0, 22.0),
            choppy: 1.05,
            foam_j: [0.78, 0.775, 0.75],
            foam_gain: 3.0,
            foam_fade: 4.0,
            foam: 0.8,
            whitecaps: 0.7,
            streaks: 0.45,
            chop: 0.8,
            wind: 0.72,
        },
    ),
    (
        "Storm",
        SeaState {
            windsea: sys(9.5, 10.0, 3.3, 7.0, 0.45, 0.0),
            swell: sys(3.5, 13.0, 3.0, 20.0, 0.0, -15.0),
            choppy: 1.22,
            foam_j: [0.64, 0.66, 0.63],
            foam_gain: 3.0,
            foam_fade: 5.0,
            foam: 1.0,
            whitecaps: 1.0,
            streaks: 1.0,
            chop: 1.0,
            wind: 1.0,
        },
    ),
];

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_sys(a: WaveSystem, b: WaveSystem, t: f32) -> WaveSystem {
    WaveSystem {
        hs: lerp(a.hs, b.hs, t),
        tp: lerp(a.tp, b.tp, t),
        gamma: lerp(a.gamma, b.gamma, t),
        spread: lerp(a.spread, b.spread, t),
        tail: lerp(a.tail, b.tail, t),
        off: lerp(a.off, b.off, t),
    }
}

impl SeaState {
    pub fn lerp(self, o: SeaState, t: f32) -> SeaState {
        SeaState {
            windsea: lerp_sys(self.windsea, o.windsea, t),
            swell: lerp_sys(self.swell, o.swell, t),
            choppy: lerp(self.choppy, o.choppy, t),
            foam_j: std::array::from_fn(|i| lerp(self.foam_j[i], o.foam_j[i], t)),
            foam_gain: lerp(self.foam_gain, o.foam_gain, t),
            foam_fade: lerp(self.foam_fade, o.foam_fade, t),
            foam: lerp(self.foam, o.foam, t),
            whitecaps: lerp(self.whitecaps, o.whitecaps, t),
            streaks: lerp(self.streaks, o.streaks, t),
            chop: lerp(self.chop, o.chop, t),
            wind: lerp(self.wind, o.wind, t),
        }
    }
    /// Continuous scale: 0 glassy, 1 calm, 2 fair, 3 heavy, 4 storm.
    pub fn at(level: f32) -> SeaState {
        let l = level.clamp(0.0, 4.0);
        let i = (l.floor() as usize).min(3);
        SEA_STATES[i].1.lerp(SEA_STATES[i + 1].1, l - i as f32)
    }
    pub fn hs(&self) -> f32 {
        (self.windsea.hs * self.windsea.hs + self.swell.hs * self.swell.hs).sqrt()
    }
}

// ---------------------------------------------------------------- spectrum

fn jonswap(w: f32, alpha: f32, wp: f32, gamma: f32, tail: f32) -> f32 {
    let sig = if w <= wp { 0.07 } else { 0.09 };
    let pm = alpha * G * G / w.powi(5) * (-1.25 * (wp / w).powi(4)).exp();
    let peak = gamma.powf((-((w - wp) * (w - wp)) / (2.0 * sig * sig * wp * wp)).exp());
    let u = ((w / wp - 1.6) / 2.9).clamp(0.0, 1.0);
    pm * peak * (1.0 + tail * u * u * (3.0 - 2.0 * u))
}

/// JONSWAP's integral for alpha = 1, to scale alpha to a significant height.
fn m0_unit(wp: f32, gamma: f32, tail: f32) -> f32 {
    let n = 480;
    let (a, b) = ((wp * 0.35).ln(), (wp * 14.0).ln());
    let mut sum = 0.0;
    for i in 0..=n {
        let w = (a + (b - a) * i as f32 / n as f32).exp();
        let s = jonswap(w, 1.0, wp, gamma, tail) * w;
        sum += if i == 0 || i == n { 0.5 } else { 1.0 } * s;
    }
    sum * (b - a) / n as f32
}

#[derive(Clone, Copy)]
struct Prepared {
    alpha: f32,
    wp: f32,
    gamma: f32,
    sp: f32,
    dx: f32,
    dz: f32,
    tail: f32,
}

fn prepare(state: &SeaState, wind_az: f32) -> Vec<Prepared> {
    let mut out = vec![];
    for (s, off) in [
        (state.windsea, 0.0),
        (state.swell, state.swell.off.to_radians()),
    ] {
        let hs = s.hs.max(0.0);
        let tp = s.tp.max(0.8);
        let gamma = s.gamma.max(1.0);
        let tail = s.tail.max(0.0);
        let wp = TAU / tp;
        let alpha = if hs > 1e-4 {
            (hs / 4.0) * (hs / 4.0) / m0_unit(wp, gamma, tail)
        } else {
            0.0
        };
        let az = wind_az + off;
        if alpha > 0.0 {
            out.push(Prepared {
                alpha,
                wp,
                gamma,
                sp: s.spread.max(0.5),
                dx: az.sin(),
                dz: -az.cos(),
                tail,
            });
        }
    }
    out
}

fn spectrum_at(kx: f32, kz: f32, systems: &[Prepared]) -> f32 {
    let k = (kx * kx + kz * kz).sqrt();
    if k < 1e-6 {
        return 0.0;
    }
    let w = (G * k).sqrt();
    let mut s_total = 0.0;
    for c in systems {
        let sk = jonswap(w, c.alpha, c.wp, c.gamma, c.tail) * G / (2.0 * w);
        let r = w / c.wp;
        let s = (c.sp * if r < 1.0 { r.powi(5) } else { r.powf(-2.5) }).clamp(0.35, SPREAD_MAX);
        let cos_d = (kx * c.dx + kz * c.dz) / k;
        let h = ((1.0 + cos_d) * 0.5).max(0.0);
        if h <= 0.0 {
            continue;
        }
        let d = (s / (4.0 * std::f32::consts::PI)).sqrt() * (1.0 + 1.0 / (8.0 * s)) * h.powf(s);
        s_total += sk * d / k;
    }
    s_total * (-k * k * 0.0004).exp()
}

fn hash32(a: u32) -> u32 {
    let a = (a ^ (a >> 16)).wrapping_mul(0x7feb_352d);
    let a = (a ^ (a >> 15)).wrapping_mul(0x846c_a68b);
    a ^ (a >> 16)
}

/// A mode's two Gaussian random numbers (Box–Muller of a hash). The longest swell modes keep
/// the mean amplitude so the swell never has holes.
fn gauss2(c: usize, nx: i32, nz: i32, seed: u32) -> (f32, f32) {
    let mut h = hash32(
        seed.wrapping_mul(0x9e37_79b1)
            ^ hash32(
                ((c as u32 + 1).wrapping_mul(0x85eb_ca6b))
                    ^ hash32(
                        (nx as u32).wrapping_mul(73_856_093)
                            ^ hash32(
                                (nz as u32)
                                    .wrapping_mul(19_349_663)
                                    .wrapping_add(0x27d4_eb2d),
                            ),
                    ),
            ),
    );
    let u1 = (h as f64 + 0.5) / 4_294_967_296.0;
    h = hash32(h ^ 0x1656_67b1);
    let u2 = (h as f64 + 0.5) / 4_294_967_296.0;
    let r = if c == 0 && nx.abs().max(nz.abs()) <= LONG_N {
        std::f64::consts::SQRT_2
    } else {
        (-2.0 * u1.ln()).sqrt()
    };
    let a = std::f64::consts::TAU * u2;
    ((r * a.cos()) as f32, (r * a.sin()) as f32)
}

fn signed(m: usize) -> i32 {
    if m < N / 2 {
        m as i32
    } else {
        m as i32 - N as i32
    }
}

fn in_band(c: usize, nx: i32, nz: i32) -> bool {
    if nx == 0 && nz == 0 {
        return false;
    }
    if nx.abs() >= N as i32 / 2 || nz.abs() >= N as i32 / 2 {
        return false;
    }
    if let Some(hi) = CASCADE_HI[c]
        && nx.abs().max(nz.abs()) > hi
    {
        return false;
    }
    if c > 0 {
        let prev_hi = CASCADE_HI[c - 1].unwrap_or(N as i32 / 2) as f32;
        let edge = (prev_hi + 0.5) * TAU / CASCADE_L[c - 1];
        let dk = TAU / CASCADE_L[c];
        if nx.abs() as f32 * dk <= edge && nz.abs() as f32 * dk <= edge {
            return false;
        }
    }
    true
}

fn omega_of(k: f32) -> f32 {
    let w0 = TAU / REPEAT;
    ((G * k).sqrt() / w0).floor() * w0
}

// ---------------------------------------------------------------- simulation

struct Cascade {
    /// (xi.re, xi.im, omega, in-band)
    modes: Vec<[f32; 4]>,
    /// (h0.re, h0.im, conj(h0(-k)).re, .im)
    h0: Vec<[f32; 4]>,
    foam: Vec<f32>,
    fields: [Vec<Complex32>; 4],
    scratch: Vec<Complex32>,
    tmp: Vec<Complex32>,
}

pub struct OceanSim {
    cascades: Vec<Cascade>,
    fft: Arc<dyn Fft<f32>>,
    frames: u32,
}

/// Mip chains (LayerMajor-free: one texture each) of the two outputs per cascade, as RGBA16F bytes.
pub struct OceanFrame {
    pub disp: [Vec<u8>; 3],
    pub norm: [Vec<u8>; 3],
    /// caustics under the finest cascade (R, G: two floor depths, see `caustics`), mean 1
    pub caustics: Vec<u8>,
    /// mean height of cascade 0 near the origin (not used yet: floating objects)
    pub _hs: f32,
}

impl OceanSim {
    pub fn new(seed: u32) -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_inverse(N);
        let scratch_len = fft.get_inplace_scratch_len();
        let cascades = (0..3)
            .map(|c| {
                let dk = TAU / CASCADE_L[c];
                let mut modes = vec![[0.0f32; 4]; N * N];
                for mz in 0..N {
                    for mx in 0..N {
                        let (nx, nz) = (signed(mx), signed(mz));
                        if !in_band(c, nx, nz) {
                            continue;
                        }
                        let (a, b) = gauss2(c, nx, nz, seed);
                        let k = dk * ((nx * nx + nz * nz) as f32).sqrt();
                        modes[mz * N + mx] = [a, b, omega_of(k), 1.0];
                    }
                }
                Cascade {
                    modes,
                    h0: vec![[0.0; 4]; N * N],
                    foam: vec![0.0; N * N],
                    fields: std::array::from_fn(|_| vec![Complex32::new(0.0, 0.0); N * N]),
                    scratch: vec![Complex32::new(0.0, 0.0); scratch_len.max(N)],
                    tmp: vec![Complex32::new(0.0, 0.0); N * N],
                }
            })
            .collect();
        Self {
            cascades,
            fft,
            frames: 0,
        }
    }

    /// Recompute h0 for a sea state and wind direction (radians, the direction waves run toward).
    pub fn set_state(&mut self, state: &SeaState, wind_az: f32) {
        let systems = prepare(state, wind_az);
        for (c, cas) in self.cascades.iter_mut().enumerate() {
            let dk = TAU / CASCADE_L[c];
            let amp = |mx: usize, mz: usize| {
                let m = cas.modes[mz * N + mx];
                if m[3] < 0.5 {
                    return (0.0, 0.0);
                }
                let (nx, nz) = (signed(mx), signed(mz));
                let s = spectrum_at(nx as f32 * dk, nz as f32 * dk, &systems);
                let scale = s.max(0.0).sqrt() * dk * 0.5;
                (m[0] * scale, m[1] * scale)
            };
            for mz in 0..N {
                for mx in 0..N {
                    let (a, b) = amp(mx, mz);
                    let (am, bm) = amp((N - mx) % N, (N - mz) % N);
                    cas.h0[mz * N + mx] = [a, b, am, -bm];
                }
            }
        }
    }

    /// Advance to time `t` (s) and produce the textures (`sun`: direction to the key light).
    pub fn step(&mut self, t: f32, dt: f32, state: &SeaState, sun: Vec3) -> OceanFrame {
        let tt = t.rem_euclid(REPEAT);
        let frames = self.frames;
        self.frames += 1;
        let fft = self.fft.clone();
        let outs: Vec<CascadeOut> = std::thread::scope(|s| {
            let handles: Vec<_> = self
                .cascades
                .iter_mut()
                .enumerate()
                .map(|(c, cas)| {
                    let fft = fft.clone();
                    s.spawn(move || cas.step(c, tt, dt, state, frames, &*fft, sun))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let mut it = outs.into_iter();
        let (d0, n0, _) = it.next().unwrap();
        let (d1, n1, _) = it.next().unwrap();
        let (d2, n2, ca) = it.next().unwrap();
        OceanFrame {
            disp: [d0, d1, d2],
            norm: [n0, n1, n2],
            caustics: ca.unwrap_or_default(),
            _hs: state.hs(),
        }
    }
}

/// One cascade's frame: displacement and normal mip chains, caustics (finest cascade only).
type CascadeOut = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);

impl Cascade {
    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        c: usize,
        t: f32,
        dt: f32,
        state: &SeaState,
        frames: u32,
        fft: &dyn Fft<f32>,
        sun: Vec3,
    ) -> CascadeOut {
        let dk = TAU / CASCADE_L[c];
        // ---- evolve
        for mz in 0..N {
            for mx in 0..N {
                let i = mz * N + mx;
                let m = self.modes[i];
                if m[3] < 0.5 {
                    for f in &mut self.fields {
                        f[i] = Complex32::new(0.0, 0.0);
                    }
                    continue;
                }
                let h0 = self.h0[i];
                let ph = -(m[2] * t).rem_euclid(TAU);
                let (s, co) = ph.sin_cos();
                // ht = h0 e + conj(h0(-k)) conj(e)
                let ht = Complex32::new(h0[0] * co - h0[1] * s, h0[0] * s + h0[1] * co)
                    + Complex32::new(h0[2] * co + h0[3] * s, -h0[2] * s + h0[3] * co);
                let (nx, nz) = (signed(mx) as f32, signed(mz) as f32);
                let (kx, kz) = (nx * dk, nz * dk);
                let k = (kx * kx + kz * kz).sqrt();
                let (knx, knz) = if k > 1e-6 {
                    (kx / k, kz / k)
                } else {
                    (0.0, 0.0)
                };
                let iht = Complex32::new(-ht.im, ht.re);
                let mul_i = |a: Complex32| Complex32::new(-a.im, a.re);
                let dx = iht * knx;
                let dz = iht * knz;
                let hx = iht * kx;
                let hz = iht * kz;
                let dxx = -ht * (kx * knx);
                let dzz = -ht * (kz * knz);
                let dxz = -ht * (kx * knz);
                self.fields[0][i] = ht + mul_i(dx);
                self.fields[1][i] = dz + mul_i(hx);
                self.fields[2][i] = hz + mul_i(dxx);
                self.fields[3][i] = dzz + mul_i(dxz);
            }
        }
        // ---- 2D inverse FFT: rows, transpose, rows, transpose back
        for f in &mut self.fields {
            fft.process_with_scratch(f, &mut self.scratch);
            transpose(f, &mut self.tmp);
            fft.process_with_scratch(&mut self.tmp, &mut self.scratch);
            transpose(&self.tmp, f);
        }
        // ---- assemble
        let lam = state.choppy * CHOP_C[c];
        let fade = state.foam_fade.max(0.2);
        let (decay, build) = if frames < 2 {
            (0.0, 0.5)
        } else {
            ((-dt / fade).exp(), (dt * 2.5).min(1.0))
        };
        let foam_j = state.foam_j[c];
        let mut disp = vec![[0.0f32; 4]; N * N];
        let mut norm = vec![[0.0f32; 4]; N * N];
        for i in 0..N * N {
            let a = self.fields[0][i];
            let b = self.fields[1][i];
            let cc = self.fields[2][i];
            let d = self.fields[3][i];
            let (h, dx, dz, hx, hz, dxx, dzz, dxz) =
                (a.re, a.im, b.re, b.im, cc.re, cc.im, d.re, d.im);
            let jxx = 1.0 + lam * dxx;
            let jzz = 1.0 + lam * dzz;
            let jxz = lam * dxz;
            let jac = jxx * jzz - jxz * jxz;
            let inv = 1.0 / jac.max(0.12);
            let sx = ((jzz * hx - jxz * hz) * inv).clamp(-6.0, 6.0);
            let sz = ((jxx * hz - jxz * hx) * inv).clamp(-6.0, 6.0);
            let gen_ = ((foam_j - jac) * state.foam_gain).clamp(0.0, 1.0);
            let foam = (self.foam[i] * decay + gen_ * build).clamp(0.0, 1.0);
            self.foam[i] = foam;
            disp[i] = [lam * dx, h, lam * dz, jac];
            norm[i] = [sx, sz, sx * sx + sz * sz, foam];
        }
        let caustics = (c == 2).then(|| mip_chain_f16(&caustics(&norm, sun)));
        (mip_chain_f16(&disp), mip_chain_f16(&norm), caustics)
    }
}

/// Caustics on a flat floor under the finest cascade (tidewater Caustics.js, as an area ratio
/// instead of a photon splat): each texel's refracted sun ray lands `depth` metres down, shifted
/// by its facet's slope; where neighbouring rays converge the light focuses. Intensity is the
/// inverse of the landing area per texel (soft-capped), at two depths, normalised to mean 1.
/// (The shader reads the map at half scale: the same net under waves half the size, i.e. at half
/// these depths: 1.5 m and 4 m, where real seas focus their short waves.)
fn caustics(norm: &[[f32; 4]], sun: Vec3) -> Vec<[f32; 4]> {
    const DEPTHS: [f32; 2] = [3.0, 8.0];
    let texel = CASCADE_L[2] / N as f32;
    let l = if sun.y > 0.05 {
        sun.normalize()
    } else {
        Vec3::new(sun.x, 0.05, sun.z).normalize()
    };
    let eta = 1.0 / 1.333;
    // refract(-l, n): the sun's ray inside the water
    let refr = |n: Vec3| {
        let cosi = l.dot(n).max(0.0);
        let k = (1.0 - eta * eta * (1.0 - cosi * cosi)).max(0.0);
        -l * eta + n * (eta * cosi - k.sqrt())
    };
    let t0 = refr(Vec3::Y);
    let base = Vec2::new(t0.x, t0.z) / (-t0.y).max(0.05);
    // horizontal drift of each texel's ray per metre of depth, relative to a flat surface
    let drift: Vec<Vec2> = norm
        .iter()
        .map(|s| {
            let t = refr(Vec3::new(-s[0], 1.0, -s[1]).normalize());
            Vec2::new(t.x, t.z) / (-t.y).max(0.05) - base
        })
        .collect();
    let at = |x: usize, y: usize| drift[(y % N) * N + (x % N)];
    let mut out = vec![[0.0f32, 0.0, 0.0, 1.0]; N * N];
    for (k, depth) in DEPTHS.iter().enumerate() {
        let g = depth / (2.0 * texel);
        let mut sum = 0.0;
        for y in 0..N {
            for x in 0..N {
                let a = (at(x + 1, y) - at(x + N - 1, y)) * g;
                let b = (at(x, y + 1) - at(x, y + N - 1)) * g;
                let j = (1.0 + a.x) * (1.0 + b.y) - a.y * b.x;
                let i = 1.125 / (j.abs() + 0.125);
                out[y * N + x][k] = i;
                sum += i;
            }
        }
        let inv = (N * N) as f32 / sum.max(1e-6);
        for o in out.iter_mut() {
            o[k] *= inv;
        }
    }
    out
}

fn transpose(src: &[Complex32], dst: &mut [Complex32]) {
    const B: usize = 16;
    for by in (0..N).step_by(B) {
        for bx in (0..N).step_by(B) {
            for y in by..by + B {
                for x in bx..bx + B {
                    dst[x * N + y] = src[y * N + x];
                }
            }
        }
    }
}

/// All mip levels (box filter, wrapping) as RGBA16F bytes, level 0 first.
fn mip_chain_f16(base: &[[f32; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(N * N * 8 * 4 / 3 + 64);
    let push = |out: &mut Vec<u8>, lvl: &[[f32; 4]]| {
        for p in lvl {
            for v in p {
                out.extend_from_slice(&f16::from_f32(*v).to_le_bytes());
            }
        }
    };
    push(&mut out, base);
    let mut prev = base.to_vec();
    let mut s = N;
    while s > 1 {
        let n = s / 2;
        let mut next = vec![[0.0f32; 4]; n * n];
        for y in 0..n {
            for x in 0..n {
                let a = prev[(2 * y) * s + 2 * x];
                let b = prev[(2 * y) * s + 2 * x + 1];
                let c = prev[(2 * y + 1) * s + 2 * x];
                let d = prev[(2 * y + 1) * s + 2 * x + 1];
                next[y * n + x] = std::array::from_fn(|k| (a[k] + b[k] + c[k] + d[k]) * 0.25);
            }
        }
        push(&mut out, &next);
        prev = next;
        s = n;
    }
    out
}

// ---------------------------------------------------------------- Bevy integration

/// The six FFT textures + caustics + procedural noise and foam-lace textures used by the water
/// shader.
#[derive(Resource, Clone)]
pub struct OceanTextures {
    pub disp: [Handle<Image>; 3],
    pub norm: [Handle<Image>; 3],
    pub caustics: Handle<Image>,
    pub noise: Handle<Image>,
    pub foam: Handle<Image>,
}

/// The latest displacement cascades on the CPU (RGBA16F mip chains, mip 0 first): swimmers bob on
/// the waves the water shader draws (`motion::waves`).
#[derive(Resource, Default, Clone)]
pub struct OceanWaves {
    pub disp: Option<[Arc<Vec<u8>>; 3]>,
}

/// Current sea, chosen from the ocean's water style and the weather.
#[derive(Resource, Clone)]
pub struct OceanState {
    pub level: f32,
    pub wind_az: f32,
    pub state: SeaState,
    /// wave clock rate (the ocean style's wave speed)
    pub speed: f32,
    applied: Option<(f32, f32)>,
}

impl Default for OceanState {
    fn default() -> Self {
        Self {
            level: 1.0,
            wind_az: 0.6,
            state: SeaState::at(1.0),
            speed: 1.0,
            applied: None,
        }
    }
}

#[derive(Resource)]
struct OceanRunner {
    sim: Option<OceanSim>,
    task: Option<Task<(OceanSim, OceanFrame)>>,
    /// wave clock (s): stops while motion is paused
    clock: f32,
    /// real time since the last step
    since: f32,
    steps: u32,
}

/// Whether the spectrum baked into the current FFT differs enough from the selected sea state to
/// need rebuilding. Kept separate from `run_sim` so paused editing cannot accidentally bypass
/// this decision before it gets a chance to update the visible surface.
fn sea_state_changed(applied: Option<(f32, f32)>, key: (f32, f32)) -> bool {
    match applied {
        Some((level, azimuth)) => (level - key.0).abs() > 0.02 || (azimuth - key.1).abs() > 0.01,
        None => true,
    }
}

/// Distance (m) from anywhere to the nearest open sea, on a coarse grid; off-map counts as sea.
#[derive(Default)]
struct CoastDistance {
    revision: u64,
    computed_at: f32,
    n: usize,
    cell: f32,
    half: f32,
    dist: Vec<f32>,
}

impl CoastDistance {
    fn update(&mut self, t: &crate::terrain::Terrain, now: f32) {
        let fresh = !self.dist.is_empty() && self.revision == t.height_revision;
        // sculpting changes heights every frame: refresh at most every few seconds
        if fresh || (!self.dist.is_empty() && now - self.computed_at < 3.0) {
            return;
        }
        let n = 64usize;
        let (half, cell) = (t.half(), t.size() / n as f32);
        let mut d = vec![f32::MAX; n * n];
        for j in 0..n {
            for i in 0..n {
                let (x, z) = (
                    -half + (i as f32 + 0.5) * cell,
                    -half + (j as f32 + 0.5) * cell,
                );
                if t.height_at(x, z) < t.sea_level - 0.5 {
                    d[j * n + i] = 0.0;
                } else if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
                    d[j * n + i] = cell * 0.5;
                }
            }
        }
        // two-pass chamfer distance transform
        let diag = cell * std::f32::consts::SQRT_2;
        for pass in 0..2 {
            let order: Vec<usize> = if pass == 0 {
                (0..n * n).collect()
            } else {
                (0..n * n).rev().collect()
            };
            for k in order {
                let (i, j) = ((k % n) as isize, (k / n) as isize);
                let dirs: [(isize, isize, f32); 4] = if pass == 0 {
                    [(-1, 0, cell), (0, -1, cell), (-1, -1, diag), (1, -1, diag)]
                } else {
                    [(1, 0, cell), (0, 1, cell), (1, 1, diag), (-1, 1, diag)]
                };
                for (di, dj, w) in dirs {
                    let (a, b) = (i + di, j + dj);
                    if a >= 0 && b >= 0 && a < n as isize && b < n as isize {
                        let v = d[b as usize * n + a as usize] + w;
                        if v < d[k] {
                            d[k] = v;
                        }
                    }
                }
            }
        }
        *self = Self {
            revision: t.height_revision,
            computed_at: now,
            n,
            cell,
            half,
            dist: d,
        };
    }
    fn at(&self, p: Vec2) -> f32 {
        if self.dist.is_empty() {
            return 0.0;
        }
        let g = ((p + Vec2::splat(self.half)) / self.cell).floor();
        if g.x < 0.0 || g.y < 0.0 || g.x >= self.n as f32 || g.y >= self.n as f32 {
            return 0.0;
        }
        self.dist[g.y as usize * self.n + g.x as usize]
    }
}

pub struct OceanPlugin;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<OceanState>()
            .init_resource::<OceanWaves>()
            .add_systems(PreStartup, create_textures)
            .add_systems(Update, (choose_state, run_sim).chain());
    }
}

fn fft_image(format: TextureFormat, data: Vec<u8>, size: u32, mips: u32) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = mips;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    img
}

fn create_textures(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let zero = mip_chain_f16(&vec![[0.0, 0.0, 0.0, 1.0]; N * N]);
    let disp = std::array::from_fn(|_| {
        images.add(fft_image(
            TextureFormat::Rgba16Float,
            zero.clone(),
            N as u32,
            MIPS,
        ))
    });
    let norm = std::array::from_fn(|_| {
        images.add(fft_image(
            TextureFormat::Rgba16Float,
            zero.clone(),
            N as u32,
            MIPS,
        ))
    });
    let caustics = images.add(fft_image(
        TextureFormat::Rgba16Float,
        mip_chain_f16(&vec![[1.0, 1.0, 0.0, 1.0]; N * N]),
        N as u32,
        MIPS,
    ));
    let noise = images.add(noise_texture());
    let foam = images.add(foam_texture());
    commands.insert_resource(OceanTextures {
        disp,
        norm,
        caustics,
        noise,
        foam,
    });
    let mut sim = OceanSim::new(7);
    let st = OceanState::default();
    sim.set_state(&st.state, st.wind_az);
    commands.insert_resource(OceanRunner {
        sim: Some(sim),
        task: None,
        clock: 0.0,
        since: 0.0,
        steps: 0,
    });
}

/// Pick the sea state from the ocean's "waves" setting, raised by stormy weather.
fn choose_state(
    style: Res<crate::water::WaterStyle>,
    sky: Res<crate::sky::SkyState>,
    wind: Res<crate::objects::Wind>,
    mut st: ResMut<OceanState>,
) {
    // waves slider 0..2.5 → glassy..storm (1.0 ≈ a fair working sea)
    let weather = sky.weather.rain * 1.2 + sky.weather.dim * 0.4;
    let level = (style.waves * 1.6 + weather).clamp(0.0, 4.0);
    // the wind blows toward its direction vector: azimuth measured from north (-Z), clockwise
    let az = wind.dir.x.atan2(-wind.dir.y);
    st.level += (level - st.level) * 0.02;
    st.wind_az = az;
    st.state = SeaState::at(st.level);
    st.speed += (style.wave_speed.clamp(0.1, 3.0) - st.speed) * 0.05;
}

fn run_sim(
    mut runner: ResMut<OceanRunner>,
    mut st: ResMut<OceanState>,
    tex: Res<OceanTextures>,
    time: Res<Time>,
    sky: Res<crate::sky::SkyState>,
    mut pending: ResMut<PendingTexWrites>,
    mut waves: ResMut<OceanWaves>,
    motion: Res<crate::wind::Motion>,
    terrain: Res<crate::terrain::Terrain>,
    cam: Query<&GlobalTransform, With<crate::camera::MainCamera>>,
    mut coast: Local<CoastDistance>,
) {
    // collect a finished frame
    if let Some(task) = &mut runner.task
        && let Some((sim, frame)) = block_on(future::poll_once(task))
    {
        runner.task = None;
        runner.sim = Some(sim);
        let disp = frame.disp.map(Arc::new);
        for (h, data) in tex
            .disp
            .iter()
            .zip(disp.iter().cloned())
            .chain(tex.norm.iter().zip(frame.norm.map(Arc::new)))
        {
            push_mips(&mut pending, h, data);
        }
        waves.disp = Some(disp);
        if !frame.caustics.is_empty() {
            push_mips(&mut pending, &tex.caustics, Arc::new(frame.caustics));
        }
    }
    if runner.sim.is_none() || std::env::var("WB_PERF").is_ok_and(|p| p.contains("nooceansim")) {
        return;
    }
    // While editing the sea holds still (after a few frames to build the first waves). A changed
    // spectrum still gets one valid frame: otherwise a paused wave or wind edit leaves the old
    // FFT textures on screen indefinitely. Walking updates every frame near the coast and less
    // often far inland, where the FFT and texture uploads are small on screen but costly on CPU.
    runner.since += time.delta_secs();
    let key = (st.level, st.wind_az);
    let needs = sea_state_changed(st.applied, key);
    if !motion.live && runner.steps >= 3 && !needs {
        return;
    }
    coast.update(&terrain, time.elapsed_secs());
    let d = cam
        .single()
        .map(|c| coast.at(c.translation().xz()))
        .unwrap_or(0.0);
    let interval = if d < 300.0 {
        0.0
    } else if d < 900.0 {
        1.0 / 20.0
    } else {
        1.0 / 10.0
    };
    if runner.since < interval {
        return;
    }
    let Some(mut sim) = runner.sim.take() else {
        return;
    };
    runner.steps += 1;
    // the spectrum follows the sea state (rebuilt when it moved noticeably)
    let state = st.state;
    let az = st.wind_az;
    if needs {
        st.applied = Some(key);
    }
    // A paused edit rebuilds the changed spectrum at the current wave-clock position. `since`
    // may have accumulated for minutes while paused, so advancing it here would make the surface
    // jump just because a setting was touched.
    let dt = if motion.live {
        runner.since.clamp(0.0, 0.25)
    } else {
        0.0
    };
    runner.since = 0.0;
    runner.clock += dt * st.speed;
    let t = runner.clock;
    // caustics follow the key light (the moon at night)
    let sun = if sky.night > 0.5 {
        sky.moon_dir
    } else {
        sky.sun_dir
    };
    runner.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        if needs {
            sim.set_state(&state, az);
        }
        let frame = sim.step(t, dt.max(1.0 / 120.0), &state, sun);
        (sim, frame)
    }));
}

#[cfg(test)]
mod tests {
    use super::sea_state_changed;

    #[test]
    fn sea_state_change_detects_paused_preview_invalidations() {
        assert!(sea_state_changed(None, (1.0, 0.6)));
        assert!(sea_state_changed(Some((1.0, 0.6)), (1.03, 0.6)));
        assert!(sea_state_changed(Some((1.0, 0.6)), (1.0, 0.62)));
    }

    #[test]
    fn sea_state_change_ignores_smoothing_noise() {
        assert!(!sea_state_changed(Some((1.0, 0.6)), (1.019, 0.609)));
    }
}

fn push_mips(pending: &mut PendingTexWrites, h: &Handle<Image>, data: Arc<Vec<u8>>) {
    let mut offset = 0usize;
    let mut s = N as u32;
    for mip in 0..MIPS {
        let len = (s * s * 8) as usize;
        pending.0.push(TexWrite {
            image: h.id(),
            x: 0,
            y: 0,
            w: s,
            h: s,
            bytes_per_texel: 8,
            mip,
            offset,
            data: data.clone(),
        });
        offset += len;
        s = (s / 2).max(1);
    }
}

/// Tiling RGBA value noise (each channel a different octave pattern).
fn noise_texture() -> Image {
    let n = 256usize;
    let mut data = Vec::with_capacity(n * n * 4);
    let periodic = |x: f32, y: f32, p: i32, seed: u32| {
        let xi = x.floor() as i32;
        let yi = y.floor() as i32;
        let fx = x - xi as f32;
        let fy = y - yi as f32;
        let h = |a: i32, b: i32| {
            (crate::noise::hash2(a.rem_euclid(p), b.rem_euclid(p), seed) & 0xffff) as f32 / 65535.0
        };
        let u = fx * fx * (3.0 - 2.0 * fx);
        let v = fy * fy * (3.0 - 2.0 * fy);
        let a = h(xi, yi) + (h(xi + 1, yi) - h(xi, yi)) * u;
        let b = h(xi, yi + 1) + (h(xi + 1, yi + 1) - h(xi, yi + 1)) * u;
        a + (b - a) * v
    };
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            let ch = |p: i32, seed: u32| {
                let v = periodic(fx * p as f32, fy * p as f32, p, seed) * 0.6
                    + periodic(fx * p as f32 * 2.0, fy * p as f32 * 2.0, p * 2, seed + 1) * 0.3
                    + periodic(fx * p as f32 * 4.0, fy * p as f32 * 4.0, p * 4, seed + 2) * 0.1;
                (v * 255.0) as u8
            };
            data.extend_from_slice(&[ch(8, 1), ch(16, 7), ch(12, 13), ch(32, 19)]);
        }
    }
    rgba8_mipped(data, n)
}

/// A tiling RGBA8 texture with its full box-filtered mip chain (wrapping), so patterns sampled
/// far away average out instead of aliasing into sparkle.
fn rgba8_mipped(base: Vec<u8>, n: usize) -> Image {
    let mut data = base.clone();
    let mut prev = base;
    let mut s = n;
    let mut levels = 1;
    while s > 1 {
        let h = s / 2;
        let mut next = vec![0u8; h * h * 4];
        for y in 0..h {
            for x in 0..h {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| prev[(yy * s + xx) * 4 + c] as u32;
                    let sum = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[(y * h + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        prev = next;
        s = h;
        levels += 1;
    }
    let mut img = Image::new_uninit(
        Extent3d {
            width: n as u32,
            height: n as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = levels;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    img
}

/// Tiling sea foam (tidewater's FoamTexture.js): an irregular bubbly mat. Dense rafts with ragged
/// edges, holes of every size and fine bubbles; the water shader thresholds the density by the
/// local foam coverage, so foam grows, tears into lace and dissolves. Mat-like shapes keep their
/// contrast in the mips (thin cell walls averaged away into a grey haze).
///   R: foam density   G: fine bubbles   B: soft mottling   A: streaks
fn foam_texture() -> Image {
    const N: usize = 512;
    let hash = |x: i32, y: i32, cells: i32, seed: u32| {
        let h = crate::noise::hash2(x.rem_euclid(cells), y.rem_euclid(cells), seed);
        ((h & 0xffff) as f32 / 65535.0, (h >> 16) as f32 / 65535.0)
    };
    // periodic value noise and its fbm (`base` cells per tile, doubling per octave)
    let vnoise = |u: f32, v: f32, cells: f32, seed: u32| {
        let (px, py) = (u * cells, v * cells);
        let (ix, iy) = (px.floor(), py.floor());
        let (fx, fy) = (px - ix, py - iy);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let c = cells as i32;
        let (ix, iy) = (ix as i32, iy as i32);
        let a = hash(ix, iy, c, seed).0;
        let b = hash(ix + 1, iy, c, seed).0;
        let cc = hash(ix, iy + 1, c, seed).0;
        let d = hash(ix + 1, iy + 1, c, seed).0;
        let top = a + (b - a) * sx;
        let bot = cc + (d - cc) * sx;
        top + (bot - top) * sy
    };
    let fbm = |u: f32, v: f32, base: f32, oct: u32, seed: u32| {
        let (mut s, mut a, mut n) = (0.0, 0.5, 0.0);
        for o in 0..oct {
            s += vnoise(u, v, base * (1 << o) as f32, seed + o * 31) * a;
            n += a;
            a *= 0.5;
        }
        s / n
    };
    // periodic Worley F1
    let worley = |u: f32, v: f32, cells: f32, seed: u32| {
        let (px, py) = (u * cells, v * cells);
        let (ix, iy) = (px.floor() as i32, py.floor() as i32);
        let (fx, fy) = (px - px.floor(), py - py.floor());
        let mut f1 = 8.0f32;
        for j in -1..=1 {
            for i in -1..=1 {
                let h = hash(ix + i, iy + j, cells as i32, seed);
                let d = ((i as f32 + h.0 - fx).powi(2) + (j as f32 + h.1 - fy).powi(2)).sqrt();
                f1 = f1.min(d);
            }
        }
        f1
    };
    let ss = crate::noise::smoothstep;
    let texel = |x: usize, y: usize| -> [u8; 4] {
        let (u, v) = ((x as f32 + 0.5) / N as f32, (y as f32 + 0.5) / N as f32);
        // domain warp: organic, flowing shapes
        let w = (
            (fbm(u, v, 3.0, 4, 101) - 0.5) * 0.14,
            (fbm(u + 0.43, v + 0.43, 3.0, 4, 107) - 0.5) * 0.14,
        );
        let (wu, wv) = (u + w.0, v + w.1);
        // ragged rafts, holes of many sizes punched through the mat, fine bubbles
        let dens = fbm(wu, wv, 4.0, 6, 113);
        let hole_a = ss(
            0.28,
            0.12,
            worley(wu, wv, 7.0, 127) + (fbm(u, v, 16.0, 3, 131) - 0.5) * 0.25,
        );
        let hole_b = ss(
            0.30,
            0.16,
            worley(wu + 0.17, wv + 0.17, 19.0, 137) + (fbm(u, v, 32.0, 2, 139) - 0.5) * 0.3,
        );
        let hole_c = ss(0.32, 0.18, worley(wu + 0.61, wv + 0.61, 47.0, 149));
        let holes = (hole_a * 0.9 + hole_b * 0.7 + hole_c * 0.45).clamp(0.0, 1.0);
        let bub = ss(0.24, 0.08, worley(u + 0.33, v + 0.33, 140.0, 151)) * 0.8
            + ss(0.2, 0.05, worley(u + 0.71, v + 0.71, 260.0, 157)) * 0.5;
        let streak = fbm(u + w.0 * 2.0, v + w.1 * 2.0, 12.0, 4, 163);
        let foam = (dens * 1.35 - holes * 0.55 + bub * 0.08).clamp(0.0, 1.0);
        let mottle = fbm(u, v, 2.0, 3, 167);
        let q = |f: f32| (f.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        [q(foam), q(bub), q(mottle), q(streak)]
    };
    let mut data = vec![0u8; N * N * 4];
    std::thread::scope(|s| {
        for (k, rows) in data.chunks_mut(N * 4 * 64).enumerate() {
            let texel = &texel;
            s.spawn(move || {
                for (r, row) in rows.chunks_mut(N * 4).enumerate() {
                    for x in 0..N {
                        row[x * 4..x * 4 + 4].copy_from_slice(&texel(x, k * 64 + r));
                    }
                }
            });
        }
    });
    rgba8_mipped(data, N)
}

/// The camera-following polar grid (Emberfall's `grid.js`): spokes packed toward the front,
/// square-ish cells growing with distance, then a skirt to 30 km. UV.x carries each vertex's
/// spacing so the shader reads the displacement at a mip the grid can hold.
pub fn polar_grid(spokes: usize, d_min: f32, d_max: f32, r_max: f32) -> Mesh {
    use bevy::mesh::{Indices, PrimitiveTopology};
    let angs: Vec<f32> = (0..=spokes)
        .map(|j| {
            let u = -1.0 + 2.0 * j as f32 / spokes as f32;
            std::f32::consts::PI * (0.55 * u + 0.45 * u * u * u)
        })
        .collect();
    let front = std::f32::consts::PI * 0.55 * (2.0 / spokes as f32);
    let mut radii = vec![0.0f32];
    let mut r = 0.0f32;
    while r < r_max {
        r += d_max.min(d_min.max(r * front));
        radii.push(r.min(r_max));
    }
    radii.extend_from_slice(&[6500.0, 10_000.0, 15_000.0, 22_000.0, 30_000.0]);
    let n = radii.len();
    let s = spokes + 1;
    let mut pos = Vec::with_capacity(n * s);
    let mut uv = Vec::with_capacity(n * s);
    for i in 0..n {
        let dr = if i < n - 1 {
            radii[i + 1] - radii[i]
        } else {
            radii[i] - radii[i - 1]
        };
        for j in 0..s {
            let da = if j < s - 1 {
                angs[j + 1] - angs[j]
            } else {
                angs[j] - angs[j - 1]
            };
            pos.push([angs[j].sin() * radii[i], 0.0, -angs[j].cos() * radii[i]]);
            uv.push([dr.max(radii[i] * da.abs()), 0.0]);
        }
    }
    let mut idx = Vec::with_capacity((n - 1) * spokes * 6);
    for i in 0..n - 1 {
        for j in 0..spokes {
            let a = (i * s + j) as u32;
            let b = a + s as u32;
            idx.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    let nor = vec![[0.0, 1.0, 0.0]; pos.len()];
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U32(idx));
    m
}
