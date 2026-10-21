//! Drawing a dungeon's map: the architecture's meshes (built off the main thread whenever the
//! plan or style changes) with their materials, the cut-away editing view (the levels above the
//! one being edited and its ceilings hidden, wall tops capped), the underground look (no sky,
//! sea or terrain; the mood's fill, fog and exposure), and the light sources.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::lights::{self, DungeonLight, LightKind};
use super::mesh::{self, Built, Layer, MeshData};
use super::model::DungeonMap;
use super::plan::{Compiled, Liquid, Plan};
use super::space::Space;
use super::styles::{self, Slot, Tex};
use crate::camera::MainCamera;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

/// A dungeon surface: the standard material weathered by `shaders/dungeon_surface.wesl` (grime,
/// moss, wet, puddles) with parallax from its style's depth map.
pub type SurfaceMaterial = ExtendedMaterial<StandardMaterial, SurfaceExt>;

#[derive(Clone, Debug, ShaderType, Reflect, Default, PartialEq)]
pub struct SurfaceParams {
    /// moss, grime, wet, puddles
    pub amounts: Vec4,
    /// moss tint (linear), w: the moss texture carries its own coverage
    pub moss: Vec4,
    /// what grime multiplies the colour by (linear), w: metres per moss repeat
    pub grime: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SurfaceExt {
    #[uniform(100)]
    pub params: SurfaceParams,
    #[texture(101)]
    #[sampler(102)]
    pub moss: Handle<Image>,
}

impl MaterialExtension for SurfaceExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/dungeon_surface.wesl".into()
    }
}

/// The weathering a style's surface gets in a map (its style's, scaled by the map's).
pub fn surface_params(
    st: &styles::Style,
    slot: Slot,
    w: &super::model::Weathering,
    art: styles::Art,
) -> SurfaceParams {
    let base = st.weathering();
    // (trims and caps stay cleaner: only grime)
    let k = match slot {
        Slot::Cap => [0.0, 0.0, 0.0, 0.0],
        Slot::Trim | Slot::Beam => [0.2, 0.7, 0.4, 0.0],
        Slot::Ceiling => [0.35, 0.5, 0.3, 0.0],
        _ => [1.0, 1.0, 1.0, 1.0],
    };
    let a = Vec4::new(
        base[0] * w.moss * k[0],
        base[1] * w.grime * k[1],
        base[2] * w.wet * k[2],
        base[3] * w.puddles * k[3],
    );
    let coverage = styles::moss_texture(art).1;
    SurfaceParams {
        amounts: a,
        // (greener than it looks in daylight: it has to read as moss under orange torchlight)
        moss: Vec4::new(0.78, 1.18, 0.55, if coverage { 1.0 } else { 0.0 }),
        grime: Vec4::new(0.55, 0.5, 0.45, 2.2),
    }
}

/// Height of the hidden ground over an underground map (everything happens far below it).
pub const ROOF: f32 = 400.0;

/// A built mesh of the architecture.
#[derive(Component)]
pub struct DungeonPart {
    pub level: i32,
    pub layer: Layer,
}

#[derive(Component)]
pub struct Backdrop;

/// What the editor shows: the level being edited, cut away above it, lights, the plan.
#[derive(Resource)]
pub struct DungeonView {
    pub level: i32,
    /// hide the levels above and this level's ceilings
    pub cut: bool,
    pub show_lights: bool,
    /// a readable light while building (off: see it lit as in the game)
    pub work_light: bool,
}

impl Default for DungeonView {
    fn default() -> Self {
        Self {
            level: 0,
            cut: true,
            show_lights: true,
            work_light: true,
        }
    }
}

/// How the sky systems should light an underground map (`sky::update_sun_moon` reads it).
#[derive(Resource, Default, Clone)]
pub struct InteriorLook {
    pub active: bool,
    pub ev100: f32,
    pub ambient: Color,
    pub ambient_brightness: f32,
    pub fog: Color,
    pub fog_density: f32,
    /// open-ceilinged rooms show the sky: the sun or moon shines (shadows cast by the walls)
    pub sky: bool,
}

#[derive(Resource, Default)]
pub struct DungeonWorld {
    /// fingerprint of the plan and styles wanted and drawn
    pub want: u64,
    pub built: u64,
    task: Option<(u64, Task<Built>)>,
    pub compiled: Option<Arc<Compiled>>,
    parts: Vec<Entity>,
    mats: HashMap<(u16, Slot, bool, bool), Handle<SurfaceMaterial>>,
    /// the weathering the materials were made with
    weathered: Option<(super::model::Weathering, styles::Art)>,
    liquid_mats: HashMap<Liquid, Handle<StandardMaterial>>,
    /// textures to give mip chains once loaded (not under `models/`, which get theirs anyway)
    mip_wanted: Vec<Handle<Image>>,
    mip_tasks: Vec<(AssetId<Image>, Task<Image>)>,
    mip_done: std::collections::HashSet<AssetId<Image>>,
    pub doorways: Vec<mesh::Doorway>,
    pub triangles: usize,
    /// ms the last build took (logs, MCP)
    pub build_ms: f32,
    backdrop: Option<Entity>,
    /// what the world's own sea and stars are set to show (an underground map, its sky)
    shown: Option<(bool, super::model::SkyMode)>,
    /// the eye's exposure as it adapts (EV100)
    ev_now: Option<f32>,
    /// the lights casting shadows now (the nearest few)
    pub shadowed: std::collections::HashSet<u32>,
    /// the fog volume of the volumetric haze
    fog_volume: Option<Entity>,
    /// the underground look was applied (terrain etc. hidden)
    hid_world: bool,
    lights: HashMap<u32, Entity>,
    light_eval: f32,
}

impl DungeonWorld {
    pub fn busy(&self) -> bool {
        self.task.is_some() || self.want != self.built
    }
}

fn fingerprint(map: &DungeonMap) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Some(m) = &map.0 {
        serde_json::to_string(&m.plan)
            .unwrap_or_default()
            .hash(&mut h);
        m.style.hash(&mut h);
        (m.kind as u8).hash(&mut h);
        // (under a sky the walls are seen from outside too: double-sided)
        (m.sky != super::model::SkyMode::None).hash(&mut h);
        // (rubble is geometry; the rest of the weathering is the materials')
        m.weathering.rubble.to_bits().hash(&mut h);
    } else {
        0u8.hash(&mut h);
    }
    h.finish()
}

/// Each room's style index.
pub fn style_index(plan: &Plan, default: &str) -> HashMap<u32, usize> {
    let d = styles::index(default);
    plan.rooms
        .iter()
        .map(|r| (r.id, r.style.as_deref().map(styles::index).unwrap_or(d)))
        .collect()
}

/// Compiles a map's plan (pillars by style).
pub fn compile(plan: &Plan, default_style: &str) -> Compiled {
    let idx = style_index(plan, default_style);
    Compiled::new(plan, &|r| {
        let st = &styles::all()[idx.get(&r.id).copied().unwrap_or(0)];
        let r_scale = if r.kind == super::plan::RoomKind::Hall {
            1.15
        } else {
            1.0
        };
        (
            st.pillar_r * r_scale,
            st.pillar_shape == styles::PillarShape::Square,
        )
    })
}

/// The plan changed: compile it now (the physics follows at once) and want new meshes.
pub(super) fn track(
    map: Res<DungeonMap>,
    mut world: ResMut<DungeonWorld>,
    mut caves: ResMut<crate::caves::render::CaveWorld>,
    view: Res<DungeonView>,
    walk: Res<crate::walk::WalkState>,
    pieces: Res<super::pieces::PieceState>,
) {
    if map.is_changed() {
        let fp = fingerprint(&map);
        if fp != world.want || world.compiled.is_none() != map.0.is_none() {
            world.want = fp;
            world.compiled = map
                .0
                .as_ref()
                .filter(|m| !m.plan.rooms.is_empty())
                .map(|m| Arc::new(compile(&m.plan, &m.style)));
        }
    }
    // the space the walker, camera and cursor see (with the editing cut)
    let interior = map.interior();
    let style = map
        .0
        .as_ref()
        .map(|m| styles::get(&m.style).id)
        .unwrap_or("");
    let blockers = map
        .0
        .as_ref()
        .map(|m| super::pieces::blockers(&m.pieces, &pieces.open))
        .unwrap_or_default();
    let want = world.compiled.clone().map(|plan| {
        let cut = (interior && view.cut && !walk.active).then(|| cut_height(&map, view.level));
        Space {
            plan,
            cut: cut.flatten(),
            interior,
            style,
            blockers: Arc::new(blockers.clone()),
        }
    });
    let changed = match (&caves.dungeon, &want) {
        (Some(a), Some(b)) => {
            !Arc::ptr_eq(&a.plan, &b.plan)
                || a.cut != b.cut
                || a.interior != b.interior
                || a.style != b.style
                || *a.blockers != *b.blockers
        }
        (None, None) => false,
        _ => true,
    };
    if changed {
        caves.dungeon = want;
    }
}

/// Where the editing view cuts: just under the floor of the level above the edited one.
pub fn cut_height(map: &DungeonMap, level: i32) -> Option<f32> {
    let m = map.0.as_ref()?;
    let y = m.plan.level_y(level);
    let above = m
        .plan
        .levels
        .iter()
        .filter(|l| l.y > y + 0.5)
        .map(|l| l.y)
        .fold(f32::MAX, f32::min);
    Some(if above < f32::MAX {
        above - 0.6
    } else {
        y + 60.0
    })
}

pub(super) fn start(map: Res<DungeonMap>, mut world: ResMut<DungeonWorld>) {
    if world.task.is_some() || world.want == world.built {
        return;
    }
    let want = world.want;
    let Some(c) = world.compiled.clone() else {
        // nothing to draw
        world.built = want;
        world.task = Some((
            want,
            AsyncComputeTaskPool::get().spawn(async { Built::default() }),
        ));
        return;
    };
    let idx = map
        .0
        .as_ref()
        .map(|m| style_index(&m.plan, &m.style))
        .unwrap_or_default();
    let rubble = map.0.as_ref().map(|m| m.weathering.rubble).unwrap_or(1.0);
    world.task = Some((
        want,
        AsyncComputeTaskPool::get().spawn(async move {
            let t0 = std::time::Instant::now();
            let b = mesh::build_with(&c, &|r| idx.get(&r.id).copied().unwrap_or(0), rubble);
            info!(
                "dungeon built: {} triangles in {:.0} ms",
                b.triangles(),
                t0.elapsed().as_secs_f32() * 1000.0
            );
            b
        }),
    ));
}

fn to_mesh(d: MeshData) -> Mesh {
    let mut m = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let n = d.pos.len();
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, d.pos);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, d.nrm);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, d.uv);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, d.col);
    // the weathering's floor height and wetness (`mesh::weather`)
    if d.aux.len() == n {
        m.insert_attribute(Mesh::ATTRIBUTE_UV_1, d.aux);
    }
    m.insert_indices(Indices::U32(d.idx));
    let _ = m.generate_tangents();
    m
}

fn sampler(
    linear_data: bool,
) -> impl Fn(&mut bevy::image::ImageLoaderSettings) + Send + Sync + 'static {
    move |s: &mut bevy::image::ImageLoaderSettings| {
        s.is_srgb = !linear_data;
        s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            anisotropy_clamp: 16,
            ..default()
        });
    }
}

impl DungeonWorld {
    #[allow(clippy::too_many_arguments)]
    fn material(
        &mut self,
        si: usize,
        slot: Slot,
        double: bool,
        parallax: bool,
        weathering: &super::model::Weathering,
        art: styles::Art,
        mats: &mut Assets<SurfaceMaterial>,
        server: &AssetServer,
    ) -> Handle<SurfaceMaterial> {
        if let Some(h) = self.mats.get(&(si as u16, slot, double, parallax)) {
            return h.clone();
        }
        let st = &styles::all()[si.min(styles::all().len() - 1)];
        let m = st.mat(slot);
        let (albedo, normal, orm) = m.tex.paths();
        let load = |p: &str, linear: bool| -> Handle<Image> {
            server
                .load_builder()
                .with_settings(sampler(linear))
                .load(p.to_string())
        };
        let a = load(&albedo, false);
        let n = normal.as_ref().map(|p| load(p, true));
        let o = orm.as_ref().map(|p| load(p, true));
        if !matches!(m.tex, Tex::Hp(_)) {
            self.mip_wanted.push(a.clone());
            self.mip_wanted.extend(n.clone());
            self.mip_wanted.extend(o.clone());
        }
        let tint = Color::srgb(m.tint[0], m.tint[1], m.tint[2]);
        // parallax on the big surfaces (walls, floors, rock, pillars) when the detail allows
        let deep = parallax
            && matches!(
                slot,
                Slot::Wall
                    | Slot::WallLower
                    | Slot::Floor
                    | Slot::Rock
                    | Slot::RockFloor
                    | Slot::Pillar
            );
        let depth_map = deep.then(|| load(&m.tex.depth_path(), true));
        let mat = if slot == Slot::Cap {
            StandardMaterial {
                base_color: Color::srgb(0.07, 0.065, 0.06),
                perceptual_roughness: 1.0,
                ..default()
            }
        } else {
            StandardMaterial {
                base_color: tint,
                base_color_texture: Some(a),
                normal_map_texture: n,
                metallic_roughness_texture: o.clone(),
                occlusion_texture: o.clone(),
                perceptual_roughness: if o.is_some() { 1.0 } else { m.rough },
                metallic: m.metal,
                // (under a sky walls and roofs are seen from outside as well)
                double_sided: double,
                cull_mode: if double {
                    None
                } else {
                    Some(bevy::render::render_resource::Face::Back)
                },
                parallax_depth_scale: if matches!(m.tex, Tex::Hp(_)) {
                    0.012
                } else {
                    0.018
                },
                max_parallax_layer_count: 12.0,
                depth_map,
                ..default()
            }
        };
        let moss = server
            .load_builder()
            .with_settings(sampler(false))
            .load(styles::moss_texture(art).0.to_string());
        let h = mats.add(SurfaceMaterial {
            base: mat,
            extension: SurfaceExt {
                params: surface_params(st, slot, weathering, art),
                moss,
            },
        });
        self.mats
            .insert((si as u16, slot, double, parallax), h.clone());
        h
    }

    fn liquid_material(
        &mut self,
        l: Liquid,
        mats: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        if let Some(h) = self.liquid_mats.get(&l) {
            return h.clone();
        }
        let (base, emissive, rough, alpha) = match l {
            Liquid::Water | Liquid::None => (
                Color::srgba(0.02, 0.05, 0.06, 0.86),
                LinearRgba::BLACK,
                0.04,
                true,
            ),
            Liquid::Lava => (
                Color::srgb(0.2, 0.04, 0.01),
                LinearRgba::rgb(14.0, 3.2, 0.5),
                0.8,
                false,
            ),
            Liquid::Slime => (
                Color::srgba(0.12, 0.25, 0.04, 0.92),
                LinearRgba::rgb(0.25, 0.7, 0.08),
                0.15,
                true,
            ),
            Liquid::Blood => (
                Color::srgba(0.18, 0.01, 0.01, 0.95),
                LinearRgba::BLACK,
                0.08,
                true,
            ),
            Liquid::Arcane => (
                Color::srgba(0.1, 0.12, 0.3, 0.85),
                LinearRgba::rgb(0.8, 1.4, 4.5),
                0.1,
                true,
            ),
        };
        let h = mats.add(StandardMaterial {
            base_color: base,
            emissive,
            perceptual_roughness: rough,
            alpha_mode: if alpha {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            },
            ..default()
        });
        self.liquid_mats.insert(l, h.clone());
        h
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    mut commands: Commands,
    mut world: ResMut<DungeonWorld>,
    map: Res<DungeonMap>,
    graphics: Res<crate::streaming::Graphics>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut surfaces: ResMut<Assets<SurfaceMaterial>>,
    server: Res<AssetServer>,
) {
    let Some((want, task)) = world.task.as_mut() else {
        return;
    };
    let want = *want;
    let Some(built) = block_on(future::poll_once(task)) else {
        return;
    };
    world.task = None;
    for e in std::mem::take(&mut world.parts) {
        commands.entity(e).despawn();
    }
    world.triangles = built.triangles();
    world.doorways = built.doorways.clone();
    let mut keys: Vec<_> = built.parts.keys().copied().collect();
    keys.sort();
    let mut parts = built.parts;
    for k in keys {
        let Some(d) = parts.remove(&k) else { continue };
        if d.is_empty() {
            continue;
        }
        // (ceilings too: shown in the builder — its Ceilings button — they're seen from above)
        let double = k.layer == Layer::Ceiling
            || map
                .0
                .as_ref()
                .is_some_and(|m| m.sky != super::model::SkyMode::None);
        let (weathering, art) = map
            .0
            .as_ref()
            .map(|m| (m.weathering, m.art))
            .unwrap_or_default();
        let parallax = graphics.detail >= 1.0;
        let mat = world.material(
            k.style as usize,
            k.slot,
            double,
            parallax,
            &weathering,
            art,
            &mut surfaces,
            &server,
        );
        let mut e = commands.spawn((
            Name::new(format!("Dungeon {:?} {:?} L{}", k.slot, k.layer, k.level)),
            Mesh3d(meshes.add(to_mesh(d))),
            MeshMaterial3d(mat),
            Transform::IDENTITY,
            DungeonPart {
                level: k.level,
                layer: k.layer,
            },
        ));
        if k.layer == Layer::Cap {
            e.insert((NotShadowCaster, NotShadowReceiver));
        }
        world.parts.push(e.id());
    }
    for l in built.liquids {
        if l.mesh.is_empty() {
            continue;
        }
        let mat = world.liquid_material(l.liquid, &mut mats);
        let e = commands
            .spawn((
                Name::new(format!("Dungeon {:?}", l.liquid)),
                Mesh3d(meshes.add(to_mesh(l.mesh))),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
                NotShadowCaster,
                DungeonPart {
                    level: l.level,
                    layer: Layer::Main,
                },
            ))
            .id();
        world.parts.push(e);
    }
    world.built = want;
}

/// The map's weathering changed (Look › Weathering): the surfaces' materials follow at once (the
/// geometry stays).
pub(super) fn weathering(
    mut world: ResMut<DungeonWorld>,
    map: Res<DungeonMap>,
    mut surfaces: ResMut<Assets<SurfaceMaterial>>,
) {
    let Some(m) = map.0.as_ref() else { return };
    let now = (m.weathering, m.art);
    if world.weathered == Some(now) {
        return;
    }
    world.weathered = Some(now);
    let all = styles::all();
    for ((si, slot, _, _), h) in world.mats.iter() {
        if let Some(mut mat) = surfaces.get_mut(h) {
            let p = surface_params(
                &all[(*si as usize).min(all.len() - 1)],
                *slot,
                &m.weathering,
                m.art,
            );
            if mat.extension.params != p {
                mat.extension.params = p;
            }
        }
    }
}

/// Mip chains for the photoscanned textures (Bevy loads them with one level: they'd shimmer).
pub(super) fn mips(mut world: ResMut<DungeonWorld>, mut images: ResMut<Assets<Image>>) {
    let wanted = std::mem::take(&mut world.mip_wanted);
    let mut keep = Vec::new();
    for h in wanted {
        let id = h.id();
        if world.mip_done.contains(&id) {
            continue;
        }
        match images.get(id) {
            Some(img) if img.data.is_some() && img.texture_descriptor.mip_level_count == 1 => {
                let img = img.clone();
                world.mip_done.insert(id);
                world.mip_tasks.push((
                    id,
                    AsyncComputeTaskPool::get()
                        .spawn(async move { crate::model_render::mips::with_mips(img) }),
                ));
            }
            Some(_) => {
                world.mip_done.insert(id);
            }
            None => keep.push(h),
        }
    }
    world.mip_wanted = keep;
    let mut i = 0;
    while i < world.mip_tasks.len() {
        if let Some(img) = block_on(future::poll_once(&mut world.mip_tasks[i].1)) {
            let (id, _) = world.mip_tasks.swap_remove(i);
            if let Some(mut slot) = images.get_mut(id) {
                *slot = img;
            }
        } else {
            i += 1;
        }
    }
}

/// The cut-away editing view: levels above the edited one hidden, its ceilings hidden, wall
/// tops capped. Walking: everything as built.
pub(super) fn view(
    view: Res<DungeonView>,
    walk: Res<crate::walk::WalkState>,
    map: Res<DungeonMap>,
    mut parts: Query<(&DungeonPart, &mut Visibility)>,
) {
    let cut = view.cut && !walk.active && map.interior();
    for (p, mut v) in &mut parts {
        let show = if !cut {
            p.layer != Layer::Cap
        } else if p.level > view.level {
            false
        } else if p.level == view.level {
            p.layer != Layer::Ceiling
        } else {
            p.layer != Layer::Cap
        };
        let want = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
}

/// The underground look: no terrain, sea or sky (a dark backdrop instead), and the mood's light,
/// fog and exposure for the sky systems.
#[allow(clippy::too_many_arguments)]
pub(super) fn interior_env(
    mut commands: Commands,
    map: Res<DungeonMap>,
    view: Res<DungeonView>,
    walk: Res<crate::walk::WalkState>,
    mut world: ResMut<DungeonWorld>,
    mut look: ResMut<InteriorLook>,
    mut lighting: ResMut<crate::sky::Lighting>,
    mut patches: Query<
        &mut Visibility,
        (With<crate::terrain::mesh::TerrainPatch>, Without<Backdrop>),
    >,
    mut named: Query<
        (&Name, &mut Visibility),
        (
            Without<crate::terrain::mesh::TerrainPatch>,
            Without<Backdrop>,
        ),
    >,
    mut backdrop: Query<(&mut Transform, &MeshMaterial3d<StandardMaterial>), With<Backdrop>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    time: Res<Time<Real>>,
) {
    let interior = map.interior();
    if interior {
        for mut v in &mut patches {
            if *v != Visibility::Hidden {
                *v = Visibility::Hidden;
            }
        }
    }
    let sky = map
        .0
        .as_ref()
        .filter(|_| interior)
        .map(|m| m.sky)
        .unwrap_or_default();
    let shown = (interior, sky);
    if Some(shown) != world.shown {
        world.shown = Some(shown);
        world.hid_world = interior;
        let hide = if interior {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        // (stars over a night sky)
        let stars = if interior && sky != super::model::SkyMode::Night {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        for (n, mut v) in &mut named {
            match n.as_str() {
                "Ocean" | "Seabed apron" => *v = hide,
                "Stars" => *v = stars,
                _ => {}
            }
        }
        if !interior {
            for mut v in &mut patches {
                *v = Visibility::Inherited;
            }
        }
        // a sky: its hour (the map keeps it)
        if interior && sky != super::model::SkyMode::None {
            if (lighting.hour - sky.hour()).abs() > 0.01 {
                lighting.hour = sky.hour();
            }
            lighting.stars = sky == super::model::SkyMode::Night;
        }
    }
    let Some(m) = map.0.as_ref().filter(|_| interior) else {
        if let Some(e) = world.backdrop.take() {
            commands.entity(e).despawn();
        }
        look.active = false;
        world.ev_now = None;
        return;
    };
    let mood = &m.mood;
    let fog = Color::srgb(mood.fog[0], mood.fog[1], mood.fog[2]);
    let open_sky = m.sky != super::model::SkyMode::None;
    // the backdrop: a dark sphere round the camera where the sky would be (none under a sky)
    if open_sky {
        if let Some(e) = world.backdrop.take() {
            commands.entity(e).despawn();
        }
    } else if world.backdrop.is_none() {
        let e = commands
            .spawn((
                Name::new("Dungeon backdrop"),
                Backdrop,
                Mesh3d(meshes.add(Sphere::new(3000.0).mesh().uv(32, 16))),
                MeshMaterial3d(mats.add(StandardMaterial {
                    base_color: Color::BLACK,
                    unlit: true,
                    cull_mode: None,
                    ..default()
                })),
                Transform::IDENTITY,
                NotShadowCaster,
                NotShadowReceiver,
                bevy::camera::visibility::NoFrustumCulling,
            ))
            .id();
        world.backdrop = Some(e);
    }
    let eye = cam.single().map(|c| c.translation()).ok();
    if let (Some(c), Ok((mut tf, mat))) = (eye, backdrop.single_mut()) {
        tf.translation = c;
        if let Some(mut mm) = mats.get_mut(&mat.0) {
            let lin = fog.to_linear();
            let want = Color::linear_rgb(lin.red * 0.25, lin.green * 0.25, lin.blue * 0.25);
            if mm.base_color != want {
                mm.base_color = want;
            }
        }
    }
    let fill = Color::srgb(mood.fill[0], mood.fill[1], mood.fill[2]);
    // (a dark scene's fill, for an eye opened as in a cave, `sky::CAVE_EV`)
    let mut ambient_brightness = 40.0 + mood.fill_strength.clamp(0.0, 1.5).powf(1.6) * 2600.0;
    let mut ambient = fill;
    let mut fog_density = 0.003 + mood.fog_density.clamp(0.0, 1.0).powi(2) * 0.045;
    // building: a readable light everywhere (the mood shows when walking, or with it off)
    if view.work_light && !walk.active {
        ambient_brightness = ambient_brightness.max(2400.0);
        ambient = fill.mix(&Color::srgb(0.85, 0.83, 0.8), 0.5);
        fog_density *= 0.25;
    }
    // the eye: opened to the dark inside, it adapts over a second or two to the sky's light out
    // in an open-ceilinged room (and back)
    let inside_ev = 10.0 - mood.exposure;
    // (building under a day sky: the cut-away lets the sun into every room, so the eye is the sky's)
    let day = m.sky == super::model::SkyMode::Day;
    let under_sky = open_sky
        && ((day && !walk.active) || (walk.active && eye.is_some_and(|e| under_open_sky(m, e))));
    let target = if under_sky {
        m.sky.ev100() - mood.exposure * 0.5
    } else {
        inside_ev
    };
    let dt = time.delta_secs().min(0.1);
    let ev = match world.ev_now {
        Some(ev) => ev + (target - ev) * (1.0 - (-dt * 1.6).exp()),
        None => target,
    };
    world.ev_now = Some(ev);
    // (the fill is set for the inside's exposure: brighter in step with a brighter eye)
    let ambient_brightness = ambient_brightness * 2f32.powf((ev - inside_ev).max(0.0));
    *look = InteriorLook {
        active: true,
        ev100: ev,
        ambient,
        ambient_brightness,
        fog,
        fog_density,
        sky: open_sky,
    };
}

/// The haze as real volumetric fog (walking in a hazy mood, at normal detail and up): a fog
/// volume over the dungeon, lit by the shadow-casting lights that light the haze (light shafts,
/// beams under spots, the glow round braziers). Lower detail keeps the cheap billboards
/// (`fx::haze`).
#[allow(clippy::too_many_arguments)]
pub(super) fn volumetric(
    mut commands: Commands,
    map: Res<DungeonMap>,
    walk: Res<crate::walk::WalkState>,
    graphics: Res<crate::streaming::Graphics>,
    mut world: ResMut<DungeonWorld>,
    cam: Query<(Entity, Option<&bevy::light::VolumetricFog>), With<MainCamera>>,
    lights_q: Query<(Entity, &DungeonLight, Has<bevy::light::VolumetricLight>)>,
    mut vols: Query<(&mut Transform, &mut bevy::light::FogVolume)>,
) {
    let m = map
        .0
        .as_ref()
        .filter(|m| m.kind == super::model::MapKind::Interior);
    let on = m.is_some_and(|m| m.mood.haze > 0.05)
        && walk.active
        && graphics.detail >= 1.0
        && world.compiled.is_some();
    let Ok((cam_e, has_fog)) = cam.single() else {
        return;
    };
    if !on {
        if has_fog.is_some() {
            commands
                .entity(cam_e)
                .remove::<bevy::light::VolumetricFog>();
        }
        if let Some(e) = world.fog_volume.take() {
            commands.entity(e).despawn();
        }
        for (e, _, has) in &lights_q {
            if has {
                commands.entity(e).remove::<bevy::light::VolumetricLight>();
            }
        }
        return;
    }
    let (Some(m), Some(c)) = (m, world.compiled.clone()) else {
        return;
    };
    if has_fog.is_none() {
        commands.entity(cam_e).insert(bevy::light::VolumetricFog {
            ambient_intensity: 0.0,
            step_count: 48,
            jitter: 0.0,
            ..default()
        });
    }
    let fog = Color::srgb(m.mood.fog[0], m.mood.fog[1], m.mood.fog[2]).mix(&Color::WHITE, 0.5);
    let density = 0.015 + m.mood.haze.clamp(0.0, 1.0) * 0.07;
    let (lo, hi) = (c.lo, c.hi);
    let tf =
        Transform::from_translation((lo + hi) * 0.5).with_scale((hi - lo).max(Vec3::splat(1.0)));
    match world.fog_volume.and_then(|e| vols.get_mut(e).ok()) {
        Some((mut t, mut v)) => {
            if *t != tf {
                *t = tf;
            }
            if (v.density_factor - density).abs() > 1e-4 || v.fog_color != fog {
                v.density_factor = density;
                v.fog_color = fog;
            }
        }
        None => {
            let e = commands
                .spawn((
                    Name::new("Dungeon haze volume"),
                    bevy::light::FogVolume {
                        fog_color: fog,
                        density_factor: density,
                        absorption: 0.2,
                        scattering: 0.4,
                        scattering_asymmetry: 0.6,
                        ..default()
                    },
                    tf,
                ))
                .id();
            world.fog_volume = Some(e);
        }
    }
    // the lights that light the haze (and cast shadows: shafts need them)
    let hazy: std::collections::HashSet<u32> = m
        .lights
        .iter()
        .filter(|l| l.haze && world.shadowed.contains(&l.id))
        .map(|l| l.id)
        .collect();
    for (e, dl, has) in &lights_q {
        let want = hazy.contains(&dl.id);
        if want && !has {
            commands.entity(e).insert(bevy::light::VolumetricLight);
        } else if !want && has {
            commands.entity(e).remove::<bevy::light::VolumetricLight>();
        }
    }
}

/// Is `p` in an open-ceilinged room (under the sky, when the map has one)?
fn under_open_sky(m: &super::model::MapDoc, p: Vec3) -> bool {
    let q = Vec2::new(p.x, p.z);
    m.plan.rooms.iter().any(|r| {
        let base = m.plan.level_y(r.level) + r.floor;
        r.kind.is_space()
            && r.ceiling == super::plan::Ceiling::Open
            && p.y > base - 0.5
            && p.y < base + r.height + 3.0
            && r.shape.sdf(q) > 0.0
    })
}

/// The light sources: the nearest `lights::BUDGET` shine (the nearest few with shadows); fires
/// flicker; the mood tints them.
#[allow(clippy::too_many_arguments)]
pub(super) fn sync_lights(
    mut commands: Commands,
    map: Res<DungeonMap>,
    mut world: ResMut<DungeonWorld>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut points: Query<(&DungeonLight, &mut PointLight, &mut Transform), Without<SpotLight>>,
    mut spots: Query<(&DungeonLight, &mut SpotLight, &mut Transform), Without<PointLight>>,
    graphics: Res<crate::streaming::Graphics>,
    time: Res<Time>,
    real: Res<Time<Real>>,
) {
    let Some(m) = map.0.as_ref() else {
        for (_, e) in world.lights.drain() {
            commands.entity(e).despawn();
        }
        return;
    };
    let eye = cam.single().map(|c| c.translation()).unwrap_or(Vec3::ZERO);
    let now = real.elapsed_secs();
    // which lights shine: every quarter second, or at once when the set changed
    let changed = map.is_changed();
    if changed || now - world.light_eval > 0.25 {
        world.light_eval = now;
        let mut order: Vec<(f32, &lights::LightDef)> = m
            .lights
            .iter()
            .map(|l| (l.position().distance_squared(eye), l))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let keep: std::collections::HashSet<u32> = order
            .iter()
            .take(lights::BUDGET)
            .map(|(_, l)| l.id)
            .collect();
        let stale: Vec<u32> = world
            .lights
            .keys()
            .filter(|id| !keep.contains(id))
            .copied()
            .collect();
        for id in stale {
            if let Some(e) = world.lights.remove(&id) {
                commands.entity(e).despawn();
            }
        }
        // the nearest few cast shadows (hazy shafts and spots first: their beams need them)
        let budget = lights::shadow_budget(graphics.detail);
        let mut cand: Vec<&lights::LightDef> = order
            .iter()
            .take(lights::BUDGET)
            .map(|(_, l)| *l)
            .filter(|l| l.shadows)
            .collect();
        cand.sort_by_key(|l| !(l.haze && l.cone > 0.0));
        world.shadowed = cand.iter().take(budget).map(|l| l.id).collect();
        for (_, l) in order.iter().take(lights::BUDGET) {
            let shadows = world.shadowed.contains(&l.id);
            let spot = l.kind.is_spot() || l.cone > 0.0;
            let respawn = match world.lights.get(&l.id) {
                Some(e) => {
                    // (a spot became a glow or back: make it again)
                    let is_spot = spots.get(*e).is_ok();
                    is_spot != spot
                }
                None => true,
            };
            if respawn {
                if let Some(e) = world.lights.remove(&l.id) {
                    commands.entity(e).despawn();
                }
                let tf = Transform::from_translation(l.position()).looking_to(
                    l.direction(),
                    if l.direction().y.abs() > 0.98 {
                        Vec3::Z
                    } else {
                        Vec3::Y
                    },
                );
                let e = if spot {
                    commands
                        .spawn((
                            Name::new(format!("Dungeon light {}", l.id)),
                            DungeonLight { id: l.id },
                            SpotLight {
                                range: l.range,
                                shadow_maps_enabled: shadows,
                                ..default()
                            },
                            tf,
                        ))
                        .id()
                } else {
                    commands
                        .spawn((
                            Name::new(format!("Dungeon light {}", l.id)),
                            DungeonLight { id: l.id },
                            PointLight {
                                range: l.range,
                                radius: 0.08,
                                shadow_maps_enabled: shadows,
                                ..default()
                            },
                            tf,
                        ))
                        .id()
                };
                world.lights.insert(l.id, e);
            }
        }
    }
    // every frame: colour, strength, flicker, position
    let t = time.elapsed_secs();
    let by_id: HashMap<u32, &lights::LightDef> = m.lights.iter().map(|l| (l.id, l)).collect();
    let tint = |c: [f32; 3]| {
        let c = m.mood.tinted(c);
        Color::srgb(c[0], c[1], c[2])
    };
    for (dl, mut pl, mut tf) in &mut points {
        let Some(l) = by_id.get(&dl.id) else { continue };
        let shadows = world.shadowed.contains(&dl.id);
        if pl.shadow_maps_enabled != shadows {
            pl.shadow_maps_enabled = shadows;
        }
        let (lumens, range, flick) = m.all_lights.apply(l);
        pl.color = tint(l.color);
        pl.intensity = lumens * lights::flicker(t, l.id, flick);
        pl.range = range;
        tf.translation = l.position();
    }
    for (dl, mut sl, mut tf) in &mut spots {
        let Some(l) = by_id.get(&dl.id) else { continue };
        let shadows = world.shadowed.contains(&dl.id);
        if sl.shadow_maps_enabled != shadows {
            sl.shadow_maps_enabled = shadows;
        }
        let (lumens, range, flick) = m.all_lights.apply(l);
        sl.color = tint(l.color);
        sl.intensity = lumens * lights::flicker(t, l.id, flick);
        sl.range = range;
        let cone = if l.cone > 0.0 {
            l.cone
        } else {
            l.kind.defaults().4.max(30.0)
        };
        sl.outer_angle = (cone * 0.5).to_radians().min(1.5);
        sl.inner_angle = sl.outer_angle * 0.6;
        *tf = Transform::from_translation(l.position()).looking_to(
            l.direction(),
            if l.direction().y.abs() > 0.98 {
                Vec3::Z
            } else {
                Vec3::Y
            },
        );
    }
    let _ = LightKind::Torch;
}
