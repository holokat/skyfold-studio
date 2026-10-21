// Under water (motion/underwater.rs): one fullscreen pass in HDR, before bloom, while the camera
// is below a water surface.
//
//   floor light  what lies down there was lit through the water above it: the sun dims per colour
//                along its refracted path, the sky with depth, and the waves overhead focus the sun
//                into a moving caustic net (the water shader's caustic map, read where the sun ray
//                entered the water, blurrier deeper down)
//   view path    absorbed per colour (red first) and scattered by the particles in the water (how
//                far one sees: about the style's clarity ×5), and light scattered towards the eye:
//                a short ray march through sun shafts (the caustic map at a coarse mip, projected
//                along the refracted sun, so beams slant down from the surface), brightest towards
//                the sun, plus the sky's diffuse glow; beyond the march the water's colour fills in
//   motes        marine snow in a few depth layers of world-space cells (parallax-correct), lit like
//                the water around them

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct UnderwaterView {
    clip_to_world: mat4x4<f32>,
    cam: vec4<f32>,      // xyz camera, w time
    fwd: vec4<f32>,      // xyz camera forward, w near plane
    level: vec4<f32>,    // x surface level, y caustic strength, z sun share, w shaft reach (m)
    extinct: vec4<f32>,  // rgb extinction per metre, w clarity
    inscat: vec4<f32>,   // rgb in-scatter colour (the deep colour, lighter over a shallow floor)
    sun: vec4<f32>,      // xyz to the key light, w key strength
    sun_col: vec4<f32>,
    sky: vec4<f32>,
}

@group(0) @binding(0) var screen_tex: texture_2d<f32>;
@group(0) @binding(1) var screen_samp: sampler;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var caustic_tex: texture_2d<f32>;
@group(0) @binding(4) var caustic_samp: sampler;
@group(0) @binding(5) var<uniform> u: UnderwaterView;

const IOR: f32 = 1.333;

fn ss(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn rot2(v: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(v.x * c - v.y * s, v.x * s + v.y * c);
}

fn hash33(p: vec3<f32>) -> vec3<f32> {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.xxy + q.yxx) * q.zyx);
}

// the sun bent into the water (points up, towards where the light comes from)
fn sun_in_water() -> vec3<f32> {
    let L = normalize(u.sun.xyz);
    return normalize(vec3<f32>(L.x / IOR, max(sqrt(max(1.0 - (1.0 - L.y * L.y) / (IOR * IOR), 0.0)), 0.1), L.z / IOR));
}

// the waves' caustic net where the sun ray through `p` (h metres down) entered the water;
// mean 1. `lod` blurs it (shafts read it coarse).
fn caustic_at(p: vec3<f32>, h: f32, ls: vec3<f32>, lod: f32) -> f32 {
    let entry = p.xz + ls.xz * (h / ls.y);
    let c = textureSampleLevel(caustic_tex, caustic_samp, rot2(entry, 0.37) / 13.85, lod);
    return mix(c.r, c.g, ss(1.5, 4.0, h));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let scene = textureSample(screen_tex, screen_samp, in.uv).rgb;
    let size = vec2<f32>(textureDimensions(depth_tex));
    let px = vec2<i32>(clamp(in.uv * size, vec2<f32>(0.0), size - 1.0));
    let d = textureLoad(depth_tex, px, 0);
    let cam = u.cam.xyz;
    let t = u.cam.w;
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let nh = u.clip_to_world * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(nh.xyz / nh.w - cam);
    // (reverse-z: view depth = near / depth; nothing there = the water goes on forever)
    let open = d < 1e-7;
    let zv = u.fwd.w / max(d, 1e-7);
    let dist = select(min(zv / max(dot(dir, u.fwd.xyz), 1e-3), 3000.0), 3000.0, open);
    let p = cam + dir * dist;

    let level = u.level.x;
    let sig = max(u.extinct.rgb, vec3<f32>(1e-4));
    // along a view ray the particles scatter light out too: clear water fades out over ~100 m,
    // murky water within a few metres
    let sig_t = sig + vec3<f32>(max(1.0 / (u.extinct.w * 1.6), 0.02));
    let alb = u.inscat.rgb;
    let L = normalize(u.sun.xyz);
    let ls = sun_in_water();
    let mu_s = max(ls.y, 0.1);
    // the sun entering the water: Fresnel-transmitted, spread over the refracted beam
    let sun = u.sun_col.rgb * u.sun.w;
    let sun_in = sun * (0.98 - 0.98 * pow(1.0 - max(L.y, 0.0), 5.0)) * ss(0.0, 0.12, L.y) * max(L.y, 0.0) / mu_s;
    let sky = u.sky.rgb;
    let sun_w = u.level.z;

    // ---- the floor and everything on it: lit through the water above it
    var col = scene;
    let hp = max(level - p.y, 0.0);
    if !open && hp > 0.02 {
        let lod = clamp(hp * 0.15, 0.0, 5.0);
        // the net of the chop, threaded with a finer one (the same map at a smaller tile)
        let fine = textureSampleLevel(caustic_tex, caustic_samp, rot2(p.xz + ls.xz * (hp / ls.y), 2.1) / 3.3, lod + 1.0).r;
        // (squared: the focused light gathers into thin bright threads, as real caustics do)
        let c0 = caustic_at(p, hp, ls, lod);
        let caus = c0 * c0 * 0.62 * mix(1.0, fine, 0.5 * exp(-hp * 0.15));
        let c_amt = ss(0.05, 0.6, hp) * exp(-0.05 * hp) * u.level.y;
        let sunlit = exp(-sig * hp / mu_s) * max(1.0 + (caus - 1.0) * c_amt * 1.6, 0.0);
        let skylit = exp(-sig * hp * 1.2) * 0.9;
        col = scene * mix(skylit, sunlit, sun_w);
    }

    // ---- along the view ray: absorption and in-scattering (sun shafts + sky glow)
    let h0 = max(level - cam.y, 0.0);
    let cos_v = dot(dir, ls);
    // forward-peaked: bright looking up the beams, a soft glow all around
    let phase = 0.55 + 0.45 * ss(-0.4, 1.0, cos_v) + 2.2 * pow(max(cos_v, 0.0), 12.0);
    let reach = min(dist, u.level.w);
    let n = 14;
    // (a fixed interleaved-gradient dither: steady shafts without temporal noise)
    let jit = fract(52.9829189 * fract(dot(in.position.xy, vec2<f32>(0.06711056, 0.00583715))));
    var ins = vec3<f32>(0.0);
    var prev = 0.0;
    for (var i = 0; i < n; i++) {
        // (denser near the eye, where shafts are sharp)
        let f = (f32(i) + jit) / f32(n);
        let s = reach * f * f;
        let ds = s - prev;
        prev = s;
        let x = cam + dir * s;
        let h = max(level - x.y, 0.0);
        // (a coarse mip of the net: broad beams; their contrast stretched, fading with depth)
        let shaft_k = 2.2 * exp(-h * 0.07) * u.level.y;
        let shaft = max(1.0 + (caustic_at(x, h, ls, 1.6 + h * 0.12) - 1.0) * shaft_k, 0.1);
        let light = sun_in * exp(-sig * h / mu_s) * (2.7 * phase * shaft) + sky * exp(-sig * h / 0.75) * 3.7;
        ins += light * alb * sig_t * exp(-sig_t * s) * ds;
    }
    // beyond the march: the rest of the path at the depth where it ended
    if dist > reach {
        let he = max(h0 + (-dir.y) * reach, 0.0);
        let light = sun_in * exp(-sig * he / mu_s) * (2.7 * phase) + sky * exp(-sig * he / 0.75) * 3.7;
        ins += light * alb * (exp(-sig_t * reach) - exp(-sig_t * dist));
    }
    var out = col * exp(-sig_t * dist) + ins;

    // ---- marine snow: a few layers of motes at growing distances (world cells, drifting)
    let drift = vec3<f32>(t * 0.05, -t * 0.012, t * 0.03);
    let snow_light = sun_in * exp(-sig * h0 / mu_s) * 0.9 + sky * exp(-sig * h0 / 0.75) * 1.2;
    for (var k = 0; k < 5; k++) {
        let lay = 0.7 * pow(2.0, f32(k));
        if lay > dist {
            break;
        }
        let cell = 0.22 * pow(1.75, f32(k));
        let x = cam + dir * lay + drift * (1.0 + f32(k) * 0.3);
        let g = floor(x / cell);
        let hr = hash33(g + f32(k) * 17.0);
        if hr.z > 0.55 {
            continue;
        }
        let c = (g + 0.2 + 0.6 * hr) * cell - drift * (1.0 + f32(k) * 0.3);
        let v = c - cam;
        let tc = dot(v, dir);
        if tc < 0.15 || tc > dist {
            continue;
        }
        let perp = length(v - dir * tc);
        // ~2 px motes far away, softer and larger close to the eye (out of focus)
        let rad = 0.0011 * tc + 0.0025;
        let m = ss(rad, rad * 0.35, perp) * (0.35 + 0.65 * hr.x);
        out += snow_light * 0.07 * m * exp(-sig_t * tc) * (1.0 - ss(6.0, 26.0, tc));
    }
    return vec4<f32>(out, 1.0);
}
