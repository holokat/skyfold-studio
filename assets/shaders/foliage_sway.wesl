// The foliage wind displacement, shared by the main pass (foliage.wesl) and the depth / shadow
// prepass (foliage_prepass.wesl) so leaves sit in the same place in both.
// wp: world position, origin: the plant's root, sway: hierarchy weight (vertex colour alpha),
// t: time (s), wind: (dir.x, dir.z, speed m/s, gustiness), gust: travelled gust offset (m).
import super::wind::gust_field;

fn foliage_sway(wp: vec3<f32>, origin: vec3<f32>, sway: f32, t: f32, wind: vec4<f32>, gust_offset: vec2<f32>) -> vec3<f32> {
    let phase = fract(dot(origin.xz, vec2<f32>(0.0731, 0.0517)));
    let speed = wind.z;
    let dir = vec3<f32>(wind.x, 0.0, wind.y);
    let perp = vec3<f32>(-dir.z, 0.0, dir.x);

    // travelling gusts (same field as the grass), sampled at the plant's root: one value per plant
    let gfield = smoothstep(0.33, 0.77, gust_field(origin.xz - gust_offset));
    let gust = mix(1.0, 0.3 + 1.5 * gfield, wind.w);

    // 1. whole-plant bend: ~0.1 m at 10 m height in a light breeze, ~0.8 m in a fresh breeze
    let h = max(wp.y - origin.y, 0.0);
    let base = 0.025 * pow(speed, 1.5) * gust;
    let rock = 0.65 + 0.35 * sin(t * (0.9 + 0.5 * phase) + phase * 6.283) + 0.12 * sin(t * 2.3 + phase * 11.0);
    let hh = h / 10.0;
    var disp = dir * (base * hh * hh * rock);
    disp += perp * (base * 0.25 * hh * hh * sin(t * 0.7 + phase * 4.0));

    // 2. branches with their leaves: weighted by the hierarchy, faster than the trunk and out of
    //    phase around the crown (the phase wavelength is ~25 m, so a branch moves as one piece)
    let bphase = dot(wp - origin, vec3<f32>(0.25, 0.14, 0.19)) + phase * 9.0;
    let bamp = 0.035 * pow(speed, 1.15) * gust * sway;
    disp += (dir * (0.6 + 0.4 * sin(t * 2.1 + bphase)) + perp * 0.45 * sin(t * 1.7 + bphase * 1.3)) * bamp;

    // keep branches roughly their length: lower them as they swing out
    let lateral = dot(disp.xz, disp.xz);
    return vec3<f32>(wp.x + disp.x, wp.y + disp.y - lateral / (2.0 * max(h, 1.0)), wp.z + disp.z);
}
