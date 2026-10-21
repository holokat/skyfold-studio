//! Fish models: each species' mesh (body, fins, eye and the Old Drowner's hooks) from
//! `assets/models/fish/<model>.glb` — length 1 along +Z, nose at z = +0.5 — its painted skin
//! (decoded off the main thread, with a mip chain), and the swimming material: a travelling body
//! wave in the vertex shader (`shaders/fish_swim.wesl`), so one mesh serves every fish and pose.
//! Each fish's tail-beat phase, turn bend and amplitude ride in its `MeshTag`, so a whole species
//! draws as one batch however many swim.

use std::collections::HashMap;

use bevy::asset::{RenderAssetUsages, uuid_handle};
use bevy::camera::visibility::RenderLayers;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::NotShadowCaster;
use bevy::mesh::MeshTag;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::species::{FishSpecies, Plan};

pub type FishMaterial = ExtendedMaterial<StandardMaterial, FishExt>;

/// The clock world fish swim by (x, seconds): it only runs while the world animates
/// (`wind::Motion`). Previews swim by the real clock instead (`TAG_LIVE`).
pub const FISH_CLOCK: Handle<ShaderBuffer> = uuid_handle!("2b7f0c64-5e1d-4f4b-9a0e-6c3d8e1f7a29");

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct FishExt {
    #[storage(100, read_only)]
    pub clock: Handle<ShaderBuffer>,
    /// x: waves along the body (rad), y: tail amplitude (body lengths; a ray's wing beat),
    /// z: tail beats per second, w: body plan (`Plan::code`)
    #[uniform(101)]
    pub swim: Vec4,
}

impl MaterialExtension for FishExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/fish.wesl".into()
    }
    fn prepass_vertex_shader() -> ShaderRef {
        "shaders/fish_prepass.wesl".into()
    }
}

/// Bit 24 of a fish's mesh tag: swim by the real clock (previews), not the world's.
const TAG_LIVE: u32 = 1 << 24;

/// Packs a fish's swim state into its mesh tag: tail-beat phase (0..1), turn bend (−1..1, + curls
/// the tail toward +x), amplitude (0 holds still … 2), `live` = by the real clock.
pub fn swim_tag(phase: f32, bend: f32, amp: f32, live: bool) -> MeshTag {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    MeshTag::new(
        b(phase.fract())
            | b(bend * 0.5 + 0.5) << 8
            | b(amp * 0.5) << 16
            | if live { TAG_LIVE } else { 0 },
    )
}

/// A species' parts, ready to spawn.
#[derive(Clone)]
pub struct Model {
    pub parts: Vec<(Handle<Mesh>, Handle<FishMaterial>)>,
    /// the painting (filled in once decoded)
    pub skin: Handle<Image>,
}

#[derive(Resource, Default)]
pub struct FishModels {
    models: HashMap<FishSpecies, Model>,
    skins: Vec<(Handle<Image>, Task<Option<Image>>)>,
}

impl FishModels {
    /// The species' model, starting to load it on first use (meshes stream in through the asset
    /// server, the skin through a background task; the fish draws once both are there).
    pub fn model(
        &mut self,
        sp: FishSpecies,
        server: &AssetServer,
        materials: &mut Assets<FishMaterial>,
        images: &mut Assets<Image>,
    ) -> Model {
        if let Some(m) = self.models.get(&sp) {
            return m.clone();
        }
        let info = sp.info();
        let path = format!("models/fish/{}.glb", info.model);
        let skin = images.reserve_handle();
        let file = std::path::PathBuf::from(crate::assets_path())
            .join(format!("models/fish/{}.png", info.model));
        self.skins.push((
            skin.clone(),
            AsyncComputeTaskPool::get().spawn(async move { load_skin(&file) }),
        ));
        let (k, amp) = match info.plan {
            Plan::Fish => (5.2, 0.05),
            Plan::Flat => (6.0, 0.04),
            Plan::Eel => (9.5, 0.06),
            Plan::Ray => (3.0, 0.16),
        };
        let beats = (0.9 + info.speed / info.length.max(0.15) * 0.9).min(4.0);
        let ext = FishExt {
            clock: FISH_CLOCK,
            swim: Vec4::new(k, amp, beats, info.plan.code()),
        };
        let mut mat = |base: StandardMaterial| {
            materials.add(FishMaterial {
                base,
                extension: ext.clone(),
            })
        };
        let body = mat(StandardMaterial {
            base_color_texture: Some(skin.clone()),
            perceptual_roughness: 0.42,
            metallic: 0.08,
            ..default()
        });
        let fins = mat(StandardMaterial {
            base_color_texture: Some(skin.clone()),
            perceptual_roughness: 0.6,
            alpha_mode: AlphaMode::Mask(0.45),
            double_sided: true,
            cull_mode: None,
            ..default()
        });
        let eye = mat(StandardMaterial {
            base_color: Color::srgb(0.043, 0.043, 0.047),
            perceptual_roughness: 0.08,
            metallic: 0.2,
            ..default()
        });
        let mut parts = vec![body, fins, eye];
        if sp == FishSpecies::Drowner {
            // the hooks and leaders in its lip
            parts.push(mat(StandardMaterial {
                base_color: Color::srgb(0.29, 0.20, 0.15),
                perceptual_roughness: 0.62,
                metallic: 0.7,
                ..default()
            }));
        }
        let model = Model {
            skin,
            parts: parts
                .into_iter()
                .enumerate()
                .map(|(i, m)| {
                    (
                        server.load(
                            GltfAssetLabel::Primitive {
                                mesh: 0,
                                primitive: i,
                            }
                            .from_asset(path.clone()),
                        ),
                        m,
                    )
                })
                .collect(),
        };
        self.models.insert(sp, model.clone());
        model
    }
}

/// Spawns one fish: a parent with `transform` (the model is 1 m long: scale it to the species'
/// length) and a child per part. `layers`: render only to those cameras (thumbnails, previews).
pub fn spawn_fish(
    commands: &mut Commands,
    model: &Model,
    transform: Transform,
    tag: MeshTag,
    layers: Option<&RenderLayers>,
) -> Entity {
    let mut e = commands.spawn((transform, Visibility::default()));
    e.with_children(|c| {
        for (mesh, mat) in &model.parts {
            let mut p = c.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                tag.clone(),
                NotShadowCaster,
            ));
            if let Some(l) = layers {
                p.insert(l.clone());
            }
        }
    });
    if let Some(l) = layers {
        e.insert(l.clone());
    }
    e.id()
}

/// Puts decoded skins in place.
pub(super) fn finish_skins(mut models: ResMut<FishModels>, mut images: ResMut<Assets<Image>>) {
    models.skins.retain_mut(|(handle, task)| {
        let Some(img) = block_on(future::poll_once(task)) else {
            return true;
        };
        match img {
            Some(img) => {
                let _ = images.insert(handle.id(), img);
            }
            None => warn!("fish: a skin painting failed to load"),
        }
        false
    });
}

pub(super) fn init_clock(mut buffers: ResMut<Assets<ShaderBuffer>>) {
    let _ = buffers.insert(
        &FISH_CLOCK,
        ShaderBuffer::new(
            vec![0.0f32; 4],
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        ),
    );
}

/// Advances the world fish clock while the world animates.
pub(super) fn tick_clock(
    time: Res<Time>,
    motion: Res<crate::wind::Motion>,
    mut clock: Local<f32>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    if !motion.live {
        return;
    }
    *clock += time.delta_secs();
    let _ = buffers.insert(
        &FISH_CLOCK,
        ShaderBuffer::new(
            vec![*clock, 0.0, 0.0, 0.0],
            RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
        ),
    );
}

/// A skin painting (sRGB RGBA, any size) with its mip chain: colour averaged in linear light
/// weighted by coverage, so fins keep their colour as they shrink.
fn load_skin(path: &std::path::Path) -> Option<Image> {
    let img = image::open(path)
        .map_err(|e| warn!("{}: {e}", path.display()))
        .ok()?
        .into_rgba8();
    let (w, h) = img.dimensions();
    let to_lin: Vec<f32> = (0..256)
        .map(|v| Color::srgb_u8(v as u8, 0, 0).to_linear().red)
        .collect();
    let to_srgb = |v: f32| {
        (Color::linear_rgb(v.clamp(0.0, 1.0), 0.0, 0.0)
            .to_srgba()
            .red
            * 255.0)
            .round() as u8
    };
    let mut data = img.into_raw();
    let mut level: Vec<u8> = data.clone();
    let (mut lw, mut lh) = (w as usize, h as usize);
    let mut levels = 1;
    while lw > 1 || lh > 1 {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                let mut rgb = [0.0f32; 3];
                let mut a = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(lw - 1), (y * 2 + dy).min(lh - 1));
                    let p = &level[(sy * lw + sx) * 4..][..4];
                    let pa = p[3] as f32 / 255.0 + 1e-4;
                    for c in 0..3 {
                        rgb[c] += to_lin[p[c] as usize] * pa;
                    }
                    a += pa;
                }
                let o = &mut next[(y * nw + x) * 4..][..4];
                for c in 0..3 {
                    o[c] = to_srgb(rgb[c] / a);
                }
                o[3] = (a / 4.0 * 255.0).round().min(255.0) as u8;
            }
        }
        data.extend_from_slice(&next);
        level = next;
        (lw, lh) = (nw, nh);
        levels += 1;
    }
    let mut out = Image::new_uninit(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    out.data = Some(data);
    out.texture_descriptor.mip_level_count = levels;
    out.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    Some(out)
}
