//! GPU block compression for textures that arrive as plain RGBA8 (glTF model textures, the terrain
//! layer arrays): BC7 for colour and packed data maps, BC5 for two-channel normal maps, every mip
//! level, at one byte per texel instead of four (the full chain is ~1.4 MB for a 1024² texture
//! instead of ~5.6 MB) and a quarter of the memory traffic when sampled.
//!
//! Encoding is pure Rust (`block_compression`, a port of Intel's ISPC kernels) on the CPU, so the
//! results are cached in the user's data folder, keyed by the pixels themselves (shared
//! textures of different models share one entry; an edited texture gets a new one) and `VERSION`.
//!
//! BC7 is encoded in two passes: every block with mode 6 (one partition, 7-bit endpoints: very
//! fast and good on most photo and painted content), then the blocks it fits worst (sharp
//! two-colour edges of painted textures, noisy scans) again with the partitioned modes, keeping
//! whichever is closer. `Effort::Fast` (model textures, ~0.1–0.4 s per 1024² on one core) re-does
//! the worst ~12 %; `Effort::High` / `Max` (the few terrain layers) re-do every block.

use std::borrow::Cow;
use std::path::PathBuf;

use bevy::render::render_resource::TextureFormat;
use block_compression::decode::decompress_blocks_as_rgba8;
use block_compression::encode::compress_rgba8;
use block_compression::{BC7Settings, CompressionVariant};

/// Bump when the encoder, its settings or the mip filter change: every cached texture is rebuilt.
pub const VERSION: u64 = 1;

/// How a texture is block-compressed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Codec {
    /// RGB colour or packed data (+ alpha when `alpha`), 1 byte per texel.
    Bc7 { alpha: bool },
    /// Two channels (normal map XY; the shader rebuilds Z), 1 byte per texel.
    Bc5,
}

/// How hard the encoder tries. `High` and `Max` split each level into strips encoded in
/// parallel on the current rayon pool (the terrain arrays: few textures, built once).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Effort {
    /// mode 6, then the worst ~12 % of blocks with the partitioned modes (model textures)
    Fast,
    /// every block also with the partitioned modes (`very_fast` search)
    High,
    /// `High` that also tries each channel as the separately coded one in modes 4 and 5 (four
    /// uncorrelated data channels, e.g. normal XY + roughness + AO: ~4 dB closer normals; as good
    /// as the `basic` search there at half its cost)
    Max,
}

/// Mode-6 blocks with a squared error above this (summed over the block's 16 texels and 3–4
/// channels: an RMS error of ~2.5 levels) are re-encoded with the partitioned modes.
const REDO_SSE: u32 = 300;
/// `Effort::Fast` re-encodes at most this share of a level's blocks (the worst first).
const FAST_REDO_SHARE: f32 = 0.12;

impl Codec {
    pub fn format(self, srgb: bool) -> TextureFormat {
        match self {
            Codec::Bc7 { .. } if srgb => TextureFormat::Bc7RgbaUnormSrgb,
            Codec::Bc7 { .. } => TextureFormat::Bc7RgbaUnorm,
            Codec::Bc5 => TextureFormat::Bc5RgUnorm,
        }
    }

    /// Channels the codec keeps (for error measures).
    pub fn channels(self) -> &'static [usize] {
        match self {
            Codec::Bc7 { alpha: true } => &[0, 1, 2, 3],
            Codec::Bc7 { alpha: false } => &[0, 1, 2],
            Codec::Bc5 => &[0, 1],
        }
    }

    /// The first pass (`None`) or the second pass for an effort.
    fn variant(self, effort: Option<Effort>) -> CompressionVariant {
        let s = match (self, effort) {
            (Codec::Bc5, _) => return CompressionVariant::BC5,
            (Codec::Bc7 { alpha: false }, None) => BC7Settings::opaque_ultra_fast(),
            (Codec::Bc7 { alpha: false }, Some(Effort::Max)) => BC7Settings::opaque_basic(),
            (Codec::Bc7 { alpha: false }, _) => BC7Settings::opaque_very_fast(),
            (Codec::Bc7 { alpha: true }, None) => BC7Settings::alpha_ultrafast(),
            (Codec::Bc7 { alpha: true }, Some(Effort::Max)) => alpha_rotations(),
            (Codec::Bc7 { alpha: true }, _) => BC7Settings::alpha_very_fast(),
        };
        CompressionVariant::BC7(s)
    }
}

/// `alpha_very_fast` searching the mode 4 / 5 channel rotation too (`BC7Settings` keeps its fields
/// private; it is plain `u32`s: index 16 is `mode45_channel0`, 3 = alpha only, 0 = all four).
fn alpha_rotations() -> BC7Settings {
    let mut s: [u32; 19] = bytemuck::cast(BC7Settings::alpha_very_fast());
    s[16] = 0;
    bytemuck::cast(s)
}

/// Bytes of a compressed level (4×4 blocks of 16 bytes; levels under 4 texels take one block row).
pub fn level_bytes(w: u32, h: u32) -> usize {
    (w.div_ceil(4) * h.div_ceil(4) * 16) as usize
}

/// The level, padded to whole 4×4 blocks by repeating its last row / column.
fn padded(px: &[u8], w: usize, h: usize) -> (Cow<'_, [u8]>, usize, usize) {
    let (pw, ph) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
    if pw == w && ph == h {
        return (Cow::Borrowed(px), w, h);
    }
    let mut out = vec![0u8; pw * ph * 4];
    for y in 0..ph {
        for x in 0..pw {
            let s = (y.min(h - 1) * w + x.min(w - 1)) * 4;
            out[(y * pw + x) * 4..][..4].copy_from_slice(&px[s..s + 4]);
        }
    }
    (Cow::Owned(out), pw, ph)
}

/// Squared error of one encoded block against the source texels at block (bx, by).
fn block_sse(
    px: &[u8],
    stride: usize,
    bx: usize,
    by: usize,
    block: &[u8],
    variant: CompressionVariant,
    channels: &[usize],
) -> u32 {
    let mut dec = [0u8; 64];
    decompress_blocks_as_rgba8(variant, 4, 4, block, &mut dec);
    let mut s = 0u32;
    for r in 0..4 {
        for c in 0..4 {
            let src = &px[(by * 4 + r) * stride + (bx * 4 + c) * 4..][..4];
            let d = &dec[(r * 4 + c) * 4..][..4];
            for &k in channels {
                let e = src[k] as i32 - d[k] as i32;
                s += (e * e) as u32;
            }
        }
    }
    s
}

/// One mip level (RGBA8, `w`×`h`) as blocks.
pub fn encode_level(px: &[u8], w: u32, h: u32, codec: Codec, effort: Effort) -> Vec<u8> {
    let (src, pw, ph) = padded(px, w as usize, h as usize);
    let stride = pw * 4;
    let row_bytes = pw / 4 * 16;
    let mut out = vec![0u8; row_bytes * (ph / 4)];
    if effort == Effort::Fast {
        encode_strip(&src, stride, 0, &mut out, codec, effort);
    } else {
        use rayon::prelude::*;
        // strips of 8 block rows: enough of them to keep every thread busy on one texture
        out.par_chunks_mut(row_bytes * 8)
            .enumerate()
            .for_each(|(i, strip)| encode_strip(&src, stride, i * 8, strip, codec, effort));
    }
    out
}

/// Encodes the block rows from `row0` on into `out` (whole rows).
fn encode_strip(
    src: &[u8],
    stride: usize,
    row0: usize,
    out: &mut [u8],
    codec: Codec,
    effort: Effort,
) {
    let bw = stride / 16;
    let rows = out.len() / (bw * 16);
    let first = codec.variant(None);
    let base = row0 * 4 * stride;
    compress_rgba8(
        first,
        &src[base..],
        out,
        (bw * 4) as u32,
        (rows * 4) as u32,
        stride as u32,
    );
    if codec == Codec::Bc5 {
        return;
    }
    let channels = codec.channels();
    let mut worst: Vec<(u32, usize)> = (0..bw * rows)
        .map(|i| {
            (
                block_sse(
                    src,
                    stride,
                    i % bw,
                    row0 + i / bw,
                    &out[i * 16..i * 16 + 16],
                    first,
                    channels,
                ),
                i,
            )
        })
        .filter(|&(e, _)| effort != Effort::Fast || e > REDO_SSE)
        .collect();
    if effort == Effort::Fast {
        let keep = ((bw * rows) as f32 * FAST_REDO_SHARE).ceil() as usize;
        if worst.len() > keep {
            worst.select_nth_unstable_by(keep, |a, b| b.0.cmp(&a.0));
            worst.truncate(keep);
        }
    }
    let good = codec.variant(Some(effort));
    for (err, i) in worst {
        let (bx, by) = (i % bw, row0 + i / bw);
        let mut block = [0u8; 16];
        compress_rgba8(
            good,
            &src[by * 4 * stride + bx * 16..],
            &mut block,
            4,
            4,
            stride as u32,
        );
        if block_sse(src, stride, bx, by, &block, good, channels) < err {
            out[i * 16..i * 16 + 16].copy_from_slice(&block);
        }
    }
}

/// A whole RGBA8 mip chain (`levels` levels of `w`×`h` halving, one after another) as blocks.
pub fn encode_chain(
    chain: &[u8],
    w: u32,
    h: u32,
    levels: u32,
    codec: Codec,
    effort: Effort,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(chain.len() / 4 + 64);
    let mut o = 0;
    for l in 0..levels {
        let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
        let n = (lw * lh * 4) as usize;
        out.extend(encode_level(&chain[o..o + n], lw, lh, codec, effort));
        o += n;
    }
    out
}

/// Decodes one level back to RGBA8 (error measures, tests).
pub fn decode_level(blocks: &[u8], w: u32, h: u32, codec: Codec) -> Vec<u8> {
    let (pw, ph) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
    let mut px = vec![0u8; (pw * ph * 4) as usize];
    decompress_blocks_as_rgba8(codec.variant(None), pw, ph, blocks, &mut px);
    if pw == w && ph == h {
        return px;
    }
    (0..h as usize)
        .flat_map(|y| px[y * pw as usize * 4..][..w as usize * 4].to_vec())
        .collect()
}

/// Peak signal-to-noise ratio (dB) of `b` against `a` (RGBA8) over the given channels.
pub fn psnr(a: &[u8], b: &[u8], channels: &[usize]) -> f64 {
    let (mut se, mut n) = (0f64, 0f64);
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for &c in channels {
            let d = pa[c] as f64 - pb[c] as f64;
            se += d * d;
            n += 1.0;
        }
    }
    if se == 0.0 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 * n / se).log10()
    }
}

/// A fast 64-bit hash of a texture's bytes (cache keys).
pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = 0x243f_6a88_85a3_08d3u64 ^ bytes.len() as u64;
    let mut words = bytes.chunks_exact(8);
    for w in &mut words {
        h = (h ^ u64::from_le_bytes(w.try_into().unwrap())).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        h ^= h >> 29;
    }
    for &b in words.remainder() {
        h = (h ^ b as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    h ^ (h >> 32)
}

/// The cache key of a texture: its pixels' hash, size, codec, colour space, effort and `VERSION`.
pub fn cache_key(content: u64, w: u32, h: u32, codec: Codec, srgb: bool, effort: Effort) -> u64 {
    let mut k = content;
    let tag = match codec {
        Codec::Bc7 { alpha } => 1 + alpha as u64,
        Codec::Bc5 => 3,
    };
    for v in [w as u64, h as u64, tag, srgb as u64, effort as u64, VERSION] {
        k = (k ^ v).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(23);
    }
    k
}

pub fn cache_dir() -> PathBuf {
    crate::paths::cache_dir("textures")
}

/// `WBTEX1`, then w, h, levels (u32) and the blocks of every level.
pub fn read_cache(key: u64, w: u32, h: u32, levels: u32) -> Option<Vec<u8>> {
    let b = std::fs::read(cache_dir().join(format!("{key:016x}.bc"))).ok()?;
    let u = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    let want: usize = (0..levels)
        .map(|l| level_bytes((w >> l).max(1), (h >> l).max(1)))
        .sum();
    (b.get(..6)? == b"WBTEX1"
        && u(6)? == w
        && u(10)? == h
        && u(14)? == levels
        && b.len() == 18 + want)
        .then(|| b[18..].to_vec())
}

pub fn write_cache(key: u64, w: u32, h: u32, levels: u32, blocks: &[u8]) {
    // (unit tests never write into the assets folder)
    if cfg!(test) {
        return;
    }
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut b = Vec::with_capacity(blocks.len() + 18);
    b.extend_from_slice(b"WBTEX1");
    for v in [w, h, levels] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(blocks);
    // (unique temp name: two jobs may write the same shared texture at once)
    let tmp = dir.join(format!(
        "{key:016x}.{:x}.tmp",
        std::process::id() as u64 ^ (blocks.as_ptr() as u64)
    ));
    if std::fs::write(&tmp, b).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join(format!("{key:016x}.bc")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x + y) * 127 / (w + h)) as u8,
                    255,
                ]
            })
            .collect()
    }

    #[test]
    fn small_levels_pad_to_one_block_and_decode_back() {
        let px = pattern(2, 2);
        for codec in [Codec::Bc7 { alpha: false }, Codec::Bc5] {
            let b = encode_level(&px, 2, 2, codec, Effort::Fast);
            assert_eq!(b.len(), 16);
            assert_eq!(decode_level(&b, 2, 2, codec).len(), 16);
        }
    }

    #[test]
    fn smooth_gradients_survive_bc7_and_bc5() {
        let px = pattern(64, 64);
        let b7 = encode_level(&px, 64, 64, Codec::Bc7 { alpha: false }, Effort::Fast);
        assert!(
            psnr(
                &px,
                &decode_level(&b7, 64, 64, Codec::Bc7 { alpha: false }),
                &[0, 1, 2]
            ) > 40.0
        );
        let b5 = encode_level(&px, 64, 64, Codec::Bc5, Effort::Fast);
        assert!(psnr(&px, &decode_level(&b5, 64, 64, Codec::Bc5), &[0, 1]) > 40.0);
    }

    #[test]
    fn rotation_setting_is_where_we_think() {
        // alpha_very_fast codes only alpha separately, alpha_basic tries every channel
        let vf: [u32; 19] = bytemuck::cast(BC7Settings::alpha_very_fast());
        let basic: [u32; 19] = bytemuck::cast(BC7Settings::alpha_basic());
        assert_eq!((vf[16], basic[16], vf[18]), (3, 0, 4));
    }

    #[test]
    fn chain_size_matches_the_gpu_layout() {
        let mut chain = vec![];
        for l in 0..4 {
            chain.extend(pattern(8 >> l, 8 >> l));
        }
        let b = encode_chain(&chain, 8, 8, 4, Codec::Bc7 { alpha: true }, Effort::High);
        assert_eq!(
            b.len(),
            level_bytes(8, 8) + level_bytes(4, 4) + level_bytes(2, 2) + level_bytes(1, 1)
        );
    }
}
