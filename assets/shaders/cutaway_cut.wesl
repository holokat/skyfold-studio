// The locked game cameras' cutaway (model_render::cutaway), shared by cutaway.wesl and its
// prepass: under a roof, the building is cut away above the player's head; anything standing in
// front of the player opens a see-through circle round it. A screen-door dither softens both
// edges (TAA smooths it). Only the game camera's own view cuts: shadow maps keep every piece, so
// the rooms stay shaded as they were.

import bevy_pbr::render::{
    mesh_view_bindings::view,
    view_transformations::position_world_to_clip,
};

struct CutawayParams {
    // xyz: the player's chest (world), w: 1 on, 0 off
    player: vec4<f32>,
    // xyz: the game camera (world), w: the see-through circle's radius (part of the view's height)
    camera: vec4<f32>,
    // what roofs the player (world xz): min x, min z, max x, max z (empty: min > max)
    roof_box: vec4<f32>,
    // x: the cut's height (world y), y: its soft band (m), z: the player's feet (world y)
    roof: vec4<f32>,
}

// Whether this fragment (world position `p`, framebuffer position `frag`) is cut away.
fn cut_away(p: vec3<f32>, frag: vec2<f32>, c: CutawayParams) -> bool {
    if c.player.w < 0.5 || distance(view.world_position, c.camera.xyz) > 2.0 {
        return false;
    }
    var fade = 0.0;
    let b = c.roof_box;
    if p.x > b.x && p.x < b.z && p.z > b.y && p.z < b.w {
        fade = smoothstep(c.roof.x, c.roof.x + c.roof.y, p.y);
    }
    // (never the floor it stands on, nor what is behind it)
    if p.y > c.roof.z + 0.15 && distance(p, c.camera.xyz) < distance(c.player.xyz, c.camera.xyz) - 0.6 {
        let clip = position_world_to_clip(c.player.xyz);
        let ndc = clip.xy / clip.w;
        let at = view.viewport.xy
            + vec2<f32>((ndc.x * 0.5 + 0.5) * view.viewport.z, (0.5 - ndc.y * 0.5) * view.viewport.w);
        let d = length(frag - at) / max(c.camera.w * view.viewport.w, 1.0);
        fade = max(fade, 1.0 - smoothstep(0.7, 1.0, d));
    }
    // interleaved gradient noise
    let noise = fract(52.9829189 * fract(dot(frag, vec2<f32>(0.06711056, 0.00583715))));
    return fade > noise;
}
