// Campfire smoke: ray-march the simulated smoke volume (R: extinction 1/m, G: light 0..1 from
// the volume's self-shadowing) inside its box. Ported from threejs-particle-fluids'
// GasVolumeRenderer; colours are scene radiance, scaled by the view exposure like PBR output.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};

struct SmokeParams {
    // xyz: volume min (world), w: steps
    min: vec4<f32>,
    // xyz: volume max (world), w: opacity
    max: vec4<f32>,
    color: vec4<f32>,
    shadow: vec4<f32>,
    // scattered firelight at one metre, before exposure
    fire: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> smoke: SmokeParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var volume: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var volume_samp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if smoke.max.w <= 0.0 {
        discard;
    }
    let cam = view.world_position;
    let dir = normalize(in.world_position.xyz - cam);
    // the simulated volume, and the render box that also covers the wind-bent plume
    let bmin = smoke.min.xyz;
    let bmax = smoke.max.xyz;
    let bend = vec2<f32>(smoke.color.w, smoke.shadow.w);
    let rmin = bmin + vec3<f32>(min(bend.x, 0.0), 0.0, min(bend.y, 0.0));
    let rmax = bmax + vec3<f32>(max(bend.x, 0.0), 0.0, max(bend.y, 0.0));
    let inv = 1.0 / (dir + vec3<f32>(1.0e-6));
    let t_near = (rmin - cam) * inv;
    let t_far = (rmax - cam) * inv;
    let lo = min(t_near, t_far);
    let hi = max(t_near, t_far);
    let t0 = max(max(max(lo.x, lo.y), lo.z), 0.0);
    let t1 = min(min(hi.x, hi.y), hi.z);
    if t1 <= t0 {
        discard;
    }
    let steps = max(smoke.min.w, 8.0);
    let step = (t1 - t0) / steps;
    let jitter = fract(sin(dot(in.position.xy, vec2<f32>(12.9898, 78.233))) * 43758.5453);
    var t = t0 + step * jitter;
    var trans = 1.0;
    var radiance = vec3<f32>(0.0);
    let extent = bmax - bmin;
    for (var i = 0; i < 64; i++) {
        if f32(i) >= steps || trans < 0.015 {
            break;
        }
        let world_p = cam + dir * t;
        var p = world_p;
        // undo the wind lean: it grows with height like a real plume
        let h = clamp((p.y - bmin.y) / extent.y, 0.0, 1.0);
        let lean = pow(h, 1.4);
        p = p - vec3<f32>(bend.x, 0.0, bend.y) * lean;
        let uvw = (p - bmin) / extent;
        if any(uvw < vec3<f32>(0.0)) || any(uvw > vec3<f32>(1.0)) {
            t += step;
            continue;
        }
        let s = textureSampleLevel(volume, volume_samp, uvw, 0.0).rg;
        let sigma = s.r * smoke.max.w;
        if sigma > 0.005 {
            let hearth = vec3<f32>((bmin.x + bmax.x) * 0.5, bmin.y + 0.25, (bmin.z + bmax.z) * 0.5);
            let fire_distance = length(world_p - hearth);
            let firelight = smoke.fire.rgb / max(fire_distance * fire_distance, 0.25);
            let shade = mix(smoke.shadow.rgb, smoke.color.rgb, clamp(s.g, 0.0, 1.0)) + firelight;
            let a = 1.0 - exp(-sigma * step);
            radiance += shade * a * trans;
            trans *= 1.0 - a;
        }
        t += step;
    }
    let alpha = 1.0 - trans;
    if alpha < 0.002 {
        discard;
    }
    return vec4<f32>(radiance / max(alpha, 1.0e-4) * view.exposure, alpha);
}
