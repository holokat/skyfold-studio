//! Writing a generated map for a game: the full PNG, a clean copy without lettering, a preview,
//! the fog-of-war mask, a 256 px minimap tile pyramid and `map.json`.

use std::fs;
use std::io::BufWriter;
use std::path::Path;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use rayon::prelude::*;

use super::render::MapResult;

pub const TILE: u32 = 256;

pub fn write_png(
    path: &Path,
    data: &[u8],
    w: u32,
    h: u32,
    color: ExtendedColorType,
) -> Result<(), String> {
    let f = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    PngEncoder::new_with_quality(
        BufWriter::new(f),
        CompressionType::Fast,
        FilterType::Adaptive,
    )
    .write_image(data, w, h, color)
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// RGB from RGBA (the maps are opaque; RGB files are a quarter smaller).
pub fn rgb(data: &[u8]) -> Vec<u8> {
    data.chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

/// Half-size RGBA (2×2 box filter).
pub fn halve(src: &[u8], size: u32) -> Vec<u8> {
    let n = (size / 2) as usize;
    let s = size as usize;
    let mut out = vec![0u8; n * n * 4];
    out.par_chunks_mut(n * 4).enumerate().for_each(|(j, row)| {
        for i in 0..n {
            for c in 0..4 {
                let a = src[((2 * j) * s + 2 * i) * 4 + c] as u32;
                let b = src[((2 * j) * s + 2 * i + 1) * 4 + c] as u32;
                let d = src[((2 * j + 1) * s + 2 * i) * 4 + c] as u32;
                let e = src[((2 * j + 1) * s + 2 * i + 1) * 4 + c] as u32;
                row[i * 4 + c] = ((a + b + d + e + 2) / 4) as u8;
            }
        }
    });
    out
}

/// The map scaled down to at most `max` px (power-of-two steps), as RGBA.
pub fn downscaled(r: &MapResult, max: u32) -> (Vec<u8>, u32) {
    let mut img = r.rgba.clone();
    let mut size = r.size;
    while size > max {
        img = halve(&img, size);
        size /= 2;
    }
    (img, size)
}

/// PNG bytes of the map at most `max` px wide (agent previews).
pub fn preview_png(r: &MapResult, max: u32) -> Result<Vec<u8>, String> {
    let (img, size) = downscaled(r, max);
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&rgb(&img), size, size, ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// PNG bytes of an RGB image scaled to at most `max` px on its long side.
pub fn preview_rgb(rgb: &[u8], w: u32, h: u32, max: u32) -> Result<Vec<u8>, String> {
    let img = image::RgbImage::from_raw(w, h, rgb.to_vec()).ok_or("preview buffer")?;
    let k = (max as f32 / w.max(h) as f32).min(1.0);
    let (pw, ph) = (
        ((w as f32 * k).round() as u32).max(1),
        ((h as f32 * k).round() as u32).max(1),
    );
    let small = if k < 1.0 {
        image::imageops::resize(&img, pw, ph, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(small.as_raw(), pw, ph, ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(bytes)
}

pub struct Exported {
    pub files: usize,
    pub bytes: u64,
    pub main: std::path::PathBuf,
}

/// Writes everything into `dir`.
pub fn export(dir: &Path, r: &MapResult) -> Result<Exported, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let style = r.settings.style.id();
    let main = dir.join(format!("map_{style}.png"));
    let clean = dir.join(format!("map_{style}_clean.png"));
    let mut files = 0usize;
    // the two big images in parallel
    let (a, b) = rayon::join(
        || {
            write_png(
                &main,
                &rgb(&r.rgba),
                r.size,
                r.size,
                ExtendedColorType::Rgb8,
            )
        },
        || {
            write_png(
                &clean,
                &rgb(&r.clean),
                r.size,
                r.size,
                ExtendedColorType::Rgb8,
            )
        },
    );
    a?;
    b?;
    files += 2;
    let (pv, ps) = downscaled(r, 1024);
    write_png(
        &dir.join("map_preview.png"),
        &rgb(&pv),
        ps,
        ps,
        ExtendedColorType::Rgb8,
    )?;
    write_png(
        &dir.join("fog_mask.png"),
        &r.fog,
        r.fog_size,
        r.fog_size,
        ExtendedColorType::L8,
    )?;
    files += 2;

    // minimap tiles from the clean map: z = 0 is the whole world in one tile
    let tiles_dir = dir.join("tiles");
    let _ = fs::remove_dir_all(&tiles_dir);
    let zmax = (r.size / TILE).max(1).trailing_zeros();
    let mut level = r.clean.clone();
    let mut size = r.size;
    let mut levels = Vec::new();
    for z in (0..=zmax).rev() {
        let per = size / TILE;
        let zdir = tiles_dir.join(z.to_string());
        fs::create_dir_all(&zdir).map_err(|e| e.to_string())?;
        let jobs: Vec<(u32, u32)> = (0..per)
            .flat_map(|y| (0..per).map(move |x| (x, y)))
            .collect();
        let lv = &level;
        jobs.par_iter().try_for_each(|(x, y)| {
            let mut px = Vec::with_capacity((TILE * TILE * 3) as usize);
            for row in 0..TILE {
                let start = (((y * TILE + row) * size + x * TILE) * 4) as usize;
                for p in lv[start..start + (TILE * 4) as usize].chunks_exact(4) {
                    px.extend_from_slice(&p[..3]);
                }
            }
            write_png(
                &zdir.join(format!("{x}_{y}.png")),
                &px,
                TILE,
                TILE,
                ExtendedColorType::Rgb8,
            )
        })?;
        files += jobs.len();
        levels.push(serde_json::json!({"z": z, "tiles_per_side": per, "px_per_m": size as f32 / r.world_size}));
        if z > 0 {
            level = halve(&level, size);
            size /= 2;
        }
    }
    levels.reverse();

    let mut meta = r.meta.clone();
    meta["files"] = serde_json::json!({
        "map": main.file_name().map(|f| f.to_string_lossy().to_string()),
        "map_clean": clean.file_name().map(|f| f.to_string_lossy().to_string()),
        "preview": "map_preview.png",
        "fog_mask": "fog_mask.png",
    });
    meta["minimap_tiles"] = serde_json::json!({
        "path": "tiles/{z}/{x}_{y}.png",
        "tile_px": TILE,
        "levels": levels,
        "note": "z 0 covers the whole world in one tile; each level doubles the tiles per side. Tiles come from the clean map (no lettering): draw icons and names from this file.",
    });
    meta["generated"] = serde_json::json!({"millis": r.millis});
    fs::write(
        dir.join("map.json"),
        serde_json::to_string_pretty(&meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    files += 1;
    let bytes = dir_size(dir);
    Ok(Exported { files, bytes, main })
}

fn dir_size(p: &Path) -> u64 {
    fs::read_dir(p)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_size(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}
