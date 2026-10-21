//! Lighting & ambience: 24h day/night cycle (sun + moon over a physical atmosphere), exposure
//! adaptation, adaptive shadow cascades, stars, fog and per-zone weather blending (a zone's
//! lighting & style is blended into the look in `look.rs`). The volumetric clouds live in
//! `clouds.rs` and follow `SkyState` (cover, wind drift).

use bevy::asset::RenderAssetUsages;
use bevy::camera::Exposure;
use bevy::light::light_consts::lux;
use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap, NotShadowCaster,
    SunDisk,
};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use crate::camera::MainCamera;
use crate::look::{Look, LookState};
use crate::noise::smoothstep;
use crate::terrain::Terrain;
use crate::world::{Weather, WeatherParams, Zones};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShadowQuality {
    Low,
    Medium,
    #[default]
    High,
}

impl ShadowQuality {
    fn map_size(self) -> usize {
        match self {
            ShadowQuality::Low => 1024,
            ShadowQuality::Medium => 2048,
            ShadowQuality::High => 4096,
        }
    }
    fn cascades(self) -> usize {
        match self {
            ShadowQuality::Low => 2,
            ShadowQuality::Medium => 3,
            ShadowQuality::High => 4,
        }
    }
}

fn yes() -> bool {
    true
}

/// Global lighting settings (saved with the world).
#[derive(Resource, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub hour: f32,
    pub playing: bool,
    /// Real minutes for one full in-game day while previewing.
    pub day_minutes: f32,
    /// Degrees the sun path leans from vertical (noon elevation = 90 - tilt).
    pub sun_tilt: f32,
    /// Rotates where the sun rises (degrees).
    pub sun_azimuth: f32,
    pub sun_strength: f32,
    pub moon_strength: f32,
    /// Lunar phase: 0 new, 0.25 first quarter, 0.5 full, 0.75 last quarter.
    pub moon_phase: f32,
    pub night_brightness: f32,
    pub exposure_comp: f32,
    /// Master switch for outdoor haze: distance fog, aerial perspective and lit-air effects.
    /// The individual amounts remain saved so turning it back on restores the look.
    #[serde(default = "yes")]
    pub haze_enabled: bool,
    pub haze: f32,
    /// Base cloud cover 0..1 (zone weather can only add to it).
    pub cloud_coverage: f32,
    pub cloud_speed: f32,
    pub cloud_shadows: f32,
    /// Cloud shape: 0 = flat stratus layers, 0.5 = stratocumulus, 1 = towering cumulus.
    pub cloud_type: f32,
    /// Optical thickness: 0.3 = thin and bright, 1 = normal, 2 = dense with dark bases.
    pub cloud_density: f32,
    /// High wispy cirrus, 0..1.
    pub cirrus: f32,
    /// Variation of the cloud pattern (any value gives a different sky).
    pub cloud_seed: u32,
    /// Cost / quality of the volumetric clouds.
    pub cloud_quality: crate::clouds::CloudQuality,
    pub stars: bool,
    pub shadow_quality: ShadowQuality,
    /// Screen-space contact shadows (`camera::contact_shadows`): about a tenth of the frame time
    /// at 5K, for detail only seen up close, so off unless asked for. (A new key: worlds saved
    /// `contact_shadows: true` by default while the setting did nothing; they don't start paying
    /// for it now.)
    #[serde(rename = "contact_shadows_on")]
    pub contact_shadows: bool,
    /// Screen-space ambient occlusion (`camera::image_quality`).
    pub ambient_occlusion: crate::camera::AmbientOcclusion,
    /// SMAA or temporal anti-aliasing (`camera::image_quality`).
    pub anti_aliasing: crate::camera::AntiAliasing,
    /// Softens the dense silhouette of far-away forests without affecting foreground trees.
    pub forest_contours: crate::forest_contours::ForestContours,
    /// Eye adaptation (`camera_fx::adaptation`): the exposure follows what is on screen (walking,
    /// camera paths; with Always the editor view too).
    pub eye_adaptation: crate::camera_fx::EyeAdaptation,
    /// Depth of field for camera paths and stills (`camera_fx::dof`).
    pub depth_of_field: crate::camera_fx::DepthOfFieldSettings,
    /// Volumetric light (`volumetrics`): sunbeams through trees and windows, haze, ground mist
    /// and halos round lamps, lit in a froxel grid. Off falls back to plain distance fog.
    pub volumetrics: crate::volumetrics::VolumetricQuality,
    /// Bounce light while walking (`bounce_light`): sunlit walls and streets light the shade
    /// round them, lanes, arcades and rooms darken where the sky is hidden.
    #[serde(default = "yes")]
    pub bounce_light: bool,
    /// Preview weather everywhere instead of following zones.
    pub weather_override: Option<Weather>,
    /// Rain and snow at all (off: zones and biomes keep their clouds, fog and mood, but nothing
    /// falls and the ground stays dry).
    #[serde(default = "yes")]
    pub precipitation: bool,
    /// Puddles, lightning and thunder (`rain`).
    pub rain: crate::rain::RainSettings,
    /// Art style (colour grading, stylisation) layered over the physical lighting.
    pub look: Look,
    /// Dust, pollen, leaves and ash in the air, and chimney smoke (`visuals::air`).
    pub air: crate::visuals::air::AirSettings,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            hour: 15.5,
            playing: false,
            day_minutes: 2.0,
            sun_tilt: 38.0,
            sun_azimuth: 20.0,
            sun_strength: 1.0,
            moon_strength: 1.0,
            moon_phase: 0.5,
            night_brightness: 1.0,
            exposure_comp: 0.0,
            haze_enabled: true,
            haze: 0.15,
            cloud_coverage: 0.3,
            cloud_speed: 1.0,
            cloud_shadows: 0.55,
            // a thin, low layer of soft cumulus by default (towering skies are a preset away)
            cloud_type: 0.4,
            cloud_density: 0.85,
            cirrus: 0.35,
            cloud_seed: 0,
            cloud_quality: crate::clouds::CloudQuality::default(),
            stars: true,
            shadow_quality: ShadowQuality::High,
            contact_shadows: false,
            ambient_occlusion: crate::camera::AmbientOcclusion::default(),
            anti_aliasing: crate::camera::AntiAliasing::default(),
            forest_contours: Default::default(),
            eye_adaptation: Default::default(),
            depth_of_field: Default::default(),
            volumetrics: crate::volumetrics::VolumetricQuality::default(),
            bounce_light: true,
            weather_override: None,
            precipitation: true,
            rain: crate::rain::RainSettings::default(),
            look: Look::default(),
            air: Default::default(),
        }
    }
}

/// Derived sky state for other systems (terrain shader, audio, UI).
#[derive(Resource, Default, Clone)]
pub struct SkyState {
    pub sun_dir: Vec3,
    pub moon_dir: Vec3,
    /// 0 = day, 1 = full night
    pub night: f32,
    /// Wind drift of the cloud field plus the pattern seed's offset (m), wrapped to the field's
    /// period (`clouds::CLOUD_TILE`, seamless). Clouds and their shadows use `xz + cloud_offset`.
    pub cloud_offset: Vec2,
    pub weather: WeatherParams,
    /// Zone the viewer (camera focus or walking player) is in.
    pub viewer_zone: u8,
    pub viewer_pos: Vec3,
    /// The exposure's EV from the light alone (the look's and the slider's offsets taken out):
    /// what eye adaptation measures the view against (`camera_fx::adaptation`).
    pub scene_ev: f32,
    /// The sky fill the bounce light's volume is drawn at (`bounce_light`): the environment
    /// map's, without the cut indoors (the volume's rays find the roof themselves).
    pub bounce_fill: f32,
}

#[derive(Component)]
pub struct Sun;
#[derive(Component)]
pub struct Moon;
#[derive(Component)]
struct StarDome;

/// Night sky: stars, milky way and the moon (with phase), drawn additively behind the clouds.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct StarsMaterial {
    /// x: star intensity, y: sky rotation, z: time
    #[uniform(0)]
    pub params: Vec4,
    /// xyz: direction towards the moon, w: angular radius (rad)
    #[uniform(1)]
    pub moon: Vec4,
    /// xyz: direction towards the sun, w: moon brightness
    #[uniform(2)]
    pub sun: Vec4,
}

impl Material for StarsMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/stars.wesl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    /// Sort behind every other transparent surface so clouds cover the stars and moon.
    fn depth_bias(&self) -> f32 {
        -1.0e9
    }
}

/// Angular radius of the drawn moon (≈ 1.8° across: larger than life, as games show it).
const MOON_RADIUS: f32 = 0.0157;

/// Fraction of the moon's disc that is lit for a phase in 0..1.
pub fn moon_illumination(phase: f32) -> f32 {
    (1.0 - (phase * std::f32::consts::TAU).cos()) * 0.5
}

/// Direction towards the moon: it trails the sun by (1 − phase) of a day, so a full moon rises
/// at dusk and a first-quarter moon is highest at sunset. Its path runs lower than the sun's.
pub fn moon_direction(l: &Lighting) -> Vec3 {
    sun_direction(
        l.hour + (1.0 - l.moon_phase) * 24.0,
        (l.sun_tilt + 26.0).min(80.0),
        l.sun_azimuth + 25.0,
    )
}

pub struct SkyPlugin;

/// Day/night, weather and camera lighting systems (the look is applied around them).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkySystems;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Lighting>()
            .init_resource::<SkyState>()
            .insert_resource(DirectionalLightShadowMap { size: 4096 })
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.55, 0.62, 0.85),
                brightness: 0.0,
                ..default()
            })
            .add_plugins(MaterialPlugin::<StarsMaterial>::default())
            .add_systems(Startup, spawn_sky)
            .add_systems(
                Update,
                (
                    advance_time,
                    update_weather,
                    update_sun_moon,
                    update_camera_grading,
                    update_cascades,
                    update_sky_meshes,
                )
                    .chain()
                    .in_set(SkySystems)
                    .after(crate::camera::CameraSystems),
            );
    }
}

fn spawn_sky(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut stars: ResMut<Assets<StarsMaterial>>,
) {
    commands.spawn((
        Name::new("Sun"),
        Sun,
        DirectionalLight {
            illuminance: lux::RAW_SUNLIGHT,
            shadow_maps_enabled: true,
            contact_shadows_enabled: true,
            ..default()
        },
        SunDisk::EARTH,
        Transform::from_xyz(1.0, 1.0, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder::default().build(),
    ));
    commands.spawn((
        Name::new("Moon"),
        Moon,
        DirectionalLight {
            illuminance: 0.0,
            color: Color::srgb(0.8, 0.86, 1.0),
            shadow_maps_enabled: false,
            ..default()
        },
        // the moon disc is drawn by the night-sky shader (craters, phase, glow)
        SunDisk {
            angular_size: 0.0,
            intensity: 0.0,
        },
        Transform::from_xyz(-1.0, 1.0, -1.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder::default().build(),
    ));

    commands.spawn((
        Name::new("Stars"),
        StarDome,
        Mesh3d(meshes.add(inward_sphere(30_000.0, 48, 24))),
        MeshMaterial3d(stars.add(StarsMaterial {
            params: Vec4::ZERO,
            moon: Vec4::ZERO,
            sun: Vec4::ZERO,
        })),
        Transform::IDENTITY,
        NotShadowCaster,
        bevy::camera::visibility::NoFrustumCulling,
    ));
}

/// UV sphere with inward-facing triangles (seen from inside).
fn inward_sphere(r: f32, seg: u32, rings: u32) -> Mesh {
    let mut pos = Vec::new();
    let mut nor = Vec::new();
    for j in 0..=rings {
        let v = j as f32 / rings as f32 * std::f32::consts::PI;
        for i in 0..=seg {
            let u = i as f32 / seg as f32 * std::f32::consts::TAU;
            let d = Vec3::new(v.sin() * u.cos(), v.cos(), v.sin() * u.sin());
            pos.push((d * r).to_array());
            nor.push((-d).to_array());
        }
    }
    let mut idx = Vec::new();
    for j in 0..rings {
        for i in 0..seg {
            let a = j * (seg + 1) + i;
            let b = a + 1;
            let c = a + seg + 1;
            let d = c + 1;
            // counter-clockwise when seen from inside the sphere
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_indices(Indices::U32(idx));
    m
}

fn advance_time(
    mut lighting: ResMut<Lighting>,
    time: Res<Time>,
    mut sky: ResMut<SkyState>,
    mut seed: Local<u32>,
) {
    if lighting.playing {
        let hours_per_sec = 24.0 / (lighting.day_minutes.max(0.05) * 60.0);
        lighting.hour = (lighting.hour + time.delta_secs() * hours_per_sec).rem_euclid(24.0);
    }
    let wind = Vec2::new(14.0, 6.0) * lighting.cloud_speed * (1.0 + sky.weather.rain * 1.5);
    // a new pattern seed moves the field to another part of its (periodic) tile
    let jump = crate::clouds::seed_offset(lighting.cloud_seed) - crate::clouds::seed_offset(*seed);
    *seed = lighting.cloud_seed;
    sky.cloud_offset =
        crate::clouds::wrap_offset(sky.cloud_offset + jump + wind * time.delta_secs());
}

/// Direction *towards* the sun for a given hour.
pub fn sun_direction(hour: f32, tilt_deg: f32, azimuth_deg: f32) -> Vec3 {
    let theta = (hour - 6.0) / 24.0 * std::f32::consts::TAU;
    let tilt = tilt_deg.to_radians();
    let az = azimuth_deg.to_radians();
    let east = Vec3::new(az.cos(), 0.0, az.sin());
    let south = Vec3::new(-az.sin(), 0.0, az.cos());
    let up = Vec3::Y;
    (east * theta.cos() + (up * tilt.cos() + south * tilt.sin()) * theta.sin()).normalize()
}

fn update_weather(
    lighting: Res<Lighting>,
    zones: Res<Zones>,
    terrain: Option<Res<Terrain>>,
    mut sky: ResMut<SkyState>,
    time: Res<Time>,
) {
    let base = Weather::Clear.params();
    let mut target = WeatherParams {
        clouds: lighting.cloud_coverage,
        ..base
    };
    let zone_id = terrain
        .as_ref()
        .map(|t| t.zone_at(sky.viewer_pos.x, sky.viewer_pos.z))
        .unwrap_or(0);
    sky.viewer_zone = zone_id;
    if let Some(w) = lighting.weather_override {
        target = w.params();
        target.clouds = target.clouds.max(lighting.cloud_coverage);
    } else {
        // a zone's own weather wins; elsewhere the biome under the viewer sets it
        let zone = zones.by_id(zone_id);
        let biome = terrain
            .as_ref()
            .and_then(|t| {
                crate::biomes::BiomePreset::from_id(t.biome_at(sky.viewer_pos.x, sky.viewer_pos.z))
            })
            .map(|b| b.def());
        let weather = zone
            .filter(|z| z.weather_enabled)
            .map(|z| (z.weather, z.weather_intensity))
            .or(biome.as_ref().map(|d| (d.weather, d.weather_intensity)));
        if let Some((w, k)) = weather {
            let p = w.params().scaled(k);
            target = p;
            target.clouds = p.clouds.max(lighting.cloud_coverage);
        }
        // a zone's own cloud cover replaces the world's (its weather can still thicken it)
        if let Some(z) = zone.filter(|z| z.clouds_enabled) {
            target.clouds = if z.weather_enabled {
                target.clouds.max(z.cloud_cover)
            } else {
                z.cloud_cover
            };
        }
    }
    if !lighting.precipitation {
        target.rain = 0.0;
        target.snow = 0.0;
        target.wet = 0.0;
    }
    let k = 1.0 - (-time.delta_secs() * 0.6).exp();
    sky.weather = sky.weather.lerp(target, k);
}

#[allow(clippy::type_complexity)]
/// Indoors (`motion::indoors`): the warm ambient fill at full day, how much of the sky fill is
/// cut, and how many stops the exposure opens.
const INDOOR_FILL: f32 = 3400.0;
const INDOOR_SKY_CUT: f32 = 0.85;
const INDOOR_EV: f32 = 1.9;
/// How much of the warm indoor fill the bounce light's volume takes over where it lights the
/// room (`bounce_light`: it brings the daylight in by the windows and doors itself, darker deep
/// inside; the rest of the fill stays, for the lamps and fires a room would have). Measured in
/// the Test-Ground inn at 8:30: by its door and windows a little brighter than without the
/// volume, its back wall about a third darker (a quarter with eye adaptation).
const BOUNCE_INDOOR_SHARE: f32 = 0.2;
/// Full-moon light (lux) and the cool ambient fill at night (× `night_brightness`). The moon is
/// the key and the fill stays well under it, so night scenes keep their form and their colours
/// (2600 of very blue fill against a 1400 lux moon flattened everything into a grey-blue haze);
/// a brighter moon also brightens the sky it lights, so it only rose a little.
const MOON_LUX: f32 = 1750.0;
const NIGHT_FILL: f32 = 1300.0;
/// Exponential-squared fog density (per metre) a zone's full haze & fog adds (`ZoneLight::fog`).
pub const ZONE_FOG_DENSITY: f32 = 0.002;
/// Deep in a cave the exposure opens this many stops (by day; less at night), instead of the
/// indoor opening (eyes adjusting to the dark, walking or not).
const CAVE_EV: f32 = 5.0;

/// The outdoor distance-fog density after applying the world haze switch. Dungeon interiors use
/// their own fog below, and never come through this path.
fn outdoor_fog_density(
    haze: crate::haze::HazeState,
    look: &crate::look::LookParams,
    weather: &WeatherParams,
    zone: &crate::world::ZoneLight,
    local: f32,
) -> f32 {
    if haze.strength <= 1e-4 {
        return 0.0;
    }
    (0.000_05 * haze.strength + haze.effective_amount() * 0.000_12) * look.haze
        + (weather.fog * 0.0016
            + weather.rain * 0.0004
            + weather.snow * 0.0006
            + zone.fog * ZONE_FOG_DENSITY)
            * local
            * haze.strength
}

fn update_sun_moon(
    lighting: Res<Lighting>,
    mut sky: ResMut<SkyState>,
    mut sun: Query<(&mut Transform, &mut DirectionalLight), (With<Sun>, Without<Moon>)>,
    mut moon: Query<(&mut Transform, &mut DirectionalLight), (With<Moon>, Without<Sun>)>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut cam: Query<
        (
            &mut Exposure,
            &mut DistanceFog,
            &mut bevy::light::AtmosphereEnvironmentMapLight,
            Option<&mut bevy::light::EnvironmentMapLight>,
            Option<&mut bevy::pbr::AtmosphereSettings>,
        ),
        With<MainCamera>,
    >,
    look_state: Res<LookState>,
    indoors: Option<Res<crate::motion::indoors::Indoors>>,
    interior: Option<Res<crate::dungeons::render::InteriorLook>>,
    volumetrics: Res<crate::volumetrics::VolumetricsActive>,
    adapt: Res<crate::camera_fx::Adaptation>,
    bounce: Option<Res<crate::bounce_light::BounceLight>>,
) {
    let lk = &look_state.params;
    // inside a building: less sky fill, a warm ambient fill, a more open exposure
    let inside = indoors.as_ref().map(|i| i.amount).unwrap_or(0.0);
    let cave = indoors.as_ref().map(|i| i.cave).unwrap_or(0.0);
    let sun_dir = sun_direction(lighting.hour, lighting.sun_tilt, lighting.sun_azimuth);
    let moon_dir = moon_direction(&lighting);
    sky.sun_dir = sun_dir;
    sky.moon_dir = moon_dir;
    let night = 1.0 - smoothstep(-0.2, 0.03, sun_dir.y);
    sky.night = night;
    let w = sky.weather;

    let look = |d: Vec3| {
        let up = if d.y.abs() > 0.98 { Vec3::Z } else { Vec3::Y };
        Transform::IDENTITY.looking_to(-d, up)
    };
    if let Ok((mut t, mut l)) = sun.single_mut() {
        *t = look(sun_dir);
        let above = smoothstep(-0.06, 0.02, sun_dir.y);
        l.illuminance = lux::RAW_SUNLIGHT
            * lighting.sun_strength
            * lk.sun
            * (1.0 - w.dim * 0.85)
            * above.max(0.0001);
        let sc = lk.sun_color();
        l.color = Color::linear_rgb(sc.x, sc.y, sc.z);
        l.shadow_maps_enabled =
            sun_dir.y > -0.02 && !std::env::var("WB_PERF").is_ok_and(|p| p.contains("noshadow"));
        l.contact_shadows_enabled = lighting.contact_shadows;
    }
    if let Ok((mut t, mut l)) = moon.single_mut() {
        *t = look(moon_dir);
        let above = smoothstep(-0.02, 0.1, moon_dir.y);
        let lit = 0.12 + 0.88 * moon_illumination(lighting.moon_phase);
        l.illuminance =
            MOON_LUX * lighting.moon_strength * lit * night * above * (1.0 - w.dim * 0.7);
        l.shadow_maps_enabled = night > 0.6 && moon_dir.y > 0.05;
    }

    // night readability: a cool ambient fill that doesn't brighten the sky
    ambient.brightness = NIGHT_FILL * night * lighting.night_brightness + 600.0 * w.dim;
    ambient.color =
        Color::srgb(0.55, 0.62, 0.85).mix(&Color::srgb(0.8, 0.82, 0.85), w.dim * (1.0 - night));
    if inside > 0.0 {
        let day_in = smoothstep(-0.2, 0.18, sun_dir.y);
        let probe = bounce.as_ref().map(|b| b.cover).unwrap_or(0.0);
        let warm =
            INDOOR_FILL * inside * (0.25 + 0.75 * day_in) * (1.0 - BOUNCE_INDOOR_SHARE * probe);
        let total = ambient.brightness + warm;
        ambient.color = ambient
            .color
            .mix(&Color::srgb(1.0, 0.86, 0.7), warm / total.max(1e-3));
        ambient.brightness = total;
    }

    let day = smoothstep(-0.2, 0.18, sun_dir.y);
    // look: + exposure brightens (lower EV); stylised looks keep nights readable
    // twilight: once the sun is below the horizon its light is gone but the night fill is only
    // fading in, so open the exposure up for that stretch (it read almost black before)
    let twilight = (1.0 - smoothstep(-0.04, 0.01, sun_dir.y)) * smoothstep(-0.22, -0.10, sun_dir.y);
    // the look's and the slider's offsets (eye adaptation measures without them, and keeps them)
    let offsets = lighting.exposure_comp - lk.exposure - lk.night_lift * night;
    // indoors the eye (`camera_fx::adaptation`) measures the room and opens part of the way itself
    let indoor_ev = INDOOR_EV * (1.0 - crate::camera_fx::INDOOR_SHARE * adapt.strength);
    let ev = 10.4 + (14.6 - 10.4) * day - 1.85 * twilight - w.dim * 1.3 + offsets
        - (indoor_ev * inside * day).max(CAVE_EV * cave * (0.35 + 0.65 * day));
    sky.scene_ev = ev - offsets;
    if let Ok((mut exp, mut fog, mut env, live, atmo)) = cam.single_mut() {
        // (the eye opens (+) or closes (−) round the procedural exposure)
        exp.ev100 = ev - adapt.open;
        // in a cave there is no sky light scattered in the air between you and the rock (with
        // the eye adjusted to the dark, the aerial perspective of even a great hall would read
        // as a blue haze): it is cut to a couple of metres
        if let Some(mut a) = atmo {
            // Bevy's atmosphere has no separate aerial-perspective switch. A near-only LUT is
            // also what caves use and keeps the sky itself while removing the outdoor veil.
            let reach = if cave <= 0.35 {
                2.0 + (4.0e4 - 2.0) * look_state.haze.strength
            } else {
                2.0
            };
            if a.aerial_view_lut_max_distance != reach {
                a.aerial_view_lut_max_distance = reach;
            }
        }
        let fill = 1.7 * lk.sky_fill * (1.0 - INDOOR_SKY_CUT * inside.max(cave));
        sky.bounce_fill = 1.7 * lk.sky_fill * (1.0 - INDOOR_SKY_CUT * cave);
        if (env.intensity - fill).abs() > 1e-3 {
            env.intensity = fill;
        }
        // Bevy copies the atmosphere light's intensity into the generated environment light
        // once; the live value is that component's
        if let Some(mut live) = live
            && (live.intensity - fill).abs() > 1e-3
        {
            live.intensity = fill;
        }
        // fog / haze colour follows the time of day
        let sunset = (1.0 - (sun_dir.y * 4.0).abs()).clamp(0.0, 1.0) * (1.0 - night);
        let day_col = Vec3::new(0.63, 0.70, 0.79);
        let sunset_col = Vec3::new(0.78, 0.55, 0.42);
        let night_col = Vec3::new(0.035, 0.05, 0.09);
        let storm_col = Vec3::new(0.42, 0.45, 0.5);
        let mut c = day_col.lerp(sunset_col, sunset).lerp(night_col, night);
        c = c.lerp(storm_col * (1.0 - night * 0.9), w.dim * 0.7);
        if w.dust > 0.0 {
            c = c.lerp(Vec3::new(0.75, 0.6, 0.4) * (1.0 - night * 0.85), w.dust);
        }
        // the viewer's zone: its fog colour, and its extra haze below
        let zl = &look_state.zone;
        c *= Vec3::from(zl.fog_tint);
        // the look's haze colour also colours the physical fog (keeping its brightness)
        if lk.color_haze > 0.0 {
            let hz = Color::srgb(lk.haze_color[0], lk.haze_color[1], lk.haze_color[2]).to_linear();
            let hz = Vec3::new(hz.red, hz.green, hz.blue);
            let lum = |v: Vec3| v.dot(Vec3::new(0.2126, 0.7152, 0.0722)).max(1e-4);
            c = c.lerp(hz * (lum(c) / lum(hz)), (lk.color_haze * 0.8).min(1.0));
        }
        // with volumetric light the weather's and the zone's fog live in the lit froxel volume; the
        // distance fog keeps the clear-air haze (and a little of the rest) for what lies past it
        let local = if volumetrics.0 { 0.25 } else { 1.0 };
        let density = outdoor_fog_density(look_state.haze, lk, &w, zl, local);
        let fog_alpha = look_state.haze.strength
            * (look_state.haze.amount * 0.5 + w.fog + zl.fog).clamp(0.0, 1.0);
        fog.color = Color::linear_rgba(c.x, c.y, c.z, fog_alpha);
        fog.directional_light_color = if look_state.haze.strength > 1e-4 {
            Color::linear_rgba(
                1.0,
                0.75,
                0.5,
                0.35 * (1.0 - night) * (1.0 - w.dim) * look_state.haze.strength,
            )
        } else {
            Color::NONE
        };
        fog.directional_light_exponent = 18.0;
        fog.falloff = FogFalloff::ExponentialSquared { density };
    }
    // a dungeon's underground map: no sun, moon or sky (unless it has a sky over its open rooms:
    // then they shine as by day or night, the walls casting their shadows); the mood's fill,
    // exposure and fog
    if let Some(il) = interior.filter(|i| i.active) {
        if !il.sky {
            for (_, mut l) in sun.iter_mut().chain(moon.iter_mut()) {
                l.illuminance = 0.0;
                l.shadow_maps_enabled = false;
            }
        }
        ambient.color = il.ambient;
        ambient.brightness = il.ambient_brightness;
        if let Ok((mut exp, mut fog, mut env, live, atmo)) = cam.single_mut() {
            exp.ev100 = il.ev100;
            env.intensity = 0.0;
            if let Some(mut live) = live {
                live.intensity = 0.0;
            }
            if let Some(mut a) = atmo
                && a.aerial_view_lut_max_distance != 2.0
            {
                a.aerial_view_lut_max_distance = 2.0;
            }
            fog.color = il.fog.with_alpha(1.0);
            fog.directional_light_color = Color::NONE;
            fog.falloff = FogFalloff::Exponential {
                density: il.fog_density,
            };
        }
    }
}

/// Perceptual white balance (−1..1 units) → Bevy's CIE xy white-point offsets. Warming moves the
/// white point along the daylight (Planckian) curve, so warm reads golden rather than pink;
/// 1.0 ≈ a 1500 K shift.
pub fn white_balance(temperature: f32, tint: f32) -> (f32, f32) {
    (0.03 * temperature, -0.026 * temperature + 0.02 * tint)
}

fn update_camera_grading(
    mut commands: Commands,
    look: Res<LookState>,
    cam: Query<(Entity, Option<&mut ColorGrading>), With<MainCamera>>,
) {
    // the art style with the viewer's zone blended in (`look`)
    let lk = &look.params;
    // contrast is applied by the art-style post pass in display space: Bevy's section contrast
    // pivots at 0.5 in linear HDR, which crushes everything below mid-grey
    let (temperature, tint) = white_balance(lk.temperature, lk.tint);
    let grading = ColorGrading {
        global: ColorGradingGlobal {
            temperature,
            tint,
            post_saturation: lk.saturation,
            ..default()
        },
        shadows: ColorGradingSection::default(),
        midtones: ColorGradingSection::default(),
        highlights: ColorGradingSection::default(),
    };
    for (e, g) in cam {
        match g {
            Some(mut g) => *g = grading.clone(),
            None => {
                commands.entity(e).insert(grading.clone());
            }
        }
    }
}

fn update_cascades(
    lighting: Res<Lighting>,
    orbit: Res<crate::camera::OrbitCamera>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut lights: Query<&mut CascadeShadowConfig, With<DirectionalLight>>,
    mut last: Local<(f32, Option<ShadowQuality>, f32)>,
    graphics: Res<crate::streaming::Graphics>,
) {
    // walking (and flythroughs) the cascades span a few hundred metres, where 2048² shows the
    // same shadows as 4096² (compared side by side) for a quarter of the shadow-pass fill
    let size = if orbit.enabled {
        lighting.shadow_quality.map_size()
    } else {
        lighting.shadow_quality.map_size().min(2048)
    };
    if shadow_map.size != size {
        shadow_map.size = size;
    }
    // Fit cascades to what the camera can see: close when walking, wide when overviewing.
    let d = if orbit.enabled {
        orbit.distance.max(20.0)
    } else {
        25.0
    };
    let changed = last.1 != Some(lighting.shadow_quality)
        || (d / last.0.max(1.0) - 1.0).abs() > 0.15
        || last.2 != graphics.detail;
    if !changed {
        return;
    }
    *last = (d, Some(lighting.shadow_quality), graphics.detail);
    // the detail setting also trims shadow cascades (each one re-renders the shadow casters)
    let by_detail = if graphics.detail < 0.8 {
        2
    } else if graphics.detail < 1.3 {
        3
    } else {
        4
    };
    let max_dist = (d * 3.5).clamp(250.0, 14_000.0);
    let first = (d * 0.45).clamp(18.0, max_dist * 0.25);
    let cfg = CascadeShadowConfigBuilder {
        num_cascades: lighting.shadow_quality.cascades().min(by_detail),
        minimum_distance: 0.3,
        first_cascade_far_bound: first,
        maximum_distance: max_dist,
        overlap_proportion: 0.25,
    }
    .build();
    for mut c in &mut lights {
        *c = cfg.clone();
    }
}

#[allow(clippy::type_complexity)]
fn update_sky_meshes(
    lighting: Res<Lighting>,
    look: Res<LookState>,
    sky: Res<SkyState>,
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q_stars: Query<(&mut Transform, &MeshMaterial3d<StarsMaterial>), With<StarDome>>,
    mut stars: ResMut<Assets<StarsMaterial>>,
) {
    let Ok(cam) = cam.single() else { return };
    let p = cam.translation();
    if let Ok((mut t, m)) = q_stars.single_mut() {
        t.translation = p;
        // clouds cover the sky locally (drawn over the dome); a heavy overcast still dims everything
        let overcast = 1.0 - smoothstep(0.55, 1.0, sky.weather.clouds) * 0.85;
        // (fog hides them too: they are drawn after the volumetric light)
        let vis = if lighting.stars {
            sky.night
                * overcast
                * (1.0 - 0.9 * sky.weather.fog.clamp(0.0, 1.0) * look.haze.strength)
        } else {
            0.0
        };
        // the moon is a bright disc at night and a pale ghost in daylight
        let moon_bright =
            (0.3 + 2.1 * sky.night) * overcast * lighting.moon_strength.clamp(0.2, 3.0);
        if let Some(mut mat) = stars.get_mut(&m.0) {
            mat.params = Vec4::new(
                vis,
                lighting.hour / 24.0 * std::f32::consts::TAU,
                time.elapsed_secs(),
                lighting.moon_phase,
            );
            mat.moon = sky.moon_dir.extend(MOON_RADIUS);
            mat.sun = sky.sun_dir.extend(moon_bright);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haze_switch_removes_and_restores_outdoor_distance_fog() {
        let mut haze = crate::haze::resolve(true, 0.7, None);
        let look = crate::look::LookParams::default();
        let weather = Weather::Fog.params();
        let zone = crate::world::ZoneLight {
            fog: 0.5,
            ..default()
        };
        let on = outdoor_fog_density(haze, &look, &weather, &zone, 1.0);
        haze = crate::haze::resolve(false, 0.7, None);
        assert_eq!(outdoor_fog_density(haze, &look, &weather, &zone, 1.0), 0.0);
        haze = crate::haze::resolve(true, 0.7, None);
        assert_eq!(outdoor_fog_density(haze, &look, &weather, &zone, 1.0), on);
    }
}
