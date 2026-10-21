//! Air life: what sunlit air is full of round the explorer. Fine dust everywhere by day (only seen
//! where a sunbeam catches it), pollen and seed fluff over meadows and grassland, leaves falling
//! and tumbling under broadleaf trees and through woods (autumn's colours in autumn woods and
//! under maples), petals drifting off flowers, volcanic ash and embers near lava and in infernal
//! places. Nothing heavy at night (the fireflies are `motes`'), none in rain or snow, none indoors
//! but dust.
//!
//! The place decides the amounts: the terrain's biome map round the eye (sampled over a few tens
//! of metres, so borders blend), the procedural trees and flowers near it (`nature::render::Near`)
//! and placed tree models, lava within a few hundred metres, the zone's Infernal mood, the weather
//! and the hour. Never zones of its own.
//!
//! One mesh of a few thousand quads in boxes that travel with the camera (`air.wesl`): no work per
//! speck on the CPU. Each kind is a group of specks; the share asked for picks how many of its
//! group show, so amounts ease in and out. The specks read the sun's shadow map, so they shine
//! only inside sunbeams, scattering forwards: looking towards the sun the air sparkles (and the
//! volumetric fog, `volumetrics::FroxelFog`, veils them at their own depth). The wind
//! carries them (`wind::WindState`: the drift per kind is integrated here, so wind changes never
//! jump), the shared gust field sweeps them along, leaves and petals flutter and tumble as they
//! fall.
//!
//! Only seen close to the ground while the world moves (walking, a camera path, Preview motion):
//! from the editor's overview a box of specks round the camera means nothing. Settings (on/off,
//! density) are saved with the world (`sky::Lighting::air`); the Graphics detail scales them.
//! MCP: `air_particles`.

use bevy::asset::RenderAssetUsages;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::biomes::BiomePreset;
use crate::mcp_tools::{McpAppExt, McpContent, McpTool, opt_arg};
use crate::noise::{Rng, smoothstep};
use crate::procgen::meshes::AssetKind;

/// What drifts in the air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AirKind {
    Dust,
    Pollen,
    Leaves,
    Petals,
    Ash,
}

impl AirKind {
    pub const ALL: [AirKind; 5] = [
        AirKind::Dust,
        AirKind::Pollen,
        AirKind::Leaves,
        AirKind::Petals,
        AirKind::Ash,
    ];
    pub fn id(self) -> &'static str {
        match self {
            AirKind::Dust => "dust",
            AirKind::Pollen => "pollen",
            AirKind::Leaves => "leaves",
            AirKind::Petals => "petals",
            AirKind::Ash => "ash",
        }
    }
    /// The kind's group: how many specks it has, their half-size (m), the box they wrap in
    /// (width, height; m), how fast they fall (m/s) and how much of the wind carries them.
    pub fn group(self) -> Group {
        match self {
            AirKind::Dust => Group {
                count: 2000,
                size: 0.008,
                width: 16.0,
                height: 8.0,
                fall: 0.01,
                carry: 0.45,
            },
            AirKind::Pollen => Group {
                count: 700,
                size: 0.024,
                width: 24.0,
                height: 10.0,
                fall: -0.02,
                carry: 0.9,
            },
            AirKind::Leaves => Group {
                count: 700,
                size: 0.07,
                width: 20.0,
                height: 14.0,
                fall: 0.6,
                carry: 0.7,
            },
            AirKind::Petals => Group {
                count: 500,
                size: 0.03,
                width: 20.0,
                height: 10.0,
                fall: 0.45,
                carry: 0.85,
            },
            AirKind::Ash => Group {
                count: 600,
                size: 0.026,
                width: 24.0,
                height: 14.0,
                fall: 0.3,
                carry: 0.8,
            },
        }
    }
}

/// How much of each group the amounts asked for show at density 1 (a meadow's pollen and petals
/// fill their groups; density 2–3 fills the dust's too).
const BASE: f32 = 1.6;

#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub count: usize,
    pub size: f32,
    pub width: f32,
    pub height: f32,
    pub fall: f32,
    pub carry: f32,
}

/// The air-life settings, saved with the world (`sky::Lighting::air`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AirSettings {
    /// Dust, pollen, leaves, petals and ash drifting round the explorer.
    pub particles: bool,
    /// How many, times what the place brings (0..3).
    pub density: f32,
    /// Smoke rising from the models' chimneys, forges and cauldrons (`visuals::chimneys`).
    pub smoke: bool,
    /// How thick that smoke is (0..2).
    pub smoke_strength: f32,
}

impl Default for AirSettings {
    fn default() -> Self {
        Self {
            particles: true,
            density: 1.0,
            smoke: true,
            smoke_strength: 1.0,
        }
    }
}

pub type AirMaterial = ExtendedMaterial<StandardMaterial, AirExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct AirParams {
    /// per kind: x share shown (0..1), y half-size (m), z box width (m), w box height (m)
    pub kinds: [Vec4; 5],
    /// per kind: xyz travelled (wind drift, fall), wrapped to the box
    pub offsets: [Vec4; 5],
    /// per kind: rgb colour (linear), w: the kind's extra (leaves: autumn share, petals: white
    /// share, ash: ember share)
    pub tints: [Vec4; 5],
    /// rgb: sky fill (before exposure), w: brightness
    pub sky: Vec4,
    /// xy: wind direction (xz), z: speed (m/s), w: clock (s)
    pub wind: Vec4,
    /// xy: travelled gust offset (m), z: gustiness, w: how much of the layer shows
    pub gust: Vec4,
    /// `volumetrics::FroxelFog::params` (w 0: no volumetric light)
    pub fog: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct AirExt {
    #[uniform(100)]
    pub params: AirParams,
    /// The volumetric light's integrated volume (`volumetrics::FroxelFog`): specks drawn after
    /// the view was fogged fog themselves at their own depth (None while it is off).
    #[texture(101, dimension = "3d")]
    #[sampler(102)]
    pub fog_volume: Option<Handle<Image>>,
}

impl MaterialExtension for AirExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/air.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/air.wesl".into()
    }
}

#[derive(Component)]
pub struct AirField(Handle<AirMaterial>);

/// What the place round the eye is like (MCP `air_particles` reports it).
#[derive(Clone, Debug, Default)]
pub struct AirReading {
    pub biome: Option<BiomePreset>,
    /// broadleaf trees within ~24 m (procedural and placed), and how many of them are maples
    pub broadleaf_trees: usize,
    pub maples: usize,
    /// flowers and flowering bushes within ~14 m
    pub flowers: usize,
    /// nearest lava (m), within 400 m
    pub lava: Option<f32>,
    pub infernal: bool,
}

/// The air life now: eased amounts per kind, what the place is like, whether it shows.
#[derive(Resource, Default)]
pub struct AirState {
    /// share of each kind's group the place asks for (eased; before density and detail)
    pub amounts: [f32; 5],
    target: [f32; 5],
    /// leaves: share in autumn colours; petals: share of white ones; dust: how sandy
    pub autumn: f32,
    white: f32,
    sandy: f32,
    target_look: [f32; 3],
    pub reading: AirReading,
    /// how much of the layer shows (0: hidden, `hidden_because` says why)
    pub visible: f32,
    pub hidden_because: Option<&'static str>,
    /// the share shown per kind after density and detail (what the shader gets)
    pub shown: [f32; 5],
    offsets: [Vec3; 5],
    clock: f32,
    timer: f32,
    /// where the eye was last frame (a jump — a teleport, a new world — snaps the amounts)
    last_eye: Option<Vec3>,
    snap: bool,
}

/// Every speck: its position in the unit cube (wrapped round the camera in the shader); uv: the
/// corner; vertex colour: (seed, phase, size, rank in its group); normal: (kind, rate, 0).
pub fn field_mesh() -> Mesh {
    let mut rng = Rng::new(0xa1_2f);
    let total: usize = AirKind::ALL.iter().map(|k| k.group().count).sum();
    let (mut pos, mut uv, mut col, mut nor, mut idx) = (
        Vec::with_capacity(total * 4),
        Vec::with_capacity(total * 4),
        Vec::with_capacity(total * 4),
        Vec::with_capacity(total * 4),
        Vec::with_capacity(total * 6),
    );
    for (k, kind) in AirKind::ALL.iter().enumerate() {
        let n = kind.group().count;
        for i in 0..n {
            let p = [rng.f32(), rng.f32(), rng.f32()];
            // (ranks spread evenly, so any share shows that share of the group)
            let rank = (i as f32 + rng.f32()) / n as f32;
            let c = [rng.f32(), rng.f32(), rng.range(0.65, 1.4), rank];
            let rate = rng.f32();
            let base = pos.len() as u32;
            for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
                pos.push(p);
                uv.push(corner);
                col.push(c);
                nor.push([k as f32, rate, 0.0]);
            }
            idx.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
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

pub fn spawn_air(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<AirMaterial>>,
) {
    let material = mats.add(AirMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            alpha_mode: AlphaMode::Premultiplied,
            unlit: true,
            cull_mode: None,
            ..default()
        },
        extension: AirExt {
            params: AirParams::default(),
            fog_volume: None,
        },
    });
    commands.spawn((
        Name::new("Air life"),
        AirField(material.clone()),
        Mesh3d(meshes.add(field_mesh())),
        MeshMaterial3d(material),
        Transform::IDENTITY,
        NotShadowCaster,
        NotShadowReceiver,
        // (wrapped round the camera in the vertex shader; the entity rides along with the camera
        // so GPU culling, which goes by the mesh's own bounds, always keeps it)
        bevy::camera::visibility::NoFrustumCulling,
    ));
}

/// What a biome brings (dust, pollen, leaves, petals, ash; shares of each group) and how autumnal
/// its leaves are.
pub fn biome_air(b: Option<BiomePreset>) -> ([f32; 5], f32) {
    use BiomePreset::*;
    match b {
        Some(Meadow) => ([0.45, 0.7, 0.04, 0.3, 0.0], 0.0),
        Some(TemperateForest) => ([0.45, 0.3, 0.25, 0.05, 0.0], 0.15),
        Some(AutumnWoods) => ([0.45, 0.2, 0.55, 0.0, 0.0], 1.0),
        Some(Taiga) => ([0.35, 0.1, 0.04, 0.0, 0.0], 0.3),
        Some(Jungle) => ([0.4, 0.35, 0.2, 0.2, 0.0], 0.05),
        Some(TropicalCoast) => ([0.35, 0.2, 0.04, 0.22, 0.0], 0.0),
        Some(Desert) => ([0.8, 0.0, 0.0, 0.0, 0.0], 0.0),
        Some(Alpine) => ([0.3, 0.35, 0.0, 0.15, 0.0], 0.0),
        Some(Tundra) => ([0.25, 0.05, 0.0, 0.0, 0.0], 0.0),
        Some(Swamp) => ([0.4, 0.3, 0.15, 0.0, 0.0], 0.4),
        Some(Volcanic) => ([0.5, 0.0, 0.0, 0.0, 0.7], 0.0),
        Some(HauntedWoods) => ([0.35, 0.05, 0.35, 0.0, 0.0], 0.85),
        None => ([0.45, 0.3, 0.04, 0.05, 0.0], 0.1),
    }
}

/// Is this tree one whose leaves fall (not a conifer, a palm or bamboo)?
fn broadleaf(kind: AssetKind) -> bool {
    matches!(
        kind,
        AssetKind::Oak | AssetKind::Birch | AssetKind::Maple | AssetKind::Willow
    )
}

/// What the place asks for: shares per kind and (autumn, white petals, sandy dust), from the
/// reading, the hour (`night`) and the weather.
pub fn wanted(
    r: &AirReading,
    night: f32,
    w: &crate::world::WeatherParams,
    biome_mix: &[(Option<BiomePreset>, f32)],
) -> ([f32; 5], [f32; 3]) {
    let mut a = [0.0f32; 5];
    let mut autumn = 0.0;
    let mut sandy = 0.0;
    let mut white = 0.35;
    for (b, wgt) in biome_mix {
        let (v, au) = biome_air(*b);
        for k in 0..5 {
            a[k] += v[k] * wgt;
        }
        autumn += au * wgt;
        if *b == Some(BiomePreset::Desert) {
            sandy += wgt;
        }
        if *b == Some(BiomePreset::TropicalCoast) || *b == Some(BiomePreset::Alpine) {
            white += 0.5 * wgt;
        }
    }
    // the trees, flowers and lava right here
    let trees = (r.broadleaf_trees as f32 / 4.0).min(1.0);
    a[2] = (a[2] + trees * 0.7).min(1.0);
    if r.broadleaf_trees > 0 {
        autumn = autumn.max(r.maples as f32 / r.broadleaf_trees as f32);
    }
    a[3] = (a[3] + (r.flowers as f32 / 8.0).min(1.0) * 0.5).min(1.0);
    if let Some(d) = r.lava {
        a[4] = a[4].max((1.0 - smoothstep(60.0, 380.0, d)) * 0.9);
    }
    if r.infernal {
        a[4] = a[4].max(0.8);
    }
    // the hour and the weather: dust, pollen and petals by day only, washed out of the air by
    // rain or snow; fewer leaves by night or in rain; ash falls in anything but a downpour
    let day = 1.0 - night;
    let dry = (1.0 - w.rain) * (1.0 - w.snow);
    a[0] = a[0] * day * dry * (1.0 - 0.5 * w.dim) + w.dust * 0.9;
    a[1] *= day * dry;
    a[2] *= (0.3 + 0.7 * day) * (1.0 - 0.6 * w.rain) * (1.0 - w.snow);
    a[3] *= day * dry;
    a[4] *= 1.0 - 0.7 * w.rain;
    // summer leaves fall far fewer than autumn's
    a[2] *= 0.45 + 0.55 * autumn;
    sandy = sandy.max(w.dust);
    (
        a.map(|v| v.clamp(0.0, 1.0)),
        [
            autumn.clamp(0.0, 1.0),
            white.clamp(0.0, 1.0),
            sandy.clamp(0.0, 1.0),
        ],
    )
}

/// The biomes round `p` (the spot and four points 30 m out) with their weights.
fn biome_mix(terrain: &crate::terrain::Terrain, p: Vec3) -> Vec<(Option<BiomePreset>, f32)> {
    let mut out: Vec<(Option<BiomePreset>, f32)> = vec![];
    for (o, w) in [
        (Vec2::ZERO, 0.4),
        (Vec2::new(30.0, 0.0), 0.15),
        (Vec2::new(-30.0, 0.0), 0.15),
        (Vec2::new(0.0, 30.0), 0.15),
        (Vec2::new(0.0, -30.0), 0.15),
    ] {
        let b = BiomePreset::from_id(terrain.biome_at(p.x + o.x, p.z + o.y));
        match out.iter_mut().find(|e| e.0 == b) {
            Some(e) => e.1 += w,
            None => out.push((b, w)),
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn read_place(
    p: Vec3,
    terrain: &crate::terrain::Terrain,
    near: Option<&crate::nature::render::Near>,
    store: &crate::objects::ObjectStore,
    lib: Option<&crate::objects::Library>,
    water: &crate::water::WaterBodies,
    zones: &crate::world::Zones,
    sky: &crate::sky::SkyState,
) -> AirReading {
    let mut r = AirReading {
        biome: BiomePreset::from_id(terrain.biome_at(p.x, p.z)),
        ..default()
    };
    if let Some(near) = near {
        let mut found = vec![];
        near.query(p.xz(), 24.0, &mut found);
        for i in &found {
            if broadleaf(i.kind) {
                r.broadleaf_trees += 1;
                r.maples += (i.kind == AssetKind::Maple) as usize;
            }
            if matches!(i.kind, AssetKind::Flowers | AssetKind::FlowerBush)
                && i.pos.xz().distance(p.xz()) < 14.0
            {
                r.flowers += 1;
            }
        }
    }
    if let Some(lib) = lib {
        for id in store.query(p.xz(), 24.0) {
            let Some(e) = store.items.get(&id).and_then(|o| lib.get(&o.asset)) else {
                continue;
            };
            if e.category == crate::objects::Category::Trees && e.kind.is_none_or(broadleaf) {
                r.broadleaf_trees += 1;
                r.maples += (e.kind == Some(AssetKind::Maple)) as usize;
            }
        }
    }
    use crate::water::WaterBehaviour;
    let mut lava = f32::MAX;
    for l in water
        .lakes
        .iter()
        .filter(|l| l.style.behaviour == WaterBehaviour::Molten)
    {
        for q in l.outline.iter().step_by(4) {
            lava = lava.min(Vec2::from_array(*q).distance(p.xz()));
        }
    }
    for rv in water
        .rivers
        .iter()
        .filter(|r| r.style.behaviour == WaterBehaviour::Molten)
    {
        for q in &rv.points {
            lava = lava.min(Vec2::new(q[0], q[2]).distance(p.xz()));
        }
    }
    r.lava = (lava < 400.0).then_some(lava);
    r.infernal = zones
        .by_id(sky.viewer_zone)
        .is_some_and(|z| z.mood_enabled && z.mood == crate::world::Mood::Infernal);
    r
}

/// Why the layer can't show here (None: it can), and how much it shows by the camera's height.
fn visibility(
    walk: &crate::walk::WalkState,
    motion: &crate::wind::Motion,
    cam: Vec3,
    terrain: &crate::terrain::Terrain,
    water: &crate::water::WaterBodies,
    settings: &AirSettings,
    dungeon: bool,
) -> (f32, Option<&'static str>) {
    if !settings.particles {
        return (0.0, Some("turned off"));
    }
    if !motion.live {
        return (
            0.0,
            Some("editing: the world holds still (walk, or turn on Preview motion)"),
        );
    }
    if dungeon {
        return (0.0, Some("inside a dungeon"));
    }
    let ground = terrain.height_at(cam.x, cam.z);
    if cam.y < terrain.sea_level - 0.2
        || water
            .water_level(Vec3::new(cam.x, ground, cam.z))
            .is_some_and(|l| cam.y < l - 0.2)
    {
        return (0.0, Some("underwater"));
    }
    let above = cam.y - ground;
    let v = if walk.active {
        1.0
    } else {
        1.0 - smoothstep(18.0, 45.0, above)
    };
    if v <= 0.0 {
        return (
            0.0,
            Some("the camera is too high above the ground (closer than ~45 m shows it)"),
        );
    }
    (v, None)
}

/// What the air life reads of the world.
#[derive(bevy::ecs::system::SystemParam)]
pub struct AirWorld<'w> {
    walk: Res<'w, crate::walk::WalkState>,
    motion: Res<'w, crate::wind::Motion>,
    terrain: Res<'w, crate::terrain::Terrain>,
    store: Res<'w, crate::objects::ObjectStore>,
    lib: Option<Res<'w, crate::objects::Library>>,
    near: Option<Res<'w, crate::nature::render::Near>>,
    water: Res<'w, crate::water::WaterBodies>,
    zones: Res<'w, crate::world::Zones>,
    sky: Res<'w, crate::sky::SkyState>,
    lighting: Res<'w, crate::sky::Lighting>,
    graphics: Res<'w, crate::streaming::Graphics>,
    wind: Res<'w, crate::wind::WindState>,
    indoors: Option<Res<'w, crate::motion::indoors::Indoors>>,
    dungeon: Res<'w, crate::dungeons::model::DungeonMap>,
    time: Res<'w, Time>,
    fog: Option<Res<'w, crate::volumetrics::FroxelFog>>,
}

pub fn update_air(
    w: AirWorld,
    cam: Query<(&GlobalTransform, Option<&DistanceFog>), With<crate::camera::MainCamera>>,
    mut field_tf: Query<&mut Transform, With<AirField>>,
    field: Query<&AirField>,
    mut state: ResMut<AirState>,
    mut mats: ResMut<Assets<AirMaterial>>,
) {
    let AirWorld {
        walk,
        motion,
        terrain,
        store,
        lib,
        near,
        water,
        zones,
        sky,
        lighting,
        graphics,
        wind,
        indoors,
        dungeon,
        time,
        fog,
    } = w;
    let Ok(f) = field.single() else { return };
    let Ok((cam, haze)) = cam.single() else {
        return;
    };
    let eye = cam.translation();
    if let Ok(mut tf) = field_tf.single_mut() {
        tf.translation = eye;
    }
    let dt = time.delta_secs().min(0.1);
    let st = &mut *state;
    if st.last_eye.is_none_or(|l| l.distance(eye) > 80.0) {
        st.snap = true;
        st.timer = 0.0;
    }
    st.last_eye = Some(eye);
    let settings = &lighting.air;
    let inside = indoors
        .as_ref()
        .map(|i| i.amount.max(i.cave))
        .unwrap_or(0.0);
    let (vis, why) = visibility(
        &walk,
        &motion,
        eye,
        &terrain,
        &water,
        settings,
        dungeon.interior(),
    );
    st.hidden_because = why;
    // what the place asks for (twice a second)
    st.timer -= dt;
    if st.timer <= 0.0 {
        st.timer = 0.5;
        let p = if walk.active { walk.position } else { eye };
        st.reading = read_place(
            p,
            &terrain,
            near.as_deref(),
            &store,
            lib.as_deref(),
            &water,
            &zones,
            &sky,
        );
        let (mut a, look) = wanted(
            &st.reading,
            sky.night,
            &sky.weather,
            &biome_mix(&terrain, p),
        );
        // indoors (or in a cave) only the dust in the light from the windows
        for v in a.iter_mut().skip(1) {
            *v *= 1.0 - inside;
        }
        a[0] *= 1.0 - 0.4 * inside;
        st.target = a;
        st.target_look = look;
        if std::mem::take(&mut st.snap) {
            st.amounts = a;
            (st.autumn, st.white, st.sandy) = (look[0], look[1], look[2]);
        }
    }
    // ease towards it over a few seconds (the layer itself over about one)
    let k = 1.0 - (-dt * 0.5).exp();
    for i in 0..5 {
        st.amounts[i] += (st.target[i] - st.amounts[i]) * k;
    }
    st.autumn += (st.target_look[0] - st.autumn) * k;
    st.white += (st.target_look[1] - st.white) * k;
    st.sandy += (st.target_look[2] - st.sandy) * k;
    let kv = 1.0 - (-dt * 2.0).exp();
    st.visible += (vis - st.visible) * kv;
    if vis == 0.0 && st.visible < 0.002 {
        st.visible = 0.0;
    }
    // the wind carries each kind (integrated, wrapped to its box)
    let live_dt = if motion.live { dt } else { 0.0 };
    st.clock = (st.clock + live_dt) % 7200.0;
    let wv = wind.velocity(time.elapsed_secs());
    for (i, kind) in AirKind::ALL.iter().enumerate() {
        let g = kind.group();
        let o = &mut st.offsets[i];
        *o += Vec3::new(wv.x * g.carry, -g.fall, wv.z * g.carry) * live_dt;
        *o = Vec3::new(
            o.x.rem_euclid(g.width),
            o.y.rem_euclid(g.height),
            o.z.rem_euclid(g.width),
        );
    }
    let detail = graphics.detail.clamp(0.5, 1.6);
    let density = settings.density.clamp(0.0, 3.0);
    for i in 0..5 {
        st.shown[i] = (st.amounts[i] * density * BASE * (0.55 + 0.45 * detail)).clamp(0.0, 1.0);
    }
    if st.visible <= 0.0 {
        // (nothing to draw: leave the material alone so it isn't re-sent every frame)
        if let Some(m) = mats.get(&f.0)
            && m.extension.params.gust.w == 0.0
        {
            return;
        }
    }
    let Some(mut m) = mats.get_mut(&f.0) else {
        return;
    };
    let p = &mut m.extension.params;
    for (i, kind) in AirKind::ALL.iter().enumerate() {
        let g = kind.group();
        p.kinds[i] = Vec4::new(st.shown[i], g.size, g.width, g.height);
        p.offsets[i] = st.offsets[i].extend(0.0);
    }
    let lin = |c: [f32; 3]| Vec3::from_array(c);
    // dust: warm motes, sandier in deserts and sandstorms; pollen: warm white; ash: dark grey
    p.tints[0] = lin([1.0, 0.94, 0.84])
        .lerp(lin([1.0, 0.8, 0.55]), st.sandy)
        .extend(0.0);
    p.tints[1] = lin([1.0, 0.95, 0.8]).extend(0.0);
    p.tints[2] = Vec3::splat(0.3).extend(st.autumn);
    p.tints[3] = lin([0.92, 0.5, 0.62]).extend(st.white);
    let ember = if st.reading.lava.is_some_and(|d| d < 200.0) || st.reading.infernal {
        0.28
    } else {
        0.08
    };
    p.tints[4] = lin([0.09, 0.085, 0.08]).extend(ember);
    p.sky = sky_fill(&sky, haze).extend(1.0);
    p.wind = Vec4::new(wind.dir.x, wind.dir.y, wind.speed, st.clock);
    p.gust = Vec4::new(
        wind.gust_offset.x,
        wind.gust_offset.y,
        wind.gustiness,
        st.visible,
    );
    // the volumetric light (only re-bound when it changes)
    let (volume, params) = froxel(fog.as_deref());
    p.fog = params;
    if m.extension.fog_volume != volume {
        m.extension.fog_volume = volume;
    }
}

/// The sky's fill light for things in the air (before exposure): the shade of a clear day, dimmer
/// under heavy cloud, nothing at night, in the colour of the air's haze (the camera's distance fog:
/// blue on a clear day, warm at sunset, grey in fog).
pub fn sky_fill(sky: &crate::sky::SkyState, haze: Option<&DistanceFog>) -> Vec3 {
    let day = smoothstep(-0.08, 0.3, sky.sun_dir.y);
    let fill = 9000.0 * day * (1.0 - 0.55 * sky.weather.dim);
    let tint = haze
        .map(|f| f.color.to_linear())
        .map(|c| Vec3::new(c.red, c.green, c.blue))
        .filter(|c| c.max_element() > 1e-3)
        .map(|c| c / c.max_element() * 0.9)
        .unwrap_or(Vec3::new(0.62, 0.72, 0.95));
    tint * fill
}

/// The froxel fog's volume and parameters while it runs (None and zero while it is off, so the
/// material falls back to an empty volume).
pub fn froxel(fog: Option<&crate::volumetrics::FroxelFog>) -> (Option<Handle<Image>>, Vec4) {
    match fog {
        Some(f) if f.params.w > 0.5 => (Some(f.image.clone()), f.params),
        _ => (None, Vec4::ZERO),
    }
}

pub struct AirPlugin;

impl Plugin for AirPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<AirMaterial>::default())
            .init_resource::<AirState>()
            .add_systems(Startup, spawn_air)
            .add_systems(Update, update_air.after(crate::sky::SkySystems));
        app.mcp_tool(McpTool {
            name: "air_particles",
            description: "Air life round the explorer: fine dust everywhere by day (it only shows where a sunbeam catches it, sparkling when you look towards the sun), pollen and seed fluff over meadows and grassland, leaves falling and tumbling under broadleaf trees and through woods (autumn colours in autumn woods and under maples), petals off flowers, ash and embers near lava and in Infernal zones. The terrain's biomes, the trees and flowers nearby, the hour (nothing heavy at night) and the weather (none in rain or snow) decide the amounts; the wind carries everything. Seen close to the ground while the world moves (walking, camera paths, Preview motion). \
`enabled` turns it on or off, `density` multiplies the amounts (0..3, default 1). Saved with the world. Returns the settings, whether it shows now (or why not), and per kind the share shown and how many specks, with what the place round the eye is like (biome, broadleaf trees and maples near, flowers near, lava) and the sun's bearing and elevation (look that way for the sparkle). Chimney smoke is `smoke`.",
            input_schema: || {
                json!({ "type": "object", "properties": {
                    "enabled": { "type": "boolean", "description": "drifting dust, pollen, leaves, petals and ash (default on)" },
                    "density": { "type": "number", "minimum": 0, "maximum": 3, "description": "times what the place brings (default 1)" } } })
            },
            run: air_tool,
        });
    }
}

fn air_tool(world: &mut World, args: Value) -> Result<Vec<McpContent>, String> {
    let enabled = opt_arg::<bool>(&args, "enabled")?;
    let density = opt_arg::<f32>(&args, "density")?;
    if enabled.is_some() || density.is_some() {
        let mut l = world.resource_mut::<crate::sky::Lighting>();
        if let Some(e) = enabled {
            l.air.particles = e;
        }
        if let Some(d) = density {
            l.air.density = d.clamp(0.0, 3.0);
        }
        world.resource_mut::<crate::world::WorldMeta>().dirty = true;
    }
    let s = world.resource::<crate::sky::Lighting>().air.clone();
    let st = world.resource::<AirState>();
    let r2 = |v: f32| (v as f64 * 100.0).round() / 100.0;
    let kinds: Vec<Value> = AirKind::ALL
        .iter()
        .enumerate()
        .map(|(i, k)| json!({ "kind": k.id(), "asked": r2(st.amounts[i]), "shown": r2(st.shown[i]), "specks": (st.shown[i] * k.group().count as f32).round() as u32 }))
        .collect();
    let r = &st.reading;
    let sun = world.resource::<crate::sky::SkyState>().sun_dir;
    Ok(vec![McpContent::Json(json!({
        "enabled": s.particles,
        "density": s.density,
        "showing": r2(st.visible),
        "hidden_because": st.hidden_because,
        "kinds": kinds,
        "autumn_leaves": r2(st.autumn),
        // (where to look for the sparkle: towards the sun)
        "sun_bearing_deg": crate::mcp_tools::dir_bearing(sun.xz()).round(),
        "sun_elevation_deg": sun.y.clamp(-1.0, 1.0).asin().to_degrees().round(),
        "here": {
            "biome": r.biome.map(|b| b.short()),
            "broadleaf_trees_near": r.broadleaf_trees,
            "maples_near": r.maples,
            "flowers_near": r.flowers,
            "lava_m": r.lava.map(|d| d.round()),
            "infernal": r.infernal,
        },
    }))])
}

/// The Weather tab's controls: air life and chimney smoke, the density folded away.
pub fn air_controls(ui: &mut bevy_egui::egui::Ui, s: &mut AirSettings) {
    crate::ui::widgets::toggle(ui, &mut s.particles, "Dust, pollen & falling leaves")
        .on_hover_text("Specks drifting in the air round you while walking: dust glinting in sunbeams, pollen over meadows, leaves under trees, petals by flowers, ash near lava. The place, the hour and the weather decide what.");
    if s.particles {
        ui.add(
            crate::ui::widgets::TrackSlider::new(&mut s.density, 0.0..=3.0)
                .step_by(0.05)
                .suffix("×")
                .text("Amount"),
        );
    }
    crate::ui::widgets::toggle(ui, &mut s.smoke, "Chimney smoke").on_hover_text("Smoke rising from chimneys, forges and cauldrons of the placed buildings, bending in the wind.");
    if s.smoke {
        ui.add(
            crate::ui::widgets::TrackSlider::new(&mut s.smoke_strength, 0.0..=2.0)
                .step_by(0.05)
                .suffix("×")
                .text("Thickness"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WeatherParams;

    fn clear() -> WeatherParams {
        WeatherParams {
            clouds: 0.0,
            fog: 0.0,
            wet: 0.0,
            rain: 0.0,
            snow: 0.0,
            dim: 0.0,
            dust: 0.0,
        }
    }

    #[test]
    fn places_bring_their_own_air() {
        let one = |b| vec![(Some(b), 1.0)];
        let r = AirReading::default();
        let (meadow, _) = wanted(&r, 0.0, &clear(), &one(BiomePreset::Meadow));
        assert!(
            meadow[1] > 0.5 && meadow[3] > 0.2 && meadow[0] > 0.3,
            "{meadow:?}"
        );
        let (autumn, look) = wanted(&r, 0.0, &clear(), &one(BiomePreset::AutumnWoods));
        assert!(autumn[2] > 0.4 && look[0] > 0.9, "{autumn:?} {look:?}");
        let (volcanic, _) = wanted(&r, 0.5, &clear(), &one(BiomePreset::Volcanic));
        assert!(volcanic[4] > 0.5 && volcanic[1] == 0.0);
        // trees and flowers right here
        let under = AirReading {
            broadleaf_trees: 8,
            maples: 4,
            flowers: 10,
            ..default()
        };
        let (a, look) = wanted(&under, 0.0, &clear(), &one(BiomePreset::Meadow));
        assert!(
            a[2] > 0.3 && a[3] > 0.5 && (look[0] - 0.5).abs() < 1e-4,
            "{a:?} {look:?}"
        );
    }

    #[test]
    fn nights_and_rain_clear_the_air() {
        let r = AirReading {
            broadleaf_trees: 8,
            ..default()
        };
        let mix = vec![(Some(BiomePreset::Meadow), 1.0)];
        let (night, _) = wanted(&r, 1.0, &clear(), &mix);
        assert_eq!((night[0], night[1], night[3]), (0.0, 0.0, 0.0));
        assert!(night[2] > 0.0 && night[2] < 0.2, "{night:?}");
        let rain = WeatherParams {
            rain: 1.0,
            ..clear()
        };
        let (wet, _) = wanted(&r, 0.0, &rain, &mix);
        assert_eq!((wet[0], wet[1], wet[3]), (0.0, 0.0, 0.0));
        // a sandstorm fills it with dust
        let storm = WeatherParams {
            dust: 1.0,
            ..clear()
        };
        let (sand, look) = wanted(
            &AirReading::default(),
            0.0,
            &storm,
            &[(Some(BiomePreset::Desert), 1.0)],
        );
        assert!(sand[0] > 0.9 && look[2] > 0.9);
    }

    #[test]
    fn the_field_holds_every_group_and_old_worlds_get_the_defaults() {
        let m = field_mesh();
        let total: usize = AirKind::ALL.iter().map(|k| k.group().count).sum();
        assert_eq!(m.count_vertices(), total * 4);
        assert!(total <= 5000);
        let s: AirSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AirSettings::default());
        let s: AirSettings = serde_json::from_str(r#"{"density": 2.0}"#).unwrap();
        assert!(s.particles && s.smoke && s.density == 2.0);
    }
}
