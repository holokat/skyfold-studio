//! Mip chains and GPU block compression for model textures. Bevy decodes a glTF's JPG / PNG
//! textures as plain RGBA8 with a single level, so tiled slate, planks and plaster shimmer and
//! sparkle a few metres away, and every 1024² texture takes 4 MB of VRAM. Every texture of a
//! library or imported model (`models/…`, `imported/…`, or embedded in any glTF) is rebuilt on the
//! async compute pool once it has loaded: its mip chain (colour maps averaged in linear light,
//! data maps as stored), block-compressed where the GPU samples BC formats (`super::compress`: BC5
//! for normal maps, BC7 for colour and packed data; cached on disk), so a 1024² texture takes
//! ~1.4 MB with all its levels instead of 4 MB without them.
//!
//! Finished images are swapped in a few per frame (`SWAP_BYTES`, `SWAP_COUNT`: a model-heavy world
//! streaming in doesn't hitch), with anisotropic filtering where their sampler allows, and keep
//! only their GPU copy (nothing on the CPU reads model textures). Bevy doesn't re-prepare a
//! material when one of its images changes (its bind group would keep the old texture, and
//! `StandardMaterial` would not see a normal map's new two-channel format), so the materials using
//! a swapped image are marked changed.
//!
//! Images built elsewhere with their own mip chains (tree bark and leaves, `trees`) ask for the
//! same compression with `compress_later`.
//!
//! `WB_NOMIPS=1` turns all of this off, `WB_NOBC=1` keeps the mip chains uncompressed (A/B tests).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use bevy::asset::{AssetPath, RenderAssetUsages};
use bevy::image::{
    CompressedImageFormatSupport, CompressedImageFormats, ImageFilterMode, ImageSampler,
    ImageSamplerDescriptor,
};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};

use super::compress::{self, Codec, Effort};

/// Upload budget per frame for swapped-in images (bytes of their new data)…
const SWAP_BYTES: usize = 16 << 20;
/// …and at most this many images per frame (each re-prepares its materials). At least one image
/// is swapped every frame.
const SWAP_COUNT: usize = 12;
/// Frames a loaded texture waits for a material that tells its use (normal map or not) before it
/// is compressed as a general colour / data map.
const ROLE_WAIT: u32 = 120;

/// How materials use an image (bits).
const COLOR: u8 = 1;
const NORMAL: u8 = 2;
const DATA: u8 = 4;

/// Images with a finished mip chain waiting to be block-compressed: (image, is a normal map).
static REQUESTS: Mutex<Vec<(AssetId<Image>, bool)>> = Mutex::new(Vec::new());

/// Asks for an image's own mip chain (RGBA8) to be block-compressed in the background where the
/// GPU samples BC formats: BC5 for a normal map (`StandardMaterial` rebuilds Z), BC7 otherwise.
pub fn compress_later(id: AssetId<Image>, normal_map: bool) {
    if let Ok(mut r) = REQUESTS.lock() {
        r.push((id, normal_map));
    }
}

/// A finished image and what its build cost.
struct Built {
    image: Image,
    /// bytes of the image as it was loaded (one RGBA8 level) and as rebuilt
    before: usize,
    after: usize,
    secs: f64,
    /// read from the disk cache
    cached: bool,
    /// error of the compressed top level against the source (dB), when encoded now
    psnr: Option<f64>,
}

#[derive(Resource, Default)]
pub struct MipJobs {
    /// loaded textures waiting to learn their use: (image, frames waited)
    pending: Vec<(AssetId<Image>, u32)>,
    running: Vec<(AssetId<Image>, Task<Built>)>,
    /// finished, waiting for their frame's swap budget
    ready: VecDeque<(AssetId<Image>, Built)>,
    done: HashSet<AssetId<Image>>,
    roles: HashMap<AssetId<Image>, u8>,
    /// (textures done, bytes added, worker seconds) for logs
    pub stats: (usize, usize, f64),
    /// (VRAM of the textures as loaded, as rebuilt, compressed count, from the cache, encode PSNRs)
    summary: (usize, usize, usize, usize, Vec<f64>),
    /// (frames that swapped images in, most bytes swapped in one frame) since the last summary
    spread: (usize, usize),
    logged: usize,
}

/// A texture of a placeable model?
fn is_model_texture(path: &AssetPath) -> bool {
    let p = path.path().to_string_lossy();
    p.starts_with("models/")
        || p.starts_with("imported/")
        || path.label().is_some_and(|l| l.starts_with("Texture"))
}

fn note_roles(m: &StandardMaterial, roles: &mut HashMap<AssetId<Image>, u8>) {
    let mut add = |h: &Option<Handle<Image>>, r: u8| {
        if let Some(h) = h {
            *roles.entry(h.id()).or_default() |= r;
        }
    };
    add(&m.base_color_texture, COLOR);
    add(&m.emissive_texture, COLOR);
    add(&m.normal_map_texture, NORMAL);
    add(&m.metallic_roughness_texture, DATA);
    add(&m.occlusion_texture, DATA);
}

/// Starts a rebuild for every model texture that just loaded (once its use is known).
pub fn queue(
    mut events: MessageReader<AssetEvent<Image>>,
    mut mat_events: MessageReader<AssetEvent<StandardMaterial>>,
    images: Res<Assets<Image>>,
    materials: Res<Assets<StandardMaterial>>,
    server: Res<AssetServer>,
    formats: Option<Res<CompressedImageFormatSupport>>,
    mut jobs: ResMut<MipJobs>,
) {
    if std::env::var("WB_NOMIPS").is_ok() {
        return;
    }
    for ev in mat_events.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = ev
            && let Some(m) = materials.get(*id)
        {
            note_roles(m, &mut jobs.roles);
        }
    }
    for ev in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::LoadedWithDependencies { id }) = ev else {
            continue;
        };
        if jobs.done.contains(id)
            || jobs.pending.iter().any(|(j, _)| j == id)
            || jobs.running.iter().any(|(j, _)| j == id)
        {
            continue;
        }
        let Some(img) = images.get(*id) else { continue };
        let d = &img.texture_descriptor;
        let wanted = matches!(
            d.format,
            TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm
        ) && d.mip_level_count == 1
            && d.dimension == TextureDimension::D2
            && d.size.depth_or_array_layers == 1
            && d.size.width.max(d.size.height) >= 64
            && img.data.is_some();
        if wanted && server.get_path(*id).is_some_and(|p| is_model_texture(&p)) {
            jobs.pending.push((*id, 0));
        }
    }
    let bc = std::env::var("WB_NOBC").is_err()
        && formats.is_some_and(|f| f.0.contains(CompressedImageFormats::BC));
    let requests = REQUESTS
        .lock()
        .map(|mut r| std::mem::take(&mut *r))
        .unwrap_or_default();
    for (id, normal_map) in requests {
        let Some(img) = images.get(id) else { continue };
        let d = &img.texture_descriptor;
        if !bc
            || jobs.done.contains(&id)
            || !matches!(
                d.format,
                TextureFormat::Rgba8UnormSrgb | TextureFormat::Rgba8Unorm
            )
            || img.data.is_none()
        {
            continue;
        }
        let codec = if normal_map {
            Codec::Bc5
        } else {
            Codec::Bc7 {
                alpha: img
                    .data
                    .as_ref()
                    .is_some_and(|d| d.chunks_exact(4).any(|p| p[3] < 255)),
            }
        };
        let img = img.clone();
        jobs.done.insert(id);
        jobs.running.push((
            id,
            AsyncComputeTaskPool::get().spawn(async move { build(img, Some(codec)) }),
        ));
    }
    let MipJobs {
        pending,
        running,
        done,
        roles,
        ..
    } = &mut *jobs;
    pending.retain_mut(|(id, waited)| {
        *waited += 1;
        let role = roles.get(id).copied().unwrap_or(0);
        if role == 0 && *waited < ROLE_WAIT {
            return true;
        }
        let Some(img) = images.get(*id) else {
            return false;
        };
        let codec = bc.then(|| {
            if role == NORMAL && img.texture_descriptor.format == TextureFormat::Rgba8Unorm {
                Codec::Bc5
            } else {
                Codec::Bc7 {
                    alpha: img
                        .data
                        .as_ref()
                        .is_some_and(|d| d.chunks_exact(4).any(|p| p[3] < 255)),
                }
            }
        });
        let img = img.clone();
        done.insert(*id);
        running.push((
            *id,
            AsyncComputeTaskPool::get().spawn(async move { build(img, codec) }),
        ));
        false
    });
}

/// Mip chain (unless the image has one), then (with a codec) the compressed chain, from the disk
/// cache when it's there.
fn build(img: Image, codec: Option<Codec>) -> Built {
    let t0 = std::time::Instant::now();
    let before = img.data.as_ref().map_or(0, Vec::len);
    let (w, h) = (
        img.texture_descriptor.size.width,
        img.texture_descriptor.size.height,
    );
    let has_chain = img.texture_descriptor.mip_level_count > 1;
    // (BC textures need whole 4×4 blocks at the top level)
    let Some(codec) = codec.filter(|_| w % 4 == 0 && h % 4 == 0) else {
        let image = if has_chain { img } else { with_mips(img) };
        let after = image.data.as_ref().map_or(0, Vec::len);
        return Built {
            image,
            before,
            after,
            secs: t0.elapsed().as_secs_f64(),
            cached: false,
            psnr: None,
        };
    };
    let srgb = img.texture_descriptor.format == TextureFormat::Rgba8UnormSrgb;
    let levels = if has_chain {
        img.texture_descriptor.mip_level_count
    } else {
        w.max(h).ilog2() + 1
    };
    let key = compress::cache_key(
        compress::content_hash(img.data.as_deref().unwrap_or_default()),
        w,
        h,
        codec,
        srgb,
        Effort::Fast,
    );
    let sampler = if has_chain {
        img.sampler.clone()
    } else {
        mip_sampler(&img.sampler)
    };
    let (blocks, cached, psnr) = match compress::read_cache(key, w, h, levels) {
        Some(b) => (b, true, None),
        None => {
            let src = img.data.clone().unwrap_or_default();
            let chain = if has_chain {
                src.clone()
            } else {
                with_mips(img).data.unwrap_or_default()
            };
            let b = compress::encode_chain(&chain, w, h, levels, codec, Effort::Fast);
            let top = compress::decode_level(&b[..compress::level_bytes(w, h)], w, h, codec);
            let psnr = compress::psnr(&src[..(w * h * 4) as usize], &top, codec.channels());
            compress::write_cache(key, w, h, levels, &b);
            (b, false, Some(psnr))
        }
    };
    let mut image = Image::new_uninit(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        codec.format(srgb),
        RenderAssetUsages::RENDER_WORLD,
    );
    let after = blocks.len();
    image.data = Some(blocks);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = sampler;
    Built {
        image,
        before,
        after,
        secs: t0.elapsed().as_secs_f64(),
        cached,
        psnr,
    }
}

/// Swaps finished images in, a few per frame, and refreshes the materials that use them.
pub fn finish(
    mut jobs: ResMut<MipJobs>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cloth: ResMut<Assets<super::cloth::ClothMaterial>>,
    mut foliage: ResMut<Assets<crate::objects::FoliageMaterial>>,
) {
    let MipJobs { running, ready, .. } = &mut *jobs;
    running.retain_mut(|(id, task)| match block_on(future::poll_once(task)) {
        Some(b) => {
            ready.push_back((*id, b));
            false
        }
        None => true,
    });
    let mut swapped = HashSet::new();
    let mut bytes = 0;
    while let Some((_, b)) = jobs.ready.front() {
        if !swapped.is_empty() && (bytes + b.after > SWAP_BYTES || swapped.len() >= SWAP_COUNT) {
            break;
        }
        let (id, b) = jobs.ready.pop_front().unwrap();
        bytes += b.after;
        if images.get(id).is_none() {
            continue;
        }
        jobs.stats.0 += 1;
        jobs.stats.1 += b.after.saturating_sub(b.before);
        jobs.stats.2 += b.secs;
        let s = &mut jobs.summary;
        s.0 += b.before;
        s.1 += b.after;
        if b.image.texture_descriptor.format.is_compressed() {
            s.2 += 1;
            s.3 += b.cached as usize;
        }
        s.4.extend(b.psnr);
        let _ = images.insert(id, b.image);
        swapped.insert(id);
    }
    if swapped.is_empty() {
        return;
    }
    jobs.spread.0 += 1;
    jobs.spread.1 = jobs.spread.1.max(bytes);
    let uses = |m: &StandardMaterial| {
        [
            &m.base_color_texture,
            &m.emissive_texture,
            &m.normal_map_texture,
            &m.metallic_roughness_texture,
            &m.occlusion_texture,
        ]
        .iter()
        .any(|h| h.as_ref().is_some_and(|h| swapped.contains(&h.id())))
    };
    let touch: Vec<_> = materials
        .iter()
        .filter(|(_, m)| uses(m))
        .map(|(id, _)| id)
        .collect();
    for id in touch {
        if let Some(m) = materials.get_mut(id) {
            m.into_inner();
        }
    }
    let touch: Vec<_> = cloth
        .iter()
        .filter(|(_, m)| uses(&m.base))
        .map(|(id, _)| id)
        .collect();
    for id in touch {
        if let Some(m) = cloth.get_mut(id) {
            m.into_inner();
        }
    }
    let touch: Vec<_> = foliage
        .iter()
        .filter(|(_, m)| uses(&m.base))
        .map(|(id, _)| id)
        .collect();
    for id in touch {
        if let Some(m) = foliage.get_mut(id) {
            m.into_inner();
        }
    }
    // one summary line whenever the queue runs dry
    let s = &jobs.summary;
    if jobs.ready.is_empty()
        && jobs.running.is_empty()
        && jobs.pending.is_empty()
        && jobs.stats.0 != jobs.logged
    {
        let mut p = s.4.clone();
        p.sort_by(f64::total_cmp);
        let pct = |q: f64| {
            p.get(((p.len() as f64 - 1.0) * q).round() as usize)
                .copied()
                .unwrap_or(0.0)
        };
        info!(
            "model textures: {} rebuilt ({} block-compressed, {} of those from the cache), VRAM {:.0} MB as loaded → {:.0} MB with mips; swapped in over {} frames (at most {:.1} MB a frame){}",
            jobs.stats.0,
            s.2,
            s.3,
            s.0 as f64 / 1e6,
            s.1 as f64 / 1e6,
            jobs.spread.0,
            jobs.spread.1 as f64 / 1e6,
            if p.is_empty() {
                String::new()
            } else {
                format!(
                    "; encoded {}: PSNR min {:.1} / median {:.1} dB",
                    p.len(),
                    pct(0.0),
                    pct(0.5)
                )
            }
        );
        jobs.logged = jobs.stats.0;
        jobs.spread = (0, 0);
    }
}

/// Trilinear + anisotropic where the model's sampler filters linearly anyway.
fn mip_sampler(sampler: &ImageSampler) -> ImageSampler {
    let mut s = match sampler {
        ImageSampler::Descriptor(d) => d.clone(),
        ImageSampler::Default => ImageSamplerDescriptor::linear(),
    };
    if s.mag_filter == ImageFilterMode::Linear && s.min_filter == ImageFilterMode::Linear {
        s.mipmap_filter = ImageFilterMode::Linear;
        s.anisotropy_clamp = 8;
    }
    ImageSampler::Descriptor(s)
}

/// The image with its full mip chain (2×2 box filter; sRGB colour averaged in linear light).
pub(crate) fn with_mips(mut img: Image) -> Image {
    let srgb = img.texture_descriptor.format == TextureFormat::Rgba8UnormSrgb;
    let (w, h) = (
        img.texture_descriptor.size.width as usize,
        img.texture_descriptor.size.height as usize,
    );
    let Some(mut data) = img.data.take() else {
        return img;
    };
    let to_lin: Vec<f32> = (0..256)
        .map(|v| Color::srgb_u8(v as u8, 0, 0).to_linear().red)
        .collect();
    // linear -> sRGB byte through a 4096-entry table
    let to_srgb: Vec<u8> = (0..4096)
        .map(|i| {
            (Color::linear_rgb(i as f32 / 4095.0, 0.0, 0.0)
                .to_srgba()
                .red
                * 255.0)
                .round() as u8
        })
        .collect();
    let mut level: Vec<u8> = data.clone();
    let (mut lw, mut lh) = (w, h);
    let mut levels = 1;
    while lw > 1 || lh > 1 {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                let (x0, y0) = (x * 2, y * 2);
                let (x1, y1) = ((x0 + 1).min(lw - 1), (y0 + 1).min(lh - 1));
                let px = |sx: usize, sy: usize| &level[(sy * lw + sx) * 4..][..4];
                let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                let o = &mut next[(y * nw + x) * 4..][..4];
                for k in 0..4 {
                    o[k] = if srgb && k < 3 {
                        let l = (to_lin[a[k] as usize]
                            + to_lin[b[k] as usize]
                            + to_lin[c[k] as usize]
                            + to_lin[d[k] as usize])
                            * 0.25;
                        to_srgb[(l * 4095.0).round() as usize]
                    } else {
                        ((a[k] as u32 + b[k] as u32 + c[k] as u32 + d[k] as u32 + 2) / 4) as u8
                    };
                }
            }
        }
        data.extend_from_slice(&next);
        level = next;
        (lw, lh) = (nw, nh);
        levels += 1;
    }
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = levels;
    img.asset_usage = RenderAssetUsages::RENDER_WORLD;
    img.sampler = mip_sampler(&img.sampler);
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_reaches_one_texel_and_averages_in_linear_light() {
        // a 4 × 2 sRGB checker of black and white
        let px: Vec<u8> = (0..8)
            .flat_map(|i| {
                if i % 2 == 0 {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                }
            })
            .collect();
        let img = Image::new(
            Extent3d {
                width: 4,
                height: 2,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            px,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        let m = with_mips(img);
        assert_eq!(m.texture_descriptor.mip_level_count, 3);
        let d = m.data.unwrap();
        assert_eq!(d.len(), (8 + 2 + 1) * 4);
        // half-white in linear light is sRGB ~188, not 128
        assert!((d[32] as i32 - 188).abs() <= 1, "{}", d[32]);
        assert_eq!(d[35], 255);
    }

    #[test]
    fn compressed_build_keeps_every_level() {
        let px: Vec<u8> = (0..64 * 32)
            .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 8) as u8, 90, 255])
            .collect();
        let img = Image::new(
            Extent3d {
                width: 64,
                height: 32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            px,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        let b = build(img, Some(Codec::Bc7 { alpha: false }));
        let d = &b.image.texture_descriptor;
        assert_eq!(
            (d.format, d.mip_level_count),
            (TextureFormat::Bc7RgbaUnormSrgb, 7)
        );
        let want: usize = (0..7)
            .map(|l| compress::level_bytes((64 >> l).max(1), (32 >> l).max(1)))
            .sum();
        assert_eq!(b.image.data.as_ref().unwrap().len(), want);
    }
}
