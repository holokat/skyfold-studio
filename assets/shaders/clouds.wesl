// Volumetric clouds, traced at half resolution after the opaque scene (see `clouds.rs`).
//
// Cumulus layer: Nubis-style density (baked cover map = the terrain's cloud-shadow function, a
// height profile by cloud type, Perlin-Worley base shape, Worley detail erosion), marched up to
// the opaque depth with a jittered start. Light: a short march toward the key light (the sun, or
// the moon at night), three multiple-scattering octaves with a two-lobe Henyey-Greenstein phase,
// sunset colours and the earth's shadow from Bevy's transmittance LUT, sky light from its
// sky-view LUT, aerial perspective from its aerial-view LUT. Cirrus: one textured shell.
// Cloud sea (`clouds/sea.rs`, off unless its extinction is set): a second, low layer marched
// the same way, a flat-bottomed deck with billowy tops in the valleys.
//
// Output: premultiplied in-scattered radiance (times exposure) + opacity, and the cloud depth.

import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import bevy_pbr::atmosphere::{
    bindings::{view, atmosphere, settings},
    functions::{sample_transmittance_lut, sample_sky_view_lut, sample_aerial_view_lut, direction_world_to_atmosphere, uv_to_ray_direction, uv_to_ndc},
    bruneton_functions::sample_transmittance_lut_segment,
};
import super::clouds_types::Clouds;
import super::noise::cloud_cover;

@group(1) @binding(0) var<uniform> cl: Clouds;
@group(1) @binding(1) var base_tex: texture_3d<f32>;
@group(1) @binding(2) var detail_tex: texture_3d<f32>;
@group(1) @binding(3) var cover_tex: texture_2d<f32>;
@group(1) @binding(4) var cirrus_tex: texture_2d<f32>;
@group(1) @binding(5) var smp: sampler;
@group(1) @binding(6) var depth_tex: texture_depth_2d;

const PI: f32 = 3.14159265;
const CLOUD_TILE: f32 = 32768.0;
const BASE_TILE: f32 = 4096.0;
const DETAIL_TILE: f32 = 512.0;
// the shapes evolve slowly; one base tile per hour of `Time::elapsed_secs_wrapped` (seamless)
const EVOLVE: f32 = 1.1378;
const MAX_DIST: f32 = 65000.0;
// energy of the multiply scattered light (the octaves alone lose most of it in thick cloud)
const MS_GAIN: f32 = 2.0;

fn remap01(v: f32, lo: f32, hi: f32) -> f32 {
    return clamp((v - lo) / (hi - lo), 0.0, 1.0);
}

fn hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * c, 1e-4), 1.5));
}

// Ray from radius r0 (cos zenith mu) against the sphere of radius R about the planet centre:
// (near, far) distances, near > far when it misses. Stable for large radii.
fn sphere(r0: f32, mu: f32, R: f32) -> vec2<f32> {
    let b = r0 * mu;
    let c = (r0 - R) * (r0 + R);
    let disc = b * b - c;
    if disc < 0.0 {
        return vec2<f32>(1.0, -1.0);
    }
    let s = sqrt(disc);
    let q = -(b + select(-s, s, b >= 0.0));
    if abs(q) < 1e-6 {
        return vec2<f32>(0.0, 0.0);
    }
    let t1 = q;
    let t2 = c / q;
    return vec2<f32>(min(t1, t2), max(t1, t2));
}

fn hits(h: vec2<f32>) -> bool {
    return h.x <= h.y && h.y > 0.0;
}

// Light from direction l reaching atmosphere-space point pa (sunset colours, earth's shadow).
fn light_at(pa: vec3<f32>, l: vec3<f32>) -> vec3<f32> {
    let r = length(pa);
    let mu = dot(l, pa / r);
    let k = atmosphere.inner_radius / r;
    let mu_h = -sqrt(max(1.0 - k * k, 0.0));
    let lit = smoothstep(mu_h - 0.004, mu_h + 0.012, mu);
    return sample_transmittance_lut(r, max(mu, mu_h + 0.001)) * lit;
}

fn cover_at(xz: vec2<f32>) -> f32 {
    let n = textureSampleLevel(cover_tex, smp, (xz + cl.offset.xy) / CLOUD_TILE, 0.0).r;
    return cloud_cover(n, cl.cover.x);
}

// Vertical profile in the layer (hf 0..1): flat bases; a cell's core reaches highest, so each
// cloud domes over its cell.
fn profile(hf: f32, w: f32) -> f32 {
    let ty = cl.cover.z;
    let top = mix(0.3, 1.0, w);
    return smoothstep(0.0, 0.1, hf) * (1.0 - smoothstep(top * mix(0.2, 0.5, ty), top, hf));
}

// Extra erosion growing toward a cloud's top: rounded shoulders instead of vertical walls.
fn shoulders(hf: f32, w: f32) -> f32 {
    let r = clamp(hf / mix(0.3, 1.0, w), 0.0, 1.0);
    return 0.22 * r * r;
}

// Cloud density 0..1 at p = (x, altitude, z). The shape noise fills the column envelope (cover
// and height profile) as a signed field: > 0 inside. Detail erosion carves its surface (billows
// on top, wisps underneath), then a steep "levels" ramp turns it into a crisp but soft-edged
// density. Without detail (light samples) the erosion is its mean, so both see the same cloud.
fn density(p: vec3<f32>, hf: f32, w: f32, detail: bool) -> f32 {
    let q = p.xz + cl.offset.xy;
    let evolve = cl.motion.w * EVOLVE;
    let b = textureSampleLevel(base_tex, smp, vec3<f32>(q.x, p.y + evolve, q.y) / BASE_TILE, 0.0);
    let wf = b.g * 0.625 + b.b * 0.25 + b.a * 0.125;
    let shape = remap01(b.r, wf - 1.0, 1.0);
    var raw = shape * profile(hf, w) - (1.0 - w) - shoulders(hf, w);
    if raw < -0.25 {
        return 0.0;
    }
    var m = 0.5;
    if detail {
        // domain warp: the base noise's low-frequency channels swirl the detail lookup, more at
        // the bottom (wisps) than in the billowy tops
        let swirl = (b.gb - 0.5) * (0.35 * (1.0 - 0.5 * hf));
        let n = textureSampleLevel(detail_tex, smp, vec3<f32>(q.x, p.y - evolve, q.y) / DETAIL_TILE + vec3<f32>(swirl.x, 0.0, swirl.y), 0.0);
        let df = n.r * 0.625 + n.g * 0.25 + n.b * 0.125;
        m = mix(df, 1.0 - df, clamp(hf * 6.0, 0.0, 1.0));
    }
    raw -= m * 0.24;
    return clamp(raw * 4.0, 0.0, 1.0);
}

// The cloud sea's values ride in the uniforms' spare channels (see `clouds/sea.rs`).
fn sea_on() -> bool {
    return cl.fill_ambient.w > 0.0 && cl.fill_color.w > cl.fill.w;
}

// The sea's cover: the baked field at two scales and angles averaged (no round cells), off the
// cumulus pattern, drifting at half the wind (every sea lookup drifts with offset / 2 and tiles).
// Not thresholded here: the coverage applies to the deck's whole field (`sea_density`).
fn sea_cover(xz: vec2<f32>) -> f32 {
    let q = (xz * 4.0 + cl.offset.xy * 2.0) / CLOUD_TILE;
    let n1 = textureSampleLevel(cover_tex, smp, q + vec2<f32>(0.37, 0.61), 0.0).r;
    let n2 = textureSampleLevel(cover_tex, smp, vec2<f32>(q.y, -q.x) * 1.5 + vec2<f32>(0.13, 0.29), 0.0).r;
    return 0.55 * n1 + 0.45 * n2;
}

// Sea density 0..1 (hf 0..1 in the deck). The deck is a field of the broad cover, the base shape
// and rounded Worley billows (a hundred-odd metres across): where it clears 1 - coverage there
// is cloud, and the more it clears, the higher that column's top, so the top heaves in billows
// and thins out into gaps. Flat, dense base (misty when soft); detail carves the surface.
fn sea_density(p: vec3<f32>, hf: f32, n: f32, detail: bool) -> f32 {
    let soft = cl.shade.w;
    let q = p.xz * 3.0 + cl.offset.xy * 1.5;
    let evolve = cl.motion.w * EVOLVE;
    let b = textureSampleLevel(base_tex, smp, vec3<f32>(q.x, p.y * 3.0 + evolve, q.y) / BASE_TILE, 0.0);
    let wf = b.g * 0.625 + b.b * 0.25 + b.a * 0.125;
    let shape = remap01(b.r, wf - 1.0, 1.0);
    let billows = b.g * 0.6 + b.b * 0.4;
    let f = 0.3 * n + 0.25 * shape + 0.45 * billows;
    var raw = f - (1.0 - cl.key_color.w) * 0.85 - 0.5 * pow(hf, 1.3) - (1.0 - smoothstep(0.0, mix(0.04, 0.4, soft), hf)) * 0.3;
    if raw < -0.2 {
        return 0.0;
    }
    var m = 0.5;
    if detail {
        let q2 = p.xz * 2.0 + cl.offset.xy;
        let swirl = (b.gb - 0.5) * (0.35 * (1.0 - 0.5 * hf));
        let dn = textureSampleLevel(detail_tex, smp, vec3<f32>(q2.x, p.y * 2.0 - evolve, q2.y) / DETAIL_TILE + vec3<f32>(swirl.x, 0.0, swirl.y), 0.0);
        let df = dn.r * 0.625 + dn.g * 0.25 + dn.b * 0.125;
        m = mix(df, 1.0 - df, clamp(hf * 4.0, 0.0, 1.0));
    }
    raw -= m * mix(0.12, 0.06, soft);
    return clamp(raw * mix(12.0, 3.0, soft), 0.0, 1.0);
}

// One cloud layer of the march: the cumulus or the sea, its band (altitude, m) and the extinction
// of its densest cloud (1/m).
struct Layer {
    sea: bool,
    base: f32,
    top: f32,
    sigma: f32,
}

fn cumulus_layer() -> Layer {
    return Layer(false, cl.layer.x, cl.layer.y, cl.cover.y);
}

fn sea_layer() -> Layer {
    return Layer(true, cl.fill.w, cl.fill_color.w, cl.fill_ambient.w);
}

fn layer_cover(ly: Layer, xz: vec2<f32>) -> f32 {
    if ly.sea {
        return sea_cover(xz);
    }
    return cover_at(xz);
}

fn layer_density(ly: Layer, p: vec3<f32>, hf: f32, w: f32, detail: bool) -> f32 {
    if ly.sea {
        return sea_density(p, hf, w, detail);
    }
    return density(p, hf, w, detail);
}

// Optical depth toward the light from p (cheap density, growing steps).
fn light_depth(ly: Layer, p: vec3<f32>, l: vec3<f32>) -> f32 {
    let H = ly.top - ly.base;
    var od = 0.0;
    var t = 0.0;
    let n = u32(cl.quality.y);
    for (var j = 0u; j < n; j++) {
        let dt = 45.0 * exp2(f32(j)) * (5.0 / cl.quality.y);
        let tm = t + dt * 0.5;
        t += dt;
        let ps = p + l * tm;
        let hf = (ps.y - ly.base) / H;
        if hf > 1.0 || hf < 0.0 {
            break;
        }
        let w = layer_cover(ly, ps.xz);
        if w > 0.003 {
            od += layer_density(ly, ps, hf, w, false) * dt;
        }
    }
    return od * ly.sigma;
}

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

// Start offset of the march: interleaved gradient noise in space, a golden-ratio sequence over
// the frames this pixel is traced (every fourth), so its samples spread evenly in time.
fn jitter(px: vec2<f32>, frame: f32) -> f32 {
    let n = fract(52.9829189 * fract(0.06711056 * px.x + 0.00583715 * px.y));
    return fract(n + 0.618034 * floor(frame / 4.0));
}

struct Cirrus {
    color: vec3<f32>,
    alpha: f32,
    t: f32,
}

// The cirrus veil: a thin textured shell, fibres filtered by the pixel footprint.
fn cirrus(d: vec3<f32>, cam: vec3<f32>, cam_a: vec3<f32>, r0: f32, mu: f32, rc: f32, amb: vec3<f32>, sky_far: vec3<f32>) -> Cirrus {
    var o: Cirrus;
    o.color = vec3<f32>(0.0);
    o.alpha = 0.0;
    o.t = 0.0;
    let amount = cl.cover.w;
    if amount <= 0.001 {
        return o;
    }
    let Re = atmosphere.inner_radius;
    let R = Re + cl.layer.z;
    let hs = sphere(r0, mu, R);
    var t = -1.0;
    if r0 < R {
        t = hs.y;
    } else if hits(hs) && hs.x > 0.0 {
        t = hs.x;
    }
    let hg_ = sphere(r0, mu, Re);
    if t <= 0.0 || (hits(hg_) && hg_.x > 0.0 && hg_.x < t) {
        return o;
    }
    let pa = cam_a + d * t;
    let up = pa / length(pa);
    let mu_l = abs(dot(d, up));
    let pw = cam + d * t;
    let q = (pw.xz + cl.offset.zw) / CLOUD_TILE;
    // pixel footprint on the shell (stretched along the view) -> mip level (1024 texels per tile)
    let px_angle = 2.0 / (view.clip_from_view[1][1] * cl.grid.y);
    let foot = t * px_angle / max(mu_l, 0.05);
    let lod = log2(max(foot / (CLOUD_TILE / 1024.0), 1.0));
    let c1 = textureSampleLevel(cirrus_tex, smp, q, lod);
    let c2 = textureSampleLevel(cirrus_tex, smp, vec2<f32>(q.y, -q.x) * 3.0 + 0.37, lod + 1.585);
    let fib = mix(c1.r, c1.b, smoothstep(0.35, 0.65, c2.g));
    let veil = c1.g * 0.7 + c2.g * 0.3;
    let cov = smoothstep(1.0 - amount, 1.0 - amount + 0.4, veil);
    let tau = amount * cov * (0.1 + 0.9 * fib * fib) * 0.4;
    var alpha = (1.0 - exp(-tau / max(mu_l, 0.15))) * (1.0 - smoothstep(120000.0, 260000.0, t));
    // fade the veil away while the camera flies through it
    alpha *= smoothstep(300.0, 1500.0, abs(r0 - R));
    if alpha < 1e-4 {
        return o;
    }
    // ice crystals: strong forward peak, a faint 22 degree halo, some backscatter; plus sky light
    let c = dot(d, cl.key.xyz);
    let hd = (acos(clamp(c, -1.0, 1.0)) - 0.384) / 0.02;
    let halo = exp(-hd * hd) * 0.03;
    let phase = hg(c, 0.75) * 0.45 + hg(c, 0.2) * 0.35 + hg(c, -0.2) * 0.2 + halo;
    let e = light_at(pa, cl.key.xyz) * cl.key_color.rgb;
    let lc = (e * phase + amb * 0.9 * cl.shade.x) * alpha;
    let tr = sample_transmittance_lut_segment(rc, mu, t);
    o.color = lc * tr + sky_far * alpha * (1.0 - (tr.r + tr.g + tr.b) / 3.0);
    o.alpha = alpha;
    o.t = t;
    return o;
}

// Where the ray (from radius r0, cos zenith mu) is inside the shells rb..rt: (start, end),
// end < start when it misses; it ends at t_end (the scene, the ground, the march's reach).
fn segment(r0: f32, mu: f32, rb: f32, rt: f32, t_end: f32) -> vec2<f32> {
    let hb = sphere(r0, mu, rb);
    let ht = sphere(r0, mu, rt);
    var t0 = 0.0;
    var t1 = -1.0;
    if r0 < rb {
        t0 = hb.y;
        t1 = ht.y;
    } else if r0 < rt {
        t1 = ht.y;
        if hits(hb) && hb.x > 0.0 {
            t1 = hb.x;
        }
    } else if hits(ht) && ht.x > 0.0 {
        t0 = ht.x;
        t1 = ht.y;
        if hits(hb) && hb.x > 0.0 {
            t1 = hb.x;
        }
    }
    return vec2<f32>(t0, min(t1, t_end));
}

// Clouds right round the camera fade out (the sea's within tens of metres, `sea::NEAR_FADE`).
// From inside or under the sea (walking a valley floor) it is ground fog instead: thin round the
// walker, so nearby plants stay clear, thickening over a few hundred metres into the bank.
fn near_fade(ly: Layer, t: f32, r0: f32) -> f32 {
    if ly.sea {
        let inside = 1.0 - smoothstep(ly.top - 40.0, ly.top + 10.0, r0 - atmosphere.inner_radius);
        let mist = mix(0.15, 1.0, smoothstep(80.0, 700.0, t));
        return smoothstep(mix(20.0, 2.0, inside), mix(60.0, 20.0, inside), t) * mix(1.0, mist, inside);
    }
    return smoothstep(cl.layer.w * 0.6, cl.layer.w, t);
}

// What the march has gathered so far: in-scattered light, transmittance and the cloud depth's
// weighted sum.
struct March {
    L: vec3<f32>,
    T: f32,
    t_acc: f32,
    w_acc: f32,
}

// Phases of the scattering octaves toward the key light (k) and the second light (f).
struct Phases {
    k0: f32,
    k1: f32,
    k2: f32,
    f0: f32,
    f1: f32,
}

// Marches one layer along the ray, carrying on from what is in front of it.
fn march(m_in: March, ly: Layer, r0: f32, mu: f32, t_end: f32, cam: vec3<f32>, cam_a: vec3<f32>, d: vec3<f32>, px: vec2<f32>, amb_top: vec3<f32>, ph: Phases) -> March {
    let Re = atmosphere.inner_radius;
    let key = cl.key.xyz;
    let fill = cl.fill.xyz;
    var T = m_in.T;
    var L = m_in.L;
    var t_acc = m_in.t_acc;
    var w_acc = m_in.w_acc;
    let rb = Re + ly.base;
    let rt = Re + ly.top;
    let tt = segment(r0, mu, rb, rt, t_end);
    let t0 = tt.x;
    let t1 = tt.y;
    let seg = t1 - t0;
    if seg > 0.0 {
        // light at the bottom and the top of the layer where the ray crosses it
        let pm = cam_a + d * (0.5 * (t0 + t1));
        let upm = pm / length(pm);
        let k_bot = light_at(upm * rb, key) * cl.key_color.rgb;
        let k_top = light_at(upm * rt, key) * cl.key_color.rgb;
        let f_bot = light_at(upm * rb, fill) * cl.fill_color.rgb;
        let f_top = light_at(upm * rt, fill) * cl.fill_color.rgb;
        // light bounced off the ground below the clouds
        let ground = (light_at(upm * (Re + 1.0), key) * cl.key_color.rgb * max(dot(key, upm), 0.0)
            + light_at(upm * (Re + 1.0), fill) * cl.fill_color.rgb * max(dot(fill, upm), 0.0)) * (0.2 / PI);
        let amb_bot = amb_top * 0.5 + ground;

        let H = ly.top - ly.base;
        let sigma = ly.sigma;
        let n = clamp(seg / 55.0, 24.0, cl.quality.x);
        let ds0 = seg / n;
        var t = t0 + ds0 * jitter(px, cl.camera.w);
        // coarse steps through empty space; entering a cloud backs up one step and marches it
        // finely, so the light comes from its lit surface rather than from a sample deep inside
        var fine = 0;
        for (var i = 0; i < 256; i++) {
            if t >= t1 || T < 0.01 {
                break;
            }
            let coarse = ds0 * (1.0 + t / 30000.0);
            let ds = select(coarse, max(coarse * 0.18, 16.0), fine > 0);
            let pa = cam_a + d * t;
            let h = length(pa) - Re;
            let hf = (h - ly.base) / H;
            let pw = cam + d * t;
            var dens = 0.0;
            var p = vec3<f32>(0.0);
            if hf >= 0.0 && hf <= 1.0 {
                let w = layer_cover(ly, pw.xz);
                if w > 0.003 {
                    p = vec3<f32>(pw.x, h, pw.z);
                    dens = layer_density(ly, p, hf, w, true) * near_fade(ly, t, r0);
                }
            }
            if dens > 0.001 {
                if fine == 0 && t - coarse > t0 {
                    fine = 10;
                    t -= coarse;
                    continue;
                }
                fine = 10;
                // extinction LOD: steps grow with distance, so far clouds get a softer optical
                // depth (their light then comes from near the lit surface, as it should)
                let lod = 1.0 / (1.0 + t / 12000.0);
                let st = dens * sigma * lod;
                let od = light_depth(ly, p, key) * lod;
                // scattering orders: each sees less extinction, carries less energy and scatters
                // more evenly (the last one isotropic)
                // the last term is light diffusing through the whole cloud (falls off ~1/(1 + k tau))
                let ms = ph.k0 * exp(-od) + MS_GAIN * (0.85 * ph.k1 * exp(-od * 0.3) + 0.72 * ph.k2 * exp(-od * 0.1) + 0.048 / (1.0 + od * 0.12));
                let msf = ph.f0 * exp(-od) + MS_GAIN * (0.85 * ph.f1 * exp(-od * 0.3) + 0.05 * exp(-od * 0.05));
                let amb = mix(amb_bot, amb_top, clamp(hf * 1.3, 0.0, 1.0)) * (1.0 - 0.4 * clamp(dens * 1.5, 0.0, 1.0));
                let lum = mix(k_bot, k_top, hf) * ms + mix(f_bot, f_top, hf) * msf + amb;
                let s = st * lum;
                let tr = exp(-st * ds);
                L += T * (s - s * tr) / max(st, 1e-7);
                let dt = T * (1.0 - tr);
                t_acc += t * dt;
                w_acc += dt;
                T *= tr;
            } else if fine > 0 {
                fine -= 1;
            }
            t += ds;
        }
    }
    var m: March;
    m.T = T;
    m.L = L;
    m.t_acc = t_acc;
    m.w_acc = w_acc;
    return m;
}

struct TraceOut {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> TraceOut {
    var out: TraceOut;
    // this frame traces one pixel of every 2x2 block of the history
    let hp = floor(in.position.xy) * 2.0 + cl.grid.zw;
    let px = hp + 0.5;
    let uv = px / cl.grid.xy;
    let d = normalize(uv_to_ray_direction(uv));
    let cam = view.world_position;
    let Re = atmosphere.inner_radius;
    let cam_a = (atmosphere.world_to_atmosphere * vec4<f32>(cam, 1.0)).xyz;
    let r0 = length(cam_a);
    let up0 = cam_a / r0;
    let mu = dot(d, up0);
    let rc = max(r0, Re + 1.0);

    // the opaque scene (farthest of the corner view pixels under this texel)
    let full = vec2<i32>(cl.size.zw);
    let sc = i32(cl.quality.w);
    let fp = vec2<i32>(px) * sc;
    var dmin = 1.0;
    for (var k = 0; k < 4; k++) {
        let o = vec2<i32>(k & 1, k >> 1u) * (sc - 1);
        dmin = min(dmin, textureLoad(depth_tex, clamp(fp + o, vec2<i32>(0), full - 1), 0));
    }
    var t_scene = 1e10;
    if dmin > 0.0 {
        let vp = view.view_from_clip * vec4<f32>(uv_to_ndc(uv), dmin, 1.0);
        t_scene = length(vp.xyz / vp.w);
    }

    let hgr = sphere(r0, mu, Re);
    var t_ground = 1e10;
    if hits(hgr) && hgr.x > 0.0 {
        t_ground = hgr.x;
    }
    let t_end = min(min(t_scene, t_ground), MAX_DIST);

    let key = cl.key.xyz;
    let fill = cl.fill.xyz;
    let ck = dot(d, key);
    let cf = dot(d, fill);
    // phase of the three scattering octaves (eccentricity halves each order)
    let pk0 = mix(hg(ck, 0.8), hg(ck, -0.25), 0.3);
    let pk1 = mix(hg(ck, 0.48), hg(ck, -0.15), 0.3);
    let pk2 = mix(hg(ck, 0.29), hg(ck, -0.09), 0.3);
    let pf0 = mix(hg(cf, 0.8), hg(cf, -0.25), 0.3);
    let pf1 = mix(hg(cf, 0.48), hg(cf, -0.15), 0.3);

    let amb_sky = sky_light(rc) * cl.shade.x;
    let amb_top = amb_sky + cl.fill_ambient.rgb;
    let d_as = direction_world_to_atmosphere(d);
    let sky_far = sample_sky_view_lut(rc, d_as);

    var m: March;
    m.T = 1.0;
    m.L = vec3<f32>(0.0);
    m.t_acc = 0.0;
    m.w_acc = 0.0;
    let ph = Phases(pk0, pk1, pk2, pf0, pf1);
    let cu = cumulus_layer();
    if sea_on() {
        // front to back: the sea first from inside or below it, the cumulus first from above
        let sl = sea_layer();
        if r0 < Re + sl.top {
            m = march(m, sl, r0, mu, t_end, cam, cam_a, d, px, amb_top, ph);
            m = march(m, cu, r0, mu, t_end, cam, cam_a, d, px, amb_top, ph);
        } else {
            m = march(m, cu, r0, mu, t_end, cam, cam_a, d, px, amb_top, ph);
            m = march(m, sl, r0, mu, t_end, cam, cam_a, d, px, amb_top, ph);
        }
    } else {
        m = march(m, cu, r0, mu, t_end, cam, cam_a, d, px, amb_top, ph);
    }
    let T = m.T;
    let L = m.L;
    let t_acc = m.t_acc;
    let w_acc = m.w_acc;

    var col = vec3<f32>(0.0);
    var alpha = 0.0;
    var depth = 20000.0;
    if w_acc > 1e-5 {
        let tc = t_acc / w_acc;
        depth = tc;
        // what little light the march left through dense cloud would let the sun's disc shine
        // through: dense cloud is opaque
        alpha = select(1.0 - T, 1.0, T < 0.02);
        let tr = sample_transmittance_lut_segment(rc, mu, tc);
        let amax = settings.aerial_view_lut_max_distance;
        let ins = mix(sample_aerial_view_lut(uv, min(tc, amax)), sky_far, clamp((tc - amax) / (2.5 * amax), 0.0, 1.0));
        col = L * tr + ins * alpha;
        // distant clouds melt into the bright horizon haze, then fade out
        col = mix(col, sky_far * alpha * 1.1, smoothstep(6000.0, 50000.0, tc) * 0.8);
        let fade = 1.0 - smoothstep(30000.0, MAX_DIST, tc);
        col *= fade;
        alpha *= fade;
    }

    // the thin high veil gets only real sky light: the artificial night fill (there to keep low
    // clouds readable) would light it up as pale daytime-blue patches against the stars
    let ci = cirrus(d, cam, cam_a, r0, mu, rc, amb_sky, sky_far);
    if ci.alpha > 0.0 {
        if r0 < Re + cl.layer.z {
            col += (1.0 - alpha) * ci.color;
        } else {
            col = ci.color + (1.0 - ci.alpha) * col;
        }
        if alpha < 0.02 {
            depth = ci.t;
        }
        alpha = alpha + (1.0 - alpha) * ci.alpha;
    }
    col *= view.exposure;
    if any(col != col) || alpha != alpha {
        col = vec3<f32>(0.0);
        alpha = 0.0;
    }
    out.color = vec4<f32>(max(col, vec3<f32>(0.0)), clamp(alpha, 0.0, 1.0));
    out.depth = depth;
    return out;
}
