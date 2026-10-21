//! Far impostors for the exported trees: the tree drawn once from the front (orthographic, on the
//! CPU, alpha-tested leaves, gently shaded) onto two crossed quads, like the editor's own
//! billboards. It is each tree's last level of detail, so engines draw distant forests with four
//! triangles a tree all the way to the draw distance (Unity's terrain billboards don't apply to
//! LOD-group trees; Godot and Unreal have no shared impostor system).

use bevy::prelude::*;

use super::core::{Arrays, arrays};

/// Picture width (px); the height follows the tree's proportions (at most twice the width).
const WIDTH: u32 = 256;

pub struct Impostor {
    pub image: image::RgbaImage,
    pub pos: Vec<[f32; 3]>,
    pub nrm: Vec<[f32; 3]>,
    pub uv: Vec<[f32; 2]>,
    pub idx: Vec<u32>,
}

fn sample(img: &image::RgbaImage, u: f32, v: f32, wrap: bool) -> [u8; 4] {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (mut x, mut y) = (u * w, v * h);
    if wrap {
        x = x.rem_euclid(w);
        y = y.rem_euclid(h);
    }
    let (x, y) = (
        (x as u32).min(img.width() - 1),
        (y as u32).min(img.height() - 1),
    );
    img.get_pixel(x, y).0
}

/// Draws `parts` (mesh, its texture, the texture repeats, alpha-tested) into `out`; `depth` keeps
/// the nearest surface (largest z: the camera looks down −Z).
fn raster(
    out: &mut image::RgbaImage,
    depth: &mut [f32],
    a: &Arrays,
    tex: &image::RgbaImage,
    wrap: bool,
    cutout: bool,
    frame: (f32, f32, f32, f32),
) {
    let (x0, y0, sx, sy) = frame;
    let (w, h) = (out.width() as i32, out.height() as i32);
    let light = Vec3::new(0.3, 0.8, 1.0).normalize();
    let to_px = |p: [f32; 3]| Vec3::new((p[0] - x0) * sx, (y0 - p[1]) * sy, p[2]);
    for t in a.idx.chunks_exact(3) {
        let [i0, i1, i2] = [t[0] as usize, t[1] as usize, t[2] as usize];
        let (p0, p1, p2) = (to_px(a.pos[i0]), to_px(a.pos[i1]), to_px(a.pos[i2]));
        let area = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
        if area.abs() < 1e-6 {
            continue;
        }
        let (minx, maxx) = (
            p0.x.min(p1.x).min(p2.x).floor().max(0.0) as i32,
            p0.x.max(p1.x).max(p2.x).ceil().min(w as f32 - 1.0) as i32,
        );
        let (miny, maxy) = (
            p0.y.min(p1.y).min(p2.y).floor().max(0.0) as i32,
            p0.y.max(p1.y).max(p2.y).ceil().min(h as f32 - 1.0) as i32,
        );
        let uv = |i: usize| a.uv.get(i).copied().unwrap_or([0.0; 2]);
        let nrm = |i: usize| Vec3::from_array(a.nrm.get(i).copied().unwrap_or([0.0, 1.0, 0.0]));
        for py in miny..=maxy {
            for px in minx..=maxx {
                let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = ((p1.x - cx) * (p2.y - cy) - (p2.x - cx) * (p1.y - cy)) / area;
                let w1 = ((p2.x - cx) * (p0.y - cy) - (p0.x - cx) * (p2.y - cy)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = p0.z * w0 + p1.z * w1 + p2.z * w2;
                let k = (py * w + px) as usize;
                if z <= depth[k] {
                    continue;
                }
                let (u0, u1, u2) = (uv(i0), uv(i1), uv(i2));
                let c = sample(
                    tex,
                    u0[0] * w0 + u1[0] * w1 + u2[0] * w2,
                    u0[1] * w0 + u1[1] * w1 + u2[1] * w2,
                    wrap,
                );
                if cutout && c[3] < 128 {
                    continue;
                }
                // gentle form shading only: the engine lights the quads again
                let n = (nrm(i0) * w0 + nrm(i1) * w1 + nrm(i2) * w2).normalize_or(Vec3::Y);
                let shade = 0.72 + 0.28 * n.dot(light).abs();
                depth[k] = z;
                out.put_pixel(
                    px as u32,
                    py as u32,
                    image::Rgba([
                        (c[0] as f32 * shade) as u8,
                        (c[1] as f32 * shade) as u8,
                        (c[2] as f32 * shade) as u8,
                        255,
                    ]),
                );
            }
        }
    }
}

/// Fills transparent texels with their opaque neighbours' colour (no dark fringes once mipmapped).
fn bleed(img: &mut image::RgbaImage) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    for _ in 0..8 {
        let src = img.clone();
        for y in 0..h {
            for x in 0..w {
                if src.get_pixel(x as u32, y as u32).0[3] > 0 {
                    continue;
                }
                let (mut acc, mut n) = ([0u32; 3], 0);
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let p = src.get_pixel(nx as u32, ny as u32).0;
                    if p[3] > 0 || p[0] | p[1] | p[2] > 0 {
                        for c in 0..3 {
                            acc[c] += p[c] as u32;
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    img.put_pixel(
                        x as u32,
                        y as u32,
                        image::Rgba([
                            (acc[0] / n) as u8,
                            (acc[1] / n) as u8,
                            (acc[2] / n) as u8,
                            0,
                        ]),
                    );
                }
            }
        }
    }
}

/// The impostor of one tree variant from its finest level (`bark`, `leaves` with their
/// textures), `height` × `width` m standing on `min_y`. `None` if nothing was drawn.
pub fn build(
    bark: &Mesh,
    leaves: Option<&Mesh>,
    bark_tex: Option<&image::RgbaImage>,
    leaf_tex: &image::RgbaImage,
    height: f32,
    width: f32,
    min_y: f32,
) -> Option<Impostor> {
    let (w, h) = (
        WIDTH,
        ((WIDTH as f32 * height / width.max(0.1)).round() as u32).clamp(WIDTH / 2, WIDTH * 2),
    );
    let mut image = image::RgbaImage::new(w, h);
    let mut depth = vec![f32::MIN; (w * h) as usize];
    let frame = (
        -width * 0.5,
        min_y + height,
        w as f32 / width.max(0.1),
        h as f32 / height.max(0.1),
    );
    let b = arrays(bark);
    match bark_tex {
        Some(t) => raster(&mut image, &mut depth, &b, t, true, false, frame),
        None => raster(&mut image, &mut depth, &b, leaf_tex, false, true, frame),
    }
    if let Some(l) = leaves {
        raster(
            &mut image,
            &mut depth,
            &arrays(l),
            leaf_tex,
            false,
            true,
            frame,
        );
    }
    let opaque = image.pixels().filter(|p| p.0[3] > 0).count();
    if opaque < 64 {
        return None;
    }
    bleed(&mut image);
    // two crossed quads (x and z planes), normals tilted up so both shade alike in any sun
    let (hw, y0, y1) = (width * 0.5, min_y, min_y + height);
    let (mut pos, mut nrm, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
    for (axis, n) in [(Vec3::X, Vec3::Z), (Vec3::Z, -Vec3::X)] {
        let nn = (n * 0.35 + Vec3::Y).normalize().to_array();
        let base = pos.len() as u32;
        for (s, t) in [(-1.0, y1), (-1.0, y0), (1.0, y0), (1.0, y1)] {
            pos.push((axis * (s * hw) + Vec3::Y * t).to_array());
            nrm.push(nn);
            uv.push([
                if s < 0.0 { 0.0 } else { 1.0 },
                if t == y1 { 0.0 } else { 1.0 },
            ]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Some(Impostor {
        image,
        pos,
        nrm,
        uv,
        idx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oaks_get_a_picture() {
        let sp = crate::trees::export_species(&[crate::procgen::meshes::AssetKind::Oak])
            .pop()
            .unwrap();
        let (bark, leaves) = &sp.variants[0][0];
        let leaf = image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 120, 30, 255]));
        let barkt = image::RgbaImage::from_pixel(8, 8, image::Rgba([90, 70, 50, 255]));
        // the tree's own frame, as the library gives it
        let pts: Vec<[f32; 3]> = arrays(bark)
            .pos
            .into_iter()
            .chain(leaves.as_ref().map(|l| arrays(l).pos).unwrap_or_default())
            .collect();
        let (lo, hi) = pts.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), p| {
            (l.min(Vec3::from_array(*p)), h.max(Vec3::from_array(*p)))
        });
        let width = (hi.x.abs().max(lo.x.abs()) * 2.0).max(hi.z.abs().max(lo.z.abs()) * 2.0);
        let imp = build(
            bark,
            leaves.as_ref(),
            Some(&barkt),
            &leaf,
            hi.y - lo.y,
            width,
            lo.y,
        )
        .unwrap();
        let covered = imp.image.pixels().filter(|p| p.0[3] > 0).count() as f32
            / (imp.image.width() * imp.image.height()) as f32;
        assert!(covered > 0.2 && covered < 0.95, "{covered}");
        assert_eq!(imp.idx.len(), 12);
    }
}
