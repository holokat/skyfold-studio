// Built objects (posts, crates, huts, ruins…): photoscanned materials mapped triplanar in object
// space, so procedural meshes need no UVs and textures stay put on rotated or scaled objects. The
// vertex colour picks the material slot (alpha × 255; 255 keeps the plain vertex colour) and
// tints it (rgb). Normals use a whiteout triplanar blend. Standard PBR lighting.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    mesh_functions,
};

struct SurfaceParams {
    // metres per texture repeat (u, v), two slots per vec4
    tiles: array<vec4<f32>, 8>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<uniform> surf: SurfaceParams;
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var albedo_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var normal_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var orm_array: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var surf_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var slot = 255u;
    var tint = vec3<f32>(1.0);
@if(VERTEX_COLORS) {
    slot = u32(round(in.color.a * 255.0));
    tint = in.color.rgb;
}
@if(VERTEX_OUTPUT_INSTANCE_INDEX) {
    if slot < 16u {
        // object space (uniform scale): M⁻¹ = Mᵀ / s²
        let m = mesh_functions::get_world_from_local(in.instance_index);
        let rot = mat3x3<f32>(m[0].xyz, m[1].xyz, m[2].xyz);
        let s2 = max(dot(m[0].xyz, m[0].xyz), 1e-6);
        let lp = transpose(rot) * (in.world_position.xyz - m[3].xyz) / s2;
        let ln = normalize(transpose(rot) * normalize(in.world_normal));
        let tp = surf.tiles[slot / 2u];
        let tile = select(tp.xy, tp.zw, (slot & 1u) == 1u);
        var w = pow(abs(ln), vec3<f32>(4.0));
        w /= max(w.x + w.y + w.z, 1e-5);
        // texture v follows the object's up axis on the side faces (planks and grain run along it)
        let uvx = vec2<f32>(lp.z, -lp.y) / tile;
        let uvy = lp.xz / tile;
        let uvz = vec2<f32>(lp.x, -lp.y) / tile;
        let i = i32(slot);
        let albedo = textureSample(albedo_array, surf_sampler, uvx, i).rgb * w.x
            + textureSample(albedo_array, surf_sampler, uvy, i).rgb * w.y
            + textureSample(albedo_array, surf_sampler, uvz, i).rgb * w.z;
        let orm = textureSample(orm_array, surf_sampler, uvx, i).rgb * w.x
            + textureSample(orm_array, surf_sampler, uvy, i).rgb * w.y
            + textureSample(orm_array, surf_sampler, uvz, i).rgb * w.z;
        var nx = textureSample(normal_array, surf_sampler, uvx, i).xyz * 2.0 - 1.0;
        var ny = textureSample(normal_array, surf_sampler, uvy, i).xyz * 2.0 - 1.0;
        var nz = textureSample(normal_array, surf_sampler, uvz, i).xyz * 2.0 - 1.0;
        // the side projections run v down the object's up axis: flip their tangent y back
        nx.y = -nx.y;
        nz.y = -nz.y;
        // whiteout blend into object space
        nx = vec3<f32>(nx.xy + ln.zy, abs(nx.z) * ln.x);
        ny = vec3<f32>(ny.xy + ln.xz, abs(ny.z) * ln.y);
        nz = vec3<f32>(nz.xy + ln.xy, abs(nz.z) * ln.z);
        let n_obj = normalize(nx.zyx * w.x + ny.xzy * w.y + nz.xyz * w.z);
        pbr_input.material.base_color = vec4<f32>(albedo * tint, 1.0);
        pbr_input.material.perceptual_roughness = clamp(orm.g, 0.05, 1.0);
        pbr_input.material.metallic = clamp(orm.b, 0.0, 1.0);
        pbr_input.diffuse_occlusion = min(pbr_input.diffuse_occlusion, vec3<f32>(orm.r)); // keeps the screen-space AO
        pbr_input.N = normalize(rot * n_obj);
    }
}
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
