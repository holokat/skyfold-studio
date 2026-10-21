//! The satellite base: the actual 3D world rendered straight down by an orthographic camera
//! into an offscreen image, one tile at a time, and stitched — for the whole world (the
//! Satellite map style) or any world rectangle at any pixel size (a bare capture agents line up
//! pixel for pixel with reference images: `map_generate` with `rect` / `width`).
//!
//! Terrain LODs, object streaming and the ocean all follow the main camera, so while a tile
//! renders the main camera hovers above it (looking down, high enough that no grass is drawn)
//! and the detail setting is at its maximum; clouds, rain, cloud shadows, haze and fog are off,
//! and the water reads this camera's depth. Editor-only things are hidden: gizmos, the terrain
//! overlay, and every entity marked [`super::HideFromMap`] (markers). Everything is put back
//! afterwards (and a save in the meantime stores the crafter's own lighting and detail).

use std::any::TypeId;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::anti_alias::smaa::Smaa;
use bevy::asset::RenderAssetUsages;
use bevy::camera::{ClearColorConfig, Exposure, Hdr, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::ShadowFilteringMethod;
use bevy::pbr::{ScreenSpaceTransmission, ScreenSpaceTransmissionQuality};
use bevy::prelude::*;
use bevy::render::gpu_readback::{ReadbackComplete, ReadbackOnce};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use super::{MapSettings, WorldMap};
use crate::camera::MainCamera;
use crate::editor::{Editor, Tab};
use crate::sky::Lighting;
use crate::streaming::Graphics;
use crate::terrain::Terrain;
use crate::terrain::overlay::TerrainOverlay;
use crate::water_render::{SceneDepth, WaterDepthTarget};

enum Phase {
    /// Waiting for streaming and LODs to catch up (frames so far).
    Settle(u32),
    /// Read-back requested (frames waited).
    Read(u32),
}

struct Stash {
    lighting: Lighting,
    graphics: Graphics,
    tab: Tab,
    surface_overlay: bool,
    overlay: (Option<Handle<Image>>, f32, &'static str),
    scene_depth: Handle<Image>,
    hidden: Vec<(Entity, Visibility)>,
    gizmos: Vec<(TypeId, bool)>,
}

/// A bare capture of a world rectangle.
#[derive(Clone, Debug)]
pub struct Shot {
    /// World xz rectangle: min x, min z, max x, max z (m). North (−Z) is the top of the image.
    pub rect: [f32; 4],
    pub width: u32,
    pub height: u32,
    /// PNG to write.
    pub path: PathBuf,
}

impl Shot {
    /// How world positions map to its pixels.
    pub fn transform(&self) -> serde_json::Value {
        let [x0, z0, x1, z1] = self.rect;
        let (sx, sz) = (
            self.width as f32 / (x1 - x0),
            self.height as f32 / (z1 - z0),
        );
        serde_json::json!({
            "world_rect_xz": self.rect,
            "size_px": [self.width, self.height],
            "px_per_m": [sx, sz],
            "world_to_px": "px = (x - rect[0]) * px_per_m[0], py = (z - rect[1]) * px_per_m[1] (pixel centres at +0.5)",
            "px_to_world": "x = rect[0] + px / px_per_m[0], z = rect[1] + py / px_per_m[1]",
            "orientation": "orthographic, straight down; north (−Z) up, east (+X) right; pixel (0,0) is the rect's north-west corner",
        })
    }
}

/// A finished bare capture.
pub struct ShotResult {
    pub shot: Shot,
    /// PNG bytes, at most 1024 px.
    pub preview: Vec<u8>,
    pub millis: u32,
}

enum Purpose {
    /// The Satellite map style: the whole world, then lettering on top.
    Map(MapSettings),
    Shot(Shot),
}

pub struct Capture {
    purpose: Purpose,
    /// World xz of the north-west corner, metres per pixel along x and z.
    min: Vec2,
    mpp: Vec2,
    tiles_x: u32,
    tiles_z: u32,
    tile_px: u32,
    /// Camera height and far plane, the same for every tile so the water (which reads the depth
    /// buffer) shades alike across tile seams.
    top: f32,
    far: f32,
    index: u32,
    phase: Phase,
    image: Vec<u8>,
    target: Handle<Image>,
    depth: Handle<Image>,
    camera: Entity,
    data: Arc<Mutex<Option<Vec<u8>>>>,
    reader: Option<Entity>,
    stash: Stash,
    started: Instant,
    cancelled: bool,
}

impl Capture {
    pub fn progress(&self) -> (f32, String) {
        let n = (self.tiles_x * self.tiles_z) as f32;
        let f = (self.index as f32
            + if matches!(self.phase, Phase::Read(_)) {
                0.8
            } else {
                0.3
            })
            / n;
        let share = if matches!(self.purpose, Purpose::Map(_)) {
            0.7
        } else {
            1.0
        };
        (
            f * share,
            format!(
                "Rendering tile {} of {} · {:.0} s",
                self.index + 1,
                n as u32,
                self.started.elapsed().as_secs_f32()
            ),
        )
    }
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
    pub fn saved_lighting(&self) -> &Lighting {
        &self.stash.lighting
    }
    pub fn saved_graphics(&self) -> &Graphics {
        &self.stash.graphics
    }
    fn tile_m(&self) -> Vec2 {
        self.mpp * self.tile_px as f32
    }
    fn tile_centre(&self, k: u32) -> Vec2 {
        let (tx, ty) = (k % self.tiles_x, k / self.tiles_x);
        self.min + Vec2::new(tx as f32 + 0.5, ty as f32 + 0.5) * self.tile_m()
    }
}

/// The Satellite map style's base: the whole world at the map's size.
pub fn begin(world: &mut World, settings: MapSettings) {
    let size_m = world.resource::<Terrain>().size();
    // tiles small enough that every tree in one is drawn with a full crown (see `start`)
    let per = ((size_m / 560.0).ceil() as u32)
        .next_power_of_two()
        .clamp(1, 16)
        .min(settings.size / 128);
    let tile_px = settings.size / per;
    let mpp = Vec2::splat(size_m / settings.size as f32);
    start(
        world,
        Purpose::Map(settings),
        Vec2::splat(-size_m * 0.5),
        mpp,
        per,
        per,
        tile_px,
    );
}

/// A bare capture of `shot.rect` at `shot.width × shot.height` px (no lettering).
pub fn begin_shot(world: &mut World, shot: Shot) {
    let [x0, z0, x1, z1] = shot.rect;
    let mpp = Vec2::new(
        (x1 - x0) / shot.width as f32,
        (z1 - z0) / shot.height as f32,
    );
    // square tiles of whole 64 px blocks, about 560 m across
    let tile_px = (((560.0 / mpp.max_element()) as u32) / 64 * 64).clamp(64, 1024);
    let tiles_x = shot.width.div_ceil(tile_px);
    let tiles_z = shot.height.div_ceil(tile_px);
    start(
        world,
        Purpose::Shot(shot.clone()),
        Vec2::new(x0, z0),
        mpp,
        tiles_x,
        tiles_z,
        tile_px,
    );
}

fn start(
    world: &mut World,
    purpose: Purpose,
    min: Vec2,
    mpp: Vec2,
    tiles_x: u32,
    tiles_z: u32,
    tile_px: u32,
) {
    let tile_m = mpp * tile_px as f32;
    let (lo, hi, sea) = {
        let t = world.resource::<Terrain>();
        let (lo, hi) = t
            .heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
        (lo.min(t.sea_level - 2.0), hi.max(t.sea_level), t.sea_level)
    };
    // high enough that the view over open water is near vertical across a tile (no seams from
    // glancing reflections); the detail setting then reaches far enough down that every tree in
    // the tile is drawn with a full crown (its two nearest mesh LODs: the farthest one is sparse
    // seen from above)
    let top = (hi + 60.0).max(sea + 420.0);
    let far = top - lo + 80.0;
    let reach = Vec2::new(tile_m.length() * 0.5, top - sea).length();
    let detail = (reach / 100.0).clamp(2.0, 6.0);

    // put the world in "satellite weather"
    let lighting = world.resource::<Lighting>().clone();
    let graphics = world.resource::<Graphics>().clone();
    {
        let mut l = world.resource_mut::<Lighting>();
        if !(9.0..=15.5).contains(&l.hour) {
            l.hour = 11.0;
        }
        l.playing = false;
        l.cloud_coverage = 0.0;
        l.cloud_shadows = 0.0;
        l.cirrus = 0.0;
        l.haze = 0.0;
        l.weather_override = Some(crate::world::Weather::Clear);
        l.precipitation = false;
    }
    world.resource_mut::<Graphics>().detail = detail;
    let (tab, surface_overlay) = {
        let mut ed = world.resource_mut::<Editor>();
        let s = (ed.tab, ed.surface_overlay);
        ed.tab = Tab::Map;
        ed.surface_overlay = false;
        s
    };
    let overlay = {
        let mut o = world.resource_mut::<TerrainOverlay>();
        let s = (o.image.take(), o.opacity, o.owner);
        o.opacity = 0.0;
        s
    };
    let mut hidden = Vec::new();
    let mut q = world.query::<(
        Entity,
        Option<&Name>,
        Has<super::HideFromMap>,
        &mut Visibility,
    )>();
    for (e, name, marker, mut vis) in q.iter_mut(world) {
        if marker || name.is_some_and(|n| matches!(n.as_str(), "Clouds" | "Precipitation")) {
            hidden.push((e, *vis));
            *vis = Visibility::Hidden;
        }
    }
    let mut gizmos = Vec::new();
    if let Some(mut store) = world.get_resource_mut::<GizmoConfigStore>() {
        for (id, config, _) in store.iter_mut() {
            gizmos.push((*id, config.enabled));
            config.enabled = false;
        }
    }

    // offscreen colour target and the depth copy the water reads
    let mut target = Image::new_target_texture(
        tile_px,
        tile_px,
        TextureFormat::Rgba8Unorm,
        Some(TextureFormat::Rgba8UnormSrgb),
    );
    target.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let mut depth = Image::new_uninit(
        Extent3d {
            width: tile_px,
            height: tile_px,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Depth32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    depth.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST;
    let (target, depth) = {
        let mut images = world.resource_mut::<Assets<Image>>();
        (images.add(target), images.add(depth))
    };
    let scene_depth = {
        let mut sd = world.resource_mut::<SceneDepth>();
        std::mem::replace(&mut sd.image, depth.clone())
    };
    let exposure = world
        .query_filtered::<&Exposure, With<MainCamera>>()
        .iter(world)
        .next()
        .copied()
        .unwrap_or(Exposure { ev100: 13.0 });
    let camera = world
        .spawn((
            Name::new("Satellite map camera"),
            Camera3d {
                depth_texture_usages: (TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_SRC)
                    .into(),
                ..default()
            },
            Camera {
                order: -40,
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Hdr,
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed {
                    width: tile_m.x,
                    height: tile_m.y,
                },
                near: 0.1,
                far: 4000.0,
                ..OrthographicProjection::default_3d()
            }),
            Transform::from_xyz(0.0, 1000.0, 0.0).looking_to(Vec3::NEG_Y, Vec3::NEG_Z),
            exposure,
            Tonemapping::AgX,
            ambient(exposure),
            Msaa::Off,
            Smaa::default(),
            ShadowFilteringMethod::Gaussian,
            ScreenSpaceTransmission {
                steps: 1,
                quality: ScreenSpaceTransmissionQuality::Low,
            },
            WaterDepthTarget(depth.clone()),
        ))
        .id();
    info!(
        "MAPLOG satellite: {tiles_x}×{tiles_z} tiles of {tile_px} px ({:.0} m)",
        tile_m.x
    );
    world.resource_mut::<WorldMap>().capture = Some(Capture {
        purpose,
        min,
        mpp,
        tiles_x,
        tiles_z,
        tile_px,
        top,
        far,
        index: 0,
        phase: Phase::Settle(0),
        image: vec![0; (tiles_x * tiles_z * tile_px * tile_px * 4) as usize],
        target,
        depth,
        camera,
        data: Arc::new(Mutex::new(None)),
        reader: None,
        stash: Stash {
            lighting,
            graphics,
            tab,
            surface_overlay,
            overlay,
            scene_depth,
            hidden,
            gizmos,
        },
        started: Instant::now(),
        cancelled: false,
    });
}

/// Sky fill for the top-down camera (the main camera's comes from the atmosphere): about a
/// quarter of the key light, a little blue.
fn ambient(e: Exposure) -> AmbientLight {
    AmbientLight {
        color: Color::srgb(0.78, 0.86, 1.0),
        brightness: 0.32 * 1.2 * 2f32.powf(e.ev100),
        affects_lightmapped_meshes: false,
    }
}

pub fn step(world: &mut World) {
    let Some(mut cap) = world.resource_mut::<WorldMap>().capture.take() else {
        return;
    };
    if cap.cancelled {
        finish(world, cap, false);
        return;
    }
    let centre = cap.tile_centre(cap.index);
    let ground = {
        let t = world.resource::<Terrain>();
        t.height_at(centre.x, centre.y).max(t.sea_level)
    };
    // the main camera hovers over the tile (streaming, LODs and the ocean centre on it)
    let mut q = world.query_filtered::<(&mut Transform, &Exposure), With<MainCamera>>();
    let mut exposure = Exposure { ev100: 13.0 };
    if let Some((mut t, e)) = q.iter_mut(world).next() {
        *t = Transform::from_xyz(centre.x, ground + 180.0, centre.y)
            .looking_to(Vec3::NEG_Y, Vec3::NEG_Z);
        exposure = *e;
    }
    if let Ok(mut e) = world.get_entity_mut(cap.camera) {
        if let Some(mut t) = e.get_mut::<Transform>() {
            *t = Transform::from_xyz(centre.x, cap.top, centre.y)
                .looking_to(Vec3::NEG_Y, Vec3::NEG_Z);
        }
        if let Some(mut p) = e.get_mut::<Projection>()
            && let Projection::Orthographic(o) = &mut *p
        {
            o.far = cap.far;
        }
        e.insert((exposure, ambient(exposure)));
    }
    match cap.phase {
        Phase::Settle(f) => {
            let queued = !world
                .resource::<crate::objects::ObjectStore>()
                .to_spawn
                .is_empty();
            let min = if cap.index == 0 { 75 } else { 6 };
            if (f >= min && !queued) || f > 240 {
                if std::env::var("WB_MAP_STATS").is_ok() {
                    info!(
                        "MAPLOG tile {} after {f} frames, ev100 {:.2}",
                        cap.index, exposure.ev100
                    );
                }
                let data = cap.data.clone();
                let reader = world
                    .spawn(ReadbackOnce::texture(cap.target.clone()))
                    .observe(move |ev: On<ReadbackComplete>| {
                        if let Ok(mut d) = data.lock() {
                            *d = Some(ev.data.clone());
                        }
                    })
                    .id();
                cap.reader = Some(reader);
                cap.phase = Phase::Read(0);
            } else {
                cap.phase = Phase::Settle(f + 1);
            }
        }
        Phase::Read(f) => {
            let got = cap.data.lock().ok().and_then(|mut d| d.take());
            if let Some(px) = got {
                if let Some(r) = cap.reader.take()
                    && let Ok(e) = world.get_entity_mut(r)
                {
                    e.despawn();
                }
                let tp = cap.tile_px as usize;
                let row = tp * 4;
                // rows may be padded to 256 bytes
                let stride = if px.len() >= row * tp {
                    px.len() / tp
                } else {
                    row
                };
                let (tx, ty) = (
                    (cap.index % cap.tiles_x) as usize,
                    (cap.index / cap.tiles_x) as usize,
                );
                let size = (cap.tiles_x * cap.tile_px) as usize;
                for y in 0..tp {
                    let src = &px[y * stride..y * stride + row];
                    let dst = ((ty * tp + y) * size + tx * tp) * 4;
                    cap.image[dst..dst + row].copy_from_slice(src);
                }
                cap.index += 1;
                cap.phase = Phase::Settle(0);
                if cap.index >= cap.tiles_x * cap.tiles_z {
                    info!(
                        "MAPLOG satellite tiles captured in {:?}",
                        cap.started.elapsed()
                    );
                    finish(world, cap, true);
                    return;
                }
            } else if f > 180 {
                warn!("satellite map: tile read-back timed out; retrying");
                cap.phase = Phase::Settle(0);
            } else {
                cap.phase = Phase::Read(f + 1);
            }
        }
    }
    world.resource_mut::<WorldMap>().capture = Some(cap);
}

/// Puts everything back; on success hands the stitched image to the map generator (or writes
/// the bare capture).
fn finish(world: &mut World, cap: Capture, ok: bool) {
    let Capture {
        purpose,
        image: pixels,
        target,
        depth,
        camera,
        reader,
        stash,
        tiles_x,
        tile_px,
        started,
        ..
    } = cap;
    *world.resource_mut::<Lighting>() = stash.lighting;
    *world.resource_mut::<Graphics>() = stash.graphics;
    {
        let mut ed = world.resource_mut::<Editor>();
        ed.tab = stash.tab;
        ed.surface_overlay = stash.surface_overlay;
    }
    {
        let mut o = world.resource_mut::<TerrainOverlay>();
        o.image = stash.overlay.0;
        o.opacity = stash.overlay.1;
        o.owner = stash.overlay.2;
    }
    world.resource_mut::<SceneDepth>().image = stash.scene_depth;
    for (e, v) in stash.hidden {
        if let Ok(mut ent) = world.get_entity_mut(e) {
            ent.insert(v);
        }
    }
    if let Some(mut store) = world.get_resource_mut::<GizmoConfigStore>() {
        for (id, config, _) in store.iter_mut() {
            if let Some((_, on)) = stash.gizmos.iter().find(|(g, _)| g == id) {
                config.enabled = *on;
            }
        }
    }
    for e in [Some(camera), reader].into_iter().flatten() {
        if let Ok(ent) = world.get_entity_mut(e) {
            ent.despawn();
        }
    }
    {
        let mut images = world.resource_mut::<Assets<Image>>();
        images.remove(&target);
        images.remove(&depth);
    }
    if !ok {
        world.resource_mut::<WorldMap>().status = Some("Cancelled.".into());
        return;
    }
    match purpose {
        Purpose::Map(settings) => super::spawn_job(world, settings, Some(pixels), false),
        Purpose::Shot(shot) => {
            // crop the whole tiles to the asked size and write the PNG off the main thread
            let stride = (tiles_x * tile_px) as usize;
            let h = std::thread::Builder::new()
                .name("satellite-capture".into())
                .spawn(move || {
                    let (w, hgt) = (shot.width as usize, shot.height as usize);
                    let mut rgb = Vec::with_capacity(w * hgt * 3);
                    for y in 0..hgt {
                        for p in pixels[y * stride * 4..(y * stride + w) * 4].chunks_exact(4) {
                            rgb.extend_from_slice(&p[..3]);
                        }
                    }
                    if let Some(dir) = shot.path.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    }
                    super::export::write_png(
                        &shot.path,
                        &rgb,
                        shot.width,
                        shot.height,
                        image::ExtendedColorType::Rgb8,
                    )?;
                    let preview = super::export::preview_rgb(&rgb, shot.width, shot.height, 1024)?;
                    Ok(ShotResult {
                        shot,
                        preview,
                        millis: started.elapsed().as_millis() as u32,
                    })
                })
                .expect("spawn capture thread");
            world.resource_mut::<WorldMap>().shot_job = Some(h);
        }
    }
}
