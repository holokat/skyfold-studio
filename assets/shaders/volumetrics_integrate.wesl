// Volumetric light, step 2 of 3 (`volumetrics.rs`): walk each froxel column away from the eye and
// accumulate the light scattered towards it and the transmittance, cell by cell (the energy-
// conserving step of Hillaire, "Physically Based and Unified Volumetric Rendering in Frostbite",
// 2015: the light a slab scatters is dimmed by the slab itself, so dense fog doesn't glow).
//
// Output per cell: rgb = light added between the eye and the cell's far edge (times exposure),
// a = transmittance over that distance.

import super::volumetrics_types::{Volumetrics, slice_depth};

@group(0) @binding(0) var<uniform> vol: Volumetrics;
@group(0) @binding(1) var scatter_tex: texture_3d<f32>;
@group(0) @binding(2) var out_tex: texture_storage_3d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    let grid = vec3<u32>(vol.grid.xyz);
    if any(id.xy >= grid.xy) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vol.grid.xy;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    // metres along this column's ray per metre of view depth
    let ray_scale = length(vec3<f32>(ndc.x * vol.proj.x, ndc.y * vol.proj.y, 1.0));
    var acc = vec3<f32>(0.0);
    var tr = 1.0;
    var d0 = vol.depth.x;
    for (var z = 0u; z < grid.z; z++) {
        let d1 = slice_depth(vol, f32(z + 1u) / vol.grid.z);
        let ds = (d1 - d0) * ray_scale;
        let s = textureLoad(scatter_tex, vec3<i32>(vec2<i32>(id.xy), i32(z)), 0);
        let sigma = max(s.a, 1e-7);
        let t = exp(-sigma * ds);
        acc += tr * s.rgb * (1.0 - t) / sigma;
        tr *= t;
        textureStore(out_tex, vec3<u32>(id.xy, z), vec4<f32>(acc, tr));
        d0 = d1;
    }
}
