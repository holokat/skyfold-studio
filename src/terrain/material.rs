//! The terrain material: Bevy's PBR `StandardMaterial` extended with procedural layer shading,
//! auto-texturing rules, splat painting, the brush cursor and zone / surface overlays.

use std::sync::OnceLock;

use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::{ShaderDefVal, ShaderRef};

use super::layers::{LAYER_COUNT, Layer};

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExt>;

pub const MAX_ZONES: usize = 64;

#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct TerrainParams {
    /// x: half size, y: cell size, z: vertices per side, w: sea level
    pub grid: Vec4,
    /// x: snow line (m above sea), y: snow blend, z: rock slope, w: beach height
    pub style: Vec4,
    /// x: dryness, y: wetness, z: time (s), w: snow cover boost (weather)
    pub style2: Vec4,
    /// xy: brush centre (world xz), z: radius, w: falloff (0..1)
    pub brush: Vec4,
    /// rgb: brush colour, a: visibility (0 hides)
    pub brush_color: Vec4,
    /// x: overlay mode (0 none, 1 zones, 2 footstep surfaces), y: zone fill opacity,
    /// z: draw zone boundaries (0/1), w: highlighted zone id
    pub overlay: Vec4,
    /// xyz: direction towards the sun, w: cloud shadow strength
    pub sun: Vec4,
    /// x: cloud coverage, y: cloud scale, zw: cloud offset (wind)
    pub clouds: Vec4,
    /// x: textures available (0/1), y: anti-tiling strength, z: normal-map strength,
    /// w: texture softening (mip bias, log2) for hand-painted looks
    pub tex: Vec4,
    /// Texture repeat size (m) per layer, packed 4 per vec4.
    pub layer_tile: [Vec4; 3],
    /// rgb: ocean shallow colour (linear), a: ocean clarity (m) — tints the sea floor
    pub water: Vec4,
    pub zone_colors: [Vec4; MAX_ZONES],
    /// Footstep-surface overlay colour of each layer.
    pub layer_overlay: [Vec4; LAYER_COUNT],
    /// x: opacity of the generic overlay image (`TerrainOverlay`; 0 off)
    pub overlay_image: Vec4,
    /// x: sea wave height (significant, m), y: shore foam, zw: wind direction (set by `visuals`)
    pub visuals: Vec4,
}

impl Default for TerrainParams {
    fn default() -> Self {
        Self {
            grid: Vec4::ZERO,
            style: Vec4::ZERO,
            style2: Vec4::ZERO,
            brush: Vec4::ZERO,
            brush_color: Vec4::ZERO,
            overlay: Vec4::ZERO,
            sun: Vec4::new(0.3, 0.8, 0.2, 0.0),
            clouds: Vec4::new(0.3, 1.0, 0.0, 0.0),
            tex: Vec4::new(0.0, 0.6, 1.0, 0.0),
            water: Vec4::new(0.1, 0.4, 0.4, 6.0),
            layer_tile: std::array::from_fn(|i| {
                let t = &super::textures::LAYER_TILE;
                Vec4::new(t[i * 4], t[i * 4 + 1], t[i * 4 + 2], t[i * 4 + 3])
            }),
            zone_colors: [Vec4::ZERO; MAX_ZONES],
            layer_overlay: std::array::from_fn(|i| {
                let c = Layer::from_index(i).default_surface().overlay_color();
                let l = Color::srgb_u8(c[0], c[1], c[2]).to_linear();
                Vec4::new(l.red, l.green, l.blue, 1.0)
            }),
            overlay_image: Vec4::ZERO,
            visuals: Vec4::ZERO,
        }
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
#[bind_group_data(TerrainKey)]
pub struct TerrainExt {
    #[uniform(100)]
    pub params: TerrainParams,
    #[texture(101)]
    #[sampler(102)]
    pub normal_map: Handle<Image>,
    #[texture(103)]
    pub splat0: Handle<Image>,
    #[texture(104)]
    pub splat1: Handle<Image>,
    #[texture(105)]
    pub splat2: Handle<Image>,
    #[texture(106, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
    #[texture(107, sample_type = "float", filterable = false)]
    pub zones: Handle<Image>,
    #[texture(108, dimension = "2d_array")]
    #[sampler(109)]
    pub albedo_array: Handle<Image>,
    #[texture(110, dimension = "2d_array")]
    pub detail_array: Handle<Image>,
    /// Generic overlay image (see `overlay::TerrainOverlay`).
    #[texture(111)]
    pub overlay: Handle<Image>,
    /// Nearest water per spot: level, distance, kind (see `visuals::shore`).
    #[texture(112, sample_type = "float", filterable = false)]
    pub shore: Handle<Image>,
    /// The volumetric clouds' baked cover map (`clouds::CloudCoverMap`): moving cloud shadows.
    #[texture(113)]
    #[sampler(114)]
    pub cloud_cover: Handle<Image>,
    /// The ground rules' slow noise fields, baked (`bake`).
    #[texture(115)]
    pub rule0: Handle<Image>,
    #[texture(116)]
    pub rule1: Handle<Image>,
    /// The shade under the trees (`nature::shade`).
    #[storage(120, read_only)]
    pub canopy: Handle<ShaderBuffer>,
    /// Draw with the reference shading (`terrain_reference.wesl`, see `ab`); A/B tests only.
    pub reference: bool,
    /// Evaluate the rules' noise per pixel instead of reading the baked maps (see `ab`); A/B tests only.
    pub per_pixel_rules: bool,
}

/// What the terrain pipeline is specialised on.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainKey {
    reference: bool,
    per_pixel_rules: bool,
}

impl From<&TerrainExt> for TerrainKey {
    fn from(e: &TerrainExt) -> Self {
        Self {
            reference: e.reference,
            per_pixel_rules: e.per_pixel_rules,
        }
    }
}

/// (terrain.wesl, terrain_reference.wesl): loaded only by runs that ask for the reference shading.
pub static REFERENCE_SHADERS: OnceLock<(Handle<Shader>, Handle<Shader>)> = OnceLock::new();

impl MaterialExtension for TerrainExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/terrain.wesl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // the shader samples the clouds' cover map (bindings 113, 114) only when told: a build
        // without them still runs the current shader (it evaluates the cover per pixel)
        if let Some(f) = descriptor.fragment.as_mut() {
            f.shader_defs
                .push(ShaderDefVal::Bool("CLOUD_COVER_MAP".into(), true));
            // likewise the rules' baked noise maps (bindings 115, 116, `bake`)
            if !key.bind_group_data.per_pixel_rules {
                f.shader_defs
                    .push(ShaderDefVal::Bool("RULE_NOISE_MAPS".into(), true));
            }
            // and the shade under the trees (binding 120)
            f.shader_defs
                .push(ShaderDefVal::Bool("CANOPY_SHADE".into(), true));
        }
        // (only the main pass's fragment shader is the terrain's own)
        if key.bind_group_data.reference
            && let Some((main, reference)) = REFERENCE_SHADERS.get()
            && let Some(f) = descriptor.fragment.as_mut()
            && f.shader == *main
        {
            f.shader = reference.clone();
        }
        Ok(())
    }
}

/// GPU-side handles for the terrain (textures + the single shared material).
#[derive(Resource, Clone)]
pub struct TerrainGpu {
    pub material: Handle<TerrainMaterial>,
    pub height: Handle<Image>,
    pub normal: Handle<Image>,
    pub splat: [Handle<Image>; 3],
    pub zones: Handle<Image>,
    /// The ground rules' baked noise maps (`bake`).
    pub rules: [Handle<Image>; 2],
    pub generation: Option<u64>,
}

pub struct TerrainMaterialPlugin;

impl Plugin for TerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
    }
}
