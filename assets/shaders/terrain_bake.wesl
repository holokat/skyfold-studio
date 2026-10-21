// Bakes the slow noise fields of the terrain's ground rules into two maps, one texel per terrain
// vertex (driven by `terrain::bake`): the shading (`terrain.wesl`, `RULE_NOISE_MAPS`) reads them
// instead of evaluating ten value noises per pixel. They only depend on where a spot is, so they
// are baked once per world; the rules themselves (`terrain_rules.wesl`) stay per pixel, where the
// slope, the height and the paint are exact.

import super::noise::{value_noise2, fbm2};
import super::terrain_rules::dry_noise;

struct BakeParams {
    // x: half size, y: cell size, z: vertices per side
    grid: vec4<f32>,
    // texels x0, z0, x1, z1 (inclusive) to bake
    rect: vec4<f32>,
}

@group(0) @binding(0) var<uniform> p: BakeParams;
// macro, mid and dry-patch noise (+0.5), macro tone
@group(0) @binding(1) var out0: texture_storage_2d<rgba8unorm, write>;
// anti-tiling noise, coarse ecotone noise
@group(0) @binding(2) var out1: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn bake(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = vec2<f32>(id.xy) + p.rect.xy;
    if j.x > p.rect.z || j.y > p.rect.w {
        return;
    }
    // the texel's vertex
    let xz = j * p.grid.y - p.grid.x;
    let t = vec2<i32>(j);
    let macro_n = value_noise2(xz * 0.004, 17u);
    let mid_n = value_noise2(xz * 0.05, 29u);
    textureStore(out0, t, vec4<f32>(macro_n, mid_n, dry_noise(xz) + 0.5, fbm2(xz * 0.0018, 3u, 91u)));
    textureStore(out1, t, vec4<f32>(value_noise2(xz * 0.045, 43u), value_noise2(xz * 0.022, 151u), 0.0, 0.0));
}
