// Chimney smoke (visuals/chimneys.rs): the puffs of one smoking chimney, forge or cauldron, camera-
// facing quads expanded here from the chimney's mouth (uv = corner; vertex colour = phase, seed,
// size at the mouth, opacity; normal = the column's height (m), the mouth's radius (m), its look +
// 0.9 × the puff's LOD rank: look 0 wood smoke, 1 steam, 2 forge smoke). Each puff rises on its own
// cycle, quick off the mouth and slowing as it cools, bends over downwind (more the stronger the
// wind, and the longer it has been aloft), swells and thins out until it is gone; steam is whiter
// and vanishes sooner.
//
// Lit by its own small model, not Bevy's full PBR (whose clustered-light and probe loops cost a
// lot over large, overlapping, alpha-blended puffs): the sun and moon through one tap of their
// shadow cascade, wrapped round a soft sphere and scattered forwards (`sun_scatter.wesl`: against a
// low sun the thin edges glow), the sky's fill from above, and the volumetric fog between the eye
// and it. Cheap where it costs: columns far off draw half their puffs (larger), puffs never grow
// past a share of their distance on screen, and the ones right at the eye fade away.

import bevy_pbr::render::{
    mesh_view_bindings::{view, lights},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    mesh_functions,
    view_transformations::{position_world_to_clip, position_world_to_view},
};
import super::sun_scatter::{sun_visibility, hg};
import super::volumetrics_fog::froxel_fog;

struct ChimneyParams {
    wind: vec4<f32>,  // xy: wind direction (xz), z: wind speed with its gusts (m/s), w: clock (s)
    look: vec4<f32>,  // x: thickness, y: fade-out starts (m from the eye), z: gone by (m), w: spare
    smoke: vec4<f32>, // rgb: wood smoke at the mouth (linear), a: grey level high up
    forge: vec4<f32>, // rgb: forge smoke at the mouth, a: grey level high up
    fog: vec4<f32>,   // `volumetrics::FroxelFog::params` (w 0: no volumetric light)
    sky: vec4<f32>,   // rgb: the sky's fill light (before exposure), w: spare
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> chimney: ChimneyParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var noise_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var fog_tex: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var fog_smp: sampler;

const PI: f32 = 3.14159265;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let src = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0)).xyz;
    var c = vec4<f32>(0.0, 0.5, 0.5, 1.0);
@if(VERTEX_COLORS) {
    c = v.color;
}
    var corner = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    corner = v.uv;
}
    var nrm = vec3<f32>(10.0, 0.3, 0.0);
@if(VERTEX_NORMALS) {
    nrm = v.normal;
}
    let height = max(nrm.x, 0.5);
    let radius = nrm.y;
    let look = floor(nrm.z + 0.01);
    let rank = (fract(nrm.z + 0.01) - 0.01) / 0.9;
    let steam = look > 0.5 && look < 1.5;
    // far off, half the puffs (the kept ones grow to cover for them)
    let dist = distance(src, view.world_position);
    let keep = 1.0 - 0.5 * smoothstep(70.0, 220.0, dist);
    if rank > keep {
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    let t = chimney.wind.w;
    let speed = chimney.wind.z;
    // a puff's life: up the column in ~1 m/s (steam quicker), less high in a strong wind
    let rise = height / select(1.0, 1.4, steam);
    let life = fract(t / rise + c.x);
    let sd = c.y;
    let h = height * (1.0 - pow(1.0 - life, 1.8)) / (1.0 + 0.1 * speed);
    // bent over downwind: the longer aloft, the further
    let wdir = vec3<f32>(chimney.wind.x, 0.0, chimney.wind.y);
    let drift = wdir * speed * rise * pow(life, 1.15) * 0.55;
    // turbulent wander growing with height, the column widening
    let spread = radius * 0.35 + height * 0.06 * life;
    let wander = vec3<f32>(sin(sd * 37.0 + t * 0.21 + life * 6.0), 0.35 * sin(sd * 11.0 + t * 0.17), cos(sd * 23.0 + t * 0.19 + life * 5.0)) * spread;
    var wp = src + vec3<f32>(0.0, h, 0.0) + drift + wander;
    // swelling as it rises and as the wind stretches it out (so a column laid over by a strong
    // wind stays a continuous trail, not a string of beads)
    var size = (c.z * (0.55 + 0.45 * sqrt(life)) + height * select(0.11, 0.07, steam) * life + length(drift) * 0.09) / sqrt(keep);
    // never more than a fifth of its distance across (a puff next to the eye would fill the view
    // with overlapping blended fragments), and gone right at the eye
    let eye = distance(wp, view.world_position);
    size = min(size, eye * 0.2);
    // face the camera, turning slowly so the noise inside rolls
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let a = sd * 6.2832 + t * (0.05 + sd * 0.08) * select(-1.0, 1.0, sd > 0.5);
    let q = vec2<f32>(corner.x * cos(a) - corner.y * sin(a), corner.x * sin(a) + corner.y * cos(a));
    wp = wp + (right * q.x + up * q.y) * size;
    // in at the mouth, thinning out as it rises (steam sooner); chimneys far off fade away
    let gone = select(1.0, 0.75, steam);
    let fade = smoothstep(0.0, 0.05, life) * (1.0 - smoothstep(0.2, gone, life)) * smoothstep(1.5, 4.0, eye);
    let far = 1.0 - smoothstep(chimney.look.y, chimney.look.z, dist);
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = normalize(view.world_position - wp);
@if(VERTEX_UVS_A)
    out.uv = corner;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(life, sd, c.w * fade * far, look);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var vc = vec4<f32>(0.5, 0.5, 1.0, 0.0);
@if(VERTEX_COLORS) {
    vc = in.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    let t = chimney.wind.w;
    let life = vc.x;
    let sd = vc.y;
    let look = vc.w;
    let steam = look > 0.5 && look < 1.5;
    // soft, billowing edges: two noise octaves rolling inside the puff (sampled before any
    // discard, where the control flow is still uniform)
    let n1 = textureSample(noise_tex, samp, uv * 0.3 + vec2<f32>(sd * 7.1, sd * 3.3 + t * 0.012)).r;
    let n2 = textureSample(noise_tex, samp, uv * 0.75 + vec2<f32>(sd * 2.9 + 0.5, sd * 5.3 - t * 0.02)).g;
    let r2 = dot(uv, uv);
    let body = (1.0 - r2) * (0.45 + 0.9 * (n1 * 0.65 + n2 * 0.35)) - 0.22;
    let dens = smoothstep(0.03, 0.45, body);
    let alpha = dens * vc.z * chimney.look.x;
    if alpha < 0.004 || r2 > 1.0 {
        discard;
    }
    // a soft sphere facing the eye
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let fwd = normalize(view.world_from_view[2].xyz);
    let N = normalize(right * uv.x + up * uv.y + fwd * sqrt(max(1.0 - r2, 0.0)) * 1.3 + vec3<f32>(n1 - 0.5, 0.0, n2 - 0.5) * 0.5);
    // greyer and denser at the mouth, paler as it rises and thins; steam white
    var base = mix(chimney.smoke.rgb, vec3<f32>(chimney.smoke.a), smoothstep(0.0, 0.5, life));
    if steam {
        base = vec3<f32>(0.82, 0.84, 0.86);
    } else if look > 1.5 {
        base = mix(chimney.forge.rgb, vec3<f32>(chimney.forge.a), smoothstep(0.0, 0.6, life));
    }
    base *= mix(0.8, 1.0, n1);
    // the sun (and the moon): one shadow tap, wrapped round the puff (light gets into smoke), and
    // scattered forwards through its thin parts
    let wp = in.world_position.xyz;
    let V = normalize(wp - view.world_position);
    let view_z = position_world_to_view(wp).z;
    var light_in = vec3<f32>(0.0);
    for (var i: u32 = 0u; i < lights.n_directional_lights; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = light.direction_to_light;
        let col = light.color.rgb * view.exposure * sun_visibility(i, wp, L, view_z);
        let wrap = max(dot(N, L) * 0.6 + 0.4, 0.0) / PI;
        light_in += col * (wrap + hg(0.65, dot(V, L)) * (1.0 - 0.6 * dens) * 0.9);
    }
    // the sky's fill, from above more than below
    let amb = (chimney.sky.rgb + lights.ambient_color.rgb) * view.exposure * mix(0.55, 1.0, smoothstep(-0.6, 0.5, N.y));
    let rgb = base * (light_in + amb);
    // the haze or fog between the eye and the puff (premultiplied)
    let f = froxel_fog(fog_tex, fog_smp, chimney.fog, in.position, wp);
    var out: FragmentOutput;
    out.color = vec4<f32>(rgb * alpha * f.a + f.rgb * alpha, alpha);
    return out;
}
