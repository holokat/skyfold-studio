// The cloth wind displacement for banners, flags and ragged hems on library models (cloth.wesl and
// its prepass): the world wind and its travelling gusts (the same field as grass and trees) blow
// the free cloth downwind while a ripple runs along it.
// wp: world position, origin: the model's origin, w: sway weight (vertex colour alpha: 0 where
// the cloth is pinned, 1 at its free edge), t: time (s), wind: (dir.x, dir.z, speed m/s,
// gustiness), gust_offset: travelled gust offset (m).
import super::wind::gust_field;

fn cloth_sway(wp: vec3<f32>, origin: vec3<f32>, w: f32, t: f32, wind: vec4<f32>, gust_offset: vec2<f32>) -> vec3<f32> {
    let speed = wind.z;
    let dir = vec3<f32>(wind.x, 0.0, wind.y);
    let perp = vec3<f32>(-dir.z, 0.0, dir.x);
    // gusts sampled where the cloth hangs: one value per banner
    let gfield = smoothstep(0.33, 0.77, gust_field(wp.xz - gust_offset));
    let gust = mix(1.0, 0.35 + 1.5 * gfield, wind.w);
    let phase = fract(dot(origin.xz, vec2<f32>(0.0731, 0.0517))) * 6.283;

    // a ripple running down and along the cloth, faster in stronger wind; the phase changes over
    // half-metres, so a painted mark and the cloth under it move as one
    let freq = 2.2 + 0.18 * speed;
    let ph = dot(wp, vec3<f32>(0.9, 1.6, 0.7)) - t * freq + phase;
    let amp = 0.03 * min(speed, 1.0) * (1.0 + 0.6 * speed) * gust * w;
    var disp = dir * amp * (0.75 + 0.45 * sin(ph)) + perp * amp * 0.55 * sin(ph * 1.27 + 1.3);
    // a slow billow of the whole banner; none at all in still air (the editor stills the wind
    // while editing: speed 0)
    disp += (dir * 0.6 + perp * 0.4) * w * 0.05 * sin(t * 0.8 + phase) * min(speed * 0.25, 1.2);
    // blown out, the free edge rises a little instead of stretching
    let lateral = dot(disp.xz, disp.xz);
    return vec3<f32>(wp.x + disp.x, wp.y + lateral * 0.35, wp.z + disp.z);
}
