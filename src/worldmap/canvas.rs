//! Vector drawing on the map: anti-aliased fills and strokes (tiny-skia), smooth curves through
//! polylines, and lettering from the bundled OFL fonts (glyph outlines from ab_glyph turned into
//! paths, so text gets real halos, tracking, faux italics and can follow a curve).

use std::sync::OnceLock;

use ab_glyph::{Font, FontVec, GlyphId, OutlineCurve, VariableFont};
use bevy::math::Vec2;
use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, StrokeDash, Transform,
};

/// Straight-alpha sRGB colour.
pub type Rgba = [u8; 4];

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}
pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Rgba {
    [r, g, b, a]
}
pub fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [
        c[0],
        c[1],
        c[2],
        (c[3] as f32 * a.clamp(0.0, 1.0)).round() as u8,
    ]
}

/// Axis-aligned box in map pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bx {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Bx {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }
    pub fn around(c: Vec2, half: Vec2) -> Self {
        Self::new(c.x - half.x, c.y - half.y, c.x + half.x, c.y + half.y)
    }
    pub fn hits(&self, o: &Bx) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.y0 < o.y1 && o.y0 < self.y1
    }
    pub fn grow(&self, d: f32) -> Bx {
        Bx {
            x0: self.x0 - d,
            y0: self.y0 - d,
            x1: self.x1 + d,
            y1: self.y1 + d,
        }
    }
    pub fn union(&self, o: &Bx) -> Bx {
        Bx {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
    pub fn centre(&self) -> Vec2 {
        Vec2::new((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }
}

/// The map being drawn: a premultiplied RGBA pixmap and the line-weight unit `u` (1 at 2048 px,
/// so every style scales with the output resolution).
pub struct Canvas {
    pub pm: Pixmap,
    pub u: f32,
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

impl Canvas {
    pub fn new(size: u32) -> Self {
        Self {
            pm: Pixmap::new(size, size).expect("map size"),
            u: size as f32 / 2048.0,
        }
    }
    pub fn fill(&mut self, path: &Path, c: Rgba) {
        self.pm.fill_path(
            path,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    pub fn fill_evenodd(&mut self, path: &Path, c: Rgba) {
        self.pm.fill_path(
            path,
            &paint(c),
            FillRule::EvenOdd,
            Transform::identity(),
            None,
        );
    }
    pub fn stroke(&mut self, path: &Path, width: f32, c: Rgba) {
        let s = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        self.pm
            .stroke_path(path, &paint(c), &s, Transform::identity(), None);
    }
    pub fn stroke_butt(&mut self, path: &Path, width: f32, c: Rgba) {
        let s = Stroke {
            width,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        self.pm
            .stroke_path(path, &paint(c), &s, Transform::identity(), None);
    }
    /// Dashed stroke; `dash` alternates on / off lengths (px).
    pub fn dashed(&mut self, path: &Path, width: f32, c: Rgba, dash: &[f32], round: bool) {
        let s = Stroke {
            width,
            line_cap: if round { LineCap::Round } else { LineCap::Butt },
            line_join: LineJoin::Round,
            dash: StrokeDash::new(dash.to_vec(), 0.0),
            ..Default::default()
        };
        self.pm
            .stroke_path(path, &paint(c), &s, Transform::identity(), None);
    }
    pub fn circle(&mut self, c: Vec2, r: f32, fill: Rgba) {
        if let Some(p) = PathBuilder::from_circle(c.x, c.y, r.max(0.05)) {
            self.fill(&p, fill);
        }
    }
    pub fn ring(&mut self, c: Vec2, r: f32, width: f32, col: Rgba) {
        if let Some(p) = PathBuilder::from_circle(c.x, c.y, r.max(0.05)) {
            self.stroke(&p, width, col);
        }
    }
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32, c: Rgba) {
        let mut pb = PathBuilder::new();
        pb.move_to(a.x, a.y);
        pb.line_to(b.x, b.y);
        if let Some(p) = pb.finish() {
            self.stroke(&p, width, c);
        }
    }
    pub fn rect(&mut self, b: Bx, c: Rgba) {
        if let Some(r) = tiny_skia::Rect::from_ltrb(b.x0, b.y0, b.x1, b.y1) {
            self.fill(&PathBuilder::from_rect(r), c);
        }
    }
}

/// Straight segments through the points.
pub fn poly_path(pts: &[Vec2], closed: bool) -> Option<Path> {
    if pts.len() < 2 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].x, pts[0].y);
    for p in &pts[1..] {
        pb.line_to(p.x, p.y);
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

/// A smooth curve through the points (Catmull–Rom as cubic Béziers).
pub fn smooth_path(pts: &[Vec2], closed: bool) -> Option<Path> {
    let n = pts.len();
    if n < 3 {
        return poly_path(pts, closed);
    }
    let get = |i: isize| -> Vec2 {
        if closed {
            pts[i.rem_euclid(n as isize) as usize]
        } else {
            pts[i.clamp(0, n as isize - 1) as usize]
        }
    };
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].x, pts[0].y);
    let segs = if closed { n } else { n - 1 };
    for i in 0..segs as isize {
        let (p0, p1, p2, p3) = (get(i - 1), get(i), get(i + 1), get(i + 2));
        let c1 = p1 + (p2 - p0) / 6.0;
        let c2 = p2 - (p3 - p1) / 6.0;
        pb.cubic_to(c1.x, c1.y, c2.x, c2.y, p2.x, p2.y);
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

// ------------------------------------------------------------------ lettering

/// The typefaces a map style letters with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// Cinzel: classical capitals for regions and titles.
    Display,
    DisplayBold,
    /// EB Garamond: places, water (italic), notes.
    Serif,
    SerifBold,
    /// Public Sans: modern atlas and in-game labels.
    Sans,
    SansBold,
}

pub struct Fonts {
    faces: Vec<(Face, FontVec)>,
}

impl Fonts {
    /// The bundled map fonts (loaded once). `None` when the font files are missing.
    pub fn get() -> Option<&'static Fonts> {
        static FONTS: OnceLock<Option<Fonts>> = OnceLock::new();
        FONTS.get_or_init(Fonts::load).as_ref()
    }

    fn load() -> Option<Fonts> {
        let dir = std::path::PathBuf::from(crate::assets_path()).join("fonts/map");
        let read = |f: &str| {
            std::fs::read(dir.join(f))
                .map_err(|e| bevy::log::warn!("map font {f}: {e}"))
                .ok()
        };
        let (cinzel, garamond, sans) = (
            read("Cinzel.ttf")?,
            read("EBGaramond.ttf")?,
            read("PublicSans.ttf")?,
        );
        let make = |bytes: &Vec<u8>, wght: f32| {
            let mut f = FontVec::try_from_vec(bytes.clone()).ok()?;
            f.set_variation(b"wght", wght);
            Some(f)
        };
        Some(Fonts {
            faces: vec![
                (Face::Display, make(&cinzel, 500.0)?),
                (Face::DisplayBold, make(&cinzel, 700.0)?),
                (Face::Serif, make(&garamond, 450.0)?),
                (Face::SerifBold, make(&garamond, 620.0)?),
                (Face::Sans, make(&sans, 450.0)?),
                (Face::SansBold, make(&sans, 700.0)?),
            ],
        })
    }

    fn font(&self, f: Face) -> &FontVec {
        &self
            .faces
            .iter()
            .find(|(k, _)| *k == f)
            .unwrap_or(&self.faces[0])
            .1
    }
}

/// How a label is lettered.
#[derive(Clone, Debug)]
pub struct TextStyle {
    pub face: Face,
    /// Em size in px.
    pub size: f32,
    /// Extra letter spacing (em).
    pub tracking: f32,
    /// Slanted (water names).
    pub italic: bool,
    pub upper: bool,
    pub fill: Rgba,
    /// Outline drawn under the letters: colour and total width (px).
    pub halo: Option<(Rgba, f32)>,
}

impl TextStyle {
    pub fn new(face: Face, size: f32, fill: Rgba) -> Self {
        Self {
            face,
            size,
            tracking: 0.0,
            italic: false,
            upper: false,
            fill,
            halo: None,
        }
    }
    pub fn tracking(mut self, em: f32) -> Self {
        self.tracking = em;
        self
    }
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    pub fn upper(mut self) -> Self {
        self.upper = true;
        self
    }
    pub fn halo(mut self, c: Rgba, width: f32) -> Self {
        self.halo = Some((c, width));
        self
    }
}

const ITALIC_SLANT: f32 = 0.2;

struct Run {
    glyphs: Vec<(GlyphId, f32, f32)>, // id, pen x (px), advance (px)
    width: f32,
    scale: f32,
}

fn layout(fonts: &Fonts, st: &TextStyle, text: &str) -> Run {
    let font = fonts.font(st.face);
    let scale = st.size / font.units_per_em().unwrap_or(1000.0);
    let text: String = if st.upper {
        text.to_uppercase()
    } else {
        text.to_string()
    };
    let mut pen = 0.0;
    let mut prev: Option<GlyphId> = None;
    let mut glyphs = Vec::new();
    let track = st.tracking * st.size;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            pen += font.kern_unscaled(p, id) * scale;
        }
        let adv = font.h_advance_unscaled(id) * scale;
        glyphs.push((id, pen, adv));
        pen += adv + track;
        prev = Some(id);
    }
    let width = if glyphs.is_empty() { 0.0 } else { pen - track };
    Run {
        glyphs,
        width,
        scale,
    }
}

/// Width of a label in px.
pub fn measure(fonts: &Fonts, st: &TextStyle, text: &str) -> f32 {
    layout(fonts, st, text).width
}

/// Cap height (px), for vertical centring.
pub fn cap_height(st: &TextStyle) -> f32 {
    st.size
        * if matches!(st.face, Face::Display | Face::DisplayBold) {
            0.7
        } else {
            0.66
        }
}

/// Adds glyph `id`'s outline to `pb`, mapped by `map` (font units, y up → px).
fn add_glyph(
    pb: &mut PathBuilder,
    font: &FontVec,
    id: GlyphId,
    map: impl Fn(f32, f32) -> (f32, f32),
) {
    let Some(outline) = font.outline(id) else {
        return;
    };
    let mut last: Option<ab_glyph::Point> = None;
    for c in &outline.curves {
        let (start, end) = match c {
            OutlineCurve::Line(a, b) => (*a, *b),
            OutlineCurve::Quad(a, _, b) => (*a, *b),
            OutlineCurve::Cubic(a, _, _, b) => (*a, *b),
        };
        if last.is_none_or(|l| (l.x - start.x).abs() > 1e-3 || (l.y - start.y).abs() > 1e-3) {
            if last.is_some() {
                pb.close();
            }
            let (x, y) = map(start.x, start.y);
            pb.move_to(x, y);
        }
        match c {
            OutlineCurve::Line(_, b) => {
                let (x, y) = map(b.x, b.y);
                pb.line_to(x, y);
            }
            OutlineCurve::Quad(_, c1, b) => {
                let (cx, cy) = map(c1.x, c1.y);
                let (x, y) = map(b.x, b.y);
                pb.quad_to(cx, cy, x, y);
            }
            OutlineCurve::Cubic(_, c1, c2, b) => {
                let (ax, ay) = map(c1.x, c1.y);
                let (bx, by) = map(c2.x, c2.y);
                let (x, y) = map(b.x, b.y);
                pb.cubic_to(ax, ay, bx, by, x, y);
            }
        }
        last = Some(end);
    }
    if last.is_some() {
        pb.close();
    }
}

/// Horizontal alignment of a label against its anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
    Right,
}

/// The box a straight label would occupy (baseline at `pos.y` + `cap/2`, i.e. `pos` is the
/// visual centre-left / centre / centre-right of the capitals).
pub fn label_box(fonts: &Fonts, st: &TextStyle, text: &str, pos: Vec2, align: Align) -> Bx {
    let w = measure(fonts, st, text);
    let x0 = match align {
        Align::Left => pos.x,
        Align::Centre => pos.x - w * 0.5,
        Align::Right => pos.x - w,
    };
    let cap = cap_height(st);
    let pad = st.halo.map(|h| h.1 * 0.5).unwrap_or(0.0);
    Bx::new(
        x0 - pad,
        pos.y - cap * 0.5 - st.size * 0.12 - pad,
        x0 + w + pad,
        pos.y + cap * 0.5 + st.size * 0.22 + pad,
    )
}

impl Canvas {
    /// Letters `text` with its visual centre line through `pos`. Returns the occupied box.
    pub fn text(
        &mut self,
        fonts: &Fonts,
        st: &TextStyle,
        text: &str,
        pos: Vec2,
        align: Align,
    ) -> Bx {
        let run = layout(fonts, st, text);
        let font = fonts.font(st.face);
        let x0 = match align {
            Align::Left => pos.x,
            Align::Centre => pos.x - run.width * 0.5,
            Align::Right => pos.x - run.width,
        };
        let base = pos.y + cap_height(st) * 0.5;
        let mut pb = PathBuilder::new();
        for (id, pen, _) in &run.glyphs {
            let s = run.scale;
            let slant = if st.italic { ITALIC_SLANT } else { 0.0 };
            add_glyph(&mut pb, font, *id, |x, y| {
                (x0 + pen + x * s + y * s * slant, base - y * s)
            });
        }
        if let Some(path) = pb.finish() {
            self.letter(&path, st);
        }
        label_box(fonts, st, text, pos, align)
    }

    /// Letters `text` along a polyline (centred on it, upright), e.g. a river name. Returns the
    /// glyph boxes, or `None` when the text is longer than the line.
    pub fn text_along(
        &mut self,
        fonts: &Fonts,
        st: &TextStyle,
        text: &str,
        line: &[Vec2],
        dry_run: bool,
    ) -> Option<Vec<Bx>> {
        let run = layout(fonts, st, text);
        let font = fonts.font(st.face);
        let total: f32 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
        if run.width > total * 0.95 || line.len() < 2 {
            return None;
        }
        // read left to right
        let pts: Vec<Vec2> = if line[0].x > line[line.len() - 1].x {
            line.iter().rev().copied().collect()
        } else {
            line.to_vec()
        };
        let start = (total - run.width) * 0.5;
        let cap = cap_height(st);
        let mut pb = PathBuilder::new();
        let mut boxes = Vec::new();
        for (id, pen, adv) in &run.glyphs {
            let (c, t) = super::field::point_at(&pts, start + pen + adv * 0.5);
            let n = Vec2::new(-t.y, t.x);
            // glyph-local (x along the tangent from the glyph centre, y up from the baseline)
            let s = run.scale;
            let slant = if st.italic { ITALIC_SLANT } else { 0.0 };
            let origin = c + n * (cap * 0.5);
            let half = adv * 0.5;
            add_glyph(&mut pb, font, *id, |x, y| {
                let lx = x * s - half + y * s * slant;
                let ly = y * s;
                let p = origin + t * lx - n * ly;
                (p.x, p.y)
            });
            boxes.push(Bx::around(c, Vec2::splat(st.size * 0.55)));
        }
        if !dry_run && let Some(path) = pb.finish() {
            self.letter(&path, st);
        }
        Some(boxes)
    }

    fn letter(&mut self, path: &Path, st: &TextStyle) {
        if let Some((c, w)) = st.halo {
            let s = Stroke {
                width: w,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            };
            self.pm
                .stroke_path(path, &paint(c), &s, Transform::identity(), None);
        }
        self.fill(path, st.fill);
    }
}
