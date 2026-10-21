// Cave effects (`crate::caves::render`): still water in the pools, glowing crystals and cobwebs —
// one material, three kinds (`fx.kind.x`: 0 water, 1 crystal, 2 web).
//
// Vertex colour — water: (daylight reaching it 0..1, depth of water under it m, -, -);
// crystal: (glow colour rgb, its strength); web: (daylight, -, -, -).
// uv — water: world x, z; crystal: (along it, 0 at its foot .. 1 at its tip, facet); web: the
// web texture.
//
// - Water: dark and clear — the floor shows through the shallows, deeper it turns to the deep
//   colour; slow ripples and now and then a drip's ring; it mirrors the lights (lanterns,
//   crystals) but the sky only as far as daylight reaches in (underground it is no mirror of a
//   bright sky that isn't there).
// - Crystals: dark glass lit from within: brightest towards the foot, veins of light, a bright
//   rim where the faces turn away; a slow pulse. In display units, so they glow whatever the
//   exposure (and bloom).
// - Webs: fine threads catching whatever light there is.
// All of them take the cave's darkness fill like the rock (`cave.wesl`): light that follows the
// exposure, so a dim cave stays readable once the eye has adjusted.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};
import super::noise::{value_noise2, value_noise3};

struct FxParams {
    // x: kind (0 water, 1 crystal, 2 web), y: the cave's darkness fill (0..1), z: glow strength
    kind: vec4<f32>,
    // water: shallow colour; crystal: body tint; web: thread colour (rgb, linear), a: opacity
    a: vec4<f32>,
    // water: deep colour (rgb)
    b: vec4<f32>,
    // rgb: the cave's fill light colour
    fill: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> fx: FxParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var fx_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var fx_sampler: sampler;

// (as `cave.wesl`: the exposure of an eye adjusted to a cave by day)
const CAVE_EXPOSURE: f32 = 1.0 / (1.2 * 776.0);

// The cave's fill on a surface of colour `albedo` (`amb`: how much daylight reaches it).
fn fill_light(albedo: vec3<f32>, amb: f32) -> vec3<f32> {
    let adjusted = view.exposure / CAVE_EXPOSURE;
    return albedo * fx.fill.rgb * fx.kind.y * (1.0 - amb * 0.85) * 0.62 * min(adjusted, 4.0);
}

// The water's surface height (m) at `p`: two drifting ripple layers and, in some 3 m cells, a
// drip's ring spreading every few seconds.
fn ripple(p: vec2<f32>, t: f32) -> f32 {
    let a = value_noise2(p * 0.9 + vec2<f32>(t * 0.07, t * 0.05), 71u);
    let b = value_noise2(p * 2.3 - vec2<f32>(t * 0.11, -t * 0.08), 73u);
    let cell = floor(p / 3.0);
    let k = fract(sin(dot(cell, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    let phase = fract(t * 0.21 + k);
    let c = (cell + 0.5 + (vec2<f32>(k, fract(k * 7.13)) - 0.5) * 1.6) * 3.0;
    let r = length(p - c);
    let ring = sin((r - phase * 3.0) * 9.0) * exp(-r * 1.4) * (1.0 - phase) * smoothstep(0.0, 0.08, phase) * step(0.55, k);
    return a * 0.03 + b * 0.01 + ring * 0.012;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var col = vec4<f32>(1.0, 0.5, 0.0, 0.0);
    var uv = vec2<f32>(0.0);
@if(VERTEX_COLORS)
    col = in.color;
@if(VERTEX_UVS_A)
    uv = in.uv;
    let wp = in.world_position.xyz;
    let kind = u32(fx.kind.x + 0.5);
    var out: FragmentOutput;

    if kind == 0u {
        // ---- still water
        let day = clamp(col.r, 0.0, 1.0);
        let depth = max(col.g, 0.0);
        let amb = smoothstep(0.0, 0.85, day);
        let t = globals.time;
        let e = 0.05;
        let h0 = ripple(wp.xz, t);
        let hx = ripple(wp.xz + vec2<f32>(e, 0.0), t);
        let hz = ripple(wp.xz + vec2<f32>(0.0, e), t);
        let n = normalize(vec3<f32>(-(hx - h0) / e, 1.0, -(hz - h0) / e));
        let deep = smoothstep(0.0, 1.1, depth);
        let base = mix(fx.a.rgb, fx.b.rgb, deep);
        // clear over the shallows (the floor shows), dark further out; a soft shore
        let alpha = mix(0.12, 0.9, smoothstep(0.0, 0.9, depth)) * smoothstep(0.0, 0.06, depth);
        pbr_input.material.base_color = vec4<f32>(base, alpha);
        pbr_input.material.perceptual_roughness = 0.06;
        pbr_input.material.metallic = 0.0;
        pbr_input.diffuse_occlusion = vec3<f32>(amb);
        pbr_input.specular_occlusion = mix(0.03, 1.0, amb);
        pbr_input.N = n;
        // a sheen at grazing angles: the dim cave round it mirrored in the still water (its
        // fill, as the rock shows it), rippling
        let v = normalize(view.world_position - wp);
        let fres = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 4.0);
        let sheen = fill_light(vec3<f32>(0.32), amb) * (0.15 + 0.85 * fres);
        pbr_input.material.emissive = vec4<f32>(fill_light(base * 1.8 + vec3<f32>(0.01), amb) + sheen, 0.0);
        out.color = apply_pbr_lighting(pbr_input);
        out.color = main_pass_post_lighting_processing(pbr_input, out.color);
        out.color.a = max(alpha, fres * 0.85 * smoothstep(0.0, 0.05, depth));
        return out;
    }

    if kind == 1u {
        // ---- a glowing crystal
        let glow = col.rgb;
        let strength = col.a * fx.kind.z;
        let along = clamp(uv.x, 0.0, 1.0);
        let n = normalize(in.world_normal);
        let v = normalize(view.world_position - wp);
        let rim = pow(1.0 - clamp(abs(dot(n, v)), 0.0, 1.0), 3.0);
        // veins of light: the zero lines of a slow noise, hair-thin
        let vn = value_noise3(wp * vec3<f32>(2.2, 0.7, 2.2) + vec3<f32>(uv.y * 3.1, globals.time * 0.04, 0.0), 81u) - 0.5;
        let vw = max(fwidth(vn), 1e-4);
        let vein = 1.0 - smoothstep(0.0, vw * 1.6, abs(vn));
        let inner = mix(1.0, 0.35, along);
        let pulse = 0.82 + 0.18 * sin(globals.time * 0.9 + wp.x * 0.37 + wp.z * 0.23);
        // (a saturated core — tone mapping turns anything much over 1 white — with bright rims
        // and veins for the bloom to spread)
        let e = glow * strength * (0.35 + 0.55 * inner + 1.1 * rim + 1.6 * vein) * pulse;
        pbr_input.material.base_color = vec4<f32>(glow * 0.12 + vec3<f32>(0.015), 1.0);
        pbr_input.material.perceptual_roughness = 0.12;
        pbr_input.material.metallic = 0.0;
        pbr_input.material.emissive = vec4<f32>(e, 0.0);
        out.color = apply_pbr_lighting(pbr_input);
        out.color = main_pass_post_lighting_processing(pbr_input, out.color);
        return out;
    }

    // ---- a cobweb
    let tex = textureSample(fx_tex, fx_sampler, uv);
    let a = tex.a * fx.a.a;
    if a < 0.015 {
        discard;
    }
    let day = clamp(col.r, 0.0, 1.0);
    let amb = smoothstep(0.0, 0.85, day);
    let base = fx.a.rgb * tex.rgb;
    pbr_input.material.base_color = vec4<f32>(base, a);
    pbr_input.material.perceptual_roughness = 0.6;
    pbr_input.material.metallic = 0.0;
    pbr_input.diffuse_occlusion = vec3<f32>(amb);
    pbr_input.specular_occlusion = mix(0.05, 1.0, amb);
    pbr_input.material.emissive = vec4<f32>(fill_light(base * 1.4, amb), 0.0);
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    out.color.a = a;
    return out;
}
