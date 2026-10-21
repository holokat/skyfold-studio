//! The photo studio thumbnails and previews are shot in: soft key / fill / rim lights and a
//! studio environment (softboxes over a neutral grey sweep, filtered once on the GPU) on their own
//! render layers, a transparent background, and framing that fits any object's bounds.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy::light::{Cascades, EnvironmentMapLight, GeneratedEnvironmentMapLight};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureUsages, TextureViewDescriptor,
    TextureViewDimension,
};

/// Thumbnails render on this layer, the turntable preview on the next; the world never sees them.
pub const THUMB_LAYER: usize = 7;
pub const PREVIEW_LAYER: usize = 9;

/// Vertical field of view: long enough a lens that objects don't look distorted.
pub const FOV: f32 = 24.0 * std::f32::consts::PI / 180.0;

/// On every studio camera.
#[derive(Component)]
pub struct StudioCamera;

/// Bevy builds the world's sun cascades for every active camera, and then culls every shadow
/// caster in the world against each of them. The sun doesn't light the studio's layers: drop the
/// studio cameras' cascades so a thumbnail or the turntable doesn't add that work.
pub fn skip_studio_cascades(
    cams: Query<Entity, With<StudioCamera>>,
    mut lights: Query<&mut Cascades>,
) {
    for mut c in &mut lights {
        if cams.iter().any(|e| c.cascades.contains_key(&e)) {
            for e in &cams {
                c.cascades.remove(&e);
            }
        }
    }
}

/// The studio's lights and filtered environment, alive while anything is being shot.
#[derive(Resource, Default)]
pub struct Studio {
    rig: Vec<Entity>,
    /// the environment's generator while it filters, then the filtered maps (kept for good)
    env: Option<Entity>,
    pub env_light: Option<EnvironmentMapLight>,
    filtered_frames: u32,
}

/// Where the camera looks from (unit vector from the subject towards the camera): a three-quarter
/// view from the front-left and a little above; fish from their left flank; flat fish and rays from
/// above.
pub fn view_dir(fish: Option<crate::fish::FishSpecies>) -> Vec3 {
    let (az, el): (f32, f32) = match fish {
        Some(crate::fish::FishSpecies::Flounder | crate::fish::FishSpecies::Gloamwing) => {
            (60.0, 55.0)
        }
        Some(_) => (72.0, 14.0),
        None => (35.0, 24.0),
    };
    let (az, el) = (az.to_radians(), el.to_radians());
    Vec3::new(az.sin() * el.cos(), el.sin(), az.cos() * el.cos())
}

/// Camera placement that fits `points` (any shape's outline, relative to the subject's origin)
/// in a `aspect`-wide view from `dir`, with `margin` (fraction of the frame) left around it.
/// Returns (camera translation, the point it looks at, near, far).
pub fn frame(points: &[Vec3], dir: Vec3, aspect: f32, margin: f32) -> (Vec3, Vec3, f32, f32) {
    let fwd = -dir.normalize();
    let right = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
    let up = right.cross(fwd);
    let (tx, ty) = (
        (FOV * 0.5).tan() * aspect * (1.0 - margin),
        (FOV * 0.5).tan() * (1.0 - margin),
    );
    let c = points.iter().copied().fold(Vec3::ZERO, |a, p| a + p) / points.len().max(1) as f32;
    let radius = points.iter().map(|p| p.distance(c)).fold(0.01f32, f32::max);
    let mut target = c;
    let mut dist = radius / (FOV * 0.5).sin();
    // a few rounds: centre the projected extent, then back off until every point fits
    for _ in 0..4 {
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in points {
            let d = *p - target;
            let z = dist + d.dot(fwd);
            let (x, y) = (d.dot(right) / z.max(1e-3), d.dot(up) / z.max(1e-3));
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        target += right * (x0 + x1) * 0.5 * dist + up * (y0 + y1) * 0.5 * dist;
        dist = points
            .iter()
            .map(|p| {
                let d = *p - target;
                let z = d.dot(fwd);
                (d.dot(right).abs() / tx).max(d.dot(up).abs() / ty) - z
            })
            .fold(0.01f32, f32::max);
    }
    let near = (dist - radius * 1.5).max(dist * 0.02);
    (target - fwd * dist, target, near, dist + radius * 3.0)
}

/// A mesh's bounds: from its positions, or the extents kept once its data went to the GPU.
pub fn mesh_bounds(m: &Mesh) -> Option<(Vec3, Vec3)> {
    if let Ok(bevy::mesh::VertexAttributeValues::Float32x3(p)) =
        m.try_attribute(Mesh::ATTRIBUTE_POSITION)
        && !p.is_empty()
    {
        return Some(p.iter().fold((Vec3::MAX, Vec3::MIN), |(a, b), v| {
            (a.min(Vec3::from_array(*v)), b.max(Vec3::from_array(*v)))
        }));
    }
    m.final_aabb.map(|a| (Vec3::from(a.min), Vec3::from(a.max)))
}

/// The 8 corners of a box.
pub fn corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// The outline a box sweeps when it turns about the vertical axis through `axis` (xz): rings top
/// and bottom, so the turntable never clips the object.
pub fn swept(min: Vec3, max: Vec3, axis: Vec2) -> Vec<Vec3> {
    let r = corners(min, max)
        .iter()
        .map(|c| Vec2::new(c.x, c.z).distance(axis))
        .fold(0.0f32, f32::max);
    (0..32)
        .map(|i| {
            let a = (i % 16) as f32 / 16.0 * std::f32::consts::TAU;
            Vec3::new(
                axis.x + a.cos() * r,
                if i < 16 { min.y } else { max.y },
                axis.y + a.sin() * r,
            )
        })
        .collect()
}

/// A camera shooting into `target` on `layer`.
#[allow(clippy::too_many_arguments)]
pub fn camera(
    studio: &Studio,
    target: Handle<Image>,
    order: isize,
    at: Vec3,
    look: Vec3,
    near: f32,
    far: f32,
    layer: usize,
) -> impl Bundle {
    (
        StudioCamera,
        Camera3d::default(),
        Camera {
            order,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        RenderTarget::Image(target.into()),
        Projection::Perspective(PerspectiveProjection {
            fov: FOV,
            near,
            far,
            ..default()
        }),
        Transform::from_translation(at).looking_at(look, Vec3::Y),
        bevy::camera::Exposure { ev100: 12.0 },
        AmbientLight {
            color: Color::srgb(0.9, 0.93, 1.0),
            brightness: 120.0,
            affects_lightmapped_meshes: false,
        },
        studio.env_light.clone().unwrap_or(EnvironmentMapLight {
            intensity: 0.0,
            ..default()
        }),
        Msaa::Sample4,
        RenderLayers::layer(layer),
    )
}

/// A render target with a transparent background.
pub fn target(images: &mut Assets<Image>, size: UVec2) -> Handle<Image> {
    let mut img = Image::new_target_texture(
        size.x,
        size.y,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    );
    // thumbnails are read back
    img.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    images.add(img)
}

/// Puts the lights up while anything is being shot (`busy`) and takes them down after.
pub fn rig(
    commands: &mut Commands,
    studio: &mut Studio,
    images: &mut Assets<Image>,
    envs: &Query<&EnvironmentMapLight>,
    busy: bool,
) {
    if !busy {
        for e in studio.rig.drain(..) {
            commands.entity(e).despawn();
        }
        return;
    }
    if studio.rig.is_empty() {
        let layers = RenderLayers::from_layers(&[THUMB_LAYER, PREVIEW_LAYER]);
        // relative to the three-quarter view: key high front-left, fill low right, rim behind
        let dir = view_dir(None);
        let right = (-dir).cross(Vec3::Y).normalize();
        for (from, lux, color) in [
            (
                dir * 0.8 - right * 0.9 + Vec3::Y * 1.3,
                52_000.0,
                Color::srgb(1.0, 0.97, 0.92),
            ),
            (
                dir * 1.0 + right * 1.2 + Vec3::Y * 0.1,
                16_000.0,
                Color::srgb(0.88, 0.93, 1.0),
            ),
            (
                -dir * 1.0 + right * 0.3 + Vec3::Y * 1.0,
                34_000.0,
                Color::WHITE,
            ),
        ] {
            studio.rig.push(
                commands
                    .spawn((
                        DirectionalLight {
                            illuminance: lux,
                            color,
                            shadow_maps_enabled: false,
                            ..default()
                        },
                        Transform::from_translation(from).looking_at(Vec3::ZERO, Vec3::Y),
                        layers.clone(),
                    ))
                    .id(),
            );
        }
    }
    // the environment is filtered on the GPU for a few frames, then kept (its maps persist)
    match studio.env {
        None if studio.env_light.is_none() => {
            let cube = images.add(environment());
            studio.env = Some(
                commands
                    .spawn(GeneratedEnvironmentMapLight {
                        environment_map: cube,
                        intensity: 2600.0,
                        ..default()
                    })
                    .id(),
            );
        }
        Some(e) => {
            // filtering starts once its pipelines are compiled: give it time, then keep the maps
            if let Ok(light) = envs.get(e) {
                studio.filtered_frames += 1;
                if studio.filtered_frames > 60 {
                    studio.env_light = Some(light.clone());
                    commands.entity(e).despawn();
                    studio.env = None;
                }
            }
        }
        None => {}
    }
}

/// A studio environment cubemap (linear radiance): a neutral grey sweep, bright overhead, two big
/// softboxes in front-left and right and a strip light behind, so metal and glaze catch clean,
/// soft highlights.
fn environment() -> Image {
    const N: usize = 64;
    let view = view_dir(None);
    let right = (-view).cross(Vec3::Y).normalize();
    let boxes = [
        (
            (view * 0.7 - right * 0.8 + Vec3::Y * 0.9).normalize(),
            0.45f32,
            7.0f32,
        ),
        (
            (view * 0.6 + right * 1.0 + Vec3::Y * 0.3).normalize(),
            0.35,
            3.0,
        ),
        ((-view + Vec3::Y * 0.8).normalize(), 0.25, 5.0),
    ];
    let mut data = Vec::with_capacity(N * N * 6 * 8);
    for face in 0..6 {
        for y in 0..N {
            for x in 0..N {
                let u = (x as f32 + 0.5) / N as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
                let d = match face {
                    0 => Vec3::new(1.0, -v, -u),
                    1 => Vec3::new(-1.0, -v, u),
                    2 => Vec3::new(u, 1.0, v),
                    3 => Vec3::new(u, -1.0, -v),
                    4 => Vec3::new(u, -v, 1.0),
                    _ => Vec3::new(-u, -v, -1.0),
                }
                .normalize();
                // sweep: dark warm floor, grey horizon, bright cool ceiling
                let h = d.y;
                let mut c = if h > 0.0 {
                    Vec3::new(0.42, 0.44, 0.47).lerp(Vec3::new(0.95, 0.97, 1.0), h.powf(0.7))
                } else {
                    Vec3::new(0.42, 0.44, 0.47).lerp(Vec3::new(0.10, 0.095, 0.09), (-h).powf(0.5))
                };
                for (axis, size, power) in boxes {
                    let a = d.dot(axis);
                    let edge = 1.0 - size * 0.5;
                    if a > edge {
                        c += Vec3::splat(power * ((a - edge) / (1.0 - edge) * 6.0).clamp(0.0, 1.0));
                    }
                }
                for ch in [c.x, c.y, c.z, 1.0] {
                    data.extend_from_slice(&half::f16::from_f32(ch).to_le_bytes());
                }
            }
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: N as u32,
            height: N as u32,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Framing fits every point inside the view and centres the object.
    #[test]
    fn framing_fits() {
        let pts = corners(Vec3::new(-0.2, 0.0, -1.5), Vec3::new(0.2, 3.0, 1.5)).to_vec();
        let dir = view_dir(None);
        let (at, look, near, far) = frame(&pts, dir, 1.0, 0.08);
        let view = Transform::from_translation(at)
            .looking_at(look, Vec3::Y)
            .to_matrix()
            .inverse();
        let proj = Mat4::perspective_rh(FOV, 1.0, near, far);
        let (mut x0, mut x1, mut y0, mut y1) = (1.0f32, -1.0f32, 1.0f32, -1.0f32);
        for p in pts {
            let c = proj * view * p.extend(1.0);
            let (x, y) = (c.x / c.w, c.y / c.w);
            assert!(x.abs() <= 0.93 && y.abs() <= 0.93, "{p} at {x}, {y}");
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        // tight on its longer axis, centred on both
        assert!((x1 - x0).max(y1 - y0) > 1.75, "{x0}..{x1} × {y0}..{y1}");
        assert!((x0 + x1).abs() < 0.05 && (y0 + y1).abs() < 0.05);
    }
}
