// Sunlight caught by small things in the air: the specks of `air.wesl` (visuals/air.rs) and the
// chimney smoke of `chimney_smoke.wesl` (visuals/chimneys.rs). Whether the sun reaches a point (one
// tap of its shadow cascade) and how much light scatters towards the eye by the angle to it
// (Henyey-Greenstein): looking towards the sun, dust sparkles and smoke glows.

import bevy_pbr::render::{
    mesh_view_bindings::{view, lights},
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    view_transformations::position_world_to_view,
    shadows::{get_cascade_index, world_to_directional_light_local},
    shadow_sampling::sample_shadow_map_hardware,
};

const PI: f32 = 3.14159265;

// Whether directional light `i` (the sun, the moon) reaches `wp`: one hardware-filtered tap of its
// shadow cascade (a speck or a puff is soft: the soft filters' dozen taps would buy nothing),
// nudged towards the light so it doesn't shadow itself.
fn sun_visibility(i: u32, wp: vec3<f32>, L: vec3<f32>, view_z: f32) -> f32 {
    let light = &lights.directional_lights[i];
    if ((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u {
        return 1.0;
    }
    let c = get_cascade_index(i, view_z);
    if c >= (*light).num_cascades {
        return 1.0;
    }
    let texel = (*light).cascades[c].texel_size;
    let p = wp + L * ((*light).shadow_depth_bias + (*light).shadow_normal_bias * texel);
    let ll = world_to_directional_light_local(i, c, vec4<f32>(p, 1.0));
    if ll.w == 0.0 {
        return 1.0;
    }
    return sample_shadow_map_hardware(ll.xy, ll.z, i32((*light).depth_texture_base_index + c));
}

// Henyey-Greenstein phase: how much light scatters by the angle `c` (cos) off its path.
fn hg(g: f32, c: f32) -> f32 {
    let d = 1.0 + g * g - 2.0 * g * c;
    return (1.0 - g * g) / (4.0 * PI * d * sqrt(d));
}

// The sun's (and the moon's) light scattered towards the eye at `wp` with anisotropy `g` (`V`:
// from the eye towards `wp`), shadowed; in display units (times exposure).
fn sun_forward(wp: vec3<f32>, V: vec3<f32>, g: f32) -> vec3<f32> {
    let view_z = position_world_to_view(wp).z;
    var out = vec3<f32>(0.0);
    for (var i: u32 = 0u; i < lights.n_directional_lights; i = i + 1u) {
        let light = lights.directional_lights[i];
        let L = light.direction_to_light;
        out += light.color.rgb * view.exposure * sun_visibility(i, wp, L, view_z) * hg(g, dot(V, L));
    }
    return out;
}
