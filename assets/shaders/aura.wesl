// Auras round models (auras.rs): a fixed pool of camera-facing quads per aura, every particle
// animated here from its seeds, so an aura costs no CPU work per particle and two draws.
//
// Vertex data: position = three seeds in the unit cube; vertex colour = (kind / 4, phase, size,
// seed); uv = the quad's corner. Kinds:
//   0 wisp: a puff of dark vapour rising from the ground round the model, curling inward and up
//     toward its crown, swelling and fading over its life; each life starts from a new spot;
//   1 mist: a big low puff creeping slowly round the model's foot;
//   2 mote: a purple spark spiralling up round the model, twinkling.
// The smoke layer (wisps and mist, alpha-blended) is lit by the scene like any surface (a soft
// ball facing the eye), glows faintly purple within and fades where it meets the ground or the
// model (soft particles, from the depth prepass) and near the eye. The glow layer (motes) is
// added to the scene. Everything runs on the aura's own clock, which stands still while the
// world's motion is frozen (editing from the overview).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    view_transformations::{position_world_to_clip, depth_ndc_to_view_z},
};

struct AuraParams {
    centre: vec4<f32>, // xyz: the anchor (world), w: radius (m)
    shape: vec4<f32>,  // x: height (m), y: strength (0 hides), z: layer (0 smoke and mist, 1 motes), w: clock (s)
    smoke: vec4<f32>,  // rgb: the vapour's colour (linear), w: its opacity
    glow: vec4<f32>,   // rgb: the glow (linear, display units), w: night 0..1
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> aura: AuraParams;

const TAU: f32 = 6.2831853;

fn hash1(x: f32) -> f32 {
    return fract(sin(x * 91.3458) * 43758.5453);
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = mix(hash2(i), hash2(i + vec2<f32>(1.0, 0.0)), u.x);
    let b = mix(hash2(i + vec2<f32>(0.0, 1.0)), hash2(i + vec2<f32>(1.0, 1.0)), u.x);
    return mix(a, b, u.y);
}

// How far (m, along the view axis) the opaque scene lies behind this fragment (huge where
// nothing is behind, or without a depth prepass).
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
    var c = vec4<f32>(0.0, 0.0, 1.0, 0.5);
@if(VERTEX_COLORS) {
    c = v.color;
}
    var corner = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    corner = v.uv;
}
    let s = v.position;
    let kind = u32(c.x * 4.0 + 0.5);
    let t = aura.shape.w;
    let R = aura.centre.w;
    let H = aura.shape.x;
    let C = aura.centre.xyz;
    var p = C;
    var size = 1.0;
    var alpha = 0.0;
    var stretch = 1.0;
    var spin = 0.0;
    if kind == 0u {
        // a wisp: up from the ground round the model, drawn inward and up toward its crown
        let rate = 1.0 / (10.0 + 6.0 * s.y);
        let k = t * rate + c.y;
        let life = fract(k);
        let gen = floor(k);
        let h1 = hash1(s.x * 13.7 + gen * 0.618);
        let h2 = hash1(s.z * 7.9 + gen * 0.371 + 3.1);
        let dir = select(-1.0, 1.0, s.z > 0.5);
        let a = h1 * TAU + dir * life * (1.1 + 0.8 * s.y);
        let rr = R * (0.4 + 0.6 * h2) * (1.0 - 0.6 * life) + 0.6 * sin(t * 0.3 + s.x * 20.0);
        let y = 0.6 + pow(life, 0.85) * H * (0.45 + 0.5 * s.y);
        p = C + vec3<f32>(cos(a) * rr, y, sin(a) * rr);
        size = (1.0 + 2.4 * life) * c.z;
        alpha = sin(life * 3.14159) * (0.6 + 0.4 * h2);
        stretch = 1.35;
        spin = s.x * TAU + t * (0.1 + 0.15 * s.y) * dir;
    } else if kind == 1u {
        // mist: big low puffs creeping round the foot, flat side down
        let dir = select(-1.0, 1.0, s.y > 0.5);
        let a = s.x * TAU + t * (0.025 + 0.02 * s.z) * dir;
        let rr = R * (0.7 + 0.6 * s.z) + 0.8 * sin(t * 0.13 + s.x * 9.0);
        let y = 0.35 + 1.1 * s.y * s.y + 0.25 * sin(t * 0.21 + s.z * 7.0);
        p = C + vec3<f32>(cos(a) * rr, y, sin(a) * rr);
        size = (2.6 + 2.4 * s.z) * c.z;
        alpha = 0.75 + 0.25 * sin(t * 0.3 + c.y * TAU);
        stretch = 0.5;
    } else {
        // a mote: a spark spiralling up round the model, twinkling
        let rate = 1.0 / (5.0 + 5.0 * s.y);
        let k = t * rate + c.y;
        let life = fract(k);
        let gen = floor(k);
        let h1 = hash1(s.x * 17.3 + gen * 0.618);
        let dir = select(-1.0, 1.0, s.z > 0.4);
        let a = h1 * TAU + dir * life * (3.0 + 3.0 * s.z);
        let rr = R * (0.3 + 0.75 * fract(h1 * 7.13)) * (1.0 - 0.45 * life);
        let y = 0.3 + life * H * (0.55 + 0.5 * s.y);
        p = C + vec3<f32>(cos(a) * rr, y, sin(a) * rr);
        size = 0.09 * c.z * (0.8 + 0.4 * sin(t * 3.0 + s.x * 30.0));
        alpha = sin(life * 3.14159) * (0.55 + 0.45 * sin(t * (2.0 + 3.0 * s.y) + s.z * 40.0));
    }
    // a camera-facing quad, turned by `spin`, stretched up (wisps) or flattened (mist)
    let right = normalize(view.world_from_view[0].xyz);
    let up = normalize(view.world_from_view[1].xyz);
    let cs = cos(spin);
    let sn = sin(spin);
    let q = vec2<f32>(corner.x * cs - corner.y * sn, corner.x * sn + corner.y * cs);
    var wp = p + (right * q.x + up * q.y * stretch) * size;
    if alpha <= 0.0 || aura.shape.y <= 0.0 {
        // nothing to draw: collapse the quad
        wp = p;
    }
    out.world_position = vec4<f32>(wp, 1.0);
    out.position = position_world_to_clip(wp);
    out.world_normal = normalize(view.world_position - wp);
@if(VERTEX_UVS_A)
    out.uv = corner;
@if(VERTEX_COLORS)
    out.color = vec4<f32>(c.x, size, max(alpha, 0.0), c.w);
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var vc = vec4<f32>(0.0, 1.0, 0.0, 0.5);
@if(VERTEX_COLORS) {
    vc = in.color;
}
    var uv = vec2<f32>(0.0);
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
    let size = vc.y;
    let seed = vc.w;
    let r2 = dot(uv, uv);
    let wp = in.world_position.xyz;
    let dist = length(view.world_position - wp);
    let strength = aura.shape.y;
    var out: FragmentOutput;
    if aura.shape.z > 0.5 {
        // motes: a bright core in a soft halo, added to the scene
        let g = exp(-r2 * 9.0) + 0.3 * exp(-r2 * 2.5);
        let a = g * vc.z * strength * (1.0 - smoothstep(0.75, 1.0, r2)) * smoothstep(0.4, 1.5, dist);
        out.color = vec4<f32>(aura.glow.rgb * a, a);
        return out;
    }
    // wisps and mist: a soft rolling puff of dark vapour
    let t = aura.shape.w;
    let n = noise2(uv * 1.3 + vec2<f32>(seed * 17.0, t * 0.05 + seed * 5.0)) * 0.65 + noise2(uv * 2.9 + vec2<f32>(seed * 7.0 - t * 0.07, seed * 3.0)) * 0.35;
    let body = (1.0 - r2) * (0.35 + 0.95 * n) - 0.2;
    let soft = smoothstep(0.0, 0.5 + size * 0.4, behind(in.position)) * smoothstep(1.5, 6.0, dist);
    let alpha = smoothstep(0.0, 0.65, body) * vc.z * aura.smoke.w * strength * soft;
    if alpha < 0.004 || r2 > 1.0 {
        discard;
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = vec4<f32>(aura.smoke.rgb, min(alpha, 1.0));
    pbr_input.material.perceptual_roughness = 1.0;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3<f32>(0.2);
    pbr_input.material.diffuse_transmission = 0.5;
    // lit as a soft ball facing the eye
    let right = normalize(view.world_from_view[0].xyz);
    let vup = normalize(view.world_from_view[1].xyz);
    let fwd = normalize(view.world_from_view[2].xyz);
    pbr_input.N = normalize(right * uv.x + vup * uv.y + fwd * sqrt(max(1.0 - r2, 0.0)) * 1.3);
    out.color = apply_pbr_lighting(pbr_input);
    // a faint purple light within, strongest in the thick of it and by night
    let inner = aura.glow.rgb * smoothstep(0.1, 0.8, body) * (0.012 + 0.045 * aura.glow.w) * strength;
    out.color = vec4<f32>(out.color.rgb + inner, out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
