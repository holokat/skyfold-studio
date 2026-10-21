//! Images of the reference map: the picture laid on the terrain, the analysis laid on the
//! terrain (regions in their own colours with borders, water in blue, rivers in cyan, ignored
//! areas hatched), and the labelled analysis preview returned to agents.

use bevy::math::Vec2;
use rayon::prelude::*;

use super::analyse::{Analysis, LAKE, NONE, RIVER, SEA};
use super::{Picture, Placement};
use crate::worldmap::canvas::{self, Align, Canvas, Face, Fonts, TextStyle};

/// A distinct, map-friendly colour for region `i` (golden-angle hues).
pub fn region_color(i: usize) -> [u8; 3] {
    let h = (i as f32 * 137.508 + 35.0) % 360.0;
    let (s, v) = (0.55, 0.92);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ]
}

/// Texel centre → world position for an overlay of `res`² over a world of `size` m.
fn texel_world(i: usize, j: usize, res: u32, size: f32) -> [f32; 2] {
    let t = size / res as f32;
    [
        (i as f32 + 0.5) * t - size * 0.5,
        (j as f32 + 0.5) * t - size * 0.5,
    ]
}

/// The picture on the terrain (RGBA texels, row 0 at −z).
pub fn picture_overlay(pic: &Picture, place: &Placement, size: f32, res: u32) -> Vec<u8> {
    let n = res as usize;
    let (w, h) = (pic.small.width() as usize, pic.small.height() as usize);
    let px = pic.small.as_raw();
    let aspect = pic.aspect();
    // soft edge over about one texel
    let edge_u = size / res as f32 / place.width;
    let edge_v = edge_u / aspect;
    let mut out = vec![0u8; n * n * 4];
    out.par_chunks_mut(n * 4).enumerate().for_each(|(j, row)| {
        for i in 0..n {
            let uv = place.to_uv(texel_world(i, j, res, size), aspect);
            let a = (uv[0].min(1.0 - uv[0]) / edge_u)
                .min(uv[1].min(1.0 - uv[1]) / edge_v)
                .clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let x = (uv[0] * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let y = (uv[1] * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
            let (x0, y0) = (x as usize, y as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let o = &mut row[i * 4..i * 4 + 4];
            for c in 0..3 {
                let p = |xx: usize, yy: usize| px[(yy * w + xx) * 3 + c] as f32;
                let top = p(x0, y0) + (p(x1, y0) - p(x0, y0)) * fx;
                let bot = p(x0, y1) + (p(x1, y1) - p(x0, y1)) * fx;
                o[c] = (top + (bot - top) * fy).round() as u8;
            }
            o[3] = (a * 255.0) as u8;
        }
    });
    out
}

fn rgba_image(w: u32, h: u32, data: Vec<u8>) -> bevy::image::Image {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
    bevy::image::Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
}

/// The picture as a texture (for the sheet over open water).
pub fn picture_image(pic: &Picture) -> bevy::image::Image {
    let (w, h) = (pic.small.width(), pic.small.height());
    let data = pic
        .small
        .as_raw()
        .chunks_exact(3)
        .flat_map(|c| [c[0], c[1], c[2], 255])
        .collect();
    rgba_image(w, h, data)
}

/// The analysis on the picture's grid as a texture, coloured like the terrain overlay.
pub fn analysis_image(
    an: &Analysis,
    ignored: &[bool],
    selected: Option<usize>,
) -> bevy::image::Image {
    let (w, h) = (an.w, an.h);
    let mut data = vec![0u8; w * h * 4];
    data.par_chunks_mut(4).enumerate().for_each(|(k, o)| {
        let mut c = cell_color(an, k, selected);
        if is_edge(an, k % w, k / w) && an.class[k] != SEA {
            c = [c[0] / 3, c[1] / 3, c[2] / 3, 255];
        }
        if ignored.get(k).copied().unwrap_or(false) && ((k % w + k / w) / 3) % 3 == 0 {
            c = [210, 210, 210, 230];
        }
        o.copy_from_slice(&c);
    });
    rgba_image(w as u32, h as u32, data)
}

/// Colour (RGBA) of an analysis cell; `edge` when a neighbour differs.
fn cell_color(an: &Analysis, k: usize, selected: Option<usize>) -> [u8; 4] {
    match an.class[k] {
        SEA => [24, 60, 130, 120],
        LAKE => [70, 150, 235, 235],
        RIVER => [60, 220, 255, 255],
        _ => {
            let r = an.region[k];
            if r == NONE {
                return [200, 200, 200, 200];
            }
            let c = region_color(r as usize);
            match selected {
                Some(s) if s == r as usize => [
                    c[0].saturating_add(40),
                    c[1].saturating_add(40),
                    c[2].saturating_add(40),
                    255,
                ],
                Some(_) => [c[0], c[1], c[2], 170],
                None => [c[0], c[1], c[2], 225],
            }
        }
    }
}

fn is_edge(an: &Analysis, x: usize, y: usize) -> bool {
    let k = y * an.w + x;
    let key = |k: usize| {
        if an.class[k] == super::analyse::LAND {
            an.region[k] as u64
        } else {
            1 << 40 | an.class[k] as u64
        }
    };
    let me = key(k);
    (x > 0 && key(k - 1) != me)
        || (x + 1 < an.w && key(k + 1) != me)
        || (y > 0 && key(k - an.w) != me)
        || (y + 1 < an.h && key(k + an.w) != me)
}

/// Is `p` inside the polygon (crossing number)?
pub fn in_poly(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            c = !c;
        }
    }
    c
}

/// The analysis on the terrain (RGBA texels, row 0 at −z).
pub fn analysis_overlay(
    an: &Analysis,
    ignored: &[bool],
    place: &Placement,
    aspect: f32,
    selected: Option<usize>,
    size: f32,
    res: u32,
) -> Vec<u8> {
    let n = res as usize;
    let (w, h) = (an.w, an.h);
    let edges: Vec<bool> = (0..w * h).map(|k| is_edge(an, k % w, k / w)).collect();
    let mut out = vec![0u8; n * n * 4];
    out.par_chunks_mut(n * 4).enumerate().for_each(|(j, row)| {
        for i in 0..n {
            let uv = place.to_uv(texel_world(i, j, res, size), aspect);
            if !(0.0..1.0).contains(&uv[0]) || !(0.0..1.0).contains(&uv[1]) {
                continue;
            }
            let (x, y) = (
                ((uv[0] * w as f32) as usize).min(w - 1),
                ((uv[1] * h as f32) as usize).min(h - 1),
            );
            let k = y * w + x;
            let mut c = cell_color(an, k, selected);
            if edges[k] && an.class[k] != SEA {
                c = [c[0] / 3, c[1] / 3, c[2] / 3, 255];
            }
            // ignored areas (the person's and found legend panels): grey hatching
            if ignored.get(k).copied().unwrap_or(false) && ((i + j) / 3) % 3 == 0 {
                c = [210, 210, 210, 230];
            }
            row[i * 4..i * 4 + 4].copy_from_slice(&c);
        }
    });
    out
}

/// The labelled analysis preview (PNG): the picture, regions tinted in their colours with
/// outlines, water, rivers, ignored areas, and each region's number, name, biome and role.
pub fn preview_png(
    an: &Analysis,
    pic: &Picture,
    ignored: &[bool],
    max_px: u32,
) -> Result<Vec<u8>, String> {
    let k = (max_px as f32 / pic.w.max(pic.h) as f32).min(1.0);
    let (pw, ph) = (
        ((pic.w as f32 * k) as u32).max(16),
        ((pic.h as f32 * k) as u32).max(16),
    );
    let photo = image::imageops::resize(&pic.small, pw, ph, image::imageops::FilterType::Triangle);
    let (w, h) = (an.w, an.h);
    let mut img = image::RgbaImage::new(pw, ph);
    for y in 0..ph {
        for x in 0..pw {
            let (cx, cy) = (
                ((x as f32 + 0.5) / pw as f32 * w as f32) as usize,
                ((y as f32 + 0.5) / ph as f32 * h as f32) as usize,
            );
            let kk = cy.min(h - 1) * w + cx.min(w - 1);
            let p = photo.get_pixel(x, y).0;
            // the picture, darkened and desaturated a little so the colours read
            let grey = (p[0] as f32 * 0.3 + p[1] as f32 * 0.59 + p[2] as f32 * 0.11) * 0.55;
            let base = [
                grey + p[0] as f32 * 0.15,
                grey + p[1] as f32 * 0.15,
                grey + p[2] as f32 * 0.15,
            ];
            let c = cell_color(an, kk, None);
            let a = c[3] as f32 / 255.0 * 0.62;
            let mut o = [0u8; 4];
            for i in 0..3 {
                o[i] = (base[i] * (1.0 - a) + c[i] as f32 * a).min(255.0) as u8;
            }
            o[3] = 255;
            if ignored.get(kk).copied().unwrap_or(false) && ((x + y) / 3) % 3 == 0 {
                o = [200, 200, 200, 255];
            }
            img.put_pixel(x, y, image::Rgba(o));
        }
    }
    // outlines, rivers and labels with the map canvas
    let pm = tiny_skia::Pixmap::from_vec(
        img.into_raw(),
        tiny_skia::IntSize::from_wh(pw, ph).ok_or("bad preview size")?,
    )
    .ok_or("bad preview")?;
    let mut cv = Canvas {
        pm,
        u: pw.max(ph) as f32 / 2048.0,
    };
    let s = |p: [f32; 2]| Vec2::new(p[0] * pw as f32, p[1] * ph as f32);
    for r in &an.regions {
        for lp in &r.outline {
            let pts: Vec<Vec2> = lp.iter().map(|q| s(*q)).collect();
            if let Some(path) = canvas::poly_path(&pts, true) {
                cv.stroke(&path, 1.4, [20, 20, 24, 200]);
            }
        }
    }
    for rv in &an.rivers {
        let pts: Vec<Vec2> = rv
            .pts
            .iter()
            .map(|q| s([q[0] / w as f32, q[1] / h as f32]))
            .collect();
        if let Some(path) = canvas::poly_path(&pts, false) {
            cv.stroke(&path, 2.2, [70, 230, 255, 255]);
        }
    }
    if let Some(fonts) = Fonts::get() {
        let size = (pw.max(ph) as f32 / 70.0).clamp(10.0, 22.0);
        let st = TextStyle::new(Face::SansBold, size, [255, 255, 255, 255])
            .halo([10, 12, 16, 230], size * 0.32);
        let st2 = TextStyle::new(Face::Sans, size * 0.78, [225, 230, 238, 255])
            .halo([10, 12, 16, 230], size * 0.3);
        let mut taken: Vec<canvas::Bx> = vec![];
        for r in &an.regions {
            let (t1, t2) = (
                format!("{} {}", r.id, r.name),
                format!(
                    "{} · {}",
                    crate::refmap::ui::biome_label(&r.biome),
                    r.role.name()
                ),
            );
            // the first free spot near the region's pole
            let spot = [0.0, 1.0, -1.0, 2.0, -2.0]
                .iter()
                .map(|k| s(r.pole) + Vec2::new(0.0, k * size * 2.0))
                .find(|p| {
                    let b = canvas::label_box(
                        fonts,
                        &st,
                        &t1,
                        *p - Vec2::new(0.0, size * 0.45),
                        Align::Centre,
                    )
                    .union(&canvas::label_box(
                        fonts,
                        &st2,
                        &t2,
                        *p + Vec2::new(0.0, size * 0.65),
                        Align::Centre,
                    ));
                    !taken.iter().any(|o| o.hits(&b))
                });
            let Some(p) = spot else { continue };
            let b1 = cv.text(
                fonts,
                &st,
                &t1,
                p - Vec2::new(0.0, size * 0.45),
                Align::Centre,
            );
            let b2 = cv.text(
                fonts,
                &st2,
                &t2,
                p + Vec2::new(0.0, size * 0.65),
                Align::Centre,
            );
            taken.push(b1.union(&b2));
        }
    }
    let rgba = cv.pm.data().to_vec();
    let mut buf = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_raw(pw, ph, rgba)
        .ok_or("bad preview buffer")?
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}
