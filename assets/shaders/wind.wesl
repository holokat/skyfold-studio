// The travelling gust field shared by grass, plants and trees, so wind waves roll across meadows
// and forests together. Two smooth value-noise taps: cheap enough to evaluate per vertex.
import super::noise::value_noise2;

// ~0 in a lull, ~1 in a gust (centred on 0.5). gp: world xz minus the travelled gust offset (m).
fn gust_field(gp: vec2<f32>) -> f32 {
    return value_noise2(gp / 140.0, 3u) * 0.62 + value_noise2(gp / 61.0 + vec2<f32>(0.37), 4u) * 0.38;
}
