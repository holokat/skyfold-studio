// Night sky on an inward sphere, drawn additively behind every other transparent surface (so the
// cloud layer covers it): procedural star field, a faint milky way, and the moon with craters,
// dark maria, the correct lit phase (from the sun direction), earthshine and a soft halo.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};
import super::noise::{hash3f, fbm2, fbm3, voronoi2};

// x: star intensity, y: sky rotation, z: time, w: moon phase (0 new, 0.5 full)
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> params: vec4<f32>;
// xyz: direction towards the moon, w: angular radius (rad)
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var<uniform> moon: vec4<f32>;
// xyz: direction towards the sun, w: moon brightness
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var<uniform> sun: vec4<f32>;

fn rotate_y(v: vec3<f32>, a: f32) -> vec3<f32> {
    let s = sin(a);
    let c = cos(a);
    return vec3<f32>(c * v.x + s * v.z, v.y, -s * v.x + c * v.z);
}

fn star_layer(dir: vec3<f32>, scale: f32, seed: u32, density: f32, px: f32) -> vec3<f32> {
    let p = dir * scale;
    let cell = floor(p);
    let f = p - cell;
    let ci = vec3<i32>(cell);
    let h = hash3f(ci.x, ci.y, ci.z, seed);
    if h < 1.0 - density {
        return vec3<f32>(0.0);
    }
    let sp = vec3<f32>(hash3f(ci.x, ci.y, ci.z, seed + 1u), hash3f(ci.x, ci.y, ci.z, seed + 2u), hash3f(ci.x, ci.y, ci.z, seed + 3u)) * 0.7 + 0.15;
    let d = length(f - sp);
    let b = hash3f(ci.x, ci.y, ci.z, seed + 4u);
    // never smaller than ~1.5 px so stars survive on any screen
    let size = max(0.05 + 0.08 * b, px * 1.5);
    let core = smoothstep(size, 0.0, d);
    let temp = hash3f(ci.x, ci.y, ci.z, seed + 5u);
    let col = mix(vec3<f32>(0.75, 0.85, 1.0), vec3<f32>(1.0, 0.86, 0.7), temp);
    let twinkle = 0.75 + 0.25 * sin(params.z * (1.5 + b * 3.0) + h * 40.0);
    return col * core * core * (0.45 + 1.8 * b * b) * twinkle;
}

struct MoonSample {
    color: vec3<f32>,
    // 1 on the disc (hides the stars behind it)
    cover: f32,
}

fn moon_sample(dir: vec3<f32>) -> MoonSample {
    var o: MoonSample;
    o.color = vec3<f32>(0.0);
    o.cover = 0.0;
    let bright = sun.w;
    if bright <= 0.0 || moon.w <= 0.0 {
        return o;
    }
    let m = normalize(moon.xyz);
    // atmospheric extinction: dimmer and warmer near the horizon, gone below it
    let ext = mix(vec3<f32>(1.0, 0.6, 0.35), vec3<f32>(1.0), smoothstep(0.0, 0.25, m.y)) * smoothstep(-0.03, 0.015, m.y);
    if max(ext.r, 0.0) <= 0.0 {
        return o;
    }
    let r = moon.w;
    let up0 = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(m.y) > 0.99);
    let right = normalize(cross(up0, m));
    let up = cross(m, right);
    let p = vec2<f32>(dot(dir, right), dot(dir, up)) / sin(r);
    let dist = length(p);
    // derivatives before any per-pixel branch (uniform control flow)
    let fw = max(fwidth(dist), 1.0e-4);
    let c = dot(dir, m);
    if c <= 0.0 {
        return o;
    }
    let ang = acos(clamp(c, -1.0, 1.0));
    // lit side: the phase sets the sun–moon–viewer angle (0 at full moon), the real sun sets
    // which limb is bright
    let elong = params.w * 6.2831853;
    let alpha = abs(3.14159265 - elong);
    let sd = normalize(sun.xyz);
    var t = sd - m * dot(sd, m);
    t = select(up, normalize(t), length(t) > 1.0e-3);
    let light = normalize(-m * cos(alpha) + t * sin(alpha));
    let lit_frac = 0.5 + 0.5 * cos(alpha);

    // halo: moonlight scattered around the disc, stronger near full moon
    let beyond = max(ang - r, 0.0);
    let halo = exp(-beyond / (r * 1.5)) * 0.045 + exp(-beyond / (r * 8.0)) * 0.012;
    o.color = vec3<f32>(0.62, 0.72, 0.92) * halo * bright * (0.2 + 0.8 * lit_frac);

    let disc = 1.0 - smoothstep(1.0 - fw, 1.0 + fw, dist);
    if disc > 0.0 {
        let q = p / max(dist, 1.0);
        let z = sqrt(max(1.0 - dot(q, q), 0.0));
        // surface normal of the visible hemisphere (it faces the viewer)
        let n = normalize(right * q.x + up * q.y - m * z);
        // dark maria, bright highlands, craters with bright rims
        let maria = smoothstep(0.5, 0.66, fbm2(q * 1.7 + vec2<f32>(3.1, 1.7), 4u, 71u));
        let rough = fbm2(q * 9.0, 3u, 72u);
        let v = voronoi2(q * 5.5, 73u);
        let crater_r = 0.18 + 0.2 * v.z;
        let rim = smoothstep(crater_r - 0.08, crater_r, v.x) * (1.0 - smoothstep(crater_r, crater_r + 0.05, v.x));
        let floor_ = 1.0 - smoothstep(crater_r - 0.12, crater_r - 0.02, v.x);
        var albedo = mix(0.9, 0.4, maria) * (0.78 + 0.36 * rough);
        albedo = albedo + rim * 0.12 * (1.0 - maria) - floor_ * 0.06;
        // lit phase with a soft terminator, slight limb darkening, faint earthshine on the dark side
        let ndl = dot(n, light);
        let lit = smoothstep(-0.03, 0.1, ndl) * (0.72 + 0.28 * z);
        let shade = lit + 0.03 * (1.0 - lit);
        let surf = vec3<f32>(1.0, 0.97, 0.92) * albedo * shade * bright;
        o.color = mix(o.color, surf, disc);
        o.cover = disc;
    }
    o.color *= ext;
    return o;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir0 = normalize(in.world_position.xyz - view.world_position);
    let mn = moon_sample(dir0);

    var c = vec3<f32>(0.0);
    let vis = params.x;
    if vis > 0.002 {
        // fade into the thick air at the horizon
        let horizon = smoothstep(-0.01, 0.12, dir0.y);
        let dir = rotate_y(dir0, params.y * 0.25);
        let px = length(fwidth(dir)) * 260.0;
        c = star_layer(dir, 260.0, 11u, 0.16, px);
        c += star_layer(dir, 120.0, 23u, 0.08, px * 0.46) * 1.6;
        // milky way band
        let band_n = normalize(vec3<f32>(0.35, 0.25, 0.9));
        let band = exp(-pow(dot(dir, band_n) / 0.22, 2.0));
        let neb = fbm2(vec2<f32>(atan2(dir.z, dir.x) * 3.0, dir.y * 6.0), 4u, 5u);
        c += vec3<f32>(0.55, 0.6, 0.85) * band * smoothstep(0.35, 0.8, neb) * 0.12;
        c += star_layer(dir, 520.0, 37u, 0.25 * band + 0.02, px * 2.0) * 0.8;
        c *= vis * horizon * (1.0 - mn.cover);
    }
    let out = c + mn.color;
    if max(out.r, max(out.g, out.b)) <= 0.0 {
        discard;
    }
    // AlphaMode::Add blends premultiplied (src + dst * (1 - a)): alpha 0 adds the light on top
    // of the sky; alpha 1 replaced the twilight sky with near-black wherever the dome drew
    return vec4<f32>(out, 0.0);
}
