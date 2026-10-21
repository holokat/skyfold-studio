// The volumetric light for transparent surfaces (`volumetrics::FroxelFog`): they are drawn after
// the view was fogged, so each fogs its own colour at its own depth. Bind `FroxelFog::image`
// (texture_3d<f32> + a filtering sampler) and `FroxelFog::params` in the material, then:
//
//   let f = froxel_fog(fog_tex, fog_smp, params, in.position, in.world_position.xyz);
//   alpha-blended:  rgb = rgb * f.a + f.rgb   (the light in front of it, and less of it behind)
//   additive glow:  rgb = rgb * f.a           (adding the fog's light again would double it)

import bevy_pbr::render::mesh_view_bindings::view;

// The light added (rgb, times exposure) and the transmittance (a) between the eye and a fragment.
fn froxel_fog(fog_tex: texture_3d<f32>, fog_smp: sampler, params: vec4<f32>, frag_coord: vec4<f32>, world_pos: vec3<f32>) -> vec4<f32> {
    if params.w < 0.5 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let uv = (frag_coord.xy - view.viewport.xy) / view.viewport.zw;
    let d = -(view.view_from_world * vec4<f32>(world_pos, 1.0)).z;
    let w = log(max(d, 1e-4) / params.x) / params.y;
    if w <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let gz = params.z;
    let wc = min(w, 1.0);
    let s = textureSampleLevel(fog_tex, fog_smp, vec3<f32>(uv, max(wc - 0.5 / gz, 0.5 / gz)), 0.0);
    return mix(vec4<f32>(0.0, 0.0, 0.0, 1.0), s, clamp(wc * gz, 0.0, 1.0));
}
