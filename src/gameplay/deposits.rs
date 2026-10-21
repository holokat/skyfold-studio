//! Ore deposits in the world. Every mining node with an ore stands as a photoscanned rock (the
//! CC0 boulder and rock-cluster scans of `crate::photoscan`, with their textures and levels of
//! detail) carrying that ore: `assets/shaders/ore.wesl` draws veins and nuggets, coal seams,
//! granite, obsidian glass, crystal facets or glowing runes into the scan in the rock's own
//! space, from the ore's `Look`. Veins, nuggets, halos and stains are filled with photoscanned
//! metal and mineral surfaces (`scans`: native copper under verdigris, rust crusts, coal, quartz,
//! gold…), mapped in the rock's space at their real size. Deposits grow a little with the tier;
//! the epic and legendary crystal ores also grow clusters of crystal shards out of the rock (the
//! library's crystal meshes, glowing from within).
//!
//! Each deposit's rock, size, turn and tilt follow from its position and ore, so the same world
//! always looks the same. They are plain meshes sharing one material per ore and scan (Bevy
//! batches them), rebuilt only when a deposit changes (moved deposits and sculpted ground just
//! update their transforms). `DepositSet` lists them as placed rocks for walk-mode collision
//! (`motion::collide`) and the engine exports.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use bevy::camera::visibility::{RenderLayers, VisibilityRange};
use bevy::light::NotShadowCaster;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::{ShaderDefVal, ShaderRef};

use super::Gameplay;
use super::ores::{Fill, Ore, Style};
use crate::noise::hash2;
use crate::objects::{FoliageMaterial, Library, Placed, builtin_id};
use crate::procgen::meshes::AssetKind;
use crate::terrain::Terrain;

pub mod scans;

pub type OreMaterial = ExtendedMaterial<StandardMaterial, OreExt>;

/// A photoscanned fill for the shader.
#[derive(Clone, Copy, Debug, ShaderType, Reflect)]
pub struct FillUniform {
    /// the scan's mean colour (linear), its texture-array layer (-1: a flat colour)
    pub mean: Vec4,
    /// its mean roughness and metalness, how far it's recoloured to the ore, metres per repeat
    pub pbr: Vec4,
}

impl Default for FillUniform {
    fn default() -> Self {
        Self {
            mean: Vec4::new(0.5, 0.5, 0.5, -1.0),
            pbr: Vec4::new(0.5, 0.0, 1.0, 1.0),
        }
    }
}

/// One ore's look for the shader (colours linear).
#[derive(Clone, Copy, Debug, Default, ShaderType, Reflect)]
pub struct OreUniform {
    /// ore colour, metallic
    pub ore: Vec4,
    /// halo / second colour, the ore's roughness
    pub halo: Vec4,
    /// glow colour × cd/m², how much of the scan's colour the host rock keeps
    pub glow: Vec4,
    /// host rock tint, vein coverage
    pub host: Vec4,
    /// nuggets, style, effect bits, 1 = crystal shard (0.25: an icon, more ore on it)
    pub shape: Vec4,
    /// what fills the veins and nuggets
    pub fill: FillUniform,
    /// what fills the halo and stains (QUARTZ: the veins)
    pub halo_fill: FillUniform,
}

/// The material: the ore's look and the photoscanned surfaces (`scans`; without them the fills
/// fall back to flat colours).
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct OreExt {
    #[uniform(100)]
    pub look: OreUniform,
    #[texture(101, dimension = "2d_array")]
    #[sampler(104)]
    pub scan_albedo: Option<Handle<Image>>,
    #[texture(102, dimension = "2d_array")]
    pub scan_normal: Option<Handle<Image>>,
    #[texture(103, dimension = "2d_array")]
    pub scan_pbr: Option<Handle<Image>>,
}

impl MaterialExtension for OreExt {
    fn fragment_shader() -> ShaderRef {
        "shaders/ore.wesl".into()
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // the fragment shader works in the rock's own space: it needs the instance's transform
        let def = ShaderDefVal::Bool("VERTEX_OUTPUT_INSTANCE_INDEX".into(), true);
        descriptor.vertex.shader_defs.push(def.clone());
        if let Some(f) = descriptor.fragment.as_mut() {
            f.shader_defs.push(def);
        }
        Ok(())
    }
}

fn lin(c: [u8; 3]) -> Vec3 {
    let l = Color::srgb_u8(c[0], c[1], c[2]).to_linear();
    Vec3::new(l.red, l.green, l.blue)
}

/// A fill for the shader (`stats`: the surfaces' averages, by layer; none loaded: a flat colour).
fn fill(f: Fill, stats: &[scans::LayerStats]) -> FillUniform {
    let Some((layer, st)) = f.surf.layer().and_then(|i| Some((i, stats.get(i)?))) else {
        return FillUniform::default();
    };
    FillUniform {
        mean: st.albedo.extend(layer as f32),
        pbr: Vec4::new(st.rough, st.metal, f.recolor, f.surf.tile()),
    }
}

/// The shader's parameters for an ore (`shard`: for its crystal shards, which take no fills).
pub fn uniform(ore: Ore, shard: bool, stats: &[scans::LayerStats]) -> OreUniform {
    let l = ore.look();
    let fills = if shard { &[][..] } else { stats };
    OreUniform {
        ore: lin(l.ore).extend(l.metallic),
        halo: lin(l.halo).extend(l.roughness),
        glow: (lin(l.glow) * l.glow_nits).extend(l.host_sat),
        host: lin(l.host).extend(l.veins),
        shape: Vec4::new(
            l.nuggets,
            l.style as u8 as f32,
            l.fx as f32,
            shard as u8 as f32,
        ),
        fill: fill(l.fill, fills),
        halo_fill: fill(l.halo_fill, fills),
    }
}

/// The scans deposits are made of: the boulders and the rock clusters.
const ROCKS: [AssetKind; 2] = [AssetKind::Boulder, AssetKind::RockCluster];
const BOULDER: usize = 0;
const CLUSTER: usize = 1;

pub struct RockVar {
    pub lods: Vec<Handle<Mesh>>,
    pub height: f32,
    pub width: f32,
    /// Spots on the rock's upper surface where shards can grow: (position, normal), rock space.
    anchors: Vec<(Vec3, Vec3)>,
}

pub struct RockScan {
    /// library asset id (colliders and exports use the library's own entry)
    pub asset: String,
    pub vars: Vec<RockVar>,
}

/// Meshes and materials of every deposit (built once the library is in).
#[derive(Resource)]
pub struct OreAssets {
    pub rocks: Vec<RockScan>,
    /// [rock scan][ore]
    mats: Vec<Vec<Handle<OreMaterial>>>,
    /// per ore (crystal ores only)
    shard_mats: Vec<Option<Handle<OreMaterial>>>,
    /// per ore: the boulder with more ore on it, for icons (thumbnails, the turntable)
    preview_mats: Vec<Handle<OreMaterial>>,
    /// crystal clusters: LODs, height, lowest point
    shards: Vec<(Vec<Handle<Mesh>>, f32, f32)>,
    /// the surface style (`art_style::surfaces`) the ore surfaces are in
    art: crate::art_style::ArtStyle,
    bc: bool,
}

/// Up to four spread-out spots near the top of a rock mesh that face up.
fn anchors_of(m: &Mesh, height: f32, min_y: f32) -> Vec<(Vec3, Vec3)> {
    use bevy::mesh::VertexAttributeValues as V;
    let (Ok(Some(V::Float32x3(p))), Ok(Some(V::Float32x3(n)))) = (
        m.try_attribute_option(Mesh::ATTRIBUTE_POSITION),
        m.try_attribute_option(Mesh::ATTRIBUTE_NORMAL),
    ) else {
        return vec![(Vec3::new(0.0, min_y + height * 0.85, 0.0), Vec3::Y)];
    };
    let cand: Vec<(Vec3, Vec3)> = p
        .iter()
        .zip(n)
        .map(|(a, b)| (Vec3::from_array(*a), Vec3::from_array(*b)))
        .filter(|(a, b)| b.y > 0.5 && a.y > min_y + height * 0.5)
        .collect();
    let Some(top) = cand.iter().max_by(|a, b| a.0.y.total_cmp(&b.0.y)).copied() else {
        return vec![(Vec3::new(0.0, min_y + height * 0.85, 0.0), Vec3::Y)];
    };
    // farthest-point picks, favouring high spots
    let mut out = vec![top];
    while out.len() < 4 {
        let next = cand.iter().max_by(|a, b| {
            let score = |c: &(Vec3, Vec3)| {
                out.iter()
                    .map(|o| o.0.xz().distance(c.0.xz()))
                    .fold(f32::MAX, f32::min)
                    + 0.6 * (c.0.y - min_y)
            };
            score(a).total_cmp(&score(b))
        });
        match next {
            Some(c) if !out.contains(c) => out.push(*c),
            _ => break,
        }
    }
    out
}

/// Builds the materials (the scans' own textures under the ore shader) and finds where shards
/// can grow, while the scans' vertices are still on the CPU.
fn build_assets(
    mut commands: Commands,
    lib: Option<Res<Library>>,
    foliage: Res<Assets<FoliageMaterial>>,
    meshes: Res<Assets<Mesh>>,
    mut mats: ResMut<Assets<OreMaterial>>,
    mut images: ResMut<Assets<Image>>,
    formats: Option<Res<bevy::image::CompressedImageFormatSupport>>,
) {
    let Some(lib) = lib else { return };
    let bc = formats.is_some_and(|f| f.0.contains(bevy::image::CompressedImageFormats::BC));
    let art = crate::art_style::ArtStyle::Realistic;
    let surfaces = scans::load_style(&mut images, bc, art)
        .map_err(|e| warn!("ore surfaces: {e} (flat colours instead)"))
        .ok();
    let stats = surfaces
        .as_ref()
        .map(|s| s.stats.as_slice())
        .unwrap_or_default();
    let ext = |look: OreUniform| OreExt {
        look,
        scan_albedo: surfaces.as_ref().map(|s| s.albedo.clone()),
        scan_normal: surfaces.as_ref().map(|s| s.normal.clone()),
        scan_pbr: surfaces.as_ref().map(|s| s.pbr.clone()),
    };
    let mut rocks = vec![];
    let mut rock_mats = vec![];
    for kind in ROCKS {
        let id = builtin_id(kind);
        let Some(tree) = lib.get(&id).and_then(|e| e.tree.as_ref()) else {
            warn!("ore deposits: no {id} scan in the library");
            return;
        };
        let base = foliage
            .get(&tree.bark)
            .map(|m| m.base.clone())
            .unwrap_or_default();
        let vars = tree
            .variants
            .iter()
            .map(|v| {
                let lods: Vec<Handle<Mesh>> = v.lods.iter().map(|l| l.0.clone()).collect();
                let probe = lods.get(1).or(lods.first()).and_then(|h| meshes.get(h));
                let anchors = probe
                    .map(|m| anchors_of(m, v.height, v.min_y))
                    .unwrap_or_default();
                RockVar {
                    lods,
                    height: v.height,
                    width: v.width.max(0.1),
                    anchors,
                }
            })
            .collect();
        rocks.push(RockScan { asset: id, vars });
        rock_mats.push(
            Ore::ALL
                .iter()
                .map(|o| {
                    mats.add(OreMaterial {
                        base: StandardMaterial {
                            base_color: Color::WHITE,
                            base_color_texture: base.base_color_texture.clone(),
                            normal_map_texture: base.normal_map_texture.clone(),
                            perceptual_roughness: base.perceptual_roughness,
                            reflectance: 0.35,
                            ..default()
                        },
                        extension: ext(uniform(*o, false, stats)),
                    })
                })
                .collect(),
        );
    }
    let preview_mats = match lib
        .get(&builtin_id(ROCKS[BOULDER]))
        .and_then(|e| e.tree.as_ref())
        .and_then(|t| foliage.get(&t.bark))
    {
        Some(m) => Ore::ALL
            .iter()
            .map(|o| {
                let mut look = uniform(*o, false, stats);
                look.host.w = (look.host.w + 0.35).min(1.0);
                look.shape.x = (look.shape.x * 1.4).min(1.0);
                look.shape.w = 0.25;
                mats.add(OreMaterial {
                    base: StandardMaterial {
                        base_color: Color::WHITE,
                        base_color_texture: m.base.base_color_texture.clone(),
                        normal_map_texture: m.base.normal_map_texture.clone(),
                        perceptual_roughness: m.base.perceptual_roughness,
                        reflectance: 0.35,
                        ..default()
                    },
                    extension: ext(look),
                })
            })
            .collect(),
        None => vec![],
    };
    let shard_mats = Ore::ALL
        .iter()
        .map(|o| {
            o.look().shards.then(|| {
                mats.add(OreMaterial {
                    base: StandardMaterial {
                        base_color: Color::WHITE,
                        perceptual_roughness: 0.06,
                        reflectance: 0.6,
                        ..default()
                    },
                    extension: ext(uniform(*o, true, stats)),
                })
            })
        })
        .collect();
    let shards = lib
        .get(&builtin_id(AssetKind::Crystals))
        .and_then(|e| e.tree.as_ref())
        .map(|t| {
            t.variants
                .iter()
                .map(|v| {
                    (
                        v.lods.iter().map(|l| l.0.clone()).collect(),
                        v.height.max(0.1),
                        v.min_y,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let scans: Vec<String> = rocks
        .iter()
        .map(|r: &RockScan| format!("{} {}", r.vars.len(), r.asset))
        .collect();
    info!(
        "ore deposits: {} ores on {} scans",
        Ore::ALL.len(),
        scans.join(" + ")
    );
    commands.insert_resource(OreAssets {
        rocks,
        mats: rock_mats,
        shard_mats,
        preview_mats,
        shards,
        art,
        bc,
    });
}

/// The world's art style changed: the ore surfaces load in the new style (painted veins,
/// nuggets and stains in a hand-painted world) and every deposit material takes them, with the
/// fills' mean colours of the new surfaces. (The host boulders stay the rock scans.)
fn restyle(
    assets: Option<ResMut<OreAssets>>,
    meta: Res<crate::world::WorldMeta>,
    mut mats: ResMut<Assets<OreMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut a) = assets else { return };
    let art = crate::art_style::surfaces(meta.art_style);
    if art == a.art {
        return;
    }
    a.art = art;
    let Ok(s) = scans::load_style(&mut images, a.bc, art)
        .map_err(|e| warn!("ore surfaces ({}): {e}", art.name()))
    else {
        return;
    };
    let mut set = |h: &Handle<OreMaterial>, ore: Option<Ore>| {
        if let Some(mut m) = mats.get_mut(h) {
            m.extension.scan_albedo = Some(s.albedo.clone());
            m.extension.scan_normal = Some(s.normal.clone());
            m.extension.scan_pbr = Some(s.pbr.clone());
            if let Some(o) = ore {
                m.extension.look.fill = fill(o.look().fill, &s.stats);
                m.extension.look.halo_fill = fill(o.look().halo_fill, &s.stats);
            }
        }
    };
    for per_rock in &a.mats {
        for (h, o) in per_rock.iter().zip(Ore::ALL) {
            set(h, Some(o));
        }
    }
    for (h, o) in a.preview_mats.iter().zip(Ore::ALL) {
        set(h, Some(o));
    }
    // (shards take no fills)
    for h in a.shard_mats.iter().flatten() {
        set(h, None);
    }
}

// ------------------------------------------------------------------ layout

/// Where and how one deposit stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Deposit {
    pub node: u32,
    pub ore: Ore,
    pub rock: usize,
    pub var: usize,
    pub tf: Transform,
    /// crystal shards: (cluster variant, transform in the rock's space)
    pub shards: Vec<(usize, Transform)>,
}

impl Deposit {
    /// The rock as a placed library object (colliders, exports).
    pub fn placed(&self, a: &OreAssets) -> Placed {
        Placed {
            group: 0,
            id: DEPOSIT_IDS | self.node as u64,
            asset: a.rocks[self.rock].asset.clone(),
            variant: self.var as u32,
            pos: self.tf.translation.to_array(),
            rot: self.tf.rotation.to_array(),
            scale: self.tf.scale.x,
            stretch: None,
            locked: false,
        }
    }
}

/// Placed-object ids of deposits (apart from the object store's and the procedural nature's).
pub const DEPOSIT_IDS: u64 = 0xD0E0_0000_0000_0000;

fn unit(h: u32) -> f32 {
    (h & 0xffff) as f32 / 65536.0
}

/// How a deposit of `ore` at `xz` stands: its rock (from its position, so it never changes while
/// the deposit stays put), a size that grows with the tier, a random turn, a tilt half way to the
/// slope, sunk in a little (more on slopes) so it never floats.
pub fn layout(a: &OreAssets, t: &Terrain, node: u32, ore: Ore, xz: Vec2) -> Option<Deposit> {
    let h = hash2(
        xz.x.to_bits() as i32,
        xz.y.to_bits() as i32,
        0x0de9 ^ ore as u32,
    );
    let rock = match ore.look().style {
        Style::Seam | Style::Glass => (h >> 3) as usize % 2,
        Style::Crystal | Style::Plain => BOULDER,
        _ if unit(h >> 5) < 0.25 => CLUSTER,
        _ => BOULDER,
    };
    let var = (h >> 9) as usize % a.rocks.get(rock)?.vars.len().max(1);
    layout_on(a, t, node, ore, xz, h, rock, var)
}

/// `layout` on a given rock scan and variant (`h`: the deposit's hash).
#[allow(clippy::too_many_arguments)]
fn layout_on(
    a: &OreAssets,
    t: &Terrain,
    node: u32,
    ore: Ore,
    xz: Vec2,
    h: u32,
    rock: usize,
    var: usize,
) -> Option<Deposit> {
    let v = a.rocks.get(rock)?.vars.get(var)?;
    // about 1.15 m tall (flat scans: wide) for common ores, 1.55 m for legendary ones
    let tier = ore.tier() as f32;
    let size = (1.15 + 0.1 * (tier - 1.0)) * (0.85 + 0.3 * unit(h.rotate_left(7)));
    let scale = size / (v.width * 0.55).max(v.height);
    let width = v.width * scale;
    let n = t.normal_at(xz.x, xz.y);
    let tan = ((1.0 - n.y * n.y).max(0.0).sqrt() / n.y.max(0.2)).min(1.5);
    let sink = 0.08 * v.height * scale + tan * width * 0.3;
    let yaw = unit(h.rotate_left(13)) * std::f32::consts::TAU;
    let tilt = Quat::from_rotation_arc(Vec3::Y, n.lerp(Vec3::Y, 0.5).normalize());
    let tf = Transform {
        translation: Vec3::new(xz.x, t.height_at(xz.x, xz.y) - sink, xz.y),
        rotation: tilt * Quat::from_rotation_y(yaw),
        scale: Vec3::splat(scale),
    };
    let mut shards = vec![];
    if ore.look().shards && !a.shards.is_empty() && !v.anchors.is_empty() {
        let count = (2 + (h >> 17) as usize % 2 + (ore.tier() >= 5) as usize).min(v.anchors.len());
        for k in 0..count {
            let hk = hash2(k as i32, h as i32, 0x5a4d);
            let (p, an) = v.anchors[k];
            let sv = hk as usize % a.shards.len();
            let (_, sh, smin) = a.shards[sv];
            // shards of about a quarter of the deposit's width, the first the biggest
            let target = width * (0.27 - 0.05 * k as f32) * (0.8 + 0.4 * unit(hk >> 8));
            let s = target / sh / scale;
            let up = an.lerp(Vec3::Y, 0.35).normalize();
            let rot = Quat::from_rotation_arc(Vec3::Y, up)
                * Quat::from_rotation_y(unit(hk >> 3) * std::f32::consts::TAU);
            // sunk so the cluster's foot sits in the rock
            let foot = p - an * (0.04 / scale) - up * (smin * s + target * 0.12 / scale);
            shards.push((
                sv,
                Transform {
                    translation: foot,
                    rotation: rot,
                    scale: Vec3::splat(s),
                },
            ));
        }
    }
    Some(Deposit {
        node,
        ore,
        rock,
        var,
        tf,
        shards,
    })
}

/// Every deposit's layout, as rendered (also for colliders and exports).
#[derive(Resource, Default)]
pub struct DepositSet {
    pub list: Vec<Deposit>,
    a: Vec<Placed>,
}

impl DepositSet {
    /// The rocks of deposits within `r` of `c`, as placed objects (walk-mode colliders).
    pub fn near(&self, c: Vec2, r: f32) -> impl Iterator<Item = &Placed> {
        self.a
            .iter()
            .filter(move |p| Vec2::new(p.pos[0], p.pos[2]).distance(c) <= r)
    }
}

/// Every deposit of the gameplay layer laid out.
pub fn layouts(gp: &Gameplay, a: &OreAssets, t: &Terrain) -> Vec<Deposit> {
    gp.nodes
        .iter()
        .filter_map(|n| layout(a, t, n.id, n.ore()?, Vec2::new(n.pos[0], n.pos[2])))
        .collect()
}

/// The deposits' rocks as placed library objects, for the engine exports (their ore is in the
/// server data).
pub fn export_objects(world: &World) -> Vec<Placed> {
    let (Some(gp), Some(a), Some(t)) = (
        world.get_resource::<Gameplay>(),
        world.get_resource::<OreAssets>(),
        world.get_resource::<Terrain>(),
    ) else {
        return vec![];
    };
    layouts(gp, a, t).iter().map(|d| d.placed(a)).collect()
}

// ------------------------------------------------------------------ rendering

/// On a deposit's root entity.
#[derive(Component)]
pub struct DepositRoot;

/// Rocks: full detail near, the scan's lighter levels further out, gone where they'd be a few
/// pixels. Shards are small: they go earlier.
const ROCK_RANGES: [f32; 3] = [25.0, 110.0, 1500.0];
const SHARD_RANGES: [f32; 2] = [40.0, 300.0];

fn ranges(ends: &[f32], n: usize, detail: f32) -> Vec<VisibilityRange> {
    let mut out = vec![];
    let mut start = 0.0;
    for k in 0..n {
        // the last level holds out to the end
        let end = if k + 1 == n {
            ends[ends.len() - 1]
        } else {
            ends[k.min(ends.len() - 1)]
        } * detail;
        out.push(VisibilityRange {
            start_margin: start..start,
            end_margin: end..end,
            use_aabb: false,
        });
        start = end;
    }
    out
}

/// Spawns a deposit (its root at `d.tf`). `layers`: for studio pictures (all levels then show
/// the full-detail mesh, nothing casts shadows).
pub fn spawn(
    commands: &mut Commands,
    a: &OreAssets,
    d: &Deposit,
    detail: f32,
    layers: Option<&RenderLayers>,
) -> Entity {
    spawn_with(
        commands,
        a,
        d,
        detail,
        layers,
        a.mats[d.rock][d.ore as usize].clone(),
    )
}

/// `spawn` with the rock's material given.
fn spawn_with(
    commands: &mut Commands,
    a: &OreAssets,
    d: &Deposit,
    detail: f32,
    layers: Option<&RenderLayers>,
    mat: Handle<OreMaterial>,
) -> Entity {
    let v = &a.rocks[d.rock].vars[d.var];
    let shard_mat = a.shard_mats[d.ore as usize].clone();
    let mut root = commands.spawn((
        Name::new(format!("{} deposit", d.ore.name())),
        DepositRoot,
        d.tf,
        Visibility::default(),
    ));
    root.with_children(|c| {
        let lods = if layers.is_some() {
            &v.lods[..1]
        } else {
            &v.lods[..]
        };
        for (k, (m, range)) in lods
            .iter()
            .zip(ranges(&ROCK_RANGES, lods.len(), detail))
            .enumerate()
        {
            let mut e = c.spawn((Mesh3d(m.clone()), MeshMaterial3d(mat.clone())));
            match layers {
                Some(l) => {
                    e.insert((l.clone(), NotShadowCaster));
                }
                None => {
                    e.insert(range);
                    if k >= 2 {
                        e.insert(NotShadowCaster);
                    }
                }
            }
        }
        let Some(sm) = &shard_mat else { return };
        for (sv, stf) in &d.shards {
            let (meshes, ..) = &a.shards[*sv];
            let lods = if layers.is_some() {
                &meshes[..1]
            } else {
                &meshes[..meshes.len().min(2)]
            };
            c.spawn((*stf, Visibility::default())).with_children(|s| {
                for (k, (m, range)) in lods
                    .iter()
                    .zip(ranges(&SHARD_RANGES, lods.len(), detail))
                    .enumerate()
                {
                    let mut e = s.spawn((Mesh3d(m.clone()), MeshMaterial3d(sm.clone())));
                    match layers {
                        Some(l) => {
                            e.insert((l.clone(), NotShadowCaster));
                        }
                        None => {
                            e.insert(range);
                            if k >= 1 {
                                e.insert(NotShadowCaster);
                            }
                        }
                    }
                }
            });
        }
    });
    if let Some(l) = layers {
        root.insert(l.clone());
    }
    root.id()
}

/// Bump when the studio deposit changes (its pictures are cached on disk).
pub const PREVIEW_VERSION: u32 = 5;

/// A studio deposit of `ore` (thumbnails, the turntable): an upright rock at `at`, shrunk so its
/// veins read in a small picture (the ore is drawn in metres).
pub fn spawn_preview(
    commands: &mut Commands,
    a: &OreAssets,
    ore: Ore,
    at: Vec3,
    layers: &RenderLayers,
) -> Option<Entity> {
    let flat = {
        let mut t = Terrain::new(2, 1.0, -10.0);
        for h in t.heights.iter_mut() {
            *h = 0.0;
        }
        t
    };
    // the same compact boulder for every ore; the scale sits below the root, which the studio
    // frames in its own space
    let var = a.rocks[BOULDER]
        .vars
        .iter()
        .enumerate()
        .min_by(|x, y| (x.1.width / x.1.height).total_cmp(&(y.1.width / y.1.height)))
        .map(|x| x.0)?;
    let mut d = layout_on(a, &flat, 0, ore, Vec2::ZERO, 0x5eed, BOULDER, var)?;
    d.tf = Transform::from_scale(Vec3::splat(0.4));
    // icons: the ore spread wider over the rock than in the world, so it reads at a glance
    let mats = a.preview_mats.get(ore as usize)?.clone();
    let body = spawn_with(commands, a, &d, 1.0, Some(layers), mats);
    let root = commands
        .spawn((
            Transform::from_translation(at),
            Visibility::default(),
            layers.clone(),
        ))
        .add_child(body)
        .id();
    Some(root)
}

#[derive(Default)]
struct Synced {
    /// what was last laid out
    key: u64,
    /// terrain heights and graphics detail it was laid out for
    revision: u64,
    detail: f32,
    at: f32,
    /// render entities by deposit (position bits, ore), with their transform
    entities: HashMap<(u32, u32, Ore), (Entity, Deposit)>,
}

/// Keeps the deposits' entities in step with the gameplay layer: spawns new ones, removes gone
/// ones and moves the rest (sculpted ground, at most 4×/s while it changes).
#[allow(clippy::too_many_arguments)]
fn sync(
    mut commands: Commands,
    gp: Res<Gameplay>,
    terrain: Res<Terrain>,
    assets: Option<Res<OreAssets>>,
    graphics: Res<crate::streaming::Graphics>,
    mut set: ResMut<DepositSet>,
    mut tfs: Query<&mut Transform, With<DepositRoot>>,
    time: Res<Time<Real>>,
    mut st: Local<Synced>,
) {
    let Some(a) = assets else { return };
    if !gp.is_changed() && !terrain.is_changed() && !graphics.is_changed() {
        return;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in &gp.nodes {
        if let Some(o) = n.ore() {
            (n.id, n.pos[0].to_bits(), n.pos[2].to_bits(), o).hash(&mut h);
        }
    }
    graphics.detail.to_bits().hash(&mut h);
    terrain.size().to_bits().hash(&mut h);
    let key = h.finish();
    let now = time.elapsed_secs();
    let ground_moved = terrain.height_revision != st.revision;
    if key == st.key && !(ground_moved && now - st.at > 0.25) {
        return;
    }
    let detail_changed = st.detail != graphics.detail;
    st.detail = graphics.detail;
    st.key = key;
    st.revision = terrain.height_revision;
    st.at = now;
    let list = layouts(&gp, &a, &terrain);
    let mut keep: HashMap<(u32, u32, Ore), (Entity, Deposit)> = HashMap::with_capacity(list.len());
    let (mut spawned, mut moved) = (0, 0);
    for d in &list {
        let k = (
            d.tf.translation.x.to_bits(),
            d.tf.translation.z.to_bits(),
            d.ore,
        );
        match st.entities.remove(&k) {
            Some((e, old))
                if !detail_changed
                    && old.rock == d.rock
                    && old.var == d.var
                    && old.shards.len() == d.shards.len() =>
            {
                if old.tf != d.tf
                    && let Ok(mut tf) = tfs.get_mut(e)
                {
                    *tf = d.tf;
                    moved += 1;
                }
                keep.insert(k, (e, d.clone()));
            }
            old => {
                if let Some((e, _)) = old {
                    commands.entity(e).despawn();
                }
                keep.insert(
                    k,
                    (
                        spawn(&mut commands, &a, d, graphics.detail, None),
                        d.clone(),
                    ),
                );
                spawned += 1;
            }
        }
    }
    let removed = st.entities.len();
    for (_, (e, _)) in st.entities.drain() {
        commands.entity(e).despawn();
    }
    st.entities = keep;
    if spawned + removed > 0 {
        debug!(
            "ore deposits: {} ({spawned} new, {removed} gone, {moved} moved)",
            list.len()
        );
    }
    set.a = list.iter().map(|d| d.placed(&a)).collect();
    set.list = list;
}

pub(super) fn build(app: &mut App) {
    app.add_plugins(MaterialPlugin::<OreMaterial>::default())
        .init_resource::<DepositSet>()
        .add_systems(Startup, build_assets)
        .add_systems(Update, restyle)
        .add_systems(
            PostUpdate,
            sync.before(bevy::transform::TransformSystems::Propagate),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every photoscanned surface loads, all alike, into three texture arrays; their averages are
    /// plausible (coal and obsidian black, quartz and silver bright, the metals metallic, the
    /// rocks not).
    #[test]
    fn surfaces_load() {
        use super::super::ores::Surf;
        let mut images = Assets::<Image>::default();
        let s = scans::load(&mut images, false).expect("ore surfaces");
        for h in [&s.albedo, &s.normal, &s.pbr] {
            let img = images.get(h).unwrap();
            assert_eq!(
                img.texture_descriptor.size.depth_or_array_layers as usize,
                Surf::ALL.len()
            );
            assert_eq!(img.texture_descriptor.mip_level_count, 11);
        }
        let st = |x: Surf| s.stats[x.layer().unwrap()];
        let luma = |x: Surf| st(x).albedo.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        assert!(
            luma(Surf::Coal) < 0.05 && luma(Surf::Obsidian) < 0.03,
            "coal {}, obsidian {}",
            luma(Surf::Coal),
            luma(Surf::Obsidian)
        );
        assert!(luma(Surf::Quartz) > 0.35 && luma(Surf::Silver) > 0.35);
        assert!(
            st(Surf::Gold).albedo.x > st(Surf::Gold).albedo.z * 1.5,
            "gold is yellow"
        );
        assert!(
            st(Surf::Verdigris).albedo.y > st(Surf::Verdigris).albedo.x,
            "verdigris is green"
        );
        for m in [Surf::Copper, Surf::Gold, Surf::Silver, Surf::RawMetal] {
            assert!(st(m).metal > 0.6, "{m:?} metallic");
        }
        for r in [
            Surf::Rust,
            Surf::Coal,
            Surf::Quartz,
            Surf::Granite,
            Surf::Obsidian,
        ] {
            assert!(st(r).metal < 0.05, "{r:?} not metallic");
        }
        assert!(st(Surf::Obsidian).rough < 0.15 && st(Surf::Rust).rough > 0.5);
    }

    /// Every ore's shader parameters are in range, and only the crystal ores grow shards; the
    /// real-world ores are filled with photoscans, shards never are.
    #[test]
    fn uniforms() {
        let stats = scans::load(&mut Assets::<Image>::default(), false)
            .expect("ore surfaces")
            .stats;
        for o in Ore::ALL {
            let u = uniform(o, false, &stats);
            assert!(
                u.ore.w >= 0.0 && u.ore.w <= 1.0 && u.halo.w > 0.0 && u.host.w <= 1.0,
                "{o:?}"
            );
            assert!(
                u.ore.truncate().max_element() <= 1.0 && u.host.truncate().min_element() > 0.0,
                "{o:?}"
            );
            assert_eq!(
                u.glow.truncate().max_element() > 0.0,
                o.look().glow_nits > 0.0
            );
            assert!((0.0..=1.0).contains(&u.glow.w), "{o:?}: host saturation");
            assert!(
                (0.0..=1.0).contains(&u.fill.pbr.z) && u.fill.pbr.w > 0.0,
                "{o:?}: fill"
            );
            let shard = uniform(o, true, &stats);
            assert_eq!(shard.shape.w, 1.0);
            assert!(shard.fill.mean.w < 0.0 && shard.halo_fill.mean.w < 0.0);
        }
        for o in [
            Ore::Copper,
            Ore::Tin,
            Ore::Iron,
            Ore::Coal,
            Ore::Stone,
            Ore::Silver,
            Ore::Gold,
            Ore::Nickel,
            Ore::Platinum,
            Ore::Titanium,
            Ore::Obsidian,
        ] {
            assert!(
                uniform(o, false, &stats).fill.mean.w >= 0.0,
                "{o:?} has no photoscanned fill"
            );
        }
        // without the surfaces every fill is a flat colour
        assert!(uniform(Ore::Copper, false, &[]).fill.mean.w < 0.0);
        assert!(Ore::Arcanite.look().shards && !Ore::Copper.look().shards);
    }
}
