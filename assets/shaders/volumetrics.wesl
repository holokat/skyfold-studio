// Volumetric light, step 1 of 3 (`volumetrics.rs`): light the air in every froxel.
//
// Each cell of the froxel grid takes one jittered sample of the medium — boundary-layer haze and
// weather fog thinning with height, ground mist hugging the terrain, both broken up by drifting
// noise — and of the light scattered towards the eye there: the sun and the moon through their
// shadow cascades (so buildings, trees and ridges cut shafts out of the lit air, and a window
// lets a beam into a dark room), the sky's light from Bevy's sky-view LUT, the artificial ambient
// fill, and the nearest lamps and fires. The result is blended with the last frame's (reprojected)
// so the jitter resolves into smooth light over a few frames.
//
// Output per cell: rgb = in-scattered light per metre (times exposure), a = extinction (1/m).

import bevy_pbr::atmosphere::{
    bindings::{view, atmosphere, lights},
    functions::{sample_transmittance_lut, sample_sky_view_lut, get_view_position},
};
import super::volumetrics_types::{Volumetrics, slice_depth, depth_slice, hg};

@group(1) @binding(0) var<uniform> vol: Volumetrics;
@group(1) @binding(1) var shadow_tex: texture_depth_2d_array;
@group(1) @binding(2) var shadow_smp: sampler_comparison;
@group(1) @binding(3) var history_tex: texture_3d<f32>;
@group(1) @binding(4) var clamp_smp: sampler;
@group(1) @binding(5) var noise_tex: texture_3d<f32>;
@group(1) @binding(6) var repeat_smp: sampler;
@group(1) @binding(7) var height_tex: texture_2d<f32>;
@group(1) @binding(8) var out_tex: texture_storage_3d<rgba16float, write>;

const SHADOWS_ENABLED: u32 = 1u;
// Light scattered many times inside fog and mist: single scattering alone leaves a fog bank dim
// and grey, where real fog under the sun glows white. An even glow (per steradian) in proportion
// to the dense layers' share, softened by shadows (the lit fog round a shadow fills it in).
const MULTI_SCATTER: f32 = 0.22;
const MAX_KEYS: u32 = 2u;

// Computed once per workgroup: the sky's light and the sun's and moon's colour after the
// atmosphere (the same for every cell; the camera's place in the atmosphere decides them).
var<workgroup> wg_sky: vec3<f32>;
var<workgroup> wg_key: array<vec3<f32>, 2>;

// Sky light: the sky-view LUT averaged over the upper hemisphere (zenith + four directions).
fn sky_light(r: f32) -> vec3<f32> {
    let c = 0.906;
    let s = 0.423;
    var sum = sample_sky_view_lut(r, vec3<f32>(0.0, 1.0, 0.0)) * 0.36;
    sum += sample_sky_view_lut(r, vec3<f32>(c, s, 0.0)) * 0.16;
    sum += sample_sky_view_lut(r, vec3<f32>(-c, s, 0.0)) * 0.16;
    sum += sample_sky_view_lut(r, vec3<f32>(0.0, s, c)) * 0.16;
    sum += sample_sky_view_lut(r, vec3<f32>(0.0, s, -c)) * 0.16;
    return sum;
}

// A directional light's colour (illuminance) after the atmosphere at the camera: sunset colours,
// and nothing once the planet is in the way.
fn key_color(i: u32, pa: vec3<f32>) -> vec3<f32> {
    let l = &lights.directional_lights[i];
    let r = length(pa);
    let mu = dot((*l).direction_to_light, pa / r);
    let k = atmosphere.inner_radius / r;
    let mu_h = -sqrt(max(1.0 - k * k, 0.0));
    let lit = smoothstep(mu_h - 0.004, mu_h + 0.012, mu);
    return (*l).color.rgb * sample_transmittance_lut(r, max(mu, mu_h + 0.001)) * lit;
}

// Terrain height (m) under a world position: bilinear over the height texture.
fn ground(xz: vec2<f32>) -> f32 {
    let n = vol.terrain.z;
    let g = clamp((xz + vol.terrain.x) / vol.terrain.y, vec2<f32>(0.0), vec2<f32>(n - 1.001));
    let i = vec2<i32>(floor(g));
    let f = g - floor(g);
    let a = textureLoad(height_tex, i, 0).r;
    let b = textureLoad(height_tex, i + vec2<i32>(1, 0), 0).r;
    let c = textureLoad(height_tex, i + vec2<i32>(0, 1), 0).r;
    let d = textureLoad(height_tex, i + vec2<i32>(1, 1), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// Drifting noise (0..1, mean ~0.5): two octaves of the tiling noise volume.
fn drift_noise(p: vec3<f32>) -> f32 {
    let q = (p + vol.wind.xyz) * vol.shape.x;
    let a = textureSampleLevel(noise_tex, repeat_smp, q, 0.0).r;
    let b = textureSampleLevel(noise_tex, repeat_smp, q * 2.73 + vec3<f32>(0.37, 0.11, 0.71), 0.0).r;
    return a * 0.65 + b * 0.35;
}

struct Medium {
    // extinction (1/m)
    sigma: f32,
    // optical depth of the dense layers (weather fog, mist) towards the sky above (×1/cos)
    shade: f32,
    // the dense layers' share of the extinction
    dense: f32,
}

// The medium at p: smooth boundary-layer haze, plus weather fog and ground mist broken into
// banks and wisps by the noise.
fn medium(p: vec3<f32>) -> Medium {
    let above = p.y - vol.haze.z;
    let haze = vol.haze.x * exp(-clamp(above, -2.0 * vol.haze.y, 40.0 * vol.haze.y) / vol.haze.y);
    let fog = vol.haze.w * exp(-clamp(above, -2.0 * vol.mist.x, 40.0 * vol.mist.x) / vol.mist.x);
    // over the ground: the street haze that sunbeams are made of, and mist in the hollows
    var mist = 0.0;
    var low = 0.0;
    if vol.terrain.w > 0.5 && (vol.mist.y > 0.0 || vol.layers.x > 0.0) {
        // (the sea is ground too: haze and mist lie on the water, not on the seabed)
        let over = max(p.y - max(ground(p.xz), vol.layers.z), 0.0);
        mist = vol.mist.y * exp(-over / vol.mist.z);
        low = vol.layers.x * exp(-over / vol.layers.y);
    }
    var m: Medium;
    let dense = fog + mist;
    if vol.wind.w > 0.0 && (dense > 1e-6 || haze + low > 1e-6) {
        let n = drift_noise(p);
        let k = max(mix(1.0, n * 2.2 - 0.1, vol.wind.w), 0.0);
        m.sigma = (haze + low) * mix(1.0, k, 0.35) + dense * k;
        m.dense = dense * k / max(m.sigma, 1e-9);
    } else {
        m.sigma = haze + low + dense;
        m.dense = dense / max(m.sigma, 1e-9);
    }
    m.shade = fog * vol.mist.x + mist * vol.mist.z;
    return m;
}

// How much of a directional light reaches p past its shadow casters (1 outside the cascades).
fn key_visibility(i: u32, p: vec3<f32>, d: f32) -> f32 {
    let l = &lights.directional_lights[i];
    if ((*l).flags & SHADOWS_ENABLED) == 0u {
        return 1.0;
    }
    var c = 0u;
    loop {
        if c >= (*l).num_cascades || d < (*l).cascades[c].far_bound {
            break;
        }
        c += 1u;
    }
    if c >= (*l).num_cascades {
        return 1.0;
    }
    // a hair towards the light so the air right on a lit surface isn't caught in its own shadow
    let pos = p + (*l).direction_to_light * 0.15;
    let clip = (*l).cascades[c].clip_from_world * vec4<f32>(pos, 1.0);
    if clip.w <= 0.0 {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if any(ndc.xy < vec2<f32>(-1.0)) || any(ndc.xy > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return textureSampleCompareLevel(shadow_tex, shadow_smp, uv, i32((*l).depth_texture_base_index + c), ndc.z);
}

// Interleaved gradient noise per cell, so neighbouring cells sample different depths.
fn cell_noise(id: vec3<u32>) -> f32 {
    let px = vec2<f32>(id.xy) + vec2<f32>(f32(id.z) * 7.31, f32(id.z) * 3.17);
    return fract(52.9829189 * fract(dot(px, vec2<f32>(0.06711056, 0.00583715))));
}

@compute @workgroup_size(4, 4, 4)
fn inject(@builtin(global_invocation_id) id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if li == 0u {
        let pa = get_view_position();
        wg_sky = sky_light(length(pa));
        for (var i = 0u; i < MAX_KEYS; i++) {
            wg_key[i] = select(vec3<f32>(0.0), key_color(i, pa), i < lights.n_directional_lights);
        }
    }
    workgroupBarrier();

    let grid = vec3<u32>(vol.grid.xyz);
    if any(id >= grid) {
        return;
    }
    // a jittered depth inside the cell (a new one every frame; the history averages them)
    let w = (f32(id.z) + fract(vol.depth.w + cell_noise(id))) / vol.grid.z;
    let d = slice_depth(vol, w);
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vol.grid.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let pv = vec3<f32>(ndc.x * vol.proj.x * d, ndc.y * vol.proj.y * d, -d);
    let p = (vol.world_from_view * vec4<f32>(pv, 1.0)).xyz;
    let ray = normalize(p - vol.camera.xyz);

    let m = medium(p);
    var light = vec3<f32>(0.0);
    if m.sigma > 1e-7 {
        let g = vol.shape.y;
        for (var i = 0u; i < MAX_KEYS; i++) {
            let col = wg_key[i];
            if max(col.r, max(col.g, col.b)) < 1e-3 {
                continue;
            }
            let ldir = lights.directional_lights[i].direction_to_light;
            let vis = key_visibility(i, p, d);
            // the fog's own shade: light that reaches deep into a fog bank has crossed the rest of it
            let fog_vis = exp(-m.shade / max(ldir.y, 0.08));
            let c = dot(ray, ldir);
            // a strong forward lobe (the glow round the sun, bright shafts looking into the light)
            // with a little back-scatter so beams still read from the side
            let phase = mix(hg(c, g), hg(c, -0.2), 0.22);
            let multi = MULTI_SCATTER * m.dense * sqrt(fog_vis) * mix(vis, 1.0, 0.5);
            light += col * (vis * fog_vis * phase + multi) * vol.shape.z;
        }
        // the sky's light and the artificial fill, from every side (with some bounced off the ground)
        light += wg_sky * (0.62 * vol.tint.w) + vol.ambient.rgb;
        // lamps and fires: soft glows in the haze round them
        let n = u32(vol.ambient.w);
        for (var k = 0u; k < n; k++) {
            let lp = vol.lights[k];
            let to = lp.position.xyz - p;
            let d2 = dot(to, to);
            let range = lp.position.w;
            if d2 < range * range {
                let dist = sqrt(d2);
                let x = d2 / (range * range);
                let fall = (1.0 - x * x) * (1.0 - x * x) / max(d2, 0.09);
                light += lp.color.rgb * fall * hg(dot(ray, to / max(dist, 1e-3)), g * 0.5) * vol.shape.w;
            }
        }
    }
    var outv = vec4<f32>(light * vol.tint.rgb * m.sigma * view.exposure, m.sigma);

    // last frame's light at this point
    if vol.grid.w > 0.0 {
        let pp = (vol.prev_view_from_world * vec4<f32>(p, 1.0)).xyz;
        let pd = -pp.z;
        if pd > vol.depth.x {
            let puv = vec2<f32>(pp.x / (pd * vol.proj.z) * 0.5 + 0.5, 0.5 - pp.y / (pd * vol.proj.w) * 0.5);
            let pw = depth_slice(vol, pd);
            if all(puv >= vec2<f32>(0.0)) && all(puv <= vec2<f32>(1.0)) && pw <= 1.0 {
                let h = textureSampleLevel(history_tex, clamp_smp, vec3<f32>(puv, pw), 0.0);
                outv = mix(outv, h, vol.grid.w);
            }
        }
    }
    textureStore(out_tex, id, outv);
}
