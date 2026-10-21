//! Map furniture: frame, grid, compass rose, scale bar and title. Positions are chosen first
//! (over open water where possible) so the lettering can keep clear of them.

use bevy::math::Vec2;
use tiny_skia::PathBuilder;

use super::MapStyle;
use super::canvas::{
    Align, Bx, Canvas, Face, Fonts, Rgba, TextStyle, measure, poly_path, rgb, rgba, with_alpha,
};
use super::render::Ctx;

pub struct Deco {
    pub frame_inset: f32,
    pub compass: Option<(Vec2, f32)>,
    /// Left end of the bar, its length (px) and what it measures (m).
    pub scale: Option<(Vec2, f32, f32)>,
    pub title: Option<(Vec2, String, TextStyle)>,
    pub reserved: Vec<Bx>,
}

/// Share of a box that is land (sampled).
fn land_share(c: &Ctx, b: &Bx) -> f32 {
    let mut land = 0;
    let n = 10;
    for j in 0..n {
        for i in 0..n {
            let p = Vec2::new(
                b.x0 + (b.x1 - b.x0) * (i as f32 + 0.5) / n as f32,
                b.y0 + (b.y1 - b.y0) * (j as f32 + 0.5) / n as f32,
            );
            if c.coast_d(p) < 0.0 {
                land += 1;
            }
        }
    }
    land as f32 / (n * n) as f32
}

fn title_style(c: &Ctx) -> TextStyle {
    let u = c.w as f32 / 2048.0;
    let k = &c.kit;
    match k.style {
        MapStyle::Parchment => TextStyle::new(Face::DisplayBold, 52.0 * u, rgb(70, 34, 26))
            .tracking(0.08)
            .halo(rgba(240, 229, 200, 230), 7.0 * u),
        MapStyle::Atlas => TextStyle::new(Face::SansBold, 26.0 * u, rgb(40, 40, 48))
            .tracking(0.12)
            .upper()
            .halo(rgba(255, 255, 255, 220), 5.0 * u),
        _ => TextStyle::new(Face::SansBold, 24.0 * u, rgb(255, 255, 255))
            .tracking(0.12)
            .upper()
            .halo(rgba(0, 0, 0, 170), 5.0 * u),
    }
}

pub fn plan(c: &Ctx, fonts: &Fonts) -> Deco {
    let u = c.w as f32 / 2048.0;
    let w = c.w as f32;
    let framed = matches!(c.kit.style, MapStyle::Parchment | MapStyle::Atlas);
    let frame_inset = if framed { 18.0 * u } else { 6.0 * u };
    let m = frame_inset + 22.0 * u;
    let mut reserved: Vec<Bx> = Vec::new();
    let layers = &c.settings.layers;
    let pick = |cands: Vec<Bx>, reserved: &[Bx]| -> Option<Bx> {
        cands
            .into_iter()
            .filter(|b| !reserved.iter().any(|r| r.hits(b)))
            .map(|b| (land_share(c, &b), b))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, b)| b)
    };

    let mut title = None;
    let text = if c.settings.title.trim().is_empty() {
        c.inp.name.clone()
    } else {
        c.settings.title.trim().to_string()
    };
    if c.settings.show_title && !text.is_empty() {
        let st = title_style(c);
        let tw = measure(fonts, &st, &text) + 64.0 * u;
        let th = st.size * 1.5 + 14.0 * u;
        let cands = vec![
            Bx::new(m, m, m + tw, m + th),
            Bx::new(w - m - tw, m, w - m, m + th),
            Bx::new((w - tw) * 0.5, m, (w + tw) * 0.5, m + th),
            Bx::new(m, w - m - th, m + tw, w - m),
            Bx::new(w - m - tw, w - m - th, w - m, w - m),
        ];
        if let Some(b) = pick(cands, &reserved) {
            reserved.push(b.grow(6.0 * u));
            title = Some((b.centre() - Vec2::new(0.0, 6.0 * u), text, st));
        }
    }

    let mut compass = None;
    if layers.compass {
        let r = match c.kit.style {
            MapStyle::Parchment => 78.0 * u,
            MapStyle::Atlas => 30.0 * u,
            _ => 24.0 * u,
        };
        let s = r * 2.9 + 12.0 * u;
        let cands = vec![
            Bx::new(w - m - s, m, w - m, m + s),
            Bx::new(m, m, m + s, m + s),
            Bx::new(w - m - s, w - m - s, w - m, w - m),
            Bx::new(m, w - m - s, m + s, w - m),
        ];
        if let Some(b) = pick(cands, &reserved) {
            reserved.push(b);
            compass = Some((b.centre() + Vec2::new(0.0, 8.0 * u), r));
        }
    }

    let mut scale = None;
    if layers.compass {
        let ppm = c.ppm();
        let target = w * 0.2 / ppm;
        let mag = 10f32.powf(target.log10().floor());
        let nice = [1.0, 2.0, 5.0]
            .iter()
            .map(|f| f * mag)
            .rfind(|v| *v <= target)
            .unwrap_or(mag);
        let len = nice * ppm;
        let bw = len + 60.0 * u;
        let bh = 42.0 * u;
        let cands = vec![
            Bx::new(m, w - m - bh, m + bw, w - m),
            Bx::new(w - m - bw, w - m - bh, w - m, w - m),
            Bx::new(m, m, m + bw, m + bh),
            Bx::new(w - m - bw, m, w - m, m + bh),
        ];
        if let Some(b) = pick(cands, &reserved) {
            reserved.push(b);
            scale = Some((Vec2::new(b.x0 + 18.0 * u, b.y1 - 16.0 * u), len, nice));
        }
    }
    Deco {
        frame_inset,
        compass,
        scale,
        title,
        reserved,
    }
}

pub fn draw(cv: &mut Canvas, c: &Ctx, fonts: &Fonts, d: &Deco) {
    let u = cv.u;
    let w = c.w as f32;
    let k = &c.kit;
    if c.settings.layers.grid {
        grid(cv, c, fonts, d);
    }
    match k.style {
        MapStyle::Parchment => {
            // double neat line, the outer one heavy, the margin a shade darker
            let o = d.frame_inset;
            let outer = Bx::new(o * 0.45, o * 0.45, w - o * 0.45, w - o * 0.45);
            let inner = Bx::new(o, o, w - o, w - o);
            frame_rect(cv, outer, 2.6 * u, k.ink);
            frame_rect(cv, inner, 0.9 * u, k.ink);
        }
        MapStyle::Atlas => {
            let o = d.frame_inset;
            let mut band = PathBuilder::new();
            band.push_rect(tiny_skia::Rect::from_ltrb(0.0, 0.0, w, w).unwrap());
            band.push_rect(tiny_skia::Rect::from_ltrb(o, o, w - o, w - o).unwrap());
            if let Some(p) = band.finish() {
                cv.fill_evenodd(&p, rgb(250, 249, 245));
            }
            frame_rect(cv, Bx::new(o, o, w - o, w - o), 1.0 * u, rgb(60, 60, 66));
        }
        _ => {}
    }
    if let Some((c0, r)) = d.compass {
        match k.style {
            MapStyle::Parchment => compass_rose(cv, fonts, c0, r, k.ink),
            MapStyle::Atlas => north_arrow(
                cv,
                fonts,
                c0,
                r,
                rgb(40, 40, 48),
                rgb(255, 255, 255),
                rgba(255, 255, 255, 220),
            ),
            _ => north_arrow(
                cv,
                fonts,
                c0,
                r,
                rgb(255, 255, 255),
                rgb(40, 46, 56),
                rgba(0, 0, 0, 150),
            ),
        }
    }
    if let Some((at, len, metres)) = d.scale {
        scale_bar(cv, c, fonts, at, len, metres);
    }
    if let Some((at, text, st)) = &d.title {
        if k.style == MapStyle::Parchment {
            cartouche(cv, fonts, *at, text, st, k.ink);
        } else {
            cv.text(fonts, st, text, *at, Align::Centre);
        }
    }
}

fn frame_rect(cv: &mut Canvas, b: Bx, width: f32, col: Rgba) {
    let pts = [
        Vec2::new(b.x0, b.y0),
        Vec2::new(b.x1, b.y0),
        Vec2::new(b.x1, b.y1),
        Vec2::new(b.x0, b.y1),
    ];
    if let Some(p) = poly_path(&pts, true) {
        cv.stroke_butt(&p, width, col);
    }
}

/// An 8×8 reference grid lettered A–H across and 1–8 down.
fn grid(cv: &mut Canvas, c: &Ctx, fonts: &Fonts, d: &Deco) {
    let u = cv.u;
    let w = c.w as f32;
    let o = d.frame_inset;
    let cells = 8;
    let step = (w - 2.0 * o) / cells as f32;
    let col = c.kit.grid;
    for k in 1..cells {
        let x = o + step * k as f32;
        let a = Vec2::new(x, o);
        let b = Vec2::new(x, w - o);
        let mut pb = PathBuilder::new();
        pb.move_to(a.x, a.y);
        pb.line_to(b.x, b.y);
        pb.move_to(o, x);
        pb.line_to(w - o, x);
        if let Some(p) = pb.finish() {
            if c.kit.style == MapStyle::Parchment {
                cv.dashed(&p, 0.7 * u, col, &[5.0 * u, 4.0 * u], false);
            } else {
                cv.stroke_butt(&p, 0.6 * u, col);
            }
        }
    }
    let face = if c.kit.serif {
        Face::SerifBold
    } else {
        Face::SansBold
    };
    let ink = if matches!(c.kit.style, MapStyle::Minimal | MapStyle::Satellite) {
        rgb(255, 255, 255)
    } else {
        c.kit.ink
    };
    let st = TextStyle::new(face, 11.0 * u, with_alpha(ink, 0.8)).halo(c.kit.halo, 2.4 * u);
    for k in 0..cells {
        let mid = o + step * (k as f32 + 0.5);
        let letter = ((b'A' + k as u8) as char).to_string();
        cv.text(
            fonts,
            &st,
            &letter,
            Vec2::new(mid, o + 10.0 * u),
            Align::Centre,
        );
        cv.text(
            fonts,
            &st,
            &format!("{}", k + 1),
            Vec2::new(o + 9.0 * u, mid),
            Align::Centre,
        );
    }
}

/// A 16-point compass rose: split-shaded points, a ticked ring, a fleur for north.
fn compass_rose(cv: &mut Canvas, fonts: &Fonts, c: Vec2, r: f32, ink: Rgba) {
    let u = cv.u;
    let paper = rgb(242, 232, 206);
    // a faint disc so it reads over land too
    cv.circle(c, r * 1.02, rgba(242, 232, 206, 150));
    cv.ring(c, r, 1.2 * u, ink);
    cv.ring(c, r * 0.9, 0.6 * u, ink);
    for k in 0..72 {
        let a = k as f32 / 72.0 * std::f32::consts::TAU;
        let d = Vec2::new(a.sin(), -a.cos());
        let len = if k % 9 == 0 {
            0.1
        } else if k % 3 == 0 {
            0.07
        } else {
            0.04
        };
        cv.line(c + d * r * 0.9, c + d * r * (0.9 + len), 0.5 * u, ink);
    }
    let point = |cv: &mut Canvas, a: f32, len: f32, half: f32| {
        let d = Vec2::new(a.sin(), -a.cos());
        let side = Vec2::new(d.y, -d.x);
        let tip = c + d * len;
        let l = c + side * half + d * half * 0.6;
        let rr = c - side * half + d * half * 0.6;
        if let Some(p) = poly_path(&[c, tip, l], true) {
            cv.fill(&p, ink);
        }
        if let Some(p) = poly_path(&[c, tip, rr], true) {
            cv.fill(&p, paper);
            cv.stroke(&p, 0.6 * u, ink);
        }
    };
    for k in 0..8 {
        point(
            cv,
            (k as f32 + 0.5) / 8.0 * std::f32::consts::TAU,
            r * 0.5,
            r * 0.07,
        );
    }
    for k in 0..4 {
        point(
            cv,
            (k as f32 + 0.5) / 4.0 * std::f32::consts::TAU,
            r * 0.68,
            r * 0.11,
        );
    }
    for k in 0..4 {
        point(
            cv,
            k as f32 / 4.0 * std::f32::consts::TAU,
            r * 1.16,
            r * 0.15,
        );
    }
    cv.circle(c, r * 0.07, paper);
    cv.ring(c, r * 0.07, 0.7 * u, ink);
    let st =
        TextStyle::new(Face::DisplayBold, r * 0.32, ink).halo(rgba(242, 232, 206, 220), 3.0 * u);
    cv.text(
        fonts,
        &st,
        "N",
        c + Vec2::new(0.0, -r * 1.36),
        Align::Centre,
    );
}

fn north_arrow(
    cv: &mut Canvas,
    fonts: &Fonts,
    c: Vec2,
    r: f32,
    dark: Rgba,
    light: Rgba,
    halo: Rgba,
) {
    let u = cv.u;
    let tip = c + Vec2::new(0.0, -r);
    let tail = c + Vec2::new(0.0, r * 0.75);
    let l = c + Vec2::new(-r * 0.45, r * 0.95);
    let rr = c + Vec2::new(r * 0.45, r * 0.95);
    if let Some(p) = poly_path(&[tip, rr, tail, l], true) {
        cv.stroke(&p, 3.0 * u, halo);
    }
    if let Some(p) = poly_path(&[tip, tail, l], true) {
        cv.fill(&p, dark);
    }
    if let Some(p) = poly_path(&[tip, rr, tail], true) {
        cv.fill(&p, light);
        cv.stroke(&p, 0.9 * u, dark);
    }
    let st = TextStyle::new(Face::SansBold, r * 0.62, dark).halo(halo, 3.0 * u);
    cv.text(
        fonts,
        &st,
        "N",
        c + Vec2::new(0.0, -r * 1.45),
        Align::Centre,
    );
}

fn scale_bar(cv: &mut Canvas, c: &Ctx, fonts: &Fonts, at: Vec2, len: f32, metres: f32) {
    let u = cv.u;
    let k = &c.kit;
    let (dark, light, text, halo) = match k.style {
        MapStyle::Parchment => (k.ink, rgb(242, 232, 206), k.ink, rgba(242, 232, 206, 220)),
        MapStyle::Atlas => (
            rgb(40, 40, 48),
            rgb(255, 255, 255),
            rgb(40, 40, 48),
            rgba(255, 255, 255, 220),
        ),
        _ => (
            rgb(255, 255, 255),
            rgb(30, 36, 44),
            rgb(255, 255, 255),
            rgba(0, 0, 0, 160),
        ),
    };
    let h = 6.0 * u;
    let segs = 4;
    let sw = len / segs as f32;
    cv.rect(
        Bx::new(
            at.x - 1.5 * u,
            at.y - h - 1.5 * u,
            at.x + len + 1.5 * u,
            at.y + 1.5 * u,
        ),
        halo,
    );
    for s in 0..segs {
        let x = at.x + sw * s as f32;
        cv.rect(
            Bx::new(x, at.y - h, x + sw, at.y),
            if s % 2 == 0 { dark } else { light },
        );
    }
    frame_rect(cv, Bx::new(at.x, at.y - h, at.x + len, at.y), 0.8 * u, dark);
    let face = if k.serif { Face::Serif } else { Face::Sans };
    let st = TextStyle::new(face, 14.0 * u, text).halo(halo, 3.0 * u);
    let fmt = |m: f32| {
        if metres >= 1000.0 {
            trim(m / 1000.0)
        } else {
            trim(m)
        }
    };
    let unit = if metres >= 1000.0 { "km" } else { "m" };
    cv.text(
        fonts,
        &st,
        "0",
        Vec2::new(at.x, at.y - h - 11.0 * u),
        Align::Centre,
    );
    cv.text(
        fonts,
        &st,
        &fmt(metres * 0.5),
        Vec2::new(at.x + len * 0.5, at.y - h - 11.0 * u),
        Align::Centre,
    );
    cv.text(
        fonts,
        &st,
        &format!("{} {unit}", fmt(metres)),
        Vec2::new(at.x + len, at.y - h - 11.0 * u),
        Align::Centre,
    );
}

fn trim(v: f32) -> String {
    let s = format!("{v:.1}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The title on a scroll-like plaque with flourished rules.
fn cartouche(cv: &mut Canvas, fonts: &Fonts, at: Vec2, text: &str, st: &TextStyle, ink: Rgba) {
    let u = cv.u;
    let tw = measure(fonts, st, text);
    let hw = tw * 0.5 + 26.0 * u;
    let hh = st.size * 0.72;
    let b = Bx::new(at.x - hw, at.y - hh, at.x + hw, at.y + hh);
    // plaque with notched (ribbon) ends
    let notch = hh * 0.55;
    let pts = [
        Vec2::new(b.x0, b.y0),
        Vec2::new(b.x1, b.y0),
        Vec2::new(b.x1 - notch, at.y),
        Vec2::new(b.x1, b.y1),
        Vec2::new(b.x0, b.y1),
        Vec2::new(b.x0 + notch, at.y),
    ];
    if let Some(p) = poly_path(&pts, true) {
        cv.fill(&p, rgba(244, 234, 210, 235));
        cv.stroke(&p, 1.4 * u, ink);
    }
    let inner: Vec<Vec2> = pts
        .iter()
        .map(|p| *p + (at - *p).normalize_or_zero() * 4.0 * u)
        .collect();
    if let Some(p) = poly_path(&inner, true) {
        cv.stroke(&p, 0.6 * u, with_alpha(ink, 0.8));
    }
    let mut plain = st.clone();
    plain.halo = None;
    cv.text(
        fonts,
        &plain,
        text,
        at + Vec2::new(0.0, 1.0 * u),
        Align::Centre,
    );
    // flourishes under the plaque
    for s in [-1.0f32, 1.0] {
        let a = at + Vec2::new(s * tw * 0.08, hh + 8.0 * u);
        let b2 = at + Vec2::new(s * (tw * 0.5 + 6.0 * u), hh + 8.0 * u);
        let mut pb = PathBuilder::new();
        pb.move_to(a.x, a.y);
        pb.quad_to((a.x + b2.x) * 0.5, a.y + 5.0 * u, b2.x, b2.y);
        if let Some(p) = pb.finish() {
            cv.stroke(&p, 0.9 * u, ink);
        }
        cv.circle(b2, 1.6 * u, ink);
    }
    cv.circle(at + Vec2::new(0.0, hh + 8.0 * u), 2.0 * u, ink);
}
