// Road surface: a ribbon draped on the terrain (or a stone bridge deck) with a photoscanned
// surface from the terrain library (albedo + height, normal XY + roughness + AO). The soft edge
// breaks up along the texture's own stones and pebbles and a slow noise, so the road sits in the
// ground instead of ending in a stripe. Cart ruts, wear, wetness (rain, shorelines, puddles in
// the ruts) and cloud shadows follow the terrain shader so both light the same.
//
// Vertex inputs: uv = (across, along) in metres, uv_b = (across / half width, end fade 0..1),
// colour = (seed, half width (narrowing on the rounded end caps), 1 on bridge decks, 1),
// tangent = the across direction.
// The vertex shader pulls the surface a little towards the camera along the view ray (more with
// distance): that changes only its depth, never where it is drawn, so it never z-fights with the
// terrain and stays on top of coarser distant terrain LODs.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_functions,
    view_transformations::position_world_to_clip,
};
import super::noise::{value_noise2, cloud_density};
import super::rain::{puddle_amount, rain_ripples};

struct RoadParams {
    // x: texture repeat (m), y: edge softness (m), z: edge breakup (m), w: cart ruts (0..1)
    surface: vec4<f32>,
    // x: normal strength, y: texture softening (mip bias), z: rain wetness, w: depth pull per metre
    look: vec4<f32>,
    // xyz: direction towards the sun, w: cloud shadow strength
    sun: vec4<f32>,
    // x: cloud coverage, y: cloud scale, zw: cloud offset (as the terrain's)
    clouds: vec4<f32>,
    // x: sea level, y: anti-tiling (0..1), z: rain puddle fill, w: rain falling (ripples)
    misc: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> road: RoadParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var albedo_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var road_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var detail_tex: texture_2d<f32>;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0)).xyz;
    let to_cam = view.world_position - wp;
    let dist = max(length(to_cam), 1e-3);
    wp += to_cam / dist * min(0.015 + dist * road.look.w, dist * 0.25);
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, v.tangent, vertex_in.instance_index);
@if(VERTEX_COLORS)
    out.color = v.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

struct Surf {
    a: vec4<f32>,
    d: vec4<f32>,
}

fn sample_surf(uv: vec2<f32>) -> Surf {
    var o: Surf;
    o.a = textureSampleBias(albedo_tex, road_sampler, uv, road.look.y);
    o.d = textureSampleBias(detail_tex, road_sampler, uv, road.look.y);
    return o;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let wp = in.world_position.xyz;
    var uv = vec2<f32>(0.0);
    var ub = vec2<f32>(0.0, 1.0);
    var col = vec4<f32>(0.5, 2.0, 0.0, 1.0);
@if(VERTEX_UVS_A)
    uv = in.uv;
@if(VERTEX_UVS_B)
    ub = in.uv_b;
@if(VERTEX_COLORS)
    col = in.color;
    let hw = max(col.y, 0.02);
    let deck = col.z;
    let tile = road.surface.x;

    // ---- surface: the scan, plus a shifted copy blended in by a slow noise where its stones
    // stand higher (hides the repeat along kilometres of road)
    let seed_off = vec2<f32>(col.x * 13.7, col.x * 7.3);
    var s = sample_surf(uv / tile + seed_off);
    let s2 = sample_surf(uv / tile * 1.07 + vec2<f32>(0.43, 0.61) + seed_off);
    let amt = road.misc.y * smoothstep(0.35, 0.75, value_noise2(wp.xz * 0.06 + col.x * 31.0, 53u));
    let h1 = s.a.a + 1.0 - amt;
    let h2 = s2.a.a + amt;
    let top = max(h1, h2) - 0.15;
    let w1 = max(h1 - top, 0.0);
    let w2 = max(h2 - top, 0.0);
    let f = w2 / max(w1 + w2, 1e-4);
    s.a = mix(s.a, s2.a, f);
    s.d = mix(s.d, s2.d, f);
    var albedo = s.a.rgb;
    let height = s.a.a;
    var nxy = s.d.xy * 2.0 - 1.0;
    var rough = s.d.z;
    let ao = s.d.w;

    // ---- wear: cart ruts (two worn, darker, smoother tracks), a lighter worn crown, dirt
    // gathering towards the edges; slow tonal variation along the road
    let across = uv.x / hw;
    var rut = 0.0;
    if road.surface.w > 0.0 && deck < 0.5 {
        let wob = (value_noise2(vec2<f32>(uv.y * 0.11, col.x * 17.0), 61u) - 0.5) * 0.12;
        let r1 = 1.0 - smoothstep(0.06, 0.2, abs(across - 0.45 - wob));
        let r2 = 1.0 - smoothstep(0.06, 0.2, abs(across + 0.45 - wob));
        rut = max(r1, r2) * road.surface.w * (0.6 + 0.4 * value_noise2(vec2<f32>(uv.y * 0.05, 3.0), 67u));
        albedo *= 1.0 - rut * 0.18;
        rough = mix(rough, rough * 0.8, rut);
        // flattened by the wheels: less relief in the tracks
        nxy *= 1.0 - rut * 0.5;
    }
    let edge_dirt = smoothstep(0.55, 1.05, abs(across));
    albedo *= 1.0 - edge_dirt * 0.12;
    let tone = value_noise2(wp.xz * 0.013 + col.x * 9.0, 71u) * 0.6 + value_noise2(wp.xz * 0.11, 73u) * 0.4;
    albedo *= 0.88 + 0.24 * tone;

    // ---- soft, ragged edge: stones (high in the height map) reach further than the gaps,
    // plus noise at two scales; the far ends fade out the same way
    let e = road.surface.y;
    let b = road.surface.z;
    let breakup = (height - 0.5) * b * 1.2 + (value_noise2(wp.xz / 1.7, 79u) - 0.5) * b * 1.3 + (value_noise2(wp.xz / 6.0, 83u) - 0.5) * b * 1.4;
    let dm = abs(uv.x) - hw;
    var alpha = 1.0 - smoothstep(-e * 0.5, e * 0.5, dm - breakup);
    alpha *= smoothstep(0.0, 1.0, ub.y * 1.15 + (height - 0.5) * 0.35 + (value_noise2(wp.xz / 1.3, 89u) - 0.5) * 0.3);
    alpha = clamp(alpha, 0.0, 1.0);

    // ---- wetness: shoreline, rain; puddles in the ruts and low spots when it rains
    let sea = road.misc.x;
    let shore_wet = (1.0 - smoothstep(0.0, 1.2, wp.y - sea)) * (1.0 - deck);
    let rain = road.look.z;
    let wet = clamp(max(shore_wet, rain), 0.0, 1.0);
    albedo *= mix(1.0, 0.55, wet);
    rough = mix(rough, 0.12, wet * 0.85);
    // puddles (rain.wesl; misc z: puddle fill, w: rain falling): in the gaps between the stones,
    // the ruts and the road's low spots; a dark mirror, rippled by the drops. (A uniform branch:
    // dry roads skip it.)
    var puddle = 0.0;
    var ripple = vec2<f32>(0.0);
    if road.misc.z > 0.0 && deck < 0.5 {
        let ny = normalize(in.world_normal).y;
        let low = (0.45 - height) * 1.5 + rut * 1.2 + edge_dirt * 0.4 - (1.0 - ny) * 25.0;
        let flat = smoothstep(0.93, 0.975, ny);
        puddle = puddle_amount(wp.xz, road.misc.z, low, flat) * alpha;
        albedo *= 1.0 - puddle * 0.5;
        rough = mix(rough, 0.02, puddle);
        nxy *= 1.0 - puddle;
        let dist = length(view.world_position - wp);
        if puddle > 0.0 && road.misc.w > 0.0 && dist < 45.0 {
            ripple = rain_ripples(wp.xz, globals.time, road.misc.w) * (1.0 - smoothstep(25.0, 45.0, dist));
        }
    }

    // ---- normal: tangent = across, bitangent = along
    let n_geo = normalize(in.world_normal);
    var n = n_geo;
@if(VERTEX_TANGENTS) {
    let t = normalize(in.world_tangent.xyz - n_geo * dot(in.world_tangent.xyz, n_geo));
    let bt = cross(n_geo, t) * in.world_tangent.w;
    n = normalize(n_geo + (t * nxy.x + bt * nxy.y) * road.look.x);
}
    if puddle > 0.0 {
        n = normalize(n - vec3<f32>(ripple.x, 0.0, ripple.y) * (0.12 * puddle));
    }

    pbr_input.material.base_color = vec4<f32>(albedo, alpha);
    pbr_input.material.perceptual_roughness = clamp(rough, 0.04, 1.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.diffuse_occlusion = min(pbr_input.diffuse_occlusion, vec3<f32>(ao)); // keeps the screen-space AO
    pbr_input.world_normal = n_geo;
    pbr_input.N = n;

    // ---- moving cloud shadows, exactly as on the terrain
    let sun_dir = normalize(road.sun.xyz);
    let to_cloud = (2000.0 - wp.y) / max(sun_dir.y, 0.05);
    let cloud_p = wp.xz + sun_dir.xz * to_cloud;
    let cloud = smoothstep(0.25, 0.7, cloud_density(cloud_p, road.clouds.x, road.clouds.zw));
    pbr_input.directional_shadow_factor = 1.0 - cloud * road.sun.w;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    out.color.a = alpha;
    return out;
}
