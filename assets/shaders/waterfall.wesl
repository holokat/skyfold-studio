// Waterfalls: the falling water, its mist and spray, and the foam over the plunge pool, all lit
// through Bevy PBR so they sit in the scene's light, fog and night. Everything runs on the fall's
// own clock, which stands still while the world's motion is frozen (editing from the overview):
// a still frame is a sharp photograph of moving water, never a smear.
//
// Sheet (mode 0): two ribbons, vertex colour = (progress down the fall, steepness, edge 0..1..0,
// layer: 0 aerated core, 1 translucent outer sheet), uv = (across 0..1, metres along). The water
// is drawn as streaks: ropes of water a hand to a forearm wide, stretched along the drop and
// racing down it (accelerating with the drop), in three layered scales. Over the lip the outer
// sheet is a glassy tongue tinted by the water's colour; air is beaten into it as it falls, so
// it turns to white water, densest in the core. The sides tear into fine strands rather than
// cut-outs; a thin fall tears into curtains with gaps. White water scatters light every way (it
// is lit from the sky above and shines through towards the sun), glassy water reflects.
// Mist and spray (mode 1): camera-facing puffs expanded here from their centres (uv = corner,
// vertex colour = phase, size, seed, opacity; negative opacity = spray; normal.z = the pool's
// level). Mist puffs rise from the impact, swell, drift and fade: faint, bright and soft, they
// fade out where they meet the ground or the water (soft particles, from the depth prepass) and
// near the eye, so they never read as a bank of fog. Spray is fine bright droplets flung out of
// the impact on short arcs.
// Foam (mode 2): a disc on the pool (uv = metres from the impact): white water boiling at the
// impact and foam carried outward by the currents, tearing into lace and dissolving; it thins
// out over the shallows.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_functions,
    view_transformations::{position_world_to_clip, depth_ndc_to_view_z},
};

struct FallParams {
    color: vec4<f32>, // rgb water colour (linear), w: mode
    deep: vec4<f32>,  // rgb depth colour, w: width
    glow: vec4<f32>,  // rgb bioluminescence × darkness
    emis: vec4<f32>,  // rgb inner glow, w: the frozen clock (s; negative: the world's clock runs)
    flow: vec4<f32>,  // flow, mist, sheet length, drop
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> fall: FallParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var noise_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var foam_tex: texture_2d<f32>;

fn ss(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn clock() -> f32 {
    return select(globals.time, fall.emis.w, fall.emis.w >= 0.0);
}

// How far (m, along the view axis) the opaque scene lies behind this fragment: soft particles fade
// where they meet the ground. (Huge where nothing is behind, or without a depth prepass.)
fn behind(frag: vec4<f32>) -> f32 {
    var gap = 1e4;
@if(DEPTH_PREPASS && !MULTISAMPLED) {
    let d = textureLoad(bevy_pbr::render::mesh_view_bindings::depth_prepass_texture, vec2<i32>(frag.xy), 0);
    if d > 0.0 {
        gap = depth_ndc_to_view_z(frag.z) - depth_ndc_to_view_z(d);
    }
}
    return gap;
}

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0)).xyz;
    var nrm = v.normal;
    var color = vec4<f32>(1.0);
@if(VERTEX_COLORS) {
    color = v.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = v.uv;
}
    if fall.color.w > 0.5 && fall.color.w < 1.5 {
        let t = clock();
        let right = normalize(view.world_from_view[0].xyz);
        let up = normalize(view.world_from_view[1].xyz);
        let pool = v.normal.z;
        var size = color.y;
        if color.w < 0.0 {
            // spray: a droplet cloud flung out of the impact on a short arc, every ~1.5 s
            let rate = 0.6 + 0.3 * color.z;
            let life = fract(t * rate + color.x);
            let a = color.z * 6.2832 + floor(t * rate + color.x) * 2.4;
            let speed = 2.5 + 3.0 * fall.flow.x * (0.5 + color.z);
            let out_dir = vec3<f32>(cos(a), 0.0, sin(a));
            let tt = life * 1.1;
            wp = wp + out_dir * speed * tt * 0.55 + vec3<f32>(0.0, speed * tt - 4.9 * tt * tt, 0.0);
            size = size * (0.7 + 1.0 * life);
            color.w = -sin(life * 3.14159) * ss(pool - 0.2, pool + 0.4, wp.y);
        } else {
            // a mist puff: rise and swell over its life, drift with the air, fade in and out; its
            // lower edge stays near the water
            let rate = 0.06 + 0.025 * fall.flow.x;
            let life = fract(t * rate + color.x);
            size = color.y * (0.55 + 0.9 * life);
            let drift = vec3<f32>(sin(color.z * 40.0 + t * 0.3), 0.0, cos(color.z * 23.0 + t * 0.25)) * (0.3 + 1.5 * life) + vec3<f32>(0.6, 0.0, 0.35) * life * 2.5;
            wp = wp + vec3<f32>(0.0, life * color.y * 1.2, 0.0) + drift;
            wp.y = max(wp.y, pool + size * 0.45);
            color.w = color.w * sin(life * 3.14159);
        }
        // spin the puff slowly so the noise inside rolls
        let a = color.z * 6.28 + t * (0.08 + color.z * 0.12) * select(-1.0, 1.0, color.z > 0.5);
        let c = cos(a);
        let s = sin(a);
        let corner = vec2<f32>(uv.x * c - uv.y * s, uv.x * s + uv.y * c);
        wp = wp + (right * corner.x + up * corner.y) * size;
        nrm = normalize(view.world_position - wp);
        // (the fragment reads the pool's level and the puff's size from here)
        color.x = pool;
        color.y = size;
    }
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = nrm;
@if(VERTEX_UVS_A)
    out.uv = uv;
@if(VERTEX_COLORS)
    out.color = color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let t = clock();
    var vc = vec4<f32>(1.0);
@if(VERTEX_COLORS) {
    vc = in.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    let water_col = fall.color.rgb;
    let mode = u32(fall.color.w + 0.5);
    let flow = fall.flow.x;
    let width = fall.deep.w;
    let wp = in.world_position.xyz;
    let V = normalize(view.world_position - wp);
    let up = vec3<f32>(0.0, 1.0, 0.0);
    var base = vec3<f32>(1.0);
    var alpha = 1.0;
    var white = 0.0;
    var rough = 0.4;
    var transmit = 0.0;
    var N = normalize(in.world_normal);
    if mode == 1u {
        // (the mist / spray branch below depends on the vertex: read every texture first)
        let r2 = dot(uv, uv);
        let n1 = textureSample(noise_tex, samp, uv * 0.2 + vec2<f32>(vc.z * 7.0, t * 0.015)).r;
        let n2 = textureSample(noise_tex, samp, uv * 0.47 + vec2<f32>(vc.z * 3.0 + 0.5, -t * 0.02)).g;
        let drops = textureSample(foam_tex, samp, uv * 0.3 + vec2<f32>(vc.z * 5.0, vc.z * 2.0)).g;
        let pool = vc.x;
        let size = vc.y;
        // soft against the ground, the water and the eye
        let soft = ss(0.0, 0.6 + size * 0.5, behind(in.position)) * ss(pool, pool + size * 0.35, wp.y) * ss(1.5, 7.0, length(view.world_position - wp));
        let right = normalize(view.world_from_view[0].xyz);
        let vup = normalize(view.world_from_view[1].xyz);
        let fwd = normalize(view.world_from_view[2].xyz);
        if vc.w < 0.0 {
            // ---- spray: a cluster of fine droplets, catching the light
            let d = ss(0.35, 0.75, drops) * pow(clamp(1.0 - r2, 0.0, 1.0), 1.5);
            alpha = d * -vc.w * (0.5 + 0.35 * fall.flow.y) * soft;
            base = vec3<f32>(0.95, 0.97, 0.98);
            rough = 0.25;
            white = 1.0;
            N = normalize(fwd + up * 0.8);
            transmit = 0.6;
        } else {
            // ---- mist: a faint, bright, rolling veil, lit as a soft sphere
            let body = (1.0 - r2) * (0.4 + 0.9 * (n1 * 0.65 + n2 * 0.35)) - 0.25;
            alpha = ss(0.0, 0.7, body) * vc.w * (0.05 + 0.14 * fall.flow.y) * soft;
            base = mix(vec3<f32>(0.96, 0.97, 0.98), water_col * 1.4 + vec3<f32>(0.5), 0.08);
            rough = 1.0;
            white = 1.0;
            N = normalize(right * uv.x + vup * uv.y + fwd * sqrt(max(1.0 - r2, 0.0)) * 1.3 + up * 0.6);
            transmit = 0.9;
        }
        if alpha < 0.003 || r2 > 1.0 {
            discard;
        }
    } else if mode == 2u {
        // ---- foam over the pool: boiling at the impact, carried outward by the currents (two
        // phases of a short radial drift, each hidden while it resets) while it churns in place
        let radius = vc.x;
        let rad = length(uv);
        let k = rad / max(radius, 0.1);
        let out_dir = uv / max(rad, 0.5);
        let period = 3.0;
        let ph0 = fract(t / period);
        let ph1 = fract(t / period + 0.5);
        let wb = abs(2.0 * ph0 - 1.0);
        let vel = out_dir * (0.25 + 0.2 * min(flow, 2.0));
        let qa = uv - vel * (ph0 * period);
        let qb = uv - vel * (ph1 * period) + vec2<f32>(3.7, 1.9);
        let fa = textureSample(foam_tex, samp, qa / 5.0 + vec2<f32>(t * 0.011, -t * 0.007));
        let fb = textureSample(foam_tex, samp, qb / 5.0 + vec2<f32>(t * 0.011, -t * 0.007));
        let fine_a = textureSample(foam_tex, samp, qa / 1.7 + vec2<f32>(0.37 - t * 0.02, 0.11 + t * 0.015)).r;
        let fine_b = textureSample(foam_tex, samp, qb / 1.7 + vec2<f32>(0.37 - t * 0.02, 0.11 + t * 0.015)).r;
        let fn1 = textureSample(noise_tex, samp, uv / 9.0 + vec2<f32>(0.3, t * 0.01)).r;
        let f = mix(fa, fb, wb);
        let dens = f.r * 0.6 + mix(fine_a, fine_b, wb) * 0.4;
        // coverage: the boil at the impact, then patches carried out and thinning
        let boil = 1.0 - ss(0.08, 0.38, k + (fn1 - 0.5) * 0.2);
        let cover = clamp(boil * 0.9 + (1.0 - ss(0.2, 1.0, k + (fn1 - 0.5) * 0.35)) * (0.35 + 0.25 * min(flow, 2.0)), 0.0, 1.0);
        let thresh = 1.0 - cover;
        let lace = ss(thresh - 0.12, thresh + 0.12, dens);
        // thins out over the shallows (the bed close under it) and at the edge of the disc
        let shallow = ss(0.05, 0.5, behind(in.position));
        alpha = lace * (0.5 + 0.45 * boil) * (1.0 - ss(0.8, 1.0, k)) * shallow * ss(0.0, 0.2, flow + 0.05);
        base = mix(vec3<f32>(0.86, 0.9, 0.92), vec3<f32>(0.95, 0.97, 0.98), f.g * 0.5 + boil * 0.5);
        rough = 0.7;
        white = 1.0;
        N = normalize(up + V * 0.25);
        transmit = 0.3;
        if alpha < 0.01 {
            discard;
        }
    } else {
        // ---- the sheet
        let p = vc.x;
        let slide = 1.0 - vc.y;
        let edge = vc.z;
        let core = vc.w < 0.5;
        let across = uv.x * width * (1.0 + 0.35 * p) * select(1.0, 0.72, core);
        let along = uv.y;
        // falling speed grows with the drop (v = sqrt(2 g h)): streaks stretch as they accelerate
        let speed = 2.0 + 1.5 * flow + sqrt(2.0 * 9.81 * max(p * fall.flow.w, 0.0)) * 0.8;
        let lay = select(0.0, 0.43, core);
        // ropes of water (a hand to a forearm wide), fine streaks within them, slow broad swells
        let s1 = textureSample(noise_tex, samp, vec2<f32>(across / 2.2 + lay, (along - t * speed) / 16.0)).r;
        let s2 = textureSample(noise_tex, samp, vec2<f32>(across / 0.9 + 0.31 + lay, (along - t * speed * 1.12) / 6.0)).g;
        let s3 = textureSample(noise_tex, samp, vec2<f32>(across / 3.6 + 0.63, (along - t * speed * 0.85) / 28.0)).b;
        let s4 = textureSample(noise_tex, samp, vec2<f32>(across / 1.7 + 0.17 + lay, (along - t * speed * 1.25) / 3.5)).a;
        // clots of white water tumbling down inside it
        let clot = textureSample(foam_tex, samp, vec2<f32>(across / 2.6 + lay, (along - t * speed) / 2.4)).r;
        // surges: the flow pulses, bands of thicker water racing down
        let surge = textureSample(noise_tex, samp, vec2<f32>(across / 40.0 + 0.5, (along - t * speed * 0.95) / 70.0)).r;
        let streak = s1 * 0.45 + s2 * 0.35 + s4 * 0.2;
        // aeration: a glassy tongue over the lip turning white as it falls (sooner in the core,
        // with more flow, and in a cascade tumbling down a slope)
        let aer = clamp(ss(0.0, select(0.45, 0.3, core), p) * (0.6 + 0.2 * min(flow, 2.0)) + 0.5 * slide + select(0.15, 0.4, core), 0.0, 1.0);
        let swell = (s3 - 0.5) * 0.9 + (surge - 0.5) * 0.8;
        white = clamp(aer * (1.0 + 2.4 * (streak - 0.5) + swell) + (clot - 0.5) * 0.6 * aer, 0.0, 1.0);
        // body: thicker where the ropes are, thin film between them
        var body = clamp(0.5 + 2.2 * (streak - 0.5) + 0.25 * aer + swell * 0.5, 0.0, 1.0);
        // a thin fall tears into curtains with gaps (a cascade down a slope stays one tumble)
        let thin = (1.0 - ss(0.4, 1.4, flow)) * vc.y;
        body = body * mix(1.0, ss(0.35, 0.6, s3 * 0.6 + s1 * 0.4), thin * 0.85);
        // the sides fray into strands, more as the water spreads lower down
        let fray = ss(0.0, 0.18 + 0.3 * p, edge + (s2 - 0.5) * 0.3 + (s4 - 0.5) * 0.2);
        // the lip: the sheet grows raggedly out of the water over the first metre or two; the foot
        // dissolves into the spray and foam; where it slides over rock it fades into it rather
        // than cutting it
        let ends = ss(0.0, 1.6, along + (s1 - 0.5) * 1.2) * (1.0 - ss(0.86, 1.0, p)) * ss(0.0, 0.15, behind(in.position));
        if core {
            alpha = mix(0.6, 1.0, white) * mix(0.55, 1.0, body) * fray * ends * ss(0.02, 0.15, p);
        } else {
            let glass = 1.0 - ss(0.05, 0.35, p);
            alpha = (mix(0.4, 1.0, white) * mix(0.45, 1.0, body) + glass * 0.25) * fray * ends;
        }
        let tint = mix(water_col, fall.deep.rgb * 2.0, 0.35);
        base = mix(tint * 0.55, mix(vec3<f32>(0.93, 0.95, 0.96), water_col, 0.06), white);
        rough = mix(0.05, 0.65, white);
        transmit = mix(0.55, 0.5, white);
        // relief: the ropes bulge; white water scatters light every way (lit as if facing the
        // sky and the eye), glassy water keeps the sheet's own normal
        let tilt = vec3<f32>(s2 - 0.5, 0.0, s4 - 0.5) * 0.6 + vec3<f32>(0.0, (s1 - 0.5) * 0.4, 0.0);
        let Ns = normalize(select(-N, N, is_front) + tilt);
        let Nw = normalize(Ns * 0.4 + up * 0.7 + V * 0.5);
        N = select(-1.0, 1.0, is_front) * normalize(mix(Ns, Nw, white * 0.75));
        if alpha < 0.01 {
            discard;
        }
    }
    pbr_input.material.base_color = vec4<f32>(base, alpha);
    pbr_input.material.perceptual_roughness = rough;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3<f32>(0.5);
    // thin water and mist let the light through (the low sun behind a fall lights it up)
    pbr_input.material.diffuse_transmission = transmit;
    pbr_input.N = select(-N, N, is_front || mode != 0u);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    // glowing water glows as it falls: brightest in the white water
    let glow = fall.glow.rgb * (0.4 + 1.2 * white) + fall.emis.rgb * (0.6 + 0.6 * white);
    // (display units, like the rest of the lit output)
    out.color = vec4<f32>(out.color.rgb + glow, out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
