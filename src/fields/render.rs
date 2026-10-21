//! Drawing the fields: each field is tiled with its crop's 8 m patch (`plants::patch`), turned
//! to its rows, around the camera at three levels of detail. One material per field carries its
//! frame (origin and row direction) and a mask of its outline in that frame; the vertex shader
//! (`shaders/crops_v2.wesl`) seats every plant on the terrain height map, drops the ones outside the
//! outline, shrinks plants into the ground towards the crop's draw distance and bends them in
//! the wind (the same gusts as the grass and trees). Patches are made the first time a crop is
//! seen.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::asset::uuid_handle;
use bevy::camera::primitives::Aabb;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, encase,
};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

use super::plants::TILE;
use super::{Crop, Field, Fields};
use crate::camera::MainCamera;
use crate::terrain::Terrain;
use crate::terrain::material::TerrainGpu;

pub type CropMaterial = ExtendedMaterial<StandardMaterial, CropExt>;

/// Shared every frame: terrain grid, wind, camera.
#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct CropParams {
    /// x: terrain half size, y: cell size, z: vertices per side, w: sea level
    pub grid: Vec4,
    /// xy: wind direction, z: wind speed (m/s), w: time
    pub wind: Vec4,
    /// xy: gust field offset, zw: camera xz
    pub gust: Vec4,
    /// x: camera y
    pub view: Vec4,
}

pub const CROP_PARAMS: Handle<ShaderBuffer> = uuid_handle!("6c1e9a52-8d3f-4b7e-a2c4-1f9e8d7b6a35");

/// Per field: its frame and outline mask, and how far its plants are drawn.
#[derive(Clone, Debug, ShaderType, Reflect, Default)]
pub struct FieldUniform {
    /// origin x, origin z, cos, sin of the row direction
    pub frame: Vec4,
    /// mask extent in the field's frame (x along the rows, z across)
    pub mask: Vec4,
    /// x: fade start, y: fade end, z: where the far canopy has risen, w: where it starts (m)
    pub look: Vec4,
    /// metres per repeat of each terrain ground layer (4 per vec4)
    pub tiles: [Vec4; 3],
    /// x: ground photoscans loaded (0/1), y: (unused: roughness comes with the vertex colour), z:
    /// normal-map strength
    pub tex: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CropExt {
    #[storage(100, read_only)]
    pub params: Handle<ShaderBuffer>,
    #[texture(101, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
    #[texture(102)]
    #[sampler(103)]
    pub mask: Handle<Image>,
    #[uniform(104)]
    pub field: FieldUniform,
    /// the crop model's atlas: colour with a cut-out alpha, and its tangent-space normal map
    #[texture(105)]
    #[sampler(106)]
    pub atlas: Handle<Image>,
    #[texture(107)]
    pub atlas_normal: Handle<Image>,
    /// the terrain's photoscanned ground layers: colour + height, and normal XY + roughness + AO
    #[texture(108, dimension = "2d_array")]
    #[sampler(109)]
    pub ground: Handle<Image>,
    #[texture(110, dimension = "2d_array")]
    pub ground_detail: Handle<Image>,
}

impl MaterialExtension for CropExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/crops_v2.wesl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "shaders/crops_v2.wesl".into()
    }
    /// Plants are placed in the vertex shader: kept out of the depth prepass.
    fn enable_prepass() -> bool {
        false
    }
}

/// A field as drawn: its material, frame and tiles.
struct Drawn {
    crop: Crop,
    outline: Vec<[f32; 2]>,
    rows: f32,
    material: Handle<CropMaterial>,
    origin: Vec2,
    rot: Quat,
    /// tile origins in the field's frame, and their ground height range
    tiles: Vec<(Vec2, f32, f32)>,
    /// the field's world bounds (for skipping it whole)
    lo: Vec2,
    hi: Vec2,
    pools: [Vec<Entity>; 4],
    heights_of: u64,
}

#[derive(Resource, Default)]
pub struct CropRender {
    meshes: HashMap<(Crop, u8), (Handle<Mesh>, f32)>,
    drawn: HashMap<u64, Drawn>,
    revision: Option<u64>,
    /// the crop models' revision the meshes were made from
    models: Option<u64>,
    last_cam: Vec3,
    clock: f32,
    /// 1×1 stand-ins for crops without a model and before the ground photoscans load
    blank: Option<[Handle<Image>; 4]>,
}

fn pixel(rgba: [u8; 4]) -> Image {
    Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba.to_vec(),
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    )
}

pub struct CropRenderPlugin;

impl Plugin for CropRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<CropMaterial>::default())
            .init_resource::<CropRender>()
            .add_systems(Startup, |mut buffers: ResMut<Assets<ShaderBuffer>>| {
                write_params(&mut buffers, &CropParams::default())
            })
            .add_systems(
                PostUpdate,
                (sync_fields, place_tiles, update_params)
                    .chain()
                    .before(crate::terrain::TerrainSystems::Gpu),
            );
    }
}

fn write_params(buffers: &mut Assets<ShaderBuffer>, p: &CropParams) {
    let mut bytes = encase::StorageBuffer::new(Vec::<u8>::new());
    if bytes.write(p).is_ok() {
        let _ = buffers.insert(
            &CROP_PARAMS,
            ShaderBuffer::new(
                bytes.into_inner(),
                RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
            ),
        );
    }
}

/// The field's frame: origin at its outline's lowest corner in the rows' frame.
fn frame(f: &Field) -> (Vec2, Quat, Vec2) {
    let rot = Quat::from_rotation_y(f.rows);
    let inv = rot.inverse();
    let local: Vec<Vec2> = f
        .poly()
        .iter()
        .map(|p| (inv * Vec3::new(p.x, 0.0, p.y)).xz())
        .collect();
    let (lo, hi) = super::bounds(&local);
    let origin = (rot * Vec3::new(lo.x, 0.0, lo.y)).xz();
    (origin, rot, hi - lo)
}

/// The outline as a mask in the field's frame (½ m texels, at most 2048 a side), and the extent
/// the mask covers (m, from the frame's origin).
fn mask_image(f: &Field, origin: Vec2, rot: Quat, size: Vec2) -> (Image, Vec2) {
    let texel = (size.max_element() / 2048.0).max(0.5);
    let (w, h) = (
        (size.x / texel).ceil() as usize + 2,
        (size.y / texel).ceil() as usize + 2,
    );
    let poly = f.poly();
    let mut data = vec![0u8; w * h];
    for j in 0..h {
        for i in 0..w {
            let local = Vec2::new((i as f32 + 0.5) * texel, (j as f32 + 0.5) * texel);
            let world = origin + (rot * Vec3::new(local.x, 0.0, local.y)).xz();
            // a soft edge one texel wide: plants keep about half a metre inside the line
            let d = crate::setpieces::poly_sdf(&poly, world) + 0.35;
            data[j * w + i] = ((0.5 - d / texel).clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    (img, Vec2::new(w as f32, h as f32) * texel)
}

/// The tiles that touch the outline (field frame origins).
fn tiles(f: &Field, origin: Vec2, rot: Quat, size: Vec2) -> Vec<Vec2> {
    let poly = f.poly();
    let (nx, nz) = ((size.x / TILE).ceil() as i32, (size.y / TILE).ceil() as i32);
    let mut out = vec![];
    for j in 0..nz.max(1) {
        for i in 0..nx.max(1) {
            let local = Vec2::new(i as f32 * TILE, j as f32 * TILE);
            let mid =
                origin + (rot * Vec3::new(local.x + TILE * 0.5, 0.0, local.y + TILE * 0.5)).xz();
            if crate::setpieces::poly_sdf(&poly, mid) < TILE * 0.72 {
                out.push(local);
            }
        }
    }
    out
}

/// Builds or drops fields' materials and tiles when the fields change.
#[allow(clippy::too_many_arguments)]
fn sync_fields(
    mut commands: Commands,
    fields: Res<Fields>,
    mut r: ResMut<CropRender>,
    mut mats: ResMut<Assets<CropMaterial>>,
    mut images: ResMut<Assets<Image>>,
    terrain: Res<Terrain>,
    gpu: Option<Res<TerrainGpu>>,
    models: Res<super::assets::CropAssets>,
    ground: Res<crate::terrain::textures::TerrainTextures>,
) {
    let Some(gpu) = gpu else { return };
    let r = &mut *r;
    let blank = r
        .blank
        .get_or_insert_with(|| {
            [
                images.add(pixel([255, 255, 255, 255])),
                images.add(pixel([128, 128, 255, 255])),
                images.add(crate::terrain::textures::placeholder_array(true)),
                images.add(crate::terrain::textures::placeholder_array(false)),
            ]
        })
        .clone();
    // a crop model (re)loaded: every field's meshes and materials are made again
    if r.models != Some(models.revision) {
        r.models = Some(models.revision);
        r.meshes.clear();
        r.revision = None;
        for d in std::mem::take(&mut r.drawn).into_values() {
            for e in d.pools.into_iter().flatten() {
                commands.entity(e).despawn();
            }
            mats.remove(&d.material);
        }
    }
    let layer_tiles: [Vec4; 3] = std::array::from_fn(|i| {
        Vec4::new(
            ground.tiles[i * 4],
            ground.tiles[i * 4 + 1],
            ground.tiles[i * 4 + 2],
            ground.tiles[i * 4 + 3],
        )
    });
    let (ground_a, ground_d) = match (&ground.albedo, &ground.detail) {
        (Some(a), Some(d)) => (a.clone(), d.clone()),
        _ => (blank[2].clone(), blank[3].clone()),
    };
    if r.revision != Some(fields.revision) {
        r.revision = Some(fields.revision);
        // gone or changed: dropped (and made again below)
        let keep: Vec<u64> = fields
            .list
            .iter()
            .filter(|f| {
                r.drawn
                    .get(&f.id)
                    .is_some_and(|d| d.crop == f.crop && d.outline == f.outline && d.rows == f.rows)
            })
            .map(|f| f.id)
            .collect();
        let stale: Vec<u64> = r
            .drawn
            .keys()
            .filter(|id| !keep.contains(id))
            .copied()
            .collect();
        for id in stale {
            if let Some(d) = r.drawn.remove(&id) {
                for e in d.pools.into_iter().flatten() {
                    commands.entity(e).despawn();
                }
                mats.remove(&d.material);
            }
        }
        let new: Vec<&Field> = fields
            .list
            .iter()
            .filter(|f| !r.drawn.contains_key(&f.id))
            .collect();
        for f in new {
            let (origin, rot, size) = frame(f);
            let (mask, extent) = mask_image(f, origin, rot, size);
            let mask = images.add(mask);
            let ranges = f.crop.ranges();
            let material = mats.add(CropMaterial {
                base: StandardMaterial {
                    base_color: Color::WHITE,
                    perceptual_roughness: 0.72,
                    reflectance: 0.3,
                    diffuse_transmission: 0.25,
                    cull_mode: None,
                    double_sided: true,
                    ..default()
                },
                extension: CropExt {
                    params: CROP_PARAMS,
                    height: gpu.height.clone(),
                    mask,
                    field: FieldUniform {
                        frame: Vec4::new(origin.x, origin.y, f.rows.cos(), f.rows.sin()),
                        mask: Vec4::new(extent.x, extent.y, 0.0, 0.0),
                        // plants fade out over the last fifth of their range; the canopy rises
                        // under them from where they start to get small (`Crop::canopy_from`)
                        look: Vec4::new(
                            ranges[2] * 0.82,
                            ranges[2],
                            f.crop.canopy_from(ranges) * 1.6,
                            f.crop.canopy_from(ranges),
                        ),
                        tiles: layer_tiles,
                        tex: Vec4::new(
                            if ground.albedo.is_some() { 1.0 } else { 0.0 },
                            models.get(f.crop).map_or(0.7, |m| m.roughness),
                            1.0,
                            0.0,
                        ),
                    },
                    // the crop model's own atlas, or the shared one the procedural plants use
                    atlas: models
                        .get(f.crop)
                        .map(|m| m.albedo.clone())
                        .or_else(|| models.atlas.as_ref().map(|a| a.albedo.clone()))
                        .unwrap_or(blank[0].clone()),
                    atlas_normal: models
                        .get(f.crop)
                        .map(|m| m.normal.clone())
                        .or_else(|| models.atlas.as_ref().map(|a| a.normal.clone()))
                        .unwrap_or(blank[1].clone()),
                    ground: ground_a.clone(),
                    ground_detail: ground_d.clone(),
                },
            });
            let (lo, hi) = super::bounds(&f.poly());
            let t = tiles(f, origin, rot, size)
                .into_iter()
                .map(|l| (l, 0.0, 0.0))
                .collect();
            r.drawn.insert(
                f.id,
                Drawn {
                    crop: f.crop,
                    outline: f.outline.clone(),
                    rows: f.rows,
                    material,
                    origin,
                    rot,
                    tiles: t,
                    lo,
                    hi,
                    pools: [vec![], vec![], vec![], vec![]],
                    heights_of: u64::MAX,
                },
            );
        }
        r.last_cam = Vec3::splat(f32::MAX);
    }
    // a new height texture (a new world) or new ground under the tiles
    for d in r.drawn.values_mut() {
        if mats.get(&d.material).is_some_and(|m| {
            m.extension.height != gpu.height
                || m.extension.ground != ground_a
                || m.extension.field.tiles != layer_tiles
        }) && let Some(mut m) = mats.get_mut(&d.material)
        {
            m.extension.height = gpu.height.clone();
            m.extension.ground = ground_a.clone();
            m.extension.ground_detail = ground_d.clone();
            m.extension.field.tiles = layer_tiles;
            m.extension.field.tex.x = if ground.albedo.is_some() { 1.0 } else { 0.0 };
        }
        if d.heights_of != terrain.height_revision {
            d.heights_of = terrain.height_revision;
            for (l, lo, hi) in &mut d.tiles {
                let (mut a, mut b) = (f32::MAX, f32::MIN);
                for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.5, 0.5)] {
                    let w =
                        d.origin + (d.rot * Vec3::new(l.x + u * TILE, 0.0, l.y + v * TILE)).xz();
                    let h = terrain.height_at(w.x, w.y);
                    a = a.min(h);
                    b = b.max(h);
                }
                (*lo, *hi) = (a - 1.0, b + 1.0);
            }
            r.last_cam = Vec3::splat(f32::MAX);
        }
    }
}

/// Puts each field's tiles around the camera, at their level of detail.
#[allow(clippy::too_many_arguments)]
fn place_tiles(
    mut commands: Commands,
    mut r: ResMut<CropRender>,
    mut meshes: ResMut<Assets<Mesh>>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
    mut q: Query<(&mut Transform, &mut Aabb, &mut Visibility)>,
    graphics: Res<crate::streaming::Graphics>,
    models: Res<super::assets::CropAssets>,
) {
    let Ok(cam) = cam.single() else { return };
    let p = cam.translation();
    if p.distance(r.last_cam) < 1.0 && !graphics.is_changed() {
        return;
    }
    r.last_cam = p;
    let k = graphics.detail.clamp(0.65, 1.5);
    let r = &mut *r;
    let mut respawned = false;
    let no_atlas = super::plants::Regions::new();
    for d in r.drawn.values_mut() {
        let r3 = d.crop.ranges().map(|x| x * k);
        // past the plants, the field's canopy (far into the distance)
        let ranges = [r3[0], r3[1], r3[2], (r3[2] * 5.0).clamp(700.0, 1600.0) * k];
        let mut wanted: [Vec<(Vec2, f32, f32)>; 4] = [vec![], vec![], vec![], vec![]];
        // the whole field out of reach: nothing to place
        let near = Vec2::new(p.x.clamp(d.lo.x, d.hi.x), p.z.clamp(d.lo.y, d.hi.y));
        if near.distance(p.xz()) < ranges[3] + TILE {
            for (l, lo, hi) in &d.tiles {
                let mid =
                    d.origin + (d.rot * Vec3::new(l.x + TILE * 0.5, 0.0, l.y + TILE * 0.5)).xz();
                let dy = (p.y - p.y.clamp(*lo, *hi)).abs();
                let dist = ((mid.distance(p.xz()) - TILE * 0.7).max(0.0).powi(2) + dy * dy).sqrt();
                if let Some(lod) = ranges.iter().position(|x| dist < *x) {
                    wanted[lod].push((*l, *lo, *hi));
                    // (the canopy also lies under the plants once they get small)
                    if lod < 3 && dist > d.crop.canopy_from(r3) * 0.9 {
                        wanted[3].push((*l, *lo, *hi));
                    }
                }
            }
        }
        if std::env::var("WB_FIELDS_LOG").is_ok() {
            info!(
                "FIELDS {:?}: tiles near/mid/far/canopy {} {} {} {} of {}",
                d.crop,
                wanted[0].len(),
                wanted[1].len(),
                wanted[2].len(),
                wanted[3].len(),
                d.tiles.len()
            );
        }
        for lod in 0..4 {
            let need = wanted[lod].len();
            if need == 0 && d.pools[lod].is_empty() {
                continue;
            }
            let (mesh, height) = r
                .meshes
                .entry((d.crop, lod as u8))
                .or_insert_with(|| {
                    let patch = if lod == 3 {
                        super::plants::canopy(
                            d.crop,
                            models.atlas.as_ref().map_or(&no_atlas, |a| &a.means),
                        )
                    } else {
                        super::plants::patch(
                            d.crop,
                            lod as u8,
                            models.get(d.crop).map(|m| m.as_ref()),
                            models.atlas.as_ref().map_or(&no_atlas, |a| &a.regions),
                        )
                    };
                    let h = patch.height;
                    (meshes.add(patch.mesh()), h)
                })
                .clone();
            while d.pools[lod].len() < need {
                // (`NoAutoAabb`: the plants are placed in the vertex shader, so the bounds Bevy
                // would work out from the mesh lie at y = 0 and the tiles get culled; ours, set
                // below, cover the ground and the crop)
                let e = commands
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(d.material.clone()),
                        Transform::IDENTITY,
                        Aabb::default(),
                        Visibility::Hidden,
                        NotShadowCaster,
                        bevy::camera::visibility::NoAutoAabb,
                    ))
                    .id();
                d.pools[lod].push(e);
                respawned = true;
            }
            for (i, e) in d.pools[lod].clone().into_iter().enumerate() {
                let Ok((mut tf, mut aabb, mut vis)) = q.get_mut(e) else {
                    continue;
                };
                match wanted[lod].get(i) {
                    Some((l, lo, hi)) => {
                        let o = d.origin + (d.rot * Vec3::new(l.x, 0.0, l.y)).xz();
                        *tf = Transform {
                            translation: Vec3::new(o.x, 0.0, o.y),
                            rotation: d.rot,
                            scale: Vec3::ONE,
                        };
                        *aabb = Aabb::from_min_max(
                            Vec3::new(-1.0, lo - 0.5, -1.0),
                            Vec3::new(TILE + 1.0, hi + height + 1.0, TILE + 1.0),
                        );
                        if *vis != Visibility::Inherited {
                            *vis = Visibility::Inherited;
                        }
                    }
                    None => {
                        if *vis != Visibility::Hidden {
                            *vis = Visibility::Hidden;
                        }
                    }
                }
            }
        }
    }
    if respawned {
        // (new entities take their places next frame)
        r.last_cam = Vec3::splat(f32::MAX);
    }
}

fn update_params(
    mut r: ResMut<CropRender>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
    terrain: Res<Terrain>,
    wind: Res<crate::wind::WindState>,
    motion: Res<crate::wind::Motion>,
    time: Res<Time>,
    cam: Query<&GlobalTransform, With<MainCamera>>,
) {
    if r.drawn.is_empty() {
        return;
    }
    if motion.live {
        r.clock += time.delta_secs();
    }
    let speed = if motion.live { wind.speed } else { 0.0 };
    let p = cam.single().map(|c| c.translation()).unwrap_or_default();
    write_params(
        &mut buffers,
        &CropParams {
            grid: Vec4::new(
                terrain.half(),
                terrain.cell,
                terrain.n() as f32,
                terrain.sea_level,
            ),
            wind: Vec4::new(wind.dir.x, wind.dir.y, speed, r.clock),
            gust: Vec4::new(wind.gust_offset.x, wind.gust_offset.y, p.x, p.z),
            view: Vec4::new(p.y, 0.0, 0.0, 0.0),
        },
    );
}
