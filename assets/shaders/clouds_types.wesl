// Uniforms of the cloud passes (`clouds::CloudGpu`).

struct Clouds {
    world_from_clip: mat4x4<f32>,
    prev_clip_from_world: mat4x4<f32>,
    // xyz: camera position, w: frame index
    camera: vec4<f32>,
    // xy: trace size (px), zw: view size (px)
    size: vec4<f32>,
    // xy: history size (px), zw: this frame's traced pixel in each 2x2 block
    grid: vec4<f32>,
    // x: coverage, y: extinction of the densest cloud (1/m), z: cloud type, w: cirrus amount
    cover: vec4<f32>,
    // xy: cumulus field offset (m), zw: cirrus offset (m)
    offset: vec4<f32>,
    // xy: change of the cumulus offset since the last frame, z: history valid, w: time (s)
    motion: vec4<f32>,
    // xyz: direction towards the key light, w: 1 for the moon
    key: vec4<f32>,
    // rgb: key illuminance above the atmosphere (lux)
    key_color: vec4<f32>,
    // xyz: direction towards the second light
    fill: vec4<f32>,
    // rgb: second light's illuminance (lux)
    fill_color: vec4<f32>,
    // x: base altitude, y: top altitude, z: cirrus altitude, w: near fade distance (m)
    layer: vec4<f32>,
    // x: ambient scale, y: storm darkening, z: night
    shade: vec4<f32>,
    // rgb: the scene's artificial ambient fill (radiance)
    fill_ambient: vec4<f32>,
    // x: coarse steps, y: light samples, z: weight of a new sample, w: history scale
    quality: vec4<f32>,
}
