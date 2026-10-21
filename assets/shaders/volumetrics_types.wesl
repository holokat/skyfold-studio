// Uniforms and froxel maths shared by the volumetric light passes (`volumetrics.rs`).
//
// The froxel grid is the view frustum cut into cells: x, y across the screen, z in slices whose
// depth grows exponentially from `depth.x` (near) to `depth.y` (far), so the cells near the eye
// are thin and the distant ones long.

struct FogLight {
    // xyz: position (m), w: range (m)
    position: vec4<f32>,
    // rgb: luminous intensity (cd), w: unused
    color: vec4<f32>,
}

struct Volumetrics {
    world_from_view: mat4x4<f32>,
    view_from_world: mat4x4<f32>,
    // the previous frame's view, for the history lookup
    prev_view_from_world: mat4x4<f32>,
    // xyz: camera (m), w: frame index
    camera: vec4<f32>,
    // xyz: grid size (cells), w: history weight (0: no history this frame)
    grid: vec4<f32>,
    // x: near, y: far (m, view depth), z: ln(far / near), w: this frame's jitter along z (0..1)
    depth: vec4<f32>,
    // xy: tan of the half field of view (x, y), zw: the previous frame's
    proj: vec4<f32>,
    // x: haze extinction at its base (1/m), y: its scale height (m), z: base height (m), w: weather fog's extinction (1/m)
    haze: vec4<f32>,
    // x: weather fog's scale height (m), y: ground mist's extinction (1/m), z: mist height over the ground (m), w: camera near plane (m)
    mist: vec4<f32>,
    // xyz: noise drift (m), w: noise strength (0..1)
    wind: vec4<f32>,
    // x: noise frequency (1/m), y: phase anisotropy g, z: sun scattering gain, w: point light gain
    shape: vec4<f32>,
    // rgb: scattering albedo (the medium's colour), w: sky light gain
    tint: vec4<f32>,
    // rgb: artificial ambient fill (radiance), w: point light count
    ambient: vec4<f32>,
    // x: terrain half size (m), y: cell (m), z: texels per side, w: 1 with a terrain
    terrain: vec4<f32>,
    // x: ground haze extinction (1/m), y: its height over the terrain (m), z: sea level (m)
    layers: vec4<f32>,
    lights: array<FogLight, 24>,
}

const PI: f32 = 3.14159265;

// View depth (m) at slice coordinate w (0..1).
fn slice_depth(v: Volumetrics, w: f32) -> f32 {
    return v.depth.x * exp(w * v.depth.z);
}

// Slice coordinate (0..1, may fall outside) of view depth d.
fn depth_slice(v: Volumetrics, d: f32) -> f32 {
    return log(max(d, 1e-4) / v.depth.x) / v.depth.z;
}

fn hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * c, 1e-4), 1.5));
}
