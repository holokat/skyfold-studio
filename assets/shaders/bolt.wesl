// A lightning bolt (rain/lightning.rs): additive ribbons whose vertex colours fall from the core
// to black at their edges, times the flash. Drawn after the view was fogged, it dims itself by
// the volumetric light's transmittance in front of it (`volumetrics_fog`), never quite to nothing:
// a strike through rain still lights its channel.

import bevy_pbr::render::forward_io::VertexOutput;
import super::volumetrics_fog::froxel_fog;

struct BoltParams {
    // rgb: the core's colour × the flash now
    color: vec4<f32>,
    // `volumetrics::FroxelFog::params` (w 0: no volumetric light)
    fog: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> bolt: BoltParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var fog_tex: texture_3d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var fog_smp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var k = 1.0;
@if(VERTEX_COLORS)
    k = in.color.r;
    let f = froxel_fog(fog_tex, fog_smp, bolt.fog, in.position, in.world_position.xyz);
    // (Bevy draws `AlphaMode::Add` as premultiplied blending with alpha 0: the colour is added)
    return vec4<f32>(bolt.color.rgb * k * max(f.a, 0.25), 0.0);
}
