// Blends the resolved clouds (premultiplied radiance, opacity) over the scene. Drawn right after
// the star dome. The clouds are traced at low resolution (each sample takes the farthest scene
// depth of its block), so on its own the upsampled result smears far clouds over foliage and
// silhouettes; each fragment therefore writes the nearest depth a cloud can have along its ray
// and is depth-tested against the scene.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var cloud_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var cloud_smp: sampler;
// x: cloud base altitude, y: nearest distance any cloud can be (near fade), z: earth radius
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var<uniform> params: vec4<f32>;

struct CompositeOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fragment(in: VertexOutput) -> CompositeOut {
    var out: CompositeOut;
    let uv = (in.position.xy - view.viewport.xy) / view.viewport.zw;
    // bilinear: sharpening the upsampling would also sharpen what little noise is left
    let c = max(textureSampleLevel(cloud_tex, cloud_smp, uv, 0.0), vec4<f32>(0.0));
    if c.a <= 1.0e-4 && max(c.r, max(c.g, c.b)) <= 1.0e-6 {
        discard;
    }
    out.color = vec4<f32>(c.rgb, min(c.a, 1.0));

    // nearest a cloud can be along this view ray: the near fade, and from below the layer the
    // distance to its (curved) base
    let cam = view.world_position;
    let d = normalize(in.world_position.xyz - cam);
    var t_min = params.y;
    let h = cam.y;
    if h < params.x {
        let r0 = params.z + max(h, 0.0);
        let rb = params.z + params.x;
        // rb² − r0² without cancellation
        let k = (rb - r0) * (rb + r0);
        let b = r0 * d.y;
        let den = b + sqrt(max(b * b + k, 0.0));
        t_min = max(t_min, select(2.0e6, k / den, den > 1.0e-3));
    }
    t_min = min(t_min, 2.0e6);
    let clip = view.clip_from_world * vec4<f32>(cam + d * t_min, 1.0);
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    return out;
}
