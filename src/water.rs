//! Water: the ocean at sea level, rivers (drawn strokes that flow downhill and carve their bed),
//! lakes (drawn outlines carved into smooth basins, or a basin flood-filled with a click) and
//! waterfalls. Every body has its own `WaterStyle`: colour, depth colour, transparency, visibility,
//! waves, flow, foam, roughness, reflections, bioluminescence, inner glow, metallic, iridescence
//! and a behaviour, starting from one of the presets in `presets`. Fish schools can be dropped
//! into any water with a click. All water shares one shader that reads the terrain height texture
//! for the water column (the refracted view ray through it: colour, what shows of the bed) and
//! shore foam.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};

use crate::editor::{Editor, Tab, WaterTool};
use crate::fish::FishSchool;
use crate::history::History;
use crate::objects::{Library, ObjectStore};
use crate::terrain::Terrain;
use crate::terrain::material::TerrainGpu;
use crate::tools::Stroke;

pub mod ground;
pub mod lakes;
pub mod presets;
pub mod waterfall;

pub use presets::{BodyKind, ReflectionQuality, WaterBehaviour, WaterCategory, preset_style};
pub use waterfall::Waterfall;

/// Legacy named water types (worlds saved before the preset catalogue, biome definitions); each
/// maps onto a preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum WaterType {
    #[default]
    TemperateSea,
    TropicalSea,
    ArcticSea,
    StormySea,
    ClearLake,
    GlacialLake,
    MountainStream,
    MuddyRiver,
    Swamp,
    Custom,
}

#[allow(dead_code)]
impl WaterType {
    pub const ALL: [WaterType; 9] = [
        WaterType::TemperateSea,
        WaterType::TropicalSea,
        WaterType::ArcticSea,
        WaterType::StormySea,
        WaterType::ClearLake,
        WaterType::GlacialLake,
        WaterType::MountainStream,
        WaterType::MuddyRiver,
        WaterType::Swamp,
    ];
    pub fn name(self) -> &'static str {
        presets::preset(self.preset_id())
            .map(|p| p.name)
            .unwrap_or("Custom")
    }
    /// The full look of the preset this type maps to.
    pub fn style(self) -> WaterStyle {
        preset_style(self.preset_id())
    }
    /// The look for rivers in a place whose lakes have this type.
    pub fn river_style(self) -> WaterStyle {
        preset_style(self.river_preset_id())
    }
}

/// Appearance and motion of one body of water. Old worlds load through serde defaults and
/// `upgrade` (an empty `preset` marks a style saved before the catalogue).
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterStyle {
    pub kind: WaterType,
    /// Preset id this look came from, "custom" once edited ("" = saved by an older version).
    pub preset: String,
    /// Water colour: how a sandy floor `clarity` metres down looks (sRGB).
    pub shallow: [f32; 3],
    /// Depth colour: infinitely deep water (sRGB).
    pub deep: [f32; 3],
    /// Underwater visibility (m).
    pub clarity: f32,
    /// Wave height: the ocean's sea state (0 glassy .. 2.5 storm), ripples on lakes and rivers.
    pub waves: f32,
    pub foam: f32,
    /// Flow speed: river current multiplier; drift of lakes and the sea (×1.6 m/s).
    pub flow: f32,
    /// 1: see-through as the visibility allows … 0: opaque (paint-like: gold, blood, void).
    pub transparency: f32,
    /// Wave animation speed (the ocean's wave clock; ripple drift on lakes and rivers).
    pub wave_speed: f32,
    /// Flow direction for lakes and the sea (degrees; rivers flow along their course).
    pub flow_dir: f32,
    /// Foam where waves meet the shore.
    pub shore_foam: f32,
    /// 0 mirror … 1 frosted: blurs the reflections.
    pub roughness: f32,
    pub reflections: ReflectionQuality,
    /// Bioluminescence: light in crests, foam and sparkles once it gets dark.
    pub glow: [f32; 3],
    pub glow_strength: f32,
    /// Inner glow (arcane waters), day and night.
    pub emissive: [f32; 3],
    pub emissive_strength: f32,
    /// 0 water … 1 liquid metal (tinted by the water colour).
    pub metallic: f32,
    /// Thin-film rainbow sheen.
    pub iridescence: f32,
    pub behaviour: WaterBehaviour,
}

impl Default for WaterStyle {
    fn default() -> Self {
        // (an unmarked preset look: `upgrade` names it on first use)
        WaterStyle {
            preset: String::new(),
            ..preset_style("tropical_water")
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct River {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    /// Centerline points; y is the water surface height.
    pub points: Vec<[f32; 3]>,
    pub width: f32,
    pub depth: f32,
    #[serde(default = "stream_style")]
    pub style: WaterStyle,
    /// The ground its bed replaced (so removing or re-routing it gives the land back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<ground::RiverGround>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lake {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    pub seed: [f32; 2],
    pub level: f32,
    #[serde(default = "lake_style")]
    pub style: WaterStyle,
    /// Drawn lakes: the smooth closed shoreline (counter-clockwise). Empty for flood-filled lakes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<[f32; 2]>,
    /// Drawn lakes: the lake type (`lakes::SHAPES` id) deciding basin, shore and plants.
    #[serde(default)]
    pub shape: String,
    /// Drawn lakes: deepest point (m).
    #[serde(default)]
    pub depth: f32,
    /// Drawn lakes: shore plant density (0 none .. 2 lush).
    #[serde(default)]
    pub plants: f32,
    /// Plants this lake grew (replaced when it is re-carved).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plant_ids: Vec<u64>,
    /// The ground before the lake was carved (so it can shrink back without leaving a pit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<lakes::GroundPatch>,
    /// Drawn lakes carved to hold their water: wherever the land falls away the ground just
    /// outside the outline is raised into a lip above the surface, and the bed stays a bowl, so
    /// no water hangs over dry ground (lakes saved before keep their original carve).
    #[serde(default)]
    pub contained: bool,
}

impl Lake {
    pub fn is_drawn(&self) -> bool {
        self.outline.len() >= 3
    }
}

fn stream_style() -> WaterStyle {
    WaterType::MountainStream.style()
}
fn lake_style() -> WaterStyle {
    WaterType::ClearLake.style()
}

/// Which body of water is selected for editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterSel {
    Ocean,
    River(u32),
    Lake(u32),
    Fish(u32),
    Waterfall(u32),
}

#[derive(Resource, Clone, Default)]
pub struct WaterBodies {
    pub rivers: Vec<River>,
    pub lakes: Vec<Lake>,
    pub fish: Vec<FishSchool>,
    pub waterfalls: Vec<Waterfall>,
    pub next_id: u32,
    pub dirty: bool,
    /// Water surface height per terrain vertex for lakes and rivers (NEG_INFINITY = dry).
    pub level_map: Vec<f32>,
    pub level_res: usize,
    pub grid_cache: Option<(f32, f32)>,
    pub selected: Option<WaterSel>,
}

/// Settings of the Water tab's tools (lives in the `Editor`).
#[derive(Clone, Debug)]
pub struct WaterToolSettings {
    /// index into `lakes::SHAPES`
    pub lake_shape: usize,
    /// × the lake type's depth
    pub lake_depth: f32,
    /// shore plant density
    pub lake_plants: f32,
    pub shape_mode: lakes::ShapeMode,
    pub brush: crate::editor::Brush,
    pub fall_width: f32,
    pub fall_flow: f32,
    pub fall_mist: f32,
    pub fall_pool: bool,
    pub fall_style: WaterStyle,
    /// preset category shown in the inspector / for new rivers
    pub preset_tab: WaterCategory,
    pub new_tab: WaterCategory,
}

impl Default for WaterToolSettings {
    fn default() -> Self {
        Self {
            lake_shape: 0,
            lake_depth: 1.0,
            lake_plants: 1.0,
            shape_mode: lakes::ShapeMode::Expand,
            brush: crate::editor::Brush {
                size: 25.0,
                strength: 0.5,
                falloff: 0.5,
            },
            fall_width: 7.0,
            fall_flow: 1.0,
            fall_mist: 0.7,
            fall_pool: true,
            fall_style: preset_style("waterfall"),
            preset_tab: WaterCategory::Ocean,
            new_tab: WaterCategory::River,
        }
    }
}

impl WaterBodies {
    /// Is `p` (on the ground) under water by at least `margin` meters (sea, lake or river)?
    pub fn is_underwater(&self, p: Vec3, margin: f32) -> bool {
        self.water_level(p).is_some_and(|l| l - p.y > margin)
    }
    /// Surface level of a lake/river covering this point, if any (not the sea).
    pub fn water_level(&self, p: Vec3) -> Option<f32> {
        if self.level_map.is_empty() || self.level_res == 0 {
            return None;
        }
        let n = self.level_res + 1;
        let (half, cell) = self.grid_cache?;
        let gx = ((p.x + half) / cell).round();
        let gz = ((p.z + half) / cell).round();
        if gx < 0.0 || gz < 0.0 || gx >= n as f32 || gz >= n as f32 {
            return None;
        }
        let v = self.level_map[gz as usize * n + gx as usize];
        if v.is_finite() { Some(v) } else { None }
    }
    /// Surface level of any water (sea included) above ground point `p`.
    pub fn surface_at(&self, p: Vec3, sea: f32) -> f32 {
        self.water_level(p).unwrap_or(f32::NEG_INFINITY).max(sea)
    }
    /// Which body covers this point (waterfalls, rivers and lakes first, then the ocean).
    pub fn body_at(&self, t: &Terrain, p: Vec2) -> Option<WaterSel> {
        if let Some(f) = self
            .waterfalls
            .iter()
            .find(|f| f.distance_xz(p) < f.width * 0.7 + 1.0)
        {
            return Some(WaterSel::Waterfall(f.id));
        }
        let ground = t.height_at(p.x, p.y);
        for r in &self.rivers {
            let near = r.points.windows(2).any(|w| {
                let a = Vec2::new(w[0][0], w[0][2]);
                let b = Vec2::new(w[1][0], w[1][2]);
                let ab = b - a;
                let u = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                p.distance(a + ab * u) < r.width * 0.75
            });
            if near {
                return Some(WaterSel::River(r.id));
            }
        }
        for l in &self.lakes {
            if l.is_drawn() {
                let poly: Vec<Vec2> = l.outline.iter().map(|q| Vec2::from_array(*q)).collect();
                if lakes::inside(&poly, p) {
                    return Some(WaterSel::Lake(l.id));
                }
            }
        }
        if let Some(level) = self.water_level(Vec3::new(p.x, ground, p.y)) {
            for l in &self.lakes {
                if (l.level - level).abs() < 0.01 {
                    return Some(WaterSel::Lake(l.id));
                }
            }
        }
        if ground < t.sea_level {
            return Some(WaterSel::Ocean);
        }
        None
    }
    /// The style of a selected body (the ocean's lives in its own resource).
    pub fn style_mut(&mut self, sel: WaterSel) -> Option<&mut WaterStyle> {
        match sel {
            WaterSel::River(id) => self
                .rivers
                .iter_mut()
                .find(|r| r.id == id)
                .map(|r| &mut r.style),
            WaterSel::Lake(id) => self
                .lakes
                .iter_mut()
                .find(|l| l.id == id)
                .map(|l| &mut l.style),
            WaterSel::Waterfall(id) => self
                .waterfalls
                .iter_mut()
                .find(|f| f.id == id)
                .map(|f| &mut f.style),
            _ => None,
        }
    }
}

pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct WaterParams {
    pub shallow: Vec4,
    pub deep: Vec4,
    pub scatter: Vec4,
    /// half size, cell, verts per side, sea level
    pub grid: Vec4,
    /// x: kind (0 ocean, 1 lake, 2 river), y: clarity, z: wave scale, w: foam
    pub params: Vec4,
    /// xy: wind dir, z: rain, w: river flow speed
    pub wind: Vec4,
    /// foam, whitecaps, streaks, chop
    pub look: Vec4,
    /// hs, rain, wind, choppy
    pub look2: Vec4,
    /// direction to the key light, key strength
    pub sun: Vec4,
    /// key light colour in display units
    pub sun_col: Vec4,
    /// sky irradiance in display units, w: glitter
    pub sky: Vec4,
    pub casc: [Vec4; 3],
    /// rgb: extinction of the water column per metre (from the shallow colour and clarity)
    pub extinct: Vec4,
    /// rgb: in-scatter albedo (the colour of infinitely deep water, from the deep colour)
    pub inscat: Vec4,
    /// xy: drift of the gust / slick / windrow field (m), z: wind speed (m/s), w: gust amount
    pub detail: Vec4,
    /// x: cloud coverage, y: cloud shadow strength, zw: cloud offset (as the terrain's)
    pub clouds: Vec4,
    /// x: surface roughness, y: shore foam, z: wave speed, w: reflection quality (0, 1, 2)
    pub style: Vec4,
    /// x: transparency, y: metallic, z: iridescence, w: behaviour
    pub optics: Vec4,
    /// rgb: bioluminescence (linear × strength), w: darkness (0 day .. 1 night)
    pub glow: Vec4,
    /// rgb: inner glow (linear × strength)
    pub emis: Vec4,
    /// xy: flow direction (lakes, sea), z: flow speed (m/s), w: tide offset (m)
    pub flowv: Vec4,
    /// xy: body centre, z: body radius (whirlpool), w: storm amount
    pub body: Vec4,
    /// x: the undisturbed surface height for planar-reflection plane matching
    pub surface: Vec4,
    /// Projective transform for the most recently rendered planar reflection.
    pub reflection_clip: Mat4,
    /// x: valid, y: plane height, z: capture-to-current exposure scale, w: reserved
    pub reflection: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct WaterExt {
    #[uniform(100)]
    pub params: WaterParams,
    #[texture(101, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
    #[texture(102)]
    #[sampler(103)]
    pub disp0: Handle<Image>,
    #[texture(104)]
    pub disp1: Handle<Image>,
    #[texture(105)]
    pub disp2: Handle<Image>,
    #[texture(106)]
    pub norm0: Handle<Image>,
    #[texture(107)]
    pub norm1: Handle<Image>,
    #[texture(108)]
    pub norm2: Handle<Image>,
    #[texture(109)]
    pub noise: Handle<Image>,
    #[texture(110)]
    pub foam: Handle<Image>,
    /// the opaque scene's depth (copied before the water is drawn): what lies behind the surface
    #[texture(111, sample_type = "depth")]
    pub scene_depth: Handle<Image>,
    /// sunlight focused by the short waves on a floor 1.5 m (R) and 4 m (G) down
    #[texture(112)]
    pub caustics: Handle<Image>,
    /// The volumetric clouds' baked cover map (`clouds::CloudCoverMap`): the cloud shadows on the
    /// water read it instead of evaluating the cover noise per pixel (as the terrain's do).
    #[texture(113)]
    #[sampler(114)]
    pub cloud_cover: Handle<Image>,
    /// A linear HDR view of the dominant flat water plane, rendered before the main camera.
    #[texture(115)]
    #[sampler(116)]
    pub planar_reflection: Handle<Image>,
}

impl MaterialExtension for WaterExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/water.wesl".into()
    }
    /// Waves move the surface in the vertex shader: kept out of the depth prepass.
    fn enable_prepass() -> bool {
        false
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/water.wesl".into()
    }
    /// The water surface writes depth, so post effects (art-style haze, outlines) see the surface
    /// rather than the sea floor beneath it. It is drawn in the transmissive phase (it reads the
    /// scene behind it) but composes the refraction itself: Bevy's own screen-space transmission
    /// blur is compiled out. So are contact shadows: the surface isn't in the depth prepass they
    /// march through (they could only find the sea floor below it), their 30 cm reach is invisible
    /// on water, and their 16-step march per pixel cost 10–20 % of the water's GPU time.
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_write_enabled = Some(true);
        }
        if let Some(f) = descriptor.fragment.as_mut() {
            f.shader_defs.retain(|d| {
                let name: &str = match d {
                    bevy::shader::ShaderDefVal::Bool(n, _)
                    | bevy::shader::ShaderDefVal::Int(n, _)
                    | bevy::shader::ShaderDefVal::UInt(n, _) => n,
                };
                name != "STANDARD_MATERIAL_SPECULAR_TRANSMISSION"
                    && name != "STANDARD_MATERIAL_DIFFUSE_OR_SPECULAR_TRANSMISSION"
                    && name != "CONTACT_SHADOWS"
            });
            // the shader samples the clouds' cover map (bindings 113, 114) only when told: a build
            // without them still runs the current shader (it evaluates the cover per pixel)
            f.shader_defs.push(bevy::shader::ShaderDefVal::Bool(
                "CLOUD_COVER_MAP".into(),
                true,
            ));
        }
        Ok(())
    }
}

#[derive(Resource)]
pub struct WaterMaterials {
    pub ocean: Handle<WaterMaterial>,
}

#[derive(Component)]
struct Ocean;

/// A lake or river mesh and the body it renders.
#[derive(Component)]
struct WaterBodyMesh {
    sel: WaterSel,
    material: Handle<WaterMaterial>,
    /// centre and radius of the body (whirlpools swirl around it)
    center: Vec2,
    radius: f32,
}

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<WaterMaterial>::default(),
            MaterialPlugin::<waterfall::WaterfallMaterial>::default(),
            crate::water_render::WaterRenderPlugin,
        ))
        .init_resource::<WaterBodies>()
        .init_resource::<WaterStyle>()
        .add_systems(Startup, spawn_ocean)
        .add_systems(Update, water_tools.in_set(crate::tools::ToolSystems::Apply))
        .add_systems(
            PostUpdate,
            (
                rebuild_bodies,
                update_water_materials,
                waterfall::update_falls,
            )
                .chain()
                .after(crate::terrain::TerrainSystems::Gpu)
                .after(crate::water_render::WaterRenderSystems),
        );
    }
}

fn water_material(
    materials: &mut Assets<WaterMaterial>,
    kind: f32,
    tex: &crate::ocean::OceanTextures,
    depth: &crate::water_render::SceneDepth,
    reflection: &crate::water_render::WaterReflection,
) -> Handle<WaterMaterial> {
    materials.add(WaterMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            // opaque, but drawn after the opaque scene with a copy of it (any transmission > 0 puts
            // the material in the transmissive phase): the shader composes reflection, glint, the
            // refracted seabed and the light scattered in the water column itself
            alpha_mode: AlphaMode::Opaque,
            specular_transmission: 1e-4,
            perceptual_roughness: 0.05,
            reflectance: 0.35,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: WaterExt {
            params: WaterParams {
                params: Vec4::new(kind, 4.0, 0.7, 0.8),
                ..default()
            },
            height: Handle::default(),
            disp0: tex.disp[0].clone(),
            disp1: tex.disp[1].clone(),
            disp2: tex.disp[2].clone(),
            norm0: tex.norm[0].clone(),
            norm1: tex.norm[1].clone(),
            norm2: tex.norm[2].clone(),
            noise: tex.noise.clone(),
            foam: tex.foam.clone(),
            scene_depth: depth.image.clone(),
            caustics: tex.caustics.clone(),
            // (bound by `update_water_materials` once the clouds have baked it)
            cloud_cover: Handle::default(),
            planar_reflection: reflection.image.clone(),
        },
    })
}

fn spawn_ocean(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WaterMaterial>>,
    tex: Res<crate::ocean::OceanTextures>,
    depth: Res<crate::water_render::SceneDepth>,
    reflection: Res<crate::water_render::WaterReflection>,
) {
    let ocean = water_material(&mut materials, 0.0, &tex, &depth, &reflection);
    commands.spawn((
        Name::new("Ocean"),
        Ocean,
        // a polar grid centred under the camera (dense near the eye, out to the horizon)
        Mesh3d(meshes.add(crate::ocean::polar_grid(384, 0.25, 22.0, 5000.0))),
        MeshMaterial3d(ocean.clone()),
        Transform::IDENTITY,
        NotShadowCaster,
        RenderLayers::layer(crate::water_render::WATER_RENDER_LAYER),
        bevy::camera::visibility::NoFrustumCulling,
    ));
    commands.insert_resource(WaterMaterials { ocean });
}

/// Display-unit lighting the water shader needs for its in-water terms.
#[derive(Clone, Copy, Default)]
struct WaterLight {
    sun_dir: Vec3,
    sun_col: Vec3,
    key: f32,
    sky: Vec3,
    /// cloud coverage, cloud shadow strength, cloud offset
    clouds: Vec4,
    /// 0 day .. 1 night (bioluminescence shows in the dark)
    night: f32,
}

/// Where a body is and the time, for the behaviours that need them.
#[derive(Clone, Copy, Default)]
struct BodyInfo {
    center: Vec2,
    radius: f32,
    time: f32,
    surface: f32,
}

fn lin3(c: [f32; 3]) -> Vec3 {
    let l = Color::srgb(c[0], c[1], c[2]).to_linear();
    Vec3::new(l.red, l.green, l.blue)
}

/// Slow rise and fall of a tidal body (m).
pub fn tide_offset(style: &WaterStyle, time: f32) -> f32 {
    if style.behaviour != WaterBehaviour::Tidal {
        return 0.0;
    }
    let period = 48.0 / style.wave_speed.clamp(0.2, 3.0);
    (0.12 + 0.18 * style.waves.min(1.5)) * (time * std::f32::consts::TAU / period).sin()
}

#[allow(clippy::too_many_arguments)]
fn apply_style(
    p: &mut WaterParams,
    style: &WaterStyle,
    kind: f32,
    terrain: &Terrain,
    rain: f32,
    wind: Vec2,
    sea: &crate::ocean::OceanState,
    light: &WaterLight,
    drift: Vec2,
    body: BodyInfo,
) {
    let lin = |c: [f32; 3]| lin3(c).extend(1.0);
    p.shallow = lin(style.shallow);
    p.deep = lin(style.deep);
    // light scattered inside the water: a brighter, greener version of the shallow colour
    p.scatter = lin(style.shallow) * 0.35 + Vec4::new(0.0, 0.02, 0.015, 0.0);
    let tide = tide_offset(style, body.time);
    p.grid = Vec4::new(
        terrain.half(),
        terrain.cell,
        terrain.n() as f32,
        terrain.sea_level + if kind == 0.0 { tide } else { 0.0 },
    );
    let wave_scale = if kind == 0.0 { 1.0 } else { style.waves * 0.6 };
    p.params = Vec4::new(kind, style.clarity, wave_scale, style.foam);
    p.wind = Vec4::new(wind.x, wind.y, rain, style.flow);
    let st = &sea.state;
    p.look = Vec4::new(st.foam, st.whitecaps, st.streaks, st.chop);
    p.look2 = Vec4::new(st.hs(), rain, st.wind, st.choppy);
    p.sun = light.sun_dir.extend(light.key);
    p.sun_col = light.sun_col.extend(1.0);
    p.sky = light.sky.extend(1.0);
    for c in 0..3 {
        let l = crate::ocean::CASCADE_L[c];
        p.casc[c] = Vec4::new(0.0, 0.0, 1.0 / l, l / crate::ocean::N as f32);
    }
    let (ext, ins) = water_optics(style);
    p.extinct = ext.extend(style.clarity);
    p.inscat = ins.extend(0.0);
    p.detail = Vec4::new(drift.x, drift.y, wind_speed(sea), 1.0);
    p.clouds = light.clouds;
    p.style = Vec4::new(
        style.roughness.clamp(0.0, 1.0),
        style.shore_foam,
        style.wave_speed,
        style.reflections.id(),
    );
    p.optics = Vec4::new(
        style.transparency.clamp(0.0, 1.0),
        style.metallic.clamp(0.0, 1.0),
        style.iridescence.clamp(0.0, 1.0),
        style.behaviour.id(),
    );
    p.glow = (lin3(style.glow) * style.glow_strength).extend(light.night);
    p.emis = (lin3(style.emissive) * style.emissive_strength).extend(0.0);
    let a = style.flow_dir.to_radians();
    p.flowv = Vec4::new(a.cos(), a.sin(), style.flow * 1.6, tide);
    let storm = if style.behaviour == WaterBehaviour::Storm {
        1.0
    } else {
        0.0
    };
    p.body = Vec4::new(body.center.x, body.center.y, body.radius.max(1.0), storm);
    p.surface = Vec4::new(
        if kind == 0.0 {
            terrain.sea_level + tide
        } else {
            body.surface
        },
        0.0,
        0.0,
        0.0,
    );
}

/// Wind speed over the sea (m/s) for the sea state's wind (0 glassy .. 1 storm).
fn wind_speed(sea: &crate::ocean::OceanState) -> f32 {
    1.0 + sea.state.wind * 19.0
}

/// Absorption of pure water per metre in the red, green and blue bands.
const PURE_WATER: Vec3 = Vec3::new(0.3, 0.045, 0.012);

/// Per-channel optics of a water style. A sandy floor `clarity` metres down still shows through
/// with the shallow colour: the light that lit it and came back up crossed ~2.3× that depth of
/// water, so σt = −ln(shallow / sand) / (2.3 · clarity) (red dies first in a clear sea, blue first
/// in muddy water; the yellow of the sand is compensated so a turquoise preset reads turquoise).
/// Every channel fades out eventually. Infinitely deep water shows the deep colour (the in-scatter
/// albedo).
pub fn water_optics(style: &WaterStyle) -> (Vec3, Vec3) {
    let clarity = style.clarity.max(0.2);
    const SAND: Vec3 = Vec3::new(1.0, 0.8, 0.55);
    let r = (lin3(style.shallow) / SAND).clamp(Vec3::splat(0.004), Vec3::splat(0.92));
    let ext = (Vec3::new(-r.x.ln(), -r.y.ln(), -r.z.ln()).max(Vec3::splat(0.35))) / (2.3 * clarity);
    // never clearer than pure water, which absorbs red within metres
    let ext = ext.max(PURE_WATER);
    (ext, lin3(style.deep))
}

#[allow(clippy::too_many_arguments)]
fn update_water_materials(
    terrain: Res<Terrain>,
    gpu: Option<Res<TerrainGpu>>,
    mut ocean_style: ResMut<WaterStyle>,
    water: Res<WaterBodies>,
    (sky, lighting): (Res<crate::sky::SkyState>, Res<crate::sky::Lighting>),
    wind: Res<crate::objects::Wind>,
    sea: Res<crate::ocean::OceanState>,
    mats: Option<Res<WaterMaterials>>,
    (scene_depth, reflection, cover): (
        Res<crate::water_render::SceneDepth>,
        Res<crate::water_render::WaterReflection>,
        Option<Res<crate::clouds::CloudCoverMap>>,
    ),
    mut materials: ResMut<Assets<WaterMaterial>>,
    mut ocean: Query<&mut Transform, (With<Ocean>, Without<crate::camera::MainCamera>)>,
    bodies: Query<&WaterBodyMesh>,
    (time, mut drift): (Res<Time>, Local<Vec2>),
    cam: Query<
        (
            &GlobalTransform,
            &bevy::camera::Exposure,
            &bevy::pbr::DistanceFog,
        ),
        With<crate::camera::MainCamera>,
    >,
    lights: Query<(&DirectionalLight, Has<crate::sky::Sun>)>,
) {
    let (Some(gpu), Some(mats)) = (gpu, mats) else {
        return;
    };
    let Ok((cam_t, exposure, fog)) = cam.single() else {
        return;
    };
    // an ocean style from an older world takes its preset's newer settings
    if ocean_style.preset.is_empty() {
        ocean_style.upgrade(BodyKind::Ocean);
    }
    let now = time.elapsed_secs();
    // the ocean grid follows the eye, turned to where it looks (its spokes are densest ahead)
    if let Ok(mut t) = ocean.single_mut() {
        let p = cam_t.translation();
        let f = cam_t.forward();
        let yaw = (-f.x).atan2(-f.z);
        t.translation = Vec3::new(
            p.x,
            terrain.sea_level + tide_offset(&ocean_style, now),
            p.z,
        );
        t.rotation = Quat::from_rotation_y(yaw);
    }
    // key light in display units: illuminance × exposure / π, warmed when the sun is low
    let exp = 1.0 / (2f32.powf(exposure.ev100) * 1.2);
    let night = sky.night > 0.5;
    let mut light = WaterLight::default();
    for (l, is_sun) in &lights {
        if is_sun != night {
            let dir = if is_sun { sky.sun_dir } else { sky.moon_dir };
            let warm = crate::noise::smoothstep(0.0, 0.35, dir.y);
            let tint = Vec3::new(1.0, 0.62, 0.38).lerp(Vec3::ONE, warm);
            let c = l.color.to_linear();
            light.sun_dir = dir;
            light.sun_col = Vec3::new(c.red, c.green, c.blue)
                * tint
                * (l.illuminance * exp / std::f32::consts::PI);
            light.key = (dir.y * 5.0 + 0.25).clamp(0.0, 1.0);
        }
    }
    let fc = fog.color.to_linear();
    light.sky = Vec3::new(fc.red, fc.green, fc.blue) * 0.8;
    // the clouds shade the sea like the land (same density field as the terrain's cloud shadows)
    light.clouds = Vec4::new(
        sky.weather.clouds,
        lighting.cloud_shadows * (1.0 - sky.night),
        sky.cloud_offset.x,
        sky.cloud_offset.y,
    );
    light.night = sky.night;

    // gusts, slicks and windrows drift downwind (integrated, so wind changes never make them jump)
    *drift += wind.dir.normalize_or_zero() * wind_speed(&sea) * 0.7 * time.delta_secs().min(0.1);
    let rain = sky.weather.rain;
    let ocean_body = BodyInfo {
        center: cam_t.translation().xz(),
        radius: 1000.0,
        time: now,
        surface: terrain.sea_level,
    };
    let mut targets: Vec<(Handle<WaterMaterial>, WaterStyle, f32, BodyInfo)> =
        vec![(mats.ocean.clone(), ocean_style.clone(), 0.0, ocean_body)];
    for b in &bodies {
        let style = match b.sel {
            WaterSel::River(id) => water.rivers.iter().find(|r| r.id == id).map(|r| {
                (
                    r.style.clone(),
                    2.0,
                    r.points
                        .get(r.points.len() / 2)
                        .map_or(terrain.sea_level, |p| p[1]),
                )
            }),
            WaterSel::Lake(id) => water.lakes.iter().find(|l| l.id == id).map(|l| {
                (
                    l.style.clone(),
                    1.0,
                    l.level,
                )
            }),
            _ => None,
        };
        if let Some((s, k, surface)) = style {
            targets.push((
                b.material.clone(),
                s,
                k,
                BodyInfo {
                    center: b.center,
                    radius: b.radius,
                    time: now,
                    surface,
                },
            ));
        }
    }
    for (h, style, kind, body) in targets {
        let Some(mut m) = materials.get_mut(&h) else {
            continue;
        };
        apply_style(
            &mut m.extension.params,
            &style,
            kind,
            &terrain,
            rain,
            wind.dir,
            &sea,
            &light,
            *drift,
            body,
        );
        if m.extension.height != gpu.height {
            m.extension.height = gpu.height.clone();
        }
        if m.extension.scene_depth != scene_depth.image {
            m.extension.scene_depth = scene_depth.image.clone();
        }
        if m.extension.planar_reflection != reflection.image {
            m.extension.planar_reflection = reflection.image.clone();
        }
        m.extension.params.reflection_clip = reflection.clip_from_world;
        m.extension.params.reflection = Vec4::new(
            if reflection.valid { 1.0 } else { 0.0 },
            reflection.plane_y,
            2.0_f32.powf(reflection.exposure_ev100 - exposure.ev100),
            0.0,
        );
        if let Some(cover) = &cover
            && m.extension.cloud_cover != cover.0
        {
            m.extension.cloud_cover = cover.0.clone();
        }
    }
}

fn smooth_path(pts: &[Vec2], iterations: usize) -> Vec<Vec2> {
    let mut p = pts.to_vec();
    for _ in 0..iterations {
        if p.len() < 3 {
            break;
        }
        let mut q = vec![p[0]];
        for w in p.windows(2) {
            q.push(w[0].lerp(w[1], 0.25));
            q.push(w[0].lerp(w[1], 0.75));
        }
        q.push(*p.last().unwrap());
        p = q;
    }
    p
}

fn resample(pts: &[Vec2], spacing: f32) -> Vec<Vec2> {
    if pts.len() < 2 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let seg = w[1] - w[0];
        let len = seg.length();
        let mut d = spacing - carry;
        while d <= len {
            out.push(w[0] + seg * (d / len));
            d += spacing;
        }
        carry = len - (d - spacing);
    }
    if out.last().unwrap().distance(*pts.last().unwrap()) > spacing * 0.3 {
        out.push(*pts.last().unwrap());
    }
    out
}

/// Waterfalls pouring down wherever a river (just made) drops steeply: each lands in the river
/// below, in the river's own water. Returns how many were added.
pub fn river_falls(t: &Terrain, water: &mut WaterBodies, river: u32) -> usize {
    let Some(r) = water.rivers.iter().find(|r| r.id == river) else {
        return 0;
    };
    let drops = waterfall::river_drops(&r.points);
    let (width, mut style) = (r.width, r.style.clone());
    style.foam = style.foam.max(0.9);
    for (a, b) in &drops {
        water.next_id += 1;
        let id = water.next_id;
        let flow = (0.8 + style.flow.clamp(0.0, 2.0) * 0.5).min(2.5);
        let path = waterfall::ballistic(t, *a, *b, flow);
        water.waterfalls.push(Waterfall {
            id,
            name: format!("Waterfall {id}"),
            path: path.iter().map(|p| p.to_array()).collect(),
            width: width * 0.85,
            flow,
            mist: 0.6,
            style: style.clone(),
            pool: None,
        });
    }
    if !drops.is_empty() {
        water.dirty = true;
    }
    drops.len()
}

/// Turn a raw stroke into a river that flows downhill, and carve its bed into the terrain.
pub fn make_river(
    t: &mut Terrain,
    h: &mut History,
    raw: &[Vec3],
    width: f32,
    depth: f32,
    id: u32,
) -> Option<River> {
    let pts: Vec<Vec2> = raw.iter().map(|p| p.xz()).collect();
    let mut pts = resample(
        &smooth_path(&resample(&pts, width * 0.8), 2),
        (width * 0.5).max(2.0),
    );
    if pts.len() < 3 {
        return None;
    }
    // rivers flow downhill: start at the higher end
    if t.height_at(pts[0].x, pts[0].y) < t.height_at(pts.last().unwrap().x, pts.last().unwrap().y) {
        pts.reverse();
    }
    // monotonic water surface following the ground
    let mut surf = Vec::with_capacity(pts.len());
    let mut s = t.height_at(pts[0].x, pts[0].y) - 0.5;
    for p in &pts {
        let g = t.height_at(p.x, p.y) - 0.5;
        s = s.min(g).max(t.sea_level - 0.05);
        surf.push(s);
    }
    // gently smooth the surface (keeping it monotonic) so it doesn't stair-step
    for _ in 0..3 {
        for i in 1..surf.len() - 1 {
            let avg = (surf[i - 1] + surf[i] + surf[i + 1]) / 3.0;
            surf[i] = avg.min(surf[i - 1]).max(surf[i + 1]);
        }
    }
    let hw = width * 0.5;
    let changed = carve_channel(t, h, &pts, &surf, hw, depth);
    Some(River {
        id,
        name: format!("River {id}"),
        points: pts.iter().zip(&surf).map(|(p, s)| [p.x, *s, p.y]).collect(),
        width,
        depth,
        style: WaterType::MountainStream.style(),
        ground: Some(ground::RiverGround::record(t.n(), &changed)),
    })
}

/// Carves the channel; returns the vertices it changed with their heights before.
fn carve_channel(
    t: &mut Terrain,
    h: &mut History,
    pts: &[Vec2],
    surf: &[f32],
    hw: f32,
    depth: f32,
) -> Vec<(usize, f32)> {
    let (mut mn, mut mx) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in pts {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let pad = hw * 2.5 + t.cell * 2.0;
    let Some(rect) = t.rect_for_circle((mn + mx) * 0.5, (mx - mn).length() * 0.5 + pad) else {
        return vec![];
    };
    h.touch_terrain(t, rect);
    let mut changed = vec![];
    for j in rect.z0..=rect.z1 {
        for i in rect.x0..=rect.x1 {
            let p = t.grid_to_world(i, j);
            if p.x < mn.x - pad || p.x > mx.x + pad || p.y < mn.y - pad || p.y > mx.y + pad {
                continue;
            }
            let idx = t.idx(i, j);
            let cur = t.heights[idx];
            if let Some(v) = ground::channel_height(p, cur, pts, surf, hw, depth) {
                t.heights[idx] = v;
                changed.push((idx, cur));
            }
        }
    }
    t.mark_heights_dirty(rect);
    changed
}

fn river_mesh(r: &River) -> Mesh {
    let pts: Vec<Vec3> = r.points.iter().map(|p| Vec3::from_array(*p)).collect();
    let hw = r.width * 0.5 * 1.25;
    let mut pos = vec![];
    let mut nor = vec![];
    let mut uv = vec![];
    let mut col = vec![];
    let mut dist = 0.0;
    for i in 0..pts.len() {
        let prev = pts[i.saturating_sub(1)];
        let next = pts[(i + 1).min(pts.len() - 1)];
        let tan = (next - prev).xz().normalize_or_zero();
        let side = Vec2::new(-tan.y, tan.x);
        if i > 0 {
            dist += pts[i].xz().distance(pts[i - 1].xz());
        }
        let drop = ((prev.y - next.y) / (next.xz().distance(prev.xz())).max(0.1)).max(0.0);
        let p = pts[i];
        for (s, u) in [(-1.0, 0.0), (1.0, 1.0)] {
            pos.push([p.x + side.x * hw * s, p.y, p.z + side.y * hw * s]);
            nor.push([0.0, 1.0, 0.0]);
            uv.push([u, dist / r.width]);
            col.push([
                tan.x * 0.5 + 0.5,
                tan.y * 0.5 + 0.5,
                (drop * 2.0).min(1.0),
                1.0,
            ]);
        }
    }
    let mut idx = vec![];
    for i in 0..pts.len() as u32 - 1 {
        let a = i * 2;
        idx.extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
    }
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Flood fill from `seed` under `level`. Returns filled vertex indices or an error message.
pub(crate) fn flood(t: &Terrain, seed: Vec2, level: f32) -> Result<Vec<usize>, &'static str> {
    let n = t.n();
    let g = t.world_to_grid(seed.x, seed.y);
    let (si, sj) = (g.x.round() as usize, g.y.round() as usize);
    if si >= n || sj >= n {
        return Err("Outside the world");
    }
    let mut seen = vec![false; n * n];
    let mut out = vec![];
    let mut q = VecDeque::new();
    q.push_back((si, sj));
    seen[sj * n + si] = true;
    let max = (n * n / 6).max(20_000);
    while let Some((i, j)) = q.pop_front() {
        let k = j * n + i;
        if t.heights[k] >= level {
            continue;
        }
        if t.heights[k] < t.sea_level - 0.5 {
            return Err(
                "That basin opens to the sea — lower the fill level or pick a higher basin",
            );
        }
        if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
            return Err("The water would spill off the edge of the world");
        }
        out.push(k);
        if out.len() > max {
            return Err("That basin is too large — lower the fill level");
        }
        for (di, dj) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (ni, nj) = ((i as i32 + di) as usize, (j as i32 + dj) as usize);
            let nk = nj * n + ni;
            if !seen[nk] {
                seen[nk] = true;
                q.push_back((ni, nj));
            }
        }
    }
    Ok(out)
}

fn lake_mesh(t: &Terrain, cells: &[usize], level: f32) -> Mesh {
    let n = t.n();
    let mut filled = vec![false; n * n];
    for &k in cells {
        filled[k] = true;
    }
    let mut pos = vec![];
    let mut idx = vec![];
    let mut uv = vec![];
    // a quad per grid cell touching a filled vertex (terrain hides the overlap)
    let mut done = vec![false; n * n];
    for &k in cells {
        let (i, j) = (k % n, k / n);
        for (ci, cj) in [
            (i.wrapping_sub(1), j.wrapping_sub(1)),
            (i, j.wrapping_sub(1)),
            (i.wrapping_sub(1), j),
            (i, j),
        ] {
            if ci >= n - 1 || cj >= n - 1 {
                continue;
            }
            let ck = cj * n + ci;
            if done[ck] {
                continue;
            }
            done[ck] = true;
            let base = pos.len() as u32;
            for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = t.grid_to_world(ci + di, cj + dj);
                pos.push([p.x, level, p.y]);
                uv.push([p.x * 0.05, p.y * 0.05]);
            }
            idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
    }
    let nor = vec![[0.0, 1.0, 0.0]; pos.len()];
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, nor);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    m.insert_indices(Indices::U32(idx));
    m
}

/// Rebuild lake / river / waterfall meshes and the water level map when water changes.
#[allow(clippy::too_many_arguments)]
fn rebuild_bodies(
    mut commands: Commands,
    mut water: ResMut<WaterBodies>,
    terrain: Res<Terrain>,
    (mut materials, mut fall_mats): (
        ResMut<Assets<WaterMaterial>>,
        ResMut<Assets<waterfall::WaterfallMaterial>>,
    ),
    mut meshes: ResMut<Assets<Mesh>>,
    old: Query<Entity, Or<(With<WaterBodyMesh>, With<waterfall::FallMesh>)>>,
    tex: Res<crate::ocean::OceanTextures>,
    scene_depth: Res<crate::water_render::SceneDepth>,
    reflection: Res<crate::water_render::WaterReflection>,
    stroke: Res<Stroke>,
    sky: Res<crate::sky::SkyState>,
    mut last_rev: Local<u64>,
    mut ed: ResMut<Editor>,
) {
    // lakes follow terrain edits once a stroke finishes
    let terrain_changed =
        *last_rev != terrain.height_revision && !stroke.active && !water.lakes.is_empty();
    if !water.dirty
        && !terrain_changed
        && water.level_res == terrain.res
        && !water.level_map.is_empty()
    {
        return;
    }
    *last_rev = terrain.height_revision;
    water.dirty = false;
    // styles saved by older versions take their preset's newer settings
    for l in &mut water.lakes {
        l.style.upgrade(BodyKind::Lake);
    }
    for r in &mut water.rivers {
        r.style.upgrade(BodyKind::River);
    }
    for f in &mut water.waterfalls {
        f.style.upgrade(BodyKind::River);
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    let n = terrain.n();
    let mut level = vec![f32::NEG_INFINITY; n * n];
    let mut failed = vec![];
    for lake in &water.lakes {
        let (mesh, center, radius) = if lake.is_drawn() {
            // drawn lakes: the smooth outline is the shore (never drains)
            let poly: Vec<Vec2> = lake.outline.iter().map(|p| Vec2::from_array(*p)).collect();
            lakes::stamp_level(&terrain, &poly, lake.level, &mut level);
            let area = lakes::signed_area(&poly).abs();
            (
                lakes::lake_mesh(&poly, lake.level),
                lakes::centroid(&poly),
                (area / std::f32::consts::PI).sqrt(),
            )
        } else {
            match flood(&terrain, Vec2::from_array(lake.seed), lake.level) {
                Ok(cells) => {
                    let mut c = Vec2::ZERO;
                    for &k in &cells {
                        level[k] = level[k].max(lake.level);
                        c += terrain.grid_to_world(k % n, k / n);
                    }
                    let c = c / cells.len().max(1) as f32;
                    let radius = (cells.len() as f32 * terrain.cell * terrain.cell
                        / std::f32::consts::PI)
                        .sqrt();
                    (lake_mesh(&terrain, &cells, lake.level), c, radius)
                }
                Err(_) => {
                    failed.push(lake.id);
                    continue;
                }
            }
        };
        let material = water_material(&mut materials, 1.0, &tex, &scene_depth, &reflection);
        commands.spawn((
            Name::new(format!("Lake {}", lake.id)),
            WaterBodyMesh {
                sel: WaterSel::Lake(lake.id),
                material: material.clone(),
                center,
                radius,
            },
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            NotShadowCaster,
            RenderLayers::layer(crate::water_render::WATER_RENDER_LAYER),
        ));
    }
    if !failed.is_empty() {
        water.lakes.retain(|l| !failed.contains(&l.id));
        ed.toast("A lake drained after the terrain changed");
    }
    for r in &water.rivers {
        let material = water_material(&mut materials, 2.0, &tex, &scene_depth, &reflection);
        let mid = r
            .points
            .get(r.points.len() / 2)
            .map(|p| Vec2::new(p[0], p[2]))
            .unwrap_or_default();
        commands.spawn((
            Name::new(format!("River {}", r.id)),
            WaterBodyMesh {
                sel: WaterSel::River(r.id),
                material: material.clone(),
                center: mid,
                radius: r.width,
            },
            Mesh3d(meshes.add(river_mesh(r))),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            NotShadowCaster,
            RenderLayers::layer(crate::water_render::WATER_RENDER_LAYER),
        ));
        // stamp the river into the level map
        let hw = r.width * 0.5;
        for w in r.points.windows(2) {
            let a = Vec3::from_array(w[0]);
            let b = Vec3::from_array(w[1]);
            let Some(rect) = terrain
                .rect_for_circle((a.xz() + b.xz()) * 0.5, a.xz().distance(b.xz()) * 0.5 + hw)
            else {
                continue;
            };
            for j in rect.z0..=rect.z1 {
                for i in rect.x0..=rect.x1 {
                    let p = terrain.grid_to_world(i, j);
                    let ab = b.xz() - a.xz();
                    let u = ((p - a.xz()).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                    if p.distance(a.xz() + ab * u) < hw {
                        let k = j * n + i;
                        level[k] = level[k].max(a.y + (b.y - a.y) * u);
                    }
                }
            }
        }
    }
    waterfall::spawn_falls(
        &mut commands,
        &mut meshes,
        &mut fall_mats,
        &tex,
        &water.waterfalls,
        &terrain,
        sky.night,
    );
    water.level_map = level;
    water.set_grid(&terrain);
}

/// Draw a lake: smooth the hand-drawn loop, carve the basin and shore, grow shore plants.
/// Returns the new lake's id.
#[allow(clippy::too_many_arguments)]
pub fn create_drawn_lake(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    water: &mut WaterBodies,
    raw: &[Vec2],
    shape: &lakes::LakeShape,
    depth_mult: f32,
    plants: f32,
) -> Result<u32, &'static str> {
    create_drawn_lake_at_level(
        t, h, store, lib, water, raw, shape, depth_mult, plants, None,
    )
}

/// Draw a lake at an authored level when supplied. The basin and its water surface must use
/// the same level from the outset; changing only the surface after carving leaves broken shores.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_drawn_lake_at_level(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    water: &mut WaterBodies,
    raw: &[Vec2],
    shape: &lakes::LakeShape,
    depth_mult: f32,
    plants: f32,
    level: Option<f32>,
) -> Result<u32, &'static str> {
    if level.is_some_and(|y| !y.is_finite()) {
        return Err("The lake level must be finite");
    }
    // A sheet can describe a valid polygon with only three to five corners. The drawing
    // smoother expects at least six mouse samples; subdivide those authored edges first.
    let sampled;
    let raw = if level.is_some() && (3..6).contains(&raw.len()) {
        sampled = raw
            .iter()
            .enumerate()
            .flat_map(|(i, p)| [*p, (*p + raw[(i + 1) % raw.len()]) * 0.5])
            .collect::<Vec<_>>();
        &sampled[..]
    } else {
        raw
    };
    let outline = lakes::smooth_outline(raw)
        .ok_or("Draw a closed loop around the area the lake should fill")?;
    if !outline.iter().all(|p| t.in_bounds(p.x, p.y)) {
        return Err("The lake must lie inside the world");
    }
    h.touch_water(water);
    water.next_id += 1;
    let id = water.next_id;
    let level = level.unwrap_or_else(|| lakes::level_for(t, &outline));
    let seed = lakes::interior_point(&outline);
    let mut lake = Lake {
        id,
        name: format!("{} {id}", shape.name),
        seed: seed.to_array(),
        level,
        style: preset_style(shape.water),
        outline: outline.iter().map(|p| p.to_array()).collect(),
        shape: shape.id.to_string(),
        depth: shape.depth * depth_mult,
        plants,
        plant_ids: vec![],
        ground: None,
        contained: true,
    };
    lakes::carve(t, h, store, lib, &mut lake, None);
    water.lakes.push(lake);
    water.selected = Some(WaterSel::Lake(id));
    water.dirty = true;
    Ok(id)
}

/// Re-carve a drawn lake after its outline, type, depth or plants changed (`prev`: the lake
/// before the edit).
pub fn recarve_lake(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    water: &mut WaterBodies,
    id: u32,
    prev: &Lake,
) {
    let Some(i) = water.lakes.iter().position(|l| l.id == id) else {
        return;
    };
    let mut lake = water.lakes[i].clone();
    if !lake.is_drawn() {
        return;
    }
    lakes::carve(t, h, store, lib, &mut lake, Some(prev));
    let poly: Vec<Vec2> = lake.outline.iter().map(|p| Vec2::from_array(*p)).collect();
    lake.seed = lakes::interior_point(&poly).to_array();
    water.lakes[i] = lake;
    water.dirty = true;
}

/// Place a waterfall from a dragged top to foot (and carve its plunge pool). Returns its id.
#[allow(clippy::too_many_arguments)]
pub fn create_waterfall(
    t: &mut Terrain,
    h: &mut History,
    store: &mut ObjectStore,
    lib: Option<&Library>,
    water: &mut WaterBodies,
    a: Vec3,
    b: Vec3,
    s: &WaterToolSettings,
) -> Result<u32, &'static str> {
    let (mut path, lands_in_water) = waterfall::build_path(t, water, a, b, s.fall_flow)?;
    let top = path[0];
    // spilling from a river: take its water
    let style = water
        .rivers
        .iter()
        .find(|r| {
            r.points
                .iter()
                .any(|p| Vec2::new(p[0], p[2]).distance(top.xz()) < r.width.max(6.0))
        })
        .map(|r| {
            let mut st = r.style.clone();
            st.foam = st.foam.max(0.9);
            st
        })
        .unwrap_or_else(|| s.fall_style.clone());
    h.touch_water(water);
    let mut pool = None;
    if s.fall_pool && !lands_in_water {
        let foot = *path.last().unwrap();
        let dir = (foot.xz() - top.xz()).normalize_or(Vec2::X);
        // (a basin dug into the slope behind a raised lip, holding water right up to its rim)
        let (c, r, level) = waterfall::pool_site(t, foot, dir, s.fall_width);
        let side = Vec2::new(-dir.y, dir.x);
        let raw: Vec<Vec2> = (0..40)
            .map(|k| {
                let a = k as f32 / 40.0 * std::f32::consts::TAU;
                let wob = 1.0
                    + 0.12 * crate::noise::value2(a.cos() * 2.0 + c.x * 0.01, a.sin() * 2.0, 5)
                    - 0.06;
                c + (dir * a.cos() * 1.05 + side * a.sin() * 0.9) * r * wob
            })
            .collect();
        if let Some(outline) = lakes::smooth_outline(&raw) {
            water.next_id += 1;
            let id = water.next_id;
            let shape = lakes::shape_by_id("plunge_pool");
            let mut pst = style.clone();
            pst.behaviour = WaterBehaviour::Bubbling;
            pst.foam = 1.0;
            pst.shore_foam = pst.shore_foam.max(0.8);
            pst.preset = "custom".into();
            let mut lake = Lake {
                id,
                name: format!("Plunge Pool {id}"),
                seed: c.to_array(),
                level,
                style: pst,
                outline: outline.iter().map(|p| p.to_array()).collect(),
                shape: shape.id.to_string(),
                depth: 1.5 + 0.6 * s.fall_flow.min(3.0),
                plants: 0.8,
                plant_ids: vec![],
                ground: None,
                contained: true,
            };
            lakes::carve(t, h, store, lib, &mut lake, None);
            water.lakes.push(lake);
            pool = Some(id);
            // the sheet falls into the pool, hugging the rock the basin cut, and ends where it
            // meets the water
            path = waterfall::land_on(
                &waterfall::ballistic(t, top, Vec3::new(foot.x, level, foot.z), s.fall_flow),
                level,
            );
        }
    }
    water.next_id += 1;
    let id = water.next_id;
    water.waterfalls.push(Waterfall {
        id,
        name: format!("Waterfall {id}"),
        path: path.iter().map(|p| p.to_array()).collect(),
        width: s.fall_width,
        flow: s.fall_flow,
        mist: s.fall_mist,
        style,
        pool,
    });
    water.selected = Some(WaterSel::Waterfall(id));
    water.dirty = true;
    Ok(id)
}

/// Which drawn lake a shoreline brush at `p` (radius `r`) works on: the selected one if it's in
/// reach, else the nearest shore.
fn lake_under_brush(water: &WaterBodies, p: Vec2, r: f32) -> Option<u32> {
    let near = |l: &Lake| {
        let poly: Vec<Vec2> = l.outline.iter().map(|q| Vec2::from_array(*q)).collect();
        lakes::sdf(&poly, p).abs()
    };
    if let Some(WaterSel::Lake(id)) = water.selected
        && let Some(l) = water.lakes.iter().find(|l| l.id == id && l.is_drawn())
        && near(l) < r * 1.5
    {
        return Some(id);
    }
    water
        .lakes
        .iter()
        .filter(|l| l.is_drawn())
        .map(|l| (l.id, near(l)))
        .filter(|(_, d)| *d < r * 1.5)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

#[allow(clippy::too_many_arguments)]
fn water_tools(
    stroke: Res<Stroke>,
    mut ed: ResMut<Editor>,
    mut terrain: ResMut<Terrain>,
    mut water: ResMut<WaterBodies>,
    mut history: ResMut<History>,
    mut store: ResMut<ObjectStore>,
    lib: Option<Res<Library>>,
    time: Res<Time>,
    mut shaping: Local<Option<(u32, Lake)>>,
) {
    if ed.tab != Tab::Water {
        return;
    }
    match ed.water_tool {
        WaterTool::River => {
            if stroke.just_ended {
                if stroke.path.len() < 4 {
                    ed.toast("Drag across the terrain to draw a river");
                    return;
                }
                history.touch_water(&water);
                water.next_id += 1;
                let id = water.next_id;
                let (w, d) = (ed.river_width, ed.river_depth);
                if let Some(mut r) = make_river(&mut terrain, &mut history, &stroke.path, w, d, id)
                {
                    r.style = ed.new_river_style.clone();
                    water.rivers.push(r);
                    water.selected = Some(WaterSel::River(id));
                    water.dirty = true;
                    match river_falls(&terrain, &mut water, id) {
                        0 => ed.toast("River created — it flows from the higher end downhill"),
                        n => ed.toast(format!("River created — it flows from the higher end downhill, over {n} waterfall{}", if n == 1 { "" } else { "s" })),
                    }
                }
            }
        }
        WaterTool::DrawLake => {
            if stroke.just_ended {
                let raw: Vec<Vec2> = stroke.path.iter().map(|p| p.xz()).collect();
                let s = &ed.water_tools;
                let shape = &lakes::SHAPES[s.lake_shape.min(lakes::SHAPES.len() - 1)];
                let (depth, plants) = (s.lake_depth, s.lake_plants);
                match create_drawn_lake(
                    &mut terrain,
                    &mut history,
                    &mut store,
                    lib.as_deref(),
                    &mut water,
                    &raw,
                    shape,
                    depth,
                    plants,
                ) {
                    Ok(id) => {
                        let l = water.lakes.iter().find(|l| l.id == id).unwrap();
                        let poly: Vec<Vec2> =
                            l.outline.iter().map(|p| Vec2::from_array(*p)).collect();
                        let area = lakes::signed_area(&poly);
                        ed.toast(format!(
                            "{} drawn: {:.0} m², {:.1} m deep — shape it with the Shore brush",
                            shape.name, area, l.depth
                        ));
                    }
                    Err(e) => ed.toast(e),
                }
            }
        }
        WaterTool::ShapeLake => {
            // (the re-carve on release must happen even if the pointer left the ground)
            if stroke.just_ended
                && let Some((id, prev)) = shaping.take()
            {
                recarve_lake(
                    &mut terrain,
                    &mut history,
                    &mut store,
                    lib.as_deref(),
                    &mut water,
                    id,
                    &prev,
                );
                ed.toast("Shore re-carved");
                return;
            }
            let Some(hit) = stroke.hit.or(stroke.last_hit) else {
                return;
            };
            let brush = ed.water_tools.brush;
            if stroke.just_started {
                *shaping = None;
                match lake_under_brush(&water, hit.xz(), brush.size) {
                    Some(id) => {
                        history.touch_water(&water);
                        let l = water.lakes.iter().find(|l| l.id == id).unwrap().clone();
                        water.selected = Some(WaterSel::Lake(id));
                        *shaping = Some((id, l));
                    }
                    None => ed.toast(
                        "Brush over the shore of a drawn lake to expand, shrink or smooth it",
                    ),
                }
            }
            if stroke.active
                && let Some((id, _)) = &*shaping
            {
                let mut mode = ed.water_tools.shape_mode;
                if stroke.inverse {
                    mode = match mode {
                        lakes::ShapeMode::Expand => lakes::ShapeMode::Shrink,
                        lakes::ShapeMode::Shrink => lakes::ShapeMode::Expand,
                        m => m,
                    };
                }
                let amount = brush.strength * 7.0 * time.delta_secs().min(0.05);
                if let Some(l) = water.lakes.iter_mut().find(|l| l.id == *id) {
                    lakes::deform(&mut l.outline, hit.xz(), brush.size, amount, mode);
                }
                water.dirty = true;
            }
        }
        WaterTool::Waterfall => {
            if stroke.just_ended {
                let (Some(a), Some(b)) =
                    (stroke.start_hit, stroke.path.last().copied().or(stroke.hit))
                else {
                    return;
                };
                if a.xz().distance(b.xz()) < 1.0 && (a.y - b.y).abs() < 1.0 {
                    ed.toast("Drag from the top of a cliff (or a river's edge) down to where the water should land");
                    return;
                }
                let s = ed.water_tools.clone();
                match create_waterfall(
                    &mut terrain,
                    &mut history,
                    &mut store,
                    lib.as_deref(),
                    &mut water,
                    a,
                    b,
                    &s,
                ) {
                    Ok(id) => {
                        let f = water.waterfalls.iter().find(|f| f.id == id).unwrap();
                        ed.toast(format!(
                            "Waterfall placed: {:.0} m drop{}",
                            f.drop(),
                            if f.pool.is_some() {
                                " with a plunge pool"
                            } else {
                                ""
                            }
                        ));
                    }
                    Err(e) => ed.toast(e),
                }
            }
        }
        WaterTool::Lake => {
            if stroke.just_started
                && let Some(hit) = stroke.hit
            {
                let level = hit.y + ed.lake_fill;
                match ground::flood_explained(&terrain, hit.xz(), level) {
                    Ok(cells) if cells.len() > 4 => {
                        history.touch_water(&water);
                        water.next_id += 1;
                        let id = water.next_id;
                        let style = ed.new_lake_style.clone();
                        water.lakes.push(Lake {
                            id,
                            name: format!("Lake {id}"),
                            seed: [hit.x, hit.z],
                            level,
                            style,
                            outline: vec![],
                            shape: String::new(),
                            depth: 0.0,
                            plants: 0.0,
                            plant_ids: vec![],
                            ground: None,
                            contained: false,
                        });
                        water.selected = Some(WaterSel::Lake(id));
                        water.dirty = true;
                        ed.toast(format!(
                            "Lake filled ({:.0} m²)",
                            cells.len() as f32 * terrain.cell * terrain.cell
                        ));
                    }
                    Ok(_) => {
                        ed.toast("Click inside a basin (a low area surrounded by higher ground)")
                    }
                    Err(e) => ed.toast(e),
                }
            }
        }
        WaterTool::Fish => {
            if stroke.just_started
                && let Some(hit) = stroke.hit
            {
                let surface = water.surface_at(hit, terrain.sea_level);
                if surface - hit.y < 0.8 {
                    ed.toast("Click on water at least knee-deep to release fish");
                    return;
                }
                history.touch_water(&water);
                water.next_id += 1;
                let id = water.next_id;
                let school = FishSchool::new(
                    id,
                    ed.fish_species,
                    Vec3::new(hit.x, surface, hit.z),
                    ed.fish_count,
                    ed.fish_radius,
                    &terrain,
                    &water,
                );
                let n = school.count;
                water.fish.push(school);
                water.selected = Some(WaterSel::Fish(id));
                let name = ed.fish_species.name();
                match crate::fish::habitat_hint(
                    ed.fish_species,
                    crate::fish::is_fresh(&water, &terrain, hit.xz()),
                ) {
                    Some(hint) => ed.toast(format!("Released {n} {name} — note: {hint}")),
                    None => ed.toast(format!("Released {n} {name} — exported as a fishing spot")),
                }
            }
        }
        WaterTool::Select => {
            if stroke.just_started
                && let Some(hit) = stroke.hit
            {
                // fish schools first, then water bodies
                let fish = water
                    .fish
                    .iter()
                    .find(|f| {
                        Vec2::new(f.center[0], f.center[2]).distance(hit.xz()) < f.radius.max(6.0)
                    })
                    .map(|f| f.id);
                water.selected = fish
                    .map(WaterSel::Fish)
                    .or_else(|| water.body_at(&terrain, hit.xz()));
                if water.selected.is_none() {
                    ed.toast("No water here");
                }
            }
        }
        WaterTool::Remove => {
            if stroke.just_started
                && let Some(hit) = stroke.hit
            {
                let p = hit.xz();
                if let Some(i) = water.fish.iter().position(|f| {
                    Vec2::new(f.center[0], f.center[2]).distance(p) < f.radius.max(6.0)
                }) {
                    history.touch_water(&water);
                    water.fish.remove(i);
                    ed.toast("Fish school removed");
                    return;
                }
                match water.body_at(&terrain, p) {
                    Some(WaterSel::Waterfall(id)) => {
                        history.touch_water(&water);
                        water.waterfalls.retain(|f| f.id != id);
                        water.dirty = true;
                        ed.toast("Waterfall removed (its plunge pool stays)");
                    }
                    Some(sel @ (WaterSel::River(id) | WaterSel::Lake(id))) => {
                        let r = ground::remove_bodies(
                            &mut terrain,
                            &mut history,
                            &mut store,
                            &mut water,
                            &[id],
                        );
                        let what = if matches!(sel, WaterSel::River(_)) {
                            "River"
                        } else {
                            "Lake"
                        };
                        ed.toast(match (r.restored, r.kept_beds) {
                            (1.., _) => format!("{what} removed and the ground it carved restored"),
                            (_, 1..) => "River removed (drawn before rivers remembered their ground: its bed stays, undo to bring the river back)".into(),
                            _ => format!("{what} removed"),
                        });
                    }
                    _ => ed.toast("Click on a river, lake, waterfall or fish school to remove it"),
                }
            }
        }
    }
}

/// Lake, river and waterfall surfaces exactly as the editor draws them (kind, name, mesh, style),
/// for the engine exporters (`crate::export`). Lakes that no longer hold water are left out.
pub fn export_bodies(
    water: &WaterBodies,
    t: &Terrain,
) -> Vec<(&'static str, String, Mesh, WaterStyle)> {
    let mut out = vec![];
    for l in &water.lakes {
        let mesh = if l.is_drawn() {
            let poly: Vec<Vec2> = l.outline.iter().map(|p| Vec2::from_array(*p)).collect();
            lakes::lake_mesh(&poly, l.level)
        } else {
            match flood(t, Vec2::from_array(l.seed), l.level) {
                Ok(cells) => lake_mesh(t, &cells, l.level),
                Err(_) => continue,
            }
        };
        out.push((
            "lake",
            if l.name.is_empty() {
                format!("Lake {}", l.id)
            } else {
                l.name.clone()
            },
            mesh,
            l.style.clone(),
        ));
    }
    for r in water.rivers.iter().filter(|r| r.points.len() >= 2) {
        out.push((
            "river",
            if r.name.is_empty() {
                format!("River {}", r.id)
            } else {
                r.name.clone()
            },
            river_mesh(r),
            r.style.clone(),
        ));
    }
    for f in &water.waterfalls {
        out.push((
            "waterfall",
            format!("Waterfall {}", f.id),
            waterfall::sheet_mesh(f, t),
            f.style.clone(),
        ));
    }
    out
}

impl WaterBodies {
    fn set_grid(&mut self, t: &Terrain) {
        self.level_res = t.res;
        self.grid_cache = Some((t.half(), t.cell));
    }
}

#[cfg(test)]
mod authored_lake_tests;
