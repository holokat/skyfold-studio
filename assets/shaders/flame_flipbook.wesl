// Fire (`crate::flames`): AetherFX's flame effects baked to looping flipbooks
// (`tools/flames/bake_flames.py`), played on a sheet facing the eye. Two neighbouring frames are
// blended for smooth motion; each fire runs at its own phase and pace and some burn mirrored, so
// no two alike. The sheet holds premultiplied colour and coverage: the fire adds its light and
// hides a little of what's behind its heart. Fires of a magic colour (a green torch) take their
// light's colour; ordinary ones keep the simulation's own. In display units (it glows the same at
// any exposure; bloom spreads it). Soft where it meets walls and floors, thin right at the eye.
//
// A wide fire (a pit, a bowl, a campfire) is one flame as wide as its bed: its sheet is `spread`
// times wider than the baked frame and the flame is stretched across all of it at the foot,
// narrowing back to its own width as it rises.
//
// uv: x across the sheet, y from its foot (0) to the top (1); vertex colour r: the fire's seed,
// g: its spread / 4 (1 = the baked width).

import bevy_pbr::render::{
    mesh_view_bindings::{view, globals},
    forward_io::{VertexOutput, FragmentOutput},
    view_transformations::depth_ndc_to_view_z,
};
import super::volumetrics_fog::froxel_fog;

struct FlipbookParams {
    // x columns, y rows, z frames, w frames per second
    grid: vec4<f32>,
    // the light's colour (linear rgb), w: how much the fire takes it (0: its own colours)
    tint: vec4<f32>,
    // x strength, y how much it hides behind it, z soft edge against walls (m), w spare
    look: vec4<f32>,
    // the volumetric light's volume (`volumetrics::FroxelFog::params`; w 0: none)
    fog: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> fb: FlipbookParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var sheet: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var sheet_sampler: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var fog_tex: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var fog_smp: sampler;

// How far (m, along the view axis) the opaque scene lies behind this fragment.
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

fn cell(i: f32, st: vec2<f32>) -> vec2<f32> {
    let cols = fb.grid.x;
    let c = vec2<f32>(i - floor(i / cols) * cols, floor(i / cols));
    return (c + st) / fb.grid.xy;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var uv = vec2<f32>(0.5);
    var seed = 0.0;
    var spread = 1.0;
@if(VERTEX_UVS_A) {
    uv = in.uv;
}
@if(VERTEX_COLORS) {
    seed = in.color.r;
    spread = max(in.color.g * 4.0, 1.0);
}
    // its own phase and pace (±8 %), half of them mirrored
    let frames = fb.grid.z;
    let pace = 1.0 + (fract(seed * 13.71) - 0.5) * 0.16;
    let f = globals.time * fb.grid.w * pace + seed * frames;
    let i0 = f - floor(f / frames) * frames;
    let a = floor(i0);
    let b = select(a + 1.0, 0.0, a + 1.0 >= frames);
    let k = i0 - a;
    // the flame's width at this height (× the baked): all of the spread at the foot, its own
    // width by three quarters of the way up
    let rise = smoothstep(0.06, 0.78, uv.y);
    let wide = mix(spread, 1.0, rise);
    var u = 0.5 + (clamp(uv.x, 0.0, 1.0) - 0.5) * spread / wide;
    let inside = smoothstep(0.0, 0.04, u) * smoothstep(1.0, 0.96, u);
    u = clamp(u, 0.0, 1.0);
    if fract(seed * 7.31) > 0.5 {
        u = 1.0 - u;
    }
    // (image rows run down from the top; half a texel in from the frame's edges)
    let px = 0.5 / vec2<f32>(textureDimensions(sheet)) * fb.grid.xy;
    let st = clamp(vec2<f32>(u, 1.0 - clamp(uv.y, 0.0, 1.0)), px, vec2<f32>(1.0) - px);
    let c = mix(textureSample(sheet, sheet_sampler, cell(a, st)), textureSample(sheet, sheet_sampler, cell(b, st)), k);
    // a magic fire's colour: its heat ramps from the light's colour to white
    let heat = max(c.r, max(c.g, c.b));
    let magic = fb.tint.rgb * heat * 1.2 + vec3<f32>(pow(heat, 3.0) * 0.5);
    let col = mix(c.rgb, magic, fb.tint.w);
    // soft against the walls and the edges of the frame, thin right at the eye
    let edge = smoothstep(0.0, 0.06, uv.x) * smoothstep(1.0, 0.94, uv.x) * smoothstep(1.0, 0.9, uv.y);
    let soft = smoothstep(0.0, max(fb.look.z, 0.01), behind(in.position));
    let near = smoothstep(0.12, 0.5, length(view.world_position - in.world_position.xyz));
    let m = edge * inside * soft * near;
    var out: FragmentOutput;
    out.color = vec4<f32>(col * fb.look.x * m, clamp(c.a * fb.look.y * m, 0.0, 1.0));
    // the haze or fog between the eye and the fire (premultiplied: dimmed, and the haze it hides lit)
    let fog = froxel_fog(fog_tex, fog_smp, fb.fog, in.position, in.world_position.xyz);
    out.color = vec4<f32>(out.color.rgb * fog.a + fog.rgb * out.color.a, out.color.a);
    return out;
}
