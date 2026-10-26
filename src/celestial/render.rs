//! Draws the sky bodies: one inward dome round the camera, between the stars (behind it) and the
//! volumetric clouds (over it), whose shader (`celestial.wesl`) ray-traces each body inside its
//! own patch of sky. Light is physical: sunlit surfaces are as bright as the sun makes them,
//! then the camera's exposure, so a planet is a pale ghost by day and bright at night.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use super::{MAX_BODIES, SkyBody};
use crate::camera::MainCamera;
use crate::look::LookState;
use crate::noise::smoothstep;
use crate::sky::{Lighting, SkyState};

/// Inside the star dome (30 km), well past the world.
const DOME_RADIUS: f32 = 29_000.0;

/// One body as the shader reads it.
#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct BodyGpu {
    /// xyz: direction towards the centre, w: angular radius (rad)
    pub dir: Vec4,
    /// x: kind, y: seed, z: brightness, w: atmosphere
    pub info: Vec4,
    /// rgb: colour, w: ring opacity
    pub color: Vec4,
    /// rgb: second colour, w: ring inner radius (body radii)
    pub color2: Vec4,
    /// rgb: atmosphere colour, w: ring outer radius
    pub atmo: Vec4,
    /// xyz: spin axis, w: spin (rad)
    pub axis: Vec4,
    /// xyz: ring plane normal, w: unused
    pub ring: Vec4,
    /// rgb: ring colour
    pub ring_color: Vec4,
}

#[derive(Clone, Debug, Default, ShaderType)]
pub struct CelestialUniform {
    pub bodies: [BodyGpu; MAX_BODIES],
    /// xyz: direction towards the sun, w: sunlight (lux)
    pub sun: Vec4,
    /// x: body count, y: time (s), z: how much they hide the stars (night), w: visibility
    pub params: Vec4,
    /// x: haze (horizon extinction), y: star visibility (lensed stars)
    pub air: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct CelestialMaterial {
    #[uniform(0)]
    pub sky: CelestialUniform,
}

impl Material for CelestialMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/celestial.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    /// After the stars (-1e9), before the clouds (-1e8): the clouds pass in front.
    fn depth_bias(&self) -> f32 {
        -5.0e8
    }
}

#[derive(Component)]
struct CelestialDome;

pub(super) fn build(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CelestialMaterial>::default())
        .add_systems(Startup, spawn)
        .add_systems(
            Update,
            update.after(crate::sky::SkySystems),
        );
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<CelestialMaterial>>,
) {
    commands.spawn((
        Name::new("Sky bodies"),
        CelestialDome,
        Mesh3d(meshes.add(crate::sky::inward_sphere(DOME_RADIUS, 48, 24))),
        MeshMaterial3d(mats.add(CelestialMaterial {
            sky: CelestialUniform::default(),
        })),
        Transform::IDENTITY,
        Visibility::Hidden,
        NotShadowCaster,
        bevy::camera::visibility::NoFrustumCulling,
    ));
}

/// The body's spin axis: up, leaned by its tilt across the line of sight.
fn spin_axis(b: &SkyBody, dir: Vec3) -> Vec3 {
    let side = Vec3::Y.cross(dir).try_normalize().unwrap_or(Vec3::X);
    let up = dir.cross(side).normalize();
    Quat::from_axis_angle(dir, b.tilt.to_radians()) * up
}

/// The ring's plane normal: the spin axis tipped `ring_open` towards the viewer.
fn ring_normal(b: &SkyBody, dir: Vec3, axis: Vec3) -> Vec3 {
    // seen exactly edge-on the normal is perpendicular to the view; opening the ring turns it
    // towards the viewer (−dir)
    let a = b.ring_open.to_radians();
    (axis * a.cos() - dir * a.sin()).normalize()
}

pub fn pack(b: &SkyBody, hour: f32) -> BodyGpu {
    let dir = b.direction(hour);
    let axis = spin_axis(b, dir);
    let ring = ring_normal(b, dir, axis);
    let c = |v: [f32; 3], w: f32| Vec4::new(v[0], v[1], v[2], w);
    BodyGpu {
        dir: dir.extend((b.size * 0.5).to_radians()),
        info: Vec4::new(
            b.kind.index(),
            (b.seed % 997) as f32,
            b.brightness,
            b.atmosphere,
        ),
        color: c(b.color, b.ring),
        color2: c(b.color2, b.ring_inner),
        atmo: c(b.atmosphere_color, b.ring_outer),
        axis: axis.extend(b.spin.to_radians()),
        ring: ring.extend(0.0),
        ring_color: c(b.ring_color, 0.0),
    }
}

#[allow(clippy::type_complexity)]
fn update(
    lighting: Res<Lighting>,
    look: Res<LookState>,
    sky: Res<SkyState>,
    time: Res<Time>,
    interior: Option<Res<crate::dungeons::render::InteriorLook>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut dome: Query<
        (&mut Transform, &mut Visibility, &MeshMaterial3d<CelestialMaterial>),
        With<CelestialDome>,
    >,
    mut mats: ResMut<Assets<CelestialMaterial>>,
) {
    let Ok((mut t, mut vis, m)) = dome.single_mut() else {
        return;
    };
    let set = &lighting.celestial;
    let shown: Vec<&SkyBody> = set.bodies.iter().filter(|b| b.enabled).take(MAX_BODIES).collect();
    // a dungeon's underground map has no sky
    let underground = interior.as_ref().is_some_and(|i| i.active && !i.sky);
    let want = if shown.is_empty() || underground {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };
    if *vis != want {
        *vis = want;
    }
    if want == Visibility::Hidden {
        return;
    }
    if let Ok(cam) = cam.single() {
        t.translation = cam.translation();
    }
    let Some(mut mat) = mats.get_mut(&m.0) else {
        return;
    };
    // heavy overcast and fog hide the sky beyond the clouds, as they do the stars
    let overcast = 1.0 - smoothstep(0.55, 1.0, sky.weather.clouds) * 0.85;
    let fog = 1.0 - 0.9 * sky.weather.fog.clamp(0.0, 1.0) * look.haze.strength;
    let stars = if lighting.stars { sky.night } else { 0.0 };
    let u = &mut mat.sky;
    u.bodies = [BodyGpu::default(); MAX_BODIES];
    for (i, b) in shown.iter().enumerate() {
        u.bodies[i] = pack(b, lighting.hour);
    }
    let sun_lux = bevy::light::light_consts::lux::RAW_SUNLIGHT
        * lighting.sun_strength
        * look.params.sun;
    u.sun = sky.sun_dir.extend(sun_lux);
    u.params = Vec4::new(
        shown.len() as f32,
        time.elapsed_secs() % 3600.0,
        sky.night,
        overcast * fog,
    );
    u.air = Vec4::new(look.haze.strength * (0.3 + lighting.haze), stars * overcast, 0.0, 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::celestial::BodyKind;

    #[test]
    fn spin_axis_is_perpendicular_to_the_view() {
        for kind in BodyKind::ALL {
            let mut b = SkyBody::preset(kind);
            b.tilt = 33.0;
            let d = b.direction(0.0);
            let a = spin_axis(&b, d);
            assert!(a.dot(d).abs() < 1e-4 && (a.length() - 1.0).abs() < 1e-4);
            // a ring opened 90° faces the viewer
            b.ring_open = 90.0;
            assert!(ring_normal(&b, d, a).dot(-d) > 0.999);
        }
    }
}
