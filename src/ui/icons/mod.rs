//! Central Icons (round · outlined · 1.5 stroke, white on a 24 × 24 grid), rasterised with
//! tiny-skia at the exact pixel size and cached as textures per (icon, pixels). The SVGs are white,
//! so an icon tints to any colour.
//!
//! The set only uses absolute `M L H V C Z` paths, circles, ellipses and rects, so a small reader
//! covers it (no SVG crate).

mod svgs;

use std::collections::HashMap;

use bevy_egui::egui::{
    self, Color32, Id, Painter, Pos2, Rect, Response, Sense, TextureHandle, TextureId, Ui, Vec2,
    pos2,
};

/// The SVG source of a Central icon.
fn svg(name: &str) -> Option<&'static str> {
    svgs::SVGS
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| svgs::SVGS[i].1)
}

/// True when `name` is a Central icon (otherwise it's a Phosphor glyph drawn as text).
pub fn has(name: &str) -> bool {
    svg(name).is_some()
}

// ------------------------------------------------------------------ the SVG reader

/// `name="value"` of one tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let after = &rest[i + name.len()..];
        let before_ok = i == 0 || rest.as_bytes()[i - 1] == b' ';
        if before_ok && let Some(v) = after.strip_prefix("=\"") {
            return v.split('"').next();
        }
        rest = &rest[i + name.len()..];
    }
    None
}

fn num(tag: &str, name: &str) -> Option<f32> {
    attr(tag, name).and_then(|v| v.trim().parse().ok())
}

/// Numbers in a path's `d` (commas, spaces, signs and exponents as separators).
fn numbers(s: &str) -> Vec<f32> {
    let mut out = vec![];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'-' || c == b'+' || c == b'.' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            let mut dot = c == b'.';
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !dot {
                    dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..i].parse() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn path_from_d(d: &str) -> Option<tiny_skia::Path> {
    let mut pb = tiny_skia::PathBuilder::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut cmds = d
        .char_indices()
        .filter(|(_, c)| c.is_ascii_alphabetic())
        .peekable();
    while let Some((i, c)) = cmds.next() {
        let end = cmds.peek().map_or(d.len(), |(j, _)| *j);
        let v = numbers(&d[i + 1..end]);
        match c {
            'M' => {
                for (k, p) in v.chunks_exact(2).enumerate() {
                    (x, y) = (p[0], p[1]);
                    if k == 0 {
                        pb.move_to(x, y)
                    } else {
                        pb.line_to(x, y)
                    }
                }
            }
            'L' => {
                for p in v.chunks_exact(2) {
                    (x, y) = (p[0], p[1]);
                    pb.line_to(x, y);
                }
            }
            'H' => {
                for &h in &v {
                    x = h;
                    pb.line_to(x, y);
                }
            }
            'V' => {
                for &vv in &v {
                    y = vv;
                    pb.line_to(x, y);
                }
            }
            'C' => {
                for p in v.chunks_exact(6) {
                    pb.cubic_to(p[0], p[1], p[2], p[3], p[4], p[5]);
                    (x, y) = (p[4], p[5]);
                }
            }
            'Z' | 'z' => pb.close(),
            _ => {}
        }
    }
    pb.finish()
}

/// Rasterise an icon at `px` × `px` (white, premultiplied).
fn rasterise(src: &str, px: u32) -> Option<egui::ColorImage> {
    let mut pixmap = tiny_skia::Pixmap::new(px, px)?;
    let root = src.split('>').next().unwrap_or("");
    let base_width = num(root, "stroke-width").unwrap_or(1.5);
    let ts = tiny_skia::Transform::from_scale(px as f32 / 24.0, px as f32 / 24.0);
    // skip what sits in <defs> (clip shapes)
    let body: String = {
        let mut s = src.to_string();
        while let (Some(a), Some(b)) = (s.find("<defs"), s.find("</defs>")) {
            s.replace_range(a..b + 7, "");
        }
        s
    };
    for raw in body.split('<').skip(1) {
        let tag = raw.split('>').next().unwrap_or("");
        let kind = tag.split_whitespace().next().unwrap_or("");
        let path = match kind {
            "path" => attr(tag, "d").and_then(path_from_d),
            "circle" => match (num(tag, "cx"), num(tag, "cy"), num(tag, "r")) {
                (Some(cx), Some(cy), Some(r)) => tiny_skia::PathBuilder::from_circle(cx, cy, r),
                _ => None,
            },
            "ellipse" => match (
                num(tag, "cx"),
                num(tag, "cy"),
                num(tag, "rx"),
                num(tag, "ry"),
            ) {
                (Some(cx), Some(cy), Some(rx), Some(ry)) => {
                    tiny_skia::Rect::from_xywh(cx - rx, cy - ry, rx * 2.0, ry * 2.0)
                        .map(tiny_skia::PathBuilder::from_oval)
                        .flatten()
                }
                _ => None,
            },
            "rect" => match (num(tag, "width"), num(tag, "height")) {
                (Some(w), Some(h)) => tiny_skia::Rect::from_xywh(
                    num(tag, "x").unwrap_or(0.0),
                    num(tag, "y").unwrap_or(0.0),
                    w,
                    h,
                )
                .map(tiny_skia::PathBuilder::from_rect),
                _ => None,
            },
            _ => None,
        };
        let Some(path) = path else { continue };
        let fill = attr(tag, "fill").is_some_and(|f| f != "none");
        let mut paint = tiny_skia::Paint {
            anti_alias: true,
            ..Default::default()
        };
        if fill {
            paint.set_color_rgba8(255, 255, 255, 255);
            let rule = if attr(tag, "fill-rule") == Some("evenodd") {
                tiny_skia::FillRule::EvenOdd
            } else {
                tiny_skia::FillRule::Winding
            };
            pixmap.fill_path(&path, &paint, rule, ts, None);
        }
        if attr(tag, "stroke") == Some("none") {
            continue;
        }
        let opacity = num(tag, "stroke-opacity").unwrap_or(1.0).clamp(0.0, 1.0);
        paint.set_color_rgba8(255, 255, 255, (opacity * 255.0).round() as u8);
        let cap = match attr(tag, "stroke-linecap") {
            Some("butt") => tiny_skia::LineCap::Butt,
            Some("square") => tiny_skia::LineCap::Square,
            _ => tiny_skia::LineCap::Round,
        };
        let dash =
            attr(tag, "stroke-dasharray").and_then(|d| tiny_skia::StrokeDash::new(numbers(d), 0.0));
        let stroke = tiny_skia::Stroke {
            width: num(tag, "stroke-width").unwrap_or(base_width),
            line_cap: cap,
            line_join: tiny_skia::LineJoin::Round,
            dash,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, ts, None);
    }
    Some(egui::ColorImage::from_rgba_premultiplied(
        [px as usize, px as usize],
        pixmap.data(),
    ))
}

// ------------------------------------------------------------------ textures

#[derive(Clone, Default)]
struct Cache(HashMap<(&'static str, u32), TextureHandle>);

/// The icon's texture at `px` physical pixels (made once, then cached).
pub fn texture(ctx: &egui::Context, name: &str, px: u32) -> Option<TextureId> {
    let i = svgs::SVGS.binary_search_by(|(n, _)| (*n).cmp(name)).ok()?;
    let (key, src) = svgs::SVGS[i];
    let px = px.clamp(4, 256);
    let id = Id::new("central_icon_cache");
    if let Some(t) = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Cache>(id)
            .0
            .get(&(key, px))
            .map(|h| h.id())
    }) {
        return Some(t);
    }
    let img = rasterise(src, px)?;
    let h = ctx.load_texture(
        format!("icon_{key}_{px}"),
        img,
        egui::TextureOptions::LINEAR,
    );
    let tid = h.id();
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Cache>(id)
            .0
            .insert((key, px), h)
    });
    Some(tid)
}

/// Paint a Central icon `size` points wide centred on `center`, tinted (any painter: panels,
/// viewport overlays, map markers).
pub fn paint(painter: &Painter, center: Pos2, name: &str, size: f32, tint: Color32) {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let px = (size * ppp).round() as u32;
    if let Some(t) = texture(ctx, name, px) {
        // snap to the pixel grid so the 1.5 px strokes stay crisp
        let min = ((center.to_vec2() - Vec2::splat(size * 0.5)) * ppp).round() / ppp;
        let rect = Rect::from_min_size(min.to_pos2(), Vec2::splat(px as f32 / ppp));
        painter.image(
            t,
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            tint,
        );
    }
}

/// A Central icon as a widget (hover sense, so it can carry a tooltip).
pub fn icon(ui: &mut Ui, name: &str, size: f32, tint: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect.center(), name, size, tint);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_rasterises_with_ink() {
        for (name, src) in svgs::SVGS.iter() {
            let img = rasterise(src, 30).unwrap_or_else(|| panic!("{name} did not rasterise"));
            let ink: u32 = img.pixels.iter().map(|p| p.a() as u32).sum();
            assert!(ink > 255 * 6, "{name} came out empty");
        }
    }

    #[test]
    fn names_are_sorted_for_lookup() {
        assert!(svgs::SVGS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(has("FormPyramide") && has("3dBoxTop") && !has("NoSuchIcon"));
    }
}
