//! Nature sounds synthesised live, from noise and a few sine "bubbles" (no recordings, so they
//! ship and export freely): the wind and its gusts, rain, a river's babble, a waterfall's roar,
//! surf, fire and lava, drips in a cave and the sea heard from under water. Each voice is an
//! endless stereo stream (a Bevy `Decodable`), built from random events and slow drifts so it
//! never repeats; the wind and rain follow the weather through `Controls`. The same voices rendered
//! to files (`tools/audio/open/synth_loops.py`) are the loops in `assets/audio` that exports carry.
//!
//! The techniques are the classic ones from procedural audio (A. Farnell, "Designing Sound"):
//! band-passed noise swept by slow drifts for wind, a dense spray of tiny filtered noise ticks
//! for rain, sine bubbles whose pitch rises as they collapse for running water and drips,
//! envelopes of low-passed noise for breaking waves, sparse clustered crackles over a roar for
//! fire.

use std::f32::consts::{FRAC_PI_4, TAU};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bevy::audio::{ChannelCount, Decodable, SampleRate, Source};
use bevy::prelude::*;

/// Sample rate of every voice (Hz).
pub const RATE: u32 = 44_100;
const SR: f32 = RATE as f32;
/// Frames between control updates (filter sweeps, drifts, weather).
const BLOCK: u32 = 32;
const DT: f32 = BLOCK as f32 / SR;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Voice {
    Wind,
    Gusts,
    Rain,
    River,
    Waterfall,
    Surf,
    Fire,
    Drips,
    Underwater,
}

impl Voice {
    pub const ALL: [Voice; 9] = [
        Voice::Wind,
        Voice::Gusts,
        Voice::Rain,
        Voice::River,
        Voice::Waterfall,
        Voice::Surf,
        Voice::Fire,
        Voice::Drips,
        Voice::Underwater,
    ];

    /// The ambience stem the voice plays as: the names the soundscapes, water and weather have
    /// always asked for, and the loop file of that name (`assets/audio/…`) that exports carry.
    pub fn stem(self) -> &'static str {
        match self {
            Voice::Wind => "amb_wind_0",
            Voice::Gusts => "amb_near_gusts_0",
            Voice::Rain => "amb_storm_0",
            Voice::River => "amb_water_0",
            Voice::Waterfall => "amb_near_waterfall_0",
            Voice::Surf => "amb_near_open_sea_0",
            Voice::Fire => "amb_fire_0",
            Voice::Drips => "amb_near_drips_0",
            Voice::Underwater => "underwater_0",
        }
    }

    pub fn from_stem(stem: &str) -> Option<Voice> {
        Self::ALL.into_iter().find(|v| v.stem() == stem)
    }

    /// Output level, matched to the loudness of the recorded loops each voice took over (so the
    /// soundscape mixes keep their balance): measured with `render` (EBU R128, 30 s).
    fn gain(self) -> f32 {
        match self {
            Voice::Wind => 0.603,
            Voice::Gusts => 0.646,
            Voice::Rain => 0.462,
            Voice::River => 0.367,
            Voice::Waterfall => 0.295,
            Voice::Surf => 0.184,
            Voice::Fire => 0.088,
            Voice::Drips => 0.624,
            Voice::Underwater => 0.074,
        }
    }

    fn generator(self, seed: u32) -> Box<dyn Generator> {
        let rng = Rng::new(seed);
        match self {
            Voice::Wind => Box::new(Wind::new(rng, false)),
            Voice::Gusts => Box::new(Wind::new(rng, true)),
            Voice::Rain => Box::new(Rain::new(rng)),
            Voice::River => Box::new(Water::new(rng, false)),
            Voice::Waterfall => Box::new(Water::new(rng, true)),
            Voice::Surf => Box::new(Surf::new(rng)),
            Voice::Fire => Box::new(Fire::new(rng)),
            Voice::Drips => Box::new(Drips::new(rng)),
            Voice::Underwater => Box::new(Underwater::new(rng)),
        }
    }
}

/// What the voices follow, written by `audio` every frame and read on the audio thread.
pub struct Controls {
    /// how hard the wind heard blows, 0 (still) ..1 (a gale)
    wind: AtomicU32,
    /// how hard it rains, 0..1
    rain: AtomicU32,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            wind: AtomicU32::new(0.3f32.to_bits()),
            rain: AtomicU32::new(0.5f32.to_bits()),
        }
    }
}

impl Controls {
    pub fn set(&self, wind: f32, rain: f32) {
        self.wind
            .store(wind.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.rain
            .store(rain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> Ctl {
        Ctl {
            wind: f32::from_bits(self.wind.load(Ordering::Relaxed)),
            rain: f32::from_bits(self.rain.load(Ordering::Relaxed)),
        }
    }
}

#[derive(Clone, Copy)]
struct Ctl {
    wind: f32,
    rain: f32,
}

/// A synthesised voice as an asset: every playback starts its own endless stream.
#[derive(Asset, TypePath)]
pub struct Synth {
    pub voice: Voice,
    pub controls: Arc<Controls>,
}

impl Decodable for Synth {
    type Decoder = Stream;

    fn decoder(&self) -> Stream {
        Stream::new(self.voice, self.controls.clone(), fastrand::u32(..))
    }
}

/// The stream a voice plays: interleaved stereo at `RATE`, forever (play it with
/// `PlaybackMode::Once`: looping would buffer it).
pub struct Stream {
    generator: Box<dyn Generator>,
    controls: Arc<Controls>,
    gain: f32,
    right: Option<f32>,
    until_control: u32,
}

impl Stream {
    fn new(voice: Voice, controls: Arc<Controls>, seed: u32) -> Self {
        Self {
            generator: voice.generator(seed),
            controls,
            gain: voice.gain(),
            right: None,
            until_control: 0,
        }
    }
}

impl Iterator for Stream {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.right.take() {
            return Some(r);
        }
        if self.until_control == 0 {
            self.until_control = BLOCK;
            self.generator.control(self.controls.get());
        }
        self.until_control -= 1;
        let [l, r] = self.generator.frame();
        self.right = Some(soft_clip(r * self.gain));
        Some(soft_clip(l * self.gain))
    }
}

impl Source for Stream {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).unwrap()
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(RATE).unwrap()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// `seconds` of a voice (interleaved stereo at `RATE`) under fixed weather, from `seed` (the loop
/// files: `tools/audio/open/synth_loops.py`).
#[cfg(test)]
fn render(voice: Voice, seconds: f32, wind: f32, rain: f32, seed: u32) -> Vec<f32> {
    let controls = Arc::new(Controls::default());
    controls.set(wind, rain);
    let n = (seconds * SR) as usize * 2;
    Stream::new(voice, controls, seed).take(n).collect()
}

/// Transparent below 0.8, then rounds off smoothly towards ±1 (a safety net, never a sound).
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.8 {
        x
    } else {
        let over = a - 0.8;
        (0.8 + 0.2 * over / (0.2 + over)).copysign(x)
    }
}

trait Generator: Send {
    /// Once per `BLOCK` frames: follow the weather, move the drifts, sweep the filters.
    fn control(&mut self, ctl: Ctl);
    /// One stereo frame.
    fn frame(&mut self) -> [f32; 2];
}

// ------------------------------------------------------------------ building blocks

/// xorshift32: fast, plenty random for noise.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9) | 1)
    }
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// 0..1
    fn f(&mut self) -> f32 {
        (self.next() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
    /// -1..1
    fn bi(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
    /// Spread evenly in pitch (for frequencies).
    fn log_range(&mut self, a: f32, b: f32) -> f32 {
        a * (b / a).powf(self.f())
    }
    fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }
}

/// Pink noise (−3 dB/octave, Paul Kellet's economy filter) from white.
#[derive(Default, Clone, Copy)]
struct Pink([f32; 3]);

impl Pink {
    fn next(&mut self, white: f32) -> f32 {
        let b = &mut self.0;
        b[0] = 0.99765 * b[0] + white * 0.099_046;
        b[1] = 0.963 * b[1] + white * 0.296_516_4;
        b[2] = 0.57 * b[2] + white * 1.052_691_3;
        (b[0] + b[1] + b[2] + white * 0.1848) * 0.3
    }
}

/// Brown noise (−6 dB/octave): leaky integrated white.
#[derive(Default, Clone, Copy)]
struct Brown(f32);

impl Brown {
    fn next(&mut self, white: f32) -> f32 {
        self.0 = 0.997 * self.0 + 0.04 * white;
        self.0 * 3.0
    }
}

/// One-pole low-pass.
#[derive(Default, Clone, Copy)]
struct OnePole {
    a: f32,
    y: f32,
}

impl OnePole {
    fn new(fc: f32) -> Self {
        let mut f = Self::default();
        f.set(fc);
        f
    }
    fn set(&mut self, fc: f32) {
        self.a = 1.0 - (-TAU * fc / SR).exp();
    }
    fn run(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

#[derive(Clone, Copy)]
enum Kind {
    LowPass,
    HighPass,
    /// band-pass with 0 dB at the centre
    BandPass,
}

/// RBJ biquad (transposed direct form II).
#[derive(Default, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(kind: Kind, fc: f32, q: f32) -> Self {
        let mut f = Self::default();
        f.set(kind, fc, q);
        f
    }
    fn set(&mut self, kind: Kind, fc: f32, q: f32) {
        let w = TAU * fc.clamp(10.0, SR * 0.45) / SR;
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q.max(0.05));
        let (b0, b1, b2) = match kind {
            Kind::LowPass => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0),
            Kind::HighPass => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0),
            Kind::BandPass => (alpha, 0.0, -alpha),
        };
        let a0 = 1.0 + alpha;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - alpha) / a0;
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// A value wandering between `lo` and `hi`: a new random goal every `min_t..max_t` seconds,
/// glided towards with time constant `tau` (seconds). Stepped at control rate.
struct Drift {
    value: f32,
    goal: f32,
    timer: f32,
    lo: f32,
    hi: f32,
    min_t: f32,
    max_t: f32,
    k: f32,
}

impl Drift {
    fn new(rng: &mut Rng, lo: f32, hi: f32, min_t: f32, max_t: f32, tau: f32) -> Self {
        let value = rng.range(lo, hi);
        Self {
            value,
            goal: value,
            timer: 0.0,
            lo,
            hi,
            min_t,
            max_t,
            k: 1.0 - (-DT / tau).exp(),
        }
    }
    fn step(&mut self, rng: &mut Rng) -> f32 {
        self.timer -= DT;
        if self.timer <= 0.0 {
            self.goal = rng.range(self.lo, self.hi);
            self.timer = rng.range(self.min_t, self.max_t);
        }
        self.value += (self.goal - self.value) * self.k;
        self.value
    }
}

/// Follow `goal` with time constant `tau` seconds (at control rate).
fn ease(value: &mut f32, goal: f32, tau: f32) {
    *value += (goal - *value) * (1.0 - (-DT / tau).exp());
}

/// Equal-power stereo gains for `pan` in -1 (left) ..1 (right).
fn pan(p: f32) -> [f32; 2] {
    let a = (p.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    [a.cos(), a.sin()]
}

/// Per-sample multiplier for an exponential decay with time constant `tau` seconds.
fn decay(tau: f32) -> f32 {
    (-1.0 / (tau * SR)).exp()
}

/// Per-sample frequency multiplier that changes pitch by `ratio` over `seconds`.
fn glide(ratio: f32, seconds: f32) -> f32 {
    (ratio.ln() / (seconds * SR)).exp()
}

/// A short sound event: a sine "bubble" (pitch gliding) or a burst of band-passed noise, under an
/// exponential decay, panned.
#[derive(Clone, Copy)]
struct Grain {
    amp: f32,
    decay: f32,
    phase: f32,
    /// cycles per sample (sine grains)
    freq: f32,
    glide: f32,
    /// noise grains are filtered white; sine grains have no filter
    filter: Option<Biquad>,
    gains: [f32; 2],
}

impl Grain {
    fn sine(hz: f32, glide: f32, tau: f32, amp: f32, pan_to: f32) -> Self {
        Self {
            amp,
            decay: decay(tau),
            phase: 0.0,
            freq: hz / SR,
            glide,
            filter: None,
            gains: pan(pan_to),
        }
    }
    fn noise(hz: f32, q: f32, tau: f32, amp: f32, pan_to: f32) -> Self {
        Self {
            amp,
            decay: decay(tau),
            phase: 0.0,
            freq: 0.0,
            glide: 1.0,
            filter: Some(Biquad::new(Kind::BandPass, hz, q)),
            gains: pan(pan_to),
        }
    }
}

/// The grains sounding at once (a fixed pool: the quietest makes way when it's full), plus ones
/// scheduled to start later (clusters of crackles, trains of bubbles).
struct Grains {
    live: Vec<Grain>,
    cap: usize,
    later: Vec<(u32, Grain)>,
}

impl Grains {
    fn new(cap: usize) -> Self {
        Self {
            live: Vec::with_capacity(cap),
            cap,
            later: Vec::new(),
        }
    }
    fn add(&mut self, g: Grain) {
        if self.live.len() < self.cap {
            self.live.push(g);
        } else if let Some((i, _)) = self
            .live
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.amp.total_cmp(&b.1.amp))
            .filter(|(_, q)| q.amp < g.amp)
        {
            self.live[i] = g;
        }
    }
    /// Start `g` in `seconds`.
    fn add_later(&mut self, seconds: f32, g: Grain) {
        self.later.push(((seconds * SR) as u32, g));
    }
    fn frame(&mut self, rng: &mut Rng) -> [f32; 2] {
        if !self.later.is_empty() {
            let mut i = 0;
            while i < self.later.len() {
                if self.later[i].0 == 0 {
                    let (_, g) = self.later.swap_remove(i);
                    self.add(g);
                } else {
                    self.later[i].0 -= 1;
                    i += 1;
                }
            }
        }
        let mut out = [0.0; 2];
        let mut i = 0;
        while i < self.live.len() {
            let g = &mut self.live[i];
            let s = match &mut g.filter {
                Some(f) => f.run(rng.bi()),
                None => {
                    g.phase = (g.phase + g.freq).fract();
                    g.freq *= g.glide;
                    (g.phase * TAU).sin()
                }
            } * g.amp;
            out[0] += s * g.gains[0];
            out[1] += s * g.gains[1];
            g.amp *= g.decay;
            if g.amp < 2e-4 {
                self.live.swap_remove(i);
            } else {
                i += 1;
            }
        }
        out
    }
}

/// A small Schroeder/Freeverb-style reverb (four damped combs, two all-passes per channel).
struct Reverb {
    combs: [[Comb; 4]; 2],
    passes: [[AllPass; 2]; 2],
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    feedback: f32,
    damp: f32,
    store: f32,
}

impl Comb {
    fn run(&mut self, x: f32) -> f32 {
        let y = self.buf[self.i];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.i] = x + self.store * self.feedback;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

struct AllPass {
    buf: Vec<f32>,
    i: usize,
}

impl AllPass {
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        b - x
    }
}

impl Reverb {
    /// `size` scales the delay lines (1 = a large room), `feedback` sets the tail.
    fn new(size: f32, feedback: f32, damp: f32) -> Self {
        let comb = |n: usize| Comb {
            buf: vec![0.0; (n as f32 * size) as usize],
            i: 0,
            feedback,
            damp,
            store: 0.0,
        };
        let pass = |n: usize| AllPass {
            buf: vec![0.0; (n as f32 * size) as usize],
            i: 0,
        };
        // Freeverb's tunings at 44.1 kHz, the right channel spread by 23 samples
        let side = |s: usize| {
            (
                [
                    comb(1116 + s),
                    comb(1188 + s),
                    comb(1277 + s),
                    comb(1356 + s),
                ],
                [pass(556 + s), pass(441 + s)],
            )
        };
        let (l, lp) = side(0);
        let (r, rp) = side(23);
        Self {
            combs: [l, r],
            passes: [lp, rp],
        }
    }
    fn run(&mut self, x: [f32; 2]) -> [f32; 2] {
        let input = (x[0] + x[1]) * 0.5 * 0.06;
        let mut out = [0.0; 2];
        for c in 0..2 {
            let mut y = 0.0;
            for comb in &mut self.combs[c] {
                y += comb.run(input);
            }
            for p in &mut self.passes[c] {
                y = p.run(y);
            }
            out[c] = y;
        }
        out
    }
}

// ------------------------------------------------------------------ voices

/// Wind: pink noise through a band-pass whose centre and width drift (the air's "voice" rising as
/// it blows harder), a low rumble, and a faint whistle in strong wind. As gusts, the same under
/// an envelope of swells that rise over a second or two, sweep the band up and pan across.
struct Wind {
    rng: Rng,
    gusty: bool,
    pink: [Pink; 2],
    brown: [Brown; 2],
    body: [Biquad; 2],
    whistle: [Biquad; 2],
    rumble: [OnePole; 2],
    top: [OnePole; 2],
    strength: f32,
    swell: Drift,
    tone: Drift,
    whistle_gain: f32,
    level: f32,
    gains: [f32; 2],
    gust: Gust,
}

/// One gust at a time: rest, rise, fall.
struct Gust {
    t: f32,
    rise: f32,
    fall: f32,
    rest: f32,
    peak: f32,
    pan: f32,
    env: f32,
}

impl Gust {
    fn start(&mut self, rng: &mut Rng) {
        self.t = 0.0;
        self.rise = rng.range(0.8, 2.4);
        self.fall = rng.range(1.5, 4.0);
        self.rest = rng.range(0.5, 5.0);
        self.peak = rng.range(0.45, 1.0);
        self.pan = rng.range(-0.6, 0.6);
    }
    fn step(&mut self, rng: &mut Rng) -> f32 {
        self.t += DT;
        let (r, f) = (self.rise, self.fall);
        self.env = if self.t < r {
            let x = self.t / r;
            x * x * (3.0 - 2.0 * x)
        } else if self.t < r + f {
            let x = 1.0 - (self.t - r) / f;
            x * x
        } else {
            0.0
        } * self.peak;
        if self.t > r + f + self.rest {
            self.start(rng);
        }
        self.env
    }
}

impl Wind {
    fn new(mut rng: Rng, gusty: bool) -> Self {
        let swell = Drift::new(&mut rng, 0.4, 1.0, 1.5, 6.0, 1.2);
        let tone = Drift::new(&mut rng, 0.0, 1.0, 2.0, 7.0, 1.5);
        let mut gust = Gust {
            t: 0.0,
            rise: 1.0,
            fall: 1.0,
            rest: 0.0,
            peak: 0.0,
            pan: 0.0,
            env: 0.0,
        };
        gust.start(&mut rng);
        // start part-way into the first gust rather than from silence
        gust.t = gust.rise * 0.5;
        Self {
            rng,
            gusty,
            pink: [Pink::default(); 2],
            brown: [Brown::default(); 2],
            body: [Biquad::new(Kind::BandPass, 400.0, 0.8); 2],
            whistle: [Biquad::new(Kind::BandPass, 900.0, 18.0); 2],
            rumble: [OnePole::new(110.0); 2],
            top: [OnePole::new(if gusty { 3000.0 } else { 1500.0 }); 2],
            strength: 0.3,
            swell,
            tone,
            whistle_gain: 0.0,
            level: 1.0,
            gains: [1.0; 2],
            gust,
        }
    }
}

impl Generator for Wind {
    fn control(&mut self, ctl: Ctl) {
        ease(&mut self.strength, ctl.wind, 1.5);
        let s = self.strength;
        let swell = self.swell.step(&mut self.rng);
        let t = self.tone.step(&mut self.rng);
        let (centre, q);
        if self.gusty {
            let g = self.gust.step(&mut self.rng);
            centre = 450.0 + (600.0 + 900.0 * s) * g * (0.7 + 0.3 * t);
            q = 1.2 + 0.8 * t;
            self.level = 0.22 + 0.9 * g;
            let p = pan(self.gust.pan);
            self.gains = [p[0] * 1.414, p[1] * 1.414];
        } else {
            centre = 170.0 + 850.0 * s.powf(0.9) * (0.65 + 0.7 * t) * (0.85 + 0.3 * swell);
            q = 0.65 + 0.7 * t;
            self.level = swell * (0.45 + 0.55 * s);
        }
        for c in 0..2 {
            let spread = 1.0 + 0.06 * c as f32;
            self.body[c].set(Kind::BandPass, centre * spread, q);
            self.whistle[c].set(
                Kind::BandPass,
                (420.0 + 1300.0 * s * (0.4 + 0.6 * t)) * spread,
                22.0,
            );
        }
        self.whistle_gain = 0.9 * (s * s) * t;
    }

    fn frame(&mut self) -> [f32; 2] {
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let n = self.pink[c].next(self.rng.bi());
            let body = self.top[c].run(self.body[c].run(n)) * 2.6;
            let whistle = self.whistle[c].run(self.rng.bi()) * self.whistle_gain;
            let rumble = self.rumble[c].run(self.brown[c].next(self.rng.bi()))
                * (0.06 + 0.15 * self.strength)
                * if self.gusty { 0.3 } else { 1.0 };
            *o = (body + whistle + rumble) * self.level * self.gains[c];
        }
        out
    }
}

/// Rain: a bed of high-passed pink noise (the far hiss of countless drops) and a spray of tiny
/// band-passed noise ticks (drops landing near), with now and then a ringing plink of a big drop
/// on something hard; the harder it rains, the denser the spray.
struct Rain {
    rng: Rng,
    pink: [Pink; 2],
    high: [Biquad; 2],
    low: [OnePole; 2],
    grains: Grains,
    amount: f32,
    per_frame: f32,
    bed: f32,
}

impl Rain {
    fn new(rng: Rng) -> Self {
        Self {
            rng,
            pink: [Pink::default(); 2],
            high: [Biquad::new(Kind::HighPass, 800.0, 0.7); 2],
            low: [OnePole::new(7500.0); 2],
            grains: Grains::new(48),
            amount: 0.5,
            per_frame: 0.0,
            bed: 0.5,
        }
    }
}

impl Generator for Rain {
    fn control(&mut self, ctl: Ctl) {
        // the loop only plays while it rains: a drizzle at least
        ease(&mut self.amount, ctl.rain.max(0.12), 2.0);
        let r = self.amount;
        self.per_frame = (80.0 + 1500.0 * r.powf(1.2)) / SR;
        self.bed = 0.35 + 0.9 * r;
    }

    fn frame(&mut self) -> [f32; 2] {
        let rng = &mut self.rng;
        if rng.f() < self.per_frame {
            let p = rng.bi();
            if rng.chance(0.05) {
                let hz = rng.log_range(1500.0, 4200.0);
                self.grains.add(Grain::sine(
                    hz,
                    glide(rng.range(1.1, 1.5), 0.02),
                    rng.range(0.006, 0.02),
                    rng.range(0.03, 0.12),
                    p,
                ));
            } else {
                let loud = rng.f();
                self.grains.add(Grain::noise(
                    rng.log_range(1800.0, 9000.0),
                    rng.range(1.0, 3.0),
                    rng.range(0.0008, 0.004),
                    0.05 + 0.9 * loud * loud,
                    p,
                ));
            }
        }
        let drops = self.grains.frame(rng);
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let n = self.pink[c].next(rng.bi());
            let bed = self.low[c].run(self.high[c].run(n)) * self.bed;
            *o = bed + drops[c] * 0.6;
        }
        out
    }
}

/// Running water. A river babbles: a stream of sine bubbles whose pitch rises as they collapse
/// (the bigger, the lower and louder) over a soft band of flowing noise. A waterfall roars:
/// broadband noise and a deep rumble carry it, with a fizz of small bubbles on top.
struct Water {
    rng: Rng,
    fall: bool,
    pink: [Pink; 2],
    brown: [Brown; 2],
    band: [Biquad; 2],
    high: [Biquad; 2],
    rumble: [OnePole; 2],
    grains: Grains,
    per_frame: f32,
    swell: [Drift; 2],
    level: [f32; 2],
}

impl Water {
    fn new(mut rng: Rng, fall: bool) -> Self {
        let swell = [
            Drift::new(&mut rng, 0.75, 1.0, 1.0, 4.0, 1.0),
            Drift::new(&mut rng, 0.75, 1.0, 1.0, 4.0, 1.0),
        ];
        let (band, rumble) = if fall {
            (Biquad::new(Kind::LowPass, 9000.0, 0.6), OnePole::new(140.0))
        } else {
            (Biquad::new(Kind::BandPass, 3500.0, 0.45), OnePole::new(260.0))
        };
        Self {
            rng,
            fall,
            pink: [Pink::default(); 2],
            brown: [Brown::default(); 2],
            band: [band; 2],
            high: [Biquad::new(Kind::HighPass, if fall { 350.0 } else { 400.0 }, 0.7); 2],
            rumble: [rumble; 2],
            grains: Grains::new(40),
            per_frame: if fall { 500.0 } else { 170.0 } / SR,
            swell,
            level: [1.0; 2],
        }
    }
}

impl Generator for Water {
    fn control(&mut self, _: Ctl) {
        for c in 0..2 {
            self.level[c] = self.swell[c].step(&mut self.rng);
        }
    }

    fn frame(&mut self) -> [f32; 2] {
        let rng = &mut self.rng;
        if rng.f() < self.per_frame {
            let hz = if self.fall {
                rng.log_range(600.0, 2500.0)
            } else {
                rng.log_range(500.0, 3200.0)
            };
            let tau = rng.range(0.004, 0.014);
            // bigger bubbles ring lower and louder
            let amp = rng.range(0.25, 0.7) * (1200.0 / hz).sqrt().min(1.4);
            self.grains.add(Grain::sine(
                hz,
                glide(rng.range(1.5, 2.6), tau * 4.0),
                tau,
                amp * if self.fall { 0.35 } else { 1.0 },
                rng.bi() * 0.8,
            ));
        }
        let bubbles = self.grains.frame(rng);
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let w = rng.bi();
            let n = if self.fall {
                self.pink[c].next(w) * 0.6 + rng.bi() * 0.5
            } else {
                self.pink[c].next(w) * 0.8 + rng.bi() * 0.3
            };
            let flow = self.high[c].run(self.band[c].run(n));
            let rumble = self.rumble[c].run(self.brown[c].next(rng.bi()));
            *o = if self.fall {
                (flow * 1.3 + rumble * 0.04) * self.level[c] + bubbles[c]
            } else {
                flow * self.level[c] + bubbles[c] * 0.5
            };
        }
        out
    }
}

/// Surf: waves every few seconds, each a swell of noise whose brightness follows its envelope
/// (the crash is bright, the wash darkens as it spreads) with a fizz of bubbles as it draws back,
/// over the sea's steady low roar.
struct Surf {
    rng: Rng,
    waves: Vec<Wave>,
    next_wave: f32,
    pink: [Pink; 2],
    brown: [Brown; 2],
    wash: [Biquad; 2],
    roar: [OnePole; 2],
    env: [f32; 2],
    grains: Grains,
    fizz_per_frame: f32,
}

struct Wave {
    t: f32,
    rise: f32,
    tau: f32,
    peak: f32,
    gains: [f32; 2],
}

impl Surf {
    fn new(mut rng: Rng) -> Self {
        // one wave already washing up
        let waves = vec![Wave {
            t: 0.8,
            rise: 0.8,
            tau: 2.4,
            peak: 0.8,
            gains: pan(rng.range(-0.3, 0.3)),
        }];
        Self {
            next_wave: rng.range(2.0, 5.0),
            rng,
            waves,
            pink: [Pink::default(); 2],
            brown: [Brown::default(); 2],
            wash: [Biquad::new(Kind::LowPass, 1000.0, 0.7); 2],
            roar: [OnePole::new(220.0); 2],
            env: [0.0; 2],
            grains: Grains::new(32),
            fizz_per_frame: 0.0,
        }
    }
}

impl Generator for Surf {
    fn control(&mut self, _: Ctl) {
        let rng = &mut self.rng;
        self.next_wave -= DT;
        if self.next_wave <= 0.0 {
            self.next_wave = rng.range(4.5, 9.5);
            self.waves.push(Wave {
                t: 0.0,
                rise: rng.range(0.5, 1.2),
                tau: rng.range(1.6, 3.4),
                peak: rng.range(0.55, 1.0),
                gains: pan(rng.range(-0.5, 0.5)),
            });
        }
        let mut env = [0.0f32; 2];
        let mut draw = 0.0f32;
        for w in &mut self.waves {
            w.t += DT;
            let e = if w.t < w.rise {
                let x = w.t / w.rise;
                x * x
            } else {
                let after = (w.t - w.rise) / w.tau;
                draw += (after * 2.0).min(1.0) * (-after).exp();
                (-after).exp()
            } * w.peak;
            env[0] += e * w.gains[0] * 1.414;
            env[1] += e * w.gains[1] * 1.414;
        }
        self.waves.retain(|w| w.t < w.rise + 6.0 * w.tau);
        for c in 0..2 {
            self.env[c] = env[c];
            self.wash[c].set(
                Kind::LowPass,
                600.0 + 6000.0 * env[c].min(1.2).powf(1.3),
                0.7,
            );
        }
        self.fizz_per_frame = 260.0 * draw.min(1.0) / SR;
    }

    fn frame(&mut self) -> [f32; 2] {
        let rng = &mut self.rng;
        if rng.f() < self.fizz_per_frame {
            let hz = rng.log_range(900.0, 3500.0);
            let tau = rng.range(0.002, 0.008);
            self.grains.add(Grain::sine(
                hz,
                glide(rng.range(1.3, 2.0), tau * 4.0),
                tau,
                rng.range(0.03, 0.12),
                rng.bi() * 0.7,
            ));
        }
        let fizz = self.grains.frame(rng);
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let n = self.pink[c].next(rng.bi());
            let wash = self.wash[c].run(n) * self.env[c] * 2.2;
            let roar = self.roar[c].run(self.brown[c].next(rng.bi())) * 0.25;
            *o = wash + roar + fizz[c];
        }
        out
    }
}

/// Fire (and lava, which plays it too): a low breathing roar, a faint hiss, and crackles: sharp
/// band-passed clicks that come in clusters, now and then a deeper pop, and the slow glop of a
/// bubble of molten rock bursting.
struct Fire {
    rng: Rng,
    brown: [Brown; 2],
    roar: [Biquad; 2],
    roar_high: [Biquad; 2],
    hiss: [Biquad; 2],
    grains: Grains,
    breath: [Drift; 2],
    level: [f32; 2],
    crackle_rate: Drift,
    per_frame: f32,
}

impl Fire {
    fn new(mut rng: Rng) -> Self {
        let breath = [
            Drift::new(&mut rng, 0.5, 1.0, 0.4, 2.0, 0.35),
            Drift::new(&mut rng, 0.5, 1.0, 0.4, 2.0, 0.35),
        ];
        let crackle_rate = Drift::new(&mut rng, 4.0, 22.0, 1.0, 4.0, 0.8);
        Self {
            rng,
            brown: [Brown::default(); 2],
            roar: [Biquad::new(Kind::LowPass, 180.0, 0.7); 2],
            roar_high: [Biquad::new(Kind::HighPass, 100.0, 0.7); 2],
            hiss: [Biquad::new(Kind::HighPass, 4000.0, 0.7); 2],
            grains: Grains::new(32),
            breath,
            level: [1.0; 2],
            crackle_rate,
            per_frame: 0.0,
        }
    }

    fn crackle(&mut self) {
        let rng = &mut self.rng;
        let p = rng.bi() * 0.8;
        let n = 1 + (rng.f().powi(3) * 6.0) as usize;
        for k in 0..n {
            let g = if rng.chance(0.12) {
                Grain::noise(
                    rng.log_range(500.0, 1400.0),
                    2.5,
                    rng.range(0.004, 0.01),
                    rng.range(0.3, 0.6),
                    p,
                )
            } else {
                Grain::noise(
                    rng.log_range(4000.0, 11000.0),
                    0.8,
                    rng.range(0.0003, 0.0015),
                    rng.range(0.4, 1.1),
                    p + rng.bi() * 0.1,
                )
            };
            if k == 0 {
                self.grains.add(g);
            } else {
                self.grains.add_later(rng.range(0.004, 0.06) * k as f32, g);
            }
        }
    }
}

impl Generator for Fire {
    fn control(&mut self, _: Ctl) {
        for c in 0..2 {
            self.level[c] = self.breath[c].step(&mut self.rng);
        }
        self.per_frame = self.crackle_rate.step(&mut self.rng) / SR;
    }

    fn frame(&mut self) -> [f32; 2] {
        if self.rng.f() < self.per_frame {
            self.crackle();
        }
        // a bubble of lava bursting: a low falling bloop
        if self.rng.f() < 0.4 / SR {
            let rng = &mut self.rng;
            self.grains.add(Grain::sine(
                rng.range(55.0, 110.0),
                glide(0.6, 0.15),
                rng.range(0.08, 0.15),
                rng.range(0.35, 0.6),
                rng.bi() * 0.6,
            ));
        }
        let crackles = self.grains.frame(&mut self.rng);
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let roar = self.roar_high[c].run(self.roar[c].run(self.brown[c].next(self.rng.bi()))) * 0.8;
            let hiss = self.hiss[c].run(self.rng.bi()) * 0.45;
            *o = (roar + hiss) * self.level[c] + crackles[c] * 0.9;
        }
        out
    }
}

/// Drips in a cave: single drops falling now and then (a sine whose pitch leaps up as the drop's
/// cavity collapses), sometimes a second right after, in a big stone reverb.
struct Drips {
    rng: Rng,
    grains: Grains,
    rate: Drift,
    per_frame: f32,
    reverb: Reverb,
}

impl Drips {
    fn new(mut rng: Rng) -> Self {
        let rate = Drift::new(&mut rng, 0.5, 1.6, 2.0, 8.0, 1.0);
        Self {
            rng,
            grains: Grains::new(12),
            rate,
            per_frame: 0.0,
            reverb: Reverb::new(1.5, 0.8, 0.4),
        }
    }

    fn drip(&mut self, amp: f32, p: f32) -> Grain {
        let rng = &mut self.rng;
        Grain::sine(
            rng.log_range(900.0, 2600.0),
            glide(rng.range(2.0, 3.0), 0.03),
            rng.range(0.012, 0.035),
            amp,
            p,
        )
    }
}

impl Generator for Drips {
    fn control(&mut self, _: Ctl) {
        self.per_frame = self.rate.step(&mut self.rng) / SR;
    }

    fn frame(&mut self) -> [f32; 2] {
        if self.rng.f() < self.per_frame {
            let (amp, p) = (self.rng.range(0.25, 0.9), self.rng.bi() * 0.8);
            let g = self.drip(amp, p);
            self.grains.add(g);
            if self.rng.chance(0.15) {
                let t = self.rng.range(0.06, 0.14);
                let g = self.drip(amp * 0.6, p);
                self.grains.add_later(t, g);
            }
        }
        let dry = self.grains.frame(&mut self.rng);
        let wet = self.reverb.run(dry);
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            *o = dry[c] * 0.7 + wet[c] * 0.3;
        }
        out
    }
}

/// The sea heard from under the surface: a dark, slowly breathing rumble, a sub-bass swell and
/// trains of bubbles rising past.
struct Underwater {
    rng: Rng,
    brown: [Brown; 2],
    sub_brown: [Brown; 2],
    body: [Biquad; 2],
    sub: [OnePole; 2],
    breath: Drift,
    level: f32,
    grains: Grains,
}

impl Underwater {
    fn new(mut rng: Rng) -> Self {
        let breath = Drift::new(&mut rng, 0.55, 1.0, 2.0, 7.0, 2.0);
        Self {
            rng,
            brown: [Brown::default(); 2],
            sub_brown: [Brown::default(); 2],
            body: [Biquad::new(Kind::LowPass, 300.0, 0.7); 2],
            sub: [OnePole::new(55.0); 2],
            breath,
            level: 1.0,
            grains: Grains::new(16),
        }
    }
}

impl Generator for Underwater {
    fn control(&mut self, _: Ctl) {
        self.level = self.breath.step(&mut self.rng);
    }

    fn frame(&mut self) -> [f32; 2] {
        let rng = &mut self.rng;
        // a train of bubbles every couple of seconds
        if rng.f() < 0.45 / SR {
            let n = 3 + (rng.f() * 9.0) as usize;
            let p = rng.bi() * 0.7;
            let mut t = 0.0;
            for _ in 0..n {
                let hz = rng.log_range(250.0, 900.0);
                let tau = rng.range(0.01, 0.035);
                let g = Grain::sine(
                    hz,
                    glide(rng.range(1.3, 1.8), tau * 4.0),
                    tau,
                    rng.range(0.12, 0.35),
                    p + rng.bi() * 0.15,
                );
                self.grains.add_later(t, g);
                t += rng.range(0.025, 0.09);
            }
        }
        let bubbles = self.grains.frame(rng);
        let mut out = [0.0; 2];
        for (c, o) in out.iter_mut().enumerate() {
            let body = self.body[c].run(self.brown[c].next(rng.bi())) * 2.0;
            let sub = self.sub[c].run(self.sub_brown[c].next(rng.bi())) * 0.6;
            *o = (body + sub) * self.level + bubbles[c] * 0.25;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Zero crossings per second of one channel: a rough measure of brightness.
    fn crossings(x: &[f32]) -> f32 {
        let l: Vec<f32> = x.iter().step_by(2).copied().collect();
        let n = l.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        n as f32 / (l.len() as f32 / SR)
    }

    #[test]
    fn every_voice_is_finite_audible_and_never_clips() {
        for v in Voice::ALL {
            let x = render(v, 12.0, 0.4, 0.6, 7);
            assert!(x.iter().all(|s| s.is_finite()), "{v:?}: not finite");
            let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak < 1.0, "{v:?}: peak {peak}");
            let r = rms(&x);
            assert!(r > 0.003, "{v:?}: too quiet ({r})");
            // the two channels differ (stereo, not mono twice) wherever there's sound
            let sounding: Vec<&[f32]> = x
                .chunks(2)
                .filter(|f| f[0].abs() + f[1].abs() > 1e-3)
                .collect();
            let diff = sounding
                .iter()
                .filter(|f| (f[0] - f[1]).abs() > 1e-4)
                .count();
            assert!(diff > sounding.len() / 4, "{v:?}: mono");
            // and it doesn't fade away or blow up over time
            let (a, b) = x.split_at(x.len() / 2);
            let ratio = rms(b) / rms(a);
            assert!((0.33..3.0).contains(&ratio), "{v:?}: level drifts {ratio}");
        }
    }

    #[test]
    fn the_weather_changes_wind_and_rain() {
        // a strong wind is brighter (its band sweeps up) than a breeze
        let breeze = crossings(&render(Voice::Wind, 8.0, 0.1, 0.0, 3));
        let gale = crossings(&render(Voice::Wind, 8.0, 0.9, 0.0, 3));
        assert!(gale > breeze * 1.3, "breeze {breeze} gale {gale}");
        // a downpour is louder than a drizzle
        let drizzle = rms(&render(Voice::Rain, 8.0, 0.0, 0.1, 3));
        let downpour = rms(&render(Voice::Rain, 8.0, 0.0, 1.0, 3));
        assert!(downpour > drizzle * 1.5, "drizzle {drizzle} downpour {downpour}");
    }

    #[test]
    fn streams_never_repeat_and_seeds_differ() {
        let a = render(Voice::River, 2.0, 0.3, 0.5, 1);
        let b = render(Voice::River, 2.0, 0.3, 0.5, 2);
        assert_ne!(a, b);
        // the same seed is the same stream (renders are reproducible)
        assert_eq!(a, render(Voice::River, 2.0, 0.3, 0.5, 1));
    }

    /// Writes 30 s of every voice as WAV to `$SKYFOLD_SYNTH_WAV` (to audition, measure and make the
    /// loops): `SKYFOLD_SYNTH_WAV=/tmp/synth cargo test --release synth::tests::render_wavs -- --ignored`
    #[test]
    #[ignore]
    fn render_wavs() {
        let Ok(dir) = std::env::var("SKYFOLD_SYNTH_WAV") else {
            return;
        };
        let seconds: f32 = std::env::var("SKYFOLD_SYNTH_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30.0);
        std::fs::create_dir_all(&dir).unwrap();
        for v in Voice::ALL {
            let t = std::time::Instant::now();
            // a fresh breeze and steady rain (the loops play under the engine's own levels)
            let x = render(v, seconds, 0.35, 0.6, 11);
            let took = t.elapsed().as_secs_f32();
            let path = std::path::Path::new(&dir).join(format!("{}.wav", v.stem()));
            write_wav(&path, &x);
            println!(
                "{:?}: {:.1} s in {:.3} s ({:.1}% of real time) rms {:.4}",
                v,
                seconds,
                took,
                100.0 * took / seconds,
                rms(&x)
            );
        }
    }

    fn write_wav(path: &std::path::Path, x: &[f32]) {
        let mut b = Vec::with_capacity(44 + x.len() * 2);
        let data = (x.len() * 2) as u32;
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&RATE.to_le_bytes());
        b.extend_from_slice(&(RATE * 4).to_le_bytes());
        b.extend_from_slice(&4u16.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        for s in x {
            b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(path, b).unwrap();
    }
}
