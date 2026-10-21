// The surf's clock, shared by the sea surface (water.wesl: breaking bores rolling in) and the
// beach (terrain.wesl: the swash running up the sand and draining back), so a wave's foam line
// crosses the waterline exactly when its swash starts up the sand.

import super::noise::value_noise2;

// Waves arrive every ~7 s (longer in a bigger sea). The arrival time is shifted along the coast by
// smooth noise, so a wave front reaches the shore as a wandering line, never a ruled one, and sets
// of bigger and smaller waves follow each other. Returns (cycle: wave number + phase 0..1, size
// 0.5..1 of this wave).
fn surf_cycle(xz: vec2<f32>, t: f32, hs: f32) -> vec2<f32> {
    let period = 6.5 + 1.6 * clamp(hs, 0.0, 2.5);
    let lag = value_noise2(xz * 0.0105, 211u) * 2.4 + value_noise2(xz * 0.043, 223u) * 0.5;
    let cyc = t / period + lag;
    return vec2<f32>(cyc, surf_size(floor(cyc), xz));
}

// Size (0.5..1) of wave number `n` at `xz`.
fn surf_size(n: f32, xz: vec2<f32>) -> f32 {
    return 0.5 + 0.5 * value_noise2(vec2<f32>(n * 0.61, n * 0.37) + xz * 0.0035, 229u);
}

// How far up the beach (m above the still water) the swash sheet reaches at phase `c` of a wave
// that runs up to `reach`: a quick rush up (a quarter of the cycle), then a slow drain.
fn swash_front(c: f32, reach: f32) -> f32 {
    let up = sin(clamp(c / 0.24, 0.0, 1.0) * 1.5708);
    let down = 1.0 - pow(clamp((c - 0.24) / 0.7, 0.0, 1.0), 0.75);
    return reach * select(down, up, c < 0.24) - 0.04;
}

// Highest run-up (m above the still water) of the waves for a sea of significant height `hs`.
fn run_up(hs: f32) -> f32 {
    return 0.12 + 0.3 * clamp(hs, 0.0, 3.0);
}
