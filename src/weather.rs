//! Rain and snow particles (vertex-shader animated, see `precip.wesl`), driven by the blended
//! weather of the zone the viewer is in.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::camera::MainCamera;
use crate::sky::SkyState;
use crate::terrain::Terrain;

const PARTICLES: u32 = 14_000;

#[derive(Clone, Debug, ShaderType, Default)]
pub struct PrecipParams {
    pub amount: Vec4,
    pub wind: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct PrecipMaterial {
    #[uniform(0)]
    pub params: PrecipParams,
    /// The volumetric light (`volumetrics::FroxelFog`; None while it is off): drawn after the view
    /// was fogged, the streaks fog themselves.
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    pub fog_volume: Option<Handle<Image>>,
    /// `FroxelFog::params` (w 0: off)
    #[uniform(3)]
    pub fog: Vec4,
}

impl Material for PrecipMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/precip.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/precip.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    /// The shader reads the fog (bindings 1–3) only when told, so a build without them still
    /// runs the current shader.
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        if let Some(f) = descriptor.fragment.as_mut() {
            f.shader_defs
                .push(bevy::shader::ShaderDefVal::Bool("FROXEL_FOG".into(), true));
        }
        Ok(())
    }
}

#[derive(Component)]
struct Precipitation;

pub struct WeatherPlugin;

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<PrecipMaterial>::default())
            .add_systems(Startup, spawn)
            .add_systems(Update, update.after(crate::camera::CameraSystems));
    }
}

fn spawn(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<PrecipMaterial>>,
) {
    let mut pos = Vec::with_capacity((PARTICLES * 4) as usize);
    let mut uv = Vec::with_capacity(pos.capacity());
    let mut idx = Vec::with_capacity((PARTICLES * 6) as usize);
    for i in 0..PARTICLES {
        for c in 0..4u32 {
            pos.push([0.0f32, 0.0, 0.0]);
            uv.push([i as f32, c as f32]);
        }
        let b = i * 4;
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U32(idx));
    commands.spawn((
        Name::new("Precipitation"),
        Precipitation,
        Mesh3d(meshes.add(m)),
        MeshMaterial3d(mats.add(PrecipMaterial {
            params: PrecipParams::default(),
            fog_volume: None,
            fog: Vec4::ZERO,
        })),
        Transform::IDENTITY,
        Visibility::Hidden,
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

fn update(
    sky: Res<SkyState>,
    terrain: Res<Terrain>,
    wind: Res<crate::objects::Wind>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(&mut Visibility, &MeshMaterial3d<PrecipMaterial>), With<Precipitation>>,
    mut mats: ResMut<Assets<PrecipMaterial>>,
    fog: Option<Res<crate::volumetrics::FroxelFog>>,
) {
    let (Ok(cam), Ok((mut vis, mat))) = (cam.single(), q.single_mut()) else {
        return;
    };
    let p = cam.translation();
    // from high above, falling particles near the lens just look like noise
    let height = p.y - terrain.height_at(p.x, p.z).max(terrain.sea_level);
    let near_ground = 1.0 - crate::noise::smoothstep(120.0, 400.0, height);
    let rain = sky.weather.rain * near_ground;
    let snow = sky.weather.snow * near_ground;
    let show = rain > 0.01 || snow > 0.01;
    let want = if show {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if *vis != want {
        *vis = want;
    }
    if !show {
        return;
    }
    let Some(mut m) = mats.get_mut(&mat.0) else {
        return;
    };
    let brightness = 1.0 - sky.night * 0.85;
    m.params.amount = Vec4::new(rain, snow, 28.0, 2.0 + sky.weather.rain * 3.0);
    m.params.wind = Vec4::new(wind.dir.x, wind.dir.y, brightness, 0.0);
    (m.fog_volume, m.fog) = crate::rain::froxel(fog.as_deref());
}
