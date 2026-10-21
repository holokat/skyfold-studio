// Sun glare and lens flare (camera_fx/flare.rs). `probe` draws three pixels, blended with the last
// frame's reading: the colour the sun is drawn in; how much of it is in view (open sky in the depth
// buffer round its place on screen; reverse-Z depth 0 = sky) and the sky's brightness a few degrees
// away; how much it outshines that sky (a sun behind cloud hardly does). Out of frame its colour and
// clearness ease towards the CPU's guess (`prior`). `flare` adds, over the view, the veiling glare
// round the sun, a faint starburst and the ghosts along the line from the sun through the centre.
// All sizes are in screen heights, x scaled by the aspect (round shapes).

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;

struct LensFlare {
    // xy: the sun on screen (uv, may lie off screen), z: glare strength, w: ghost strength
    sun: vec4<f32>,
    // rgb: the look's sun colour, w: view aspect (width / height)
    tint: vec4<f32>,
    // x: starburst turn (rad), y: angular scale (tan ½fov / tan 25°), z: probe radius (screen
    // heights), w: how far the probe's disc is in frame (1 measured … 0 `prior`)
    params: vec4<f32>,
    // out of frame: rgb the sun's colour, w how clear it likely is
    prior: vec4<f32>,
}

@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var probe_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> lf: LensFlare;

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const GOLDEN: f32 = 2.39996323;
const INNER_N: i32 = 40;
const RING_N: i32 = 16;

// one tap round the sun (`o` from it in screen heights): rgb its colour, a 1 where it is open sky
fn tap(o: vec2<f32>, dims: vec2<i32>) -> vec4<f32> {
    let uv = clamp(lf.sun.xy + o * vec2<f32>(1.0 / lf.tint.w, 1.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let p = min(vec2<i32>(uv * vec2<f32>(dims)), dims - vec2<i32>(1));
    if textureLoad(depth_tex, p, 0) > 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(min(textureLoad(hdr_tex, p, 0).rgb, vec3<f32>(60000.0)), 1.0);
}

@fragment
fn probe(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(textureDimensions(depth_tex));
    let r = lf.params.z;
    // a disc three times the sun's (Vogel spiral, the middle counting more)
    var open = 0.0;
    var wsum = 0.0;
    var inner = vec3<f32>(0.0);
    for (var i = 0; i < INNER_N; i++) {
        let f = (f32(i) + 0.5) / f32(INNER_N);
        let a = f32(i) * GOLDEN;
        let t = tap(vec2<f32>(cos(a), sin(a)) * sqrt(f) * r, dims);
        let w = 1.0 - 0.5 * f;
        open += t.a * w;
        inner += t.rgb * w;
        wsum += w;
    }
    // the sky a few degrees out
    var ring = 0.0;
    var ring_n = 0.0;
    for (var i = 0; i < RING_N; i++) {
        let a = (f32(i) + 0.5) * 6.2831853 / f32(RING_N);
        let t = tap(vec2<f32>(cos(a), sin(a)) * r * 5.0, dims);
        ring += dot(t.rgb, LUMA);
        ring_n += t.a;
    }
    let vis = open / wsum;
    let lum_in = dot(inner, LUMA) / max(open, 1.0e-4);
    // (the ring all hidden: the sun peeks through a gap in the leaves, and shines)
    let lum_ring = select(min(lum_in * 0.003, 8.0), ring / max(ring_n, 1.0), ring_n > 0.5);
    // in frame the colour and clearness are measured and smoothed (half the last reading kept);
    // out of frame they ease towards the guess
    let inside = lf.params.w;
    let keep = mix(0.94, 0.5, inside);
    if in.position.x < 1.0 {
        let chroma = min(inner / max(dot(inner, LUMA), 1.0e-6), vec3<f32>(4.0));
        return vec4<f32>(mix(lf.prior.rgb, select(lf.prior.rgb, chroma, open > 0.5), inside), keep);
    }
    if in.position.x < 2.0 {
        // what is in view, and the glare's scale: the sky's brightness a few degrees from the sun,
        // as exposed (glare is the sun's light: it brightens and darkens with the exposure like the
        // sky round it); out of frame both are read from the edge nearest the sun
        return vec4<f32>(vis, min(lum_ring, 2000.0), 0.0, 0.5);
    }
    // a clear sun outshines the sky round it hundreds of times, through thin cloud a few times,
    // behind cloud about once (no open sky to measure: keep the last reading)
    let clear = smoothstep(1.0, 4.0, log2(max(lum_in, 1.0e-6) / max(lum_ring, 1.0e-6)));
    let measured = select(lf.prior.w, clear, open > 0.5);
    return vec4<f32>(mix(lf.prior.w, measured, inside), 0.0, 0.0, select(0.97, keep, open > 0.5));
}

// signed distance to a hexagon of inradius r (Inigo Quilez)
fn hexagon(p: vec2<f32>, r: f32) -> f32 {
    let k = vec3<f32>(-0.866025404, 0.5, 0.577350269);
    var q = abs(p);
    q -= 2.0 * min(dot(k.xy, q), 0.0) * k.xy;
    q -= vec2<f32>(clamp(q.x, -k.z * r, k.z * r), r);
    return length(q) * sign(q.y);
}

// an aperture image at signed distance `sd` from its edge (size r): a soft fill, a brighter rim
fn aperture(sd: f32, r: f32, rim: f32) -> f32 {
    let fill = 1.0 - smoothstep(-0.5 * r, 0.03 * r, sd);
    let edge = exp(-(sd * sd) / (0.0025 * r * r));
    return fill * (1.0 - rim) + edge * rim;
}

// one ghost centred at `c`, radius r: hex 0 a disc … 1 the hexagonal aperture; red, green and blue
// at slightly different sizes (the glass's dispersion fringes its edge)
fn ghost(p: vec2<f32>, c: vec2<f32>, r: f32, hex: f32, rim: f32) -> vec3<f32> {
    let d = p - c;
    if dot(d, d) > r * r * 1.7 {
        return vec3<f32>(0.0);
    }
    // (the blades turned 15°)
    let q = mat2x2<f32>(0.9659, 0.2588, -0.2588, 0.9659) * d;
    let l = length(d);
    let rr = r * 1.03;
    let rb = r * 0.97;
    return vec3<f32>(
        aperture(mix(l - rr, hexagon(q, rr), hex), rr, rim),
        aperture(mix(l - r, hexagon(q, r), hex), r, rim),
        aperture(mix(l - rb, hexagon(q, rb), hex), rb, rim),
    );
}

// a thin ring ghost, its colours split wider (a rainbow edge)
fn ring_ghost(p: vec2<f32>, c: vec2<f32>, r: f32, w: f32) -> vec3<f32> {
    let l = length(p - c);
    if abs(l - r) > w * 4.0 + r * 0.05 {
        return vec3<f32>(0.0);
    }
    let e = vec3<f32>(l - r * 1.035, l - r, l - r * 0.965) / w;
    return exp(-e * e);
}

@fragment
fn flare(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let chroma = textureLoad(probe_tex, vec2<i32>(0, 0), 0).rgb;
    let p1 = textureLoad(probe_tex, vec2<i32>(1, 0), 0);
    let shine = p1.r * textureLoad(probe_tex, vec2<i32>(2, 0), 0).r;
    let scale = p1.g;
    if shine < 1.0e-3 {
        discard;
    }
    let aspect = lf.tint.w;
    // the sun's colour as drawn (orange low down), a little paler (the glare scatters all colours)
    let tint = mix(vec3<f32>(1.0), chroma, 0.7) * lf.tint.rgb;
    let p = (in.uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);
    let s = (lf.sun.xy - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);
    let d = p - s;
    // (an angle: the same glare at any field of view)
    let r = length(d) * lf.params.y;

    // veiling glare: a tight core round the disc, a wide soft halo, a faint veil over the frame
    let c1 = r / 0.012;
    let c2 = r / 0.09;
    let glare = 1.2 / (1.0 + c1 * c1) + 0.45 * pow(1.0 + c2 * c2, -1.3) + 0.04 * exp(-r * 2.0);
    // starburst: thin spikes from the aperture's six blades, a finer set between, uneven
    let a = atan2(d.y, d.x) + lf.params.x;
    let spikes = pow(abs(cos(a * 3.0)), 90.0) + 0.4 * pow(abs(cos(a * 9.0 + 0.6)), 180.0);
    let uneven = 0.7 + 0.3 * sin(a * 11.0 + 1.3) * sin(a * 5.0 + 0.2);
    let burst = spikes * uneven * exp(-r / 0.055) * smoothstep(0.0015, 0.006, r);
    var col = (glare + 0.25 * burst) * lf.sun.z;

    // ghosts on the line from the sun through the centre, at s · (1 − t): between the sun and the
    // centre for t < 1, beyond it for t > 1
    let g = lf.sun.w;
    var gh = vec3<f32>(0.0);
    if g > 1.0e-3 {
        gh += ghost(p, s * (1.0 - 0.38), 0.016, 1.0, 0.3) * vec3<f32>(1.0, 0.74, 0.44) * 0.12;
        gh += ghost(p, s * (1.0 - 0.64), 0.05, 0.0, 0.15) * vec3<f32>(0.45, 0.85, 0.8) * 0.035;
        gh += ghost(p, s * (1.0 - 1.22), 0.027, 1.0, 0.35) * vec3<f32>(0.8, 1.0, 0.5) * 0.09;
        gh += ring_ghost(p, s * (1.0 - 1.5), 0.075, 0.006) * vec3<f32>(0.9, 0.82, 1.0) * 0.026;
        gh += ghost(p, s * (1.0 - 1.78), 0.02, 1.0, 0.4) * vec3<f32>(0.75, 0.56, 1.0) * 0.1;
        gh += ghost(p, s * (1.0 - 2.15), 0.055, 0.6, 0.2) * vec3<f32>(0.45, 0.62, 1.0) * 0.035;
        gh += ghost(p, s * (1.0 - 2.7), 0.13, 0.0, 0.1) * vec3<f32>(1.0, 0.7, 0.45) * 0.014;
        // a faint wide ring round the sun itself
        gh += ring_ghost(p, s, 0.36, 0.012) * 0.016;
        gh *= g;
    }
    let out = (col * tint + gh * tint) * shine * scale;
    if max(out.r, max(out.g, out.b)) < 1.0e-4 {
        discard;
    }
    return vec4<f32>(out, 0.0);
}
