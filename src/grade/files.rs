//! LUT files: reading `.cube` files, LUT strips (Unreal, Unity, Godot) and HALD images, writing
//! `.cube` files, and the "grade it in another app" round-trip (a screenshot with a neutral LUT
//! strip along its bottom edge: whatever the photo app does to the picture it also does to the
//! strip, which then reads back as the LUT).
//!
//! Every LUT read here takes and gives sRGB-encoded display colour (Rec.709 primaries), what
//! Photoshop, Resolve, Lightroom and the engines' LUT strips assume.

use std::path::Path;

use super::Lut3;

/// Cells per side of the round-trip strip (32 slices of 32 × 32: a 1024 × 32 strip).
pub const STRIP_SIZE: u32 = 32;

/// Reads a `.cube`, `.png`, `.tif(f)` or `.jpg` LUT.
pub fn load(path: &Path) -> Result<Lut3, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "cube" => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("could not read {}: {e}", path.display()))?;
            parse_cube(&text)
        }
        "png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" => {
            let img = image::open(path)
                .map_err(|e| format!("could not open {}: {e}", path.display()))?;
            from_image(&img.into_rgb32f())
        }
        _ => Err("a LUT is a .cube file or a LUT image (.png)".into()),
    }
}

/// A `.cube` file (Adobe / Resolve): `LUT_3D_SIZE`, optional `LUT_1D_SIZE` shaper,
/// `DOMAIN_MIN` / `DOMAIN_MAX` or `LUT_3D_INPUT_RANGE`, red fastest.
pub fn parse_cube(text: &str) -> Result<Lut3, String> {
    let mut size3 = 0usize;
    let mut size1 = 0usize;
    let mut lo = [0.0f32; 3];
    let mut hi = [1.0f32; 3];
    let mut rows: Vec<[f32; 3]> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let nums = |it: std::str::SplitWhitespace| -> Result<Vec<f32>, String> {
            it.map(|s| s.parse::<f32>().map_err(|_| format!("line {}: `{line}`", n + 1)))
                .collect()
        };
        match key {
            "TITLE" => {}
            "LUT_3D_SIZE" => size3 = nums(it)?.first().copied().unwrap_or(0.0) as usize,
            "LUT_1D_SIZE" => size1 = nums(it)?.first().copied().unwrap_or(0.0) as usize,
            "DOMAIN_MIN" => lo = three(&nums(it)?).ok_or("DOMAIN_MIN needs 3 numbers")?,
            "DOMAIN_MAX" => hi = three(&nums(it)?).ok_or("DOMAIN_MAX needs 3 numbers")?,
            "LUT_3D_INPUT_RANGE" | "LUT_1D_INPUT_RANGE" => {
                let v = nums(it)?;
                if v.len() == 2 {
                    lo = [v[0]; 3];
                    hi = [v[1]; 3];
                }
            }
            k if k.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') => {
                let v = nums(line.split_whitespace())?;
                rows.push(three(&v).ok_or_else(|| format!("line {}: expected 3 numbers", n + 1))?);
            }
            // other keywords (LUT_IN_VIDEO_RANGE, …) don't change the table
            _ => {}
        }
    }
    let span = [0, 1, 2].map(|k| (hi[k] - lo[k]).max(1e-6));
    let norm = |c: [f32; 3]| [0, 1, 2].map(|k| (c[k] - lo[k]) / span[k]);
    match (size1, size3) {
        (0, 0) => Err("not a LUT: no LUT_3D_SIZE or LUT_1D_SIZE".into()),
        (_, s3) if s3 > 0 => {
            if !(2..=256).contains(&s3) {
                return Err(format!("LUT_3D_SIZE {s3} is out of range"));
            }
            let need3 = s3 * s3 * s3;
            let shaper_rows = if size1 > 0 { size1 } else { 0 };
            if rows.len() != need3 + shaper_rows {
                return Err(format!(
                    "the file has {} colours, LUT_3D_SIZE {s3} needs {}",
                    rows.len(),
                    need3 + shaper_rows
                ));
            }
            let shaper = (shaper_rows > 0).then(|| rows[..shaper_rows].to_vec());
            let cube = Lut3 {
                size: s3,
                data: rows[shaper_rows..].to_vec(),
            };
            Ok(Lut3::from_fn(super::RUNTIME_SIZE, |c| {
                let c = match &shaper {
                    Some(s) => curve3(s, c),
                    None => c,
                };
                cube.sample(norm(c))
            }))
        }
        (s1, _) => {
            if rows.len() != s1 || s1 < 2 {
                return Err(format!("the file has {} rows, LUT_1D_SIZE needs {s1}", rows.len()));
            }
            Ok(Lut3::from_fn(super::RUNTIME_SIZE, |c| curve3(&rows, norm(c))))
        }
    }
}

fn three(v: &[f32]) -> Option<[f32; 3]> {
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// Each channel through its own 1D curve.
fn curve3(curve: &[[f32; 3]], c: [f32; 3]) -> [f32; 3] {
    let m = (curve.len() - 1) as f32;
    [0, 1, 2].map(|k| {
        let p = c[k].clamp(0.0, 1.0) * m;
        let i = (p.floor() as usize).min(curve.len() - 2);
        let t = p - i as f32;
        curve[i][k] * (1.0 - t) + curve[i + 1][k] * t
    })
}

/// A `.cube` file of `lut`.
pub fn write_cube(lut: &Lut3, title: &str) -> String {
    let mut s = format!(
        "# Made with Skyfold Studio\nTITLE \"{}\"\nLUT_3D_SIZE {}\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n",
        title.replace('"', "'"),
        lut.size
    );
    for v in &lut.data {
        s.push_str(&format!("{:.6} {:.6} {:.6}\n", v[0], v[1], v[2]));
    }
    s
}

/// A LUT image: a strip of `n` slices of `n` × `n` (Unreal 256 × 16, Godot/Unity 1024 × 32, any
/// n² × n; blue across the slices, red across each slice, green down it, or up it as Unity
/// writes them) or a HALD image (a square of side L³ holding an L² cube).
pub fn from_image(img: &image::Rgb32FImage) -> Result<Lut3, String> {
    let (w, h) = img.dimensions();
    if h >= 2 && w == h * h {
        let n = h;
        // green runs down the strip (Unreal, Godot) or up it (Unity): an ordinary grade keeps
        // green rising with its index, so the brighter-green edge is the top of the cube
        let mean_g = |y: u32| (0..w).map(|x| img.get_pixel(x, y)[1]).sum::<f32>() / w as f32;
        let up = mean_g(0) > mean_g(n - 1);
        let data = (0..n)
            .flat_map(|b| (0..n).flat_map(move |g| (0..n).map(move |r| (r, g, b))))
            .map(|(r, g, b)| {
                let y = if up { n - 1 - g } else { g };
                let p = img.get_pixel(b * n + r, y);
                [p[0], p[1], p[2]]
            })
            .collect();
        return Ok(Lut3 {
            size: n as usize,
            data,
        }
        .resample(super::RUNTIME_SIZE));
    }
    if w == h {
        let level = (w as f32).cbrt().round() as u32;
        if level >= 2 && level * level * level == w {
            let n = level * level;
            let data = (0..n * n * n)
                .map(|i| {
                    let p = img.get_pixel(i % w, i / w);
                    [p[0], p[1], p[2]]
                })
                .collect();
            return Ok(Lut3 {
                size: n as usize,
                data,
            }
            .resample(super::RUNTIME_SIZE));
        }
    }
    Err(format!(
        "a {w} × {h} picture is not a LUT: a LUT strip is n² × n (256 × 16, 1024 × 32) and a HALD image is square (512 × 512)"
    ))
}

/// The neutral round-trip strip as sRGB pixels (`STRIP_SIZE`² × `STRIP_SIZE`).
fn identity_strip() -> image::RgbImage {
    let n = STRIP_SIZE;
    let v = |i: u32| (i as f32 / (n - 1) as f32 * 255.0).round() as u8;
    image::RgbImage::from_fn(n * n, n, |x, y| image::Rgb([v(x % n), v(y), v(x / n)]))
}

/// The capture for the round-trip: the view with the neutral strip added under it (the picture is
/// widened with black if it is narrower than the strip).
pub fn with_strip(view: &image::RgbImage) -> image::RgbImage {
    let n = STRIP_SIZE;
    let (vw, vh) = view.dimensions();
    let w = vw.max(n * n);
    let mut out = image::RgbImage::new(w, vh + n);
    image::imageops::replace(&mut out, view, 0, 0);
    image::imageops::replace(&mut out, &identity_strip(), 0, vh as i64);
    out
}

/// Reads the LUT back from a round-trip capture after it was graded elsewhere: the strip is the
/// bottom `STRIP_SIZE` rows, left-aligned.
pub fn from_round_trip(img: &image::Rgb32FImage) -> Result<Lut3, String> {
    let n = STRIP_SIZE;
    let (w, h) = img.dimensions();
    if w < n * n || h <= n {
        return Err(format!(
            "this picture ({w} × {h}) has no Skyfold colour strip along its bottom: grade the captured picture without cropping or resizing it"
        ));
    }
    let strip = image::imageops::crop_imm(img, 0, h - n, n * n, n).to_image();
    // a strip that was cropped off or painted over no longer climbs from black to white
    let first = strip.get_pixel(0, 0);
    let last = strip.get_pixel(n * n - 1, n - 1);
    if luma3(last) - luma3(first) < 0.3 {
        return Err(
            "the colour strip along the bottom of the picture is missing or was painted over: use adjustment layers only, and don't crop the picture".into(),
        );
    }
    let data = (0..n)
        .flat_map(|b| (0..n).flat_map(move |g| (0..n).map(move |r| (r, g, b))))
        .map(|(r, g, b)| {
            let p = strip.get_pixel(b * n + r, g);
            [p[0], p[1], p[2]]
        })
        .collect();
    Ok(Lut3 {
        size: n as usize,
        data,
    }
    .resample(super::RUNTIME_SIZE))
}

fn luma3(p: &image::Rgb<f32>) -> f32 {
    super::luma([p[0], p[1], p[2]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= tol)
    }

    #[test]
    fn reads_a_resolve_cube() {
        let mut text = String::from("# Resolve\nTITLE \"warm\"\nLUT_3D_SIZE 2\n\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text += &format!("{} {} {}\n", r as f32 * 0.9 + 0.1, g, b as f32 * 0.8);
                }
            }
        }
        let lut = parse_cube(&text).unwrap();
        assert!(close(lut.sample([0.0, 0.0, 0.0]), [0.1, 0.0, 0.0], 1e-4));
        assert!(close(lut.sample([1.0, 1.0, 1.0]), [1.0, 1.0, 0.8], 1e-4));
        // written and read again
        let again = parse_cube(&write_cube(&lut, "warm")).unwrap();
        assert!(close(again.sample([0.3, 0.6, 0.2]), lut.sample([0.3, 0.6, 0.2]), 1e-4));
    }

    #[test]
    fn reads_domain_and_1d_cubes() {
        let text = "DOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let lut = parse_cube(text).unwrap();
        // input 1.0 is the middle of a 0..2 domain
        assert!(close(lut.sample([1.0, 1.0, 1.0]), [0.5, 0.5, 0.5], 1e-4));
        let one_d = "LUT_1D_SIZE 3\n0 0 0\n0.25 0.5 0.5\n1 1 1\n";
        let lut = parse_cube(one_d).unwrap();
        assert!(close(lut.sample([0.5, 0.5, 0.5]), [0.25, 0.5, 0.5], 1e-4));
    }

    #[test]
    fn rejects_broken_cubes() {
        assert!(parse_cube("hello").is_err());
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 x\n").is_err());
    }

    #[test]
    fn reads_strips_both_ways_up_and_hald() {
        let strip = image::DynamicImage::ImageRgb8(identity_strip()).into_rgb32f();
        assert!(from_image(&strip).unwrap().is_identity(0.01));
        let flipped = image::imageops::flip_vertical(&strip);
        assert!(from_image(&flipped).unwrap().is_identity(0.01));
        // HALD level 2: an 8 × 8 picture holding a 4³ cube
        let hald = image::Rgb32FImage::from_fn(8, 8, |x, y| {
            let i = y * 8 + x;
            let c = |v: u32| v as f32 / 3.0;
            image::Rgb([c(i % 4), c(i / 4 % 4), c(i / 16)])
        });
        assert!(from_image(&hald).unwrap().is_identity(0.01));
        assert!(from_image(&image::Rgb32FImage::new(300, 200)).is_err());
    }

    #[test]
    fn round_trip_recovers_an_edit() {
        let view = image::RgbImage::from_pixel(640, 360, image::Rgb([90, 120, 60]));
        let cap = with_strip(&view);
        assert_eq!(cap.dimensions(), (1024, 392));
        // the "photo app": a warm, darker grade over the whole picture
        let edit = |p: &image::Rgb<u8>| {
            image::Rgb([
                (p[0] as f32 * 0.95 + 10.0).min(255.0) as u8,
                (p[1] as f32 * 0.9) as u8,
                (p[2] as f32 * 0.8) as u8,
            ])
        };
        let mut graded = cap.clone();
        for p in graded.pixels_mut() {
            *p = edit(p);
        }
        let lut = from_round_trip(&image::DynamicImage::ImageRgb8(graded).into_rgb32f()).unwrap();
        let s = lut.sample([0.5, 0.5, 0.5]);
        assert!(close(s, [(127.5 * 0.95 + 10.0) / 255.0, 0.45, 0.4], 0.01), "{s:?}");
        // an uncaptured picture is refused
        let plain = image::DynamicImage::ImageRgb8(view).into_rgb32f();
        assert!(from_round_trip(&plain).is_err());
    }
}
