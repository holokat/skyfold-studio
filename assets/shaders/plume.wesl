// Smoke and steam plumes (visuals/volcano.rs): camera-facing puffs expanded here from where they
// leave the source (uv = corner, vertex colour = phase, seed, size, opacity). Each puff rises on its
// own cycle, slowing as the column spreads, bending downwind more the higher and older it gets,
// swelling and thinning out; it is lit through Bevy PBR as a soft sphere (sun side bright, the
// underside and the dense core darker), so the plume takes the scene's light, fog and night, and
// the lava below glows on its lowest puffs.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_functions,
    view_transformations::position_world_to_clip,
};

struct PlumeParams {
    wind: vec4<f32>,   // xy: wind direction (xz), z: wind speed (m/s), w: rise time of a puff (s)
    shape: vec4<f32>,  // x: column height, y: source radius, z: spread with height, w: opacity
    color: vec4<f32>,  // rgb: colour at the source, a: grey level high up
    glow: vec4<f32>,   // rgb: lava glow on the low puffs, w: night
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> plume: PlumeParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var noise_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var samp: sampler;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
    let src = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0)).xyz;
    var c = vec4<f32>(0.0, 0.5, 4.0, 1.0);
@if(VERTEX_COLORS) {
    c = v.color;
}
    var corner = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    corner = v.uv;
}
    let t = globals.time;
    let rise = max(plume.wind.w, 1.0);
    let life = fract(t / rise + c.x);
    let height = plume.shape.x;
    let sd = c.y;
    // a buoyant column: quick off the vent, slowing as it spreads
    let h = height * (1.0 - pow(1.0 - life, 1.7));
    // the wind bends it over: the longer a puff has been aloft, the further downwind
    let wdir = vec3<f32>(plume.wind.x, 0.0, plume.wind.y);
    let drift = wdir * plume.wind.z * life * rise * 0.32;
    // turbulent wander growing with height, and the column widening
    let spread = plume.shape.y * 0.25 + height * plume.shape.z * life * 0.55;
    let wander = vec3<f32>(sin(sd * 37.0 + t * 0.05 + life * 5.0), 0.0, cos(sd * 23.0 + t * 0.04 + life * 4.0)) * spread;
    var wp = src + vec3<f32>(0.0, h, 0.0) + drift + wander;
    let size = c.z * (0.45 + 0.55 * sqrt(life)) + height * plume.shape.z * life * 0.5;
    // face the camera, turning slowly so the noise inside rolls
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let a = sd * 6.2832 + t * (0.02 + sd * 0.03) * select(-1.0, 1.0, sd > 0.5);
    let q = vec2<f32>(corner.x * cos(a) - corner.y * sin(a), corner.x * sin(a) + corner.y * cos(a));
    wp = wp + (right * q.x + up * q.y) * size;
    let fade = smoothstep(0.0, 0.07, life) * (1.0 - smoothstep(0.5, 1.0, life));
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = normalize(view.world_position - wp);
@if(VERTEX_UVS_A)
    out.uv = corner;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(life, sd, c.w * fade, size);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var vc = vec4<f32>(0.5, 0.5, 1.0, 4.0);
@if(VERTEX_COLORS) {
    vc = in.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    let t = globals.time;
    let life = vc.x;
    let sd = vc.y;
    // billowing, cauliflower edges: two noise octaves rolling inside the puff
    let n1 = textureSample(noise_tex, samp, uv * 0.3 + vec2<f32>(sd * 7.1, sd * 3.3 + t * 0.006)).r;
    let n2 = textureSample(noise_tex, samp, uv * 0.75 + vec2<f32>(sd * 2.9 + 0.5, sd * 5.3 - t * 0.01)).g;
    let r2 = dot(uv, uv);
    let body = (1.0 - r2) * (0.5 + 0.95 * (n1 * 0.65 + n2 * 0.35)) - 0.2;
    let dens = smoothstep(0.04, 0.4, body);
    let alpha = dens * vc.z * plume.shape.w;
    if alpha < 0.003 || r2 > 1.0 {
        discard;
    }
    // a soft sphere facing the eye
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let fwd = normalize(view.world_from_view[2].xyz);
    let N = normalize(right * uv.x + up * uv.y + fwd * sqrt(max(1.0 - r2, 0.0)) * 1.2 + vec3<f32>(n1 - 0.5, 0.0, n2 - 0.5) * 0.6);
    // dark and dense off the vent, paler as it rises and thins; the undersides in shadow
    var base = mix(plume.color.rgb, vec3<f32>(plume.color.a), smoothstep(0.0, 0.55, life));
    base *= mix(0.55, 1.0, smoothstep(-0.6, 0.5, N.y)) * mix(0.7, 1.0, n1);
    pbr_input.material.base_color = vec4<f32>(base, alpha);
    pbr_input.material.perceptual_roughness = 1.0;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3<f32>(0.0);
    // the lava below lights the lowest puffs (most visible at night)
    // (only the undersides of the lowest puffs, right over the vent)
    let glow = plume.glow.rgb * pow(1.0 - smoothstep(0.0, 0.12, life), 2.0) * smoothstep(0.1, -0.7, N.y) * (0.1 + 0.9 * plume.glow.w) * dens;
    pbr_input.material.emissive = vec4<f32>(glow, 0.0);
    pbr_input.N = N;
    pbr_input.world_normal = N;
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
