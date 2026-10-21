// A dungeon's portal (`crate::dungeons::portal`): a standing sheet of swirling liquid light in
// an arch — a bright churning core, slow spiral arms of plasma, a glowing rim, ripples running
// out from the middle and sparks drifting through it. Lit from within, in display units (it
// glows the same whatever the exposure; bloom spreads it).
//
// uv: 0..1 across the sheet (x) and bottom to top (y); the vertex colour's alpha fades the
// edge (the engine's own sheets; a model's sheet has none and stays whole).

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    mesh_view_bindings::{view, globals},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
};
import super::noise::{value_noise2, fbm2};

struct PortalParams {
    // the bright core colour (linear rgb), w: strength
    core: vec4<f32>,
    // the rim / plasma colour (linear rgb), w: opacity
    rim: vec4<f32>,
    // x: swirl speed, y: seed, z: aspect (height / width), w: time offset
    look: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> portal: PortalParams;

fn rot(v: vec2<f32>, a: f32) -> vec2<f32> {
    let s = sin(a);
    let c = cos(a);
    return vec2<f32>(v.x * c - v.y * s, v.x * s + v.y * c);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var uv = vec2<f32>(0.5);
    var edge = 1.0;
@if(VERTEX_UVS_A)
    uv = in.uv;
@if(VERTEX_COLORS)
    edge = in.color.a;
    let t = globals.time * portal.look.x + portal.look.w;
    let seed = u32(portal.look.y);
    // centred, corrected for the sheet's shape (the middle sits a little low, like a door's)
    let aspect = max(portal.look.z, 0.2);
    let p = (uv - vec2<f32>(0.5, 0.45)) * vec2<f32>(1.0, aspect);
    let r = length(p);
    let a = atan2(p.y, p.x);
    // churning liquid: noise in coordinates that turn faster towards the middle
    let q = rot(p, t * 0.8 / (0.2 + r * 2.2));
    let churn = fbm2(q * 4.0 + vec2<f32>(0.0, t * 0.3), 5u, seed + 7u);
    // three spiral arms wound by the radius, turning; their lanes darker
    let wind = a * 3.0 + r * 16.0 - t * 2.2 + churn * 3.0;
    let arms = pow(0.5 + 0.5 * sin(wind), 1.6);
    // ripples running out from the middle
    let ripples = 0.5 + 0.5 * sin(r * 30.0 - t * 4.5 + churn * 5.0);
    let fine = value_noise2(rot(p, -t * 0.5) * 16.0 + vec2<f32>(t * 1.1, 0.0), seed + 13u);
    // sparks: round specks drifting outwards
    let sp = rot(p, t * 0.12) * 22.0 + vec2<f32>(0.0, -t * 2.5);
    let cell = floor(sp);
    let h = fract(sin(dot(cell, vec2<f32>(12.9898, 78.233)) + f32(seed)) * 43758.5453);
    let in_cell = length(fract(sp) - vec2<f32>(0.5)) ;
    let spark = step(0.965, h) * (1.0 - smoothstep(0.06, 0.16, in_cell)) * (0.6 + 0.4 * sin(t * 7.0 + h * 40.0));

    let heart = exp(-r * r * 14.0);
    let body = mix(0.22, 1.0, arms) * (0.55 + 0.45 * churn) + ripples * 0.18 + fine * 0.12;
    var col = mix(portal.rim.rgb, portal.core.rgb, clamp(heart * 1.2 + arms * 0.35 * churn, 0.0, 1.0)) * body * 1.25;
    col += portal.core.rgb * heart * 1.1;
    col += portal.core.rgb * spark * 3.0;
    // the rim: brightest along the edge of the sheet
    let rim = pow(1.0 - clamp(edge, 0.0, 1.0), 2.0) + smoothstep(0.34, 0.5, r) * 0.5;
    col += portal.rim.rgb * rim * 1.6;
    let strength = portal.core.w;
    pbr_input.material.base_color = vec4<f32>(col * 0.04, 1.0);
    pbr_input.material.perceptual_roughness = 0.2;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.emissive = vec4<f32>(col * strength, 0.0);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    // see-through at the very edge and a touch in the swirl's darker lanes
    let alpha = clamp(smoothstep(0.0, 0.18, edge) * portal.rim.w * (0.86 + 0.14 * arms), 0.0, 1.0);
    out.color.a = alpha;
    return out;
}
